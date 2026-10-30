# Android 应用托管插件（Kotlin）

[English](kotlin-app.en.md)

Android 应用把自己和所在设备的能力（通讯录、通知、设备信息……）作为 rutis 插件提供：插件用 Kotlin 写、编进应用，应用连上 rutis 后自己按顺序装上它们，各带应用给的配置；它们提供的服务在 rutis 上以 `<id>:<插件>` 出现。设计见 [Android 应用托管](../design-android-runtime-2026-10-12.md) 与 [应用接入](../design-app-integration-2026-10-14.md)。

`kotlin/rutis/android-sample` 是一个完整的示例应用（`contacts`、`notify`、`device` 三个插件，`notify` 的配置在 `RutisApp` 里给出，一个开始 / 停止的界面），下文的代码都取自它。

## 1. 依赖

Android 10（API 29）或更高，JDK 17。Kotlin SDK 不发布到 Maven，从源码引用：把本仓库的 tag `v0.19.1` 取到应用项目旁边，再把应用的 `local.properties`（Android SDK 的位置）复制给它：

```bash
git clone --depth 1 --branch v0.19.1 https://github.com/arcships/rutis.git ../rutis
cp local.properties ../rutis/kotlin/rutis/
```

`kotlin/rutis` 要自己找到 Android SDK（`ANDROID_HOME`、`ANDROID_SDK_ROOT` 或它的 `local.properties` 里的 `sdk.dir`）才构建 Android 的那份；找不到时应用拿到的是桌面 JVM 的那份，没有 R8 规则。

应用的 `settings.gradle.kts` 用 Gradle 的 composite build 引入它（路径相对应用项目的根目录）：

```kotlin
includeBuild("../rutis/kotlin/rutis")
```

应用的 `build.gradle.kts`：

```kotlin
plugins {
    id("com.android.application")
    kotlin("android")
    kotlin("plugin.serialization")
    id("com.google.devtools.ksp")
}

dependencies {
    // 不写版本：Gradle 用 includeBuild 引入的源码构建它们。
    implementation("io.github.arcships:rutis")
    ksp("io.github.arcships:rutis-ksp")
    // 测试工具，在 JVM 上跑插件的单元测试（§7）。
    testImplementation("io.github.arcships:rutis-testing")
    kspTest("io.github.arcships:rutis-ksp")
}
```

升级时在 `../rutis` 里取新的 tag（`git fetch --depth 1 origin tag v<版本> && git checkout v<版本>`）。

KSP 处理器在编译期生成方法表与代理，运行时不做方法反射；AAR 带了 R8 规则，开 `isMinifyEnabled` 的 release 构建不用为 rutis 加任何规则。`AndroidManifest.xml` 要有 `android.permission.INTERNET`。

## 2. 写插件

插件是普通 Kotlin：一个 `@RutisService` 类、一个 `Rutis.define`。完整的 API 见 [kotlin/README](../../kotlin/README.zh-CN.md) 与 [插件 API](plugin-api.md)。

```kotlin
@Serializable
data class DeviceInfo(val manufacturer: String, val model: String, val release: String, val sdk: Int)

@RutisService
class Device(private val context: Context) {
    fun info(): DeviceInfo = DeviceInfo(Build.MANUFACTURER, Build.MODEL, Build.VERSION.RELEASE, Build.VERSION.SDK_INT)
}

fun device(context: Context) = Rutis.define<Rutis.NoConfig>(name = "device") { ctx, _ ->
    ctx.provide("device", Device(context.applicationContext))
}
```

- 插件要 `Context` 时，像上面这样把插件写成一个接 `Context` 的函数，传 `applicationContext`。
- `provides` 不用写：提供的服务自动让 rutis 知道，rutis 上别的行按名字使用。
- 等待 rutis 的调用（`@RutisClient` 接口里的非 `suspend` 方法）不能在主线程上调用，会抛 `IllegalStateException`：在协程里用 `suspend` 方法，或换个线程。rutis 调进应用的方法本来就在后台线程上运行。
- 插件本身也是一个值：`Rutis.define(...)` 的结果（`Plugin`）可以照常作为参数交给一个服务，收的一方在别处挂它，每挂一次是一份。`@RutisClient` 接口、`@RutisService` 类的参数或返回值类型写 `Plugin`：

  ```kotlin
  @RutisClient
  interface Parts {
      suspend fun register(scope: String, part: Plugin)
  }

  val git = Rutis.define<Rutis.NoConfig>(name = "git", inject = listOf("parts")) { ctx, _ ->
      val pool = openPool()
      val status = Rutis.define<StatusConfig>(name = "git_status", inject = listOf("history")) { ctx, config ->
          ctx.provide("gitStatus", GitStatus(pool, config.depth))
      }
      ctx.use<Parts>("parts").register("session", status)
  }
  ```

  每一份在应用的进程里运行。它属于交出它的那一行的这一代：这一行卸载、应用 `stop()` 或断开时，副本先结束。交给 `RutisTest` 的测试替身（同一个运行时里的插件）时，替身收到的就是这个 `Plugin` 本身，可以直接 `RutisTest.load(it, config)`；替身交回时插件拿到的也是原来那个。参数或返回值要直接是 `Plugin`（或 `Plugin?`），`List<Plugin>` 这类放在集合或数据类里的插件定义不支持，要交几个就调用几次。规则见 [插件 API](plugin-api.md#插件定义)。

- 配置：JSON Schema 由 `@Serializable` 类型生成（或用 `schema` 给出）；配置由应用在 `RutisApp` 里给出（§3）。SDK 只解码：没给的键用构造参数的默认值（没有默认值的可空字段为 null），不按 Schema 校验；缺必填的键、值的类型不对时，插件装载失败，按 rutis 的措辞点名这个键：`config: city is required`、`config: days should be an integer, got "x"`（[插件 API：配置](plugin-api.md#配置)）。rutis 装载的行（宿主 `rutis.json` 里的插件、测试的行）由 rutis 先按 Schema 校验、补默认值，运行时再解码；之后解码仍出错时写成 `<路径> is not a value the plugin accepts`。

### 带方法的对象

`@Serializable` 类是数据，复制；`@RutisService` 类的实例按引用交给别的语言：对方拿到代理，调用的是这里的原对象。两个注解都没有的类传不过去：`a Cache cannot cross: annotate it @RutisService to pass it by reference, or @Serializable to copy it`。别的进程交来的对象，参数类型写 `@RutisClient` 接口，或者写 `Any?`：

```kotlin
/** 怎样使用别的进程交来的对象。 */
@RutisClient
interface Cache {
    suspend fun get(key: String): Long?
    suspend fun set(key: String, value: Long)
}

@RutisService
class Warmer {
    suspend fun warm(cache: Cache): Long? {
        cache.set("a", 1)                    // 调用的是对方的原对象
        return cache.get("a")
    }

    /** 不知道是什么时写 Any?：对象是 RemoteObject，按名字调用。 */
    suspend fun peek(value: Any?): Any? = (value as? RemoteObject)?.call("get", "a")
}
```

- 参数类型写 `@RutisClient` 接口时拿到 KSP 生成的代理，和 `ctx.use<Cache>("cache")` 是同一种：`suspend` 方法挂起，普通方法阻塞（不能在主线程上调用）。`@RutisClient` 方法的返回值也一样。
- 参数写 `Any?` 时拿到 `arcships.rutis.advanced.RemoteObject`：`suspend fun call(method, vararg args)` 挂起，`fun callBlocking(method, vararg args)` 阻塞，参数和结果都是普通的 Kotlin 值；`release()` 立即交还引用，不等它被回收。列表、map 里的对象和事件参数里的对象，经 `Any?` 拿到 `RemoteObject`；`List<Cache>` 这类集合里的接口类型不支持。
- 参数类型是数据（`@Serializable` 类、`String` 等）时，这次调用以 `bad-argument` 失败：`argument 1 should be a Point, got an object (an object is received only as a @RutisClient interface or Any?)`。
- 代理上没有数据字段：要数据，让方法返回它。调用对象没有的方法，这次调用以 `no-method` 失败：`Cache has no method gett (methods: get, set, size)`。
- 同一个对象交出几次，这里拿到同一个代理；把收到的代理或 `RemoteObject` 再交出去，发出的是原来那个对象，交回原来的进程，那边拿到的是原对象。
- 对象不跟行走：交出它的行卸载后照常可以调用。应用交出的对象在应用的进程里：应用 `stop()` 或断开后，别处对它的调用以 `peer-gone` 失败；对方的进程退出后，这里的调用同样以 `peer-gone` 失败。
- 同一个进程里，对象的类实现了该接口时拿到的是对象本身，否则是在本进程里调它的代理；`RutisTest` 的测试替身是同一个运行时里的插件，也是这样。规则见 [插件 API](plugin-api.md#带方法的对象)。

## 3. 接入 rutis

```kotlin
@Serializable
data class NotifyConfig(val channel: String = "rutis")

fun notify(context: Context) = Rutis.define<NotifyConfig>(name = "notify") { ctx, config -> /* … */ }

val rutis = RutisApp(
    plugins = listOf(contacts(context), notify(context), device(context)),
    configs = mapOf("notify" to NotifyConfig(channel = "rutis sample")),
    connect = "wss://rutis.example.com:7443/rutis",
    id = "kim-pixel",
    token = token,
)
rutis.start()
```

| 参数 | 含义 |
| --- | --- |
| `plugins` | 连上后按顺序装上的插件 |
| `configs` | 可选：插件的配置，按插件名给出，值是它的配置类型；没给的插件用配置 Schema 的默认值（kotlinx 的默认参数）。值不是那个插件的配置类型、或名字不是 `plugins` 里的插件时，构造 `RutisApp` 就抛 `IllegalArgumentException`：`rutis: plugin notify: … is not its configuration: …`、`rutis: configs names nope, which is not one of the plugins: contacts, device, notify`。配置只在 `RutisApp` 里给：`Rutis.serve` 的插件由 rutis 的行给配置 |
| `connect` | rutis 接入点的地址（推荐：应用在 NAT 后、地址会变）。非回环只能 `wss://` |
| `listen` | 改为等 rutis 拨入，如 `wss://0.0.0.0:7443/rutis`（地址固定的设备）；`wss://` 要给 `certificate`（PEM 证书链与私钥）。一次一个控制方，新连接接管旧的 |
| `id` | 这条连接自报的 id（小写字母、数字、`-`）：它的插件在 rutis 上注册为 `<id>:<插件>`（`kim-pixel:contacts`）；每台设备一个 |
| `token` | rutis 一侧的共享 token（`RUTIS_TOKEN_<行 id>`） |
| `trust` | 系统之外额外信任的 CA（PEM），用于自签证书的接入点 |
| `peer` | rutis 的 `id`，要核对时给 |
| `shutdownTimeout` | 插件卸载时清理的期限，默认 10 秒 |
| `log` | 状态行的去处，默认 `System.err`（logcat 的 `System.err`） |

- `start()` 立即返回；连上（rutis 问候）后应用按 `plugins` 的顺序装上插件，与 rutis 上的插件规则相同：一个插件 `inject` 的服务都在时才装上（同一个应用里别的插件提供的，或者 rutis 提供的），其中一个撤走时卸载，回来后再装。rutis 上可共享的服务都会到应用这边；应用的插件提供的服务都报给 rutis（带插件名），rutis 给的不报回去。一个插件装载失败只影响它自己：`System.err` 收到 `rutis: plugin <名>: <错误>` 一行，后面的照装；这一行只在设备上，rutis 一侧看到的是这个插件没有出现。断线自动重连（0.5 秒起翻倍到 30 秒；token 或 id 被拒每 30 秒；协议不兼容则停下）；每次连接都是新的，重连后重新装上插件。
- `stop()` 撤回服务、逆序清理全部插件，然后断开；之后可以再 `start()`。
- `rutis.state` 是 `StateFlow<RutisState>`：`Stopped(reason)`、`Connecting(retrying)`（`retrying` 是上一次失败的原因）、`Connected(address)`。拿来显示即可，SDK 不据此做任何决定。

rutis 一侧（[rutis-host](rutis-host.md)）：一行 `rutis-host/listen` 等应用连进来，设备和它们的插件都不写在 rutis.json 里：

```json
{
  "id": "main",
  "listen": [{ "name": "public", "address": "0.0.0.0:7443", "cert": "server.pem", "key": "server.key" }],
  "rows": [
    { "id": "phone", "name": "rutis-host/listen", "config": { "listener": "public" } }
  ]
}
```

`RUTIS_TOKEN_PHONE=<token> rutis-host run`。每条通过的连接是一个会话，应用装着的插件注册为 `<应用的 id>:<插件>`（`kim-pixel:contacts`），它们提供的服务全局可见，别的行按名字 `inject`；同一 id 的新连接顶替旧的。`run` 的输出以行 id 开头：`phone: connected kim-pixel`、`phone: disconnected kim-pixel (connection lost)`。某台设备的应用停下（`stop`、断网、进程被杀）时只卸载它的插件。

两台设备提供同名服务时按内核规则：后来的那台的插件被拒绝，它的 `System.err` 写着 rutis 给的原因（`rutis: cannot announce contacts: …`）。应用里的插件装不上（被拒绝，或自己的 `apply` 失败）时，0.19 只在设备的日志里写出，宿主上不打印。0.18 的"`instanced` 分组里一个 `py:digest`，每台设备一份"在 0.19 不再支持，按设备区分是设备上插件自己的事：

- 把它放到设备上：digest 写成应用里的一个插件，`inject` 同一个应用的 `contacts`（`contacts` 装上后它才装上）。
- 由宿主上的插件自己按设备区分：设备上的插件以带设备名的名字提供服务（示例应用的插件都由配置给出服务名：`Samples.configs(suffix = "tablet")` 给出 `contacts-tablet`），宿主上的一个插件监听框架事件 `rutis/service`（`{ name, scope, available, row }`，[插件 API：框架事件](plugin-api.md#框架事件-rutis)）：`contacts-<设备>` 出现（`available: true`）时在事件处理里调用服务 `loader`：`loader.create({id: "digest-<设备>", name: "py:digest-one", config: {device}, inject: ["contacts-<设备>"]})`，撤走（`available: false`）时 `loader.remove("digest-<设备>")`；不在 `apply` 里调用。完整写法见 [Swift 应用 §4](swift-app.md#每台设备各算一份)。

开发时可以不用证书：`adb reverse tcp:7443 tcp:7443`，应用连 `ws://127.0.0.1:7443/rutis`。Android 默认禁止明文流量，在网络安全配置里只对回环放行（示例应用的 `res/xml/network_security_config.xml`）。

## 4. 权限

授权提示属于应用：应用在合适的时候向用户要权限（示例应用在按“开始”时 `requestPermissions`），插件只检查。rutis 从不弹提示。

| 情况 | 插件怎么做 | rutis 一侧看到 |
| --- | --- | --- |
| 装载时就需要（`contacts`） | `apply` 里检查，没有就抛错，说明哪个权限 | 应用装这个插件失败（`rutis: plugin contacts: …`，错误里有权限名），rutis 上没有 `<id>:contacts`；其余插件照常 |
| 调用时才需要（`notify` 发通知） | 方法里检查，没有就抛错，带 `code` 与 `data` | 调用方收到错误：`code` 为 `permission-denied`，`data.permission` 为权限名 |
| 用户在设置里撤销运行时权限 | — | 系统杀掉应用进程 → 连接断开 → 它的插件卸载；应用重启、重连后重新装上 |

示例应用的错误类型（普通 Kotlin 属性，跨进程时带上）：

```kotlin
class PermissionDenied(val permission: String) :
    Exception("$permission is not granted to this application: grant it in the application (its settings, or when it asks)") {
    val code: String = "permission-denied"
    val data: Map<String, String> = mapOf("permission" to permission)
}
```

用户后来授予了权限：重新连接（`stop()` 再 `start()`），应用就会重新装上之前失败的插件。示例应用在 `onRequestPermissionsResult` 里这样做。

## 5. 何时 start / stop（业务层）

SDK 只连接；前后台、保活、唤醒是应用自己的事。常见做法：

| 需要 | 做法 |
| --- | --- |
| 只在用户打开应用时提供能力 | 跟随界面：`onStart` 里 `start()`，`onStop` 里 `stop()` |
| 一直在线 | 前台服务里持有 `RutisApp`：服务启动时 `start()`，停止时 `stop()`；常驻通知让用户知道 |
| 偶尔需要 | 收到推送（FCM）时短暂 `start()`，做完 `stop()` |

前台服务（Android 14 起必须声明类型）：

```xml
<uses-permission android:name="android.permission.FOREGROUND_SERVICE" />
<uses-permission android:name="android.permission.FOREGROUND_SERVICE_DATA_SYNC" />
<service android:name=".RutisService" android:foregroundServiceType="dataSync" android:exported="false" />
```

- 类型按应用真正做的事选。`dataSync` 在 Android 15 起每天累计约 6 小时；需要更长时间常驻、又没有更贴切的类型时，用 `specialUse` 并写明用途（`<property android:name="android.app.PROPERTY_SPECIAL_USE_FGS_SUBTYPE" android:value="…"/>`），上架 Google Play 要在后台审核里说明。插件用到定位、相机、麦克风等时，类型要包含对应的 `location`、`camera`、`microphone`，并持有相应权限。
- 服务要在 `onStartCommand` 里尽快 `startForeground(id, notification, type)`。

电池优化：

- Doze 和应用待机会推迟应用的网络访问，连接会断，按退避重连。要常驻时，请用户把应用加入电池优化白名单：打开 `Settings.ACTION_IGNORE_BATTERY_OPTIMIZATION_SETTINGS` 让用户自己选；直接请求 `ACTION_REQUEST_IGNORE_BATTERY_OPTIMIZATIONS` 需要 `REQUEST_IGNORE_BATTERY_OPTIMIZATIONS` 权限，Google Play 只允许少数类别的应用这样做。
- 一些厂商系统另有后台限制（自启动、后台运行开关），需要引导用户在系统设置里放行。
- 不管哪种情况，rutis 只看到服务“在”或“不在”：断开时应用的插件卸载，使用者停下，重连后恢复。

## 6. 线程与生命周期细节

- `start()`、`stop()` 可以在主线程调用，立即返回。
- rutis 调进应用的每个调用在自己的线程上运行；`suspend` 方法在这个线程上执行，调用链随协程传递，插件代码不用传上下文。
- 插件要持久化的数据用平台自己的存储（`SharedPreferences`、Room……），SDK 不提供存储。

## 7. 测试

- 插件的单元测试在 JVM 上用测试工具 `arcships.rutis.testing.RutisTest`（构件 `io.github.arcships:rutis-testing`，见 kotlin/README）：插件在测试进程里的真实运行时中运行，是 `rutis-host test`（`PATH` 上或 `RUTIS_HOST`）的一行，每个测试进程起一个宿主。装载出来的 `loaded.on(name) { args -> ... }` 在根作用域监听：插件发出的、测试自己发出的都送到，返回撤销函数。
- 设备上没有 `rutis-host` 进程：测试像应用一样用 `RutisApp` 装自己的插件，拨到开发机上的 rutis（一个 `rutis-host/listen` 行，经 `adb reverse`），插件之间经它用对方的服务和事件。仓库里的设备测试（模拟器，在本地跑，见 [CI](../ci.md) §12）：`./gradlew :rutis:connectedDebugAndroidTest -Pandroid.testInstrumentationRunnerArguments.address=<ws 地址> -Pandroid.testInstrumentationRunnerArguments.token=<token>`（SDK 的主线程规则；没有 `address` 时跳过），`cargo test -p rutis-host --features android --test android`（示例应用的 release 构建经 R8 拨出到 rutis-host、启停；两条连接各自装上插件；权限提示指向应用、拒绝后 `contacts` 装载失败、`notify` 的调用方收到 `permission-denied`，授权后恢复）。`tools/android-emulator.sh start` 像 CI 那样起一个模拟器。

## 8. 真机验收清单

模拟器上已自动验证上面的流程；以下要在真机上手动走一遍（发布前、或换了大的 Android 版本时）：

1. 一台电脑运行 rutis-host，配置如 §3，用真实证书（或自签证书 + 应用的 `trust`）监听 `wss://`；两台手机装示例应用，连同一个 Wi-Fi 或移动网络。
2. 两台手机各填自己的 id（如 `pixel`、`galaxy`）与 token，按“开始”：两台各弹出本应用（“rutis sample”）的通讯录、通知授权提示。
3. 一台允许、一台拒绝：允许的那台装上 `contacts`（rutis 上出现 `<id>:contacts`），使用它的行（如 `py:digest`）拿到本机通讯录；拒绝的那台，应用的 logcat 里是 `rutis: plugin contacts: …`、错误里写着 `android.permission.READ_CONTACTS`，rutis 上没有它的 `contacts`，调用 `notify.post` 的行收到 `permission-denied`。
4. 在拒绝的那台的系统设置里授予权限，回到应用按“停止”再“开始”（或重启应用）：它的 `contacts` 行恢复。
5. 一台按“停止”：只有它的插件卸载，另一台不受影响；再“开始”恢复。
6. 关掉一台的网络 1 分钟再打开：它的插件卸载，联网后自动重连、重新装上插件。
7. 在设置里撤销已授予的权限：应用进程被系统杀掉，它的插件卸载；重新打开应用并开始后按 §4 的规则重新装上。
8. 用 release 构建（R8）重复第 2、3 步，确认行为与 debug 构建相同。

## 9. 已知限制

- 不能在设备上装载新代码：换插件就发新版本应用。
- 身份是过渡方案：共享 token 加自报 id，rutis 不核实 id。
