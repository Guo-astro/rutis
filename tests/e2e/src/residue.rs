//! The residue checks every scenario ends with (quality standard Q5.4.2;
//! the proposed checks of quality-status §5):
//!
//! - every process a host started has exited: Linux, the host's process
//!   group in /proc, and, with this process a child subreaper, orphans that
//!   left the group; macOS (other Unix), the group in `ps`; Windows: not
//!   yet (a Job Object around each host, #232), reported as skipped;
//! - no socket file is left in the scenario directory;
//! - every port the scenario reserved can be bound again;
//! - the hosts' temporary directory is empty;
//! - no captured output contains a credential of the scenario.
//!
//! Orphans that left their host's group carry nothing that ties them to a
//! scenario: they are found as children of this test process that are no
//! host (nor the `cargo build` of the host). A scenario reports every such
//! orphan, also another scenario's when scenarios of one test binary run in
//! parallel: the failure is real, its attribution may not be. Run one
//! scenario per binary, or `--test-threads=1`, to attribute it.

use std::collections::HashSet;
use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::host::Started;

/// What the residue checks found.
#[derive(Debug, Default)]
pub struct Residue {
    /// What is left over, one item each.
    pub found: Vec<String>,
    /// Checks not run on this platform, and why (Q7.8).
    pub skipped: Vec<String>,
}

impl Residue {
    pub fn report(&self) -> String {
        self.found
            .iter()
            .map(|item| format!("  - {item}\n"))
            .collect()
    }
}

/// Every host pid this test process started: the hosts, not orphans.
static HOSTS: Mutex<Option<HashSet<u32>>> = Mutex::new(None);

pub(crate) fn started(pid: u32) {
    HOSTS
        .lock()
        .unwrap()
        .get_or_insert_with(HashSet::new)
        .insert(pid);
}

fn is_host(pid: u32) -> bool {
    HOSTS
        .lock()
        .unwrap()
        .as_ref()
        .is_some_and(|hosts| hosts.contains(&pid))
}

/// Make this test process the reaper of the hosts' orphans (Linux), so a
/// runtime that leaves its host's process group is still found: it becomes
/// a child of this process instead of init's.
pub(crate) fn watch_orphans() {
    #[cfg(target_os = "linux")]
    {
        static ONCE: std::sync::Once = std::sync::Once::new();
        // SAFETY: prctl(2) with integer arguments.
        ONCE.call_once(|| unsafe {
            libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0);
        });
    }
}

pub(crate) fn check(dir: &Path, hosts: &[Started], ports: &[u16], secrets: &[String]) -> Residue {
    let mut residue = Residue::default();
    stop_running(hosts, &mut residue);
    processes(hosts, crate::hang_guard(), &mut residue);
    sockets(dir, &mut residue);
    for port in ports {
        if let Err(error) = std::net::TcpListener::bind(("127.0.0.1", *port)) {
            residue
                .found
                .push(format!("port {port} cannot be bound again: {error}"));
        }
    }
    temporary(&dir.join("tmp"), &mut residue);
    for host in hosts {
        let lines = host.output.lines();
        let leaked = crate::leaked(lines.iter().map(|line| line.text.as_str()), secrets);
        for line in leaked {
            residue
                .found
                .push(format!("{} printed a credential: {line}", host.name));
        }
    }
    residue
}

#[derive(Debug)]
#[cfg_attr(not(unix), allow(dead_code))]
struct Process {
    pid: u32,
    ppid: u32,
    pgid: u32,
    zombie: bool,
    command: String,
}

/// A host still running at the end is residue; kill its group now rather
/// than wait out the hang guard for it (Unix; on Windows `Host`'s drop
/// kills it).
fn stop_running(hosts: &[Started], residue: &mut Residue) {
    let Some(processes) = list() else { return };
    for host in hosts {
        let running = processes
            .iter()
            .any(|process| process.pid == host.pid && !process.zombie);
        if running {
            residue.found.push(format!(
                "{} (pid {}) still ran when the scenario finished; killed",
                host.name, host.pid
            ));
            // SAFETY: killpg(2) on the group the scenario made for the host.
            #[cfg(unix)]
            unsafe {
                libc::killpg(host.pid as i32, libc::SIGKILL);
            }
        }
    }
}

/// Wait, up to `wait`, until no process of the hosts' groups (and no
/// orphan reparented here) runs; then list those still running.
fn processes(hosts: &[Started], wait: Duration, residue: &mut Residue) {
    let Some(_) = list() else {
        residue.skipped.push(
            "processes: not implemented on this platform; on Windows each host goes into a \
             Job Object whose process list must end empty (#232)"
                .into(),
        );
        return;
    };
    let groups: HashSet<u32> = hosts.iter().map(|host| host.pid).collect();
    let deadline = Instant::now() + wait;
    loop {
        let left = leftover(&groups);
        if left.is_empty() {
            return;
        }
        if Instant::now() >= deadline {
            for process in left {
                residue.found.push(format!(
                    "process {} still runs ({}), started by host {}",
                    process.pid,
                    process.command,
                    match groups.contains(&process.pgid) {
                        true => format!("pid {}", process.pgid),
                        false => "of this test (an orphan outside its group)".into(),
                    }
                ));
            }
            return;
        }
        // Polling the process table: there is no event for "a group ended".
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// The live processes of `groups`, and orphans reparented to this process.
fn leftover(groups: &HashSet<u32>) -> Vec<Process> {
    let me = std::process::id();
    let mut left = Vec::new();
    for process in list().unwrap_or_default() {
        let orphan = process.ppid == me && !is_host(process.pid);
        if process.zombie {
            // Exited; reap orphans that became ours.
            #[cfg(unix)]
            if orphan {
                // SAFETY: waitpid(2) on a child of this process that is no
                // host (std owns those).
                unsafe {
                    libc::waitpid(process.pid as i32, std::ptr::null_mut(), libc::WNOHANG);
                }
            }
            continue;
        }
        if groups.contains(&process.pgid) || orphan {
            left.push(process);
        }
    }
    left
}

#[cfg(target_os = "linux")]
fn list() -> Option<Vec<Process>> {
    let mut processes = Vec::new();
    for entry in std::fs::read_dir("/proc").ok()?.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse().ok())
        else {
            continue;
        };
        // `pid (comm) state ppid pgrp …`; comm may hold spaces and parens.
        let Ok(stat) = std::fs::read_to_string(entry.path().join("stat")) else {
            continue;
        };
        let Some((head, rest)) = stat.rsplit_once(") ") else {
            continue;
        };
        let fields: Vec<&str> = rest.split_whitespace().collect();
        let (Some(state), Some(ppid), Some(pgid)) = (fields.first(), fields.get(1), fields.get(2))
        else {
            continue;
        };
        let command = std::fs::read(entry.path().join("cmdline"))
            .map(|bytes| String::from_utf8_lossy(&bytes).replace('\0', " "))
            .unwrap_or_default();
        processes.push(Process {
            pid,
            ppid: ppid.parse().unwrap_or(0),
            pgid: pgid.parse().unwrap_or(0),
            zombie: *state == "Z" || *state == "X",
            command: match command.trim() {
                "" => head
                    .split_once(" (")
                    .map_or("", |(_, comm)| comm)
                    .to_owned(),
                command => command.to_owned(),
            },
        });
    }
    Some(processes)
}

#[cfg(all(unix, not(target_os = "linux")))]
fn list() -> Option<Vec<Process>> {
    let ps = std::process::Command::new("ps")
        .args(["-A", "-o", "pid=,ppid=,pgid=,stat=,command="])
        .stdout(std::process::Stdio::piped())
        .spawn()
        .ok()?;
    // ps lists itself: a child of this process, but no host's residue.
    let own = ps.id();
    let output = ps.wait_with_output().ok()?;
    let processes = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let pid = fields.next()?.parse().ok()?;
            let ppid = fields.next()?.parse().ok()?;
            let pgid = fields.next()?.parse().ok()?;
            let state = fields.next()?;
            if pid == own {
                return None;
            }
            Some(Process {
                pid,
                ppid,
                pgid,
                zombie: state.starts_with('Z'),
                command: fields.collect::<Vec<_>>().join(" "),
            })
        })
        .collect();
    Some(processes)
}

#[cfg(not(unix))]
fn list() -> Option<Vec<Process>> {
    None
}

/// Socket files anywhere in the scenario directory (links not followed:
/// `node_modules` links into the checkout).
fn sockets(dir: &Path, residue: &mut Residue) {
    let mut stack = vec![dir.to_owned()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            #[cfg(unix)]
            let socket = std::os::unix::fs::FileTypeExt::is_socket(&kind);
            // Windows: by name only, until #232 looks at the processes'
            // handles.
            #[cfg(not(unix))]
            let socket = entry.path().extension().is_some_and(|ext| ext == "sock");
            if socket {
                residue
                    .found
                    .push(format!("socket file {}", entry.path().display()));
            } else if kind.is_dir() {
                stack.push(entry.path());
            }
        }
    }
}

/// What is left in the hosts' temporary directory. Tools keep some files
/// there on purpose, across runs; they are not residue: tsx its compile
/// cache (`tsx-<uid>`), Node its own (`node-compile-cache`, which tsx turns
/// on), uv its locks (`uv-<hash>.lock`, in install scenarios).
fn temporary(tmp: &Path, residue: &mut Residue) {
    for entry in std::fs::read_dir(tmp).into_iter().flatten().flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let kept = name.starts_with("tsx-")
            || name == "node-compile-cache"
            || (name.starts_with("uv-") && name.ends_with(".lock"));
        if !kept {
            residue
                .found
                .push(format!("temporary file left: {}", entry.path().display()));
        }
    }
}

/// The checks find what they look for (Unix: they need a shell and
/// process groups).
#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::host::Host;

    /// One at a time: an orphan of one test would be residue in another.
    static SERIAL: Mutex<()> = Mutex::new(());

    fn shell(script: &str) -> (Host, Started) {
        let mut command = std::process::Command::new("sh");
        command.args(["-c", script]);
        Host::spawn("sh", command)
    }

    #[test]
    fn processes_left_in_the_group_are_found() {
        let _serial = SERIAL
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let (mut host, started) = shell("sleep 60 & echo started; wait");
        host.expect("started");
        let mut residue = Residue::default();
        processes(std::slice::from_ref(&started), Duration::ZERO, &mut residue);
        // The shell and its sleep.
        assert_eq!(residue.found.len(), 2, "{residue:?}");
        drop(host); // kills the group
        let mut residue = Residue::default();
        processes(&[started], crate::hang_guard(), &mut residue);
        assert!(residue.found.is_empty(), "{residue:?}");
    }

    #[test]
    fn a_host_still_running_is_residue_and_is_killed() {
        let _serial = SERIAL
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let (mut host, started) = shell("sleep 60 & echo started; wait");
        host.expect("started");
        let dir = crate::root().join(format!("residue-running-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let residue = check(&dir, std::slice::from_ref(&started), &[], &[]);
        // Killed, not waited out: waited out, the shell and its sleep (60 s)
        // would still run past the hang guard and be found as two processes
        // instead.
        assert_eq!(residue.found.len(), 1, "{residue:?}");
        assert!(residue.found[0].contains("still ran"), "{residue:?}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn an_orphan_that_left_the_group_is_found() {
        let _serial = SERIAL
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        watch_orphans();
        let (mut host, started) = shell("setsid sleep 60 & echo $!");
        let orphan: i32 = host
            .wait_for("its pid", |line| line.parse::<i32>().is_ok())
            .parse()
            .unwrap();
        host.wait_exit();
        let mut residue = Residue::default();
        processes(std::slice::from_ref(&started), Duration::ZERO, &mut residue);
        assert!(
            residue
                .found
                .iter()
                .any(|item| item.starts_with(&format!("process {orphan} "))),
            "{residue:?}"
        );
        // SAFETY: kill(2) on the orphan this test made; it is our child now.
        unsafe { libc::kill(orphan, libc::SIGKILL) };
        let mut residue = Residue::default();
        processes(&[started], crate::hang_guard(), &mut residue);
        assert!(residue.found.is_empty(), "{residue:?}");
    }

    #[test]
    fn sockets_ports_temporary_files_and_credentials_are_found() {
        let _serial = SERIAL
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = crate::root().join(format!("residue-self-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("tmp/tsx-0")).unwrap();
        let _socket = std::os::unix::net::UnixListener::bind(dir.join("tmp/left.sock")).unwrap();
        let held = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = held.local_addr().unwrap().port();
        let (mut host, started) = shell("echo token=e2e-secret");
        host.expect("token=");
        host.wait_exit();

        let residue = check(&dir, &[started], &[port], &["e2e-secret".into()]);
        let report = residue.report();
        assert!(report.contains("socket file"), "{report}");
        assert!(report.contains(&format!("port {port}")), "{report}");
        assert!(report.contains("temporary file left"), "{report}");
        assert!(!report.contains("tsx-0"), "{report}");
        assert!(report.contains("printed a credential"), "{report}");
        assert_eq!(residue.found.len(), 4, "{report}");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
