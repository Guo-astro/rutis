//! Rows of a remote runtime: a Python runtime on another "machine" (a
//! process listening on a WebSocket, started by no one here), reached by a
//! link. The loader manages its rows as it does a local runtime's: the
//! rows inject a Rust service and provide one back, and they stop when the
//! runtime goes away. Needs a Python with `websockets`
//! (RUTIS_PYTHON, else python3; python on Windows).
#![cfg(feature = "python")]

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rutis::Ctx;
use rutis_bridge::channel::PeerId;
use rutis_bridge::runtime::RuntimeAccessPlugin;
use rutis_bridge::runtime::RuntimePlugin;
use rutis_bridge::session::{host_key, HostDispatch};
use rutis_bridge::session::{Reply, Value as RpcValue};
use rutis_bridge::transport::websocket::{Config, WebSocketPlugin};
use rutis_bridge::{Credential, IdentityPlugin, LinkConfig, LinkPlugin, StaticIdentity};
use rutis_loader::{
    Chain, Layer, LoaderOptions, LoaderPlugin, Patch, RuntimeResolver, RuntimeRowsPlugin,
};
use rutis_test_support::{Guard, HANG_GUARD};
use serde_json::{json, Value};
use tokio::io::AsyncBufReadExt;

/// Twice HANG_GUARD: the remote runtimes start cold (#238).
const COLD: Guard = Guard(Duration::from_secs(HANG_GUARD.as_secs() * 2));

const WEATHER: &str = r#"
inject = ["clock"]


class Weather:
    def __init__(self, clock):
        self.clock = clock

    def today(self):
        return f"remote at {self.clock.now()}"


provides = {"weather": Weather}


def apply(ctx, config):
    ctx.provide("weather", Weather(ctx.use("clock")))
"#;

#[derive(Clone, Default)]
struct Clock(Arc<Mutex<u64>>);

impl HostDispatch for Clock {
    fn invoke(&self, _method: &str, _args: RpcValue) -> Reply {
        let mut now = self.0.lock().unwrap();
        *now += 1;
        Ok(json!(*now).into())
    }

    fn methods(&self) -> Option<Value> {
        Some(json!({ "now": "sync" }))
    }
}

fn id(s: &str) -> PeerId {
    PeerId::new(s).unwrap()
}

/// Start the remote runtime: `gpu`, accepting `main` with a token.
async fn remote_python(project: &Path) -> (tokio::process::Child, String) {
    let python = std::env::var("RUTIS_PYTHON")
        .unwrap_or_else(|_| if cfg!(windows) { "python" } else { "python3" }.into());
    let sdk = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../python/rutis");
    let mut child = tokio::process::Command::new(python)
        .args(["-m", "rutis", "listen:ws://127.0.0.1:0/rutis"])
        .args(["--id", "gpu", "--peer", "main"])
        .arg(project)
        .env("PYTHONPATH", sdk)
        .env("RUTIS_TOKEN", "main-token")
        .current_dir(project)
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut lines = tokio::io::BufReader::new(child.stderr.take().unwrap()).lines();
    let address = COLD
        .within("the runtime's address", async {
            loop {
                let line = lines
                    .next_line()
                    .await
                    .unwrap()
                    .expect("the runtime's address");
                if let Some(address) = line.strip_prefix("rutis: listening on ") {
                    break address.to_owned();
                }
            }
        })
        .await;
    // What the runtime reports later goes to the test's output.
    tokio::spawn(async move {
        while let Ok(Some(line)) = lines.next_line().await {
            eprintln!("runtime: {line}");
        }
    });
    (child, address)
}

/// guarantee: loader/remote-runtime-rows
#[tokio::test(flavor = "multi_thread")]
async fn a_remote_python_runtime_runs_loader_rows() {
    let project = tempfile::tempdir().unwrap();
    std::fs::write(project.path().join("weather_plugin.py"), WEATHER).unwrap();
    let (mut runtime_process, address) = remote_python(project.path()).await;

    let root = Ctx::root().unwrap();
    let clock = Clock::default();
    root.provide_as::<dyn HostDispatch>(host_key("clock"), Arc::new(clock.clone()))
        .unwrap();
    (&root.plugin(WebSocketPlugin::new(Config::new()).unwrap()))
        .await
        .unwrap();
    root.plugin(IdentityPlugin::new(
        "main",
        StaticIdentity::new(id("main")).present(id("gpu"), Credential::Bearer("main-token".into())),
    ));
    let link = LinkPlugin::new(
        LinkConfig::dial(id("gpu"), "websocket", "main", &address).require("runtime"),
    );
    let link_state = link.state();
    root.plugin(link);
    root.plugin(RuntimeAccessPlugin::new(id("gpu"), "py"));
    let python = RuntimePlugin::remote("py");
    let rows = Arc::new(RuntimeResolver::modules(python.handle()));
    let handle = python.handle();
    root.plugin(python);
    let plugin_loader = LoaderPlugin::new(
        Chain::new().with_shared(rows.clone()),
        LoaderOptions::default(),
    );
    let loader = plugin_loader.handle();
    root.plugin(plugin_loader).await.unwrap();
    root.plugin(RuntimeRowsPlugin::new(rows));

    let patches: Vec<Patch> = serde_json::from_value(json!([{
        "insert": [{ "id": "w", "name": "py:weather_plugin", "config": {} }]
    }]))
    .unwrap();
    loader
        .reconcile(vec![Layer::new("rows", patches)], None)
        .await
        .unwrap();

    // The row runs over there, and its service is here.
    let weather = COLD
        .eventually(
            || root.get_as::<dyn HostDispatch>(host_key("weather")),
            "the remote row's service",
        )
        .await;
    assert!(handle.is_remote() && handle.supports("leaf"));
    let today = weather.invoke("today", json!([]).into()).unwrap();
    assert_eq!(today.json().unwrap(), json!("remote at 1"));
    drop(weather);

    // The runtime goes away: its rows stop and their services go.
    runtime_process.start_kill().unwrap();
    COLD.eventually(
        || {
            root.get_as::<dyn HostDispatch>(host_key("weather"))
                .is_none()
                .then_some(())
        },
        "the withdrawal",
    )
    .await;
    COLD.eventually(
        || {
            matches!(
                *link_state.borrow(),
                rutis_bridge::LinkState::Waiting { .. }
            )
            .then_some(())
        },
        "the link retrying",
    )
    .await;
    root.shutdown().await.unwrap();
}

/// The same with a remote Node runtime: the row names an npm package that
/// exists only where the runtime runs.
/// guarantee: loader/remote-runtime-rows
#[cfg(feature = "node")]
#[tokio::test(flavor = "multi_thread")]
async fn a_remote_node_runtime_resolves_and_runs_npm_rows() {
    let project = tempfile::tempdir().unwrap();
    let anchor = project.path().join("package.json");
    std::fs::write(&anchor, "{}").unwrap();
    let package = project.path().join("node_modules/remote-weather");
    std::fs::create_dir_all(&package).unwrap();
    std::fs::write(
        package.join("package.json"),
        json!({
            "name": "remote-weather", "version": "1.0.0", "type": "module", "main": "index.mjs",
            "rutis": { "provides": { "weather": { "today": "sync" } } }
        })
        .to_string(),
    )
    .unwrap();
    std::fs::write(
        package.join("index.mjs"),
        "export const inject = ['clock']\nexport function apply(ctx) {\n  ctx.provide('weather', { today() { return `node at ${ctx.clock.now()}` } })\n}\n",
    )
    .unwrap();
    let runtime = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../node/rutis-runtime");
    let mut child = tokio::process::Command::new("node")
        .args(["--import", "tsx"])
        .arg(runtime.join("src/runner.mjs"))
        .arg("listen:ws://127.0.0.1:0/rutis")
        .args(["--id", "edge", "--peer", "main"])
        .arg(&anchor)
        .env("RUTIS_TOKEN", "main-token")
        .current_dir(&runtime)
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut lines = tokio::io::BufReader::new(child.stderr.take().unwrap()).lines();
    let address = COLD
        .within("the runtime's address", async {
            loop {
                let line = lines
                    .next_line()
                    .await
                    .unwrap()
                    .expect("the runtime's address");
                if let Some(address) = line.strip_prefix("rutis: listening on ") {
                    break address.to_owned();
                }
            }
        })
        .await;
    // What the runtime reports later goes to the test's output.
    tokio::spawn(async move {
        while let Ok(Some(line)) = lines.next_line().await {
            eprintln!("runtime: {line}");
        }
    });

    let root = Ctx::root().unwrap();
    root.provide_as::<dyn HostDispatch>(host_key("clock"), Arc::new(Clock::default()))
        .unwrap();
    (&root.plugin(WebSocketPlugin::new(Config::new()).unwrap()))
        .await
        .unwrap();
    root.plugin(IdentityPlugin::new(
        "main",
        StaticIdentity::new(id("main"))
            .present(id("edge"), Credential::Bearer("main-token".into())),
    ));
    root.plugin(LinkPlugin::new(
        LinkConfig::dial(id("edge"), "websocket", "main", &address).require("runtime"),
    ));
    root.plugin(RuntimeAccessPlugin::new(id("edge"), "node"));
    let node = RuntimePlugin::remote("node");
    let mut catalog = rutis_loader::ServiceCatalog::new();
    catalog.register_shared("clock");
    let rows = Arc::new(RuntimeResolver::node(node.handle()).with_catalog(&catalog));
    root.plugin(node);
    let options = LoaderOptions {
        catalog,
        ..LoaderOptions::default()
    };
    let plugin_loader = LoaderPlugin::new(Chain::new().with_shared(rows.clone()), options);
    let loader = plugin_loader.handle();
    root.plugin(plugin_loader).await.unwrap();
    root.plugin(RuntimeRowsPlugin::new(rows));

    let patches: Vec<Patch> = serde_json::from_value(json!([{
        "insert": [
            { "id": "w", "name": "remote-weather", "config": {} },
            { "id": "x", "name": "not-installed-there", "config": {} }
        ]
    }]))
    .unwrap();
    loader
        .reconcile(vec![Layer::new("rows", patches)], None)
        .await
        .unwrap();
    let weather = COLD
        .eventually(
            || root.get_as::<dyn HostDispatch>(host_key("weather")),
            "the remote npm row's service",
        )
        .await;
    assert_eq!(
        weather
            .invoke("today", json!([]).into())
            .unwrap()
            .json()
            .unwrap(),
        json!("node at 1")
    );
    // A package the runtime does not have is unresolved, not a failure.
    let missing = COLD
        .eventually(
            || {
                let entry = loader.get("x")?;
                match entry.status {
                    rutis_loader::EntryStatus::Unresolved(error) => Some(error),
                    _ => None,
                }
            },
            "the missing package unresolved",
        )
        .await;
    assert!(
        matches!(missing, rutis_loader::LoaderError::NotFound { .. }),
        "{missing:?}"
    );
    drop(weather);
    child.start_kill().unwrap();
    root.shutdown().await.unwrap();
}

/// The rows of a remote runtime share its one control session: both
/// services come from the same session, and unloading one row leaves the
/// session and the other row's service.
/// guarantee: loader/remote-runtime-rows
#[tokio::test(flavor = "multi_thread")]
async fn remote_rows_share_the_runtime_session() {
    let project = tempfile::tempdir().unwrap();
    std::fs::write(project.path().join("weather_plugin.py"), WEATHER).unwrap();
    std::fs::write(
        project.path().join("second_plugin.py"),
        "class Second:\n    def ping(self):\n        return 'pong'\n\n\nprovides = {\"second\": Second}\n\n\ndef apply(ctx, config):\n    ctx.provide(\"second\", Second())\n",
    )
    .unwrap();
    let (_runtime_process, address) = remote_python(project.path()).await;

    let root = Ctx::root().unwrap();
    root.provide_as::<dyn HostDispatch>(host_key("clock"), Arc::new(Clock::default()))
        .unwrap();
    (&root.plugin(WebSocketPlugin::new(Config::new()).unwrap()))
        .await
        .unwrap();
    root.plugin(IdentityPlugin::new(
        "main",
        StaticIdentity::new(id("main")).present(id("gpu"), Credential::Bearer("main-token".into())),
    ));
    root.plugin(LinkPlugin::new(
        LinkConfig::dial(id("gpu"), "websocket", "main", &address).require("runtime"),
    ));
    root.plugin(RuntimeAccessPlugin::new(id("gpu"), "py"));
    let python = RuntimePlugin::remote("py");
    let rows = Arc::new(RuntimeResolver::modules(python.handle()));
    root.plugin(python);
    let plugin_loader = LoaderPlugin::new(
        Chain::new().with_shared(rows.clone()),
        LoaderOptions::default(),
    );
    let loader = plugin_loader.handle();
    root.plugin(plugin_loader).await.unwrap();
    root.plugin(RuntimeRowsPlugin::new(rows));
    let layer = |rows: Value| -> Vec<Layer> {
        let patches: Vec<Patch> = serde_json::from_value(json!([{ "insert": rows }])).unwrap();
        vec![Layer::new("rows", patches)]
    };
    loader
        .reconcile(
            layer(json!([
                { "id": "w", "name": "py:weather_plugin", "config": {} },
                { "id": "s", "name": "py:second_plugin", "config": {} }
            ])),
            None,
        )
        .await
        .unwrap();
    let weather = COLD
        .eventually(
            || root.get_as::<dyn HostDispatch>(host_key("weather")),
            "weather",
        )
        .await;
    let second = COLD
        .eventually(
            || root.get_as::<dyn HostDispatch>(host_key("second")),
            "second",
        )
        .await;
    assert_eq!(
        weather.origin(),
        second.origin(),
        "both rows run on one session"
    );
    let session = weather.origin().unwrap().to_owned();
    drop(weather);

    loader
        .reconcile(
            layer(json!([{ "id": "s", "name": "py:second_plugin", "config": {} }])),
            None,
        )
        .await
        .unwrap();
    COLD.eventually(
        || {
            root.get_as::<dyn HostDispatch>(host_key("weather"))
                .is_none()
                .then_some(())
        },
        "weather unloaded",
    )
    .await;
    assert_eq!(second.origin(), Some(session.as_str()), "the session stays");
    let pong = tokio::task::spawn_blocking(move || second.invoke("ping", json!([]).into()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(pong.json().unwrap(), json!("pong"));
    root.shutdown().await.unwrap();
}

/// A WebSocket client to a listener at `address`, presenting `token` and
/// speaking the endpoint protocol.
async fn connect(
    address: &str,
    token: &str,
) -> Result<
    tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>,
    tokio_tungstenite::tungstenite::Error,
> {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    let url = url::Url::parse(address).unwrap();
    let tcp = tokio::net::TcpStream::connect((
        url.host_str().unwrap(),
        url.port_or_known_default().unwrap(),
    ))
    .await
    .map_err(tokio_tungstenite::tungstenite::Error::Io)?;
    let mut request = address.into_client_request().unwrap();
    let headers = request.headers_mut();
    headers.insert(
        "Sec-WebSocket-Protocol",
        rutis_bridge::protocol().parse().unwrap(),
    );
    headers.insert("Authorization", format!("Bearer {token}").parse().unwrap());
    Ok(tokio_tungstenite::client_async(request, tcp).await?.0)
}

/// A remote runtime that dials rutis, stood in for until a runtime can
/// dial (#383): both ends listen, and a relay dials each and passes the
/// frames between them. The handshake is end to end — the runtime greets as
/// `gpu`, rutis checks that against the link — so what is tested is rutis's
/// listening side of a runtime link (dial-out design R-P0).
/// guarantee: loader/remote-runtime-rows
#[tokio::test(flavor = "multi_thread")]
async fn a_runtime_link_can_listen() {
    use futures_util::{SinkExt, StreamExt};
    use rutis_bridge::transport::websocket::ListenerConfig;

    let project = tempfile::tempdir().unwrap();
    std::fs::write(project.path().join("weather_plugin.py"), WEATHER).unwrap();
    let (_runtime_process, runtime_address) = remote_python(project.path()).await;

    let root = Ctx::root().unwrap();
    let clock = Clock::default();
    root.provide_as::<dyn HostDispatch>(host_key("clock"), Arc::new(clock.clone()))
        .unwrap();
    let websocket = WebSocketPlugin::new(Config::new().listener(ListenerConfig::new(
        "public",
        "127.0.0.1:0".parse().unwrap(),
        id("main"),
    )))
    .unwrap();
    let transport = websocket.clone();
    (&root.plugin(websocket)).await.unwrap();
    let address = format!(
        "ws://{}/rutis",
        transport.transport().unwrap().local_addr("public").unwrap()
    );
    root.plugin(IdentityPlugin::new(
        "main",
        StaticIdentity::new(id("main")).accept_token("gpu-token", id("gpu")),
    ));
    root.plugin(LinkPlugin::new(
        LinkConfig::listen(id("gpu"), "websocket", "main", "public").require("runtime"),
    ));
    root.plugin(RuntimeAccessPlugin::new(id("gpu"), "py"));
    let python = RuntimePlugin::remote("py");
    let rows = Arc::new(RuntimeResolver::modules(python.handle()));
    let handle = python.handle();
    root.plugin(python);
    let plugin_loader = LoaderPlugin::new(
        Chain::new().with_shared(rows.clone()),
        LoaderOptions::default(),
    );
    let loader = plugin_loader.handle();
    root.plugin(plugin_loader).await.unwrap();
    root.plugin(RuntimeRowsPlugin::new(rows));
    let patches: Vec<Patch> = serde_json::from_value(json!([{
        "insert": [{ "id": "w", "name": "py:weather_plugin", "config": {} }]
    }]))
    .unwrap();
    loader
        .reconcile(vec![Layer::new("rows", patches)], None)
        .await
        .unwrap();

    // The relay: rutis's side is retried until its link listens.
    let runtime = connect(&runtime_address, "main-token").await.unwrap();
    let rutis = COLD
        .within("rutis's listener", async {
            loop {
                match connect(&address, "gpu-token").await {
                    Ok(socket) => break socket,
                    Err(_) => tokio::time::sleep(Duration::from_millis(50)).await,
                }
            }
        })
        .await;
    let (mut to_runtime, mut from_runtime) = runtime.split();
    let (mut to_rutis, mut from_rutis) = rutis.split();
    let relay = tokio::spawn(async move {
        let up = async {
            while let Some(Ok(message)) = from_runtime.next().await {
                if to_rutis.send(message).await.is_err() {
                    break;
                }
            }
        };
        let down = async {
            while let Some(Ok(message)) = from_rutis.next().await {
                if to_runtime.send(message).await.is_err() {
                    break;
                }
            }
        };
        tokio::select! { _ = up => {}, _ = down => {} }
    });

    // The runtime that came in runs the row, as one rutis dialed would.
    let weather = COLD
        .eventually(
            || root.get_as::<dyn HostDispatch>(host_key("weather")),
            "the remote row's service",
        )
        .await;
    assert!(handle.is_remote() && handle.supports("leaf"));
    let today = weather.invoke("today", json!([]).into()).unwrap();
    assert_eq!(today.json().unwrap(), json!("remote at 1"));
    drop(weather);

    // The connection goes: the row stops and its service goes.
    relay.abort();
    COLD.eventually(
        || {
            root.get_as::<dyn HostDispatch>(host_key("weather"))
                .is_none()
                .then_some(())
        },
        "the withdrawal",
    )
    .await;
    root.shutdown().await.unwrap();
}
