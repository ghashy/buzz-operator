use sman::bunch_controller::BunchController;
use tracing_subscriber::prelude::__tracing_subscriber_SubscriberExt;

use sman::configuration;

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    init_tracing_subscriber();

    let config = configuration::Configuration::load_configuration().unwrap();

    let mut controller = BunchController::new(config);
    controller.run().await;
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
