use tokio::sync::mpsc::{self, error::SendError};

use super::service_unit::ProcessID;
use crate::connect_addr::ConnectAddr;

/// Stores pid, address information and
/// [temination_sender](`UnitConnection::termination_sender`) to single child
/// process.
#[derive(Clone)]
pub(super) struct UnitConnection {
    termination_sender: mpsc::Sender<()>,
    pid: ProcessID,
    connect_addr: ConnectAddr,
}

impl UnitConnection {
    pub(super) fn new(
        sender: mpsc::Sender<()>,
        pid: ProcessID,
        connect_addr: ConnectAddr,
    ) -> Self {
        UnitConnection {
            termination_sender: sender,
            pid,
            connect_addr,
        }
    }
    pub(super) fn get_pid(&self) -> ProcessID {
        self.pid
    }
    pub(super) async fn terminate(&self) -> Result<(), SendError<()>> {
        self.termination_sender.send(()).await
    }
    pub(super) fn addr(&self) -> &ConnectAddr {
        &self.connect_addr
    }
}
