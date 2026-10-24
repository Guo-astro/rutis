# Rust API convergence: one way to write a plugin

[中文](design-rust-api-convergence-2026-10-24.md)

Status: design (2026-10-24), #617, target version 0.17. Only the Rust public API changes; the protocol, the other languages' SDKs and runtime behavior stay as they are.

## 1. Goal

> A Rust plugin author learns one way to write a plugin, three service methods, and event methods named as in every other language. The smallest plugin has no `BoxFuture`, no lifetime parameters and no `Effect`. What hosts use goes into `rutis::host`; what rutis's own crates use among themselves goes into the hidden `rutis::internal`. No capability is removed; the default path gets shorter.

Python plugins already work this way: the same gating, reload, cleanup, cross-process calls and five event modes, with 10 methods on `Context`. So the large Rust surface (71 names at the root, 40 methods on `Ctx`, 28 on `EventBus`, four plugin traits) is not caused by crossing languages or processes, but by three fixable problems:

- methods that hosts and rutis's own crates call sit on the objects plugin authors use;
- Rust's language limits were worked around in the clumsiest way, such as an `_opt` twin for every `on*` and a hand-written `BoxFuture`;
- each new capability added a name and nothing was ever merged. Typed dependencies, for example, became a second way to write a plugin, and nobody outside the kernel uses it.

## 2. What a plugin author writes

### The smallest plugin

```rust
use std::sync::Arc;
use rutis::{Ctx, Plugin, Result};

/// A service is a type.
struct Greeting(String);

/// Provides Greeting. What apply registers is withdrawn when the plugin stops.
struct Greeter {
    lang: String,
}

impl Plugin for Greeter {
    type Deps = ();

    async fn apply(&self, ctx: &Ctx, _: ()) -> Result {
        ctx.provide(Greeting(format!("hello from {}", self.lang)))?;
        Ok(())
    }
}

/// Depends on Greeting: starts when it appears, restarts when it is replaced.
struct Printer;

impl Plugin for Printer {
    type Deps = Arc<Greeting>;

    async fn apply(&self, _: &Ctx, greeting: Arc<Greeting>) -> Result {
        println!("{}", greeting.0);
        Ok(())
    }
}
```

Today's README example writes `BoxFuture`, `Box::pin(async move { … })` and `Ok(Effect::Done)`, imports `CordisError`, and makes the reader choose between `Plugin` and `TypedPlugin`.

### Events

```rust
use rutis::serde::{Deserialize, Serialize};
use rutis::{events::Mode, Ctx, Event, Plugin, Result};

#[derive(Serialize, Deserialize)]
struct Created(String, u32); // emit("session/created", id, size) in other languages

impl Event for Created {
    const NAME: &'static str = "session/created";
    type Value = String;
}

struct Audit;

impl Plugin for Audit {
    type Deps = ();

    async fn apply(&self, ctx: &Ctx, _: ()) -> Result {
        ctx.on::<Created>(|event, call| async move {
            if call.mode() == Mode::Waterfall {
                let inner = call.next().await?; // middleware: run the rest of the chain first
                return Ok(inner.map(|text| format!("[{text}]")));
            }
            Ok(Some(format!("audited {}", event.0))) // Ok(None) is "no result"
        })?;
        ctx.on_sync::<Created>(|event, _| Ok(Some(format!("sync {}", event.1))))?;
        ctx.listen::<Created>().prefix("session/").on(|_, call| async move {
            println!("saw {}", call.name());
            Ok(None)
        })?;
        Ok(())
    }
}
```

Events are sent in five modes, named as in the other languages:

```rust
ctx.emit(Created("s1".into(), 3))?;                         // returns at once
ctx.parallel(Created("s1".into(), 3)).await?;               // waits for all
let first = ctx.serial(Created("s1".into(), 3)).await?;     // first result
let out = ctx
    .waterfall(Created("s1".into(), 3))
    .terminal(|e| async move { Ok(Some(format!("saved {}", e.0))) }) // optional
    .await?;
let quick = ctx.bail(Created("s1".into(), 3))?;             // synchronous
let from_python = ctx
    .filter(|l| l.runtime.as_deref() == Some("py"))
    .serial(Created("s1".into(), 3))
    .await?;
```

### Cleanup and background tasks

```rust
impl Plugin for Server {
    type Deps = Arc<dyn Store>;

    async fn apply(&self, ctx: &Ctx, store: Arc<dyn Store>) -> Result {
        let listener = tokio::net::TcpListener::bind(&self.addr).await?; // io::Error through `?`
        ctx.spawn(async move {
            loop {
                let (conn, _) = listener.accept().await?;
                tokio::spawn(serve(conn, store.clone()));
            }
        })?;
        ctx.effect(async move { flush_logs().await })?;
        Ok(())
    }
}
```

### Configuration

Configuration lives in the struct's fields, plus an `fn new(config: &Config) -> Result<Self>`. Whoever mounts the plugin builds it with that function, and rebuilds it with the same function when the configuration changes:

```rust
#[derive(Deserialize, JsonSchema)]
struct WeatherConfig {
    city: String,
}

struct Weather {
    city: String,
}

impl Weather {
    fn new(config: &WeatherConfig) -> Result<Self> {
        Ok(Weather { city: config.city.clone() })
    }
}

builtins.register("weather", Weather::new);                // rutis-loader
rutis_sdk::export_plugin!(id: "weather", build: Weather::new); // dylib plugin
let view = root.plugin_from(Weather::new, config);         // mounted by a host; view.update(new_config)
```

### Every name used

The `rutis` root keeps 9 names:

| Name | What it is |
| --- | --- |
| `Plugin` | The only plugin trait |
| `Deps` | What `type Deps` can be (`()`, `Arc<T>`, `Option<Arc<T>>`, tuples of them); authors do not implement it |
| `Ctx` | See the table below |
| `Event` | An event type |
| `Disposer` | Returned by `provide`, `on`, `effect`, `spawn`; withdraws early |
| `Error`, `Result` | The only error type |
| `service` | `#[rutis::service]`, to offer a service to other languages |
| `serde` | For deriving `Serialize` / `Deserialize` on events |

The types that appear in event signatures live in `rutis::events`: `Call`, `SyncCall`, `Mode`, `ListenerInfo`, `Scope`, the builders `Listen`, `Filtered` and `Waterfall` that `ctx.listen()`, `ctx.filter()` and `ctx.waterfall()` return, and `Listener`, which async listener closures satisfy automatically. The compiler infers all of them; the only one an author normally writes is `Mode`.

`Ctx` has 16 methods for plugins:

| Group | Methods |
| --- | --- |
| Services | `provide`, `provide_as`, `get` |
| Lifecycle | `effect`, `spawn`, `cancelled`, `plugin` |
| Events | `on`, `on_sync`, `listen`, `emit`, `parallel`, `serial`, `waterfall`, `bail`, `filter` |

Compared with Python:

| Task | Python | Rust |
| --- | --- | --- |
| Define a plugin | `define_plugin(apply, inject=[…])` | `impl Plugin`: `type Deps` + `async fn apply` |
| Dependencies | `inject=["llm"]`, `ctx.use("llm")` | `type Deps = Arc<dyn Llm>`, passed to `apply` |
| Provide a service | `ctx.provide("weather", obj)` | `ctx.provide(value)`; trait objects with `ctx.provide_as::<dyn T>(arc)`; the name other languages see comes from `#[rutis::service]` |
| Cleanup | `ctx.effect(cleanup)` | `ctx.effect(async { … })` |
| Background task | an asyncio task | `ctx.spawn(async { … })` (`ctx.Go` in Go, `ctx.launch` in Kotlin) |
| Listen | `ctx.on(name, fn)` | `ctx.on::<E>(…)`, `ctx.on_sync::<E>(…)`, `ctx.listen::<E>()` |
| Send | `emit` / `parallel` / `serial` / `waterfall` / `bail` | same names |
| Filter | `ctx.filter(keep)` | `ctx.filter(|info| …)` |
| Configuration | `apply(ctx, config)` | struct fields + `new(&Config)` |

Python's `use` is replaced by `Deps` in Rust. Of Rust's 16 methods, 7 have no Python counterpart, and each corresponds to a language difference:

- `provide_as`: Rust has to name the trait-object type;
- `get`: an optional read, while Python's `use` only reads declared dependencies;
- `spawn` and `cancelled`: Rust's own way of running and cancelling tasks;
- `on_sync`: Rust cannot tell at run time whether a closure is sync or async;
- `listen`: listening by prefix or under another name;
- `plugin`: a Rust application mounts child plugins itself.

## 3. Rules

### Plugins

```rust
pub trait Plugin: Send + Sync + 'static {
    type Deps: Deps;
    fn apply(&self, ctx: &Ctx, deps: Self::Deps) -> impl Future<Output = Result> + Send;
    fn name(&self) -> &str { /* last segment of the type name */ }
    fn validate(&self) -> Result { Ok(()) }
}
```

- Implementations write `async fn apply` (allowed since Rust 1.75; rutis's MSRV is 1.88).
- `type Deps` is required; write `()` when there are none. Associated type defaults are not stable Rust yet, so this line cannot be left out.
- `name` defaults to the last segment of the type name (`Greeter`); loader rows have their own ids.
- `validate` runs at load, before `apply`; when it fails the plugin is Failed and `apply` does not run.

A plugin author needs to know three things:

1. `apply` runs once every required service in `Deps` is ready; optional ones are not waited for.
2. Everything `apply` registers through `ctx` (services, listeners, cleanups, tasks, child plugins) is withdrawn in reverse order when the plugin stops; when `apply` fails, what it registered so far is withdrawn the same way.
3. When a required service is withdrawn or replaced, the plugin stops first and runs `apply` again once the service is ready.

The six states, exactly-once cleanup and "consumers stop before providers" are unchanged. They are written in [Guarantees](guarantees.en.md) for host authors and for troubleshooting; writing a plugin does not require reading them first.

### Dependencies

| `type Deps =` | Gates on | Passed |
| --- | --- | --- |
| `()` | nothing | `()` |
| `Arc<T>` | `T` | the service |
| `Option<Arc<T>>` | nothing | the service if visible at load; its later arrival or departure does not reload |
| a tuple of up to 8 | all members | each value |

- `T` may be a trait object: `Arc<dyn Llm>`.
- Dependencies are told apart by type only. For two services of the same type, use two newtypes (`struct Primary(Db)`, `struct Replica(Db)`): the type is the name. Services told apart by a run-time name (transports, identities, runtime sessions) belong to the host layer (appendix A).
- `Deps` is a sealed trait; authors do not implement it.
- A dependency withdrawn after the gate opened but before `apply` reads it returns the plugin to Pending, as today.

### Services

- `ctx.provide(value)`: provides it under its own type.
- `ctx.provide_as::<dyn T>(arc)`: provides it under a trait-object type, e.g. `ctx.provide_as::<dyn Llm>(Arc::new(MyLlm))`.
- `ctx.get::<T>()`: an optional read; `T` may be a trait object. It does not gate and creates no reload relation; diagnostics record it as an undeclared access. Use it for "use it if it is there"; services a plugin needs go in `Deps`.
- Today's `require`, reads and writes by name or instance, provides with a `check()` predicate and writable bindings move to `rutis::host` and `rutis::internal` (appendix A).

### Cleanup and background tasks

- `ctx.effect(future)`: runs the future when the plugin stops, in reverse order together with every other registration. Registering it while the plugin is already stopping runs it at once and returns an `Inactive` error, so a resource just acquired is always released. Errors from cleanups are collected as today (`take_cleanup_errors`).
- `ctx.spawn(task)`: runs `task` on the runtime now. When the plugin stops, `task` is dropped at its current `.await`; stopping does not wait for it to finish. Put asynchronous shutdown work into `effect`. An error the task returns goes to the error sink (as an `emit` listener's failure does) and the plugin keeps running.
- `ctx.cancelled()`: a future that completes when the plugin stops, for long-running listener calls.
- `ctx.plugin(child)`: mounts a child plugin, which stops with this one.

Today `ctx.effect(f)` means "run `f` now and use the `Effect` it returns as the cleanup", unlike `effect(cleanup)` in every other language. After this change `Effect` is used only by `host::DynPlugin`.

### Events

- Listening:
  - `ctx.on::<E>(|event, call| async move { … })` registers an async listener;
  - `ctx.on_sync::<E>(|event, call| …)` registers a sync listener, which every mode can call;
  - to listen by prefix or under another name, use `ctx.listen::<E>()` with `.prefix(p)` or `.name(n)`, ending with `.on(…)` or `.on_sync(…)`.
  - The `prepend`, `once` and `global` options live in `host::ListenExt`; after `use rutis::host::*` they chain onto the same builder.
- A listener is a closure taking `(Arc<E>, Call<E>)`; a sync listener takes `(Arc<E>, SyncCall<E>)`. It returns `Result<Option<E::Value>>`, where `Ok(None)` is "no result". The compiler infers the parameter types, and the body can use `?` on any error.
- `call.mode()` is this dispatch's mode; `call.name()` is the name dispatched (for prefix listeners); `call.next()` works only in `waterfall`; `call.ctx()` is the `Ctx` that registered the listener.
- Sending:
  - `emit`, `parallel`, `serial`, `waterfall` and `bail` take `E` or `Arc<E>`;
  - `waterfall` returns a builder: `.terminal(…)` may come before `.await`;
  - `bail` is today's `bail_sync`, renamed to match the other languages;
  - `ctx.filter(pred)` gives the same five methods.
- Names: an event registers and dispatches under `E::NAME`. Event types without a fixed name (rutis-bridge's `Args`, for example) override `Event::name(&self)`, so the value carries its own name; to listen to such an event use `ctx.listen::<Args>().name("file-changed")`.
- The semantics of the five modes, scope, filters and waiting at unload follow the [unified events design](design-events-2026-10-18.en.md), unchanged.

### Errors

- Every signature has one error type, `rutis::Error`, and `rutis::Result<T = (), E = Error>`.
- `?` turns any `std::error::Error + Send + Sync + 'static` into `rutis::Error`, rutis's own errors included. `Error::msg("…")` makes one from a string.
- `rutis::Error` itself does not implement `std::error::Error`; that is what lets `?` accept any error (as with `anyhow::Error` and Bevy's `BevyError`). It converts into `Box<dyn std::error::Error + Send + Sync>`, so hosts use `?` as usual in functions returning `Box<dyn Error>`.
- A host that needs to tell rutis's failures apart calls `err.kind()`, which returns `Option<&host::ErrorKind>`. `ErrorKind` is today's `CordisError` enum, renamed, with the same variants. An error from plugin code has `kind()` `None`; when it reaches a host it is wrapped in `ErrorKind::PluginFailed`.
- Errors still do not implement `Clone`; share them as `Arc<Error>` (D11 and D25 unchanged).

### Three layers

| Layer | For | Documentation | Stability |
| --- | --- | --- | --- |
| `rutis` (root) and `rutis::events` | plugin authors | plugin guide | changes listed in release notes |
| `rutis::host` | applications embedding rutis, tests | host guide | same |
| `rutis::internal` | rutis-bridge, rutis-loader, rutis-dylib, rutis-sdk, rutis-macros, rutis-host | `#[doc(hidden)]` | none; if something outside the repository needs it, open an issue to move it to `host` |

- The rule: what writing a plugin needs stays at the root; what an application embedding rutis needs goes to `host`; what nothing but rutis's own crates uses goes to `internal`.
- `Ctx`'s methods are split by the same rule: the `host` and `internal` methods live in extension traits (`host::CtxHost`, `internal::CtxInternal`) and need `use rutis::host::*` before they can be called. rustdoc lists extension-trait methods in their own section of the `Ctx` page.
- rutis-loader is split the same way: the root keeps the 11 names needed to assemble a loader, the rest move into modules by topic, and what only rutis-host and tests use goes to `internal`.

The full contents of each layer and where every current name goes are in appendix A; migration and implementation steps are in appendix B.

## 4. Trade-offs

- **Add `rutis::prelude` and change nothing else**: another prelude does not remove any of the 40 `Ctx` methods or 28 `EventBus` methods from the documentation. So the root itself serves as the prelude, with no extra module.
- **Make the new style the default and keep the old one**: that adds one more way to do the same thing, and too many ways is exactly the problem #617 describes. So 0.17 makes the whole change at once and keeps no old names (appendix B).
- **A `#[rutis::plugin]` macro**: `async fn` already works in traits, so `BoxFuture` goes away without a macro. A macro would also hide the real trait from authors and make error messages harder to read.
- **Configuration as an associated type, `apply(ctx, config, deps)` (the Cordis / Python shape)**: every plugin would write one more associated type, and a plugin without configuration would be mounted as `ctx.plugin(P, ())`. In Rust, configuration naturally lives in struct fields; rebuilding a plugin when its configuration changes is the mounter's job.
- **One `on` that takes both sync and async closures** (the marker-type trick axum and Bevy use): tried. With two candidate impls the compiler cannot infer the closure's parameter types, so every listener has to be written `|e: Arc<Created>, call: Call<Created>|`. With two methods, `on::<E>(|e, call| async move { …? … })` needs no annotation (both verified on Rust 1.88).
- **`listen` with every option**: no Rust code outside the kernel uses `prepend`, `once` or `global`; Cordis plugins use Cordis's own API and never pass through here. So those three options go to `host`, and the builder plugin authors see has only `prefix` and `name`.
- **One `dispatch(mode, …)` instead of five methods**: the five modes return different things (no wait, aggregated errors, first result, chained, synchronous); a single method either loses the types or returns an enum for the caller to take apart.
- **Keep names in dependencies (`Keyed<T>`, `DepKey<T>`)**: nobody outside the kernel uses them. Writing a name into a type needs `&str` const generics, which stable Rust does not have; a name chosen at run time needs a separate set of keys, which is where `Typed::with_keys` came from. Newtypes cover two services of one type.
- **Make `rutis::Error` implement `std::error::Error` too**: that collides with `impl<E: std::error::Error> From<E>`, so `?` could no longer accept any error and authors would be back to `map_err`.

## 5. Acceptance

Against #617's acceptance list:

| #617 | This design |
| --- | --- |
| A plugin author sees ≤ 15 entry points by default | 9 at the root; the compiler infers the types in `rutis::events` |
| A minimal Rust plugin needs no hand-written `BoxFuture` or lifetimes | the examples in section 2; the README, guides and examples all use this style |
| `EventBus` has ≤ 10 public methods | plugins use the 9 event methods on `Ctx`; `EventBus` moves into `internal` |
| ≤ 5 service provide / read methods | 3: `provide`, `provide_as`, `get` |
| `rutis-loader` has a clear prelude and unstable boundary | 11 names at the root, the rest grouped into modules, `internal` with no stability promise |
| The first page of the docs lists "this is all you need" | new `docs/guide/rust-plugin.md` (write a Rust plugin), one more row on the guide index |

Also:

- `cargo public-api` produces snapshots of the public surface of the `rutis` root, `rutis::events` and `rutis::host`, committed to the repository and compared in CI. Adding a name then means changing the snapshot, visible in review, so the surface cannot grow unnoticed.
- The Rust code in `examples/` and the guides does not mention `rutis::internal`; CI checks this with grep.
- The Rust code in `docs/guide/rust-plugin.md` and the README compiles as doctests, as the development handbook's does today.
- Every pattern in section 2 is a test, including: `?` accepting `io::Error`; listener closures compiling without annotations; `effect` running at once when registered after the plugin started stopping; a `spawn`ed task being dropped when the plugin stops; `DynPlugin` and `Plugin` plugins depending on each other.
- Rerun the event dispatch benchmark ([event dispatch performance](performance-event-dispatch-2026-09-27.en.md)). With an owned `Call`, each call clones one more `Arc`; the PR records the numbers before and after.

## Appendix A: where names go

### `rutis::host`

| Area | Names |
| --- | --- |
| Root and shutdown (`CtxHost`) | `Ctx::root`, `root_with`, `root_with_sink`, `root_view`, `root_ctx`, `view`, `shutdown`, `shutdown_with_timeout`, `is_shutting_down`, `handle`, `error_sink`, `take_cleanup_errors`, `diagnostics` |
| Services by key (`CtxHost`) | `require`, `require_at`, `get_at`, `provide_at`; the key is a `Key<T>` |
| Scopes and instances (`CtxHost`) | `isolate`, `isolate_label`, `instance`, `scope`, `row`, `dispose_self`, `cancellation_token` |
| Mounting (`CtxHost`) | `plugin_from(build, config)` (a typed config closure; dependencies come from `P::Deps`), `plugin_with(factory, config)` |
| Events (`CtxHost`) | `waterfall_sync`, `subscriptions` |
| Listen options (`ListenExt`) | `prepend`, `once`, `global` on `ctx.listen::<E>()` |
| The object-safe plugin form | `DynPlugin`, `PluginFactory`, `Effect`, `EffectMeta`, `EffectPhase`, `BoxFuture` |
| Fibers | `FiberView`, `FiberState`, `FiberStatusChanged`, `PluginId`, `Snapshot`, `DisposeWaitError`, `shutdown_deadline` |
| Keys | `Key<T>`, `TypeKey`, `InstanceId` |
| Errors | `ErrorKind`, `ServiceReadError`, `ServiceReadFailure`, `NextMisuse`, `CallError`, `ErrorSink`, `default_sink` |
| Diagnostics | `host::diagnostics::*` (today's 10 diagnostics types) |
| Event descriptions | `EventSubscription`, `ListenerKind` |

- **`DynPlugin`** is today's `Plugin` trait, renamed, with one difference: `injects()` returns `Vec<TypeKey>` instead of a borrowed slice. rutis provides `impl<P: Plugin> DynPlugin for P`, so there is one `ctx.plugin(…)` and it mounts any plugin. Plugins whose dependencies are only known at run time (rutis-loader's `JsFactory`, for example) and lists of plugins in a `Vec<Box<dyn DynPlugin>>` use `DynPlugin`.
- **`Key<T>`** merges today's `Key<T>` and `DepKey<T>`: `Key::of()`, `const fn Key::named("primary")`, `Key::dynamic(name)`, `.instance(id)`, convertible into `TypeKey`. The key carries the value type, so a type mismatch in `provide_at(key, arc)` is a compile error instead of today's run-time error.
- **`ServiceKey`** is removed; it was an alias of `TypeKey`.

### `rutis::internal`

`EventBus` with `events::{remote, wire}`, `EventKey`, `EventPattern`, `EventOptions`, `DispatchAttempt`, `ServiceIntercept`, `ServiceWriter`, `ServiceWriteError`, `ServiceWriteFailure`, `__sdk_service`, and `CtxInternal`: `events()`, `mark_scope`, `mark_row`, `provide_checked` (was `provide_as_with_check`), `provide_mut_at` (was `provide_mut_as`), `require_provided_as`, `effect_named`, `refresh`, `in_instance_key`.

The 10 `EventBus` methods that only rutis-bridge / rutis-loader use (`on_remote`, `watch_table`, `observe_dispatch`, `emit_framework`, `dispatch`, `dispatch_sync`, `invoke_entry`, `invoke_entry_sync`, `transcoder`, `set_transcoder`) stay here together with `try_filter`, with unchanged signatures. Plugins and hosts no longer get an `EventBus`; `ctx.events().on(&ctx, &key, …)` becomes the `ctx.on…` methods.

### The 71 names at today's `rutis` root

| Today | 0.17 |
| --- | --- |
| `Plugin` | `host::DynPlugin` (`injects` returns `Vec`); the new `Plugin` is the former `TypedPlugin`, minus `BoxFuture` and `Effect` as in section 3 |
| `TypedPlugin` | `Plugin` |
| `Typed` | removed (`impl<P: Plugin> DynPlugin for P`) |
| `PluginFactory` | `host::PluginFactory` (`build` returns `Box<dyn DynPlugin>`) |
| `TypedPluginFactory`, `TypedFactory` | removed; use `plugin_from`, `Builtins::register`, `export_plugin!(build:)` |
| `Deps` | `Deps` (sealed; `Keys` removed) |
| `DepKey`, `Keyed`, `KeyedGate` | removed; named dependencies use `host::DynPlugin` + `host::Key<T>` |
| `Gate` | removed; take an `Arc<T>` and leave it unused |
| `BoxFuture` | `host::BoxFuture` |
| `Effect`, `EffectMeta`, `EffectPhase` | `host::` |
| `Disposer` | `Disposer` |
| `Ctx` | `Ctx` (methods split per section 2 and this appendix) |
| `TypeKey`, `InstanceId` | `host::` |
| `Key` | `host::Key<T>` (merged with `DepKey`) |
| `ServiceKey` | removed (an alias of `TypeKey`) |
| `ServiceIntercept`, `ServiceWriter` | `internal::` |
| `CallError` | `host::CallError` |
| `service` | `service` |
| `CordisError` | `Error`; the enum itself is renamed `host::ErrorKind` |
| `ServiceReadError`, `ServiceReadFailure`, `NextMisuse`, `ErrorSink`, `default_sink` | `host::` |
| `ServiceWriteError`, `ServiceWriteFailure` | `internal::` |
| `EventBus`, `DispatchAttempt` | `internal::` |
| `EventSubscription`, `ListenerKind` | `host::` |
| `Filtered` | `events::Filtered` |
| `Event` | `Event` (adds `fn name(&self)`, returning `NAME` by default) |
| `EventKey`, `EventPattern`, `EventOptions` | `internal::`; plugins use `ctx.listen::<E>()` |
| `Listener` | `events::Listener` (satisfied by async listener closures; not implemented by hand) |
| `SyncListener`, `PatternListener`, `SyncPatternListener` | removed; listeners are closures, and prefix listeners read the name from `call.name()` |
| `Terminal` | removed; use `ctx.waterfall(e).terminal(…)` |
| `Call`, `SyncCall` | `events::Call<E>`, `events::SyncCall<E>` (owned, no lifetime parameter) |
| `Mode`, `ListenerInfo`, `Scope` | `events::` |
| `FiberView`, `FiberState`, `FiberStatusChanged`, `PluginId`, `Snapshot`, `DisposeWaitError`, `shutdown_deadline` | `host::` |
| diagnostics types (`RuntimeDiagnostics` and 9 more) | `host::diagnostics::` |
| `serde` | `serde` |

### The 40 methods on today's `Ctx`

| Today | 0.17 |
| --- | --- |
| `provide`, `get`, `plugin`, `cancelled` | unchanged (`get` accepts trait objects) |
| `provide_as(key, arc)` | `provide_as::<T>(arc)`; with a key, `host::provide_at(Key<T>, arc)` |
| `get_as(key)` | `get::<T>()` without a name; `host::get_at` with a key |
| `effect(f)` | `effect(future)`, see section 3 |
| `require`, `require_as` | `host::require`, `host::require_at` |
| `root`, `root_with`, `root_with_sink`, `root_view`, `root_ctx`, `view`, `shutdown`, `shutdown_with_timeout`, `is_shutting_down`, `handle`, `error_sink`, `take_cleanup_errors`, `diagnostics`, `isolate`, `isolate_label`, `instance`, `scope`, `row`, `dispose_self`, `cancellation_token`, `plugin_with`, `plugin_from` | `host::CtxHost` |
| `events` | `internal::CtxInternal`; the event methods are on `Ctx` |
| `mark_scope`, `mark_row`, `provide_as_with_check`, `provide_mut_as`, `require_provided_as`, `effect_named`, `refresh`, `in_instance_key` | `internal::CtxInternal` (two renamed: `provide_checked`, `provide_mut_at`) |
| new | `spawn`, `on`, `on_sync`, `listen`, `emit`, `parallel`, `serial`, `waterfall`, `bail`, `filter` |

### The 28 methods on today's `EventBus`

| Today | 0.17 |
| --- | --- |
| `on`, `on_opt`, `once`, `on_sync`, `on_sync_opt`, `on_pattern`, `on_pattern_opt`, `on_sync_pattern`, `on_sync_pattern_opt` | `ctx.on`, `ctx.on_sync`, `ctx.listen::<E>()` (`.prefix()`, `.name()`; `.prepend()`, `.once()`, `.global()` in `host::ListenExt`) |
| `emit`, `parallel`, `serial`, `waterfall`, `bail_sync` | `ctx.emit`, `parallel`, `serial`, `waterfall`, `bail` |
| `filter` | `ctx.filter` |
| `waterfall_sync`, `subscriptions` | `host::CtxHost` |
| `try_filter`, `on_remote`, `watch_table`, `observe_dispatch`, `emit_framework`, `dispatch`, `dispatch_sync`, `invoke_entry`, `invoke_entry_sync`, `transcoder`, `set_transcoder` | `internal` (signatures unchanged) |

### rutis-loader

The root keeps the names needed to assemble a loader: `LoaderPlugin`, `LoaderOptions`, `Loader`, `Layer`, `Patch`, `Builtins`, `Chain`, `Resolver`, `ServiceCatalog`, `LoaderError`, `RowEvent`, 11 in all. The rest move into modules by topic:

| Module | Contents |
| --- | --- |
| `runtime` | `RuntimeResolver`, `RuntimeRowsPlugin`, `RuntimeRows` and the like; the submodules `runtime::binary`, `runtime::go`, `runtime::swift` hold each language's `*Resolver`, `*Runtimes`, `*Manifest` |
| `peer` | `PeerResolver`, `register_peer_node`, `PeerRows`, `PeerRowsPlugin`, `LoaderCatalog` |
| `report` | what the loader's methods return: `CheckReport`, `ReconcileReport`, `RowInfo`, `RowStatus`, `RowCheck`, `Isolate`, `EntryInfo`, `EntryStatus`, `Failure`, `Unknown`, `Unavailable`, `Supply`, `Provider`, `Requirement`, `InstanceInfo`, `LoaderChanged`, `PendingEditDropped` |
| `edit` | imperative edits and persistence: `Edit`, `Editable`, `NewEntry`, `CreateInstance`, `Instance`, `InstanceResult`, `Persist`, `NoPersist`, `PersistError`, `Version`, `apply_edit`, `VolatileUpdate`, `volatile_key`, `volatile_paths` |
| `resolve` | for writing a resolver: `Build`, `Resolved`, `Declarations`, `Declared`, `ScopedFactory`, `Expressions`, `ExprScope` |
| `internal` (`#[doc(hidden)]`) | used only by rutis-host and tests: `apply_patches`, `Composed`, `ComposedRow`, `Owner`, `PatchWarning`, `is_runtime_name`, `resolve_entry`, `PLUGIN_API`, `SWIFT_PLUGIN_API`, `GO_MARKER`, `SWIFT_MARKER`, `read_*_manifest`, `node_schema`, `REMOVED_EVENTS` |

For plugins, `Builtins` has two registration methods:

- `register(name, build)`: `build: Fn(&C) -> Result<P>` with `P: Plugin` and `C: DeserializeOwned + JsonSchema`. The config schema comes from `C`, the dependencies from `P::Deps`. Without a schema, write `C` as `serde_json::Value`.
- `register_with(name, build)`: `build: Fn(&Build, &C) -> Result<P>`, for one build per instance.

`register_fn` is removed in favor of `register`. `register_raw` stays, for a `PluginFactory<Value>` whose dependencies are only known at run time.

### rutis-sdk (dylib plugins)

`export_plugin!(id: "weather", build: Weather::new)` takes the same build function as `Builtins::register`; `schema:` stays optional. The existing `factory:` form stays for plugins that need a `PluginFactory<Value>`. A dylib plugin already has to be built with exactly the host's SDK, so 0.17 plugins are simply rebuilt as usual.

## Appendix B: migration and implementation

### Migration

0.17 makes the whole change at once, with no old names kept, as 0.13 did for unified events: old code does not compile, and `docs/migration-0.17.md` gives the table to fix it by. There is no `#[deprecated]` transition: kept names would still show up in the documentation and in completion, which is exactly what #617 is about. The protocol does not change, so working with other languages and other nodes is unaffected.

**Rust plugin authors** change three things:

1. `impl Plugin` / `impl TypedPlugin` become the new `impl Plugin`: add `type Deps`, turn `fn apply<'a>(…) -> BoxFuture<…>` into `async fn apply(&self, ctx: &Ctx, deps: Self::Deps) -> Result`, and drop `Box::pin` and `Ok(Effect::Done)`. The types listed in `injects()` move into `type Deps`, and the matching `ctx.require::<T>()` calls in `apply` become parameters.
2. `ctx.events().on(&ctx, &EventKey::of(), |ctx, e, call| Box::pin(async move { … }))` becomes `ctx.on::<E>(|e, call| async move { … })`; `bail_sync` becomes `bail`; dispatch passes the value.
3. `CordisError` becomes `rutis::Error`; `.map_err(|e| CordisError::PluginFailed(e.into()))?` becomes `?`. A cleanup returned as an `Effect` becomes `ctx.effect(async move { … })`.

Before and after:

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

**Applications embedding rutis** add `use rutis::host::*;`; `PluginFactory` and hand-written object-safe plugins move to `host::DynPlugin` (`injects` returns `Vec`); `Builtins::register_fn` becomes `register`; names gone from the rutis-loader root are found in the modules of appendix A.

**The crates in this repository** change in the same release: rutis-bridge, rutis-loader, rutis-dylib, rutis-sdk, rutis-macros, rutis-host, rutis-dsh, rutis-dev, rutis-xtask, aimux-llm, and the examples rutis-agent, rutis-cli and `examples/`. Outside the kernel there are 42 `impl Plugin` and 14 `impl PluginFactory`; they become `host::DynPlugin` / `host::PluginFactory` mechanically. Only aimux-llm and the examples switch to the new `Plugin`, since they are what plugin authors read. The pure renames (`CordisError` appears 1277 times) are done by script. The only real rewrite is in the kernel: events, cleanup and the plugin wrapper.

The other languages' SDKs and runtimes, the protocol and `rutis.json` are unaffected.

### Implementation

Commits on one integration branch in this order, merged as a whole, shipped in 0.17:

1. Kernel: the new `Plugin` / `Deps` / `DynPlugin` with the blanket impl; `Error` / `ErrorKind`; `effect`, `spawn` and the event methods on `Ctx` (with an owned `Call`); the merged `Key<T>`; the `host` / `internal` modules and extension traits; removal of the names in appendix A.
2. rutis-bridge, rutis-loader, rutis-dylib, rutis-sdk and rutis-macros move to `DynPlugin` and `internal`; `Builtins::register` and `export_plugin!(build:)` take typed build functions.
3. Regroup the rutis-loader root and modules.
4. rutis-host, rutis-dsh, rutis-dev, rutis-xtask, aimux-llm and the examples (rutis-agent, rutis-cli, `examples/`).
5. Documentation in both languages: the README's Rust part, the new `guide/rust-plugin.md`, `guide/README.md`, `guide/rust-host.md`, the Rust parts of `guide/plugin-api.md`, the development handbook, `migration-0.17.md`.
6. Public-surface snapshots and the CI checks.
