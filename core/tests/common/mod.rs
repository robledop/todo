use std::sync::Mutex;
use async_trait::async_trait;
use outlook_tasks_core::AuthError;
use outlook_tasks_core::graph::TokenProvider;

/// Always returns the same token. `force_refresh` returns the same value.
pub struct StaticTokenProvider(pub String);

#[async_trait]
impl TokenProvider for StaticTokenProvider {
    async fn access_token(&self) -> Result<String, AuthError> {
        Ok(self.0.clone())
    }
    async fn force_refresh(&self) -> Result<String, AuthError> {
        Ok(self.0.clone())
    }
}

/// Returns `stale` until `force_refresh` is called once, then returns `fresh`.
/// Records how many times `force_refresh` was invoked.
pub struct ScriptedProvider {
    state: Mutex<(String, u32)>,
    fresh: String,
}

impl ScriptedProvider {
    pub fn new(stale: &str, fresh: &str) -> Self {
        Self { state: Mutex::new((stale.to_string(), 0)), fresh: fresh.to_string() }
    }
    pub fn refresh_count(&self) -> u32 {
        self.state.lock().unwrap().1
    }
}

#[async_trait]
impl TokenProvider for ScriptedProvider {
    async fn access_token(&self) -> Result<String, AuthError> {
        Ok(self.state.lock().unwrap().0.clone())
    }
    async fn force_refresh(&self) -> Result<String, AuthError> {
        let mut g = self.state.lock().unwrap();
        g.0 = self.fresh.clone();
        g.1 += 1;
        Ok(g.0.clone())
    }
}
