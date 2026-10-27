# Objects with methods crossing languages by reference in 0.18

[中文](design-object-refs-2026-10-27.md)

Status: final, not implemented. Date: 2026-10-27 (revised the same day: sending rules stay as they are; corrected the facts about mixing with 0.17; Go only learns to receive; error texts use only what the wire carries; added idle stop, non-home plugin references, and objects reaching Rust services). Baseline: `main` `85678323` (after 0.17.2). Scheduled for: 0.18.
Issue: [#687](https://github.com/arcships/rutis/issues/687) (split from [#675](https://github.com/arcships/rutis/issues/675), part of [#663](https://github.com/arcships/rutis/issues/663)).
Based on: [Plugin API](guide/plugin-api.en.md) "How values cross"; [design philosophy](design-philosophy.en.md) §7; [value types](design-value-types-2026-10-15.en.md) (V0 developer transparency, V4 capability negotiation, V10 `undefined`); [plugin definitions](design-plugin-values-2026-10-25.en.md) (the precedent for adding a kind of reference); [diagnostics](design-diagnostics-2026-10-26.en.md) (error codes `bad-value`, `bad-argument`, `no-method`, `peer-gone`; §10).

Out of scope: Rust plugins (`#[rutis::service]`) sending and receiving their own objects (§9); live reads and writes of an object's data fields across processes; event forwarding between nodes (`NodeEvent` arguments are JSON and still carry no references).

## 1. Which option

**Option 1: implement what the docs say.** Reasons:

- **Developer transparency** (V0): in one process, Cordis hands the object itself to the other side. Handing an object to a service in another language the same way is the most natural code. Changing the docs instead would make developers rewrite their code for crossing languages.
- **Most of it exists**: `object` references on the wire, `call` frames with a method name, `Release` and counting, the host forwarding objects, and the `objects` capability are all there. Node, Bun, Python, Go, Swift and Kotlin can already **send** objects and serve method calls on them. Only none of them except Rust **receives** them (they do not declare `objects`, and receiving one fails the call). What is missing is the receiving side.
- **The entry points exist**: a received object is the same thing as the service `ctx.use` returns: a set of methods you can call. Each language receives it with the service proxy it already has, and no new way of writing code is added (§4).

## 2. Decisions

| # | Decision | Section |
| --- | --- | --- |
| O1 | Sending rules do not change: data is copied; class instances and plain objects with function-valued properties go by reference (JS); instances other than dataclasses go by reference (Python); `rutis.Ref(v)` (Go); `@RutisService` (Swift, Kotlin). Objects go by reference at any depth in data, so dataclass fields may hold references | 3 |
| O2 | A received object is used like the service proxy from `ctx.use`: every method is an async call, there are no data fields; Go uses a struct of function fields, Swift and Kotlin `@RutisClient` types; `any` / `Any` receives an object type that mirrors the existing function types | 4 |
| O3 | The same object handed over several times gives the same proxy; handed back to its process, it is the original object | 4 |
| O4 | Lifetime is the same as function references: released when the receiver no longer holds it; not tied to the row; outstanding references do not keep a runtime running; calls fail with `peer-gone` once the other process is gone | 5 |
| O5 | The wire format does not change; every SDK declares the existing capability `objects`; the 0.18 host also recognises it in the compat handshake. A call that would hand an object to an end that does not take objects fails with `bad-value`, and the session continues | 6 |
| O6 | `undefined`: kept only as a whole argument or a whole return value; inside arrays, objects and Maps it is `null` / `None` at any depth. This is a fix to match the written rule (V10) | 7 |
| O7 | When Node or Python receives a plugin reference that is not coming home, only that call fails; the session no longer ends (the same as the old handling of received objects) | 8 |

## 3. What goes by reference

The rule does not change; it is what the Plugin API guide says today: **data is copied; objects with methods and class instances go by reference.** The value being sent decides, as in Cordis, regardless of what the other side expects.

| Language | Copied (data) | By reference (the other side gets a proxy) |
| --- | --- | --- |
| JS (Node, Bun) | Plain objects (prototype `Object.prototype` or `null`) with no function-valued property; arrays | Class instances; plain objects with function-valued properties (`{ get() {}, set() {} }`) |
| Python | `dict`, `list`, `tuple`, dataclasses (copied as dicts); functions inside a dict go as function references | Instances of other classes; instances with `__call__` go as function references |
| Go | Structs, slices, maps | `rutis.Ref(v)` (exists, the only way) |
| Swift | `Codable` structs and classes | Instances of `@RutisService` types (exists) |
| Kotlin | `@Serializable` classes | Instances of `@RutisService` classes (exists) |
| Rust | serde values | Does not send its own objects (§9); forwards other processes' objects |

- **Objects in data**: objects can appear at any depth in data (array elements, dict values, dataclass fields), and each goes by reference. Dataclass fields could not hold references before; to meet this rule they now work like dicts.
- **Why Go needs something written**: a Go struct is data and can also have methods, and the value alone does not tell which, so going by reference needs `rutis.Ref(v)` (an existing rule). Go only has to learn to receive.
- **Why Swift and Kotlin need an annotation**: these languages cannot list a class's methods at run time; `@RutisService` generates the method table at compile time. It is the annotation already used to provide services.

## 4. What the receiver gets

A received object is the same kind of thing as the service proxy from `ctx.use`: **each method is an async cross-process call on the original object; there are no data fields**. To get data, have a method return it.

| Language | How | What it gets |
| --- | --- | --- |
| JS (Node, Bun) | Nothing to declare | A proxy: `await cache.get('a')`; every name read on it is a method (except symbols, `then`, `toJSON`, `toString`, `valueOf`, so awaiting a proxy gives the proxy); `console.log` shows `[object from py]` |
| Python | Nothing to declare | A proxy: `await cache.get("a")`; any public name is a method (as with `ctx.use`); its `repr` is `<object from node>` |
| Go | Declare the parameter as a struct of function fields (the same as a `ctx.Use` target, with fields mapped to method names by the same rule); `any` receives `*rutis.Object` | Each function field of the struct calls the matching method (blocking, first argument `context.Context`). `*rutis.Object` mirrors `*rutis.Function`: `Call(ctx, method string, result any, args ...any) error`, and can be handed on as is |
| Swift | Declare the parameter as a `@RutisClient` protocol (`any Cache`); `Any` receives `Rutis.RemoteObject` | The same proxy the macro generates (the one `ctx.use(name, Cache.self)` uses) |
| Kotlin | Declare the parameter as a `@RutisClient` interface; `Any?` receives `RemoteObject` | The same proxy KSP generates; `suspend` methods suspend, plain methods block (as with service clients) |
| Rust | Hand-written host dispatch and Cordis bindings use `rutis_bridge::session::ObjectRef` (exists) | `call` / `call_async` |

- `*rutis.Object`, `Rutis.RemoteObject` and `RemoteObject` mirror the existing `*rutis.Function`, `Rutis.RemoteFunction` and `RemoteFunction`: they appear only when the parameter type is `any` / `Any`, to hand the object on or call it by name.
- **The same object**: an object handed over several times gives the receiver the same proxy (JS `===`, Python `is`); a proxy handed back to its process is the original object there. Handed to a third process, the host forwards it as usual.
- **In the same process** the receiver gets the object itself (Cordis semantics): methods stay synchronous and fields stay readable. Across processes there are only methods, and all of them are async. This is written down as a boundary rule (principle 7). The testing tool's strict mode (the default) hands out proxies shaped as across processes, so the difference shows up early.
- **Call semantics** are those of function references: arguments and return values cross by "How values cross"; errors thrown by the other side come back as usual; cancellation reaches the other side as usual ([diagnostics](design-diagnostics-2026-10-26.en.md) §6.1). Methods of Go objects may be called concurrently (an existing rule).

## 5. Lifetime

- **The same as function references**: every hand-over counts once; when the receiver's proxy is no longer referenced (each language uses what it uses for function references: JS `FinalizationRegistry`, Python `weakref.finalize`, Go `runtime.AddCleanup`, and so on) it sends `Release`; at a count of 0 the sender lets the object go. An object passed as an event argument still lives until that call ends.
- **Not tied to the row**: after the row that handed the object over unloads, the object still exists and can still be called, as with Cordis objects in one process. To disable it on unload, the author does so in a cleanup (as with functions). Plugin definitions are tied to the row because copies are mounted from them; objects have nothing like that.
- **Idle stop**: on-demand runtimes (Go, Swift) stop `idle` seconds after their last row unloads (exists). Objects of theirs held elsewhere do **not** keep them running: whoever holds a reference is not a row, and the host does not decide a process's life by references, as with function references. After the stop, calls on those objects fail with `peer-gone`. To keep them usable, keep the row that handed them over loaded, or make the runtime `eager`.
- **Process gone**: after the sender's runtime exits, disconnects or stops for idleness, calls on the proxy fail with code `peer-gone`, and so do calls in progress. Only that call fails; later calls on the proxy fail the same way. Nothing switches to a new process (the new process does not have the object).

## 6. Wire format and capability

- **The wire format does not change**: `{"type":"reference","value":{"id","home","kind":"object","origin"}}`; a method call is a `call` frame with `method`; the `get` frame (live property read) stays for Rust's `ObjectRef::get` and Cordis, and the new proxies do not use it. The reference carries no class name or method list, so the receiver and the host cannot name the object's class (§8).
- **Capability `objects`** (exists; only Rust declares it today):
  - Node, Bun, Python, Go, Swift and Kotlin declare it in both the endpoint and compat handshakes.
  - The 0.18 host adds `objects` to `COMPAT_DECLARED`. Local runtimes are all started with the compat format (`runtime/process.rs`); a 0.17 host only recognises the capabilities in `COMPAT_DECLARED`, which has no `objects`, so under a 0.17 host they always "do not take objects".
  - Runtimes still check the far end's `objects` only on endpoint sessions: on a compat session the far end is always the Rust host, which takes objects in every version. The host checks the **downstream** side when it forwards (exists).
- **Mixing with 0.17**:
  - Under a 0.18 host, an object to be forwarded to a 0.17 runtime: it does not declare `objects`, so this call fails with `bad-value` (text in §8), and the session continues. A 0.17 end never receives an object.
  - 0.18 runtimes under a 0.17.2 host: the host does not recognise their `objects`, so forwarding an object between two runtimes is refused with 0.17.2's text (`an object with methods cannot cross to py; pass plain data or a function instead`); only that call fails, and the session continues. A 0.18 runtime handing an object to the 0.17 host's own Rust code (`ObjectRef`, `live_objects.rs`) keeps working.
  - No protocol version bump, no new capability name.

## 7. `undefined`

Today the same value has two shapes in Python: when `[1, undefined, 3]` is an argument, the middle element is a special placeholder value; inside an object it is `None`. JS encodes arrays element by element (keeping the `undefined` tag) and encodes an object of plain data as one JSON value (`undefined` becomes `null`), and Python does not normalise on decoding. The Plugin API guide and V10 say `None`.

Rule (a fix to match V10):

- **As a whole argument**: keeps the meaning "not given", as today: for JS `f(1, undefined)`, Python receives one argument fewer or `None`, Go the zero value.
- **As a whole return value**: JS receives `undefined`; other languages their empty value.
- **Everywhere else** (inside arrays, objects, Maps, Sets at any depth, and holes in sparse arrays): always `null` / `None` / `nil`. The sender writes `null`; the receiver reads an incoming `undefined` tag as `null` too, so values from older ends agree.
- Result: `[1, undefined, 3]` at any level is `[1, None, 3]` in Python and `[1, null, 3]` in JS.

## 8. What developers see

Error codes follow [diagnostics](design-diagnostics-2026-10-26.en.md) §4.3; codes do not appear in the text for people. The prefix (row name, method, argument position) is written as in the diagnostics design.

```ts
// TypeScript: hand a class instance to a Python service
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

Who writes the text decides what it can say: the end that holds the object knows its class and methods; the receiver and the host only know which runtime it comes from, and write only "an object from py".

| Case | What is shown | Written by | Code |
| --- | --- | --- | --- |
| Calling a method the object does not have | `chat: warmer.warm failed: an object from node: Cache has no method gett (methods: get, set)`; for a plain object, `the object has no method gett (methods: get, set)` | The end holding the object ("an object from node" added by the host) | `no-method`, `data`: `{object, method, methods}` (`object` is the class name, `null` for a plain object) |
| Reading a data field on a JS proxy as a method | `… an object from node: Cache has no method size (size is a field; objects passed by reference expose only their methods)` | Same | `no-method` |
| The other process has exited or stopped for idleness | `chat: a call on an object from py failed: runtime py exited` | Receiver / host | `peer-gone`, `data`: `{runtime, reason}` |
| A 0.18 host forwarding to a 0.17 runtime | `chat: argument 0 of warmer.warm: an object from node cannot cross to py: py does not take objects (older than rutis 0.18); upgrade it` | Host | `bad-value` |
| Between 0.18 runtimes under a 0.17.2 host | 0.17.2's text: `argument 0 of warmer.warm: an object with methods cannot cross to py; pass plain data or a function instead` | 0.17.2 host | none (0.17 has no codes) |
| Swift class without the annotation | `argument 0 of warmer.warm: a Cache cannot cross: mark it @RutisService to pass it by reference, or make it Codable to copy it` | Sender | `bad-value` |
| Kotlin class without the annotation | `argument 0 of warmer.warm: a Cache cannot cross: annotate it @RutisService to pass it by reference, or @Serializable to copy it` | Sender | `bad-value` |
| Go parameter type that cannot take an object | `chat: warmer.warm: argument 0 should be a string, got an object from node` | Receiver | `bad-argument` |
| Swift / Kotlin parameter type that cannot take an object | `chat: warmer.warm: argument 0 should be a Cache, got an object from node; an object is received as a @RutisClient protocol (interface) or Any` | Receiver | `bad-argument` |
| An object reaching a parameter of a Rust service (`#[rutis::service]`) | `chat: store.put: argument 0 should be a Config, got an object from py; store takes data and functions, so pass a function for each method` | Host | `bad-argument` |
| Node or Python receiving a plugin definition that is not coming home | `chat: argument 0 of parts.register: a plugin definition from py can only be handed to a Rust service` | Receiver | `bad-value` |

The "values are copied" hint in [diagnostics](design-diagnostics-2026-10-26.en.md) §10 becomes: "copied by value; objects with methods go by reference, and the other side can only call their methods".

## 9. Rust

- **Host side**: every object crossing runtimes is forwarded by the host (exists), so this column tests forwarding: between any two languages, back to its own process, `Release` reaching the source, `peer-gone` after the source exits.
- **Hand-written host dispatch and Cordis bindings** still receive objects and call methods with `rutis_bridge::session::ObjectRef` (in [Cordis](guide/cordis.en.md), TS interfaces with methods generate proxy structs wrapping `ObjectRef`). The sending rules do not change, so the values they receive do not change either.
- **Rust plugins do not send or receive their own objects in 0.18**: Rust cannot build an implementation of a trait at run time, so this needs a separate macro on traits, and there is no user for it today (criterion 4). Rust plugins still receive behaviour through function parameters (`impl Fn`). When an object reaches a `#[rutis::service]` parameter, the call fails with `bad-argument`, and the message suggests passing functions instead (§8).

## 10. Conformance cases

One shared expectations file (as for events and value types), one column per language; the testing tools' strict mode is checked against the same file.

| Case | Node | Bun | Python | Go | Swift | Kotlin | Rust |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Send an object with methods, the other side calls a method, the original object's state changes | send / receive | send / receive | send / receive | send (`Ref`) / receive (struct, `*Object`) | send / receive (protocol, `RemoteObject`) | send / receive (interface, `RemoteObject`) | forward; receive with `ObjectRef` |
| A plain object with function-valued properties goes by reference | ✓ | ✓ | — | — | — | — | forward |
| Data stays data: plain objects / dicts / structs / `Codable` / `@Serializable` copied, functions inside by reference | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | — |
| A dataclass field holding an object | — | — | ✓ | — | — | — | — |
| Handed over twice gives the same proxy; handed back gives the original | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | forward |
| Handed to a third language | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | forward |
| As a return value, an element inside data, an event argument | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | forward |
| Still callable after the sending row unloads | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | forward |
| Cancellation through a method call (the caller gives up, the other side is cancelled) | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | forward |
| Missing method: `no-method`, only this call fails | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |
| Receiver drops the proxy: the sender's object is released | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | forward |
| Sender's process killed: calls fail with `peer-gone`, the session and other rows are unaffected | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |
| Idle stop: after an on-demand runtime stops, its objects fail with `peer-gone` | — | — | — | ✓ | ✓ | — | ✓ |
| The other side does not declare `objects`: `bad-value`, the session continues | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |
| The declared type cannot take an object: `bad-argument` | — | — | — | ✓ | ✓ | ✓ | ✓ (`#[rutis::service]` parameter) |
| The testing tool's strict mode hands out proxies shaped as across processes | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | — |
| A plugin reference that is not coming home: only this call fails | ✓ | ✓ | ✓ | — | — | — | — |
| `undefined` is `null` at any depth and kept as a whole argument | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |

Across versions: the release gate's cross-version meetings gain two cases: a 0.18 host forwarding an object to a 0.17 runtime (`bad-value`, the session continues); objects between two 0.18 runtimes under a 0.17.2 host (only that call fails, with 0.17.2's text, and the session continues), together with a 0.18 runtime handing an object to the 0.17.2 host's `ObjectRef` (works). The [guarantees table](guarantees.md) gains `bridge/object-refs` (§3–§6), each entry naming the tests that check it.

## 11. Compatibility and migration

Goes into "Migrating to 0.18" (both languages):

- **Failed before, works now**: objects with methods sent to Node, Bun, Python, Go, Swift and Kotlin. The sending rules do not change, so what Rust hosts and Cordis bindings receive does not change.
- **Python**: dataclass fields can hold functions and objects (an error before).
- **`undefined`**: a fix to match the written rule (the Plugin API guide, V10): `undefined` inside arrays is `None` in Python and `null` in JS at any level. Before, an array passed as an argument kept a special value; that was a bug against the docs.
- **Node, Python**: receiving a plugin definition that is not coming home fails only that call and no longer ends the session.
- Error texts change (§8). The wire format and the protocol version do not.

## 12. Docs to update

- [Plugin API](guide/plugin-api.en.md) (both): remove "only to Rust" from the "objects with methods, class instances" row of "How values cross" and write it as §3 and §4 of this design; add a paragraph on "the same process versus across processes" and idle stop (§5); rewrite the row of the testing tool's differences that says objects with methods always go by reference.
- Each language guide (both): [TypeScript](guide/typescript-plugin.en.md), [Bun](guide/bun-plugin.en.md), [Python](guide/python-plugin.en.md) (dataclass fields), [Go](guide/go-plugin.en.md) (struct-of-function-fields parameters, `*rutis.Object`), [Swift](guide/swift-plugin.en.md), [Swift App](guide/swift-app.en.md), [Kotlin App](guide/kotlin-app.en.md) (`@RutisClient` parameters, `RemoteObject`), [Rust plugin](guide/rust-plugin.en.md) (objects reaching service parameters), [rutis-host](guide/rutis-host.en.md) (`idle` ignores references).
- [Protocol design](design-protocol-plugins-2026-09-25.en.md): the meaning of `objects` in the compat handshake, the position rule for `undefined`.
- "Migrating to 0.18", the [guarantees table](guarantees.md).

## 13. Implementation

| Phase | Contents |
| --- | --- |
| O-1 | Specification and host: `objects` and the `undefined` rule in the protocol design; add `objects` to `COMPAT_DECLARED`; texts and codes for forwarding refusals, `peer-gone`, and objects reaching `#[rutis::service]` parameters (the code table comes with #677); Rust encoding and decoding write and read `undefined` in containers as `null`; the shared conformance expectations file and the Rust column; guarantee entries |
| O-2 | Node, Bun: declare `objects`; the proxy for received objects; `no-method` text and `data`; non-home plugin references fail only that call (the same decode code); testing tool strict mode; `undefined`; guides |
| O-3 | Python, Go: declare `objects`; the Python proxy, dataclass fields holding references, non-home plugin references fail only that call; Go struct-of-function-fields parameters and `*rutis.Object`; testing tools; `undefined`; guides |
| O-4 | Swift, Kotlin: declare `objects`; `@RutisClient`-typed parameters receiving objects, `RemoteObject`; testing tools; guides; final rewrite of "Migrating to 0.18" and the Plugin API guide |

One PR per phase, merged in order; later phases reuse O-1's expectations file.

Completion (#687 closes when all are met):

- [ ] Every row of §10 passes in all seven columns, and the two cross-version cases are in the release gate;
- [ ] Node and Python fail only the call when they receive a non-home plugin reference;
- [ ] Python's `undefined` agrees at the top level and when nested;
- [ ] The docs in §12 are updated in both languages.
