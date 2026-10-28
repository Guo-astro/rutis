# Write a Swift Plugin

[中文](swift-plugin.md)

From creating a project to publishing it, and to a host using it. Needs macOS 14 or later and Xcode 26 or later with the Swift 6.2 toolchain (the Command Line Tools alone build and run plugins, but `swift test` needs Xcode's test frameworks); the SDK itself is written in the Swift 5 language mode, and so is the project `new` makes.

Swift plugins follow the same conventions as TypeScript, Python and Go plugins (see the [plugin API](plugin-api.en.md)); as with Go, **a Swift plugin is compiled into a binary, and a binary is one runtime process in the host**. A binary holds one or several plugins; a host can run many binaries at once, starting each only when a row uses it. Swift is the language for what macOS offers its own programs: calendars and reminders (EventKit), contacts, accessibility and Apple events automation, screen capture, Core ML.

## 1. Create the project

```bash
rutis-host new weather --lang swift
cd weather
swift test
```

The project (a SwiftPM package):

| File | What it is |
| --- | --- |
| `Sources/WeatherPlugin/Plugin.swift` | The plugin (library `WeatherPlugin`, exporting `plugin`) |
| `Sources/weather/main.swift` | The binary: `Rutis.serve(plugin)` |
| `Tests/WeatherPluginTests/PluginTests.swift` | Unit tests with `RutisTest`, no host needed |
| `Info.plist` | Linked into the binary: its name and the usage descriptions macOS shows when the plugin asks for access (see [System access](#5-system-access)) |
| `Package.swift` | Depends on `https://github.com/arcships/rutis` (the products `Rutis` and `RutisTest`); `linkerSettings` link `Info.plist` |
| `rutis.dev.json` | Configuration of the local runtime and other plugins for testing |
| `.github/workflows/release.yml` | On a `v*` tag: tests, then a universal binary (arm64 + x86_64) on a GitHub release |

The first build compiles the SDK's macros (swift-syntax, at build time only); Xcode asks once to trust them (`-skipMacroValidation` in CI that runs `xcodebuild`).

## 2. Write the plugin

```swift
import Rutis

/// The configuration: a plain Decodable type; @RutisConfig derives its JSON Schema.
@RutisConfig
public struct Config: Decodable {
    /// the city to report on
    public var city: String
}

/// A service it uses, in any language: @RutisClient makes `LLMProxy`.
@RutisClient
public protocol LLM {
    func ask(_ question: String) async throws -> String
}

/// The service it provides: @RutisService makes its methods the service's.
@RutisService
public final class Weather: @unchecked Sendable {
    let llm: any LLM
    let city: String

    init(llm: any LLM, city: String) { self.llm = llm; self.city = city }

    public func today() async throws -> String {
        try await llm.ask("weather in \(city)") + " in \(city)"
    }

    public func unit() -> String { "celsius" }
}

public let plugin = Rutis.define(Plugin(
    name: "weather",
    inject: ["llm"],                                            // runs only while all are there
    provides: ["weather": .of(Weather.self, sync: ["unit"])],   // declares what it provides (check knows before running); unit sync, the rest async
    config: Config.self,
    apply: { ctx, config in
        ctx.provide("weather", Weather(llm: try ctx.use("llm", LLMProxy.self), city: config.city))
        ctx.effect { /* a cleanup, optional */ }
    }
))
```

In short:

| Topic | Rule |
| --- | --- |
| Methods | `@RutisService` makes every instance method that is not `private` or `fileprivate` a method of the service, its wire name the base name (`today`); arguments by position. Overloads, generic methods and function parameters that are not `async throws` are compile errors. Methods of a superclass or of a protocol extension are not seen: declare them in the type |
| Sync or async | `async` by default, whatever the Swift signature; `provides: [... .of(T.self, sync: ["unit"])]` marks synchronous ones. A `sync` method may itself be `async` and call other services: the Task waits without holding a thread. A sync method blocks a Node caller's event loop: keep methods that do I/O `async` |
| Using services | A protocol with `@RutisClient` (its methods `async throws`); `ctx.use("llm", LLMProxy.self)` returns an `any LLM`, the provider itself when it is in the same process and implements the protocol. Or call by name: `try await ctx.use("llm").call("ask", values: ["q"], as: String.self)` |
| Call chain | It travels with the Task (`@TaskLocal`): plugin code passes no context. Tasks a plugin starts with `ctx.spawn { ... }` carry it and are cancelled at unload; the unload waits for them at most until the shutdown deadline it shares with the listeners, so a Task that ignores cancellation does not hold it |
| Concurrency | Every incoming call runs in a Task of its own: provided objects must be safe for concurrent use |
| Cancellation | See [plugin API: cancellation](plugin-api.en.md#cancellation) |
| Values | Data (`Codable` structs, arrays, dictionaries, numbers, strings) is copied, by the SDK's own encoder and decoder; `Date`, `Data`, `Decimal`, `UUID`, `Set`, dictionaries whose keys are not `String`, and `Rutis.Advanced.DateTime`, `Rutis.Advanced.CalendarDate`, `Rutis.Advanced.TimeOfDay`, `Rutis.Advanced.BigInt` cross as their own values (table below); integers beyond ±(2^53−1) (`Int64`, `UInt64`, ...) cross as big integers by themselves; instances of `@RutisService` types and closures cross by reference (see [Objects with methods](#objects-with-methods)); a parameter of type `Any` receives native values, and an object of another process as a `Rutis.Advanced.Client` |
| Errors | A thrown error crosses with its type's name (`QuotaError`), its message (`LocalizedError`'s `errorDescription`, else its description), and the stored properties `code` and `data` when it has them, which the caller reads as its error's `code` and `data`. An error received is a `RutisError` (`name`, `message`, `code`, `data`, `errors`, `cause`), matched by `name` |
| `fatalError`, `try!`, a force unwrap | Not catchable in Swift: the runtime process ends, its rows stop, and the host does what `restart` says ([When a runtime ends](rutis-host.en.md#when-a-runtime-ends)) |

How Swift values cross ([Passing values](plugin-api.en.md#passing-values)):

| Value | Swift |
| --- | --- |
| Point in time | `Date`, truncated to the microsecond |
| Local date and time, date, time of day | `Rutis.Advanced.DateTime`, `Rutis.Advanced.CalendarDate`, `Rutis.Advanced.TimeOfDay` (Swift has no such types) |
| Binary data | `Data` |
| Map | A `Dictionary` whose keys are not `String`; sent with its keys sorted, so one value is always sent the same way |
| Set | `Set`, sent sorted |
| Big integer | `Rutis.Advanced.BigInt`, or an integer type that holds it |
| Decimal | `Decimal`: at most 38 digits, and trailing zeros are not kept (`1.10` arrives as `1.1`); declare the parameter `String` to keep the text as it was sent |
| UUID | `UUID` |

### Handing a plugin to a service

A plugin is itself a value, to hand to a service and mount elsewhere many times: see [plugin API: plugin definitions](plugin-api.en.md#plugin-definitions).

### Objects with methods

Data (`Codable` structs and classes) is copied; an instance of a `@RutisService` type goes to another language by reference, the far end getting a proxy: see [plugin API: objects with methods](plugin-api.en.md#objects-with-methods).

### Events

Every listener on a host, in any language and any process, is on one event bus ([design](../design-events-2026-10-18.en.md)): where an event is emitted and where its listeners are does not change the result. A listener's parameters are the event's arguments, decoded by their types (missing ones `nil`, extra ones ignored).

```swift
let off = ctx.on("session/created") { (id: String) in print("new session \(id)") }

try ctx.emit("session/created", id)   // returns at once; throws only for a refused emission
```

The other ways to emit (`parallel`, `serial`, `waterfall`, `bail`), filters, the rules and a complete example are in [plugin API: events](plugin-api.en.md#swift).

The config is validated against `@RutisConfig`'s schema, its defaults filled, before it is decoded: a row whose config does not match fails with `config: …` ([plugin API: configuration](plugin-api.en.md#configuration)). Optional properties (`T?`) and `var`s with a literal initial value (`var units = "metric"`, also numbers, booleans and arrays and dictionaries of them) are not required; the literal is the default. Any other initial value is no default, and the property is required. A type writing its own `static let rutisSchema` is validated against it; a plain `Decodable` struct without `@RutisConfig` is only decoded (a primitive type such as `String`, or a `CaseIterable` enum with `String` raw values, is still validated against the schema derived from the type). A decoding error after the schema passed is written `<path> is not a value the plugin accepts`.

## 3. Test

`RutisTest` needs no host, but the plugin runs in its real runtime and values cross a real session (an in-memory socket pair); the test side is the host:

```swift
import RutisTest
import Testing
@testable import WeatherPlugin

@RutisService
final class FakeLLM: @unchecked Sendable {
    func ask(_ question: String) -> String { "sunny" }
}

@Test func today() async throws {
    let loaded = try await RutisTest.load(plugin, config: ["city": "Oslo"], services: ["llm": FakeLLM()])
    let today = try await loaded.service("weather").call("today", as: String.self)
    await loaded.close()
    #expect(today == "sunny in Oslo")
}
```

```bash
swift test
```

Events in tests: `loaded.on(name) { ... }` listens as the host does, and `loaded.emit` emits from the row's scope. What works only within one process (sharing a class instance that is not a `@RutisService`, relying on shared memory) fails here, as it would in a host. The rest of the interface is in [plugin API: the Swift test tool](plugin-api.en.md#the-swift-test-tool).

## 4. Run it in a local host

```bash
rutis-host dev
```

`dev` builds the package's executable product with `swift build` and runs each of its plugins as the runtime `swift-weather`; a change to the sources, `Package.swift` or `Package.resolved` rebuilds it and restarts only that runtime, and a failed build keeps the old one running (the compiler's errors are printed). Each build says `weather: building`, then `weather: built in 12.3s`: the first one compiles the dependencies, the SDK's macros among them, and can take minutes. Each build is copied to `.rutis/swift/<product>-<n>`, a new file each time. Other services the plugin needs go in `rutis.dev.json`; they may be Python or TypeScript plugins (with their `runtimes`). When their files change, only those rows reload, without a rebuild, and the Swift runtime does not restart for it; a change to `rutis.dev.json` applies while it runs (see [Developing a plugin with `dev`](rutis-host.en.md#developing-a-plugin-with-dev)). `rutis-host check` lists every row and every Swift binary with its platform, SDK, plugin API and plugins; a binary built with an SDK older than rutis 0.13 cannot run, and `check` says to rebuild it.

### Calling a method

After it starts and after each reload, `dev` prints which services there are and the row that provides each: `rutis-host dev: services: llm (llm), loader (host), weather (weather)`.

While `dev` runs, in another terminal in the project's directory, call a service the plugin provides, to see what it returns and what it says when it fails, without writing a plugin to call it:

```console
$ rutis-host call weather.today
"sunny in Oslo"
```

Each argument is a JSON value (a string with its quotes: `'"Oslo"'`); the result is printed as JSON; an error goes to stderr with exit status 1. See [rutis-host: Calling a service](rutis-host.en.md#calling-a-service).

## 5. System access

macOS asks the user before a program reads calendars, contacts and the like, and remembers the answer for that program. For a Swift plugin, that program is **the plugin binary**: the runtime makes itself responsible for its own requests when it starts (whoever started it: `rutis-host`, a terminal, a process manager), so the prompt names the binary, and what the user grants applies to it alone, not to the host or the terminal. Plugins in one binary share its access; plugins that need different access go in different binaries.

- The prompt shows the binary's name (`CFBundleName` in `Info.plist`) and the usage description of what is asked for: add the key (`NSCalendarsFullAccessUsageDescription`, `NSRemindersFullAccessUsageDescription`, `NSContactsUsageDescription`, `NSAppleEventsUsageDescription`, ...) to `Info.plist`. The template's `Package.swift` links it into the binary.
- A denial is an ordinary failure: check the answer and throw, and the caller, in any language, gets the error:

```swift
import EventKit

struct CalendarAccessDenied: Error, LocalizedError {
    let code = "denied"
    var errorDescription: String? { "calendar access is not granted to this plugin" }
}

@RutisService
public final class Calendar: @unchecked Sendable {
    let store = EKEventStore()

    public func titles(_ days: Int) async throws -> [String] {
        guard try await store.requestFullAccessToEvents() else { throw CalendarAccessDenied() }
        let start = Date()
        let end = start.addingTimeInterval(Double(days) * 86_400)
        let events = store.events(matching: store.predicateForEvents(withStart: start, end: end, calendars: nil))
        return events.map { $0.title ?? "" }
    }
}
```

- Local builds are ad-hoc signed (`swift build` does it): macOS knows the binary by its signature, so every rebuild is a new program to it and may ask again. `tccutil reset Calendar <CFBundleIdentifier>` forgets an answer. A binary signed with a Developer ID keeps its access across versions.

## 6. Publish

Push a tag such as `v0.1.0`: the template's workflow (on a GitHub macOS runner with Xcode) runs the tests and `rutis-host check`, builds a universal binary for arm64 and x86_64 (`swift build -c release --arch arm64 --arch x86_64`), and publishes it with `SHA256SUMS` on a GitHub release. With the secrets it names (a Developer ID certificate and notarization credentials) it signs the binary with the hardened runtime and notarizes it; without them the binary is ad-hoc signed, and a host that downloads it has to trust it.

Whoever wants your plugin in a binary of their own depends on your package and serves it with theirs: `Rutis.serve(WeatherPlugin.plugin, other.plugin)`.

## 7. Used by a host

How a host installs this plugin: [rutis-host: How a host installs plugins](rutis-host.en.md#how-a-host-installs-plugins).

## 8. Run it on another machine

A plugin binary can also run on another Mac as a remote runtime, with `language` set to `swift`. Two ways, with the same arguments, environment variables and rules as the Python and Go runtimes; see [Connect nodes](nodes.en.md#a-language-runtime-on-another-machine):

```bash
# Listen, and rutis connects (certificate and key in RUTIS_CERT, RUTIS_KEY)
RUTIS_TOKEN=secret RUTIS_CERT=server.pem RUTIS_KEY=server.key \
  ./weather listen:wss://0.0.0.0:7443/rutis --id gpu --peer main .

# Dial out to a rutis access point, reconnecting by fixed rules after a disconnect
RUTIS_TOKEN=secret RUTIS_CA=ca.pem \
  ./weather dial:wss://main.example.com:7443/rutis --id worker-1 --peer main .
```

- Listening: a non-loopback address needs `wss://`; the key may be an EC (P-256, P-384, P-521) or RSA key, PKCS#8, SEC1 or PKCS#1, in PEM. One controller at a time, a newer connection taking over; the runtime says `rutis: listening on <address>` once bound.
- Dialing out: it reports its state on stderr (`rutis: connected to …`, `rutis: connection lost (…); retrying in 1s`, `rutis: refused: auth-rejected (…); retrying in 30s`); SIGTERM or Ctrl-C unloads all rows and exits; an incompatible access point makes it exit with status 1 (`rutis: refused: incompatible (…); stopping`). For the rules, see [Runtimes that connect in](nodes.en.md#runtimes-that-connect-in).

To offer the plugins from an iOS, iPadOS or macOS app instead, see [Host plugins in an app](swift-app.en.md).
