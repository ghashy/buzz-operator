use std::fmt;
use std::net::IpAddr;
use std::path::PathBuf;

use serde::Deserialize;

/// This type represents both tcp and unix socket connections in simple enum.
#[derive(Deserialize, Debug, Clone, PartialEq, Eq)]
pub enum ConnectAddr {
    #[serde(rename = "unix")]
    Unix(PathBuf),
    #[serde(untagged)]
    Tcp { addr: IpAddr, port: u16 },
}

impl fmt::Display for ConnectAddr {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            ConnectAddr::Unix(buf) => f.write_str(&buf.display().to_string()),
            ConnectAddr::Tcp { addr, port } => write!(f, "{}:{}", addr, port),
        }
    }
}

impl From<ConnectAddr> for String {
    fn from(value: ConnectAddr) -> Self {
        match value {
            ConnectAddr::Unix(buf) => buf
                .to_str()
                .expect(&format!("Can't represent Path as &str: {:?}", buf))
                .to_string(),
            ConnectAddr::Tcp { addr, port } => format!("{}:{}", addr, port),
        }
    }
}
