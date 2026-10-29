---
name: rutis-remote-plugin
description: Use when a rutis plugin must run on another machine, device or app instead of next to the host — "remote plugin", running a Python/Node/Go/Swift plugin on a GPU box or a Mac, connecting two rutis hosts, exposing a phone app's capabilities to rutis. Walks through choosing a node link vs a remote runtime vs app hosting, writing the plugin in the rutis paradigm, deploying, configuring rutis.json, verifying, and the rules that keep it inside the rutis design. Self-contained; reference material is in references/.
---

# Adding a remote plugin to rutis

rutis (https://github.com/arcships/rutis) is a plugin host with a Rust core that takes plugins in several languages. This skill does not depend on the rutis repository; everything it refers to is in `references/`:

| File | Contents |
| --- | --- |
| `references/plugin-api.md` | The plugin API: declarations, ctx, events, lifecycle, how values cross, sync calls and reentrancy, cancellation, versions |
| `references/nodes.md` | Links, running plugins on another node, remote language runtimes, security |
| `references/rutis-json.md` | `rutis.json`: `runtimes` (`restart`, `listen` / `instances`), `listen`, `rows`; exit status, live reload, when a runtime ends, credentials, deployment |

Keep one sentence in mind: **rutis has no such thing as a "remote plugin" — only an ordinary plugin plus a runtime or node that runs somewhere else.** The plugin code is exactly what it would be locally; what changes is where its runtime runs and how rutis reaches it. Do not write a separate plugin API, protocol or lifecycle for remote use.

## 1. Decide which kind it is

| What the other side is | Use | Row on the rutis side |
| --- | --- | --- |
| A language runtime on another machine; **this rutis** loads, configures, starts and stops its plugins | Remote runtime (link + `runtime`) | `<runtime name>:<plugin>` |
| Another full rutis host that manages its own plugins and only shares services and events | Node link (`export` / `import`) | No new row; imported services are available by name |
| Another full rutis host on which this side wants to run rows it writes | Node link (`host: true` there, `rows: true` here) | `peer:<peer>/<plugin>` |
| A phone or desktop app offering its own capabilities as plugins | App hosting (Swift apps; Android designed) | `<runtime name>:<plugin>`, one instance per connection |

When unsure, ask one question: **who decides whether the plugin is loaded and with what configuration?** This rutis → remote runtime / app hosting. The other side → node link.

## 2. What works today (2026-10-19; check the repository before relying on it)

| Language | Remote listening `listen:` (rutis dials in) | Dialing out `dial:` (runtime connects to rutis) |
| --- | --- | --- |
| Python | ✅ | ✅ ([#383](https://github.com/arcships/rutis/issues/383)) |
| Node / TypeScript | ✅ | ✅ ([#384](https://github.com/arcships/rutis/issues/384)) |
| Go | ✅ | ✅ ([#384](https://github.com/arcships/rutis/issues/384)) |
| Bun | ✅ (#384) | ✅ (#384) |
| Swift (macOS plugin binaries) | ✅ ([#387](https://github.com/arcships/rutis/issues/387)) | ✅ ([#387](https://github.com/arcships/rutis/issues/387)) |
| Swift apps (plugins hosted in an iOS / iPadOS / macOS app, `RutisApp`) | ✅ ([#389](https://github.com/arcships/rutis/issues/389)) | ✅ ([#389](https://github.com/arcships/rutis/issues/389)) |
| Kotlin apps (plugins hosted in an Android app) | Designed, not implemented ([#391](https://github.com/arcships/rutis/issues/391)) | Same |

"One access point accepting many connections, one instance per connection" (many devices, autoscaled workers) works on the rutis side ([#382](https://github.com/arcships/rutis/issues/382): `listen` and `instances` on a `runtimes.remote` entry); of the language runtimes Python, Go, Swift, Node and Bun can connect to it so far (step 4b). Tracking issue: [#392](https://github.com/arcships/rutis/issues/392).

## 3. Write the plugin: the paradigm, same as local

Details in `references/plugin-api.md`. The essentials:

- Write a leaf plugin with the language's SDK: declare `inject` (service names it uses) and a config schema; in `apply(ctx, config)` use `ctx.use` / `ctx.provide` / `ctx.effect`. What `ctx.provide` provides reaches rutis by itself (its public methods, all `async`); `provides` is optional, to mark methods `sync` and let `rutis-host check` know before running what the plugin provides. The current plugin API version is 3 (rutis 0.13: events with `ctx.on` and the five ways to emit, `emit` / `parallel` / `serial` / `waterfall` / `bail`, plus `filter`).
- A service of the host application's Rust code is used the same way, `ctx.use(name)`, once the host marks its type `#[rutis::service]` and provides it (from rutis 0.15; no binding to write on either side). An unmarked Rust service is not reachable: a row waiting for it is reported with the fix by the host (`Loader::waits`).
- The plugin does **not** decide when it runs, does not reconnect, does not probe whether the other side is up: it is applied when all its services are ready, stops when any is withdrawn, and starts again when it returns. A remote disconnect is, to the plugin, an ordinary stop.
- Values follow the cross-process rules: JSON-compatible data is copied; dates and times, binary data, maps, sets, big integers, decimals and UUIDs are copied and arrive as each language's own types (integers beyond ±(2^53−1) cross as big integers automatically); functions and objects go by reference; nesting at most 64 levels. Never assume the same process.
- Do not hold locks while calling other services: your service may be called again in the middle of your own synchronous call. Prefer `async` methods for calls that cross runtimes and may cross each other.
- Events are **one logical bus** across processes, languages and machines: `ctx.on` registers with the host by itself, and an event reaches every listener in the emitter's scope or above it, wherever that listener runs, with the same result as in one process. Nothing to declare for a remote runtime; between nodes, list the event names (or `prefix/*`) in the link's `export` on the emitting side and `import` on the listening side (one hop). `emit` never waits and a failing listener is only reported; use `parallel` to wait, `serial` / `waterfall` / `bail` for results. When the far end goes away its listeners leave the bus and calls in flight fail once (`BindingError`). Runtimes and nodes older than 0.19 cannot join a current host (refused by protocol version); upgrade every end together.
- Dependency declarations contain only service names, never languages, machines or addresses.

## 4. Deploy a remote runtime (listening, as of today)

On the remote machine:

1. **Install** the plugin there: Python `pip install` into the interpreter the runtime uses; Node `npm install` in the project; Bun `bun add` in the project; Go, build the binary; Swift (macOS), build the binary with `swift build` (its Info.plist linked in when the plugin uses calendars, contacts and the like: macOS asks the user on that machine, for that binary). A remote runtime loads only plugins already deployed there — **there is no code upload**.
2. Prepare TLS: non-loopback addresses must be `wss://`. Point `RUTIS_CERT` / `RUTIS_KEY` at the certificate and key; with a self-signed certificate, give the issuing CA to `RUTIS_CA` on the rutis side.
3. Set the token: `RUTIS_TOKEN` on the remote side, equal to `RUTIS_TOKEN_<RUNTIME NAME>` on the rutis side.
4. Start it listening. `--id` is this runtime's name, `--peer` is the name of the rutis that controls it:

   ```bash
   # Python
   RUTIS_TOKEN=… RUTIS_CERT=server.pem RUTIS_KEY=server.key \
     python -m rutis listen:wss://0.0.0.0:7443/rutis --id gpu --peer main /srv/plugins

   # Node (inside that machine's Node project)
   node node_modules/@arcships/rutis-runtime/src/runner.mjs listen:wss://0.0.0.0:7443/rutis --id gpu --peer main ./package.json

   # Bun (inside that machine's Bun project; rows `<runtime>:<module>`, language `bun`)
   bun --no-install --no-env-file node_modules/@arcships/rutis-bun/src/main.ts listen:wss://0.0.0.0:7443/rutis --id gpu --peer main .
   ```

   A Go or Swift plugin binary accepts `listen:wss://…` as its channel the same way.

   Every remote runtime (Python, Node, Bun, a Go or Swift binary) reports on stderr the address it actually bound: `rutis: listening on <address>`. With port 0 (`listen:ws://127.0.0.1:0/rutis`) the system picks a free port; a script or test reads it from this line instead of picking a port first and passing it on (another process can take that port before the runtime binds it). `rutis-host run` does the same for its listeners: `rutis-host: listening on <name> <address>`.

   The Python runtime also reports on stderr: when a session ends, `rutis: session ended (<reason>); waiting for a controller` (reason such as `heartbeat timeout`, `connection lost`, `closed (1001): …`), or `rutis: session ended (replaced by a new connection)` when a new connection takes over.

## 4b. Or let the runtime connect in (dialing out)

When the remote machines are behind NAT, change address, or come and go in numbers, the runtime connects to rutis instead. On the remote machine, with the plugins installed:

```bash
# Python
RUTIS_TOKEN=… RUTIS_CA=ca.pem \
  python -m rutis dial:wss://main.example.com:7443/rutis --id worker-1 --peer main /srv/plugins

# Node (inside that machine's Node project)
RUTIS_TOKEN=… RUTIS_CA=ca.pem \
  node node_modules/@arcships/rutis-runtime/src/runner.mjs dial:wss://main.example.com:7443/rutis --id worker-1 --peer main ./package.json

# Bun (inside that machine's Bun project)
RUTIS_TOKEN=… RUTIS_CA=ca.pem \
  bun --no-install --no-env-file node_modules/@arcships/rutis-bun/src/main.ts dial:wss://main.example.com:7443/rutis --id worker-1 --peer main .

# Go or Swift: the plugin binary takes the same channel, flags and environment
RUTIS_TOKEN=… RUTIS_CA=ca.pem \
  ./weather dial:wss://main.example.com:7443/rutis --id worker-1 --peer main .
```

- `--id` is this connection's own id (`[a-z0-9-]+`, unique per process: two processes with one id keep taking over from each other); `--peer` is the rutis `id`. `RUTIS_CA` is needed only for a self-signed certificate on the rutis side.
- It reconnects by itself: backoff 0.5 s doubling to 30 s; refused token or id every 30 s; an incompatible protocol stops it (exit status 1). Each disconnect unloads its rows; each reconnection is a new session. SIGTERM / Ctrl-C stops it for good. It reports on stderr: `rutis: connected to …`, `rutis: connection lost (…); retrying in …`, `rutis: refused: auth-rejected (…)`, `rutis: refused: incompatible (…); stopping`.
- The rutis side: a `listen` entry, and on the `runtimes.remote` entry `listen` plus `instances` naming an `instanced` group whose rows are `<name>:<plugin>`; no link row. The token is `RUTIS_TOKEN_<NAME>` there:

  ```json
  {
    "id": "main",
    "listen": [{ "name": "public", "address": "0.0.0.0:7443", "cert": "server.pem", "key": "server.key" }],
    "runtimes": { "remote": [{ "name": "worker", "language": "python", "listen": "public", "instances": "worker" }] },
    "rows": [{ "id": "worker", "group": true, "instanced": true, "config": [{ "id": "embedder", "name": "worker:embedder" }] }]
  }
  ```

  Every connection is one instance of the group: its rows run on it, and the services they provide are seen only in that instance.

## 4c. Or host the plugins in an app (Swift: iOS, iPadOS, macOS)

The plugins are compiled into the app (the Swift package `https://github.com/arcships/rutis`, product `Rutis`, from `0.15.0`); the app only starts and stops the connection, and rutis loads the plugins its configuration names. The rutis side is the same as 4b, with `"language": "swift"`.

```swift
let rutis = RutisApp(plugins: [health, device], connect: "wss://main.example.com:7443/rutis",
                     id: "kim-iphone", token: token)   // optional: peer:, trust: [SecCertificate], shutdownTimeout:, log:
try await rutis.start()   // returns once dialing started; reconnects by the 4b rules until stop()
await rutis.stop()        // unloads every plugin (latest first), then disconnects; start() may follow
// rutis.state: .stopped(reason:) / .connecting / .connected — Observable, and rutis.states (AsyncStream)
```

- When to start and stop is the app's business (on iOS, typically start in the foreground and stop on entering the background); a suspended app is, to rutis, a lost connection: its instance closes and its users stop.
- Permissions (HealthKit, location, contacts) belong to the app: entitlements and usage descriptions in the app; a denial is an error the plugin throws, which reaches the caller.
- Connecting to a local-network address asks the user for the local network permission (`NSLocalNetworkUsageDescription` in Info.plist); until allowed, the app keeps retrying.
- A device at a fixed address can listen instead: `RutisApp(plugins:, listen: "wss://0.0.0.0:7443/rutis", id:, token:, certificate: <SecIdentity>)`, linked to from rutis like any remote runtime (step 5).
- Guide: https://github.com/arcships/rutis/blob/main/docs/guide/swift-app.en.md

## 4d. Or bring your own connection

When the far end is reached by a way of connecting rutis does not ship (a relay, Bluetooth), write a connection plugin that hands the connection over as a session: in Rust `ctx.plugin(rutis_bridge::Session::new(channel))` (the host provides `rutis_bridge::LocalId` in the root ctx; rutis-host does), in other languages `sessions.open({ send, close })` with `inject: ['sessions']`. Details and a full example: [references/nodes.md](references/nodes.md#your-own-connection). The far end's plugins are then `<far id>:<plugin>` here; rows `<connection row>:<plugin>` run plugins there.

## 5. Configure the rutis side

Details in `references/rutis-json.md` and `references/nodes.md`.

```json
{
  "runtimes": { "remote": [{ "name": "gpu", "language": "python", "dial": "wss://gpu.example.com:7443/rutis" }] },
  "rows": [
    { "id": "embedder", "name": "gpu:embedder", "config": { "model": "small" } }
  ]
}
```

- `language`: `python` / `py` / `go` / `swift` / `node` / `bun`.
- One entry describes the runtime whole: `dial: <address>` (rutis dials it), `listen: <listener>` (it dials in; its `--id` is the runtime's name), or `listen` + `instances` (many, step 4b). Not `dial` with `listen`, nor `dial` with `instances`. The connection is over WebSocket.
- The form before 0.19 (only `name` and `language`, plus a `rutis-bridge/peer` row with `"runtime": "<name>"`) still works; do not write both for one runtime (the host refuses to start).
- Rows are named `<runtime name>:<plugin>`; the plugin is resolved on the remote machine.

## 6. Verify

- `rutis-host check`: remote runtimes are not contacted during checking; services that might come from them are reported as such, not as failures.
- `rutis-host run`: expect `runtime <name>: link connected` (`<link row id>: link connected` for the old link row; `runtime <name>: connected <id>` per connection with `instances`), then `<row id>: running`. `rutis-host status` lists them under `connections:`.
- `link refused: auth-rejected` means the token or certificate is wrong; `link stopped: incompatible` means the protocol or address form is wrong, or the far end is not a runtime. A row failing with `runtime <name>: this connection does not contain plugin <plugin>` names a plugin the remote side does not have.
- Stop the remote process: those rows stop, and so do the rows using their services; when it comes back rutis reconnects and loads them again.

## 7. What not to do

| Don't | Because |
| --- | --- |
| Implement reconnects, heartbeats or offline caches in the plugin to "keep the remote available" | Connections and lifecycle belong to rutis; a plugin only sees whether services are there |
| Upload code to a remote runtime or load plugins by path | A remote runtime loads only deployed plugins |
| Use `ws://` to a non-loopback address | Non-loopback needs TLS; the rutis dialer refuses it outright |
| Model "devices" or "identities" in the protocol or the plugin | Device information is a service some plugin provides; identity belongs to a future management service |
| Alias same-named services to tell machines apart | Same-named plugins on different connections are told apart by instances (#382) |
| Write a separate plugin API for remote use, or make an SDK depend on Rust | The runtime side is implemented entirely in its platform language |
| Handle phone foreground/background or keep-alive in an SDK | That is the app's business logic; the SDK offers only `start` / `stop` |

## Further reading

- Remote plugins design: https://github.com/arcships/rutis/blob/main/docs/design-remote-plugins-2026-10-03.en.md
- Remote runtimes — dialing out and dynamic connections: https://github.com/arcships/rutis/blob/main/docs/design-remote-runtime-dial-2026-10-14.en.md
- App integration: https://github.com/arcships/rutis/blob/main/docs/design-app-integration-2026-10-14.en.md
