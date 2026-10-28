# 写一个 Swift 插件

[English](swift-plugin.en.md)

从创建项目到发布，再到被宿主使用。需要 macOS 14 或更高，以及带 Swift 6.2 工具链的 Xcode 26 或更高（只有 Command Line Tools 也能构建和运行插件，但 `swift test` 要用 Xcode 的测试框架）；SDK 本身按 Swift 5 语言模式编写，`new` 生成的项目也是。

Swift 插件和 TypeScript、Python、Go 插件的写法与规则相同（见 [插件 API](plugin-api.md)）；和 Go 一样，**Swift 插件编译成二进制，一个二进制就是宿主里的一个运行时进程**。一个二进制可以带一个或几个插件；宿主可以同时运行很多个二进制，只在有行用到时才启动它。Swift 用来接入 macOS 给本机程序的能力：日历与提醒（EventKit）、通讯录、辅助功能与 Apple 事件自动化、屏幕录制、Core ML。

## 1. 创建项目

```bash
rutis-host new weather --lang swift
cd weather
swift test
```

得到的项目（一个 SwiftPM 包）：

| 文件 | 作用 |
| --- | --- |
| `Sources/WeatherPlugin/Plugin.swift` | 插件本身（库 `WeatherPlugin`，导出 `plugin`） |
| `Sources/weather/main.swift` | 二进制：`Rutis.serve(plugin)` |
| `Tests/WeatherPluginTests/PluginTests.swift` | 用 `RutisTest` 的单元测试，不需要宿主 |
| `Info.plist` | 链进二进制：它的名字，以及插件请求权限时 macOS 显示的用途说明（见 [系统权限](#5-系统权限)） |
| `Package.swift` | 依赖 `https://github.com/arcships/rutis`（产品 `Rutis` 与 `RutisTest`）；`linkerSettings` 链入 `Info.plist` |
| `rutis.dev.json` | 本地运行时的配置和测试用的其他插件 |
| `.github/workflows/release.yml` | 打 `v*` tag 时测试，并把通用二进制（arm64 + x86_64）发布到 GitHub Release |

第一次构建会编译 SDK 的宏（swift-syntax，只在编译期）；Xcode 会询问一次是否信任它们（CI 里用 `xcodebuild` 时加 `-skipMacroValidation`）。

## 2. 写插件

```swift
import Rutis

/// 配置：普通的 Decodable 类型；@RutisConfig 由它生成 JSON Schema。
@RutisConfig
public struct Config: Decodable {
    /// 要查天气的城市
    public var city: String
}

/// 它用到的服务，可以是任何语言的：@RutisClient 生成 `LLMProxy`。
@RutisClient
public protocol LLM {
    func ask(_ question: String) async throws -> String
}

/// 它提供的服务：@RutisService 让它的方法成为服务的方法。
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
    inject: ["llm"],                                            // 全部就绪时才运行
    provides: ["weather": .of(Weather.self, sync: ["unit"])],   // 声明提供什么（check 运行前就知道）；unit 标为同步，其余异步
    config: Config.self,
    apply: { ctx, config in
        ctx.provide("weather", Weather(llm: try ctx.use("llm", LLMProxy.self), city: config.city))
        ctx.effect { /* 清理，可选 */ }
    }
))
```

要点：

| 主题 | 规则 |
| --- | --- |
| 方法 | `@RutisService` 让每个不是 `private` / `fileprivate` 的实例方法成为服务的方法，线上名是基本名（`today`），参数按位置。重载、泛型方法、不是 `async throws` 的函数参数在编译期报错。看不到父类和协议扩展里的方法：在类型里声明 |
| 同步还是异步 | 默认 `async`，与 Swift 签名无关；`provides: [... .of(T.self, sync: ["unit"])]` 标出同步方法。`sync` 方法自己也可以是 `async`、调用别的服务：Task 等待时不占线程。同步方法会阻塞 Node 调用方的事件循环：做 I/O 的方法保持 `async` |
| 使用服务 | 带 `@RutisClient` 的协议（方法须 `async throws`）；`ctx.use("llm", LLMProxy.self)` 返回 `any LLM`，提供者在同一进程且实现了这个协议时就是它本身。也可以按名字调用：`try await ctx.use("llm").call("ask", values: ["q"], as: String.self)` |
| 调用链 | 随 Task 传递（`@TaskLocal`）：插件代码不传任何上下文。插件用 `ctx.spawn { ... }` 起的 Task 带着它，卸载时被取消；卸载至多等它们到关闭时限（和监听器共用），不响应取消的 Task 不会拖住卸载 |
| 并发 | 每个进来的调用在自己的 Task 里运行：提供的对象要能并发使用 |
| 取消 | 调用方放弃时取消方法的 Task（`Task.checkCancellation()` 抛 `CancellationError`）；在已取消的 Task 里发出的调用抛 `CancellationError`；取消后仍要调用（比如报告自己已停下）时在 `Task { … }` 里调用，它带着调用链、不带取消 |
| 值 | 数据（`Codable` 结构体、数组、字典、数字、字符串）按值复制，用 SDK 自己的编码器与解码器；`Date`、`Data`、`Decimal`、`UUID`、`Set`、键不是 `String` 的字典，以及 `Rutis.DateTime`、`Rutis.CalendarDate`、`Rutis.TimeOfDay`、`Rutis.BigInt` 按各自的值传递（见下表）；超出 ±(2^53−1) 的整数（`Int64`、`UInt64` 等）自动按大整数传递；`@RutisService` 类型的实例和闭包按引用传递（见[带方法的对象](#带方法的对象)）；类型为 `Any` 的参数收到原生值，别的进程的对象是 `Rutis.Client` |
| 错误 | 抛出的错误带着类型名（`QuotaError`）、消息（`LocalizedError` 的 `errorDescription`，否则它的描述），以及存储属性 `code`、`data`（有的话）跨过去，调用方读到的错误带同样的 `code`、`data`。收到的错误是 `RutisError`（`name`、`message`、`code`、`data`、`errors`、`cause`），按 `name` 匹配 |
| `fatalError`、`try!`、强制解包 | Swift 里不可捕获：运行时进程结束、它的行停止，宿主按 `restart` 处理（[运行时结束时](rutis-host.md#运行时结束时)） |

Swift 的值怎样跨过去（[传递值](plugin-api.md#传递值)）：

| 值 | Swift |
| --- | --- |
| 时间点 | `Date`，截断到微秒 |
| 本地日期时间、日期、一天中的时间 | `Rutis.DateTime`、`Rutis.CalendarDate`、`Rutis.TimeOfDay`（Swift 没有这些类型） |
| 二进制数据 | `Data` |
| 映射 | 键不是 `String` 的 `Dictionary`；发出时按键排序，同一个值总是同样发出 |
| 集合 | `Set`，发出时排序 |
| 大整数 | `Rutis.BigInt`，或装得下它的整数类型 |
| 十进制数 | `Decimal`：至多 38 位，不保留末尾的零（`1.10` 收到是 `1.1`）；要原样保留文本时把参数声明为 `String` |
| UUID | `UUID` |

### 把插件交给服务

插件本身也是一个值：`Rutis.define(...)` 的结果可以照常作为参数交给一个服务（比如应用的部件登记处），收的一方在别处挂它，每挂一次是一份，用的是你在 `apply` 里准备好的东西。`@RutisClient` 的参数或返回值类型写 `Rutis.Definition`：

```swift
@RutisClient
protocol Parts {
    func register(_ scope: String, _ part: Rutis.Definition) async throws
}

let git = Rutis.define(Plugin(name: "git", inject: ["parts"]) { ctx in
    let pool = try await openPool()
    let status = Rutis.define(Plugin(name: "git_status", inject: ["history"], config: StatusConfig.self) { ctx, config in
        ctx.provide("gitStatus", GitStatus(pool: pool, depth: config.depth))
    })
    try await ctx.use("parts", PartsProxy.self).register("session", status)
})
```

- 每一份在这个插件的运行时里运行，挂在哪个实例里就按那个实例隔离，按它的 `inject` 门控。
- 它属于交出它的那一行的这一代：这一行卸载、重载或结束时，先结束从它挂出的每一份，再卸载这一行。
- 交给 `RutisTest` 的测试替身时，替身收到的就是这个定义本身，可以直接 `RutisTest.load(definition, config: ...)`，和宿主挂它一样；替身把它交回插件时，插件拿到的也是原来那个。
- 参数或返回值要直接是 `Rutis.Definition`（或 `Rutis.Definition?`）。放在数组、字典或结构体里的插件定义（比如 `[Rutis.Definition]`）不支持，要交几个就调用几次。
- 规则见 [插件 API](plugin-api.md#插件定义)。

### 带方法的对象

数据（`Codable` 结构体和类）复制；`@RutisService` 类型的实例按引用交给别的语言：对方拿到代理，调用的是这里的原对象。别的进程交来的对象，参数类型写成 `@RutisClient` 协议，或者写 `Any`：

```swift
/// 怎样使用别的进程交来的对象：写在文件的顶层。
@RutisClient
protocol Cache {
    func get(_ key: String) async throws -> Int?
    func set(_ key: String, _ value: Int) async throws
}

@RutisService
final class Warmer: @unchecked Sendable {
    func warm(_ cache: any Cache) async throws -> Int? {
        try await cache.set("a", 1)          // 方法都是 async，调用的是对方的原对象
        return try await cache.get("a")
    }

    /// 不知道是什么时写 Any：对象是 Rutis.Client，按名字调用。
    func peek(_ value: Any) async throws -> Int? {
        guard let cache = value as? Rutis.Client else { return nil }
        return try await cache.call("get", values: ["a"], as: Int?.self)
    }
}
```

- 参数类型写 `@RutisClient` 协议（`any Cache`）时拿到这个协议的代理，和 `ctx.use("cache", CacheProxy.self)` 是同一种代理；`@RutisClient` 方法的返回值也一样。参数写 `Any` 时拿到 `Rutis.Client`：`try await client.call("get", [Rutis.encode("a")], as: Int?.self)` 或 `call("get", values: ["a"], as: Int?.self)`。数组、字典里的对象和事件参数里的对象，经 `Any` 拿到 `Rutis.Client`；`[any Cache]` 在 0.18 不支持。
- 只有写在文件顶层的 `@RutisClient` 协议能收对象：宏为它生成的 `rutisObjectProxy` 重载只在文件顶层对 Swift 可见。嵌在别的类型里的 `@RutisClient` 协议照常可以 `ctx.use`，但用它做参数类型时收不了对象（`bad-argument`）。可选类型（`(any Cache)?`）的参数和返回值在 0.18 里也收不了对象，写 `any Cache`。
- 参数类型是数据（`Codable` 类型、`String` 等）时，这次调用以 `bad-argument` 失败：`warmer.place: argument 1 should be a Spot, got an object (an object is received only as a @RutisClient protocol or Any)`。
- 代理上没有数据字段：要数据，让方法返回它。调用对象没有的方法，这次调用以 `no-method` 失败：`Cache has no method gett (methods: get, set, size)`。
- 同一个对象交出几次，这里拿到同一个代理；把收到的代理或 `Rutis.Client` 再交出去，发出的是原来那个对象，交回原来的进程，那边拿到的是原对象。
- 对象不跟行走：交出它的行卸载后照常可以调用。按需启动的 Swift 运行时在最后一行卸载 `idle` 秒后停下，别处持有它的对象不让它保持运行；之后的调用以 `peer-gone` 失败：`a call on an object from swift-plugin-binary failed: runtime swift-plugin-binary stopped because it was idle`。要一直可用，让交出它的行保持加载，或者把运行时设成 `eager`。对方的运行时退出后，调用同样以 `peer-gone` 失败。
- 对象不能交给 0.18 之前的一端：直接连着这样的一端时，这次调用以 `bad-value` 失败（`… cannot cross to main; pass plain data or a function instead, or upgrade main to rutis 0.18`），会话继续。
- 同一个运行时里的行拿到的是对象本身。`RutisTest` 按跨进程的样子给代理。规则见 [插件 API](plugin-api.md#带方法的对象)。

### 事件

一个宿主上所有语言、所有进程的监听器都在同一条事件总线上（[设计](../design-events-2026-10-18.md)）：事件在哪里发出、监听器在哪里，结果都一样。监听器的参数就是事件的参数，按声明的类型解码（缺的为 `nil`，多的忽略）；waterfall 里最后一个参数是 `Rutis.Next<R>`。

```swift
let off = ctx.on("session/created") { (id: String) in print("new session \(id)") }   // 选项：prepend:、global:

try ctx.emit("session/created", id)                                   // 立即返回；只在发出被拒时抛出
try await ctx.parallel("session/created", id)                         // 等全部；失败是名为 AggregateError 的 RutisError
let value = try await ctx.serial("config/resolve", key, as: String.self)   // 第一个结果（nil 和 false 以外的）
let first = try await ctx.bail("config/resolve", key, as: String.self)     // 以同步调用进行的 serial
let text = try await ctx.waterfall("chat/reply", message, as: String.self) { (message: String) in message }   // terminal：链的末端
let some = try await ctx.filter { $0.row == "chat" }.serial("config/resolve", key, as: String.self)
```

一个完整的例子：插件回答 `greeting/pick`，并在 `greeting/format` 这条 waterfall 上加一环；它提供的服务用 `serial` 取第一个结果，用 `waterfall` 让别的插件（任何语言）改写问候语，用 `emit` 发通知：

```swift
import Rutis

@RutisService
public final class Greeter: @unchecked Sendable {
    let ctx: Rutis.Ctx
    init(ctx: Rutis.Ctx) { self.ctx = ctx }

    public func hello(_ name: String, _ lang: String) async throws -> String {
        let word = try await ctx.serial("greeting/pick", lang, as: String.self) ?? "Hello"
        let text = try await ctx.waterfall("greeting/format", "\(word), \(name)", as: String.self) { (text: String) in text }
        try ctx.emit("greeting/said", name)
        return text ?? ""
    }
}

public let greeter = Rutis.define(Plugin(name: "greeter") { ctx in
    ctx.on("greeting/pick") { (lang: String) -> String? in lang == "sv" ? "Hej" : nil }   // nil：交给下一个监听器
    ctx.on("greeting/format") { (text: String, next: Rutis.Next<String>) in
        (try await next() ?? text) + "!"                                                // next 运行链的其余部分
    }
    ctx.provide("greeter", Greeter(ctx: ctx))
})
```

规则（waterfall、失败、作用域、过滤、取消、保留名）在各语言相同：见 [插件 API：事件](plugin-api.md#事件) 和 [Go 指南的表](go-plugin.md#事件)。在 Swift 里，监听器的 Task 在它的行卸载时被取消，`parallel`、`serial`、`waterfall`、`bail` 的发出方放弃时也被取消；监听器一律按同步监听器调用，所以 `bail` 在任何地方都能到达 Swift 监听器。

配置先按 `@RutisConfig` 的 Schema 补默认值、校验，再解码：配置不合格的行以 `config: …` 失败（[插件 API：配置](plugin-api.md#配置)）。可选属性（`T?`）和带字面量初值的 `var`（`var units = "metric"`，还有数字、布尔值以及它们的数组和字典）不必填，字面量就是默认值；其他初值不算默认值，这个属性仍必填。自己写 `static let rutisSchema` 的类型按它校验；没有 `@RutisConfig` 的普通 `Decodable` 结构体只解码（`String` 这类基本类型、原始值为 `String` 的 `CaseIterable` 枚举仍按由类型得出的 Schema 校验）。Schema 通过后解码仍出错时写成 `<路径> is not a value the plugin accepts`。

## 3. 测试

`RutisTest` 不需要宿主，但插件在它真实的运行时里运行，值经真实的会话（内存里的一对 socket）传递；测试一方就是宿主：

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

测试里的事件：`loaded.emit`、`parallel`、`serial`、`waterfall`、`bail` 从这一行的作用域发出；`loaded.on(name) { ... }` 像宿主一样监听；`loaded.load(config:row:scope:)` 再装载一行（`RutisTest.scope("t1/s1")`）；`loaded.update(config:)` 像 rutis 一样改一行的配置；`loaded.reported` 是运行时报告的 `emit` 监听器失败。只在同一进程里才成立的写法（共享一个不是 `@RutisService` 的 class 实例、依赖共享内存）在这里失败，在宿主里也会失败。

## 4. 在本地宿主里运行

```bash
rutis-host dev
```

`dev` 用 `swift build` 构建包的可执行产品，把它的每个插件作为运行时 `swift-weather` 运行；源码、`Package.swift` 或 `Package.resolved` 变化时重新构建，只重启这个运行时，构建失败时旧的继续运行（打印编译器的错误）。每次构建先打印 `weather: building`，完成后打印 `weather: built in 12.3s`：第一次要编译依赖（包括 SDK 的宏），可能要几分钟。每次构建复制到 `.rutis/swift/<产品>-<序号>`，每次一个新文件。插件需要的其他服务写在 `rutis.dev.json` 里；它们可以是 Python 或 TypeScript 插件（带上它们的 `runtimes`）；它们的文件变化时只重新加载这些行，不重新构建，Swift 运行时也不为此重启；`rutis.dev.json` 改了也在运行中生效（见 [用 dev 开发插件](rutis-host.md#用-dev-开发插件)）。`rutis-host check` 列出每一行和每个 Swift 二进制，带平台、SDK、插件 API 和插件；用 rutis 0.13 以前的 SDK 构建的二进制不能运行，`check` 提示重新构建。

### 调用一个方法

`dev` 启动后和每次重新加载后，打印现在有哪些服务、各由哪一行提供：`rutis-host dev: services: llm (llm), loader (host), weather (weather)`。

`dev` 运行时，另开一个终端，在项目目录里调用插件提供的服务，看它返回什么、出错时报什么，不用另写一个调用它的插件：

```console
$ rutis-host call weather.today
"sunny in Oslo"
```

每个参数是一个 JSON 值（字符串带引号：`'"Oslo"'`），结果打印成 JSON；错误写到标准错误并以 1 退出。见 [rutis-host：调用一个服务](rutis-host.md#调用一个服务)。

## 5. 系统权限

程序读日历、通讯录等之前，macOS 会询问用户，并为这个程序记住答复。对 Swift 插件，这个程序是**插件二进制本身**：运行时启动时让自己为自己的请求负责（不论是谁启动它：`rutis-host`、终端、进程管理器），所以提示里是这个二进制的名字，用户的授权只给它，不给宿主或终端。同一个二进制里的插件共享它的授权；需要不同授权的插件放进不同的二进制。

- 提示显示二进制的名字（`Info.plist` 的 `CFBundleName`）和所请求权限的用途说明：把对应的键（`NSCalendarsFullAccessUsageDescription`、`NSRemindersFullAccessUsageDescription`、`NSContactsUsageDescription`、`NSAppleEventsUsageDescription` 等）加进 `Info.plist`。模板的 `Package.swift` 把它链进二进制。
- 被拒绝是普通的失败：检查答复并抛出，任何语言的调用方都会收到这个错误：

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

- 本地构建是 ad-hoc 签名的（`swift build` 自动签）：macOS 按签名认程序，所以每次重新构建对它都是一个新程序，可能再问一次。`tccutil reset Calendar <CFBundleIdentifier>` 清除一次答复。用 Developer ID 签名的二进制在版本之间保留授权。

## 6. 发布

推一个 tag，比如 `v0.1.0`：模板的工作流（在带 Xcode 的 GitHub macOS runner 上）运行测试和 `rutis-host check`，构建 arm64 与 x86_64 的通用二进制（`swift build -c release --arch arm64 --arch x86_64`），连同 `SHA256SUMS` 发布到 GitHub Release。配好它列出的 secrets（Developer ID 证书和公证凭据）时，用 hardened runtime 签名并公证；没有时二进制是 ad-hoc 签名，下载它的宿主要先信任它。

想把你的插件放进自己二进制的人依赖你的包，和自己的插件一起 serve：`Rutis.serve(WeatherPlugin.plugin, other.plugin)`。

## 7. 被宿主使用

宿主在 `rutis.json` 里指定 Swift 插件目录，把二进制放进去：

```json
{
  "runtimes": { "swift": { "dir": "plugins/swift", "restart": "on-failure" } },
  "rows": [{ "id": "weather", "name": "swift:weather", "config": { "city": "Oslo" } }]
}
```

从 Release 下载二进制放进这个目录（下载的二进制带隔离属性：`rutis-host check` 会提示，信任它之后用 `xattr -d com.apple.quarantine <文件>` 清除）。宿主在所有二进制里找 `swift:<插件>`；两个二进制有同名插件时写 `swift-<二进制名>:<插件>`。设置与 `runtimes.go` 相同：二进制在第一次有行用到时启动，空闲 `idle` 秒（60）后停下，`"start": "eager"` 全部启动并一直运行，`restart`（默认 `no`、`on-failure`、`exit`）对每个运行时各自生效，换了二进制在它的运行时重启后生效。运行 Swift 二进制不需要 Swift 工具链。见 [rutis-host 与 rutis.json](rutis-host.md)；Rust 宿主见 [在 Rust 应用里嵌入](rust-host.md)（rutis-loader 的 `BinaryResolver<Swift>`、`BinaryRuntimes<Swift>`）。

## 8. 在另一台机器上运行

插件二进制也可以在另一台 Mac 上作为远程运行时运行，`language` 写 `swift`。两种方式，参数、环境变量和规则与 Python、Go 运行时相同；见 [连接节点](nodes.md#别的机器上的语言运行时)：

```bash
# 监听，由 rutis 连过来（证书和私钥在 RUTIS_CERT、RUTIS_KEY）
RUTIS_TOKEN=secret RUTIS_CERT=server.pem RUTIS_KEY=server.key \
  ./weather listen:wss://0.0.0.0:7443/rutis --id gpu --peer main .

# 拨出到 rutis 的接入点，断开后按固定规则重连
RUTIS_TOKEN=secret RUTIS_CA=ca.pem \
  ./weather dial:wss://main.example.com:7443/rutis --id worker-1 --peer main .
```

- 监听：非回环地址必须用 `wss://`；私钥可以是 EC（P-256、P-384、P-521）或 RSA，PKCS#8、SEC1 或 PKCS#1，PEM 格式。一次一个控制方，新连接接管；绑定后打印 `rutis: listening on <地址>`。
- 拨出：在 stderr 报告状态（`rutis: connected to …`、`rutis: connection lost (…); retrying in 1s`、`rutis: refused: auth-rejected (…); retrying in 30s`）；SIGTERM 或 Ctrl-C 卸载全部行后退出；接入点不兼容时以状态 1 退出（`rutis: refused: incompatible (…); stopping`）。规则见 [连进来的运行时](nodes.md#连进来的运行时)。

要从 iOS、iPadOS 或 macOS 应用里提供插件，见 [在应用里托管插件](swift-app.md)。
