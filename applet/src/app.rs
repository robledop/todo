use cosmic::app::Core;
use cosmic::iced::platform_specific::shell::wayland::commands::popup::{destroy_popup, get_popup};
use cosmic::iced::window::Id;
use cosmic::iced::{Limits, Subscription};
use cosmic::prelude::*;
use cosmic::widget;

use crate::config::Config;
use crate::consts::APP_ID;

pub struct AppModel {
    core: Core,
    popup: Option<Id>,
    #[allow(dead_code)]
    config: Config,
}

#[derive(Debug, Clone)]
pub enum Message {
    TogglePopup,
    PopupClosed(Id),
    UpdateConfig(Config),
}

impl cosmic::Application for AppModel {
    type Executor = cosmic::executor::Default;
    type Flags = ();
    type Message = Message;
    const APP_ID: &'static str = APP_ID;

    fn core(&self) -> &Core {
        &self.core
    }
    fn core_mut(&mut self) -> &mut Core {
        &mut self.core
    }

    fn init(core: Core, _flags: ()) -> (Self, Task<cosmic::Action<Message>>) {
        let model = AppModel { core, popup: None, config: Config::load() };
        (model, Task::none())
    }

    fn on_close_requested(&self, id: Id) -> Option<Message> {
        Some(Message::PopupClosed(id))
    }

    fn view(&self) -> Element<'_, Message> {
        self.core
            .applet
            .icon_button("checkbox-checked-symbolic")
            .on_press(Message::TogglePopup)
            .into()
    }

    fn view_window(&self, _id: Id) -> Element<'_, Message> {
        let content = widget::list_column().add(widget::text::body("Outlook Tasks"));
        self.core.applet.popup_container(content).into()
    }

    fn subscription(&self) -> Subscription<Message> {
        self.core()
            .watch_config::<Config>(APP_ID)
            .map(|update| Message::UpdateConfig(update.config))
    }

    fn update(&mut self, message: Message) -> Task<cosmic::Action<Message>> {
        match message {
            Message::UpdateConfig(config) => {
                self.config = config;
            }
            Message::TogglePopup => {
                if let Some(p) = self.popup.take() {
                    return destroy_popup(p);
                }
                // Never unwrap the main window id - a missing parent must not panic
                // (the panel would respawn-loop). Just skip opening the popup.
                let Some(parent) = self.core.main_window_id() else {
                    log::warn!("no main window id; cannot open popup");
                    return Task::none();
                };
                let new_id = Id::unique();
                self.popup = Some(new_id);
                let mut settings =
                    self.core.applet.get_popup_settings(parent, new_id, None, None, None);
                settings.positioner.size_limits = Limits::NONE
                    .max_width(372.0)
                    .min_width(300.0)
                    .min_height(200.0)
                    .max_height(1080.0);
                return get_popup(settings);
            }
            Message::PopupClosed(id) => {
                if self.popup == Some(id) {
                    self.popup = None;
                }
            }
        }
        Task::none()
    }

    fn style(&self) -> Option<cosmic::iced::theme::Style> {
        Some(cosmic::applet::style())
    }
}
