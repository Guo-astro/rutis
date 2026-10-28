# Build a host open to plugins

[中文](open-host.md)

For: embedding rutis in your own Rust product and opening it to plugins other developers write. The example throughout is `jot`, a command-line note tool: the host provides a `notes` service, and third parties write commands in TypeScript or Python.

The other sections of this guide are being written (#671); the full API reference is in [Rust host](rust-host.en.md).

## 3. Keep plugins in check

Third-party plugins are not yours: one may crash or hang. What rutis gives you is putting plugins in processes of their own, and a greeting limit for local runtimes; limiting calls and which services a plugin may use are yours to decide (see the end of this section).

### A process per plugin

Plugins in one runtime share its process: one that crashes or hangs stops the others with it. Give each third-party plugin a runtime of its own (`named`), with `RuntimeResolver::modules` for its rows, named `<runtime name>:<npm name or file>`:

```rust,no_run
# use std::sync::Arc;
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
        .named(&command.id);                              // runtime name: two or more of a-z, 0-9, -
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

- rutis does not limit calls: the caller limits its own (Rust's `tokio::time::timeout`, Python's `asyncio.wait_for` and so on). For a plugin stuck in synchronous code that a cancellation cannot stop, end its process with `kill()` on `LocalRuntime::processes()`, then call `restart()` on the fiber `root.plugin(runtime)` returned: a new process, its rows loaded again.
- rutis does not decide which services a plugin may use: which services go to which plugin is your business, yours to hand out.
- Services are shared by name; that is not a permission.
- Plugins in one process can reach each other through the language itself (globals, the module cache); to keep plugins apart, put them in different runtimes.
- A plugin process has all the rights of the host's user (files, network, child processes). rutis has no sandbox and no memory or CPU limits.

The options are listed in [Rust host: local runtime options](rust-host.en.md#local-runtime-options).
