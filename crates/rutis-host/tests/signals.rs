//! How `rutis-host run` and `rutis-host dev` end on a signal (#174): the
//! released binary, started as a user or a service manager starts it, with a
//! Python and a Node row that record their cleanup and their process.
//!
//! guarantee: every cleanup runs exactly once on unload, also when a signal
//! ends the host; the runtime processes it started have exited when it has
//! (Q5.4.4, Q6.6.3; risks B2, B3); the exit code follows
//! docs/guide/rutis-host.md.
//!
//! Unix only: the test sends SIGINT and SIGTERM with kill(2), to the process
//! group as a terminal does on Ctrl-C and to the host alone as a service
//! manager does. The Windows console events (Ctrl-C, Ctrl-Break, closing the
//! console) need a console shared with the test process, and CI does not run
//! the rutis-host tests on Windows; they are not covered here (#186 / S8).
#![cfg(unix)]

use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use serde_json::json;

/// Only against a hang: nothing here waits for time to pass.
const HANG_GUARD: Duration = Duration::from_secs(60);

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn file_url(path: &Path) -> String {
    url::Url::from_file_path(path).unwrap().to_string()
}

/// A Python plugin: it prints its process when it starts and when its
/// cleanup starts, and records each cleanup run in `<out>/<row>.cleanup`.
/// With `hold`, its cleanup never ends, and it starts a process of its own
/// that never ends either (sharing the output, so the output ends only
/// once it is gone).
const PYTHON_PLUGIN: &str = r#"
import os, subprocess, sys, threading

def say(line):
    # One write per line: the runtimes share this output, and print() would
    # write the newline separately, letting another process's line in.
    sys.stdout.write(line + "\n")
    sys.stdout.flush()

def apply(ctx, config):
    out, row = config["out"], config["row"]
    if config.get("hold"):
        # A process of its own that never ends, sharing the output.
        child = subprocess.Popen([sys.executable, "-c", "import threading; threading.Event().wait()"])
        say(f"plugin {row} started {child.pid}")
    say(f"plugin {row} applied in {os.getpid()}")
    def cleanup():
        say(f"plugin {row} cleanup started")
        if config.get("hold"):
            threading.Event().wait()
        with open(os.path.join(out, row + ".cleanup"), "a") as f:
            f.write("cleanup\n")
    ctx.effect(cleanup)
"#;

/// A Python plugin whose apply never ends.
const HANGING_PLUGIN: &str = r#"
import os, threading

def apply(ctx, config):
    print(f"plugin {config['row']} hangs in {os.getpid()}", flush=True)
    threading.Event().wait()
"#;

/// The same in JavaScript.
fn node_plugin() -> String {
    let sdk = repo()
        .join("node/rutis/src/index.mjs")
        .canonicalize()
        .unwrap();
    format!(
        "import {{ definePlugin }} from '{}'\n\
         import {{ appendFileSync }} from 'node:fs'\n\
         import {{ join }} from 'node:path'\n\
         export default definePlugin({{ apply(ctx, config) {{\n\
           console.log(`plugin ${{config.row}} applied in ${{process.pid}}`)\n\
           return async () => {{\n\
             console.log(`plugin ${{config.row}} cleanup started`)\n\
             if (config.hold) await new Promise(() => {{}})\n\
             appendFileSync(join(config.out, config.row + '.cleanup'), 'cleanup\\n')\n\
           }}\n\
         }} }})\n",
        file_url(&sdk)
    )
}

/// A project for `rutis-host run`: a Python row and a Node row.
fn run_project(dir: &Path, hold: bool) -> PathBuf {
    std::fs::write(dir.join("marker.py"), PYTHON_PLUGIN).unwrap();
    std::fs::write(dir.join("marker.mjs"), node_plugin()).unwrap();
    std::fs::write(
        dir.join("package.json"),
        r#"{ "name": "signals", "type": "module" }"#,
    )
    .unwrap();
    let out = dir.to_str().unwrap();
    let mut py = json!({ "project": "." });
    if let Some(python) = std::env::var_os("RUTIS_PYTHON") {
        py["python"] = json!(python.to_str().unwrap());
    }
    let config = json!({
        "id": "signals",
        "runtimes": {
            "py": py,
            "node": { "project": ".", "runtime": repo().join("node/rutis-runtime") },
        },
        "rows": [
            { "id": "py-row", "name": "py:marker", "config": { "out": out, "row": "py-row", "hold": hold } },
            { "id": "node-row", "name": "./marker.mjs", "config": { "out": out, "row": "node-row", "hold": hold } },
        ]
    });
    let file = dir.join("rutis.json");
    std::fs::write(&file, serde_json::to_string_pretty(&config).unwrap()).unwrap();
    file
}

/// A Python plugin project for `rutis-host dev`.
fn dev_project(dir: &Path) {
    std::fs::write(dir.join("marker.py"), PYTHON_PLUGIN).unwrap();
    std::fs::write(
        dir.join("pyproject.toml"),
        "[project]\nname = \"marker\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    let config = json!({
        "rows": [{ "id": "marker", "config": { "out": dir, "row": "marker" } }]
    });
    std::fs::write(dir.join("rutis.dev.json"), config.to_string()).unwrap();
}

/// The host binary, running in a process group of its own (as a shell
/// starts a foreground job), its output read line by line.
struct Host {
    child: Child,
    lines: Receiver<String>,
    stderr: Receiver<String>,
    /// Every line read so far, for the failure message.
    seen: Vec<String>,
    /// The runtime processes seen: in process groups of their own, they
    /// are killed with the host when the test fails.
    runtimes: Vec<i32>,
}

impl Host {
    fn start(args: &[&str], dir: &Path) -> Self {
        Self::start_ignoring(args, dir, None)
    }

    /// [`Host::start`], with the signal `ignored` ignored, as `nohup` (or
    /// a shell starting a script's background job) does.
    fn start_ignoring(args: &[&str], dir: &Path, ignored: Option<i32>) -> Self {
        use std::os::unix::process::CommandExt;
        let mut command = Command::new(env!("CARGO_BIN_EXE_rutis-host"));
        command
            .args(args)
            .current_dir(dir)
            // The repository's rutis package, for an interpreter without it.
            .env("RUTIS_PYTHON_PATH", repo().join("python/rutis"))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0);
        if let Some(python) = std::env::var_os("RUTIS_PYTHON") {
            // What `dev` runs when the project has no .venv.
            let venv = Path::new(&python).parent().and_then(Path::parent);
            if let Some(venv) = venv.filter(|venv| venv.join("pyvenv.cfg").exists()) {
                command.env("VIRTUAL_ENV", venv);
            }
        }
        if let Some(signal) = ignored {
            // SAFETY: only signal(2), which is async-signal-safe, between
            // fork and exec.
            unsafe {
                command.pre_exec(move || {
                    libc::signal(signal, libc::SIG_IGN);
                    Ok(())
                });
            }
        }
        let mut child = command.spawn().expect("the rutis-host binary");
        let (send, lines) = mpsc::channel();
        let stdout = child.stdout.take().unwrap();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if send.send(line).is_err() {
                    break;
                }
            }
        });
        let (send, stderr) = mpsc::channel();
        let mut error = child.stderr.take().unwrap();
        std::thread::spawn(move || {
            let mut text = String::new();
            let _ = error.read_to_string(&mut text);
            let _ = send.send(text);
        });
        Self {
            child,
            lines,
            stderr,
            seen: Vec::new(),
            runtimes: Vec::new(),
        }
    }

    fn pid(&self) -> i32 {
        self.child.id() as i32
    }

    /// Read lines until one satisfies `want`; what it returns.
    fn until<T>(&mut self, mut want: impl FnMut(&str) -> Option<T>) -> T {
        loop {
            match self.lines.recv_timeout(HANG_GUARD) {
                Ok(line) => {
                    let found = want(&line);
                    // A plugin's own process, killed too if the test fails.
                    if let Some((_, pid)) = line.split_once(" started ") {
                        self.runtimes.extend(pid.trim().parse::<i32>().ok());
                    }
                    self.seen.push(line);
                    if let Some(found) = found {
                        return found;
                    }
                }
                Err(error) => self.fail(&format!("waiting for a line: {error}")),
            }
        }
    }

    /// The process of each row as it starts, once every row runs.
    fn started(&mut self, rows: &[&str]) -> Vec<i32> {
        let mut pids = Vec::new();
        let mut running = 0;
        while pids.len() < rows.len() || running < rows.len() {
            let (pid, ran) = self.until(|line| {
                let applied = rows.iter().find_map(|row| {
                    line.strip_prefix(&format!("plugin {row} applied in "))
                        .and_then(|pid| pid.trim().parse::<i32>().ok())
                });
                let ran = rows.iter().any(|row| line == format!("{row}: running"));
                (applied.is_some() || ran).then_some((applied, ran))
            });
            pids.extend(pid);
            self.runtimes.extend(pid);
            running += ran as usize;
        }
        pids
    }

    /// Wait for the host to exit; its status and what it wrote to stderr.
    /// Its output is read to the end: every process that shared it has
    /// closed it.
    fn exit(mut self) -> (ExitStatus, String, Vec<String>) {
        let pid = self.pid();
        let (send, exited) = mpsc::channel();
        let mut child = self.child;
        std::thread::spawn(move || {
            let _ = send.send(child.wait());
        });
        let status = match exited.recv_timeout(HANG_GUARD) {
            Ok(status) => status.unwrap(),
            Err(_) => {
                kill_all(pid, &self.runtimes);
                panic!("rutis-host did not exit; its output: {:#?}", self.seen);
            }
        };
        let stderr = self.stderr.recv_timeout(HANG_GUARD);
        loop {
            match self.lines.recv_timeout(HANG_GUARD) {
                Ok(line) => self.seen.push(line),
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Err(error) => {
                    kill_all(pid, &self.runtimes);
                    panic!("the output did not end: {error}; {:#?}", self.seen)
                }
            }
        }
        (status, stderr.unwrap_or_default(), self.seen)
    }

    fn fail(&mut self, what: &str) -> ! {
        kill_all(self.pid(), &self.runtimes);
        let _ = self.child.wait();
        let stderr = self.stderr.recv_timeout(HANG_GUARD).unwrap_or_default();
        panic!("{what}; output: {:#?}; stderr: {stderr}", self.seen);
    }
}

/// Kill the host's process group and the runtime processes, so a failing
/// test leaves nothing behind.
fn kill_all(host: i32, runtimes: &[i32]) {
    // SAFETY: plain kill(2).
    unsafe {
        libc::kill(-host, libc::SIGKILL);
        for pid in runtimes {
            libc::kill(*pid, libc::SIGKILL);
        }
    }
}

/// Send `signal` to the host's process group (`group`), as a terminal does
/// on Ctrl-C, or to the host alone, as a service manager does.
fn signal(host: &Host, signal: i32, group: bool) {
    let target = if group { -host.pid() } else { host.pid() };
    // SAFETY: plain kill(2).
    assert_eq!(unsafe { libc::kill(target, signal) }, 0, "kill");
}

/// Whether the process `pid` still exists (a zombie counts: nothing reaped
/// it).
fn alive(pid: i32) -> bool {
    // SAFETY: signal 0 only checks that the process exists.
    unsafe { libc::kill(pid, 0) == 0 }
}

fn cleanups(dir: &Path, row: &str) -> usize {
    std::fs::read_to_string(dir.join(format!("{row}.cleanup")))
        .map(|text| text.lines().count())
        .unwrap_or(0)
}

/// `run` ended by `signal`: every cleanup ran once, the runtimes exited,
/// the host exited 0.
fn run_ends_cleanly(sig: i32, group: bool) {
    let dir = tempfile::tempdir().unwrap();
    let file = run_project(dir.path(), false);
    let mut host = Host::start(&["run", file.to_str().unwrap()], dir.path());
    let pids = host.started(&["py-row", "node-row"]);
    signal(&host, sig, group);
    let (status, stderr, output) = host.exit();
    let context = format!("status {status:?}; output: {output:#?}; stderr: {stderr}");
    assert_eq!(status.code(), Some(0), "{context}");
    for row in ["py-row", "node-row"] {
        assert_eq!(cleanups(dir.path(), row), 1, "{row}'s cleanup; {context}");
    }
    for pid in pids {
        assert!(
            !alive(pid),
            "runtime process {pid} outlived the host; {context}"
        );
    }
}

/// Ctrl-C in a terminal: SIGINT reaches the host and every process of its
/// group.
#[test]
fn run_ends_on_ctrl_c_after_every_cleanup() {
    run_ends_cleanly(libc::SIGINT, true);
}

/// A service manager stopping the host: SIGTERM to the host.
#[test]
fn run_ends_on_sigterm_after_every_cleanup() {
    run_ends_cleanly(libc::SIGTERM, false);
}

/// The terminal closing: SIGHUP to the host.
#[test]
fn run_ends_on_sighup_after_every_cleanup() {
    run_ends_cleanly(libc::SIGHUP, false);
}

/// A signal while the rows still start (one hangs in apply) ends the host
/// too: the deadline passes, so 2, and the runtime process is gone.
#[test]
fn a_signal_while_starting_ends_the_host() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("hangs.py"), HANGING_PLUGIN).unwrap();
    let mut py = json!({ "project": "." });
    if let Some(python) = std::env::var_os("RUTIS_PYTHON") {
        py["python"] = json!(python.to_str().unwrap());
    }
    let config = json!({
        "runtimes": { "py": py },
        "rows": [{ "id": "py-row", "name": "py:hangs", "config": { "row": "py-row" } }]
    });
    let file = dir.path().join("rutis.json");
    std::fs::write(&file, config.to_string()).unwrap();
    let mut host = Host::start(
        &["run", "--shutdown-timeout", "1", file.to_str().unwrap()],
        dir.path(),
    );
    let pid = host.until(|line| {
        line.strip_prefix("plugin py-row hangs in ")
            .and_then(|pid| pid.trim().parse::<i32>().ok())
    });
    host.runtimes.push(pid);
    signal(&host, libc::SIGTERM, false);
    let (status, stderr, output) = host.exit();
    let context = format!("status {status:?}; output: {output:#?}; stderr: {stderr}");
    assert_eq!(status.code(), Some(2), "{context}");
    // Row states are printed only once the host has started.
    assert!(
        !output.iter().any(|line| line.starts_with("py-row:")),
        "{context}"
    );
    assert!(
        !alive(pid),
        "runtime process {pid} outlived the host; {context}"
    );
}

/// `dev` reloading a plugin whose new code hangs in apply still ends on
/// Ctrl-C (past the deadline, so 2), and the runtime process with it.
#[test]
fn dev_ends_on_ctrl_c_while_a_reload_hangs() {
    let dir = tempfile::tempdir().unwrap();
    dev_project(dir.path());
    let mut host = Host::start(&["dev", "--shutdown-timeout", "1", "."], dir.path());
    let pids = host.started(&["marker"]);
    std::fs::write(dir.path().join("marker.py"), HANGING_PLUGIN).unwrap();
    host.until(|line| line.starts_with("plugin marker hangs in ").then_some(()));
    signal(&host, libc::SIGINT, true);
    let (status, stderr, output) = host.exit();
    let context = format!("status {status:?}; output: {output:#?}; stderr: {stderr}");
    assert_eq!(status.code(), Some(2), "{context}");
    for pid in pids {
        assert!(
            !alive(pid),
            "runtime process {pid} outlived the host; {context}"
        );
    }
}

#[test]
fn dev_ends_on_ctrl_c_after_its_cleanup() {
    let dir = tempfile::tempdir().unwrap();
    dev_project(dir.path());
    let mut host = Host::start(&["dev", "."], dir.path());
    let pids = host.started(&["marker"]);
    signal(&host, libc::SIGINT, true);
    let (status, stderr, output) = host.exit();
    let context = format!("status {status:?}; output: {output:#?}; stderr: {stderr}");
    assert_eq!(status.code(), Some(0), "{context}");
    assert_eq!(cleanups(dir.path(), "marker"), 1, "{context}");
    for pid in pids {
        assert!(
            !alive(pid),
            "runtime process {pid} outlived the host; {context}"
        );
    }
}

/// A cleanup that never ends: past the deadline the host exits 2, names
/// what was still stopping, and ends the runtime processes anyway, with
/// the processes they started (the output ends).
#[test]
fn run_past_the_cleanup_deadline_exits_2_and_ends_the_runtimes() {
    let dir = tempfile::tempdir().unwrap();
    let file = run_project(dir.path(), true);
    let mut host = Host::start(
        &["run", "--shutdown-timeout", "1", file.to_str().unwrap()],
        dir.path(),
    );
    let pids = host.started(&["py-row", "node-row"]);
    signal(&host, libc::SIGTERM, false);
    let (status, stderr, output) = host.exit();
    let context = format!("status {status:?}; output: {output:#?}; stderr: {stderr}");
    assert_eq!(status.code(), Some(2), "{context}");
    assert!(stderr.contains("did not finish"), "{context}");
    for pid in pids {
        assert!(
            !alive(pid),
            "runtime process {pid} outlived the host; {context}"
        );
    }
}

/// A second Ctrl-C while cleanups run ends the host at once, with 2, and
/// the runtime processes with it.
#[test]
fn a_second_ctrl_c_exits_at_once() {
    let dir = tempfile::tempdir().unwrap();
    let file = run_project(dir.path(), true);
    // A deadline the test never reaches: only the second Ctrl-C ends it.
    let mut host = Host::start(
        &["run", "--shutdown-timeout", "600", file.to_str().unwrap()],
        dir.path(),
    );
    let pids = host.started(&["py-row", "node-row"]);
    signal(&host, libc::SIGINT, true);
    // Rows unload one after another: the first cleanup holds the others.
    host.until(|line| line.ends_with(" cleanup started").then_some(()));
    signal(&host, libc::SIGINT, true);
    let (status, stderr, output) = host.exit();
    let context = format!("status {status:?}; output: {output:#?}; stderr: {stderr}");
    assert_eq!(status.code(), Some(2), "{context}");
    for pid in pids {
        assert!(
            !alive(pid),
            "runtime process {pid} outlived the host; {context}"
        );
    }
}

/// Started ignoring SIGHUP (`nohup`), the host keeps ignoring it: the
/// SIGTERM after it is what stops the host, which then ends normally.
#[test]
fn a_signal_ignored_when_the_host_started_stays_ignored() {
    let dir = tempfile::tempdir().unwrap();
    let file = run_project(dir.path(), false);
    let mut host = Host::start_ignoring(
        &["run", file.to_str().unwrap()],
        dir.path(),
        Some(libc::SIGHUP),
    );
    let pids = host.started(&["py-row", "node-row"]);
    signal(&host, libc::SIGHUP, false);
    signal(&host, libc::SIGTERM, false);
    let (status, stderr, output) = host.exit();
    let context = format!("status {status:?}; output: {output:#?}; stderr: {stderr}");
    assert_eq!(status.code(), Some(0), "{context}");
    assert!(stderr.contains("SIGTERM: stopping"), "{context}");
    assert!(!stderr.contains("SIGHUP"), "{context}");
    for pid in pids {
        assert!(
            !alive(pid),
            "runtime process {pid} outlived the host; {context}"
        );
    }
}

/// The processes `parent` started, as pgrep(1) lists them.
fn children(parent: i32) -> Vec<i32> {
    let output = Command::new("pgrep")
        .args(["-P", &parent.to_string()])
        .output()
        .expect("pgrep");
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|pid| pid.trim().parse().ok())
        .collect()
}

/// A Go row's runtime ends with the host on Ctrl-C too: out of the
/// terminal's process group, it is unloaded over its channel, and the host
/// waits for it to exit (run in the Go CI job: the name has `go`).
#[test]
fn run_with_a_go_row_ends_on_ctrl_c() {
    let dir = tempfile::tempdir().unwrap();
    let plugins = dir.path().join("plugins");
    std::fs::create_dir_all(&plugins).unwrap();
    let status = Command::new("go")
        .args(["build", "-o"])
        .arg(plugins.join("netkit"))
        .arg("./cmd/netkit")
        .current_dir(repo().join("crates/rutis-loader/tests/fixtures/go"))
        .status()
        .expect("go is on PATH");
    assert!(status.success());
    let config = json!({
        "runtimes": { "go": { "dir": "plugins" } },
        "rows": [{ "id": "ping", "name": "go:ping" }]
    });
    let file = dir.path().join("rutis.json");
    std::fs::write(&file, config.to_string()).unwrap();
    let mut host = Host::start(&["run", file.to_str().unwrap()], dir.path());
    host.until(|line| (line == "ping: running").then_some(()));
    let pids = children(host.pid());
    assert!(!pids.is_empty(), "the Go runtime process");
    host.runtimes.extend(&pids);
    signal(&host, libc::SIGINT, true);
    let (status, stderr, output) = host.exit();
    let context = format!("status {status:?}; output: {output:#?}; stderr: {stderr}");
    assert_eq!(status.code(), Some(0), "{context}");
    for pid in pids {
        assert!(
            !alive(pid),
            "runtime process {pid} outlived the host; {context}"
        );
    }
}
