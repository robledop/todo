use outlook_tasks_core::auth::{AuthConfig, OAuthClient};
use serde_json::json;
use wiremock::matchers::{body_string_contains, method, path};
use wiremock::{Match, Mock, MockServer, Request, ResponseTemplate};

/// Asserts a substring is ABSENT from the request body (e.g. no client secret).
struct BodyExcludes(&'static str);
impl Match for BodyExcludes {
    fn matches(&self, request: &Request) -> bool {
        !std::str::from_utf8(&request.body).unwrap_or("").contains(self.0)
    }
}

fn config_for(server: &MockServer) -> AuthConfig {
    AuthConfig {
        client_id: "cid".into(),
        auth_url: format!("{}/authorize", server.uri()),
        token_url: format!("{}/token", server.uri()),
        redirect_url: "http://localhost:1/".into(),
        scopes: vec!["Tasks.ReadWrite".into(), "offline_access".into(), "openid".into()],
    }
}

#[tokio::test]
async fn authorize_url_contains_pkce_and_scopes() {
    let server = MockServer::start().await;
    let oauth = OAuthClient::new(&config_for(&server)).unwrap();
    let pending = oauth.begin_auth();
    let url = pending.authorize_url.to_string();
    assert!(url.contains("client_id=cid"));
    assert!(url.contains("code_challenge="));
    assert!(url.contains("code_challenge_method=S256"));
    assert!(url.contains("Tasks.ReadWrite"));
    assert!(url.contains("offline_access"));
    assert!(!pending.csrf_state.is_empty());
}

#[tokio::test]
async fn exchange_code_returns_tokens() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/token"))
        // PKCE auth-code exchange must send these form fields and NO secret.
        .and(body_string_contains("grant_type=authorization_code"))
        .and(body_string_contains("code=code123"))
        .and(body_string_contains("code_verifier="))
        .and(body_string_contains("client_id=cid"))
        .and(BodyExcludes("client_secret"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token": "at1", "token_type": "Bearer",
            "expires_in": 3600, "refresh_token": "rt1", "scope": "Tasks.ReadWrite"
        })))
        .expect(1)
        .mount(&server)
        .await;

    let oauth = OAuthClient::new(&config_for(&server)).unwrap();
    let pending = oauth.begin_auth();
    let state = pending.csrf_state.clone();
    let tokens = oauth.exchange_code(pending, "code123".into(), state).await.unwrap();
    assert_eq!(tokens.access_token, "at1");
    assert_eq!(tokens.refresh_token.as_deref(), Some("rt1"));
}

#[tokio::test]
async fn exchange_code_rejects_state_mismatch() {
    let server = MockServer::start().await;
    let oauth = OAuthClient::new(&config_for(&server)).unwrap();
    let pending = oauth.begin_auth();
    let err = oauth.exchange_code(pending, "code123".into(), "WRONG".into()).await.unwrap_err();
    assert!(matches!(err, outlook_tasks_core::AuthError::StateMismatch));
}

#[tokio::test]
async fn refresh_returns_rotated_token() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token": "at2", "token_type": "Bearer",
            "expires_in": 3600, "refresh_token": "rt2"
        })))
        .mount(&server)
        .await;

    let oauth = OAuthClient::new(&config_for(&server)).unwrap();
    let tokens = oauth.refresh("rt1").await.unwrap();
    assert_eq!(tokens.access_token, "at2");
    assert_eq!(tokens.refresh_token.as_deref(), Some("rt2"));
}

use outlook_tasks_core::auth::parse_redirect;

#[test]
fn parse_redirect_extracts_code_and_state() {
    let p = parse_redirect("/?code=ABC&state=XYZ").unwrap();
    assert_eq!(p.code, "ABC");
    assert_eq!(p.state, "XYZ");
}

#[test]
fn parse_redirect_surfaces_provider_error() {
    let err = parse_redirect("/?error=access_denied").unwrap_err();
    assert!(matches!(err, outlook_tasks_core::AuthError::Provider(_)));
}

use outlook_tasks_core::auth::{RedirectParams, TokenSet};

#[test]
fn token_set_debug_redacts_secrets() {
    let ts = TokenSet {
        access_token: "super-secret-access".into(),
        refresh_token: Some("super-secret-refresh".into()),
        expires_in: std::time::Duration::from_secs(3600),
    };
    let dbg = format!("{ts:?}");
    assert!(!dbg.contains("super-secret-access"), "access token leaked: {dbg}");
    assert!(!dbg.contains("super-secret-refresh"), "refresh token leaked: {dbg}");
    assert!(dbg.contains("<redacted>"));
}

#[test]
fn redirect_params_debug_redacts_code_and_state() {
    let p = RedirectParams { code: "the-auth-code".into(), state: "the-csrf-state".into() };
    let dbg = format!("{p:?}");
    assert!(!dbg.contains("the-auth-code"), "code leaked: {dbg}");
    assert!(!dbg.contains("the-csrf-state"), "state leaked: {dbg}");
}

#[test]
fn consumers_requests_user_read_scope() {
    let cfg = AuthConfig::consumers("cid", "http://localhost/");
    assert!(cfg.scopes.iter().any(|s| s == "User.Read"), "User.Read must be requested");
    assert!(cfg.scopes.iter().any(|s| s == "Tasks.ReadWrite"));
    assert!(cfg.scopes.iter().any(|s| s == "offline_access"));
}
