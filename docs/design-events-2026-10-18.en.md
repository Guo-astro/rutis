# Unified events: one logical bus

[中文](design-events-2026-10-18.md)

Status: proposal (2026-10-18), superseding the drafts in #340, #358 and #359, and boundary rules 2–5 for events in `requirements-protocol-plugins` §5. Old behavior is not kept.

## 1. Goal

Whatever form a plugin takes — a Rust core plugin, a Cordis plugin, a Node / Bun / Python / Go leaf plugin, a remote runtime, or a plugin on another node — events have **one set of rules and one way of use**, and are transparent to developers:

> Wherever an event is emitted and wherever its listeners are, the result is the same as if all listeners were in one process.

There is only one case where this cannot be done: when it is physically impossible (a sync wait that forms a cycle, a sync wait on a peer that has disconnected). Then an explicit error is raised; it is never silently replaced by some other behavior (design philosophy §7, principle 7).

## 2. Current state (2026-10-18, `ffd6e8b3`)

Today only the Rust core bus has one set of rules internally. Every other path has its own, and they do not connect, or only half connect:

| Path | Modes available | Key | Problems |
| --- | --- | --- | --- |
| Rust in-process | emit / parallel / serial / waterfall / sync bail, waterfall | type (`EventKey<E>`), name, instance | inconsistent even on its own: non-instance listeners are not filtered by liveness, and unloading does not wait; `parallel` errors are in completion order |
| Cordis ↔ rutis | everything treated as `parallel`; sync `bail` errors; `waterfall` becomes a stretch of async | generated types | dispatch mode lost (#340); only events without a return value can be forwarded; `internal/*` cannot cross (#359) |
| rutis → leaf runtime | only a manual `Process::emit` by the embedder, treated as `parallel` on the other side | string | not connected to the rutis bus; no entry in rutis-host configuration; none for dial-in instances |
| leaf runtime → rutis | **does not exist** | — | leaf `ctx.emit` stays inside its own process |
| runtime ↔ runtime | **does not exist** | — | one isolated bus per process |
| node ↔ node | treated as `parallel`, JSON only | `NodeEvent` dynamic name | mode lost; new value types rejected; silently dropped on disconnect; crosstalk between instances |

Leaf plugins have only `on` / `emit`, and the four languages do not even agree on those two: whether `emit` waits for listeners, whether an error is thrown / returned / ends the process, whether unloading waits for running listeners, whether a single error is wrapped — all differ.

Real plugins need far more than "notifications". The published dsh plugins have about 30 listeners using `waterfall` (middleware, transforms), `serial` (take the first result) and `parallel` (a barrier that waits for all), and rely heavily on filtering events by their own session or agent (Cordis's `thisArg.filter` or event names). Seven more places hand-write an emit where "one listener's error does not affect the others".

## 3. Model

### 1. One logical bus

All listeners on one host — those of Rust plugins, of every plugin in every runtime process, and those brought in through node links — form **one logical bus**. rutis is the center of this bus: each runtime registers its listeners with the host, just as services have been registered automatically since 0.12.

- **Registration is automatic.** Plugins write `ctx.on(name, listener)` as usual, with nothing to declare.
- **Dispatch follows the global listener table.** When an event is emitted, rutis runs all listeners for that name in registration order. Listeners in this process are called directly; those elsewhere are sent one call each, made according to the dispatch mode.
- **Local fast path.** When all listeners for a name are in the emitter's process, dispatch happens locally without going through the host. The host pushes to each runtime "which names also have listeners elsewhere", and the runtime uses that to decide between local dispatch and going through the host. Until an emit sent through the host is answered, later emits of that name keep going through the host; the runtime takes that answer in frame order, as it reads the reply (2026-10-21 revision), so an emit started by a call that arrives after the host answered goes local again.

### 2. Event = name + arguments + scope

- **The name** is a string, shared by every language and every path. The convention is `area/verb`, such as `session/created`.
- **Arguments** cross boundaries under the value type rules (the ten values of 0.11, objects and functions passed by reference). Between nodes too, no longer limited to JSON.
- **The scope** is the instance the emitter is in, but it is a **descriptor**, not a delivery rule: events are delivered by name on one root's bus, and every listener receives them, as in Cordis's `EventsService`. The chain appears in `ListenerInfo` for filters; to keep an event within an instance, filter by `info.scope` or use distinct names. There is no default instance isolation.
- **Filter**: an emit can carry a filter function, called once per listener with that listener's description (plugin name, row id, scope chain), returning whether to deliver. The filter runs **in the emitter's process**, and the listener descriptions are provided by the logical bus, so it works the same for listeners anywhere, with no cross-process call. All five languages use the same mechanism; Cordis's `Context.filter` (the filter on `thisArg`) connects here: for Cordis's own listeners it is given their context, and for listeners elsewhere it is given a context representing them (with scope and plugin information). A scope is not a default filter; a filter narrows by its own predicate.
- **Typed events in Rust**: an event type uses `Event::NAME` as its cross-language name, with arguments encoded and decoded by serde. Types are a Rust language feature and are kept; across languages, events correspond by name.

### 3. Five dispatch modes, the same meaning everywhere

The semantics follow Cordis (real plugins are all written against it). The table below is the whole rule set, regardless of where listeners are:

| Mode | Emitter gets | Execution | Return value | Listener error |
| --- | --- | --- | --- | --- |
| `emit` | returns immediately, no waiting | calls each listener in registration order; events with the same emitter and name reach each listener in emit order | ignored | **affects neither the emitter nor other listeners**; reported on the listener's side (the host's error collection, the runtime's stderr status line). No longer ends the process |
| `parallel` | waits for all to finish | started in registration order, async parts overlap | ignored | all run to completion; on failure, errors are aggregated in **registration order** and thrown (each language's aggregate error type) |
| `serial` | the first return value that "has a result" | awaits each in registration order | stops and returns it when the return value is not `null` / `undefined` / `false` (Rust: `Some`) | thrown, stops |
| `waterfall` | the outermost listener's return value | outside in, each listener gets `next`; `next` may be a function in another process (passed by reference) | passed along the chain; not calling `next` vetoes the rest | thrown to the emitter; calling `next` twice is an error |
| `bail` (sync) | the first return value with a result, synchronously | calls synchronously in registration order; listeners elsewhere run as **sync calls** (the same mechanism as sync service methods, taking part in wait-cycle detection) | same as `serial` | thrown, stops; an explicit error on a wait cycle (`SyncWaitCycle`) or a disconnected peer. **An async listener** (JS returning a Promise, Python returning a coroutine, a Rust async listener) **is an explicit error**, the same in every language |

Rust's `waterfall_sync` works the same way as sync `bail`.

"Has a result" is judged the same in every language: returning `null` / `undefined` (Python `None`, Go `nil`, Rust `None`) or `false` means no result; anything else is a result. Rust's `Some(false)` also counts as no result, consistent with the other languages.

**One listener table.** A listener is registered once and can be dispatched by any mode; the emitter chooses the mode (as Cordis already does). In `waterfall`, a listener gets `next` from its arguments. Rust drops the separate waterfall registration (`on_waterfall*`), and listeners become uniformly:

```rust
ctx.events().on(&ctx, &key, |ctx, event: &E, call: Call<E::Value>| async move {
    // call.mode(): which mode this is; call.next(..): waterfall only, an error in any other mode
    Ok(None) // or Ok(Some(v))
})
```

Sync listeners are registered with `on_sync` (same table, marked as sync) and can be called by every mode; async listeners cannot be called by sync `bail` / `waterfall_sync`.

### 4. How each language writes it

Every language has all five modes, written the language's native way:

| | Listen | Emit |
| --- | --- | --- |
| Rust | `ctx.events().on(&ctx, &key, ..)`, `on_sync` | `emit` / `parallel` / `serial` / `waterfall` / `bail_sync` / `waterfall_sync` |
| Node (including Cordis plugins), Bun | `ctx.on(name, fn)` | `ctx.emit` / `await ctx.parallel` / `await ctx.serial` / `ctx.waterfall` / `ctx.bail` (for Cordis plugins, Cordis's own API; Bun has no Cordis, only leaf plugins) |
| Python | `ctx.on(name, fn)` | `ctx.emit` / `await ctx.parallel` / `await ctx.serial` / `await ctx.waterfall` / `ctx.bail` |
| Go | `ctx.On(name, fn)` | `ctx.Emit` / `ctx.Parallel` / `ctx.Serial` / `ctx.Waterfall` / `ctx.Bail` |

Each language's test tools provide the same modes and are compared against the same expectation file (as with value types).

### 5. Listener lifecycle

- A listener belongs to the plugin (row) that registered it and is unregistered when that plugin unloads, the same in every language.
- **Unloading waits for running listeners to finish**, at most until the shutdown deadline, while sending a cancellation signal (Go's context, Python task cancellation, JS's AbortSignal). Today Go and Rust instance listeners work this way and the rest do not; this becomes the only behavior.
- Events emitted after unloading no longer reach it; a call in flight to it fails once with a transport error, as with service calls.

### 6. Disconnects and failures

- When a runtime process or node disconnects, its listeners are taken off the logical bus, as if their plugins had unloaded. Later events are no longer sent to them — this matches what "the listener is gone" means in-process, and is not silent loss.
- Within a dispatch in progress, a call to the disconnected side fails once: `parallel` / `serial` / `waterfall` / `bail` hand the error to the emitter; `emit` records a line in the report.
- No call timeout is added (consistent with service calls).

### 7. Framework events

`internal/*` remains each runtime's own framework-internal mechanism and does not cross boundaries: what it carries is meaningful only inside that process (listener registration hooks, property access proxies).

Framework information needed across languages is published as ordinary events under the reserved `rutis/` namespace, which every language can listen to:

| Event | Arguments |
| --- | --- |
| `rutis/service` | service name, scope, appeared or withdrawn |
| `rutis/row` | row id, scope, state (Pending / Running / Failed / Stopped) |
| `rutis/runtime` | runtime name, state (running / down + reason) |

Plugins cannot emit `rutis/*`, only listen to it. The `internal/dispatch` and `internal/service` that Cordis plugins use in-process are unchanged; before an event arriving from elsewhere calls a listener in this process, the Node runtime fires `internal/dispatch` once with rutis's mode name, so validation plugins on it still see such events.

### 8. Between nodes

Listeners on another node become entries on this node's logical bus through the node link, under exactly the same rules as on the local machine (five modes, scope, value types, disconnects).

Which events nodes share is decided by the node link's existing `export` / `import`, as for services: by name or prefix. The old `events: {out, in}` is merged into it.

## 4. What is physically impossible

Only sync waits: when sync `bail` / `waterfall_sync` waits on listeners elsewhere, a cycle raises `SyncWaitCycle` and a disconnected peer raises a transport error, the same as sync service calls; an async listener is an error. Beyond that there are no boundary rules; the rules are section 3.

## 5. Old behavior is not kept

- **Protocol**: the event protocol moves to a new version (listener registration, dispatch with mode and filter, calls to listeners, `rutis/*` events). Mixed with 0.12 or earlier runtimes and nodes, mounting / connecting fails outright, with a message saying which side to upgrade. No old path is left.
- **Removed configuration and APIs**: `RuntimePlugin::emits` / `LocalRuntime::emits`, `Mount.emits` / `events`, `events` / `emits` in Cordis mounts and the generator, `events: {out, in}` in node links, Rust's `on_waterfall*`. Writing them is an error, and the error gives the replacement; `docs/migration-0.13` lists every change.
- **Behavior changes** (stated in the release notes): an `emit` listener's error no longer ends the process and no longer interrupts other listeners; Go's `Emit` no longer waits for listeners; Rust `parallel` errors are in registration order; all Rust listeners are filtered by liveness and waited for on unload; `Some(false)` no longer counts as a result; sync `bail` errors on an async listener (Cordis used to treat the Promise as the result).
- **Event boundary rules**: rules 2–5 in `requirements-protocol-plugins` §5 are deleted and replaced by section 3 of this document; `boundary/5.2`–`5.5` in the guarantee registry are rewritten.

## 6. Implementation

Done in one go, merged together, released in one version:

1. **Protocol specification first**: frame format, capabilities, listener descriptions, calls and cancellation, `rutis/*` events. Written into `design-protocol-plugins` and the runtime contract (`.agents/skills/rutis-new-runtime/references/runtime-contract.md`) as the common basis for every implementation.
2. **Implemented together**: Rust core and bridge (logical bus, entries for listeners elsewhere, one listener table, filters, scope, fixing the inconsistencies); Node (Cordis plugins registering through Cordis `internal/listener`, using Cordis's five modes directly) and Bun; Python, Go (five modes, filters, waiting on unload, test tools); node links (`export` / `import` covering events).
3. **One test suite**: an event expectation file that the real runtimes and test tools of all five languages are compared against; a matrix covering "emitter × listener location × mode × scope × filter × disconnect"; events added to the two-node and netem scenarios; the old boundary rule tests deleted or rewritten.
4. **Documentation**: an "Events" section in each language guide; plugin API, migration notes, guarantee registry, agent skills.
5. **Overall review, release gate rehearsal, release.**

The Swift and Kotlin SDKs start after this, implementing the new protocol directly.

## 7. Basis

- Design philosophy §7: principle 1 (the model is implemented once, in the Rust core), principle 7 (what cannot be done becomes a rule, not a silent degradation), principle 10 (explicit over implicit).
- Value types V0: plugins write only code native to their language.
- Automatic service export in 0.12: registration is automatic, declarations optional. Events follow the same approach.
- Cordis 4.0.4 `events.ts`: the reference semantics for the five modes.
