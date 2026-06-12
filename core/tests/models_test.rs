use outlook_tasks_core::models::{GraphCollection, TaskStatus, TodoList, TodoTask};

#[test]
fn deserializes_todo_list_collection() {
    let json = r#"{"value":[{"id":"L1","displayName":"Tasks","wellknownListName":"defaultList"}]}"#;
    let coll: GraphCollection<TodoList> = serde_json::from_str(json).unwrap();
    assert_eq!(coll.value.len(), 1);
    assert_eq!(coll.value[0].id, "L1");
    assert_eq!(coll.value[0].display_name, "Tasks");
    assert_eq!(coll.value[0].wellknown_list_name.as_deref(), Some("defaultList"));
    assert!(coll.next_link.is_none());
}

#[test]
fn parses_next_link() {
    let json = r#"{"value":[],"@odata.nextLink":"https://x/page2"}"#;
    let coll: GraphCollection<TodoTask> = serde_json::from_str(json).unwrap();
    assert_eq!(coll.next_link.as_deref(), Some("https://x/page2"));
}

#[test]
fn task_status_roundtrips_and_tolerates_unknown() {
    assert_eq!(serde_json::to_string(&TaskStatus::Completed).unwrap(), "\"completed\"");
    let s: TaskStatus = serde_json::from_str("\"waitingOnOthers\"").unwrap();
    assert_eq!(s, TaskStatus::WaitingOnOthers);
    let u: TaskStatus = serde_json::from_str("\"someFutureValue\"").unwrap();
    assert_eq!(u, TaskStatus::Unknown);
}

#[test]
fn todo_task_ignores_unknown_fields() {
    let json = r#"{"id":"T1","title":"Buy milk","status":"notStarted","importance":"low","extra":123}"#;
    let t: TodoTask = serde_json::from_str(json).unwrap();
    assert_eq!(t.id, "T1");
    assert_eq!(t.title, "Buy milk");
    assert_eq!(t.status, TaskStatus::NotStarted);
}

#[test]
fn todo_task_parses_last_modified_date_time() {
    let json = r#"{"id":"T1","title":"x","status":"completed","lastModifiedDateTime":"2026-06-10T00:00:00Z"}"#;
    let t: TodoTask = serde_json::from_str(json).unwrap();
    assert_eq!(t.last_modified_date_time.as_deref(), Some("2026-06-10T00:00:00Z"));
}

#[test]
fn todo_task_parses_due_date_and_day() {
    let json = r#"{"id":"T1","title":"x","status":"notStarted","dueDateTime":{"dateTime":"2026-06-15T00:00:00.0000000","timeZone":"UTC"}}"#;
    let t: TodoTask = serde_json::from_str(json).unwrap();
    assert_eq!(t.due_day(), Some("2026-06-15"));
}

#[test]
fn todo_task_without_due_has_no_day() {
    let json = r#"{"id":"T1","title":"x","status":"notStarted"}"#;
    let t: TodoTask = serde_json::from_str(json).unwrap();
    assert_eq!(t.due_day(), None);
}

use outlook_tasks_core::models::Importance;

#[test]
fn importance_roundtrips_camelcase_and_defaults_normal() {
    assert_eq!(serde_json::to_string(&Importance::High).unwrap(), "\"high\"");
    let i: Importance = serde_json::from_str("\"low\"").unwrap();
    assert_eq!(i, Importance::Low);
    assert_eq!(Importance::default(), Importance::Normal);
    let u: Importance = serde_json::from_str("\"weird\"").unwrap();
    assert_eq!(u, Importance::Unknown); // forward-compatible
}

use outlook_tasks_core::models::{
    PatternedRecurrence, RecurrencePattern, RecurrencePatternType, RecurrenceRange,
    RecurrenceRangeType,
};

#[test]
fn weekly_recurrence_serializes_to_graph_shape() {
    let rec = PatternedRecurrence {
        pattern: RecurrencePattern {
            pattern_type: RecurrencePatternType::Weekly,
            interval: 2,
            month: None,
            day_of_month: None,
            days_of_week: vec!["monday".into(), "thursday".into()],
            first_day_of_week: Some("sunday".into()),
            index: None,
        },
        range: RecurrenceRange {
            range_type: RecurrenceRangeType::NoEnd,
            start_date: "2026-06-20".into(),
            end_date: None,
            number_of_occurrences: None,
            recurrence_time_zone: Some("UTC".into()),
        },
    };
    let v = serde_json::to_value(&rec).unwrap();
    assert_eq!(v["pattern"]["type"], "weekly");
    assert_eq!(v["pattern"]["interval"], 2);
    assert_eq!(v["pattern"]["daysOfWeek"], serde_json::json!(["monday", "thursday"]));
    assert_eq!(v["range"]["type"], "noEnd");
    assert_eq!(v["range"]["startDate"], "2026-06-20");
    // unset numeric/string fields are omitted
    assert!(v["pattern"].get("dayOfMonth").is_none());
    assert!(v["range"].get("endDate").is_none());
}

#[test]
fn recurrence_deserializes_relative_monthly() {
    let json = r#"{"pattern":{"type":"relativeMonthly","interval":1,"daysOfWeek":["tuesday"],"index":"third"},"range":{"type":"numbered","startDate":"2026-01-01","numberOfOccurrences":5}}"#;
    let rec: PatternedRecurrence = serde_json::from_str(json).unwrap();
    assert_eq!(rec.pattern.pattern_type, RecurrencePatternType::RelativeMonthly);
    assert_eq!(rec.pattern.index.as_deref(), Some("third"));
    assert_eq!(rec.range.range_type, RecurrenceRangeType::Numbered);
    assert_eq!(rec.range.number_of_occurrences, Some(5));
}

#[test]
fn todo_task_parses_importance_recurrence_reminder_with_defaults() {
    let json = r#"{
        "id":"T1","title":"x","status":"notStarted","importance":"high","isReminderOn":true,
        "reminderDateTime":{"dateTime":"2026-06-20T09:00:00.0000000","timeZone":"UTC"},
        "recurrence":{"pattern":{"type":"daily","interval":1},"range":{"type":"noEnd","startDate":"2026-06-20"}}
    }"#;
    let t: TodoTask = serde_json::from_str(json).unwrap();
    assert_eq!(t.importance, Importance::High);
    assert!(t.is_reminder_on);
    assert_eq!(t.reminder_date_time.as_ref().unwrap().date_time, "2026-06-20T09:00:00.0000000");
    assert_eq!(t.recurrence.as_ref().unwrap().pattern.pattern_type, RecurrencePatternType::Daily);

    // Missing fields default cleanly.
    let bare: TodoTask = serde_json::from_str(r#"{"id":"T2","title":"y","status":"completed"}"#).unwrap();
    assert_eq!(bare.importance, Importance::Normal);
    assert!(!bare.is_reminder_on);
    assert!(bare.recurrence.is_none());
}

use outlook_tasks_core::models::TaskInput;

#[test]
fn task_input_create_body_skips_unset_fields() {
    let input = TaskInput { title: "Buy milk".into(), ..Default::default() };
    let v = input.to_body(false); // create
    assert_eq!(v["title"], "Buy milk");
    assert_eq!(v["importance"], "normal");
    assert!(v.get("dueDateTime").is_none());
    assert!(v.get("recurrence").is_none());
    assert!(v.get("isReminderOn").is_none());
}

#[test]
fn task_input_update_body_sends_nulls_to_clear() {
    use serde_json::Value;
    let input = TaskInput { title: "x".into(), ..Default::default() };
    let v = input.to_body(true); // update
    // The key must be PRESENT and null (not absent - `["k"].is_null()` is true for both).
    assert_eq!(v.get("dueDateTime"), Some(&Value::Null));
    assert_eq!(v.get("recurrence"), Some(&Value::Null));
    assert_eq!(v.get("isReminderOn"), Some(&Value::Bool(false)));
    assert_eq!(v.get("reminderDateTime"), Some(&Value::Null));
}

#[test]
fn task_input_body_includes_due_and_reminder_when_set() {
    let dt = |s: &str| outlook_tasks_core::models::DateTimeTimeZone {
        date_time: s.into(),
        time_zone: Some("UTC".into()),
    };
    let input = TaskInput {
        title: "x".into(),
        due: Some(dt("2026-06-20T00:00:00.0000000")),
        reminder: Some(dt("2026-06-20T09:00:00.0000000")),
        ..Default::default()
    };
    let v = input.to_body(false);
    assert_eq!(v["dueDateTime"]["dateTime"], "2026-06-20T00:00:00.0000000");
    assert_eq!(v["isReminderOn"], true);
    assert_eq!(v["reminderDateTime"]["dateTime"], "2026-06-20T09:00:00.0000000");
}

#[test]
fn task_input_body_includes_recurrence() {
    use outlook_tasks_core::models::{
        PatternedRecurrence, RecurrencePattern, RecurrencePatternType, RecurrenceRange,
        RecurrenceRangeType,
    };
    let rec = PatternedRecurrence {
        pattern: RecurrencePattern {
            pattern_type: RecurrencePatternType::Daily,
            interval: 1,
            month: None,
            day_of_month: None,
            days_of_week: vec![],
            first_day_of_week: None,
            index: None,
        },
        range: RecurrenceRange {
            range_type: RecurrenceRangeType::NoEnd,
            start_date: "2026-06-20".into(),
            end_date: None,
            number_of_occurrences: None,
            recurrence_time_zone: Some("UTC".into()),
        },
    };
    let input = TaskInput { title: "x".into(), recurrence: Some(rec), ..Default::default() };
    let v = input.to_body(false);
    assert_eq!(v["recurrence"]["pattern"]["type"], "daily");
    assert_eq!(v["recurrence"]["range"]["startDate"], "2026-06-20");
}
