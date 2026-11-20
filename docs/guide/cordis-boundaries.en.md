# Cordis compatibility boundaries

[中文](cordis-boundaries.md)

When a Cordis plugin runs in rutis without source changes, "compatible" means the framework contract in [Requirements](../requirements-protocol-plugins.en.md) §4: the synchronous / asynchronous shape of service methods, arguments and results, assembly and dependency gating, unload cleanups, object identity, and what the five event modes mean. Differences caused by the JavaScript stack, by process boundaries, and by a few capabilities that are not implemented are not hidden; the eight below are where they are. Each says what it is now, what it is not, and what to do.

How to mount a plugin and which Rust types are generated: [Cordis](cordis.en.md). The five modes and how values cross a boundary: [plugin API: events](plugin-api.en.md#events) and [passing values](plugin-api.en.md#passing-values). Guarantee numbers, status and tests for each item: [guarantee register](../guarantees.en.md) §8.

## 1. Five event modes, no `collect`

**What it is now**

- There are five modes: `emit`, `parallel`, `serial`, `waterfall`, `bail`. A Cordis plugin uses Cordis's own methods of those names; across processes they are the same modes on one listener table, and listeners elsewhere and in this process take part in one chain in registration order.
- `emit` is a notification: it returns `undefined`. When a Cordis plugin's `emit` returns, the listeners in this process have all been called (it waits synchronously for the host to dispatch, except behind an earlier `emit` of this process that has not started yet: the call arrives later, in order); listeners elsewhere are not waited for. A failing listener is only reported: the emitter and the other listeners are not affected.
- `parallel` waits for all of them and returns `undefined` on success too. Failures throw an `AggregateError` whose children are in registration order, with a single failure included.
- `serial` is asynchronous; `bail` is synchronous. They are separate methods and both return the first value that is a result: `null` / `undefined` / `false` is no result, anything else (including `0` and `""`) is. An asynchronous listener under `bail` is item 2.

**What it is not**

- `emit` / `parallel` do not collect listeners' return values. It makes no difference that every listener returns one: in the expectations file, the three listeners of `parallel/all-ok` return `"A"`, `null` and `"C"`, and the expected result of `parallel` is no result.
- There is no `collect`, and no method that gathers every listener's result into an array. Cordis 4.0.4's dispatch surface is `dispatch` and these five modes; rutis adds no sixth.
- `serial` / `bail` do not run every listener and then pick a result either: the first result stops the dispatch and the rest are not called.

**What to do**

- Need every listener's result: have the listeners write into a shared structure (`Map`, array), use `parallel` as a barrier, and read after it returns.
- Need one result: use `serial` (it can be awaited); in a synchronous context use `bail`, and make sure the chain has no asynchronous listener (item 2).
- Need only a notification: use `emit`; need failures: use `parallel` and handle the `AggregateError`.

Evidence: guarantees `events/emit`, `events/parallel`, `events/serial-bail`. `node/rutis-runtime/test/cordis-events.test.mjs` (`the five Cordis methods dispatch through the host, each in its mode`; `leaf plugins: …` asserts `await leaf.parallel('o')` is `undefined`); `crates/rutis-bridge/src/runtime/testing/events.expected.json` (`emit/reaches-all`, `parallel/all-ok`).

## 2. `bail` meeting an asynchronous listener: `AsyncListener`

**What it is now**

- A synchronous `bail` calls its listeners one at a time, synchronously. An `async` function in this process, or a plain function returning a Promise, makes the dispatch fail with `AsyncListener`; the Promise is discarded.
- A listener elsewhere already known to be asynchronous (an entry marked `async` in another runtime) fails it the same way. The message is `listener of <event> in <listener> is asynchronous and cannot run in a synchronous bail`, with `<listener>` naming the row, runtime, node or plugin.

**What it is not**

- It is not what old Cordis did, taking the returned Promise itself as the short-circuit value.
- It does not fall back to waiting: the listener is not awaited, and it is not skipped so that a later result can be found.

**What to do**

- Need to wait for listeners: use `serial` (asynchronous).
- Need to short-circuit synchronously: make the listeners synchronous functions (returning no Promise), and make sure no asynchronous listener is anywhere on the chain.
- Coming from Cordis, wherever a `ctx.bail` listener may be `async`, change it to `serial`, or make the listener synchronous.

Evidence: guarantees `events/serial-bail`, `events/hub-bail`; `crates/rutis/tests/events/events.rs::bail_meets_an_async_listener`; `crates/rutis-bridge/tests/event_hub.rs::bail_is_synchronous_everywhere`; `node/rutis-runtime/test/cordis-events.test.mjs` (`the five Cordis methods dispatch through the host, each in its mode`: `root.bail('b')` throws `AsyncListener`).

## 3. `intercept`: a non-empty merged configuration is not supported

**What it is now**

- A row's `inject` may be a list, or an object. In the object, each name accepts only a bare declaration: `null`, `true` or an empty object `{}`, which is the same as putting the name in the list.
- A non-empty value (for example `{ "dep": { "level": 1 } }`) makes the loader return `Unsupported`; the row is `unresolved` and does not run. `rutis-host status` shows `unresolved: unsupported: intercept config for "dep" in inject of "i"`.

**What it is not**

- It is not "ignore the intercept and keep running": the row does not start.
- It is not Cordis's merged per-service configuration carried into rutis. rutis's `ServiceIntercept` intercepts service reads and writes; it is a different thing from `intercept` in Cordis's configuration layer.
- It is not a row-level `intercept` field: rutis's rows have no such field, and it is ignored as unknown if written; the values in the `inject` object are what it reads.
- It is not specific to some runtimes: the loader is what refuses it, so rows of every language behave the same.

**What to do**

- The plugin needs that configuration: move it into the plugin's own `config` and merge it in `apply`, or drop the row.
- It is only a bare declaration: write `["dep"]` instead of `{ "dep": {} }`, or keep `{ "dep": true }`.
- Run `rutis-host check`, or look at `rutis-host status`, to see which row and which service.

Evidence: `crates/rutis-loader/src/loader/desired.rs`, `parse_scope` (`null` / `true` / `{}` are accepted, anything else returns `Unsupported`); `crates/rutis-loader/tests/scope.rs::unknown_services_and_intercept_config_are_unresolved`; guarantee `loader/isolate-and-inject`.

## 4. Direct `ctx.set`: no notification, refreshed after the next call

**What it is now**

- `ctx.provide` publishes and property assignment (`ctx.counter = x`) replaces with a notification: the first fires `internal/service`, the second goes through `internal/set`. The compatibility layer re-reads the slot and publishes the new object to the rutis side under a new handle and version.
- A direct `ctx.set('counter', x)` sends no notification. The Node runtime re-reads every slot after each call from rutis to a service method exported here, a throwing method included; for an asynchronous method it refreshes when its Promise settles. A changed object identity gets a new handle and version and the rutis side is told.

**What it is not**

- It is not visible to rutis at once: when the replacement happens inside a call, the refresh waits for the call to return (synchronous) or settle (asynchronous); on the asynchronous path the old proxy may briefly be read.
- A proxy already held does not follow the replacement: it keeps addressing the object it was made for until the caller lets go. "The next call hits the new object" holds only for a fresh read of the service (`ctx.get`, or `ctx.use` after the plugin is gated and started again); the next call on an old handle still reaches the old object.
- It is not how other Cordis plugins in the same process see it: they read Cordis's store and get the new object directly. This boundary concerns the rutis side only (the host, other processes, other languages).

**What to do**

- For another process, the host or another language to see the replacement: replace through property assignment, or withdraw and `provide` again; do not use `ctx.set`.
- If you use `ctx.set`: do it inside a method call, and after the call returns have the user read the service again (`ctx.get`, `ctx.use`), rather than reusing a proxy obtained before the replacement.
- Do not replace silently with `ctx.set` from a timer or an event listener and expect the next call to hit the new object.

Evidence: guarantee `boundary/5.1`; `crates/rutis-bridge/tests/service_projection.rs::projected_service_follows_replacement_and_withdrawal`, `crates/rutis-bridge/tests/projection_lifecycle.rs::a_throwing_method_still_refreshes_the_slot`; implementation in `node/rutis-runtime/src/runner.mjs` (`refresh`, `dispatchCall`).

## 5. Services the host provides: proxies, `instanceof` does not hold

**What it is now**

- A service rutis provides to a Cordis plugin is a proxy on the Cordis side, built from its declared methods: calls through the interface behave natively (arguments, results, asynchronous / synchronous); an unknown method fails with `no-method` and lists the methods there are.
- The proxy has only the declared methods; properties, getters and fields are not there. On the Node side an unknown property yields a function that fails with `no-method` when called, so `typeof ctx.log.anything === 'function'` is not a check that the member exists.

**What it is not**

- It is not an instance of the class the plugin declares: `ctx.log instanceof Log` is `false` (the same plugin in native Cordis gets `true`).
- It is not the host's object itself, and carries none of its prototype chain, constructor or private fields.
- It is not a snapshot of the host's object: every call crosses to the host and reads its state at that moment.

**What to do**

- Program against the interface: call the declared methods; do not test `instanceof`, `constructor` or prototypes.
- Need an instance of a class: wrap the proxy in a class of your own inside the plugin.
- Need to know whether the service is there: use `ctx.get('log')` or `inject` (dependency gating), not `instanceof` or `typeof`.
- When writing a `.d.ts` for the plugin, declare an `interface`, not a `class` you then expect `instanceof` to match.

Evidence: guarantee `boundary/5.7`; `crates/rutis-bridge/tests/boundary_rules.rs::a_host_service_is_a_proxy_not_an_instance_of_the_declared_class` (the same plugin gets `instanceof` `true` in native Cordis and `false` through the layer, with the same call results); [Requirements §5](../requirements-protocol-plugins.en.md) rule 7.

## 6. Synchronous waits: `SyncWaitCycle`

**What it is now**

- If a synchronous method (one that is not `async` in Cordis) waits, while running, for a result that needs the event loop (a timer, a Promise), the call fails with `SyncWaitCycle` instead of hanging. Node, Bun and Python find it themselves; a future awaited in the host does too.
- Before forwarding a synchronous call, the host checks the cross-runtime wait relation: if forwarding would make runtimes wait for each other (two runtimes calling each other synchronously, say), it does not forward and returns `SyncWaitCycle` to the caller; only that call fails.
- A synchronous `bail` waiting on a listener elsewhere is a synchronous call and takes part in the same wait-cycle check.

**What it is not**

- It is not a timeout: there is no "give up after this long"; a real cycle is reported at once.
- It is not a guard over every synchronous wait: a synchronous call a runtime does not mark on its frame is outside the check and may deadlock (explicitly not guaranteed); a call that says it is not waited for (`awaited: false`) and then waits anyway also leaks an edge. Every runtime in this repository marks its synchronous calls.
- It is not something a longer timeout gets around: a synchronous wait must not wait for the event loop in the first place.

**What to do**

- To wait for a Promise, a timer or a service elsewhere: make the method `async`. The interface shape changes, but that is the only correct fix.
- A real cycle: make one side asynchronous, or mark a fire-and-forget call `awaited: false` (session protocol SESS-83); do not wait on each other inside a synchronous chain.
- On seeing `SyncWaitCycle`: read the call chain in the error; the host's line also names the rows the call went through.

Evidence: guarantees `boundary/5.6`, `bridge/sync-wait-cycle-refused`; `crates/rutis-bridge/tests/rpc_callbacks.rs::node_sync_wait_pumps_callbacks_and_reports_its_executor_cycle` (the fixture `node/rutis-runtime/test/fixtures/rpc-client.mjs` asserts `SyncWaitCycle`), `::sync_wait_detects_an_async_callback_already_running_on_its_runtime`; `crates/rutis-loader/tests/sync_cycles.rs`; designs [multilanguage runtimes](../design-multilang-runtimes-2026-10-03.en.md) §9 and [function references](../design-function-refs-2026-11-03.en.md).

## 7. Unload order and what the runtime overrides

**What it is now**

- The `sdk/unload-order` guarantee is: a row unloads through a stack, run in reverse, one entry at a time; a `provide`'s withdrawal, `effect` and the cleanup `apply` returns are all in it. It covers cleanups registered through the plugin API: Node leaf plugins (`definePlugin`) and the other five runtimes.
- A native Cordis fiber's `_unload` is Cordis's own: it starts every disposal at once with `Promise.all`. That is Cordis 4.0.4, and the compatibility layer does not change it. The Node runtime keeps a stack of its own for leaf plugins only, drained in reverse by one Cordis effect, which gives the same order as elsewhere.
- The Node runtime does override Cordis's `events.dispatch` and the five methods (`emit`, `parallel`, `serial`, `waterfall`, `bail`), connecting them to the logical bus; `internal/*` still goes through Cordis's own table, and listeners register through `internal/listener`.

**What it is not**

- It is not "a Cordis plugin's `ctx.effect` cleanups also run in reverse, one at a time": they start together, completion order is not promised, and `sdk/unload-order` does not cover them.
- It is not "the runtime patches `_unload`": `_unload` is unchanged; what is overridden is `dispatch` and the five methods on the events object, and every Context gains a read-only `ctx.rutis` and `ctx.signal`, with listener registration hooked on `internal/listener`.
- It is not a takeover of every Cordis behavior: for names on the bus, `events.dispatch` still returns a callback (one), `internal/*` keeps Cordis's own, and the five methods keep Cordis's shape (a Cordis plugin's `emit` and `waterfall` wait for the host synchronously).

**What to do**

- A Cordis plugin must not rely on the order of two `ctx.effect` cleanups: put ordered steps in one cleanup and run them in the order you need; do not split them into two `ctx.effect`s and expect an order.
- Need "stopped means no more work": use the row's `ctx.signal`, not cleanup order.
- Use Context methods (`ctx.on`, `ctx.emit`, …); do not change `ctx.events` or its internals. When a plugin calls `events.dispatch` itself, the callback it gets dispatches through the bus.

Evidence: guarantee `sdk/unload-order`; design [unload order](../design-unload-order-2026-11-01.en.md) (§2 D7, §3 "the unload stack"); `node/rutis-runtime/src/leaf.mjs`, `toCordis` (its own stack); `node/rutis-runtime/src/cordis-events.mjs`, `install` (overrides `events.dispatch` and the five methods); Cordis 4.0.4 `lib/index.js` (`_unload` uses `Promise.all`); `node/rutis-runtime/test/cordis-events.test.mjs` (`events.dispatch gives one callback that dispatches through the host`).

## 8. Across processes: object identity, cancellation, timeouts, event scope

| | What it is now | What it is not | What to do |
| --- | --- | --- | --- |
| Object identity | Objects with methods cross by reference: the same object handed over several times is one proxy, and handed back to its own process it is the original object. A proxy has methods, not fields; a released reference can no longer be called; after the provider's process exits, a call fails with `peer-gone`. | It is not a copy or a snapshot, and not the same JavaScript object on both sides: `instanceof`, constructors and private fields do not hold. Plain data crosses by value and has no identity after the trip. | Hand the object itself over when identity matters, not a copy of its fields. An object cannot be an argument of a `#[rutis::service]` method (it fails with `bad-argument`). Hold and reuse the proxy; handle `peer-gone`. |
| Cancellation | Dropping a call's future in Rust cancels it; a Python coroutine, a Go context, a Swift Task and a Kotlin coroutine receive cancellation; a JavaScript method receives an abort only if the caller passed an `AbortSignal` (the Rust bindings pass one when calling Cordis methods). In a dispatch, an emitter that gives up cancels running listeners, except for `emit`; JavaScript cannot cancel a dispatch and has no per-invocation signal. | It does not guarantee the far end stopped: the method may have run to completion with its result discarded, and cancellation is not reported as "did not run". `emit` is never cancelled. | Make long methods `async` and take an `AbortSignal`, checking `signal.aborted` at checkpoints. Make side effects idempotent. After a cancellation or timeout, treat the result as unknown, neither failed nor succeeded. |
| Timeouts | rutis puts no time limit on a call; the caller sets one in its own language: Rust `tokio::time::timeout`, Python `asyncio.wait_for`, Go `context.WithTimeout`, a Swift `Task` cancellation, Kotlin `withTimeout`. A timeout cancels. Stopping the host has its own deadline: `rutis-host`'s `--shutdown-timeout`, 10 seconds by default; waiting for running listeners uses it too. | It is not the compatibility layer timing calls for you; and it is not the far end stopping at the deadline: the result is unknown and side effects may already have happened. The unload deadline is not a call timeout; it limits how long stopping the host waits for listeners and cleanups. | Put a timeout around every cross-process call. Use asynchronous methods; do not make a synchronous method wait for the event loop (item 6). Use `rutis-host status` for calls in progress; set `--shutdown-timeout` to what stopping the host needs. |
| Event scope | Events are delivered by name on one flat bus to every listener. Scope (`ctx.rutis.scope`, the `scope` in a listener's descriptor) is a description for filters, marked by the loader with the group instance. `isolate` isolates services, not events. `internal/*` stays in the process; only the host emits `rutis/*`. | It is not routing, isolation or a permission boundary: a listener in another instance or tenant still hears the same name unless it excludes it with a filter or a different event name. | To narrow the range: use different event names, or a filter (`ctx.filter`, `thisArg[Context.filter]`) on `c.rutis.scope` / the row. Do not use scope as a security boundary; `rutis/*` can only be listened to. |

Evidence: guarantees `bridge/object-refs`, `events/cancel`, `events/flat`, `events/hub-flat`, `boundary/5.8`; `crates/rutis-bridge/src/session/rpc/tests/objects.rs` (`an_object_given_twice_is_one_relay_and_comes_back_as_itself`, `a_call_on_an_object_given_up_is_cancelled_at_its_owner`); `node/rutis-runtime/test/session.test.mjs` (`an object from the far end is a proxy whose methods call it (#687)`); `crates/rutis-bridge/src/runtime/testing/events.expected.json` (`scope/flat-bus`, `cancel/emitter-gives-up`, which leaves out `node`, `node-leaf` and `bun`: JavaScript cannot cancel a dispatch); [plugin API: cancellation](plugin-api.en.md#cancellation).
