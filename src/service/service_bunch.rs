use std::path::{Path, PathBuf};
use std::time::Duration;

use async_trait::async_trait;
use tokio::task::JoinSet;

use crate::configuration::ServiceConfig;
use crate::connect_addr::ConnectAddr;

use super::service_unit::{
    ProcessID, ServiceUnit, ServiceUnitError, TermSignal,
};
use super::unit_connection::UnitConnection;
use super::Service;

#[derive(Debug)]
pub enum ServiceBunchError {
    FailedCreateAddress(std::io::Error),
    JoinError(tokio::task::JoinError),
    FailedToStart,
}

impl std::error::Error for ServiceBunchError {}

impl std::fmt::Display for ServiceBunchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ServiceBunchError::FailedCreateAddress(e) => {
                f.write_fmt(format_args!("{}", e))
            }
            ServiceBunchError::JoinError(e) => {
                f.write_fmt(format_args!("{}", e))
            }
            ServiceBunchError::FailedToStart => {
                f.write_fmt(format_args!("Failed to start"))
            }
        }
    }
}

#[derive(Clone, Default)]
enum BunchMode {
    #[default]
    Running,
    Updating,
    Stopping,
}

pub struct ServiceBunch {
    /// Service's configuration
    config: ServiceConfig,
    /// This bunch should have some connection to every process.
    /// In our case, we have `Vec` with connections to our processes.
    unit_connections: Vec<UnitConnection>,
    join_set: JoinSet<Result<(), ServiceUnitError>>,
    bunch_mode: BunchMode,
}

impl ServiceBunch {
    pub fn new(config: ServiceConfig) -> ServiceBunch {
        ServiceBunch {
            config,
            unit_connections: Vec::new(),
            join_set: JoinSet::new(),
            bunch_mode: Default::default(),
        }
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

    fn is_stable(&self, pid: ProcessID) -> bool {
        match self.unit_connections.iter().find(|a| a.get_pid() == pid) {
            Some(s) => s.is_stable(),
            None => false,
        }
    }

    /// If failure happened with stable service version, limit taken from the
    /// service config, and if it happened with unstable service, limit is 1.
    fn is_limit_exceeded(&mut self, pid: ProcessID) -> bool {
        // Wait until failure signal
        let Some(connection) = self
            .unit_connections
            .iter_mut()
            .find(|a| a.get_pid() == pid)
        else {
            tracing::warn!("Was checking failures, pid not found: {}", pid);
            return false;
        };

        // Limit to 1 for unstable
        if !connection.is_stable() {
            return true;
        }

        // Calculate the failures within the last minute
        let failures_count = connection.failure_add();

        if connection.last_check_time.elapsed() > Duration::from_secs(60) {
            tracing::warn!(
                "ServiceUnit failure found, pid: {}, 60 sec timer started",
                pid
            );
            connection.failures_reset();
            return false;
        }

        let limit = self.config.fails_limit;
        if failures_count > limit {
            tracing::warn!("Got {} failures in ServiceUnit with pid: {}, limit is {}, stopping", failures_count, pid, limit);
            true
        } else {
            tracing::warn!(
                "Got {} failures in ServiceUnit with pid: {}",
                failures_count,
                pid
            );
            false
        }
    }

    fn handle_service_unit_error(&mut self, serv_err: ServiceUnitError) {
        use BunchMode::*;
        use ServiceUnitError::*;

        match serv_err {
            ExitError { id, err } => match self.bunch_mode {
                Running => {
                    if self.is_limit_exceeded(id) {
                        // stop service bunch
                    } else {
                        // restart service
                    }
                }
                Updating => {
                    if self.is_limit_exceeded(id) {
                        if self.is_stable(id) {
                            // stop service bunch
                        } else {
                            // stop updating
                        }
                    }
                }
                Stopping => {
                    tracing::info!("{}, are stopping, so ignore", err);
                }
            },
            _ => {}
        }
    }
}

#[async_trait]
impl Service<(), ServiceBunchError> for ServiceBunch {
    type Output = ();

    // TODO: implement task queue, it should perform rolling updates, and
    // check unit services count.
    async fn wait_on(&mut self) -> Result<(), ServiceBunchError> {
        // Wait on child processes execution
        loop {
            match self.join_set.join_next().await {
                Some(Ok(Ok(()))) => {
                    tracing::info!("Some unit service exited wit 0 code")
                }
                // Some task has finished with error
                Some(Ok(Err(serv_err))) => {
                    tracing::error!(
                        "ServiceUnit exited with error: {}",
                        serv_err
                    );
                    self.handle_service_unit_error(serv_err);
                }
                // Some join error
                Some(Err(e)) => return Err(ServiceBunchError::JoinError(e)),
                // All tasks are finished
                None => return Ok(()),
            }
        }
    }

    fn run(&mut self) -> Result<Self::Output, ServiceBunchError> {
        // Get all addresses for underlying services
        let addresses = self
            .figure_out_bunch_of_addresses()
            .map_err(|e| ServiceBunchError::FailedCreateAddress(e))?;

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
                    let _ = self.try_terminate();
                    return Err(ServiceBunchError::FailedToStart);
                }
            };
            // Add to join
            self.join_set.spawn(async move { service.wait_on().await });

            self.unit_connections
                .push(UnitConnection::new(term_tx, pid));
        }

        Ok(())
    }

    fn try_terminate(&mut self) -> Result<(), ServiceBunchError> {
        self.unit_connections.iter().for_each(|service| {
            // Ignore any send errors, as the receiver may have already been dropped
            let _ = service.terminate();
        });
        Ok(())
    }
}

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
        let bunch = ServiceBunch {
            config: config.clone(),
            unit_connections: Vec::new(),
            join_set: JoinSet::new(),
            bunch_mode: BunchMode::default(),
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

        let bunch = ServiceBunch {
            config: config.clone(),
            unit_connections: Vec::new(),
            join_set: JoinSet::new(),
            bunch_mode: BunchMode::default(),
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
