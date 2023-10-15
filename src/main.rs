use std::path::Path;
use std::process::Command;
use std::process::Stdio;

use notify::RecursiveMode;
use notify::Watcher;

// ───── Current Crate Imports ────────────────────────────────────────────── //

use sman::configuration;

// ───── Body ─────────────────────────────────────────────────────────────── //

fn main() {
    let conf = configuration::Settings::load_configuration().unwrap();

    // std::fs::DirBuilder::new()
    //     .create(conf.app_dir.as_path().join("log/"))
    //     .expect("Faile to create log directory!");

    // let mut pids = vec![];
    // for i in 0..conf.instances_count {
    //     match Command::new(format!("./{}", &conf.stable_exec))
    //         .current_dir(&conf.app_dir)
    //         .stdout(Stdio::from(
    //             std::fs::File::create(
    //                 conf.app_dir
    //                     .as_path()
    //                     .join(format!("log/instance_{}.log", i)),
    //             )
    //             .expect("Failed to create log file!"),
    //         ))
    //         .spawn()
    //     {
    //         Ok(c) => {
    //             println!("Successfully spawned instance {}", i);
    //             pids.push(c);
    //         }
    //         Err(e) => {
    //             eprintln!("Error: {}", e);
    //             eprintln!("Terminating app...");
    //             return;
    //         }
    //     }
    // }

    let (tx, rx) = std::sync::mpsc::channel();

    // Automatically select the best implementation for your platform.
    let mut watcher = notify::recommended_watcher(move |res| match res {
        Ok(event) => tx.send(event).unwrap(),
        Err(e) => println!("watch error: {:?}", e),
    })
    .unwrap();

    // Add a path to be watched. All files and directories at that path and
    // below will be monitored for changes.
    watcher
        .watch(conf.app_dir.as_path(), RecursiveMode::Recursive)
        .unwrap();

    // Start an infinite loop to receive and handle the events
    loop {
        match rx.recv() {
            Ok(event) => println!("Event: {:?}", event),
            Err(e) => println!("Error: {:?}", e),
        }
    }

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
}
