mod common;

use std::sync::Arc;
use common::StaticTokenProvider;
use outlook_tasks_core::graph::GraphClient;
use serde_json::json;
use wiremock::matchers::{header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn list_lists_sends_bearer_and_parses() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/me/todo/lists"))
        .and(header("Authorization", "Bearer test-token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "value": [
                { "id": "L1", "displayName": "Tasks", "wellknownListName": "defaultList" },
                { "id": "L2", "displayName": "Groceries" }
            ]
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = GraphClient::new(
        server.uri(),
        reqwest::Client::new(),
        Arc::new(StaticTokenProvider("test-token".to_string())),
    );

    let lists = client.list_lists().await.unwrap();
    assert_eq!(lists.len(), 2);
    assert_eq!(lists[0].id, "L1");
    assert_eq!(lists[1].display_name, "Groceries");
}

#[tokio::test]
async fn rejects_next_link_to_foreign_origin() {
    let server = MockServer::start().await;
    // The first page points its nextLink at a DIFFERENT host; the client must
    // refuse to follow it (and never send the bearer token there).
    Mock::given(method("GET"))
        .and(path("/me/todo/lists"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "value": [ { "id": "L1", "displayName": "Tasks" } ],
            "@odata.nextLink": "http://evil.example/me/todo/lists?$skiptoken=x"
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = GraphClient::new(
        server.uri(),
        reqwest::Client::new(),
        Arc::new(StaticTokenProvider("test-token".to_string())),
    );

    let err = client.list_lists().await.unwrap_err();
    assert!(matches!(err, outlook_tasks_core::GraphError::Decode(_)));
}

#[tokio::test]
async fn list_tasks_follows_next_link() {
    let server = MockServer::start().await;
    let page2 = format!("{}/me/todo/lists/L1/tasks-page2", server.uri());

    Mock::given(method("GET"))
        .and(path("/me/todo/lists/L1/tasks"))
        .and(header("Authorization", "Bearer test-token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "value": [ { "id": "T1", "title": "First", "status": "notStarted" } ],
            "@odata.nextLink": page2
        })))
        .expect(1)
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/me/todo/lists/L1/tasks-page2"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "value": [ { "id": "T2", "title": "Second", "status": "completed" } ]
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = GraphClient::new(
        server.uri(),
        reqwest::Client::new(),
        std::sync::Arc::new(common::StaticTokenProvider("test-token".to_string())),
    );

    let tasks = client.list_tasks("L1", true).await.unwrap();
    assert_eq!(tasks.len(), 2);
    assert_eq!(tasks[0].id, "T1");
    assert_eq!(tasks[1].title, "Second");
}

use outlook_tasks_core::models::TaskStatus;
use wiremock::matchers::body_json;

#[tokio::test]
async fn create_task_posts_title_and_parses_result() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/me/todo/lists/L1/tasks"))
        .and(header("Authorization", "Bearer test-token"))
        .and(body_json(json!({ "title": "Buy milk" })))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "id": "T9", "title": "Buy milk", "status": "notStarted"
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = GraphClient::new(
        server.uri(),
        reqwest::Client::new(),
        std::sync::Arc::new(common::StaticTokenProvider("test-token".to_string())),
    );

    let task = client.create_task("L1", "Buy milk").await.unwrap();
    assert_eq!(task.id, "T9");
    assert_eq!(task.status, TaskStatus::NotStarted);
}

#[tokio::test]
async fn set_status_patches_status() {
    let server = MockServer::start().await;
    Mock::given(method("PATCH"))
        .and(path("/me/todo/lists/L1/tasks/T9"))
        .and(header("Authorization", "Bearer test-token"))
        .and(body_json(json!({ "status": "completed" })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "T9", "title": "Buy milk", "status": "completed"
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = GraphClient::new(
        server.uri(),
        reqwest::Client::new(),
        std::sync::Arc::new(common::StaticTokenProvider("test-token".to_string())),
    );

    let task = client.set_status("L1", "T9", TaskStatus::Completed).await.unwrap();
    assert_eq!(task.status, TaskStatus::Completed);
}

use common::ScriptedProvider;
use outlook_tasks_core::GraphError;
use std::time::Duration;

#[tokio::test]
async fn retries_once_after_401_with_refreshed_token() {
    let server = MockServer::start().await;
    // Stale token -> 401.
    Mock::given(method("GET"))
        .and(path("/me/todo/lists"))
        .and(header("Authorization", "Bearer stale"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;
    // Fresh token -> 200.
    Mock::given(method("GET"))
        .and(path("/me/todo/lists"))
        .and(header("Authorization", "Bearer fresh"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "value": [] })))
        .mount(&server)
        .await;

    let provider = std::sync::Arc::new(ScriptedProvider::new("stale", "fresh"));
    let client = GraphClient::new(server.uri(), reqwest::Client::new(), provider.clone());

    let lists = client.list_lists().await.unwrap();
    assert!(lists.is_empty());
    assert_eq!(provider.refresh_count(), 1, "must force-refresh exactly once");
}

#[tokio::test]
async fn forbidden_is_not_retried() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/me/todo/lists"))
        .respond_with(ResponseTemplate::new(403))
        .expect(1) // exactly one request - no retry
        .mount(&server)
        .await;

    let provider = std::sync::Arc::new(ScriptedProvider::new("t", "t2"));
    let client = GraphClient::new(server.uri(), reqwest::Client::new(), provider.clone());

    let err = client.list_lists().await.unwrap_err();
    assert!(matches!(err, GraphError::Forbidden));
    assert_eq!(provider.refresh_count(), 0, "403 must not trigger refresh");
}

#[tokio::test]
async fn throttled_reads_retry_after() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/me/todo/lists"))
        .respond_with(ResponseTemplate::new(429).append_header("Retry-After", "30"))
        .mount(&server)
        .await;

    let client = GraphClient::new(
        server.uri(),
        reqwest::Client::new(),
        std::sync::Arc::new(common::StaticTokenProvider("t".to_string())),
    );

    let err = client.list_lists().await.unwrap_err();
    assert!(matches!(err, GraphError::Throttled { retry_after: Some(d) } if d == Duration::from_secs(30)));
}

struct NoQuery;
impl wiremock::Match for NoQuery {
    fn matches(&self, request: &wiremock::Request) -> bool {
        request.url.query().is_none()
    }
}

#[tokio::test]
async fn list_tasks_omits_select_query() {
    // Microsoft To Do rejects `$select` on the tasks endpoint with
    // 400 RequestBroker--ParseUri, so the client must request bare tasks.
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/me/todo/lists/L1/tasks"))
        .and(NoQuery)
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "value": [] })))
        .expect(1)
        .mount(&server)
        .await;
    let client = GraphClient::new(
        server.uri(),
        reqwest::Client::new(),
        std::sync::Arc::new(common::StaticTokenProvider("test-token".to_string())),
    );
    let tasks = client.list_tasks("L1", true).await.unwrap();
    assert!(tasks.is_empty());
}

#[tokio::test]
async fn list_tasks_pending_only_filters_server_side() {
    // Without completed, the client asks the server to filter, which is much
    // faster than paging every completed task. To Do supports this $filter.
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/me/todo/lists/L1/tasks"))
        .and(query_param("$filter", "status ne 'completed'"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "value": [ { "id": "T1", "title": "Buy milk", "status": "notStarted" } ]
        })))
        .expect(1)
        .mount(&server)
        .await;
    let client = GraphClient::new(
        server.uri(),
        reqwest::Client::new(),
        std::sync::Arc::new(common::StaticTokenProvider("test-token".to_string())),
    );
    let tasks = client.list_tasks("L1", false).await.unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].id, "T1");
}

#[tokio::test]
async fn delete_task_sends_delete_and_succeeds_on_204() {
    let server = MockServer::start().await;
    Mock::given(method("DELETE"))
        .and(path("/me/todo/lists/L1/tasks/T9"))
        .and(header("Authorization", "Bearer test-token"))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;
    let client = GraphClient::new(
        server.uri(),
        reqwest::Client::new(),
        std::sync::Arc::new(common::StaticTokenProvider("test-token".to_string())),
    );
    client.delete_task("L1", "T9").await.unwrap();
}

#[tokio::test]
async fn delete_task_maps_404_to_error() {
    let server = MockServer::start().await;
    Mock::given(method("DELETE"))
        .and(path("/me/todo/lists/L1/tasks/GONE"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;
    let client = GraphClient::new(
        server.uri(),
        reqwest::Client::new(),
        std::sync::Arc::new(common::StaticTokenProvider("t".to_string())),
    );
    let err = client.delete_task("L1", "GONE").await.unwrap_err();
    assert!(matches!(err, outlook_tasks_core::GraphError::Http { status: 404, .. }));
}
