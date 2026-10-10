//! RCWLP host, console version. Type commands in the window; see `server::HELP`.

use std::io::{self, BufRead};
use std::sync::mpsc;
use std::thread;

use rcwlp_host::platform::{self, Config};
use rcwlp_host::server::{self, Out};
use rcwlp_host::{clock, protocol};

fn main() {
    let config = parse_args();
    let (tx, commands) = mpsc::channel();
    thread::spawn(move || {
        for line in io::stdin().lock().lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    let out = Box::new(|o: Out| match o {
        Out::Event(s) => println!("{} {s}", clock::stamp()),
        Out::Debug(s) => println!("{} [verbose] {s}", clock::stamp()),
        Out::Reply(s) => println!("{s}"),
        Out::Pairing(_) | Out::Connected(_) => {}
    });
    if let Err(e) = platform::run(config, out, commands) {
        eprintln!("{e}");
        std::process::exit(1);
    }
}

const USAGE: &str = "Usage: rcwlp-host [--port <udp port>] [--verbose]

  --port <n>     Listen on this UDP port instead of 47391. The phone's
                 \"Add PC by IP\" then needs ip:port.
  --verbose, -v  Also log every packet that gets ignored, and why.
  --version      Print the version and exit.";

fn parse_args() -> Config {
    let mut config = Config { port: protocol::DEFAULT_PORT, verbose: false };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--port" => match args.next().and_then(|p| p.parse().ok()) {
                Some(port) => config.port = port,
                None => usage_error(),
            },
            "--verbose" | "-v" => config.verbose = true,
            "--version" => {
                println!("rcwlp-host {} (protocol v{})", server::VERSION, protocol::VERSION);
                std::process::exit(0);
            }
            "--help" | "-h" => {
                println!("{USAGE}");
                std::process::exit(0);
            }
            _ => usage_error(),
        }
    }
    config
}

fn usage_error() -> ! {
    eprintln!("{USAGE}");
    std::process::exit(2);
}
