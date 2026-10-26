# Remote connection layering: the remote plugin layer and connection plugins

[中文](design-remote-connection-layering-2026-10-26.md)

Status: final, not implemented. Date: 2026-10-26. Baseline: `main` `b4eee45e` (0.17.1). Scheduled: 0.19.
Issue: [#695](https://github.com/arcships/rutis/issues/695) (implementation [#696](https://github.com/arcships/rutis/issues/696)–[#699](https://github.com/arcships/rutis/issues/699)). Decision record: [remote connection layering](decision-remote-connection-layering-2026-10-26.en.md).
Supersedes: in [remote plugins](design-remote-plugins-2026-10-03.en.md), the parts about connecting and managing runtimes (the session source in §3.1, the connection direction in §4.1, §4.4, the remote language plugin row in §8); in [remote runtimes: dialing out and dynamic connections](design-remote-runtime-dial-2026-10-14.en.md), the rutis-side parts (the rutis side of §3, §4, the access point parts of §5). The rest of both documents remains valid; see §11.

Out of scope: how local runtimes (`LocalRuntime`, the Go / Swift binary runtimes) connect; node interconnection (`export` / `import` / `host` / `rows`); the session protocol; the SDKs on the side of each language runtime (the dial-out, listening and reconnection specifications do not change); the management service for identity and pairing.

## 1. Problem

The same remote language runtime behaves differently in rutis depending only on which side opens the connection:

| The same remote plugin | rutis connects to it (link row + `runtime`) | It connects in to rutis (access point) |
| --- | --- | --- |
| Who can use the services it provides | All rows | Only rows in the same group |
| The remote side lacks a plugin | The whole row fails | Fails only on that connection |
| Status output | Only the link row | Nothing is printed |
| Configuration | Two places: `runtimes.remote` + a `rutis-bridge/peer` row, not checked against each other | One place |
| `language` | Takes effect | Has no effect |
| After reconnecting | Only rows resolved while offline are resolved again | Each connection is described again |

There are two causes:

1. **The connection side does plugin management work.** After a connection arrives, the access point directly has the loader create an instance, and handles takeover by the same id; the link carries a `runtime` switch and requires the other side to declare itself a runtime.
2. **The plugin management side is written twice.** Runtimes rutis connects to go through `RuntimePlugin::remote` + `RuntimeResolver`; runtimes that connect in go through `RuntimeInstances`, bypassing `RuntimePlugin`, so discovery, readiness and feature checks are each written twice.

Consequences:

- The connection direction should only be a deployment choice (the dial-out design R1), but it now decides the visibility of services and how failures show.
- A host owner who wants to build their own connection (forwarding through a relay, using an external authentication service, switching to QUIC or Bluetooth) has no entry point: the transport and authentication can be replaced, but "who connects to whom, when to reconnect, how many connections" only has the two built-in forms.

## 2. Decisions

| # | Decision | Section |
| --- | --- | --- |
| C1 | Split into two layers: the **remote plugin layer** handles discovery, binding, lifecycle and handshake; **connection plugins** handle dialing out or listening, authentication, reconnection and the transport | 3 |
| C2 | The two layers hand over only "one connection": runtime name, connection id, message channel; handed over when connected, taken back when disconnected | 4 |
| C3 | There is only one remote plugin layer, shared by all connection directions and all connection plugins | 5 |
| C4 | One machine or many is decided by configuration (`instances`), independent of the connection direction | 5 |
| C5 | A new connection with the same id takes over the old one; the remote plugin layer decides this | 5 |
| C6 | The handshake and protocol checks belong to the remote plugin layer; connection plugins do not implement the rutis protocol, they only hand over the message channel | 4, 5 |
| C7 | Connections are transparent to plugin authors and to people writing configuration: configuration only names the runtime, the plugins and the address of a built-in connection; connection plugins are mounted by the host and do not appear in row configuration | 7 |
| C8 | Authentication belongs to connection plugins; the authentication layer of the built-in connections opens up credential types and allows waiting for an external service's answer | 6, 9 |
| C9 | The runtime side does not change: the session protocol, the `listen:` / `dial:` command lines, the reconnection specification, and the `dial_conformance` / `websocket_conformance` cases all stay the same | 10 |
| C10 | Old configuration is still accepted; it is converted at startup and the new form is shown | 10 |

## 3. Two layers

```text
Remote plugin layer (one shared implementation in rutis)
  Discovery: which plugins the other side carries   Binding: which rows run on this connection
  Lifecycle: load, stop, reload after reconnecting  Handshake: check protocol version and capabilities
        ▲
        │  Handover: one connection (runtime name, connection id, message channel; connected / disconnected)
        │
Connection plugins (replaceable, there can be several)
  Dial out / listen    Authentication    Reconnection    Transport (WebSocket, relay, QUIC, ...)
```

| Task | Owner | Where it is today |
| --- | --- | --- |
| Read the `mount` reply, check `rows.v2` / `hosts`, read the carried plugins | Remote plugin layer | One copy each in `RuntimePlugin` and `AcceptInstances` |
| Create instances per `instances`, or provide a global `Runtime` | Remote plugin layer | The access point's `Accept` callback |
| Same-id takeover | Remote plugin layer | Access point, listening link |
| Who the other side claims to be, and whether it is a runtime | Remote plugin layer (handshake) | The link's `require`, the access point's `require("runtime")` |
| When to dial, backoff, waiting after credentials are rejected | Connection plugin | link |
| Whether credentials are valid, revocation | Connection plugin | `Identity` / `Admission` |
| How messages are delivered | Connection plugin (using `Transport`) | `Transport` |

## 4. Handover

For each remote runtime the remote plugin layer provides a **connection registry** service, keyed by runtime name. Connection plugins depend on it and hand over each message channel they establish:

```rust
// rutis-bridge: connection plugins depend only on this
pub trait RuntimeConnections: Send + Sync + 'static {
    /// Hand over one connection; returns after the handshake completes.
    fn offer(&self, offer: Offer) -> BoxFuture<'_, Result<Connected, Refused>>;
}

pub struct Offer {
    pub channel: Channel,
    pub who: Who,
}

pub enum Who {
    /// The other side must greet with this id (rutis connects to a known runtime).
    Known(PeerId),
    /// The other side reports its own id; this admission rule decides whether to accept it and which connection id it counts as (waiting for it to connect in).
    Greeted(Arc<dyn Admission>),
}

/// One admitted connection. Dropping it = the connection plugin takes the connection back.
pub struct Connected { /* id(), ended() */ }

pub enum Refused {
    Incompatible(String), // protocol or capabilities do not match: do not retry
    Rejected(String),     // identity mismatch or admission refused: wait as for rejected credentials
    Conflict(String),     // single-connection mode already has a connection with another id: may retry
    Retryable(String),    // the other side did not greet, disconnected, etc.
}
```

- `Connected::ended()` gives the reason and category when the connection ends (same as `Refused`, plus "taken over by a new connection with the same id"); the connection plugin uses it to decide whether and when to reconnect.
- Before `offer` returns, the remote plugin layer has completed the handshake, feature checks and discovery; when `Connected` is returned, the rows on this connection have already started loading.
- Why a "registry" rather than "one service per connection": the number of connections changes and ids are reported by the other side; one service key per connection would let names chosen by the other side occupy the application's service keys (the dial-out design §4 already has this constraint), and would also need a mechanism to observe services appearing. The registry is provided by the remote plugin layer and connection plugins depend on it, which is an ordinary dependency: when the runtime declaration is removed, the registry is withdrawn, and the connection plugin stops and disconnects with it.
- Connection plugins do not read or write message contents and do not implement the rutis protocol. On refusal, the remote plugin layer tells the other side through the channel's `Closer` (4003 refused, 4002 taken over); what the runtime side sees is the same as today.

## 5. Remote plugin layer

One implementation, in rutis-loader (binding needs the loader's instances), with its interface defined in rutis-bridge (connection plugins do not depend on the loader). It merges today's `RuntimePlugin::remote` + the remote branch of `RuntimeResolver` with `RuntimeInstances`.

For each connection:

1. **Handshake**: check the other side's id per `Who`, require the other side to declare the `runtime` capability, check `rows.v2` and `hosts`. On mismatch, `Refused`.
2. **Discovery**: read `plugins` from the `mount` reply; when absent, ask `rows.schema` row by row. Read once per connection.
3. **Binding**, based on configuration only:

   | Runtime declaration | One connection arrives | A connection with a second id |
   | --- | --- | --- |
   | No `instances` | Provide `Runtime#<name>`; rows become ready in the existing two stages (`Runtime` → `RuntimeRows` → row); services are globally visible | `Refused::Conflict`, saying which connection already exists |
   | With `instances` | Create an instance in that `instanced` group under the connection id; rows run in the instance, and services are visible only inside the instance | Another instance |

4. **Lifecycle**:
   - Disconnect: withdraw `Runtime` or close the instance; users stop; the runtime side cleans up per the existing lease.
   - A new connection with the same id: first end the old connection (stop rows, release the instance; the old `ended()` reports "taken over"), then admit the new one.
   - Reconnect: discover again, and **all rows** of this runtime are resolved again against the new description, no longer only refreshing rows resolved while offline. After a remote plugin is upgraded, new configuration items are validated.
5. **The remote side lacks a plugin**: in both modes, that row is unavailable on that connection (`Unavailable`), saying "this connection does not contain plugin X", without affecting other rows or other connections.

One runtime can have several connection plugins handing over connections at the same time (for example the built-in listener plus a relay plugin): in multi-connection mode each one is an instance; in single-connection mode the first to arrive is admitted and the rest get `Conflict`.

## 6. Connection plugins

A connection plugin is an ordinary rutis plugin: it depends on `RuntimeConnections#<runtime name>` and on the transport and authentication it needs, calls `offer` after establishing a channel, and reconnects according to the category of `Refused` / `ended()`.

rutis has two built in, replacing today's link (runtime use) and access point:

| Built-in connection plugin | Replaces | Authentication | Reconnection |
| --- | --- | --- | --- |
| Dial out: rutis connects to the runtime | `rutis-bridge/peer` row + `runtime` | `Identity` (one token per runtime) | Keeps the link's backoff and categories (from 0.5 s, doubling up to 30 s, reset after 60 s stable; every 30 s when rejected; stop when incompatible) |
| Listen: wait for the runtime to connect in | `AccessPointPlugin` | `Admission` (shared token + self-reported id) | Does not reconnect on its own; waits for the other side to connect again |

- Both work on the shared listener's registrations (`Registrations`), with unchanged rules: on one listener, node links and dialing-out and listening runtimes do not interfere with each other.
- Revocation (#273) stays in the connection plugin: for a revoked id the connection plugin drops `Connected`, and later connections are refused as rejected credentials; the guarantee does not change.
- `LinkPlugin` and `PeerPlugin` for node interconnection stay as they are, only losing the `runtime` feature (`RuntimeAccessPlugin` is retired).

## 7. Configuration

One `runtimes.remote` entry fully describes one remote runtime; no extra link row is needed. The three fields are independent:

| Field | Meaning |
| --- | --- |
| `dial` | Built-in dial out: rutis connects to this address |
| `listen` | Built-in listen: wait on this listener for it to connect in |
| `instances` | Multi-connection mode: each connection is an instance of this `instanced` group |

At most one of `dial` and `listen` may be written; when neither is written, no built-in connection is started, and connections are handed over by connection plugins the host mounts.

```json
{
  "id": "main",
  "listen": [{ "name": "public", "address": "0.0.0.0:7443", "cert": "server.pem", "key": "server.key" }],
  "runtimes": { "remote": [
    { "name": "gpu",    "language": "python", "dial": "wss://gpu.example.com:7443/rutis" },
    { "name": "mac",    "language": "swift",  "listen": "public" },
    { "name": "worker", "language": "python", "listen": "public", "instances": "worker" }
  ] },
  "rows": [
    { "id": "embedder", "name": "gpu:embedder" },
    { "id": "calendar", "name": "mac:calendar" },
    { "id": "worker", "group": true, "instanced": true, "config": [
      { "id": "chunker", "name": "worker:chunker" }
    ] }
  ]
}
```

- `mac` is one device connecting in, with globally visible services. Today this needs an extra listening `rutis-bridge/peer` row (with `runtime`); the new form has only this one entry.
- Token rules do not change: `RUTIS_TOKEN_<name>`, falling back to `RUTIS_TOKEN` when absent. When two entries on the same listener would accept the same token, startup fails and says which one to give its own token (today this case is a 403 at run time).
- `language` takes effect in all modes (for example `node` resolves row names by npm name).
- Row configuration shows only the runtime name, not the connection. Changing the connection method (for example from `listen` to a relay plugin) does not change any row.

## 8. Output

`rutis-host run` prints for each remote runtime:

```text
runtime gpu: connected gpu
runtime gpu: link down (gpu.example.com:7443: Connection refused); retrying in 2.1s
runtime worker: connected worker-1
runtime worker: disconnected worker-1 (connection lost)
runtime mac: refused mac-2 (already connected: mac)
embedder: gpu does not contain plugin embedder
```

- `connected` / `disconnected` / `refused` and missing plugins are reported by the remote plugin layer, the same for all connection plugins, including connections that connect in (not printed today).
- `link …` lines are the retry status of the built-in connection plugins, in the same format as today, with the prefix changed from the link row id to `runtime <name>`. Connection plugins built by the host decide their own output.

## 9. Host-built connections

The host owner mounts a connection plugin in Rust and gives it to a runtime name:

```rust
let relay = RelayConnection::new("relay.example.com", token);   // a plugin the host writes itself
host.plugin(relay.for_runtime("gpu"));                          // depends on RuntimeConnections#gpu
```

- It only needs to produce an ordered, reliable channel that preserves message boundaries (the existing `Channel` contract); it does not need to know the rutis protocol.
- Authentication is opened up: `Presented` adds custom credentials (for example request headers); `Identity::verify` and `Admission::admit` become awaitable, so built-in connections can also ask an external authentication service.
- Contract tests: rutis-bridge provides a set of connection plugin contract tests (on the in-memory transport) for host-built plugins to test themselves against: handshake failure categories, `ended()`, takeover, taking back on disconnect.
- Example: a connection plugin that forwards through a relay server; rutis and the runtime both connect to the relay, and the runtime side does not change.
- Under rutis-host, a `runtimes.remote` entry without `dial` / `listen` waits for other plugins (for example a connection plugin loaded as a dylib) to hand over connections.

## 10. Compatibility and migration

- The runtime side does not change at all (C9): deployed Python, Node, Bun, Go, Swift and Kotlin runtimes and apps do not need upgrading.
- Old configuration is still accepted (C10):
  - A `rutis-bridge/peer` row with `runtime`: converted at startup into `dial` or `listen` on the matching `runtimes.remote` entry, and the new form is printed.
  - `listen` + `instances`: the form does not change, nor does its meaning.
- Behavior changes (written into "Migrating to 0.19"):
  - The output prefix changes from the link row id to `runtime <name>`; connections that connect in start having output.
  - When a runtime rutis connects to lacks a plugin, the whole row no longer fails; it becomes `Unavailable` (the same as for connections that connect in).
  - After reconnecting, all rows are resolved again.
  - A token conflict on the same listener changes from a 403 at run time to a startup error.
- Rust API: `AccessPointPlugin`, `Accept`, `Accepted`, `RuntimeAccessPlugin`, `Features::runtime` and `RuntimePlugin::remote` are removed (breaking); they are replaced by the built-in connection plugins and `RuntimeConnections`.

## 11. Superseded designs

| Old document | Superseded parts | Parts still valid |
| --- | --- | --- |
| [remote plugins](design-remote-plugins-2026-10-03.en.md) | The session source and connection direction of runtimes (the last item of §3.1, the last item of §4.1, the connection method in §4.4), the remote language plugin row in §8 | The runtime contract (§4.2), two-stage readiness (§4.3), lease semantics (the cleanup order in §4.4), node interconnection (§5), sessions and compatibility (§6), lifecycle and security (§7) |
| [remote runtimes: dialing out and dynamic connections](design-remote-runtime-dial-2026-10-14.en.md) | The access point and the implementation of "connection = instance" in §4, the access point parts of §5, the Rust-side work in §7 | Support for both directions (R1), the transitional identity implementation (§5), the dial-out and reconnection specification (§6), the runtime-side implementation for each language |
| [network stack](design-protocol-channel-decoupling-2026-10-03.en.md) | The statement that a link is the session source for runtimes | The channel contract, framing, connectors, shared listener registrations, the WebSocket binding |

## 12. Phases

| Phase | Content | Acceptance |
| --- | --- | --- |
| L1 [#696](https://github.com/arcships/rutis/issues/696) | Handover interface and the unified remote plugin layer | The same set of row tests gives the same results over both kinds of connection; single-connection mode refuses a second id; after a remote upgrade, reconnecting validates against the new schema |
| L2 [#697](https://github.com/arcships/rutis/issues/697) | Built-in dial-out and listening connection plugins; the access point and `RuntimeAccessPlugin` retired | `websocket_conformance` and `dial_conformance` pass with unchanged cases; the revocation guarantee does not change |
| L3 [#698](https://github.com/arcships/rutis/issues/698) | rutis-host configuration, output, old configuration conversion, migration guide and guide updates | The three forms and the old form end to end; old configuration runs without changing a single character |
| L4 [#699](https://github.com/arcships/rutis/issues/699) | Opened-up authentication, the Rust embedding interface, the relay example, connection plugin contract tests | The relay example connects a Python runtime end to end; authentication that waits for an external answer passes the contract tests |

The formal coverage in 0.20 ([#613](https://github.com/arcships/rutis/issues/613) remote leases, [#624](https://github.com/arcships/rutis/issues/624) access point) covers this design.
