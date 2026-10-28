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
| remote plugin `<id>:<plugin name>` | a child of the session, registered automatically | one plugin of the far end, as it looks here; the only name remote plugins have |
| connection plugin | outside the core; anyone can write one | gets a `Channel` and mounts a session on it |
| └ `rutis-host/spawn`, `rutis-host/dial`, `rutis-host/listen` | shipped in rutis-host | start a local process, dial, listen; used for local runtimes, remote runtimes and nodes alike |

### 2.2 Structure

```text
group (optional; with isolate, it is what this connection can borrow)
 ├─ connection plugin (rutis-host/dial, rutis-host/listen, rutis-host/spawn, or anyone's)
 │   └─ rutis-bridge/session             one per Channel
 │       ├─ <id>:weather                 a far plugin, registered automatically
 │       └─ <id>:llm
 └─ this end's plugins on this connection (only meaningful for nodes, §2.3)
```

### 2.3 Relations

- **Connection plugin → session**: `ctx.plugin(Session::new(channel))`. The Channel closes, the session unloads; the session unloads, the Channel closes.
- **Session → remote plugins**: a far plugin is registered as the child `<id>:<plugin>` when it starts running on this connection and unloaded when it ends; when the session unloads they all unload with it. "Running on this connection" has two sources only:
  - this end has it run: a row `<id>:<plugin>`, for which the loader finds the session and calls `run` (the far end is a language runtime or an app), or creates a row with the far end's lent `loader` service (the far end is a rutis node, §2.6 decision 1);
  - the far end puts it there: when the far end is a rutis node, the other rows in the group its connection plugin is in (not counting `<id>:<plugin>` rows).
- **Borrowing services (lent on demand)**: what a far plugin declares in `inject`, the session borrows from the scope the connection plugin is in, as local runtimes do today. What can be borrowed is what is visible in that scope; to limit it, write `isolate` on the connection plugin's row (or on its group).
- **Events**: whatever names the far plugins listen to, the session sends them the events of those names emitted in the connection plugin's scope; what they emit appears on this end's bus, in their own scope. `isolate` does not affect events (an existing rule, [Plugin API](guide/plugin-api.en.md) "Scopes"); to limit them, use the listener's existing filters.
- **Remote plugins ↔ local plugins**: services a remote plugin provides appear as ordinary `provide`s and local rows `inject` them as usual; every lifecycle is the kernel's own.
- **Replacement**: only between sessions under the same connection plugin (§4.2).

### 2.4 API

Rust (connection plugins compiled into a host):

```rust
pub struct Channel { /* §3, with an optional id */ }

impl Session {
    pub fn new(channel: Channel) -> Self;
}
impl SessionHandle {
    /// The id the far end gave in its handshake (the same as the Channel's when it has one).
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
- A session `open` creates is mounted under the plugin that called it: when that plugin unloads, all its sessions unload. It carries no id; the hello decides (as for many devices).

These four names are all. Three options that were considered are done with what already exists:

| Need | Not added | Done with |
| --- | --- | --- |
| a connection with the wrong id must not replace a good connection with that id first; a device's id must not replace a local runtime | an `admit` callback | the Channel's optional `id` (the id the connection layer vouches for); replacement only under the same connection plugin (§3, §4.2) |
| limiting which services and events nodes give each other | an `export` list | the far end borrows by `inject`, within the connection plugin's scope; limited with `isolate` (2.3) |
| letting the far node start plugins here | `host(catalog)` and a `PluginCatalog` parameter | the far plugin (or the far session) borrows this end's `loader` service (the reserved-name service #185 defines); usable when visible in the scope, isolated with `isolate: ["loader"]` on the connection row when not wanted |

Why `run` stays: the far end may be a language runtime or an app rather than a rutis kernel, with no `loader` service to borrow; `run` is the step of the session protocol that has the far end run a plugin (`rows.v2`). When the far end is a rutis node, `run` is borrowing its `loader` to create a row.

### 2.5 The structure afterwards

```text
rutis-host
 ├─ rutis-host/spawn  py        ── pipe::spawn ─────────▶ Channel(id py)  ─▶ session py
 │                                                                         └─ py:coach … (run)
 ├─ rutis-host/dial   gpu       ── websocket::dial ────▶ Channel(id gpu) ─▶ session gpu
 │                                                                         └─ gpu:embedder (run)
 ├─ rutis-host/listen worker    ── websocket::listen ──▶ one Channel per connection (no id)
 │     ├─ session worker-1   └─ worker-1:chunker …
 │     └─ session worker-2   └─ worker-2:chunker …
 ├─ group office (isolate: ["loader"])
 │     ├─ rutis-host/dial office ─ websocket::dial ─▶ Channel(id office) ─▶ session office
 │     │                                                                   └─ office:clock (put there by the far end)
 │     └─ weather                this end's plugin on this connection; the far end sees main:weather
 └─ a TS plugin relay an agent wrote while running ── sessions.open(far) ──▶ session (id from the far end)
```

Local runtimes, remote runtimes, devices and nodes all have the same shape; there is no node-specific code.

### 2.6 Decisions on the two open items

**Decision 1: node links drop both `export` and `import`, and use scopes.**

- Rule: whatever a far plugin `inject`s is borrowed, from what is visible in the scope of this end's connection plugin; whatever it listens to is sent. To limit, write `isolate` on the connection row or its group. Which plugins run on the connection: 2.3.
- Basis: local runtimes have always worked this way (borrow by `inject`, scope sets the range); with nodes the same, nodes need no code of their own.
- Every use in [Connecting nodes](guide/nodes.en.md) and the tests, checked:

  | Use today | Where | Becomes |
  | --- | --- | --- |
  | `export: ["weather"]` + `import: ["weather"]` at the peer: the peer's local rows use this node's `weather` | nodes.md "What runs on a link"; `node.rs`, `websocket_soak.rs`, `cordis_node.rs` | the peer puts the rows using `weather` into its connection group: they run on this connection and borrow `weather` from this node by `inject`. Or this node puts the row providing `weather` into its own connection group, and the peer sees `main:weather`, whose service is visible everywhere there (the peer limits it with `isolate`) |
  | `import` refused when it collides with a local service | nodes.md | the borrowing rule: what is in the local scope is used locally; a same-named service provided by a plugin the far end put there follows the kernel's existing rule (the later registration is refused), and that remote plugin fails, saying why (§7) |
  | events `export: ["session/*"]` + `import: ["session/*"]` at the peer | nodes.md "Events between nodes"; `events_nodes.rs` | the peer puts the rows listening to `session/…` into its connection group and they get this node's `session/…`; to take only some, use the listener's filters (by name, or by the scope in the descriptor) |
  | `import: ["t/s/*"]` making the listening end announce only some listeners | `events_nodes.rs` | only the listeners of the plugins in the connection group are announced; listeners outside the group never are |
  | Multi-hop: a imports b's `clock` and exports it to c | `multihop.rs`, `websocket_multihop.rs`, `fault_links.rs` | rows in c's connection group `inject: ["clock"]` and borrow it from a; a's scope has `b:clock` (put there by a's connection to b), so it can be borrowed. Events still go one hop only: c's listeners are not on the connection between a and b |
  | `host: true` + `rows: true` at the peer + a row `peer:office/scanner` | nodes.md "Running plugins on another node"; `peer_rows.rs`, `node.rs` | a row `office:scanner`: this end's session borrows office's `loader` to create the row. office isolates `loader` on its connection row if it does not want that (borrowable by default, matching #185 L3, "any plugin may inject loader by default") |

- One existing use is lost. Today any local row can use a service the peer `export`s (after `import` it is a global service here); afterwards only the rows in the connection group (running on this connection) can borrow it, and a row outside the group that needs it goes into the group, or the far end puts the provider there (seen here as `<id>:<plugin>`, whose service is visible everywhere). This is no new mechanism: "on which connection" is written as a group instead of a list.
- The protocol does not change: borrowing services and announcing listeners use the existing frames (as runtimes always have).

**Decision 2: a plugin's configuration belongs to whoever starts it.**

- Plugins the far end puts on the connection itself (rows in a node's connection group) are configured there.
- When this end decides what the far end runs and with what configuration, it writes a row `<id>:<plugin>` with `config`: for a runtime or app far end the loader calls `run`, for a node it borrows the far `loader`. Either way the configuration is this end's.
- How apps get configuration today ([App integration](design-app-integration-2026-10-14.en.md) D2, D6; Kotlin and Swift `RutisApp`): the app only reports the plugins it carries (`plugins` in the `mount` reply); which one loads and with what configuration comes from the rows of an `instanced` group in the rutis configuration, and the SDK `apply`s only when a `rows.v2` start request arrives. That is "this end starts it", i.e. `run`.
- Migration: **apps and SDKs do not change.** A `phone:health` row in the `listen` + `instances` group becomes one `run("health", config)` in each app connection's instance; the local rows using it (`py:coach`) stay in the same instance. Each device's configuration is still the group's one, as today; per-device differences are the plugin reading the device's own settings (D7).
- Not done: apps starting plugins themselves and reporting them to rutis (no user).

## 3. Channel

```rust
pub struct Channel {
    pub sender: Box<dyn Sender>,
    pub receiver: Box<dyn Receiver>,
    pub closer: Arc<dyn Closer>,
    /// The far end's id as the connection layer vouches for it; None: the hello decides.
    pub id: Option<PeerId>,
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
- `id`: who fills it in.

  | Connection plugin | `id` |
  | --- | --- |
  | `rutis-host/spawn` | the name of the process it started (the runtime name) |
  | `rutis-host/dial` | the configured name |
  | `rutis-host/listen`, one | the name of the entry the token belongs to |
  | `rutis-host/listen`, many | none; the hello decides |
  | `sessions.open` | none |

- `ChannelInfo` goes: the far end's id comes from `id` above and the handshake; `transport` and `label` were for diagnostics only, which the connection plugin's own output now gives.
- `Closer::close(&str)`, `replaced()` and `rejected(&str)` become `close(Close)`. What each carrier does:

  | `Close` | WebSocket | local pipe (child process) | memory |
  | --- | --- | --- | --- |
  | `Done` | close code 1000 (1001 when the host stops) | closes the child's stdin | the other end's `recv` gets `Ok(None)` |
  | `Replaced` | close code 4002 | as `Done` | the other end's `recv` gets `Closed` |
  | `Rejected(r)` | close code 4003, reason `r` | as `Done` | the other end's `recv` gets `Rejected` |

- `ChannelError { Closed, Rejected }` stays: `Rejected` means the far end refused this end with 4003.
- `ConnectError` leaves the core and becomes `websocket::DialError`, the error of `websocket::dial`; `rutis-host/dial` backs off by it (retryable 0.5 → 30 s, auth rejected every 30 s, incompatible stops; rules unchanged).
- Once given to `Session::new`, only the session closes the Channel; before that, the connection plugin may close it itself (a wrong token at upgrade, say).

## 4. Lifecycle

### 4.1 Channel and session

One to one, born and gone together, never reused.

| What happened | Who knows first | Result |
| --- | --- | --- |
| Handshake done | session | checks the id as in 4.2; if it passes, replaces, then starts registering remote plugins |
| The check fails | session | `close(Rejected(reason))`; nothing is unloaded, nothing registered |
| Handshake fails (protocol version, missing capability, 10 s timeout) | session | the far end first gets the protocol's own error frame, then `close(Done)`; the session fails with that reason |
| The far end leaves first (`recv` returns `Ok(None)` or an error) | session | the session unloads: remote plugins unload in reverse, calls in flight fail once |
| This end leaves first (connection plugin unloads, row removed, host stops) | kernel | the session unloads: remote plugins unload in reverse, then `close(Done)` |
| Reconnect | connection plugin | a new Channel, a new session; the old session is already gone |

A connection plugin waits for the session to unload through the kernel handle `ctx.plugin(...)` returns (`FiberView::watch()`), then decides whether to reconnect (dial backs off and reconnects; spawn follows the runtime's `restart`; listen does nothing). The session never reconnects.

### 4.2 Checking the id, and replacement

The session core keeps an id table (one per host) recording which connection plugin each id belongs to: from that connection plugin's first session until that connection plugin unloads. `<id>:<plugin>` is unique in the whole host.

Once the handshake is done, in order:

1. The Channel has an `id` and the hello's id differs: `close(Rejected("expected <id>, the far end says <hello id>"))`.
2. The id belongs to another connection plugin: `close(Rejected("<id> is taken by <that connection plugin's row>"))`.
3. The id belongs to the same connection plugin and a session is connected: the old session unloads (remote plugins in reverse, every cleanup finished), and the old Channel is `close(Replaced)`.
4. The new session enters the table and starts registering remote plugins.

- Both refusals (1, 2) come before anything old is unloaded, so a refused connection never replaces a good one first; an accepted connection starts only once the old one is unloaded (3 before 4). These are the order the TLA mutation `access-mut-accept-before-release` guards: it moves "accept the new connection" before "release the old one" and the model finds a counterexample; the new implementation's 3 → 4 maps one to one onto `TakeSlot` → `Accept` in the model, and 1, 2 onto the admission check the model makes before `TakeSlot`.
- A device saying `py`: `py` belongs to the `rutis-host/spawn` row, step 2 refuses it, and the local runtime is unaffected.
- Replacements of one id run one after another; of two connections arriving together, the one whose handshake finishes later replaces the earlier.
- Boundary: an id enters the table with its first session. At start-up, `rutis-host/listen` rows accepting many ids start accepting after the spawn rows' first sessions (local processes start fast); if a dial row's far end stays unreachable, a device can take that name first, and the dial row is then refused (`refused (gpu is taken by worker)`). Device ids are given by the developer in the app, and as today ("two installs with the same id keep replacing each other") keeping them distinct is the developer's job.
- Two guarantees stay: close codes 4002 (replaced) and 4003 (rejected), and "unload the old before accepting the new". The test `an_id_revoked_while_its_connection_takes_over_is_not_accepted` becomes `a_connection_whose_listener_stopped_while_it_takes_over_is_not_accepted` (see §12).

### 4.3 Remote plugins on the kernel's lifecycle

| Far end | The local remote plugin `<id>:<plugin>` |
| --- | --- |
| The plugin starts running on this connection (`run` succeeded, a row created with the borrowed `loader` runs, a row of the far group runs) | registered as a child of the session, its `apply` done; the services it provides appear as ordinary `provide`s |
| Running fails (the far end throws) | this child fails, with the far end's name, message, `code` and `site` (0.18 §4.2) |
| The far end ended it | the child unloads; rows using its services stop or wait by the kernel's rules |
| The connection drops | the session unloads, and every child with it; the services they borrowed here are returned |

- There is no separate "connection state": connected means the session exists, disconnected means it does not. A runtime's state (whether the local process is up) belongs to the connection plugin; "runtime state + whether the session is connected" replaces `LinkState`, `AccessEvent` and `RuntimeState`.
- A row in rutis.json that runs at the far end (`gpu:embedder`) waits while the far end is away ("waiting for runtime gpu"); once a session appears the loader calls `run`; a far end without the plugin makes the row `Unavailable` (#768's text, unchanged).
- Rows this end creates on a far node with the borrowed `loader` hang under the far end's session for this connection, are not written to the far end's configuration file, and unload when the session ends (as rows hosted by `host` do today). This needs #185's `loader.create` to accept the caller's scope as the parent and to keep rows made that way out of the editable layer (as copies in `instanced` groups are never written back); implemented with S3.

## 5. Extending

There is one extension point: write a connection plugin, get a `Channel`, mount a session.

### 5.1 A Rust connection plugin

```rust
// Reach a runtime through a relay
async fn apply(ctx: &Ctx, config: Relay) -> Result<(), Error> {
    loop {
        let mut channel = relay::connect(&config.address, &config.token).await?;  // its own auth and carrier
        channel.id = Some(PeerId::new(&config.name)?);   // the relay vouches for who is there
        let session = ctx.plugin(Session::new(channel));
        disposed(session.watch()).await;                 // the kernel's FiberView::watch(): wait until the session unloads
        sleep(backoff.next()).await;
    }
}
```

To limit what the far end can borrow, write `isolate` on this connection plugin's row; the group it is in is what the connection can borrow.

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
- Underneath it is the same implementation as `Session::new`: `sessions.open` makes a `Sender` / `Closer` from `far` and a `Receiver` from `near`'s queue, with no `id`.
- Cost: every message of such a connection plugin passes once more through the runtime process it runs in.

### 5.3 Loading at run time

Connection plugins load as any plugin does: a row in rutis.json, the loader's `create`, the `loader` service of #185. A TS / Python connection plugin an agent writes while running is loaded this way, and the sessions `sessions.open` creates hang under it; unloading it unloads its sessions and every remote plugin below them.

## 6. The shipped connection plugins, and how rutis.json expands

The three plugins live in rutis-host (not in rutis-bridge), on plain functions in rutis-bridge: `websocket::dial`, `websocket::listen`, `pipe::spawn` (and `memory::pair` for tests).

| Plugin | Does | The Channel's `id` |
| --- | --- | --- |
| `rutis-host/spawn` | `pipe::spawn` starts a local runtime process and mounts a session; restarts it by the runtime's `restart` when it exits | the runtime name |
| `rutis-host/dial` | `websocket::dial`, mounts a session; after the session unloads, backs off by the `DialError` category and reconnects | the configured name |
| `rutis-host/listen` | `websocket::listen`: compares the token against the current list at upgrade (401 / 403 unchanged); mounts a session per Channel; with `instances`, first creates an instance named after the id in that `instanced` group with the loader's existing `create_instance`, mounts the session in it, and closes the instance when the session unloads | one: the name of the entry the token belongs to; many: none |

Dispatching the rows on one listener by token (what `Registrations` does today) belongs to `rutis-host/listen`; a token collision on one listener is still a start-up error (#702). All three plugins' configuration schemas have `additionalProperties: false` ([Config schema](design-config-schema-2026-10-27.en.md) C3).

rutis.json:

| Written as | Expands to |
| --- | --- |
| `runtimes.py` (and `node`, `bun`, `go`, `swift`, `kotlin`, the local runtimes) | `rutis-host/spawn`, `id` the runtime name |
| a `runtimes.remote` entry with `dial` | `rutis-host/dial`, `id` the runtime name |
| a `runtimes.remote` entry with `listen` | `rutis-host/listen`, one |
| a `runtimes.remote` entry with `listen` + `instances` | `rutis-host/listen`, many, one instance per connection in the `instances` group |
| a node | not expanded: write a `rutis-host/dial` or `rutis-host/listen` row directly; to put this end's plugins on the connection, or to use `isolate`, write that row and those rows into one group |

A node:

```json
{ "id": "office", "group": true, "isolate": ["loader"], "config": [
  { "id": "link", "name": "rutis-host/dial", "config": { "name": "office", "address": "wss://office.example.com/rutis" } },
  { "id": "weather", "name": "weather" },
  { "id": "scanner", "name": "office:scanner", "config": { "dpi": 300 } }
] }
```

- `weather` is this end's plugin on this connection, which office sees as `main:weather`; `office:scanner` borrows office's `loader` and runs there (when office does not isolate `loader`); `isolate: ["loader"]` keeps office from borrowing this end's `loader`.
- For a row `<id>:<plugin>` the loader finds the session of that name: for a runtime or app far end it calls `run`, for a node it borrows `loader`. In an instance of an `instanced` group it finds the session in that instance. Local and remote, one and many, runtimes and nodes go through one resolver.
- The services a remote plugin provides in an instance are visible in that instance only, by the existing instance service rules ([Instance services](design-instance-services-2026-10-08.en.md)).
- A Rust program embedding rutis-host that needs its own way of connecting registers a connection plugin through `Embedding.builtins` and writes its row in rutis.json.

## 7. What developers see

Following [Diagnostics](design-diagnostics-2026-10-26.en.md) §4.5: output never says session, `rutis-bridge/session` or internal ids; rows go by row name, runtimes by their name, remote plugins as `<id>:<plugin>`.

**Input**

- rutis.json: `dial` / `listen` / `instances` in `runtimes.remote` (#791); a node is a `rutis-host/dial` or `rutis-host/listen` row, in a group with the rows to put on the connection; rows running at the far end are `<id>:<plugin>`.
- Rust: `Session::new(channel)`, `SessionHandle::{id, run}`, the Channel's `id`; `websocket::{dial, listen}`, `pipe::spawn`, `memory::pair`.
- Other languages: `open(far)` on the `sessions` service.

**What `run` prints** (lines from #791 unchanged; new ones marked)

```text
runtime gpu: link connected
runtime gpu: link down (gpu.example.com:7443: Connection refused); retrying in 2.1s
runtime gpu: link refused: auth-rejected (gpu.example.com: upgrade refused with 403 Forbidden); retrying in 30s
runtime worker: connected worker-1
runtime worker: disconnected worker-1 (connection lost)
runtime worker: disconnected worker-1 (replaced by a new connection)            ← new
runtime worker: refused py (py is taken by runtime py)                          ← new
runtime gpu: refused (expected gpu, the far end says gpu-2)                      ← new
link: link connected                                                             (a node row, prefixed with the row id)
```

**`rutis-host status`**: the `connections:` section as before, each connected id followed by the remote plugins it registered:

```text
connections:
  gpu: link connected (gpu:embedder)
  worker: worker-1 (worker-1:chunker), worker-2 (worker-2:chunker)
  link: link connected (office:clock, office:scanner)
```

**Errors**

| Case | Text |
| --- | --- |
| The far end lacks a row's plugin | `embedder: unresolved: entry "embedder" rejected: runtime gpu: this connection does not contain plugin embedder` (#768, unchanged) |
| A service of a plugin the far end put there is provided here already | `office:weather: weather is provided here by row weather: not registered from office` |
| The far end cannot borrow a service it needs (isolated, or not here) | the far row waits or fails by its own rules; for a runtime far end the text is today's for a local runtime that cannot borrow a service |
| Borrowing a node's `loader` that is isolated | `scanner: unresolved: entry "scanner" rejected: office does not lend loader here` |
| 0.18 remote syntax (`rutis-bridge/peer` rows, `import`, `runtime`, `transport`, `identity`) | no dedicated message: `rutis-bridge/peer` is an unknown plugin name, reported as 0.18 reports a row that does not resolve; extra fields in `runtimes.remote` are reported as unknown rutis-host configuration fields |
| The far end throws while running a plugin | the remote plugin `<id>:<plugin>` fails, printing the far end's error as in 0.18 §4.2 |

## 8. What is removed or made internal

The old forms are removed in 0.19, not deprecated, with no dedicated conversion or message.

| Name | Where | Why | Who is affected |
| --- | --- | --- | --- |
| `Transport`, `Dial`, `transport_key` | rutis-bridge `lib.rs` | carriers produce a `Channel` directly | Rust programs embedding rutis-bridge; tests; the docs promising "implement Transport" |
| `Embedding.transports`, `runtimes.remote.transport` | rutis-host | as above; only on unreleased main (#791) | no released users; the e2e `fixture-host` |
| `Embedding.identity` | rutis-host `host.rs` | a host's own credentials go in its own connection plugin | Rust programs embedding rutis-host |
| `Identity`, `IdentityPlugin`, `identity_key`, `StaticIdentity`, `Credential`, `Presented` | rutis-bridge `identity.rs` | authentication is in the connection plugin: `websocket::dial` takes a token, `rutis-host/listen` compares against its list at upgrade | tests; embedders |
| `Admission`, `AdmissionPlugin`, `admission_key`, `SharedToken`, `Rejection` | rutis-bridge `identity.rs` | replaced by the Channel's `id` and the check in §4.2 | rutis-host; tests |
| `Revocations`, `Revocable` | rutis-bridge `identity.rs` | no production caller; existing connections are cut by stopping their row | `revocation.rs`; two guarantee rows; the TLA model |
| `Registration`, `Registered`, `RegistrationError`, `Registrations`, `Ticket`, `Refusal`, `Deliver` | rutis-bridge `registration.rs` | dispatch by token moves into `rutis-host/listen` | WebSocket listening; `memory_mux.rs` |
| `AccessPointPlugin`, `AccessPointConfig`, `AccessEvent`, `Accept`, `Accepted`, `Release` | rutis-bridge `access_point.rs` | the callback-shaped handover | rutis-host; loader `connections.rs`; tests |
| `LinkPlugin`, `LinkConfig`, `LinkState`, `Connect`, `Failure`, `Retry`, `protocol()` | rutis-bridge `link.rs` | dialling and reconnecting move to `rutis-host/dial`, listening to `rutis-host/listen`; `protocol()` has no caller | rutis-host; tests; `examples/smoke.rs` |
| `PeerPlugin`, `PeerHandle`, `Features` | rutis-bridge `compose.rs` | a composition plugin; nodes have no features of their own any more; `PeerPlugin::new` always returns `Ok` | rutis-host; loader; tests |
| `Peer`, `peer_key`, `Handler`, `Offered`, `Offers`, `family()` | rutis-bridge `peer.rs` | merged into the session; `family()` has no caller | bridge internals; tests |
| `ExportPlugin`, `ImportPlugin`, and `export`, `import` | rutis-bridge `services.rs`, the list part of `events/link.rs` | decision 1: borrow by `inject`, scope sets the range | node configurations; tests; a hint in `check.rs` |
| `HostPlugin`, `PluginCatalog`, `StaticCatalog`, `Described`, `Installed`, `ServiceKeys`, and `host: true`, `rows: true` | rutis-bridge `host.rs` | borrow the far end's `loader` service | node configurations; `peer_rows.rs`, `node.rs` |
| `peer:<peer>/<plugin>` rows, the row name `rutis-bridge/peer`, `PeerRows`, `PeerResolver`, `PeerRowsPlugin`, `LoaderCatalog`, `register_peer_node`, `node_schema` | rutis-loader `peer.rs` | remote plugins have one name only, `<id>:<plugin>`; a node is a connection plugin's row | node configurations; persisted samples |
| `runtime`, `transport`, `identity` on link rows | rutis-loader `peer.rs` | go with the `rutis-bridge/peer` row | rutis.json files using them |
| `RuntimeSession`, `runtime_session_key`, `RuntimeAccessPlugin` | rutis-bridge `runtime/` | the service-shaped handover; replaced by `SessionHandle` | loader; tests |
| `RuntimePlugin::remote` | rutis-bridge `runtime/plugin.rs` | remote and local go through the same session | rutis-host; loader; tests |
| `LocalRuntime` | rutis-bridge `runtime/local.rs` | starting processes moves to `rutis-host/spawn` | rutis-host; tests |
| `RuntimeInstances`, `AcceptInstances`, `RuntimeConnection` | loader `runtime/connections.rs` | remote-only instance code; listen uses the loader's existing `create_instance` and the resolver is one | rutis-host; loader tests; rutis-migrate's name table |
| `LinkState`, `AccessEvent`, the remote part of `RuntimeState` | bridge, rutis-host `status.rs` | become "runtime state + whether the session is connected" | rutis-host's output code |
| `ChannelInfo`, `Closer::replaced`, `Closer::rejected` | rutis-bridge `channel/mod.rs` | §3 | carriers; tests |
| `ConnectError` (out of `channel`) | rutis-bridge `channel/mod.rs` | belongs to the dialling function | carriers and dial |
| `MemoryPlugin`, `MemoryTransport`, `LocalPlugin`, `WebSocketPlugin`, `WebSocketTransport` | `transport/*` | exist only to provide `Transport#<kind>`; `memory::pair`, `pipe::spawn`, `websocket::{dial, listen}` stay | tests; rutis-host |

## 9. Before and after

Line counts are `wc -l` on `3d19634f` (including in-file unit tests); "public items" is the number of lines starting with `pub fn|struct|enum|trait|type|const|use|mod` (including public methods inside impls). The after column is an **estimate**.

| File | Lines now | Public items now | Lines after (estimate) |
| --- | ---: | ---: | ---: |
| rutis-bridge `lib.rs` | 177 | 29 | 50 |
| `channel/mod.rs` | 165 | 13 | 130 |
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
| `events/link.rs` | 795 | 2 | 450 |
| `transport/websocket/mod.rs` | 262 | 8 | 150 |
| `transport/memory/mod.rs` | 409 | 11 | 200 |
| `transport/local/mod.rs` | 206 | 13 | 150 |
| new `session/plugin.rs` (Session, SessionHandle, `sessions`, id table, borrowing by inject, plugins on the connection) | — | — | 650 |
| **rutis-bridge subtotal** | **7234** | **244** | **2280** |
| rutis-loader `runtime/connections.rs` | 550 | 10 | 0 |
| `runtime.rs` | 1621 | 14 | 1450 |
| `peer.rs` | 998 | 14 | 0 |
| `runtime/discover.rs` | 53 | 0 | 53 |
| **rutis-loader subtotal** | **3222** | **38** | **1503** |
| rutis-host `host.rs` | 2089 | 27 | 1750 |
| `config.rs` | 1434 | 42 | 1250 |
| `status.rs` | 979 | 18 | 800 |
| new `connect.rs` (spawn, dial, listen) | — | — | 700 |
| **rutis-host subtotal** | **4502** | **87** | **4500** |
| **Total** | **14958** | **369** | **about 8280** |

- Names exported at the rutis-bridge root: 59 now, about 7 after (`Channel`, `Close`, `PeerId`, `Session`, `SessionHandle`, `RowHandle`, `thrown`), estimated; `fingerprint` stays in the `websocket` module.
- The session's public API: four names, `Session::new`, `SessionHandle::id`, `SessionHandle::run`, `sessions.open`.
- Lifecycle paths go from 4 to 1, handover shapes from 2 to 1, admission, replacement and state each from 3 copies to 1; nodes have no code of their own.
- Tests: files testing only removed interfaces are deleted or rewritten, about 5000 lines (`access_point.rs` 584, `revocation.rs` 798, `link.rs` 547, `fault_links.rs` 959, `memory_mux.rs` 730, `runtime_instances.rs` 703, `peer_rows.rs` 556, most of `node.rs` 1011, …); behaviour tests move to the new interfaces; estimated.

## 10. What happens to the merged PRs

**Conclusion: no revert; replaced inside the new implementation.** Why:

- The bulk of the mess (four paths, two shapes, three copies of admission and state) predates 0.19; reverting #768 and #791 only returns to the same state in 0.18 and solves nothing.
- #791's `runtimes.remote` syntax, its token checks and *Migrating to 0.19* stay in the new architecture; reverting and redoing them would cost CI, the migration guide and the guides one more round each.

| PR | Kept | Replaced |
| --- | --- | --- |
| #768 (R1 loader) | `discover.rs` (shared discovery); `Process::check_rows` (one check); "a missing plugin is always `Unavailable`" and its text; the cases of `remote_modes.rs` (run through spawn / dial / listen instead) | the two call sites (`RuntimeResolver`'s remote branch, `RuntimeInstances`) become one resolver that looks up a session |
| #791 (R2 rutis-host) | `dial` / `listen` / `instances` in `runtimes.remote`; the token checks for one listener and for one-runtime `listen`; the `runtime <name>` status lines and `status`'s `connections:`; the `Report` parameter of `control::serve`; *Migrating to 0.19* (rewritten per §11); most cases of `remote_runtimes.rs` | the `transport` field and `Embedding.transports`; the connections `host.rs` builds from `PeerPlugin { runtime }`, `AccessPointPlugin`, `AdmissionPlugin`, `SharedToken`, replaced by the three connection plugins; "the old link-row syntax still works" is deleted; the case `a_link_row_still_connects_a_runtime` is deleted, `an_added_transport_carries_instances` becomes "a connection plugin an embedding program registers carries many runtimes" |
| #799 | — | already closed, not reopened. The guarantees §15 recount it carried (`2d817aa3`) is redone in the implementation PR that changes the guarantees |
| branch `claude/remote-drop-transport` (not pushed, `9a5543cc`) | its removal of the `transport` field, `Embedding.transports`, their tests and docs | folded into S4 of §13; not pushed on its own |

## 11. Compatibility and *Migrating to 0.19*

No compatibility. Unchanged are only: the session protocol; every language runtime and SDK (Kotlin and Swift apps change no code); rutis.json `format` stays 1; #791's `runtimes.remote` syntax.

The "Remote runtimes" section of *Migrating to 0.19* is rewritten to give only the new syntax and the mapping:

1. **Remote runtimes**: one `runtimes.remote` entry (#791's table stays). Delete "the old syntax keeps working, needs no change and is not converted", "keep using link rows if you need those fields", the error for writing both, and the `transport` item.
2. **Nodes**: a mapping table.

   | 0.18 | 0.19 |
   | --- | --- |
   | `{ "name": "rutis-bridge/peer", "config": { "peer": "office", "dial": "<address>" } }` | `{ "name": "rutis-host/dial", "config": { "name": "office", "address": "<address>" } }` |
   | `"listen": "<listener>"` | `rutis-host/listen`, `"listener": "<listener>"` |
   | `export: [...]` | put the providing rows into the connection group (the far end sees `<this id>:<plugin>`), or write nothing and let the far end's rows in its group borrow by `inject` |
   | `import: [...]` | put the using rows into the connection group, borrowing by `inject`; limit with `isolate` on the group |
   | `export` / `import` of events | put the listening rows into the connection group; take only some with the listener's filters |
   | `host: true` | write nothing: the far end can borrow `loader` by default; `isolate: ["loader"]` to refuse |
   | `rows: true` + a row `peer:office/scanner` | a row `office:scanner` |
   | `runtime: "gpu"` | `dial` or `listen` in `runtimes.remote` |
   | `transport`, `identity` | a connection plugin of your own with your own carrier and credentials (§5), registered through `Embedding.builtins` |

3. **Your own way of connecting**: "implement a `Transport` and add it to `Embedding.transports`" becomes "hand over a `Channel`: write a connection plugin that calls `Session::new(channel)`; in other languages use `sessions.open`".
4. **Rust API**: "the Rust APIs of rutis, rutis-loader and rutis-bridge do not change" becomes a table:

   | 0.18 | 0.19 |
   | --- | --- |
   | `LinkPlugin` + `PeerPlugin { runtime }` + `RuntimePlugin::remote` | `Session::new(websocket::dial(...)?)` in a connection plugin, rows `<name>:<plugin>` |
   | `AccessPointPlugin` + `RuntimeInstances` | the `rutis-host/listen` syntax, or a connection plugin of your own + an `instanced` group |
   | `ExportPlugin` / `ImportPlugin` / `HostPlugin` | scopes and `isolate`; the far end borrows `loader` |
   | `Identity` / `Admission` / `Revocations` | the connection plugin's own authentication; the Channel's `id` |
   | `Transport` | `websocket::dial` / `websocket::listen` / `pipe::spawn` give a `Channel` directly |
   | `Closer::replaced()` / `rejected(r)` | `close(Close::Replaced)` / `close(Close::Rejected(r))` |
   | `ConnectError` (`rutis_bridge::`) | `websocket::DialError` |
   | `Embedding.identity` | a connection plugin, through `Embedding.builtins` |

5. **Persisted samples**: the 0.8–0.18 samples contain `rutis-bridge/peer` rows (with `import`, `runtime`, `transport`, `identity`) and `peer:` rows, which 0.19 no longer reads. Those parts are taken out of the samples (expectations along), `rejected/` cases are added whose errors are exactly what the 0.18 configuration schema says (an unknown plugin name, an unknown field), and the guarantee row for persisted samples states this 0.19 exception.
6. **New output**: the three lines marked "new" in §7; `status`'s `connections:` lists the remote plugins after each id.

## 12. Documents to change with it

| Document | Change |
| --- | --- |
| `docs/guide/nodes.md` / `.en.md` | rewrite "One link" and "What runs on a link": a node is a `rutis-host/dial` / `rutis-host/listen` row and a connection group; the `export` / `import` / `host` / `rows` / `runtime` table goes, replaced by "borrow by `inject`, scope sets the range, `isolate` limits"; "Running plugins on another node" becomes `office:scanner` and borrowing `loader`; "implement a `Transport`" becomes "write a connection plugin"; the revocation hook in "Security" becomes "existing connections are cut by stopping their row", and the `host: true` item becomes `isolate: ["loader"]` |
| `docs/guide/rutis-host.md` / `.en.md` | `runtimes.remote` table without `transport` and the "neither" row; the start-up error list along; delete `Embedding`'s `transports` and `identity`; the link-row section becomes connection plugin rows |
| `docs/guide/rust-host.md` / `.en.md` | the `PeerPlugin`, `RuntimePlugin::remote` example becomes a connection plugin + `Session::new` |
| `docs/guide/swift-app.md`, `kotlin-app.md` (both languages) | the `rutis-bridge/peer` row in the listening example becomes `dial` in `runtimes.remote`; nothing else changes |
| `docs/guide/cordis.md`, `plugin-api.md`, `bun-plugin.md` and others mentioning nodes or `peer:` | rewritten in the new syntax at each place |
| `.agents/skills/rutis-remote-plugin/SKILL.md` and `references/nodes.md`, `rutis-json.md`, `plugin-api.md` | as the guides; "implement Transport and add it to Embedding.transports" becomes "hand over a Channel / write a connection plugin", with a minimal `sessions.open` example |
| `.agents/skills/rutis-new-runtime/references/plugin-api.md` | the section on making an existing runtime work remotely: nothing changes on the runtime side, and rutis no longer needs `RuntimePlugin::remote` |
| `docs/migration-0.19.md` / `.en.md` | §11 |
| `docs/guarantees.md` / `.en.md` | `bridge/revocation-closes-only-its-connections` and `bridge/revocation-refuses-reconnects` become "after the list changes, new connections are refused (403 at upgrade, or 4003 after the hello); existing connections are cut by stopping their row"; `host/env-tokens-not-revocable` along; `host/remote-runtimes` without `transport` and the old link row; `loader/runtime-connection-instances` rewritten as listen + an `instanced` group; `loader/peer-rows` rewritten as `<id>:<plugin>` rows borrowing `loader`; `events/node-links`, `events/link-export-import`, `events/node-config` rewritten as plugins in a connection group sending and receiving by name, with `node-config` removed; `events/no-multi-hop` worded per the multi-hop row of §2.6; in `bridge/dial-out-rules` the source of 4003 becomes §4.2; a new row: the check and replacement order of §4.2; the persisted-sample row states the 0.19 exception; every row's test names follow the tests; §15 recounted |
| `specs/remote-lease/` (both READMEs, `AccessPoint.tla`, configs, `expected.txt`) | drop the revocation actions and `RevokedStaysOut`, `RevocationScoped`, and the configs `access-revoke-same`, `access-revoke-distinct`, `access-mut-revoke-cuts-all`; the admission check becomes steps 1 and 2 of §4.2 (the vouched-for id, taken by another connection plugin); `NoStaleHandOver` stays, worded "a stopped listener hands over no connection"; `access-mut-accept-before-release` stays; the "matching code" table points at `rutis-bridge/src/session/plugin.rs` (the id table) and `rutis-host/src/connect.rs` (listen); in `RemoteLease.tla` the revoked far end becomes "a stopped dial row" |
| `crates/rutis-bridge/README.md`, the module docs in `lib.rs`, the root `README` | describe Channel, session and the carrier functions |
| the old design `design-remote-runtime-consistency-2026-10-26.md` / `.en.md` | a header line: "the implementation part is replaced by [Remote connections: one session plugin](design-remote-session-2026-10-28.en.md)" |

## 13. Implementation

Add first, delete after; after each PR merges, main works and CI passes.

| PR | Content | Done when | Depends on |
| --- | --- | --- | --- |
| S1 rutis-bridge: Channel and carrier functions | `Close`, `close(Close)`, `Channel.id`; remove `ChannelInfo`; `ConnectError` becomes `websocket::DialError`; add `websocket::dial`, `websocket::listen`, `pipe::spawn`, `memory::pair` (the existing `Transport` implementations call them) | existing tests pass with unchanged cases (only the `close` calls change); new unit tests: the three `Close`s on the three carriers (4002, 4003, stdin closed) | — |
| S2 rutis-bridge: the session plugin | `Session::new`, `SessionHandle::{id, run}`; the id table, check and replacement (§4.2); remote plugins `<id>:<plugin>`; borrowing services by `inject` and sending events to far listeners (the runtime's existing code, extended to rutis-node far ends); plugins in the connection group registered at the far end; the `sessions` service and `open`; the old links / access points untouched | tests: each step of §4.2 (vouched id differs, taken by another connection plugin, same-plugin replacement unloading the old before accepting the new — the scenario of `access-mut-accept-before-release`, ported); the four endings; two rutis kernels over a memory Channel borrowing services from each other, exchanging events, limited by `isolate`; Node and Python test plugins run `run` through `sessions.open` | S1 |
| S3 rutis-loader: one resolver | rows `<id>:<plugin>` look up a session by name: `run` for runtimes, borrowed `loader` for nodes; in an instance, the instance's session; `loader.create` accepts the caller's scope as parent and keeps such rows out of the editable layer (§4.3); the old path stays alongside | `remote_modes.rs`'s cases run on sessions: spawn, dial, listen, listen + `instances` give word-for-word the same missing-plugin text; on a node, `office:scanner` runs through `loader`, unloads when the session ends, and the far configuration file is unchanged; existing instance-service cases pass | S2 |
| S4 rutis-host: three connection plugins | `rutis-host/spawn`, `dial`, `listen` (schemas with `additionalProperties: false`); `runtimes.*` and `runtimes.remote` expand into them; state becomes two items, §7's output; remove the `transport` field, `Embedding.transports`, `Embedding.identity` (folding in the drop-transport branch); persisted samples and `rejected/` cases | `remote_runtimes.rs`, `dial_in.rs`, `kotlin.rs` pass end to end on the new plugins; nodes end to end (two rutis-hosts: connection groups, borrowing, events, `office:scanner`); `dial_conformance` passes against `rutis-host/listen` for every language; the `persisted` test passes; the e2e `fixture-host` builds; #702's token-check cases pass | S3 |
| S5 removal | remove everything in §8 and the tests that test only it; rewrite the TLA model and run `specs/remote-lease/run.sh` | `grep` for the §8 names under `crates/` finds nothing; `cargo test --workspace` and clippy pass; TLA `expected.txt` updated and passing | S4 |
| S6 documents | everything in §12; *Migrating to 0.19* per §11 | both languages say the same; the docs no longer promise `Transport`, `Embedding.transports`, `Revocations`, `export` / `import` or `rutis-bridge/peer`; every test name in the guarantees exists | S5 (behaviour visible with S4 can go into the guides with S4) |
