//! Pure logic for deciding which task reminders have just come due. The view
//! layer drives this from a timer; the notification side effect lives in
//! `crate::notify`.

use jiff::Timestamp;
use outlook_tasks_core::models::{TaskStatus, TodoTask};

/// The instant a task's reminder is set for, or `None` if the reminder is off,
/// absent, or the stored date/zone can't be parsed.
///
/// `reminderDateTime` is a wall-clock time (`2026-06-15T09:00:00.0000000`) in an
/// IANA zone; we resolve it to an absolute instant so it can be compared to now.
pub fn reminder_instant(task: &TodoTask) -> Option<Timestamp> {
    if !task.is_reminder_on {
        return None;
    }
    let dtz = task.reminder_date_time.as_ref()?;
    // Drop any fractional seconds; reminders are minute-granular.
    let civil = dtz.date_time.get(..19).unwrap_or(dtz.date_time.as_str());
    let dt: jiff::civil::DateTime = civil.parse().ok()?;
    let tz = match dtz.time_zone.as_deref() {
        Some(name) => jiff::tz::TimeZone::get(name).ok()?,
        None => jiff::tz::TimeZone::system(),
    };
    dt.to_zoned(tz).ok().map(|z| z.timestamp())
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
