# Write a Rust Plugin

From starting the project to testing it, running it in a host, and publishing it. Needs Rust 1.88 or later.

Rust plugins follow the same rules as plugins in the other languages (see the [plugin API](plugin-api.en.md)); what differs: **a Rust plugin is compiled into a Rust host**, with no `rutis-host new` template and no `rutis-host dev`. The plugin is an ordinary library crate; a host adds it to its dependencies and mounts it in its own code ([embed rutis in a Rust application](rust-host.en.md)).

The whole guide builds one plugin, `greeter`: it provides the service `Greeter` (`hello(name)` returns a greeting), translates the greeting with the `Translate` service another plugin provides, takes the greeting from its configuration, and emits the event `greeter/greeted` on every greeting.

## 1. Start the project

<!-- example: rust-en run -->
```bash
cargo new --lib greeter
cd greeter
cargo add rutis serde --features serde/derive
cargo add --dev tokio --features macros,rt-multi-thread
cargo test
```

- `rutis`: the plugin API.
- `serde`: the configuration and event types derive `Serialize` / `Deserialize`.
- `tokio` (a dev-dependency): the tests and the host of step 7 run on it. The plugin itself does not depend on tokio.

`cargo test` runs the test `cargo new` generated; from step 2 on, the plugin's code replaces it.

## 2. Provide a service

A service is a type; the plugin implements `rutis::Plugin` and provides the service with `ctx.provide` in `apply`. Replace `src/lib.rs` with:

<!-- example: rust-en file=greeter/src/lib.rs -->
```rust
use rutis::{Ctx, Plugin, Result};

/// The service greeter provides.
pub struct Greeter;

impl Greeter {
    pub fn hello(&self, name: &str) -> String {
        format!("Hello, {name}!")
    }
}

/// The plugin.
pub struct GreeterPlugin;

impl Plugin for GreeterPlugin {
    type Deps = ();

    async fn apply(&self, ctx: &Ctx, _: ()) -> Result {
        ctx.provide(Greeter)?;
        Ok(())
    }
}
```

- What `apply` registers through `ctx` (services, listeners) is withdrawn when the plugin stops; you do not clean it up yourself.
- `type Deps` is required; with no dependencies it is `()`, and the `apply` parameter is `_: ()`.
- The plugin and the services it provides are `Send + Sync`: they run on tokio and may be called at the same time.
- To let plugins in other languages call this service by name, see [serve a service to other languages](#serve-a-service-to-other-languages).

## 3. Use a service

The services the plugin uses go in `type Deps`, and `apply` receives them. `Translate` is a trait another plugin provides (the test of step 6 and the host of step 7 each provide a fake one):

<!-- example: rust-en file=greeter/src/lib.rs -->
```rust
use std::sync::Arc;

use rutis::{Ctx, Plugin, Result};

/// The service greeter uses, provided by another plugin.
pub trait Translate: Send + Sync {
    fn text(&self, text: &str) -> String;
}

/// The service greeter provides.
pub struct Greeter {
    translate: Arc<dyn Translate>,
}

impl Greeter {
    pub fn hello(&self, name: &str) -> String {
        format!("{}, {name}!", self.translate.text("Hello"))
    }
}

/// The plugin.
pub struct GreeterPlugin;

impl Plugin for GreeterPlugin {
    type Deps = Arc<dyn Translate>;

    async fn apply(&self, ctx: &Ctx, translate: Arc<dyn Translate>) -> Result {
        ctx.provide(Greeter { translate })?;
        Ok(())
    }
}
```

`apply` runs once every service in `Deps` is ready; when one is withdrawn or replaced, the plugin stops first and runs `apply` again once it is back. Several services are a tuple: `type Deps = (Arc<A>, Arc<dyn B>);`.

## 4. Configuration

The configuration is a struct the plugin is built from: `GreeterPlugin::new(&config)`. A field left out takes its value from `Default`; here `greeting` defaults to `"Hello"`:

<!-- example: rust-en file=greeter/src/lib.rs -->
```rust
use std::sync::Arc;

use rutis::{Ctx, Plugin, Result};
use serde::Deserialize;

/// The service greeter uses, provided by another plugin.
pub trait Translate: Send + Sync {
    fn text(&self, text: &str) -> String;
}

/// The configuration. A field left out takes its value from Default.
#[derive(Clone, Deserialize)]
#[serde(default)]
pub struct Config {
    pub greeting: String,
}

impl Default for Config {
    fn default() -> Self {
        Config { greeting: "Hello".into() }
    }
}

/// The service greeter provides.
pub struct Greeter {
    greeting: String,
    translate: Arc<dyn Translate>,
}

impl Greeter {
    pub fn hello(&self, name: &str) -> String {
        format!("{}, {name}!", self.translate.text(&self.greeting))
    }
}

/// The plugin.
pub struct GreeterPlugin {
    config: Config,
}

impl GreeterPlugin {
    pub fn new(config: &Config) -> Result<Self> {
        Ok(GreeterPlugin { config: config.clone() })
    }
}

impl Plugin for GreeterPlugin {
    type Deps = Arc<dyn Translate>;

    async fn apply(&self, ctx: &Ctx, translate: Arc<dyn Translate>) -> Result {
        ctx.provide(Greeter {
            greeting: self.config.greeting.clone(),
            translate,
        })?;
        Ok(())
    }
}
```

Where the configuration comes from: a test writes it (step 6); in a host, the host passes it in (step 7). When a host mounts plugins from the rows of `rutis.json` with rutis-loader, the row's `config` is decoded as `Config`, and `#[serde(default)]` fills the fields left out; `Config` then also derives `schemars::JsonSchema`, see [mounted by rutis-loader from configuration](#mounted-by-rutis-loader-from-configuration).

## 5. Emit and listen to events

An event is a type: `NAME` is its name in every language, and its fields are encoded with serde. `Greeter` keeps the plugin's `ctx` and emits `greeter/greeted` from `hello` with `ctx.emit`; the plugin listens to the same event with `ctx.on` in `apply`, counting the greetings, and `count()` returns the count:

<!-- example: rust-en file=greeter/src/lib.rs -->
```rust
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use rutis::{Ctx, Event, Plugin, Result};
use serde::{Deserialize, Serialize};

/// The service greeter uses, provided by another plugin.
pub trait Translate: Send + Sync {
    fn text(&self, text: &str) -> String;
}

/// The configuration. A field left out takes its value from Default.
#[derive(Clone, Deserialize)]
#[serde(default)]
pub struct Config {
    pub greeting: String,
}

impl Default for Config {
    fn default() -> Self {
        Config { greeting: "Hello".into() }
    }
}

/// Emitted on every greeting, with the name.
#[derive(Serialize, Deserialize)]
pub struct Greeted(pub String);

impl Event for Greeted {
    const NAME: &'static str = "greeter/greeted";
    type Value = (); // emitted only, no result asked for
}

/// The service greeter provides.
pub struct Greeter {
    greeting: String,
    translate: Arc<dyn Translate>,
    greeted: Arc<AtomicU64>,
    ctx: Ctx,
}

impl Greeter {
    pub fn hello(&self, name: &str) -> Result<String> {
        self.ctx.emit(Greeted(name.into()))?; // returns at once, without waiting for listeners
        Ok(format!("{}, {name}!", self.translate.text(&self.greeting)))
    }

    pub fn count(&self) -> u64 {
        self.greeted.load(Ordering::SeqCst)
    }
}

/// The plugin.
pub struct GreeterPlugin {
    config: Config,
}

impl GreeterPlugin {
    pub fn new(config: &Config) -> Result<Self> {
        Ok(GreeterPlugin { config: config.clone() })
    }
}

impl Plugin for GreeterPlugin {
    type Deps = Arc<dyn Translate>;

    async fn apply(&self, ctx: &Ctx, translate: Arc<dyn Translate>) -> Result {
        let greeted = Arc::new(AtomicU64::new(0));
        let count = greeted.clone();
        ctx.on::<Greeted>(move |_, _| {
            count.fetch_add(1, Ordering::SeqCst);
            async { Ok(None) } // Ok(None): no result
        })?;
        ctx.provide(Greeter {
            greeting: self.config.greeting.clone(),
            translate,
            greeted,
            ctx: ctx.clone(),
        })?;
        Ok(())
    }
}
```

- The listeners of every language on a host are on one event bus: a plugin in another language that does `on('greeter/greeted', …)` receives it too, with the name as the argument. A one-field struct is one argument, a tuple struct with two or more fields is that many arguments, and a unit struct has none.
- A service keeping a clone of the plugin's `ctx` is fine: it is a handle, and always emits as this plugin. After the plugin stops, what is registered through it is refused with `InactiveEffect`.
- The other ways to emit (`parallel`, `serial`, `waterfall`, `bail`), `on_sync`, prefix listeners and filters: [plugin API: events](plugin-api.en.md#rust).

## 6. Test

Tests need no separate tool: in a `#[tokio::test]`, create a root `Ctx`, provide a fake `Translate`, mount the plugin, and assert. Creating the root, mounting and shutting down are host methods, so `use rutis::host::*;`. Add a test module at the end of `src/lib.rs`; the whole file is:

<!-- example: rust-en file=greeter/src/lib.rs -->
```rust
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use rutis::{Ctx, Event, Plugin, Result};
use serde::{Deserialize, Serialize};

/// The service greeter uses, provided by another plugin.
pub trait Translate: Send + Sync {
    fn text(&self, text: &str) -> String;
}

/// The configuration. A field left out takes its value from Default.
#[derive(Clone, Deserialize)]
#[serde(default)]
pub struct Config {
    pub greeting: String,
}

impl Default for Config {
    fn default() -> Self {
        Config { greeting: "Hello".into() }
    }
}

/// Emitted on every greeting, with the name.
#[derive(Serialize, Deserialize)]
pub struct Greeted(pub String);

impl Event for Greeted {
    const NAME: &'static str = "greeter/greeted";
    type Value = (); // emitted only, no result asked for
}

/// The service greeter provides.
pub struct Greeter {
    greeting: String,
    translate: Arc<dyn Translate>,
    greeted: Arc<AtomicU64>,
    ctx: Ctx,
}

impl Greeter {
    pub fn hello(&self, name: &str) -> Result<String> {
        self.ctx.emit(Greeted(name.into()))?; // returns at once, without waiting for listeners
        Ok(format!("{}, {name}!", self.translate.text(&self.greeting)))
    }

    pub fn count(&self) -> u64 {
        self.greeted.load(Ordering::SeqCst)
    }
}

/// The plugin.
pub struct GreeterPlugin {
    config: Config,
}

impl GreeterPlugin {
    pub fn new(config: &Config) -> Result<Self> {
        Ok(GreeterPlugin { config: config.clone() })
    }
}

impl Plugin for GreeterPlugin {
    type Deps = Arc<dyn Translate>;

    async fn apply(&self, ctx: &Ctx, translate: Arc<dyn Translate>) -> Result {
        let greeted = Arc::new(AtomicU64::new(0));
        let count = greeted.clone();
        ctx.on::<Greeted>(move |_, _| {
            count.fetch_add(1, Ordering::SeqCst);
            async { Ok(None) } // Ok(None): no result
        })?;
        ctx.provide(Greeter {
            greeting: self.config.greeting.clone(),
            translate,
            greeted,
            ctx: ctx.clone(),
        })?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rutis::host::*;
    use tokio::sync::mpsc;

    /// A fake Translate: knows two words, returns the rest as they are.
    struct FakeTranslate;

    impl Translate for FakeTranslate {
        fn text(&self, text: &str) -> String {
            match text {
                "Hello" => "Bonjour".into(),
                "Hi" => "Salut".into(),
                other => other.into(),
            }
        }
    }

    #[tokio::test]
    async fn greets_with_the_configured_greeting() -> Result {
        let root = Ctx::root()?;
        root.provide_as::<dyn Translate>(Arc::new(FakeTranslate))?;
        let plugin = root.plugin(GreeterPlugin::new(&Config { greeting: "Hi".into() })?);
        (&plugin).await?; // apply has run

        // Listen on the root, as a host does, to the events the plugin emits
        let (sender, mut greeted) = mpsc::unbounded_channel();
        root.on::<Greeted>(move |event, _| {
            let _ = sender.send(event.0.clone());
            async { Ok(None) }
        })?;

        let greeter = root.get::<Greeter>().unwrap();
        assert_eq!(greeter.hello("Ada")?, "Salut, Ada!");
        assert_eq!(greeted.recv().await.as_deref(), Some("Ada"));
        assert_eq!(greeter.count(), 1);

        plugin.dispose().await?; // stop: the service and the listener are withdrawn
        assert!(root.get::<Greeter>().is_none());
        root.shutdown().await?;
        Ok(())
    }
}
```

<!-- example: rust-en run -->
```bash
cargo test
```

- `root.provide_as::<dyn Translate>(…)` provides a service under a trait object type; `root.get::<T>()` gets a service, `None` when there is none.
- `(&plugin).await` returns once the plugin's mount has been handled. A plugin whose dependencies are missing is not running then, so provide the fake services first, then mount the plugin.
- `emit` does not wait for listeners, so the test waits for its own listener to receive the event before looking at `count()`.
- A failing `emit` listener goes to the root's error sink; to assert on those in a test, create the root with `Ctx::root_with_sink(handle, sink)`.

## 7. Run it in a host

A Rust host mounts Rust plugins. Write a minimal host in the project's `examples/`: create the root, mount a plugin that provides a fake `Translate` and `greeter`, and call it once. `examples/host.rs`:

<!-- example: rust-en file=greeter/examples/host.rs -->
```rust,ignore
use std::sync::Arc;

use greeter::{Config, Greeter, GreeterPlugin, Translate};
use rutis::host::*;
use rutis::{Ctx, Plugin, Result};

/// A fake Translate: knows two words, returns the rest as they are.
struct FakeTranslate;

impl Translate for FakeTranslate {
    fn text(&self, text: &str) -> String {
        match text {
            "Hello" => "Bonjour".into(),
            "Hi" => "Salut".into(),
            other => other.into(),
        }
    }
}

/// The plugin that provides the fake Translate.
struct FakeTranslatePlugin;

impl Plugin for FakeTranslatePlugin {
    type Deps = ();

    async fn apply(&self, ctx: &Ctx, _: ()) -> Result {
        ctx.provide_as::<dyn Translate>(Arc::new(FakeTranslate))?;
        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result {
    let root = Ctx::root()?; // on the current tokio runtime
    root.plugin(FakeTranslatePlugin).await?;
    root.plugin(GreeterPlugin::new(&Config::default())?).await?;

    let greeter = root.get::<Greeter>().unwrap();
    println!("{}", greeter.hello("Ada")?);
    root.shutdown().await?; // waits for every plugin to stop
    Ok(())
}
```

<!-- example: rust-en run -->
```bash
cargo run --example host
```

```console
$ cargo run --example host
Bonjour, Ada!
```

- The host decides which plugins to mount and when: `root.plugin(plugin)` mounts one, `.await` on the returned handle waits for its mount to be handled, `dispose()` unmounts it, `root.shutdown()` unmounts them all.
- The order does not matter: mounted first, `greeter` waits, and runs `apply` once `Translate` appears. Above, the fake `Translate` is mounted first, so `greeter` is usable as soon as it is mounted.
- After changing the plugin's code, run `cargo run --example host` again: a Rust plugin is compiled with its host, and is not reloaded while running.
- A host can also mount plugins in other languages from `rutis.json` with rutis-loader, calling each other with the Rust plugins on the same event bus: see [embed rutis in a Rust application](rust-host.en.md#runtime-and-loader).

## 8. Publish

Publish to crates.io: add `description` and `license` to `Cargo.toml` (crates.io requires them); the package name is global on crates.io, for example `your-name-greeter`. The dependency range is `rutis = "0.19"`, that is `>=0.19.0, <0.20.0`: a 0.x minor release of rutis may make incompatible changes.

```toml
[package]
name = "your-name-greeter"
version = "0.1.0"
edition = "2024"
description = "Greets in the configured language"
license = "MIT"

[lib]
name = "greeter"

[dependencies]
rutis = "0.19"
serde = { version = "1", features = ["derive"] }

[dev-dependencies]
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
```

Test and publish on a `v*` tag (set `CARGO_REGISTRY_TOKEN` in the repository's secrets):

<!-- example: rust-en file=greeter/.github/workflows/publish.yml -->
```yaml
name: publish

on:
  push:
    tags: ["v*"]

permissions:
  contents: read

jobs:
  publish:
    runs-on: ubuntu-24.04
    steps:
      - uses: actions/checkout@v4
      - run: cargo test
      - run: cargo publish
        env:
          CARGO_REGISTRY_TOKEN: ${{ secrets.CARGO_REGISTRY_TOKEN }}
```

How a host mounts your plugin: it adds it to its `[dependencies]` and mounts it with `root.plugin(…)` in its code, or registers it for rutis-loader to mount from configuration, see [embed rutis in a Rust application: mount Rust plugins](rust-host.en.md#mount-rust-plugins).

## Debugging

The plugin runs in the host's process: run the host (`target/debug/examples/host` from step 7, or the test binary of `cargo test`) under an IDE, `rust-lldb` or `rust-gdb`, and set breakpoints in the plugin's code. How to see each row's state in the host, the providers of services and the calls in flight: [embed rutis in a Rust application: debugging](rust-host.en.md#debugging).

## Rust specifics

### Serve a service to other languages

Mark the service type's `impl` block `#[rutis::service]` and provide it as usual; rows in every language can then call it by name. The name comes from the type (`Calendar` is `calendar`); `#[service(name = "…")]` changes it. The host links `rutis-bridge`:

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

Unmarked types are not served. What can cross, how errors are thrown, methods taking `&Ctx`: [mark the type `#[rutis::service]`](rust-host.en.md#mark-the-type-rutisservice). A service's parameters take data and functions, not objects with methods: when another language passes an object to such a parameter, the call fails with `bad-argument` ([plugin API: objects with methods](plugin-api.en.md#objects-with-methods)).

### Use a service another language provides, by name

A service a plugin in another language provides (Python's `ctx.provide("translate", …)`, Go's `ctx.Provide("translate", …)`) has no type in Rust and cannot go in `type Deps`: wait for it by name and call it by name. The plugin implements the object-safe form `rutis::host::DynPlugin`, writes `host_key("translate")` in `injects()`, and calls with `rutis_bridge::session::call_service` (`cargo add rutis-bridge`, the version the host uses):

```rust,ignore
use rutis::host::*;
use rutis::Ctx;
use rutis_bridge::session::{call_service, host_key};

struct Greeter;

impl DynPlugin for Greeter {
    fn name(&self) -> &str {
        "greeter"
    }

    // Starts once a service named translate is there, stops when it goes: as with Deps.
    fn injects(&self) -> Vec<TypeKey> {
        vec![host_key("translate").erase()]
    }

    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, rutis::Result<Effect>> {
        Box::pin(async move {
            let word: String = call_service(ctx, "translate", "text", ("Hello",)).await?;
            println!("{word}, Ada!");                  // Bonjour, Ada!
            Ok(Effect::Done)
        })
    }
}
```

- The arguments are a tuple (`("Hello",)` for one, `()` for none), and the result is decoded as the type you write; a failure is an `Err`, which `?` turns into a `rutis::Error`. The other rules: [call a service by name from Rust](rust-host.en.md#call-a-service-by-name-from-rust).
- It is the same whatever language provides it, in whichever runtime or node.
- A host mounts it directly with `root.plugin(Greeter)`; to have rutis-loader mount it from a row, use `Builtins::register_raw`, see [mounted by rutis-loader from configuration](#mounted-by-rutis-loader-from-configuration).

### What `type Deps` can be

| `type Deps =` | Waits for | Passed |
| --- | --- | --- |
| `()` | nothing | `()` |
| `Arc<T>` | `T` | the service |
| `Option<Arc<T>>` | nothing | the service if visible at load; its arrival or departure later does not reload |
| a tuple of up to eight | all members | each value |

`T` may be a trait object (`Arc<dyn Translate>`). `ctx.get::<T>()` is an optional read: it does not wait and creates no reload relation; services the plugin needs go in `Deps`. For two services of one type told apart by name, see [embed rutis in a Rust application: depend on services by name](rust-host.en.md#depend-on-services-by-name).

The plugin's other optional items: `fn name(&self) -> &str` (its name in logs and diagnostics, by default the last segment of the type name); `fn validate(&self) -> Result` (run at load, before `apply`; an error leaves the plugin Failed and `apply` does not run).

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
        let file = std::fs::File::open(&self.path)?; // io::Error converts with `?`
        ctx.effect(|| {
            let stop = ctx.clone();
            let task = tokio::spawn(async move {
                let mut tick = tokio::time::interval(Duration::from_secs(60));
                loop {
                    tokio::select! {
                        _ = stop.cancelled() => break,         // the plugin starts stopping
                        _ = tick.tick() => store.flush(),
                    }
                }
            });
            async move {
                let _ = task.await;                            // on stop, wait for it to exit
                drop(file);
                Ok(())
            }
        })?;
        Ok(())
    }
}
```

- `ctx.effect(|| { …; async move { … } })` runs the closure now and registers the future it returns as a cleanup; when the plugin stops, it runs with the other registrations in reverse order. When the plugin is already stopping, `effect` (like `provide` and `on`) returns `InactiveEffect` and the closure does not run, so a resource taken inside the closure never leaks.
- Background tasks are `tokio::spawn`: start them in the `effect` closure and wait for them in the cleanup; inside, watch `ctx.cancelled()`, which completes when the plugin starts stopping. How long stopping waits for cleanups is the host's choice: a Rust host's `shutdown()` waits as long as it takes, `shutdown_with_timeout(duration)` limits it.
- A long-running listener watches `call.ctx().cancelled()` the same way. `ctx.plugin(child)` mounts a child plugin that stops with this one; for a plugin to hand over and mount many times, use `rutis::Definition`, see [plugin API: plugin definitions](plugin-api.en.md#plugin-definitions).

### Errors

Every signature has one error type, `rutis::Error`; `rutis::Result<T = ()>` is `Result<T, rutis::Error>`. `?` converts any `std::error::Error + Send + Sync + 'static` into it; `Error::msg("…")` builds one from a string:

```rust
use rutis::{Error, Result};

#[derive(Debug, thiserror::Error)]
#[error("quota exceeded: {used} of {limit}")]
pub struct QuotaExceeded {
    used: u32,
    limit: u32,
}

fn remaining(limit: &str, used: u32) -> Result<u32> {
    let limit: u32 = limit.trim().parse()?;        // ParseIntError converts with `?`
    if limit == 0 {
        return Err(Error::msg("limit must not be 0"));
    }
    if used > limit {
        return Err(QuotaExceeded { used, limit }.into()); // your own error type
    }
    check_name("greeter").map_err(Error::msg)?;     // String is not an error type
    Ok(limit - used)
}

fn check_name(_: &str) -> std::result::Result<(), String> {
    Ok(())
}
```

`anyhow::Error` does not implement `std::error::Error`, so `?` cannot convert it: write `.map_err(|e| Error::msg(format!("{e:#}")))?`. A plugin's error reaches the host wrapped in `PluginFailed`, with the original error as its source, see [embed rutis in a Rust application: errors a host sees](rust-host.en.md#errors-a-host-sees).

### Mounted by rutis-loader from configuration

When a host mounts plugins from the rows of `rutis.json` with rutis-loader, it registers the constructor under a name. The configuration's JSON Schema is generated from `Config`, so `Config` also derives `schemars::JsonSchema` (`cargo add schemars@1`, the version rutis-loader uses):

```rust,ignore
#[derive(Clone, Deserialize, schemars::JsonSchema)]
#[serde(default)]
pub struct Config {
    pub greeting: String,
}

let mut builtins = Builtins::new();
builtins.register("greeter", GreeterPlugin::new); // row: { "id": "greeter", "name": "greeter", "config": { "greeting": "Hi" } }
```

Before `new`, the loader fills the row's configuration with the defaults in the Schema and validates it: a row whose configuration does not validate fails with `config: …`, and the constructor is not called ([plugin API: configuration](plugin-api.en.md#configuration)). The loader, runtimes and rows in other languages: [embed rutis in a Rust application](rust-host.en.md#runtime-and-loader).

A plugin without configuration: make the configuration type an empty struct, and a row without `config` and one with `"config": {}` both mount:

```rust,ignore
#[derive(Deserialize, schemars::JsonSchema)]
pub struct NoConfig {}

builtins.register("clock", |_: &NoConfig| Ok(Clock)); // row: { "id": "clock", "name": "clock" }
```

With `|_: &()|`, a row without `config` mounts, but one with `"config": {}` fails: `clock: config: config should be null, got an object`.

A `DynPlugin` (such as `Greeter` in [use a service by name](#use-a-service-another-language-provides-by-name) above) registers with `register_raw`, given a `PluginFactory<serde_json::Value>`; the services it waits for come from its `injects()`:

```rust,ignore
struct GreeterFactory;

impl PluginFactory<serde_json::Value> for GreeterFactory {
    fn name(&self) -> &str {
        "greeter"
    }
    fn injects(&self) -> Vec<TypeKey> {
        Greeter.injects()
    }
    fn build(&self, _config: &serde_json::Value) -> rutis::Result<Box<dyn DynPlugin>> {
        Ok(Box::new(Greeter))
    }
}

builtins.register_raw("greeter", GreeterFactory, None); // None: no configuration Schema
```


### dylib plugins

The same plugin can also be built as a dylib that a host loads at run time. The crate depends only on `rutis-sdk` and uses rutis through `rutis_sdk::rutis` (`use rutis_sdk::rutis::{Ctx, Plugin, Result};`), then exports the constructor:

```rust,ignore
rutis_sdk::export_plugin!(id: "greeter", build: GreeterPlugin::new);
```

- `build:` takes the same constructors as `Builtins::register`; `schema:` is optional and gives the configuration's JSON Schema.
- Event and configuration types derive with the SDK's serde: `#[derive(rutis_sdk::rutis::serde::Deserialize)]` with `#[serde(crate = "rutis_sdk::rutis::serde")]`.
- A dylib plugin is built with exactly the host's SDK. Workspaces, packaging and troubleshooting: the [external plugin guide](../external-plugin-guide.en.md).

## Advanced: plugin API

Plugins as values, the other ways to emit, scopes, cancellation, how values cross languages: the [plugin API](plugin-api.en.md).
