# rutis-bridge

Connect a [rutis](https://github.com/arcships/rutis) application to other processes, languages and machines.

- **Plugins in other languages** (`runtime`): TypeScript / JavaScript and Python plugins run in language runtimes, on this machine (`LocalRuntime::node`, `LocalRuntime::python`) or on another one (`RuntimePlugin::remote`); [rutis-loader](https://crates.io/crates/rutis-loader) manages them as rows.
- **Links between nodes**: `LinkPlugin` dials or listens for one peer and reconnects; on the session, nodes share services (`ExportPlugin`, `ImportPlugin`), run plugins for each other (`HostPlugin`) and share events (`LinkEventsPlugin`, by `export` / `import`). `PeerPlugin` composes them.
- **Channels**: processes it starts and Unix sockets (`pipe`; on Windows, processes on loopback channels), in-process channels (`memory`), WebSocket with TLS (`websocket`). Each gives a `Channel` directly: `pipe::spawn`, `memory::pair`, `websocket::dial`, `websocket::listen`.
- **Cordis plugins mounted in Rust** (`cordis`): published Cordis plugins with Rust bindings generated at build time.

| feature | adds | default |
| --- | --- | --- |
| `node` | local Node runtimes | yes |
| `python` | local Python runtimes | |
| `websocket` | the WebSocket transport (rustls) | |
| `cordis` | Cordis mounts and their binding generation | |
| `testing` | channel contract tests and conformance suites | |

```toml
rutis-bridge = { version = "0.17", features = ["python", "websocket"] }
```

A service plugins use by name is a `dyn rutis_bridge::session::HostDispatch` under `host_key(name)`, a `rutis::host::Key<dyn HostDispatch>`: `ctx.provide_at(host_key("clock"), Arc::new(Clock))?`. The other key helpers (`peer_key`, `identity_key`, …) return typed keys too; `.erase()` gives the `TypeKey` where one is needed.

Guides (Chinese): [embedding in Rust](https://github.com/arcships/rutis/blob/main/docs/guide/rust-host.md), [nodes](https://github.com/arcships/rutis/blob/main/docs/guide/nodes.md), [Cordis](https://github.com/arcships/rutis/blob/main/docs/guide/cordis.md). Local runtimes need Linux or macOS.

Released with rutis-loader, rutis-host and the npm and PyPI packages at one version.
