# Plugin API

TypeScript / JavaScript plugins (`@arcships/rutis`), Python plugins (`rutis`) and Go plugins (`github.com/arcships/rutis/go/rutis`, see [Go plugins](go-plugin.en.md)) use the same conventions. The current plugin API version is **2**.

## Declarations

| Declaration | TypeScript (`definePlugin({...})`) | Python (`define_plugin(apply, ...)` or module variable) | Go (`rutis.Define(rutis.Plugin[C]{...})`) | Meaning |
| --- | --- | --- | --- | --- |
| Services used | `inject: ['llm']` | `inject=["llm"]` | `Inject: []string{"llm"}` | The plugin starts when all are ready. If any is revoked, the plugin stops and waits for it to return. |
| Services provided (optional) | `provides: { weather: { today: 'sync' } }` | `provides={"weather": Weather}` or `{"weather": {"today": "sync"}}` | `Provides: rutis.Provides{"weather": rutis.MethodsOf[*Weather](rutis.Sync("Unit"))}` (exported methods, `async` unless `Sync` names them) | Not needed: what `ctx.provide` provides reaches rutis by itself (see below). When present it applies: it marks methods `sync` and lets `rutis-host check` know before running who provides what; a declared service offers only its declared methods. |
| Configuration | `config: { type: 'object', ... }` | `config={...}` (module variable: `Config`) | the type `C` (schema derived from it), or `Schema` | JSON Schema used by the host to validate and display configuration. |
| Startup | `apply(ctx, config)` | `apply(ctx, config)` | `Apply: func(ctx *rutis.Ctx, config C) error` | May be async; returns a cleanup function or nothing (Go: an error fails the load). |

## `ctx`

| Method | Purpose |
| --- | --- |
| `ctx.use(name)` | Gets a service declared in `inject`. A service provided by a plugin in the same runtime process is the object itself; services from other processes, languages, or machines are proxies. |
| `ctx.provide(name, value)` | Provides a service until the plugin is unloaded or the returned function is called. It reaches rutis by itself, used by name from other languages and processes; when `provides` does not declare it, its methods are the public methods of `value` (JavaScript: its own and its prototype chain's functions, not `constructor`; Python: callable attributes not starting with `_`; Go: exported methods), all called as `async`; fields are not methods. The inferred methods follow each language's rules, so the same service written in two languages can differ: a JavaScript class extending `EventEmitter` also exposes `on`, `emit` and the rest; list the methods in `provides` to expose only those. |
| `ctx.effect(cleanup)` | Runs `cleanup` when the plugin is unloaded. It may be async. |
| `ctx.on(name, listener)` | Listens on the runtime process's shared event bus: events the host emits into it (names declared with the runtime's `emits`, see [Embedding in a Rust application](rust-host.en.md)) and events plugins in the process emit with `ctx.emit`. The listener is unregistered when the plugin unloads; the returned function unregisters it at once. |
| `ctx.emit(name, ...args)` | Dispatches on this runtime process's event bus to listeners in this plugin and other plugins in the same process; calls listeners synchronously without awaiting returned promises/coroutines (for Go and listener errors, see below). |

Node, Bun, Python and Go support leaf events (plugin API 2) and report the `events` runtime feature. Node and Bun use Cordis's own event service (each Bun row holds a Cordis fiber), so they share one bus with Cordis plugins in the same process and dispatch exactly as Cordis does; Python and Go implement the same semantics (against `cordis/src/events.ts` and `crates/rutis/src/bus.rs`). Go exposes `ctx.On(name, func(context.Context, ...any) error) func()` and `ctx.Emit(callerCtx, name, args...) error`; listeners receive a context preserving the current call chain and cancelled when their plugin unloads. Unload waits for running listeners to finish before cleanups. Pass the received caller context when emitting from a plugin method; in `Apply`, the plugin's `ctx` itself can be used.

Local emission calls listeners in registration order. A synchronous exception in Node, Bun or Python stops later listeners; returned promises/coroutines are not awaited, and unhandled asynchronous failures end the runtime process. Go's `ctx.Emit` executes synchronously and returns the first error; goroutines started by the plugin follow the existing `ctx.Go` contract. The host's `Process::emit` instead waits for every listener and aggregates failures as `AggregateError`. Go starts a goroutine for each host-event listener, so their execution order is not guaranteed. As with Rust's `Ctx::dispose_self`, a listener must not wait for its own row to unload (a Go listener that synchronously unloads or updates its own row through a host callback would wait on itself): let the listener return first.

An error that crossed a session and is thrown on crosses again with its whole error graph (causes, an `AggregateError`'s children) on Node, Bun, Python and Go, not only inside an aggregate: Node decodes it into a real `Error` whose re-encoding carries the whole graph, and Python's `encode_error` and Go's `encodeError` splice a `RemoteError`'s graph in (nodes appended, references shifted) to match.

`internal/*` belongs to the runtime and cannot be listened to, emitted or listed in `emits`. When `RuntimePlugin` / `LocalRuntime` has `emits` configured, a runtime lacking the `events` feature fails at mount (`lacks events`). This feature check does not cover direct `Process::mount` / `attach` for static Cordis mounts: older Node runtimes already deliver Cordis events without reporting the feature.

`emits` only declares host → runtime events. A leaf plugin's `ctx.emit` / `ctx.Emit` dispatches within the process; `RuntimePlugin` exposes no `Mount.events` / `EventSink` configuration, so runtime → Rust host forwarding is not available here.

The Node and Python test tools use the runtime's own event implementation: Node's goes through Cordis, as the runtime does, and Python's uses `rutis.events`. Test tools declare host events at load: Node's `load(plugin, { emits })`, Python's `load(plugin, emits=[...])`, and Go's `rutistest.Load(t, plugin, config, services, "event", ...)`. Their host-emission entry points are `await t.emit(...)`, `await t.emit(...)`, and `loaded.Emit(callerCtx, name, args...)`; all check the declaration and wait for listeners. The Python test tool cancels and settles unfinished local asynchronous listeners at unload. It preserves an existing test-body or cleanup exception; otherwise it reports listener failures. A real runtime exits on an unhandled asynchronous failure.

Cleanups run in reverse registration order: the function returned by `apply` runs first, followed by functions registered with `ctx.effect`.

In Go: `ctx.Use(name, &target)` binds the service to a struct of function fields (or a `*rutis.Service`); `ctx.Provide(name, value)`; `ctx.Effect(func(context.Context) error)`; `ctx.Go(task)` runs a goroutine stopped at unload. `*rutis.Ctx` is a `context.Context` cancelled when the plugin unloads.

## Lifecycle

The host controls when a plugin runs:

- It starts (`apply`) when all injected services are ready.
- If any injected service is revoked, it stops and cleans up. It starts again when the service returns.
- A configuration change restarts it: cleanup runs, then `apply` runs with the new configuration.
- When a provided service is revoked, plugins that use it stop first, followed by its provider.

Services obtained in `apply` remain valid while the plugin is running, so the plugin does not need to handle a service disappearing midway through its work.

A service name has one provider in rutis: when another plugin (any language, any node) already provides a service of that name, the plugin providing it later fails (to load, or, when it provides it after loading, at that point), and the error names both, for example `service weather is already provided by row forecast: row weather cannot provide it too`. An undeclared service appears only once its plugin runs; plugins depending on it wait for it as usual, and `rutis-host check` reports such a dependency with a warning, `nothing declares <service>; it may be provided at runtime by …`, not as a failure (with `check --strict`, as one: see [Checking a configuration](rutis-host.en.md#checking-a-configuration)). The same for one provided from a thread or goroutine before `apply` returns: the host has handled it before the plugin has loaded, and a clash fails the load.

When a 0.12 runtime is mounted by a host older than 0.12, the host does not take undeclared services, which stay visible in their process only; a plugin providing one is told so on stderr, once per name: `rutis: service <name> is not declared in provides, and this host (older than 0.12) does not take undeclared services; declare it or upgrade rutis-host`. Declare it in `provides`, or upgrade rutis-host.

When the host stops, it unloads every plugin, each cleanup runs once, and then the runtime process exits. When the host goes away without unloading them (it was killed, or crashed), the runtime sees its connection to the host close, unloads every plugin itself, and exits; timers, sockets, threads or goroutines a plugin left behind do not keep it running.

Cleanups have a deadline: when the host stops, the `--shutdown-timeout` of `rutis-host` (10 seconds by default, see [rutis-host](rutis-host.en.md#stopping-and-exit-status)); when the host went away, the runtime uses the same time (`rutis-host` passes it to its runtimes in the `RUTIS_SHUTDOWN_TIMEOUT` environment variable; 10 seconds without it). A cleanup not finished by then, and those after it, are not waited for: the runtime writes a line saying so on stderr and exits with status 1. So work that must happen should not rely on cleanup alone, and a cleanup should not wait without end. In Node and Bun, a cleanup that blocks synchronously (an endless loop) holds the thread, and the deadline cannot interrupt it.

## Passing values

A plugin and the services it uses may run in different processes. Across processes:

| Value | How it is passed |
| --- | --- |
| `null` / `None`, booleans, numbers, strings | Copied |
| Arrays / lists, plain objects / dictionaries with string keys | Recursively copied |
| Python dataclasses | Copied as dictionaries |
| Dates and times, binary data, maps, sets, big integers, decimals, UUIDs | Copied; each language receives its own types, see [the ten values](#the-ten-values) below |
| `Error` / exceptions | Passed as a value: copied as data `{ name, message }` (in TypeScript also `stack`). Thrown: the other side receives an error with the same name and message (in TypeScript a built-in error class keeps its class, any other is an `Error`; in Python a `rutis.peer.RemoteError` whose `name` is the original class name); other attributes are not passed |
| Functions, Promises / coroutines | Passed by reference: the other side calls or awaits the original |
| Objects with methods, class instances | Passed by reference: the other side receives a proxy that calls the original object |
| `undefined` inside data | Becomes `null` (`None` in Python) |
| Go structs, slices, maps | Copied: struct fields and the items of slices and maps each follow the rules of this section, with field names by their `json` tags; a struct is always data, `rutis.Ref(v)` passes an object by reference. A `map[string]T` is an object, a map whose keys are not strings is a map, and a `map[K]struct{}` is a set |
| Integers beyond ±(2^53−1) | Cross as big integers automatically, see [the ten values](#the-ten-values) |
| Symbols, cyclic references, `NaN` / `Infinity`, other JS typed arrays (such as `Int16Array`), Python `time` with a time zone, Go channels | Cannot cross processes (the call fails) |
| Values nested more than 64 levels deep | Cannot cross processes (the call fails, see below) |

Nesting depth: a value nests at most **64 levels**. Each array / list / tuple, object / dictionary (dataclasses included), map, set, and Go slice, map or struct is a level, whether or not it holds references; an `Error` / exception passed as a value is a level too. A call's argument list is not a level: each argument may nest 64 levels; a result may nest 64 levels. The side that **would send** a deeper value refuses it before sending, failing only that call, with an error naming the limit (`value nested deeper than 64 levels`):

- as an argument: the caller's call fails (in Rust with `Error::Value`) and the far end does not run it;
- as a result: the caller receives the error the callee throws;
- the session goes on, and other calls on it (those running at the same time included) are not affected.

All five runtimes (Rust, Node, Bun, Python, Go) and the test tool's strict mode check this limit when sending. The limit binds the sender: the receiving Rust side leaves a margin for a far end that does not keep to it (an older version, say) and reads frames up to 208 levels deep on the wire, so a value **received** from such a far end may nest deeper than 64 levels (up to about 100 levels of arrays holding references, about 206 of data); that margin is not guaranteed and plugins must not rely on it. When such a far end sends a frame nested too deep for the Rust side to read: if the frame shows which call it belongs to, only that call fails (the far end's call receives an error; a reply to this side's call fails that call), and the references in it are released only when the session ends; if it does not, the session ends, saying why. See [#279](https://github.com/arcships/rutis/issues/279).

Numbers have the limits below. They describe current behaviour and are **not guaranteed** to stay this way (explicitly not guaranteed); a plugin that depends on them must handle them itself:

- In JavaScript (Node, Bun) numbers are IEEE 754 doubles: a JS `number` beyond ±(2^53−1) is already inexact and is sent as it is; use `bigint` to be exact. `-0` becomes `0`; an integral float (Python's `2.0`) arrives in JS as `2` and leaves again as an integer.
- Python keeps `-0.0`.
- NaN and ±Infinity cannot cross.

Values are not copied within one process. Code that mutates an object after passing it may work in a unit test but fail across processes. The test tool's strict mode (enabled by default) applies the rules above to expose this early.

### The ten values

| Value | JS (Node, Bun) | Python | Go | Rust (`rutis-bridge`) |
| --- | --- | --- | --- | --- |
| Point in time | `Date` | `datetime` with a time zone (received in UTC) | `time.Time` (received in UTC) | `Instant` |
| Local date and time | String `"2026-10-15T14:30:05"` | `datetime` without a time zone | `rutis.DateTime` | `DateTime` |
| Date | String `"2026-10-15"` | `date` | `rutis.Date` | `Date` |
| Time of day | String `"14:30:05"` | `time` without a time zone | `rutis.TimeOfDay` | `TimeOfDay` |
| Binary data | `Uint8Array` (`Buffer`, `ArrayBuffer`, `DataView` can be sent too) | `bytes` (`bytearray`, `memoryview` can be sent too) | `[]byte` | `Vec<u8>` (`Bytes` in generated code) |
| Map | `Map` | `dict` with a key that is not a string (received as a subclass of `dict`) | A map whose keys are not strings (`map[any]any` decoded into `any`) | `Vec<(Value, Value)>` (`ValueMap` in generated code) |
| Set | `Set` | `set` (`frozenset` can be sent too) | `map[K]struct{}`, or a slice (a slice decoded into `any`) | `Vec<Value>` (`ValueSet` in generated code) |
| Big integer | `bigint` | `int` (received as a subclass of `int`) | `*big.Int`, or an integer type that holds it | `BigInt`, or an integer type that holds it |
| Decimal | String `"12.30"` | `Decimal` | `rutis.Decimal` | `Decimal` |
| UUID | String (lowercase) | `UUID` | `rutis.UUID` | `Uuid` |

- Plugins write their language's native code; rutis does the conversion. A received big integer or map is, in Python, a subclass of `int` or `dict`, used as usual; passed on as it is, it is still a big integer or map.
- JS has no type for local date-times, dates, times of day, decimals or UUIDs: they arrive as plain strings, their content complete, and `===` works as usual. Passed on from JS as they are, such values lose their type (a `date` sent by Python comes back through JS as a `str`). To create one of these five values from nothing in JS, or to pass one on as its type, use the constructors of `@arcships/rutis`: `rutis.datetime('2026-10-15T14:30')`, `rutis.date('2026-10-15')`, `rutis.time('14:30')`, `rutis.decimal('1.10')`, `rutis.uuid('…')`; a plain string is still a string. Go can also decode these five values into a `string`.
- Integers: an integer beyond ±(2^53−1) (absolute value above 9007199254740991; Python `int`, Go `int64` / `uint64`, Rust integers) crosses as a big integer automatically, with nothing special to write; JS receives it as a `bigint`, and within range still as a `number`. A received big integer decodes into a declared integer type that holds it; one that does not hold it fails the call. `bool` is not affected.
- Times: years 0001–9999. A local date and time has no time zone and crosses as it is, with no zone guessed and no conversion to a point in time. A point in time is truncated toward negative infinity to the receiver's precision: milliseconds in JS, microseconds in Python, nanoseconds in Go and Rust; Python also truncates local date-times and times of day to the microsecond.
- A decimal keeps its trailing zeros (`1.10` is not `1.1`) and uses no exponent; NaN and infinities cannot cross. Big integers and decimals have at most 4300 digits.
- Map keys and set elements can only be strings, finite numbers, booleans, big integers, local date-times, dates, times of day, decimals and UUIDs; points in time, binary data, `null`, arrays, objects, maps, sets and references cannot, and the sender refuses them before sending. When two keys become equal after conversion on the receiving side (a JS `Map` with keys `1` and `true` arriving in Python, say), the call fails: an entry would be lost. A JS `Map` with string keys still crosses as a map; a plain Python `dict` with string keys crosses as an object.
- Each value is sent only to a far end that declared it. When the far end does not take it (a runtime older than 0.11, say, integers beyond ±(2^53−1) included), only that call fails, with an error naming the value, and the session and other calls go on. Events forwarded between nodes do not carry these ten values; leaf events within one runtime process carry them as usual.

### Where the test tool differs from a real runtime

The test tools (`@arcships/rutis/testing`, `rutis.testing`, Go's `rutistest`) and the real runtimes are checked against the same expectations file: the same calls on the same plugin (sync, async, callbacks, errors, the values in the table above) must give the same results ([`weather.expected.json`](../../crates/rutis-bridge/src/runtime/testing/weather.expected.json)). In strict mode, these are the only known differences:

| Difference | Why |
| --- | --- |
| A plugin that ends its process (`process.exit`, `os._exit`) ends the test | The test tool runs the plugin in the test's process; the expectations file marks this entry `allowed`, and the test tool does not make that call |
| No restarts: a config change or a withdrawn injected service does not stop and start the plugin | `load` starts it once; the host decides the lifecycle (see above) |
| Sync calls do not block, so there is no `SyncWaitCycle`; no cancellation (`AbortSignal`, coroutine cancellation) | The plugin and the test share one event loop |
| Python: an exception that crosses is a `RemoteError` whose `__cause__` is the original exception | For debugging; a real runtime's `RemoteError` has no `__cause__` |
| TypeScript: only plugins written with `definePlugin` load, not Cordis plugins (events go through Cordis's event service; the plugin model is still not Cordis's) | The tool mounts one plugin, with no services or dependency gating |
| TypeScript / Python events: the plugin under test hears only what it emits with `ctx.emit` and what `t.emit` sends; on a real runtime the whole process shares one event bus, so it also hears other plugins in the process (and, on Node, Cordis plugins) | The test tool loads one plugin. `t.emit` accepts only names declared in `emits` at load, as `RuntimePlugin::emits`; both refuse `internal/*` |
| The Python tool cancels and settles local asynchronous listeners at unload, preserving test-body or cleanup exceptions; the real runtime exits on an unhandled asynchronous failure | Keeps the test process running while failing the test explicitly |

The ten values cross by the same rules on both sides, so they are not a difference. With strict mode off (`strict: false`), values skip the table above and other differences can appear.

## Synchronous calls and reentrancy

A method declared `sync` returns synchronously. Across processes, the caller blocks while waiting. During that wait:

- If the callee calls back into the caller, the callback can reach and run in the waiting caller.
- The Python runtime also handles other incoming calls while waiting synchronously. A Python service may therefore be called while it is already handling another synchronous call. **Do not hold locks while calling another service.**
- The Go runtime runs every incoming call on a goroutine of its own, so it is always reentrant: provided Go objects must be safe for concurrent use. The call chain travels in `context.Context`: **pass on the ctx you were given** when calling other services, or a synchronous caller waiting on you may not recognize your callback.
- The Node runtime handles only calls that belong to the current call chain while waiting synchronously; it runs the others once its stack is empty.
- In the Python, Node (and Bun) runtimes, a call run during a synchronous wait is stacked above the waiting one: the call that started first waits for the one that started later.

When runtimes call each other synchronously, every participant may end up waiting for another: two Node runtimes calling each other, or one Node and two Python runtimes in a nested pattern. Before forwarding each synchronous call, the host checks whether it would close such a wait cycle; if it would, the call is not forwarded, its caller gets a `SyncWaitCycle` error, and every other call completes. Do not retry the same call at once (it will most likely close the same cycle again): use an `async` method, try at another moment, or treat it as a failure.

The check covers runtimes that declare the `sync-wait` capability (the Node, Python, Bun and Go runtimes of this repository; the protocol is in §9 of the [multi-language design](../design-multilang-runtimes-2026-10-03.en.md)). Synchronous calls from a runtime that does not declare it (an older SDK) are outside the check and may deadlock as before; a runtime that does not declare `reentrant-sync` is taken as non-reentrant. For calls across runtimes that may cross in both directions, `async` methods are still the first choice.

Do not wait inside a synchronous method for a result that requires the event loop to advance, such as a Promise. This produces `SyncWaitCycle`; make the method `async` instead.

An `async` method runs up to its first `await` before the reply, like a JS async function, so an `async` method that never suspends can be awaited inside a synchronous call chain too. This holds on every supported Python (asyncio's own eager start from 3.12, rutis's on 3.10 and 3.11).

## Cancellation

When the caller abandons an async call (for example, on timeout), the call is cancelled: a Python coroutine is cancelled, a Go method's `context.Context` is cancelled, and a JavaScript method receives an abort when the caller supplied an `AbortSignal` (Rust bindings pass one when calling Cordis methods). Without either mechanism, the callee runs to completion and its result is discarded.

## Versions

Plugins are tagged with the plugin API version they use (`definePlugin` / `define_plugin` / `rutis.Define` adds the tag automatically). If the runtime is older than the plugin requires, loading fails with an explanation of what to upgrade. Depend on the SDK's major version range; it does not need to match the host version.

Plugin API 2 adds leaf events. New runtimes still accept API 1. Plugins tagged API 2 by the new SDK need an updated runtime; Go binaries also need a host supporting API 2. Wire protocol and Go manifest formats are unchanged.
