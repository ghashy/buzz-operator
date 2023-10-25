#[derive(Debug, Clone, Copy)]
pub enum Event {
    UpdateRequest,
    StopRequest,
    ServicesEstablished,
    UpdatingFinished,
    UpdatingFailed,
}

#[derive(Debug, PartialEq, Eq)]
pub enum State {
    Starting,
    Running,
    Updating,
    Stopping,
}

pub struct StateBox(State);

impl StateBox {
    pub fn new() -> StateBox {
        StateBox(State::Starting)
    }

    pub fn update(&mut self, event: Event) -> &State {
        match (&self, event) {
            // Transition to update state
            (StateBox(State::Running), Event::UpdateRequest) => {
                self.0 = State::Updating;
                &self.0
            }
            // Transition to running state
            (StateBox(State::Starting), Event::ServicesEstablished)
            | (StateBox(State::Updating), Event::UpdatingFinished)
            | (StateBox(State::Updating), Event::UpdatingFailed) => {
                self.0 = State::Running;
                &self.0
            }
            // Transition to stopping state
            (StateBox(State::Running), Event::StopRequest)
            | (StateBox(State::Updating), Event::StopRequest) => {
                self.0 = State::Stopping;
                &self.0
            }
            _ => &self.0,
        }
    }

    fn next(self, event: Event) -> StateBox {
        match (&self, event) {
            // Transition to update state
            (StateBox(State::Running), Event::UpdateRequest) => {
                StateBox(State::Updating)
            }
            // Transition to running state
            (StateBox(State::Starting), Event::ServicesEstablished)
            | (StateBox(State::Updating), Event::UpdatingFinished)
            | (StateBox(State::Updating), Event::UpdatingFailed) => {
                StateBox(State::Running)
            }
            // Transition to stopping state
            (StateBox(State::Running), Event::StopRequest)
            | (StateBox(State::Updating), Event::StopRequest) => {
                StateBox(State::Stopping)
            }
            _ => self,
        }
    }

    pub fn current(&self) -> &State {
        &self.0
    }

    pub fn is(&self, with: State) -> bool {
        self.0 == with
    }
}

// struct BunchStateMachine<S> {
//     state: S,
// }

// struct Starting;

// struct Running;

// struct Updating;

// struct Stopping;

// pub enum BunchMode {
//     Starting(BunchStateMachine<Starting>),
//     Running(BunchStateMachine<Running>),
//     Updating(BunchStateMachine<Updating>),
//     Stopping(BunchStateMachine<Stopping>),
// }

// impl BunchMode {
//     pub fn new() -> Self {
//         BunchMode::Starting(BunchStateMachine { state: Starting {} })
//     }
//     pub fn consume()
// }

// impl From<BunchStateMachine<Starting>> for BunchStateMachine<Running> {
//     fn from(value: BunchStateMachine<Starting>) -> Self {
//         BunchStateMachine { state: Running {} }
//     }
// }

// impl From<BunchStateMachine<Running>> for BunchStateMachine<Updating> {
//     fn from(value: BunchStateMachine<Running>) -> Self {
//         BunchStateMachine { state: Updating {} }
//     }
// }

// impl From<BunchStateMachine<Running>> for BunchStateMachine<Stopping> {
//     fn from(value: BunchStateMachine<Running>) -> Self {
//         BunchStateMachine { state: Stopping {} }
//     }
// }

// impl From<BunchStateMachine<Updating>> for BunchStateMachine<Running> {
//     fn from(value: BunchStateMachine<Updating>) -> Self {
//         BunchStateMachine { state: Running {} }
//     }
// }

// impl From<BunchStateMachine<Updating>> for BunchStateMachine<Stopping> {
//     fn from(value: BunchStateMachine<Updating>) -> Self {
//         BunchStateMachine { state: Stopping {} }
//     }
// }
