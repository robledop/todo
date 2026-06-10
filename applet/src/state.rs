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
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(id: &str, status: TaskStatus) -> TodoTask {
        TodoTask { id: id.into(), title: id.into(), status }
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
}
