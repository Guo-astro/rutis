//! What `rutis-host run` writes while its runtimes write too (#332): the
//! released binary, with a Python and a Node row that write each line in
//! two pieces, the newline only once the host has written its own lines.
//!
//! guarantee: host/output-lines-whole — every line a plugin writes comes
//! out whole, on the stream it wrote it to, and no line of the host's (or
//! of another runtime's) lands inside it; a last line without a newline
//! comes out as a line of its own once the runtime ends, before the host
//! exits.
//!
//! Before #332 the runtimes wrote straight into the host's output, so the
//! row's status line landed after the first piece: `plugin py-row line
//! py-row: running`. Deterministic: the first piece is written while the
//! row starts, before the host can say that it runs, and the newline only
//! once the test has read that line.
//!
//! Unix only: the host is ended with SIGTERM, as in `signals.rs`; CI does
//! not run the rutis-host tests on Windows.
#![cfg(unix)]

use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use rutis_test_support::HANG_GUARD;
use serde_json::json;

/// Only against a hang: the host starts a Python and a Node runtime cold.
const GUARD: Duration = Duration::from_secs(6 * HANG_GUARD.as_secs());

const ROWS: [&str; 2] = ["py-row", "node-row"];

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// A Python plugin: `plugin <row> line` written as `print` writes it, the
/// newline separately, and only once the gate file exists; the same on
/// stderr; and, as it unloads, `plugin <row> last` with no newline at all.
const PYTHON_PLUGIN: &str = r#"
import os, sys, threading, time

def apply(ctx, config):
    row, gate = config["row"], config["gate"]
    print(f"plugin {row} line", end="", flush=True)
    print(f"plugin {row} err", end="", file=sys.stderr, flush=True)
    def rest():
        # Waits for the gate, which the test makes once it read the row's
        # status line.
        while not os.path.exists(gate):
            time.sleep(0.01)
        print(flush=True)
        print(file=sys.stderr, flush=True)
    threading.Thread(target=rest, daemon=True).start()
    ctx.effect(lambda: print(f"plugin {row} last", end="", flush=True))
"#;

/// The same in JavaScript.
fn node_plugin() -> String {
    let sdk = repo()
        .join("node/rutis/src/index.mjs")
        .canonicalize()
        .unwrap();
    format!(
        "import {{ definePlugin }} from '{}'\n\
         import {{ existsSync }} from 'node:fs'\n\
         export default definePlugin({{ apply(ctx, config) {{\n\
           process.stdout.write(`plugin ${{config.row}} line`)\n\
           process.stderr.write(`plugin ${{config.row}} err`)\n\
           const timer = setInterval(() => {{\n\
             if (!existsSync(config.gate)) return\n\
             clearInterval(timer)\n\
             process.stdout.write('\\n')\n\
             process.stderr.write('\\n')\n\
           }}, 10)\n\
           return () => {{\n\
             clearInterval(timer)\n\
             process.stdout.write(`plugin ${{config.row}} last`)\n\
           }}\n\
         }} }})\n",
        url::Url::from_file_path(&sdk).unwrap()
    )
}

fn project(dir: &Path) -> PathBuf {
    std::fs::write(dir.join("pieces.py"), PYTHON_PLUGIN).unwrap();
    std::fs::write(dir.join("pieces.mjs"), node_plugin()).unwrap();
    std::fs::write(
        dir.join("package.json"),
        r#"{ "name": "output", "type": "module" }"#,
    )
    .unwrap();
    let gate = dir.join("gate");
    let mut py = json!({ "project": "." });
    if let Some(python) = std::env::var_os("RUTIS_PYTHON") {
        py["python"] = json!(python.to_str().unwrap());
    }
    let config = json!({
        "id": "output",
        "runtimes": {
            "py": py,
            "node": { "project": ".", "runtime": repo().join("node/rutis-runtime") },
        },
        "rows": [
            { "id": "py-row", "name": "py:pieces", "config": { "row": "py-row", "gate": gate } },
            { "id": "node-row", "name": "./pieces.mjs", "config": { "row": "node-row", "gate": gate } },
        ]
    });
    let file = dir.join("rutis.json");
    std::fs::write(&file, serde_json::to_string_pretty(&config).unwrap()).unwrap();
    file
}

/// The host, its stdout read line by line, its stderr whole.
struct Host {
    child: Child,
    lines: Receiver<String>,
    stderr: Receiver<String>,
    seen: Vec<String>,
}

impl Host {
    fn start(config: &Path, dir: &Path) -> Self {
        use std::os::unix::process::CommandExt;
        let mut child = Command::new(env!("CARGO_BIN_EXE_rutis-host"))
            .arg("run")
            .arg(config)
            .current_dir(dir)
            .env("RUTIS_PYTHON_PATH", repo().join("python/rutis"))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0)
            .spawn()
            .expect("the rutis-host binary");
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
        }
    }

    /// Read lines until one satisfies `want`; that line.
    fn wait_line(&mut self, want: impl Fn(&str) -> bool) -> String {
        loop {
            match self.lines.recv_timeout(GUARD) {
                Ok(line) => {
                    self.seen.push(line.clone());
                    if want(&line) {
                        return line;
                    }
                }
                Err(error) => self.fail(&format!("waiting for a line: {error}")),
            }
        }
    }

    /// Wait for the host to exit, and its output to end; its status, its
    /// stderr and every line it wrote on stdout.
    fn exit(mut self) -> (std::process::ExitStatus, String, Vec<String>) {
        let pid = self.child.id() as i32;
        let (send, exited) = mpsc::channel();
        let mut child = self.child;
        std::thread::spawn(move || {
            let _ = send.send(child.wait());
        });
        let Ok(status) = exited.recv_timeout(GUARD) else {
            // SAFETY: plain kill(2) of the host's group.
            unsafe { libc::kill(-pid, libc::SIGKILL) };
            panic!("rutis-host did not exit; its output: {:#?}", self.seen);
        };
        loop {
            match self.lines.recv_timeout(GUARD) {
                Ok(line) => self.seen.push(line),
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Err(error) => panic!("the output did not end: {error}; {:#?}", self.seen),
            }
        }
        let stderr = self.stderr.recv_timeout(GUARD).unwrap_or_default();
        (status.unwrap(), stderr, self.seen)
    }

    fn fail(&mut self, what: &str) -> ! {
        // SAFETY: plain kill(2) of the host's group; the runtimes, in
        // groups of their own, end with their channels.
        unsafe { libc::kill(-(self.child.id() as i32), libc::SIGKILL) };
        let _ = self.child.wait();
        let stderr = self.stderr.recv_timeout(GUARD).unwrap_or_default();
        panic!("{what}; output: {:#?}; stderr: {stderr}", self.seen);
    }
}

/// guarantee: host/output-lines-whole (#332)
#[test]
fn plugin_lines_written_in_pieces_come_out_whole() {
    let dir = tempfile::tempdir().unwrap();
    let config = project(dir.path());
    let mut host = Host::start(&config, dir.path());

    // The first piece of each plugin's line is written while its row
    // starts; the host says the row runs after that.
    for _ in ROWS {
        let line = host.wait_line(|line| line.ends_with(": running"));
        assert!(
            ROWS.iter().any(|row| line == format!("{row}: running")),
            "a plugin's piece ran into the host's line {line:?}; output: {:#?}",
            host.seen
        );
    }
    // Now the newlines.
    std::fs::write(dir.path().join("gate"), "").unwrap();
    for _ in ROWS {
        let line = host.wait_line(|line| line.contains(" line"));
        assert!(
            ROWS.iter().any(|row| line == format!("plugin {row} line")),
            "a broken line {line:?}; output: {:#?}",
            host.seen
        );
    }

    // SAFETY: plain kill(2).
    assert_eq!(
        unsafe { libc::kill(host.child.id() as i32, libc::SIGTERM) },
        0
    );
    let (status, stderr, seen) = host.exit();
    assert!(status.success(), "{status}; stderr: {stderr}");

    // Every line of a plugin's is whole; every other line is the host's
    // and has none of a plugin's in it.
    let mut wanted: Vec<String> = ROWS
        .iter()
        .flat_map(|row| [format!("plugin {row} line"), format!("plugin {row} last")])
        .collect();
    for line in &seen {
        match wanted.iter().position(|want| want == line) {
            Some(found) => drop(wanted.remove(found)),
            None => assert!(
                !line.contains("plugin "),
                "a broken line {line:?}; output: {:#?}",
                seen
            ),
        }
    }
    assert!(wanted.is_empty(), "missing {wanted:?}; output: {:#?}", seen);
    for row in ROWS {
        let whole = format!("plugin {row} err");
        assert!(
            stderr.lines().any(|line| line == whole),
            "no whole {whole:?} on stderr: {stderr}"
        );
        assert!(
            stderr
                .lines()
                .all(|line| !line.contains(&whole) || line == whole),
            "a broken stderr line: {stderr}"
        );
    }
}
