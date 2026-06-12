use serde::{Deserialize, Serialize};

/// A To Do list (`todoTaskList`). The built-in list has
/// `wellknown_list_name == Some("defaultList")`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TodoList {
    pub id: String,
    #[serde(rename = "displayName")]
    pub display_name: String,
    #[serde(rename = "wellknownListName", skip_serializing_if = "Option::is_none", default)]
    pub wellknown_list_name: Option<String>,
}

/// A task (`todoTask`), reduced to the fields v1 uses.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TodoTask {
    pub id: String,
    pub title: String,
    pub status: TaskStatus,
    /// UTC ISO-8601 timestamp; sortable lexicographically. Used to order
    /// completed tasks by date.
    #[serde(rename = "lastModifiedDateTime", default, skip_serializing_if = "Option::is_none")]
    pub last_modified_date_time: Option<String>,
    /// Due date, if set. A `dateTimeTimeZone` whose `date_time` is local to its
    /// `time_zone`; only the calendar day matters for "due".
    #[serde(rename = "dueDateTime", default, skip_serializing_if = "Option::is_none")]
    pub due_date_time: Option<DateTimeTimeZone>,
}

impl TodoTask {
    /// The due calendar day as `YYYY-MM-DD`, if a due date is set.
    pub fn due_day(&self) -> Option<&str> {
        self.due_date_time
            .as_ref()
            .map(|d| d.date_time.get(..10).unwrap_or(d.date_time.as_str()))
    }
}

/// Microsoft Graph `dateTimeTimeZone`: a wall-clock time plus its zone name.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DateTimeTimeZone {
    #[serde(rename = "dateTime")]
    pub date_time: String,
    #[serde(rename = "timeZone", default, skip_serializing_if = "Option::is_none")]
    pub time_zone: Option<String>,
}

/// `taskStatus` enumeration. `Unknown` guards against unknown future values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TaskStatus {
    NotStarted,
    InProgress,
    Completed,
    WaitingOnOthers,
    Deferred,
    #[serde(other)]
    Unknown,
}

/// `importance` enumeration; serialized as camelCase (`"low"`/`"normal"`/`"high"`).
/// `Unknown` keeps an unexpected Graph value from failing the whole task decode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum Importance {
    Low,
    #[default]
    Normal,
    High,
    #[serde(other)]
    Unknown,
}

/// OData collection envelope: `{ "value": [...], "@odata.nextLink": "..." }`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphCollection<T> {
    pub value: Vec<T>,
    #[serde(rename = "@odata.nextLink", skip_serializing_if = "Option::is_none", default)]
    pub next_link: Option<String>,
}

/// Minimal create-task request body: `{"title":"..."}`.
#[derive(Debug, Clone, Serialize)]
pub struct CreateTask<'a> {
    pub title: &'a str,
}

/// Partial update body for completing/changing a task: `{"status":"completed"}`.
#[derive(Debug, Clone, Serialize)]
pub struct UpdateTaskStatus {
    pub status: TaskStatus,
}
