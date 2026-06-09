use std::sync::Mutex;
use async_trait::async_trait;

use crate::error::KeyringError;

/// What we persist between launches. The access token is never stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredToken {
    pub refresh_token: String,
    pub account_id: String,
}

/// Abstracts secret storage so the authenticator is testable without a keyring.
#[async_trait]
pub trait TokenStore: Send + Sync {
    async fn load(&self) -> Result<Option<StoredToken>, KeyringError>;
    async fn save(&self, token: &StoredToken) -> Result<(), KeyringError>;
    async fn clear(&self) -> Result<(), KeyringError>;
}

/// In-memory store for tests.
#[derive(Default)]
pub struct InMemoryTokenStore {
    inner: Mutex<Option<StoredToken>>,
}

#[async_trait]
impl TokenStore for InMemoryTokenStore {
    async fn load(&self) -> Result<Option<StoredToken>, KeyringError> {
        Ok(self.inner.lock().unwrap().clone())
    }
    async fn save(&self, token: &StoredToken) -> Result<(), KeyringError> {
        *self.inner.lock().unwrap() = Some(token.clone());
        Ok(())
    }
    async fn clear(&self) -> Result<(), KeyringError> {
        *self.inner.lock().unwrap() = None;
        Ok(())
    }
}
