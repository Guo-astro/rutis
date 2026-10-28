# Build a host open to plugins

[中文](open-host.md)

For: embedding rutis in your own Rust product and opening it to plugins other developers write. The example throughout is `jot`, a command-line note tool: the host provides a `notes` service and opens one extension point, `commands.register(name, handler)`; third parties write commands in TypeScript or Python. The full API reference is in [Rust host](rust-host.en.md).

## 1. Embed

`jot <command> <argument>…` runs a command. The commands are not in jot: each file in `commands/` is a plugin, `.ts` running in the Node runtime and `.py` in the Python runtime; a plugin registers its command with `commands.register` when it starts.

The dependencies:

<!-- example: jot-en file=jot/Cargo.toml -->
```toml
[package]
name = "jot"
version = "0.1.0"
edition = "2021"

[dependencies]
rutis = "0.19"
rutis-loader = { version = "0.19", features = ["node", "python"] }
rutis-bridge = { version = "0.19", features = ["python"] }
serde_json = "1"
tokio = { version = "1", features = ["rt-multi-thread", "macros"] }
```

The host (`src/main.rs`):

<!-- example: jot-en file=jot/src/main.rs -->
```rust,no_run
use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use rutis::host::*;
use rutis::Ctx;
use rutis_bridge::runtime::LocalRuntime;
use rutis_bridge::session;
use rutis_loader::runtime::{RuntimeResolver, RuntimeRowsPlugin};
use rutis_loader::{Chain, Layer, LoaderOptions, LoaderPlugin, Patch, ServiceCatalog};
use serde_json::{json, Value};

/// The notes: one per line of `notes.txt`.
pub struct Notes {
    file: PathBuf,
}

#[rutis::service]                                     // plugins use it by the name "notes"
impl Notes {
    /// Add a note.
    pub fn add(&self, text: String) -> std::io::Result<()> {
        let mut file = std::fs::OpenOptions::new().create(true).append(true).open(&self.file)?;
        writeln!(file, "{text}")
    }

    /// Every note, oldest first.
    pub fn list(&self) -> Vec<String> {
        let text = std::fs::read_to_string(&self.file).unwrap_or_default();
        text.lines().map(str::to_owned).collect()
    }
}

/// A command: given its arguments, the text jot prints.
pub type Run = Arc<dyn Fn(Vec<String>) -> BoxFuture<'static, rutis::Result<String>> + Send + Sync>;

/// The commands plugins add.
#[derive(Clone, Default)]
pub struct Commands(Arc<Mutex<BTreeMap<String, Run>>>);

#[rutis::service]                                     // plugins use it by the name "commands"
impl Commands {
    /// Add the command `name`. It goes when the plugin that added it unloads.
    pub fn register(&self, ctx: &Ctx, name: String, run: Run) -> rutis::Result<()> {
        self.0.lock().unwrap().insert(name.clone(), run);
        let commands = self.0.clone();
        ctx.effect(|| async move {                    // ctx is the calling row's: runs when it unloads
            commands.lock().unwrap().remove(&name);
            Ok(())
        })?;
        Ok(())
    }
}

impl Commands {                                       // not marked: for jot itself
    fn get(&self, name: &str) -> Option<Run> {
        self.0.lock().unwrap().get(name).cloned()
    }

    fn names(&self) -> Vec<String> {
        self.0.lock().unwrap().keys().cloned().collect()
    }
}

/// A row for each file in commands/: .ts runs in Node, .py in Python.
fn command_rows(dir: &Path) -> std::io::Result<Vec<Value>> {
    let mut rows = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        let Some(id) = path.file_stem().and_then(|stem| stem.to_str()) else { continue };
        match path.extension().and_then(|extension| extension.to_str()) {
            Some("ts") => rows.push(json!({ "id": id, "name": path })),
            Some("py") => rows.push(json!({ "id": id, "name": format!("py:{id}") })),
            _ => {}
        }
    }
    Ok(rows)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some((name, args)) = args.split_first() else {
        eprintln!("usage: jot <command> [<argument>...]");
        std::process::exit(2);
    };
    let dir = std::env::current_dir()?;               // absolute paths: each runtime's process starts in a directory of its own

    let root = Ctx::root()?;
    root.provide(Notes { file: dir.join("notes.txt") })?;
    let commands = Commands::default();
    root.provide(commands.clone())?;

    let catalog = ServiceCatalog::new();
    let node = LocalRuntime::node(dir.join("node_modules/@arcships/rutis-runtime"), dir.join("package.json"));
    let python = LocalRuntime::python(dir.join("commands")).interpreter(dir.join(".venv/bin/python"));
    let node_rows = Arc::new(RuntimeResolver::node(node.handle()).with_catalog(&catalog));
    let python_rows = Arc::new(RuntimeResolver::modules(python.handle()).with_catalog(&catalog));
    let runtimes = [node.handle(), python.handle()];
    root.plugin(node);
    root.plugin(python);

    let loader = LoaderPlugin::new(
        Chain::new().with_shared(node_rows.clone()).with_shared(python_rows.clone()),
        LoaderOptions { catalog, ..LoaderOptions::default() },
    );
    let handle = loader.handle();
    root.plugin(loader).await?;
    root.plugin(RuntimeRowsPlugin::new(node_rows));
    root.plugin(RuntimeRowsPlugin::new(python_rows));

    for runtime in &runtimes {
        runtime.ready().await;
    }
    let rows: Vec<Patch> = serde_json::from_value(json!([{ "insert": command_rows(&dir.join("commands"))? }]))?;
    handle.reconcile(vec![Layer::new("commands", rows)], None).await?; // returns once every row's apply has ended

    let code = match commands.get(name) {
        Some(run) => match run(args.to_vec()).await {
            Ok(text) => {
                println!("{text}");
                0
            }
            Err(error) => {
                // A command's own usage error (code "usage") is told to the user as it is
                match rutis_bridge::thrown(&error).filter(|thrown| thrown.code() == Some("usage")) {
                    Some(session::Error::Remote { message, .. }) => eprintln!("jot: {message}"),
                    _ => eprintln!("jot: {name} failed: {error}"),
                }
                1
            }
        },
        None => {
            eprintln!("jot: no command {name} (commands: {})", commands.names().join(", "));
            2
        }
    };
    root.shutdown().await?;
    std::process::exit(code);
}
```

Two commands third parties wrote. A plugin depends on `@arcships/rutis` or `rutis` only, and does not know its host is jot:

<!-- example: jot-en file=jot/commands/count.ts -->
```ts
import { definePlugin } from '@arcships/rutis'

interface Notes {
  list(): Promise<string[]>
}

interface Commands {
  register(name: string, run: (args: string[]) => Promise<string>): Promise<void>
}

export default definePlugin({
  inject: ['notes', 'commands'],
  async apply(ctx) {
    const notes = ctx.use<Notes>('notes')
    await ctx.use<Commands>('commands').register('count', async () => {
      const all = await notes.list()
      const words = all.join(' ').split(/\s+/).filter(Boolean).length
      return `${all.length} notes, ${words} words`
    })
  },
})
```

<!-- example: jot-en file=jot/commands/shout.py -->
```python
from rutis import define_plugin


class UsageError(Exception):
    code = "usage"


async def apply(ctx, config):
    notes = ctx.use("notes")

    async def shout(args):
        if not args:
            raise UsageError("say what to shout: jot shout <text>")
        text = " ".join(args).upper()
        await notes.add(text)
        return f"added: {text}"

    await ctx.use("commands").register("shout", shout)


plugin = define_plugin(apply, inject=["notes", "commands"])
```

Install the two runtimes (Node 22 or later, Python 3.10 or later) and run:

<!-- example: jot-en run -->
```bash
cd jot
npm init -y
npm install @arcships/rutis-runtime @arcships/rutis
python3 -m venv .venv
.venv/bin/pip install rutis
cargo run -q -- shout buy milk
cargo run -q -- count
```

```text
added: BUY MILK
1 notes, 2 words
```

### What each part does

| Part | What it does | Why it is put together so |
| --- | --- | --- |
| `Ctx::root()` | The root of the plugin tree, on the current tokio runtime | The host's own services (`notes`, `commands`) are provided at the root, where every row sees them |
| `root.provide(…)` | Provides a service; a type marked `#[rutis::service]` is also provided by name (section 2) | No list of services to keep |
| `LocalRuntime::node` / `::python` | A language runtime on this machine, one process per runtime; `root.plugin(runtime)` starts it, unloading ends it | It is a plugin, so it ends with the root; its rows wait for it |
| `RuntimeResolver` | Resolves row names to plugins in the runtime: `node` takes npm names and files, `modules` takes `<runtime name>:<module>` (here `py:shout`) | One resolver per runtime |
| `ServiceCatalog` | Service names to keys, one shared by the loader and the resolvers | The services rows wait for by name meet the services the host provides by type |
| `Chain` | Asks its resolvers in order; the first that knows a row name resolves it | Rows of several languages in one configuration |
| `with_shared` | Puts a resolver something else holds too into the `Chain` | The same `RuntimeResolver` also goes to `RuntimeRowsPlugin` |
| `LoaderPlugin` / `handle()` | Loads plugins from rows; `handle()` is the `Loader` you work with afterwards | Take the `handle` before handing the plugin to the root |
| `RuntimeRowsPlugin` | Once a runtime (re)starts, resolves its rows again, so they run with what their plugins declare now | Mounted after the runtime and the loader |
| `Layer`, `Patch` | Where rows come from: layers, later ones overriding earlier ones; a layer is `Patch`es (`insert` adds rows, or one changes fields by `id`) | The host's own rows and the rows the user adds each in a layer (section 4) |
| `root.shutdown()` | Unloads every plugin and waits for each cleanup; the runtime processes end with them | No limit: `shutdown_with_timeout` for one ([default output](rust-host.en.md#default-output)) |

To run plugins on another machine, see [the remote section of rutis-host.en.md](rutis-host.en.md#remote-runtimes).

### Waiting until ready

`reconcile` returns once every row has loaded once: its `apply` ended (here the command is registered by then), or it failed (listed in the report's `failures`, and the default error sink prints a line `<row>: failed: …`). Rows of a runtime still starting wait for it and have no result in the report yet, so wait for `RuntimeHandle::ready()` first. A row waiting for a service nobody provides does not hold `reconcile` up: it keeps waiting, and after 10 s the error sink says once what it waits for.

A row whose `apply` never returns (it waits for a call that never returns) holds `reconcile` up too. rutis puts no limit on `apply`; how long to wait is the host's decision. To see where it is stuck, print the status while waiting:

```rust,no_run
# use rutis_loader::{Layer, Loader};
# async fn demo(handle: Loader, layers: Vec<Layer>) -> Result<(), Box<dyn std::error::Error>> {
use std::time::Duration;

let reconcile = handle.reconcile(layers, None);
tokio::pin!(reconcile);
tokio::select! {
    report = &mut reconcile => { report?; }
    _ = tokio::time::sleep(Duration::from_secs(3)) => {
        eprintln!("jot: still starting after 3s\n{}", handle.status());
        reconcile.await?;                             // keep waiting; or end here
    }
}
# Ok(())
# }
```

With `await new Promise(() => {})` in the `apply` of `commands/stuck.ts`:

```text
jot: still starting after 3s
rows:
  count: running
  stuck: starting
  shout: running
services:
  commands: host
  loader: host
  notes: host
runtimes:
  node: running (pid 57684)
  py: running (pid 57685)
calls:
  none
```

The row `starting` is the `apply` that has not returned; when it is calling a row's service, `calls` shows which call.

### Paths and environment

- Each runtime's process starts in a directory of its own (Python: its project directory, here `commands/`; Node: the runtime's package directory) and takes the paths it is given as they are: write absolute paths, since a relative one counts from that directory.
- `LocalRuntime::python(dir)` imports plugin modules from that directory; choose the interpreter with `.interpreter(…)` (its environment needs `rutis` installed); `.python_path(dir)` puts modules that are not installed, or a source checkout of rutis, ahead on `PYTHONPATH`.
- `LocalRuntime::env(name, value)` adds an environment variable to the runtime's process; to start the runtime's process yourself, build a `rutis_bridge::runtime::Launcher` (`env`, `cwd`, `arg`) and give it to `LocalRuntime::launcher(name, launcher, anchor)`.
- When row names are not files, npm names or `<runtime name>:<module>` (jot letting the user write `wordcount`, say), implement `rutis_loader::Resolver`: resolve the names it knows and return `LoaderError::NotFound` for the others, so the `Chain` asks the next; the types it uses are in `rutis_loader::resolve`. Rust plugins compiled into the host go in `Builtins` ([Mount Rust plugins](rust-host.en.md#mount-rust-plugins)).

## 2. Define the plugin surface

The plugin surface is the services the host offers: jot's are `notes` and `commands`.

### Write services with `#[rutis::service]`

Put `#[rutis::service]` on the `impl` block and provide the value as usual; rows of every language then use it by name (`Notes` and `Commands` above):

- What plugins see is the block's public `&self` methods, named in camelCase; to callers they are all asynchronous (a Promise in TypeScript, awaitable in Python).
- The service name is the type's, first word lowercased (`Notes` is `notes`); `#[service(name = "…")]` renames it.
- `Err(e)` is an error thrown at the caller, with `name`, `code`, `message` and `data`.
- Methods and types not marked stay inside: jot's own `Commands::get` is in another `impl`, not marked.

What can cross and how errors are thrown, in full: [Mark the type `#[rutis::service]`](rust-host.en.md#mark-the-type-rutisservice) and [Exported services: reference](rust-host.en.md#exported-services-reference).

### Registering callbacks

The `run` of `commands.register(name, run)` is a function in the plugin, passed by reference: calling the `Run` Rust holds calls that function in the plugin's process, and its future completes once the Promise (or Python coroutine) has ended; it is `Err` when the plugin throws.

- Write the parameter as a function type (`impl Fn`, `Arc<dyn Fn … + Send + Sync>`, an alias works), returning `BoxFuture<'static, Result<…>>`, with at most four arguments.
- A `&Ctx` parameter is not passed by the plugin: it is the calling row's context. A cleanup registered on it (`ctx.effect`) runs when that row unloads or restarts, so a plugin's command goes when it unloads, with no unregistering in the plugin.
- Called from outside any row (from Rust, from a peer node), a method taking `&Ctx` fails, the error saying it needs the calling row's context.

### Which row calls

`rutis_bridge::session::caller_row()` gives the calling row: `id()` is the loader's row id. For instance, when two plugins register the same command name, refuse the second and say which row has it:

```rust,no_run
# use std::collections::BTreeMap;
# use std::sync::{Arc, Mutex};
# use rutis::host::BoxFuture;
# use rutis::Ctx;
# pub type Run = Arc<dyn Fn(Vec<String>) -> BoxFuture<'static, rutis::Result<String>> + Send + Sync>;
use rutis_bridge::session::caller_row;

/// The commands, each with the row that added it.
#[derive(Clone, Default)]
pub struct Commands(Arc<Mutex<BTreeMap<String, (String, Run)>>>);

#[rutis::service]
impl Commands {
    /// Add the command `name`, unless another plugin has.
    pub fn register(&self, ctx: &Ctx, name: String, run: Run) -> Result<(), String> {
        let row = caller_row().map(|row| row.id().to_owned()).unwrap_or_default();
        let mut commands = self.0.lock().unwrap();
        if let Some((owner, _)) = commands.get(&name).filter(|(owner, _)| *owner != row) {
            return Err(format!("{name} is already added by {owner}"));
        }
        commands.insert(name.clone(), (row, run));
        let all = self.0.clone();
        ctx.effect(|| async move {
            all.lock().unwrap().remove(&name);
            Ok(())
        })
        .map_err(|error| error.to_string())?;
        Ok(())
    }
}
```

Read it in the method body, before the first `.await` (the method above is synchronous). Calls from Rust or a peer node give `None`. The rest is in [Which row calls](rust-host.en.md#which-row-calls).

### Types for plugin authors

`rutis-host types` reads the types marked `#[rutis::service]` from the source and writes TypeScript declarations and Python types ([Declarations](rust-host.en.md#declarations)):

```sh
rutis-host types . -o jot.d.ts --python jot_types.py
```

In the `jot.d.ts` it writes, `Commands` is:

```ts
/** The commands plugins add. */
export interface Commands {
  /**
   * Add the command `name`. It goes when the plugin that added it unloads.
   * @throws {ErrorThrown}
   */
  register(name: string, run: (arg0: string[]) => string | Promise<string>): Promise<void>
}
```

A plugin author writes `ctx.use<Services['commands']>('commands')` (Python: `commands: Commands = ctx.use("commands")`), and the editor, `tsc` and pyright check the calls before they run; without them it runs all the same. Give the two files to plugin authors (section 5 puts them in the template), and write them again when the Rust side changes.

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

## 4. Manage plugins

### Install, upgrade, uninstall

A host embedding rutis installs a plugin's package with the package manager itself, where the runtime finds it, then adds a row:

| Plugin | Where it goes (jot) | The row's `name` |
| --- | --- | --- |
| npm package | `npm install <package>`, into jot's `node_modules` (the directory of `LocalRuntime::node`'s `package.json`) | The package name |
| PyPI package | `.venv/bin/pip install <package>`, into the environment of `.interpreter(…)` | `py:<entry point or module>` |
| A single file | Nothing to install | Its absolute path (Node), `py:<module>` (in the Python project directory) |

Upgrading is installing the new version, then reloading the row (`Loader::reload`, section 5) or restarting its runtime (Node does not import a module it has imported again); uninstalling is removing the row, then the package. rutis-host's `add`, `remove` and `upgrade` do the same for `rutis.json`; where each language installs to and how its rows are named is in [rutis-host: how a host installs plugins](rutis-host.en.md#how-a-host-installs-plugins).

### Add and remove rows, change their config

The commands the user adds go in an editable layer (here called `user`): the `Loader`'s edit methods change that layer only and hand it to `LoaderOptions::persist` to store. jot keeps it in `jot.json`:

```rust,no_run
# use std::path::PathBuf;
# use std::sync::Arc;
# use rutis_loader::{Chain, Layer, Loader, LoaderOptions, LoaderPlugin, Patch, ServiceCatalog};
use rutis::host::BoxFuture;
use rutis_loader::edit::{Edit, Editable, NewEntry, Persist, PersistError, Version};
use rutis_loader::LoaderError;
use serde_json::json;

/// The rows the user added, kept in `jot.json`.
struct JotFile(PathBuf);

impl JotFile {
    fn read(&self) -> String {
        std::fs::read_to_string(&self.0).unwrap_or_else(|_| "[]".into())
    }
}

impl Persist for JotFile {
    fn load<'a>(&'a self, _layer: &'a str) -> BoxFuture<'a, Result<(Vec<Patch>, Version), LoaderError>> {
        Box::pin(async move {
            let text = self.read();
            let patches = serde_json::from_str(&text)
                .map_err(|error| LoaderError::InvalidEntry(format!("jot.json: {error}")))?;
            Ok((patches, Version(text)))
        })
    }

    fn save<'a>(
        &'a self,
        _layer: &'a str,
        expected: &'a Version,
        _edits: &'a [Edit],
        patches: &'a [Patch],
    ) -> BoxFuture<'a, Result<Version, PersistError>> {
        Box::pin(async move {
            if self.read() != expected.0 {
                return Err(PersistError::Conflict);  // changed elsewhere: the loader reads it again and redoes the edit
            }
            let text = serde_json::to_string_pretty(patches).map_err(|e| PersistError::Failed(e.to_string()))?;
            std::fs::write(&self.0, &text).map_err(|e| PersistError::Failed(e.to_string()))?;
            Ok(Version(text))
        })
    }
}

# async fn demo(dir: PathBuf, chain: Chain, catalog: ServiceCatalog, rows: Vec<Patch>, root: rutis::Ctx) -> Result<(), Box<dyn std::error::Error>> {
# use rutis::host::CtxHost;
let file = JotFile(dir.join("jot.json"));
let (user, version) = file.load("user").await?;
let loader = LoaderPlugin::new(chain, LoaderOptions { catalog, persist: Arc::new(file), ..LoaderOptions::default() });
let handle = loader.handle();
root.plugin(loader).await?;
handle
    .reconcile(
        vec![Layer::new("commands", rows), Layer::new("user", user)],
        Some(Editable::new("user", version)),
    )
    .await?;

// jot add-command ./today.ts
let file = std::fs::canonicalize("today.ts")?;
let entry = NewEntry { id: Some("today".into()), name: file.display().to_string(), ..NewEntry::default() };
handle.create(entry, None, None).await?;               // loads and starts it; adds nothing when it cannot start
handle.update("today", json!({ "format": "iso" })).await?;  // a new config: the row restarts
handle.set_disabled("today", true).await?;             // stopped, the row kept
handle.remove("today").await?;                         // unloaded and removed
# Ok(())
# }
```

- Each edit method returns once the rows it affects have loaded; when a new row cannot start (no such plugin, a config that does not match the schema, a failing `apply`), the edit is rolled back and the error returned says why, for instance `a row today already exists`.
- Only rows of the editable layer can be changed: a row of the `commands` layer (a command jot brings) cannot, the error saying the row is not owned by the editable layer.
- Without an editable layer (`reconcile(…, None)`, the jot of section 1), `create` and `remove` fail: `this host keeps no editable configuration, so rows cannot be added or removed`.
- The full row format (groups, `isolate`, `inject`) and the kinds of edit (`Edit`) are in [the rutis-loader design](../design-rutis-loader-2026-10-02.en.md).

### How a row is doing

`handle.get(id)` gives a row's config and state (`EntryStatus`); `handle.status()` gives every row, service, runtime and call in progress, see section 6.

### Plugins that manage plugins

A host using `LoaderPlugin` also provides the service `loader` by name, with the name and shapes of Cordis's `ctx.loader`: `create(row)`, `reload(id)`, `remove(id)`, `status()`. A plugin with `inject: ['loader']` uses it; a command listing the plugins, for instance:

<!-- example: jot-en file=jot/commands/plugins.ts -->
```ts
import { definePlugin } from '@arcships/rutis'

interface Loader {
  status(): Promise<{ rows: { id: string; state: string; reason?: string }[] }>
}

interface Commands {
  register(name: string, run: (args: string[]) => Promise<string>): Promise<void>
}

export default definePlugin({
  inject: ['loader', 'commands'],
  async apply(ctx) {
    const loader = ctx.use<Loader>('loader')
    await ctx.use<Commands>('commands').register('plugins', async () => {
      const { rows } = await loader.status()   // called in the command, not in apply
      return rows.map(row => `${row.id}: ${row.state}`).join('\n')
    })
  },
})
```

<!-- example: jot-en run -->
```bash
cargo run -q -- plugins
```

```text
count: running
plugins: running
shout: running
```

- Call the `loader` methods once `apply` has returned (in service methods, in events): called in a row's own `apply`, one waits for that `apply` to end, and so hangs.
- `create` and `remove` change the editable layer, as the Rust methods above do; without one they fail.
- Any plugin can `inject` it; rutis adds no permission: to keep a plugin from managing plugins, do not install that plugin.
- The methods' arguments, results and error texts are in [Plugin API: managing plugins with `loader`](plugin-api.en.md#managing-plugins-with-loader).

## 5. Serve plugin authors well

### A template and fake services

Plugin authors have to write, test and try a command without jot. Give them a template: the project `rutis-host new` makes, plus three things from the host — the types section 2 generates, a fake `notes` and `commands`, and the rows putting them in `rutis.dev.json`. A plugin author follows it:

<!-- example: jot-command-en run -->
```bash
npx @arcships/rutis-host new count --lang node
cd count
npm install
```

The types the host generated (written by `rutis-host types`, put in the template as they are):

<!-- example: jot-command-en file=count/src/jot.d.ts -->
```ts
// Generated from the service declarations; do not edit.

/** What a method's `Err(e)` throws: named after `e`'s type, `code` its variant, `message` its `Display`, `data` its serde value. */
export interface RustError<Name extends string = string, Code extends string = string, Data = unknown> extends Error {
  readonly name: Name
  readonly code: Code
  readonly data: Data
}

/** The notes: one per line of `notes.txt`. */
export interface Notes {
  /**
   * Add a note.
   * @throws {IoErrorThrown}
   */
  add(text: string): Promise<void>
  /** Every note, oldest first. */
  list(): Promise<string[]>
}

/** The commands plugins add. */
export interface Commands {
  /**
   * Add the command `name`. It goes when the plugin that added it unloads.
   * @throws {ErrorThrown}
   */
  register(name: string, run: (arg0: string[]) => string | Promise<string>): Promise<void>
}

/** What a method returning `Err(Error)` throws. */
export type ErrorThrown = RustError<"Error", string, undefined>

/** What a method returning `Err(io::Error)` throws. */
export type IoErrorThrown = RustError<"Error", "Error", undefined>

/** The exported services by name: `ctx.use<Services['name']>('name')`. */
export interface Services {
  notes: Notes
  commands: Commands
}
```

A fake jot: `notes` with two notes, and `commands` with one more method, `run`, to try a registered command:

<!-- example: jot-command-en file=count/dev/fake-jot.ts -->
```ts
import { definePlugin } from '@arcships/rutis'

const commands = new Map<string, (args: string[]) => Promise<string>>()

export default definePlugin({
  provides: { notes: { add: 'async', list: 'async' }, commands: { register: 'async', run: 'async' } },
  apply(ctx) {
    const notes = ['buy milk', 'call Ada back']
    ctx.provide('notes', {
      add: async (text: string) => { notes.push(text) },
      list: async () => notes,
    })
    ctx.provide('commands', {
      register: async (name: string, run: (args: string[]) => Promise<string>) => { commands.set(name, run) },
      run: async (name: string, args: string[] = []) => commands.get(name)!(args),
    })
  },
})
```

<!-- example: jot-command-en file=count/rutis.dev.json -->
```json
{
  "format": 1,
  "rows": [
    { "id": "count" },
    { "id": "jot", "name": "./dev/fake-jot.ts" }
  ]
}
```

The command the author writes and its test (replace both files of the template together):

<!-- example: jot-command-en file=count/src/index.ts -->
```ts
import { definePlugin } from '@arcships/rutis'
import type { Services } from './jot.js'

export default definePlugin({
  inject: ['notes', 'commands'],
  async apply(ctx) {
    const notes = ctx.use<Services['notes']>('notes')
    await ctx.use<Services['commands']>('commands').register('count', async () => {
      const all = await notes.list()
      const words = all.join(' ').split(/\s+/).filter(Boolean).length
      return `${all.length} notes, ${words} words`
    })
  },
})
```

<!-- example: jot-command-en file=count/test/index.test.ts -->
```ts
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { load } from '@arcships/rutis/testing'
import plugin from '../src/index.ts'

test('counts the notes and their words', async () => {
  const commands = new Map<string, (args: string[]) => Promise<string>>()
  const t = await load(plugin, {
    services: {
      notes: { list: async () => ['buy milk', 'call Ada back'] },
      commands: { register: async (name: string, run: (args: string[]) => Promise<string>) => { commands.set(name, run) } },
    },
  })
  assert.equal(await commands.get('count')!([]), '2 notes, 5 words')
  await t.unload()
})
```

<!-- example: jot-command-en run -->
```bash
npm test
```

<!-- example: jot-command-en run until="count: running" -->
```bash
npx rutis-host dev
```

While `dev` runs, try the command from another terminal:

```console
$ npx rutis-host call commands.run '"count"'
"2 notes, 5 words"
```

- The fakes of a test go in `load`'s `services`; the fakes of `dev` are rows of `rutis.dev.json`.
- Authors writing commands in Python use the `jot_types.py` that `--python` writes; the fake jot can stay in TypeScript: rows of different languages sit together in `rutis.dev.json`.
- The test tools, `dev` and `rutis-host call` are in each language's plugin guide ([TypeScript](typescript-plugin.en.md#3-test), [Python](python-plugin.en.md#3-test)) and in [rutis-host: developing a plugin with `dev`](rutis-host.en.md#developing-a-plugin-with-dev).

### The development loop

When the host itself should take a plugin's new code without restarting (jot growing a `--watch`), have the resolvers forget what they resolved, then reload the row:

```rust,no_run
# use std::sync::Arc;
# use rutis_loader::runtime::RuntimeResolver;
# use rutis_loader::Loader;
# async fn demo(handle: Loader, node_rows: Arc<RuntimeResolver>, python_rows: Arc<RuntimeResolver>) {
use rutis::host::FiberState;
use rutis_loader::report::{cannot_reload, EntryStatus};

let id = "count";                                     // the row whose file changed
node_rows.invalidate_all();
python_rows.invalidate_all();
match handle.reload(id).await {
    Ok(_) => eprintln!("{id}: reloaded"),
    Err(error) => {
        let running = handle
            .get(id)
            .is_some_and(|row| matches!(&row.status, EntryStatus::Running(s) if s.state == FiberState::Active));
        eprintln!("{}", cannot_reload(id, &error, running)); // the line rutis-host dev prints
    }
}
# }
```

- When the new code cannot be imported (a syntax error), the row keeps running the previous version: `count: cannot reload: …; the previous version keeps running`. When it imports but its `apply` fails, the row fails with that error and does not run until the code is fixed.
- Only the row's own entry module is imported again: when a module it imports changed, restart its runtime (`restart()` on the runtime's fiber).
- Rows using a service the row provides restart with it.

### Saying errors in the host's words

A plugin's error reaches the host with its `name`, `code`, `message`, `data`, `stack` and the row it came from (`site`). `rutis_bridge::thrown(&error)` gives them, so the host decides by `code` how to say it, never by the text. The jot of section 1 tells the user an error with the `code` `usage` as it is, and names the command for any other:

```console
$ cargo run -q -- shout
jot: say what to shout: jot shout <text>
```

Without this it prints `jot: shout failed: UsageError: say what to shout: jot shout <text>`.

- The plugin gives the `code`: a string property `code` on the error in TypeScript and Python, a method `Code()` in Go ([Plugin API](plugin-api.en.md)). Say in the documentation for plugin authors which codes the host knows.
- rutis's own refusals carry a `code` too (`rutis_bridge::session::code`: `not-declared`, `unavailable`, `no-method`, `bad-argument`, `bad-value`, `peer-gone`, `bad-config`).
- A row that fails to start, a runtime that ends: the error sink reports these, on standard error by default (`count: failed: …`). To say them in the host's words, replace the sink: `Ctx::root_with_sink(handle, sink)`, see [default output](rust-host.en.md#default-output).

## 6. See the state

### The state now

`Loader::status()` returns a snapshot: each row's state and why (what it waits for, why it failed), each service and the row providing it (`host` for the host's), each runtime's state and process id, and the calls in progress (who calls, which service's method, for how long). It implements `Display`, one item a line, to print as it is (as in "Waiting until ready" in section 1); its fields are for programs:

```rust,no_run
# fn demo(handle: rutis_loader::Loader) {
let status = handle.status();
for row in status.rows.iter().filter(|row| row.reason.is_some()) {
    eprintln!("jot: {} is not running: {}", row.id, row.reason.as_deref().unwrap_or(""));
}
for call in &status.calls {
    eprintln!("jot: {call}");                         // "<caller> → <service>.<method>  12s"
}
# }
```

- It is what `rutis-host status` prints (the format is in [rutis-host: seeing the current state](rutis-host.en.md#seeing-the-current-state)).
- What it cannot see: calls within one runtime, and calls of functions passed by reference (only "host → row" shows); calls from Rust plugins in the process count as `host`.
- Everything about plugins and services, rutis's own plugins included: `root.diagnostics()`, see [Debugging](rust-host.en.md#debugging).

### Acting on changes

To act when something changes (telling the user a command's plugin failed, say), listen to `rutis/row` and `rutis/runtime` at the root:

```rust,no_run
# use rutis::host::CtxHost;
# fn demo(root: &rutis::Ctx) -> Result<(), Box<dyn std::error::Error>> {
use rutis_bridge::events::{RuntimeEvent, RuntimeEventState};
use rutis_loader::report::RowEventState;
use rutis_loader::RowEvent;

root.on::<RowEvent>(|event, _| async move {
    if event.state == RowEventState::Failed {
        eprintln!("jot: {} failed: {}", event.row, event.error.as_deref().unwrap_or(""));
    }
    Ok(None)
})?;
root.on::<RuntimeEvent>(|event, _| async move {
    if event.state == RuntimeEventState::Down {
        eprintln!("jot: runtime {} ended: {}", event.runtime, event.reason.as_deref().unwrap_or(""));
    }
    Ok(None)
})?;
# Ok(())
# }
```

- `RowEvent`: the row `row` is now `pending`, `running`, `failed` (`error` saying why) or `stopped`. `RuntimeEvent`: the runtime `runtime` is up (`running`, also after each reconnect) or ended (`down`, `reason` saying how).
- They are ordinary events on the bus, which plugins of every language receive too; plugins cannot emit `rutis/*`. The other ways to use events are in [Rust host: events](rust-host.en.md#events).
