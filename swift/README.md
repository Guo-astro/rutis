# Swift SDK

[中文](README.zh-CN.md)

Write rutis plugins in Swift and run them as a macOS plugin binary, or host them in an iOS, iPadOS or macOS app (`RutisApp`). The package is the repository root's `Package.swift` (targets under `swift/rutis`); it follows the release train's `v` tags. Design: [Swift SDK](../docs/design-swift-sdk-2026-10-12.en.md). The guide: [Write a Swift plugin](../docs/guide/swift-plugin.en.md) (`rutis-host new <name> --lang swift` makes a project).

```swift
// Package.swift: .package(url: "https://github.com/arcships/rutis", from: "0.16.0")
//                .product(name: "Rutis", package: "rutis")
import Rutis

@RutisConfig
struct Config: Decodable {
    /// the city to report on
    var city: String
}

@RutisClient
protocol LLM {
    func ask(_ question: String) async throws -> String
}

@RutisService
final class Weather {
    let llm: any LLM
    let city: String
    init(llm: any LLM, city: String) { self.llm = llm; self.city = city }

    func today(_ question: String) async throws -> String { try await llm.ask("weather in \(city): \(question)") }
    func unit() -> String { "celsius" }
}

let weather = Rutis.define(Plugin(
    name: "weather",
    inject: ["llm"],
    provides: ["weather": .of(Weather.self, sync: ["unit"])],   // optional
    config: Config.self,
    apply: { ctx, config in
        ctx.provide("weather", Weather(llm: try ctx.use("llm", LLMProxy.self), city: config.city))
        ctx.on("session/created") { (session: String) in print("new session \(session)") }
        ctx.effect { print("weather unloaded") }
    }))

// Sources/weather/main.swift
Rutis.serve(weather)
```

- `swift build` makes the binary; `<binary> --rutis-manifest` prints what it serves. `rutis-host` runs it from `runtimes.swift` (rows `swift:<plugin>`), Rust hosts with rutis-loader's `SwiftResolver` / `SwiftRuntimes`.
- Plugin code takes no context parameter: the call chain travels with the Task. Each incoming call runs in a Task of its own; a synchronous call suspends the Task, never a thread.
- Values: the SDK's own encoder and decoder, driven by `Codable` (never `JSONEncoder`). `Date`, `Data`, `Decimal`, `UUID`, `Set` and non-`String`-keyed dictionaries cross as rutis's tagged values; `Rutis.DateTime`, `Rutis.CalendarDate`, `Rutis.TimeOfDay` and `Rutis.BigInt` are the types Swift lacks. Instances of `@RutisService` types and closures cross by reference.
- Errors cross with their name (the type's), message, `code` and `data` (stored properties of those names); a received error is a `RutisError`, matched by `name`.
- Events: `ctx.on`, `emit`, `parallel`, `serial`, `waterfall`, `bail`, `filter` (the [plugin API](../docs/guide/plugin-api.en.md#events)).
- Tests: `RutisTest` (product `RutisTest`) runs a plugin across a real in-memory session, the test being its host:

```swift
let loaded = try await RutisTest.load(weather, config: ["city": "Oslo"], services: ["llm": FakeLLM()])
let answer = try await loaded.service("weather").call("today", "rain?", as: String.self)
await loaded.close()
```

Platforms: macOS 14 and iOS 17. In an app, `RutisApp(plugins:connect:id:token:)` serves the plugins compiled into it while started (`start()`, `stop()`, an observable `state`): [Host plugins in an app](../docs/guide/swift-app.en.md). Swift 5 language mode, Swift 6.2 toolchain or later (the macros use swift-syntax 602–603). The SDK's own tests: `swift run RutisSelfTest`.
