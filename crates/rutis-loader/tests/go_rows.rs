//! Go rows: one runtime per binary, several side by side, each started when
//! a row uses it. Rows name a plugin (`go:ping`) and find the binary that
//! has it from its manifest; plugins in different binaries use each other's
//! services through rutis.
#![cfg(feature = "go")]

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use rutis::{Ctx, FiberState};
use rutis_bridge::session::{host_key, settle, HostDispatch};
use rutis_loader::{
    Chain, EntryStatus, GoBinaries, GoResolver, GoRuntimeState, GoRuntimes, GoRuntimesHandle,
    Layer, Loader, LoaderError, LoaderOptions, LoaderPlugin, Patch, ServiceCatalog,
};
use rutis_test_support::{Guard, HANG_GUARD};
use serde_json::{json, Value};

/// Twice HANG_GUARD: Go runtimes start cold, one per binary (#238).
const COLD: Guard = Guard(Duration::from_secs(HANG_GUARD.as_secs() * 2));

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
    resolver: Arc<GoResolver>,
    runtimes: GoRuntimesHandle,
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
    let catalog = ServiceCatalog::new();
    let resolver = Arc::new(
        GoResolver::new(GoBinaries::new().dir(&plugins))
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
    let go = GoRuntimes::new(resolver.clone(), dir.path()).idle(idle);
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

fn layer(rows: Vec<Value>) -> Vec<Layer> {
    let patches: Vec<Patch> = serde_json::from_value(json!([{ "insert": rows }])).unwrap();
    vec![Layer::new("rows", patches)]
}

fn row(id: &str, name: &str) -> Value {
    json!({ "id": id, "name": name })
}

fn state(loader: &Loader, id: &str) -> Option<FiberState> {
    match loader.get(id)?.status {
        EntryStatus::Running(snapshot) => Some(snapshot.state),
        _ => None,
    }
}

fn unresolved(loader: &Loader, id: &str) -> Option<String> {
    match loader.get(id)?.status {
        EntryStatus::Unresolved(error) => Some(error.to_string()),
        _ => None,
    }
}

fn runtime_state(runtimes: &GoRuntimesHandle, name: &str) -> Option<GoRuntimeState> {
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
    let processes: Vec<String> = rutis_bridge::transport::local::running_processes()
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
    root.get_as::<dyn HostDispatch>(host_key(name))
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

#[tokio::test(flavor = "multi_thread")]
async fn rows_find_their_binary_and_call_across_binaries() {
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
            state(&loader, "ping") == Some(FiberState::Active)
                && state(&loader, "caller") == Some(FiberState::Active)
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
        Some(GoRuntimeState::Running)
    );
    assert_eq!(
        runtime_state(&fixture.runtimes, "go-caller"),
        Some(GoRuntimeState::Running)
    );
    assert_eq!(
        runtime_state(&fixture.runtimes, "go-spare"),
        Some(GoRuntimeState::NotStarted),
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

#[tokio::test(flavor = "multi_thread")]
async fn a_plugin_in_two_binaries_is_named_with_its_runtime() {
    let fixture = fixture(None, &[("netkit", "netkit"), ("dup", "dup")]).await;
    fixture
        .loader
        .reconcile(layer(vec![row("p", "go:ping")]), None)
        .await
        .unwrap();
    let message = unresolved(&fixture.loader, "p").expect("go:ping is ambiguous");
    assert!(message.contains("several Go binaries"), "{message}");
    assert!(
        message.contains("go-dup") && message.contains("go-netkit"),
        "{message}"
    );

    fixture
        .loader
        .reconcile(layer(vec![row("p", "go-dup:ping")]), None)
        .await
        .unwrap();
    let loader = fixture.loader.clone();
    COLD.until(
        || state(&loader, "p") == Some(FiberState::Active),
        "the qualified row",
    )
    .await;
    assert_eq!(
        call(&fixture.root, "ping", "echo", json!(["x"]))
            .await
            .unwrap(),
        json!("dup pong x")
    );
    fixture.root.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_crash_stops_only_its_runtimes_rows_and_is_not_restarted() {
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
                state(&loader, id) == Some(FiberState::Active)
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
                Some(GoRuntimeState::Stopped(_))
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
            state(&loader, "ping") != Some(FiberState::Active)
                && state(&loader, "caller") != Some(FiberState::Active)
        },
        "its rows and their users stop",
    )
    .await;
    assert_eq!(
        state(&loader, "spare"),
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
        || runtime_state(&runtimes, "go-spare") == Some(GoRuntimeState::NotStarted),
        "the idle stop of spare",
    )
    .await;
    assert!(matches!(
        runtime_state(&fixture.runtimes, "go-netkit"),
        Some(GoRuntimeState::Stopped(_))
    ));
    // A restart does.
    fixture.runtimes.restart("go-netkit").await.unwrap();
    COLD.until(
        || {
            ["ping", "caller"].iter().all(|id| {
                state(&loader, id) == Some(FiberState::Active)
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
    fixture.root.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn an_idle_runtime_stops_and_starts_again_when_used() {
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
            state(&loader, "ping") == Some(FiberState::Active)
                && state(&loader, "spare") == Some(FiberState::Active)
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
        || runtime_state(&runtimes, "go-spare") == Some(GoRuntimeState::NotStarted),
        "the idle stop of spare",
    )
    .await;
    assert_eq!(
        runtime_state(&fixture.runtimes, "go-netkit"),
        Some(GoRuntimeState::Running)
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
        || runtime_state(&runtimes, "go-netkit") == Some(GoRuntimeState::NotStarted),
        "the idle stop",
    )
    .await;

    fixture
        .loader
        .reconcile(layer(vec![row("ping", "go:ping")]), None)
        .await
        .unwrap();
    COLD.until(
        || state(&loader, "ping") == Some(FiberState::Active),
        "the row again",
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

/// A binary replaced in place (renamed over: Windows refuses to replace a
/// running executable, so there a new build goes to a new file, below).
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn a_replaced_binary_takes_effect_when_its_runtime_restarts() {
    let fixture = fixture(None, &[("netkit", "netkit")]).await;
    fixture
        .loader
        .reconcile(layer(vec![row("ping", "go:ping")]), None)
        .await
        .unwrap();
    let loader = fixture.loader.clone();
    COLD.until(
        || state(&loader, "ping") == Some(FiberState::Active),
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
            state(&loader, "ping") == Some(FiberState::Active)
                && state(&loader, "extra") == Some(FiberState::Active)
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
#[tokio::test(flavor = "multi_thread")]
async fn a_new_build_takes_effect_when_its_runtime_restarts() {
    let fixture = fixture(None, &[("netkit", "netkit")]).await;
    fixture
        .loader
        .reconcile(layer(vec![row("ping", "go:ping")]), None)
        .await
        .unwrap();
    let loader = fixture.loader.clone();
    COLD.until(
        || state(&loader, "ping") == Some(FiberState::Active),
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
#[tokio::test(flavor = "multi_thread")]
async fn a_restart_given_up_half_way_leaves_the_runtime_usable() {
    let fixture = fixture(None, &[("netkit", "netkit")]).await;
    fixture
        .loader
        .reconcile(layer(vec![row("ping", "go:ping")]), None)
        .await
        .unwrap();
    let loader = fixture.loader.clone();
    COLD.until(
        || state(&loader, "ping") == Some(FiberState::Active),
        "the row",
    )
    .await;
    // clock: the short timeout is the scenario, not a wait: the caller gives
    // up half way through the restart, whatever the restart does meanwhile.
    let _ = tokio::time::timeout(
        Duration::from_millis(1),
        fixture.runtimes.restart("go-netkit"),
    )
    .await;
    COLD.until(
        || state(&loader, "ping") == Some(FiberState::Active),
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
        || state(&loader, "ping") == Some(FiberState::Active),
        "the row after the restart",
    )
    .await;
    assert_eq!(
        call(&fixture.root, "ping", "echo", json!(["still"]))
            .await
            .unwrap(),
        json!("netkit pong still")
    );
    fixture.root.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn names_of_other_resolvers_are_not_claimed() {
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
