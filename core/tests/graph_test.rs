mod common;

use std::sync::Arc;
use common::StaticTokenProvider;
use outlook_tasks_core::graph::GraphClient;
use outlook_tasks_core::models::{
    PatternedRecurrence, RecurrencePattern, RecurrencePatternType, RecurrenceRange,
    RecurrenceRangeType, TaskInput,
};
use serde_json::json;
use wiremock::matchers::{body_partial_json, header, method, path, query_param};
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
            // No `body`: this edit never loaded a note, so the PATCH omits it
            // rather than risk clearing a server-side note (body is Option::None).
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
async fn create_task_with_note_sends_html_body() {
    use outlook_tasks_core::models::TaskInput;
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/me/todo/lists/L1/tasks"))
        // The To Do API only accepts html for `body`, so the form's escaped text
        // arrives here as html content.
        .and(body_partial_json(
            json!({ "body": { "content": "buy &lt;5", "contentType": "html" } }),
        ))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "id": "T9", "title": "x", "status": "notStarted"
        })))
        .expect(1)
        .mount(&server)
        .await;
    let client = GraphClient::new(
        server.uri(),
        reqwest::Client::new(),
        std::sync::Arc::new(common::StaticTokenProvider("test-token".to_string())),
    );
    let input = TaskInput { title: "x".into(), body: Some("buy &lt;5".into()), ..Default::default() };
    client.create_task("L1", &input).await.unwrap();
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

#[tokio::test]
async fn http_error_body_is_bounded() {
    // A pathologically large error body must be capped before it reaches logs/UI.
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/me/todo/lists"))
        .respond_with(ResponseTemplate::new(400).set_body_string("x".repeat(10_000)))
        .mount(&server)
        .await;
    let client = GraphClient::new(
        server.uri(),
        reqwest::Client::new(),
        std::sync::Arc::new(common::StaticTokenProvider("t".to_string())),
    );
    match client.list_lists().await.unwrap_err() {
        outlook_tasks_core::GraphError::Http { status, body } => {
            assert_eq!(status, 400);
            assert!(body.len() < 3000, "error body should be bounded, got {}", body.len());
        }
        other => panic!("expected Http error, got {other:?}"),
    }
}

/// Matches a request whose JSON body does NOT contain the given top-level key.
struct BodyLacks(&'static str);
impl wiremock::Match for BodyLacks {
    fn matches(&self, req: &wiremock::Request) -> bool {
        serde_json::from_slice::<serde_json::Value>(&req.body)
            .ok()
            .and_then(|v| v.as_object().map(|o| !o.contains_key(self.0)))
            .unwrap_or(true)
    }
}

fn relative_monthly_input() -> TaskInput {
    TaskInput {
        title: "Test".into(),
        recurrence: Some(PatternedRecurrence {
            pattern: RecurrencePattern {
                pattern_type: RecurrencePatternType::RelativeMonthly,
                interval: 1,
                month: None,
                day_of_month: None,
                days_of_week: vec!["monday".into()],
                first_day_of_week: None,
                index: Some("first".into()),
            },
            range: RecurrenceRange {
                range_type: RecurrenceRangeType::NoEnd,
                start_date: "2026-06-01".into(),
                end_date: None,
                number_of_occurrences: None,
                recurrence_time_zone: Some("UTC".into()),
            },
        }),
        ..Default::default()
    }
}

/// What the re-fetch returns once the Outlook endpoint has stored the pattern.
fn relative_task_body() -> serde_json::Value {
    json!({
        "id": "T1", "title": "Test", "status": "notStarted",
        "recurrence": {
            "pattern": {"type":"relativeMonthly","interval":1,"daysOfWeek":["monday"],"index":"first"},
            "range": {"type":"noEnd","startDate":"2026-07-06"}
        }
    })
}

#[tokio::test]
async fn create_task_routes_relative_recurrence_through_outlook() {
    let server = MockServer::start().await;
    // To Do create must NOT carry the relative recurrence (it would degrade to daily).
    Mock::given(method("POST"))
        .and(path("/me/todo/lists/L1/tasks"))
        .and(BodyLacks("recurrence"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({"id":"T1","title":"Test","status":"notStarted"})))
        .expect(1)
        .mount(&server)
        .await;
    // The relative pattern is applied via the Outlook (beta) endpoint, same id.
    Mock::given(method("PATCH"))
        .and(path("/me/outlook/tasks/T1"))
        .and(body_partial_json(json!({"recurrence":{"pattern":{"type":"relativeMonthly","index":"first"}}})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id":"T1"})))
        .expect(1)
        .mount(&server)
        .await;
    // Then the task is re-read via To Do and now carries the relative recurrence.
    Mock::given(method("GET"))
        .and(path("/me/todo/lists/L1/tasks/T1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(relative_task_body()))
        .expect(1)
        .mount(&server)
        .await;

    let client = GraphClient::new(
        server.uri(),
        reqwest::Client::new(),
        Arc::new(StaticTokenProvider("test-token".to_string())),
    );
    let task = client.create_task("L1", &relative_monthly_input()).await.unwrap();
    assert_eq!(task.id, "T1");
    assert_eq!(
        task.recurrence.unwrap().pattern.pattern_type,
        RecurrencePatternType::RelativeMonthly
    );
}

#[tokio::test]
async fn update_task_preserves_relative_recurrence_via_outlook() {
    let server = MockServer::start().await;
    // To Do update omits recurrence so it can't clobber/degrade the existing one.
    Mock::given(method("PATCH"))
        .and(path("/me/todo/lists/L1/tasks/T1"))
        .and(BodyLacks("recurrence"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id":"T1","title":"Test","status":"notStarted"})))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("PATCH"))
        .and(path("/me/outlook/tasks/T1"))
        .and(body_partial_json(json!({"recurrence":{"pattern":{"type":"relativeMonthly"}}})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id":"T1"})))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/me/todo/lists/L1/tasks/T1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(relative_task_body()))
        .expect(1)
        .mount(&server)
        .await;

    let client = GraphClient::new(
        server.uri(),
        reqwest::Client::new(),
        Arc::new(StaticTokenProvider("test-token".to_string())),
    );
    let task = client.update_task("L1", "T1", &relative_monthly_input()).await.unwrap();
    assert_eq!(
        task.recurrence.unwrap().pattern.pattern_type,
        RecurrencePatternType::RelativeMonthly
    );
}

#[tokio::test]
async fn create_task_rolls_back_when_outlook_unavailable() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/me/todo/lists/L1/tasks"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({"id":"T1","title":"Test","status":"notStarted"})))
        .expect(1)
        .mount(&server)
        .await;
    // The Outlook endpoint is unavailable.
    Mock::given(method("PATCH"))
        .and(path("/me/outlook/tasks/T1"))
        .respond_with(ResponseTemplate::new(503))
        .expect(1)
        .mount(&server)
        .await;
    // The just-created task is rolled back so a retry can't duplicate it.
    Mock::given(method("DELETE"))
        .and(path("/me/todo/lists/L1/tasks/T1"))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;

    let client = GraphClient::new(
        server.uri(),
        reqwest::Client::new(),
        Arc::new(StaticTokenProvider("test-token".to_string())),
    );
    let err = client.create_task("L1", &relative_monthly_input()).await.unwrap_err();
    assert!(matches!(err, outlook_tasks_core::GraphError::RecurrenceUnavailable));
}

#[tokio::test]
async fn update_task_reports_when_outlook_unavailable() {
    let server = MockServer::start().await;
    Mock::given(method("PATCH"))
        .and(path("/me/todo/lists/L1/tasks/T1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id":"T1","title":"Test","status":"notStarted"})))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("PATCH"))
        .and(path("/me/outlook/tasks/T1"))
        .respond_with(ResponseTemplate::new(503))
        .expect(1)
        .mount(&server)
        .await;
    // No DELETE: an update keeps its other field changes (no rollback).
    Mock::given(method("DELETE"))
        .and(path("/me/todo/lists/L1/tasks/T1"))
        .respond_with(ResponseTemplate::new(204))
        .expect(0)
        .mount(&server)
        .await;

    let client = GraphClient::new(
        server.uri(),
        reqwest::Client::new(),
        Arc::new(StaticTokenProvider("test-token".to_string())),
    );
    let err = client.update_task("L1", "T1", &relative_monthly_input()).await.unwrap_err();
    assert!(matches!(err, outlook_tasks_core::GraphError::RecurrenceUnavailable));
}

#[tokio::test]
async fn update_task_skips_outlook_when_recurrence_unchanged() {
    let server = MockServer::start().await;
    // The To Do update returns the task already carrying the same relative pattern.
    Mock::given(method("PATCH"))
        .and(path("/me/todo/lists/L1/tasks/T1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(relative_task_body()))
        .expect(1)
        .mount(&server)
        .await;
    // So the Outlook endpoint must NOT be touched (an outage can't break this edit).
    Mock::given(method("PATCH"))
        .and(path("/me/outlook/tasks/T1"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&server)
        .await;

    let client = GraphClient::new(
        server.uri(),
        reqwest::Client::new(),
        Arc::new(StaticTokenProvider("test-token".to_string())),
    );
    let task = client.update_task("L1", "T1", &relative_monthly_input()).await.unwrap();
    assert_eq!(
        task.recurrence.unwrap().pattern.pattern_type,
        RecurrencePatternType::RelativeMonthly
    );
}

#[tokio::test]
async fn get_me_sends_bearer_and_parses() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/me"))
        .and(header("Authorization", "Bearer test-token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "displayName": "Jane Doe",
            "userPrincipalName": "jane@outlook.com",
            "mail": "jane@outlook.com"
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = GraphClient::new(
        server.uri(),
        reqwest::Client::new(),
        Arc::new(StaticTokenProvider("test-token".to_string())),
    );

    let me = client.get_me().await.unwrap();
    assert_eq!(me.name(), Some("Jane Doe"));
    assert_eq!(me.email(), Some("jane@outlook.com"));
}
