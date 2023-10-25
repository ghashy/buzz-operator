use tokio::{
    sync::mpsc::{self, error::SendError},
    time::Instant,
};

use crate::connect_addr::{self, ConnectAddr};

use super::service_unit::ProcessID;

#[derive(Clone, Default)]
enum UnitMode {
    #[default]
    Keeping,
    Stopping,
}

#[derive(Clone)]
pub(super) struct UnitConnection {
    pub(super) last_check_time: Instant,
    termination_sender: mpsc::Sender<()>,
    pid: ProcessID,
    failure_count: u16,
    mode: UnitMode,
    is_stable: bool,
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
            failure_count: 0,
            mode: UnitMode::default(),
            last_check_time: Instant::now(),
            is_stable: true,
            connect_addr,
        }
    }
    pub(super) fn failure_add(&mut self) -> u16 {
        self.failure_count += 1;
        self.failure_count
    }
    pub(super) fn failures_reset(&mut self) {
        self.last_check_time = Instant::now();
        self.failure_count = 0;
    }
    pub(super) fn is_stable(&self) -> bool {
        self.is_stable
    }
    pub(super) fn get_pid(&self) -> ProcessID {
        self.pid
    }
    pub(crate) async fn terminate(&self) -> Result<(), SendError<()>> {
        self.termination_sender.blocking_send(())
    }
    pub(super) fn addr(&self) -> &ConnectAddr {
        &self.connect_addr
    }
}
