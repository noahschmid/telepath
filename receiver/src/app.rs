use iced::widget::{button, column, container, pick_list, text, text_input, vertical_space};
use iced::{Color, Element, Subscription, Task, Theme};

use crate::{audio, net};

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
    connection_state: ConnectionState,
    latency_us: u32,
    error: Option<String>,

    output_devices: Vec<String>,
    selected_output: Option<String>,

    input_devices: Vec<String>,
    selected_input: Option<String>,
    return_enabled: bool,
}

impl Default for App {
    fn default() -> Self {
        Self {
            host: "192.168.1.".to_string(),
            port: "7271".to_string(),
            connection_state: ConnectionState::Disconnected,
            latency_us: 0,
            error: None,
            output_devices: vec![],
            selected_output: None,
            input_devices: vec![],
            selected_input: None,
            return_enabled: false,
        }
    }
}

#[derive(Debug, Clone)]
pub enum Message {
    HostChanged(String),
    PortChanged(String),
    ConnectPressed,
    DisconnectPressed,
    Connected,
    Disconnected,
    LatencyUpdated(u32),
    Error(String),
    OutputDevicesLoaded(Vec<String>),
    OutputDeviceSelected(String),
    InputDevicesLoaded(Vec<String>),
    InputDeviceSelected(String),
    ReturnToggled,
}

impl App {
    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::HostChanged(v) => self.host = v,
            Message::PortChanged(v) => {
                if v.chars().all(|c| c.is_ascii_digit()) {
                    self.port = v;
                }
            }
            Message::OutputDevicesLoaded(devs) => {
                if self.selected_output.is_none() {
                    self.selected_output = devs.first().cloned();
                }
                self.output_devices = devs;
            }
            Message::OutputDeviceSelected(d) => self.selected_output = Some(d),
            Message::InputDevicesLoaded(devs) => {
                if self.selected_input.is_none() {
                    self.selected_input = devs.first().cloned();
                    net::set_return_device(self.selected_input.clone());
                }
                self.input_devices = devs;
            }
            Message::InputDeviceSelected(d) => {
                net::set_return_device(Some(d.clone()));
                self.selected_input = Some(d);
            }
            Message::ReturnToggled => {
                self.return_enabled = !self.return_enabled;
                net::set_return_enabled(self.return_enabled);
            }
            Message::ConnectPressed => {
                self.connection_state = ConnectionState::Connecting;
                self.error = None;
                self.latency_us = 0;
                self.return_enabled = false;
                net::set_return_enabled(false);
            }
            Message::DisconnectPressed => {
                net::request_disconnect();
                self.connection_state = ConnectionState::Disconnected;
                self.latency_us = 0;
                self.return_enabled = false;
                net::set_return_enabled(false);
            }
            Message::Connected => self.connection_state = ConnectionState::Connected,
            Message::Disconnected => {
                self.connection_state = ConnectionState::Disconnected;
                self.latency_us = 0;
                self.return_enabled = false;
            }
            Message::LatencyUpdated(us) => self.latency_us = us,
            Message::Error(e) => {
                self.error = Some(e);
                self.connection_state = ConnectionState::Disconnected;
                self.return_enabled = false;
            }
        }
        Task::none()
    }

    pub fn subscription(&self) -> Subscription<Message> {
        let subs = vec![
            Subscription::run(audio::enumerate_output_devices_stream),
            Subscription::run(audio::enumerate_input_devices_stream),
        ];

        if self.connection_state != ConnectionState::Disconnected {
            let port = self.port.parse::<u16>().unwrap_or(7271);
            Subscription::batch(
                subs.into_iter()
                    .chain(std::iter::once(net::session_subscription(
                        self.host.clone(),
                        port,
                        self.selected_output.clone().unwrap_or_default(),
                    ))),
            )
        } else {
            Subscription::batch(subs)
        }
    }

    pub fn theme(&self) -> Theme {
        Theme::Dark
    }

    pub fn view(&self) -> Element<'_, Message> {
        let connected = self.connection_state == ConnectionState::Connected;
        let can_connect = self.connection_state == ConnectionState::Disconnected
            && !self.host.is_empty()
            && self.selected_output.is_some();

        let config_col = column![
            text("Host address").size(12),
            text_input("192.168.x.x", &self.host)
                .on_input(Message::HostChanged)
                .padding(8),
            vertical_space().height(4),
            text("Port").size(12),
            text_input("7271", &self.port)
                .on_input(Message::PortChanged)
                .padding(8),
        ]
        .spacing(4);

        let output_col = column![
            text("Output device (VB-Cable / loopback)").size(12),
            pick_list(
                self.output_devices.as_slice(),
                self.selected_output.as_ref(),
                Message::OutputDeviceSelected,
            )
            .placeholder("Select output device..."),
        ]
        .spacing(4);

        let ret_btn_label = if self.return_enabled {
            "⏹  Stop return"
        } else {
            "▶  Send DAW audio to host"
        };
        let ret_btn = {
            let b = button(text(ret_btn_label).size(13)).padding([8, 20]).style(
                if self.return_enabled {
                    button::danger
                } else {
                    button::secondary
                },
            );
            if connected && self.selected_input.is_some() {
                b.on_press(Message::ReturnToggled)
            } else {
                b
            }
        };
        let return_col = column![
            text("Return stream input (DAW loopback)").size(12),
            pick_list(
                self.input_devices.as_slice(),
                self.selected_input.as_ref(),
                Message::InputDeviceSelected,
            )
            .placeholder("Select input device..."),
            vertical_space().height(4),
            ret_btn,
        ]
        .spacing(4);

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

        let latency_section = if connected {
            let ms = self.latency_us as f32 / 1000.0;
            column![
                vertical_space().height(12),
                text(format!("One-way latency: {ms:.2} ms")).size(13),
                text(format!("→ set DAW track delay to {ms:.2} ms"))
                    .size(11)
                    .color(Color::from_rgb(0.5, 0.5, 0.5)),
            ]
            .spacing(4)
        } else {
            column![]
        };

        let mut col = column![
            text("Telepath Receiver").size(26),
            vertical_space().height(12),
            config_col,
            vertical_space().height(8),
            output_col,
            vertical_space().height(8),
            return_col,
            vertical_space().height(16),
            connect_btn,
            latency_section,
        ]
        .spacing(0)
        .max_width(400);

        if let Some(err) = &self.error {
            col = col.push(vertical_space().height(8)).push(
                text(err.as_str())
                    .size(12)
                    .color(Color::from_rgb(0.85, 0.3, 0.3)),
            );
        }

        container(col).padding(28).into()
    }
}
