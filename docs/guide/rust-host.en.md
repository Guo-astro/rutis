# Embed rutis in a Rust Application

Use the crates directly when your application needs to provide its own Rust services to plugins or when you want to embed rutis in an existing Rust program. If you do not need this, use [rutis-host](rutis-host.en.md); both use the same row configuration format.

## Dependencies

```toml
[dependencies]
rutis = "0.11"
rutis-loader = { version = "0.11", features = ["node", "python", "peer"] }
rutis-bridge = { version = "0.11", features = ["python", "websocket"] }   # node is enabled by default
```

| Crate / feature | Contents |
| --- | --- |
| `rutis` | Core: plugins, dependencies, and services. |
| `rutis-loader` | Loads plugins from data rows. `node` / `python` / `go` enable rows for those languages; `peer` enables node rows and `peer:` rows. |
| `rutis-bridge` | Connects processes, languages, and machines. `node` (default), `python` and `go` enable local runtimes; `websocket` enables the WebSocket transport; `cordis` mounts Cordis plugins and generates Rust bindings (see [Cordis](cordis.en.md)); `testing` enables compatibility tests. |

Enable only the language features you use so the application compiles and starts only the required components.

## Provide Rust services to plugins

In Rust, a service used by name from a plugin is a `dyn HostDispatch` registered under `host_key(name)`:

```rust
use std::sync::Arc;
use rutis_bridge::session::{host_key, HostDispatch, Reply, Value};
use serde_json::json;

struct Clock;

impl HostDispatch for Clock {
    fn invoke(&self, method: &str, _args: Value) -> Reply {
        match method {
            "now" => Ok(json!(42).into()),
            other => Err(rutis_bridge::session::Error::Value(format!("no method {other}"))),
        }
    }
    // The plugin uses this method shape to decide whether to call synchronously or asynchronously.
    fn methods(&self) -> Option<serde_json::Value> {
        Some(json!({ "now": "sync" }))
    }
}

root.provide_as::<dyn HostDispatch>(host_key("clock"), Arc::new(Clock))?;
```

Services provided by plugins also appear in rutis under `host_key(name)`, so Rust plugins can use them by name.

### Which row calls

A service that registers things for plugins (tools, routes, listeners) or grants things per plugin needs to know which row calls it. During `invoke`, `session::caller_row()` returns the calling row; tie a registration to that row's `Ctx`, and it goes when the row unloads or restarts, while the runtime keeps running:

```rust
use rutis::Effect;
use rutis_bridge::session::{caller_row, native_error, HostDispatch, Reply, Value};

impl HostDispatch for Tools {
    fn invoke(&self, method: &str, args: Value) -> Reply {
        let [name]: [String; 1] = rutis_bridge::session::decode_value(args)?;
        if let Some(row) = caller_row() {
            // row.id(): the loader's row id; row.key(): its key in its runtime (a new one per restart);
            // row.scope(): the label it isolates this service under (an instance's inside instances), None outside any.
            self.add(row.id(), &name);
            let (tools, id) = (self.clone(), row.id().to_owned());
            row.ctx()
                .effect(move || Effect::Disposer(Box::new(move || Ok(tools.remove(&id, &name)))))
                .map_err(native_error)?;
        }
        Ok(Value::Undefined)
    }
    // methods …
}
```

- Two rows of one runtime are two callers, with different `key()` and `instance()`.
- A call from Rust, from a peer node (including `peer:` rows, whose calls arrive through the services between nodes), or from a runtime outside any row (a service from `Mount::hosts`) is `None`.
- Read it in `invoke` itself: a future that `invoke` returns runs later, elsewhere, and does not see it.
- It comes from how rutis-loader's rows lease services (`Process::lease_host_for`): each row has the services it leases registered in a scope of its own, so the wire is unchanged and it works with Node, Bun, Python and Go runtimes, 0.11 ones included. When you load rows with `Process` yourself, lease with `lease_host_for` and isolate the name under `HostLease::label()`; calls through `lease_host` / `lease_host_in` are `None`.

Declare host events through `emits` on the Rust API below; `rutis-host` JSON configuration does not yet expose this setting. Local events between plugins in one process do not need it.

## Runtime and loader

```rust
use std::sync::Arc;
use rutis_bridge::runtime::LocalRuntime;
use rutis_loader::{
    Chain, Layer, LoaderOptions, LoaderPlugin, Patch, RuntimeResolver, RuntimeRowsPlugin, ServiceCatalog,
};

// Share every service name across languages, as rutis-host does. You can also register only selected names:
// catalog.register_shared("clock").
let mut catalog = ServiceCatalog::new();
catalog.share_by_name();

// Local Node runtime: the directory containing @arcships/rutis-runtime and the package.json used for resolution.
// `emits` names the events the host may emit into the runtime (`Process::emit`):
// the runtime rejects an event it was not named, and plugins receive the
// named ones with `ctx.on` (the same event bus). Set before the runtime is mounted.
// Only runtimes reporting the `events` feature (Node, Bun, Python, Go) accept `emits`; others
// fail at mount. `internal/*` cannot be declared.
let node = LocalRuntime::node("app/node_modules/@arcships/rutis-runtime", "app/package.json")
    .emits(["file-changed", "active-leaf-changed"]);
let node_rows = Arc::new(RuntimeResolver::node(node.handle()).with_catalog(&catalog));
// Local Python runtime: plugin module directory and an interpreter with rutis installed.
let python = LocalRuntime::python("app/plugins").interpreter("app/.venv/bin/python");
let python_rows = Arc::new(RuntimeResolver::modules(python.handle()).with_catalog(&catalog));
root.plugin(node);
root.plugin(python);

let loader = LoaderPlugin::new(
    Chain::new().with_shared(python_rows.clone()).with_shared(node_rows.clone()),
    LoaderOptions { catalog, ..LoaderOptions::default() },
);
let handle = loader.handle();
root.plugin(loader).await?;
root.plugin(RuntimeRowsPlugin::new(node_rows));
root.plugin(RuntimeRowsPlugin::new(python_rows));

let rows: Vec<Patch> = serde_json::from_value(json!([{ "insert": [
    { "id": "weather", "name": "weather-plugin", "config": { "city": "Oslo" } },
    { "id": "llm", "name": "py:llm_gateway" }
] }]))?;
handle.reconcile(vec![Layer::new("app", rows)], None).await?;
```

- A runtime is a plugin. Its process starts with the plugin and exits when the plugin is unloaded. If the process exits unexpectedly, the runtime stops and its rows wait; `RuntimeHandle::state()` reports `Down(reason)`, including why the process ended. The application decides whether to restart it by calling `restart` on the runtime fiber.
- For the full row format (groups, `isolate`, `inject`, expressions, imperative updates, and persistence), see the [rutis-loader design](../design-rutis-loader-2026-10-02.en.md).
- To pick up plugin code changes, call `RuntimeResolver::invalidate_all()` and then `Loader::reload(row)`. Node and Python both reimport only the plugin's entry module.
- With `RUTIS_TRACE` set, each runtime-channel message produces a line on stderr with its direction and length, but not its contents.
- A runtime process gets no standard input, and writes to the application's standard output and error. A plugin that reads the terminal (a terminal UI, Python's `input()`) needs the terminal itself, since it checks that its input is one and sets the terminal's modes: give its runtime this process's input with `stdin(Stdio::Inherit)`. The setting covers the whole runtime process, so every plugin in it can read the input. Load such a plugin into a runtime of its own (`named`), and give the terminal to one runtime at most: processes reading the same terminal race for each key. `stdout(Stdio::Null)` and `stderr(Stdio::Null)` keep the other runtimes from writing over it. The same settings exist on `Launcher`.

```rust
use rutis_bridge::runtime::Stdio;

// A terminal UI in its own runtime, reading the keyboard; the plugins' runtime writes no output over it.
let tui = LocalRuntime::node("app/node_modules/@arcships/rutis-runtime", "app/package.json")
    .named("tui")
    .stdin(Stdio::Inherit);
let node = LocalRuntime::node("app/node_modules/@arcships/rutis-runtime", "app/package.json")
    .stdout(Stdio::Null);
```

### Go runtimes

Go plugins are compiled binaries, one runtime per binary (feature `go` of `rutis-loader`). `GoResolver` reads binaries' manifests (`<binary> --rutis-manifest`) and routes `go:<plugin>` to the binary that has it; `GoRuntimes` starts a binary's runtime when a row uses it and stops it once idle:

```rust
use rutis_loader::{GoBinaries, GoResolver, GoRuntimes};

let go = Arc::new(
    GoResolver::new(GoBinaries::new().dir("app/plugins/go").file("app/bin/netkit"))
        .with_catalog(&catalog)
        .reserve(["node", "py"]),              // other runtimes' names, which no Go runtime may take
);
// In the loader's Chain: Chain::new().with_shared(go.clone())…; after mounting the loader:
let go_runtimes = GoRuntimes::new(go, "app").idle(Some(Duration::from_secs(60)));
let control = go_runtimes.handle();            // restart("go-netkit"), runtimes()
root.plugin(go_runtimes).await?;
```

- A runtime is named after its file (`netkit` → `go-netkit`); when two binaries have a plugin of the same name, the row is `go-netkit:<plugin>`.
- While a process runs, rows resolve from the manifest it started with; a replaced binary takes effect after `control.restart("go-netkit")`. A crashed runtime is not restarted automatically.
- To run one fixed binary, it also works as Python does: `LocalRuntime::go(binary, project)` with `RuntimeResolver::modules` (rows `<runtime name>:<plugin>`).

## Nodes

```rust
use rutis_bridge::transport::websocket::{Config, WebSocketPlugin};
use rutis_bridge::{Credential, Features, IdentityPlugin, LinkConfig, PeerId, PeerPlugin, StaticIdentity};

let (main, office) = (PeerId::new("main")?, PeerId::new("office")?);
root.plugin(WebSocketPlugin::new(Config::new())?).await?;
root.plugin(IdentityPlugin::new(
    "main",
    StaticIdentity::new(main).present(office.clone(), Credential::Bearer(token)),
));
root.plugin(PeerPlugin::new(
    LinkConfig::dial(office, "websocket", "main", "wss://office.example.com/rutis"),
    Features { export: vec!["clock".into()], import: vec!["calendar".into()], ..Features::default() },
)?);
```

For rutis-loader configuration, `rutis_loader::register_peer_node` registers the `rutis-bridge/peer` row (the same format as `rutis.json`), and `PeerResolver` resolves `peer:` rows. For a remote runtime, add `RuntimePlugin::remote(name)` and a node row whose `runtime` is the same name; `RuntimeResolver::modules(handle)` resolves row names as `<name>:<module>`. See [Connect nodes](nodes.en.md) for concepts and security.
