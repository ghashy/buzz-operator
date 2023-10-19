use std::fs::File;
use std::io::{BufWriter, Write};

use chrono::Local;

use crate::configuration::AppConfig;

pub struct Log {
    app_config: AppConfig,
    log_file: BufWriter<File>,
}

impl Log {
    pub fn new(config: AppConfig) -> Log {
        let file = File::options()
            .create(true)
            .append(true)
            .write(true)
            .open(&config.log_dir)
            .expect("Can't open log file!");
        Self {
            app_config: config,
            log_file: BufWriter::new(file),
        }
    }

    pub fn write(&mut self, message: &str) -> Result<(), std::io::Error> {
        let current_datetime = Local::now();
        let datetime = current_datetime.format("%Y-%m-%d %H:%M:%S").to_string();
        // Also print log to stdout to view it in `journalctl` utility
        println!("{}: {}", datetime, message);
        writeln!(&mut self.log_file, "{}: {}", datetime, message)?;
        self.log_file.flush()?;
        Ok(())
    }
}
