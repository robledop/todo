use cosmic::cosmic_config::{self, cosmic_config_derive::CosmicConfigEntry, CosmicConfigEntry};

#[derive(Debug, Clone, PartialEq, Eq, CosmicConfigEntry)]
#[version = 1]
pub struct Config {
    pub selected_list_id: Option<String>,
    pub poll_interval_secs: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self { selected_list_id: None, poll_interval_secs: 300 }
    }
}

impl Config {
    /// Loads the persisted config, falling back to defaults on any error.
    pub fn load() -> Self {
        cosmic_config::Config::new(crate::consts::APP_ID, Config::VERSION)
            .map(|ctx| match Config::get_entry(&ctx) {
                Ok(config) => config,
                Err((_errors, config)) => config,
            })
            .unwrap_or_default()
    }
}
