# Host Plugins in an App (Swift)

[中文](swift-app.md)

An iOS, iPadOS or macOS app can offer what it and its device can do (health data, location, contacts, the app's own features) to rutis as ordinary rutis plugins. The plugins are compiled into the app and ship with it; rutis loads, configures and unloads them as its configuration says; the app only starts and stops the connection. Needs iOS / iPadOS 17 or macOS 14 or later, and Xcode 16 or later.

The plugins are the same as [Swift plugins](swift-plugin.en.md) for Mac binaries: the same definition can be served as a binary or hosted by an app. This guide covers what differs: adding the SDK to an app, starting and stopping, the rutis side, permissions, and the network.

## 1. Add the SDK

In Xcode: File → Add Package Dependencies…, `https://github.com/arcships/rutis`, from `0.15.0`, and add the library `Rutis` to the app target. In a package:

```swift
.package(url: "https://github.com/arcships/rutis", from: "0.15.0"),
// in the target:
.product(name: "Rutis", package: "rutis"),
```

The first build compiles the SDK's macros (swift-syntax, at build time only), and Xcode asks once to trust them; in CI, pass `-skipMacroValidation` to `xcodebuild`. The SDK is linked into the app: nothing is loaded at run time.

## 2. Write the plugins

As in the [Swift plugin guide](swift-plugin.en.md#2-write-the-plugin): a plugin declares what it uses and provides, provides its services in `apply`, and registers its cleanups. Plugins that use the device's capabilities call the system APIs directly:

```swift
import HealthKit
import Rutis

/// Thrown when the user has not granted access: the caller gets it, in any language.
struct HealthAccessDenied: Error, LocalizedError {
    let code: String   // "not-determined" or "denied"
    var errorDescription: String? { "step counts are not shared with this app (\(code))" }
}

@RutisService
public final class Health: @unchecked Sendable {
    let store = HKHealthStore()
    let steps = HKQuantityType(.stepCount)

    /// Asks the user (the prompt names the app); whether access was granted.
    public func authorize() async throws -> Bool {
        try await store.requestAuthorization(toShare: [steps], read: [steps])
        return store.authorizationStatus(for: steps) == .sharingAuthorized
    }

    /// Today's steps.
    public func stepsToday() async throws -> Double {
        switch store.authorizationStatus(for: steps) {
        case .notDetermined: throw HealthAccessDenied(code: "not-determined")
        case .sharingDenied: throw HealthAccessDenied(code: "denied")
        default: break
        }
        // … an HKStatisticsQuery for today
    }
}

public let health = Rutis.define(Plugin(name: "health") { ctx in
    ctx.provide("health", Health())
})
```

rutis does not model devices: when its rows need to know which device they are on, a plugin says so, like any service:

```swift
@RutisService
public final class Device: @unchecked Sendable {
    public func name() async -> String { await UIDevice.current.name }
    public func system() async -> String { await "\(UIDevice.current.systemName) \(UIDevice.current.systemVersion)" }
}

public let device = Rutis.define(Plugin(name: "device") { ctx in ctx.provide("device", Device()) })
```

Test plugins with `RutisTest` as in the plugin guide (§3); it runs in the iOS simulator too.

## 3. Start and stop

```swift
import Rutis

let rutis = RutisApp(
    plugins: [health, device],
    connect: "wss://rutis.example.com:7443/rutis",
    id: "kim-iphone",                 // this app's id: lower-case letters, digits and -
    token: secrets.rutisToken)

try await rutis.start()   // dials; rutis then loads the plugins its configuration names
await rutis.stop()        // every plugin cleans up, latest first; then it disconnects
```

| Parameter | Meaning |
| --- | --- |
| `plugins` | The plugins rutis may load. It loads those its configuration names, with their config; the others stay unloaded |
| `connect` | The rutis access point: `wss://…`; `ws://` only on loopback (the simulator reaching the Mac it runs on) |
| `id` | This connection's id, which names its instance on the rutis side; give every installation its own (for example one stored at first launch) |
| `token` | The shared token of the access point. Keep it in the Keychain, not in the app's code |
| `peer` | Optional: the `id` of the rutis to accept; another one is refused |
| `trust` | Optional: CA certificates (`SecCertificate`) trusted besides the system's, for a certificate of your own CA |
| `shutdownTimeout` | How long the plugins' cleanups may take once the connection ends, in seconds (10) |
| `log` | Where the connection's and the plugins' lines go (stderr by default; for example `{ logger.info("\($0)") }`) |

- `start()` returns once dialing has started; what retrying cannot fix (an invalid id or address) it throws. From then on it stays connected by itself: a connection that fails or drops is tried again (from 0.5 s, doubling up to 30 s); a refused token every 30 s; every connection is a new one, and rutis loads the plugins again. Calling `start()` when started does nothing.
- `stop()` unloads every plugin while rutis still hears it, closes the connection, and does not reconnect; it returns once done, and `start()` may follow.
- `state` is `.stopped(reason:)`, `.connecting` or `.connected`. `RutisApp` is `Observable`, so SwiftUI views showing `rutis.state` update by themselves; `for await state in rutis.states { … }` follows it elsewhere. `.stopped(reason: "…")` means it stopped by itself: rutis is incompatible (older than 0.13, say), which retrying cannot fix.
- The lines in `log` are those of every Swift runtime: `rutis: connected to …`, `rutis: connection lost (…); retrying in 1s`, `rutis: refused: auth-rejected (…); retrying in 30s`, `rutis: refused: incompatible (…); stopping`, and the plugins' failures.

When to start and stop is the app's business; the SDK acts on neither the app's state nor its own. On iOS an app is suspended soon after it goes to the background, and to rutis a suspended app is one whose connection was lost: its instance closes and the rows using its services stop. The usual way is to connect while in the foreground:

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
                // Time for the cleanups before iOS suspends the app.
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

An app that needs to stay connected longer uses what iOS allows it (a background mode that fits what it does); a Mac app (a menu bar app, a login item) can stay connected all along.

## 4. The rutis side

rutis listens for apps on an access point, and every app that connects is an instance of a group: the group's rows say which plugins to load on each app, with which config, and which rows of rutis's own use them.

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
RUTIS_TOKEN_PHONE=<the token the apps present> rutis-host run rutis.json
```

- Each app connection creates an instance named by its id, and closes it when it goes; a newer connection with the same id takes over.
- In each instance, `phone:health` loads the app's `health` plugin, and `py:coach` (a Python plugin on the rutis machine) uses that app's `health` and `device`. Two phones with the same plugins are two instances: each `coach` sees its own phone only, and rows outside the group see no phone's services.
- An app that does not have a plugin a row names fails that row in its instance only (`this connection does not contain plugin …`).
- When an app stops or is suspended, its instance closes and `coach` stops there; when it starts again, a new instance loads the rows again.

For the fields and rules, see [Runtimes that connect in](nodes.en.md#runtimes-that-connect-in) and [rutis-host](rutis-host.en.md#runtimes-that-connect-in).

### Listening instead

A device with an address rutis can reach (a Mac or an iPad on a fixed address in a lab) can wait for rutis to connect instead:

```swift
let rutis = RutisApp(plugins: [health, device], listen: "wss://0.0.0.0:7443/rutis",
                     id: "lab-ipad", token: token, certificate: identity)   // a SecIdentity
```

`certificate` is the TLS identity (from a PKCS#12 file with `SecPKCS12Import`, or the Keychain); `ws://` is allowed on loopback only. One connection at a time, a newer one taking over; `state` is `.connecting` while no rutis is connected. On the rutis side, a node row links to it, as to [a runtime on another machine](nodes.en.md#a-language-runtime-on-another-machine), with `language: "swift"`:

```json
{
  "runtimes": { "remote": [{ "name": "ipad", "language": "swift" }] },
  "rows": [
    { "id": "ipad", "name": "rutis-bridge/peer", "config": { "peer": "lab-ipad", "dial": "wss://10.0.0.20:7443/rutis", "runtime": "ipad" } },
    { "id": "health", "name": "ipad:health" }
  ]
}
```

Dialing out is the usual way: devices change address and sit behind NAT, and only rutis needs a certificate.

## 5. Permissions

What the system asks the user (health data, location, contacts, photos) belongs to the app: the prompt names the app, and the answer applies to it. Every plugin in the app shares the app's permissions.

- Add the capability (HealthKit: the HealthKit capability, which adds the `com.apple.developer.healthkit` entitlement) and the usage descriptions to the app's `Info.plist` (`NSHealthShareUsageDescription`, `NSHealthUpdateUsageDescription`, `NSLocationWhenInUseUsageDescription`, `NSContactsUsageDescription`, …). Without the description the app crashes when it asks.
- A refusal is an ordinary failure: check the answer and throw. The caller, in any language on the rutis side, gets the error with its name, message and `code` (the example above throws `HealthAccessDenied` with `code` `denied`).
- A prompt appears over the app and the call that asked waits for the user's answer: ask while the app is in front (rutis calls the plugin while the app is connected, which with the pattern above is while it is in front), or from the app's own interface.
- HealthKit does not tell an app whether reading was denied (reads return no data); the sharing status (`authorizationStatus(for:)`) tells. `HKHealthStore.isHealthDataAvailable()` is false on devices without Health (some iPads).

## 6. The network

- **Local network permission (iOS 14+).** The first time the app connects to an address on the local network (a rutis on `wss://192.168.1.10:7443`, a `.local` name), iOS asks the user whether the app may find and connect to devices on the local network. Add `NSLocalNetworkUsageDescription` to `Info.plist` with a sentence saying why; without it the prompt says nothing useful. Until the user allows it the connection fails, and the app keeps retrying (`rutis: cannot connect (…); retrying in …`, `state` `.connecting`); once allowed, the next try connects. If the user declined, only the Settings app (Privacy & Security → Local Network) can change it: say so in the app. Listening on a local address asks the same. rutis on the Internet (a public name or address), and loopback in the simulator, need no such permission. The SDK does not use Bonjour, so `NSBonjourServices` is not needed.
- **TLS.** A non-loopback address must be `wss://`. A certificate from a public CA needs nothing more; for your own CA, pass it in `trust`. App Transport Security applies to the connection: TLS 1.2 or later with forward secrecy (what rutis's listener serves).
- **Ids.** The id is not verified by rutis: it only tells connections apart and names their instances (the token is the credential). Two installations with the same id take over from each other in turn.

## 7. Mac apps

The same `RutisApp` runs in a Mac app; a sandboxed Mac app needs the `com.apple.security.network.client` entitlement to dial (and `network.server` to listen). Mac apps are not suspended in the background, so a menu bar app or login item can stay connected. A plugin that is not part of an app is better served as a [plugin binary](swift-plugin.en.md): rutis starts it on demand.
