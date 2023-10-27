use std::net::IpAddr;
use std::path::PathBuf;

use serde::Deserialize;

/// This type represents both tcp and unix socket connections in simple enum.
#[derive(Deserialize, Debug, Clone)]
pub enum ConnectAddr {
    #[serde(rename = "unix")]
    Unix(PathBuf),
    #[serde(untagged)]
    Tcp { addr: IpAddr, port: u16 },
}
