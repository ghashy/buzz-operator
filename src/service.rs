use std::os::fd::AsFd;
use std::path::{Path, PathBuf};

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::mpsc;

use crate::configuration::{self, ServiceConfig};
use crate::connect_addr::ConnectAddr;

struct Service {
    process: Child,
    term_rx: mpsc::Receiver<()>,
}

impl Service {
    fn new(
        config: &ServiceConfig,
        addr: &ConnectAddr,
        term_rx: mpsc::Receiver<()>,
    ) -> Result<Service, std::io::Error> {
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
        let mut process = command.current_dir(&config.app_dir);

        match addr {
            ConnectAddr::Unix(sock) => {
                process =
                    process.args(&["--unix-socket".into(), sock.to_owned()]);
            }
            ConnectAddr::Tcp { addr, port } => {
                process = process.args(&[
                    "--ip",
                    &addr.to_string(),
                    "--port",
                    &port.to_string(),
                ]);
            }
        }

        // If there were custom args, pass them
        if let Some(args) = &config.start_args {
            process = process.args(args);
        }

        let process = process.spawn()?;

        Ok(Service { process, term_rx })
    }

    fn terminate(self) -> std::io::Result<()> {
        let id = self.process.id().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::NotFound, "a")
        })?;

        unsafe {
            if libc::kill(id as i32, libc::SIGTERM) == 0 {
                return Ok(());
            } else {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    "Kill failed",
                ));
            }
        }
    }

    // async fn get_output(&self) -> Option<&tokio::process::ChildStdout> {
    //     self.process.stdout.as_ref()
    // }

    /// This function will wait until child process exits itself,
    /// or the signal will be received over the `self.term_rx`.
    async fn wait(mut self) -> std::io::Result<()> {
        use std::io::{Error, ErrorKind};

        tokio::select! {
            stat = self.process.wait() => {
                // Handle child process exit
                match stat {
                    Ok(status) => {
                        if status.success() {
                            return Ok(())
                        } else {
                            return Err(Error::from(ErrorKind::Other))
                        }
                    },
                    Err(err) => {
                        return Err(err);
                    },
                }
            }
            _ = self.term_rx.recv() => {
                // Handle rolling update request
                self.terminate()
            }
        }
    }
}

pub struct ServiceBunch {
    name: String,
    services: Vec<mpsc::Sender<()>>,
}

impl ServiceBunch {
    pub fn new(name: &str) -> ServiceBunch {
        ServiceBunch {
            name: name.to_string(),
            services: Vec::new(),
        }
    }

    pub fn spawn(
        &mut self,
        conf: configuration::ServiceConfig,
    ) -> std::io::Result<()> {
        let addresses = self.figure_out_bunch_of_addresses(
            conf.connect_addr.clone(),
            conf.instances_count,
        )?;

        for address in addresses.iter() {
            let (tx, rx) = tokio::sync::mpsc::channel(100);
            let mut service = Service::new(&conf, address, rx).unwrap();
            self.services.push(tx);

            // Start service executing
            tokio::spawn(async move {
                // TODO: What should I do with this?
                let service_exited = service.wait().await;
            });
        }

        Ok(())
    }

    // pub async fn start(&mut self) {
    //     let mut rolling_update_rx = self.rolling_update_tx.clone();
    //     for service in self.services.iter_mut() {
    //         tokio::spawn(async move {
    //             service.watch(rolling_update_rx).await;
    //         });
    //     }
    // }

    // pub fn request_rolling_update(&self) {
    //     let _ = self.rolling_update_tx.try_send(());
    // }

    /// Get bunch of available network addresses
    fn figure_out_bunch_of_addresses(
        &self,
        start_from: ConnectAddr,
        count: u16,
    ) -> std::io::Result<Vec<ConnectAddr>> {
        match start_from {
            ConnectAddr::Unix(path) => {
                let sockets = get_sockets_paths(&self.name, count, &path)?;

                Ok(sockets
                    .into_iter()
                    .map(|path| ConnectAddr::Unix(path))
                    .collect())
            }
            ConnectAddr::Tcp { addr, port } => {
                let mut available_addresses = Vec::new();
                let mut current_port = port;

                for _ in 0..count {
                    while !is_port_available(addr, current_port) {
                        current_port += 1;
                    }

                    available_addresses.push(ConnectAddr::Tcp {
                        addr: addr.clone(),
                        port: current_port,
                    });

                    current_port += 1;
                }

                Ok(available_addresses)
            }
        }
    }
}

impl Drop for ServiceBunch {
    fn drop(&mut self) {
        // Iterate over the services and send a signal to stop each one
        for service in self.services.iter() {
            // Ignore any send errors, as the receiver may have already been dropped
            let _ = service.send(());
        }
    }
}
// pub struct MainStack(Vec<ServiceBunch>);

// impl MainStack {
// fn spawn(conf: configuration::Configuration) {
// for service_config in conf.services.iter() {
//     let mut service = match Service::new(service_config) {
//         Ok(s) => Some(s),
//         Err(e) => None,
//     };
//
//     if let Some(service) = service {
//         // services.push(service);
//         tokio::spawn(async move {
//             match service.get_output().await {
//                 Some(stdout) => {}
//                 None => todo!(),
//             }
//         });
//     }
// }
// }
// }

/// Check if a specific TCP port is available on a given IP address
fn is_port_available(addr: std::net::IpAddr, port: u16) -> bool {
    if let Ok(listener) = std::net::TcpListener::bind((addr, port)) {
        drop(listener);
        true
    } else {
        false
    }
}

/// Get pack of paths to required count of unix sockets for certain service.
fn get_sockets_paths(
    service_name: &str,
    sockets_count: u16,
    unix_socket_dir: &Path,
) -> std::io::Result<Vec<PathBuf>> {
    let mut sock_indices = Path::read_dir(unix_socket_dir)?
        .flatten()
        .filter_map(|f| {
            f.file_name().into_string().ok().and_then(|f| {
                if let Some(stripped) = f.strip_prefix("sock") {
                    Some(stripped.parse::<u16>().ok())
                } else {
                    None
                }
            })
        })
        .flatten()
        .collect::<Vec<_>>();

    for _ in 0..sockets_count {
        let min = find_min_not_occupied(sock_indices.clone());
        sock_indices.push(min);
    }

    Ok(sock_indices
        .into_iter()
        .map(|idx| {
            unix_socket_dir.join(format!("{}-socket-{}", service_name, idx))
        })
        .collect())
}

fn find_min_not_occupied(mut numbers: Vec<u16>) -> u16 {
    numbers.sort();

    let mut min_not_occupied: u16 = 1;
    for &num in &numbers {
        if num > min_not_occupied {
            break;
        }
        min_not_occupied += 1;
    }

    min_not_occupied
}

#[cfg(test)]
mod service_tests {
    use super::*;
    use tokio_test::task::spawn;

    #[tokio::test]
    async fn wait_success() {
        let (_, term_rx) = mpsc::channel(1);
        let process = Command::new("echo")
            .arg("Hello, World!")
            .spawn()
            .expect("Failed to spawn process");

        let service = Service { process, term_rx };
        let wait_handle = spawn(service.wait());
        let result = wait_handle.await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn wait_failure() {
        let (_term_tx, term_rx) = mpsc::channel(1);
        let process = Command::new("false")
            .spawn()
            .expect("Failed to spawn process");

        let service = Service { process, term_rx };
        let wait_handle = spawn(service.wait());
        let result = wait_handle.await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn terminate_success() {
        let (_, term_rx) = mpsc::channel(1);
        let process = Command::new("sleep")
            .arg("1")
            .spawn()
            .expect("Failed to spawn process");

        let service = Service { process, term_rx };

        let result = service.terminate();

        assert!(result.is_ok());
    }
}

#[cfg(test)]
mod service_bunch_tests {
    use super::*;

    #[test]
    fn test_figure_out_bunch_of_addresses_unix() {
        let start_from = ConnectAddr::Unix(PathBuf::from("test_app/sockets"));
        let bunch = ServiceBunch {
            name: "test".to_string(),
            services: Vec::new(),
        };
        let count = 3;
        let addresses = bunch
            .figure_out_bunch_of_addresses(start_from, count)
            .unwrap();

        assert_eq!(addresses.len(), count as usize);
        for address in addresses {
            match address {
                ConnectAddr::Unix(path) => {
                    assert_eq!(path.starts_with("test_app/sockets"), true);
                }
                _ => panic!("Expected Unix address, but got TCP address"),
            }
        }
    }

    #[test]
    fn test_figure_out_bunch_of_addresses_tcp() {
        let start_from = ConnectAddr::Tcp {
            addr: "127.0.0.1".parse().unwrap(),
            port: 8000,
        };
        let bunch = ServiceBunch {
            name: "test".to_string(),
            services: Vec::new(),
        };
        let count = 3;
        let addresses = bunch
            .figure_out_bunch_of_addresses(start_from, count)
            .unwrap();

        assert_eq!(addresses.len(), count as usize);
        let mut port = 8000;
        for address in addresses {
            match address {
                ConnectAddr::Tcp { addr, port: p } => {
                    assert_eq!(
                        addr,
                        "127.0.0.1".parse::<std::net::IpAddr>().unwrap()
                    );
                    assert_eq!(p, port);
                    port += 1;
                }
                _ => panic!("Expected TCP address, but got Unix address"),
            }
        }
    }

    #[test]
    fn test_is_port_available() {
        let addr = "127.0.0.1".parse().unwrap();
        let port = 50000;
        let is_available = is_port_available(addr, port);
        assert_eq!(is_available, true);
    }

    #[test]
    fn test_get_socket_path() {
        let unix_socket_path = std::path::Path::new("test_app/sockets");
        let socket_path =
            get_sockets_paths("test", 5, &unix_socket_path).unwrap();
        for path in socket_path.iter() {
            assert_eq!(path.starts_with("test_app/sockets"), true);
        }
    }

    #[test]
    fn test_find_min_not_occupied() {
        let numbers = vec![1, 2, 4, 5, 6];
        let min = find_min_not_occupied(numbers);
        assert_eq!(min, 3);
    }
}
