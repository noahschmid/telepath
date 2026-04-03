use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use iced::futures::stream;
use ringbuf::{HeapCons, HeapProd, HeapRb, traits::Split};

use crate::app::Message;

// ---------------------------------------------------------------------------
// Device enumeration (output devices)
// ---------------------------------------------------------------------------

pub fn list_output_devices() -> Vec<String> {
    let host = cpal::default_host();
    host.output_devices()
        .map(|devs| devs.filter_map(|d| d.name().ok()).collect())
        .unwrap_or_default()
}

pub fn enumerate_output_devices_stream() -> impl iced::futures::Stream<Item = Message> {
    stream::once(async {
        let devices = tokio::task::spawn_blocking(list_output_devices)
            .await
            .unwrap_or_default();
        Message::DevicesLoaded(devices)
    })
}

/// Query the native sample rate of the named output device.
pub fn output_device_sample_rate(device_name: &str) -> Result<u32, PlaybackError> {
    let host = cpal::default_host();
    let device = host
        .output_devices()?
        .find(|d| d.name().ok().as_deref() == Some(device_name))
        .ok_or_else(|| PlaybackError::DeviceNotFound(device_name.to_string()))?;
    Ok(device.default_output_config()?.sample_rate().0)
}

// ---------------------------------------------------------------------------
// Playback
// ---------------------------------------------------------------------------

/// Start playing audio to the named output device.
///
/// Returns:
///  - A ring buffer producer — the network task pushes received f32 samples here.
///  - A oneshot sender — dropping or sending on it stops playback cleanly.
///
/// cpal::Stream is !Send, so the stream is owned by a dedicated thread.
/// The ring buffer consumer is moved into the cpal callback closure.
pub fn start_playback(
    device_name: &str,
    sample_rate: u32,
) -> Result<(HeapProd<f32>, tokio::sync::oneshot::Sender<()>), PlaybackError> {
    let device_name = device_name.to_string();

    // Validate device exists before spawning.
    {
        let host = cpal::default_host();
        host.output_devices()?
            .find(|d| d.name().ok().as_deref() == Some(&device_name))
            .ok_or_else(|| PlaybackError::DeviceNotFound(device_name.clone()))?;
    }

    // Ring buffer: ~500 ms at the given sample rate.
    // The network task owns the producer; the cpal callback owns the consumer.
    let buf_capacity = (0.5 * sample_rate as f64) as usize;
    let (prod, cons) = HeapRb::<f32>::new(buf_capacity).split();

    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();

    std::thread::Builder::new()
        .name(format!("cpal-playback-{device_name}"))
        .spawn(move || {
            playback_thread(device_name, sample_rate, cons, shutdown_rx);
        })
        .map_err(|e| PlaybackError::ThreadSpawn(e.to_string()))?;

    Ok((prod, shutdown_tx))
}

fn playback_thread(
    device_name: String,
    sample_rate: u32,
    mut cons: HeapCons<f32>,
    shutdown_rx: tokio::sync::oneshot::Receiver<()>,
) {
    use ringbuf::traits::Consumer;

    eprintln!("[audio] playback_thread started for device: {device_name:?}");

    let host = cpal::default_host();
    let device = match host
        .output_devices()
        .ok()
        .and_then(|mut devs| devs.find(|d| d.name().ok().as_deref() == Some(&device_name)))
    {
        Some(d) => d,
        None => {
            eprintln!("[audio] FATAL: output device not found: {device_name:?}");
            return;
        }
    };

    // Use the device's native channel count so we don't have to negotiate with
    // WASAPI/ALSA. We write the same mono sample to every channel.
    let default_config = match device.default_output_config() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[audio] failed to get default output config: {e}");
            return;
        }
    };

    let channels = default_config.channels() as usize;
    let config = cpal::StreamConfig {
        channels: channels as u16,
        sample_rate: cpal::SampleRate(sample_rate),
        buffer_size: cpal::BufferSize::Default,
    };

    let stream = match device.build_output_stream(
        &config,
        move |data: &mut [f32], _| {
            // data is interleaved: [L0, R0, L1, R1, ...]
            // Pop one sample per frame and duplicate to all channels.
            let frames = data.len() / channels;
            for frame in 0..frames {
                let s = cons.try_pop().unwrap_or(0.0);
                for ch in 0..channels {
                    data[frame * channels + ch] = s;
                }
            }
        },
        |err| eprintln!("[audio] output stream error: {err}"),
        None,
    ) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[audio] FATAL: failed to build output stream: {e}");
            return;
        }
    };

    if let Err(e) = stream.play() {
        eprintln!("[audio] FATAL: failed to play stream: {e}");
        return;
    }

    eprintln!("[audio] playback stream running");
    let _ = shutdown_rx.blocking_recv();
    eprintln!("[audio] playback_thread shutting down");
    drop(stream);
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum PlaybackError {
    #[error("device not found: {0}")]
    DeviceNotFound(String),
    #[error("cpal devices error: {0}")]
    Devices(#[from] cpal::DevicesError),
    #[error("default output config error: {0}")]
    Config(#[from] cpal::DefaultStreamConfigError),
    #[error("build stream error: {0}")]
    Build(#[from] cpal::BuildStreamError),
    #[error("thread spawn failed: {0}")]
    ThreadSpawn(String),
}