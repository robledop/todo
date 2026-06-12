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
