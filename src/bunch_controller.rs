use std::collections::HashSet;
use std::path::{Path, PathBuf};

use tokio::process::Command;
use tokio::sync::mpsc::{Receiver, Sender};
use tokio::task::JoinSet;

use crate::configuration::Configuration;
use crate::connect_addr::ConnectAddr;
use crate::fs_watcher::FileSystemWatcher;
use crate::service::service_bunch::message::Message;
use crate::service::service_bunch::{ServiceBunch, ServiceBunchError};
use crate::service::Service;

// TODO: implement healthchecks
// Create separate module, and give tx, and addresses, and config params with
// healthchecks. It will check and send

struct BunchConnection {
    name: String,
    tx: Sender<Message>,
}

/// High level abstraction above [`ServiceBunch`]. Runs all bunchs, listens for
/// filesystem notifications, generates rolling update requests, provides single
/// interface to all backend bunches.
///
/// For communication with bunches we create (tx, rx) pairs with
/// [`tokio::sync::mpsc::channel`] and share them
/// between `BunchController` and every of controlled bunch.
/// ```text
///                   +--------------------+
///                   |  BunchController   |
///                   +--------------------+
///                   |  tx, tx, tx, tx    |
///                -->|        rx          |<--
///               /   +-------↑------------+  /
///              /            |              /
///             /             |             /
///            /        (tx, rx)           /
///           /           tx.clone()      /
///         (tx, rx)     /     \          (tx, rx)
///               |     /       \               |
///        |-------    /         \       |-------
/// +------v----------v--+ +------v------v------+
/// |   ServiceBunch 1   | |   ServiceBunch 2   |
/// +--------------------+ +--------------------+
/// |       tx, rx       | |      tx, rx        |
/// +--------------------+ +--------------------+
/// ```
pub struct BunchController {
    /// Handle to notification system
    fs_watcher: FileSystemWatcher,
    bunch_connections: Vec<BunchConnection>,
    // WARN: after `run()` call bunches are emtpy. We move all to the `bunches_join` execution.
    bunches: Vec<ServiceBunch>,
    bunches_join: JoinSet<Result<(), ServiceBunchError>>,
    message_receiver: Receiver<Message>,
    config: Configuration,
}

impl BunchController {
    pub fn new(config: Configuration) -> BunchController {
        let mut bunches = Vec::new();
        let mut bunch_connections = Vec::new();
        let (controller_tx, message_receiver) = tokio::sync::mpsc::channel(100);
        for service_config in config.services.clone().into_iter() {
            let (tx, controller_rx) = tokio::sync::mpsc::channel(100);
            let name = service_config.name.clone();
            let service_bunch =
                ServiceBunch::new(service_config, controller_rx, controller_tx.clone());
            bunches.push(service_bunch);
            bunch_connections.push(BunchConnection { name, tx });
        }
        let fs_watcher = FileSystemWatcher::new(
            &bunches
                .iter()
                .map(|bunch| bunch.get_config().app_dir.as_path())
                .collect::<Vec<&Path>>()[..],
        );
        BunchController {
            fs_watcher,
            bunch_connections,
            bunches,
            bunches_join: JoinSet::new(),
            message_receiver,
            config,
        }
    }

    pub async fn run(&mut self) {
        // Take ownership over `bunches` vec.
        let bunches = std::mem::replace(&mut self.bunches, Vec::new());
        // Spawn all bunches tasks in the `bunches_join`
        for mut bunch in bunches.into_iter() {
            match bunch.run() {
                Ok(_) => (),
                Err(e) => tracing::error!("Can't run bunch, {}", e),
            }
            self.bunches_join
                .spawn(async move { bunch.wait_on().await });
        }

        loop {
            tokio::select! {
                // Wait bunches executing
                bunch_join = self.bunches_join.join_next() => {
                    match bunch_join {
                    Some(Ok(Ok(()))) => tracing::info!("Some bunch exited with code 0"),
                    Some(Ok(Err(bunch_err))) => tracing::error!("Some bunch exited with error: {}", bunch_err),
                    Some(Err(join_err)) => tracing::error!("Join error happened: {}", join_err),
                    // All bunches finished
                    None => return,
                    }
                }
                // Wait bunches messages
                rx_join = self.message_receiver.recv() => {
                    match rx_join {
                        Some(message) => {
                            match message {
                                Message::UnitSpawned(name, addr) => {
                                    self.run_update_script(&name, Some(vec![addr]), None).await;
                                }
                                Message::UnitsSpawned(name, addr) => {
                                    self.run_update_script(&name,Some(addr), None).await;
                                }
                                Message::UnitDespawned(name ,old) => {
                                    self.run_update_script(&name, None, Some(vec![old])).await;
                                }
                                Message::UnitsDespawned(name, old) => {
                                    self.run_update_script(&name,None, Some(old)).await;
                                }
                                Message::UnitReplaced { name, old, new } => {
                                    self.run_update_script(&name, Some(vec![new]), Some(vec![old])).await;
                                }
                                Message::UpdateFail {name, old, new} => {
                                    self.run_update_script(&name, Some(new), Some(old)).await;
                                }
                                Message::UpdateFinished(name) => {
                                    self.replace_stable_exec(&name);
                                }
                                _ => unreachable!(),
                            }
                        },
                        // All bunches exited
                        None => {
                            return;
                        },
                    }
                }
                // Wait fs watcher events
                watch = self.fs_watcher.receiver.recv() => {
                    match watch {
                        // Handle path event
                        Some(event) => {
                            for path in event.paths.iter().collect::<HashSet<_>>() {
                                tracing::info!("File created: {}", path.display());
                                self.send_update_message(path).await;
                            }
                        },
                        // Filesystem watcher will not exit
                        None => unreachable!(),
                    }
                }
            }
        }
    }

    async fn send_update_message(&self, path: &PathBuf) {
        // If there are service which have this app_dir path and new_exec_name
        if let Some(service) = self
            .config
            .services
            .iter()
            .find(|service| service.app_dir.join(&service.new_exec_name).eq(path))
        {
            // If there are connection to this service
            if let Some(connection) = self
                .bunch_connections
                .iter()
                .find(|conn| conn.name == service.name)
            {
                tracing::info!("Trying to send update request to {} service", service.name);
                // Send a message
                if let Err(e) = connection.tx.send(Message::StartUpdate).await {
                    tracing::error!(
                        "Failed to send message from BunchController to {} service: {}",
                        service.name,
                        e
                    );
                }
            }
        }
    }

    async fn run_update_script(
        &self,
        name: &str,
        to_add: Option<Vec<ConnectAddr>>,
        to_remove: Option<Vec<ConnectAddr>>,
    ) {
        let Some(service) = self
            .config
            .services
            .iter()
            .find(|service| service.name.eq(name))
        else {
            return;
        };

        let Some(update_script) = &service.update_script else {
            return;
        };

        let command = &mut Command::new(update_script);

        match (to_add, to_remove) {
            (None, None) => return,
            (None, Some(rm)) => {
                pass_args(rm.iter().collect(), command, "--rm");
            }
            (Some(add), None) => {
                pass_args(add.iter().collect(), command, "--add");
            }
            (Some(add), Some(rm)) => {
                let common_addresses: Vec<_> =
                    add.iter().filter(|address| rm.contains(address)).collect();

                let add_filtered = add
                    .iter()
                    .filter(|address| !common_addresses.contains(address))
                    .collect::<Vec<_>>();

                let remove_filtered: Vec<_> = rm
                    .iter()
                    .filter(|address| !common_addresses.contains(address))
                    .collect();

                pass_args(add_filtered, command, "--add");
                pass_args(remove_filtered, command, "--rm");
            }
        }
        match command.spawn() {
            Ok(mut child) => {
                tracing::info!("Run update script for service {}", service.name);
                match child.wait().await {
                    Ok(status) => tracing::info!("Update script exited with status: {:?}", status),
                    Err(e) => tracing::error!(
                        "Failed to wait update script for service {}: {}",
                        service.name,
                        e
                    ),
                }
            }
            Err(e) => tracing::error!(
                "Failed to run update script for service {}: {}",
                service.name,
                e
            ),
        }
    }

    async fn run_health_check_script() -> bool {
        todo!()
    }

    fn replace_stable_exec(&self, name: &str) {
        if let Some(service) = self
            .config
            .services
            .iter()
            .find(|service| service.name.eq(name))
        {
            let stable_name = service.app_dir.join(&service.stable_exec_name);
            match std::fs::remove_file(&stable_name) {
                Ok(()) => {
                    tracing::info!("Service {} stable was deleted!", service.name);
                    match std::fs::rename(
                        service.app_dir.join(&service.new_exec_name),
                        &stable_name,
                    ) {
                        Ok(()) => {
                            tracing::info!("Service {} new exec was stabilized!", service.name)
                        }
                        Err(e) => tracing::error!(
                            "Failed to stabilize {} serivce new exec: {}",
                            service.name,
                            e
                        ),
                    }
                }
                Err(e) => tracing::error!(
                    "Failed to delete {} service's new exec file: {}",
                    service.name,
                    e
                ),
            }
        }
    }
}

fn pass_args(addrs: Vec<&ConnectAddr>, command: &mut Command, first_arg: &str) {
    if !addrs.is_empty() {
        let mut args = vec![first_arg.to_string()];
        args.extend(addrs.iter().map(|addr| addr.to_string()));
        command.args(args);
    }
}
