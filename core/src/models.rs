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
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
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
    #[serde(default, skip_serializing_if = "is_normal")]
    pub importance: Importance,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recurrence: Option<PatternedRecurrence>,
    #[serde(rename = "isReminderOn", default, skip_serializing_if = "std::ops::Not::not")]
    pub is_reminder_on: bool,
    #[serde(rename = "reminderDateTime", default, skip_serializing_if = "Option::is_none")]
    pub reminder_date_time: Option<DateTimeTimeZone>,
    /// The task's note. Graph returns `text` for app-authored notes and may return
    /// `html` for notes touched in Outlook.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<ItemBody>,
}

fn is_normal(i: &Importance) -> bool {
    *i == Importance::Normal
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

/// `bodyType` enumeration for `itemBody.contentType`. `Unknown` keeps an
/// unexpected Graph value from failing the whole task decode. Only used on the
/// read side; writes always emit `html` (see `TaskInput::to_body`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum BodyType {
    #[default]
    Text,
    Html,
    #[serde(other)]
    Unknown,
}

/// Microsoft Graph `itemBody`: a task's note. `content` is plain text when
/// `content_type` is `Text`, or HTML markup when `Html`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ItemBody {
    pub content: String,
    #[serde(rename = "contentType", default)]
    pub content_type: BodyType,
}

/// `taskStatus` enumeration. `Unknown` guards against unknown future values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum TaskStatus {
    #[default]
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

/// `recurrencePattern.type`. `Unknown` keeps a new Graph value from failing the
/// whole task decode (the form treats `Unknown` as "no recurrence").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RecurrencePatternType {
    Daily,
    Weekly,
    AbsoluteMonthly,
    RelativeMonthly,
    AbsoluteYearly,
    RelativeYearly,
    #[serde(other)]
    Unknown,
}

impl RecurrencePatternType {
    /// True for the relative ("Nth weekday") patterns. The v1.0 To Do task
    /// endpoint silently downgrades these to `daily` on write, so they have to be
    /// applied via the Outlook task endpoint instead.
    pub fn is_relative(self) -> bool {
        matches!(self, Self::RelativeMonthly | Self::RelativeYearly)
    }
}

/// `recurrencePattern.index` for relative monthly/yearly patterns. Only used by
/// the form (the wire model stores `index: Option<String>`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WeekIndex {
    First,
    Second,
    Third,
    Fourth,
    Last,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecurrencePattern {
    #[serde(rename = "type")]
    pub pattern_type: RecurrencePatternType,
    pub interval: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub month: Option<u8>,
    #[serde(rename = "dayOfMonth", default, skip_serializing_if = "Option::is_none")]
    pub day_of_month: Option<u8>,
    #[serde(rename = "daysOfWeek", default, skip_serializing_if = "Vec::is_empty")]
    pub days_of_week: Vec<String>,
    #[serde(rename = "firstDayOfWeek", default, skip_serializing_if = "Option::is_none")]
    pub first_day_of_week: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index: Option<String>,
}

/// `recurrenceRange.type`. `Unknown` guards against new Graph values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RecurrenceRangeType {
    EndDate,
    NoEnd,
    Numbered,
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecurrenceRange {
    #[serde(rename = "type")]
    pub range_type: RecurrenceRangeType,
    #[serde(rename = "startDate")]
    pub start_date: String,
    #[serde(rename = "endDate", default, skip_serializing_if = "Option::is_none")]
    pub end_date: Option<String>,
    #[serde(rename = "numberOfOccurrences", default, skip_serializing_if = "Option::is_none")]
    pub number_of_occurrences: Option<i32>,
    #[serde(rename = "recurrenceTimeZone", default, skip_serializing_if = "Option::is_none")]
    pub recurrence_time_zone: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PatternedRecurrence {
    pub pattern: RecurrencePattern,
    pub range: RecurrenceRange,
}

/// OData collection envelope: `{ "value": [...], "@odata.nextLink": "..." }`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphCollection<T> {
    pub value: Vec<T>,
    #[serde(rename = "@odata.nextLink", skip_serializing_if = "Option::is_none", default)]
    pub next_link: Option<String>,
}

/// Editable fields for creating or updating a task. `to_body` produces the Graph
/// request JSON: on create, unset optional fields are omitted; on update, they are
/// sent as explicit nulls so an edit can clear them.
#[derive(Debug, Clone, Default)]
pub struct TaskInput {
    pub title: String,
    pub importance: Importance,
    pub due: Option<DateTimeTimeZone>,
    pub recurrence: Option<PatternedRecurrence>,
    pub reminder: Option<DateTimeTimeZone>,
    /// The note's HTML content, or `None` for no note. The To Do API only accepts
    /// `html` for `body` on write, so the form HTML-escapes plain text before it
    /// reaches here; `to_body` always emits `contentType: "html"`.
    pub body: Option<String>,
}

impl TaskInput {
    pub fn to_body(&self, for_update: bool) -> serde_json::Value {
        use serde_json::{json, Map, Value};
        let mut o = Map::new();
        o.insert("title".into(), json!(self.title));
        o.insert("importance".into(), serde_json::to_value(self.importance).unwrap());

        let opt = |o: &mut Map<String, Value>, key: &str, val: Option<Value>| match val {
            Some(v) => {
                o.insert(key.into(), v);
            }
            None => {
                if for_update {
                    o.insert(key.into(), Value::Null);
                }
            }
        };

        opt(&mut o, "dueDateTime", self.due.as_ref().map(|d| serde_json::to_value(d).unwrap()));
        opt(&mut o, "recurrence", self.recurrence.as_ref().map(recurrence_request_body));

        match &self.reminder {
            Some(r) => {
                o.insert("isReminderOn".into(), json!(true));
                o.insert("reminderDateTime".into(), serde_json::to_value(r).unwrap());
            }
            None => {
                if for_update {
                    o.insert("isReminderOn".into(), json!(false));
                    o.insert("reminderDateTime".into(), Value::Null);
                }
            }
        }

        // The To Do API documents `body` as write-only-HTML with no nullable form,
        // so a note is never sent as `null`. `Some` writes an html `itemBody` (an
        // empty string clears the note with an empty html body); `None` omits
        // `body` entirely, leaving any server-side note untouched. The form only
        // emits `Some("")` when a note that was actually loaded got emptied, so an
        // unrelated edit of a task whose note was never loaded can't wipe it.
        if let Some(html) = &self.body {
            o.insert("body".into(), itembody_html(html));
        }
        Value::Object(o)
    }
}

/// A Graph `itemBody` request value with `contentType: "html"`.
fn itembody_html(content: &str) -> serde_json::Value {
    serde_json::json!({ "content": content, "contentType": "html" })
}

/// Serializes a recurrence for a To Do **request**, dropping the range's
/// `startDate`/`endDate`. The To Do endpoint returns 400 ("Invalid JSON, Error
/// converting value ... to type 'Microsoft.OData.Edm.Date'") on any value in those
/// fields - a service-side parser bug confirmed against the live API - so they are
/// omitted; the service derives the recurrence start from `dueDateTime`.
fn recurrence_request_body(r: &PatternedRecurrence) -> serde_json::Value {
    let mut v = serde_json::to_value(r).expect("recurrence serializes");
    if let Some(range) = v.get_mut("range").and_then(serde_json::Value::as_object_mut) {
        range.remove("startDate");
        range.remove("endDate");
    }
    v
}

/// Partial update body for completing/changing a task: `{"status":"completed"}`.
#[derive(Debug, Clone, Serialize)]
pub struct UpdateTaskStatus {
    pub status: TaskStatus,
}

/// Microsoft Graph `user` resource (the `/me` endpoint), reduced to the fields the
/// settings screen displays. All optional: a personal account may omit `mail`, in
/// which case `userPrincipalName` carries the address.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct UserProfile {
    #[serde(rename = "displayName", default)]
    pub display_name: Option<String>,
    #[serde(rename = "userPrincipalName", default)]
    pub user_principal_name: Option<String>,
    #[serde(default)]
    pub mail: Option<String>,
}

impl UserProfile {
    /// The account's display name, if Graph returned one.
    pub fn name(&self) -> Option<&str> {
        self.display_name.as_deref()
    }

    /// The account's email: `mail` when present, else `userPrincipalName` (which
    /// holds the address for personal Microsoft accounts).
    pub fn email(&self) -> Option<&str> {
        self.mail.as_deref().or(self.user_principal_name.as_deref())
    }
}

impl std::fmt::Display for UserProfile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match (self.name(), self.email()) {
            (Some(n), Some(e)) => write!(f, "{n} <{e}>"),
            (Some(n), None) => f.write_str(n),
            (None, Some(e)) => f.write_str(e),
            (None, None) => f.write_str("Signed in"),
        }
    }
}
