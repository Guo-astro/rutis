//! The fixture Rust host of E2E scenario S4 (#188; plan
//! `docs/design-quality-step2-2026-10-11.md` §3.5): a Rust program that
//! embeds rutis as `docs/guide/rust-host.md` says, so a service can move
//! between a Python row, another node and a Rust row while the host runs.
//!
//! ```text
//! rutis-e2e-fixture-host [rutis.json]
//! ```
//!
//! It starts what `rutis.json` names with rutis-host's library, plus one
//! row compiled in: `fixture/calendar`, the service `calendar` in Rust
//! (`today()` is `"rust"`). It prints each row's state as `rutis-host run`
//! does, then reads commands from stdin, one per line:
//!
//! - `reload`: read `rutis.json` again and run it instead, all or nothing,
//!   as `rutis-host run` does on SIGUSR1 (`Host::reload`): a runtime
//!   (`node`, `py`, `bun`) the file no longer has is stopped after its
//!   rows, and its process has ended before the answer. Prints `applied`,
//!   or `rejected: <reason>` for what needs a restart (another `id`,
//!   `listen`, an added or changed runtime) or would fail.
//! - `stop`, or the end of stdin: shut down (every cleanup runs), then exit
//!   0; 2 when the shutdown did not finish.
//!
//! Links present `RUTIS_TOKEN_<PEER>` (else `RUTIS_TOKEN`), read when they
//! dial, so a link added by a reload has its credential too.

use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use rutis::{BoxFuture, CordisError, Ctx, Effect, Plugin};
use rutis_bridge::session::{
    host_key, Error as CallError, HostDispatch, Reply, Value as CallValue,
};
use rutis_host::config::HostConfig;
use rutis_host::host::{Embedding, Host, PROCESS_EXIT};
use serde_json::json;

/// How long cleanups may take when the host stops.
const SHUTDOWN: Duration = Duration::from_secs(10);

fn main() {
    let path = PathBuf::from(
        std::env::args()
            .nth(1)
            .unwrap_or_else(|| "rutis.json".into()),
    );
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("the async runtime");
    let code = runtime.block_on(run(&path));
    drop(runtime);
    rutis_host::host::output_written();
    std::process::exit(code);
}

async fn run(path: &Path) -> i32 {
    let config = match HostConfig::read(path) {
        Ok(config) => config,
        Err(error) => {
            eprintln!("fixture-host: {error}");
            return 1;
        }
    };
    let embedding = Embedding {
        builtins: Some(Box::new(|builtins| {
            builtins.register_fn::<serde_json::Value, _>("fixture/calendar", |_| {
                Ok(Box::new(RustCalendar))
            });
        })),
        identity: None,
    };
    let mut host = match Host::start_with(&config, embedding).await {
        Ok(host) => host,
        Err(error) => {
            eprintln!("fixture-host: {error}");
            return 1;
        }
    };
    let started = async {
        host.runtimes_ready().await?;
        host.load(config.rows()).await
    };
    if let Err(error) = started.await {
        eprintln!("fixture-host: {error}");
        stop(&host).await;
        return 1;
    }
    rutis_host::status::follow(host.followed());

    // Commands from stdin; the channel closes with it.
    let (commands, mut received) = tokio::sync::mpsc::unbounded_channel();
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines() {
            let Ok(line) = line else { break };
            if commands.send(line).is_err() {
                break;
            }
        }
    });
    while let Some(command) = received.recv().await {
        match command.trim() {
            "reload" => match reload(&mut host, path).await {
                Ok(()) => println!("applied"),
                Err(reason) => println!("rejected: {reason}"),
            },
            "stop" => break,
            "" => {}
            other => eprintln!("fixture-host: unknown command {other:?}"),
        }
    }
    stop(&host).await
}

/// Shut down; the exit code.
async fn stop(host: &Host) -> i32 {
    if let Err(error) = host.shut_down(SHUTDOWN).await {
        eprintln!("fixture-host: {error}");
        host.kill_processes().await;
        return 2;
    }
    if let Err(error) = host.processes_exit(PROCESS_EXIT).await {
        eprintln!("fixture-host: {error}");
        host.kill_processes().await;
        return 2;
    }
    0
}

/// Read `path` again and run it instead.
async fn reload(host: &mut Host, path: &Path) -> Result<(), String> {
    let reloaded = host.reload(HostConfig::read(path)?).await?;
    match reloaded.warnings.is_empty() {
        true => Ok(()),
        false => Err(reloaded.warnings.join("; ")),
    }
}

/// The row `fixture/calendar`: the service `calendar`, in Rust.
struct RustCalendar;

impl Plugin for RustCalendar {
    fn name(&self) -> &str {
        "fixture/calendar"
    }

    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, CordisError>> {
        Box::pin(async move {
            ctx.provide_as::<dyn HostDispatch>(host_key("calendar"), Arc::new(Calendar))?;
            Ok(Effect::Done)
        })
    }
}

struct Calendar;

impl HostDispatch for Calendar {
    fn invoke(&self, method: &str, _args: CallValue) -> Reply {
        match method {
            "today" => Ok(json!("rust").into()),
            other => Err(CallError::Value(format!("calendar has no method {other}"))),
        }
    }

    fn methods(&self) -> Option<serde_json::Value> {
        Some(json!({ "today": "async" }))
    }
}
