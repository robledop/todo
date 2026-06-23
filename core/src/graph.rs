use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use reqwest::{Method, StatusCode};
use serde::de::DeserializeOwned;

use crate::error::{AuthError, GraphError};
use crate::models::{
    GraphCollection, PatternedRecurrence, RecurrencePatternType, TaskInput, TaskStatus, TodoList,
    TodoTask, UpdateTaskStatus, UserProfile,
};

/// Supplies bearer access tokens to the Graph client and refreshes them on demand.
#[async_trait]
pub trait TokenProvider: Send + Sync {
    async fn access_token(&self) -> Result<String, AuthError>;
    async fn force_refresh(&self) -> Result<String, AuthError>;
}

/// Thin typed client over `/me/todo`. `base_url` is the API root with no trailing
/// slash, e.g. `https://graph.microsoft.com/v1.0` (tests pass the mock server URI).
pub struct GraphClient {
    base_url: String,
    http: reqwest::Client,
    tokens: Arc<dyn TokenProvider>,
}

impl GraphClient {
    pub fn new(
        base_url: impl Into<String>,
        http: reqwest::Client,
        tokens: Arc<dyn TokenProvider>,
    ) -> Self {
        Self { base_url: base_url.into(), http, tokens }
    }

    pub async fn list_lists(&self) -> Result<Vec<TodoList>, GraphError> {
        self.collect_pages(format!("{}/me/todo/lists", self.base_url)).await
    }

    /// Fetches ALL pending (not-completed) tasks (`$filter=status ne 'completed'`),
    /// following pages. Pending is the always-visible list, so it is returned
    /// complete; it is small relative to completed history. No `$select`: the To Do
    /// tasks endpoint rejects it with 400 RequestBroker--ParseUri.
    pub async fn list_pending(&self, list_id: &str) -> Result<Vec<TodoTask>, GraphError> {
        let url = format!(
            "{}/me/todo/lists/{}/tasks?$filter=status%20ne%20%27completed%27",
            self.base_url,
            seg(list_id),
        );
        self.collect_pages(url).await
    }

    /// Fetches the first page of completed tasks, newest-due first
    /// (`$filter=status eq 'completed'` + `$orderby=dueDateTime/dateTime desc`),
    /// plus a validated `@odata.nextLink` for "load more". Completed history can be
    /// large, so it is paged rather than fetched all at once.
    pub async fn list_completed_page(
        &self,
        list_id: &str,
    ) -> Result<(Vec<TodoTask>, Option<String>), GraphError> {
        let url = format!(
            "{}/me/todo/lists/{}/tasks?$filter=status%20eq%20%27completed%27&$orderby=dueDateTime/dateTime%20desc",
            self.base_url,
            seg(list_id),
        );
        self.fetch_tasks_page(url).await
    }

    /// Fetches a subsequent tasks page from an `@odata.nextLink` produced by
    /// `list_completed_page`. Re-validates the origin before sending the bearer
    /// token: callers pass server-returned links today, but this is a public API
    /// and must never carry the token to a host off the Graph origin.
    pub async fn list_tasks_page(
        &self,
        next_link: &str,
    ) -> Result<(Vec<TodoTask>, Option<String>), GraphError> {
        if !same_graph_origin(&self.base_url, next_link) {
            return Err(GraphError::Decode("unexpected nextLink origin".into()));
        }
        self.fetch_tasks_page(next_link.to_string()).await
    }

    /// Creates a task in a list from the given input.
    pub async fn create_task(&self, list_id: &str, input: &TaskInput) -> Result<TodoTask, GraphError> {
        let url = format!("{}/me/todo/lists/{}/tasks", self.base_url, seg(list_id));
        let mut body = input.to_body(false);
        let relative = take_relative(input, &mut body);
        let resp = self.execute(Method::POST, &url, Some(body)).await?;
        let task = resp.json::<TodoTask>().await.map_err(|e| GraphError::Decode(e.to_string()))?;
        self.finish_with_recurrence(list_id, task, relative, true).await
    }

    /// Updates a task's editable fields (PATCH).
    pub async fn update_task(
        &self,
        list_id: &str,
        task_id: &str,
        input: &TaskInput,
    ) -> Result<TodoTask, GraphError> {
        let url =
            format!("{}/me/todo/lists/{}/tasks/{}", self.base_url, seg(list_id), seg(task_id));
        let mut body = input.to_body(true);
        let relative = take_relative(input, &mut body);
        let resp = self.execute(Method::PATCH, &url, Some(body)).await?;
        let task = resp.json::<TodoTask>().await.map_err(|e| GraphError::Decode(e.to_string()))?;
        self.finish_with_recurrence(list_id, task, relative, false).await
    }

    /// GETs a single task by id.
    pub async fn get_task(&self, list_id: &str, task_id: &str) -> Result<TodoTask, GraphError> {
        let url =
            format!("{}/me/todo/lists/{}/tasks/{}", self.base_url, seg(list_id), seg(task_id));
        self.get_json(&url).await
    }

    /// GETs the signed-in user's profile (`/me`). Requires the `User.Read` scope;
    /// a token granted without it gets 403 `GraphError::Forbidden` (not retried),
    /// which the caller renders as an "account unavailable" hint rather than
    /// re-authenticating.
    pub async fn get_me(&self) -> Result<UserProfile, GraphError> {
        self.get_json(&format!("{}/me", self.base_url)).await
    }

    /// After a create/update, applies a relative recurrence (if any) via the
    /// Outlook endpoint - which the To Do endpoint can't store - then re-reads the
    /// task so the caller sees the recurrence (and the due date Graph adjusted to
    /// the next occurrence). Non-relative recurrences are already stored by the
    /// To Do write, so the task is returned as-is.
    async fn finish_with_recurrence(
        &self,
        list_id: &str,
        task: TodoTask,
        relative: Option<PatternedRecurrence>,
        created: bool,
    ) -> Result<TodoTask, GraphError> {
        let Some(rec) = relative else { return Ok(task) };
        // The To Do write left any existing recurrence intact; if it already
        // matches what we want (an edit that didn't change the schedule), there's
        // nothing to write via Outlook - so a beta outage can't break that edit.
        if relative_recurrence_matches(task.recurrence.as_ref(), &rec) {
            return Ok(task);
        }
        match self.set_outlook_recurrence(&task.id, &rec).await {
            Ok(()) => self.get_task(list_id, &task.id).await,
            Err(e) => {
                log::warn!("Outlook recurrence write failed, falling back: {e}");
                // Roll back a just-created task so a retry can't duplicate it; an
                // update keeps its other field changes. Either way the caller gets
                // a clear, user-facing error instead of a silent daily downgrade.
                if created {
                    let _ = self.delete_task(list_id, &task.id).await;
                }
                Err(GraphError::RecurrenceUnavailable)
            }
        }
    }

    /// Applies a recurrence to a task via the Outlook task endpoint (beta), which,
    /// unlike the To Do endpoint, preserves relative ("Nth weekday") patterns. The
    /// task id is shared between the two endpoints; Graph adjusts the task's due
    /// date to the next occurrence of the pattern.
    async fn set_outlook_recurrence(
        &self,
        task_id: &str,
        recurrence: &PatternedRecurrence,
    ) -> Result<(), GraphError> {
        let url = format!("{}/me/outlook/tasks/{}", self.beta_base(), seg(task_id));
        let recurrence =
            serde_json::to_value(recurrence).map_err(|e| GraphError::Decode(e.to_string()))?;
        let body = serde_json::json!({ "recurrence": recurrence });
        self.execute(Method::PATCH, &url, Some(body)).await?;
        Ok(())
    }

    /// The beta API root, derived from `base_url` (`.../v1.0` -> `.../beta`).
    fn beta_base(&self) -> String {
        self.base_url.replace("/v1.0", "/beta")
    }

    /// Updates a task's status (e.g. `Completed`).
    pub async fn set_status(
        &self,
        list_id: &str,
        task_id: &str,
        status: TaskStatus,
    ) -> Result<TodoTask, GraphError> {
        let url =
            format!("{}/me/todo/lists/{}/tasks/{}", self.base_url, seg(list_id), seg(task_id));
        let body = serde_json::to_value(UpdateTaskStatus { status })
            .map_err(|e| GraphError::Decode(e.to_string()))?;
        let resp = self.execute(Method::PATCH, &url, Some(body)).await?;
        resp.json::<TodoTask>().await.map_err(|e| GraphError::Decode(e.to_string()))
    }

    /// Deletes a task. Expects HTTP 204; non-2xx maps through `GraphError`.
    pub async fn delete_task(&self, list_id: &str, task_id: &str) -> Result<(), GraphError> {
        let url =
            format!("{}/me/todo/lists/{}/tasks/{}", self.base_url, seg(list_id), seg(task_id));
        self.execute(Method::DELETE, &url, None).await?;
        Ok(())
    }

    /// GETs a single tasks page: its items plus an origin-validated
    /// `@odata.nextLink` (dropped if it points off the Graph origin, so the bearer
    /// token is never sent to a forged host).
    async fn fetch_tasks_page(
        &self,
        url: String,
    ) -> Result<(Vec<TodoTask>, Option<String>), GraphError> {
        let page: GraphCollection<TodoTask> = self.get_json(&url).await?;
        let next = page.next_link.filter(|n| same_graph_origin(&self.base_url, n));
        Ok((page.value, next))
    }

    /// GETs every page of an OData collection, following `@odata.nextLink`.
    /// Refuses to follow a next link whose origin differs from `base_url` so a
    /// bearer token is never sent to an unexpected host.
    async fn collect_pages<T: DeserializeOwned>(
        &self,
        first_url: String,
    ) -> Result<Vec<T>, GraphError> {
        let mut url = first_url;
        let mut out = Vec::new();
        loop {
            let page: GraphCollection<T> = self.get_json(&url).await?;
            out.extend(page.value);
            match page.next_link {
                Some(next) if same_graph_origin(&self.base_url, &next) => url = next,
                Some(_) => return Err(GraphError::Decode("unexpected nextLink origin".into())),
                None => break,
            }
        }
        Ok(out)
    }

    /// Sends an authenticated GET to an absolute URL, decoding the JSON body.
    /// Retries once after a forced token refresh on a 401.
    async fn get_json<T: DeserializeOwned>(&self, url: &str) -> Result<T, GraphError> {
        let resp = self.execute(Method::GET, url, None).await?;
        resp.json::<T>().await.map_err(|e| GraphError::Decode(e.to_string()))
    }

    /// Runs a request with bearer auth; on 401 forces a refresh and retries once;
    /// maps non-success statuses to precise `GraphError`s.
    async fn execute(
        &self,
        method: Method,
        url: &str,
        body: Option<serde_json::Value>,
    ) -> Result<reqwest::Response, GraphError> {
        let token = self
            .tokens
            .access_token()
            .await
            .map_err(|e| GraphError::Token(e.to_string()))?;

        let resp = self.send_once(&method, url, &body, &token).await?;
        match map_status(resp).await {
            Err(GraphError::Unauthorized) => {
                let token = self
                    .tokens
                    .force_refresh()
                    .await
                    .map_err(|e| GraphError::Token(e.to_string()))?;
                let resp = self.send_once(&method, url, &body, &token).await?;
                map_status(resp).await
            }
            other => other,
        }
    }

    async fn send_once(
        &self,
        method: &Method,
        url: &str,
        body: &Option<serde_json::Value>,
        token: &str,
    ) -> Result<reqwest::Response, GraphError> {
        let mut req = self.http.request(method.clone(), url).bearer_auth(token);
        if let Some(b) = body {
            req = req.json(b);
        }
        req.send().await.map_err(|e| GraphError::Network(e.to_string()))
    }
}

/// True only if `candidate` shares `base`'s scheme, host, port, and path prefix.
/// Guards the bearer token against a forged `@odata.nextLink` pointing at a
/// look-alike host (a raw string-prefix check would accept `graph.microsoft.com.evil`).
fn same_graph_origin(base: &str, candidate: &str) -> bool {
    let (Ok(base), Ok(next)) = (url::Url::parse(base), url::Url::parse(candidate)) else {
        return false;
    };
    next.scheme() == base.scheme()
        && next.host_str() == base.host_str()
        && next.port_or_known_default() == base.port_or_known_default()
        && path_within(base.path(), next.path())
}

/// True if `next_path` is `base_path` or a path nested under it. Unlike a raw
/// `starts_with`, this requires a path boundary so a sibling such as `/v1.0foo`
/// does not pass for a base of `/v1.0`.
fn path_within(base_path: &str, next_path: &str) -> bool {
    if next_path == base_path {
        return true;
    }
    match next_path.strip_prefix(base_path) {
        Some(rest) => base_path.ends_with('/') || rest.starts_with('/'),
        None => false,
    }
}

/// True if `current` already holds the same relative pattern as `desired`,
/// comparing only the fields meaningful to the pattern type (the server fills the
/// others with zero/defaults that the form leaves unset). Lets an edit that
/// didn't change the schedule skip the Outlook write entirely.
fn relative_recurrence_matches(
    current: Option<&PatternedRecurrence>,
    desired: &PatternedRecurrence,
) -> bool {
    let Some(cur) = current else { return false };
    let (c, d) = (&cur.pattern, &desired.pattern);
    if c.pattern_type != d.pattern_type {
        return false;
    }
    let mut cur_days: Vec<&str> = c.days_of_week.iter().map(String::as_str).collect();
    let mut want_days: Vec<&str> = d.days_of_week.iter().map(String::as_str).collect();
    cur_days.sort_unstable();
    want_days.sort_unstable();
    // `month` only matters for relativeYearly; relativeMonthly leaves it unset
    // while the server echoes 0.
    let month_matches =
        d.pattern_type != RecurrencePatternType::RelativeYearly || c.month == d.month;
    c.interval == d.interval && c.index == d.index && cur_days == want_days && month_matches
}

/// If `input`'s recurrence is a relative ("Nth weekday") pattern, removes it from
/// the To Do request `body` - so the write leaves any existing recurrence intact -
/// and returns it to be applied via the Outlook endpoint instead.
fn take_relative(input: &TaskInput, body: &mut serde_json::Value) -> Option<PatternedRecurrence> {
    let rec = input.recurrence.as_ref().filter(|r| r.pattern.pattern_type.is_relative())?;
    if let Some(obj) = body.as_object_mut() {
        obj.remove("recurrence");
    }
    Some(rec.clone())
}

/// Percent-encodes an opaque Graph id for safe use as a single URL path segment.
fn seg(id: &str) -> String {
    use percent_encoding::{utf8_percent_encode, AsciiSet, CONTROLS};
    // Encode only characters that would corrupt a single URL path segment, while
    // leaving the base64url-ish id alphabet (including '-', '_', '=') literal:
    // Graph's To Do endpoint rejects over-encoded ids with 400 RequestBroker--ParseUri.
    const SEGMENT: &AsciiSet = &CONTROLS
        .add(b' ')
        .add(b'"')
        .add(b'#')
        .add(b'%')
        .add(b'/')
        .add(b'<')
        .add(b'>')
        .add(b'?')
        .add(b'`')
        .add(b'{')
        .add(b'}')
        .add(b'\\')
        .add(b'^')
        .add(b'|')
        .add(b'[')
        .add(b']');
    utf8_percent_encode(id, SEGMENT).to_string()
}

/// Maps a response's status to `Ok(resp)` for 2xx, or a precise `GraphError`.
async fn map_status(resp: reqwest::Response) -> Result<reqwest::Response, GraphError> {
    let status = resp.status();
    if status.is_success() {
        return Ok(resp);
    }
    match status {
        StatusCode::UNAUTHORIZED => Err(GraphError::Unauthorized),
        StatusCode::FORBIDDEN => Err(GraphError::Forbidden),
        StatusCode::TOO_MANY_REQUESTS => {
            let retry_after = resp
                .headers()
                .get(reqwest::header::RETRY_AFTER)
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.parse::<u64>().ok())
                .map(Duration::from_secs);
            Err(GraphError::Throttled { retry_after })
        }
        other => {
            let mut body = resp.text().await.unwrap_or_default();
            bound_error_body(&mut body);
            Err(GraphError::Http { status: other.as_u16(), body })
        }
    }
}

/// Caps a Graph error body so an unexpectedly large or junk response can't bloat
/// logs or the UI. Graph error payloads are small JSON objects, well under this.
fn bound_error_body(body: &mut String) {
    const MAX_BODY: usize = 2048;
    if body.len() <= MAX_BODY {
        return;
    }
    let mut end = MAX_BODY;
    while end > 0 && !body.is_char_boundary(end) {
        end -= 1;
    }
    body.truncate(end);
    body.push_str("...(truncated)");
}

#[cfg(test)]
mod tests {
    use super::{same_graph_origin, seg};

    #[test]
    fn same_origin_requires_a_path_boundary() {
        let base = "https://graph.microsoft.com/v1.0";
        assert!(same_graph_origin(base, "https://graph.microsoft.com/v1.0/me/todo/lists"));
        assert!(same_graph_origin(base, "https://graph.microsoft.com/v1.0"));
        // A sibling that only shares the prefix string must be rejected.
        assert!(!same_graph_origin(base, "https://graph.microsoft.com/v1.0foo/evil"));
        // Different host / scheme are rejected too.
        assert!(!same_graph_origin(base, "https://evil.example/v1.0/me"));
        assert!(!same_graph_origin(base, "http://graph.microsoft.com/v1.0/me"));
    }

    #[test]
    fn seg_preserves_base64url_id_characters() {
        // Microsoft To Do list/task ids are base64url-ish with '=' padding and
        // must reach Graph literally; over-encoding '-', '_', or '=' triggers a
        // 400 invalidRequest / RequestBroker--ParseUri.
        assert_eq!(seg("AAMkAGI2THk-_AAA="), "AAMkAGI2THk-_AAA=");
    }

    #[test]
    fn seg_encodes_path_breaking_characters() {
        // Genuinely unsafe path characters must still be encoded so an id can't
        // escape its path segment.
        assert_eq!(seg("a/b?c#d e"), "a%2Fb%3Fc%23d%20e");
    }
}
