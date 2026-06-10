mod app;
mod config;
mod consts;
mod state;

fn main() -> cosmic::iced::Result {
    env_logger::init();
    cosmic::applet::run::<app::AppModel>(())
}
