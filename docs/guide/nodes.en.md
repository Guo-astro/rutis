# Connect Nodes

A rutis host is a **node**. Nodes connect through a **link**: one side dials (`dial`) and the other listens (`listen`). Once connected, they can share services, run plugins on each other's behalf, and forward events. The connection reconnects automatically when interrupted.

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

Both sides use the same token. The dialing side presents `RUTIS_TOKEN` (or `RUTIS_TOKEN_OFFICE`); the listening side uses it to verify that the peer is `main`.

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

The node row's `config` determines what happens over the connection. Changing `export`, `import`, or `events` does not disconnect the link; only the corresponding feature restarts.

| Field | Purpose |
| --- | --- |
| `export: ["weather"]` | Makes this node's `weather` service available to the peer. |
| `import: ["calendar"]` | Registers the peer's `calendar` service on this node. It is rejected if this node already has a service with that name. |
| `events: { "out": ["tick"], "in": ["tock"] }` | Forwards events in one direction. |
| `host: true` | Allows the peer to run plugins installed on this node. Enable only for trusted peers. |
| `rows: true` | Runs rows named `peer:<peer>/<plugin>` from this file on the peer. |
| `runtime: "<name>"` | Identifies the peer as a language runtime; see below. |

Calls retain their call chain across nodes: callbacks during a synchronous call, including callbacks that call back again, can cross multiple nodes.

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
  "runtimes": { "remote": [{ "name": "gpu", "language": "python" }] },
  "rows": [
    { "id": "gpu", "name": "rutis-bridge/peer", "config": { "peer": "gpu", "dial": "wss://gpu.example.com:7443/rutis", "runtime": "gpu" } },
    { "id": "embedder", "name": "gpu:embedder" }
  ]
}
```

A remote Python runtime row is named `<runtime-name>:<module>`; the module is resolved on the remote machine. A Node runtime can also run remotely: in the Node project on that machine, run `node node_modules/@arcships/rutis-runtime/src/runner.mjs listen:wss://… --id <name> --peer <controller> ./package.json` (plugins are resolved from that `package.json`) and set `language` to `node`. A remote runtime serves one controller at a time. When its controller disconnects, it unloads all plugins running for that controller.

### Runtimes that connect in

A runtime can also connect to rutis instead: use this when the remote machines are behind NAT, change address, or come and go in numbers (devices, workers that scale). On the rutis side, give the `runtimes.remote` entry `listen` (the listener they connect to) and `instances` (an `instanced` group), and write no node row:

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

Every connection presents the same token (`RUTIS_TOKEN_WORKER`, or `RUTIS_TOKEN`), reports an id of its own, and becomes an instance of the group: created when it connects, closed when it disconnects; a newer connection with the same id takes over. The group's rows run, in each instance, on that instance's connection, and the services they provide are seen only in that instance. For the fields and rules, see [rutis-host](rutis-host.en.md#runtimes-that-connect-in).

The rutis side is ready; connecting out (`dial:wss://…`) comes to the language runtimes next: Python with [#383](https://github.com/arcships/rutis/issues/383); Node, Go and Bun with [#384](https://github.com/arcships/rutis/issues/384). Until then a runtime only listens, and rutis connects to it (as above).

## Connect a Cordis application as a node

An existing Cordis application can become a node through `@arcships/rutis-runtime/bridge`; see [Cordis](cordis.en.md).

## Security

- Use `wss://` for remote connections. A listener without TLS may bind only to a loopback address.
- A Python runtime cannot listen on `wss://` with CPython 3.13.6: its TLS sockets deadlock when one thread is blocked reading while another writes ([python/cpython#137583](https://github.com/python/cpython/issues/137583)), so every handshake hangs, and rutis refuses to start `listen:wss://` on that version. Use 3.13.7 or later (3.13.5 and earlier, and 3.14, are not affected); `ws://` is not affected.
- Use one token per peer (`RUTIS_TOKEN_<NODE>`) and set it only on the nodes that need it.
- `host: true` lets a peer run any plugin installed on this node. Enable it only for trusted peers. It can load installed plugins, not file paths.
- Nodes share services by name, and only services listed in `export` are shared.
