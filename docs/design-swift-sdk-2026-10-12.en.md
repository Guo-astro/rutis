# The Swift SDK: the macOS plugin runtime and hosting plugins in Apple apps (design draft)

[中文](design-swift-sdk-2026-10-12.md)

Status: final, not implemented. Date: 2026-10-12; final revision 2026-10-14 (the app part now follows [app integration](design-app-integration-2026-10-14.en.md), and remote connections support listening and dialing out, see §16). Base: `main` `a763cdc` (0.10.0).
Depends on: [the multilang master plan](design-multilang-runtimes-2026-10-03.en.md) (this is its M3; the revision of its dlopen approach is appendix A), [app integration](design-app-integration-2026-10-14.en.md) (the generic part of shape ②), [remote runtimes: dial-out and dynamic access](design-remote-runtime-dial-2026-10-14.en.md) (connections), [the Go plugin runtime](design-go-runtime-2026-10-10.en.md) (shape ① reuses its decisions and machinery; a side-by-side is appendix C), [the plugin API](guide/plugin-api.en.md).

Out of scope: a Swift-side Cordis; build systems other than SwiftPM; the host compiling Swift code; Linux and Windows (§13); app foreground/background, background survival, push wakeup (the business layer, see app integration §7); App Intents and other ways for the Apple ecosystem to call back into rutis; running rutis itself on the device.

## 1. Decisions

| # | Decision | Section |
| --- | --- | --- |
| S1 | Two hosting shapes share one SDK: ① macOS plugin binaries (local, multi-process); ② apps (plugins hosted inside iOS / iPadOS / macOS apps) | 2 |
| S2 | Shape ①: one binary, one runtime process, holding one or more plugins as its `main.swift`'s `Rutis.serve(...)` says; the host can run several Swift runtimes at once | 3 |
| S3 | Shape ① row name `swift:<plugin>`, routed by `SwiftResolver` through manifests; qualified form `<runtime>:<plugin>`; the runtime name comes from the file name (prefix `swift-`) | 3 |
| S4 | Shape ① binaries describe themselves: `--rutis-manifest` prints the manifest, so resolving needs no running process; while running, the startup manifest is what counts, and a replaced file takes effect after a restart | 3 |
| S5 | Shape ①: start on demand, stop when idle; `eager` starts all and never stops | 3 |
| S6 | Shape ②: plugins compile into the app, are written in the rutis model and driven by rutis; the SDK gives the app only `start` / `stop`; the app dials out | 7 |
| S7 | Calls within one process skip IPC; across processes and languages, rutis forwards by name | 4 |
| S8 | One Task per incoming call; declares `sync-wait` and `reentrant-sync`, not `sync-stack`. Applies to both shapes | 5 |
| S9 | The call chain lives in a `@TaskLocal`; a wait suspends the current Task and never blocks a thread; methods default to `async`, `.sync` marks the rest; calling a peer's `sync` method sends a frame with `sync: true` | 5 |
| S10 | Leaf plugin; plugin API version 2 (with leaf events `ctx.on` / `ctx.emit`); config decoded via `Codable`, its schema written by hand on the config type | 4 |
| S11 | Providing services: an explicit closure table (Swift has no method reflection) | 4 |
| S12 | Using services: a `RutisClient` struct of closure fields, signatures inferred with parameter packs; signatures that infer poorly fall back to 0–3-argument overloads | 4 |
| S13 | Values split along the type system: value types copy as data, closures cross as references; objects need an explicit `Rutis.ref` | 4 |
| S14 | Remote connections support both directions: dialing out (`dial:`) and listening (`listen:`); dialing out is recommended for app hosting | 6 |
| S15 | The host does not compile; `run` only runs ready-made binaries, `dev` only rebuilds the project itself; binaries of different SDK versions coexist | 10, 11 |
| S16 | The SDK lives at `swift/rutis`, distributed through the repository-root `Package.swift`, versioned by the main release line's `v` tags; the core depends only on Foundation | 12 |
| S17 | Platforms: macOS 14+ (both shapes), iOS / iPadOS 17+ (shape ②); Linux later, Windows out | 13 |

## 2. Two hosting shapes

| | ① macOS plugin binary | ② App |
| --- | --- | --- |
| Use | Mac system capabilities (EventKit, Contacts, Accessibility, screen capture, Core ML), AppleScript / AX automation | the app offers its own capabilities and those of its device as rutis plugins |
| Who starts it | the rutis host starts the process on demand | the app itself; the business layer decides when to `start` / `stop` |
| Where plugins come from | plugins compiled into the binary, described by its manifest | plugins compiled into the app, shipped with the app |
| Connection | local channel (`fd:3` / `unix:` / `tcp:`); dial-out when remote | dials out to a rutis access point |
| Authorization (TCC / entitlements) | on the plugin binary | on the app |
| Machinery | `SwiftResolver` / `SwiftRuntimes` (shared with Go) | [dial-out and dynamic access](design-remote-runtime-dial-2026-10-14.en.md) (one instance per connection) |

Plugin code is the same in both shapes: one plugin definition can be `serve`d as a binary or handed to an app to host. §4–§6 hold for both shapes.

## 3. Shape ①: the macOS plugin runtime

### 3.1 Runtime model

```text
rutis host
 ├─ LocalRuntime::node / ::python ...
 ├─ SwiftRuntimes (SwiftResolver + lifecycle)
 │    ├─ plugins/swift/weather ── swift-weather ── weather
 │    ├─ plugins/swift/kits   ── swift-kits    ── calendar, contacts
 │    └─ plugins/swift/ml     ── not started (no row uses it)
 └─ LoaderPlugin + one RuntimeRowsPlugin per running runtime
```

- As the Go runtime: a row depends on `RuntimeRows#<runtime>` plus every name the plugin `inject`s (a leaf runtime; rutis gates every name); the services a plugin `provides` are published from the row's fiber to `host_key(name)`; when the process exits, all its rows stop, and rows using its services stop with them.
- `isolate`, instance labels, service ids, exported handles: same as the Python and Go runtimes.
- TCC is attributed to the binary the plugin lives in (§8).

### 3.2 Binary and manifest

- A plugin author distributes: a Swift package (exporting plugin definitions) plus `cmd/<name>/main.swift` (`serve`ing its own plugins), or one package with several plugins compiled into one binary.
- The host accepts any executable that answers `--rutis-manifest` (sources in A.5).

```swift
// example.com/kits/cmd/kits/main.swift
import Rutis
import WeatherKits

Rutis.serve(WeatherKits.calendar, WeatherKits.contacts)
```

| `serve` rules | |
| --- | --- |
| Arguments | `--rutis-manifest`: print the manifest and exit; otherwise `<channel> [--id <endpoint>] [--peer <endpoint>] <project>` |
| Duplicate plugin names | exit at startup, naming both definitions |
| Registration | only in `serve`'s arguments; no static-initializer registry |
| Subcommands | `Rutis.serveArgs(args, defs...)`; also `Rutis.serveConn(channel, defs...)` (in-memory session, for tests) |

```json
{
  "manifest": 1,
  "sdk": "0.11.0",
  "pluginApi": 2,
  "runtime": "rutis-swift-runtime:1",
  "platform": "macos/arm64",
  "plugins": {
    "calendar": {
      "config": { "type": "object", "properties": { "calendars": { "type": "array", "items": { "type": "string" } } } },
      "inject": [],
      "provides": { "calendar": { "events": "async", "units": "sync" } },
      "version": null
    }
  }
}
```

| Item | Rule |
| --- | --- |
| Output | stdout, exit code 0; no channel connection, no `apply` |
| Plugin entries | same as the `rows.schema` reply, generated by the same code |
| `runtime` | the SDK marker string `rutis-swift-runtime:1`: directory scans only execute files containing it |
| `platform` | `macos/<arch>` (universal binaries report `macos/universal`); the host reports "not built for <os>/<arch>" on mismatch |
| `sdk` | the constant `Rutis.sdkVersion` |
| Plugin `version` | a static string the author writes, `null` by default |
| Execution timeout | 30 seconds (as Go: the first run of a new binary on macOS waits for the system to assess it) |
| Cache key / bypass / trust | as Go: path, size, mtime (Unix adds ctime); bypassed on start, `restart`, `check`; only sources given by the configuration are executed |

### 3.3 `SwiftResolver`

In rutis-loader (feature `swift`), implements `Resolver`, structurally the same as `GoResolver`:

| Item | Rule |
| --- | --- |
| Sources | `SwiftBinaries`: a file list plus a directory list; directories are rescanned once when an unknown plugin name appears |
| Candidates in a directory | executable, not starting with `.`, containing the SDK marker string; only candidates are executed |
| Manifest failure | in `binaries`, a configuration error; in a directory, skipped with a diagnostic; `check` lists them and exits non-zero |
| Runtime name | strip extension → lowercase → non-`[a-z0-9-]` to `-` → prefix `swift-` (`Net.Kits_v1` → `swift-net-kits-v1`) |
| `swift:<plugin>` | search all manifests; none: `NotFound`; several: an ambiguity error suggesting `<runtime>:<plugin>` |
| `<runtime>:<plugin>` | searches only that runtime's manifest |
| Name collisions | with other runtimes (`node`, `py`, `bun`, `go`, remote) or each other: a configuration error in `binaries`; skipped with a diagnostic in a directory |
| Resolution result | a leaf result; shares the "build `Resolved` from declarations" code with `GoResolver` |
| meta | `{ source: "swift", binary, runtime, version, sdk }` |
| Replaced file while running | resolve from the startup manifest; plugins only in the new binary report "replaced, effective after `restart("<runtime>")`" |
| Second-stage release | `RuntimeRowsPlugin` already takes a trait (the Go G1 change); `SwiftResolver` implements the same trait per runtime |

The only differences from `GoResolver` are three constants (marker string, prefix, feature name); the implementation goes through shared code (§9).

### 3.4 `SwiftRuntimes`

Structurally the same as `GoRuntimes`, in rutis-loader, mounted after the loader.

```rust
let swift = Arc::new(SwiftResolver::new(SwiftBinaries::new().dir("plugins/swift").file("bin/weather")).with_catalog(&catalog));
chain = chain.with_shared(swift.clone());
root.plugin(SwiftRuntimes::new(swift, project).idle(Duration::from_secs(60)));
```

| Item | Rule |
| --- | --- |
| Structure | each running runtime = a child `Ctx`: `LocalRuntime::swift(binary, project).named(name)` + `RuntimeRowsPlugin`; stopping = dropping the child `Ctx` |
| Start | started when a row resolves to a runtime that is not running (plus a periodic check); cold starts run in parallel |
| Idle stop | after its last row unloads, wait `idle` (default 60 seconds); stop if no enabled entry resolves to it; at most one start / restart / stop in progress at a time |
| Crash | rows stop with the reason recorded; no automatic restart. Restart conditions: `SwiftRuntimes::restart(name)`, or the binary file changed |
| `eager()` / `runtimes()` | as Go |
| One fixed binary | no resolver needed: `LocalRuntime::swift` + `RuntimeResolver::modules` |

## 4. The Swift SDK (shared core)

At `swift/rutis` (SwiftPM target `Rutis`), Swift 5 language mode, toolchain 5.9 and up, deployment targets macOS 14 / iOS 17. Layered:

| Layer | Contents | Platforms |
| --- | --- | --- |
| Core | session protocols (compat / endpoint), leaf API, values, concurrency, `RutisTest` | macOS + iOS (Foundation only; touches no process or file-system facilities) |
| Channel·process | `fd:3`, `unix:`, `tcp:` (POSIX) | macOS (shape ①) |
| Channel·dial | the WebSocket client (Foundation `URLSessionWebSocketTask`), reconnecting per [the dial-out design](design-remote-runtime-dial-2026-10-14.en.md) §6 | macOS + iOS |
| Channel·listen | the WebSocket server (Network.framework `NWListener` + `NWProtocolWebSocket`), with the same semantics as Go's remote listening (one controller at a time, a new connection takes over) | macOS + iOS |
| serve packaging | `Rutis.serve` / `serveArgs` / the manifest CLI | macOS (shape ①) |
| App hosting | `RutisApp` (§7) | macOS + iOS (shape ②) |

### 4.1 Plugin

```swift
import Rutis

struct Config: RutisConfig {          // the config type carries both schema and decoding
    static let schema = Schema.object(
        properties: ["city": .string(description: "The city to look up")],
        required: ["city"])
    var city: String                   // synthesized Codable
}

struct LLM: RutisClient {             // using services: a struct of closure fields
    var ask: (RutisCtx, String) async throws -> String
    var listModels: ((RutisCtx) async throws -> [String])?   // declare only the methods used
    init(_ s: RutisService) {         // built from the proxy; parameter packs infer from the field type
        ask = s.method("ask")
        listModels = s.method("listModels")
    }
}

final class Weather {
    var llm: LLM; var city: String
    func today(_ ctx: RutisCtx, _ question: String) async throws -> String { ... }
}

let weather = Rutis.define(Plugin(
    name: "weather",
    inject: ["llm"],
    provides: ["weather": Shape(.async("today"), .sync("unit"))],   // declare shapes (into the manifest)
    config: Config.self,
    apply: { ctx, config in                                          // the implementation (at load time)
        let llm = ctx.use("llm", LLM.self)
        let weather = Weather(llm: llm, city: config.city)
        ctx.provide("weather", [
            .async("today") { try await weather.today($0, $1) },
            .sync("unit") { _ in "celsius" },
        ])
        ctx.effect { print("weather unloaded") }
    }))
```

| API | Rule |
| --- | --- |
| `Rutis.define` | validates the declarations (names, closure table matching the shape) and returns a definition that can be served or handed to an app to host |
| `apply` throws | the load fails; cleanups registered so far still run |
| `ctx.use(name, C.self)` | `C: RutisClient`; `name` must be in `inject`; a same-process provider hands over its local implementation |
| `ctx.provide(name, methods)` | the closure table must match the shape declared in `provides`; lasts until unload or the returned withdraw function |
| `ctx.effect(cleanup)` | `cleanup` may be `async`; runs in reverse order at unload |
| `ctx.on(name, listener)` | listens for events on this runtime process's event bus (those the host sends in via `emits`, and those emitted by same-process plugins via `ctx.emit`); unregistered at plugin unload, or immediately by the returned function; listeners carry a call chain and are cancelled at plugin unload |
| `ctx.emit(name, args...)` | dispatches to this process's event bus, calling listeners synchronously in registration order and returning at the first error; does not cross processes. `internal/*` is not available |
| `ctx.spawn { ... }` | runs in a child Task, errors captured to stderr; cancelled and awaited at unload |
| `RutisCtx` | the row scope: cancellation (unload cancels the row's Task tree), `project`, the call chain (§5) |
| `Rutis.NoConfig` | no configuration |

### 4.2 Providing services: the closure table

Swift has no method reflection (Mirror cannot enumerate a type's methods, nor synthesize methods of arbitrary signatures), so instead of Go's reflected `MethodsOf` or Python's `inspect`, the table is **explicit closures**:

- Declaration (at define time): `Shape(.async("today"), .sync("unit"))` → manifest and `rows.schema`.
- Implementation (at apply time): `.async("today") { (ctx, city) in ... }` with any closure signature (arguments decode positionally into the closure's parameter types; return values encode per §4.5); the SDK checks the table against the shape.
- Same-process calls: the closure is called directly, no encoding, no crossing processes (S7).
- Optional later: an `@RutisService` macro generates shape and closure table from the type.

### 4.3 Using services: `RutisClient`

```swift
protocol RutisClient { init(_ service: RutisService) }
```

- The struct's closure fields are the methods used: the field name is the method name; write only the methods you use; every field must match a method the service declares, or construction throws the error to `use`.
- `s.method("ask")` is a parameter-pack generic: `func method<each A, R>(_ name: String) -> (RutisCtx, repeat each A) async throws -> R`, inferring the closure signature from the field type (verified with Swift 6.3 on this machine, appendix E.3); signatures that infer poorly use a set of overloads for 0–3 arguments.
- Cross-process proxies are all `async`: there is no blocking `callSync`. Calling a method the peer declares `sync` sends a frame with `sync: true`, and the current Task suspends to wait (§5).

### 4.4 Configuration

| Item | Rule |
| --- | --- |
| Decoding | `JSONDecoder`, `Config: Codable`; a failure fails the load |
| Schema | `RutisConfig.schema`, written by hand (Swift reflection cannot produce a schema; like Python's hand-written `Config`) |
| Value mapping | JSON Schema semantics as Go §6.3 |

### 4.5 Values

| Swift value | Across processes |
| --- | --- |
| `nil`, `Bool`, `Int` (within ±2^53−1), `Double`, `String` | copied; out-of-range integers error |
| `Optional`, `Array`, `Dictionary<String, T>`, `Codable` structs / enums | copied recursively, encoded per `Codable` |
| Closures (function references) | reference; arguments and return values encode per this table |
| `Rutis.ref(v)` | reference; `v: AnyObject & RutisExportable` (carries its own closure table, §4.2); the far end gets a proxy calling its methods |
| A class instance | cannot cross processes without an explicit `ref`: an error suggests a struct or `Rutis.ref` |
| `Error` | name and message copied; the name is the error type name (`String(reflecting:)`), overridable via `RutisErrorName` |
| `RutisFuture` | an async result; `await fut.value()` |
| `Data`, non-string-keyed dictionaries, cycles, non-`Codable` | cannot cross processes (as Go's `[]byte` and friends) |

- The boundary comes from the type system itself (appendix D).
- Receiving: function types → closures calling the remote function; everything else decodes via `Codable` into the declared type. Object references are not received (as Go).

## 5. Concurrency, call chains, cancellation

Applies to both shapes.

| Item | Rule |
| --- | --- |
| Session structure | one reader thread (blocking channel reads; frames enter the async world through an `AsyncStream`; blocking reads never sit on the cooperative pool); one actor serializes frame writes; one new Task per incoming `call` / `invoke` / `get` / `await` / control operation; row state under its own locks |
| Reentrancy | any incoming call may run at any time; declares `reentrant-sync`. Does not declare `sync-stack`: calls run concurrently on separate Tasks with no single-stack order (the Go situation, see the master plan §9) |
| Synchronous waits | calling a peer's `sync` method sends a frame with `sync: true`, and the current Task suspends on a continuation until the reply — **no thread is blocked**; declares `sync-wait` and takes part in the host's wait-cycle detection (#228). While waiting, other Tasks keep executing incoming calls |
| Plugin requirements | service closures are concurrency-safe; no locks held while calling other services (goes in the guide) |
| Call chain | `@TaskLocal var path: [String]`: an incoming call sets `path + [id]` in its Task; outgoing calls take the chain from the current TaskLocal; `apply` and cleanups carry the `rows.load` / `rows.unload` chains |
| `sync` methods | a `.sync` closure in the closure table must not make cross-process calls (all proxies are async, so this does not compile); the only behavioral difference from Go; goes in the guide |
| Incoming cancellation | a `cancel` frame → `Task.cancel()` on that call; `Task.checkCancellation()` throws `CancellationError` |
| Outgoing cancellation | `async`: send `cancel`, throw `CancellationError` immediately, drop late results; `sync`: throw immediately, drop late results, the peer completes anyway |
| Row unload | cancels the row's Task tree; after withdrawing the service, in-flight calls are awaited (the deadline is the rutis side's) |
| `SyncWaitCycle` | never produced here; received as a `Rutis.RemoteError`, `isSyncWaitCycle(err)` |

## 6. Channels and contract

### 6.1 Channels

| channel | Shape | Implementation |
| --- | --- | --- |
| `fd:3` | ① | Unix local inheritance: `dup` + POSIX read/write |
| `unix:<path>` | ① | dial-back: POSIX socket |
| `tcp:127.0.0.1:<port>` | ① (reserved) | send `RUTIS_CHANNEL_TOKEN` + newline first, then remove it from the environment |
| `dial:wss://…` | ① remote, ② (recommended) | WebSocket client, subprotocol `rutis.3`; credentials and id per the dial-out design §5; reconnect per the dial-out design §6 |
| `listen:ws://…` / `listen:wss://…` | ① remote, ② | WebSocket server; non-loopback must be `wss://` (`RUTIS_CERT` / `RUTIS_KEY` semantics); credentials, heartbeat and message size limit as in Go's remote listening |

- Line framing; the per-message size limit as [#173](https://github.com/arcships/rutis/issues/173); the `project` argument is provided through `Rutis.project` (in shape ②, the app's agreed directory).
- Non-loopback addresses must use `wss://`; a self-signed certificate is trusted through the `RUTIS_CA` semantics (an SDK API parameter).

### 6.2 Contract

Compat protocol version 2 (local channels), endpoint protocol 3 (remote, listening and dialing out); features `["rows.v2", "hosts", "leaf", "scopes", "events"]`; capabilities `signals`, `sync-wait`, `reentrant-sync`, plus `runtime` declared in the endpoint handshake; call ids in this session use the `node:` prefix (as every leaf runtime).

| Control operation | Behavior |
| --- | --- |
| `mount` | `{ services: {}, features, plugins, implementation: { name: "github.com/arcships/rutis", version: Rutis.sdkVersion }, engine: { name: "swift", version: <the toolchain version embedded at build> } }`; `plugins` is the same as the manifest's `plugins` (the dial-out design §4) |
| `rows.schema(entry)` / `rows.load` / `rows.update` / `rows.unload` | as Go §8.2; `rows.load` ignores `inject` (gating is done by rutis) |
| `hosts.provide` / `hosts.withdraw`, `release` / `get`, `dispose` | as Go |

### 6.3 Exit and cleanup

| Situation | Rule |
| --- | --- |
| Errors thrown by `apply`, methods, cleanups | captured as error replies |
| `fatalError` / `try!` / force-unwrap crashes, uncaught errors in `ctx.spawn` | not catchable in Swift → in shape ① the process exits and all rows stop (aligned with Go); in shape ② the app crashes: the connection drops, and the rutis side closes the instance and withdraws its services |
| Session already ended | unload all rows, cleanups take at most 5 seconds in total; past that, remaining cleanups are abandoned and the affected rows logged to stderr; shape ① `exit(0)`, shape ② returns to the disconnected state (the app stays alive) |
| `rows.unload`, `dispose` deadlines | none set here (as Go / Python); the deadline is the rutis side's |

## 7. Shape ②: hosting in apps

The generic model is in [app integration](design-app-integration-2026-10-14.en.md): plugins are written in the rutis model, compiled into the app and driven by rutis; the SDK gives the app only `start` / `stop`; the app dials out to a rutis access point, and each connection is one instance on the rutis side. This section covers only the Swift side.

```swift
let rutis = RutisApp(
    plugins: [health, location, device],         // plugin definitions compiled into the app (§4.1)
    connect: "wss://rutis.example.com:7443/rutis",
    id: "kim-iphone",                             // self-reported id (the dial-out design §5)
    token: secrets.rutisToken)

try await rutis.start()   // connect; from then on rutis loads plugins per its configuration
await rutis.stop()        // all plugins clean up in reverse order, then disconnect
```

| Item | Rule |
| --- | --- |
| `start()` | connect, greet, report `plugins`; after a disconnect, reconnect automatically per the dial-out design §6 until `stop()` |
| `stop()` | unload all rows (cleanups in reverse order), close the connection; `start()` may be called again |
| State | `rutis.state` (`stopped` / `connecting` / `connected`) is observable for the business layer to display; the SDK makes no lifecycle decisions from it |
| Suspended or terminated by the system | no special handling: to the rutis side it looks like a disconnect |
| Packaging | the SDK links statically into the app (a SwiftPM library dependency); no dynamic loading |
| Authorization | entitlements (HealthKit and friends) and usage descriptions live in the app's Info.plist / provisioning profile; prompts point at the app; a denial = `apply` or a method call throws, and the error goes back to the caller as usual |
| Local network | connecting to a LAN address triggers iOS's local-network permission (`NSLocalNetworkUsageDescription`); goes in the SDK guide |
| Changing code | ship a new app version |
| Device information | rutis does not model devices; a plugin provides it when needed (e.g. a `device` plugin) |

## 8. TCC and signing

Authorization attaches to whoever calls the system API: in shape ① the plugin binary, in shape ② the app.

| Item | Shape ① | Shape ② |
| --- | --- | --- |
| Attribution | the runtime binary itself; the name in the prompt comes from its signature and embedded Info.plist | the app |
| Usage descriptions | a bare executable links its Info.plist into `__TEXT,__info_plist` (linker `-sectcreate`, SwiftPM `linkerSettings`); the `new --lang swift` template ships it | the app's Info.plist |
| Local development | ad-hoc signing is enough (`swift build` output is ad-hoc signed by default on Apple Silicon) | the app's development signing |
| Distribution | Developer ID signing + notarization + hardened runtime | the app's own distribution channel |
| Library validation | nothing is dlopened, so no library-validation problem (appendix A.4) | same |
| Quarantine attribute | the Go `xattr` check and reporting is reused | — |
| Granularity | plugins in one binary share one TCC identity; different granularity means different binaries | plugins in one app share the app's authorization |

Acceptance: an EventKit plugin runs in a signed binary and the prompt points at that binary (shape ①, the M3 text); in an iOS app, a HealthKit plugin's prompt points at the app, and after a denial the error goes back to the caller on the rutis side (shape ②).

## 9. Rust-side changes

| Change | Where |
| --- | --- |
| Extract the language-independent parts of `go.rs` into a shared module: manifest execution and caching, directory scanning and marker sniffing, runtime-name derivation, the on-demand lifecycle and restart, file-replacement semantics; `go.rs` becomes a thin wrapper with unchanged behavior, guarded by the existing tests | `rutis-loader/src/runtime/binary.rs` (new) |
| `SwiftResolver`, `SwiftBinaries`, `SwiftRuntimes` | `rutis-loader/src/runtime/swift.rs` (new, thin wrapper) |
| `swift_runtime_name`; `Launcher::swift` (the same spawn path as `Launcher::go`); `LocalRuntime::swift` | `rutis-bridge/src/runtime/{local,process}.rs` |
| features: rutis-bridge `swift = []`; rutis-loader `swift = ["runtimes", "rutis-bridge/swift"]`; rutis-host enables `swift` on its dependency | the three `Cargo.toml`s |
| Remote npm naming rejects `swift:` and `<swift runtime>:` prefixes | already implemented; tests only |
| Shape ②: `runtimes.remote[].language` accepts `"swift"`; the rest is in the dial-out design §7 | `rutis-host/src/host.rs` |
| `scripts/train.mjs` gains the version regex for `swift/rutis/Sources/Rutis/Version.swift` | `scripts/train.mjs` |

If the `go.rs` extraction turns out too entangled, `swift.rs` copies first and unifies later; deviations are recorded here with the implementation.

## 10. rutis-host

```json
{
  "runtimes": { "swift": { "dir": "plugins/swift", "binaries": ["bin/weather"], "start": "on-demand", "idle": 60 } },
  "rows": [{ "id": "weather", "name": "swift:weather", "config": { "city": "Oslo" } }]
}
```

The fields are the same as `runtimes.go` (`dir` / `binaries`, at least one; `start`: `on-demand` (default) / `eager`; `idle` in seconds; plus `project`). Shape ②'s configuration is in the dial-out design §4.

| Command | Behavior | Phase |
| --- | --- | --- |
| `check` | per binary: runtime name, platform, SDK / plugin API versions, plugins; ambiguities; each row's target binary; failed candidates; the quarantine attribute | S2 |
| `dev` | without `rutis.dev.json`, detect a Swift project by `Package.swift`; rebuild and restart per §11 | S2 |
| `new <name> --lang swift` | the plugin target, `cmd/<name>/main.swift`, the `Info.plist` with usage descriptions and `linkerSettings`, tests, `rutis.dev.json`, the release workflow (release artifacts are universal binaries) | S2 |
| `run` | no Swift toolchain needed | — |

## 11. Changing code (shape ①)

As Go §12: `dev` watches `.swift`, `Package.swift`, `Package.resolved` → `swift build -c release` into a new cache file `<name>-<seq>` → on success, replace that runtime's file and restart it; on failure print the error, the old process continues. Deploying: replace by rename → `SwiftRuntimes::restart("<runtime>")`.

## 12. Repository and releases

| Item | Rule |
| --- | --- |
| Location | `swift/rutis`: target `Rutis` (core + API), `RutisTest`; the session layer in `Sources/Rutis/Internal/Peer`; the conformance executable target `RutisConformance` |
| Manifest | a `Package.swift` at the **repository root**, targets pointed at `swift/rutis/Sources/...` via `path:`; platforms `.macOS(.v14)`, `.iOS(.v17)`. SwiftPM git dependencies only read the repository root (appendix E.1); the repository's packed size is about 18 MiB (2026-10-14), an acceptable clone cost for dependents |
| Versioning | `Rutis.sdkVersion` follows the main release line: the existing `v0.x.y` tags are exactly the form SwiftPM accepts (appendix E.2), so no new tag namespace; `from:` starts at the first release containing the root `Package.swift` |
| How dependents write it | plugin authors and apps share one line: `.package(url: "https://github.com/arcships/rutis", from: "0.11.0")` + `.product(name: "Rutis", package: "rutis")`; apps add the package directly in Xcode |
| Dependencies | core: Foundation only; dial-out uses `URLSessionWebSocketTask`, listening uses Network.framework; no uniffi / swift-bridge (appendix B) |
| Compatibility | within one protocol version, a binary / app of any SDK version runs on a host of any version, provided its plugin API is not above what the host supports |
| Language mode | Swift 5 mode to start, the API designed Sendable-friendly; the toolchain policy as Go: "the current two stable releases" (today 6.3 and 6.4) |
| Docs | `docs/guide/swift-plugin.md` (shape ①), `docs/guide/swift-app.md` (shape ②, including integration notes such as the local-network permission), Chinese and English; a Swift column in `docs/guide/plugin-api.md` |

## 13. Platforms

| Platform | Shape ① | Shape ② | Notes |
| --- | --- | --- | --- |
| macOS 14+ | ✅ | ✅ (Mac apps) | the Swift 5.9 runtime ships with the system, so parameter packs have no back-deployment problem |
| iOS / iPadOS 17+ | — | ✅ | no process spawning, no dynamic loading |
| watchOS / visionOS | — | later | join when needed |
| Linux | later | — | join when there is a real need |
| Windows | out | — | |

## 14. Tests

| Test | Where | What |
| --- | --- | --- |
| Session-layer unit tests | `swift/rutis/Tests` | encode/decode, reference counting, callbacks during a sync wait, cancellation, TaskLocal chains, concurrency, suspended waits not blocking |
| iOS core unit tests | same, CI adds `xcodebuild test -destination 'platform=iOS Simulator'` | the core layer compiles and tests on iOS (values, leaf API, in-memory session, the `RutisApp` state machine) |
| Session conformance | `rutis-bridge/tests/runtime_conformance.rs` + the `RutisConformance` target | `session::testing::session` |
| Contract × channels | `session_matrix.rs` gains `Swift` | `fd:3`, dial-back |
| Swift specifics | `rutis-bridge/tests/swift_runtime.rs` | cancellation reaching the Task; error shapes; `fatalError` exit withdrawing services; references and release |
| Manifest / binaries / on-demand | `rutis-loader/tests/swift_rows.rs` | the full set mirroring `go_rows.rs` |
| Runtime conformance | `multilang.rs` (feature `swift`) | a Swift leaf plugin; cross-language cold starts calling each other; a provider going away stops only its users; `inject` gating |
| Cross sync calls | `multilang.rs` | Node→Swift→calling back into Node (with the chain), Swift↔Go, Swift↔Python: no deadlock; a cross sync cycle between Node and Swift is detected as `SyncWaitCycle` |
| Listening | `rutis-bridge/tests/websocket_conformance.rs` gains a Swift remote end | the same assertions as Go's remote listening |
| Dial-out | `rutis-bridge/tests/dial_conformance.rs` gains a Swift column (the dial-out design §6) | backoff, rejected credentials, same-id takeover, the instance closing on disconnect, `stop` / `start` |
| Authorization (e2e) | the e2e suite | shape ① EventKit; shape ② iOS HealthKit: prompt attribution, the error going back after a denial. Clicking the prompts is a manual acceptance checklist, the rest is automated |
| Fixtures | `crates/rutis-loader/tests/fixtures/swift` | built into at least two binaries at test start |
| CI | `runtimes-swift` (a macOS runner; two stable toolchains; including the iOS Simulator unit tests); `test` and `network-macos` gain the Swift fixture build | `swift test` + `xcodebuild test`; `cargo test -p rutis-bridge --features swift,…`; `-p rutis-loader --features swift,…`; `-p rutis-host` |

## 15. Phasing

| Phase | Content | Acceptance |
| --- | --- | --- |
| S1 | the SDK core (session, leaf API, values, concurrency) + shape ①'s `serve` (`fd` / `unix`), manifest, `RutisTest`; the `binary.rs` extraction + the thin `go.rs` wrapper; `Launcher::swift`, the feature; `SwiftResolver`; `SwiftRuntimes` (`eager` only) | all of §14 except on-demand, dial-out and authorization pass; several binaries in parallel; the iOS Simulator unit tests pass; Go's existing tests stay green |
| S2 | shape ① on-demand start and idle stop; rutis-host (`runtimes.swift`, `dev`, `check`, `new --lang swift`); the guides; TCC / EventKit | the on-demand tests pass; `dev` restarts only its own runtime after a code change; the prompt points at the binary, a denial's error goes back |
| A1 | shape ②: the dial-out and listening channels, `RutisApp`; depends on the dial-out design's R-P1 | an app (simulator and device) dials out and connects, and loads plugins per the rutis configuration; after `stop` its users stop, after `start` they recover; same-named plugins on two devices each live in their own instance; the `dial_conformance` and `websocket_conformance` Swift columns pass |

A1 depends only on S1's session layer and the dial-out design's R-P1, and can run in parallel with S2.

Later (when needed): downloading prebuilt binaries (`swift add <URL>`), receiving object references, the `@RutisService` / schema macro, plugin versions generated by the build, watchOS / visionOS, Linux.

## 16. Departures from the master plan and the first draft

| Original | This design |
| --- | --- |
| Master plan M3's goal: "macOS system capabilities" | widened to hosting in Apple apps (shape ②); the generic part is in the app integration design |
| Master plan: "a signed auxiliary program loading plugin bundles with `dlopen`" | revised: one signed binary per plugin group (appendix A) |
| Master plan row name `swift:Calendar.bundle` | shape ① `swift:<plugin>`; shape ② `<runtime>:<plugin>` (the dial-out design) |
| Master plan §2.7 files Swift under "runtimes that load code by name" | filed with Go under "the plugin is a compiled program"; on iOS the integration unit is the app |
| Master plan §9 open question "Swift (M3) confirms reentrancy" | reentrant, and declares `sync-wait` (§5) |
| First draft shape ②a (the app listens, the host dials in) | listening kept as a channel capability (S14); dialing out recommended for app hosting: device addresses are unstable, certificates would need IPs |
| First draft shape ②b (the app dials out as a peer, export / import) | removed: replaced by runtime semantics driven by rutis (app integration D2); the export / import governance model is no longer needed |
| First draft background survival, APNs, App Intents | removed: foreground/background belongs to the business layer; calling back into rutis is out of scope |
| First draft capabilities declare only `signals`, `reentrant-sync` | adds `sync-wait`: without it the runtime is outside wait-cycle detection |
| First draft deployment target macOS 13 | macOS 14: the same Swift 5.9 runtime as iOS 17 |

## Appendix

### A. Why not a dlopen auxiliary program (the revision of the master plan's M3)

**A.1 Go (M4) landed first and built the machinery.** `GoResolver`, `GoRuntimes`, the two-stage release, on-demand start, restart, `check`, `dev`, `new` all exist (#227); shape ① attaches through shared code (§9). The dlopen route would need new bundle discovery and loading, a second API layer between the auxiliary program and its bundles, and a separate answer for manifests / caching / platform checks. On iOS, dlopen is not an option at all.

**A.2 Swift's cross-toolchain binary compatibility is not dependable.** When bundles and the auxiliary program compile separately, cross-module calls need module stability + library evolution, which cannot be guaranteed for "bundles by any author, built with any Swift version"; the workaround is pushing the boundary down to a plain C ABI, which means inventing another API layer. SwiftPM also has no loadable-bundle product type.

**A.3 TCC granularity.** One auxiliary program means one TCC identity: grant calendar access to plugin A, and plugin B in the same process can read calendars too. In the executable model each plugin group gets its own narrow grant, still satisfying M3's acceptance.

**A.4 Signing and library validation.** A signed auxiliary program with the hardened runtime that dlopens bundles not signed by the same team needs `disable-library-validation` (widening the attack surface) or every plugin author signing with one team. Executables and apps with statically linked plugins have no such problem.

**A.5 The loss and its substitutes.** What is lost: dropping a new bundle into a running process and having it load. Substitutes: the deployment composes several plugin packages into one binary in `cmd/main.swift` (as Go appendix H.3); or several plugins each in their own binary, started on demand, stopped when idle. In shape ② the equivalent is shipping a new app version.

### B. Swift facts

| Fact | Source |
| --- | --- |
| `@_cdecl` became official as `@c` in Swift 6.3 (SE-0495); this design needs no C ABI | [SE-0495](https://github.com/swiftlang/swift-evolution/blob/main/proposals/0495-cdecl.md) |
| macOS 10.14.4+ ships the Swift runtime; CLI binaries need not bundle a runtime | [swift.org ABI stability](https://swift.org/blog/abi-stability-and-apple/) |
| Releases land each March and September | [swift-evolution](https://github.com/swiftlang/swift-evolution/blob/main/README.md), [platform-support](https://swift.org/platform-support/) |
| UniFFI's Swift 6 support is marked partial; swift-bridge is 0.x and had a roughly 16-month hiatus; neither is adopted | [UniFFI Swift](https://mozilla.github.io/uniffi-rs/latest/swift/overview.html), crates.io |
| Blocking reads must not sit on the cooperative pool (its width is the core count); a dedicated thread + continuation is the standard approach | Swift concurrency docs |
| Linking an Info.plist into a bare executable's `__TEXT,__info_plist` (`-sectcreate`) is long-standing practice | CMake / swift-forums practice |
| `URLSessionWebSocketTask` is Foundation's WebSocket client, supporting subprotocols and custom TLS trust; Network.framework's `NWListener` + `NWProtocolWebSocket` is a WebSocket server with subprotocols and TLS | Apple documentation |
| iOS: no process spawning, downloading and executing code is forbidden, background apps are suspended, local-network permission (iOS 14+) | Apple platform documentation |

### C. Side-by-side with the Go runtime (shape ①)

| | Go | Swift |
| --- | --- | --- |
| Process model | one binary per plugin group | same |
| Self-description | `--rutis-manifest` | same |
| Row / runtime names | `go:<plugin>` / `go-<name>` | `swift:<plugin>` / `swift-<name>` |
| Execution unit per call | a goroutine (blocking wait) | a Task (suspending wait, no thread held) |
| Reentrancy / sync-stack | reentrant / not declared | same |
| sync-wait | declared | same |
| Call chain | `context.Context` | `@TaskLocal` |
| Method enumeration | reflection, `MethodsOf` | explicit closure table |
| Schema | reflected, overridable | hand-written |
| Values by default | data, `rutis.Ref` for explicit references | value types are data; classes need `Rutis.ref` |
| Calling out from a sync method | allowed (blocks the goroutine) | not allowed (all proxies are async) |
| panic / fatalError | captured as an error; uncaught in a goroutine → process exit | errors are `throws`; `fatalError` → process exit |
| Remote | listens (`listen:`); dialing out comes with dial-out design R-P3 | listens and dials out |
| Platforms | Linux / macOS / Windows | macOS (①) / macOS + iOS (②) |
| Distribution | git tag `go/rutis/vX.Y.Z` + the module proxy | root `Package.swift` + the main release line's `v` tags |

### D. Why the value boundary needs no `Ref`-style rule

Go needs an explicit `Ref` because a Go struct can be either data or an object, and the language does not distinguish them. Swift's value types (struct / enum) and reference types (class) are two separate worlds in the type system: structs copy as data; a class crosses processes only through an explicit `Rutis.ref` and must carry its own method table (Swift cannot reflect out its methods; the same mechanism as §4.2's closure table).

### E. Local PoCs (2026-10-12, Swift 6.3.2 / Xcode)

**E.1**: a root `Package.swift` declaring targets at a subdirectory path (`path: "swift/rutis/Sources/Rutis"`) builds and runs fine.

**E.2 SwiftPM's version-tag rule**: tested against a git dependency, `1.0.0` and `v1.1.0` are accepted as versions; `rutis-swift-1.0.0`, `swift/rutis/1.0.0`, `rutis-mono-1.2.0` are all ignored. Conclusion: the main release `v` tags are directly usable as SwiftPM versions.

**E.3 Parameter-pack contextual inference**: `func method<each A, R>(_:) -> (Ctx, repeat each A) async throws -> R` infers from the field's closure type at `LLM(ask: s.method("ask"), …)` and compiles. Signatures that infer poorly fall back to 0–3-argument overloads (S12).
