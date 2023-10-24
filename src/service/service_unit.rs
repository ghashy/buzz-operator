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
    TerminationFailed(ProcessID),
    ServiceNotStarted(std::io::Error),
    MpscError(tokio::sync::mpsc::error::SendError<TermSignal>),
}

impl std::error::Error for ServiceUnitError {}

impl std::fmt::Display for ServiceUnitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ServiceUnitError::ExitError { id, err } => {
                f.write_fmt(format_args!("{} pid: {} ", err, id))
            }
            ServiceUnitError::TerminationFailed(id) => f.write_fmt(
                format_args!("Failed to terminate service with pid {}", id),
            ),
            ServiceUnitError::ServiceNotStarted(err) => {
                f.write_fmt(format_args!("{}", err))
            }
            ServiceUnitError::MpscError(err) => {
                f.write_fmt(format_args!("{}", err))
            }
        }
    }
}

pub enum TermSignal {
    Terminate,
}

pub struct ServiceUnit {
    command: Command,
    child: Option<tokio::process::Child>,
    id: Option<ProcessID>,
    term_rx: mpsc::Receiver<TermSignal>,
}

impl ServiceUnit {
    /// Create a new `ServiceUnit`. At this point `ServiceUnit` is not ran.
    /// To run `ServiceUnit`, you should call `run_and_wait` function.
    /// WARN: addres should not be taken from conf!
    pub fn new(
        config: &ServiceConfig,
        address: &ConnectAddr,
        term_rx: mpsc::Receiver<TermSignal>,
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
            // TODO: implement different terminations.
            _ = self.term_rx.recv() => {
                tracing::info!("Terminating service with id {}", self.id.unwrap());
                // Handle rolling update request
                self.try_terminate()
            }
        }
    }

    /// Try to terminate child process.
    fn try_terminate(&mut self) -> Result<(), ServiceUnitError> {
        let id = self.id.unwrap();

        unsafe {
            if libc::kill(id as i32, libc::SIGTERM) == 0 {
                return Ok(());
            } else {
                return Err(ServiceUnitError::TerminationFailed(id));
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

        let result = tokio::select! {
            result = term_tx.send(TermSignal::Terminate) => {
                result
                      .map_err(|e| ServiceUnitError::MpscError(e))
            }
            result = service.wait_on() => {
                result
            }
        };

        assert!(result.is_ok());
    }
}
