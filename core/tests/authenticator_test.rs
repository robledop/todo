use std::sync::Arc;

use async_trait::async_trait;
use outlook_tasks_core::auth::{
    AuthConfig, Authenticator, BootstrapOutcome, InMemoryTokenStore, OAuthClient, StoredToken,
    TokenStore,
};
use outlook_tasks_core::graph::TokenProvider;
use outlook_tasks_core::KeyringError;
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

/// A store whose `clear()` always fails, to exercise the sign-out failure path.
struct ClearFailsStore(InMemoryTokenStore);

#[async_trait]
impl TokenStore for ClearFailsStore {
    async fn load(&self) -> Result<Option<StoredToken>, KeyringError> {
        self.0.load().await
    }
    async fn save(&self, token: &StoredToken) -> Result<(), KeyringError> {
        self.0.save(token).await
    }
    async fn clear(&self) -> Result<(), KeyringError> {
        Err(KeyringError::Other("keyring unavailable".into()))
    }
}

#[tokio::test]
async fn sign_out_clears_store_and_cache() {
    let server = MockServer::start().await;
    // The token endpoint must NOT be hit after sign-out: no refresh token remains.
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token": "at", "token_type": "Bearer",
            "expires_in": 3600, "refresh_token": "rt2"
        })))
        .expect(0)
        .mount(&server)
        .await;

    let store = Arc::new(InMemoryTokenStore::default());
    store
        .save(&StoredToken { refresh_token: "rt1".into(), account_id: "primary".into() })
        .await
        .unwrap();
    let auth = Authenticator::new(oauth_for(&server), store.clone(), "primary");

    auth.sign_out().await.unwrap();

    // Store emptied, bootstrap now signed-out, and no cached access token remains
    // (the next access_token finds no refresh token rather than hitting /token).
    assert!(store.load().await.unwrap().is_none());
    assert!(matches!(auth.bootstrap().await, BootstrapOutcome::SignedOut));
    assert!(auth.access_token().await.is_err());
}

#[tokio::test]
async fn sign_out_keyring_failure_keeps_cache() {
    let server = MockServer::start().await;
    // One refresh during bootstrap caches an access token. Sign-out must NOT cause
    // a second token call, and on keyring-clear failure the cache stays valid.
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token": "at-cached", "token_type": "Bearer",
            "expires_in": 3600, "refresh_token": "rt-new"
        })))
        .expect(1)
        .mount(&server)
        .await;

    let store = Arc::new(ClearFailsStore(InMemoryTokenStore::default()));
    store
        .save(&StoredToken { refresh_token: "rt-old".into(), account_id: "primary".into() })
        .await
        .unwrap();
    let auth = Authenticator::new(oauth_for(&server), store, "primary");

    assert!(matches!(auth.bootstrap().await, BootstrapOutcome::Ready)); // caches at-cached

    // Keyring delete fails -> Err, and the cached token is left intact.
    assert!(auth.sign_out().await.is_err());
    assert_eq!(auth.access_token().await.unwrap(), "at-cached"); // from cache, no 2nd /token
}
