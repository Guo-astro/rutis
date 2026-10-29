# rutis-bridge

Connect a [rutis](https://github.com/arcships/rutis) application to other processes, languages and machines.

- **Channel**: a `Channel` is one connection to a far end, by message: a `sender`, a `receiver` and a `closer` (`close(Close::Done | Close::Replaced | Close::Rejected(reason))`; on WebSocket 1000, 4002, 4003). The low-level functions give one directly: `pipe::spawn` (a process it starts, its channel on a pipe or Unix socket; loopback on Windows), `websocket::dial` and `websocket::listen` (WebSocket with TLS), `memory::pair` (in-process, for tests).
- **Session** (`rutis-bridge/session`): on one `Channel`, brings the far end's plugins into the local kernel. `ctx.plugin(Session::new(channel))` greets and mounts in one step (or `Session::handshake(&ctx, channel).await`, look at `greeted.id()`, then `ctx.plugin(greeted)`); the far end's plugins register as `<id>:<plugin>`, services are visible both ways, events are one bus. `SessionHandle::run(plugin, config)` has the far end run a plugin (what [rutis-loader](https://crates.io/crates/rutis-loader) calls for a row `<connection row>:<plugin>`). When the channel closes the session unloads, and the other way round. Plugins in other languages open one with the service `sessions`: `sessions.open(far)` (`Session::service()` mounts it).
- **Connection plugins**: whatever gets a `Channel` and mounts a session on it. rutis-host's `rutis-host/dial` and `rutis-host/listen` are two; a connection of your own (another transport, your own authentication) is one more, with no other extension point.
- **Plugins in other languages** (`runtime`): TypeScript / JavaScript, Python, Bun, Go and Swift plugins run in language runtimes, on this machine or behind a connection; rutis-loader manages them as rows. The bridge knows no language: the command that starts each language's runtime is rutis-host's (`rutis_host::languages::<language>::launcher`, given to `rutis_host::connect::SpawnPlugin`).
- **Nodes**: a far rutis kernel is a session's far end like any other: the services of both are seen on both sides, events are one bus, and each runs the other's plugins (`run`) from the `PluginCatalog` its host provides (rutis-loader's: what its loader resolves).
- **Cordis plugins mounted in Rust** (`cordis`): published Cordis plugins with Rust bindings generated at build time.

| feature | adds | default |
| --- | --- | --- |
| `websocket` | `websocket::dial` / `websocket::listen` (rustls) | |
| `cordis` | Cordis mounts and their binding generation | |
| `testing` | channel contract tests and conformance suites | |

```toml
rutis-bridge = { version = "0.17", features = ["websocket"] }
```

A service plugins use by name is a `dyn rutis_bridge::session::HostDispatch` under `host_key(name)`, a `rutis::host::Key<dyn HostDispatch>`: `ctx.provide_at(host_key("clock"), Arc::new(Clock))?`. The other key helpers (`session_key`, `catalog_key`) return typed keys too; `.erase()` gives the `TypeKey` where one is needed.

Guides (Chinese): [embedding in Rust](https://github.com/arcships/rutis/blob/main/docs/guide/rust-host.md), [nodes](https://github.com/arcships/rutis/blob/main/docs/guide/nodes.md), [Cordis](https://github.com/arcships/rutis/blob/main/docs/guide/cordis.md). Local runtimes need Linux or macOS.

Released with rutis-loader, rutis-host and the npm and PyPI packages at one version.
