use std::sync::Arc;

use cosmic::app::Core;
use cosmic::cosmic_config::CosmicConfigEntry;
use cosmic::iced::platform_specific::shell::wayland::commands::popup::{destroy_popup, get_popup};
use cosmic::iced::window::Id;
use cosmic::iced::{time, Alignment, Length, Limits, Subscription};
use cosmic::prelude::*;
use cosmic::widget;

use outlook_tasks_core::auth::{
    AuthConfig, Authenticator, BootstrapOutcome, LoopbackServer, OAuthClient, Oo7TokenStore,
};
use outlook_tasks_core::graph::{GraphClient, TokenProvider};
use outlook_tasks_core::models::{TaskStatus, TodoList, TodoTask};
use outlook_tasks_core::GraphError;

use crate::config::Config;
use crate::consts::{ACCOUNT_ID, APP_ID, CLIENT_ID, GRAPH_BASE};
use crate::state::{AppState, PopupView, Ready};

/// How often to check loaded tasks for reminders that just came due.
const REMINDER_CHECK_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);
/// Reminders that came due longer ago than this many seconds - e.g. while the
/// applet was closed or the machine asleep - are skipped rather than replayed, so
/// a missed reminder doesn't resurface as a stale notification.
const REMINDER_GRACE_SECS: i64 = 120;

/// The authenticated services. Held as `Option` so a (practically impossible)
/// construction failure from malformed constants degrades to a config-error
/// state instead of panicking in `init`.
struct Services {
    auth: Arc<Authenticator>,
    graph: Arc<GraphClient>,
}

pub struct AppModel {
    core: Core,
    popup: Option<Id>,
    config: Config,
    state: AppState,
    services: Option<Services>,
    /// Timestamp of the last reminder check; `None` until the first tick sets the
    /// baseline so reminders from before startup don't fire.
    reminder_last_check: Option<jiff::Timestamp>,
}

/// Classified outcome of a Graph call, so the UI can react to auth-expiry and
/// throttling rather than treating every failure as a plain string.
#[derive(Debug, Clone)]
pub enum FetchError {
    /// Needs re-sign-in (401 after refresh, or token acquisition failed).
    Auth,
    /// Throttled; optional Retry-After seconds.
    Throttled(Option<u64>),
    /// Any other error, already rendered to a message.
    Other(String),
}

fn classify_graph(e: GraphError) -> FetchError {
    match e {
        GraphError::Unauthorized | GraphError::Token(_) => FetchError::Auth,
        GraphError::Throttled { retry_after } => {
            FetchError::Throttled(retry_after.map(|d| d.as_secs()))
        }
        other => FetchError::Other(other.to_string()),
    }
}

/// Formats a `YYYY-MM-DD` due day as e.g. "Jun 15"; falls back to the raw value.
fn format_due(day: &str) -> String {
    chrono::NaiveDate::parse_from_str(day, "%Y-%m-%d")
        .map(|d| d.format("%b %-d").to_string())
        .unwrap_or_else(|_| day.to_string())
}

/// A page of tasks plus the `@odata.nextLink` for the next page, if any.
type TaskPage = (Vec<TodoTask>, Option<String>);

#[derive(Debug, Clone)]
pub enum Message {
    TogglePopup,
    PopupClosed(Id),
    UpdateConfig(Config),
    Bootstrapped(BootstrapOutcome),
    SignIn,
    SignedIn(BootstrapOutcome),
    ListsLoaded(Result<Vec<TodoList>, FetchError>),
    SelectList(String),
    Tick,
    Refresh,
    /// Carries the list id the tasks were fetched for, so stale responses from a
    /// previously-selected list can be discarded.
    TasksLoaded(String, Result<TaskPage, FetchError>),
    /// Fetch the next page of tasks (the "Load more" button).
    LoadMore,
    MoreTasksLoaded(String, Result<TaskPage, FetchError>),
    ToggleTask(String),
    TaskUpdated(String, TaskStatus, Result<Box<TodoTask>, FetchError>),
    DeleteRequested(String),
    DeleteCancelled,
    DeleteConfirmed(String),
    TaskDeleted(Box<TodoTask>, Result<(), FetchError>),
    OpenCreate,
    OpenEdit(String),
    CancelForm,
    Form(crate::task_form::FormMsg),
    SaveForm,
    FormSaved(Result<Box<TodoTask>, FetchError>),
    ShowCompleted(bool),
    Retry,
    /// Periodic check for reminders that just came due.
    ReminderTick,
    /// Result of firing one reminder notification (logged on failure).
    ReminderNotified(Result<(), String>),
}

impl cosmic::Application for AppModel {
    type Executor = cosmic::executor::Default;
    type Flags = ();
    type Message = Message;
    const APP_ID: &'static str = APP_ID;

    fn core(&self) -> &Core {
        &self.core
    }
    fn core_mut(&mut self) -> &mut Core {
        &mut self.core
    }

    fn init(core: Core, _flags: ()) -> (Self, Task<cosmic::Action<Message>>) {
        let config = Config::load();

        // Build services without panicking. The Authenticator uses a redirect-less
        // OAuth client (refresh doesn't need a redirect); sign-in builds its own
        // client with the bound loopback.
        let (state, services, startup) = match Self::build_services() {
            Ok(services) => {
                let auth = services.auth.clone();
                let startup = Task::perform(async move { auth.bootstrap().await }, |o| {
                    cosmic::action::app(Message::Bootstrapped(o))
                });
                (AppState::SignedOut, Some(services), startup)
            }
            Err(e) => {
                log::error!("failed to initialise services: {e}");
                (AppState::Error(format!("Configuration error: {e}")), None, Task::none())
            }
        };

        let model = AppModel {
            core,
            popup: None,
            config,
            state,
            services,
            reminder_last_check: None,
        };
        (model, startup)
    }

    fn on_close_requested(&self, id: Id) -> Option<Message> {
        Some(Message::PopupClosed(id))
    }

    fn view(&self) -> Element<'_, Message> {
        let today = chrono::Local::now().format("%Y-%m-%d").to_string();
        let due = match &self.state {
            AppState::Ready(ready) => ready.due_count(&today),
            _ => 0,
        };
        if due == 0 {
            return self
                .core
                .applet
                .icon_button("checkbox-checked-symbolic")
                .on_press(Message::TogglePopup)
                .into();
        }
        // Panel badge: icon + count of currently-due tasks.
        let body = widget::Row::new()
            .push(widget::icon::from_name("checkbox-checked-symbolic").size(16))
            .push(widget::text::body(due.to_string()))
            .spacing(4)
            .align_y(Alignment::Center);
        widget::button::custom(body)
            .class(cosmic::theme::Button::AppletIcon)
            .on_press(Message::TogglePopup)
            .into()
    }

    fn view_window(&self, _id: Id) -> Element<'_, Message> {
        let content: Element<'_, Message> = match &self.state {
            AppState::NoKeyring => widget::Column::new()
                .push(widget::text::title4("No keyring found"))
                .push(widget::text::body(
                    "Install gnome-keyring or KWallet to store your sign-in, then retry.",
                ))
                .push(widget::button::standard("Retry").on_press(Message::Retry))
                .spacing(8)
                .padding(12)
                .into(),
            AppState::SignedOut => widget::Column::new()
                .push(widget::text::title4("Outlook Tasks"))
                .push(widget::text::body("Sign in to your outlook.com account."))
                .push(widget::button::suggested("Sign in").on_press(Message::SignIn))
                .spacing(8)
                .padding(12)
                .into(),
            AppState::Authenticating => widget::Column::new()
                .push(widget::text::body("Waiting for sign-in in your browser..."))
                .padding(12)
                .into(),
            AppState::Error(msg) => widget::Column::new()
                .push(widget::text::title4("Something went wrong"))
                .push(widget::text::body(msg.as_str()))
                .spacing(8)
                .padding(12)
                .into(),
            AppState::Ready(ready) => match &ready.view {
                PopupView::List => self.ready_view(ready),
                PopupView::Form(form) => crate::task_form::form_view(form),
            },
        };
        self.core.applet.popup_container(content).into()
    }

    fn subscription(&self) -> Subscription<Message> {
        let poll = time::every(std::time::Duration::from_secs(self.config.poll_interval_secs.max(60)))
            .map(|_| Message::Tick);
        let reminders = time::every(REMINDER_CHECK_INTERVAL).map(|_| Message::ReminderTick);
        let cfg = self
            .core()
            .watch_config::<Config>(APP_ID)
            .map(|update| Message::UpdateConfig(update.config));
        Subscription::batch(vec![poll, reminders, cfg])
    }

    fn update(&mut self, message: Message) -> Task<cosmic::Action<Message>> {
        match message {
            Message::UpdateConfig(config) => {
                let switched = config.selected_list_id != self.config.selected_list_id;
                self.config = config;
                if switched
                    && let (AppState::Ready(ready), Some(id)) =
                        (&mut self.state, self.config.selected_list_id.clone())
                    && ready.selected_list_id != id
                    && ready.lists.iter().any(|l| l.id == id)
                {
                    ready.selected_list_id = id.clone();
                    ready.loading = true;
                    ready.tasks.clear();
                    return self.load_tasks(id);
                }
            }

            Message::TogglePopup => return self.toggle_popup(),
            Message::PopupClosed(id) => {
                if self.popup == Some(id) {
                    self.popup = None;
                }
            }

            // A fresh sign-in and a startup check both produce a BootstrapOutcome.
            Message::Bootstrapped(outcome) | Message::SignedIn(outcome) => {
                return self.apply_outcome(outcome);
            }

            Message::Retry => {
                self.state = AppState::SignedOut;
                return self.rebootstrap();
            }

            Message::SignIn => {
                // Ignore a second click while a browser flow is already running.
                if matches!(self.state, AppState::Authenticating) {
                    return Task::none();
                }
                self.state = AppState::Authenticating;
                return self.sign_in();
            }

            Message::ListsLoaded(Ok(lists)) => {
                let persisted = self.config.selected_list_id.clone();
                match Ready::pick_initial_list(&lists, persisted.as_deref()) {
                    Some(id) => {
                        // Persist if we fell back from a dead/absent persisted id so
                        // a stale list id doesn't linger in config forever.
                        if persisted.as_deref() != Some(id.as_str()) {
                            self.config.selected_list_id = Some(id.clone());
                            self.persist_config();
                        }
                        self.state = AppState::Ready(Ready {
                            lists,
                            selected_list_id: id.clone(),
                            loading: true,
                            ..Default::default()
                        });
                        return self.load_tasks(id);
                    }
                    None => self.state = AppState::Ready(Ready { lists, ..Default::default() }),
                }
            }
            Message::ListsLoaded(Err(e)) => return self.handle_fetch_error(e),

            Message::SelectList(id) => {
                self.config.selected_list_id = Some(id.clone());
                self.persist_config();
                if let AppState::Ready(ready) = &mut self.state {
                    ready.selected_list_id = id.clone();
                    ready.loading = true;
                    ready.tasks.clear();
                }
                return self.load_tasks(id);
            }

            // Periodic poll: skip while extra completed pages are loaded so they're
            // not collapsed. An explicit Refresh always reloads from page 1.
            Message::Tick => {
                if let AppState::Ready(ready) = &self.state
                    && !ready.selected_list_id.is_empty()
                    && !ready.loaded_more
                {
                    return self.load_tasks(ready.selected_list_id.clone());
                }
            }
            Message::Refresh => {
                if let AppState::Ready(ready) = &self.state
                    && !ready.selected_list_id.is_empty()
                {
                    return self.load_tasks(ready.selected_list_id.clone());
                }
            }

            Message::ReminderTick => return self.check_reminders(),
            Message::ReminderNotified(Err(e)) => log::warn!("reminder notification failed: {e}"),
            Message::ReminderNotified(Ok(())) => {}
            Message::TasksLoaded(list_id, result) => {
                // Discard responses for a list the user already navigated away from.
                let current =
                    matches!(&self.state, AppState::Ready(r) if r.selected_list_id == list_id);
                if !current {
                    return Task::none();
                }
                match result {
                    Ok((tasks, next_link)) => {
                        if let AppState::Ready(ready) = &mut self.state {
                            ready.apply_refresh(tasks);
                            ready.next_link = next_link;
                            ready.loaded_more = false;
                            ready.loading_more = false;
                            ready.loading = false;
                            ready.error = None;
                        }
                    }
                    Err(e) => return self.handle_fetch_error(e),
                }
            }

            Message::LoadMore => return self.load_more(),
            Message::MoreTasksLoaded(list_id, result) => {
                let current =
                    matches!(&self.state, AppState::Ready(r) if r.selected_list_id == list_id);
                if !current {
                    return Task::none();
                }
                match result {
                    Ok((tasks, next_link)) => {
                        if let AppState::Ready(ready) = &mut self.state {
                            ready.append_page(tasks);
                            ready.next_link = next_link;
                            ready.loaded_more = true;
                            ready.loading_more = false;
                        }
                    }
                    Err(e) => {
                        if let AppState::Ready(ready) = &mut self.state {
                            ready.loading_more = false;
                        }
                        return self.handle_fetch_error(e);
                    }
                }
            }

            Message::ShowCompleted(show) => {
                // Refetch: ticking on loads completed tasks; ticking off returns
                // to the fast pending-only request.
                let list_id = if let AppState::Ready(ready) = &mut self.state {
                    ready.show_completed = show;
                    ready.loading = true;
                    (!ready.selected_list_id.is_empty()).then(|| ready.selected_list_id.clone())
                } else {
                    None
                };
                if let Some(list_id) = list_id {
                    return self.load_tasks(list_id);
                }
            }

            Message::ToggleTask(task_id) => return self.toggle_task(task_id),
            Message::TaskUpdated(_id, _prev, Ok(updated)) => {
                if let AppState::Ready(ready) = &mut self.state
                    && let Some(t) = ready.tasks.iter_mut().find(|t| t.id == updated.id)
                {
                    *t = *updated;
                }
            }
            Message::TaskUpdated(task_id, prev, Err(e)) => {
                if let AppState::Ready(ready) = &mut self.state {
                    ready.restore_status(&task_id, prev);
                }
                return self.handle_fetch_error(e);
            }

            Message::DeleteRequested(id) => {
                // A not-yet-created task has no server id - can't be deleted.
                if !crate::state::Ready::is_placeholder(&id)
                    && let AppState::Ready(ready) = &mut self.state
                {
                    ready.request_delete(&id);
                }
            }
            Message::DeleteCancelled => {
                if let AppState::Ready(ready) = &mut self.state {
                    ready.cancel_delete();
                }
            }
            Message::DeleteConfirmed(id) => return self.delete_task(id),
            Message::TaskDeleted(task, Ok(())) => {
                // Ensure it's gone even if a poll re-added it between remove and ack.
                if let AppState::Ready(ready) = &mut self.state {
                    ready.tasks.retain(|t| t.id != task.id);
                }
            }
            Message::TaskDeleted(task, Err(e)) => {
                // Restore the optimistically-removed task, then route the typed error
                // (auth-expiry/throttle handled by handle_fetch_error).
                if let AppState::Ready(ready) = &mut self.state
                    && !ready.tasks.iter().any(|t| t.id == task.id)
                {
                    ready.tasks.push(*task);
                }
                return self.handle_fetch_error(e);
            }

            Message::OpenCreate => {
                if let AppState::Ready(ready) = &mut self.state {
                    ready.view = PopupView::Form(Box::new(crate::task_form::TaskForm::create()));
                }
            }
            Message::OpenEdit(id) => {
                // A not-yet-created (temp-) task can't be edited via PATCH.
                if !crate::state::Ready::is_placeholder(&id)
                    && let AppState::Ready(ready) = &mut self.state
                    && let Some(task) = ready.tasks.iter().find(|t| t.id == id)
                {
                    ready.view = PopupView::Form(Box::new(crate::task_form::TaskForm::from_task(task)));
                }
            }
            Message::CancelForm => {
                if let AppState::Ready(ready) = &mut self.state {
                    ready.view = PopupView::List;
                }
            }
            Message::Form(fmsg) => {
                if let AppState::Ready(ready) = &mut self.state
                    && let PopupView::Form(form) = &mut ready.view
                {
                    form.as_mut().apply(fmsg);
                }
            }
            Message::SaveForm => return self.save_form(),
            Message::FormSaved(Ok(task)) => {
                // Apply the returned task immediately (the next poll reconciles the rest):
                // replace by id on edit, push on create.
                if let AppState::Ready(ready) = &mut self.state {
                    let id = task.id.clone();
                    if let Some(existing) = ready.tasks.iter_mut().find(|t| t.id == id) {
                        *existing = *task;
                    } else {
                        ready.tasks.push(*task);
                    }
                    ready.view = crate::state::PopupView::List;
                }
            }
            Message::FormSaved(Err(e)) => {
                log::error!("save failed: {e:?}");
                // A request error (e.g. a Graph 400) carries the real reason in its
                // body; show it in the form and keep the form open so it can be
                // corrected and retried, rather than a generic "Save failed".
                if let FetchError::Other(msg) = &e {
                    if let AppState::Ready(ready) = &mut self.state
                        && let crate::state::PopupView::Form(form) = &mut ready.view
                    {
                        form.error = Some(msg.clone());
                    }
                    return Task::none();
                }
                // Auth-expiry and throttling keep their existing handling.
                return self.handle_fetch_error(e);
            }
        }
        Task::none()
    }

    fn style(&self) -> Option<cosmic::iced::theme::Style> {
        Some(cosmic::applet::style())
    }
}

impl AppModel {
    fn ready_view<'a>(&'a self, ready: &'a Ready) -> Element<'a, Message> {
        use cosmic::iced::advanced::text::{Ellipsize, EllipsizeHeightLimit, Wrapping};
        let today = chrono::Local::now().format("%Y-%m-%d").to_string();

        // Header: list dropdown + refresh.
        let names: Vec<String> = ready.lists.iter().map(|l| l.display_name.clone()).collect();
        let selected_idx = ready
            .lists
            .iter()
            .position(|l| l.id == ready.selected_list_id);
        let lists_for_pick = ready.lists.clone();
        let dropdown = widget::dropdown(names, selected_idx, move |i| {
            Message::SelectList(lists_for_pick[i].id.clone())
        });

        let header = widget::Row::new()
            .push(dropdown)
            .push(widget::space::horizontal())
            .push(
                widget::button::icon(widget::icon::from_name("list-add-symbolic"))
                    .on_press(Message::OpenCreate),
            )
            .push(
                widget::button::icon(widget::icon::from_name("view-refresh-symbolic"))
                    .on_press(Message::Refresh),
            )
            .align_y(Alignment::Center)
            .spacing(8);

        // Task rows: title on the left, due date on the right (red when due).
        // Right padding keeps the due dates clear of the scrollbar lane.
        let visible = ready.visible_tasks();
        let mut list = widget::Column::new()
            .spacing(4)
            .padding(cosmic::iced::Padding { top: 0.0, right: 12.0, bottom: 0.0, left: 0.0 });
        for task in &visible {
            let id = task.id.clone();
            // A row pending delete confirmation shows a prompt instead of the task.
            if ready.confirming_delete.as_deref() == Some(task.id.as_str()) {
                let confirm = widget::Row::new()
                    .push(widget::text::body(format!("Delete \u{201c}{}\u{201d}?", task.title)))
                    .push(widget::space::horizontal())
                    .push(
                        widget::button::destructive("Delete")
                            .on_press(Message::DeleteConfirmed(id.clone())),
                    )
                    .push(widget::button::text("Cancel").on_press(Message::DeleteCancelled))
                    .align_y(Alignment::Center)
                    .spacing(8);
                list = list.push(confirm);
                continue;
            }
            let checked = task.status == TaskStatus::Completed;
            let edit_id = id.clone();
            let mut row = widget::Row::new()
                .push(widget::checkbox(checked).on_toggle({
                    let id = id.clone();
                    move |_| Message::ToggleTask(id.clone())
                }));
            // High-importance marker: a red "!" between the checkbox and the title.
            if task.importance == outlook_tasks_core::models::Importance::High {
                row = row.push(widget::text::body("!").class(cosmic::theme::Text::Color(
                    cosmic::iced::Color::from_rgb(0.8, 0.2, 0.2),
                )));
            }
            // Single-line title that ellipsizes when too long, taking the leftover
            // width so the due date and trash stay pinned and fully visible. A real
            // task's title is a link that opens the edit form; a temp (not-yet-
            // created) row has no server id to edit, so it stays plain text.
            let title_text = widget::text::body(task.title.clone())
                .wrapping(Wrapping::None)
                .ellipsize(Ellipsize::End(EllipsizeHeightLimit::Lines(1)))
                .width(Length::Fill);
            let title: Element<'_, Message> = if Ready::is_placeholder(&task.id) {
                title_text.into()
            } else {
                widget::button::custom(title_text)
                    .class(cosmic::theme::Button::Link)
                    .on_press(Message::OpenEdit(edit_id))
                    .width(Length::Fill)
                    .into()
            };
            let mut row = row.push(title).align_y(Alignment::Center).spacing(8);
            if let Some(day) = task.due_day() {
                let mut due_label = widget::text::caption(format_due(day));
                if crate::state::is_due(day, &today) {
                    due_label = due_label.class(cosmic::theme::Text::Color(
                        cosmic::iced::Color::from_rgb(0.8, 0.2, 0.2),
                    ));
                }
                row = row.push(due_label);
            }
            // No trash on a not-yet-created (temp-) row.
            if !Ready::is_placeholder(&task.id) {
                let delete = widget::button::icon(widget::icon::from_name("user-trash-symbolic"))
                    .on_press(Message::DeleteRequested(id.clone()));
                row = row.push(widget::tooltip(
                    delete,
                    widget::text::body("Delete task"),
                    widget::tooltip::Position::Top,
                ));
            }
            list = list.push(row);
        }
        if visible.is_empty() {
            let msg = if ready.loading {
                "Loading..."
            } else if ready.show_completed {
                "No tasks."
            } else {
                "No pending tasks."
            };
            list = list.push(widget::text::body(msg));
        } else if ready.loading {
            // Reload in progress (e.g. fetching completed) - shown below current rows.
            list = list.push(widget::text::caption("Loading..."));
        }
        // "Load more" for the next page of (completed) tasks, when one exists.
        if ready.next_link.is_some() {
            let label = if ready.loading_more { "Loading..." } else { "Load more" };
            let load_more = widget::button::text(label)
                .on_press_maybe((!ready.loading_more).then_some(Message::LoadMore));
            list = list.push(
                widget::Row::new()
                    .push(widget::space::horizontal())
                    .push(load_more)
                    .push(widget::space::horizontal())
                    .align_y(Alignment::Center),
            );
        }

        let show_completed_toggle = widget::Row::new()
            .push(widget::checkbox(ready.show_completed).on_toggle(Message::ShowCompleted))
            .push(widget::text::body("Show completed"))
            .align_y(Alignment::Center)
            .spacing(8);

        let mut col = widget::Column::new()
            .push(header)
            .push(widget::text::caption(format!(
                "{} open · {} due",
                ready.open_count(),
                ready.due_count(&today)
            )))
            .push(show_completed_toggle)
            .push(widget::divider::horizontal::default())
            .push(widget::scrollable(list).height(Length::Fixed(280.0)))
            .spacing(8)
            .padding(12);
        if let Some(err) = &ready.error {
            col = col.push(widget::text::caption(err).class(cosmic::theme::Text::Color(
                cosmic::iced::Color::from_rgb(0.8, 0.2, 0.2),
            )));
        }
        col.into()
    }

    /// Fires notifications for reminders that crossed their time since the last
    /// check. The first call only records a baseline. Catch-up is capped by
    /// `REMINDER_GRACE` so reminders missed while closed/asleep aren't replayed.
    fn check_reminders(&mut self) -> Task<cosmic::Action<Message>> {
        let now = jiff::Timestamp::now();
        let Some(last) = self.reminder_last_check.replace(now) else {
            return Task::none();
        };
        let floor = jiff::Timestamp::from_second(now.as_second() - REMINDER_GRACE_SECS).unwrap_or(now);
        let lower = last.max(floor);
        let AppState::Ready(ready) = &self.state else {
            return Task::none();
        };
        let due = crate::reminders::due_reminders(&ready.tasks, lower, now);
        if due.is_empty() {
            return Task::none();
        }
        let notifications: Vec<Task<cosmic::Action<Message>>> = due
            .iter()
            .map(|task| {
                let summary = task.title.clone();
                Task::perform(crate::notify::notify(summary, "Reminder".to_string()), |res| {
                    cosmic::action::app(Message::ReminderNotified(res.map_err(|e| e.to_string())))
                })
            })
            .collect();
        Task::batch(notifications)
    }

    fn toggle_popup(&mut self) -> Task<cosmic::Action<Message>> {
        if let Some(p) = self.popup.take() {
            return destroy_popup(p);
        }
        // Never unwrap the main window id - a missing parent must not panic.
        let Some(parent) = self.core.main_window_id() else {
            log::warn!("no main window id; cannot open popup");
            return Task::none();
        };
        let new_id = Id::unique();
        self.popup = Some(new_id);
        let mut settings = self.core.applet.get_popup_settings(parent, new_id, None, None, None);
        settings.positioner.size_limits = Limits::NONE
            .max_width(372.0)
            .min_width(300.0)
            .min_height(200.0)
            .max_height(1080.0);
        let open_task = get_popup(settings);
        // Refresh on open if signed in.
        if matches!(self.state, AppState::Ready(_)) {
            return Task::batch([open_task, Task::done(cosmic::action::app(Message::Refresh))]);
        }
        open_task
    }

    /// Builds the authenticator + Graph client. Fails only on a malformed
    /// compile-time constant (client id / URL), which `init` renders as an error
    /// state rather than a panic.
    fn build_services() -> Result<Services, String> {
        // Disable redirects: a 3xx from Graph must not redirect a bearer-bearing
        // request to another origin.
        let http = reqwest::ClientBuilder::new()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| e.to_string())?;
        let refresh_oauth =
            OAuthClient::new(&AuthConfig::consumers(CLIENT_ID, "http://localhost/"))
                .map_err(|e| e.to_string())?;
        let store = Arc::new(Oo7TokenStore::new(APP_ID, ACCOUNT_ID));
        let auth = Arc::new(Authenticator::new(refresh_oauth, store, ACCOUNT_ID));
        let graph =
            Arc::new(GraphClient::new(GRAPH_BASE, http, auth.clone() as Arc<dyn TokenProvider>));
        Ok(Services { auth, graph })
    }

    /// Applies a startup/sign-in outcome to the UI state.
    fn apply_outcome(&mut self, outcome: BootstrapOutcome) -> Task<cosmic::Action<Message>> {
        match outcome {
            BootstrapOutcome::Ready => {
                self.state = AppState::Ready(Ready { loading: true, ..Default::default() });
                self.load_lists()
            }
            BootstrapOutcome::SignedOut => {
                self.state = AppState::SignedOut;
                Task::none()
            }
            BootstrapOutcome::NoKeyring => {
                self.state = AppState::NoKeyring;
                Task::none()
            }
        }
    }

    /// Re-runs the startup session check (used after an auth-expiry error).
    fn rebootstrap(&self) -> Task<cosmic::Action<Message>> {
        let Some(services) = &self.services else {
            return Task::none();
        };
        let auth = services.auth.clone();
        Task::perform(async move { auth.bootstrap().await }, |o| {
            cosmic::action::app(Message::Bootstrapped(o))
        })
    }

    /// Schedules a single delayed refresh (honors a 429 Retry-After).
    fn schedule_retry(&self, secs: u64) -> Task<cosmic::Action<Message>> {
        let wait = std::time::Duration::from_secs(secs.clamp(1, 3600));
        Task::perform(async move { tokio::time::sleep(wait).await }, |_| {
            cosmic::action::app(Message::Refresh)
        })
    }

    /// Routes a classified Graph error: re-auth on expiry, back off on throttle,
    /// otherwise show the message.
    fn handle_fetch_error(&mut self, error: FetchError) -> Task<cosmic::Action<Message>> {
        match error {
            FetchError::Auth => {
                self.state = AppState::SignedOut;
                self.rebootstrap()
            }
            FetchError::Throttled(secs) => {
                let wait = secs.unwrap_or(60);
                if let AppState::Ready(ready) = &mut self.state {
                    ready.loading = false;
                    ready.error = Some(format!("Rate limited - retrying in {wait}s"));
                }
                self.schedule_retry(wait)
            }
            FetchError::Other(msg) => {
                self.set_error(msg);
                Task::none()
            }
        }
    }

    fn sign_in(&mut self) -> Task<cosmic::Action<Message>> {
        let Some(services) = &self.services else {
            return Task::none();
        };
        let auth = services.auth.clone();
        Task::perform(async move { run_sign_in(auth).await }, |o| {
            cosmic::action::app(Message::SignedIn(o))
        })
    }

    fn load_lists(&self) -> Task<cosmic::Action<Message>> {
        let Some(services) = &self.services else {
            return Task::none();
        };
        let graph = services.graph.clone();
        Task::perform(
            async move { graph.list_lists().await.map_err(classify_graph) },
            |r| cosmic::action::app(Message::ListsLoaded(r)),
        )
    }

    fn load_tasks(&self, list_id: String) -> Task<cosmic::Action<Message>> {
        let Some(services) = &self.services else {
            return Task::none();
        };
        // Pending is always fetched complete; completed is fetched as a first page
        // (newest-due first) only when opted in, with "load more" for the rest.
        let include_completed = matches!(&self.state, AppState::Ready(r) if r.show_completed);
        let graph = services.graph.clone();
        let id_for_msg = list_id.clone();
        Task::perform(
            async move {
                let pending = graph.list_pending(&list_id).await.map_err(classify_graph)?;
                let (completed, next) = if include_completed {
                    graph.list_completed_page(&list_id).await.map_err(classify_graph)?
                } else {
                    (Vec::new(), None)
                };
                let mut tasks = pending;
                tasks.extend(completed);
                Ok::<TaskPage, FetchError>((tasks, next))
            },
            move |r| cosmic::action::app(Message::TasksLoaded(id_for_msg.clone(), r)),
        )
    }

    /// Loads the next page of tasks ("Load more") and appends it to the list.
    fn load_more(&mut self) -> Task<cosmic::Action<Message>> {
        let Some(graph) = self.services.as_ref().map(|s| s.graph.clone()) else {
            return Task::none();
        };
        let AppState::Ready(ready) = &mut self.state else {
            return Task::none();
        };
        if ready.loading_more {
            return Task::none();
        }
        let Some(next) = ready.next_link.clone() else {
            return Task::none();
        };
        ready.loading_more = true;
        let list_id = ready.selected_list_id.clone();
        Task::perform(
            async move { graph.list_tasks_page(&next).await.map_err(classify_graph) },
            move |r| cosmic::action::app(Message::MoreTasksLoaded(list_id.clone(), r)),
        )
    }

    fn toggle_task(&mut self, task_id: String) -> Task<cosmic::Action<Message>> {
        // Don't PATCH a placeholder that has no server id yet.
        if Ready::is_placeholder(&task_id) {
            return Task::none();
        }
        let Some(services) = &self.services else {
            return Task::none();
        };
        let graph = services.graph.clone();
        let AppState::Ready(ready) = &mut self.state else {
            return Task::none();
        };
        let Some(prev) = ready.toggle_optimistic(&task_id) else {
            return Task::none();
        };
        let new_status = if prev == TaskStatus::Completed {
            TaskStatus::NotStarted
        } else {
            TaskStatus::Completed
        };
        let list_id = ready.selected_list_id.clone();
        let id_for_msg = task_id.clone();
        Task::perform(
            async move {
                graph.set_status(&list_id, &task_id, new_status).await.map(Box::new).map_err(classify_graph)
            },
            move |r| cosmic::action::app(Message::TaskUpdated(id_for_msg.clone(), prev, r)),
        )
    }

    fn delete_task(&mut self, task_id: String) -> Task<cosmic::Action<Message>> {
        let Some(services) = &self.services else { return Task::none() };
        let graph = services.graph.clone();
        let AppState::Ready(ready) = &mut self.state else { return Task::none() };
        ready.cancel_delete();
        let Some(pos) = ready.tasks.iter().position(|t| t.id == task_id) else {
            return Task::none();
        };
        let removed = ready.tasks.remove(pos); // optimistic remove
        let list_id = ready.selected_list_id.clone();
        let carried = removed.clone();
        Task::perform(
            async move { graph.delete_task(&list_id, &task_id).await.map_err(classify_graph) },
            move |r| cosmic::action::app(Message::TaskDeleted(Box::new(carried.clone()), r)),
        )
    }

    fn save_form(&mut self) -> Task<cosmic::Action<Message>> {
        let Some(services) = &self.services else { return Task::none() };
        let graph = services.graph.clone();
        let tz = system_tz();
        let AppState::Ready(ready) = &mut self.state else { return Task::none() };
        let crate::state::PopupView::Form(form) = &mut ready.view else { return Task::none() };
        let input = match form.to_input(&tz) {
            Ok(i) => i,
            Err(msg) => {
                form.error = Some(msg.to_string());
                return Task::none();
            }
        };
        let mode = form.mode.clone();
        let list_id = ready.selected_list_id.clone();
        Task::perform(
            async move {
                match mode {
                    crate::task_form::FormMode::Create => graph
                        .create_task(&list_id, &input)
                        .await
                        .map(Box::new)
                        .map_err(classify_graph),
                    crate::task_form::FormMode::Edit { task_id } => graph
                        .update_task(&list_id, &task_id, &input)
                        .await
                        .map(Box::new)
                        .map_err(classify_graph),
                }
            },
            |r| cosmic::action::app(Message::FormSaved(r)),
        )
    }

    fn set_error(&mut self, message: String) {
        if let AppState::Ready(ready) = &mut self.state {
            ready.loading = false;
            ready.error = Some(message);
        } else {
            log::error!("{message}");
        }
    }

    fn persist_config(&self) {
        if let Ok(ctx) = cosmic::cosmic_config::Config::new(APP_ID, Config::VERSION)
            && let Err(e) = self.config.write_entry(&ctx)
        {
            log::warn!("failed to persist config: {e:?}");
        }
    }
}

/// The system IANA timezone name (e.g. "America/Sao_Paulo"), or "UTC" if unknown.
/// Used for `dueDateTime`/`reminderDateTime` so reminders fire at local wall time.
fn system_tz() -> String {
    jiff::tz::TimeZone::system()
        .iana_name()
        .map(str::to_string)
        .unwrap_or_else(|| "UTC".to_string())
}

/// Runs the full interactive sign-in and maps the outcome (including a missing
/// keyring on save) to a BootstrapOutcome so the UI shows the right state.
async fn run_sign_in(auth: Arc<Authenticator>) -> BootstrapOutcome {
    match try_sign_in(&auth).await {
        Ok(()) => BootstrapOutcome::Ready,
        Err(SignInError::NoKeyring) => BootstrapOutcome::NoKeyring,
        Err(SignInError::Other(e)) => {
            log::error!("sign-in failed: {e}");
            BootstrapOutcome::SignedOut
        }
    }
}

enum SignInError {
    NoKeyring,
    Other(String),
}

async fn try_sign_in(auth: &Authenticator) -> Result<(), SignInError> {
    use outlook_tasks_core::AuthError;

    let loopback = LoopbackServer::bind().map_err(|e| SignInError::Other(e.to_string()))?;
    let redirect = loopback.redirect_url();
    let oauth = OAuthClient::new(&AuthConfig::consumers(CLIENT_ID, &redirect))
        .map_err(|e| SignInError::Other(e.to_string()))?;
    let pending = oauth.begin_auth();
    let expected_state = pending.csrf_state.clone();
    open::that(pending.authorize_url.to_string())
        .map_err(|e| SignInError::Other(e.to_string()))?;
    // 5-minute window for the user to complete the browser flow.
    let params = loopback
        .wait_for_code(expected_state, std::time::Duration::from_secs(300))
        .await
        .map_err(|e| SignInError::Other(e.to_string()))?;
    let tokens = oauth
        .exchange_code(pending, params.code, params.state)
        .await
        .map_err(|e| SignInError::Other(e.to_string()))?;
    auth.complete_sign_in(tokens).await.map_err(|e| match e {
        AuthError::Store(_) => SignInError::NoKeyring,
        other => SignInError::Other(other.to_string()),
    })
}
