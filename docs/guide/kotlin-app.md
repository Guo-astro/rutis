# Android 应用托管插件（Kotlin）

[English](kotlin-app.en.md)

Android 应用把自己和所在设备的能力（通讯录、通知、设备信息……）作为 rutis 插件提供：插件用 Kotlin 写、编进应用，应用连上 rutis 后，装哪些插件、用什么配置、何时卸载都由 rutis 的配置决定。设计见 [Android 应用托管](../design-android-runtime-2026-10-12.md) 与 [应用接入](../design-app-integration-2026-10-14.md)。

`kotlin/rutis/android-sample` 是一个完整的示例应用（`contacts`、`notify`、`device` 三个插件，一个开始 / 停止的界面），下文的代码都取自它。

## 1. 依赖

Android 10（API 29）或更高，JDK 17。应用的 `build.gradle.kts`：

```kotlin
plugins {
    id("com.android.application")
    kotlin("android")
    kotlin("plugin.serialization")
    id("com.google.devtools.ksp")
}

dependencies {
    implementation("io.github.arcships:rutis:0.17.1")
    ksp("io.github.arcships:rutis-ksp:0.17.1")
}
```

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
- `provides` 不用写：提供的服务自动让 rutis 知道，rutis 一侧同一实例里的行照样能用。
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

  每一份在应用的进程里运行；应用连进来时，它只能挂在这个连接所在的实例里（或更里面）。它属于交出它的那一行的这一代：这一行卸载、应用 `stop()` 或断开时，副本先结束。交给 `RutisTest` 的测试替身时，替身收到的就是这个 `Plugin` 本身，可以直接 `RutisTest.load(it, config)`；替身交回时插件拿到的也是原来那个。参数或返回值要直接是 `Plugin`（或 `Plugin?`），`List<Plugin>` 这类放在集合或数据类里的插件定义不支持，要交几个就调用几次。规则见 [插件 API](plugin-api.md#插件定义)。

## 3. 接入 rutis

```kotlin
val rutis = RutisApp(
    plugins = listOf(contacts(context), notify(context), device(context)),
    connect = "wss://rutis.example.com:7443/rutis",
    id = "kim-pixel",
    token = token,
)
rutis.start()
```

| 参数 | 含义 |
| --- | --- |
| `connect` | rutis 接入点的地址（推荐：应用在 NAT 后、地址会变）。非回环只能 `wss://` |
| `listen` | 改为等 rutis 拨入，如 `wss://0.0.0.0:7443/rutis`（地址固定的设备）；`wss://` 要给 `certificate`（PEM 证书链与私钥）。一次一个控制方，新连接接管旧的 |
| `id` | 这条连接自报的 id（小写字母、数字、`-`），即它在 rutis 里的实例名；每台设备一个 |
| `token` | rutis 一侧的共享 token（`RUTIS_TOKEN_<运行时名>`） |
| `trust` | 系统之外额外信任的 CA（PEM），用于自签证书的接入点 |
| `peer` | rutis 的 `id`，要核对时给 |
| `shutdownTimeout` | 插件卸载时清理的期限，默认 10 秒 |
| `log` | 状态行的去处，默认 `System.err`（logcat 的 `System.err`） |

- `start()` 立即返回，连上后由 rutis 装载插件；断线自动重连（0.5 秒起翻倍到 30 秒；token 或 id 被拒每 30 秒；协议不兼容则停下）。
- `stop()` 逆序清理全部插件，然后断开；之后可以再 `start()`。
- `rutis.state` 是 `StateFlow<RutisState>`：`Stopped(reason)`、`Connecting(retrying)`（`retrying` 是上一次失败的原因）、`Connected(address)`。拿来显示即可，SDK 不据此做任何决定。

rutis 一侧（[rutis-host](rutis-host.md#连进来的运行时)）：每条连接是 `instanced` 分组的一个实例，分组里写这台设备要装的插件，以及使用它们的其他行：

```json
{
  "id": "main",
  "listen": [{ "name": "public", "address": "0.0.0.0:7443", "cert": "server.pem", "key": "server.key" }],
  "runtimes": { "remote": [{ "name": "phone", "language": "kotlin", "listen": "public", "instances": "phone" }] },
  "rows": [
    { "id": "phone", "group": true, "instanced": true, "config": [
      { "id": "contacts", "name": "phone:contacts" },
      { "id": "device", "name": "phone:device" },
      { "id": "digest", "name": "py:digest", "inject": ["contacts"] }
    ] }
  ]
}
```

`RUTIS_TOKEN_PHONE=<token> rutis-host run`。两台设备带同名插件时各在自己的实例里，互不冲突；某台设备的应用停下（`stop`、断网、进程被杀）时只关闭它的实例。

开发时可以不用证书：`adb reverse tcp:7443 tcp:7443`，应用连 `ws://127.0.0.1:7443/rutis`。Android 默认禁止明文流量，在网络安全配置里只对回环放行（示例应用的 `res/xml/network_security_config.xml`）。

## 4. 权限

授权提示属于应用：应用在合适的时候向用户要权限（示例应用在按“开始”时 `requestPermissions`），插件只检查。rutis 从不弹提示。

| 情况 | 插件怎么做 | rutis 一侧看到 |
| --- | --- | --- |
| 装载时就需要（`contacts`） | `apply` 里检查，没有就抛错，说明哪个权限 | 这一行装载失败，错误里有权限名 |
| 调用时才需要（`notify` 发通知） | 方法里检查，没有就抛错，带 `code` 与 `data` | 调用方收到错误：`code` 为 `permission-denied`，`data.permission` 为权限名 |
| 用户在设置里撤销运行时权限 | — | 系统杀掉应用进程 → 连接断开 → 实例关闭；应用重启、重连后按配置重新装载 |

示例应用的错误类型（普通 Kotlin 属性，跨进程时带上）：

```kotlin
class PermissionDenied(val permission: String) :
    Exception("$permission is not granted to this application: grant it in the application (its settings, or when it asks)") {
    val code: String = "permission-denied"
    val data: Map<String, String> = mapOf("permission" to permission)
}
```

用户后来授予了权限：重新连接（`stop()` 再 `start()`），rutis 就会重新装载之前失败的插件。示例应用在 `onRequestPermissionsResult` 里这样做。

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
- 不管哪种情况，rutis 只看到服务“在”或“不在”：断开时实例关闭，使用者停下，重连后恢复。

## 6. 线程与生命周期细节

- `start()`、`stop()` 可以在主线程调用，立即返回。
- rutis 调进应用的每个调用在自己的线程上运行；`suspend` 方法在这个线程上执行，调用链随协程传递，插件代码不用传上下文。
- 插件要持久化的数据用平台自己的存储（`SharedPreferences`、Room……），SDK 不提供存储。

## 7. 测试

- 插件的单元测试用测试工具 `arcships.rutis.testing.RutisTest`（见 kotlin/README），在 JVM 或设备上都能跑。
- 仓库里的设备测试（模拟器，在本地跑，见 [CI](../ci.md) §12）：`./gradlew :rutis:connectedDebugAndroidTest`（SDK 的主线程规则），`cargo test -p rutis-host --features android --test android`（示例应用的 release 构建经 R8 拨出到 rutis-host、启停；两条连接各成实例；权限提示指向应用、拒绝后 `contacts` 装载失败、`notify` 的调用方收到 `permission-denied`，授权后恢复）。`tools/android-emulator.sh start` 像 CI 那样起一个模拟器。

## 8. 真机验收清单

模拟器上已自动验证上面的流程；以下要在真机上手动走一遍（发布前、或换了大的 Android 版本时）：

1. 一台电脑运行 rutis-host，配置如 §3，用真实证书（或自签证书 + 应用的 `trust`）监听 `wss://`；两台手机装示例应用，连同一个 Wi-Fi 或移动网络。
2. 两台手机各填自己的 id（如 `pixel`、`galaxy`）与 token，按“开始”：两台各弹出本应用（“rutis sample”）的通讯录、通知授权提示。
3. 一台允许、一台拒绝：允许的那台，`contacts` 行在它的实例里运行，使用它的行（如 `py:digest`）拿到本机通讯录；拒绝的那台，rutis-host 报这一行失败、错误里写着 `android.permission.READ_CONTACTS`，调用 `notify.post` 的行收到 `permission-denied`。
4. 在拒绝的那台的系统设置里授予权限，回到应用按“停止”再“开始”（或重启应用）：它的 `contacts` 行恢复。
5. 一台按“停止”：只有它的实例关闭，另一台不受影响；再“开始”恢复。
6. 关掉一台的网络 1 分钟再打开：实例关闭，联网后自动重连、行重新装载。
7. 在设置里撤销已授予的权限：应用进程被系统杀掉，实例关闭；重新打开应用并开始后按 §4 的规则重新装载。
8. 用 release 构建（R8）重复第 2、3 步，确认行为与 debug 构建相同。

## 9. 已知限制

- 不能在设备上装载新代码：换插件就发新版本应用。
- 身份是过渡方案：共享 token 加自报 id，rutis 不核实 id。
