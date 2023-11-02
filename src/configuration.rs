//! Here we manage user configuration file.

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

#[derive(Deserialize, Debug, Clone)]
pub struct ServiceConfig {
    pub name: String,
    pub instances_count: u16,
    pub stable_exec_name: String,
    pub new_exec_name: String,
    pub connect_addr: ConnectAddr,
    pub roll_time_sec: Option<u32>,
    pub start_args: Option<Vec<String>>,
    pub update_script: Option<PathBuf>,
    pub app_dir: PathBuf,
    pub log_dir: Option<PathBuf>,
    #[serde(default = "default_fail_limit")]
    pub fails_limit: u16,
}

impl ServiceConfig {
    /// If `log_dir` param not set, `log` directory create automatically in the
    /// app directory
    pub fn get_log_dir(&self) -> PathBuf {
        self.log_dir
            .clone()
            .unwrap_or_else(|| self.app_dir.join("logs"))
    }
    pub fn create_test_config() -> ServiceConfig {
        ServiceConfig {
            name: "test".to_string(),
            instances_count: 5,
            stable_exec_name: "test_stable".to_string(),
            new_exec_name: "test_new".to_string(),
            connect_addr: ConnectAddr::Unix("test_app/sockets".into()),
            roll_time_sec: Some(10),
            start_args: None,
            update_script: None,
            app_dir: "test_app".into(),
            log_dir: None,
            fails_limit: 5,
        }
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
            .add_source(config::File::with_name("buzzoperator.yaml"))
            .build()?;

        settings.try_deserialize()
    }
}

fn default_fail_limit() -> u16 {
    5
}
