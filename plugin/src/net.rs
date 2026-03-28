//! Plugin-side network session.
//!
//! Runs inside a dedicated thread (spawned by `TelepathPlugin::start_session`)
//! on a single-threaded tokio runtime, keeping the nih-plug audio thread free.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use ringbuf::traits::Producer;
use ringbuf::HeapProd;
use tokio::net::{TcpStream, UdpSocket};
use tokio::time::{interval, Duration};

use common::framing::{read_message, write_message};
use common::packet::{now_us, AudioPacket, MAX_PACKET_BYTES};
use common::protocol::{
    ClientHello, ClientMessage, HandshakeResult, PluginMessage, ServerHello, PROTOCOL_VERSION,
};

use crate::config::ConnectionConfig;
use crate::{ConnState, ConnectionState};

pub async fn run_session(
    cfg: ConnectionConfig,
    mut prod: HeapProd<f32>,
    network_latency_ms: Arc<AtomicU32>,
    connection_state: Arc<ConnectionState>,
    mut shutdown_rx: tokio::sync::oneshot::Receiver<()>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // ------------------------------------------------------------------
    // 1. TCP connect to host
    // ------------------------------------------------------------------

    let addr = format!("{}:{}", cfg.host, cfg.port);
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
            "host protocol version {} — plugin expects {PROTOCOL_VERSION}",
            server_hello.version
        )
        .into());
    }

    eprintln!(
        "[plugin] host device: {} @ {} Hz",
        server_hello.device_name, server_hello.sample_rate
    );

    // ------------------------------------------------------------------
    // 3. Bind UDP socket — plugin receives audio on this port
    // ------------------------------------------------------------------

    let udp = UdpSocket::bind("0.0.0.0:0").await?;
    let udp_listen_port = udp.local_addr()?.port();

    // ------------------------------------------------------------------
    // 4. Send ClientHello
    // ------------------------------------------------------------------

    write_message(
        &mut tcp_tx,
        &ClientHello {
            version: PROTOCOL_VERSION,
            requested_sample_rate: server_hello.sample_rate,
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

    connection_state.store(ConnState::Connected);
    eprintln!("[plugin] session ready — UDP listening on port {udp_listen_port}");

    // ------------------------------------------------------------------
    // 6. Main session loop
    //
    // IMPORTANT: read_message() must not be polled directly in a select!
    // loop with competing branches. The UDP recv branch fires constantly;
    // dropping the TCP future mid-read corrupts stream alignment.
    // Spawn a dedicated task that reads TCP and sends via a channel.
    // ------------------------------------------------------------------

    let (tcp_in_tx, mut tcp_in_rx) =
        tokio::sync::mpsc::channel::<Result<ClientMessage, String>>(16);

    tokio::spawn(async move {
        loop {
            match read_message::<_, ClientMessage>(&mut tcp_rx).await {
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

    let mut udp_buf = vec![0u8; MAX_PACKET_BYTES];
    let mut ping_seq = 0u32;
    let mut ping_timer = interval(Duration::from_millis(500));
    ping_timer.tick().await; // consume the immediate first tick

    loop {
        tokio::select! {
            // Periodic RTT probe
            _ = ping_timer.tick() => {
                write_message(&mut tcp_tx, &PluginMessage::Ping {
                    seq: ping_seq,
                    timestamp_us: now_us(),
                })
                .await?;
                ping_seq = ping_seq.wrapping_add(1);
            }

            // Incoming TCP from host (via dedicated reader task)
            maybe_msg = tcp_in_rx.recv() => {
                match maybe_msg {
                    None => break Ok(()), // host closed connection
                    Some(Err(e)) => return Err(e.into()),
                    Some(Ok(msg)) => match msg {
                        ClientMessage::Pong { timestamp_us, .. } => {
                            let rtt_us = now_us().saturating_sub(timestamp_us);
                            // Store one-way latency in microseconds for sub-ms precision.
                            // RTT / 2, assuming symmetric path.
                            let one_way_us = (rtt_us / 2) as u32;
                            network_latency_ms.store(one_way_us, Ordering::Release);
                        }
                        ClientMessage::StreamStats { packets_sent, packets_dropped, .. } => {
                            if packets_dropped > 0 {
                                eprintln!(
                                    "[plugin] host stats: {packets_sent} sent, \
                                     {packets_dropped} dropped"
                                );
                            }
                        }
                        ClientMessage::Error(e) => {
                            return Err(format!("host error: {e}").into());
                        }
                    }
                }
            }

            // Incoming UDP audio packet
            result = udp.recv(&mut udp_buf) => {
                let n = result?;
                match AudioPacket::decode(&udp_buf[..n]) {
                    Ok(pkt) => {
                        for &s in &pkt.samples {
                            let _ = prod.try_push(s);
                        }
                    }
                    Err(e) => eprintln!("[plugin] bad UDP packet: {e}"),
                }
            }

            // Disconnect requested by the plugin (user clicked Disconnect)
            _ = &mut shutdown_rx => {
                eprintln!("[plugin] shutdown signal received — sending Disconnect");
                let _ = write_message(&mut tcp_tx, &PluginMessage::Disconnect).await;
                break Ok(());
            }
        }
    }
}
