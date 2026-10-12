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
//! - `reload`: read `rutis.json` again and apply the rows as one loader
//!   edit; a runtime (`node`, `py`, `bun`) the file no longer has is
//!   stopped after that, and its process has ended before the answer.
//!   Prints `applied`, or `rejected: <reason>` for what needs a restart
//!   (another `id`, `listen`, an added or changed runtime) or failed.
//! - `stop`, or the end of stdin: shut down (every cleanup runs), then exit
//!   0; 2 when the shutdown did not finish.
//!
//! Links present `RUTIS_TOKEN_<PEER>` (else `RUTIS_TOKEN`), read when they
//! dial, so a link added by a reload has its credential too. It accepts no
//! links: nothing here listens for one.

use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use rutis::{BoxFuture, CordisError, Ctx, Effect, Plugin};
use rutis_bridge::session::{
    host_key, Error as CallError, HostDispatch, Reply, Value as CallValue,
};
use rutis_bridge::{Credential, Identity, IdentityPlugin, PeerId, Presented};
use rutis_host::config::{token, HostConfig};
use rutis_host::host::{Embedding, Host, PROCESS_EXIT};
use serde_json::json;

/// How long cleanups may take when the host stops.
const SHUTDOWN: Duration = Duration::from_secs(10);

/// How long a stopped runtime's process may take to end: longer than the
/// grace the local transport gives it before killing it.
const RUNTIME_EXIT: Duration = Duration::from_secs(15);

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
    std::process::exit(code);
}

async fn run(path: &Path) -> i32 {
    let mut config = match HostConfig::read(path) {
        Ok(config) => config,
        Err(error) => {
            eprintln!("fixture-host: {error}");
            return 1;
        }
    };
    let identity = match PeerId::new(config.id.clone()) {
        Ok(local) => EnvIdentity { local },
        Err(error) => {
            eprintln!("fixture-host: id: {error}");
            return 1;
        }
    };
    let embedding = Embedding {
        builtins: Some(Box::new(|builtins| {
            builtins.register_fn::<serde_json::Value, _>("fixture/calendar", |_| {
                Ok(Box::new(RustCalendar))
            });
        })),
        identity: Some(IdentityPlugin::new("host", identity)),
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
    rutis_host::status::follow(host.loader.clone(), host.peers.clone());

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
            "reload" => match reload(&mut host, &mut config, path).await {
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

/// Read `path` again and apply the difference to `config`: the rows in one
/// loader edit, then stop the runtimes the file no longer has.
async fn reload(host: &mut Host, config: &mut HostConfig, path: &Path) -> Result<(), String> {
    let new = HostConfig::read(path)?;
    if new.id != config.id {
        return Err(format!(
            "id {:?} is now {:?}: restart the host",
            config.id, new.id
        ));
    }
    if format!("{:?}", new.listen) != format!("{:?}", config.listen) {
        return Err("listen changed: restart the host".into());
    }
    let (old, now) = (&config.runtimes, &new.runtimes);
    if format!("{:?}{:?}", old.go, old.remote) != format!("{:?}{:?}", now.go, now.remote) {
        return Err("runtimes.go or runtimes.remote changed: restart the host".into());
    }
    let mut stopped = Vec::new();
    for (name, old, now) in [
        ("node", format!("{:?}", old.node), format!("{:?}", now.node)),
        ("py", format!("{:?}", old.py), format!("{:?}", now.py)),
        ("bun", format!("{:?}", old.bun), format!("{:?}", now.bun)),
    ] {
        match (old == "None", now == "None") {
            (false, true) => stopped.push(name),
            _ if old == now => {}
            _ => {
                return Err(format!(
                    "runtimes.{name} added or changed: restart the host"
                ))
            }
        }
    }
    host.load(new.rows()).await?;
    *config = new;
    for name in stopped {
        let before = rutis_bridge::transport::local::running_processes();
        host.stop_runtime(name).await?;
        // Its process ends once its channel closed: one of those running
        // before (the others are other runtimes', which go on).
        let deadline = Instant::now() + RUNTIME_EXIT;
        loop {
            let running = rutis_bridge::transport::local::running_processes();
            if before.iter().any(|pid| !running.contains(pid)) {
                break;
            }
            if Instant::now() >= deadline {
                return Err(format!(
                    "the {name} runtime's process did not end within {RUNTIME_EXIT:?}"
                ));
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
    Ok(())
}

/// Presents each peer's token from the environment when dialing it.
struct EnvIdentity {
    local: PeerId,
}

impl Identity for EnvIdentity {
    fn local(&self) -> &PeerId {
        &self.local
    }

    fn credential(&self, peer: &PeerId) -> Option<Credential> {
        token(peer.as_str()).map(Credential::Bearer)
    }

    fn verify(&self, _presented: Presented<'_>) -> Option<PeerId> {
        None
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
