use std::{
    future::Future,
    path::Path,
    pin::Pin,
    task::{Context, Poll},
};

use notify::Result;
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use tokio::sync::mpsc;

struct FileSystemWatcher {
    watcher: RecommendedWatcher,
    receiver: mpsc::Receiver<Result<notify::Event>>,
}

impl FileSystemWatcher {
    fn new(path: &Path) -> FileSystemWatcher {
        let (tx, rx) = mpsc::channel(100);

        let mut watcher = notify::recommended_watcher(move |res| {
            let tx = tx.clone();
            tokio::spawn(async move {
                tx.send(res).await.unwrap();
            });
        })
        .unwrap();

        // Add a path to be watched.
        watcher.watch(path, RecursiveMode::NonRecursive).unwrap();

        // Create an instance of the custom Future
        FileSystemWatcher {
            watcher,
            receiver: rx,
        }
    }
}

impl Future for FileSystemWatcher {
    type Output = ();

    fn poll(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Self::Output> {
        // Poll the receiver for the next event
        match Pin::new(&mut self.receiver).poll_recv(cx) {
            Poll::Ready(Some(Ok(event))) => {
                println!("Event: {:?}", event);
                Poll::Pending
            }
            Poll::Ready(Some(Err(e))) => {
                println!("Error: {:?}", e);
                Poll::Pending
            }
            Poll::Ready(None) => Poll::Ready(()), // Receiver closed, future completed
            Poll::Pending => Poll::Pending,
        }
    }
}
