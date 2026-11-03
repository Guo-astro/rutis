# Migrating to 0.21

[中文](migration-0.21.md)

0.21 is not released yet; this guide is written alongside the incompatible changes, a section with each one, and finished before the release as [#694](https://github.com/arcships/rutis/issues/694) says. It covers what differs from 0.20, and nothing else; a form that goes is deleted rather than deprecated.

The incompatible changes, by kind, each with its section:

- **A received function is asynchronous by default**: outside a synchronous call, calling a function another process handed over is now asynchronous (a Promise in Node and Bun, an awaitable in Python), so the old code gains an `await` ([A received function is asynchronous by default](#a-received-function-is-asynchronous-by-default-932)).

Everything else is unchanged or more permissive.

## A received function is asynchronous by default (#932)

Design: [how a function passed by reference is called](design-function-refs-2026-11-03.en.md). Node, Bun and Python are alike; the Go, Swift, Kotlin and Rust forms do not change.

### What changed

A function passed by reference (a callback another plugin hands over, or a host's) is now called as a service method is: asynchronous outside a synchronous call, and the result itself inside one (a `sync` method, a synchronous `waterfall`, a listener called synchronously).

| Case | 0.20 | 0.21 |
| --- | --- | --- |
| Calling a received function in `apply`, an `async` method, a listener or a timer | waits for the result and returns it; a fake written `async` meets `SyncWaitCycle` | gives a Promise (Node, Bun) or an awaitable (Python); `await` it for the result |
| Calling a received function in a `sync` method or a synchronous `waterfall` | returns the result | unchanged: nothing else could run there, so it returns the result |
| A test calling a function the plugin handed it (TypeScript, Bun) | waits for the result | asynchronous: `await` it. A fake may be `async` |
| A test calling a function the plugin handed it (Python) | the test gets the plugin's own function | unchanged: `await` an `async def`, use a plain `def`'s value |

### What to change

- **TypeScript, Bun**: `const text = f()` → `const text = await f()`. The changed code also runs on a 0.20 Node or Bun (`await` of a value that is not a Promise gives the value).
- **Python**: `text = f()` → `text = await f()` (the same as `f.call_async(...)`; a promise the function itself returns is settled too, as JavaScript's `await` does, so it is not awaited twice); inside a `sync` method keep `f()`, which returns the result there. On a 0.20 runtime awaiting a plain value fails (`TypeError: object str can't be used in 'await' expression`), so one Python plugin should not run on both.
- **A test's fakes**: what had to return directly may now do either. A test whose fake is `async` and which awaits the plugin's function passes on 0.21 and fails on 0.20 — write it for 0.21 and do not mix.

### Who is affected

- Plugin authors: only where they call a function another process handed over (most plugins do not); calling service methods and emitting or hearing events are unchanged.
- Host owners: calling a function a plugin handed over is unchanged (Rust's `call` / `call_async`, each language's own form in Go, Swift and Kotlin). When a host hands a callback to a Node, Bun or Python plugin, that plugin's code awaits it.
- People deploying: no change.
- The wire format, the session protocol (4), the `rutis.json` format (1) and the plugin API (3) are unchanged; what `rutis-host types` writes needs no regeneration (a callback's return type already writes `string | Promise<string>`, and `await` type-checks in both contexts).
