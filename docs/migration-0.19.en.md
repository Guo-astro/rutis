# Migrating to 0.19

[中文](migration-0.19.md)

0.19 is for host owners. The one incompatible change is how remote runtimes connect (the first section below); the rest are new options, off by default, whose sections are added as 0.19's changes land.

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
- Tokens are as before: `RUTIS_TOKEN_<NAME>`, else `RUTIS_TOKEN`. One runtime's `listen` without a token stops the host at startup.
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
- `rutis_host::control::serve` takes a `Report` (a function returning the status text) instead of a `Loader` as its second argument; `Host::status_report()` gives it.
- The runtime side, the session protocol, and the Rust APIs of rutis, rutis-loader and rutis-bridge do not change.
