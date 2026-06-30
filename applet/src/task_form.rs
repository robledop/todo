use chrono::Datelike;
use outlook_tasks_core::models::{
    BodyType, DateTimeTimeZone, Importance, ItemBody, PatternedRecurrence, RecurrencePattern,
    RecurrencePatternType, RecurrenceRange, RecurrenceRangeType, TaskInput, TodoTask, WeekIndex,
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
    pub importance: Importance,
    pub reminder_on: bool,
    pub reminder_date: Option<String>, // YYYY-MM-DD
    pub reminder_time: String,         // HH:MM
    /// The note as editable plain text. HTML notes are flattened for display; the
    /// canonical value lives here while the multiline editor buffer (which is not
    /// `Clone`/`Eq`) lives on `AppModel`.
    pub note: String,
    /// The original HTML `content` when the loaded note was `html`, kept verbatim
    /// so an edit that does not touch the note re-sends it unchanged instead of
    /// re-escaping a flattened copy. Mirrors `preserved_recurrence`; cleared by
    /// `set_note` once the visible text diverges from the rendered original.
    pub preserved_note_html: Option<String>,
    /// The note's rendered text as loaded, used to tell "the note was emptied"
    /// (so clear it) from "there was never a note to begin with, or it was not
    /// loaded" (so leave the server note untouched). Guards against an edit of a
    /// task whose `body` was absent silently wiping a server-side note.
    pub original_note: String,
    pub error: Option<String>,
    /// A save request is in flight; suppresses re-submits so a double-click
    /// can't create the task twice.
    pub saving: bool,
    /// A recurrence the form can't represent (an `Unknown` pattern from Graph),
    /// kept verbatim so saving an edit preserves it instead of nulling it out.
    /// Set only by `from_task`; cleared the moment the user touches the repeat
    /// control, which is taken as intent to redefine the recurrence.
    pub preserved_recurrence: Option<PatternedRecurrence>,

    // View-only picker state (not part of to_input/from_task), but it participates
    // in PartialEq/Eq - fine, CalendarModel is Eq.
    pub due_open: bool,
    pub due_cal: cosmic::widget::calendar::CalendarModel,
    pub reminder_open: bool,
    pub reminder_cal: cosmic::widget::calendar::CalendarModel,
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
            importance: Importance::Normal,
            reminder_on: false,
            reminder_date: None,
            reminder_time: "09:00".into(),
            note: String::new(),
            preserved_note_html: None,
            original_note: String::new(),
            error: None,
            saving: false,
            preserved_recurrence: None,
            due_open: false,
            due_cal: cosmic::widget::calendar::CalendarModel::now(),
            reminder_open: false,
            reminder_cal: cosmic::widget::calendar::CalendarModel::now(),
        }
    }
}

impl TaskForm {
    pub fn create() -> Self {
        Self::default()
    }

    /// Sets the note's plain text (driven by the multiline editor on `AppModel`).
    /// Once the visible text diverges from the rendered original, any preserved
    /// HTML is dropped so the edit is saved as fresh escaped HTML, not the stale
    /// original. Mirrors how touching the repeat control drops `preserved_recurrence`.
    pub fn set_note(&mut self, text: String) {
        if let Some(html) = &self.preserved_note_html
            && text != crate::note::html_to_text(html)
        {
            self.preserved_note_html = None;
        }
        self.note = text;
    }

    /// Builds a `TaskInput` from the form for the given timezone name (e.g. the
    /// system IANA zone, or "UTC" in tests), or an error message if invalid.
    pub fn to_input(&self, tz: &str) -> Result<TaskInput, &'static str> {
        let title = self.title.trim();
        if title.is_empty() {
            return Err("Title is required");
        }
        if self.repeat != RepeatKind::None && self.due.is_none() {
            return Err("Set a due date to repeat");
        }

        let reminder = if self.reminder_on {
            let date = self.reminder_date.as_ref().ok_or("Set a reminder date")?;
            let (h, m) = parse_hhmm(&self.reminder_time).ok_or("Reminder time must be HH:MM")?;
            Some(DateTimeTimeZone {
                date_time: format!("{date}T{h:02}:{m:02}:00.0000000"),
                time_zone: Some(tz.to_string()),
            })
        } else {
            None
        };

        let due = self.due.as_ref().map(|d| day_to_dtz(d, tz));
        // Fall back to a preserved, unrepresentable recurrence so an edit doesn't
        // clear it; cleared once the user touches the repeat control.
        let recurrence = self.build_recurrence().or_else(|| self.preserved_recurrence.clone());

        // The note is written as html (the only type the API accepts on write).
        // A non-empty note re-sends preserved HTML verbatim when untouched, else
        // escapes the plain text. An emptied note clears (`Some("")`) only if a
        // note was actually loaded; a task that never had a loaded note is left
        // untouched (`None`), so an unrelated edit can't wipe a server-side note.
        let body = if !self.note.trim().is_empty() {
            Some(self.preserved_note_html.clone().unwrap_or_else(|| crate::note::text_to_html(&self.note)))
        } else if !self.original_note.trim().is_empty() {
            Some(String::new())
        } else {
            None
        };

        Ok(TaskInput {
            title: title.to_string(),
            importance: self.importance,
            due,
            recurrence,
            reminder,
            body,
        })
    }

    fn build_recurrence(&self) -> Option<PatternedRecurrence> {
        if self.repeat == RepeatKind::None {
            return None;
        }
        let start_date = self.due.clone().unwrap_or_default();
        let weekday_str = |i: usize| WEEKDAYS[i % 7].to_string();
        let selected: Vec<String> = self
            .weekdays
            .iter()
            .enumerate()
            .filter(|(_, on)| **on)
            .map(|(i, _)| weekday_str(i))
            .collect();

        // `firstDayOfWeek` is a weekly-only property; leave it unset otherwise.
        let mut first_day_of_week = None;
        let (pattern_type, days_of_week, day_of_month, index, month) = match self.repeat {
            RepeatKind::None => return None,
            RepeatKind::Daily => (RecurrencePatternType::Daily, vec![], None, None, None),
            RepeatKind::Weekly => {
                first_day_of_week = Some("sunday".to_string());
                let days = if selected.is_empty() { vec![due_weekday(&start_date)] } else { selected };
                (RecurrencePatternType::Weekly, days, None, None, None)
            }
            RepeatKind::Monthly => match self.monthly_mode {
                MonthlyMode::DayOfMonth => {
                    (RecurrencePatternType::AbsoluteMonthly, vec![], Some(self.day_of_month), None, None)
                }
                MonthlyMode::NthWeekday => (
                    RecurrencePatternType::RelativeMonthly,
                    vec![weekday_str(self.nth_weekday)],
                    None,
                    Some(index_str(self.nth_index)),
                    None,
                ),
            },
            RepeatKind::Yearly => match self.monthly_mode {
                MonthlyMode::DayOfMonth => (
                    RecurrencePatternType::AbsoluteYearly,
                    vec![],
                    Some(self.day_of_month),
                    None,
                    Some(self.year_month),
                ),
                MonthlyMode::NthWeekday => (
                    RecurrencePatternType::RelativeYearly,
                    vec![weekday_str(self.nth_weekday)],
                    None,
                    Some(index_str(self.nth_index)),
                    Some(self.year_month),
                ),
            },
        };

        Some(PatternedRecurrence {
            pattern: RecurrencePattern {
                pattern_type,
                interval: self.interval.max(1),
                month,
                day_of_month,
                days_of_week,
                first_day_of_week,
                index,
            },
            // Microsoft To Do only supports open-ended recurrence: it ignores any
            // range end and always stores `noEnd`. `startDate` is carried for reading
            // tasks back, but TaskInput::to_body strips it from requests (the endpoint
            // 400s on any Edm.Date there; it derives the start from dueDateTime).
            range: RecurrenceRange {
                range_type: RecurrenceRangeType::NoEnd,
                start_date,
                end_date: None,
                number_of_occurrences: None,
                recurrence_time_zone: Some("UTC".to_string()),
            },
        })
    }

    /// Pre-fills a form from an existing task (Edit mode). Unsupported recurrence
    /// shapes fall back to RepeatKind::None.
    pub fn from_task(task: &TodoTask, tz: &str) -> Self {
        // Graph echoes due/reminder times back in UTC; re-express them in the
        // local zone so the form shows the wall time the user set, not the UTC
        // time on the wire.
        let due = task.due_date_time.as_ref().map(|d| local_date(d, tz));
        let reminder_date = task.reminder_date_time.as_ref().map(|d| local_date(d, tz));
        let reminder_time = task
            .reminder_date_time
            .as_ref()
            .map(|d| local_time(d, tz))
            .unwrap_or_else(|| "09:00".into());

        let due_cal = cal_from(due.as_deref());
        let reminder_cal = cal_from(reminder_date.as_deref());
        let (note, preserved_note_html) = note_from_body(task.body.as_ref());
        let original_note = note.clone();

        let mut form = Self {
            mode: FormMode::Edit { task_id: task.id.clone() },
            title: task.title.clone(),
            due,
            importance: task.importance,
            reminder_on: task.is_reminder_on,
            reminder_date,
            reminder_time,
            note,
            preserved_note_html,
            original_note,
            due_cal,
            reminder_cal,
            ..Self::default()
        };

        if let Some(rec) = &task.recurrence {
            form.apply_recurrence(rec);
        }
        form
    }

    fn apply_recurrence(&mut self, rec: &PatternedRecurrence) {
        use RecurrencePatternType as PT;
        // An unrecognized pattern can't be represented in the form; leave repeat
        // None but keep the original so saving the edit preserves it rather than
        // clearing it (it is reused in `to_input` unless the user edits recurrence).
        if rec.pattern.pattern_type == PT::Unknown {
            self.repeat = RepeatKind::None;
            self.preserved_recurrence = Some(rec.clone());
            return;
        }
        self.interval = rec.pattern.interval.max(1);
        let wd_index = |name: &str| WEEKDAYS.iter().position(|w| *w == name);
        self.weekdays = [false; 7];
        for d in &rec.pattern.days_of_week {
            if let Some(i) = wd_index(d) {
                self.weekdays[i] = true;
            }
        }
        if let Some(first) = rec.pattern.days_of_week.first().and_then(|d| wd_index(d)) {
            self.nth_weekday = first;
        }
        if let Some(dom) = rec.pattern.day_of_month {
            self.day_of_month = dom;
        }
        if let Some(m) = rec.pattern.month {
            self.year_month = m;
        }
        self.nth_index = rec
            .pattern
            .index
            .as_deref()
            .and_then(index_from_str)
            .unwrap_or(WeekIndex::First);

        self.repeat = match rec.pattern.pattern_type {
            PT::Daily => RepeatKind::Daily,
            PT::Weekly => RepeatKind::Weekly,
            PT::AbsoluteMonthly => {
                self.monthly_mode = MonthlyMode::DayOfMonth;
                RepeatKind::Monthly
            }
            PT::RelativeMonthly => {
                self.monthly_mode = MonthlyMode::NthWeekday;
                RepeatKind::Monthly
            }
            PT::AbsoluteYearly => {
                self.monthly_mode = MonthlyMode::DayOfMonth;
                RepeatKind::Yearly
            }
            PT::RelativeYearly => {
                self.monthly_mode = MonthlyMode::NthWeekday;
                RepeatKind::Yearly
            }
            PT::Unknown => RepeatKind::None, // unreachable (early-returned); keeps match exhaustive
        };

        // Weekly with no explicit days: mirror to_input's due-weekday default so a
        // title-only edit doesn't silently change the schedule on save.
        if self.repeat == RepeatKind::Weekly
            && !self.weekdays.iter().any(|b| *b)
            && let Some(due) = self.due.clone()
            && let Some(i) = wd_index(&due_weekday(&due))
        {
            self.weekdays[i] = true;
        }
    }
}

/// A field edit emitted by the form view. Routed through `Message::Form` to keep
/// the app-level `Message` enum flat.
#[derive(Debug, Clone)]
pub enum FormMsg {
    Title(String),
    DueToggle,         // open/close the due calendar popover
    DuePicked(String), // YYYY-MM-DD (from calendar on_select)
    DuePrevMonth,
    DueNextMonth,
    DueCleared,
    Repeat(RepeatKind),
    Interval(String),
    Weekday(usize, bool),
    MonthlyMode(MonthlyMode),
    DayOfMonth(String),
    NthIndex(WeekIndex),
    NthWeekday(usize),
    YearMonth(u8),
    Importance(Importance),
    ReminderOn(bool),
    ReminderToggle,
    ReminderDate(String),
    ReminderPrevMonth,
    ReminderNextMonth,
    ReminderTime(String),
}

impl TaskForm {
    /// Applies a field edit to the form (pure).
    pub fn apply(&mut self, msg: FormMsg) {
        match msg {
            FormMsg::Title(s) => self.title = s,
            FormMsg::DueToggle => self.due_open = !self.due_open,
            FormMsg::DuePicked(d) => {
                self.due_cal = cal_from(Some(&d));
                self.due = Some(d);
                self.due_open = false;
            }
            FormMsg::DuePrevMonth => self.due_cal.show_prev_month(),
            FormMsg::DueNextMonth => self.due_cal.show_next_month(),
            FormMsg::DueCleared => self.due = None,
            FormMsg::Repeat(r) => {
                self.repeat = r;
                // The user is redefining recurrence; drop any preserved original.
                self.preserved_recurrence = None;
            }
            FormMsg::Interval(s) => self.interval = s.parse().unwrap_or(self.interval).max(1),
            FormMsg::Weekday(i, on) => {
                if i < 7 {
                    self.weekdays[i] = on;
                }
            }
            FormMsg::MonthlyMode(m) => self.monthly_mode = m,
            FormMsg::DayOfMonth(s) => {
                self.day_of_month = s.parse().unwrap_or(self.day_of_month).clamp(1, 31);
            }
            FormMsg::NthIndex(i) => self.nth_index = i,
            FormMsg::NthWeekday(w) => self.nth_weekday = w.min(6),
            FormMsg::YearMonth(m) => self.year_month = m.clamp(1, 12),
            FormMsg::Importance(i) => self.importance = i,
            FormMsg::ReminderOn(b) => self.reminder_on = b,
            FormMsg::ReminderToggle => self.reminder_open = !self.reminder_open,
            FormMsg::ReminderDate(d) => {
                self.reminder_cal = cal_from(Some(&d));
                self.reminder_date = Some(d);
                self.reminder_open = false;
            }
            FormMsg::ReminderPrevMonth => self.reminder_cal.show_prev_month(),
            FormMsg::ReminderNextMonth => self.reminder_cal.show_next_month(),
            FormMsg::ReminderTime(t) => self.reminder_time = t,
        }
    }
}

/// Maps a `RepeatKind` to/from a dropdown index.
const REPEATS: [RepeatKind; 5] = [
    RepeatKind::None,
    RepeatKind::Daily,
    RepeatKind::Weekly,
    RepeatKind::Monthly,
    RepeatKind::Yearly,
];

/// Maps an `Importance` to/from a dropdown index.
const IMPORTANCES: [Importance; 3] = [Importance::Low, Importance::Normal, Importance::High];

/// Month names for the yearly-month dropdown (index 0 == January).
const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

/// Short weekday labels for the weekly toggles and nth-weekday dropdown
/// (index 0 == Monday, mirroring `TaskForm::weekdays`).
const WEEKDAY_LABELS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

const INDEX_LABELS: [&str; 5] = ["First", "Second", "Third", "Fourth", "Last"];

/// Renders the create/edit form. All field edits route through
/// `Message::Form(FormMsg::...)`; Cancel returns to the list and Save commits.
/// Save is disabled while the form is invalid (the validity hint says why).
pub fn form_view<'a>(
    form: &'a TaskForm,
    note_editor: &'a cosmic::widget::text_editor::Content,
) -> cosmic::Element<'a, crate::app::Message> {
    use crate::app::Message;
    use cosmic::widget;

    let title_label = match &form.mode {
        FormMode::Create => "New task",
        FormMode::Edit { .. } => "Edit task",
    };

    let title_input = widget::text_input("Title", &form.title)
        .on_input(|s| Message::Form(FormMsg::Title(s)));

    let mut col = widget::Column::new()
        .push(widget::text::title4(title_label))
        .push(field_label("Title"))
        .push(title_input)
        .push(field_label("Due"))
        .push(date_field(
            form.due.as_deref(),
            form.due_open,
            &form.due_cal,
            true,
            FormMsg::DueToggle,
            FormMsg::DuePicked,
            FormMsg::DuePrevMonth,
            FormMsg::DueNextMonth,
            Some(FormMsg::DueCleared),
        ))
        .push(field_label("Repeat"))
        .push(repeat_dropdown(form));

    if form.repeat != RepeatKind::None {
        col = col.push(recurrence_controls(form));
    }

    col = col
        .push(field_label("Importance"))
        .push(importance_dropdown(form))
        .push(reminder_controls(form))
        .push(field_label("Note"))
        .push(
            widget::TextEditor::new(note_editor)
                .placeholder("Add a note")
                .height(cosmic::iced::Length::Fixed(96.0))
                .padding(8)
                .on_action(Message::NoteEdit),
        );

    // Validity hint (timezone-independent): show the first blocking error, if any.
    if let Some(err) = form.to_input("UTC").err() {
        col = col.push(error_caption(err));
    }
    // Surface a save error from a previous attempt.
    if let Some(err) = &form.error {
        col = col.push(error_caption(err));
    }

    // Disable Save while the form is invalid (timezone-independent check, same as
    // the validity hint above) or while a save is already in flight: an omitted
    // `on_press` renders a disabled button.
    let save_press =
        (!form.saving && form.to_input("UTC").is_ok()).then_some(Message::SaveForm);
    let save_label = if form.saving { "Saving..." } else { "Save" };

    // The form body scrolls; the action bar is pinned below it so Save and Cancel
    // stay visible however long the form grows or whichever date picker is open.
    let body = widget::scrollable(col.spacing(8).padding(12))
        .height(cosmic::iced::Length::Fixed(380.0));

    let footer = widget::Row::new()
        .push(widget::space::horizontal())
        .push(widget::button::text("Cancel").on_press(Message::CancelForm))
        .push(widget::button::suggested(save_label).on_press_maybe(save_press))
        .align_y(cosmic::iced::Alignment::Center)
        .spacing(8)
        .padding(cosmic::iced::Padding { top: 0.0, right: 12.0, bottom: 12.0, left: 12.0 });

    widget::Column::new()
        .push(body)
        .push(widget::divider::horizontal::default())
        .push(footer)
        .spacing(8)
        .into()
}

fn field_label(text: &str) -> cosmic::Element<'static, crate::app::Message> {
    cosmic::widget::text::caption(text.to_string()).into()
}

fn error_caption(text: &str) -> cosmic::Element<'static, crate::app::Message> {
    cosmic::widget::text::caption(text.to_string())
        .class(cosmic::theme::Text::Color(cosmic::iced::Color::from_rgb(0.8, 0.2, 0.2)))
        .into()
}

/// A date button that opens a calendar popover. `clearable` adds a "Clear"
/// button (used by the optional due date).
#[allow(clippy::too_many_arguments)]
fn date_field<'a>(
    value: Option<&str>,
    open: bool,
    model: &'a cosmic::widget::calendar::CalendarModel,
    clearable: bool,
    toggle: FormMsg,
    picked: fn(String) -> FormMsg,
    prev: FormMsg,
    next: FormMsg,
    clear: Option<FormMsg>,
) -> cosmic::Element<'a, crate::app::Message> {
    use crate::app::Message;
    use cosmic::widget;

    let label = value.map(str::to_string).unwrap_or_else(|| "None".to_string());
    let button = widget::button::standard(label).on_press(Message::Form(toggle.clone()));

    let trigger: cosmic::Element<'a, Message> = if open {
        let cal = widget::calendar(
            model,
            move |d| Message::Form(picked(d.to_string())),
            move || Message::Form(prev.clone()),
            move || Message::Form(next.clone()),
            jiff::civil::Weekday::Sunday,
        );
        // A bare popover popup has no surface of its own and renders see-through
        // over the form; give the calendar an opaque themed background.
        let popup = widget::container(cal)
            .padding(cosmic::theme::spacing().space_xs)
            .class(cosmic::theme::Container::Dropdown);
        widget::popover(button)
            .popup(popup)
            .on_close(Message::Form(toggle))
            .into()
    } else {
        button.into()
    };

    let mut row = widget::Row::new().push(trigger).align_y(cosmic::iced::Alignment::Center).spacing(8);
    if let Some(clear) = clear
        && clearable
        && value.is_some()
    {
        row = row.push(widget::button::text("Clear").on_press(Message::Form(clear)));
    }
    row.into()
}

fn repeat_dropdown(form: &TaskForm) -> cosmic::Element<'static, crate::app::Message> {
    use crate::app::Message;
    let items: Vec<&'static str> = vec!["None", "Daily", "Weekly", "Monthly", "Yearly"];
    let idx = REPEATS.iter().position(|r| *r == form.repeat);
    cosmic::widget::dropdown(items, idx, |i| Message::Form(FormMsg::Repeat(REPEATS[i]))).into()
}

fn importance_dropdown(form: &TaskForm) -> cosmic::Element<'static, crate::app::Message> {
    use crate::app::Message;
    let items: Vec<&'static str> = vec!["Low", "Normal", "High"];
    let idx = IMPORTANCES.iter().position(|i| *i == form.importance);
    cosmic::widget::dropdown(items, idx, |i| Message::Form(FormMsg::Importance(IMPORTANCES[i]))).into()
}

/// Interval + per-kind sub-controls.
fn recurrence_controls(form: &TaskForm) -> cosmic::Element<'_, crate::app::Message> {
    use crate::app::Message;
    use cosmic::widget;

    let interval = widget::Row::new()
        .push(widget::text::body("Every"))
        .push(
            widget::text_input("1", form.interval.to_string())
                .on_input(|s| Message::Form(FormMsg::Interval(s)))
                .width(cosmic::iced::Length::Fixed(64.0)),
        )
        .push(widget::text::body(match form.repeat {
            RepeatKind::Daily => "day(s)",
            RepeatKind::Weekly => "week(s)",
            RepeatKind::Monthly => "month(s)",
            RepeatKind::Yearly => "year(s)",
            RepeatKind::None => "",
        }))
        .align_y(cosmic::iced::Alignment::Center)
        .spacing(8);

    let mut col = widget::Column::new().push(interval).spacing(8);

    match form.repeat {
        RepeatKind::Weekly => col = col.push(weekday_toggles(form)),
        RepeatKind::Monthly => col = col.push(monthly_controls(form, false)),
        RepeatKind::Yearly => col = col.push(monthly_controls(form, true)),
        RepeatKind::Daily | RepeatKind::None => {}
    }

    col.into()
}

fn weekday_toggles(form: &TaskForm) -> cosmic::Element<'static, crate::app::Message> {
    use crate::app::Message;
    use cosmic::widget;

    let mut row = widget::Row::new().align_y(cosmic::iced::Alignment::Center).spacing(4);
    for (i, label) in WEEKDAY_LABELS.iter().enumerate() {
        let on = form.weekdays[i];
        let mut btn = widget::button::text(label.to_string())
            .on_press(Message::Form(FormMsg::Weekday(i, !on)));
        if on {
            btn = btn.class(cosmic::theme::Button::Suggested);
        }
        row = row.push(btn);
    }
    row.into()
}

/// Day-of-month vs nth-weekday radios with the matching sub-control; `yearly`
/// adds a month dropdown.
fn monthly_controls(form: &TaskForm, yearly: bool) -> cosmic::Element<'_, crate::app::Message> {
    use crate::app::Message;
    use cosmic::widget;

    let mode_radios = widget::Row::new()
        .push(widget::radio(
            widget::text::body("Day of month"),
            MonthlyMode::DayOfMonth,
            Some(form.monthly_mode),
            |m| Message::Form(FormMsg::MonthlyMode(m)),
        ))
        .push(widget::radio(
            widget::text::body("Nth weekday"),
            MonthlyMode::NthWeekday,
            Some(form.monthly_mode),
            |m| Message::Form(FormMsg::MonthlyMode(m)),
        ))
        .align_y(cosmic::iced::Alignment::Center)
        .spacing(12);

    let mut col = widget::Column::new().push(mode_radios).spacing(8);

    if yearly {
        let month_idx = (form.year_month.clamp(1, 12) - 1) as usize;
        let months: Vec<&'static str> = MONTHS.to_vec();
        let month_dd = widget::dropdown(months, Some(month_idx), |i| {
            Message::Form(FormMsg::YearMonth((i as u8) + 1))
        });
        col = col.push(widget::Row::new()
            .push(widget::text::body("Month"))
            .push(month_dd)
            .align_y(cosmic::iced::Alignment::Center)
            .spacing(8));
    }

    match form.monthly_mode {
        MonthlyMode::DayOfMonth => {
            col = col.push(
                widget::Row::new()
                    .push(widget::text::body("Day"))
                    .push(
                        widget::text_input("1", form.day_of_month.to_string())
                            .on_input(|s| Message::Form(FormMsg::DayOfMonth(s)))
                            .width(cosmic::iced::Length::Fixed(64.0)),
                    )
                    .align_y(cosmic::iced::Alignment::Center)
                    .spacing(8),
            );
        }
        MonthlyMode::NthWeekday => {
            let index_items: Vec<&'static str> = INDEX_LABELS.to_vec();
            let index_idx = INDEXES.iter().position(|i| *i == form.nth_index);
            let index_dd = widget::dropdown(index_items, index_idx, |i| {
                Message::Form(FormMsg::NthIndex(INDEXES[i]))
            });
            let weekday_items: Vec<&'static str> = WEEKDAY_LABELS.to_vec();
            let weekday_idx = Some(form.nth_weekday.min(6));
            let weekday_dd = widget::dropdown(weekday_items, weekday_idx, |i| {
                Message::Form(FormMsg::NthWeekday(i))
            });
            col = col.push(
                widget::Row::new()
                    .push(index_dd)
                    .push(weekday_dd)
                    .align_y(cosmic::iced::Alignment::Center)
                    .spacing(8),
            );
        }
    }
    col.into()
}

/// Reminder toggler with a date popover and an HH:MM time input.
fn reminder_controls(form: &TaskForm) -> cosmic::Element<'_, crate::app::Message> {
    use crate::app::Message;
    use cosmic::widget;

    let spacing = cosmic::theme::active().cosmic().spacing;

    let toggle = widget::toggler(form.reminder_on)
        .label("Remind me".to_string())
        .spacing(spacing.space_xs)
        .on_toggle(|b| Message::Form(FormMsg::ReminderOn(b)));

    let mut col = widget::Column::new().push(toggle).spacing(8);

    if form.reminder_on {
        col = col
            .push(date_field(
                form.reminder_date.as_deref(),
                form.reminder_open,
                &form.reminder_cal,
                false,
                FormMsg::ReminderToggle,
                FormMsg::ReminderDate,
                FormMsg::ReminderPrevMonth,
                FormMsg::ReminderNextMonth,
                None,
            ))
            .push(
                widget::Row::new()
                    .push(widget::text::body("Time"))
                    .push(
                        widget::text_input("09:00", &form.reminder_time)
                            .on_input(|s| Message::Form(FormMsg::ReminderTime(s)))
                            .width(cosmic::iced::Length::Fixed(96.0)),
                    )
                    .align_y(cosmic::iced::Alignment::Center)
                    .spacing(8),
            );
    }
    col.into()
}

fn day_to_dtz(day: &str, tz: &str) -> DateTimeTimeZone {
    DateTimeTimeZone { date_time: format!("{day}T00:00:00.0000000"), time_zone: Some(tz.to_string()) }
}

fn index_str(i: WeekIndex) -> String {
    match i {
        WeekIndex::First => "first",
        WeekIndex::Second => "second",
        WeekIndex::Third => "third",
        WeekIndex::Fourth => "fourth",
        WeekIndex::Last => "last",
    }
    .to_string()
}

/// Weekday of a `YYYY-MM-DD` as the Graph lowercase name; falls back to "monday".
fn due_weekday(day: &str) -> String {
    day.parse::<chrono::NaiveDate>()
        .map(|d| match d.weekday() {
            chrono::Weekday::Mon => "monday",
            chrono::Weekday::Tue => "tuesday",
            chrono::Weekday::Wed => "wednesday",
            chrono::Weekday::Thu => "thursday",
            chrono::Weekday::Fri => "friday",
            chrono::Weekday::Sat => "saturday",
            chrono::Weekday::Sun => "sunday",
        })
        .unwrap_or("monday")
        .to_string()
}

/// Strict `HH:MM` (00:00-23:59). Returns `None` for anything else (no coercion).
fn parse_hhmm(t: &str) -> Option<(u8, u8)> {
    let (h, m) = t.split_once(':')?;
    let h: u8 = h.parse().ok()?;
    let m: u8 = m.parse().ok()?;
    (h < 24 && m < 60).then_some((h, m))
}

fn date_part(dt: &str) -> String {
    dt.get(..10).unwrap_or(dt).to_string()
}

fn time_part(dt: &str) -> String {
    dt.get(11..16).unwrap_or("09:00").to_string()
}

/// The editable plain-text note and any HTML to preserve, derived from a task's
/// `body`. HTML is flattened for display; its original markup is preserved only
/// when it renders to non-empty text, so an empty/whitespace body is just no note.
fn note_from_body(body: Option<&ItemBody>) -> (String, Option<String>) {
    let Some(body) = body else { return (String::new(), None) };
    let display = crate::note::note_display_text(body);
    let preserved = (body.content_type == BodyType::Html && !display.trim().is_empty())
        .then(|| body.content.clone());
    (display, preserved)
}

/// The local calendar day (`YYYY-MM-DD`) of a stored `dateTimeTimeZone`, converted
/// from the zone Graph stored it in into `tz`. Falls back to the raw date if the
/// stored value or zone can't be parsed.
fn local_date(dtz: &DateTimeTimeZone, tz: &str) -> String {
    crate::reminders::local_civil(dtz, tz)
        .map(|dt| dt.strftime("%Y-%m-%d").to_string())
        .unwrap_or_else(|| date_part(&dtz.date_time))
}

/// The local wall-clock `HH:MM` of a stored `dateTimeTimeZone`, converted from the
/// zone Graph stored it in into `tz`. Falls back to the raw time on a parse error.
fn local_time(dtz: &DateTimeTimeZone, tz: &str) -> String {
    crate::reminders::local_civil(dtz, tz)
        .map(|dt| dt.strftime("%H:%M").to_string())
        .unwrap_or_else(|| time_part(&dtz.date_time))
}

fn index_from_str(s: &str) -> Option<WeekIndex> {
    Some(match s {
        "first" => WeekIndex::First,
        "second" => WeekIndex::Second,
        "third" => WeekIndex::Third,
        "fourth" => WeekIndex::Fourth,
        "last" => WeekIndex::Last,
        _ => return None,
    })
}

/// A `CalendarModel` selected/visible at the given `YYYY-MM-DD`, or today if the
/// day is absent or unparsable.
fn cal_from(day: Option<&str>) -> cosmic::widget::calendar::CalendarModel {
    let d = day
        .and_then(|s| s.parse::<jiff::civil::Date>().ok())
        .unwrap_or_else(|| jiff::Zoned::now().date());
    cosmic::widget::calendar::CalendarModel::new(d, d)
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

#[cfg(test)]
mod tests {
    use super::*;
    use outlook_tasks_core::models::{RecurrencePatternType as PT, RecurrenceRangeType as RT};

    fn base() -> TaskForm {
        // 2026-06-20 is a Saturday.
        TaskForm { title: "T".into(), due: Some("2026-06-20".into()), ..TaskForm::default() }
    }

    #[test]
    fn title_required() {
        let f = TaskForm { title: "  ".into(), ..TaskForm::default() };
        assert!(f.to_input("UTC").is_err());
    }

    #[test]
    fn recurrence_requires_due() {
        let f = TaskForm { title: "T".into(), repeat: RepeatKind::Daily, due: None, ..TaskForm::default() };
        assert!(f.to_input("UTC").is_err());
    }

    #[test]
    fn recurrence_range_model_keeps_iso_start_and_utc_zone() {
        // The model carries an ISO startDate (used internally / when reading tasks
        // back) and a UTC zone; the request layer drops startDate/endDate.
        let mut f = base(); // due 2026-06-20, Ends Never
        f.repeat = RepeatKind::Daily;
        let range = f.to_input("UTC").unwrap().recurrence.unwrap().range;
        assert_eq!(range.start_date, "2026-06-20");
        assert_eq!(range.end_date, None);
        assert_eq!(range.recurrence_time_zone.as_deref(), Some("UTC"));
    }

    #[test]
    fn recurrence_time_zone_is_always_utc() {
        // Graph 400s on a non-UTC recurrenceTimeZone, so it must be "UTC" even when
        // the task's due/reminder use the local zone.
        let mut f = base();
        f.repeat = RepeatKind::Daily;
        let rec = f.to_input("America/Sao_Paulo").unwrap().recurrence.unwrap();
        assert_eq!(rec.range.recurrence_time_zone.as_deref(), Some("UTC"));
        // The due date still carries the local zone.
        assert_eq!(
            f.to_input("America/Sao_Paulo").unwrap().due.unwrap().time_zone.as_deref(),
            Some("America/Sao_Paulo")
        );
    }

    #[test]
    fn reminder_requires_date_and_valid_time() {
        let mut f = base();
        f.reminder_on = true;
        f.reminder_date = None;
        assert!(f.to_input("UTC").is_err());
        f.reminder_date = Some("2026-06-20".into());
        f.reminder_time = "9am".into();
        assert!(f.to_input("UTC").is_err()); // not HH:MM
        f.reminder_time = "09:30".into();
        let rem = f.to_input("UTC").unwrap().reminder.unwrap();
        assert_eq!(rem.date_time, "2026-06-20T09:30:00.0000000");
        assert_eq!(rem.time_zone.as_deref(), Some("UTC"));
    }

    #[test]
    fn reminder_uses_given_timezone() {
        let mut f = base();
        f.reminder_on = true;
        f.reminder_date = Some("2026-06-20".into());
        f.reminder_time = "08:00".into();
        let rem = f.to_input("America/Sao_Paulo").unwrap().reminder.unwrap();
        assert_eq!(rem.time_zone.as_deref(), Some("America/Sao_Paulo"));
    }

    #[test]
    fn plain_task_has_due_no_recurrence() {
        let input = base().to_input("UTC").unwrap();
        assert_eq!(input.title, "T");
        assert!(input.recurrence.is_none());
        assert_eq!(input.due.unwrap().date_time, "2026-06-20T00:00:00.0000000");
    }

    #[test]
    fn weekly_explicit_days_map_with_first_day() {
        let mut f = base();
        f.repeat = RepeatKind::Weekly;
        f.interval = 2;
        f.weekdays = [true, false, false, true, false, false, false]; // Mon, Thu
        let rec = f.to_input("UTC").unwrap().recurrence.unwrap();
        assert_eq!(rec.pattern.pattern_type, PT::Weekly);
        assert_eq!(rec.pattern.interval, 2);
        assert_eq!(rec.pattern.days_of_week, vec!["monday".to_string(), "thursday".to_string()]);
        assert_eq!(rec.pattern.first_day_of_week.as_deref(), Some("sunday"));
        assert_eq!(rec.range.start_date, "2026-06-20");
    }

    #[test]
    fn weekly_empty_defaults_to_due_weekday() {
        let mut f = base();
        f.repeat = RepeatKind::Weekly; // no weekdays selected
        let rec = f.to_input("UTC").unwrap().recurrence.unwrap();
        assert_eq!(rec.pattern.days_of_week, vec!["saturday".to_string()]);
    }

    #[test]
    fn monthly_absolute_and_relative() {
        let mut f = base();
        f.repeat = RepeatKind::Monthly;
        f.monthly_mode = MonthlyMode::DayOfMonth;
        f.day_of_month = 15;
        let abs = f.to_input("UTC").unwrap().recurrence.unwrap();
        assert_eq!(abs.pattern.pattern_type, PT::AbsoluteMonthly);
        assert_eq!(abs.pattern.day_of_month, Some(15));
        assert!(abs.pattern.first_day_of_week.is_none()); // weekly-only

        f.monthly_mode = MonthlyMode::NthWeekday;
        f.nth_index = WeekIndex::Third;
        f.nth_weekday = 1; // Tuesday
        let rel = f.to_input("UTC").unwrap().recurrence.unwrap();
        assert_eq!(rel.pattern.pattern_type, PT::RelativeMonthly);
        assert_eq!(rel.pattern.index.as_deref(), Some("third"));
        assert_eq!(rel.pattern.days_of_week, vec!["tuesday".to_string()]);
    }

    #[test]
    fn yearly_absolute_and_relative() {
        let mut f = base();
        f.repeat = RepeatKind::Yearly;
        f.year_month = 3;
        f.day_of_month = 10;
        f.monthly_mode = MonthlyMode::DayOfMonth;
        let abs = f.to_input("UTC").unwrap().recurrence.unwrap();
        assert_eq!(abs.pattern.pattern_type, PT::AbsoluteYearly);
        assert_eq!(abs.pattern.month, Some(3));
        assert_eq!(abs.pattern.day_of_month, Some(10));

        f.monthly_mode = MonthlyMode::NthWeekday;
        f.nth_index = WeekIndex::Last;
        f.nth_weekday = 4; // Friday
        let rel = f.to_input("UTC").unwrap().recurrence.unwrap();
        assert_eq!(rel.pattern.pattern_type, PT::RelativeYearly);
        assert_eq!(rel.pattern.month, Some(3));
        assert_eq!(rel.pattern.index.as_deref(), Some("last"));
        assert_eq!(rel.pattern.days_of_week, vec!["friday".to_string()]);
    }

    #[test]
    fn recurrence_is_always_no_end() {
        // Microsoft To Do only supports open-ended recurrence; the form always
        // produces a noEnd range with no end date or occurrence count.
        let mut f = base();
        f.repeat = RepeatKind::Daily;
        let range = f.to_input("UTC").unwrap().recurrence.unwrap().range;
        assert_eq!(range.range_type, RT::NoEnd);
        assert_eq!(range.end_date, None);
        assert_eq!(range.number_of_occurrences, None);
    }

    #[test]
    fn importance_propagates() {
        let mut f = base();
        f.importance = Importance::High;
        assert_eq!(f.to_input("UTC").unwrap().importance, Importance::High);
    }

    #[test]
    fn apply_edits_fields() {
        let mut f = TaskForm::create();
        f.apply(FormMsg::Title("Hi".into()));
        f.apply(FormMsg::Repeat(RepeatKind::Weekly));
        f.apply(FormMsg::Weekday(2, true));
        f.apply(FormMsg::Interval("3".into()));
        assert_eq!(f.title, "Hi");
        assert_eq!(f.repeat, RepeatKind::Weekly);
        assert!(f.weekdays[2]);
        assert_eq!(f.interval, 3);
    }

    use outlook_tasks_core::models::{
        DateTimeTimeZone, PatternedRecurrence, RecurrencePattern, RecurrenceRange,
    };

    fn task_with(rec: Option<PatternedRecurrence>, due: &str) -> TodoTask {
        TodoTask {
            id: "T1".into(),
            title: "Pay rent".into(),
            due_date_time: Some(DateTimeTimeZone {
                date_time: format!("{due}T00:00:00.0000000"),
                time_zone: Some("UTC".into()),
            }),
            recurrence: rec,
            ..TodoTask::default()
        }
    }

    #[test]
    fn from_task_prefills_basic_fields() {
        let f = TaskForm::from_task(&task_with(None, "2026-06-20"), "UTC");
        assert_eq!(f.mode, FormMode::Edit { task_id: "T1".into() });
        assert_eq!(f.title, "Pay rent");
        assert_eq!(f.due.as_deref(), Some("2026-06-20"));
        assert_eq!(f.repeat, RepeatKind::None);
    }

    fn reminder_task(date_time: &str, zone: &str) -> TodoTask {
        TodoTask {
            id: "T1".into(),
            title: "Pay rent".into(),
            is_reminder_on: true,
            reminder_date_time: Some(DateTimeTimeZone {
                date_time: date_time.into(),
                time_zone: Some(zone.into()),
            }),
            ..TodoTask::default()
        }
    }

    #[test]
    fn from_task_shows_reminder_in_local_time() {
        // Graph echoes reminders back in UTC: a 09:00 America/Sao_Paulo (UTC-3)
        // reminder returns as 12:00 UTC. The form must show 09:00, not 12:00.
        let f = TaskForm::from_task(
            &reminder_task("2026-06-15T12:00:00.0000000", "UTC"),
            "America/Sao_Paulo",
        );
        assert!(f.reminder_on);
        assert_eq!(f.reminder_time, "09:00");
        assert_eq!(f.reminder_date.as_deref(), Some("2026-06-15"));
    }

    #[test]
    fn reminder_survives_edit_without_drifting() {
        // Reopen 11:00 UTC (08:00 local) and re-save: the body must reproduce
        // 08:00 local, which Graph re-stores as the same 11:00 UTC - no shift.
        let f = TaskForm::from_task(
            &reminder_task("2026-06-15T11:00:00.0000000", "UTC"),
            "America/Sao_Paulo",
        );
        assert_eq!(f.reminder_time, "08:00");
        let rem = f.to_input("America/Sao_Paulo").unwrap().reminder.unwrap();
        assert_eq!(rem.date_time, "2026-06-15T08:00:00.0000000");
        assert_eq!(rem.time_zone.as_deref(), Some("America/Sao_Paulo"));
    }

    #[test]
    fn from_task_shows_due_day_in_local_zone() {
        // A due day set at local midnight in Asia/Tokyo (UTC+9) is stored by Graph
        // as the prior day 15:00 UTC. The form must still show the local day.
        let mut task = task_with(None, "2026-06-20");
        task.due_date_time = Some(DateTimeTimeZone {
            date_time: "2026-06-19T15:00:00.0000000".into(),
            time_zone: Some("UTC".into()),
        });
        let f = TaskForm::from_task(&task, "Asia/Tokyo");
        assert_eq!(f.due.as_deref(), Some("2026-06-20"));
    }

    fn note_task(content: &str, content_type: BodyType) -> TodoTask {
        TodoTask {
            id: "T1".into(),
            title: "Pay rent".into(),
            body: Some(ItemBody { content: content.into(), content_type }),
            ..TodoTask::default()
        }
    }

    #[test]
    fn from_task_loads_text_note_verbatim() {
        let f = TaskForm::from_task(&note_task("buy milk", BodyType::Text), "UTC");
        assert_eq!(f.note, "buy milk");
        assert_eq!(f.preserved_note_html, None);
    }

    #[test]
    fn from_task_renders_and_preserves_html_note() {
        let f = TaskForm::from_task(&note_task("<p>hello</p>", BodyType::Html), "UTC");
        assert_eq!(f.note, "hello");
        assert_eq!(f.preserved_note_html.as_deref(), Some("<p>hello</p>"));
    }

    #[test]
    fn from_task_empty_body_object_is_no_note() {
        let f = TaskForm::from_task(&note_task("", BodyType::Text), "UTC");
        assert_eq!(f.note, "");
        assert_eq!(f.preserved_note_html, None);
        assert!(f.to_input("UTC").unwrap().body.is_none());
    }

    #[test]
    fn to_input_escapes_a_text_note_to_html() {
        let mut f = base();
        f.set_note("a < b\nc".into());
        assert_eq!(f.to_input("UTC").unwrap().body.as_deref(), Some("a &lt; b<br>c"));
    }

    #[test]
    fn unedited_html_note_is_preserved_verbatim() {
        let f = TaskForm::from_task(&note_task("<p>hi <b>there</b></p>", BodyType::Html), "UTC");
        assert_eq!(f.to_input("UTC").unwrap().body.as_deref(), Some("<p>hi <b>there</b></p>"));
    }

    #[test]
    fn editing_html_note_re_escapes_as_fresh_html() {
        let mut f = TaskForm::from_task(&note_task("<p>hello</p>", BodyType::Html), "UTC");
        f.set_note("hello world".into());
        assert_eq!(f.preserved_note_html, None);
        assert_eq!(f.to_input("UTC").unwrap().body.as_deref(), Some("hello world"));
    }

    #[test]
    fn emptying_a_loaded_note_clears_it_with_empty_html() {
        // A note that existed and was emptied is cleared with an empty html body.
        let mut f = TaskForm::from_task(&note_task("buy milk", BodyType::Text), "UTC");
        f.set_note(String::new());
        assert_eq!(f.to_input("UTC").unwrap().body.as_deref(), Some(""));
    }

    #[test]
    fn html_that_renders_empty_is_left_untouched() {
        // <br> flattens to "" so the editor shows nothing and there is no visible
        // note to begin with; saving leaves the body alone (omit) rather than
        // re-sending the original or writing an empty clear.
        let f = TaskForm::from_task(&note_task("<br>", BodyType::Html), "UTC");
        assert_eq!(f.note, "");
        assert!(f.to_input("UTC").unwrap().body.is_none());
    }

    #[test]
    fn text_note_with_entity_literals_preserves_displayed_text() {
        // A text body shows its content literally; written as html it is escaped,
        // and reading that html back recovers the same displayed text.
        let f = TaskForm::from_task(&note_task("100 &amp; 200", BodyType::Text), "UTC");
        assert_eq!(f.note, "100 &amp; 200");
        let html = f.to_input("UTC").unwrap().body.unwrap();
        assert_eq!(crate::note::html_to_text(&html), "100 &amp; 200");
    }

    #[test]
    fn unknown_content_type_preserves_displayed_text() {
        // An unexpected contentType is shown verbatim; an untouched save round-trips
        // the displayed text through html escaping rather than corrupting it.
        let f = TaskForm::from_task(&note_task("<b>x</b>", BodyType::Unknown), "UTC");
        assert_eq!(f.note, "<b>x</b>");
        let html = f.to_input("UTC").unwrap().body.unwrap();
        assert_eq!(crate::note::html_to_text(&html), "<b>x</b>");
    }

    #[test]
    fn note_never_loaded_is_left_untouched_on_edit() {
        // A task whose `body` was absent (not loaded) must not have its server-side
        // note wiped by a title-only edit: `to_input` omits `body` entirely.
        let mut task = note_task("", BodyType::Text);
        task.body = None;
        let f = TaskForm::from_task(&task, "UTC");
        assert_eq!(f.note, "");
        assert!(f.to_input("UTC").unwrap().body.is_none());
    }

    #[test]
    fn from_task_then_to_input_roundtrips_weekly() {
        let mut weekly = base();
        weekly.repeat = RepeatKind::Weekly;
        weekly.interval = 3;
        weekly.weekdays = [false, true, false, false, true, false, false]; // Tue, Fri
        let rec = weekly.to_input("UTC").unwrap().recurrence;

        let task = task_with(rec.clone(), "2026-06-20");
        let form = TaskForm::from_task(&task, "UTC");
        let rec2 = form.to_input("UTC").unwrap().recurrence;
        assert_eq!(rec, rec2); // schedule preserved through edit
    }

    #[test]
    fn from_task_then_to_input_roundtrips_monthly_and_yearly() {
        for (repeat, mode) in [
            (RepeatKind::Monthly, MonthlyMode::DayOfMonth),
            (RepeatKind::Monthly, MonthlyMode::NthWeekday),
            (RepeatKind::Yearly, MonthlyMode::DayOfMonth),
            (RepeatKind::Yearly, MonthlyMode::NthWeekday),
        ] {
            let mut src = base();
            src.repeat = repeat;
            src.monthly_mode = mode;
            src.day_of_month = 12;
            src.year_month = 4;
            src.nth_index = WeekIndex::Second;
            src.nth_weekday = 2; // Wednesday
            let rec = src.to_input("UTC").unwrap().recurrence;
            let form = TaskForm::from_task(&task_with(rec.clone(), "2026-06-20"), "UTC");
            assert_eq!(rec, form.to_input("UTC").unwrap().recurrence, "{repeat:?}/{mode:?}");
        }
    }

    #[test]
    fn from_task_unknown_pattern_falls_back_to_none() {
        use outlook_tasks_core::models::{RecurrencePatternType, RecurrenceRangeType};
        let rec = PatternedRecurrence {
            pattern: RecurrencePattern {
                pattern_type: RecurrencePatternType::Unknown,
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
                recurrence_time_zone: None,
            },
        };
        let form = TaskForm::from_task(&task_with(Some(rec), "2026-06-20"), "UTC");
        assert_eq!(form.repeat, RepeatKind::None);
    }

    #[test]
    fn unknown_recurrence_is_preserved_unless_user_edits_repeat() {
        use outlook_tasks_core::models::{RecurrencePatternType, RecurrenceRangeType};
        let rec = PatternedRecurrence {
            pattern: RecurrencePattern {
                pattern_type: RecurrencePatternType::Unknown,
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
                recurrence_time_zone: None,
            },
        };
        // Untouched: saving an edit preserves the unrepresentable recurrence.
        let form = TaskForm::from_task(&task_with(Some(rec.clone()), "2026-06-20"), "UTC");
        assert!(form.to_input("UTC").unwrap().recurrence.is_some());
        // Touching the repeat control discards it, so the edit can clear recurrence.
        let mut edited = TaskForm::from_task(&task_with(Some(rec), "2026-06-20"), "UTC");
        edited.apply(FormMsg::Repeat(RepeatKind::None));
        assert!(edited.to_input("UTC").unwrap().recurrence.is_none());
    }
}
