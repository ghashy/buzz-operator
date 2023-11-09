use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand, ValueEnum};
use libc::__c_anonymous_ptrace_syscall_info_data;
use tracing_subscriber::prelude::__tracing_subscriber_SubscriberExt;

use buzzoperator::bunch_controller::BunchController;
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
    command: Option<Commands>,
    #[arg(long, short)]
    test_config: bool,

    /// These are for SystemD. Do not use these arguments!
    /// Use instead: systemctl <command> buzzoperator
    #[arg(value_enum, hide = true)]
    daemon_mode: Option<Mode>,
}

#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, ValueEnum)]
enum Mode {
    StartDaemon,
    ReloadDaemon,
}

#[derive(Subcommand)]
enum Commands {
    /// Start services with name, specified in the config.
    Start(Services),
    /// Stop services with name, specified in the config.
    Stop(Services),
    /// Get status of services.
    Status(Services),
    /// Restart services with name, specified in the config.
    Restart(Services),
}

#[derive(Args)]
struct Services {
    /// Names of the services.
    names: Vec<String>,
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
            // TODO: finish this
            // match connect_to_sock() {
            //     Ok(stream) => match request_reload(stream) {
            //         Ok(response) => tracing::info!("Response from daemon: {}", response),
            //         Err(e) => tracing::error!("Failed to reload: {}", e),
            //     },
            //     Err(e) => {
            //         tracing::error!("Can't reload daemon, error: {}", e);
            //         return ExitCode::FAILURE;
            //     };
            match command {
                // name can be 'all'
                Commands::Start(name) => todo!(),
                Commands::Stop(name) => todo!(),
                Commands::Restart(name) => todo!(),
            }
        }
    }

    ExitCode::SUCCESS
}

// TODO: Finish this
async fn start_daemon(config: configuration::Configuration, listener: UnixListener) {
    let (mut controller, mut tx) = BunchController::new(config);
    controller.run_and_wait().await;

    if let Err(e) = remove_sock() {
        tracing::error!("Failed to remove socket file {}, error: {}", SOCK_FILE, e);
    }
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
