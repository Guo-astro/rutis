//! What `rutis-host run` prints of its links (#321): the listener's bound
//! address once at start, and one line per change of a peer row's link
//! state, with its category (docs/guide/rutis-host.md, "What `run`
//! prints"). Two hosts, the released binary, over loopback `ws://`: `main`
//! dials `office`.
//!
//! guarantee: host/link-state-output

use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver};

use rutis_test_support::{scaled, HANG_GUARD};
use serde_json::{json, Value};

mod common;

/// A running `rutis-host run`, its output read line by line.
struct Host {
    child: Child,
    lines: Receiver<String>,
    stderr: Receiver<String>,
    seen: Vec<String>,
}

impl Host {
    /// Run `config` (written to `<dir>/<name>.json`) with `tokens`
    /// (`RUTIS_TOKEN_<PEER>`, value) and none of the host's other variables.
    fn run(dir: &Path, name: &str, config: Value, tokens: &[(&str, &str)]) -> Self {
        let file = dir.join(format!("{name}.json"));
        std::fs::write(&file, serde_json::to_string_pretty(&config).unwrap()).unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_rutis-host"));
        command
            .arg("run")
            .arg(&file)
            .current_dir(dir)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (variable, _) in std::env::vars_os() {
            let variable = variable.to_string_lossy();
            if common::VARIABLES.contains(&variable.as_ref())
                || variable.starts_with("RUTIS_TOKEN_")
            {
                command.env_remove(variable.as_ref());
            }
        }
        for (peer, token) in tokens {
            command.env(format!("RUTIS_TOKEN_{}", peer.to_uppercase()), token);
        }
        let mut child = command.spawn().expect("the rutis-host binary");
        let (send, lines) = mpsc::channel();
        // The host's own lines are on stderr (#678): read with stdout's.
        let lines_too = send.clone();
        let stdout = child.stdout.take().unwrap();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if send.send(line).is_err() {
                    break;
                }
            }
        });
        let (send, stderr) = mpsc::channel();
        let error = child.stderr.take().unwrap();
        std::thread::spawn(move || {
            let mut text = String::new();
            for line in BufReader::new(error).lines().map_while(Result::ok) {
                text.push_str(&line);
                text.push('\n');
                let _ = lines_too.send(line);
            }
            let _ = send.send(text);
        });
        Self {
            child,
            lines,
            stderr,
            seen: Vec::new(),
        }
    }

    /// Read lines until one satisfies `want`; what it returns.
    fn wait<T>(&mut self, what: &str, mut want: impl FnMut(&str) -> Option<T>) -> T {
        loop {
            match self.lines.recv_timeout(scaled(HANG_GUARD)) {
                Ok(line) => {
                    let found = want(&line);
                    self.seen.push(line);
                    if let Some(found) = found {
                        return found;
                    }
                }
                Err(error) => {
                    let _ = self.child.kill();
                    let _ = self.child.wait();
                    let stderr = self
                        .stderr
                        .recv_timeout(scaled(HANG_GUARD))
                        .unwrap_or_default();
                    panic!(
                        "waiting for {what}: {error}; output: {:#?}; stderr: {stderr}",
                        self.seen
                    );
                }
            }
        }
    }

    /// The first line starting with `prefix`.
    fn line(&mut self, prefix: &str) -> String {
        self.wait(prefix, |line| {
            line.starts_with(prefix).then(|| line.to_owned())
        })
    }

    /// Kill it; everything it printed, stdout then stderr.
    fn end(mut self) -> String {
        let _ = self.child.kill();
        let _ = self.child.wait();
        while let Ok(line) = self.lines.recv_timeout(scaled(HANG_GUARD)) {
            self.seen.push(line);
        }
        let stderr = self
            .stderr
            .recv_timeout(scaled(HANG_GUARD))
            .unwrap_or_default();
        format!("{}\n{stderr}", self.seen.join("\n"))
    }
}

/// `office`: listens on loopback, port 0, and links to `main`.
fn office(dir: &Path, token: &str) -> (Host, String) {
    let mut office = Host::run(
        dir,
        "office",
        json!({
            "id": "office",
            "listen": [{ "name": "local", "address": "127.0.0.1:0" }],
            "rows": [
                { "id": "main", "name": "rutis-bridge/peer", "config": { "peer": "main", "listen": "local" } }
            ]
        }),
        &[("main", token)],
    );
    let address = office.line("rutis-host: listening on local ");
    let address = address
        .strip_prefix("rutis-host: listening on local ")
        .unwrap()
        .to_owned();
    assert!(
        address.starts_with("127.0.0.1:") && !address.ends_with(":0"),
        "the bound address, with the port picked: {address}"
    );
    (office, address)
}

/// `main`: dials `office` at `address`, with `config` added to the row's.
fn main_host(dir: &Path, address: &str, token: &str, config: Value) -> Host {
    let mut row = json!({ "peer": "office", "dial": format!("ws://{address}/rutis") });
    row.as_object_mut()
        .unwrap()
        .extend(config.as_object().unwrap().clone());
    Host::run(
        dir,
        "main",
        json!({
            "id": "main",
            "rows": [{ "id": "office", "name": "rutis-bridge/peer", "config": row }]
        }),
        &[("office", token)],
    )
}

/// Connected, then the far end goes away: `link connected` on both ends,
/// then `link down (…); retrying in …` on the dialing end. The row keeps
/// its own line (`running`) apart.
#[test]
fn a_link_reports_connected_and_down() {
    let dir = tempfile::tempdir().unwrap();
    let (mut office, address) = office(dir.path(), "same-token");
    let mut main = main_host(dir.path(), &address, "same-token", json!({}));
    main.line("office: running");
    main.line("office: link connected");
    office.line("main: link connected");
    let printed = office.end();
    assert!(!printed.contains("same-token"), "{printed}");
    let down = main.line("office: link down (");
    assert!(down.contains("; retrying in "), "{down}");
    let printed = main.end();
    assert!(!printed.contains("same-token"), "{printed}");
}

/// Nothing listening: `link down`, with the next attempt's wait.
#[test]
fn an_unreachable_far_end_is_reported_as_down() {
    let dir = tempfile::tempdir().unwrap();
    // A port nothing listens on: bound, then closed.
    let address = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .to_string();
    let mut main = main_host(dir.path(), &address, "a-token", json!({}));
    let down = main.line("office: link down (");
    assert!(down.contains("; retrying in "), "{down}");
    main.end();
}

/// Another token: `link refused: auth-rejected (…); retrying in 30s`, and
/// neither token anywhere in either host's output (B7).
#[test]
fn a_refused_token_is_reported_by_its_category_without_it() {
    let dir = tempfile::tempdir().unwrap();
    let (office, address) = office(dir.path(), "office-token-1");
    let mut main = main_host(dir.path(), &address, "main-token-2", json!({}));
    let refused = main.line("office: link refused: ");
    assert!(
        refused.starts_with("office: link refused: auth-rejected (")
            && refused.ends_with("); retrying in 30s"),
        "{refused}"
    );
    let printed = format!("{}\n{}", main.end(), office.end());
    for token in ["office-token-1", "main-token-2"] {
        assert!(!printed.contains(token), "{token} printed: {printed}");
    }
}

/// The far end lacks a required capability: `link stopped: incompatible`.
#[test]
fn an_incompatible_far_end_is_reported_as_stopped() {
    let dir = tempfile::tempdir().unwrap();
    let (office, address) = office(dir.path(), "same-token");
    let mut main = main_host(
        dir.path(),
        &address,
        "same-token",
        json!({ "require": ["runtime"] }),
    );
    let stopped = main.line("office: link stopped: ");
    assert!(
        stopped.starts_with("office: link stopped: incompatible (") && stopped.contains("runtime"),
        "{stopped}"
    );
    main.end();
    office.end();
}
