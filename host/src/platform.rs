//! The real platform: a UDP socket and the ViGEmBus driver, and the loop that runs the server
//! on them.

use std::io;
use std::net::{SocketAddr, UdpSocket};
use std::sync::Arc;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use crate::pad::{self, Pad};
use crate::protocol::{self, PadKind};
use crate::server::{self, FeedbackFn, Out, OutFn, Platform, Server, VirtualPad};
use crate::store::Store;

/// Upper bound on how long the loop sleeps waiting for a packet.
const TICK: Duration = Duration::from_millis(20);

pub struct Config {
    pub port: u16,
    /// Report why packets were ignored.
    pub verbose: bool,
}

struct WindowsPlatform {
    socket: UdpSocket,
    /// Rumble arrives on driver threads, which poke the socket through this so the loop
    /// handles it right away instead of after its receive timeout.
    waker: Arc<UdpSocket>,
}

impl Platform for WindowsPlatform {
    fn now(&self) -> Instant {
        Instant::now()
    }

    fn send(&self, data: &[u8], to: SocketAddr) -> io::Result<()> {
        self.socket.send_to(data, to).map(drop)
    }

    fn plug(&self, kind: PadKind, mut on_feedback: FeedbackFn) -> Result<Box<dyn VirtualPad>, String> {
        let waker = self.waker.clone();
        let pad = Pad::plug(kind, move |feedback| {
            on_feedback(feedback);
            let _ = waker.send(&[]);
        })
        .map_err(|e| e.to_string())?;
        Ok(Box::new(pad))
    }
}

/// Runs a host until a "quit" arrives on `commands`. Each line on `commands` is handled like
/// one typed into the console; everything the host has to say goes to `out`.
pub fn run(config: Config, mut out: OutFn, commands: Receiver<String>) -> Result<(), String> {
    pad::check_driver().map_err(|e| {
        format!(
            "Cannot reach the ViGEmBus driver ({e}). \
             Install it from https://github.com/nefarius/ViGEmBus/releases and try again."
        )
    })?;
    let port = config.port;
    let socket = UdpSocket::bind(("0.0.0.0", port)).map_err(|e| format!("Cannot listen on UDP port {port}: {e}"))?;
    socket.set_read_timeout(Some(TICK)).map_err(|e| e.to_string())?;
    let receiver = socket.try_clone().map_err(|e| e.to_string())?;
    let waker = UdpSocket::bind(("127.0.0.1", 0))
        .and_then(|w| w.connect(("127.0.0.1", port)).map(|()| w))
        .map_err(|e| format!("Cannot create the loopback socket: {e}"))?;

    let host_name = std::env::var("COMPUTERNAME").unwrap_or_else(|_| "PC".into());
    let store = Store::load();
    out(Out::Event(format!(
        "RCWLP host {} \"{host_name}\" listening on UDP port {port} (protocol v{}). {} paired phone(s).",
        server::VERSION,
        protocol::VERSION,
        store.clients.len()
    )));
    out(Out::Reply(server::HELP.into()));
    let platform = WindowsPlatform { socket, waker: Arc::new(waker) };
    let mut server = Server::new(platform, store, host_name, out, config.verbose);

    let mut buf = [0u8; 1500];
    loop {
        match receiver.recv_from(&mut buf) {
            // Empty datagrams are the waker's pokes: nothing to handle, just a turn of the loop.
            Ok((0, _)) => {}
            Ok((n, from)) => server.handle(&buf[..n], from),
            Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) => {}
            // Windows surfaces ICMP "port unreachable" from an earlier send this way; harmless.
            Err(e) if e.kind() == io::ErrorKind::ConnectionReset => {}
            Err(e) => server.event(format!("Receive error: {e}")),
        }
        server.drain_feedback();
        // A closed command channel (stdin at EOF) just means no more commands.
        while let Ok(line) = commands.try_recv() {
            if !server.command(&line) {
                return Ok(());
            }
        }
        server.tick();
    }
}
