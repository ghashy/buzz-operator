use std::mem::swap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use tokio::select;
use tokio::sync::mpsc::{self, Receiver};
use tokio::task::JoinSet;

use crate::configuration::ServiceConfig;
use crate::connect_addr::ConnectAddr;

use super::service_unit::{ProcessID, ServiceUnit, ServiceUnitError};
use super::unit_connection::UnitConnection;
use super::Service;

use self::error::ServiceBunchError;
use self::message::Message;
use self::state_machine::StateBox;

mod error;
pub mod message;
mod state_machine;

pub struct ServiceBunch {
    /// Service's configuration
    config: ServiceConfig,
    /// This bunch should have some connection to every process.
    /// In our case, we have `Vec` with connections to our processes.
    stable_connections: Vec<UnitConnection>,
    new_connections: Vec<UnitConnection>,
    join_set: JoinSet<Result<(), ServiceUnitError>>,
    state: StateBox,
    controller_connection: Receiver<Message>,

    failure_count: u16,
    last_check_time: Instant,
}

impl ServiceBunch {
    pub fn new(
        config: ServiceConfig,
        controller_connection: Receiver<Message>,
    ) -> ServiceBunch {
        let capacity = config.instances_count as usize;
        ServiceBunch {
            config,
            stable_connections: Vec::with_capacity(capacity),
            new_connections: Vec::new(),
            join_set: JoinSet::new(),
            state: StateBox::new(),
            controller_connection,
            failure_count: 0,
            last_check_time: Instant::now(),
        }
    }

    fn is_stable(&self, pid: ProcessID) -> bool {
        match self.new_connections.iter().find(|c| c.get_pid() == pid) {
            Some(s) => false,
            None => true,
        }
    }

    fn failure_add(&mut self) -> u16 {
        self.failure_count += 1;
        self.failure_count
    }

    fn failures_reset(&mut self) {
        self.last_check_time = Instant::now();
        self.failure_count = 0;
    }

    /// If failure happened with stable service version, limit taken from the
    /// service config, and if it happened with unstable service, limit is 1.
    fn is_limit_exceeded(&mut self, pid: ProcessID) -> bool {
        // Check connection is it stable or not
        let Some(connection) =
            self.stable_connections.iter().find(|c| c.get_pid() == pid)
        else {
            tracing::warn!("Was checking failures, pid not found: {}", pid);
            return false;
        };

        // Limit to 1 for unstable
        if self.is_stable(connection.get_pid()) {
            return true;
        }

        // Calculate the failures within the last minute
        let failures_count = self.failure_add();

        if self.last_check_time.elapsed() > Duration::from_secs(60) {
            tracing::warn!(
                "ServiceUnit failure found, pid: {}, 60 sec timer started",
                pid
            );
            self.failures_reset();
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
        use ServiceUnitError::*;

        match serv_err {
            ExitError { id, err } => {
                tracing::error!("Pid {} exited with error: {}", id, err);
                if self.is_limit_exceeded(id) {
                    if self.is_stable(id) {
                        self.terminate();
                    } else {
                        self.state.update(state_machine::Event::UpdatingFailed);
                    }
                } else {
                    // Restart service
                    self.stable_connections.retain(|c| c.get_pid() == id);
                    let address = self
                        .get_single_address()
                        .expect("Can't get single address");
                    let (term_tx, pid) = self
                        .spawn_unit_service(&address)
                        .expect("Can't spawn unit service");
                    self.stable_connections.push(UnitConnection::new(
                        term_tx,
                        pid,
                        address.clone(),
                    ));
                }
            }
            _ => {}
        }
    }

    fn terminate(&mut self) {
        self.state.update(state_machine::Event::StopRequest);
        self.stable_connections.iter().for_each(|service| {
            // Ignore any send errors, as the receiver may have already been dropped
            let _ = service.terminate();
        });
        self.stable_connections.clear();
    }

    /*
        HINT: What this fn does:
        1. Run new async task which will spawn new process and exit after
        timeout.
        2. After timeout despawn one stable process.
        3. Do all again until all processes will be replaced.
    */
    async fn start_update(&mut self) -> Result<(), ServiceBunchError> {
        self.state.update(state_machine::Event::UpdateRequest);

        while !self.stable_connections.is_empty()
            && self.state.is(state_machine::State::Updating)
        {
            let address = self.get_single_address()?;

            let (term_tx, pid) = match self.spawn_unit_service(&address) {
                Ok(value) => value,
                Err(value) => return value,
            };

            self.new_connections.push(UnitConnection::new(
                term_tx,
                pid,
                address.clone(),
            ));

            tokio::time::sleep(std::time::Duration::from_secs(10)).await;

            let connection = self.stable_connections.pop().unwrap();
            connection.terminate().await.unwrap();

            if !self.state.is(state_machine::State::Updating) {
                for process in self.new_connections.iter() {
                    let _ = process.terminate().await;
                }
                self.new_connections.clear();
                return Err(ServiceBunchError::UpdateIterrupted);
            }
        }

        // On this point stable_connections should be empty
        swap(&mut self.stable_connections, &mut self.new_connections);
        self.state.update(state_machine::Event::ServicesEstablished);
        Ok(())
    }

    fn get_single_address(&mut self) -> Result<ConnectAddr, ServiceBunchError> {
        let mut addressses = find_available_addresses(
            &self.config.name,
            &self.config.connect_addr,
            1,
        )
        .map_err(|e| ServiceBunchError::FailedCreateAddress(e))?;
        let address = addressses.pop().unwrap();
        Ok(address)
    }

    fn spawn_unit_service(
        &mut self,
        address: &ConnectAddr,
    ) -> Result<(mpsc::Sender<()>, u32), Result<(), ServiceBunchError>> {
        let (term_tx, term_rx) = tokio::sync::mpsc::channel(100);
        let mut service =
            ServiceUnit::new(&self.config, address, term_rx).unwrap();
        let pid = match service.run() {
            Ok(pid) => pid,
            Err(e) => {
                tracing::error!("Failed to start ServiceUnit with: {}", e);
                self.state.update(state_machine::Event::StopRequest);
                let _ = self.terminate();
                return Err(Err(ServiceBunchError::FailedToStart));
            }
        };
        self.join_set.spawn(async move { service.wait_on().await });
        Ok((term_tx, pid))
    }
}

#[async_trait]
impl Service<(), ServiceBunchError> for ServiceBunch {
    type Output = ();

    async fn wait_on(&mut self) -> Result<(), ServiceBunchError> {
        // Wait on child processes execution
        loop {
            select! {
                join = self.join_set.join_next() => {
                    // Some service finished
                    match join {
                        Some(Ok(Ok(()))) => {
                            tracing::info!("Some unit service exited wit 0 code")
                        }
                        Some(Ok(Err(serv_err))) => {
                            self.handle_service_unit_error(serv_err);
                        }
                        // Some join error
                        Some(Err(e)) => {
                            let _ = self.terminate();
                            return Err(ServiceBunchError::JoinError(e))
                        },
                        // HINT: This is a normal loop exit point
                        None => return Ok(()),
                    }
                }
                notify = self.controller_connection.recv() => {
                    match notify {
                        Some(message) => {
                            match message {
                                message::Message::StartUpdate => {
                                    match self.start_update().await {
                                        Ok(_) => {
                                        },
                                        Err(e) => tracing::error!("{}", e),
                                    }
                                }
                                message:: Message::Shutdown => {
                                    let _ = self.terminate();
                                    return Ok(())
                                }
                            }
                        }
                        None => {
                            let _ = self.terminate();
                            return Err(ServiceBunchError::ControllerConnectionDropped)
                        }
                    }
                }
            }
        }
    }

    fn run(&mut self) -> Result<Self::Output, ServiceBunchError> {
        // Get all addresses for underlying services
        let addresses = find_available_addresses(
            &self.config.name,
            &self.config.connect_addr,
            self.config.instances_count,
        )
        .map_err(|e| ServiceBunchError::FailedCreateAddress(e))?;

        for address in addresses.iter() {
            let (term_tx, pid) = match self.spawn_unit_service(address) {
                Ok(value) => value,
                Err(value) => return value,
            };

            self.stable_connections.push(UnitConnection::new(
                term_tx,
                pid,
                address.clone(),
            ));
        }
        self.state.update(state_machine::Event::ServicesEstablished);
        Ok(())
    }
}

/// Get bunch of available network addresses
fn find_available_addresses(
    name: &str,
    addr: &ConnectAddr,
    count: u16,
) -> std::io::Result<Vec<ConnectAddr>> {
    match addr {
        ConnectAddr::Unix(path) => {
            let sockets = get_sockets_paths(name, count, &path)?;

            Ok(sockets
                .into_iter()
                .map(|path| ConnectAddr::Unix(path))
                .collect())
        }
        ConnectAddr::Tcp { addr, port } => {
            let mut available_addresses = Vec::new();
            let mut current_port = *port;

            for _ in 0..count {
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
mod test_pack1 {
    use super::*;

    #[test]
    fn test_find_available_addresses_unix() {
        let config = ServiceConfig::create_test_config();
        let (config, addresses) = prepare_addresses(config);

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
    fn test_find_available_addresses_tcp() {
        let config = ServiceConfig {
            connect_addr: ConnectAddr::Tcp {
                addr: "127.0.0.1".parse().unwrap(),
                port: 8000,
            },
            ..ServiceConfig::create_test_config()
        };
        let (config, addresses) = prepare_addresses(config);

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

    fn prepare_addresses(
        config: ServiceConfig,
    ) -> (ServiceConfig, Vec<ConnectAddr>) {
        let (_tx, controller_connection) = tokio::sync::mpsc::channel(100);
        let bunch = ServiceBunch {
            config: config.clone(),
            stable_connections: Vec::new(),
            join_set: JoinSet::new(),
            state: StateBox::new(),
            controller_connection,
            new_connections: Vec::new(),
            failure_count: 0,
            last_check_time: Instant::now(),
        };
        let addresses = find_available_addresses(
            &bunch.config.name,
            &bunch.config.connect_addr,
            bunch.config.instances_count,
        )
        .unwrap();
        (config, addresses)
    }
}

// TODO: add new tests
#[cfg(test)]
mod test_pack2 {
    use super::*;

    #[tokio::test]
    async fn test1() {}
}
