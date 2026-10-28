# rutis for Kotlin

[English](README.md)

用 Kotlin 写 [rutis](https://github.com/arcships/rutis) 插件。SDK（`kotlin/rutis`，Maven 坐标 `io.github.arcships:rutis` 与 `io.github.arcships:rutis-ksp`，随 rutis 发布线）是 Kotlin Multiplatform 库，两个 target：**android**，供托管插件的 Android 应用（K-P2 阶段）；**jvm**，在桌面上运行同一份代码：一致性测试与测试入口 `Rutis.serve`。设计：[docs/design-android-runtime-2026-10-12.md](../docs/design-android-runtime-2026-10-12.md)。

状态：K-P2。插件、服务、值、错误、统一事件与测试工具在两个 target 上可用，与其他运行时过同样的一致性套件；应用用 `RutisApp` 接入 rutis（WebSocket 拨出与监听），在 Android 模拟器上测过（含 R8 的 release 构建）。随 0.15 起发布到 Maven Central。

```kotlin
import arcships.rutis.*
import kotlinx.serialization.Serializable

@Serializable
data class Config(val city: String)            // 配置的 JSON Schema 由它推出

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
    provides = mapOf("weather" to service<Weather>(sync = setOf("unit"))),   // 可选
) { ctx, config ->
    ctx.provide("weather", Weather(ctx.use<Llm>("llm"), config.city))
    ctx.on("session/created") { args -> println("new session ${args[0]}"); null }
    ctx.effect { /* 插件卸载时运行，后注册的先运行 */ }
}
```

构建里加序列化插件和处理器。处理器在编译期为 `@RutisService` 类生成方法表、为 `@RutisClient` 接口生成代理（运行时不用方法反射；AAR 带保留它们的 R8 规则）：

```kotlin
plugins {
    kotlin("plugin.serialization")
    id("com.google.devtools.ksp")
}
dependencies {
    implementation("io.github.arcships:rutis:<版本>")
    ksp("io.github.arcships:rutis-ksp:<版本>")
}
```

## API

| | |
| --- | --- |
| `Rutis.define<C>(name, inject, provides?, schema?) { ctx, config -> }` | `C` 是 `@Serializable` 类型（无配置用 `Rutis.NoConfig`）。`provides` 可选：没写时 `ctx.provide` 提供的服务自动对外，方法全为 `async`；`service<T>(sync = setOf(...))` 把方法标成 `sync`。插件 API 3 |
| `ctx.use<I>(name)` | `I` 是 `@RutisClient` 接口，`name` 在 `inject` 中。同进程提供者的对象实现了 `I` 时得到对象本身，否则得到代理。`suspend` 函数挂起等待，普通函数阻塞本线程 |
| `ctx.provide(name, obj)` | `obj` 是 `@RutisService` 类的实例：它的 public 成员函数（含父类的）就是方法，按函数名；至插件卸载或调用返回的函数为止 |
| `ctx.effect { }` | 清理，插件卸载时逆序运行 |
| `ctx.on(name, prepend, global) { args -> }` | 监听器：`args` 是解码成原生值的事件参数，waterfall 中最后一个是 `args.next`；返回值在 `serial`、`bail`、`waterfall` 中算结果 |
| `ctx.emit` / `parallel` / `serial` / `waterfall` / `bail`，`ctx.filter { d -> }` | 五种发出方式：`emit` 立即返回，`bail` 阻塞，其余三种挂起；`parallel` 的失败是名为 `AggregateError` 的 `RutisException` |
| `ctx` 本身 | 插件的协程作用域：`ctx.launch { }` 带着调用链，插件卸载时取消并等待 |

错误：插件抛普通异常；有 public `code: String`、`data` 属性的异常带着它们跨过去。收到的错误和 SDK 自己的错误（`AggregateError`、`SyncWaitCycle`、`ReservedEvent` 等）都是 `RutisException`，按 `name` 匹配。

值：由 SDK 自己的 kotlinx-serialization 格式 `RutisFormat`（`arcships.rutis.advanced`）承载，不用 JSON 的默认编码。`java.time.Instant`、`kotlin.time.Instant` 是时间点，`LocalDateTime` / `LocalDate` / `LocalTime` 是本地日期时间 / 日期 / 时刻，`ByteArray` 是二进制，键不是字符串的 `Map` 是 Map，`Set` 是 Set，`BigInteger` 是大整数（超出 ±(2^53−1) 的任何整数也是），`BigDecimal` 是 Decimal（保留标度），`UUID`、`kotlin.uuid.Uuid` 是 UUID。`@Serializable` 类里这些 JDK 类型的字段标 `@Contextual`（kotlinx 的规则）。类型为 `Any?` 的参数收到原生值：`Long`、`Double`、`Map<String, Any?>`、`List<Any?>` 等。`Rutis.define` 的结果 `Plugin` 也能作为参数和返回值（客户端、服务类里类型写 `Plugin`），按插件定义传，收的一方挂它（[插件 API](../docs/guide/plugin-api.md#插件定义)）。

线程：每个进来的调用一个线程，`suspend` 方法在其上 `runBlocking`。调用链放在 `ThreadLocal`，随协程传递（含 `withContext` 换线程），插件代码不传上下文。调用被取消时取消方法的协程并中断其线程。

## 测试插件

`arcships.rutis.testing` 在插件真实的运行时里、经真实的（内存）会话运行插件，测试一方是宿主：

```kotlin
RutisTest.load(weather, Config("Oslo"), services = mapOf("llm" to FakeLlm())).use { loaded ->
    assertEquals("celsius", loaded.service("weather").call("unit"))
    loaded.parallel("session/created", "s1")
}
```

测试给的服务是 `@RutisService` 类的实例。`loaded.on`、`emit`、`parallel` 等以宿主身份监听和发出；`loaded.load(config, row, scope)` 装载更多行；`loaded.reported()` 是运行时报告的行。

## 在应用里托管插件

应用用 `RutisApp` 把它带的插件交给 rutis（设计 §五）：连上后应用自己按顺序装上它们，各带应用给的配置（`configs`，按插件名；不给时用 Schema 的默认值），它 inject 的服务都在时才装上，rutis 把它们注册为 `<id>:<插件>`。

```kotlin
val rutis = RutisApp(
    plugins = listOf(contacts(context), notify(context), device(context)),
    configs = mapOf("notify" to NotifyConfig(channel = "rutis sample")),
    connect = "wss://rutis.example.com:7443/rutis",   // 或 listen = "wss://0.0.0.0:7443/rutis", certificate = pem
    id = "kim-pixel",                                 // 这条连接的 id：rutis 上注册为 kim-pixel:contacts 等
    token = secrets.rutisToken,
)
rutis.start()   // 立即返回；连上后装上插件；断线自动重连，直到 stop()
rutis.stop()    // 撤回服务，全部插件逆序清理，然后断开；之后可再 start()
rutis.state     // StateFlow<RutisState>：Stopped(reason) / Connecting(retrying) / Connected(address)
```

何时 start / stop（跟随界面、在前台服务里）是应用的业务；SDK 不据 `state` 做任何决定。`trust` 是系统之外额外信任的 CA（PEM），用于自签证书的接入点。等待 rutis 的调用（`@RutisClient` 接口的非 `suspend` 方法）在主线程上被拒绝：用 `suspend` 调用或换个线程。rutis 一侧用一行 `rutis-host/listen` 让应用连进来（[rutis-host](../docs/guide/rutis-host.md)）。`rutis/android-sample` 是这样做的应用，设备测试也用它。完整的指南（权限、前台服务、电池优化、真机清单）：[Android 应用托管插件](../docs/guide/kotlin-app.md)。

## 桌面测试入口

jvm target 上，`Rutis.serve(args, plugins…)` 把插件作为 rutis 运行时运行：`<channel> --id <id> <project>`，以 `<id>` 握手；通道是 Unix socket 路径（`unix:<path>` 或裸路径）或 `tcp:127.0.0.1:<port>`（先发 `RUTIS_CHANNEL_TOKEN`）。Rust 一致性测试经 rutis-bridge 的通用 `Launcher` 以 `java -jar` 启动它（不在 rutis-host 里配置本地 Kotlin 运行时）。

远程通道与其他语言相同（[连接节点](../docs/guide/nodes.md)）：`dial:wss://… --id <id> --peer <rutis 的 id>` 拨出到 rutis 的接入点并保持连接，`listen:wss://… --id <id>` 等 rutis 拨入；`RUTIS_TOKEN`、`RUTIS_CA`（拨出时额外信任的 CA）、`RUTIS_CERT` / `RUTIS_KEY`（监听 `wss://` 的证书与私钥，PEM：PKCS#8、PKCS#1 或 SEC1），重连规则与状态行同 [拨出与动态接入](../docs/design-remote-runtime-dial-2026-10-14.md) §六，SIGTERM / Ctrl-C 卸载全部行后以 0 退出。rutis 一侧 `runtimes.remote[].language` 写 `kotlin`（或 `kt`）。拨出用 OkHttp，监听用 SDK 自带的 WebSocket 服务端；两者都在共用源码里，Android 应用用的是同一份代码。

## 构建与测试 SDK

JDK 17。不需要安装 Gradle：仓库里有 wrapper。

```sh
cd kotlin/rutis
./gradlew build          # SDK、处理器、单元测试、测试工具对两份期望文件
```

android target 只在装有 Android SDK 时构建（`ANDROID_HOME`，或 `local.properties` 的 `sdk.dir`）。带 Kotlin 的 Rust 套件用 feature `kotlin`，构建 `rutis-conformance/build/libs/rutis-conformance-all.jar`（或用 `RUTIS_KOTLIN_JAR` 指定的）：

```sh
cargo test -p rutis-bridge --features testing,websocket,kotlin --test runtime_conformance --test value_roundtrip --test session_matrix kotlin_
cargo test -p rutis-bridge --features testing,websocket,kotlin --test dial_conformance kotlin::
cargo test -p rutis-bridge --features testing,websocket,kotlin --test kotlin_remote
cargo test -p rutis-bridge --features testing,kotlin --test kotlin_runtime
cargo test -p rutis-loader --features node,python,kotlin --test events_conformance --test multilang_kotlin
```

在 Android 设备或模拟器上（`adb`；`tools/android-emulator.sh start` 起一个）：SDK 的设备测试，以及示例应用拨出到 rutis-host（它的另一行要 Python 3.10+）：

```sh
./gradlew :rutis:connectedDebugAndroidTest
cargo test -p rutis-host --features android --test android -- --test-threads 1
```
