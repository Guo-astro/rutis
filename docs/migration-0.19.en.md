# Migrating to 0.19

[中文](migration-0.19.md)

0.19 is for host owners. Four changes are incompatible: how remote runtimes connect (the first section below), upgrading hosts and runtimes together (session protocol 4), the host side's tokens, and the platform cleanup (the last section: removed forms are removed outright, not deprecated, with no conversion or hint); the rest are new options, off by default, whose sections are added as 0.19's changes land.

## Remote runtimes (#696, #698)

Design: [Remote runtimes: one entry describes a runtime, one runtime and many behave alike](design-remote-runtime-consistency-2026-10-26.en.md).

### The new form: one `runtimes.remote` entry describes it whole

Before, one remote runtime took two places: a `runtimes.remote` entry and a `rutis-bridge/peer` row with `runtime`. Nothing checked the two against each other, and with one of them missing the rows waited forever. From 0.19, one entry says it all:

```json
// 0.18
"runtimes": { "remote": [{ "name": "gpu", "language": "python" }] },
"rows": [
  { "id": "gpu", "name": "rutis-bridge/peer", "config": { "peer": "gpu", "dial": "wss://gpu.example.com:7443/rutis", "runtime": "gpu" } },
  { "id": "embedder", "name": "gpu:embedder" }
]

// 0.19
"runtimes": { "remote": [{ "name": "gpu", "language": "python", "dial": "wss://gpu.example.com:7443/rutis" }] },
"rows": [
  { "id": "embedder", "name": "gpu:embedder" }
]
```

| The 0.18 node row | The 0.19 `runtimes.remote` entry |
| --- | --- |
| `"dial": "<address>"` | `"dial": "<address>"` |
| `"listen": "<listener>"` | `"listen": "<listener>"` (without `instances`: one runtime, whose `--id` is the runtime's name) |

- Many runtimes (`listen` + `instances`) are written as before.
- **The old form still works; nothing needs to change, and nothing is converted.** Do not write both for one runtime: a `runtimes.remote` entry with `dial` or `listen` and a node row whose `"runtime"` names it stop the host at startup:

  ```
  remote runtime gpu: link row gpu connects it too: remove the link row, as runtimes.remote connects it itself
  ```

- The node row's `export`, `import`, `host`, `rows` and similar fields have no counterpart in the new form: its connection only runs that runtime's rows. Keep the node row where you need them.
- The token is `RUTIS_TOKEN_<NAME>` (see [Tokens on the host side](#tokens-on-the-host-side-695)). One runtime's `listen` without a token stops the host at startup.

### What behaves differently

1. **A plugin missing on one remote runtime fails that row alone.** When the remote side does not have a row's plugin, the row used to be unresolved (`unresolved: no plugin named "gpu:embedder"`), and an edit adding such a row was refused. Now it is as with many runtimes: only that row fails, the others run, an edit adding such a row is not refused, and it is checked again the next time the runtime connects. `run` prints:

   ```
   embedder: unresolved: entry "embedder" rejected: runtime gpu: this connection does not contain plugin embedder
   ```

   Rust embeddings telling this case by `LoaderError::NotFound` use `rutis_loader::report::Unavailable::is(&error)` instead.

2. **With many runtimes, the text names the runtime.** `this connection does not contain plugin X` becomes `runtime <name>: this connection does not contain plugin X` (matching on the latter part is not affected).

3. **`listen` without `instances` is no longer an error.** It used to stop the host (`listen needs instances`); now it is one runtime connecting in.

4. **One text for a runtime without the row contract.** One runtime and many both say `the <name> runtime lacks rows.v2 and hosts: a runtime speaking the row contract is required (for Node, @arcships/rutis-runtime 0.3.0 or later)`.

5. **New output.** The new form's connection lines start with `runtime <name>` (an old-form node row still uses its row id); with many runtimes, each connection coming, going and being refused is printed:

   ```
   runtime gpu: link connected
   runtime gpu: link down (gpu.example.com:7443: Connection refused); retrying in 2.1s
   runtime worker: connected worker-1
   runtime worker: disconnected worker-1 (connection lost)
   runtime worker: refused (the worker runtime lacks rows.v2 and hosts: …)
   ```

   `rutis-host status` has a `connections:` part when there are such runtimes. Scripts reading `run`'s output by the prefix of a line are not affected by the new lines.

### Rust embeddings

- `rutis_host::control::serve` takes one more argument after the `Loader`: a `Report` (a function returning the status text, which `status` is answered with); `Host::status_report()` gives it.
- The Rust APIs of rutis and rutis-loader do not change. rutis-bridge's connection and session parts changed (#695, [Remote connections: one session plugin](design-remote-session-2026-10-28.en.md) §3, §11):

  | 0.18 | 0.19 |
  | --- | --- |
  | `rutis_bridge::transport::{local, memory, websocket}` | `rutis_bridge::{pipe, memory, websocket}` |
  | `Transport`, `Dial`, `transport_key`; `LocalPlugin`, `MemoryPlugin`, `WebSocketPlugin` and their `*Transport`; `websocket::Config` | `websocket::dial`, `websocket::listen`, `pipe::spawn` and `memory::pair` give a `Channel` directly |
  | `ListenerConfig::new(name, address, local id)` | `ListenerConfig::new(name, address)` |
  | `Closer::replaced()` / `rejected(r)` / `close(&str)` | `close(Close::Replaced)` / `close(Close::Rejected(r))` / `close(Close::Done)` |
  | `Channel`'s `info` (`ChannelInfo`), `Spawn::new(program, peer)` | removed; the far end's id comes only from the session handshake; `Spawn::new(program)` |
  | `rutis_host::host::Embedding`'s `transports` | removed; a connection of your own becomes a connection plugin (once the remote-connection redesign lands) |
  | `Connection::open(channel, dispatch)`, `Connection::open_with(channel, dispatch, Format::Endpoint(e))`, `Format` | `Connection::open(channel, dispatch, e)`: only the endpoint format, this side always names itself |
  | `Process::attach(channel, mount, Format::Endpoint(e))` | `Process::attach(channel, mount, e)` |
  | `session::PROTOCOL` (2), `cordis::PROTOCOL`; `session::ENDPOINT_PROTOCOL` at 3 | removed; `session::ENDPOINT_PROTOCOL` and `cordis::ENDPOINT_PROTOCOL` are 4 |
  | `LinkConfig`'s `compat` | removed |

- The `transport` field of `rutis-bridge/peer` rows and of `runtimes.remote` is removed: a rutis.json that sets it is refused at startup as an unknown field.

## Upgrade together (#695)

0.19 refuses 0.18 and older hosts, runtimes, nodes and SDKs at the handshake: the session protocol goes from version 3 to 4 and keeps only the endpoint format ([Remote connections: one session plugin](design-remote-session-2026-10-28.en.md) §3). The refusal says which end to upgrade:

- A 0.19 host starting a 0.18 local runtime (which speaks the local protocol 2) prints:

  ```
  incompatible session: the far end speaks protocol 2, this side 4: upgrade the far end
  ```

- A 0.18 node or remote runtime dialing a 0.19 listener prints, on the dialer:

  ```
  this endpoint speaks rutis.4, the dialer offered rutis.3: upgrade the dialer
  ```

- 0.19 dialing a 0.18 listener is refused in the listener's words, for example a 0.18 remote Python runtime:

  ```
  this endpoint speaks rutis.3, the dialer offered rutis.4: upgrade this endpoint (PyPI rutis)
  ```

- A 0.18 host starting a 0.19 local runtime (passing no `--id`): the runtime stops before the handshake:

  ```
  a local channel needs --id <endpoint>: fd:3; a host older than rutis 0.19 started this runtime: upgrade the host
  ```

Upgrade the host and every language runtime and SDK (Node, Bun, Python, Go, Swift, Kotlin) to 0.19 together; rebuild Go and Swift binaries with the 0.19 SDK. Meetings across versions hold only between adjacent releases without incompatible changes.

If you start runtime processes yourself, or implement a runtime:

- Local channels (`fd:3`, a socket path, `tcp:`) take `--id <name>` too: `<channel> --id <name> [--peer <id>] <project>`. The host starts a local runtime with `--id <runtime name>`, and the runtime greets as that name. Without `--id` the runtime refuses to start: `a local channel needs --id <endpoint>: <channel>; a host older than rutis 0.19 started this runtime: upgrade the host`.
- The `--format` argument of the Node and Bun runtimes is removed; `rutisProtocol` in their `package.json` is 4.
- The WebSocket subprotocol is `rutis.4`.
- Go: `advanced.ServeConn(conn, defs...)` becomes `advanced.ServeConn(conn, id, defs...)`; `id` is the name it greets as, as `--id`.

## Tokens on the host side (#695)

**Tokens**: the host side reads only `RUTIS_TOKEN_<NAME>`; a host that set only `RUTIS_TOKEN` sets it by name instead. `RUTIS_TOKEN` on the runtime side is unchanged.

`<NAME>` is a peer row's `peer` or a remote runtime's `name`, in upper case, `-` as `_`:

```bash
# 0.18: one RUTIS_TOKEN for every peer
RUTIS_TOKEN=secret rutis-host run rutis.json
# 0.19: one for each peer and each remote runtime
RUTIS_TOKEN_OFFICE=secret RUTIS_TOKEN_GPU=secret rutis-host run rutis.json
```

What you see when it is not changed:

- A remote runtime that connects in (`listen`) has no token, and the host does not start:

  ```
  rutis-host: remote runtime phone: set RUTIS_TOKEN_PHONE: the token its runtimes present when they dial in
  ```

- A dialing peer row presents no token, the far end refuses it, and `run` retries every 30 s: `office: link refused: auth-rejected (…); retrying in 30s`. A listening peer row without a token refuses the far end when it connects in.

Two entries on one listener whose `RUTIS_TOKEN_<NAME>` variables have the same value still fail before starting (#702); the text now names both variables:

```
rutis-host: listener public: link row main accepts RUTIS_TOKEN_MAIN and remote runtime phone accepts RUTIS_TOKEN_PHONE, the same token, so a connection presenting it cannot be told apart: give RUTIS_TOKEN_MAIN or RUTIS_TOKEN_PHONE another value
```

## The SDKs' default entry points (#692)

From 0.19, the root of each language's SDK keeps only the names plugins are commonly written with; the rest is in advanced: TypeScript / Bun `@arcships/rutis/advanced`, Python `rutis.advanced`, Go `github.com/arcships/rutis/go/rutis/advanced`, Swift `Rutis.Advanced`, Kotlin `arcships.rutis.advanced`. The names only the code Swift macros and Kotlin KSP generate uses are in `Rutis._Generated` and `arcships.rutis.generated`.

**Moved directly, no old names.** The old locations are removed in 0.19: using an old name fails to compile or import, saying the name does not exist; change the import as the table shows. A Python plugin shares the host environment's one `rutis` with the runtime, so it upgrades to 0.19 together with the host.

Only imports change: a name moved off the root (value types, `PLUGIN_API`, the types that describe events, the errors only one way of emitting raises, and so on) is imported from advanced under the same name; the full list is in [SDK entry points](design-sdk-entry-convergence-2026-10-27.en.md) §3. The few names added to the SDKs since follow the same rule: Python's `LoadError` is in `rutis.advanced`; Kotlin's `RemoteObject` in `arcships.rutis.advanced` and `ObjectProxy` in `arcships.rutis.generated`; Swift's `Rutis.located` in `Rutis._Generated`.

| Language | Old form | New form |
| --- | --- | --- |
| TypeScript / Bun | `import { datetime } from '@arcships/rutis'` | `import { datetime } from '@arcships/rutis/advanced'` |
| Python | `from rutis import AggregateError` | `from rutis.advanced import AggregateError` |
| Go | `rutis.DateOf(t)` | Import `github.com/arcships/rutis/go/rutis/advanced` and write `advanced.DateOf(t)` |
| Swift | `let d: Rutis.DateTime` | `let d: Rutis.Advanced.DateTime` |
| Kotlin | `RutisFormat.encode(…)` | `import arcships.rutis.advanced.RutisFormat` |

Two more points for Kotlin:

- Use the same version of `rutis-ksp` as of the SDK: the code a `rutis-ksp` before 0.19 generated refers to the old places at the root and does not compile against the 0.19 SDK.
- These classes changed packages. A Kotlin plugin library published on its own, already compiled, has to be recompiled with the 0.19 SDK before it goes into an application with the 0.19 SDK.

Unchanged: `definePlugin` / `define_plugin` / `Define` / `Rutis.define` at each language's root, every method on `ctx`, the testing tools, Rust. The code Swift macros and Kotlin KSP generate is updated with the SDK and needs no change. The plugin API version is unchanged: plugins built with an older SDK run on the 0.19 runtimes as before.

## Platform cleanup (#694)

0.19 removes what does not belong in the platform ([platform cleanup](design-platform-cleanup-2026-10-28.en.md)). There is no compatibility: a removed Rust API is a compile error.

| 0.18 | 0.19 |
| --- | --- |
| `LocalRuntime::call_limit`, `kill_on_timeout`, `allowed_services`, error code `timeout` | Callers limit their own calls (Rust's `tokio::time::timeout`, Python's `asyncio.wait_for` and so on); when a plugin is still stuck after a timeout, the host ends its process with `processes().kill()` and calls `FiberView::restart()` on the fiber `root.plugin(runtime)` returned; which services a plugin may use is the host's own decision |
| `Loader::set_overlay`, `overlays()`, `LoaderChanged::Overlay` | Removed; to override a row, write it in a layer of your own (the layer list given to `reconcile`) |
| The error name on the wire `CordisError`, the generated type `CordisErrorThrown` | `Error`, `ErrorThrown` |
| `ServiceCatalog::share_by_name()`, `share_registered_only()` | delete the line (sharing by name is the default) |
| `Loader::waiting_for(&entry)` | `Loader::waits(&entry)` (the note on an unmarked Rust service is `WaitReason::Explained`) |
| `Loader::locate(plugin)` | `Loader::row(instance)` |
| `Loader::schema_of(name)` | `loader.resolve(name).await?.schema` |
| `rutis_loader::runtime::go::{GoBinaries, GoManifest, GoBinary, GoResolver, GoRuntimes, GoRuntimesHandle, GoRuntimeState, GoRuntimeInfo}` and the 8 Swift ones of the same names | `rutis_loader::runtime::binary::{Binaries, BinaryManifest, BinaryFile, BinaryResolver<Go>, BinaryRuntimes<Go>, BinaryRuntimesHandle<Go>, BinaryRuntimeState, BinaryRuntimeInfo}`, with `Go` and `Swift` in `runtime::go` and `runtime::swift` |
| `ctx.plugin_from(\|cfg\| …)` | Implement `rutis::host::PluginFactory` and call `ctx.plugin_with(factory, config)` |
| `rutis_loader::status::RowState` (the type of `status::Row::state`) | `rutis_loader::report::EntryStatus`: waiting is `Running` with the `Pending` phase, starting is the `Loading` phase; what `Display` prints is unchanged |
| `rutis_loader::report::RowState` (the values of the `rutis/row` event) | Renamed `rutis_loader::report::RowEventState`; the values are unchanged |
| `take_cleanup_errors()` | A cleanup's error is in the unload result (the error `dispose` returns) |
| `FiberView::effects()`, `EffectMeta`, `EffectPhase` | No replacement; for diagnostics use `diagnostics()` |
| `EventBus::observe_dispatch` | No replacement; to observe, register an ordinary listener |
| feature `anyhow`, `Error::from_anyhow` | `.map_err(\|e\| Error::msg(format!("{e:#}")))` |
| `rutis_bridge::cordis::build::rutis_plugin`, `cordis::server` | Removed (a Cordis host mounting Rust plugins is no longer supported) |
| `waterfall_sync` | Use `waterfall` (asynchronous) or the synchronous `bail` |
| `internal::intercept_require_as`, `intercept_set_as`, `ServiceIntercept`, the `Intercept*` variants of `ServiceReadFailure` / `ServiceWriteFailure` | No replacement |
| `rutis_bridge::build::Exports`, build-dependency `rutis-bridge = { features = ["exports"] }` | `rutis_types::Exports`, build-dependency `rutis-types`; API unchanged |
| `internal::config::{closest, decode, …}` | `rutis::config::closest`; validation `rutis::config::check` |
| `rutis_loader::config::check(schema, config, options)` | `rutis::config::check(schema, config)` |
| `internal::provide_checked`, `refresh`, `DependencyStatus::CheckPending`, `CheckRejected`, `CheckPanicked` | No replacement |

### Tools

- `rutis-dev` (the dev channel) is removed; use `rutis-host dev` during development.
- `rutis-migrate` is no longer released with 0.19. The 0.17 migration still works with `cargo install rutis-migrate --version 0.18.0`.
- `rutis-host go add <module>@<version>` is removed; use `rutis-host add <module>@<version>`, which installs into `runtimes.go.dir` with the same `go install` and adds the row. For the binary without a row, run `GOBIN=<runtimes.go.dir> go install <module>@<version>` yourself.

### Go plugins

Rebuild them with SDK 0.18 or later (the `required` exception for binaries built before 0.18 is removed).

### Python

The minimum is 3.12 (3.10 and 3.11 are no longer supported).
