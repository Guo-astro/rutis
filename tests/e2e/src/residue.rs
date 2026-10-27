//! The residue checks every scenario ends with (quality standard Q5.4.2;
//! the proposed checks of quality-status §5):
//!
//! - every process a host started has exited. Unix: the processes whose
//!   environment carries the scenario's [`MARKER`](crate::MARKER), wherever
//!   they are: rutis-host puts each runtime in a process group of its own,
//!   and a runtime that outlives its host is reparented to init (launchd).
//!   Linux reads `/proc/<pid>/environ`, macOS `KERN_PROCARGS2`. macOS hides
//!   the environment of Apple's own programs (`/bin/sh`, `/bin/sleep`), so
//!   the processes of the hosts' groups, and the children of everything
//!   found, count too; such a program is missed only once it is orphaned
//!   outside its host's group. Windows: the processes in each host's Job
//!   Object;
//! - no socket file is left in the scenario directory (Windows: files named
//!   `*.sock`, the name rutis gives them; the file type does not say);
//! - every port the scenario reserved, its proxies' among them, can be
//!   bound again;
//! - the hosts' temporary directory is empty;
//! - no captured output contains a credential of the scenario.
//!
//! On Linux this test process is also the reaper of the hosts' orphans (a
//! child subreaper), so the orphans that exit do not stay zombies of init's
//! and their exit is seen here.

use std::collections::HashSet;
use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::host::{Host, Started};

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

#[cfg(unix)]
fn is_host(pid: u32) -> bool {
    HOSTS
        .lock()
        .unwrap()
        .as_ref()
        .is_some_and(|hosts| hosts.contains(&pid))
}

/// Make this test process the reaper of the hosts' orphans (Linux): a
/// runtime whose host died becomes a child of this process instead of
/// init's, and is reaped here once it exits.
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

/// The checks for the scenario `scenario` (its id, which its hosts'
/// markers start with).
pub(crate) fn check(
    scenario: &str,
    dir: &Path,
    hosts: &[Started],
    ports: &[u16],
    secrets: &[String],
    wait: Duration,
) -> Residue {
    let mut residue = Residue::default();
    stop_running(hosts, &mut residue);
    let left = processes(scenario, hosts, wait, &mut residue);
    // Reported, then ended: a failed scenario leaves nothing running.
    for process in &left {
        // SAFETY: kill(2) on a process just listed as running and the
        // scenario's.
        #[cfg(unix)]
        unsafe {
            libc::kill(process.pid as i32, libc::SIGKILL);
        }
        #[cfg(not(unix))]
        let _ = process;
    }
    #[cfg(windows)]
    for host in hosts {
        host.job.terminate();
    }
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

/// A process left over, and whose it is.
struct Left {
    pid: u32,
    command: String,
    /// The host it was started by, as far as known.
    host: String,
}

/// A host still running at the end is residue; kill it with its group
/// (Unix) or job (Windows) now rather than wait out the hang guard for it.
fn stop_running(hosts: &[Started], residue: &mut Residue) {
    #[cfg(unix)]
    let processes = list();
    for host in hosts {
        #[cfg(unix)]
        let running = processes
            .iter()
            .any(|process| process.pid == host.pid && !process.zombie);
        #[cfg(windows)]
        let running = host
            .job
            .processes()
            .is_ok_and(|pids| pids.contains(&host.pid));
        #[cfg(not(any(unix, windows)))]
        let running = false;
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
            #[cfg(windows)]
            host.job.terminate();
        }
    }
}

/// Wait, up to `wait`, until no process of the scenario runs; then list
/// those still running.
fn processes(
    scenario: &str,
    hosts: &[Started],
    wait: Duration,
    residue: &mut Residue,
) -> Vec<Left> {
    if cfg!(not(any(unix, windows))) {
        residue
            .skipped
            .push("processes: no process listing on this platform".into());
        return Vec::new();
    }
    let deadline = Instant::now() + wait;
    loop {
        let left = leftover(scenario, hosts);
        if left.is_empty() {
            return left;
        }
        if Instant::now() >= deadline {
            for process in &left {
                residue.found.push(format!(
                    "process {} still runs ({}), started by {}",
                    process.pid, process.command, process.host
                ));
            }
            return left;
        }
        // Polling the process table: there is no event for "a process of
        // the scenario ended".
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// The scenario's processes still running.
#[cfg(unix)]
fn leftover(scenario: &str, hosts: &[Started]) -> Vec<Left> {
    let processes = list();
    reap(&processes);
    let prefix = format!("{scenario}/");
    let pids: Vec<u32> = hosts.iter().map(|host| host.pid).collect();
    members(&processes, &|marker| marker.starts_with(&prefix), &pids)
        .into_iter()
        .map(|process| Left {
            pid: process.pid,
            command: process.command.clone(),
            host: match &process.marker {
                Some(marker) => marker[prefix.len()..].to_owned(),
                None => "a host of the scenario (its group or parent; it shows no \
                         environment)"
                    .to_owned(),
            },
        })
        .collect()
}

#[cfg(windows)]
fn leftover(_scenario: &str, hosts: &[Started]) -> Vec<Left> {
    let mut left = Vec::new();
    for host in hosts {
        let pids = host.job.processes().unwrap_or_else(|error| {
            panic!("cannot list the processes in {}'s job: {error}", host.name)
        });
        left.extend(pids.into_iter().map(|pid| Left {
            pid,
            command: crate::job::image(pid),
            host: host.name.clone(),
        }));
    }
    left
}

#[cfg(not(any(unix, windows)))]
fn leftover(_scenario: &str, _hosts: &[Started]) -> Vec<Left> {
    Vec::new()
}

/// What `host` started that still runs ([`crate::Scenario::descendants`]).
pub(crate) fn descendants(host: &Host) -> Vec<(u32, String)> {
    #[cfg(unix)]
    {
        let processes = list();
        let marker = host.marker();
        members(&processes, &|found| found == marker, &[host.pid()])
            .into_iter()
            .filter(|process| process.pid != host.pid())
            .map(|process| (process.pid, process.command.clone()))
            .collect()
    }
    #[cfg(windows)]
    {
        let pids = host
            .job()
            .processes()
            .unwrap_or_else(|error| panic!("cannot list the processes in the host's job: {error}"));
        pids.into_iter()
            .filter(|&pid| pid != host.pid())
            .map(|pid| (pid, crate::job::image(pid)))
            .collect()
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = host;
        Vec::new()
    }
}

#[derive(Debug)]
#[cfg(unix)]
struct Process {
    pid: u32,
    ppid: u32,
    pgid: u32,
    zombie: bool,
    command: String,
    /// Its [`MARKER`](crate::MARKER), if its environment is readable and
    /// has one.
    marker: Option<String>,
}

/// The live processes of `processes` that belong to the hosts `hosts`:
/// those whose marker is `ours`; the hosts and the processes of their
/// groups; and the children of all these, transitively. A process with
/// another marker never belongs (another scenario's, or another host's).
///
/// The hosts' ids count only while they still name the hosts: a host's
/// pid while it is a child of this process (running, or exited and not
/// yet waited for); its group while no other process has its pid (a pid
/// is not given out while a group of that id has members). Once a host
/// has been waited for, its pid may be another program's.
#[cfg(unix)]
fn members<'a>(
    processes: &'a [Process],
    ours: &dyn Fn(&str) -> bool,
    hosts: &[u32],
) -> Vec<&'a Process> {
    let me = std::process::id();
    let holder = |pid: u32| processes.iter().find(|process| process.pid == pid);
    let pids: Vec<u32> = hosts
        .iter()
        .copied()
        .filter(|&pid| holder(pid).is_some_and(|process| process.ppid == me))
        .collect();
    let groups: Vec<u32> = hosts
        .iter()
        .copied()
        .filter(|&pid| pids.contains(&pid) || holder(pid).is_none())
        .collect();
    let candidate =
        |process: &Process| !process.zombie && process.marker.as_deref().is_none_or(ours);
    let mut found: HashSet<u32> = processes
        .iter()
        .filter(|process| candidate(process))
        .filter(|process| {
            process.marker.is_some()
                || pids.contains(&process.pid)
                || groups.contains(&process.pgid)
        })
        .map(|process| process.pid)
        .collect();
    loop {
        let children: Vec<u32> = processes
            .iter()
            .filter(|process| {
                candidate(process) && !found.contains(&process.pid) && found.contains(&process.ppid)
            })
            .map(|process| process.pid)
            .collect();
        if children.is_empty() {
            break;
        }
        found.extend(children);
    }
    processes
        .iter()
        .filter(|process| found.contains(&process.pid))
        .collect()
}

/// Reap the orphans that became children of this process (Linux, as their
/// subreaper) and have exited.
#[cfg(unix)]
fn reap(processes: &[Process]) {
    let me = std::process::id();
    for process in processes {
        if process.zombie && process.ppid == me && !is_host(process.pid) {
            // SAFETY: waitpid(2) on a child of this process that is no
            // host (std owns those).
            unsafe {
                libc::waitpid(process.pid as i32, std::ptr::null_mut(), libc::WNOHANG);
            }
        }
    }
}

/// The marker in an environment block (`NAME=value` strings).
#[cfg(unix)]
fn marker<'a>(mut environment: impl Iterator<Item = &'a [u8]>) -> Option<String> {
    let name = format!("{}=", crate::MARKER);
    environment.find_map(|variable| {
        variable
            .strip_prefix(name.as_bytes())
            .map(|value| String::from_utf8_lossy(value).into_owned())
    })
}

#[cfg(target_os = "linux")]
fn list() -> Vec<Process> {
    let mut processes = Vec::new();
    let Ok(entries) = std::fs::read_dir("/proc") else {
        panic!("cannot list processes: /proc is not readable");
    };
    for entry in entries.flatten() {
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
        // Readable for this user's processes.
        let environment = std::fs::read(entry.path().join("environ")).unwrap_or_default();
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
            marker: marker(environment.split(|&byte| byte == 0)),
        });
    }
    processes
}

/// libproc for the process table, `KERN_PROCARGS2` for each one's
/// arguments and environment (`ps` shows no environment of other
/// processes).
#[cfg(target_os = "macos")]
fn list() -> Vec<Process> {
    // SAFETY: proc_listallpids(3) with no buffer returns the count.
    let count = unsafe { libc::proc_listallpids(std::ptr::null_mut(), 0) };
    assert!(
        count > 0,
        "cannot list processes: {}",
        std::io::Error::last_os_error()
    );
    // Room for the processes started since.
    let mut pids = vec![0 as libc::c_int; count as usize + 256];
    // SAFETY: the buffer is as long as the size given.
    let count = unsafe {
        libc::proc_listallpids(
            pids.as_mut_ptr().cast(),
            (pids.len() * std::mem::size_of::<libc::c_int>()) as libc::c_int,
        )
    };
    assert!(
        count > 0,
        "cannot list processes: {}",
        std::io::Error::last_os_error()
    );
    pids.truncate(count as usize);
    let mut processes = Vec::new();
    for pid in pids.into_iter().filter(|&pid| pid > 0) {
        // SAFETY: zeroed is a valid proc_bsdinfo; proc_pidinfo(3) writes
        // at most its size.
        let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
        let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
        let written = unsafe {
            libc::proc_pidinfo(
                pid,
                libc::PROC_PIDTBSDINFO,
                0,
                (&mut info as *mut libc::proc_bsdinfo).cast(),
                size,
            )
        };
        if written != size {
            // Gone, or not ours to look at.
            continue;
        }
        let (arguments, marker) = procargs(pid).unwrap_or_default();
        let command = match arguments.is_empty() {
            false => arguments,
            true => {
                // SAFETY: pbi_comm is NUL-terminated within its length.
                let comm = unsafe { std::ffi::CStr::from_ptr(info.pbi_comm.as_ptr()) };
                comm.to_string_lossy().into_owned()
            }
        };
        processes.push(Process {
            pid: pid as u32,
            ppid: info.pbi_ppid,
            pgid: info.pbi_pgid,
            zombie: info.pbi_status == libc::SZOMB,
            command,
            marker,
        });
    }
    processes
}

/// The arguments of `pid`, joined, and its marker: `KERN_PROCARGS2` is
/// `argc`, the executable's path, padding, the arguments, then the
/// environment, each NUL-terminated. Apple's own programs show no
/// environment.
#[cfg(target_os = "macos")]
fn procargs(pid: libc::c_int) -> Option<(String, Option<String>)> {
    static ARGMAX: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    let argmax = *ARGMAX.get_or_init(|| {
        let mut mib = [libc::CTL_KERN, libc::KERN_ARGMAX];
        let mut value: libc::c_int = 0;
        let mut size = std::mem::size_of::<libc::c_int>();
        // SAFETY: sysctl(3) writing one c_int.
        let ok = unsafe {
            libc::sysctl(
                mib.as_mut_ptr(),
                2,
                (&mut value as *mut libc::c_int).cast(),
                &mut size,
                std::ptr::null_mut(),
                0,
            )
        };
        match ok {
            0 => value as usize,
            _ => 1 << 20,
        }
    });
    // One buffer per thread, reused: the residue checks list every
    // process every 50 ms while they wait.
    thread_local! {
        static BUFFER: std::cell::RefCell<Vec<u8>> = const { std::cell::RefCell::new(Vec::new()) };
    }
    BUFFER.with_borrow_mut(|buffer| {
        buffer.resize(argmax, 0);
        procargs_into(pid, buffer)
    })
}

#[cfg(target_os = "macos")]
fn procargs_into(pid: libc::c_int, buffer: &mut [u8]) -> Option<(String, Option<String>)> {
    let mut size = buffer.len();
    let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid];
    // SAFETY: sysctl(3) writing at most `size` bytes into the buffer.
    let ok = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            3,
            buffer.as_mut_ptr().cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if ok != 0 || size < 4 {
        return None;
    }
    let buffer = &buffer[..size];
    let argc = i32::from_ne_bytes(buffer[..4].try_into().unwrap()).max(0) as usize;
    let mut strings = buffer[4..].split(|&byte| byte == 0);
    let _executable = strings.next()?;
    let mut strings = strings.skip_while(|string| string.is_empty());
    let arguments: Vec<String> = (&mut strings)
        .take(argc)
        .map(|argument| String::from_utf8_lossy(argument).into_owned())
        .collect();
    let marker = marker(strings.take_while(|string| !string.is_empty()));
    Some((arguments.join(" "), marker))
}

/// Other Unix: `ps`, without environments; the hosts' groups and their
/// children only.
#[cfg(all(unix, not(any(target_os = "linux", target_os = "macos"))))]
fn list() -> Vec<Process> {
    let ps = std::process::Command::new("ps")
        .args(["-A", "-o", "pid=,ppid=,pgid=,stat=,command="])
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("cannot list processes: ps");
    // ps lists itself: a child of this process, but no host's residue.
    let own = ps.id();
    let output = ps.wait_with_output().expect("cannot list processes: ps");
    String::from_utf8_lossy(&output.stdout)
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
                marker: None,
            })
        })
        .collect()
}

/// Socket files anywhere in the scenario directory (links not followed:
/// `node_modules` links into the checkout), but rutis-host's `status`
/// sockets a killed host leaves (see [`temporary`]).
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
            // Windows: by name only. An AF_UNIX socket file there is a
            // reparse point the file type does not tell apart; rutis names
            // the ones it makes `*.sock`, and a host's processes are gone
            // (its job is empty) by the time this runs.
            #[cfg(not(unix))]
            let socket = entry.path().extension().is_some_and(|ext| ext == "sock");
            let status = dir
                .file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with("rutis-host-"));
            if socket && !status {
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
/// on), uv its locks (`uv-<hash>.lock`, in install scenarios), the Swift
/// toolchain (`swift build` in a Swift project, run by its tests and by
/// `dev`) its build's directories (`TemporaryDirectory.<random>`,
/// `swift-generated-sources`) and SwiftPM its locks (`_<a path>.lock`).
/// rutis-host keeps its `status` sockets in `rutis-host-<uid>` (when there
/// is no `XDG_RUNTIME_DIR`): a host that ends removes its own, but a killed
/// one cannot, and the next host for the same configuration replaces it.
fn temporary(tmp: &Path, residue: &mut Residue) {
    for entry in std::fs::read_dir(tmp).into_iter().flatten().flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let kept = name.starts_with("tsx-")
            || name == "node-compile-cache"
            || (name.starts_with("uv-") && name.ends_with(".lock"))
            || name.starts_with("TemporaryDirectory.")
            || name == "swift-generated-sources"
            || (name.starts_with('_') && name.ends_with(".lock"))
            || name.starts_with("rutis-host-");
        if !kept {
            residue
                .found
                .push(format!("temporary file left: {}", entry.path().display()));
        }
    }
}

/// The checks find what they look for. The processes are this test binary
/// itself ([`tests::helper`]): on macOS, Apple's own programs (`sh`,
/// `sleep`) show no environment, and the checks must find processes by
/// their marker.
#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Command, Stdio};

    /// One at a time: a test's orphan reaped by another's check would be
    /// waited for by neither.
    static SERIAL: Mutex<()> = Mutex::new(());

    const HELPER: &str = "RUTIS_E2E_HELPER";

    /// Not a test of its own: the process the other tests start (this
    /// binary, running only this function), which does what
    /// `RUTIS_E2E_HELPER` says. Without it, nothing.
    #[test]
    fn helper() {
        let Ok(what) = std::env::var(HELPER) else {
            return;
        };
        match what.as_str() {
            // Run until killed.
            "wait" => {
                println!("started");
                loop {
                    std::thread::park();
                }
            }
            // Start a `wait`, in this process group, and run until killed.
            "tree" => {
                let mut child = helper_command("wait")
                    .stdout(Stdio::null())
                    .spawn()
                    .expect("the helper's child");
                println!("started");
                let _ = child.wait();
            }
            // Start a `wait` in a process group of its own (Unix) and exit:
            // it is orphaned, as a runtime whose host died.
            "orphan" => {
                let mut command = helper_command("wait");
                command
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null());
                #[cfg(unix)]
                std::os::unix::process::CommandExt::process_group(&mut command, 0);
                let child = command.spawn().expect("the helper's child");
                println!("orphan {}", child.id());
                std::process::exit(0);
            }
            other => panic!("{HELPER}={other}?"),
        }
    }

    fn helper_command(what: &str) -> Command {
        let mut command = Command::new(std::env::current_exe().expect("this test binary"));
        command
            .args([
                "--exact",
                "residue::tests::helper",
                "--nocapture",
                "--test-threads=1",
                "--quiet",
            ])
            .env(HELPER, what);
        command
    }

    fn start(scenario: &str, what: &str) -> (Host, Started) {
        Host::spawn(
            "helper",
            &format!("{scenario}/helper"),
            helper_command(what),
            false,
        )
    }

    fn serial() -> std::sync::MutexGuard<'static, ()> {
        SERIAL
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    #[test]
    fn processes_a_host_started_are_found() {
        let _serial = serial();
        let scenario = format!("residue-tree-{}", std::process::id());
        let (mut host, started) = start(&scenario, "tree");
        host.expect("started");
        let mut residue = Residue::default();
        processes(
            &scenario,
            std::slice::from_ref(&started),
            Duration::ZERO,
            &mut residue,
        );
        // The helper and its child.
        assert_eq!(residue.found.len(), 2, "{residue:?}");
        assert!(
            residue.found.iter().all(|item| item.contains("helper")),
            "{residue:?}"
        );
        drop(host); // kills its group, or its job
        let mut residue = Residue::default();
        processes(&scenario, &[started], crate::hang_guard(), &mut residue);
        assert!(residue.found.is_empty(), "{residue:?}");
    }

    #[test]
    fn a_host_still_running_is_residue_and_is_killed() {
        let _serial = serial();
        let scenario = format!("residue-running-{}", std::process::id());
        let (mut host, started) = start(&scenario, "tree");
        host.expect("started");
        let dir = crate::root().join(&scenario);
        std::fs::create_dir_all(&dir).unwrap();
        let residue = check(
            &scenario,
            &dir,
            std::slice::from_ref(&started),
            &[],
            &[],
            crate::hang_guard(),
        );
        // Killed, not waited out: waited out, the helper and its child
        // (which never exit) would be found as two more processes.
        assert_eq!(residue.found.len(), 1, "{residue:?}");
        assert!(residue.found[0].contains("still ran"), "{residue:?}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The gap the marker closes: before it, on macOS, a process that left
    /// its host's group and outlived the host was found by nothing.
    #[test]
    fn an_orphan_outside_the_group_is_found() {
        let _serial = serial();
        watch_orphans();
        let scenario = format!("residue-orphan-{}", std::process::id());
        let (mut host, started) = start(&scenario, "orphan");
        let orphan: u32 = host
            .wait_for("its child's pid", |line| line.starts_with("orphan "))
            .trim_start_matches("orphan ")
            .parse()
            .unwrap();
        host.wait_exit();
        let descendants = descendants(&host);
        assert_eq!(
            descendants.iter().map(|(pid, _)| *pid).collect::<Vec<_>>(),
            [orphan],
            "{descendants:?}"
        );
        let mut residue = Residue::default();
        processes(
            &scenario,
            std::slice::from_ref(&started),
            Duration::ZERO,
            &mut residue,
        );
        assert_eq!(residue.found.len(), 1, "{residue:?}");
        assert!(
            residue.found[0].starts_with(&format!("process {orphan} ")),
            "{residue:?}"
        );
        // SAFETY: kill(2) on the orphan this test made; it has not been
        // reaped (it runs).
        #[cfg(unix)]
        unsafe {
            libc::kill(orphan as i32, libc::SIGKILL);
        }
        #[cfg(windows)]
        started.job.terminate();
        let mut residue = Residue::default();
        processes(&scenario, &[started], crate::hang_guard(), &mut residue);
        assert!(residue.found.is_empty(), "{residue:?}");
    }

    /// Unix: a socket file is made with `UnixListener`.
    #[cfg(unix)]
    #[test]
    fn sockets_ports_temporary_files_and_credentials_are_found() {
        let _serial = serial();
        let scenario = format!("residue-self-test-{}", std::process::id());
        let dir = crate::root().join(&scenario);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("tmp/tsx-0")).unwrap();
        let _socket = std::os::unix::net::UnixListener::bind(dir.join("tmp/left.sock")).unwrap();
        let held = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = held.local_addr().unwrap().port();
        let mut command = Command::new("sh");
        command.args(["-c", "echo token=e2e-secret"]);
        let (mut host, started) = Host::spawn("sh", &format!("{scenario}/sh"), command, false);
        host.expect("token=");
        host.wait_exit();

        let residue = check(
            &scenario,
            &dir,
            &[started],
            &[port],
            &["e2e-secret".into()],
            crate::hang_guard(),
        );
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
