use std::sync::Arc;

use outlook_tasks_core::auth::{
    AuthConfig, Authenticator, BootstrapOutcome, InMemoryTokenStore, OAuthClient, StoredToken,
    TokenStore,
};
use outlook_tasks_core::graph::TokenProvider;
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn oauth_for(server: &MockServer) -> OAuthClient {
    OAuthClient::new(&AuthConfig {
        client_id: "cid".into(),
        auth_url: format!("{}/authorize", server.uri()),
        token_url: format!("{}/token", server.uri()),
        redirect_url: "http://localhost:1/".into(),
        scopes: vec!["Tasks.ReadWrite".into()],
    })
    .unwrap()
}

#[tokio::test]
async fn bootstrap_refreshes_and_persists_rotation() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token": "at-new", "token_type": "Bearer",
            "expires_in": 3600, "refresh_token": "rt-new"
        })))
        .expect(1)
        .mount(&server)
        .await;

    let store = Arc::new(InMemoryTokenStore::default());
    store
        .save(&StoredToken { refresh_token: "rt-old".into(), account_id: "primary".into() })
        .await
        .unwrap();

    let auth = Authenticator::new(oauth_for(&server), store.clone(), "primary");

    let outcome = auth.bootstrap().await;
    assert!(matches!(outcome, BootstrapOutcome::Ready));

    // Rotated refresh token was persisted.
    assert_eq!(store.load().await.unwrap().unwrap().refresh_token, "rt-new");

    // Access token is now cached: a second read does not hit the token endpoint
    // (mock .expect(1) would fail on drop if it were called again).
    assert_eq!(auth.access_token().await.unwrap(), "at-new");
}

#[tokio::test]
async fn bootstrap_without_token_is_signed_out() {
    let server = MockServer::start().await;
    let store = Arc::new(InMemoryTokenStore::default());
    let auth = Authenticator::new(oauth_for(&server), store, "primary");
    assert!(matches!(auth.bootstrap().await, BootstrapOutcome::SignedOut));
}

#[tokio::test]
async fn concurrent_access_refreshes_only_once() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token": "at", "token_type": "Bearer",
            "expires_in": 3600, "refresh_token": "rt2"
        })))
        .expect(1) // single-flight: one network refresh despite two concurrent callers
        .mount(&server)
        .await;

    let store = Arc::new(InMemoryTokenStore::default());
    store
        .save(&StoredToken { refresh_token: "rt1".into(), account_id: "primary".into() })
        .await
        .unwrap();
    let auth = Arc::new(Authenticator::new(oauth_for(&server), store, "primary"));

    let (a, b) = tokio::join!(auth.access_token(), auth.access_token());
    assert_eq!(a.unwrap(), "at");
    assert_eq!(b.unwrap(), "at");
}
