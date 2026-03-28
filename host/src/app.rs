use iced::widget::{button, column, container, pick_list, row, text, vertical_space};
use iced::{Color, Element, Subscription, Task, Theme};

use crate::{audio, net};

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub enum ServerState {
    /// TCP server is bound and waiting for a plugin connection.
    Listening,
    /// Plugin connected, not yet streaming.
    Connected(String),
    /// Actively capturing and streaming audio to the plugin.
    Recording(String),
}

#[derive(Debug)]
pub struct App {
    devices: Vec<String>,
    selected_device: Option<String>,
    server_state: ServerState,
    level_db: f32,
    rtt_ms: Option<f32>,
    error: Option<String>,
}

impl Default for App {
    fn default() -> Self {
        Self {
            devices: vec![],
            selected_device: None,
            server_state: ServerState::Listening,
            level_db: -60.0,
            rtt_ms: None,
            error: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Messages
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum Message {
    DevicesLoaded(Vec<String>),
    DeviceSelected(String),
    /// Plugin connected from the given IP address.
    PluginConnected(String),
    PluginDisconnected,
    RecordToggled,
    LevelUpdated(f32),
    RttUpdated(f32),
    ServerError(String),
}

// ---------------------------------------------------------------------------
// Update
// ---------------------------------------------------------------------------

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
            Message::PluginConnected(addr) => {
                self.server_state = ServerState::Connected(addr);
                self.error = None;
            }
            Message::PluginDisconnected => {
                net::set_recording(false);
                self.server_state = ServerState::Listening;
                self.rtt_ms = None;
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
            Message::RttUpdated(ms) => self.rtt_ms = Some(ms),
            Message::ServerError(e) => {
                self.error = Some(e);
                self.server_state = ServerState::Listening;
                net::set_recording(false);
            }
        }
        Task::none()
    }

    pub fn subscription(&self) -> Subscription<Message> {
        Subscription::batch([
            Subscription::run(audio::enumerate_devices_stream),
            net::server_subscription(self.selected_device.clone()),
        ])
    }

    pub fn theme(&self) -> Theme {
        Theme::Dark
    }

    // ---------------------------------------------------------------------------
    // View
    // ---------------------------------------------------------------------------

    pub fn view(&self) -> Element<Message> {
        let device_row = column![
            text("Audio input").size(12),
            pick_list(
                self.devices.as_slice(),
                self.selected_device.as_ref(),
                Message::DeviceSelected,
            )
            .placeholder("Select audio input device...")
        ]
        .spacing(4);

        let status = match &self.server_state {
            ServerState::Listening => "Waiting for plugin connection...".to_string(),
            ServerState::Connected(addr) => format!("Plugin connected — {addr}"),
            ServerState::Recording(addr) => format!("● Streaming — {addr}"),
        };

        let record_btn = self.record_button();

        let mut content = column![
            text("Telepath Host").size(26),
            vertical_space().height(16),
            device_row,
            vertical_space().height(12),
            text(status),
            vertical_space().height(32),
            record_btn,
        ]
        .spacing(0)
        .align_x(iced::Alignment::Center)
        .max_width(340);

        // Stats — only visible while connected or recording
        if matches!(
            &self.server_state,
            ServerState::Connected(_) | ServerState::Recording(_)
        ) {
            let rtt = self.rtt_ms.map_or("—".into(), |ms| format!("{ms:.1} ms"));

            content = content.push(vertical_space().height(24)).push(
                row![
                    text(format!("Level  {:.1} dBFS", self.level_db)).size(12),
                    text(format!("   RTT  {rtt}")).size(12),
                ]
                .spacing(0),
            );
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

    fn record_button(&self) -> Element<Message> {
        let (label, is_recording, enabled) = match &self.server_state {
            ServerState::Listening => ("⏺  REC", false, false),
            ServerState::Connected(_) => ("⏺  REC", false, true),
            ServerState::Recording(_) => ("⏹  STOP", true, true),
        };

        let btn = button(text(label).size(22))
            .padding([22, 52])
            .style(if is_recording {
                button::danger
            } else {
                button::primary
            });

        if enabled {
            btn.on_press(Message::RecordToggled).into()
        } else {
            btn.into()
        }
    }
}
