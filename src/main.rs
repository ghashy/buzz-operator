use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand, ValueEnum};
use serde::{Deserialize, Serialize};
use tokio::io::AsyncReadExt;
use tracing_subscriber::prelude::__tracing_subscriber_SubscriberExt;

use buzzoperator::bunch_controller::{BunchController, ControllerCommand};
use buzzoperator::configuration;

// TODO: add config file verification with -t flag
// TODO: add `reload` possibility
// TODO: implement remove all addresses on termination, and stopping all processes gracefully

/// Unit socket file
const SOCK_FILE: &'static str = "/tmp/buzzoperator.sock";

#[derive(Parser)]
#[command(author, version, about, long_about)]
#[command(propagate_version = true)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
    #[arg(long, short)]
    test_config: bool,

    /// These are for SystemD. Do not use these arguments!
    /// Use instead: systemctl <command> buzzoperator
    #[arg(value_enum, hide = false)]
    daemon_mode: Option<Mode>,
}

#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, ValueEnum)]
enum Mode {
    StartDaemon,
    ReloadDaemon,
}

#[derive(Debug, Subcommand, Serialize, Deserialize)]
enum Command {
    /// Start service with name, specified in the config.
    Start(ServiceName),
    /// Stop service with name, specified in the config.
    Stop(ServiceName),
    /// Restart service with name, specified in the config.
    Restart(ServiceName),
}

#[derive(Debug, Args, Serialize, Deserialize)]
struct ServiceName {
    /// Name of the service.
    name: String,
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> ExitCode {
    // Parse command line arguments
    let cli = Cli::parse();

    // Load configuration
    let config = match configuration::Configuration::load_configuration() {
        Ok(c) => {
            if cli.test_config {
                println!("Configuration is OK!");
                c
            } else {
                c
            }
        }
        Err(e) => {
            eprintln!("Configuration is not OK, error: {}", e);
            return ExitCode::FAILURE;
        }
    };

    init_tracing_subscriber();

    // Handle daemon commands
    if let Some(mode) = cli.daemon_mode {
        match mode {
            Mode::ReloadDaemon => match connect_to_sock() {
                Ok(stream) => match request_reload(stream) {
                    Ok(response) => tracing::info!("Response from daemon: {}", response),
                    Err(e) => tracing::error!("Failed to reload: {}", e),
                },
                Err(e) => {
                    tracing::error!("Can't reload daemon, error: {}", e);
                    return ExitCode::FAILURE;
                }
            },
            Mode::StartDaemon => {
                if is_sock_exists() {
                    tracing::error!(
                        "Seems that daemon already started! There is socketfile: {}",
                        SOCK_FILE
                    );
                    return ExitCode::FAILURE;
                }
                match create_sock() {
                    Ok(listener) => {
                        start_daemon(config, listener).await;
                    }
                    Err(e) => {
                        eprintln!("Failed to create socketfile: {}, error: {}", SOCK_FILE, e);
                        return ExitCode::FAILURE;
                    }
                }
            }
        }
        // Or send signals to daemon
    } else {
        if let Some(command) = cli.command {
            match connect_to_sock() {
                Ok(mut stream) => {
                    let serialized_command = match serde_json::to_vec(&command) {
                        Ok(bytes) => bytes,
                        Err(e) => {
                            tracing::error!(
                                "Failed to serialize command: {:?}, error: {}",
                                command,
                                e
                            );
                            return ExitCode::FAILURE;
                        }
                    };
                    if let Err(e) = stream.write(&serialized_command) {
                        tracing::error!(
                            "Failed to send {:?} command over unix socket, error: {}",
                            command,
                            e
                        );
                    }
                    stream.flush().unwrap();
                    let mut buffer = Vec::new();
                    if let Err(e) = stream.read(&mut buffer) {
                        tracing::error!("Failed to read message from unix socket: {}", e);
                    }
                    let str = String::from_utf8_lossy(&buffer);
                    println!("{}", str);
                }
                Err(e) => {
                    tracing::error!("Can't connect to socket file, error: {}", e);
                    return ExitCode::FAILURE;
                }
            }
        }
    }

    ExitCode::SUCCESS
}

async fn start_daemon(config: configuration::Configuration, listener: UnixListener) {
    let (mut controller, sender) = BunchController::new(config);
    let listener = tokio::net::UnixListener::from_std(listener).unwrap();

    loop {
        tokio::select! {
            _ = controller.run_and_wait() => {
                    break;
                }
            message = listener.accept() => {
                match message {
                Ok((unix_stream, _addr)) => {
                    exec_command(unix_stream, sender.clone()).await;
                },
                Err(e) => tracing::error!("Failed to get intput on unix stream: {}", e),
                }
            }

        }
    }

    if let Err(e) = remove_sock() {
        tracing::error!("Failed to remove socket file {}, error: {}", SOCK_FILE, e);
    }
}

async fn exec_command(
    mut stream: tokio::net::UnixStream,
    sender: tokio::sync::mpsc::Sender<ControllerCommand>,
) {
    let mut buffer = Vec::new();

    // if let Err(e) = stream.read(&mut buffer).await {
    //     tracing::error!("Failed to read message from unix socket: {}", e);
    // }

    let timeout = tokio::time::sleep(std::time::Duration::from_secs(1));

    // Read from the socket until the timeout expires
    tokio::select! {
        result = stream.read_to_end(&mut buffer) => {
            if let Err(e) = result {
                tracing::error!("Failed to read message from unix socket: {}", e);
            }
        }
        _ = timeout => {
            // Exit from select! macro
        }
    }
    tracing::info!("Read for 1 sec: {}", String::from_utf8_lossy(&buffer));

    let command = match serde_json::from_slice::<Command>(&buffer) {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("Failed to parse message from unix socket: {}", e);
            return;
        }
    };

    let controler_command = match command {
        Command::Start(name) => ControllerCommand {
            command: buzzoperator::bunch_controller::Commands::StartServices(vec![name.name]),
            feedback: stream.into_std().unwrap(),
        },
        Command::Stop(name) => ControllerCommand {
            command: buzzoperator::bunch_controller::Commands::StopServices(vec![name.name]),
            feedback: stream.into_std().unwrap(),
        },
        Command::Restart(name) => ControllerCommand {
            command: buzzoperator::bunch_controller::Commands::RestartServices(vec![name.name]),
            feedback: stream.into_std().unwrap(),
        },
    };

    match sender.send(controler_command).await {
        Ok(_) => {}
        Err(e) => tracing::error!("Failed to send message to daemon: {}", e),
    };
    tracing::info!("Command is done");
}

fn init_tracing_subscriber() {
    let filelog = "/var/log/buzzoperator.log";

    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .with_level(true)
        .without_time()
        .finish()
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer(
                    std::fs::File::options()
                        .create(true)
                        .append(true)
                        .write(true)
                        .open(filelog)
                        .expect("Can't open log file!"),
                )
                .with_ansi(false),
        );
    tracing::subscriber::set_global_default(subscriber).expect("Failed to set up tracing");
}

fn request_reload(mut stream: UnixStream) -> std::io::Result<String> {
    stream.write_all(b"reload")?;
    let mut response = String::new();
    stream.read_to_string(&mut response)?;
    Ok(response)
}

fn is_sock_exists() -> bool {
    std::fs::metadata(SOCK_FILE).is_ok()
}

fn create_sock() -> std::io::Result<UnixListener> {
    std::os::unix::net::UnixListener::bind(SOCK_FILE)
}

fn remove_sock() -> std::io::Result<()> {
    std::fs::remove_file(&SOCK_FILE)
}

fn connect_to_sock() -> std::io::Result<UnixStream> {
    std::os::unix::net::UnixStream::connect(SOCK_FILE)
}
