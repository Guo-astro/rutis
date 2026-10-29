# 写一个 Swift 插件

[English](swift-plugin.en.md)

从创建项目到发布。需要 macOS 14 或更高、带 Swift 6.2 工具链的 Xcode 26 或更高（`swift test` 要用 Xcode 的测试框架），以及 Node.js 22 或更高（运行 `npx` 和第 7 步的假服务）。Swift 用来接入 macOS 给本机程序的能力：日历与提醒（EventKit）、通讯录、辅助功能与 Apple 事件自动化、屏幕录制、Core ML。

## 1. 起项目

<!-- example: swift run -->
```bash
npx @arcships/rutis-host new greeter --lang swift
cd greeter
swift test
```

Swift 插件编译成二进制，一个二进制就是宿主里的一个运行时进程。得到的项目是一个 SwiftPM 包：

| 文件 | 作用 |
| --- | --- |
| `Sources/GreeterPlugin/Plugin.swift` | 插件本身（库 `GreeterPlugin`，导出 `plugin`） |
| `Sources/greeter/main.swift` | 二进制：`Rutis.serve(plugin)` |
| `Tests/GreeterPluginTests/PluginTests.swift` | 测试，不需要宿主 |
| `rutis.dev.json` | 本地运行时的行（第 7 步） |
| `Info.plist` | 链进二进制，macOS 请求权限时用（见 [系统权限](#系统权限)） |
| `Package.swift` | 依赖 `https://github.com/arcships/rutis`（产品 `Rutis` 与 `RutisTest`） |
| `.github/workflows/release.yml` | 打 `v*` tag 时测试并发布二进制（第 8 步） |

第一次构建会编译 SDK 的宏（swift-syntax，只在编译期），要一两分钟；Xcode 会询问一次是否信任它们。

## 2. 提供服务

生成的 `Sources/GreeterPlugin/Plugin.swift` 是一个插件，它提供服务 `greeter`：

<!-- example: swift file=greeter/Sources/GreeterPlugin/Plugin.swift -->
```swift
// The greeter rutis plugin. The Swift plugin guide goes on from here:
// https://github.com/arcships/rutis/blob/main/docs/guide/swift-plugin.en.md
import Rutis

/// The plugin's configuration (guide, step 4): `@RutisConfig` derives its
/// JSON Schema; a `var` with a literal initial value is its default, filled
/// in when the row leaves it out.
@RutisConfig
public struct Config: Decodable {
    /// how to greet
    public var greeting = "Hello"
}

/// The service the plugin provides (guide, step 2): `@RutisService` makes
/// its methods the service's.
@RutisService
public final class Greeter: @unchecked Sendable {
    let greeting: String

    init(greeting: String) { self.greeting = greeting }

    public func hello(_ name: String) -> String { "\(greeting), \(name)!" }
}

public let plugin = Rutis.define(Plugin(
    name: "greeter",
    // The services it uses (guide, step 3): it starts once they are all there.
    inject: [],
    // What it provides, so that `rutis-host check` knows before it runs.
    provides: ["greeter": .of(Greeter.self)],
    config: Config.self,
    apply: { ctx, config in
        ctx.provide("greeter", Greeter(greeting: config.greeting))
    }
))
```

- `Rutis.define(Plugin(...))` 定义插件；`name` 是它在二进制里的名字。宿主装上插件、启动它时调用 `apply`。
- `@RutisService` 让类的每个公开方法成为服务的方法（`hello`），`ctx.provide("greeter", …)` 提供这个服务：任何语言的插件都能按名字 `greeter` 调用它，插件卸载时服务随之撤销。
- `provides` 写出插件提供哪些服务，宿主不运行插件就知道。

## 3. 使用服务

`greeter` 改成先把问候语交给服务 `translate` 翻译。`translate` 由别的插件提供，用哪种语言写都可以：

<!-- example: swift file=greeter/Sources/GreeterPlugin/Plugin.swift -->
```swift
import Rutis

@RutisConfig
public struct Config: Decodable {
    /// how to greet
    public var greeting = "Hello"
}

@RutisClient
public protocol Translate {
    func text(_ text: String) async throws -> String
}

@RutisService
public final class Greeter: @unchecked Sendable {
    let translate: any Translate
    let greeting: String

    init(translate: any Translate, greeting: String) {
        self.translate = translate
        self.greeting = greeting
    }

    public func hello(_ name: String) async throws -> String {
        "\(try await translate.text(greeting)), \(name)!"
    }
}

public let plugin = Rutis.define(Plugin(
    name: "greeter",
    inject: ["translate"],
    provides: ["greeter": .of(Greeter.self)],
    config: Config.self,
    apply: { ctx, config in
        let translate = try ctx.use("translate", TranslateProxy.self)
        ctx.provide("greeter", Greeter(translate: translate, greeting: config.greeting))
    }
))
```

- `inject` 列出插件用到的服务。这些服务都就绪了插件才启动；其中一个撤销，插件就停下，它回来时插件再启动。
- `@RutisClient` 从协议生成 `TranslateProxy`；`ctx.use("translate", TranslateProxy.self)` 返回 `any Translate`，调用它的方法就是调用这个服务。用到的服务的方法写成 `async throws`。

现在模板自带的测试会失败（它没有给 `translate`），第 6 步换掉它。

## 4. 配置

`Config` 是插件的配置：`@RutisConfig` 由它生成 JSON Schema，`var greeting = "Hello"` 的字面量就是默认值。`apply` 之前，rutis 给配置补上默认值并校验，`apply` 收到的 `config.greeting` 一定有值；配置不合格的行以 `config: …` 失败，不调用 `apply`。

配置从哪里来：测试里由 `RutisTest.load(plugin, config: ["greeting": "Hi"])` 传入（第 6 步）；运行时来自宿主里这个插件那一行的 `config`（第 7 步的 `rutis.dev.json`）。不写 `greeting` 时就是 `Hello`。

## 5. 发出和监听事件

`hello` 每次问候后发出事件 `greeter/greeted`；同一个插件监听它，数一共问候了几次，用新方法 `count` 读出：

<!-- example: swift file=greeter/Sources/GreeterPlugin/Plugin.swift -->
```swift
import Rutis

@RutisConfig
public struct Config: Decodable {
    /// how to greet
    public var greeting = "Hello"
}

@RutisClient
public protocol Translate {
    func text(_ text: String) async throws -> String
}

actor Greeted {
    var count = 0
    func add() { count += 1 }
}

@RutisService
public final class Greeter: @unchecked Sendable {
    let ctx: Rutis.Ctx
    let translate: any Translate
    let greeting: String
    let greeted: Greeted

    init(ctx: Rutis.Ctx, translate: any Translate, greeting: String, greeted: Greeted) {
        self.ctx = ctx
        self.translate = translate
        self.greeting = greeting
        self.greeted = greeted
    }

    public func hello(_ name: String) async throws -> String {
        let text = "\(try await translate.text(greeting)), \(name)!"
        try ctx.emit("greeter/greeted", name)
        return text
    }

    public func count() async -> Int { await greeted.count }
}

public let plugin = Rutis.define(Plugin(
    name: "greeter",
    inject: ["translate"],
    provides: ["greeter": .of(Greeter.self)],
    config: Config.self,
    apply: { ctx, config in
        let greeted = Greeted()
        ctx.on("greeter/greeted") { (_: String) in await greeted.add() }
        let translate = try ctx.use("translate", TranslateProxy.self)
        ctx.provide("greeter", Greeter(ctx: ctx, translate: translate, greeting: config.greeting, greeted: greeted))
    }
))
```

- `ctx.emit(name, 参数…)` 发出事件，立即返回，不等监听器。
- `ctx.on(name) { … }` 监听事件，闭包的参数就是事件的参数，按声明的类型解码；插件卸载时监听随之撤销。
- 监听器在哪个进程、用哪种语言写都一样收到：别的插件也可以监听 `greeter/greeted`。

其余发出方式（等全部监听器、取第一个结果、串成一条链）见 [插件 API：事件](plugin-api.md#事件)。

## 6. 测试

`RutisTest` 不需要宿主：插件在真实的运行时里运行，测试一方就是宿主，给插件假的服务、调用它提供的服务、像宿主一样监听事件。`Tests/GreeterPluginTests/PluginTests.swift` 换成：

<!-- example: swift file=greeter/Tests/GreeterPluginTests/PluginTests.swift -->
```swift
import Rutis
import RutisTest
import Testing

@testable import GreeterPlugin

@RutisService
final class FakeTranslate: @unchecked Sendable {
    func text(_ text: String) -> String { ["Hello": "Bonjour", "Hi": "Salut"][text] ?? text }
}

@Test func greeterGreetsInTranslation() async throws {
    let loaded = try await RutisTest.load(plugin, config: ["greeting": "Hi"], services: ["translate": FakeTranslate()])
    let (greeted, received) = AsyncStream.makeStream(of: String.self)
    let off = await loaded.on("greeter/greeted") { (name: String) in _ = received.yield(name) }
    let greeter = try await loaded.service("greeter")
    #expect(try await greeter.call("hello", "Ada", as: String.self) == "Salut, Ada!")
    var names = greeted.makeAsyncIterator()
    #expect(await names.next() == "Ada")
    #expect(try await greeter.call("count", as: Int.self) == 1)
    await off()
    await loaded.close()
}
```

<!-- example: swift run -->
```bash
swift test
```

- `RutisTest.load(plugin, config:, services:)` 装载插件：`config` 是这一行的配置，`services` 是假的服务（`@RutisService` 类型的实例），插件 `inject` 的服务都要给。
- `loaded.service("greeter").call("hello", "Ada", as: String.self)` 像别的插件一样调用服务。
- `loaded.on(name) { … }` 像宿主一样监听事件；事件是另一边发出的，所以测试等它到达（这里用 `AsyncStream`），不在调用返回时直接检查。
- `loaded.close()` 卸载插件、结束测试的宿主。

值在测试里经真实的会话传递，和在宿主里一样：只在同一进程里才成立的写法在这里就失败。测试工具的其余接口见 [插件 API：Swift 的测试工具](plugin-api.md#swift-的测试工具)。

## 7. 本地运行并试一下

`rutis-host dev` 在本地宿主里运行插件，它运行的是 `rutis.dev.json` 里的**行**：每一行是宿主里运行的一个插件。插件要用 `translate`，所以要有一行提供它。Swift 没有本语言的假服务，这里用 TypeScript 写一个，放在 `dev/`：

<!-- example: swift file=greeter/dev/fake-translate.ts -->
```ts
import { definePlugin } from '@arcships/rutis'

const words: Record<string, string> = { Hello: 'Bonjour', Hi: 'Salut' }

export default definePlugin({
  provides: { translate: { text: 'async' } },
  apply(ctx) {
    ctx.provide('translate', { text: async (text: string) => words[text] ?? text })
  },
})
```

<!-- example: swift run -->
```bash
npm install --prefix dev @arcships/rutis
```

`rutis.dev.json` 换成两行：

<!-- example: swift file=greeter/rutis.dev.json -->
```json
{
  "format": 1,
  "runtimes": { "node": { "project": "dev" } },
  "rows": [
    { "id": "greeter", "config": { "greeting": "Hello" } },
    { "id": "translate", "name": "./dev/fake-translate.ts" }
  ]
}
```

- 和插件同 id 的那一行（`greeter`）就是插件自己，不写 `name`，`config` 是它的配置。
- 另一行 `translate` 运行 `dev/fake-translate.ts`，它提供 `translate`；`runtimes.node` 让宿主用 `dev/` 里装的包运行它。

<!-- example: swift run until="greeter: running" -->
```bash
npx @arcships/rutis-host dev
```

`dev` 用 `swift build` 构建二进制，启动两行，然后打印现在有哪些服务、各由哪一行提供：

```text
greeter: building
greeter: built in 3.2s
rutis-host dev: running greeter; changes reload it (Ctrl-C ends)
greeter: starting
translate: running
rutis-host dev: services: greeter (greeter), loader (host), translate (translate)
greeter: running
```

`dev` 运行时，另开一个终端，在项目目录里看宿主现在的状态、调用一次服务：

```console
$ npx @arcships/rutis-host status
rows:
  greeter: running
  translate: running
services:
  greeter: greeter
  loader: host
  translate: translate
runtimes:
  node: running (pid 66737)
  swift-greeter: running (pid 66740)
calls:
  none
$ npx @arcships/rutis-host call greeter.hello '"Ada"'
"Bonjour, Ada!"
$ npx @arcships/rutis-host call greeter.count
1
```

每个参数是一个 JSON 值（字符串带引号）；出错时错误写到标准错误并以 1 退出（见 [rutis-host：调用一个服务](rutis-host.md#调用一个服务)）。

把 `Plugin.swift` 里 `hello` 的 `\(name)!` 改成 `\(name)!!` 并保存：`dev` 重新构建，只重启 Swift 运行时，`greeter` 换上新代码（重启期间 `greeter` 暂时不在服务列表里）：

```text
greeter: building
greeter: built in 1.1s
swift-greeter: rebuilt and restarted
rutis-host dev: services: loader (host), translate (translate)
rutis-host dev: services: greeter (greeter), loader (host), translate (translate)
```

```console
$ npx @arcships/rutis-host call greeter.hello '"Ada"'
"Bonjour, Ada!!"
```

构建失败时打印编译器的错误，旧的构建继续运行。改 `dev/fake-translate.ts` 只重新加载 `translate` 那一行（`translate: reloaded (dev/fake-translate.ts changed)`），不重新构建。各行的状态行的先后每次可能不同。

Ctrl-C 结束 `dev`。`dev` 的其余规则见 [rutis-host：用 dev 开发插件](rutis-host.md#用-dev-开发插件)。在另一台机器上运行见 [rutis-host.md](rutis-host.md) 的远程一节。

## 8. 发布

Swift 插件发布的是二进制，不发到包仓库。推一个 tag，比如 `v0.1.0`：模板的工作流在 GitHub 的 macOS runner 上运行测试和 `rutis-host check`，构建 arm64 与 x86_64 的通用二进制，连同 `SHA256SUMS` 发布到这个仓库的 GitHub Release。配好它列出的 secrets（Developer ID 证书和公证凭据）时，二进制签名并公证；没有时是 ad-hoc 签名，下载它的宿主要先信任它。

依赖范围：`Package.swift` 里 `.package(url: "https://github.com/arcships/rutis", from: "<版本>")` 接受这个版本到下一个主版本之前的 SDK。

宿主怎样装你的插件见 [rutis-host：宿主怎样装插件](rutis-host.md#宿主怎样装插件)。

## 调试

`dev --debug` 不支持 Swift 项目。用 LLDB 连上正在运行的运行时：进程号在 `rutis-host status` 的 `runtimes` 里（`swift-greeter: running (pid 66740)`）。

```bash
lldb -p 66740
```

```
(lldb) breakpoint set -n GreeterPlugin.Greeter.hello
(lldb) continue
```

有调用到 `hello` 时停住；`bt` 看调用栈，`frame variable` 看变量，`detach` 断开而让运行时继续。

- 停在断点时整个运行时停着：这个二进制里其他插件的调用也等它继续。
- 每次重新构建都是一个新进程：重新连接，重新下断点。
- macOS 上第一次调试时，系统会请求允许开发者工具。
- `print` 的内容出现在 `dev` 的输出里。
- 测试不用宿主：在 Xcode 里打开 `Package.swift`，直接调试测试。
- 一行停在 `waiting` 或失败时，`rutis-host status` 写出它在等什么、失败的第一行。

## 系统权限

程序读日历、通讯录等之前，macOS 会询问用户，并为这个程序记住答复。对 Swift 插件，这个程序是**插件二进制本身**：运行时启动时让自己为自己的请求负责（不论是谁启动它：`rutis-host`、终端、进程管理器），所以提示里是这个二进制的名字，用户的授权只给它，不给宿主或终端。同一个二进制里的插件共享它的授权；需要不同授权的插件放进不同的二进制。

- 提示显示二进制的名字（`Info.plist` 的 `CFBundleName`）和所请求权限的用途说明：把对应的键（`NSCalendarsFullAccessUsageDescription`、`NSRemindersFullAccessUsageDescription`、`NSContactsUsageDescription`、`NSAppleEventsUsageDescription` 等）加进 `Info.plist`。模板的 `Package.swift` 把它链进二进制。
- 被拒绝是普通的失败：检查答复并抛出，任何语言的调用方都会收到这个错误：

```swift
import EventKit
import Foundation
import Rutis

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

要从 iOS、iPadOS 或 macOS 应用里提供插件，见 [在应用里托管插件](swift-app.md)。

## 进阶：插件 API

插件作为值、带方法的对象、值怎样传递、错误、同步方法、取消等见 [插件 API](plugin-api.md)。
