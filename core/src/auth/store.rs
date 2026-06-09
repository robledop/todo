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

use std::collections::HashMap;

/// Maps an oo7 error to `Unavailable` (no Secret Service provider / no bus /
/// missing default collection) or `Other`.
pub fn classify_keyring_error(err: oo7::Error) -> KeyringError {
    match err {
        oo7::Error::DBus(oo7::dbus::Error::ZBus(_))
        | oo7::Error::DBus(oo7::dbus::Error::IO(_))
        | oo7::Error::DBus(oo7::dbus::Error::NotFound(_)) => KeyringError::Unavailable,
        other => KeyringError::Other(other.to_string()),
    }
}

/// Secret Service (freedesktop) backed store via oo7. The account id is fixed at
/// construction so `load`/`save`/`clear` always key the same item (v1 is single
/// account; a future multi-account version constructs one store per account).
pub struct Oo7TokenStore {
    app_id: String,
    account_id: String,
}

impl Oo7TokenStore {
    pub fn new(app_id: impl Into<String>, account_id: impl Into<String>) -> Self {
        Self { app_id: app_id.into(), account_id: account_id.into() }
    }

    fn attributes(&self) -> HashMap<&str, &str> {
        HashMap::from([("app", self.app_id.as_str()), ("account", self.account_id.as_str())])
    }

    async fn keyring(&self) -> Result<oo7::Keyring, KeyringError> {
        let keyring = oo7::Keyring::new().await.map_err(classify_keyring_error)?;
        keyring.unlock().await.map_err(classify_keyring_error)?;
        Ok(keyring)
    }
}

#[async_trait]
impl TokenStore for Oo7TokenStore {
    async fn load(&self) -> Result<Option<StoredToken>, KeyringError> {
        let keyring = self.keyring().await?;
        let attrs = self.attributes();
        let items = keyring.search_items(&attrs).await.map_err(classify_keyring_error)?;
        let Some(item) = items.into_iter().next() else {
            return Ok(None);
        };
        let secret = item.secret().await.map_err(classify_keyring_error)?;
        // Refresh tokens are ASCII; reject corruption rather than masking it.
        let refresh_token = String::from_utf8(secret.as_bytes().to_vec())
            .map_err(|e| KeyringError::Other(format!("stored token is not valid UTF-8: {e}")))?;
        Ok(Some(StoredToken { refresh_token, account_id: self.account_id.clone() }))
    }

    async fn save(&self, token: &StoredToken) -> Result<(), KeyringError> {
        let keyring = self.keyring().await?;
        let attrs = self.attributes();
        keyring
            .create_item(
                "Outlook Tasks OAuth refresh token",
                &attrs,
                token.refresh_token.as_str(),
                true, // replace existing -> no duplicates on rotation
            )
            .await
            .map_err(classify_keyring_error)
    }

    async fn clear(&self) -> Result<(), KeyringError> {
        let keyring = self.keyring().await?;
        let attrs = self.attributes();
        keyring.delete(&attrs).await.map_err(classify_keyring_error)
    }
}
