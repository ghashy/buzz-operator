use std::path::Path;

use notify::{Event, Result};
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use tokio::sync::mpsc;

/// Type for listening events from filesystem. Provides us with the possibility
/// to detect new binary file creation event.
/// WARN: Sends only `create` notifications, all other skipped,
/// but printed in logs.
pub struct FileSystemWatcher {
    watcher: RecommendedWatcher,
    pub receiver: mpsc::Receiver<Event>,
}

impl FileSystemWatcher {
    pub fn new(paths: &[&Path]) -> FileSystemWatcher {
        let (tx, rx) = mpsc::channel(100);

        let mut watcher = notify::recommended_watcher(move |res: Result<Event>| {
            if let Ok(event) = res {
                match event.kind {
                    notify::EventKind::Create(_) => {
                        tx.blocking_send(event)
                            .expect("Failed to send filesystem notification");
                    }
                    _ => tracing::trace!("FS EVENT: {:?}", event),
                }
            }
        })
        .unwrap();

        for &path in paths.iter() {
            watcher.watch(path, RecursiveMode::NonRecursive).unwrap();
        }

        // Create an instance of the custom Future
        FileSystemWatcher {
            watcher,
            receiver: rx,
        }
    }
}

// impl Future for FileSystemWatcher {
//     type Output = Option<Event>;

//     fn poll(
//         mut self: Pin<&mut Self>,
//         cx: &mut Context<'_>,
//     ) -> Poll<Self::Output> {
//         // Poll the receiver for the next event
//         match self.receiver.poll_recv(cx) {
//             Poll::Ready(Some(Ok(event))) => {
//                 tracing::info!("FileSystemWatcher event: {:?}", event);
//                 Poll::Ready(Some(event))
//             }
//             // Just trace error
//             Poll::Ready(Some(Err(e))) => {
//                 tracing::error!("FileSystemWatcher error: {}", e);
//                 Poll::Pending
//             }
//             // Receiver closed, future completed
//             Poll::Ready(None) => Poll::Ready(None),
//             Poll::Pending => Poll::Pending,
//         }
//     }
// }
