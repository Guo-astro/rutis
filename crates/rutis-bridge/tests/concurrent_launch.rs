//! Many runtime processes started at once from one host process (Q6.5.2,
//! risk C11, issue #184). Each process must get its own channel and nothing
//! else: a descriptor of another process's channel that crosses into it
//! keeps that channel open after its own process has ended, and its host
//! does not see the end until the stray holder ends too.
//!
//! Each round starts `THREADS` processes at the same moment, one per thread
//! with its own runtime, as parallel tests do. While all are alive, every
//! process is checked for sockets (and, for the shell, pipes) it should
//! not hold; then each ends its process and must see its channel end while
//! the others are still running. A failure names the round and the thread.
// Unix only (Q7.8): what leaks between processes started at once is a
// descriptor inherited across fork/exec (fd:3, socket pairs), found with
// /proc or lsof; Windows hands no descriptor over (loopback handover).
#![cfg(unix)]

use std::path::Path;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use rutis_bridge::channel::ChannelError;
use rutis_bridge::pipe::{self, Processes, Spawn};

const THREADS: usize = 16;
const ROUNDS: usize = 5;
/// A hang guard only: a channel whose far end leaked never ends while the
/// stray holder lives, and the holders wait for each other.
const HANG_GUARD: Duration = Duration::from_secs(20);

/// What a process holds that it should not: descriptors other than fd 3
/// that are sockets, or pipes too when `pipes`. Its own channel is fd 3;
/// fds 0–2 are inherited on purpose.
fn strays(pid: u32, pipes: bool) -> Vec<String> {
    descriptors(pid)
        .into_iter()
        .filter(|(fd, kind)| *fd > 3 && (*kind == Kind::Socket || (pipes && *kind == Kind::Pipe)))
        .map(|(fd, kind)| format!("fd {fd} ({kind:?})"))
        .collect()
}

#[derive(Debug, PartialEq, Eq)]
enum Kind {
    Socket,
    Pipe,
    Other,
}

#[cfg(target_os = "linux")]
fn descriptors(pid: u32) -> Vec<(i32, Kind)> {
    let entries = std::fs::read_dir(format!("/proc/{pid}/fd")).expect("list the process's fds");
    entries
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let fd = entry.file_name().to_str()?.parse().ok()?;
            let target = std::fs::read_link(entry.path()).ok()?;
            let target = target.to_string_lossy();
            let kind = match () {
                _ if target.starts_with("socket:") => Kind::Socket,
                _ if target.starts_with("pipe:") => Kind::Pipe,
                _ => Kind::Other,
            };
            Some((fd, kind))
        })
        .collect()
}

#[cfg(target_os = "macos")]
fn descriptors(pid: u32) -> Vec<(i32, Kind)> {
    let size = std::mem::size_of::<libc::proc_fdinfo>();
    let mut fds: Vec<libc::proc_fdinfo> = Vec::with_capacity(1024);
    // SAFETY: the buffer holds `capacity` entries and proc_pidinfo writes at
    // most the byte count it is given.
    let written = unsafe {
        libc::proc_pidinfo(
            pid as libc::c_int,
            libc::PROC_PIDLISTFDS,
            0,
            fds.as_mut_ptr().cast(),
            (fds.capacity() * size) as libc::c_int,
        )
    };
    assert!(
        written > 0,
        "list the fds of {pid}: {}",
        std::io::Error::last_os_error()
    );
    // SAFETY: proc_pidinfo initialised `written` bytes of whole entries.
    unsafe { fds.set_len(written as usize / size) };
    fds.iter()
        .map(|info| {
            let kind = match info.proc_fdtype as libc::c_int {
                libc::PROX_FDTYPE_SOCKET => Kind::Socket,
                libc::PROX_FDTYPE_PIPE => Kind::Pipe,
                _ => Kind::Other,
            };
            (info.proc_fd, kind)
        })
        .collect()
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn descriptors(_pid: u32) -> Vec<(i32, Kind)> {
    // No portable way to list another process's fds: the end-of-channel
    // check still runs.
    Vec::new()
}

/// Runs `round` on `THREADS` threads at once, `ROUNDS` times, each thread on
/// a runtime of its own. The barrier lines the threads up: at the start, so
/// the processes are started together, and between the steps of a round.
fn rounds(round: impl Fn(usize, usize, &Lineup) + Send + Sync + 'static) {
    let round = Arc::new(round);
    for number in 0..ROUNDS {
        let barrier = Arc::new(Lineup::default());
        let threads: Vec<_> = (0..THREADS)
            .map(|thread| {
                let (round, barrier) = (round.clone(), barrier.clone());
                std::thread::Builder::new()
                    .name(format!("launch-{number}-{thread}"))
                    .spawn(move || round(number, thread, &barrier))
                    .unwrap()
            })
            .collect();
        let failures: Vec<_> = threads
            .into_iter()
            .enumerate()
            .filter_map(|(thread, handle)| handle.join().err().map(|_| thread))
            .collect();
        assert!(
            failures.is_empty(),
            "round {number}: threads {failures:?} failed (see their panics above)"
        );
    }
}

/// Whether processes dial a loopback address instead of taking fd 3
/// (`RUTIS_LOCAL_HANDOVER=loopback`): their channel is then a TCP socket
/// on any fd.
fn loopback() -> bool {
    std::env::var_os(rutis_bridge::pipe::HANDOVER_VARIABLE).is_some_and(|value| value == "loopback")
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

/// A barrier for the `THREADS` threads of a round that gives up after
/// [`HANG_GUARD`]: a thread stuck in a start (one whose descriptor another
/// process holds, say) must not hang the others, which then end their
/// processes and so release it. Once it gives up, it stays given up.
#[derive(Default)]
struct Lineup {
    /// Threads arrived, generation, given up.
    state: Mutex<(usize, usize, bool)>,
    changed: Condvar,
}

impl Lineup {
    fn wait(&self) -> Result<(), String> {
        let mut state = self.state.lock().unwrap();
        if state.2 {
            return Err("the round already gave up waiting".into());
        }
        let generation = state.1;
        state.0 += 1;
        if state.0 == THREADS {
            *state = (0, generation + 1, false);
            self.changed.notify_all();
            return Ok(());
        }
        let (mut state, timeout) = self
            .changed
            .wait_timeout_while(state, HANG_GUARD, |state| state.1 == generation && !state.2)
            .unwrap();
        if state.1 != generation {
            return Ok(());
        }
        if timeout.timed_out() && !state.2 {
            state.2 = true;
            self.changed.notify_all();
            return Err(format!(
                "only {} of {THREADS} threads got here within {HANG_GUARD:?}",
                state.0
            ));
        }
        Err("the round gave up waiting".into())
    }
}

/// What went wrong on one thread: carried to the end of its round rather
/// than panicking at once, which would leave the others waiting.
struct Failures(Vec<String>);

impl Failures {
    fn check(&mut self, what: &str, ok: Result<(), String>) {
        if let Err(error) = ok {
            self.0.push(format!("{what}: {error}"));
        }
    }

    fn report(self, number: usize, thread: usize) {
        assert!(
            self.0.is_empty(),
            "round {number}, thread {thread}: {}",
            self.0.join("; ")
        );
    }
}

/// guarantee: bridge/spawned-processes-own-channel (issue #184, risk C11,
/// Q6.5.2): `pipe::spawn` processes started at once each
/// hold their own channel only.
#[test]
fn spawned_processes_started_at_once_hold_only_their_own_channel() {
    if loopback() {
        // The shell cannot dial: it talks on fd 3 only.
        eprintln!("skipped: a shell takes fd 3, not a loopback address");
        return;
    }
    // A shell that reports its pid, waits for a line, and exits. Its
    // standard input and output move to the channel for good: a shell
    // keeps a copy of a descriptor it redirects for one command only (on
    // fd 10, say) until the command is done, which would look like a stray.
    let mut spawn = Spawn::new("sh");
    spawn.args = vec![
        "-c".into(),
        r#"exec <&3 >&3; echo "$$"; read line; exit 0"#.into(),
    ];
    rounds(move |number, thread, barrier| {
        let processes = Processes::default();
        let runtime = runtime();
        let mut failures = Failures(Vec::new());
        failures.check("line up", barrier.wait());
        let channel = runtime.block_on(pipe::spawn(&spawn, &processes));
        let mut channel = match channel {
            Ok(channel) => Some(channel),
            Err(error) => {
                failures.check("start", Err(error.to_string()));
                None
            }
        };
        let pid = channel
            .as_mut()
            .and_then(|channel| match channel.receiver.recv() {
                Ok(Some(line)) => String::from_utf8_lossy(&line).trim().parse::<u32>().ok(),
                other => {
                    failures.check("first line", Err(format!("{other:?}")));
                    None
                }
            });
        // Every process of the round is running.
        failures.check("line up", barrier.wait());
        if let Some(pid) = pid {
            let strays = strays(pid, true);
            failures.check(
                "descriptors",
                match strays.is_empty() {
                    true => Ok(()),
                    false => Err(format!("process {pid} holds {}", strays.join(", "))),
                },
            );
        }
        failures.check("line up", barrier.wait());
        if let Some(mut channel) = channel {
            // Its process exits; the others still run until every thread
            // has seen its own channel end.
            let ended = channel
                .sender
                .send(b"bye")
                .map_err(|error| error.to_string());
            failures.check("send", ended);
            let end = runtime.block_on(async move {
                let end = tokio::task::spawn_blocking(move || channel.receiver.recv());
                tokio::time::timeout(HANG_GUARD, end).await
            });
            failures.check(
                "end",
                match end {
                    Ok(Ok(Err(ChannelError::Closed { reason })))
                        if reason == "the process exited normally" =>
                    {
                        Ok(())
                    }
                    Ok(other) => Err(format!("unexpected end {other:?}")),
                    Err(_) => Err(format!(
                        "the channel did not end within {HANG_GUARD:?} after its process exited: another process holds its socket"
                    )),
                },
            );
        }
        failures.check("line up", barrier.wait());
        failures.report(number, thread);
    });
}

/// guarantee: bridge/spawned-processes-own-channel (issue #184, risk C11,
/// Q6.5.2): Node runtimes launched at once through the `Process` facade
/// each start and hold their own channel only.
#[test]
fn node_runtimes_launched_at_once_hold_only_their_own_channel() {
    if !rutis_test_support::runtimes::enabled("node") {
        return;
    }
    use rutis_bridge::runtime::Process;
    use serde_json::json;
    use std::io::Write;

    let mut plugin = tempfile::Builder::new().suffix(".mjs").tempfile().unwrap();
    plugin
        .write_all(
            br#"
      export function apply(ctx) {
        ctx.provide('probe', {
          pid() { return process.pid },
          exit() { setImmediate(() => process.exit(0)) },
        })
      }
    "#,
        )
        .unwrap();
    let plugin = Arc::new(plugin);
    let package = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../node/rutis-runtime");
    rounds(move |number, thread, barrier| {
        let runtime = runtime();
        let mut failures = Failures(Vec::new());
        failures.check("line up", barrier.wait());
        let process = runtime.block_on(Process::launch(
            &package,
            plugin.path(),
            json!({}),
            json!({ "probe": ["pid", "exit"] }),
        ));
        let process = match process {
            Ok(process) => Some(process),
            Err(error) => {
                failures.check("launch", Err(error.to_string()));
                None
            }
        };
        let pid = process.as_ref().and_then(|process| {
            match runtime.block_on(process.call_async("probe", "pid", json!([]))) {
                Ok(pid) => pid.as_u64().map(|pid| pid as u32),
                Err(error) => {
                    failures.check("pid", Err(error.to_string()));
                    None
                }
            }
        });
        failures.check("line up", barrier.wait());
        // Node opens pipes of its own; a socket other than fd 3 is a stray.
        // Over loopback its channel is a socket on any fd: only the end of
        // each session is checked.
        if let Some(pid) = pid.filter(|_| !loopback()) {
            let strays = strays(pid, false);
            failures.check(
                "descriptors",
                match strays.is_empty() {
                    true => Ok(()),
                    false => Err(format!("process {pid} holds {}", strays.join(", "))),
                },
            );
        }
        failures.check("line up", barrier.wait());
        if let Some(process) = process {
            // The reply may lose the race with the exit: either is fine.
            let _ = runtime.block_on(process.call_async("probe", "exit", json!([])));
            let closed = runtime
                .block_on(async { tokio::time::timeout(HANG_GUARD, process.closed()).await });
            failures.check(
                "end",
                closed.map_err(|_| format!(
                    "the session did not end within {HANG_GUARD:?} after its process exited: another process holds its socket"
                )),
            );
        }
        failures.check("line up", barrier.wait());
        failures.report(number, thread);
    });
}
