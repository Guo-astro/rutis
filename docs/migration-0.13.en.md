# Migrating to 0.13

[中文](migration-0.13.md)

0.13 unifies events ([design](design-events-2026-10-18.en.md), [protocol spec](design-events-protocol-2026-10-18.en.md)): one logical bus, five modes that mean the same everywhere. Old behaviour is not kept. This page lists the changes by area.

## Rust kernel (`rutis`)

**Event types are serde.** `Event` now requires `Serialize + DeserializeOwned`, and so does `Event::Value`:

```rust
#[derive(Serialize, Deserialize)]          // new
struct Joined { room: String }
impl Event for Joined { const NAME: &'static str = "room/joined"; type Value = String; }
```

A value type that is not serde (a handle, a channel) does not belong in an event: put it in a service.

**Listeners are found by name.** `EventKey::<E>::of()` is named `E::NAME` (before, `NAME` was only for diagnostics and `of()` was a channel of its own). Two types with the same `NAME` now share listeners, converted through serde: give unrelated types distinct names. `named` / `dynamic` are unchanged.

**One listener table.** A listener registered once is called by every mode; the emitter chooses the mode.

| Before | Now |
| --- | --- |
| `on(ctx, key, \|ctx, e\| ..)` | `on(ctx, key, \|ctx, e, call: Call<'_, V>\| ..)`; `call.mode()`, `call.next()` (waterfall only) |
| `on_waterfall(ctx, key, \|ctx, e, next\| next.call())` | `on(ctx, key, \|ctx, e, call\| call.next())`; the value is `Option<V>` |
| `on_sync` / `on_waterfall_sync` of a `SyncEvent` | `on_sync(ctx, key, \|ctx, e, call: SyncCall<'_, V>\| ..)` for any event; `SyncEvent` is gone |
| `on_pattern(ctx, EventPattern::any_prefix([..]), ..)` | `on_pattern(ctx, &EventPattern::prefix(p), \|ctx, key, e, call\| ..)`, one prefix per registration |
| `*_keyed(name, ..)`, `*_instance(id, ..)`, `EventKey::instance(id)` | `EventKey::named` / `dynamic`; scopes instead of instance channels (below) |
| `serial(ctx, key, &e)` | `serial(ctx, key, Arc<E>)` |
| `waterfall(..) -> Result<V>`, `Terminal` returning `V` | `-> Result<Option<V>>`, `Terminal` returning `Option<V>`; `()` is "no terminal" |
| `waterfall_sync(.., \|ctx, e\| Ok(v))` | `\|ctx, e\| Ok(Some(v))` |

**Behaviour changes.**

- A listener is called with the `Ctx` that registered it, not the emitter's.
- `emit` starts every listener in registration order without waiting for any (before: one after another); one name's emits still start in emit order. A failure is reported to the error sink as `CordisError::ListenerFailed` and never stops the other listeners.
- `parallel` failures are always `CordisError::ListenersFailed` (AggregateError) in registration order, also for one failure (before: one error as is, several as `Aggregate` in completion order).
- `Some(false)` is no result in `serial` / `bail_sync`.
- `bail_sync` / `waterfall_sync` meeting an async (`on`) listener fail with `AsyncListener`.
- Every listener is skipped once its plugin is unloading, and the unload waits for its running calls (their `ctx.cancelled()` fires); before, only instance, pattern and synchronous listeners were.
- `rutis/*` names are reserved: emitting one fails with `ReservedEvent`.

**Scopes replace instance channels.** The loader marks each group instance with `Ctx::mark_scope(group)`; `Ctx::scope()` is the chain from the root. A listener hears events emitted in its scope and below it. To pick listeners further, `ctx.events().filter(|info| ..)` gives the same emitting methods filtered by each listener's `ListenerInfo`.

**Diagnostics.** `EventSubscription`, `EventBacklog` and `DispatchAttempt` carry the event `name` instead of a `TypeKey`; `DispatchMode` is `rutis::Mode`; `ReentrantEvent` carries a `name`.

**No per-type opt-in.** Every event type crosses processes; there is no `WIRE` switch or the like (it appeared only in a draft of the design and was never released).

## What to upgrade

- **Every end together to 0.13**: rutis-host (or the embedded `rutis-bridge` / `rutis-loader`), every runtime package (`@arcships/rutis-runtime`, `@arcships/rutis-bun`, Python's `rutis`, the Go module `github.com/arcships/rutis/go/rutis`) and every linked node. The event protocol has no old path: a runtime or node of 0.12 or earlier connecting to a 0.13 host is refused before anything loads, and the message says which end to upgrade:

  ```text
  incompatible session: the far end (<name> <version>) does not declare events-v2 (the event protocol of rutis 0.13): upgrade it to rutis 0.13 or later
  ```

  When the far end does not report its implementation (the compat format of local runtimes), the parentheses read `a runtime older than rutis 0.13`. A node link stops with `link stopped: incompatible (…)` and does not retry. The other way round, a 0.13 runtime meeting an older host exits with status 1, on stderr: `rutis: refused: incompatible (this host does not declare events-v2 (the event protocol of rutis 0.13): upgrade rutis-host to 0.13 or later); stopping`.
- **Plugin API 3**: plugins written with the 0.13 SDKs are tagged API 3 (adding `parallel`, `serial`, `waterfall`, `bail` and `filter`), and an older runtime refuses them with the existing message; 0.13 runtimes still load API 2 plugins. A plugin using only `on` / `emit` needs no code change, but `emit` behaves differently (below).
- **Nothing to declare in `rutis.json`**: listeners register themselves, and the host side no longer configures event names.

## Leaf plugins (Node, Bun, Python, Go)

A `ctx.on` listener now receives the events emitted in its plugin's scope and inside it, whatever process, language or node the emitter is in; before, only those of its own runtime process.

**`emit` no longer waits or throws.** It starts every listener in registration order and returns at once; a listener's failure is written as one line where the listener runs, `rutis: listener of <name> in row <row> failed: <ErrorName>: <message>`, reaches neither the emitter nor the other listeners, and no longer ends the runtime process. To wait for the listeners and get their errors, use `parallel`:

```ts
// Before: a synchronous throw stopped later listeners; an unhandled async failure ended the process
ctx.emit('session/created', id)
// Now: waits for all; failures are an AggregateError (registration order)
await ctx.parallel('session/created', id)
```

For the listeners' return values use `serial` (the first result), `waterfall` (middleware) or the synchronous `bail`. A hand-written loop that kept one failing listener from stopping the others can become `emit`.

**Go.** Listeners return a value, and `Emit` no longer waits:

```go
// Before
ctx.On("session/created", func(ctx context.Context, args ...any) error { return nil })
err := ctx.Emit(callerCtx, "session/created", id) // ran synchronously, returned the first error

// Now
ctx.On("session/created", func(ctx context.Context, args ...any) (any, error) { return nil, nil })
err := ctx.Emit(callerCtx, "session/created", id)     // returns at once; err is only a refusal
err = ctx.Parallel(callerCtx, "session/created", id)  // waits for all; *rutis.AggregateError
```

A listener's `ctx` now carries the emitter's call chain and is cancelled when the emitter gives up or the row unloads.

**Unloading waits for running listeners** (in every language): the row's listeners leave the bus first, the running ones get a cancellation signal (JavaScript `ctx.signal`, Python task cancellation, Go's `ctx`) and are waited for, at most `RUTIS_SHUTDOWN_TIMEOUT`, before the cleanups run.

**Names.** `internal/*` still cannot be listened to or emitted; `rutis/*` are now reserved framework events (`rutis/service`, `rutis/row`, `rutis/runtime`): listen to them, emitting one fails with `ReservedEvent`. A plugin's own events named `rutis/…` need a new name.

## Test tools

Declaring host events at load is gone: listeners register themselves and a test emits with the five modes.

| Before | Now |
| --- | --- |
| Node / Bun: `load(plugin, { emits: ['tick'] })`, `await t.emit('tick', 1)` waiting for listeners | `load(plugin)`; `t.emit('tick', 1)` returns at once, `await t.parallel('tick', 1)` waits; also `serial`, `waterfall`, `bail`, `filter`. Several plugins on one bus: `const h = host(); await h.load(a); await h.load(b, { scope: 't1' })` |
| Python: `load(plugin, emits=["tick"])`, `await t.emit("tick", 1)` | `load(plugin)`; `t.emit("tick", 1)` returns at once, `await t.parallel("tick", 1)` waits. A shared bus: `bus = Bus(); load(a, bus=bus)`, `load(b, bus=bus, scope="t1")` |
| Go: `rutistest.Load(t, plugin, config, services, "tick")`, `loaded.Emit(ctx, "tick", 1)` waiting for listeners | `rutistest.Load(t, plugin, config, services)` (then options such as `rutistest.InScope`, `rutistest.Row`); `loaded.Emit` returns at once, `loaded.Parallel` waits; `loaded.On` listens as the host does |

Still passing `emits`: Node / Bun say `the config field emits was removed in rutis 0.13: listeners register themselves; emit with ctx.emit / parallel / serial / waterfall / bail (docs/migration-0.13.md)`, Python says `the load option emits was removed in rutis 0.13: …`, Go does not compile (the parameter type changed). Failures of `emit` listeners are no longer thrown: find them in `t.reported` (Python `t.reported`, Go `loaded.Reported()`).

## Rust applications embedding rutis (`rutis-bridge`, `rutis-loader`)

Host → runtime and runtime → host need no declaration and have no separate entry point any more: the Rust plugins in the host and the plugins in runtimes are on one logical bus.

| Removed | Instead |
| --- | --- |
| `LocalRuntime::emits`, `RuntimePlugin::emits` | Nothing: listeners in runtimes register themselves |
| `Mount.emits`, `Mount.events` (`EventSink`) | Nothing; listen on the Rust side with `ctx.events().on(..)` to what runtimes emit |
| `Process::emit(name, args)` | The five modes on `ctx.events()`; `Process::emit` waited for every listener, which is `parallel` |
| `runtime::events::{Events, EmitToCordis, EventSink}` (also re-exported from `rutis_bridge::cordis`) | As above |
| Nodes: `EventsPlugin`, `NodeEvent`, `node_event(name)`, `Features { outbound, inbound }` | `Features { export, import }` (shared by events and services, see [node links](#node-links) below); `PeerPlugin` starts `LinkEventsPlugin` itself; by name with `EventKey::<rutis_bridge::events::Args>::dynamic(name)` |

Before:

```rust
let node = LocalRuntime::node(runtime_dir, "app/package.json")
    .emits(["file-changed"]);
// …
process.emit("file-changed", vec![json!("a.txt").into()]).await?;
```

Now, by name with any arguments:

```rust
use rutis::EventKey;
use rutis_bridge::events::Args;
use rutis_bridge::session::Value;

let node = LocalRuntime::node(runtime_dir, "app/package.json");
// …
let key = EventKey::<Args>::dynamic("file-changed");
ctx.events().parallel(&ctx, &key, Arc::new(Args(vec![Value::Data(json!("a.txt"))]))).await?;
```

Or with a typed event (a one-field tuple struct is one argument):

```rust
#[derive(Serialize, Deserialize)]
struct FileChanged(String);
impl Event for FileChanged { const NAME: &'static str = "file-changed"; type Value = (); }

ctx.events().parallel(&ctx, &EventKey::of(), Arc::new(FileChanged("a.txt".into()))).await?;
```

When a plugin in a runtime calls `ctx.emit("file-changed", …)`, the host's Rust listeners `on` the same key receive it too; that direction did not exist before.

## Cordis mounts (the `cordis` feature of `rutis-bridge`)

`events` and `emits` of the mount manifest and the generator are removed, and the generator no longer generates event types:

```toml
[package.metadata.rutis-cordis.mounts.credentials]
plugin = "@deepseek-ai/dsh-credentials-local"
events = ["credentials/record-updated"]   # remove
emits = ["file-changed"]                  # remove
```

Still there, the build fails: `mount credentials: the config field events was removed in rutis 0.13: listeners register themselves; emit with ctx.emit / parallel / serial / waterfall / bail (docs/migration-0.13.md)`. `.event(name)` and `.emit(name)` of `rutis_bridge::cordis::build` are removed as well.

On the Rust side define the event type yourself (named by the Cordis event's name), or receive it by name with `Args`:

```rust
#[derive(Serialize, Deserialize)]
struct RecordUpdated(serde_json::Value);  // the argument the Cordis plugin emits
impl Event for RecordUpdated { const NAME: &'static str = "credentials/record-updated"; type Value = (); }

ctx.events().on(&ctx, &EventKey::<RecordUpdated>::of(), listener)?;
```

The mounted Cordis plugins emit with Cordis's own `ctx.emit` / `parallel` / `serial` / `waterfall` / `bail`, and Rust listeners are called with Cordis's semantics; events Rust emits reach the Cordis listeners. Before, only events without a return value could be forwarded and a `waterfall` became a stretch of async code; now the five modes mean the same everywhere.

## Cordis plugins

No code changes. What changes:

- Events cross processes: a Cordis plugin's `ctx.on` receives events emitted by other runtimes, the host and nodes (by scope), and its `ctx.emit` and the rest reach them.
- A synchronous `ctx.bail` meeting an asynchronous listener (an `async` function, or one returning a Promise) fails with `AsyncListener`; before, Cordis took the Promise as the result.
- `parallel` failures are always an `AggregateError` in registration order, even a single one.
- `Context.filter` is passed `{ rutis: descriptor }` for listeners elsewhere; every Context has a new read-only `ctx.rutis`.

**A Cordis application as a node** (`@arcships/rutis-runtime/bridge`): `out` / `in` of `Events` become `export` / `import`:

```js
// Before
ctx.plugin(Events, { peer: 'main', out: ['tock'], in: ['tick'] })
// Now
ctx.plugin(Events, { peer: 'main', export: ['tock'], import: ['tick'] })
```

Still writing `out` / `in` gives `the peer config field events was removed in rutis 0.13: list events: { out } in export and events: { in } in import (docs/migration-0.13.md)`.

## Node links

`events: { out, in }` of the node row is removed; events share `export` / `import` with services:

Before:

```json
{ "id": "office", "name": "rutis-bridge/peer", "config": { "peer": "office", "dial": "wss://office.example.com/rutis", "export": ["clock"], "events": { "out": ["tick"], "in": ["tock"] } } }
```

Now:

```json
{ "id": "office", "name": "rutis-bridge/peer", "config": { "peer": "office", "dial": "wss://office.example.com/rutis", "export": ["clock", "tick"], "import": ["tock"] } }
```

Still there, it gives `the peer config field events was removed in rutis 0.13: list events: { out } in export and events: { in } in import (docs/migration-0.13.md)`.

- An item is a name, or a prefix ending in `*` (`session/*`; `*` alone is every name); prefixes apply to events only. A name in `export` exports that service and also lets this node's events of that name reach the far end, so a name that is both a service and an event shares both.
- Events between nodes are now the far listeners as entries on this node's bus: the five modes, scopes, filters and every value type as on one machine (before: always `parallel`, JSON only, silently dropped on a disconnect). When the link goes away they leave the bus, and calls in progress fail once. Events travel one hop: what a node imports is not shared further.
- The far listeners sit in the scope of the node row; events no longer leak between instances, by the scope rule.

## Behaviour changes at a glance

| Change | Where |
| --- | --- |
| A failing `emit` listener no longer ends the process, stops the other listeners or reaches the emitter; it is reported in one line where it runs | Node, Bun, Python, Go, Rust |
| Go's `Emit` no longer waits for listeners; its error is only a refusal | Go |
| `parallel` failures are always an `AggregateError`, children in registration order, even a single one | Every language |
| `false` is no result, Rust's `Some(false)` too | `serial`, `bail` |
| A synchronous `bail` / `waterfall_sync` meeting an asynchronous listener (or a function returning a Promise / coroutine) fails with `AsyncListener` | Every language; Cordis took the Promise as the result |
| A Rust listener is called with the `ctx` that registered it, not the emitter's | Rust |
| Unloading waits for running listeners (at most `RUTIS_SHUTDOWN_TIMEOUT`) and signals them to cancel | Every language |
| A listener in an instance no longer receives other instances' events | Scopes |
| `rutis/*` names are reserved: listen only | Every language |
| Plugin API 3; mixing with runtimes and nodes of 0.12 and earlier is refused | Every end |
