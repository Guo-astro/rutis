# Write a Rust Plugin

From dependencies to writing and testing the plugin, and to a host using it. Needs Rust 1.88 or later.

Rust plugins follow the same rules as TypeScript, Python and Go plugins (see the [plugin API](plugin-api.en.md)); what differs is how they are distributed: **a Rust plugin is compiled into a Rust host** ([embed rutis in a Rust application](rust-host.en.md)), **or built as a dylib plugin** that a host loads at run time (the [external plugin guide](../external-plugin-guide.en.md)). The plugin is written the same way for both.

Writing a plugin takes only these:

| Name | For |
| --- | --- |
| `rutis::Plugin` | The one plugin trait: `type Deps` and `async fn apply` |
| `rutis::Ctx` | The context `apply` receives, see the table below |
| `rutis::Result`, `rutis::Error` | The one error type |
| `rutis::Event` | Event types |
| `rutis::events::Mode` | Which mode a listener is called in |
| `rutis::Disposer` | What `provide`, `on` and `effect` return, to withdraw early |
| `#[rutis::service]` | Serves a service to other languages |
| `rutis::Definition` | A plugin definition: a plugin to hand over and mount many times, see [Plugin definitions](#plugin-definitions) |

The `Ctx` methods for plugins:

| Group | Methods |
| --- | --- |
| Services | `provide`, `provide_as`, `get` |
| Lifecycle | `effect`, `cancelled`, `plugin` |
| Events | `on`, `on_sync`, `listen`, `emit`, `parallel`, `serial`, `waterfall`, `bail`, `filter` |

The types of listeners and call objects are inferred by the compiler; you do not write them.

## 1. Dependencies

```toml
[dependencies]
rutis = "0.17"
serde = { version = "1", features = ["derive"] }             # event and config types derive Serialize / Deserialize
tokio = { version = "1", features = ["rt-multi-thread", "macros", "time"] }  # background tasks, tests
```

## 2. Write the plugin

```rust
use std::sync::Arc;
use rutis::{Ctx, Plugin, Result};

/// A service is a type.
pub struct Greeting(pub String);

/// Provides Greeting. What apply registers is withdrawn when the plugin stops.
pub struct Greeter {
    pub lang: String,
}

impl Plugin for Greeter {
    type Deps = ();

    async fn apply(&self, ctx: &Ctx, _: ()) -> Result {
        ctx.provide(Greeting(format!("hello from {}", self.lang)))?;
        Ok(())
    }
}

/// Depends on Greeting: starts when it appears, restarts when it is replaced.
pub struct Printer;

impl Plugin for Printer {
    type Deps = Arc<Greeting>;

    async fn apply(&self, _: &Ctx, greeting: Arc<Greeting>) -> Result {
        println!("{}", greeting.0);
        Ok(())
    }
}
```

A plugin does not decide when it runs; the host does. Writing one takes three facts:

1. `apply` runs once every required service in `Deps` is ready; optional ones are not waited for.
2. Everything `apply` registers through `ctx` (services, listeners, cleanups, child plugins) is withdrawn in reverse order when the plugin stops; when `apply` fails, what it registered so far is withdrawn the same way.
3. When a required service is withdrawn or replaced, the plugin stops first, and runs `apply` again once the service is ready.

| Topic | Rule |
| --- | --- |
| `type Deps` | Required; `()` when there are no dependencies, with `_: ()` as the `apply` parameter |
| `name` | The name in logs and diagnostics of a plugin mounted with `ctx.plugin(p)`, by default the last segment of the type name (`Greeter`); override `fn name(&self) -> &str` to change it. A plugin mounted from its constructor has the name it was mounted under: the name registered with `Builtins::register`, the `id` of `export_plugin!`, the type name for `plugin_from` |
| `validate` | `fn validate(&self) -> Result`, run at load, before `apply`; an error leaves the plugin Failed and `apply` does not run |
| Configuration | Fields of the struct, plus a `fn new(config: &Config) -> Result<Self>`; whoever mounts the plugin builds it with that, and rebuilds it with it when the configuration changes (see [section 4](#4-used-by-a-host)) |
| Concurrency | `apply`, listeners and service methods run on tokio and may be called at the same time: the plugin and the services it provides are `Send + Sync` |

The guarantees (six states, exactly-once cleanup, consumers stop before their provider) are in [guarantees](../guarantees.en.md); you do not need them to write a plugin.

### Services

```rust
use std::sync::Arc;
use rutis::{Ctx, Plugin, Result};

pub trait Llm: Send + Sync {
    fn ask(&self, question: &str) -> String;
}

struct Echo;

impl Llm for Echo {
    fn ask(&self, question: &str) -> String {
        question.to_owned()
    }
}

/// Used when there, not needed.
pub struct Cache;

pub struct Forecast {
    pub llm: Arc<dyn Llm>,
}

struct LocalLlm;

impl Plugin for LocalLlm {
    type Deps = ();

    async fn apply(&self, ctx: &Ctx, _: ()) -> Result {
        ctx.provide_as::<dyn Llm>(Arc::new(Echo))?; // provided as the trait object type
        Ok(())
    }
}

struct Weather;

impl Plugin for Weather {
    type Deps = (Arc<dyn Llm>, Option<Arc<Cache>>);

    async fn apply(&self, ctx: &Ctx, (llm, cache): Self::Deps) -> Result {
        if cache.is_some() {
            // a cache was there at load
        }
        ctx.provide(Forecast { llm })?;
        Ok(())
    }
}
```

| Method | What it does |
| --- | --- |
| `ctx.provide(value)` | Provides `value` under its own type until the plugin stops, or until the returned `Disposer`'s `dispose()` |
| `ctx.provide_as::<dyn T>(arc)` | Provides under a trait object type: `ctx.provide_as::<dyn Llm>(Arc::new(MyLlm))` |
| `ctx.get::<T>()` | An optional read; `T` may be a trait object. It does not gate and creates no reload relation; diagnostics record it as an undeclared access. For "use it if it is there"; services the plugin needs go in `Deps` |

What `type Deps` can be:

| `type Deps =` | Gates on | Passed |
| --- | --- | --- |
| `()` | nothing | `()` |
| `Arc<T>` | `T` | the service |
| `Option<Arc<T>>` | nothing | the service if visible at load; its arrival or departure later does not reload |
| a tuple of up to eight | all members | each value |

`T` may be a trait object (`Arc<dyn Llm>`). When two services of one type are provided under different names and a plugin depends on one of them by name, use the host-level form: [embed rutis in a Rust application: depend on services by name](rust-host.en.md#depend-on-services-by-name).

### Serve a service to other languages

Mark the service type's `impl` block `#[rutis::service]` and provide it as usual; rows of every language can then call it by name. The name comes from the type (`Calendar` is `calendar`); `#[service(name = "…")]` changes it:

```rust
use std::sync::atomic::{AtomicU64, Ordering};
use rutis::{Ctx, Plugin, Result};

#[derive(Default)]
pub struct Calendar {
    next: AtomicU64,
}

#[rutis::service]                     // served as "calendar"
impl Calendar {
    pub async fn add(&self, title: String) -> u64 {
        let id = self.next.fetch_add(1, Ordering::SeqCst);
        println!("{id}: {title}");
        id
    }
}

struct Calendars;

impl Plugin for Calendars {
    type Deps = ();

    async fn apply(&self, ctx: &Ctx, _: ()) -> Result {
        ctx.provide(Calendar::default())?; // provided means served
        Ok(())
    }
}
# fn main() {}
```

```python
async def apply(ctx, config):
    await ctx.use("calendar").add("standup")

plugin = define_plugin(apply, inject=["calendar"])
```

An unmarked type is not served. What crosses, how errors are thrown and methods taking `&Ctx`: [mark the type `#[rutis::service]`](rust-host.en.md#mark-the-type-rutisservice). For several copies of one service, use rutis-loader instances: mount one copy in each instance, and the rows of an instance use its own.

A service's parameters take data and functions, not objects with methods: when another language hands an object (a class instance, a `@RutisService` instance, `rutis.Ref(v)` and so on) to such a parameter, the call fails with `bad-argument`, and the error suggests passing functions instead: `store.put: argument 1 should be a Config, got an object from py; store takes data and functions, so pass a function for each method`. To use the other side's behaviour, make the parameter a function (`impl Fn`) and have the caller pass one function per method. Rust plugins neither send nor receive their own objects in 0.18; objects passing through a Rust host are forwarded by it as usual. A hand-written [`HostDispatch`](rust-host.en.md#write-a-hostdispatch-by-hand) receives an object as a `rutis_bridge::session::Reference` and calls its methods with `call_method` / `call_method_async`; such a call made while handling another session's call is forwarded on that session's behalf automatically.

### Plugin definitions

A `Plugin` is an instance: its config is in its fields, and handing it to `ctx.plugin` uses it up. To hand a plugin elsewhere, to be mounted there many times (one per session, say), use `rutis::Definition`: `Definition::new(ctx, build)` makes a definition, whose `build` gets the `ctx` where a copy mounts and that copy's config, and returns a `Plugin`; the mounting side writes `ctx.plugin(def.at(&ctx, config)?)`.

```rust
use std::sync::{Arc, Mutex};
use rutis::{Ctx, Definition, Plugin, Result};

/// One per session; `depth` comes from its own config.
struct Part {
    depth: u32,
}

impl Plugin for Part {
    type Deps = ();

    async fn apply(&self, _: &Ctx, _: ()) -> Result {
        println!("depth {}", self.depth);
        Ok(())
    }
}

/// Hands a definition of `Part` to whoever mounts parts (here just a list).
struct Git {
    parts: Arc<Mutex<Vec<Definition>>>,
}

impl Plugin for Git {
    type Deps = ();

    async fn apply(&self, ctx: &Ctx, _: ()) -> Result {
        let pool = Arc::new(String::from("pool")); // prepared in apply
        let definition = Definition::new(ctx, move |_at: &Ctx, depth: u32| {
            let _pool = pool.clone();
            Ok(Part { depth })
        });
        self.parts.lock().unwrap().push(definition);
        Ok(())
    }
}

/// The mounting side: each mount is one copy, with its own config.
fn mount(at: &Ctx, definition: &Definition) -> Result {
    at.plugin(definition.at(at, 3u32)?);
    Ok(())
}
# fn main() {}
```

- A definition belongs to the current generation of the plugin that made it: when that plugin unloads, restarts or ends, every copy mounted from it ends first, and is cleaned up, before the plugin's own cleanups run; then `at` fails and `ended()` completes.
- It can only be mounted inside the instance of the plugin that made it, or deeper.
- A plugin definition another language hands over is a `Definition` too, mounted the same way; see the [plugin API](plugin-api.en.md#plugin-definitions). A definition made in Rust cannot be handed to another language.

### Events

All listeners of every language and process on a host are on one event bus ([plugin API: events](plugin-api.en.md#events)): wherever an event is emitted and wherever its listeners are, the result is the same.

An event is a type: `NAME` is its name in every language, and its arguments go through serde. A tuple struct of two or more fields is that many arguments, so `Created` below is `emit("session/created", id, size)` in other languages; a unit struct has no arguments; anything else (a struct with named fields, a newtype) is one argument.

```rust
use rutis::events::Mode;
use rutis::{Ctx, Event, Plugin, Result};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub struct Created(pub String, pub u32);

impl Event for Created {
    const NAME: &'static str = "session/created";
    type Value = String; // what serial, bail and waterfall return
}

/// Only emitted, no result asked for: its listeners return Ok(None).
#[derive(Serialize, Deserialize)]
pub struct Closed(pub String);

impl Event for Closed {
    const NAME: &'static str = "session/closed";
    type Value = ();
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
            Ok(Some(format!("audited {}", event.0))) // Ok(None) is no result
        })?;
        ctx.on_sync::<Created>(|event, _| Ok(Some(format!("sync {}", event.1))))?;
        ctx.listen::<Created>().prefix("session/").on(|_, call| async move {
            println!("{}", call.name()); // a prefix listener: the actual name
            Ok(None)
        })?;
        Ok(())
    }
}
```

Events are sent in five ways, named as in every other language:

```rust
# use rutis::{Ctx, Event};
# use serde::{Deserialize, Serialize};
# #[derive(Serialize, Deserialize)]
# pub struct Created(pub String, pub u32);
# impl Event for Created {
#     const NAME: &'static str = "session/created";
#     type Value = String;
# }
# async fn demo(ctx: &Ctx) -> rutis::Result {
ctx.emit(Created("s1".into(), 3))?;                         // returns at once
ctx.parallel(Created("s1".into(), 3)).await?;               // waits for all
let first = ctx.serial(Created("s1".into(), 3)).await?;     // the first result
let out = ctx
    .waterfall(Created("s1".into(), 3))
    .terminal(|e| async move { Ok(Some(format!("saved {}", e.0))) }) // optional
    .await?;
let quick = ctx.bail(Created("s1".into(), 3))?;             // synchronous
let from_python = ctx
    .filter(|l| l.runtime.as_deref() == Some("py"))
    .serial(Created("s1".into(), 3))
    .await?;
# let _ = (first, out, quick, from_python);
# Ok(())
# }
```

| Topic | Rule |
| --- | --- |
| Listener | A closure taking `(Arc<E>, Call<E>)`, a synchronous one `(Arc<E>, SyncCall<'_, E>)`; it returns `Result<Option<E::Value>>`, where `Ok(None)` and `Ok(Some(false))` are no result. The compiler infers the parameter types, and `?` works on any error inside. The closure is called many times: clone what moves into `async move` inside the closure first |
| `call` | `mode()` is this dispatch's mode; `name()` the dispatched name; `next()`, in a `waterfall` only (elsewhere `WaterfallNext`), runs the rest of the chain, and not calling it vetoes the rest; `ctx()` is the `Ctx` that registered the listener, and what is registered on it belongs to this plugin |
| Sync and async | Every mode can call an `on_sync` listener, but it cannot call `next` in an async `waterfall`; `bail` cannot call an `on` listener (`AsyncListener`) |
| Names | Registered and sent under `E::NAME`. `listen::<E>().name(n)` listens under another name; `.prefix(p)` hears every name under the prefix, each decoded as `E`, and an event of another shape fails that call with `EventValue` |
| Sending | `emit`, `parallel`, `serial`, `waterfall` and `bail` take an `E` or an `Arc<E>`; `waterfall` returns a builder: `.terminal(…)` first if wanted, then `.await`; without a terminal, the last `next()` gets `None`. `ctx.filter(keep)` gives the same five methods, `keep` seeing each listener's `ListenerInfo` |
| Failures | An `emit` listener's failure does not reach the emitter; it is reported to the host. `parallel` runs every listener and returns `ListenersFailed` (in registration order, also for a single failure); `serial`, `bail` and `waterfall` stop at the first error and return it |
| Unload | When the plugin stops its listeners are withdrawn; running calls see `call.ctx().cancelled()`, and the unload waits for them, at most until the shutdown deadline. Dropping a `parallel`, `serial` or `waterfall` future cancels that dispatch |
| Reserved names | `rutis/*` (`rutis/service`, `rutis/row`, `rutis/runtime`) can be listened to, not sent (`ReservedEvent`) |

#### Advanced: `once`, `prepend`, `global`

```rust
# use rutis::{Ctx, Event};
# use serde::{Deserialize, Serialize};
# #[derive(Serialize, Deserialize)]
# pub struct Created(pub String, pub u32);
# impl Event for Created {
#     const NAME: &'static str = "session/created";
#     type Value = String;
# }
# fn demo(ctx: &Ctx) -> rutis::Result {
ctx.listen::<Created>().once().on(|_, _| async move { Ok(None) })?;    // called at most once
ctx.listen::<Created>().prepend().on_sync(|_, _| Ok(None))?;           // before this name's existing listeners
ctx.listen::<Created>().global().on(|_, _| async move { Ok(None) })?;  // filters skip it
# Ok(())
# }
```

Options chain, ending with `.on(…)` or `.on_sync(…)`. An event type with no fixed name (the value decides it) has an empty `NAME` and overrides `fn name(&self) -> &str` to return the value's name; listen to it with `.name(…)`.

#### Emitting from a service

A service whose methods emit keeps a clone of the plugin's `Ctx`; it then emits as that plugin, whoever calls the method:

```rust
# use rutis::Event;
# use serde::{Deserialize, Serialize};
# #[derive(Serialize, Deserialize)]
# pub struct Created(pub String, pub u32);
# impl Event for Created {
#     const NAME: &'static str = "session/created";
#     type Value = String;
# }
use rutis::{Ctx, Plugin, Result};

pub struct Sessions {
    ctx: Ctx,                                         // the plugin's, kept by the service
}

impl Sessions {
    pub async fn create(&self, id: String) -> Result {
        self.ctx.parallel(Created(id, 1)).await       // waits for every listener
    }
}

struct SessionsPlugin;

impl Plugin for SessionsPlugin {
    type Deps = ();

    async fn apply(&self, ctx: &Ctx, _: ()) -> Result {
        ctx.provide(Sessions { ctx: ctx.clone() })?;
        Ok(())
    }
}
```

- Keeping the clone is fine: it is a handle, and keeps nothing running. When the plugin stops, its services are withdrawn, so nobody reaches `Sessions` through rutis any more.
- After the stop, what the kept `ctx` registers (`on`, `effect`, `provide`) is refused with `InactiveEffect`. Do not rely on emitting from it then: a method that a caller who kept the service may still call checks for the stop first, as `ctx.cancelled()` completes once the plugin starts stopping.
- Events of a type that are only emitted, with no result asked for, have `type Value = ();` (`Closed` above): the listeners return `Ok(None)`.

### Cleanup and background tasks

```rust
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use rutis::{Ctx, Plugin, Result};

pub trait Store: Send + Sync {
    fn flush(&self);
}

struct Syncer {
    path: PathBuf,
}

impl Plugin for Syncer {
    type Deps = Arc<dyn Store>;

    async fn apply(&self, ctx: &Ctx, store: Arc<dyn Store>) -> Result {
        let file = std::fs::File::open(&self.path)?; // io::Error through `?`
        ctx.effect(|| {
            let stop = ctx.clone();
            let task = tokio::spawn(async move {
                let mut tick = tokio::time::interval(Duration::from_secs(60));
                loop {
                    tokio::select! {
                        _ = stop.cancelled() => break,         // the plugin is stopping
                        _ = tick.tick() => store.flush(),
                    }
                }
            });
            async move {
                let _ = task.await;                            // wait for it to exit at stop
                drop(file);
                Ok(())
            }
        })?;
        Ok(())
    }
}
```

- `ctx.effect(|| { …; async move { … } })` runs the closure now and registers the future it returns as a cleanup. When the plugin stops, cleanups run with the other registrations, in reverse order. Without a cleanup, do not call `effect`; for several, call it several times.
- When the plugin is already stopping or stopped, `effect` (and every registration: `provide`, `on`, …) returns an `InactiveEffect` error and the closure does not run. So a resource taken inside the closure is never taken when the registration is refused, and never leaks.
- Background tasks are tokio's own `tokio::spawn` (a `JoinSet` for a group): start them in the `effect` closure and wait for them in the cleanup it returns; inside, watch `ctx.cancelled()`, which completes when the plugin starts stopping. Stopping waits for cleanups; how long is the host's choice: rutis-host waits at most 10 s (`--shutdown-timeout`, `RUTIS_SHUTDOWN_TIMEOUT`), a Rust embedding's `shutdown()` waits as long as it takes, and `shutdown_with_timeout(duration)` limits it, its error naming which plugin's cleanup has not finished (one registered with `effect` by the file and line it was registered at).
- A long-running listener watches `call.ctx().cancelled()` the same way. `ctx.plugin(child)` mounts a child plugin that stops with this one.
- A cleanup's error is reported to the host. `dispose()` on the `Disposer` that `effect` returns runs the cleanup early.

### Errors

Every signature has one error type, `rutis::Error`; `rutis::Result<T = ()>` is `Result<T, rutis::Error>`. `?` converts any `std::error::Error + Send + Sync + 'static` into it; `Error::msg("…")` makes one from a message:

```rust
use rutis::{Error, Result};

#[derive(Debug, thiserror::Error)]
#[error("quota exceeded: {used} of {limit}")]
pub struct QuotaExceeded {
    used: u32,
    limit: u32,
}

fn remaining(limit: &str, used: u32) -> Result<u32> {
    let limit: u32 = limit.trim().parse()?;        // ParseIntError through `?`
    if limit == 0 {
        return Err(Error::msg("limit must not be 0"));
    }
    if used > limit {
        return Err(QuotaExceeded { used, limit }.into()); // your own error type
    }
    check_name("weather").map_err(Error::msg)?;     // a String is not an error type
    Ok(limit - used)
}

fn check_name(_: &str) -> std::result::Result<(), String> {
    Ok(())
}
```

| Case | Write |
| --- | --- |
| An `io::Error`, a parse error or your own thiserror type in plugin code | `?` |
| Just a message, or a library returning `Result<_, String>` | `Error::msg("…")`, `.map_err(Error::msg)?` |
| An `anyhow::Error` in plugin code | `.map_err(\|e\| Error::msg(format!("{e:#}")))?`. `anyhow::Error` does not implement `std::error::Error`, so `?` cannot convert it |
| Your own thiserror type wrapping a rutis error | `#[error(transparent)] Rutis(#[from] rutis::Error)` or `#[source] cause: rutis::Error` |

An error from plugin code reaches the host as a `PluginFailed`, with the original error as its source. How a host tells rutis's own failures apart: [embed rutis in a Rust application: errors a host sees](rust-host.en.md#errors-a-host-sees).

## 3. Test

Tests need no separate tool: in a `#[tokio::test]`, make a root `Ctx`, mount the plugin and the services it needs, and assert. Making and closing the root are host methods, so `use rutis::host::*;` first:

```rust
# use std::sync::Arc;
# use rutis::events::Mode;
# use rutis::{Event, Plugin};
# use serde::{Deserialize, Serialize};
# pub struct Greeting(pub String);
# pub struct Greeter { pub lang: String }
# impl Plugin for Greeter {
#     type Deps = ();
#     async fn apply(&self, ctx: &Ctx, _: ()) -> rutis::Result {
#         ctx.provide(Greeting(format!("hello from {}", self.lang)))?;
#         Ok(())
#     }
# }
# #[derive(Serialize, Deserialize)]
# pub struct Created(pub String, pub u32);
# impl Event for Created {
#     const NAME: &'static str = "session/created";
#     type Value = String;
# }
# struct Audit;
# impl Plugin for Audit {
#     type Deps = ();
#     async fn apply(&self, ctx: &Ctx, _: ()) -> rutis::Result {
#         ctx.on::<Created>(|event, call| async move {
#             if call.mode() == Mode::Waterfall {
#                 return Ok(call.next().await?.map(|text| format!("[{text}]")));
#             }
#             Ok(Some(format!("audited {}", event.0)))
#         })?;
#         Ok(())
#     }
# }
use rutis::host::*;
use rutis::Ctx;

#[tokio::test]
async fn greets() -> Result<(), Box<dyn std::error::Error>> {
    let root = Ctx::root()?;
    let greeter = root.plugin(Greeter { lang: "Rust".into() });
    (&greeter).await?; // apply has run
    assert_eq!(root.get::<Greeting>().unwrap().0, "hello from Rust");

    greeter.dispose().await?; // stopped: the service is withdrawn
    assert!(root.get::<Greeting>().is_none());
    root.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn audits() -> Result<(), Box<dyn std::error::Error>> {
    let root = Ctx::root()?;
    let audit = root.plugin(Audit);
    (&audit).await?;
    let out = root.serial(Created("s1".into(), 3)).await?;
    assert_eq!(out.as_deref(), Some("audited s1"));
    root.shutdown().await?;
    Ok(())
}
# fn main() {}
```

```bash
cargo test
```

- For a plugin that depends on other services, mount a plugin providing fakes first, or provide them on the root: `root.provide_as::<dyn Llm>(Arc::new(FakeLlm))?`.
- `(&view).await` returns once the plugin's load was handled: a plugin whose dependency is missing is still `Pending` then, not running. So a test mounts its fakes first and the plugin after: `(&view).await` then returns with the plugin running.
- An `emit` listener's failure goes to the root's error sink: make the root with `Ctx::root_with_sink(handle, sink)` to collect and assert on them.
- Listeners in other languages and values crossing processes need a real runtime, as in production: [embed rutis in a Rust application](rust-host.en.md#runtime-and-loader).

## 4. Used by a host

### Compiled into a Rust host

The plugin's configuration is a struct, and a constructor builds the plugin from it:

```rust
use rutis::host::*;
use rutis::{Ctx, Plugin, Result};
use serde::Deserialize;

#[derive(Deserialize)]
pub struct WeatherConfig {
    pub city: String,
}

pub struct Forecast(pub String);

pub struct Weather {
    city: String,
}

impl Weather {
    pub fn new(config: &WeatherConfig) -> Result<Self> {
        Ok(Weather { city: config.city.clone() })
    }
}

impl Plugin for Weather {
    type Deps = ();

    async fn apply(&self, ctx: &Ctx, _: ()) -> Result {
        ctx.provide(Forecast(format!("sunny in {}", self.city)))?;
        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = Ctx::root()?;
    let weather = root.plugin_from(Weather::new, WeatherConfig { city: "Oslo".into() });
    (&weather).await?;
    weather.update(WeatherConfig { city: "Bergen".into() }).await?; // rebuilt with Weather::new
    root.shutdown().await?;
    Ok(())
}
```

To mount it from configuration with rutis-loader (rows of `rutis.json`), register the same constructor under a name; the config is decoded as `WeatherConfig` and its JSON Schema generated from it, so it also derives `schemars::JsonSchema`. That is schemars 1, the version rutis-loader uses (it does not re-export it): add `schemars = "1"` to `[dependencies]`.

```rust,ignore
#[derive(Deserialize, schemars::JsonSchema)]
pub struct WeatherConfig {
    pub city: String,
}

let mut builtins = Builtins::new();
builtins.register("weather", Weather::new); // row: { "id": "weather", "name": "weather", "config": { "city": "Oslo" } }
```

Before `build`, the loader fills the row's config with the schema's defaults and validates it: a row whose config does not match fails with `config: …`, and the constructor is not called ([plugin API: configuration](plugin-api.en.md#configuration)); extra keys are written on stderr. `Option` fields and fields with `#[serde(default)]` are not required; left out, they take serde's default. A decoding error after the schema passed is written `<path> is not a value the plugin accepts`.

For one copy per instance, `register_with(name, |build, config| …)`. The loader (with `builtins` in its `Chain`), runtimes and rows of other languages: [embed rutis in a Rust application](rust-host.en.md#runtime-and-loader).

### Debugging

The plugin runs in the host's process: run the host (or the test binary of `cargo test`) in an IDE, `rust-lldb` or `rust-gdb`, and set breakpoints in the plugin's code. How to see each row's state, which plugin provides a service, and the calls in progress in the host: the [Rust host guide](rust-host.en.md#debugging).

### Dylib plugin

The same plugin can also be built as a dylib that a host loads at run time. The crate depends on `rutis-sdk` only, reaches rutis through `rutis_sdk::rutis` (`use rutis_sdk::rutis::{Ctx, Plugin, Result};`) and exports its constructor:

```rust,ignore
rutis_sdk::export_plugin!(id: "weather", build: Weather::new);
```

- `build:` takes the same constructor as `Builtins::register`; `schema:` optionally gives the config's JSON Schema (a `serde_json::Value`): with it the config is filled and validated as for `Builtins` rows; without it, only decoded.
- A dylib plugin derives its event and config types with the SDK's serde: `#[derive(rutis_sdk::rutis::serde::Deserialize)]` with `#[serde(crate = "rutis_sdk::rutis::serde")]`. The plugin's own serde is not the SDK's.
- A dylib plugin is built with exactly the SDK of its host. Workspace, packing and troubleshooting: the [external plugin guide](../external-plugin-guide.en.md).

Upgrading from 0.16: [migrating to 0.17](../migration-0.17.en.md).
