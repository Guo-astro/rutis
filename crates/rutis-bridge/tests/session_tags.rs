//! A call chain crossing processes keeps each process's calls apart (#325).
//!
//! Two processes, a and b, each with a runtime session (here a Rust far end
//! in-process, as `host` ↔ `rt`) opened first and a link to the other
//! opened second, so both number their sessions alike. a's runtime calls a
//! service a forwards over the link; b forwards it to its own runtime,
//! whose service calls back over the link into a's runtime, which is still
//! waiting synchronously for the first call.
//!
//! Session tags (`s1/host:1`) mark another session's calls in a chain. Were
//! they unique only within a process, b would take a's `s1/host:1` for its
//! own session `s1` and restore it to `host:1`: the very id b then calls its
//! runtime with, which the runtime refuses as an invalid invocation, ending
//! that session. Tests of one process never saw it: there every tag is
//! unique. So this one runs a and b as two processes of this test binary.

use std::io::{BufRead, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use rutis_bridge::channel::{Channel, Closer, PeerId};
use rutis_bridge::session::{Connection, Dispatch, Endpoint, Error, Reply, Value};
use rutis_test_support::{scaled, HANG_GUARD};
use serde_json::json;

const NAME: &str = "a_call_chain_crossing_processes_keeps_each_process_s_calls_apart";
const ROLE: &str = "RUTIS_325_ROLE";
const DIR: &str = "RUTIS_325_DIR";

fn id(s: &str) -> PeerId {
    PeerId::new(s).unwrap()
}

/// #325: run as the test, it starts b, then a, and expects a's call to
/// complete; run with `RUTIS_325_ROLE`, it is that process.
///
/// guarantee: bridge/call-chains-across-processes (docs/guarantees.md)
#[test]
fn a_call_chain_crossing_processes_keeps_each_process_s_calls_apart() {
    match std::env::var(ROLE).as_deref() {
        Ok("a") => return role(Role::A),
        Ok("b") => return role(Role::B),
        _ => {}
    }
    let dir = tempfile::tempdir().unwrap();
    let mut b = Process::start("b", dir.path().into());
    let port = dir.path().join("port");
    let deadline = Instant::now() + scaled(HANG_GUARD);
    while !port.exists() {
        assert!(
            Instant::now() < deadline,
            "b did not listen: {}",
            b.finish()
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    let mut a = Process::start("a", dir.path().into());
    let status = a.wait(scaled(HANG_GUARD * 3));
    let (a_out, b_out) = (a.finish(), b.finish());
    assert!(
        status == Some(true),
        "a's chain through b failed\n--- a ---\n{a_out}\n--- b ---\n{b_out}"
    );
}

struct Process(Child);
/// b serves until it is ended: never left running, even if the test panics.
impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
impl Process {
    fn start(role: &str, dir: PathBuf) -> Self {
        let child = Command::new(std::env::current_exe().unwrap())
            .args([NAME, "--exact", "--nocapture", "--test-threads=1"])
            .env(ROLE, role)
            .env(DIR, dir)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        Self(child)
    }
    /// Whether it exited successfully, `None` if still running at `limit`.
    fn wait(&mut self, limit: Duration) -> Option<bool> {
        let deadline = Instant::now() + limit;
        loop {
            if let Some(status) = self.0.try_wait().unwrap() {
                return Some(status.success());
            }
            if Instant::now() > deadline {
                return None;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    /// End it and return what it wrote.
    fn finish(&mut self) -> String {
        let _ = self.0.kill();
        let _ = self.0.wait();
        let mut text = String::new();
        for stream in [
            self.0
                .stdout
                .take()
                .map(|s| Box::new(s) as Box<dyn std::io::Read>),
            self.0
                .stderr
                .take()
                .map(|s| Box::new(s) as Box<dyn std::io::Read>),
        ]
        .into_iter()
        .flatten()
        {
            for line in std::io::BufReader::new(stream).lines() {
                text.push_str(&line.unwrap_or_default());
                text.push('\n');
            }
        }
        text
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Role {
    A,
    B,
}

/// A TCP connection as a channel.
struct Tcp(TcpStream);
impl Closer for Tcp {
    fn close(&self, _: rutis_bridge::Close) {
        let _ = self.0.shutdown(Shutdown::Both);
    }
}
fn channel(stream: TcpStream) -> Channel {
    stream.set_nodelay(true).unwrap();
    let closer = Arc::new(Tcp(stream.try_clone().unwrap()));
    rutis_bridge::pipe::framed(stream.try_clone().unwrap(), stream, closer)
}

fn endpoint(name: &str) -> Endpoint {
    Endpoint::rust(id(name))
}

fn role(role: Role) {
    let dir = PathBuf::from(std::env::var(DIR).unwrap());
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    let _entered = runtime.enter();
    // The runtime session first, the link second: both processes tag
    // their sessions in the same order.
    let link = Arc::new(std::sync::OnceLock::<Connection>::new());
    let (host_end, rt_end) = rutis_bridge::memory::pair();
    let host = Connection::open(
        host_end,
        Arc::new(Host { link: link.clone() }),
        endpoint("host"),
    )
    .unwrap();
    let rt = Connection::open(rt_end, Arc::new(Runtime(role)), endpoint("rt")).unwrap();
    let stream = match role {
        Role::B => {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let port = listener.local_addr().unwrap().port();
            std::fs::write(dir.join("port.tmp"), port.to_string()).unwrap();
            std::fs::rename(dir.join("port.tmp"), dir.join("port")).unwrap();
            listener.accept().unwrap().0
        }
        Role::A => {
            let port = std::fs::read_to_string(dir.join("port")).unwrap();
            TcpStream::connect(("127.0.0.1", port.parse::<u16>().unwrap())).unwrap()
        }
    };
    let (local, far) = match role {
        Role::A => ("a", "b"),
        Role::B => ("b", "a"),
    };
    let session = Connection::open(
        channel(stream),
        Arc::new(Link { host: host.clone() }),
        Endpoint::rust(id(local)).expect(id(far)),
    )
    .unwrap();
    link.set(session.clone()).ok();
    runtime.block_on(async {
        host.ready().await.unwrap();
        rt.ready().await.unwrap();
        session.ready().await.unwrap();
    });
    eprintln!(
        "{local}: sessions host {}, rt {}, link {}",
        host.tag(),
        rt.tag(),
        session.tag()
    );
    // Say why a session ends, should it.
    for (name, connection) in [("host", &host), ("rt", &rt), ("link", &session)] {
        let connection = connection.clone();
        runtime.spawn(async move {
            connection.closed().await;
            eprintln!(
                "{local}: {name} session ended: {:?}",
                connection.close_reason()
            );
        });
    }
    match role {
        Role::A => {
            let reply = host.invoke("app", "go", Value::Undefined);
            eprintln!("a: go returned {reply:?}");
            let reply = reply.unwrap().json().unwrap();
            assert_eq!(reply, json!("b's work, with a's echo"));
            assert!(rt.close_reason().is_none() && host.close_reason().is_none());
            std::io::stdout().flush().unwrap();
        }
        // b serves until a is done and the test ends it.
        Role::B => loop {
            std::thread::sleep(Duration::from_secs(1));
        },
    }
}

/// The host side of the runtime session: what the runtime calls goes over
/// the link, with the chain rebased for it.
struct Host {
    link: Arc<std::sync::OnceLock<Connection>>,
}
impl Dispatch for Host {
    fn invoke(&self, peer: &Connection, target: &str, method: &str, args: Value) -> Reply {
        let link = self.link.get().expect("the link is open");
        link.forward(peer, || link.invoke(target, method, args))
    }
}

/// The link's dispatch: what the other process calls goes to this
/// process's runtime.
struct Link {
    host: Connection,
}
impl Dispatch for Link {
    fn invoke(&self, peer: &Connection, target: &str, method: &str, args: Value) -> Reply {
        self.host
            .forward(peer, || self.host.invoke(target, method, args))
    }
}

/// The runtime. a's `go` calls b's `work`; b's `work` calls a's `echo`,
/// which a's runtime runs while it still waits for `work`.
struct Runtime(Role);
impl Dispatch for Runtime {
    fn invoke(&self, peer: &Connection, _: &str, method: &str, _: Value) -> Reply {
        match (self.0, method) {
            (Role::A, "go") => peer.invoke("svc", "work", Value::Undefined),
            (Role::B, "work") => {
                let echo = peer.invoke("svc", "echo", Value::Undefined)?.json()?;
                Ok(json!(format!("b's work, with {}", echo.as_str().unwrap_or("?"))).into())
            }
            (Role::A, "echo") => Ok(json!("a's echo").into()),
            _ => Err(Error::Value(format!("unknown method {method}"))),
        }
    }
}
