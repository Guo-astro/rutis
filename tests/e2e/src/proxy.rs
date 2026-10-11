//! A TCP proxy between two processes of a scenario, to break the network
//! between them and bring it back (S5, #189) without containers.

use std::io::{ErrorKind, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::thread::JoinHandle;

/// A TCP proxy on a loopback port to a target address: each connection to
/// [`Proxy::addr`] is relayed to a new connection to the target. Made by
/// [`Scenario::proxy`](crate::Scenario::proxy); closed when it is dropped
/// or the scenario ends.
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
    /// The connections relayed now.
    connections: Vec<Weak<Connection>>,
    /// The relaying threads, two per connection.
    relays: Vec<JoinHandle<()>>,
    /// What went wrong in the proxy's own threads; the scenario's end
    /// reports it as residue.
    errors: Vec<String>,
}

/// Both ends of one relayed connection.
struct Connection {
    client: TcpStream,
    server: TcpStream,
    closed: AtomicBool,
}

/// How often a relaying thread blocked in a read checks whether its
/// connection was closed. Shutting a socket down wakes such a read on
/// Unix; on Windows it does not (the read returns once the peer closes, or
/// after FIN_WAIT_2's two minutes), so the thread has to look.
const CHECK_CLOSED: std::time::Duration = std::time::Duration::from_millis(100);

impl Connection {
    fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        let _ = self.client.shutdown(Shutdown::Both);
        let _ = self.server.shutdown(Shutdown::Both);
    }
}

impl Proxy {
    pub(crate) fn start(target: SocketAddr) -> Proxy {
        let listener = TcpListener::bind("127.0.0.1:0").expect("a port for the proxy");
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

    /// Bring the network back: listen on the same address again.
    pub fn restore(&self) {
        let state = self.shared.state.lock().unwrap();
        assert!(!state.closed, "the proxy is closed: the scenario ended");
        if state.accepting.is_some() {
            return;
        }
        drop(state);
        let listener = TcpListener::bind(self.shared.addr)
            .unwrap_or_else(|error| panic!("the proxy cannot listen again: {error}"));
        self.shared.accept(listener);
    }
}

impl Drop for Proxy {
    fn drop(&mut self) {
        self.shared.close();
    }
}

impl Shared {
    /// Accept on `listener` until stopped.
    fn accept(self: &Arc<Self>, listener: TcpListener) {
        let shared = self.clone();
        let stopped = Arc::new(AtomicBool::new(false));
        let stop = stopped.clone();
        let thread = std::thread::spawn(move || {
            for client in listener.incoming() {
                // `stop` wakes this thread with a connection of its own.
                if stop.load(Ordering::SeqCst) {
                    return;
                }
                match client {
                    Ok(client) => shared.relay(client),
                    Err(error) => {
                        shared
                            .state
                            .lock()
                            .unwrap()
                            .errors
                            .push(format!("proxy {}: accept failed: {error}", shared.addr));
                        return;
                    }
                }
            }
        });
        self.state.lock().unwrap().accepting = Some((thread, stopped));
    }

    /// Relay `client` to a new connection to the target; if the target
    /// cannot be reached, close `client`, as an unreachable network would.
    fn relay(&self, client: TcpStream) {
        let Ok(server) = TcpStream::connect(self.target) else {
            let _ = client.shutdown(Shutdown::Both);
            return;
        };
        let (Ok(client_reader), Ok(server_reader)) = (client.try_clone(), server.try_clone())
        else {
            let _ = client.shutdown(Shutdown::Both);
            let _ = server.shutdown(Shutdown::Both);
            return;
        };
        for reader in [&client_reader, &server_reader] {
            let _ = reader.set_read_timeout(Some(CHECK_CLOSED));
        }
        let connection = Arc::new(Connection {
            client,
            server,
            closed: AtomicBool::new(false),
        });
        let mut state = self.state.lock().unwrap();
        state.connections.retain(|weak| weak.strong_count() > 0);
        state.connections.push(Arc::downgrade(&connection));
        state.relays.retain(|relay| !relay.is_finished());
        for (mut from, to) in [(client_reader, true), (server_reader, false)] {
            let connection = connection.clone();
            state.relays.push(std::thread::spawn(move || {
                let mut to = match to {
                    true => &connection.server,
                    false => &connection.client,
                };
                let mut buffer = [0u8; 16 * 1024];
                // Either way ending ends the connection: the tests need no
                // half-closed connections.
                loop {
                    match from.read(&mut buffer) {
                        Ok(0) => break,
                        Ok(n) => {
                            if to.write_all(&buffer[..n]).is_err() {
                                break;
                            }
                        }
                        Err(error)
                            if matches!(
                                error.kind(),
                                ErrorKind::WouldBlock | ErrorKind::TimedOut
                            ) =>
                        {
                            if connection.closed.load(Ordering::SeqCst) {
                                break;
                            }
                        }
                        Err(_) => break,
                    }
                }
                connection.close();
            }));
        }
    }

    /// Stop accepting and close the connections, waiting for the threads;
    /// `closed`: for good.
    fn stop(&self, closed: bool) {
        let mut state = self.state.lock().unwrap();
        state.closed |= closed;
        let accepting = state.accepting.take();
        drop(state);
        if let Some((accepting, stopped)) = accepting {
            // Wake the accept: it sees it is stopped and drops the
            // listener. The connect fails only if it is not listening.
            stopped.store(true, Ordering::SeqCst);
            let _ = TcpStream::connect(self.addr);
            if accepting.join().is_err() {
                self.error("the accepting thread panicked");
            }
        }
        // No connection comes any more.
        let mut state = self.state.lock().unwrap();
        let connections = std::mem::take(&mut state.connections);
        let relays = std::mem::take(&mut state.relays);
        drop(state);
        for connection in connections.iter().filter_map(Weak::upgrade) {
            connection.close();
        }
        for relay in relays {
            if relay.join().is_err() {
                self.error("a relaying thread panicked");
            }
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
