# Objects with methods crossing languages by reference in 0.18

[中文](design-object-refs-2026-10-27.md)

Status: final, not implemented. Date: 2026-10-27. Baseline: `main` `85678323` (after 0.17.2). Scheduled for: 0.18.
Issue: [#687](https://github.com/arcships/rutis/issues/687) (split from [#675](https://github.com/arcships/rutis/issues/675), part of [#663](https://github.com/arcships/rutis/issues/663)).
Based on: [Plugin API](guide/plugin-api.en.md) "How values cross"; [design philosophy](design-philosophy.en.md) §7; [value types](design-value-types-2026-10-15.en.md) (V0 developer transparency, V4 capability negotiation, V10 `undefined`); [plugin definitions](design-plugin-values-2026-10-25.en.md) (the precedent for adding a kind of reference); [diagnostics](design-diagnostics-2026-10-26.en.md) (error codes `bad-value`, `no-method`, `peer-gone`; §10).

Out of scope: Rust plugins (`#[rutis::service]`) sending and receiving their own objects (§9); live reads and writes of an object's data fields across processes; event forwarding between nodes (`NodeEvent` arguments are JSON and still carry no references).

## 1. Which option

**Option 1: implement what the docs say.** Reasons:

- **Developer transparency** (V0): in one process, Cordis hands the object itself to the other side. Handing an object to a service in another language the same way is the most natural code. Changing the docs instead would make developers rewrite their code for crossing languages.
- **Most of it exists**: `object` references on the wire, `call` frames with a method name, `Release` and counting, the host forwarding objects, and the `objects` capability are all there. Node, Bun, Python, Go, Swift and Kotlin can already **send** objects and serve method calls on them. Only none of them except Rust **receives** them (they do not declare `objects`, and receiving one fails the call). What is missing is mainly the receiving side.
- **The entry points exist**: a received object is the same thing as the service `ctx.use` returns: a set of methods you can call. Each language receives it with the service proxy it already has, and no new way of writing code is added (§4).

## 2. Decisions

| # | Decision | Section |
| --- | --- | --- |
| O1 | The **value being sent** decides between copy and reference: data types are copied, types with methods go by reference; nothing rutis-specific is written (Go's `any` and Swift / Kotlin method tables are the parts the languages cannot do, and use the existing ways of writing them) | 3 |
| O2 | A plain JS object (literal) is always data; its functions go as function references, and `this` is that object when they are called. Only class instances go by reference, and a class instance with no methods is data. Python dicts and dataclasses follow the same rule | 3 |
| O3 | A received object is used like the service proxy from `ctx.use`: every method is an async call, there are no data fields; Go, Swift and Kotlin receive it with their existing service client types (structs of function fields, `@RutisClient`) | 4 |
| O4 | The same object handed over several times gives the same proxy; handed back to its process, it is the original object | 4 |
| O5 | Lifetime is the same as function references: released when the receiver no longer holds it; not tied to the row; calls fail with `peer-gone` once the other process is gone | 5 |
| O6 | The wire format does not change; every SDK declares the existing capability `objects` (in both handshakes). A 0.17 end does not declare it, and the call that would send it an object fails with `bad-value` | 6 |
| O7 | `undefined`: kept only as a whole argument or a whole return value; inside arrays, objects and Maps it is `null` / `None` at any depth, written that way by the sender and read that way by the receiver | 7 |

## 3. What goes by reference

Rule: **data is copied, things with methods go by reference.** The value being sent decides, as in Cordis, regardless of what the other side expects.

| Language | Copied (data) | By reference (the other side gets a proxy) |
| --- | --- | --- |
| JS (Node, Bun) | Plain objects (prototype `Object.prototype` or `null`); arrays; instances of classes with no methods (own enumerable properties copied) | Instances of classes with methods: a function other than `constructor` on the prototype chain (before `Object.prototype`), or an own function-valued property |
| Python | `dict`, `list`, `tuple`, dataclasses (copied as dicts); instances of classes with no public methods (such as `SimpleNamespace`, public attributes copied) | Instances of classes with public methods (not starting with `_`) |
| Go | Structs, slices, maps | Values sent to a parameter or return value declared as an interface type (except `any` and `error`); at `any`, write `rutis.Ref(v)` (exists) |
| Swift | `Codable` structs and classes | Instances of `@RutisService` types (exists) |
| Kotlin | `@Serializable` classes | Instances of `@RutisService` classes (exists) |
| Rust | serde values | Does not send its own objects (§9); forwards other processes' objects |

Notes:

- **Functions in data**: functions inside plain objects / dicts / structs still go as function references, and the other fields are copied. So `{ total: 3, onProgress }` arrives in Python as `{"total": 3, "onProgress": <function>}`, with both. When a function in a plain JS object is called, `this` is that object, so `{ count: 0, inc() { this.count++ } }` also works across processes.
- **Objects in data**: objects can appear at any depth in data (array elements, dict values, dataclass fields), and each goes by reference. Dataclass fields could not hold references before; now they work like dicts.
- **Cordis plugins**: in Node, a plain object with an `apply` function is a Cordis plugin. It is still sent as an object reference and read as a plugin definition when it reaches a `rutis::Definition` ([plugin definitions](design-plugin-values-2026-10-25.en.md) R1, R2). This is Cordis's own definition of a plugin, not a new rule.
- **Callable objects**: Python instances with `__call__` still go as function references.
- **Why Go needs something written**: a Go struct is data and can also have methods, and the value alone does not tell which. When the declared type is an interface nothing is written; only `any` needs `rutis.Ref(v)`.
- **Why Swift and Kotlin need an annotation**: these languages cannot list a class's methods at run time; `@RutisService` generates the method table at compile time. It is the annotation already used to provide services.

## 4. What the receiver gets

A received object is the same kind of thing as the service proxy from `ctx.use`: **each method is an async cross-process call on the original object; there are no data fields**. To get data, have a method return it.

| Language | How | What it gets |
| --- | --- | --- |
| JS (Node, Bun) | Nothing to declare | A proxy: `await cache.get('a')`; every name read on it is a method (except symbols, `then`, `toJSON`, `toString`, `valueOf`, so awaiting a proxy gives the proxy); `console.log` shows `[object from py]` |
| Python | Nothing to declare | A proxy: `await cache.get("a")`; any public name is a method (as with `ctx.use`); its `repr` is `<object from node>` |
| Go | Declare the parameter as a struct of function fields (the same as a `ctx.Use` target, with fields mapped to method names by the same rule); `any` receives `*rutis.Object` | Each function field of the struct calls the matching method (blocking, first argument `context.Context`); `*rutis.Object` has `Call(ctx, method, args...) (any, error)` and can be handed on as is |
| Swift | Declare the parameter as a `@RutisClient` protocol (`any Cache`); `Any` receives `Rutis.RemoteObject` | The same proxy the macro generates (the one `ctx.use(name, Cache.self)` uses) |
| Kotlin | Declare the parameter as a `@RutisClient` interface; `Any?` receives `RemoteObject` | The same proxy KSP generates; `suspend` methods suspend, plain methods block (as with service clients) |
| Rust | Hand-written host dispatch and Cordis bindings use `rutis_bridge::session::ObjectRef` (exists) | `call` / `call_async` |

- `*rutis.Object`, `Rutis.RemoteObject` and `RemoteObject` mirror the existing `*rutis.Function`, `Rutis.RemoteFunction` and `RemoteFunction`: they appear only when the parameter type is `any` / `Any`, to hand the object on or call it by name.
- **The same object**: an object handed over several times gives the receiver the same proxy (JS `===`, Python `is`); a proxy handed back to its process is the original object there. Handed to a third process, the host forwards it as usual.
- **In the same process** the receiver gets the object itself (Cordis semantics): methods stay synchronous and fields stay readable. Across processes there are only methods, and all of them are async. This is written down as a boundary rule (principle 7); the testing tool's strict mode (the default) hands out proxies shaped as across processes, so the difference shows up early.
- **Call semantics** are those of function references: arguments and return values cross by "How values cross"; errors thrown by the other side come back as usual; cancellation reaches the other side as usual ([diagnostics](design-diagnostics-2026-10-26.en.md) §6.1). Methods of Go objects may be called concurrently (an existing rule).

## 5. Lifetime

- **The same as function references**: every hand-over counts once; when the receiver's proxy is no longer referenced (JS `FinalizationRegistry`, Python `weakref.finalize`, Go `runtime.AddCleanup`, Swift `deinit`, Kotlin's cleaner, the same mechanism each uses for function references) it sends `Release`; at a count of 0 the sender lets the object go. An object passed as an event argument still lives until that call ends.
- **Not tied to the row**: after the row that handed the object over unloads, the object still exists and can still be called, as with Cordis objects in one process. To disable it on unload, the author does so in a cleanup (as with functions). Plugin definitions are tied to the row because copies are mounted from them; objects have nothing like that.
- **Process gone**: after the sender's runtime exits or disconnects, calls on the proxy fail with code `peer-gone`, and so do calls in progress. Only that call fails; later calls on the proxy fail the same way. Nothing switches to a new process (the new process does not have the object).

## 6. Wire format and capability

- **The wire format does not change**: `{"type":"reference","value":{"id","home","kind":"object","origin"}}`; a method call is a `call` frame with `method`; the `get` frame (live property read) stays for Rust's `ObjectRef::get` and Cordis, and the new proxies do not use it.
- **Capability `objects`** (exists; only Rust declares it today): Node, Bun, Python, Go, Swift and Kotlin declare it in both the endpoint and compat handshakes; Rust's compat declaration adds it too. The sender and the host's forwarding both check whether the **receiving side** declared it, in both handshakes (today only the endpoint one is checked).
- **Mixing with 0.17**:
  - A 0.18 object sent to a 0.17 runtime: it does not declare `objects`, so this call fails with `bad-value`, saying the other side is too old (§8), and the session continues. A 0.17 end never receives an object.
  - A 0.18 runtime connected to a 0.17 host: the 0.17 host already declares `objects` and checks the downstream capability when forwarding, and the wire format is unchanged, so objects pass between 0.18 runtimes as usual.
  - No protocol version bump, no new capability name.

## 7. `undefined`

Today the same value has two shapes in Python: `[1, undefined, 3]` as an argument is `[1, UNDEFINED, 3]`, and inside an object it is `[1, None, 3]`. JS encodes arrays element by element (keeping the `undefined` tag) and encodes an object of plain data as one JSON value (`undefined` becomes `null`), and Python does not normalise on decoding.

Rule (carrying out V10):

- **As a whole argument**: keeps the meaning "not given", as today: for JS `f(1, undefined)`, Python receives one argument fewer or `None` (today's `_apply`), Go the zero value.
- **As a whole return value**: JS receives `undefined`; other languages their empty value.
- **Everywhere else** (inside arrays, objects, Maps, Sets at any depth, and holes in sparse arrays): always `null` / `None` / `nil`. The sender writes `null`; the receiver reads an incoming `undefined` tag as `null` too, so values from older ends agree.
- Result: `[1, undefined, 3]` at any level is `[1, None, 3]` in Python and `[1, null, 3]` in JS. `UNDEFINED` no longer appears in Python plugin code.

## 8. What developers see

Error codes follow [diagnostics](design-diagnostics-2026-10-26.en.md) §4.3; codes do not appear in the text for people. The prefix (row name, method, argument position) is written as in the diagnostics design.

```ts
// TypeScript: hand an instance of a class with methods to a Python service
class Cache { #m = new Map(); get(k) { return this.#m.get(k) } set(k, v) { this.#m.set(k, v) } }
await ctx.use('warmer').warm(new Cache())
```

```python
# Python: use it as usual; methods are async
class Warmer:
    async def warm(self, cache):
        await cache.set("a", 1)
        return await cache.get("a")    # 1, calling the Cache in TS
```

```go
// Go: declare the parameter as a struct of function fields, the same as a ctx.Use target
type Cache struct {
	Get func(ctx context.Context, key string) (any, error)
	Set func(ctx context.Context, key string, value any) error
}
func (w *Warmer) Warm(ctx context.Context, cache Cache) (any, error) {
	if err := cache.Set(ctx, "a", 1); err != nil { return nil, err }
	return cache.Get(ctx, "a")
}
```

```swift
// Swift: declare the parameter as a @RutisClient protocol
@RutisClient protocol Cache { func get(_ key: String) async throws -> Int?; func set(_ key: String, _ value: Int) async throws }
@RutisService final class Warmer { func warm(_ cache: any Cache) async throws -> Int? { try await cache.set("a", 1); return try await cache.get("a") } }
```

```kotlin
// Kotlin: declare the parameter as a @RutisClient interface
@RutisClient interface Cache { suspend fun get(key: String): Int?; suspend fun set(key: String, value: Int) }
@RutisService class Warmer { suspend fun warm(cache: Cache): Int? { cache.set("a", 1); return cache.get("a") } }
```

| Case | What is shown | Code |
| --- | --- | --- |
| Calling a method the object does not have | `chat: warmer.warm failed: the Cache from node has no method gett (methods: get, set)` | `no-method` |
| Reading a data field on a JS proxy as a method | `… the Cache from node has no method size (size is a field; objects passed by reference expose only their methods)` | `no-method` |
| The other process has exited | `chat: a method of the Cache from py failed: runtime py exited` | `peer-gone` |
| Handing it to a 0.17 runtime | `chat: argument 0 of warmer.warm: an object with methods (Cache) cannot cross to py 0.17.2, which does not take objects; upgrade it to rutis 0.18` | `bad-value` |
| Swift class without the annotation | `argument 0 of warmer.warm: a Cache cannot cross: mark it @RutisService to pass it by reference, or make it Codable to copy it` | `bad-value` |
| Kotlin class without the annotation | `argument 0 of warmer.warm: a Cache cannot cross: annotate it @RutisService to pass it by reference, or @Serializable to copy it` | `bad-value` |
| Go parameter type that cannot take an object | `chat: warmer.warm: argument 0 should be a string, got an object with methods (Cache from node)` | `bad-argument` |
| Swift / Kotlin parameter is a protocol / interface without `@RutisClient` | `warmer.warm: argument 0 should be a Cache, but Cache is not @RutisClient, so an object cannot be received as it` | `bad-argument` |

The "values are copied" hint in [diagnostics](design-diagnostics-2026-10-26.en.md) §10 becomes: "copied by value; objects with methods go by reference, and the other side can only call their methods".

## 9. Rust

- **Host side**: every object crossing runtimes is forwarded by the host (exists), so this column tests forwarding: between any two languages, back to its own process, `Release` reaching the source, `peer-gone` after the source exits.
- **Hand-written host dispatch and Cordis bindings** still receive objects and call methods with `rutis_bridge::session::ObjectRef`.
- **Rust plugins do not send or receive their own objects in 0.18**: Rust cannot build an implementation of a trait at run time, so this needs a separate macro on traits, and there is no user for it today (criterion 4). Rust plugins still receive behaviour through function parameters (`impl Fn`); a `#[rutis::service]` parameter of another rutis-bridge type is still a compile error, with a message that now points to function parameters.

## 10. Conformance cases

One shared expectations file (as for events and value types), one column per language; the testing tools' strict mode is checked against the same file.

| Case | Node | Bun | Python | Go | Swift | Kotlin | Rust |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Send an object with methods, the other side calls a method, the original object's state changes | send / receive | send / receive | send / receive | send (interface, `Ref`) / receive (struct, `*Object`) | send / receive (protocol, `RemoteObject`) | send / receive (interface, `RemoteObject`) | forward; receive with `ObjectRef` |
| Handed over twice gives the same proxy; handed back gives the original | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | forward |
| Handed to a third language | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | forward |
| As a return value, an element inside data, an event argument | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | forward |
| Data stays data: plain objects / dicts / structs / `Codable` / `@Serializable` copied; functions inside go by reference, `this` is the object (JS) | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | — |
| Instances of classes with no methods are copied | ✓ | ✓ | ✓ | — | — | — | — |
| Missing method: `no-method`, only this call fails | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |
| Receiver drops the proxy: the sender's object is released | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | forward |
| Sender's process killed: calls fail with `peer-gone`, the session and other rows are unaffected | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |
| The other side does not declare `objects`: `bad-value`, the session continues | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |
| The declared type cannot take an object: `bad-argument` | — | — | — | ✓ | ✓ | ✓ | — |
| `undefined` is `null` at any depth and kept as a whole argument | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |

Across versions: the release gate's cross-version meetings gain two cases: a 0.18 object sent to a 0.17 runtime (`bad-value`, the session continues); objects passed between 0.18 runtimes through a 0.17 host (works). The [guarantees table](guarantees.md) gains `bridge/object-refs` (§3–§6) and its `undefined` entry is rewritten, each naming the tests that check it.

## 11. Compatibility and migration

0.18 already has a migration guide; this goes into "Migrating to 0.18" (both languages). It is a breaking change, but it only affects values that could only be sent to Rust before:

- **JS**: plain objects with function properties used to go whole as an object reference (only Rust received it); now they are data plus function references. Instances of classes with no methods used to go as object references; now they are copied. Rust hosts reading such values with `ObjectRef` read them as data instead. Cordis plugin objects with `apply` are unchanged.
- **All languages**: `undefined` inside arrays is now `null` (before, JS sending to JS or Python kept `undefined` / `UNDEFINED`).
- **Python**: dataclass fields can hold functions and objects (an error before); instances of classes with no public methods are copied (by reference before, received only by Rust).
- **Failed before, works now**: objects with methods sent to Node, Bun, Python, Go, Swift and Kotlin.
- Error texts change (§8). The wire format and the protocol version do not.

## 12. Docs to update

- [Plugin API](guide/plugin-api.en.md) (both): the "objects with methods, class instances" row of "How values cross" becomes §3 and §4 of this design; the "`undefined` in data" and "holes in arrays" rows become §7; add a paragraph "the same process versus across processes"; rewrite the row of the testing tool's differences that says objects with methods always go by reference.
- Each language guide (both): [TypeScript](guide/typescript-plugin.en.md), [Bun](guide/bun-plugin.en.md), [Python](guide/python-plugin.en.md), [Go](guide/go-plugin.en.md) (interface parameters, structs of function fields, `*rutis.Object`), [Swift](guide/swift-plugin.en.md), [Swift App](guide/swift-app.en.md), [Kotlin App](guide/kotlin-app.en.md) (`@RutisClient` parameters, `RemoteObject`), [Rust host](guide/rust-host.en.md) (what `ObjectRef` now receives), [Cordis](guide/cordis.en.md).
- [Protocol design](design-protocol-plugins-2026-09-25.en.md): the meaning of the `objects` capability, checking in both handshakes, the position rule for `undefined`.
- "Migrating to 0.18", the [guarantees table](guarantees.md).

## 13. Implementation

| Phase | Contents |
| --- | --- |
| O-1 | Specification and host: `objects` and the `undefined` rule in the protocol design; Rust's compat handshake declares `objects`, and forwarding checks it in both handshakes; Rust encoding and decoding write and read `undefined` in containers as `null`; error texts and codes for object calls (the code table comes with #677); the shared conformance expectations file and the Rust column; guarantee entries |
| O-2 | Node, Bun: the sending rules of §3 (plain objects are data, `this` of function properties, classes with no methods copied); the proxy for received objects; declare `objects`; testing tool strict mode; `undefined`; guides |
| O-3 | Python, Go: sending rules (Python instances with no public methods, dataclasses with references; Go interface parameters by reference); the proxy (Python), structs of function fields and `*rutis.Object` (Go); declare `objects`; testing tools; `undefined`; guides |
| O-4 | Swift, Kotlin: `@RutisClient`-typed parameters receiving objects, `RemoteObject`; declare `objects`; testing tools; guides; final rewrite of "Migrating to 0.18" and the Plugin API guide |

One PR per phase, merged in order; later phases reuse O-1's expectations file. #687 closes when O-4 is merged and all seven columns pass.
