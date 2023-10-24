use crate::configuration::Configuration;
use crate::fs_watcher::FileSystemWatcher;

pub struct Controller {
    fs_watcher: FileSystemWatcher,
    config: Configuration,
}

impl Controller {
    pub fn new(
        fs_watcher: FileSystemWatcher,
        config: Configuration,
    ) -> Controller {
        Controller { fs_watcher, config }
    }
}
