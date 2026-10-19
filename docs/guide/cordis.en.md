# Cordis

rutis and Cordis (the Node plugin framework) interoperate in two ways:

- **Use a Cordis application as a node:** connect an existing Cordis application to a rutis node so they can use each other's services, share events, and let rutis run plugins inside it. Use `@arcships/rutis-runtime/bridge`.
- **Mount Cordis plugins in a Rust application:** run published Cordis plugins in a real Cordis context without changing their source, and use Rust types generated at build time from their type declarations. Enable the `cordis` feature of `rutis-bridge`.

A Cordis plugin can also run as a row in rutis-loader: name its package in `rutis.json`, with the package unchanged; see [Write a TypeScript plugin](typescript-plugin.en.md#existing-cordis-plugins):

- Cordis rows in the same Node runtime use each other's services by Cordis's native rules, getting the objects themselves.
- The services a row provides reach rutis by themselves: rows of other runtimes, other nodes and Rust plugins use them by name. The methods are the functions of the service object (its own and its prototype chain's, not `constructor`), all called as `async`. When the package's `package.json` declares `rutis.provides`, that declaration applies (to mark some methods `sync`, say).
- When a service of the same name already has another provider (a row of another runtime, a Rust plugin, an import from a node), the row providing it later fails, also when it provides it after loading, and the error names both, for example `service storage is already provided by row prompts: row storage cannot provide it too`.
- `rutis-host check` runs no plugin, so it does not see undeclared services: a service a row depends on that only such rows may provide is a warning, `nothing declares <service>; it may be provided at runtime by …`, not a failure (with `check --strict`, one).

## Events

A Cordis plugin uses Cordis's own event API as before, and it is rutis's event API: `ctx.on`, `ctx.emit`, `ctx.parallel`, `ctx.serial`, `ctx.waterfall`, `ctx.bail`. Cordis listeners register themselves on the host's logical bus, so whether a Cordis plugin runs as a row of rutis.json, mounted in a Rust application, or in a Cordis application acting as a node, its events reach listeners in any language and process by the same rules (by scope), and events emitted elsewhere reach its listeners. A Cordis application acting as a node shares with its peer only the names its `Events` lists in `export` / `import` ([below](#use-a-cordis-application-as-a-node)). The rules are in [plugin API: events](plugin-api.en.md#events).

```ts
export function apply(ctx: Context) {
  // waterfall: next is the last argument; listeners elsewhere (Python, Go, Rust) are on the same chain
  ctx.on('chat/before-send', async (message: string, next: () => Promise<string>) => (await next()).trim())

  ctx.on('chat/sent', (message: string) => { /* a failure here does not reach the emitter */ })
}

// Emit from anywhere in the plugin:
const out = await ctx.waterfall('chat/before-send', text, (text: string) => text)  // the last argument is the terminal
await ctx.parallel('chat/sent', out)       // waits for every listener, wherever it is
```

- **Filters**: Cordis's `Context.filter` is wired to rutis's filters. In `ctx.emit(thisArg, name, ...)` (and the other four), when `thisArg[Context.filter]` is a function it is the filter: a listener in this process is passed its own Context, a listener elsewhere `{ rutis: descriptor }`. Every Context has a read-only `ctx.rutis`, so `c => c.rutis.row === 'chat'` works for both. The descriptor's fields are in [filter](plugin-api.en.md#filter).
- **`prepend` / `global`** are Cordis's own listener options, ordering and exempting listeners elsewhere the same way.
- **Synchronous `bail`**: listeners elsewhere run as synchronous calls; an `async` function or a listener returning a Promise fails it with `AsyncListener` (before, Cordis took the Promise as the result).
- **`parallel`** failures are always an `AggregateError`, its errors in registration order (as in Cordis), with the message `<n> listeners of <name> failed`, including failures of listeners elsewhere. A failing `emit` listener is reported in one line where it runs and does not reach the emitter.
- **`internal/*`** stays each runtime's own: `internal/dispatch` and `internal/service` are local to the process and never cross. Before an event arriving from elsewhere first calls a listener in this process, the runtime fires `internal/dispatch` once for that dispatch, with rutis's mode name (`emit`, `parallel`, `serial`, `waterfall`, `bail`), so `internal/dispatch` validators still see such events.
- **Unload**: the row's `ctx.signal` is aborted and its running listeners are waited for before the cleanups run.

## Use a Cordis application as a node

```bash
npm install @arcships/rutis-runtime
```

```js
import { Link, Export, Import, Host, Events } from '@arcships/rutis-runtime/bridge'

ctx.plugin(Link, { peer: 'main', id: 'mac', dial: 'wss://main.example.com/rutis', token: process.env.RUTIS_TOKEN })  // or listen: 'wss://…'
ctx.plugin(Export, { peer: 'main', services: { calendar: { today: 'sync', later: 'async' } } })
ctx.plugin(Import, { peer: 'main', services: ['clock'] })
ctx.plugin(Host, { peer: 'main' })                     // let main run npm plugins installed here
ctx.plugin(Events, { peer: 'main', export: ['tock'], import: ['session/*'] })
```

- `Link` maintains a connection to one peer and exposes it as `rutisPeer.<id>`. Other features depend on it and are recreated after every reconnect.
- `Export` exposes Cordis services to the peer with the declared method shapes. `Import` registers peer services as Cordis services and rejects a name already used locally.
- `Host` lets the peer load npm plugins installed here, running them with the row's `isolate` and `inject` settings. Enable it only for trusted peers.
- `Events` shares events with the peer by the rules between nodes ([events between nodes](nodes.en.md#events-between-nodes)): for the names in `export`, events this application emits reach the peer's listeners; for the names in `import`, this application's listeners receive the events the peer emits. An item is a name, or a prefix ending in `*`. The five modes, filters and every value type work; events travel one hop. The `out` / `in` of before 0.13 are refused.
- Credentials can also come from `RUTIS_TOKEN`, `RUTIS_CA`, `RUTIS_CERT`, and `RUTIS_KEY`.

For the rutis node configuration, see [Connect nodes](nodes.en.md).

## Mount Cordis plugins in a Rust application

Currently supported on Unix (Linux and macOS), with Node 22 or later. See the [compatibility layer design](../design-protocol-plugin-mount.en.md) for design boundaries and [`examples/dsh-baseline`](../../examples/dsh-baseline) for a complete example.

### 1. Prepare an npm project

Create an npm project beside the application and install the plugins to mount and the [`@arcships/rutis-runtime`](https://www.npmjs.com/package/@arcships/rutis-runtime) runtime (source is in `node/rutis-runtime` in this repository). Use the versions in this project's lockfile. The runtime protocol version must match the crate; this is checked during the build.

```json
{
  "private": true,
  "type": "module",
  "dependencies": {
    "@deepseek-ai/cordis": "4.0.4",
    "@deepseek-ai/dsh-credentials-local": "0.2.0-rc.1",
    "@arcships/rutis-runtime": "0.13.0"
  }
}
```

```sh
npm --prefix cordis ci
```

The build does not install npm dependencies automatically. It fails with the required command if a package is missing. During development in this repository, you can use a source reference such as `"file:../path/to/rutis/node/rutis-runtime"`.

### 2. Declare mounts in `Cargo.toml`

```toml
[dependencies]
rutis = "…"
rutis-bridge = { version = "0.13", features = ["cordis"] }
tokio = { version = "1", features = ["full"] }

[build-dependencies]
rutis-bridge = { version = "0.13", features = ["cordis"] }

[package.metadata.rutis-cordis]
npm = "cordis"                   # npm project from step 1, relative to Cargo.toml
# runtime = "…"                  # defaults to <npm>/node_modules/@arcships/rutis-runtime

# Each mount generates one module
[package.metadata.rutis-cordis.mounts.credentials]
plugin = "@deepseek-ai/dsh-credentials-local"   # npm package, or path = "src/plugin.ts"
version = "0.2.0-rc.1"           # optional: build fails if the installed version differs
# provide = ["…"]                # services provided to the plugin by the rutis application

# Combined mount: dependent plugins share a single Cordis Context
[package.metadata.rutis-cordis.mounts.workspace]
group = [
    { name = "storage", plugin = "@deepseek-ai/dsh-storage" },
    { name = "storage_json", plugin = "@deepseek-ai/dsh-storage-json" },
    { name = "storage_domain", plugin = "@deepseek-ai/dsh-storage-domain" },
    { name = "sessions", plugin = "@deepseek-ai/dsh-session-persistence-jsonl" },
    { name = "workspace", plugin = "@deepseek-ai/dsh-workspace" },
]
```

In `build.rs`:

```rust
fn main() {
    rutis_bridge::cordis::build::from_manifest().expect("generate Cordis bindings");
}
```

Include all generated mount modules in the application:

```rust
rutis_bridge::include_mounts!();   // generates modules such as credentials and workspace
```

Generated files live in Cargo's build directory and should not be committed or maintained by hand. Cargo regenerates them when a plugin or its types change. Members that cannot be bound are listed as build warnings with their location and reason; the remaining members are still generated.

### 3. Use a mount

```rust
let ctx = rutis::Ctx::root()?;
let view = ctx.plugin(credentials::Plugin::new(credentials::Config {
    dsh_home: Some("/tmp/home".into()),
    ..Default::default()
}));
(&view).await?;

// This is a regular rutis service: dependency gating, replacement, and revocation use native rutis rules.
let store = ctx.require::<credentials::CredentialProvider>()?;
store.set(&credentials::CredentialRef::from("app/api-key"), "s3cret").await?;
```

| Capability | Usage |
| --- | --- |
| Methods | Synchronous methods remain synchronous; Promise-returning methods become `async fn`. They return `Result<T, rutis_bridge::cordis::Error>`; Cordis business errors are `Error::Remote`. |
| Data types | Branded types become newtypes (`CredentialRef::from("…")`); interfaces become structs; literal unions become enums. Optional (`x?: T`) becomes `Option` where `None` sends `undefined`. Required but nullable (`T \| null`) becomes `Option` where `None` sends `null`. Optional and nullable becomes `Option<Option<T>>`. |
| Live objects | Objects with methods (such as `Workspace`) are proxies: property getters read live values, methods call the original object, and passing them back restores the original object. Unions of live objects become `ObjectRef`; unions mixing data and live objects become enums. |
| Callbacks | Function arguments take Rust closures. Returned functions (such as unsubscribe functions) become `RemoteFunction`. |
| Cancellation / timeout | Dropping the returned future cancels the call; Cordis methods receive an `AbortSignal` and abort. For example: `tokio::time::timeout(d, store.read_record(&key)).await`. |
| Events | Nothing to declare: the mount's Cordis listeners and Rust listeners are on one logical bus. On the Rust side define an event type of the same name (`impl Event` with `NAME` the Cordis event's name, deriving serde), or send and receive by name with `EventKey::<rutis_bridge::events::Args>::dynamic(name)`; all five modes work ([embedding in Rust: events](rust-host.en.md#events)). `events` / `emits` of the manifest were removed in 0.13. |
| Host services | Services in `provide` generate traits (such as `SystemPromptHost`). Implement the trait and register it with the generated `provide_system_prompt(&ctx, host)` function. The mount waits until the service is ready. |

### Deployment

Manifest-driven mounts locate the runtime and plugins relative to the npm project. When moving the binary to another machine or directory, deploy the npm project with its resolved `node_modules` and point `RUTIS_CORDIS_ROOT` to it:

```sh
cp -RL cordis /opt/app/cordis        # -L expands symlinks such as file: dependencies
RUTIS_CORDIS_ROOT=/opt/app/cordis /opt/app/my-app
```

If unset, the build-time location is used, so development needs no extra configuration. TypeScript source files mounted with `path` should be inside the npm project and copied with it. Use the default runtime at `node_modules/@arcships/rutis-runtime` (leave `runtime` unset) so the project can move as a unit.

### 4. Cordis plugin boundaries

Some behavior provided by the JavaScript call stack cannot be preserved across processes. The rules are in [Requirements §5](../requirements-protocol-plugins.en.md). Events are not among them: the five modes work across processes as in one (see [events](#events) above), and the only thing they cannot do, a synchronous `bail` waiting in a cycle or on a far end that went away, fails with an error. The main points are:

- After direct `ctx.set` replacement, rutis switches to the new object on the next call to that service.
- A synchronous method cannot wait during execution for a result that requires the Node event loop; this returns `SyncWaitCycle`.
- A host-provided service is a proxy on the Cordis side, so `instanceof` checks fail.
- A plugin must not block the event loop for long periods. The compatibility layer does not impose call timeouts; use an async method with `tokio::time::timeout` when needed. A timeout cancels the call.
- An uncaught exception or unhandled Promise rejection in a plugin ends the whole Node process under Node's default behavior, stopping every plugin in the same mount. The mount's services are then revoked and rutis plugins that depend on them stop waiting. Calls return `Error::Transport`, which reports how the process ended. To recover, the application unloads and mounts it again.

### 5. Common build errors

| Error | Resolution |
| --- | --- |
| `the Cordis plugins are not installed: run npm --prefix … ci` | Install the npm project's dependencies. |
| `… is not installed: add it to …/package.json` | Add the plugin to the npm project and install it. |
| `… 1.0.0 is installed, 2.0.0 is required` | Make the npm project match the configured `version`. |
| `… speaks protocol N, this rutis-bridge speaks M` | Install an `@arcships/rutis-runtime` version that matches the crate. |
| `native plugin dependencies are unresolved: … (name)` (at runtime) | Put the missing dependency in the same `group`, or provide it from rutis with `provide`. |
