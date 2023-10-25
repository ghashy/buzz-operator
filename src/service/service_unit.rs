use async_trait::async_trait;
use tokio::process::Command;
use tokio::sync::mpsc;

use crate::configuration::ServiceConfig;
use crate::connect_addr::ConnectAddr;

use super::Service;

pub(super) type ProcessID = u32;

#[derive(Debug)]
pub enum ServiceUnitError {
    ExitError { id: ProcessID, err: String },
    ServiceNotStarted(std::io::Error),
    MpscError,
}

impl std::error::Error for ServiceUnitError {}

impl std::fmt::Display for ServiceUnitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ServiceUnitError::ExitError { id, err } => {
                f.write_fmt(format_args!("{} pid: {} ", err, id))
            }
            ServiceUnitError::ServiceNotStarted(err) => {
                f.write_fmt(format_args!("{}", err))
            }
            ServiceUnitError::MpscError => f.write_str("Mpsc error"),
        }
    }
}

pub struct ServiceUnit {
    command: Command,
    child: Option<tokio::process::Child>,
    id: Option<ProcessID>,
    term_rx: mpsc::Receiver<()>,
}

impl ServiceUnit {
    /// Create a new `ServiceUnit`. At this point `ServiceUnit` is not ran.
    /// To run `ServiceUnit`, you should call `run_and_wait` function.
    /// WARN: addres should not be taken from conf!
    pub fn new(
        config: &ServiceConfig,
        address: &ConnectAddr,
        term_rx: mpsc::Receiver<()>,
    ) -> std::result::Result<ServiceUnit, std::io::Error> {
        // Make sure log directory exists
        if !config.get_log_dir().exists() {
            std::fs::DirBuilder::new()
                .create(config.get_log_dir())
                .expect(&format!(
                    "Failed to create '{}' service log directory!",
                    config.name
                ));
            tracing::info!(
                "Created log directory: {}",
                config.get_log_dir().display()
            );
        }

        let mut command =
            Command::new(&config.app_dir.join(&config.stable_exec_name));
        command.current_dir(&config.app_dir);

        match address {
            ConnectAddr::Unix(sock) => {
                command.args(&["--unix-socket".into(), sock.to_owned()]);
            }
            ConnectAddr::Tcp { addr, port } => {
                command.args(&[
                    "--ip",
                    &addr.to_string(),
                    "--port",
                    &port.to_string(),
                ]);
            }
        }

        // If there were custom args, pass them
        if let Some(args) = &config.start_args {
            command.args(args);
        }

        Ok(ServiceUnit {
            command,
            child: None,
            id: None,
            term_rx,
        })
    }
}

#[async_trait]
impl Service<(), ServiceUnitError> for ServiceUnit {
    type Output = ProcessID;

    fn run(&mut self) -> Result<Self::Output, ServiceUnitError> {
        let child = self
            .command
            .spawn()
            .map_err(|e| ServiceUnitError::ServiceNotStarted(e))?;
        self.id = child.id();
        self.child = Some(child);
        Ok(self.id.unwrap())
    }

    /// This function will wait until child process exits itself,
    /// or the signal will be received over the `self.term_rx`.
    async fn wait_on(&mut self) -> Result<(), ServiceUnitError> {
        let mut child = self.child.take().unwrap();
        tokio::select! {
            stat = child.wait() => {
                // Handle child process exit
                match stat {
                    Ok(status) => {
                        if status.success() {
                            Ok(())
                        } else {
                            Err(ServiceUnitError::ExitError { id: self.id.unwrap(), err: format!("Exited with code {:?}", status.code())})
                        }
                    },
                    Err(err) => {
                            Err(ServiceUnitError::ExitError { id: self.id.unwrap(), err: err.to_string()})
                    },
                }
            }
            signal = self.term_rx.recv() => {
                tracing::info!("Terminating service with id {}", self.id.unwrap());
                // Handle rolling update request
                self.terminate();
                Ok(())

            }
        }
    }

    /// Try to terminate child process.
    fn terminate(&mut self) {
        let pid = self.id.unwrap() as i32;

        unsafe {
            if libc::kill(pid, 0) != 0 {
                tracing::info!("No process with pid {}!", pid);
                return;
            }
            let result = libc::kill(pid, libc::SIGTERM);
            if result == 0 {
                tracing::info!("Process with pid {} terminated", pid);
            } else {
                tracing::error!(
                    "Process with pid {}, can't be terminated, error code: {}",
                    pid,
                    result
                );
                let result = libc::kill(pid, libc::SIGKILL);
                if result == 0 {
                    tracing::warn!("Process with pid {} killed", pid);
                } else {
                    tracing::error!(
                        "Process with pid {}, can't be killed, error code: {}",
                        pid,
                        result
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio_test::task::spawn;

    #[tokio::test]
    async fn wait_success() {
        let (_, term_rx) = mpsc::channel(1);
        let mut command = Command::new("echo");
        command.arg("Hello, World!");

        let mut service = ServiceUnit {
            command,
            child: None,
            id: None,
            term_rx,
        };
        let run = service.run();
        assert!(run.is_ok());

        let result = spawn(service.wait_on()).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn wait_failure() {
        let (_term_tx, term_rx) = mpsc::channel(1);
        let command = Command::new("false");

        let mut service = ServiceUnit {
            command,
            child: None,
            id: None,
            term_rx,
        };
        let run = service.run();
        assert!(run.is_ok());

        let result = spawn(service.wait_on()).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn terminate_success() {
        let (term_tx, term_rx) = mpsc::channel(1);
        let mut command = Command::new("sleep");
        command.arg("1");

        let mut service = ServiceUnit {
            command,
            child: None,
            id: None,
            term_rx,
        };

        let run = service.run();
        assert!(run.is_ok());
        let pid = run.unwrap();
        assert_eq!(unsafe { libc::kill(pid as i32, 0) }, 0);

        let result = tokio::select! {
            result = term_tx.send(()) => {
                Ok(())
            }
            result = service.wait_on() => {
                result
            }
        };

        assert_eq!(unsafe { libc::kill(pid as i32, 0) }, 1);
        assert!(result.is_ok());
    }
}
