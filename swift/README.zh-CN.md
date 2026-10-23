# Swift SDK

[English](README.md)

用 Swift 写 rutis 插件，编成 macOS 插件二进制运行，或在 iOS、iPadOS、macOS 应用里托管（`RutisApp`）。包是仓库根的 `Package.swift`（目标在 `swift/rutis` 下），版本跟发布线的 `v` tag。设计：[Swift SDK](../docs/design-swift-sdk-2026-10-12.md)。指南：[写一个 Swift 插件](../docs/guide/swift-plugin.md)（`rutis-host new <名字> --lang swift` 创建项目）。

```swift
// Package.swift: .package(url: "https://github.com/arcships/rutis", from: "0.15.0")
//                .product(name: "Rutis", package: "rutis")
import Rutis

@RutisConfig
struct Config: Decodable {
    /// 要查天气的城市
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
    provides: ["weather": .of(Weather.self, sync: ["unit"])],   // 可选
    config: Config.self,
    apply: { ctx, config in
        ctx.provide("weather", Weather(llm: try ctx.use("llm", LLMProxy.self), city: config.city))
        ctx.on("session/created") { (session: String) in print("new session \(session)") }
        ctx.effect { print("weather unloaded") }
    }))

// Sources/weather/main.swift
Rutis.serve(weather)
```

- `swift build` 生成二进制；`<二进制> --rutis-manifest` 打印它提供的插件。`rutis-host` 按 `runtimes.swift` 运行它（行名 `swift:<插件>`），Rust 宿主用 rutis-loader 的 `SwiftResolver` / `SwiftRuntimes`。
- 插件代码不传任何上下文参数：调用链随 Task 传递。每个进来的调用一个 Task；同步调用挂起 Task，不占线程。
- 值：SDK 自己的编码器与解码器，由 `Codable` 驱动（不用 `JSONEncoder`）。`Date`、`Data`、`Decimal`、`UUID`、`Set` 和键不是 `String` 的字典按 rutis 的带标记的值跨进程；`Rutis.DateTime`、`Rutis.CalendarDate`、`Rutis.TimeOfDay`、`Rutis.BigInt` 是 Swift 没有的类型。`@RutisService` 类型的实例与闭包按引用。
- 错误带着名字（类型名）、信息、`code` 与 `data`（同名存储属性）跨进程；收到的错误是 `RutisError`，按 `name` 匹配。
- 事件：`ctx.on`、`emit`、`parallel`、`serial`、`waterfall`、`bail`、`filter`（见[插件 API](../docs/guide/plugin-api.md#事件)）。
- 测试：`RutisTest`（产品 `RutisTest`）让插件经真实的内存会话运行，测试一方是它的宿主：

```swift
let loaded = try await RutisTest.load(weather, config: ["city": "Oslo"], services: ["llm": FakeLLM()])
let answer = try await loaded.service("weather").call("today", "rain?", as: String.self)
await loaded.close()
```

平台：macOS 14、iOS 17。应用里用 `RutisApp(plugins:connect:id:token:)` 在开始后提供编进应用的插件（`start()`、`stop()`、可观察的 `state`）：[在应用里托管插件](../docs/guide/swift-app.md)。Swift 5 语言模式，Swift 6.2 工具链及以上（宏使用 swift-syntax 602–603）。SDK 自己的测试：`swift run RutisSelfTest`。
