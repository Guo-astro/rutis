# rutis-host and rutis.json

`rutis-host` is a host that requires no Rust code. It assembles the core, plugin loader, local Node / Python / Go / Swift runtimes, and network connections, all driven by a `rutis.json` file. This page is for whoever assembles plugins: installing plugins others wrote, writing `rutis.json`, running it and trying it; plugin authors use it for local development too. For applications that need custom Rust services, see [Embed in a Rust application](rust-host.en.md); to open your own Rust product to plugins, see [Build a host open to plugins](open-host.en.md).

## Quick start

A host made of two plugins: `greeter`, written in TypeScript, provides the `greeter` service (`hello(name)`); `welcome`, written in Python, uses it and provides the `welcome` service (`everyone(names)`). Plugins usually install from npm and PyPI (`npm:@your-name/greeter`, `pypi:your-name-welcome`); here `rutis-host new` makes the two plugin projects locally and they install from files, so every step runs as written. It needs Node 22 or later, and [uv](https://docs.astral.sh/uv/) with Python 3.12 or later.

1. Make the two plugin projects. `greeter` is the template as it is, packed as an npm package (`npm pack` runs `prepack` first, which builds with `tsc`):

<!-- example: rutis-host-en run -->
```bash
npx @arcships/rutis-host new greeter --lang node
cd greeter
npm install
npm pack                                              # makes greeter-0.1.0.tgz
cd ..
npx @arcships/rutis-host new welcome --lang python
```

Replace `welcome/src/welcome/__init__.py` with a plugin that uses `greeter` (the template's test tests the code it replaces, so it is not run here; writing plugins: [Python plugins](python-plugin.en.md)):

<!-- example: rutis-host-en file=welcome/src/welcome/__init__.py -->
```python
from rutis import define_plugin


class Welcome:
    def __init__(self, greeter):
        self.greeter = greeter

    async def everyone(self, names: list[str]) -> list[str]:
        return [await self.greeter.hello(name) for name in names]


def apply(ctx, config):
    ctx.provide("welcome", Welcome(ctx.use("greeter")))


plugin = define_plugin(apply, inject=["greeter"], provides={"welcome": Welcome})
```

2. Make the host's own directory, install `rutis-host` there, and install the two plugins with it:

<!-- example: rutis-host-en run -->
```bash
mkdir app
cd app
npm init -y
npm install -D @arcships/rutis-host                   # rutis-host brings the Node runtime
npx rutis-host add ../greeter/greeter-0.1.0.tgz       # an npm package: into the Node project here
npx rutis-host add ../welcome                         # a Python project: makes .venv if there is none, installs rutis at the same version
```

`add` only needs to know where a plugin comes from (`npm:`, `pypi:`, a Go module path, a local path); it works out the language, the package manager, the environment and the row name. After the package manager's output, for each plugin it prints the plugin's description (version, `inject`, `provides`, config schema) and what `check` says about its row:

```text
installed ../greeter/greeter-0.1.0.tgz 0.1.0 (node) into /home/me/app: plugin greeter
greeter (greeter): ok
  version: "0.1.0"
  inject: []
  provides: {"greeter":{"hello":"async"}}
  config: {"type":"object","properties":{"greeting":{"type":"string","default":"Hello"}}}
rutis.json: added row greeter, runtimes.node
installed ../welcome 0.1.0 (py) into /home/me/app/.venv: plugin welcome
welcome (py:welcome): ok
  version: "0.1.0"
  inject: ["greeter"]
  provides: {"welcome":{"everyone":"async"}}
rutis.json: added row welcome, runtimes.py
```

Installed first, `welcome` gets `cannot run: nothing provides greeter`; `add` still exits with 0, and installing `greeter` settles it.

The `rutis.json` that `add` wrote: the runtimes to start (`runtimes`), what it installed (`installed`), a row for each plugin (`rows`; `add` writes the row's `name`: the package name for Node, `py:<entry point>` for Python):

```json
{
  "format": 1,
  "runtimes": {
    "node": {},
    "py": {}
  },
  "installed": {
    "greeter": {
      "from": "../greeter/greeter-0.1.0.tgz"
    },
    "py:welcome": {
      "from": "../welcome"
    }
  },
  "rows": [
    {
      "id": "greeter",
      "name": "greeter"
    },
    {
      "id": "welcome",
      "name": "py:welcome"
    }
  ]
}
```

3. Check it, then run it:

<!-- example: rutis-host-en run -->
```bash
npx rutis-host check --strict                         # every row ok, every service a row uses declared by another row; exits with 0
```

<!-- example: rutis-host-en run until="welcome: running" -->
```bash
npx rutis-host run --watch-config
```

```text
greeter: running
welcome: running
```

Each row prints its state when it changes (also `starting`, `waiting for its services`, …; see [What `run` prints](#what-run-prints)).

4. In another terminal, in `app`, see which services there are and which row provides each, then call one (`status` and `call` are not on Windows):

```console
$ npx rutis-host status
rows:
  greeter: running
  welcome: running
services:
  greeter: greeter
  loader: host
  sessions: host
  welcome: welcome
runtimes:
  node: running (pid 12416)
  py: running (pid 12415)
calls:
  none
$ npx rutis-host call welcome.everyone '["Ana", "Bo"]'
[
  "Hello, Ana!",
  "Hello, Bo!"
]
```

Each argument of `call` is a JSON value, a string in quotes (`'"Ana"'`); the result prints as JSON. See [Seeing the current state](#seeing-the-current-state) and [Calling a service](#calling-a-service).

5. Change the configuration while it runs: add `"config": { "greeting": "Hej" }` to the `greeter` row in `rutis.json` and save. With `--watch-config` the host reads the file again and restarts only the row that changed:

```text
rutis-host: reloaded rutis.json (1 row changed)
```

Call it again and `welcome.everyone` returns `"Hej, Ana!"`, `"Hej, Bo!"`. On Unix, SIGUSR1 does the same without `--watch-config`; see [Changing the configuration while it runs](#changing-the-configuration-while-it-runs).

6. Stop it with Ctrl-C: each plugin's cleanup runs, the runtimes exit, and `rutis-host` exits with 0.

Next:

- Published plugins: `npx rutis-host add npm:@your-name/greeter`, `npx rutis-host add pypi:your-name-welcome`; Bun, Go and Swift plugins, and installing by hand: [How a host installs plugins](#how-a-host-installs-plugins).
- A plugin author runs the plugin in its own project with `rutis-host dev`, which reloads it when its files change and prints the services after it starts (`rutis-host dev: services: …`); see [Developing a plugin with `dev`](#developing-a-plugin-with-dev).
- A plugin running on another machine, or an app on a device connecting in: see [Remote runtimes and devices](#remote-runtimes-and-devices).

## A minimal rutis.json

Written by hand, a `rutis.json` that runs needs only the runtimes and the rows:

```json
{
  "runtimes": { "node": {} },
  "rows": [{ "id": "greeter", "name": "greeter" }]
}
```

- `rows`: a row for each plugin to run; `id` names the row, `name` is the plugin (an npm package name, `py:<entry point>`, …; see [`rows`](#rows)), with a `config` when it needs one.
- `runtimes`: one for each runtime the rows use; an empty object takes the defaults. An npm package name does not tell the host to use Node: without `runtimes.node` the row fails with `no plugin named "greeter"`. A `py:` row without `runtimes.py` enables it with its defaults and a warning (see [`runtimes`](#runtimes)).
- `format` (`1` by default) and `installed` (what `add` records) may be left out; the other fields: [`rutis.json`](#rutisjson).

## Installation

| Method | Command |
| --- | --- |
| npm | `npx @arcships/rutis-host …`, or install with `npm install -D @arcships/rutis-host` and run `npx rutis-host …` |
| PyPI | `uvx rutis-host …`, or install with `uv add --dev rutis-host` and run `uv run rutis-host …` |
| Binary | `rutis-host-<version>-<platform>.tar.gz` from GitHub Releases (Linux and macOS, x64 / arm64), or `.zip` for Windows x64 |
| crates.io | `cargo install rutis-host` |

The npm distribution includes the Node runtime (`@arcships/rutis-runtime`), and the PyPI distribution includes the Python runtime (`rutis`). They are used when the project does not provide its own runtime. Go and Swift plugins are compiled binaries: running them needs no other package and no Go or Swift toolchain (Swift plugins run on macOS).

## How a host installs plugins

With `rutis-host add <plugin> [rutis.json]`: say where the plugin comes from, not its language.

| Written | Source | Language |
| --- | --- | --- |
| `npm:@ana/wordcount`, `npm:@ana/wordcount@1.2.0` | npm | Bun when the package depends on `@arcships/rutis-bun` (`dependencies` or `devDependencies`), else Node |
| `pypi:bo-llm`, `pypi:bo-llm@0.3.0` | PyPI | Python |
| `github.com/bo/netkit/cmd/netkit`, `…@v0.4.1` | Go module | Go |
| `./plugins/notes` | local directory | by its project file: `package.json` (Bun or Node as above), `pyproject.toml`, `go.mod` |
| `./wordcount-1.2.0.tgz`, `./bo_llm-0.3.0-py3-none-any.whl` | local package | `.tgz` by the `package.json` inside; `.whl` is Python |
| `./netkit` | local binary | Go when built with the Go SDK, Swift when built with the Swift SDK |

- The version follows the last `@` (one not at the start): an exact version or a tag; without one, the latest. Local paths start with `./`, `../` or `/`, relative to the directory of `rutis.json`.
- A name without a source is refused: the same name may be on npm and on PyPI, and a wrong guess installs someone else's package.

  ```text
  rutis-host: wordcount: say where it comes from: npm:wordcount, pypi:wordcount, a Go module path or a local path
  ```

What `add` does:

1. It installs into the place the language's runtime uses now: Node and Bun into `runtimes.node.project` and `runtimes.bun.project` (by default the directory of `rutis.json`); Python into the interpreter the host would use (`runtimes.py.python`, else `$VIRTUAL_ENV`, else `<runtimes.py.project>/.venv`); Go and Swift into `runtimes.go.dir` and `runtimes.swift.dir` (`plugins/go` when there is no `runtimes.go`).
2. What the runtime lacks it sets up: `npm init -y` without a `package.json`; `@arcships/rutis-runtime`, when missing, installed with the plugin (at `rutis-host`'s version); for Bun, `bun add -d` of a missing `@arcships/rutis-bun`; for Python, `uv venv` without an environment (`python3 -m venv` without uv) and the `rutis` package of the same version when missing; for Go and Swift, the directory. Node, Bun, Python and Go themselves are not installed.
3. It runs the package manager: `npm install`, `bun add`, `uv pip install --python <interpreter>` (`<interpreter> -m pip install` without uv; `uv add` in a uv project, see below), `GOBIN=<dir> go install <module>@<version>` (`go install ./...` inside a local Go project; a local binary is copied into the directory). Its output shows as it is.
4. It finds the plugins in the package and describes them: a Node or Bun plugin is the package itself; Python's are the distribution's entry points in the group `rutis.plugins` (a package may have several); Go's and Swift's are the plugins of the binary's manifest.
5. It writes `rutis.json`: a record per plugin under `installed` (keyed by its row name), a row `{ "id": …, "name": <row name> }` per plugin at the end of `rows`, the row name in its usual form: the package name for Node, `bun:<package>` for Bun, `py:<entry point>` for Python, `go:<plugin>` for Go, `swift:<plugin>` for Swift (the id is the plugin's name, `-2`, `-3` added when it is taken; no row when one of that name is there), and the language under `runtimes` when it is not there (`"node": {}`, `"py": {}`, `"go": { "dir": "plugins/go" }`). The file keeps its keys' order, and is written to a temporary file then renamed.
6. It checks the new rows as `check` does, and prints what it finds: a service it needs that nothing provides, a required config value missing. `add` still exits with 0: the row is written, and what is missing can be added next.

When the Python environment is a uv project's `.venv` (`runtimes.py.project`, by default the directory of `rutis.json`, has a `pyproject.toml` and a `uv.lock`, and the environment the host uses is the `.venv` there, as after `uv add --dev rutis-host` and `uv run rutis-host add …`; and uv is there), `add` records the plugin in the `pyproject.toml`'s dev dependency group with `uv add --dev --project <project> <package>` (a local path under `[tool.uv.sources]`), so the next `uv sync` keeps it; a missing `rutis` is installed the same way. `upgrade` runs `uv add --dev --project <project> --upgrade-package <package> <package>>=<the version there>` (`<package>==<version>` when a version is given; `--reinstall-package` for a local path), and `remove` runs `uv remove --dev --project <project> <package>`, which is also the command an error gives to uninstall a package that is not a plugin. Otherwise `uv pip install` runs as before, and `pyproject.toml` is not changed.

The file written: the second argument; without one, `rutis.json` here; without `rutis.json`, `rutis.dev.json` (a plugin project); without either, a new `rutis.json`.

`remove` and `upgrade`:

```text
$ rutis-host remove llm
uninstalled bo-llm from /app/.venv
rutis.json: removed row llm

$ rutis-host upgrade
@ana/wordcount: 1.2.0 → 1.3.0
bo-llm: 0.3.0 is the latest
a running host keeps the old versions until it restarts
```

- `remove <row name, plugin or reference>` uninstalls the package the plugin is in (`npm uninstall`, `bun remove`, `uv pip uninstall` (`uv remove` in a uv project), deleting the binaries whose manifest has it), removes all of that package's records and the rows with those row names (in groups too), and prints each row removed; a package with several plugins goes as a whole, and it says so. `runtimes` stays.
- `upgrade [<row name, plugin or reference>[@version]]`, without a name, upgrades every record: the newest version from the record's source (or the version given); rows stay. A plugin the new version adds is recorded, and how to add its row is printed; for one it no longer has, the record goes and each row using it prints `<row id>: the new version has no plugin <plugin>`.
- A running host keeps running the old code until it restarts (reloading a row imports only its own entry module again; an upgrade replaces the whole package). Rows `add` adds join a running host as [Changing the configuration while it runs](#changing-the-configuration-while-it-runs) says, under `run --watch-config` and `dev`; when such a row needs a runtime that is not running yet, the `runtimes.<language>` written is refused, saying to restart.

On errors (on stderr, exit status 1, `rutis.json` unchanged):

| Case | Output |
| --- | --- |
| No package manager | `rutis-host: cannot run npm (No such file or directory): install Node.js 22 or later`; for Python with neither uv nor `python3`: `rutis-host: cannot create the Python environment /app/.venv: install uv, or Python 3.12 or later` |
| The package manager fails | its own output, then `rutis-host: npm install @ana/wordcount failed (exit status 1); rutis.json is unchanged` |
| Not a plugin | `rutis-host: bo-utils has no rutis.plugins entry point: it is not a plugin; it stays installed: uv pip uninstall --python /app/.venv/bin/python bo-utils` |
| A local directory it does not recognize | `rutis-host: ./plugins/notes has no package.json, pyproject.toml or go.mod` |
| The row name is another package's | `rutis-host: py:llm is already installed from pypi:other-llm: remove it first` (when known only once installed, followed by the command that uninstalls it) |
| No Go directory | `rutis-host: runtimes.go has no dir to install into: add one, or remove runtimes.go to use plugins/go` |
| `remove` of a name not recorded | `rutis-host: llm was not installed by add: uninstall it with the package manager that installed it, and remove its row` |

What the package manager installed before a failure stays, as when npm or uv are run directly; the error gives the command that uninstalls it.

**Install only plugins you trust.** `add` and `upgrade` run the package manager, which may run the package's install scripts, and `add` imports the plugin's code to check it; the same as running `npm install`, `uv pip install` or `go install` directly. rutis adds no sandbox or review.

### Without add

Installing by hand works as before, with the same row names: each language's plugins go where that language's runtime finds them, and a row names the plugin:

| Plugin | Install it with | Into | Row `name` |
| --- | --- | --- | --- |
| TypeScript, JavaScript (npm) | `npm install <package>` | the Node project (`runtimes.node.project`, by default the directory of `rutis.json`), beside `@arcships/rutis-runtime` (or the one the npm `rutis-host` brings) | the package name (`@your-name/weather`) or a subpath of it; a file: `./plugins/tool.ts` |
| Bun | `bun add @arcships/rutis-bun <package>` | the Bun project (`runtimes.bun.project`) | `bun:<package>` (see [Each language](#each-language) below) |
| Python | `uv venv` once, then `uv pip install --python .venv/bin/python "rutis==<rutis-host's version>" <distribution>` (with pip: `.venv/bin/python -m pip install …`) | the environment of `runtimes.py.python` (by default `$VIRTUAL_ENV`, then `<runtimes.py.project>/.venv`); its `rutis` is the Python runtime, at rutis-host's version | `py:<entry point>` (`py:llm`), or `py:<module>` for a module file in `runtimes.py.project` |
| Go | `GOBIN=<runtimes.go.dir> go install <module>/cmd/<binary>@<version>` (with the machine's Go toolchain), or the binary for the platform from the plugin's release | `runtimes.go.dir` | `go:<plugin>` (see [Each language](#each-language)) |
| Swift (macOS) | the binary from the plugin's release | `runtimes.swift.dir` | `swift:<plugin>` (see [Each language](#each-language)) |
| Rust | not with `rutis-host`: a Rust application mounts its Rust plugins itself | | [Embed in a Rust application](rust-host.en.md) |

- The plugins of one language share its runtime: one Node project, one Python environment. A Python plugin depends on `rutis` with a lower bound only, so installing one does not downgrade the runtime; a TypeScript plugin keeps the SDK version it was written against, and several copies work side by side ([plugin API: versions](plugin-api.en.md#versions)).
- Declare under `runtimes` every runtime a row uses (`"node": {}`, `"py": {}`, …: an empty object takes the defaults); one left out is enabled with its defaults and a warning (see [`runtimes`](#runtimes)).
- After installing, `rutis-host check --strict` says whether every row can run and whether every service a row uses is declared by another row; in CI it fails otherwise.

### Each language

- **TypeScript, JavaScript**: the row name is the package name (`@your-name/greeter`) or a subpath of it. A plugin package brings the `@arcships/rutis` version it was written against, and several versions work side by side in one host; a plugin carries the plugin API version it uses, and a host runtime older than that fails with a clear error.
- **Bun**: for a package that depends on `@arcships/rutis-bun`, `add` installs it into `runtimes.bun.project` with `bun add`. The row name `bun:<npm name or subpath>` resolves from that project's `node_modules`, and `bun:./path` is relative to the project; `bun:sqlite` is the project's module `sqlite`, not Bun's built-in one. `runtimes.bun` can sit beside `runtimes.node` and `runtimes.py`; services are shared by name across runtimes.
- **Python**: the row name is the plugin's entry point name in the `rutis.plugins` group (`py:welcome`), not the distribution name. The Python runtime is the `rutis` package in that environment, at the same version as `rutis-host` (`rutis-host --version`).
- **Go**: the binaries go into `runtimes.go.dir` (`add` uses `go install`; the binary for the platform from the plugin's release works too; a binary downloaded on macOS carries the quarantine attribute, which `rutis-host check` points out). The row name `go:<plugin>` is looked up in every binary; when two binaries have a plugin of that name, the row fails naming both files, and the plugins' author renames one. A binary starts the first time a row uses it and stops after 60 idle seconds (`"start": "eager"` starts them all and keeps them running); a replaced binary takes effect when the host restarts.
- **Swift** (macOS): download the binary from the plugin's release into `runtimes.swift.dir`; a downloaded binary carries the quarantine attribute, which `rutis-host check` points out: once you trust it, clear it with `xattr -d com.apple.quarantine <file>`. The row name is `swift:<plugin>` (a plugin name is in one binary only); the other settings are those of `runtimes.go`. Running it needs no Swift toolchain.
- **Rust**: Rust plugins are compiled into a Rust host: see [Embed in a Rust application](rust-host.en.md); a Rust host also runs the plugins of the languages above (Go's `BinaryResolver<Go>`, `BinaryRuntimes<Go>`, for one): see [Runtime and loader](rust-host.en.md#runtime-and-loader).

## Commands

| Command | Purpose |
| --- | --- |
| `rutis-host run [rutis.json]` | Runs the configuration (reading it again while it runs on SIGUSR1 on Unix, or with `--watch-config`; see [Changing the configuration while it runs](#changing-the-configuration-while-it-runs)) and prints each row's status changes, the connections of connection rows and local runtimes that end (see [What `run` prints](#what-run-prints)). Press Ctrl-C to stop (see [Stopping and exit status](#stopping-and-exit-status)). |
| `rutis-host dev [directory]` | Runs a plugin project with its `rutis.dev.json` file and follows its changes: a changed file reloads the rows whose file it is, a changed `rutis.dev.json` is read again; a Go or Swift project is rebuilt (`go build`, `swift build`) when its sources change, restarting only its runtime (see [Developing a plugin with `dev`](#developing-a-plugin-with-dev)). |
| `rutis-host check [rutis.json] [--strict]` | Without running any plugin, checks that each row can run (see [Checking a configuration](#checking-a-configuration)) and prints its version, dependencies, provided services, and configuration schema, and lists every Go and Swift binary (runtime name, platform, SDK, plugin API, plugins; a binary of an SDK older than rutis 0.13 cannot run). Exits with a nonzero status if a row or binary cannot run. A dependency no row declares, which another row may provide at runtime, is a warning; with `--strict`, an error. Without `rutis.json`, checks the plugin project in the current directory. |
| `rutis-host status [rutis.json \| directory]` | Asks the `rutis-host` running that configuration (or that `dev` project) what it does now: each row's state and why, each service's provider, the runtimes, the calls not returned yet (see [Seeing the current state](#seeing-the-current-state)); not on Windows. |
| `rutis-host call [--config <rutis.json \| directory>] <service>.<method> [arguments…]` | Calls a service in the `rutis-host` running that configuration (or that `dev` project), as the host, and prints the result as JSON; `run` and `dev` both accept it (see [Calling a service](#calling-a-service)); not on Windows. |
| `rutis-host new <name> --lang node\|bun\|python\|go\|swift` | Creates a plugin project. |
| `rutis-host add <plugin> [rutis.json]` | Installs a plugin by its source (`npm:`, `pypi:`, a Go module path, a local path) with what its runtime lacks, records it under `installed`, adds its row and prints its introduction (see [How a host installs plugins](#how-a-host-installs-plugins)). |
| `rutis-host remove <plugin> [rutis.json]` | Uninstalls the package of a plugin `add` installed, and removes its records and rows. |
| `rutis-host upgrade [<plugin>[@<version>]] [rutis.json]` | Upgrades plugins `add` installed to the newest version (or the one given); a running host uses it once it restarts. |
| `rutis-host test <plugin> -- <runtime command…> <project>` | For the SDKs' test tools: a host with no rows of its own that starts a runtime as a local runtime, loads `<plugin>` there to drive the test, and serves `rutis-host/test` to run plugins in that runtime; it exits when the runtime ends (see [Running plugins under test](#running-plugins-under-test-rutis-host-test)). |
| `rutis-host types [directory] [-o file] [--python file]` | Writes the TypeScript declarations of the Rust services marked `#[rutis::service]` in the crate in `directory` (by default the current one) and in the crates it uses, to stdout or to `file`; `--python` writes their Python types too; see [Embed in a Rust application: Declarations](rust-host.en.md#declarations). |

Add `--help` or `-h` to any command to print its usage (only that command's lines) and exit 0 without doing anything else; `rutis-host --help` prints them all.

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
| `waiting for llm (row model is starting)` / `(row model has not provided it)` | The row that provides it is starting; or runs but has not provided it (one running on a connection's far end while the connection is down, say) |
| `waiting for its services` | When rutis cannot say what it waits for |

When the reason changes (the provider going from starting to failed, say), another line is printed.

A local runtime (`node`, `py`, `bun`, Go's `go-<binary>`, Swift's `swift-<binary>`) whose process ends on its own, or that cannot start, prints a line, and another when it runs again; one that `rutis-host` stops (the host stopping, a Go or Swift runtime stopping when idle) prints none:

| Output | Meaning |
| --- | --- |
| `runtime <name>: down (<reason>)` | The runtime process ended; the reason says how, such as `exited with signal: 9 (SIGKILL)` or `exited with exit status: 1`; or it could not start, and the reason is the error. Its rows go back to `waiting for runtime <name> (down: …)`; other runtimes and their rows go on. |
| `runtime <name>: running` | A runtime that printed `down` runs again (a Go or Swift runtime rebuilt by `dev`). |

A runtime that stays down prints once; one that starts again and goes down again prints the line again, however briefly it ran. The reason in parentheses is for people, and its wording is not guaranteed.

The rows of a runtime that ended only wait for it, whichever plugin made its process exit; one that was still starting when the runtime ended is not reported as `failed`. A call in progress to one of them fails saying how the process ended. What the runtime itself wrote about it (an error and its stack, say) is in the output as before.

The connections of connection rows (`rutis-host/dial`, `rutis-host/listen`; see [Remote runtimes and devices](#remote-runtimes-and-devices)) print lines starting with the row id, in two words only, `connected` and `disconnected`, the reason in parentheses:

```text
gpu: connected
gpu: disconnected (gpu.example.com:7443: Connection refused); retrying in 2.1s
gpu: disconnected (auth-rejected: gpu.example.com:7443: upgrade refused with 403 Forbidden); retrying in 30s
gpu: disconnected (incompatible: this endpoint speaks rutis.3, the dialer offered rutis.4: upgrade this endpoint (PyPI rutis)); stopped
phone: connected phone-1
phone: disconnected phone-1 (connection lost)
phone: disconnected phone-1 (replaced by a new connection)
```

| Output | Meaning |
| --- | --- |
| `<row id>: connected` | dial: the handshake is done; the far end's plugins start registering as `<row id>:<plugin>` |
| `<row id>: disconnected (<reason>); retrying in 2.1s` | dial: it could not connect, or the connection ended; retried with backoff: from 0.5 seconds, doubling, at most 30 seconds, from 0.5 seconds again once a connection lasted 60 seconds |
| `<row id>: disconnected (auth-rejected: <reason>); retrying in 30s` | dial: refused: a wrong token (401 / 403 on the upgrade), a certificate not trusted (wrong name, expired, CA not trusted), or the far end closing with 4003; the reason names the far end's address; tried again every 30 seconds |
| `<row id>: disconnected (incompatible: <reason>); stopped` | dial: the protocols do not match, and the reason says which end to upgrade; not retried; change the configuration and reload |
| `<row id>: connected <id>` | listen: a connection finished its handshake; `<id>` is the id the far end gave in it, and its plugins register as `<id>:<plugin>` |
| `<row id>: disconnected <id> (<reason>)` | listen: that connection ended; its remote plugins all unloaded |
| `<row id>: disconnected <id> (replaced by a new connection)` | listen: a newer connection with the same id finished its handshake; the old connection's remote plugins unloaded first |

- No token or other credential appears in the output; whole seconds are written `30s`, other waits to a tenth of a second.
- A connection whose handshake fails on a listen row (a wrong token, refused with 401 / 403 on the upgrade; protocols that do not match) prints nothing: it touched no connection there is; the dialing end says why.
- When the host stops, or the row is removed or stopped, its connections close with it, and no `disconnected` is printed.

A listener that fails in an `emit` (the emitter does not wait, so nobody else hears of it) prints one line on stderr, where it ran: `rutis: listener of <name> in row <row> failed: <ErrorName>: <message>` for the host's own, the same line from the runtime for a runtime's; a call the host could not deliver names where it went (`in runtime <name>`, `in node <peer>`). A failing filter of an `emit` prints `rutis: filter of <name> from row <row> failed: …` (`from the mount` for a plugin a mount loaded).

### Events

Every row's listeners, in any language and on linked nodes, are on one event bus, with one set of rules ([Unified events](../design-events-2026-10-18.en.md)): nothing is declared in `rutis.json`. An `instanced` group only affects the scope chain in descriptors; events are delivered by name to every listener on the bus, so filter by instance when needed. `rutis-host` also emits, and plugins of every language may listen to:

| Event | When |
| --- | --- |
| `rutis/row` | each state change of a row: `{ row, plugin, scope, state: "pending" \| "running" \| "failed" \| "stopped", error }` |
| `rutis/runtime` | a runtime runs (`running`) or ended (`down`, with `reason`) |
| `rutis/service` | a service a runtime's row provides appeared, was replaced or went (`available`) |

A runtime of rutis 0.12 or older (or a node) is refused by its protocol version when it connects: `incompatible session: the far end speaks protocol 2, this side 4: upgrade the far end`.

## Checking a configuration

For each row (except disabled ones), `check` verifies that:

- its plugin is found and resolves;
- the row passes the checks made before loading (scopes, and the config of built-in plugins such as connection rows);
- every service it waits for (its plugin's `inject`, plus the row's own `inject`) is provided by another row: as plugins declare in `provides`.

A row waiting for a service no row provides, and no row may provide at runtime, would wait forever: `check` says which service is missing and what to do, and exits with 1; when only disabled rows provide it, it names them. What some rows provide is not known during the check (rows run on a connection's far end, which the check does not connect to); a service missing then is only reported as possibly coming from them, not as a failure.

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
- Rows whose schema is known only when they run (rows run on the far end of a connection not connected) say `config not checked: its schema is known only when it runs`, which is not a warning.

`run` and `dev` apply the same rules: a row whose config does not match fails (`weather: config: …`, a line per problem), and rows waiting for it say so (`chat: waiting for weather (row weather failed: config: …)`); extra keys are warned about once per row and config value. A row run in a runtime (a local runtime's, such as Node's or Python's, or one on the far end of a connection) has a schema known only once its runtime is up (its connection connected); it is checked then, before the row starts: a row whose config does not match does not start, and never runs with that config (in 0.19.0 it started all the same).

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
2. A change to what a running host cannot change: refused, saying to restart the host. That is the top-level `format`, `id`, `listen`, `runtimes.go` and `runtimes.swift` (any change), and a local runtime (`node`, `py`, `bun`) added or with changed settings.
3. The new rows are checked first (as by `check`): a row that cannot start (its plugin not found, its settings failing the checks before loading), and that is not failing now, refuses it. A row that waits for a service is not one that cannot start, as at start.
4. Then the new rows replace the old: added rows start, removed rows unload, changed rows restart; rows that did not change are left alone (rows using a service of a restarted row wait for it and come back with it, as always). A row failing after the replacement (its plugin's `apply` throwing, say) that was not failing before brings the old rows back (the rows replaced restart once more), and refuses it.
5. A local runtime removed: its rows unload first, then the runtime stops, and the reload is done once its process has exited.

When rows of an `instanced` group change, the change applies in every instance there is: an added row starts in each instance, a removed one unloads in each, a changed one restarts in each; the instances stay, and the rows in them that did not change are left alone. Instances created later run the new rows.

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

Credentials are read from the environment when a row dials, or when a far end dials in (see [Credentials](#credentials)), so a connection row a reload adds connects too. The environment does not change while the process runs: a new token still needs a restart.

Plugins can add and remove rows too, through the service `loader` (`create`, `remove`; see [The host writes its own plugins](../design-host-writes-plugins-2026-10-27.en.md)). `run` writes these changes back into `rutis.json`: only `rows` changes (a row added goes last, with only the fields the plugin gave, `id` always; a row removed goes), its other fields and the order of its keys stay, written as `rutis-host add` writes it (two-space indents). What it writes is no change to the file: `--watch-config` does not read it again for it. When the file was changed after the host read it (by hand, by `rutis-host add`) and has not been read again, a plugin's next change first reads back the file's `rows` and runs them, then makes the change on them and writes it back; a change outside `rows` still takes SIGUSR1 or `--watch-config`. When the file cannot be written, the row is added or removed all the same, and the plugin's error says `<row>: added, but rutis.json could not be written: <why>` (`removed` for a removal); a restart of the host loses the change. `dev` does not write `rutis.dev.json`: under `dev`, `create` and `remove` get `this host keeps no editable configuration, so rows cannot be added or removed`.

## Managing plugins with `loader`

`rutis-host` provides a service `loader`, with which plugins add and remove rows and load a row's code again while the host runs: an agent, say, writes a new plugin, has it loaded, is told what is wrong when it breaks it, puts the fixed version in, and removes it when done. Any plugin writing `inject: ["loader"]` can use it; to keep a plugin from managing plugins, do not add it. How each language writes it, what each method returns and the full table of errors: [Plugin API](plugin-api.en.md#managing-plugins-with-loader).

| Method | What it does | Returns |
| --- | --- | --- |
| `create(options, parent?, position?)` | adds a row and starts it; `options` is `{id?, name, config?, disabled?, inject?}`, written as a row of `rutis.json` is. A row that cannot start is not left behind | the row's id (8 hex digits, generated, when `id` is not given) |
| `reload(id)` | loads the row's code again: when it cannot load, the previous version keeps running; when the new code fails in `apply`, the row runs the new code and fails, as in [`dev`](#developing-a-plugin-with-dev) | nothing |
| `remove(id)` | unloads the row and removes it | nothing |
| `status()` | reads the current state, the same snapshot as [`rutis-host status`](#seeing-the-current-state) | rows, services, calls in progress |

The methods are called after `apply` returns (in a service method, in an event). A row calling `create`, `reload` or `remove` in its own `apply` waits until that `apply` ends, so it hangs; to mount child plugins in `apply`, use `ctx.plugin`.

On failure the plugin gets an error whose `message` is the text the terminal shows, for example:

| Cause | `message` |
| --- | --- |
| `create`, `apply` throws | `weather: KeyError: 'units'` |
| `create`, the `id` exists | `a row weather already exists` |
| `reload`, a syntax error | `weather: cannot reload: cannot load weather.py: SyntaxError: expected ':' (weather.py:3); the previous version keeps running` |
| `reload` / `remove` of a row that does not exist | `no row wether` |
| `create` / `remove` under `dev` | `this host keeps no editable configuration, so rows cannot be added or removed` |
| the row added, but `rutis.json` cannot be written | `weather: added, but rutis.json could not be written: <the system's reason>` |

**`run` writes back into `rutis.json`.** A row added through `loader` goes at the end of `rows` (`id` always, otherwise only the fields the plugin gave), a row removed goes; the other fields and the order of keys stay, `rutis-host check rutis.json` treats the written file as a hand-written one, and the rows written run again after a restart. The details of writing back (`--watch-config`, a file changed after the host read it): [Changing the configuration while it runs](#changing-the-configuration-while-it-runs). **`dev` keeps no editable configuration**: `dev` develops one plugin project, whose rows are changed in `rutis.dev.json`; `create` and `remove` get the error above, `reload` and `status` work as usual.

What the terminal shows (in the format of [What `run` prints](#what-run-prints)):

| What the plugin did | Terminal |
| --- | --- |
| `create` succeeded | `weather: running` (or `weather: waiting for …`, as for a hand-written row) |
| `create` failed | two lines: `fails: failed: KeyError: 'units'`, then `fails: removed` (taken back) |
| `reload` succeeded, or hit a syntax error (the previous version keeps running) | nothing: the row stays `running` |
| `reload`, the new code's `apply` failed | `weather: failed: KeyError: 'units'`; once fixed and reloaded, `weather: running` |
| `remove` | `weather: removed` |

rutis-host takes the name `loader`: another row in `rutis.json` providing a service named `loader` fails with the error for a name already provided (`service loader is already provided by …: row <row> cannot provide it too`), and the terminal prints `<row>: failed …` as usual. Rename that row's service.

## Developing a plugin with `dev`

`rutis-host dev [directory]` runs the plugin project in the directory (the current one by default) as rows: the project's own plugin, configured by the row of `rutis.dev.json` with the same id (a `name` that row gives is not used, and `dev` warns `rutis-host: warning: <directory>/rutis.dev.json: row <id>: its name <name> is ignored: <id> is the project's own plugin (<its name>)`), and the other rows that file adds (fakes of the services the plugin uses, other plugins). It looks at the project's files every 0.4 seconds (not `node_modules`, `.venv`, `venv`, `target`, `dist`, `build`, `__pycache__`, nor hidden files and directories other than `.env`) and follows them:

- A changed file that is a row's module (`./dev/fake-llm.ts`; for a Python row `py:fake_llm`, the module's file `fake_llm.py` or `fake_llm/__init__.py` in the Python runtime's project) reloads that row. Any other changed file of the project (the plugin's other modules, `package.json`, `pyproject.toml`) reloads the project's own row. Rows whose files did not change are left alone.
- In a Go or Swift project, a change to its sources (`.go`, `go.mod`, `go.sum`; Swift sources, `Package.swift`, `Package.resolved`) rebuilds it and restarts its runtime on the new build; a failed build keeps the last one running. A change to the module of another row (a Python or TypeScript row of `rutis.dev.json`) reloads that row without a rebuild; a change to any other file does nothing.
- `rutis.dev.json` is read again when its content changes, by the rules of `run --watch-config` (all or nothing; what a running host cannot change is refused, saying to restart; see [Changing the configuration while it runs](#changing-the-configuration-while-it-runs)); on Unix, SIGUSR1 does so too. Rows whose entry changed restart with it, added rows start, removed rows unload.
- Rows that use a service of a row that reloaded restart with it, as they always do when a service they use goes and comes back.
- A local runtime (`node`, `py`, `bun`) whose process ended and that nothing restarts (`restart` is `"no"`, the default, or `"on-failure"` gave up) starts again at the next change to one of its rows (a file or its entry in `rutis.dev.json`), with the code as it is then; its rows come back with it. A change to other rows does not start it. One that its `restart` starts again (`"on-failure"`, not given up), or that is starting, is left to it: its rows start with the code as it is then.
- A reload imports the row's own module again; for a Python plugin that is a package, with the package's other modules (a change to the `helpers.py` of `from .helpers import X` also prints `<row id>: reloaded (src/<package>/helpers.py changed)`, and the new code runs). Modules it imports from outside its package (another top-level module, a package installed in the environment) are not imported again: after changing one of those, restart `dev`.
- A reload whose new code does not import (a syntax error, an error while importing it) changes nothing: the row runs the version it ran, also when it restarts later for another reason (a service it uses reloading), until a change that imports. New code that imports but whose `apply` fails is the code the row runs from then on: the row fails with that error and does not run until a change fixes it.

`dev --debug` runs the project's language runtime under its debugger and prints how to connect (`rutis-host dev: debugging: …`): the Python runtime reads this terminal, and `breakpoint()` stops in pdb; the Node runtime's inspector is on `127.0.0.1:9229`, Bun's on `ws://127.0.0.1:6499/rutis`; a Go project is built without optimisation and its runtime runs under Delve, listening on `127.0.0.1:2345` (install `dlv` first). The same holds for Node, Bun, Python and Go runtimes that `rutis.dev.json` adds (an added Go binary runs as it was built; Delve's address is fixed, so there can be one Go runtime); when a debugger it needs is not installed, `dev` does not start. A Swift project does not take `--debug`. How to use each: the [TypeScript](typescript-plugin.en.md#debugging), [Bun](bun-plugin.en.md#debugging), [Python](python-plugin.en.md#debugging) and [Go](go-plugin.en.md#debugging) guides.

Besides the lines of [`run`](#what-run-prints), `dev` prints:

| Output | Meaning |
| --- | --- |
| `<row id>: reloaded (<files> changed)` | The row runs the new code; the files are relative to the project. |
| `<row id>: cannot reload: <reason>; the previous version keeps running` | The new code does not import; the row runs on the version it ran. When the new code made another row fail, the reason names that row first: `<other row> failed with it: …`. Without `; the previous version keeps running` (its new `apply` failing, say), the row does not run now. While it runs the previous version, `rutis-host status` says of it `<row id>: running (the previous version: its last reload failed)`, until a reload succeeds. |
| `<row id>: restarted (<other row> reloaded)` | The row uses a service of the other row, and restarted with it. The cause may also be `<row> updated`, `added` or `removed` (by `rutis.dev.json`), `<Go or Swift runtime> restarted`, or `<row> restarted`: a row it uses that restarted with the change (`caller: restarted (greeter restarted)` when `llm` reloaded and `greeter` uses it). The cause named is always a row the row uses, each row once, by its own change when it had one (`caller: restarted (weather reloaded)` when `weather` and `llm` changed together, not `weather reloaded, weather restarted`); when that is not known (a dependency inside a runtime, between Cordis rows), no cause is named: `<row id>: restarted`. `started` or `stopped` instead of `restarted` when the row was not running before, or is not running now. |
| `rutis-host: reloaded rutis.dev.json (<n> rows changed)` | `rutis.dev.json` was read again; each row that changed follows: `<row id>: updated (its row in rutis.dev.json changed)`, `<row id>: added`, `<row id>: removed`. Refused: `rutis-host: reload rejected: <reason>; nothing changed`. |
| `runtime <name>: starting again (<files> changed)` | A runtime that was down starts again for the change; `runtime <name>: running` follows when it runs, or `runtime <name>: down (<reason>)` again. When its `restart` starts it again: `runtime <name>: down; its restart starts it again, its rows with the code as it is then (…)`; when it is starting: `runtime <name>: starting; its rows start with the code as it is then (…)`. |
| `rutis-host dev: services: forecast (weather), llm (llm), loader (host), sessions (host)` | Once every row is running, waiting or failed, the services now and the rows providing them (as `services` in `rutis-host status`), by name; one the host provides as `host` (`loader`, which rutis provides itself, say); when no row provides one, `rutis-host dev: services: none`, whatever the host provides. Printed again whenever the services change (a runtime can provide one after its row started), and after each reload, rebuild or reading of `rutis.dev.json`, not compared with the last. It may come before or after the rows' state lines. It starts with `rutis-host dev: `, not in the form `<row>: <state>`, so scripts reading the state lines are not affected; `run` does not print it: use `rutis-host status` |
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
- `rutis-host/dial` rows reconnect by themselves and are not affected.

## Seeing the current state

While `run` or `dev` runs, run `rutis-host status` in another terminal in the same directory (or `rutis-host status path/to/rutis.json`, `rutis-host status <the dev project's directory>`) to print the host's state now, once. It answers while starting is stuck too (a row's `apply` waiting on a call that does not return, before `run` prints any state line):

```
rows:
  slow: running
  caller: starting
services:
  loader: host
  sessions: host
  slow: slow
runtimes:
  node: running (pid 4242)
  py: running (pid 4243)
calls:
  caller → slow.wait  42s
connections:
  gpu: connected
  phone: kim, lee
```

- `rows`: each row (a row inside instances once per instance, as `<row> in <group> #<number>`) and its state: `running`, `starting`, `stopped`, `disabled`, and `waiting` for a waiting row when rutis cannot say for what; a waiting row says what it waits for (in the words of `run`'s waiting lines); a failed row, the first line of its failure. When a runtime exits, its rows show at once that they wait for it.
- `services`: each service by name and the row providing it; one the host (Rust) provides says `host`.
- `runtimes`: each runtime's state (`running`, `starting`, `down`, `stopped`), how one that exited ended, and a local runtime's process id.
- `calls`: the calls into a row's service that have not returned: who calls (`host` for one no row made), which service and method, for how long; `(row <row>)` when the row called is named otherwise than the service. Not seen: calls within one runtime, and calls of functions passed by reference (only "host → row" is seen, without a method name); a call an in-process Rust plugin makes is listed as from `host`.
- `connections`: a line per connection row (`rutis-host/dial`, `rutis-host/listen`); the part is left out when there are none. A dial row says `connected` or `disconnected`; a listen row, the ids connected now (`phone: kim, lee`), `none connected` when none is. 0.19 does not list the remote plugins each connection registered.
- rutis sets no time limit on calls: how long to wait is the caller's decision (how each language limits a call: [plugin API](plugin-api.en.md)). This only shows which call has waited how long.

`run` and `dev` open a local socket only the user can use (mode 0600, inside the user's own 0700 directory) that answers `status`, which changes nothing, and `rutis-host call` (see [Calling a service](#calling-a-service)). It lives under `rutis-host-<uid>/` in the system's runtime directory (`$XDG_RUNTIME_DIR`, else the temporary directory; `/tmp` when that path would be too long), named by a hash of the configuration's path, so nothing is written into the project; the host removes it when it ends (and the directory, when no other host's socket is in it); a host killed outright (`kill -9`) cannot, and the next host for the same configuration replaces it. `status` finds the directory by its own environment: run as another user, with `sudo`, or from a terminal with other variables, it looks elsewhere and says no host runs. When another host already answers for the same configuration, the later one runs as usual with a warning. Not on Windows in 0.18.

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
| No such service | `rutis-host: no row provides forcast; did you mean forecast? (services: forecast, llm, loader, sessions)` |
| The row providing it is not running | `rutis-host: forecast is not available (row weather waits for its runtime go-weather)` |
| No such method | `rutis-host: forecast has no method tomorow (methods: today, week)` |
| An argument of the wrong type | `rutis-host: forecast.today: argument 1 should be a string, got 3` |
| The method throws | `rutis-host: call to forecast.today failed: KeyError: 'region'`, then the row it failed in (`in weather:`) and its stack |
| The method takes the calling row's `&Ctx` | `rutis-host: tools.register needs a calling row; call it from a plugin, not from rutis-host call` |
| No host running | `rutis-host: no rutis-host run or dev is running for /work/weather (start one, or name its configuration with --config)` |

`run` and `dev` both accept `call`. Who can connect is as for `status`: only the user can use the socket. Not on Windows (`rutis-host call is not available on Windows`).

## Running plugins under test: `rutis-host test`

`rutis-host test` is for the test tools of the SDKs: it runs the plugin under test in a real runtime against a real rutis kernel, so that a test sees what a host does. A plugin author does not run it by hand; the test tool does. A test tool finds it on `PATH`, or at the path `RUTIS_HOST` names.

```console
rutis-host test <plugin> -- <runtime command…> <project>
rutis-host test <plugin> --channel <channel>
```

- It starts a host with no rows of its own (its id is `host`), and starts the runtime as every local runtime is started: `<runtime command…> <channel> --id test <project>`, the channel on fd 3, or, on Windows (and with `RUTIS_LOCAL_HANDOVER=loopback`), a loopback address with a one-time token. For Node, `node <@arcships/rutis-runtime>/src/runner.mjs <package.json>`; for Python, `python3 -m rutis <project directory>`. The runtime's output is passed on a whole line at a time, as a local runtime's.
- The host has the plugin `rutis-host/test`, which provides the service `rutis-host/test`:

| Method | Does |
| --- | --- |
| `load(name, config?, options?)` | Runs the plugin `name`, as the runtime knows it (a file or package for Node and Bun, a module for Python), with `config`, as a row of its own (`test:<name>`, by the session's `run`); returns the row's id. `options` gives fields of the row: `id` (one already there: `a row oslo is loaded already`), and `isolate`, as a row's in `rutis.json` (`{ "forecast": true }`, or a label). Without `id`: the file's name without its extension (`/tests/weather.mjs` is `weather`), else `name`, with `-2`, `-3`… while one is taken. When the row cannot start, it fails with the row's line (`weather: config: city is required`) and no row stays. |
| `update(id, config)` | Gives the row `config`, as a changed row's config: the plugin restarts with it. Fails when the plugin refuses it (the row keeps its config) or fails with it. |
| `unload(id)` | Removes the row; returns once its plugin unloaded in the runtime. |
| `reported(id)` | What the host reported of the row: each of its listeners that failed in an `emit`, each filter of its `emit` that failed, as the host prints them (`weather: listener of weather/ask failed: Error: boom`). |

- `<plugin>` is loaded first (as `load(<plugin>)`), once the runtime greeted: the plugin that drives the test. It injects `rutis-host/test`, provides the fakes the plugin under test uses (`ctx.provide`), listens (`ctx.on`) and emits (`ctx.emit`) as any plugin. It calls `load` outside its `apply` (in a task it starts, or a listener): the loader is starting it while its `apply` runs.
- Plugins loaded this way share the runtime and one event bus with the driving plugin; services are visible to all of them, as in any host.
- It exits once the runtime ends: 0 when every cleanup ran. The driving plugin ends the test by ending its runtime (Node `process.exit(0)`, Python `os._exit(0)`); the runtime says so (`runtime test: row tester called process.exit(0)`). Ctrl-C and SIGTERM stop it as they stop `run`.

What a test tool does: run `rutis-host test <driving plugin> -- <runtime command…> <project>`, read what the driving plugin reports in the output, and wait for it to exit.

A language whose tests call the plugin under test from the test process (Go, Swift, Kotlin) runs the runtime in its own process and uses `--channel`: the test tool hands `rutis-host test` the channel as a host hands one to a local runtime, `<channel>` being `fd:<n>` (an inherited socket, Unix) or `tcp:<host>:<port>` (the test tool listens on a loopback address; `rutis-host` connects and first sends the one-time token in `RUTIS_CHANNEL_TOKEN` and a newline; the only way on Windows), and the runtime greets with the id `test`. The rest is as above; the runtime's output is the test tool's own.

In 0.19, test tools use a local runtime only. Tests on a device (an Android instrumented test, an iOS simulator) do not use `rutis-host test`: there, the application installs its own plugins.

| Case | Output |
| --- | --- |
| The runtime cannot start | `rutis-host: test: cannot start /usr/bin/nodee: No such file or directory (os error 2)` (exit status 1) |
| The runtime ends before greeting | `rutis-host: test: <why>` (exit status 1) |
| The driving plugin cannot start | `rutis-host: ` and its row's line, as `load` fails (`rutis-host: tester: unresolved: …`; exit status 1) |
| No `--`, or no project after the runtime's command | `rutis-host: usage: rutis-host test <plugin> -- <runtime command...> <project>`, the next line `--channel`'s usage (exit status 1) |
| `--channel`'s channel is not there or not written right | `rutis-host: test: fd:97: no such descriptor`, `rutis-host: test: tcp:127.0.0.1:1: Connection refused (os error 61)`, `rutis-host: test: pipe:x: a channel is fd:<n> or tcp:<host>:<port>` (exit status 1) |
| `update` or `unload` of a row `load` did not add | `no row weather was loaded` |

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
    "swift": { "dir": "plugins/swift", "restart": "on-failure" }
  },
  "listen": [
    { "name": "public", "address": "0.0.0.0:7443", "cert": "tls/server.pem", "key": "tls/server.key" }
  ],
  "installed": {
    "py:summarize": { "from": "pypi:bo-summarize" }
  },
  "rows": [
    { "id": "weather", "name": "weather-plugin", "config": { "city": "Oslo" } },
    { "id": "summarize", "name": "py:summarize" },
    { "id": "llm", "name": "py:llm_gateway" },
    { "id": "dns", "name": "go:dns" },
    { "id": "calendar", "name": "swift:calendar" },
    { "id": "local-tool", "name": "./tools/tool.ts" },
    { "id": "gpu", "name": "rutis-host/dial", "config": { "address": "wss://gpu.example.com/rutis" } },
    { "id": "embedder", "name": "gpu:embedder" },
    { "id": "phone", "name": "rutis-host/listen", "config": { "listener": "public" } }
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

If a runtime package is missing, startup fails and prints an installation command. When the Python environment does not exist (the `.venv/bin/python` that `python` names has not been created) or its interpreter lacks the `rutis` package, the error gives the commands that create the environment and install the `rutis` of this host's version (`uv venv <environment>` and `uv pip install --python <interpreter> rutis==<version>`, or `python3 -m venv` and pip); with no environment at all and only a `python3` without `rutis`, it gives the commands that create `<project>/.venv`, which the host then uses by itself.

Rows using a language that `runtimes` does not declare (a `py:llmbox` row without `runtimes.py`, say) enable that runtime with its defaults, as if the file said `"py": {}`, with a warning on stderr:

```
rutis-host: warning: rutis.json: rows use the py runtime (py:llmbox), which runtimes does not declare: it runs with its defaults; declare "runtimes": { "py": {} } to say so
```

`py:` rows enable `py`, `bun:` rows `bun`, and JavaScript or TypeScript files named by a path or a `file:` URL (`.js`, `.mjs`, `.cjs`, `.ts`, `.mts`, `.cts`) `node`; an npm package name is not guessed (it may be another resolver's). Disabled rows do not count. Go and Swift need their binaries named and are never enabled so: for such a row (`go:<plugin>`), `check` says to declare `runtimes.go` (`runtimes.swift`). In `rutis-host dev`, the project's own runtime is set up by `dev`: rows of `rutis.dev.json` in it (`./dev/fake-llm.ts` in a TypeScript project, `py:fake_llm` in a Python one) have no warning; one another runtime runs (a `py:` row in a TypeScript project) has it once, not again each time `rutis.dev.json` is read again.

A runtime enabled so that cannot start with its defaults (no `package.json`, `@arcships/rutis-runtime` not installed, or no `rutis` in the Python environment, say) does not stop the host: a warning on stderr says why and what to do (`rutis-host: warning: the node runtime, which rows use but runtimes does not declare, cannot run with its defaults: …; its rows do not run`), its rows fail as before (`unresolved: no plugin named …`), and the other rows run. A runtime declared under `runtimes` that cannot start still fails the start.

`restart` of `node`, `py`, `bun`, `go` and `swift` (`"no"`, the default; `"on-failure"`; `"exit"`) says what happens when the runtime's process ends on its own (for `go` and `swift`, each of their runtimes'); see [When a runtime ends](#when-a-runtime-ends).

### `listen`

WebSocket listeners used by other nodes, and by the far ends of `rutis-host/listen` rows, to connect. `cert` and `key` are a TLS certificate and private key in PEM format; they can also be supplied through `RUTIS_CERT` and `RUTIS_KEY` (the file's own come first). They go together: with only one of them the host does not start, rather than listening without TLS. A listener without TLS can bind only to a loopback address.

### `installed`

The plugins `rutis-host add` installed, keyed by the plugin's row name (`@ana/wordcount`, `bun:<package>`, `py:<entry point>`, `go:<plugin>`, `swift:<plugin>`), each with one field:

| Field | Meaning |
| --- | --- |
| `from` | The plugin reference without a version (`npm:@ana/wordcount`, `pypi:bo-llm`, a Go module path, a local path): how `remove` and `upgrade` find the package. The plugins of one package have the same. |

The language comes from the key's prefix (none for Node). Records do not change how rows resolve: a row name resolves the same with a record or without. A key in none of these forms is refused:

```text
rutis-host: rutis.json: installed.ruby:geo: name it by its row: a Node package, or bun:, py:, go: or swift: and the plugin
```

No version is recorded: the package manager's own files keep it (`package.json` and the lock file, the Python environment); `check` shows it. A `rutis-host` older than 0.19 reads a file with `installed` as having an unknown field.

### `rows`

Each row runs a plugin:

| Field | Meaning |
| --- | --- |
| `id` | Row name, unique within this file. |
| `name` | Plugin identifier: an npm package (or package subpath), `py:<name>` (Python entry point or module), `bun:<module>` (a module in the Bun runtime), `go:<plugin>` (the Go binary that has it; a plugin name is in one binary only), `swift:<plugin>` (likewise, Swift binaries), `./relative-path` (plugin file), `rutis-host/dial` / `rutis-host/listen` (a connection; see [Remote runtimes and devices](#remote-runtimes-and-devices); nodes connect the same way, see [Connect nodes](nodes.en.md)), or `<connection row>:<plugin>` (a plugin run on a connection's far end). |
| `config` | Plugin configuration. |
| `inject` | Additional service names to wait for. |
| `isolate` | `{ "service": true }` gives this row its own instance of the service; `{ "service": "label" }` shares one instance among rows with that label. |
| `disabled` | When `true`, the row does not run. |

Services are shared by name across all plugins. Any plugin that declares `inject: ["weather"]` can use a `weather` service provided in any language or on any node.

### Remote runtimes and devices

Language runtimes on other machines and apps on devices connecting in are written as **connection rows**. rutis-host ships two connection plugins, `rutis-host/dial` and `rutis-host/listen`; a connection row's id names the connection (the prefix of its output, the name of its token). Local runtimes stay under [`runtimes`](#runtimes), unchanged:

| Plugin | What it does | Config |
| --- | --- | --- |
| `rutis-host/dial` | Dials `address`; once connected, the far end's plugins appear here; after a disconnection it reconnects with backoff | `address`: `wss://…` (or `ws://…` to a loopback address) |
| `rutis-host/listen` | Waits on one listener of [`listen`](#listen) for far ends to connect in, each connection on its own; a newer connection with an id replaces the older | `listener`: the name of a listener under `listen` |

```json
{
  "id": "main",
  "listen": [{ "name": "public", "address": "0.0.0.0:7443", "cert": "server.pem", "key": "server.key" }],
  "runtimes": { "py": { "project": "plugins" } },
  "rows": [
    { "id": "gpu", "name": "rutis-host/dial", "config": { "address": "wss://gpu.example.com:7443/rutis" } },
    { "id": "embedder", "name": "gpu:embedder" },
    { "id": "phone", "name": "rutis-host/listen", "config": { "listener": "public" } }
  ]
}
```

- A plugin run on the far end is the row `<connection row>:<plugin>` (`gpu:embedder`), its configuration in the row's `config`: while the far end is not connected, the row waits; once it is, rutis has the far end run it. The plugin name is resolved on the far end; no language is written. When the far end does not have that plugin, only this row fails, other rows are not affected, and adding such a row is not refused: `embedder: unresolved: entry "embedder" rejected: runtime gpu: this connection does not contain plugin embedder`.
- `<connection row>:<plugin>` rows go with `rutis-host/dial`: when the host decides which plugins the far end runs, the far end's runtime listens and the host dials it. The far ends of `rutis-host/listen` bring only the plugins they installed themselves and provide.
- The plugins a far end installed itself (an app on a device installing its own, or what the far end runs anyway) register as `<far end's id>:<plugin>` (`phone-1:health`), not listed in rutis.json. Their services are visible everywhere, as local ones are; one named like a service already here follows the kernel's rule: the later one is refused, and that remote plugin fails saying so, such as `office:weather: weather is provided here by row weather: not registered from office`.
- The host creates no instance per connection. Logic computed once per device goes on the device, or the host's plugin tells the devices apart itself (an example: [Swift app](swift-app.en.md#once-per-device)).
- The token is `RUTIS_TOKEN_<row id>` (uppercase, `-` written as `_`): dial presents it (without it, it dials with none); listen checks the far ends connecting in with it, all of them presenting this one token. A listen row without a token fails: `phone: set RUTIS_TOKEN_PHONE: the token the far ends present when they connect`. The host side never reads a bare `RUTIS_TOKEN`.
- Dial also trusts the CAs in `RUTIS_CA` (for self-signed certificates).
- A listen row whose listener does not exist fails: `no listener named private: add it to listen`. One listener can carry several listen rows, connections routed to them by token; two entries that would accept the same token stop the host before it starts (see [Credentials](#credentials)).
- Replacing: once a newer connection with an id already connected on a listen row finishes its handshake, the old connection's remote plugins all unload first (in reverse order, every cleanup run), then the old connection is closed with close code 4002, and only then are the new connection's plugins registered. A connection whose handshake fails touches no connection there is.
- When the host stops or the row stops, its connections close with close code 1000; the far end reconnects by its own rules.
- The two plugins take no other config fields: `check` reports a misspelled field by the schema.
- How the connections change: [What `run` prints](#what-run-prints); their state now: `connections` in [Seeing the current state](#seeing-the-current-state); the far end's runtime commands: [Connect nodes](nodes.en.md#a-language-runtime-on-another-machine).
- 0.18's `runtimes.remote` is removed in 0.19: written, it is an unknown field; how to change it: [Migrating to 0.19](../migration-0.19.en.md).
- A connection of your own (another transport, your own authentication): hand over a Channel: write a connection plugin that calls `Session::new(channel)`; other languages use the service `sessions`' `open`. See [Embed in a Rust application](rust-host.en.md) and the [plugin API](plugin-api.en.md).

## Credentials

Do not put credentials in the file; they are read from the environment when a row dials or a far end dials in:

| Environment variable | Purpose |
| --- | --- |
| `RUTIS_TOKEN_<NAME>` | A token by name (uppercase, with `-` written as `_`): `rutis-host/dial` and `rutis-host/listen` rows by row id, presented by dial and checked by listen against the far ends connecting in (all of them presenting this one). The host side reads only these per-name variables, never a bare `RUTIS_TOKEN`. |
| `RUTIS_CA` | Additional trusted CA certificate in PEM format for `wss://` connections. |
| `RUTIS_CERT` / `RUTIS_KEY` | Listener certificate and key when they are not specified under `listen`. |

On one listener, each `rutis-host/listen` row must accept a token of its own, disabled ones included (enabling one would clash). When two would accept the same token (two `RUTIS_TOKEN_<name>` variables set to one value), a connection could not tell which it is for, so `run`, `check` and a configuration reload fail before starting, naming both rows and the two variables, one of which needs another value:

```
rutis-host: listener public: row main accepts RUTIS_TOKEN_MAIN and row phone accepts RUTIS_TOKEN_PHONE, the same token, so a connection presenting it cannot be told apart: give RUTIS_TOKEN_MAIN or RUTIS_TOKEN_PHONE another value
```

The environment cannot be changed from outside the process, so these tokens cannot be revoked while it runs: to cut off a connected node or a device that connects in, change the token and restart `rutis-host`, which drops every connection. A connection there is now is cut off by stopping its row: the row connecting it stops, and its connection closes with it.

## Deployment

Place `rutis.json`, the Node project (with plugins and `@arcships/rutis-runtime` installed), the Python environment (with `rutis` and plugins installed), and the Go and Swift plugin directories (binaries for the platform) together. Run `rutis-host run /srv/app/rutis.json` under a process manager such as systemd. When `rutis-host` exits, the runtime processes it started exit as well.

To apply a changed `rutis.json`, use `systemctl reload` (`ExecReload=/bin/kill -USR1 $MAINPID`), or run with `--watch-config`; see [Changing the configuration while it runs](#changing-the-configuration-while-it-runs).

Installed with npm (`ExecStart=/srv/app/node_modules/.bin/rutis-host run /srv/app/rutis.json`), `MAINPID` is the launcher `bin/rutis-host.mjs`, a Node process that runs the `rutis-host` binary. It passes SIGTERM and SIGUSR1 on to the binary, so `systemctl stop` and `systemctl reload` work as with the binary, and it exits as the binary does. It does not pass on SIGINT and SIGHUP (a terminal sends them to the binary itself): a SIGINT sent to the launcher alone does nothing, so stop it with SIGTERM. The binary from GitHub Releases or crates.io, and the one a PyPI install puts in the environment (`.venv/bin/rutis-host`), have no launcher.

When stopping, have the process manager send SIGTERM to `rutis-host` alone and let it unload the rows. By default (`KillMode=control-group`), systemd also sends SIGTERM to the runtime processes, which then exit before their cleanups run; set `KillMode=mixed`, and set `TimeoutStopSec` longer than the cleanup deadline. If `rutis-host` is killed (SIGKILL, or ending the process on Windows), it cannot unload the rows: on Windows, the runtime processes are ended with it; on Unix, they notice that their channel closed, unload their rows themselves, and exit; one a plugin keeps busy ends a second after the cleanup deadline.
