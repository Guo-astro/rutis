> Copied from `docs/guide/rutis-host.en.md` in the rutis repository (main, 2026-10-18) so this skill can be used on its own. The original is authoritative: https://github.com/arcships/rutis/blob/main/docs/guide/rutis-host.en.md

## Stopping and exit status

When `run` or `dev` receives a termination signal (SIGINT (Ctrl-C), SIGTERM or SIGHUP on Unix; Ctrl-C, Ctrl-Break or closing the console on Windows), it unloads every row and runtime: each plugin's cleanup runs once, the runtime processes exit, and then `rutis-host` exits.

Cleanups have a deadline, 10 seconds by default. Change it with `--shutdown-timeout <seconds>` or the `RUTIS_SHUTDOWN_TIMEOUT` environment variable (the flag takes precedence; fractions are allowed; it must be above 0). When the deadline passes, or a second signal arrives while stopping (pressing Ctrl-C again), `rutis-host` prints the plugins that are still stopping, ends the runtime processes and the processes they started without waiting for their cleanups, and exits at once. When starting fails, what did start is stopped the same way.

`rutis-host` passes the deadline to the runtime processes it starts (in `RUTIS_SHUTDOWN_TIMEOUT`). When `rutis-host` itself is ended without stopping them (`kill -9`, a crash), each runtime sees its connection close, unloads its rows itself within that deadline, and exits, whatever its plugins left running; on Windows they end with `rutis-host` (a Job Object), without their cleanups.

| Exit status | Meaning |
| --- | --- |
| 0 | Ended normally, including after a signal once every cleanup finished. |
| 1 | Could not start or run: invalid arguments or configuration, a runtime failed to start, a row failed to load; for `check`, a row cannot run. |
| 2 | Stopping did not finish: the deadline passed or another signal arrived while stopping, or every cleanup finished but the runtime processes did not exit in time (what is left of the deadline, at least 3 seconds). In these cases `rutis-host` ends the runtime processes and the processes they started. |
| 3 | A local runtime whose `restart` is `"exit"` ended on its own, and the host stopped (every cleanup finished); see [When a runtime ends](#when-a-runtime-ends). When stopping did not finish, 2. |

Status 2 is separate from 1 so that process managers and scripts can tell a configuration problem from a plugin whose cleanup did not finish; 3 tells them a runtime crashed and the host should be restarted. It is also below 126; shells use 126 and above for a program that could not be executed and for one ended by signal n (128 + n).

On Unix, a signal `rutis-host` was started ignoring (SIGHUP under `nohup`, for example) stays ignored. The runtime processes are in process groups of their own, so Ctrl-C in a terminal reaches only `rutis-host`, which unloads the rows. On Windows, they are started in console process groups of their own, which ignore Ctrl-C; Ctrl-Break and closing the console still reach them, and their cleanups may then not run, so use Ctrl-C to stop. When the console closes, Windows also ends `rutis-host` after a few seconds, whatever the deadline.

## Changing the configuration while it runs

While `run` runs, `rutis.json` can change without restarting the host:

- On Unix, send `rutis-host` SIGUSR1 and it reads the file again. Under systemd, `ExecReload=/bin/kill -USR1 $MAINPID` makes `systemctl reload` do so. SIGHUP is still a termination signal (the host stops when its terminal closes); it does not reload.
- With `--watch-config` (off by default), it reads the file again when its content changes: once the content has stayed the same for 500 milliseconds after a change; content the same as last read is not read again, and a file that is briefly not there (an editor deleting and creating it) is no change. Windows has no SIGUSR1, so this is the only way there; it is also the way for a Kubernetes ConfigMap update, which changes the mounted file.

Both run the same reload, all or nothing:

1. A file that cannot be read, or is not a valid configuration: refused.
2. A change to what a running host cannot change: refused, saying to restart the host. That is the top-level `format`, `id`, `listen`, `runtimes.go` (any change), `runtimes.remote` (an entry added or removed, or any field of one changed, `listen` and `instances` included), and a local runtime (`node`, `py`, `bun`) added or with changed settings.
3. The new rows are checked first (as by `check`): a row that cannot start (its plugin not found, its settings failing the checks before loading), and that is not failing now, refuses it. A row that waits for a service is not one that cannot start, as at start.
4. Then the new rows replace the old: added rows start, removed rows unload, changed rows restart; rows that did not change are left alone (rows using a service of a restarted row wait for it and come back with it, as always). A row failing after the replacement (its plugin's `apply` throwing, say) that was not failing before brings the old rows back (the rows replaced restart once more), and refuses it.
5. A local runtime removed: its rows unload first, then the runtime stops, and the reload is done once its process has exited.

When rows of an `instanced` group change, the change applies in every instance there is: an added row starts in each instance, a removed one unloads in each, a changed one restarts in each; the connections (instances) stay, and the rows in them that did not change are left alone. Instances created later run the new rows.

It prints one line for the result:

```
rutis-host: reloaded <file> (<n> rows changed)
rutis-host: reloaded <file> (1 row changed, runtime py stopped)
rutis-host: reload rejected: <reason>; nothing changed
```

Refused, the old configuration runs on; the reason names the row and what is wrong with it, or what needs the host restarted. Row state changes are printed by the state lines as usual. SIGUSR1, or a file change, while a reload runs makes one more reload once it ends; a termination signal while a reload runs stops the host as usual.

Credentials are read from the environment when a link dials, or when a far end dials in (see [Credentials](#credentials)), so a peer row a reload adds connects too. The environment does not change while the process runs: a new token still needs a restart.

## When a runtime ends

When a local runtime's (`node`, `py`, `bun`) process ends on its own (a crash, `kill -9`, a plugin making the process exit), `rutis-host` does what that runtime's `restart` says:

| `restart` | What happens |
| --- | --- |
| `"no"` (the default) | Nothing: its rows go back to `waiting for its services`, other runtimes and their rows go on; restart the host to bring it back. |
| `"on-failure"` | After a wait, it starts the runtime again, and its rows run again (loaded anew: the plugins' `apply` runs once more); prints `runtime <name>: restarting`, then `runtime <name>: running` once it runs. |
| `"exit"` | Unloads every other row (each cleanup runs once) and exits with 3, for systemd, Kubernetes or the like to restart the whole host; prints `rutis-host: runtime <name> ended (<reason>), and its restart is "exit": stopping; …` on stderr. |

The waits and the limit of `on-failure` are fixed, not configurable:

- 1 second before the first restart, doubling each time after, up to 30 seconds;
- a runtime that ran for 60 seconds or more before it ended starts over from 1 second;
- one that ends again after 5 restarts within 5 minutes is not restarted: `runtime <name>: gave up after 5 restarts within 5 minutes` is printed and it stays `down`, as with `"no"` (so a plugin that crashes its runtime as it starts is restarted 5 times at most).

Also:

- What the process held in memory is lost with the crash; a restart loses nothing more. Calls in progress have already failed explicitly and are not retried. What runs again is what plugins do as they start (`apply`), as when the host is restarted by hand.
- Once the host stops (a termination signal, the `exit` policy), no runtime is restarted; nor is one removed by [changing the configuration while it runs](#changing-the-configuration-while-it-runs).
- `restart` is a runtime setting: changing it while the host runs needs a restart.
- `dev` follows the same rules and defaults; write `"runtimes": { "py": { "restart": "on-failure" } }` in `rutis.dev.json` (for the project's own runtime, only `restart` is taken).
- Go and Swift: `runtimes.go.restart` and `runtimes.swift.restart` apply to each of their runtimes on its own (`runtime go-<name>: restarting`); a runtime stopped for being idle did not end on its own. Remote runtimes and peer rows' links reconnect by themselves and are not affected.

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
| `node` | `project` (default `.`), `runtime`, `restart` | Starts the Node runtime. Plugin packages are resolved from `project`'s `package.json`. Install `@arcships/rutis-runtime` there, specify it with `runtime`, or use the copy included with the npm distribution of rutis-host. |
| `bun` | `project` (default `.`), `runtime`, `program`, `restart` | Starts the Bun runtime, with rows named `bun:<module>`. Modules resolve from `project` (packages in `node_modules`, `./relative-paths`). Install `@arcships/rutis-bun` there or specify it with `runtime`; `program` is the Bun executable, by default `bun` on `PATH`. See [Write a Bun plugin](https://github.com/arcships/rutis/blob/main/docs/guide/bun-plugin.en.md). |
| `py` | `project` (default `.`), `python`, `restart` | Starts the Python runtime. The interpreter defaults to `$VIRTUAL_ENV/bin/python`, then `<project>/.venv/bin/python`, then `python3` (on Windows, `Scripts\python.exe` and `python`). Its environment must contain `rutis` and the plugins. |
| `go` | `dir`, `binaries`, `start` (`on-demand` by default / `eager`), `idle` (seconds, default 60), `project` (default `.`) | Go plugins: one runtime process per binary. Every executable in `dir` that contains the Go SDK's marker counts (the directory is trusted as a whole and should be dedicated); `binaries` lists single files. A runtime is named after its file (`netkit` → `go-netkit`). An on-demand runtime starts when a row first uses it and stops after `idle` seconds without one; `eager` starts all and keeps them running (not together with `idle`). A replaced binary takes effect when the host restarts. |
| `swift` | as `go` | Swift plugins (macOS): as `go`, for binaries built with the Swift SDK (`plugin-binary` → `swift-plugin-binary`); see [Swift plugins](https://github.com/arcships/rutis/blob/main/docs/guide/swift-plugin.en.md). |
| `remote` | `name`, `language`, `listen`, `instances` | A runtime on another machine: connected through a node row with `"runtime": "<name>"`, see [Connect nodes](https://github.com/arcships/rutis/blob/main/docs/guide/nodes.en.md); or, with `listen` and `instances`, one that connects in, see below. |

If a runtime package is missing, startup fails and prints an installation command.

`restart` of `node`, `py` and `bun` (`"no"`, the default; `"on-failure"`; `"exit"`) says what happens when the runtime's process ends on its own; see [When a runtime ends](#when-a-runtime-ends).

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

A Python runtime connects in with `python -m rutis dial:wss://… --id <id> --peer <this host's id> <project>`, a Go or Swift plugin binary with `<binary> dial:wss://… --id <id> --peer <this host's id> <project>`, and Node and Bun runtimes with `dial:` too; the commands are in [Connect nodes](https://github.com/arcships/rutis/blob/main/docs/guide/nodes.en.md#runtimes-that-connect-in).

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

Do not put credentials in the file; they are read from the environment when a link dials or a far end dials in:

| Environment variable | Purpose |
| --- | --- |
| `RUTIS_TOKEN` | Token presented when connecting to other nodes and checked when accepting connections. |
| `RUTIS_TOKEN_<NODE>` | Token for a specific node (uppercase, with `-` written as `_`); takes precedence over `RUTIS_TOKEN`. For a runtime that connects in (`listen` / `instances`), by the runtime's name: every connection presents this one token. |
| `RUTIS_CA` | Additional trusted CA certificate in PEM format for `wss://` connections. |
| `RUTIS_CERT` / `RUTIS_KEY` | Listener certificate and key when they are not specified under `listen`. |

The environment cannot be changed from outside the process, so these tokens cannot be revoked while it runs: to cut off a peer or a runtime that connects in, change the token and restart `rutis-host`, which drops every connection. Cutting off one identity at runtime takes the identity layer's revocation hook; see [Connect Nodes](https://github.com/arcships/rutis/blob/main/docs/guide/nodes.en.md#security).

## Deployment

Place `rutis.json`, the Node project (with plugins and `@arcships/rutis-runtime` installed), the Python environment (with `rutis` and plugins installed), and the Go plugin directory (binaries for the platform) together. Run `rutis-host run /srv/app/rutis.json` under a process manager such as systemd. When `rutis-host` exits, the runtime processes it started exit as well.

To apply a changed `rutis.json`, use `systemctl reload` (`ExecReload=/bin/kill -USR1 $MAINPID`), or run with `--watch-config`; see [Changing the configuration while it runs](#changing-the-configuration-while-it-runs).

When stopping, have the process manager send SIGTERM to `rutis-host` alone and let it unload the rows. By default (`KillMode=control-group`), systemd also sends SIGTERM to the runtime processes, which then exit before their cleanups run; set `KillMode=mixed`, and set `TimeoutStopSec` longer than the cleanup deadline. If `rutis-host` is killed (SIGKILL, or ending the process on Windows), it cannot unload the rows: on Windows, the runtime processes are ended with it; on Unix, they notice that their channel closed, unload their rows themselves, and exit.
