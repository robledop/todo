use cosmic::widget;

use crate::app::Message;
use crate::state::AccountInfo;

/// The settings screen: an account section plus a sign-out action. A titled column
/// with a back button, mirroring the create/edit form so more sections can be
/// appended later.
pub fn settings_view(
    account: &AccountInfo,
    confirming_sign_out: bool,
    sign_out_error: Option<&str>,
) -> cosmic::Element<'static, Message> {
    let header = widget::Row::new()
        .push(
            widget::button::icon(widget::icon::from_name("go-previous-symbolic"))
                .on_press(Message::CloseSettings),
        )
        .push(widget::text::title4("Settings"))
        .align_y(cosmic::iced::Alignment::Center)
        .spacing(8);

    let account_section = widget::Column::new()
        .push(widget::text::caption("Account"))
        .push(account_card(account))
        .spacing(4);

    let sign_out: cosmic::Element<'static, Message> = if confirming_sign_out {
        widget::Row::new()
            .push(widget::button::destructive("Sign out").on_press(Message::SignOutConfirmed))
            .push(widget::button::text("Cancel").on_press(Message::SignOutCancelled))
            .align_y(cosmic::iced::Alignment::Center)
            .spacing(8)
            .into()
    } else {
        widget::button::standard("Sign out").on_press(Message::SignOutRequested).into()
    };

    let mut col = widget::Column::new()
        .push(header)
        .push(widget::divider::horizontal::default())
        .push(account_section)
        .push(sign_out)
        .spacing(12)
        .padding(12);

    if let Some(err) = sign_out_error {
        col = col.push(
            widget::text::caption(err.to_string())
                .class(cosmic::theme::Text::Color(cosmic::iced::Color::from_rgb(0.8, 0.2, 0.2))),
        );
    }
    col.into()
}

/// Renders the account section body for each load state.
fn account_card(account: &AccountInfo) -> cosmic::Element<'static, Message> {
    match account {
        AccountInfo::NotLoaded | AccountInfo::Loading => {
            widget::text::body("Loading account...").into()
        }
        AccountInfo::Loaded(profile) => {
            let mut col = widget::Column::new().spacing(2);
            match (profile.name(), profile.email()) {
                (Some(name), Some(email)) => {
                    col = col.push(widget::text::body(name.to_string()));
                    col = col.push(widget::text::caption(email.to_string()));
                }
                (Some(name), None) => col = col.push(widget::text::body(name.to_string())),
                (None, Some(email)) => col = col.push(widget::text::body(email.to_string())),
                (None, None) => col = col.push(widget::text::body("Signed in")),
            }
            col.into()
        }
        AccountInfo::Unavailable => widget::Column::new()
            .push(widget::text::body("Signed in"))
            .push(widget::text::caption("Sign out and back in to show account details."))
            .spacing(2)
            .into(),
    }
}
