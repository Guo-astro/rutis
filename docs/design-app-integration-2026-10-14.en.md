# App integration

[中文](design-app-integration-2026-10-14.md)

Status: final, not implemented. Date: 2026-10-14. Baseline: `main` `a763cdc` (0.10.0).
Based on: [Remote runtimes: dialing out and dynamic connections](design-remote-runtime-dial-2026-10-14.en.md) (how apps connect), [the multilingual design](design-multilang-runtimes-2026-10-03.en.md), [remote plugins](design-remote-plugins-2026-10-03.en.md), [the plugin API](guide/plugin-api.en.md).
Per platform: [the Swift SDK](design-swift-sdk-2026-10-12.en.md) §7 (iOS / iPadOS / macOS apps), [Android app hosting](design-android-runtime-2026-10-12.en.md) (Kotlin).

This document defines the general, language-independent model for "an app hosting rutis plugins". Swift multi-process plugins on the Mac are not covered here; see Swift SDK §3.

Out of scope: loading new code on the device (download, upload, hot replacement); the SDK managing the platform's foreground/background lifecycle; an identity and pairing system (it needs a management service, not implemented in this project); the app ecosystem calling back into rutis (App Intents, Android Intents); a Kotlin desktop JVM runtime; running rutis itself on the device.

## 1. Decisions

| # | Decision | Section |
| --- | --- | --- |
| D1 | Capabilities in an app are written in the rutis plugin model (`inject` / `provides` / `apply(ctx, config)` / `effect`), the same concept as plugins in other languages | 3 |
| D2 | rutis drives loading, configuring and unloading plugins; the app does not decide these itself | 3 |
| D3 | Plugins are compiled into the app and ship with it; no new code is loaded on the device | 3 |
| D4 | The SDK gives the app only `start` / `stop`; foreground/background, keep-alive and wake-up belong to the business layer | 4 |
| D5 | When the app connects, it reports the plugins it carries (`plugins` in the `mount` reply, with the same content as `plugins` in `--rutis-manifest`) | 5 |
| D6 | Which plugins are loaded, and with what configuration, is decided by rows in the rutis configuration; plugins the app carries but the configuration does not mention are not loaded | 5 |
| D7 | rutis does not model devices: device IDs, device information and the like are services provided by ordinary plugins; rutis only does plugin discovery and connection | 3 |
| D8 | No aliases: when several app connections carry same-named plugins, each connection is an instance, and same-named services resolve within their own instance | 5 |
| D9 | The app dials out by IP / server name to a rutis access point; connections, instances, identity and reconnection all follow the dial-out design | 6 |
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
 ├─ access point (listening, wss)  ◀──dial out──  ├─ business code: decides when to start / stop
 ├─ one instance per connection                   ├─ rutis SDK
 │    └─ rows in config: which plugin, what config │    └─ plugin definitions compiled into the app (the model)
 └─ rows in the same instance inject the app's    │         e.g. health, contacts, device
    services by name                              └─ system APIs (permissions belong to the app)
```

- The model matches [the plugin API](guide/plugin-api.en.md): a plugin declares its dependencies and the services it provides, and in `apply` uses services, provides services and registers cleanup; configuration comes from rutis.
- The same plugin definition can either be `serve`d as a binary on the Mac or handed to an app to host (Swift SDK §2).
- Between plugins: calls inside the same app get the object itself; across connections or languages they go by name through rutis.
- rutis has only two jobs: plugin discovery and connection. Device ID, model, OS version and the like are provided by plugins (for example a `device` plugin); rows that need them `inject` it.

## 4. The SDK's surface for the app

| Primitive | Semantics |
| --- | --- |
| `start()` | Connect to rutis and report the carried plugins; from then on rutis drives plugin loading; reconnect automatically after disconnection until `stop()` |
| `stop()` | Clean up all plugins in reverse order, then disconnect; `start()` may be called again |

The SDK guarantees:

- Whether it is `stop`, a network drop or the system killing the process, rutis sees the same thing: the instance closes, services are withdrawn, consumers stop.
- After reconnecting, plugins are loaded again according to the current rutis configuration, independent of any state from the previous connection.
- Data a plugin needs to persist is handled by the plugin itself with platform storage APIs; the SDK provides no storage contract.
- Connection state is observable, for the business layer to display; the SDK makes no lifecycle decisions based on it.

The SDK does not do: foreground/background handling, background keep-alive, push wake-up, battery-optimization prompts, permission reconciliation.

## 5. Loading and naming

- Plugins the app carries: `plugins` in the `mount` reply; each entry is the same as a `rows.schema` reply (plugin name, version, `inject`, `provides` and method shapes, configuration schema), with no device information.
- Loading reuses rutis's existing rows: the configuration has a row that points to a plugin in the app and gives its configuration; when the app connects and carries that plugin, the row loads in that connection's instance; if the app does not carry it, the row fails in that instance with "this connection does not contain plugin X", without affecting other instances. For how to write the configuration, see the dial-out design §4.
- App disconnects: the instance closes, those rows stop, and their consumers stop with them; after reconnecting they load again.
- Same-named plugins over several connections = instances ([creating plugin instances on demand](design-loader-instance-mounts-and-services-2026-10-07.en.md), [service names inside instances](design-instance-services-2026-10-08.en.md)): service names inside an instance resolve within that instance, and consumers in the same instance only see this connection's services; rows outside the instance cannot use an instance's services.
- Same-named services outside instances (global) still follow the existing rule: the later registration is rejected.

## 6. Connection

Everything follows [Remote runtimes: dialing out and dynamic connections](design-remote-runtime-dial-2026-10-14.en.md):

- The app dials out by IP or server name to a rutis access point; non-loopback must use `wss://` (dial-out design §3).
- One instance per connection (dial-out design §4).
- Connections are told apart by an id the app reports itself; during the transition a shared token is used; identity, pairing and revocation belong to a future management service (dial-out design §5).
- Reconnection, takeover by the same id, and closing the instance on disconnect follow dial-out design §6; Swift / Kotlin pass the same conformance tests.

## 7. Platform reference (business layer, not an SDK obligation)

| Platform | Background capability | Common business-layer practice |
| --- | --- | --- |
| macOS apps | Can stay resident | Login item, menu bar app, always on |
| iOS / iPadOS | Suspended soon after going to the background; only a few categories may run in the background | `start` in the foreground, `stop` in the background; when needed, wake with a push and `start` briefly |
| Android | A foreground service can stay resident (with a persistent notification; from Android 14 its type must be declared); vendor systems may kill the process | When residency is needed, run a foreground service and guide the user to the battery-optimization allowlist; otherwise follow the UI |
| watchOS / visionOS | More restricted / same as iOS | Not covered for now |

Integration notes (the iOS local network permission, usage descriptions for each capability, Android foreground service types, and so on) go into each platform's SDK guide.

## 8. Implementation: native

The Swift and Kotlin SDKs each implement the session protocol, the runtime contract and dialing out in native code, as Python, Node, Bun and Go do; no embedded Rust.

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
