//! Sends message to [`crate::bunch_controller::BunchController`] in functions:
//! - [`ServiceBunch::terminate`]
//! - [`ServiceBunch::handle_service_unit_error`]
//! - [`ServiceBunch::start_update`]
//! - [`<ServiceBunch as Service>::run`]

use std::mem::swap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use tokio::sync::mpsc::{self, Receiver, Sender};
use tokio::task::JoinSet;

use crate::configuration::ServiceConfig;
use crate::connect_addr::ConnectAddr;

use super::service_unit::{ProcessID, ServiceUnit, ServiceUnitError, UnitVersion};
use super::unit_connection::UnitConnection;
use super::Service;

pub use self::error::ServiceBunchError;
use self::message::Message;
use self::state_machine::StateBox;

mod error;
pub mod message;
mod state_machine;

/// Type for managing some instances of the same service.
///
/// `ServiceBunch` can run one or more instances of a service, restart
/// these instances on failure, count failures and when certain threshold of
/// failures count is exceeded, it terminates itself.
/// Also `ServiceBunch` has possibility to perform rolling updates, when it
/// gets a signal over the [`self.controller_rx`](ServiceBunch::controller_rx)
/// channel.
pub struct ServiceBunch {
    /// Service's configuration
    config: ServiceConfig,
    /// This bunch should have some connection to every process.
    /// In our case, we have `Vec` with connections to our processes.
    stable_connections: Vec<UnitConnection>,
    /// There we store all new connections during the rolling update.
    new_connections: Vec<UnitConnection>,
    /// It provides us with `awaiting in one place` interface for all
    /// `ServiceUnit`s.
    join_set: JoinSet<Result<(), ServiceUnitError>>,
    /// Stores internal simple `finite state machine`.
    state: StateBox,

    /// Communication channel with higher (in program hierachy) type. Input.
    controller_rx: Receiver<Message>,
    /// Communication channel with higher (in program hierachy) type. Output.
    controller_tx: Sender<Message>,

    failure_count: u16,
    last_check_time: Instant,
}

impl ServiceBunch {
    pub fn new(
        config: ServiceConfig,
        controller_rx: Receiver<Message>,
        controller_tx: Sender<Message>,
    ) -> ServiceBunch {
        // Make sure log directory exists
        if !config.get_log_dir().exists() {
            std::fs::DirBuilder::new()
                .create(config.get_log_dir())
                .expect(&format!(
                    "Failed to create '{}' service log directory!",
                    config.name
                ));
            tracing::info!("Created log directory: {}", config.get_log_dir().display());
        }

        let capacity = config.instances_count as usize;
        ServiceBunch {
            config,
            stable_connections: Vec::with_capacity(capacity),
            new_connections: Vec::new(),
            join_set: JoinSet::new(),
            state: StateBox::new(),
            controller_rx,
            controller_tx,
            failure_count: 0,
            last_check_time: Instant::now(),
        }
    }

    pub fn get_config(&self) -> &ServiceConfig {
        &self.config
    }

    /// Returns `false`, if found process with given `pid` among
    /// the `new_connections`. Otherwise, `true`.
    fn is_stable_pid(&self, pid: ProcessID) -> bool {
        match self.new_connections.iter().find(|c| c.get_pid() == pid) {
            Some(_) => false,
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

    /// If failure happens with a stable service version, we take the limit
    /// value from the service config, but if failure happens with
    /// the `new version` service (during rolling update), limit is set to `1`.
    fn reg_new_failure_and_is_limit_exceeded(&mut self, pid: ProcessID) -> bool {
        // Check connection is it stable or not
        let Some(connection) = self.stable_connections.iter().find(|c| c.get_pid() == pid) else {
            tracing::warn!("Was checking failures, pid not found: {}", pid);
            return false;
        };

        // Limit to 1 for unstable
        if !self.is_stable_pid(connection.get_pid()) {
            return true;
        }

        // Calculate the failures within the last minute
        let failures_count = self.failure_add();

        if self.last_check_time.elapsed() > Duration::from_secs(60) {
            tracing::warn!(
                "ServiceUnit failure found, name {}, 60 sec timer started",
                self.config.name
            );
            self.failures_reset();
            return false;
        }

        let limit = self.config.fails_limit;
        if failures_count > limit {
            tracing::warn!(
                "Got {} failures in ServiceUnit with name {}, limit is {}, stopping",
                failures_count,
                self.config.name,
                limit
            );
            true
        } else {
            tracing::warn!(
                "Got {} failures in ServiceUnit with name {}",
                failures_count,
                self.config.name
            );
            false
        }
    }

    // FIXME: review this method, is it too complicated? Remove duplicates
    /// In the underlying service [`Service::run`] function,
    /// [`ServiceUnitError`] can happen. In this case we handle it here.
    /// We do three things here:
    /// 1. If failure limit is not exceeded, restart underlying [`ServiceUnit`].
    /// 2. If rolling update is performing at the moment, and failed service is
    /// from the new version, stop the rolling update.
    /// 3. If failure limit is exceeded, stop all underlying services using
    /// [`ServiceBunch::terminate`].
    fn handle_service_unit_error(&mut self, serv_err: ServiceUnitError) {
        use ServiceUnitError::*;

        match serv_err {
            ExitError { id, err } => {
                tracing::error!("Pid {} exited with error: {}", id, err);
                if self.reg_new_failure_and_is_limit_exceeded(id) {
                    if self.is_stable_pid(id) {
                        self.terminate();
                    } else {
                        self.state.update(state_machine::Event::UpdatingFailed);
                    }
                } else {
                    // Restart service
                    let address = self.get_single_address().expect("Can't get single address");
                    let (term_tx, pid) = self
                        .spawn_unit_service(&address, UnitVersion::Stable)
                        .expect("Can't spawn unit service");
                    self.stable_connections.push(UnitConnection::new(
                        term_tx,
                        pid,
                        address.clone(),
                    ));
                    // Send message to controller
                    if let Some(lost_connection) =
                        take_from_vec_with_pid(&mut self.stable_connections, id)
                    {
                        let tx = self.controller_tx.clone();
                        let name = self.config.name.clone();
                        tokio::spawn(async move {
                            if let Err(e) = tx
                                .send(Message::UnitReplaced {
                                    name,
                                    old: lost_connection.addr().clone(),
                                    new: address,
                                })
                                .await
                            {
                                tracing::error!("{}", e);
                            }
                        });
                    }
                }
            }
            HealthCheckFailed(addr) => {
                // If updating state, stop updating
                if let Some(_) = self.new_connections.iter().find(|c| c.addr().eq(&addr)) {
                    self.state.update(state_machine::Event::UpdatingFailed);
                    // If not updating
                } else if let Some(conn) =
                    self.stable_connections.iter().find(|c| c.addr().eq(&addr))
                {
                    let conn = conn.clone();
                    // Count failures
                    if self.reg_new_failure_and_is_limit_exceeded(conn.get_pid()) {
                        // Terminate entire bunch
                        self.terminate();
                    } else {
                        // Terminate one instance, and start new one
                        let name = self.config.name.clone();
                        tracing::error!("Healthcheck failed, terminating unit service {}", name);
                        let available_addr =
                            find_available_addresses(&name, &self.config.connect_addr, 1)
                                .expect("Failed to get available ConnectAddr!");
                        let (term_tx, pid) = self
                            .spawn_unit_service(&available_addr[0], UnitVersion::Stable)
                            .expect("Can't spawn unit service!");

                        self.stable_connections.push(UnitConnection::new(
                            term_tx,
                            pid,
                            available_addr[0].clone(),
                        ));

                        // Send message to controller
                        if let Some(lost_connection) =
                            take_from_vec_with_pid(&mut self.stable_connections, conn.get_pid())
                        {
                            let tx = self.controller_tx.clone();
                            let name = self.config.name.clone();
                            tokio::spawn(async move {
                                if let Err(e) = tx
                                    .send(Message::UnitReplaced {
                                        name,
                                        old: lost_connection.addr().clone(),
                                        new: available_addr.into_iter().next().unwrap(),
                                    })
                                    .await
                                {
                                    tracing::error!("{}", e);
                                }
                            });
                        }

                        tokio::spawn(async move {
                            if let Err(e) = conn.terminate().await {
                                tracing::error!(
                                    "Failed to send termination message to {} service , error: {}",
                                    name,
                                    e
                                );
                            }
                        });
                    }
                }
            }
            _ => {}
        }
    }

    /// Stop all underlying `ServiceUnit` instances, `self.join_set` will exit
    /// at the [`JoinSet::join_next()`] in the `run` function inside [`select!`]
    /// macro, and then this `ServiceBunch` will return control to the parent
    /// type.
    fn terminate(&mut self) {
        self.state.update(state_machine::Event::StopRequest);
        // Clear `self.stable_connections`
        let connections = std::mem::replace(&mut self.stable_connections, Vec::new());

        let tx = self.controller_tx.clone();
        let name = self.config.name.clone();
        // Do stuff in async context
        tokio::spawn(async move {
            // Addresses for sending to controller
            let addrs = connections.iter().map(|conn| conn.addr().clone()).collect();
            // Despawn all child processes
            for service in connections.into_iter() {
                // Ignore any send errors, as the receiver may have already been dropped
                let _ = service.terminate().await;
            }

            // Send message
            if let Err(e) = tx.send(Message::UnitsDespawned(name, addrs)).await {
                tracing::error!("{}", e);
            }
        });
    }

    /// How it works:
    /// 1. Run a new async task which will spawn new process and exit after
    /// timeout.
    /// 2. Next, if it is still in the `State::Update`, despawn one stable
    /// process.
    /// 3. Do all again until all children processes will be replaced.
    async fn start_update(&mut self) -> Result<(), ServiceBunchError> {
        self.state.update(state_machine::Event::UpdateRequest);

        tracing::info!("Starting rolling update in service `{}`", self.config.name,);
        while !self.stable_connections.is_empty() && self.state.is(state_machine::State::Updating) {
            let address = self.get_single_address()?;

            let (term_tx, pid) = match self.spawn_unit_service(&address, UnitVersion::New) {
                Ok(value) => value,
                Err(e) => return Err(e),
            };

            self.new_connections
                .push(UnitConnection::new(term_tx, pid, address.clone()));

            // Send message to controller
            let tx = self.controller_tx.clone();
            let send_addr = address.clone();
            let name = self.config.name.clone();
            tokio::spawn(async move {
                if let Err(e) = tx.send(Message::UnitSpawned(name, send_addr)).await {
                    tracing::error!("{}", e);
                }
            });

            // TODO: compute update time with config's setting in mind.
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;

            tracing::info!("Pop one stable connection from store and terminate it");
            let connection = self.stable_connections.pop().unwrap();
            connection.terminate().await.unwrap();

            // Send message to controller
            let tx = self.controller_tx.clone();
            let name = self.config.name.clone();
            tokio::spawn(async move {
                if let Err(e) = tx.send(Message::UnitDespawned(name, address)).await {
                    tracing::error!("{}", e);
                }
            });

            if !self.state.is(state_machine::State::Updating) {
                tracing::warn!("Exited from `Updating` state, terminating all new processes");
                for process in self.new_connections.iter() {
                    let _ = process.terminate().await;
                }
                self.new_connections.clear();
                return Err(ServiceBunchError::UpdateIterrupted);
            }
        }

        // At this point stable_connections should be empty
        swap(&mut self.stable_connections, &mut self.new_connections);
        self.state.update(state_machine::Event::ServicesEstablished);
        if let Err(e) = self
            .controller_tx
            .send(Message::UpdateFinished(self.config.name.clone()))
            .await
        {
            tracing::error!("{}", e);
        }
        self.state.update(state_machine::Event::UpdatingFinished);
        Ok(())
    }

    /// Provides our app with correct and working
    /// [`ConnectAddr`](crate::connect_addr::ConnectAddr). It can select
    /// free tcp port, as well as find unix socket path.
    fn get_single_address(&mut self) -> Result<ConnectAddr, ServiceBunchError> {
        let mut addressses =
            find_available_addresses(&self.config.name, &self.config.connect_addr, 1)
                .map_err(|e| ServiceBunchError::FailedCreateAddress(e))?;
        let address = addressses.pop().unwrap();
        Ok(address)
    }

    /// Spawn single [`ServiceUnit`]. If it can't spawn service, it will stop
    /// this `ServiceBunch`.
    fn spawn_unit_service(
        &mut self,
        address: &ConnectAddr,
        version: UnitVersion,
    ) -> Result<(mpsc::Sender<()>, ProcessID), ServiceBunchError> {
        let (term_tx, term_rx) = tokio::sync::mpsc::channel(100);
        let mut service = ServiceUnit::new(&self.config, address, term_rx, version).unwrap();
        let pid = match service.run() {
            Ok(pid) => pid,
            Err(e) => {
                tracing::error!("Failed to start ServiceUnit with: {}", e);
                match self.state.current() {
                    // If failed on start, terminate self
                    state_machine::State::Starting => {
                        self.state.update(state_machine::Event::StopRequest);
                        let _ = self.terminate();
                        return Err(ServiceBunchError::FailedToStart);
                    }
                    // If failed on update, stop update
                    state_machine::State::Updating => {
                        self.state.update(state_machine::Event::UpdatingFailed);
                        return Err(ServiceBunchError::FailedToStart);
                    }
                    _ => {
                        unreachable!()
                    }
                }
            }
        };
        self.join_set.spawn(async move { service.wait_on().await });
        tracing::info!("New unit service spawned with pid {}", pid);
        Ok((term_tx, pid))
    }
}

#[async_trait]
impl Service<ServiceBunchError> for ServiceBunch {
    type Output = Vec<ConnectAddr>;

    async fn wait_on(&mut self) -> Result<(), ServiceBunchError> {
        // Wait on child processes execution
        loop {
            tokio::select! {
                join = self.join_set.join_next() => {
                    // Some service finished
                    match join {
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

                        // Exit with 0 only in rolling update mode
                        Some(Ok(Ok(()))) => {
                            tracing::info!("SOME PROCESS EXITED WITH 0 CODE");
                        }
                    }
                }
                notify = self.controller_rx.recv() => {
                    match notify {
                        Some(message) => {
                            match message {
                                message::Message::StartUpdate => {
                                    match self.start_update().await {
                                        Ok(_) => {},
                                        Err(e) => tracing::error!("{}", e),
                                    }
                                }
                                message::Message::HealthCheckFailed(addr) => {
                                    self.handle_service_unit_error(ServiceUnitError::HealthCheckFailed(addr));
                                }
                                message:: Message::Shutdown => {
                                    let _ = self.terminate();
                                    return Ok(())
                                }
                                _ => unreachable!()
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
            let (term_tx, pid) = match self.spawn_unit_service(address, UnitVersion::Stable) {
                Ok(value) => value,
                Err(e) => return Err(e),
            };

            self.stable_connections
                .push(UnitConnection::new(term_tx, pid, address.clone()));
        }

        // Send message to controller
        let tx = self.controller_tx.clone();
        let name = self.config.name.clone();
        let addrs = self
            .stable_connections
            .iter()
            .map(|conn| conn.addr().clone())
            .collect();
        tokio::spawn(async move {
            // Addresses for sending to controller
            if let Err(e) = tx.send(Message::UnitsSpawned(name, addrs)).await {
                tracing::error!("{}", e);
            }
        });

        self.state.update(state_machine::Event::ServicesEstablished);
        Ok(self
            .stable_connections
            .iter()
            .map(|conn| conn.addr().clone())
            .collect())
    }
}

fn take_from_vec_with_pid(
    vector: &mut Vec<UnitConnection>,
    pid: ProcessID,
) -> Option<UnitConnection> {
    match vector.iter().position(|conn| conn.get_pid() == pid) {
        Some(pos) => Some(vector.remove(pos)),
        None => None,
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
        .map(|idx| unix_socket_dir.join(format!("{}-socket-{}", service_name, idx)))
        .collect())
}

/// Finds minimal available number in range 0..∞
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
        let (config, addresses) = prepare_bunch_and_addresses(config);

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
        let (config, addresses) = prepare_bunch_and_addresses(config);

        assert_eq!(addresses.len(), config.instances_count as usize);
        let mut port = 8000;
        for address in addresses {
            match address {
                ConnectAddr::Tcp { addr, port: p } => {
                    assert_eq!(addr, "127.0.0.1".parse::<std::net::IpAddr>().unwrap());
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
        let socket_path = get_sockets_paths("test", 5, &unix_socket_path).unwrap();
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

    fn prepare_bunch_and_addresses(config: ServiceConfig) -> (ServiceConfig, Vec<ConnectAddr>) {
        let (_tx, controller_rx) = tokio::sync::mpsc::channel(100);
        let (controller_tx, _rx) = tokio::sync::mpsc::channel(100);
        let bunch = ServiceBunch {
            config: config.clone(),
            stable_connections: Vec::new(),
            join_set: JoinSet::new(),
            state: StateBox::new(),
            controller_rx,
            controller_tx,
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
    // use super::*;

    #[tokio::test]
    async fn test1() {}
}
