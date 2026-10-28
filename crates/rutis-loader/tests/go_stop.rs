//! A Go runtime that does not stop within the shutdown deadline is killed
//! before another process starts under its name (#538). A test binary of
//! its own: it sets `RUTIS_SHUTDOWN_TIMEOUT`, which everything in the
//! process reads.
#![cfg(feature = "go")]

mod common;

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use common::runtimes::{layer, COLD};
use rutis::host::{CtxHost, FiberState};
use rutis::Ctx;
use rutis_bridge::session::{host_key, HostDispatch};
use rutis_bridge::transport::local::{kill_processes, running_processes};
use rutis_loader::report::EntryStatus;
use rutis_loader::runtime::binary::{Binaries, BinaryResolver, BinaryRuntimes};
use rutis_loader::runtime::go::Go;
use rutis_loader::{Chain, Loader, LoaderOptions, LoaderPlugin, ServiceCatalog};
use serde_json::{json, Value};

/// The runtime's own deadline here, in seconds.
const DEADLINE: f64 = 1.0;

/// The fixture binary `stuck`, whose plugin never finishes its cleanup,
/// built into `dir`.
fn build_stuck(dir: &Path) {
    let name = if cfg!(windows) { "stuck.exe" } else { "stuck" };
    let status = std::process::Command::new("go")
        .args(["build", "-o"])
        .arg(dir.join(name))
        .arg("./cmd/stuck")
        .current_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/go"))
        .status()
        .expect("go is on PATH");
    assert!(status.success(), "go build stuck");
}

/// The process serving the service `stuck`, once it is published.
fn pid(root: &Ctx) -> Option<u64> {
    let service = root.get_at::<dyn HostDispatch>(&host_key("stuck"))?;
    let value: Value = service.invoke("pid", json!([]).into()).ok()?.json().ok()?;
    value.as_u64()
}

fn generation(loader: &Loader) -> Option<u64> {
    match loader.get("stuck")?.status {
        EntryStatus::Running(snapshot) if snapshot.state == FiberState::Active => {
            Some(snapshot.generation)
        }
        _ => None,
    }
}

/// guarantee: loader/go-runtimes
#[tokio::test(flavor = "multi_thread")]
async fn a_runtime_that_does_not_stop_in_time_is_killed_before_the_next_starts() {
    // Before anything starts: every runtime process inherits it.
    std::env::set_var("RUTIS_SHUTDOWN_TIMEOUT", DEADLINE.to_string());
    let dir = tempfile::tempdir().unwrap();
    let plugins = dir.path().join("plugins");
    std::fs::create_dir_all(&plugins).unwrap();
    build_stuck(&plugins);
    let root = Ctx::root().unwrap();
    let catalog = ServiceCatalog::new();
    let resolver =
        Arc::new(BinaryResolver::<Go>::new(Binaries::new().dir(&plugins)).with_catalog(&catalog));
    let plugin = LoaderPlugin::new(
        Chain::new().with_shared(resolver.clone()),
        LoaderOptions {
            catalog,
            ..LoaderOptions::default()
        },
    );
    let loader = plugin.handle();
    root.plugin(plugin).await.unwrap();
    let go = BinaryRuntimes::<Go>::new(resolver.clone(), dir.path());
    let runtimes = go.handle();
    root.plugin(go).await.unwrap();
    loader
        .reconcile(
            layer(vec![json!({ "id": "stuck", "name": "go:stuck" })]),
            None,
        )
        .await
        .unwrap();
    let first = COLD.eventually(|| pid(&root), "the first process").await;
    let first_generation = generation(&loader).expect("the row is active");
    assert_eq!(running_processes(), vec![first as u32]);

    // How many runtime processes ran at once, while the restart goes on.
    let most = Arc::new(AtomicUsize::new(0));
    let watching = Arc::new(AtomicBool::new(true));
    let watcher = std::thread::spawn({
        let (most, watching) = (most.clone(), watching.clone());
        move || {
            while watching.load(Ordering::SeqCst) {
                most.fetch_max(running_processes().len(), Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(2));
            }
        }
    });
    let started = Instant::now();
    COLD.within("the restart", runtimes.restart("go-stuck"))
        .await
        .unwrap();
    let took = started.elapsed();
    let second = COLD
        .eventually(
            || pid(&root).filter(|pid| *pid != first),
            "the next process",
        )
        .await;
    watching.store(false, Ordering::SeqCst);
    watcher.join().unwrap();

    assert!(
        took >= Duration::from_secs_f64(DEADLINE),
        "the old runtime got its deadline: {took:?}"
    );
    assert_eq!(
        most.load(Ordering::SeqCst),
        1,
        "the next process started while the old one still ran"
    );
    assert_eq!(running_processes(), vec![second as u32]);

    // The old runtime's stop is over: nothing of it withdraws the new
    // process's row or service later.
    let generation_now = generation(&loader).expect("the row runs again");
    assert!(generation_now > first_generation);
    tokio::time::sleep(Duration::from_secs_f64(DEADLINE * 2.0)).await;
    assert_eq!(generation(&loader), Some(generation_now));
    assert_eq!(pid(&root), Some(second), "the new service stays");

    // This runtime does not stop by itself either.
    kill_processes();
    root.shutdown().await.unwrap();
}
