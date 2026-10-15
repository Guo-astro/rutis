# rutis-host and rutis.json

`rutis-host` is a host that requires no Rust code. It assembles the core, plugin loader, local Node / Python / Go runtimes, and network connections, all driven by a `rutis.json` file. Plugin authors can use it for local development or deployment. For applications that need custom Rust services, see [Embed in a Rust application](rust-host.en.md).

## Installation

| Method | Command |
| --- | --- |
| npm | `npx @arcships/rutis-host …`, or install with `npm install -D @arcships/rutis-host` and run `npx rutis-host …` |
| PyPI | `uvx rutis-host …`, or install with `uv add --dev rutis-host` and run `uv run rutis-host …` |
| Binary | `rutis-host-<version>-<platform>.tar.gz` from GitHub Releases (Linux and macOS, x64 / arm64), or `.zip` for Windows x64 |
| crates.io | `cargo install rutis-host` |

The npm distribution includes the Node runtime (`@arcships/rutis-runtime`), and the PyPI distribution includes the Python runtime (`rutis`). They are used when the project does not provide its own runtime. Go plugins are compiled binaries: running them needs no other package and no Go toolchain.

## Commands

| Command | Purpose |
| --- | --- |
| `rutis-host run [rutis.json]` | Runs the configuration and prints each row's status changes and the state of node rows' links (see [What `run` prints](#what-run-prints)). Press Ctrl-C to stop (see [Stopping and exit status](#stopping-and-exit-status)). |
| `rutis-host dev [directory]` | Runs a plugin project with its `rutis.dev.json` file and reloads it when files change; a Go project is rebuilt, restarting only its runtime. |
| `rutis-host check [rutis.json]` | Without running any plugin, checks that each row can run (see [Checking a configuration](#checking-a-configuration)) and prints its version, dependencies, provided services, and configuration schema, and lists every Go binary (runtime name, SDK, plugin API, plugins). Exits with a nonzero status if a row or binary cannot run. Without `rutis.json`, checks the plugin project in the current directory. |
| `rutis-host new <name> --lang node\|bun\|python\|go` | Creates a plugin project. |
| `rutis-host go add <module>@<version> [rutis.json]` | `go install`s a plugin binary into `runtimes.go.dir` with the machine's Go toolchain. |

## What `run` prints

The output of `run` and `dev` is read by people, scripts and tests. Its format is as follows; changes to it are treated as incompatible changes.

At start, each [`listen`](#listen) listener prints once the address it is actually bound to (with port 0, the port the system picked):

```
rutis-host: listening on <listener name> <address:port>
```

Then each change of a row's state prints `<row id>: <state>` (`running`, `waiting for its services`, `starting`, `failed: …`, `disabled`, `removed`, and so on; a copy inside an instance is `<row id> in PluginId(n)`).

A node row's (`rutis-bridge/peer`) state only says that the row runs; whether its link is connected is printed on a line of its own, when the link's state changes:

| Output | Meaning |
| --- | --- |
| `<row id>: link connected` | The session is ready; the far end can be used. |
| `<row id>: link down (<reason>); retrying in 2.1s` | Cannot connect, or the connection broke (the far end is not up, the network is cut, the session ended); retried with backoff after the time given. |
| `<row id>: link refused: auth-rejected (<reason>); retrying in 30s` | Refused: a wrong token, an untrusted certificate (another name, expired, unknown CA), an identity that does not match; retried after 30 seconds. |
| `<row id>: link stopped: incompatible (<reason>)` | Incompatible: the protocol, the address's form, a capability the far end lacks (such as `runtime`); not retried until the configuration changes and the row reloads. |
| `<row id>: link down (the session ended)` | Listening side: the session ended; it waits for the far end to connect again (a listening side does not retry, so no `retrying in`). |

- The category (`auth-rejected`, `incompatible`) comes from the link's classification of the failure, never from the error's text; the reason in parentheses is detail for people, such as what is wrong with a refused certificate, and its wording is not guaranteed.
- No token or other credential appears in the output.
- Whole seconds are written `30s`; other waits to a tenth of a second.
- States are checked every 200 ms and only changes are printed: a line the same as the last one is not printed again (the same refusal every 30 seconds is printed once), and a state lasting less than 200 ms may not be printed. Nothing is printed while a dialing side has no result yet (connecting), nor while a listening side has no far end yet.

## Checking a configuration

For each row (except disabled ones), `check` verifies that:

- its plugin is found and resolves;
- the row passes the checks made before loading (scopes, and the config of built-in plugins such as link rows);
- every service it waits for (its plugin's `inject`, plus the row's own `inject`) is provided by another row: as plugins declare in `provides`, and a link row provides what it `import`s.

A row waiting for a service no row provides would wait forever: `check` says which service is missing and what to do, and exits with 1; when only disabled rows provide it, it names them. What some rows provide is not known during the check (runtimes on other machines, which the check does not connect to, and `peer:` rows); a service missing then is only reported as possibly coming from them, not as a failure.

A plugin's own config is validated by its runtime when it loads; `check` prints its schema.

## Stopping and exit status

When `run` or `dev` receives a termination signal (SIGINT (Ctrl-C), SIGTERM or SIGHUP on Unix; Ctrl-C, Ctrl-Break or closing the console on Windows), it unloads every row and runtime: each plugin's cleanup runs once, the runtime processes exit, and then `rutis-host` exits.

Cleanups have a deadline, 10 seconds by default. Change it with `--shutdown-timeout <seconds>` or the `RUTIS_SHUTDOWN_TIMEOUT` environment variable (the flag takes precedence; fractions are allowed; it must be above 0). When the deadline passes, or a second signal arrives while stopping (pressing Ctrl-C again), `rutis-host` prints the plugins that are still stopping, ends the runtime processes and the processes they started without waiting for their cleanups, and exits at once. When starting fails, what did start is stopped the same way.

`rutis-host` passes the deadline to the runtime processes it starts (in `RUTIS_SHUTDOWN_TIMEOUT`). When `rutis-host` itself is ended without stopping them (`kill -9`, a crash), each runtime sees its connection close, unloads its rows itself within that deadline, and exits, whatever its plugins left running; on Windows they end with `rutis-host` (a Job Object), without their cleanups.

| Exit status | Meaning |
| --- | --- |
| 0 | Ended normally, including after a signal once every cleanup finished. |
| 1 | Could not start or run: invalid arguments or configuration, a runtime failed to start, a row failed to load; for `check`, a row cannot run. |
| 2 | Stopping did not finish: the deadline passed or another signal arrived while stopping, or every cleanup finished but the runtime processes did not exit in time (what is left of the deadline, at least 3 seconds). In these cases `rutis-host` ends the runtime processes and the processes they started. |

Status 2 is separate from 1 so that process managers and scripts can tell a configuration problem from a plugin whose cleanup did not finish. It is also below 126; shells use 126 and above for a program that could not be executed and for one ended by signal n (128 + n).

On Unix, a signal `rutis-host` was started ignoring (SIGHUP under `nohup`, for example) stays ignored. The runtime processes are in process groups of their own, so Ctrl-C in a terminal reaches only `rutis-host`, which unloads the rows. On Windows, they are started in console process groups of their own, which ignore Ctrl-C; Ctrl-Break and closing the console still reach them, and their cleanups may then not run, so use Ctrl-C to stop. When the console closes, Windows also ends `rutis-host` after a few seconds, whatever the deadline.

## Output

The standard output and standard error of the runtime processes (Node, Bun, Python, Go) are not written straight onto `rutis-host`'s: `rutis-host` reads them through pipes and writes them a whole line at a time — a line a plugin writes to stdout comes out on `rutis-host`'s stdout, one written to stderr on its stderr, unchanged, with no prefix. `rutis-host` writes its own status lines a whole line at a time too. So a plugin's line and a line of `rutis-host`'s, or lines of two runtimes, never run into each other, also when the plugin writes a line in pieces (Python's `print` writes the text, then the newline) and when stdout and stderr are the same file (`2>&1`).

- A line comes out when its newline comes; nothing is flushed after a time: a prompt without a newline, or a progress bar redrawn with `\r`, shows when the line ends. When a runtime ends, a last line without a newline comes out as a line of its own.
- A line longer than 64 KiB without a newline comes out in pieces.
- The plugins in one runtime process share that process's output: two plugins in the same process that each write half a line still run together.
- A runtime process's stdout and stderr are pipes, not a terminal: libraries that decide on colours and the like by whether they write to a terminal write as for no terminal.
- The pipes are always read: a plugin that writes a lot is not held up by a full pipe; it waits, as before, only while `rutis-host`'s own output cannot be written (a paused terminal, a reader that does not read).
- `rutis-host` writes the rest of the runtimes' output before it exits. A process a runtime started that is still running after the runtime exited holds that output too; `rutis-host` waits for it at most 2 more seconds, and what it writes after that does not come out.

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
| `bun` | `project` (default `.`), `runtime`, `program` | Starts the Bun runtime, with rows named `bun:<module>`. Modules resolve from `project` (packages in `node_modules`, `./relative-paths`). Install `@arcships/rutis-bun` there or specify it with `runtime`; `program` is the Bun executable, by default `bun` on `PATH`. See [Write a Bun plugin](bun-plugin.en.md). |
| `py` | `project` (default `.`), `python` | Starts the Python runtime. The interpreter defaults to `$VIRTUAL_ENV/bin/python`, then `<project>/.venv/bin/python`, then `python3` (on Windows, `Scripts\python.exe` and `python`). Its environment must contain `rutis` and the plugins. |
| `go` | `dir`, `binaries`, `start` (`on-demand` by default / `eager`), `idle` (seconds, default 60), `project` (default `.`) | Go plugins: one runtime process per binary. Every executable in `dir` that contains the Go SDK's marker counts (the directory is trusted as a whole and should be dedicated); `binaries` lists single files. A runtime is named after its file (`netkit` → `go-netkit`). An on-demand runtime starts when a row first uses it and stops after `idle` seconds without one; `eager` starts all and keeps them running (not together with `idle`). A replaced binary takes effect when the host restarts. |
| `remote` | `name`, `language`, `listen`, `instances` | A runtime on another machine: connected through a node row with `"runtime": "<name>"`, see [Connect nodes](nodes.en.md); or, with `listen` and `instances`, one that connects in, see below. |

If a runtime package is missing, startup fails and prints an installation command.

#### Runtimes that connect in

With `listen` (the name of a listener under `listen`) and `instances` (the id of a top-level `instanced` group), rutis does not connect to the runtime: the runtime connects to that listener. Any number of connections may come in, and each is an instance of the group:

```json
{
  "listen": [{ "name": "public", "address": "0.0.0.0:7443", "cert": "server.pem", "key": "server.key" }],
  "runtimes": {
    "py": { "project": "." },
    "remote": [{ "name": "phone", "language": "python", "listen": "public", "instances": "phone" }]
  },
  "rows": [
    { "id": "phone", "group": true, "instanced": true, "config": [
      { "id": "contacts", "name": "phone:contacts" },
      { "id": "digest", "name": "py:digest" }
    ] }
  ]
}
```

- A runtime connecting presents the token `RUTIS_TOKEN_<NAME>` (the runtime's name in uppercase, `-` written as `_`; `RUTIS_TOKEN` when that is not set) and reports an id of its own (`[a-z0-9-]`). A wrong token is refused; the id only tells connections apart and is not a credential.
- Each connection is an instance of the group: created when it connects, closed when it disconnects. A newer connection with the same id takes over: the old instance closes before the new one is created.
- The group's `<name>:<plugin>` rows run, in each instance, on that instance's connection. When a connection does not have the plugin, the row fails in that instance only (`this connection does not contain plugin <plugin>`); other instances are not affected, and adding such a row is not refused because of it.
- The services a connection's plugins provide belong to its instance: several connections with the same plugin do not collide, and rows in the same instance (in any language) that use the name get their own connection's service. Rows outside the instance cannot use them; put the consumer in the group.
- No node row is needed. `listen` and `instances` go together; a listener or group that does not exist, a group that is not `instanced`, or a missing token stops the host at startup.
- `<name>:` rows belong in that group; anywhere else the row fails.

A Python runtime connects in with `python -m rutis dial:wss://… --id <id> --peer <this host's id> <project>`; see [Connect nodes](nodes.en.md#runtimes-that-connect-in). Dialing out for Node, Go and Bun comes with [#384](https://github.com/arcships/rutis/issues/384).

### `listen`

WebSocket listeners used by other nodes to connect. `cert` and `key` are a TLS certificate and private key in PEM format; they can also be supplied through `RUTIS_CERT` and `RUTIS_KEY` (the file's own come first). They go together: with only one of them the host does not start, rather than listening without TLS. A listener without TLS can bind only to a loopback address.

### `rows`

Each row runs a plugin:

| Field | Meaning |
| --- | --- |
| `id` | Row name, unique within this file. |
| `name` | Plugin identifier: an npm package (or package subpath), `py:<name>` (Python entry point or module), `bun:<module>` (a module in the Bun runtime), `go:<plugin>` (the Go binary that has it; `go-<binary name>:<plugin>` when two do), `./relative-path` (plugin file), `<remote-runtime>:<module or plugin>`, `rutis-bridge/peer` (a node; see [Connect nodes](nodes.en.md)), or `peer:<node>/<plugin>` (a plugin running on another node). |
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
| `RUTIS_TOKEN_<NODE>` | Token for a specific node (uppercase, with `-` written as `_`); takes precedence over `RUTIS_TOKEN`. For a runtime that connects in (`listen` / `instances`), by the runtime's name: every connection presents this one token. |
| `RUTIS_CA` | Additional trusted CA certificate in PEM format for `wss://` connections. |
| `RUTIS_CERT` / `RUTIS_KEY` | Listener certificate and key when they are not specified under `listen`. |

## Deployment

Place `rutis.json`, the Node project (with plugins and `@arcships/rutis-runtime` installed), the Python environment (with `rutis` and plugins installed), and the Go plugin directory (binaries for the platform) together. Run `rutis-host run /srv/app/rutis.json` under a process manager such as systemd. When `rutis-host` exits, the runtime processes it started exit as well.

When stopping, have the process manager send SIGTERM to `rutis-host` alone and let it unload the rows. By default (`KillMode=control-group`), systemd also sends SIGTERM to the runtime processes, which then exit before their cleanups run; set `KillMode=mixed`, and set `TimeoutStopSec` longer than the cleanup deadline. If `rutis-host` is killed (SIGKILL, or ending the process on Windows), it cannot unload the rows: on Windows, the runtime processes are ended with it; on Unix, they notice that their channel closed, unload their rows themselves, and exit.
