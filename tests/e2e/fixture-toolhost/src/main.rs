//! A small host with an extension point (#690, the host owner's check of
//! #663): a Rust application embedding rutis, as `docs/guide/rust-host.md`
//! says, whose plugins register "tools". It offers the service `tools`
//! (`tools.register(name, description, run)`, `run` a function the plugin
//! passes), lists the tools registered and calls them. When a tool fails,
//! the host reads the plugin's error with `rutis_bridge::thrown`: its name,
//! code and message, which it can say in its own words.
//!
//! It runs the plugins of `toolhost.json` in the working directory (`{
//! "plugins": [{ "id": "weather", "name": "/abs/path/src/index.ts" }] }`),
//! in the Node runtime at `RUTIS_NODE_RUNTIME` (a `@arcships/rutis-runtime`
//! package). Commands on stdin, one per line, each answered with one JSON
//! line on stdout:
//!
//! - `list`: `{"tools":[{"name":…,"description":…}]}`
//! - `call <tool> <json>`: `{"ok":<result>}`, or `{"error":{"name":…,
//!   "code":…,"message":…}}` (`code` null when the error has none), or
//!   `{"error":{"unknown":<tool>}}`
//! - `stop`, or the end of stdin: shut down, exit 0
//!
//! It prints `{"ready":true}` once its plugins run, `{"failed":…}` and
//! exits 1 when they cannot start.

use std::collections::BTreeMap;
use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rutis::host::{BoxFuture, CtxHost};
use rutis::Ctx;
use rutis_bridge::runtime::{LocalRuntime, Stdio};
use rutis_loader::report::EntryStatus;
use rutis_loader::runtime::{RuntimeResolver, RuntimeRowsPlugin};
use rutis_loader::{Chain, Layer, LoaderOptions, LoaderPlugin, Patch, ServiceCatalog};
use serde_json::{json, Value};

/// A tool's function: the plugin's, called by reference.
type Run = Arc<dyn Fn(Value) -> BoxFuture<'static, Result<Value, rutis::Error>> + Send + Sync>;

/// The extension point: the tools plugins registered.
#[derive(Default)]
pub struct Tools {
    tools: Mutex<BTreeMap<String, (String, Run)>>,
}

#[rutis::service]
impl Tools {
    /// Register the tool `name`: `run(input)` runs it.
    pub fn register(&self, name: String, description: String, run: Run) {
        self.tools.lock().unwrap().insert(name, (description, run));
    }
}

impl Tools {
    fn list(&self) -> Value {
        let tools = self.tools.lock().unwrap();
        let tools: Vec<Value> = tools
            .iter()
            .map(|(name, (description, _))| json!({ "name": name, "description": description }))
            .collect();
        json!({ "tools": tools })
    }

    fn get(&self, name: &str) -> Option<Run> {
        self.tools
            .lock()
            .unwrap()
            .get(name)
            .map(|(_, run)| run.clone())
    }
}

/// Write one JSON line on stdout.
fn answer(value: Value) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{value}");
    let _ = out.flush();
}

fn main() {
    let runtime = tokio::runtime::Runtime::new().expect("a tokio runtime");
    let code = runtime.block_on(run());
    std::process::exit(code);
}

async fn run() -> i32 {
    let (root, tools) = match start().await {
        Ok(started) => started,
        Err(error) => {
            answer(json!({ "failed": error }));
            return 1;
        }
    };
    answer(json!({ "ready": true }));
    let (send, mut lines) = tokio::sync::mpsc::unbounded_channel::<String>();
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines() {
            let Ok(line) = line else { break };
            if send.send(line).is_err() {
                break;
            }
        }
    });
    while let Some(line) = lines.recv().await {
        let line = line.trim();
        let (command, rest) = line.split_once(' ').unwrap_or((line, ""));
        match command {
            "list" => answer(tools.list()),
            "call" => {
                let (name, input) = rest.split_once(' ').unwrap_or((rest, "null"));
                let Some(run) = tools.get(name) else {
                    answer(json!({ "error": { "unknown": name } }));
                    continue;
                };
                let input: Value = serde_json::from_str(input).unwrap_or(Value::Null);
                match run(input).await {
                    Ok(result) => answer(json!({ "ok": result })),
                    Err(error) => answer(json!({ "error": described(&error) })),
                }
            }
            "stop" => break,
            "" => {}
            other => answer(json!({ "error": { "command": other } })),
        }
    }
    match root.shutdown().await {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("toolhost: shutdown: {error}");
            2
        }
    }
}

/// What the host knows of a tool's failure: the plugin's error, structured.
fn described(error: &rutis::Error) -> Value {
    match rutis_bridge::thrown(error) {
        Some(thrown) => json!({
            "name": match thrown {
                rutis_bridge::session::Error::Remote { name, .. } => name.clone(),
                _ => String::new(),
            },
            "code": thrown.code(),
            "message": match thrown {
                rutis_bridge::session::Error::Remote { message, .. } => message.clone(),
                other => other.to_string(),
            },
        }),
        None => json!({ "name": null, "code": null, "message": error.to_string() }),
    }
}

async fn start() -> Result<(Ctx, Arc<Tools>), String> {
    let dir = std::env::current_dir().map_err(|e| e.to_string())?;
    let config: Value = serde_json::from_str(
        &std::fs::read_to_string(dir.join("toolhost.json"))
            .map_err(|e| format!("toolhost.json: {e}"))?,
    )
    .map_err(|e| format!("toolhost.json: {e}"))?;
    let node_package = std::env::var_os("RUTIS_NODE_RUNTIME")
        .map(PathBuf::from)
        .ok_or("RUTIS_NODE_RUNTIME is not set")?;

    let root = Ctx::root().map_err(|e| e.to_string())?;
    // The extension point, served to the plugins of every language by name.
    root.provide(Tools::default()).map_err(|e| e.to_string())?;
    let tools = root.get::<Tools>().ok_or("the tools service")?;

    let mut catalog = ServiceCatalog::new();
    catalog.share_by_name();
    // Only this program writes to stdout: its answers are whole lines.
    let node = LocalRuntime::node(node_package, dir.join("package.json")).stdout(Stdio::Null);
    let rows = Arc::new(RuntimeResolver::node(node.handle()).with_catalog(&catalog));
    root.plugin(node);
    let plugin = LoaderPlugin::new(
        Chain::new().with_shared(rows.clone()),
        LoaderOptions {
            catalog,
            ..LoaderOptions::default()
        },
    );
    let loader = plugin.handle();
    root.plugin(plugin).await.map_err(|e| e.to_string())?;
    root.plugin(RuntimeRowsPlugin::new(rows));

    let patches: Vec<Patch> =
        serde_json::from_value(json!([{ "insert": config["plugins"].clone() }]))
            .map_err(|e| e.to_string())?;
    let report = loader
        .reconcile(vec![Layer::new("toolhost", patches)], None)
        .await
        .map_err(|e| e.to_string())?;
    if !report.failures.is_empty() {
        let failures: Vec<String> = report.failures.iter().map(|f| f.to_string()).collect();
        return Err(failures.join("; "));
    }
    // Each plugin running: a row of the runtime starts once it is up.
    let ids: Vec<String> = config["plugins"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|row| row["id"].as_str().map(str::to_owned))
        .collect();
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    loop {
        let states: Vec<String> = ids
            .iter()
            .map(|id| match loader.get(id).map(|entry| entry.status) {
                Some(EntryStatus::Running(snapshot)) => format!("{:?}", snapshot.state),
                other => format!("{other:?}"),
            })
            .collect();
        if states.iter().all(|state| state == "Active") {
            break;
        }
        if std::time::Instant::now() > deadline {
            return Err(format!("the plugins did not start: {ids:?} {states:?}"));
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    Ok((root, tools))
}
