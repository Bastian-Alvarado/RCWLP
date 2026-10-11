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

/// Where to get the ViGEmBus driver the host needs.
pub const DRIVER_URL: &str = "https://github.com/nefarius/ViGEmBus/releases";

/// Why the host couldn't start.
#[derive(Debug)]
pub enum StartError {
    /// The ViGEmBus driver isn't installed, or can't be reached.
    NoDriver(String),
    /// Something else has the UDP port, most likely another RCWLP host.
    PortInUse(u16),
    Other(String),
}

impl std::fmt::Display for StartError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StartError::NoDriver(e) => write!(
                f,
                "RCWLP needs the ViGEmBus driver to create virtual controllers, and can't reach it ({e}). \
                 Install it from {DRIVER_URL}, then start RCWLP again."
            ),
            StartError::PortInUse(port) => write!(
                f,
                "UDP port {port} is already in use. Is RCWLP already running? Look for its icon in the \
                 notification area, or close the other host window."
            ),
            StartError::Other(e) => f.write_str(e),
        }
    }
}

/// Runs a host until a "quit" arrives on `commands`. Each line on `commands` is handled like
/// one typed into the console; everything the host has to say goes to `out`.
pub fn run(config: Config, mut out: OutFn, commands: Receiver<String>) -> Result<(), StartError> {
    pad::check_driver().map_err(|e| StartError::NoDriver(e.to_string()))?;
    let port = config.port;
    let socket = UdpSocket::bind(("0.0.0.0", port)).map_err(|e| match e.kind() {
        // Windows reports a port held by another program as "access denied".
        io::ErrorKind::AddrInUse | io::ErrorKind::PermissionDenied => StartError::PortInUse(port),
        _ => StartError::Other(format!("Cannot listen on UDP port {port}: {e}")),
    })?;
    let other = |e: io::Error| StartError::Other(e.to_string());
    socket.set_read_timeout(Some(TICK)).map_err(other)?;
    let receiver = socket.try_clone().map_err(other)?;
    let waker = UdpSocket::bind(("127.0.0.1", 0))
        .and_then(|w| w.connect(("127.0.0.1", port)).map(|()| w))
        .map_err(|e| StartError::Other(format!("Cannot create the loopback socket: {e}")))?;

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
