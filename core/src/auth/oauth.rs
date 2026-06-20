use std::time::Duration;

use oauth2::basic::{BasicClient, BasicTokenResponse};
use oauth2::{
    AuthUrl, AuthorizationCode, ClientId, CsrfToken, EndpointNotSet, EndpointSet,
    PkceCodeChallenge, PkceCodeVerifier, RedirectUrl, RefreshToken, Scope, TokenResponse, TokenUrl,
};
use url::Url;

use crate::error::AuthError;

/// Bound the IdP HTTP calls so a stalled token endpoint can't hang sign-in or a
/// refresh indefinitely (a refresh holds the authenticator's refresh lock for
/// its whole duration).
const HTTP_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const HTTP_TOTAL_TIMEOUT: Duration = Duration::from_secs(30);

/// Fully-typed oauth2 client once auth + token URIs are set.
type ConfiguredClient =
    BasicClient<EndpointSet, EndpointNotSet, EndpointNotSet, EndpointNotSet, EndpointSet>;

/// Endpoint + scope configuration. `AuthConfig::consumers` builds the Microsoft
/// personal-accounts (`/consumers`) URLs; tests construct it with mock URLs.
#[derive(Debug, Clone)]
pub struct AuthConfig {
    pub client_id: String,
    pub auth_url: String,
    pub token_url: String,
    pub redirect_url: String,
    pub scopes: Vec<String>,
}

impl AuthConfig {
    pub fn consumers(client_id: impl Into<String>, redirect_url: impl Into<String>) -> Self {
        Self {
            client_id: client_id.into(),
            auth_url: "https://login.microsoftonline.com/consumers/oauth2/v2.0/authorize".into(),
            token_url: "https://login.microsoftonline.com/consumers/oauth2/v2.0/token".into(),
            redirect_url: redirect_url.into(),
            // `Tasks.ReadWrite` for the data, `offline_access` for a refresh
            // token. `openid` is requested for Microsoft's consent/refresh
            // behavior only - the returned id_token is intentionally never parsed
            // or trusted (auth decisions use the Graph access token alone), so no
            // nonce/id_token validation is required.
            scopes: vec![
                "Tasks.ReadWrite".into(),
                "offline_access".into(),
                "openid".into(),
            ],
        }
    }
}

/// Tokens returned by an exchange or refresh. `Debug` is hand-written to keep the
/// access/refresh token values out of logs and error formatting.
#[derive(Clone)]
pub struct TokenSet {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_in: Duration,
}

impl std::fmt::Debug for TokenSet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TokenSet")
            .field("access_token", &"<redacted>")
            .field("refresh_token", &self.refresh_token.as_ref().map(|_| "<redacted>"))
            .field("expires_in", &self.expires_in)
            .finish()
    }
}

/// State carried between building the authorize URL and redeeming the code.
/// `verifier` is move-only (consumed by `exchange_code`).
pub struct PendingAuth {
    pub authorize_url: Url,
    pub csrf_state: String,
    verifier: PkceCodeVerifier,
}

pub struct OAuthClient {
    client: ConfiguredClient,
    http: reqwest::Client,
    scopes: Vec<String>,
}

impl OAuthClient {
    pub fn new(cfg: &AuthConfig) -> Result<Self, AuthError> {
        let client = BasicClient::new(ClientId::new(cfg.client_id.clone()))
            .set_auth_uri(
                AuthUrl::new(cfg.auth_url.clone()).map_err(|e| AuthError::Protocol(e.to_string()))?,
            )
            .set_token_uri(
                TokenUrl::new(cfg.token_url.clone()).map_err(|e| AuthError::Protocol(e.to_string()))?,
            )
            .set_redirect_uri(
                RedirectUrl::new(cfg.redirect_url.clone())
                    .map_err(|e| AuthError::Protocol(e.to_string()))?,
            );
        // No redirects: avoids SSRF on the token endpoint.
        let http = reqwest::ClientBuilder::new()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(HTTP_CONNECT_TIMEOUT)
            .timeout(HTTP_TOTAL_TIMEOUT)
            .build()
            .map_err(|e| AuthError::Protocol(e.to_string()))?;
        Ok(Self { client, http, scopes: cfg.scopes.clone() })
    }

    /// Builds the authorize URL with a fresh PKCE challenge + CSRF state.
    pub fn begin_auth(&self) -> PendingAuth {
        let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();
        let mut req = self.client.authorize_url(CsrfToken::new_random);
        for s in &self.scopes {
            req = req.add_scope(Scope::new(s.clone()));
        }
        let (url, csrf) = req.set_pkce_challenge(challenge).url();
        PendingAuth { authorize_url: url, csrf_state: csrf.secret().clone(), verifier }
    }

    /// Redeems an authorization code after validating CSRF state.
    pub async fn exchange_code(
        &self,
        pending: PendingAuth,
        code: String,
        returned_state: String,
    ) -> Result<TokenSet, AuthError> {
        if !constant_time_eq(&returned_state, &pending.csrf_state) {
            return Err(AuthError::StateMismatch);
        }
        let resp = self
            .client
            .exchange_code(AuthorizationCode::new(code))
            .set_pkce_verifier(pending.verifier)
            .request_async(&self.http)
            .await
            .map_err(|e| AuthError::Provider(e.to_string()))?;
        Ok(token_set_from(&resp))
    }

    /// Refreshes using a stored refresh token. Handles rotation at the caller.
    pub async fn refresh(&self, refresh_token: &str) -> Result<TokenSet, AuthError> {
        let resp = self
            .client
            .exchange_refresh_token(&RefreshToken::new(refresh_token.to_string()))
            .request_async(&self.http)
            .await
            .map_err(|e| AuthError::Provider(e.to_string()))?;
        Ok(token_set_from(&resp))
    }
}

fn token_set_from(resp: &BasicTokenResponse) -> TokenSet {
    TokenSet {
        access_token: resp.access_token().secret().clone(),
        refresh_token: resp.refresh_token().map(|r| r.secret().clone()),
        expires_in: resp.expires_in().unwrap_or(Duration::from_secs(3600)),
    }
}

/// Length-independent byte comparison to avoid a timing oracle on the CSRF state.
pub(crate) fn constant_time_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Parsed authorization redirect parameters. `Debug` is hand-written so the
/// one-time `code` and CSRF `state` never reach logs or assertion output.
#[derive(Clone, PartialEq, Eq)]
pub struct RedirectParams {
    pub code: String,
    pub state: String,
}

impl std::fmt::Debug for RedirectParams {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RedirectParams")
            .field("code", &"<redacted>")
            .field("state", &"<redacted>")
            .finish()
    }
}

/// Parses the `code`/`state` from a loopback redirect request line such as
/// `/?code=ABC&state=XYZ`. Returns `Provider` if the IdP returned `error=...`.
pub fn parse_redirect(request_target: &str) -> Result<RedirectParams, AuthError> {
    let full = format!("http://localhost{request_target}");
    let url = Url::parse(&full).map_err(|e| AuthError::Protocol(e.to_string()))?;
    let pairs: std::collections::HashMap<String, String> =
        url.query_pairs().into_owned().collect();
    if let Some(err) = pairs.get("error") {
        return Err(AuthError::Provider(err.clone()));
    }
    let code = pairs.get("code").ok_or(AuthError::MissingCode)?.clone();
    let state = pairs.get("state").cloned().unwrap_or_default();
    Ok(RedirectParams { code, state })
}
