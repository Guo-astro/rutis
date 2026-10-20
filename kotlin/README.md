# rutis for Kotlin

[中文](README.zh-CN.md)

Write [rutis](https://github.com/arcships/rutis) plugins in Kotlin. The SDK (`kotlin/rutis`, Maven coordinates `io.github.arcships:rutis` and `io.github.arcships:rutis-ksp`, released with the rutis train) is a Kotlin Multiplatform library with two targets: **android**, for Android applications that host plugins (phase K-P2), and **jvm**, which runs the same code on the desktop: the conformance tests and the test entry `Rutis.serve`. Design: [docs/design-android-runtime-2026-10-12.md](../docs/design-android-runtime-2026-10-12.md).

Status: phase K-P1, the SDK core. Plugins, services, values, errors, unified events and the testing tool work on the jvm target and pass the same conformance suites as the other runtimes. Connecting an application to rutis (`RutisApp`, dialing out and listening over WebSocket) is K-P2; nothing is published to Maven Central yet.

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

The build applies the serialization plugin and the processor, which generates the method tables of `@RutisService` classes and the proxies of `@RutisClient` interfaces at compile time (no method is found by reflection at run time; the AAR ships the R8 rules that keep them):

```kotlin
plugins {
    kotlin("plugin.serialization")
    id("com.google.devtools.ksp")
}
dependencies {
    implementation("io.github.arcships:rutis:<version>")
    ksp("io.github.arcships:rutis-ksp:<version>")
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

Values: `RutisFormat`, the SDK's own kotlinx-serialization format, carries them, never JSON's default encoding. `java.time.Instant` and `kotlin.time.Instant` cross as instants, `LocalDateTime` / `LocalDate` / `LocalTime` as datetime / date / time, `ByteArray` as bytes, a `Map` whose keys are not strings as a map, a `Set` as a set, `BigInteger` as a bigint (any integer beyond ±(2^53−1) too), `BigDecimal` as a decimal (its scale kept), `UUID` and `kotlin.uuid.Uuid` as a uuid. In a `@Serializable` class, fields of these JDK types are `@Contextual` (kotlinx's rule). A parameter of type `Any?` receives native values: `Long`, `Double`, `Map<String, Any?>`, `List<Any?>`, …

Threading: every incoming call runs on a thread of its own; a `suspend` method runs there in `runBlocking`. The call chain travels in a `ThreadLocal` carried into coroutines (`withContext` included), so plugin code passes no context. A cancelled call cancels the method's coroutine and interrupts its thread.

## Testing a plugin

`arcships.rutis.testing` runs the plugin in its real runtime over a real (in-memory) session, the test being the host:

```kotlin
RutisTest.load(weather, Config("Oslo"), services = mapOf("llm" to FakeLlm())).use { loaded ->
    assertEquals("celsius", loaded.service("weather").call("unit"))
    loaded.parallel("session/created", "s1")
}
```

Services the test gives are instances of `@RutisService` classes. `loaded.on`, `emit`, `parallel`, … listen and emit as the host; `loaded.load(config, row, scope)` loads more rows; `loaded.reported()` is what the runtime reported.

## The desktop test entry

On the jvm target, `Rutis.serve(args, plugins…)` runs plugins as a rutis runtime: `<channel> <project>`, the channel a Unix socket path (`unix:<path>` or bare) or `tcp:127.0.0.1:<port>` (presenting `RUTIS_CHANNEL_TOKEN`). The Rust conformance tests start it with `java -jar` through rutis-bridge's generic `Launcher` (no Kotlin runtime is configured in rutis-host in K-P1).

## Building and testing the SDK

JDK 17. No Gradle install is needed: the wrapper is in the repository.

```sh
cd kotlin/rutis
./gradlew build          # the SDK, the processor, the unit tests, the testing tool against both expectation files
```

The android target is built only where an Android SDK is installed (`ANDROID_HOME`, or `sdk.dir` in `local.properties`). The Rust suites with Kotlin take the feature `kotlin` and build `rutis-conformance/build/libs/rutis-conformance-all.jar` (or use the one `RUTIS_KOTLIN_JAR` names):

```sh
cargo test -p rutis-bridge --features testing,websocket,kotlin --test runtime_conformance --test value_roundtrip --test session_matrix kotlin_
cargo test -p rutis-bridge --features testing,kotlin --test kotlin_runtime
cargo test -p rutis-loader --features node,python,kotlin --test events_conformance --test multilang_kotlin
```
