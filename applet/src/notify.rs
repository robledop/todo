//! Desktop notifications via the freedesktop `org.freedesktop.Notifications`
//! service (zbus). Used to alert the user when a task reminder comes due.

use std::collections::HashMap;

use zbus::zvariant::Value;

/// Named icon shown on the notification; a standard freedesktop name so it
/// recolors with the theme and falls back gracefully if absent.
const APP_ICON: &str = "appointment-soon";
/// Auto-dismiss timeout in milliseconds (-1 would defer to the daemon default).
const TIMEOUT_MS: i32 = 5000;

/// Shows a single desktop notification. Fire-and-forget: the caller logs any
/// error rather than surfacing it.
pub async fn notify(summary: String, body: String) -> zbus::Result<()> {
    let conn = zbus::Connection::session().await?;
    let actions: Vec<&str> = Vec::new();
    let hints: HashMap<&str, Value> = HashMap::new();
    // Notify(app_name, replaces_id, app_icon, summary, body, actions, hints, expire_timeout)
    let args = (
        "Outlook Tasks",
        0u32,
        APP_ICON,
        summary.as_str(),
        body.as_str(),
        actions,
        hints,
        TIMEOUT_MS,
    );
    conn.call_method(
        Some("org.freedesktop.Notifications"),
        "/org/freedesktop/Notifications",
        Some("org.freedesktop.Notifications"),
        "Notify",
        &args,
    )
    .await?;
    Ok(())
}
