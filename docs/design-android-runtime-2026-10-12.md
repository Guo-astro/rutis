# Android 应用托管（Kotlin SDK）

[English](design-android-runtime-2026-10-12.en.md)

状态：定稿，未实现。日期：2026-10-12，2026-10-14 定稿修订（改为按 [应用接入](design-app-integration-2026-10-14.md)，见 §十）。基准：`main` `a763cdc`（0.10.0）。
依据：[应用接入](design-app-integration-2026-10-14.md)（通用模型）、[远程运行时：拨出与动态接入](design-remote-runtime-dial-2026-10-14.md)（连接）、[多语言总体稿](design-multilang-runtimes-2026-10-03.md)、[插件 API](guide/plugin-api.md)、[Swift SDK](design-swift-sdk-2026-10-12.md)（形态②的 Swift 对应物）。

定位：Android 应用把自身能力与所在设备的能力作为 rutis 插件提供；插件用 Kotlin 编写、编进应用，由 rutis 驱动。桌面 JVM 运行时见 [Kotlin 运行时](design-kotlin-runtime-2026-10-12.md)（挂起）。

范围外：在设备上装载新代码（DEX 下载、上传、热替换）；应用的前后台、前台服务、Doze 适配（业务层，见应用接入 §七）；Intent 等 Android 生态反向调用 rutis；在设备上运行 rutis 本体；Play 合规流程本身。

## 一、决策

| # | 决策 | 章节 |
| --- | --- | --- |
| K1 | 插件按 rutis 范式编写、编进应用（APK / AAB），由 rutis 驱动装载、配置、卸载 | 二 |
| K2 | SDK 对应用只提供 `start` / `stop`；前台服务、Doze、厂商后台限制属于业务层 | 五 |
| K3 | 远程连接两个方向都支持：拨出到 rutis 的接入点（推荐，每条连接在 rutis 侧是一个实例）与监听（rutis 拨入）；非回环必须 `wss://`，无明文例外 | 五 |
| K4 | SDK 是 Kotlin 库，两个 target：android 与 jvm；jvm target 用于桌面上的一致性测试，也是将来桌面运行时的基础 | 七 |
| K5 | 每个进来的调用一个平台线程；`suspend` 方法在本调用线程上 `runBlocking` 桥接；声明 `sync-wait` 与 `reentrant-sync`，不声明 `sync-stack` | 四 |
| K6 | 无运行时反射：服务方法用显式方法表，配置 schema 显式给出（与 Swift 一致）；KSP 生成器为后续项 | 三 |
| K7 | 值：JSON 兼容类型递归复制；函数与显式 `Rutis.ref` 按引用；整数超出 ±2^53−1 报错 | 三 |
| K8 | 依赖：kotlin-stdlib、kotlinx-coroutines-core、kotlinx-serialization-json、OkHttp（WebSocket 客户端）、Java-WebSocket（WebSocket 服务端，纯 Java） | 七 |
| K9 | 系统权限归属应用；未授权 = `apply` 或方法调用失败；运行中撤销运行时权限时系统会杀应用进程，对 rutis 表现为断开 | 六 |
| K10 | 平台：Android 10（API 29）起；纯 Kotlin，无 native 代码 | 七 |
| K11 | 宿主侧只需 `runtimes.remote[].language` 接受 `"kotlin"` / `"kt"`；其余按拨出稿 | 八 |

## 二、模型

```text
rutis（桌面 / 服务器）                    Android 应用
 ├─ 接入点（监听，wss）  ◀──拨出────────  ├─ 业务代码：决定何时 start / stop
 ├─ 每条连接一个实例                       ├─ rutis Kotlin SDK
 │    └─ 行 phone:contacts、phone:notify   │    └─ 编进应用的插件：contacts / notify / device …
 └─ 其他行按名字 inject 这些服务           └─ Android API（权限归属应用）
```

- 同一应用内插件互调拿对象本身；跨连接、跨语言按名字经 rutis。
- rutis 不建模设备：型号、系统版本等由插件提供（如 `device` 插件）。
- 应用进程死亡、网络断开、`stop()`：对 rutis 都是断开——实例关闭，服务撤销，使用者停下；重连后按 rutis 当前配置重新装载。

## 三、插件 API

```kotlin
import arcships.rutis.*

@Serializable
data class Config(val city: String)

class Llm(s: RutisService) : RutisClient {
    val ask = s.method1<String, String>("ask")        // 按参数个数的重载，0–5 个参数
}

val weather = Rutis.define(
    name = "weather",
    inject = listOf("llm"),
    provides = mapOf("weather" to Shape(async("today"), sync("unit"))),
    config = Rutis.config<Config>(
        Schema.obj(mapOf("city" to Schema.string("要查天气的城市")), required = listOf("city"))),
) { ctx, config ->
    val llm = ctx.use("llm", ::Llm)
    ctx.provide("weather", Methods(
        async("today") { q: String -> llm.ask("weather in ${config.city}: $q") },
        sync("unit") { -> "celsius" },
    ))
    ctx.effect { /* 卸载时逆序执行 */ }
}
```

| API | 规则 |
| --- | --- |
| `Rutis.define` | 校验声明（名称、方法表与形状一致）；带插件 API 版本 2 |
| `apply` | 可为 suspend；抛异常则装载失败，已登记的清理照常运行 |
| `ctx.use(name, factory)` | `name` 须在 `inject` 中；同进程提供者直接给本地实现 |
| `ctx.provide(name, methods)` | 方法表须与 `provides` 声明的形状一致；至卸载或返回的撤销函数为止 |
| `ctx.effect(cleanup)` | 可为 suspend，卸载时逆序执行 |
| `ctx.on(name, listener)` / `ctx.emit(name, args…)` | 叶子事件（插件 API 2）：本运行时进程内的事件总线，语义与其他运行时相同；不跨进程，`internal/*` 不可用 |
| `ctx.launch { }` | 在行的协程作用域运行；卸载时取消并等待 |
| 方法形状 | 默认 `async`，`sync` 显式标记 |
| 配置 | kotlinx-serialization 解码到 `@Serializable` 类型；schema 显式给出；失败即装载失败 |
| 错误 | 异常按错误图返回；名字取异常类简名 |

| Kotlin 值 | 跨进程 |
| --- | --- |
| `null`、`Boolean`、`String`、`Int` / `Long` / `Double` | 复制；整数超出 ±2^53−1 报错 |
| `List`、`Map<String, *>`、`@Serializable` 类型 | 递归复制 |
| 函数（含 suspend 函数） | 引用 |
| `Rutis.ref(v)` | 引用；`v` 自带方法表 |
| 异常 | 名字与消息复制 |
| 其他对象 | 不可跨进程：报错并提示改用数据类型或 `Rutis.ref` |

## 四、线程与会话

| 项 | 规则 |
| --- | --- |
| 会话结构 | 一个读线程解析帧；写帧一把锁串行；每个进来的调用一个新平台线程（ART 无虚拟线程） |
| 出站调用 | 当前线程阻塞在 `CompletableFuture` 上，读线程完成它；调用对端 `sync` 方法时帧带 `sync: true` |
| 可重入 | 进来的调用任何时候都有新线程可跑；声明 `reentrant-sync`；插件的服务可能在自己同步调用途中被再次调用，调用 rutis 服务时不要持锁 |
| 能力 | `sync-wait`、`reentrant-sync`；不声明 `sync-stack`（并发多线程，只参与调用链边） |
| suspend 桥接 | 在本调用线程上 `runBlocking`；不在读线程上桥接 |
| 取消 | `cancel` 帧 → 取消该调用的协程作用域并中断其线程 |
| 插件线程未捕获异常 | 应用崩溃：对 rutis 表现为断开 |
| 契约 | endpoint 协议 3，握手声明能力 `runtime`；特性 `["rows.v2", "hosts", "leaf", "scopes", "events"]`；`mount` 回复带 `plugins`（拨出稿 §四）与 `implementation`、`engine`（kotlin + ART 版本）；调用号前缀 `node:` |

## 五、应用托管

```kotlin
val rutis = RutisApp(
    plugins = listOf(contacts, notify, device),
    connect = "wss://rutis.example.com:7443/rutis",
    id = "kim-pixel",                       // 自报 id（拨出稿 §五）
    token = secrets.rutisToken,
)

rutis.start()   // 连上；此后插件由 rutis 按配置装载
rutis.stop()    // 全部插件逆序清理，然后断开
```

| 项 | 规则 |
| --- | --- |
| `start()` | 连接、问候、报 `plugins`；断线后按拨出稿 §六 自动重连，直到 `stop()` |
| `stop()` | 卸载全部行（清理逆序），关闭连接；可再次 `start()` |
| 状态 | `rutis.state`（`Stopped` / `Connecting` / `Connected`，`StateFlow`）供业务层显示；SDK 不据此做生命周期决定 |
| 证书 | 非回环必须 `wss://`；自签证书经 SDK 参数信任其 CA |
| 监听 | `RutisApp(listen = "wss://0.0.0.0:7443/rutis", …)` 改为等 rutis 拨入，语义同 Go 的远程监听（一次一个控制方、新连接接管）；适合地址固定的设备，一般场景推荐拨出 |
| 换代码 | 发新版本应用 |
| 业务层参考 | 需要常驻时由应用开前台服务（Android 14 起须声明前台服务类型）并引导电池优化白名单；否则跟随界面 `start` / `stop`。写进 SDK 指南，不进 SDK |

## 六、权限

| 情况 | 行为 |
| --- | --- |
| 装载时未授权 | 插件在 `apply` 中检查并抛错，错误点名权限；行装载失败 |
| 调用时未授权 | 方法抛错，错误传回 rutis 侧调用方 |
| 运行中撤销运行时权限 | 系统杀应用进程 → 断开 → 实例关闭；应用重启并重连后，按配置重新装载，未授权的插件 `apply` 失败 |
| 特殊权限（通知监听、辅助功能等）被关 | 插件自行感知并撤销自己提供的服务（`ctx.provide` 返回的撤销函数），或在方法调用时报错 |

授权提示指向应用（与 Swift 形态② 对应）。SDK 不做权限对账，权限是插件自己的事。

## 七、SDK 与打包

| 项 | 规则 |
| --- | --- |
| 坐标 | `io.github.arcships:rutis`（Kotlin Multiplatform：android + jvm）；发布 Maven Central；版本进 `scripts/train.mjs` |
| 仓库位置 | `kotlin/rutis`（Gradle 项目） |
| 包 | `arcships.rutis`（API）；会话层 `arcships.rutis.internal.peer` |
| 依赖 | K8；插件作者只依赖本库 |
| 平台 | minSdk 29；纯 Kotlin，无 ABI 限制 |
| 测试助手 | `arcships.rutis.testing`：无宿主加载、严格模式（跨进程值语义模拟），同 Go `rutistest` |

## 八、宿主侧

| 改动 | 位置 |
| --- | --- |
| `runtimes.remote[].language` 接受 `"kotlin"` / `"kt"` → `RuntimeResolver::modules` | `rutis-host/src/host.rs` |
| 其余（接入点、连接即实例、身份层） | 拨出稿 §七 |

配置示例见拨出稿 §四（`language` 写 `kotlin`）。

## 九、测试与阶段

| 测试 | 位置 | 内容 |
| --- | --- | --- |
| 会话单元 | `kotlin/rutis`（jvm target） | 编码、引用计数、同步调用中的回调、取消、调用链、并发 |
| 会话一致性 | `rutis-bridge/tests/runtime_conformance.rs` + Kotlin conformance 入口（jvm target） | `session::testing::session` |
| 运行时一致性 | `multilang.rs` 加 Kotlin 列 | 叶子插件；多语言互调；撤提供者只停使用者；`inject` 门控；交叉同步调用不卡死 |
| 拨出 | `dial_conformance.rs` 加 Kotlin 列 | 拨出稿 §六 全部条目 |
| 监听 | `websocket_conformance.rs` 加 Kotlin 远程端 | 与 Go 的远程监听同一套断言 |
| 设备 | Android 模拟器仪表化测试 | `RutisApp` 在 ART 上拨出接入、装载、`stop` / `start`；权限被拒的错误传回 |
| CI | JDK job（jvm target）+ 模拟器 job | Gradle 构建先于 cargo 测试 |

| 阶段 | 工作 | 验收 |
| --- | --- | --- |
| K-P1 | SDK 核心（会话、插件 API、值、线程），jvm target | 桌面过会话与运行时一致性测试 |
| K-P2 | 拨出与监听通道、`RutisApp`、android target；依赖拨出稿 R-P1 | 模拟器与真机上的应用拨出接入，按 rutis 配置装载插件；`stop` 后使用者停下，`start` 后恢复；两台设备同名插件各在自己的实例；`dial_conformance`、`websocket_conformance` Kotlin 列过 |
| K-P3 | 示例能力插件（contacts、notify、device）与 SDK 指南 | 真机：授权提示指向应用，拒绝后错误传回 |

后续（有需求时）：KSP 生成方法表与 schema；桌面 JVM 运行时（重开 Kotlin 运行时稿）。

## 十、与初稿的出入

| 初稿 | 本文 | 理由 |
| --- | --- | --- |
| 只有设备监听、宿主拨入 | 两个方向都支持，推荐拨出 | 设备地址不稳定、在 NAT 后；方向与拨出稿 R1 一致 |
| 局域网允许明文 `ws://` | 删除 | 违反远程插件 §7"非回环必须 TLS"；宿主拨号端也拒绝非回环明文 |
| 经认证端点上传 DEX、`DexClassLoader` 装载 | 删除，插件编进应用 | 违反远程插件 §7"不提供代码上传"；与 iOS 模型一致 |
| 热载预算（每插件 50 次）与自重启 | 删除 | 不再装载新代码 |
| 权限对账（30 秒周期）、撤销即撤服务 | 删除，权限是插件自己的事 | 撤销运行时权限时系统直接杀进程，对账路径不会发生 |
| 前台服务、Doze、厂商自启动引导 | 移到业务层参考 | SDK 不管平台生命周期 |
| 辅助 APK 作为唯一载体 | 任意应用接入 SDK 即可托管插件 | 应用本身按范式提供能力 |
| SDK 只依赖 kotlin-stdlib | K8 | Android 无内置 WebSocket 客户端与服务端；配置解码用 kotlinx-serialization（编译期，无运行时反射） |
| arm64-v8a 与 x86_64 | 无 ABI 限制 | 纯 Kotlin，无 native 代码 |
