# Host Plugins in an App (Swift)

[中文](swift-app.md)

An iOS, iPadOS or macOS app can offer what it and its device can do (health data, location, contacts, the app's own features) to rutis as ordinary rutis plugins. The plugins are compiled into the app and ship with it; rutis loads, configures and unloads them as its configuration says; the app only starts and stops the connection. Needs iOS / iPadOS 17 or macOS 14 or later, and Xcode 26 or later with the Swift 6.2 toolchain.

The plugins are the same as [Swift plugins](swift-plugin.en.md) for Mac binaries: the same definition can be served as a binary or hosted by an app. This guide covers what differs: adding the SDK to an app, starting and stopping, the rutis side, permissions, and the network.

## 1. Add the SDK

In Xcode: File → Add Package Dependencies…, `https://github.com/arcships/rutis`, from `0.18.0`, and add the library `Rutis` to the app target. In a package:

```swift
.package(url: "https://github.com/arcships/rutis", from: "0.18.0"),
// in the target:
.product(name: "Rutis", package: "rutis"),
```

The first build compiles the SDK's macros (swift-syntax, at build time only), and Xcode asks once to trust them; in CI, pass `-skipMacroValidation` to `xcodebuild`. The SDK is linked into the app: nothing is loaded at run time.

## 2. Write the plugins

As in the [Swift plugin guide](swift-plugin.en.md#2-write-the-plugin): a plugin declares what it uses, provides its services in `apply` (they reach rutis by themselves, no `provides` needed), and registers its cleanups. Plugins that use the device's capabilities call the system APIs directly:

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

A plugin definition a plugin hands to a service ([Swift plugin](swift-plugin.en.md#handing-a-plugin-to-a-service)) runs in the app's process; when the app dials in, it mounts only in that connection's instance (or deeper), and every copy mounted from it ends when the app calls `stop()` or disconnects.

### Objects with methods

As in the [Swift plugin](swift-plugin.en.md#objects-with-methods): an instance of a `@RutisService` type is handed over by reference; for an object another process passes, a parameter typed with a file-level `@RutisClient` protocol (`any Cache`, not optional) gets its proxy, and one typed `Any` gets a `Rutis.Advanced.Client` (`try await client.call("get", values: ["a"], as: Int?.self)`).

```swift
@RutisClient
protocol Cache {
    func get(_ key: String) async throws -> Int?
    func set(_ key: String, _ value: Int) async throws
}

@RutisService
public final class Warmer: @unchecked Sendable {
    public func warm(_ cache: any Cache) async throws -> Int? {
        try await cache.set("a", 1)          // a call of the far end's object
        return try await cache.get("a")
    }
}
```

- Every method is an async call across processes, and there are no data fields; a method the object lacks fails with `no-method`.
- A `@RutisClient` protocol nested in another type, and a parameter of a data type, do not take objects: the call fails with `bad-argument` (`argument 1 should be a Spot, got an object (an object is received only as a @RutisClient protocol or Any)`). `[any Cache]` is not supported in 0.18; objects inside arrays and dictionaries arrive through `Any` as `Rutis.Advanced.Client`.
- A received proxy passed on sends the original object; passed back to its process, it is the original there.
- Objects the app hands over live in the app's process: after the app calls `stop()` or disconnects, calls on them elsewhere fail with `peer-gone`.
- Objects cannot go to a rutis older than 0.18: the call fails with `bad-value`, and the error says to upgrade.

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
- `start()` and `stop()` take effect in the order they are called, each in a `Task` of its own too: a `start()` called during a stop starts once the stop is done (the user came back during the cleanup: the app connects again); a `stop()` called during a start undoes it (nothing keeps dialing in the background).
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
- An app that does not have a plugin a row names fails that row in its instance only (`runtime <name>: this connection does not contain plugin …`).
- When an app stops or is suspended, its instance closes and `coach` stops there; when it starts again, a new instance loads the rows again.

For the fields and rules, see [Runtimes that connect in](nodes.en.md#runtimes-that-connect-in) and [rutis-host](rutis-host.en.md#runtimes-that-connect-in).

### Listening instead

A device with an address rutis can reach (a Mac or an iPad on a fixed address in a lab) can wait for rutis to connect instead:

```swift
let rutis = RutisApp(plugins: [health, device], listen: "wss://0.0.0.0:7443/rutis",
                     id: "lab-ipad", token: token, certificate: identity)   // a SecIdentity
```

`certificate` is the TLS identity (from a PKCS#12 file with `SecPKCS12Import`, or the Keychain); `ws://` is allowed on loopback only. Port `0` asks for a free port: the `log` line `rutis: listening on …` says which, and later starts bind that port again, so the address stays the same. One connection at a time, a newer one taking over; `state` is `.connecting` while no rutis is connected, and `.connected` once a connection's rutis greeted (within 10 s, or the connection is closed). A connection must present its upgrade request within 10 s, and at most 32 upgrade at once. If the listener fails later (the network went away), it is bound again with backoff (`rutis: listening failed (…); listening again in 1s`), a connected rutis staying connected. On the rutis side, a node row links to it, as to [a runtime on another machine](nodes.en.md#a-language-runtime-on-another-machine), with `language: "swift"`:

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

## 8. The sample app

[`swift/examples/HealthApp`](../../swift/examples/HealthApp) is an Xcode project of an iOS app hosting the `health` and `device` plugins of §2, with the start-in-front, stop-in-the-background business layer of §3. It depends on the SDK in this checkout (a local package), and reads where to connect from launch arguments (`-RutisConnect`, `-RutisID`, `-RutisToken`, and `-RutisCA`, a CA certificate in base64 DER), which Xcode's scheme sets.

On the simulator, against a `rutis-host` on the same Mac (the simulator reaches the Mac's loopback):

```bash
cargo test -p rutis-e2e --test ios_app -- --ignored
```

builds the app with `xcodebuild`, runs it on two new simulators with ids of their own and a `rutis-host run` with the group of §4 and a Python row using the app's services, and checks: each simulator is an instance whose Python row sees its own app's `device`; the app going to the background stops its row and coming back starts it again, while the other simulator's runs on; before the user decides, reading steps fails with the plugin's `HealthAccessDenied` (`code` `not-determined`) at the Python caller; asking shows the Health sheet over the app, naming it (read off a screenshot). It needs Xcode and an iOS simulator runtime (`xcodebuild -downloadPlatform iOS`), and runs locally only, as does `bash tools/ios-selftest.sh` (the SDK's own tests inside a simulator): CI runs no test that needs a simulator, a device or a permission prompt. The simulator cannot answer the sheet, and `simctl privacy` has no HealthKit service, so those are checked by hand, below.

### Manual checklist (a real device)

What only a person and a device can check. With an iPhone (iOS 17 or later) on the same network as a Mac running rutis:

1. On the Mac, make a certificate for its LAN address (any tool that makes a CA and a server certificate with the IP in its subject alternative names, for example `mkcert 192.168.1.10`), and run rutis with the configuration of §4, its listener `0.0.0.0:7443` with that certificate and key, and `RUTIS_TOKEN_PHONE` set; a Python row such as the e2e's `coach` (in `tests/e2e/tests/ios_app.rs`) shows what each instance sees.
2. Open `swift/examples/HealthApp/HealthApp.xcodeproj`, set your team in Signing & Capabilities (HealthKit stays enabled), and in the scheme's arguments set `-RutisConnect wss://<the Mac's IP>:7443/rutis`, `-RutisToken <the token>`, and `-RutisCA` to the CA certificate (`base64 -i rootCA.der`, after `openssl x509 -in rootCA.pem -outform der -out rootCA.der`). Run it on the iPhone.
3. **Local network:** iOS asks whether Rutis Health may find devices on the local network, with the sentence of `NSLocalNetworkUsageDescription`. Decline: the app shows `connecting` and the console `rutis: cannot connect (…); retrying in …`. Allow it in Settings → Privacy & Security → Local Network: the next try connects (`connected`), and rutis loads the plugins (the Python row runs).
4. **The prompt is the app's:** call `health.authorize()` (the e2e's `coach` does once `<dir>/<id>.authorize` exists): the Health sheet names Rutis Health and shows its usage descriptions.
5. **A denial reaches the caller:** turn everything off and tap Allow (or Don't Allow): `authorize()` returns `false`, and `stepsToday()` fails at the Python caller with `HealthAccessDenied: step counts are not shared with this app (denied)`, `code` `denied`.
6. **A grant:** in Settings → Health → Data Access & Devices → Rutis Health, turn steps on: `stepsToday()` returns today's steps.
7. **Background and foreground:** press Home: the console shows the plugins unloading and `stopped`, and rutis's row for this phone stops; open the app again: `connected`, and the row runs again.
8. **Two devices:** run the app on a second device (or a simulator) with another `-RutisID`: two instances, each row seeing its own device (`device.id()`).
9. **Network loss:** turn on Airplane Mode while in front: rutis's row stops; turn it off: the app reconnects by itself and the row comes back.
