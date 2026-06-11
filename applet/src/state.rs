use outlook_tasks_core::models::{TaskStatus, TodoList, TodoTask};

/// Top-level UI state.
#[derive(Debug, Clone)]
pub enum AppState {
    /// No Secret Service provider available.
    NoKeyring,
    /// No stored session.
    SignedOut,
    /// Browser sign-in in progress.
    Authenticating,
    /// Unrecoverable configuration error (e.g. a malformed client id/URL); shown
    /// instead of crashing, since the panel respawns a crashed applet.
    Error(String),
    /// Signed in and showing data.
    Ready(Ready),
}

#[derive(Debug, Clone, Default)]
pub struct Ready {
    pub lists: Vec<TodoList>,
    pub selected_list_id: String,
    pub tasks: Vec<TodoTask>,
    pub add_input: String,
    pub error: Option<String>,
    pub loading: bool,
    /// When false (default), completed tasks are hidden from the list.
    pub show_completed: bool,
}

impl Ready {
    /// Count of not-yet-completed tasks in the current list (the panel badge).
    pub fn open_count(&self) -> usize {
        self.tasks.iter().filter(|t| t.status != TaskStatus::Completed).count()
    }

    /// Picks the initial list: the persisted one if still present, else the
    /// default list (`wellknownListName == "defaultList"`), else the first.
    pub fn pick_initial_list(lists: &[TodoList], persisted: Option<&str>) -> Option<String> {
        if let Some(id) = persisted
            && lists.iter().any(|l| l.id == id)
        {
            return Some(id.to_string());
        }
        lists
            .iter()
            .find(|l| l.wellknown_list_name.as_deref() == Some("defaultList"))
            .or_else(|| lists.first())
            .map(|l| l.id.clone())
    }

    /// Optimistically flips a task's completion and returns its previous status
    /// (for rollback). No-op returns `None`.
    pub fn toggle_optimistic(&mut self, task_id: &str) -> Option<TaskStatus> {
        let task = self.tasks.iter_mut().find(|t| t.id == task_id)?;
        let previous = task.status;
        task.status = if previous == TaskStatus::Completed {
            TaskStatus::NotStarted
        } else {
            TaskStatus::Completed
        };
        Some(previous)
    }

    /// Restores a task's status after a failed update.
    pub fn restore_status(&mut self, task_id: &str, status: TaskStatus) {
        if let Some(task) = self.tasks.iter_mut().find(|t| t.id == task_id) {
            task.status = status;
        }
    }

    /// Replaces an optimistic placeholder task (by temp id) with the real one.
    pub fn reconcile_created(&mut self, temp_id: &str, created: TodoTask) {
        if let Some(task) = self.tasks.iter_mut().find(|t| t.id == temp_id) {
            *task = created;
        }
    }

    /// Removes an optimistic placeholder after a failed create.
    pub fn remove_task(&mut self, task_id: &str) {
        self.tasks.retain(|t| t.id != task_id);
    }

    /// Applies a freshly-fetched task list, preserving not-yet-reconciled
    /// optimistic placeholders (`temp-*`) so a poll landing mid-create doesn't
    /// drop a task the user just added.
    pub fn apply_refresh(&mut self, fetched: Vec<TodoTask>) {
        let pending: Vec<TodoTask> =
            self.tasks.iter().filter(|t| Self::is_placeholder(&t.id)).cloned().collect();
        self.tasks = fetched;
        self.tasks.extend(pending);
    }

    /// True for an optimistic placeholder id the server hasn't assigned yet.
    pub fn is_placeholder(id: &str) -> bool {
        id.starts_with("temp-")
    }

    /// Tasks to render: pending tasks (in server order) always; when
    /// `show_completed` is set, completed tasks follow, most-recent first by
    /// `last_modified_date_time` (unknown dates sort last).
    pub fn visible_tasks(&self) -> Vec<&TodoTask> {
        let mut pending: Vec<&TodoTask> =
            self.tasks.iter().filter(|t| t.status != TaskStatus::Completed).collect();
        if !self.show_completed {
            return pending;
        }
        let mut completed: Vec<&TodoTask> =
            self.tasks.iter().filter(|t| t.status == TaskStatus::Completed).collect();
        completed.sort_by(|a, b| b.last_modified_date_time.cmp(&a.last_modified_date_time));
        pending.append(&mut completed);
        pending
    }

    /// Count of pending (not-completed) tasks that are currently due - their due
    /// day is today or earlier. `today` is `YYYY-MM-DD`.
    pub fn due_count(&self, today: &str) -> usize {
        self.tasks
            .iter()
            .filter(|t| t.status != TaskStatus::Completed)
            .filter(|t| t.due_day().is_some_and(|d| is_due(d, today)))
            .count()
    }
}

/// True when a task's due day (`YYYY-MM-DD`) is due - today or earlier.
pub fn is_due(due_day: &str, today: &str) -> bool {
    due_day <= today
}

#[cfg(test)]
mod tests {
    use super::*;

    use outlook_tasks_core::models::DateTimeTimeZone;

    fn task(id: &str, status: TaskStatus) -> TodoTask {
        TodoTask {
            id: id.into(),
            title: id.into(),
            status,
            last_modified_date_time: None,
            due_date_time: None,
        }
    }

    fn task_dated(id: &str, status: TaskStatus, date: &str) -> TodoTask {
        TodoTask {
            id: id.into(),
            title: id.into(),
            status,
            last_modified_date_time: Some(date.into()),
            due_date_time: None,
        }
    }

    fn task_due(id: &str, status: TaskStatus, due_day: &str) -> TodoTask {
        TodoTask {
            id: id.into(),
            title: id.into(),
            status,
            last_modified_date_time: None,
            due_date_time: Some(DateTimeTimeZone {
                date_time: format!("{due_day}T00:00:00.0000000"),
                time_zone: Some("UTC".into()),
            }),
        }
    }

    #[test]
    fn open_count_excludes_completed() {
        let ready = Ready {
            tasks: vec![
                task("a", TaskStatus::NotStarted),
                task("b", TaskStatus::Completed),
                task("c", TaskStatus::InProgress),
            ],
            ..Default::default()
        };
        assert_eq!(ready.open_count(), 2);
    }

    #[test]
    fn pick_initial_prefers_persisted_then_default() {
        let lists = vec![
            TodoList { id: "L1".into(), display_name: "Tasks".into(), wellknown_list_name: Some("defaultList".into()) },
            TodoList { id: "L2".into(), display_name: "Work".into(), wellknown_list_name: None },
        ];
        assert_eq!(Ready::pick_initial_list(&lists, Some("L2")).as_deref(), Some("L2"));
        assert_eq!(Ready::pick_initial_list(&lists, Some("GONE")).as_deref(), Some("L1"));
        assert_eq!(Ready::pick_initial_list(&lists, None).as_deref(), Some("L1"));
    }

    #[test]
    fn toggle_then_restore_roundtrips() {
        let mut ready = Ready { tasks: vec![task("a", TaskStatus::NotStarted)], ..Default::default() };
        let prev = ready.toggle_optimistic("a").unwrap();
        assert_eq!(prev, TaskStatus::NotStarted);
        assert_eq!(ready.tasks[0].status, TaskStatus::Completed);
        ready.restore_status("a", prev);
        assert_eq!(ready.tasks[0].status, TaskStatus::NotStarted);
    }

    #[test]
    fn reconcile_replaces_placeholder() {
        let mut ready = Ready { tasks: vec![task("temp-1", TaskStatus::NotStarted)], ..Default::default() };
        ready.reconcile_created("temp-1", task("T7", TaskStatus::NotStarted));
        assert_eq!(ready.tasks[0].id, "T7");
    }

    #[test]
    fn apply_refresh_keeps_optimistic_placeholders() {
        let mut ready = Ready {
            tasks: vec![task("temp-1", TaskStatus::NotStarted), task("T1", TaskStatus::Completed)],
            ..Default::default()
        };
        // Server returns only the persisted task; the pending placeholder survives.
        ready.apply_refresh(vec![task("T1", TaskStatus::NotStarted)]);
        assert_eq!(ready.tasks.len(), 2);
        assert!(ready.tasks.iter().any(|t| t.id == "temp-1"));
    }

    #[test]
    fn visible_tasks_hides_completed_by_default() {
        let ready = Ready {
            tasks: vec![
                task("a", TaskStatus::NotStarted),
                task("b", TaskStatus::Completed),
                task("c", TaskStatus::InProgress),
            ],
            ..Default::default()
        };
        let visible: Vec<&str> = ready.visible_tasks().iter().map(|t| t.id.as_str()).collect();
        assert_eq!(visible, vec!["a", "c"]);
    }

    #[test]
    fn visible_tasks_appends_completed_newest_first_when_enabled() {
        let ready = Ready {
            show_completed: true,
            tasks: vec![
                task("p", TaskStatus::NotStarted),
                task_dated("old", TaskStatus::Completed, "2026-06-01T00:00:00Z"),
                task_dated("new", TaskStatus::Completed, "2026-06-10T00:00:00Z"),
            ],
            ..Default::default()
        };
        // Pending first, then completed sorted by date descending.
        let visible: Vec<&str> = ready.visible_tasks().iter().map(|t| t.id.as_str()).collect();
        assert_eq!(visible, vec!["p", "new", "old"]);
    }

    #[test]
    fn is_due_includes_today_and_past_only() {
        assert!(is_due("2026-06-01", "2026-06-13")); // past
        assert!(is_due("2026-06-13", "2026-06-13")); // today
        assert!(!is_due("2026-06-20", "2026-06-13")); // future
    }

    #[test]
    fn due_count_counts_pending_due_tasks() {
        let ready = Ready {
            tasks: vec![
                task_due("overdue", TaskStatus::NotStarted, "2026-06-01"),
                task_due("today", TaskStatus::NotStarted, "2026-06-13"),
                task_due("future", TaskStatus::NotStarted, "2026-06-20"),
                task("nodue", TaskStatus::NotStarted),
                task_due("done", TaskStatus::Completed, "2026-06-01"),
            ],
            ..Default::default()
        };
        // overdue + today; future, no-due, and completed are excluded.
        assert_eq!(ready.due_count("2026-06-13"), 2);
    }
}
