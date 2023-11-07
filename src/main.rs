use std::path::Path;

use buzzoperator::bunch_controller::BunchController;
use tracing_subscriber::prelude::__tracing_subscriber_SubscriberExt;

use buzzoperator::configuration;

// TODO: add config file verification with -t flag
// TODO: add `reload` possibility
// TODO: implement remove all addresses on termination, and stopping all processes gracefully
#[tokio::main(flavor = "multi_thread")]
async fn main() {
    init_tracing_subscriber();

    let config = configuration::Configuration::load_configuration().unwrap();
    dbg!(&config);

    let mut controller = BunchController::new(config);
    controller.run().await;
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
