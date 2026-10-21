# The Kotlin Plugin Runtime (Design)

[中文](design-kotlin-runtime-2026-10-12.md)

> **Suspended (2026-10-14; updated 2026-10-19)**: to be reopened when there is a concrete need (plugins from the Maven / JVM ecosystem). Kotlin's current main line is [Android app hosting](design-android-runtime-2026-10-12.en.md): its K-P1 (milestone 0.15) does not depend on this document, and its jvm target only serves conformance tests on a desktop; it is the future basis for this runtime. On reopening, the Android design (aligned with rutis 0.13, see its §11) takes precedence, and the following parts of this document are void: method reflection (D20, the `methodsOf` / `schemaOf` of §7.1) is replaced by KSP-generated method tables and proxies, with schemas derived from the `SerialDescriptor`; the value layer (D8, §7) by `RutisFormat` and the ten values; plugin API 1 (D14) by 3 with unified events; a required `provides` by automatic exposure; remote connections (D18) work in both directions (listening and dialing out, see [dial-out and dynamic access](design-remote-runtime-dial-2026-10-14.en.md); the "dial out only" written here before was wrong); dependencies follow K8 of the Android design (replacing D13); the crash behavior (D15) follows the runtime's `restart` (default `no`, #274). The statements below that "this is the first new language runtime since the overall design" and that "Swift has not been started" reflect the time of drafting and are out of date (Bun and Go have landed; Swift is finalized).

Status: suspended (see above). Date: 2026-10-12. Baseline: `main` `ebf0711` (0.9.1).
Based on: [the multilingual design](design-multilang-runtimes-2026-10-03.en.md) (with its [M1](design-multilang-m1-2026-10-04.en.md) and [M2](design-multilang-m2-2026-10-04.en.md) implementation notes), [the Bun runtime](design-bun-runtime-2026-10-09.en.md), [the Go plugin runtime](design-go-runtime-2026-10-10.en.md), [developer packages](design-developer-packages-2026-10-06.en.md), and [the plugin API](guide/plugin-api.en.md). Swift (M3 of the overall design) has not been started; this is the first new language runtime since that design, and it answers the reentrancy question the Go design §9 proposed, for Kotlin (§6). The answer should be recorded back into the overall design §9.

Out of scope: Cordis in Kotlin; non-JVM Kotlin Multiplatform targets; embedding a JVM in the host process (Appendix A explains why not); the host compiling Kotlin code at run time.

## 1. Decisions

| # | Decision | Section |
| --- | --- | --- |
| D1 | One Kotlin runtime per JVM process, on the existing wire protocol (compat protocol 2), isomorphic to Python/Go/Bun | 2, A |
| D2 | Runtime name `kt`, rows `kt:<plugin>`, through `RuntimeResolver::modules` (the `py:` path); multiple instances via `.named()`, qualified rows `<runtime name>:<plugin>` | 3 |
| D3 | JDK baseline 21 (virtual threads), 25 LTS recommended; discovery: configuration → `JAVA_HOME` → `PATH`, with a `java -version` precheck before launch (like the Python interpreter) | 4 |
| D4 | Channel defaults to socket-path dial-back (`Connect::DialBack`), loopback TCP on Windows; no `fd:3` — the JDK has no public API to wrap an inherited fd | 4 |
| D5 | Threading: one blocking reader thread + one virtual thread per incoming call; capabilities `sync-wait` + `reentrant-sync`, no `sync-stack` (the Go class: concurrent stacks, chain edges only) | 6 |
| D6 | The plugin API uses suspend for asynchrony; synchronous bridging happens only in `runBlocking` on a virtual thread, never on the reader thread | 6 |
| D7 | Methods are `async` by default; `sync` is explicit (Go D12's reason: synchronous methods block event-loop peers) | 7 |
| D8 | Values: JSON-compatible types recursively copied; functions and explicit `rutis.Ref` by reference; objects given to `ctx.provide` always by reference | 7 |
| D9 | One child-first `URLClassLoader` per plugin; unload = `close()` + drop references, reclaimed by GC; leak-free unloading is a first-class SDK obligation (Metaspace is the largest engineering risk) | 8 |
| D10 | Hot reload = swap the classloader (jar mtime+size stamp, like the Python module reload); DCEVM/JBR is not part of the default distribution | 8 |
| D11 | No Kotlin scripting / `.kts` as the plugin vehicle; a plugin is a compiled jar. `dev` has the host invoke the project's own `gradle jar` (as Go D15: the host does not compile; dev rebuilds only the project) | 8 |
| D12 | Channel and frames: newline-delimited compact JSON, 16 MiB per frame; reference counts, `cancel`, call-chain `path`, `sync: true` all per the existing protocol | 5 |
| D13 | Single SDK artifact `io.github.arcships:rutis` (SDK + runtime in one, the Python model), depending only on kotlin-stdlib; kotlinx-coroutines-core and kotlin-reflect ship with the runtime, not the plugin's compile classpath | 9 |
| D14 | Plugin API version `PLUGIN_API = 1`, the existing mechanism as-is | 7 |
| D15 | Crash semantics: an exception in a service method is returned as an error; an uncaught exception in a plugin-spawned thread ends the process, all its rows stop, no automatic restart | 6 |
| D16 | Cancellation: a `cancel` frame cancels the call's coroutine scope and interrupts its virtual thread | 6 |
| D17 | This runtime never produces `SyncWaitCycle` (incoming calls always have a fresh virtual thread); received ones surface as remote errors | 6 |
| D18 | A remote runtime (`listen:ws`) uses the endpoint format (protocol 3), in a later phase | 12 |
| D19 | Process start matches Python: start at mount; on-demand/idle (Go-style) is an open question (a resident JVM costs an order of magnitude more memory than Python) | 13 |
| D20 | Config schema: derived by reflection from a data class primary constructor (kotlin-reflect inside the runtime), or given explicitly by `Plugin.schema` | 7 |

## 2. Background: JVM / Kotlin findings

From 2025–2026 official documentation and first-hand sources (re-verify against the §14 checklist before implementing):

| Finding | Source | Consequence |
| --- | --- | --- |
| Virtual threads GA in JDK 21 (JEP 444); JDK 24 (JEP 491) stops `synchronized` blocking from pinning carrier threads | openjdk.org/jeps/444, /491 | "One execution unit per incoming call" is nearly free; baseline 21, 24/25 recommended; on 21–23 a plugin holding `synchronized` across a synchronous rutis call pins a carrier (throughput, not deadlock) |
| Unix domain sockets since JDK 16 (JEP 380): `SocketChannel.open(UnixDomainSocketAddress)` | openjdk.org/jeps/380 | Dial-back channel with zero native dependencies; the whole session layer is pure Java, no JNI/FFM |
| The JDK has no public API to wrap an inherited fd (fd:3); `System.inheritedChannel` covers stdio only | JDK API docs | The channel dials back by socket path (D4); the local transport already supports it (`Connect::DialBack`) |
| `runBlocking` blocks the current thread and creates an event loop; nesting on shared dispatcher threads is explicitly discouraged | kotlinx.coroutines docs, issue #3983 | Synchronous bridging only on a dedicated virtual thread (D6) |
| `Dispatchers.IO` defaults to a parallelism cap of 64 (or cores) | kotlinx.coroutines docs | Not a "thread per call" pool; incoming calls start virtual threads directly |
| A classloader is reclaimed only when every class it loaded is unreachable; static references, leftover threads, JNI global references pin it, ending in `OutOfMemoryError: Metaspace` | morling.dev, "Class Unloading in Layered Java Applications" | Repeated load/unload is rutis's normal mode; leak-free unloading is a first-class obligation (D9) plus a dedicated soak test |
| JetBrains does not recommend Kotlin scripting as a general scripting vehicle (2024-11 "State of Kotlin Scripting"); the JSR-223 Kotlin engine takes seconds per eval | blog.jetbrains.com/kotlin | No `.kts` plugins (D11) |
| K2 compiler ~2× faster; Gradle incremental compilation on by default | JetBrains benchmarks, Kotlin Gradle docs | The dev loop "edit to reloadable" is seconds, with a resident Gradle/Kotlin daemon |
| GraalVM native image is a closed-world assumption, fundamentally at odds with loading code at run time | GraalVM compatibility guide | Excluded (startup optimization via AppCDS, optional in K5) |
| Kotlin/Native can only export a C ABI via `-produce dynamic`; no library for Rust to link statically; cinterop remains `@ExperimentalForeignApi` | kotlinlang.org native docs | Direct linking excluded (Appendix A); the JVM ecosystem is Kotlin's main value |
| FFM (JEP 454) only covers Java→native; from JDK 24 JNI/FFM warn by default and will refuse by default later (JEP 472) | openjdk.org/jeps/454, /472 | This design uses neither; if the embedded route ever opens, its costs are listed in Appendix A |
| Cross-process synchronous call round-trip: ~30 µs measured on Node, 25–60 µs on Python | overall design §8 | Expect the same order or better on the JVM; measure before shipping (§14) |

## 3. Runtime model

```text
rutis host
 ├─ LocalRuntime::node / ::python / ::bun / ::go ...
 ├─ LocalRuntime::kotlin(project) (named "kt")
 │    └─ JVM process (leaf SDK + runtime contract)
 │         ├─ kt:weather (one jar, one classloader)
 │         ├─ kt:tools.ping
 │         └─ kt:analytics (another jar)
 └─ LoaderPlugin + one RuntimeRowsPlugin per runtime
```

- A row depends on `RuntimeRows#kt` plus every name its plugin injects (leaf runtime: rutis gates every name, as Python).
- Services in `provides` are published from the row's fiber at `host_key(name)` and recorded in the loader's service-name catalog.
- `isolate`, instance labels, service ids (`name` + NUL + label), export handle generations: all as Python.
- Process exit: every row of this runtime stops, then the rows using their services; no automatic restart.
- Plugins in one process call each other on the object itself, not over the wire; across processes and languages, by name through rutis.

## 4. The process

### 4.1 Launch

```text
java -cp <rutis-kotlin runtime jar> arcships.rutis.runtime.Main <channel> [--id <endpoint>] [--peer <endpoint>] <project>
```

| Item | Rule |
| --- | --- |
| Arguments | identical to the Python runtime (`python3 -m rutis <channel> … <project>`) |
| Working directory | `project` (the Gradle root) |
| Classpath | the runtime jar (a fat jar with kotlin-stdlib, kotlinx-coroutines-core, kotlin-reflect) + the project classpath (§8.1) |
| JVM options | defaults; no `-Xrs` (the child's own SIGTERM shutdown hook performs a graceful dispose); no `--enable-native-access` (no native calls) |
| Precheck | the host runs `java -version` first; below 21 it fails naming what is needed (like the `python -c "import rutis"` precheck) |

### 4.2 Channels

| Channel | How |
| --- | --- |
| `<socket path>` (Unix, default) | the host creates a socket in a private directory and passes the path as the channel argument; the runtime dials `SocketChannel.open(UnixDomainSocketAddress)` |
| `tcp:127.0.0.1:<port>` (Windows) | send `RUTIS_CHANNEL_TOKEN` + newline first, then remove it from the environment, as Python's `__main__.py` |
| `listen:ws://…` | remote runtime (D18, later phase) |
| `fd:3` | unsupported (D4); appearing in configuration is a startup error explaining why |

Framing: newline-delimited compact JSON, 16 MiB per frame; a larger frame closes the channel (as #173).

## 5. The contract

The same set as Python/Bun/Go: compat handshake (protocol 2), capabilities `["sync-wait", "reentrant-sync"]` (no `sync-stack`, no `objects` — references can be sent but not received); features `FEATURES = ["rows.v2", "hosts", "leaf", "scopes"]`.

The `mount` reply: `{ services: {}, features, implementation: { name: "io.github.arcships:rutis", version }, engine: { name: "kotlin", version: <Kotlin version>, vm: <JVM version> } }` (extra `engine` fields are not a protocol change, as Bun §3.3; `check` prints them).

| Control operation | Behaviour |
| --- | --- |
| `mount` | see above |
| `dispose` | unload every row, wait for calls in flight to drain |
| `rows.load(key, entry, config, isolate, inject, exports)` | resolve `entry` (§8.2) → create a child-first classloader → decode the configuration → run `apply`; on failure unload the row, then report |
| `rows.update(key, config)` | unload and load again (a leaf has no volatile fields) |
| `rows.unload(key)` | withdraw provides (send the `service(id, None, version)` notification) → cancel the row's coroutine scope → cleanups in reverse → `close()` the classloader |
| `rows.schema(entry)` | `{ config, inject, provides, version }`; an unknown entry is an error listing the available names |
| `hosts.provide(name, methods, label?)` / `hosts.withdraw(id)` | register/withdraw rutis service proxies (`scopes` rules as Python) |
| `release` / `get` | release an exported object / read an exported field |

- `ctx.provide` swaps the export slot to a fresh handle (`name` generation 1, `name#N` after; scoped id `name\0label`); the `service(id, handle, version)` notification goes out before the `rows.load` reply.
- Call-id prefix stays `node:` (overall design §3); call chains are rebased across sessions by Rust.

## 6. Threading: synchronous calls and reentrancy

```text
JVM process
 ├─ reader thread (platform): blocking frame reads, parse, dispatch
 ├─ one new virtual thread per incoming invoke/call/get/await/control operation
 ├─ one write lock: frames serialized
 └─ row coroutine scope: ctx.launch tasks run on virtual threads, cancelled and awaited at unload
```

| Item | Rule |
| --- | --- |
| Outgoing synchronous call | the current virtual thread blocks on a `CompletableFuture` completed by the reader thread; blocking a virtual thread is cheap, no loop to pump |
| Reentrancy | an incoming call can run at any time (a fresh virtual thread always exists); declares `reentrant-sync`. The cost, as Python/Go: **a plugin's service may be called again while it is itself inside a synchronous call; do not hold locks across calls into rutis** |
| Capability combination | `sync-wait` (outgoing synchronous calls carry `sync: true`, joining the host's wait-cycle detection) + `reentrant-sync`; **no `sync-stack`** — calls arriving during a synchronous wait run concurrently on other virtual threads, they are not stacked (the goroutine class; chain edges only) |
| `SyncWaitCycle` | never produced locally (the reader thread never runs plugin code; incoming calls never lack an execution unit); received ones surface as remote errors |
| Cancellation | a `cancel` frame cancels the call's coroutine scope and interrupts its virtual thread (interruptible blocking I/O; non-interruptible local computation runs to completion and its result is discarded — reported honestly) |
| Exceptions | an exception in a service method returns as an error graph (name/message/graph); an exception from `apply` fails the load with registered cleanups still run; an uncaught exception in a plugin-spawned platform thread ends the process and stops all rows (as Bun §3.6 / Go §8.3) |
| Plugin obligations | provided service objects safe for concurrent use; no locks held across rutis calls |
| Suspend bridging | a suspend method runs inside `runBlocking` on its virtual thread; `apply` and cleanups likewise |

**Answer to the Go design's proposal** (the open item in overall design §9): Kotlin supports making "every leaf runtime is reentrant; only the Cordis runtime is not" a rule — virtual threads make reentrancy the natural, cheap shape on the JVM. Swift (M3) still answers for itself; once it does, record the outcome in the overall design §9.

## 7. The Kotlin SDK

### 7.1 A plugin

```kotlin
// weather/src/main/kotlin/weather/WeatherPlugin.kt
package weather

data class Config(val city: String? = null)

interface Llm {
    suspend fun ask(question: String): String
}

class Weather(private val llm: Llm, private val city: String) {
    suspend fun today(): String = llm.ask("weather in $city") + " in $city"
    fun unit(): String = "celsius"
}

val plugin = rutis.definePlugin(
    name = "weather",
    inject = listOf("llm"),
    provides = rutis.provides("weather" to rutis.methodsOf<Weather>(sync = "unit")),
    config = rutis.schemaOf<Config>(),
) { ctx, config ->
    val llm = ctx.use<Llm>("llm")
    ctx.provide("weather", Weather(llm, config.city ?: "Oslo"))
}
```

| API | Rule |
| --- | --- |
| `definePlugin(...)` | returns a definition tagged `api = 1` (D14); a newer `api` than the runtime is refused naming both versions |
| `apply(ctx, config)` | may be suspend; throwing fails the load with registered cleanups still run |
| `ctx.use<T>(name)` | `name` must be in `inject`; same process yields the object itself (any interface — in-process calls are direct, no signature conversion); across processes, a proxy |
| `ctx.provide(name, value)` | until unload or the returned withdraw function; `value`'s type must match the `provides` declaration |
| `ctx.effect(cleanup)` | run in reverse at unload, may be suspend |
| `ctx.launch { }` | on a virtual thread of the row's coroutine scope; cancelled and awaited at unload (the `ctx.Go` counterpart); uncaught exceptions per §6 |
| `rutis.NoConfig` | no configuration |

### 7.2 Methods and shape

| Item | Rule |
| --- | --- |
| Service methods | public member functions (including suspend) of the type declared in `provides` |
| Naming | the method name as-is (Kotlin convention is already lower-case); `@RutisName("...")` overrides; a clash fails `definePlugin` |
| Signature | arguments decoded by position; extra is an error, missing use defaults; the return value is the result |
| Shape | `methodsOf<T>(sync = ...)` defaults to `async`; named methods are `sync` (D7) |
| `async` execution | runs on a virtual thread, replied with an asynchronous result reference; caller cancellation reaches the suspend function (coroutine cancellation) + thread interrupt |
| Errors | exceptions return as the error graph; the name is the exception class's simple name |

### 7.3 Configuration schema

| Kotlin | JSON Schema |
| --- | --- |
| `String` / `Boolean` / `Int`, `Long` / `Double` | `string` / `boolean` / `integer` / `number` |
| `List<T>`, `Array` | `array` + `items` |
| `Map<String, T>` | `object` + `additionalProperties` |
| data class | `object` + `properties`; constructor parameters without defaults are `required` |
| `@Doc("…")` | `description` |
| `rutis.schemaOf<T>()` | derived by reflection (kotlin-reflect inside the runtime; not needed on the plugin's compile classpath) |
| `Plugin.schema` | given explicitly, overriding derivation (the Go §6.3 counterpart) |

The configuration decodes into the data class by kotlinx serialization rules; a failure fails the load.

### 7.4 Values

| Kotlin value | Across processes |
| --- | --- |
| `null`, `Boolean`, `String` | copied |
| `Int` / `Long` / `Double` | copied; integers beyond ±2^53−1 are errors |
| `List<*>`, `Map<String, *>`, data classes | recursively copied (JSON rules) |
| exceptions | name and message copied |
| functions (`(A) -> B`, suspend functions) | by reference |
| `rutis.Ref(v)` | by reference; the proxy calls `v`'s public methods |
| values given to `ctx.provide` | always by reference |
| not receivable | `undefined` (decoded as a dedicated `Undefined` value, not null; Kotlin `null` encodes as null) |

## 8. Loading and hot reload

### 8.1 Plugin jars and the classpath

- A plugin is a jar (the Gradle `jar` output; fat jars such as shadow for many dependencies). A project may hold several plugin jars.
- Project classpath: conventional directories under `project` (e.g. `build/libs/` and `libs/`) plus the runtime jar; or listed explicitly by `rutis.json`/configuration (K2 decides which is least surprising).
- One child-first `URLClassLoader` per plugin (parent = the runtime jar's loader, holding kotlin-stdlib/SDK — shared with the runtime, avoiding duplicate classes); project dependency jars sit in the parent, plugin jars in the child.

### 8.2 Entry resolution

- `kt:weather.WeatherPlugin` (contains a dot) → used directly as a fully-qualified class name; load it and take its `plugin`/`Plugin` declaration.
- `kt:weather` (no dot) → scan the `rutis.json` descriptors (§8.3) at the root of each classpath jar and match by plugin name; several matches is an ambiguity error suggesting the fully-qualified form.

### 8.3 The `rutis.json` descriptor

One file at the jar root, generated by the template (§11) or written by hand:

```json
{
  "rutisPlugin": 1,
  "name": "weather",
  "version": "1.2.0",
  "entry": "weather.WeatherPlugin",
  "inject": ["llm"],
  "provides": { "weather": { "today": "async", "unit": "sync" } },
  "config": { "type": "object", "properties": { "city": { "type": "string" } } }
}
```

- In K2 only the runtime reads it, answering `rows.schema` and entry resolution; Rust reading jars offline (Go D5's benefit — unzipping one member) is a K5 optimization.
- If the descriptor disagrees with the loaded declaration's `provides`/`inject`, the runtime's actual declaration wins, with a warning.

### 8.4 Hot reload and unloading

- dev mode: a change of the jar file's mtime+size (the Python module stamp) → drop the old classloader, build a new one, run `apply` again. Only that plugin's loader is swapped; parent-layer dependency jars are not reloaded (to change those, restart the runtime).
- Unload in three steps: withdraw provides → cancel the row scope and await its (virtual-thread) tasks → cleanups in reverse; then `close()` the loader, drop every strong reference, and let GC reclaim it.
- **Leak-free obligations** (SDK guide + soak tests): `ctx.launch` tasks always end at unload; platform threads a plugin starts itself must be stopped by it (the SDK cannot reclaim them, as with Go's "goroutines ignoring ctx.Done()"); never register plugin classes into global registries (logging frameworks, shutdown hooks — the runtime cleans up the hooks it registered for the row).
- Soak acceptance: Metaspace usage stable (falling back after GC) after 200 load/unload cycles; a leak is a defect.
- DCEVM/JBR state-preserving hot swap: a dev option, not in the default distribution (non-standard JVM).

## 9. Repository and release

| Item | Rule |
| --- | --- |
| Location | `kotlin/rutis` (a Gradle project, module `io.github.arcships:rutis`) |
| Packages | `arcships.rutis` (SDK: `definePlugin`, types, testing); runtime entry `arcships.rutis.runtime.Main`; session layer `arcships.rutis.internal.peer` |
| Dependencies | the SDK compiles against kotlin-stdlib only; the runtime jar additionally carries kotlinx-coroutines-core and kotlin-reflect (fat jar) |
| Release | Maven Central (Sonatype Central Portal; `io.github.arcships` namespace verified via the GitHub organization); versions managed by `scripts/train.mjs` |
| Why a single artifact | as Python: the runtime has no third-party compile-time coupling, so installing the SDK forces nothing extra on plugin authors (D13) |
| `io.github.arcships.rutis.testing` | host-free test helpers: `load(plugin, config, services)` + strict mode (cross-process value semantics simulated), as Go's `rutistest` |
| JDK support | baseline 21; CI matrix on 21 and the current LTS (25) |

## 10. Rust changes

| Change | Where |
| --- | --- |
| `Launcher::kotlin(java, runtimeJar, project)`: `java -cp <runtimeJar> arcships.rutis.runtime.Main`, cwd = project, **no** `inherit_fd` (→ dial-back); loopback on Windows | `rutis-bridge/src/runtime/process.rs` |
| `LocalRuntime::kotlin(project)` named `"kt"`; `.java()` to change the JVM | `rutis-bridge/src/runtime/local.rs` |
| features: rutis-bridge `kotlin = []`; rutis-loader `kotlin = ["runtimes", "rutis-bridge/kotlin"]` | two `Cargo.toml`s |
| Row resolution reuses `RuntimeResolver::modules` (prefix `kt:`, uncached, two-stage refresh); remote `Naming::Npm` rejecting `kt:` prefixes is the existing generic rule | no new code (verify only) |
| `RuntimeRowsPlugin` already takes a trait | no change |
| rutis-host: `config.rs` gains a `kotlin` field in `Runtimes` (`project`/`java`/`runtimeJar`); a mounting branch in `host.rs` + the `java -version` precheck; the remote language mapping gains `"kotlin"|"kt"` | `rutis-host/src/*` |

## 11. rutis-host

```json
{
  "runtimes": { "kt": { "project": "." } },
  "rows": [{ "id": "weather", "name": "kt:weather", "config": { "city": "Oslo" } }]
}
```

| Field | Default | Meaning |
| --- | --- | --- |
| `project` | `.` | the Gradle project root, also the working directory; plugin jars resolve from here |
| `java` | `JAVA_HOME` → `PATH` | the JVM executable |
| `runtime` | the runtime jar from the project's Gradle cache, else `~/.rutis/kotlin/rutis-<version>.jar` | the runtime fat jar location |

| Command | Behaviour | Phase |
| --- | --- | --- |
| `check` | lists the JVM version, SDK/plugin API versions, each jar's plugins and versions, ambiguities, each row's jar | K3 |
| `dev` | without `rutis.dev.json`, a `build.gradle.kts` means a Kotlin project: watch `.kt`/`build.gradle.kts` → `gradle -q jar` (the project's own build; the host does not compile) → on success trigger the row reload for that jar; on failure print errors while the old code keeps running | K4 |
| `new <name> --lang kotlin` | a Gradle project template: `build.gradle.kts` (kotlin("jvm"), depending on `io.github.arcships:rutis`), `src/main/kotlin/...` plugin, `rutis.json`, tests (`rutis.testing`), `rutis.dev.json`, a publish workflow (tag → `gradle test`, `check`, publish to Central) | K4 |
| `run` | needs no toolchain beyond a JDK (the Gradle wrapper comes with the template) | — |

## 12. Tests

| Test | Where | Content |
| --- | --- | --- |
| Session unit | inside `kotlin/rutis` | encoding, reference counting, callbacks during sync calls, cancellation, call-chain context, concurrency |
| Session conformance | `rutis-bridge/tests/runtime_conformance.rs` + a Kotlin `conformance` entry | the full `session::testing::session` (references, callbacks, errors, malformed frames ending the session) |
| Contract × channels | `session_matrix.rs` gains `Kotlin` | `runtime::testing::runtime`: socket dial-back; loopback on Windows |
| Kotlin versions of the Python tests | `kotlin_runtime.rs` (bridge), `kotlin_rows.rs` (loader, incl. hot reload and a Metaspace soak) | describe, exported loading, host leases, callbacks during sync calls, unload, row reload |
| Columns in the existing matrix | `cancellation.rs`, `error_shape.rs`, `rpc_callbacks.rs`, `process_exit.rs`, `live_objects.rs`, `sync_cycles.rs` (kt+py, kt+node) | the same assertions per language |
| Runtime conformance | `multilang.rs` (feature `kotlin`) | Kotlin leaf plugins; several languages cold-start and call each other; revoking a provider stops only its users; `inject` gating |
| SDK unit | `kotlin/rutis` `testing` helpers | host-free loading, strict-mode value semantics |
| CI | `.github/workflows/ci.yml` | a job builds `kotlin/` with Gradle before the cargo tests; a JDK 21/25 matrix; `train.mjs` version consistency gains the maven coordinates |

## 13. Phases

| Phase | Work | Acceptance |
| --- | --- | --- |
| K1 research spike | measure the §14 checklist: a virtual-thread reentrancy prototype, dial-back socket frame round-trip latency, Metaspace behaviour over URLClassLoader load/unload cycles, Gradle incremental build time, the JDK discovery matrix | findings recorded back here; the design revised where a check fails |
| K2 session layer | the full `internal/peer` + the conformance entry | `kotlin_meets_the_session_contract` passes |
| K3 runtime + SDK + Rust | contract operations, load/unload, projection; `Launcher::kotlin`, features, host mounting | `kotlin_runtime.rs`/`kotlin_rows.rs`/`multilang.rs` Kotlin columns pass; the same compatibility suite as Python |
| K4 developer flow | templates, `dev`, `check`, Maven Central publishing, train.mjs | `rutis-host new --lang kotlin` through `dev` hot reload end to end; the coordinates resolve |
| K5 optimizations and remote | `listen:ws` (protocol 3 + `signals`), Rust offline resolution reading `rutis.json`, an on-demand/idle decision, AppCDS | a remote Kotlin runtime passes `websocket_conformance`; offline resolution starts no process |

## 14. K1 verification checklist (measure before implementing)

1. **Synchronous round-trip**: port the Python benchmark onto the dial-back unix socket (expected in the 25–60 µs range).
2. **Virtual-thread reentrancy**: incoming calls during a synchronous wait run on fresh virtual threads; `synchronized` pinning compared on 21 vs 24.
3. **Classloader unloading**: the Metaspace curve over 200 load/unload cycles (with coroutines and kotlin-reflect in the parent layer).
4. **Cold start**: JDK 21/25 time from launch through the first plugin load to the `mount` reply; before/after AppCDS.
5. **Dev loop**: incremental `jar` time for a single-file change with a resident Gradle daemon (expected: seconds).
6. **JDK discovery**: error presentation for `JAVA_HOME`/PATH/missing `java`/too-old versions on macOS, Linux, Windows.

## 15. Open questions

- **on-demand/idle** (D19): a resident JVM (tens to a couple hundred MB) is heavier than Python; whether to copy Go's `GoRuntimes` "start on demand, stop when idle" is decided with data in K5.
- **Schema derivation**: reflection (D20) versus kotlinx-serialization's schema module, or both (the latter is more idiomatic for `@Serializable` users but adds a compile-time dependency).
- **Project classpath convention** (§8.1): `build/libs` vs `libs` vs an explicit manifest, decided in K2.
- **Release infrastructure**: train.mjs has no maven precedent; Central Portal namespace verification and signing setup.
- **`synchronized` pinning on 21–23**: a documented warning (D3 today) or raise the baseline to 24/25 (dropping 21 LTS while still supported).
- **Swift (M3)**: this design votes yes on "leaf runtimes are reentrant"; once the Swift design answers, record it in the overall design §9, and revisit this item if it disagrees.

## Appendix A: why not embedded, and not Kotlin/Native

**Embedding a JVM (JNI Invocation API / the jni crate / FFM)**: technically mature (`JNI_CreateJavaVM`), but in direct conflict with host stability first — a plugin's native crash, heap OOM, GC pauses, and the JVM's signal handlers (`SIGSEGV` implicit null checks, shutdown hooks; `-Xrs`/`-XX:+UseSignalChaining` needed to coexist with the host) all land inside the Rust host process; after JEP 472, JNI/FFM warn by default and will refuse later, so the host manages `--enable-native-access`; JNI thread attach/detach and local/global reference pairing are permanent complexity. A cross-process synchronous call costs ~30–60 µs, adequate for plugin service semantics; only a hard "microsecond synchronous call" requirement reopens this route (and even then FFM only covers Java→native — Rust→JVM remains the Invocation API).

**Kotlin/Native linking the rutis core directly**: it can only export a C ABI via `-produce dynamic` + `@CName` — no library for Rust to link statically; cinterop is `@ExperimentalForeignApi`; above all it gives up the JVM ecosystem, which for agent-written plugins is the main value. If a "single binary, no JVM" need appears later, the shape is "a Rust staticlib embedded into a Kotlin/Native executable" as an additional target, not a replacement for this design.

**GraalVM native image**: its closed-world assumption fundamentally conflicts with loading plugins at run time — excluded; startup optimization uses AppCDS (K5).
