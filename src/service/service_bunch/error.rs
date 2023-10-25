#[derive(Debug)]
pub enum ServiceBunchError {
    FailedCreateAddress(std::io::Error),
    ControllerConnectionDropped,
    JoinError(tokio::task::JoinError),
    FailedToStart,
    UpdateIterrupted,
}

impl std::error::Error for ServiceBunchError {}

impl std::fmt::Display for ServiceBunchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ServiceBunchError::FailedCreateAddress(e) => {
                f.write_fmt(format_args!("{}", e))
            }
            ServiceBunchError::JoinError(e) => {
                f.write_fmt(format_args!("{}", e))
            }
            ServiceBunchError::FailedToStart => {
                f.write_fmt(format_args!("Failed to start"))
            }
            ServiceBunchError::ControllerConnectionDropped => {
                f.write_str("Controller connection dropped")
            }
            ServiceBunchError::UpdateIterrupted => {
                f.write_str("Update interrupted")
            }
        }
    }
}
