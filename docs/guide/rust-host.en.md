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

### Export an existing Rust service

A Rust service you already have (including one from a dependency crate) can be used by name from the rows of every language without writing any binding: `build.rs` names its type, and the build generates its dispatcher and TypeScript declarations from the Rust definitions ([#440](https://github.com/arcships/rutis/issues/440)).

```toml
[dependencies]
rutis-bridge = "0.12"

[build-dependencies]
rutis-bridge = { version = "0.12", features = ["cordis"] }
```

```rust
// build.rs
fn main() {
    rutis_bridge::cordis::build::Exports::new()
        // A type of a dependency crate, by its path as the application writes it.
        .service("calendar", "calendar_services::Calendar")
        // Inside each instance of the instanced group `session`: each instance's rows get their own.
        .service_in("notes", "calendar_services::Notes", "session")
        // A copy of the declarations where the rows' TypeScript imports them (optional).
        .typescript("rows/services.d.ts")
        .generate()
        .expect("generate the exported services");
}
```

```rust
rutis_bridge::include_exports!();          // the generated module `exports`

let mut catalog = ServiceCatalog::new();
catalog.export_all(exports::all());         // the names, served by the loader
// The native service, provided as usual: at TypeKey::of::<Calendar>().
root.provide(Calendar::new())?;
// An instance's Notes: from a row inside the group, at TypeKey::instance::<Notes>(build.instance("session")?).
```

```ts
import { definePlugin } from '@arcships/rutis'
import type { CalendarErrorThrown, Services } from './services.js'

export default definePlugin({
  inject: ['calendar'],
  async apply(ctx) {
    const calendar = ctx.use<Services['calendar']>('calendar')
    const entry = await calendar.add({ title: 'Standup', startsAt: Date.now() })
    try {
      await calendar.get(99_999)
    } catch (error) {
      const thrown = error as CalendarErrorThrown   // name 'CalendarError', code 'NotFound', data { NotFound: { id: 99999 } }
    }
    // A function passed by reference: the Rust side's future resolves once the returned Promise settled.
    await calendar.subscribe(async change => { /* ... */ })
  },
})
```

- The loader serves an export while its native service is there: rows that inject the name wait for it and stop when it goes, as with any service. An instance export resolves as `register_shared_instance` does: a row gets the nearest enclosing instance of the group. Without the loader, mount `exports::calendar().plugin(None)` yourself.
- Every public `&self` method is exported and is `async` for the caller (a Promise in JavaScript, an awaitable in Python); Python and Go rows call the same dispatcher. A method the generator cannot bind is left out with a build warning saying where and why.
- `Err(e)` is thrown in the caller's language with `name` (the type of `e`), `code` (its variant), `message` (its `Display`) and `data` (its serde value). What crosses, how the source of a dependency crate is found, and the limits: [Cordis](cordis.en.md#export-rust-services-to-rows). A complete example: [`examples/rust-services`](../../examples/rust-services).

### Write a `HostDispatch` by hand

In Rust, a service used by name from a plugin is a `dyn HostDispatch` registered under `host_key(name)`; the exports above generate one. For a service whose dispatch is its own business (such as one that hands each row its own object), write it by hand:

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

Declare host events through `emits` on the Rust API below; `rutis-host` JSON configuration does not yet expose this setting. Local events between plugins in one process do not need it.

## Runtime and loader

```rust
use std::sync::Arc;
use rutis_bridge::runtime::LocalRuntime;
use rutis_loader::{
    Chain, Layer, LoaderOptions, LoaderPlugin, Patch, RuntimeResolver, RuntimeRowsPlugin, ServiceCatalog,
};

// Every service name is shared across languages by name, as in rutis-host. To share only the names
// registered as shared (register_shared, export), call catalog.share_registered_only().
let mut catalog = ServiceCatalog::new();

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
