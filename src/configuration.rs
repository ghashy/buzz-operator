use serde::Deserialize;
use std::{collections::HashMap, path::PathBuf};

// ───── Body ─────────────────────────────────────────────────────────────── //

#[derive(Deserialize, Debug)]
pub struct Settings {
    pub instances_count: u16,
    pub stable_exec: String,
    pub new_exec: String,
    pub app_dir: PathBuf,
    pub health_check: Option<u16>,
}

impl Settings {
    pub fn load_configuration() -> Result<Settings, config::ConfigError> {
        // Initialise our configuration reader
        let settings = config::Config::builder()
            .add_source(config::File::with_name("configuration"))
            .build()?;

        // Try to deserialize the configuration values it read into
        // our `Settings` type.
        settings.try_deserialize()
    }
}
