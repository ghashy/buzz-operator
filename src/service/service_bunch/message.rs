use crate::connect_addr::ConnectAddr;

/// Type for communication between [`ServiceBunch`](super::ServiceBunch) and
/// any higher controller type.
pub enum Message {
    // From controller
    StartUpdate,
    Shutdown,
    // To Controller
    ServiceSpawned(ConnectAddr),
    ServiceReplaced {
        old: ConnectAddr,
        new: ConnectAddr,
    },
    UpdateFail {
        old: Vec<ConnectAddr>,
        new: Vec<ConnectAddr>,
    },
    ServiceDespawned(Vec<ConnectAddr>),
}
