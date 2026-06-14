use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use reqwest::{Method, StatusCode};
use serde::de::DeserializeOwned;

use crate::error::{AuthError, GraphError};
use crate::models::{GraphCollection, TaskInput, TaskStatus, TodoList, TodoTask, UpdateTaskStatus};

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
    /// `list_completed_page` (already origin-checked when it was returned).
    pub async fn list_tasks_page(
        &self,
        next_link: &str,
    ) -> Result<(Vec<TodoTask>, Option<String>), GraphError> {
        self.fetch_tasks_page(next_link.to_string()).await
    }

    /// Creates a task in a list from the given input.
    pub async fn create_task(&self, list_id: &str, input: &TaskInput) -> Result<TodoTask, GraphError> {
        let url = format!("{}/me/todo/lists/{}/tasks", self.base_url, seg(list_id));
        let resp = self.execute(Method::POST, &url, Some(input.to_body(false))).await?;
        resp.json::<TodoTask>().await.map_err(|e| GraphError::Decode(e.to_string()))
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
        let resp = self.execute(Method::PATCH, &url, Some(input.to_body(true))).await?;
        resp.json::<TodoTask>().await.map_err(|e| GraphError::Decode(e.to_string()))
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
        && next.path().starts_with(base.path())
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
            let body = resp.text().await.unwrap_or_default();
            Err(GraphError::Http { status: other.as_u16(), body })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::seg;

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
