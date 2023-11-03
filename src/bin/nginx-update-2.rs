use clap::Parser;
use std::process::{Command, Stdio};

const NGINX_CONF: &str = "/Users/ghashy/Desktop/test/nginx.conf";
const UPSTREAM_NAME: &str = "ghashy_backend";

#[derive(Parser, Debug)]
#[clap(version, about)]
struct Arguments {
    #[clap(short, long, value_parser, num_args = 1..)]
    add: Option<Vec<String>>,
    #[clap(short, long, value_parser, num_args = 1..)]
    remove: Option<Vec<String>>,
}

fn main() {
    let args = Arguments::parse();

    let add = args.add.unwrap_or_default();
    let remove = args.remove.unwrap_or_default();

    let remove_commands: Vec<String> = remove
        .iter()
        .map(|addr| format!("sed -i '/^server {};/d' {}", addr, NGINX_CONF))
        .collect();

    let add_commands: Vec<String> = add
        .iter()
        .map(|addr| format!("awk '/^upstream {} {{/ {{ print; print \"\\tserver {};\"; next }};1' {}", UPSTREAM_NAME, addr, NGINX_CONF))
        .collect();

    for command in remove_commands {
        Command::new("sh")
            .arg("-c")
            .arg(&command)
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .output()
            .expect("Failed to execute command");
    }

    for command in add_commands {
        Command::new("sh")
            .arg("-c")
            .arg(&command)
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .output()
            .expect("Failed to execute command");
    }
}
