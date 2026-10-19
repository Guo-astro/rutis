# Android 应用托管（Kotlin SDK）

[English](design-android-runtime-2026-10-12.en.md)

状态：定稿；K-P1 已实现（#390，`kotlin/rutis`），K-P2、K-P3 未实现。日期：2026-10-12，2026-10-14 定稿修订（改为按 [应用接入](design-app-integration-2026-10-14.md)，见 §十），2026-10-19 对齐 rutis 0.13 的运行时契约（值、自动对外、错误、调用方的行、统一事件、开发者无感，见 §十一）。基准：`main` `028783f`（0.13.0）。
依据：[运行时契约](../.agents/skills/rutis-new-runtime/references/runtime-contract.md)（实现以它为准）、[应用接入](design-app-integration-2026-10-14.md)（通用模型）、[远程运行时：拨出与动态接入](design-remote-runtime-dial-2026-10-14.md)（连接）、[跨语言值类型](design-value-types-2026-10-15.md)、[统一事件的协议规格](design-events-protocol-2026-10-18.md)、[多语言总体稿](design-multilang-runtimes-2026-10-03.md)、[插件 API](guide/plugin-api.md)、[Swift SDK](design-swift-sdk-2026-10-12.md)（形态②的 Swift 对应物）、[Go 插件运行时](design-go-runtime-2026-10-10.md)（每调用一个执行单元的先例）。

定位：Android 应用把自身能力与所在设备的能力作为 rutis 插件提供；插件用 Kotlin 编写、编进应用，由 rutis 驱动。本文是 Kotlin SDK 的唯一现行设计：桌面 JVM 运行时（[Kotlin 运行时](design-kotlin-runtime-2026-10-12.md)）挂起，不是本文的前提；K-P1 的 jvm target 只用于在桌面上跑一致性测试。

范围外：在设备上装载新代码（DEX 下载、上传、热替换）；应用的前后台、前台服务、Doze 适配（业务层，见应用接入 §七）；Intent 等 Android 生态反向调用 rutis；在设备上运行 rutis 本体；Play 合规流程本身；宿主侧的 `kt:` 本地运行时（挂起的桌面运行时）；接收对象引用（能力 `objects`，同 Go）。

## 一、决策

| # | 决策 | 章节 |
| --- | --- | --- |
| K1 | 插件按 rutis 范式编写、编进应用（APK / AAB），由 rutis 驱动装载、配置、卸载 | 二 |
| K2 | SDK 对应用只提供 `start` / `stop`；前台服务、Doze、厂商后台限制属于业务层 | 五 |
| K3 | 远程连接两个方向都支持：拨出到 rutis 的接入点（推荐，每条连接在 rutis 侧是一个实例）与监听（rutis 拨入）；非回环必须 `wss://`，无明文例外；拨出规则按契约 §1 | 五 |
| K4 | SDK 是 Kotlin 库，两个 target：android 与 jvm；jvm target 用于桌面上的一致性测试（`Rutis.serve` 测试入口），也是将来桌面运行时的基础 | 七 |
| K5 | 每个进来的调用一个平台线程；`suspend` 方法在本调用线程上 `runBlocking` 桥接；调用链放在 `ThreadLocal`，经 `asContextElement()` 随协程传递，插件代码不传上下文；声明 `sync-wait` 与 `reentrant-sync`，不声明 `sync-stack` | 四 |
| K6 | 无方法的运行时反射：KSP 处理器（`rutis-ksp`，只在编译期）为 `@RutisService` 类生成方法表、为 `@RutisClient` 接口生成代理；配置 schema 由 `@Serializable` 配置类型的 `SerialDescriptor` 推出，不另写 | 三 |
| K7 | 值：SDK 自己的 kotlinx-serialization 格式（`RutisFormat`，不用 `Json` 默认编码）逐个检查；十种带标记的值全部实现并声明 `value-<tag>`；超出 ±(2^53−1) 的整数自动按 `bigint`；`@RutisService` 实例与函数按引用，其余 `@Serializable` 值按数据 | 三 |
| K8 | 运行时依赖：kotlin-stdlib、kotlinx-coroutines-core、kotlinx-serialization-core 与 -json（帧）、OkHttp（WebSocket 客户端）、Java-WebSocket（WebSocket 服务端，纯 Java）；编译期另有 KSP 处理器 | 七 |
| K9 | 系统权限归属应用；未授权 = `apply` 或方法调用失败；运行中撤销运行时权限时系统会杀应用进程，对 rutis 表现为断开 | 六 |
| K10 | 平台：Android 10（API 29）起；jvm target JDK 17；纯 Kotlin，无 native 代码 | 七 |
| K11 | 宿主侧只需 `runtimes.remote[].language` 接受 `"kotlin"` / `"kt"`（已实现）；其余按拨出稿 | 八 |
| K12 | 插件 API 版本 **3**；统一事件 `ctx.on` / `emit` / `parallel` / `serial` / `waterfall` / `bail` / `filter`，能力 `events-v2`；监听器一律登记为 `kind: "sync"`（每次调用一个线程，同 Go） | 3.4 |
| K13 | 服务自动对外：`ctx.provide(name, obj)` 不需要 `provides`，没声明的服务以 `rows.announce` 通告、方法全为 `async`；方法种类是声明（默认 `async`），与 `suspend` 与否无关 | 3.2 |
| K14 | 错误两个方向都带 `name` / `message` / `code` / `data` 与错误图；收到的错误与 SDK 自己的错误都是 `RutisException`，按 `name` 匹配 | 3.5 |
| K15 | 调用方的行：每行按 `rows.load` 的 `isolate` 用自己的服务 id；作用域标签不透明，只原样拼接 | 4.2 |
| K16 | 测试工具 `arcships.rutis.testing` 在测试进程里经真实会话（内存通道）运行插件，测试一方是宿主；与真实运行时对同一份 `weather.expected.json`、`events.expected.json` 比对 | 九 |

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
- 事件：应用里插件的监听器登记到宿主的逻辑总线，与其他语言的监听器一样被任何发出方送达（按作用域）；连接所在的实例是这些行的作用域。

## 三、插件 API

```kotlin
import arcships.rutis.*
import kotlinx.serialization.Serializable

@Serializable
data class Config(val city: String)            // schema 由它推出

@RutisClient                                    // 用到的服务：普通接口
interface Llm {
    suspend fun ask(question: String): String
}

@RutisService                                   // 提供的服务：普通类
class Weather(private val llm: Llm, private val city: String) {
    suspend fun today(question: String): String = llm.ask("weather in $city: $question")
    fun unit(): String = "celsius"
}

val weather = Rutis.define<Config>(
    name = "weather",
    inject = listOf("llm"),
    provides = mapOf("weather" to service<Weather>(sync = setOf("unit"))),   // 可选：不写时自动对外，方法全为 async
) { ctx, config ->
    val llm = ctx.use<Llm>("llm")
    ctx.provide("weather", Weather(llm, config.city))
    ctx.on("session/created") { args -> println("new session ${args[0]}") }
    ctx.effect { /* 卸载时逆序执行 */ }
}
```

| API | 规则 |
| --- | --- |
| `Rutis.define<C>(name, inject, provides?, schema?) { ctx, config -> }` | 校验声明（名称；`provides` 里的方法在类的方法表中）；带插件 API 版本 3；`C` 是 `@Serializable` 类型（无配置用 `Rutis.NoConfig`） |
| `apply` | 可为 suspend；抛异常则装载失败，已登记的清理照常运行 |
| `ctx.use<I>(name)` | `I` 是 `@RutisClient` 接口；`name` 须在 `inject` 中。服务按本行自己的 id 找（§4.2）：同进程提供者实现了 `I` 时得到对象本身，否则得到代理（同进程经本地方法表，跨进程经 rutis）。接口的方法须是服务提供的方法，否则 `use` 抛错并点名方法 |
| `ctx.provide(name, obj)` | `obj` 是 `@RutisService` 类的实例；声明过的服务只对外声明的方法，没声明的自动通告（§3.2）；至卸载或返回的撤销函数为止 |
| `ctx.effect(cleanup)` | 可为 suspend，卸载时逆序执行 |
| `ctx.on` / `emit` / `parallel` / `serial` / `waterfall` / `bail` / `filter` | 统一事件，§3.4 |
| `ctx.scope` | 本行的作用域链 `List<Scope>`（`rows.load` 第七个参数） |
| `ctx.launch { }` | 在行的协程作用域运行（带调用链）；卸载时取消并等待 |
| 卸载信号 | 行的协程作用域在卸载时取消：`isActive` / `ensureActive()`，与 Kotlin 原生取消相同 |

### 3.1 用 KSP 生成方法表与代理

Android 上不用方法反射：R8 会改名与裁剪，`suspend` 函数经 Java 反射是带 `Continuation` 的签名，参数的泛型类型也不完整。所以方法表与代理在编译期由 KSP 生成（同 Room、Moshi codegen 的做法）：

| 项 | 规则 |
| --- | --- |
| `@RutisService` | 生成 `<类名>_RutisService`：方法表（线上名 → 用 `RutisFormat` 与编译期确定的 serializer 解码参数、调用、编码返回值）与方法名列表 |
| 哪些方法 | 类及其父类上的 `public` 成员函数（`Any` 的 `equals` / `hashCode` / `toString` 不算；属性不算）——对应 Go 的"导出方法"；线上名是函数名；同名重载为编译错误 |
| `@RutisClient` | 生成 `<接口名>_RutisClient` 代理：`suspend` 函数挂起等待；普通函数阻塞当前线程等待（插件代码运行在每调用一个的线程上；在 Android 主线程上调用时抛错，避免 ANR） |
| 查找 | `ctx.provide` / `ctx.use<I>()` 按类名找生成的类（`Class.forName("<名>_RutisService")`，Room、Moshi 同款）；SDK 的 AAR 自带 consumer R8 规则保留生成类与被标注类的名字 |
| 依赖 | 应用的 `build.gradle.kts` 加 `ksp("io.github.arcships:rutis-ksp:<版本>")`（`new` 模板与指南写好）；处理器不进 APK |

### 3.2 服务、方法种类与自动对外

- 方法种类是声明，不由 Kotlin 签名决定：`service<Weather>(sync = setOf("unit"))` 列出的方法为 `sync`，其余与没声明的服务全部为 `async`（契约 §4）；也可写 `mapOf("today" to Kind.Async, "unit" to Kind.Sync)`。`suspend` 与否只决定实现怎样在本调用线程上运行。
- 自动对外：`ctx.provide` 的服务不在 `provides` 里时，按契约 §4 以 `service(id, handle, version, { row, methods })` 通告，方法取生成的方法表、全部 `async`；只向 `mount` 带 `announce: true` 的宿主通告，回复到了才答复 `rows.load`（重名让装载失败）；宿主比 0.12 旧时按契约在 stderr 提示一次。
- `rows.schema` 回复与 `mount` 的 `plugins` 里 `announces` 总是 `true`。
- 调用对端声明为 `sync` 的方法时帧带 `sync: true`（种类取自 `hosts.provide` 的方法表），参加宿主的等待环检测。
- 同进程调用：对象实现了使用方接口时直接调用（不编码）；否则经本地方法表。

### 3.3 值

值层是 SDK 自己的 kotlinx-serialization 格式 `RutisFormat`（自定义 `Encoder` / `Decoder`），由编译期生成的 serializer 驱动，逐个检查每一个值；不经 `Json` 的默认编码（它把日期编成数字或字符串、二进制编成数组，结构体内部绕过检查，即 Go 在 #355 的问题）。规则按[值类型设计](design-value-types-2026-10-15.md)与契约 §5。

| Kotlin 值 | 跨进程 |
| --- | --- |
| `null`、`Boolean`、`String`、`Double` / `Float` | 复制；NaN、±Infinity 让这次调用失败 |
| `Int` / `Long` / `ULong` 等整数 | ±(2^53−1) 内为数字；超出时**自动**按 `bigint`（V6） |
| `List` / `Array`、`Map<String, *>`、`@Serializable` 类型 | 递归复制为数据（字段名按 `@SerialName`）；字符串键的 Map 是对象 |
| `java.time.Instant`、`kotlin.time.Instant` | `instant`；纳秒精度，年份 0001–9999 |
| `java.time.LocalDateTime` / `LocalDate` / `LocalTime` | `datetime` / `date` / `time` |
| `ByteArray` | `bytes` |
| 键不是 `String` 的 `Map` | `map`；键与 `Set` 元素的限制按契约 §5（时间点、`ByteArray`、`null`、列表、Map、Set、引用作键在发出前拒绝） |
| `Set` | `set` |
| `java.math.BigInteger` | `bigint`；收到的 `bigint` 解码进放得下的声明整数类型，放不下时这次调用失败 |
| `java.math.BigDecimal` | `decimal`（保留标度即末尾零；按 `toPlainString()`，不用指数） |
| `java.util.UUID`、`kotlin.uuid.Uuid` | `uuid`（小写） |
| 函数（`suspend` 或普通函数类型） | 引用；参数与返回值按本表编解码 |
| `@RutisService` 类的实例 | 引用；远端得到调用其方法表的代理 |
| 异常 | 作为值时复制为 `{ name, message }`；抛出时见 §3.5 |
| 声明为 `Any?` 的参数 | 按上表反向解码成原生值：整数为 `Long`，浮点为 `Double`，对象为 `Map<String, Any?>`，列表为 `List<Any?>`，`map` 为 `Map<Any?, Any?>`，`set` 为 `Set<Any?>`，`bigint` 为 `BigInteger`，`decimal` 为 `BigDecimal`，时间为 `java.time` 类型，函数为 `RemoteFunction`（`suspend operator fun invoke(vararg args)`） |
| 其他对象（既不是 `@Serializable` 也不是 `@RutisService`）、循环引用 | 不可跨进程：这次调用失败，点名类型 |

- `@Serializable` 类的字段是 `java.time`、`BigInteger`、`BigDecimal`、`java.util.UUID` 时，kotlinx 要求字段标 `@Contextual`（kotlinx 自己的规则，`RutisFormat` 提供这些 serializer）；`kotlin.time.Instant`、`kotlin.uuid.Uuid` 不需要。方法的参数与返回值不需要任何标注（生成的代码直接选 serializer）。
- 嵌套至多 64 层，由发送方在发出前检查；一个值的任何一处不合规则只让这次调用失败，会话继续。
- 每种标记只发给声明了 `value-<tag>` 的对端；对端没有声明时这次调用失败，点名哪种值。SDK 在两种握手里声明全部十个 `value-<tag>`。
- 收到的带标记的值或键不合法时只让这次调用失败，其余照常解码，其中的引用全部导入再释放。
- 数据里的 `undefined` 收到为 `null`。

### 3.4 事件

统一事件按[协议规格](design-events-protocol-2026-10-18.md)与契约 §6。API（同 Go 的形状：监听器收到参数列表，`next` 在最后）：

```kotlin
val off = ctx.on("greeting/format") { args -> "${args.next!!() ?: args[0]}!" }
ctx.on("rutis/row", prepend = true) { args -> println(args[0]) }

ctx.emit("t/ping", 1, "a")                                  // 立即返回；只因 ReservedEvent 或参数发不出而抛
ctx.parallel("t/ping", 1)                                    // suspend；失败：RutisException(name = "AggregateError")
val first = ctx.serial("t/ask", "q")                         // suspend；Any?
val text = ctx.waterfall("greeting/format", "hi", terminal = { args -> args[0] })
val vote = ctx.bail("t/check", 42)                           // 阻塞本线程；线上同步：sync: true，参加等待环检测
val some = ctx.filter { d -> d.row == "chat" }               // Emitter：同样的五个方法
```

| 项 | 规则 |
| --- | --- |
| 监听器 | `suspend (EventArgs) -> Any?`；`EventArgs` 是解码成原生值（§3.3 的 `Any?` 规则）的参数 `List<Any?>`，`waterfall` 中最后一个元素是 `Next`（`suspend () -> Any?`），`args.next` 取它；`args.decode<T>(i)` 把第 `i` 个参数解码进 `@Serializable` 类型；返回值原样回复 |
| 选项 | `prepend`、`global`；返回立即注销的函数；随行卸载注销 |
| 登记 | `ctx.on` 时发 `events.on [{ id, name, row, plugin, kind: "sync", prepend, global }]`（`id` 会话内递增、不复用）；`apply` 期间登记的全部得到回复后才答复 `rows.load`；`internal/*` 在本地拒绝，宿主拒绝时按契约报告或让装载失败 |
| `kind` | 一律 `"sync"`：每次调用一个线程，监听器（含 `suspend` 的）在本线程上 `runBlocking` 跑完再回复，宿主可以同步等它（同 Go），所以 Kotlin 监听器从不引起 `AsyncListener` |
| 快速路径 | 名字不在 `events.elsewhere`、无前缀匹配，且（`emit`）没有经宿主、未答复的同名 `emit` 时本进程分发；否则发 `events.dispatch`。本进程条目的描述用 `mount` 的 `runtime` |
| 分发规则 | 快照（名字、登记顺序、作用域链前缀、存活）→ 过滤（非 `global`）→ 每次开始前检查存活；五种方式的语义与"没有结果"（`null`、`false`）按契约 §6 |
| `emit` 顺序 | 每个监听器一个串行队列：同一监听器上的 `emit` 调用按到达顺序一个接一个执行，不同监听器并发 |
| 过滤 | 快速路径直接对每个描述调用；经宿主时 `filter` 是一个选择器函数引用，凭描述的 `listener` 找到本进程的条目、去掉该字段后再交给插件的谓词；描述是 `Listener`（`name`、`prefix`、`plugin`、`row`、`runtime`、`node`、`scope`、`global`，没有值时为 `null`） |
| 取消 | 发出方的协程被取消：经宿主时对 `events.dispatch` 发 `cancel`；收到 `events.call` 的 `signal` 中止或 `cancel` 时取消该监听器的协程并中断其线程 |
| 保留名 | 发出 `rutis/*` 同步以 `ReservedEvent` 拒绝（`rutis/* events are emitted by rutis only: <name>`）；可以监听 `rutis/*` |
| 报告 | 没有人等待的失败按契约 §6 的原文各写一行（jvm 写 stderr；`RutisApp` 写到 `log` 回调，默认 `System.err`） |
| 卸载 | 收到 `rows.unload` 后立即拒绝这一行的 `events.call` → 撤下提供的服务 → 取消这一行正在运行的监听器并等待，至多关闭时限（jvm `RUTIS_SHUTDOWN_TIMEOUT`，`RutisApp` 的 `shutdownTimeout`，默认 10 秒），超过时写 `rutis: listeners of row <row> still running after <s>s; unloading anyway` → 逆序清理 |
| 断开 | `elsewhere` 视为空；卸载全部行 |

### 3.5 错误

| 方向 | 规则 |
| --- | --- |
| 发出（插件抛出） | `name` = 异常类的简名（`javaClass.simpleName`）；`message`；`cause` 链进错误图；异常有公开属性 `code: String`、`data` 时（读 `getCode()` / `getData()` 两个 getter，原生 Kotlin 写法，不需要 rutis 类型；SDK 的 consumer R8 规则保留这两个 getter）带上 `code` 与 `data`；`data` 发不出时略去，其余照常发出 |
| 收到 | `RutisException`：`name`、`message`、`code: String?`、`data: Any?`（按值解码）、`cause`、`errors`（`AggregateError` 的子错误，按登记顺序）；按 `name` 匹配（`e.name == "SyncWaitCycle"`），不按类 |
| 再次抛出 | 收到的 `RutisException` 原样再抛时整个错误图（含 `code` / `data`）再跨一次 |
| SDK 自己的错误 | 也是 `RutisException`，名字按契约：`AggregateError`、`AsyncListener`、`WaterfallNext`、`ReservedEvent`、`SyncWaitCycle`、`BindingError`；常量 `RutisException.SYNC_WAIT_CYCLE` 等 |

### 3.6 配置

| 项 | 规则 |
| --- | --- |
| 类型 | `@Serializable` 类型，经 `RutisFormat` 从 `rows.load` 的配置值解码；失败即装载失败，错误点名字段 |
| Schema | 由类型的 `SerialDescriptor` 推出（编译期生成，无反射）：基本类型、列表、Map、嵌套类、可空、有默认值的字段（不进 `required`）、枚举（`enum`）；`define(schema = …)` 给出时以它为准 |

## 四、线程与会话

### 4.1 线程

| 项 | 规则 |
| --- | --- |
| 会话结构 | 一个读线程解析帧；写帧一把锁串行；每个进来的调用一个新平台线程（ART 无虚拟线程；jvm target 用同一份代码）；`events.on` / `events.off` / `events.elsewhere` 在读线程上按帧序就地处理 |
| 出站调用 | 普通函数：当前线程阻塞在 `CompletableFuture` 上，读线程完成它；`suspend` 函数：挂起在同一个 future 上；调用对端 `sync` 方法或 `bail` 时帧带 `sync: true` |
| 可重入 | 进来的调用任何时候都有新线程可跑；声明 `reentrant-sync`；插件的服务可能在自己同步调用途中被再次调用，调用 rutis 服务时不要持锁 |
| 能力 | `sync-wait`、`reentrant-sync`；不声明 `sync-stack`（并发多线程，只参与调用链边） |
| suspend 桥接 | 在本调用线程上 `runBlocking`；不在读线程上桥接 |
| 调用链 | `ThreadLocal`，进来的调用在其线程上设 `path + [id]`；以 `asContextElement()` 放进协程上下文，`withContext` 换线程也带着；插件自己 `Thread { }` 起的线程没有链（写进指南：用 `ctx.launch` 或协程） |
| `sync` 方法 | 实现可以是 `suspend`，也可以在里面调用其他服务；与 Go 相同 |
| 取消 | `cancel` 帧或 `signal` 中止 → 取消该调用的协程作用域并中断其线程；发出方的协程被取消时对 `async` 调用发 `cancel` |
| `SyncWaitCycle` | 自身不产生；收到时为 `RutisException`，`name == "SyncWaitCycle"` |
| 插件线程未捕获异常 | 应用崩溃：对 rutis 表现为断开 |

### 4.2 契约

compat 协议 2（jvm 测试入口的本地通道）、endpoint 协议 3（拨出与监听）。

| 项 | 值 |
| --- | --- |
| 能力（两种握手都声明） | `signals`、`sync-wait`、`reentrant-sync`、`events-v2`、十个 `value-<tag>`；endpoint 握手另声明 `runtime`；不声明 `sync-stack`、`objects` |
| 特性（`mount` 回复） | `["rows.v2", "hosts", "leaf", "scopes", "rows.announce"]` |
| `mount` | 记下 `announce` 与 `runtime`；回复带 `plugins`（拨出稿 §四，每项同 `rows.schema` 回复）、`implementation: { name: "io.github.arcships:rutis", version }`、`engine`（`kotlin` + ART / JVM 版本） |
| `rows.schema` | `{ config, inject, provides, version, announces: true }` |
| `rows.load(key, entry, config, isolate, inject, exports, { scope, id })` | 核对插件 API（插件比 SDK 新时拒绝）→ 解码配置 → `apply`；声明的服务以三参数 `service` 报告、没声明的以四参数通告并等答复；`apply` 期间的 `events.on` 全部得到回复后才答复；失败先卸载再报告；`id` 是描述与报告行称呼这一行的名字 |
| `rows.update` / `rows.unload` / `hosts.*` / `release` / `get` / `dispose` | 与 Go 相同；卸载顺序见 §3.4 |
| 调用号 | 前缀 `node:` |
| 旧宿主 | 对端的 `hello` 没有声明 `events-v2` 时，握手后立即结束会话，写 `rutis: refused: incompatible (this host does not declare events-v2 (the event protocol of rutis 0.13): upgrade rutis-host to 0.13 or later); stopping`；jvm 入口以状态 1 退出，`RutisApp` 停止重连、`state` 为 `Stopped` 并带这条原因 |
| 会话结束 | 卸载全部行，清理合计受关闭时限约束（默认 10 秒），超过时写一行；jvm 入口超时以 1 退出，`RutisApp` 回到未连接并按拨出规则重连 |

- **调用方的行**：宿主把每一行的服务租进这一行自己的作用域（标签里带 U+0001），以便 `host:<id>` 的调用让宿主知道是哪一行在调用。SDK 对每一行按 `rows.load` 的 `isolate`（名字 → 标签）计算该行使用的服务 id（名字，或名字 + NUL + 标签），`ctx.use` 只用这一行自己的 id；标签不透明，不解析、不比较内容，只校验不含 NUL、不为空。插件代码在哪一行里运行，调用就指向哪一行的 id（行的协程作用域带着行）。

### 4.3 通道

| channel | target | 实现 |
| --- | --- | --- |
| `unix:<path>` 或裸路径 | jvm | 拨回：JDK 16+ `UnixDomainSocketAddress`（测试入口） |
| `tcp:127.0.0.1:<port>` | jvm | 先发 `RUTIS_CHANNEL_TOKEN` + 换行（测试入口；Windows） |
| `dial:wss://…` | android + jvm | OkHttp WebSocket 客户端，子协议 `rutis.3`；按契约 §1 的拨出规则（Bearer `RUTIS_TOKEN`、`RUTIS_CA`、非回环 `ws://` 与不合法 id 启动即拒绝、按状态码 / 关闭码分类、每次断开逆序卸载全部行、stderr 状态行原文、`RUTIS_RETRY` 供测试） |
| `listen:wss://…` | android + jvm | Java-WebSocket 服务端；非回环必须 `wss://`；`RUTIS_TOKEN` 常量时间比较；心跳 30 秒；一次一个控制方，新连接接管 |

`fd:3` 不实现：JDK 没有包装继承 fd 的公开 API，且只用于宿主拉起的本地运行时（不在本文范围）。按行分帧，单条消息至多 16 MiB。jvm 测试入口 `Rutis.serve(args, plugins…)` 的参数与 Go 相同：`<channel> [--id <endpoint>] [--peer <endpoint>] <project>`。

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
| 参数 | `plugins`、`connect`（或 `listen`）、`id`、`token`；可选 `peer`、`trust`（额外信任的 CA，`RUTIS_CA` 的语义）、`shutdownTimeout`（默认 10 秒）、`log`（报告行与状态行的去处） |
| `start()` | 连接、问候、报 `plugins`；断线后按拨出规则（契约 §1）自动重连，直到 `stop()`；凭据被拒每 30 秒重试；不兼容（含宿主没有 `events-v2`）停止并把原因放进 `state` |
| `stop()` | 卸载全部行（清理逆序），关闭连接；可再次 `start()` |
| 状态 | `rutis.state`（`Stopped(reason?)` / `Connecting` / `Connected`，`StateFlow`）供业务层显示；SDK 不据此做生命周期决定 |
| 证书 | 非回环必须 `wss://`；自签证书经 `trust` 信任其 CA |
| 监听 | `RutisApp(listen = "wss://0.0.0.0:7443/rutis", certificate = …, …)` 改为等 rutis 拨入，语义同 Go 的远程监听（一次一个控制方、新连接接管）；适合地址固定的设备，一般场景推荐拨出 |
| 换代码 | 发新版本应用 |
| 业务层参考 | 需要常驻时由应用开前台服务（Android 14 起须声明前台服务类型）并引导电池优化白名单；否则跟随界面 `start` / `stop`。写进 SDK 指南，不进 SDK |

## 六、权限

| 情况 | 行为 |
| --- | --- |
| 装载时未授权 | 插件在 `apply` 中检查并抛错，错误点名权限；行装载失败 |
| 调用时未授权 | 方法抛错，错误传回 rutis 侧调用方（可带 `code`，如 `"permission-denied"`） |
| 运行中撤销运行时权限 | 系统杀应用进程 → 断开 → 实例关闭；应用重启并重连后，按配置重新装载，未授权的插件 `apply` 失败 |
| 特殊权限（通知监听、辅助功能等）被关 | 插件自行感知并撤销自己提供的服务（`ctx.provide` 返回的撤销函数），或在方法调用时报错 |

授权提示指向应用（与 Swift 形态② 对应）。SDK 不做权限对账，权限是插件自己的事。

## 七、SDK 与打包

| 项 | 规则 |
| --- | --- |
| 坐标 | `io.github.arcships:rutis`（Kotlin Multiplatform：android + jvm）与 `io.github.arcships:rutis-ksp`（KSP 处理器）；发布 Maven Central；版本跟主发布线，进 `scripts/train.mjs`；首个版本 0.14.0 |
| 仓库位置 | `kotlin/rutis`（Gradle 项目：`rutis`、`rutis-ksp`、一致性入口 `rutis-conformance`） |
| 包 | `arcships.rutis`（API）；会话层 `arcships.rutis.internal.peer`；测试工具 `arcships.rutis.testing` |
| 依赖 | K8；插件作者只依赖本库（与编译期的 KSP 处理器） |
| 平台 | minSdk 29；jvm target JDK 17；纯 Kotlin，无 ABI 限制 |
| R8 | AAR 带 consumer 规则：保留 `*_RutisService` / `*_RutisClient` 与被标注类的名字、异常的 `getCode()` / `getData()` |
| 文档 | `docs/guide/kotlin-app.md`（中英）；`docs/guide/plugin-api.md` 加 Kotlin 列 |

## 八、宿主侧

| 改动 | 位置 |
| --- | --- |
| `runtimes.remote[].language` 接受 `"kotlin"` / `"kt"` → `RuntimeResolver::modules` | `rutis-host/src/host.rs`（已实现） |
| 一致性：事件期望文件的语言加 `kotlin`；`session_matrix.rs`、`events_conformance.rs`、`multilang.rs` 加 Kotlin（测试用 feature `kotlin`，经通用的 `Launcher::new("java")` 启动 `rutis-conformance` 的 jar；不加 `LocalRuntime::kotlin` 与宿主配置） | `rutis-bridge/src/runtime/testing/events.rs`、测试 |
| 其余（接入点、连接即实例、身份层） | 拨出稿 §七 |

配置示例见拨出稿 §四（`language` 写 `kotlin`）。

## 九、测试与阶段

运行时契约的一致性测试就是契约本身（契约 §8）；Kotlin 过与其他运行时相同的全部套件。

| 测试 | 位置 | 内容 |
| --- | --- | --- |
| 会话单元 | `kotlin/rutis`（jvm target） | 编解码（十种值、键规则、深度、超界整数）、引用计数、同步调用中的回调、取消、调用链（含 `withContext` 换线程）、并发、KSP 生成代码 |
| 会话一致性 | `rutis-bridge/tests/runtime_conformance.rs` + `rutis-conformance` 入口（jvm target） | `session::testing::session` |
| 契约 × 通道 | `session_matrix.rs` 加 Kotlin | 拨回 `unix:`、回环 `tcp:`、（K-P2）监听 WebSocket；`weather.expected.json` 的全部调用、样例值与事件 |
| 值往返 | `tests/value_roundtrip.rs` 加 Kotlin 端 | 十种值与键规则的属性测试 |
| 事件一致性 | `rutis-loader/tests/events_conformance.rs`（X = kotlin；Y 含 kotlin） | `events.expected.json` 全部用例；CI 的 `RUTIS_EVENTS_REQUIRE` 加 `kotlin` |
| 测试工具 | `kotlin/rutis`（jvm target） | `arcships.rutis.testing` 对 `weather.expected.json` 与 `events.expected.json`（`tool` 不为 `false` 的用例）比对，结果与真实运行时相同 |
| 运行时一致性 | `multilang.rs` 加 Kotlin 列 | 叶子插件；多语言互调；撤提供者只停使用者；`inject` 门控；没声明的服务被通告；交叉同步调用不卡死，与 Node 的交叉同步环被检测为 `SyncWaitCycle` |
| 专项 | `rutis-bridge/tests/kotlin_runtime.rs` | 旧宿主拒绝（原文、状态 1）；错误 `code` / `data` 两个方向与再抛；调用方的行（两行不同 `isolate`）；取消到协程 |
| 拨出 | `dial_conformance.rs` 加 Kotlin 列 | 拨出稿 §六 全部条目 |
| 监听 | `websocket_conformance.rs` 加 Kotlin 远程端 | 与 Go 的远程监听同一套断言 |
| 设备 | Android 模拟器仪表化测试 | `RutisApp` 在 ART 上拨出接入、装载、`stop` / `start`；R8 开启的 release 构建里方法表与代理可用；权限被拒的错误传回 |
| CI | JDK job（jvm target）+ 模拟器 job | Gradle 构建先于 cargo 测试 |

| 阶段 | 工作 | 验收 |
| --- | --- | --- |
| K-P1 | SDK 核心（会话、插件 API、KSP 处理器、值、错误、事件、线程），jvm target，`unix:` / `tcp:` 测试入口，测试工具 | 桌面过：`runtime_conformance`、`session_matrix`（本地通道 + `weather.expected.json`）、`value_roundtrip`、`events_conformance`（Kotlin 作 X 与 Y）、`multilang` 的 Kotlin 列、测试工具对两份期望文件、旧宿主拒绝 |
| K-P2 | 拨出与监听通道、`RutisApp`、android target；依赖拨出稿 R-P1（已完成） | 模拟器与真机上的应用拨出接入，按 rutis 配置装载插件；`stop` 后使用者停下，`start` 后恢复；两台设备同名插件各在自己的实例；`dial_conformance`、`websocket_conformance`、`session_matrix` 监听列 Kotlin 过；R8 release 构建可用 |
| K-P3 | 示例能力插件（contacts、notify、device）与 SDK 指南 | 真机：授权提示指向应用，拒绝后错误传回 |

后续（有需求时）：桌面 JVM 运行时（重开 Kotlin 运行时稿）；接收对象引用（`objects`）。

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
| SDK 只依赖 kotlin-stdlib | K8 | Android 无内置 WebSocket 客户端与服务端；值层与配置用 kotlinx-serialization（编译期，无运行时反射） |
| arm64-v8a 与 x86_64 | 无 ABI 限制 | 纯 Kotlin，无 native 代码 |

## 十一、对齐 rutis 0.13（2026-10-19）

0.11–0.13 改变了运行时契约：十种值（0.11）、自动对外与错误的 `code` / `data`（0.12）、调用方的行（#441）、统一事件与插件 API 3（0.13）、拨出规则写定（#383/#384）。本节记录本文因此做的修订与新决定；实现以[运行时契约](../.agents/skills/rutis-new-runtime/references/runtime-contract.md)为准。

### 11.1 修订

| 2026-10-14 稿 | 现在 | 原因 |
| --- | --- | --- |
| 插件 API 2，叶子事件只在本进程，特性 `events` | 插件 API 3；统一事件（§3.4），能力 `events-v2`，特性 `events` 删除，对旧宿主按原文拒绝 | 0.13 事件协议 |
| 显式方法表 `Methods(async("today") { … })`、`provides` 必写 | KSP 从 `@RutisService` 类生成；`provides` 可选，自动对外 | 自动对外（0.12）与开发者无感（V0） |
| `RutisClient` 子类里 `s.method1<String, String>("ask")` | 普通接口 + `@RutisClient`（KSP 生成代理），`ctx.use<Llm>("llm")` | V0 |
| Schema 显式给出 | 由 `@Serializable` 配置类型的 `SerialDescriptor` 推出 | V0；仍然没有反射 |
| JSON 兼容类型复制；整数超限报错；值经 kotlinx-serialization-json | 自己的 `RutisFormat`；十种值；超界整数自动 `bigint` | 值类型设计 V1、V6、§六 |
| `Rutis.ref(v)` 显式引用 | `@RutisService` 实例按引用 | 方法表已由类型给出 |
| 异常只复制名字与消息 | `code` / `data` 两个方向；收到的为 `RutisException` | 0.12 错误契约 |
| 特性 `["rows.v2", "hosts", "leaf", "scopes", "events"]` | `["rows.v2", "hosts", "leaf", "scopes", "rows.announce"]` + 能力 `events-v2`、`value-*` | 契约 §2、§3 |
| K-P1 验收"会话与运行时一致性测试" | 加值往返、事件一致性、测试工具对两份期望文件、旧宿主拒绝 | 一致性测试即契约 |
| 挂起的 Kotlin 运行时稿的提示说"远程只拨出" | 两个方向都支持（K3），挂起稿的提示已改正 | 与 K3、拨出稿 R1 一致 |

### 11.2 仍需要 rutis 专用代码的地方

开发者无感（V0）：插件作者只写原生 Kotlin。Android 上不用方法的运行时反射（R8、`suspend` 签名），下面是仍然需要的 rutis 专用写法，已压到最少：

| 地方 | 写法 | 为什么不能更少 |
| --- | --- | --- |
| 插件定义与进入点 | `Rutis.define<C>(…) { ctx, config -> }`；`RutisApp(…)` | 每种语言都有：范式本身 |
| 提供的服务类 | 类上一个 `@RutisService` | 方法表只能在编译期生成；一个注解，类本身仍是普通 Kotlin |
| 用到的服务 | 接口上一个 `@RutisClient`；`ctx.use<Llm>("llm")` | 同上；接口仍是普通 Kotlin，可被任何实现替代 |
| 声明 `sync` 方法（可选） | `provides = mapOf("weather" to service<Weather>(sync = setOf("unit")))` | 只在需要 `sync` 或想让 `check` 提前知道时写 |
| 构建配置 | `ksp("io.github.arcships:rutis-ksp:…")` 一行 | KSP 处理器要在应用的构建里登记 |
| 事件的少量类型 | `EventArgs`、`Next`、过滤的描述 `Listener` | 语言没有对应的东西 |
| 收到的错误 | `RutisException`，按 `name` 匹配 | 跨进程的错误不是发出方的类（各语言相同） |

不是 rutis 的、但 Kotlin 生态本来就要的：数据类与配置的 `@Serializable`；`@Serializable` 类里 `java.time` / `BigInteger` / `BigDecimal` / `java.util.UUID` 字段的 `@Contextual`（kotlinx 的规则；`kotlin.time.Instant`、`kotlin.uuid.Uuid` 不需要）。

不需要的：方法表、JSON 编码、引用包装、上下文参数、错误的 `code` / `data` 包装（普通属性即可）、同步等待与重入处理、事件登记、十种值的包装类型（全部是 JDK / Kotlin 原生类型）。

### 11.3 本次的决定

| # | 决定 |
| --- | --- |
| 1 | 方法表与代理用 KSP 在编译期生成，按类名约定查找（Room / Moshi 同款），R8 规则随 AAR 发布；不用方法反射 |
| 2 | `public` 成员函数对外，线上名是函数名，重载为编译错误 |
| 3 | 方法种类是声明（默认 `async`），与 `suspend` 无关；客户端接口里 `suspend` 函数挂起等待、普通函数阻塞等待（主线程上拒绝） |
| 4 | 调用链放 `ThreadLocal` + `asContextElement()`，插件代码不传上下文 |
| 5 | 值层是自定义 kotlinx-serialization 格式 `RutisFormat`，十种值全部实现，映射到 `java.time`、`BigInteger`、`BigDecimal`、`UUID` 等原生类型，另认 `kotlin.time.Instant`、`kotlin.uuid.Uuid`；`Any?` 参数解码为原生值 |
| 6 | 配置 schema 由 `SerialDescriptor` 推出，可覆盖 |
| 7 | 错误 `code` / `data` 读异常的 `getCode()` / `getData()`；收到的与 SDK 自己的错误都是 `RutisException`，按名字匹配 |
| 8 | 监听器形状同 Go：`suspend (EventArgs) -> Any?`，`next` 在最后；一律 `kind: "sync"`；`emit` 按监听器串行排队；`bail` 是阻塞函数，其余四种是 `suspend` |
| 9 | 调用方的行：每行按 `isolate` 算自己的服务 id，标签不透明 |
| 10 | 测试工具经真实会话（内存通道）运行插件，测试一方是宿主，对两份期望文件比对 |
| 11 | K-P1 的桌面测试入口是 jvm 上的 `Rutis.serve`（`unix:` / `tcp:`），由测试经通用 `Launcher` 启动；不重开桌面运行时，不加宿主配置 |
| 12 | 不声明 `objects`，同 Go |

## 十二、K-P1 实现记录（2026-10-19，#390）

实现在 `kotlin/rutis`（`rutis`、`rutis-ksp`、`rutis-conformance`）。与上文不同或上文没写的地方：

| 项 | 实现 | 原因 |
| --- | --- | --- |
| 源码布局 | 两个 target 共用 `rutis/src/shared`；jvm 专有的本地通道与 `Rutis.serve` 在 `src/jvmMain`。没有 Android SDK 时只配 jvm target | `UnixDomainSocketAddress` 不在 Android 的 API 里；jvm target 在没有 Android SDK 的机器上照常构建 |
| 写帧 | 一个写线程按入队顺序发帧（调用号在入队时分配） | 插件线程会被中断（取消），中断一个正在写 `SocketChannel` 的线程会关闭通道；写线程从不被中断 |
| 被行卸载取消的监听器 | 分发仍在进行时，它以"没有结果"结束（`null`） | 被取消的协程不能正常返回值；Go、Python 的监听器取消后返回，效果相同 |
| `RUTIS_CHANNEL_TOKEN` | 读取后不能从环境里删去 | JDK 没有修改本进程环境的公开 API；插件自己起的子进程会继承它 |
| 主线程上的阻塞调用 | 未检查 | 只在 Android 上有意义，随 `RutisApp` 在 K-P2 做 |
| `--rutis-manifest` | jvm 入口也答复（同 Go 的形状，`runtime` 为 `rutis-kotlin-runtime:1`） | 便于检查；K-P1 不由宿主读取 |
| 多语言列 | `crates/rutis-loader/tests/multilang_kotlin.rs`（同 `multilang_go.rs`），含经 Kotlin 的交叉同步环 | 自成一体，不改其他语言共用的测试 |
| 发出方取消 | 在一致性期望里 Kotlin 的取消是 `CancellationException` | 协程的取消 |
