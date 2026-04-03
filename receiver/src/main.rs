mod app;
mod audio;
mod net;

use app::App;

fn main() -> iced::Result {
    iced::application("Telepath Receiver", App::update, App::view)
        .theme(App::theme)
        .subscription(App::subscription)
        .run()
}