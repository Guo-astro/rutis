//! The Bun runtime speaks the same protocol and row contract as the other
//! runtimes: describe, load with exports, lease hosts, sync and async calls,
//! callbacks into Rust from a synchronous call, unload. And what is its own:
//! it runs incoming calls while a synchronous call waits, plugins resolve
//! only what the project installed, the project's `.env` is not read, an
//! uncaught error ends the process, an edited plugin is imported again, and
//! a Bun older than the package supports is refused.
// Unix only (Q7.8): Bun on Windows is not a declared platform of the Bun
// runtime.
#![cfg(all(unix, feature = "bun"))]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rutis::Ctx;
use rutis_bridge::runtime::{row_projection, Launcher, Mount, Process};
use rutis_bridge::session::{host_key, HostDispatch};
use rutis_bridge::session::{settle, Reply, Value as RpcValue};
use rutis_test_support::{recv_within, until, within, HANG_GUARD};
use serde_json::{json, Value};

const WEATHER: &str = r#"
export const inject = ['clock']
export const config = { type: 'object', properties: { city: { type: 'string', default: 'Paris' } } }
export const provides = { weather: { today: 'sync', later: 'async', each: 'sync' } }
export function apply(ctx, config) {
  const clock = ctx.use('clock')
  const city = config.city ?? 'Paris'
  ctx.provide('weather', {
    today: () => `${city} at ${clock.now()}`,
    later: async () => { await Bun.sleep(10); return `${city} later` },
    // A Rust callback, called back during this synchronous call.
    each: callback => ['mon', 'tue'].map(day => callback(day)),
  })
  return () => console.log('weather: bye')
}
"#;

struct Clock(Arc<AtomicUsize>);

impl HostDispatch for Clock {
    fn invoke(&self, method: &str, _args: RpcValue) -> Reply {
        assert_eq!(method, "now");
        Ok(json!(self.0.fetch_add(1, Ordering::SeqCst)).into())
    }

    fn methods(&self) -> Option<Value> {
        Some(json!({ "now": "sync" }))
    }
}

fn package() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../bun/rutis-bun")
}

async fn mount(launcher: Launcher, project: &Path) -> Result<Arc<Process>, String> {
    Process::mount(
        &package(),
        Mount {
            anchor: Some(project),
            launcher: Some(&launcher),
            ..Mount::default()
        },
    )
    .await
    .map_err(|error| error.to_string())
}

async fn bun(project: &Path) -> Arc<Process> {
    // A registry nothing listens on: a download attempt fails at once
    // instead of fetching (the runtime runs with --no-install).
    let launcher = Launcher::bun(None, &package(), project)
        .env("BUN_CONFIG_REGISTRY", "http://127.0.0.1:9/")
        .env("NPM_CONFIG_REGISTRY", "http://127.0.0.1:9/");
    mount(launcher, project).await.unwrap()
}

/// Load `entry` as row `key` exporting what it provides, and wait for its
/// first service to reach `ctx`.
async fn load(
    process: &Arc<Process>,
    ctx: &Ctx,
    key: &str,
    entry: &Path,
    config: Value,
) -> Arc<rutis_bridge::runtime::Projection> {
    let described = process.describe_row(entry).await.unwrap();
    let projection = row_projection(&described.provides);
    projection.attach(ctx, process.clone()).unwrap();
    process
        .load_row_exporting(
            key,
            entry,
            config,
            &[],
            &[],
            &described.provides,
            projection.clone(),
        )
        .await
        .unwrap();
    let name = described.provides.keys().next().unwrap().clone();
    let service = host_key(&name);
    until(
        || ctx.get_as::<dyn HostDispatch>(service.clone()).is_some(),
        &name,
    )
    .await;
    projection
}

#[tokio::test(flavor = "multi_thread")]
async fn bun_rows_follow_the_row_contract() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("weather.ts"), WEATHER).unwrap();
    let process = bun(dir.path()).await;
    for feature in ["rows.v2", "hosts", "leaf", "scopes"] {
        assert!(process.supports(feature), "{feature}");
    }
    assert_eq!(process.about()["engine"]["name"], json!("bun"));
    assert_eq!(
        process.about()["implementation"]["name"],
        json!("@arcships/rutis-bun")
    );

    let entry = Path::new("./weather.ts");
    let described = process.describe_row(entry).await.unwrap();
    assert_eq!(described.inject, ["clock"]);
    assert_eq!(
        Value::Object(described.provides.clone()),
        json!({ "weather": { "today": "sync", "later": "async", "each": "sync" } })
    );
    assert_eq!(
        described.config.unwrap()["properties"]["city"]["default"],
        json!("Paris")
    );
    assert_eq!(
        described.version, None,
        "a file of the project has no version"
    );

    let lease = process
        .lease_host("clock", Arc::new(Clock(Arc::default())), None)
        .await
        .unwrap();
    let ctx = Ctx::root().unwrap();
    let projection = load(&process, &ctx, "w", entry, json!({ "city": "Oslo" })).await;
    let key = host_key("weather");
    let weather = ctx.get_as::<dyn HostDispatch>(key.clone()).unwrap();
    assert_eq!(
        weather
            .invoke("today", json!([]).into())
            .unwrap()
            .json()
            .unwrap(),
        json!("Oslo at 0")
    );
    let later = settle(weather.invoke("later", json!([]).into()).unwrap())
        .await
        .unwrap();
    assert_eq!(later.json().unwrap(), json!("Oslo later"));
    let callback = RpcValue::callback(|args| {
        let [day]: [String; 1] = rutis_bridge::session::decode_value(args)?;
        Ok(json!(day.to_uppercase()).into())
    });
    let days = weather
        .invoke("each", RpcValue::List(vec![callback]))
        .unwrap();
    assert_eq!(days.json().unwrap(), json!(["MON", "TUE"]));
    drop(weather);

    process.unload_row("w").await.unwrap();
    until(
        || ctx.get_as::<dyn HostDispatch>(key.clone()).is_none(),
        "the withdrawal",
    )
    .await;
    projection.close();
    lease.release().await.unwrap();
    process.dispose().await.unwrap();
    ctx.shutdown().await.unwrap();
}

/// An installed package reports its version; a name the project has not
/// installed is not found, and is never fetched.
#[tokio::test(flavor = "multi_thread")]
async fn packages_resolve_from_the_project_only() {
    let dir = tempfile::tempdir().unwrap();
    let package = dir.path().join("node_modules/@acme/weather");
    std::fs::create_dir_all(&package).unwrap();
    std::fs::write(
        package.join("package.json"),
        r#"{ "name": "@acme/weather", "version": "1.2.3", "type": "module", "main": "index.ts" }"#,
    )
    .unwrap();
    std::fs::write(package.join("index.ts"), WEATHER).unwrap();
    std::fs::write(dir.path().join("package.json"), "{}").unwrap();
    // A plugin of the project that imports a package nobody installed.
    std::fs::write(
        dir.path().join("needs.ts"),
        "import pad from 'left-pad'\nexport function apply() { pad('x', 2) }\n",
    )
    .unwrap();
    let process = bun(dir.path()).await;

    let described = process
        .describe_row(Path::new("@acme/weather"))
        .await
        .unwrap();
    assert_eq!(described.version.as_deref(), Some("1.2.3"));

    match process.describe_row(Path::new("@acme/missing")).await {
        Err(rutis_bridge::session::Error::Remote { name, .. }) => assert_eq!(name, "NotFound"),
        other => panic!("an unknown package should be NotFound, got {other:?}"),
    }
    // Nothing is downloaded: the registry is unreachable (bun()), and
    // nothing lands in the project.
    let error = process
        .describe_row(Path::new("./needs.ts"))
        .await
        .expect_err("an uninstalled import fails");
    assert!(error.to_string().contains("left-pad"), "{error}");
    assert!(!dir.path().join("node_modules/left-pad").exists());
    process.dispose().await.unwrap();
}

/// The environment is what the host gives: the project's `.env` is not read.
#[tokio::test(flavor = "multi_thread")]
async fn the_projects_env_file_is_not_read() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(".env"), "RUTIS_PROBE=from-dotenv\n").unwrap();
    std::fs::write(
        dir.path().join("probe.ts"),
        r#"
export const provides = { probe: { value: 'sync' } }
export function apply(ctx) { ctx.provide('probe', { value: () => process.env.RUTIS_PROBE ?? null }) }
"#,
    )
    .unwrap();
    let process = bun(dir.path()).await;
    let ctx = Ctx::root().unwrap();
    let projection = load(&process, &ctx, "p", Path::new("./probe.ts"), json!({})).await;
    let probe = ctx.get_as::<dyn HostDispatch>(host_key("probe")).unwrap();
    assert_eq!(
        probe
            .invoke("value", json!([]).into())
            .unwrap()
            .json()
            .unwrap(),
        Value::Null
    );
    drop(probe);
    projection.close();
    process.dispose().await.unwrap();
    ctx.shutdown().await.unwrap();
}

/// While a synchronous call of the runtime waits, an unrelated call into it
/// runs: here the first call waits until the second one has run, which
/// without that would never happen.
#[tokio::test(flavor = "multi_thread")]
async fn incoming_calls_run_while_a_synchronous_call_waits() {
    /// Waits until opened; says when a call has entered it.
    struct Gate(
        Mutex<std::sync::mpsc::Receiver<()>>,
        std::sync::mpsc::Sender<()>,
    );
    impl HostDispatch for Gate {
        fn invoke(&self, _method: &str, _args: RpcValue) -> Reply {
            let _ = self.1.send(());
            self.0
                .lock()
                .unwrap()
                .recv_timeout(HANG_GUARD)
                .map_err(|_| rutis_bridge::session::Error::Value("never opened".into()))?;
            Ok(json!("opened").into())
        }
        fn methods(&self) -> Option<Value> {
            Some(json!({ "wait": "sync" }))
        }
    }
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("gate.ts"),
        r#"
export const inject = ['gate']
export const provides = { door: { pass: 'sync', knock: 'sync' } }
export function apply(ctx) {
  const gate = ctx.use('gate')
  ctx.provide('door', { pass: () => gate.wait(), knock: () => 'knocked' })
}
"#,
    )
    .unwrap();
    let process = bun(dir.path()).await;
    let (open, opened) = std::sync::mpsc::channel();
    let (enter, entered) = std::sync::mpsc::channel();
    let lease = process
        .lease_host("gate", Arc::new(Gate(Mutex::new(opened), enter)), None)
        .await
        .unwrap();
    let ctx = Ctx::root().unwrap();
    let projection = load(&process, &ctx, "d", Path::new("./gate.ts"), json!({})).await;
    let door = ctx.get_as::<dyn HostDispatch>(host_key("door")).unwrap();

    let passing = door.clone();
    let pass = tokio::task::spawn_blocking(move || passing.invoke("pass", json!([]).into()));
    // `pass` waits in Bun for the gate; `knock` must still be answered.
    let entered = tokio::task::spawn_blocking(move || recv_within("pass at the gate", &entered));
    entered.await.unwrap();
    let knocking = door.clone();
    let knock = within(
        "an unrelated call while a synchronous call waits",
        tokio::task::spawn_blocking(move || knocking.invoke("knock", json!([]).into())),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(knock.json().unwrap(), json!("knocked"));
    open.send(()).unwrap();
    assert_eq!(
        pass.await.unwrap().unwrap().json().unwrap(),
        json!("opened")
    );
    drop(door);
    projection.close();
    lease.release().await.unwrap();
    process.dispose().await.unwrap();
    ctx.shutdown().await.unwrap();
}

/// An uncaught error in a plugin ends the process, and with it every
/// service of the runtime.
#[tokio::test(flavor = "multi_thread")]
async fn an_uncaught_error_ends_the_runtime() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("fragile.ts"),
        r#"
export const provides = { fragile: { later: 'sync' } }
export function apply(ctx) {
  ctx.provide('fragile', { later: () => { setTimeout(() => { throw new Error('boom') }, 10); return 'scheduled' } })
}
"#,
    )
    .unwrap();
    let process = bun(dir.path()).await;
    let ctx = Ctx::root().unwrap();
    let projection = load(&process, &ctx, "f", Path::new("./fragile.ts"), json!({})).await;
    let fragile = ctx.get_as::<dyn HostDispatch>(host_key("fragile")).unwrap();
    assert_eq!(
        fragile
            .invoke("later", json!([]).into())
            .unwrap()
            .json()
            .unwrap(),
        json!("scheduled")
    );
    drop(fragile);
    tokio::time::timeout(Duration::from_secs(10), process.closed())
        .await
        .expect("the process ends");
    assert!(
        process
            .exit_status()
            .is_some_and(|status| !status.contains("status: 0")),
        "{:?}",
        process.exit_status()
    );
    projection.close();
    ctx.shutdown().await.unwrap();
}

/// A row loaded again after its plugin file changed runs the new code.
#[tokio::test(flavor = "multi_thread")]
async fn an_edited_plugin_is_imported_again() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("greeter.ts");
    let greeter = |word: &str| {
        format!(
            "export const provides = {{ greeter: {{ greet: 'sync' }} }}\n\
             export function apply(ctx) {{ ctx.provide('greeter', {{ greet: () => '{word}' }}) }}\n"
        )
    };
    std::fs::write(&file, greeter("Hello")).unwrap();
    let process = bun(dir.path()).await;
    let ctx = Ctx::root().unwrap();
    let entry = Path::new("./greeter.ts");
    let greet = |ctx: &Ctx| {
        ctx.get_as::<dyn HostDispatch>(host_key("greeter"))
            .unwrap()
            .invoke("greet", json!([]).into())
            .unwrap()
            .json()
            .unwrap()
    };
    let projection = load(&process, &ctx, "g", entry, json!({})).await;
    assert_eq!(greet(&ctx), json!("Hello"));
    process.unload_row("g").await.unwrap();
    projection.close();
    until(
        || {
            ctx.get_as::<dyn HostDispatch>(host_key("greeter"))
                .is_none()
        },
        "the withdrawal",
    )
    .await;

    // A different size, so the change shows even within one mtime tick.
    std::fs::write(&file, greeter("Welcome")).unwrap();
    let projection = load(&process, &ctx, "g", entry, json!({})).await;
    assert_eq!(greet(&ctx), json!("Welcome"));
    projection.close();
    process.dispose().await.unwrap();
    ctx.shutdown().await.unwrap();
}

/// A Bun older than the package supports is refused before the session
/// starts, with a message saying what it needs.
#[tokio::test(flavor = "multi_thread")]
async fn a_bun_too_old_for_the_package_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let copy = dir.path().join("rutis-bun");
    std::fs::create_dir_all(&copy).unwrap();
    let mut manifest: Value =
        serde_json::from_str(&std::fs::read_to_string(package().join("package.json")).unwrap())
            .unwrap();
    manifest["engines"]["bun"] = json!(">=99.0");
    std::fs::write(copy.join("package.json"), manifest.to_string()).unwrap();
    // A copy, not a link: the runtime reads the manifest next to its code.
    std::fs::create_dir_all(copy.join("src")).unwrap();
    for file in std::fs::read_dir(package().join("src")).unwrap() {
        let file = file.unwrap().path();
        std::fs::copy(&file, copy.join("src").join(file.file_name().unwrap())).unwrap();
    }
    // The runtime imports its dependencies (`@deepseek-ai/cordis`), which Bun
    // resolves before the version check runs: the copy needs them, as an
    // installed package has them (`bun install`; see docs/ci.md).
    let modules = package().join("node_modules");
    assert!(
        modules.is_dir(),
        "{}: run `bun install` in bun/rutis-bun first",
        modules.display()
    );
    std::os::unix::fs::symlink(&modules, copy.join("node_modules")).unwrap();
    let Err(error) = mount(Launcher::bun(None, &copy, dir.path()), dir.path()).await else {
        panic!("an old Bun is refused");
    };
    // It says why on stderr, and exits before greeting.
    assert!(error.contains("exit status: 2"), "{error}");
}

/// The Bun runtime started from one of its test fixtures, in `project`.
fn fixture_launcher(fixture: &str, project: &Path) -> Launcher {
    Launcher::new("bun")
        .arg("--no-install")
        .arg("--no-env-file")
        .arg(package().join("test/fixtures").join(fixture))
        .cwd(project)
        .inherit_fd()
}

/// A runtime that does not report what rows need fails when it starts, with
/// one clear error, instead of every row failing later.
#[tokio::test(flavor = "multi_thread")]
async fn a_runtime_without_the_row_contract_fails_to_start() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = rutis_bridge::runtime::LocalRuntime::launcher(
        "old",
        fixture_launcher("old-runtime.ts", dir.path()),
        dir.path(),
    );
    let handle = runtime.handle();
    let ctx = Ctx::root().unwrap();
    let view = ctx.plugin(runtime);
    let _ = (&view).await;
    assert_eq!(view.state().state, rutis::FiberState::Failed);
    match handle.state() {
        rutis_bridge::runtime::RuntimeState::Down(message) => {
            assert!(message.contains("lacks rows.v2 and hosts"), "{message}")
        }
        _ => panic!("the runtime should be down"),
    }
    ctx.shutdown().await.unwrap();
}

/// Unloading a row withdraws its services even when the runtime's
/// withdrawal reaches the session late. Here it never comes: the runtime is
/// made to drop it.
#[tokio::test(flavor = "multi_thread")]
async fn unloading_a_row_withdraws_its_services_without_waiting_for_the_runtime() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("weather.ts"), WEATHER).unwrap();
    let process = mount(
        fixture_launcher("dropping-runtime.ts", dir.path()),
        dir.path(),
    )
    .await
    .unwrap();
    let lease = process
        .lease_host("clock", Arc::new(Clock(Arc::default())), None)
        .await
        .unwrap();
    let ctx = Ctx::root().unwrap();
    let projection = load(&process, &ctx, "w", Path::new("./weather.ts"), json!({})).await;
    process.unload_row("w").await.unwrap();
    until(
        || {
            ctx.get_as::<dyn HostDispatch>(host_key("weather"))
                .is_none()
        },
        "the withdrawal",
    )
    .await;
    projection.close();
    lease.release().await.unwrap();
    process.dispose().await.unwrap();
    ctx.shutdown().await.unwrap();
}
