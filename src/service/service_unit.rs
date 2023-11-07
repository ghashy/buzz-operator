use std::fs::OpenOptions;
use std::process::Stdio;

use async_trait::async_trait;
use tokio::process::Command;
use tokio::sync::mpsc;

use crate::configuration::ServiceConfig;
use crate::connect_addr::ConnectAddr;

use super::Service;

pub enum UnitVersion {
    Stable,
    New,
}

pub(super) type ProcessID = u32;

/// Error type for `ServiceUnit`
#[derive(Debug)]
pub enum ServiceUnitError {
    /// `ServiceUnit` exited with non zero code
    ExitError {
        id: ProcessID,
        err: String,
    },
    /// `run` function failed to start service
    ServiceNotStarted(std::io::Error),
    HealthCheckFailed(ConnectAddr),
    /// Any error with [tokio::sync::mpsc::channel]
    MpscError,
}

impl std::error::Error for ServiceUnitError {}

impl std::fmt::Display for ServiceUnitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ServiceUnitError::ExitError { id, err } => {
                f.write_fmt(format_args!("{} pid: {} ", err, id))
            }
            ServiceUnitError::ServiceNotStarted(err) => f.write_fmt(format_args!("{}", err)),
            ServiceUnitError::MpscError => f.write_str("Mpsc error"),
            ServiceUnitError::HealthCheckFailed(addr) => f.write_fmt(format_args!(
                "Health check failed, addr: {}",
                addr.to_string()
            )),
        }
    }
}

/// The lowest abstraction above unix system process.
///
/// This is a very simple type, which allows to run, wait, and terminate unix
/// process. You should follow the [Service](crate::service::Service) trait's
/// methods call order.
pub struct ServiceUnit {
    /// After creating, we store parametrized `Command` in this field
    command: Command,
    /// We store handle to actual process in this field after `run` was called
    child: Option<tokio::process::Child>,
    /// We can know `ProcessID` only after we `run` it
    id: Option<ProcessID>,
    /// Channel for communicating with higher (in program hierarchy) object
    term_rx: mpsc::Receiver<()>,
}

impl ServiceUnit {
    /// Create a new `ServiceUnit`. At this point `ServiceUnit` is not ran.
    /// To run `ServiceUnit`, you should call `run_and_wait` function.
    /// WARN: addres should not be taken from conf!
    ///
    /// `version` - spawn stable binary version or new.
    pub fn new(
        config: &ServiceConfig,
        address: &ConnectAddr,
        term_rx: mpsc::Receiver<()>,
        version: UnitVersion,
    ) -> std::result::Result<ServiceUnit, std::io::Error> {
        let mut command = Command::new(&config.app_dir.join(match version {
            UnitVersion::Stable => &config.stable_exec_name,
            UnitVersion::New => &config.new_exec_name,
        }));

        // Redirect output to logfile if any, or to dev/null
        if let Some(ref logfile) = config.log_dir {
            match OpenOptions::new()
                .create(true)
                .append(true)
                .open(logfile.join(&config.name))
            {
                Ok(logfile) => {
                    command.stdin(Stdio::from(logfile));
                }
                Err(e) => {
                    tracing::warn!(
                        "Failed to create log file {} for {} unit service, error: {}",
                        logfile.display(),
                        config.name,
                        e
                    );
                    command.stdout(Stdio::null());
                    command.stderr(Stdio::null());
                }
            }
        } else {
            tracing::info!(
                "Log dir not set for {} unit service, redirect output to dev/null",
                config.name
            );
            command.stdout(Stdio::null());
            command.stderr(Stdio::null());
        }

        // Run process in `app_dir` from provided config
        command.current_dir(&config.app_dir);

        // WARN: Backend service we run should accept these arguments
        match address {
            ConnectAddr::Unix(sock) => {
                command.args(&["--unix-socket".into(), sock.to_owned()]);
            }
            ConnectAddr::Tcp { addr, port } => {
                command.args(&["--ip", &addr.to_string(), "--port", &port.to_string()]);
            }
        }

        // If custom args provided, pass them
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

    /// Currently, this app don't support custom termination signals.
    /// We terminate process in any case. At first we try to terminate it gently,
    /// and then if it didn't work we send [libc::SIGKILL].
    fn terminate(&self) {
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

#[async_trait]
impl Service<ServiceUnitError> for ServiceUnit {
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
            _ = self.term_rx.recv() => {
                tracing::info!("Terminating service with id {}", self.id.unwrap());
                // Handle rolling update request
                self.terminate();
                Ok(())

            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

        let result = service.wait_on().await;
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

        let result = service.wait_on().await;
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

        // Run service
        let run = service.run();
        assert!(run.is_ok());

        let pid = run.unwrap();

        // Assert that process exists
        assert_eq!(unsafe { libc::kill(pid as i32, 0) }, 0);

        let (service, term) = tokio::join!(service.wait_on(), term_tx.send(()));

        // Assert that terminate signal was sent
        assert!(term.is_ok());
        assert!(service.is_ok());

        // Wait system to perform termination
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;

        // Assert that process was terminated
        assert_eq!(unsafe { libc::kill(pid as i32, 0) }, -1);
    }
}
