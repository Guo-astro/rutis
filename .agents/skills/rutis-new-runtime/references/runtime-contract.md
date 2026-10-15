# The rutis runtime contract (summary)

> Compiled from the rutis implementation and design docs on main (2026-10-14) and shipped with this skill so it can be used on its own. The code and the originals are authoritative:
> multi-language design https://github.com/arcships/rutis/blob/main/docs/design-multilang-runtimes-2026-10-03.en.md ·
> remote plugins https://github.com/arcships/rutis/blob/main/docs/design-remote-plugins-2026-10-03.en.md ·
> reference implementations: Python https://github.com/arcships/rutis/tree/main/python/rutis/rutis , Go https://github.com/arcships/rutis/tree/main/go/rutis

A runtime is a process (or a remote connection) that holds one **session** with rutis: a handshake, then call frames in both directions. rutis drives the runtime with control operations to load and unload plugins; services the plugins provide are published by rutis under their names, and plugins in any language use them by name.

## 1. Frames and channels

- Frames: newline-delimited compact JSON; a message is at most 16 MiB, beyond which the channel is closed.
- Local channels (given as an argument when rutis starts the process):

  | channel | How |
  | --- | --- |
  | `fd:3` | One end of an inherited socket pair |
  | `unix:<path>` or a bare path | Dial back to a socket rutis created in a private directory |
  | `tcp:127.0.0.1:<port>` | Dial back to a local port; first send the value of the environment variable `RUTIS_CHANNEL_TOKEN` and a newline, then remove it from the environment (used on Windows) |

- Remote channels: `listen:wss://…` (wait for rutis to dial in) and `dial:wss://…` (connect to a rutis access point; Python implements it). WebSocket subprotocol `rutis.3`. Non-loopback requires TLS (`RUTIS_CERT` / `RUTIS_KEY`); the credential `RUTIS_TOKEN` is compared in constant time; 30-second heartbeat (`RUTIS_HEARTBEAT` overrides); one controller at a time, a new connection takes over from the old one.
- Dialing out: present `RUTIS_TOKEN` as a Bearer token, trust `RUTIS_CA` besides the system roots, greet with `--id` declaring `runtime`; refuse a non-loopback `ws://` and an invalid id at start. Classify failures like rutis-bridge's dialer: 401 / 403 at the upgrade or close code 4003 after the hello → retry every 30 s; 400 / 404 / 405 / 426 or a subprotocol mismatch → stop, exit status 1; anything else (4002 included) → backoff 0.5 s doubling to 30 s, reset after a 60-second session (`RUTIS_RETRY=<initial ms>,<max ms>,<stable ms>,<rejected ms>` shortens them for tests). Unload every row (cleanups in reverse) on each disconnect; SIGTERM unloads, closes and exits 0. State lines on stderr: `rutis: connected to …`, `rutis: cannot connect (…); retrying in <s>s`, `rutis: connection lost (…); retrying in <s>s`, `rutis: refused: auth-rejected (…); retrying in <s>s`, `rutis: refused: incompatible (…); stopping`. `dial_conformance.rs` checks all of it.
- Process arguments: `<channel> [--id <endpoint>] [--peer <endpoint>] <project>`.

## 2. Handshake

| Format | Used for | Handshake |
| --- | --- | --- |
| compat, protocol 2 | Local channels | `{"op":"hello","version":2,"capabilities":[…]}` |
| endpoint, protocol 3 | Remote | Local and peer endpoint ids, implementation name and version, capabilities; a remote runtime also declares the capability `runtime` (a rutis link with `runtime` checks it after the handshake and stops with `incompatible` if it is missing) |

### Capabilities (declare what is true; never inferred)

| Capability | Declare it when | If not declared |
| --- | --- | --- |
| `sync-wait` | Synchronous calls you send carry `sync: true` on the frame, and you accept that field | Your synchronous calls are outside the host's wait-cycle detection and can deadlock |
| `reentrant-sync` | While waiting synchronously you still run **any** incoming call | You are treated as non-reentrant (only calls on your call chain run; others are deferred) |
| `sync-stack` | One thread, one stack: calls run while waiting are stacked on top of the waiting one (last in, first out) | You take part only in call-chain edges (runtimes with one execution unit per call do not declare it) |
| `signals` | Signal frames of the endpoint format; every existing implementation (Rust, Node, Python, Bun, Go) declares it | — |
| `objects` | You can receive object references from the peer | You can send references but not receive them |

How to choose: one execution unit per incoming call (thread / goroutine / Task / virtual thread) → `sync-wait` + `reentrant-sync`, no `sync-stack`; single-threaded event loops that run incoming calls while waiting, like Python and Bun → all three; Node is not reentrant → no `reentrant-sync`.

## 3. `mount`

The first control operation from rutis. Reply:

```json
{
  "services": {},
  "features": ["rows.v2", "hosts", "leaf", "scopes", "events"],
  "implementation": { "name": "<package name>", "version": "<SDK version>" },
  "engine": { "name": "<language>", "version": "<language / VM version>" }
}
```

| Feature | Meaning |
| --- | --- |
| `rows.v2` | Supports the row operations below |
| `hosts` | Supports `hosts.provide` / `hosts.withdraw` |
| `leaf` | Leaf runtime: rutis gates every `inject` of its plugins |
| `scopes` | Service ids may carry an instance label: `name`, or `name` + NUL + `label` |
| `events` | Leaf events (plugin API 2): `ctx.on` / `ctx.emit`, and events sent from the host to the runtime; mounting fails if the host configured `emits` and the runtime lacks `events` |

Adding fields to the `mount` reply (such as `engine` or `plugins`) is not a protocol change.

## 4. Control operations

| Operation | Behavior |
| --- | --- |
| `rows.schema(entry)` | `{ config, inject, provides, version }`: config JSON Schema, service names used, service names provided with each method `sync` / `async`; an unknown entry is an error listing the available names |
| `rows.load(key, entry, config, isolate, inject, exports)` | Resolve the entry → check the plugin API version (a plugin newer than the runtime is refused, naming both versions) → decode the config → `apply`; services from `ctx.provide` are announced with `service(id, handle, version)` before the reply; on failure unload the row, then report |
| `rows.update(key, config)` | Leaf plugins have no volatile fields: unload, then load again |
| `rows.unload(key)` | Withdraw provides first (`service(id, None, version)`) → cancel the row's tasks → run cleanups in reverse order |
| `hosts.provide(name, methods, label?)` | Register a proxy for a rutis service so plugins can `ctx.use` it; call target `host:<id>` |
| `hosts.withdraw(id)` | Remove that proxy |
| `release` / `get` | Return an exported object / read a field of one |
| `dispose` | Unload every row and let calls in flight drain |

- Call ids this side originates in the session are prefixed `node:` (the same for every leaf runtime); the call chain `path` is rewritten across sessions on the Rust side.
- Export slots: each `ctx.provide` gets a new handle (`name` for the first generation, then `name#N`).
- When the session ends (rutis is gone): unload every row; cleanup is bounded by `RUTIS_SHUTDOWN_TIMEOUT` (default 10 seconds); past it, write a line to stderr and exit with status 1.

## 5. Values

As in the plugin API's "how values cross" (see `plugin-api.md`): JSON-compatible data is copied; functions, promises / coroutines and objects with methods go by reference; integers within ±(2^53−1); nesting at most 64 levels; NaN, ±Infinity, binary data and the like cannot cross. Values over a limit are refused by the **sender** before sending: only that call fails and the session continues. Errors cross with their name, message and whole error graph (causes, children of aggregate errors).

## 6. Extra rules for compiled-program runtimes (the Go model)

- The executable answers `--rutis-manifest` by printing its manifest to stdout and exiting (no channel, no `apply`):

  ```json
  { "manifest": 1, "sdk": "<version>", "pluginApi": 2, "runtime": "rutis-<language>-runtime:1",
    "platform": "<os>/<arch>", "plugins": { "<name>": { "config": {}, "inject": [], "provides": {}, "version": null } } }
  ```

- The file contains the marker string `rutis-<language>-runtime:1`; when scanning a directory rutis executes only files containing it. Plugin entries are generated by the same code as the `rows.schema` reply.
- Two plugins with the same name in one file: exit at startup naming both definitions. Plugins are registered only through the arguments to `serve(...)`, never through static registries.

## 7. Conformance tests (the contract itself)

The Rust tests in the rutis repository start your runtime and check it item by item:

| Test | Checks |
| --- | --- |
| `rutis-bridge/tests/runtime_conformance.rs` | `session::testing::session`: references, callbacks, errors, a malformed frame ends the session |
| `rutis-bridge/tests/session_matrix.rs` | The runtime contract over every channel |
| `rutis-loader/tests/multilang*.rs` | Cross-language cold starts and calls, withdrawing a provider stops only its users, `inject` gating, crossing synchronous calls do not deadlock |
| `rutis-bridge/tests/websocket_conformance.rs` | Remote listening |
| Test-tool expectations `crates/rutis-bridge/src/runtime/testing/weather.expected.json` | The SDK's host-free test tool and the real runtime give the same results |
