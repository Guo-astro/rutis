# Unified events: protocol specification

[中文](design-events-protocol-2026-10-18.md)

Status: specification (2026-10-18), step 1 of [#464](https://github.com/arcships/rutis/issues/464). It makes the accepted design [Unified events: one logical bus](design-events-2026-10-18.en.md) precise enough for the Rust kernel and bridge, Node / Bun, Python and Go to be implemented in parallel from this document alone. It does not reopen the design's decisions; where the design is silent, the choice made here is listed in §15.

Why a document of its own and not an appendix of [protocol plugin design](design-protocol-plugins-2026-09-25.en.md): that document is a historical proposal whose only current section is §6.1 (value tags); its §11 (events) is superseded in full by this one. Putting a specification of this size inside a historical document would leave readers unsure which half is current. That document now points here from §11, and the [runtime contract](../.agents/skills/rutis-new-runtime/references/runtime-contract.md) has an "Events" section summarizing what a runtime implements.

In this document "must" is a hard requirement of every implementation. Frames, field names, error names and quoted messages are exact; prose around them is not normative.

## 1. Terms

| Term | Meaning |
| --- | --- |
| **host** | The rutis process (rutis-host or an application embedding rutis-bridge). It is the hub of the logical bus. |
| **process** | The host, or one runtime session (a local runtime process, a remote runtime, an attached runtime). Each holds one session with the host. |
| **node** | Another rutis host reached over a node link. |
| **entry** | One registered listener on the logical bus: a listener of the host's Rust plugins, of a runtime's rows, or one imported over a node link. |
| **table** | The host's ordered list of every entry. |
| **descriptor** | What the bus says of an entry (§4.2); the filter sees it. |
| **scope chain** | The list of scopes from the root to a scope (§3). |
| **dispatch** | One emission of an event: its name, arguments, mode, emitter scope and optional filter and terminal. |
| **dispatcher** | Who runs a dispatch: the emitter's own process on the fast path (§5.2), otherwise the host. |
| **invocation** | One call of one entry within a dispatch. |
| **no result** | A return value that is `undefined` / `null` (JavaScript), `None` (Python, Rust), `nil` (Go), or `false` (any language, Rust `Some(false)` included). Everything else is a result. |

## 2. Version, capability and refusals of older peers

### 2.1 The capability `events-v2`

The session protocol versions do not change: the compat format stays protocol 2, the endpoint format protocol 3, the WebSocket subprotocol `rutis.3`. Events ride on existing `invoke` / `call` frames; no frame type is added. What changes is the event protocol, gated by one **mandatory capability**:

- Every 0.13 endpoint (host, runtime, node) declares `events-v2` in its `hello.capabilities`, in both formats. Rust adds it to `COMPAT_CAPABILITIES`, `COMPAT_DECLARED` and `Endpoint::rust`.
- Every 0.13 endpoint ends the session right after the handshake when the far end did not declare `events-v2`. Nothing else is sent: no `mount`, no `link.offers`.
- A 0.12 or older far end does not declare it, so mixing fails before anything is loaded, on every path: local runtimes, remote runtimes, attached runtimes and node links.

### 2.2 Failure messages

| Who refuses | Message (exact) | Then |
| --- | --- | --- |
| Rust (host or node), far end named itself (endpoint format) | `incompatible session: the far end (<name> <version>) does not declare events-v2 (the event protocol of rutis 0.13): upgrade it to rutis 0.13 or later` | `Handshake::Incompatible`; a runtime fails to start with this error; a node link stops (status `link stopped: incompatible (…)`), not retried |
| Rust, compat format (the far end names no implementation) | `incompatible session: the far end (a runtime older than rutis 0.13) does not declare events-v2 (the event protocol of rutis 0.13): upgrade it to rutis 0.13 or later` | as above |
| A 0.13 runtime whose host lacks it | stderr: `rutis: refused: incompatible (this host does not declare events-v2 (the event protocol of rutis 0.13): upgrade rutis-host to 0.13 or later); stopping` | exit status 1; a dialing runtime stops retrying |

A 0.12 runtime meeting a 0.13 host sees its session closed after the hello and exits as it does for any lost session; the host's message above is the one the user reads.

### 2.3 Plugin API 3

The plugin API version becomes **3** (Node `PLUGIN_API`, Python `PLUGIN_API`, Go manifest `pluginApi`): plugins gain `parallel`, `serial`, `waterfall`, `bail` and `filter`. A 0.13 runtime loads plugins declaring API 2 or 3; a plugin declaring 3 is refused by an older runtime with the existing message.

### 2.4 Removed

| Removed | Where | Error when still used (exact) |
| --- | --- | --- |
| `mount` arguments `events`, `emits`; `mount` feature `events`; runtime→host `event`; host→runtime `emit` | protocol | — (unreachable: §2.1 refuses the peer first) |
| `node events.forward`, `NodeEvent`, `EventsPlugin` | rutis-bridge | — (removed from the API) |
| `RuntimePlugin::emits`, `LocalRuntime::emits`, `Mount.emits`, `Mount.events`, `Process::emit`, `runtime::events::{Events, EmitToCordis}` | Rust API | removed; compile error |
| Cordis mount / generator config `events`, `emits` | `rutis-bridge` generator config | `the config field <events\|emits> was removed in rutis 0.13: listeners register themselves; emit with ctx.emit / parallel / serial / waterfall / bail (docs/migration-0.13.md)` |
| Node link config `events: { out, in }` | `rutis-bridge/peer` row config | `the peer config field events was removed in rutis 0.13: list events: { out } in export and events: { in } in import (docs/migration-0.13.md)` |
| `on_waterfall*`, `on_waterfall_sync*`, `*_instance`, `*_keyed`, `EventKey::instance`, `SyncEvent` | rutis core | removed; compile error (§12.1 gives the replacements) |

## 3. Scopes

A **scope** is a loader group instance (a copy of a `group`, such as one tenant or one session) or the root. Every plugin, row and emission is in exactly one scope: the innermost instance that contains it, or the root.

- **Scope chain** on the wire: a JSON array of `{ "id": <string>, "group": <string> }`, outermost first. The root is `[]`. `id` is opaque, unique among the host's live and past instances (Rust: the group fiber's `InstanceId` in decimal); `group` is the loader group's name. Example: `[{"id":"17","group":"tenant"},{"id":"42","group":"session"}]`.
- **Rule**: an entry with chain `L` hears a dispatch emitted at chain `E` when `L` is a prefix of `E` (compared by `id`). Root entries hear everything; an entry in tenant `t1` does not hear `t2`.
- **Where a process learns chains**: the host gives each row its chain. `rows.load` takes a seventh argument `{ "scope": <chain>, "id": <the row's id> }` (0.13 hosts always send seven arguments; `exports` is `{}` when the row exports nothing). `id` is the id the configuration gives the row (the loader's row id; the copies a group's instances make share it), not the row key, which is unique on the session: descriptors (§4.2) and every reported message (§5.6, §11) call the row by it, while protocol messages keep using the key. Without `id`, the key. Plugins loaded by `mount`'s `plugins` list get the mount argument `scope` (the mounting plugin's chain). Rust plugins read `Ctx::scope()` (§12.1). `mount` also carries `runtime`, the runtime's name as descriptors give it (§4.2): a runtime uses it for its own entries' descriptors on the fast path, so they are the same as through the host.
- **The emitter's scope is never self-reported**: a runtime's `events.dispatch` names the emitting `row`; the host uses that row's chain and refuses a row that is not loaded on that session (`BindingError`, `row <key> is not loaded in this runtime`). Mount-mode plugins name `row: null` and get the mount's chain.
- Leaf `isolate` labels (service scopes, U+0001 leases) are unrelated to event scopes.

## 4. Listener registration

### 4.1 Messages

All event messages are `invoke` frames with an empty `target` (control operations), family `events`. The receiving side must handle `events.on`, `events.off` and `events.elsewhere` **inline**, in frame order, before reading the next frame (they only change tables). Every one is answered with `return undefined` or a `throw`.

| Direction | Method | Arguments | Answer |
| --- | --- | --- | --- |
| runtime → host | `events.on` | `[registration]` (§4.2) | once the entry is in the table and the `events.elsewhere` it causes are queued on every other session |
| runtime → host | `events.off` | `[[id, …]]` | once the entries are out of the table; an unknown id is ignored |
| host → runtime | `events.elsewhere` | `[{ "add": [names], "remove": [names], "addPrefixes": [prefixes], "removePrefixes": [prefixes] }]` (absent fields mean `[]`) | after applying it |

### 4.2 Registration and descriptor

Registration (runtime → host):

```json
{ "id": "7", "name": "session/created", "row": "chat", "plugin": "chat-ui", "kind": "any", "prepend": false, "global": false }
```

| Field | Rules |
| --- | --- |
| `id` | String chosen by the runtime, unique within the session for the session's life, never reused |
| `name` | The event name. `internal/*` is refused (`BindingError`, `event <name> is internal to its runtime`). `rutis/*` may be listened to |
| `row` | The registering row's key; `null` for a mount-mode plugin. The host refuses a row not loaded on the session. A descriptor's `row` is not this but the row's id (§3) |
| `plugin` | Name of the registering plugin (the Cordis plugin's name, which may be a child plugin of the row; the leaf plugin's name) |
| `kind` | `"sync"`: known synchronous (every Go listener); `"async"`: known asynchronous (JavaScript `AsyncFunction`, Python `async def`); `"any"`: known only from what it returns (other JavaScript and Python functions) |
| `prepend` | The listener option `prepend` (every language, §12; Cordis's own option on Node / Bun) |
| `global` | The listener option `global` (every language, §12; Cordis's own on Node / Bun): the entry is exempt from custom filters (§6), never from the scope rule |

Runtimes never register prefixes; Rust pattern listeners are prefix entries of the host (`prefix: true`, `name` = the prefix).

The **descriptor** the bus keeps and the filter sees (JSON; Rust `ListenerInfo`, Go `rutis.Listener`):

```json
{ "name": "session/created", "prefix": false, "plugin": "chat-ui", "row": "chat", "runtime": "py", "node": null, "scope": [{"id":"17","group":"tenant"}], "global": false }
```

`row` is the row's id (`rows.load`'s `id`; for a Rust row the row `Ctx::mark_row` marked, the loader's row id), not the row key, so a filter selects by the name in the configuration, the same for a row of any language in any process; `runtime` is the runtime's name (`null` for the host's own Rust plugins); `node` is the peer id of an entry imported over a node link (`null` otherwise); `scope` is the entry's chain, set by the host from the row (for an imported entry: the link's scope, §9).

### 4.3 Order

- Each entry gets a **position**. The host gives appended entries increasing positions in the order it accepts them (`events.on` frames in arrival order, Rust registrations when made, link imports when received); a `prepend` entry takes a position lower than every existing one. A dispatch takes the matching entries (same name, or a prefix entry whose prefix the name starts with) sorted by position: **registration order** in this document.
- A process keeps its own entries in the same relative order as the host (its frames are processed in order; it applies `prepend` among its own entries the same way).
- `rows.update` re-registers the row's listeners: they get new positions.

### 4.4 Guarantees between registration and dispatch

1. **Same process**: a listener registered (in program order) before an emission in the same process is in that dispatch, on the fast path and through the host (its `events.on` precedes the `events.dispatch` on the session, and both are handled in order).
2. **Rows**: a runtime answers `rows.load` only after every `events.on` the row sent during `apply` was answered. A row the host reports Running therefore has its listeners on the bus.
3. **Across processes**: before answering `events.on`, the host queues the resulting `events.elsewhere` on every other session. Any dispatch that causally follows the answer (the registering plugin then calls a service in another process, which emits) sees the entry.
4. **Withdrawal**: once `events.off` is answered (or the host withdrew the row, §7.1), no new invocation of those entries is started. Invocations already sent fail once (§7.1).
5. Rust registrations in the host are in the table when `on` returns, with their `events.elsewhere` queued.

### 4.5 Presence ("elsewhere")

For each runtime session R, the host maintains **elsewhere(R)**: every name and prefix that has at least one entry not in R (host Rust plugins, other runtimes, node imports), regardless of scope. It sends R the changes with `events.elsewhere`: the full current set as `add` right after the handshake and before `mount`, then only differences, in order. R's fast-path check (§5.2) uses it. When R's session ends, R treats elsewhere(R) as empty.

The kernel's process-local diagnostic events (names starting with `rutis::`, such as `rutis::ServiceChanged` and `rutis::FiberStatusChanged`) name Rust types and cannot be decoded elsewhere: their entries never count towards elsewhere and are not announced over node links.

## 5. Dispatch

### 5.1 The algorithm (the same for every dispatcher)

1. **Snapshot**: the entries matching the name, in registration order, whose scope chain is a prefix of the emitter's chain (§3), that are live (not withdrawn).
2. **Filter** (§6), if the dispatch has one: evaluated once for each non-`global` entry of the snapshot, in order, before any invocation starts. Entries it rejects are dropped.
3. **Run** the mode (§5.4). Immediately before starting each invocation the dispatcher checks the entry is still live; a withdrawn entry is skipped as if it had never been in the snapshot.

### 5.2 Who dispatches: the fast path

A runtime R dispatches an emission of name N itself (**fast path**), with no message to the host, when N is not in elsewhere(R) and no prefix in elsewhere(R) is a prefix of N, and, for `emit` only, R has no earlier `emit` of N sent through the host still unanswered (§5.5). Otherwise it sends `events.dispatch` and the host dispatches over the whole table, invoking R's own entries back through `events.call`. The Rust host always dispatches itself over the table. Results are the same either way; only where the code runs differs.

### 5.3 Messages

**`events.dispatch`** (runtime → host), `invoke` with empty target; a `bail` dispatch is a synchronous call, its frame carrying `sync: true` under the existing `sync-wait` rules:

```json
[{ "row": "chat", "name": "session/created", "mode": "serial", "args": [ … ], "filter": <function reference>, "terminal": <function reference> }]
```

`mode` is one of `emit`, `parallel`, `serial`, `waterfall`, `bail`. `args` is a `list` of values (§10). `filter` (§6) and `terminal` (`waterfall` only) are optional function references. `rutis/*` names are refused before sending (§11). The answer is per mode (§5.4).

**`events.call`** (host → runtime, or node → node over a link), `invoke` with empty target; synchronous (`sync: true` under the `sync-wait` rules) for `bail` and `waterfall-sync`:

```json
[{ "listener": "7", "dispatch": "d1042", "own": false, "name": "session/created", "mode": "waterfall", "args": [ … ], "next": <function reference>, "signal": {"type":"signal"} }]
```

| Field | Rules |
| --- | --- |
| `listener` | The receiving side's entry id (its own `events.on` id, or for a link the id it announced) |
| `dispatch` | Host-chosen id, unique per host; the same for every invocation of one dispatch |
| `own` | `true` when the dispatch came from this same session's `events.dispatch` |
| `mode` | `emit`, `parallel`, `serial`, `waterfall`, `bail`, `waterfall-sync` (only Rust emits `waterfall-sync`) |
| `next` | `waterfall` / `waterfall-sync` only: a function reference taking no arguments, returning the rest of the chain |
| `signal` | The existing `signal` value: it aborts when the host cancels this invocation (§5.7) |

The receiving runtime calls the listener with the event arguments, and in `waterfall` / `waterfall-sync` with `next` appended as the last argument. It answers with the listener's return value as is (the dispatcher applies the result rule), or a `throw` with the listener's error. In `emit` mode it answers `undefined` once the listener finished, whatever happened; a failure is reported on its side (§5.6).

An `events.call` for an id that is unknown or withdrawn is answered with `throw` `BindingError`, message `listener <id> of <name> is withdrawn`.

`internal/dispatch` (Node / Bun): when a dispatch with `own: false` first invokes a listener in the process, the runtime emits Cordis `internal/dispatch` (mode, name, args, `null`) once for that `dispatch` id before calling it; emissions of the process itself go through Cordis's own `dispatch()` and fire it there. A throw from it fails that invocation.

Cordis's `events.dispatch(type, args)` (the primitive under the five methods, which returns the callbacks to call; some published plugins call it and then the callbacks themselves): for a name that is not `internal/*` and a type `emit`, `serial` or `bail`, it returns one callback that dispatches on the bus in `parallel`, `serial` or `bail` with the arguments it is called with (here, or through the host when the name is elsewhere). The listeners are the bus's, not Cordis's, so this is how such a plugin reaches them.

### 5.4 Modes

Semantics follow Cordis 4.0.4 `events.ts` (the design's table). "Start" means calling a local listener, or sending its `events.call` (or calling a Rust listener in the host).

| Mode | Dispatcher does | The emitter gets (`events.dispatch` answer) |
| --- | --- | --- |
| `emit` | Starts every invocation in registration order without waiting for any | Answered once every invocation has been started; the emitter's API had already returned (`void` / `None` / `nil` / `Ok(())`) |
| `parallel` | Starts every invocation in registration order, then waits for all | `undefined` when all succeeded; otherwise `throw` `AggregateError` whose children are the failures **in registration order** (not completion order), also when only one failed |
| `serial` | Starts one at a time, waits for each | The first return value that is a result; a listener's error is thrown as is and stops the rest; `undefined` when none had a result |
| `waterfall` | Invokes the first entry with `next`; `next` of entry *i* invokes entry *i+1* (skipping withdrawn entries); `next` of the last calls `terminal` with the event arguments, or yields `undefined` without one | The outermost listener's return value, **as is** (not normalized; `false` stays `false`). No entries: the terminal's value, or `undefined`. A listener that does not call `next` vetoes the rest |
| `bail` | Like `serial`, each invocation synchronous (`sync: true`) | Like `serial`, synchronously |
| `waterfall-sync` | Like `waterfall`, synchronous, `next` called synchronously | Like `waterfall`, synchronously (Rust emitters only) |

`next` rules: it takes no arguments; calling it a second time, or after its dispatch ended or was cancelled, fails that call with `WaterfallNext` (§11). In `waterfall` the receiving runtime gives the listener a `next` returning a Promise / awaitable / Go blocking call; in `waterfall-sync` it calls the reference synchronously.

Synchronous modes and listener kinds: in `bail` / `waterfall-sync` an entry of kind `async` is not invoked; the dispatch fails with `AsyncListener`. An entry of kind `any` is invoked; if it returns a Promise / thenable / awaitable, the listener's runtime discards it (a Python coroutine is closed; a Promise's rejection is caught and ignored) and fails the invocation with `AsyncListener`. Go listeners are `sync`. Rust: `on` entries are `async`, `on_sync` entries `sync`. Every synchronous invocation takes part in wait-cycle detection like a synchronous service call (`SyncWaitCycle`).

### 5.5 Emit order

For each (emitting process, name), every listener's invocations of `emit` dispatches **start** in emission order:

- The host handles one session's `events.dispatch` frames in order and starts each emit's invocations before those of the next emit of the same name from the same process; its own Rust listeners keep the kernel's per-key chain.
- Frames to one session arrive in order, and a runtime **must start** `emit`-mode invocations in arrival order: Node / Bun / Python call the listener before handling the next frame (it runs synchronously up to its first `await`); Go queues them per listener and runs one after another (different listeners run concurrently).
- A runtime that sent an `emit` of N through the host keeps sending emits of N through the host until that one is answered (§5.2). The host answers after starting the invocations, so its `events.call` for that emit reaches the runtime before any later fast-path emit of N.

### 5.6 Errors in `emit`

An `emit` listener's failure affects neither the emitter nor other listeners, and never ends a process. It is reported where the listener runs, once:

- Runtime: stderr line `rutis: listener of <name> in row <row> failed: <ErrorName>: <message>`.
- Host: its error sink (the line printed by rutis-host is the same text).
- An invocation that could not be delivered or answered (transport, value capability, withdrawn): the dispatcher reports it: `rutis: listener of <name> in <runtime <runtime>|node <peer>> failed: <ErrorName>: <message>`.

A failing filter in an `emit` dispatch delivers to no listener and is reported by the dispatcher the same way (`listener` replaced by `filter`: `rutis: filter of <name> from row <row> failed: …`).

### 5.7 Cancellation

- **Emitter gives up** (a Rust future dropped, a Python task cancelled, a Go context cancelled; JavaScript has no way to cancel a dispatch): on the host path the runtime sends `cancel` for its `events.dispatch` call. The dispatcher starts no further invocation, sends `cancel` for every invocation in flight (its `signal` aborts), and fails any later `next` call with `WaterfallNext`. The emitter's call ends with its own language's cancellation. `emit` dispatches are not cancelled.
- **The receiving side** maps an aborted `signal` to: Go, the listener's `context.Context` is cancelled; Python, the listener's task is cancelled; Rust, the listener future is dropped. JavaScript listeners have no per-invocation signal: the invocation runs on and its answer is discarded (only unload, §7, aborts `ctx.signal`).
- No call timeouts are added.

## 6. Filter

- **Where it runs**: always in the emitter's process. On the fast path the runtime calls the plugin's filter function directly for each candidate. Through the host, `events.dispatch` carries `filter`: a function reference to a **selector** the runtime makes around the plugin's filter. The host calls it once per dispatch, after the snapshot (§5.1), with one argument: the list of descriptors of the non-`global` candidates in registration order. The selector calls the plugin's filter on each, in order, in the emitter's process, and returns a list of booleans of the same length (`true` = deliver), using the language's own truthiness / `bool`. The host calls it synchronously in `bail` dispatches, asynchronously otherwise. A wrong length or non-booleans fail the dispatch (`BindingError`, `filter of <name> returned <n> answers for <m> listeners`).
- **Errors**: a throwing filter fails the whole dispatch with that error before any invocation (in `emit`: §5.6). In Go the filter returns `(bool, error)`: the error it returns, with its name, is the failure, and so is a panic. In Rust, `try_filter` takes a predicate returning `Result<bool, CordisError>`, whose error is the failure as it is; `filter` takes a plain `bool` predicate, which fails only by panicking.
- **Default**: the scope rule is applied first and always; the filter only narrows it. `global` entries skip the filter.
- **Cordis** (`Context.filter`): `ctx.emit(thisArg, name, …)` and the other four with a `thisArg` whose `[Context.filter]` is a function use it as the filter. It is called with the entry's own Cordis `Context` for listeners in the process, and with `{ rutis: <descriptor> }` for every other entry. Every Context in a rutis Node / Bun runtime has a read-only `ctx.rutis` with its descriptor, so `c => c.rutis.row === 'chat'` works for both.
- **Other languages**: `filter(predicate)` returns an emitter with the same five methods (§12); the predicate receives the descriptor (Node / Bun leaf and Python: a plain object / `dict` with the JSON fields; Go: `rutis.Listener`; Rust: `&ListenerInfo`).

## 7. Lifecycle and disconnection

### 7.1 Unload

When a row unloads (the host sends `rows.unload`, or the runtime reports `rows.ended`), in this order:

1. The host withdraws the row's entries from the table (and sends `events.elsewhere`) when it sends `rows.unload` or receives `rows.ended`; a runtime that sends `events.off` for them as well is harmless.
2. The runtime stops admitting invocations for the row at once: an `events.call` arriving afterwards fails with the withdrawn error (§5.3).
3. Provides are withdrawn (as in 0.12).
4. Running listener invocations of the row get the cancellation signal (Go: the context; Python: task cancellation; JavaScript: the row's `ctx.signal` aborts; Rust: the fiber's stop, as for its other tasks) and are waited for, at most `RUTIS_SHUTDOWN_TIMEOUT` (default 10 s; the host's own shutdown deadline for Rust plugins). Past it the runtime writes `rutis: listeners of row <row> still running after <s>s; unloading anyway`, abandons them, and continues.
5. Cleanups run in reverse order, then `rows.unload` is answered.

An invocation that was already sent when its entry was withdrawn fails once, with the withdrawn error or the transport error; it is never retried.

### 7.2 Disconnection

When a runtime session or a node link ends, the host withdraws every entry that came over it (as if their rows unloaded) and sends `events.elsewhere`. Invocations in flight to it fail once with the transport error service calls get (`BindingError`, the session's end as message): `parallel`, `serial`, `waterfall`, `bail` hand it to the emitter (`parallel` inside the `AggregateError`); `emit` reports it (§5.6). A synchronous wait on it fails the same way. The runtime side unloads every row (as in 0.12) and treats elsewhere as empty.

## 8. Framework events `rutis/*`

Emitted by the host only, in `emit` mode, each with one argument (a record). Plugins may listen; emitting `rutis/*` from any plugin fails with `ReservedEvent`.

| Name | Argument | Fired | Emitter scope |
| --- | --- | --- | --- |
| `rutis/service` | `{ "name": <service name>, "scope": <chain>, "available": <bool>, "row": <providing row or null> }` | When a service name gets a provider in a scope (`available: true`), when the provider is replaced (`true` again, new `row`), when it is withdrawn with none left (`false`, `row` of the last provider) | The service's scope |
| `rutis/row` | `{ "row": <key>, "plugin": <name>, "scope": <chain>, "state": "pending" \| "running" \| "failed" \| "stopped", "error": <message or null> }` | On every state change of a configured row (any language, Rust rows included); `error` set with `failed` | The row's scope |
| `rutis/runtime` | `{ "runtime": <name>, "state": "running" \| "down", "reason": <text or null> }` | `running` when a runtime's mount succeeded (also after each reconnect); `down` when its session ended or its mount failed, `reason` saying why (exit status, transport error, refusal message) | Root |

`internal/*` stays inside each runtime and is never registered with or sent to the host.

## 9. Node links

- **Which events are shared**: the link's existing `export` / `import` lists, shared with services. An item is a name, or a prefix written with a trailing `*` (`session/*`; `*` alone is every name). Prefix items apply to events only; services keep exact names.
- **Meaning**: `export: [x]` on node A means A's emissions of `x` reach B's listeners; `import: [x]` on B means B's listeners hear A's `x`. The listener entries therefore travel from importer to exporter: B announces its entries of names matching its `import`, and A accepts those matching its `export`. `events: { out }` becomes `export`, `events: { in }` becomes `import`. A name may be shared both ways.
- **Link messages** (family `events`, on the link's session; each node registers the family when the link starts):

  | Method | Arguments | Meaning |
  | --- | --- | --- |
  | `events.on` | `[{ "id", "name", "prefix", "plugin", "row", "runtime", "kind", "prepend", "global" }]` | An entry of the sender, offered to the receiver. When the receiver offers `events`, the sender announces its whole table in table order with `prepend: false`; after that, each entry as it registers, `prepend` as registered: the receiver's order is the sender's |
  | `events.off` | `[[id, …]]` | Withdrawn |
  | `events.call` | as §5.3 | Invoke an entry the sender accepted |

- **What a node announces**: only its own entries (host Rust plugins and its runtimes), never entries it imported over any link (no relaying, so no loops), and only entries whose scope chain is a prefix of the link's scope (the scope of the `rutis-bridge/peer` row; the root for a link outside instances), since only those could hear an event arriving there. A prefix entry is announced narrowed to what both it and the `import` item match (two prefixes: the longer when one extends the other; a prefix and a name: the name).
- **Where an accepted entry sits**: at the link's scope on the accepting node, appended in arrival order, or in front of every existing entry when announced with `prepend: true`; descriptor `node` = the peer id, `plugin` / `row` / `runtime` as announced, `scope` = the link's scope. An entry announced that matches nothing in `export` is ignored (answered `undefined`).
- **Rules**: identical to local entries: five modes, scope, filter, value types, withdrawal and disconnection (§7.2). The invoked node runs the entry (its Rust listener, or relays `events.call` to its runtime). A disconnected link's entries are withdrawn.

## 10. Values

Event arguments, results, `next` / terminal results and filter descriptors are session values on every path: the ten tagged values, references to functions and objects, signals, the 64-level depth limit, the same encoding and capability rules as calls ([protocol plugin design §6.1](design-protocol-plugins-2026-09-25.en.md#61-tagged-values-in-the-session-protocol-current-normative)). The sender checks against the receiving far end's capabilities; a value it cannot send fails **only that invocation** (one listener: in `parallel` one child of the `AggregateError`, in `emit` a report), or for `events.dispatch` the whole dispatch. Node links carry them the same way (the 0.12 JSON-only rule is gone). References passed as event arguments live as long as the invocation, as for call arguments.

## 11. Errors

| `name` | When | Message (exact; `<…>` filled in) |
| --- | --- | --- |
| `AggregateError` | `parallel` with failures | `<n> listeners of <name> failed`; children in registration order: on the wire the error graph's root is an `error` node named `AggregateError` whose `errors` refer to the children's nodes (each child's own graph grafted in), as JavaScript encodes an `AggregateError` |
| `AsyncListener` | async listener in `bail` / `waterfall-sync` | `listener of <name> in row <row> is asynchronous and cannot run in a synchronous <bail\|waterfall-sync>` |
| `WaterfallNext` | `next` twice / after the end / after cancellation / outside a waterfall (Rust) / Rust sync listener in an async waterfall | `next of <name> was already called` · `next of <name> was called after its waterfall ended` · `next is available only in a waterfall (this is <mode>)` · `a synchronous listener cannot call next in an asynchronous waterfall; register it with on` |
| `ReservedEvent` | emitting `rutis/*` (any mode, any language, refused synchronously by the emit API) | `rutis/* events are emitted by rutis only: <name>` |
| `SyncWaitCycle` | a synchronous invocation would close a wait cycle | as for service calls |
| `BindingError` | transport errors, withdrawn listeners, refused frames | the transport's message, or as given in §3, §4.2, §5.3, §6 |

A listener's own error crosses with its name, message and graph, as any call's error.

Failures nobody waits for are reported on the side they happen, one stderr line each, in every runtime (besides §5.6's listener and filter lines):

| When | Line (exact; `<…>` filled in) |
| --- | --- |
| an `emit` sent through the host is refused or not answered | `rutis: emit of <name> from row <row> failed: <ErrorName>: <message>` |
| the host refuses a listener's `events.on` (outside a row load, which fails instead) | `rutis: registration of listener of <name> in row <row> failed: <ErrorName>: <message>` |

## 12. APIs

### 12.1 Rust

```rust
// rutis (kernel); everything below is in `rutis::events`, the common types also at the crate root
pub trait Event: Serialize + DeserializeOwned + Send + Sync + 'static {
    const NAME: &'static str;
    type Value: Serialize + DeserializeOwned + Send + 'static;
}   // every event crosses processes: derive Serialize and Deserialize

pub enum Mode { Emit, Parallel, Serial, Waterfall, Bail, WaterfallSync }

pub struct Call<'a, V> { /* .. */ }
impl<'a, V> Call<'a, V> {
    pub fn mode(&self) -> Mode;
    pub fn name(&self) -> &str;
    /// The rest of the chain (`waterfall` only; otherwise `Err(WaterfallNext)`).
    pub fn next(self) -> BoxFuture<'a, Result<Option<V>, CordisError>>;
}
pub struct SyncCall<'a, V> { /* .. */ }
impl<'a, V> SyncCall<'a, V> {
    pub fn mode(&self) -> Mode;
    pub fn name(&self) -> &str;
    /// `waterfall_sync` only; in an async `waterfall`: `Err(WaterfallNext)`.
    pub fn next(self) -> Result<Option<V>, CordisError>;
}

pub trait Listener<E: Event>: Send + Sync + 'static {
    fn call<'a>(&'a self, ctx: &'a Ctx, event: &'a E, call: Call<'a, E::Value>)
        -> BoxFuture<'a, Result<Option<E::Value>, CordisError>>;
}   // implemented for `for<'a> Fn(&'a Ctx, &'a E, Call<'a, E::Value>) -> BoxFuture<'a, …>`
pub trait SyncListener<E: Event>: Send + Sync + 'static {
    fn call(&self, ctx: &Ctx, event: &E, call: SyncCall<'_, E::Value>)
        -> Result<Option<E::Value>, CordisError>;
}   // implemented for `Fn(&Ctx, &E, SyncCall<'_, E::Value>) -> Result<…>`
pub trait PatternListener<E: Event> { /* as Listener, plus `key: EventKey<E>` before `event` */ }
pub trait SyncPatternListener<E: Event> { /* as SyncListener, plus `key` */ }
pub trait Terminal<E: Event>: Send + 'static {
    fn call<'a>(&'a self, ctx: &'a Ctx, event: &'a E) -> BoxFuture<'a, Result<Option<E::Value>, CordisError>>;
}   // implemented for such closures, and for `()`: no terminal (the last `next` gives `None`)

pub struct EventOptions { pub prepend: bool, pub once: bool, pub global: bool }

impl EventBus {   // ctx.events()
    pub fn on<E: Event>(&self, ctx: &Ctx, key: &EventKey<E>, l: impl Listener<E>) -> Result<Disposer, CordisError>;
    pub fn on_opt<E: Event>(&self, ctx: &Ctx, key: &EventKey<E>, l: impl Listener<E>, o: EventOptions) -> Result<Disposer, CordisError>;
    pub fn on_sync<E: Event>(&self, ctx: &Ctx, key: &EventKey<E>, l: impl SyncListener<E>) -> Result<Disposer, CordisError>;
    pub fn on_sync_opt<E: Event>(&self, ctx: &Ctx, key: &EventKey<E>, l: impl SyncListener<E>, o: EventOptions) -> Result<Disposer, CordisError>;
    pub fn once<E: Event>(&self, ctx: &Ctx, key: &EventKey<E>, l: impl Listener<E>) -> Result<Disposer, CordisError>;
    pub fn on_pattern<E: Event>(&self, ctx: &Ctx, p: &EventPattern<E>, l: impl PatternListener<E>) -> Result<Disposer, CordisError>;
    pub fn on_sync_pattern<E: Event>(&self, ctx: &Ctx, p: &EventPattern<E>, l: impl SyncPatternListener<E>) -> Result<Disposer, CordisError>;
    // and on_pattern_opt / on_sync_pattern_opt with EventOptions

    pub fn emit<E: Event>(&self, ctx: &Ctx, key: &EventKey<E>, e: Arc<E>) -> Result<(), CordisError>;
    pub fn parallel<'a, E: Event>(&self, ctx: &'a Ctx, key: &EventKey<E>, e: Arc<E>) -> BoxFuture<'a, Result<(), CordisError>>;
    pub fn serial<'a, E: Event>(&self, ctx: &'a Ctx, key: &EventKey<E>, e: Arc<E>) -> BoxFuture<'a, Result<Option<E::Value>, CordisError>>;
    pub fn waterfall<'a, E: Event, T: Terminal<E> + 'a>(&self, ctx: &'a Ctx, key: &EventKey<E>, e: &'a E, terminal: T) -> BoxFuture<'a, Result<Option<E::Value>, CordisError>>;
    pub fn bail_sync<E: Event>(&self, ctx: &Ctx, key: &EventKey<E>, e: &E) -> Result<Option<E::Value>, CordisError>;
    pub fn waterfall_sync<E: Event, F>(&self, ctx: &Ctx, key: &EventKey<E>, e: &E, terminal: F) -> Result<Option<E::Value>, CordisError>
        where F: FnOnce(&Ctx, &E) -> Result<Option<E::Value>, CordisError>;

    /// The same six emitting methods, filtered (§6).
    pub fn filter(&self, keep: impl Fn(&ListenerInfo) -> bool + Send + Sync + 'static) -> Filtered<'_>;
    /// `filter` with a predicate that can fail: its error fails the dispatch (§6).
    pub fn try_filter(&self, keep: impl Fn(&ListenerInfo) -> Result<bool, CordisError> + Send + Sync + 'static) -> Filtered<'_>;
}

pub struct ListenerInfo {
    pub name: String, pub prefix: bool, pub plugin: String, pub row: Option<String>,
    pub runtime: Option<String>, pub node: Option<String>, pub scope: Vec<Scope>, pub global: bool,
}
pub struct Scope { pub id: String, pub group: String }

impl Ctx {
    /// The chain of loader instances around this context; `[]` at the root.
    pub fn scope(&self) -> Vec<Scope>;
    /// Make this context's fiber a scope of `group` (the loader calls it for
    /// each group instance before its rows start).
    pub fn mark_scope(&self, group: &str) -> Result<(), CordisError>;
    /// Make this context's fiber the row `key`: `ListenerInfo::row` of the
    /// listeners registered in it and below it (the loader calls it for each row).
    pub fn mark_row(&self, key: &str) -> Result<(), CordisError>;
    /// The nearest marked row.
    pub fn row(&self) -> Option<String>;
}
```

What rutis-bridge builds the logical bus on (the host side; plugins do not use it):

```rust
/// How one Rust type crosses as JSON; carried by every payload and value.
pub struct Codec { /* .. */ }   // Codec::of::<T>(), to_json / from_json, to_args / from_args (the argument mapping below)
pub struct Payload { /* .. */ } // an event: Payload::new(Arc<E>), Payload::from_boxed(box, codec)
pub struct PayloadRef<'a> { /* .. */ }
pub struct Value { /* .. */ }   // a result: Value::new(v), Value::from_boxed(box, codec), into_typed::<T>(transcoder)
/// Converts an event or a value to another type: a listener whose type differs
/// from the dispatch's, a result of another type. Default: JSON. The bridge
/// installs its own (session values, `Args`) with `EventBus::set_transcoder`.
pub trait Transcoder: Send + Sync + 'static {
    fn event(&self, from: PayloadRef<'_>, into: &Codec) -> Result<Box<dyn Any + Send>, CordisError>;
    fn value(&self, from: &Value, into: &Codec) -> Result<Box<dyn Any + Send>, CordisError>;
}

pub enum ListenerKind { Sync, Async, Any }
/// A remote entry: a listener whose calls go to an external dispatcher
/// (a runtime session, a node link). Same table, positions, scope rule,
/// filter (by its `ListenerInfo`) and `global` as any entry.
pub trait RemoteListener: Send + Sync + 'static {
    fn kind(&self) -> ListenerKind;   // `Async`: never called by bail / waterfall-sync (AsyncListener)
    /// Called in registration order when the call starts (for `emit`, before
    /// the dispatch resolves); dropping the future cancels the call (§5.7).
    fn call<'a>(&'a self, call: RemoteCall<'a>) -> BoxFuture<'a, Result<Option<Value>, CordisError>>;
    /// Inline on the dispatching thread, no kernel lock held: a bridge's
    /// sync-wait path (SyncWaitCycle) applies as to any synchronous call.
    fn call_sync(&self, call: RemoteSyncCall<'_>) -> Result<Option<Value>, CordisError>;
}
// RemoteCall / RemoteSyncCall: name(), mode(), dispatch() (one id per dispatch),
// origin() (the raw dispatch's tag), emitter() (&Ctx), event() (PayloadRef),
// next(): Option<RemoteNext> / Option<RemoteSyncNext> — 'static, Clone; the first
// call runs the rest of the chain, a second fails "already called", a call after
// the entry's call ended fails "called after its waterfall ended".
pub struct RemoteEntry { pub name: String, pub prefix: bool, pub info: ListenerInfo, pub options: EventOptions, pub origin: u64 }

impl EventBus {
    pub fn on_remote(&self, ctx: &Ctx, entry: RemoteEntry, l: Arc<dyn RemoteListener>) -> Result<Disposer, CordisError>;
    /// The current entries, then every change; a Rust `on` is reported before it returns (§4.4).
    pub fn watch_table(&self, owner: &Ctx, f: impl Fn(&TableChange) + Send + Sync + 'static) -> Result<(Vec<TableEntry>, Disposer), CordisError>;
    /// A dispatch whose event is erased (a runtime's `events.dispatch`): name, mode, payload,
    /// optional emitter scope, batch filter (§6 selector), terminal, origin, `framework` (`rutis/*`).
    /// For `emit` it resolves once every call started.
    pub fn dispatch<'a>(&self, ctx: &'a Ctx, d: Dispatch) -> BoxFuture<'a, Result<Option<Value>, CordisError>>;
    pub fn dispatch_sync(&self, ctx: &Ctx, d: Dispatch) -> Result<Option<Value>, CordisError>;   // bail, waterfall-sync
    /// Call one entry by id outside a dispatch (a node running the other node's `events.call`, §9).
    pub fn invoke_entry<'a>(&self, ctx: &'a Ctx, id: u64, call: EntryCall) -> BoxFuture<'a, Result<Option<Value>, CordisError>>;
    pub fn invoke_entry_sync(&self, ctx: &Ctx, id: u64, call: EntryCall) -> Result<Option<Value>, CordisError>;
    /// `emit` of a framework event: only rutis emits `rutis/*`.
    pub fn emit_framework<E: Event>(&self, ctx: &Ctx, key: &EventKey<E>, e: Arc<E>) -> Result<(), CordisError>;
}
```

- **Names**: `EventKey::<E>::of()` is named `E::NAME`; `EventKey::named(n)` / `EventKey::dynamic(n)` are named `n`. `EventPattern::prefix(p)` registers a prefix entry. Listeners are found by name only: a listener of another Rust type with the same name hears the event, converted by the transcoder (serde).
- **Arguments ↔ `E`** (every event type): `E` serializing as a tuple or tuple struct of two or more fields → one argument per field; unit or unit struct → no arguments; anything else (struct, newtype, map, sequence) → one argument. Decoding: 0 arguments → from unit; 1 → from that argument; 2 or more → from the list as a tuple (`Codec::to_args` / `from_args`). The bridge's transcoder encodes by the service-argument rules (`session::arg` / `decode_value`), so `Bytes`, `Instant`, `ValueMap`, `ObjectRef`, `RemoteFunction` cross as tagged values and references.
- **Dynamic events**: `rutis_bridge::events::Args(pub Vec<rutis_bridge::session::Value>)` with `Value = rutis_bridge::session::Value`, used only as `EventKey::<Args>::dynamic(name)` (`of()` fails with `BindingError`, `Args events need a name: EventKey::dynamic(name)`); the bridge's transcoder takes its values as they are.
- A Rust listener is called with the context that registered it (its `ctx.cancelled()` is its unload signal, §7.1).
- `Ok(Some(false))` is no result in `serial` / `bail_sync`; `waterfall` returns the value as is.
- Scope: the emitter's scope is `ctx.scope()`; there is no API for plugins to emit into another scope (filter by `ListenerInfo::scope` instead).
- Removed: `on_waterfall*`, `on_waterfall_sync*`, `WaterfallListener`, `SyncWaterfallListener`, `Next`, `SyncNext`, `SyncEvent`, `EventKey::instance`, every `*_instance` and `*_keyed` method, `EventPattern::any_prefix`.

### 12.2 Other languages

`[thisArg,]` is Cordis's optional first argument. In every language a `waterfall` listener receives `next` as its **last** argument.

| | Node / Bun (Cordis plugin) | Node / Bun (leaf) | Python | Go |
| --- | --- | --- | --- | --- |
| listen | `ctx.on(name, fn, options?) → () => boolean` (`options`: `prepend`, `global`; `ctx.once` too) | `ctx.on(name, fn, { prepend?, global }?) → () => void` | `ctx.on(name, fn, *, prepend=False, global_=False) → Callable[[], None]` | `ctx.On(name string, fn func(ctx context.Context, args ...any) (any, error), options ...rutis.ListenOption) func()` (`rutis.Prepend()`, `rutis.Global()`) |
| listener | `(...args[, next]) => value \| Promise` | same | `fn(*args[, next]) -> value \| awaitable` | `fn(ctx, args...)`; in a waterfall the last arg is a `rutis.Next` (`func(context.Context) (any, error)`) |
| `emit` | `ctx.emit([thisArg,] name, ...args): void` | `ctx.emit(name, ...args): void` | `ctx.emit(name, *args) -> None` | `ctx.Emit(ctx context.Context, name string, args ...any) error` (refusals only) |
| `parallel` | `await ctx.parallel([thisArg,] name, ...args)` | `await ctx.parallel(name, ...args)` | `await ctx.parallel(name, *args)` | `ctx.Parallel(ctx, name, args...) error` |
| `serial` | `await ctx.serial([thisArg,] name, ...args)` | `await ctx.serial(name, ...args)` | `await ctx.serial(name, *args)` | `ctx.Serial(ctx, name, args...) (any, error)` |
| `waterfall` | `ctx.waterfall([thisArg,] name, ...args, terminal)` (Cordis: the last argument is the terminal) | `ctx.waterfall(name, ...args, terminal)` | `await ctx.waterfall(name, *args, terminal=None)` | `ctx.Waterfall(ctx, name string, terminal rutis.Next, args ...any) (any, error)` (`terminal` may be nil) |
| `bail` | `ctx.bail([thisArg,] name, ...args)` (synchronous) | `ctx.bail(name, ...args)` | `ctx.bail(name, *args)` (synchronous) | `ctx.Bail(ctx, name, args...) (any, error)` |
| filter | `thisArg[Context.filter] = c => boolean`; `c.rutis` is the descriptor | `ctx.filter(d => boolean)` → `{ emit, parallel, serial, waterfall, bail }` | `ctx.filter(lambda d: bool)` → object with the five | `ctx.Filter(func(rutis.Listener) (bool, error)) *rutis.Emitter` (the five methods; a returned error, or a panic, is the filter's failure) |
| unload signal | `ctx.signal` (the row's; per Cordis Context too) | `ctx.signal` | task cancellation | the listener's `ctx` |
| parallel failure | `AggregateError` | `AggregateError` | `rutis.AggregateError` | `*rutis.AggregateError` |

- No result arrives as `undefined` (JS), `None`, `nil`. A Node `waterfall` returns a Promise whenever any listener is outside the process; plugins `await` it.
- Go: `rutis.Listener` = `struct { Name string; Prefix bool; Plugin, Row, Runtime, Node string; Scope []rutis.Scope; Global bool }` (`""` for null); `rutis.Scope` = `struct { ID, Group string }`; `rutis.Next` = `func(context.Context) (any, error)`. Python and leaf descriptors are plain `dict` / objects with the JSON field names.
- Reserved names: `internal/*` cannot be listened to or emitted by leaf plugins (Python, Go, Node leaf; as in 0.12); `rutis/*` can be listened to, never emitted.
- **Testing tools** (`@arcships/rutis/testing`, `rutis.testing`, `rutistest`): the same five methods, `filter`, scopes (a row can be loaded into a named scope path, such as `t1/s1`), unload waiting and cancellation, in one process (always the fast path). They do not emit `rutis/*`.

## 13. Event expectation file

The cross-language conformance list, like `weather.expected.json`: `crates/rutis-bridge/src/runtime/testing/events.expected.json`, read by the Rust runner for real runtimes (`session_matrix.rs`, with the two-node and netem scenarios adding the `node` location) and by each testing tool. The starter set below is the initial content of that file (step 7 of #464); new rules add cases, never change the format.

### 13.1 The conformance plugin

Each language provides plugin `conformance-events` (and the Rust host a Rust plugin of the same name). Every listener and every emitter is a row of it; the row key is the participant's `label`. Its config is one participant (a listener, or `{ "emitter": true }`). A listener row registers one listener as configured; an emitter row provides service `emitter-<label>` with method `dispatch(spec)` (async; `bail` specs use the sync method `dispatch_sync(spec)`) that emits with the language's native API and returns the result or throws. Each invocation start, filter call and cancellation is recorded by calling the host's sync service `events-log.add(entry)`.

### 13.2 Format

Top level: `about` (how to read), `plugin`, `scopes` (scope path → group: the runner creates these instances), `locations`, `cases`.

| Location | Meaning |
| --- | --- |
| `X` | The runtime under test |
| `Y` | Another runtime process: the runner runs each case once with Y a second process of X's language and once with Y each other language |
| `host` | A Rust plugin in the host |
| `node` | A Rust plugin on a second node linked to the host; both sides `export` and `import` `t/*` and `rutis/*`, not `n/*`; node listeners are at the node's root |

A **case**: `case` (id), `rule` (the section of this document), `name` (default event name), `listeners`, `steps`, optional `vary`, `only`, `except`, `tool`.

- **Listener**: `label`, `at` (location), `scope` (scope path, default `root`), `name` (default the case's), `kind` (`sync` / `async` / `any`; default `sync` when every step of the case uses `bail` or `waterfall-sync`, else `async`; Go is always `sync`; JavaScript / Python `async` = async function, `any` = plain function returning what `does` returns), `options` (`{ "prepend": true }`, `{ "global": true }`; every language), `log` (`"label"` default; `"arg"`: logs `label:<args[0]>`; a list of field names: logs `label:<f1>,<f2>` of `args[0]`; `false`: nothing), `does`:

  | `does` | The listener |
  | --- | --- |
  | `{ "returns": v }` | returns `v` (`null`: JS `null`, Python `None`, …) |
  | `{ "returnsNothing": true }` | returns `undefined` / `None` / `nil` / `None` |
  | `{ "echo": true }` | returns `args[0]` |
  | `{ "throws": { "name", "message" } }` | throws an error of that name and message |
  | `{ "after": ms, "then": does }` | waits `ms`, then does `then` |
  | `{ "next": "<text>" }` | returns `"<text>" + r`, `r` = `next()`'s result (`""` when no result) |
  | `{ "veto": v }` | returns `v` without calling `next` |
  | `{ "nextTwice": true }` | calls `next()` twice, returns the second's result |
  | `{ "waits": true }` | waits for cancellation, then logs `label:cancelled` and returns `null` |
  | `{ "calls": 0 }` | calls `args[0]` (a function) with `"x"`, returns its result |
  | `{ "emits": spec }` | emits `spec` (§ below) from its own row and returns the result |

- **Emit spec**: `from` (location), `row` (emitter label, default `e`), `scope` (default `root`), `mode`, `name` (default the case's), `args` (values; tagged values written `{"$<tag>": …}` as in `weather.expected.json`; `{"$callback":"upper"}` is a function upper-casing its argument), `filter` (`{ "keep": [labels] }`: deliver to entries whose `row` is listed; the probe logs `filter:<row>` for each descriptor it is called with), `via` (`"thisArg"`: Cordis `Context.filter`, Node / Bun Cordis emitters only), `terminal` (text: a terminal that logs `terminal` and returns the text), `cancel` (`{ "when": "<label>" }`: the emitter cancels the dispatch once that listener logged its start).
- **Emitter rows**: the runner creates each emitter a spec names (`row`, default `e`) at its `from` location and `scope`; a listener's `emits` emits from the listener's own row.
- **Steps**, in order: `{ "emit": spec, "expect": … }` (`"background": true`: the step does not wait for the dispatch; the next `unload` / `kill` step's `expect` is about that dispatch's outcome and the log), `{ "load": listener }` (a new row; `provides`: it also provides an empty service of that name), `{ "unload": label }`, `{ "kill": "Y" | "node" }` (end that process / the link; it stays down until a `restart` step), `{ "restart": "Y" }`, `{ "concurrent": [steps], "hold": true }` (`hold`: the host's test hook holds the dispatches until all have arrived; `expect` then applies to each, `throws` meaning at least one). `unload` and `kill` take `"when": "<label>"`: first wait for that listener's start.
- **Expect**: `returns` (value; `null` = no result), `throws` (`{}` any error, or `name`, `message` (substring), `errors` (children in order, each `{}` / `name` / `message`)), `log` (exact order), `logUnordered`, `logContains` (in this order, others may be between), `reported` (`[{ "label", "name" }]`, §5.6), `filterSees` (the rows the filter was called with, in order; the runner checks each descriptor's `runtime`, `node` and `scope` against where that participant actually is), `viaHost` (whether an `events.dispatch` reached the host), `settles` (`true`: the step ends, either way, within 5 s). `logLacks` (entries that must not be in the log). The log accumulates over the whole case; every log expectation is about the whole log so far. The runner waits up to 5 s for logs to be complete.
- **`vary`**: participants (labels, `e` for the emitter) whose location the runner varies over every location (the product), `at` / `from` being the first assignment. `node` is never assigned to a scope other than `root`.
- **`only` / `except`**: `{ "<label>": [languages] }` (`rust`, `node`, `bun`, `python`, `go`) restricts / excludes assignments, with `why`.
- **`tool`**: `false` when the testing tools do not run the case (it needs other processes, `rutis/*` or the host). Tools run every other case with every location set to `X`.

### 13.3 Starter cases

```json
{
  "about": [
    "Unified events conformance (#464): one list both the real runtimes and the SDK testing tools are checked against. Format: docs/design-events-protocol-2026-10-18.en.md §13."
  ],
  "plugin": "conformance-events",
  "scopes": { "t1": "tenant", "t1/s1": "session", "t2": "tenant" },
  "locations": ["X", "Y", "host", "node"],
  "cases": [
    { "case": "emit/reaches-all", "rule": "5.4 emit", "name": "t/emit",
      "listeners": [
        { "label": "a", "at": "X", "does": { "after": 100, "then": { "returns": "A" } } },
        { "label": "b", "at": "host", "does": { "returns": null } },
        { "label": "c", "at": "Y", "does": { "returns": "C" } } ],
      "vary": ["a", "b", "c", "e"],
      "steps": [ { "emit": { "from": "X", "mode": "emit", "args": [1] }, "expect": { "returns": null, "logUnordered": ["a", "b", "c"] } } ] },

    { "case": "emit/order-per-listener", "rule": "5.5", "name": "t/order",
      "listeners": [ { "label": "c", "at": "Y", "log": "arg", "does": { "returns": null } } ],
      "vary": ["c", "e"],
      "steps": [
        { "emit": { "from": "X", "mode": "emit", "args": [1] } },
        { "emit": { "from": "X", "mode": "emit", "args": [2] } },
        { "emit": { "from": "X", "mode": "emit", "args": [3] }, "expect": { "log": ["c:1", "c:2", "c:3"] } } ] },

    { "case": "emit/order-across-fast-path", "rule": "5.2, 5.5", "name": "t/switch", "tool": false,
      "listeners": [
        { "label": "a", "at": "X", "log": "arg", "does": { "returns": null } },
        { "label": "b", "at": "Y", "log": false, "does": { "returns": null } } ],
      "steps": [
        { "emit": { "from": "X", "mode": "emit", "args": [1] }, "expect": { "viaHost": true } },
        { "unload": "b" },
        { "emit": { "from": "X", "mode": "emit", "args": [2] }, "expect": { "log": ["a:1", "a:2"], "viaHost": false } } ] },

    { "case": "emit/error-isolated", "rule": "5.6", "name": "t/emit-error",
      "listeners": [
        { "label": "a", "at": "Y", "does": { "throws": { "name": "Boom", "message": "a failed" } } },
        { "label": "b", "at": "X", "does": { "returns": null } } ],
      "vary": ["a", "b", "e"],
      "steps": [ { "emit": { "from": "X", "mode": "emit", "args": [] },
        "expect": { "returns": null, "logUnordered": ["a", "b"], "reported": [ { "label": "a", "name": "Boom" } ] } } ] },

    { "case": "parallel/all-ok", "rule": "5.4 parallel", "name": "t/par",
      "listeners": [
        { "label": "a", "at": "X", "does": { "returns": "A" } },
        { "label": "b", "at": "host", "does": { "returns": null } },
        { "label": "c", "at": "node", "does": { "returns": "C" } } ],
      "vary": ["a", "b", "c", "e"],
      "steps": [ { "emit": { "from": "X", "mode": "parallel", "args": [] }, "expect": { "returns": null, "logUnordered": ["a", "b", "c"] } } ] },

    { "case": "parallel/errors-in-registration-order", "rule": "5.4 parallel", "name": "t/par-errors",
      "listeners": [
        { "label": "a", "at": "Y", "does": { "after": 300, "then": { "throws": { "name": "AErr", "message": "a" } } } },
        { "label": "b", "at": "X", "does": { "throws": { "name": "BErr", "message": "b" } } },
        { "label": "c", "at": "host", "does": { "returns": null } } ],
      "vary": ["a", "b", "c", "e"],
      "steps": [ { "emit": { "from": "X", "mode": "parallel", "args": [] },
        "expect": { "throws": { "name": "AggregateError", "errors": [ { "name": "AErr" }, { "name": "BErr" } ] }, "logUnordered": ["a", "b", "c"] } } ] },

    { "case": "parallel/one-error-is-aggregated", "rule": "5.4 parallel", "name": "t/par-one",
      "listeners": [ { "label": "a", "at": "Y", "does": { "throws": { "name": "AErr", "message": "a" } } } ],
      "vary": ["a", "e"],
      "steps": [ { "emit": { "from": "X", "mode": "parallel", "args": [] }, "expect": { "throws": { "name": "AggregateError", "errors": [ { "name": "AErr" } ] } } } ] },

    { "case": "serial/first-result", "rule": "1 no result, 5.4 serial", "name": "t/serial",
      "listeners": [
        { "label": "a", "at": "X", "does": { "returns": null } },
        { "label": "b", "at": "Y", "does": { "returns": false } },
        { "label": "c", "at": "host", "does": { "returns": "C" } },
        { "label": "d", "at": "node", "does": { "returns": "D" } } ],
      "vary": ["a", "b", "c", "d", "e"],
      "steps": [ { "emit": { "from": "X", "mode": "serial", "args": ["x"] }, "expect": { "returns": "C", "log": ["a", "b", "c"] } } ] },

    { "case": "serial/no-result", "rule": "1 no result", "name": "t/serial-none",
      "listeners": [
        { "label": "a", "at": "X", "does": { "returnsNothing": true } },
        { "label": "b", "at": "Y", "does": { "returns": false } },
        { "label": "c", "at": "host", "does": { "returns": null } } ],
      "vary": ["a", "b", "c", "e"],
      "steps": [ { "emit": { "from": "X", "mode": "serial", "args": [] }, "expect": { "returns": null, "log": ["a", "b", "c"] } } ] },

    { "case": "serial/zero-is-a-result", "rule": "1 no result", "name": "t/serial-zero",
      "listeners": [ { "label": "a", "at": "Y", "does": { "returns": 0 } }, { "label": "b", "at": "X", "does": { "returns": 1 } } ],
      "vary": ["a", "b", "e"],
      "steps": [ { "emit": { "from": "X", "mode": "serial", "args": [] }, "expect": { "returns": 0, "log": ["a"] } } ] },

    { "case": "serial/error-stops", "rule": "5.4 serial", "name": "t/serial-error",
      "listeners": [
        { "label": "a", "at": "Y", "does": { "throws": { "name": "AErr", "message": "a" } } },
        { "label": "b", "at": "X", "does": { "returns": "B" } } ],
      "vary": ["a", "b", "e"],
      "steps": [ { "emit": { "from": "X", "mode": "serial", "args": [] }, "expect": { "throws": { "name": "AErr", "message": "a" }, "log": ["a"] } } ] },

    { "case": "waterfall/chain", "rule": "5.4 waterfall", "name": "t/wf",
      "listeners": [
        { "label": "a", "at": "X", "does": { "next": "a" } },
        { "label": "b", "at": "Y", "does": { "next": "b" } },
        { "label": "c", "at": "host", "does": { "veto": "c" } } ],
      "vary": ["a", "b", "c", "e"],
      "steps": [ { "emit": { "from": "X", "mode": "waterfall", "args": [] }, "expect": { "returns": "abc", "log": ["a", "b", "c"] } } ] },

    { "case": "waterfall/veto", "rule": "5.4 waterfall", "name": "t/wf-veto",
      "listeners": [
        { "label": "a", "at": "X", "does": { "next": "a" } },
        { "label": "b", "at": "Y", "does": { "veto": "v" } },
        { "label": "c", "at": "node", "does": { "veto": "c" } } ],
      "vary": ["a", "b", "c", "e"],
      "steps": [ { "emit": { "from": "X", "mode": "waterfall", "args": [] }, "expect": { "returns": "av", "log": ["a", "b"] } } ] },

    { "case": "waterfall/terminal", "rule": "5.4 waterfall", "name": "t/wf-terminal",
      "listeners": [ { "label": "a", "at": "Y", "does": { "next": "a" } }, { "label": "b", "at": "host", "does": { "next": "b" } } ],
      "vary": ["a", "b", "e"],
      "steps": [ { "emit": { "from": "X", "mode": "waterfall", "args": [], "terminal": "t" }, "expect": { "returns": "abt", "log": ["a", "b", "terminal"] } } ] },

    { "case": "waterfall/no-listeners", "rule": "5.4 waterfall", "name": "t/wf-empty", "listeners": [],
      "vary": ["e"],
      "steps": [
        { "emit": { "from": "X", "mode": "waterfall", "args": [], "terminal": "t" }, "expect": { "returns": "t", "log": ["terminal"] } },
        { "emit": { "from": "X", "mode": "waterfall", "args": [] }, "expect": { "returns": null, "log": ["terminal"] } } ] },

    { "case": "waterfall/end-without-terminal", "rule": "5.4 waterfall", "name": "t/wf-end",
      "listeners": [ { "label": "a", "at": "Y", "does": { "next": "a" } } ],
      "vary": ["a", "e"],
      "except": [ { "e": ["node", "bun"], "why": "a Cordis waterfall always has a terminal (its last argument); the probe passes one returning undefined, which is the same" } ],
      "steps": [ { "emit": { "from": "X", "mode": "waterfall", "args": [] }, "expect": { "returns": "a", "log": ["a"] } } ] },

    { "case": "waterfall/next-twice", "rule": "5.4 next", "name": "t/wf-twice",
      "listeners": [ { "label": "a", "at": "Y", "does": { "nextTwice": true } }, { "label": "b", "at": "X", "does": { "veto": "b" } } ],
      "vary": ["a", "b", "e"],
      "steps": [ { "emit": { "from": "X", "mode": "waterfall", "args": [] }, "expect": { "throws": { "name": "WaterfallNext" }, "log": ["a", "b"] } } ] },

    { "case": "waterfall/error", "rule": "5.4 waterfall", "name": "t/wf-error",
      "listeners": [ { "label": "a", "at": "X", "does": { "next": "a" } }, { "label": "b", "at": "Y", "does": { "throws": { "name": "BErr", "message": "b" } } } ],
      "vary": ["a", "b", "e"],
      "steps": [ { "emit": { "from": "X", "mode": "waterfall", "args": [] }, "expect": { "throws": { "name": "BErr" }, "log": ["a", "b"] } } ] },

    { "case": "waterfall-sync/chain", "rule": "5.4 waterfall-sync", "name": "t/wfs", "tool": false,
      "listeners": [ { "label": "a", "at": "X", "does": { "next": "a" } }, { "label": "b", "at": "Y", "does": { "next": "b" } } ],
      "vary": ["a", "b"],
      "steps": [ { "emit": { "from": "host", "mode": "waterfall-sync", "args": [], "terminal": "t" }, "expect": { "returns": "abt", "log": ["a", "b", "terminal"] } } ] },

    { "case": "bail/first-result", "rule": "5.4 bail", "name": "t/bail",
      "listeners": [
        { "label": "a", "at": "Y", "does": { "returns": null } },
        { "label": "b", "at": "host", "does": { "returns": "B" } },
        { "label": "c", "at": "X", "does": { "returns": "C" } } ],
      "vary": ["a", "b", "c", "e"],
      "steps": [ { "emit": { "from": "X", "mode": "bail", "args": [] }, "expect": { "returns": "B", "log": ["a", "b"] } } ] },

    { "case": "bail/async-listener", "rule": "5.4 kinds", "name": "t/bail-async",
      "listeners": [ { "label": "a", "at": "Y", "kind": "async", "does": { "returns": "A" } } ],
      "vary": ["a", "e"],
      "except": [ { "a": ["go"], "why": "Go listeners are synchronous" } ],
      "steps": [ { "emit": { "from": "X", "mode": "bail", "args": [] }, "expect": { "throws": { "name": "AsyncListener" }, "log": [] } } ] },

    { "case": "bail/awaitable-from-plain-function", "rule": "5.4 kinds", "name": "t/bail-any",
      "listeners": [ { "label": "a", "at": "Y", "kind": "any", "does": { "after": 10, "then": { "returns": "A" } } } ],
      "vary": ["a", "e"],
      "only": [ { "a": ["node", "bun", "python"], "why": "only JavaScript and Python functions can return an awaitable unannounced" } ],
      "steps": [ { "emit": { "from": "X", "mode": "bail", "args": [] }, "expect": { "throws": { "name": "AsyncListener" }, "log": ["a"] } } ] },

    { "case": "bail/call-back-into-waiting-emitter", "rule": "5.4 bail, sync-wait", "name": "t/bail-out", "tool": false,
      "listeners": [
        { "label": "p", "at": "Y", "does": { "emits": { "from": "Y", "row": "p", "mode": "bail", "name": "t/bail-back", "args": [] } } },
        { "label": "q", "at": "X", "name": "t/bail-back", "does": { "returns": "Q" } } ],
      "vary": ["p", "q"],
      "steps": [ { "emit": { "from": "X", "mode": "bail", "args": [] }, "expect": { "returns": "Q", "log": ["p", "q"] } } ] },

    { "case": "bail/wait-cycle", "rule": "5.4 bail, SyncWaitCycle", "name": "t/c1", "tool": false,
      "listeners": [ { "label": "p", "at": "Y", "does": { "returns": "P" } }, { "label": "q", "at": "X", "name": "t/c2", "does": { "returns": "Q" } } ],
      "only": [ { "X": ["node"], "Y": ["node"], "why": "a cycle needs two runtimes that do not declare reentrant-sync" } ],
      "steps": [ { "concurrent": [
          { "emit": { "from": "X", "mode": "bail", "args": [] } },
          { "emit": { "from": "Y", "row": "f", "mode": "bail", "name": "t/c2", "args": [] } } ], "hold": true,
        "expect": { "settles": true, "throws": { "name": "SyncWaitCycle" } } } ] },

    { "case": "scope/up-not-down", "rule": "3", "name": "t/scope",
      "listeners": [
        { "label": "r", "at": "host", "scope": "root", "does": { "returns": null } },
        { "label": "u", "at": "X", "scope": "t1", "does": { "returns": null } },
        { "label": "s", "at": "Y", "scope": "t1/s1", "does": { "returns": null } },
        { "label": "v", "at": "X", "scope": "t2", "does": { "returns": null } } ],
      "vary": ["r", "u", "s", "v", "e"],
      "steps": [
        { "emit": { "from": "X", "scope": "t1/s1", "mode": "parallel", "args": [] }, "expect": { "logUnordered": ["r", "u", "s"] } },
        { "emit": { "from": "X", "scope": "t1", "mode": "parallel", "args": [] }, "expect": { "logUnordered": ["r", "u", "s", "r", "u"] } },
        { "emit": { "from": "X", "scope": "t2", "mode": "parallel", "args": [] }, "expect": { "logUnordered": ["r", "u", "s", "r", "u", "r", "v"] } },
        { "emit": { "from": "X", "scope": "root", "mode": "parallel", "args": [] }, "expect": { "logUnordered": ["r", "u", "s", "r", "u", "r", "v", "r"] } } ] },

    { "case": "filter/keeps-and-sees", "rule": "6", "name": "t/filter",
      "listeners": [
        { "label": "a", "at": "X", "does": { "returns": null } },
        { "label": "b", "at": "host", "does": { "returns": null } },
        { "label": "c", "at": "Y", "does": { "returns": null } },
        { "label": "d", "at": "X", "scope": "t2", "does": { "returns": null } } ],
      "vary": ["a", "b", "c", "e"],
      "steps": [ { "emit": { "from": "X", "mode": "serial", "args": [], "filter": { "keep": ["a", "c"] } },
        "expect": { "returns": null, "log": ["filter:a", "filter:b", "filter:c", "a", "c"],
          "filterSees": ["a", "b", "c"] } } ] },

    { "case": "filter/cordis-this-arg", "rule": "6 Cordis", "name": "t/filter-cordis",
      "listeners": [ { "label": "a", "at": "Y", "does": { "returns": null } }, { "label": "b", "at": "X", "does": { "returns": null } } ],
      "vary": ["a", "b"],
      "only": [ { "e": ["node", "bun"], "why": "Context.filter is Cordis's" } ],
      "steps": [ { "emit": { "from": "X", "mode": "parallel", "args": [], "via": "thisArg", "filter": { "keep": ["b"] } }, "expect": { "logUnordered": ["filter:a", "filter:b", "b"] } } ] },

    { "case": "filter/global-skips-filter", "rule": "4.2 global, 6", "name": "t/filter-global",
      "listeners": [ { "label": "g", "at": "Y", "options": { "global": true }, "does": { "returns": null } } ],
      "vary": ["g", "e"],
      "steps": [ { "emit": { "from": "X", "mode": "parallel", "args": [], "filter": { "keep": [] } }, "expect": { "log": ["g"] } } ] },

    { "case": "order/prepend", "rule": "4.3", "name": "t/prepend",
      "listeners": [ { "label": "b", "at": "X", "does": { "returns": null } }, { "label": "a", "at": "Y", "options": { "prepend": true }, "does": { "returns": null } } ],
      "vary": ["a", "b", "e"],
      "steps": [ { "emit": { "from": "X", "mode": "serial", "args": [] }, "expect": { "log": ["a", "b"] } } ] },

    { "case": "values/every-tag", "rule": "10", "name": "t/values",
      "listeners": [ { "label": "a", "at": "Y", "does": { "echo": true } } ],
      "vary": ["a", "e"],
      "steps": [ { "emit": { "from": "X", "mode": "serial", "args": [ [
          { "$instant": "2026-10-15T03:22:15.123Z" }, { "$datetime": "2026-10-15T14:30:05.25" }, { "$date": "2026-10-15" },
          { "$time": "14:30:05.25" }, { "$bytes": "aGVsbG8=" }, { "$map": [[1, "a"]] }, { "$set": ["a"] },
          { "$bigint": "-123456789012345678901" }, { "$decimal": "1.10" }, { "$uuid": "0f8fad5b-d9cb-469f-a165-70867728950e" } ] ] },
        "expect": { "returns": [
          { "$instant": "2026-10-15T03:22:15.123Z" }, { "$datetime": "2026-10-15T14:30:05.25" }, { "$date": "2026-10-15" },
          { "$time": "14:30:05.25" }, { "$bytes": "aGVsbG8=" }, { "$map": [[1, "a"]] }, { "$set": ["a"] },
          { "$bigint": "-123456789012345678901" }, { "$decimal": "1.10" }, { "$uuid": "0f8fad5b-d9cb-469f-a165-70867728950e" } ] } } ] },

    { "case": "values/function-argument", "rule": "10", "name": "t/callback",
      "listeners": [ { "label": "a", "at": "Y", "does": { "calls": 0 } } ],
      "vary": ["a", "e"],
      "steps": [ { "emit": { "from": "X", "mode": "serial", "args": [ { "$callback": "upper" } ] }, "expect": { "returns": "X" } } ] },

    { "case": "unload/waits-and-cancels", "rule": "7.1", "name": "t/unload",
      "listeners": [ { "label": "b", "at": "Y", "does": { "waits": true } } ],
      "vary": ["b", "e"],
      "steps": [
        { "emit": { "from": "X", "mode": "parallel", "args": [] }, "background": true },
        { "unload": "b", "when": "b", "expect": { "settles": true, "log": ["b", "b:cancelled"] } } ] },

    { "case": "unload/not-called-after", "rule": "7.1", "name": "t/unloaded",
      "listeners": [ { "label": "a", "at": "X", "does": { "returns": null } }, { "label": "b", "at": "Y", "does": { "returns": "B" } } ],
      "vary": ["a", "b", "e"],
      "steps": [ { "unload": "b" }, { "emit": { "from": "X", "mode": "serial", "args": [] }, "expect": { "returns": null, "log": ["a"] } } ] },

    { "case": "registration/loaded-row-hears", "rule": "4.4", "name": "t/late", "tool": false,
      "listeners": [],
      "steps": [
        { "load": { "label": "z", "at": "Y", "does": { "returns": "Z" } } },
        { "emit": { "from": "X", "mode": "serial", "args": [] }, "expect": { "returns": "Z", "log": ["z"] } } ] },

    { "case": "cancel/emitter-gives-up", "rule": "5.7", "name": "t/cancel", "tool": false,
      "listeners": [ { "label": "b", "at": "Y", "does": { "waits": true } } ],
      "vary": ["b", "e"],
      "except": [ { "e": ["node", "bun"], "why": "JavaScript cannot cancel a dispatch" }, { "b": ["node", "bun"], "why": "JavaScript listeners have no per-invocation signal" } ],
      "steps": [ { "emit": { "from": "X", "mode": "parallel", "args": [], "cancel": { "when": "b" } }, "expect": { "throws": {}, "log": ["b", "b:cancelled"] } } ] },

    { "case": "disconnect/in-flight-fails-once", "rule": "7.2", "name": "t/gone", "tool": false,
      "listeners": [ { "label": "a", "at": "X", "does": { "returns": null } }, { "label": "b", "at": "Y", "does": { "waits": true } } ],
      "steps": [
        { "emit": { "from": "X", "mode": "serial", "args": [] }, "background": true },
        { "kill": "Y", "when": "b", "expect": { "throws": { "name": "BindingError" } } },
        { "emit": { "from": "X", "mode": "serial", "args": [] }, "expect": { "returns": null, "log": ["a", "b", "a"] } } ] },

    { "case": "disconnect/emit-reported", "rule": "7.2, 5.6", "name": "t/gone-emit", "tool": false,
      "listeners": [ { "label": "b", "at": "Y", "does": { "waits": true } } ],
      "steps": [
        { "emit": { "from": "X", "mode": "emit", "args": [] } },
        { "kill": "Y", "when": "b", "expect": { "reported": [ { "label": "b", "name": "BindingError" } ] } } ] },

    { "case": "disconnect/node-link", "rule": "7.2, 9", "name": "t/gone-node", "tool": false,
      "listeners": [ { "label": "n", "at": "node", "does": { "waits": true } }, { "label": "a", "at": "X", "does": { "returns": "A" } } ],
      "steps": [
        { "emit": { "from": "X", "mode": "serial", "args": [] }, "background": true },
        { "kill": "node", "when": "n", "expect": { "throws": { "name": "BindingError" } } },
        { "emit": { "from": "X", "mode": "serial", "args": [] }, "expect": { "returns": "A", "log": ["n", "a"] } } ] },

    { "case": "node/not-shared", "rule": "9", "name": "n/private", "tool": false,
      "listeners": [ { "label": "n", "at": "node", "does": { "returns": "N" } } ],
      "steps": [ { "emit": { "from": "host", "mode": "serial", "args": [] }, "expect": { "returns": null, "log": [] } } ] },

    { "case": "rutis/row", "rule": "8", "name": "rutis/row", "tool": false,
      "listeners": [
        { "label": "w", "at": "X", "scope": "root", "log": ["row", "state"], "does": { "returns": null } },
        { "label": "o", "at": "X", "scope": "t1", "log": ["row", "state"], "does": { "returns": null } } ],
      "steps": [
        { "load": { "label": "z", "at": "Y", "scope": "t2", "name": "t/unused", "does": { "returns": null } } },
        { "unload": "z", "expect": { "logContains": ["w:z,running", "w:z,stopped"], "logLacks": ["o:z,running", "o:z,stopped"] } } ] },

    { "case": "rutis/service", "rule": "8", "name": "rutis/service", "tool": false,
      "listeners": [ { "label": "w", "at": "X", "log": ["name", "available"], "does": { "returns": null } } ],
      "steps": [
        { "load": { "label": "z", "at": "Y", "provides": "zsvc", "name": "t/unused", "does": { "returns": null } } },
        { "unload": "z", "expect": { "logContains": ["w:zsvc,true", "w:zsvc,false"] } } ] },

    { "case": "rutis/runtime", "rule": "8", "name": "rutis/runtime", "tool": false,
      "listeners": [ { "label": "w", "at": "X", "log": ["state"], "does": { "returns": null } } ],
      "steps": [ { "kill": "Y" }, { "restart": "Y", "expect": { "logContains": ["w:down", "w:running"] } } ] },

    { "case": "reserved/rutis-emit", "rule": "8, 11", "name": "rutis/fake", "listeners": [],
      "vary": ["e"],
      "steps": [
        { "emit": { "from": "X", "mode": "emit", "args": [] }, "expect": { "throws": { "name": "ReservedEvent" } } },
        { "emit": { "from": "X", "mode": "serial", "args": [] }, "expect": { "throws": { "name": "ReservedEvent" } } } ] },

    { "case": "fast-path/local-only", "rule": "5.2", "name": "t/local", "tool": false,
      "listeners": [ { "label": "a", "at": "X", "does": { "returns": "A" } } ],
      "steps": [
        { "emit": { "from": "X", "mode": "serial", "args": [] }, "expect": { "returns": "A", "viaHost": false } },
        { "load": { "label": "b", "at": "Y", "does": { "returns": "B" } } },
        { "emit": { "from": "X", "mode": "serial", "args": [] }, "expect": { "returns": "A", "viaHost": true } } ] },

    { "case": "rust/typed-args", "rule": "12.1", "name": "t/typed", "tool": false,
      "listeners": [ { "label": "h", "at": "host", "typed": "struct", "does": { "echo": true } }, { "label": "k", "at": "host", "name": "t/typed-pair", "typed": "tuple", "does": { "echo": true } } ],
      "steps": [
        { "emit": { "from": "X", "mode": "serial", "args": [ { "city": "Oslo" } ] }, "expect": { "returns": { "city": "Oslo" } } },
        { "emit": { "from": "X", "mode": "serial", "name": "t/typed-pair", "args": ["x", 2] }, "expect": { "returns": ["x", 2] } } ] },

    { "case": "rust/next-outside-waterfall", "rule": "11 WaterfallNext", "name": "t/rust-next", "tool": false,
      "listeners": [ { "label": "h", "at": "host", "does": { "next": "h" } } ],
      "steps": [ { "emit": { "from": "X", "mode": "serial", "args": [] }, "expect": { "throws": { "name": "WaterfallNext" } } } ] }
  ]
}
```

Rust-only listener field used above: `typed` (`"struct"`: a typed event `struct { city: String }`; `"tuple"`: `struct Pair(String, i64)`; both derive `Serialize` / `Deserialize`, as every Rust event does). A Rust listener whose `does` is `next` calls `call.next()` in whatever mode it is invoked.

Also checked outside the file (Rust tests): the handshake refusals of §2.2 against a 0.12 runtime and node (the cross-version end-to-end tests); every frame of §4–§9 decoded and refused as written (session unit tests); unload past the deadline (§7.1).

## 14. Where existing protocol changes

| Existing | Change |
| --- | --- |
| `hello.capabilities` | + `events-v2` (mandatory, §2.1) |
| `mount` | − `events`, − `emits`; + `scope`, + `runtime` (§3); reply feature `events` removed |
| `rows.load` | seventh argument `{ scope, id }` (§3) |
| `rows.unload`, `rows.ended`, session end | withdraw the row's / session's entries (§7) |
| runtime → host `event`, host → runtime `emit` | removed; replaced by `events.dispatch`, `events.call` |
| node link `events.forward` | removed; replaced by link `events.on` / `events.off` / `events.call` (§9) |
| link config `export` / `import` | also select events; `*` prefixes (§9) |

## 15. Decisions made in this spec

Where the design was silent, this specification chose:

1. **A separate document** rather than an appendix: the protocol plugin design is historical except §6.1, and its §11 is superseded whole; it and the runtime contract point here.
2. **No session version bump; a mandatory capability `events-v2`** checked right after the handshake on every path, with the exact messages of §2.2. Frames do not change, so bumping protocol 2 / 3 (and `rutis.3`) would only replace a precise upgrade message with a generic version mismatch.
3. **Plugin API 3**; 0.13 runtimes still load API 2 plugins.
4. **The host dispatches** whenever any matching entry is outside the emitter's process; the emitter's process only runs the filter (one batched selector call per dispatch) and the terminal. The emitter's own entries are then invoked back through `events.call`.
5. **Presence as ordered diffs** of names and prefixes, full set first, handled inline; scope is ignored for presence (it only disables the fast path more often).
6. **Emit order means start order** per (emitting process, name); the host answers an `emit` dispatch after starting its invocations; a runtime keeps a name on the host path while an earlier emit of it is unanswered; Go queues emit invocations per listener.
7. **Registration order** is the host's acceptance order with positions; `prepend` takes the lowest position; `rows.load` waits for the row's `events.on` answers; the host queues presence updates before answering `events.on`.
8. **The host withdraws a row's entries** when it sends `rows.unload` or receives `rows.ended`; the runtime refuses that row's invocations at once.
9. **Snapshot, then a liveness check before each start**: withdrawn entries are skipped silently; invocations already sent fail once with `BindingError` (`listener <id> of <name> is withdrawn`) or the transport error.
10. **Listener kinds** `sync` / `async` / `any`: declared-async entries are never invoked by `bail` / `waterfall-sync`; `any` entries are checked by what they return.
11. **`parallel` always throws `AggregateError`**, even for one failure (Cordis).
12. **`waterfall` returns the outermost value as is**; with no entries, the terminal's value or no result; `next` takes no arguments; the terminal receives the event arguments; JavaScript's terminal is Cordis's mandatory last argument, optional elsewhere.
13. **The filter runs over the scope-matched snapshot before any invocation**; `global` entries skip it (not the scope rule); a throwing filter fails the dispatch (in `emit`, reported).
14. **Cordis filter contexts**: real `Context` for local hooks, `{ rutis: descriptor }` for others, and `ctx.rutis` on every Context; other languages use `filter(predicate)` returning an emitter.
15. **Scopes are loader group instances**, `[{id, group}]` chains, root `[]`, delivered by a seventh `rows.load` argument and a `mount` field; the host derives the emitter scope from the row and never trusts a reported one; Rust gets `Ctx::scope` / `Ctx::mark_scope`.
16. **Cancellation per language**; JavaScript listeners have no per-invocation signal (Cordis's listener signature has no place for one) and see only `ctx.signal` at unload; JavaScript cannot cancel a dispatch.
17. **Unload waits at most `RUTIS_SHUTDOWN_TIMEOUT`** (default 10 s), then writes the §7.1 line and goes on.
18. **`rutis/*` payloads** are one record each with lowercase states; fired in `emit` mode at the subject's scope; a provider replacement fires `available: true` again.
19. **Node links**: `export` = my emissions reach the peer, `import` = my listeners hear the peer (so `events.out` → `export`, `events.in` → `import`); entries travel from importer to exporter; `*` suffix = prefix, for events only; only a node's own entries at or above the link's scope are announced; imported entries are never relayed (no multi-hop events); accepted entries sit at the link's scope; a name may be shared both ways.
20. **Every Rust event type crosses processes**: `Event` requires `Serialize + DeserializeOwned` (derived), its `Value` too, so no type is ever process-local and there is no opt-in (amended 2026-10-18; the earlier `const WIRE` / `NoWire` are dropped). Listeners are found by name; a listener of another Rust type with the same name gets the event converted through serde by the bus's transcoder. Argument mapping by tuple / unit / single value; dynamic events are `rutis_bridge::events::Args`; `SyncEvent`, instance keys and the deprecated keyed / instance methods are removed with `on_waterfall*`; `Terminal` returns `Option` (`()` = none); `on` keeps its registering `ctx` parameter, and a Rust listener is called with that context. The kernel adds `Ctx::mark_row` (descriptor `row` of host Rust listeners) and, for the bridge, remote entries (`RemoteListener`), the raw `dispatch` / `dispatch_sync`, `invoke_entry` and `watch_table` (§12.1).
21. **A Rust `on_sync` listener cannot call `next` in an asynchronous waterfall** (`WaterfallNext`); register with `on` for that.
22. **Go listeners are `func(ctx, args ...any) (any, error)`** with `next` as the last argument in a waterfall; `Emit` returns an error only for refusals.
23. **`emit`-mode `events.call` is answered after the listener finished**, always with `undefined`; failures are reported where the listener runs, with the §5.6 line.
24. **Error names and messages** of §11, including `AsyncListener`, `WaterfallNext`, `ReservedEvent`; transport-class failures keep `BindingError`.
25. **Node / Bun fire Cordis `internal/dispatch` once per dispatch** in each process that runs a listener of it, besides the emitter's own Cordis dispatch.
26. **Testing tools** run in one process with scopes, filter, unload waiting and cancellation, without `rutis/*`; the expectation file marks cases they skip with `tool: false`.
27. **`prepend` and `global` exist in every language** (amended 2026-10-18): Cordis's own options on Node / Bun Cordis plugins, `ctx.on(name, fn, { prepend, global })` for Node / Bun leaves, `ctx.on(name, fn, prepend=…, global_=…)` in Python, `rutis.Prepend()` / `rutis.Global()` in Go, `EventOptions` in Rust (§12).

Decided while implementing the host side (#464, `events/host`):

28. **`mount` names the runtime** (`runtime`), so fast-path descriptors equal the host's.
29. **The host keeps a session's `emit` dispatches in order** without blocking its reader: an `emit` it can start at once (no pending filter selector) is answered `undefined` before the next frame is read; one that cannot (its selector call must come back over the same session) and every later `emit` of that session wait in a queue, each answered once its invocations started (the answer is then a future the runtime awaits). §5.5's guarantees hold either way.
30. **The host handles `events.on`, `events.off`, `events.elsewhere`, and `emit`-mode `events.dispatch` / `events.call` on the session reader**, in frame order (the session layer runs these control operations inline); every other event operation runs as a task, like any call.
31. **A node announces each own entry once per `import` item it matches** (narrowed as §9 says), each announcement with an id of its own; an entry another node announced is accepted once per `export` item it matches, the same way. Announcements start when the peer offers the `events` family and start over at each new offer of it.
32. **`AggregateError` on the wire** carries its children in its error graph (§11), so every language rebuilds the children in registration order.
33. **`rutis/row`**: `pending` is emitted when the loader spawns the row, then each change of its fiber's state as the loader sees it (`loading` counts as `pending`; `unloading` and disposal as `stopped`); a group row is a row too (`plugin` `group`). **`rutis/service`** is emitted for the services a runtime row provides to rutis (as the host projects them), at the row's scope, with the row's id.
