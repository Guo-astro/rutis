# Embed rutis in a Rust Application

Use the crates directly when your application needs to provide its own Rust services to plugins or when you want to embed rutis in an existing Rust program. If you do not need this, use [rutis-host](rutis-host.en.md); both use the same row configuration format.

## Dependencies

```toml
[dependencies]
rutis = "0.17"
rutis-loader = { version = "0.17", features = ["node", "python", "peer"] }
rutis-bridge = { version = "0.17", features = ["python", "websocket"] }   # node is enabled by default
schemars = "1"                     # the config types Builtins::register decodes derive schemars::JsonSchema
serde = { version = "1", features = ["derive"] }
tokio = { version = "1", features = ["rt-multi-thread", "macros"] }
```

| Crate / feature | Contents |
| --- | --- |
| `rutis` | Core: plugins, dependencies, and services. |
| `rutis-loader` | Loads plugins from data rows. `node` / `python` / `go` enable rows for those languages; `peer` enables node rows and `peer:` rows; `install`: installing plugins from npm, PyPI, Go modules and local paths (see [Installing plugins](#installing-plugins)). |
| `rutis-bridge` | Connects processes, languages, and machines. `node` (default), `python` and `go` enable local runtimes; `websocket` enables the WebSocket transport; `cordis` mounts Cordis plugins and generates Rust bindings (see [Cordis](cordis.en.md)); `exports` (in `[build-dependencies]`) writes the TypeScript declarations of [marked Rust services](#mark-the-type-rutisservice) from `build.rs`; `testing` enables compatibility tests. |

Enable only the language features you use so the application compiles and starts only the required components.

## Mount Rust plugins

How to write a Rust plugin: [write a Rust plugin](rust-plugin.en.md); `Greeter`, `Printer` and `Weather` below are its examples. The methods of a host (making and closing the root, reading and writing by key, mounting from configuration) are in `rutis::host`; after `use rutis::host::*;` they are methods of `Ctx`:

```rust
use rutis::host::*;
use rutis::Ctx;
use rutis_loader::Builtins;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = Ctx::root()?;                       // on the current tokio runtime
    root.plugin(Greeter { lang: "Rust".into() }).await?; // provides Greeting
    root.plugin(Printer).await?;                   // its Deps (Greeting) are there: prints "hello from Rust"
    let weather = root.plugin_from(Weather::new, WeatherConfig { city: "Oslo".into() });
    weather.update(WeatherConfig { city: "Bergen".into() }).await?; // rebuilt with Weather::new

    let mut builtins = Builtins::new();            // rutis-loader rows find it by name
    builtins.register("weather", Weather::new);
    // …into the loader's Chain, see "Runtime and loader" below

    root.shutdown().await?;                        // waits for every cleanup
    Ok(())
}
# use std::sync::Arc;
# use rutis::Plugin;
# pub struct Greeting(pub String);
# pub struct Greeter { pub lang: String }
# impl Plugin for Greeter {
#     type Deps = ();
#     async fn apply(&self, ctx: &Ctx, _: ()) -> rutis::Result {
#         ctx.provide(Greeting(format!("hello from {}", self.lang)))?;
#         Ok(())
#     }
# }
# pub struct Printer;
# impl Plugin for Printer {
#     type Deps = Arc<Greeting>;
#     async fn apply(&self, _: &Ctx, greeting: Arc<Greeting>) -> rutis::Result {
#         println!("{}", greeting.0);
#         Ok(())
#     }
# }
# #[derive(serde::Deserialize, schemars::JsonSchema)]
# pub struct WeatherConfig { pub city: String }
# pub struct Weather { city: String }
# impl Weather {
#     pub fn new(config: &WeatherConfig) -> rutis::Result<Self> {
#         Ok(Weather { city: config.city.clone() })
#     }
# }
# impl Plugin for Weather {
#     type Deps = ();
#     async fn apply(&self, _: &Ctx, _: ()) -> rutis::Result {
#         println!("weather in {}", self.city);
#         Ok(())
#     }
# }
```

| To | Write |
| --- | --- |
| Make the root | `Ctx::root()` (the current tokio runtime; an error without one), `Ctx::root_with(handle)`, `Ctx::root_with_sink(handle, sink)` (your own error sink) |
| Mount a plugin | `root.plugin(p)`: any `Plugin`, or a `host::DynPlugin`; a `Box<dyn DynPlugin>` is one too, for a list of plugins chosen at run time |
| Mount from configuration | `root.plugin_from(build, config)`: `build` is a `Fn(&C) -> Result<P>`, the dependencies come from `P::Deps`; the returned `FiberView` can `update(config)`, and its name in diagnostics is the plugin's type name. When the dependencies are known only at run time, implement `host::PluginFactory<C>` and use `plugin_with(factory, config)` |
| rutis-loader rows | `Builtins::register(name, build)`: the config is decoded as `C`, its schema generated from `C` (none for `serde_json::Value`); `register_with(name, \|build, config\| …)`: one copy built per instance; `register_raw(name, factory, schema)`: a `PluginFactory<serde_json::Value>` |
| Read services | `get::<T>()`; by key `get_at(&key)`, provide with `provide_at(key, arc)`, the key a `host::Key<T>` (see [depend on services by name](#depend-on-services-by-name)); `require` / `require_at` are strict reads, of declared dependencies in a `DynPlugin`'s `apply` |
| Stop | `root.shutdown().await` unloads everything and waits for the cleanups, with no time limit (how long is the host's choice); `shutdown_with_timeout(limit)` bounds the wait, its error naming the plugins and cleanups not finished ([default output](#default-output)); one plugin: `view.dispose()`, `view.restart()`, `view.dispose_with_timeout(limit)` |
| Observe | `view.state()`, `view.watch()`, `root.diagnostics()`, `root.subscriptions()` |

What waiting on a mounted plugin waits for:

- `view.await` (and `update`, `restart`) returns once the fiber has handled what was queued for it: its load, the update, the restart. A plugin whose `Deps` are not all there does not load: it stays `Pending`, and `view.await` returns `Ok(())` all the same, with the plugin not running.
- To wait until the plugin runs, watch its state: `view.watch()` receives every change of state; wait for `Active` (running) or `Failed` (the load failed, its error in the snapshot's `error`). With a dependency missing this waits on; bound it with `tokio::time::timeout`, and on a timeout `view.state()` and `root.diagnostics()` say what the plugin waits for.
- A plugin still waiting after 10 seconds is reported once to the error sink: `ErrorKind::StillWaiting` names it and the services it misses (the default sink prints `printer: still waiting after 10s for app::Greeting: …`), and it goes on waiting. rutis's own plugins (transports, links, runtimes, the loader) are not reported: their own state says what they wait for. `root.report_waiting_after(Some(duration))` changes the delay for the whole tree, `None` turns it off ([dependency diagnostics](../core-features.en.md#dependency-diagnostics)).
- Mounting order does not change the outcome: a consumer mounted before its provider waits for it. The example above mounts the provider first, so the consumer runs by the time `view.await` returns.

### Depend on services by name

`type Deps` depends by type. When two services of one type are provided under different names (two plugins providing `dyn Llm` as `openai` and `local`) and a plugin depends on one by name, implement the object-safe form of a plugin, `host::DynPlugin`: list the keys in `injects()` and read them with `require_at` in `apply`:

```rust
use rutis::host::*;
use rutis::Ctx;

pub trait Llm: Send + Sync {}

const OPENAI: Key<dyn Llm> = Key::named("openai");

struct Router {
    local: Key<dyn Llm>,              // Key::dynamic(a name from the config)
}

impl DynPlugin for Router {
    fn name(&self) -> &str {
        "router"
    }

    fn injects(&self) -> Vec<TypeKey> {
        vec![OPENAI.erase(), self.local.erase()]
    }

    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, rutis::Result<Effect>> {
        Box::pin(async move {
            let smart = ctx.require_at(&OPENAI)?;
            let fast = ctx.require_at(&self.local)?;
            // starts once both are ready; restarts when either is replaced
            Ok(Effect::Done)
        })
    }
}
```

- The provider writes `ctx.provide_at(Key::named("openai"), arc)?`; a read that does not gate is `ctx.get_at(&key)`.
- A `Key<T>` names its value type: `Key::of()`, `Key::named("x")` (a `const` if you like), `Key::dynamic(name)` (a name known at run time), `.instance(id)` (an instance's own); `.erase()` gives the `TypeKey` that `injects` wants. A value of another type than the key's does not compile.
- Every `Plugin` is a `DynPlugin`, so the two kinds can depend on each other. The `Effect` that `DynPlugin::apply` returns is a cleanup, `Effect::Done` for none; `ctx.effect` in `apply` works too.

### Errors a host sees

- Every signature's error is `rutis::Error`. A host function returning `Box<dyn Error>` (or `Box<dyn Error + Send + Sync>`) uses `?` directly; with rutis's `anyhow` feature, one returning `anyhow::Result` does too.
- `err.kind()` returns `Option<&host::ErrorKind>`: `Some` for rutis's own failures (`Closed`, `InjectUnsatisfied`, `ListenersFailed`, …: 0.16's `CordisError` renamed, the same variants), `None` for an error from plugin code, which reaches the host wrapped in `ErrorKind::PluginFailed`.
- Errors are not `Clone`. An error several parties hold is an `Arc<ErrorKind>`: what the error sink (`ErrorSink`) receives, what `take_cleanup_errors()` returns, and the result of awaiting a `FiberView`, `dispose()` or `shutdown()`.

```rust,no_run
use std::sync::Arc;
use rutis::host::*;
use rutis::Ctx;

fn describe(error: &rutis::Error) -> &'static str {
    match error.kind() {
        Some(ErrorKind::Closed) => "root is shut down",
        Some(ErrorKind::ListenersFailed { .. }) => "some listeners failed",
        Some(_) => "another rutis failure",
        None => "the plugin's own error",
    }
}

let sink: ErrorSink = Arc::new(|error: Arc<ErrorKind>| eprintln!("[app] {error}"));
let root = Ctx::root_with_sink(tokio::runtime::Handle::current(), sink);
```

### Default output

The error sink `Ctx::root()` and `Ctx::root_with(handle)` install (`host::default_sink()`) writes each error it gets to standard error, with no prefix such as `[rutis]`; an error is usually one line, and errors gathered together (`multiple errors:`) take a line each. What went wrong is visible even when nobody reads a result:

| What | One line, beginning with the name | Error |
| --- | --- | --- |
| A plugin or rutis-loader row fails to start (`apply` failed, the row's plugin is unknown, its config is wrong) | `weather: failed: no API key` | `ErrorKind::StartFailed { plugin, error }`; a `view.await` on it still gets the error |
| A language runtime's process ends or cannot start | `runtime py: down (runtime process exited with exit status: 1)` | `ErrorKind::RuntimeDown { runtime, reason }`; its rows wait for it |
| A plugin still waits after 10 seconds | `printer: still waiting after 10s for app::Greeting: …` | `ErrorKind::StillWaiting` |
| A type marked `#[rutis::service]` is provided, but rutis-bridge is not linked | `service calendar is provided by type but not registered by name: …` | `ErrorKind::NotServedByName`, once per service |
| A listener fails | `listener of … failed: …` | `ErrorKind::ListenerFailed` |

The name is the row's (once the row has loaded), else the plugin's; a copy of a row refused in an instance is written `weather in session 7` (the group and the instance's number). rutis's own plugins (transports, links, runtimes, the loader) do not report failed starts here: the runtime's line or the link's state says so already. An `apply` failing because it was stopped (`dispose`, a shutdown) and a runtime ending while the whole tree shuts down are not reported. A config change refused for a running row, which goes on with its previous config, is no failed start and is not reported here; `reconcile`'s result has it.

One cause can print several lines: a plugin that fails to start reports the errors of the cleanups its rollback ran, then `StartFailed` (whose error holds them too); a parent failing because a child failed reports once for each.

The sink is called from inside rutis: do not wait on rutis operations in it. rutis-loader reports refused rows while holding its operation lock, so a sink calling that loader's `reconcile` or another change waits forever; to do such things, send the error to another task.

To turn it off or change it, replace the sink: `Ctx::root_with_sink(handle, sink)`. To keep the rest and drop one kind, filter by variant in your own sink and hand the rest to `default_sink()`:

```rust,no_run
use std::sync::Arc;
use rutis::host::*;
use rutis::Ctx;

let fallback = default_sink();
let sink: ErrorSink = Arc::new(move |error: Arc<ErrorKind>| {
    if !matches!(*error, ErrorKind::StartFailed { .. }) {
        fallback(error);
    }
});
let root = Ctx::root_with_sink(tokio::runtime::Handle::current(), sink);
```

Shutting down has no time limit: `root.shutdown()` waits until every cleanup has finished, and how long to wait is the host's choice (rutis-host: 10 s by default). To bound it, `root.shutdown_with_timeout(duration)`; when the time runs out the shutdown goes on in the background, and the error (`DisposeWaitError::TimedOut`) lists in `stuck` the plugins not finished and what each is doing, shown as one line:

```text
not finished after 3s: weather: still running the cleanup registered at src/weather.rs:12:9
```

A cleanup registered with `ctx.effect` is named by the file, line and column it was registered at; any other by the plugin or row and what it is doing: `still withdrawing calendar` (a service it provided), `still running the cleanup its apply returned` (the cleanup `apply` returned, as a rutis-loader row's is), `its apply has not returned`, `still waiting for its event calls, or the copies of its plugin definitions, to end`. The root is no plugin: only a cleanup the host registered on it with `ctx.effect` is listed, as `the host`. rutis's own plugins (links, runtimes and the like) are listed only when no plugin of the developer's is stuck, being the only lead then.

## Provide Rust services to plugins

### Mark the type `#[rutis::service]`

Add `#[rutis::service]` to a Rust service's `impl` block and provide it as usual: the rows of every language call it by name ([#510](https://github.com/arcships/rutis/issues/510)). No `build.rs`, no `HostDispatch`; the crate that defines the service depends on `rutis` alone.

```rust,ignore
use rutis::host::BoxFuture;
use rutis::service;

#[service]                         // served as "calendar": the type's name, first word lowercased
impl Calendar {
    pub fn new() -> Self { /* … */ }                          // no self: not served
    pub async fn add(&self, entry: NewEntry) -> Result<Entry, CalendarError> { /* … */ }
    pub fn get(&self, id: u64) -> Result<Entry, CalendarError> { /* … */ }
    pub async fn subscribe(&self, ctx: &rutis::Ctx, handler: impl Fn(Change) -> BoxFuture<'static, rutis::Result>) { /* … */ }
    #[service(skip)]
    pub fn compact(&mut self) { /* … */ }                     // stays in Rust
}

root.provide(Calendar::new())?;    // that is all
```

```ts
const calendar = ctx.use<Services['calendar']>('calendar')
const entry = await calendar.add({ title: 'Standup', startsAt: Date.now() })
```

- **What is served**: the public `&self` methods of the marked `impl` blocks; a type may have several marked blocks. Each is `async` for the caller (a Promise in JavaScript, an awaitable in Python), and Node, Bun, Python and Go rows call the same dispatcher. Method names are camelCase (`add_entry` is `addEntry`). A method under `#[cfg(...)]` is served, and checked for whether it can be bound, only in the builds that include it (two methods of one name are fine when `#[cfg]` keeps one out of each build); two methods of one name across a type's marked blocks cannot both be served, and `provide` fails naming it. Parameter types may name lifetimes or `Self`.
- **Its name**: by default the type's name with the first word lowercased (`Calendar` is `calendar`, `TreeCommands` `treeCommands`, `HTTPClient` `httpClient`); `#[service(name = "cal")]` renames it.
- **Provided means served**: `ctx.provide(value)` (and `provide_as::<T>` providing it as its own type) also serves it by name; one disposer removes both, and both go when the plugin unloads, stopping the rows that use it, as with any service. Provided with a `check()` predicate (a way of providing internal to rutis's own crates), the name is gated by it too: while it fails, the rows using the name stop as the native service's users do. A writable binding is not served, nor is a named one (`provide_at(Key::named(…), …)`): a second value of the type would take the same name. Each call by name reads the service through the providing context's strict read path, so read interceptors (`intercept_require_as`) deny or replace it as they would for a Rust reader in the calling row's place: those of the row's ancestors apply (the provider's when no row calls). The read serves only the value the entry was provided with: a reference kept after the service went, even once the slot is provided again, is refused. A `provide` refused because the native service or the name is already there serves nothing. The name is served after the native service and withdrawn before it (by the disposer and by the plugin unloading alike), so a row the name wakes can always call it.
- **Its scope follows where it is provided**: provided inside an instance (`ctx.provide_at(Key::of().instance(build.instance("session")?), notes)`), it is that instance's own, and the rows in that instance get it; provided outside instances, it is global. Provided in an isolation scope (`ctx.isolate(Key::<Notes>::of(), "a")`), the name is served in the same scope (`isolate(host_key("notes"), "a")`), so two isolated scopes each serve their own. No instance group is written down. Once an instance provides it, the rows in that instance use that instance's own and do not fall back to a global one of the same name: when the instance's service goes, they wait for it to be provided there again (design R3, scope follows where it is provided).
- **What crosses, how errors are thrown**: see the [reference](#exported-services-reference): serde parameters and results, callbacks by reference, the calling row's `&Ctx`, `Err(e)` thrown with `name`, `code`, `message` and `data`.
- **Errors at compile time**: a public method in a marked block that cannot be bound (`&mut self`, a borrowed result, a mutable reference parameter, a generic parameter that is not a function, …) is a compile error pointing at it; mark one you keep in Rust `#[service(skip)]`.
- **Name conflicts**: when two types in one scope have the same name, the later `provide` fails, and the error names both; nothing of it is provided, the native service included.
- **Unmarked types are not served**: host-internal services (storage, locks, database handles, …) do not reach plugins. When a row waits for a name an unmarked Rust service would have, `Loader::waiting_for(&entry)` lists what the row waits for and says to add `#[rutis::service]` to that type's `impl`. `Loader::check` reports a marked type's name as `Provider::Marked(type)`, served once the type is provided, which a check cannot know (`rutis-host check` prints it as such).
- **dylib plugins**: a marked type in a dylib (whose crate depends on `rutis-sdk` only and writes `#[rutis_sdk::rutis::service]`, or `use rutis_sdk::rutis::service`) is served once the plugin loads; the host need not know it. On a version swap the old generation's entry point goes with its services and the new one's appears when it provides them. A rebuild needs no new version: two builds of one plugin keep their types' `TypeId`s, and each value is served with the code of the build that provided it, so both can be loaded at once. A build that drops the marker serves its values by name no more, and names are those of the newest build. The host links `rutis-bridge`.
- **Without rutis-bridge**: serving by name and checking clashes are its work. In a program that does not link it, a marked type is provided by type only, and the first time it is provided the error sink gets one `ErrorKind::NotServedByName` (by default `service calendar is provided by type but not registered by name: depend on rutis-bridge to use it by name and to check clashes`).
- **TypeScript declarations**: `rutis-host types <crate dir> -o services.d.ts` finds every marked type in the crate and in the crates it uses that depend on rutis, and writes their declarations (`Services['calendar']`, the data types, `<Name>Thrown`). To refresh them on every build, write `rutis_bridge::build::Exports::new().typescript("rows/services.d.ts").generate()` in `build.rs` (feature `exports` of the `rutis-bridge` build dependency). Calls need no declarations.

A complete example: [`examples/marked-services`](../../examples/marked-services).

### Third-party types: wrap them in a type of your own

Rust does not let your crate write an `impl` for a type whose source you cannot change (one of another crate). Wrap it in a type of your own crate, mark that `#[rutis::service]`, and have each method you serve call the wrapped one:

```rust,ignore
use rutis::service;

/// The calendar.                          // doc comments go into the TypeScript declarations
pub struct Calendar(pub calendar_services::Calendar);

#[service]                                 // served as "calendar"
impl Calendar {
    /// Add an entry; returns it with its id.
    pub fn add(&self, entry: NewEntry) -> Result<Entry, CalendarError> {
        self.0.add(entry)
    }

    pub async fn get(&self, id: u64) -> Result<Entry, CalendarError> {
        self.0.get(id).await
    }
}

root.provide(Calendar(calendar_services::Calendar::new()))?;
```

- The wrapper is an ordinary marked type, and the rules above apply: its name is by default the wrapper's (`#[service(name = "…")]` renames it); provided inside an instance, it is that instance's own.
- Parameters and results use the dependency crate's types as they are; for the TypeScript declarations the generator follows the `use` items into the dependency crate's source.
- Forward only the methods you serve; the doc comments on the forwarding methods go into the declarations.
- A complete example: [`examples/rust-services`](../../examples/rust-services): a dependency crate's `Calendar` for every row, and its `Notes`, one per `session` instance, with `build.rs` refreshing the declarations on every build.

Listing types in `build.rs` (`Exports::service` / `service_in`), as 0.16 did, is removed in 0.17: see [Migrating to 0.17](../migration-0.17.en.md#the-service-list) for the change.

### Write a `HostDispatch` by hand

In Rust, a service used by name from a plugin is a `dyn HostDispatch` registered under `host_key(name)`; the mark above generates one. For a service whose dispatch is its own business (such as one that hands each row its own object), write it by hand:

```rust
# use rutis::host::CtxHost;
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

# fn demo(root: &rutis::Ctx) -> Result<(), Box<dyn std::error::Error>> {
root.provide_at(host_key("clock"), Arc::new(Clock))?; // host_key returns a Key<dyn HostDispatch>
# Ok(())
# }
```

To give an error a code and data, as a marked service's `Err(e)` has, return `rutis_bridge::exports::failure(name, code, message, data)`: the caller's language throws an error named `name` with `message`, and `code` and `data` (a `serde_json::Value`, or `None`) beside them.

Services provided by plugins also appear in rutis under `host_key(name)`, so Rust plugins can use them by name.

### Call a service by name from Rust

A host, or a test, calls any service provided by name (a marked type, a service of a row of any language, a peer's, a hand-written `HostDispatch`) in one line with `rutis_bridge::session::call_service`, as a row of another language would:

```rust
# use rutis::host::*;
# use rutis::Ctx;
# pub struct Weather;
# #[rutis::service]
# impl Weather {
#     pub async fn today(&self, city: String) -> String { format!("sunny in {city}") }
# }
use rutis_bridge::session::call_service;

# #[tokio::main]
# async fn main() -> Result<(), Box<dyn std::error::Error>> {
# let root = Ctx::root()?;
root.provide(Weather)?;                                          // a #[rutis::service] type
let today: String = call_service(&root, "weather", "today", ("Oslo",)).await?;
assert_eq!(today, "sunny in Oslo");
# root.shutdown().await?;
# Ok(())
# }
```

- The arguments are a tuple (`("Oslo", 3)`; `("Oslo",)` for one, `()` for none) or a list, encoded as a call from another language would be; the result is decoded into the type you name (`serde_json::Value` for any, `()` to drop it).
- The service is the one `ctx` sees by name (`host_key(name)`); a method its service declares `sync` runs on a blocking thread, the others are awaited. A hand-written `HostDispatch` whose `methods()` is `None` counts as all async: its `invoke` then runs on the calling task and must not block.
- What fails is an `Err` (`rutis_bridge::session::Error`): the method's own error with its `code()` and `data()`, a sync method's panic with its message, an unknown method, `ServiceNotFound` when no service of that name is provided (a row that has not run yet: wait for it first, `reconcile` below).
- It calls from no row: a method that takes the calling row's `&Ctx` fails.

### Which row calls

A service that registers things for plugins (tools, routes, listeners) or grants things per plugin needs to know which row calls it. During `invoke`, `session::caller_row()` returns the calling row; tie a registration to that row's `Ctx`, and it goes when the row unloads or restarts, while the runtime keeps running:

```rust
use rutis_bridge::session::{caller_row, native_error, HostDispatch, Reply, Value};
# #[derive(Clone)]
# struct Tools;
# impl Tools {
#     fn add(&self, _row: &str, _name: &str) {}
#     fn remove(&self, _row: &str, _name: &str) {}
# }

impl HostDispatch for Tools {
    fn invoke(&self, method: &str, args: Value) -> Reply {
        let [name]: [String; 1] = rutis_bridge::session::decode_value(args)?;
        if let Some(row) = caller_row() {
            // row.id(): the loader's row id; row.key(): its key in its runtime (a new one per restart);
            // row.scope(): the label it isolates this service under (an instance's inside instances), None outside any.
            self.add(row.id(), &name);
            let (tools, id) = (self.clone(), row.id().to_owned());
            row.ctx()
                .effect(|| async move {
                    tools.remove(&id, &name);
                    Ok(())
                })
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

### Receive plugin definitions

Rows of other languages can hand plugin definitions to Rust services (the [plugin API](plugin-api.en.md#plugin-definitions)). Give the parameter the type `rutis::Definition`: before the method runs, the host reads the definition's declarations once from the runtime that defined it. Mount it with `ctx.plugin(def.at(&ctx, config)?)`:

```rust,ignore
use rutis::{Ctx, Definition};

#[rutis::service]
impl Parts {
    pub fn register(&self, scope: String, def: Definition) {
        self.add(scope, def); // drop it when def.ended() completes
    }
}

// When a session opens, on the session's own ctx
for def in parts.of("session") {
    ctx.plugin(def.at(&ctx, config.clone())?);
}
```

- Each copy runs in the runtime that defined it, like a row of that runtime: `def.at(&ctx, …)` computes the keys for the instance `ctx` is in, isolates shared names per instance, and gates the copy on its declared `inject`. Mounting needs rutis-loader in the tree.
- A plugin definition belongs to the current generation of the row that handed it over (as an argument, the calling row): when that row unloads, reloads or ends, or the runtime's process exits, every copy mounted from it ends first; then `at` fails and `ended()` completes. It can only be mounted inside that row's instance, or deeper.
- One value handed over several times is one definition: the two `Definition`s are equal.
- When a service of another language returns a plugin definition, read it with `rutis_bridge::runtime::returned(&*service, value).await`; it belongs to the row that provides the service.
- `rutis-host types` declares `Definition` parameters and results as the TS plugin type.
- A plugin definition only goes back to the runtime that defined it: a call handing it to a third runtime fails.

## Runtime and loader

```rust,no_run
# use rutis::host::*;
# use rutis::Ctx;
# use rutis_loader::Builtins;
# use serde_json::json;
# #[tokio::main]
# async fn main() -> Result<(), Box<dyn std::error::Error>> {
# let root = Ctx::root()?;
# let mut builtins = Builtins::new();
# builtins.register("weather", Weather::new);
use std::sync::Arc;
use rutis_bridge::runtime::LocalRuntime;
use rutis_loader::runtime::{RuntimeResolver, RuntimeRowsPlugin};
use rutis_loader::{Chain, Layer, LoaderOptions, LoaderPlugin, Patch, ServiceCatalog};

// Absolute paths: each runtime's process starts in a directory of its own (see below).
let app = std::env::current_dir()?.join("app");

// Every service name is shared across languages by name, as in rutis-host. To share only the names
// registered as shared (register_shared, export), call catalog.share_registered_only().
let mut catalog = ServiceCatalog::new();

// Local Node runtime: the directory containing @arcships/rutis-runtime and the package.json used for resolution.
// Events need no configuration: listeners in the runtime register themselves on the host's bus (see "Events" below).
let node = LocalRuntime::node(app.join("node_modules/@arcships/rutis-runtime"), app.join("package.json"));
let node_rows = Arc::new(RuntimeResolver::node(node.handle()).with_catalog(&catalog));
// Local Python runtime: plugin module directory and an interpreter with rutis installed.
let python = LocalRuntime::python(app.join("plugins")).interpreter(app.join(".venv/bin/python"));
let python_rows = Arc::new(RuntimeResolver::modules(python.handle()).with_catalog(&catalog));
let runtimes = [node.handle(), python.handle()];
root.plugin(node);
root.plugin(python);

let loader = LoaderPlugin::new(
    // Asked in order: the Rust plugins registered in `builtins` ("Mount Rust plugins" above), then the runtimes.
    Chain::new().with(builtins).with_shared(python_rows.clone()).with_shared(node_rows.clone()),
    LoaderOptions { catalog, ..LoaderOptions::default() },
);
let handle = loader.handle();
root.plugin(loader).await?;
root.plugin(RuntimeRowsPlugin::new(node_rows));
root.plugin(RuntimeRowsPlugin::new(python_rows));

let rows: Vec<Patch> = serde_json::from_value(json!([{ "insert": [
    { "id": "forecast", "name": "weather", "config": { "city": "Oslo" } },   // the Rust plugin in builtins
    { "id": "web", "name": "weather-plugin", "config": { "city": "Oslo" } },
    { "id": "llm", "name": "py:llm_gateway" }
] }]))?;
// Wait for the runtimes before reconciling: only then does the report have
// how their rows loaded (otherwise those rows wait for their runtime first).
for runtime in &runtimes {
    runtime.ready().await;
}
// Returns once every row has been loaded: see below.
let report = handle.reconcile(vec![Layer::new("app", rows)], None).await?;
for failure in &report.failures {
    eprintln!("{failure}");                                                  // "<row id>: <error>"
}
# root.shutdown().await?;
# Ok(())
# }
# #[derive(serde::Deserialize, schemars::JsonSchema)]
# pub struct WeatherConfig { pub city: String }
# pub struct Weather;
# impl Weather {
#     pub fn new(_: &WeatherConfig) -> rutis::Result<Self> { Ok(Weather) }
# }
# impl rutis::Plugin for Weather {
#     type Deps = ();
#     async fn apply(&self, _: &Ctx, _: ()) -> rutis::Result { Ok(()) }
# }
```

- `reconcile` returns once every row has been through its load, as `view.await` does for each: a row whose services are there has run its plugin's `apply` (in its runtime, for a row of another language), or failed, and is in the report's `failures`; a row waiting for a service no row provides yet waits on, and `Loader::waiting_for(&entry)` says what for. A runtime still starting (one waiting for its debugger, say) is not waited for: its rows wait for it, the others start, and the report has nothing yet of how its rows load; for the full report, wait for `RuntimeHandle::ready()` first, as above. `Chain` asks its resolvers in order, so a row name both `builtins` and a runtime know goes to `builtins`.
- Configs: for `Builtins` rows, dylib rows with a `schema:` and rows your own resolver gives a schema with `Resolved::with_schema`, the loader fills the config with defaults and validates it before `build`; a row that does not match fails with `config: …` and is listed in `failures` ([plugin API: configuration](plugin-api.en.md#configuration)). Extra keys are written on stderr by default (`<row>: config: …`), once per row and config value; `Loader::on_config_warning(Arc::new(|row, issue| …))` hands them to you instead.
- Paths: each runtime's process starts in a directory of its own (Python and Go: their project; Node: the runtime's package), and is given the paths as written. Relative paths would then be read from that directory, not the application's: give absolute ones, as above.
- A runtime is a plugin. Its process starts with the plugin and exits when the plugin is unloaded. If the process exits unexpectedly, the runtime stops and its rows wait; `RuntimeHandle::state()` reports `Down(reason)`, including why the process ended. The application decides whether to restart it by calling `restart` on the runtime fiber.
- No row is blamed for its runtime ending: a row that was still loading in it waits for it as the others do, it does not fail. The runtime's `Down` reason says how its process ended, and calls in progress to its rows fail saying so.
- Runtime processes do not outlive the application. Dropping its tokio runtime kills them. Killed by a signal it does not handle (Ctrl-C, SIGTERM) or by `kill -9`, it leaves runtimes that unload their rows and exit within `RUTIS_SHUTDOWN_TIMEOUT` (10 seconds by default), and end themselves a second after that when a plugin keeps one busy. A `shutdown_with_timeout` may not finish while a plugin keeps its runtime busy (its rows cannot unload): end what is left with `LocalRuntime::processes().kill()`, or every runtime process with `rutis_bridge::transport::local::kill_processes()`, as rutis-host does.
- A remote runtime (`RuntimePlugin::remote`) whose session ends and comes back has its rows resolved again by the new session before they start, so a plugin upgraded over there brings its new config schema and declarations.
- For the full row format (groups, `isolate`, `inject`, expressions, imperative updates, and persistence), see the [rutis-loader design](../design-rutis-loader-2026-10-02.en.md).
- To pick up plugin code changes, call `RuntimeResolver::invalidate_all()` and then `Loader::reload(row)`. Node and Python both reimport only the plugin's entry module.
- With `RUTIS_TRACE` set, each runtime-channel message produces a line on stderr with its direction and length, but not its contents.
- A runtime process gets no standard input, and writes to the application's standard output and error. A plugin that reads the terminal (a terminal UI, Python's `input()`) needs the terminal itself, since it checks that its input is one and sets the terminal's modes: give its runtime this process's input with `stdin(Stdio::Inherit)`. The setting covers the whole runtime process, so every plugin in it can read the input. Load such a plugin into a runtime of its own (`named`), and give the terminal to one runtime at most: processes reading the same terminal race for each key. `stdout(Stdio::Null)` and `stderr(Stdio::Null)` keep the other runtimes from writing over it. The same settings exist on `Launcher`.

```rust,no_run
# use rutis_bridge::runtime::LocalRuntime;
# let app = std::path::PathBuf::from("/srv/app");
use rutis_bridge::runtime::Stdio;

// A terminal UI in its own runtime, reading the keyboard; the plugins' runtime writes no output over it.
let tui = LocalRuntime::node(app.join("node_modules/@arcships/rutis-runtime"), app.join("package.json"))
    .named("tui")
    .stdin(Stdio::Inherit);
let node = LocalRuntime::node(app.join("node_modules/@arcships/rutis-runtime"), app.join("package.json"))
    .stdout(Stdio::Null);
```

### Go runtimes

Go plugins are compiled binaries, one runtime per binary (feature `go` of `rutis-loader`). `GoResolver` reads binaries' manifests (`<binary> --rutis-manifest`) and routes `go:<plugin>` to the binary that has it; `GoRuntimes` starts a binary's runtime when a row uses it and stops it once idle:

```rust,no_run
# use std::sync::Arc;
# use std::time::Duration;
# use rutis::host::*;
# use rutis_loader::ServiceCatalog;
# #[tokio::main]
# async fn main() -> Result<(), Box<dyn std::error::Error>> {
# let root = rutis::Ctx::root()?;
# let catalog = ServiceCatalog::new();
# let app = std::env::current_dir()?.join("app");
use rutis_loader::runtime::go::{GoBinaries, GoResolver, GoRuntimes};

let go = Arc::new(
    GoResolver::new(GoBinaries::new().dir(app.join("plugins/go")).file(app.join("bin/netkit")))
        .with_catalog(&catalog)
        .reserve(["node", "py"]),              // other runtimes' names, which no Go runtime may take
);
// In the loader's Chain: Chain::new().with_shared(go.clone())…; after mounting the loader:
let go_runtimes = GoRuntimes::new(go, &app).idle(Some(Duration::from_secs(60)));
let control = go_runtimes.handle();            // restart("go-netkit"), runtimes()
root.plugin(go_runtimes).await?;
# let _ = control;
# Ok(())
# }
```

- A runtime is named after its file (`netkit` → `go-netkit`); when two binaries have a plugin of the same name, the row is `go-netkit:<plugin>`.
- While a process runs, rows resolve from the manifest it started with; a replaced binary takes effect after `control.restart("go-netkit")`.
- A crashed runtime is not restarted automatically by default. After `control.supervise(RestartPolicy::OnFailure, |end| …)` (`rutis_loader::runtime::binary::{RestartPolicy, RuntimeEnd}`), a runtime whose process ended on its own starts again after a wait (1 s, doubling each time up to 30 s, back to 1 s after 60 s of running; after 5 restarts within 5 minutes one more end gives up), each runtime counted on its own; `RestartPolicy::Exit` only reports the end to the callback (`RuntimeEnd::Exit`), for the application to stop. `end.line()` is the line rutis-host prints; `control.unsupervise()` stops supervising, and a restart waiting out its wait does not happen. A runtime that ended before the first `supervise` (it crashed while the application was starting) is handled by `supervise` all the same, only its latest end kept; an application that will not supervise calls `control.unsupervise()` once, and those ends are no longer kept. A binary whose handshake was refused (an old SDK) did not end on its own and is not restarted.
- Compiled languages other than Go (Swift, later) share the implementation: `BinaryResolver<L>` and `BinaryRuntimes<L>` of `rutis_loader::runtime::binary`, where `L` implements `BinaryLanguage` (the row prefix, the SDK's marker, how a binary starts); `GoResolver` is `BinaryResolver<Go>`.

- To run one fixed binary, it also works as Python does: `LocalRuntime::go(binary, project)` with `RuntimeResolver::modules` (rows `<runtime name>:<plugin>`).

## Installing plugins

The `install` feature of `rutis-loader` (off by default) is the installing code `rutis-host add` / `remove` / `upgrade` use: an embedding host does its own "install a plugin" with it, without knowing the plugin's language. The languages it installs are the language features enabled with it (`node`, `bun`, `python`, `go`, `swift`).

```rust,no_run
use rutis_loader::install::{Installer, Places};

# async fn example(home: std::path::PathBuf) -> Result<(), rutis_loader::install::Error> {
let installer = Installer::new(Places {
    node: Some(home.join("plugins/node")),    // a Node project (npm init -y without a package.json)
    python: Some(home.join("plugins/.venv")), // a Python environment (created when missing, with rutis of the same version)
    go: Some(home.join("plugins/go")),        // a directory of Go binaries
    ..Places::default()
});
let about = installer.describe("npm:@ana/wordcount").await?;  // before installing: each plugin's record and RowSchema
let added = installer.add("pypi:bo-llm").await?;              // installed: an Installed per plugin
for plugin in &added {
    println!("{}: row name {}", plugin.name, plugin.row_name()); // llm: row name py:llm
}
installer.upgrade(&added[0], None).await?;                    // the newest version, or the one given
installer.remove(&added[0]).await?;                           // uninstalls the whole package
# let _ = about;
# Ok(())
# }
```

- References are strings, written as for `rutis-host add` (`npm:`, `pypi:`, a Go module path, a local path, with `@version` or not); `add` / `describe` return the error of one that does not read.
- An `Installed` is a record (`from` only; serializable: `rutis-host`'s `installed` is made of them) with the plugin's `name` and `language`; `row_name()` is the plugin's row name (Node as it is; `bun:`, `py:`, `go:`, `swift:` and the plugin's name), which records are keyed by; `Installed::of_row(row name, from)` reads one back.
- The records are the caller's: the API reads and writes no configuration file, and touches no loader or runtime. For `add` to refuse a row name another package has, hand it the records with `Installer::with_installed(records)`.
- `Places.base` is where relative local paths start (by default the current directory); `Places.python` is an environment directory or an interpreter.
- Errors are `install::Error`: `Failed` (a package manager failed; its output was shown), `NoPlace` (`Places` has no place for the language), `Other`. What a package manager installed before a failure stays; the error gives the command that uninstalls it.
- The package managers (npm, bun, uv or pip, go) run as commands only when called, their output shown as it is; `add`, `upgrade` and `describe` may run the package's install scripts and import the plugin's code to describe it: install only plugins you trust.

## Debugging

The host is an ordinary Rust program, and the Rust plugins compiled into it and the dylib plugins run in its process: run the host in an IDE, `rust-lldb` or `rust-gdb`, and breakpoints and call stacks work as usual.

Three things show what happens in the host:

- **Default output**: a load that fails, a dependency that does not come for a long time, and a language runtime that exits are written to standard error by default, each line beginning with the plugin, row or runtime name, with no `[rutis]` prefix (it can be turned off, see [default output](#default-output)). `shutdown_with_timeout` running out names the plugins and cleanups not finished.
- **Current state**: `Loader::status()` returns a snapshot: each row's state and why, which plugin provides each service, each runtime (its state, why, and a local runtime's pid), and the calls in progress. It implements `Display`, one item a line, ready to print (the format: [rutis-host guide, "Seeing the current state"](rutis-host.en.md#seeing-the-current-state)). Under `rutis-host`, `rutis-host status` prints the same. The runtimes come from the loader's resolvers: `RuntimeResolver` and the Go and Swift `BinaryResolver` list theirs; a `Resolver` of your own lists its runtimes by implementing `runtimes()`. Calls are recorded on the runtime's side: calls within one runtime are not seen. `rutis_bridge::runtime::RuntimeState` implements `Debug`.
- **Kernel diagnostics**: `root.diagnostics()`'s `Display` is one item a line: the plugins (their state, what they wait for) and the services (who provides them), by the plugins' and rows' names, without rutis's own plugins and the root by default; `{:#}` lists those too. Its fields (`PluginDiagnostics::platform`, `row` and the rest) are for programs.

The language runtimes (Node, Bun, Python, Go) are processes of their own, each debugged with its own debugger: run the plugin project with `rutis-host dev --debug` (see the [TypeScript](typescript-plugin.en.md#debugging), [Bun](bun-plugin.en.md#debugging), [Python](python-plugin.en.md#debugging) and [Go](go-plugin.en.md#debugging) guides), or attach the debugger to the running process by the pid in the state.

rutis has no `tracing` / `log` integration; the host's own logging works as usual. The environment variable `RUTIS_TRACE=1` prints each message crossing a runtime channel (its direction and length only) on standard error.

## Nodes

```rust,no_run
# use rutis::host::*;
# #[tokio::main]
# async fn main() -> Result<(), Box<dyn std::error::Error>> {
# let root = rutis::Ctx::root()?;
# let token = std::env::var("RUTIS_TOKEN")?;
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
# Ok(())
# }
```

For rutis-loader configuration, `rutis_loader::peer::register_peer_node` registers the `rutis-bridge/peer` row (the same format as `rutis.json`), and `rutis_loader::peer::PeerResolver` resolves `peer:` rows. For a remote runtime, add `RuntimePlugin::remote(name)` and a node row whose `runtime` is the same name; `RuntimeResolver::modules(handle)` resolves row names as `<name>:<module>`. See [Connect nodes](nodes.en.md) for concepts and security.

## Events

The host's Rust plugins, the plugins in every runtime (any language) and the listeners on linked nodes are on one logical bus ([plugin API: events](plugin-api.en.md#events)). Runtime listeners register themselves and the host side configures nothing: what `ctx.emit` and the other modes send reaches listeners in every language, and what they emit reaches the Rust listeners here. How plugins write it: [write a Rust plugin: events](rust-plugin.en.md#events); a host uses the same methods on its root `Ctx`.

An event type is named across languages by `Event::NAME` and its arguments are serde-encoded, so it and its `Value` derive `Serialize` / `Deserialize`. A tuple struct of two or more fields is that many arguments, so `Created` below is `emit("session/created", id, size)` in other languages:

```rust
use rutis::events::Mode;
use rutis::host::*;
use rutis::Event;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
struct Created(String, u32);
impl Event for Created {
    const NAME: &'static str = "session/created";
    type Value = String;
}

# async fn demo(ctx: &rutis::Ctx) -> Result<(), Box<dyn std::error::Error>> {
let off = ctx.on::<Created>(|event, call| async move {                // the listener is called with this ctx
    if call.mode() == Mode::Waterfall {
        // middleware: run the rest of the chain, then rewrite its result
        let inner = call.next().await?;
        return Ok(inner.map(|text| format!("[{text}]")));
    }
    Ok(Some(format!("audited {}", event.0)))                          // Ok(None) is no result
})?;
ctx.on_sync::<Created>(|event, _| Ok(Some(format!("sync {}", event.1))))?;

ctx.emit(Created("s1".into(), 3))?;                                   // returns at once
ctx.parallel(Created("s1".into(), 3)).await?;                         // waits for all; ListenersFailed
let first: Option<String> = ctx.serial(Created("s1".into(), 3)).await?;
let out = ctx
    .waterfall(Created("s1".into(), 3))
    .terminal(|e| async move { Ok(Some(format!("saved {}", e.0))) })  // optional: no terminal
    .await?;
let sync = ctx.bail(Created("s1".into(), 3));                         // meets the async listener above: AsyncListener
let chain = ctx.waterfall_sync(Created("s1".into(), 3), |e| Ok(Some(e.0.clone()))); // a host method: a synchronous waterfall
let python_only = ctx
    .filter(|l| l.runtime.as_deref() == Some("py"))                   // ListenerInfo
    .serial(Created("s1".into(), 3))
    .await?;
off.dispose().await?;
# let _ = (first, out, sync, chain, python_only);
# Ok(())
# }
```

| Topic | Rule |
| --- | --- |
| Listeners | A listener registered once can be called by all five modes; `call.mode()` is this call's mode, `call.next()` works only in a `waterfall` (otherwise `WaterfallNext`). `on_sync` registers a synchronous listener, callable by every mode but unable to call `next` in an asynchronous `waterfall`; an (asynchronous) `on` listener cannot be called by `bail` / `waterfall_sync` (`AsyncListener`) |
| Options and prefixes | `ctx.listen::<E>()` followed by `.prefix("session/")` (every name under the prefix; `call.name()` is the actual one), `.name(n)`, `.once()`, `.prepend()`, `.global()`, then `.on(…)` or `.on_sync(…)` |
| By name | Without a type, listen with `rutis_bridge::events::Args`: `ctx.listen::<Args>().name("file-changed")`, arguments `Args(vec![Value::Data(json!(…))])`, results `ArgValue`. Typed and `Args` listeners of one name receive each other's events, converted through serde. An event type whose value decides its name has an empty `NAME` and overrides `Event::name` |
| Scope | The emitter's scope is `ctx.scope()`: the loader marks each group instance as a scope. Events are delivered by name to every listener; a scope is a descriptor for filters (`ListenerInfo::scope`) |
| Failures | A failing `emit` listener reaches the error sink as `ErrorKind::ListenerFailed` (`Ctx::root_with_sink`), and `rutis_bridge::events::report_line` gives the line rutis-host prints; `parallel` gives `ErrorKind::ListenersFailed` (registration order); errors of listeners elsewhere arrive with their name and message |
| Unload and cancellation | When a plugin unloads, its listeners leave the bus, running calls see `ctx.cancelled()`, and the unload waits for them (up to the shutdown deadline). Dropping the future of `parallel` / `serial` / `waterfall` cancels the dispatch |
| Framework events | `rutis_loader::RowEvent` (`rutis/row`), `rutis_bridge::events::{ServiceEvent, RuntimeEvent}` (`rutis/service`, `rutis/runtime`): listen with `ctx.on`; emitting `rutis/*` fails with `ReservedEvent` |
| Nodes | `Features { export, import }` of `PeerPlugin` select events too (names or `*` prefixes), see [nodes](#nodes) and [connecting nodes](nodes.en.md#events-between-nodes) |

Tests need no separate tool: in a `#[tokio::test]`, take `Ctx::root()` (or `Ctx::root_with_sink` to collect `emit` failures), load the plugins, and emit and assert as above ([write a Rust plugin: test](rust-plugin.en.md#3-test)). Listeners in runtimes need a real runtime (`LocalRuntime`), as in production.

## Exported services: reference

Rust services, used by name from the rows of every language (TypeScript, JavaScript, Python, Go): for a type marked `#[rutis::service]`, the macro generates the dispatcher at compile time and providing the type serves it ([above](#mark-the-type-rutisservice)); a type whose source you cannot change is wrapped in a type of your own and marked ([above](#third-party-types-wrap-them-in-a-type-of-your-own)). The rules below say what crosses and how errors are thrown. The generator writes the TypeScript declarations from the source: `rutis-host types`, or `rutis_bridge::build::Exports` in `build.rs` (feature `exports` of the build dependency). Examples: [`examples/marked-services`](../../examples/marked-services), [`examples/rust-services`](../../examples/rust-services).

| `Exports` | Effect |
| --- | --- |
| `.typescript(file)` | Write the declarations to `file` (relative to `Cargo.toml`), only when they change |
| `.declared(path, typescript)` | The TypeScript type to use for the Rust type at `path`, where the generator cannot derive it |
| `.generate()` | Write the declarations from `build.rs` (after `.typescript(…)`); Cargo runs it again when the source changes |
| `.declarations(crate dir)` | The declarations, outside a build script (what `rutis-host types` uses) |

### Where the source comes from

The generator finds the marked types in the crate and in the crates it uses that depend on rutis. The `path` of `.declared` is written as the application writes it: `crate::…` for its own types, a dependency's crate name (as `Cargo.toml` names it, renames included) for a dependency's. The generator runs `cargo metadata` (for the target being built, offline first) to find each crate's entry file and the features Cargo resolved, and reads the module tree with syn: `mod` files (`#[path]` included), `use` items, globs and `pub use` re-exports (a type re-exported from a private module is found), type aliases (generic ones too). The methods are the public `&self` methods of the type's marked inherent `impl` blocks (but those marked `#[service(skip)]`). `#[cfg]` is evaluated: `feature = "…"` from the features Cargo resolved, target `cfg`s from `CARGO_CFG_*`, `test` and `doc` as off. Stable Rust only: nothing is compiled or expanded for this.

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

Arguments are decoded with serde (any `Deserialize`), results encoded with serde (any `Serialize`); the compiler checks every call the macro generates against the real signatures.

**Functions passed by reference:** the Rust side gets a function that calls the caller's: its future resolves once the function returned and the Promise it returned settled, so a subscriber has handled an event before the native side goes on; a throw or rejection is the future's `Err` (`E: From<rutis::host::CallError>`, which `rutis::Error`, `anyhow::Error` and boxed errors are), so it takes the native failure path. What the function returns where Rust expects `()` is ignored.

**Cancellation:** a call the caller stops waiting for (an `AbortSignal`, a dropped future) drops the method's future on the Rust side, as for every call.

**The calling row's context:** a parameter `&rutis::Ctx` (or `Ctx`) is not a parameter in TypeScript: it is the context of the row making the call (`rutis_bridge::session::caller_row()`), so what the method registers on it (`ctx.effect`, a listener) is that row's and goes when the row unloads or restarts, for example a subscription. Called from outside any row (from Rust, from a peer node), such a method fails, saying it needs the calling row's context.

### Errors

A method returning `Result<T, E>` (`std::result::Result`, an alias of it, `anyhow::Result`, `io::Result`) throws `Err(e)` in the caller's language: a JavaScript `Error`, a Python `rutis.RemoteError`, a Go `*rutis.RemoteError`. It is not an unclassified call error:

| `E` | `name` | `code` | `data` |
| --- | --- | --- | --- |
| An enum with serde | its type name | its variant as serde writes it: the string it serializes to, the one key of an externally tagged variant, the `tag` field of an internally or adjacently tagged one (except an internally or adjacently tagged variant given its own `rename`: see [Limits](#limits)) | its serde value |
| An enum without serde (or `untagged`) | its type name | the variant's Rust name (from its derived `Debug`) | its serde value if it has one |
| A struct with serde | its type name | the string it serializes to, else its type name | its serde value |
| `String` | `Error` | `Error` (the string is the `message`) | — |
| `rutis::Error`, `rutis::host::ErrorKind` | `CordisError` (the name on the wire is unchanged) | the `ErrorKind` variant (`PluginFailed` for an error from plugin code) | — |
| `rutis_bridge::session::Error` | thrown as it is (a call it forwarded keeps the far end's error) | | |
| Anything else (`anyhow::Error`, `io::Error`) | its type name | its type name | — |

`message` is always `e`'s `Display`. JavaScript reads `error.code` and `error.data`; Python `error.code`, `error.data`; Go `remote.Code`, `remote.Data` (big integers as `*big.Int`); Rust `Error::code()`, `Error::data()`. They cross in the error graph, so a runtime that does not know them still gets the name and message. In the declarations each error type has a `<Name>Thrown` type, `RustError<'Name', 'Code1' | 'Code2', Data>`, named in the method's JSDoc (`@throws {CalendarErrorThrown}`).

### Declarations

The file `rutis-host types` or `.typescript(…)` writes has an interface per service type, the data types it uses, `RustError`, the `<Name>Thrown` types, and `Services`, the services by name: `ctx.use<Services['calendar']>('calendar')`. It passes `tsc --strict` (both examples check it). Commit it: `build.rs` rewrites it when the Rust side changes; with `rutis-host types`, run it again.

### Limits

- The generator does not see what a macro writes: types, `impl` blocks or methods written by a macro (other than derives) are not found. A service type must be a concrete struct or enum (not generic, not a trait object).
- Methods taking `&mut self`, borrowing a reference out, taking a reference to a function's argument (`Fn(&A)`), a generic parameter other than a function, `impl Trait` other than `Fn`, or a function that does not return a future of a `Result` cannot be bound: the macro fails the compile ([above](#mark-the-type-rutisservice)), and the generator leaves them out of the declarations with a warning.
- A data type needs a serde derive the generator can read: a hand-written `Serialize`, `#[serde(with / serialize_with / into / remote)]`, or a type of a crate not read (other than `std`, `serde_json`, `chrono`, `uuid`, `url` and the tagged values) needs `.declared(path, typescript)`: until then the methods using it are left out of the declarations (calls are not affected), the warning naming the type.
- `#[cfg]` is evaluated as above; a `cfg` the generator cannot see is taken as off.
- A method's future must be `Send` (the compile fails otherwise).
- The macro sees only the `impl` block: decisions that depend on a type (is it a `Result`, how its error gets a `code`, does a parameter take a function) are left to the expanded, concrete code, so type aliases are recognized. Write an async method as `async fn`, or return `BoxFuture<'_, T>`, `Pin<Box<dyn Future<Output = T> + Send>>` or `impl Future<Output = T>`; an alias of a future as the return type is a compile error. A `&Ctx` parameter is recognized by the name `Ctx`.
- An error's `code` is taken from the value when it is thrown, by the table above; for an internally or adjacently tagged enum whose variant `#[serde(rename = "...")]` gives a name unrelated to its Rust one, the `code` is the Rust name.
- The generated code depends on `rutis` only (a dylib plugin writes `#[rutis_sdk::rutis::service]` or `use rutis_sdk::rutis::service`, and the code then goes through `rutis_sdk::rutis`: the path the marker is reached by decides, so a plugin built against an sdk-bundle, with no dependency written, works too; another path: `#[service(crate = "my::rutis")]`). Values are encoded by the `rutis-bridge` the host links.
