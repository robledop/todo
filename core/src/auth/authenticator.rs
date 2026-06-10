use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use tokio::sync::Mutex;

use crate::auth::oauth::{OAuthClient, TokenSet};
use crate::auth::store::{StoredToken, TokenStore};
use crate::error::{AuthError, KeyringError};
use crate::graph::TokenProvider;

/// Refresh this long before actual expiry to avoid using a near-dead token.
const EXPIRY_SKEW: Duration = Duration::from_secs(60);

/// Result of a startup session check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootstrapOutcome {
    Ready,
    SignedOut,
    NoKeyring,
}

struct CachedAccess {
    token: String,
    expires_at: Instant,
}

/// Heuristic: an `invalid_grant` from the token endpoint means the refresh token
/// is revoked/expired and must be discarded (vs a transient network error, where
/// we keep the token and retry later).
fn is_invalid_grant(err: &AuthError) -> bool {
    matches!(err, AuthError::Provider(msg) if msg.contains("invalid_grant"))
}

/// Owns the refresh-token store and the OAuth client; serves valid access tokens.
pub struct Authenticator {
    oauth: OAuthClient,
    store: Arc<dyn TokenStore>,
    account_id: String,
    cache: Mutex<Option<CachedAccess>>,
    /// Serializes refreshes so concurrent callers can't redeem the same refresh
    /// token in parallel and race Microsoft's rotation (single-flight).
    refresh_lock: Mutex<()>,
}

impl Authenticator {
    pub fn new(
        oauth: OAuthClient,
        store: Arc<dyn TokenStore>,
        account_id: impl Into<String>,
    ) -> Self {
        Self {
            oauth,
            store,
            account_id: account_id.into(),
            cache: Mutex::new(None),
            refresh_lock: Mutex::new(()),
        }
    }

    /// Persists tokens from a fresh sign-in and caches the access token.
    pub async fn complete_sign_in(&self, tokens: TokenSet) -> Result<(), AuthError> {
        let refresh_token = tokens.refresh_token.clone().ok_or(AuthError::NoRefreshToken)?;
        self.store
            .save(&StoredToken { refresh_token, account_id: self.account_id.clone() })
            .await
            .map_err(|e| AuthError::Store(e.to_string()))?;
        self.cache_access(&tokens).await;
        Ok(())
    }

    /// Startup check: classifies into Ready / SignedOut / NoKeyring.
    pub async fn bootstrap(&self) -> BootstrapOutcome {
        match self.store.load().await {
            Err(KeyringError::Unavailable) => BootstrapOutcome::NoKeyring,
            Err(_) => BootstrapOutcome::SignedOut,
            Ok(None) => BootstrapOutcome::SignedOut,
            Ok(Some(_)) => match self.refresh_locked(true).await {
                Ok(_) => BootstrapOutcome::Ready,
                Err(_) => BootstrapOutcome::SignedOut,
            },
        }
    }

    /// Refreshes the access token under the single-flight lock. When `force` is
    /// false, re-checks the cache after acquiring the lock so a refresh that just
    /// completed on another task is reused instead of duplicated. Clears the
    /// stored refresh token on an unrecoverable `invalid_grant`.
    async fn refresh_locked(&self, force: bool) -> Result<String, AuthError> {
        let _guard = self.refresh_lock.lock().await;
        if !force {
            if let Some(c) = self.cache.lock().await.as_ref() {
                if c.expires_at > Instant::now() {
                    return Ok(c.token.clone());
                }
            }
        }
        let stored = self
            .store
            .load()
            .await
            .map_err(|e| AuthError::Store(e.to_string()))?
            .ok_or(AuthError::NoRefreshToken)?;
        let tokens = match self.oauth.refresh(&stored.refresh_token).await {
            Ok(t) => t,
            Err(e) => {
                if is_invalid_grant(&e) {
                    let _ = self.store.clear().await;
                    *self.cache.lock().await = None;
                }
                return Err(e);
            }
        };
        if let Some(new_rt) = &tokens.refresh_token {
            self.store
                .save(&StoredToken {
                    refresh_token: new_rt.clone(),
                    account_id: self.account_id.clone(),
                })
                .await
                .map_err(|e| AuthError::Store(e.to_string()))?;
        }
        self.cache_access(&tokens).await;
        Ok(tokens.access_token)
    }

    async fn cache_access(&self, tokens: &TokenSet) {
        let expires_at = Instant::now() + tokens.expires_in.saturating_sub(EXPIRY_SKEW);
        *self.cache.lock().await =
            Some(CachedAccess { token: tokens.access_token.clone(), expires_at });
    }
}

#[async_trait]
impl TokenProvider for Authenticator {
    async fn access_token(&self) -> Result<String, AuthError> {
        if let Some(c) = self.cache.lock().await.as_ref() {
            if c.expires_at > Instant::now() {
                return Ok(c.token.clone());
            }
        }
        self.refresh_locked(false).await
    }

    async fn force_refresh(&self) -> Result<String, AuthError> {
        self.refresh_locked(true).await
    }
}
