# Remote connections: one session plugin

[中文](design-remote-session-2026-10-28.md)

Status: draft, not implemented. Date: 2026-10-28. Base: `main` `3d19634f`. Scheduled: 0.19.
Issue: [#695](https://github.com/arcships/rutis/issues/695) (implemented by [#696](https://github.com/arcships/rutis/issues/696) loader, [#698](https://github.com/arcships/rutis/issues/698) rutis-host). Basis: the two 2026-10-28 comments on #695.
Relation to earlier designs: replaces the implementation part of [Remote runtime consistency](design-remote-runtime-consistency-2026-10-26.en.md) (the "inside rutis-host" column of §4, §5, §7, §8); its configuration (`dial` / `listen` / `instances` in `runtimes.remote`) and "a missing plugin is always `Unavailable`" stay. The [decision record](decision-remote-runtime-consistency-2026-10-26.en.md)'s "no new handover interface, do not rewrite links and access points" is overturned by this design; see §1 for why.

Out of scope: the session protocol (unchanged); the session side of each language runtime and SDK (unchanged); opening up authentication (a host's own credentials belong in its own connection plugin, with no interface in the core); revocation while running (see `Revocations` in §8); the loader's row model and `instanced` groups themselves.

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

### 2.1 Plugins

| Plugin | Belongs to | Does |
| --- | --- | --- |
| `rutis-bridge/session` | rutis-bridge; the only remote-related plugin in the core | brings the far end's plugins into the local kernel over one `Channel`; one per Channel |
| remote plugin `<id>:<plugin name>` | a child of the session, registered automatically | one plugin of the far end, as it looks here |
| connection plugin | outside the core; anyone can write one | gets a `Channel` and mounts a session on it |
| └ `rutis-host/spawn`, `rutis-host/dial`, `rutis-host/listen` | shipped in rutis-host | start a local process, dial, listen |

### 2.2 Structure

```text
connection plugin (rutis-host/dial, rutis-host/listen, rutis-host/spawn, or anyone's)
 └─ rutis-bridge/session             one per Channel
     ├─ <id>:weather                 a far plugin, registered automatically
     └─ <id>:llm
```

### 2.3 Relations

- **Connection plugin → session**: `ctx.plugin(Session::new(channel))`. The Channel closes, the session unloads; the session unloads, the Channel closes.
- **Session → remote plugins**: a far plugin is registered as a child when it starts running and unloaded when it ends; when the session unloads they all unload with it.
- **Remote plugins ↔ local plugins**: as for any plugin (`inject` / `provide`); every lifecycle is the kernel's own.
- **Replacement**: when the same id connects again, through whatever connection plugin, the session core unloads the old one before mounting the new one.
- **Isolation**: `isolate` on the connection plugin's row; when each connection needs a scope of its own, the loader's existing `instanced` groups (§6).

### 2.4 API

Rust (connection plugins compiled into a host):

```rust
pub struct Channel { /* §3 */ }

pub struct Session;
impl Session {
    pub fn new(channel: Channel) -> Self;
    /// Called once the far end has said its id, before replacement and registration;
    /// an Err closes the Channel with Close::Rejected.
    pub fn admit(self, check: impl Fn(&str) -> Result<(), String> + Send + Sync + 'static) -> Self;
    /// The services and events (names or `*` prefixes) this end offers on this connection; for node links.
    pub fn export(self, names: impl IntoIterator<Item = String>) -> Self;
    /// Lets the far end run plugins installed here (a link row's `host: true`).
    pub fn host(self, catalog: Arc<dyn PluginCatalog>) -> Self;
    pub fn handle(&self) -> SessionHandle;
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
interface Options { admit?(id: string): void; export?: string[] }
sessions.open(far: Far, options?: Options): Near
```

- The functions in `far` and `near` cross languages by reference (0.18 #687).
- A session `open` creates is mounted under the plugin that called it: when that plugin unloads, all its sessions unload.
- `admit` and `export` are the Rust methods of the same names; `host` exists in Rust only (the plugin catalog belongs to the host).

`admit` and `export` are the two additions beyond the #695 comments. Why: the far end's id is known only from its hello, and a rejection has to happen before replacement (otherwise a connection that will be rejected has already unloaded the old connection with the same id — exactly the bug the TLA mutation `access-mut-accept-before-release` catches), so a connection plugin needs a decision point after the hello and before replacement; `export` is the provider's list decided by open item 1 (§2.6).

### 2.5 The structure afterwards

```text
rutis-host
 ├─ rutis-host/spawn  py          ── pipe::spawn ──────────▶ Channel ─▶ session (id py)
 │                                                                       └─ py:coach … (run)
 ├─ rutis-host/dial   gpu         ── websocket::dial ─────▶ Channel ─▶ session (id gpu)
 │                                                                       └─ gpu:embedder (run)
 ├─ rutis-host/listen worker      ── websocket::listen ──▶ one Channel per connection
 │     ├─ session (id worker-1)    └─ worker-1:chunker …
 │     └─ session (id worker-2)    └─ worker-2:chunker …
 ├─ rutis-host/dial   office (node) ── export: [weather] ─▶ session (id office)
 │                                                        └─ office:clock … (what the far end exports)
 └─ a TS plugin relay an agent wrote while running ── sessions.open(far) ──▶ session (id from the far end)
```

Every connection has the same shape; below the session there is no per-case branch.

### 2.6 Decisions on the two open items

**Decision 1: node links keep only the provider's `export`; `import` goes.**

- Rule: what one end `export`s, services and events, the other end registers in full; to limit it, write `isolate` on the receiving connection row.
- Basis (every use in [Connecting nodes](guide/nodes.en.md) and the code was checked: `node.rs`, `multihop.rs`, `fault_links.rs`, `websocket_multihop.rs`, `websocket_soak.rs`, `cordis_node.rs`, `events_nodes.rs`, the persisted samples):
  - The three uses of `import` each have a replacement without a new mechanism:
    1. Taking part of the far end's services: decided by the provider's `export` (one row per connection; the provider writes it per peer anyway). A receiver that does not trust the provider writes `isolate` on the connection row: those services stay below that row and other local rows cannot see them.
    2. Refusing a name that collides with a local service: kept, by the kernel's existing rule (the later registration is refused); that remote plugin fails and says why (§7).
    3. Events: today the emitting end must `export` and the listening end `import`. Now only the emitter's `export` counts: the listening end announces all its listeners to the far end, and the emitter sends only the names in its `export`. A listener wanting to filter by origin uses the existing filters (a far listener's descriptor carries its scope, [Unified events](design-events-2026-10-18.en.md)).
  - Runtimes (local and remote) never used `export` / `import`: their services and listeners are directly on the host. Without `import`, nodes behave like runtimes.
- One existing use is lost. `import: ["t/s/*"]` in `events_nodes.rs` makes the listening end announce only some of its listeners' names; afterwards it announces all of them, so the far end can see which event names this end listens to. Only event names are announced, not data — data still comes only for what the emitter `export`s — and nobody needs those names hidden ([Design philosophy](design-philosophy.en.md) §7, "should we build it", point 4), so there is no replacement and no new mechanism; the `events/link-export-import` row of the guarantees says so.
- The protocol does not change: announcements and offers use the existing frames; only their content changes from the `import` list to everything. A 0.18 node meeting a 0.19 node still filters by its `import` on its side, and that works.

**Decision 2: a plugin's configuration belongs to whoever starts it.**

- Plugins the far end starts itself (a node's `export`, an app on a device starting its own), are configured there; locally there is only the registered `<id>:<plugin>`.
- When the host decides what the far end runs and with what configuration, it uses `run(plugin, config)`; a row `<runtime name>:<plugin>` with `config` in rutis.json is a `run` on that runtime's session.
- How apps get configuration today ([App integration](design-app-integration-2026-10-14.en.md) D2, D6; Kotlin `RutisApp`, Swift `RutisApp`): the app only reports the plugins it carries (`plugins` in the `mount` reply); which one loads and with what configuration comes from the rows of an `instanced` group in the rutis configuration, and the SDK `apply`s only when a `rows.v2` start request arrives. That is "the host starts it", which in the new architecture is `run`.
- Migration: **apps and SDKs do not change.** A `phone:health` row in the `listen` + `instances` group becomes one `run("health", config)` in each app connection's instance; the local rows using it (`py:coach`) stay in the same instance. Each device's configuration is still the group's one, as today (there is no per-device configuration today either); a plugin that should behave differently per device reads the device's own settings, which is the plugin's business (D7).
- Not done: apps starting plugins themselves and reporting them to rutis (it needs an export side in the SDKs and has no user). If it is ever needed, it is a node's `export`, with no new mechanism.

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
- `ChannelInfo` goes: the far end's id comes from the session handshake; `transport` and `label` were for diagnostics only, which the connection plugin's own output now gives.
- `Closer::close(&str)`, `replaced()` and `rejected(&str)` become `close(Close)`. What each carrier does:

  | `Close` | WebSocket | local pipe (child process) | memory |
  | --- | --- | --- | --- |
  | `Done` | close code 1000 (1001 when the host stops) | closes the child's stdin | the other end's `recv` gets `Ok(None)` |
  | `Replaced` | close code 4002 | as `Done` (the child belongs to this end and is never replaced) | the other end's `recv` gets `Closed` |
  | `Rejected(r)` | close code 4003, reason `r` | as `Done` | the other end's `recv` gets `Rejected` |

- `ChannelError { Closed, Rejected }` stays: `Rejected` means the far end refused this end with 4003.
- `ConnectError` leaves the core: it classifies a failure to dial and only dialling and its retries use it. It becomes the error of `websocket::dial`, and `rutis-host/dial` backs off by it (retryable 0.5 → 30 s, auth rejected every 30 s, incompatible stops; rules unchanged).
- Once given to `Session::new`, only the session closes the Channel; before that, the connection plugin may close it itself (a wrong token at upgrade, say).

## 4. Lifecycle

### 4.1 Channel and session

One to one, born and gone together, never reused.

| What happened | Who knows first | Result |
| --- | --- | --- |
| Handshake done | session | calls `admit(id)`; if it passes, replaces as in 4.2, then starts registering remote plugins |
| `admit` refuses | session | `close(Rejected(reason))`; nothing is unloaded, nothing registered |
| Handshake fails (protocol version, missing capability, 10 s timeout) | session | the far end first gets the protocol's own error frame, then `close(Done)`; the session fails with that reason |
| The far end leaves first (`recv` returns `Ok(None)` or an error) | session | the session unloads: remote plugins unload in reverse, calls in flight fail once |
| This end leaves first (connection plugin unloads, row removed, host stops) | kernel | the session unloads: remote plugins unload in reverse, then `close(Done)` |
| Reconnect | connection plugin | a new Channel, a new session; the old session is already gone |

A connection plugin waits for the session to unload through the kernel handle `ctx.plugin(...)` returns, then decides whether to reconnect (dial backs off and reconnects; spawn follows the runtime's `restart`; listen does nothing). The session never reconnects.

### 4.2 Replacement

- The session core keeps an id table (one per host, in `rutis-bridge/session`'s own shared state). `<id>:<plugin>` is unique in the whole host.
- When a new session's `admit` passes and the table already has the id:
  1. the old session unloads (its remote plugins unload in reverse, every cleanup finishes);
  2. the old Channel is `close(Replaced)`;
  3. the new session enters the table and starts registering remote plugins.
- Replacements of one id run one after another; of two connections arriving together, the one whose handshake finishes later replaces the earlier.
- A connection plugin only refuses (`admit`). With many connections, `rutis-host/listen` refuses an id that names another connection row of this host (a device saying `py`), so a device cannot replace a local runtime; the same id under the same connection plugin replaces as usual (a device reconnecting).
- Two guarantees stay: close codes 4002 (replaced) and 4003 (rejected), and "unload the old before accepting the new". They are the TLA mutation `access-mut-accept-before-release` and the test `an_id_revoked_while_its_connection_takes_over_is_not_accepted` (renamed `a_connection_whose_listener_stopped_while_it_takes_over_is_not_accepted`, see §12).

### 4.3 Remote plugins on the kernel's lifecycle

| Far end | The local remote plugin `<id>:<plugin>` |
| --- | --- |
| The plugin starts running (`run` succeeded, or the far end `export`s it) | registered as a child of the session, its `apply` done; the services it provides appear as ordinary `provide`s |
| `run` fails (the far end throws) | this child fails, with the far end's name, message, `code` and `site` (0.18 §4.2) |
| The far end ended it (`rows.ended`, or no longer `export`ed) | the child unloads; rows using its services stop or wait by the kernel's rules |
| The connection drops | the session unloads, and every child with it |

- There is no separate "connection state" for plugins to look at: connected means the session exists, disconnected means it does not. A runtime's state (whether the local process is up) belongs to the connection plugin; "runtime state + whether the session is connected" replaces the three of `LinkState`, `AccessEvent` and `RuntimeState`.
- A row in rutis.json that runs at the far end (`gpu:embedder`) waits while the far end is away ("waiting for runtime gpu"); once a session appears the loader calls `run`; a far end without the plugin makes the row `Unavailable` (#768's text, unchanged).

## 5. Extending

There is one extension point: write a connection plugin, get a `Channel`, mount a session.

### 5.1 A Rust connection plugin

```rust
// Reach a runtime through a relay
async fn apply(ctx: &Ctx, config: Relay) -> Result<(), Error> {
    loop {
        let channel = relay::connect(&config.address, &config.token).await?;  // its own auth and carrier
        let session = ctx.plugin(Session::new(channel).admit({
            let want = config.name.clone();
            move |id| (id == want).then_some(()).ok_or_else(|| format!("expected {want}, got {id}"))
        }));
        disposed(session.watch()).await; // the kernel's FiberView::watch(): wait until the session unloads
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

## 6. The shipped connection plugins, and how rutis.json expands

The three plugins live in rutis-host (not in rutis-bridge), on plain functions in rutis-bridge: `websocket::dial`, `websocket::listen`, `pipe::spawn` (and `memory::pair` for tests).

| Plugin | Does | `admit` |
| --- | --- | --- |
| `rutis-host/spawn` | `pipe::spawn` starts a local runtime process and mounts a session; restarts it by the runtime's `restart` when it exits | the id is the name of the process this row started |
| `rutis-host/dial` | `websocket::dial`, mounts a session; after the session unloads, backs off by the `ConnectError` category and reconnects | the id equals this row's name |
| `rutis-host/listen` | `websocket::listen`: compares the token against the current list at upgrade (401 / 403 unchanged); mounts a session per Channel; with `instances`, first creates an instance named after the id in that `instanced` group with the loader's existing `create_instance`, mounts the session in it, and closes the instance when the session unloads | one: the id equals this row's name; many: the id is not the name of another connection row of this host |

Dispatching the rows on one listener by token (what `Registrations` does today) belongs to `rutis-host/listen`; a token collision on one listener is still a start-up error (#702).

The rutis.json syntax does not change; the host expands it into rows of these three plugins:

| rutis.json | Expands to |
| --- | --- |
| `runtimes.py` (and `node`, `bun`, `go`, `swift`, `kotlin`, the local runtimes) | `rutis-host/spawn`, `id` the runtime name |
| a `runtimes.remote` entry with `dial` | `rutis-host/dial`, `id` the runtime name |
| a `runtimes.remote` entry with `listen` | `rutis-host/listen`, one |
| a `runtimes.remote` entry with `listen` + `instances` | `rutis-host/listen`, many, one instance per connection in the `instances` group |
| link row `rutis-bridge/peer` with `dial` | `rutis-host/dial`, with `export`, `host`, `rows` |
| link row `rutis-bridge/peer` with `listen` | `rutis-host/listen`, one, with `export`, `host`, `rows` |

- For a row `<runtime name>:<plugin>` (and a node's `peer:<peer>/<plugin>`), the loader finds the session of that name and calls `run`. In an instance of an `instanced` group it finds the session in that instance. Local and remote, one and many go through one resolver.
- The services a remote plugin provides in an instance are visible in that instance only, by the existing instance service rules ([Instance services](design-instance-services-2026-10-08.en.md)).
- The row name `rutis-bridge/peer` stays: it is how node configurations are written, and renaming it would only make every node configuration change.
- A Rust program embedding rutis-host that needs its own way of connecting registers a connection plugin through `Embedding.builtins` and writes its row in rutis.json; `Embedding.transports` and `Embedding.identity` are gone.

## 7. What developers see

Following [Diagnostics](design-diagnostics-2026-10-26.en.md) §4.5: output never says session, `rutis-bridge/session` or internal ids; rows go by row name, runtimes by their name, remote plugins as `<id>:<plugin>` (the plugin name the developer wrote plus the id the far end gave).

**Input**

- rutis.json: as after #791 (`dial` / `listen` / `instances` in `runtimes.remote`; `dial` / `listen` / `export` / `host` / `rows` on link rows). The fields removed are in §8.
- Rust: `Session::new(channel)`, `admit`, `export`, `host`, `SessionHandle::{id, run}`; `websocket::{dial, listen}`, `pipe::spawn`, `memory::pair`.
- Other languages: `open` on the `sessions` service.

**What `run` prints** (lines from #791 unchanged; the two new ones marked)

```text
runtime gpu: link connected
runtime gpu: link down (gpu.example.com:7443: Connection refused); retrying in 2.1s
runtime gpu: link refused: auth-rejected (gpu.example.com: upgrade refused with 403 Forbidden); retrying in 30s
runtime worker: connected worker-1
runtime worker: disconnected worker-1 (connection lost)
runtime worker: disconnected worker-1 (replaced by a new connection)        ← new
runtime worker: refused py (py is runtime py of this host)                   ← new
office: link connected
```

**`rutis-host status`**: the `connections:` section as before (one: the link's state; many: the ids connected), each connected id followed by the remote plugins it registered:

```text
connections:
  gpu: link connected (gpu:embedder)
  worker: worker-1 (worker-1:chunker), worker-2 (worker-2:chunker)
  office: link connected (office:clock, office:weather)
```

**Errors**

| Case | Text |
| --- | --- |
| The far end lacks a row's plugin | `embedder: unresolved: entry "embedder" rejected: runtime gpu: this connection does not contain plugin embedder` (#768, unchanged) |
| A service the far end `export`s is provided here already | `office:weather: weather is provided here by row weather: not registered from office` |
| A link row has `import` | `office: config: import was removed in 0.19: what office exports is registered here; to keep it inside this row, use isolate` |
| A link row has `runtime` | `office: config: runtime was removed in 0.19: write the runtime in runtimes.remote with dial or listen` |
| A link row has `transport` or `identity` | `office: config: transport was removed in 0.19: a host's own way of connecting is a plugin of its own (docs/migration-0.19.md)` (the same for `identity`) |
| `runtimes.remote` has `transport` | nothing special: the field only existed on unreleased main, so it is an unknown field |
| The far end throws while running a plugin | the remote plugin `<id>:<plugin>` fails, printing the far end's error as in 0.18 §4.2 |

## 8. What is removed or made internal

| Name | Where | Why | Who is affected |
| --- | --- | --- | --- |
| `Transport`, `Dial`, `transport_key` | rutis-bridge `lib.rs` | carriers produce a `Channel` directly; no service looked up by kind is needed | Rust programs embedding rutis-bridge; bridge and loader tests; the docs promising "implement Transport" |
| `Embedding.transports` | rutis-host `host.rs` | as above; only on unreleased main (#791) | no released users; the e2e `fixture-host` |
| `runtimes.remote.transport` | rutis-host `config.rs` | as above; unreleased | none |
| `transport`, `identity` on link rows | rutis-host / loader `peer.rs` | they name the removed `Transport#<kind>`, `Identity#<name>` | rutis.json files using them (the `branch` row of persisted samples 0.10–0.18) |
| `Embedding.identity` | rutis-host `host.rs` | a host's own credentials go in its own connection plugin | Rust programs embedding rutis-host |
| `Identity`, `IdentityPlugin`, `identity_key`, `StaticIdentity`, `Credential`, `Presented` | rutis-bridge `identity.rs` | authentication is in the connection plugin: `websocket::dial` takes a token, `rutis-host/listen` compares against its list at upgrade | tests (many use `StaticIdentity`); embedders |
| `Admission`, `AdmissionPlugin`, `admission_key`, `SharedToken`, `Rejection` | rutis-bridge `identity.rs` | replaced by `admit` and listen's list | rutis-host; tests |
| `Revocations`, `Revocable` | rutis-bridge `identity.rs` | no production caller; existing connections are cut by stopping their row | the `revocation.rs` tests; two guarantee rows; the TLA model |
| `Registration`, `Registered`, `RegistrationError`, `Registrations`, `Ticket`, `Refusal`, `Deliver` | rutis-bridge `registration.rs` | dispatch by token moves into `rutis-host/listen` | WebSocket listening; `memory_mux.rs` |
| `AccessPointPlugin`, `AccessPointConfig`, `AccessEvent`, `Accept`, `Accepted`, `Release` | rutis-bridge `access_point.rs` | the callback-shaped handover; replaced by the session | rutis-host; loader `connections.rs`; tests |
| `LinkPlugin`, `LinkConfig`, `LinkState`, `Connect`, `Failure`, `Retry`, `protocol()` | rutis-bridge `link.rs` | dialling and reconnecting move to `rutis-host/dial`, listening to `rutis-host/listen`; `protocol()` has no caller | rutis-host; tests; `examples/smoke.rs` |
| `PeerPlugin`, `PeerHandle`, `Features` | rutis-bridge `compose.rs` | a composition plugin; its features become the session's `export` / `host`; `PeerPlugin::new` always returns `Ok` | rutis-host; loader `peer.rs`; tests |
| `Peer`, `peer_key`, `Handler`, `Offered`, `Offers`, `family()` | rutis-bridge `peer.rs` | merged into the session; `family()` has no caller | bridge internals; tests |
| `ImportPlugin` and the link row's `import` | rutis-bridge `services.rs`, loader `peer.rs` | decision 1 | node configurations; persisted samples; a hint in `check.rs` |
| `RuntimeSession`, `runtime_session_key`, `RuntimeAccessPlugin` | rutis-bridge `runtime/mod.rs`, `runtime/access.rs` | the service-shaped handover; replaced by `SessionHandle` | loader; tests |
| `RuntimePlugin::remote` | rutis-bridge `runtime/plugin.rs` | remote and local go through the same session; rows run with `run` | rutis-host; loader; tests |
| `LocalRuntime` | rutis-bridge `runtime/local.rs` | starting processes moves to `rutis-host/spawn` (on `pipe::spawn`) | rutis-host; tests |
| `RuntimeInstances`, `AcceptInstances`, `RuntimeConnection` | loader `runtime/connections.rs` | remote-only instance code; listen uses the loader's existing `create_instance` and the resolver is one | rutis-host; loader tests; rutis-migrate's name table |
| the remote part of `RuntimeState`, `LinkState`, `AccessEvent` (three states) | bridge, rutis-host `status.rs` | become "runtime state + whether the session is connected" | rutis-host's output code |
| `ChannelInfo`, `Closer::replaced`, `Closer::rejected` | rutis-bridge `channel/mod.rs` | §3 | carriers; tests |
| `ConnectError` (out of `channel`) | rutis-bridge `channel/mod.rs` | belongs to the dialling function | carriers and dial |
| `runtime` on link rows | loader `peer.rs`, rutis-host | from 0.19 written in `runtimes.remote`; the old form is removed, not deprecated | rutis.json files using it |
| `MemoryPlugin`, `MemoryTransport`, `LocalPlugin`, `WebSocketPlugin`, `WebSocketTransport` | `transport/*` | exist only to provide `Transport#<kind>`; `memory::pair`, `pipe::spawn`, `websocket::{dial, listen}` stay | tests; rutis-host |

The old forms are removed in 0.19 (which is the incompatible release for remote connections anyway), not deprecated.

## 9. Before and after

Line counts are `wc -l` on `3d19634f` (including in-file unit tests); "public items" is the number of lines starting with `pub fn|struct|enum|trait|type|const|use|mod` (including public methods inside impls). The after column is an **estimate**.

| File | Lines now | Public items now | Lines after (estimate) |
| --- | ---: | ---: | ---: |
| rutis-bridge `lib.rs` | 177 | 29 | 70 |
| `channel/mod.rs` | 165 | 13 | 130 |
| `access_point.rs` | 607 | 16 | 0 |
| `compose.rs` | 270 | 6 | 0 |
| `identity.rs` | 564 | 27 | 40 (only `fingerprint`) |
| `registration.rs` | 409 | 13 | 0 |
| `link.rs` | 751 | 15 | 0 |
| `peer.rs` | 382 | 14 | 0 |
| `services.rs` | 543 | 6 | 300 |
| `host.rs` | 359 | 10 | 300 |
| `runtime/access.rs` | 109 | 2 | 0 |
| `runtime/plugin.rs` | 667 | 23 | 350 |
| `runtime/local.rs` | 417 | 22 | 0 |
| `runtime/mod.rs` | 142 | 14 | 110 |
| `events/link.rs` | 795 | 2 | 700 |
| `transport/websocket/mod.rs` | 262 | 8 | 150 |
| `transport/memory/mod.rs` | 409 | 11 | 200 |
| `transport/local/mod.rs` | 206 | 13 | 150 |
| new `session/plugin.rs` (Session, SessionHandle, `sessions`, id table) | — | — | 600 |
| **rutis-bridge subtotal** | **7234** | **244** | **3100** |
| rutis-loader `runtime/connections.rs` | 550 | 10 | 0 |
| `runtime.rs` | 1621 | 14 | 1450 |
| `peer.rs` | 998 | 14 | 450 |
| `runtime/discover.rs` | 53 | 0 | 53 |
| **rutis-loader subtotal** | **3222** | **38** | **1953** |
| rutis-host `host.rs` | 2089 | 27 | 1800 |
| `config.rs` | 1434 | 42 | 1350 |
| `status.rs` | 979 | 18 | 850 |
| new `connect.rs` (spawn, dial, listen) | — | — | 700 |
| **rutis-host subtotal** | **4502** | **87** | **4700** |
| **Total** | **14958** | **369** | **about 9750** |

- Names exported at the rutis-bridge root: 59 now, about 13 after (`Channel`, `Close`, `PeerId`, `Session`, `SessionHandle`, `RowHandle`, `thrown`, `fingerprint` and the catalog types a node's `host` uses), estimated.
- Lifecycle paths go from 4 to 1, handover shapes from 2 to 1, admission, replacement and state each from 3 copies to 1.
- Tests: files testing only removed interfaces are deleted or rewritten, about 4000 lines (`access_point.rs` 584, `revocation.rs` 798, `link.rs` 547, `fault_links.rs` 959, `memory_mux.rs` 730, `runtime_instances.rs` 703, …); behaviour tests move to the new interfaces; estimated.

## 10. What happens to the merged PRs

**Conclusion: no revert; replaced inside the new implementation.** Why:

- The bulk of the mess (four paths, two shapes, three copies of admission and state) predates 0.19; reverting #768 and #791 only returns to the same state in 0.18 and solves nothing.
- #791's `runtimes.remote` syntax, its token checks and *Migrating to 0.19* stay in the new architecture; reverting and redoing them would cost CI, the migration guide and the guides one more round each.

| PR | Kept | Replaced |
| --- | --- | --- |
| #768 (R1 loader) | `discover.rs` (shared discovery); `Process::check_rows` (one check); "a missing plugin is always `Unavailable`" and its text; the cases of `remote_modes.rs` (run through the three connection plugins spawn / dial / listen instead) | the two call sites (`RuntimeResolver`'s remote branch, `RuntimeInstances`) become one resolver that looks up a session |
| #791 (R2 rutis-host) | `dial` / `listen` / `instances` in `runtimes.remote`; the token checks for one listener and for one-runtime `listen`; the `runtime <name>` status lines and `status`'s `connections:`; the `Report` parameter of `control::serve`; *Migrating to 0.19* (rewritten per §11); most cases of `remote_runtimes.rs` | the `transport` field and `Embedding.transports`; the connections `host.rs` builds from `PeerPlugin { runtime }`, `AccessPointPlugin`, `AdmissionPlugin`, `SharedToken`, replaced by the three connection plugins; "the old link-row syntax still works" becomes a start-up error; the case `a_link_row_still_connects_a_runtime` becomes an error case, `an_added_transport_carries_instances` becomes "a connection plugin an embedding program registers carries many runtimes" |
| #799 | — | already closed, not reopened: removal replaces deprecation. The guarantees §15 recount it carried (`2d817aa3`) is redone in the implementation PR that changes the guarantees |
| branch `claude/remote-drop-transport` (not pushed, `9a5543cc`) | its removal of the `transport` field, `Embedding.transports`, their tests and docs | folded into S4 of §13; not pushed on its own |

## 11. Compatibility and *Migrating to 0.19*

Unchanged: the session protocol; every language runtime and SDK (Kotlin and Swift apps change no code); rutis.json `format` stays 1; #791's `runtimes.remote` syntax; `peer:` rows; the existing output lines.

Incompatible (all written into the "Remote runtimes" section of *Migrating to 0.19*):

1. **`runtime` on link rows is removed**: write it in `runtimes.remote` (the existing table stays). "The old syntax keeps working, needs no change and is not converted" and "keep using link rows if you need those fields" are deleted and replaced by: `runtime` is a start-up error (§7's text); to use `export` or `host` as well, a link row and a `runtimes.remote` entry are two connections, or, without rows of a runtime, only the link row.
2. **`import` on link rows is removed** (decision 1): everything the far end `export`s registers; limit it with `isolate`; events go by the emitter's `export` only. With a rewritten example.
3. **`transport` and `identity` on link rows, and `Embedding.identity`, are removed**: a host's own carrier and credentials are a connection plugin (§5), registered through `Embedding.builtins`.
4. **Your own way of connecting**: "implement a `Transport` and add it to `Embedding.transports`" becomes "hand over a `Channel`: write a connection plugin that calls `Session::new(channel)`; in other languages use `sessions.open`". The `Embedding.transports` entry is deleted from the migration notes (it was never released).
5. **Rust API**: "the Rust APIs of rutis, rutis-loader and rutis-bridge do not change" becomes a table:

   | 0.18 | 0.19 |
   | --- | --- |
   | `LinkPlugin` + `PeerPlugin { runtime }` + `RuntimePlugin::remote` | `Session::new(websocket::dial(...)?)` in a connection plugin, rows `<name>:<plugin>` |
   | `AccessPointPlugin` + `RuntimeInstances` | the `rutis-host/listen` syntax, or a connection plugin of your own + an `instanced` group |
   | `ExportPlugin` / `ImportPlugin` | `Session::export`; nothing on the receiving side |
   | `HostPlugin` | `Session::host(catalog)` |
   | `Identity` / `Admission` / `Revocations` | the connection plugin's own authentication; `Session::admit` |
   | `Transport` | `websocket::dial` / `websocket::listen` / `pipe::spawn` give a `Channel` directly |
   | `Closer::replaced()` / `rejected(r)` | `close(Close::Replaced)` / `close(Close::Rejected(r))` |
   | `ConnectError` (`rutis_bridge::`) | `websocket::DialError` |

6. **Persisted samples**: the 0.8–0.18 samples contain `import`, `runtime`, `transport` and `identity`, which 0.19 no longer reads. Following the precedent of `events` in 0.13: those parts are taken out of the samples (expectations along), each form gets a `rejected/` case with §7's text, and the guarantee row for persisted samples states this 0.19 exception.
7. **New output**: the two lines marked "new" in §7; `status`'s `connections:` lists the remote plugins after each id.

## 12. Documents to change with it

| Document | Change |
| --- | --- |
| `docs/guide/nodes.md` / `.en.md` | drop `import` from the `export` / `import` table; the events section goes by the emitter only; delete the `runtime` field row; delete "neither: a link row provides the session"; "implement a `Transport`" becomes "write a connection plugin"; the revocation hook in "Security" becomes "existing connections are cut by stopping their row" |
| `docs/guide/rutis-host.md` / `.en.md` | `runtimes.remote` table without `transport` and the "neither" row; the start-up error list along; delete `Embedding`'s `transports` and `identity`; "implement Transport" becomes connection plugins |
| `docs/guide/rust-host.md` / `.en.md` | the `PeerPlugin`, `RuntimePlugin::remote` + link row example becomes a connection plugin + `Session::new` |
| `docs/guide/swift-app.md`, `kotlin-app.md` (both languages) | the link row with `runtime` in the listening example becomes `dial` in `runtimes.remote`; nothing else changes |
| `docs/guide/cordis.md`, `plugin-api.md`, `bun-plugin.md` and others mentioning `instances` / nodes | check the wording at each place; leave unchanged behaviour alone |
| `.agents/skills/rutis-remote-plugin/SKILL.md` and `references/nodes.md`, `rutis-json.md`, `plugin-api.md` | as the guides; "implement Transport and add it to Embedding.transports" becomes "hand over a Channel / write a connection plugin", with a minimal `sessions.open` example |
| `.agents/skills/rutis-new-runtime/references/plugin-api.md` | the section on making an existing runtime work remotely: nothing changes on the runtime side, and rutis no longer needs `RuntimePlugin::remote` |
| `docs/migration-0.19.md` / `.en.md` | §11 |
| `docs/guarantees.md` / `.en.md` | `bridge/revocation-closes-only-its-connections` and `bridge/revocation-refuses-reconnects` become "after the list changes, new connections are refused (403 at upgrade, or 4003 after the hello); existing connections are cut by stopping their row"; `host/env-tokens-not-revocable` along; `host/remote-runtimes` without `transport` and the old link row; `loader/runtime-connection-instances` rewritten as listen + an `instanced` group; `events/node-links`, `events/link-export-import`, `events/node-config` go by `export` only, with `import` refused in `node-config`; `loader/peer-rows` without `import`; in `bridge/dial-out-rules` the source of 4003 becomes `admit`; every row's test names follow the tests; §15 recounted |
| `specs/remote-lease/` (both READMEs, `AccessPoint.tla`, configs, `expected.txt`) | drop the revocation actions and the properties `RevokedStaysOut` and `RevocationScoped`, and the configs `access-revoke-same`, `access-revoke-distinct`, `access-mut-revoke-cuts-all`; `NoStaleHandOver` stays, worded "a stopped listener hands over no connection"; `access-mut-accept-before-release` stays; the "matching code" table points at `rutis-bridge/src/session/plugin.rs` (the id table) and `rutis-host/src/connect.rs` (listen); in `RemoteLease.tla` the revoked far end becomes "a stopped dial row", the rest unchanged |
| `crates/rutis-bridge/README.md`, the module docs in `lib.rs`, the root `README` | describe Channel, session and the carrier functions |
| the old design `design-remote-runtime-consistency-2026-10-26.md` / `.en.md` | a header line: "the implementation part is replaced by [Remote connections: one session plugin](design-remote-session-2026-10-28.en.md)" |

## 13. Implementation

Add first, delete after; after each PR merges, main works and CI passes.

| PR | Content | Done when | Depends on |
| --- | --- | --- | --- |
| S1 rutis-bridge: Channel and carrier functions | `Close`, `close(Close)`; remove `ChannelInfo`; `ConnectError` becomes `websocket::DialError`; add `websocket::dial`, `websocket::listen`, `pipe::spawn`, `memory::pair` (the existing `Transport` implementations call them) | existing tests pass with unchanged cases (only the `close` calls change); new unit tests: the three `Close`s on the three carriers (4002, 4003, stdin closed) | — |
| S2 rutis-bridge: the session plugin | `Session`, `SessionHandle::{id, run}`, `admit`, `export`, `host`; the id table and replacement; remote plugins `<id>:<plugin>`; the `sessions` service and `open`; the old links / access points untouched | tests: unload the old before accepting the new (the scenario of `access-mut-accept-before-release` and `an_id_revoked_while_its_connection_takes_over_is_not_accepted`, ported); the four endings (far end first / this end first / `admit` refuses / handshake fails); Node and Python test plugins run `run` over a memory Channel through `sessions.open` | S1 |
| S3 rutis-loader: one resolver | rows `<name>:<plugin>` and `peer:<peer>/<plugin>` look up a session by name and call `run`; in an instance, the instance's session; the old path stays alongside | `remote_modes.rs`'s cases run on sessions: spawn, dial, listen, listen + `instances` give word-for-word the same missing-plugin text; existing instance-service cases pass | S2 |
| S4 rutis-host: three connection plugins | `rutis-host/spawn`, `dial`, `listen`; rutis.json expands into them; state becomes two items, §7's output; remove the `transport` field, `Embedding.transports`, `Embedding.identity` (folding in the drop-transport branch); `runtime`, `import`, `transport`, `identity` on link rows are errors; persisted samples and `rejected/` cases | `remote_runtimes.rs`, `dial_in.rs`, `kotlin.rs` pass end to end on the new plugins; `dial_conformance` passes against `rutis-host/listen` for every language; the `persisted` test passes; the e2e `fixture-host` builds; #702's token-check cases pass | S3 |
| S5 removal | remove everything in §8 and the tests that test only it; rewrite the TLA model and run `specs/remote-lease/run.sh` | `grep` for the §8 names under `crates/` finds nothing; `cargo test --workspace` and clippy pass; TLA `expected.txt` updated and passing | S4 |
| S6 documents | everything in §12; *Migrating to 0.19* per §11 | both languages say the same; the docs no longer promise `Transport`, `Embedding.transports` or `Revocations`; every test name in the guarantees exists | S5 (behaviour visible with S4 can be documented in the guides with S4) |
