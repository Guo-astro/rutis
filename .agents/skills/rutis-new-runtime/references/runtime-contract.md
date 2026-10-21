# The rutis runtime contract (summary)

> Compiled from the rutis implementation and design docs on main (2026-10-18) and shipped with this skill so it can be used on its own. The code and the originals are authoritative:
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
| `events-v2` | You implement the event protocol of rutis 0.13 (§6). Mandatory from 0.13: declare it in both handshake formats, and end the session right after the handshake when the far end does not | A 0.13 host ends the session: `incompatible session: the far end (…) does not declare events-v2 (the event protocol of rutis 0.13): upgrade it to rutis 0.13 or later` |

How to choose: one execution unit per incoming call (thread / goroutine / Task / virtual thread) → `sync-wait` + `reentrant-sync`, no `sync-stack`; single-threaded event loops that run incoming calls while waiting, like Python and Bun → all three; Node is not reentrant → no `reentrant-sync`.

## 3. `mount`

The first control operation from rutis. Reply:

```json
{
  "services": {},
  "features": ["rows.v2", "hosts", "leaf", "scopes"],
  "implementation": { "name": "<package name>", "version": "<SDK version>" },
  "engine": { "name": "<language>", "version": "<language / VM version>" }
}
```

| Feature | Meaning |
| --- | --- |
| `rows.v2` | Supports the row operations below |
| `hosts` | Supports `hosts.provide` / `hosts.withdraw` |
| `leaf` | Leaf runtime: rutis gates every `inject` of its plugins |
| `scopes` | Service ids may carry a scope label: `name`, or `name` + NUL + `label` |
| `events` | rutis 0.12 only: leaf events (plugin API 2), `ctx.on` / `ctx.emit` and host events configured with `emits`. Removed in 0.13 together with the `mount` arguments `events` / `emits`; events are §6 |
| `rows.announce` | Services a row provides without declaring them are announced to the host with their methods (see `service` below); only to a host whose `mount` arguments carry `announce: true` (rutis 0.12 and later). Every runtime of the repository does it (Node for Cordis and leaf rows, Bun, Python, Go) |

Adding fields to the `mount` reply (such as `engine` or `plugins`) is not a protocol change.

## 4. Control operations

| Operation | Behavior |
| --- | --- |
| `rows.schema(entry)` | `{ config, inject, provides, version, announces }`: config JSON Schema, service names used, service names provided with each method `sync` / `async`, and `announces: true` when the plugin may provide services `provides` does not list (the host's `check` then reports them as possibly provided at runtime); the Node runtime adds `cordis: true` for a Cordis plugin (not a leaf one), whose dependencies on Cordis rows of its runtime resolve in Cordis, and a leaf runtime leaves it out; an unknown entry is an error listing the available names |
| `rows.load(key, entry, config, isolate, inject, exports, row)` | Resolve the entry → check the plugin API version (a plugin newer than the runtime is refused, naming both versions) → decode the config → `apply`; services from `ctx.provide` are announced with `service(id, handle, version)` before the reply; on failure unload the row, then report. From 0.13 the seventh argument is `{ scope }`, the row's scope chain (§6), `exports` being `{}` when the row exports nothing; answer only once every `events.on` the row sent during `apply` was answered |
| `rows.update(key, config)` | Leaf plugins have no volatile fields: unload, then load again |
| `rows.unload(key)` | Withdraw provides first (`service(id, None, version)`) → cancel the row's tasks → run cleanups in reverse order |
| `hosts.provide(name, methods, label?)` | Register a proxy for a rutis service so plugins can `ctx.use` it; call target `host:<id>` |
| `hosts.withdraw(id)` | Remove that proxy |
| `release` / `get` | Return an exported object / read a field of one |
| `dispose` | Unload every row and let calls in flight drain |

- Scope labels are opaque strings (any text without NUL, not empty): an instance's, a row's configured `isolate` label, or one the host makes for a single row. The host leases each row's services into a scope of that row's own (a label with U+0001 in it), so that the calls to `host:<id>` tell it which row calls (`session::caller_row`, #441). A row therefore reaches a host service only through the id its own `isolate` gives the name, never another row's, and a call from a plugin's code targets the id of the row it runs in.
- Call ids this side originates in the session are prefixed `node:` (the same for every leaf runtime); the call chain `path` is rewritten across sessions on the Rust side.
- Export slots: each `ctx.provide` gets a new handle (`name` for the first generation, then `name#N`).
- Inferring methods: the language's public methods of the object (JavaScript: own and prototype-chain functions up to `Object.prototype`, not `constructor`; Python: callable attributes not starting with `_`, properties not read; Go: exported methods, by wire name), every one `async`; fields are not methods. `rows.schema` answers `announces: true` for every plugin of an SDK that announces (0.12 on), whether or not it declares `provides`: the declared names still count as provided. Mounted without `announce` (a host older than 0.12), a runtime tells a plugin providing an undeclared service, once per name on stderr: `rutis: service <name> is not declared in provides, and this host (older than 0.12) does not take undeclared services; declare it or upgrade rutis-host`. A service provided from another thread before `apply` returns is answered by the host before `rows.load` is.
- `service(id, handle, version)` reports a declared slot. With `rows.announce`, to a host that asked (`announce: true`), a service the row did not declare is reported as `service(id, handle, version, { row: <row key>, methods: { <method>: "async" } })`, the methods inferred from each new object (the language's public methods); the host publishes it, or releases the handle when it does not follow it. Await the host's reply to these before answering `rows.load`, so a name clash fails the load. An older host never gets the fourth argument.
- When the session ends (rutis is gone): unload every row; cleanup is bounded by `RUTIS_SHUTDOWN_TIMEOUT` (default 10 seconds); past it, write a line to stderr and exit with status 1.

## 5. Values

As in the plugin API's "how values cross" (see `plugin-api.md`): JSON-compatible data is copied; functions, promises / coroutines and objects with methods go by reference; nesting at most 64 levels (a `list`, `record`, `map` or `set` is one level); NaN, ±Infinity, symbols and cyclic references cannot cross. Values over a limit are refused by the **sender** before sending: only that call fails and the session continues. Errors cross with their name, message and whole error graph (causes, children of aggregate errors). An error node may also carry `code` (a string) and `data` (a graph value, decoded like any other value; for example what an exported Rust service's `Err(e)` serializes to): decode them onto the native error (JavaScript properties `code` / `data`, Python attributes, Go `RemoteError.Code` / `Data`), encode them from the native error when the plugin's error has them (a string `code`, any `data`; leave out `data` that cannot cross and send the rest of the error), and keep both when the plugin throws a received error again. A far end that does not know them ignores them, so they need no capability.

Ten tagged values cross beside `reference` and `signal`: `instant`, `datetime`, `date`, `time`, `bytes`, `map`, `set`, `bigint`, `decimal`, `uuid` (wire text and rules: protocol design §6.1, https://github.com/arcships/rutis/blob/main/docs/design-protocol-plugins-2026-09-25.en.md; per-language mapping: https://github.com/arcships/rutis/blob/main/docs/design-value-types-2026-10-15.en.md §2).

- Each tag has a capability `value-<tag>`, declared in both handshake formats. Send a tag only to a far end that declared it; otherwise fail only that call, naming the value. A runtime may implement a subset and declare only that.
- Map each value to the language's native type; where the language has none, use the closest type that loses nothing (JS receives `datetime`, `date`, `time`, `decimal`, `uuid` as plain strings). Plugin authors must not need rutis-specific types; SDK constructors are only for creating a value of a type the language lacks.
- An integer beyond ±(2^53−1) is sent as a `bigint` automatically; a received `bigint` decodes into a declared integer type that holds it.
- Map keys and set elements: strings, finite numbers, booleans, `bigint`, `datetime`, `date`, `time`, `decimal`, `uuid` only. Keys that become equal after conversion fail the call.
- Points in time are truncated toward negative infinity to the receiver's precision. `undefined` inside data becomes `null`.
- A malformed tagged value or key fails only that call; keep decoding the rest so every reference in it is imported and released. A frame not in the protocol's JSON shape still ends the session.

## 6. Events (rutis 0.13, #464)

Full specification: https://github.com/arcships/rutis/blob/main/docs/design-events-protocol-2026-10-18.en.md (section numbers below refer to it); design: https://github.com/arcships/rutis/blob/main/docs/design-events-2026-10-18.en.md. All listeners on a host form **one logical bus** whose table the host keeps. Your runtime registers its plugins' listeners automatically and dispatches through the host unless every listener of the name is in your process.

**Messages** (all `invoke` frames with an empty target, family `events`; handle `events.on` / `events.off` / `events.elsewhere` inline, in frame order, and start an `emit`-mode `events.call` in frame order too):

| Direction | Method and arguments | Meaning |
| --- | --- | --- |
| you → host | `events.on [{ id, name, row, plugin, kind, prepend, global }]` | Register a listener when a plugin calls `ctx.on`. `id`: a string unique in the session, never reused. `kind`: `"sync"` (known synchronous), `"async"` (async function / `async def`), `"any"` (known only from what it returns). Never register `internal/*` |
| you → host | `events.off [[id, …]]` | A listener's disposer was called |
| host → you | `events.elsewhere [{ add, remove, addPrefixes, removePrefixes }]` | Names / prefixes that have listeners outside your process: the full set first (before `mount`), then differences. Treat it as empty when the session ends |
| you → host | `events.dispatch [{ row, name, mode, args, filter?, terminal? }]` | An emission that is not on the fast path. The host answers an `emit` with a plain `undefined` once its listeners started: take that reply in place, in frame order (as you read its frame, before the next frame; not on a later task, tick or thread), and from then on count the emit as answered. A 0.13 host may answer with a future: settle it, then count the emit as answered. `mode`: `emit` / `parallel` / `serial` / `waterfall` / `bail` (`bail` is a synchronous call). `filter`: a function reference to a selector taking the list of descriptors and returning a list of booleans, running the plugin's filter on each in your process; the descriptors of your own listeners also carry `listener`, your `events.on` id: find the listener by it (never by the other fields: two copies of a row can have equal descriptors) and remove it before calling the plugin's filter. `terminal`: `waterfall`'s innermost function, called with the event arguments |
| host → you | `events.call [{ listener, dispatch, own, name, mode, args, next?, signal }]` | Run one listener: call it with `args`, plus `next` as the last argument in `waterfall` / `waterfall-sync`; answer its return value as is, or `throw` its error. In `emit` mode start the listener in frame order, before you handle the next frame. A runtime that is not reentrant holds it back while one of its synchronous calls waits, like any call not on the wait's chain, and runs the held calls in frame order afterwards. Answer `undefined` after it finished and report a failure yourself (below). An unknown or withdrawn `listener`: `throw` `BindingError` `listener <id> of <name> is withdrawn` |

**Fast path**: dispatch an emission of `name` in your own process when `name` is not in elsewhere, no prefix in it matches, and (for `emit`) no earlier `emit` of `name` you sent through the host is still unanswered. Otherwise send `events.dispatch`. Frame order makes this safe: apply an `emit` reply in place, as you read its frame and before the next frame (never on a later task, tick or thread), and start `emit`-mode `events.call`s in frame order. Then an emit started by a later frame (a service call, a sync call returning) sees the earlier one answered and may go local, and the host's `events.call` for the earlier one has already started its listener. A runtime that is not reentrant holds those calls back while one of its synchronous calls waits. It must run a fast-path emit of `name` behind any held `emit`-mode `events.call` of `name` that has not run yet, never before it (§5.5).

**Dispatch rules** (the same on both paths, §5): take the listeners of the name in registration order whose scope chain is a prefix of the emitting row's chain; apply the filter (non-`global` listeners only) before any listener runs; check each listener is still live just before starting it.

| Mode | Behavior |
| --- | --- |
| `emit` | Start every listener in order, wait for none; the API returns at once. A failure never reaches the emitter: write `rutis: listener of <name> in row <row> failed: <ErrorName>: <message>` on stderr, never end the process |
| `parallel` | Start all in order, wait for all; failures → `AggregateError` with the failures in **registration order**, even when only one failed |
| `serial` | One at a time; return the first value that is a result; an error stops and is thrown as is |
| `waterfall` | Outermost first, each gets `next` (no arguments; a second call or a call after the end fails with `WaterfallNext`); the outermost return value as is |
| `bail` | Like `serial`, synchronous. A listener of kind `async` is not called and the dispatch fails with `AsyncListener`; one returning a Promise / awaitable fails the same (discard it) |

"No result" is `undefined` / `null` / `None` / `nil` / `false`; anything else is a result. Start `emit` invocations in arrival order (one execution unit per call: queue them per listener and run them one after another). Refuse emitting `rutis/*` synchronously with `ReservedEvent` (`rutis/* events are emitted by rutis only: <name>`); `rutis/*` can be listened to.

**Scope**: each row's chain arrives as the seventh `rows.load` argument `{ scope: [{ id, group }, …], id }` (root `[]`); mount-mode plugins get `mount`'s `scope`. `mount` also carries `runtime`, your runtime's name as the host's descriptors give it: use it in the descriptors of your own listeners on the fast path, so a filter sees the same on both paths. Your emissions name their `row` (the key); the host never takes a scope from you. `id` is the row's id in the configuration: descriptors (`row`) and every reported line call the row by it, never by its key (the key without an `id`).

**Filter API**: Cordis plugins use `thisArg[Context.filter]`, called with the real `Context` for listeners in your process and `{ rutis: descriptor }` for others; give every Context a read-only `ctx.rutis` (its descriptor). Other plugins: `ctx.filter(predicate)` returns an emitter with the five methods. A descriptor is `{ name, prefix, plugin, row, runtime, node, scope, global }`.

**Unload and disconnection**: the host withdraws a row's listeners when it sends `rows.unload` (or gets `rows.ended`); refuse the row's `events.call` from then on, withdraw provides, signal cancellation to its running listeners (Go context, Python task cancellation, JavaScript the row's `ctx.signal`) and wait for them at most `RUTIS_SHUTDOWN_TIMEOUT` (then write `rutis: listeners of row <row> still running after <s>s; unloading anyway`), then run cleanups. A `cancel` for an `events.call` aborts its `signal`. When the session ends, unload every row as before.

**Reporting** (failures nobody waits for, one stderr line each, §5.6 and §11): `rutis: listener of <name> in row <row> failed: <ErrorName>: <message>` (an `emit` listener), `rutis: filter of <name> from row <row> failed: …` (the filter of an `emit`), `rutis: emit of <name> from row <row> failed: …` (an `events.dispatch` in `emit` mode refused or unanswered), `rutis: registration of listener of <name> in row <row> failed: …` (an `events.on` refused outside a row load; during a load, fail the load instead).

**Errors** (§11; names and messages exact): `AggregateError` (`<n> listeners of <name> failed`, children in registration order, encoded as JavaScript encodes an `AggregateError`), `AsyncListener`, `WaterfallNext`, `ReservedEvent`, `SyncWaitCycle`, `BindingError`. Refuse registering `internal/*` (the host answers `BindingError` `event <name> is internal to its runtime`).

**An old host**: when the host's `hello` lacks `events-v2`, end the session right after the handshake, write `rutis: refused: incompatible (this host does not declare events-v2 (the event protocol of rutis 0.13): upgrade rutis-host to 0.13 or later); stopping` and exit with status 1 (a dialing runtime does not retry).

**API per language** (§12.2): `on`, `emit`, `parallel`, `serial`, `waterfall` (terminal last), `bail`, `filter`, in the language's native style; a `waterfall` listener receives `next` as its last argument. Plugin API version 3.

## 7. Extra rules for compiled-program runtimes (the Go model)

- The executable answers `--rutis-manifest` by printing its manifest to stdout and exiting (no channel, no `apply`):

  ```json
  { "manifest": 1, "sdk": "<version>", "pluginApi": 3, "runtime": "rutis-<language>-runtime:1",
    "platform": "<os>/<arch>", "plugins": { "<name>": { "config": {}, "inject": [], "provides": {}, "version": null } } }
  ```

- The file contains the marker string `rutis-<language>-runtime:1`; when scanning a directory rutis executes only files containing it. Plugin entries are generated by the same code as the `rows.schema` reply.
- Two plugins with the same name in one file: exit at startup naming both definitions. Plugins are registered only through the arguments to `serve(...)`, never through static registries.

## 8. Conformance tests (the contract itself)

The Rust tests in the rutis repository start your runtime and check it item by item:

| Test | Checks |
| --- | --- |
| `rutis-bridge/tests/runtime_conformance.rs` | `session::testing::session`: references, callbacks, errors, a malformed frame ends the session |
| `rutis-bridge/tests/session_matrix.rs` | The runtime contract over every channel |
| `rutis-loader/tests/multilang*.rs` | Cross-language cold starts and calls, withdrawing a provider stops only its users, `inject` gating, crossing synchronous calls do not deadlock |
| `rutis-bridge/tests/websocket_conformance.rs` | Remote listening |
| Event expectations `crates/rutis-bridge/src/runtime/testing/events.expected.json` (0.13) | The five modes, scope, filter, unload, cancellation, disconnection and `rutis/*` events, with listeners and emitters in your runtime, another runtime, the host and a linked node; the testing tool runs the in-process cases |
| Test-tool expectations `crates/rutis-bridge/src/runtime/testing/weather.expected.json` | The SDK's host-free test tool and the real runtime give the same results, an undeclared service (`undeclared`: inferred methods, all `async`) included |
