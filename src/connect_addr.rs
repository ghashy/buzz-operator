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

impl<'s> From<&'s std::ffi::OsStr> for ConnectAddr {
    fn from(value: &'s std::ffi::OsStr) -> Self {
        todo!()
    }
}
impl<'s> From<&'s str> for ConnectAddr {
    fn from(value: &'s str) -> Self {
        todo!()
    }
}
impl From<std::ffi::OsString> for ConnectAddr {
    fn from(value: std::ffi::OsString) -> Self {
        todo!()
    }
}
impl From<std::string::String> for ConnectAddr {
    fn from(value: std::string::String) -> Self {
        todo!()
    }
}
impl std::str::FromStr for ConnectAddr {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        todo!()
    }
}
impl clap::ValueEnum for ConnectAddr {
    fn from_str(input: &str, ignore_case: bool) -> Result<Self, String> {
        todo!()
    }

    fn value_variants<'a>() -> &'a [Self] {
        todo!()
    }

    fn to_possible_value(&self) -> Option<clap::builder::PossibleValue> {
        todo!()
    }
}
impl clap::builder::ValueParserFactory for ConnectAddr {
    type Parser = ();
    fn value_parser() -> Self::Parser {
        todo!()
    }
}
