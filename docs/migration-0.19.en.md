# Migrating to 0.19

[中文](migration-0.19.md)

0.19 is for host owners. The incompatible changes are how remote runtimes connect (the first section below) and the host side's tokens; the rest are new options, off by default, whose sections are added as 0.19's changes land.

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

**Nothing has to change.** The old forms still work in 0.19, and the editor, compiler or tests point to the new place; they are removed only when the plugin API version increases, and that release's notes say so again.

Only imports change: a name moved off the root (value types, `PLUGIN_API`, the types that describe events, the errors only one way of emitting raises, and so on) is imported from advanced under the same name; the full list is in [SDK entry points](design-sdk-entry-convergence-2026-10-27.en.md) §3. The few names added to the SDKs since follow the same rule: Python's `LoadError` is in `rutis.advanced`; Kotlin's `RemoteObject` in `arcships.rutis.advanced` and `ObjectProxy` in `arcships.rutis.generated`; Swift's `Rutis.located` in `Rutis._Generated`.

| Language | Old form | What you see | New form |
| --- | --- | --- | --- |
| TypeScript / Bun | `import { datetime } from '@arcships/rutis'` | Struck through in the editor; on hover: `'datetime' is deprecated. Moved in 0.19: import it from '@arcships/rutis/advanced'. Removed when the plugin API version changes.` | `import { datetime } from '@arcships/rutis/advanced'` |
| Python | `from rutis import AggregateError`, run under pytest | `DeprecationWarning: AggregateError moved in 0.19: import it from rutis.advanced. The old name is removed when the plugin API version changes.` (fails under `-W error`) | `from rutis.advanced import AggregateError` |
| Go | `rutis.DateOf(t)`, run `staticcheck` | `SA1019: rutis.DateOf is deprecated: moved in 0.19: use advanced.DateOf.` | Import `github.com/arcships/rutis/go/rutis/advanced` and write `advanced.DateOf(t)` |
| Swift | `let d: Rutis.DateTime` | `warning: 'DateTime' is deprecated: renamed to 'Rutis.Advanced.DateTime'`, with a fix-it | `let d: Rutis.Advanced.DateTime` |
| Kotlin | `RutisFormat.encode(…)` | `w: 'typealias RutisFormat = RutisFormat' is deprecated. Moved in 0.19: use arcships.rutis.advanced.RutisFormat.`; the IDE replaces it in one step and adds the import | `import arcships.rutis.advanced.RutisFormat` |

Two more points for Kotlin:

- Import the advanced names explicitly (the IDE's replacement adds explicit imports). In 0.19 both the old and the new names exist, so with both `import arcships.rutis.*` and `import arcships.rutis.advanced.*` the compiler reports `Overload resolution ambiguity` for `RutisFormat`, `Scope` and the like. Old code with only `import arcships.rutis.*` is not affected.
- These classes changed packages. A Kotlin plugin library published on its own, already compiled, has to be recompiled with the 0.19 SDK before it goes into an application with the 0.19 SDK (its source does not change).

Unchanged: `definePlugin` / `define_plugin` / `Define` / `Rutis.define` at each language's root, every method on `ctx`, the testing tools, Rust. The code Swift macros and Kotlin KSP generate is updated with the SDK and needs no change; the code 0.17.2's `rutis-ksp` generated still compiles against the 0.19 SDK, with deprecation warnings only. The plugin API version is unchanged: plugins built with an older SDK run on the 0.19 runtimes as before.

## Platform cleanup (#694)

Design: [0.19 platform cleanup](design-platform-cleanup-2026-10-28.en.md). What does not belong to the platform is deleted in 0.19, with no forwarding or old names: code using it fails to compile; change it as below.

### Rust API

| 0.18 | 0.19 |
| --- | --- |
| `ServiceCatalog::share_by_name()`, `share_registered_only()` | delete the line (sharing by name is the default) |
| `Loader::waiting_for(&entry)` | `Loader::waits(&entry)` (the note on an unmarked Rust service is `WaitReason::Explained`) |
| `Loader::locate(plugin)` | `Loader::row(instance)` |
| `Loader::schema_of(name)` | `loader.resolve(name).await?.schema` |
| `rutis_loader::runtime::go::{GoBinaries, GoManifest, GoBinary, GoResolver, GoRuntimes, GoRuntimesHandle, GoRuntimeState, GoRuntimeInfo}` and the 8 Swift ones of the same names | `rutis_loader::runtime::binary::{Binaries, BinaryManifest, BinaryFile, BinaryResolver<Go>, BinaryRuntimes<Go>, BinaryRuntimesHandle<Go>, BinaryRuntimeState, BinaryRuntimeInfo}`, with `Go` and `Swift` in `runtime::go` and `runtime::swift` |
