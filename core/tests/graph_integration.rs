//! Live integration test against the real Microsoft Graph (Microsoft To Do) API.
//! For every task variation it creates a task titled "Test", verifies the server
//! persisted it as sent, and deletes it again - so the account is left clean.
//!
//! It mutates the signed-in account, so it skips under CI (which has no keyring
//! or sign-in). Locally it runs whenever a client id and a stored sign-in are
//! present - run it signed in via the applet, so a refresh token is in the
//! Secret Service:
//!
//! ```sh
//! OUTLOOK_TASKS_CLIENT_ID=<your-client-id> \
//!   cargo test -p outlook-tasks-core --test graph_integration -- --nocapture
//! ```
//!
//! It auto-skips (printing why) under CI, or when the client id or the stored
//! sign-in is missing.

use std::sync::Arc;

use outlook_tasks_core::auth::{AuthConfig, Authenticator, OAuthClient, Oo7TokenStore, TokenStore};
use outlook_tasks_core::graph::{GraphClient, TokenProvider};
use outlook_tasks_core::models::{
    DateTimeTimeZone, Importance, PatternedRecurrence, RecurrencePattern, RecurrencePatternType,
    RecurrenceRange, RecurrenceRangeType, TaskInput, TodoTask,
};

const APP_ID: &str = "dev.robledop.OutlookTasks";
const ACCOUNT_ID: &str = "primary";
const GRAPH_BASE: &str = "https://graph.microsoft.com/v1.0";
/// Every created task uses this due/recurrence-anchor date.
const DUE: &str = "2026-06-17";

#[tokio::test]
async fn create_verify_delete_all_task_variations() {
    if std::env::var_os("CI").is_some() {
        eprintln!("skipping: live Graph integration test does not run under CI");
        return;
    }
    let Ok(client_id) = std::env::var("OUTLOOK_TASKS_CLIENT_ID") else {
        eprintln!("skipping: OUTLOOK_TASKS_CLIENT_ID is not set");
        return;
    };

    // Reuse the stored sign-in (refresh token in the Secret Service).
    let store = Arc::new(Oo7TokenStore::new(APP_ID, ACCOUNT_ID));
    match store.load().await {
        Ok(Some(_)) => {}
        Ok(None) => {
            eprintln!("skipping: not signed in (no refresh token in the keyring)");
            return;
        }
        Err(e) => {
            eprintln!("skipping: keyring unavailable: {e}");
            return;
        }
    }

    let oauth = OAuthClient::new(&AuthConfig::consumers(client_id, "http://localhost/"))
        .expect("build oauth client");
    let auth = Arc::new(Authenticator::new(oauth, store, ACCOUNT_ID));
    let http = reqwest::Client::builder().build().expect("build http client");
    let graph = GraphClient::new(GRAPH_BASE, http, auth as Arc<dyn TokenProvider>);

    // Target the default list (fall back to the first list).
    let lists = graph.list_lists().await.expect("list task lists");
    let list = lists
        .iter()
        .find(|l| l.wellknown_list_name.as_deref() == Some("defaultList"))
        .or_else(|| lists.first())
        .expect("at least one task list");
    let list_id = list.id.clone();
    eprintln!("using list: {} ({})", list.display_name, list_id);

    let mut failures: Vec<String> = Vec::new();

    for case in variations() {
        let created = match graph.create_task(&list_id, &case.input).await {
            Ok(t) => t,
            Err(e) => {
                failures.push(format!("[{}] create failed: {e}", case.name));
                continue;
            }
        };
        // The app stores and re-displays exactly this created task, so verify it.
        match (case.check)(&created) {
            Ok(()) => eprintln!("[{}] OK", case.name),
            Err(e) => failures.push(format!("[{}] {e}", case.name)),
        }
        if let Err(e) = graph.delete_task(&list_id, &created.id).await {
            eprintln!("[{}] WARNING: cleanup delete failed for {}: {e}", case.name, created.id);
        }
    }

    assert!(failures.is_empty(), "{} variation(s) failed:\n{}", failures.len(), failures.join("\n"));
}

/// Asserts the server-persisted task matches what the variation sent.
type Check = Box<dyn Fn(&TodoTask) -> Result<(), String>>;

struct Case {
    name: &'static str,
    input: TaskInput,
    check: Check,
}

fn variations() -> Vec<Case> {
    vec![
        Case {
            name: "plain",
            input: input(None, Importance::Normal, None, None),
            check: Box::new(|t| {
                want("recurrence", t.recurrence.is_some(), false)?;
                want("importance", t.importance, Importance::Normal)?;
                want("has due", t.due_date_time.is_some(), false)
            }),
        },
        Case {
            name: "with-due",
            input: input(Some(due_dtz(DUE)), Importance::Normal, None, None),
            check: Box::new(|t| want("due date", due_date(t), Some(DUE.to_string()))),
        },
        Case {
            name: "importance-high",
            input: input(None, Importance::High, None, None),
            check: Box::new(|t| want("importance", t.importance, Importance::High)),
        },
        Case {
            name: "importance-low",
            input: input(None, Importance::Low, None, None),
            check: Box::new(|t| want("importance", t.importance, Importance::Low)),
        },
        Case {
            name: "recurrence-daily",
            input: input(Some(due_dtz(DUE)), Importance::Normal, Some(daily()), None),
            check: Box::new(|t| want_pattern(t, RecurrencePatternType::Daily)),
        },
        Case {
            name: "recurrence-weekly",
            input: input(Some(due_dtz(DUE)), Importance::Normal, Some(weekly(&["monday", "wednesday"])), None),
            check: Box::new(|t| {
                want_pattern(t, RecurrencePatternType::Weekly)?;
                want_days(t, &["monday", "wednesday"])
            }),
        },
        Case {
            name: "recurrence-monthly-day-of-month",
            input: input(Some(due_dtz(DUE)), Importance::Normal, Some(monthly_absolute(17)), None),
            check: Box::new(|t| {
                want_pattern(t, RecurrencePatternType::AbsoluteMonthly)?;
                want("dayOfMonth", t.recurrence.as_ref().and_then(|r| r.pattern.day_of_month), Some(17))
            }),
        },
        Case {
            name: "recurrence-monthly-nth-weekday",
            input: input(Some(due_dtz(DUE)), Importance::Normal, Some(relative_monthly("first", "monday")), None),
            check: Box::new(|t| {
                want_pattern(t, RecurrencePatternType::RelativeMonthly)?;
                want("index", t.recurrence.as_ref().and_then(|r| r.pattern.index.clone()), Some("first".to_string()))?;
                want_days(t, &["monday"])
            }),
        },
        Case {
            name: "recurrence-yearly-day-of-month",
            input: input(Some(due_dtz(DUE)), Importance::Normal, Some(yearly_absolute(6, 17)), None),
            check: Box::new(|t| {
                want_pattern(t, RecurrencePatternType::AbsoluteYearly)?;
                want("month", t.recurrence.as_ref().and_then(|r| r.pattern.month), Some(6))?;
                want("dayOfMonth", t.recurrence.as_ref().and_then(|r| r.pattern.day_of_month), Some(17))
            }),
        },
        Case {
            name: "recurrence-yearly-nth-weekday",
            input: input(Some(due_dtz(DUE)), Importance::Normal, Some(relative_yearly(6, "first", "monday")), None),
            check: Box::new(|t| {
                want_pattern(t, RecurrencePatternType::RelativeYearly)?;
                want("month", t.recurrence.as_ref().and_then(|r| r.pattern.month), Some(6))?;
                want("index", t.recurrence.as_ref().and_then(|r| r.pattern.index.clone()), Some("first".to_string()))?;
                want_days(t, &["monday"])
            }),
        },
        Case {
            name: "reminder",
            input: input(Some(due_dtz(DUE)), Importance::Normal, None, Some(reminder_dtz(DUE, "09:00"))),
            check: Box::new(|t| {
                want("isReminderOn", t.is_reminder_on, true)?;
                want("reminder date", reminder_date(t), Some(DUE.to_string()))
            }),
        },
        Case {
            name: "kitchen-sink",
            input: input(
                Some(due_dtz(DUE)),
                Importance::High,
                Some(relative_monthly("first", "monday")),
                Some(reminder_dtz(DUE, "08:30")),
            ),
            check: Box::new(|t| {
                want("importance", t.importance, Importance::High)?;
                want_pattern(t, RecurrencePatternType::RelativeMonthly)?;
                want("isReminderOn", t.is_reminder_on, true)?;
                // A relative recurrence makes Graph move the due to the next
                // occurrence, so just assert one is present rather than its value.
                want("has due", t.due_date_time.is_some(), true)
            }),
        },
    ]
}

fn input(
    due: Option<DateTimeTimeZone>,
    importance: Importance,
    recurrence: Option<PatternedRecurrence>,
    reminder: Option<DateTimeTimeZone>,
) -> TaskInput {
    TaskInput { title: "Test".to_string(), importance, due, recurrence, reminder }
}

fn due_dtz(date: &str) -> DateTimeTimeZone {
    DateTimeTimeZone { date_time: format!("{date}T00:00:00.0000000"), time_zone: Some("UTC".into()) }
}

fn reminder_dtz(date: &str, hm: &str) -> DateTimeTimeZone {
    DateTimeTimeZone {
        date_time: format!("{date}T{hm}:00.0000000"),
        time_zone: Some("UTC".into()),
    }
}

/// A `noEnd` range anchored at `DUE`. `TaskInput::to_body` strips the start date
/// from the request (the To Do endpoint rejects it), so the server derives the
/// recurrence start from the due date.
fn range() -> RecurrenceRange {
    RecurrenceRange {
        range_type: RecurrenceRangeType::NoEnd,
        start_date: DUE.into(),
        end_date: None,
        number_of_occurrences: None,
        recurrence_time_zone: Some("UTC".into()),
    }
}

fn pattern(pattern_type: RecurrencePatternType) -> RecurrencePattern {
    RecurrencePattern {
        pattern_type,
        interval: 1,
        month: None,
        day_of_month: None,
        days_of_week: vec![],
        first_day_of_week: None,
        index: None,
    }
}

fn recurrence(pattern: RecurrencePattern) -> PatternedRecurrence {
    PatternedRecurrence { pattern, range: range() }
}

fn daily() -> PatternedRecurrence {
    recurrence(pattern(RecurrencePatternType::Daily))
}

fn weekly(days: &[&str]) -> PatternedRecurrence {
    let mut p = pattern(RecurrencePatternType::Weekly);
    p.days_of_week = days.iter().map(|s| s.to_string()).collect();
    p.first_day_of_week = Some("sunday".into());
    recurrence(p)
}

fn monthly_absolute(day_of_month: u8) -> PatternedRecurrence {
    let mut p = pattern(RecurrencePatternType::AbsoluteMonthly);
    p.day_of_month = Some(day_of_month);
    recurrence(p)
}

fn relative_monthly(index: &str, weekday: &str) -> PatternedRecurrence {
    let mut p = pattern(RecurrencePatternType::RelativeMonthly);
    p.index = Some(index.to_string());
    p.days_of_week = vec![weekday.to_string()];
    recurrence(p)
}

fn yearly_absolute(month: u8, day_of_month: u8) -> PatternedRecurrence {
    let mut p = pattern(RecurrencePatternType::AbsoluteYearly);
    p.month = Some(month);
    p.day_of_month = Some(day_of_month);
    recurrence(p)
}

fn relative_yearly(month: u8, index: &str, weekday: &str) -> PatternedRecurrence {
    let mut p = pattern(RecurrencePatternType::RelativeYearly);
    p.month = Some(month);
    p.index = Some(index.to_string());
    p.days_of_week = vec![weekday.to_string()];
    recurrence(p)
}

fn due_date(t: &TodoTask) -> Option<String> {
    t.due_date_time.as_ref().map(|d| d.date_time.chars().take(10).collect())
}

fn reminder_date(t: &TodoTask) -> Option<String> {
    t.reminder_date_time.as_ref().map(|d| d.date_time.chars().take(10).collect())
}

fn want<T: PartialEq + std::fmt::Debug>(label: &str, got: T, expected: T) -> Result<(), String> {
    if got == expected {
        Ok(())
    } else {
        Err(format!("{label}: got {got:?}, expected {expected:?}"))
    }
}

fn want_pattern(t: &TodoTask, expected: RecurrencePatternType) -> Result<(), String> {
    want("recurrence pattern type", t.recurrence.as_ref().map(|r| r.pattern.pattern_type), Some(expected))
}

fn want_days(t: &TodoTask, expected: &[&str]) -> Result<(), String> {
    let got = t.recurrence.as_ref().map(|r| r.pattern.days_of_week.clone()).unwrap_or_default();
    for day in expected {
        if !got.iter().any(|d| d == day) {
            return Err(format!("daysOfWeek {got:?} missing {day}"));
        }
    }
    Ok(())
}
