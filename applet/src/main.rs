mod app;
mod config;
mod consts;
mod notify;
mod reminders;
mod settings;
mod state;
mod task_form;

fn main() -> cosmic::iced::Result {
    env_logger::init();
    cosmic::applet::run::<app::AppModel>(())
}
