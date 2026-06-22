use std::time::Duration;
use thiserror::Error;

/// Errors from the OAuth / token layer.
#[derive(Debug, Error)]
pub enum AuthError {
    #[error("oauth provider error: {0}")]
    Provider(String),
    #[error("protocol error: {0}")]
    Protocol(String),
    #[error("redirect did not contain an authorization code")]
    MissingCode,
    #[error("csrf state mismatch")]
    StateMismatch,
    #[error("no refresh token is stored; sign-in required")]
    NoRefreshToken,
    #[error("token store error: {0}")]
    Store(String),
}

/// Errors from the Graph HTTP layer.
#[derive(Debug, Error)]
pub enum GraphError {
    #[error("unauthorized")]
    Unauthorized,
    #[error("forbidden")]
    Forbidden,
    #[error("throttled")]
    Throttled { retry_after: Option<Duration> },
    #[error("token acquisition failed: {0}")]
    Token(String),
    #[error("http {status}: {body}")]
    Http { status: u16, body: String },
    #[error("network error: {0}")]
    Network(String),
    #[error("decode error: {0}")]
    Decode(String),
    /// A relative ("Nth weekday") recurrence couldn't be written because the
    /// Outlook endpoint that stores it (the To Do endpoint can't) was unavailable.
    /// User-facing: surfaced verbatim in the task form.
    #[error(
        "Couldn't save the \"Nth weekday\" repeat - that service is unavailable right now. \
         Pick a different repeat option, or try again later."
    )]
    RecurrenceUnavailable,
}

/// Errors from the secret-storage layer.
#[derive(Debug, Error)]
pub enum KeyringError {
    #[error("no secret service provider is available")]
    Unavailable,
    #[error("keyring error: {0}")]
    Other(String),
}

/// Aggregate error surfaced to the applet layer.
#[derive(Debug, Error)]
pub enum CoreError {
    #[error(transparent)]
    Auth(#[from] AuthError),
    #[error(transparent)]
    Graph(#[from] GraphError),
    #[error(transparent)]
    Keyring(#[from] KeyringError),
}
