use std::fs::OpenOptions;
use std::path::Path;
use std::process::Stdio;

use tokio::process::Command;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

use crate::connect_addr::ConnectAddr;

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
}

pub struct ServiceUnitHandle {
    sender: oneshot::Sender<()>,
    join_handle: JoinHandle<Result<(), ServiceUnitError>>,
}

impl ServiceUnit {
    pub fn new(
        address: &ConnectAddr,
        exec: &Path,
        log: Option<&Path>,
        name: &str,
        start_args: Option<Vec<String>>,
    ) -> std::result::Result<ServiceUnit, std::io::Error> {
        let mut command = Command::new(exec);

        // Redirect output to logfile if any, or to dev/null
        if let Some(ref logfile) = log {
            match OpenOptions::new().create(true).append(true).open(logfile) {
                Ok(logfile) => {
                    command.stdin(Stdio::from(logfile));
                }
                Err(e) => {
                    tracing::warn!(
                        "Failed to create log file {} for {} unit service, error: {}",
                        logfile.display(),
                        name,
                        e
                    );
                    command.stdout(Stdio::null());
                    command.stderr(Stdio::null());
                }
            }
        } else {
            tracing::info!(
                "Log dir not set for {} unit service, redirect output to dev/null",
                "execname"
            );
            command.stdout(Stdio::null());
            command.stderr(Stdio::null());
        }

        // Run process in exec dir
        command.current_dir(exec.parent().unwrap());

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
        if let Some(args) = &start_args {
            command.args(args);
        }

        Ok(ServiceUnit { command })
    }

    /// This function will wait until child process exits itself,
    /// or the signal will be received over the `self.term_rx`.
    pub fn run(self) -> Result<ServiceUnitHandle, ServiceUnitError> {
        let child = self
            .command
            .spawn()
            .map_err(|e| ServiceUnitError::ServiceNotStarted(e))?;
        let (tx, rx) = oneshot::channel();

        let join_handle = tokio::spawn(async move {
            tokio::select! {
                stat = child.wait() => {
                    // Handle child process exit
                    match stat {
                        Ok(status) => {
                            if status.success() {
                                Ok(())
                            } else {
                                Err(ServiceUnitError::ExitError { id: child.id().unwrap(), err: format!("Exited with code {:?}", status.code())})
                            }
                        },
                        Err(err) => {
                                Err(ServiceUnitError::ExitError { id: child.id().unwrap(), err: err.to_string()})
                        },
                    }
                }
                _ = rx => {
                    tracing::info!("Terminating service with id {}", child.id().unwrap());
                    // Handle rolling update request
                    Self::terminate(child.id().unwrap() as i32);
                    Ok(())

                }
            }
        });
        Ok(ServiceUnitHandle {
            sender: tx,
            join_handle,
        })
    }

    fn terminate(pid: i32) {
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

// #[cfg(test)]
// mod tests {
//     use super::*;

//     #[tokio::test]
//     async fn wait_success() {
//         let (_, term_rx) = mpsc::channel(1);
//         let mut command = Command::new("echo");
//         command.arg("Hello, World!");

//         let mut service = ServiceUnit {
//             command,
//             child: None,
//             id: None,
//             term_rx,
//         };
//         let run = service.run();
//         assert!(run.is_ok());

//         let result = service.wait_on().await;
//         assert!(result.is_ok());
//     }

//     #[tokio::test]
//     async fn wait_failure() {
//         let (_term_tx, term_rx) = mpsc::channel(1);
//         let command = Command::new("false");

//         let mut service = ServiceUnit {
//             command,
//             child: None,
//             id: None,
//             term_rx,
//         };
//         let run = service.run();
//         assert!(run.is_ok());

//         let result = service.wait_on().await;
//         assert!(result.is_err());
//     }

//     #[tokio::test]
//     async fn terminate_success() {
//         let (term_tx, term_rx) = mpsc::channel(1);
//         let mut command = Command::new("sleep");
//         command.arg("1");

//         let mut service = ServiceUnit {
//             command,
//             child: None,
//             id: None,
//             term_rx,
//         };

//         // Run service
//         let run = service.run();
//         assert!(run.is_ok());

//         let pid = run.unwrap();

//         // Assert that process exists
//         assert_eq!(unsafe { libc::kill(pid as i32, 0) }, 0);

//         let (service, term) = tokio::join!(service.wait_on(), term_tx.send(()));

//         // Assert that terminate signal was sent
//         assert!(term.is_ok());
//         assert!(service.is_ok());

//         // Wait system to perform termination
//         tokio::time::sleep(std::time::Duration::from_millis(10)).await;

//         // Assert that process was terminated
//         assert_eq!(unsafe { libc::kill(pid as i32, 0) }, -1);
//     }
// }
