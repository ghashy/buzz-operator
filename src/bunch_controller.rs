use std::path::Path;

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

pub struct BunchController {
    fs_watcher: FileSystemWatcher,
    bunch_connections: Vec<BunchConnection>,
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
        for mut bunch in self.bunches.drain(..).into_iter() {
            match bunch.run() {
                Ok(_) => {}
                Err(e) => tracing::error!("Can't run bunch, {}", e),
            }
            self.bunches_join
                .spawn(async move { bunch.wait_on().await });
        }
        // TODO we shoud track all current addresses
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
                                    self.run_script(Some(vec![addr]), None).await;
                                }
                                Message::ServiceReplaced { old, new } => {
                                    self.run_script(Some(vec![new]), Some(vec![old])).await;
                                },
                                Message::UpdateFail { old, new} => {
                                    self.run_script(Some(new), Some(old)).await;
                                },
                                Message::ServiceDespawned(old) => {
                                    self.run_script(None, Some(old)).await;
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
                        Some(Ok(event)) => {
                            let paths = event.paths
                            .iter()
                            .map(|path| path.to_string_lossy())
                            .collect::<String>();
                            println!("FS EVENT: {}", paths);
                        },
                        Some(Err(e)) => {
                            tracing::error!("{}", e);
                        }
                        // Filesystem watcher will not exit
                        None => unreachable!(),
                    }
                }
            }
        }
    }

    async fn run_script(
        &self,
        new: Option<Vec<ConnectAddr>>,
        old: Option<Vec<ConnectAddr>>,
    ) -> bool {
        todo!()
    }
}
