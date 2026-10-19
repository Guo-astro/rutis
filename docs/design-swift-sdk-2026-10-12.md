# Swift SDK：macOS 插件运行时与 Apple 应用托管（设计稿）

[English](design-swift-sdk-2026-10-12.en.md)

状态：定稿，未实现。日期：2026-10-12，2026-10-14 定稿修订（应用部分改为按 [应用接入](design-app-integration-2026-10-14.md)，远程连接支持监听与拨出，见 §十六）。基准：`main` `a763cdc`（0.10.0）。
依据：[多语言总体稿](design-multilang-runtimes-2026-10-03.md)（本文是其 M3，对其 dlopen 方案的修订见附录 A）、[应用接入](design-app-integration-2026-10-14.md)（形态②的通用部分）、[远程运行时：拨出与动态接入](design-remote-runtime-dial-2026-10-14.md)（连接）、[Go 插件运行时](design-go-runtime-2026-10-10.md)（形态①沿用其决策与机器，对照见附录 C）、[插件 API](guide/plugin-api.md)。

范围外：Swift 侧 Cordis；SwiftPM 之外的编译系统；宿主运行时编译 Swift 代码；Linux 与 Windows（§十三）；应用的前后台、后台存活、推送唤醒（业务层，见应用接入 §七）；App Intents 等 Apple 生态反向调用 rutis；在设备上运行 rutis 本体。

## 一、决策

| # | 决策 | 章节 |
| --- | --- | --- |
| S1 | 两种托管形态共用一个 SDK：① macOS 插件二进制（本地多进程）；② 应用（iOS / iPadOS / macOS 应用内托管插件） | 二 |
| S2 | 形态①：一个二进制一个运行时进程，含一个或多个插件，由 `main.swift` 的 `Rutis.serve(...)` 决定；宿主可同时运行多个 Swift 运行时 | 三 |
| S3 | 形态①行名 `swift:<插件>`，`SwiftResolver` 按清单路由；限定写法 `<运行时名>:<插件>`；运行时名由文件名得出（前缀 `swift-`） | 三 |
| S4 | 形态①二进制自描述：`--rutis-manifest` 输出清单，解析不需要启动进程；运行期间以启动清单为准，换文件在重启后生效 | 三 |
| S5 | 形态①按需启动、空闲停止；`eager` 全部启动且不停止 | 三 |
| S6 | 形态②：插件编进应用，按 rutis 范式编写、由 rutis 驱动；SDK 对应用只提供 `start` / `stop`；连接为应用拨出 | 七 |
| S7 | 同一进程内调用不走 IPC；跨进程、跨语言经 rutis 按名字转发 | 四 |
| S8 | 每个进来的调用一个 Task；声明 `sync-wait` 与 `reentrant-sync`，不声明 `sync-stack`。适用于两种形态 | 五 |
| S9 | 调用链放 `@TaskLocal`；等待挂起当前 Task，不阻塞线程；方法默认 `async`，`.sync` 标记；调用对端 `sync` 方法时帧带 `sync: true` | 五 |
| S10 | 叶子插件；插件 API 版本 2（含叶子事件 `ctx.on` / `ctx.emit`）；配置 `Codable` 解码，Schema 手写在配置类型上 | 四 |
| S11 | 提供服务：显式闭包表（Swift 无方法反射） | 四 |
| S12 | 使用服务：`RutisClient` 闭包字段结构体，参数包推断签名；推断不稳的签名回退到 0–3 参数重载 | 四 |
| S13 | 值按类型系统分界：值类型按数据复制，闭包按引用；对象须 `Rutis.ref` 显式按引用 | 四 |
| S14 | 远程连接两个方向都支持：拨出（`dial:`）与监听（`listen:`）；应用托管推荐拨出 | 六 |
| S15 | 宿主不编译；`run` 只运行现成二进制，`dev` 只重建项目自身；不同 SDK 版本的二进制可共存 | 十、十一 |
| S16 | SDK 位于 `swift/rutis`，经仓库根 `Package.swift` 分发，版本跟主发布线的 `v` tag；核心只依赖 Foundation | 十二 |
| S17 | 平台：macOS 14+（两种形态）、iOS / iPadOS 17+（形态②）；Linux 后续，Windows 不做 | 十三 |

## 二、两种托管形态

| | ① macOS 插件二进制 | ② 应用 |
| --- | --- | --- |
| 用途 | Mac 系统能力（EventKit、通讯录、辅助功能、屏幕录制、Core ML）、AppleScript / AX 自动化 | 应用把自身能力与所在设备的能力作为 rutis 插件提供 |
| 谁拉起 | rutis 宿主按需启动进程 | 应用自己；业务层决定何时 `start` / `stop` |
| 插件从哪来 | 二进制里编进的插件，清单自描述 | 应用里编进的插件，随应用发布 |
| 连接 | 本地通道（`fd:3` / `unix:` / `tcp:`）；远程时拨出 | 拨出到 rutis 的接入点 |
| 授权（TCC / entitlements） | 记在插件二进制 | 记在应用 |
| 机器 | `SwiftResolver` / `SwiftRuntimes`（与 Go 共用） | [拨出与动态接入](design-remote-runtime-dial-2026-10-14.md)（每条连接一个实例） |

两种形态的插件代码相同：同一个插件定义既可以 `serve` 成二进制，也可以交给应用托管。§四至§六 对两种形态一致。

## 三、形态①：macOS 插件运行时

### 3.1 运行时模型

```text
rutis 宿主
 ├─ LocalRuntime::node / ::python ...
 ├─ SwiftRuntimes（SwiftResolver + 生命周期）
 │    ├─ plugins/swift/weather ── swift-weather ── weather
 │    ├─ plugins/swift/kits   ── swift-kits    ── calendar、contacts
 │    └─ plugins/swift/ml     ── 未启动（无行使用）
 └─ LoaderPlugin + 每个运行中的运行时一个 RuntimeRowsPlugin
```

- 与 Go 运行时相同：行依赖 `RuntimeRows#<运行时名>` 与插件 `inject` 的全部名字（叶子运行时，rutis 门控全部名字）；插件 `provides` 的服务从行的 fiber 发布到 `host_key(name)`；进程退出时其全部行停止，使用其服务的行随之停止。
- `isolate`、实例标签、服务 id、导出句柄：与 Python / Go 运行时相同。
- TCC 记在插件所在的二进制上（§八）。

### 3.2 二进制与清单

- 插件作者分发：Swift 包（导出插件定义）+ `cmd/<名字>/main.swift`（`serve` 自己的插件），或一个包里若干插件编成一个二进制。
- 宿主接受任何响应 `--rutis-manifest` 的可执行文件（来源见附录 A.5）。

```swift
// example.com/kits/cmd/kits/main.swift
import Rutis
import WeatherKits

Rutis.serve(WeatherKits.calendar, WeatherKits.contacts)
```

| `serve` 规则 | |
| --- | --- |
| 参数 | `--rutis-manifest`：打印清单后退出；否则 `<channel> [--id <endpoint>] [--peer <endpoint>] <project>` |
| 插件同名 | 启动即退出，指明两个定义 |
| 登记方式 | 只在 `serve` 的参数里，不用静态初始化的注册表 |
| 子命令 | `Rutis.serveArgs(args, defs...)`；另有 `Rutis.serveConn(channel, defs...)`（内存会话，测试用） |

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

| 项 | 规则 |
| --- | --- |
| 输出 | stdout，退出码 0；不连通道，不执行 `apply` |
| 插件条目 | 与 `rows.schema` 回复相同，同一段代码生成 |
| `runtime` | SDK 标记串 `rutis-swift-runtime:1`：目录扫描只执行含它的文件 |
| `platform` | `macos/<arch>`（通用二进制报 `macos/universal`）；宿主核对不符报"不是为 <os>/<arch> 构建的" |
| `sdk` | 常量 `Rutis.sdkVersion` |
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
| 解析结果 | 叶子结果，与 `GoResolver` 共用"由声明构造 `Resolved`"的代码 |
| meta | `{ source: "swift", binary, runtime, version, sdk }` |
| 运行中换文件 | 以启动清单解析；新二进制才有的插件报"已更换，`restart("<运行时名>")` 后生效" |
| 第二段放行 | `RuntimeRowsPlugin` 已接受 trait（Go G1 改造），`SwiftResolver` 按运行时实现同一 trait |

与 `GoResolver` 的差异只有三处常量（标记串、前缀、feature 名），实现走共用代码（§九）。

### 3.4 `SwiftRuntimes`

与 `GoRuntimes` 同构，位于 rutis-loader，挂在 loader 之后。

```rust
let swift = Arc::new(SwiftResolver::new(SwiftBinaries::new().dir("plugins/swift").file("bin/weather")).with_catalog(&catalog));
chain = chain.with_shared(swift.clone());
root.plugin(SwiftRuntimes::new(swift, project).idle(Duration::from_secs(60)));
```

| 项 | 规则 |
| --- | --- |
| 结构 | 每个运行中的运行时 = 一个子 `Ctx`：`LocalRuntime::swift(binary, project).named(名)` + `RuntimeRowsPlugin`；停止 = 释放子 `Ctx` |
| 启动 | 解析出一行且其运行时未运行时启动（另有周期检查补足）；多个冷启动并行 |
| 空闲停止 | 最后一行卸载后等 `idle`（默认 60 秒），无未禁用条目解析到它则停止；同一时刻启动 / 重启 / 停止只一个在进行 |
| 崩溃 | 行停止，记录原因，不自动重启；重启条件：`SwiftRuntimes::restart(名)`，或二进制文件已变化 |
| `eager()` / `runtimes()` | 与 Go 相同 |
| 单个固定二进制 | 可不用 resolver：`LocalRuntime::swift` + `RuntimeResolver::modules` |

## 四、Swift SDK（共享核心）

位于 `swift/rutis`（SwiftPM 目标 `Rutis`），Swift 5 语言模式，工具链 5.9 及以上，部署目标 macOS 14 / iOS 17。分层：

| 层 | 内容 | 平台 |
| --- | --- | --- |
| 核心 | 会话协议（compat / endpoint）、叶子 API、值、并发、`RutisTest` | macOS + iOS（只依赖 Foundation；不触碰进程与文件系统设施） |
| 通道·进程 | `fd:3`、`unix:`、`tcp:`（POSIX） | macOS（形态①） |
| 通道·拨出 | WebSocket 客户端（Foundation `URLSessionWebSocketTask`），按 [拨出稿](design-remote-runtime-dial-2026-10-14.md) §六 的规范重连 | macOS + iOS |
| 通道·监听 | WebSocket 服务端（Network.framework `NWListener` + `NWProtocolWebSocket`），语义同 Go 的远程监听（一次一个控制方、新连接接管） | macOS + iOS |
| serve 打包 | `Rutis.serve` / `serveArgs` / 清单 CLI | macOS（形态①） |
| 应用托管 | `RutisApp`（§七） | macOS + iOS（形态②） |

### 4.1 插件

```swift
import Rutis

struct Config: RutisConfig {          // 配置类型：Schema 与解码都在类型上
    static let schema = Schema.object(
        properties: ["city": .string(description: "要查天气的城市")],
        required: ["city"])
    var city: String                   // Codable 合成
}

struct LLM: RutisClient {             // 使用服务：闭包字段结构体
    var ask: (RutisCtx, String) async throws -> String
    var listModels: ((RutisCtx) async throws -> [String])?   // 只声明用到的方法
    init(_ s: RutisService) {         // 从代理构造；参数包按字段类型推断
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
    provides: ["weather": Shape(.async("today"), .sync("unit"))],   // 声明形状（进清单）
    config: Config.self,
    apply: { ctx, config in                                          // 实现（装载时）
        let llm = ctx.use("llm", LLM.self)
        let weather = Weather(llm: llm, city: config.city)
        ctx.provide("weather", [
            .async("today") { try await weather.today($0, $1) },
            .sync("unit") { _ in "celsius" },
        ])
        ctx.effect { print("weather unloaded") }
    }))
```

| API | 规则 |
| --- | --- |
| `Rutis.define` | 校验声明（名称、闭包表与形状一致），返回可 serve、可交给应用托管的定义 |
| `apply` 错误 | 装载失败；已登记的清理照常运行 |
| `ctx.use(name, C.self)` | `C: RutisClient`；`name` 须在 `inject` 中；同进程提供者直接给本地实现 |
| `ctx.provide(name, methods)` | 闭包表须与 `provides` 声明的形状一致；至卸载或返回的撤销函数为止 |
| `ctx.effect(cleanup)` | `cleanup` 可 `async`，卸载时逆序执行 |
| `ctx.on(name, listener)` | 监听本运行时进程事件总线上的事件（宿主经 `emits` 发入的，及同进程插件 `ctx.emit` 的）；随插件卸载注销，返回的函数立即注销；监听器带调用链、随插件卸载取消 |
| `ctx.emit(name, args...)` | 向本进程事件总线分发，按注册顺序同步调用监听器，遇首个错误返回；不跨进程。`internal/*` 不可用 |
| `ctx.spawn { ... }` | 子 Task 中运行，捕获错误写 stderr；卸载时取消并等待 |
| `RutisCtx` | 行作用域：取消（卸载时取消行 Task 树）、`project`、调用链（§五） |
| `Rutis.NoConfig` | 无配置 |

### 4.2 提供服务：闭包表

Swift 没有方法反射（Mirror 不能枚举类型的方法，也不能合成任意签名的方法），所以不像 Go 用 `MethodsOf` 反射、Python 用 `inspect`，而是**显式闭包表**：

- 声明（define 时）：`Shape(.async("today"), .sync("unit"))` → 清单与 `rows.schema`。
- 实现（apply 时）：`.async("today") { (ctx, city) in ... }`，闭包签名任意（参数按位置解码进闭包参数类型，返回值按 §4.5 编码）；SDK 校验表与形状一致。
- 同进程调用：直接调用闭包，不编码、不跨进程（S7）。
- 后续可选：`@RutisService` 宏从类型生成形状与闭包表。

### 4.3 使用服务：`RutisClient`

```swift
protocol RutisClient { init(_ service: RutisService) }
```

- 结构体的闭包字段即要用的方法：字段名即方法名；只写用到的方法，每个字段须对应服务声明的方法，否则构造时把错误抛给 `use`。
- `s.method("ask")` 是参数包泛型：`func method<each A, R>(_ name: String) -> (RutisCtx, repeat each A) async throws -> R`，从字段类型推断闭包签名（本机 Swift 6.3 实测可行，附录 E.3）；推断不稳的签名用 0–3 参数的重载集。
- 跨进程代理一律 `async`：没有阻塞式的 `callSync`。调用对端声明为 `sync` 的方法时，帧带 `sync: true`，当前 Task 挂起等待（§五）。

### 4.4 配置

| 项 | 规则 |
| --- | --- |
| 解码 | `JSONDecoder`，`Config: Codable`；失败即装载失败 |
| Schema | `RutisConfig.schema` 手写（Swift 反射产不了 Schema，与 Python 的手写 `Config` 一致） |
| 值映射 | JSON Schema 语义与 Go §6.3 相同 |

### 4.5 值

| Swift 值 | 跨进程 |
| --- | --- |
| `nil`、`Bool`、`Int`（±2^53−1 内）、`Double`、`String` | 复制；整数超限报错 |
| `Optional`、`Array`、`Dictionary<String, T>`、`Codable` 的 struct / enum | 递归复制，按 `Codable` 编码 |
| 闭包（函数引用） | 引用；参数与返回值按本表编码 |
| `Rutis.ref(v)` | 引用；`v: AnyObject & RutisExportable`（自带闭包表，见 §4.2），远端得到调用其方法的代理 |
| class 实例 | 不显式 `ref` 不可跨进程：报错并提示改用 struct 或 `Rutis.ref` |
| `Error` | 复制名字与消息；名字 = 错误类型名（`String(reflecting:)`），可经 `RutisErrorName` 覆盖 |
| `RutisFuture` | 异步结果；`await fut.value()` |
| `Data`、非字符串键字典、循环引用、非 `Codable` | 不可跨进程（同 Go 的 `[]byte` 等） |

- 分界来自类型系统本身（附录 D）。
- 接收：函数类型 → 调用远端函数的闭包；其余按 `Codable` 解码进声明类型。不接收对象引用（同 Go）。

## 五、并发、调用链、取消

适用于两种形态。

| 项 | 规则 |
| --- | --- |
| 会话结构 | 一个读线程（阻塞读通道，帧经 `AsyncStream` 进 async 世界；阻塞读不落在协作线程池上）；写帧一个 actor 串行；每个进来的 `call` / `invoke` / `get` / `await` / 控制操作一个新 Task；行状态各自加锁 |
| 可重入 | 任何时候执行任何进来的调用；声明 `reentrant-sync`。不声明 `sync-stack`：调用并发跑在不同 Task 上，无单栈顺序（与 Go 同类，见总体稿 §九） |
| 同步等待 | 调用对端 `sync` 方法时帧带 `sync: true`，当前 Task 挂起在 continuation 上等回复——**不阻塞任何线程**；声明 `sync-wait`，参与宿主的等待环检测（#228）。等待期间其他 Task 照常执行进来的调用 |
| 插件要求 | 服务闭包并发安全；调用其他服务时不持锁（写进指南） |
| 调用链 | `@TaskLocal var path: [String]`：进来的调用在其 Task 里设 `path + [id]`；发出的调用从当前 TaskLocal 取链；`apply` / 清理带 `rows.load` / `rows.unload` 的链 |
| `sync` 方法 | 闭包表 `.sync` 的闭包内不得发出跨进程调用（代理全部 async，编译期就调不了）；与 Go 的唯一行为差异，写进指南 |
| 进来的取消 | `cancel` 帧 → 该调用的 `Task.cancel()`；`Task.checkCancellation()` 抛 `CancellationError` |
| 发出的取消 | `async`：发 `cancel`，立即抛 `CancellationError`，丢弃迟到结果；`sync`：立即抛，丢弃迟到结果，对端照常执行 |
| 行卸载 | 取消该行 Task 树；撤销服务后等待进行中的调用（时限由 rutis 侧决定） |
| `SyncWaitCycle` | 自身不产生；收到时为 `Rutis.RemoteError`，`isSyncWaitCycle(err)` |

## 六、通道与契约

### 6.1 通道

| channel | 形态 | 实现 |
| --- | --- | --- |
| `fd:3` | ① | Unix 本地继承：`dup` + POSIX read/write |
| `unix:<path>` | ① | 拨回：POSIX socket |
| `tcp:127.0.0.1:<port>` | ①（预留） | 先发 `RUTIS_CHANNEL_TOKEN` + 换行，随即从环境删除 |
| `dial:wss://…` | ①远程、②（推荐） | WebSocket 客户端，子协议 `rutis.3`；凭据与 id 按拨出稿 §五；重连按拨出稿 §六 |
| `listen:ws://…` / `listen:wss://…` | ①远程、② | WebSocket 服务端；非回环必须 `wss://`（`RUTIS_CERT` / `RUTIS_KEY` 语义）；凭据、心跳、消息上限同 Go 的远程监听 |

- 按行分帧；单条消息长度上限同 [#173](https://github.com/arcships/rutis/issues/173)；`project` 参数经 `Rutis.project` 提供（形态②中为应用的约定目录）。
- 非回环地址必须 `wss://`；自签证书经 `RUTIS_CA` 语义（SDK API 参数）信任。

### 6.2 契约

compat 协议版本 2（本地通道）、endpoint 协议 3（远程，监听与拨出）；特性 `["rows.v2", "hosts", "leaf", "scopes", "events"]`；能力 `signals`、`sync-wait`、`reentrant-sync`，endpoint 握手另声明 `runtime`；会话内调用号 `node:` 前缀（与其他叶子运行时相同）。

| 控制操作 | 行为 |
| --- | --- |
| `mount` | `{ services: {}, features, plugins, implementation: { name: "github.com/arcships/rutis", version: Rutis.sdkVersion }, engine: { name: "swift", version: <构建时嵌入的工具链版本> } }`；`plugins` 与清单的 `plugins` 相同（拨出稿 §四） |
| `rows.schema(entry)` / `rows.load` / `rows.update` / `rows.unload` | 与 Go §8.2 相同；`rows.load` 忽略 `inject`（门控由 rutis 做） |
| `hosts.provide` / `hosts.withdraw`、`release` / `get`、`dispose` | 与 Go 相同 |

### 6.3 退出与清理

| 情况 | 规则 |
| --- | --- |
| `apply`、方法、清理抛出的错误 | 捕获为错误回复 |
| `fatalError` / `try!` / 强解包崩溃、`ctx.spawn` 里未捕获的错误 | Swift 不可捕获 → 形态①进程退出、行全部停止（与 Go 对齐）；形态②是应用崩溃：连接断开，rutis 侧实例关闭、服务撤销 |
| 会话已结束 | 卸载全部行，清理合计至多 5 秒；超时放弃余下清理，stderr 记录涉及的行；形态① `exit(0)`，形态② 回到未连接状态（应用继续活着） |
| `rows.unload`、`dispose` 时限 | 不设时限（同 Go / Python），时限由 rutis 侧决定 |

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
| `start()` | 连接、问候、报 `plugins`；断线后按拨出稿 §六 自动重连，直到 `stop()` |
| `stop()` | 卸载全部行（清理逆序），关闭连接；可再次 `start()` |
| 状态 | `rutis.state`（`stopped` / `connecting` / `connected`）可观察，供业务层显示；SDK 不据此做任何生命周期决定 |
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
| 把 `go.rs` 中与语言无关的部分抽成共用模块：清单执行与缓存、目录扫描与标记串嗅探、运行时名推导、按需生命周期与 restart、文件替换语义；`go.rs` 改为薄封装，行为不变，现有测试保护 | `rutis-loader/src/runtime/binary.rs`（新） |
| `SwiftResolver`、`SwiftBinaries`、`SwiftRuntimes` | `rutis-loader/src/runtime/swift.rs`（新，薄封装） |
| `swift_runtime_name`；`Launcher::swift`（与 `Launcher::go` 同一条 spawn 路径）；`LocalRuntime::swift` | `rutis-bridge/src/runtime/{local,process}.rs` |
| feature：rutis-bridge `swift = []`；rutis-loader `swift = ["runtimes", "rutis-bridge/swift"]`；rutis-host 依赖开 `swift` | 三个 `Cargo.toml` |
| 远程 npm 命名拒绝 `swift:` 与 `<swift 运行时名>:` 前缀 | 已实现，仅加测试 |
| 形态②：`runtimes.remote[].language` 接受 `"swift"`；其余见拨出稿 §七 | `rutis-host/src/host.rs` |
| `scripts/train.mjs` 版本正则加 `swift/rutis/Sources/Rutis/Version.swift` | `scripts/train.mjs` |

`go.rs` 抽取若耦合过深，`swift.rs` 先照抄、随后统一，出入随实现记录在本文。

实现记录（0.14，抽取已完成）：`binary.rs` 提供 `BinaryLanguage` trait（`PREFIX`、`TITLE`、`MARKER`、`PLUGIN_API` 常量，`runtime_name`、`launch` 两个函数）和以它为参数的 `Binaries`、`BinaryManifest`、`BinaryResolver<L>`、`BinaryRuntimes<L>`、`BinaryRuntimesHandle<L>`、`BinaryRuntimeState`、`BinaryRuntimeInfo`、`read_binary_manifest`，feature `binary`（`go` 开启它，`swift` 也应开启）。`go.rs` 只剩 `struct Go` 的实现和 `GoResolver = BinaryResolver<Go>` 等类型别名，Go 用户的代码不用改。`swift.rs` 照此写：`struct Swift` 实现 `BinaryLanguage`（`swift_runtime_name`、`LocalRuntime::swift`）加别名。崩溃重启也在共用模块里：`BinaryRuntimesHandle::supervise(RestartPolicy, 回调)`，退避用 `rutis_loader::backoff`（与 rutis-host 的 node/py/bun 同一份）；rutis-host 的 `runtimes.go.restart` 经它实现，`runtimes.swift.restart` 照搬即可。§3.4 "崩溃不自动重启"因此只是默认值。


## 十、rutis-host

```json
{
  "runtimes": { "swift": { "dir": "plugins/swift", "binaries": ["bin/weather"], "start": "on-demand", "idle": 60 } },
  "rows": [{ "id": "weather", "name": "swift:weather", "config": { "city": "Oslo" } }]
}
```

字段与 `runtimes.go` 相同（`dir` / `binaries` 至少一个；`start`：`on-demand`（默认）/ `eager`；`idle` 秒；另有 `project`）。形态②的配置见拨出稿 §四。

| 命令 | 行为 | 阶段 |
| --- | --- | --- |
| `check` | 每个二进制：运行时名、平台、SDK / 插件 API 版本、插件；歧义；每行的目标二进制；失败的候选文件；隔离属性 | S2 |
| `dev` | 无 `rutis.dev.json` 时据 `Package.swift` 判定 Swift 项目；重建与重启见 §十一 | S2 |
| `new <name> --lang swift` | 插件 target、`cmd/<name>/main.swift`、含 usage description 的 `Info.plist` 与 `linkerSettings`、测试、`rutis.dev.json`、发布工作流（发布产物为通用二进制） | S2 |
| `run` | 不需要 Swift 工具链 | — |

## 十一、换代码（形态①）

与 Go §十二 相同：`dev` 监视 `.swift`、`Package.swift`、`Package.resolved` → `swift build -c release` 到缓存目录新文件 `<name>-<序号>` → 成功替换该运行时的文件并重启它；失败打印错误，旧进程继续。部署：改名替换 → `SwiftRuntimes::restart("<运行时名>")`。

## 十二、仓库与发布

| 项 | 规则 |
| --- | --- |
| 位置 | `swift/rutis`：目标 `Rutis`（核心 + API）、`RutisTest`；会话层 `Sources/Rutis/Internal/Peer`；conformance 可执行目标 `RutisConformance` |
| 清单 | **仓库根** `Package.swift`，目标用 `path:` 指向 `swift/rutis/Sources/...`；平台 `.macOS(.v14)`、`.iOS(.v17)`。SwiftPM git 依赖只认仓库根（附录 E.1）；仓库打包体积约 18 MiB（2026-10-14），依赖方克隆代价可接受 |
| 版本 | `Rutis.sdkVersion` 跟主发布线：现有 `v0.x.y` tag 正是 SwiftPM 认的形式（附录 E.2），无需新 tag 命名空间；`from:` 从首个含根 `Package.swift` 的 release 起 |
| 依赖方写法 | 插件作者与应用同一条：`.package(url: "https://github.com/arcships/rutis", from: "0.11.0")` + `.product(name: "Rutis", package: "rutis")`；应用在 Xcode 里直接添加包 |
| 依赖 | 核心仅 Foundation；拨出用 `URLSessionWebSocketTask`，监听用 Network.framework；不引入 uniffi / swift-bridge（附录 B） |
| 兼容 | 同一协议版本内，任意 SDK 版本的二进制 / 应用可在任意宿主版本运行，前提是插件 API 不高于宿主支持 |
| 语言模式 | Swift 5 模式起步，API 按 Sendable 友好设计；工具链策略同 Go："当前两个稳定版"（今日为 6.3、6.4） |
| 文档 | `docs/guide/swift-plugin.md`（形态①）、`docs/guide/swift-app.md`（形态②，含本地网络权限等接入注意事项），中英；`docs/guide/plugin-api.md` 加 Swift 列 |

## 十三、平台

| 平台 | 形态① | 形态② | 说明 |
| --- | --- | --- | --- |
| macOS 14+ | ✅ | ✅（Mac 应用） | Swift 5.9 运行时随系统，参数包无部署顺延问题 |
| iOS / iPadOS 17+ | — | ✅ | 无进程 spawn、无动态装载 |
| watchOS / visionOS | — | 后续 | 有需求时接入 |
| Linux | 后续 | — | 有真实需求再接 |
| Windows | 不做 | — | |

## 十四、测试

| 测试 | 位置 | 内容 |
| --- | --- | --- |
| 会话层单元 | `swift/rutis/Tests` | 编解码、引用计数、同步中回调、取消、TaskLocal 取链、并发、挂起等待不阻塞 |
| iOS 核心单测 | 同上，CI 加 `xcodebuild test -destination 'platform=iOS Simulator'` | 核心层在 iOS 上编译并测试（值、叶子 API、内存会话、`RutisApp` 状态机） |
| 会话一致性 | `rutis-bridge/tests/runtime_conformance.rs` + `RutisConformance` 目标 | `session::testing::session` |
| 契约 × 通道 | `session_matrix.rs` 加 `Swift` | `fd:3`、拨回 |
| Swift 专项 | `rutis-bridge/tests/swift_runtime.rs` | 取消到 Task；错误形状；`fatalError` 退出撤回服务；引用与 release |
| 清单 / 多二进制 / 按需 | `rutis-loader/tests/swift_rows.rs` | 与 `go_rows.rs` 同构全套 |
| 运行时一致性 | `multilang.rs`（feature `swift`） | Swift 叶子插件；多语言冷启动互调；提供者移除只停使用者；`inject` 门控 |
| 交叉同步调用 | `multilang.rs` | Node→Swift→回调 Node（带链）、Swift↔Go、Swift↔Python：不卡死；Node 与 Swift 的交叉同步环被检测为 `SyncWaitCycle` |
| 监听 | `rutis-bridge/tests/websocket_conformance.rs` 加 Swift 远程端 | 与 Go 的远程监听同一套断言 |
| 拨出 | `rutis-bridge/tests/dial_conformance.rs` 加 Swift 列（拨出稿 §六） | 退避、凭据被拒、同 id 接管、断开即关闭实例、`stop` / `start` |
| 授权（e2e） | e2e 套件 | 形态① EventKit；形态② iOS HealthKit：提示归属、拒绝后错误传回。授权点击为人工验收清单，其余自动化 |
| 夹具 | `crates/rutis-loader/tests/fixtures/swift` | 测试开始时构建成至少两个二进制 |
| CI | `runtimes-swift`（macOS runner；两个稳定工具链；含 iOS Simulator 单测）；`test`、`network-macos` 加 Swift 夹具构建 | `swift test` + `xcodebuild test`；`cargo test -p rutis-bridge --features swift,…`；`-p rutis-loader --features swift,…`；`-p rutis-host` |

## 十五、阶段

| 阶段 | 内容 | 验收 |
| --- | --- | --- |
| S1 | SDK 核心（会话、叶子 API、值、并发）+ 形态①的 `serve`（`fd` / `unix`）、清单、`RutisTest`；`binary.rs` 抽取 + `go.rs` 薄封装；`Launcher::swift`、feature；`SwiftResolver`；`SwiftRuntimes`（仅 `eager`） | §十四 除按需、拨出、授权外全部通过；多二进制并行；iOS Simulator 单测过；Go 原有测试全绿 |
| S2 | 形态①按需启动与空闲停止；rutis-host（`runtimes.swift`、`dev`、`check`、`new --lang swift`）；指南；TCC / EventKit | 按需启动测试过；`dev` 改代码只重启本运行时；授权提示指向二进制、拒绝错误传回 |
| A1 | 形态②：拨出与监听通道、`RutisApp`；依赖拨出稿 R-P1 | 应用（模拟器与真机）拨出接入，按 rutis 配置装载插件；`stop` 后使用者停下，`start` 后恢复；两台设备同名插件各在自己的实例；`dial_conformance`、`websocket_conformance` Swift 列过 |

A1 只依赖 S1 的会话层与拨出稿 R-P1，可与 S2 并行。

后续（有需求时）：下载预编译二进制（`swift add <URL>`）、接收对象引用、`@RutisService` / Schema 宏、插件版本由构建生成、watchOS / visionOS、Linux。

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

## 附录

### A. 为什么不用 dlopen 辅助程序（对总体稿 M3 的修订）

**A.1 Go（M4）先落地，机器已建成。** `GoResolver`、`GoRuntimes`、两段放行、按需启动、restart、`check`、`dev`、`new` 都在（#227），形态①以共用代码接入（§九）；dlopen 路线要新造 bundle 发现与装载、辅助程序与 bundle 之间的第二层 API，以及清单 / 缓存 / 平台核对的另一套答案。iOS 上 dlopen 更无从谈起。

**A.2 Swift 的跨工具链二进制兼容不可靠。** bundle 与辅助程序分开编译时，跨模块调用需要 module stability + library evolution，对"任意作者、任意 Swift 版本编出的 bundle"不可保证；绕开要把边界压成纯 C ABI，就得再造一层 API。SwiftPM 也没有 loadable-bundle 产品类型。

**A.3 TCC 粒度。** 辅助程序一个 TCC 身份：给 A 插件授权日历，同进程的 B 也能读日历。可执行文件模型里每个插件组一个窄授权，且仍满足 M3 验收。

**A.4 签名与库验证。** 开 hardened runtime 的签名辅助程序 dlopen 非同队签名的 bundle，需要 `disable-library-validation`（扩大攻击面）或所有插件作者同一团队签名。可执行文件 / 静态链入应用都无此问题。

**A.5 损失与替代。** 失去"往运行中的进程里丢一个新 bundle 就装载"。替代：部署方在 `cmd/main.swift` 里组合多个插件包为一个二进制（同 Go 附录 H.3）；或多个插件各自成二进制、按需启动、空闲即停。形态②里 = 发新版应用。

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
| 调用链 | `context.Context` | `@TaskLocal` |
| 方法枚举 | 反射 `MethodsOf` | 显式闭包表 |
| Schema | 反射生成，可覆盖 | 手写 |
| 值默认 | 数据，`rutis.Ref` 显式引用 | 值类型即数据；class 须 `Rutis.ref` |
| sync 方法内发调用 | 可以（阻塞 goroutine） | 不可以（代理全 async） |
| panic / fatalError | 捕获为错误；goroutine 未捕获 → 进程退出 | 错误即 `throws`；`fatalError` → 进程退出 |
| 远程 | 监听（`listen:`）；拨出随拨出稿 R-P3 | 监听与拨出 |
| 平台 | Linux / macOS / Windows | macOS（①）/ macOS + iOS（②） |
| 分发 | git tag `go/rutis/vX.Y.Z` + 模块代理 | 根 `Package.swift` + 主发布线 `v` tag |

### D. 值分界为什么不需要 `Ref` 那样的规则

Go 需要显式 `Ref` 是因为 Go 结构体既可以是数据也可以是对象，语言不分。Swift 的值类型（struct / enum）与引用类型（class）在类型系统里就是两个世界：struct 按数据复制；class 要跨进程必须显式 `Rutis.ref` 并自带方法表（Swift 无法反射枚举其方法，与 §4.2 的闭包表同一机制）。

### E. 本机 PoC 实测（2026-10-12，Swift 6.3.2 / Xcode）

**E.1**：根 `Package.swift` 声明子目录路径的目标（`path: "swift/rutis/Sources/Rutis"`）构建、运行正常。

**E.2 SwiftPM 版本 tag 规则**：对 git 依赖实测——`1.0.0` 与 `v1.1.0` 被接受为版本；`rutis-swift-1.0.0`、`swift/rutis/1.0.0`、`rutis-mono-1.2.0` 均被忽略。结论：主发布 `v` tag 直接可用作 SwiftPM 版本。

**E.3 参数包上下文推断**：`func method<each A, R>(_:) -> (Ctx, repeat each A) async throws -> R` 在 `LLM(ask: s.method("ask"), …)` 处按字段闭包类型完成推断并编译通过。推断不稳的签名回退到 0–3 参数重载（S12）。
