# 在应用里托管插件（Swift）

[English](swift-app.en.md)

iOS、iPadOS 或 macOS 应用可以把它自己和所在设备的能力（健康数据、定位、通讯录、应用本身的功能）作为普通的 rutis 插件提供给 rutis。插件编进应用、随应用发布；装载、配置、卸载都由 rutis 按配置决定；应用只负责开始和停止连接。需要 iOS / iPadOS 17 或 macOS 14 以上，Xcode 26 以上（带 Swift 6.2 工具链）。

插件与 Mac 上的 [Swift 插件](swift-plugin.md) 完全相同：同一个插件定义既可以 `serve` 成二进制，也可以交给应用托管。本文只写不同的部分：把 SDK 加进应用、开始与停止、rutis 一侧、权限、网络。

## 1. 加入 SDK

在 Xcode 里：File → Add Package Dependencies…，填 `https://github.com/arcships/rutis`，从 `0.17.0` 起，把库 `Rutis` 加到应用的 target。在包里：

```swift
.package(url: "https://github.com/arcships/rutis", from: "0.17.0"),
// target 里：
.product(name: "Rutis", package: "rutis"),
```

第一次构建会编译 SDK 的宏（swift-syntax，只在编译期），Xcode 会要求信任一次；CI 里给 `xcodebuild` 加 `-skipMacroValidation`。SDK 静态链入应用，运行时不装载任何东西。

## 2. 写插件

与 [Swift 插件指南](swift-plugin.md#2-写插件) 相同：插件声明它用什么，在 `apply` 里提供服务（服务自动对外，不需要写 `provides`）、登记清理。用到设备能力的插件直接调用系统 API：

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

rutis 不建模设备：行需要知道自己在哪台设备上时，由插件像提供任何服务一样提供：

```swift
@RutisService
public final class Device: @unchecked Sendable {
    public func name() async -> String { await UIDevice.current.name }
    public func system() async -> String { await "\(UIDevice.current.systemName) \(UIDevice.current.systemVersion)" }
}

public let device = Rutis.define(Plugin(name: "device") { ctx in ctx.provide("device", Device()) })
```

插件的测试照插件指南 §3 用 `RutisTest`，在 iOS 模拟器里同样能跑。

## 3. 开始与停止

```swift
import Rutis

let rutis = RutisApp(
    plugins: [health, device],
    connect: "wss://rutis.example.com:7443/rutis",
    id: "kim-iphone",                 // 这个应用的 id：小写字母、数字和 -
    token: secrets.rutisToken)

try await rutis.start()   // 拨出；此后 rutis 按配置装载插件
await rutis.stop()        // 全部插件逆序清理，然后断开
```

| 参数 | 含义 |
| --- | --- |
| `plugins` | rutis 可以装载的插件。它装载配置里写到的，带上各自的配置；其余不装 |
| `connect` | rutis 的接入点：`wss://…`；`ws://` 只限回环（模拟器连它所在的 Mac） |
| `id` | 这条连接的 id，在 rutis 侧命名它的实例；每个安装用自己的 id（例如首次启动时生成并保存） |
| `token` | 接入点的共享 token。放在钥匙串里，不要写在应用代码里 |
| `peer` | 可选：接受的 rutis 的 `id`；对端是别的 id 时拒绝 |
| `trust` | 可选：在系统根证书之外额外信任的 CA 证书（`SecCertificate`），用于自己的 CA 签发的证书 |
| `shutdownTimeout` | 连接结束后插件清理最多可用的秒数（10） |
| `log` | 连接与插件的状态行写到哪里（默认 stderr；例如 `{ logger.info("\($0)") }`） |

- `start()` 开始拨出后即返回；重试修不好的（不合法的 id 或地址）直接抛出。此后它自己保持连接：连不上或断开时重试（从 0.5 秒起翻倍，最长 30 秒）；token 被拒每 30 秒重试；每次连接都是新的，rutis 重新装载插件。已经开始时再调用 `start()` 什么也不做。
- `stop()` 在 rutis 还能收到的时候卸载全部插件，然后关闭连接，不再重连；完成后返回，之后可以再 `start()`。
- `start()` 与 `stop()` 按调用的先后生效，各在自己的 `Task` 里调用也一样：停止进行中调用的 `start()` 等它完成后再开始（用户在清理期间回到前台，应用照样连上）；开始进行中调用的 `stop()` 撤销它（不会在后台继续拨出）。
- `state` 是 `.stopped(reason:)`、`.connecting` 或 `.connected`。`RutisApp` 遵循 `Observable`，显示 `rutis.state` 的 SwiftUI 视图会自己更新；别处用 `for await state in rutis.states { … }` 跟随。`.stopped(reason: "…")` 表示它自己停下了：rutis 不兼容（例如早于 0.13），重试修不好。
- `log` 收到的行与所有 Swift 运行时相同：`rutis: connected to …`、`rutis: connection lost (…); retrying in 1s`、`rutis: refused: auth-rejected (…); retrying in 30s`、`rutis: refused: incompatible (…); stopping`，以及插件的失败。

何时开始、何时停止由应用决定；SDK 不据应用的状态、也不据自己的状态做任何事。iOS 上应用进入后台后很快被挂起，在 rutis 看来被挂起的应用就是断开了：它的实例关闭，用它服务的行停下。常见做法是在前台时连着：

```swift
@main
struct HealthApp: App {
    @Environment(\.scenePhase) private var phase
    @State private var rutis = RutisApp(plugins: [health, device], connect: Settings.address,
                                        id: Settings.id, token: Settings.token)

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

rutis 在一个接入点上等应用连进来，每个连进来的应用是一个分组的一个实例：分组里的行写明在每个应用上装哪些插件、用什么配置，以及 rutis 自己的哪些行使用它们。

```json
{
  "id": "main",
  "listen": [{ "name": "public", "address": "0.0.0.0:7443", "cert": "server.pem", "key": "server.key" }],
  "runtimes": {
    "py": { "project": "plugins" },
    "remote": [{ "name": "phone", "language": "swift", "listen": "public", "instances": "phone" }]
  },
  "rows": [
    { "id": "phone", "group": true, "instanced": true, "config": [
      { "id": "health", "name": "phone:health" },
      { "id": "device", "name": "phone:device" },
      { "id": "coach", "name": "py:coach", "inject": ["health", "device"] }
    ] }
  ]
}
```

```bash
RUTIS_TOKEN_PHONE=<应用出示的 token> rutis-host run rutis.json
```

- 每条应用连接以它的 id 创建一个实例，断开时关闭；同一 id 的新连接接管旧的。
- 每个实例里，`phone:health` 装载该应用的 `health` 插件，`py:coach`（rutis 机器上的 Python 插件）使用该应用的 `health` 与 `device`。两部手机带同样的插件就是两个实例：每个 `coach` 只看到自己那部手机，分组外的行看不到任何手机的服务。
- 应用没有某一行写到的插件时，只有这一行在它的实例里失败（`this connection does not contain plugin …`）。
- 应用停止或被挂起时它的实例关闭，那里的 `coach` 停下；再次开始时新的实例重新装载这些行。

字段与规则见 [连进来的运行时](nodes.md#连进来的运行时) 和 [rutis-host](rutis-host.md#连进来的运行时)。

### 改为监听

rutis 能访问到的设备（实验室里地址固定的 Mac 或 iPad）也可以等 rutis 连过来：

```swift
let rutis = RutisApp(plugins: [health, device], listen: "wss://0.0.0.0:7443/rutis",
                     id: "lab-ipad", token: token, certificate: identity)   // 一个 SecIdentity
```

`certificate` 是 TLS 身份（用 `SecPKCS12Import` 从 PKCS#12 文件得到，或取自钥匙串）；`ws://` 只限回环。端口写 `0` 表示取一个空闲端口：`log` 的 `rutis: listening on …` 一行给出是哪个，之后的每次开始都再绑定这个端口，地址保持不变。一次一个连接，新连接接管；没有 rutis 连着时 `state` 是 `.connecting`，连接上的 rutis 问候之后才是 `.connected`（10 秒内没有问候就关闭这个连接）。连接须在 10 秒内发来升级请求，同时升级的连接至多 32 个。监听器之后失败时（网络断了）按退避重新绑定（`rutis: listening failed (…); listening again in 1s`），已连着的 rutis 不受影响。rutis 一侧用一个节点行连接它，与 [别的机器上的语言运行时](nodes.md#别的机器上的语言运行时) 相同，`language` 为 `"swift"`：

```json
{
  "runtimes": { "remote": [{ "name": "ipad", "language": "swift" }] },
  "rows": [
    { "id": "ipad", "name": "rutis-bridge/peer", "config": { "peer": "lab-ipad", "dial": "wss://10.0.0.20:7443/rutis", "runtime": "ipad" } },
    { "id": "health", "name": "ipad:health" }
  ]
}
```

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
- **id。** rutis 不核实 id：它只用来区分连接、给实例命名（凭据是 token）。两个安装用同一个 id 会轮流把对方顶掉。

## 7. Mac 应用

同一个 `RutisApp` 也在 Mac 应用里运行；沙盒里的 Mac 应用拨出需要 entitlement `com.apple.security.network.client`（监听需要 `network.server`）。Mac 应用在后台不会被挂起，菜单栏应用或登录项可以一直连着。不属于某个应用的插件更适合做成 [插件二进制](swift-plugin.md)：由 rutis 按需启动。

## 8. 示例应用

[`swift/examples/HealthApp`](../../swift/examples/HealthApp) 是一个 iOS 应用的 Xcode 项目，托管 §2 的 `health` 与 `device` 插件，业务层按 §3 在前台开始、进入后台停止。它依赖本仓库里的 SDK（本地包），连接到哪里由启动参数给出（`-RutisConnect`、`-RutisID`、`-RutisToken`，以及 `-RutisCA`：base64 的 DER 格式 CA 证书），在 Xcode 的 scheme 里设置。

在模拟器上，连同一台 Mac 上的 `rutis-host`（模拟器能访问 Mac 的回环地址）：

```bash
cargo test -p rutis-e2e --test ios_app -- --ignored
```

它用 `xcodebuild` 构建应用，在两台新建的模拟器上以各自的 id 运行，`rutis-host run` 带 §4 的分组和一个使用应用服务的 Python 行，检查：每台模拟器是一个实例，其中的 Python 行看到的是自己那个应用的 `device`；应用进入后台时它的行停下、回到前台时重新开始，另一台的行不受影响；用户决定之前读步数失败，Python 调用方收到插件的 `HealthAccessDenied`（`code` 为 `not-determined`）；请求授权时"健康"的授权页显示在应用之上、写着应用的名字（从截图里读出）。需要 Xcode 与 iOS 模拟器运行时（`xcodebuild -downloadPlatform iOS`），只在本地运行，`bash tools/ios-selftest.sh`（SDK 自己的测试在模拟器里运行）也一样：CI 不运行需要模拟器、设备或授权提示的测试。模拟器上无法点击授权页，`simctl privacy` 也没有 HealthKit，这些由下面的人工清单检查。

### 人工验收清单（真机）

只有人和真机能检查的部分。iPhone（iOS 17 以上）与运行 rutis 的 Mac 在同一网络：

1. 在 Mac 上为它的局域网地址签一张证书（任何能生成 CA 和服务器证书、把 IP 写进 subject alternative names 的工具，例如 `mkcert 192.168.1.10`），按 §4 的配置运行 rutis，监听器 `0.0.0.0:7443` 用这张证书和私钥，设置 `RUTIS_TOKEN_PHONE`；一个 Python 行（例如 e2e 的 `coach`，在 `tests/e2e/tests/ios_app.rs` 里）显示每个实例看到什么。
2. 打开 `swift/examples/HealthApp/HealthApp.xcodeproj`，在 Signing & Capabilities 里选你的 team（HealthKit 保持开启），在 scheme 的参数里设 `-RutisConnect wss://<Mac 的 IP>:7443/rutis`、`-RutisToken <token>`，`-RutisCA` 设为 CA 证书（先 `openssl x509 -in rootCA.pem -outform der -out rootCA.der`，再 `base64 -i rootCA.der`）。在 iPhone 上运行。
3. **本地网络：** iOS 询问是否允许 Rutis Health 查找本地网络上的设备，说明是 `NSLocalNetworkUsageDescription` 的那句话。拒绝：应用显示 `connecting`，控制台是 `rutis: cannot connect (…); retrying in …`。到"设置 → 隐私与安全性 → 本地网络"里允许：下一次重试连上（`connected`），rutis 装载插件（Python 行运行）。
4. **提示属于应用：** 调用 `health.authorize()`（e2e 的 `coach` 在 `<dir>/<id>.authorize` 出现后调用）："健康"的授权页写着 Rutis Health，显示它的用途说明。
5. **拒绝传回调用方：** 全部关掉后点"允许"（或"不允许"）：`authorize()` 返回 `false`，`stepsToday()` 在 Python 调用方失败，`HealthAccessDenied: step counts are not shared with this app (denied)`，`code` 为 `denied`。
6. **授权：** 在"设置 → 健康 → 数据访问与设备 → Rutis Health"里打开步数：`stepsToday()` 返回今天的步数。
7. **后台与前台：** 回到主屏幕：控制台显示插件卸载、`stopped`，rutis 上这部手机的行停下；再打开应用：`connected`，行重新运行。
8. **两台设备：** 在第二台设备（或模拟器）上以另一个 `-RutisID` 运行：两个实例，各自的行看到自己的设备（`device.id()`）。
9. **断网：** 在前台时打开飞行模式：rutis 的行停下；关闭飞行模式：应用自己重连，行恢复。
