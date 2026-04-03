use std::sync::atomic::{AtomicBool, Ordering};

use ringbuf::traits::Producer;
use tokio::net::{TcpStream, UdpSocket};
use tokio::time::{interval, Duration};

use common::framing::{read_message, write_message};
use common::packet::{now_us, AudioPacket, MAX_PACKET_BYTES};
use common::protocol::{
    ClientHello, HandshakeResult, HostMessage, ReceiverMessage, ServerHello, DEFAULT_PORT,
    PROTOCOL_VERSION,
};

use crate::app::Message;
use crate::audio;

// ---------------------------------------------------------------------------
// Disconnect flag — set by App::update(DisconnectPressed)
// ---------------------------------------------------------------------------

static DISCONNECT_REQUESTED: AtomicBool = AtomicBool::new(false);
static RETURN_ENABLED: AtomicBool = AtomicBool::new(false);
static RETURN_DEVICE: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

pub fn request_disconnect() {
    DISCONNECT_REQUESTED.store(true, Ordering::Release);
}

pub fn set_return_enabled(val: bool) {
    RETURN_ENABLED.store(val, Ordering::Release);
}

pub fn set_return_device(dev: Option<String>) {
    *RETURN_DEVICE.lock().unwrap() = dev;
}

// ---------------------------------------------------------------------------
// Subscription
// ---------------------------------------------------------------------------

pub fn session_subscription(
    host: String,
    port: u16,
    device: String,
) -> iced::Subscription<Message> {
    iced::Subscription::run_with_id(
        (host.clone(), port, device.clone()),
        session_stream(host, port, device),
    )
}

fn session_stream(
    host: String,
    port: u16,
    device: String,
) -> impl iced::futures::Stream<Item = Message> {
    iced::stream::channel(32, move |mut tx| async move {
        DISCONNECT_REQUESTED.store(false, Ordering::Release);

        match run_session(host, port, device, &mut tx).await {
            Ok(()) => {
                let _ = tx.try_send(Message::Disconnected);
            }
            Err(e) => {
                let _ = tx.try_send(Message::Error(e.to_string()));
            }
        }
    })
}

// ---------------------------------------------------------------------------
// Session
// ---------------------------------------------------------------------------

async fn run_session(
    host: String,
    port: u16,
    device: String,
    tx: &mut iced::futures::channel::mpsc::Sender<Message>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // ------------------------------------------------------------------
    // 1. TCP connect
    // ------------------------------------------------------------------
    let addr = format!("{host}:{port}");
    let stream = TcpStream::connect(&addr)
        .await
        .map_err(|e| format!("TCP connect to {addr} failed: {e}"))?;

    let (mut tcp_rx, mut tcp_tx) = stream.into_split();

    // ------------------------------------------------------------------
    // 2. Receive ServerHello
    // ------------------------------------------------------------------
    let server_hello: ServerHello = read_message(&mut tcp_rx).await?;

    if server_hello.version != PROTOCOL_VERSION {
        return Err(format!(
            "host protocol version {} — receiver expects {PROTOCOL_VERSION}",
            server_hello.version
        )
        .into());
    }

    eprintln!(
        "[receiver] host device: {} @ {} Hz",
        server_hello.device_name, server_hello.sample_rate
    );

    // ------------------------------------------------------------------
    // 3. Query our output device sample rate + bind UDP
    // ------------------------------------------------------------------
    let device_clone = device.clone();
    let output_sample_rate =
        tokio::task::spawn_blocking(move || audio::output_device_sample_rate(&device_clone))
            .await?
            .map_err(|e| format!("output device error: {e}"))?;

    let udp = UdpSocket::bind("0.0.0.0:0").await?;
    let udp_listen_port = udp.local_addr()?.port();

    // ------------------------------------------------------------------
    // 4. Send ClientHello
    // ------------------------------------------------------------------
    write_message(
        &mut tcp_tx,
        &ClientHello {
            version: PROTOCOL_VERSION,
            requested_sample_rate: output_sample_rate,
            udp_listen_port,
        },
    )
    .await?;

    // ------------------------------------------------------------------
    // 5. Receive HandshakeResult
    // ------------------------------------------------------------------
    let result: HandshakeResult = read_message(&mut tcp_rx).await?;
    match result {
        HandshakeResult::Ready => {}
        HandshakeResult::Error(e) => return Err(e.to_string().into()),
    }

    eprintln!("[receiver] session ready — UDP on port {udp_listen_port}");
    let _ = tx.try_send(Message::Connected);

    // Spawn return stream task — captures DAW loopback and sends to host.
    let return_host_addr = format!("{host}:{}", server_hello.return_udp_port);
    let return_task = tokio::spawn(return_stream_task(
        return_host_addr,
        server_hello.sample_rate,
    ));

    // ------------------------------------------------------------------
    // 6. Start audio playback
    // ------------------------------------------------------------------
    // Use the host's sample rate (what it will actually send).
    let (mut audio_prod, _playback_shutdown) =
        audio::start_playback(&device, server_hello.sample_rate)
            .map_err(|e| format!("playback start failed: {e}"))?;

    // ------------------------------------------------------------------
    // 7. TCP reader task (avoids dropping partial reads in select!)
    // ------------------------------------------------------------------
    let (tcp_in_tx, mut tcp_in_rx) = tokio::sync::mpsc::channel::<Result<HostMessage, String>>(16);

    tokio::spawn(async move {
        loop {
            match read_message::<_, HostMessage>(&mut tcp_rx).await {
                Ok(msg) => {
                    if tcp_in_tx.send(Ok(msg)).await.is_err() {
                        break;
                    }
                }
                Err(e) => {
                    let _ = tcp_in_tx.send(Err(e.to_string())).await;
                    break;
                }
            }
        }
    });

    // ------------------------------------------------------------------
    // 8. Main session loop
    // ------------------------------------------------------------------
    let mut udp_buf = vec![0u8; MAX_PACKET_BYTES];
    let mut ping_seq = 0u32;
    let mut ping_timer = interval(Duration::from_millis(500));
    ping_timer.tick().await;

    loop {
        // Check for user-requested disconnect
        if DISCONNECT_REQUESTED.swap(false, Ordering::AcqRel) {
            eprintln!("[receiver] disconnect requested — sending Disconnect");
            let _ = write_message(&mut tcp_tx, &ReceiverMessage::Disconnect).await;
            break;
        }

        tokio::select! {
            // Ping timer
            _ = ping_timer.tick() => {
                write_message(&mut tcp_tx, &ReceiverMessage::Ping {
                    seq: ping_seq,
                    timestamp_us: now_us(),
                })
                .await?;
                ping_seq = ping_seq.wrapping_add(1);
            }

            // TCP from host
            maybe_msg = tcp_in_rx.recv() => {
                match maybe_msg {
                    None => {
                        eprintln!("[receiver] TCP reader closed");
                        break;
                    }
                    Some(Err(e)) => return Err(e.into()),
                    Some(Ok(msg)) => match msg {
                        HostMessage::Pong { timestamp_us, .. } => {
                            let rtt_us = now_us().saturating_sub(timestamp_us);
                            let one_way_us = (rtt_us / 2) as u32;
                            let _ = tx.try_send(Message::LatencyUpdated(one_way_us));
                        }
                        HostMessage::StreamStats { packets_dropped, .. } => {
                            if packets_dropped > 0 {
                                eprintln!("[receiver] {packets_dropped} packets dropped by host");
                            }
                        }
                        HostMessage::Error(e) => {
                            return Err(format!("host error: {e}").into());
                        }
                    }
                }
            }

            // Incoming UDP audio
            result = udp.recv(&mut udp_buf) => {
                let n = result?;
                match AudioPacket::decode(&udp_buf[..n]) {
                    Ok(pkt) => {
                        for &s in &pkt.samples {
                            let _ = audio_prod.try_push(s);
                        }
                    }
                    Err(e) => eprintln!("[receiver] bad UDP packet: {e}"),
                }
            }
        }
    }

    return_task.abort();
    Ok(())
}

// ---------------------------------------------------------------------------
// Return stream task
// Captures DAW playback from the selected input device and streams it
// back to the host over UDP. Runs independently; toggled via RETURN_ENABLED.
// ---------------------------------------------------------------------------

async fn return_stream_task(host_addr: String, sample_rate: u32) {
    use common::packet::{now_us, AudioPacket, MAX_FRAMES};
    use ringbuf::traits::Producer;

    let udp = match tokio::net::UdpSocket::bind("0.0.0.0:0").await {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[return] failed to bind UDP: {e}");
            return;
        }
    };
    if let Err(e) = udp.connect(&host_addr).await {
        eprintln!("[return] failed to connect to {host_addr}: {e}");
        return;
    }

    let mut capture_rx: Option<tokio::sync::mpsc::Receiver<Vec<f32>>> = None;
    let mut capture_shutdown: Option<tokio::sync::oneshot::Sender<()>> = None;
    let mut currently_enabled = false;
    let mut seq = 0u32;

    loop {
        let enabled = RETURN_ENABLED.load(Ordering::Acquire);

        // Start or stop capture when toggle changes
        if enabled != currently_enabled {
            if enabled {
                let device = RETURN_DEVICE.lock().unwrap().clone();
                if let Some(dev) = device {
                    match crate::audio::start_capture(&dev) {
                        Ok((rx, shutdown)) => {
                            capture_rx = Some(rx);
                            capture_shutdown = Some(shutdown);
                            eprintln!("[return] capture started on {dev}");
                        }
                        Err(e) => eprintln!("[return] capture failed: {e}"),
                    }
                }
            } else {
                drop(capture_shutdown.take());
                capture_rx = None;
                eprintln!("[return] capture stopped");
            }
            currently_enabled = enabled;
        }

        if let Some(ref mut rx) = capture_rx {
            match rx.recv().await {
                Some(frames) => {
                    for chunk in frames.chunks(MAX_FRAMES) {
                        let pkt = AudioPacket {
                            seq,
                            timestamp_us: now_us(),
                            samples: chunk.to_vec(),
                        };
                        if let Ok(bytes) = pkt.encode() {
                            let _ = udp.send(&bytes).await;
                        }
                        seq = seq.wrapping_add(1);
                    }
                }
                None => {
                    // Capture device disappeared
                    eprintln!("[return] capture device closed unexpectedly");
                    capture_shutdown = None;
                    capture_rx = None;
                    currently_enabled = false;
                    RETURN_ENABLED.store(false, Ordering::Release);
                }
            }
        } else {
            // Nothing to send — sleep to avoid busy-loop
            tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
        }
    }
}
