//! Network layer for the Telepath host.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::time::{interval, Duration};

use common::framing::{read_message, write_message};
use common::packet::{now_us, AudioPacket};
use common::protocol::{
    ClientHello, ClientMessage, HandshakeResult, PluginMessage, ServerHello, DEFAULT_PORT,
    PROTOCOL_VERSION,
};

use crate::app::Message;
use crate::audio;

// ---------------------------------------------------------------------------
// Shared state (written by App, read by the subscription task)
// ---------------------------------------------------------------------------

static RECORDING: AtomicBool = AtomicBool::new(false);
static SELECTED_DEVICE: Mutex<Option<String>> = Mutex::new(None);

pub fn set_recording(val: bool) {
    RECORDING.store(val, Ordering::Release);
}

pub fn set_device(dev: Option<String>) {
    *SELECTED_DEVICE.lock().unwrap() = dev;
}

// ---------------------------------------------------------------------------
// Subscription entry point
// ---------------------------------------------------------------------------

/// Creates an iced Subscription that runs the TCP server.
/// `run_with_id` uses `device` as the subscription ID — changing the device
/// tears down the old stream and starts a fresh one.
pub fn server_subscription(device: Option<String>) -> iced::Subscription<Message> {
    iced::Subscription::run_with_id(device.clone(), server_stream(device))
}

fn server_stream(device: Option<String>) -> impl iced::futures::Stream<Item = Message> {
    iced::stream::channel(32, move |mut tx| async move {
        loop {
            match run_server(device.clone(), &mut tx).await {
                Ok(()) => {}
                Err(e) => {
                    let _ = tx.try_send(Message::ServerError(e.to_string()));
                    tokio::time::sleep(Duration::from_secs(3)).await;
                }
            }
        }
    })
}

// ---------------------------------------------------------------------------
// Server loop — binds once, accepts sessions indefinitely
// ---------------------------------------------------------------------------

async fn run_server(
    device: Option<String>,
    tx: &mut iced::futures::channel::mpsc::Sender<Message>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let socket = tokio::net::TcpSocket::new_v4()?;
    socket.set_reuseaddr(true)?;
    socket.bind(format!("0.0.0.0:{DEFAULT_PORT}").parse()?)?;
    let listener = socket.listen(1)?;
    eprintln!("[host] listening on :{DEFAULT_PORT}");

    loop {
        let (stream, peer) = listener.accept().await?;
        let plugin_ip = peer.ip().to_string();
        eprintln!("[host] plugin connected from {peer}");

        match run_session(stream, plugin_ip.clone(), device.clone(), tx).await {
            Ok(()) => eprintln!("[host] session ended cleanly"),
            Err(e) => eprintln!("[host] session error: {e}"),
        }

        let _ = tx.try_send(Message::PluginDisconnected);
        RECORDING.store(false, Ordering::Release);
    }
}

// ---------------------------------------------------------------------------
// Session — handshake + audio loop for one plugin connection
// ---------------------------------------------------------------------------

async fn run_session(
    stream: TcpStream,
    plugin_ip: String,
    device: Option<String>,
    tx: &mut iced::futures::channel::mpsc::Sender<Message>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let (mut tcp_rx, mut tcp_tx) = stream.into_split();

    // ------------------------------------------------------------------
    // 1. Handshake
    // ------------------------------------------------------------------

    let device_name = device.as_deref().ok_or("no audio device selected")?;

    let sample_rate = tokio::task::spawn_blocking({
        let name = device_name.to_string();
        move || audio::device_sample_rate(&name)
    })
    .await??;

    write_message(
        &mut tcp_tx,
        &ServerHello {
            version: PROTOCOL_VERSION,
            sample_rate,
            channels: 1,
            device_name: device_name.to_string(),
        },
    )
    .await?;

    let client_hello: ClientHello = read_message(&mut tcp_rx).await?;

    if client_hello.version != PROTOCOL_VERSION {
        write_message(
            &mut tcp_tx,
            &HandshakeResult::Error(common::protocol::SessionError::VersionMismatch {
                got: client_hello.version,
                expected: PROTOCOL_VERSION,
            }),
        )
        .await?;
        return Err("protocol version mismatch".into());
    }

    write_message(&mut tcp_tx, &HandshakeResult::Ready).await?;

    let plugin_udp_addr = format!("{}:{}", plugin_ip, client_hello.udp_listen_port);
    eprintln!("[host] handshake OK — audio → {plugin_udp_addr}");
    let _ = tx.try_send(Message::PluginConnected(plugin_ip.clone()));

    // ------------------------------------------------------------------
    // 2. Audio capture + UDP socket
    // ------------------------------------------------------------------

    // start_capture spawns its own thread that owns the !Send cpal::Stream.
    // shutdown_tx stops the stream when dropped or sent on.
    let (mut audio_rx, shutdown_tx) =
        audio::start_capture(device_name).map_err(|e| format!("capture: {e}"))?;

    let udp = UdpSocket::bind("0.0.0.0:0").await?;
    udp.connect(&plugin_udp_addr).await?;

    // ------------------------------------------------------------------
    // 3. Main session loop
    //
    // IMPORTANT: read_message() must never be polled inside a select! loop
    // that has competing branches — if the audio branch wins, the TCP future
    // is dropped mid-read, leaving the stream misaligned on the next call.
    // Solution: spawn a dedicated task for TCP reads and feed results through
    // a channel so TCP reads are never interrupted.
    // ------------------------------------------------------------------

    let (tcp_in_tx, mut tcp_in_rx) =
        tokio::sync::mpsc::channel::<Result<PluginMessage, String>>(16);

    tokio::spawn(async move {
        loop {
            match read_message::<_, PluginMessage>(&mut tcp_rx).await {
                Ok(msg) => {
                    if tcp_in_tx.send(Ok(msg)).await.is_err() {
                        break; // session ended
                    }
                }
                Err(e) => {
                    let _ = tcp_in_tx.send(Err(e.to_string())).await;
                    break;
                }
            }
        }
    });

    let mut seq = 0u32;
    let mut packets_sent = 0u64;
    let mut packets_dropped = 0u32;
    let mut stats_timer = interval(Duration::from_secs(1));
    stats_timer.tick().await;
    let mut level_peak = 0.0f32;
    let mut level_report_countdown = 0usize;

    loop {
        tokio::select! {
            // Audio chunk from cpal
            maybe_frames = audio_rx.recv() => {
                let Some(frames) = maybe_frames else {
                    eprintln!("[host] audio_rx closed — capture thread exited unexpectedly");
                    return Err("audio device closed".into());
                };

                let chunk_peak = frames.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
                level_peak = level_peak.max(chunk_peak);

                level_report_countdown = level_report_countdown.saturating_sub(frames.len());
                if level_report_countdown == 0 {
                    let db = 20.0 * level_peak.max(1e-7_f32).log10();
                    let _ = tx.try_send(Message::LevelUpdated(db));
                    level_peak = 0.0;
                    level_report_countdown = (sample_rate / 10) as usize;
                }

                if !RECORDING.load(Ordering::Acquire) {
                    continue;
                }

                // cpal may deliver large ALSA period chunks — split into
                // MAX_FRAMES-sized packets so encode() never rejects them.
                for chunk in frames.chunks(common::packet::MAX_FRAMES) {
                    let pkt = AudioPacket {
                        seq,
                        timestamp_us: now_us(),
                        samples: chunk.to_vec(),
                    };
                    match pkt.encode() {
                        Ok(bytes) => {
                            if udp.send(&bytes).await.is_ok() {
                                packets_sent += 1;
                            } else {
                                packets_dropped += 1;
                            }
                        }
                        Err(e) => {
                            eprintln!("[host] encode error: {e}");
                            packets_dropped += 1;
                        }
                    }
                    seq = seq.wrapping_add(1);
                }
            }

            // Incoming TCP message from plugin (via dedicated reader task)
            maybe_msg = tcp_in_rx.recv() => {
                match maybe_msg {
                    None => {
                        eprintln!("[host] TCP reader task exited");
                        break;
                    }
                    Some(Err(e)) => return Err(e.into()),
                    Some(Ok(msg)) => match msg {
                        PluginMessage::Ping { seq, timestamp_us } => {
                            write_message(&mut tcp_tx, &ClientMessage::Pong {
                                seq,
                                timestamp_us,
                            })
                            .await?;
                        }
                        PluginMessage::Disconnect => {
                            eprintln!("[host] plugin requested disconnect");
                            break;
                        }
                        PluginMessage::RecordStart | PluginMessage::RecordStop => {}
                    }
                }
            }

            // Periodic StreamStats to plugin
            _ = stats_timer.tick() => {
                eprintln!("[host] sending StreamStats");
                let db = 20.0 * level_peak.max(1e-7_f32).log10();
                write_message(&mut tcp_tx, &ClientMessage::StreamStats {
                    packets_sent,
                    packets_dropped,
                    level_dbfs_x100: (db * 100.0) as i16,
                })
                .await?;
            }
        }
    }

    // Dropping shutdown_tx signals the capture thread to stop.
    drop(shutdown_tx);
    Ok(())
}
