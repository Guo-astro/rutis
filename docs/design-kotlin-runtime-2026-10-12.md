# Kotlin 插件运行时（设计稿）

[English](design-kotlin-runtime-2026-10-12.en.md)

> **挂起（2026-10-14；2026-10-19 更新）**：有具体需求（Maven / JVM 生态的插件）时再重开。Kotlin 的当前主线是 [Android 应用托管](design-android-runtime-2026-10-12.md)：它的 K-P1（里程碑 0.14）不依赖本文，其 jvm target 只用于桌面上的一致性测试，是本运行时将来的基础。重开时以 Android 稿（已对齐 rutis 0.13，见其 §十一）为准，本文的下列内容作废：方法反射（D20、§7.1 的 `methodsOf` / `schemaOf`）改为 KSP 生成方法表与代理、schema 由 `SerialDescriptor` 推出；值层（D8、§7）改为 `RutisFormat` 与十种值；插件 API 1（D14）改为 3 与统一事件；`provides` 必写改为自动对外；远程连接（D18）两个方向都支持（监听与拨出，见 [拨出与动态接入](design-remote-runtime-dial-2026-10-14.md)，此前这里写的"只拨出"有误）；依赖以 Android 稿 K8 为准（取代 D13）；崩溃策略（D15）按运行时的 `restart`（默认 `no`，#274）。下文"本设计是总体稿之后第一个新语言运行时""Swift 尚未开工"为起草时的表述，已过时（Bun、Go 已落地，Swift 已定稿）。

状态：挂起（见上）。日期：2026-10-12。基线：`main` `ebf0711`（0.9.1）。
依据：[多语言插件总体设计](design-multilang-runtimes-2026-10-03.md)（及其 [M1](design-multilang-m1-2026-10-04.md)、[M2](design-multilang-m2-2026-10-04.md) 实施稿）、[Bun 运行时设计](design-bun-runtime-2026-10-09.md)、[Go 插件运行时设计](design-go-runtime-2026-10-10.md)、[开发者包设计](design-developer-packages-2026-10-06.md)、[插件 API 约定](guide/plugin-api.md)。Swift（总体稿的 M3）尚未开工，本设计是总体稿之后第一个新语言运行时，按 Go 设计 §9 的提议替 Kotlin 回答可重入问题（§6），结论应回记到总体稿 §九。

范围外：Kotlin 里的 Cordis；Kotlin Multiplatform 的非 JVM target；在宿主进程内嵌 JVM（附录 A 说明为何不做）；宿主在运行时编译 Kotlin 代码。

## 1. 决定

| # | 决定 | 章节 |
| --- | --- | --- |
| D1 | 一个 Kotlin 运行时一个 JVM 进程，走现有 wire protocol（compat 协议 2），与 Python/Go/Bun 完全同构 | 2, A |
| D2 | 运行时名 `kt`，行名 `kt:<插件>`，走 `RuntimeResolver::modules`（同 `py:` 路径）；多实例 `.named()`，限定行 `<runtime name>:<插件>` | 3 |
| D3 | JDK 基线 21（虚拟线程），推荐 25 LTS；发现顺序：配置 → `JAVA_HOME` → `PATH`，启动前 `java -version` 预检（同 Python 解释器发现） | 4 |
| D4 | 通道默认 socket 路径回连（`Connect::DialBack`），Windows 走 loopback TCP；不用 `fd:3`——JDK 没有包装继承 fd 的公开 API | 4 |
| D5 | 线程模型：一个阻塞读线程 + 每个进来的调用一个虚拟线程；能力声明 `sync-wait` + `reentrant-sync`，不声明 `sync-stack`（与 Go 同类：并发多栈，只参与调用链边） | 6 |
| D6 | 插件 API 用 suspend 表达异步；同步桥接只发生在虚拟线程上的 `runBlocking`，绝不在读线程上 | 6 |
| D7 | 方法默认 `async`，`sync` 显式标记（Go D12 同理由：同步方法会阻塞事件循环型对端） | 7 |
| D8 | 值传递：JSON 兼容类型递归复制；函数与显式 `rutis.Ref` 按引用；`ctx.provide` 的服务对象总是按引用 | 7 |
| D9 | 每个插件一个 child-first `URLClassLoader`；卸载 = `close()` + 丢引用，靠 GC 回收；无泄漏卸载是 SDK 的一等义务（Metaspace 是最大工程风险） | 8 |
| D10 | 热重载 = 换 classloader（jar 的 mtime+size 戳，同 Python 模块重载）；DCEVM/JBR 不进默认分发 | 8 |
| D11 | 不用 Kotlin scripting / `.kts` 作插件载体；插件形态 = 编译后的 jar。`dev` 模式由宿主调用项目自己的 `gradle jar`（Go D15 同款：宿主不编译，dev 只重建项目自身） | 8 |
| D12 | 通道与帧：换行分隔紧凑 JSON，单帧 16 MiB；引用计数、`cancel`、调用链 `path`、`sync: true` 全按现有协议 | 5 |
| D13 | SDK 单 artifact `io.github.arcships:rutis`（SDK + 运行时同包，Python 模式），只依赖 kotlin-stdlib；kotlinx-coroutines-core 与 kotlin-reflect 由运行时携带，不进插件编译期 | 9 |
| D14 | 插件 API 版本 `PLUGIN_API = 1`，照抄现有机制 | 7 |
| D15 | 崩溃语义：服务方法抛异常 → 作为错误返回；插件自己起的线程里未捕获异常 → 进程退出，本运行时全部行停止，不自动重启 | 6 |
| D16 | 取消：`cancel` 帧取消该调用的协程作用域并中断其虚拟线程 | 6 |
| D17 | 本运行时不产生 `SyncWaitCycle`（进来的调用永远有新虚拟线程可跑）；收到时作为远程错误呈现 | 6 |
| D18 | 远程运行时（`listen:ws`）用端点格式（协议 3），排在后面阶段 | 12 |
| D19 | 进程启动策略与 Python 一致：挂载即启动；on-demand/idle（Go 式）作为开放问题（JVM 常驻内存比 Python 高一个量级） | 13 |
| D20 | 配置 schema：data class 主构造器反射派生（运行时内置 kotlin-reflect），或 `Plugin.schema` 显式给定 | 7 |

## 2. 背景：JVM / Kotlin 事实

以下结论来自 2025–2026 年的官方文档与一手来源（调研报告存档于本文撰写过程；实现前按 §14 的清单复核）：

| 事实 | 来源 | 对设计的影响 |
| --- | --- | --- |
| 虚拟线程 JDK 21 GA（JEP 444）；JDK 24（JEP 491）后 `synchronized` 阻塞不再 pin carrier 线程 | openjdk.org/jeps/444、/491 | “每个进来的调用一个执行单元”几乎免费；基线 21、推荐 24/25；21–23 上插件若在 `synchronized` 块里同步调 rutis 服务会 pin carrier（吞吐问题，不是死锁） |
| Unix domain socket JDK 16 起（JEP 380）`SocketChannel.open(UnixDomainSocketAddress)` | openjdk.org/jeps/380 | 回连通道零 native 依赖；整个会话层纯 Java，不需要 JNI/FFM |
| JDK 没有包装继承 fd（fd:3）的公开 API；`System.inheritedChannel` 只覆盖 stdio | JDK API 文档 | 通道走 socket 路径回连（D4）；本地 transport 已支持（`Connect::DialBack`） |
| `runBlocking` 阻塞当前线程并新建事件循环；在共享调度器线程上嵌套是明确劝退模式 | kotlinx.coroutines 文档、issue #3983 | 同步桥接只在专用虚拟线程上做（D6） |
| `Dispatchers.IO` 默认并行上限 64（或核数取大） | kotlinx.coroutines 文档 | 不能当“每调用一线程”的池用；进来的调用直接开虚拟线程 |
| classloader 只有在其加载的所有类不可达后才能回收；静态引用、残留线程、JNI 全局引用都会 pin 住它，最终 `OutOfMemoryError: Metaspace` | morling.dev《Class Unloading in Layered Java Applications》等 | 反复 load/unload 是 rutis 的常态，无泄漏卸载是 SDK 一等义务（D9）+ 专门的 soak 测试 |
| JetBrains 官方不推荐 Kotlin scripting 作通用脚本方案（2024-11《State of Kotlin Scripting》）；JSR-223 Kotlin 引擎每次 eval 秒级 | blog.jetbrains.com/kotlin | `.kts` 不作插件载体（D11） |
| K2 编译器提速约 2 倍、Gradle 增量编译默认开启 | JetBrains 官方基准、Kotlin Gradle 文档 | dev 模式“改代码到可重载”压到秒级，依赖常驻 Gradle/Kotlin daemon |
| GraalVM native image 是封闭世界假设，与运行时加载新代码根本冲突 | GraalVM 兼容性指南 | 排除（启动优化改用 AppCDS，K5 可选） |
| Kotlin/Native 只能 `-produce dynamic` 导出 C ABI，不能产出供 Rust 静态链接的库；cinterop 仍在 `@ExperimentalForeignApi` | kotlinlang.org native 文档 | 排除直链方案（附录 A）；JVM 生态是 Kotlin 的主要价值 |
| FFM（JEP 454）只解决 Java→native 方向；JDK 24 起 JNI/FFM 默认警告、未来默认拒绝（JEP 472） | openjdk.org/jeps/454、/472 | 本设计不用 JNI/FFM；若将来嵌入式路线启动，参数与许可成本都在附录 A 列明 |
| 跨进程同步调用往返：Node 实测约 30 µs，Python 25–60 µs | 总体稿 §八 | 预期 JVM 同量级或更好；上线前实测（§14） |

## 3. 运行时模型

```text
rutis 宿主
 ├─ LocalRuntime::node / ::python / ::bun / ::go ...
 ├─ LocalRuntime::kotlin(project)（名 "kt"）
 │    └─ JVM 进程（叶子 SDK + 运行时契约）
 │         ├─ kt:weather（一个 jar，一个 classloader）
 │         ├─ kt:tools.ping
 │         └─ kt:analytics（另一个 jar）
 └─ LoaderPlugin + 每个运行时一个 RuntimeRowsPlugin
```

- 行依赖 `RuntimeRows#kt` 加上插件 `inject` 的每个名字（叶子运行时：rutis 门控全部名字，同 Python）。
- `provides` 的服务从行的 fiber 发布到 `host_key(name)`，登记进 loader 服务名目录。
- `isolate`、实例标签、服务 id（`name` + NUL + label）、导出 handle 世代：全部同 Python。
- 进程退出：本运行时的全部行停止，使用其服务的行随后停止；不自动重启。
- 同一进程内插件互相调用拿对象本身，不过 wire；跨进程/跨语言按名字走 rutis。

## 4. 进程

### 4.1 启动命令

```text
java -cp <rutis-kotlin 运行时 jar> arcships.rutis.runtime.Main <channel> [--id <endpoint>] [--peer <endpoint>] <project>
```

| 项 | 规则 |
| --- | --- |
| 参数 | 与 Python 运行时完全一致（`python3 -m rutis <channel> … <project>`） |
| 工作目录 | `project`（项目的 Gradle 根） |
| classpath | 运行时 jar（含 kotlin-stdlib、kotlinx-coroutines-core、kotlin-reflect，fat jar）+ 项目 classpath（见 §8.1） |
| JVM 选项 | 默认；不传 `-Xrs`（子进程自己的 SIGTERM shutdown hook 用于优雅 dispose）；`--enable-native-access` 不需要（无 native 调用） |
| 预检 | 宿主先跑 `java -version`，不达 21 报错并说明需要什么（同 `python -c "import rutis"` 预检） |

### 4.2 通道

| 通道 | 方式 |
| --- | --- |
| `<socket path>`（Unix，默认） | 宿主在私有目录建 socket 并把路径作为 channel 参数传入；运行时 `SocketChannel.open(UnixDomainSocketAddress)` 回连 |
| `tcp:127.0.0.1:<port>`（Windows） | 先发 `RUTIS_CHANNEL_TOKEN` + 换行，随即从环境删除，同 Python `__main__.py` |
| `listen:ws://…` | 远程运行时（D18，后续阶段） |
| `fd:3` | 不支持（D4）；配置里出现即启动报错说明原因 |

帧：换行分隔紧凑 JSON，单帧上限 16 MiB，超限关通道（与 #173 一致）。

## 5. 契约

与 Python/Bun/Go 同一套：compat 握手（协议 2），能力 `["sync-wait", "reentrant-sync"]`（不声明 `sync-stack`，不声明 `objects`——能发对象引用但不能收）；功能 `FEATURES = ["rows.v2", "hosts", "leaf", "scopes"]`。

`mount` 回复：`{ services: {}, features, implementation: { name: "io.github.arcships:rutis", version }, engine: { name: "kotlin", version: <Kotlin 版本> , vm: <JVM 版本> } }`（`engine` 增加字段不算协议变化，同 Bun §3.3；`check` 打印）。

| 控制操作 | 行为 |
| --- | --- |
| `mount` | 见上 |
| `dispose` | 卸载全部行，等待在途调用排空 |
| `rows.load(key, entry, config, isolate, inject, exports)` | 解析 `entry`（§8.2）→ 建 child-first classloader → 解码配置 → 跑 `apply`；失败则卸载该行再报错 |
| `rows.update(key, config)` | 卸载 + 重新加载（叶子无 volatile 字段） |
| `rows.unload(key)` | 先撤 provides（发 `service(id, None, version)` 通知）→ 取消行协程作用域 → 逆序跑 cleanup → `close()` classloader |
| `rows.schema(entry)` | `{ config, inject, provides, version }`；未知 entry 报错并列出可用的名字 |
| `hosts.provide(name, methods, label?)` / `hosts.withdraw(id)` | 登记/撤销 rutis 服务代理（`scopes` 规则同 Python） |
| `release` / `get` | 归还导出对象 / 读导出字段 |

- `ctx.provide` 触发导出槽换新 handle（`name` 第 1 代，`name#N` 之后；scoped id `name\0label`），`service(id, handle, version)` 通知先于 `rows.load` 回复发出。
- 调用号前缀沿用 `node:`（总体稿 §三），调用链 `path` 跨会话由 Rust 侧 rebase。

## 6. 线程模型：同步与可重入

```text
JVM 进程
 ├─ 读线程（平台线程）：阻塞读帧，解析后派发
 ├─ 每个进来的 invoke/call/get/await/控制操作：一个新虚拟线程
 ├─ 一个写锁：帧串行写出
 └─ 行协程作用域：ctx.launch 的任务跑在虚拟线程上，卸载时取消并 await
```

| 项 | 规则 |
| --- | --- |
| 出站同步调用 | 当前虚拟线程阻塞在 `CompletableFuture` 上；读线程完成它。阻塞虚拟线程是廉价的，不用泵任何循环 |
| 可重入性 | 进来的调用任何时候都能跑（总有新虚拟线程）；声明 `reentrant-sync`。同 Python/Go 的代价：**插件的服务可能在自己同步调用途中被再次调用，调用 rutis 服务时不要持有锁** |
| 能力组合 | `sync-wait`（出站同步调用带 `sync: true`，参与宿主等待环检测）+ `reentrant-sync`；**不声明 `sync-stack`**——同步等待期间进来的调用并发跑在别的虚拟线程上，不是压栈（Go 的 goroutine 同款；只参与调用链边） |
| `SyncWaitCycle` | 本地永不产生（读线程不跑插件代码，进来的调用不缺执行单元）；收到时按远程错误呈现 |
| 取消 | `cancel` 帧 → 取消该调用的协程作用域 + 中断其虚拟线程（阻塞 I/O 可被中断；不可中断的本地计算照常跑完，结果丢弃——诚实呈现） |
| 异常 | 服务方法抛异常 → 错误图（name/message/graph）返回；`apply` 抛异常 → 加载失败，已注册 cleanup 照跑；插件自己起的平台线程未捕获异常 → 进程退出，全部行停止（Bun §3.6 / Go §8.3 同款） |
| 插件义务 | 提供的服务对象并发安全（可能同时被多个调用用）；不跨 rutis 调用持有锁 |
| suspend 桥接 | 方法是 suspend 函数时，在其虚拟线程上 `runBlocking`；`apply`、cleanup 同理 |

**对 Go 设计提议的回答**（总体稿 §九的悬案）：Kotlin 支持把“每个叶子运行时可重入、只有 Cordis 运行时不可重入”定为规则——虚拟线程使可重入在 JVM 上是自然且廉价的形态。Swift（M3）仍待其自己的设计确认；确认后把结论回记总体稿 §九。

## 7. Kotlin SDK

### 7.1 一个插件

```kotlin
// weather/src/main/kotlin/weather/WeatherPlugin.kt
package weather

data class Config(val city: String? = null)

interface Llm {
    suspend fun ask(question: String): String
}

class Weather(private val llm: Llm, private val city: String) {
    suspend fun today(): String = llm.ask("weather in $city") + " in $city"
    fun unit(): String = "celsius"
}

val plugin = rutis.definePlugin(
    name = "weather",
    inject = listOf("llm"),
    provides = rutis.provides("weather" to rutis.methodsOf<Weather>(sync = "unit")),
    config = rutis.schemaOf<Config>(),
) { ctx, config ->
    val llm = ctx.use<Llm>("llm")
    ctx.provide("weather", Weather(llm, config.city ?: "Oslo"))
}
```

| API | 规则 |
| --- | --- |
| `definePlugin(...)` | 返回带 `api = 1` 标记的定义（D14）；`api` 比运行时新则拒载并报两边的版本 |
| `apply(ctx, config)` | 可为 suspend；抛异常则加载失败，已注册 cleanup 照跑 |
| `ctx.use<T>(name)` | `name` 必须在 `inject` 里；同进程拿到的是对象本身（接口随意，同进程直调不做签名转换），跨进程是代理 |
| `ctx.provide(name, value)` | 直到卸载或调用返回的撤销函数；`value` 的类型必须匹配 `provides` 声明 |
| `ctx.effect(cleanup)` | 卸载时逆序执行，可为 suspend |
| `ctx.launch { }` | 在行协程作用域的虚拟线程上跑；卸载时取消并 await（Go `ctx.Go` 对应物）；未捕获异常按 §6 处理 |
| `rutis.NoConfig` | 无配置 |

### 7.2 方法与形状

| 项 | 规则 |
| --- | --- |
| 服务方法 | `provides` 声明的类型的公开成员函数（含 suspend） |
| 命名 | 方法名原样（Kotlin 惯例已小写开头）；`@RutisName("...")` 覆盖；冲突在 `definePlugin` 报错 |
| 签名 | 参数按位置解码，多了报错、少了用默认值；返回值即结果 |
| 形状 | `methodsOf<T>(sync = ...)` 默认 `async`，列出的名字是 `sync`（D7） |
| `async` 执行 | 虚拟线程上跑，异步结果引用回复；调用方取消传给 suspend 函数（协程取消）+ 线程中断 |
| 错误 | 异常按错误图返回；名字取异常类简名 |

### 7.3 配置 schema

| Kotlin | JSON Schema |
| --- | --- |
| `String` / `Boolean` / `Int`、`Long` / `Double` | `string` / `boolean` / `integer` / `number` |
| `List<T>`、`Array` | `array` + `items` |
| `Map<String, T>` | `object` + `additionalProperties` |
| data class | `object` + `properties`；无默认值的构造参数是 `required` |
| `@Doc("…")` | `description` |
| `rutis.schemaOf<T>()` | 反射派生（运行时内置 kotlin-reflect；插件编译期不需要） |
| `Plugin.schema` | 显式给定，覆盖派生（Go §6.3 对应物） |

配置由 kotlinx 序列化规则解码到 data class；失败即加载失败。

### 7.4 值

| Kotlin 值 | 跨进程 |
| --- | --- |
| `null`、`Boolean`、`String` | 复制 |
| `Int` / `Long` / `Double` | 复制；超出 ±2^53−1 的整数是错误 |
| `List<*>`、`Map<String, *>`、data class | 递归复制（按 JSON 规则） |
| 异常 | 名字与消息复制 |
| 函数（`(A) -> B`、suspend 函数） | 按引用 |
| `rutis.Ref(v)` | 按引用；代理调用 `v` 的公开方法 |
| `ctx.provide` 给出的值 | 总是按引用 |
| 收不到的 | `undefined`（解码为专用 `Undefined` 值而非 null； Kotlin 侧 `null` 编码为 null） |

## 8. 装载与热重载

### 8.1 插件 jar 与 classpath

- 插件 = 一个 jar（Gradle `jar` 产物；依赖多的用 fat jar，如 shadow）。项目可以放多个插件 jar。
- 项目 classpath：`project` 下的约定目录（如 `build/libs/` 与 `libs/`）+ 运行时 jar；也可由 `rutis.json`/配置显式列出（决定于 K2 验证哪种最少惊讶）。
- 每个插件一个 child-first `URLClassLoader`（父 = 运行时 jar 的 loader，含 kotlin-stdlib/SDK——插件与运行时共享一份，避免重复类）；项目依赖 jar 放父层，插件 jar 放子层。

### 8.2 entry 解析

- `kt:weather.WeatherPlugin`（含点）→ 直接当类全名，加载后取 `plugin`/`Plugin` 声明。
- `kt:weather`（不含点）→ 扫描 classpath 各 jar 根部的 `rutis.json` 描述符（§8.3），按插件名匹配；多个匹配报歧义并建议全名形式。

### 8.3 `rutis.json` 描述符

jar 根部一份，模板生成（§11），也可手写：

```json
{
  "rutisPlugin": 1,
  "name": "weather",
  "version": "1.2.0",
  "entry": "weather.WeatherPlugin",
  "inject": ["llm"],
  "provides": { "weather": { "today": "async", "unit": "sync" } },
  "config": { "type": "object", "properties": { "city": { "type": "string" } } }
}
```

- K2 阶段只由运行时读它应答 `rows.schema` 与 entry 解析；Rust 侧离线读 jar 解析（Go D5 的收益，zip 读一个成员即可）列为 K5 优化。
- 描述符与加载后声明的 `provides`/`inject` 不一致时，以运行时实际声明为准并警告。

### 8.4 热重载与卸载

- dev 模式：jar 文件 mtime+size 变化（同 Python 模块戳）→ 丢弃旧 classloader，建新的重新 `apply`。只换该插件的 loader；父层依赖 jar 不重载（要换依赖就重启运行时）。
- 卸载三步：撤 provides → 取消行作用域并 await（虚拟线程任务）→ 逆序 cleanup，然后 `close()` loader 并丢掉全部强引用，交给 GC。
- **无泄漏义务**（SDK 文档 + soak 测试）：`ctx.launch` 的任务一定随卸载结束；插件自己起的平台线程必须自己停（SDK 无法回收，同 Go 的“忽略 ctx.Done 的 goroutine”）；不要把插件类注册进任何全局注册表（日志框架、shutdown hook 等——运行时在行卸载后清理自己登记的 hook）。
- soak 验收：200 次 load/unload 循环后 Metaspace 用量稳定（GC 后回落）；泄漏即缺陷。
- DCEVM/JBR 保持状态的热替换：dev 可选项，不进默认分发（依赖非标准 JVM）。

## 9. 仓库与分发

| 项 | 规则 |
| --- | --- |
| 位置 | `kotlin/rutis`（Gradle 项目，模块名 `io.github.arcships:rutis`） |
| 包 | `arcships.rutis`（SDK：`definePlugin`、类型、testing）；运行时入口 `arcships.rutis.runtime.Main`；会话层 `arcships.rutis.internal.peer` |
| 依赖 | SDK 编译期只依赖 kotlin-stdlib；运行时 jar 另带 kotlinx-coroutines-core、kotlin-reflect（fat jar） |
| 发布 | Maven Central（Sonatype Central Portal；`io.github.arcships` 命名空间用 GitHub 组织验证）；版本进 `scripts/train.mjs` |
| 为什么单 artifact | Python 同理：运行时无第三方编译期耦合，插件作者装 SDK 不被强加东西（D13） |
| `io.github.arcships.rutis.testing` | 无宿主测试助手：`load(plugin, config, services)` + 严格模式（跨进程值语义模拟），同 Go `rutistest` |
| JDK 支持 | 基线 21；在 21 与最新 LTS（25）的 CI 矩阵上测 |

## 10. Rust 侧改动

| 改动 | 位置 |
| --- | --- |
| `Launcher::kotlin(java, runtimeJar, project)`：`java -cp <runtimeJar> arcships.rutis.runtime.Main`，cwd=project，**不** `inherit_fd`（→ 回连）；Windows loopback | `rutis-bridge/src/runtime/process.rs` |
| `LocalRuntime::kotlin(project)`，名 `"kt"`；`.java()` 换解释器 | `rutis-bridge/src/runtime/local.rs` |
| feature：rutis-bridge `kotlin = []`；rutis-loader `kotlin = ["runtimes", "rutis-bridge/kotlin"]` | 两个 `Cargo.toml` |
| 行解析复用 `RuntimeResolver::modules`（前缀 `kt:`，不缓存，二段刷新）；远程 `Naming::Npm` 拒绝 `kt:` 前缀已有通用规则 | 无新代码（核对即可） |
| `RuntimeRowsPlugin` 已参数化为 trait | 无改动 |
| rutis-host：`config.rs` `Runtimes` 加 `kotlin` 字段（`project`/`java`/`runtimeJar`）；`host.rs` 挂载分支 + `java -version` 预检；远程语言映射加 `"kotlin"|"kt"` | `rutis-host/src/*` |

## 11. rutis-host

```json
{
  "runtimes": { "kt": { "project": "." } },
  "rows": [{ "id": "weather", "name": "kt:weather", "config": { "city": "Oslo" } }]
}
```

| 字段 | 默认 | 含义 |
| --- | --- | --- |
| `project` | `.` | Gradle 项目根，也是工作目录；插件 jar 从这里解析 |
| `java` | `JAVA_HOME` → `PATH` | JVM 可执行文件 |
| `runtime` | 项目依赖里的运行时 jar（Gradle cache），否则 `~/.rutis/kotlin/rutis-<version>.jar` | 运行时 fat jar 位置 |

| 命令 | 行为 | 阶段 |
| --- | --- | --- |
| `check` | 列出 JVM 版本、SDK/插件 API 版本、每个 jar 的插件与版本、歧义、每行的 jar | K3 |
| `dev` | 无 `rutis.dev.json` 时，`build.gradle.kts` 存在即 Kotlin 项目：监听 `.kt`/`build.gradle.kts` → `gradle -q jar`（项目自己的构建，宿主不编译）→ 成功后触发该 jar 的行重载；失败打印错误，旧代码继续跑 | K4 |
| `new <name> --lang kotlin` | Gradle 项目模板：`build.gradle.kts`（kotlin("jvm")、依赖 `io.github.arcships:rutis`）、`src/main/kotlin/...` 插件、`rutis.json`、测试（`rutis.testing`）、`rutis.dev.json`、发布 workflow（tag → `gradle test`、`check`、publish to Central） | K4 |
| `run` | 不需要 JDK 之外的工具链（Gradle wrapper 由模板带） | — |

## 12. 测试

| 测试 | 位置 | 内容 |
| --- | --- | --- |
| 会话单元 | `kotlin/rutis` 内部 | 编码、引用计数、同步调用中的回调、取消、调用链上下文、并发 |
| 会话 conformance | `rutis-bridge/tests/runtime_conformance.rs` + Kotlin `conformance` 入口 | `session::testing::session` 全套（引用、回调、错误、malformed 帧终结会话） |
| 契约 × 通道 | `session_matrix.rs` 加 `Kotlin` | `runtime::testing::runtime`：socket 回连；Windows loopback |
| Python 测试的 Kotlin 版 | `kotlin_runtime.rs`（bridge）、`kotlin_rows.rs`（loader，含热重载与 Metaspace soak） | describe、导出加载、host 租约、同步调用中回调、unload、行重载 |
| 现有矩阵加列 | `cancellation.rs`、`error_shape.rs`、`rpc_callbacks.rs`、`process_exit.rs`、`live_objects.rs`、`sync_cycles.rs`（kt+py、kt+node 组合） | 各语言的同一组断言 |
| runtime conformance | `multilang.rs`（feature `kotlin`） | Kotlin 叶子插件；多语言冷启动互调；撤提供者只停使用者；`inject` 门控 |
| SDK 单元 | `kotlin/rutis` 的 `testing` helper | 无宿主加载、严格模式值语义 |
| CI | `.github/workflows/ci.yml` | job 先 `gradle build` kotlin/，再跑 cargo 测试；JDK 21 与 25 矩阵；`train.mjs` 版本一致性加 maven 坐标 |

## 13. 阶段

| 阶段 | 工作 | 验收 |
| --- | --- | --- |
| K1 研究验证 | §14 清单逐项实测：虚拟线程重入原型、回连 socket 帧往返延迟、URLClassLoader 循环卸载的 Metaspace 行为、Gradle 增量编译耗时、JDK 发现矩阵 | 结论回填本文档；不达标处修订设计 |
| K2 会话层 | `internal/peer` 全量 + conformance 入口 | `kotlin_meets_the_session_contract` 通过 |
| K3 运行时 + SDK + Rust 接入 | 契约操作、加载/卸载、投影；`Launcher::kotlin`、features、host 挂载 | `kotlin_runtime.rs`/`kotlin_rows.rs`/`multilang.rs` Kotlin 列通过；与 Python 同一套兼容性测试 |
| K4 开发流程 | 模板、`dev`、`check`、Maven Central 发布、train.mjs | `rutis-host new --lang kotlin` 到 `dev` 热重载全流程；依赖坐标可解析 |
| K5 优化与远程 | `listen:ws`（协议 3 + `signals`）、Rust 离线读 `rutis.json` 解析、on-demand/idle 评估、AppCDS | 远程 Kotlin 运行时过 `websocket_conformance`；离线解析不启动进程 |

## 14. K1 验证清单（实现前实测）

1. **同步往返延迟**：回连 unix socket 上 Python 基准测试的移植（期望 25–60 µs 量级）。
2. **虚拟线程重入**：同步调用途中进来的调用在新虚拟线程执行；`synchronized` pin 在 21 与 24 上的对比。
3. **classloader 卸载**：200 次 load/unload 循环的 Metaspace 曲线（含 coroutines、kotlin-reflect 在父层的情况）。
4. **冷启动**：JVM 21/25 + 首个插件加载到 `mount` 回复的耗时；AppCDS 前后对比。
5. **dev 循环**：常驻 Gradle daemon 下单文件改动的 `jar` 增量耗时（期望秒级）。
6. **JDK 发现**：`JAVA_HOME`/PATH/`java` 缺失/版本不足在 macOS、Linux、Windows 的错误呈现。

## 15. 开放问题

- **on-demand/idle**（D19）：JVM 常驻内存（数十至一二百 MB）比 Python 重；是否照 Go 的 `GoRuntimes` 做“用才启动、闲置即停”，K5 用数据决定。
- **schema 派生**：反射派生（D20）与 kotlinx-serialization 的 schema 模块二选一还是并存（后者对 `@Serializable` 用户更惯用，但多一个编译期依赖）。
- **项目 classpath 约定**（§8.1）：`build/libs` vs `libs` vs 显式清单，K2 定。
- **发布基建**：train.mjs 无 maven 先例；Central Portal 的 namespace 验证与签名配置。
- **21–23 的 `synchronized` pin**：文档警告（现状 D3）还是把基线提到 24/25（放弃仍在支持期的 21 LTS）。
- **Swift（M3）**：本设计对“叶子运行时可重入”投了赞同票；Swift 设计确认后回记总体稿 §九，若推翻则重审本条。

## 附录 A：为什么不走嵌入式与 Kotlin/Native

**嵌入式 JVM（JNI Invocation API / jni crate / FFM）**：技术上成熟（`JNI_CreateJavaVM`），但与“宿主稳定性优先”直接冲突——插件 native 崩溃、堆 OOM、GC 停顿、JVM 信号处理器（`SIGSEGV` 隐式空指针检查、shutdown hook；`-Xrs`/`-XX:+UseSignalChaining` 才能与宿主共存）全部进入 Rust 宿主进程；JEP 472 后 JNI/FFM 默认警告、未来默认拒绝，宿主还得管理 `--enable-native-access`；JNI 线程 attach/detach、local/global 引用配对是持久的复杂度。跨进程同步调用约 30–60 µs，对插件服务调用的语义足够；只有出现“微秒级同步调用”的硬需求才值得重开此路（且届时 FFM 只解决 Java→native 方向，Rust→JVM 仍只有 Invocation API）。

**Kotlin/Native 直链 rutis core**：只能以 `-produce dynamic` + `@CName` 导出 C ABI，不能产出供 Rust 静态链接的库；cinterop 处于 `@ExperimentalForeignApi`；最关键是丢掉 JVM 生态——对“agent 写插件”场景，生态是主要价值。若将来出现“单二进制、无 JVM”的需求，方向应是“Rust staticlib 嵌入 Kotlin/Native 可执行文件”作为附加 target，而非替代本设计。

**GraalVM native image**：封闭世界假设与运行时加载插件根本冲突，排除；启动优化用 AppCDS（K5）。
