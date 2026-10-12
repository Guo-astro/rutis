# Plugin API

TypeScript / JavaScript plugins (`@arcships/rutis`), Python plugins (`rutis`) and Go plugins (`github.com/arcships/rutis/go/rutis`, see [Go plugins](go-plugin.en.md)) use the same conventions. The current plugin API version is **1**.

## Declarations

| Declaration | TypeScript (`definePlugin({...})`) | Python (`define_plugin(apply, ...)` or module variable) | Go (`rutis.Define(rutis.Plugin[C]{...})`) | Meaning |
| --- | --- | --- | --- | --- |
| Services used | `inject: ['llm']` | `inject=["llm"]` | `Inject: []string{"llm"}` | The plugin starts when all are ready. If any is revoked, the plugin stops and waits for it to return. |
| Services provided | `provides: { weather: { today: 'async' } }` | `provides={"weather": Weather}` or `{"weather": {"today": "async"}}` | `Provides: rutis.Provides{"weather": rutis.MethodsOf[*Weather](rutis.Sync("Unit"))}` (exported methods, `async` unless `Sync` names them) | Declares the service name and each method kind (`sync` / `async`). Other plugins can call only declared methods. |
| Configuration | `config: { type: 'object', ... }` | `config={...}` (module variable: `Config`) | the type `C` (schema derived from it), or `Schema` | JSON Schema used by the host to validate and display configuration. |
| Startup | `apply(ctx, config)` | `apply(ctx, config)` | `Apply: func(ctx *rutis.Ctx, config C) error` | May be async; returns a cleanup function or nothing (Go: an error fails the load). |

## `ctx`

| Method | Purpose |
| --- | --- |
| `ctx.use(name)` | Gets a service declared in `inject`. A service provided by a plugin in the same runtime process is the object itself; services from other processes, languages, or machines are proxies. |
| `ctx.provide(name, value)` | Provides a service until the plugin is unloaded or the returned function is called. |
| `ctx.effect(cleanup)` | Runs `cleanup` when the plugin is unloaded. It may be async. |

Cleanups run in reverse registration order: the function returned by `apply` runs first, followed by functions registered with `ctx.effect`.

In Go: `ctx.Use(name, &target)` binds the service to a struct of function fields (or a `*rutis.Service`); `ctx.Provide(name, value)`; `ctx.Effect(func(context.Context) error)`; `ctx.Go(task)` runs a goroutine stopped at unload. `*rutis.Ctx` is a `context.Context` cancelled when the plugin unloads.

## Lifecycle

The host controls when a plugin runs:

- It starts (`apply`) when all injected services are ready.
- If any injected service is revoked, it stops and cleans up. It starts again when the service returns.
- A configuration change restarts it: cleanup runs, then `apply` runs with the new configuration.
- When a provided service is revoked, plugins that use it stop first, followed by its provider.

Services obtained in `apply` remain valid while the plugin is running, so the plugin does not need to handle a service disappearing midway through its work.

When the host stops, it unloads every plugin, each cleanup runs once, and then the runtime process exits. When the host goes away without unloading them (it was killed, or crashed), the runtime sees its connection to the host close, unloads every plugin itself, and exits; timers, sockets, threads or goroutines a plugin left behind do not keep it running.

Cleanups have a deadline: when the host stops, the `--shutdown-timeout` of `rutis-host` (10 seconds by default, see [rutis-host](rutis-host.en.md#stopping-and-exit-status)); when the host went away, the runtime uses the same time (`rutis-host` passes it to its runtimes in the `RUTIS_SHUTDOWN_TIMEOUT` environment variable; 10 seconds without it). A cleanup not finished by then, and those after it, are not waited for: the runtime writes a line saying so on stderr and exits with status 1. So work that must happen should not rely on cleanup alone, and a cleanup should not wait without end. In Node and Bun, a cleanup that blocks synchronously (an endless loop) holds the thread, and the deadline cannot interrupt it.

## Passing values

A plugin and the services it uses may run in different processes. Across processes:

| Value | How it is passed |
| --- | --- |
| `null` / `None`, booleans, numbers, strings | Copied |
| Arrays / lists, plain objects / dictionaries with string keys | Recursively copied |
| Python dataclasses | Copied as dictionaries |
| `Error` / exceptions | Passed as a value: copied as data `{ name, message }` (in TypeScript also `stack`). Thrown: the other side receives an error with the same name and message (in TypeScript a built-in error class keeps its class, any other is an `Error`; in Python a `rutis.peer.RemoteError` whose `name` is the original class name); other attributes are not passed |
| Functions, Promises / coroutines | Passed by reference: the other side calls or awaits the original |
| Objects with methods, class instances | Passed by reference: the other side receives a proxy that calls the original object |
| Symbols, bigints, Maps, Sets, TypeScript Dates, binary data, cyclic references, `NaN` / `Infinity` | Cannot cross processes (the call fails). Whether Maps, Sets, Dates, binary data and bigints should cross is decided in [#308](https://github.com/arcships/rutis/issues/308); until then both the real runtimes and the test tools refuse them |
| `undefined` inside TypeScript data (holding no functions or objects with methods) | Becomes `null` |
| Go structs, slices, `map[string]T` | Copied as JSON (by their `json` tags); a struct is always data, `rutis.Ref(v)` passes an object by reference. Integers beyond ±2^53−1, `[]byte`, channels and non-string map keys cannot cross |
| Python `int` | Copied. An integer beyond ±(2^53−1) (absolute value above 9007199254740991) cannot cross processes: the side sending it refuses it before sending, failing only that call, with an error naming the limit (`integer beyond ±(2^53-1)`), and the session goes on; whether it is an argument or a result, at any nesting depth. The same limit as JavaScript's safe integers and Go. Pass larger integers as strings. `bool` is not affected. See [#356](https://github.com/arcships/rutis/issues/356) |
| Values nested more than 64 levels deep | Cannot cross processes (the call fails, see below) |

Nesting depth: a value nests at most **64 levels**. Each array / list / tuple, object / dictionary (dataclasses included), and Go slice, map or struct is a level, whether or not it holds references; an `Error` / exception passed as a value is a level too. A call's argument list is not a level: each argument may nest 64 levels; a result may nest 64 levels. The side that **would send** a deeper value refuses it before sending, failing only that call, with an error naming the limit (`value nested deeper than 64 levels`):

- as an argument: the caller's call fails (in Rust with `Error::Value`) and the far end does not run it;
- as a result: the caller receives the error the callee throws;
- the session goes on, and other calls on it (those running at the same time included) are not affected.

All five runtimes (Rust, Node, Bun, Python, Go) and the test tool's strict mode check this limit when sending. The limit binds the sender: the receiving Rust side leaves a margin for a far end that does not keep to it (an older version, say) and reads frames up to 144 levels deep on the wire, so a value **received** from such a far end may nest deeper than 64 levels (up to about 70 levels of arrays holding references, about 142 of data); that margin is not guaranteed and plugins must not rely on it. When such a far end sends a frame nested too deep for the Rust side to read: if the frame shows which call it belongs to, only that call fails (the far end's call receives an error; a reply to this side's call fails that call), and the references in it are released only when the session ends; if it does not, the session ends, saying why. See [#279](https://github.com/arcships/rutis/issues/279).

Numbers have the limits below. They describe current behaviour and are **not guaranteed** to stay this way (explicitly not guaranteed); a plugin that depends on them must handle them itself:

- In JavaScript (Node, Bun) numbers are IEEE 754 doubles: an integer beyond ±(2^53−1) becomes the nearest double (2^53+1 arrives in JS as 2^53); `-0` becomes `0`; an integral float (Python's `2.0`) arrives in JS as `2` and leaves again as an integer. Pass large integers that must stay exact as strings.
- Every integer Python sends is within ±(2^53−1) (larger ones cannot cross, see the table above); Python keeps `-0.0`.
- NaN and ±Infinity cannot cross.

Values are not copied within one process. Code that mutates an object after passing it may work in a unit test but fail across processes. The test tool's strict mode (enabled by default) applies the rules above to expose this early.

### Where the test tool differs from a real runtime

The test tools (`@arcships/rutis/testing`, `rutis.testing`) and the real runtimes are checked against the same expectations file: the same calls on the same plugin (sync, async, callbacks, errors, the values in the table above) must give the same results ([`weather.expected.json`](../../crates/rutis-bridge/src/runtime/testing/weather.expected.json)). In strict mode, these are the only known differences:

| Difference | Why |
| --- | --- |
| A plugin that ends its process (`process.exit`, `os._exit`) ends the test | The test tool runs the plugin in the test's process; the expectations file marks this entry `allowed`, and the test tool does not make that call |
| No restarts: a config change or a withdrawn injected service does not stop and start the plugin | `load` starts it once; the host decides the lifecycle (see above) |
| Sync calls do not block, so there is no `SyncWaitCycle`; no cancellation (`AbortSignal`, coroutine cancellation) | The plugin and the test share one event loop |
| Python: an exception that crosses is a `RemoteError` whose `__cause__` is the original exception | For debugging; a real runtime's `RemoteError` has no `__cause__` |
| TypeScript: only plugins written with `definePlugin` load, not Cordis plugins | The test tool does not include Cordis |

Maps, Sets, Dates, binary data and bigints are refused on both sides, so they are not a difference; whether they will cross later is [#308](https://github.com/arcships/rutis/issues/308). With strict mode off (`strict: false`), values skip the table above and other differences can appear. Go's `rutistest` and Bun are not yet checked against this file.

## Synchronous calls and reentrancy

A method declared `sync` returns synchronously. Across processes, the caller blocks while waiting. During that wait:

- If the callee calls back into the caller, the callback can reach and run in the waiting caller.
- The Python runtime also handles other incoming calls while waiting synchronously. A Python service may therefore be called while it is already handling another synchronous call. **Do not hold locks while calling another service.**
- The Go runtime runs every incoming call on a goroutine of its own, so it is always reentrant: provided Go objects must be safe for concurrent use. The call chain travels in `context.Context`: **pass on the ctx you were given** when calling other services, or a synchronous caller waiting on you may not recognize your callback.
- The Node runtime handles only calls that belong to the current call chain while waiting synchronously; it runs the others once its stack is empty.
- In the Python, Node (and Bun) runtimes, a call run during a synchronous wait is stacked above the waiting one: the call that started first waits for the one that started later.

When runtimes call each other synchronously, every participant may end up waiting for another: two Node runtimes calling each other, or one Node and two Python runtimes in a nested pattern. Before forwarding each synchronous call, the host checks whether it would close such a wait cycle; if it would, the call is not forwarded, its caller gets a `SyncWaitCycle` error, and every other call completes. Do not retry the same call at once (it will most likely close the same cycle again): use an `async` method, try at another moment, or treat it as a failure.

The check covers runtimes that declare the `sync-wait` capability (the Node, Python and Bun runtimes of this repository; the protocol is in §9 of the [multi-language design](../design-multilang-runtimes-2026-10-03.en.md)). Synchronous calls from a runtime that does not declare it (an older SDK, the Go runtime) are outside the check and may deadlock as before; a runtime that does not declare `reentrant-sync` is taken as non-reentrant. For calls across runtimes that may cross in both directions, `async` methods are still the first choice.

Do not wait inside a synchronous method for a result that requires the event loop to advance, such as a Promise. This produces `SyncWaitCycle`; make the method `async` instead.

An `async` method runs up to its first `await` before the reply, like a JS async function, so an `async` method that never suspends can be awaited inside a synchronous call chain too. This holds on every supported Python (asyncio's own eager start from 3.12, rutis's on 3.10 and 3.11).

## Cancellation

When the caller abandons an async call (for example, on timeout), the call is cancelled: a Python coroutine is cancelled, a Go method's `context.Context` is cancelled, and a JavaScript method receives an abort when the caller supplied an `AbortSignal` (Rust bindings pass one when calling Cordis methods). Without either mechanism, the callee runs to completion and its result is discarded.

## Versions

Plugins are tagged with the plugin API version they use (`definePlugin` / `define_plugin` / `rutis.Define` adds the tag automatically). If the runtime is older than the plugin requires, loading fails with an explanation of what to upgrade. Depend on the SDK's major version range; it does not need to match the host version.
