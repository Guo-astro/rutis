# Android app hosting (Kotlin SDK)

[中文](design-android-runtime-2026-10-12.md)

Status: finalized; K-P1 implemented (#390, `kotlin/rutis`), K-P2 implemented (#391, see §13), K-P3 implemented (#388: the sample plugins `contacts`, `notify`, `device` in `kotlin/rutis/android-sample`, the guide [Hosting plugins in an Android application](guide/kotlin-app.en.md); the permission prompt and denial checked automatically on an emulator, the real-device checklist in the guide's §8). Date: 2026-10-12; finalized and revised 2026-10-14 (now follows [app integration](design-app-integration-2026-10-14.en.md); see §10); aligned with the runtime contract of rutis 0.13 on 2026-10-19 (values, automatic exposure, errors, the calling row, unified events, developer transparency; see §11). Baseline: `main` `028783f` (0.13.0).
Based on: [the runtime contract](../.agents/skills/rutis-new-runtime/references/runtime-contract.md) (implementations follow it), [app integration](design-app-integration-2026-10-14.en.md) (the general model), [remote runtimes: dial-out and dynamic access](design-remote-runtime-dial-2026-10-14.en.md) (connections), [cross-language value types](design-value-types-2026-10-15.en.md), [the unified events protocol specification](design-events-protocol-2026-10-18.en.md), [the multilingual design](design-multilang-runtimes-2026-10-03.en.md), [the plugin API](guide/plugin-api.en.md), [the Swift SDK](design-swift-sdk-2026-10-12.en.md) (the Swift counterpart of shape ②), [the Go plugin runtime](design-go-runtime-2026-10-10.en.md) (the precedent for one execution unit per call).

Positioning: an Android app provides its own capabilities and those of the device it runs on as rutis plugins; the plugins are written in Kotlin, compiled into the app, and driven by rutis. This is the only current design of the Kotlin SDK: the desktop-JVM runtime ([the Kotlin runtime](design-kotlin-runtime-2026-10-12.en.md)) is suspended and is not a premise of this document; K-P1's jvm target only serves to run the conformance tests on a desktop.

Out of scope: loading new code on the device (DEX download, upload, hot swap); app foreground/background, foreground services, Doze handling (business layer; see app integration §7); Intents and other Android-ecosystem calls back into rutis; running rutis itself on the device; the Play compliance process itself; a host-side local `kt:` runtime (the suspended desktop runtime); receiving object references (the capability `objects`, as in Go).

## 1. Decisions

| # | Decision | Section |
| --- | --- | --- |
| K1 | Plugins are written in the rutis style and compiled into the app (APK / AAB); rutis drives loading, configuration, and unloading | 2 |
| K2 | The SDK offers the app only `start` / `stop`; foreground services, Doze, and vendor background restrictions belong to the business layer | 5 |
| K3 | Remote connections are supported in both directions: dialing out to a rutis access point (recommended; each connection is one instance on the rutis side) and listening (rutis dials in); non-loopback must be `wss://`, with no plaintext exception; dialing follows contract §1 | 5 |
| K4 | The SDK is a Kotlin library with two targets, android and jvm; the jvm target serves conformance tests on the desktop (the `Rutis.serve` test entry) and is the basis of a future desktop runtime | 7 |
| K5 | One platform thread per incoming call; `suspend` methods are bridged with `runBlocking` on that call's thread; the call chain lives in a `ThreadLocal` carried into coroutines with `asContextElement()`, and plugin code passes no context; declares `sync-wait` and `reentrant-sync`, not `sync-stack` | 4 |
| K6 | No runtime reflection of methods: a KSP processor (`rutis-ksp`, compile time only) generates method tables for `@RutisService` classes and proxies for `@RutisClient` interfaces; the config schema derives from the `@Serializable` config type's `SerialDescriptor`, with nothing written by hand | 3 |
| K7 | Values: the SDK's own kotlinx-serialization format (`RutisFormat`, never `Json`'s default encoding) checks every value; all ten tagged values are implemented and `value-<tag>` declared; integers beyond ±(2^53−1) go as `bigint` automatically; `@RutisService` instances and functions go by reference, other `@Serializable` values as data | 3 |
| K8 | Runtime dependencies: kotlin-stdlib, kotlinx-coroutines-core, kotlinx-serialization-core and -json (frames), OkHttp (WebSocket client), Java-WebSocket (WebSocket server, pure Java); plus the KSP processor at compile time | 7 |
| K9 | System permissions belong to the app; not granted = `apply` or a method call fails; when a runtime permission is revoked while running, the system kills the app process, which rutis sees as a disconnect | 6 |
| K10 | Platform: Android 10 (API 29) and later; the jvm target on JDK 17; pure Kotlin, no native code | 7 |
| K11 | The host side only needs `runtimes.remote[].language` to accept `"kotlin"` / `"kt"` (implemented); everything else follows the dial-out design | 8 |
| K12 | Plugin API version **3**; unified events `ctx.on` / `emit` / `parallel` / `serial` / `waterfall` / `bail` / `filter`, capability `events-v2`; every listener registers as `kind: "sync"` (one thread per call, as in Go) | 3.4 |
| K13 | Services are exposed automatically: `ctx.provide(name, obj)` needs no `provides`, undeclared services are announced with `rows.announce`, every method `async`; a method's kind is a declaration (default `async`), independent of `suspend` | 3.2 |
| K14 | Errors carry `name` / `message` / `code` / `data` and the error graph in both directions; received errors and the SDK's own errors are `RutisException`, matched by `name` | 3.5 |
| K15 | The calling row: each row uses its own service ids from `rows.load`'s `isolate`; scope labels are opaque and only concatenated as given | 4.2 |
| K16 | The testing tool `arcships.rutis.testing` runs plugins in the test process over a real session (an in-memory channel), with the test as the host; it is checked against the same `weather.expected.json` and `events.expected.json` as the real runtime | 9 |

## 2. Model

```text
rutis (desktop / server)                  Android app
 ├─ access point (listening, wss) ◀──dial── ├─ business code: decides when to start / stop
 ├─ one instance per connection             ├─ rutis Kotlin SDK
 │    └─ rows phone:contacts, phone:notify  │    └─ plugins compiled into the app: contacts / notify / device …
 └─ other rows inject these services by name └─ Android APIs (permissions belong to the app)
```

- Plugins within the same app call each other on the objects themselves; across connections and languages, calls go through rutis by name.
- rutis does not model devices: model, OS version, and the like are provided by plugins (such as a `device` plugin).
- App process death, network loss, `stop()`: to rutis these are all a disconnect — the instance closes, services are withdrawn, consumers stop; after reconnecting, plugins are loaded again according to rutis's current configuration.
- Events: listeners of plugins in the app register on the host's logical bus and are reached by any emitter like listeners in other languages (by scope); the connection's instance is the scope of these rows.

## 3. Plugin API

```kotlin
import arcships.rutis.*
import kotlinx.serialization.Serializable

@Serializable
data class Config(val city: String)            // the schema derives from it

@RutisClient                                    // a service used: an ordinary interface
interface Llm {
    suspend fun ask(question: String): String
}

@RutisService                                   // a service provided: an ordinary class
class Weather(private val llm: Llm, private val city: String) {
    suspend fun today(question: String): String = llm.ask("weather in $city: $question")
    fun unit(): String = "celsius"
}

val weather = Rutis.define<Config>(
    name = "weather",
    inject = listOf("llm"),
    provides = mapOf("weather" to service<Weather>(sync = setOf("unit"))),   // optional: without it the service is exposed automatically, every method async
) { ctx, config ->
    val llm = ctx.use<Llm>("llm")
    ctx.provide("weather", Weather(llm, config.city))
    ctx.on("session/created") { args -> println("new session ${args[0]}") }
    ctx.effect { /* runs in reverse order on unload */ }
}
```

| API | Rule |
| --- | --- |
| `Rutis.define<C>(name, inject, provides?, schema?) { ctx, config -> }` | Validates the declaration (names; the methods in `provides` exist in the class's method table); carries plugin API version 3; `C` is a `@Serializable` type (`Rutis.NoConfig` for none) |
| `apply` | May be suspend; throwing fails the load, and already-registered cleanups still run |
| `ctx.use<I>(name)` | `I` is a `@RutisClient` interface; `name` must be in `inject`. The service is looked up by this row's own id (§4.2): a same-process provider implementing `I` is returned itself, otherwise a proxy (through the local method table in the same process, through rutis across processes). The interface's methods must be methods the service provides; otherwise `use` throws, naming the method |
| `ctx.provide(name, obj)` | `obj` is an instance of a `@RutisService` class; a declared service exposes only its declared methods, an undeclared one is announced automatically (§3.2); lasts until unload or until the returned withdraw function is called |
| `ctx.effect(cleanup)` | May be suspend; runs in reverse order on unload |
| `ctx.on` / `emit` / `parallel` / `serial` / `waterfall` / `bail` / `filter` | Unified events, §3.4 |
| `ctx.scope` | The row's scope chain `List<Scope>` (`rows.load`'s seventh argument) |
| `ctx.launch { }` | Runs in the row's coroutine scope (with the call chain); cancelled and awaited on unload |
| Unload signal | The row's coroutine scope is cancelled on unload: `isActive` / `ensureActive()`, Kotlin's own cancellation |

### 3.1 Method tables and proxies from KSP

Android does not use method reflection: R8 renames and strips, `suspend` functions seen through Java reflection have a `Continuation` signature, and parameters' generic types are incomplete. So method tables and proxies are generated at compile time by KSP (as Room and Moshi codegen do):

| Item | Rule |
| --- | --- |
| `@RutisService` | Generates `<Class>_RutisService`: the method table (wire name → decode the arguments with `RutisFormat` and serializers chosen at compile time, call, encode the result) and the list of method names |
| Which methods | `public` member functions of the class and its superclasses (`Any`'s `equals` / `hashCode` / `toString` do not count; properties do not count), the counterpart of Go's "exported methods"; the wire name is the function name; overloads of one name are a compile error |
| `@RutisClient` | Generates the proxy `<Interface>_RutisClient`: `suspend` functions suspend to wait; plain functions block the current thread to wait (plugin code runs on per-call threads; calling one on the Android main thread throws, to avoid ANRs) |
| Lookup | `ctx.provide` / `ctx.use<I>()` find the generated class by name (`Class.forName("<name>_RutisService")`, as Room and Moshi do); the SDK's AAR ships consumer R8 rules keeping the names of generated and annotated classes |
| Dependency | The app's `build.gradle.kts` adds `ksp("io.github.arcships:rutis-ksp:<version>")` (written by the `new` template and the guide); the processor is not in the APK |

### 3.2 Services, method kinds and automatic exposure

- A method's kind is a declaration, not the Kotlin signature: methods listed in `service<Weather>(sync = setOf("unit"))` are `sync`, everything else, and every method of an undeclared service, is `async` (contract §4); `mapOf("today" to Kind.Async, "unit" to Kind.Sync)` also works. Whether a function is `suspend` only decides how the implementation runs on the call's thread.
- Automatic exposure: a service from `ctx.provide` not in `provides` is announced by contract §4 as `service(id, handle, version, { row, methods })`, the methods taken from the generated method table, all `async`; only to a host whose `mount` carried `announce: true`, and `rows.load` is answered once the replies arrived. The reply is always `undefined`: on a name clash the host fails the row itself, as the later provider, naming both, once `rows.load` is answered; waiting for the replies is what lets it do so before the row counts as loaded, and the SDK reads no error from them; with a host older than 0.12, the one-time stderr notice of the contract applies.
- `announces` is always `true` in the `rows.schema` reply and in `mount`'s `plugins`.
- A call to a method the peer declared `sync` carries `sync: true` (the kind comes from the method table of `hosts.provide`) and takes part in the host's wait-cycle detection.
- Calls in the same process: the object is called directly (no encoding) when it implements the user's interface; otherwise through the local method table.

### 3.3 Values

The value layer is the SDK's own kotlinx-serialization format `RutisFormat` (a custom `Encoder` / `Decoder`), driven by serializers generated at compile time, checking every single value; it never goes through `Json`'s default encoding (which encodes dates as numbers or strings and binary as arrays, and lets class internals bypass checks: Go's problem in #355). The rules follow [the value types design](design-value-types-2026-10-15.en.md) and contract §5.

| Kotlin value | Across processes |
| --- | --- |
| `null`, `Boolean`, `String`, `Double` / `Float` | Copied; NaN and ±Infinity fail the call |
| Integers `Int` / `Long` / `ULong` … | Numbers within ±(2^53−1); beyond it, **automatically** `bigint` (V6) |
| `List` / `Array`, `Map<String, *>`, `@Serializable` types | Copied recursively as data (field names per `@SerialName`); a `Map` with string keys is an object |
| `java.time.Instant`, `kotlin.time.Instant` | `instant`; nanosecond precision, years 0001–9999 |
| `java.time.LocalDateTime` / `LocalDate` / `LocalTime` | `datetime` / `date` / `time` |
| `ByteArray` | `bytes` |
| `Map` with non-`String` keys | `map`; restrictions on keys and `Set` elements per contract §5 (instants, `ByteArray`, `null`, lists, maps, sets and references as keys are refused before sending) |
| `Set` | `set` |
| `java.math.BigInteger` | `bigint`; a received `bigint` decodes into a declared integer type that holds it, and fails the call when none does |
| `java.math.BigDecimal` | `decimal` (the scale, i.e. trailing zeros, is kept; written with `toPlainString()`, no exponent) |
| `java.util.UUID`, `kotlin.uuid.Uuid` | `uuid` (lowercase) |
| Functions (`suspend` or plain function types) | By reference; arguments and results encoded by this table |
| Instances of `@RutisService` classes | By reference; the far end gets a proxy calling its method table |
| Exceptions | As a value, copied as `{ name, message }`; thrown, see §3.5 |
| Parameters declared `Any?` | Decoded back into native values by this table: integers as `Long`, floating point as `Double`, objects as `Map<String, Any?>`, lists as `List<Any?>`, `map` as `Map<Any?, Any?>`, `set` as `Set<Any?>`, `bigint` as `BigInteger`, `decimal` as `BigDecimal`, times as `java.time` types, functions as `RemoteFunction` (`suspend operator fun invoke(vararg args)`) |
| Other objects (neither `@Serializable` nor `@RutisService`), cycles | Cannot cross: the call fails, naming the type |

- When fields of a `@Serializable` class are `java.time`, `BigInteger`, `BigDecimal` or `java.util.UUID`, kotlinx requires `@Contextual` on them (kotlinx's own rule; `RutisFormat` provides those serializers); `kotlin.time.Instant` and `kotlin.uuid.Uuid` need nothing. Method parameters and results need no annotation at all (the generated code chooses the serializers directly).
- Nesting is at most 64 levels, checked by the sender before sending; anything in a value breaking a rule fails only that call, and the session continues.
- Each tag goes only to a far end that declared `value-<tag>`; otherwise the call fails, naming the value. The SDK declares all ten `value-<tag>` in both handshakes.
- A malformed tagged value or key received fails only that call; the rest is still decoded, so every reference in it is imported and released.
- `undefined` inside data arrives as `null`.

### 3.4 Events

Unified events follow [the protocol specification](design-events-protocol-2026-10-18.en.md) and contract §6. The API (Go's shape: a listener gets the argument list, with `next` last):

```kotlin
val off = ctx.on("greeting/format") { args -> "${args.next!!() ?: args[0]}!" }
ctx.on("rutis/row", prepend = true) { args -> println(args[0]) }

ctx.emit("t/ping", 1, "a")                                  // returns at once; throws only for ReservedEvent or arguments that cannot be sent
ctx.parallel("t/ping", 1)                                    // suspend; failure: RutisException(name = "AggregateError")
val first = ctx.serial("t/ask", "q")                         // suspend; Any?
val text = ctx.waterfall("greeting/format", "hi", terminal = { args -> args[0] })
val vote = ctx.bail("t/check", 42)                           // blocks this thread; synchronous on the wire: sync: true, in wait-cycle detection
val some = ctx.filter { d -> d.row == "chat" }               // Emitter: the same five methods
```

| Item | Rule |
| --- | --- |
| Listeners | `suspend (EventArgs) -> Any?`; `EventArgs` is the `List<Any?>` of arguments decoded into native values (§3.3's `Any?` rule); in `waterfall` its last element is the `Next` (`suspend () -> Any?`), also `args.next`; `args.decode<T>(i)` decodes argument `i` into a `@Serializable` type; the return value is answered as is |
| Options | `prepend`, `global`; returns a function that deregisters at once; deregistered when the row unloads |
| Registration | `ctx.on` sends `events.on [{ id, name, row, plugin, kind: "sync", prepend, global }]` (`id` increasing within the session, never reused); `rows.load` is answered after every registration made during `apply` was answered; `internal/*` is refused locally, and a refusal by the host is reported, or fails the load, per the contract |
| `kind` | Always `"sync"`: one thread per call, so a listener (a `suspend` one included) runs to the end with `runBlocking` on that thread before the reply, and the host can wait for it synchronously (as Go). Kotlin listeners therefore never cause `AsyncListener` |
| Fast path | Dispatch in this process when the name is not in `events.elsewhere`, no prefix matches, and (for `emit`) no earlier `emit` of the name sent through the host is unanswered; otherwise send `events.dispatch`. Descriptors of this process's entries use `mount`'s `runtime` |
| Dispatch rules | Snapshot (name, registration order, scope chain prefix, live) → filter (non-`global`) → check liveness before each start; the five modes and "no result" (`null`, `false`) per contract §6 |
| `emit` order | One serial queue per listener: `emit` invocations on one listener run one after another in arrival order; different listeners run concurrently |
| Filters | On the fast path, called directly on each descriptor; through the host, `filter` is a selector function reference that finds this process's entries by the descriptor's `listener` and removes that field before calling the plugin's predicate; descriptors are `Listener` (`name`, `prefix`, `plugin`, `row`, `runtime`, `node`, `scope`, `global`, `null` when absent) |
| Cancellation | The emitting coroutine is cancelled: through the host, a `cancel` is sent for the `events.dispatch`; an aborted `signal` or a `cancel` for an `events.call` cancels that listener's coroutine and interrupts its thread |
| Reserved names | Emitting `rutis/*` is refused synchronously with `ReservedEvent` (`rutis/* events are emitted by rutis only: <name>`); `rutis/*` can be listened to |
| Reporting | Failures nobody waits for write one line each with the contract §6 wording (stderr on the jvm; `RutisApp` writes to its `log` callback, `System.err` by default) |
| Unload | After `rows.unload`, refuse the row's `events.call` at once → withdraw provided services → cancel the row's running listeners and wait for them, at most the shutdown deadline (jvm `RUTIS_SHUTDOWN_TIMEOUT`, `RutisApp`'s `shutdownTimeout`, 10 seconds by default), writing `rutis: listeners of row <row> still running after <s>s; unloading anyway` past it → cleanups in reverse order |
| Disconnection | `elsewhere` becomes empty; every row unloads |

### 3.5 Errors

| Direction | Rule |
| --- | --- |
| Sending (the plugin throws) | `name` = the exception class's simple name (`javaClass.simpleName`); `message`; the `cause` chain goes into the error graph; when the exception has public properties `code: String` and/or `data` (read through the two getters `getCode()` / `getData()`: plain Kotlin, no rutis type needed; the SDK's consumer R8 rules keep the two getters), they are sent as `code` and `data`; `data` that cannot cross is left out and the rest sent |
| Receiving | `RutisException`: `name`, `message`, `code: String?`, `data: Any?` (decoded as a value), `cause`, `errors` (the children of an `AggregateError`, in registration order); matched by `name` (`e.name == "SyncWaitCycle"`), not by class |
| Rethrowing | A received `RutisException` thrown again carries its whole error graph (`code` / `data` included) across once more |
| The SDK's own errors | Also `RutisException`, with the contract's names: `AggregateError`, `AsyncListener`, `WaterfallNext`, `ReservedEvent`, `SyncWaitCycle`, `BindingError`; constants `RutisException.SYNC_WAIT_CYCLE` and so on |

### 3.6 Configuration

| Item | Rule |
| --- | --- |
| Type | A `@Serializable` type, decoded with `RutisFormat` from `rows.load`'s configuration value; a failure fails the load, naming the field |
| Schema | Derived from the type's `SerialDescriptor` (generated at compile time, no reflection): primitives, lists, maps, nested classes, nullables, fields with defaults (not in `required`), enums (`enum`); `define(schema = …)` takes precedence when given |

## 4. Threading and session

### 4.1 Threads

| Item | Rule |
| --- | --- |
| Session structure | One reader thread parses frames; frame writes serialized by one lock; each incoming call gets a new platform thread (ART has no virtual threads; the jvm target uses the same code); `events.on` / `events.off` / `events.elsewhere` are handled in place on the reader thread, in frame order |
| Outgoing calls | Plain functions: the current thread blocks on a `CompletableFuture` that the reader thread completes; `suspend` functions suspend on the same future; calls to the peer's `sync` methods and `bail` carry `sync: true` |
| Reentrancy | An incoming call always has a new thread to run on; declares `reentrant-sync`; a plugin's service may be called again in the middle of its own synchronous call, so do not hold locks while calling rutis services |
| Capabilities | `sync-wait`, `reentrant-sync`; `sync-stack` not declared (concurrent threads, taking part only in call-chain edges) |
| suspend bridging | `runBlocking` on the call's own thread; never bridged on the reader thread |
| Call chain | A `ThreadLocal`: an incoming call sets `path + [id]` on its thread; put into the coroutine context with `asContextElement()`, so it follows `withContext` across threads; threads a plugin starts itself with `Thread { }` have no chain (in the guide: use `ctx.launch` or coroutines) |
| `sync` methods | The implementation may be `suspend` and may call other services; the same as Go |
| Cancellation | A `cancel` frame or an aborted `signal` → cancel that call's coroutine scope and interrupt its thread; when the calling coroutine is cancelled, an `async` call sends `cancel` |
| `SyncWaitCycle` | Never produced here; received as a `RutisException` with `name == "SyncWaitCycle"` |
| Uncaught exceptions on plugin threads | The app crashes: rutis sees a disconnect |

### 4.2 Contract

compat protocol 2 (the local channels of the jvm test entry), endpoint protocol 3 (dialing out and listening).

| Item | Value |
| --- | --- |
| Capabilities (in both handshakes) | `signals`, `sync-wait`, `reentrant-sync`, `events-v2`, the ten `value-<tag>`; the endpoint handshake also declares `runtime`; `sync-stack` and `objects` are not declared |
| Features (`mount` reply) | `["rows.v2", "hosts", "leaf", "scopes", "rows.announce"]` |
| `mount` | Records `announce` and `runtime`; the reply carries `plugins` (dial-out design §4, each like a `rows.schema` reply), `implementation: { name: "io.github.arcships:rutis", version }`, `engine` (`kotlin` + the ART / JVM version) |
| `rows.schema` | `{ config, inject, provides, version, announces: true }` |
| `rows.load(key, entry, config, isolate, inject, exports, { scope, id })` | Check the plugin API (a plugin newer than the SDK is refused) → decode the config → `apply`; declared services are reported with a three-argument `service`, undeclared ones announced with four arguments, waiting for the answers; answered after every `events.on` sent during `apply` was answered; on failure unload first, then report; `id` is how descriptors and report lines call the row |
| `rows.update` / `rows.unload` / `hosts.*` / `release` / `get` / `dispose` | As Go; the unload order is §3.4's |
| Call ids | Prefixed `node:` |
| Old hosts | When the far end's `hello` does not declare `events-v2`, the session ends right after the handshake with `rutis: refused: incompatible (this host does not declare events-v2 (the event protocol of rutis 0.13): upgrade rutis-host to 0.13 or later); stopping`; the jvm entry exits with status 1, `RutisApp` stops reconnecting with `state` `Stopped` carrying that reason |
| Session end | Every row unloads; cleanup as a whole is bounded by the shutdown deadline (10 seconds by default), and past it one line is written; the jvm entry then exits with 1, `RutisApp` returns to disconnected and reconnects by the dialing rules |

- **The calling row**: the host leases each row's services into a scope of that row's own (a label with U+0001 in it), so that calls to `host:<id>` tell it which row calls. For each row the SDK computes the service ids the row uses from `rows.load`'s `isolate` (name → label): the name, or the name + NUL + the label; `ctx.use` uses only that row's own ids; labels are opaque, never parsed or compared by content, only checked for containing no NUL and not being empty. Code runs in a row, so its calls target that row's ids (the row's coroutine scope carries the row).

### 4.3 Channels

| channel | target | Implementation |
| --- | --- | --- |
| `unix:<path>` or a bare path | jvm | Dial back: JDK 16+ `UnixDomainSocketAddress` (the test entry) |
| `tcp:127.0.0.1:<port>` | jvm | First send `RUTIS_CHANNEL_TOKEN` + newline (the test entry; Windows) |
| `dial:wss://…` | android + jvm | An OkHttp WebSocket client, subprotocol `rutis.3`; the dialing rules of contract §1 (Bearer `RUTIS_TOKEN`, `RUTIS_CA`, a non-loopback `ws://` and an invalid id refused at start, classified by status / close code, every row unloaded in reverse on each disconnect, the stderr state lines word for word, `RUTIS_RETRY` for tests) |
| `listen:wss://…` | android + jvm | A Java-WebSocket server; non-loopback requires `wss://`; `RUTIS_TOKEN` compared in constant time; 30-second heartbeat; one controller at a time, a new connection takes over |

`fd:3` is not implemented: the JDK has no public API wrapping an inherited fd, and it only serves local runtimes the host starts (out of scope here). Newline framing, messages at most 16 MiB. The jvm test entry `Rutis.serve(args, plugins…)` takes Go's arguments: `<channel> [--id <endpoint>] [--peer <endpoint>] <project>`.

## 5. App hosting

```kotlin
val rutis = RutisApp(
    plugins = listOf(contacts, notify, device),
    connect = "wss://rutis.example.com:7443/rutis",
    id = "kim-pixel",                       // self-reported id (dial-out design §5)
    token = secrets.rutisToken,
)

rutis.start()   // connect; from here rutis loads plugins according to its configuration
rutis.stop()    // clean up all plugins in reverse order, then disconnect
```

| Item | Rule |
| --- | --- |
| Parameters | `plugins`, `connect` (or `listen`), `id`, `token`; optional `peer`, `trust` (additional CAs, the semantics of `RUTIS_CA`), `shutdownTimeout` (10 seconds by default), `log` (where report and state lines go) |
| `start()` | Connects, greets, reports `plugins`; after a disconnect, reconnects automatically by the dialing rules (contract §1) until `stop()`; rejected credentials retry every 30 seconds; incompatibility (a host without `events-v2` included) stops and puts the reason in `state` |
| `stop()` | Unloads all rows (cleanups in reverse order) and closes the connection; `start()` may be called again |
| State | `rutis.state` (`Stopped(reason?)` / `Connecting` / `Connected`, a `StateFlow`) for the business layer to display; the SDK makes no lifecycle decisions based on it |
| Certificates | Non-loopback must be `wss://`; for a self-signed certificate, trust its CA through `trust` |
| Listening | `RutisApp(listen = "wss://0.0.0.0:7443/rutis", certificate = …, …)` instead waits for rutis to dial in, with the same semantics as Go's remote listening (one controller at a time, a new connection takes over); suits devices with a fixed address; dialing out is recommended in general |
| Changing code | Ship a new version of the app |
| Business-layer reference | When it needs to stay resident, the app runs a foreground service (from Android 14 the foreground service type must be declared) and guides the user to the battery-optimization allowlist; otherwise it follows the UI with `start` / `stop`. This goes in the SDK guide, not in the SDK |

## 6. Permissions

| Case | Behavior |
| --- | --- |
| Not granted at load time | The plugin checks in `apply` and throws, with the error naming the permission; the row fails to load |
| Not granted at call time | The method throws; the error goes back to the caller on the rutis side (it may carry a `code`, such as `"permission-denied"`) |
| Runtime permission revoked while running | The system kills the app process → disconnect → the instance closes; after the app restarts and reconnects, plugins are loaded again according to the configuration, and the `apply` of plugins without the permission fails |
| A special permission (notification listener, accessibility, etc.) turned off | The plugin notices on its own and withdraws the services it provides (the withdraw function returned by `ctx.provide`), or fails on method calls |

Permission prompts point at the app (corresponding to Swift shape ②). The SDK does no permission reconciliation; permissions are the plugin's own business.

## 7. SDK and packaging

| Item | Rule |
| --- | --- |
| Coordinates | `io.github.arcships:rutis` (Kotlin Multiplatform: android + jvm) and `io.github.arcships:rutis-ksp` (the KSP processor); published to Maven Central; versioned with the main release line, in `scripts/train.mjs`; first version 0.15.0 (0.14 is not released on its own) |
| Repository location | `kotlin/rutis` (a Gradle project: `rutis`, `rutis-ksp`, the conformance entry `rutis-conformance`) |
| Packages | `arcships.rutis` (API); session layer `arcships.rutis.internal.peer`; testing tool `arcships.rutis.testing` |
| Dependencies | K8; plugin authors depend on this library only (and the KSP processor at compile time) |
| Platform | minSdk 29; the jvm target on JDK 17; pure Kotlin, no ABI restriction |
| R8 | The AAR ships consumer rules: keep the names of `*_RutisService` / `*_RutisClient` and of annotated classes, and exceptions' `getCode()` / `getData()` |
| Docs | `docs/guide/kotlin-app.md` (Chinese and English); `docs/guide/plugin-api.md` gains a Kotlin column |

## 8. The host side

| Change | Location |
| --- | --- |
| `runtimes.remote[].language` accepts `"kotlin"` / `"kt"` → `RuntimeResolver::modules` | `rutis-host/src/host.rs` (implemented) |
| Conformance: the event expectation file's languages gain `kotlin`; `session_matrix.rs`, `events_conformance.rs`, `multilang.rs` gain Kotlin (a test feature `kotlin`, starting the `rutis-conformance` jar through the generic `Launcher::new("java")`; no `LocalRuntime::kotlin`, no host configuration) | `rutis-bridge/src/runtime/testing/events.rs`, tests |
| Everything else (access point, connection-as-instance, identity layer) | Dial-out design §7 |

For a configuration example, see dial-out design §4 (with `language` set to `kotlin`).

## 9. Tests and phases

The runtime contract's conformance tests are the contract itself (contract §8); Kotlin passes the same suites as every other runtime.

| Test | Location | Content |
| --- | --- | --- |
| Session unit | `kotlin/rutis` (jvm target) | Encoding (the ten values, key rules, depth, out-of-range integers), reference counting, callbacks during synchronous calls, cancellation, call chains (across `withContext` thread switches too), concurrency, KSP-generated code |
| Session conformance | `rutis-bridge/tests/runtime_conformance.rs` + the `rutis-conformance` entry (jvm target) | `session::testing::session` |
| Contract × channel | `session_matrix.rs` gains Kotlin | Dial-back `unix:`, loopback `tcp:`, (K-P2) WebSocket listening; every call, sample value and event of `weather.expected.json` |
| Value round trips | `tests/value_roundtrip.rs` gains a Kotlin end | Property tests of the ten values and the key rules |
| Events conformance | `rutis-loader/tests/events_conformance.rs` (X = kotlin; Y includes kotlin) | Every case of `events.expected.json`; CI's `RUTIS_EVENTS_REQUIRE` adds `kotlin` |
| Testing tool | `kotlin/rutis` (jvm target) | `arcships.rutis.testing` is checked against `weather.expected.json` and `events.expected.json` (cases whose `tool` is not `false`), with the same results as the real runtime |
| Runtime conformance | A Kotlin column in `multilang.rs` | Leaf plugins; cross-language calls; withdrawing a provider stops only its consumers; `inject` gating; undeclared services announced; crossed synchronous calls do not deadlock, and a crossing sync cycle with Node is detected as `SyncWaitCycle` |
| Specifics | `rutis-bridge/tests/kotlin_runtime.rs` | Old-host refusal (word for word, status 1); errors' `code` / `data` both ways and rethrowing; the calling row (two rows with different `isolate`); cancellation reaching the coroutine |
| Dial-out | A Kotlin column in `dial_conformance.rs` | Every item of dial-out design §6 |
| Listening | A Kotlin remote end in `websocket_conformance.rs` | The same assertions as Go's remote listening |
| Device | Android emulator instrumented tests | `RutisApp` on ART dials out to the access point, loads, `stop` / `start`; method tables and proxies work in an R8 release build; errors from denied permissions go back |
| CI | JDK job (jvm target) + emulator job | Gradle build before cargo tests |

| Phase | Work | Acceptance |
| --- | --- | --- |
| K-P1 | SDK core (session, plugin API, KSP processor, values, errors, events, threading), jvm target, the `unix:` / `tcp:` test entry, the testing tool | Passes on the desktop: `runtime_conformance`, `session_matrix` (local channels + `weather.expected.json`), `value_roundtrip`, `events_conformance` (Kotlin as X and Y), the Kotlin column of `multilang`, the testing tool against both expectation files, the old-host refusal |
| K-P2 | Dial-out and listening channels, `RutisApp`, android target; depends on dial-out design R-P1 (done) | Apps on emulators and real devices dial out to the access point and load plugins according to rutis configuration; consumers stop after `stop` and recover after `start`; same-named plugins on two devices each live in their own instance; the Kotlin columns of `dial_conformance`, `websocket_conformance` and `session_matrix`'s listening case pass; an R8 release build works |
| K-P3 | Example capability plugins (contacts, notify, device) and the SDK guide | Real device: permission prompts point at the app; after denial the error goes back |

Follow-ups (when needed): the desktop-JVM runtime (reopening the Kotlin runtime design); receiving object references (`objects`).

## 10. Differences from the first draft

| First draft | This document | Reason |
| --- | --- | --- |
| Only device listening, host dials in | Both directions supported, dialing out recommended | Device addresses are unstable and behind NAT; the direction matches dial-out design R1 |
| Plaintext `ws://` allowed on the LAN | Removed | Violates remote plugins §7 "non-loopback must use TLS"; the host's dialing side also rejects non-loopback plaintext |
| DEX uploaded through an authenticated endpoint, loaded with `DexClassLoader` | Removed; plugins are compiled into the app | Violates remote plugins §7 "no code upload"; consistent with the iOS model |
| Hot reload budget (50 per plugin) and self-restart | Removed | New code is no longer loaded |
| Permission reconciliation (30-second cycle), revocation withdraws services | Removed; permissions are the plugin's own business | When a runtime permission is revoked the system kills the process outright, so the reconciliation path never happens |
| Foreground service, Doze, vendor auto-start guidance | Moved to the business-layer reference | The SDK does not manage the platform lifecycle |
| Helper APK as the only carrier | Any app that integrates the SDK can host plugins | The app itself provides capabilities in the rutis style |
| SDK depends on kotlin-stdlib only | K8 | Android has no built-in WebSocket client or server; the value layer and configuration use kotlinx-serialization (compile time, no runtime reflection) |
| arm64-v8a and x86_64 | No ABI restriction | Pure Kotlin, no native code |

## 11. Alignment with rutis 0.13 (2026-10-19)

0.11–0.13 changed the runtime contract: ten value types (0.11), automatic exposure and errors' `code` / `data` (0.12), the calling row (#441), unified events and plugin API 3 (0.13), the dialing rules written down (#383/#384). This section records the revisions and new decisions that follow; implementations follow [the runtime contract](../.agents/skills/rutis-new-runtime/references/runtime-contract.md).

### 11.1 Revisions

| 2026-10-14 text | Now | Why |
| --- | --- | --- |
| Plugin API 2, leaf events only within the process, feature `events` | Plugin API 3; unified events (§3.4), capability `events-v2`, feature `events` removed, old hosts refused word for word | The 0.13 event protocol |
| Explicit method tables `Methods(async("today") { … })`, `provides` required | KSP generates them from `@RutisService` classes; `provides` optional, automatic exposure | Automatic exposure (0.12) and developer transparency (V0) |
| `s.method1<String, String>("ask")` in a `RutisClient` subclass | An ordinary interface + `@RutisClient` (KSP-generated proxy), `ctx.use<Llm>("llm")` | V0 |
| Schema given explicitly | Derived from the `@Serializable` config type's `SerialDescriptor` | V0; still no reflection |
| JSON-compatible types copied; out-of-range integers fail; values through kotlinx-serialization-json | Own `RutisFormat`; ten values; out-of-range integers become `bigint` automatically | Value types design V1, V6, §6 |
| `Rutis.ref(v)` for explicit references | `@RutisService` instances go by reference | The class already gives the method table |
| Exceptions copy only name and message | `code` / `data` both ways; received errors are `RutisException` | The 0.12 error contract |
| Features `["rows.v2", "hosts", "leaf", "scopes", "events"]` | `["rows.v2", "hosts", "leaf", "scopes", "rows.announce"]` + the capabilities `events-v2`, `value-*` | Contract §2, §3 |
| K-P1 acceptance "session and runtime conformance tests" | Adds value round trips, events conformance, the testing tool against both expectation files, the old-host refusal | The conformance tests are the contract |
| The suspended Kotlin runtime design's note said "remote: dial-out only" | Both directions (K3); the suspended design's note is corrected | Consistent with K3 and dial-out design R1 |

### 11.2 Where rutis-specific code is still needed

Developer transparency (V0): plugin authors write only native Kotlin. Android does not use runtime reflection of methods (R8, `suspend` signatures); these are the rutis-specific pieces that remain, kept to a minimum:

| Where | What | Why it cannot be less |
| --- | --- | --- |
| Plugin definition and entry point | `Rutis.define<C>(…) { ctx, config -> }`; `RutisApp(…)` | Every language has it: the paradigm itself |
| Provided service classes | One `@RutisService` on the class | Method tables can only be generated at compile time; one annotation, and the class stays plain Kotlin |
| Services used | One `@RutisClient` on the interface; `ctx.use<Llm>("llm")` | The same; the interface stays plain Kotlin and can be replaced by any implementation |
| Declaring `sync` methods (optional) | `provides = mapOf("weather" to service<Weather>(sync = setOf("unit")))` | Only when a method should be `sync`, or `check` should know ahead |
| Build configuration | One line `ksp("io.github.arcships:rutis-ksp:…")` | The KSP processor must be registered in the app's build |
| A few event types | `EventArgs`, `Next`, the filter's descriptor `Listener` | The language has nothing corresponding |
| Received errors | `RutisException`, matched by `name` | An error from another process is not the sender's class (the same in every language) |

Not rutis's, but what the Kotlin ecosystem asks for anyway: `@Serializable` on data classes and the configuration; `@Contextual` on `java.time` / `BigInteger` / `BigDecimal` / `java.util.UUID` fields inside `@Serializable` classes (kotlinx's rule; `kotlin.time.Instant` and `kotlin.uuid.Uuid` need none).

Not needed: method tables, JSON encoding, reference wrappers, context parameters, wrappers for errors' `code` / `data` (plain properties do), handling of synchronous waits and reentrancy, event registration, wrapper types for the ten values (all are JDK / Kotlin native types).

### 11.3 Decisions made here

| # | Decision |
| --- | --- |
| 1 | Method tables and proxies are generated at compile time by KSP, found by a class-name convention (as Room / Moshi), with R8 rules shipped in the AAR; no method reflection |
| 2 | `public` member functions are exposed; the wire name is the function name; overloads are a compile error |
| 3 | A method's kind is a declaration (default `async`), independent of `suspend`; in client interfaces `suspend` functions suspend to wait and plain functions block to wait (refused on the main thread) |
| 4 | The call chain lives in a `ThreadLocal` + `asContextElement()`; plugin code passes no context |
| 5 | The value layer is a custom kotlinx-serialization format `RutisFormat` implementing all ten values, mapped to native types (`java.time`, `BigInteger`, `BigDecimal`, `UUID` …), also accepting `kotlin.time.Instant` and `kotlin.uuid.Uuid`; `Any?` parameters decode into native values |
| 6 | The config schema derives from the `SerialDescriptor` and can be overridden |
| 7 | Errors' `code` / `data` are read through the exception's `getCode()` / `getData()`; received errors and the SDK's own are `RutisException`, matched by name |
| 8 | Listeners have Go's shape: `suspend (EventArgs) -> Any?`, `next` last; always `kind: "sync"`; `emit` queues serially per listener; `bail` is a blocking function, the other four are `suspend` |
| 9 | The calling row: each row computes its own service ids from `isolate`; labels are opaque |
| 10 | The testing tool runs plugins over a real session (in-memory channel) with the test as the host, checked against both expectation files |
| 11 | K-P1's desktop test entry is `Rutis.serve` on the jvm (`unix:` / `tcp:`), started by the tests through the generic `Launcher`; the desktop runtime is not reopened and no host configuration is added |
| 12 | `objects` is not declared, as Go |

## 12. K-P1 implementation notes (2026-10-19, #390)

The implementation is in `kotlin/rutis` (`rutis`, `rutis-ksp`, `rutis-conformance`). Where it differs from the above, or the above says nothing:

| Item | Implementation | Why |
| --- | --- | --- |
| Source layout | Both targets share `rutis/src/shared`; the jvm-only local channels and `Rutis.serve` are in `src/jvmMain`. Without an Android SDK only the jvm target is configured | `UnixDomainSocketAddress` is not in Android's API; the jvm target builds on machines without an Android SDK |
| Writing frames | One writer thread sends frames in the order they are queued (call ids are taken when queued) | Plugin threads are interrupted (cancellation), and interrupting a thread writing to a `SocketChannel` closes the channel; the writer is never interrupted |
| A listener cancelled by its row's unload | While the dispatch goes on, it ends with no result (`null`) | A cancelled coroutine cannot return a value normally; Go and Python listeners return after their cancellation, to the same effect |
| `RUTIS_CHANNEL_TOKEN` | Not removed from the environment once read | The JDK has no public API to change its own environment; child processes a plugin starts inherit it |
| Blocking calls on the main thread | Not checked | It matters only on Android; it comes with `RutisApp` in K-P2 |
| `--rutis-manifest` | The jvm entry answers it too (Go's shape, `runtime` `rutis-kotlin-runtime:1`) | Handy for checks; no host reads it in K-P1 |
| The multilang column | `crates/rutis-loader/tests/multilang_kotlin.rs` (as `multilang_go.rs`), the crossing synchronous calls through Kotlin included | Self-contained, leaving the tests other languages share unchanged |
| An emitter giving up | Kotlin's cancellation in the expectations is `CancellationException` | A coroutine's cancellation |

## 13. K-P2 implementation notes (2026-10-20, #391)

The remote channels are in the shared sources, `arcships.rutis.internal.remote` in `rutis/src/shared`: the android and jvm targets run the same code. The jvm `Rutis.serve` takes `dial:` / `listen:` (`--id`, `--peer`, `RUTIS_TOKEN`, `RUTIS_CA`, `RUTIS_CERT` / `RUTIS_KEY`, `RUTIS_RETRY`, `RUTIS_HEARTBEAT`, as the other languages), and the conformance tests run through it. Where it differs from the above, or the above says nothing:

| Item | Implementation | Why |
| --- | --- | --- |
| Listening | Not Java-WebSocket (K8) but the SDK's own: the upgrade request and the frames handled on a `java.net.ServerSocket` (`wss://` layers an `SSLSocket` over each accepted TCP connection; `Listen.kt`, `Frames.kt`), as Go's `listen.go` and Swift's `Remote/WebSocket.swift` do | Java-WebSocket can refuse a handshake only with 404 or 500, while rutis's dialer classifies by status (401 / 403 retried, 400 / 404 stop); it does not check that client frames are masked; its heartbeat counts in seconds and drops a silent peer with 1006. None of the three meets the assertions Go's listener meets (§9). The SDK has one runtime dependency less |
| Dialing | OkHttp 5 (K8); statuses, `Sec-WebSocket-Protocol` and certificate errors classified by type; Android's network security config refusing cleartext (`UnknownServiceException`) is incompatible | Retrying cannot fix it |
| A listener's writes | One writer thread per connection writes the frames in order (pings and pongs ahead of the messages waiting); `send` waits until its frame is written. A write blocked for longer than the heartbeat timeout closes the socket; a close frame gets at most 1 second, then the socket closes whether it went out or not. A forced close closes the TCP socket under TLS, without waiting for TLS's locks | When the far end stopped reading, the write used to block holding a lock: the reader blocked answering a ping, the shared heartbeat timer thread blocked and every connection lost its heartbeat, and `stop()` hung, and the next `start()` with it |
| A listener's request heads | The TLS handshake and the request head get 10 seconds in all (as Go's `ReadHeaderTimeout`), then the connection closes; at most 64 upgrades are under way at once, more connections close at once | Each read used to get its own 10 seconds: a client dripping one byte at a time held a thread for as long as it liked |
| The dialing heartbeat | The SDK's own, as the other runtimes': every ping interval it looks whether anything came from the access point (a message, a ping, a pong; the access point pings every interval) and disconnects when nothing came for `timeoutMillis` (30 s); OkHttp's ping is off, and an unsolicited pong (RFC 6455 §5.5.3) tells the access point this end is there. The ping and pong counts come from OkHttp's `RealWebSocket`; when they cannot be had (an application with another OkHttp version) there is no timeout, TCP decides | OkHttp's ping fails a connection whose pong has not come by the next ping: the timeout was the ping interval (10 to 20 s), not 30 s |
| Close codes | Answering the far end's close frame, 1005, 1006 and codes outside the defined ranges are answered with 1000 | RFC 6455 §7.4.1 forbids 1005 on the wire; OkHttp refuses `close(1005)` |
| `trust` | The trust manager of the system's trust plus extra CAs is an `X509ExtendedTrustManager`, passing the socket- and engine-aware checks on to the system's | Under a network security config with a `<domain-config>`, Android's system trust refuses checks without a host name: a server certificate signed by a system CA was refused once `trust` was set |
| Sending | Waits for room in OkHttp's send queue before handing a message over | OkHttp closes the connection when its queue would exceed 16 MiB |
| The far end greeting as another endpoint | `EndpointMismatchException` (a subclass of `IncompatibleException`); a dialer counts it as refused credentials | As Go's `ErrEndpointMismatch`: classified by type, not by text |
| PEM | Certificates through `CertificateFactory`; private keys in PKCS#8, PKCS#1 (RSA) or SEC1 (EC), the latter two wrapped as PKCS#8 by the SDK for `KeyFactory`; in the process only, never in the Android Keystore | The platform APIs take PKCS#8 only |
| SIGTERM / Ctrl-C | The jvm entry catches them through `sun.misc.Signal`, unloads every row and exits 0 | The JVM's default shutdown hooks exit 143, and an orderly stop cannot run inside a hook |
| rutis-host | `kotlin` / `kt` in `runtimes.remote[].language` is a language that runs only remotely: no local runtime, template or project, rows `<runtime>:<plugin>` | Kotlin runs in applications; the desktop runtime is suspended |
| `RutisApp` | In the shared sources (usable and tested on the jvm too); `start()` and `stop()` both return at once, `state` tells the outcome; a `start` after `stop` connects once the previous round has unloaded; `trust` and `certificate` are PEM text (`certificate` holds the chain and the private key); no environment variables, default backoff and heartbeat | Applications call them on the main thread; an application's configuration is not in its environment |
| State | `Stopped(reason)`, `Connecting(retrying)`, `Connected(address)`: `Connecting` carries the last failure's state line (such as `refused: auth-rejected (…)`); what a round reports after `stop()` is not taken, so it always ends at `Stopped` | The business layer shows why it is not connected yet |
| Main thread | A call that waits for rutis (a blocking call over the session, a remote function, a remote future, a `bail` dispatched through rutis) throws `IllegalStateException` on Android's main thread, saying to use `suspend` or another thread; the check is in `src/androidMain`, nothing is refused on the jvm | §11.3 #3 |
| OkHttp | 5.3.2: from 5.4 its AAR requires applications to compile with compileSdk 36 or later | Not raising applications' compileSdk |
| Device tests | The SDK's own (`src/androidInstrumentedTest`: the main-thread rule; `trust` under a network security config with a `<domain-config>`, over a real TLS handshake); the sample application `kotlin/rutis/android-sample`, driven by `crates/rutis-host/tests/android.rs`: its release build (R8, no rules of the application's own) started and stopped by intents, and the debug build's device test with two connections; on rutis's side a Python row loads through the application's service | The device tests cannot run against the release build to check R8: the dependencies the test APK shares with the application (kotlin, androidx.tracing) are removed by the application's R8 |
