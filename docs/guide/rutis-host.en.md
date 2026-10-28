# rutis-host and rutis.json

`rutis-host` is a host that requires no Rust code. It assembles the core, plugin loader, local Node / Python / Go / Swift runtimes, and network connections, all driven by a `rutis.json` file. Plugin authors can use it for local development or deployment. For applications that need custom Rust services, see [Embed in a Rust application](rust-host.en.md).

## Quick start

A host for two plugins someone published: a TypeScript plugin, `@your-name/weather` on npm, which uses an `llm` service, and a Python plugin, `your-name-llm` on PyPI, whose entry point `llm` provides it. It needs Node 22 or later, and [uv](https://docs.astral.sh/uv/) with Python 3.10 or later.

1. Install `rutis-host` and the plugins, in a directory of the host's own:

```bash
mkdir app && cd app
npm init -y                                                # the Node project TypeScript plugins go in
npm install @arcships/rutis-host @your-name/weather        # rutis-host brings the Node runtime
uv venv                                                    # .venv: the environment Python plugins run in
uv pip install --python .venv/bin/python "rutis==0.17.2" your-name-llm   # the Python runtime at rutis-host's version, and the plugin
```

2. Write `rutis.json`: the runtimes to start, and a row per plugin (its `name` is the npm package, or `py:` and the entry point; `config` is the plugin's configuration):

```json
{
  "format": 1,
  "runtimes": { "node": {}, "py": {} },
  "rows": [
    { "id": "llm", "name": "py:llm" },
    { "id": "weather", "name": "@your-name/weather", "config": { "city": "Oslo" } }
  ]
}
```

3. Check it, then run it:

```bash
npx rutis-host check --strict        # every row ok, every service a row uses declared by another; exit 0
npx rutis-host run --watch-config
```

```text
llm: running
weather: running
```

Each row prints its state when it changes (also `starting`, `waiting for its services`, …; see [What `run` prints](#what-run-prints)).

4. Change the configuration while it runs: change `"Oslo"` to `"Bergen"` in `rutis.json` and save. With `--watch-config` the host reads the file again and restarts only the row that changed:

```text
rutis-host: reloaded rutis.json (1 row changed)
```

On Unix, SIGUSR1 does the same without `--watch-config`; see [Changing the configuration while it runs](#changing-the-configuration-while-it-runs).

5. Stop it with Ctrl-C: each plugin's cleanup runs, the runtimes exit, and `rutis-host` exits with 0.

A plugin not published yet installs the same way from a file: `npm install ../weather/your-name-weather-0.1.0.tgz` (what `npm pack` makes in the plugin's project), `uv pip install --python .venv/bin/python ../llm` (the plugin's project directory). The other languages: [How a host installs plugins](#how-a-host-installs-plugins).

## Installation

| Method | Command |
| --- | --- |
| npm | `npx @arcships/rutis-host …`, or install with `npm install -D @arcships/rutis-host` and run `npx rutis-host …` |
| PyPI | `uvx rutis-host …`, or install with `uv add --dev rutis-host` and run `uv run rutis-host …` |
| Binary | `rutis-host-<version>-<platform>.tar.gz` from GitHub Releases (Linux and macOS, x64 / arm64), or `.zip` for Windows x64 |
| crates.io | `cargo install rutis-host` |

The npm distribution includes the Node runtime (`@arcships/rutis-runtime`), and the PyPI distribution includes the Python runtime (`rutis`). They are used when the project does not provide its own runtime. Go and Swift plugins are compiled binaries: running them needs no other package and no Go or Swift toolchain (Swift plugins run on macOS).

## How a host installs plugins

Each language's plugins go where that language's runtime finds them, and a row names the plugin:

| Plugin | Install it with | Into | Row `name` |
| --- | --- | --- | --- |
| TypeScript, JavaScript (npm) | `npm install <package>` | the Node project (`runtimes.node.project`, by default the directory of `rutis.json`), beside `@arcships/rutis-runtime` (or the one the npm `rutis-host` brings) | the package name (`@your-name/weather`) or a subpath of it; a file: `./plugins/tool.ts` |
| Bun | `bun add @arcships/rutis-bun <package>` | the Bun project (`runtimes.bun.project`) | `bun:<package>` ([Bun plugins](bun-plugin.en.md#3-run-it-in-a-host)) |
| Python | `uv venv` once, then `uv pip install --python .venv/bin/python "rutis==<rutis-host's version>" <distribution>` (with pip: `.venv/bin/python -m pip install …`) | the environment of `runtimes.py.python` (by default `$VIRTUAL_ENV`, then `<runtimes.py.project>/.venv`); its `rutis` is the Python runtime, at rutis-host's version | `py:<entry point>` (`py:llm`), or `py:<module>` for a module file in `runtimes.py.project` |
| Go | `rutis-host go add <module>/cmd/<binary>@<version>` (with the machine's Go toolchain), or the binary for the platform from the plugin's release | `runtimes.go.dir` | `go:<plugin>` ([Go plugins](go-plugin.en.md#6-used-by-a-host)) |
| Swift (macOS) | the binary from the plugin's release | `runtimes.swift.dir` | `swift:<plugin>` ([Swift plugins](swift-plugin.en.md#7-used-by-a-host)) |
| Rust | not with `rutis-host`: a Rust application mounts its Rust plugins itself | | [Embed in a Rust application](rust-host.en.md) |

- The plugins of one language share its runtime: one Node project, one Python environment. A Python plugin depends on `rutis` with a lower bound only, so installing one does not downgrade the runtime; a TypeScript plugin keeps the SDK version it was written against, and several copies work side by side ([plugin API: versions](plugin-api.en.md#versions)).
- Declare under `runtimes` every runtime a row uses (`"node": {}`, `"py": {}`, …: an empty object takes the defaults); one left out is enabled with its defaults and a warning (see [`runtimes`](#runtimes)).
- After installing, `rutis-host check --strict` says whether every row can run and whether every service a row uses is declared by another row; in CI it fails otherwise.

## Commands

| Command | Purpose |
| --- | --- |
| `rutis-host run [rutis.json]` | Runs the configuration (reading it again while it runs on SIGUSR1 on Unix, or with `--watch-config`; see [Changing the configuration while it runs](#changing-the-configuration-while-it-runs)) and prints each row's status changes, the state of node rows' links and local runtimes that end (see [What `run` prints](#what-run-prints)). Press Ctrl-C to stop (see [Stopping and exit status](#stopping-and-exit-status)). |
| `rutis-host dev [directory]` | Runs a plugin project with its `rutis.dev.json` file and follows its changes: a changed file reloads the rows whose file it is, a changed `rutis.dev.json` is read again; a Go or Swift project is rebuilt (`go build`, `swift build`) when its sources change, restarting only its runtime (see [Developing a plugin with `dev`](#developing-a-plugin-with-dev)). |
| `rutis-host check [rutis.json] [--strict]` | Without running any plugin, checks that each row can run (see [Checking a configuration](#checking-a-configuration)) and prints its version, dependencies, provided services, and configuration schema, and lists every Go and Swift binary (runtime name, platform, SDK, plugin API, plugins; a binary of an SDK older than rutis 0.13 cannot run). Exits with a nonzero status if a row or binary cannot run. A dependency no row declares, which another row may provide at runtime, is a warning; with `--strict`, an error. Without `rutis.json`, checks the plugin project in the current directory. |
| `rutis-host status [rutis.json \| directory]` | Asks the `rutis-host` running that configuration (or that `dev` project) what it does now: each row's state and why, each service's provider, the runtimes, the calls not returned yet (see [Seeing the current state](#seeing-the-current-state)); not on Windows. |
| `rutis-host call [--config <rutis.json \| directory>] <service>.<method> [arguments…]` | Calls a service in the `rutis-host` running that configuration (or that `dev` project), as the host, and prints the result as JSON; `dev` accepts it, `run` with `--allow-call` (see [Calling a service](#calling-a-service)); not on Windows. |
| `rutis-host new <name> --lang node\|bun\|python\|go\|swift` | Creates a plugin project. |
| `rutis-host go add <module>@<version> [rutis.json]` | `go install`s a plugin binary into `runtimes.go.dir` with the machine's Go toolchain. |
| `rutis-host types [directory] [-o file]` | Writes the TypeScript declarations of the Rust services marked `#[rutis::service]` in the crate in `directory` (by default the current one) and in the crates it uses, to stdout or to `file`; see [Embed in a Rust application](rust-host.en.md#mark-the-type-rutisservice). |

## What `run` prints

The output of `run` and `dev` is read by people, scripts and tests. Its format is as follows; changes to it are treated as incompatible changes.

These status lines all go to standard error (from 0.18; standard output before), and plugins' own output goes to the stream they write it to (see [Output](#output)): `rutis-host run > plugins.log` keeps only what plugins write to standard output.

At start, each [`listen`](#listen) listener prints once the address it is actually bound to (with port 0, the port the system picked):

```
rutis-host: listening on <listener name> <address:port>
```

Then each change of a row's state prints `<row id>: <state>` (`running`, `waiting for …`, `starting`, `failed: …` (when the failed step can be said, `failed in apply: …`, `call to <service>.<method> failed: …` and so on, with the row and the location the plugin's language gave indented under it), `disabled`, `removed`, and so on; a copy inside an instance is `<row id> in PluginId(n)`).

A waiting row says what it waits for and why (from 0.17.2, #676). Waiting for its own runtime, it names only the runtime; otherwise each missing service is `<service> (<why>)`, several joined by `; `:

| Output | Meaning |
| --- | --- |
| `waiting for runtime <name> (starting)` | The row's runtime is still starting (restarting too) |
| `waiting for runtime <name> (down: <reason>)` | The row's runtime ended, the reason as in `runtime <name>: down` |
| `waiting for runtime <name> (stopped)` / `waiting for runtime <name>` | The host stopped the runtime; or it runs and is resolving its rows again (a remote runtime is written so too) |
| `waiting for lmm (no row provides it; did you mean llm?)` | No row declares it; a service with a close name is suggested (most likely a misspelling) |
| `waiting for llm (no enabled row provides it; rutis-host check says whether a disabled row does)` | No enabled row provides it, but rows that are disabled (themselves or by their group) were never resolved while the host runs, so what they provide is not known; `rutis-host check` resolves them and says whether a disabled row provides it. A close name adds `, did you mean llm?` after `it` here too |
| `waiting for llm (row model failed: <first line of the error>)` | The row that provides it failed |
| `waiting for llm (row model, which provides it, is disabled)` | The row that provides it is disabled; written so only when that row's plugin has been resolved (another row enabling the same plugin, say), otherwise as `no enabled row provides it` above |
| `waiting for llm (row model's runtime py is down)` / `… is not running yet` / `(row model waits for its runtime gpu)` | The row that provides it waits for its own runtime (a remote runtime's state the host does not know: it only says it waits) |
| `waiting for bsvc (row b waits for asvc from this row: they inject each other)` | Two rows `inject` each other's services; neither starts |
| `waiting for llm (row model waits too, for x)` | The row that provides it waits itself |
| `waiting for llm (row model is starting)` / `(row model has not provided it)` | The row that provides it is starting; or runs but has not provided it (a link row whose link is down, say) |
| `waiting for its services` | When rutis cannot say what it waits for |

When the reason changes (the provider going from starting to failed, say), another line is printed.

A local runtime (`node`, `py`, `bun`, Go's `go-<binary>`, Swift's `swift-<binary>`) whose process ends on its own, or that cannot start, prints a line, and another when it runs again; one that `rutis-host` stops (the host stopping, a Go or Swift runtime stopping when idle) prints none:

| Output | Meaning |
| --- | --- |
| `runtime <name>: down (<reason>)` | The runtime process ended; the reason says how, such as `exited with signal: 9 (SIGKILL)` or `exited with exit status: 1`; or it could not start, and the reason is the error. Its rows go back to `waiting for runtime <name> (down: …)`; other runtimes and their rows go on. |
| `runtime <name>: running` | A runtime that printed `down` runs again (a Go or Swift runtime rebuilt by `dev`). |

A runtime that stays down prints once; one that starts again and goes down again prints the line again, however briefly it ran. The reason in parentheses is for people, and its wording is not guaranteed.

The rows of a runtime that ended only wait for it, whichever plugin made its process exit; one that was still starting when the runtime ended is not reported as `failed`. A call in progress to one of them fails saying how the process ended. What the runtime itself wrote about it (an error and its stack, say) is in the output as before.

A node row's (`rutis-bridge/peer`) state only says that the row runs; whether its link is connected is printed on a line of its own, when the link's state changes:

| Output | Meaning |
| --- | --- |
| `<row id>: link connected` | The session is ready; the far end can be used. |
| `<row id>: link down (<reason>); retrying in 2.1s` | Cannot connect, or the connection broke (the far end is not up, the network is cut, the connection ended); retried with backoff after the time given. |
| `<row id>: link refused: auth-rejected (<reason>); retrying in 30s` | Refused: a wrong token, an untrusted certificate (another name, expired, unknown CA), an identity that does not match; retried after 30 seconds. |
| `<row id>: link stopped: incompatible (<reason>)` | Incompatible: the protocol, the address's form, a capability the far end lacks (such as `runtime`); not retried until the configuration changes and the row reloads. |
| `<row id>: link down (the connection ended)` | Listening side: the connection ended; it waits for the far end to connect again (a listening side does not retry, so no `retrying in`). |

- The category (`auth-rejected`, `incompatible`) comes from the link's classification of the failure, never from the error's text; the reason in parentheses is detail for people, such as what is wrong with a refused certificate, and its wording is not guaranteed.
- No token or other credential appears in the output.
- Whole seconds are written `30s`; other waits to a tenth of a second.
- States are checked every 200 ms and only changes are printed: a line the same as the last one is not printed again (the same refusal every 30 seconds is printed once), and a state lasting less than 200 ms may not be printed. Nothing is printed while a dialing side has no result yet (connecting), nor while a listening side has no far end yet.

The connections of the remote runtimes `runtimes.remote` connects itself (`dial`, `listen`, `listen` + `instances`) print lines starting `runtime <name>`:

| Output | Meaning |
| --- | --- |
| `runtime <name>: link connected`, … | One runtime (`dial`, `listen`): its link's state, each line as the node row's in the table above, prefixed `runtime <name>` instead (a node row of the old form still uses its row id) |
| `runtime <name>: connected <id>` | Many: a connection came in and is an instance |
| `runtime <name>: disconnected <id> (<reason>)` | Many: the connection ended (or a newer one with the same id took over); its instance closed |
| `runtime <name>: refused (<reason>)` | Many: a connection was refused before it became an instance, such as a runtime not speaking the row contract (`lacks rows.v2 and hosts`); a connection with a wrong token is refused in its handshake and not printed |

A listener that fails in an `emit` (the emitter does not wait, so nobody else hears of it) prints one line on stderr, where it ran: `rutis: listener of <name> in row <row> failed: <ErrorName>: <message>` for the host's own, the same line from the runtime for a runtime's; a call the host could not deliver names where it went (`in runtime <name>`, `in node <peer>`). A failing filter of an `emit` prints `rutis: filter of <name> from row <row> failed: …` (`from the mount` for a plugin a mount loaded).

### Events

Every row's listeners, in any language and on linked nodes, are on one event bus, with one set of rules ([Unified events](../design-events-2026-10-18.en.md)): nothing is declared in `rutis.json`. An `instanced` group only affects the scope chain in descriptors; events are delivered by name to every listener on the bus, so filter by instance when needed. `rutis-host` also emits, and plugins of every language may listen to:

| Event | When |
| --- | --- |
| `rutis/row` | each state change of a row: `{ row, plugin, scope, state: "pending" \| "running" \| "failed" \| "stopped", error }` |
| `rutis/runtime` | a runtime runs (`running`) or ended (`down`, with `reason`) |
| `rutis/service` | a service a runtime's row provides appeared, was replaced or went (`available`) |

A runtime of rutis 0.12 or older (or a node) is refused when it connects: `incompatible session: the far end (…) does not declare events-v2 (the event protocol of rutis 0.13): upgrade it to rutis 0.13 or later`.

## Checking a configuration

For each row (except disabled ones), `check` verifies that:

- its plugin is found and resolves;
- the row passes the checks made before loading (scopes, and the config of built-in plugins such as link rows);
- every service it waits for (its plugin's `inject`, plus the row's own `inject`) is provided by another row: as plugins declare in `provides`, and a link row provides what it `import`s.

A row waiting for a service no row provides, and no row may provide at runtime, would wait forever: `check` says which service is missing and what to do, and exits with 1; when only disabled rows provide it, it names them. What some rows provide is not known during the check (runtimes on other machines, which the check does not connect to, and `peer:` rows); a service missing then is only reported as possibly coming from them, not as a failure.

What a plugin `ctx.provide`s reaches rutis without a declaration (from 0.12, every plugin of the four SDKs, one declaring `provides` too, which may provide more; also a Cordis row without `rutis.provides`), so it is known only when the plugin runs. A service no row declares, while there are such rows, is a warning on stderr, and `check` still exits with 0:

```
rutis-host: warning: <row>: nothing declares <service>; it may be provided at runtime by <rows>, …
```

At most three rows are named, the rest as `and N more`; when the row and one of those rows are Cordis rows, the line goes on with `; between Cordis rows of one runtime it resolves in Cordis`: dependencies between Cordis rows of one Node runtime resolve in Cordis. A declared name counts as provided, as before.

When the only row declaring a service is disabled, the warning names that row, not the rows that may provide it at runtime:

```
rutis-host: warning: <row>: <service> is declared only by row <disabled row>, which is disabled: enable it, or add a row that provides <service>
```

When a missing name is a character or two from a declared one, the warnings above and the line saying what to do about a missing service end with `; did you mean <name>?`.

`check --strict` makes these warnings errors: stderr says `rutis-host: error: …`, the row says `cannot run with --strict: nothing declares <service>` (`only disabled rows declare <service>` for one only disabled rows declare), and the exit status is 1. Use it to make sure before deploying (in CI, say) that every dependency has a declared provider.

In two more cases every row says `ok` but some rows will not start, and `check` reports an error on stderr and exits with 1 (only a warning in 0.17.2):

```
rutis-host: error: rows a, b wait for each other (a waits for bsvc from b, b waits for asvc from a): none of them starts; break the cycle in their inject
rutis-host: error: rows one, two all provide llm: only the first to start provides it, the others fail; keep one, or isolate the name in each row
```

- Rows whose `inject` wait for each other's services (a cycle): none of them starts. Only services one row alone provides count: with another provider a row may start from that one, which is no cycle.
- Rows declaring the same service in one scope: only the first to start provides it, the others fail. Rows that `isolate` the name, each in a scope of its own, do not count. Rows that can never start (waiting for a service no row provides) do not count either.

Rows using a language whose runtime `runtimes` does not declare enable that runtime with its defaults, with a warning (see [`runtimes`](#runtimes)).

### The config of each row

From 0.18 `check` fills each row's config with its plugin's defaults and validates it against the plugin's JSON Schema, by the rules every language's SDK applies when it loads the row (the keywords checked are in the [plugin API](plugin-api.en.md#configuration)). A config that does not match cannot run: one line per problem, sorted by path, exit status 1:

```
weather (weather): cannot run: config: units should be "metric" or "imperial", got "kelvin"
weather (weather): cannot run: config: home.lat should be a number, got "north"
```

A key the schema does not name, where it allows extra keys (an object with `properties` and no `additionalProperties`), is a warning on stderr, with a close known key suggested; the row still runs and its plugin gets the key's value as it is. With `--strict` it is an error, and the row `cannot run with --strict`:

```
rutis-host: warning: weather: config: cty is not a config key (did you mean city?)
```

- Cordis rows (Cordis plugins exporting `Config`) are validated by Cordis with their own `Config` when they load; `check` prints their schema and does not check their config.
- Rows whose schema is known only when they run (remote runtimes not connected, a `peer:` row whose peer is not there) say `config not checked: its schema is known only when it runs`, which is not a warning.
- Go binaries built with an SDK older than 0.18 list every field without `omitempty` as `required` in their manifest; `check` ignores their `required` (the rest is checked).

`run` and `dev` apply the same rules: a row whose config does not match fails (`weather: config: …`, a line per problem), and rows waiting for it say so (`chat: waiting for weather (row weather failed: config: …)`); extra keys are warned about once per row and config value. Runtimes built with an SDK older than 0.18 do not check configs when they load rows; `check` checks them all the same.

## Stopping and exit status

When `run` or `dev` receives a termination signal (SIGINT (Ctrl-C), SIGTERM or SIGHUP on Unix; Ctrl-C, Ctrl-Break or closing the console on Windows), it unloads every row and runtime: each plugin's cleanup runs once, the runtime processes exit, and then `rutis-host` exits.

Cleanups have a deadline, 10 seconds by default. Change it with `--shutdown-timeout <seconds>` or the `RUTIS_SHUTDOWN_TIMEOUT` environment variable (the flag takes precedence; fractions are allowed; it must be above 0). When the deadline passes, or a second signal arrives while stopping (pressing Ctrl-C again), `rutis-host` prints the plugins that are still stopping, ends the runtime processes and the processes they started without waiting for their cleanups, and exits at once. When starting fails, what did start is stopped the same way.

`rutis-host` passes the deadline to the runtime processes it starts (in `RUTIS_SHUTDOWN_TIMEOUT`). When `rutis-host` itself is ended without stopping them (`kill -9`, a crash), each runtime sees its connection close, unloads its rows itself within that deadline, and exits, whatever its plugins left running; on Windows they end with `rutis-host` (a Job Object), without their cleanups. Also when a plugin keeps a runtime busy (a handler that never returns, so the runtime can run nothing else): a second after the deadline the runtime ends itself, without the cleanups it could not run. The deadline counts from when the runtime sees its connection close, which it sees on a thread of its own (the Node and Bun runtimes' I/O thread, which tells a watchdog thread; the Python runtime's thread that reads its connection).

| Exit status | Meaning |
| --- | --- |
| 0 | Ended normally, including after a signal once every cleanup finished. |
| 1 | Could not start or run: invalid arguments or configuration, a runtime failed to start, a row failed to load; for `check`, a row cannot run. |
| 2 | Stopping did not finish: the deadline passed or another signal arrived while stopping, or every cleanup finished but the runtime processes did not exit in time (what is left of the deadline, at least 3 seconds). In these cases `rutis-host` ends the runtime processes and the processes they started. |
| 3 | A local runtime whose `restart` is `"exit"` ended on its own, and the host stopped (every cleanup finished); see [When a runtime ends](#when-a-runtime-ends). When stopping did not finish, 2. |

Status 2 is separate from 1 so that process managers and scripts can tell a configuration problem from a plugin whose cleanup did not finish; 3 tells them a runtime crashed and the host should be restarted. It is also below 126; shells use 126 and above for a program that could not be executed and for one ended by signal n (128 + n).

On Unix, a signal `rutis-host` was started ignoring (SIGHUP under `nohup`, for example) stays ignored. The runtime processes are in process groups of their own, so Ctrl-C in a terminal reaches only `rutis-host`, which unloads the rows. On Windows, they are started in console process groups of their own, which ignore Ctrl-C; Ctrl-Break and closing the console still reach them, and their cleanups may then not run, so use Ctrl-C to stop. When the console closes, Windows also ends `rutis-host` after a few seconds, whatever the deadline.

## Changing the configuration while it runs

While `run` runs, `rutis.json` can change without restarting the host (`dev` reads `rutis.dev.json` again the same way; see [Developing a plugin with `dev`](#developing-a-plugin-with-dev)):

- On Unix, send `rutis-host` SIGUSR1 and it reads the file again. Under systemd, `ExecReload=/bin/kill -USR1 $MAINPID` makes `systemctl reload` do so. SIGHUP is still a termination signal (the host stops when its terminal closes); it does not reload.
- Installed with npm, the process you start (`npx rutis-host run`, and so systemd's `MAINPID`) is the launcher `bin/rutis-host.mjs`, which runs the `rutis-host` binary: on Unix it passes SIGUSR1 and SIGTERM on to it, so the reload above and stopping work as with the binary (and Node opens no debugger for SIGUSR1). It does not pass on SIGINT and SIGHUP: a terminal sends those to the whole process group, the binary included, which would otherwise get them twice. A SIGINT sent to the launcher alone (`kill -INT <its pid>`) therefore does nothing; to stop it from a script, send SIGTERM, or SIGINT to the process group.
- With `--watch-config` (off by default), it reads the file again when its content changes: once the content has stayed the same for 500 milliseconds after a change; content the same as last read is not read again, and a file that is briefly not there (an editor deleting and creating it) is no change. Windows has no SIGUSR1, so this is the only way there; it is also the way for a Kubernetes ConfigMap update, which changes the mounted file.

Both run the same reload, all or nothing:

1. A file that cannot be read, or is not a valid configuration: refused.
2. A change to what a running host cannot change: refused, saying to restart the host. That is the top-level `format`, `id`, `listen`, `runtimes.go` and `runtimes.swift` (any change), `runtimes.remote` (an entry added or removed, or any field of one changed, `listen` and `instances` included), and a local runtime (`node`, `py`, `bun`) added or with changed settings.
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

Refused, the old configuration runs on; the reason names the row and what is wrong with it, or what needs the host restarted. Refused because a row's config does not match its schema, a line per problem comes first, then the rejection:

```
weather: config: units should be "metric" or "imperial", got "kelvin"
rutis-host: reload rejected: row weather's config does not match its plugin's schema; nothing changed
```

Row state changes are printed by the state lines as usual. SIGUSR1, or a file change, while a reload runs makes one more reload once it ends; a termination signal while a reload runs stops the host as usual.

Credentials are read from the environment when a link dials, or when a far end dials in (see [Credentials](#credentials)), so a peer row a reload adds connects too. The environment does not change while the process runs: a new token still needs a restart.

## Developing a plugin with `dev`

`rutis-host dev [directory]` runs the plugin project in the directory (the current one by default) as rows: the project's own plugin, configured by the row of `rutis.dev.json` with the same id, and the other rows that file adds (fakes of the services the plugin uses, other plugins). It looks at the project's files every 0.4 seconds (not `node_modules`, `.venv`, `venv`, `target`, `dist`, `build`, `__pycache__`, nor hidden files and directories other than `.env`) and follows them:

- A changed file that is a row's module (`./dev/fake-llm.ts`; for a Python row `py:fake_llm`, the module's file `fake_llm.py` or `fake_llm/__init__.py` in the Python runtime's project) reloads that row. Any other changed file of the project (the plugin's other modules, `package.json`, `pyproject.toml`) reloads the project's own row. Rows whose files did not change are left alone.
- In a Go or Swift project, a change to its sources (`.go`, `go.mod`, `go.sum`; Swift sources, `Package.swift`, `Package.resolved`) rebuilds it and restarts its runtime on the new build; a failed build keeps the last one running. In a Go project, a row of `rutis.dev.json` whose `name` is a directory of the project (`./dev/fakellm`, a main package of the same module serving one plugin) is a fake written in Go: built into a binary of its own and run as its own runtime (`go-fakellm`); `check` in the project builds it too, and `rutis.json` has no such rows. A change to the sources builds each binary again, and restarts only the runtimes whose build changed (see the [Go guide](go-plugin.en.md#4-run-it-in-a-local-host)). A change to the module of another row (a Python or TypeScript row of `rutis.dev.json`) reloads that row without a rebuild; a change to any other file does nothing.
- `rutis.dev.json` is read again when its content changes, by the rules of `run --watch-config` (all or nothing; what a running host cannot change is refused, saying to restart; see [Changing the configuration while it runs](#changing-the-configuration-while-it-runs)); on Unix, SIGUSR1 does so too. Rows whose entry changed restart with it, added rows start, removed rows unload.
- Rows that use a service of a row that reloaded restart with it, as they always do when a service they use goes and comes back.
- A local runtime (`node`, `py`, `bun`) whose process ended and that nothing restarts (`restart` is `"no"`, the default, or `"on-failure"` gave up) starts again at the next change to one of its rows (a file or its entry in `rutis.dev.json`), with the code as it is then; its rows come back with it. A change to other rows does not start it. One that its `restart` starts again (`"on-failure"`, not given up), or that is starting, is left to it: its rows start with the code as it is then.
- A reload imports again only the row's own module, not the modules it imports: after changing one of those, restart `dev`.
- A reload whose new code does not import (a syntax error, an error while importing it) changes nothing: the row runs the version it ran, also when it restarts later for another reason (a service it uses reloading), until a change that imports. New code that imports but whose `apply` fails is the code the row runs from then on: the row fails with that error and does not run until a change fixes it.

`dev --debug` runs the project's language runtime under its debugger and prints how to connect (`rutis-host dev: debugging: …`): the Python runtime reads this terminal, and `breakpoint()` stops in pdb; the Node runtime's inspector is on `127.0.0.1:9229`, Bun's on `ws://127.0.0.1:6499/rutis`; a Go project is built without optimisation and its runtime runs under Delve, listening on `127.0.0.1:2345` (install `dlv` first). The same holds for Node, Bun, Python and Go runtimes that `rutis.dev.json` adds (an added Go binary runs as it was built; Delve's address is fixed, so there can be one Go runtime); when a debugger it needs is not installed, `dev` does not start. A Swift project does not take `--debug`. How to use each: the [TypeScript](typescript-plugin.en.md#debugging), [Bun](bun-plugin.en.md#debugging), [Python](python-plugin.en.md#debugging) and [Go](go-plugin.en.md#debugging) guides.

Besides the lines of [`run`](#what-run-prints), `dev` prints:

| Output | Meaning |
| --- | --- |
| `<row id>: reloaded (<files> changed)` | The row runs the new code; the files are relative to the project. |
| `<row id>: cannot reload: <reason>; the previous version keeps running` | The new code does not import; the row runs on the version it ran. When the new code made another row fail, the reason names that row first: `<other row> failed with it: …`. Without `; the previous version keeps running` (its new `apply` failing, say), the row does not run now. |
| `<row id>: restarted (<other row> reloaded)` | The row uses a service of the other row, and restarted with it. The cause may also be `<row> updated`, `added` or `removed` (by `rutis.dev.json`), `<Go or Swift runtime> restarted`, or `<row> restarted`: a row it uses that restarted with the change (`caller: restarted (greeter restarted)` when `llm` reloaded and `greeter` uses it). The cause named is always a row the row uses; when that is not known (a dependency inside a runtime, between Cordis rows), no cause is named: `<row id>: restarted`. `started` or `stopped` instead of `restarted` when the row was not running before, or is not running now. |
| `rutis-host: reloaded rutis.dev.json (<n> rows changed)` | `rutis.dev.json` was read again; each row that changed follows: `<row id>: updated (its row in rutis.dev.json changed)`, `<row id>: added`, `<row id>: removed`. Refused: `rutis-host: reload rejected: <reason>; nothing changed`. |
| `runtime <name>: starting again (<files> changed)` | A runtime that was down starts again for the change; `runtime <name>: running` follows when it runs, or `runtime <name>: down (<reason>)` again. When its `restart` starts it again: `runtime <name>: down; its restart starts it again, its rows with the code as it is then (…)`; when it is starting: `runtime <name>: starting; its rows start with the code as it is then (…)`. |
| `rutis-host dev: services: forecast (weather), llm (llm)` | Once started (every row running or failed), and after each reload, rebuild or reading of `rutis.dev.json`, the services now and the rows providing them (as `services` in `rutis-host status`), by name; one the host provides as `host`; none as `rutis-host dev: services: none`. Printed every time, not compared with the last. It starts with `rutis-host dev: `, not in the form `<row>: <state>`, so scripts reading the state lines are not affected; `run` does not print it: use `rutis-host status` |
| `<project>: building`, `<project>: built in 1.2s`, `<runtime>: rebuilt and restarted` | A Go or Swift project was rebuilt and its runtime restarted on the build. When the build fails: `<project>: the build failed; the last build keeps running`, followed by the compiler's errors; when the new build does not run: `<runtime>: cannot restart: <reason>; the previous build is kept`. |

## When a runtime ends

When a local runtime's (`node`, `py`, `bun`, each Go or Swift runtime) process ends on its own (a crash, `kill -9`, a plugin making the process exit), `rutis-host` does what that runtime's `restart` says:

| `restart` | What happens |
| --- | --- |
| `"no"` (the default) | Nothing: its rows go back to `waiting for runtime <name> (down: …)`, other runtimes and their rows go on; restart the host to bring it back. |
| `"on-failure"` | After a wait, it starts the runtime again, and its rows run again (loaded anew: the plugins' `apply` runs once more); prints `runtime <name>: restarting`, then `runtime <name>: running` once it runs. |
| `"exit"` | Unloads every other row (each cleanup runs once) and exits with 3, for systemd, Kubernetes or the like to restart the whole host; prints `rutis-host: runtime <name> ended (<reason>), and its restart is "exit": stopping; …` on stderr. |

The waits and the limit of `on-failure` are fixed, not configurable:

- 1 second before the first restart, doubling each time after, up to 30 seconds;
- a runtime that ran for 60 seconds or more before it ended starts over from 1 second;
- one that ends again after 5 restarts within 5 minutes is not restarted: `runtime <name>: gave up after 5 restarts within 5 minutes` is printed and it stays `down`, as with `"no"` (so a plugin that crashes its runtime as it starts is restarted 5 times at most).

Also:

- What the process held in memory is lost with the crash; a restart loses nothing more. Calls in progress have already failed explicitly and are not retried. What runs again is what plugins do as they start (`apply`), as when the host is restarted by hand.
- A row is not blamed for its runtime ending: one that was still starting in it waits for it as the others do, and runs again with them.
- Once the host stops (a termination signal, the `exit` policy), no runtime is restarted; nor is one removed by [changing the configuration while it runs](#changing-the-configuration-while-it-runs).
- `restart` is a runtime setting: changing it while the host runs needs a restart.
- `dev` follows the same rules and defaults; write `"runtimes": { "py": { "restart": "on-failure" } }` (or `"go"`, `"swift"`, …) in `rutis.dev.json` (for the project's own runtime, only `restart` is taken). A runtime that stays down there also starts again at the next change to one of its rows (see [Developing a plugin with `dev`](#developing-a-plugin-with-dev)).

- Go: `runtimes.go.restart` applies to every Go runtime, each on its own (its own waits and limit; `runtime go-<name>: restarting`). A runtime stopped for being idle did not end on its own; nor did one whose binary's handshake was refused (an old SDK), which is not restarted until its file changes. Whatever `restart` says, a crashed Go runtime also starts again when a row uses it once its binary changed. Changing `runtimes.go` while the host runs needs a restart, as before. `runtimes.swift.restart` is the same for the Swift runtimes (`runtime swift-<name>: restarting`).
- Remote runtimes and peer rows' links reconnect by themselves and are not affected.

## Seeing the current state

While `run` or `dev` runs, run `rutis-host status` in another terminal in the same directory (or `rutis-host status path/to/rutis.json`, `rutis-host status <the dev project's directory>`) to print the host's state now, once. It answers while starting is stuck too (a row's `apply` waiting on a call that does not return, before `run` prints any state line):

```
rows:
  slow: running
  caller: starting
services:
  slow: slow
runtimes:
  node: running (pid 4242)
  py: running (pid 4243)
calls:
  caller → slow.wait  42s
connections:
  gpu: link connected
  worker: worker-1, worker-2
```

- `rows`: each row (a row inside instances once per instance, as `<row> in <group> #<number>`) and its state: `running`, `starting`, `stopped`, `disabled`, and `waiting` for a waiting row when rutis cannot say for what; a waiting row says what it waits for (in the words of `run`'s waiting lines); a failed row, the first line of its failure. When a runtime exits, its rows show at once that they wait for it.
- `services`: each service by name and the row providing it; one the host (Rust) provides says `host`.
- `runtimes`: each runtime's state (`running`, `starting`, `down`, `stopped`), how one that exited ended, and a local runtime's process id.
- `calls`: the calls into a row's service that have not returned: who calls (`host` for one no row made), which service and method, for how long; `(row <row>)` when the row called is named otherwise than the service. Not seen: calls within one runtime, and calls of functions passed by reference (only "host → row" is seen, without a method name); a call an in-process Rust plugin makes is listed as from `host`.
- `connections`: the remote runtimes `runtimes.remote` connects itself (the part is left out when there are none). One runtime: its link's state (as `run`'s link lines; before it connects, `connecting` or `waiting for it to connect`); many: the ids connected now (`none connected` when none is).
- rutis sets no time limit on calls: how long to wait is the caller's decision (how each language limits a call: [plugin API](plugin-api.en.md)). This only shows which call has waited how long.

`run` and `dev` open a local socket only the user can use (mode 0600, inside the user's own 0700 directory) that answers `status`, which changes nothing, and `rutis-host call` (`dev` accepts it, `run` with `--allow-call`; see [Calling a service](#calling-a-service)). It lives under `rutis-host-<uid>/` in the system's runtime directory (`$XDG_RUNTIME_DIR`, else the temporary directory; `/tmp` when that path would be too long), named by a hash of the configuration's path, so nothing is written into the project; the host removes it when it ends (and the directory, when no other host's socket is in it); a host killed outright (`kill -9`) cannot, and the next host for the same configuration replaces it. `status` finds the directory by its own environment: run as another user, with `sudo`, or from a terminal with other variables, it looks elsewhere and says no host runs. When another host already answers for the same configuration, the later one runs as usual with a warning. Not on Windows in 0.18.

When stopping does not finish by the deadline (see [Stopping and exit status](#stopping-and-exit-status)), the calls not returned are listed too, after the plugins still stopping: `call not returned: caller → slow.wait  12s`.

## Calling a service

While `run` or `dev` runs, in another terminal, `rutis-host call` calls a method of a service, to see what it returns and what it says when it fails, without writing a plugin to call it:

```console
$ rutis-host call forecast.today '"Oslo"'
"sunny in Oslo"
$ rutis-host call forecast.week '{"days": 3}'
{
  "days": 3,
  ...
}
```

- `<service>.<method>` splits at the last `.` (a service's name may have dots). Each argument is a JSON value: `3`, `true`, `null`, `'"Oslo"'`, `'{"days": 3}'`, `'[1, 2]'`; a string needs its quotes, and an argument that is not JSON is an error, never taken for a string. Values JSON cannot write (dates, binary data, maps) cannot be passed from the command line: try such a method in a test.
- It finds the host as `rutis-host status` does: the `rutis.json` here (in a plugin project, the project `dev` runs), or the one `--config <rutis.json | directory>` names. It only reaches a running host, and starts none.
- It calls the service the root scope sees by that name, whichever language, runtime or node provides it, as [calling a service by name from Rust](rust-host.en.md#call-a-service-by-name-from-rust) does. The call comes from no row: `rutis-host status` shows it as `host → forecast.today (row weather)  3s`.
- The result is printed as JSON indented by two spaces; no result is `null`. A value JSON does not have is written as its text: points in time, dates, big integers, decimals and UUIDs as strings (`"2026-10-27"`), binary data, maps, sets and references as what they are (`"<a map>"`); for people to read, not to pass back.
- No time limit: it waits for the result; Ctrl-C ends `call` (exit status 130), and the host drops the call.
- On an error it writes it on stderr and exits with 1:

| Case | Output |
| --- | --- |
| An argument that is not JSON | `rutis-host: argument 1 is not JSON: write a string in quotes, as '"Oslo"'` |
| No method named | `rutis-host: name a method, as forecast.<method>` |
| No such service | `rutis-host: no row provides forcast; did you mean forecast? (services: forecast, llm)` |
| The row providing it is not running | `rutis-host: forecast is not available (row weather waits for its runtime go-weather)` |
| No such method | `rutis-host: forecast has no method tomorow (methods: today, week)` |
| An argument of the wrong type | `rutis-host: forecast.today: argument 1 should be a string, got 3` |
| The method throws | `rutis-host: call to forecast.today failed: KeyError: 'region'`, then the row it failed in (`in weather:`) and its stack |
| The method takes the calling row's `&Ctx` | `rutis-host: tools.register needs a calling row; call it from a plugin, not from rutis-host call` |
| No host running | `rutis-host: no rutis-host run or dev is running for /work/weather (start one, or name its configuration with --config)` |
| `run` without `--allow-call` | `rutis-host: this rutis-host run does not accept calls; start it with rutis-host run --allow-call` |

`call` runs any method of any service, so `dev` accepts it and `run` does not by default: to use it in production, add `--allow-call` to the command that starts it (on the command line only, never in `rutis.json`). Who can connect is as for `status`: only the user can use the socket. Not on Windows (`rutis-host call is not available on Windows`).

## Output

The standard output and standard error of the runtime processes (Node, Bun, Python, Go, Swift) are not written straight onto `rutis-host`'s: `rutis-host` reads them through pipes and writes them a whole line at a time — a line a plugin writes to stdout comes out on `rutis-host`'s stdout, one written to stderr on its stderr. `rutis-host` writes its own status lines to stderr, a whole line at a time too. So a plugin's line and a line of `rutis-host`'s, or lines of two runtimes, never run into each other, also when the plugin writes a line in pieces (Python's `print` writes the text, then the newline) and when stdout and stderr are the same file (`2>&1`).

Each line a plugin writes can start with the name of the row that wrote it (from 0.18, #678):

```text
[weather] fetching forecast for Berlin
[go-net-probe] probing 10.0.0.1
```

| | Default | Switch |
| --- | --- | --- |
| `rutis-host dev` | on | `--no-prefix-output` |
| `rutis-host run` | off: plugins' output as written | `--prefix-output` |

- The rows of a Node, Bun or Python runtime share its process, so the runtime adds the name of the row running; output whose row cannot be told (a thread a Python plugin started itself, code that runs in no row) starts with the runtime's name (`[py] `). Output written to the file descriptor itself (a child process a plugin started, `os.write`, a native module) has none. The rules are in the [plugin API](plugin-api.en.md#output).
- Go and Swift runtimes run one plugin per process: `rutis-host` adds the runtime's name as it reads their output (`[go-<binary>] `, `[swift-<binary>] `); the SDKs are unchanged.
- `rutis-host`'s own lines and the runtimes' own lines (`runtime py: …`) have none.
- Under `dev --debug` the Python runtime reads this terminal (pdb): its output does not go through a pipe, and has no name and is not held until a line ends; Node, Bun and Go are prefixed as usual.

In the Node, Bun and Python runtimes, when a service method holds the runtime for 5 seconds in one go, the runtime says once on stderr which row's which method it is, that the other rows wait, and what to change (`runtime py: forecast.tomorrow of row weather has held the runtime for 5 s, …`, see the [plugin API](plugin-api.en.md#plugins-in-one-runtime-share-its-process)). It is only said, never interrupted; under `dev --debug` nothing is said.

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
    "swift": { "dir": "plugins/swift", "restart": "on-failure" },
    "remote": [{ "name": "gpu", "language": "python" }]
  },
  "listen": [
    { "name": "public", "address": "0.0.0.0:7443", "cert": "tls/server.pem", "key": "tls/server.key" }
  ],
  "rows": [
    { "id": "weather", "name": "weather-plugin", "config": { "city": "Oslo" } },
    { "id": "llm", "name": "py:llm_gateway" },
    { "id": "dns", "name": "go:dns" },
    { "id": "calendar", "name": "swift:calendar" },
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
| `bun` | `project` (default `.`), `runtime`, `program`, `restart` | Starts the Bun runtime, with rows named `bun:<module>`. Modules resolve from `project` (packages in `node_modules`, `./relative-paths`). Install `@arcships/rutis-bun` there or specify it with `runtime`; `program` is the Bun executable, by default `bun` on `PATH`. See [Write a Bun plugin](bun-plugin.en.md). |
| `py` | `project` (default `.`), `python`, `restart` | Starts the Python runtime. The interpreter defaults to `$VIRTUAL_ENV/bin/python`, then `<project>/.venv/bin/python`, then `python3` (on Windows, `Scripts\python.exe` and `python`). Its environment must contain `rutis` and the plugins. |
| `go` | `dir`, `binaries`, `start` (`on-demand` by default / `eager`), `idle` (seconds, default 60), `project` (default `.`), `restart` | Go plugins: one runtime process per binary. Every executable in `dir` that contains the Go SDK's marker counts (the directory is trusted as a whole and should be dedicated); `binaries` lists single files. A runtime is named after its file (`netkit` → `go-netkit`). An on-demand runtime starts when a row first uses it and stops after `idle` seconds without one (objects it handed out and that are held elsewhere do not count as use: once it stopped, calls on them fail with `peer-gone`); `eager` starts all and keeps them running (not together with `idle`). A replaced binary takes effect when the host restarts. |
| `swift` | as `go` | Swift plugins (macOS): as `go`, for binaries built with the Swift SDK (`plugin-binary` → `swift-plugin-binary`); see [Swift plugins](swift-plugin.en.md). |
| `remote` | `name`, `language`, `dial`, `listen`, `instances`, `transport` | A runtime on another machine, written whole in one entry: `dial` has rutis dial it, `listen` waits for it to connect in, `listen` + `instances` makes each connection an instance; see [Remote runtimes](#remote-runtimes) below. With none of them, it is connected through a node row with `"runtime": "<name>"` (the form before 0.19), see [Connect nodes](nodes.en.md). |

If a runtime package is missing, startup fails and prints an installation command. When the Python environment does not exist (the `.venv/bin/python` that `python` names has not been created) or its interpreter lacks the `rutis` package, the error gives the commands that create the environment and install the `rutis` of this host's version (`uv venv <environment>` and `uv pip install --python <interpreter> rutis==<version>`, or `python3 -m venv` and pip); with no environment at all and only a `python3` without `rutis`, it gives the commands that create `<project>/.venv`, which the host then uses by itself.

Rows using a language that `runtimes` does not declare (a `py:llmbox` row without `runtimes.py`, say) enable that runtime with its defaults, as if the file said `"py": {}`, with a warning on stderr:

```
rutis-host: warning: rutis.json: rows use the py runtime (py:llmbox), which runtimes does not declare: it runs with its defaults; declare "runtimes": { "py": {} } to say so
```

`py:` rows enable `py`, `bun:` rows `bun`, and JavaScript or TypeScript files named by a path or a `file:` URL (`.js`, `.mjs`, `.cjs`, `.ts`, `.mts`, `.cts`) `node`; an npm package name is not guessed (it may be another resolver's). Disabled rows do not count, and a runtime under `runtimes.remote` of that name (for Node, a remote runtime of `language: node`) keeps its rows. Go and Swift need their binaries named and are never enabled so: for such a row (`go:<plugin>`, `go-<binary>:<plugin>`), `check` says to declare `runtimes.go` (`runtimes.swift`). In `rutis-host dev`, the project's own runtime is set up by `dev`: rows of `rutis.dev.json` in it (`./dev/fake-llm.ts` in a TypeScript project, `py:fake_llm` in a Python one) have no warning; one another runtime runs (a `py:` row in a TypeScript project) has it once, not again each time `rutis.dev.json` is read again.

A runtime enabled so that cannot start with its defaults (no `package.json`, `@arcships/rutis-runtime` not installed, or no `rutis` in the Python environment, say) does not stop the host: a warning on stderr says why and what to do (`rutis-host: warning: the node runtime, which rows use but runtimes does not declare, cannot run with its defaults: …; its rows do not run`), its rows fail as before (`unresolved: no plugin named …`), and the other rows run. A runtime declared under `runtimes` that cannot start still fails the start.

`restart` of `node`, `py`, `bun`, `go` and `swift` (`"no"`, the default; `"on-failure"`; `"exit"`) says what happens when the runtime's process ends on its own (for `go` and `swift`, each of their runtimes'); see [When a runtime ends](#when-a-runtime-ends).

#### Remote runtimes

One `remote` entry describes a remote runtime whole; no node row is needed:

| `dial` | `listen` | `instances` | Meaning |
| --- | --- | --- | --- |
| address | — | — | One runtime, which rutis dials; its `--id` is `name` |
| — | listener | — | One runtime, which connects in; its `--id` is `name` |
| — | listener | group | Many, each connection an instance (see [Runtimes that connect in](#runtimes-that-connect-in)) |
| — | — | — | No connection of its own: a node row with `"runtime": "<name>"`, or the embedding program, provides its session |

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

- Token: `RUTIS_TOKEN_<NAME>` (the runtime's name in upper case, `-` as `_`), else `RUTIS_TOKEN`. With `dial` it is presented; with `listen` it admits the runtime connecting in, and without one the host does not start.
- `transport`: optional, `websocket` by default. A host's own way of connecting (a relay, QUIC, …) implements `Transport`, which the Rust program embedding rutis-host mounts (`Embedding`'s `transports`), and `transport` names it; one runtime and many both use it. On another transport, `listen` names that transport's own listener; no listener of that name is needed in `listen`.
- Refused at startup: `dial` with `listen` (`dial and listen both set`), `dial` with `instances` (`dial and instances both set`), `instances` without `listen`, `transport` without `dial` or `listen`, a `transport` the host does not have, a `listen` listener that does not exist, and a node row with `"runtime"` for the same runtime (`link row <id> connects it too`).
- One runtime's `listen` takes part in the same-token check as a listening node row does (#702, #726): two entries that would accept one token stop the host at startup.
- When the remote side does not contain a row's plugin, only that row fails (`runtime <name>: this connection does not contain plugin <plugin>`); the other rows are not affected, and adding such a row is not refused.
- How the connections change: [What `run` prints](#what-run-prints); their state now: `connections` in [Seeing the current state](#seeing-the-current-state).

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
- The group's `<name>:<plugin>` rows run, in each instance, on that instance's connection. When a connection does not have the plugin, the row fails in that instance only (`runtime <name>: this connection does not contain plugin <plugin>`); other instances are not affected, and adding such a row is not refused because of it.
- The services a connection's plugins provide belong to its instance: several connections with the same plugin do not collide, and rows in the same instance (in any language) that use the name get their own connection's service. Rows outside the instance cannot use them; put the consumer in the group.
- No node row is needed. `instances` goes with `listen` (`listen` alone is one runtime); a listener or group that does not exist, a group that is not `instanced`, or a missing token stops the host at startup.
- `<name>:` rows belong in that group; anywhere else the row fails.

A Python runtime connects in with `python -m rutis dial:wss://… --id <id> --peer <this host's id> <project>`, a Go plugin binary with `<binary> dial:wss://… --id <id> --peer <this host's id> <project>`, and Node and Bun runtimes with `dial:` too; the commands are in [Connect nodes](nodes.en.md#runtimes-that-connect-in).

### `listen`

WebSocket listeners used by other nodes to connect. `cert` and `key` are a TLS certificate and private key in PEM format; they can also be supplied through `RUTIS_CERT` and `RUTIS_KEY` (the file's own come first). They go together: with only one of them the host does not start, rather than listening without TLS. A listener without TLS can bind only to a loopback address.

### `rows`

Each row runs a plugin:

| Field | Meaning |
| --- | --- |
| `id` | Row name, unique within this file. |
| `name` | Plugin identifier: an npm package (or package subpath), `py:<name>` (Python entry point or module), `bun:<module>` (a module in the Bun runtime), `go:<plugin>` (the Go binary that has it; `go-<binary name>:<plugin>` when two do), `swift:<plugin>` (likewise, Swift binaries), `./relative-path` (plugin file), `<remote-runtime>:<module or plugin>`, `rutis-bridge/peer` (a node; see [Connect nodes](nodes.en.md)), or `peer:<node>/<plugin>` (a plugin running on another node). |
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

On one listener, each entry that accepts connections with these variables (a listening peer row, a runtime that connects in) must accept a token of its own; listening peer rows must, on different listeners too (they share the host's one identity; a row with an identity or transport of its own counts when the other uses the host's), disabled ones included (enabling one would clash). When two would accept the same token (say neither has its own and both fall back to `RUTIS_TOKEN`), a connection could not tell which it is for, so `run`, `check` and a configuration reload fail before starting, naming both entries and the `RUTIS_TOKEN_<name>` to set:

```
rutis-host: listener public: link row main and remote runtime phone both accept RUTIS_TOKEN, so a connection presenting it cannot be told apart: set RUTIS_TOKEN_MAIN or RUTIS_TOKEN_PHONE to a token of its own
```

The environment cannot be changed from outside the process, so these tokens cannot be revoked while it runs: to cut off a peer or a runtime that connects in, change the token and restart `rutis-host`, which drops every connection. Cutting off one identity at runtime takes the identity layer's revocation hook; see [Connect Nodes](nodes.en.md#security).

## Deployment

Place `rutis.json`, the Node project (with plugins and `@arcships/rutis-runtime` installed), the Python environment (with `rutis` and plugins installed), and the Go and Swift plugin directories (binaries for the platform) together. Run `rutis-host run /srv/app/rutis.json` under a process manager such as systemd. When `rutis-host` exits, the runtime processes it started exit as well.

To apply a changed `rutis.json`, use `systemctl reload` (`ExecReload=/bin/kill -USR1 $MAINPID`), or run with `--watch-config`; see [Changing the configuration while it runs](#changing-the-configuration-while-it-runs).

Installed with npm (`ExecStart=/srv/app/node_modules/.bin/rutis-host run /srv/app/rutis.json`), `MAINPID` is the launcher `bin/rutis-host.mjs`, a Node process that runs the `rutis-host` binary. It passes SIGTERM and SIGUSR1 on to the binary, so `systemctl stop` and `systemctl reload` work as with the binary, and it exits as the binary does. It does not pass on SIGINT and SIGHUP (a terminal sends them to the binary itself): a SIGINT sent to the launcher alone does nothing, so stop it with SIGTERM. The binary from GitHub Releases or crates.io, and the one a PyPI install puts in the environment (`.venv/bin/rutis-host`), have no launcher.

When stopping, have the process manager send SIGTERM to `rutis-host` alone and let it unload the rows. By default (`KillMode=control-group`), systemd also sends SIGTERM to the runtime processes, which then exit before their cleanups run; set `KillMode=mixed`, and set `TimeoutStopSec` longer than the cleanup deadline. If `rutis-host` is killed (SIGKILL, or ending the process on Windows), it cannot unload the rows: on Windows, the runtime processes are ended with it; on Unix, they notice that their channel closed, unload their rows themselves, and exit; one a plugin keeps busy ends a second after the cleanup deadline.
