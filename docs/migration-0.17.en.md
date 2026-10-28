# Migrating to 0.17

[中文](migration-0.17.md)

In 0.17 only the public Rust API needs migrating: a plugin is written one way, what hosts use moved to `rutis::host`, the event methods moved to `Ctx`, and there is one error type, `rutis::Error`. These changes add nothing and change no behaviour: code that compiles after migrating runs as it did on 0.16. The [plugin definitions](guide/plugin-api.en.md#plugin-definitions) 0.17 also adds need no change to existing code.

Only projects that write Rust plugins or embed rutis in Rust are affected. TypeScript, Python, Go, Swift and Kotlin plugin code and `rutis.json` are unchanged; the protocol only adds the capability `plugins` for plugin definitions and connects with 0.16 as before. Dylib plugins are rebuilt with the new SDK, as always.

Where every old name went: the design's [appendix A](design-rust-api-convergence-2026-10-24.en.md#appendix-a-where-names-go); the new way to write things: [write a Rust plugin](guide/rust-plugin.en.md) and [embed rutis in a Rust application](guide/rust-host.en.md).

## Step 1: run the migration tool

```bash
cargo install rutis-migrate --version 0.18.0
rutis-migrate 0.17 path/to/crate            # shows the diff only
rutis-migrate 0.17 path/to/crate --write    # writes the files, then runs cargo check
```

`rutis-migrate` is no longer released from 0.19 on; 0.18.0 is the last version.

- It changes the `.rs` files under `PATH` that use rutis (skipping `target`, hidden directories and `node_modules`). The syntax tree only locates the edits; they are made on the original text, so comments and formatting stay.
- Without `--write` it prints a diff and changes nothing. With `--write` it writes the files, then runs `cargo check --all-targets` in `PATH` and lists the errors left (`--no-check` skips it).
- Where it cannot rewrite something, it inserts a `// TODO(rutis 0.17): …` line saying what to do; the compile errors left are next to these comments. `grep -rn "TODO(rutis 0.17)"` lists them all.
- A file already in the 0.17 style is left alone (only names moved off the rutis-loader root get their module paths), so it can run again.

What it rewrites:

| 0.16 | 0.17 |
| --- | --- |
| `CordisError` in type position | `rutis::Error`; `Arc<CordisError>` and variant paths (`CordisError::Closed`) are `host::ErrorKind` |
| `.map_err(\|e\| CordisError::PluginFailed(Box::new(e)))?` | `?`; `CordisError::PluginFailed(e.into())` becomes `Error::from(ErrorKind::PluginFailed(e.into()))` (`e` may be a string, which `?` does not take) |
| `impl Plugin` | `impl host::DynPlugin` (the same trait, renamed), `injects` returning `Vec<TypeKey>` |
| `Box<dyn Plugin>`, `Arc<dyn Plugin>`, `P: Plugin`, `impl Plugin` in argument position | `DynPlugin` |
| `impl TypedPlugin` | `impl Plugin`, `apply` as an `async fn`, a final `Ok(Effect::Done)` as `Ok(())`; `Typed::new(p)` as `p` |
| `ctx.effect(\|\| Effect::AsyncDisposer(…))`, `Effect::Disposer(…)` | `ctx.effect(\|\| async move { … })` |
| `ctx.events().on(&ctx, &key, \|c, e, call\| Box::pin(async move { … }))`, and `on_sync`, `on_opt`, `on_pattern`, `once` | `ctx.on::<E>(\|e, call\| async move { … })`, `ctx.on_sync`, `ctx.listen::<E>()…`; a `c` in the block becomes `call.ctx()` |
| A hand-written `impl Listener<E>` struct | an `async fn on_event(…)` method of the struct, registered through a closure calling it |
| `match err { CordisError::X => … }`, `matches!(err, …)` | `match &*err { ErrorKind::X => … }`: an `Error` dereferences to the `ErrorKind` the host sees |
| `events.emit(&ctx, &key, Arc::new(v))`, and `parallel`, `serial`; `bail_sync`; `waterfall(&ctx, &key, &e, t)` | `ctx.emit(v)` and so on; `ctx.bail(v)`; `ctx.waterfall(e).terminal(t)` |
| `provide_as(TypeKey::of::<T>(), arc)`, `get_as(TypeKey::of::<T>())` | `provide_as::<T>(arc)`, `get::<T>()`; with a named key, `provide_at`, `get_at`, `require_at` |
| `ServiceKey`, `DepKey` | `TypeKey`, `host::Key` |
| Names moved off the `rutis` root | `rutis::host::…`, `rutis::host::diagnostics::…`, `rutis::events::…`; a file calling host methods gets `use rutis::host::CtxHost;` |
| Names moved off the `rutis_loader` root | their module paths (`rutis_loader::report::…` and so on, see [rutis-loader](#rutis-loader) below) |
| `builtins.register_fn(name, \|c\| Ok(Box::new(P::new(c))))` | `builtins.register(name, \|c\| Ok(P::new(c)))` |

Where it leaves a TODO: a cleanup returning `Effect::Many`, a cleanup that `apply` returns, a key or prefix whose event type it cannot find, a listener struct it cannot rewrite, `Gate` / `Keyed` / `KeyedGate` / `Typed::with_keys`, a `register_with` that builds a factory per copy (see [rutis-loader](#rutis-loader)), an event sent under a name chosen at run time (see [events](#events)), and an untyped `TypeKey` given to `provide_at`. A `rutis::Error` put where an `Arc<ErrorKind>` is expected gets no TODO; the compile error points at it, and [errors](#errors) below says what to write.

## Then by hand

### Plugins

After the tool, an old plugin runs as before, as a `host::DynPlugin`. To make it a new `Plugin`: move the types listed in `injects()` into `type Deps`, turn the matching `require` calls in `apply` into its parameter, and register the `Effect` that `apply` returned with `ctx.effect` in `apply`:

```rust
// 0.16
impl Plugin for Weather {
    fn name(&self) -> &str { "weather" }
    fn injects(&self) -> &[TypeKey] { &self.injects } // [TypeKey::of::<dyn Llm>()]
    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, CordisError>> {
        Box::pin(async move {
            let llm = ctx.require_as::<dyn Llm>(TypeKey::of::<dyn Llm>())?;
            let file = std::fs::read_to_string(&self.path)
                .map_err(|e| CordisError::PluginFailed(e.into()))?;
            ctx.provide(Forecast::new(llm, file))?;
            Ok(Effect::Done)
        })
    }
}

// 0.17
impl Plugin for Weather {
    type Deps = Arc<dyn Llm>;

    async fn apply(&self, ctx: &Ctx, llm: Arc<dyn Llm>) -> Result {
        let file = std::fs::read_to_string(&self.path)?;
        ctx.provide(Forecast::new(llm, file))?;
        Ok(())
    }
}
```

- `name` is optional, by default the last segment of the type name.
- A plugin that depends on services by name (`Keyed`, `Typed::with_keys`) stays a `DynPlugin`: list `Key::<T>::named("x").erase()` in `injects()` and read it with `ctx.require_at(&Key::named("x"))` in `apply` ([depend on services by name](guide/rust-host.en.md#depend-on-services-by-name)). `Gate<T>` becomes an `Arc<T>` that you leave unused.
- A plugin with configuration needs no factory. `TypedPluginFactory`, `TypedFactory`, and a `PluginFactory<C>` written only to build the plugin become a constructor, `fn new(config: &C) -> Result<Self>`, given to `root.plugin_from(Weather::new, config)`, `builtins.register("weather", Weather::new)` or `export_plugin!(id: "weather", build: Weather::new)`. A factory whose dependencies are known only at run time still implements `host::PluginFactory`, mounted with `plugin_with` or `Builtins::register_raw`.

### Errors

```rust
// 0.16
fn load(path: &Path) -> Result<String, CordisError> {
    std::fs::read_to_string(path).map_err(|e| CordisError::PluginFailed(e.into()))
}
match &error {
    CordisError::Closed => println!("closed"),
    other => println!("{other}"),
}
let shared: Arc<CordisError> = Arc::new(error);

// 0.17
fn load(path: &Path) -> rutis::Result<String> {
    Ok(std::fs::read_to_string(path)?)
}
match error.kind() {
    Some(ErrorKind::Closed) => println!("closed"),
    _ => println!("{error}"),
}
let shared: Arc<ErrorKind> = Arc::new(error.into_kind());
```

- `?` converts any `std::error::Error + Send + Sync + 'static` into a `rutis::Error`. A string is not an error type: `Err("…".into())` becomes `Err(Error::msg("…"))`, a `Result<_, String>` becomes `.map_err(Error::msg)?`.
- `err.kind()` returns `Option<&ErrorKind>`: `None` for an error from plugin code (through `?`, or `Error::msg`). `&*err` is the `ErrorKind` the host sees, where a plugin code error is a `PluginFailed`.
- An error several parties hold is an `Arc<ErrorKind>`: the error sink, `take_cleanup_errors()`, the result of awaiting a `FiberView` / `dispose()` / `shutdown()`, the `Arc` members of `ErrorKind`, `LoaderError::Rejected { error }`. From a `rutis::Error`: `Arc::new(err.into_kind())`.
- thiserror: `#[from] CordisError` becomes `#[from] rutis::Error` (`#[error(transparent)]` works as before). anyhow: with rutis's `anyhow` feature, `?` in a host converts to `anyhow::Error`; in plugin code, `.map_err(Error::from_anyhow)?`.

### Events

```rust
// 0.16
struct Audit;
impl Listener<Created> for Audit {
    fn call<'a>(&'a self, c: &'a Ctx, e: &'a Created, _: Call<'a, String>)
        -> BoxFuture<'a, Result<Option<String>, CordisError>> {
        Box::pin(async move {
            let prefix = c.get_as::<Prefix>(TypeKey::of::<Prefix>());
            Ok(Some(format!("{} {}", prefix.map_or("", |p| p.0), e.0)))
        })
    }
}
ctx.events().on_opt(&ctx, &EventKey::<Created>::of(), Audit, EventOptions::default().prepend(true))?;
ctx.events().on_pattern(&ctx, &EventPattern::prefix("session/"), Names)?; // Names: impl PatternListener
ctx.events().emit(&ctx, &EventKey::of(), Arc::new(Created("s1".into(), 3)))?;
let out = ctx.events().waterfall(&ctx, &EventKey::of(), &Created("s1".into(), 3), ()).await?;
let quick = ctx.events().bail_sync(&ctx, &EventKey::of(), &Created("s1".into(), 3))?;

// 0.17
ctx.listen::<Created>().prepend().on(|e, call| async move {
    let prefix = call.ctx().get::<Prefix>();
    Ok(Some(format!("{} {}", prefix.map_or("", |p| p.0), e.0)))
})?;
ctx.listen::<Created>().prefix("session/").on(|_, call| async move {
    println!("{}", call.name());
    Ok(None)
})?;
ctx.emit(Created("s1".into(), 3))?;
let out = ctx.waterfall(Created("s1".into(), 3)).await?;
let quick = ctx.bail(Created("s1".into(), 3))?;
```

- Listeners are closures only, taking `(Arc<E>, Call<E>)`; the old `&Ctx` parameter is `call.ctx()`. `Call` has no lifetime any more, and `call.next()` consumes it. The compiler infers the closure's parameter types; no annotations.
- A hand-written `impl Listener<E>` or `PatternListener<E>` struct (`Audit`, `Names` above): the tool turns its `call` into the struct's own `async fn on_event` and registers a closure calling it; a closure, as above, works too. A closure is called many times: clone what moves into `async move` inside it first: `let audit = Arc::new(Audit); ctx.on::<Created>(move |e, call| { let audit = audit.clone(); async move { audit.run(&e, call).await } })?;`.
- Options: `EventOptions::default().prepend(true)` / `.once(true)` / `.global(true)` become `ctx.listen::<E>().prepend()` / `.once()` / `.global()`. A prefix listener reads the actual name from `call.name()`.
- The sending methods take an `E` or an `Arc<E>`, the type inferred from it; a `waterfall` terminal is `.terminal(|e| async move { … })`, left out when there is none; `waterfall_sync` is the host method `ctx.waterfall_sync(e, |e| …)`.
- Listening under another name: `EventKey::<E>::named(n)` / `dynamic(n)` becomes `ctx.listen::<E>().name(n)`; `rutis_loader::edit::volatile_key(ctx)` now returns the name: `ctx.listen::<VolatileUpdate>().name(volatile_key(ctx)).on(…)`.
- Sending under a name chosen at run time (`EventKey::<E>::dynamic(name)` and the like): give the event type an empty `NAME` and override `fn name(&self) -> &str` to return the value's name, then `ctx.emit(value)` as usual. The tool first writes `rutis::internal::CtxInternal::named(..)` here, with a TODO; `internal` is for rutis's own crates, with no stability promise.

```rust
#[derive(Serialize, Deserialize)]
struct Branch {
    #[serde(skip)]
    name: String, // picks the name only, not an argument
    step: u32,
}

impl Event for Branch {
    const NAME: &'static str = "";
    type Value = ();
    fn name(&self) -> &str {
        &self.name
    }
}

ctx.emit(Branch { name: format!("branch/{i}"), step: i })?;
```

- `EventBus` and `ctx.events()` are no longer for plugins and hosts; the `Ctx` methods above cover them.

### Services by key

```rust
// 0.16
ctx.provide_as(TypeKey::of::<dyn Llm>(), Arc::new(MyLlm) as Arc<dyn Llm>)?;
ctx.provide_as(TypeKey::keyed::<dyn Llm>("openai"), openai)?;
let llm = ctx.get_as::<dyn Llm>(TypeKey::keyed::<dyn Llm>("openai"));
let notes = ctx.require_as::<Notes>(TypeKey::instance::<Notes>(id))?;
ctx.provide_as::<dyn HostDispatch>(host_key("clock"), Arc::new(Clock))?;

// 0.17
ctx.provide_as::<dyn Llm>(Arc::new(MyLlm))?;
ctx.provide_at(Key::named("openai"), openai)?;
let llm = ctx.get_at(&Key::<dyn Llm>::named("openai"));
let notes = ctx.require_at(&Key::<Notes>::of().instance(id))?;
ctx.provide_at(host_key("clock"), Arc::new(Clock))?;
```

- `host::Key<T>` merges the old `Key<T>` and `DepKey<T>`: `Key::of()`, `const fn Key::named("x")`, `Key::dynamic(name)`, `.instance(id)`, and `.erase()` for the `TypeKey`. A key names its value type, so a mismatch does not compile. `provide_at`, `get_at`, `require_at` and `require` are host methods (`use rutis::host::*;`).
- rutis-bridge's functions returning keys now return typed keys: `host_key(n)` and `host_key_in(n, id)` are `Key<dyn HostDispatch>`, `peer_key(&id)` is `Key<Peer>`, `transport_key(k)` is `Key<dyn Transport>`, `identity_key` is `Key<dyn Identity>`, `admission_key` is `Key<dyn Admission>`, `runtime_session_key` is `Key<dyn RuntimeSession>`, `RuntimePlugin::key(n)` is `Key<Runtime>`. Where a `TypeKey` is needed (`injects`, `isolate`, comparing with a `TypeKey`), write `.erase()` or `.into()`.
- `ctx.get::<T>()` now takes trait objects too (`ctx.get::<dyn Llm>()`).

### Cleanups

```rust
// 0.16
ctx.effect(|| {
    let task = tokio::spawn(worker(ctx.cancelled()));
    Effect::AsyncDisposer(Box::new(move || Box::pin(async move {
        let _ = task.await;
        Ok(())
    })))
})?;

// 0.17
ctx.effect(|| {
    let task = tokio::spawn(worker(ctx.cancelled()));
    async move {
        let _ = task.await;
        Ok(())
    }
})?;
```

The closure still runs at once, and the future it returns is the cleanup; the rule that a registration on a stopping plugin is refused and the closure does not run is unchanged. `Effect::Disposer` (a synchronous cleanup) also becomes `async move { …; Ok(()) }`; `Effect::Many` becomes several `effect` calls; `Effect::Done` is not calling `effect`. `Effect` itself is still there, in `rutis::host`, and `DynPlugin::apply` still returns it.

### Hosts

```rust
// 0.16
use rutis::{CordisError, Ctx, FiberState, TypeKey, Typed};

// 0.17
use rutis::host::*;                // Ctx::root, shutdown, require, FiberState, TypeKey…
use rutis::Ctx;
```

- `Ctx::root`, `root_with`, `root_with_sink`, `shutdown`, `shutdown_with_timeout`, `diagnostics`, `isolate`, `scope`, `plugin_with`, `plugin_from`, `require`, `take_cleanup_errors` and the other host methods come from the extension trait `host::CtxHost`; after `use rutis::host::*;` they are called on `Ctx` as before.
- `FiberView`, `FiberState`, `Snapshot`, `PluginId`, `TypeKey`, `InstanceId`, `ErrorSink`, `default_sink`, `CallError`, `BoxFuture`, `Effect` and the like are in `rutis::host`; the diagnostics types in `rutis::host::diagnostics`; `Mode`, `ListenerInfo`, `Scope` and `Filtered` in `rutis::events`.
- There is one `ctx.plugin(…)`, for any plugin: a `Plugin`, a `DynPlugin`, or a `Box<dyn DynPlugin>`.

### rutis-loader

The root keeps the 11 names that assembling a loader takes: `LoaderPlugin`, `LoaderOptions`, `Loader`, `Layer`, `Patch`, `Builtins`, `Chain`, `Resolver`, `ServiceCatalog`, `LoaderError`, `RowEvent`. The rest is grouped by topic:

| Module | Contents |
| --- | --- |
| `report` | `CheckReport`, `ReconcileReport`, `RowInfo`, `RowStatus`, `RowState`, `RowCheck`, `EntryInfo`, `EntryStatus`, `Failure`, `Unavailable`, `InstanceInfo`, `LoaderChanged` and more |
| `edit` | `Edit`, `Editable`, `NewEntry`, `CreateInstance`, `Persist`, `NoPersist`, `PersistError`, `Version`, `apply_edit`, `VolatileUpdate`, `volatile_key`, `volatile_paths` and more |
| `resolve` | `Build`, `Resolved`, `Declarations`, `Declared`, `ScopedFactory`, `Expressions`, `ExprScope` |
| `runtime` | `RuntimeResolver`, `RuntimeRowsPlugin`, `RuntimeRows` and more; `runtime::go`, `runtime::swift` and `runtime::binary` hold each language's `*Resolver`, `*Runtimes`, `RestartPolicy` and so on |
| `peer` | `PeerResolver`, `register_peer_node`, `PeerRows`, `PeerRowsPlugin`, `LoaderCatalog` |

```rust
// 0.16
builtins.register("weather", WeatherFactory);                     // impl PluginFactory<WeatherConfig>
builtins.register_fn::<Value, _>("echo", |_| Ok(Box::new(Echo)));
builtins.register_with("notes", |build| Ok(NotesFactory::new(build.instance("session")?)));

// 0.17
builtins.register("weather", Weather::new);                       // fn new(&WeatherConfig) -> Result<Weather>
builtins.register::<Value, _, _>("echo", |_| Ok(Echo));
builtins.register_with("notes", |build: &Build, config: &NotesConfig| {
    Notes::new(build.instance("session")?, config)
});
```

- `register(name, build)`: `build` is a `Fn(&C) -> Result<P>` with `P: Plugin`; the config's schema is generated from `C` (none for `serde_json::Value`), the dependencies come from `P::Deps`. When the closure's parameter has no type, name `C` with `register::<C, _, _>`.
- `register_fn` is gone; use `register`. `register_raw(name, factory, schema)` still takes a `PluginFactory<Value>`, for a plugin whose dependencies are known only at run time.
- `register_with(name, |build, config| …)` now builds the plugin itself for each copy. An old per-copy factory whose dependencies are known only at run time (a key carrying the instance, say) becomes a `Resolver` of your own, returning `Resolved::new(factory).with_scoped(scoped)`, where `scoped` builds each copy's factory from its `Build`; put it in the `Chain`:

```rust
use std::sync::Arc;
use rutis::host::{BoxFuture, PluginFactory};
use rutis_loader::resolve::{Build, Resolved, ScopedFactory};
use rutis_loader::{LoaderError, Resolver};
use serde_json::Value;

struct NotesResolver;

impl Resolver for NotesResolver {
    fn resolve<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Result<Arc<Resolved>, LoaderError>> {
        Box::pin(async move {
            if name != "notes" {
                return Err(LoaderError::NotFound { name: name.to_owned() });
            }
            let scoped: ScopedFactory = Arc::new(|build: &Build| {
                let factory: Arc<dyn PluginFactory<Value>> = Arc::new(NotesFactory::new(build.instance("session")?));
                Ok(factory)
            });
            Ok(Arc::new(Resolved::new(Arc::new(OutsideInstances)).with_scoped(scoped)))
        })
    }
}
```

`OutsideInstances` is the factory used outside instances (its `build` returning an error will do).

### rutis-sdk (dylib plugins)

```rust
// 0.16
rutis_sdk::export_plugin! { id: "weather", factory: WeatherFactory }

// 0.17
rutis_sdk::export_plugin! { id: "weather", build: Weather::new }
```

`build:` takes the same constructor as `Builtins::register`, its parameter type decoded from the JSON config; `schema:` stays optional. `factory:` remains, for a `PluginFactory<ConfigValue>` whose dependencies are known only at run time. A dylib plugin is built with the SDK of its host anyway, so a 0.17 plugin is simply rebuilt.

### The service list

The service list in `build.rs` is removed ([#647](https://github.com/arcships/rutis/issues/647)): `Exports::service`, `Exports::service_in`, `rutis_bridge::include_exports!`, `ServiceCatalog::export`, `ServiceCatalog::export_all`, and what the generated code used in `rutis_bridge::exports` (`Export`, `ExportPlugin`, `FromCallback` and the like). Since 0.15 a Rust service is served with `#[rutis::service]` ([mark the type](guide/rust-host.en.md#mark-the-type-rutisservice)), and the list was left for types whose source you cannot change. The migration tool does not change these places; the compile errors point at them.

Your own type: mark its `impl` `#[rutis::service]` and delete its line from the list. A type whose source you cannot change: wrap it in a type of your own, mark that, and forward each method you serve ([third-party types](guide/rust-host.en.md#third-party-types-wrap-them-in-a-type-of-your-own)):

```rust
// 0.16
// build.rs
rutis_bridge::build::Exports::new()
    .service("calendar", "calendar_services::Calendar")
    .service_in("notes", "calendar_services::Notes", "session")
    .typescript("rows/services.d.ts")
    .generate()
    .expect("generate the exported services");
// src/lib.rs
rutis_bridge::include_exports!();
catalog.export_all(exports::all());
root.provide(calendar_services::Calendar::new())?;

// 0.17
// build.rs: the declarations only. If they need not follow every build, delete build.rs and use rutis-host types
rutis_bridge::build::Exports::new()
    .typescript("rows/services.d.ts")
    .generate()
    .expect("generate the service declarations");
// src/lib.rs
pub struct Calendar(pub calendar_services::Calendar);

#[rutis::service]
impl Calendar {
    /// Add an entry; returns it with its id.
    pub fn add(&self, entry: NewEntry) -> Result<Entry, CalendarError> {
        self.0.add(entry)
    }
    // …the other methods you serve
}

root.provide(Calendar(calendar_services::Calendar::new()))?;
```

| 0.16 | 0.17 |
| --- | --- |
| `.service(name, path)` | Wrap and mark; a name other than the type's is `#[rutis::service(name = "…")]` |
| `.service_in(name, path, group)` | Provide the wrapper inside the instance: `ctx.provide_at(Key::of().instance(build.instance("session")?), notes)`; no group is written |
| `include_exports!()`, `catalog.export_all(exports::all())`, `Export::plugin` | Delete them: providing serves |
| The generated `exports::DispatchCalendar` and the like | By name: `ctx.get_at::<dyn HostDispatch>(&host_key("calendar"))` |
| `exports::TYPESCRIPT` | `Exports::new().declarations(crate dir)`, or read the file `.typescript(…)` writes |
| `.generate()` alone, without `.typescript(…)` | An error: name the declarations' file with `.typescript(…)` |
| `.declared(path, typescript)`, `.typescript(file)` | Unchanged |

- What crosses, how errors are thrown and the declarations do not change: with the wrapper named as the wrapped type, and the same method names and doc comments, the file `.typescript(…)` writes is the same byte for byte (`examples/rust-services` was migrated so).
- The wrapper is provided, not the wrapped type. Rust code that depends on the wrapped type depends on the wrapper instead (`.0` is the wrapped value).

## What did not change

- Gating, reloads, unloading, cleanups, the five event modes, scopes, filters, and every entry of the [guarantees](guarantees.en.md) but `bridge/exported-services`, withdrawn with the service list: its rules are all in `bridge/marked-services`.
- The protocol and the wire (plugin definitions only add the capability `plugins`); a rutis error thrown by a Rust service is still named `CordisError` in other languages.
- Plugin code in other languages, `rutis.json` and rutis-host.
