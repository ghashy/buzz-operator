use std::path::Path;

use tokio::sync::mpsc::{Receiver, Sender};
use tokio::task::JoinSet;

use crate::configuration::Configuration;
use crate::fs_watcher::FileSystemWatcher;
use crate::service::service_bunch::message::Message;
use crate::service::service_bunch::{ServiceBunch, ServiceBunchError};
use crate::service::Service;

struct BunchConnection {
    name: String,
    rx: Receiver<Message>,
    tx: Sender<Message>,
}

pub struct BunchController {
    fs_watcher: FileSystemWatcher,
    bunch_connections: Vec<BunchConnection>,
    bunches: Vec<ServiceBunch>,
    bunches_join: JoinSet<Result<(), ServiceBunchError>>,
    rx_join: JoinSet<()>,
    config: Configuration,
}

impl BunchController {
    pub fn new(config: Configuration) -> BunchController {
        let mut bunches = Vec::new();
        let mut bunch_connections = Vec::new();
        for service_config in config.services.clone().into_iter() {
            let (tx, controller_rx) = tokio::sync::mpsc::channel(100);
            let (controller_tx, rx) = tokio::sync::mpsc::channel(100);
            let name = service_config.name.clone();
            let service_bunch =
                ServiceBunch::new(service_config, controller_rx, controller_tx);
            bunches.push(service_bunch);
            bunch_connections.push(BunchConnection { name, rx, tx });
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
            rx_join: JoinSet::new(),
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
        for mut connection in self.bunch_connections.drain(..).into_iter() {
            self.rx_join.spawn(async move {
                match connection.rx.recv().await {
                    Some(message) => match message {
                        // Call script add new
                        Message::ServiceSpawned(addr) => todo!(),
                        // Call script add new, remove old
                        Message::ServiceReplaced { old, new } => todo!(),
                        // Call script remove all old, add all new
                        Message::UpdateFail(_) => todo!(),
                        // Call script remove all addresses
                        Message::ServiceDespawned(_) => todo!(),
                        _ => unreachable!(),
                    },
                    None => todo!(),
                }
            });
        }
        loop {
            tokio::select! {
                bunch_join = self.bunches_join.join_next() => {
                    match bunch_join {
                    Some(Ok(Ok(()))) => todo!(),
                    Some(Ok(Err(bunch_err))) => todo!(),
                    Some(Err(join_err)) => todo!(),
                    None => todo!(),
                    }
                }
                rx_join = self.rx_join.join_next() => {
                    match rx_join {
                        Some(Ok(())) => todo!(),
                        Some(Err(join_err)) => todo!(),
                        None => todo!(),
                    }
                }
            }
        }
    }
}
