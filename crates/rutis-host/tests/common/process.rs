//! The `rutis-host` binary as a process a test drives (Unix): started as a
//! shell starts a foreground job, its output read line by line, signalled
//! with kill(2); and a project of a Python and a Node row that print their
//! process and record their cleanups.

use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use rutis_test_support::{scaled, HANG_GUARD};
use serde_json::json;

use super::repo;

/// Only against a hang: nothing here waits for time to pass. The host
/// starts a Python and a Node runtime cold, and on its way out may wait out
/// a deadline and its runtimes' exits (`PROCESS_EXIT`, `KILLED_EXIT`): six
/// times [`HANG_GUARD`], as before.
pub const GUARD: Duration = Duration::from_secs(6 * HANG_GUARD.as_secs());

pub fn file_url(path: &Path) -> String {
    url::Url::from_file_path(path).unwrap().to_string()
}

/// A Python plugin: it prints its process when it starts and when its
/// cleanup starts, and records each cleanup run in `<out>/<row>.cleanup`.
/// With `hold`, its cleanup never ends, and it starts a process of its own
/// that never ends either (sharing the output, so the output ends only
/// once it is gone).
pub const PYTHON_PLUGIN: &str = r#"
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
pub const HANGING_PLUGIN: &str = r#"
import os, threading

def apply(ctx, config):
    print(f"plugin {config['row']} hangs in {os.getpid()}", flush=True)
    threading.Event().wait()
"#;

/// The same in JavaScript (the marker plugin; `delay`: its cleanup takes
/// that many milliseconds).
pub fn node_plugin() -> String {
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
             if (config.delay) await new Promise(done => setTimeout(done, config.delay))\n\
             appendFileSync(join(config.out, config.row + '.cleanup'), 'cleanup\\n')\n\
           }}\n\
         }} }})\n",
        file_url(&sdk)
    )
}

/// A project for `rutis-host run`: a Python row and a Node row.
pub fn run_project(dir: &Path, hold: bool) -> PathBuf {
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
pub fn dev_project(dir: &Path) {
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
pub struct Host {
    pub child: Child,
    pub lines: Receiver<String>,
    pub stderr: Receiver<String>,
    /// Every line read so far, for the failure message.
    pub seen: Vec<String>,
    /// The runtime processes seen: in process groups of their own, they
    /// are killed with the host when the test fails.
    pub runtimes: Vec<i32>,
}

impl Host {
    pub fn start(args: &[&str], dir: &Path) -> Self {
        Self::start_ignoring(args, dir, None)
    }

    /// [`Host::start`], with the signal `ignored` ignored, as `nohup` (or
    /// a shell starting a script's background job) does.
    pub fn start_ignoring(args: &[&str], dir: &Path, ignored: Option<i32>) -> Self {
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

    pub fn pid(&self) -> i32 {
        self.child.id() as i32
    }

    /// Read lines until one satisfies `want`; what it returns.
    pub fn wait_line<T>(&mut self, mut want: impl FnMut(&str) -> Option<T>) -> T {
        loop {
            match self.lines.recv_timeout(scaled(GUARD)) {
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

    /// The process of each row as it starts, in the order of `rows`, once
    /// every row runs.
    pub fn started(&mut self, rows: &[&str]) -> Vec<i32> {
        let mut pids = vec![None; rows.len()];
        let mut running = 0;
        while pids.contains(&None) || running < rows.len() {
            let (applied, ran) = self.wait_line(|line| {
                let applied = rows.iter().enumerate().find_map(|(index, row)| {
                    line.strip_prefix(&format!("plugin {row} applied in "))
                        .and_then(|pid| pid.trim().parse::<i32>().ok())
                        .map(|pid| (index, pid))
                });
                let ran = rows.iter().any(|row| line == format!("{row}: running"));
                (applied.is_some() || ran).then_some((applied, ran))
            });
            if let Some((index, pid)) = applied {
                pids[index] = Some(pid);
                self.runtimes.push(pid);
            }
            running += ran as usize;
        }
        pids.into_iter().flatten().collect()
    }

    /// Wait for the host to exit; its status and what it wrote to stderr.
    /// Its output is read to the end: every process that shared it has
    /// closed it.
    pub fn exit(mut self) -> (ExitStatus, String, Vec<String>) {
        let pid = self.pid();
        let (send, exited) = mpsc::channel();
        let mut child = self.child;
        std::thread::spawn(move || {
            let _ = send.send(child.wait());
        });
        let status = match exited.recv_timeout(scaled(GUARD)) {
            Ok(status) => status.unwrap(),
            Err(_) => {
                kill_all(pid, &self.runtimes);
                panic!("rutis-host did not exit; its output: {:#?}", self.seen);
            }
        };
        let stderr = self.stderr.recv_timeout(scaled(GUARD));
        loop {
            match self.lines.recv_timeout(scaled(GUARD)) {
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

    pub fn fail(&mut self, what: &str) -> ! {
        kill_all(self.pid(), &self.runtimes);
        let _ = self.child.wait();
        let stderr = self.stderr.recv_timeout(scaled(GUARD)).unwrap_or_default();
        panic!("{what}; output: {:#?}; stderr: {stderr}", self.seen);
    }
}

/// Kill the host's process group and the runtime processes, so a failing
/// test leaves nothing behind.
pub fn kill_all(host: i32, runtimes: &[i32]) {
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
pub fn signal(host: &Host, signal: i32, group: bool) {
    let target = if group { -host.pid() } else { host.pid() };
    // SAFETY: plain kill(2).
    assert_eq!(unsafe { libc::kill(target, signal) }, 0, "kill");
}

/// Whether the process `pid` still exists (a zombie counts: nothing reaped
/// it).
pub fn alive(pid: i32) -> bool {
    // SAFETY: signal 0 only checks that the process exists.
    unsafe { libc::kill(pid, 0) == 0 }
}

pub fn cleanups(dir: &Path, row: &str) -> usize {
    std::fs::read_to_string(dir.join(format!("{row}.cleanup")))
        .map(|text| text.lines().count())
        .unwrap_or(0)
}
