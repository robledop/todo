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
use crate::state::{AppState, Ready};

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
    /// Monotonic counter for optimistic placeholder ids.
    temp_seq: u64,
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
    TasksLoaded(String, Result<Vec<TodoTask>, FetchError>),
    AddInput(String),
    AddSubmit,
    TaskCreated(String, Result<TodoTask, FetchError>),
    ToggleTask(String),
    TaskUpdated(String, TaskStatus, Result<TodoTask, FetchError>),
    ShowCompleted(bool),
    Retry,
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

        let model = AppModel { core, popup: None, config, state, services, temp_seq: 0 };
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
            AppState::Ready(ready) => self.ready_view(ready),
        };
        self.core.applet.popup_container(content).into()
    }

    fn subscription(&self) -> Subscription<Message> {
        let poll = time::every(std::time::Duration::from_secs(self.config.poll_interval_secs.max(60)))
            .map(|_| Message::Tick);
        let cfg = self
            .core()
            .watch_config::<Config>(APP_ID)
            .map(|update| Message::UpdateConfig(update.config));
        Subscription::batch(vec![poll, cfg])
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

            Message::Tick | Message::Refresh => {
                if let AppState::Ready(ready) = &self.state
                    && !ready.selected_list_id.is_empty()
                {
                    return self.load_tasks(ready.selected_list_id.clone());
                }
            }
            Message::TasksLoaded(list_id, result) => {
                // Discard responses for a list the user already navigated away from.
                let current =
                    matches!(&self.state, AppState::Ready(r) if r.selected_list_id == list_id);
                if !current {
                    return Task::none();
                }
                match result {
                    Ok(tasks) => {
                        if let AppState::Ready(ready) = &mut self.state {
                            ready.apply_refresh(tasks);
                            ready.loading = false;
                            ready.error = None;
                        }
                    }
                    Err(e) => return self.handle_fetch_error(e),
                }
            }

            Message::AddInput(value) => {
                if let AppState::Ready(ready) = &mut self.state {
                    ready.add_input = value;
                }
            }
            Message::AddSubmit => return self.add_task(),

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

            Message::TaskCreated(temp_id, Ok(created)) => {
                if let AppState::Ready(ready) = &mut self.state {
                    ready.reconcile_created(&temp_id, created);
                }
            }
            Message::TaskCreated(temp_id, Err(e)) => {
                if let AppState::Ready(ready) = &mut self.state {
                    ready.remove_task(&temp_id);
                }
                return self.handle_fetch_error(e);
            }

            Message::ToggleTask(task_id) => return self.toggle_task(task_id),
            Message::TaskUpdated(_id, _prev, Ok(updated)) => {
                if let AppState::Ready(ready) = &mut self.state
                    && let Some(t) = ready.tasks.iter_mut().find(|t| t.id == updated.id)
                {
                    *t = updated;
                }
            }
            Message::TaskUpdated(task_id, prev, Err(e)) => {
                if let AppState::Ready(ready) = &mut self.state {
                    ready.restore_status(&task_id, prev);
                }
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
                widget::button::icon(widget::icon::from_name("view-refresh-symbolic"))
                    .on_press(Message::Refresh),
            )
            .align_y(Alignment::Center)
            .spacing(8);

        // Task rows: title on the left, due date on the right (red when due).
        let visible = ready.visible_tasks();
        let mut list = widget::Column::new().spacing(4);
        for task in &visible {
            let id = task.id.clone();
            let checked = task.status == TaskStatus::Completed;
            let mut row = widget::Row::new()
                .push(widget::checkbox(checked).on_toggle(move |_| Message::ToggleTask(id.clone())))
                .push(widget::text::body(&task.title))
                .align_y(Alignment::Center)
                .spacing(8);
            if let Some(day) = task.due_day() {
                let mut due_label = widget::text::caption(format_due(day));
                if crate::state::is_due(day, &today) {
                    due_label = due_label.class(cosmic::theme::Text::Color(
                        cosmic::iced::Color::from_rgb(0.8, 0.2, 0.2),
                    ));
                }
                row = row.push(widget::space::horizontal()).push(due_label);
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

        let show_completed_toggle = widget::Row::new()
            .push(widget::checkbox(ready.show_completed).on_toggle(Message::ShowCompleted))
            .push(widget::text::body("Show completed"))
            .align_y(Alignment::Center)
            .spacing(8);

        let add = widget::text_input("Add a task...", &ready.add_input)
            .on_input(Message::AddInput)
            .on_submit(|_| Message::AddSubmit);

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
            .push(widget::divider::horizontal::default())
            .push(add)
            .spacing(8)
            .padding(12);
        if let Some(err) = &ready.error {
            col = col.push(widget::text::caption(err).class(cosmic::theme::Text::Color(
                cosmic::iced::Color::from_rgb(0.8, 0.2, 0.2),
            )));
        }
        col.into()
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
        // Only fetch completed tasks when the user has opted in - pending-only is
        // a single fast request; "all" pages through every completed task.
        let include_completed = matches!(&self.state, AppState::Ready(r) if r.show_completed);
        let graph = services.graph.clone();
        let id_for_msg = list_id.clone();
        Task::perform(
            async move { graph.list_tasks(&list_id, include_completed).await.map_err(classify_graph) },
            move |r| cosmic::action::app(Message::TasksLoaded(id_for_msg.clone(), r)),
        )
    }

    fn add_task(&mut self) -> Task<cosmic::Action<Message>> {
        let Some(services) = &self.services else {
            return Task::none();
        };
        let graph = services.graph.clone();
        let AppState::Ready(ready) = &mut self.state else {
            return Task::none();
        };
        let title = ready.add_input.trim().to_string();
        if title.is_empty() {
            return Task::none();
        }
        self.temp_seq += 1;
        let temp_id = format!("temp-{}", self.temp_seq);
        ready.tasks.push(TodoTask {
            id: temp_id.clone(),
            title: title.clone(),
            status: TaskStatus::NotStarted,
            last_modified_date_time: None,
            due_date_time: None,
        });
        ready.add_input.clear();
        let list_id = ready.selected_list_id.clone();
        Task::perform(
            async move { graph.create_task(&list_id, &title).await.map_err(classify_graph) },
            move |r| cosmic::action::app(Message::TaskCreated(temp_id.clone(), r)),
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
                graph.set_status(&list_id, &task_id, new_status).await.map_err(classify_graph)
            },
            move |r| cosmic::action::app(Message::TaskUpdated(id_for_msg.clone(), prev, r)),
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
