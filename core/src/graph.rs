use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use reqwest::{Method, StatusCode};
use serde::de::DeserializeOwned;

use crate::error::{AuthError, GraphError};
#[allow(unused_imports)]
use crate::models::{CreateTask, GraphCollection, TaskStatus, TodoList, TodoTask, UpdateTaskStatus};

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
    use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
    utf8_percent_encode(id, NON_ALPHANUMERIC).to_string()
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
