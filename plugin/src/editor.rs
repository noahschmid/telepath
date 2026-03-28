use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use nih_plug::prelude::*;
use nih_plug_egui::{create_egui_editor, egui, EguiState};

use crate::config::ConnectionConfig;

pub fn create(
    egui_state: Arc<EguiState>,
    config: Arc<Mutex<ConnectionConfig>>,
    connection_state: Arc<crate::ConnectionState>,
    latency_ms: Arc<std::sync::atomic::AtomicU32>,
    connect_requested: Arc<AtomicBool>,
    disconnect_requested: Arc<AtomicBool>,
) -> Option<Box<dyn Editor>> {
    create_egui_editor(
        egui_state,
        // Editor-local UI state (ephemeral, not persisted)
        EditorState::default(),
        // build() — called once when the editor window opens
        |_ctx, _editor_state| {},
        // update() — called every frame
        move |ctx, _setter, editor_state| {
            let connect_req = Arc::clone(&connect_requested);
            let disconnect_req = Arc::clone(&disconnect_requested);
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.heading("Telepath");
                ui.add_space(12.0);

                // -----------------------------------------------------------------
                // Connection config
                // -----------------------------------------------------------------
                let mut cfg = config.lock().unwrap();
                let connected = connection_state.is_connected();

                ui.label("Plugin host");
                ui.add_enabled_ui(!connected, |ui| {
                    ui.text_edit_singleline(&mut cfg.host);
                });

                ui.add_space(4.0);
                ui.label("Port");
                ui.add_enabled_ui(!connected, |ui| {
                    let mut port_str = cfg.port.to_string();
                    if ui.text_edit_singleline(&mut port_str).changed() {
                        if let Ok(p) = port_str.parse::<u16>() {
                            cfg.port = p;
                        }
                    }
                });

                ui.add_space(12.0);

                // -----------------------------------------------------------------
                // Connect / disconnect button
                // -----------------------------------------------------------------
                match connection_state.load() {
                    crate::ConnState::Disconnected => {
                        if ui.button("Connect").clicked() {
                            nih_plug::nih_log!("[telepath] Connect button clicked — setting connect_requested");
                    connect_req.store(true, Ordering::Release);
                        }
                    }
                    crate::ConnState::Connecting => {
                        ui.add_enabled_ui(false, |ui| {
                            let _ = ui.button("Connecting...");
                        });
                    }
                    crate::ConnState::Connected => {
                        if ui.button("Disconnect").clicked() {
                            nih_plug::nih_log!("[telepath] Disconnect button clicked — setting disconnect_requested");
                    disconnect_req.store(true, Ordering::Release);
                        }
                    }
                }

                if let Some(err) = &editor_state.last_error {
                    ui.add_space(4.0);
                    ui.colored_label(egui::Color32::from_rgb(220, 80, 80), err);
                }

                // -----------------------------------------------------------------
                // Latency readout
                // -----------------------------------------------------------------
                ui.add_space(16.0);
                ui.separator();
                ui.add_space(8.0);

                let latency_us = latency_ms.load(Ordering::Relaxed); // actually microseconds
                let latency_ms_f = latency_us as f32 / 1000.0;
                ui.label(format!("Reported latency: {latency_ms_f:.2} ms"));
            });
        },
    )
}

/// Ephemeral per-frame flags — the editor sets these, the plugin reads them
/// on the next `process()` call via `AuxiliaryBuffers` context.
/// Stored in the editor's local state (not persisted).
#[derive(Default)]
pub struct EditorState {
    pub connect_requested: bool,
    pub disconnect_requested: bool,
    pub last_error: Option<String>,
}
