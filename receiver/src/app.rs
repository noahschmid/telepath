use iced::widget::{button, column, container, pick_list, row, text, text_input, vertical_space};
use iced::{Color, Element, Subscription, Task, Theme};

use crate::{audio, net};

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Clone, PartialEq)]
pub enum ConnectionState {
    #[default]
    Disconnected,
    Connecting,
    Connected,
}

#[derive(Debug)]
pub struct App {
    host: String,
    port: String,
    devices: Vec<String>,
    selected_device: Option<String>,
    connection_state: ConnectionState,
    /// One-way network latency in microseconds.
    latency_us: u32,
    error: Option<String>,
}

impl Default for App {
    fn default() -> Self {
        Self {
            host: "192.168.1.".to_string(),
            port: "7271".to_string(),
            devices: vec![],
            selected_device: None,
            connection_state: ConnectionState::Disconnected,
            latency_us: 0,
            error: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Messages
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum Message {
    HostChanged(String),
    PortChanged(String),
    DevicesLoaded(Vec<String>),
    DeviceSelected(String),
    ConnectPressed,
    DisconnectPressed,
    Connected,
    Disconnected,
    LatencyUpdated(u32),
    Error(String),
}

// ---------------------------------------------------------------------------
// Update
// ---------------------------------------------------------------------------

impl App {
    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::HostChanged(v) => self.host = v,
            Message::PortChanged(v) => {
                if v.chars().all(|c| c.is_ascii_digit()) {
                    self.port = v;
                }
            }
            Message::DevicesLoaded(devs) => {
                if self.selected_device.is_none() {
                    self.selected_device = devs.first().cloned();
                }
                self.devices = devs;
            }
            Message::DeviceSelected(d) => self.selected_device = Some(d),
            Message::ConnectPressed => {
                self.connection_state = ConnectionState::Connecting;
                self.error = None;
                self.latency_us = 0;
            }
            Message::DisconnectPressed => {
                net::request_disconnect();
                self.connection_state = ConnectionState::Disconnected;
                self.latency_us = 0;
            }
            Message::Connected => self.connection_state = ConnectionState::Connected,
            Message::Disconnected => {
                self.connection_state = ConnectionState::Disconnected;
                self.latency_us = 0;
            }
            Message::LatencyUpdated(us) => self.latency_us = us,
            Message::Error(e) => {
                self.error = Some(e);
                self.connection_state = ConnectionState::Disconnected;
                self.latency_us = 0;
            }
        }
        Task::none()
    }

    pub fn subscription(&self) -> Subscription<Message> {
        let device_enum = Subscription::run(audio::enumerate_output_devices_stream);

        // Only run the network subscription while connecting or connected.
        if self.connection_state != ConnectionState::Disconnected {
            let port = self.port.parse::<u16>().unwrap_or(7271);
            Subscription::batch([
                device_enum,
                net::session_subscription(
                    self.host.clone(),
                    port,
                    self.selected_device.clone().unwrap_or_default(),
                ),
            ])
        } else {
            device_enum
        }
    }

    pub fn theme(&self) -> Theme {
        Theme::Dark
    }

    // ---------------------------------------------------------------------------
    // View
    // ---------------------------------------------------------------------------

    pub fn view(&self) -> Element<Message> {
        let can_connect = self.connection_state == ConnectionState::Disconnected
            && !self.host.is_empty()
            && self.selected_device.is_some();

        let connect_btn = match self.connection_state {
            ConnectionState::Disconnected => button("Connect")
                .padding([10, 32])
                .on_press_maybe(can_connect.then_some(Message::ConnectPressed)),
            ConnectionState::Connecting => button("Connecting...").padding([10, 32]),
            ConnectionState::Connected => button("Disconnect")
                .padding([10, 32])
                .style(button::danger)
                .on_press(Message::DisconnectPressed),
        };

        let latency_row = if self.connection_state == ConnectionState::Connected {
            let ms = self.latency_us as f32 / 1000.0;
            let hint = format!("→ set DAW track delay to {ms:.2} ms");
            column![
                vertical_space().height(16),
                text(format!("One-way latency:  {ms:.2} ms")).size(13),
                text(hint).size(11).color(Color::from_rgb(0.5, 0.5, 0.5)),
            ]
            .spacing(4)
        } else {
            column![]
        };

        let mut col = column![
            text("Telepath Receiver").size(26),
            vertical_space().height(16),
            text("Host address").size(12),
            text_input("192.168.x.x", &self.host)
                .on_input(Message::HostChanged)
                .padding(8),
            vertical_space().height(6),
            text("Port").size(12),
            text_input("7271", &self.port)
                .on_input(Message::PortChanged)
                .padding(8),
            vertical_space().height(6),
            text("Output device (virtual cable / loopback)").size(12),
            pick_list(
                self.devices.as_slice(),
                self.selected_device.as_ref(),
                Message::DeviceSelected,
            )
            .placeholder("Select output device..."),
            vertical_space().height(16),
            connect_btn,
            latency_row,
        ]
        .spacing(4)
        .max_width(380);

        if let Some(err) = &self.error {
            col = col
                .push(vertical_space().height(8))
                .push(text(err.as_str()).size(12).color(Color::from_rgb(0.85, 0.3, 0.3)));
        }

        container(col).padding(28).into()
    }
}