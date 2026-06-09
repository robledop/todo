mod common;

use std::sync::Arc;
use common::StaticTokenProvider;
use outlook_tasks_core::graph::GraphClient;
use serde_json::json;
use wiremock::matchers::{header, method, path};
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

    let tasks = client.list_tasks("L1").await.unwrap();
    assert_eq!(tasks.len(), 2);
    assert_eq!(tasks[0].id, "T1");
    assert_eq!(tasks[1].title, "Second");
}
