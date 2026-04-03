use iced::widget::{button, column, container, pick_list, text, vertical_space};
use iced::{Color, Element, Subscription, Task, Theme};

use crate::{audio, net};

#[derive(Debug, Clone, PartialEq)]
pub enum ServerState {
    Listening,
    Connected(String),
    Recording(String),
}

#[derive(Debug)]
pub struct App {
    // Capture (mic → receiver)
    devices: Vec<String>,
    selected_device: Option<String>,
    server_state: ServerState,
    level_db: f32,

    // Monitoring (DAW playback → Scarlett)
    output_devices: Vec<String>,
    selected_output: Option<String>,
    monitoring: bool,

    error: Option<String>,
}

impl Default for App {
    fn default() -> Self {
        Self {
            devices: vec![],
            selected_device: None,
            server_state: ServerState::Listening,
            level_db: -60.0,
            output_devices: vec![],
            selected_output: None,
            monitoring: false,
            error: None,
        }
    }
}

#[derive(Debug, Clone)]
pub enum Message {
    // Capture side
    DevicesLoaded(Vec<String>),
    DeviceSelected(String),
    PluginConnected(String),
    PluginDisconnected,
    RecordToggled,
    LevelUpdated(f32),
    ServerError(String),

    // Monitoring side
    OutputDevicesLoaded(Vec<String>),
    OutputDeviceSelected(String),
    MonitorToggled,
}

impl App {
    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::DevicesLoaded(devs) => {
                if self.selected_device.is_none() {
                    self.selected_device = devs.first().cloned();
                    net::set_device(self.selected_device.clone());
                }
                self.devices = devs;
            }
            Message::DeviceSelected(dev) => {
                net::set_device(Some(dev.clone()));
                self.selected_device = Some(dev);
            }
            Message::OutputDevicesLoaded(devs) => {
                if self.selected_output.is_none() {
                    self.selected_output = devs.first().cloned();
                    net::set_output_device(self.selected_output.clone());
                }
                self.output_devices = devs;
            }
            Message::OutputDeviceSelected(dev) => {
                net::set_output_device(Some(dev.clone()));
                self.selected_output = Some(dev);
            }
            Message::MonitorToggled => {
                self.monitoring = !self.monitoring;
                net::set_monitoring(self.monitoring);
            }
            Message::PluginConnected(addr) => {
                self.server_state = ServerState::Connected(addr);
                self.error = None;
            }
            Message::PluginDisconnected => {
                net::set_recording(false);
                net::set_monitoring(false);
                self.monitoring = false;
                self.server_state = ServerState::Listening;
                self.level_db = -60.0;
            }
            Message::RecordToggled => match &self.server_state {
                ServerState::Connected(addr) => {
                    let addr = addr.clone();
                    net::set_recording(true);
                    self.server_state = ServerState::Recording(addr);
                }
                ServerState::Recording(addr) => {
                    let addr = addr.clone();
                    net::set_recording(false);
                    self.server_state = ServerState::Connected(addr);
                }
                ServerState::Listening => {}
            },
            Message::LevelUpdated(db) => self.level_db = db,
            Message::ServerError(e) => {
                self.error = Some(e);
                self.server_state = ServerState::Listening;
                net::set_recording(false);
                net::set_monitoring(false);
                self.monitoring = false;
            }
        }
        Task::none()
    }

    pub fn subscription(&self) -> Subscription<Message> {
        Subscription::batch([
            Subscription::run(audio::enumerate_devices_stream),
            Subscription::run(audio::enumerate_output_devices_stream),
            net::server_subscription(self.selected_device.clone()),
        ])
    }

    pub fn theme(&self) -> Theme {
        Theme::Dark
    }

    pub fn view(&self) -> Element<'_, Message> {
        let connected = matches!(
            &self.server_state,
            ServerState::Connected(_) | ServerState::Recording(_)
        );

        // ── Capture section ───────────────────────────────────────────────
        let capture_col = column![
            text("Audio input (mic)").size(12),
            pick_list(
                self.devices.as_slice(),
                self.selected_device.as_ref(),
                Message::DeviceSelected,
            )
            .placeholder("Select input..."),
        ]
        .spacing(4);

        // ── Output / monitor section ───────────────────────────────────────
        let output_col = column![
            text("Audio output (DAW monitor)").size(12),
            pick_list(
                self.output_devices.as_slice(),
                self.selected_output.as_ref(),
                Message::OutputDeviceSelected,
            )
            .placeholder("Select output..."),
        ]
        .spacing(4);

        // ── Status ─────────────────────────────────────────────────────────
        let status = match &self.server_state {
            ServerState::Listening => "Waiting for receiver connection...".to_string(),
            ServerState::Connected(addr) => format!("Receiver connected — {addr}"),
            ServerState::Recording(addr) => format!("● Streaming — {addr}"),
        };

        // ── Record button ──────────────────────────────────────────────────
        let (rec_label, rec_recording) = match &self.server_state {
            ServerState::Recording(_) => ("⏹  STOP", true),
            _ => ("⏺  REC", false),
        };
        let rec_btn = {
            let b = button(text(rec_label).size(20))
                .padding([18, 44])
                .style(if rec_recording {
                    button::danger
                } else {
                    button::primary
                });
            if connected {
                b.on_press(Message::RecordToggled)
            } else {
                b
            }
        };

        // ── Monitor button ─────────────────────────────────────────────────
        let mon_btn = {
            let label = if self.monitoring {
                "⏹  Stop monitor"
            } else {
                "▶  Monitor DAW"
            };
            let b = button(text(label).size(14))
                .padding([10, 24])
                .style(if self.monitoring {
                    button::danger
                } else {
                    button::secondary
                });
            if connected && self.selected_output.is_some() {
                b.on_press(Message::MonitorToggled)
            } else {
                b
            }
        };

        let mut content = column![
            text("Telepath Host").size(26),
            vertical_space().height(12),
            capture_col,
            vertical_space().height(8),
            output_col,
            vertical_space().height(12),
            text(status).size(12),
            vertical_space().height(24),
            rec_btn,
            vertical_space().height(12),
            mon_btn,
        ]
        .spacing(0)
        .align_x(iced::Alignment::Center)
        .max_width(360);

        if connected {
            content = content
                .push(vertical_space().height(16))
                .push(text(format!("Level  {:.1} dBFS", self.level_db)).size(12));
        }

        if let Some(err) = &self.error {
            content = content.push(vertical_space().height(8)).push(
                text(err.as_str())
                    .size(12)
                    .color(Color::from_rgb(0.85, 0.3, 0.3)),
            );
        }

        container(content).padding(28).center_x(iced::Fill).into()
    }
}
