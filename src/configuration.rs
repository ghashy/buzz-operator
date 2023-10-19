use std::path::PathBuf;

use serde::Deserialize;

use crate::connect_addr::ConnectAddr;

#[derive(Deserialize, Debug)]
pub struct AppConfig {
    pub log_dir: PathBuf,
}

impl AppConfig {
    pub fn load_configuration() -> Result<AppConfig, config::ConfigError> {
        // Initialise our configuration reader
        let settings = config::Config::builder()
            .add_source(config::File::with_name("app_config"))
            .build()?;

        settings.try_deserialize()
    }
}

#[derive(Deserialize, Debug)]
pub struct ServiceConfig {
    pub name: String,
    pub instances_count: u16,
    pub stable_exec_name: String,
    pub new_exec_name: String,
    pub app_dir: PathBuf,
    pub connect_addr: ConnectAddr,
    pub roll_time_sec: Option<u32>,
    pub start_args: Option<Vec<String>>,
    pub update_script: Option<PathBuf>,
    pub log_dir: Option<PathBuf>,
}

impl ServiceConfig {
    pub fn get_log_dir(&self) -> PathBuf {
        self.log_dir
            .clone()
            .unwrap_or_else(|| self.app_dir.join("logs"))
    }
}

#[derive(Deserialize, Debug)]
pub struct Configuration {
    pub services: Vec<ServiceConfig>,
}

impl Configuration {
    pub fn load_configuration() -> Result<Configuration, config::ConfigError> {
        // Initialise our configuration reader
        let settings = config::Config::builder()
            .add_source(config::File::with_name("sman.yaml"))
            .build()?;

        settings.try_deserialize()
    }
}
