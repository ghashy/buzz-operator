use std::time::Duration;

use sman::fs_watcher::FileSystemWatcher;
use sman::service::Service;
use tracing_subscriber::prelude::__tracing_subscriber_SubscriberExt;

use sman::service::service_bunch::ServiceBunch;
use sman::{configuration, fs_watcher};

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    init_tracing_subscriber();

    let services_configuration =
        configuration::Configuration::load_configuration().unwrap();

    let config = services_configuration.services[0].clone();
    let fs_watcher = FileSystemWatcher::new(&config.app_dir);
    let mut bunch = ServiceBunch::new(config, fs_watcher);

    bunch.run().unwrap();
    match bunch.wait_on().await {
        Ok(_) => {}
        Err(e) => println!("{:}", e),
    }

    // let term = tokio::spawn(async {
    //     tokio::time::sleep(Duration::from_secs(5)).await;
    //     return;
    // });

    // let (a, b) = tokio::join!(term, bunch.run_and_wait());
}

fn init_tracing_subscriber() {
    let app_conf = configuration::AppConfig::load_configuration().unwrap();

    // Make sure log directory exists
    if !app_conf.log_dir.exists() {
        std::fs::DirBuilder::new()
            .create(&app_conf.log_dir)
            .expect("Failed to create sman's log directory!");
    }

    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .with_level(true)
        .finish()
        .with(
            tracing_subscriber::fmt::layer().with_writer(
                std::fs::File::options()
                    .create(true)
                    .append(true)
                    .write(true)
                    .open(app_conf.log_dir.join("sman.log"))
                    .expect("Can't open log file!"),
            ),
        );
    tracing::subscriber::set_global_default(subscriber)
        .expect("Failed to set up tracing");
}
// let mut services = Vec::new();

// for service in services.iter_mut() {
//     service.terminate().await;
// }

// let conf1 = &conf.services[1];

// let (tx, rx) = std::sync::mpsc::channel();

// Automatically select the best implementation for your platform.
// let mut watcher = notify::recommended_watcher(move |res| match res {
//     Ok(event) => tx.send(event).unwrap(),
//     Err(e) => println!("watch error: {:?}", e),
// })
// .unwrap();

// Add a path to be watched. All files and directories at that path and
// below will be monitored for changes.
// watcher
//     .watch(conf.app_dir.as_path(), RecursiveMode::Recursive)
//     .unwrap();

// Start an infinite loop to receive and handle the events
// loop {
//     match rx.recv() {
//         Ok(event) => println!("Event: {:?}", event),
//         Err(e) => println!("Error: {:?}", e),
//     }
// }

// std::thread::sleep(std::time::Duration::from_secs(100));

// println!("Terminating all processes...");
// for process in pids.iter_mut() {
//     match process.kill() {
//         Ok(_) => {
//             println!("Succesfully terminated process {}", process.id())
//         }
//         Err(e) => {
//             eprintln!("Failed to terminate process {}: {}", process.id(), e)
//         }
//     }
// }
