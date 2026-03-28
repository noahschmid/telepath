use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use nih_plug::prelude::*;
use nih_plug_egui::EguiState;
use ringbuf::{
    traits::{Consumer, Split},
    HeapRb,
};

mod config;
mod editor;
mod params;

use config::ConnectionConfig;
use params::TelepathParams;

// ---------------------------------------------------------------------------
// Connection state — shared between plugin and editor
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ConnState {
    Disconnected,
    Connecting,
    Connected,
}

/// Atomic wrapper so the editor (UI thread) and plugin (audio thread) can
/// read/write connection state without a mutex on the hot path.
pub struct ConnectionState(AtomicU32);

impl ConnectionState {
    pub fn new(s: ConnState) -> Arc<Self> {
        Arc::new(Self(AtomicU32::new(s as u32)))
    }

    pub fn store(&self, s: ConnState) {
        self.0.store(s as u32, Ordering::Release);
    }

    pub fn load(&self) -> ConnState {
        match self.0.load(Ordering::Acquire) {
            1 => ConnState::Connecting,
            2 => ConnState::Connected,
            _ => ConnState::Disconnected,
        }
    }

    pub fn is_connected(&self) -> bool {
        self.load() == ConnState::Connected
    }
}

// ---------------------------------------------------------------------------
// Jitter buffer type aliases
// ---------------------------------------------------------------------------

type JitterProd = ringbuf::HeapProd<f32>;
type JitterCons = ringbuf::HeapCons<f32>;

// ---------------------------------------------------------------------------
// Plugin
// ---------------------------------------------------------------------------

pub struct TelepathPlugin {
    params: Arc<TelepathParams>,

    /// Persisted connection config (host string, port).
    /// Stored here rather than in Params because nih-plug Params only supports
    /// numeric types. Serialized manually via the #[persist] mechanism in params.
    config: Arc<Mutex<ConnectionConfig>>,

    /// Shared with the editor for live status display.
    connection_state: Arc<ConnectionState>,

    /// Total latency in samples reported to the DAW via set_latency_samples().
    latency_samples: Arc<AtomicU32>,
    /// Total latency in microseconds — kept in sync with latency_samples for the editor display.
    total_latency_us: Arc<AtomicU32>,

    /// One-way network latency in **microseconds** (RTT / 2).
    /// Written by the network task, read by process().
    network_latency_us: Arc<AtomicU32>,

    /// Consumer half of the lock-free jitter buffer.
    /// The network receive task owns the producer half.
    jitter_cons: Option<JitterCons>,

    /// Sample rate set by the DAW in initialize().
    sample_rate: f32,

    /// Set by the editor when the user clicks Connect. Polled in process().
    connect_requested: Arc<AtomicBool>,
    /// Set by the editor when the user clicks Disconnect. Polled in process().
    disconnect_requested: Arc<AtomicBool>,

    /// Sending on this signals the network task to send Disconnect and exit.
    /// Replaced each time a session starts; dropping it also triggers shutdown.
    session_shutdown_tx: Option<tokio::sync::oneshot::Sender<()>>,
}

impl Default for TelepathPlugin {
    fn default() -> Self {
        Self {
            params: Arc::new(TelepathParams::default()),
            config: Arc::new(Mutex::new(ConnectionConfig::default())),
            connection_state: ConnectionState::new(ConnState::Disconnected),
            latency_samples: Arc::new(AtomicU32::new(0)),
            total_latency_us: Arc::new(AtomicU32::new(0)),
            network_latency_us: Arc::new(AtomicU32::new(0)),
            jitter_cons: None,
            sample_rate: 44100.0,
            connect_requested: Arc::new(AtomicBool::new(false)),
            disconnect_requested: Arc::new(AtomicBool::new(false)),
            session_shutdown_tx: None,
        }
    }
}

impl TelepathPlugin {
    /// Recompute latency and notify the DAW via set_latency_samples().
    /// Must only be called from initialize() or process() where a context exists.
    fn recompute_latency(&self, context: &mut impl ProcessContext<Self>) {
        let buffer_depth_ms = self.params.buffer_depth_ms.value();
        let network_ms = self.network_latency_us.load(Ordering::Relaxed) as f32 / 1000.0;
        let total_ms = buffer_depth_ms + network_ms;
        let samples = (total_ms / 1000.0 * self.sample_rate).round() as u32;
        self.latency_samples.store(samples, Ordering::Release);
        self.total_latency_us
            .store((total_ms * 1000.0).round() as u32, Ordering::Release);
        context.set_latency_samples(samples);
    }

    /// Spin up the jitter buffer and network receive task for a new session.
    fn start_session(&mut self) {
        nih_log!("[telepath] start_session() called");
        let buf_capacity = (0.5 * 48000.0) as usize;
        let (prod, cons) = HeapRb::<f32>::new(buf_capacity).split();
        self.jitter_cons = Some(cons);

        let cfg = self.config.lock().unwrap().clone();
        let connection_state = Arc::clone(&self.connection_state);
        let network_latency_ms = Arc::clone(&self.network_latency_us);

        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
        self.session_shutdown_tx = Some(shutdown_tx);
        connection_state.store(ConnState::Connecting);

        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("tokio runtime");

            nih_log!("[telepath] network thread running");
            rt.block_on(async move {
                nih_log!("[telepath] run_session() starting");
                match crate::net::run_session(
                    cfg,
                    prod,
                    network_latency_ms,
                    Arc::clone(&connection_state),
                    shutdown_rx,
                )
                .await
                {
                    Ok(()) => {
                        nih_log!("[telepath] run_session() returned Ok — disconnected");
                        connection_state.store(ConnState::Disconnected);
                    }
                    Err(e) => {
                        nih_error!("[telepath] Session error: {e}");
                        connection_state.store(ConnState::Disconnected);
                    }
                }
            });
        });
    }

    fn stop_session(&mut self) {
        // Signal the network task to send PluginMessage::Disconnect and exit.
        // Dropping the sender (by replacing with None) also triggers shutdown.
        if let Some(tx) = self.session_shutdown_tx.take() {
            let _ = tx.send(());
        }
        self.jitter_cons = None;
        self.connection_state.store(ConnState::Disconnected);
    }
}

impl Plugin for TelepathPlugin {
    const NAME: &'static str = "Telepath";
    const VENDOR: &'static str = "Vantix Software";
    const URL: &'static str = "";
    const EMAIL: &'static str = "";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");

    const AUDIO_IO_LAYOUTS: &'static [AudioIOLayout] = &[AudioIOLayout {
        main_input_channels: None,
        main_output_channels: NonZeroU32::new(1),
        ..AudioIOLayout::const_default()
    }];

    const MIDI_INPUT: MidiConfig = MidiConfig::None;
    const SAMPLE_ACCURATE_AUTOMATION: bool = false;

    type SysExMessage = ();
    type BackgroundTask = ();

    fn params(&self) -> Arc<dyn Params> {
        self.params.clone()
    }

    fn editor(&mut self, _async_executor: AsyncExecutor<Self>) -> Option<Box<dyn Editor>> {
        let egui_state = EguiState::from_size(320, 240);
        editor::create(
            egui_state,
            Arc::clone(&self.config),
            Arc::clone(&self.connection_state),
            Arc::clone(&self.total_latency_us),
            Arc::clone(&self.connect_requested),
            Arc::clone(&self.disconnect_requested),
        )
    }

    fn initialize(
        &mut self,
        _audio_io_layout: &AudioIOLayout,
        buffer_config: &BufferConfig,
        context: &mut impl InitContext<Self>,
    ) -> bool {
        self.sample_rate = buffer_config.sample_rate;
        // Report initial latency (0 until a session connects and measures RTT).
        context.set_latency_samples(0);
        true
    }

    fn reset(&mut self) {
        if let Some(cons) = &mut self.jitter_cons {
            while cons.try_pop().is_some() {}
        }
    }

    fn process(
        &mut self,
        buffer: &mut Buffer,
        _aux: &mut AuxiliaryBuffers,
        context: &mut impl ProcessContext<Self>,
    ) -> ProcessStatus {
        // Poll editor→plugin command flags (set by UI thread, consumed here).
        if self.connect_requested.swap(false, Ordering::AcqRel) {
            nih_log!(
                "[telepath] connect_requested consumed in process() — calling start_session()"
            );
            self.start_session();
        }
        if self.disconnect_requested.swap(false, Ordering::AcqRel) {
            nih_log!(
                "[telepath] disconnect_requested consumed in process() — calling stop_session()"
            );
            self.stop_session();
        }

        let gain = self.params.gain.smoothed.next();

        // Recompute latency when the buffer depth knob moves OR when a new
        // ping result arrives (network_latency_us changed since last block).
        let network_us_now = self.network_latency_us.load(Ordering::Relaxed);
        let knob_moving = self.params.buffer_depth_ms.smoothed.is_smoothing();
        // We detect a changed ping result by comparing against latency_samples —
        // if network_us changed the recomputed sample count will differ.
        let expected_samples = {
            let network_ms = network_us_now as f32 / 1000.0;
            let total_ms = self.params.buffer_depth_ms.value() + network_ms;
            (total_ms / 1000.0 * self.sample_rate).round() as u32
        };
        let latency_stale = expected_samples != self.latency_samples.load(Ordering::Relaxed);

        if knob_moving || latency_stale {
            self.recompute_latency(context);
        }

        let Some(cons) = &mut self.jitter_cons else {
            for sample in buffer.iter_samples().flatten() {
                *sample = 0.0;
            }
            return ProcessStatus::Normal;
        };

        for sample in buffer.iter_samples().flatten() {
            *sample = cons.try_pop().unwrap_or(0.0) * gain;
        }

        ProcessStatus::Normal
    }
}

mod net;

// ---------------------------------------------------------------------------
// Export
// ---------------------------------------------------------------------------

impl ClapPlugin for TelepathPlugin {
    const CLAP_ID: &'static str = "com.vantix.telepath";
    const CLAP_DESCRIPTION: Option<&'static str> = Some("Remote recording receiver");
    const CLAP_MANUAL_URL: Option<&'static str> = None;
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES: &'static [ClapFeature] = &[ClapFeature::Instrument, ClapFeature::Utility];
}

impl Vst3Plugin for TelepathPlugin {
    const VST3_CLASS_ID: [u8; 16] = *b"VantixTelepath__";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] =
        &[Vst3SubCategory::Instrument, Vst3SubCategory::Tools];
}

nih_export_clap!(TelepathPlugin);
nih_export_vst3!(TelepathPlugin);
