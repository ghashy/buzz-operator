use tokio::sync::mpsc::Sender;

use crate::configuration::Configuration;
use crate::fs_watcher::FileSystemWatcher;
use crate::service::service_bunch::message::Message;

struct BunchConnection {
    name: String,
    connection: Sender<Message>,
}

pub struct Controller {
    fs_watcher: FileSystemWatcher,
    bunches: Vec<BunchConnection>,
    config: Configuration,
}

impl Controller {
    pub fn new(config: Configuration) -> Controller {
        todo!()
    }
}
