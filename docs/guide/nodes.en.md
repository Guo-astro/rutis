# Connect Nodes

A rutis host is a **node**. Nodes connect through a **link**: one side dials (`dial`) and the other listens (`listen`). Once connected, they can share services, run plugins on each other's behalf, and share events. The connection reconnects automatically when interrupted.

The examples below configure rows in `rutis.json`. In Rust, use `rutis_bridge::PeerPlugin` for the same purpose (see [Embed in a Rust application](rust-host.en.md)).

## A link

```json
{ "id": "office", "name": "rutis-bridge/peer", "config": { "peer": "office", "dial": "wss://office.example.com/rutis" } }
```

The other side (`office`) listens:

```json
{
  "id": "office",
  "listen": [{ "name": "public", "address": "0.0.0.0:7443", "cert": "server.pem", "key": "server.key" }],
  "rows": [
    { "id": "main", "name": "rutis-bridge/peer", "config": { "peer": "main", "listen": "public" } }
  ]
}
```

Both sides use the same token. The dialing side (`main`) presents `RUTIS_TOKEN_OFFICE`; the listening side (`office`) checks it against `RUTIS_TOKEN_MAIN` to verify that the peer is `main`. Set both variables to the same value.

- `dial` accepts `wss://…` for remote connections (TLS required) or `ws://127.0.0.1…` for the local loopback interface.
- For a self-signed certificate, add its issuing CA to `RUTIS_CA` on the dialing side.
- After a disconnect, retries use backoff starting at 0.5 seconds and capped at 30 seconds. Rejected credentials (403 or an untrusted certificate) are retried every 30 seconds. An incompatible protocol stops retrying and reports an error.
- `rutis-host run` prints a line when a link's state changes, with a readable category and never the token. The node row's own `running` only says that the row runs, not that the link is connected:

  ```
  office: link connected
  office: link down (127.0.0.1:7443: Connection refused (os error 61)); retrying in 2.1s
  office: link refused: auth-rejected (office.example.com: upgrade refused with 403 Forbidden); retrying in 30s
  office: link stopped: incompatible (office does not declare runtime)
  ```

  `auth-rejected` means the credentials were refused (a wrong token, a certificate for another name or expired, an untrusted CA), `incompatible` that the ends do not fit (not retried), and `link down` that the far end cannot be reached for now. The full format is in [What `run` prints](rutis-host.en.md#what-run-prints).
- A dialing peer that arrives before the listening side's link is ready is asked to retry, so either side can start first.

## What a link can do

The node row's `config` determines what happens over the connection. Changing `export` or `import` does not disconnect the link; only the corresponding feature restarts.

| Field | Purpose |
| --- | --- |
| `export: ["weather", "session/*"]` | Makes this node's `weather` service available to the peer, and this node's emissions of `weather` and of every `session/…` event reach the peer's listeners. |
| `import: ["calendar", "tock"]` | Registers the peer's `calendar` service on this node (rejected if this node already has a service with that name), and this node's listeners of `calendar` and `tock` hear the peer's emissions of them. |
| `host: true` | Allows the peer to run plugins installed on this node. Enable only for trusted peers. |
| `rows: true` | Runs rows named `peer:<peer>/<plugin>` from this file on the peer. |
| `runtime: "<name>"` | Identifies the peer as a language runtime; see below. |

Calls retain their call chain across nodes: callbacks during a synchronous call, including callbacks that call back again, can cross multiple nodes.

### Events between nodes

An item of `export` / `import` is a name, or a prefix written with a trailing `*` (`session/*`; `*` alone is every name); prefixes select events only, services keep exact names. A name may be shared both ways. With the peer's `import` matching this node's `export`, the peer's listeners are entries of this node's event bus, under the same rules as local ones: all five modes (`emit`, `parallel`, `serial`, `waterfall`, `bail`), filters, every value type, and when the link goes down they leave the bus (a call in flight fails once). Events go one hop: what a node imported is never shared further. A listener on the peer is an entry of the bus; its descriptor carries the link row's scope (the root for a link outside instances), for filters. The whole model is in [Unified events](../design-events-2026-10-18.en.md).

For example, `session/…` events emitted on `main` reach listeners in any language on `office`, and `audit/flagged` emitted on `office` reaches the listeners of `main`:

```json
{ "id": "office", "name": "rutis-bridge/peer", "config": { "peer": "office", "dial": "wss://office.example.com/rutis", "export": ["session/*"], "import": ["audit/flagged"] } }
```

```json
{ "id": "main", "name": "rutis-bridge/peer", "config": { "peer": "main", "listen": "public", "import": ["session/*"], "export": ["audit/flagged"] } }
```

A name is shared only when the emitting end lists it in `export` and the listening end in `import`. On `main`, `await ctx.serial("session/quota", user)` asks the listeners on `main` and on `office` one by one in registration order and returns the first result; when `office` goes away, its listeners leave the bus and later dispatches ask only those on `main`.

Runtimes (local ones, [on another machine](#a-language-runtime-on-another-machine), [connecting in](#runtimes-that-connect-in)) are not node links: their listeners are on the host's bus directly, with no `export` / `import`.

`events: { "out": […], "in": […] }` was removed in rutis 0.13 and is refused: put `out` in `export` and `in` in `import`. Nodes of 0.12 and earlier are refused when they connect (`link stopped: incompatible (…)`); see [migrating to 0.13](../migration-0.13.en.md).

## Run a plugin on another node

When the peer enables `host: true` and this node enables `rows: true`, a `peer:<peer>/<plugin>` row loads that plugin on the peer:

```json
{ "id": "office", "name": "rutis-bridge/peer", "config": { "peer": "office", "dial": "wss://office.example.com/rutis", "rows": true } },
{ "id": "scanner", "name": "peer:office/scanner-plugin", "config": { "dpi": 300 } }
```

The plugin runs on the peer and uses that node's services. This node still controls its configuration and lifecycle. If the peer goes offline, the row waits and reloads the plugin when the peer returns. The peer executes the row's `isolate` and `inject` settings.

## A language runtime on another machine

For example, run a Python runtime on a GPU machine and manage its plugins from the main node:

```bash
# On the GPU machine: install rutis[network] and the plugins, then listen
RUTIS_TOKEN=secret RUTIS_CERT=server.pem RUTIS_KEY=server.key \
  python -m rutis listen:wss://0.0.0.0:7443/rutis --id gpu --peer main /srv/plugins
```

```json
{
  "id": "main",
  "runtimes": { "remote": [{ "name": "gpu", "language": "python", "dial": "wss://gpu.example.com:7443/rutis" }] },
  "rows": [
    { "id": "embedder", "name": "gpu:embedder" }
  ]
}
```

One `runtimes.remote` entry describes a remote runtime whole. Who connects, and whether there is one runtime or many, are each decided by one field:

| Written | Meaning |
| --- | --- |
| `dial: <address>` | One runtime, which rutis dials; its `--id` is the runtime's name |
| `listen: <listener>` | One runtime, which connects in; its `--id` is the runtime's name |
| `listen` + `instances: <group>` | Many, each connection an instance (see [Runtimes that connect in](#runtimes-that-connect-in)) |
| none of them | No connection of its own: a node row with `runtime`, or the program embedding the host, provides its session |

`dial` and `listen` cannot both be set, nor `dial` and `instances` (rutis can dial only one runtime). The token is `RUTIS_TOKEN_<NAME>`. The connection is over WebSocket.

The form before 0.19 — `runtimes.remote` with only `name` and `language`, plus a `rutis-bridge/peer` row with `"runtime": "gpu"` — still works (see [Migrating to 0.19](../migration-0.19.en.md)).

`run` prints how the connections change, prefixed `runtime <name>`:

```text
runtime gpu: link connected
runtime gpu: link down (gpu.example.com:7443: Connection refused); retrying in 2.1s
runtime worker: connected worker-1
runtime worker: disconnected worker-1 (connection lost)
runtime worker: refused (the worker runtime lacks rows.v2 and hosts: …)
```

The `connections:` part of `rutis-host status` gives their state now. When the remote side does not contain a row's plugin, only that row fails (`runtime gpu: this connection does not contain plugin embedder`); the other rows are not affected.

A remote Python runtime row is named `<runtime-name>:<module>`; the module is resolved on the remote machine. A Node runtime can also run remotely: in the Node project on that machine, run `node node_modules/@arcships/rutis-runtime/src/runner.mjs listen:wss://… --id <name> --peer <controller> ./package.json` (plugins are resolved from that `package.json`) and set `language` to `node`; likewise a Bun runtime: in the Bun project on that machine, run `bun --no-install --no-env-file node_modules/@arcships/rutis-bun/src/main.ts listen:wss://… --id <name> --peer <controller> .` (plugins are resolved from that directory, rows named `<runtime-name>:<module>`) and set `language` to `bun`. A remote runtime serves one controller at a time. When its controller disconnects, it unloads all plugins running for that controller. Every remote runtime (Python, Node, Bun, a Go or Swift binary) prints one line on stderr once it listens, with the address it actually bound: `rutis: listening on <address>`. With port 0 (`listen:ws://127.0.0.1:0/rutis`) the system picks a free port, and a script or test reads it from this line; do not pick a free port first and pass it on, as another process can take it before the runtime binds it. The Python runtime also prints one line `rutis: session ended (<reason>); waiting for a controller` when a session ends (the reason such as `heartbeat timeout`, `connection lost`, `closed (1001): …`), and `rutis: session ended (replaced by a new connection)` when a new connection takes over.

### Runtimes that connect in

A runtime can also connect to rutis instead: use this when the remote machines are behind NAT, change address, or come and go in numbers (devices, workers that scale). For one runtime, give the `runtimes.remote` entry only `listen` (the listener it connects to), and give the runtime the runtime's name as its `--id`; when their number changes, add `instances` (an `instanced` group). Either way, write no node row:

```json
{
  "id": "main",
  "listen": [{ "name": "public", "address": "0.0.0.0:7443", "cert": "server.pem", "key": "server.key" }],
  "runtimes": { "remote": [{ "name": "worker", "language": "python", "listen": "public", "instances": "worker" }] },
  "rows": [
    { "id": "worker", "group": true, "instanced": true, "config": [
      { "id": "embedder", "name": "worker:embedder" }
    ] }
  ]
}
```

Every connection presents the same token (`RUTIS_TOKEN_WORKER` on the rutis side), reports an id of its own, and becomes an instance of the group: created when it connects, closed when it disconnects; a newer connection with the same id takes over. The group's rows run, in each instance, on that instance's connection, and the services they provide are seen only in that instance. That holds for the services they provide without declaring them too (which reach rutis by themselves): those are visible only inside that instance, to the rows of the instance, never to rows outside it or to other instances. For the fields and rules, see [rutis-host](rutis-host.en.md#runtimes-that-connect-in).

A Python runtime connects in with `dial:`:

```bash
# The worker machine: with rutis[network] and the plugins installed, connect to rutis
RUTIS_TOKEN=secret RUTIS_CA=ca.pem \
  python -m rutis dial:wss://main.example.com:7443/rutis --id worker-1 --peer main /srv/plugins
```

A Go or Swift plugin binary takes the same `dial:` channel (and `listen:`), with the same arguments, environment variables, reconnection rules and state lines as Python (set `language` to `go` or `swift`):

```bash
# The worker machine: with the plugin binary there, connect to rutis
RUTIS_TOKEN=secret RUTIS_CA=ca.pem \
  ./weather dial:wss://main.example.com:7443/rutis --id worker-1 --peer main .
```

- `--id` is the id this connection reports (lower-case letters, digits and `-`), which names its instance; `--peer` is the rutis `id`.
- `RUTIS_TOKEN` equals `RUTIS_TOKEN_WORKER` on the rutis side; `RUTIS_CA` is a further trusted CA (for a self-signed certificate), besides the system roots. A non-loopback address must be `wss://`.
- The runtime keeps the connection itself and reconnects by fixed rules: when it cannot connect or the connection drops, it waits from 0.5 s, doubling up to 30 s, back to 0.5 s once a connection has lasted 60 s; a refused token or id (401 / 403 at the upgrade, or rutis rejecting the id with close code 4003) is tried again every 30 s; an incompatible protocol (400 at the upgrade and the like) is not retried, and the process exits with status 1. On every disconnect all rows are unloaded first (cleanups in reverse order); a reconnection is a new session, and the rows load again.
- States go to stderr: `rutis: connected to …`, `rutis: connection lost (…); retrying in 1s`, `rutis: refused: auth-rejected (…); retrying in 30s`, `rutis: refused: incompatible (…); stopping`.
- SIGTERM or Ctrl-C stops it: all rows are unloaded, the connection closes, and it does not reconnect.
- Two processes with the same id take over from each other, displacing one another in turn at the pace of their backoffs; give every machine and worker an id of its own.

Node and Bun runtimes dial out with the same arguments, environment variables and rules; set `language` in `runtimes.remote` to `node` or `bun`:

```bash
# In a Node project (plugins are resolved from package.json)
RUTIS_TOKEN=secret RUTIS_CA=ca.pem \
  node node_modules/@arcships/rutis-runtime/src/runner.mjs dial:wss://main.example.com:7443/rutis --id worker-1 --peer main ./package.json
# In a Bun project (plugins are resolved from this directory)
RUTIS_TOKEN=secret RUTIS_CA=ca.pem \
  bun --no-install --no-env-file node_modules/@arcships/rutis-bun/src/main.ts dial:wss://main.example.com:7443/rutis --id worker-1 --peer main .
```

Node and Bun run each session in a child process: on a disconnect the child unloads every row and exits, and a reconnection starts a new one. `RUTIS_CA` is trusted together with the roots Node and Bun trust by default.

Kotlin plugins usually ship inside an Android application, which dials out to rutis ([Android app hosting](../design-android-runtime-2026-10-12.en.md) §5). The SDK's jvm target takes the same `dial:` and `listen:` channels, environment variables and rules in `Rutis.serve` (its listener serving `wss://` with `RUTIS_CERT` and `RUTIS_KEY`), with `language` set to `kotlin` (or `kt`).

## Connect a Cordis application as a node

An existing Cordis application can become a node through `@arcships/rutis-runtime/bridge`; see [Cordis](cordis.en.md).

## Security

- Use `wss://` for remote connections. A listener without TLS may bind only to a loopback address.
- A Python runtime cannot listen on `wss://` with CPython 3.13.6: its TLS sockets deadlock when one thread is blocked reading while another writes ([python/cpython#137583](https://github.com/python/cpython/issues/137583)), so every handshake hangs, and rutis refuses to start `listen:wss://` on that version. Use 3.13.7 or later (3.13.5 and earlier, and 3.14, are not affected); `ws://` is not affected. Dialing out (`dial:wss://`) is not affected either: one thread does all of a connection's reads and writes.
- Use one token per peer (`RUTIS_TOKEN_<NODE>`) and set it only on the nodes that need it.
- `rutis-host` takes its tokens from the environment, so they cannot be changed while it runs: changing a token takes a restart of `rutis-host` (a restart drops every connection). To disconnect existing connections, stop that row: when the row that links to the peer, or that accepts the runtimes, stops, its connections close and calls in flight on them fail with a transport error.
- `host: true` lets a peer run any plugin installed on this node. Enable it only for trusted peers. It can load installed plugins, not file paths.
- Nodes share services and events by name, and only those listed in `export` are shared; an event also needs the far end to list it in `import`.
