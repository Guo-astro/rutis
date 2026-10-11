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

## Passing values

A plugin and the services it uses may run in different processes. Across processes:

| Value | How it is passed |
| --- | --- |
| `null` / `None`, booleans, numbers, strings | Copied |
| Arrays / lists, plain objects / dictionaries with string keys | Recursively copied |
| Python dataclasses | Copied as dictionaries |
| `Error` / exceptions | Name and message are copied; the other side receives an error with the same name when thrown |
| Functions, Promises / coroutines | Passed by reference: the other side calls or awaits the original |
| Objects with methods, class instances | Passed by reference: the other side receives a proxy that calls the original object |
| Symbols, bigints, Maps, Sets, binary data, cyclic references | Cannot cross processes. In TypeScript, Maps, Sets, and Dates are copied using JSON rules, so their contents are lost. |
| Go structs, slices, `map[string]T` | Copied as JSON (by their `json` tags); a struct is always data, `rutis.Ref(v)` passes an object by reference. Integers beyond ±2^53−1, `[]byte`, channels and non-string map keys cannot cross |

Values are not copied within one process. Code that mutates an object after passing it may work in a unit test but fail across processes. The test tool's strict mode (enabled by default) applies the rules above to expose this early.

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

## Cancellation

When the caller abandons an async call (for example, on timeout), the call is cancelled: a Python coroutine is cancelled, a Go method's `context.Context` is cancelled, and a JavaScript method receives an abort when the caller supplied an `AbortSignal` (Rust bindings pass one when calling Cordis methods). Without either mechanism, the callee runs to completion and its result is discarded.

## Versions

Plugins are tagged with the plugin API version they use (`definePlugin` / `define_plugin` / `rutis.Define` adds the tag automatically). If the runtime is older than the plugin requires, loading fails with an explanation of what to upgrade. Depend on the SDK's major version range; it does not need to match the host version.
