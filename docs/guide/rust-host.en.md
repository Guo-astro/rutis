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
| `rutis-bridge` | Connects processes, languages, and machines. `node` (default), `python` and `go` enable local runtimes; `websocket` enables the WebSocket transport; `cordis` mounts Cordis plugins and generates Rust bindings (see [Cordis](cordis.en.md)); `exports` (in `[build-dependencies]`) generates the dispatchers of [exported Rust services](#export-an-existing-rust-service); `testing` enables compatibility tests. |

Enable only the language features you use so the application compiles and starts only the required components.

## Provide Rust services to plugins

### Export an existing Rust service

A Rust service you already have (including one from a dependency crate) can be used by name from the rows of every language without writing any binding: `build.rs` names its type, and the build generates its dispatcher and TypeScript declarations from the Rust definitions ([#440](https://github.com/arcships/rutis/issues/440)).

```toml
[dependencies]
rutis-bridge = "0.12"

[build-dependencies]
rutis-bridge = { version = "0.12", features = ["exports"] }
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
- Every public `&self` method is exported and is `async` for the caller (a Promise in JavaScript, an awaitable in Python); Python and Go rows call the same dispatcher. A parameter `&Ctx` is the calling row's context, not a parameter for the caller: what the method registers on it goes when that row unloads. A method the generator cannot bind is left out with a build warning saying where and why.
- `Err(e)` is thrown in the caller's language with `name` (the type of `e`), `code` (its variant), `message` (its `Display`) and `data` (its serde value). What crosses, how the source of a dependency crate is found, and the limits: [Cordis](#exported-services-reference). A complete example: [`examples/rust-services`](../../examples/rust-services).

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

## Exported services: reference

Existing Rust services, used by name from the rows of every language (TypeScript, JavaScript, Python, Go), with the dispatcher and TypeScript declarations generated at build time from the Rust definitions. `build.rs` lists the types with `rutis_bridge::build::Exports` (feature `exports` of the build dependency); the application declares the generated module with `rutis_bridge::include_exports!()` and registers `exports::all()` with `ServiceCatalog::export_all`. Usage: [above](#export-an-existing-rust-service); example: [`examples/rust-services`](../../examples/rust-services).

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
| An enum with serde | its type name | its variant as serde writes it: the string it serializes to, the one key of an externally tagged variant, the `tag` field of an internally or adjacently tagged one | its serde value |
| An enum without serde (or `untagged`) | its type name | the variant's Rust name (from its derived `Debug`) | its serde value if it has one |
| A struct with serde | its type name | the string it serializes to, else its type name | its serde value |
| `String` | `Error` | `Error` (the string is the `message`) | — |
| `rutis::CordisError` | `CordisError` | its variant | — |
| `rutis_bridge::session::Error` | thrown as it is (a call it forwarded keeps the far end's error) | | |
| Anything else (`anyhow::Error`, `io::Error`) | its type name | its type name | — |

`message` is always `e`'s `Display`. JavaScript reads `error.code` and `error.data`; Python `error.code`, `error.data`; Go `remote.Code`, `remote.Data` (big integers as `*big.Int`); Rust `Error::code()`, `Error::data()`. They cross in the error graph, so a runtime that does not know them still gets the name and message. In the declarations each error type has a `<Name>Thrown` type, `RustError<'Name', 'Code1' | 'Code2', Data>`, named in the method's JSDoc (`@throws {CalendarErrorThrown}`).

### Declarations

`rutis_exports.d.ts` has an interface per service type, the data types it uses, `RustError`, the `<Name>Thrown` types, and `Services`, the services by name: `ctx.use<Services['calendar']>('calendar')`. It passes `tsc --strict` (`examples/rust-services` checks it). Commit the `.typescript(…)` copy: the build rewrites it when the Rust side changes.

### Limits

- What a macro writes is not seen: types, `impl` blocks or methods written by a macro (other than derives) are not found. A service type must be a concrete struct or enum (not generic, not a trait object).
- Methods taking `&mut self`, borrowing a reference out, taking a reference to a function's argument (`Fn(&A)`), a generic parameter other than a function, `impl Trait` other than `Fn`, or a function that does not return a future of a `Result` are left out with a warning.
- A data type needs a serde derive the generator can read: a hand-written `Serialize`, `#[serde(with / serialize_with / into / remote)]`, or a type of a crate not read (other than `std`, `serde_json`, `chrono`, `uuid`, `url` and the tagged values) needs `.declared(path, typescript)`: until then the methods using it are left out, the warning naming the type.
- `#[cfg]` is evaluated as above; a `cfg` the build script cannot see is taken as off.
- A method's future must be `Send` (the compile fails otherwise).
