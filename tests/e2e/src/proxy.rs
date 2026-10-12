//! A TCP proxy between two processes of a scenario, to break the network
//! between them and bring it back (S5, #189) without containers.

use std::io::{ErrorKind, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::thread::JoinHandle;
use std::time::Duration;

/// A TCP proxy on a loopback port to a target address: each connection to
/// [`Proxy::addr`] is relayed to a new connection to the target, both
/// ways, half-closes included. Made by
/// [`Scenario::proxy`](crate::Scenario::proxy); closed when it is dropped
/// or the scenario ends.
///
/// What goes wrong in the proxy itself (a thread that panics, a listener
/// that fails) is residue of the scenario. What happens on a relayed
/// connection (a reset, an unreachable target) is the network's
/// behaviour: it is passed on as a closed connection, not reported.
pub struct Proxy {
    shared: Arc<Shared>,
}

pub(crate) struct Shared {
    addr: SocketAddr,
    target: SocketAddr,
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    /// Accepting: the listener's thread, and the flag that stops it.
    accepting: Option<(JoinHandle<()>, Arc<AtomicBool>)>,
    /// Closed for good: `restore` does not reopen it.
    closed: bool,
    /// Bumped by every cut: a connection accepted before it and set up
    /// after it is closed at once.
    generation: u64,
    /// The connections relayed now.
    connections: Vec<Weak<Connection>>,
    /// One thread per accepted connection; it joins the connection's other
    /// relaying thread before it ends.
    relays: Vec<JoinHandle<()>>,
    /// What went wrong in the proxy's own threads; the scenario's end
    /// reports it as residue.
    errors: Vec<String>,
}

/// Both ends of one relayed connection.
struct Connection {
    client: TcpStream,
    server: TcpStream,
    /// Cut: the relaying threads stop.
    closed: AtomicBool,
    /// How many directions reached their end of stream.
    ended: AtomicU8,
}

/// How long setting up a relayed connection may wait for the target. A
/// loopback target answers at once, or refuses (Windows retries a refused
/// connect for about two seconds); this bounds a cut waiting for a setup.
const CONNECT: Duration = Duration::from_secs(5);

/// Windows: how often a relaying thread blocked in a read checks whether
/// its connection was cut. Shutting a socket down wakes such a read on
/// Unix, not on Windows (there the read returns once the peer closes, or
/// after FIN_WAIT_2's two minutes). A read that times out on Windows leaves
/// the socket "indeterminate" by the documentation; in practice no data is
/// lost (it timed out because none came), and the alternative, closing the
/// handle under a blocked read, is worse.
#[cfg(windows)]
const CHECK_CUT: Duration = Duration::from_millis(100);

/// The proxy's port range: below every platform's ephemeral range (Linux
/// 32768–60999, macOS and Windows 49152–65535), so no outgoing connection
/// takes the port as its source port while the proxy is cut.
const PORTS: std::ops::Range<u16> = 20000..32000;

impl Connection {
    fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        let _ = self.client.shutdown(Shutdown::Both);
        let _ = self.server.shutdown(Shutdown::Both);
    }

    /// Relay `from` to the other end until its end of stream, which is
    /// passed on as a half-close; the connection closes once both ways have
    /// ended, or on an error (a reset is passed on as one).
    fn relay(&self, mut from: TcpStream, to_server: bool) {
        let mut to = match to_server {
            true => &self.server,
            false => &self.client,
        };
        let mut buffer = [0u8; 16 * 1024];
        loop {
            match from.read(&mut buffer) {
                Ok(0) => {
                    let _ = to.shutdown(Shutdown::Write);
                    if self.ended.fetch_add(1, Ordering::SeqCst) == 1 {
                        self.close();
                    }
                    return;
                }
                Ok(n) => {
                    if to.write_all(&buffer[..n]).is_err() {
                        break;
                    }
                }
                Err(error)
                    if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) =>
                {
                    if self.closed.load(Ordering::SeqCst) {
                        return;
                    }
                }
                Err(error) if error.kind() == ErrorKind::Interrupted => {}
                Err(_) => break,
            }
        }
        self.close();
    }
}

impl Proxy {
    pub(crate) fn start(target: SocketAddr) -> Proxy {
        let listener = listen();
        let addr = listener.local_addr().expect("the proxy's address");
        let shared = Arc::new(Shared {
            addr,
            target,
            state: Mutex::new(State::default()),
        });
        shared.accept(listener);
        Proxy { shared }
    }

    pub(crate) fn shared(&self) -> Arc<Shared> {
        self.shared.clone()
    }

    /// Where to connect instead of the target.
    pub fn addr(&self) -> SocketAddr {
        self.shared.addr
    }

    /// Break the network: close every connection relayed now, and stop
    /// listening, so new connections are refused. When it returns, both
    /// are done.
    pub fn cut(&self) {
        self.shared.stop(false);
    }

    /// Bring the network back: listen on the same address again. Fails the
    /// scenario if something else took the port meanwhile.
    pub fn restore(&self) {
        let mut state = self.shared.state.lock().unwrap();
        assert!(!state.closed, "the proxy is closed: the scenario ended");
        if state
            .accepting
            .as_ref()
            .is_some_and(|(thread, _)| !thread.is_finished())
        {
            return;
        }
        // Not listening, or its listener failed (an error, reported):
        // listen anew.
        let failed = state.accepting.take();
        drop(state);
        if let Some((thread, _)) = failed {
            self.shared.join(thread, "the accepting thread");
        }
        let listener = TcpListener::bind(self.shared.addr).unwrap_or_else(|error| {
            panic!(
                "the proxy cannot listen on {} again: {error}",
                self.shared.addr
            )
        });
        self.shared.accept(listener);
    }
}

impl Drop for Proxy {
    fn drop(&mut self) {
        self.shared.close();
    }
}

/// A listener on a free loopback port of [`PORTS`].
fn listen() -> TcpListener {
    use std::hash::{BuildHasher, RandomState};
    let span = u64::from(PORTS.end - PORTS.start);
    let first = RandomState::new().hash_one(std::process::id()) % span;
    let mut last = None;
    for offset in 0..span.min(200) {
        let port = PORTS.start + ((first + offset * 7919) % span) as u16;
        match TcpListener::bind(("127.0.0.1", port)) {
            Ok(listener) => return listener,
            Err(error) => last = Some(error),
        }
    }
    panic!("no free port for the proxy in {PORTS:?}: {last:?}");
}

impl Shared {
    /// Accept on `listener` until stopped; each connection is set up and
    /// relayed by threads of its own, so a slow target never holds up the
    /// accept (or a cut waiting for it).
    fn accept(self: &Arc<Self>, listener: TcpListener) {
        let shared = self.clone();
        let stopped = Arc::new(AtomicBool::new(false));
        let stop = stopped.clone();
        let mut state = self.state.lock().unwrap();
        let generation = state.generation;
        let thread = std::thread::spawn(move || {
            for client in listener.incoming() {
                // `stop` wakes this thread with a connection of its own.
                if stop.load(Ordering::SeqCst) {
                    return;
                }
                match client {
                    Ok(client) => {
                        let relayed = shared.clone();
                        let thread = std::thread::spawn(move || relayed.relay(client, generation));
                        shared.add_relay(thread);
                    }
                    Err(error) if error.kind() == ErrorKind::Interrupted => {}
                    Err(error) => {
                        shared.error(&format!("accept failed: {error}"));
                        return;
                    }
                }
            }
        });
        state.accepting = Some((thread, stopped));
    }

    /// Keep `thread`; join those already finished, so their panics are
    /// reported now.
    fn add_relay(&self, thread: JoinHandle<()>) {
        let mut state = self.state.lock().unwrap();
        let (finished, running) = std::mem::take(&mut state.relays)
            .into_iter()
            .partition(|relay| relay.is_finished());
        state.relays = running;
        state.relays.push(thread);
        drop(state);
        for relay in finished {
            self.join(relay, "a relaying thread");
        }
    }

    /// Relay `client` to a new connection to the target; if the target
    /// cannot be reached, close `client`, as an unreachable network would.
    fn relay(&self, client: TcpStream, generation: u64) {
        let Ok(server) = TcpStream::connect_timeout(&self.target, CONNECT) else {
            let _ = client.shutdown(Shutdown::Both);
            return;
        };
        let (Ok(client_reader), Ok(server_reader)) = (client.try_clone(), server.try_clone())
        else {
            let _ = client.shutdown(Shutdown::Both);
            let _ = server.shutdown(Shutdown::Both);
            return;
        };
        #[cfg(windows)]
        for reader in [&client_reader, &server_reader] {
            let _ = reader.set_read_timeout(Some(CHECK_CUT));
        }
        let connection = Arc::new(Connection {
            client,
            server,
            closed: AtomicBool::new(false),
            ended: AtomicU8::new(0),
        });
        let mut state = self.state.lock().unwrap();
        if state.generation != generation {
            // Cut while it was being set up.
            drop(state);
            connection.close();
            return;
        }
        state.connections.retain(|weak| weak.strong_count() > 0);
        state.connections.push(Arc::downgrade(&connection));
        drop(state);
        let back = {
            let connection = connection.clone();
            std::thread::spawn(move || connection.relay(server_reader, false))
        };
        connection.relay(client_reader, true);
        self.join(back, "a relaying thread");
    }

    /// Stop accepting and close the connections, waiting for the threads;
    /// `closed`: for good.
    fn stop(&self, closed: bool) {
        let mut state = self.state.lock().unwrap();
        state.closed |= closed;
        state.generation += 1;
        let accepting = state.accepting.take();
        drop(state);
        if let Some((accepting, stopped)) = accepting {
            // Wake the accept: it sees it is stopped and drops the
            // listener. The connect fails only if it is not listening.
            stopped.store(true, Ordering::SeqCst);
            let _ = TcpStream::connect(self.addr);
            self.join(accepting, "the accepting thread");
        }
        // No connection comes any more; one being set up sees the new
        // generation and closes itself.
        let mut state = self.state.lock().unwrap();
        let connections = std::mem::take(&mut state.connections);
        let relays = std::mem::take(&mut state.relays);
        drop(state);
        for connection in connections.iter().filter_map(Weak::upgrade) {
            connection.close();
        }
        for relay in relays {
            self.join(relay, "a relaying thread");
        }
    }

    fn join(&self, thread: JoinHandle<()>, what: &str) {
        if thread.join().is_err() {
            self.error(&format!("{what} panicked"));
        }
    }

    fn error(&self, what: &str) {
        let error = format!("proxy {}: {what}", self.addr);
        self.state.lock().unwrap().errors.push(error);
    }

    /// Close for good, and return what went wrong in its threads.
    pub(crate) fn close(&self) -> Vec<String> {
        self.stop(true);
        std::mem::take(&mut self.state.lock().unwrap().errors)
    }
}
