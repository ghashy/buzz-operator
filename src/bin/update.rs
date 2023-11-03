use std::net::IpAddr;

use buzz_operator::connect_addr::ConnectAddr;
use clap::Parser;

/// Backend zero2prod server, all args passed, unix socket preferred
#[derive(Parser, Debug)]
#[clap(version, about)]
struct Arguments {
    #[clap(short, long, value_parser, num_args = 1)]
    add: Option<Vec<ConnectAddr>>,
    #[clap(short, long)]
    remove: Option<Vec<ConnectAddr>>,
}

fn main() {
    let args = Arguments::parse();
}
