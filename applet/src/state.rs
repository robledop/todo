use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use outlook_tasks_core::models::{TaskStatus, TodoList, TodoTask};

use crate::task_form::TaskForm;

/// Which popup screen is shown: the task list, or the create/edit form.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum PopupView {
    #[default]
    List,
    // Boxed: TaskForm is large (calendar models), keeping PopupView/Ready small.
    Form(Box<TaskForm>),
}

/// Top-level UI state.
#[derive(Debug, Clone)]
pub enum AppState {
    /// No Secret Service provider available.
    NoKeyring,
    /// Startup session check running; no sign-in is offered yet, so a stale
    /// bootstrap result can't clobber an in-progress browser flow.
    Bootstrapping,
    /// No stored session.
    SignedOut,
    /// Browser sign-in in progress.
    Authenticating,
    /// Unrecoverable configuration error (e.g. a malformed client id/URL); shown
    /// instead of crashing, since the panel respawns a crashed applet.
    Error(String),
    /// Signed in and showing data. Boxed: `Ready` is much larger than the other
    /// variants, so boxing keeps `AppState` small (as `PopupView` does for the form).
    Ready(Box<Ready>),
}

#[derive(Debug, Clone, Default)]
pub struct Ready {
    pub lists: Vec<TodoList>,
    pub selected_list_id: String,
    pub tasks: Vec<TodoTask>,
    pub error: Option<String>,
    pub loading: bool,
    /// When false (default), completed tasks are hidden from the list.
    pub show_completed: bool,
    /// `@odata.nextLink` for the current list's tasks when more pages exist
    /// ("Load more"); reset on a fresh load.
    pub next_link: Option<String>,
    /// A "load more" fetch is in flight.
    pub loading_more: bool,
    /// At least one extra page has been loaded; suppresses the periodic
    /// auto-refresh so manually-loaded pages aren't collapsed.
    pub loaded_more: bool,
    /// Id of the task currently awaiting delete confirmation, if any.
    pub confirming_delete: Option<String>,
    /// Whether the popup shows the task list or the create/edit form.
    pub view: PopupView,
    /// Tasks just marked complete that are playing their exit animation before
    /// being hidden, keyed by task id with the instant the user completed them.
    /// Only populated while completed tasks are hidden; otherwise the task stays
    /// visible and needs no exit.
    pub completing: HashMap<String, Instant>,
    /// Monotonic counter bumped on every full task load. A load response carries
    /// the generation it was issued under and is discarded unless it still
    /// matches, so a slow load can't overwrite the result of a newer one.
    pub load_gen: u64,
    /// Tasks with an in-flight status PATCH. Blocks a second toggle of the same
    /// task until the first resolves, so two racing PATCHes can't land out of
    /// order and leave the wrong status.
    pub toggling: HashSet<String>,
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

    /// Applies a freshly-fetched task list, carrying over rows the server's
    /// pending-only list omits: not-yet-reconciled optimistic placeholders
    /// (`temp-*`), so a poll landing mid-create doesn't drop a task the user
    /// just added; and rows still playing their completion exit animation, so a
    /// poll landing mid-animation doesn't make the row vanish early.
    pub fn apply_refresh(&mut self, fetched: Vec<TodoTask>) {
        let carried: Vec<TodoTask> = self
            .tasks
            .iter()
            .filter(|t| Self::is_placeholder(&t.id) || self.completing.contains_key(&t.id))
            .cloned()
            .collect();
        self.tasks = fetched;
        self.tasks.extend(carried);
    }

    /// Appends a "load more" page to the current tasks (subsequent pages are
    /// disjoint from earlier ones).
    pub fn append_page(&mut self, mut fetched: Vec<TodoTask>) {
        self.tasks.append(&mut fetched);
    }

    /// True for an optimistic placeholder id the server hasn't assigned yet.
    pub fn is_placeholder(id: &str) -> bool {
        id.starts_with("temp-")
    }

    /// Tasks to render: pending tasks sorted by due date ascending (overdue/due
    /// first, undated last) always; when `show_completed` is set, completed
    /// tasks follow, newest-due first (undated last).
    pub fn visible_tasks(&self) -> Vec<&TodoTask> {
        // Pending tasks, plus any task still playing its completion exit
        // animation, so a just-completed row collapses in place rather than
        // vanishing. (`completing` is only populated while completed are hidden.)
        let mut pending: Vec<&TodoTask> = self
            .tasks
            .iter()
            .filter(|t| t.status != TaskStatus::Completed || self.completing.contains_key(&t.id))
            .collect();
        // Sort by due date ascending so due/overdue (red) tasks float to the top;
        // tasks with no due date go last (server order preserved among ties).
        pending.sort_by(|a, b| {
            a.due_day()
                .is_none()
                .cmp(&b.due_day().is_none())
                .then_with(|| a.due_day().cmp(&b.due_day()))
        });
        if !self.show_completed {
            return pending;
        }
        let mut completed: Vec<&TodoTask> = self
            .tasks
            .iter()
            .filter(|t| t.status == TaskStatus::Completed && !self.completing.contains_key(&t.id))
            .collect();
        // Newest-due first (undated last), matching the server's $orderby so
        // "load more" appends older completed below.
        completed.sort_by(|a, b| {
            a.due_day()
                .is_none()
                .cmp(&b.due_day().is_none())
                .then_with(|| b.due_day().cmp(&a.due_day()))
        });
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

    /// Marks a task for delete confirmation (replacing any prior one).
    pub fn request_delete(&mut self, task_id: &str) {
        self.confirming_delete = Some(task_id.to_string());
    }

    /// Clears the pending delete confirmation.
    pub fn cancel_delete(&mut self) {
        self.confirming_delete = None;
    }

    /// Marks a task's status change as in flight. Returns false if one is
    /// already in flight, in which case the caller should ignore the new toggle.
    pub fn begin_toggle(&mut self, task_id: &str) -> bool {
        self.toggling.insert(task_id.to_string())
    }

    /// Clears a task's in-flight status change once its PATCH has resolved.
    pub fn end_toggle(&mut self, task_id: &str) {
        self.toggling.remove(task_id);
    }

    /// Bumps and returns the load generation, marking the start of a new full
    /// load. Responses tagged with an earlier generation are then stale.
    pub fn next_load_gen(&mut self) -> u64 {
        self.load_gen += 1;
        self.load_gen
    }

    /// True if a load response for `list_id`/`generation` is still the one being
    /// awaited (same list, newest generation) and so should be applied.
    pub fn is_current_load(&self, list_id: &str, generation: u64) -> bool {
        self.selected_list_id == list_id && self.load_gen == generation
    }

    /// Switches the visible list, clearing every list-scoped piece of state so
    /// stale pagination, a pending delete confirmation, or an in-flight exit
    /// animation from the previous list can't bleed into the new one.
    pub fn switch_to_list(&mut self, id: String) {
        self.selected_list_id = id;
        self.tasks.clear();
        self.next_link = None;
        self.loading_more = false;
        self.loaded_more = false;
        self.confirming_delete = None;
        self.completing.clear();
        self.toggling.clear();
        self.loading = true;
    }

    /// Starts the completion exit animation for a task (no-op if already
    /// animating). Only meaningful while completed tasks are hidden.
    pub fn begin_completing(&mut self, task_id: &str, at: Instant) {
        self.completing.entry(task_id.to_string()).or_insert(at);
    }

    /// True while the task is playing its completion exit animation.
    pub fn is_completing(&self, task_id: &str) -> bool {
        self.completing.contains_key(task_id)
    }

    /// Stops a task's exit animation - e.g. its completion failed and was
    /// reverted, so it must reappear as a normal row.
    pub fn cancel_completing(&mut self, task_id: &str) {
        self.completing.remove(task_id);
    }

    /// Drops tasks whose exit animation has finished. Returns true if any were
    /// removed, i.e. the visible list changed.
    pub fn prune_completing(&mut self, now: Instant) -> bool {
        let before = self.completing.len();
        self.completing
            .retain(|_, started| complete_anim(now.saturating_duration_since(*started)).is_some());
        self.completing.len() != before
    }
}

/// How long a just-completed row holds, struck through, before it collapses.
pub const COMPLETE_HOLD: Duration = Duration::from_millis(380);
/// How long the row takes to collapse its height to nothing after the hold.
pub const COMPLETE_COLLAPSE: Duration = Duration::from_millis(240);
/// Clip height the collapse starts from, in px. Only needs to be at least the
/// real single-line row height; `clip` hides any excess, so it never jumps.
pub const COMPLETE_ROW_MAX_H: f32 = 32.0;

/// The visual state of a completing row, given how long since it was completed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CompleteAnim {
    /// Max-height clip for the collapse, in px (full height during the hold).
    pub max_height: f32,
}

/// Exit animation for a completing row. `None` once it has finished and the row
/// should be dropped. Holds at full height through [`COMPLETE_HOLD`], then eases
/// the height to zero over [`COMPLETE_COLLAPSE`].
pub fn complete_anim(elapsed: Duration) -> Option<CompleteAnim> {
    if elapsed >= COMPLETE_HOLD + COMPLETE_COLLAPSE {
        return None;
    }
    let max_height = if elapsed <= COMPLETE_HOLD {
        COMPLETE_ROW_MAX_H
    } else {
        let t = (elapsed - COMPLETE_HOLD).as_secs_f32() / COMPLETE_COLLAPSE.as_secs_f32();
        // Ease-out cubic: collapses quickly, then settles gently closed.
        let eased = 1.0 - (1.0 - t).powi(3);
        COMPLETE_ROW_MAX_H * (1.0 - eased)
    };
    Some(CompleteAnim { max_height })
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
        TodoTask { id: id.into(), title: id.into(), status, ..Default::default() }
    }

    fn task_due(id: &str, status: TaskStatus, due_day: &str) -> TodoTask {
        TodoTask {
            id: id.into(),
            title: id.into(),
            status,
            due_date_time: Some(DateTimeTimeZone {
                date_time: format!("{due_day}T00:00:00.0000000"),
                time_zone: Some("UTC".into()),
            }),
            ..Default::default()
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
    fn visible_tasks_sorts_completed_by_due_desc() {
        let ready = Ready {
            show_completed: true,
            tasks: vec![
                task_due("p", TaskStatus::NotStarted, "2026-06-20"),
                task_due("old", TaskStatus::Completed, "2026-05-01"),
                task_due("new", TaskStatus::Completed, "2026-06-10"),
            ],
            ..Default::default()
        };
        // Pending first, then completed by due date descending.
        let visible: Vec<&str> = ready.visible_tasks().iter().map(|t| t.id.as_str()).collect();
        assert_eq!(visible, vec!["p", "new", "old"]);
    }

    #[test]
    fn visible_tasks_sorted_by_due_date_undated_last() {
        let ready = Ready {
            tasks: vec![
                task_due("future", TaskStatus::NotStarted, "2026-07-04"),
                task("nodue", TaskStatus::NotStarted),
                task_due("overdue", TaskStatus::NotStarted, "2026-06-04"),
                task_due("soon", TaskStatus::NotStarted, "2026-06-16"),
            ],
            ..Default::default()
        };
        let visible: Vec<&str> = ready.visible_tasks().iter().map(|t| t.id.as_str()).collect();
        assert_eq!(visible, vec!["overdue", "soon", "future", "nodue"]);
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

    #[test]
    fn delete_confirmation_is_single_and_clearable() {
        let mut ready = Ready {
            tasks: vec![task("a", TaskStatus::NotStarted), task("b", TaskStatus::NotStarted)],
            ..Default::default()
        };
        ready.request_delete("a");
        assert_eq!(ready.confirming_delete.as_deref(), Some("a"));
        ready.request_delete("b"); // replaces
        assert_eq!(ready.confirming_delete.as_deref(), Some("b"));
        ready.cancel_delete();
        assert_eq!(ready.confirming_delete, None);
    }

    #[test]
    fn visible_tasks_keeps_completing_task_in_place_while_hidden() {
        let mut ready = Ready {
            tasks: vec![
                task_due("a", TaskStatus::NotStarted, "2026-06-10"),
                task_due("b", TaskStatus::NotStarted, "2026-06-12"),
                task_due("c", TaskStatus::NotStarted, "2026-06-15"),
            ],
            show_completed: false,
            ..Default::default()
        };
        // Complete the middle task: now Completed, but still animating out.
        ready.toggle_optimistic("b");
        ready.begin_completing("b", Instant::now());
        // It stays in its due-sorted position instead of vanishing.
        let visible: Vec<&str> = ready.visible_tasks().iter().map(|t| t.id.as_str()).collect();
        assert_eq!(visible, vec!["a", "b", "c"]);
        // Once it stops animating, it's hidden like any completed task.
        ready.cancel_completing("b");
        let visible: Vec<&str> = ready.visible_tasks().iter().map(|t| t.id.as_str()).collect();
        assert_eq!(visible, vec!["a", "c"]);
    }

    #[test]
    fn toggling_guard_blocks_reentrant_status_change() {
        let mut ready = Ready::default();
        assert!(ready.begin_toggle("a")); // first toggle proceeds
        assert!(!ready.begin_toggle("a")); // second blocked while in flight
        ready.end_toggle("a");
        assert!(ready.begin_toggle("a")); // allowed again once it resolves
    }

    #[test]
    fn load_generation_supersedes_older_loads() {
        let mut ready = Ready { selected_list_id: "L1".into(), ..Default::default() };
        let g1 = ready.next_load_gen();
        let g2 = ready.next_load_gen();
        assert!(g2 > g1);
        assert!(ready.is_current_load("L1", g2)); // newest on current list
        assert!(!ready.is_current_load("L1", g1)); // superseded generation
        assert!(!ready.is_current_load("L2", g2)); // wrong list
    }

    #[test]
    fn switch_to_list_clears_all_list_scoped_state() {
        let mut ready = Ready {
            selected_list_id: "old".into(),
            tasks: vec![task("a", TaskStatus::NotStarted)],
            next_link: Some("https://graph/old/next".into()),
            loading_more: true,
            loaded_more: true,
            confirming_delete: Some("a".into()),
            ..Default::default()
        };
        ready.begin_completing("a", Instant::now());
        ready.switch_to_list("new".into());
        assert_eq!(ready.selected_list_id, "new");
        assert!(ready.tasks.is_empty());
        assert_eq!(ready.next_link, None);
        assert!(!ready.loading_more);
        assert!(!ready.loaded_more);
        assert_eq!(ready.confirming_delete, None);
        assert!(ready.completing.is_empty());
        assert!(ready.loading);
    }

    #[test]
    fn apply_refresh_preserves_completing_row() {
        let mut ready = Ready {
            tasks: vec![task("a", TaskStatus::NotStarted), task("b", TaskStatus::NotStarted)],
            show_completed: false,
            ..Default::default()
        };
        ready.toggle_optimistic("b"); // b -> Completed (optimistic)
        ready.begin_completing("b", Instant::now()); // animating out
        // A pending-only refresh (the server omits the now-completed "b") must
        // not drop the row while its exit animation is still playing.
        ready.apply_refresh(vec![task("a", TaskStatus::NotStarted)]);
        assert!(ready.tasks.iter().any(|t| t.id == "b"), "completing row preserved");
        let visible: Vec<&str> = ready.visible_tasks().iter().map(|t| t.id.as_str()).collect();
        assert_eq!(visible, vec!["a", "b"]);
    }

    #[test]
    fn completing_only_starts_once_and_keeps_first_instant() {
        let mut ready = Ready::default();
        let t0 = Instant::now();
        ready.begin_completing("a", t0);
        ready.begin_completing("a", t0 + Duration::from_secs(1)); // ignored
        assert!(ready.is_completing("a"));
        assert_eq!(ready.completing.get("a"), Some(&t0));
        ready.cancel_completing("a");
        assert!(!ready.is_completing("a"));
    }

    #[test]
    fn prune_completing_drops_only_finished_animations() {
        let mut ready = Ready::default();
        let start = Instant::now();
        ready.begin_completing("a", start);
        // Mid-animation: nothing pruned.
        assert!(!ready.prune_completing(start + COMPLETE_HOLD));
        assert!(ready.is_completing("a"));
        // Past the full duration: pruned, and the change is reported.
        assert!(ready.prune_completing(start + COMPLETE_HOLD + COMPLETE_COLLAPSE));
        assert!(!ready.is_completing("a"));
    }

    #[test]
    fn complete_anim_holds_full_height_then_collapses_to_none() {
        // Full height from the start through the entire hold phase.
        assert_eq!(complete_anim(Duration::ZERO).unwrap().max_height, COMPLETE_ROW_MAX_H);
        assert_eq!(complete_anim(COMPLETE_HOLD).unwrap().max_height, COMPLETE_ROW_MAX_H);
        // During the collapse the height shrinks monotonically.
        let quarter = complete_anim(COMPLETE_HOLD + COMPLETE_COLLAPSE / 4).unwrap().max_height;
        let three_q = complete_anim(COMPLETE_HOLD + COMPLETE_COLLAPSE * 3 / 4).unwrap().max_height;
        assert!(quarter < COMPLETE_ROW_MAX_H, "collapse should have begun");
        assert!(three_q < quarter, "height should keep decreasing");
        assert!(three_q > 0.0, "still partly open before the end");
        // At and past the end the animation is over.
        assert!(complete_anim(COMPLETE_HOLD + COMPLETE_COLLAPSE).is_none());
    }
}
