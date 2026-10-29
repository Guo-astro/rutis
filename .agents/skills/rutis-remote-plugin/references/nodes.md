> Copied from `docs/guide/nodes.en.md` in the rutis repository (main, 2026-10-29) so this skill can be used on its own. The original is authoritative: https://github.com/arcships/rutis/blob/main/docs/guide/nodes.en.md

# Connect Nodes

A rutis host is a **node**. Two nodes are joined by a connection: one writes a `rutis-host/dial` row to connect, the other a `rutis-host/listen` row to take the connection. Once connected, the services of each are seen on both sides, events are one bus, and each side can have the other run plugins. The dialing side connects again when the connection drops. Nodes are written the same way as [remote runtimes](#a-language-runtime-on-another-machine); there is no configuration of their own for nodes.

## Two nodes

`main` connects to `office`:

```json
{
  "id": "main",
  "rows": [
    { "id": "office", "name": "rutis-host/dial", "config": { "address": "wss://office.example.com/rutis" } }
  ]
}
```

`office` takes the connection from `main`:

```json
{
  "id": "office",
  "listen": [{ "name": "public", "address": "0.0.0.0:7443", "cert": "server.pem", "key": "server.key" }],
  "rows": [
    { "id": "main", "name": "rutis-host/listen", "config": { "listener": "public" } }
  ]
}
```

- Both ends use one token: `main`'s row `office` presents `RUTIS_TOKEN_OFFICE`, and `office`'s row `main` checks it against `RUTIS_TOKEN_MAIN`; set both variables to the same value (the row id in upper case, `-` as `_`).
- Each end gives the `id` of its rutis.json in the handshake: `office`'s plugins are named `office:<plugin>` on `main`, and the other way round `main:<plugin>`.
- `address` is `wss://…` (between machines, TLS required) or `ws://127.0.0.1…` (loopback). For a self-signed certificate, put the CA that signed it in the dialing side's `RUTIS_CA`.
- Reconnecting: backoff starts at 0.5 s and doubles up to 30 s, back to 0.5 s after a connection that lasted 60 s; rejected credentials (403, an untrusted certificate) are retried every 30 s; an incompatible protocol stops it. A dialer arriving before the listening side has its row is refused (403) and tries again 30 s later, so start the listening side first.
- `rutis-host run` prints the connection's changes, prefixed with the row id and without the token; the row's own `running` only says it runs, not that it is connected:

  ```text
  office: connected
  office: disconnected (office.example.com:7443: Connection refused); retrying in 2.1s
  office: disconnected (auth-rejected: office.example.com:7443: upgrade refused with 403 Forbidden); retrying in 30s
  main: connected main
  main: disconnected main (connection lost)
  ```

  The first three lines are on `main`, the last two on `office` (a `rutis-host/listen` row writes the id that connected after `connected` / `disconnected`). The full format is in [What rutis-host prints](https://github.com/arcships/rutis/blob/main/docs/guide/rutis-host.en.md#what-run-prints).

## Once connected

- **Services are seen on both sides**: a service a row on `office` provides can be injected and called on `main` directly, and the other way round, without any list. Here it is provided by the remote plugin `office:<the row providing it>`. Call chains are kept: callbacks during a synchronous call, and calls back from them, cross several nodes.
- **One name, kernel rules**: when both sides provide a name, each uses its own; the one registered later is refused, and that remote plugin fails, saying why:

  ```text
  office:weather: weather is provided here by row weather: not registered from office
  ```

- A service that came over a connection is not sent back over it; through a node in between, it reaches a third one (`b`'s service reaches `c` through `a`).
- To keep a service from the far end, use `isolate` on the row that provides it: a service only in one scope does not cross connections.
- **Events are one bus**: the far end's listeners are entries of this side's bus, with the same rules as on one machine: the five modes (`emit`, `parallel`, `serial`, `waterfall`, `bail`), filters and every value type, without any list. When the connection drops, they leave the bus (calls in flight fail once). A far listener's description carries the connection's scope, for filters. The full model is in [unified events](https://github.com/arcships/rutis/blob/main/docs/design-events-2026-10-18.en.md).
- When the connection drops, the far end's plugins here unload; rows depending on their services stop and wait by the kernel's rules, and come back once it reconnects.

## Run a plugin on another node

A row `<connection row>:<plugin>` has the far end run a plugin, with the row's config:

```json
{ "id": "office", "name": "rutis-host/dial", "config": { "address": "wss://office.example.com/rutis" } },
{ "id": "scanner", "name": "office:scanner", "config": { "dpi": 300 } }
```

- The far end looks `scanner` up with its own loader, as it resolves its own rows (npm packages, `py:` modules, plugins in Go or Swift binaries and so on); the plugin runs there, using that side's services, and what it provides is seen here. Its config and its starting and stopping are this side's.
- While the far end is not connected, the row waits, and loads once it is; when the far end has no such plugin, only this row fails: `scanner: unresolved: entry "scanner" rejected: runtime office: this connection does not contain plugin scanner`.

## A language runtime on another machine

For example, run only a Python runtime on a GPU machine and have the main node decide which plugins it runs:

```bash
# On the GPU machine: install rutis[network] and the plugins, then listen
RUTIS_TOKEN=secret RUTIS_CERT=server.pem RUTIS_KEY=server.key \
  python -m rutis listen:wss://0.0.0.0:7443/rutis --id gpu --peer main /srv/plugins
```

```json
{
  "id": "main",
  "rows": [
    { "id": "gpu", "name": "rutis-host/dial", "config": { "address": "wss://gpu.example.com:7443/rutis" } },
    { "id": "embedder", "name": "gpu:embedder" }
  ]
}
```

- One `rutis-host/dial` row is one connection, and its row id (`gpu`) names it: the runtime on the far end takes this name as its `--id`, and the rutis `id` as its `--peer`.
- A plugin run on the far end is the row `<connection row>:<plugin>` (`gpu:embedder`), its configuration in the row's `config`. The plugin name is resolved on the far end, with no language written: a module in a Python runtime, a package or module in a Node or Bun runtime, a plugin's name in a Go or Swift binary. While the far end is not connected, the row waits; once it is, rutis has the far end run it.
- Token: `RUTIS_TOKEN_GPU` on the rutis side (the row id in uppercase, `-` written as `_`), `RUTIS_TOKEN` on the runtime's, both set to one value. For a self-signed certificate, put its CA in `RUTIS_CA` on the rutis side. The connection is over WebSocket.
- After a disconnection rutis reconnects with backoff: from 0.5 seconds, doubling, at most 30 seconds, from 0.5 seconds again once a connection lasted 60 seconds; refused, every 30 seconds; incompatible protocols, it stops.
- `run` prints how the connection changes, prefixed with the row id:

  ```text
  gpu: connected
  gpu: disconnected (gpu.example.com:7443: Connection refused); retrying in 2.1s
  gpu: disconnected (auth-rejected: gpu.example.com:7443: upgrade refused with 403 Forbidden); retrying in 30s
  ```

  The `connections:` part of `rutis-host status` gives its state now (`gpu: connected`). When the far end does not contain a row's plugin, only that row fails (`embedder: unresolved: entry "embedder" rejected: runtime gpu: this connection does not contain plugin embedder`); the other rows are not affected. Fields and rules: [rutis-host](https://github.com/arcships/rutis/blob/main/docs/guide/rutis-host.en.md#remote-runtimes-and-devices).
- 0.18's `runtimes.remote` is removed in 0.19; how to change it: [Migrating to 0.19](https://github.com/arcships/rutis/blob/main/docs/migration-0.19.en.md).

A Node runtime can also run remotely: in the Node project on that machine, run `node node_modules/@arcships/rutis-runtime/src/runner.mjs listen:wss://… --id <name> --peer <controller> ./package.json` (plugins are resolved from that `package.json`); likewise a Bun runtime: in the Bun project on that machine, run `bun --no-install --no-env-file node_modules/@arcships/rutis-bun/src/main.ts listen:wss://… --id <name> --peer <controller> .` (plugins are resolved from that directory). A Go or Swift plugin binary runs `<binary> listen:wss://… --id <name> --peer <controller> .`. The rutis side is the same one `rutis-host/dial` row for all of them. A remote runtime serves one controller at a time. When its controller disconnects, it unloads all plugins running for that controller. Every remote runtime (Python, Node, Bun, a Go or Swift binary) prints one line on stderr once it listens, with the address it actually bound: `rutis: listening on <address>`. With port 0 (`listen:ws://127.0.0.1:0/rutis`) the system picks a free port, and a script or test reads it from this line; do not pick a free port first and pass it on, as another process can take it before the runtime binds it. The Python runtime also prints one line `rutis: session ended (<reason>); waiting for a controller` when a session ends (the reason such as `heartbeat timeout`, `connection lost`, `closed (1000): …`), and `rutis: session ended (replaced by a new connection)` when a new connection takes over.

### Runtimes that connect in

A runtime can also connect to rutis instead: use this when the remote machines are behind NAT or change address. The rutis side is one `rutis-host/listen` row naming a listener under `listen`:

```json
{
  "id": "main",
  "listen": [{ "name": "public", "address": "0.0.0.0:7443", "cert": "server.pem", "key": "server.key" }],
  "rows": [
    { "id": "worker", "name": "rutis-host/listen", "config": { "listener": "public" } }
  ]
}
```

Every connection presents the same token (`RUTIS_TOKEN_WORKER` on the rutis side), gives an id in its handshake, and stands on its own: the plugins it brings register as `<id>:<plugin>`, their services visible everywhere; a newer connection with the same id replaces the older (the older one's plugins all unload first, then it is closed with 4002). rutis creates no instance per connection.

 In 0.19 the far ends of `rutis-host/listen` bring only the plugins they installed themselves and provide (such as [Swift](https://github.com/arcships/rutis/blob/main/docs/guide/swift-app.en.md) and [Kotlin](https://github.com/arcships/rutis/blob/main/docs/guide/kotlin-app.en.md) apps installing their own); `<connection row>:<plugin>` rows do not run on a runtime that connected in. For rutis to decide which plugins a runtime runs, have the runtime listen and rutis dial it with `rutis-host/dial` (the section above). Fields and rules: [rutis-host](https://github.com/arcships/rutis/blob/main/docs/guide/rutis-host.en.md#remote-runtimes-and-devices).

A Python runtime connects in with `dial:`:

```bash
# The worker machine: with rutis[network] and the plugins installed, connect to rutis
RUTIS_TOKEN=secret RUTIS_CA=ca.pem \
  python -m rutis dial:wss://main.example.com:7443/rutis --id worker-1 --peer main /srv/plugins
```

A Go or Swift plugin binary takes the same `dial:` channel (and `listen:`), with the same arguments, environment variables, reconnection rules and state lines as Python:

```bash
# The worker machine: with the plugin binary there, connect to rutis
RUTIS_TOKEN=secret RUTIS_CA=ca.pem \
  ./weather dial:wss://main.example.com:7443/rutis --id worker-1 --peer main .
```

- `--id` is the id this connection gives in its handshake (lower-case letters, digits and `-`); its plugins register as `<id>:<plugin>`; `--peer` is the rutis `id`.
- `RUTIS_TOKEN` equals `RUTIS_TOKEN_WORKER` on the rutis side; `RUTIS_CA` is a further trusted CA (for a self-signed certificate), besides the system roots. A non-loopback address must be `wss://`.
- The runtime keeps the connection itself and reconnects by fixed rules: when it cannot connect or the connection drops, it waits from 0.5 s, doubling up to 30 s, back to 0.5 s once a connection has lasted 60 s; a refused token (401 / 403 at the upgrade, or the handshake's check refusing it with close code 4003) is tried again every 30 s; an incompatible protocol (400 at the upgrade and the like) is not retried, and the process exits with status 1. On every disconnect all rows are unloaded first (cleanups in reverse order); a reconnection is a new session, and the rows load again.
- States go to stderr: `rutis: connected to …`, `rutis: connection lost (…); retrying in 1s`, `rutis: refused: auth-rejected (…); retrying in 30s`, `rutis: refused: incompatible (…); stopping`.
- SIGTERM or Ctrl-C stops it: all rows are unloaded, the connection closes, and it does not reconnect.
- Two processes with the same id take over from each other, displacing one another in turn at the pace of their backoffs; give every machine and worker an id of its own.

Node and Bun runtimes dial out with the same arguments, environment variables and rules:

```bash
# In a Node project (plugins are resolved from package.json)
RUTIS_TOKEN=secret RUTIS_CA=ca.pem \
  node node_modules/@arcships/rutis-runtime/src/runner.mjs dial:wss://main.example.com:7443/rutis --id worker-1 --peer main ./package.json
# In a Bun project (plugins are resolved from this directory)
RUTIS_TOKEN=secret RUTIS_CA=ca.pem \
  bun --no-install --no-env-file node_modules/@arcships/rutis-bun/src/main.ts dial:wss://main.example.com:7443/rutis --id worker-1 --peer main .
```

Node and Bun run each session in a child process: on a disconnect the child unloads every row and exits, and a reconnection starts a new one. `RUTIS_CA` is trusted together with the roots Node and Bun trust by default.

Kotlin plugins usually ship inside an Android application, which dials out to rutis ([Android app hosting](https://github.com/arcships/rutis/blob/main/docs/design-android-runtime-2026-10-12.en.md) §5). The SDK's jvm target takes the same `dial:` and `listen:` channels, environment variables and rules in `Rutis.serve` (its listener serving `wss://` with `RUTIS_CERT` and `RUTIS_KEY`).

## Cordis applications

A Cordis application joins rutis by running its plugins in the Node runtime (`<package name>` rows); see [Cordis](https://github.com/arcships/rutis/blob/main/docs/guide/cordis.en.md).

## Your own connection

Write your own way of connecting (a relay, Bluetooth, another transport) as a connection plugin: take a connection that carries binary messages and hand it over as a session. In Rust, hand over a `Channel`: `ctx.plugin(rutis_bridge::Session::new(channel))`, with the host providing `rutis_bridge::LocalId` in the root ctx (rutis-host does); in other languages, declare `inject: ['sessions']` and call `sessions.open({ send, close })`, then feed incoming messages to the returned `near.receive(bytes)` and call `near.end({ kind: 'done' })` when the connection ends (https://github.com/arcships/rutis/blob/main/docs/guide/plugin-api.en.md#your-own-connection-sessionsopen). The far end's plugins are named `<far id>:<plugin>` here; to have the far end run a plugin, write a row `<connection row>:<plugin>`, which waits while the far end is not there.

## Security

- Use `wss://` for remote connections. A listener without TLS may bind only to a loopback address.
- A Python runtime cannot listen on `wss://` with CPython 3.13.6: its TLS sockets deadlock when one thread is blocked reading while another writes ([python/cpython#137583](https://github.com/python/cpython/issues/137583)), so every handshake hangs, and rutis refuses to start `listen:wss://` on that version. Use 3.13.7 or later (3.13.5 and earlier, and 3.14, are not affected); `ws://` is not affected. Dialing out (`dial:wss://`) is not affected either: one thread does all of a connection's reads and writes.
- Use one token per connection row (`RUTIS_TOKEN_<ROW ID>`) and set it only on the nodes that need it.
- `rutis-host` takes its tokens from the environment, so they cannot be changed while it runs: changing a token takes a restart of `rutis-host` (a restart drops every connection). To disconnect existing connections, stop that row: when it stops, its connections close and calls in flight on them fail with a transport error.
- Connected nodes see each other's shareable services and events, and can have this node run any plugin its loader resolves (`<connection row>:<plugin>`): connect only to trusted nodes. Use `isolate` on your own rows for services the far end should not see.
