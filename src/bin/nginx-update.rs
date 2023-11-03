use std::{
    fs::{File, OpenOptions},
    io::{BufReader, BufWriter, Read, Write},
    ops::Add,
    process::Command,
};

use clap::Parser;

const NGINX_CONF: &'static str = "/etc/nginx/nginx.conf";
const UPSTREAM_NAME: &'static str = "my_backend";

/// Backend zero2prod server, all args passed, unix socket preferred
#[derive(Parser, Debug)]
#[clap(version, about)]
struct Arguments {
    #[clap(short, long, value_parser, num_args = 1..)]
    add: Option<Vec<String>>,
    #[clap(short, long, value_parser, num_args = 1..)]
    remove: Option<Vec<String>>,
}

fn main() {
    let re = regex::Regex::new(&format!(r".*upstream {} \{{", UPSTREAM_NAME))
        .expect("Can't parse regex!");
    let args = Arguments::parse();

    let conf = OpenOptions::new()
        .read(true)
        .write(true)
        .open(NGINX_CONF)
        .expect("Can't open nginx.conf!");

    let mut conf_s = String::new();
    BufReader::new(conf)
        .read_to_string(&mut conf_s)
        .expect("Can't read nginx.conf into memory!");
    let mut output = String::new();

    // Remove old addresses
    let add = args.add.unwrap_or(Vec::new());
    let remove = args.remove.unwrap_or(Vec::new());

    for (i, line) in conf_s.lines().enumerate() {
        // Push line if not contained in remove list
        if !line.trim().strip_prefix("server ").is_some_and(|addr| {
            remove
                .iter()
                .find(|&element| {
                    addr.strip_suffix(';').unwrap_or("") == element
                })
                .is_some()
        }) {
            output.push_str(line);
            output.push('\n');
        } else {
            println!("Remove old: {}", line);
        }

        // Push new addresses
        if re.is_match(line) {
            println!("Found match on {} line.", i);
            let whitespace = line
                .split_once(|c: char| !c.is_whitespace())
                .unwrap_or(("", ""))
                .0;
            // Add new addresses
            for add in add.iter() {
                if conf_s.contains(add) {
                    continue;
                }
                println!("Add {}", add);
                output += whitespace;
                output += &String::from("\tserver ").add(add);
                output.push(';');
                output.push('\n');
            }
        }
    }
    write_conf(output);

    // Check configuration
    let command = Command::new("nginx").arg("-t").spawn();
    match command {
        Ok(mut child) => {
            match child.wait() {
                Ok(status) => {
                    if status.success() {
                        println!("nginx.conf is good!");
                        reload_nginx();
                    } else {
                        eprintln!("Error: {}", status);
                        // Revert backup
                        write_conf(conf_s);
                    }
                }
                Err(e) => {
                    eprintln!("Error: {}", e);
                    // Revert backup
                    write_conf(conf_s);
                }
            }
        }
        Err(e) => {
            eprintln!("Error: {}", e);
            // Revert backup
            write_conf(conf_s);
        }
    }
}

fn write_conf(output: String) {
    let mut writer = BufWriter::new(
        File::create(NGINX_CONF).expect("Can't overwrite nginx config!"),
    );
    writer
        .write_all(output.as_bytes())
        .expect("Can't write text into nginx.conf file!");
    writer.flush().expect("Failed to flush data");
}

fn reload_nginx() {
    let command = Command::new("systemctl").arg("reload").arg("nginx").spawn();

    match command {
        Ok(mut child) => match child.wait() {
            Ok(status) => {
                println!("ExitStatus: {}", status);
            }
            Err(e) => {
                eprintln!("Error: {}", e);
            }
        },
        Err(e) => {
            eprintln!("Error: {}", e);
        }
    }
}
