use std::collections::HashSet;
use std::io::Write;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use tokio::process::Command;
use tokio::sync::mpsc::{Receiver, Sender};
use tokio::task::JoinSet;

use crate::configuration::{Configuration, ServiceConfig};
use crate::connect_addr::ConnectAddr;
use crate::fs_watcher::FileSystemWatcher;
use crate::service::service_bunch::message::Message;
use crate::service::service_bunch::{ServiceBunch, ServiceBunchError};
use crate::service::Service;

#[derive(Debug)]
pub enum Commands {
    StartServices(Vec<String>),
    StopServices(Vec<String>),
    RestartServices(Vec<String>),
    SendStatusToStream(Vec<String>),
    StartAll,
    StopAll,
    RestartAll,
    Exit,
}

/// `UnixStream` is used to send feedback about command execution
#[derive(Debug)]
pub struct ControllerCommand {
    pub command: Commands,
    pub feedback: UnixStream,
}

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
    /// We store it here, because without it Receiver will drop connection
    controller_tx: Sender<Message>,
    commands_receiver: Receiver<ControllerCommand>,
    config: Configuration,
    _empty_handler: Option<tokio::sync::oneshot::Sender<()>>,
}

impl BunchController {
    pub fn new(config: Configuration) -> (BunchController, Sender<ControllerCommand>) {
        let bunches: Vec<ServiceBunch> = Vec::new();
        let bunch_connections: Vec<BunchConnection> = Vec::new();
        let fs_watcher = FileSystemWatcher::new(&[]);
        // Connection to bunches
        let (controller_tx, message_receiver) = tokio::sync::mpsc::channel(100);
        // Connection to app
        let (app_tx, commands_receiver) = tokio::sync::mpsc::channel(100);

        // For JoinSet not to close
        let (_empty_handler, rx) = tokio::sync::oneshot::channel::<()>();
        let mut joinset = JoinSet::new();
        joinset.spawn(async move {
            let _ = rx.await;
            Ok(())
        });

        (
            BunchController {
                fs_watcher,
                bunch_connections,
                bunches,
                bunches_join: joinset,
                controller_tx,
                message_receiver,
                commands_receiver,
                config,
                _empty_handler: Some(_empty_handler),
            },
            app_tx,
        )
    }

    pub fn update_config(&mut self, new: Configuration) {
        self.config = new
    }

    async fn run_command(&mut self, mut command: ControllerCommand) {
        tracing::info!("Handle message: {:?}", command);
        match command.command {
            Commands::StartServices(services) => {
                self.spawn(self.filter_service_names(services, &command.feedback))
                    .await
            }
            Commands::StopServices(services) => {
                self.despawn(self.filter_service_names(services, &command.feedback))
            }
            Commands::RestartServices(services) => {
                self.despawn(self.filter_service_names(services.clone(), &command.feedback));
                self.spawn(self.filter_service_names(services, &command.feedback))
                    .await
            }
            Commands::SendStatusToStream(services) => {
                for name in self.filter_service_names(services, &command.feedback) {
                    if let Some(_) = self
                        .bunch_connections
                        .iter()
                        .find(|conn| conn.name.eq(&name))
                    {
                        let _ = command
                            .feedback
                            .write_fmt(format_args!("{} is run!\0", name));
                        command.feedback.flush().unwrap();
                    } else {
                        let _ = command
                            .feedback
                            .write_fmt(format_args!("{} is not run!\0", name));
                        command.feedback.flush().unwrap();
                    }
                }
            }
            Commands::StartAll => {
                self.spawn(
                    self.config
                        .services
                        .clone()
                        .into_iter()
                        .map(|service| service.name)
                        .collect(),
                )
                .await
            }
            Commands::StopAll => self.despawn(
                self.config
                    .services
                    .clone()
                    .into_iter()
                    .map(|service| service.name)
                    .collect(),
            ),
            Commands::RestartAll => {
                self.despawn(
                    self.config
                        .services
                        .clone()
                        .into_iter()
                        .map(|service| service.name)
                        .collect(),
                );
                self.spawn(
                    self.config
                        .services
                        .clone()
                        .into_iter()
                        .map(|service| service.name)
                        .collect(),
                )
                .await
            }
            Commands::Exit => {
                self.despawn(
                    self.config
                        .services
                        .clone()
                        .into_iter()
                        .map(|service| service.name)
                        .collect(),
                );
                let _ = self._empty_handler.take().unwrap().send(());
            }
        }
    }

    fn filter_service_names(&self, names: Vec<String>, feedback: &UnixStream) -> Vec<String> {
        tracing::info!("Trying to spawn: {:?}", &names);
        names
            .into_iter()
            .filter(|name| {
                self.config
                    .services
                    .iter()
                    .find(|service_conf| {
                        let name = name.clone();
                        let mut feedback = feedback.try_clone().unwrap();
                        if service_conf.name.eq(&name) {
                            tokio::spawn(async move {
                                let _ = feedback.write_fmt(format_args!(
                                    "Failed to run {}, no entry on configuration file!\0",
                                    name
                                ));
                                feedback.flush().unwrap();
                            });
                            true
                        } else {
                            tokio::spawn(async move {
                                let _ = feedback.write_fmt(format_args!(
                                    "Found {} service in configuration\0",
                                    name
                                ));
                                feedback.flush().unwrap();
                            });
                            false
                        }
                    })
                    .is_some()
            })
            .collect()
    }

    async fn spawn(&mut self, names: Vec<String>) {
        for service_config in self
            .config
            .services
            .clone()
            .into_iter()
            .filter(|conf| names.contains(&conf.name))
        {
            if self
                .bunch_connections
                .iter()
                .find(|conn| conn.name.eq(&service_config.name))
                .is_some()
            {
                tracing::warn!(
                    "{} already spawned, skip spawn command",
                    service_config.name
                );
            }
            let (tx, controller_rx) = tokio::sync::mpsc::channel(100);
            let name = service_config.name.clone();
            let mut service_bunch =
                ServiceBunch::new(service_config, controller_rx, self.controller_tx.clone());

            match service_bunch.run() {
                Ok(addrs) => {
                    self.start_health_check_tasks(&name, addrs.iter().collect());
                    self.run_update_script(&name, Some(addrs), None).await;
                }
                Err(e) => {
                    tracing::error!("Failed to spawn service {}, error: {}", name, e);
                }
            };
            self.bunches_join
                .spawn(async move { service_bunch.wait_on().await });
            self.bunch_connections.push(BunchConnection { name, tx });
        }
    }

    fn despawn(&mut self, names: Vec<String>) {
        for name in names.into_iter() {
            if let Some(position) = self
                .bunch_connections
                .iter()
                .position(|conn| conn.name.eq(&name))
            {
                let connection = self.bunch_connections.remove(position);
                tokio::spawn(async move {
                    let _ = connection.tx.send(Message::Shutdown).await;
                });
            } else {
                tracing::warn!("{} service is not run, can't despawn it!", name);
            }
        }
    }

    /// Run this controller.
    pub async fn run_and_wait(&mut self) {
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
                // Wait commands
                command = self.commands_receiver.recv() => {
                    if let Some(command) = command {
                        self.run_command(command).await;
                    }
                }
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
                                    self.start_health_check_tasks(&name, vec![&addr]);
                                    self.run_update_script(&name, Some(vec![addr]), None).await;
                                }
                                Message::UnitsSpawned(name, addr) => {
                                    self.start_health_check_tasks(&name, addr.iter().collect());
                                    self.run_update_script(&name,Some(addr), None).await;
                                }
                                Message::UnitDespawned(name ,old) => {
                                    self.run_update_script(&name, None, Some(vec![old])).await;
                                }
                                Message::UnitsDespawned(name, old) => {
                                    self.run_update_script(&name,None, Some(old)).await;
                                }
                                Message::UnitReplaced { name, old, new } => {
                                    self.start_health_check_tasks(&name, vec![&new]);
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

    /// Try to send update request to `ServiceBunch` by app_dir path.
    /// By path, because file notification system look for new backend service's
    /// executables in the specified paths.
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

    /// Run specified in configuration file command to update some
    /// configuration (NGINX, for example).
    /// This function will pass args to the command:
    /// --remove ... (list of addresses to remove from)
    /// --add ... (list of addresses to add to)
    async fn run_update_script(
        &self,
        name: &str,
        to_add: Option<Vec<ConnectAddr>>,
        to_remove: Option<Vec<ConnectAddr>>,
    ) {
        let Some(service_config) = self.find_service_config(name) else {
            return;
        };

        let Some(update_exec) = &service_config.update_exec else {
            return;
        };

        let command = &mut Command::new(update_exec);

        command.stdout(Stdio::null());
        command.stderr(Stdio::null());

        match (to_add, to_remove) {
            (None, None) => return,
            (None, Some(rm)) => {
                pass_args(rm.iter().collect(), command, "--remove");
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
                tracing::info!("Run update script for service {}", service_config.name);
                match child.wait().await {
                    Ok(status) => tracing::info!(
                        "Update script exited with {} status",
                        if status.success() { "OK" } else { " ERR " }
                    ),
                    Err(e) => tracing::error!(
                        "Failed to wait update script for service {}: {}",
                        service_config.name,
                        e
                    ),
                }
            }
            Err(e) => tracing::error!(
                "Failed to run update script for service {}: {}",
                service_config.name,
                e
            ),
        }
    }

    /// This function removes stable executable from `app_dir`, and renames new executable to
    /// stable.
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

    /// These tasks will execute until health check fails
    /// If healthcheck exec returns not 0, we think that it is failure, and
    /// we send message `HealthCheckFailed`.
    fn start_health_check_tasks(&self, name: &str, addrs: Vec<&ConnectAddr>) {
        // Find service config by name
        let Some(service_config) = self.find_service_config(name) else {
            return;
        };

        // If exec path is set
        let Some(exec_path) = &service_config.health_check_exec else {
            return;
        };

        // If there are a connection to this service
        if let Some(connection) = self.bunch_connections.iter().find(|conn| conn.name == name) {
            for addr in addrs {
                let addr = addr.clone();
                let addr_str = addr.to_string();
                let connection = connection.tx.clone();
                let exec_path = exec_path.clone();
                let service_name = service_config.name.clone();
                let times_per_hour = service_config.health_check_rate.unwrap_or(1);
                let secs_count = 60 * 60 / times_per_hour as u64;

                tokio::spawn(async move {
                    loop {
                        // Setup command
                        let mut command = Command::new(&exec_path);
                        // Command output to dev/null
                        command.stdout(Stdio::null());
                        command.stderr(Stdio::null());
                        // Pass addr
                        command.arg(&addr_str);

                        // Run command, handle exit status
                        match command.spawn() {
                            Ok(mut child) => match child.wait().await {
                                Ok(status) if !status.success() => {
                                    tracing::warn!(
                                        "Health check failed for {} service, addr: {}",
                                        service_name,
                                        addr_str
                                    );
                                    if let Err(e) = connection
                                        .send(Message::HealthCheckFailed(addr.clone()))
                                        .await
                                    {
                                        tracing::warn!("Failed to send HealthCheckFailed message to {} service: {}", service_name, e);
                                    }
                                    break;
                                }
                                Err(e) => {
                                    tracing::error!(
                                        "Failed to wait exit from health check command ({}) for {} service, error: {}",
                                        exec_path.display(),
                                        service_name,
                                        e
                                    );
                                    break;
                                }
                                _ => {
                                    // tracing::info!(
                                    //     "Health check passed! Service name: {}, addr: {}",
                                    //     service_name,
                                    //     addr_str
                                    // );
                                }
                            },
                            Err(e) => {
                                tracing::error!(
                                    "Failed to spawn health check command ({}), error: {}",
                                    exec_path.display(),
                                    e
                                );
                                break;
                            }
                        }
                        tokio::time::sleep(Duration::from_secs(secs_count)).await;
                    }
                });
            }
        }
    }

    /// Try to find the service config by a name.
    fn find_service_config(&self, name: &str) -> Option<&ServiceConfig> {
        self.config
            .services
            .iter()
            .find(|service| service.name.eq(name))
    }
}

fn pass_args(addrs: Vec<&ConnectAddr>, command: &mut Command, first_arg: &str) {
    if !addrs.is_empty() {
        let mut args = vec![first_arg.to_string()];
        args.extend(addrs.iter().map(|addr| addr.to_string()));
        command.args(args);
    }
}

// use std::collections::HashSet;
// use std::path::{Path, PathBuf};
// use std::process::Stdio;
// use std::time::Duration;

// use tokio::process::Command;
// use tokio::sync::mpsc::{Receiver, Sender};
// use tokio::task::JoinSet;

// use crate::configuration::{Configuration, ServiceConfig};
// use crate::connect_addr::ConnectAddr;
// use crate::fs_watcher::FileSystemWatcher;
// use crate::service::service_bunch::message::Message;
// use crate::service::service_bunch::{ServiceBunch, ServiceBunchError};
// use crate::service::Service;

// struct BunchConnection {
//     name: String,
//     tx: Sender<Message>,
// }

// /// High level abstraction above [`ServiceBunch`]. Runs all bunchs, listens for
// /// filesystem notifications, generates rolling update requests, provides single
// /// interface to all backend bunches.
// ///
// /// For communication with bunches we create (tx, rx) pairs with
// /// [`tokio::sync::mpsc::channel`] and share them
// /// between `BunchController` and every of controlled bunch.
// /// ```text
// ///                   +--------------------+
// ///                   |  BunchController   |
// ///                   +--------------------+
// ///                   |  tx, tx, tx, tx    |
// ///                -->|        rx          |<--
// ///               /   +-------↑------------+  /
// ///              /            |              /
// ///             /             |             /
// ///            /        (tx, rx)           /
// ///           /           tx.clone()      /
// ///         (tx, rx)     /     \          (tx, rx)
// ///               |     /       \               |
// ///        |-------    /         \       |-------
// /// +------v----------v--+ +------v------v------+
// /// |   ServiceBunch 1   | |   ServiceBunch 2   |
// /// +--------------------+ +--------------------+
// /// |       tx, rx       | |      tx, rx        |
// /// +--------------------+ +--------------------+
// /// ```
// pub struct BunchController {
//     /// Handle to notification system
//     fs_watcher: FileSystemWatcher,
//     bunch_connections: Vec<BunchConnection>,
//     // WARN: after `run()` call bunches are emtpy. We move all to the `bunches_join` execution.
//     bunches: Vec<ServiceBunch>,
//     bunches_join: JoinSet<Result<(), ServiceBunchError>>,
//     message_receiver: Receiver<Message>,
//     config: Configuration,
// }

// impl BunchController {
//     pub fn new(config: Configuration) -> BunchController {
//         let mut bunches = Vec::new();
//         let mut bunch_connections = Vec::new();
//         let (controller_tx, message_receiver) = tokio::sync::mpsc::channel(100);
//         for service_config in config.services.clone().into_iter() {
//             let (tx, controller_rx) = tokio::sync::mpsc::channel(100);
//             let name = service_config.name.clone();
//             let service_bunch =
//                 ServiceBunch::new(service_config, controller_rx, controller_tx.clone());
//             bunches.push(service_bunch);
//             bunch_connections.push(BunchConnection { name, tx });
//         }
//         let fs_watcher = FileSystemWatcher::new(
//             &bunches
//                 .iter()
//                 .map(|bunch| bunch.get_config().app_dir.as_path())
//                 .collect::<Vec<&Path>>()[..],
//         );
//         BunchController {
//             fs_watcher,
//             bunch_connections,
//             bunches,
//             bunches_join: JoinSet::new(),
//             message_receiver,
//             config,
//         }
//     }

//     /// Run this controller.
//     pub async fn run_and_wait(&mut self) {
//         // Take ownership over `bunches` vec.
//         let bunches = std::mem::replace(&mut self.bunches, Vec::new());
//         // Spawn all bunches tasks in the `bunches_join`
//         for mut bunch in bunches.into_iter() {
//             match bunch.run() {
//                 Ok(_) => (),
//                 Err(e) => tracing::error!("Can't run bunch, {}", e),
//             }
//             self.bunches_join
//                 .spawn(async move { bunch.wait_on().await });
//         }

//         loop {
//             tokio::select! {
//                 // Wait bunches executing
//                 bunch_join = self.bunches_join.join_next() => {
//                     match bunch_join {
//                     Some(Ok(Ok(()))) => tracing::info!("Some bunch exited with code 0"),
//                     Some(Ok(Err(bunch_err))) => tracing::error!("Some bunch exited with error: {}", bunch_err),
//                     Some(Err(join_err)) => tracing::error!("Join error happened: {}", join_err),
//                     // All bunches finished
//                     None => return,
//                     }
//                 }
//                 // Wait bunches messages
//                 rx_join = self.message_receiver.recv() => {
//                     match rx_join {
//                         Some(message) => {
//                             match message {
//                                 Message::UnitSpawned(name, addr) => {
//                                     self.start_health_check_tasks(&name, vec![&addr]);
//                                     self.run_update_script(&name, Some(vec![addr]), None).await;
//                                 }
//                                 Message::UnitsSpawned(name, addr) => {
//                                     self.start_health_check_tasks(&name, addr.iter().collect());
//                                     self.run_update_script(&name,Some(addr), None).await;
//                                 }
//                                 Message::UnitDespawned(name ,old) => {
//                                     self.run_update_script(&name, None, Some(vec![old])).await;
//                                 }
//                                 Message::UnitsDespawned(name, old) => {
//                                     self.run_update_script(&name,None, Some(old)).await;
//                                 }
//                                 Message::UnitReplaced { name, old, new } => {
//                                     self.start_health_check_tasks(&name, vec![&new]);
//                                     self.run_update_script(&name, Some(vec![new]), Some(vec![old])).await;
//                                 }
//                                 Message::UpdateFail {name, old, new} => {
//                                     self.run_update_script(&name, Some(new), Some(old)).await;
//                                 }
//                                 Message::UpdateFinished(name) => {
//                                     self.replace_stable_exec(&name);
//                                 }
//                                 _ => unreachable!(),
//                             }
//                         },
//                         // All bunches exited
//                         None => {
//                             return;
//                         },
//                     }
//                 }
//                 // Wait fs watcher events
//                 watch = self.fs_watcher.receiver.recv() => {
//                     match watch {
//                         // Handle path event
//                         Some(event) => {
//                             for path in event.paths.iter().collect::<HashSet<_>>() {
//                                 tracing::info!("File created: {}", path.display());
//                                 self.send_update_message(path).await;
//                             }
//                         },
//                         // Filesystem watcher will not exit
//                         None => unreachable!(),
//                     }
//                 }
//             }
//         }
//     }

//     /// Try to send update request to `ServiceBunch` by app_dir path.
//     /// By path, because file notification system look for new backend service's
//     /// executables in the specified paths.
//     async fn send_update_message(&self, path: &PathBuf) {
//         // If there are service which have this app_dir path and new_exec_name
//         if let Some(service) = self
//             .config
//             .services
//             .iter()
//             .find(|service| service.app_dir.join(&service.new_exec_name).eq(path))
//         {
//             // If there are connection to this service
//             if let Some(connection) = self
//                 .bunch_connections
//                 .iter()
//                 .find(|conn| conn.name == service.name)
//             {
//                 tracing::info!("Trying to send update request to {} service", service.name);
//                 // Send a message
//                 if let Err(e) = connection.tx.send(Message::StartUpdate).await {
//                     tracing::error!(
//                         "Failed to send message from BunchController to {} service: {}",
//                         service.name,
//                         e
//                     );
//                 }
//             }
//         }
//     }

//     /// Run specified in configuration file command to update some
//     /// configuration (NGINX, for example).
//     /// This function will pass args to the command:
//     /// --remove ... (list of addresses to remove from)
//     /// --add ... (list of addresses to add to)
//     async fn run_update_script(
//         &self,
//         name: &str,
//         to_add: Option<Vec<ConnectAddr>>,
//         to_remove: Option<Vec<ConnectAddr>>,
//     ) {
//         let Some(service_config) = self.find_service_config(name) else {
//             return;
//         };

//         let Some(update_exec) = &service_config.update_exec else {
//             return;
//         };

//         let command = &mut Command::new(update_exec);

//         command.stdout(Stdio::null());
//         command.stderr(Stdio::null());

//         match (to_add, to_remove) {
//             (None, None) => return,
//             (None, Some(rm)) => {
//                 pass_args(rm.iter().collect(), command, "--remove");
//             }
//             (Some(add), None) => {
//                 pass_args(add.iter().collect(), command, "--add");
//             }
//             (Some(add), Some(rm)) => {
//                 let common_addresses: Vec<_> =
//                     add.iter().filter(|address| rm.contains(address)).collect();

//                 let add_filtered = add
//                     .iter()
//                     .filter(|address| !common_addresses.contains(address))
//                     .collect::<Vec<_>>();

//                 let remove_filtered: Vec<_> = rm
//                     .iter()
//                     .filter(|address| !common_addresses.contains(address))
//                     .collect();

//                 pass_args(add_filtered, command, "--add");
//                 pass_args(remove_filtered, command, "--rm");
//             }
//         }
//         match command.spawn() {
//             Ok(mut child) => {
//                 tracing::info!("Run update script for service {}", service_config.name);
//                 match child.wait().await {
//                     Ok(status) => tracing::info!(
//                         "Update script exited with {} status",
//                         if status.success() { "OK" } else { " ERR " }
//                     ),
//                     Err(e) => tracing::error!(
//                         "Failed to wait update script for service {}: {}",
//                         service_config.name,
//                         e
//                     ),
//                 }
//             }
//             Err(e) => tracing::error!(
//                 "Failed to run update script for service {}: {}",
//                 service_config.name,
//                 e
//             ),
//         }
//     }

//     /// This function removes stable executable from `app_dir`, and renames new executable to
//     /// stable.
//     fn replace_stable_exec(&self, name: &str) {
//         if let Some(service) = self
//             .config
//             .services
//             .iter()
//             .find(|service| service.name.eq(name))
//         {
//             let stable_name = service.app_dir.join(&service.stable_exec_name);
//             match std::fs::remove_file(&stable_name) {
//                 Ok(()) => {
//                     tracing::info!("Service {} stable was deleted!", service.name);
//                     match std::fs::rename(
//                         service.app_dir.join(&service.new_exec_name),
//                         &stable_name,
//                     ) {
//                         Ok(()) => {
//                             tracing::info!("Service {} new exec was stabilized!", service.name)
//                         }
//                         Err(e) => tracing::error!(
//                             "Failed to stabilize {} serivce new exec: {}",
//                             service.name,
//                             e
//                         ),
//                     }
//                 }
//                 Err(e) => tracing::error!(
//                     "Failed to delete {} service's new exec file: {}",
//                     service.name,
//                     e
//                 ),
//             }
//         }
//     }

//     /// These tasks will execute until health check fails
//     /// If healthcheck exec returns not 0, we think that it is failure, and
//     /// we send message `HealthCheckFailed`.
//     fn start_health_check_tasks(&self, name: &str, addrs: Vec<&ConnectAddr>) {
//         // Find service config by name
//         let Some(service_config) = self.find_service_config(name) else {
//             return;
//         };

//         // If exec path is set
//         let Some(exec_path) = &service_config.health_check_exec else {
//             return;
//         };

//         // If there are a connection to this service
//         if let Some(connection) = self.bunch_connections.iter().find(|conn| conn.name == name) {
//             for addr in addrs {
//                 let addr = addr.clone();
//                 let addr_str = addr.to_string();
//                 let connection = connection.tx.clone();
//                 let exec_path = exec_path.clone();
//                 let service_name = service_config.name.clone();
//                 let times_per_hour = service_config.health_check_rate.unwrap_or(1);
//                 let secs_count = 60 * 60 / times_per_hour as u64;

//                 tokio::spawn(async move {
//                     loop {
//                         // Setup command
//                         let mut command = Command::new(&exec_path);
//                         // Command output to dev/null
//                         command.stdout(Stdio::null());
//                         command.stderr(Stdio::null());
//                         // Pass addr
//                         command.arg(&addr_str);

//                         // Run command, handle exit status
//                         match command.spawn() {
//                             Ok(mut child) => match child.wait().await {
//                                 Ok(status) if !status.success() => {
//                                     tracing::warn!(
//                                         "Health check failed for {} service, addr: {}",
//                                         service_name,
//                                         addr_str
//                                     );
//                                     if let Err(e) = connection
//                                         .send(Message::HealthCheckFailed(addr.clone()))
//                                         .await
//                                     {
//                                         tracing::warn!("Failed to send HealthCheckFailed message to {} service: {}", service_name, e);
//                                     }
//                                     break;
//                                 }
//                                 Err(e) => {
//                                     tracing::error!(
//                                         "Failed to wait exit from health check command ({}) for {} service, error: {}",
//                                         exec_path.display(),
//                                         service_name,
//                                         e
//                                     );
//                                     break;
//                                 }
//                                 _ => {
//                                     // tracing::info!(
//                                     //     "Health check passed! Service name: {}, addr: {}",
//                                     //     service_name,
//                                     //     addr_str
//                                     // );
//                                 }
//                             },
//                             Err(e) => {
//                                 tracing::error!(
//                                     "Failed to spawn health check command ({}), error: {}",
//                                     exec_path.display(),
//                                     e
//                                 );
//                                 break;
//                             }
//                         }
//                         tokio::time::sleep(Duration::from_secs(secs_count)).await;
//                     }
//                 });
//             }
//         }
//     }

//     /// Try to find the service config by a name.
//     fn find_service_config(&self, name: &str) -> Option<&ServiceConfig> {
//         self.config
//             .services
//             .iter()
//             .find(|service| service.name.eq(name))
//     }
// }

// fn pass_args(addrs: Vec<&ConnectAddr>, command: &mut Command, first_arg: &str) {
//     if !addrs.is_empty() {
//         let mut args = vec![first_arg.to_string()];
//         args.extend(addrs.iter().map(|addr| addr.to_string()));
//         command.args(args);
//     }
// }
