# Android app hosting (Kotlin SDK)

[中文](design-android-runtime-2026-10-12.md)

Status: finalized, not implemented. Date: 2026-10-12; finalized and revised 2026-10-14 (now follows [app integration](design-app-integration-2026-10-14.en.md); see §10). Baseline: `main` `a763cdc` (0.10.0).
Based on: [app integration](design-app-integration-2026-10-14.en.md) (the general model), [remote runtimes: dial-out and dynamic access](design-remote-runtime-dial-2026-10-14.en.md) (connections), [the multilingual design](design-multilang-runtimes-2026-10-03.en.md), [the plugin API](guide/plugin-api.en.md), [the Swift SDK](design-swift-sdk-2026-10-12.en.md) (the Swift counterpart of shape ②).

Positioning: an Android app provides its own capabilities and those of the device it runs on as rutis plugins; the plugins are written in Kotlin, compiled into the app, and driven by rutis. For the desktop-JVM runtime, see [the Kotlin runtime](design-kotlin-runtime-2026-10-12.en.md) (suspended).

Out of scope: loading new code on the device (DEX download, upload, hot swap); app foreground/background, foreground services, Doze handling (business layer; see app integration §7); Intents and other Android-ecosystem calls back into rutis; running rutis itself on the device; the Play compliance process itself.

## 1. Decisions

| # | Decision | Section |
| --- | --- | --- |
| K1 | Plugins are written in the rutis style, compiled into the app (APK / AAB), and loaded, configured and unloaded under rutis's control | 2 |
| K2 | The SDK gives the app only `start` / `stop`; foreground services, Doze and vendor background limits belong to the business layer | 5 |
| K3 | Remote connections support both directions: dialing out to a rutis access point (recommended; each connection is one instance on the rutis side) and listening (rutis dials in); non-loopback must be `wss://`, no plaintext exception | 5 |
| K4 | The SDK is a Kotlin library with two targets: android and jvm; the jvm target is for conformance tests on the desktop and is also the basis for a future desktop runtime | 7 |
| K5 | One platform thread per incoming call; `suspend` methods are bridged with `runBlocking` on that call's thread; declares `sync-wait` and `reentrant-sync`, not `sync-stack` | 4 |
| K6 | No runtime reflection: service methods use explicit method tables and config schemas are given explicitly (consistent with Swift); a KSP generator is a follow-up | 3 |
| K7 | Values: JSON-compatible types are copied recursively; functions and explicit `Rutis.ref` go by reference; integers beyond ±2^53−1 are an error | 3 |
| K8 | Dependencies: kotlin-stdlib, kotlinx-coroutines-core, kotlinx-serialization-json, OkHttp (WebSocket client), Java-WebSocket (WebSocket server, pure Java) | 7 |
| K9 | System permissions belong to the app; not granted = `apply` or the method call fails; when a runtime permission is revoked while running, the system kills the app process, which rutis sees as a disconnect | 6 |
| K10 | Platform: Android 10 (API 29) and later; pure Kotlin, no native code | 7 |
| K11 | The only host-side change: `runtimes.remote[].language` accepts `"kotlin"` / `"kt"`; everything else follows the dial-out design | 8 |

## 2. Model

```text
rutis (desktop / server)                  Android app
 ├─ access point (listens, wss)  ◀──dial out──  ├─ business code: decides when to start / stop
 ├─ one instance per connection                 ├─ rutis Kotlin SDK
 │    └─ rows phone:contacts, phone:notify      │    └─ plugins compiled into the app: contacts / notify / device …
 └─ other rows inject these services by name    └─ Android API (permissions belong to the app)
```

- Plugins in the same app calling each other get the object itself; across connections and across languages, calls go by name through rutis.
- rutis does not model the device: model, OS version and the like are provided by plugins (e.g. a `device` plugin).
- App process death, network loss, `stop()`: to rutis these are all a disconnect — the instance closes, its services are withdrawn, consumers stop; after reconnecting, plugins are loaded again according to rutis's current configuration.

## 3. Plugin API

```kotlin
import arcships.rutis.*

@Serializable
data class Config(val city: String)

class Llm(s: RutisService) : RutisClient {
    val ask = s.method1<String, String>("ask")        // overloads by argument count, 0–5 arguments
}

val weather = Rutis.define(
    name = "weather",
    inject = listOf("llm"),
    provides = mapOf("weather" to Shape(async("today"), sync("unit"))),
    config = Rutis.config<Config>(
        Schema.obj(mapOf("city" to Schema.string("city to look up the weather for")), required = listOf("city"))),
) { ctx, config ->
    val llm = ctx.use("llm", ::Llm)
    ctx.provide("weather", Methods(
        async("today") { q: String -> llm.ask("weather in ${config.city}: $q") },
        sync("unit") { -> "celsius" },
    ))
    ctx.effect { /* runs in reverse order on unload */ }
}
```

| API | Rule |
| --- | --- |
| `Rutis.define` | Validates the declaration (names; method tables match the shapes); carries plugin API version 2 |
| `apply` | May be suspend; throwing fails the load, and cleanups already registered still run |
| `ctx.use(name, factory)` | `name` must be in `inject`; a provider in the same process hands over its local implementation directly |
| `ctx.provide(name, methods)` | The method table must match the shape declared in `provides`; lasts until unload or until the returned withdraw function is called |
| `ctx.effect(cleanup)` | May be suspend; runs in reverse order on unload |
| `ctx.on(name, listener)` / `ctx.emit(name, args…)` | Leaf events (plugin API 2): an event bus within this runtime process, with the same semantics as other runtimes; does not cross processes; `internal/*` is not available |
| `ctx.launch { }` | Runs in the row's coroutine scope; cancelled and awaited on unload |
| Method shape | `async` by default; `sync` is marked explicitly |
| Config | Decoded with kotlinx-serialization into a `@Serializable` type; the schema is given explicitly; failure fails the load |
| Errors | Exceptions are returned as error maps; the name is the exception class's simple name |

| Kotlin value | Across processes |
| --- | --- |
| `null`, `Boolean`, `String`, `Int` / `Long` / `Double` | Copied; integers beyond ±2^53−1 are an error |
| `List`, `Map<String, *>`, `@Serializable` types | Copied recursively |
| Functions (including suspend functions) | By reference |
| `Rutis.ref(v)` | By reference; `v` carries its own method table |
| Exceptions | Name and message copied |
| Other objects | Cannot cross processes: an error suggesting a data type or `Rutis.ref` instead |

## 4. Threading and session

| Item | Rule |
| --- | --- |
| Session structure | One reader thread parses frames; frame writes are serialized by one lock; each incoming call gets a new platform thread (ART has no virtual threads) |
| Outgoing calls | The current thread blocks on a `CompletableFuture`, which the reader thread completes; calls to the peer's `sync` methods carry `sync: true` in the frame |
| Reentrancy | An incoming call always has a new thread to run on; declares `reentrant-sync`; a plugin's service may be called again in the middle of its own synchronous call, so do not hold locks while calling rutis services |
| Capabilities | `sync-wait`, `reentrant-sync`; `sync-stack` is not declared (concurrent multi-threading; it only contributes call-chain edges) |
| suspend bridging | `runBlocking` on that call's thread; never bridged on the reader thread |
| Cancellation | A `cancel` frame → cancel that call's coroutine scope and interrupt its thread |
| Uncaught exceptions on plugin threads | The app crashes: rutis sees a disconnect |
| Contract | endpoint protocol 3, the handshake declaring capability `runtime`; features `["rows.v2", "hosts", "leaf", "scopes", "events"]`; the `mount` reply carries `plugins` (dial-out design §4) plus `implementation` and `engine` (kotlin + ART version); call id prefix `node:` |

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
| `start()` | Connects, greets, reports `plugins`; after a disconnect, reconnects automatically per dial-out design §6 until `stop()` |
| `stop()` | Unloads all rows (cleanups in reverse order) and closes the connection; `start()` may be called again |
| State | `rutis.state` (`Stopped` / `Connecting` / `Connected`, a `StateFlow`) for the business layer to display; the SDK makes no lifecycle decisions based on it |
| Certificates | Non-loopback must be `wss://`; for a self-signed certificate, trust its CA through an SDK parameter |
| Listening | `RutisApp(listen = "wss://0.0.0.0:7443/rutis", …)` instead waits for rutis to dial in, with the same semantics as Go's remote listening (one controller at a time, a new connection takes over); suits devices with a fixed address; dialing out is recommended in general |
| Changing code | Ship a new version of the app |
| Business-layer reference | When it needs to stay resident, the app runs a foreground service (from Android 14 the foreground service type must be declared) and guides the user to the battery-optimization allowlist; otherwise it follows the UI with `start` / `stop`. This goes in the SDK guide, not in the SDK |

## 6. Permissions

| Case | Behavior |
| --- | --- |
| Not granted at load time | The plugin checks in `apply` and throws, with the error naming the permission; the row fails to load |
| Not granted at call time | The method throws; the error goes back to the caller on the rutis side |
| Runtime permission revoked while running | The system kills the app process → disconnect → the instance closes; after the app restarts and reconnects, plugins are loaded again according to the configuration, and the `apply` of plugins without the permission fails |
| A special permission (notification listener, accessibility, etc.) turned off | The plugin notices on its own and withdraws the services it provides (the withdraw function returned by `ctx.provide`), or fails on method calls |

Permission prompts point at the app (corresponding to Swift shape ②). The SDK does no permission reconciliation; permissions are the plugin's own business.

## 7. SDK and packaging

| Item | Rule |
| --- | --- |
| Coordinates | `io.github.arcships:rutis` (Kotlin Multiplatform: android + jvm); published to Maven Central; version goes into `scripts/train.mjs` |
| Repository location | `kotlin/rutis` (a Gradle project) |
| Packages | `arcships.rutis` (API); session layer `arcships.rutis.internal.peer` |
| Dependencies | K8; plugin authors depend on this library only |
| Platform | minSdk 29; pure Kotlin, no ABI restriction |
| Test helpers | `arcships.rutis.testing`: host-less loading, strict mode (simulates cross-process value semantics), like Go's `rutistest` |

## 8. The host side

| Change | Location |
| --- | --- |
| `runtimes.remote[].language` accepts `"kotlin"` / `"kt"` → `RuntimeResolver::modules` | `rutis-host/src/host.rs` |
| Everything else (access point, connection-as-instance, identity layer) | Dial-out design §7 |

For a configuration example, see dial-out design §4 (with `language` set to `kotlin`).

## 9. Tests and phases

| Test | Location | Content |
| --- | --- | --- |
| Session unit | `kotlin/rutis` (jvm target) | Encoding, reference counting, callbacks during synchronous calls, cancellation, call chains, concurrency |
| Session conformance | `rutis-bridge/tests/runtime_conformance.rs` + a Kotlin conformance entry point (jvm target) | `session::testing::session` |
| Runtime conformance | A Kotlin column in `multilang.rs` | Leaf plugins; cross-language calls; withdrawing a provider stops only its consumers; `inject` gating; crossed synchronous calls do not deadlock |
| Dial-out | A Kotlin column in `dial_conformance.rs` | Every item of dial-out design §6 |
| Listening | A Kotlin remote end in `websocket_conformance.rs` | The same assertions as Go's remote listening |
| Device | Android emulator instrumented tests | `RutisApp` on ART dials out to the access point, loads, `stop` / `start`; errors from denied permissions go back |
| CI | JDK job (jvm target) + emulator job | Gradle build before cargo tests |

| Phase | Work | Acceptance |
| --- | --- | --- |
| K-P1 | SDK core (session, plugin API, values, threading), jvm target | Passes session and runtime conformance on the desktop |
| K-P2 | Dial-out and listening channels, `RutisApp`, android target; depends on dial-out design R-P1 | Apps on emulators and real devices dial out to the access point and load plugins according to rutis configuration; consumers stop after `stop` and recover after `start`; same-named plugins on two devices each live in their own instance; the `dial_conformance` and `websocket_conformance` Kotlin columns pass |
| K-P3 | Example capability plugins (contacts, notify, device) and the SDK guide | Real device: permission prompts point at the app; after denial the error goes back |

Follow-ups (when needed): KSP generation of method tables and schemas; the desktop-JVM runtime (reopening the Kotlin runtime design).

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
| SDK depends on kotlin-stdlib only | K8 | Android has no built-in WebSocket client or server; config decoding uses kotlinx-serialization (compile time, no runtime reflection) |
| arm64-v8a and x86_64 | No ABI restriction | Pure Kotlin, no native code |
