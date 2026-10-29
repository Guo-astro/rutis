# App integration

[中文](design-app-integration-2026-10-14.md)

Status: final, not implemented. Date: 2026-10-14. Baseline: `main` `a763cdc` (0.10.0).
From 0.19, D2, D5, D6, D8 and §5 are rewritten by decisions 2 and 3 of [Remote connections: one session plugin](design-remote-session-2026-10-28.en.md) §2.6: the plugins on the device are installed and configured by the app itself, and rutis does not create an instance per connection.
Based on: [Remote runtimes: dialing out and dynamic connections](design-remote-runtime-dial-2026-10-14.en.md) (how apps connect), [the multilingual design](design-multilang-runtimes-2026-10-03.en.md), [remote plugins](design-remote-plugins-2026-10-03.en.md), [the plugin API](guide/plugin-api.en.md).
Per platform: [the Swift SDK](design-swift-sdk-2026-10-12.en.md) §7 (iOS / iPadOS / macOS apps), [Android app hosting](design-android-runtime-2026-10-12.en.md) (Kotlin).

This document defines the general, language-independent model for "an app hosting rutis plugins". Swift multi-process plugins on the Mac are not covered here; see Swift SDK §3.

Out of scope: loading new code on the device (download, upload, hot replacement); the SDK managing the platform's foreground/background lifecycle; an identity and pairing system (it needs a management service, not implemented in this project); the app ecosystem calling back into rutis (App Intents, Android Intents); a Kotlin desktop JVM runtime; running rutis itself on the device.

## 1. Decisions

| # | Decision | Section |
| --- | --- | --- |
| D1 | Capabilities in an app are written in the rutis plugin model (`inject` / `provides` / `apply(ctx, config)` / `effect`), the same concept as plugins in other languages | 3 |
| D2 | Once connected to rutis, the app installs the plugins it carries itself, in order, and unloads them in reverse on disconnect or `stop()`; rutis does not drive them | 3 |
| D3 | Plugins are compiled into the app and ship with it; no new code is loaded on the device | 3 |
| D4 | The SDK gives the app only `start` / `stop`; foreground/background, keep-alive and wake-up belong to the business layer | 4 |
| D5 | The app reports the services its plugins provide with `services.announce`, naming the plugin; rutis registers them as `<id>:<plugin>` | 5 |
| D6 | A plugin's configuration is written in the app (each plugin in `RutisApp` may carry one; without one, its configuration Schema's defaults); the rutis configuration does not list the app's plugins | 5 |
| D7 | rutis does not model devices: device IDs, device information and the like are services provided by ordinary plugins; rutis only does plugin discovery and connection | 3 |
| D8 | No aliases, and no instance per connection: when several apps provide same-named services, the kernel rule applies, the later one is refused with the reason; telling devices apart is the business of the plugins on the devices | 5 |
| D9 | The app is recommended to dial out by IP / server name to a rutis access point (each SDK also supports listening, see dial-out design R1); connections, identity and reconnection follow the dial-out design | 6 |
| D10 | The SDKs are native implementations (Swift / Kotlin), with no embedded Rust | 8 |

## 2. Requirements

Core requirement: let rutis use capabilities on Apple and Android devices (contacts, notifications, location, health data, the app's own features, and so on).

| Real-world constraint | Effect on the design |
| --- | --- |
| On mobile platforms a developer ships **one app** (signed, distributed through a store or sideloaded) | The unit of integration is the app; plugins ship with the app (D3) |
| The same app runs on different devices and OS versions, and the system grants different background permissions | rutis makes no assumptions about being online; it only sees whether a service is "there" or "not there" (D4); device differences are handled and provided by the plugins themselves (D7) |
| Devices sit behind NAT, change addresses, and sleep | The app dials out (D9) |
| System permissions are granted by the user to the app | A denied permission = `apply` or a method call fails, an ordinary error within the model |

## 3. Model

```text
rutis (desktop / server)                        App on the device
 ├─ rutis-host/listen (wss)  ◀──dial out────────  ├─ business code: decides when to start / stop
 ├─ one session per connection                    ├─ rutis SDK: once connected, installs the plugins with their configurations
 │    └─ <id>:<plugin>: the plugins the app runs  │    └─ plugin definitions compiled into the app (the model)
 └─ other rows inject the app's services by name  │         e.g. health, contacts, device
                                                  └─ system APIs (permissions belong to the app)
```

- The model matches [the plugin API](guide/plugin-api.en.md): a plugin declares its dependencies and the services it provides, and in `apply` uses services, provides services and registers cleanup; configuration comes from the app (D6).
- The same plugin definition can either be `serve`d as a binary on the Mac or handed to an app to host (Swift SDK §2).
- Between plugins: calls inside the same app get the object itself; across connections or languages they go by name through rutis.
- rutis has only two jobs: plugin discovery and connection. Device ID, model, OS version and the like are provided by plugins (for example a `device` plugin); rows that need them `inject` it.

## 4. The SDK's surface for the app

| Primitive | Semantics |
| --- | --- |
| `start()` | Connect to rutis, install the plugins in order (each with its configuration) and report the services they provide; reconnect automatically after disconnection until `stop()` |
| `stop()` | Clean up all plugins in reverse order, then disconnect; `start()` may be called again |

The SDK guarantees:

- Whether it is `stop`, a network drop or the system killing the process, rutis sees the same thing: the session unloads, `<id>:<plugin>` unload, services are withdrawn, consumers stop.
- Reconnecting is a new session: the app installs its plugins again, independent of any state from the previous connection.
- Data a plugin needs to persist is handled by the plugin itself with platform storage APIs; the SDK provides no storage contract.
- Connection state is observable, for the business layer to display; the SDK makes no lifecycle decisions based on it.

The SDK does not do: foreground/background handling, background keep-alive, push wake-up, battery-optimization prompts, permission reconciliation.

## 5. Loading and naming

- The app installs its plugins itself: once connected (rutis greeted), in the order of `RutisApp`, with the configuration the app gives (`RutisApp`'s `configs`, by plugin name), or the configuration Schema's defaults without one. The rule is the kernel's: a plugin is `apply`d once every service it `inject`s is there (another plugin's of the app, or rutis's), unloaded when one goes, installed again when it is back. A plugin that fails to load is reported alone, on the device (`rutis: plugin <name>: <error>`), and the ones after it are still installed; the session protocol has no frame reporting it to the host, which sees the plugin not appear. On disconnect or `stop()` the services are withdrawn first, then the plugins clean up in reverse.
- Reporting services: after the handshake both sides offer the `services` family (`link.offers`); each announces only once the far end offered it, and again to every new offer. For each service a plugin provides, the app sends `services.announce { name, service, shape, version, plugin }` (the frame nodes already use, plus the name of the plugin providing it); on withdrawal, `services.withdraw { name, version }`. `version` comes from one increasing sequence per session. rutis sends its shareable services to the app with the same frames (not those that came over this session); the app keeps the newest version of each and drops it on withdrawal, and does not report rutis's services back. rutis registers each plugin as a remote plugin `<id>:<plugin>` (`kim-iphone:health`), and the services it provides are globally visible.
- rutis's rows no longer load the app's plugins: `plugins` in the `mount` reply is empty, and `rows.load` does not find the app's plugins. The rutis configuration has one row of a connection plugin (`rutis-host/listen` or `rutis-host/dial`) and lists no devices.
- Same-named services follow the kernel rule: the later registration is refused, and that remote plugin fails with the reason. Logic that must run once per device goes on the device; a plugin on the host that handles several devices tells them apart itself, or the plugins on the devices use different service names (`health-<device>`).

## 6. Connection

Everything follows [Remote runtimes: dialing out and dynamic connections](design-remote-runtime-dial-2026-10-14.en.md):

- The app dials out by IP or server name to a rutis access point; non-loopback must use `wss://` (dial-out design §3).
- Connections are told apart by an id the app reports itself; during the transition a shared token is used; identity, pairing and revocation belong to a future management service (dial-out design §5).
- Reconnection and takeover by the same id follow dial-out design §6; disconnecting unloads the connection's session and the `<id>:<plugin>` under it. Swift / Kotlin pass the same conformance tests.

## 7. Platform reference (business layer, not an SDK obligation)

| Platform | Background capability | Common business-layer practice |
| --- | --- | --- |
| macOS apps | Can stay resident | Login item, menu bar app, always on |
| iOS / iPadOS | Suspended soon after going to the background; only a few categories may run in the background | `start` in the foreground, `stop` in the background; when needed, wake with a push and `start` briefly |
| Android | A foreground service can stay resident (with a persistent notification; from Android 14 its type must be declared); vendor systems may kill the process | When residency is needed, run a foreground service and guide the user to the battery-optimization allowlist; otherwise follow the UI |
| watchOS / visionOS | More restricted / same as iOS | Not covered for now |

Integration notes (the iOS local network permission, usage descriptions for each capability, Android foreground service types, and so on) go into each platform's SDK guide.

## 8. Implementation: native

The Swift and Kotlin SDKs each implement the session protocol, the runtime contract and connections in both directions in native code, as Python, Node, Bun and Go do; no embedded Rust (dial-out design R8).

| Consideration | Conclusion |
| --- | --- |
| Criterion | An embedded Rust connection module only pays off if every remote-plugin language can accept it |
| Per language | Swift and Kotlin can accept it; Python and Node / Bun would have to maintain a matrix of per-platform prebuilt packages; Go could only use cgo, which breaks pure static cross-compilation and conflicts with the Go runtime's "standard library only" |
| Local multi-process | A native session implementation is needed in each language anyway; a Rust module that only takes over remote connections gains little, and taking over the session would mean rewriting the existing runtimes |
| Precedent | The existing remote runtimes in every language are native implementations |

- The two implementations are kept consistent by the existing session and runtime conformance tests (`runtime_conformance`, `multilang`) plus the dial-out design's `dial_conformance`.
- Embedded Rust is evaluated only when a need to "run rutis itself on the device" appears, in a separate design.

## 9. Phases

| Phase | Work | Acceptance |
| --- | --- | --- |
| Prerequisite | Dial-out design R-P0, R-P1 | See the dial-out design |
| P1 | iOS and Android together: Swift SDK phase A1, Android design phases K-P1, K-P2 | The example apps on both platforms each dial out and connect, load plugins according to rutis configuration, and rows on the rutis side can use their services; after `stop` the consumers stop, after `start` they recover; same-named plugins on two devices each live in their own instance |
| P2 | Example capability plugins and SDK guides (Android design K-P3, Swift shape ② permission acceptance) | On real devices: the permission prompt identifies the app, and after a denial the error is passed back |
