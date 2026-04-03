use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use iced::futures::stream;
use ringbuf::{traits::Split, HeapCons, HeapProd, HeapRb};

use crate::app::Message;

// ---------------------------------------------------------------------------
// Output device enumeration
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
        Message::OutputDevicesLoaded(devices)
    })
}

// ---------------------------------------------------------------------------
// Input device enumeration
// ---------------------------------------------------------------------------

pub fn list_input_devices() -> Vec<String> {
    let host = cpal::default_host();
    host.input_devices()
        .map(|devs| devs.filter_map(|d| d.name().ok()).collect())
        .unwrap_or_default()
}

pub fn enumerate_input_devices_stream() -> impl iced::futures::Stream<Item = Message> {
    stream::once(async {
        let devices = tokio::task::spawn_blocking(list_input_devices)
            .await
            .unwrap_or_default();
        Message::InputDevicesLoaded(devices)
    })
}

// ---------------------------------------------------------------------------
// Output device sample rate query
// ---------------------------------------------------------------------------

pub fn output_device_sample_rate(device_name: &str) -> Result<u32, AudioError> {
    let host = cpal::default_host();
    let device = host
        .output_devices()?
        .find(|d| d.name().ok().as_deref() == Some(device_name))
        .ok_or_else(|| AudioError::DeviceNotFound(device_name.to_string()))?;
    Ok(device.default_output_config()?.sample_rate().0)
}

// ---------------------------------------------------------------------------
// Playback
// ---------------------------------------------------------------------------

pub fn start_playback(
    device_name: &str,
    sample_rate: u32,
) -> Result<(HeapProd<f32>, tokio::sync::oneshot::Sender<()>), AudioError> {
    let device_name = device_name.to_string();

    {
        let host = cpal::default_host();
        host.output_devices()?
            .find(|d| d.name().ok().as_deref() == Some(&device_name))
            .ok_or_else(|| AudioError::DeviceNotFound(device_name.clone()))?;
    }

    let buf_capacity = (0.5 * sample_rate as f64) as usize;
    let (prod, cons) = HeapRb::<f32>::new(buf_capacity).split();
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();

    std::thread::Builder::new()
        .name(format!("cpal-playback-{device_name}"))
        .spawn(move || playback_thread(device_name, sample_rate, cons, shutdown_rx))
        .map_err(|e| AudioError::ThreadSpawn(e.to_string()))?;

    Ok((prod, shutdown_tx))
}

fn playback_thread(
    device_name: String,
    sample_rate: u32,
    mut cons: HeapCons<f32>,
    shutdown_rx: tokio::sync::oneshot::Receiver<()>,
) {
    use ringbuf::traits::Consumer;

    let host = cpal::default_host();
    let device = match host
        .output_devices()
        .ok()
        .and_then(|mut d| d.find(|d| d.name().ok().as_deref() == Some(&device_name)))
    {
        Some(d) => d,
        None => {
            eprintln!("[audio] output device not found: {device_name}");
            return;
        }
    };

    let default_config = match device.default_output_config() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[audio] output config error: {e}");
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
            eprintln!("[audio] failed to build output stream: {e}");
            return;
        }
    };

    if let Err(e) = stream.play() {
        eprintln!("[audio] failed to play output stream: {e}");
        return;
    }

    eprintln!("[audio] playback running on {device_name}");
    let _ = shutdown_rx.blocking_recv();
    drop(stream);
}

// ---------------------------------------------------------------------------
// Capture (for return stream)
// ---------------------------------------------------------------------------

pub fn start_capture(
    device_name: &str,
) -> Result<
    (
        tokio::sync::mpsc::Receiver<Vec<f32>>,
        tokio::sync::oneshot::Sender<()>,
    ),
    AudioError,
> {
    let device_name = device_name.to_string();

    {
        let host = cpal::default_host();
        host.input_devices()?
            .find(|d| d.name().ok().as_deref() == Some(&device_name))
            .ok_or_else(|| AudioError::DeviceNotFound(device_name.clone()))?;
    }

    let (audio_tx, audio_rx) = tokio::sync::mpsc::channel::<Vec<f32>>(256);
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();

    std::thread::Builder::new()
        .name(format!("cpal-capture-{device_name}"))
        .spawn(move || capture_thread(device_name, audio_tx, shutdown_rx))
        .map_err(|e| AudioError::ThreadSpawn(e.to_string()))?;

    Ok((audio_rx, shutdown_tx))
}

fn capture_thread(
    device_name: String,
    audio_tx: tokio::sync::mpsc::Sender<Vec<f32>>,
    shutdown_rx: tokio::sync::oneshot::Receiver<()>,
) {
    let host = cpal::default_host();
    let device = match host
        .input_devices()
        .ok()
        .and_then(|mut d| d.find(|d| d.name().ok().as_deref() == Some(&device_name)))
    {
        Some(d) => d,
        None => {
            eprintln!("[audio] input device not found: {device_name}");
            return;
        }
    };

    let config = match device.default_input_config() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[audio] input config error: {e}");
            return;
        }
    };

    let stream = match config.sample_format() {
        cpal::SampleFormat::F32 => build_capture_stream::<f32>(&device, &config.into(), audio_tx),
        cpal::SampleFormat::I16 => build_capture_stream::<i16>(&device, &config.into(), audio_tx),
        cpal::SampleFormat::I32 => build_capture_stream::<i32>(&device, &config.into(), audio_tx),
        cpal::SampleFormat::U16 => build_capture_stream::<u16>(&device, &config.into(), audio_tx),
        fmt => {
            eprintln!("[audio] unsupported format: {fmt:?}");
            return;
        }
    };

    let stream = match stream {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[audio] failed to build capture stream: {e}");
            return;
        }
    };

    if let Err(e) = stream.play() {
        eprintln!("[audio] failed to play capture stream: {e}");
        return;
    }

    eprintln!("[audio] capture running on {device_name}");
    let _ = shutdown_rx.blocking_recv();
    drop(stream);
}

trait ToF32Sample: Copy {
    fn to_f32s(self) -> f32;
}
impl ToF32Sample for f32 {
    fn to_f32s(self) -> f32 {
        self
    }
}
impl ToF32Sample for i16 {
    fn to_f32s(self) -> f32 {
        self as f32 / i16::MAX as f32
    }
}
impl ToF32Sample for i32 {
    fn to_f32s(self) -> f32 {
        self as f32 / i32::MAX as f32
    }
}
impl ToF32Sample for u16 {
    fn to_f32s(self) -> f32 {
        (self as f32 / u16::MAX as f32) * 2.0 - 1.0
    }
}

fn build_capture_stream<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    tx: tokio::sync::mpsc::Sender<Vec<f32>>,
) -> Result<cpal::Stream, AudioError>
where
    T: cpal::Sample + cpal::SizedSample + ToF32Sample + Send + 'static,
{
    let stream = device.build_input_stream(
        config,
        move |data: &[T], _| {
            let frames: Vec<f32> = data.iter().map(|s| s.to_f32s()).collect();
            let _ = tx.try_send(frames);
        },
        |err| eprintln!("[audio] capture stream error: {err}"),
        None,
    )?;
    Ok(stream)
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum AudioError {
    #[error("device not found: {0}")]
    DeviceNotFound(String),
    #[error("cpal devices error: {0}")]
    Devices(#[from] cpal::DevicesError),
    #[error("default config error: {0}")]
    Config(#[from] cpal::DefaultStreamConfigError),
    #[error("build stream error: {0}")]
    Build(#[from] cpal::BuildStreamError),
    #[error("play stream error: {0}")]
    Play(#[from] cpal::PlayStreamError),
    #[error("thread spawn failed: {0}")]
    ThreadSpawn(String),
}
