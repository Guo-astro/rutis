# Host Plugins in an App (Swift)

[中文](swift-app.md)

An iOS, iPadOS or macOS app can offer what it and its device can do (health data, location, contacts, the app's own features) to rutis as ordinary rutis plugins. The plugins are compiled into the app and ship with it; once connected to rutis the app installs them itself, in order, each with the configuration the app gives, and the services they provide appear on rutis as `<id>:<plugin>`; the app only starts and stops the connection. Needs iOS / iPadOS 17 or macOS 14 or later, and Xcode 26 or later with the Swift 6.2 toolchain.

The plugins are the same as [Swift plugins](swift-plugin.en.md) for Mac binaries: the same definition can be served as a binary or hosted by an app. This guide covers what differs: adding the SDK to an app, configuration, starting and stopping, the rutis side, permissions, and the network.

## 1. Add the SDK

In Xcode: File → Add Package Dependencies…, `https://github.com/arcships/rutis`, from `0.18.0`, and add the library `Rutis` to the app target. In a package:

```swift
.package(url: "https://github.com/arcships/rutis", from: "0.18.0"),
// in the target:
.product(name: "Rutis", package: "rutis"),
```

The first build compiles the SDK's macros (swift-syntax, at build time only), and Xcode asks once to trust them; in CI, pass `-skipMacroValidation` to `xcodebuild`. The SDK is linked into the app: nothing is loaded at run time.

## 2. Write the plugins

As in the [Swift plugin guide](swift-plugin.en.md#2-provide-a-service): a plugin declares what it uses, provides its services in `apply` (they reach rutis by themselves, no `provides` needed), and registers its cleanups. Plugins that use the device's capabilities call the system APIs directly:

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

rutis does not model devices: when its rows need to know which device they are on, a plugin says so, like any service. The plugin's configuration type is written as usual ([configuration](swift-plugin.en.md)); to give it in the app, make it `Encodable` too:

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

Test plugins with `RutisTest` as in the plugin guide (§3); it runs in the iOS simulator too.

A plugin definition a plugin hands to a service ([Plugin API: handing over plugin definitions in each language](plugin-api.en.md#handing-over-plugin-definitions-in-each-language)) runs in the app's process; every copy mounted from it ends when the app calls `stop()` or disconnects.

### Objects with methods

As in the [Plugin API: handing over and receiving objects in each language](plugin-api.en.md#handing-over-and-receiving-objects-in-each-language): an instance of a `@RutisService` type is handed over by reference; for an object another process passes, a parameter typed with a file-level `@RutisClient` protocol (`any Cache`, not optional) gets its proxy, and one typed `Any` gets a `Rutis.Advanced.Client` (`try await client.call("get", values: ["a"], as: Int?.self)`).

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
    configs: ["device": DeviceConfig(installation: "kim-iphone")],
    connect: "wss://rutis.example.com:7443/rutis",
    id: "kim-iphone",                 // this app's id: lower-case letters, digits and -
    token: secrets.rutisToken)

try await rutis.start()   // dials; once connected, installs health, then device
await rutis.stop()        // withdraws the services, every plugin cleans up, latest first; then it disconnects
```

| Parameter | Meaning |
| --- | --- |
| `plugins` | The plugins installed once connected, in this order |
| `configs` | Optional: the plugins' configurations by plugin name, each a value of its configuration type (`Encodable`); a plugin without one gets its configuration Schema's defaults. A name that is not one of `plugins` makes `start()` throw: `configs names nope, which is not one of the plugins: device, health`. Configurations are given in `RutisApp` only: a plugin binary's plugins get theirs from rutis's rows |
| `connect` | The rutis access point: `wss://…`; `ws://` only on loopback (the simulator reaching the Mac it runs on) |
| `id` | This connection's id: its plugins are registered on rutis as `<id>:<plugin>` (`kim-iphone:health`); give every installation its own (for example one stored at first launch) |
| `token` | The shared token of the access point. Keep it in the Keychain, not in the app's code |
| `peer` | Optional: the `id` of the rutis to accept; another one is refused |
| `trust` | Optional: CA certificates (`SecCertificate`) trusted besides the system's, for a certificate of your own CA |
| `shutdownTimeout` | How long the plugins' cleanups may take once the connection ends, in seconds (10) |
| `log` | Where the connection's and the plugins' lines go (stderr by default; for example `{ logger.info("\($0)") }`) |

- `start()` returns once dialing has started; what retrying cannot fix (an invalid id or address) it throws. From then on it stays connected by itself: a connection that fails or drops is tried again (from 0.5 s, doubling up to 30 s); a refused token every 30 s. Calling `start()` when started does nothing.
- Once connected (rutis greeted), the app installs the plugins in the order of `plugins`, by the rule plugins follow on rutis: a plugin is installed once every service it `inject`s is there (provided by another plugin of the app, or by rutis), unloaded when one goes, and installed again when it is back. rutis's shareable services all come over to the app (withdrawn here when rutis withdraws them); every service the app's plugins provide is reported to rutis (with the plugin's name), and other rows on rutis use them by name. The app does not report back the services rutis gave it.
- A plugin that fails to load (a configuration its Schema refuses, `apply` throwing) affects only itself: `log` gets a line like `rutis: plugin device: BindingError: config: installation is required`, and the plugins after it are still installed. The line is on the device only: on rutis's side the plugin just does not appear (no `<id>:device`).
- Every connection is a new one: on disconnect the plugins clean up, latest first, and are installed again on reconnect.
- `stop()` withdraws the services and unloads every plugin, latest first, while rutis still hears it, closes the connection, and does not reconnect; it returns once done, and `start()` may follow.
- `start()` and `stop()` take effect in the order they are called, each in a `Task` of its own too: a `start()` called during a stop starts once the stop is done (the user came back during the cleanup: the app connects again); a `stop()` called during a start undoes it (nothing keeps dialing in the background).
- `state` is `.stopped(reason:)`, `.connecting` or `.connected`. `RutisApp` is `Observable`, so SwiftUI views showing `rutis.state` update by themselves; `for await state in rutis.states { … }` follows it elsewhere. `.stopped(reason: "…")` means it stopped by itself: rutis is incompatible (older than 0.13, say), which retrying cannot fix.
- The lines in `log` are those of every Swift runtime: `rutis: connected to …`, `rutis: connection lost (…); retrying in 1s`, `rutis: refused: auth-rejected (…); retrying in 30s`, `rutis: refused: incompatible (…); stopping`, and the plugins' failures.

When to start and stop is the app's business; the SDK acts on neither the app's state nor its own. On iOS an app is suspended soon after it goes to the background, and to rutis a suspended app is one whose connection was lost: its plugins unload on rutis and the rows using their services stop. The usual way is to connect while in the foreground:

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

rutis waits for apps with one `rutis-host/listen` row; devices need not be listed in rutis.json, nor the plugins the apps run:

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
RUTIS_TOKEN_PHONE=<the token the apps present> rutis-host run rutis.json
```

- Every connection let in is a session, and the plugins the app runs are registered as `<the app's id>:<plugin>` (`kim-iphone:health`, `kim-iphone:device`); the services they provide are globally visible on rutis, and other rows `inject` them by name. A newer connection with the same id replaces the older one.
- The output of `run` starts with the row id: `phone: connected kim-iphone`, `phone: disconnected kim-iphone (connection lost)`, `phone: disconnected kim-iphone (replaced by a new connection)`. The `connections:` section of `rutis-host status` lists the plugins each app registered: `phone: kim-iphone (kim-iphone:health, kim-iphone:device)`.
- When an app stops or is suspended, its plugins unload and the rows using their services stop; when it starts again, the app installs its plugins again and those rows come back.
- Same-named services follow the kernel rule: when two devices both provide `health`, the later device's `health` plugin is refused with the reason, and the first one stays usable.

### Once per device

The 0.18 way (one `py:coach` in an `instanced` group, one per device) is not supported in 0.19: rutis does not create an instance per connection. Telling devices apart is the business of the plugins on the devices, in one of two ways:

- **Put it on the device.** Write coach as a plugin in the app, using that app's `health` (it is installed once `health` is):

  ```swift
  let coach = Rutis.define(Plugin(name: "coach", inject: ["health"]) { ctx in
      let health = try ctx.use("health", HealthProxy.self)
      ctx.provide("coach", Coach(health: health))
  })
  let rutis = RutisApp(plugins: [health, coach], connect: …, id: id, token: token)
  ```

  Every device has a coach of its own, seeing its own `health`; when their service names differ too (next item), rows on rutis can use each one.

- **Let the plugin on the host tell the devices apart.** The plugins on the devices provide their services under names with the device's id:

  ```swift
  public let health = Rutis.define(Plugin(name: "health", config: DeviceConfig.self) { ctx, config in
      ctx.provide("health-\(config.installation)", Health())   // health-kim-iphone
  })
  ```

  The plugin `coach` on the host listens to the framework event `rutis/service` (its argument `{ name, scope, available, row }`, see [the plugin API: framework events](plugin-api.en.md#framework-events-rutis)): when a `health-<device>` appears (`available: true`), it adds a row `coach-<device>` for that device through the service `loader`, running `py:coach-one`, a plugin minding one device; when it is withdrawn (`available: false`), it removes the row. The `loader` methods are called in the event handler, not in `apply` (called in a row's own `apply`, they wait for that `apply` to finish, and hang):

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

  `py:coach-one` gets its device's service with `ctx.use(f"health-{config['device']}")`. The two devices' service names differ, so neither is refused; each device has a row `coach-<device>`, seen in `rutis-host status`.

### Listening instead

A device with an address rutis can reach (a Mac or an iPad on a fixed address in a lab) can wait for rutis to connect instead:

```swift
let rutis = RutisApp(plugins: [health, device], configs: ["device": DeviceConfig(installation: "lab-ipad")], listen: "wss://0.0.0.0:7443/rutis",
                     id: "lab-ipad", token: token, certificate: identity)   // a SecIdentity
```

`certificate` is the TLS identity (from a PKCS#12 file with `SecPKCS12Import`, or the Keychain); `ws://` is allowed on loopback only. Port `0` asks for a free port: the `log` line `rutis: listening on …` says which, and later starts bind that port again, so the address stays the same. One connection at a time, a newer one taking over; `state` is `.connecting` while no rutis is connected, and `.connected` once a connection's rutis greeted (within 10 s, or the connection is closed). A connection must present its upgrade request within 10 s, and at most 32 upgrade at once. If the listener fails later (the network went away), it is bound again with backoff (`rutis: listening failed (…); listening again in 1s`), a connected rutis staying connected. On the rutis side, one `rutis-host/dial` row links to it; once connected the app installs its plugins as well, registered as `lab-ipad:health` and so on:

```json
{
  "rows": [
    { "id": "ipad", "name": "rutis-host/dial", "config": { "address": "wss://10.0.0.20:7443/rutis" } }
  ]
}
```

The token goes in `RUTIS_TOKEN_IPAD` (the row id in upper case); for a certificate of your own CA, `RUTIS_CA` points at the CA certificate.

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
- **Ids.** The id is not verified by rutis: it only tells connections apart and names their remote plugins (`<id>:<plugin>`; the token is the credential). Two installations with the same id take over from each other in turn.

## 7. Mac apps

The same `RutisApp` runs in a Mac app; a sandboxed Mac app needs the `com.apple.security.network.client` entitlement to dial (and `network.server` to listen). Mac apps are not suspended in the background, so a menu bar app or login item can stay connected. A plugin that is not part of an app is better served as a [plugin binary](swift-plugin.en.md): rutis starts it on demand.

## 8. The sample app

[`swift/examples/HealthApp`](../../swift/examples/HealthApp) is an Xcode project of an iOS app hosting the `health` and `device` plugins of §2. Its service names carry this installation's id (the connection's `id`), the second way of §4 "Once per device": `health-<id>`, `device-<id>` (with `-RutisID kim-iphone`, `health-kim-iphone` and `device-kim-iphone`), given through `RutisApp`'s `configs` (`HealthConfig(service:)`, `DeviceConfig(installation:service:)`), so two devices on one rutis do not refuse each other; with the start-in-front, stop-in-the-background business layer of §3. It depends on the SDK in this checkout (a local package), and reads where to connect from launch arguments (`-RutisConnect`, `-RutisID`, `-RutisToken`, and `-RutisCA`, a CA certificate in base64 DER), which Xcode's scheme sets.

On the simulator, against a `rutis-host` on the same Mac (the simulator reaches the Mac's loopback):

```bash
cargo test -p rutis-e2e --test ios_app -- --ignored
```

builds the app with `xcodebuild`, runs it on two new simulators with ids of their own and a `rutis-host run` with the `rutis-host/listen` row of §4 and a Python row using the app's services, and checks: each simulator's app installs its own plugins (`<id>:health`, `<id>:device`) under service names of its own (`health-<id>`, `device-<id>`); the app going to the background unloads its plugins and coming back installs them again, while the other simulator is not affected; before the user decides, reading steps fails with the plugin's `HealthAccessDenied` (`code` `not-determined`) at the Python caller; asking shows the Health sheet over the app, naming it (read off a screenshot). It needs Xcode and an iOS simulator runtime (`xcodebuild -downloadPlatform iOS`), and runs locally only, as does `bash tools/ios-selftest.sh` (the SDK's own tests inside a simulator): CI runs no test that needs a simulator, a device or a permission prompt. The simulator cannot answer the sheet, and `simctl privacy` has no HealthKit service, so those are checked by hand, below.

### Manual checklist (a real device)

What only a person and a device can check. With an iPhone (iOS 17 or later) on the same network as a Mac running rutis:

1. On the Mac, make a certificate for its LAN address (any tool that makes a CA and a server certificate with the IP in its subject alternative names, for example `mkcert 192.168.1.10`), and run rutis with the configuration of §4, its listener `0.0.0.0:7443` with that certificate and key, and `RUTIS_TOKEN_PHONE` set; a Python row such as the e2e's `coach` (in `tests/e2e/tests/ios_app.rs`) shows what it sees of the app.
2. Open `swift/examples/HealthApp/HealthApp.xcodeproj`, set your team in Signing & Capabilities (HealthKit stays enabled), and in the scheme's arguments set `-RutisConnect wss://<the Mac's IP>:7443/rutis`, `-RutisToken <the token>`, and `-RutisCA` to the CA certificate (`base64 -i rootCA.der`, after `openssl x509 -in rootCA.pem -outform der -out rootCA.der`). Run it on the iPhone.
3. **Local network:** iOS asks whether Rutis Health may find devices on the local network, with the sentence of `NSLocalNetworkUsageDescription`. Decline: the app shows `connecting` and the console `rutis: cannot connect (…); retrying in …`. Allow it in Settings → Privacy & Security → Local Network: the next try connects (`connected`), and the app installs its plugins (the Python row runs).
4. **The prompt is the app's:** call `health.authorize()` (the e2e's `coach` does once `<dir>/<id>.authorize` exists): the Health sheet names Rutis Health and shows its usage descriptions.
5. **A denial reaches the caller:** turn everything off and tap Allow (or Don't Allow): `authorize()` returns `false`, and `stepsToday()` fails at the Python caller with `HealthAccessDenied: step counts are not shared with this app (denied)`, `code` `denied`.
6. **A grant:** in Settings → Health → Data Access & Devices → Rutis Health, turn steps on: `stepsToday()` returns today's steps.
7. **Background and foreground:** press Home: the console shows the plugins unloading and `stopped`, and on rutis this phone's plugins unload and the rows using them stop; open the app again: `connected`, the plugins are installed again and the rows run again.
8. **Two devices:** run the app on a second device (or a simulator) with another `-RutisID`: rutis has two sets of `<id>:health`, `<id>:device`, with services `health-<id>` and `device-<id>`, both usable; a row listening to `rutis/service` sees each device's services appear (§4).
9. **Network loss:** turn on Airplane Mode while in front: rutis's row stops; turn it off: the app reconnects by itself and the row comes back.
