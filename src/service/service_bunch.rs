use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use tokio::sync::mpsc;
use tokio::task::JoinSet;

use crate::configuration::ServiceConfig;
use crate::connect_addr::ConnectAddr;

use super::service_unit::{
    ProcessID, ServiceUnit, ServiceUnitError, TerminateSignal,
};
use super::Service;

#[derive(Debug)]
pub enum ServiceBunchError {
    FailedCreateAddress(std::io::Error),
}

impl std::error::Error for ServiceBunchError {}

impl std::fmt::Display for ServiceBunchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ServiceBunchError::FailedCreateAddress(e) => {
                f.write_fmt(format_args!("{}", e))
            }
        }
    }
}

#[derive(Clone)]
struct UnitConnection {
    termination_sender: mpsc::Sender<TerminateSignal>,
    pid: ProcessID,
}

impl UnitConnection {
    fn new(sender: mpsc::Sender<TerminateSignal>, pid: ProcessID) -> Self {
        UnitConnection {
            termination_sender: sender,
            pid,
        }
    }
}

pub struct ServiceBunch {
    /// Service's configuration
    config: ServiceConfig,
    /// This bunch should have some connection to every process.
    /// In our case, we have `Vec` with connections to our processes.
    unit_connections: Vec<UnitConnection>,
    join_set: JoinSet<Result<(), ServiceUnitError>>,
    /// In this field we store current amount of failures
    failure_count: Arc<AtomicU16>,
    /// We need to somehow send messages to `failure_checker` tokio task,
    /// and for this purpose we use `Sender<()>` again :)
    failure_check_signal_tx: mpsc::Sender<ProcessID>,
    /// This receiver will be given to `failure_checker` tokio task
    failure_check_signal_rx: Option<mpsc::Receiver<ProcessID>>,
}

impl ServiceBunch {
    pub fn new(config: ServiceConfig) -> ServiceBunch {
        let (signal_tx, signal_rx) = tokio::sync::mpsc::channel(100);
        ServiceBunch {
            config,
            unit_connections: Vec::new(),
            join_set: JoinSet::new(),
            failure_count: Arc::new(AtomicU16::new(0)),
            failure_check_signal_tx: signal_tx,
            failure_check_signal_rx: Some(signal_rx),
        }
    }

    /// This checker do not need to count for failures, we implemented this
    /// logic in `run_and_wait` function
    fn start_failure_checker(&mut self) {
        // Start a separate Tokio task to check for failures
        let failure_counter = self.failure_count.clone();
        let mut signal_rx = self.failure_check_signal_rx.take().unwrap();
        let limit = self.config.fails_limit;
        let services_conn = self.unit_connections.clone();
        tokio::spawn(async move {
            let mut last_check_time = Instant::now();
            // Wait until failure signal
            while let Some(_) = signal_rx.recv().await {
                // Calculate the failures within the last minute
                let failures = failure_counter.load(Ordering::Relaxed);

                if last_check_time.elapsed() > Duration::from_secs(60) {
                    // Reset the failure count
                    last_check_time = Instant::now();
                    // Reset timer
                    failure_counter.store(1, Ordering::Relaxed);
                    continue;
                }

                if failures > limit {
                    // TODO: implement failures handle
                    // Terminate all services
                    for service in services_conn.iter() {
                        service
                            .termination_sender
                            .send(TerminateSignal::Terminate)
                            .await;
                    }
                    return; // Exit the function
                }
            }
        });
    }

    /// Get bunch of available network addresses
    fn figure_out_bunch_of_addresses(
        &self,
    ) -> std::io::Result<Vec<ConnectAddr>> {
        match &self.config.connect_addr {
            ConnectAddr::Unix(path) => {
                let sockets = get_sockets_paths(
                    &self.config.name,
                    self.config.instances_count,
                    &path,
                )?;

                Ok(sockets
                    .into_iter()
                    .map(|path| ConnectAddr::Unix(path))
                    .collect())
            }
            ConnectAddr::Tcp { addr, port } => {
                let mut available_addresses = Vec::new();
                let mut current_port = *port;

                for _ in 0..self.config.instances_count {
                    while !is_port_available(*addr, current_port) {
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

#[async_trait]
impl Service<(), ServiceBunchError> for ServiceBunch {
    type Output = ();

    async fn wait_on(&mut self) -> Result<(), ServiceBunchError> {
        match self.join_set.join_next().await {
            Some(_) => todo!(),
            None => todo!(),
        }
    }

    fn run(&mut self) -> Result<Self::Output, ServiceBunchError> {
        // Get all addresses for underlying services
        let addresses = self
            .figure_out_bunch_of_addresses()
            .map_err(|e| ServiceBunchError::FailedCreateAddress(e))?;

        self.start_failure_checker();

        for address in addresses.iter() {
            // This is channel to communicate with each service process
            let (term_tx, term_rx) = tokio::sync::mpsc::channel(100);
            let mut service =
                ServiceUnit::new(&self.config, address, term_rx).unwrap();

            // Run service
            let pid = match service.run() {
                Ok(pid) => pid,
                Err(e) => {
                    tracing::error!("Failed to start ServiceUnit with: {}", e);
                    continue;
                }
            };
            // Add to join
            self.join_set.spawn(async move { service.wait_on().await });

            self.unit_connections
                .push(UnitConnection::new(term_tx, pid));

            let failure_counter = self.failure_count.clone();
            let signal_counter = self.failure_check_signal_tx.clone();
        }

        Ok(())
    }

    fn try_terminate(&mut self) -> Result<(), ServiceBunchError> {
        self.unit_connections.iter().for_each(|service| {
            // Ignore any send errors, as the receiver may have already been dropped
            let _ = service.termination_sender.send(TerminateSignal::Terminate);
        });
        Ok(())
    }
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
// pub struct MainStack(Vec<ServiceBunch>);

// impl MainStack {
// fn spawn(conf: configuration::Configuration) {
// for service_config in conf.services.iter() {
//     let mut service = match ServiceUnit::new(service_config) {
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
mod tests {
    use super::*;

    #[test]
    fn test_figure_out_bunch_of_addresses_unix() {
        let config = ServiceConfig::create_test_config();
        let (signal_tx, signal_rx) = tokio::sync::mpsc::channel(100);
        let bunch = ServiceBunch {
            config: config.clone(),
            unit_connections: Vec::new(),
            failure_count: Arc::new(AtomicU16::new(0)),
            failure_check_signal_tx: signal_tx,
            failure_check_signal_rx: Some(signal_rx),
            join_set: JoinSet::new(),
        };
        let addresses = bunch.figure_out_bunch_of_addresses().unwrap();

        assert_eq!(addresses.len(), config.instances_count as usize);
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
        let config = ServiceConfig {
            connect_addr: ConnectAddr::Tcp {
                addr: "127.0.0.1".parse().unwrap(),
                port: 8000,
            },
            ..ServiceConfig::create_test_config()
        };

        let (signal_tx, signal_rx) = tokio::sync::mpsc::channel(100);
        let bunch = ServiceBunch {
            config: config.clone(),
            unit_connections: Vec::new(),
            failure_count: Arc::new(AtomicU16::new(0)),
            failure_check_signal_tx: signal_tx,
            failure_check_signal_rx: Some(signal_rx),
            join_set: JoinSet::new(),
        };
        let addresses = bunch.figure_out_bunch_of_addresses().unwrap();

        assert_eq!(addresses.len(), config.instances_count as usize);
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
