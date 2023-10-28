use std::collections::HashSet;
use std::path::{Path, PathBuf};

use tokio::sync::mpsc::{Receiver, Sender};
use tokio::task::JoinSet;

use crate::configuration::Configuration;
use crate::connect_addr::ConnectAddr;
use crate::fs_watcher::FileSystemWatcher;
use crate::service::service_bunch::message::Message;
use crate::service::service_bunch::{ServiceBunch, ServiceBunchError};
use crate::service::Service;

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
/// ```
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
            let service_bunch = ServiceBunch::new(
                service_config,
                controller_rx,
                controller_tx.clone(),
            );
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
                Ok(addrs) => {
                    self.run_update_script(Some(addrs), None);
                }
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
                                Message::ServiceSpawned(addr) => {
                                    self.run_update_script(Some(vec![addr]), None);
                                }
                                Message::ServiceDespawned(old) => {
                                    self.run_update_script(None, Some(old));
                                },
                                Message::ServiceReplaced { old, new } => {
                                    self.run_update_script(Some(vec![new]), Some(vec![old]));
                                },
                                Message::UpdateFail { old, new} => {
                                    self.run_update_script(Some(new), Some(old));
                                },
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
        if let Some(service) = self.config.services.iter().find(|service| {
            service.app_dir.join(&service.new_exec_name).eq(path)
        }) {
            // If there are connection to this service
            if let Some(connection) = self
                .bunch_connections
                .iter()
                .find(|conn| conn.name == service.name)
            {
                tracing::info!(
                    "Trying to send update request to {} service",
                    service.name
                );
                // Send a message
                if let Err(e) = connection.tx.send(Message::StartUpdate).await {
                    tracing::error!("Failed to send message from BunchController to {} service: {}", service.name, e);
                }
            }
        }
    }

    fn run_update_script(
        &self,
        to_add: Option<Vec<ConnectAddr>>,
        to_remove: Option<Vec<ConnectAddr>>,
    ) {
        tracing::info!("UPDATE SCRIPT RAN");
    }

    async fn run_health_check_script() -> bool {
        todo!()
    }
}
