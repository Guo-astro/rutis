> Copied from `docs/guide/rutis-host.en.md` in the rutis repository (main, 2026-10-14) so this skill can be used on its own. The original is authoritative: https://github.com/arcships/rutis/blob/main/docs/guide/rutis-host.en.md

## `rutis.json`

```json
{
  "format": 1,
  "id": "main",
  "runtimes": {
    "node": { "project": "." },
    "py": { "project": "plugins", "python": ".venv/bin/python" },
    "go": { "dir": "plugins/go" },
    "remote": [{ "name": "gpu", "language": "python" }]
  },
  "listen": [
    { "name": "public", "address": "0.0.0.0:7443", "cert": "tls/server.pem", "key": "tls/server.key" }
  ],
  "rows": [
    { "id": "weather", "name": "weather-plugin", "config": { "city": "Oslo" } },
    { "id": "llm", "name": "py:llm_gateway" },
    { "id": "dns", "name": "go:dns" },
    { "id": "local-tool", "name": "./tools/tool.ts" },
    { "id": "office", "name": "rutis-bridge/peer", "config": { "peer": "office", "dial": "wss://office.example.com/rutis", "export": ["weather"] } },
    { "id": "gpu", "name": "rutis-bridge/peer", "config": { "peer": "gpu", "dial": "wss://gpu.example.com/rutis", "runtime": "gpu" } },
    { "id": "embedder", "name": "gpu:embedder" }
  ]
}
```

Relative paths in the file are resolved from the directory containing the file.

An unknown field is an error: the host does not start, and says the file, the line and column, and the fields allowed there. An unknown field in `rows` is a warning: the row runs (without it), and before starting the host says the file, which row, and the fields a row has. A misspelt field is never silently ignored.

### `format`

The file's format version, currently `1`. It may be omitted, and is then read as `1` (files from before 0.10 have no such field and are read as before); the `rutis.dev.json` that `rutis-host new` generates includes it. A file of a newer format than this `rutis-host` supports is not read; the host reports an error before starting, for example:

```text
rutis.json: this file is format 2, this rutis-host reads up to 1: upgrade rutis-host
```

Upgrade `rutis-host` in that case. The format version increases only when the meaning of `rutis.json` changes, and the release notes say so.

### `id`

The endpoint ID of this node. Connected nodes see this name. Defaults to `host`.

### `runtimes`

| Key | Fields | Purpose |
| --- | --- | --- |
| `node` | `project` (default `.`), `runtime` | Starts the Node runtime. Plugin packages are resolved from `project`'s `package.json`. Install `@arcships/rutis-runtime` there, specify it with `runtime`, or use the copy included with the npm distribution of rutis-host. |
| `bun` | `project` (default `.`), `runtime`, `program` | Starts the Bun runtime, with rows named `bun:<module>`. Modules resolve from `project` (packages in `node_modules`, `./relative-paths`). Install `@arcships/rutis-bun` there or specify it with `runtime`; `program` is the Bun executable, by default `bun` on `PATH`. See [Write a Bun plugin](https://github.com/arcships/rutis/blob/main/docs/guide/bun-plugin.en.md). |
| `py` | `project` (default `.`), `python` | Starts the Python runtime. The interpreter defaults to `$VIRTUAL_ENV/bin/python`, then `<project>/.venv/bin/python`, then `python3` (on Windows, `Scripts\python.exe` and `python`). Its environment must contain `rutis` and the plugins. |
| `go` | `dir`, `binaries`, `start` (`on-demand` by default / `eager`), `idle` (seconds, default 60), `project` (default `.`) | Go plugins: one runtime process per binary. Every executable in `dir` that contains the Go SDK's marker counts (the directory is trusted as a whole and should be dedicated); `binaries` lists single files. A runtime is named after its file (`netkit` → `go-netkit`). An on-demand runtime starts when a row first uses it and stops after `idle` seconds without one; `eager` starts all and keeps them running (not together with `idle`). A replaced binary takes effect when the host restarts. |
| `remote` | `name`, `language` | A runtime on another machine, connected through a node row with `"runtime": "<name>"`; see [Connect nodes](https://github.com/arcships/rutis/blob/main/docs/guide/nodes.en.md). |

If a runtime package is missing, startup fails and prints an installation command.

### `listen`

WebSocket listeners used by other nodes to connect. `cert` and `key` are a TLS certificate and private key in PEM format; they can also be supplied through `RUTIS_CERT` and `RUTIS_KEY` (the file's own come first). They go together: with only one of them the host does not start, rather than listening without TLS. A listener without TLS can bind only to a loopback address.

### `rows`

Each row runs a plugin:

| Field | Meaning |
| --- | --- |
| `id` | Row name, unique within this file. |
| `name` | Plugin identifier: an npm package (or package subpath), `py:<name>` (Python entry point or module), `bun:<module>` (a module in the Bun runtime), `go:<plugin>` (the Go binary that has it; `go-<binary name>:<plugin>` when two do), `./relative-path` (plugin file), `<remote-runtime>:<module or plugin>`, `rutis-bridge/peer` (a node; see [Connect nodes](https://github.com/arcships/rutis/blob/main/docs/guide/nodes.en.md)), or `peer:<node>/<plugin>` (a plugin running on another node). |
| `config` | Plugin configuration. |
| `inject` | Additional service names to wait for. |
| `isolate` | `{ "service": true }` gives this row its own instance of the service; `{ "service": "label" }` shares one instance among rows with that label. |
| `disabled` | When `true`, the row does not run. |

Services are shared by name across all plugins. Any plugin that declares `inject: ["weather"]` can use a `weather` service provided in any language or on any node.

## Credentials

Do not put credentials in the file:

| Environment variable | Purpose |
| --- | --- |
| `RUTIS_TOKEN` | Token presented when connecting to other nodes and checked when accepting connections. |
| `RUTIS_TOKEN_<NODE>` | Token for a specific node (uppercase, with `-` written as `_`); takes precedence over `RUTIS_TOKEN`. |
| `RUTIS_CA` | Additional trusted CA certificate in PEM format for `wss://` connections. |
| `RUTIS_CERT` / `RUTIS_KEY` | Listener certificate and key when they are not specified under `listen`. |

## Deployment

Place `rutis.json`, the Node project (with plugins and `@arcships/rutis-runtime` installed), the Python environment (with `rutis` and plugins installed), and the Go plugin directory (binaries for the platform) together. Run `rutis-host run /srv/app/rutis.json` under a process manager such as systemd. When `rutis-host` exits, the runtime processes it started exit as well.

When stopping, have the process manager send SIGTERM to `rutis-host` alone and let it unload the rows. By default (`KillMode=control-group`), systemd also sends SIGTERM to the runtime processes, which then exit before their cleanups run; set `KillMode=mixed`, and set `TimeoutStopSec` longer than the cleanup deadline. If `rutis-host` is killed (SIGKILL, or ending the process on Windows), it cannot unload the rows: on Windows, the runtime processes are ended with it; on Unix, they notice that their channel closed, unload their rows themselves, and exit.
