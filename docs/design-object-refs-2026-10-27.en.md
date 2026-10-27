# Objects with methods crossing languages by reference in 0.18

[中文](design-object-refs-2026-10-27.md)

Status: final, being implemented (corrected while implementing O-1: in §8 the text of a value that cannot be sent follows the diagnostics design's existing form, `warmer.warm: argument 1 cannot be sent: …`, not `argument 1 of warmer.warm: …`; §10's expectation file is `crates/rutis-bridge/src/runtime/testing/objects.expected.json`). Date: 2026-10-27 (revised four times the same day. Fourth: waiting cycles of synchronous calls, release when the holder's process exits. Third: how proxies are handed on, how Swift recognises client protocols, calls on objects skipping the method table, `no-method` in diagnostics §4.3. First two: sending rules stay as they are, corrected the facts about mixing with 0.17, Go only learns to receive, added idle stop and objects reaching Rust services; error texts say only what their writer knows, argument numbers start at 1, `any` / `Any` reuse existing types, dropped non-home plugin references and dataclass fields, and corrected [diagnostics](design-diagnostics-2026-10-26.en.md) §8.1 row 6 along the way). Baseline: `main` `85678323` (after 0.17.2). Scheduled for: 0.18.
Issue: [#687](https://github.com/arcships/rutis/issues/687) (split from [#675](https://github.com/arcships/rutis/issues/675), part of [#663](https://github.com/arcships/rutis/issues/663)).
Based on: [Plugin API](guide/plugin-api.en.md) "How values cross"; [design philosophy](design-philosophy.en.md) §7; [value types](design-value-types-2026-10-15.en.md) (V0 developer transparency, V4 capability negotiation, V10 `undefined`); [plugin definitions](design-plugin-values-2026-10-25.en.md) (the precedent for adding a kind of reference); [diagnostics](design-diagnostics-2026-10-26.en.md) (error codes `bad-value`, `bad-argument`, `no-method`, `peer-gone`; §10).

Out of scope:

- Rust plugins (`#[rutis::service]`) sending and receiving their own objects (§9).
- Live reads and writes of an object's data fields across processes.
- Event forwarding between nodes (`NodeEvent` arguments are JSON and still carry no references).
- Node and Python ending the session when they receive a plugin reference that is not coming home: a correct host never sends one (it does not forward plugin definitions, [plugin definitions](design-plugin-values-2026-10-25.en.md) R3), Go, Swift and Kotlin behave the same, and it is unrelated to #687.
- Dataclass fields holding references: an explicit error today, and #687 does not need it (objects inside dicts and lists already go by reference).

## 1. Which option

**Option 1: implement what the docs say.** Reasons:

- **Developer transparency** (V0): in one process, Cordis hands the object itself to the other side. Handing an object to a service in another language the same way is the most natural code. Changing the docs instead would make developers rewrite their code for crossing languages.
- **Most of it exists**: `object` references on the wire, `call` frames with a method name, `Release` and counting, the host forwarding objects, and the `objects` capability are all there. Node, Bun, Python, Go, Swift and Kotlin can already **send** objects and serve method calls on them. Only none of them except Rust **receives** them (they do not declare `objects`, and receiving one fails the call). What is missing is the receiving side.
- **The entry points exist**: a received object is the same thing as the service `ctx.use` returns: a set of methods you can call. Each language receives it with the service proxy it already has, and no new way of writing code is added (§4).

## 2. Decisions

| # | Decision | Section |
| --- | --- | --- |
| O1 | Sending rules do not change: data is copied; class instances and plain objects with function-valued properties go by reference (JS); instances other than dataclasses go by reference (Python); `rutis.Ref(v)` (Go); `@RutisService` (Swift, Kotlin) | 3 |
| O2 | A received object is used like the service proxy from `ctx.use`: every method is an async call, there are no data fields. Go uses a struct of function fields, Swift and Kotlin `@RutisClient` types; `any` / `Any` reuse existing types (Go `*rutis.Service`, Swift `Rutis.Client`), and only Kotlin adds `RemoteObject` | 4 |
| O3 | The same object handed over several times gives the same proxy; handed back to its process, it is the original object | 4 |
| O4 | Lifetime is the same as function references: released when the receiver no longer holds it; not tied to the row; outstanding references do not keep a runtime running; calls fail with `peer-gone` once the other process is gone | 5 |
| O5 | The wire format does not change; every SDK declares the existing capability `objects`; the 0.18 host also recognises it in the compat handshake. A call that would hand an object to an end that does not take objects fails, and the session continues | 6 |
| O6 | `undefined`: kept only as a whole argument or a whole return value; inside arrays, objects and Maps it is `null` / `None` at any depth. This is a fix to match the written rule (V10) | 7 |

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

- **Objects in data**: objects inside array elements and dict values each go by reference (exists). Dataclass fields still cannot hold references (out of scope).
- **Why Go needs something written**: a Go struct is data and can also have methods, and the value alone does not tell which, so going by reference needs `rutis.Ref(v)` (an existing rule). Go only has to learn to receive.
- **Why Swift and Kotlin need an annotation**: these languages cannot list a class's methods at run time; `@RutisService` generates the method table at compile time. It is the annotation already used to provide services.

## 4. What the receiver gets

A received object is the same kind of thing as the service proxy from `ctx.use`: **each method is an async cross-process call on the original object; there are no data fields**. To get data, have a method return it.

| Language | How | What it gets |
| --- | --- | --- |
| JS (Node, Bun) | Nothing to declare | A proxy: `await cache.get('a')`; every name read on it is a method (except symbols, `then`, `toJSON`, `toString`, `valueOf`, so awaiting a proxy gives the proxy); `console.log` shows `[remote object]` |
| Python | Nothing to declare | A proxy: `await cache.get("a")`; any public name is a method (as with `ctx.use`); its `repr` is `<remote object>` |
| Go | Declare the parameter as a struct of function fields (the same as a `ctx.Use` target, with fields mapped to method names by the same rule); `any` receives `*rutis.Service` | Each function field of the struct calls the matching method (blocking, first argument `context.Context`). If the struct has fields that are not functions, the call fails with `bad-argument`. `*rutis.Service` is the type `ctx.Use` already gives: `Call(ctx, method, result, args...) error`; for an object, `Methods()` returns `nil` |
| Swift | Declare the parameter as a `@RutisClient` protocol (`any Cache`); `Any` receives `Rutis.Client` | The same proxy `ctx.use(name, CacheProxy.self)` uses (`CacheProxy.rutisMake(client)`). `Rutis.Client` is the type inside that proxy: `call(_:values:as:)` |
| Kotlin | Declare the parameter as a `@RutisClient` interface; `Any?` receives `RemoteObject` | The same proxy KSP generates; `suspend` methods suspend, plain methods block (as with service clients). `RemoteObject` mirrors `RemoteFunction` and has a by-name `call`: Kotlin has no public by-name client, so it is the only language that adds a type |
| Rust | Hand-written host dispatch and Cordis bindings use `rutis_bridge::session::ObjectRef` (exists) | `call` / `call_async` |

- **How Swift recognises a client protocol**: not by deriving a name (`any X` → `XProxy`). `@RutisService` today writes `args.decode(i, as: (any P).self)` for every `any P` parameter, and parameters such as `any Sendable` or `any Hashable` rely on it to decode as usual; deriving a name would make them fail to compile. Instead, `@RutisClient` also generates an overload `rutisObjectProxy(_: (any Cache).Type)` for its protocol; generated code calls `rutisObjectProxy`, and that overload is preferred over a generic fallback that keeps today's decoding. So only parameters of `@RutisClient` protocols receive objects, and other `any P` parameters do not change. Return types of `@RutisClient` methods use the same mechanism (today `ClientMacro`'s `call(..., as: (any Cache).self)` would fail).
- **Inside data**: objects inside arrays and dictionaries, and event arguments, arrive in Swift as `Any` → `Rutis.Client` and in Kotlin as `Any?` → `RemoteObject`. Kotlin's KSP interface parameters work through `Tables.clientFactory`, but nested types such as `List<Cache>` have no serializer; Swift's `[any Cache]` is not in 0.18 either.
- **Calls on objects skip the method table**: the wire reference is only `{id, home, kind, origin}`, so an object has no method table. Today Go's `Service.Call` and `bind` and Swift's `Client.call` report "has no method" when a method is missing from the table, and take `sync` from it; for an object they skip that step (`Methods()` of the `*rutis.Service` is `nil`, the `Rutis.Client`'s method table is empty). Calls go to the object as `call` frames (not a service's `invoke`); `sync` is used only by Go's blocking struct fields, as `RemoteFunction` does with `Call(ctx, args, true)`. A missing method is answered with `no-method` by the end holding the object.
- **The same object**: an object handed over several times gives the receiver the same proxy (JS `===`, Python `is`); a proxy handed back to its process is the original object there. Handed to a third process, the host forwards it as usual. How each language hands a proxy on:
  - JS, Python: the proxy is the reference and goes out as is.
  - Go: only `*rutis.Service` keeps identity and goes back as the original object. This needs a hook from package `rutis` into the passthrough in `convert`'s `toPeer` (`encode.go`, around line 90), like `PluginToPeer` / `PluginFromPeer` (`internal/convert` cannot import `rutis`). A struct of function fields is a struct and is sent as data (each function field becomes a new function reference), and function fields cannot be compared; to hand the object itself on, take the parameter as `any`.
  - Swift, Kotlin: when the encoder sends a client proxy that wraps an object, it sends that object's reference. This needs `RutisProxy` to expose its `rutisClient`; in Kotlin, the proxy returned by the KSP-generated `_RutisClient.create` implements an internal marker exposing its handle and object reference, recognised in `Native.fromNative` before `objectOf`.
- **In the same process** the receiver gets the object itself (Cordis semantics): methods stay synchronous and fields stay readable. Across processes there are only methods, and all of them are async. This is written down as a boundary rule (principle 7). The testing tool's strict mode (the default) hands out proxies shaped as across processes, so the difference shows up early.
- **Call semantics** are those of function references: arguments and return values cross by "How values cross"; errors thrown by the other side come back as usual; cancellation reaches the other side as usual ([diagnostics](design-diagnostics-2026-10-26.en.md) §6.1). Methods of Go objects may be called concurrently (an existing rule).

## 5. Lifetime

- **The same as function references**: every hand-over counts once; when the receiver's proxy is no longer referenced (each language uses what it uses for function references: JS `FinalizationRegistry`, Python `weakref.finalize`, Go `runtime.AddCleanup`, and so on) it sends `Release`; at a count of 0 the sender lets the object go. An object passed as an event argument still lives until that call ends.
- **Not tied to the row**: after the row that handed the object over unloads, the object still exists and can still be called, as with Cordis objects in one process. To disable it on unload, the author does so in a cleanup (as with functions). Plugin definitions are tied to the row because copies are mounted from them; objects have nothing like that.
- **Idle stop**: on-demand runtimes (Go, Swift) stop `idle` seconds after their last row unloads (exists). Objects of theirs held elsewhere do **not** keep them running: whoever holds a reference is not a row, and the host does not decide a process's life by references, as with function references. After the stop, calls on those objects fail with `peer-gone`, saying the runtime stopped because it was idle. To keep them usable, keep the row that handed them over loaded, or make the runtime `eager`.
- **Process gone**: after the sender's runtime exits, disconnects or stops for idleness, calls on the proxy fail with code `peer-gone`, and so do calls in progress. Only that call fails; later calls on the proxy fail the same way. Nothing switches to a new process (the new process does not have the object).
- **Holder's process exits**: when the side holding a proxy exits without sending `Release`, the sender still lets go of the objects it handed to that session; they do not stay in memory. This is what function references do today and needs no new code: when a session closes, the host clears that session's exports and imports (`Connection::close`), the forwarded objects among them are dropped, and `Release` goes to the source session; each SDK also clears its own exports when its session closes (Node, Bun, Python, Go, Swift and Kotlin all do), which releases directly connected endpoint sessions.

## 6. Wire format and capability

- **The wire format does not change**: `{"type":"reference","value":{"id","home","kind":"object","origin"}}`; a method call is a `call` frame with `method`; the `get` frame (live property read) stays for Rust's `ObjectRef::get` and Cordis, and the new proxies do not use it. The reference has no class name or method list, the `origin` of a forwarded reference is mostly empty, and a local runtime's only far end is the host, so **the receiver does not know which runtime an object comes from** (§8).
- **Capability `objects`** (exists; only Rust declares it today):
  - Node, Bun, Python, Go, Swift and Kotlin declare it in both the endpoint and compat handshakes.
  - The 0.18 host adds `objects` to `COMPAT_DECLARED`. Local runtimes are all started with the compat format (`runtime/process.rs`); a 0.17 host only recognises the capabilities in `COMPAT_DECLARED`, which has no `objects`, so under a 0.17 host they always "do not take objects".
  - Runtimes still check the far end's `objects` only on endpoint sessions: on a compat session the far end is always the Rust host, which takes objects in every version. The host checks the **downstream** side when it forwards (exists).
- **Mixing with 0.17**:
  - Under a 0.18 host, an object to be forwarded to a 0.17 runtime: it does not declare `objects`, so the host refuses and this call fails with `bad-value`; the session continues. A 0.17 end never receives an object.
  - Under a 0.18 host, an object sent by a 0.17 runtime and forwarded to a 0.18 runtime: works now (it failed before).
  - 0.18 runtimes under a 0.17.2 host: the host does not recognise their `objects`, so forwarding an object between two runtimes is refused with 0.17.2's text (`warmer.warm: argument 1 cannot be sent: an object with methods cannot cross to py; pass plain data or a function instead`); only that call fails, and the session continues. A 0.18 runtime handing an object to the 0.17 host's own Rust code (`ObjectRef`, `live_objects.rs`) keeps working.
  - A 0.18 runtime connected directly to a 0.17 runtime over an endpoint session: the far end does not declare `objects`, so the sender refuses with its own existing text (§8); only that call fails.
  - No protocol version bump, no new capability name.
- **Waiting cycles of synchronous calls**: Go's blocking struct fields call an object's methods with `call` frames marked `sync`. The host's waiting-cycle check before forwarding a synchronous call (`bridge/sync-wait-cycle-refused`, #228 / #261) already covers them: forwarding an object call goes through the same `request_sync` → `send` → `waits::forward`, and incoming frames marked `sync` enter the wait graph whatever their kind. So when two runtimes call each other synchronously through objects, the call fails with `SyncWaitCycle` instead of hanging. Today's tests use only services and functions; O-1 adds one through objects.

## 7. `undefined`

Today the same value has two shapes in Python: when `[1, undefined, 3]` is an argument, the middle element is a special placeholder value; inside an object it is `None`. JS encodes arrays element by element (keeping the `undefined` tag) and encodes an object of plain data as one JSON value (`undefined` becomes `null`), and Python does not normalise on decoding. The Plugin API guide and V10 say `None`.

Rule (a fix to match V10):

- **As a whole argument**: keeps the meaning "not given", as today: for JS `f(1, undefined)`, Python receives one argument fewer or `None`, Go the zero value.
- **As a whole return value**: JS receives `undefined`; other languages their empty value.
- **Everywhere else** (inside arrays, objects, Maps, Sets at any depth, and holes in sparse arrays): always `null` / `None` / `nil`. The sender writes `null`; the receiver reads an incoming `undefined` tag as `null` too, so values from older ends agree.
- Result: `[1, undefined, 3]` at any level is `[1, None, 3]` in Python and `[1, null, 3]` in JS.

Holes in sparse arrays are values (as the Plugin API guide and today's code have it), not values that cannot be sent. [Diagnostics](design-diagnostics-2026-10-26.en.md) §8.1 row 6 uses a sparse array as its example of "a value cannot be sent", which contradicts this; this PR changes it to a value that really cannot be sent (`NaN`), with the argument number counted from 1.

## 8. What developers see

Error codes follow [diagnostics](design-diagnostics-2026-10-26.en.md) §4.3; codes do not appear in the text for people. The prefix (row name, method, argument position counted from 1) is written as in the diagnostics design.

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
// Swift: declare the parameter as a @RutisClient protocol; a service by name is ctx.use("cache", CacheProxy.self)
@RutisClient protocol Cache { func get(_ key: String) async throws -> Int?; func set(_ key: String, _ value: Int) async throws }
@RutisService final class Warmer { func warm(_ cache: any Cache) async throws -> Int? { try await cache.set("a", 1); return try await cache.get("a") } }
```

```kotlin
// Kotlin: declare the parameter as a @RutisClient interface
@RutisClient interface Cache { suspend fun get(key: String): Int?; suspend fun set(key: String, value: Int) }
@RutisService class Warmer { suspend fun warm(cache: Cache): Int? { cache.set("a", 1); return cache.get("a") } }
```

Who writes the text decides what it can say: the end holding the object knows its class and methods; the receiver knows only "an object"; only the host knows which runtime an object comes from (`far_name()`), so only text written by the host says `from node`.

| Case | What is shown | Written by | Code |
| --- | --- | --- | --- |
| Calling a method the object does not have | `Cache has no method gett (methods: get, set)`; for a plain object, `the object has no method gett (methods: get, set)`. When printed, the prefix of the diagnostics design is added: `chat: warmer.warm failed: Cache has no method gett (methods: get, set)` | The end holding the object, passed back as is | `no-method` |
| Reading a data field on a JS proxy as a method | `… Cache has no method size (size is a field; objects passed by reference expose only their methods)` | The end holding the object | `no-method` |
| The other process has exited | `chat: a call on an object from py failed: runtime py exited` | Host (a local runtime's far end is always the host; so are calls forwarded by the host) | `peer-gone`, `data`: `{runtime, reason}` |
| The other side stopped for idleness | `chat: a call on an object from go-netkit failed: runtime go-netkit stopped because it was idle` | Host | `peer-gone` |
| A 0.18 host forwarding to a 0.17 runtime | `chat: warmer.warm: argument 1 cannot be sent: an object from node cannot cross to py: py does not take objects (older than rutis 0.18); upgrade it` | Host | `bad-value` |
| Between 0.18 runtimes under a 0.17.2 host | 0.17.2's text: `warmer.warm: argument 1 cannot be sent: an object with methods cannot cross to py; pass plain data or a function instead` | 0.17.2 host | none (0.17 has no codes) |
| A 0.18 runtime sending directly to a 0.17 runtime over an endpoint session | The sender's existing text plus an upgrade hint: `warmer.warm: argument 1 cannot be sent: an object with methods (Cache) cannot cross to py; pass plain data or a function instead, or upgrade py to rutis 0.18` | Sender | `bad-value` |
| Swift class without the annotation | `warmer.warm: argument 1 cannot be sent: a Cache cannot cross: mark it @RutisService to pass it by reference, or make it Codable to copy it` | Sender | `bad-value` |
| Kotlin class without the annotation | `warmer.warm: argument 1 cannot be sent: a Cache cannot cross: annotate it @RutisService to pass it by reference, or @Serializable to copy it` | Sender | `bad-value` |
| Go parameter type that cannot take an object (not a struct, or a struct with data fields) | `chat: warmer.warm: argument 1 should be a string, got an object`; `chat: warmer.warm: argument 1 should be a Cache, got an object (an object is received only as a struct of function fields or any)` | Receiver | `bad-argument` |
| Swift / Kotlin parameter type that cannot take an object | `chat: warmer.warm: argument 1 should be a Cache, got an object (an object is received only as a @RutisClient protocol or Any)` (Kotlin: `interface or Any?`) | Receiver | `bad-argument` |
| An object reaching a parameter of a Rust service (`#[rutis::service]`) | `chat: store.put: argument 1 should be a Config, got an object from py; store takes data and functions, so pass a function for each method` | Host | `bad-argument` |

`no-method` data: service calls keep `{service, method, methods}`; an object call uses its object variant `{object, method, methods}`, where `object` is the class name (`null` for a plain object), filled in by the end holding the object. This PR adds that variant to the `no-method` row of [diagnostics](design-diagnostics-2026-10-26.en.md) §4.3, so #677's implementation follows it.

The "values are copied" hint in [diagnostics](design-diagnostics-2026-10-26.en.md) §10 becomes: "copied by value; objects with methods go by reference, and the other side can only call their methods".

## 9. Rust

- **Host side**: every object crossing runtimes is forwarded by the host (exists), so this column tests forwarding: between any two languages, back to its own process, `Release` reaching the source, `peer-gone` after the source exits.
- **Hand-written host dispatch and Cordis bindings** still receive objects and call methods with `rutis_bridge::session::ObjectRef` (in [Cordis](guide/cordis.en.md), TS interfaces with methods generate proxy structs wrapping `ObjectRef`). The sending rules do not change, so the values they receive do not change either.
- **Rust plugins do not send or receive their own objects in 0.18**: Rust cannot build an implementation of a trait at run time, so this needs a separate macro on traits, and there is no user for it today (criterion 4). Rust plugins still receive behaviour through function parameters (`impl Fn`). When an object reaches a `#[rutis::service]` parameter, the call fails with `bad-argument`, and the message suggests passing functions instead (§8).

## 10. Conformance cases

One shared expectations file (as for events and value types), one column per language; the testing tools' strict mode is checked against the same file. It is `crates/rutis-bridge/src/runtime/testing/objects.expected.json`. A test carrying the marker `objects-case(<language>): <case>` runs that case for that column; `crates/rutis-bridge/tests/objects_expected.rs` checks that every ✓ and "forward" cell of the languages implemented so far (`enforced` in the file) has a test, each stage adding its languages.

| Case | Node | Bun | Python | Go | Swift | Kotlin | Rust |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Send an object with methods, the other side calls a method, the original object's state changes | send / receive | send / receive | send / receive | send (`Ref`) / receive (struct, `*Service`) | send / receive (protocol, `Client`) | send / receive (interface, `RemoteObject`) | forward; receive with `ObjectRef` |
| A plain object with function-valued properties goes by reference | ✓ | ✓ | — | — | — | — | forward |
| Data stays data: plain objects / dicts / structs / `Codable` / `@Serializable` copied, functions inside by reference | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | — |
| Handed over twice gives the same proxy; handed back gives the original | ✓ | ✓ | ✓ | ✓ (`*Service`) | ✓ | ✓ | forward |
| Handed to a third language | ✓ | ✓ | ✓ | ✓ (`*Service`) | ✓ | ✓ | forward |
| As a return value, an element of an array or dict, an event argument | ✓ | ✓ | ✓ | ✓ | ✓ (return values as `@RutisClient` types; inside data and event arguments `Any` → `Client`) | ✓ (inside data and event arguments `Any?` → `RemoteObject`) | forward |
| Still callable after the sending row unloads | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | forward |
| Cancellation through a method call (the caller gives up, the other side is cancelled) | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | forward |
| Missing method: `no-method` at call time, only this call fails | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |
| Receiver drops the proxy: the sender's object is released | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | forward |
| The process holding the proxy is killed: the sender's object is released | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | forward |
| Two runtimes calling each other synchronously through objects: `SyncWaitCycle`, no hang | — | — | — | ✓ (blocking function fields) | — | — | ✓ |
| Sender's process killed: calls fail with `peer-gone`, the session and other rows are unaffected | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |
| Idle stop: after an on-demand runtime stops, its objects fail with `peer-gone`, saying it stopped because it was idle | — | — | — | ✓ | ✓ | — | ✓ |
| The other side does not declare `objects`: the call fails, the session continues | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |
| The declared type cannot take an object: `bad-argument` | — | — | — | ✓ (including structs with data fields) | ✓ | ✓ | ✓ (`#[rutis::service]` parameter) |
| The testing tool's strict mode hands out proxies shaped as across processes | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | — |
| `undefined` is `null` at any depth and kept as a whole argument | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |

Across versions: the release gate's cross-version meetings gain these cases: a 0.18 host forwarding an object to a 0.17 runtime (`bad-value`, the session continues); under a 0.18 host, a 0.17 runtime's object handed to a 0.18 runtime (works); objects between two 0.18 runtimes under a 0.17.2 host (only that call fails, with 0.17.2's text, and the session continues), together with a 0.18 runtime handing an object to the 0.17.2 host's `ObjectRef` (works). The [guarantees table](guarantees.md) gains `bridge/object-refs` (§3–§6), each entry naming the tests that check it.

## 11. Compatibility and migration

Goes into "Migrating to 0.18" (both languages):

- **Failed before, works now**: objects with methods sent to Node, Bun, Python, Go, Swift and Kotlin. The sending rules do not change, so what Rust hosts and Cordis bindings receive does not change.
- **`undefined`**: a fix to match the written rule (the Plugin API guide, V10): `undefined` inside arrays is `None` in Python and `null` in JS at any level. Before, an array passed as an argument kept a special value; that was a bug against the docs.
- Error texts change (§8). The wire format and the protocol version do not.

## 12. Docs to update

- In this PR: [diagnostics](design-diagnostics-2026-10-26.en.md) (both) §8.1 row 6 (§7); the §4.3 `no-method` row gains the object-call `data` `{object, method, methods}` (§8).
- [Plugin API](guide/plugin-api.en.md) (both): remove "only to Rust" from the "objects with methods, class instances" row of "How values cross" and write it as §3 and §4 of this design; add a paragraph on "the same process versus across processes" and idle stop (§5); rewrite the row of the testing tool's differences that says objects with methods always go by reference.
- Each language guide (both): [TypeScript](guide/typescript-plugin.en.md), [Bun](guide/bun-plugin.en.md), [Python](guide/python-plugin.en.md), [Go](guide/go-plugin.en.md) (struct-of-function-fields parameters, `any` receiving `*rutis.Service`), [Swift](guide/swift-plugin.en.md), [Swift App](guide/swift-app.en.md) (`@RutisClient` parameters, `Any` receiving `Rutis.Client`), [Kotlin App](guide/kotlin-app.en.md) (`@RutisClient` parameters, `RemoteObject`), [Rust plugin](guide/rust-plugin.en.md) (objects reaching service parameters), [rutis-host](guide/rutis-host.en.md) (`idle` ignores references).
- [Protocol design](design-protocol-plugins-2026-09-25.en.md): the meaning of `objects` in the compat handshake, the position rule for `undefined`.
- "Migrating to 0.18", the [guarantees table](guarantees.md).

## 13. Implementation

| Phase | Contents |
| --- | --- |
| O-1 | Specification and host: `objects` and the `undefined` rule in the protocol design; add `objects` to `COMPAT_DECLARED`; texts and codes for forwarding refusals, `peer-gone` (including idle stop), and objects reaching `#[rutis::service]` parameters (the code table comes with #677); Rust encoding and decoding write and read `undefined` in containers as `null`; the shared conformance expectations file and the Rust column, including tests for a waiting cycle through objects and for release after the holder is killed; guarantee entries |
| O-2 | Node, Bun: declare `objects`; the proxy for received objects; `no-method` text and `data`; the upgrade hint when refusing on an endpoint session; testing tool strict mode; `undefined`; guides |
| O-3 | Python, Go: declare `objects`; the Python proxy; Go struct-of-function-fields parameters and `any` receiving `*rutis.Service`, calls on objects skipping the method table, `*rutis.Service` handed on as is through the `toPeer` hook; testing tools; `undefined`; guides |
| O-4 | Swift, Kotlin: declare `objects`; `@RutisClient`-typed parameters and return values receiving objects (Swift through the `rutisObjectProxy` overload; the macro declaration at `Rutis/API/Macros.swift:10` becomes `@attached(peer, names: suffixed(Proxy), named(rutisObjectProxy))`), Swift `Any` receiving `Rutis.Client`, Kotlin `RemoteObject`; client proxies handed on send the original reference (`rutisClient`, `Native.fromNative`); calls on objects skip the method table; testing tools; guides; final rewrite of "Migrating to 0.18" and the Plugin API guide |

One PR per phase, merged in order; later phases reuse O-1's expectations file.

Completion (#687 closes when all are met):

- [ ] Every row of §10 passes in all seven columns, and the cross-version cases are in the release gate;
- [ ] Python's `undefined` agrees at the top level and when nested;
- [ ] The docs in §12 are updated in both languages.
