//! `rutis-host dev --debug` (#679): each language's debugger reaches the
//! plugin's code.
//!
//! - Python: `breakpoint()` in `apply` stops in pdb, which reads the host's
//!   input (a pipe here, a terminal for a person) and answers on its
//!   output.
//! - Node and Bun: the runtime's own inspector (127.0.0.1:9229, and
//!   127.0.0.1:6499/rutis) takes a client, which stops the runtime at a
//!   `debugger` statement and lets it go on.
//! - Go: the runtime runs under Delve (127.0.0.1:2345), which a client
//!   halts and continues. Only where `dlv` is installed: CI installs it on
//!   Linux and sets `RUTIS_TEST_DELVE=required`, which turns a missing
//!   `dlv` into a failure instead of a skip.
//!
//! The ports are the debuggers' fixed defaults, so the tests take a lock
//! that every rutis-host test process on the machine shares.
//!
//! Unix only, as the other tests of `dev`: the host is stopped with
//! kill(2).
#![cfg(unix)]

mod common;

use std::io::Write;
use std::net::TcpStream;
use std::path::Path;

use common::process::{file_url, signal, Host, GUARD};
use common::repo;
use rutis_test_support::scaled;
use serde_json::{json, Value};

/// Held while a test uses the debuggers' ports: `flock(2)` on a file every
/// test process on this machine opens.
struct PortsLock(#[allow(dead_code)] std::fs::File);

fn lock_ports() -> PortsLock {
    use std::os::fd::AsRawFd;
    let file = std::fs::File::options()
        .create(true)
        .append(true)
        .open("/tmp/rutis-host-debug-ports.lock")
        .unwrap();
    // SAFETY: plain flock(2) on an open file.
    assert_eq!(unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) }, 0);
    PortsLock(file)
}

/// A package.json project whose plugin is `source`, with the runtime
/// package of this checkout (`package`, at `from`) where `dev` finds it.
fn script_project(dir: &Path, file: &str, package: &str, from: &str) {
    let modules = dir.join("node_modules/@arcships");
    std::fs::create_dir_all(&modules).unwrap();
    std::os::unix::fs::symlink(
        repo().join(from).canonicalize().unwrap(),
        modules.join(package),
    )
    .unwrap();
    std::fs::write(
        dir.join("package.json"),
        r#"{ "name": "weather", "type": "module" }"#,
    )
    .unwrap();
    let sdk = file_url(
        &repo()
            .join("node/rutis/src/index.mjs")
            .canonicalize()
            .unwrap(),
    );
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(
        dir.join("src").join(file),
        format!(
            "import {{ definePlugin }} from '{sdk}'\n\
             let beats = 0\n\
             export default definePlugin({{ apply() {{\n\
               const timer = setInterval(() => {{\n\
                 beats += 1\n\
               }}, 20)\n\
               return () => clearInterval(timer)\n\
             }} }})\n"
        ),
    )
    .unwrap();
}

/// A client of a runtime's inspector (Chrome DevTools Protocol in Node,
/// WebKit's in Bun: the same messages here).
struct Inspector {
    url: String,
    socket: tungstenite::WebSocket<tungstenite::stream::MaybeTlsStream<TcpStream>>,
    /// The scripts' URLs by id, as the inspector announces them.
    scripts: std::collections::HashMap<String, String>,
}

impl Inspector {
    fn connect(url: &str) -> Result<Self, String> {
        let (mut socket, _) = tungstenite::connect(url).map_err(|error| error.to_string())?;
        if let tungstenite::stream::MaybeTlsStream::Plain(stream) = socket.get_mut() {
            stream
                .set_read_timeout(Some(scaled(GUARD)))
                .map_err(|error| error.to_string())?;
        }
        Ok(Self {
            url: url.to_owned(),
            socket,
            scripts: Default::default(),
        })
    }

    fn fail(&self, error: &dyn std::fmt::Display) -> String {
        format!("the inspector at {}: {error}", self.url)
    }

    fn send(&mut self, id: u64, method: &str, params: Value) -> Result<(), String> {
        let message = json!({ "id": id, "method": method, "params": params });
        self.socket
            .send(tungstenite::Message::text(message.to_string()))
            .map_err(|error| self.fail(&error))
    }

    /// Each message until one satisfies `want`.
    fn read_until(&mut self, want: impl Fn(&Value) -> bool) -> Result<Value, String> {
        loop {
            let message = self.socket.read().map_err(|error| self.fail(&error))?;
            let Some(value) = message
                .to_text()
                .ok()
                .and_then(|text| serde_json::from_str::<Value>(text).ok())
            else {
                continue;
            };
            if value["method"] == "Debugger.scriptParsed" {
                let params = &value["params"];
                if let (Some(id), Some(url)) = (params["scriptId"].as_str(), params["url"].as_str())
                {
                    self.scripts.insert(id.to_owned(), url.to_owned());
                }
            }
            if want(&value) {
                return Ok(value);
            }
        }
    }

    /// Call `method`: its reply.
    fn call(&mut self, id: u64, method: &str, params: Value) -> Result<Value, String> {
        self.send(id, method, params)?;
        self.read_until(|message| message["id"] == id)
    }
}

/// Connect to the inspector at `url`, set a breakpoint on line 5 of the
/// plugin's `file` (in its timer), wait for the runtime to stop there, and
/// let it go on: the script and line it stopped at.
fn stop_in_inspector(url: &str, file: &str) -> Result<(String, u64), String> {
    let mut inspector = Inspector::connect(url)?;
    inspector.call(1, "Debugger.enable", json!({}))?;
    // WebKit's inspector (Bun) starts with breakpoints off.
    inspector.call(
        2,
        "Debugger.setBreakpointsActive",
        json!({ "active": true }),
    )?;
    // A reloaded module's URL ends in `?rutis-reload=<n>`.
    let pattern = format!("/src/{}(\\?.*)?$", file.replace('.', "\\."));
    let set = inspector.call(
        3,
        "Debugger.setBreakpointByUrl",
        json!({ "urlRegex": pattern, "lineNumber": 4 }),
    )?;
    if !set["error"].is_null() {
        return Err(inspector.fail(&set));
    }
    let paused = inspector.read_until(|message| message["method"] == "Debugger.paused")?;
    let frame = &paused["params"]["callFrames"][0];
    let script = frame["url"]
        .as_str()
        .filter(|url| !url.is_empty())
        .map(str::to_owned)
        .or_else(|| {
            let script = frame["location"]["scriptId"].as_str()?;
            inspector.scripts.get(script).cloned()
        })
        .unwrap_or_default();
    let line = frame["location"]["lineNumber"].as_u64().unwrap_or(u64::MAX);
    inspector.call(
        4,
        "Debugger.removeBreakpoint",
        json!({ "breakpointId": set["result"]["breakpointId"] }),
    )?;
    inspector.send(5, "Debugger.resume", json!({}))?;
    inspector.read_until(|message| message["method"] == "Debugger.resumed")?;
    Ok((script, line))
}

/// End the host with SIGTERM: it exits 0.
fn stop(host: Host) {
    signal(&host, libc::SIGTERM, false);
    let (status, stderr, output) = host.exit();
    assert_eq!(
        status.code(),
        Some(0),
        "output: {output:#?}; stderr: {stderr}"
    );
}

/// guarantee: host/dev-debug
#[test]
fn dev_debug_stops_python_at_breakpoint() {
    let _ports = lock_ports();
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("pyproject.toml"),
        "[project]\nname = \"marker\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(dir.path().join("marker")).unwrap();
    std::fs::write(
        dir.path().join("marker/__init__.py"),
        "from rutis import define_plugin\n\
         \n\
         def apply(ctx, config):\n\
         \x20   answer = 6 * 7\n\
         \x20   breakpoint()\n\
         \x20   print(\"plugin went on\", flush=True)\n\
         \n\
         plugin = define_plugin(apply)\n",
    )
    .unwrap();
    let (mut host, mut input) = Host::start_talking(&["dev", "--debug", "."], dir.path(), &[]);
    host.wait_lines(&[
        "rutis-host dev: debugging: the Python runtime reads this terminal: breakpoint() stops there in pdb",
    ]);
    // pdb's prompt has no newline: its answer ends the line.
    // The line it stopped at: the next one before Python 3.13,
    // `breakpoint()` itself since.
    host.wait_line(|line| {
        (line == "-> breakpoint()" || line.starts_with("-> print(")).then_some(())
    });
    input.write_all(b"p answer\n").unwrap();
    host.wait_line(|line| (line == "(Pdb) 42").then_some(()));
    input.write_all(b"c\n").unwrap();
    host.wait_lines(&["(Pdb) plugin went on", "marker: running"]);
    stop(host);
}

/// guarantee: host/dev-debug
#[test]
fn dev_debug_opens_the_node_inspector() {
    let _ports = lock_ports();
    let dir = tempfile::tempdir().unwrap();
    script_project(
        dir.path(),
        "index.mjs",
        "rutis-runtime",
        "node/rutis-runtime",
    );
    let (mut host, _input) = Host::start_talking(&["dev", "--debug", "."], dir.path(), &[]);
    let url = host.wait_line(|line| {
        line.strip_prefix("Debugger listening on ")
            .map(str::to_owned)
    });
    assert!(url.starts_with("ws://127.0.0.1:9229/"), "{url}");
    host.wait_lines(&["weather: running"]);
    match stop_in_inspector(&url, "index.mjs") {
        Ok((script, 4)) if script.contains("/src/index.mjs") => {}
        other => host.fail(&format!("{other:?}")),
    }
    stop(host);
}

/// guarantee: host/dev-debug
#[test]
fn dev_debug_opens_the_bun_inspector() {
    let _ports = lock_ports();
    let dir = tempfile::tempdir().unwrap();
    script_project(dir.path(), "index.ts", "rutis-bun", "bun/rutis-bun");
    // Bun manages it: the project runs in Bun.
    std::fs::write(dir.path().join("bun.lock"), "").unwrap();
    let (mut host, _input) = Host::start_talking(&["dev", "--debug", "."], dir.path(), &[]);
    host.wait_lines(&["  ws://127.0.0.1:6499/rutis", "weather: running"]);
    match stop_in_inspector("ws://127.0.0.1:6499/rutis", "index.ts") {
        Ok((script, 4)) if script.contains("/src/index.ts") => {}
        other => host.fail(&format!("{other:?}")),
    }
    stop(host);
}

/// Halt the runtime through Delve's JSON-RPC API at `address`, and let it
/// go on: the state it was halted in.
fn halt_in_delve(address: &str) -> Result<Value, String> {
    let fail = |error: &dyn std::fmt::Display| format!("Delve at {address}: {error}");
    let mut socket = TcpStream::connect(address).map_err(|error| fail(&error))?;
    socket
        .set_read_timeout(Some(scaled(GUARD)))
        .map_err(|error| fail(&error))?;
    let reader = socket.try_clone().map_err(|error| fail(&error))?;
    let mut replies = serde_json::Deserializer::from_reader(reader).into_iter::<Value>();
    let mut call = |id: u64, command: &str| {
        let call =
            json!({ "id": id, "method": "RPCServer.Command", "params": [{ "name": command }] });
        socket
            .write_all(call.to_string().as_bytes())
            .map_err(|error| fail(&error))
    };
    call(1, "halt")?;
    let reply = loop {
        let reply = replies
            .next()
            .ok_or_else(|| fail(&"the connection ended"))?
            .map_err(|error| fail(&error))?;
        if reply["id"] == 1 {
            break reply;
        }
    };
    if !reply["error"].is_null() {
        return Err(fail(&reply));
    }
    // It goes on; the reply would come at its next stop.
    call(2, "continue")?;
    Ok(reply["result"]["State"].clone())
}

/// guarantee: host/dev-debug
#[test]
fn dev_debug_runs_the_binary_under_delve() {
    let installed = std::process::Command::new("dlv")
        .arg("version")
        .output()
        .is_ok_and(|output| output.status.success());
    if !installed {
        assert_ne!(
            std::env::var("RUTIS_TEST_DELVE").as_deref(),
            Ok("required"),
            "dlv is not installed"
        );
        eprintln!("skipped: dlv is not installed");
        return;
    }
    let _ports = lock_ports();
    let dir = tempfile::tempdir().unwrap();
    rutis_host::new::create(dir.path(), "net-probe", "go").unwrap();
    let project = dir.path().join("net-probe");
    // The SDK of this checkout, not a published one.
    let sdk = repo().join("go/rutis").canonicalize().unwrap();
    let module = std::fs::read_to_string(project.join("go.mod")).unwrap();
    std::fs::write(
        project.join("go.mod"),
        format!(
            "{module}\nreplace github.com/arcships/rutis/go/rutis => {}\n",
            sdk.display()
        ),
    )
    .unwrap();
    // Builds use the module cache, never the network.
    let env = [
        ("GOPROXY", "off"),
        ("GOTOOLCHAIN", "local"),
        ("GOFLAGS", "-mod=mod"),
    ];
    let (mut host, _input) = Host::start_talking(&["dev", "--debug", "."], &project, &env);
    // Delve's line, with the runtime's name as every line of a Go
    // runtime's under `dev` (#678).
    let (mut delve, mut running) = (false, false);
    host.wait_line(|line| {
        delve |=
            line.starts_with("[go-") && line.ends_with("] API server listening at: 127.0.0.1:2345");
        running |= line == "net-probe: running";
        (delve && running).then_some(())
    });
    match halt_in_delve("127.0.0.1:2345") {
        Ok(state) if state["Running"] == json!(false) => {}
        other => host.fail(&format!("{other:?}")),
    }
    stop(host);
}
