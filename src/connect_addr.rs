use std::net::IpAddr;
use std::path::PathBuf;

use serde::Deserialize;

#[derive(Deserialize, Debug, Clone)]
// #[serde(untagged)]
pub enum ConnectAddr {
    #[serde(rename = "unix")]
    Unix(PathBuf),
    #[serde(untagged)]
    Tcp { addr: IpAddr, port: u16 },
}
