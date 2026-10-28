# Remote connections: one session plugin

[中文](design-remote-session-2026-10-28.md)

Status: final, not implemented. Date: 2026-10-28. Base: `main` `3d19634f`. Scheduled: 0.19.
Issue: [#695](https://github.com/arcships/rutis/issues/695) (implemented by [#696](https://github.com/arcships/rutis/issues/696) loader, [#698](https://github.com/arcships/rutis/issues/698) rutis-host). Basis: the two 2026-10-28 comments on #695.
Relation to earlier designs: replaces the implementation part of [Remote runtime consistency](design-remote-runtime-consistency-2026-10-26.en.md) (the "inside rutis-host" column of §4, §5, §7, §8); its "a missing plugin is always `Unavailable`" stays, and the `runtimes.remote` syntax goes (§6). The [decision record](decision-remote-runtime-consistency-2026-10-26.en.md)'s "no new handover interface, do not rewrite links and access points" is overturned by this design; see §1 for why.

Out of scope: the session protocol's frames (unchanged; only the compat format goes, §3); language runtimes and SDKs change only where §3 and §13 say; opening up authentication (a host's own credentials belong in its own connection plugin, with no interface in the core); revocation while running (see `Revocations` in §8); the loader's row model and `instanced` groups themselves.

## 1. Today

### 1.1 Four lifecycle paths

One job — bringing the plugins at the far end of a connection into the local kernel — has four implementations, each managing its own connection, admission, replacement and state:

| # | Case | Connection | Handed to plugin management as | Plugin management |
| --- | --- | --- | --- | --- |
| L1 | Local runtime | `LocalRuntime`: the local transport starts the process, a `LinkPlugin` carries the session | held directly in process | `RuntimePlugin` + each language's `RuntimeResolver` |
| L2 | One remote runtime | `LinkPlugin` (dial or listen) → `PeerPlugin { runtime }` → `RuntimeAccessPlugin` | service `RuntimeSession#<name>` | `RuntimePlugin::remote` + `RuntimeResolver` |
| L3 | Many runtimes dialling in | `AccessPointPlugin` (a `Registration` on a `Transport`, `Admission`, one slot per id) | callback `Accept` / `Accepted` / `Release` | `RuntimeInstances`, `AcceptInstances`, instance value `RuntimeConnection` |
| L4 | Node links | `PeerPlugin` (`LinkPlugin` + `ExportPlugin` + `ImportPlugin` + `LinkEventsPlugin` + `HostPlugin`) | service `Peer#<id>` | the loader's `PeerRows` / `PeerResolver` (`peer:<peer>/<plugin>` rows) |

### 1.2 Two handover shapes

- **Service shape** (L2, L4): the connection side provides `RuntimeSession#<name>` or `Peer#<id>`; plugin management `inject`s it.
- **Callback shape** (L3): the access point hands each connection to an `Accept` passed in from outside and gets a `Release` back.

### 1.3 What is written more than once

| Concern | Copies | Where |
| --- | --- | --- |
| Admission | 3 | `Identity::accept` in the WebSocket upgrade; `Admission::admit` after the access point's hello; `Revocable` wrapping both |
| Replacement | 3 | generations of `Registration` on a listening link; the access point's per-id slot; the runtime side's lease (that one is at the far end and stays) |
| State | 3 | `LinkState`, `AccessEvent`, `RuntimeState` |
| Choosing a carrier | 3 | the `Transport#<kind>` service, the `transport` field of link rows and `runtimes.remote`, `Embedding.transports` |
| Discovery and checks | 1 copy, 2 call sites | #768 merged them (`discover.rs`, `Process::check_rows`); L2 and L3 still call them separately |

### 1.4 The structure today

```text
rutis-host
 ├─ Transport#local, Transport#websocket, Transport#<kind> from Embedding.transports
 ├─ Identity#host (or Embedding.identity)              ← compares tokens at upgrade
 ├─ L1 each local runtime: RuntimePlugin ─ LocalRuntime ─ LinkPlugin ─ child process
 ├─ L2 each single remote runtime: PeerPlugin { runtime }
 │     ├─ LinkPlugin (dial / listen, Registration)      ← LinkState
 │     └─ RuntimeAccessPlugin ── provides RuntimeSession#<name>
 │   RuntimePlugin::remote ── injects RuntimeSession#<name> ← RuntimeState
 ├─ L3 each many-instance remote runtime: AdmissionPlugin (SharedToken)
 │     AccessPointPlugin (Registration, slots)          ← AccessEvent
 │       └─ Accept = RuntimeInstances::accept ── one instance per connection (RuntimeConnection)
 └─ L4 each link row: PeerPlugin
       ├─ LinkPlugin ─ ExportPlugin ─ ImportPlugin ─ LinkEventsPlugin ─ HostPlugin
       └─ PeerRowsPlugin ── peer:<peer>/<plugin> rows
```

The previous decision record held that the layers were already separated with two handover points, so it only added consistency. Reviewing 0.19 showed that the two handover points are themselves a duplicate (one job, two shapes), and admission, replacement and state are each written three times: the layering did not remove code, it spread the duplicates over different files.


## 2. The new architecture

Principle: keep only what the protocol itself needs and what has users; nothing that copies an old feature's semantics and then adds rules. Defaults follow Cordis: services are visible everywhere, same-named services conflict by the kernel's rules, events are one flat bus, and `isolate` is for the host to use on its own rows.

### 2.1 Plugins

| Plugin | Belongs to | Does |
| --- | --- | --- |
| `rutis-bridge/session` | rutis-bridge; the only remote-related plugin in the core | brings the far end's plugins into the local kernel over one `Channel`; one per Channel |
| remote plugin `<id>:<plugin name>` | a child of the session | one plugin of the far end, as it looks here; the only name remote plugins have |
| connection plugin | outside the core; anyone can write one | gets a `Channel` and mounts a session on it |
| └ `rutis-host/spawn`, `rutis-host/dial`, `rutis-host/listen` | shipped in rutis-host | start a local process, dial, listen; used for local runtimes, remote runtimes, devices and nodes alike |

### 2.2 Structure

```text
connection plugin (rutis-host/dial, rutis-host/listen, rutis-host/spawn, or anyone's)
 └─ rutis-bridge/session             one per Channel
     ├─ <id>:weather                 a far plugin
     └─ <id>:llm
```

### 2.3 Relations

- **Connection plugin → session**: `ctx.plugin(Session::new(channel))`. The Channel closes, the session unloads; the session unloads, the Channel closes.
- **Session → remote plugins**: far plugins come from two sources, both registered as `<id>:<plugin>` and unloaded when they end or the session unloads:
  - this end has it run: a row `<connection row>:<plugin>`, for which the loader finds the session and calls `run`;
  - plugins the far end installs itself and that provide services (when the far end is a rutis node, or a device app installing its own plugins).
- **Services**: visible everywhere on both sides through the session. What a far plugin `inject`s is borrowed from this end (as local runtimes do today); what the far end provides is `provide`d here by its remote plugin. Same names follow the kernel's rule: the later registration is refused. Services that came over a session are not sent back over the same session.
- **Events**: one flat bus. The far end's listeners are entries on this end's bus, as runtimes' are today.
- **Remote plugins ↔ local plugins**: as for any plugin (`inject` / `provide`); every lifecycle is the kernel's own.
- **Replacement**: only inside `rutis-host/listen` (§4.2). The session core keeps no ids and replaces nothing.

### 2.4 API

Rust (connection plugins compiled into a host):

```rust
pub struct Channel { /* §3 */ }

impl Session {
    pub fn new(channel: Channel) -> Self;
}
impl SessionHandle {
    /// The id the far end gave in its handshake.
    pub fn id(&self) -> &str;
    /// Has the far end run `plugin` with `config`; it registers as `<id>:<plugin>`, and stops when the handle is dropped.
    pub fn run(&self, plugin: &str, config: Value) -> Result<RowHandle, Error>;
}
```

Any language (connection plugins loaded while running): `rutis-bridge/session` provides the service `sessions`, with one method:

```ts
type Close = { kind: 'done' } | { kind: 'replaced' } | { kind: 'rejected'; reason: string }
interface Far  { send(message: Uint8Array): void; close(why: Close): void }
interface Near { receive(message: Uint8Array): Promise<void>; end(why: Close): void }
sessions.open(far: Far): Near
```

- The functions in `far` and `near` cross languages by reference (0.18 #687).
- A session `open` creates is mounted under the plugin that called it: when that plugin unloads, all its sessions unload.

These four names are the session's whole public API. Why `run` stays: having the far end run a plugin is a step of the session protocol (`rows.v2`), and for a language runtime or an app it is the only way; for a rutis node far end it is used too, with no second way.

### 2.5 The structure afterwards

```text
rutis-host
 ├─ rutis-host/spawn  py        ── pipe::spawn --id py ─▶ Channel ─▶ session py
 │                                                                         └─ py:coach … (run)
 ├─ rutis-host/dial   gpu       ── websocket::dial ────▶ Channel ─▶ session gpu
 │                                                                         └─ gpu:embedder (run)
 ├─ rutis-host/listen phone     ── websocket::listen ──▶ one Channel per connection
 │     ├─ session phone-1   └─ phone-1:health (installed by the device)
 │     └─ session phone-2   └─ phone-2:health
 ├─ rutis-host/dial   office    ── websocket::dial ────▶ Channel ─▶ session office
 │                                                                         └─ office:clock (provided by office)
 └─ a TS plugin relay an agent wrote while running ── sessions.open(far) ──▶ session (id from the far end)
```

Local runtimes, remote runtimes, devices and nodes all have the same shape; there is no node- or remote-runtime-specific code.

### 2.6 Decisions on node links and configuration

**Decision 1: node links drop `export` and `import`.** Services are visible everywhere on both sides through the session, same names by the kernel's rule; events are a flat bus. This is how Cordis and today's runtimes work, and nodes need no code of their own. A host that does not want the far end to see some services uses `isolate` on its own rows; that is the host's business, not a rule of remote connections.

Every use in [Connecting nodes](guide/nodes.en.md) and the tests, checked:

| Use today | Where | Becomes |
| --- | --- | --- |
| `export` + `import` at the peer to share services | nodes.md; `node.rs`, `websocket_soak.rs`, `cordis_node.rs` | nothing to write: each side's services are visible on the other |
| `import` refused on a clash with a local service | nodes.md | the kernel's rule: the later registration is refused, and that remote plugin fails, saying why (§7) |
| `export` / `import` of events, and prefixes | nodes.md; `events_nodes.rs` | nothing to write: the far listeners are on this end's bus |
| Multi-hop: a imports b's `clock` and exports it to c | `multihop.rs`, `websocket_multihop.rs`, `fault_links.rs` | nothing to write: b's `clock` is visible everywhere on a and reaches c through a's session with c |
| `host: true` + `rows: true` + a row `peer:office/scanner` | nodes.md; `peer_rows.rs`, `node.rs` | a row `office:scanner`; the loader calls `run` |

**Decision 2: a plugin's configuration belongs to whoever starts it.**

- Plugins already running at the far end are configured there.
- When this end decides what the far end runs and with what configuration, it writes a row `<connection row>:<plugin>` with `config`, and the loader calls `run`.
- How apps get configuration today ([App integration](design-app-integration-2026-10-14.en.md) D2, D6; Kotlin and Swift `RutisApp`): the app only reports the plugins it carries (`plugins` in the `mount` reply); which one loads and with what configuration comes from the rows of an `instanced` group in the rutis configuration, and the SDK `apply`s only when a `rows.v2` start request arrives. The host no longer creates an instance per connection (decision 3), so this no longer works for many devices.
- Instead: the app on the device installs the plugins it carries itself, configured in the app (each plugin in `RutisApp` may carry a configuration; without one, the schema's defaults); once connected it reports the services they provide with the existing `services.announce` frame (which nodes already use), and the host registers them as `<id>:<plugin>`. This is a 0.19 change to the Kotlin and Swift SDKs (§13 A1, A2); D2 and D6 of the app integration design are rewritten along.
- Runtimes the host connects one at a time (the language runtimes behind `rutis-host/spawn`, `dial` and one-runtime `listen`) still get rows `<connection row>:<plugin>` written by the host, through `run`.

**Decision 3: telling devices apart is the business of the plugins on the devices.**

- `rutis-host/listen` creates just one session per connection it lets through, and the far plugins register as ordinary plugins `<id>:<plugin>`; the services they provide are visible everywhere, with same names conflicting by the kernel's rule (the later one is refused, saying why). The host creates no instance per connection.
- Logic that must run once per device goes on the device; a plugin on the host that handles many devices tells them apart itself, or the plugins on the devices use different service names. The platform does no isolation for plugins.
- The loader's existing `instanced` groups stay a general feature; remote connections no longer use them specially.
- 0.18's "one `py:coach` on the host per device" (the example in the Swift and Kotlin app guides) is not supported in 0.19. The ways to change it: move coach onto the device (a plugin in the app), or have coach tell devices apart itself (for example, the device plugins provide their service as `health-<device>`, and coach looks it up by name).

## 3. Channel

```rust
pub struct Channel {
    pub sender: Box<dyn Sender>,
    pub receiver: Box<dyn Receiver>,
    pub closer: Arc<dyn Closer>,
}

pub trait Sender: Send {
    fn send(&mut self, message: &[u8]) -> Result<(), ChannelError>;   // blocks under backpressure
    fn max_message(&self) -> Option<usize> { None }
}
pub trait Receiver: Send {
    fn recv(&mut self) -> Result<Option<Vec<u8>>, ChannelError>;      // Ok(None): the far end finished normally
}
pub trait Closer: Send + Sync {
    fn close(&self, why: Close);                                      // idempotent; wakes send / recv
}

pub enum Close { Done, Replaced, Rejected(String) }
```

- `Sender` / `Receiver` / `Closer` stay split: sending and receiving each on one thread, closing from any. Per message, blocking, as `rpc.rs`'s `open(channel, dispatch)` uses it today.
- **Ids always come from the handshake**: the session protocol keeps only the endpoint format (protocol version 3), in which both sides give their own id in the handshake. The compat format goes: `Format::Compat` (`rpc.rs:1208`) and `COMPAT_CAPABILITIES` in `rpc.rs`, the `rust:` / `node:` prefixes of call ids, the `PROTOCOL = 2` local handshake, and `compat` and `name_far_end` at `link.rs:361`. `rutis-host/spawn` starts a local runtime with `--id <runtime name>`, and the runtime gives that name in its handshake. The controller a runtime expects (its `--peer`) is still checked by the existing handshake check (`Handshake::IdentityMismatch` at `rpc.rs:2482`).
- Each language runtime's local mode moves to the endpoint format (the current state of each was checked):

  | Runtime | Now | To change |
  | --- | --- | --- |
  | Node (`node/rutis-runtime/src/runner.mjs`, `session.mjs`) | compat locally by default, endpoint with `--format endpoint` (`runner.mjs:29`) | endpoint only: drop `--format`, require `--id` locally too; remove the compat branch and the `node:` prefix in `session.mjs` |
  | Bun (`bun/rutis-bun/src/main.ts`) | as Node: `--format endpoint` optional (`main.ts:107`, `:119`) | as Node |
  | Python (`python/rutis/rutis/__main__.py`, `peer.py`) | only compat locally (`__main__.py:14`); `--id` only for network channels | build an endpoint from `--id` for local channels too; remove the compat handshake in `peer.py` |
  | Go (`go/rutis/serve.go` and the `peer` package) | `parseArgs` gives an endpoint only to network channels (`serve.go:164`); `ServeConn` speaks compat (`serve.go:87`) | endpoint for local channels too; remove compat |
  | Swift (`swift/rutis/Sources/Rutis/API/Serve.swift`) | local `serve` speaks compat (`Serve.swift:58`); endpoint only for `listen:` / `dial:` | endpoint locally too; remove compat |
  | Kotlin (`internal/peer/Peer.kt`, `internal/Sessions.kt`, jvm `Serve.kt`) | `Peer.kt:33` has compat (version 2) and `compatOrigin`; `Sessions.kt:53` picks the format by whether there is an endpoint; Android apps only dial (already endpoint) | endpoint for the jvm's local channels; remove compat and `compatOrigin` |

- `ChannelInfo` goes: the far end's id comes from the handshake; `transport` and `label` were for diagnostics only, which the connection plugin's own output now gives.
- `Closer::close(&str)`, `replaced()` and `rejected(&str)` become `close(Close)`. The three kinds stay because 4002 and 4003 are close codes of the protocol, by which every language's dialler decides how to retry (`bridge/dial-out-rules`):

  | `Close` | WebSocket | local pipe (child process) | memory |
  | --- | --- | --- | --- |
  | `Done` | close code 1000 (1001 when the host stops) | closes the child's stdin | the other end's `recv` gets `Ok(None)` |
  | `Replaced` | close code 4002 | as `Done` | the other end's `recv` gets `Closed` |
  | `Rejected(r)` | close code 4003, reason `r` | as `Done` | the other end's `recv` gets `Rejected` |

- `ChannelError { Closed, Rejected }` stays: `Rejected` means the far end refused this end with 4003.
- `ConnectError` leaves the core and goes inside `rutis-host/dial`: its three categories (retryable, auth rejected, incompatible) and the reconnect numbers (0.5 → 30 s, auth rejected every 30 s, incompatible stops, reset after 60 s connected) are unchanged; they are a convention every language's dialler shares.
- Once given to `Session::new`, only the session closes the Channel; before that, the connection plugin may close it itself (a wrong token at upgrade, say).

## 4. Lifecycle

### 4.1 Channel and session

One to one, born and gone together, never reused.

| What happened | Who knows first | Result |
| --- | --- | --- |
| Handshake fails (protocol version, missing capability, 10 s timeout) | session | the far end first gets the protocol's own error frame, then `close(Done)`; the session fails with that reason |
| The far end leaves first (`recv` returns `Ok(None)` or an error) | session | the session unloads: remote plugins unload in reverse, calls in flight fail once |
| This end leaves first (connection plugin unloads, row removed, host stops) | kernel | the session unloads: remote plugins unload in reverse, then `close(Done)` |
| Reconnect | connection plugin | a new Channel, a new session; the old session is already gone |

A connection plugin waits for the session to unload through the kernel handle `ctx.plugin(...)` returns (`FiberView::watch()`), then decides whether to reconnect (dial backs off and reconnects; spawn follows the runtime's `restart`; listen does nothing). The session never reconnects.

### 4.2 Replacement

- `rutis-host/spawn` and `rutis-host/dial` have one Channel at a time: the new one starts only after the old one unloaded, so nothing is replaced.
- Replacement happens only inside `rutis-host/listen`. listen keeps its own connections by id; once a new connection with the same id finishes its handshake:
  1. the old session unloads (remote plugins in reverse, every cleanup finished);
  2. the old Channel is `close(Replaced)` (4002);
  3. the new session starts registering the far plugins.
- A new connection has no remote plugin before its handshake is done (remote plugins only come from `run` and from services the far end provides, both after step 3), so a connection whose handshake fails never affects the old one.
- TLA: the mutation `access-mut-accept-before-release` moves "accept the new connection" before "release the old one" and the model finds a counterexample; listen's order 1, 2 → 3 is the order it guards.
- The host keeps no names: spawn, dial and one-runtime listen are named by their row id (which the loader already requires to be unique); two entries with the same token on one listener are a start-up error (#702).

### 4.3 Remote plugins on the kernel's lifecycle

| Far end | The local remote plugin `<id>:<plugin>` |
| --- | --- |
| `run` succeeded, or a far plugin starts providing services | registered as a child of the session (for `run`, it is the row itself), its `apply` done; the services it provides appear as ordinary `provide`s |
| Running fails (the far end throws) | this plugin fails, with the far end's name, message, `code` and `site` (0.18 §4.2) |
| The far end ended it | it unloads; rows using its services stop or wait by the kernel's rules |
| The connection drops | the session unloads, and every remote plugin with it |

- There is no separate "connection state": connected means the session exists, disconnected means it does not. "Runtime state + whether the session is connected" replaces `LinkState`, `AccessEvent` and `RuntimeState`.
- A row running at the far end (`gpu:embedder`) waits while the far end is away ("waiting for gpu"); once a session appears the loader calls `run`. Before `run` it reads `plugins` in the `mount` reply, asking `rows.schema` when there are none (#768's shared discovery; every runtime reports this way in the protocol); a far end without the plugin makes the row `Unavailable`.
- `check_rows`: a far end lacking the `rows.v2` and `hosts` capabilities is refused (the row contract is part of the protocol); the error text drops the old version number.

## 5. Extending

There is one extension point: write a connection plugin, get a `Channel`, mount a session.

### 5.1 A Rust connection plugin

```rust
// Reach a runtime through a relay
async fn apply(ctx: &Ctx, config: Relay) -> Result<(), Error> {
    loop {
        let channel = relay::connect(&config.address, &config.token).await?;  // its own auth and carrier
        let session = ctx.plugin(Session::new(channel));
        disposed(session.watch()).await;                 // the kernel's FiberView::watch(): wait until the session unloads
        sleep(backoff.next()).await;
    }
}
```

### 5.2 A connection plugin in any language

```ts
export default definePlugin({
  name: 'relay',
  inject: ['sessions'],
  async apply(ctx, config: { url: string }) {
    const ws = new WebSocket(config.url)
    ws.binaryType = 'arraybuffer'
    await new Promise(ok => ws.addEventListener('open', ok, { once: true }))
    const near = ctx.sessions.open({
      send: m => ws.send(m),
      close: why => ws.close(why.kind === 'rejected' ? 4003 : why.kind === 'replaced' ? 4002 : 1000),
    })
    ws.onmessage = e => near.receive(new Uint8Array(e.data))
    ws.onclose = () => near.end({ kind: 'done' })
    ctx.effect(() => () => ws.close())
  },
})
```

- `receive` completes once the message is in the session's receive queue; when the queue is full (64 messages, as the memory channel) it waits, which carries backpressure back to the connection plugin.
- Underneath it is the same implementation as `Session::new`: `sessions.open` makes a `Sender` / `Closer` from `far` and a `Receiver` from `near`'s queue.
- Cost: every message of such a connection plugin passes once more through the runtime process it runs in.

### 5.3 Loading at run time

Connection plugins load as any plugin does: a row in rutis.json, the loader's `create`, the `loader` service of #185. A TS / Python connection plugin an agent writes while running is loaded this way, and the sessions `sessions.open` creates hang under it; unloading it unloads its sessions and every remote plugin below them.

## 6. The shipped connection plugins, and rutis.json

The three plugins live in rutis-host (not in rutis-bridge), on plain functions in rutis-bridge: `websocket::dial`, `websocket::listen`, `pipe::spawn` (and `memory::pair` for tests).

| Plugin | Does | Configuration |
| --- | --- | --- |
| `rutis-host/spawn` | `pipe::spawn` starts a local runtime process with `--id <runtime name>` and mounts a session; restarts it by `restart` when it exits | the existing fields of `runtimes.*` |
| `rutis-host/dial` | `websocket::dial`, mounts a session; after the session unloads, backs off by the `ConnectError` category and reconnects | `address`; the token is only `RUTIS_TOKEN_<row id>`; `RUTIS_CA` |
| `rutis-host/listen` | `websocket::listen`: compares the token at upgrade (401 / 403 unchanged); one session per connection let through, same-id replacement (§4.2) | `listener`; tokens as dial |

- The host side accepts only `RUTIS_TOKEN_<name>`, with no fallback to the shared `RUTIS_TOKEN` (the fallback at `config.rs:912` and the "who shares `RUTIS_TOKEN`" check from `config.rs:969` go); `RUTIS_TOKEN` on the runtime side is unchanged. One port dispatching to several entries by token (what `Registrations` does today) and `RUTIS_TOKEN_<name>` stay, because they have users (Swift and Kotlin apps and every language's dialler connect this way); the dispatching code lives in `rutis-host/listen`.
- `runtimes.node` / `py` / `go` / `swift` / `bun` and the other local runtimes are still shorthand for `rutis-host/spawn` rows, unchanged.
- `runtimes.remote` goes: remote runtimes are written the same way as nodes, as `rutis-host/dial` or `rutis-host/listen` rows. The `language` field is no longer needed: remote plugin names are resolved at the far end (the `mount` reply and `rows.schema`).
- All three plugins' configuration schemas have `additionalProperties: false` ([Config schema](design-config-schema-2026-10-27.en.md) C3).

```json
{
  "id": "main",
  "listen": [{ "name": "public", "address": "0.0.0.0:7443", "cert": "server.pem", "key": "server.key" }],
  "runtimes": { "py": { "project": "plugins" } },
  "rows": [
    { "id": "gpu", "name": "rutis-host/dial", "config": { "address": "wss://gpu.example.com:7443/rutis" } },
    { "id": "embedder", "name": "gpu:embedder" },
    { "id": "office", "name": "rutis-host/dial", "config": { "address": "wss://office.example.com/rutis" } },
    { "id": "scanner", "name": "office:scanner", "config": { "dpi": 300 } },
    { "id": "phone", "name": "rutis-host/listen", "config": { "listener": "public" } }
  ]
}
```

After the `phone` row, the plugins installed on each device that connects register as `<device id>:<plugin>` (`phone-1:health`); devices need not be listed in rutis.json.

## 7. What developers see

Following [Diagnostics](design-diagnostics-2026-10-26.en.md) §4.5: output never says session, `rutis-bridge/session` or internal ids; rows go by row name, remote plugins as `<id>:<plugin>`.

**Input**

- rutis.json: `runtimes.*` (local runtimes); `rutis-host/dial` and `rutis-host/listen` rows (remote runtimes, devices, nodes); rows running at the far end are `<connection row>:<plugin>`.
- Rust: `Session::new(channel)`, `SessionHandle::{id, run}`; `websocket::{dial, listen}`, `pipe::spawn`, `memory::pair`.
- Other languages: `open(far)` on the `sessions` service.

**What `run` prints**: prefixed with the connection row's id, using only the words connected / disconnected, with the reason in parentheses.

```text
gpu: connected
gpu: disconnected (gpu.example.com:7443: Connection refused); retrying in 2.1s
gpu: disconnected (auth-rejected: upgrade refused with 403 Forbidden); retrying in 30s
gpu: disconnected (incompatible: the far end speaks protocol 1, this side 3); stopped
phone: connected phone-1
phone: disconnected phone-1 (connection lost)
phone: disconnected phone-1 (replaced by a new connection)
office: connected
```

**`rutis-host status`**: a `connections:` section, one line per connection row, each connected one followed by the remote plugins it registered:

```text
connections:
  gpu: connected (gpu:embedder)
  phone: phone-1 (phone-1:health), phone-2 (phone-2:health)
  office: connected (office:clock, office:scanner)
```

**Errors**

| Case | Text |
| --- | --- |
| The far end lacks a row's plugin | `embedder: unresolved: entry "embedder" rejected: runtime gpu: this connection does not contain plugin embedder` (#768) |
| A far service has the name of a local one | `office:weather: weather is provided here by row weather: not registered from office` |
| The far end lacks the row contract | `the gpu runtime lacks rows.v2 and hosts: a runtime speaking the row contract is required` (without today's "for Node, @arcships/rutis-runtime 0.3.0 or later") |
| A 0.18 host or runtime | refused in the handshake by protocol version (`the far end speaks protocol 2, this side 3: upgrade the far end`), not retried |
| 0.18 remote syntax (`runtimes.remote`, `rutis-bridge/peer` rows and their fields) | no dedicated message: `runtimes.remote` is an unknown rutis-host configuration field and `rutis-bridge/peer` an unknown plugin name, reported by the 0.18 rules |
| The far end throws while running a plugin | the remote plugin `<id>:<plugin>` fails, printing the far end's error as in 0.18 §4.2 |

## 8. What is removed or made internal

The old forms are removed in 0.19, not deprecated, with no dedicated conversion or message.

| Name | Where | Why | Who is affected |
| --- | --- | --- | --- |
| `Transport`, `Dial`, `transport_key` | rutis-bridge `lib.rs` | carriers produce a `Channel` directly | Rust programs embedding rutis-bridge; tests; the docs promising "implement Transport" |
| `Embedding.transports`, `Embedding.identity` | rutis-host `host.rs` | a host's own carrier and credentials go in its own connection plugin | Rust programs embedding rutis-host; the e2e `fixture-host` |
| `runtimes.remote` (`name`, `language`, `dial`, `listen`, `instances`, `transport`) and `connect()`'s field-combination checks | rutis-host `config.rs`, `host.rs` | remote runtimes are written as connection plugin rows | rutis.json files using it; persisted samples 0.10–0.18 |
| `Identity`, `IdentityPlugin`, `identity_key`, `StaticIdentity`, `Credential`, `Presented` | rutis-bridge `identity.rs` | authentication is in the connection plugin | tests; embedders |
| `Admission`, `AdmissionPlugin`, `admission_key`, `SharedToken`, `Rejection` | rutis-bridge `identity.rs` | listen compares tokens; ids come from the handshake | rutis-host; tests |
| `Revocations`, `Revocable` | rutis-bridge `identity.rs` | no production caller; existing connections are cut by stopping their row | `revocation.rs`; three guarantee rows; the TLA model |
| `Registration`, `Registered`, `RegistrationError`, `Registrations`, `Ticket`, `Refusal`, `Deliver` | rutis-bridge `registration.rs` | dispatch by token moves into `rutis-host/listen`; registration generations are gone | WebSocket listening; `memory_mux.rs`; the TLA model |
| `AccessPointPlugin`, `AccessPointConfig`, `AccessEvent`, `Accept`, `Accepted`, `Release` | rutis-bridge `access_point.rs` | the callback-shaped handover | rutis-host; loader `connections.rs`; tests |
| `LinkPlugin`, `LinkConfig`, `LinkState`, `Connect`, `Failure`, `Retry`, `protocol()` | rutis-bridge `link.rs` | dialling and reconnecting move to `rutis-host/dial`, listening to `rutis-host/listen`; `protocol()` has no caller | rutis-host; tests; `examples/smoke.rs` |
| `PeerPlugin`, `PeerHandle`, `Features` | rutis-bridge `compose.rs` | a composition plugin; `PeerPlugin::new` always returns `Ok` | rutis-host; loader; tests |
| `Peer`, `peer_key`, `Handler`, `Offered`, `Offers`, `family()` | rutis-bridge `peer.rs` | merged into the session; `family()` has no caller | bridge internals; tests |
| `ExportPlugin`, `ImportPlugin`, and `export`, `import` | rutis-bridge `services.rs`, the list part of `events/link.rs` | decision 1 | node configurations; tests; a hint in `check.rs` |
| `HostPlugin`, `PluginCatalog`, `StaticCatalog`, `Described`, `Installed`, `ServiceKeys`, and `host`, `rows` | rutis-bridge `host.rs` | what the far end runs goes through `run` only | node configurations; `peer_rows.rs`, `node.rs` |
| `peer:<peer>/<plugin>` rows, the row name `rutis-bridge/peer` (and its `runtime`, `transport`, `identity` fields), `PeerRows`, `PeerResolver`, `PeerRowsPlugin`, `LoaderCatalog`, `register_peer_node`, `node_schema` | rutis-loader `peer.rs` | remote plugins are only `<id>:<plugin>`; a node is a connection plugin's row | node configurations; persisted samples |
| `RuntimeSession`, `runtime_session_key`, `RuntimeAccessPlugin` | rutis-bridge `runtime/` | the service-shaped handover | loader; tests |
| `RuntimePlugin::remote` | rutis-bridge `runtime/plugin.rs` | remote and local go through the same session | rutis-host; loader; tests |
| `LocalRuntime` | rutis-bridge `runtime/local.rs` | starting processes moves to `rutis-host/spawn` | rutis-host; tests |
| `RuntimeInstances`, `AcceptInstances`, `RuntimeConnection` | loader `runtime/connections.rs` | decision 3: the host creates no instance per connection | rutis-host; loader tests; rutis-migrate's name table |
| `LinkState`, `AccessEvent`, the remote part of `RuntimeState` | bridge, rutis-host `status.rs` | become "runtime state + whether the session is connected" | rutis-host's output code |
| `ChannelInfo`, `Closer::replaced`, `Closer::rejected` | rutis-bridge `channel/mod.rs` | §3 | carriers; tests |
| `ConnectError` (out of `channel`) | rutis-bridge `channel/mod.rs` | goes inside `rutis-host/dial` | carriers and dial |
| `MemoryPlugin`, `MemoryTransport`, `LocalPlugin`, `WebSocketPlugin`, `WebSocketTransport` | `transport/*` | exist only to provide `Transport#<kind>` | tests; rutis-host |
| the compat format: `Format::Compat`, `COMPAT_CAPABILITIES`, the `rust:` / `node:` call prefixes, `PROTOCOL = 2`, `compat` and `name_far_end` in `link.rs` | rutis-bridge `session/rpc.rs`, `session/mod.rs`, `link.rs`, `runtime/process.rs:919` | every runtime uses the endpoint format, ids come from the handshake | local runtimes' start-up arguments; fuzz targets; tests |
| each language runtime's compat path, and `--format` in Node and Bun | `node/rutis-runtime`, `bun/rutis-bun`, `python/rutis`, `go/rutis`, `swift/rutis`, `kotlin/rutis` | as above | every language runtime and SDK (released together in 0.19) |
| the host side's fallback to the shared `RUTIS_TOKEN` | rutis-host `config.rs` | one token per name; the fallback only brings the extra "who shares it" check | hosts that set only `RUTIS_TOKEN` |
| `ServiceCatalog::share_registered_only` and its `by_name` switch | rutis-loader `catalog.rs` | a switch back to pre-0.12 behaviour, with no production caller | three tests (`check.rs`, `scope.rs`, `marked_services.rs`); the open-host and rust-host guides |
| all of `revocation.rs`, `an_id_revoked_while_its_connection_takes_over_is_not_accepted`, the Registration-generation unit tests | rutis-bridge | removed with revocation and Registration, not renamed and kept | — |

## 9. Before and after

Line counts are `wc -l` on `3d19634f` (including in-file unit tests); "public items" is the number of lines starting with `pub fn|struct|enum|trait|type|const|use|mod` (including public methods inside impls). The after column is an **estimate**.

| File | Lines now | Public items now | Lines after (estimate) |
| --- | ---: | ---: | ---: |
| rutis-bridge `lib.rs` | 177 | 29 | 50 |
| `channel/mod.rs` | 165 | 13 | 110 |
| `access_point.rs` | 607 | 16 | 0 |
| `compose.rs` | 270 | 6 | 0 |
| `identity.rs` | 564 | 27 | 40 (only `fingerprint`) |
| `registration.rs` | 409 | 13 | 0 |
| `link.rs` | 751 | 15 | 0 |
| `peer.rs` | 382 | 14 | 0 |
| `services.rs` | 543 | 6 | 0 |
| `host.rs` | 359 | 10 | 0 |
| `runtime/access.rs` | 109 | 2 | 0 |
| `runtime/plugin.rs` | 667 | 23 | 350 |
| `runtime/local.rs` | 417 | 22 | 0 |
| `runtime/mod.rs` | 142 | 14 | 110 |
| `events/link.rs` | 795 | 2 | 400 |
| `transport/websocket/mod.rs` | 262 | 8 | 150 |
| `transport/memory/mod.rs` | 409 | 11 | 200 |
| `transport/local/mod.rs` | 206 | 13 | 150 |
| `session/rpc.rs` (compat removed) | 3267 | 49 | 3120 |
| new `session/plugin.rs` (Session, SessionHandle, `sessions`, services and listeners both ways) | — | — | 550 |
| **rutis-bridge subtotal** | **10501** | **293** | **5230** |
| rutis-loader `runtime/connections.rs` | 550 | 10 | 0 |
| `runtime.rs` | 1621 | 14 | 1400 |
| `peer.rs` | 998 | 14 | 0 |
| `runtime/discover.rs` | 53 | 0 | 53 |
| `catalog.rs` | 528 | 23 | 505 |
| **rutis-loader subtotal** | **3750** | **61** | **1958** |
| rutis-host `host.rs` | 2089 | 27 | 1650 |
| `config.rs` | 1434 | 42 | 1100 |
| `status.rs` | 979 | 18 | 780 |
| new `connect.rs` (spawn, dial, listen) | — | — | 600 |
| **rutis-host subtotal** | **4502** | **87** | **4130** |
| **Total** | **18753** | **441** | **about 11320** |

- This table counts `session/rpc.rs` and `catalog.rs` too, which the previous version did not (removing compat and `share_registered_only` touches them).
- Names exported at the rutis-bridge root: 59 now, about 7 after (`Channel`, `Close`, `PeerId`, `Session`, `SessionHandle`, `RowHandle`, `thrown`), estimated.
- The session's public API: four names, `Session::new`, `SessionHandle::id`, `SessionHandle::run`, `sessions.open`.
- Lifecycle paths go from 4 to 1, handover shapes from 2 to 1, admission, replacement and state each from 3 copies to 1 (replacement only in listen); nodes and remote runtimes have no code of their own.
- Each language runtime's entry points (after removing compat, estimated): Node `runner.mjs` 1162 → 1140, `session.mjs` 810 → 740; Bun `main.ts` 203 → 195; Python `__main__.py` 352 → 340, `peer.py` 1903 → 1820; Go `serve.go` 295 → 270; Swift `Serve.swift` 263 → 240; Kotlin `Peer.kt` 1438 → 1360, jvm `Serve.kt` 217 → 205. The Kotlin and Swift SDKs gain apps installing their own plugins and `services.announce` (A1, A2), about 150 more lines each.
- Tests: files testing only removed interfaces are deleted or rewritten, about 5500 lines (`access_point.rs` 584, `revocation.rs` 798, `link.rs` 547, `fault_links.rs` 959, `memory_mux.rs` 730, `runtime_instances.rs` 703, `peer_rows.rs` 556, most of `node.rs` 1011, the `runtimes.remote` part of `remote_runtimes.rs`, …), estimated.

## 10. What happens to the merged PRs

**Conclusion: no revert; replaced inside the new implementation.** Why:

- The bulk of the mess (four paths, two shapes, three copies of admission and state) predates 0.19; reverting #768 and #791 only returns to the same state in 0.18 and solves nothing.
- Both PRs still have parts the new architecture uses (the "Kept" column below); reverting and redoing them would cost CI, the migration guide and the guides one more round each.

| PR | Kept | Replaced or removed |
| --- | --- | --- |
| #768 (R1 loader) | `discover.rs` (`plugins` in the `mount` reply, with `rows.schema` as fallback); `Process::check_rows` (error text without the version number); `Unavailable` for a missing plugin and its text; the cases of `remote_modes.rs` (run through spawn / dial / listen instead) | the two call sites become one resolver that looks up a session |
| #791 (R2 rutis-host) | the token checks for one-runtime listen and for one listener (#702); `status`'s `connections:` section and the 0.18 status channel; the `Report` parameter of `control::serve`; the *Migrating to 0.19* file (its content rewritten per §11) | the `runtimes.remote` syntax (`dial` / `listen` / `instances` / `transport` / `language`) and its checks; `Embedding.transports`; the connections `host.rs` builds; the `runtime <name>: link …` output (now §7); `remote_runtimes.rs` rewritten for the new syntax |
| #799 | — | already closed, not reopened. The guarantees §15 recount it carried (`2d817aa3`) is redone in the implementation PR that changes the guarantees |
| branch `claude/remote-drop-transport` (not pushed, `9a5543cc`) | its removal of the `transport` field and `Embedding.transports` | folded into S4 of §13 (with `runtimes.remote` gone entirely, only part of it is used); not pushed on its own |

## 11. Compatibility and *Migrating to 0.19*

No compatibility. Unchanged are only: the endpoint format of the session protocol; rutis.json `format` stays 1; the syntax of `runtimes.node` / `py` / `go` / `swift` / `bun` and the other local runtimes.

The "Remote runtimes" section of *Migrating to 0.19* is rewritten to give only the new syntax and the mapping:

1. **Remote runtimes and nodes**:

   | 0.18 | 0.19 |
   | --- | --- |
   | a `runtimes.remote` entry + a `rutis-bridge/peer` row with `runtime`, `dial` | one row `{ "id": "gpu", "name": "rutis-host/dial", "config": { "address": "<address>" } }` |
   | the same, `listen` | one `rutis-host/listen` row, `"listener": "<listener>"` |
   | `listen` + `instances` in `runtimes.remote`, and the rows in the `instanced` group | one `rutis-host/listen` row; the remote plugin rows in the group go, and the app installs its plugins itself (decision 2); rows one per device on the host (`py:coach`) change per decision 3 |
   | a `rutis-bridge/peer` link row with `dial` / `listen` | the same two rows |
   | `export`, `import` (services and events) | nothing: services are visible on both sides, events are one bus |
   | `host: true` + `rows: true` + a row `peer:office/scanner` | a row `office:scanner` |
   | `transport`, `identity`, `Embedding.transports`, `Embedding.identity` | a connection plugin of your own with your own carrier and credentials (§5), registered through `Embedding.builtins` |

2. **Your own way of connecting**: "implement a `Transport` and add it to `Embedding.transports`" becomes "hand over a `Channel`: write a connection plugin that calls `Session::new(channel)`; in other languages use `sessions.open`".
3. **Rust API**: "the Rust APIs of rutis, rutis-loader and rutis-bridge do not change" becomes a table:

   | 0.18 | 0.19 |
   | --- | --- |
   | `LinkPlugin` + `PeerPlugin { runtime }` + `RuntimePlugin::remote` | `Session::new(websocket::dial(...)?)` in a connection plugin, rows `<name>:<plugin>` |
   | `AccessPointPlugin` + `RuntimeInstances` | `rutis-host/listen`, or a connection plugin of your own |
   | `ExportPlugin` / `ImportPlugin` / `HostPlugin` | not needed |
   | `Identity` / `Admission` / `Revocations` | the connection plugin's own authentication; ids come from the handshake |
   | `Transport` | `websocket::dial` / `websocket::listen` / `pipe::spawn` give a `Channel` directly |
   | `Closer::replaced()` / `rejected(r)` | `close(Close::Replaced)` / `close(Close::Rejected(r))` |
   | `rutis_bridge::ConnectError` | inside `rutis-host/dial` |

4. **Apps (Kotlin, Swift)**: plugin configuration goes into `RutisApp`, and the app installs its plugins itself once connected (decision 2); the `<runtime>:<plugin>` rows in the rutis-side `instanced` group go. Rows one per device on the host (`py:coach`) are no longer supported; change them per decision 3: move them onto the device, or have them tell devices apart themselves.
5. **Upgrade together**: a 0.19 host cannot meet 0.18 or earlier runtimes and SDKs (compat is gone; the handshake refuses by protocol version); hosts and every language runtime and SDK move to 0.19 together. The cross-version guarantee holds only between adjacent releases with no incompatible change.
6. **Tokens**: the host side accepts only `RUTIS_TOKEN_<name>`; a host that set only `RUTIS_TOKEN` sets one per name. `RUTIS_TOKEN` on the runtime side is unchanged.
7. **`ServiceCatalog::share_registered_only` is removed**: a Rust embedding calling it drops the call (sharing by name is the default since 0.12).
8. **Persisted samples**: the 0.8–0.18 samples contain `runtimes.remote`, `rutis-bridge/peer` rows and `peer:` rows, which 0.19 no longer reads. Those parts are taken out of the samples (expectations along), `rejected/` cases are added whose errors are exactly what the 0.18 rules say (an unknown field, an unknown plugin name), and the guarantee row for persisted samples states this 0.19 exception.
9. **Output**: connection status lines become §7's (prefixed with the row id, connected / disconnected); `status`'s `connections:` lists the remote plugins after each connection.

## 12. Documents to change with it

| Document | Change |
| --- | --- |
| `docs/guide/nodes.md` / `.en.md` | rewrite: remote runtimes and nodes are both `rutis-host/dial` / `rutis-host/listen` rows; the `export` / `import` / `host` / `rows` / `runtime` table and the list rules of "Events between nodes" go, replaced by "services are visible on both sides, events are one bus"; "Running plugins on another node" becomes `office:scanner`; "implement a `Transport`" becomes "write a connection plugin"; the revocation hook in "Security" goes, replaced by "existing connections are cut by stopping their row" |
| `docs/guide/rutis-host.md` / `.en.md` | the `runtimes.remote` section goes, replaced by rows of the three connection plugins; the start-up error list along; delete `Embedding`'s `transports` and `identity`; "What run prints" per §7 |
| `docs/guide/rust-host.md` / `.en.md` | the `PeerPlugin`, `RuntimePlugin::remote` example becomes a connection plugin + `Session::new` |
| `docs/guide/swift-app.md`, `kotlin-app.md` (both languages) | the rutis side becomes `rutis-host/listen` and `rutis-host/dial` rows; the app configures its plugins in `RutisApp` and installs them itself; the "one `py:coach` per device" example goes, replaced by decision 3's two ways |
| `docs/design-app-integration-2026-10-14.md` / `.en.md` | D2 and D6 rewritten: the plugins on a device are installed and configured by the app; "one instance per connection" in §5 goes |
| `docs/guide/open-host.md`, `rust-host.md` (both languages) and §9 of `design-host-writes-plugins-2026-10-27.md` | remove the mentions of `share_registered_only` |
| each language runtime's guide (`typescript-plugin`, `bun-plugin`, `python-plugin`, `go-plugin`, `swift-plugin`, `kotlin-app`) and `.agents/skills/rutis-new-runtime/references/runtime-contract.md` | local start-up arguments always carry `--id`; the endpoint format only; compat and `--format` gone |
| `docs/guide/cordis.md`, `plugin-api.md`, `bun-plugin.md` and others mentioning nodes, `peer:` or `runtimes.remote` | rewritten in the new syntax at each place |
| `.agents/skills/rutis-remote-plugin/SKILL.md` and `references/nodes.md`, `rutis-json.md`, `plugin-api.md` | as the guides; "implement Transport and add it to Embedding.transports" becomes "hand over a Channel / write a connection plugin", with a minimal `sessions.open` example |
| `.agents/skills/rutis-new-runtime/references/plugin-api.md` | the section on making an existing runtime work remotely: nothing changes on the runtime side; on the rutis side, a `rutis-host/dial` / `listen` row |
| `docs/migration-0.19.md` / `.en.md` | §11 |
| `docs/guarantees.md` / `.en.md` | narrow `bridge/cross-version-meetings` to hold only between adjacent releases with no incompatible change, stating that 0.19 and 0.18 are not guaranteed to meet and that hosts and every language runtime move to 0.19 together; `events/old-peer-refused` along (0.18 and earlier are refused in the handshake); remove `bridge/revocation-closes-only-its-connections`, `bridge/revocation-refuses-reconnects`, `host/env-tokens-not-revocable`, `events/link-export-import`, `events/node-config`, `events/no-multi-hop`; replace `host/remote-runtimes` with a row for the three connection plugins; remove `loader/runtime-connection-instances`; rewrite `loader/peer-rows` as `<id>:<plugin>` rows through `run`; rewrite `events/node-links` as "the far end's listeners are entries on this end's bus"; in `bridge/dial-out-rules` the source of 4003 becomes listen and the handshake check; a new row: listen's replacement order; the persisted-sample row states the 0.19 exception; every row's test names follow the tests; §15 recounted |
| `specs/remote-lease/` (both READMEs, both `.tla`, configs, `expected.txt`) | removed with revocation and Registration: the revocation actions, registration generations and the hand-over re-check, the properties `RevokedStaysOut`, `RevocationScoped`, `NoStaleHandOver`, the configs `access-revoke-same`, `access-revoke-distinct`, `access-mut-revoke-cuts-all`, `access-mut-route-by-id`, `access-mut-no-gen-check`, `access-registration`, and the revoked far end in `RemoteLease.tla`; `access-mut-accept-before-release` stays as the mutation for listen's replacement order. Once listen's new code is written, a minimal model is built from it (same-id replacement: unload the old before accepting the new), replacing `AccessPoint.tla` |
| `crates/rutis-bridge/README.md`, the module docs in `lib.rs`, the root `README` | describe Channel, session and the carrier functions |
| the old design `design-remote-runtime-consistency-2026-10-26.md` / `.en.md` | a header line: "replaced by [Remote connections: one session plugin](design-remote-session-2026-10-28.en.md)" |

## 13. Implementation

Add first, delete after; after each PR merges, main works and CI passes.

| PR | Content | Done when | Depends on |
| --- | --- | --- | --- |
| S1 rutis-bridge: Channel and carrier functions | `Close`, `close(Close)`; remove `ChannelInfo`; add `websocket::dial`, `websocket::listen`, `pipe::spawn`, `memory::pair` (the existing `Transport` implementations call them) | existing tests pass with unchanged cases (only the `close` calls change); new unit tests: the three `Close`s on the three carriers (4002, 4003, stdin closed) | — |
| S2 rutis-bridge: the session plugin | `Session::new`, `SessionHandle::{id, run}`; remote plugins `<id>:<plugin>`; services visible on both sides (borrowed and provided) and the far end's listeners on this end's bus (the runtime's existing code, extended to rutis-node far ends); the `sessions` service and `open(far)`; the old links / access points untouched | tests: the four endings; two rutis kernels over a memory Channel using each other's services, a same-named one refused by the kernel's rule, events reaching both ways; Node and Python test plugins run `run` through `sessions.open` | S1 |
| R1–R6 language runtimes: endpoint format locally | one PR per language (Node, Bun, Python, Go, Swift, Kotlin), per the table in §3: local channels also build an endpoint from `--id`; compat stays for now, so they still run under a host not yet changed | each language's existing tests pass; new test: a local channel gives the name from `--id` in its handshake | — |
| A1, A2 Kotlin and Swift SDKs: apps install their own plugins | plugins in `RutisApp` may carry a configuration; once connected the app installs its plugins itself and reports their services with `services.announce`; no more waiting for `rows.v2` | each SDK's existing tests rewritten for the new behaviour; new test: two devices connect to one listen, each one's plugins register as `<id>:<plugin>`, and a same-named service from the later one is refused, saying why | — |
| S2b local runtimes on the endpoint format, compat removed | the host code starting local runtimes passes `--id` and uses the endpoint format; remove `Format::Compat`, `COMPAT_CAPABILITIES`, the `rust:` / `node:` prefixes, `PROTOCOL = 2`, `compat` in `link.rs`; every language removes its compat path and `--format` | every runtime test and `runtime_conformance` pass on the endpoint format; fuzz targets move to endpoint; `grep -i compat` finds nothing in `crates/rutis-bridge/src/session` or the runtimes' session code | R1–R6, S1 |
| S3 rutis-loader: one resolver | rows `<name>:<plugin>` look up the session by connection row and call `run`; the old path stays alongside | `remote_modes.rs`'s cases run on sessions: spawn, dial, one-runtime listen give word-for-word the same missing-plugin text | S2 |
| S4 rutis-host: three connection plugins | `rutis-host/spawn`, `dial`, `listen` (schemas with `additionalProperties: false`; listen's token dispatch, same-id replacement); `runtimes.*` expand into spawn; remove `runtimes.remote`, `Embedding.transports`, `Embedding.identity` (folding in the drop-transport branch) and the host side's `RUTIS_TOKEN` fallback; state becomes two items, §7's output; persisted samples and `rejected/` cases | `remote_runtimes.rs`, `dial_in.rs`, `kotlin.rs` pass end to end in the new syntax (many devices rewritten per decision 3); nodes end to end (two rutis-hosts using each other's services, events, `office:scanner`); `dial_conformance` passes against `rutis-host/listen` for every language (4002, 4003 as before); the `persisted` test passes; the e2e `fixture-host` builds; #702's token-check cases pass | S3, S2b, A1, A2 |
| S5 removal | remove everything in §8 (including `share_registered_only`) and the tests that test only it; remove the cross_version e2e scenarios meeting 0.18 (0.19 and 0.18 are not guaranteed to meet); remove the revocation and Registration parts of the TLA models and the configs listed in §12; build the minimal model from S4's listen code | `grep` for the §8 names under `crates/` finds nothing; `cargo test --workspace` and clippy pass; the new model and `access-mut-accept-before-release` run, `expected.txt` updated | S4 |
| S6 documents | everything in §12; *Migrating to 0.19* per §11 | both languages say the same; the docs no longer promise `Transport`, `Embedding.transports`, `Revocations`, `export` / `import`, `rutis-bridge/peer` or `runtimes.remote`; every test name in the guarantees exists | S5 (behaviour visible with S4 can go into the guides with S4) |
