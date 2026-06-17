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
async fn list_completed_page_then_loads_more() {
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

    // First completed page returns one task and the next link; no auto-follow.
    let (page1, next) = client.list_completed_page("L1").await.unwrap();
    assert_eq!(page1.len(), 1);
    assert_eq!(page1[0].id, "T1");
    assert_eq!(next.as_deref(), Some(page2.as_str()));

    // "Load more" follows the next link to the second page.
    let (page2_tasks, next2) = client.list_tasks_page(&next.unwrap()).await.unwrap();
    assert_eq!(page2_tasks.len(), 1);
    assert_eq!(page2_tasks[0].title, "Second");
    assert!(next2.is_none());
}

#[tokio::test]
async fn list_completed_drops_foreign_origin_next_link() {
    // A forged nextLink pointing off the Graph origin must be dropped so the
    // bearer token is never sent there.
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/me/todo/lists/L1/tasks"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "value": [ { "id": "T1", "title": "First", "status": "notStarted" } ],
            "@odata.nextLink": "https://evil.example.com/me/todo/lists/L1/tasks-page2"
        })))
        .mount(&server)
        .await;
    let client = GraphClient::new(
        server.uri(),
        reqwest::Client::new(),
        std::sync::Arc::new(common::StaticTokenProvider("test-token".to_string())),
    );
    let (tasks, next) = client.list_completed_page("L1").await.unwrap();
    assert_eq!(tasks.len(), 1);
    assert!(next.is_none(), "foreign-origin nextLink must be dropped");
}

use outlook_tasks_core::models::TaskStatus;
use wiremock::matchers::body_json;

#[tokio::test]
async fn create_task_posts_input_body() {
    use outlook_tasks_core::models::TaskInput;
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/me/todo/lists/L1/tasks"))
        .and(body_json(json!({ "title": "Buy milk", "importance": "normal" })))
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
    let input = TaskInput { title: "Buy milk".into(), ..Default::default() };
    let task = client.create_task("L1", &input).await.unwrap();
    assert_eq!(task.id, "T9");
}

#[tokio::test]
async fn update_task_patches_input_body() {
    use outlook_tasks_core::models::TaskInput;
    let server = MockServer::start().await;
    Mock::given(method("PATCH"))
        .and(path("/me/todo/lists/L1/tasks/T9"))
        .and(body_json(json!({
            "title": "Renamed", "importance": "high",
            "dueDateTime": null, "recurrence": null,
            "isReminderOn": false, "reminderDateTime": null
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "T9", "title": "Renamed", "status": "notStarted", "importance": "high"
        })))
        .expect(1)
        .mount(&server)
        .await;
    let client = GraphClient::new(
        server.uri(),
        reqwest::Client::new(),
        std::sync::Arc::new(common::StaticTokenProvider("test-token".to_string())),
    );
    let input = TaskInput {
        title: "Renamed".into(),
        importance: outlook_tasks_core::models::Importance::High,
        ..Default::default()
    };
    let task = client.update_task("L1", "T9", &input).await.unwrap();
    assert_eq!(task.title, "Renamed");
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

struct NoSelect;
impl wiremock::Match for NoSelect {
    fn matches(&self, request: &wiremock::Request) -> bool {
        !request.url.query_pairs().any(|(k, _)| k == "$select")
    }
}

#[tokio::test]
async fn list_completed_filters_orders_and_omits_select() {
    // Completed is filtered + ordered by due date (newest first), never `$select`,
    // which To Do rejects with 400 RequestBroker--ParseUri.
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/me/todo/lists/L1/tasks"))
        .and(query_param("$filter", "status eq 'completed'"))
        .and(query_param("$orderby", "dueDateTime/dateTime desc"))
        .and(NoSelect)
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "value": [] })))
        .expect(1)
        .mount(&server)
        .await;
    let client = GraphClient::new(
        server.uri(),
        reqwest::Client::new(),
        std::sync::Arc::new(common::StaticTokenProvider("test-token".to_string())),
    );
    let (tasks, _next) = client.list_completed_page("L1").await.unwrap();
    assert!(tasks.is_empty());
}

#[tokio::test]
async fn list_pending_filters_server_side() {
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
    let tasks = client.list_pending("L1").await.unwrap();
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

#[tokio::test]
async fn list_tasks_page_rejects_foreign_next_link() {
    // A next link pointing off the Graph origin must be refused before any
    // request is made, so the bearer token is never sent to a forged host.
    let server = MockServer::start().await;
    let client = GraphClient::new(
        server.uri(),
        reqwest::Client::new(),
        std::sync::Arc::new(common::StaticTokenProvider("t".to_string())),
    );
    let err = client
        .list_tasks_page("http://evil.example/me/todo/lists/L1/tasks?$skiptoken=x")
        .await
        .unwrap_err();
    // Decode = early origin rejection; a Network error would mean a request went out.
    assert!(matches!(err, outlook_tasks_core::GraphError::Decode(_)));
    assert_eq!(server.received_requests().await.unwrap().len(), 0);
}
