use outlook_tasks_core::models::{
    DateTimeTimeZone, Importance, PatternedRecurrence, RecurrencePattern, RecurrencePatternType,
    RecurrenceRange, RecurrenceRangeType, TaskInput, TodoTask, WeekIndex,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormMode {
    Create,
    Edit { task_id: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RepeatKind {
    #[default]
    None,
    Daily,
    Weekly,
    Monthly,
    Yearly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MonthlyMode {
    #[default]
    DayOfMonth,
    NthWeekday,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum EndKind {
    #[default]
    Never,
    OnDate,
    After,
}

/// Plain, UI-independent form data. Dates are `YYYY-MM-DD` strings; the time is
/// `HH:MM`. (The view layer keeps separate `CalendarModel`s for the pickers.)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskForm {
    pub mode: FormMode,
    pub title: String,
    pub due: Option<String>,          // YYYY-MM-DD
    pub repeat: RepeatKind,
    pub interval: u16,
    pub weekdays: [bool; 7],          // Mon..Sun
    pub monthly_mode: MonthlyMode,
    pub day_of_month: u8,             // 1..=31
    pub nth_index: WeekIndex,         // for NthWeekday
    pub nth_weekday: usize,           // 0=Mon..6=Sun
    pub year_month: u8,               // 1..=12 (yearly)
    pub end: EndKind,
    pub end_date: Option<String>,     // YYYY-MM-DD (OnDate)
    pub occurrences: u32,             // After N
    pub importance: Importance,
    pub reminder_on: bool,
    pub reminder_date: Option<String>, // YYYY-MM-DD
    pub reminder_time: String,         // HH:MM
    pub error: Option<String>,
}

impl Default for TaskForm {
    fn default() -> Self {
        Self {
            mode: FormMode::Create,
            title: String::new(),
            due: None,
            repeat: RepeatKind::None,
            interval: 1,
            weekdays: [false; 7],
            monthly_mode: MonthlyMode::DayOfMonth,
            day_of_month: 1,
            nth_index: WeekIndex::First,
            nth_weekday: 0,
            year_month: 1,
            end: EndKind::Never,
            end_date: None,
            occurrences: 1,
            importance: Importance::Normal,
            reminder_on: false,
            reminder_date: None,
            reminder_time: "09:00".into(),
            error: None,
        }
    }
}

impl TaskForm {
    pub fn create() -> Self {
        Self::default()
    }
}

const WEEKDAYS: [&str; 7] =
    ["monday", "tuesday", "wednesday", "thursday", "friday", "saturday", "sunday"];

const INDEXES: [WeekIndex; 5] = [
    WeekIndex::First,
    WeekIndex::Second,
    WeekIndex::Third,
    WeekIndex::Fourth,
    WeekIndex::Last,
];
