# Embed rutis in a Rust Application

Use the crates directly when your application needs to provide its own Rust services to plugins or when you want to embed rutis in an existing Rust program. If you do not need this, use [rutis-host](rutis-host.en.md); both use the same row configuration format.

## Dependencies

```toml
[dependencies]
rutis = "0.15"
rutis-loader = { version = "0.15", features = ["node", "python", "peer"] }
rutis-bridge = { version = "0.15", features = ["python", "websocket"] }   # node is enabled by default
```

| Crate / feature | Contents |
| --- | --- |
| `rutis` | Core: plugins, dependencies, and services. |
| `rutis-loader` | Loads plugins from data rows. `node` / `python` / `go` enable rows for those languages; `peer` enables node rows and `peer:` rows. |
| `rutis-bridge` | Connects processes, languages, and machines. `node` (default), `python` and `go` enable local runtimes; `websocket` enables the WebSocket transport; `cordis` mounts Cordis plugins and generates Rust bindings (see [Cordis](cordis.en.md)); `exports` (in `[build-dependencies]`) generates the dispatchers of [exported Rust services](#export-an-existing-rust-service); `testing` enables compatibility tests. |

Enable only the language features you use so the application compiles and starts only the required components.

## Provide Rust services to plugins

### Mark the type `#[rutis::service]`

Add `#[rutis::service]` to a Rust service's `impl` block and provide it as usual: the rows of every language call it by name ([#510](https://github.com/arcships/rutis/issues/510)). No list, no `build.rs`, no `export_all`, no `HostDispatch`; the crate that defines the service depends on `rutis` alone.

```rust
use rutis::{service, BoxFuture, CordisError};

#[service]                         // served as "calendar": the type's name, first word lowercased
impl Calendar {
    pub fn new() -> Self { /* … */ }                          // no self: not served
    pub async fn add(&self, entry: NewEntry) -> Result<Entry, CalendarError> { /* … */ }
    pub fn get(&self, id: u64) -> Result<Entry, CalendarError> { /* … */ }
    pub async fn subscribe(&self, ctx: &rutis::Ctx, handler: impl Fn(Change) -> BoxFuture<'static, Result<(), CordisError>>) { /* … */ }
    #[service(skip)]
    pub fn compact(&mut self) { /* … */ }                     // stays in Rust
}

root.provide(Calendar::new())?;    // that is all
```

```ts
const calendar = ctx.use<Services['calendar']>('calendar')
const entry = await calendar.add({ title: 'Standup', startsAt: Date.now() })
```

- **What is served**: the public `&self` methods of the marked `impl` blocks; a type may have several marked blocks. Each is `async` for the caller (a Promise in JavaScript, an awaitable in Python), and Node, Bun, Python and Go rows call the same dispatcher. Method names are camelCase (`add_entry` is `addEntry`). A method under `#[cfg(...)]` is served in the builds that include it (two methods of one name are fine when `#[cfg]` keeps one out of each build); parameter types may name lifetimes or `Self`.
- **Its name**: by default the type's name with the first word lowercased (`Calendar` is `calendar`, `TreeCommands` `treeCommands`, `HTTPClient` `httpClient`); `#[service(name = "cal")]` renames it.
- **Provided means served**: `ctx.provide(value)` (and `provide_as` when the key is the type itself) also serves it by name; one disposer removes both, and both go when the plugin unloads, stopping the rows that use it, as with any service. Provided with a `check()` predicate (`provide_as_with_check`), the name is gated by it too: while it fails, the rows using the name stop as the native service's users do. A mutable binding (`provide_mut_as`) is not served, nor is a keyed one (`TypeKey::keyed`): a second value of the type would take the same name. Each call by name reads the service through the providing context's strict read path, so read interceptors (`intercept_require_as`) deny or replace it as they would for a Rust reader in the calling row's place: those of the row's ancestors apply (the provider's when no row calls). The read serves only the value the entry was provided with: a reference kept after the service went, even once the slot is provided again, is refused. A `provide` refused because the native service is already there serves nothing by name.
- **Its scope follows where it is provided**: provided inside an instance (`TypeKey::instance::<Notes>(build.instance("session")?)`), it is that instance's own, and the rows in that instance get it; provided outside instances, it is global. Provided in an isolation scope (`ctx.isolate(TypeKey::of::<Notes>(), "a")`), the name is served in the same scope (`isolate(host_key("notes"), "a")`), so two isolated scopes each serve their own. No instance group is written down. Once an instance provides it, the rows in that instance use that instance's own and do not fall back to a global one of the same name: when the instance's service goes, they wait for it to be provided there again (design R3, scope follows where it is provided).
- **What crosses, how errors are thrown**: as for the third-party types below ([reference](#exported-services-reference)): serde parameters and results, callbacks by reference, the calling row's `&Ctx`, `Err(e)` thrown with `name`, `code`, `message` and `data`.
- **Errors at compile time**: a public method in a marked block that cannot be bound (`&mut self`, a borrowed result, a mutable reference parameter, a generic parameter that is not a function, …) is a compile error pointing at it; mark one you keep in Rust `#[service(skip)]`.
- **Name conflicts**: when two types in one scope have the same name, the later `provide` fails, and the error names both; nothing of it is provided, the native service included.
- **Unmarked types are not served**: host-internal services (storage, locks, database handles, …) do not reach plugins. When a row waits for a name an unmarked Rust service would have, `Loader::waiting_for(&entry)` lists what the row waits for and says to add `#[rutis::service]` to that type's `impl`. `Loader::check` reports a marked type's name as `Provider::Marked(type)`, served once the type is provided, which a check cannot know (`rutis-host check` prints it as such).
- **dylib plugins**: a marked type in a dylib (whose crate depends on `rutis-sdk` only and writes `#[rutis_sdk::rutis::service]`, or `use rutis_sdk::rutis::service`) is served once the plugin loads; the host need not know it. On a version swap the old generation's entry point goes with its services and the new one's appears when it provides them. A rebuild needs no new version: two builds of one plugin keep their types' `TypeId`s, and each value is served with the code of the build that provided it, so both can be loaded at once. A build that drops the marker serves its values by name no more, and names are those of the newest build. The host links `rutis-bridge`.
- **TypeScript declarations**: `rutis-host types <crate dir> -o services.d.ts` finds every marked type in the crate and in the crates it uses that depend on rutis, and writes their declarations (`Services['calendar']`, the data types, `<Name>Thrown`). To refresh them on every build, write `rutis_bridge::build::Exports::new().typescript("rows/services.d.ts").generate()` in `build.rs`, with no type list. Calls need no declarations.

A complete example: [`examples/marked-services`](../../examples/marked-services).

### Third-party types: list them in `build.rs`

For a type whose source you cannot change (one of another crate), `build.rs` names its type, and the build generates its dispatcher and TypeScript declarations from the Rust definitions ([#440](https://github.com/arcships/rutis/issues/440)). Do not list a marked type as well: that is a build error.

```toml
[dependencies]
rutis-bridge = "0.15"

[build-dependencies]
rutis-bridge = { version = "0.15", features = ["exports"] }
```

```rust
// build.rs
fn main() {
    rutis_bridge::build::Exports::new()
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

- The loader serves an export while its native service is there: rows that inject the name wait for it and stop when it goes, as with any service. An instance export resolves as `register_shared_instance` does: a row gets the nearest enclosing instance of the group. Without the loader, mount `exports::calendar().plugin(None)` yourself.
- Every public `&self` method is exported, by the same rules as marked ones; a method the generator cannot bind is left out with a build warning saying where and why. A complete example: [`examples/rust-services`](../../examples/rust-services).

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

To give an error a code and data, as an exported service's `Err(e)` has, return `rutis_bridge::exports::failure(name, code, message, data)`: the caller's language throws an error named `name` with `message`, and `code` and `data` (a `serde_json::Value`, or `None`) beside them.

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
- A call from Rust, from a peer node (including `peer:` rows, whose calls arrive through the services between nodes), from a runtime outside any row (a service from `Mount::hosts`), or from code of a row that uses the service without declaring it (a Cordis `ctx.get`, which finds the registration shared by all rows) is `None`.
- Read it in `invoke` itself: a future that `invoke` returns runs later, elsewhere, and does not see it.
- It comes from how rutis-loader's rows lease services (`Process::lease_host_for`): each row has the services it leases registered in a scope of its own (besides the registration shared by all rows, as before), so the wire is unchanged and it works with Node, Bun, Python and Go runtimes, 0.11 ones included. When you load rows with `Process` yourself, lease with `lease_host_for` and isolate the name under `HostLease::label()`; calls through `lease_host` / `lease_host_in` are `None`.

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
// Events need no configuration: listeners in the runtime register themselves on the host's bus (see "Events" below).
let node = LocalRuntime::node("app/node_modules/@arcships/rutis-runtime", "app/package.json");
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
- While a process runs, rows resolve from the manifest it started with; a replaced binary takes effect after `control.restart("go-netkit")`.
- A crashed runtime is not restarted automatically by default. After `control.supervise(RestartPolicy::OnFailure, |end| …)`, a runtime whose process ended on its own starts again after a wait (1 s, doubling each time up to 30 s, back to 1 s after 60 s of running; after 5 restarts within 5 minutes one more end gives up), each runtime counted on its own; `RestartPolicy::Exit` only reports the end to the callback (`RuntimeEnd::Exit`), for the application to stop. `end.line()` is the line rutis-host prints; `control.unsupervise()` stops supervising, and a restart waiting out its wait does not happen. A runtime that ended before the first `supervise` (it crashed while the application was starting) is handled by `supervise` all the same, only its latest end kept; an application that will not supervise calls `control.unsupervise()` once, and those ends are no longer kept. A binary whose handshake was refused (an old SDK) did not end on its own and is not restarted.
- Compiled languages other than Go (Swift, later) share the implementation: `BinaryResolver<L>` and `BinaryRuntimes<L>`, where `L` implements `BinaryLanguage` (the row prefix, the SDK's marker, how a binary starts); `GoResolver` is `BinaryResolver<Go>`.

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

## Events

The host's Rust plugins, the plugins in every runtime (any language) and the listeners on linked nodes are on one logical bus ([plugin API: events](plugin-api.en.md#events)). Runtime listeners register themselves and the host side configures nothing: what you emit on `ctx.events()` reaches listeners in every language, and what they emit reaches the Rust listeners here.

An event type is named across languages by `Event::NAME` and its arguments are serde-encoded, so it and its `Value` derive `Serialize` / `Deserialize`. A tuple struct of two or more fields is that many arguments, so `Created` below is `emit("session/created", id, size)` in other languages:

```rust
use std::sync::Arc;
use rutis::{BoxFuture, Call, CordisError, Ctx, Event, EventKey, Listener, Mode, SyncCall};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
struct Created(String, u32);
impl Event for Created {
    const NAME: &'static str = "session/created";
    type Value = String;
}

struct Audit;
impl Listener<Created> for Audit {
    fn call<'a>(&'a self, _ctx: &'a Ctx, event: &'a Created, call: Call<'a, String>)
        -> BoxFuture<'a, Result<Option<String>, CordisError>> {
        Box::pin(async move {
            if call.mode() == Mode::Waterfall {
                // middleware: run the rest of the chain, then rewrite its result
                let inner = call.next().await?;
                return Ok(inner.map(|text| format!("[{text}]")));
            }
            Ok(Some(format!("audited {}", event.0)))      // Ok(None) is no result
        })
    }
}

fn terminal<'a>(_: &'a Ctx, e: &'a Created) -> BoxFuture<'a, Result<Option<String>, CordisError>> {
    Box::pin(async move { Ok(Some(format!("saved {}", e.0))) })
}

let key = EventKey::<Created>::of();
let events = ctx.events();
let off = events.on(&ctx, &key, Audit)?;                                   // the listener is called with this ctx
events.on_sync(&ctx, &key, |_: &Ctx, e: &Created, _: SyncCall<'_, String>| Ok(Some(format!("sync {}", e.1))))?;

events.emit(&ctx, &key, Arc::new(Created("s1".into(), 3)))?;              // returns at once
events.parallel(&ctx, &key, Arc::new(Created("s1".into(), 3))).await?;    // waits for all; ListenersFailed
let first: Option<String> = events.serial(&ctx, &key, Arc::new(Created("s1".into(), 3))).await?;
let out = events.waterfall(&ctx, &key, &Created("s1".into(), 3), terminal).await?;   // `()`: no terminal
let sync = events.bail_sync(&ctx, &key, &Created("s1".into(), 3));        // meets Audit (async): AsyncListener
let python_only = events
    .filter(|l| l.runtime.as_deref() == Some("py"))                       // ListenerInfo
    .serial(&ctx, &key, Arc::new(Created("s1".into(), 3)))
    .await?;
off.dispose().await?;
```

| Topic | Rule |
| --- | --- |
| Listeners | A listener registered once can be called by all five modes; `call.mode()` is this call's mode, `call.next()` works only in a `waterfall` (otherwise `WaterfallNext`). `on_sync` registers a synchronous listener, callable by every mode but unable to call `next` in an asynchronous `waterfall`; an (asynchronous) `on` listener cannot be called by `bail_sync` / `waterfall_sync` (`AsyncListener`) |
| Options and prefixes | `on_opt` / `on_sync_opt` take an `EventOptions`, written with its builders: `EventOptions::default().prepend(true)` (also `.once(true)`, `.global(true)`); `on_pattern(&ctx, &EventPattern::prefix("session/"), ..)` receives every name under the prefix |
| By name | Without a type, use `rutis_bridge::events::Args`: `EventKey::<Args>::dynamic("file-changed")`, arguments `Args(vec![Value::Data(json!(…))])`, results `ArgValue`. Typed and `Args` listeners of one name receive each other's events, converted through serde |
| Scope | The emitter's scope is `ctx.scope()`: the loader calls `Ctx::mark_scope` for each group instance, and a listener receives the events of its scope and those inside it. There is no API to emit into another scope; filter on `ListenerInfo::scope` |
| Failures | A failing `emit` listener reaches the error sink as `CordisError::ListenerFailed` (`Ctx::root_with_sink`), and `rutis_bridge::events::report_line` gives the line rutis-host prints; `parallel` gives `CordisError::ListenersFailed` (registration order); errors of listeners elsewhere arrive with their name and message |
| Unload and cancellation | When a plugin unloads, its listeners leave the bus, running calls see `ctx.cancelled()`, and the unload waits for them (up to the shutdown deadline). Dropping the future of `parallel` / `serial` / `waterfall` cancels the dispatch |
| Framework events | `rutis_loader::RowEvent` (`rutis/row`), `rutis_bridge::events::{ServiceEvent, RuntimeEvent}` (`rutis/service`, `rutis/runtime`): listen with `on`; emitting `rutis/*` fails with `ReservedEvent` |
| Nodes | `Features { export, import }` of `PeerPlugin` select events too (names or `*` prefixes), see [nodes](#nodes) and [connecting nodes](nodes.en.md#events-between-nodes) |

Tests need no separate tool: in a `#[tokio::test]`, take `Ctx::root()` (or `Ctx::root_with_sink` to collect `emit` failures), load the plugins, and emit and assert as above. Listeners in runtimes need a real runtime (`LocalRuntime`), as in production.

## Exported services: reference

Rust services, used by name from the rows of every language (TypeScript, JavaScript, Python, Go). For a type marked `#[rutis::service]`, the macro generates the dispatcher at compile time and providing the type serves it ([above](#mark-the-type-rutisservice)); for a type whose source you cannot change, `build.rs` lists it with `rutis_bridge::build::Exports` (feature `exports` of the build dependency), the application declares the generated module with `rutis_bridge::include_exports!()` and registers `exports::all()` with `ServiceCatalog::export_all` ([above](#third-party-types-list-them-in-buildrs)). Both follow the rules below for what crosses and how errors are thrown; the generator writes the TypeScript declarations of both from the source. Examples: [`examples/marked-services`](../../examples/marked-services), [`examples/rust-services`](../../examples/rust-services).

| `Exports` | Effect |
| --- | --- |
| `.service(name, path)` | Export the struct or enum at `path` as the service `name`, outside instances |
| `.service_in(name, path, group)` | Inside each instance of the instanced group `group` (as `register_shared_instance`) |
| `.declared(path, typescript)` | The TypeScript type to use for the Rust type at `path`, where the generator cannot derive it |
| `.typescript(file)` | Also write the declarations to `file` (relative to `Cargo.toml`), only when they change |
| `.generate()` | Write `OUT_DIR/rutis_exports.rs` and `rutis_exports.d.ts`; `exports::TYPESCRIPT` holds the declarations |

### Where the source comes from

`path` is written as the application writes it: `crate::…` for its own types, a dependency's crate name (as `Cargo.toml` names it, renames included) for a dependency's. The build script runs `cargo metadata` (for the target being built, offline first) to find each crate's entry file and the features Cargo resolved, and reads the module tree with syn: `mod` files (`#[path]` included), `use` items, globs and `pub use` re-exports (a type re-exported from a private module is found), type aliases (generic ones too). The methods are the public `&self` methods of the type's inherent `impl` blocks anywhere in its crate. `#[cfg]` is evaluated: `feature = "…"` from the features Cargo resolved, target `cfg`s from `CARGO_CFG_*`, `test` and `doc` as off. Stable Rust only: nothing is compiled or expanded for this.

### What crosses

| Rust | TypeScript | |
| --- | --- | --- |
| `bool`, `String`, `&str`, `char`, `PathBuf`, `&Path` | `boolean`, `string` | |
| `f32`, `f64`, `u8`–`u32`, `i8`–`i32` | `number` | |
| `u64`, `i64`, `usize`, `isize`, `u128`, `i128` | `number \| bigint` | beyond ±(2^53−1) it crosses as a bigint (0.11 value rules); both are accepted |
| `Option<T>` | `T \| null` | a trailing `Option` parameter may be left out; an `Option` field is optional |
| `Vec<T>`, `&[T]`, sets, `VecDeque<T>`, `[T; N]` / tuples | `T[]` / `[A, B]` | |
| `HashMap<K, V>`, `BTreeMap<K, V>` | `Record<string, V>` | |
| `Box<T>`, `Arc<T>`, `Rc<T>`, `Cow<T>` | `T` | |
| `serde_json::Value` | `unknown` | |
| `rutis_bridge::session::{Instant, Bytes, BigInt, ValueMap, ValueSet}` | `Date`, `Uint8Array`, `bigint`, `Map`, `Set` | the tagged values |
| A struct or enum deriving `Serialize` / `Deserialize` | an `interface` or a union, as serde writes it | `rename`, `rename_all`, `rename_all_fields`, `tag`, `content`, `untagged`, `transparent`, `flatten`, `skip`, `default`, `skip_serializing_if`; generic types are generic in TypeScript; docs become JSDoc |
| `impl Fn(A, ..) -> BoxFuture<'static, Result<X, E>>`, a generic `F: Fn(..)`, `Arc<dyn Fn ..+ Send + Sync>`, `Box<dyn Fn .. + Send>` (aliases too) | `(arg0: A, ..) => X \| Promise<X>` | a function passed by reference; up to four arguments, taken by value |
| `async fn`, or a return type `BoxFuture<'_, T>`, `Pin<Box<dyn Future<Output = T> ..>>`, `impl Future<Output = T>` | `Promise<T>` | every method is `Promise`-returning for the caller |

Arguments are decoded with serde (any `Deserialize`), results encoded with serde (any `Serialize`); Cargo checks every generated call against the real signatures.

**Functions passed by reference:** the Rust side gets a function that calls the caller's: its future resolves once the function returned and the Promise it returned settled, so a subscriber has handled an event before the native side goes on; a throw or rejection is the future's `Err` (`E: From<rutis_bridge::session::Error>`, which `CordisError`, `anyhow::Error` and boxed errors are), so it takes the native failure path. What the function returns where Rust expects `()` is ignored.

**Cancellation:** a call the caller stops waiting for (an `AbortSignal`, a dropped future) drops the method's future on the Rust side, as for every call.

**The calling row's context:** a parameter `&rutis::Ctx` (or `Ctx`) is not a parameter in TypeScript: it is the context of the row making the call (`rutis_bridge::session::caller_row()`), so what the method registers on it (`ctx.effect`, a listener) is that row's and goes when the row unloads or restarts, for example a subscription. Called from outside any row (from Rust, from a peer node), such a method fails, saying it needs the calling row's context.

### Errors

A method returning `Result<T, E>` (`std::result::Result`, an alias of it, `anyhow::Result`, `io::Result`) throws `Err(e)` in the caller's language: a JavaScript `Error`, a Python `rutis.RemoteError`, a Go `*rutis.RemoteError`. It is not an unclassified call error:

| `E` | `name` | `code` | `data` |
| --- | --- | --- | --- |
| An enum with serde | its type name | its variant as serde writes it: the string it serializes to, the one key of an externally tagged variant, the `tag` field of an internally or adjacently tagged one (except, for a `#[rutis::service]` type, a tagged variant given its own `rename`: see [Limits](#limits)) | its serde value |
| An enum without serde (or `untagged`) | its type name | the variant's Rust name (from its derived `Debug`) | its serde value if it has one |
| A struct with serde | its type name | the string it serializes to, else its type name | its serde value |
| `String` | `Error` | `Error` (the string is the `message`) | — |
| `rutis::CordisError` | `CordisError` | its variant | — |
| `rutis_bridge::session::Error` | thrown as it is (a call it forwarded keeps the far end's error) | | |
| Anything else (`anyhow::Error`, `io::Error`) | its type name | its type name | — |

`message` is always `e`'s `Display`. JavaScript reads `error.code` and `error.data`; Python `error.code`, `error.data`; Go `remote.Code`, `remote.Data` (big integers as `*big.Int`); Rust `Error::code()`, `Error::data()`. They cross in the error graph, so a runtime that does not know them still gets the name and message. In the declarations each error type has a `<Name>Thrown` type, `RustError<'Name', 'Code1' | 'Code2', Data>`, named in the method's JSDoc (`@throws {CalendarErrorThrown}`).

### Declarations

`rutis_exports.d.ts` (and the file `rutis-host types` writes) has an interface per service type, the data types it uses, `RustError`, the `<Name>Thrown` types, and `Services`, the services by name: `ctx.use<Services['calendar']>('calendar')`. It passes `tsc --strict` (`examples/rust-services` checks it). Commit the `.typescript(…)` copy: the build rewrites it when the Rust side changes.

### Limits

- What a macro writes is not seen: types, `impl` blocks or methods written by a macro (other than derives) are not found. A service type must be a concrete struct or enum (not generic, not a trait object).
- Methods taking `&mut self`, borrowing a reference out, taking a reference to a function's argument (`Fn(&A)`), a generic parameter other than a function, `impl Trait` other than `Fn`, or a function that does not return a future of a `Result` are left out with a warning.
- A data type needs a serde derive the generator can read: a hand-written `Serialize`, `#[serde(with / serialize_with / into / remote)]`, or a type of a crate not read (other than `std`, `serde_json`, `chrono`, `uuid`, `url` and the tagged values) needs `.declared(path, typescript)`: until then the methods using it are left out, the warning naming the type.
- `#[cfg]` is evaluated as above; a `cfg` the build script cannot see is taken as off.
- A method's future must be `Send` (the compile fails otherwise).

For types marked `#[rutis::service]`, also:

- The macro sees only the `impl` block: decisions that depend on a type (is it a `Result`, how its error gets a `code`, does a parameter take a function) are left to the expanded, concrete code, so type aliases are recognized. Write an async method as `async fn`, or return `BoxFuture<'_, T>`, `Pin<Box<dyn Future<Output = T> + Send>>` or `impl Future<Output = T>`; an alias of a future as the return type is a compile error. A `&Ctx` parameter is recognized by the name `Ctx`.
- An error's `code` is taken from the value when it is thrown, by the table above; for an internally or adjacently tagged enum whose variant `#[serde(rename = "...")]` gives a name unrelated to its Rust one, the `code` is the Rust name.
- The generated code depends on `rutis` only (a dylib plugin writes `#[rutis_sdk::rutis::service]` or `use rutis_sdk::rutis::service`, and the code then goes through `rutis_sdk::rutis`: the path the marker is reached by decides, so a plugin built against an sdk-bundle, with no dependency written, works too; another path: `#[service(crate = "my::rutis")]`). Values are encoded by the `rutis-bridge` the host links, one more dynamic dispatch than the code `build.rs` generates, negligible next to crossing processes.
