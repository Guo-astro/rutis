# Swift SDK：macOS 插件运行时与 Apple 应用托管（设计稿）

[English](design-swift-sdk-2026-10-12.en.md)

状态：定稿，未实现。日期：2026-10-12，2026-10-14 定稿修订（应用部分改为按 [应用接入](design-app-integration-2026-10-14.md)，远程连接支持监听与拨出，见 §十六），2026-10-19 对齐 rutis 0.13 的运行时契约（值、自动对外、错误、调用方的行、统一事件、开发者无感，见 §十七）。基准：`main` `028783f`（0.13.0）。
依据：[运行时契约](../.agents/skills/rutis-new-runtime/references/runtime-contract.md)（实现以它为准）、[多语言总体稿](design-multilang-runtimes-2026-10-03.md)（本文是其 M3，对其 dlopen 方案的修订见附录 A）、[应用接入](design-app-integration-2026-10-14.md)（形态②的通用部分）、[远程运行时：拨出与动态接入](design-remote-runtime-dial-2026-10-14.md)（连接）、[跨语言值类型](design-value-types-2026-10-15.md)、[统一事件的协议规格](design-events-protocol-2026-10-18.md)、[Go 插件运行时](design-go-runtime-2026-10-10.md)（形态①沿用其决策与机器，对照见附录 C）、[插件 API](guide/plugin-api.md)。

范围外：Swift 侧 Cordis；SwiftPM 之外的编译系统；宿主运行时编译 Swift 代码；Linux 与 Windows（§十三）；应用的前后台、后台存活、推送唤醒（业务层，见应用接入 §七）；App Intents 等 Apple 生态反向调用 rutis；在设备上运行 rutis 本体；接收对象引用（能力 `objects`，同 Go）。

## 一、决策

| # | 决策 | 章节 |
| --- | --- | --- |
| S1 | 两种托管形态共用一个 SDK：① macOS 插件二进制（本地多进程）；② 应用（iOS / iPadOS / macOS 应用内托管插件） | 二 |
| S2 | 形态①：一个二进制一个运行时进程，含一个或多个插件，由 `main.swift` 的 `Rutis.serve(...)` 决定；宿主可同时运行多个 Swift 运行时 | 三 |
| S3 | 形态①行名 `swift:<插件>`，`SwiftResolver` 按清单路由；限定写法 `<运行时名>:<插件>`；运行时名由文件名得出（前缀 `swift-`） | 三 |
| S4 | 形态①二进制自描述：`--rutis-manifest` 输出清单，解析不需要启动进程；运行期间以启动清单为准，换文件在重启后生效 | 三 |
| S5 | 形态①按需启动、空闲停止；`eager` 全部启动且不停止；崩溃后按运行时的 `restart`（`no` 默认 / `on-failure` / `exit`），与 Go 共用 `binary.rs` 的实现 | 三、十 |
| S6 | 形态②：插件编进应用，按 rutis 范式编写、由 rutis 驱动；SDK 对应用只提供 `start` / `stop`；连接推荐应用拨出 | 七 |
| S7 | 同一进程内调用不走 IPC；跨进程、跨语言经 rutis 按名字转发 | 四 |
| S8 | 每个进来的调用一个 Task；声明 `sync-wait` 与 `reentrant-sync`，不声明 `sync-stack`。适用于两种形态 | 五 |
| S9 | 调用链放 `@TaskLocal`，插件代码不传任何上下文参数；等待挂起当前 Task，不阻塞线程；方法默认 `async`，`sync` 只声明调用方怎样等，实现可以是 `async` 函数；调用对端 `sync` 方法时帧带 `sync: true` | 五 |
| S10 | 叶子插件；插件 API 版本 **3**；配置是普通的 `Decodable` 类型，由 SDK 的值解码器解码；Schema 由 `@RutisConfig` 宏生成（可选；不写时为 `{"type":"object"}`） | 4.4 |
| S11 | 提供服务：`@RutisService` 宏（编译期）从类型的方法生成方法表；`ctx.provide(name, obj)` 自动对外，`provides` 可选，没声明的服务以 `rows.announce` 通告、方法全为 `async` | 4.2 |
| S12 | 使用服务：普通的 Swift 协议加 `@RutisClient` 宏，宏生成代理 `<协议名>Proxy`；`ctx.use("llm", LLMProxy.self)` 得到 `any LLM`；同进程提供者实现了该协议时得到对象本身 | 4.3 |
| S13 | 值：SDK 自己的 `Encoder` / `Decoder`（由 `Codable` 驱动，不用 `JSONEncoder`）逐个检查；十种带标记的值全部实现并声明 `value-<tag>`；超出 ±(2^53−1) 的整数自动按 `bigint`；`@RutisService` 类型的实例与闭包按引用，其余 `Codable` 值按数据 | 4.5 |
| S14 | 远程连接两个方向都支持：拨出（`dial:`）与监听（`listen:`）；应用托管推荐拨出 | 六 |
| S15 | 宿主不编译；`run` 只运行现成二进制，`dev` 只重建项目自身；不同 SDK 版本的二进制可共存 | 十、十一 |
| S16 | SDK 位于 `swift/rutis`，经仓库根 `Package.swift` 分发，版本跟主发布线的 `v` tag；运行库 `Rutis` 只依赖 Foundation（监听另用系统的 Network.framework）；宏目标 `RutisMacros` 只在编译期依赖 swift-syntax | 十二 |
| S17 | 平台：macOS 14+（两种形态）、iOS / iPadOS 17+（形态②）；Linux 后续，Windows 不做 | 十三 |
| S18 | 统一事件（插件 API 3）：`ctx.on` / `emit` / `parallel` / `serial` / `waterfall` / `bail` / `filter`，能力 `events-v2`；监听器一律登记为 `kind: "sync"`（每次调用一个 Task，同 Go） | 4.7 |
| S19 | 错误两个方向都带 `name` / `message` / `code` / `data` 与错误图；收到的错误与 SDK 自己的错误都是 `RutisError`，按 `name` 匹配 | 4.6 |
| S20 | 调用方的行：每行按 `rows.load` 的 `isolate` 用自己的服务 id；作用域标签不透明，只原样拼接 | 6.2 |
| S21 | 测试工具 `RutisTest` 在测试进程里经真实会话（内存通道）运行插件，测试一方是宿主；与真实运行时对同一份 `weather.expected.json`、`events.expected.json` 比对 | 十四 |

## 二、两种托管形态

| | ① macOS 插件二进制 | ② 应用 |
| --- | --- | --- |
| 用途 | Mac 系统能力（EventKit、通讯录、辅助功能、屏幕录制、Core ML）、AppleScript / AX 自动化 | 应用把自身能力与所在设备的能力作为 rutis 插件提供 |
| 谁拉起 | rutis 宿主按需启动进程 | 应用自己；业务层决定何时 `start` / `stop` |
| 插件从哪来 | 二进制里编进的插件，清单自描述 | 应用里编进的插件，随应用发布 |
| 连接 | 本地通道（`fd:3` / `unix:` / `tcp:`）；远程时拨出或监听 | 拨出到 rutis 的接入点（也可监听） |
| 授权（TCC / entitlements） | 记在插件二进制 | 记在应用 |
| 机器 | `SwiftResolver` / `SwiftRuntimes`（与 Go 共用 `binary.rs`） | [拨出与动态接入](design-remote-runtime-dial-2026-10-14.md)（每条连接一个实例） |

两种形态的插件代码相同：同一个插件定义既可以 `serve` 成二进制，也可以交给应用托管。§四至§六 对两种形态一致。

## 三、形态①：macOS 插件运行时

### 3.1 运行时模型

```text
rutis 宿主
 ├─ LocalRuntime::node / ::python ...
 ├─ SwiftRuntimes（SwiftResolver + 生命周期，binary.rs）
 │    ├─ plugins/swift/weather ── swift-weather ── weather
 │    ├─ plugins/swift/kits   ── swift-kits    ── calendar、contacts
 │    └─ plugins/swift/ml     ── 未启动（无行使用）
 └─ LoaderPlugin + 每个运行中的运行时一个 RuntimeRowsPlugin
```

- 与 Go 运行时相同：行依赖 `RuntimeRows#<运行时名>` 与插件 `inject` 的全部名字（叶子运行时，rutis 门控全部名字）；插件提供的服务从行的 fiber 发布到 `host_key(name)`；进程退出时其全部行停止，使用其服务的行随之停止。
- `isolate`、实例标签、服务 id、导出句柄：与 Python / Go 运行时相同。
- TCC 记在插件所在的二进制上（§八）。

### 3.2 二进制与清单

- 插件作者分发：Swift 包（导出插件定义）+ `Sources/<名字>/main.swift`（`serve` 自己的插件），或一个包里若干插件编成一个二进制。
- 宿主接受任何响应 `--rutis-manifest` 的可执行文件（来源见附录 A.5）。

```swift
// example.com/kits/Sources/kits/main.swift
import Rutis
import WeatherKits

Rutis.serve(WeatherKits.calendar, WeatherKits.contacts)
```

| `serve` 规则 | |
| --- | --- |
| 参数 | `--rutis-manifest`：打印清单后退出；否则 `<channel> [--id <endpoint>] [--peer <endpoint>] <project>` |
| 插件同名 | 启动即退出，指明两个定义 |
| 登记方式 | 只在 `serve` 的参数里，不用静态初始化的注册表 |
| 子命令 | `Rutis.serveArgs(args, defs...)`；另有 `Rutis.serveConn(channel, defs...)`（内存会话，`RutisTest` 与一致性入口用） |

```json
{
  "manifest": 1,
  "sdk": "0.15.0",
  "pluginApi": 3,
  "runtime": "rutis-swift-runtime:1",
  "platform": "macos/arm64",
  "plugins": {
    "calendar": {
      "config": { "type": "object", "properties": { "calendars": { "type": "array", "items": { "type": "string" } } } },
      "inject": [],
      "provides": { "calendar": { "events": "async", "units": "sync" } },
      "announces": true,
      "version": null
    }
  }
}
```

| 项 | 规则 |
| --- | --- |
| 输出 | stdout，退出码 0；不连通道，不执行 `apply` |
| 插件条目 | 与 `rows.schema` 回复相同，同一段代码生成；`announces` 总是 `true`（SDK 会通告没声明的服务，契约 §4） |
| `runtime` | SDK 标记串 `rutis-swift-runtime:1`：目录扫描只执行含它的文件 |
| `platform` | `macos/<arch>`（通用二进制报 `macos/universal`）；宿主核对不符报"不是为 <os>/<arch> 构建的" |
| `sdk` | 常量 `Rutis.sdkVersion` |
| `pluginApi` | 3（插件 API 版本；SDK 能装载声明 2 或 3 的插件定义） |
| 插件 `version` | 作者静态串，缺省 `null` |
| 执行超时 | 30 秒（同 Go：macOS 上新二进制第一次运行要等系统评估） |
| 缓存键 / 绕过 / 信任 | 与 Go 相同：路径、大小、mtime（Unix 加 ctime）；启动、`restart`、`check` 绕过；只执行配置给出的来源 |

### 3.3 `SwiftResolver`

位于 rutis-loader（feature `swift`），实现 `Resolver`，与 `GoResolver` 同构：

| 项 | 规则 |
| --- | --- |
| 来源 | `SwiftBinaries`：文件列表 + 目录列表；遇到未知插件名时重扫一次目录 |
| 目录中的候选文件 | 可执行、不以 `.` 开头、内容含 SDK 标记串；只执行候选文件 |
| 取清单失败 | `binaries` 中为配置错误；目录中跳过 + 诊断；`check` 列出并以非零退出 |
| 运行时名 | 去扩展名 → 小写 → 非 `[a-z0-9-]` 换 `-` → 加前缀 `swift-`（`Net.Kits_v1` → `swift-net-kits-v1`） |
| `swift:<插件>` | 查所有清单；无：`NotFound`；多个：歧义错误，提示 `<运行时名>:<插件>` |
| `<运行时名>:<插件>` | 只查该运行时的清单 |
| 重名 | 与其他运行时（`node`、`py`、`bun`、`go`、远程）或彼此重名：`binaries` 中为配置错误；目录中跳过 + 诊断 |
| 解析结果 | 叶子结果，与 `GoResolver` 共用"由声明构造 `Resolved`"的代码；声明 `announces` 的插件，`check` 按契约报"可能在运行时提供" |
| meta | `{ source: "swift", binary, runtime, version, sdk }` |
| 运行中换文件 | 以启动清单解析；新二进制才有的插件报"已更换，`restart("<运行时名>")` 后生效" |
| 第二段放行 | `RuntimeRowsPlugin` 已接受 trait（Go G1 改造），`SwiftResolver` 按运行时实现同一 trait |
| 旧二进制 | 清单的 `pluginApi` 低于 3 的二进制不会声明 `events-v2`：会话在握手后被宿主拒绝（契约 §2），`check` 提前报出"用 0.13 以后的 SDK 重新构建" |

与 `GoResolver` 的差异只有三处常量（标记串、前缀、feature 名），实现走共用代码（§九）。

### 3.4 `SwiftRuntimes`

与 `GoRuntimes` 同构，位于 rutis-loader，挂在 loader 之后。

```rust
let swift = Arc::new(SwiftResolver::new(SwiftBinaries::new().dir("plugins/swift").file("bin/weather")).with_catalog(&catalog));
chain = chain.with_shared(swift.clone());
root.plugin(SwiftRuntimes::new(swift, project).idle(Duration::from_secs(60)).restart(Restart::No));
```

| 项 | 规则 |
| --- | --- |
| 结构 | 每个运行中的运行时 = 一个子 `Ctx`：`LocalRuntime::swift(binary, project).named(名)` + `RuntimeRowsPlugin`；停止 = 释放子 `Ctx` |
| 启动 | 解析出一行且其运行时未运行时启动（另有周期检查补足）；多个冷启动并行 |
| 空闲停止 | 最后一行卸载后等 `idle`（默认 60 秒），无未禁用条目解析到它则停止；同一时刻启动 / 重启 / 停止只一个在进行。空闲停止不是崩溃，不触发 `restart` |
| 崩溃 | 行停止，记录原因；之后按 `restart`：`no`（默认）不重启；`on-failure` 按 rutis-host 的规则重启（退避 1–30 秒，5 分钟内 5 次后放弃，打印同样的状态行）；`exit` 卸载其他行后宿主以 3 退出。规则与 node / py / bun 的 `restart` 相同（[rutis-host](guide/rutis-host.md#运行时结束时)），在 `binary.rs` 里为全部编译型运行时（Go、Swift）实现一次 |
| 手动重启 | `SwiftRuntimes::restart(名)`，或二进制文件已变化 |
| `eager()` / `runtimes()` | 与 Go 相同 |
| 单个固定二进制 | 可不用 resolver：`LocalRuntime::swift` + `RuntimeResolver::modules` |

## 四、Swift SDK（共享核心）

位于 `swift/rutis`（SwiftPM 目标 `Rutis`），Swift 5 语言模式，工具链 5.9 及以上，部署目标 macOS 14 / iOS 17。分层：

| 层 | 内容 | 平台 |
| --- | --- | --- |
| 核心 | 会话协议（compat / endpoint）、叶子 API、值编解码、事件、并发 | macOS + iOS（只依赖 Foundation；不触碰进程与文件系统设施） |
| 宏 | `RutisMacros`：`@RutisService`、`@RutisClient`、`@RutisConfig`；编译器插件，只在编译期运行，不进二进制 | 构建机 |
| 通道·进程 | `fd:3`、`unix:`、`tcp:`（POSIX） | macOS（形态①） |
| 通道·拨出 | WebSocket 客户端（Foundation `URLSessionWebSocketTask`；升级被拒时从任务的 `response` 读 HTTP 状态码，按契约 §1 分类），按契约 §1 与拨出稿 §六 的规范重连 | macOS + iOS |
| 通道·监听 | WebSocket 服务端（Network.framework `NWListener` + `NWProtocolWebSocket`），语义同 Go 的远程监听（一次一个控制方、新连接接管） | macOS + iOS |
| serve 打包 | `Rutis.serve` / `serveArgs` / 清单 CLI | macOS（形态①） |
| 应用托管 | `RutisApp`（§七） | macOS + iOS（形态②） |
| 测试工具 | `RutisTest`（§十四） | macOS + iOS |

### 4.1 插件

```swift
import Rutis

@RutisConfig                          // 可选：生成配置的 JSON Schema
struct Config: Decodable {
    /// 要查天气的城市
    var city: String
}

@RutisClient                          // 用到的服务：普通协议
protocol LLM {
    func ask(_ question: String) async throws -> String
}

@RutisService                         // 提供的服务：普通类型
final class Weather {
    let llm: any LLM
    let city: String
    init(llm: any LLM, city: String) { self.llm = llm; self.city = city }

    func today(_ question: String) async throws -> String {
        try await llm.ask("weather in \(city): \(question)")
    }
    func unit() -> String { "celsius" }
}

let weather = Rutis.define(Plugin(
    name: "weather",
    inject: ["llm"],
    provides: ["weather": .of(Weather.self, sync: ["unit"])],   // 可选：不写时自动对外，方法全为 async
    config: Config.self,
    apply: { ctx, config in
        let llm = try ctx.use("llm", LLMProxy.self)
        ctx.provide("weather", Weather(llm: llm, city: config.city))
        ctx.on("session/created") { (session: String) in print("new session \(session)") }
        ctx.effect { print("weather unloaded") }
    }))
```

| API | 规则 |
| --- | --- |
| `Rutis.define` | 校验声明（名称、`provides` 里的方法在类型的方法表中），返回可 serve、可交给应用托管的定义；带插件 API 版本 3 |
| `apply` | 可 `async throws`；抛错即装载失败，已登记的清理照常运行 |
| `ctx.use(name, P.self)` | `P` 是 `@RutisClient` 生成的代理类型，返回 `any 协议`；`name` 须在 `inject` 中。服务按本行自己的 id 找（§6.2）：同进程提供者实现了该协议时得到对象本身，否则得到经本地方法表（同进程）或经 rutis（跨进程）调用的代理。代理的方法须是服务提供的方法，否则 `use` 抛错并点名方法 |
| `ctx.provide(name, obj)` | `obj` 是 `@RutisService` 类型的实例；声明过的服务只对外声明的方法，没声明的服务自动通告（§4.2）；至卸载或返回的撤销函数为止；返回撤销函数 |
| `ctx.effect(cleanup)` | `cleanup` 可 `async`，卸载时逆序执行 |
| `ctx.on` / `emit` / `parallel` / `serial` / `waterfall` / `bail` / `filter` | 统一事件，§4.7 |
| `ctx.scope` | 本行的作用域链 `[Rutis.Scope]`（`rows.load` 第七个参数） |
| `ctx.spawn { ... }` | 子 Task 中运行（继承调用链），捕获错误写 stderr；卸载时取消并等待，和监听器共用一个关闭时限（§7.1），超过时写 `rutis: tasks of row <row> still running after <s>s; unloading anyway`，不再等 |
| 卸载信号 | 行的 Task 树在卸载时被取消：`Task.isCancelled` / `try Task.checkCancellation()`，与 Swift 原生取消相同 |
| `ctx.project` | 项目目录（形态②为应用约定的目录） |
| `Rutis.NoConfig` | 无配置 |

插件的方法、监听器、清理都不带 rutis 的上下文参数：调用链在 `@TaskLocal` 里，随 `await` 与子 Task 自动传递（§五）。

### 4.2 提供服务：`@RutisService`

Swift 没有方法反射（`Mirror` 只能枚举存储属性，不能枚举或调用方法），所以方法表在编译期由宏生成：

- `@RutisService` 附加在 class / actor / struct 上，生成对 `RutisExportable` 的遵循：方法表（线上名 → 解码参数、调用、编码返回值的闭包）与方法名列表。
- 哪些方法：非 `private` / `fileprivate` 的实例方法（`static`、`init`、下标、属性不算）——对应 Go 的"导出方法"、Python 的"不以 `_` 开头"。线上名是方法的基本名（`today(_:)` → `today`），参数按位置传，参数标签不上线；同一基本名有两个重载时编译错误。
- 方法可以是同步或 `async`、`throws` 或不 `throws`；参数与返回值是 §4.5 的任何值（`Codable`、闭包、`@RutisService` 实例、`Any?`）。
- 方法的 `sync` / `async` 是声明，不由 Swift 签名决定：`provides` 里 `.of(T.self, sync: ["unit"])` 列出的方法为 `sync`，其余与没声明的服务全部为 `async`（契约 §4）。也可以写字面量 `["today": .async, "unit": .sync]`。
- 自动对外：`ctx.provide` 的服务不在 `provides` 里时，按契约 §4 以 `service(id, handle, version, { row, methods })` 通告，方法取方法表、全部 `async`；只向 `mount` 带 `announce: true` 的宿主通告，回复到了才答复 `rows.load`。回复总是 `undefined`：重名时由宿主在 `rows.load` 答复后让这一行作为后来的提供者失败，错误写明双方；等这些回复是为了让宿主在这一行算作装载完成前处理完通告，SDK 不从回复里读错误。宿主比 0.12 旧时按契约在 stderr 提示一次。
- `rows.schema` 与清单的 `announces` 总是 `true`。
- 同进程调用：直接调用对象（S7）；只经方法表、不编码的路径用于对象没有实现使用方协议的情形。

### 4.3 使用服务：`@RutisClient`

- 使用方写一个普通的 Swift 协议，只列用到的方法；`@RutisClient` 生成 `<协议名>Proxy`（遵循该协议与 `RutisProxy`）。
- 代理的方法：`async throws`；协议里的同步方法不能做跨进程代理（编译期报错并提示改为 `async throws`），因为 Swift 不阻塞线程等待。
- 调用对端声明为 `sync` 的方法时帧带 `sync: true`，当前 Task 挂起等待（§五），参加宿主的等待环检测；声明为 `async` 的方法发普通调用。种类取自 `hosts.provide` 给出的方法表，不取自 Swift 签名。
- 协议是普通 Swift，测试里可以用任何手写实现替代。

### 4.4 配置

| 项 | 规则 |
| --- | --- |
| 类型 | 任何 `Decodable` 类型；由 SDK 的值解码器（§4.5）从 `rows.load` 的配置值解码；失败即装载失败，错误点名字段 |
| Schema | `@RutisConfig` 由存储属性生成 JSON Schema：`String` / 整数 / 浮点 / `Bool` / 数组 / 字典 / 嵌套的 `@RutisConfig` 类型 / 可选（不进 `required`）/ `String` 原始值的枚举（`enum`）；`///` 文档注释成为 `description`。不写宏时 Schema 是 `{"type":"object"}`（宿主不预先校验，解码照常检查）。类型上写 `static let rutisSchema` 时以它为准 |
| 值映射 | JSON Schema 语义与 Go §6.3 相同 |

### 4.5 值

值层是 SDK 自己的 `RutisEncoder` / `RutisDecoder`（实现 `Encoder` / `Decoder`，由类型的 `Codable` 实现驱动），逐个检查每一个值；不经 `JSONEncoder`（它把 `Date` 编成数字、`Data` 编成 base64，结构体内部绕过检查，即 Go 在 #355 的问题）。规则按[值类型设计](design-value-types-2026-10-15.md)与契约 §5。

| Swift 值 | 跨进程 |
| --- | --- |
| `nil`、`Bool`、`String`、`Double` / `Float` | 复制；NaN、±Infinity 让这次调用失败 |
| 整数（`Int`、`Int64`、`UInt64` 等） | ±(2^53−1) 内为数字；超出时**自动**按 `bigint`（V6） |
| `Array`、`Dictionary<String, T>`、`Codable` 的 struct / enum / class | 递归复制为数据（字段名按 `CodingKeys`）；字符串键的字典是对象 |
| `Date` | `instant`；发送与接收都截断到微秒（向负无穷），年份 0001–9999 |
| `Rutis.DateTime` / `Rutis.CalendarDate` / `Rutis.TimeOfDay` | `datetime` / `date` / `time`（Swift 没有这三种类型；`Rutis.DateTime` 等可与 `DateComponents` 互转）；精度纳秒 |
| `Data` | `bytes` |
| 键不是 `String` 的 `Dictionary` | `map`；键与 `Set` 元素的限制按契约 §5（`Date`、`Data`、可选、数组、字典、集合、引用作键在发出前拒绝） |
| `Set` | `set` |
| `Rutis.BigInt` | `bigint`；收到的 `bigint` 解码进放得下的声明整数类型，放不下时这次调用失败 |
| `Decimal` | `decimal`；Foundation `Decimal` 至多 38 位有效数字，超出时这次调用失败；它不保存末尾零（`1.10` 收到为 `1.1`），要原样保留文本时把参数声明为 `String` |
| `UUID` | `uuid`（小写） |
| 闭包（`async throws` 函数类型） | 引用；参数与返回值按本表编解码。收到的函数只能声明为 `async throws` 函数类型（跨进程调用要等待） |
| `@RutisService` 类型的实例 | 引用；远端得到调用其方法表的代理 |
| `Error` | 作为值时复制为 `{ name, message }`；抛出时见 §4.6 |
| 声明为 `Any` / `Any?` 的参数 | 按上表反向解码成原生值：数字为 `Int` / `Double`，对象为 `[String: Any?]`，列表为 `[Any?]`，`map` 为 `[AnyHashable: Any?]`，`set` 为 `Set<AnyHashable>`，`bigint` 为 `Rutis.BigInt`，函数为 `Rutis.RemoteFunction`（`callAsFunction`）；原样转发时类型不变 |
| 其他 class 实例（不是 `Codable` 也不是 `@RutisService`）、循环引用 | 不可跨进程：这次调用失败，点名类型 |

- 嵌套至多 64 层，由发送方在发出前检查；一个值的任何一处不合规则只让这次调用失败，会话继续。
- 每种标记只发给声明了 `value-<tag>` 的对端；对端没有声明时这次调用失败，点名哪种值。SDK 在两种握手里声明全部十个 `value-<tag>`。
- 收到的带标记的值或键不合法时只让这次调用失败，其余照常解码，其中的引用全部导入再释放（契约 §5）。
- 数据里的 `undefined` 收到为 `nil`。

### 4.6 错误

| 方向 | 规则 |
| --- | --- |
| 发出（插件抛出） | `name` = 错误类型的简名（`String(describing: type(of: error))`）；`message` = `LocalizedError.errorDescription`，没有时 `String(describing: error)`；错误有名为 `code` 的 `String` 存储属性、名为 `data` 的存储属性时（`Mirror` 读取，原生 Swift 写法，不需要 rutis 类型），带上 `code` 与 `data`；`data` 发不出时略去，其余照常发出 |
| 收到 | `RutisError`：`name`、`message`、`code: String?`、`data: Any?`（按值解码）、`cause`、`errors`（`AggregateError` 的子错误，按登记顺序）；按 `name` 匹配（`err.name == "SyncWaitCycle"`），不按类型 |
| 再次抛出 | 收到的 `RutisError` 原样再抛时整个错误图（含 `code` / `data`）再跨一次 |
| SDK 自己的错误 | 也是 `RutisError`，名字按契约：`AggregateError`、`AsyncListener`、`WaterfallNext`、`ReservedEvent`、`SyncWaitCycle`、`BindingError`；常量 `RutisError.Name.syncWaitCycle` 等 |

### 4.7 事件

统一事件按[协议规格](design-events-protocol-2026-10-18.md)与契约 §6。API（Swift 的原生写法）：

```swift
let off = ctx.on("greeting/format") { (text: String, next: Rutis.Next<String>) async throws -> String in
    "\(try await next() ?? text)!"
}
ctx.on("rutis/row", prepend: true) { (row: [String: Any?]) in print(row) }

try ctx.emit("t/ping", 1, "a")                                   // 立即返回；只因 ReservedEvent 或参数发不出而抛
try await ctx.parallel("t/ping", 1)                               // 失败：RutisError(name: "AggregateError")
let first: String? = try await ctx.serial("t/ask", "q", as: String.self)
let text: String? = try await ctx.waterfall("greeting/format", "hi", as: String.self) { (text: String) in text }
let vote: Bool? = try await ctx.bail("t/check", 42, as: Bool.self)  // 线上同步：sync: true，参加等待环检测
let some = ctx.filter { $0.row == "chat" }                        // Rutis.Emitter：同样的五个方法
```

| 项 | 规则 |
| --- | --- |
| 监听器 | 任意参数个数的闭包（参数包），参数按声明类型解码；`waterfall` 中最后一个参数是 `Rutis.Next<R>`（`async throws -> R?`）；多出的事件参数忽略，缺少的参数为 `nil`（非可选时这次调用失败）；返回值原样回复 |
| 选项 | `prepend:`、`global:`；返回立即注销的函数；随行卸载注销 |
| 登记 | `ctx.on` 时发 `events.on [{ id, name, row, plugin, kind: "sync", prepend, global }]`（`id` 会话内递增、不复用）；`apply` 期间登记的全部得到回复后才答复 `rows.load`；`internal/*` 在本地拒绝，宿主拒绝时按契约报告或让装载失败 |
| `kind` | 一律 `"sync"`：每次调用一个 Task，监听器即使是 `async` 闭包也在自己的 Task 里跑完再回复，宿主可以同步等它（同 Go），所以 Swift 监听器从不引起 `AsyncListener` |
| 快速路径 | 名字不在 `events.elsewhere`、无前缀匹配，且（`emit`）没有经宿主、未答复的同名 `emit` 时本进程分发；否则发 `events.dispatch`。本进程条目的描述用 `mount` 的 `runtime` |
| 分发规则 | 快照（名字、登记顺序、作用域链前缀、存活）→ 过滤（非 `global`）→ 每次开始前检查存活；五种方式的语义与"没有结果"（`nil`、`false`）按契约 §6 |
| `emit` 顺序 | 每个监听器一个串行队列：同一监听器上的 `emit` 调用按到达顺序一个接一个执行，不同监听器并发 |
| 过滤 | 快速路径直接对每个描述调用；经宿主时 `filter` 是一个选择器函数引用，凭描述的 `listener` 找到本进程的条目、去掉该字段后再交给插件的谓词；描述是 `Rutis.Listener`（`name`、`prefix`、`plugin`、`row`、`runtime`、`node`、`scope`、`global`，没有值时为 `nil`） |
| 取消 | 发出方的 Task 被取消：经宿主时对 `events.dispatch` 发 `cancel`；收到 `events.call` 的 `signal` 中止或 `cancel` 时取消该监听器的 Task |
| 保留名 | 发出 `rutis/*` 同步以 `ReservedEvent` 拒绝（`rutis/* events are emitted by rutis only: <name>`）；可以监听 `rutis/*` |
| 报告 | 没有人等待的失败按契约 §6 的原文各写一行 stderr（形态②写到 `RutisApp` 的日志回调，默认 stderr） |
| 卸载 | 收到 `rows.unload` 后立即拒绝这一行的 `events.call` → 撤下提供的服务 → 取消这一行正在运行的监听器 Task 和 `ctx.spawn` 的 Task → 等待监听器，至多关闭时限（形态① `RUTIS_SHUTDOWN_TIMEOUT`，形态② `shutdownTimeout` 参数，默认 10 秒），超过时写 `rutis: listeners of row <row> still running after <s>s; unloading anyway` → 再在同一时限的剩余时间内等待 `ctx.spawn` 的 Task（监听器已用完时限就不再等），超过时写 `rutis: tasks of row <row> still running after <s>s; unloading anyway` → 逆序清理 |
| 断开 | `elsewhere` 视为空；卸载全部行 |

## 五、并发、调用链、取消

适用于两种形态。

| 项 | 规则 |
| --- | --- |
| 会话结构 | 一个读线程（阻塞读通道，帧经 `AsyncStream` 进 async 世界；阻塞读不落在协作线程池上）；写帧一个 actor 串行；每个进来的 `call` / `invoke` / `get` / `await` / 控制操作一个新 Task；`events.on` / `events.off` / `events.elsewhere` 在读循环里按帧序就地处理；行状态各自加锁 |
| 可重入 | 任何时候执行任何进来的调用；声明 `reentrant-sync`。不声明 `sync-stack`：调用并发跑在不同 Task 上，无单栈顺序（与 Go 同类，见契约 §2） |
| 同步等待 | 调用对端 `sync` 方法或 `bail` 时帧带 `sync: true`，当前 Task 挂起在 continuation 上等回复——**不阻塞任何线程**；声明 `sync-wait`，参与宿主的等待环检测。等待期间其他 Task 照常执行进来的调用 |
| 插件要求 | 服务对象可被并发调用（推荐 `actor` 或 `Sendable` 类型）；调用其他服务时不持锁（写进指南） |
| 调用链 | `@TaskLocal var path: [String]`：进来的调用在其 Task 里设 `path + [id]`；发出的调用从当前 TaskLocal 取链；`apply` / 清理带 `rows.load` / `rows.unload` 的链；插件自己用 `Task.detached` 时链丢失（写进指南：用 `ctx.spawn` 或普通 `Task {}`） |
| `sync` 方法 | 实现可以是 `async`，也可以在里面调用其他服务（挂起不占线程）；与 Go 的行为相同 |
| 进来的取消 | `cancel` 帧或 `signal` 中止 → 该调用的 `Task.cancel()`；`Task.checkCancellation()` 抛 `CancellationError` |
| 发出的取消 | Task 被取消时：`async` 调用发 `cancel`，立即抛 `CancellationError`，丢弃迟到结果；`sync` 调用立即抛，丢弃迟到结果，对端照常执行 |
| 行卸载 | 取消该行 Task 树；顺序按 §4.7 的卸载 |
| `SyncWaitCycle` | 自身不产生；收到时为 `RutisError`，`name == "SyncWaitCycle"` |

## 六、通道与契约

### 6.1 通道

| channel | 形态 | 实现 |
| --- | --- | --- |
| `fd:3` | ① | Unix 本地继承：`dup` + POSIX read/write |
| `unix:<path>` 或裸路径 | ① | 拨回：POSIX socket |
| `tcp:127.0.0.1:<port>` | ① | 先发 `RUTIS_CHANNEL_TOKEN` + 换行，随即从环境删除（`session_matrix` 的回环用例） |
| `dial:wss://…` | ①远程、②（推荐） | WebSocket 客户端，子协议 `rutis.3`；按契约 §1 的拨出规则：`RUTIS_TOKEN` 作 Bearer、`RUTIS_CA` 加信任、非回环 `ws://` 与不合法 id 启动即拒绝、按状态码 / 关闭码分类重试或停止、每次断开逆序卸载全部行、SIGTERM 卸载后以 0 退出、stderr 状态行原文；`RUTIS_RETRY` 供测试缩短。形态②中这些值是 `RutisApp` 的参数 |
| `listen:ws://…` / `listen:wss://…` | ①远程、② | WebSocket 服务端；非回环必须 `wss://`（`RUTIS_CERT` / `RUTIS_KEY`）；`RUTIS_TOKEN` 常量时间比较；心跳 30 秒（`RUTIS_HEARTBEAT`）；一次一个控制方，新连接接管 |

- 按行分帧，单条消息至多 16 MiB，超出关闭通道；`project` 参数经 `ctx.project` 提供。
- 对端（宿主）的 `hello` 没有声明 `events-v2` 时，握手后立即结束会话，stderr 写 `rutis: refused: incompatible (this host does not declare events-v2 (the event protocol of rutis 0.13): upgrade rutis-host to 0.13 or later); stopping`，形态①以状态 1 退出（拨出不重试），形态②停止重连、`state` 为 `stopped` 并带这条原因。

### 6.2 契约

compat 协议版本 2（本地通道）、endpoint 协议 3（远程，监听与拨出）。

| 项 | 值 |
| --- | --- |
| 能力（两种握手都声明） | `signals`、`sync-wait`、`reentrant-sync`、`events-v2`、十个 `value-<tag>`；endpoint 握手另声明 `runtime`；不声明 `sync-stack`、`objects` |
| 特性（`mount` 回复） | `["rows.v2", "hosts", "leaf", "scopes", "rows.announce"]` |
| 调用号 | 前缀 `node:`（与其他叶子运行时相同） |

| 控制操作 | 行为 |
| --- | --- |
| `mount` | 记下参数 `announce` 与 `runtime`；回复 `{ services: {}, features, plugins, implementation: { name: "github.com/arcships/rutis", version: Rutis.sdkVersion }, engine: { name: "swift", version: <构建时嵌入的工具链版本> } }`；`plugins` 与清单的 `plugins` 相同（拨出稿 §四） |
| `rows.schema(entry)` | 清单条目（`config`、`inject`、`provides`、`version`、`announces: true`）；未知入口报错并列出可用的名字 |
| `rows.load(key, entry, config, isolate, inject, exports, { scope, id })` | 解析入口 → 核对插件 API（插件比 SDK 新时拒绝，写出两个版本）→ 解码配置 → `apply`；声明的服务以 `service(id, handle, version)` 报告、没声明的以四参数通告并等宿主答复；`apply` 期间的 `events.on` 全部得到回复后才答复；失败时先卸载这一行再报告；忽略 `inject`（门控由 rutis 做）；`id` 是描述与报告行称呼这一行的名字（没有时用 `key`） |
| `rows.update(key, config)` | 先卸载再装载（叶子插件没有易变字段），监听器重新登记 |
| `rows.unload(key)` | §4.7 的卸载顺序 |
| `hosts.provide(name, methods, label?)` / `hosts.withdraw(id)` | 为 rutis 服务登记代理，调用目标 `host:<id>`；`id` = `name`，有标签时 `name` + NUL + 标签 |
| `release` / `get`、`dispose` | 与 Go 相同；`dispose` 卸载全部行并让进行中的调用完成 |

- **调用方的行**：宿主把每一行的服务租进这一行自己的作用域（标签里带 U+0001），以便 `host:<id>` 的调用让宿主知道是哪一行在调用。SDK 对每一行按 `rows.load` 的 `isolate`（名字 → 标签）计算该行使用的服务 id，`ctx.use` 只用这一行自己的 id；标签不透明，不解析、不比较内容，只校验不含 NUL、不为空。插件代码在哪一行里运行，发出的调用就指向哪一行的 id（行的 Task 树带着行，`ctx.spawn` 也是）。
- **会话结束**（rutis 不在了）：卸载全部行；清理合计受 `RUTIS_SHUTDOWN_TIMEOUT`（默认 10 秒）约束，超过时写一行 stderr；形态①以状态 1 退出（全部完成时以 0），形态②回到未连接状态（应用继续活着），之后按拨出规则重连。

### 6.3 退出与清理

| 情况 | 规则 |
| --- | --- |
| `apply`、方法、监听器、清理抛出的错误 | 捕获为错误回复（`emit` 的监听器写报告行） |
| `fatalError` / `try!` / 强解包崩溃 | Swift 不可捕获 → 形态①进程退出、行全部停止，之后按运行时的 `restart`（§3.4）；形态②是应用崩溃：连接断开，rutis 侧实例关闭、服务撤销 |
| `ctx.spawn` 里抛出的错误 | 写 stderr，不结束进程（同其他运行时的后台任务） |
| `rows.unload`、`dispose` 时限 | 监听器等待受关闭时限约束（§4.7）；清理本身不另设时限，时限由 rutis 侧决定 |

## 七、形态②：应用托管

通用模型见 [应用接入](design-app-integration-2026-10-14.md)：插件按范式编写、编进应用、由 rutis 驱动；SDK 对应用只提供 `start` / `stop`；应用拨出到 rutis 的接入点，每条连接在 rutis 侧是一个实例。本节只写 Swift 侧。

```swift
let rutis = RutisApp(
    plugins: [health, location, device],         // 编进应用的插件定义（§4.1）
    connect: "wss://rutis.example.com:7443/rutis",
    id: "kim-iphone",                             // 自报 id（拨出稿 §五）
    token: secrets.rutisToken)

try await rutis.start()   // 连上；此后插件由 rutis 按配置装载
await rutis.stop()        // 全部插件逆序清理，然后断开
```

| 项 | 规则 |
| --- | --- |
| 参数 | `plugins`、`connect`（或 `listen:`，见下）、`id`、`token`；可选 `peer`（rutis 的 id）、`trust`（额外信任的 CA 证书，`RUTIS_CA` 的语义）、`shutdownTimeout`（默认 10 秒）、`log`（报告行的去处，默认 stderr） |
| `start()` | 连接、问候、报 `plugins`；断线后按拨出规则（契约 §1）自动重连，直到 `stop()`；凭据被拒每 30 秒重试；不兼容（含宿主没有 `events-v2`）停止并把原因放进 `state` |
| `stop()` | 卸载全部行（清理逆序），关闭连接；可再次 `start()` |
| 监听 | `RutisApp(plugins:, listen: "wss://0.0.0.0:7443/rutis", certificate:, token:)` 改为等 rutis 拨入，语义同 §6.1 的 `listen:`；适合地址固定的设备，一般场景推荐拨出 |
| 状态 | `rutis.state`（`stopped(reason?)` / `connecting` / `connected`）可观察（`AsyncStream` 与 `@Observable`），供业务层显示；SDK 不据此做任何生命周期决定 |
| 被系统挂起或终止 | 不做特殊处理：对 rutis 侧表现为断开 |
| 打包 | SDK 静态链入应用（SwiftPM 库依赖）；无动态装载 |
| 授权 | entitlements（HealthKit 等）与 usage description 在应用的 Info.plist / provisioning profile；授权提示指向应用；拒绝 = `apply` 或方法调用抛错，照常传回调用方 |
| 本地网络 | 连局域网地址时 iOS 会弹本地网络权限（`NSLocalNetworkUsageDescription`），写进 SDK 指南 |
| 换代码 | 发新版本应用 |
| 设备信息 | rutis 不建模设备；需要时由插件提供（如 `device` 插件） |

## 八、TCC 与签名

授权记在"谁调用系统 API"上：形态①是插件二进制，形态②是应用。

| 项 | 形态① | 形态② |
| --- | --- | --- |
| 授权归属 | 运行时二进制自身；提示里的名字来自其签名与内嵌 Info.plist | 应用 |
| 用途说明 | 裸可执行文件把 Info.plist 链进 `__TEXT,__info_plist`（linker `-sectcreate`，SwiftPM `linkerSettings`）；`new --lang swift` 模板带好 | 应用 Info.plist |
| 本地开发 | ad-hoc 签名即可（Apple Silicon 上 `swift build` 产物默认 ad-hoc） | 应用的开发签名 |
| 分发 | Developer ID 签名 + 公证 + hardened runtime | 应用本身的分发渠道 |
| 库验证 | 不 dlopen，无 library validation 问题（附录 A.4） | 同左 |
| 隔离属性 | 复用 Go 的 `xattr` 检查与提示 | — |
| 粒度 | 同二进制的插件共享一个 TCC 身份；需要不同粒度分属不同二进制 | 同应用的插件共享应用的授权 |

验收：EventKit 插件在签名二进制里运行，授权提示指向该二进制（形态①，M3 原文）；iOS 应用里 HealthKit 插件的授权提示指向应用，拒绝后错误传回 rutis 侧调用方（形态②）。

## 九、Rust 侧改动

| 改动 | 位置 |
| --- | --- |
| 共用的编译型运行时模块：清单执行与缓存、目录扫描与标记串嗅探、运行时名推导、按需生命周期、`restart` 策略（`no` / `on-failure` / `exit`，规则与 rutis-host 的本机运行时相同）、手动 restart、文件替换语义；`go.rs` 是它的薄封装（从 Go 抽取，Go 现有测试保护） | `rutis-loader/src/runtime/binary.rs` |
| `SwiftResolver`、`SwiftBinaries`、`SwiftRuntimes` | `rutis-loader/src/runtime/swift.rs`（新，薄封装，三处常量） |
| `swift_runtime_name`；`Launcher::swift`（与 `Launcher::go` 同一条 spawn 路径）；`LocalRuntime::swift` | `rutis-bridge/src/runtime/{local,process}.rs` |
| feature：rutis-bridge `swift = []`；rutis-loader `swift = ["runtimes", "rutis-bridge/swift"]`；rutis-host 依赖开 `swift` | 三个 `Cargo.toml` |
| 远程 npm 命名拒绝 `swift:` 与 `<swift 运行时名>:` 前缀 | 已实现，仅加测试 |
| 形态②：`runtimes.remote[].language` 接受 `"swift"`；其余见拨出稿 §七 | `rutis-host/src/host.rs`（已实现） |
| 一致性：事件期望文件的语言加 `swift`（`only` / `except` 可用）；`events_conformance.rs` 与 `session_matrix.rs` 加 Swift 运行时（feature `swift`） | `rutis-bridge/src/runtime/testing/events.rs`、测试 |
| `scripts/train.mjs` 版本正则加 `swift/rutis/Sources/Rutis/Version.swift` | `scripts/train.mjs` |

实现记录（0.15，抽取已完成）：`binary.rs` 提供 `BinaryLanguage` trait（`PREFIX`、`TITLE`、`MARKER`、`PLUGIN_API` 常量，`runtime_name`、`launch` 两个函数）和以它为参数的 `Binaries`、`BinaryManifest`、`BinaryResolver<L>`、`BinaryRuntimes<L>`、`BinaryRuntimesHandle<L>`、`BinaryRuntimeState`、`BinaryRuntimeInfo`、`read_binary_manifest`，feature `binary`（`go` 开启它，`swift` 也应开启：`swift = ["binary", "rutis-bridge/swift"]`）。`go.rs` 只剩 `struct Go` 的实现和 `GoResolver = BinaryResolver<Go>` 等类型别名，Go 用户的代码不用改。`swift.rs` 照此写：`struct Swift` 实现 `BinaryLanguage`（`swift_runtime_name`、`LocalRuntime::swift`）加别名。崩溃重启也在共用模块里：`BinaryRuntimesHandle::supervise(RestartPolicy, 回调)`，退避用 `rutis_loader::backoff`（与 rutis-host 的 node/py/bun 同一份）；rutis-host 的 `runtimes.go.restart` 经它实现，`runtimes.swift.restart` 照搬即可。

## 十、rutis-host

```json
{
  "runtimes": { "swift": { "dir": "plugins/swift", "binaries": ["bin/weather"], "start": "on-demand", "idle": 60, "restart": "no" } },
  "rows": [{ "id": "weather", "name": "swift:weather", "config": { "city": "Oslo" } }]
}
```

字段与 `runtimes.go` 相同（`dir` / `binaries` 至少一个；`start`：`on-demand`（默认）/ `eager`；`idle` 秒；`restart`：`no`（默认）/ `on-failure` / `exit`，含义同 node / py / bun；另有 `project`）。`restart` 是运行时的设置，运行中改它要重启宿主。形态②的配置见拨出稿 §四。

| 命令 | 行为 | 阶段 |
| --- | --- | --- |
| `check` | 每个二进制：运行时名、平台、SDK / 插件 API 版本、插件；歧义；每行的目标二进制；失败的候选文件；隔离属性；插件 API 低于 3 的旧二进制 | S2 |
| `dev` | 无 `rutis.dev.json` 时据 `Package.swift` 判定 Swift 项目；重建与重启见 §十一；崩溃策略同 `run` | S2 |
| `new <name> --lang swift` | 插件 target、`Sources/<name>/main.swift`、含 usage description 的 `Info.plist` 与 `linkerSettings`、`RutisTest` 测试、`rutis.dev.json`、发布工作流（发布产物为通用二进制） | S2 |
| `run` | 不需要 Swift 工具链 | — |

## 十一、换代码（形态①）

与 Go §十二 相同：`dev` 监视 `.swift`、`Package.swift`、`Package.resolved` → `swift build -c release` 到缓存目录新文件 `<name>-<序号>` → 成功替换该运行时的文件并重启它；失败打印错误，旧进程继续。部署：改名替换 → `SwiftRuntimes::restart("<运行时名>")`。

## 十二、仓库与发布

| 项 | 规则 |
| --- | --- |
| 位置 | `swift/rutis`：目标 `Rutis`（核心 + API）、`RutisMacros`（宏插件）、`RutisTest`；会话层 `Sources/Rutis/Internal/Peer`；一致性可执行目标 `RutisConformance`（`conformance-weather`、`conformance-events` 两个插件） |
| 清单 | **仓库根** `Package.swift`，目标用 `path:` 指向 `swift/rutis/Sources/...`；平台 `.macOS(.v14)`、`.iOS(.v17)`。SwiftPM git 依赖只认仓库根（附录 E.1）；仓库打包体积约 18 MiB（2026-10-14），依赖方克隆代价可接受 |
| 版本 | `Rutis.sdkVersion` 跟主发布线：现有 `v0.x.y` tag 正是 SwiftPM 认的形式（附录 E.2），无需新 tag 命名空间；`from:` 从首个含根 `Package.swift` 的 release（0.15.0）起 |
| 依赖方写法 | 插件作者与应用同一条：`.package(url: "https://github.com/arcships/rutis", from: "0.15.0")` + `.product(name: "Rutis", package: "rutis")`；应用在 Xcode 里直接添加包（第一次构建时 Xcode 要求信任宏，CI 用 `-skipMacroValidation`，写进指南） |
| 依赖 | `Rutis` 仅 Foundation 与系统的 Network.framework（监听）；`RutisMacros` 编译期依赖 swift-syntax（版本范围跟两个支持的工具链）；不引入 uniffi / swift-bridge（附录 B） |
| 兼容 | 同一协议版本内，任意 SDK 版本的二进制 / 应用可在任意宿主版本运行，前提是插件 API 不高于宿主支持、双方都声明 `events-v2`（rutis 0.13 起） |
| 语言模式 | Swift 5 模式起步，API 按 Sendable 友好设计；工具链策略同 Go："当前两个稳定版"（今日为 6.3、6.4） |
| 文档 | `docs/guide/swift-plugin.md`（形态①）、`docs/guide/swift-app.md`（形态②，含本地网络权限等接入注意事项），中英；`docs/guide/plugin-api.md` 加 Swift 列（声明、ctx、事件各语言写法、十种值、错误、测试工具差异） |

## 十三、平台

| 平台 | 形态① | 形态② | 说明 |
| --- | --- | --- | --- |
| macOS 14+ | ✅ | ✅（Mac 应用） | Swift 5.9 运行时随系统，参数包无部署顺延问题 |
| iOS / iPadOS 17+ | — | ✅ | 无进程 spawn、无动态装载 |
| watchOS / visionOS | — | 后续 | 有需求时接入 |
| Linux | 后续 | — | 有真实需求再接 |
| Windows | 不做 | — | |

## 十四、测试

运行时契约的一致性测试就是契约本身（契约 §8）；Swift 过与其他运行时相同的全部套件。

| 测试 | 位置 | 内容 |
| --- | --- | --- |
| 会话层单元 | `swift/rutis/Tests` | 编解码（十种值、键规则、截断、深度、超界整数）、引用计数、同步中回调、取消、TaskLocal 取链、并发、挂起等待不阻塞、宏展开（方法表、代理、Schema） |
| iOS 核心单测 | 同上，CI 加 `xcodebuild test -destination 'platform=iOS Simulator'` | 核心层在 iOS 上编译并测试（值、叶子 API、事件、内存会话、`RutisApp` 状态机） |
| 会话一致性 | `rutis-bridge/tests/runtime_conformance.rs` + `RutisConformance` 目标 | `session::testing::session` |
| 契约 × 通道 | `session_matrix.rs` 加 `Swift` | `fd:3`、拨回、回环 `tcp:`、（A1）监听 WebSocket；`weather.expected.json` 的全部调用、样例值与事件 |
| 值往返 | `tests/value_roundtrip.rs` 加 Swift 端 | 十种值与键规则的属性测试 |
| 事件一致性 | `rutis-loader/tests/events_conformance.rs`（X = swift；Y 含 swift） | `events.expected.json` 全部用例：五种方式、作用域、过滤、卸载等待与取消、断开、`rutis/*`；CI 的 `RUTIS_EVENTS_REQUIRE` 在 macOS 上加 `swift` |
| 测试工具 | `swift/rutis/Tests/RutisTestConformance` | `RutisTest` 对 `weather.expected.json` 与 `events.expected.json`（`tool` 不为 `false` 的用例）比对，结果与真实运行时相同（含 `undeclared`：推断的方法全为 `async`） |
| 旧宿主 | `rutis-bridge/tests/swift_runtime.rs` | 不声明 `events-v2` 的宿主：原文拒绝行、状态 1、拨出不重试 |
| Swift 专项 | `rutis-bridge/tests/swift_runtime.rs` | 取消到 Task；错误形状（`code` / `data` 两个方向、再抛保留错误图）；`fatalError` 退出撤回服务；引用与 release；调用方的行（两行不同 `isolate` 各用自己的 id） |
| 清单 / 多二进制 / 按需 / 重启策略 | `rutis-loader/tests/swift_rows.rs` | 与 `go_rows.rs` 同构全套；`on-failure` 下 kill 后行恢复，空闲停止不触发重启 |
| 运行时一致性 | `multilang.rs`（feature `swift`） | Swift 叶子插件；多语言冷启动互调；提供者移除只停使用者；`inject` 门控；没声明的服务被通告 |
| 交叉同步调用 | `multilang.rs` | Node→Swift→回调 Node（带链）、Swift↔Go、Swift↔Python：不卡死；Node 与 Swift 的交叉同步环被检测为 `SyncWaitCycle` |
| 监听 | `rutis-bridge/tests/websocket_conformance.rs` 加 Swift 远程端 | 与 Go 的远程监听同一套断言 |
| 拨出 | `rutis-bridge/tests/dial_conformance.rs` 加 Swift 列（拨出稿 §六） | 退避、凭据被拒、同 id 接管、断开即关闭实例、`stop` / `start` |
| 授权（e2e） | e2e 套件 | 形态① EventKit；形态② iOS HealthKit：提示归属、拒绝后错误传回。授权点击为人工验收清单，其余自动化 |
| 夹具 | `crates/rutis-loader/tests/fixtures/swift` | 测试开始时构建成至少两个二进制 |
| CI | `runtimes-swift`（macOS runner；两个稳定工具链；含 iOS Simulator 单测）；`test`、`network-macos` 加 Swift 夹具构建 | `swift test` + `xcodebuild test`；`cargo test -p rutis-bridge --features swift,…`；`-p rutis-loader --features swift,…`；`-p rutis-host` |

## 十五、阶段

| 阶段 | 内容 | 验收 |
| --- | --- | --- |
| S1 | SDK 核心（会话、叶子 API、宏、值、错误、事件、并发）+ 形态①的 `serve`（`fd` / `unix` / `tcp`）、清单、`RutisTest`；`binary.rs` 抽取 + `go.rs` 薄封装；`Launcher::swift`、feature；`SwiftResolver`；`SwiftRuntimes`（仅 `eager`） | §十四 除按需、重启策略、监听、拨出、授权外全部通过，即：`runtime_conformance`、`session_matrix`（本地通道 + `weather.expected.json`）、`value_roundtrip`、`events_conformance`（Swift 作 X 与 Y）、`multilang`、测试工具对两份期望文件、旧宿主拒绝；多二进制并行；iOS Simulator 单测过；Go 原有测试全绿 |
| S2 | 形态①按需启动与空闲停止；`restart` 策略（`binary.rs`，Go 同时得到）；rutis-host（`runtimes.swift`、`dev`、`check`、`new --lang swift`）；指南；TCC / EventKit | 按需启动与重启策略测试过；`dev` 改代码只重启本运行时；授权提示指向二进制、拒绝错误传回 |
| A1 | 形态②：拨出与监听通道、`RutisApp`；依赖拨出稿 R-P1（已完成） | 应用（模拟器与真机）拨出接入，按 rutis 配置装载插件；`stop` 后使用者停下，`start` 后恢复；两台设备同名插件各在自己的实例；`dial_conformance`、`websocket_conformance`、`session_matrix` 的监听列 Swift 过 |

A1 只依赖 S1 的会话层，可与 S2 并行。

后续（有需求时）：下载预编译二进制（`swift add <URL>`）、接收对象引用（`objects`）、插件版本由构建生成、watchOS / visionOS、Linux。

## 十六、与总体稿及初稿的出入

| 原文 | 本文 |
| --- | --- |
| 总体稿 M3 目标"macOS 的系统能力" | 扩展到 Apple 应用托管（形态②）；通用部分见应用接入稿 |
| 总体稿"一个签名的辅助程序，用 `dlopen` 加载插件 bundle" | 修订：一个插件组一个签名二进制（附录 A） |
| 总体稿行名 `swift:Calendar.bundle` | 形态① `swift:<插件名>`；形态② `<运行时名>:<插件名>`（拨出稿） |
| 总体稿 §二.7 把 Swift 归为"按名字加载代码的" | 归为与 Go 相同的"插件就是编译好的程序"；iOS 上集成单位是应用 |
| 总体稿 §九 待定"Swift（M3）确认可重入" | 可重入，并声明 `sync-wait`（§五） |
| 初稿形态②a（应用监听，宿主拨入） | 监听保留为通道能力（S14）；应用托管推荐拨出：设备地址不稳定、证书需带 IP |
| 初稿形态②b（应用作为 peer 拨出，export / import） | 删除：改为由 rutis 驱动的运行时语义（应用接入 D2），export / import 的治理模型不再需要 |
| 初稿后台存活、APNs、App Intents | 删除：前后台属于业务层；反向调用不在范围内 |
| 初稿能力只声明 `signals`、`reentrant-sync` | 加 `sync-wait`：不声明则不在等待环检测范围内 |
| 初稿部署目标 macOS 13 | macOS 14：与 iOS 17 同为 Swift 5.9 运行时 |

## 十七、对齐 rutis 0.13（2026-10-19）

0.11–0.13 改变了运行时契约：十种值（0.11）、自动对外与错误的 `code` / `data`（0.12）、调用方的行（#441）、统一事件与插件 API 3（0.13）、拨出规则写定（#383/#384）、运行时崩溃策略（#274，0.12）。本节记录本文因此做的修订与新决定；实现以[运行时契约](../.agents/skills/rutis-new-runtime/references/runtime-contract.md)为准。

### 17.1 修订

| 2026-10-14 稿 | 现在 | 原因 |
| --- | --- | --- |
| 插件 API 2，叶子事件 `ctx.on` / `ctx.emit` 只在本进程，特性 `events` | 插件 API 3；统一事件（§4.7），能力 `events-v2`，特性 `events` 删除，对旧宿主按原文拒绝 | 0.13 事件协议；0.12 的叶子事件已删除 |
| 显式闭包表 `Shape(.async("today"), …)` + `.async("today") { … }` | `@RutisService` 宏生成方法表；`provides` 可选 | 自动对外（0.12）与开发者无感（V0） |
| `RutisClient` 闭包字段结构体、`s.method("ask")` | 普通协议 + `@RutisClient` 宏生成代理 | V0：使用方写原生协议 |
| 方法第一个参数 `RutisCtx` | 不带上下文参数，调用链在 `@TaskLocal` | V0；TaskLocal 已随 `await` 与子 Task 传递 |
| `.sync` 闭包内不能发跨进程调用 | `sync` 只是声明，实现可以 `async` | 每调用一个 Task，挂起不占线程，与 Go 行为一致，去掉唯一的差异 |
| `Codable` 经 JSON 编码；`Data`、非字符串键字典不可跨进程；整数超限报错 | 自己的 `Encoder` / `Decoder`；十种值；超界整数自动 `bigint` | 值类型设计 V1、V6、§六 |
| class 须 `Rutis.ref(v)` 显式按引用 | `@RutisService` 类型的实例按引用，不需要 `ref` | 宏已给出方法表，引用与否由类型决定（附录 D） |
| 错误名 `String(reflecting:)`，只带名字与消息 | 简名；`code` / `data` 两个方向；收到的为 `RutisError` | 0.12 错误契约；与其他语言的名字一致 |
| 会话结束清理至多 5 秒，形态① `exit(0)` | `RUTIS_SHUTDOWN_TIMEOUT`（默认 10 秒），超时以 1 退出 | 契约 §4 |
| 崩溃不自动重启 | 按 `restart`，默认 `no`，`binary.rs` 为 Go 与 Swift 实现 | #274 维护者决定（0.12） |
| SDK 首个版本 0.11.0 | 0.15.0 | 里程碑 0.15（0.14 未单独发布） |

### 17.2 仍需要 rutis 专用代码的地方

开发者无感（V0）：插件作者只写原生 Swift。Swift 没有方法的运行时反射（iOS 上也不能生成代码），下面是仍然需要的 rutis 专用写法，已压到最少：

| 地方 | 写法 | 为什么不能更少 |
| --- | --- | --- |
| 插件定义与进入点 | `Rutis.define(Plugin(...))`；形态① `Rutis.serve(...)`，形态② `RutisApp(...)` | 每种语言都有：声明依赖、提供与配置是范式本身 |
| 提供的服务类型 | 类型上一个 `@RutisService` | 没有方法反射，方法表只能在编译期生成；一个属性，类型本身仍是普通 Swift |
| 用到的服务 | 协议上一个 `@RutisClient`；`ctx.use("llm", LLMProxy.self)` | 同上；代理由宏生成，协议仍是普通 Swift，可被任何实现替代 |
| 声明 `sync` 方法（可选） | `provides: ["weather": .of(Weather.self, sync: ["unit"])]` | 只在需要 `sync` 或想让 `check` 提前知道时写；同 Go 的 `MethodsOf(Sync(...))` |
| 配置 Schema（可选） | 配置类型上一个 `@RutisConfig` | 不写时配置照常解码，只是宿主不能提前校验 |
| Swift 没有的值类型 | `Rutis.DateTime`、`Rutis.CalendarDate`、`Rutis.TimeOfDay`、`Rutis.BigInt` | V0 的唯一例外：语言没有的类型；`Date`、`Data`、`Decimal`、`UUID`、`Set`、`Dictionary` 都是原生的 |
| 事件的少量类型 | `waterfall` 的 `Rutis.Next<R>`；过滤的描述 `Rutis.Listener` | 语言没有对应的东西 |
| 收到的错误 | `RutisError`，按 `name` 匹配 | 跨进程的错误不是发出方的类型（各语言相同） |

不需要的：方法表、JSON 编码、引用包装（`ref`）、上下文参数、错误的 `code` / `data` 包装（普通存储属性即可）、同步等待与重入处理、事件登记。

### 17.3 本次的决定

| # | 决定 |
| --- | --- |
| 1 | 方法表、代理、Schema 用 Swift 宏（`@RutisService` / `@RutisClient` / `@RutisConfig`）在编译期生成；不用手写表，也不用 `Mirror` 猜方法 |
| 2 | 服务方法的线上名是基本名，按位置传参，重载即编译错误；非 `private` / `fileprivate` 的实例方法对外 |
| 3 | 方法种类是声明（默认 `async`），与 Swift 签名无关；代理一律 `async throws` |
| 4 | 调用链只在 `@TaskLocal`，插件代码不传上下文 |
| 5 | 值层是自己的 `Encoder` / `Decoder`，十种值全部实现；`Date` 截断到微秒；`Decimal` 超过 38 位失败、末尾零不保留（要保留时声明 `String`）；`Any` 参数解码为原生值 |
| 6 | `@RutisService` 实例按引用，其余 `Codable` 值（含 class）按数据；收到的函数只能声明为 `async throws` |
| 7 | 错误 `code` / `data` 从同名存储属性读取（`Mirror`）；收到的与 SDK 自己的错误都是一个类型 `RutisError`，按名字匹配 |
| 8 | 监听器一律 `kind: "sync"`，`emit` 按监听器串行排队；`bail` 在 Swift 里是 `async` API、线上同步 |
| 9 | 调用方的行：每行按 `isolate` 算自己的服务 id，标签不透明 |
| 10 | `RutisTest` 经真实会话（内存通道）运行插件，测试一方是宿主，对两份期望文件比对（同 Go 的 `rutistest`） |
| 11 | 崩溃策略 `restart` 与 node / py / bun 同值同默认（`no`），在 `binary.rs` 里为 Go、Swift 一起实现，放在 S2；空闲停止不算崩溃 |
| 12 | 拨出用 `URLSessionWebSocketTask`（升级被拒的状态码取自任务的 `response`），监听用 Network.framework；形态②的环境变量对应为 `RutisApp` 参数 |
| 13 | 不声明 `objects`（不接收对象引用），同 Go；有需求时再加 |

### 17.4 S2 的实现记录（2026-10-19，#387）

S2 按 §十、§十一 实现，另把 A1 里与形态①共用的远程连接（`listen:` / `dial:`）一并做了。实现时定下的：

| 项 | 做法 | 原因 |
| --- | --- | --- |
| 授权归属（§八） | `Rutis.serve` 开始服务前，若 macOS 认定的负责进程不是自己，用 `posix_spawn(POSIX_SPAWN_SETEXEC)` 加 `responsibility_spawnattrs_setdisclaim` 原地重新执行自己（同一 pid、同样的描述符），之后为自己负责；两个函数运行时查找，缺失时不变 | 子进程默认继承父进程（终端、宿主）的负责进程，提示会指向终端，授权也给了终端；放在 SDK 里，不论谁启动二进制都成立，宿主不用改 |
| `dev` 的构建 | `swift build --product <产品>`（debug），复制到 `.rutis/swift/<产品>-<n>` | 开发循环要快；新文件避免运行中被改写的二进制与签名不符。§十一 原写 `-c release`，发布工作流用 release |
| 模板的测试 | Swift Testing（`@Test`）+ `RutisTest` | Xcode 16 起的默认；只有 Command Line Tools 时没有测试框架（e2e 在那里只构建插件，指南写明 `swift test` 要 Xcode） |
| `check` | 插件 API 低于 3 的二进制报"用 0.13 以后的 SDK 重新构建"并失败，Go 同样 | §3.3 |
| 宿主的编译型语言 | rutis-host 的 Go 专用部分抽成 `languages/binary.rs`（`BinaryHost`、`DevBuild`），Go、Swift 各一个文件 | 注册表"一个文件加一行"对编译型语言也成立 |
| 监听的 WebSocket 层 | 升级与帧由 SDK 处理，监听用 `NWListener`（TCP / TLS） | `NWProtocolWebSocket` 拒绝时一律 400、看不到路径，而拨号方按状态码分类（401 / 403 重试，400 停止） |
| `RUTIS_KEY` | EC（P-256/384/521）或 RSA，PKCS#8 / SEC1 / PKCS#1；身份由 `SecIdentityCreate` 组成，不进钥匙串 | `SecItemImport` 不接受未加密的 PKCS#8 |

验收（§十五 S2，即总体稿 M3 的原始验收）：按需启动与重启策略（`swift_rows.rs`）；`dev` 改代码只重启本运行时（e2e `dev_loop`）；授权提示指向二进制、拒绝错误传回（e2e `tcc.rs`：负责进程是二进制自己、签名标识来自 Info.plist、未授权时 Python 调用方收到插件的错误；提示本身与授权后的情形为人工验收）。

### 17.5 A1 的实现记录（2026-10-20，#389）

A1 按 §七 实现。实现时定下的：

| 项 | 做法 | 原因 |
| --- | --- | --- |
| 远程通道只有一份实现 | 拨出循环与监听循环在 `RutisApp` 里；二进制的 `dial:`、`listen:`（§6.1）就是一个 `RutisApp`，设置取自环境变量（`RUTIS_TOKEN`、`RUTIS_CA`、`RUTIS_CERT` / `RUTIS_KEY`），SIGTERM、Ctrl-C 让它停止，因不兼容停下时以状态 1 退出 | `dial_conformance`、`session_matrix`、`swift_remote.rs` 的列同时检查应用的连接，没有两份要保持一致 |
| `start()` | 开始拨出或地址绑定后即返回；只抛出重试修不好的（不合法的 id 或地址、`wss://` 没有证书、地址被占用）；已经开始时什么也不做 | 等第一次连上会让离线时启动的应用卡住；连接的情况看 `state` |
| `stop()` | 在控制方还能收到时卸载全部行（与 SIGTERM 相同），关闭连接，完成后返回 | 应用停止、断网、被杀，rutis 侧看到的是同一件事（应用接入 §四） |
| `start()` 与 `stop()` 的先后 | 按调用顺序依次生效（2026-10-21 修订）：停止进行中调用的 `start()` 等它完成后开始；开始进行中调用的 `stop()` 撤销它；一次运行自己结束时先发布它最后的状态，再让后来的 `start()` 开始 | 应用在前台开始、进后台停止，各在自己的 `Task` 里；用户在清理期间回来时应用要连上，不能停在 `stopped`，也不能在后台继续拨出 |
| 监听的连接 | 连接上的 rutis 问候之后才是 `connected`，问候时限同拨出（10 秒）；升级请求须在 10 秒内到达（含 TLS，同 Go 的 `ReadHeaderTimeout`），同时升级的连接至多 32 个；监听器绑定后失败时按退避重新绑定，已有会话不受影响（2026-10-21 修订） | 未认证的连接不能占住应用的线程；网络切换后监听自己恢复 |
| `state` | 用手写的 `ObservationRegistrar` 遵循 `Observable`（不用 `@Observable` 宏），读写都在 SDK 的锁内；`states` 每个调用方一个 `AsyncStream`，先给出当前状态 | 状态在 SDK 自己的线程上变化：宏生成的存储没有保护；任何线程都可以读 |
| 报告行与清理时限 | 每个运行时一份（`Events.Settings`：`log`、`shutdownTimeout`），不再到处写 stderr、读 `RUTIS_SHUTDOWN_TIMEOUT` | §七 的 `log`、`shutdownTimeout`；应用的 stderr 没有人看 |
| 监听的证书 | `certificate: SecIdentity` | iOS 没有公开的办法在钥匙串之外把 PEM 私钥和证书配成身份（`SecIdentityCreate` 只在 macOS）；应用从 PKCS#12 文件或钥匙串得到 `SecIdentity`。二进制仍用 `RUTIS_CERT` / `RUTIS_KEY` |
| 监听的二进制收到 SIGTERM | 卸载全部行后以 0 退出，与拨出相同 | 底下是同一个 `RutisApp` |
| 应用里的 `ctx.project` | 应用包的路径 | 应用没有项目目录；包是它的资源所在 |
| 示例应用与它的测试 | `swift/examples/HealthApp`，一个 Xcode 项目（SDK 作本地包，Info.plist 由构建设置生成）；`rutis-e2e --test ios_app` 用 `xcodebuild` 构建，在它新建的两台模拟器上运行、连 `rutis-host run`，把"设置"切到前台让它进入后台，用 Vision 从截图读出"健康"授权页（`tests/e2e/ios/text.swift`） | 开发者打开的就是被测试的；模拟器上没有什么能点击授权页，`simctl privacy` 也没有 HealthKit（Xcode 26.5），所以拒绝与授权是人工清单（指南 §8） |
| 在哪里运行 | 只在本地：SDK 自己的测试在模拟器上运行（`tools/ios-selftest.sh`：`swift build --triple arm64-apple-ios17.0-simulator`、`simctl spawn`）与 e2e；CI 在 macOS 上跑同样的 SDK 测试（`runtimes-swift`，`swift_app.rs` 直接驱动 `RutisApp`） | 需要模拟器、设备或系统授权提示的测试不在 CI 里跑：在 runner 上会带来一堆权限问题（docs/ci.md 的本地测试） |

## 附录

### A. 为什么不用 dlopen 辅助程序（对总体稿 M3 的修订）

**A.1 Go（M4）先落地，机器已建成。** `GoResolver`、`GoRuntimes`、两段放行、按需启动、restart、`check`、`dev`、`new` 都在（#227），形态①以共用代码接入（§九）；dlopen 路线要新造 bundle 发现与装载、辅助程序与 bundle 之间的第二层 API，以及清单 / 缓存 / 平台核对的另一套答案。iOS 上 dlopen 更无从谈起。

**A.2 Swift 的跨工具链二进制兼容不可靠。** bundle 与辅助程序分开编译时，跨模块调用需要 module stability + library evolution，对"任意作者、任意 Swift 版本编出的 bundle"不可保证；绕开要把边界压成纯 C ABI，就得再造一层 API。SwiftPM 也没有 loadable-bundle 产品类型。

**A.3 TCC 粒度。** 辅助程序一个 TCC 身份：给 A 插件授权日历，同进程的 B 也能读日历。可执行文件模型里每个插件组一个窄授权，且仍满足 M3 验收。

**A.4 签名与库验证。** 开 hardened runtime 的签名辅助程序 dlopen 非同队签名的 bundle，需要 `disable-library-validation`（扩大攻击面）或所有插件作者同一团队签名。可执行文件 / 静态链入应用都无此问题。

**A.5 损失与替代。** 失去"往运行中的进程里丢一个新 bundle 就装载"。替代：部署方在 `main.swift` 里组合多个插件包为一个二进制（同 Go 附录 H.3）；或多个插件各自成二进制、按需启动、空闲即停。形态②里 = 发新版应用。

### B. Swift 事实表

| 事实 | 来源 |
| --- | --- |
| Swift 6.3 起 `@_cdecl` 转正为 `@c`（SE-0495）；本设计不需要 C ABI | [SE-0495](https://github.com/swiftlang/swift-evolution/blob/main/proposals/0495-cdecl.md) |
| macOS 10.14.4+ Swift 运行时随系统；CLI 二进制无需捆绑 runtime | [swift.org ABI stability](https://swift.org/blog/abi-stability-and-apple/) |
| 发布节奏每年 3 / 9 月 | [swift-evolution](https://github.com/swiftlang/swift-evolution/blob/main/README.md)、[platform-support](https://swift.org/platform-support/) |
| uniffi Swift 6 支持标注 partial；swift-bridge 0.x 且曾断更约 16 个月——均不采用 | [UniFFI Swift](https://mozilla.github.io/uniffi-rs/latest/swift/overview.html)、crates.io |
| 阻塞读不应落在协作线程池（池宽 = 核数）；专用线程 + continuation 是标准做法 | Swift concurrency 文档 |
| 裸可执行文件把 Info.plist 链进 `__TEXT,__info_plist`（`-sectcreate`）是长期既有技术 | CMake / swift-forums 实践 |
| `URLSessionWebSocketTask` 是 Foundation 的 WebSocket 客户端，支持子协议与自定义 TLS 信任；Network.framework 的 `NWListener` + `NWProtocolWebSocket` 是带子协议与 TLS 的 WebSocket 服务端 | Apple 文档 |
| Swift 宏（SE-0382、SE-0389、SE-0402）在编译期运行，可以给类型添加遵循与成员、给协议生成同级类型；`Mirror` 只能枚举存储属性 | swift-evolution |
| iOS：无进程 spawn、禁止下载执行代码、后台应用挂起、本地网络权限（iOS 14+） | Apple 平台文档 |

### C. 与 Go 运行时逐项对照（形态①）

| | Go | Swift |
| --- | --- | --- |
| 进程模型 | 一组插件一个二进制 | 同 |
| 自描述 | `--rutis-manifest` | 同 |
| 行名 / 运行时名 | `go:<插件>` / `go-<名>` | `swift:<插件>` / `swift-<名>` |
| 每调用执行单元 | goroutine（阻塞等待） | Task（挂起等待，不占线程） |
| 可重入 / sync-stack | 可重入 / 不声明 | 同 |
| sync-wait | 声明 | 同 |
| 调用链 | `context.Context`（显式传） | `@TaskLocal`（自动传） |
| 方法枚举 | 反射 `MethodsOf` | `@RutisService` 宏 |
| 使用服务 | 函数字段结构体 | `@RutisClient` 协议 |
| Schema | 反射生成，可覆盖 | `@RutisConfig` 宏生成，可覆盖 |
| 值默认 | 数据，`rutis.Ref` 显式引用 | `Codable` 即数据；`@RutisService` 实例即引用 |
| sync 方法内发调用 | 可以（阻塞 goroutine） | 可以（挂起 Task） |
| 监听器种类 | `sync` | `sync` |
| panic / fatalError | 捕获为错误；goroutine 未捕获 → 进程退出 | 错误即 `throws`；`fatalError` → 进程退出 |
| 崩溃后 | `restart` 策略（`binary.rs`） | 同 |
| 测试工具 | `rutistest`（真实会话） | `RutisTest`（真实会话） |
| 远程 | 监听与拨出 | 同 |
| 平台 | Linux / macOS / Windows | macOS（①）/ macOS + iOS（②） |
| 分发 | git tag `go/rutis/vX.Y.Z` + 模块代理 | 根 `Package.swift` + 主发布线 `v` tag |

### D. 值分界为什么不需要 `Ref` 那样的规则

Go 需要显式 `Ref` 是因为 Go 结构体既可以是数据也可以是对象，语言不分。Swift 里，一个类型有没有对外的方法表由 `@RutisService` 决定，而它本来就要写在提供的服务类型上：有方法表的实例按引用，其余 `Codable` 值（struct、enum，以及 `Codable` 的 class）按数据。两者都不是的值不可跨进程，错误点名类型。

### E. 本机 PoC 实测（2026-10-12，Swift 6.3.2 / Xcode）

**E.1**：根 `Package.swift` 声明子目录路径的目标（`path: "swift/rutis/Sources/Rutis"`）构建、运行正常。

**E.2 SwiftPM 版本 tag 规则**：对 git 依赖实测——`1.0.0` 与 `v1.1.0` 被接受为版本；`rutis-swift-1.0.0`、`swift/rutis/1.0.0`、`rutis-mono-1.2.0` 均被忽略。结论：主发布 `v` tag 直接可用作 SwiftPM 版本。

**E.3 参数包上下文推断**：`func method<each A, R>(_:) -> (repeat each A) async throws -> R` 按目标闭包类型完成推断并编译通过。现在用于监听器（`ctx.on`）与收到的函数引用的类型化解码；方法表与代理由宏生成，不再依赖它。
