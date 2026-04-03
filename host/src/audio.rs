use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use iced::futures::stream;

use crate::app::Message;

// ---------------------------------------------------------------------------
// Device enumeration
// ---------------------------------------------------------------------------

pub fn list_input_devices() -> Vec<String> {
    let host = cpal::default_host();
    host.input_devices()
        .map(|devs| devs.filter_map(|d| d.name().ok()).collect())
        .unwrap_or_default()
}

pub fn enumerate_devices_stream() -> impl iced::futures::Stream<Item = Message> {
    stream::once(async {
        let devices = tokio::task::spawn_blocking(list_input_devices)
            .await
            .unwrap_or_default();
        Message::DevicesLoaded(devices)
    })
}

// ---------------------------------------------------------------------------
// Sample rate query (used in ServerHello before capture starts)
// ---------------------------------------------------------------------------

pub fn device_sample_rate(device_name: &str) -> Result<u32, CaptureError> {
    let host = cpal::default_host();
    let device = host
        .input_devices()?
        .find(|d| d.name().ok().as_deref() == Some(device_name))
        .ok_or_else(|| CaptureError::DeviceNotFound(device_name.to_string()))?;
    Ok(device.default_input_config()?.sample_rate().0)
}

// ---------------------------------------------------------------------------
// Capture
// ---------------------------------------------------------------------------

/// Start capturing from the named device.
///
/// Spawns a dedicated non-async thread that owns the `cpal::Stream` for its
/// entire lifetime (cpal::Stream is !Send on Linux/ALSA and cannot be moved
/// across thread or async boundaries). Returns a tokio mpsc Receiver that
/// yields interleaved f32 PCM chunks, and a oneshot Sender used to stop
/// capture: sending on it (or dropping it) causes the thread to drop the
/// stream and exit.
///
/// The cpal callback uses `try_send` — frames are dropped rather than
/// blocking if the network falls behind.
pub fn start_capture(
    device_name: &str,
) -> Result<
    (
        tokio::sync::mpsc::Receiver<Vec<f32>>,
        tokio::sync::oneshot::Sender<()>,
    ),
    CaptureError,
> {
    let device_name = device_name.to_string();

    // Validate device exists before spawning the thread.
    {
        let host = cpal::default_host();
        host.input_devices()?
            .find(|d| d.name().ok().as_deref() == Some(&device_name))
            .ok_or_else(|| CaptureError::DeviceNotFound(device_name.clone()))?;
    }

    let (audio_tx, audio_rx) = tokio::sync::mpsc::channel::<Vec<f32>>(256);
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();

    // Use std::thread::Builder instead of spawn so we can propagate errors.
    std::thread::Builder::new()
        .name(format!("cpal-capture-{device_name}"))
        .spawn(move || {
            capture_thread(device_name, audio_tx, shutdown_rx);
        })
        .map_err(|e| CaptureError::ThreadSpawn(e.to_string()))?;

    Ok((audio_rx, shutdown_tx))
}

/// Runs entirely on its own thread. Builds + plays the cpal stream, then
/// blocks until the shutdown signal arrives (or audio_tx is dropped by the
/// receiver side closing).
fn capture_thread(
    device_name: String,
    audio_tx: tokio::sync::mpsc::Sender<Vec<f32>>,
    shutdown_rx: tokio::sync::oneshot::Receiver<()>,
) {
    use cpal::traits::StreamTrait;

    eprintln!("[audio] capture_thread started for device: {device_name:?}");
    let host = cpal::default_host();
    let device = match host
        .input_devices()
        .ok()
        .and_then(|mut devs| devs.find(|d| d.name().ok().as_deref() == Some(&device_name)))
    {
        Some(d) => d,
        None => {
            eprintln!("[audio] FATAL: device not found in capture thread: {device_name:?}");
            eprintln!(
                "[audio] available devices: {:?}",
                cpal::default_host()
                    .input_devices()
                    .map(|d| d.filter_map(|x| x.name().ok()).collect::<Vec<_>>())
                    .unwrap_or_default()
            );
            return;
        }
    };
    eprintln!("[audio] device found OK");

    let config = match device.default_input_config() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[audio] config error: {e}");
            return;
        }
    };

    let stream_result = match config.sample_format() {
        cpal::SampleFormat::F32 => build_stream::<f32>(&device, &config.into(), audio_tx),
        cpal::SampleFormat::I16 => build_stream::<i16>(&device, &config.into(), audio_tx),
        cpal::SampleFormat::I32 => build_stream::<i32>(&device, &config.into(), audio_tx),
        cpal::SampleFormat::U16 => build_stream::<u16>(&device, &config.into(), audio_tx),
        fmt => {
            eprintln!("[audio] unsupported format: {fmt:?}");
            return;
        }
    };

    let stream = match stream_result {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[audio] FATAL: failed to build stream: {e}");
            return;
        }
    };

    if let Err(e) = stream.play() {
        eprintln!("[audio] FATAL: failed to play stream: {e}");
        return;
    }

    eprintln!("[audio] capture stream running — waiting for shutdown signal");

    // Block until shutdown signal or the sender side disconnects.
    let _ = shutdown_rx.blocking_recv();

    eprintln!("[audio] capture_thread shutting down");
    drop(stream);
}

/// Converts any supported cpal sample type to f32 in [-1.0, 1.0].
trait ToF32Sample: Copy {
    fn to_f32_sample(self) -> f32;
}
impl ToF32Sample for f32 {
    fn to_f32_sample(self) -> f32 {
        self
    }
}
impl ToF32Sample for i16 {
    fn to_f32_sample(self) -> f32 {
        self as f32 / i16::MAX as f32
    }
}
impl ToF32Sample for i32 {
    fn to_f32_sample(self) -> f32 {
        self as f32 / i32::MAX as f32
    }
}
impl ToF32Sample for u16 {
    fn to_f32_sample(self) -> f32 {
        (self as f32 / u16::MAX as f32) * 2.0 - 1.0
    }
}

fn build_stream<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    tx: tokio::sync::mpsc::Sender<Vec<f32>>,
) -> Result<cpal::Stream, CaptureError>
where
    T: cpal::Sample + cpal::SizedSample + ToF32Sample + Send + 'static,
{
    let stream = device.build_input_stream(
        config,
        move |data: &[T], _| {
            let frames: Vec<f32> = data.iter().map(|s| s.to_f32_sample()).collect();
            let _ = tx.try_send(frames);
        },
        |err| eprintln!("[audio] stream error: {err}"),
        None,
    )?;

    stream.play()?;
    Ok(stream)
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum CaptureError {
    #[error("device not found: {0}")]
    DeviceNotFound(String),
    #[error("cpal devices error: {0}")]
    Devices(#[from] cpal::DevicesError),
    #[error("default stream config error: {0}")]
    Config(#[from] cpal::DefaultStreamConfigError),
    #[error("build stream error: {0}")]
    Build(#[from] cpal::BuildStreamError),
    #[error("play stream error: {0}")]
    Play(#[from] cpal::PlayStreamError),
    #[error("unsupported sample format: {0:?}")]
    UnsupportedFormat(cpal::SampleFormat),
    #[error("failed to spawn capture thread: {0}")]
    ThreadSpawn(String),
}
