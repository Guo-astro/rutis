# Plugin API

TypeScript / JavaScript plugins (`@arcships/rutis`), Python plugins (`rutis`) and Go plugins (`github.com/arcships/rutis/go/rutis`, see [Go plugins](go-plugin.en.md)) use the same conventions; so do Swift plugins (the Swift package `Rutis` of this repository, see [Swift plugins](swift-plugin.en.md)), and Kotlin plugins (`io.github.arcships:rutis`, K-P1: the desktop jvm target and the testing tool, applications connecting in K-P2, see [kotlin/README](../../kotlin/README.md)). The current plugin API version is **3**.

## Declarations

| Declaration | TypeScript (`definePlugin({...})`) | Python (`define_plugin(apply, ...)` or module variable) | Go (`rutis.Define(rutis.Plugin[C]{...})`) | Swift (`Rutis.define(Plugin(...))`) | Kotlin（`Rutis.define<C>(...) { ctx, config -> }`） | Meaning |
| --- | --- | --- | --- | --- | --- | --- |
| Services used | `inject: ['llm']` | `inject=["llm"]` | `Inject: []string{"llm"}` | `inject: ["llm"]` | `inject = listOf("llm")` | The plugin starts when all are ready. If any is revoked, the plugin stops and waits for it to return. |
| Services provided (optional) | `provides: { weather: { today: 'sync' } }` | `provides={"weather": Weather}` or `{"weather": {"today": "sync"}}` | `Provides: rutis.Provides{"weather": rutis.MethodsOf[*Weather](rutis.Sync("Unit"))}` (exported methods, `async` unless `Sync` names them) | `provides: ["weather": .of(Weather.self, sync: ["unit"])]` (the methods of a `@RutisService` type, `async` unless `sync` names them) | `provides = mapOf("weather" to service<Weather>(sync = setOf("unit")))` (the public member functions of a `@RutisService` class, `async` unless `sync` names them) | Not needed: what `ctx.provide` provides reaches rutis by itself (see below). When present it applies: it marks methods `sync` and lets `rutis-host check` know before running who provides what; a declared service offers only its declared methods. |
| Configuration | `config: { type: 'object', ... }` | `config={...}` (module variable: `Config`) | the type `C` (schema derived from it), or `Schema` | the `Decodable` type of `config:` (schema from `@RutisConfig`, or `static let rutisSchema`) | the `@Serializable` type `C` (schema derived from its `SerialDescriptor`), or `schema` | JSON Schema: before `apply` the config is filled with its defaults and validated against it (see [Configuration](#configuration)); `rutis-host check` does the same. |
| Startup | `apply(ctx, config)` | `apply(ctx, config)` | `Apply: func(ctx *rutis.Ctx, config C) error` | `apply: { ctx, config in ... }` (`async throws`) | `{ ctx, config -> }`, may suspend; an exception fails the load | May be async; returns a cleanup function or nothing (Go: an error fails the load). |

## Configuration

Before `apply`, each language's SDK fills a row's config with its schema's defaults and validates it, and `rutis-host check` does the same by the same rules (one shared case list, `specs/config-schema/cases.json`, keeps every language word for word the same):

- **Defaults**: no config (none, or `null`) is the root's `default`, else `{}` when the root's `type` is or includes `object`. Then level by level: in an object that is there, each missing property with a `default` is filled, then its child objects and array items. Through `$ref` and `allOf`, not `anyOf` / `oneOf`; a missing child object without a `default` of its own is not made (give it `default: {}`). `apply` gets a copy.
- **Keywords checked**: `type`, `properties`, `required`, `additionalProperties`, `items`, `enum`, `const`, `minimum`, `maximum`, `default`, `anyOf`, `oneOf` (matching one branch is enough), `allOf`, `$ref` (to `#/$defs/…` and `#/definitions/…`). Others (`format`, `pattern`, `minLength`, …) are neither checked nor warned about. No coercion: `"5"` is not a number.
- **Extra keys**: where the schema has `properties` and no `additionalProperties`, a key it does not name is a warning (the host's, once per row and value), with a close key suggested; its value reaches `apply` as it is. `additionalProperties: false` makes it an error.
- **A config that does not match**: the row fails and `apply` is not called. Every problem at once, sorted by path, one line each (at most 20, then `config: and N more problems`): `weather: config: units should be "metric" or "imperial", got "kelvin"`. Rows depending on it say `waiting for weather (row weather failed: config: …)`. Across languages the error is a `BindingError` with code `bad-config` and `data: { issues: [{ path, message }] }`; testing tools throw the same.
- **Typed configs** (Go, Swift, Kotlin, Rust): the schema comes from the type and states only its decoder's own rules; the config is validated first, then decoded. See each language's guide for which fields are required and where defaults come from.

## `ctx`

| Method | Purpose |
| --- | --- |
| `ctx.use(name)` | Gets a service declared in `inject`. A service provided by a plugin in the same runtime process is the object itself; services from other processes, languages, or machines are proxies. A service neither the plugin nor its row declares in `inject`, when the plugin does not get it, says `service <name> is not available to this plugin, which does not declare it in inject: add '<name>' to its inject` (only a declared service is sure to be given); calling a host service method that needs the calling row (a Rust method taking `&Ctx`, say) also says to declare the service in `inject` |
| `ctx.provide(name, value)` | Provides a service until the plugin is unloaded or the returned function is called. It reaches rutis by itself, used by name from other languages and processes; when `provides` does not declare it, its methods are the public methods of `value` (JavaScript: its own and its prototype chain's functions, not `constructor`; Python: callable attributes not starting with `_`; Go: exported methods), all called as `async`; fields are not methods. The inferred methods follow each language's rules, so the same service written in two languages can differ: a JavaScript class extending `EventEmitter` also exposes `on`, `emit` and the rest; list the methods in `provides` to expose only those. |
| `ctx.effect(cleanup)` | Runs `cleanup` when the plugin is unloaded. It may be async. |
| `ctx.on(name, listener, options?)` | Listens to an event: wherever it is emitted (another plugin, another runtime, the host, a linked node), a same-named event on this root's bus reaches `listener`. Unregistered when the plugin unloads; the returned function unregisters it at once. See [Events](#events) below. |
| `ctx.emit` / `parallel` / `serial` / `waterfall` / `bail` | The five ways to emit; `ctx.filter(keep)` gives the same five methods delivering to some listeners only. See [Events](#events). |
| `ctx.signal` | (JavaScript) Aborted when the plugin unloads; Python cancels the task, in Go it is `ctx` itself. |

An error that crossed a session and is thrown on crosses again with its whole error graph (causes, an `AggregateError`'s children) on Node, Bun, Python and Go, not only inside an aggregate: Node decodes it into a real `Error` whose re-encoding carries the whole graph, and Python's `encode_error` and Go's `encodeError` splice a `RemoteError`'s graph in (nodes appended, references shifted) to match.

Cleanups run in reverse registration order: the function returned by `apply` runs first, followed by functions registered with `ctx.effect`.

In Go: `ctx.Use(name, &target)` binds the service to a struct of function fields (or a `*advanced.Service`); `ctx.Provide(name, value)`; `ctx.Effect(func(context.Context) error)`; `ctx.Go(task)` runs a goroutine stopped at unload. `*rutis.Ctx` is a `context.Context` cancelled when the plugin unloads.

In Kotlin: `ctx.use<I>(name)` uses the service through the `@RutisClient` interface `I` (the provider's own object when one in this process implements `I`); `ctx.provide(name, obj)` takes an instance of a `@RutisService` class, whose method table the KSP processor generates at compile time; `ctx.effect { }`; `ctx` is a coroutine scope cancelled when the plugin unloads, and coroutines started with `ctx.launch { }` are cancelled and waited for then. A received error is a `RutisException`, matched by `name`.

Rust plugins in the host (built in, or dylib plugins): add `#[rutis::service]` to the service type's `impl` block, and once `ctx.provide(value)` provides it, it reaches other languages by name (by default the type's name, first word lowercased); rows of every language call it with `ctx.use(name)`. Unmarked Rust types are not served. See [Rust host](rust-host.en.md#mark-the-type-rutisservice). How to write the plugin itself: [write a Rust plugin](rust-plugin.en.md).

## Events

Every listener on a host — of the host's Rust plugins, of every plugin in every runtime process (any language), of what node links bring in — is on **one logical bus**. There is one rule:

> Wherever an event is emitted and wherever its listeners are, the result is the same as if every listener were in one process.

The only exceptions are what is physically impossible (a cycle of synchronous waits, waiting synchronously on a far end that went away); those fail with an error and never silently turn into something else. Design: [unified events](../design-events-2026-10-18.en.md); wire protocol: [protocol spec](../design-events-protocol-2026-10-18.en.md).

- **Registration is automatic**: a plugin writes `ctx.on(name, listener)` as usual and declares nothing, neither in `rutis.json` nor in the host's Rust code. A runtime registers its listeners with the host, as services reach rutis by themselves.
- **An event is a name, arguments and an emitter scope descriptor**. The name is a string, the same in every language and on every path, by convention `area/verb` (`session/created`). Arguments cross as in [passing values](#passing-values). The scope describes the loader instance the emitter is in; delivery is by name (below).
- **One listener table**: a listener registered once can be called by any of the modes below; the emitter chooses the mode.
- **Through the host only when needed**: when every listener of a name is in the emitter's process, the dispatch runs in that process without the host; the results are the same, only where the code runs differs.

### The five modes

| Mode | The emitter gets | Order | Return values | A listener fails |
| --- | --- | --- | --- | --- |
| `emit` | Returns at once, waits for nothing | Starts every listener in registration order; events of one emitter and one name reach each listener in emission order | Ignored | Reaches neither the emitter nor the other listeners; reported in one line where the listener runs (see [reported failures](#reported-failures)); never ends a process |
| `parallel` | Waits for all | Starts them in registration order; asynchronous parts overlap | Ignored | All run to the end; failures are thrown as an `AggregateError` with the failures in **registration order** (not completion order), even when only one failed |
| `serial` | The first return value that is a result, or no result | One at a time in registration order | The first result stops the dispatch and is returned | Thrown; the rest do not run |
| `waterfall` | The outermost listener's return value, as is | Outermost first; each listener's last argument is `next`; `next()` runs the rest of the chain and, at its end, the terminal | Passed along the chain; not calling `next` vetoes the rest | Thrown to the emitter; calling `next` twice, or after the dispatch ended, fails with `WaterfallNext` |
| `bail` (synchronous) | The first result, synchronously | Called synchronously in registration order; listeners elsewhere run as synchronous calls, under the same wait-cycle detection as synchronous service calls | As `serial` | Thrown, the rest do not run; a wait cycle (`SyncWaitCycle`) or a far end that went away fails it; an asynchronous listener fails it with `AsyncListener` |

- **No result**: returning `undefined` / `null` (JavaScript), `None` (Python), `nil` (Go), `None` (Rust), or `false` in any language (Rust's `Some(false)` included). Everything else is a result, `0` and `""` too.
- **`waterfall` returns as is**: no "result or not" normalization, `false` stays `false`. `next` takes no arguments and returns the result of the rest of the chain (JavaScript: a Promise; Python: an awaitable; Go: a blocking call; Rust: a future). The terminal receives the event's arguments; without one, the last `next()` gives no result. With no listeners, `waterfall` returns the terminal's value.
- **Asynchronous listeners**: a JavaScript `async` function, a Python `async def`, a Rust listener registered with `on` cannot be called by `bail` (`AsyncListener`); a plain function that returns a Promise / coroutine fails the same (the Promise / coroutine is discarded). Go listeners are all synchronous.
- **Rust's synchronous waterfall**: `ctx.waterfall(e).sync(terminal)` runs the whole chain synchronously on this thread, under the rules of `bail`: an `on_sync` listener calls `call.next()` as usual (synchronously; it returns the rest of the chain's value); an asynchronous `on` listener fails the dispatch with `AsyncListener`; a listener elsewhere (another process, another language) cannot get a synchronous `next`, so it can only be called asynchronously and fails the dispatch with `AsyncListener` too. The event is lent to the listeners (`Arc<E>`), never serialized, so it may carry fields that cannot be (`#[serde(skip)]`). Registration order, vetoes, the terminal and the result with no listener are as in the asynchronous `waterfall`.

### In each language

Each language has the five modes, written the language's own way:

| | Rust (`rutis`) | Node / Bun leaf plugin (`@arcships/rutis`) | Cordis plugin (in the Node runtime) | Python (`rutis`) | Go (`rutis`) | Swift (`Rutis`) | Kotlin（`arcships.rutis`） |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Listen | `ctx.on::<E>(listener)`; a synchronous listener `ctx.on_sync::<E>(..)`; options and prefixes `ctx.listen::<E>().prefix(p)` / `.name(n)` / `.once()` / `.prepend()` / `.global()`, then `.on(..)` → `Disposer` | `ctx.on(name, fn, { prepend, global }?)` → `() => void` | Cordis's own `ctx.on(name, fn, options?)` → `() => boolean`; `ctx.once` | `ctx.on(name, fn, *, prepend=False, global_=False)` → `Callable[[], None]` | `ctx.On(name, func(ctx context.Context, args ...any) (any, error), advanced.Prepend(), advanced.Global())` → `func()` | `ctx.on(name, prepend:, global:) { (args...) in ... }` → `() -> Void` | `ctx.on(name, prepend = false, global = false) { args -> }` → `() -> Unit` |
| Listener | a closure `\|event: Arc<E>, call: Call<E>\|` returning `Result<Option<E::Value>>`, with `call.mode()`, `call.next()`; an `on_sync` closure gets a `SyncCall` | `(...args[, next]) => value \| Promise` | The same | `fn(*args[, next])`, may be `async def` | `fn(ctx, args...)`; in a waterfall the last argument is a `advanced.Next` (`func(context.Context) (any, error)`) | a closure of any parameters, decoded by their types; in a waterfall the last one is a `Rutis.Advanced.Next<R>` (`async throws -> R?`) | `suspend (EventArgs) -> Any?`; in a waterfall `args.next` (`suspend () -> Any?`) is the last argument |
| `emit` | `ctx.emit(e)`, `e` an `E` or `Arc<E>` | `ctx.emit(name, ...args): void` | `ctx.emit([thisArg,] name, ...args)` | `ctx.emit(name, *args) -> None` | `ctx.Emit(ctx, name, args...) error` (refusals only) | `try ctx.emit(name, args...)` (refusals only) | `ctx.emit(name, *args)` (throws only when refused) |
| `parallel` | `ctx.parallel(e).await` | `await ctx.parallel(name, ...args)` | `await ctx.parallel([thisArg,] name, ...args)` | `await ctx.parallel(name, *args)` | `ctx.Parallel(ctx, name, args...) error` | `try await ctx.parallel(name, args...)` | `ctx.parallel(name, *args)` (suspends) |
| `serial` | `ctx.serial(e).await -> Option<E::Value>` | `await ctx.serial(name, ...args)` | `await ctx.serial([thisArg,] name, ...args)` | `await ctx.serial(name, *args)` | `ctx.Serial(ctx, name, args...) (any, error)` | `try await ctx.serial(name, args..., as: R.self)` | `ctx.serial(name, *args): Any?` (suspends) |
| `waterfall` | `ctx.waterfall(e).terminal(t).await` (`.terminal` optional); synchronously `ctx.waterfall(e).sync(t)?` | `ctx.waterfall(name, ...args)`; with a terminal `ctx.waterfall.with(terminal)(name, ...args)` (every argument is the event's, functions too) | `ctx.waterfall([thisArg,] name, ...args, terminal)` | `await ctx.waterfall(name, *args, terminal=None)` | `ctx.Waterfall(ctx, name, terminal, args...) (any, error)` (`terminal` a `rutis.Terminal`, `func(ctx, args ...any) (any, error)`, or `nil`) | `try await ctx.waterfall(name, args..., as: R.self) { terminal }` | `ctx.waterfall(name, *args, terminal = { args -> })` (suspends) |
| `bail` | `ctx.bail(e)` | `ctx.bail(name, ...args)` | `ctx.bail([thisArg,] name, ...args)` | `ctx.bail(name, *args)` | `ctx.Bail(ctx, name, args...) (any, error)` | `try await ctx.bail(name, args..., as: R.self)` (synchronous on the wire) | `ctx.bail(name, *args): Any?` (blocks its thread) |
| Filter | `ctx.filter(\|info\| ..)`, then the same five methods | `ctx.filter(d => boolean)` → `{ emit, parallel, serial, waterfall, bail }` | `thisArg[Context.filter] = c => boolean`, `c.rutis` is the descriptor | `ctx.filter(lambda d: ...)` → an object with the five methods | `ctx.Filter(func(rutis.Listener) (bool, error)) *rutis.Emitter` | `ctx.filter { (l: Rutis.Advanced.Listener) in ... }` → `Rutis.Advanced.Emitter` | `ctx.filter { d -> Boolean }` → `Emitter` |
| Scope | `ctx.scope()` | the descriptor's `scope` | `ctx.rutis.scope` | the descriptor's `scope` | `ctx.Scope()` | `ctx.scope` | `ctx.scope` |
| Unload signal | a listener is called with the `ctx` that registered it: `ctx.cancelled()` | `ctx.signal` | `ctx.signal` | the listener's task is cancelled | the listener's `ctx` is cancelled | the listener's Task is cancelled | the listener's coroutine is cancelled |
| `parallel` failures | `ErrorKind::ListenersFailed` | `AggregateError` | `AggregateError` | `rutis.advanced.AggregateError` (`errors`) | `*advanced.AggregateError` (`Errors`) | `RutisError` named `AggregateError` (`errors`) | `RutisException` (`name == "AggregateError"`, `errors`) |

Names with `advanced` in them are in each language's advanced submodule (0.19, #692): Go `github.com/arcships/rutis/go/rutis/advanced`, Python `rutis.advanced`, Swift `Rutis.Advanced`, Kotlin `arcships.rutis.advanced`, JavaScript and TypeScript `@arcships/rutis/advanced`.

In Node, `waterfall` returns a Promise, so always `await` it. In Go, pass the `ctx` you received when emitting from a plugin method (it carries the call chain), and `ctx` itself in `Apply`. Rust's emitting methods are on `Ctx`, named as in the other languages.

The same waterfall listener in the five languages:

```rust
# use rutis::{Ctx, Event};
# use serde::{Deserialize, Serialize};
# #[derive(Serialize, Deserialize)]
# pub struct Format(pub String);
# impl Event for Format {
#     const NAME: &'static str = "greeting/format";
#     type Value = String;
# }
# fn demo(ctx: &Ctx) -> rutis::Result {
// Format: impl Event, Value = String
ctx.on::<Format>(|_, call| async move { Ok(call.next().await?.map(|text| format!("{text}!"))) })?;
# Ok(())
# }
```

```ts
// A Node / Bun leaf plugin; a Cordis plugin writes the same
ctx.on('greeting/format', async (text: string, next: () => Promise<string>) => `${await next()}!`)
```

```python
async def exclaim(text, next):
    return f"{await next()}!"

ctx.on("greeting/format", exclaim)
```

```go
ctx.On("greeting/format", func(ctx context.Context, args ...any) (any, error) {
	inner, err := args[len(args)-1].(advanced.Next)(ctx)
	if err != nil {
		return nil, err
	}
	return fmt.Sprint(inner, "!"), nil
})
```

### Examples

One complete example per language, each doing the same thing: the plugin answers `greeting/pick` and adds a link to the `greeting/format` waterfall; the `greeter` it provides takes the first result with `serial`, lets other plugins (in any language) rewrite the greeting with `waterfall`, and notifies with `emit`. For Cordis plugins, see [Cordis](cordis.en.md#events).

#### TypeScript

The example is a test file; run it in a project `rutis-host new` made:

<!-- example: plugin-api-ts-en run -->
```bash
npx @arcships/rutis-host new greeter --lang node
cd greeter
npm install
```

The two plugins `greeter` and `shout` share one bus on the test tool's `host()`: `shout` is a link of the waterfall. The test tool loads plugins on a real host by their files, so each plugin is the default export of its own file:

<!-- example: plugin-api-ts-en file=greeter/src/talk.ts -->
```ts
import { definePlugin } from '@arcships/rutis'

export default definePlugin({
  apply(ctx) {
    // undefined / null / false is "no result": serial asks the next listener
    ctx.on('greeting/pick', (lang: string) => (lang === 'sv' ? 'Hej' : undefined))
    ctx.provide('greeter', {
      async hello(name: string, lang: string) {
        const word = (await ctx.serial<string>('greeting/pick', lang)) ?? 'Hello'
        // Every argument is the event's; the terminal is given with `with`: the innermost link
        const text = await ctx.waterfall.with((text: string) => text)<string>('greeting/format', `${word}, ${name}`)
        ctx.emit('greeting/said', name)                  // returns at once, without waiting for listeners
        return text
      },
    })
  },
})
```

<!-- example: plugin-api-ts-en file=greeter/src/shout.ts -->
```ts
import { definePlugin } from '@arcships/rutis'

export default definePlugin({
  apply(ctx) {
    // A waterfall listener's last argument is next: the rest of the chain
    ctx.on('greeting/format', async (text: string, next: () => Promise<string>) => `${await next()}!`)
  },
})
```

<!-- example: plugin-api-ts-en file=greeter/test/events.test.ts -->
```ts
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { host } from '@arcships/rutis/testing'
import talk from '../src/talk.ts'
import shout from '../src/shout.ts'

test('plugins hear each other', async () => {
  const h = host()
  const t = await h.load(talk)
  await h.load(shout)
  const said: string[] = []
  t.on('greeting/said', (name: string) => { said.push(name) })   // the test listens as a plugin next to it
  assert.equal(await t.service('greeter').hello('Ada', 'sv'), 'Hej, Ada!')
  assert.equal(await t.service('greeter').hello('Bo', 'en'), 'Hello, Bo!')
  assert.equal(await h.serial('greeting/pick', 'sv'), 'Hej')   // the test emits too
  await h.unload()
  assert.deepEqual(said, ['Ada', 'Bo'])
})
```

<!-- example: plugin-api-ts-en run -->
```bash
npm test
```

- `emit` returns at once; a failing listener reaches neither the emitter nor the other listeners and is reported in one line by the host (in a test, `await h.reported()` / `await t.reported()`). To wait for every listener and get their errors, use `await ctx.parallel(...)`: failures are an `AggregateError` in registration order.
- `waterfall` returns a Promise, so always `await` it. `bail` is a synchronous `serial`; an `async` listener fails it with `AsyncListener`.
- `ctx.filter(d => d.row === 'audit').parallel(...)` delivers only to the listeners the filter keeps; `ctx.on(name, fn, { prepend: true })` goes first, `{ global: true }` is exempt from filters.
- Scopes: the listeners of a plugin loaded into an instance carry the scope as a descriptor, and a listener receives every event of the name on the root bus, whatever the emitter's instance.
- On unload the plugin's `ctx.signal` is aborted and its running listeners are waited for (up to the shutdown deadline) before the cleanups run.
- `h` and the `t` that `load(plugin)` returns have the five methods and `filter` too, emitting from the test's own plugin.

#### Bun

The same SDK and test tool as TypeScript; tests run with `bun test`. Events emitted by Node, Python and Go plugins and by Rust code in the host reach listeners in Bun too, and the other way round.

```ts
import { definePlugin } from '@arcships/rutis'

export default definePlugin({
  apply(ctx) {
    ctx.on('greeting/pick', (lang: string) => (lang === 'sv' ? 'Hej' : undefined))   // undefined when there is no result
    ctx.on('greeting/format', async (text: string, next: () => Promise<string>) => `${await next()}!`)
    ctx.provide('greeter', {
      async hello(name: string, lang: string) {
        const word = (await ctx.serial<string>('greeting/pick', lang)) ?? 'Hello'
        const text = await ctx.waterfall.with((text: string) => text)<string>('greeting/format', `${word}, ${name}`)
        ctx.emit('greeting/said', name)                                               // returns at once
        return text
      },
    })
  },
})
```

`host()` puts several plugins on one bus:

```ts
import { expect, test } from 'bun:test'
import { host } from '@arcships/rutis/testing'
import plugin from '../src/index.ts'

test('greets through events', async () => {
  const h = host()
  const t = await h.load(plugin)
  expect(await t.service('greeter').hello('Ada', 'sv')).toBe('Hej, Ada!')
  expect(await h.serial('greeting/pick', 'en')).toBeUndefined()
  await h.unload()
})
```

#### Python

The example is a test file; run it in a project `rutis-host new` made:

<!-- example: plugin-api-py-en run -->
```bash
uvx rutis-host new greeter --lang python
cd greeter
uv sync
```

The two plugins `greeter` and `shout` are on one `Bus`:

<!-- example: plugin-api-py-en file=greeter/tests/test_events.py -->
```python
import asyncio
import unittest

from rutis import define_plugin
from rutis.testing import Bus, load


def greeter_apply(ctx, config):
    # None or False is "no result": serial asks the next listener
    ctx.on("greeting/pick", lambda lang: "Hej" if lang == "sv" else None)

    class Greeter:
        async def hello(self, name, lang):
            word = await ctx.serial("greeting/pick", lang) or "Hello"
            # terminal: the innermost end of the chain, given the event's arguments
            text = await ctx.waterfall("greeting/format", f"{word}, {name}", terminal=lambda text: text)
            ctx.emit("greeting/said", name)      # returns at once, waits for no listener
            return text

    ctx.provide("greeter", Greeter())


greeter = define_plugin(greeter_apply)
said = []


def shout_apply(ctx, config):
    async def exclaim(text, next):           # a waterfall listener gets next last
        return f"{await next()}!"

    ctx.on("greeting/format", exclaim)
    ctx.on("greeting/said", said.append)


shout = define_plugin(shout_apply)


class Events(unittest.TestCase):
    def test_plugins_hear_each_other(self):
        async def go():
            bus = Bus()                          # plugins on one bus hear each other
            async with load(greeter, bus=bus, id="greeter") as t, load(shout, bus=bus, id="shout"):
                self.assertEqual(await t.service("greeter").hello("Ada", "sv"), "Hej, Ada!")
                self.assertEqual(await t.service("greeter").hello("Bo", "en"), "Hello, Bo!")
                self.assertEqual(await t.serial("greeting/pick", "sv"), "Hej")  # the test can emit too
                self.assertEqual(said, ["Ada", "Bo"])    # emits go through the host: delivered once the serial after them is back

        asyncio.run(go())
```

<!-- example: plugin-api-py-en run -->
```bash
uv run python -m unittest discover -s tests
```

- `emit` returns at once; a failing listener reaches neither the emitter nor the other listeners and the host reports it in one line (in a test, in `await t.reported()`). To wait for every listener and get their errors, use `await ctx.parallel(...)`: failures are a `rutis.advanced.AggregateError` whose `errors` are in registration order.
- `bail` is a synchronous `serial`: an `async def` listener (or a function returning a coroutine) fails it with `rutis.advanced.AsyncListener`.
- `ctx.filter(lambda d: d["row"] == "audit")` returns an object with the five methods that delivers only to the listeners the filter keeps; `ctx.on(name, fn, prepend=True)` goes first, `global_=True` is exempt from filters.
- On unload, the tasks of running listeners are cancelled and waited for (at most `RUTIS_SHUTDOWN_TIMEOUT`) before the cleanups run. When the emitter gives up (its task is cancelled), the listeners in progress are cancelled too.

#### Go

```go
off := ctx.On("session/created", func(ctx context.Context, args ...any) (any, error) {
	return nil, nil // nil and false are no result; anything else is one
}, advanced.Prepend()) // options: advanced.Prepend(), advanced.Global()

err := ctx.Emit(callerCtx, "session/created", id)          // returns at once; err only for a refused emission
err = ctx.Parallel(callerCtx, "session/created", id)        // waits for all; *advanced.AggregateError in registration order
value, err := ctx.Serial(callerCtx, "config/resolve", key)   // the first result
value, err = ctx.Bail(callerCtx, "config/resolve", key)      // Serial as a synchronous call
value, err = ctx.Waterfall(callerCtx, "chat/reply", terminal, msg) // terminal may be nil; terminal is a rutis.Terminal, func(ctx context.Context, args ...any) (any, error), called with the event's arguments
value, err = ctx.Filter(func(l rutis.Listener) (bool, error) { return l.Row == "chat", nil }).Serial(callerCtx, "config/resolve", key)
```

The complete example:

```go
package greeter

import (
	"context"
	"fmt"

	"github.com/arcships/rutis/go/rutis"
	"github.com/arcships/rutis/go/rutis/advanced"
)

type Greeter struct{ ctx *rutis.Ctx }

func (g *Greeter) Hello(ctx context.Context, name, lang string) (string, error) {
	word, err := g.ctx.Serial(ctx, "greeting/pick", lang) // the first result; nil when none
	if err != nil {
		return "", err
	}
	if word == nil {
		word = "Hello"
	}
	text, err := g.ctx.Waterfall(ctx, "greeting/format", func(_ context.Context, args ...any) (any, error) {
		return args[0], nil // the terminal: the innermost end of the chain
	}, fmt.Sprintf("%v, %s", word, name))
	if err != nil {
		return "", err
	}
	if err := g.ctx.Emit(ctx, "greeting/said", name); err != nil { // returns at once
		return "", err
	}
	return text.(string), nil
}

var Plugin = rutis.Define(rutis.Plugin[struct{}]{
	Name: "greeter",
	Apply: func(ctx *rutis.Ctx, _ struct{}) error {
		ctx.On("greeting/pick", func(_ context.Context, args ...any) (any, error) {
			if args[0] == "sv" {
				return "Hej", nil
			}
			return nil, nil // no result: the next listener goes on
		})
		ctx.On("greeting/format", func(ctx context.Context, args ...any) (any, error) {
			next := args[len(args)-1].(advanced.Next) // waterfall: next is the last argument
			inner, err := next(ctx)
			if err != nil {
				return nil, err
			}
			return fmt.Sprintf("%v!", inner), nil
		})
		ctx.Provide("greeter", &Greeter{ctx: ctx})
		return nil
	},
})
```

Its test is in [the Go test tool](#the-go-test-tool).

| Topic | Rule |
| --- | --- |
| Waterfall | The listener gets `next` (a `advanced.Next`, `func(context.Context) (any, error)`) as its last argument; calling it runs the rest of the chain, not calling it vetoes the rest; a second call fails with `WaterfallNext`. The outermost value is returned as it is |
| Failures | A failing `Emit` listener never reaches the emitter or the other listeners: it is reported where the listener runs (`rutis: listener of <name> in row <row> failed: …` on stderr). `Parallel` runs all and returns an `*advanced.AggregateError` (even for one failure); `Serial`, `Bail` and `Waterfall` stop at the first error and return it |
| Scope | Events are delivered by name on one bus; `ctx.Scope()` is a descriptor for filters, not a delivery rule |
| Filter | `Filter(keep)` asks `keep` once per listener, in registration order, before any runs, with its `rutis.Listener` (`Name`, `Prefix`, `Plugin`, `Row`, `Runtime`, `Node`, `Scope`, `Global`); `keep` returns `(bool, error)`: an error it returns (or a panic) fails the dispatch before any listener runs, with that error (`Emit` does not return it but reports a line `rutis: filter of <name> from row <row> failed: …`); listeners registered with `advanced.Global()` skip filters |
| Context | A listener's `ctx` is the emitter's (its call chain), cancelled when the row unloads, and when the emitter of a `Parallel`, `Serial`, `Waterfall` or `Bail` gives up; an `Emit` listener is not cancelled when the emitter gives up. Unloading waits for running listeners, at most `RUTIS_SHUTDOWN_TIMEOUT` |
| Names | `internal/*` is the runtime's own; `rutis/*` (`rutis/service`, `rutis/row`, `rutis/runtime`) can be listened to but not emitted (`ReservedEvent`) |

Pass on the context your caller gave you, as with service calls. `errors.Is(err, advanced.ErrWaterfallNext)` (and `ErrReservedEvent`, `ErrAsyncListener`, `ErrBinding`) matches an error of that name from any process.

#### Swift

A listener's parameters are the event's arguments, decoded by their types (missing ones `nil`, extra ones ignored); in a waterfall its last parameter is a `Rutis.Advanced.Next<R>`.

```swift
let off = ctx.on("session/created") { (id: String) in print("new session \(id)") }   // options: prepend:, global:

try ctx.emit("session/created", id)                                   // returns at once; throws only for a refused emission
try await ctx.parallel("session/created", id)                         // waits for all; a RutisError named AggregateError
let value = try await ctx.serial("config/resolve", key, as: String.self)   // the first result (anything but nil and false)
let first = try await ctx.bail("config/resolve", key, as: String.self)     // serial as a synchronous call
let text = try await ctx.waterfall("chat/reply", message, as: String.self) { (message: String) in message }   // the terminal: the end of the chain
let some = try await ctx.filter { $0.row == "chat" }.serial("config/resolve", key, as: String.self)
```

The complete example:

```swift
import Rutis

@RutisService
public final class Greeter: @unchecked Sendable {
    let ctx: Rutis.Ctx
    init(ctx: Rutis.Ctx) { self.ctx = ctx }

    public func hello(_ name: String, _ lang: String) async throws -> String {
        let word = try await ctx.serial("greeting/pick", lang, as: String.self) ?? "Hello"
        let text = try await ctx.waterfall("greeting/format", "\(word), \(name)", as: String.self) { (text: String) in text }
        try ctx.emit("greeting/said", name)
        return text ?? ""
    }
}

public let greeter = Rutis.define(Plugin(name: "greeter") { ctx in
    ctx.on("greeting/pick") { (lang: String) -> String? in lang == "sv" ? "Hej" : nil }   // nil: the next listener goes on
    ctx.on("greeting/format") { (text: String, next: Rutis.Advanced.Next<String>) in
        (try await next() ?? text) + "!"                                                // next runs the rest of the chain
    }
    ctx.provide("greeter", Greeter(ctx: ctx))
})
```

The rules (waterfall, failures, scope, filter, cancellation, reserved names) are those of the Go table above. In Swift, a listener's Task is cancelled when its row unloads, and when the emitter of a `parallel`, `serial`, `waterfall` or `bail` gives up; every listener is called as a synchronous one, so `bail` reaches Swift listeners anywhere.

#### Rust

How to write an event type, `ctx.on` and `ctx.emit`: [write a Rust plugin: events](rust-plugin.en.md#5-emit-and-listen-to-events). A listener reads the dispatch's mode from `call.mode()`; `on_sync` registers a synchronous listener; `listen::<E>()` gives a name, a prefix and options:

```rust
use rutis::events::Mode;
use rutis::{Ctx, Event, Plugin, Result};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub struct Created(pub String, pub u32);

impl Event for Created {
    const NAME: &'static str = "session/created";
    type Value = String; // what serial, bail and waterfall return
}

struct Audit;

impl Plugin for Audit {
    type Deps = ();

    async fn apply(&self, ctx: &Ctx, _: ()) -> Result {
        ctx.on::<Created>(|event, call| async move {
            if call.mode() == Mode::Waterfall {
                let inner = call.next().await?; // middleware: run the rest of the chain first
                return Ok(inner.map(|text| format!("[{text}]")));
            }
            Ok(Some(format!("audited {}", event.0))) // Ok(None) is no result
        })?;
        ctx.on_sync::<Created>(|event, _| Ok(Some(format!("sync {}", event.1))))?;
        ctx.listen::<Created>().prefix("session/").on(|_, call| async move {
            println!("{}", call.name()); // a prefix listener: the actual name
            Ok(None)
        })?;
        Ok(())
    }
}
```

The five ways to send, and filters:

```rust
# use rutis::{Ctx, Event};
# use serde::{Deserialize, Serialize};
# #[derive(Serialize, Deserialize)]
# pub struct Created(pub String, pub u32);
# impl Event for Created {
#     const NAME: &'static str = "session/created";
#     type Value = String;
# }
# async fn demo(ctx: &Ctx) -> rutis::Result {
ctx.emit(Created("s1".into(), 3))?;                         // returns at once
ctx.parallel(Created("s1".into(), 3)).await?;               // waits for all
let first = ctx.serial(Created("s1".into(), 3)).await?;     // the first result
let out = ctx
    .waterfall(Created("s1".into(), 3))
    .terminal(|e| async move { Ok(Some(format!("saved {}", e.0))) }) // optional
    .await?;
let quick = ctx.bail(Created("s1".into(), 3))?;             // synchronous
let chain = ctx
    .waterfall(Created("s1".into(), 3))
    .sync(|e| Ok(Some(e.0.clone())))?;                       // a synchronous waterfall
let from_python = ctx
    .filter(|l| l.runtime.as_deref() == Some("py"))
    .serial(Created("s1".into(), 3))
    .await?;
# let _ = (first, out, quick, chain, from_python);
# Ok(())
# }
```

| Topic | Rule |
| --- | --- |
| Listener | A closure taking `(Arc<E>, Call<E>)`, a synchronous one `(Arc<E>, SyncCall<'_, E>)`; it returns `Result<Option<E::Value>>`, where `Ok(None)` and `Ok(Some(false))` are no result. The compiler infers the parameter types, and `?` works on any error inside. The closure is called many times: clone what moves into `async move` inside the closure first |
| `call` | `mode()` is this dispatch's mode; `name()` the dispatched name; `next()`, in a `waterfall` only (elsewhere `WaterfallNext`), runs the rest of the chain, and not calling it vetoes the rest; `ctx()` is the `Ctx` that registered the listener, and what is registered on it belongs to this plugin |
| Sync and async | Every mode can call an `on_sync` listener, but it can call `next` only in a synchronous waterfall (`.sync(…)`): in an async `waterfall`, `next` fails with `WaterfallNext`; neither `bail` nor a synchronous waterfall can call an `on` listener (`AsyncListener`) |
| Names | Registered and sent under `E::NAME`. `listen::<E>().name(n)` listens under another name; `.prefix(p)` hears every name under the prefix, each decoded as `E`, and an event of another shape fails that call with `EventValue` |
| Sending | `emit`, `parallel`, `serial`, `waterfall` and `bail` take an `E` or an `Arc<E>`; `waterfall` returns a builder: `.terminal(…)` first if wanted, then `.await`; without a terminal, the last `next()` gets `None`. Or `.sync(terminal)`, which runs it to its end synchronously and returns `Result<Option<E::Value>>` (the terminal is a synchronous `Send` closure and may borrow the caller's locals). `.sync(…)` after `.terminal(…)` fails with `Validation` and runs nothing. `ctx.filter(keep)` gives the same five methods, `keep` seeing each listener's `ListenerInfo` |
| Failures | An `emit` listener's failure does not reach the emitter; it is reported to the host. `parallel` runs every listener and returns `ListenersFailed` (in registration order, also for a single failure); `serial`, `bail` and `waterfall` stop at the first error and return it |
| Unload | When the plugin stops its listeners are withdrawn; running calls see `call.ctx().cancelled()`, and the unload waits for them, at most until the shutdown deadline. Dropping a `parallel`, `serial` or `waterfall` future cancels that dispatch |
| Reserved names | `rutis/*` (`rutis/service`, `rutis/row`, `rutis/runtime`) can be listened to, not sent (`ReservedEvent`) |

When a synchronous waterfall (`.sync(…)`) fails, the error's `to_string()` is, word for word: meeting an asynchronous listener registered with `on` (`ErrorKind::AsyncListener`), meeting a listener elsewhere (`ErrorKind::AsyncListener`, here row `chat`'s), `.terminal(…)` written first (`ErrorKind::Validation`):

```text
listener of t/draft in plugin root is asynchronous and cannot run in a synchronous waterfall
listener of t/draft in row chat, in another process, is asynchronous and cannot run in a synchronous waterfall
config validation failed: ["a waterfall with an asynchronous terminal cannot run synchronously: pass the terminal to .sync(…) instead of .terminal(…)"]
```

`once`, `prepend`, `global`:

```rust
# use rutis::{Ctx, Event};
# use serde::{Deserialize, Serialize};
# #[derive(Serialize, Deserialize)]
# pub struct Created(pub String, pub u32);
# impl Event for Created {
#     const NAME: &'static str = "session/created";
#     type Value = String;
# }
# fn demo(ctx: &Ctx) -> rutis::Result {
ctx.listen::<Created>().once().on(|_, _| async move { Ok(None) })?;    // called at most once
ctx.listen::<Created>().prepend().on_sync(|_, _| Ok(None))?;           // before this name's existing listeners
ctx.listen::<Created>().global().on(|_, _| async move { Ok(None) })?;  // filters skip it
# Ok(())
# }
```

Options chain, ending with `.on(…)` or `.on_sync(…)`. An event type with no fixed name (the value decides it) has an empty `NAME` and overrides `fn name(&self) -> &str` to return the value's name; listen to it with `.name(…)`.

#### Across languages

A TypeScript plugin emits, a Python plugin listens, nothing to change. The host's `rutis.json`:

```json
{
  "runtimes": { "node": { "project": "web" }, "py": { "project": "plugins" } },
  "rows": [
    { "id": "sessions", "name": "sessions" },
    { "id": "audit", "name": "py:audit" }
  ]
}
```

The TypeScript plugin `sessions` in `web/` emits the event, and asks others for a quota with `serial`:

```ts
ctx.provide('sessions', {
  async create(user: string) {
    const quota = (await ctx.serial<number>('session/quota', user)) ?? 1
    const id = crypto.randomUUID()
    await ctx.parallel('session/created', id, user, quota)   // waits for every listener, in any language
    return id
  },
})
```

`plugins/audit.py` answers the quota and records the creation:

```python
import sys

from rutis import define_plugin


def apply(ctx, config):
    ctx.on("session/quota", lambda user: 10 if user == "ada" else None)

    async def created(session_id, user, quota):
        print(f"session {session_id} for {user}, quota {quota}", file=sys.stderr)

    ctx.on("session/created", created)


plugin = define_plugin(apply)
```

Arguments cross processes as in [passing values](#passing-values); an error a Python listener raises is an error of the same name on the TypeScript side (in `parallel`, a child of the `AggregateError`).

### Scope

A scope is a loader group instance (a copy of an `instanced` group, such as a tenant or a session), or the root. Every plugin and every emission is in one scope: the innermost instance around it, or the root.

> Events are one flat bus: every listener of the name in a root receives every dispatch, whatever the emitter's or listener's scope.

A scope is only a `ListenerInfo` descriptor for filters; there is no default instance isolation. To keep an event within an instance, filter on the descriptor's `scope` or use distinct event names. The host determines the emitter's scope from the emitting row. A leaf plugin's `isolate` label (a service scope) has nothing to do with event scopes.

### Filter

An emission can carry a filter function: it is called once for each candidate listener with that listener's **descriptor** and returns whether to deliver.

- It runs **in the emitter's process**, in registration order, before any listener starts; it sees the name-matched candidates, and works the same for listeners anywhere.
- When the filter throws, the whole dispatch fails with that error and no listener starts (in `emit`, it is reported instead).
- Listeners registered with the `global` option are never filtered.

The descriptor (JSON fields, `null` when there is no value; Rust `ListenerInfo`, `None` when there is none; Go `rutis.Listener`, `""` when there is none):

| Field | Meaning |
| --- | --- |
| `name` | The event name (a prefix, for a Rust prefix listener) |
| `prefix` | Whether it is a Rust prefix listener |
| `plugin` | The plugin that registered it: its declared name (`definePlugin({ name })`, `define_plugin(name=)`, the Go plugin's name, a Cordis plugin's name), else its row's id |
| `row` | Its row's id, the name the configuration gives the row (the copies a group's instances make share it); `null` for a plugin loaded by a mount |
| `runtime` | The runtime it runs in; `null` for the host's own Rust plugins |
| `node` | For a listener a node link brought in, the far node's id; `null` otherwise |
| `scope` | Its scope chain, `[{ id, group }]`, outermost first, the root `[]` |
| `global` | Whether it was registered with `global` |

Cordis plugins use Cordis's own way: in `ctx.emit(thisArg, name, ...)` (and the other four), when `thisArg[Context.filter]` is a function it is the filter. A listener in this process is passed its own Cordis `Context`, any other listener `{ rutis: descriptor }`; in the Node runtime every Context has a read-only `ctx.rutis`, so `c => c.rutis.row === 'chat'` works for both.

### `prepend` and `global`

Every language has these two listener options: `prepend` puts the listener before every listener of the name registered so far (the host decides the one registration order everyone sees); `global` exempts it from filters. Rust: `ctx.listen::<E>().prepend()` (also `.global()`, `.once()`), then `.on(..)` / `.on_sync(..)`; Node / Bun: `ctx.on(name, fn, { prepend: true })` (for a Cordis plugin, Cordis's own options); Python: `ctx.on(name, fn, prepend=True, global_=True)`; Go: `ctx.On(name, fn, advanced.Prepend(), advanced.Global())`.

### Lifecycle

- A listener belongs to the plugin (row) that registered it and is unregistered when the plugin unloads; the returned function unregisters it at once.
- **On unload**: the row's listeners leave the bus first (later events no longer reach them); the ones still running get a cancellation signal — Go the listener's `context`, Python task cancellation, JavaScript the row's `ctx.signal` aborted, Rust `ctx.cancelled()` — and are waited for, at most `RUTIS_SHUTDOWN_TIMEOUT` (10 seconds by default; the host's Rust plugins use the host's shutdown deadline), before the cleanups run. Past it, the runtime writes `rutis: listeners of row <row> still running after <s>s; unloading anyway` on stderr and goes on.
- A call already on its way to a withdrawn listener fails once (`BindingError`, `listener <id> of <name> is withdrawn`, or a transport error); it is never retried.
- **The emitter gives up** (Rust drops the future, Python cancels the task, Go cancels the context): no further listener starts and running ones are cancelled; the emitter gets its language's cancellation. `emit` is not cancelled. JavaScript has no way to cancel a dispatch, and JavaScript listeners have no per-call signal (only `ctx.signal`, on unload).
- Which context a listener runs in: a Rust listener is called with the `ctx` **that registered it**, so what it registers belongs to the listener's plugin; a Go listener's `ctx` carries the emitter's call chain and is cancelled when the emitter gives up or the row unloads: pass it on to the services the listener calls.
- As with Rust's `Ctx::dispose_self`, a listener must not wait for its own row to finish unloading: return from the listener first.

### Disconnection

When a runtime process or a node link goes away, its listeners leave the bus as if their plugins had unloaded: later events no longer go to them (the listeners are gone; nothing is silently dropped). In a dispatch in progress, a call to the side that went away fails once, with the same transport error a service call gets (`BindingError`): `parallel` puts it in its `AggregateError`, `serial` / `waterfall` / `bail` throw it to the emitter, `emit` reports a line. A synchronous wait on it fails the same. There is no call timeout, as for service calls.

### Framework events `rutis/*`

The host publishes framework information with `emit`, one argument (a record) per event. Plugins in every language can listen; any plugin emitting `rutis/*` (in any mode) fails with `ReservedEvent`.

| Event | Argument | When | Scope |
| --- | --- | --- | --- |
| `rutis/service` | `{ name, scope, available, row }` | A service name gets a provider in a scope (`available: true`), the provider is replaced (`true` again, the new `row`), or it is withdrawn with no provider left (`false`) | The service's scope |
| `rutis/row` | `{ row, plugin, scope, state, error }`, `state` one of `pending` / `running` / `failed` / `stopped` | Every state change of a configured row (any language, Rust rows and group rows included); `failed` with `error` | The row's scope |
| `rutis/runtime` | `{ runtime, state, reason }`, `state` one of `running` / `down` | A runtime mounted (after every reconnection too); its session ended or its mount failed (`reason` says why) | The root |

`rutis/row` is an event on the same bus, so every listener sees it. In Rust the types are `rutis_loader::RowEvent` and `rutis_bridge::events::{ServiceEvent, RuntimeEvent}`. The test tools emit no `rutis/*` events.

`internal/*` is each runtime's own framework machinery (Cordis's `internal/dispatch`, `internal/service` and the like) and never crosses processes: leaf plugins cannot listen to or emit it, and the host refuses to register it (`event <name> is internal to its runtime`). Cordis plugins use it in their process as before; events arriving from elsewhere also go through Cordis's own dispatch, so `internal/dispatch` validators still see them.

### Values

An event's arguments, results, and the results of `next` and the terminal cross on every path as in [passing values](#passing-values): the ten values, references to functions and objects, the 64-level depth limit, between nodes too. A value that cannot be sent fails only that one call (in `parallel` one child of the `AggregateError`, in `emit` a reported line); when the emitter's own arguments cannot be sent, the whole dispatch fails. A reference passed as an argument lives until that call ends.

Rust typed events are named across languages by `Event::NAME` and their arguments are serde-encoded, so an `Event` and its `Value` derive `Serialize` / `Deserialize`. Arity: a tuple struct of two or more fields (`struct Created(String, u32)`) is that many arguments; a unit struct is none; anything else (a struct with named fields, a newtype, a map, a sequence) is one argument. Listeners are found by name: a listener of another Rust type with the same name receives the event too, converted through serde. To receive any arguments by name, listen with `rutis_bridge::events::Args` (`ctx.listen::<Args>().name(name)`); an event type whose value decides its name overrides `Event::name`.

### Reported failures

A failure nobody waits for is written in one line where it happens (a runtime on stderr; the host into its error sink, which rutis-host prints with the same text):

| When | The line |
| --- | --- |
| A listener fails in `emit` | `rutis: listener of <name> in row <row> failed: <ErrorName>: <message>` |
| A call that could not be delivered or got no answer (transport, a value, withdrawn), reported by the dispatcher | `rutis: listener of <name> in runtime <runtime> failed: …` or `… in node <peer> failed: …` |
| The filter of an `emit` fails | `rutis: filter of <name> from row <row> failed: …` |
| An `emit` through the host is refused or not answered | `rutis: emit of <name> from row <row> failed: …` (for a plugin a mount loaded `from the mount`, in the filter line too) |
| The host refuses to register a listener (while the row loads, the load fails instead) | `rutis: registration of listener of <name> in row <row> failed: …` |

### Errors

| Name | When | In each language |
| --- | --- | --- |
| `AggregateError` | `parallel` had failures; children in registration order, message `<n> listeners of <name> failed` | JS `AggregateError` (`errors`); Python `rutis.advanced.AggregateError` (`errors`); Go `*advanced.AggregateError` (`Errors`); Rust `ErrorKind::ListenersFailed` |
| `AsyncListener` | `bail` or Rust's synchronous waterfall met an asynchronous listener (or, for the synchronous waterfall, a listener elsewhere) | JS an `Error` named `AsyncListener`; Python `rutis.advanced.AsyncListener`; Go `advanced.ErrAsyncListener`; Rust `ErrorKind::AsyncListener` |
| `WaterfallNext` | `next` called twice, after the dispatch ended or was cancelled; in Rust, `call.next()` outside a waterfall, or by a synchronous listener in an asynchronous waterfall | JS `WaterfallNext`; Python `rutis.advanced.WaterfallNext`; Go `advanced.ErrWaterfallNext`; Rust `ErrorKind::WaterfallNext` |
| `ReservedEvent` | Emitting `rutis/*`, refused synchronously by the emitting API | JS `ReservedEvent`; Python `rutis.advanced.ReservedEvent`; Go `advanced.ErrReservedEvent`; Rust `ErrorKind::ReservedEvent` |
| `SyncWaitCycle` | A synchronous call (`bail`) would close a cycle of waits | As for service calls: Python `rutis.advanced.SyncWaitCycle`; Go `advanced.ErrSyncWaitCycle` |
| `BindingError` | A transport failure, a withdrawn listener, a refused frame | Python `rutis.BindingError`; Go `rutis.ErrBinding`; Rust `ErrorKind::ListenerWithdrawn` or a transport error |

In Go, check with `errors.Is(err, advanced.ErrWaterfallNext)` and the like; an error of that name from any process matches. An error a listener throws crosses with its name, message and error graph, as any call's error.

**Match a listener's error by its name, not its class.** The host dispatches every event, also when the listener and the emitter are in the same runtime, so what arrives is always the language's remote error (TypeScript: an `Error` of the same built-in class or a plain `Error` with that `name`; Python: `rutis.RemoteError`, rutis's own names as their classes; Go: `*rutis.RemoteError`, `errors.As` then `Name`), with its `name`, `message` and `code`, not the object the listener threw. A match by the plugin's own error class fails, in a test as in a host.

### Mixing with 0.12 and earlier

The event protocol is rutis 0.13's, with no old path. A runtime or node of 0.12 or earlier connecting to a current host is refused by its protocol version before anything loads:

```text
incompatible session: the far end speaks protocol 2, this side 4: upgrade the far end
```

## Lifecycle

The host controls when a plugin runs:

- It starts (`apply`) when all injected services are ready.
- If any injected service is revoked, it stops and cleans up. It starts again when the service returns.
- A configuration change restarts it: cleanup runs, then `apply` runs with the new configuration.
- When a provided service is revoked, plugins that use it stop first, followed by its provider.

Services obtained in `apply` remain valid while the plugin is running, so the plugin does not need to handle a service disappearing midway through its work.

A host using `rutis-host` or `LoaderPlugin` provides the service `loader` (`create`, `reload`, `remove`, `status`; see [The host writes its own plugins](../design-host-writes-plugins-2026-10-27.en.md)). Its methods are called after `apply` returns (in a service method or an event): called in a row's own `apply`, they wait until that `apply` ends, so they hang. To mount child plugins in `apply`, use `ctx.plugin` (Rust; Cordis plugins in Node).

A service name has one provider in rutis: when another plugin (any language, any node) already provides a service of that name, the plugin providing it later fails (to load, or, when it provides it after loading, at that point), and the error names both, for example `service weather is already provided by row forecast: row weather cannot provide it too`. Only that plugin fails: the others, those in its runtime included, keep using the provider they had. An undeclared service appears only once its plugin runs; plugins depending on it wait for it as usual, and `rutis-host check` reports such a dependency with a warning, `nothing declares <service>; it may be provided at runtime by …`, not as a failure (with `check --strict`, as one: see [Checking a configuration](rutis-host.en.md#checking-a-configuration)). The same for one provided from a thread or goroutine before `apply` returns: the host has handled it before the plugin has loaded, and a clash fails the load.

When the host stops, it unloads every plugin, each cleanup runs once, and then the runtime process exits. When the host goes away without unloading them (it was killed, or crashed), the runtime sees its connection to the host close, unloads every plugin itself, and exits; timers, sockets, threads or goroutines a plugin left behind do not keep it running.

Cleanups have a deadline: when the host stops, the `--shutdown-timeout` of `rutis-host` (10 seconds by default, see [rutis-host](rutis-host.en.md#stopping-and-exit-status)); when the host went away, the runtime uses the same time (`rutis-host` passes it to its runtimes in the `RUTIS_SHUTDOWN_TIMEOUT` environment variable; 10 seconds without it). A cleanup not finished by then, and those after it, are not waited for: the runtime writes a line saying so on stderr and exits with status 1. So work that must happen should not rely on cleanup alone, and a cleanup should not wait without end. In Node and Bun, a cleanup that blocks synchronously (an endless loop) holds the thread, and the deadline cannot interrupt it.

## Managing plugins with `loader`

A host using `rutis-host` or `LoaderPlugin` provides a service `loader`: with it a plugin adds a row, loads a row's code again, removes a row and reads the current state, to the same effect as writing and changing rows in `rutis.json` by hand. It is an ordinary service: `inject: ["loader"]`, then `ctx.use("loader")`; any plugin in any language can use it, and the platform adds no permission or limit (to keep a plugin from managing plugins, do not add it). Design: [The host writes its own plugins](../design-host-writes-plugins-2026-10-27.en.md).

| Method | Arguments | Returns |
| --- | --- | --- |
| `create(options, parent?, position?)` | `options`: `{id?, name, config?, disabled?, inject?}`, written as a row of `rutis.json` is | the row's id; without `id`, a generated one of 8 hex digits, such as `"3f2a91c0"` |
| `reload(id)` | a row's id | nothing. Loads the row's code again |
| `remove(id)` | a row's id | nothing. Unloads the row and removes it |
| `status()` | none | the current state, the same that `rutis-host status` reads: `{"rows": [{"id": "weather", "state": "running"}, …], "services": [{"name": "weather", "provider": "weather"}, …], "calls": […]}` |

`create` is all or nothing: a new row that cannot start (its module not found, a syntax error, its config not matching the schema, `apply` throwing) is not left behind. On `reload`, when the new code cannot load (a syntax or import error), the previous version keeps running; when the new code fails in `apply`, the row runs the new code and fails with that error until it is fixed and reloaded again (as [`rutis-host dev`](rutis-host.en.md#developing-a-plugin-with-dev) reloads).

**When to call it.** The methods of `loader` are called after `apply` returns: in a service method, in an event. A row calling `create`, `reload` or `remove` in its own `apply` waits until that `apply` ends, so it hangs, and nothing reports it. To mount child plugins in `apply`, use `ctx.plugin` (Rust; Cordis plugins in Node): they belong to the row and are unloaded with it.

Python:

```python
from rutis import define_plugin


class Maker:
    def __init__(self, loader):
        self.loader = loader

    async def step(self):
        row = await self.loader.create({"id": "weather", "name": "py:weather"})
        await self.loader.reload(row)
        snapshot = await self.loader.status()
        await self.loader.remove(row)


def apply(ctx, config):
    ctx.provide("maker", Maker(ctx.use("loader")))


plugin = define_plugin(apply, name="maker", inject=["loader"])
```

TypeScript:

```ts
import { definePlugin } from '@arcships/rutis'

interface Loader {
  create(options: { id?: string; name: string; config?: unknown }): Promise<string>
  reload(id: string): Promise<void>
  remove(id: string): Promise<void>
  status(): Promise<{ rows: { id: string; state: string }[] }>
}

export default definePlugin({
  name: 'maker',
  inject: ['loader'],
  apply(ctx) {
    const loader = ctx.use<Loader>('loader')
    ctx.provide('maker', {
      step: async () => {
        const row = await loader.create({ name: 'py:weather' })
        await loader.remove(row)
      },
    })
  },
})
```

Go (`Inject: []string{"loader"}`):

```go
var loader struct {
	Create func(ctx context.Context, options map[string]any) (string, error)
	Reload func(ctx context.Context, id string) error
	Status func(ctx context.Context) (map[string]any, error)
	Remove func(ctx context.Context, id string) error
}
if err := ctx.Use("loader", &loader); err != nil {
	return err
}
// In a service method:
row, err := loader.Create(ctx, map[string]any{"name": "py:weather"})
```

**Errors.** Each case throws an error: its `name` is `"Error"` (`rutis.RemoteError` in Python, `*rutis.RemoteError` in Go), its `code` is `"Error"`, its `data` is `{"row": <row id>}` (the generated id when none was given), and its `message` is the text rutis prints for people in the terminal:

| Cause | `message` | The row |
| --- | --- | --- |
| `create`, `weather.py` has a syntax error | `weather: cannot load weather.py: SyntaxError: expected ':' (weather.py:3)` | not left |
| `create`, `apply` throws | `weather: KeyError: 'units'` | taken back, not left |
| `create`, the config does not match the schema | `weather: config: units should be "metric" or "imperial", got "kelvin"` | not left |
| `create`, no such module | `weather: cannot load "py:wether": ModuleNotFoundError: No module named 'wether'` | not left |
| `create`, the `id` exists | `a row weather already exists` | the existing row unchanged |
| `reload`, a syntax error | `weather: cannot reload: cannot load weather.py: SyntaxError: expected ':' (weather.py:3); the previous version keeps running` | the previous version keeps running |
| `reload`, `apply` throws | `weather: cannot reload: KeyError: 'units'` | runs the new code and fails |
| `reload` / `remove` of a row that does not exist | `no row wether` | — |
| `create` / `remove`, the host keeps no editable configuration | `this host keeps no editable configuration, so rows cannot be added or removed` | — |
| `create` took effect but was not stored | `weather: added, but rutis.json could not be written: <the system's reason>` (`removed` for a removal) | runs; gone after a restart of the host |

**Where changes are kept.** `rutis-host run` writes rows added and removed back into `rutis.json` (only `rows` changes; see [Managing plugins with `loader`](rutis-host.en.md#managing-plugins-with-loader)); `rutis-host dev` and an embedding host without a `Persist` keep no editable configuration: `create` and `remove` get the error above, `reload` and `status` work as usual.

**The name `loader` taken.** A service name has one provider, so another row also providing a service named `loader` fails with the error for a name already provided: `service loader is already provided by …: row <row> cannot provide it too`. Rename that row's service.

## Plugins in one runtime share its process

The rows of one runtime (`node`, `py`, `bun`) run in one process: they share memory, global state and the event loop, and a plugin blocking the event loop stops the others too. To isolate a plugin, put it in another runtime instance. A host can put a plugin in a process of its own (see [Build a host open to plugins](open-host.en.md#3-keep-plugins-in-check)).

When a service method holds the runtime for 5 seconds in one go (fixed; from 0.18, #678), the runtime says once on stderr which row's which method it is, that the other rows wait meanwhile, and what to change, for example:

```text
runtime py: forecast.tomorrow of row weather has held the runtime for 5 s, and the other rows wait meanwhile: it blocks the event loop (a def method, or a blocking call in an async def one); make it async def and await what it waits for (asyncio.sleep, an async client, asyncio.to_thread for a blocking call), and it will not hold the runtime
```

In Python every stretch of a method is watched: a whole `def` method, and each stretch of an `async def` method between two `await`s (a `time.sleep` or `requests.get` in it, say); make it `async def` and `await` what it waits for (`asyncio.sleep`, an async client, `asyncio.to_thread` for a blocking call). In Node and Bun, the stretch from a method's start to its first `await` is watched (`readFileSync`, `execSync`, a busy loop), not blocking after an `await`; use asynchronous calls and `await` them, or move the computation to a worker thread. Blocking in `apply` or a listener is not said. It is said once and only said: the method runs on to its end. Under `rutis-host dev --debug` nothing is said (a runtime stopped at a breakpoint may stay there as long as you like). The synchronous methods of Go, Swift and Kotlin do not hold the whole runtime, and have no such line.

An error a plugin leaves unhandled (an unhandled promise rejection or an exception thrown in a timer in JavaScript, an exception nobody retrieves from a Python task) does not end the process (from 0.18, #688):

- **Where the row is traced, that row fails**, saying what and where, for example `weather: failed: unhandled rejection: Error: no region`, with the plugin's own frames under it (`at … (file:///…/weather.mjs:12:9)`); the error's own `code` and `data` go with it. A row still loading fails to load (`failed in apply: unhandled rejection: …`). The other rows of the runtime run on. A row's `apply`, service methods, listeners and the functions it hands over run with the row in their context, and so do the tasks and promises they start. When a row calls a service of another row of the same runtime directly and the innermost plugin file on the error's stack is the other row's, which of the two left it unhandled cannot be told (the stack says where it was thrown, not who dropped it): it is reported as from an unknown row, and no row is charged. Python always traces so (read from the task's context); an exception in a task the plugin still holds is not unhandled, and is not reported. Node traces only under `rutis-host dev`: it records the row each promise belongs to (`async_hooks`), which costs something on every promise.
- **Where it is not, the source is unknown**: `runtime node: an unhandled error from an unknown row: <stack>` (only the plugin's own frames, or the whole stack when none is the plugin's), and no row fails. So it is for Node under `rutis-host run` and always for Bun, and in Python for a task or callback that runs in no row. An exception a Python thread leaves unhandled is printed by Python's own rules; the thread ends and the process goes on. Run under `rutis-host dev` to find the row.
- **A plugin ending its process** (`process.exit`, `os._exit`): the process ends as before, the rows of the runtime stop and wait for it (see [Lifecycle](#lifecycle)), and the runtime first writes `runtime node: row weather called process.exit(1)`. The row is the one running for Node under `dev` and for Python, else the one whose plugin file is on the stack (named only when exactly one row matches); Bun drops the plugin's frame in a tail call (`() => process.exit(1)`), and then names no row.
- The Go, Swift and Kotlin runtimes are unchanged: a panic or an uncaught exception follows the language's own rules.

## Output

What a plugin writes to standard output and standard error (`console.log`, `print`, `fmt.Println`) goes to the host's terminal, each to its own stream. Under `rutis-host dev` each line starts with the name of the row that wrote it (from 0.18, #678):

```text
[weather] fetching forecast for Berlin
```

- The rows of a Node, Bun or Python runtime share its process, so the runtime adds the name of the row running: a row's `apply` and cleanups, its service methods, its listeners and the functions it hands over, and the asynchronous work they start (the async context in Node and Bun, contextvars in Python). Output whose row cannot be told starts with the runtime's name (`[py] `): a thread a Python plugin started itself, and code that runs in no row. What is written to the file descriptor itself (a child process, `os.write`, a native module) the runtime does not see; it is passed on as it is.
- Go and Swift run one plugin per process: the host adds the runtime's name as it reads the output (`[go-weather] `); the SDKs are unchanged.
- Bun's `console` writes to the file descriptors itself: under the prefix the runtime puts in a `console` that writes through `process.stdout` / `process.stderr`, which prints objects in Node's format on one line (`{ a: 1 }`), not Bun's usual multi-line one.
- `rutis-host run` adds nothing by default (plugins' output in production stays as written); `--prefix-output` / `--no-prefix-output` turn it on or off, see [rutis-host](rutis-host.en.md#output).

Python's `logging` is unchanged: by default it shows WARNING and above, which is Python's own rule. To see INFO, configure logging in the plugin:

```python
import logging

logging.basicConfig(level=logging.INFO)
```

## Passing values

A plugin and the services it uses may run in different processes. Across processes:

| Value | How it is passed |
| --- | --- |
| `null` / `None`, booleans, numbers, strings | Copied |
| Arrays / lists, plain objects / dictionaries with string keys | Recursively copied |
| Python dataclasses | Copied as dictionaries |
| Dates and times, binary data, maps, sets, big integers, decimals, UUIDs | Copied; each language receives its own types, see [the ten values](#the-ten-values) below |
| `Error` / exceptions | Passed as a value: copied as data `{ name, message }` (in TypeScript also `stack`). Thrown: the other side receives an error with the same name and message (in TypeScript a built-in error class keeps its class, any other is an `Error`; in Python a `rutis.RemoteError` whose `name` is the original class name; in Go a `*rutis.RemoteError`). An error thrown in any language may also carry a code and data: in TypeScript a string `code` property and a `data` property, in Python a str `code` attribute and a `data` attribute, in Go the methods `Code() string` and `Data() any`, in Rust [`rutis_bridge::exports::failure`](rust-host.en.md#write-a-hostdispatch-by-hand) (what the `Err(e)` of an [exported Rust service](rust-host.en.md#exported-services-reference) becomes). The receiver reads them as `code` and `data` (Go: `Code`, `Data`; Rust: `Error::code()`, `Error::data()`), and they pass on when it throws the error again. `data` crosses as data (big integers as bigints); data that cannot cross is left out, and the rest of the error still crosses. Other attributes are not passed |
| Functions, Promises / coroutines | Passed by reference: the other side calls or awaits the original |
| Objects with methods, class instances | Passed by reference, to every language: the other side receives a proxy whose methods call the original object. Which values count as objects and how each language receives them: see [Objects with methods](#objects-with-methods) below |
| Plugin definitions (the result of `definePlugin`; Cordis plugins in Node; the result of Python's `define_plugin`; Go's `*rutis.Definition`; the result of `Rutis.define` in Swift and Kotlin) | By reference, going back only to the runtime that defined them: a Rust service whose parameter or return type is `rutis::Definition` receives a plugin definition it can mount; see [Plugin definitions](#plugin-definitions) below. Node, Bun, Python, Go, Swift and Kotlin can hand them over |
| `undefined` | Kept only as a whole argument or a whole result: as an argument it means "not given" (Python receives one argument fewer or `None`, Go the zero value); as a result JS receives `undefined` and other languages their empty value. Inside arrays, objects, Maps and Sets it becomes `null` at any depth (`None` in Python) |
| Empty slots of an array (`[1, , 3]`) | The same as `undefined` inside an array: `null` |
| Go structs, slices, maps | Copied: struct fields and the items of slices and maps each follow the rules of this section, with field names by their `json` tags; a struct is always data, `rutis.Ref(v)` passes an object by reference. A `map[string]T` is an object, a map whose keys are not strings is a map, and a `map[K]struct{}` is a set |
| Integers beyond ±(2^53−1) | Cross as big integers automatically, see [the ten values](#the-ten-values) |
| Symbols, cyclic references, `NaN` / `Infinity`, other JS typed arrays (such as `Int16Array`), Python `time` with a time zone, Go channels | Cannot cross processes (the call fails) |
| Values nested more than 64 levels deep | Cannot cross processes (the call fails, see below) |
| A call or result over 16 MiB once encoded | Cannot cross processes (the call fails, see below) |

A value that cannot cross fails **only that call**: the error names the method, which argument (or the result) and why; for example, when an argument is too large once encoded, the call gets `store.put: argument 1 cannot be sent: the message would be 30889096 bytes, over the limit of 16777216 bytes (16 MiB)`. The session, the runtime, other calls on the same session and other rows are not affected. The error's name depends on which side refused the value and its language (`BindingError` from the host, `RangeError` or `TypeError` from Node and Bun, `ValueError` from Python, `Error` from Go, Swift and Kotlin) and is not guaranteed: to tell which failure it is, check the error code: a value that cannot cross is always `bad-value`, with `data` saying whether it was an argument or the result (`{part, argument?, limit?}`); not the name or the text.

Nesting depth: a value nests at most **64 levels**. Each array / list / tuple, object / dictionary (dataclasses included), map, set, and Go slice, map or struct is a level, whether or not it holds references; an `Error` / exception passed as a value is a level too. A call's argument list is not a level: each argument may nest 64 levels; a result may nest 64 levels. The side that **would send** a deeper value refuses it before sending, failing only that call, with an error naming the limit (`value nested deeper than 64 levels`):

- as an argument: the caller's call fails (in Rust with `Error::Value`) and the far end does not run it;
- as a result: the caller receives the error the callee throws;
- the session goes on, and other calls on it (those running at the same time included) are not affected.

All five runtimes (Rust, Node, Bun, Python, Go) check this limit when sending; under the test tools the plugin runs in these runtimes and is checked the same. The limit binds the sender: the receiving Rust side leaves a margin for a far end that does not keep to it (an older version, say) and reads frames up to 208 levels deep on the wire, so a value **received** from such a far end may nest deeper than 64 levels (up to about 100 levels of arrays holding references, about 206 of data); that margin is not guaranteed and plugins must not rely on it. When such a far end sends a frame nested too deep for the Rust side to read: if the frame shows which call it belongs to, only that call fails (the far end's call receives an error; a reply to this side's call fails that call), and the references in it are released only when the session ends; if it does not, the session ends, saying why. See [#279](https://github.com/arcships/rutis/issues/279).

Message size: a call (with its arguments) or a result is at most **16 MiB** once encoded (16777216 bytes, the default limit of the local channels and of WebSocket). The side that **would send** a larger message refuses it before sending, failing only that call, with an error naming the method, which argument (the largest) or the result, the size and the limit (`store.put: argument 1 cannot be sent: the message would be 30889096 bytes, over the limit of 16777216 bytes (16 MiB)`): as an argument, the caller's call fails and the far end does not run it; as a result, the caller receives that error; the session goes on and other calls are not affected. Rust, Node, Bun, Python, Go, Swift and Kotlin all check before sending. A far end of an earlier version may still send a message over the limit: the receiver closes that connection as before, which ends that session (a local runtime exits with it); other sessions are not affected. Send large data in pieces, or send where it is (a path, an address).

Numbers have the limits below. They describe current behaviour and are **not guaranteed** to stay this way (explicitly not guaranteed); a plugin that depends on them must handle them itself:

- In JavaScript (Node, Bun) numbers are IEEE 754 doubles: a JS `number` beyond ±(2^53−1) is already inexact and is sent as it is; use `bigint` to be exact. `-0` becomes `0`; an integral float (Python's `2.0`) arrives in JS as `2` and leaves again as an integer.
- Python keeps `-0.0`.
- NaN and ±Infinity cannot cross.

Values are not copied within one process. Code that mutates an object after passing it may work there but fail across processes. Under the test tools, the services a test gives are in the same runtime as the plugin and are not copied either, so a test does not catch this.

### Objects with methods

The value being sent decides whether it is copied or goes by reference, whatever the other side expects:

| Language | Copied (data) | By reference (the other side gets a proxy) |
| --- | --- | --- |
| JS (Node, Bun) | Plain objects (prototype `Object.prototype` or `null`) with no function-valued property; arrays | Class instances; plain objects with function-valued properties (`{ get() {}, set() {} }`) |
| Python | `dict`, `list`, `tuple`, dataclasses (copied as dicts) | Instances of other classes (instances with `__call__` go as functions) |
| Go | Structs, slices, maps | `rutis.Ref(v)` |
| Swift | `Codable` structs and classes | Instances of `@RutisService` types |
| Kotlin | `@Serializable` classes | Instances of `@RutisService` classes |
| Rust | serde values | Does not send its own objects; forwards objects of other processes |

Objects inside arrays and dictionaries each go by reference too. A received object is the same kind of thing as the service proxy `ctx.use` returns: each method is an asynchronous call on the original object, and there are no data fields; to get data, have a method return it.

| Language | How to receive | What you get |
| --- | --- | --- |
| JS (Node, Bun) | Nothing to declare | A proxy: `await cache.get('a')` |
| Python | Nothing to declare | A proxy: `await cache.get("a")` |
| Go | A parameter whose type is a struct of function fields (as for a `ctx.Use` target); or `any` | Function fields that call the methods (blocking); `any` receives `*advanced.Service` |
| Swift | A parameter typed with a `@RutisClient` protocol (`any Cache`); or `Any` | The protocol's proxy; `Any` receives `Rutis.Advanced.Client` |
| Kotlin | A parameter typed with a `@RutisClient` interface; or `Any?` | The interface's proxy; `Any?` receives `RemoteObject` (`arcships.rutis.advanced`) |
| Rust | Hand-written `HostDispatch` | `rutis_bridge::session::Reference` (`call_method`, `call_method_async`), `ObjectRef` in Cordis bindings; a `#[rutis::service]` parameter does not take objects (`bad-argument`) |

- One object passed several times is one proxy on the far end; a proxy passed back to its own process is the original object there, and passed to a third process it is still the same object.
- Calling a method the object lacks fails that call with `no-method`: `Cache has no method gett (methods: get, set, size)`, with `data` `{object, method, methods}`.
- A parameter whose declared type cannot take an object (data in Go, Swift and Kotlin) fails the call with `bad-argument`, saying how to receive one.
- Lifetime is that of a function reference: the sender lets the object go once the receiver no longer holds the proxy, or the receiver's process exits. An object does not follow its row: after the row that passed it unloads, it can still be called.
- Once the sender's runtime has exited or disconnected, calls on its objects fail with `peer-gone` (`a call on an object from py failed: runtime py exited`), and only those calls fail.

#### Handing over and receiving objects in each language

**TypeScript, Bun**: a class instance, or a plain object with function-valued properties, is handed over by reference. An object another process passes here is a proxy:

```ts
const cache = await ctx.use('store').cache()
await cache.set('a', 1)          // every method is an asynchronous call of the far end's object
const value = await cache.get('a')
```

- Every name read on a proxy is a method (but `then`, `toJSON`, `toString`, `valueOf` and symbols: so awaiting a proxy gives the proxy itself, and the object's own methods of these names cannot be called through it). `console.log` shows `[remote object]`.
- `undefined` inside an array or object arrives as `null`; only a whole argument or a whole result keeps it.

**Python**: `dict`, `list`, `tuple` and dataclasses are copied; an instance of any other class is handed over by reference:

```python
cache = await ctx.use("store").cache()
await cache.set("a", 1)          # every method is async, a call of the far end's object
value = await cache.get("a")
```

- Every public name on a proxy is a method (as with `ctx.use`). Its `repr` is `<remote object>`. One object passed several times is one proxy on the far end (`is`).
- `undefined` (sent by JavaScript) inside a list or dict is `None` here, at any depth.

**Go**: structs, slices and maps are data and are copied; to hand an object over by reference, write `rutis.Ref(v)`. For an object another process passes, write the parameter's type as a struct of function fields (as for a `ctx.Use` target, the field names matching the methods by the same rule), or as `any` or `*advanced.Service`:

```go
type Cache struct {
	Get func(ctx context.Context, key string) (any, error)
	Set func(ctx context.Context, key string, value any) error
}

func (w *Warmer) Warm(ctx context.Context, cache Cache) (any, error) {
	if err := cache.Set(ctx, "a", 1); err != nil {
		return nil, err
	}
	return cache.Get(ctx, "a")
}
```

- A function field's call blocks; its first parameter is a `context.Context`. A struct with a field that is not a function, or a parameter of another type, fails the call with `bad-argument`: `warmer.warm: argument 1 should be a Cache, got an object (an object is received only as a struct of function fields or any)`.
- `any` receives a `*advanced.Service`, the type `ctx.Use` gives: `Call(ctx, method, result, args...)`. An object has no method table, so `Methods()` returns `nil`, and a method it lacks is answered `no-method` by the object's own process.
- Only a `*advanced.Service` keeps its identity: one object received several times is one `*advanced.Service`, and handed back to its process it is the original there. A struct of function fields is data: handed over, each field becomes a new function reference; to hand over the object itself, make the parameter `any`.
- An object's methods may be called concurrently.

**Swift**: `Codable` structs and classes are copied; an instance of a `@RutisService` type is handed over by reference. For an object another process passes, type the parameter with a `@RutisClient` protocol, or as `Any`:

```swift
/// How to use an object another process passes: declared at file level.
@RutisClient
protocol Cache {
    func get(_ key: String) async throws -> Int?
    func set(_ key: String, _ value: Int) async throws
}

@RutisService
final class Warmer: @unchecked Sendable {
    func warm(_ cache: any Cache) async throws -> Int? {
        try await cache.set("a", 1)          // every method is async, a call of the far end's object
        return try await cache.get("a")
    }

    /// When you do not know what arrives, take Any: an object is a Rutis.Advanced.Client, called by name.
    func peek(_ value: Any) async throws -> Int? {
        guard let cache = value as? Rutis.Advanced.Client else { return nil }
        return try await cache.call("get", values: ["a"], as: Int?.self)
    }
}
```

- A parameter typed with a `@RutisClient` protocol (`any Cache`) gets that protocol's proxy, the same kind of proxy as `ctx.use("cache", CacheProxy.self)`; so does the result of a `@RutisClient` method. A parameter typed `Any` gets a `Rutis.Advanced.Client`: `try await client.call("get", [Rutis._Generated.encode("a")], as: Int?.self)` or `call("get", values: ["a"], as: Int?.self)`. Objects inside arrays and dictionaries, and in event arguments, arrive through `Any` as `Rutis.Advanced.Client`; `[any Cache]` is not supported in 0.18.
- Only a `@RutisClient` protocol declared at file level receives objects: the `rutisObjectProxy` overload the macro generates for it is visible to Swift only at file scope. A `@RutisClient` protocol nested in another type still works with `ctx.use`, but a parameter typed with it does not take an object (`bad-argument`). An optional parameter or result (`(any Cache)?`) does not take an object in 0.18 either; write `any Cache`.
- A parameter of a data type (a `Codable` type, `String` and so on) fails the call with `bad-argument`: `warmer.place: argument 1 should be a Spot, got an object (an object is received only as a @RutisClient protocol or Any)`.
- A received proxy or `Rutis.Advanced.Client` passed on sends the original object.

Swift apps, Kotlin apps and Rust: [Swift app](swift-app.en.md#objects-with-methods), [Kotlin app](kotlin-app.en.md#objects-with-methods), [Rust plugin](rust-plugin.en.md#serve-a-service-to-other-languages).

#### The same process versus across processes

- **In the same process** (rows of the same runtime) the receiver gets the object itself: methods stay synchronous and fields stay readable.
- **Across processes** there are only methods, and every call is asynchronous. Code that reads a field or relies on a synchronous result works in one process and fails across processes; under the test tools, the services a test gives are in the plugin's runtime, so the plugin gets the object itself and a test does not catch this.
- **Idle stop**: an on-demand runtime (Go, Swift) stops `idle` seconds after its last row unloads. Its objects held elsewhere do not keep it running. After it stops, calls on them fail with `peer-gone`: `a call on an object from go-netkit failed: runtime go-netkit stopped because it was idle`. To keep them usable, keep the row that passed them loaded, or make the runtime `eager`.
- **Identity**: one object received several times is one proxy, and handed back home it is the original object, in every language (Go: only through `*advanced.Service`).

### The ten values

| Value | JS (Node, Bun) | Python | Go | Kotlin | Rust (`rutis-bridge`) | Swift |
| --- | --- | --- | --- | --- | --- | --- |
| Point in time | `Date` | `datetime` with a time zone (received in UTC) | `time.Time` (received in UTC) | `java.time.Instant`, `kotlin.time.Instant` (received as `java.time.Instant`) | `Instant` | `Date` (to the microsecond) |
| Local date and time | String `"2026-10-15T14:30:05"` | `datetime` without a time zone | `advanced.DateTime` | `LocalDateTime` | `DateTime` | `Rutis.Advanced.DateTime` |
| Date | String `"2026-10-15"` | `date` | `advanced.Date` | `LocalDate` | `Date` | `Rutis.Advanced.CalendarDate` |
| Time of day | String `"14:30:05"` | `time` without a time zone | `advanced.TimeOfDay` | `LocalTime` | `TimeOfDay` | `Rutis.Advanced.TimeOfDay` |
| Binary data | `Uint8Array` (`Buffer`, `ArrayBuffer`, `DataView` can be sent too) | `bytes` (`bytearray`, `memoryview` can be sent too) | `[]byte` | `ByteArray` | `Vec<u8>` (`Bytes` in generated code) | `Data` |
| Map | `Map` | `dict` with a key that is not a string (received as a subclass of `dict`) | A map whose keys are not strings (`map[any]any` decoded into `any`) | a `Map` whose keys are not strings | `Vec<(Value, Value)>` (`ValueMap` in generated code) | A `Dictionary` whose keys are not `String` (`[AnyHashable: Any?]` decoded into `Any`) |
| Set | `Set` | `set` (`frozenset` can be sent too) | `map[K]struct{}`, or a slice (a slice decoded into `any`) | `Set` | `Vec<Value>` (`ValueSet` in generated code) | `Set` |
| Big integer | `bigint` | `int` (received as a subclass of `int`) | `*big.Int`, or an integer type that holds it | `BigInteger`, or an integer type that holds it | `BigInt`, or an integer type that holds it | `Rutis.Advanced.BigInt`, or an integer type that holds it |
| Decimal | String `"12.30"` | `Decimal` | `advanced.Decimal` | `BigDecimal` (its scale kept) | `Decimal` | `Decimal` (38 digits, no trailing zeros; declare `String` for the text) |
| UUID | String (lowercase) | `UUID` | `advanced.UUID` | `java.util.UUID`, `kotlin.uuid.Uuid` | `Uuid` | `UUID` |

- Plugins write their language's native code; rutis does the conversion. A received big integer or map is, in Python, a subclass of `int` or `dict`, used as usual; passed on as it is, it is still a big integer or map.
- JS has no type for local date-times, dates, times of day, decimals or UUIDs: they arrive as plain strings, their content complete, and `===` works as usual. Passed on from JS as they are, such values lose their type (a `date` sent by Python comes back through JS as a `str`). To create one of these five values from nothing in JS, or to pass one on as its type, use the constructors of `@arcships/rutis/advanced`: `rutis.datetime('2026-10-15T14:30')`, `rutis.date('2026-10-15')`, `rutis.time('14:30')`, `rutis.decimal('1.10')`, `rutis.uuid('…')`; a plain string is still a string. Go can also decode these five values into a `string`.
- Integers: an integer beyond ±(2^53−1) (absolute value above 9007199254740991; Python `int`, Go `int64` / `uint64`, Rust integers) crosses as a big integer automatically, with nothing special to write; JS receives it as a `bigint`, and within range still as a `number`. A received big integer decodes into a declared integer type that holds it; one that does not hold it fails the call. `bool` is not affected.
- Times: years 0001–9999. A local date and time has no time zone and crosses as it is, with no zone guessed and no conversion to a point in time. A point in time is truncated toward negative infinity to the receiver's precision: milliseconds in JS, microseconds in Python, nanoseconds in Go and Rust; Python also truncates local date-times and times of day to the microsecond.
- A decimal keeps its trailing zeros (`1.10` is not `1.1`) and uses no exponent; NaN and infinities cannot cross. Big integers and decimals have at most 4300 digits.
- Map keys and set elements can only be strings, finite numbers, booleans, big integers, local date-times, dates, times of day, decimals and UUIDs; points in time, binary data, `null`, arrays, objects, maps, sets and references cannot, and the sender refuses them before sending. When two keys become equal after conversion on the receiving side (a JS `Map` with keys `1` and `true` arriving in Python, say), the call fails: an entry would be lost. A JS `Map` with string keys still crosses as a map; a plain Python `dict` with string keys crosses as an object.
- Each value is sent only to a far end that declared it. When the far end does not take it (a runtime older than 0.11, say, integers beyond ±(2^53−1) included), only that call fails, with an error naming the value, and the session and other calls go on. Event arguments and results carry them as calls do, between nodes too.

### Plugin definitions

A plugin is a value: a plugin definition can be handed over as an ordinary argument or return value of a service call, and the receiver mounts it. Each mount is one copy, and each copy can have its own config. So a plugin can hand what its `apply` prepared to parts mounted elsewhere, writing nothing specific to rutis ([design](../design-plugin-values-2026-10-25.en.md)).

```js
export default definePlugin({
  inject: ['parts'],
  async apply(ctx) {
    const pool = await openPool()
    await ctx.use('parts').register('session', definePlugin({
      inject: ['history'],
      apply(ctx, config) { /* use pool directly */ },
    }))
  },
})
```

- What can be handed over: the result of `definePlugin` in Node and Bun; Cordis plugins in Node (functions, classes, objects with `apply`); the result of Python's `define_plugin`; the result of Go's `rutis.Define` (`*rutis.Definition`, the type a client's function field takes); the result of `Rutis.define` in Swift and Kotlin (a client's parameter type is `Rutis.Definition`, `Plugin`; see [handing over plugin definitions in each language](#handing-over-plugin-definitions-in-each-language) below, [Kotlin application](kotlin-app.en.md#2-writing-a-plugin)).
- A plugin definition an application that dials in hands over (`RutisApp` in Swift and Kotlin) runs in that application; when the application's connection ends, every copy mounted from it ends with it.
- The receiver is Rust: a parameter or return type `rutis::Definition`, mounted with `ctx.plugin(def.at(&ctx, config)?)`; see [Rust host](rust-host.en.md#receive-plugin-definitions). Other parameter types are unchanged: a data parameter that receives a plugin definition fails the call.
- Each copy runs in the runtime that defined it, isolated by the instance that mounts it, and gated on its `inject`.
- A plugin definition belongs to the current generation of the row that handed it over: when that row unloads, reloads or ends, every copy mounted from it ends first, and is cleaned up, before the row unloads. Mounting it afterwards fails. When the runtime's process exits, its copies end too.
- A plugin definition only goes back to the runtime that defined it; it cannot be handed on to a third runtime (that call fails).

#### Handing over plugin definitions in each language

Python: hand the result of `define_plugin(...)` over as an ordinary argument; each copy runs in this process:

```python
async def apply(ctx, config):
    pool = await open_pool()

    def session(ctx, config):
        ctx.provide("gitStatus", GitStatus(pool, config["depth"]))

    ctx.use("parts").register("session", define_plugin(session, inject=["history"]))
```

Go: the result of `rutis.Define(...)` is a `*rutis.Definition`, the type a function field's parameter takes; a definition only handed over may leave out `Name`:

```go
var parts struct {
	Register func(ctx context.Context, scope string, def *rutis.Definition) error
}
if err := ctx.Use("parts", &parts); err != nil {
	return err
}
return parts.Register(ctx, "session", rutis.Define(rutis.Plugin[SessionConfig]{
	Inject: []string{"history"},
	Apply:  func(ctx *rutis.Ctx, config SessionConfig) error { /* use pool directly */ return nil },
}))
```

Swift: in a `@RutisClient` protocol, the parameter or result type is `Rutis.Definition`:

```swift
@RutisClient
protocol Parts {
    func register(_ scope: String, _ part: Rutis.Definition) async throws
}

let git = Rutis.define(Plugin(name: "git", inject: ["parts"]) { ctx in
    let pool = try await openPool()
    let status = Rutis.define(Plugin(name: "git_status", inject: ["history"], config: StatusConfig.self) { ctx, config in
        ctx.provide("gitStatus", GitStatus(pool: pool, depth: config.depth))
    })
    try await ctx.use("parts", PartsProxy.self).register("session", status)
})
```

The parameter or result must be a `Rutis.Definition` (or `Rutis.Definition?`) itself. Plugin definitions inside an array, a dictionary or a struct (such as `[Rutis.Definition]`) are not supported: make one call per definition.

Rust: a `Plugin` is an instance: its config is in its fields, and handing it to `ctx.plugin` uses it up. To hand a plugin elsewhere, to be mounted there many times (one per session, say), use `rutis::Definition`: `Definition::new(ctx, build)` makes a definition, whose `build` gets the `ctx` where a copy mounts and that copy's config, and returns a `Plugin`; the mounting side writes `ctx.plugin(def.at(&ctx, config)?)`.

```rust
use std::sync::{Arc, Mutex};
use rutis::{Ctx, Definition, Plugin, Result};

/// One per session; `depth` comes from its own config.
struct Part {
    depth: u32,
}

impl Plugin for Part {
    type Deps = ();

    async fn apply(&self, _: &Ctx, _: ()) -> Result {
        println!("depth {}", self.depth);
        Ok(())
    }
}

/// Hands a definition of `Part` to whoever mounts parts (here just a list).
struct Git {
    parts: Arc<Mutex<Vec<Definition>>>,
}

impl Plugin for Git {
    type Deps = ();

    async fn apply(&self, ctx: &Ctx, _: ()) -> Result {
        let pool = Arc::new(String::from("pool")); // prepared in apply
        let definition = Definition::new(ctx, move |_at: &Ctx, depth: u32| {
            let _pool = pool.clone();
            Ok(Part { depth })
        });
        self.parts.lock().unwrap().push(definition);
        Ok(())
    }
}

/// The mounting side: each mount is one copy, with its own config.
fn mount(at: &Ctx, definition: &Definition) -> Result {
    at.plugin(definition.at(at, 3u32)?);
    Ok(())
}
# fn main() {}
```

- A definition made in Rust belongs to the current generation of the plugin that made it: when that plugin unloads, restarts or ends, every copy mounted from it ends first, and is cleaned up, before the plugin's own cleanups run; then `at` fails and `ended()` completes.
- It can only be mounted inside the instance of the plugin that made it, or deeper.
- A plugin definition another language hands over is a `Definition` too, mounted the same way. A definition made in Rust cannot be handed to another language.

In tests, a definition a test double receives can be loaded as it is; see each language's test tool: [Python](#the-python-test-tool), [Go](#the-go-test-tool), [Swift](#the-swift-test-tool).

### Where the test tool differs from a real runtime

Each language's test tool (`@arcships/rutis/testing`, `rutis.testing`, Go's `rutistest`, Swift's and Kotlin's `RutisTest`) runs the plugin as a row of a real host: `rutis-host test` (on `PATH`, or at `RUTIS_HOST`) starts a host, and the plugin runs in its language's real runtime. The host checks the config and fills its defaults, starts the plugin once the services it injects are there, dispatches every event, and unloads by its rules; no switch turns this off. These are the only known differences:

| Difference | Why |
| --- | --- |
| The test's services, `on` and emitting belong to a test plugin in the same runtime: when the plugin calls a service the test gave, it gets the object itself, and values are not copied | That is how the test tool drives the plugin; behaviour across processes is checked by every runtime against the same expectations files ([`weather.expected.json`](../../crates/rutis-bridge/src/runtime/testing/weather.expected.json), [events expectations file](../../crates/rutis-bridge/src/runtime/testing/events.expected.json)) |
| The Python, Go, Swift and Kotlin runtimes are in the test's process: a plugin that ends its process (`os._exit` and the like) ends the test | These languages call the plugin under test from the test's process (`rutis-host test --channel`) |
| Local runtimes only, no remote nodes or devices; tests on a device use the plugins an application installs itself | The test tools start local runtimes only |

## Test tools

Each language's plugin guide shows loading a plugin with its test tool, getting a service, listening to the events the plugin emits, and unloading. This section is the rest of the interface. Where a test tool differs from a real runtime is [above](#where-the-test-tool-differs-from-a-real-runtime).

### The TypeScript and Bun test tool

`@arcships/rutis/testing`, the same one under Bun. The plugin runs in the real runtime (`@arcships/rutis-runtime`, `@arcships/rutis-bun` under Bun), managed by a real host (`rutis-host test`, see [rutis-host](rutis-host.en.md)): the host checks the config and fills its defaults, gates the plugin on the services it injects and dispatches its events, and values between the plugin and the test cross by the rules between processes. A test process starts a host and a runtime at its first `load`; the tests after it share them, and only rows are unloaded between tests. The test's services, `on` and emitting are those of a test plugin next to the plugin, in the same runtime. It needs:

- `@arcships/rutis-runtime` (`@arcships/rutis-bun` under Bun) and `@arcships/rutis-host` as dev dependencies (a project `rutis-host new` made has them), or `RUTIS_HOST` naming a `rutis-host`;
- the plugin to be the default export of its module: the test tool has the runtime load it by its file. A plugin made with `definePlugin` inside the test file cannot be loaded: `load needs a plugin that its module exports as default`.

| Name | What it does |
| --- | --- |
| `load(plugin, { config, services })` | Loads one plugin, as the one row of a `host()` of its own; unloading it unloads that `host()` too |
| `host({ services })` | Plugins on the host, sharing its bus. `services` are the test's, given to every row that injects them |
| `h.load(plugin, { config, services, id, isolate })` | Loads another row on the `host()`; `services` are this row's only; `id` is the row's id (by default its module's file name); `isolate` is as in a row of `rutis.json` (`{ forecast: true }`, or a label) |
| The five methods on `h`, `h.filter`, `await h.reported()`, `h.unload()` | Emit from the test's plugin; the failures the host reported of these rows; unload every row, latest first, then withdraw the test's services |

The `t` that `load` and `h.load` return:

| Member | What it does |
| --- | --- |
| `t.service(name)` | The service the plugin provides, as rutis sees it: its declared methods, or, undeclared, the object's methods, all async |
| `t.methods(name)` | The methods rutis sees: `{ today: 'async' }` |
| `t.provided()` | The names of the services the plugin provides now |
| `t.on(name, listener, options)` | Listen as another plugin next to it does; see the language guides |
| `t.emit`, `t.parallel`, `t.serial`, `t.waterfall`, `t.bail` | Emit from the test's plugin |
| `t.filter(keep)` | The five methods, delivering only to the listeners `keep(descriptor)` keeps |
| `await t.reported()` | The failures the host reported of this row (listeners failing in an `emit`, filters): `{ line }`, where `line` is the line the host prints |
| `await t.update(config)` | Change the config as a host does |
| `await t.unload()` | Unload the row as a host does (withdraw its listeners and services, abort `ctx.signal`, wait for its running listeners, then run its cleanups, latest first), then withdraw the test's services and listeners of it |
| `t.row`, `t.host` | The row's id on the host (its module's file name, with `-2`, `-3`, … when taken); the `host()` it is on |

`PluginTestError`: what the test tool refuses: a load that fails (its text is the row's status, as the host prints it, such as `weather: config: city is required`), a plugin it cannot load.

### The Python test tool

`rutis.testing`:

`load` starts `rutis-host test` (the file `RUTIS_HOST` names, else `rutis-host` on `PATH`; the plugin project's dev dependency `rutis-host` installs it) and connects the Python runtime in the test's process to it, in the test's event loop. The plugin is a row of that host and runs as it does on any host: the host's loader checks its config and fills in the defaults, every event goes through the host, and the host unloads it. The services the test gives (`services`) and the test's own listeners (`t.on`) belong to the test's plugin `tester`, in the same runtime.

| Parameter of `load(plugin, *, …)` | What it does |
| --- | --- |
| `config` | The plugin's config; the host's loader fills in the defaults of the plugin's `config` declaration and checks it, and a config that does not match fails `load` with the row's state line (`weather: config: city is required`) |
| `services` | The services the plugin `inject`s, by name |
| `bus` | Share one host with the plugins on the same `Bus` (default: one of its own) |
| `id` | Its row's id; without one, the plugin's declared name, else its module's, with `-2`, `-3`… added while taken; an id given that is taken fails `load` in the host's words |
| `isolate` | The row's `isolate`, as in `rutis.json` (`{"notes": True}`, or a label for a service) |

| Member of `Loaded` (`async with load(...) as t`) | What it does |
| --- | --- |
| `t.service(name)` | The service the plugin provides, as rutis sees it: its declared methods, or, undeclared, the object's public methods, all async |
| `t.methods(name)` | The methods rutis sees: `{"today": "async"}` |
| `t.provided()` | The names of the services the plugin provides now |
| `t.on(name, listener, *, prepend=False, global_=False)` | Listen from the test's plugin: the plugin's emissions and the test's own reach it, its arguments sent by the host; returns the function that stops it, and without it, it stops when the plugin unloads |
| `t.emit`, `t.parallel`, `t.serial`, `t.waterfall`, `t.bail` | Emit from the plugin's row, through the host |
| `t.filter(predicate)` | The five methods, delivering only to the listeners `predicate(descriptor)` keeps |
| `await t.reported()` | What the host reported of this row (listeners failing in an `emit`, filters of its `emit`), each a `Reported` whose `line` is the host's line |
| `await t.update(config)` | The host changes the config: unload, then run again with the new config; a config that does not match fails in the host's words, and the row keeps its config |
| `await t.unload()` | The host unloads it: withdraw listeners and services, cancel and wait for running listeners, then run the cleanups, latest first (done when the `async with` block exits) |

`Bus()`: a host for tests; the plugins on the same `bus` are on one host and hear each other; it starts with the first `load` and stops when the last one ends. It has `await bus.reported()` (of the rows on the bus, row by row) and `emitter()` (emit as the test's plugin). `Reported`: one failure nobody received, with the field `line`, the line the host prints (`ticker: listener of tick failed: ValueError: boom`, then the stack). `PluginTestError`: the host refused to load the plugin or change its config (the text is the host's), the plugin does not provide the service asked for, or `rutis-host` cannot be started; a subclass of `AssertionError`.

A plugin definition a test double receives can be passed to `load` as it is.

### The Go test tool

`github.com/arcships/rutis/go/rutis/rutistest`. The test process runs the Go runtime and starts `rutis-host test` once (`rutis-host` on `PATH`, or the path in `RUTIS_HOST`): a host with no rows of its own, in a session with that runtime (on Unix a socket pair, `--channel fd:3`; on Windows and with `RUTIS_LOCAL_HANDOVER=loopback` a loopback connection). Each `Load` is a row of that host, which checks its config and fills its defaults in, waits for the services it injects and dispatches its events. The services a test gives, its listeners and its emissions are plugins of the test's in the same runtime; they use what every plugin uses. Rows unload when their test ends; the host stays for the next test and ends with the test process. Calls between the test and the plugin are calls between two rows of one Go runtime.

| Name | What it does |
| --- | --- |
| `loaded.Emit`, `Parallel`, `Serial`, `Waterfall`, `Bail`, `Filter` | Emit, from the test's plugin, on the host's bus |
| `loaded.On(name, fn, options...)` | Listen, as the test's plugin; the options are `advanced.Prepend()`, `advanced.Global()`; returns the function that stops it |
| `loaded.Load(config, options...)` | Load another row of the plugin, with the same services |
| `loaded.Update(config)` | Change the row's config, as rutis does for a Go row: a restart; a config the Schema refuses fails the test and the row keeps its config |
| `loaded.Reported()` | What the runtime and the host reported since the row loaded: failed `Emit` listeners, filters, emissions |
| `loaded.Provided()`, `loaded.Unload()` | The names of the services the row provides now; unload |
| `loaded.Service(name)` | The service as a plugin injecting it gets it: `Call(ctx, method, &result, args...)`, `Methods()`, and `Bind(&target)`, which fills a struct of function fields as `ctx.Use` does |
| `TryLoad` | `Load` giving back the row's line when it cannot start (`weather: config: city is required`) |
| Option of `Load`, `TryLoad` and `loaded.Load` | `Row(key)` gives the row its id (default the plugin's name, then `<name>-2`, `<name>-3`, ...) |

A host has one provider of a service name: a service the test gives to two `Load`s whose rows are both loaded is one provider, so both get the same value; another value fails the second `Load`. The provider goes when the last row using it unloads.

A test of [the Go `greeter` above](#go):

```go
func TestEvents(t *testing.T) {
	loaded := rutistest.Load(t, Plugin, struct{}{}, nil)
	said := make(chan any, 1)
	loaded.On("greeting/said", func(_ context.Context, args ...any) (any, error) {
		said <- args[0] // the test's plugin listens on the host's bus
		return nil, nil
	})
	var text string
	if err := loaded.Service("greeter").Call(context.Background(), "hello", &text, "Ada", "sv"); err != nil {
		t.Fatal(err)
	}
	if text != "Hej, Ada!" || <-said != "Ada" {
		t.Fatalf("got %q", text)
	}
	word, err := loaded.Serial(context.Background(), "greeting/pick", "en") // the test emits too
	if err != nil || word != nil {
		t.Fatalf("got %v, %v", word, err)
	}
}
```

A plugin definition the plugin hands to a test double (the double's method takes a `*rutis.Definition`) can be passed to `rutistest.Load` as it is: it loads another row of the runtime that defined it, with what the plugin's `Apply` prepared. A definition without a `Name` can also be passed to `rutistest.Load` directly.

### The Swift test tool

`RutisTest`:

| Name | What it does |
| --- | --- |
| `loaded.emit`, `parallel`, `serial`, `waterfall`, `bail` | Emit as a plugin of the same runtime does |
| `loaded.on(name, prepend:, global:) { ... }` | Listen as a plugin of the same runtime does |
| `loaded.load(config:row:isolate:)` | Load another row of the plugin |
| `loaded.update(config:)` | Change a row's config: the host checks it, and the plugin applies again |
| `await loaded.reported` | What was reported since the test started: the runtime's lines of failed `emit` listeners, then the host's lines of the test's rows |
| `loaded.provided`, `loaded.row`, `loaded.unload()`, `loaded.close()` | The names of the services the row provides now; the row's id; unload the row; unload every row of the test |
| `RutisTest.load(_:config:services:row:isolate:)` | `row` is the row's id (by default the plugin's name); `isolate` the row's `isolate` (service name: `true` or a label), as in rutis.json |

The plugin runs as a row of a real host: the test process runs the Swift runtime, and `rutis-host test` (on `PATH`, or the path in `RUTIS_HOST`) runs a host whose session is with it. The host checks the row's config and fills its defaults, starts the row once the services it injects are there, and dispatches every event. The test's side (its services, `on` and the five emits) is a plugin in the same runtime, so values between the test and the plugin do not cross processes; what crosses processes is checked by the runtimes' own tests. One host serves the whole test process; `close()` unloads the test's rows. Rows load at the root: the test tool places no row in a scope. On a device or the iOS simulator there is no `rutis-host`: test there with an app that installs its own plugins (`RutisApp`, [Swift app guide](swift-app.en.md)).

A test double receives the plugin definition itself: `RutisTest.load(definition, config: ...)` loads it as a host mounts it; handed back by the double, the plugin gets the original one.

## Synchronous calls and reentrancy

A method declared `sync` returns synchronously. Across processes, the caller blocks while waiting. During that wait:

- If the callee calls back into the caller, the callback can reach and run in the waiting caller.
- The Python runtime also handles other incoming calls while waiting synchronously. A Python service may therefore be called while it is already handling another synchronous call. **Do not hold locks while calling another service.**
- The Go runtime runs every incoming call on a goroutine of its own, so it is always reentrant: provided Go objects must be safe for concurrent use. The call chain travels in `context.Context`: **pass on the ctx you were given** when calling other services, or a synchronous caller waiting on you may not recognize your callback.
- The Node runtime handles only calls that belong to the current call chain while waiting synchronously; it runs the others once its stack is empty.
- In the Python, Node (and Bun) runtimes, a call run during a synchronous wait is stacked above the waiting one: the call that started first waits for the one that started later.

When runtimes call each other synchronously, every participant may end up waiting for another: two Node runtimes calling each other, or one Node and two Python runtimes in a nested pattern. Before forwarding each synchronous call, the host checks whether it would close such a wait cycle; if it would, the call is not forwarded, its caller gets a `SyncWaitCycle` error, and every other call completes. Do not retry the same call at once (it will most likely close the same cycle again): use an `async` method, try at another moment, or treat it as a failure.

The check covers every synchronous call a runtime marks as one on its frame, as every runtime of this repository does (the protocol is in §9 of the [multi-language design](../design-multilang-runtimes-2026-10-03.en.md)); a runtime that does not declare `reentrant-sync` is taken as non-reentrant. For calls across runtimes that may cross in both directions, `async` methods are still the first choice.

Do not wait inside a synchronous method for a result that requires the event loop to advance, such as a Promise. This produces `SyncWaitCycle`; make the method `async` instead.

An `async` method runs up to its first `await` before the reply, like a JS async function, so an `async` method that never suspends can be awaited inside a synchronous call chain too.

## How long a call waits

rutis sets no time limit on a call, and cancels no call for either side: how long a call may take is the caller's business. To limit it, use the caller's own language; the cancellation reaches the far end (see [Cancellation](#cancellation)):

| Language | How |
| --- | --- |
| Python | `await asyncio.wait_for(weather.tomorrow(), timeout=5)` |
| Go | `ctx, cancel := context.WithTimeout(ctx, 5*time.Second)`, passing `ctx` to the call |
| Swift | call in a `Task`, and cancel the `Task` when the time is up |
| Kotlin | `withTimeout(5_000) { weather.tomorrow() }` |
| Rust | `tokio::time::timeout(Duration::from_secs(5), weather.tomorrow())` (dropping the future cancels it) |
| TypeScript | `Promise.race` with a timer; the far end is not cancelled, as in plain JS (a promise cannot be cancelled) |

To see how long a call has been waiting, use `rutis-host status` (it lists the calls in progress).

## Cancellation

When the caller abandons an async call (for example, on timeout), the call is cancelled: a Python coroutine is cancelled, a Go method's `context.Context` is cancelled, and a JavaScript method receives an abort when the caller supplied an `AbortSignal` (Rust bindings pass one when calling Cordis methods). Without either mechanism, the callee runs to completion and its result is discarded.

- **Swift**: a caller that gives up cancels the method's Task (`Task.checkCancellation()` throws `CancellationError`); a call the plugin makes from a cancelled Task throws `CancellationError`; to still call once cancelled (to report that it stopped, say), call from `Task { … }`, which keeps the call chain and not the cancellation.
- **Rust**: dropping the call's future cancels it.
- How a dispatch of an event is cancelled: [events: lifecycle](#lifecycle).

## Your own connection: `sessions.open`

To let rutis use the plugins at the other end of a connection of your own (a relay, Bluetooth, another transport), write a connection plugin: take a connection that carries binary messages and hand it to the service `sessions`. The far end can be a language runtime, an app that installs its own plugins, or another rutis.

```ts
type Close = { kind: 'done' } | { kind: 'replaced' } | { kind: 'rejected'; reason: string }
interface Far  { send(message: Uint8Array): void; close(why: Close): void }
interface Near { receive(message: Uint8Array): Promise<void>; end(why: Close): void }
sessions.open(far: Far): Near
```

```ts
export default definePlugin({
  name: 'relay',
  inject: ['sessions'],
  async apply(ctx, config: { url: string }) {
    const ws = new WebSocket(config.url)
    ws.binaryType = 'arraybuffer'
    await new Promise(ok => ws.addEventListener('open', ok, { once: true }))
    const near = ctx.sessions.open({
      send: m => ws.send(m),
      close: why => ws.close(why.kind === 'rejected' ? 4003 : why.kind === 'replaced' ? 4002 : 1000),
    })
    ws.onmessage = e => near.receive(new Uint8Array(e.data))
    ws.onclose = () => near.end({ kind: 'done' })
    ctx.effect(() => () => ws.close())
  },
})
```

- Write `sessions` in `inject`; without it, `open` fails: `declare sessions in inject`.
- The session hangs under the plugin that called `open`: when that plugin unloads, its sessions and the far end's plugins all unload.
- `receive` completes once the message is in the session's receive queue; the queue holds 64 messages, and when it is full `receive` waits, which carries backpressure back to the connection.
- The far end's plugins are named `<far id>:<plugin>` here; to have the far end run a plugin, write a row `<connection row>:<plugin>` in rutis.json (for example `relay:contacts`); while the far end is not there, that row waits.
- A connection plugin compiled into a Rust host uses `rutis_bridge::Session` directly: `ctx.plugin(Session::new(channel))`; the host must provide `LocalId` in the root ctx (rutis-host does).

## Versions

Plugins are tagged with the plugin API version they use (`definePlugin` / `define_plugin` / `rutis.Define` adds the tag automatically). If the runtime is older than the plugin requires, loading fails with an explanation of what to upgrade; a newer runtime loads it. How a plugin depends on its SDK:

- JavaScript, TypeScript: a caret range (`"@arcships/rutis": "^0.17.2"`). The plugin uses the SDK of the minor version it was written against; when the host has another version, npm installs the plugin a copy of its own, and several versions work side by side.
- Python: a lower bound only (`rutis>=0.17`). The plugin and the Python runtime are one `rutis` package in the host's environment, so an upper bound (`<0.18`) would downgrade the runtime when the plugin is installed, or keep the host from upgrading. So a release that removes or moves a name plugins import says so in its migration guide, and plugins upgrade together with the host (0.19, for example, moves the advanced names into `rutis.advanced`, see [Migrating to 0.19](../migration-0.19.en.md)).
- Go, Swift: the SDK is compiled into the binary (`require`, `from:`), whatever version the host installs.

Plugin API 3 (rutis 0.13) adds `parallel`, `serial`, `waterfall`, `bail` and `filter`, and events become one logical bus; 0.13 runtimes still load API 2 plugins, and an older runtime refuses an API 3 plugin with the existing message. The 0.13 event protocol does not mix with runtimes or nodes of 0.12 and earlier (see [mixing with 0.12 and earlier](#mixing-with-012-and-earlier) above). Plugin API 2 added leaf events; API 1 plugins still load.
