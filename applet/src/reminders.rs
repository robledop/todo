//! Pure logic for deciding which task reminders have just come due. The view
//! layer drives this from a timer; the notification side effect lives in
//! `crate::notify`.

use jiff::Timestamp;
use outlook_tasks_core::models::{DateTimeTimeZone, TaskStatus, TodoTask};

/// The instant a task's reminder is set for, or `None` if the reminder is off,
/// absent, or the stored date/zone can't be parsed.
///
/// `reminderDateTime` is a wall-clock time (`2026-06-15T09:00:00.0000000`) in an
/// IANA zone; we resolve it to an absolute instant so it can be compared to now.
pub fn reminder_instant(task: &TodoTask) -> Option<Timestamp> {
    if !task.is_reminder_on {
        return None;
    }
    to_instant(task.reminder_date_time.as_ref()?)
}

/// Resolves a stored `dateTimeTimeZone` (a wall-clock time plus its zone name) to
/// an absolute instant. `None` if the date or zone can't be parsed.
fn to_instant(dtz: &DateTimeTimeZone) -> Option<Timestamp> {
    // Drop any fractional seconds; reminders are minute-granular.
    let civil = dtz.date_time.get(..19).unwrap_or(dtz.date_time.as_str());
    let dt: jiff::civil::DateTime = civil.parse().ok()?;
    let tz = match dtz.time_zone.as_deref() {
        Some(name) => resolve_zone(name)?,
        None => jiff::tz::TimeZone::system(),
    };
    dt.to_zoned(tz).ok().map(|z| z.timestamp())
}

/// Re-expresses a stored `dateTimeTimeZone` as the equivalent wall-clock time in
/// the `local` zone (an IANA or Windows zone name). Graph echoes reminder/due
/// times back in UTC, so the form uses this to show the local time the user set
/// rather than the UTC time on the wire. `None` if the stored value or `local`
/// can't be parsed.
pub fn local_civil(dtz: &DateTimeTimeZone, local: &str) -> Option<jiff::civil::DateTime> {
    let target = resolve_zone(local)?;
    Some(to_instant(dtz)?.to_zoned(target).datetime())
}

/// Resolves a stored zone name to a `TimeZone`. IANA names resolve directly;
/// Windows zone names (as Outlook stores them, e.g. "Pacific Standard Time")
/// are mapped to their IANA equivalent first, so reminders created in Outlook
/// still fire instead of silently failing to parse.
fn resolve_zone(name: &str) -> Option<jiff::tz::TimeZone> {
    if let Ok(tz) = jiff::tz::TimeZone::get(name) {
        return Some(tz);
    }
    jiff::tz::TimeZone::get(windows_to_iana(name)?).ok()
}

/// Maps a Windows time-zone id to its primary IANA zone (the CLDR `windowsZones`
/// "001" default). Covers the populated zones; an unmapped name yields `None`.
fn windows_to_iana(name: &str) -> Option<&'static str> {
    let iana = match name {
        "Dateline Standard Time" => "Etc/GMT+12",
        "UTC-11" => "Etc/GMT+11",
        "Hawaiian Standard Time" => "Pacific/Honolulu",
        "Alaskan Standard Time" => "America/Anchorage",
        "Pacific Standard Time (Mexico)" => "America/Tijuana",
        "Pacific Standard Time" => "America/Los_Angeles",
        "US Mountain Standard Time" => "America/Phoenix",
        "Mountain Standard Time (Mexico)" => "America/Chihuahua",
        "Mountain Standard Time" => "America/Denver",
        "Central America Standard Time" => "America/Guatemala",
        "Central Standard Time" => "America/Chicago",
        "Central Standard Time (Mexico)" => "America/Mexico_City",
        "Canada Central Standard Time" => "America/Regina",
        "SA Pacific Standard Time" => "America/Bogota",
        "Eastern Standard Time" => "America/New_York",
        "US Eastern Standard Time" => "America/Indiana/Indianapolis",
        "Venezuela Standard Time" => "America/Caracas",
        "Atlantic Standard Time" => "America/Halifax",
        "SA Western Standard Time" => "America/La_Paz",
        "Newfoundland Standard Time" => "America/St_Johns",
        "E. South America Standard Time" => "America/Sao_Paulo",
        "Argentina Standard Time" => "America/Argentina/Buenos_Aires",
        "SA Eastern Standard Time" => "America/Cayenne",
        "Greenland Standard Time" => "America/Godthab",
        "UTC" => "Etc/UTC",
        "Cape Verde Standard Time" => "Atlantic/Cape_Verde",
        "Morocco Standard Time" => "Africa/Casablanca",
        "GMT Standard Time" => "Europe/London",
        "Greenwich Standard Time" => "Atlantic/Reykjavik",
        "W. Europe Standard Time" => "Europe/Berlin",
        "Central Europe Standard Time" => "Europe/Budapest",
        "Romance Standard Time" => "Europe/Paris",
        "Central European Standard Time" => "Europe/Warsaw",
        "W. Central Africa Standard Time" => "Africa/Lagos",
        "GTB Standard Time" => "Europe/Bucharest",
        "E. Europe Standard Time" => "Europe/Chisinau",
        "Egypt Standard Time" => "Africa/Cairo",
        "South Africa Standard Time" => "Africa/Johannesburg",
        "FLE Standard Time" => "Europe/Kiev",
        "Israel Standard Time" => "Asia/Jerusalem",
        "Turkey Standard Time" => "Europe/Istanbul",
        "Belarus Standard Time" => "Europe/Minsk",
        "Arabic Standard Time" => "Asia/Baghdad",
        "Arab Standard Time" => "Asia/Riyadh",
        "Russian Standard Time" => "Europe/Moscow",
        "E. Africa Standard Time" => "Africa/Nairobi",
        "Iran Standard Time" => "Asia/Tehran",
        "Arabian Standard Time" => "Asia/Dubai",
        "Azerbaijan Standard Time" => "Asia/Baku",
        "Caucasus Standard Time" => "Asia/Yerevan",
        "Afghanistan Standard Time" => "Asia/Kabul",
        "West Asia Standard Time" => "Asia/Tashkent",
        "Pakistan Standard Time" => "Asia/Karachi",
        "India Standard Time" => "Asia/Kolkata",
        "Sri Lanka Standard Time" => "Asia/Colombo",
        "Nepal Standard Time" => "Asia/Kathmandu",
        "Central Asia Standard Time" => "Asia/Almaty",
        "Bangladesh Standard Time" => "Asia/Dhaka",
        "Myanmar Standard Time" => "Asia/Yangon",
        "SE Asia Standard Time" => "Asia/Bangkok",
        "China Standard Time" => "Asia/Shanghai",
        "Singapore Standard Time" => "Asia/Singapore",
        "Taipei Standard Time" => "Asia/Taipei",
        "W. Australia Standard Time" => "Australia/Perth",
        "Korea Standard Time" => "Asia/Seoul",
        "Tokyo Standard Time" => "Asia/Tokyo",
        "Cen. Australia Standard Time" => "Australia/Adelaide",
        "AUS Central Standard Time" => "Australia/Darwin",
        "E. Australia Standard Time" => "Australia/Brisbane",
        "AUS Eastern Standard Time" => "Australia/Sydney",
        "Tasmania Standard Time" => "Australia/Hobart",
        "West Pacific Standard Time" => "Pacific/Port_Moresby",
        "Central Pacific Standard Time" => "Pacific/Guadalcanal",
        "New Zealand Standard Time" => "Pacific/Auckland",
        "Fiji Standard Time" => "Pacific/Fiji",
        "Tonga Standard Time" => "Pacific/Tongatapu",
        _ => return None,
    };
    Some(iana)
}

/// Tasks whose reminder instant falls in the half-open window `(lower, now]` and
/// that are reminder-on and not completed - i.e. reminders that crossed their
/// time since the last check and should fire exactly once now.
pub fn due_reminders(tasks: &[TodoTask], lower: Timestamp, now: Timestamp) -> Vec<&TodoTask> {
    tasks
        .iter()
        .filter(|t| t.status != TaskStatus::Completed)
        .filter(|t| reminder_instant(t).is_some_and(|rt| rt > lower && rt <= now))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use outlook_tasks_core::models::DateTimeTimeZone;

    fn ts(s: &str) -> Timestamp {
        s.parse().unwrap()
    }

    fn reminder(on: bool, date_time: &str, tz: Option<&str>, status: TaskStatus) -> TodoTask {
        TodoTask {
            id: "T1".into(),
            title: "Pay rent".into(),
            status,
            is_reminder_on: on,
            reminder_date_time: Some(DateTimeTimeZone {
                date_time: date_time.into(),
                time_zone: tz.map(Into::into),
            }),
            ..TodoTask::default()
        }
    }

    #[test]
    fn instant_resolves_iana_zone_to_utc() {
        // 09:00 in America/Sao_Paulo (UTC-3) is 12:00 UTC.
        let t = reminder(true, "2026-06-15T09:00:00.0000000", Some("America/Sao_Paulo"), TaskStatus::NotStarted);
        assert_eq!(reminder_instant(&t), Some(ts("2026-06-15T12:00:00Z")));
    }

    #[test]
    fn instant_handles_utc() {
        let t = reminder(true, "2026-06-15T09:00:00.0000000", Some("UTC"), TaskStatus::NotStarted);
        assert_eq!(reminder_instant(&t), Some(ts("2026-06-15T09:00:00Z")));
    }

    #[test]
    fn instant_resolves_windows_zone_name() {
        // Outlook stores the Windows id "Pacific Standard Time"; 2026-06-15 is in
        // PDT (UTC-7), so 09:00 local resolves to 16:00 UTC.
        let t = reminder(
            true,
            "2026-06-15T09:00:00.0000000",
            Some("Pacific Standard Time"),
            TaskStatus::NotStarted,
        );
        assert_eq!(reminder_instant(&t), Some(ts("2026-06-15T16:00:00Z")));
    }

    #[test]
    fn instant_none_on_unknown_windows_zone() {
        let t = reminder(
            true,
            "2026-06-15T09:00:00",
            Some("Narnia Standard Time"),
            TaskStatus::NotStarted,
        );
        assert_eq!(reminder_instant(&t), None);
    }

    #[test]
    fn instant_none_when_reminder_off() {
        let t = reminder(false, "2026-06-15T09:00:00.0000000", Some("UTC"), TaskStatus::NotStarted);
        assert_eq!(reminder_instant(&t), None);
    }

    #[test]
    fn instant_none_on_unparsable_date_or_zone() {
        assert_eq!(reminder_instant(&reminder(true, "not-a-date", Some("UTC"), TaskStatus::NotStarted)), None);
        assert_eq!(
            reminder_instant(&reminder(true, "2026-06-15T09:00:00", Some("Nowhere/Nope"), TaskStatus::NotStarted)),
            None
        );
    }

    #[test]
    fn window_is_lower_exclusive_now_inclusive() {
        let lower = ts("2026-06-15T12:00:00Z");
        let now = ts("2026-06-15T12:00:30Z");
        let at_lower = reminder(true, "2026-06-15T12:00:00", Some("UTC"), TaskStatus::NotStarted); // excluded
        let at_now = reminder(true, "2026-06-15T12:00:30", Some("UTC"), TaskStatus::NotStarted); // included
        let future = reminder(true, "2026-06-15T12:01:00", Some("UTC"), TaskStatus::NotStarted); // excluded
        let tasks = vec![at_lower, at_now, future];
        let due = due_reminders(&tasks, lower, now);
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].reminder_date_time.as_ref().unwrap().date_time, "2026-06-15T12:00:30");
    }

    #[test]
    fn completed_tasks_never_remind() {
        let lower = ts("2026-06-15T11:59:00Z");
        let now = ts("2026-06-15T12:01:00Z");
        let done = reminder(true, "2026-06-15T12:00:00", Some("UTC"), TaskStatus::Completed);
        assert!(due_reminders(std::slice::from_ref(&done), lower, now).is_empty());
    }
}
