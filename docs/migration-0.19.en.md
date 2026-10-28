# Migrating to 0.19

[中文](migration-0.19.md)

0.19 is for host owners. Three changes are incompatible: how remote runtimes connect (the first section below), the host side's tokens, and the platform cleanup (the last section: removed forms are removed outright, not deprecated, with no conversion or hint); the rest are new options, off by default, whose sections are added as 0.19's changes land.

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
| `"transport": "<kind>"` | `"transport": "<kind>"` (`websocket` by default) |

- Many runtimes (`listen` + `instances`) are written as before.
- **The old form still works; nothing needs to change, and nothing is converted.** Do not write both for one runtime: a `runtimes.remote` entry with `dial` or `listen` and a node row whose `"runtime"` names it stop the host at startup:

  ```
  remote runtime gpu: link row gpu connects it too: remove the link row, as runtimes.remote connects it itself
  ```

- The node row's `export`, `import`, `host`, `rows` and similar fields have no counterpart in the new form: its connection only runs that runtime's rows. Keep the node row where you need them.
- The token is `RUTIS_TOKEN_<NAME>` (see [Tokens on the host side](#tokens-on-the-host-side-695)). One runtime's `listen` without a token stops the host at startup.
- A connection of your own (a relay, QUIC, …) implements `Transport`, which the Rust program embedding rutis-host mounts (`Embedding`'s new `transports`), and `transport` names it; one runtime and many both use it.

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

- `rutis_host::host::Embedding` has a new field, `transports`. Code that writes every field in a struct literal adds `transports: Vec::new()`, or uses `..Embedding::default()`.
- `rutis_host::control::serve` takes one more argument after the `Loader`: a `Report` (a function returning the status text, which `status` is answered with); `Host::status_report()` gives it.
- The runtime side, the session protocol, and the Rust APIs of rutis, rutis-loader and rutis-bridge do not change.

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

- Import the advanced names explicitly (the IDE's replacement adds explicit imports). In 0.19 both the old and the new names exist, so with both `import arcships.rutis.*` and `import arcships.rutis.advanced.*` the compiler reports `Overload resolution ambiguity` for `RutisFormat`, `Scope` and the like. Old code with only `import arcships.rutis.*` is not affected.
- These classes changed packages. A Kotlin plugin library published on its own, already compiled, has to be recompiled with the 0.19 SDK before it goes into an application with the 0.19 SDK (its source does not change).

Unchanged: `definePlugin` / `define_plugin` / `Define` / `Rutis.define` at each language's root, every method on `ctx`, the testing tools, Rust. The code Swift macros and Kotlin KSP generate is updated with the SDK and needs no change; the code 0.17.2's `rutis-ksp` generated still compiles against the 0.19 SDK, with deprecation warnings only. The plugin API version is unchanged: plugins built with an older SDK run on the 0.19 runtimes as before.

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
| `waterfall_sync` | Use `waterfall` (asynchronous) or the synchronous `bail` |

### Tools

- `rutis-dev` (the dev channel) is removed; use `rutis-host dev` during development.
- `rutis-migrate` is no longer released with 0.19. The 0.17 migration still works with `cargo install rutis-migrate --version 0.18.0`.
