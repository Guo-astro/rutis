# rutis for Kotlin

[中文](README.zh-CN.md)

Write [rutis](https://github.com/arcships/rutis) plugins in Kotlin. The SDK (`kotlin/rutis`, Maven coordinates `io.github.arcships:rutis`, `io.github.arcships:rutis-ksp` and the testing tool `io.github.arcships:rutis-testing`, released with the rutis train) is a Kotlin Multiplatform library with two targets: **android**, for Android applications that host plugins (phase K-P2), and **jvm**, which runs the same code on the desktop: the conformance tests and the test entry `Rutis.serve`. Design: [docs/design-android-runtime-2026-10-12.md](../docs/design-android-runtime-2026-10-12.md).

Status: phase K-P2. Plugins, services, values, errors and unified events work on both targets and pass the same conformance suites as the other runtimes; an application connects to rutis with `RutisApp` (dialing out and listening over WebSocket), tested on an Android emulator (the R8 release build included). Published to Maven Central from 0.15.

```kotlin
import arcships.rutis.*
import kotlinx.serialization.Serializable

@Serializable
data class Config(val city: String)            // the config's JSON Schema is derived from it

@RutisClient                                    // a service the plugin uses: a plain interface
interface Llm {
    suspend fun ask(question: String): String
}

@RutisService                                   // a service it provides: a plain class
class Weather(private val llm: Llm, private val city: String) {
    suspend fun today(question: String): String = llm.ask("weather in $city: $question")
    fun unit(): String = "celsius"
}

val weather = Rutis.define<Config>(
    name = "weather",
    inject = listOf("llm"),
    provides = mapOf("weather" to service<Weather>(sync = setOf("unit"))),   // optional
) { ctx, config ->
    ctx.provide("weather", Weather(ctx.use<Llm>("llm"), config.city))
    ctx.on("session/created") { args -> println("new session ${args[0]}"); null }
    ctx.effect { /* runs when the plugin unloads, latest first */ }
}
```

The SDK is not published to Maven; it is used from source: fetch this repository's tag (`git clone --depth 1 --branch v<version> https://github.com/arcships/rutis.git ../rutis`) and add `includeBuild("../rutis/kotlin/rutis")` to `settings.gradle.kts` (an Android application also lets it find the Android SDK, see [kotlin-app](../docs/guide/kotlin-app.en.md#1-dependencies)). The build applies the serialization plugin and the processor, which generates the method tables of `@RutisService` classes and the proxies of `@RutisClient` interfaces at compile time (no method is found by reflection at run time; the AAR ships the R8 rules that keep them):

```kotlin
plugins {
    kotlin("plugin.serialization")
    id("com.google.devtools.ksp")
}
dependencies {
    // No version: built from the source includeBuild brings in.
    implementation("io.github.arcships:rutis")
    ksp("io.github.arcships:rutis-ksp")
    testImplementation("io.github.arcships:rutis-testing")   // the testing tool
    kspTest("io.github.arcships:rutis-ksp")
}
```

## The API

| | |
| --- | --- |
| `Rutis.define<C>(name, inject, provides?, schema?) { ctx, config -> }` | `C` is `@Serializable` (`Rutis.NoConfig` for none). `provides` is optional: a service `ctx.provide` provides without it reaches rutis by itself, every method `async`; `service<T>(sync = setOf(...))` marks methods `sync`. Plugin API 3 |
| `ctx.use<I>(name)` | `I` is a `@RutisClient` interface, `name` is in `inject`. A provider in this process whose object implements `I` gives the object itself; any other gives a proxy. A `suspend` function suspends, a plain one blocks its thread |
| `ctx.provide(name, obj)` | `obj` is an instance of a `@RutisService` class: its public member functions (its superclasses' included) are its methods, by name; until the plugin unloads or the returned function is called |
| `ctx.effect { }` | a cleanup, run latest first when the plugin unloads |
| `ctx.on(name, prepend, global) { args -> }` | a listener: `args` are the event's arguments as native values, with `args.next` last in a waterfall; its result counts in `serial`, `bail` and `waterfall` |
| `ctx.emit` / `parallel` / `serial` / `waterfall` / `bail`, `ctx.filter { d -> }` | the five ways of emitting; `emit` returns at once, `bail` blocks, the other three suspend; failures of `parallel` are a `RutisException` named `AggregateError` |
| `ctx` itself | the plugin's coroutine scope: `ctx.launch { }` keeps the call chain and is cancelled and waited for when the plugin unloads |

Errors: a plugin throws ordinary exceptions; one with public `code: String` and `data` properties crosses with them. A received error, and the SDK's own (`AggregateError`, `SyncWaitCycle`, `ReservedEvent`, …), is a `RutisException`: match it by `name`.

Values: `RutisFormat` (`arcships.rutis.advanced`), the SDK's own kotlinx-serialization format, carries them, never JSON's default encoding. `java.time.Instant` and `kotlin.time.Instant` cross as instants, `LocalDateTime` / `LocalDate` / `LocalTime` as datetime / date / time, `ByteArray` as bytes, a `Map` whose keys are not strings as a map, a `Set` as a set, `BigInteger` as a bigint (any integer beyond ±(2^53−1) too), `BigDecimal` as a decimal (its scale kept), `UUID` and `kotlin.uuid.Uuid` as a uuid. In a `@Serializable` class, fields of these JDK types are `@Contextual` (kotlinx's rule). A parameter of type `Any?` receives native values: `Long`, `Double`, `Map<String, Any?>`, `List<Any?>`, … The result of `Rutis.define`, a `Plugin`, can be a parameter or result too (type it `Plugin` in a client or service class): it crosses as a plugin definition, which the receiver mounts ([the plugin API](../docs/guide/plugin-api.en.md#plugin-definitions)).

Threading: every incoming call runs on a thread of its own; a `suspend` method runs there in `runBlocking`. The call chain travels in a `ThreadLocal` carried into coroutines (`withContext` included), so plugin code passes no context. A cancelled call cancels the method's coroutine and interrupts its thread.

## Testing a plugin

`arcships.rutis.testing` (artifact `io.github.arcships:rutis-testing`, jvm) runs the plugin in its real runtime, in the test process, as a row of the real rutis kernel: `rutis-host test`, on `PATH` or at `RUTIS_HOST`, started once per test process; rows are unloaded between tests. rutis checks the config and fills its defaults, resolves what the plugin injects and dispatches its events, as a host running it does:

```kotlin
RutisTest.load(weather, Config("Oslo"), services = mapOf("llm" to FakeLlm())).use { loaded ->
    assertEquals("celsius", loaded.service("weather").call("unit"))
    loaded.parallel("session/created", "s1")
}
```

Services the test gives are instances of `@RutisService` classes. The test's services, `loaded.on`, `emit`, `parallel`, … are a plugin of the same runtime, at the root scope; `loaded.load(config, row, isolate)` loads more rows (`row` is the row's id, rutis gives one from the plugin's name otherwise); `loaded.reported()` is what rutis reported of the test's rows (listeners and `emit` filters that failed). On a device there is no `rutis-host`: the test installs its own plugins with `RutisApp`, dialing a rutis on the development machine ([kotlin-app](../docs/guide/kotlin-app.en.md) §7).

## Hosting plugins in an application

An application brings its plugins to rutis with `RutisApp` (design §5): once connected it installs them itself, in order, each with the configuration it gives (`configs`, by plugin name; without one, the Schema's defaults) once the services it injects are there, and rutis registers them as `<id>:<plugin>`.

```kotlin
val rutis = RutisApp(
    plugins = listOf(contacts(context), notify(context), device(context)),
    configs = mapOf("notify" to NotifyConfig(channel = "rutis sample")),
    connect = "wss://rutis.example.com:7443/rutis",   // or listen = "wss://0.0.0.0:7443/rutis", certificate = pem
    id = "kim-pixel",                                 // this connection's id: rutis registers kim-pixel:contacts, …
    token = secrets.rutisToken,
)
rutis.start()   // returns at once; installs the plugins once connected; reconnects by itself until stop()
rutis.stop()    // withdraws the services, every plugin's cleanups in reverse, then disconnects; start() again later
rutis.state     // StateFlow<RutisState>: Stopped(reason) / Connecting(retrying) / Connected(address)
```

When to start and stop (with the screen, in a foreground service) is the application's business; the SDK decides nothing by `state`. `trust` takes the PEM of a CA trusted besides the system's, for a self-signed access point. A call that waits for rutis (a non-`suspend` method of a `@RutisClient` interface) is refused on the main thread: call it as a `suspend` function or from another thread. On rutis's side, one `rutis-host/listen` row lets the applications in ([rutis-host](../docs/guide/rutis-host.en.md)). `rutis/android-sample` is an application doing this, and the device tests run with it. The whole guide (permissions, foreground services, battery optimization, a real-device checklist): [Hosting plugins in an Android application](../docs/guide/kotlin-app.en.md).

## The desktop test entry

On the jvm target, `Rutis.serve(args, plugins…)` runs plugins as a rutis runtime: `<channel> --id <id> <project>`, greeting as `<id>`; the channel a Unix socket path (`unix:<path>` or bare) or `tcp:127.0.0.1:<port>` (presenting `RUTIS_CHANNEL_TOKEN`). The Rust conformance tests start it with `java -jar` through rutis-bridge's generic `Launcher` (no local Kotlin runtime is configured in rutis-host).

The remote channels are those of the other languages ([nodes](../docs/guide/nodes.en.md)): `dial:wss://… --id <id> --peer <rutis's id>` dials a rutis `rutis-host/listen` row and stays connected, `listen:wss://… --id <id>` waits for rutis to dial in; `RUTIS_TOKEN`, `RUTIS_CA` (a CA trusted besides the system's when dialing), `RUTIS_CERT` / `RUTIS_KEY` (the certificate and key of a `wss://` listener, PEM: PKCS#8, PKCS#1 or SEC1); the reconnection rules and state lines of [dialing out](../docs/design-remote-runtime-dial-2026-10-14.en.md) §6; SIGTERM / Ctrl-C unloads every row and exits 0. On the rutis side, a `rutis-host/dial` row reaches a runtime that listens, a `rutis-host/listen` row takes one that dials in. Dialing uses OkHttp, listening the SDK's own WebSocket server; both are in the shared sources, the same code an Android application runs.

## Building and testing the SDK

JDK 17. No Gradle install is needed: the wrapper is in the repository.

```sh
cd kotlin/rutis
./gradlew build          # the SDK, the processor, the unit tests, the testing tool against both expectation files
```

The testing tool's tests run on `rutis-host test`: `RUTIS_HOST`, else the repository's `target/debug/rutis-host` (`cargo build -p rutis-host`).

The android target is built only where an Android SDK is installed (`ANDROID_HOME`, or `sdk.dir` in `local.properties`). The Rust suites with Kotlin take the feature `kotlin` and build `rutis-conformance/build/libs/rutis-conformance-all.jar` (or use the one `RUTIS_KOTLIN_JAR` names):

```sh
cargo test -p rutis-bridge --features testing,websocket,kotlin --test runtime_conformance --test value_roundtrip --test session_matrix kotlin_
cargo test -p rutis-bridge --features testing,websocket,kotlin --test dial_conformance kotlin::
cargo test -p rutis-bridge --features testing,websocket,kotlin --test kotlin_remote
cargo test -p rutis-bridge --features testing,kotlin --test kotlin_runtime
cargo test -p rutis-loader --features node,python,kotlin --test events_conformance --test multilang_kotlin
```

On an Android device or emulator (`adb`; `tools/android-emulator.sh start` boots one): the SDK's device tests, and the sample application dialing a rutis-host (Python 3.10+ for its other row):

```sh
./gradlew :rutis:connectedDebugAndroidTest
cargo test -p rutis-host --features android --test android -- --test-threads 1
```
