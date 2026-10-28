# Core features

[中文](core-features.md) · [Development handbook](development-handbook.en.md) · [API docs](https://docs.rs/rutis)

What the `rutis` core offers beyond plugins, fibers, services and events, and where its boundaries are. To get started, read the quick start in the [README](../README.md), [Write a Rust plugin](guide/rust-plugin.en.md) and the [application design guide](development-guide.en.md) first.

The host methods below (`plugin_with`, `shutdown`, `diagnostics`, `require` and the like) come from `rutis::host::CtxHost`: write `use rutis::host::*;` first. What is marked as in `rutis::internal` is used between rutis's own crates (rutis-bridge, rutis-loader and the others), with no stability promise.

## Config hot update

change config at runtime, reusing the state machine's exactly-once cleanup; affected consumers follow automatically:

```rust
struct Weather { city: String }

struct WeatherFactory;
impl PluginFactory<WeatherConfig> for WeatherFactory {
    /// Construction only: the dry run and the real load both call it.
    fn build(&self, config: &WeatherConfig) -> rutis::Result<Box<dyn DynPlugin>> {
        Ok(Box::new(Weather { city: config.city.clone() }))
    }
}

let view = ctx.plugin_with(WeatherFactory, cfg_v1);
view.update(cfg_v2).await?;   // dry-run failure leaves everything untouched; success unloads + reloads
```

`Weather` is an ordinary `Plugin`; the factory (`host::PluginFactory<C>`) builds it from the current config on each load, and lists its dependencies in the factory's `injects()`.

## Events: one listener table, five modes

Events are dispatched by **name** (`E::NAME`; a type with no fixed name overrides `Event::name` so each value carries its own, and is listened to with `ctx.listen::<E>().name(n)`). Event types derive `Serialize` / `Deserialize`, so listeners of another Rust type with the same name, and listeners in other processes or on other nodes, hear the same event ([unified events design](design-events-2026-10-18.en.md), [protocol spec](design-events-protocol-2026-10-18.en.md) §12.1).

```rust
#[derive(Serialize, Deserialize)]
struct RoomEvent { room: String }
impl Event for RoomEvent { const NAME: &'static str = "room/joined"; type Value = String; }

// Registered once, called by any mode; call.mode() says which, call.next() only in a waterfall
ctx.on::<RoomEvent>(|e, _call| async move { Ok(Some(format!("hi {}", e.room))) })?;
let hi = ctx.serial(RoomEvent { room: "a".into() }).await?;
```

- `emit` returns at once and starts every listener in registration order without waiting for any; one name's emits start in emit order; a listener's failure goes to the ErrorSink (`ListenerFailed`) and never affects the other listeners.
- `parallel` starts in registration order and waits for all; failures are always `ListenersFailed` (AggregateError), in registration order.
- `serial` / `bail` return the first result (`None` and `Some(false)` are no result); an error stops them. `bail` meeting an async listener fails with `AsyncListener`.
- `waterfall` runs outer to inner, each listener calling the rest with `call.next()`; not calling it vetoes; the terminal is optional (`ctx.waterfall(e).terminal(…)`, none when left out); the outermost value is returned.
- Synchronous listeners register with `on_sync`; every mode can call them. `ctx.listen::<E>().prefix("room/")` registers a prefix listener, which reads the actual name from `call.name()`; `listen()` also has `.once()`, `.prepend()` and `.global()`.
- **Scope**: the loader marks each group instance as a scope (`mark_scope`, in `rutis::internal`); `ctx.scope()` is the scope chain from the root. Events are delivered by name on one root's bus, and every listener receives them; a scope is only a descriptor in `ListenerInfo` for filters.
- **Filter**: `ctx.filter(|info: &ListenerInfo| ..)` gives the same five emitting methods; the filter sees each non-`global` listener's descriptor (plugin, row, runtime, node, scope) in the emitter's process, before any call.
- **Lifecycle**: a listener is called with the `Ctx` that registered it; once its fiber unloads no new call starts, and the unload waits for running calls (their `ctx.cancelled()` has fired), at most until the host's shutdown deadline.
- `rutis/*` is reserved for framework events: plugins listen to them; emitting one fails with `ReservedEvent`.

## Cleanup names

A shutdown that runs out of time names the cleanups still running in its Stuck hint: one registered with `ctx.effect` by where it was registered, one registered with `effect_named("cache watcher", || effect)` (in `rutis::internal`) by that name.

## Intercepting service reads and writes

`ctx.intercept_require_as::<T>(key, hook)` applies only to strict reads that passed the declaration, instance and readiness checks; the optional reads `get` / `get_at` still locate the service directly. `ctx.intercept_set_as::<T>(key, hook)` handles updates to writable services. `hook` returns a `ServiceIntercept<T>`; it and writable bindings are in `rutis::internal`: `provide_mut_at(key, value)` returns a `ServiceWriter<T>` bound to the registration's generation, which the provider updates with `writer.set(&ctx, Arc::new(value))`; earlier `Arc<T>` snapshots keep their old value, and a write does not reload consumers. An interceptor can substitute a value of the same type or refuse the operation, so register trusted code only.

## API boundaries

The services a plugin needs go in its `type Deps`, and what `apply` receives is the result of a strict read. A host's `require` / `require_at` are the same strict reads, corresponding to Cordis's ordinary plugin service access. They check the dependency declarations (`Deps`, or `DynPlugin::injects()`) along the fiber ancestry and distinguish undeclared, unavailable, out-of-scope, and inactive reads, retaining the call site. If a read is both out of scope and inactive, the instance boundary takes precedence; registration and instance dispatch define their own error order. `get` / `get_at` correspond to Cordis's explicit `ctx.get()` locator: they return `Option` without enforcing declarations, and create no reload relation. A service is normally hidden while its provider is inactive or the reader is unloading, except that the provider's subtree can read its own service during cleanup. Instance keys also have subtree visibility checks. The `Ctx` that registered a listener (whose `on` was called) owns it, and the callback is called with that `Ctx`: resources the callback registers belong to the listener's plugin. See the compiling [listener ownership example](../crates/rutis/examples/listener_ctx_ownership.rs).

## Dependency diagnostics

`ctx.diagnostics()` lists live fibers with their identity, state, declared and bound dependencies, and service bindings. `injects[].status` distinguishes missing, out-of-scope instance, provider not ready, being removed, rejected by `check`, and `check` panicked; `TypeKey::describe()` gives the type name, qualifier and instance id. A read uses only registered metadata and the latest gate result: it calls no plugin or `check` and triggers no lifecycle transition. It reads fibers and bindings one by one and is **not an atomic snapshot of the tree**: under concurrent lifecycle changes, the states, dependencies and bindings in one result may come from different moments. A `check` status (a `check()` predicate is registered with `provide_checked`, in `rutis::internal`) may lag at the last gate result; after `refresh()`, wait for the affected fibers to settle and read again. Differences from Cordis in dispatch observation, cleanup trees and service interception are covered in the [design draft](design-cordis-observation.en.md).

A plugin whose services (its `Deps` or `injects`) never get a provider stays Pending, which by itself is no error. Still missing them after 10 seconds, the ErrorSink gets one `ErrorKind::StillWaiting` naming the plugin and the missing types (the default sink prints `printer: still waiting after 10s for app::Greeting: it starts once that is provided`); the plugin goes on waiting, and is reported at most once each time it starts waiting. rutis's own plugins (a link waiting for its peer, a runtime's rows waiting for the runtime) are not reported: their own state says what they wait for (from 0.18, #678). `ctx.report_waiting_after(Some(duration))` changes the delay, `None` reports nothing, for the whole tree (from 0.17.2, #676). Missing keys are written as `TypeKey::describe()` gives them; `ctx.name_keys_with(&namer)` adds a function naming keys, and rutis-loader adds one when it is mounted: a service by its name (`llm`), a row's runtime as `runtime py`. rutis-host turns it off: its row states already say what each row waits for.

`ctx.is_shutting_down()` reads the root's shutdown flag directly. It becomes true when `Ctx::shutdown()` is called, before cleanup completes, and stays true afterward. Child and isolated contexts share the flag; disposing or shutting down a child alone leaves it false. Use it when only the shutdown state is needed, such as before a loader edit or instance creation, to avoid collecting a diagnostic snapshot.

## Errors and panics

Every signature's error is `rutis::Error`, and `?` accepts any `std::error::Error`. A host tells rutis's failures apart with `err.kind()` (an `Option<&host::ErrorKind>`, `None` for plugin code's own errors); errors that are shared (the error sink, a `FiberView`'s result) are `Arc<ErrorKind>`.

Synchronous and asynchronous `apply` panics become plugin errors; a `check()` panic leaves a dependency unready; a `waterfall` callback panic propagates to its caller. `settle` is a FIFO barrier for one fiber, and Pending can be a stable result. A plugin whose load failed (in construction, `validate` or `apply`) stays Failed and loads again only when its dependencies change (a new provider generation, or one removed and back); repeated notifications of the same dependencies do not retry it. Root `dispose()` remains restartable, while `shutdown()` closes it permanently; dropping a waiting future does not stop cleanup already in progress. `update(config)` reassembles a plugin without replacing process code. An early `Disposer::dispose()` failure returns to its caller without notifying the error sink; these errors join a terminal unload result or reach the error sink on reload.

## Relation to Cordis

rutis is an idiomatic Rust implementation of the [Cordis](https://github.com/shigma/cordis) paradigm, not a translation: all 96 original specs reviewed line by line, the 58 language-agnostic invariants locked by automated parity tests; every other difference is explicitly declared (decision table + non-port list + audit record). Known deliberate strengthenings: cross-effect cleanup is strictly serial LIFO (cordis runs concurrently), one name's emits start in emit order.
