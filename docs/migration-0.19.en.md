# Migrating to 0.19

[中文](migration-0.19.md)

0.19 is for host owners. Only the changes since 0.18 are written here. Removed forms are removed outright, not deprecated, with no conversion or hint. The incompatible changes, by kind, each with the section saying how to change:

- **Protocol 4**: 0.18 and older hosts, runtimes, nodes and SDKs are refused at the handshake; upgrade them to 0.19 together ([Upgrade together](#upgrade-together-695)).
- **How remote configuration is written**: `runtimes.remote`, `rutis-bridge/peer` rows and `peer:` rows are removed, written as the connection rows `rutis-host/dial` and `rutis-host/listen` instead ([Remote runtimes and devices](#remote-runtimes-and-devices-695-696-698)); the host side's tokens go by the connection row's id ([Tokens on the host side](#tokens-on-the-host-side-695)).
- **Nodes**: node rows lose `host: true`, `export` and `import`, and giving out a token allows the other side to run plugins on this node; a Cordis application as a node (`@arcships/rutis-runtime/bridge`) is removed ([Nodes](#nodes)).
- **Old names at the SDKs' roots removed outright**: a name moved to advanced is not found at its old place ([The SDKs' default entry points](#the-sdks-default-entry-points-692)).
- **The service name `loader` is taken**: a row providing `loader` itself fails ([The service name `loader`](#the-service-name-loader-185)).
- **Runtime rows and `isolate`**: an isolate sees only the providers in it, as with Rust rows ([Runtime rows and `isolate`](#runtime-rows-and-isolate-896)).
- **Every language's test tool goes through `rutis-host test`**: the plugin runs on a real host, and `scope` and other options are removed ([TypeScript and Bun](#typescript-and-bun-plugins), [Python](#python), [Go](#go-plugins), [Swift](#swift), [Kotlin](#kotlin)).
- **Python 3.12 at least** ([Python](#python)).
- **Go and Swift row names keep only the language prefix**: `go-<binary>:<plugin>` becomes `go:<plugin>` ([Go plugins](#go-plugins)).
- **The Kotlin SDK is not published to Maven**: it is used from source ([Kotlin](#kotlin)).
- **Rust APIs removed and renamed**: the connection and session parts ([Rust embeddings](#rust-embeddings)), the platform cleanup ([Platform cleanup](#platform-cleanup-694)), and a filter failing in a runtime's `emit` reported as `FilterFailed` ([Error kinds](#error-kinds-879)).
- **Tools**: `rutis-dev` is removed, `rutis-migrate` is no longer released, `rutis-host go add` is removed ([Tools](#tools)).

The rest is new, and changes nothing already written.

## Remote runtimes and devices (#695, #696, #698)

Design: [Remote connections: one session plugin](design-remote-session-2026-10-28.en.md).

### The new form: connection rows

Remote runtimes and devices are written as connection rows: `rutis-host/dial` (rutis dials) or `rutis-host/listen` (the far end connects in). `runtimes.remote` is removed, not deprecated and not converted (rutis-host ships two connection plugins: dial and listen); a rutis.json that has it is refused by the 0.18 rules as an unknown field, with no hint of its own:

```
rutis-host: rutis.json: unknown field `remote`, expected one of …
```

Local runtimes (`runtimes.node` / `py` / `go` / `swift` / `bun`) are written as before.

```json
// 0.18
"runtimes": { "remote": [{ "name": "gpu", "language": "python" }] },
"rows": [
  { "id": "gpu", "name": "rutis-bridge/peer", "config": { "peer": "gpu", "dial": "wss://gpu.example.com:7443/rutis", "runtime": "gpu" } },
  { "id": "embedder", "name": "gpu:embedder" }
]

// 0.19
"rows": [
  { "id": "gpu", "name": "rutis-host/dial", "config": { "address": "wss://gpu.example.com:7443/rutis" } },
  { "id": "embedder", "name": "gpu:embedder" }
]
```

| 0.18 | 0.19 |
| --- | --- |
| A `runtimes.remote` entry + a `rutis-bridge/peer` row with `runtime`, `dial` | One row `{ "id": "gpu", "name": "rutis-host/dial", "config": { "address": "<address>" } }` |
| The same, `listen` | One row `rutis-host/listen`, `"listener": "<listener>"` |
| `listen` + `instances` on a `runtimes.remote` entry, and the rows of its `instanced` group | One row `rutis-host/listen`; the group's remote-plugin rows go, the app installing its own plugins instead (see [Apps](#apps-kotlin-swift) below); rows once per device on the host (`py:coach`) change as that section says |
| `language` of `runtimes.remote` | Not written: plugin names are resolved on the far end |

- The row id names the connection: the runtime's `--id` is it, the token is `RUTIS_TOKEN_<row id>` (see [Tokens on the host side](#tokens-on-the-host-side-695)), and output lines start with it.
- A plugin run on the far end is still the row `<connection row>:<plugin>` (`gpu:embedder`), its config in that row. Such rows go with `rutis-host/dial`: a runtime that connected in under 0.18 with rutis choosing its plugins now listens, and rutis dials it.
- On `rutis-host/listen`, a newer connection with an id replaces the older: the older's remote plugins all unload first, then the older is closed with close code 4002, and only then are the newer's plugins registered. When the host stops, connections close with close code 1000.
- The host no longer creates an instance per connection: the plugins a far end brings register as `<far end's id>:<plugin>`, their services visible everywhere; same names follow the kernel's rule, the later one refused.

### Nodes

Nodes connect with connection rows too: `rutis-host/dial` on one end, `rutis-host/listen` on the other ([connect nodes](guide/nodes.en.md)). `rutis-bridge/peer` rows and `peer:` rows are removed, not deprecated and not converted; `rutis-bridge/peer` is a plugin name 0.19 does not know, refused by the 0.18 rules with no hint of its own:

```
office (rutis-bridge/peer): no plugin named "rutis-bridge/peer"
```

A 0.18 row `peer:<peer>/<plugin>` is not refused in 0.19: it shows `waiting for peer`, as it is read as a row on a connection row named `peer`, and there is no such row. Rewrite it as `<connection row>:<plugin>` (`peer:office/scanner` becomes `office:scanner`, `office` being the id of the row connecting to the far end).

```json
// 0.18: main's side
{ "id": "office", "name": "rutis-bridge/peer", "config": { "peer": "office", "dial": "wss://office.example.com/rutis", "export": ["weather"], "import": ["clock"], "rows": true } },
{ "id": "scanner", "name": "peer:office/scanner", "config": { "dpi": 300 } }

// 0.19: main's side
{ "id": "office", "name": "rutis-host/dial", "config": { "address": "wss://office.example.com/rutis" } },
{ "id": "scanner", "name": "office:scanner", "config": { "dpi": 300 } }

// 0.19: office's side (in 0.18, a rutis-bridge/peer row with listen, and host: true)
{ "id": "main", "name": "rutis-host/listen", "config": { "listener": "public" } }
```

| 0.18 | 0.19 |
| --- | --- |
| `dial` of a `rutis-bridge/peer` node row | One `rutis-host/dial` row, `"address": "<address>"` |
| `listen` of a `rutis-bridge/peer` node row | One `rutis-host/listen` row, `"listener": "<listener>"` |
| `export`, `import` (services and events, `*` prefixes included) | Not written: services are seen on both sides, events are one bus |
| `host: true` + `rows: true` + the row `peer:office/scanner` | The row `office:scanner` (nothing to open on the far end) |
| `peer` | Not written: the far end's id comes from the handshake (its rutis.json's `id`) |
| `require`, `declare`, `transport`, `identity` | Not written; your own way of connecting and its credentials are a connection plugin, registered through `Embedding.builtins` |
| `@arcships/rutis-runtime/bridge` (a Cordis application as a node: `Link`, `Export`, `Import`, `Host`, `Events`) | Removed: a Cordis application joins rutis by running its plugins in the Node runtime (`<package name>` rows) |

- Tokens go by the connection row's id: `main`'s row `office` presents `RUTIS_TOKEN_OFFICE`, `office`'s row `main` checks `RUTIS_TOKEN_MAIN` (it was the node row's `peer`, usually the same name).
- One name on both sides: an `import` of a name already provided here used to be refused; now the kernel's rule refuses the one registered later, and that remote plugin fails: `office:weather: weather is provided here by row weather: not registered from office`.
- **Giving out a token allows the other side to run, on this node, any plugin this node can resolve** (the far end used to need `host: true`; who may connect is decided by the connection row's token, and a connected node is trusted). Connected nodes see each other's shareable services: connect only to trusted nodes; use `isolate` on the row providing a service the far end should not see.
- A service a node gets over one connection reaches a third node over another (services used to go one hop).
- `run`'s output: `office: link connected` / `link down (…)` / `link refused: …` / `link stopped: …` become `office: connected` / `office: disconnected (…); retrying in …` / `office: disconnected (incompatible: …); stopped`; the listening side writes `main: connected main`, `main: disconnected main (…)`.

### What behaves differently

1. **A remote runtime missing a plugin fails only that row.** When the far end has no plugin for a row, that row used to fail to resolve (`unresolved: no plugin named "gpu:embedder"`) and an edit adding such a row was refused; now only that row fails, the other rows go on, adding such a row is not refused, and the far end is checked again when it next connects:

   ```
   embedder: unresolved: entry "embedder" rejected: runtime gpu: this connection does not contain plugin embedder
   ```

   Rust code embedding rutis that matched `LoaderError::NotFound` for this case uses `rutis_loader::report::Unavailable::is(&error)` instead.

2. **A runtime without the row contract is told without a version:** `the gpu runtime lacks rows.v2 and hosts: a runtime speaking the row contract is required`.

3. **Output.** A connection's state lines start with the connection row's id, in two words only, `connected` and `disconnected`, the reason in parentheses (formerly `runtime <name>: link …` and `<row id>: link …`):

   ```
   gpu: connected
   gpu: disconnected (gpu.example.com:7443: Connection refused); retrying in 2.1s
   gpu: disconnected (auth-rejected: gpu.example.com:7443: upgrade refused with 403 Forbidden); retrying in 30s
   gpu: disconnected (incompatible: this endpoint speaks rutis.3, the dialer offered rutis.4: upgrade this endpoint (PyPI rutis)); stopped
   phone: connected phone-1
   phone: disconnected phone-1 (connection lost)
   phone: disconnected phone-1 (replaced by a new connection)
   ```

   A connection whose handshake fails on a listen row (a wrong token, protocols that do not match) prints nothing; the dialing end says why. The `connections:` part of `rutis-host status` has a line per connection row: a dial row `connected` / `disconnected`, a listen row the ids connected (`none connected` when none is):

   ```
   connections:
     gpu: connected
     phone: kim, lee
   ```

   Scripts reading `run` that matched `runtime <name>: link` or `link connected` match `<row id>: connected` / `disconnected` instead.

4. **Persisted rutis.json files.** Files of 0.8–0.18 with `runtimes.remote` are not read by 0.19 (the unknown field above); files with `rutis-bridge/peer` node rows are read, but those rows fail as an unknown plugin name (see [Nodes](#nodes)). These are exceptions to rutis.json staying readable across versions, `format` still 1.

### Apps (Kotlin, Swift)

- The app installs the plugins it carries itself, their configurations in `RutisApp(configs: …)` (without one, the plugin's schema defaults), and once connected announces the services they provide, which the host registers as `<app id>:<plugin>`. On the rutis side the `<runtime>:<plugin>` rows in an `instanced` group go; one row `rutis-host/listen` (the app dials out) or `rutis-host/dial` (the app listens) stays.
- A plugin in the app that cannot be installed (its config not matching the schema, `apply` failing, a service name rutis already has) is written only in the device's log in 0.19; the host prints nothing.
- Rows once per device on the host (`py:coach` in an `instanced` group) are not supported any more. Instead put it on the device (a plugin in the app); or have the device's plugin provide its service as `health-<device>` and a plugin on the host listen to the `rutis/service` event, calling the service `loader` on `available: true` for `health-<device>`: `loader.create({id: "coach-<device>", name: "py:coach-one", config: {device}, inject: ["health-<device>"]})`, and `loader.remove("coach-<device>")` on `available: false`, in the event handler, not in `apply`. Full examples: [Swift apps](guide/swift-app.en.md#once-per-device), [Kotlin apps](guide/kotlin-app.en.md).

### Rust embeddings

- `rutis_host::control::serve` takes one more argument after the `Loader`: a `Report` (a function returning the status text, which `status` is answered with); `Host::status_report()` gives it.
- Listed here are the connection and session parts of rutis-bridge and rutis-loader (removals in the rutis kernel and elsewhere are in [Platform cleanup](#platform-cleanup-694)). These changed, and a local language runtime is now the far end of a session, as a remote one is (#695, [Remote connections: one session plugin](design-remote-session-2026-10-28.en.md) §3, §6, §8, §13 S4):

  | 0.18 | 0.19 |
  | --- | --- |
  | `rutis_bridge::transport::{local, memory, websocket}` | `rutis_bridge::{pipe, memory, websocket}` |
  | `Transport`, `Dial`, `transport_key`; `LocalPlugin`, `MemoryPlugin`, `WebSocketPlugin` and their `*Transport`; `websocket::Config` | `websocket::dial`, `websocket::listen`, `pipe::spawn` and `memory::pair` give a `Channel` directly |
  | `LinkPlugin` + `PeerPlugin { runtime }` + `RuntimePlugin::remote`; `RuntimeSession`, `runtime_session_key`, `RuntimeAccessPlugin` | In a connection plugin, `ctx.plugin(Session::new(websocket::dial(...)?))` (or `Session::handshake(&ctx, channel).await`, then `ctx.plugin(greeted)`), rows `<name>:<plugin>` |
  | `AccessPointPlugin`, `AccessPointConfig`, `AccessEvent`, `Accept`, `Accepted`, `Release`; the loader's `RuntimeInstances`, `AcceptInstances`, `RuntimeConnection` | `rutis-host/listen`, or a connection plugin of your own |
  | `rutis_bridge::runtime::LocalRuntime` (`node`, `python`, `go`, `swift`, `bun`, `launcher`, `named`, `env`, `interpreter`, `python_path`, `stdin` / `stdout` / `stderr`, `greeting`, `handle`, `processes`) | `rutis_host::connect::SpawnPlugin::new(name, launcher, anchor)`, the plugin `rutis-host/spawn` (dependency `rutis-host`): `launcher` is `rutis_bridge::runtime::Launcher::node(&package)`, `::python`, `::go`, `::swift`, `::bun` or `Launcher::new`, `name` is what `named` gave (`node`, `py`, … by default); `.interpreter`, `.python_path`, `.env`, `.stdio(stdin, stdout, stderr)`, `.greeting(limit)`, `.handle()`, `.processes()`; `output_prefix` is on `Launcher`. Mount it with `root.plugin(spawn)`; the root provides the id sessions greet with: `root.provide(rutis_bridge::LocalId(PeerId::new("main")?))` |
  | `RuntimePlugin` (`session`, `named`, `host`, `starting`, `stopped`, `failed`, `handle`), the service `runtime::Runtime` (`Runtime::key`, `host_methods`) | Removed: a runtime is the far end of a session. Its state is kept by `rutis_bridge::runtime::RuntimeStates` (`new(name, anchor)`, `starting`, `ready`, `failed`, `stopped`, `announce(ctx)` for the `rutis/runtime` events) and read through its `RuntimeHandle` (`handle()`); `ended_status(&process)` says how its process ended |
  | `RuntimePlugin::host(name, methods)`, `LocalRuntime::host` | The host service gives its methods with `HostDispatch::methods()` |
  | `rutis_loader::runtime::RuntimeResolver::node(handle)` / `::modules(handle)`, `with_catalog`; `invalidate`, `invalidate_all`, rutis-host's `Host::invalidate`; `runtime::resolve_entry` | One `rutis_loader::runtime::SessionResolver` for every runtime, last in the chain (`Chain::new().with(builtins).with_shared(sessions.clone())`): `SessionResolver::npm(&catalog, "node")` (also npm names, subpaths, `file:` URLs and absolute paths, on the row `node`) or `SessionResolver::new(&catalog)` (rows `<row>:<plugin>`, `py:shout`); `.runtime(handle)` for each local runtime (`Loader::status` lists it, its waiting rows say `waiting for runtime py (starting)`); `.attach(&loader)` before mounting the loader. Rust no longer resolves npm names nor caches them by `package.json`: the Node runtime does, at every resolution; after a plugin code change, just `Loader::reload(row)` |
  | `RuntimeRows` (`RuntimeRows::key`), `RowsSource` | `SessionRows`, `SessionRows::key(row)` |
  | `RuntimeRowsPlugin::new(resolver)` | `SessionResolver::follow_row(name)` once the loader runs; after a runtime restarts, its rows are resolved again by themselves |
  | `BinaryLanguage::launch` returning a `LocalRuntime`; `BinaryRuntimes::new(resolver, project)` | `launch` returns a `Launcher`; `BinaryRuntimes::new(resolver, project, spawn)`, `spawn` a `rutis_loader::runtime::binary::Spawn` (`Arc<dyn Fn(&str, Launcher, &Path) -> Spawned>`, `Spawned { plugin, handle, processes }`); rutis-host passes one making a `SpawnPlugin` |
  | `LocalRuntime::go(binary, project)` with `RuntimeResolver::modules` (one fixed binary) | `SpawnPlugin::new(name, Launcher::go(&binary, &project), &project)`, rows `<name>:<plugin>` through `SessionResolver` |
  | `Admission`, `AdmissionPlugin`, `admission_key`, `SharedToken`, `Rejection` | The connection plugin's own authentication; the id comes from the handshake |
  | `ListenerConfig::new(name, address, local id)` | `ListenerConfig::new(name, address)` |
  | `Closer::replaced()` / `rejected(r)` / `close(&str)` | `close(Close::Replaced)` / `close(Close::Rejected(r))` / `close(Close::Done)` |
  | `Channel`'s `info` (`ChannelInfo`), `Spawn::new(program, peer)` | removed; the far end's id comes only from the session handshake; `Spawn::new(program)` |
  | `rutis_host::host::Embedding`'s `transports` | removed; a connection of your own, with its credentials, becomes a connection plugin, registered through `Embedding.builtins` |
  | `Connection::open(channel, dispatch)`, `Connection::open_with(channel, dispatch, Format::Endpoint(e))`, `Format` | `Connection::open(channel, dispatch, e)`: only the endpoint format, this side always names itself |
  | `Process::attach(channel, mount, Format::Endpoint(e))` | `Process::attach(channel, mount, e)` |
  | `session::PROTOCOL` (2), `cordis::PROTOCOL`; `session::ENDPOINT_PROTOCOL` at 3 | removed; `session::ENDPOINT_PROTOCOL` and `cordis::ENDPOINT_PROTOCOL` are 4 |
  | `LinkPlugin`, `LinkConfig` (and its `compat`), `LinkState`, `Connect`, `Dialer`, `Failure`, `Retry`; `pipe::dial`, `memory::dialer`, `websocket::admit_registered`, `protocol()` | removed: dialing and redialing are `rutis-host/dial`'s, or your own connection plugin's; the subprotocol is `format!("rutis.{}", session::ENDPOINT_PROTOCOL)` |
  | `PeerPlugin`, `PeerHandle`, `Features`; `Peer`, `peer_key`, `Handler`, `Offered`, `Offers`, `family()` | removed: a node is a `Session` too, the far end's plugins `<id>:<plugin>` |
  | `ExportPlugin`, `ImportPlugin`, `HostPlugin`, `LinkEventsPlugin`, `ServiceKeys` | not needed: services are seen on both sides, events are one bus, the far end runs plugins through `run` |
  | `Identity`, `IdentityPlugin`, `identity_key`, `StaticIdentity`, `Presented`; `Registration`, `Registered`, `RegistrationError`, `Registrations`, `Accepting`, `Ticket`, `Refusal`, `Deliver` | the connection plugin's own authentication; ids come from the handshake. `fingerprint` stays |
  | Revocation while running: `Revocations`, `Revocable`, `Identity::revocations()`, `Admission::revocations()` (#834) | removed: to cut existing connections, stop the row |
  | `rutis_bridge::Credential` | `rutis_bridge::websocket::Credential` (what `websocket::dial` presents) |
  | `Installed { described: Described { schema, version, integrity }, factory }`; `StaticCatalog::with(name, described, factory)` | `Installed { schema, version, factory }`; `StaticCatalog::with(name, Installed::new(factory))` |
  | rutis-loader's feature `peer` and `rutis_loader::peer` (`PeerResolver`, `PeerRows`, `PeerRowsPlugin`, `register_peer_node`, `node_schema`, `LoaderCatalog`) | removed; rows `<connection row>:<plugin>` are resolved by `runtime::SessionResolver`, `LoaderCatalog` is in `rutis_loader::runtime` |
  | `identity` of `rutis_host::host::Embedding` | removed |
  | a connection of your own implementing `Transport` | hand over a `Channel`: `ctx.plugin(Session::new(channel))`, or `Session::handshake(&ctx, channel).await` then `ctx.plugin(greeted)`; other languages use `open` of the service `sessions` |

- A Rust embedding that mounts `Session` itself provides, in the root ctx, the id this end gives in the handshake: `root.provide(rutis_bridge::LocalId(id))`; without it the session fails: `the host provides no id for its sessions`. rutis-host provides it (rutis.json's `id`, `main` by default).

- A connection of your own: hand over a `Channel`: write a connection plugin that calls `Session::new(channel)`; other languages use `sessions.open`.

## Upgrade together (#695)

0.19 refuses 0.18 and older hosts, runtimes, nodes and SDKs at the handshake: the session protocol goes from version 3 to 4 and keeps only the endpoint format ([Remote connections: one session plugin](design-remote-session-2026-10-28.en.md) §3). The refusal says which end to upgrade:

- A 0.19 host starting a 0.18 local runtime (which speaks the local protocol 2) prints:

  ```
  incompatible session: the far end speaks protocol 2, this side 4: upgrade the far end
  ```

- A 0.18 node or remote runtime dialing a 0.19 listener prints, on the dialer:

  ```
  this endpoint speaks rutis.4, the dialer offered rutis.3: upgrade the dialer
  ```

- 0.19 dialing a 0.18 listener is refused in the listener's words, for example a 0.18 remote Python runtime:

  ```
  this endpoint speaks rutis.3, the dialer offered rutis.4: upgrade this endpoint (PyPI rutis)
  ```

- A 0.18 host starting a 0.19 local runtime (passing no `--id`): the runtime stops before the handshake:

  ```
  a local channel needs --id <endpoint>: fd:3; a host older than rutis 0.19 started this runtime: upgrade the host
  ```

Upgrade the host and every language runtime and SDK (Node, Bun, Python, Go, Swift, Kotlin) to 0.19 together; rebuild Go and Swift binaries with the 0.19 SDK. Meetings across versions hold only between adjacent releases without incompatible changes.

If you start runtime processes yourself, or implement a runtime:

- Local channels (`fd:3`, a socket path, `tcp:`) take `--id <name>` too: `<channel> --id <name> [--peer <id>] <project>`. The host starts a local runtime with `--id <runtime name>`, and the runtime greets as that name. Without `--id` the runtime refuses to start: `a local channel needs --id <endpoint>: <channel>; a host older than rutis 0.19 started this runtime: upgrade the host`.
- The `--format` argument of the Node and Bun runtimes is removed; `rutisProtocol` in their `package.json` is 4.
- The WebSocket subprotocol is `rutis.4`.
- Go: `advanced.ServeConn(conn, defs...)` becomes `advanced.ServeConn(conn, id, defs...)`; `id` is the name it greets as, as `--id`.

## Tokens on the host side (#695)

**Tokens**: the host side reads only `RUTIS_TOKEN_<NAME>`; a host that set only `RUTIS_TOKEN` sets it by name instead. `RUTIS_TOKEN` on the runtime side is unchanged.

`<NAME>` is the id of a connection row (`rutis-host/dial`, `rutis-host/listen`), in upper case, `-` as `_`:

```bash
# 0.18: one RUTIS_TOKEN for every peer
RUTIS_TOKEN=secret rutis-host run rutis.json
# 0.19: one for each connection row
RUTIS_TOKEN_OFFICE=secret RUTIS_TOKEN_GPU=secret rutis-host run rutis.json
```

What you see when it is not changed:

- A `rutis-host/listen` row without a token fails:

  ```
  phone: set RUTIS_TOKEN_PHONE: the token the far ends present when they connect
  ```

- A `rutis-host/dial` row presents no token, the far end refuses it, and `run` retries every 30 s: `office: disconnected (auth-rejected: …); retrying in 30s`.

Two entries on one listener whose `RUTIS_TOKEN_<NAME>` variables have the same value still fail before starting (#702); the text now names both variables:

```
rutis-host: listener public: row main accepts RUTIS_TOKEN_MAIN and row phone accepts RUTIS_TOKEN_PHONE, the same token, so a connection presenting it cannot be told apart: give RUTIS_TOKEN_MAIN or RUTIS_TOKEN_PHONE another value
```

## The SDKs' default entry points (#692)

From 0.19, the root of each language's SDK keeps only the names plugins are commonly written with; the rest is in advanced: TypeScript / Bun `@arcships/rutis/advanced`, Python `rutis.advanced`, Go `github.com/arcships/rutis/go/rutis/advanced`, Swift `Rutis.Advanced`, Kotlin `arcships.rutis.advanced`. The names only the code Swift macros and Kotlin KSP generate uses are in `Rutis._Generated` and `arcships.rutis.generated`.

**Moved directly, no old names.** The old locations are removed in 0.19: using an old name fails to compile or import, saying the name does not exist; change the import as the table shows. A Python plugin shares the host environment's one `rutis` with the runtime, so it upgrades to 0.19 together with the host.

Only imports change: a name moved off the root (value types, `PLUGIN_API`, the types that describe events, the errors only one way of emitting raises, and so on) is imported from advanced under the same name; the full list is in [SDK entry points](design-sdk-entry-convergence-2026-10-27.en.md) §3. The few names added to the SDKs since follow the same rule: Python's `LoadError` is in `rutis.advanced`; Kotlin's `RemoteObject` in `arcships.rutis.advanced` and `ObjectProxy` in `arcships.rutis.generated`; Swift's `Rutis.located` in `Rutis._Generated`.

| Language | Old form | New form |
| --- | --- | --- |
| TypeScript / Bun | `import { datetime } from '@arcships/rutis'` | `import { datetime } from '@arcships/rutis/advanced'` |
| Python | `from rutis import AggregateError` | `from rutis.advanced import AggregateError` |
| Go | `rutis.DateOf(t)` | Import `github.com/arcships/rutis/go/rutis/advanced` and write `advanced.DateOf(t)` |
| Swift | `let d: Rutis.DateTime` | `let d: Rutis.Advanced.DateTime` |
| Kotlin | `RutisFormat.encode(…)` | `import arcships.rutis.advanced.RutisFormat` |

Two more points for Kotlin:

- Use the same version of `rutis-ksp` as of the SDK: the code a `rutis-ksp` before 0.19 generated refers to the old places at the root and does not compile against the 0.19 SDK.
- These classes changed packages. A Kotlin plugin library published on its own, already compiled, has to be recompiled with the 0.19 SDK before it goes into an application with the 0.19 SDK.

Not affected by the move: `definePlugin` / `define_plugin` / `Define` / `Rutis.define` at each language's root, every method on `ctx`, the testing tools, Rust. The code Swift macros and Kotlin KSP generate is updated with the SDK and needs no change. The plugin API version is unchanged: plugins built with an older SDK run on the 0.19 runtimes as before.

## The service name `loader` (#185)

A host using `LoaderPlugin` (rutis-host included) now provides a service named `loader` ([Managing plugins with `loader`](guide/plugin-api.en.md#managing-plugins-with-loader)); a row that itself provides a service named `loader` now fails with the existing error for a name already provided (`service loader is already provided by …: row <row> cannot provide it too`): rename its service.

## Runtime rows and `isolate` (#896)

A runtime's row (a Node, Bun, Python, Go, Swift or Kotlin plugin) is isolated by its `isolate` on the rutis side too, as a Rust row is:

- For a service name in its `isolate` it sees only the providers in the isolate (the rows with the same label, or with `true` the row alone), not those outside it, the host's services included. In 0.18 such a row was not affected by `isolate` on the rutis side, and a row in an isolate could use a service of that name provided outside it (by the host, say); in 0.19 it waits for a provider in the isolate. To use the service from outside, take the name out of the row's `isolate`.
- A service it provides under a name in its `isolate` is in the isolate only, not published everywhere.
- With no provider in a private isolate (`true`), the row shows in `run`'s output as waiting for the service (0.18 showed `running` while it waited in the runtime).

## Platform cleanup (#694)

0.19 removes what does not belong in the platform ([platform cleanup](design-platform-cleanup-2026-10-28.en.md)). There is no compatibility: a removed Rust API is a compile error.

| 0.18 | 0.19 |
| --- | --- |
| `Loader::set_overlay`, `overlays()`, `LoaderChanged::Overlay` | Removed; to override a row, write it in a layer of your own (the layer list given to `reconcile`) |
| The error name on the wire `CordisError`, the generated type `CordisErrorThrown` | `Error`, `ErrorThrown` |
| The generated type `ErrorThrown` where it meant an error type other than rutis's named `Error` (`io::Error`, `anyhow::Error` and so on) | `<module name>ErrorThrown` (`IoErrorThrown`, `AnyhowErrorThrown`): `ErrorThrown` is rutis's own error's only; the `name` it is thrown with is unchanged, still `Error` (#771 X4) |
| `ServiceCatalog::share_by_name()`, `share_registered_only()` | delete the line (sharing by name is the default) |
| `Loader::waiting_for(&entry)` | `Loader::waits(&entry)` (the note on an unmarked Rust service is `WaitReason::Explained`) |
| `Loader::locate(plugin)` | `Loader::row(instance)` |
| `Loader::schema_of(name)` | `loader.resolve(name).await?.schema` |
| `rutis_loader::runtime::go::{GoBinaries, GoManifest, GoBinary, GoResolver, GoRuntimes, GoRuntimesHandle, GoRuntimeState, GoRuntimeInfo}` and the 8 Swift ones of the same names | `rutis_loader::runtime::binary::{Binaries, BinaryManifest, BinaryFile, BinaryResolver<Go>, BinaryRuntimes<Go>, BinaryRuntimesHandle<Go>, BinaryRuntimeState, BinaryRuntimeInfo}`, with `Go` and `Swift` in `runtime::go` and `runtime::swift` |
| `ctx.plugin_from(\|cfg\| …)` | Implement `rutis::host::PluginFactory` and call `ctx.plugin_with(factory, config)` |
| `rutis_loader::status::RowState` (the type of `status::Row::state`) | `rutis_loader::report::EntryStatus`: waiting is `Running` with the `Pending` phase, starting is the `Loading` phase; what `Display` prints is unchanged |
| `rutis_loader::report::RowState` (the values of the `rutis/row` event) | Renamed `rutis_loader::report::RowEventState`; the values are unchanged |
| `rutis_loader::internal::*` | `rutis_loader::patch::{apply_patches, Composed, ComposedRow, Owner, PatchWarning}`, `rutis_loader::runtime::is_runtime_name`, `rutis_loader::runtime::binary::read_binary_manifest`; the plugin API version is `Go::PLUGIN_API`, `Swift::PLUGIN_API` (constants of `BinaryLanguage`) |
| `take_cleanup_errors()` | A cleanup's error is in the unload result (the error `dispose` returns) |
| `FiberView::effects()`, `EffectMeta`, `EffectPhase` | No replacement; for diagnostics use `diagnostics()` |
| `EventBus::observe_dispatch`, `internal::DispatchAttempt` | No replacement; to observe, register an ordinary listener |
| The host's `events.elsewhere` to runtime sessions; `rutis_bridge::session::Dispatch::arrived` / `replied`; `rutis_bridge::events::testing::Tap::dispatches` / `answered` (#813) | Removed: runtimes dispatch nothing in their own process, every emission goes to the host (`events.dispatch`), so there is nothing to tell them or to count; a runtime of your own deletes its `events.elsewhere` handler |
| `internal::EventPattern` (prefix subscriptions) | No replacement |
| `try_filter`, `try_filter_named` of `internal::CtxInternal` | `EventBus::dispatch` / `dispatch_sync` with `Dispatch { filter: Some(BatchFilter), … }` (`rutis::internal`) |
| feature `anyhow`, `Error::from_anyhow`, `From<Error> for anyhow::Error` | `.map_err(\|e\| Error::msg(format!("{e:#}")))` |
| `rutis_bridge::cordis::build::rutis_plugin`, `cordis::server` | Removed (a Cordis host mounting Rust plugins is no longer supported) |
| `waterfall_sync`; the event protocol's mode `waterfall-sync` (for those implementing a runtime) | Use `waterfall` (asynchronous) or the synchronous `bail` |
| `internal::intercept_require_as`, `intercept_set_as`, `ServiceIntercept`, the `Intercept*` variants of `ServiceReadFailure` / `ServiceWriteFailure` | No replacement; `internal::CtxInternal::require_provided_as` no longer takes an `actor` argument |
| `rutis_bridge::build::Exports`, build-dependency `rutis-bridge = { features = ["exports"] }` | `rutis_types::Exports`, build-dependency `rutis-types`; API unchanged |
| `internal::config::{closest, decode, …}` | `rutis::config::closest`; validation `rutis::config::check` |
| `rutis_loader::config::check(schema, config, options)` | `rutis::config::check(schema, config)` |
| `internal::provide_checked`, `refresh`, `service::Check`, `Exposure`'s `check`, `DependencyStatus::CheckPending`, `CheckRejected`, `CheckPanicked` | No replacement |
| `rutis_bridge::session::Connection::independent_future` | No replacement |
| `rutis_loader::backoff`, `runtime::binary::{RestartPolicy, RuntimeEnd}`, `BinaryRuntimesHandle::supervise` / `unsupervise` | Removed: the loader no longer restarts a crashed Go or Swift runtime. Call `BinaryRuntimesHandle::restart(name)` yourself; the new `runtime::binary::refused(why)` tells a binary whose handshake was refused, which a restart would not start either. With rutis-host, set `runtimes.<key>.restart` |
| `get_key`, `require_key`, `provide_key` of `internal::CtxInternal` (keyed by `TypeKey`) | Hold a `rutis::host::Key<T>` and use `get_at`, `require_at`, `provide_at` of `CtxHost`. Along with it these take a `Key`: the `key` argument of `ServiceCatalog::register_keyed` and `readable_keyed`; `SessionRows::key(row)` returns `Key<SessionRows>`; the `key` argument of `Projection::service_keyed` and `service_keyed_occupied`; the `key` function of `row_projection_with` and `row_projection_of` returns `Key<dyn HostDispatch>` (pass `host_key` itself). Where a `TypeKey` is needed (`DynPlugin::injects`), use `key.erase()` |

### Error kinds (#879)

When a filter fails in an `emit` a runtime makes through the host, the host's error receiver gets `ErrorKind::FilterFailed { name, emitter, error }` (`emitter` is `row <row>` or `the mount`), as for the same failure in a Rust plugin; 0.18 gave `ErrorKind::PluginFailed(<text>)`. Rust code that told this failure by `PluginFailed` tells it by `FilterFailed` instead. The printed text is unchanged: `filter of t/fe from row chat failed: FErr: no`.

### Tools

- `rutis-dev` (the dev channel) is removed; use `rutis-host dev` during development.
- `rutis-migrate` is no longer released with 0.19. The 0.17 migration still works with `cargo install rutis-migrate --version 0.18.0`.
- `rutis-host go add <module>@<version>` is removed; use `rutis-host add <module>@<version>`, which installs into `runtimes.go.dir` with the same `go install` and adds the row. For the binary without a row, run `GOBIN=<runtimes.go.dir> go install <module>@<version>` yourself.

### Go plugins

Rebuild them with SDK 0.18 or later (the `required` exception for binaries built before 0.18 is removed).

The row names `go-<binary>:<plugin>` and `swift-<binary>:<plugin>` are removed; write `go:<plugin>` and `swift:<plugin>`. A `rutis.json` still using them is refused when read, with the name to write. When two binaries have a plugin of the same name the row fails naming both files, and the plugins' author renames one.

- `rutistest` runs the plugin under the real kernel: it needs `rutis-host` on `PATH`, or its path in `RUTIS_HOST` (the template's release workflow installs it before `go test`). `Load`, `TryLoad` and `Loaded` keep their shape; `TryLoad`'s error is now the host's line for the row (`weather: config: city is required`). The test tool no longer has scopes: `InScope`, `InChain` and `Loaded.Scope` are removed, and so is `RuntimeName` (the host names the runtime). Calls between the test and the plugin are calls between two rows of one Go runtime. A service given to two `Load`s whose rows are loaded at once is one provider: give the same value.
- The runtime no longer checks a row's config against the schema or fills its defaults: the host does, and sends the filled config; the runtime only decodes it (`config: <path> is not a value the plugin accepts` when it does not decode into the config type).
- Every emission goes through the host, also when every listener is in the same binary. `ctx.Emit` (and the other modes) after the session ended, in a cleanup run then, fails with `the host disconnected` instead of reaching the listeners of the same binary.

### TypeScript and Bun plugins

`@arcships/rutis/testing` runs the plugin on a real host (`rutis-host test`; see [plugin API: the TypeScript and Bun test tool](guide/plugin-api.en.md#the-typescript-and-bun-test-tool)):

- Add `@arcships/rutis-runtime` (`@arcships/rutis-bun` under Bun) and `@arcships/rutis-host` as dev dependencies (a project `rutis-host new` made has them), or set `RUTIS_HOST` to a `rutis-host`.
- The plugin must be the default export of its module: move a plugin made with `definePlugin` inside a test file into a file of its own, or `load` fails with `load needs a plugin that its module exports as default`.
- Removed options: `strict`, `scope`, `scopes`, `deadline`, `runtime`, and `h.at(scope)`: the test tool offers nothing a host cannot do, and scopes come from a row's `isolate`. `row` is now `id`; `h.load` also takes `isolate`, as in a row of `rutis.json`.
- `t.reported` and `h.reported` are methods: `await t.reported()`, each entry `{ line }`, `line` the line the host prints.
- Values between the test and the plugin cross by the rules between processes; the events `t.on` receives arrive through the host, a little after the emitter returns.
- The host checks the config and fills its defaults; a load that fails is a `PluginTestError` whose text is the row's status, such as `weather: config: city is required`.
- The runtimes no longer dispatch events in their own process: every emission goes through the host (a median of about 71 microseconds for `emit`; see the [events benchmark](performance-events-2026-10-28.en.md)).

### Python

The minimum is 3.12 (3.10 and 3.11 are no longer supported).

`rutis.testing` tests on a real host (#813 §7.5): `load` starts `rutis-host test` (the file `RUTIS_HOST` names, else `rutis-host` on `PATH`; the template's dev dependency `rutis-host` installs it), and the plugin runs on the Python runtime in the test's process.

| 0.18 | 0.19 |
| --- | --- |
| `load(..., strict=)`, `scope=`, `runtime=`, `deadline=` | Removed: values cross through the real runtime, and there are no options for scopes, the runtime's name or the unload wait (unloading waits `RUTIS_SHUTDOWN_TIMEOUT`); rows in scopes are a host configuration's |
| `load(..., row=)` | `load(..., id=)`; and `isolate=`, as a row's `isolate` in `rutis.json` |
| `Bus(scopes=)`, `bus.listeners(name)`, `bus.emitter(row=, strict=)` | `Bus()`, `bus.emitter()` (emit as the test's plugin) |
| `t.reported`, `bus.reported` (lists of `Reported(kind, name, row, error)`) | `await t.reported()`, `await bus.reported()`, `Reported(line)`: the line the host reported |
| A config that does not match makes `load` raise `BindingError` (`bad-config`) | `PluginTestError`, its text the row's state line the host writes (`weather: config: city is required`) |
| A service the test gives that the plugin provides too: `… is already provided by the host` | The runtime's words: `… is already provided by tester` |
| `rutis.config` (`check`, `configured`, …) | Removed: only the host's loader checks a config and fills its defaults; the plugin gets the filled config |

The runtime no longer dispatches events in its own process: every emission goes through the host, and the host reports a listener failing in `emit` too (`<row>: listener of <event> failed: …`). To see the listeners' results right after an `emit` in the same process, wait for one round trip through the host (`await ctx.parallel(...)`, say).

### Swift

- `RutisTest` runs the plugin as a row of a real host, `rutis-host test`, which it starts: put `rutis-host` on `PATH` (`npm install --global @arcships/rutis-host`) or set `RUTIS_HOST` to its path. One host serves the test process; `close()` unloads the test's rows.
- The test tool no longer has scope: `RutisTest.scope(_:groups:)`, the `scope:` parameters and `Loaded.scope` are removed, and so is `runtimeName:`. `row:` is the row's id (by default the plugin's name); `isolate:` is the row's `isolate`, as in rutis.json.
- `loaded.reported` is read with `await`. A config the host refuses fails `load` with the host's line of the row: `weather: config: city is required`.
- The test's services, `on` and emits are a plugin in the plugin's runtime: values between the test and the plugin no longer cross processes.
- On a device or in the iOS simulator `RutisTest` is an app installing its own plugins, connected to a host on the development machine (`RUTIS_TEST_CONNECT`, `RUTIS_TEST_TOKEN`; [Swift app guide](guide/swift-app.en.md)); `Loaded.load` and `update` are macOS only there.
- The Swift SDK no longer validates configs: the host checks a row's config and fills its defaults before the runtime gets it. An app's own plugins are decoded only: with `@RutisConfig`, a missing key takes the literal initial value, and a missing required key or a value of another type fails with `config: <key> is required` or `config: <key> should be <type>, got <value>`.
- Every event of a Swift plugin is dispatched by the host; the listeners and emits of an app's own plugins are the connection's.

### Kotlin

- The Kotlin SDK is not published to Maven (0.18 was `implementation("io.github.arcships:rutis:0.18.0")`): fetch this repository's tag `v0.19.0`, bring it in with `includeBuild("../rutis/kotlin/rutis")`, and write the dependencies without a version (`implementation("io.github.arcships:rutis")`, `ksp("io.github.arcships:rutis-ksp")`); see [kotlin-app](guide/kotlin-app.en.md#1-dependencies).
- The testing tool is an artifact of its own, `io.github.arcships:rutis-testing` (jvm): `testImplementation("io.github.arcships:rutis-testing")`, with `kspTest("io.github.arcships:rutis-ksp")` (the SDK is used from source, with no version: see [kotlin-app](guide/kotlin-app.en.md#1-dependencies)). It runs the plugin's real runtime in the test process, as a row of `rutis-host test`, and needs `rutis-host` (on `PATH` or at `RUTIS_HOST`).
- The testing tool no longer has scope: `RutisTest.load(scope:)`, `Loaded.load(scope:)` and `RutisTest.scope(...)` are removed; so are `RutisTest.load(runtime:)` and `Loaded.provided`. Without `row`, rutis gives the row an id from the plugin's name (no longer `test`); a load that fails fails with the row's line (`weather: config: city is required`); `loaded.reported()` is the lines rutis reported (`weather: listener of x failed: …`).
- The test's services, `loaded.on`, `emit` and so on are a plugin of the same runtime: values do not cross between processes.
- Tests on a device no longer use `RutisTest`: they install their own plugins with `RutisApp`, dialing a rutis on the development machine.
- Every event is dispatched by the host: a synchronous `bail` on the Android main thread is always refused (it was allowed when this process had listeners).
- Config is validated and its defaults filled in rutis alone; the SDK only decodes. The plugins an application installs itself with `RutisApp` are not checked by rutis: a key left out takes the constructor default, and a required key missing or a value of the wrong type fails the load with `config: …` naming the key (`config: city is required`).
