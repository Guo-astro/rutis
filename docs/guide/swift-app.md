# 在应用里托管插件（Swift）

[English](swift-app.en.md)

iOS、iPadOS 或 macOS 应用可以把它自己和所在设备的能力（健康数据、定位、通讯录、应用本身的功能）作为普通的 rutis 插件提供给 rutis。插件编进应用、随应用发布；应用连上 rutis 后自己按顺序装上它们，各带应用给的配置，它们提供的服务在 rutis 上以 `<id>:<插件>` 出现；应用只负责开始和停止连接。需要 iOS / iPadOS 17 或 macOS 14 以上，Xcode 26 以上（带 Swift 6.2 工具链）。

插件与 Mac 上的 [Swift 插件](swift-plugin.md) 完全相同：同一个插件定义既可以 `serve` 成二进制，也可以交给应用托管。本文只写不同的部分：把 SDK 加进应用、配置、开始与停止、rutis 一侧、权限、网络。

## 1. 加入 SDK

在 Xcode 里：File → Add Package Dependencies…，填 `https://github.com/arcships/rutis`，从 `0.18.0` 起，把库 `Rutis` 加到应用的 target。在包里：

```swift
.package(url: "https://github.com/arcships/rutis", from: "0.18.0"),
// target 里：
.product(name: "Rutis", package: "rutis"),
```

第一次构建会编译 SDK 的宏（swift-syntax，只在编译期），Xcode 会要求信任一次；CI 里给 `xcodebuild` 加 `-skipMacroValidation`。SDK 静态链入应用，运行时不装载任何东西。

## 2. 写插件

与 [Swift 插件指南](swift-plugin.md#2-提供服务) 相同：插件声明它用什么，在 `apply` 里提供服务（服务自动对外，不需要写 `provides`）、登记清理。用到设备能力的插件直接调用系统 API：

```swift
import HealthKit
import Rutis

/// 用户没有授权时抛出：调用方（任何语言）收到它。
struct HealthAccessDenied: Error, LocalizedError {
    let code: String   // "not-determined" 或 "denied"
    var errorDescription: String? { "step counts are not shared with this app (\(code))" }
}

@RutisService
public final class Health: @unchecked Sendable {
    let store = HKHealthStore()
    let steps = HKQuantityType(.stepCount)

    /// 请求授权（提示里是应用的名字）；返回是否得到授权。
    public func authorize() async throws -> Bool {
        try await store.requestAuthorization(toShare: [steps], read: [steps])
        return store.authorizationStatus(for: steps) == .sharingAuthorized
    }

    /// 今天的步数。
    public func stepsToday() async throws -> Double {
        switch store.authorizationStatus(for: steps) {
        case .notDetermined: throw HealthAccessDenied(code: "not-determined")
        case .sharingDenied: throw HealthAccessDenied(code: "denied")
        default: break
        }
        // … 用 HKStatisticsQuery 查今天
    }
}

public let health = Rutis.define(Plugin(name: "health") { ctx in
    ctx.provide("health", Health())
})
```

rutis 不建模设备：行需要知道自己在哪台设备上时，由插件像提供任何服务一样提供。插件的配置类型照常写（[配置](swift-plugin.md)；要在应用里给出它，再加 `Encodable`）：

```swift
@RutisService
public final class Device: @unchecked Sendable {
    let installation: String
    init(installation: String) { self.installation = installation }
    public func id() -> String { installation }
    public func system() async -> String { await "\(UIDevice.current.systemName) \(UIDevice.current.systemVersion)" }
}

@RutisConfig
public struct DeviceConfig: Codable {
    public var installation: String
}

public let device = Rutis.define(Plugin(name: "device", config: DeviceConfig.self) { ctx, config in
    ctx.provide("device", Device(installation: config.installation))
})
```

插件的测试照插件指南 §3 用 `RutisTest`，在 iOS 模拟器里同样能跑。

插件交给服务的插件定义（[插件 API：各语言怎样交出插件定义](plugin-api.md#各语言怎样交出插件定义)）在应用的进程里运行；应用 `stop()` 或断开时，从它挂出的每一份结束。

### 带方法的对象

和 [插件 API：各语言怎样交出和收对象](plugin-api.md#各语言怎样交出和收对象) 相同：`@RutisService` 类型的实例按引用交出；别的进程交来的对象，参数类型写文件顶层的 `@RutisClient` 协议（`any Cache`，不是可选类型）拿到它的代理，写 `Any` 拿到 `Rutis.Advanced.Client`（`try await client.call("get", values: ["a"], as: Int?.self)`）。

```swift
@RutisClient
protocol Cache {
    func get(_ key: String) async throws -> Int?
    func set(_ key: String, _ value: Int) async throws
}

@RutisService
public final class Warmer: @unchecked Sendable {
    public func warm(_ cache: any Cache) async throws -> Int? {
        try await cache.set("a", 1)          // 调用的是对方的原对象
        return try await cache.get("a")
    }
}
```

- 方法都是 async 的跨进程调用，没有数据字段；对象没有的方法以 `no-method` 失败。
- 嵌在别的类型里的 `@RutisClient` 协议、数据类型的参数收不了对象：这次调用以 `bad-argument` 失败（`argument 1 should be a Spot, got an object (an object is received only as a @RutisClient protocol or Any)`）。`[any Cache]` 在 0.18 不支持，数组、字典里的对象经 `Any` 拿到 `Rutis.Advanced.Client`。
- 收到的代理再交出去，发出的是原来那个对象；交回原来的进程，那边拿到的是原对象。
- 应用交出的对象在应用的进程里：应用 `stop()` 或断开后，别处对它的调用以 `peer-gone` 失败。
- 对象不能交给 0.18 之前的 rutis：这次调用以 `bad-value` 失败，错误提示升级。

## 3. 开始与停止

```swift
import Rutis

let rutis = RutisApp(
    plugins: [health, device],
    configs: ["device": DeviceConfig(installation: "kim-iphone")],
    connect: "wss://rutis.example.com:7443/rutis",
    id: "kim-iphone",                 // 这个应用的 id：小写字母、数字和 -
    token: secrets.rutisToken)

try await rutis.start()   // 拨出；连上后按顺序装上 health、device
await rutis.stop()        // 撤回服务，全部插件逆序清理，然后断开
```

| 参数 | 含义 |
| --- | --- |
| `plugins` | 连上后按顺序装上的插件 |
| `configs` | 可选：插件的配置，按插件名给出，值是它的配置类型（`Encodable`）；没给的插件用它的配置 Schema 的默认值。名字不是 `plugins` 里的插件时 `start()` 抛出：`configs names nope, which is not one of the plugins: device, health`。配置只在 `RutisApp` 里给：插件二进制的插件由 rutis 的行给配置 |
| `connect` | rutis 的接入点：`wss://…`；`ws://` 只限回环（模拟器连它所在的 Mac） |
| `id` | 这条连接的 id：它的插件在 rutis 上注册为 `<id>:<插件>`（`kim-iphone:health`）；每个安装用自己的 id（例如首次启动时生成并保存） |
| `token` | 接入点的共享 token。放在钥匙串里，不要写在应用代码里 |
| `peer` | 可选：接受的 rutis 的 `id`；对端是别的 id 时拒绝 |
| `trust` | 可选：在系统根证书之外额外信任的 CA 证书（`SecCertificate`），用于自己的 CA 签发的证书 |
| `shutdownTimeout` | 连接结束后插件清理最多可用的秒数（10） |
| `log` | 连接与插件的状态行写到哪里（默认 stderr；例如 `{ logger.info("\($0)") }`） |

- `start()` 开始拨出后即返回；重试修不好的（不合法的 id 或地址）直接抛出。此后它自己保持连接：连不上或断开时重试（从 0.5 秒起翻倍，最长 30 秒）；token 被拒每 30 秒重试。已经开始时再调用 `start()` 什么也不做。
- 连上（rutis 问候）之后，应用按 `plugins` 的顺序装上插件，与 rutis 上的插件规则相同：一个插件 `inject` 的服务都在时才装上（同一个应用里别的插件提供的，或者 rutis 提供的），其中一个撤走时卸载，回来后再装。rutis 上可共享的服务都会到应用这边（rutis 撤下时这边也撤下）；应用的插件提供的服务都报给 rutis（带插件名），rutis 上别的行按名字使用它们。应用不把 rutis 给它的服务再报回去。
- 一个插件装载失败（配置不合 Schema、`apply` 抛出）只影响它自己：`log` 收到 `rutis: plugin device: BindingError: config: installation is required` 这样的一行，后面的插件照装。这一行只在设备上：rutis 一侧看到的是这个插件没有出现（没有 `<id>:device`）。
- 每次连接都是新的：断开时插件逆序清理，重连后重新装上。
- `stop()` 在 rutis 还能收到的时候撤回服务、逆序卸载全部插件，然后关闭连接，不再重连；完成后返回，之后可以再 `start()`。
- `start()` 与 `stop()` 按调用的先后生效，各在自己的 `Task` 里调用也一样：停止进行中调用的 `start()` 等它完成后再开始（用户在清理期间回到前台，应用照样连上）；开始进行中调用的 `stop()` 撤销它（不会在后台继续拨出）。
- `state` 是 `.stopped(reason:)`、`.connecting` 或 `.connected`。`RutisApp` 遵循 `Observable`，显示 `rutis.state` 的 SwiftUI 视图会自己更新；别处用 `for await state in rutis.states { … }` 跟随。`.stopped(reason: "…")` 表示它自己停下了：rutis 不兼容（例如早于 0.13），重试修不好。
- `log` 收到的行与所有 Swift 运行时相同：`rutis: connected to …`、`rutis: connection lost (…); retrying in 1s`、`rutis: refused: auth-rejected (…); retrying in 30s`、`rutis: refused: incompatible (…); stopping`，以及插件的失败。

何时开始、何时停止由应用决定；SDK 不据应用的状态、也不据自己的状态做任何事。iOS 上应用进入后台后很快被挂起，在 rutis 看来被挂起的应用就是断开了：它的插件在 rutis 上卸载，用它们服务的行停下。常见做法是在前台时连着：

```swift
@main
struct HealthApp: App {
    @Environment(\.scenePhase) private var phase
    @State private var rutis = RutisApp(plugins: [health, device], configs: ["device": DeviceConfig(installation: Settings.id)],
                                        connect: Settings.address, id: Settings.id, token: Settings.token)

    var body: some Scene {
        WindowGroup {
            Text("rutis: \(rutis.state)")
        }
        .onChange(of: phase) { _, phase in
            switch phase {
            case .active:
                Task { try? await rutis.start() }
            case .background:
                // 在 iOS 挂起应用之前给清理留出时间。
                let task = UIApplication.shared.beginBackgroundTask()
                Task {
                    await rutis.stop()
                    UIApplication.shared.endBackgroundTask(task)
                }
            default:
                break
            }
        }
    }
}
```

需要连得更久的应用用 iOS 允许它的方式（与它所做的事相符的后台模式）；Mac 应用（菜单栏应用、登录项）可以一直连着。

## 4. rutis 一侧

rutis 用一行 `rutis-host/listen` 等应用连进来；设备不必在 rutis.json 里列出，应用装着的插件也不写在这里：

```json
{
  "id": "main",
  "listen": [{ "name": "public", "address": "0.0.0.0:7443", "cert": "server.pem", "key": "server.key" }],
  "rows": [
    { "id": "phone", "name": "rutis-host/listen", "config": { "listener": "public" } }
  ]
}
```

```bash
RUTIS_TOKEN_PHONE=<应用出示的 token> rutis-host run rutis.json
```

- 每条通过的连接是一个会话，应用装着的插件注册为 `<应用的 id>:<插件>`（`kim-iphone:health`、`kim-iphone:device`），它们提供的服务在 rutis 上全局可见，别的行按名字 `inject`。同一 id 的新连接顶替旧的。
- `run` 的输出以行 id 开头：`phone: connected kim-iphone`、`phone: disconnected kim-iphone (connection lost)`、`phone: disconnected kim-iphone (replaced by a new connection)`。`rutis-host status` 的 `connections:` 一节列出每个应用注册的插件：`phone: kim-iphone (kim-iphone:health, kim-iphone:device)`。
- 应用停止或被挂起时它的插件卸载，用它们服务的行停下；再次开始时应用重新装上插件，这些行恢复。
- 同名服务按内核规则：两台设备都提供 `health` 时，后来的那台的 `health` 插件被拒绝并说明原因，先来的照常可用。

### 每台设备各算一份

0.18 的写法（`instanced` 分组里一个 `py:coach`，每台设备一份）在 0.19 不再支持：rutis 不为每条连接建实例。按设备区分是设备上插件自己的事，两种改法：

- **把它放到设备上。** coach 写成应用里的一个插件，它使用同一个应用的 `health`（`health` 装上后它才装上）：

  ```swift
  let coach = Rutis.define(Plugin(name: "coach", inject: ["health"]) { ctx in
      let health = try ctx.use("health", HealthProxy.self)
      ctx.provide("coach", Coach(health: health))
  })
  let rutis = RutisApp(plugins: [health, coach], connect: …, id: id, token: token)
  ```

  每台设备上各有一个 coach，各自看到的是自己的 `health`；它们的服务名也各不相同时（下一条），rutis 上的行可以分别使用。

- **由宿主上的插件自己按设备区分。** 设备上的插件以带设备 id 的名字提供服务：

  ```swift
  public let health = Rutis.define(Plugin(name: "health", config: DeviceConfig.self) { ctx, config in
      ctx.provide("health-\(config.installation)", Health())   // health-kim-iphone
  })
  ```

  宿主上的插件 `coach` 监听框架事件 `rutis/service`（参数 `{ name, scope, available, row }`，见 [插件 API：框架事件](plugin-api.md#框架事件-rutis)）：一个 `health-<设备>` 出现（`available: true`）时，经服务 `loader` 为这台设备加一行 `coach-<设备>`，运行只管一台设备的插件 `py:coach-one`；它撤走（`available: false`）时删掉这一行。`loader` 的方法在事件处理里调用，不在 `apply` 里（在一行自己的 `apply` 里调用会等到这个 `apply` 结束，卡住）：

  ```python
  from rutis import define_plugin

  def apply(ctx, config):
      loader = ctx.use("loader")

      async def seen(event):
          name = event["name"]
          if not name.startswith("health-"):
              return
          device = name.removeprefix("health-")          # kim-iphone
          if event["available"]:
              await loader.create({"id": f"coach-{device}", "name": "py:coach-one",
                                   "config": {"device": device}, "inject": [name]})
          else:
              await loader.remove(f"coach-{device}")

      ctx.on("rutis/service", seen)

  coach = define_plugin(apply, inject=["loader"])
  ```

  `py:coach-one` 用 `ctx.use(f"health-{config['device']}")` 拿到它那台设备的服务。两台设备的服务名不同，不会相互拒绝；每台设备一行 `coach-<设备>`，`rutis-host status` 里看得到。

### 改为监听

rutis 能访问到的设备（实验室里地址固定的 Mac 或 iPad）也可以等 rutis 连过来：

```swift
let rutis = RutisApp(plugins: [health, device], configs: ["device": DeviceConfig(installation: "lab-ipad")], listen: "wss://0.0.0.0:7443/rutis",
                     id: "lab-ipad", token: token, certificate: identity)   // 一个 SecIdentity
```

`certificate` 是 TLS 身份（用 `SecPKCS12Import` 从 PKCS#12 文件得到，或取自钥匙串）；`ws://` 只限回环。端口写 `0` 表示取一个空闲端口：`log` 的 `rutis: listening on …` 一行给出是哪个，之后的每次开始都再绑定这个端口，地址保持不变。一次一个连接，新连接接管；没有 rutis 连着时 `state` 是 `.connecting`，连接上的 rutis 问候之后才是 `.connected`（10 秒内没有问候就关闭这个连接）。连接须在 10 秒内发来升级请求，同时升级的连接至多 32 个。监听器之后失败时（网络断了）按退避重新绑定（`rutis: listening failed (…); listening again in 1s`），已连着的 rutis 不受影响。rutis 一侧用一行 `rutis-host/dial` 连接它，应用连上后同样自己装上插件，它们注册为 `lab-ipad:health` 等：

```json
{
  "rows": [
    { "id": "ipad", "name": "rutis-host/dial", "config": { "address": "wss://10.0.0.20:7443/rutis" } }
  ]
}
```

token 写在 `RUTIS_TOKEN_IPAD`（行 id 大写）；自己的 CA 签发的证书用 `RUTIS_CA` 指向 CA 证书。

一般场景用拨出：设备地址会变、在 NAT 后面，而且只有 rutis 需要证书。

## 5. 权限

系统向用户询问的权限（健康数据、定位、通讯录、相册）记在应用上：提示里是应用的名字，用户的回答对应用生效。应用里的插件共用应用的权限。

- 在应用里加上能力（HealthKit：加 HealthKit capability，它会加上 `com.apple.developer.healthkit` entitlement），并在应用的 `Info.plist` 写用途说明（`NSHealthShareUsageDescription`、`NSHealthUpdateUsageDescription`、`NSLocationWhenInUseUsageDescription`、`NSContactsUsageDescription` …）。没有用途说明时，应用在请求授权时崩溃。
- 拒绝是普通的失败：检查结果并抛出。rutis 一侧任何语言的调用方都收到这个错误，带名字、消息与 `code`（上面的例子抛出 `HealthAccessDenied`，`code` 为 `denied`）。
- 提示显示在应用之上，请求授权的调用等用户回答：在应用位于前台时请求（rutis 只在应用连着时调用插件，按上面的做法就是应用在前台时），或者从应用自己的界面发起。
- HealthKit 不告诉应用读取是否被拒绝（读取只是没有数据）；分享状态（`authorizationStatus(for:)`）会告诉。没有"健康"的设备（部分 iPad）上 `HKHealthStore.isHealthDataAvailable()` 为 false。

## 6. 网络

- **本地网络权限（iOS 14 起）。** 应用第一次连接本地网络上的地址时（`wss://192.168.1.10:7443` 上的 rutis、`.local` 名字），iOS 会询问用户是否允许该应用查找并连接本地网络上的设备。在 `Info.plist` 里写 `NSLocalNetworkUsageDescription`，说明为什么需要；不写的话提示里没有有用的说明。用户允许之前连接会失败，应用持续重试（`rutis: cannot connect (…); retrying in …`，`state` 为 `.connecting`）；允许之后下一次重试就会连上。用户拒绝后只能在"设置"（隐私与安全性 → 本地网络）里改：在应用里告诉用户。在本地地址上监听同样会询问。互联网上的 rutis（公网名字或地址）、模拟器里的回环地址不需要这个权限。SDK 不使用 Bonjour，不需要 `NSBonjourServices`。
- **TLS。** 非回环地址必须是 `wss://`。公共 CA 签发的证书无需其他设置；自己的 CA 用 `trust` 传入。连接受 App Transport Security 约束：TLS 1.2 以上、前向保密（rutis 的监听器提供的就是这样）。
- **id。** rutis 不核实 id：它只用来区分连接、给远程插件命名（`<id>:<插件>`；凭据是 token）。两个安装用同一个 id 会轮流把对方顶掉。

## 7. Mac 应用

同一个 `RutisApp` 也在 Mac 应用里运行；沙盒里的 Mac 应用拨出需要 entitlement `com.apple.security.network.client`（监听需要 `network.server`）。Mac 应用在后台不会被挂起，菜单栏应用或登录项可以一直连着。不属于某个应用的插件更适合做成 [插件二进制](swift-plugin.md)：由 rutis 按需启动。

## 8. 示例应用

[`swift/examples/HealthApp`](../../swift/examples/HealthApp) 是一个 iOS 应用的 Xcode 项目，托管 §2 的 `health` 与 `device` 插件。服务名按 §4 "每台设备各算一份"的第二种改法带上这个安装的 id（连接的 `id`）：`health-<id>`、`device-<id>`（`-RutisID kim-iphone` 时是 `health-kim-iphone`、`device-kim-iphone`），经 `RutisApp` 的 `configs` 给出（`HealthConfig(service:)`、`DeviceConfig(installation:service:)`），两台设备连同一个 rutis 不会相互拒绝，业务层按 §3 在前台开始、进入后台停止。它依赖本仓库里的 SDK（本地包），连接到哪里由启动参数给出（`-RutisConnect`、`-RutisID`、`-RutisToken`，以及 `-RutisCA`：base64 的 DER 格式 CA 证书），在 Xcode 的 scheme 里设置。

在模拟器上，连同一台 Mac 上的 `rutis-host`（模拟器能访问 Mac 的回环地址）：

```bash
cargo test -p rutis-e2e --test ios_app -- --ignored
```

它用 `xcodebuild` 构建应用，在两台新建的模拟器上以各自的 id 运行，`rutis-host run` 带 §4 的 `rutis-host/listen` 行和一个使用应用服务的 Python 行，检查：每台模拟器的应用装上自己的插件（`<id>:health`、`<id>:device`），服务名各不相同（`health-<id>`、`device-<id>`）；应用进入后台时它的插件卸载、回到前台时重新装上，另一台不受影响；用户决定之前读步数失败，Python 调用方收到插件的 `HealthAccessDenied`（`code` 为 `not-determined`）；请求授权时"健康"的授权页显示在应用之上、写着应用的名字（从截图里读出）。需要 Xcode 与 iOS 模拟器运行时（`xcodebuild -downloadPlatform iOS`），只在本地运行，`bash tools/ios-selftest.sh`（SDK 自己的测试在模拟器里运行）也一样：CI 不运行需要模拟器、设备或授权提示的测试。模拟器上无法点击授权页，`simctl privacy` 也没有 HealthKit，这些由下面的人工清单检查。

### 人工验收清单（真机）

只有人和真机能检查的部分。iPhone（iOS 17 以上）与运行 rutis 的 Mac 在同一网络：

1. 在 Mac 上为它的局域网地址签一张证书（任何能生成 CA 和服务器证书、把 IP 写进 subject alternative names 的工具，例如 `mkcert 192.168.1.10`），按 §4 的配置运行 rutis，监听器 `0.0.0.0:7443` 用这张证书和私钥，设置 `RUTIS_TOKEN_PHONE`；一个 Python 行（例如 e2e 的 `coach`，在 `tests/e2e/tests/ios_app.rs` 里）显示它从应用看到什么。
2. 打开 `swift/examples/HealthApp/HealthApp.xcodeproj`，在 Signing & Capabilities 里选你的 team（HealthKit 保持开启），在 scheme 的参数里设 `-RutisConnect wss://<Mac 的 IP>:7443/rutis`、`-RutisToken <token>`，`-RutisCA` 设为 CA 证书（先 `openssl x509 -in rootCA.pem -outform der -out rootCA.der`，再 `base64 -i rootCA.der`）。在 iPhone 上运行。
3. **本地网络：** iOS 询问是否允许 Rutis Health 查找本地网络上的设备，说明是 `NSLocalNetworkUsageDescription` 的那句话。拒绝：应用显示 `connecting`，控制台是 `rutis: cannot connect (…); retrying in …`。到"设置 → 隐私与安全性 → 本地网络"里允许：下一次重试连上（`connected`），应用装上插件（Python 行运行）。
4. **提示属于应用：** 调用 `health.authorize()`（e2e 的 `coach` 在 `<dir>/<id>.authorize` 出现后调用）："健康"的授权页写着 Rutis Health，显示它的用途说明。
5. **拒绝传回调用方：** 全部关掉后点"允许"（或"不允许"）：`authorize()` 返回 `false`，`stepsToday()` 在 Python 调用方失败，`HealthAccessDenied: step counts are not shared with this app (denied)`，`code` 为 `denied`。
6. **授权：** 在"设置 → 健康 → 数据访问与设备 → Rutis Health"里打开步数：`stepsToday()` 返回今天的步数。
7. **后台与前台：** 回到主屏幕：控制台显示插件卸载、`stopped`，rutis 上这部手机的插件卸载、用它们的行停下；再打开应用：`connected`，插件重新装上，行重新运行。
8. **两台设备：** 在第二台设备（或模拟器）上以另一个 `-RutisID` 运行：rutis 上有两组 `<id>:health`、`<id>:device`，服务是 `health-<id>`、`device-<id>`，两台都可用；监听 `rutis/service` 的行看到两台各自的服务出现（§4）。
9. **断网：** 在前台时打开飞行模式：rutis 的行停下；关闭飞行模式：应用自己重连，行恢复。
