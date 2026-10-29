//! Go rows: one runtime per binary, several side by side, each started when
//! a row uses it. Rows name a plugin (`go:ping`) and find the binary that
//! has it from its manifest; plugins in different binaries use each other's
//! services through rutis.
#![cfg(feature = "go")]

mod common;

use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, Mutex, OnceLock};
use std::task::Poll;
use std::time::Duration;

use common::runtimes::{greet_as_main, layer, row_state, spawner, COLD};
use rutis::host::{BoxFuture, CtxHost, DynPlugin, Effect, FiberState, TypeKey};
use rutis::{Ctx, Error};
use rutis_bridge::runtime::RuntimeState;
use rutis_bridge::session::{host_key, settle, HostDispatch};
use rutis_loader::report::EntryStatus;
use rutis_loader::runtime::binary::{
    Binaries, BinaryResolver, BinaryRuntimeState, BinaryRuntimes, BinaryRuntimesHandle,
};
use rutis_loader::runtime::go::Go;
use rutis_loader::runtime::SessionRows;
use rutis_loader::{Chain, Loader, LoaderError, LoaderOptions, LoaderPlugin, ServiceCatalog};
use serde_json::{json, Value};
use tokio::sync::oneshot;

/// The fixture binaries (`tests/fixtures/go/cmd/*`), built once.
fn built() -> &'static Path {
    static BUILT: OnceLock<PathBuf> = OnceLock::new();
    BUILT.get_or_init(|| {
        let out = tempfile::tempdir().unwrap().keep();
        let module = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/go");
        for name in ["netkit", "netkit2", "caller", "dup", "spare"] {
            let status = std::process::Command::new("go")
                .args(["build", "-o"])
                .arg(out.join(exe(name)))
                .arg(format!("./cmd/{name}"))
                .current_dir(&module)
                .status()
                .expect("go is on PATH");
            assert!(status.success(), "go build {name}");
        }
        out
    })
}

fn exe(name: &str) -> String {
    match cfg!(windows) {
        true => format!("{name}.exe"),
        false => name.to_owned(),
    }
}

/// Put the fixture binary `name` into `dir` as `as_name`.
fn install(dir: &Path, name: &str, as_name: &str) -> PathBuf {
    let target = dir.join(exe(as_name));
    let staged = dir.join(format!(".{as_name}.new"));
    std::fs::copy(built().join(exe(name)), &staged).unwrap();
    // Renamed over: a running binary is replaced, not written in place.
    std::fs::rename(&staged, &target).unwrap();
    target
}

struct Fixture {
    root: Ctx,
    loader: Loader,
    resolver: Arc<BinaryResolver<Go>>,
    runtimes: BinaryRuntimesHandle<Go>,
    dir: tempfile::TempDir,
}

async fn fixture(idle: Option<Duration>, binaries: &[(&str, &str)]) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let plugins = dir.path().join("plugins");
    std::fs::create_dir_all(&plugins).unwrap();
    for (name, as_name) in binaries {
        install(&plugins, name, as_name);
    }
    let root = Ctx::root().unwrap();
    greet_as_main(&root);
    let catalog = ServiceCatalog::new();
    let resolver = Arc::new(
        BinaryResolver::<Go>::new(Binaries::new().dir(&plugins))
            .with_catalog(&catalog)
            .reserve(["node", "py"]),
    );
    let plugin = LoaderPlugin::new(
        Chain::new().with_shared(resolver.clone()),
        LoaderOptions {
            catalog,
            ..LoaderOptions::default()
        },
    );
    let loader = plugin.handle();
    root.plugin(plugin).await.unwrap();
    let go = BinaryRuntimes::<Go>::new(resolver.clone(), dir.path(), spawner()).idle(idle);
    let runtimes = go.handle();
    root.plugin(go).await.unwrap();
    Fixture {
        root,
        loader,
        resolver,
        runtimes,
        dir,
    }
}

fn row(id: &str, name: &str) -> Value {
    json!({ "id": id, "name": name })
}

/// The generation of the row's fiber while it is active: a later one has
/// loaded again since (each load is a new generation).
fn active_generation(loader: &Loader, id: &str) -> Option<u64> {
    match loader.get(id)?.status {
        EntryStatus::Running(snapshot) if snapshot.state == FiberState::Active => {
            Some(snapshot.generation)
        }
        _ => None,
    }
}

fn unresolved(loader: &Loader, id: &str) -> Option<String> {
    match loader.get(id)?.status {
        EntryStatus::Unresolved(error) => Some(error.to_string()),
        _ => None,
    }
}

fn runtime_state(runtimes: &BinaryRuntimesHandle<Go>, name: &str) -> Option<BinaryRuntimeState> {
    runtimes
        .runtimes()
        .into_iter()
        .find(|runtime| runtime.name == name)
        .map(|runtime| runtime.state)
}

/// Where the Go runtimes, the rows `ids` and the processes the local
/// transport started are: for a wait that timed out.
fn situation(fixture: &Fixture, ids: &[&str]) -> String {
    let runtimes: Vec<String> = fixture
        .runtimes
        .runtimes()
        .into_iter()
        .map(|runtime| format!("{}: {:?}", runtime.name, runtime.state))
        .collect();
    let rows: Vec<String> = ids
        .iter()
        .map(|id| match fixture.loader.get(id) {
            Some(entry) => format!("{id}: {:?}", entry.status),
            None => format!("{id}: none"),
        })
        .collect();
    let processes: Vec<String> = rutis_bridge::pipe::running_processes()
        .into_iter()
        .map(|pid| match alive(pid) {
            Some(true) => format!("{pid} (alive)"),
            Some(false) => format!("{pid} (gone)"),
            None => pid.to_string(),
        })
        .collect();
    format!("\nruntimes: {runtimes:?}\nrows: {rows:?}\nprocesses not waited for: {processes:?}")
}

/// Whether the process `pid` still exists (a zombie not waited for does);
/// `None` where this cannot tell (Windows has no `kill -0`).
fn alive(pid: u32) -> Option<bool> {
    if !cfg!(unix) {
        return None;
    }
    let status = std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stderr(std::process::Stdio::null())
        .status();
    Some(status.is_ok_and(|status| status.success()))
}

fn service(root: &Ctx, name: &str) -> Option<Arc<dyn HostDispatch>> {
    root.get_at::<dyn HostDispatch>(&host_key(name))
}

/// Call `method` of the service `name` synchronously, off the async runtime.
async fn call(root: &Ctx, name: &str, method: &str, args: Value) -> Result<Value, String> {
    let service = service(root, name).ok_or(format!("no service {name}"))?;
    let method = method.to_owned();
    tokio::task::spawn_blocking(move || service.invoke(&method, args.into()))
        .await
        .unwrap()
        .map_err(|error| error.to_string())
        .and_then(|value| value.json().map_err(|error| error.to_string()))
}

/// guarantee: loader/go-runtimes
#[tokio::test(flavor = "multi_thread")]
async fn rows_find_their_binary_and_call_across_binaries() {
    if !rutis_test_support::runtimes::enabled("go") {
        return;
    }
    let fixture = fixture(
        None,
        &[
            ("netkit", "netkit"),
            ("caller", "caller"),
            ("spare", "spare"),
        ],
    )
    .await;
    let plugins = fixture.dir.path().join("plugins");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // An executable without the SDK's marker is never run.
        let ran = fixture.dir.path().join("ran");
        let script = plugins.join("tool");
        std::fs::write(&script, format!("#!/bin/sh\ntouch {}\n", ran.display())).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        // One with the marker that prints no manifest is skipped, and said so.
        let broken = plugins.join("broken");
        std::fs::write(&broken, "#!/bin/sh\n# rutis-go-runtime:1\nexit 3\n").unwrap();
        std::fs::set_permissions(&broken, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    fixture
        .loader
        .reconcile(
            layer(vec![row("ping", "go:ping"), row("caller", "go:caller")]),
            None,
        )
        .await
        .unwrap();
    let loader = fixture.loader.clone();
    COLD.until(
        || {
            row_state(&loader, "ping") == Some(FiberState::Active)
                && row_state(&loader, "caller") == Some(FiberState::Active)
        },
        "both rows",
    )
    .await;
    assert_eq!(
        call(&fixture.root, "caller", "relay", json!(["hi"]))
            .await
            .unwrap(),
        json!("netkit pong hi"),
        "a sync call from one binary's plugin into another's"
    );
    let caller = service(&fixture.root, "caller").unwrap();
    let later = settle(caller.invoke("relayLater", json!(["hi"]).into()).unwrap())
        .await
        .unwrap();
    assert_eq!(later.json().unwrap(), json!("netkit later hi"));
    drop(caller);

    assert_eq!(
        runtime_state(&fixture.runtimes, "go-netkit"),
        Some(BinaryRuntimeState::Running)
    );
    assert_eq!(
        runtime_state(&fixture.runtimes, "go-caller"),
        Some(BinaryRuntimeState::Running)
    );
    assert_eq!(
        runtime_state(&fixture.runtimes, "go-spare"),
        Some(BinaryRuntimeState::NotStarted),
        "a binary no row uses is not started"
    );
    let meta = fixture.loader.get("ping").unwrap().meta;
    assert_eq!(meta["runtime"], json!("go-netkit"));
    assert_eq!(meta["source"], json!("go"));
    #[cfg(unix)]
    {
        assert!(
            !fixture.dir.path().join("ran").exists(),
            "an unmarked file was run"
        );
        let diagnostics = fixture.resolver.diagnostics();
        assert!(
            diagnostics
                .iter()
                .any(|line| line.contains("broken") && line.contains("exited")),
            "{diagnostics:?}"
        );
        assert!(
            !diagnostics.iter().any(|line| line.contains("tool")),
            "{diagnostics:?}"
        );
    }
    fixture.root.shutdown().await.unwrap();
}

/// guarantee: loader/go-runtimes
#[tokio::test(flavor = "multi_thread")]
async fn a_plugin_in_two_binaries_fails_naming_both_files() {
    if !rutis_test_support::runtimes::enabled("go") {
        return;
    }
    let fixture = fixture(None, &[("netkit", "netkit"), ("dup", "dup")]).await;
    fixture
        .loader
        .reconcile(layer(vec![row("p", "go:ping")]), None)
        .await
        .unwrap();
    let message = unresolved(&fixture.loader, "p").expect("go:ping is ambiguous");
    assert!(message.contains("several Go binaries"), "{message}");
    // The two files, for the plugin's author to rename one plugin.
    let file = |name: &str| format!("{}{name}", std::path::MAIN_SEPARATOR);
    assert!(
        message.contains(&file("dup")) && message.contains(&file("netkit")),
        "{message}"
    );

    fixture.root.shutdown().await.unwrap();
}

/// What `states` says of `name`, when it is down: why.
fn down(runtimes: &BinaryRuntimesHandle<Go>, name: &str) -> Option<String> {
    runtimes
        .states()
        .into_iter()
        .find(|(runtime, _)| runtime == name)
        .and_then(|(_, state)| match state {
            RuntimeState::Down(why) => Some(why),
            _ => None,
        })
}

/// A restart that cannot read the replacement keeps the old runtime; a
/// start that fails on a removed binary is reported as Stopped and is not
/// retried on every sweep.
/// guarantee: loader/runtime-down-stops-its-rows, loader/go-runtimes
#[tokio::test(flavor = "multi_thread")]
async fn a_failed_start_keeps_the_old_runtime_and_stays_stopped() {
    if !rutis_test_support::runtimes::enabled("go") {
        return;
    }
    let fixture = fixture(Some(Duration::from_millis(300)), &[("netkit", "netkit")]).await;
    fixture
        .loader
        .reconcile(layer(vec![row("ping", "go:ping")]), None)
        .await
        .unwrap();
    let loader = fixture.loader.clone();
    COLD.until(
        || {
            row_state(&loader, "ping") == Some(FiberState::Active)
                && service(&fixture.root, "ping").is_some()
        },
        "ping and its service",
    )
    .await;
    let running = process(&fixture.runtimes, "go-netkit").expect("go-netkit runs");

    // A replacement that has no manifest, in a file of its own (Windows
    // refuses to replace a running executable): the running runtime keeps
    // its process until it restarts, and the restart cannot start it.
    let builds = fixture.dir.path().join("builds");
    std::fs::create_dir_all(&builds).unwrap();
    let bad = builds.join(exe("netkit-bad"));
    std::fs::write(&bad, b"not a Go binary").unwrap();
    fixture.resolver.replace("go-netkit", &bad);
    let error = fixture.runtimes.restart("go-netkit").await.unwrap_err();
    assert!(!error.is_empty(), "{error}");
    assert!(Arc::ptr_eq(
        &running,
        &process(&fixture.runtimes, "go-netkit").expect("the old runtime still runs")
    ));
    assert_eq!(row_state(&loader, "ping"), Some(FiberState::Active));
    assert_eq!(
        call(&fixture.root, "ping", "echo", json!(["still"]))
            .await
            .unwrap(),
        json!("netkit pong still")
    );

    // A runtime that was stopped and whose binary is gone is reported as
    // Stopped, and sweeps leave it there.
    fixture.loader.reconcile(layer(vec![]), None).await.unwrap();
    let runtimes = fixture.runtimes.clone();
    COLD.until(
        || process(&runtimes, "go-netkit").is_none(),
        "the idle stop",
    )
    .await;
    std::fs::remove_file(&bad).unwrap();
    let error = fixture.runtimes.restart("go-netkit").await.unwrap_err();
    assert!(error.contains("no such file"), "{error}");
    let stopped = down(&fixture.runtimes, "go-netkit");
    assert!(
        stopped
            .as_deref()
            .is_some_and(|why| why.contains("no such file")),
        "{stopped:?}"
    );
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(down(&fixture.runtimes, "go-netkit"), stopped);
    fixture.root.shutdown().await.unwrap();
}

/// After a restart, rows resolve from the manifest the new process started
/// with, and a binary replaced since shows as replaced: the restart keeps
/// the record of what it started.
/// guarantee: loader/go-runtimes
#[tokio::test(flavor = "multi_thread")]
async fn a_restarted_runtime_keeps_its_start_manifest() {
    if !rutis_test_support::runtimes::enabled("go") {
        return;
    }
    let fixture = fixture(None, &[("netkit", "netkit")]).await;
    fixture
        .loader
        .reconcile(layer(vec![row("ping", "go:ping")]), None)
        .await
        .unwrap();
    let loader = fixture.loader.clone();
    let first = COLD
        .eventually(|| active_generation(&loader, "ping"), "the row")
        .await;
    fixture.runtimes.restart("go-netkit").await.unwrap();
    COLD.until(
        || active_generation(&loader, "ping").is_some_and(|generation| generation > first),
        "the row after the restart",
    )
    .await;

    // A new build, not restarted into: the running process is the old one.
    let builds = fixture.dir.path().join("builds");
    std::fs::create_dir_all(&builds).unwrap();
    let build = install(&builds, "netkit2", "netkit-2");
    fixture.resolver.replace("go-netkit", &build);
    let info = fixture
        .runtimes
        .runtimes()
        .into_iter()
        .find(|runtime| runtime.name == "go-netkit")
        .unwrap();
    assert!(info.replaced, "the runtime shows the pending restart");
    fixture
        .loader
        .reconcile(
            layer(vec![row("ping", "go:ping"), row("extra", "go:extra")]),
            None,
        )
        .await
        .unwrap();
    let message = unresolved(&fixture.loader, "extra").expect("extra waits for the restart");
    assert!(
        message.contains("replaced") && message.contains("go-netkit"),
        "{message}"
    );
    fixture.root.shutdown().await.unwrap();
}

/// A Go binary (a script with the SDK's marker) at `path` that prints the
/// plugin `late` the first time its manifest is read and deletes itself.
#[cfg(unix)]
fn vanishing(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::write(
        path,
        "#!/bin/sh\n# rutis-go-runtime:1\n\
         [ \"$1\" = --rutis-manifest ] || exit 1\n\
         rm -f \"$0\"\n\
         echo '{\"pluginApi\":1,\"plugins\":{\"late\":{\"inject\":[],\"provides\":{}}}}'\n",
    )
    .unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// A Go binary at `path` whose manifest read always fails; each read adds
/// a line to the file `runs`. `version` makes a file of another size. It
/// is written aside and renamed into place: one change of the file, which
/// a sweep cannot see half made (written, then made executable).
#[cfg(unix)]
fn always_failing(path: &Path, runs: &Path, version: usize) {
    use std::os::unix::fs::PermissionsExt;
    // Outside the binaries' directory: not a binary of its own meanwhile.
    let staged = runs.with_file_name("staged");
    std::fs::write(
        &staged,
        format!(
            "#!/bin/sh\n# rutis-go-runtime:1\n# {}\necho run >> '{}'\nexit 3\n",
            "v".repeat(version),
            runs.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::rename(&staged, path).unwrap();
}

/// A binary a row uses that is gone when its runtime starts: the failure is
/// reported, sweeps do not start it again (every 100 ms before #539), and
/// it is tried again once its file changes, once per change.
/// guarantee: loader/go-runtimes
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn a_missing_binary_is_tried_again_only_when_its_file_changes() {
    let fixture = fixture(None, &[]).await;
    let binary = fixture.dir.path().join("plugins").join("late");
    let count = fixture.dir.path().join("runs");
    vanishing(&binary);
    // The row resolves from the manifest; the start reads the binary again
    // and finds it gone.
    fixture
        .loader
        .reconcile(layer(vec![row("late", "go:late")]), None)
        .await
        .unwrap();
    let runtimes = fixture.runtimes.clone();
    let why = COLD
        .eventually(|| down(&runtimes, "go-late"), "the failed start")
        .await;
    assert!(why.contains("no Go runtime go-late"), "{why}");
    assert!(!binary.exists());
    // Sweeps (every 100 ms) leave it.
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(down(&fixture.runtimes, "go-late").as_deref(), Some(&*why));

    // The file is back, its manifest failing: tried once, then left again.
    always_failing(&binary, &count, 1);
    COLD.until(|| runs(&count) == 1, "a start on the file that came back")
        .await;
    // A binary in a directory whose manifest fails is not one of its
    // runtimes: the start fails as before.
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(runs(&count), 1, "sweeps read the failing binary again");
    assert_eq!(down(&fixture.runtimes, "go-late").as_deref(), Some(&*why));

    // Changed again: tried once more.
    always_failing(&binary, &count, 2);
    COLD.until(|| runs(&count) == 2, "a start on the changed file")
        .await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(runs(&count), 2, "sweeps read the failing binary again");
    assert_ne!(row_state(&fixture.loader, "late"), Some(FiberState::Active));
    fixture.root.shutdown().await.unwrap();
}

/// guarantee: loader/runtime-down-stops-its-rows, loader/go-runtimes
#[tokio::test(flavor = "multi_thread")]
async fn a_crash_stops_only_its_runtimes_rows_and_is_not_restarted() {
    if !rutis_test_support::runtimes::enabled("go") {
        return;
    }
    // The idle stop of spare, once unused, is the marker that sweeps ran.
    let fixture = fixture(
        Some(Duration::from_millis(300)),
        &[
            ("netkit", "netkit"),
            ("caller", "caller"),
            ("spare", "spare"),
        ],
    )
    .await;
    fixture
        .loader
        .reconcile(
            layer(vec![
                row("ping", "go:ping"),
                row("caller", "go:caller"),
                row("spare", "go:spare"),
            ]),
            None,
        )
        .await
        .unwrap();
    let loader = fixture.loader.clone();
    // A row's services are published once its runtime reports them, after
    // the row is active.
    COLD.until(
        || {
            ["ping", "caller", "spare"].iter().all(|id| {
                row_state(&loader, id) == Some(FiberState::Active)
                    && service(&fixture.root, id).is_some()
            })
        },
        "three rows and their services",
    )
    .await;
    let crashed = call(&fixture.root, "ping", "crash", json!([])).await;
    let runtimes = fixture.runtimes.clone();
    COLD.eventually_with(
        || {
            matches!(
                runtime_state(&runtimes, "go-netkit"),
                Some(BinaryRuntimeState::Stopped(_))
            )
            .then_some(())
        },
        "the crashed runtime",
        || {
            let situation = situation(&fixture, &["ping", "caller", "spare"]);
            format!("\nthe crash call: {crashed:?}{situation}")
        },
    )
    .await;
    COLD.until(
        || {
            row_state(&loader, "ping") != Some(FiberState::Active)
                && row_state(&loader, "caller") != Some(FiberState::Active)
        },
        "its rows and their users stop",
    )
    .await;
    assert_eq!(
        row_state(&loader, "spare"),
        Some(FiberState::Active),
        "another binary runs on"
    );
    assert_eq!(
        call(&fixture.root, "spare", "echo", json!(["still"]))
            .await
            .unwrap(),
        json!("spare pong still")
    );
    // Resolving its rows again does not restart it, nor does a sweep. The
    // decision is `want`'s: the resolution calls it before `reload`
    // returns, and every sweep calls it for a runtime its rows wait for.
    // A restart it decides removes the runtime first thing in its task.
    fixture.loader.reload("ping").await.unwrap();
    // The marker: spare, no longer used, stops once idle. A sweep stops it
    // after calling `want` for go-netkit, and its stop then waits for the
    // process to end; a restart decided before is under way by then.
    fixture
        .loader
        .reconcile(
            layer(vec![row("ping", "go:ping"), row("caller", "go:caller")]),
            None,
        )
        .await
        .unwrap();
    let runtimes = fixture.runtimes.clone();
    COLD.until(
        || runtime_state(&runtimes, "go-spare") == Some(BinaryRuntimeState::NotStarted),
        "the idle stop of spare",
    )
    .await;
    assert!(matches!(
        runtime_state(&fixture.runtimes, "go-netkit"),
        Some(BinaryRuntimeState::Stopped(_))
    ));
    // What a status line reads: the crashed runtime, down; the other one
    // running; the idle one, not there.
    let states: Vec<(String, &str)> = fixture
        .runtimes
        .states()
        .into_iter()
        .map(|(name, state)| {
            let state = match state {
                RuntimeState::Down(_) => "down",
                RuntimeState::Ready(_) => "ready",
                _ => "other",
            };
            (name, state)
        })
        .collect();
    assert_eq!(
        states,
        [("go-caller".into(), "ready"), ("go-netkit".into(), "down")]
    );
    // A restart does.
    fixture.runtimes.restart("go-netkit").await.unwrap();
    COLD.until(
        || {
            ["ping", "caller"].iter().all(|id| {
                row_state(&loader, id) == Some(FiberState::Active)
                    && service(&fixture.root, id).is_some()
            })
        },
        "the rows and their services again",
    )
    .await;
    assert_eq!(
        call(&fixture.root, "caller", "relay", json!(["back"]))
            .await
            .unwrap(),
        json!("netkit pong back")
    );
    assert!(fixture
        .runtimes
        .states()
        .iter()
        .all(|(_, state)| matches!(state, RuntimeState::Ready(_))));
    fixture.root.shutdown().await.unwrap();
}

/// guarantee: loader/go-runtimes
#[tokio::test(flavor = "multi_thread")]
async fn an_idle_runtime_stops_and_starts_again_when_used() {
    if !rutis_test_support::runtimes::enabled("go") {
        return;
    }
    let fixture = fixture(
        Some(Duration::from_millis(300)),
        &[("netkit", "netkit"), ("spare", "spare")],
    )
    .await;
    fixture
        .loader
        .reconcile(
            layer(vec![row("ping", "go:ping"), row("spare", "go:spare")]),
            None,
        )
        .await
        .unwrap();
    let loader = fixture.loader.clone();
    COLD.until(
        || {
            row_state(&loader, "ping") == Some(FiberState::Active)
                && row_state(&loader, "spare") == Some(FiberState::Active)
        },
        "the rows",
    )
    .await;
    let ping = COLD
        .eventually(|| service(&fixture.root, "ping"), "ping's service")
        .await;
    // In use: it keeps running past the idle time. The marker: spare, used
    // no longer, stops once idle. go-netkit started no later, so unused it
    // would be idle as long: a sweep decides every runtime's stop in one
    // pass under one lock, and a stop removes its runtime first thing,
    // while spare's then waits for its process to end. Stopped and started
    // again by a later sweep, its row would publish `ping` anew.
    fixture
        .loader
        .reconcile(layer(vec![row("ping", "go:ping")]), None)
        .await
        .unwrap();
    let runtimes = fixture.runtimes.clone();
    COLD.until(
        || runtime_state(&runtimes, "go-spare") == Some(BinaryRuntimeState::NotStarted),
        "the idle stop of spare",
    )
    .await;
    assert_eq!(
        runtime_state(&fixture.runtimes, "go-netkit"),
        Some(BinaryRuntimeState::Running)
    );
    let now = service(&fixture.root, "ping").expect("ping's service");
    assert!(
        std::ptr::addr_eq(Arc::as_ptr(&now), Arc::as_ptr(&ping)),
        "the same row, never stopped"
    );
    drop((now, ping));
    assert_eq!(
        call(&fixture.root, "ping", "echo", json!(["in use"]))
            .await
            .unwrap(),
        json!("netkit pong in use")
    );

    fixture.loader.reconcile(layer(vec![]), None).await.unwrap();
    COLD.until(
        || runtime_state(&runtimes, "go-netkit") == Some(BinaryRuntimeState::NotStarted),
        "the idle stop",
    )
    .await;

    fixture
        .loader
        .reconcile(layer(vec![row("ping", "go:ping")]), None)
        .await
        .unwrap();
    COLD.until(
        || row_state(&loader, "ping") == Some(FiberState::Active),
        "the row again",
    )
    .await;
    // As at the start: the row's service is announced after it is active.
    COLD.eventually(|| service(&fixture.root, "ping"), "ping's service again")
        .await;
    assert_eq!(
        call(&fixture.root, "ping", "echo", json!(["again"]))
            .await
            .unwrap(),
        json!("netkit pong again")
    );
    fixture.root.shutdown().await.unwrap();
}

/// A binary replaced in place (renamed over: Windows refuses to replace a
/// running executable, so there a new build goes to a new file, below).
/// guarantee: loader/go-runtimes
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn a_replaced_binary_takes_effect_when_its_runtime_restarts() {
    if !rutis_test_support::runtimes::enabled("go") {
        return;
    }
    let fixture = fixture(None, &[("netkit", "netkit")]).await;
    fixture
        .loader
        .reconcile(layer(vec![row("ping", "go:ping")]), None)
        .await
        .unwrap();
    let loader = fixture.loader.clone();
    COLD.until(
        || row_state(&loader, "ping") == Some(FiberState::Active),
        "the row",
    )
    .await;

    install(&fixture.dir.path().join("plugins"), "netkit2", "netkit");
    fixture
        .loader
        .reconcile(
            layer(vec![row("ping", "go:ping"), row("extra", "go:extra")]),
            None,
        )
        .await
        .unwrap();
    let message = unresolved(&fixture.loader, "extra").expect("extra waits for the restart");
    assert!(
        message.contains("replaced") && message.contains("go-netkit"),
        "{message}"
    );
    let info = fixture
        .runtimes
        .runtimes()
        .into_iter()
        .find(|runtime| runtime.name == "go-netkit")
        .unwrap();
    assert!(info.replaced, "the runtime shows the pending restart");
    assert_eq!(
        call(&fixture.root, "ping", "echo", json!(["old"]))
            .await
            .unwrap(),
        json!("netkit pong old"),
        "the running process is the old binary"
    );

    fixture.runtimes.restart("go-netkit").await.unwrap();
    fixture.loader.reload("extra").await.unwrap();
    COLD.until(
        || {
            row_state(&loader, "ping") == Some(FiberState::Active)
                && row_state(&loader, "extra") == Some(FiberState::Active)
        },
        "both rows on the new binary",
    )
    .await;
    COLD.until(
        || {
            let service = service(&fixture.root, "ping");
            service
                .and_then(|service| service.invoke("echo", json!(["new"]).into()).ok())
                .and_then(|value| value.json().ok())
                == Some(json!("netkit2 pong new"))
        },
        "the new ping",
    )
    .await;
    fixture.root.shutdown().await.unwrap();
}

/// A new build in a new file, as `rutis-host dev` makes on every platform:
/// the runtime keeps its name, and the build takes effect on restart.
/// guarantee: loader/go-runtimes
#[tokio::test(flavor = "multi_thread")]
async fn a_new_build_takes_effect_when_its_runtime_restarts() {
    if !rutis_test_support::runtimes::enabled("go") {
        return;
    }
    let fixture = fixture(None, &[("netkit", "netkit")]).await;
    fixture
        .loader
        .reconcile(layer(vec![row("ping", "go:ping")]), None)
        .await
        .unwrap();
    let loader = fixture.loader.clone();
    COLD.until(
        || row_state(&loader, "ping") == Some(FiberState::Active),
        "the row",
    )
    .await;
    let builds = fixture.dir.path().join("builds");
    std::fs::create_dir_all(&builds).unwrap();
    let build = install(&builds, "netkit2", "netkit-2");
    fixture.resolver.replace("go-netkit", &build);
    fixture.runtimes.restart("go-netkit").await.unwrap();
    COLD.until(
        || {
            service(&fixture.root, "ping")
                .and_then(|service| service.invoke("echo", json!(["new"]).into()).ok())
                .and_then(|value| value.json().ok())
                == Some(json!("netkit2 pong new"))
        },
        "the new build",
    )
    .await;
    assert_eq!(
        fixture.loader.get("ping").unwrap().meta["runtime"],
        json!("go-netkit"),
        "the runtime keeps its name"
    );
    fixture.root.shutdown().await.unwrap();
}

/// A restart whose caller gives up half way (a timeout around it) leaves
/// the runtime usable: the rows run again, and the next restart goes on.
/// guarantee: loader/go-runtimes
#[tokio::test(flavor = "multi_thread")]
async fn a_restart_given_up_half_way_leaves_the_runtime_usable() {
    if !rutis_test_support::runtimes::enabled("go") {
        return;
    }
    let fixture = fixture(None, &[("netkit", "netkit")]).await;
    fixture
        .loader
        .reconcile(layer(vec![row("ping", "go:ping")]), None)
        .await
        .unwrap();
    let loader = fixture.loader.clone();
    let first = COLD
        .eventually(|| active_generation(&loader, "ping"), "the row")
        .await;
    // clock: the short timeout is the scenario, not a wait: the caller gives
    // up half way through the restart, whatever the restart does meanwhile.
    let _ = tokio::time::timeout(
        Duration::from_millis(1),
        fixture.runtimes.restart("go-netkit"),
    )
    .await;
    // Active is not enough here, nor below: the row stays active on the
    // runtime that is stopping until the stop reaches it (#307). A later
    // generation is the row loaded again, on the new runtime.
    let again = COLD
        .eventually(
            || active_generation(&loader, "ping").filter(|generation| *generation > first),
            "the row again",
        )
        .await;
    COLD.within(
        "a restart after one given up",
        fixture.runtimes.restart("go-netkit"),
    )
    .await
    .unwrap();
    COLD.until(
        || active_generation(&loader, "ping").is_some_and(|generation| generation > again),
        "the row after the restart",
    )
    .await;
    // Active is not yet published: the row's service follows (#403).
    COLD.eventually(|| service(&fixture.root, "ping"), "the row's service")
        .await;
    assert_eq!(
        call(&fixture.root, "ping", "echo", json!(["still"]))
            .await
            .unwrap(),
        json!("netkit pong still")
    );
    fixture.root.shutdown().await.unwrap();
}

/// What the first cleanup of a [`Gate`] takes: it says it began, then waits
/// for the release.
type Hold = Option<(oneshot::Sender<()>, oneshot::Receiver<()>)>;

/// A user of a runtime's rows whose cleanup waits until released: the
/// runtime stops only after its users did (D14), so it stays stopping
/// until then.
struct Gate {
    injects: [TypeKey; 1],
    hold: Arc<Mutex<Hold>>,
}

impl DynPlugin for Gate {
    fn name(&self) -> &str {
        "gate"
    }

    fn injects(&self) -> Vec<TypeKey> {
        self.injects.to_vec()
    }

    fn apply<'a>(&'a self, _ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, Error>> {
        let hold = self.hold.clone();
        Box::pin(async move {
            Ok(Effect::AsyncDisposer(Box::new(move || {
                Box::pin(async move {
                    let held = hold.lock().unwrap().take();
                    if let Some((began, release)) = held {
                        let _ = began.send(());
                        let _ = release.await;
                    }
                    Ok(())
                })
            })))
        })
    }
}

/// Poll `future` once: its output if that finishes it.
async fn poll_once<F: Future + Unpin>(future: &mut F) -> Option<F::Output> {
    std::future::poll_fn(|cx| match Pin::new(&mut *future).poll(cx) {
        Poll::Ready(output) => Poll::Ready(Some(output)),
        Poll::Pending => Poll::Ready(None),
    })
    .await
}

/// A restart its caller gave up goes on to the end, as one waited for: the
/// next restart waits for it, and no second process runs under the name
/// while the first is stopping. Before #307 the given-up restart stopped
/// half way and freed the name while the old runtime was still stopping:
/// the next restart started a new runtime at once, and returned while the
/// old one still ran.
/// guarantee: loader/go-runtimes
#[tokio::test(flavor = "multi_thread")]
async fn a_restart_given_up_half_way_still_finishes_before_the_next() {
    if !rutis_test_support::runtimes::enabled("go") {
        return;
    }
    let fixture = fixture(None, &[("netkit", "netkit")]).await;
    fixture
        .loader
        .reconcile(layer(vec![row("ping", "go:ping")]), None)
        .await
        .unwrap();
    let loader = fixture.loader.clone();
    let first = COLD
        .eventually(|| active_generation(&loader, "ping"), "the row")
        .await;
    let (began, began_rx) = oneshot::channel();
    let (release, release_rx) = oneshot::channel();
    let gate = fixture.root.plugin(Gate {
        injects: [SessionRows::key("go-netkit")],
        hold: Arc::new(Mutex::new(Some((began, release_rx)))),
    });
    COLD.until(|| gate.state().state == FiberState::Active, "the gate")
        .await;

    // The restart begins, and its caller gives up while the old runtime
    // stops: it stays stopping until the gate is released.
    let mut given_up = Box::pin(fixture.runtimes.restart("go-netkit"));
    assert!(poll_once(&mut given_up).await.is_none());
    COLD.within("the old runtime to stop its users", began_rx)
        .await
        .unwrap();
    drop(given_up);

    let mut next = Box::pin(fixture.runtimes.restart("go-netkit"));
    let early = poll_once(&mut next).await;
    assert!(
        early.is_none(),
        "a restart returned while the runtime before it was still stopping: {early:?}"
    );
    release.send(()).unwrap();
    COLD.within("the restart after the given-up one", next)
        .await
        .unwrap();
    COLD.until(
        || active_generation(&loader, "ping").is_some_and(|generation| generation > first),
        "the row after the restarts",
    )
    .await;
    assert_eq!(
        call(&fixture.root, "ping", "echo", json!(["again"]))
            .await
            .unwrap(),
        json!("netkit pong again")
    );
    fixture.root.shutdown().await.unwrap();
}

/// A Go binary (a script with the SDK's marker) at `path` whose manifest
/// read fails the first time and prints the plugin `late` from then on;
/// each read adds a line to the file `runs`.
#[cfg(unix)]
fn failing_once(path: &Path, runs: &Path) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::write(
        path,
        format!(
            "#!/bin/sh\n# rutis-go-runtime:1\n\
             [ \"$1\" = --rutis-manifest ] || exit 1\n\
             echo run >> '{runs}'\n\
             [ \"$(wc -l < '{runs}')\" -gt 1 ] || exit 3\n\
             echo '{{\"pluginApi\":1,\"plugins\":{{\"late\":{{\"inject\":[],\"provides\":{{}}}}}}}}'\n",
            runs = runs.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[cfg(unix)]
fn runs(runs: &Path) -> usize {
    std::fs::read_to_string(runs)
        .map(|text| text.lines().count())
        .unwrap_or(0)
}

/// A manifest read that failed is read again, with its file unchanged, when
/// a row asks for a plugin no binary has, once the failure has stood for
/// 5 s: it may have passed. Before #306 a failure stayed cached until the
/// file changed (a binary just replaced on disk then had no plugins). Not
/// sooner: lookups of missing names do not run a failing binary each time.
#[cfg(unix)]
#[tokio::test(start_paused = true)]
async fn a_failed_manifest_read_is_read_again_for_a_missing_plugin_once_it_stood() {
    use rutis_loader::Resolver;
    let dir = tempfile::tempdir().unwrap();
    let plugins = dir.path().join("plugins");
    std::fs::create_dir_all(&plugins).unwrap();
    let count = dir.path().join("runs");
    failing_once(&plugins.join("later"), &count);
    let resolver = BinaryResolver::<Go>::new(Binaries::new().dir(&plugins));

    for name in ["go:late", "go:late", "go:typo"] {
        match resolver.resolve(name).await {
            Err(LoaderError::NotFound { .. }) => {}
            other => panic!("{name}: {:?}", other.map(|resolved| resolved.meta.clone())),
        }
    }
    assert_eq!(runs(&count), 1, "a failed read is not read again at once");
    tokio::time::advance(Duration::from_secs(4)).await;
    assert!(resolver.resolve("go:typo").await.is_err());
    assert_eq!(runs(&count), 1, "nor within 5 s");

    tokio::time::advance(Duration::from_secs(1)).await;
    let resolved = resolver.resolve("go:late").await.expect("read again");
    assert_eq!(resolved.meta["runtime"], json!("go-later"));
    assert_eq!(runs(&count), 2);
}

/// The same for a listed binary.
#[cfg(unix)]
#[tokio::test(start_paused = true)]
async fn a_failed_manifest_read_of_a_listed_binary_is_read_again_once_it_stood() {
    use rutis_loader::Resolver;
    let dir = tempfile::tempdir().unwrap();
    let count = dir.path().join("runs");
    let binary = dir.path().join("lister");
    failing_once(&binary, &count);
    let resolver = BinaryResolver::<Go>::new(Binaries::new().file(&binary));

    for _ in 0..2 {
        match resolver.resolve("go:late").await {
            Err(LoaderError::NotFound { .. }) => {}
            other => panic!("{:?}", other.map(|resolved| resolved.meta.clone())),
        }
    }
    assert_eq!(runs(&count), 1, "a failed read is not read again at once");

    tokio::time::advance(Duration::from_secs(5)).await;
    let resolved = resolver.resolve("go:late").await.expect("read again");
    assert_eq!(resolved.meta["runtime"], json!("go-lister"));
    assert_eq!(runs(&count), 2);
}

/// A Go binary (a script with the SDK's marker) at `path` built against a
/// Go SDK older than 0.13: its manifest has the plugin `old`, and its
/// session greets in the format of 0.12, without `events-v2`; each start
/// adds a line to the file `runs`. It takes its channel as a Go binary
/// does (its first argument): `fd:3`, a socket path to dial, or, with
/// `RUTIS_LOCAL_HANDOVER=loopback`, `tcp:<address>` presenting the token of
/// `RUTIS_CHANNEL_TOKEN` first.
#[cfg(unix)]
fn old_sdk(path: &Path, runs: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let script = format!(
        r#"#!/usr/bin/env python3
# rutis-go-runtime:1
import os, socket, sys
if sys.argv[1] == "--rutis-manifest":
    print('{{"pluginApi":1,"plugins":{{"old":{{"inject":[],"provides":{{}}}}}}}}')
    sys.exit(0)
with open({runs:?}, "a") as runs:
    runs.write("run\n")
channel = sys.argv[1]
if channel.startswith("fd:"):
    conn = socket.socket(fileno=int(channel[3:]))
elif channel.startswith("tcp:"):
    host, port = channel[4:].rsplit(":", 1)
    conn = socket.create_connection((host, int(port)))
    conn.sendall((os.environ["RUTIS_CHANNEL_TOKEN"] + "\n").encode())
else:
    conn = socket.socket(socket.AF_UNIX)
    conn.connect(channel.removeprefix("unix:"))
conn.sendall(b'{{"op":"hello","version":2}}\n')
while conn.recv(4096):
    pass
"#,
        runs = runs.display().to_string()
    );
    std::fs::write(path, script).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// A binary of an older Go SDK (here one speaking protocol 2) is refused at
/// the handshake by protocol version, as a local runtime of Node, Bun or
/// Python is: its runtime
/// is down saying why, its binary is marked refused and is not started
/// again, and its rows fail with the same message instead of waiting for
/// it. A new binary in its place is tried.
///
/// guarantee: events/old-peer-refused, loader/go-runtimes
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn a_binary_of_an_old_sdk_is_refused_and_its_rows_fail() {
    let fixture = fixture(None, &[]).await;
    let plugins = fixture.dir.path().join("plugins");
    let count = fixture.dir.path().join("runs");
    old_sdk(&plugins.join("legacy"), &count);
    let refusal =
        "incompatible session: the far end speaks protocol 2, this side 4: upgrade the far end";
    fixture
        .loader
        .reconcile(
            layer(vec![row("first", "go:old"), row("second", "go:old")]),
            None,
        )
        .await
        .unwrap();
    let loader = fixture.loader.clone();
    COLD.eventually_with(
        || {
            ["first", "second"]
                .iter()
                .all(|id| unresolved(&loader, id).is_some_and(|why| why.contains(refusal)))
                .then_some(())
        },
        "the rows fail with the refusal",
        || situation(&fixture, &["first", "second"]),
    )
    .await;
    match runtime_state(&fixture.runtimes, "go-legacy") {
        Some(BinaryRuntimeState::Stopped(why)) => assert!(why.contains(refusal), "{why}"),
        other => panic!("the runtime: {other:?}"),
    }
    // Not started again: by sweeps, by its rows resolving again, by a new
    // row.
    fixture.loader.reload("first").await.unwrap_err();
    fixture
        .loader
        .reconcile(
            layer(vec![
                row("first", "go:old"),
                row("second", "go:old"),
                row("third", "go:old"),
            ]),
            None,
        )
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(runs(&count), 1, "started again");
    assert!(unresolved(&fixture.loader, "third").is_some_and(|why| why.contains(refusal)));
    // A new binary in its place is tried.
    std::fs::remove_file(plugins.join("legacy")).unwrap();
    old_sdk(&plugins.join("legacy"), &count);
    let _ = fixture.loader.reload("third").await;
    COLD.until(|| runs(&count) == 2, "the new binary started")
        .await;
    fixture.root.shutdown().await.unwrap();
}

/// guarantee: loader/go-runtimes
#[tokio::test(flavor = "multi_thread")]
async fn names_of_other_resolvers_are_not_claimed() {
    if !rutis_test_support::runtimes::enabled("go") {
        return;
    }
    let fixture = fixture(None, &[("netkit", "netkit")]).await;
    use rutis_loader::Resolver;
    for name in ["py:weather", "go-nowhere:ping", "weather", "go:"] {
        match fixture.resolver.resolve(name).await {
            Err(LoaderError::NotFound { .. }) => {}
            other => panic!("{name}: {other:?}"),
        }
    }
    fixture.root.shutdown().await.unwrap();
}

/// The process of the runtime `name`, while it runs.
fn process(
    runtimes: &BinaryRuntimesHandle<Go>,
    name: &str,
) -> Option<Arc<rutis_bridge::runtime::Process>> {
    runtimes
        .states()
        .into_iter()
        .find(|(runtime, _)| runtime == name)
        .and_then(|(_, state)| match state {
            RuntimeState::Ready(process) => Some(process),
            _ => None,
        })
}
