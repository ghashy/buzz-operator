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
    UnitSpawned(ConnectAddr),
    UnitsSpawned(Vec<ConnectAddr>),
    UnitDespawned(ConnectAddr),
    UnitsDespawned(Vec<ConnectAddr>),
    UnitReplaced {
        old: ConnectAddr,
        new: ConnectAddr,
    },
    UpdateFail {
        old: Vec<ConnectAddr>,
        new: Vec<ConnectAddr>,
    },
}
