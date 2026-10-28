# Build a host open to plugins

[中文](open-host.md)

For: embedding rutis in your own Rust product and opening it to plugins other developers write. The example throughout is `jot`, a command-line note tool: the host provides a `notes` service, and third parties write commands in TypeScript or Python.

The other sections of this guide are being written (#671); the full API reference is in [Rust host](rust-host.en.md).

## 3. Keep plugins in check

Third-party plugins are not yours: one may crash, hang, or call a service you did not mean to give it. What follows are options of `LocalRuntime`, all off by default; unset, things are as without them.

### A process per plugin

Plugins in one runtime share its process: one that crashes or hangs stops the others with it. Give each third-party plugin a runtime of its own (`named`), with `RuntimeResolver::modules` for its rows, named `<runtime name>:<npm name or file>`:

```rust,no_run
# use std::sync::Arc;
# use std::time::Duration;
# use rutis::host::*;
# #[tokio::main]
# async fn main() -> Result<(), Box<dyn std::error::Error>> {
# let root = rutis::Ctx::root()?;
# let app = std::env::current_dir()?.join("app");
# struct Command { id: String, dir: std::path::PathBuf }
# let commands: Vec<Command> = Vec::new();
use rutis_bridge::runtime::LocalRuntime;
use rutis_loader::runtime::{RuntimeResolver, RuntimeRowsPlugin};
use rutis_loader::{Chain, LoaderOptions, LoaderPlugin, ServiceCatalog};

let catalog = ServiceCatalog::new();
let runtime_package = app.join("node_modules/@arcships/rutis-runtime");
let mut chain = Chain::new();
let mut resolvers = Vec::new();
let mut fibers = Vec::new();
for command in &commands {                                // a process per third-party command
    let runtime = LocalRuntime::node(&runtime_package, command.dir.join("package.json"))
        .named(&command.id)                               // runtime name: two or more of a-z, 0-9, -
        .call_limit(Duration::from_secs(30))
        .kill_on_timeout()
        .allowed_services(["notes"]);
    let rows = Arc::new(RuntimeResolver::modules(runtime.handle()).with_catalog(&catalog));
    fibers.push(root.plugin(runtime));
    chain = chain.with_shared(rows.clone());
    resolvers.push(rows);
}
let loader = LoaderPlugin::new(chain, LoaderOptions { catalog, ..LoaderOptions::default() });
root.plugin(loader).await?;
for rows in resolvers {
    root.plugin(RuntimeRowsPlugin::new(rows));
}
// a row: { "id": "wordcount", "name": "wordcount:jot-wordcount" }
# Ok(())
# }
```

- Several plugins share a process when their rows have the same prefix.
- `RuntimeResolver::node` is unchanged, still the default Node runtime's (rows without a prefix); it does not take a row name with a runtime prefix, so it takes no other runtime's rows.
- `modules` asks the runtime at every resolution rather than caching by the package's `version`.

### Call limit

`call_limit(limit)` limits each call into the runtime's plugin code: a call by name of a service its rows provide, and a call of a function it handed over (a command handler a plugin registered with `jot`, say). It is the same whether the caller is the host, a plugin of another runtime or a Rust plugin; it is counted from the call being sent to its end, a returned promise included. Calls between plugins of the same runtime do not go through the host and are not limited.

Over the limit, the call is cancelled (the far end is told, as when the caller gives up) and its caller gets an error with code `timeout` and `data` `{runtime, limit, service?, method?}` (`limit` in milliseconds):

```text
chat: call to weather.tomorrow did not finish within 30s (the host's limit for runtime agent); the call was cancelled
a function of wordcount did not finish within 30s (the host's limit for runtime wordcount); the call was cancelled
```

Cancelling does not stop every plugin: a TypeScript promise cannot be cancelled, and a Python `def` method or a JavaScript busy loop holds the whole process. Add `kill_on_timeout()` to end the runtime's process after a call over the limit:

```text
a function of wordcount did not finish within 30s (the host's limit for runtime wordcount); the call was cancelled; runtime wordcount was ended
```

The runtime's state (`Loader::status()`) is `wordcount: down (ended by the host: a function of wordcount did not finish within 30s)`; its rows wait for it, and the other calls in progress in that process fail with `peer-gone`. Whether to restart it is your decision: call `restart()` on the fiber `root.plugin(runtime)` returned:

```rust,no_run
# use rutis::host::*;
# async fn restart(fiber: FiberView) -> Result<(), Box<dyn std::error::Error>> {
fiber.restart().await?;   // a new process with the same options; its rows load again
# Ok(())
# }
```

A caller's own limit (Rust's `tokio::time::timeout`, Python's `asyncio.wait_for` and so on) still works; whichever comes first applies.

### Services a plugin may use

`allowed_services([...])` lists the services from outside the runtime its plugins may use: yours, Rust plugins', those rows of other runtimes provide. Services rows of the same runtime provide each other are not limited, and neither are your own calls into plugins (`call_service`).

A row whose plugin's `inject` holds a service not on the list fails to start, saying why, with code `unavailable`:

```text
wordcount: uses llm, which the host does not let runtime wordcount use (allowed: tools, notes)
```

When plugins use the loader as a service (#185), leave it off the list of the runtimes whose plugins should not.

### Greeting limit

A local runtime's process that has not greeted within 30 s of starting is ended. When a debugger stops it at its first line (`--inspect-brk`), lift the limit with `greeting(Duration::MAX)`:

```rust,no_run
# use std::time::Duration;
# use rutis_bridge::runtime::LocalRuntime;
# let app = std::path::PathBuf::from("/srv/app");
let runtime = LocalRuntime::node(app.join("node_modules/@arcships/rutis-runtime"), app.join("package.json"))
    .named("wordcount")
    .env("NODE_OPTIONS", "--inspect-brk")
    .greeting(Duration::MAX);
```

Without a limit, a process that starts and never greets stays starting; `Loader::status()` shows it:

```text
rows:
  wordcount: waiting for runtime wordcount (starting)
runtimes:
  wordcount: starting (pid 4242)
```

On Windows a process has an id here only once it has connected to the host, so the line reads `wordcount: starting`.

### What is not a limit

- Read interception decides the names services are shared by; it is not a permission. Whether a plugin may use a service is `allowed_services`.
- Plugins in one process can reach each other through the language itself (globals, the module cache), so the list covers only services from outside the process; to keep plugins apart, put them in different runtimes.
- A plugin process has all the rights of the host's user (files, network, child processes). rutis has no sandbox and no memory or CPU limits.

The options are listed in [Rust host: local runtime options](rust-host.en.md#local-runtime-options).
