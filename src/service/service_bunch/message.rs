use crate::connect_addr::ConnectAddr;

/// Type for communication between [`ServiceBunch`](super::ServiceBunch) and
/// any higher controller type.
pub enum Message {
    // From controller
    StartUpdate,
    Shutdown,
    // From HealthCheck module
    HealthCheckFailed(ConnectAddr),
    // To Controller
    UnitSpawned(String, ConnectAddr),
    UnitsSpawned(String, Vec<ConnectAddr>),
    UnitDespawned(String, ConnectAddr),
    UnitsDespawned(String, Vec<ConnectAddr>),
    UnitReplaced {
        name: String,
        old: ConnectAddr,
        new: ConnectAddr,
    },
    UpdateFail {
        name: String,
        old: Vec<ConnectAddr>,
        new: Vec<ConnectAddr>,
    },
    UpdateFinished(String),
}
