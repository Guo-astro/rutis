# Migrating to 0.21

[中文](migration-0.21.md)

0.21 is not released yet; this guide is written alongside the incompatible changes, a section with each one, and finished before the release as [#694](https://github.com/arcships/rutis/issues/694) says. It covers what differs from 0.20, and nothing else; a form that goes is deleted rather than deprecated.

The incompatible changes, by kind, each with its section:

- **A received function is asynchronous by default**: outside a synchronous call, calling a function another process handed over is now asynchronous (a Promise in Node and Bun, an awaitable in Python), so the old code gains an `await` ([A received function is asynchronous by default](#a-received-function-is-asynchronous-by-default-932)).
- **Rust's `rutis::Error::Value` is a struct variant with a `reason`**: `Value(String)` → `Value { message, reason }`, so a host that matches on the variant, or builds it, changes ([Rust's value errors carry a reason](#rusts-value-errors-carry-a-reason-936)).
- **The error texts changed**: making errors readable is what this release is about, so almost every sentence rutis makes itself is new. Codes and the keys of `data` are stable; **code that matches on text has to match on codes** ([The error texts changed](#the-error-texts-changed-933-936-937)).

Everything else is unchanged or more permissive.

## Rust's value errors carry a reason (#936)

Design: [one shape for a value that cannot cross](design-value-errors-2026-11-03.en.md). Only Rust host code is affected: a plugin is written as before.

### What changed

`rutis::Error::Value` is a struct variant now, carrying a `reason`: the name of why the value cannot cross, which appears in the error's `data.reason` (one of `non-finite`, `cycle`, `too-deep`, `type`, `tag`, `limit`). Two constructors are new: `Error::value(message)` and `Error::why(reason, message)`.

| 0.20 | 0.21 |
| --- | --- |
| `Error::Value(String::from("…"))` | `Error::value("…")` (no reason), or `Error::why("too-deep", "…")` |
| `match error { Error::Value(text) => … }` | `match error { Error::Value { message, .. } => … }` |
| `error.data()` is `{part, argument?, limit?}` | a value that cannot cross adds `reason` |

### What to change

- **Building one**: `Error::Value(text)` → `Error::value(text)`; the text stays a `String` as it was. Where a reason is known, `Error::why("non-finite", text)`; the name is one of those six.
- **Matching one**: `Error::Value(text) =>` → `Error::Value { message, .. } =>`; with the reason, `Error::Value { message, reason } =>` (`reason` is `Option<&'static str>`, `None` when this side does not know it).
- **Reading `data`**: a program reads the new `data.reason` by name; code that does not read it is unaffected (one more key). The codes and frames across processes are unchanged: an old host reading an error from a new runtime looks only at the keys it knows.

```rust
// 0.20
let error = Error::Value(format!("{call}: …"));
match error {
    Error::Value(text) => eprintln!("{text}"),
    _ => {}
}

// 0.21
let error = Error::value(format!("{call}: …"));
let error = Error::why("too-deep", format!("{call}: …"));
match error {
    Error::Value { message, .. } => eprintln!("{message}"),
    _ => {}
}
```

### Who is affected

- Plugin authors: no change; a plugin is written as before and the plugin API (3) is unchanged.
- Host owners: Rust hosts that build or match `rutis::Error::Value` change (the two lines above); other languages have no such variant.
- People deploying: no change. The session protocol (4), the `rutis.json` format (1) and the manifest format (1) are unchanged.

## The error texts changed (#933 #936 #937)

Design: [error messages](design-error-messages-2026-11-03.en.md), [one shape for a value that cannot cross](design-value-errors-2026-11-03.en.md), [test tool errors](design-test-tool-errors-2026-11-03.en.md), and the codes and wording of the [diagnostics design](design-diagnostics-2026-10-26.en.md) §4.3 and §4.5.

Readable errors are what this release is about, so almost every sentence rutis makes itself is new: shorter, saying whose it is, and putting the *why* in `data` where a program reads it. **The codes and the keys of `data` are the contract; the texts are not** — code that matches on text has to match on codes.

### The ones you see most

| 0.20 | 0.21 |
| --- | --- |
| `waiting for lmm (no row provides it; did you mean llm?)` | `waiting for lmm (nothing provides it yet; did you mean llm?)` |
| `row b waits for weather from this row: they inject each other` | `row b waits for weather too: they inject each other` |
| `weather is already provided by store` | `weather is already provided by store: rename one of them`; a name the host holds itself reads `… is already provided by the host: rename it` |
| `weather has no method today (methods: ask)` | the same sentence; when this side knows who provides it, `; it is provided by row store` follows, and `data.provider` is `store` |
| `a number that is not finite (NaN) cannot cross` | `the number NaN is not finite`, with `data.reason` `non-finite` |
| `value nested deeper than 64 levels, the most a value crossing processes may nest (a cyclic reference?)` | `the value nests deeper than 64 levels` (`data.reason` `too-deep`), or `the value holds itself (a cycle)` (`cycle`) |
| `argument 1 should be a string, got 3` | `argument 1: <the decoder's own sentence>` (Rust, Go and Kotlin; Swift, Python and Node keep the old shape) |
| `await requires the Node thread occupied by its parent synchronous call; path: test:5 -> …` | `<the call the developer wrote> would wait on itself: it is awaited inside the synchronous call that produces it (synchronous calls)` |
| the state words `unloading`, `inactive` | `stopping`, `not running (its group is not running)` |
| `[remote object]`, `<remote object>` | `<object of another process>` |

A result that does not decode has its own code now, `bad-result`: the message is `<service>.<method>: the result: <the decoder's own sentence>` and `data.expected` is the declared type (`a string`, `a Forecast`), no longer one blanket phrase.

### What to change

Match on codes, not texts:

| Language | How to read them |
| --- | --- |
| TypeScript / Bun | `error.code`, `error.data` (an error a plugin throws carries them too) |
| Python | `error.code`, `error.data` |
| Go | `err.Code()`, `err.Data()`; `errors.Is(err, advanced.ErrBinding)` for "this is a refusal" |
| Swift | `error.code`, `error.data` |
| Kotlin | `error.code`, `error.data` |
| Rust | `error.code()`, `error.data()` |

The codes are `not-declared`, `unavailable`, `no-method`, `bad-argument`, `bad-value`, `bad-result` (new), `peer-gone`, `bad-config`. `SyncWaitCycle` and `AggregateError` carry no code: match them by type name (in Rust, the `Error::SyncWaitCycle` variant).

`data` gained two keys in this release: `reason` on `bad-value` (`non-finite`, `cycle`, `too-deep`, `type`, `tag`, `limit`) and `provider` on `no-method`; `expected` on `bad-argument` and `bad-result` is the real type now. Code that reads `data` by name is unaffected by the extra keys.

Code that only shows the error to a developer **changes nothing**: the sentences are shorter and more accurate, and the internal ids that should never have shown (`path: test:5 -> …`) are gone.

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
