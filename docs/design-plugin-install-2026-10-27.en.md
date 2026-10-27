# Plugin installation: `rutis-host add` / `remove` / `upgrade` and an embedding API

[中文](design-plugin-install-2026-10-27.md)

Status: draft, not implemented. Date: 2026-10-27. Base: `main` `103c0d22` (after 0.17.2). Scheduled: 0.19.
Issue: [#691](https://github.com/arcships/rutis/issues/691) (part of [#663](https://github.com/arcships/rutis/issues/663), H2 and O4).
Relation to the 0.18 designs: the plugin description `add` prints is the description `check` already prints (version, `inject`, `provides`, config schema), not a second one; rows `add` writes carry no `config`, defaults are filled by the SDK as in [0.18 config schema](design-config-schema-2026-10-27.en.md) C1, and when a required field is missing `add` says so with the same check `check` runs; error text follows [0.18 diagnostics](design-diagnostics-2026-10-26.en.md) §4.5.
Compatibility: fully compatible, not part of "Migrating to 0.19". The old row names (`py:`, `bun:`, `go:`, `swift:` prefixes, npm package names) and manual installation keep working, unconverted.

Out of scope: sandboxing or reviewing install scripts (`add` runs the package manager, the same as running npm / uv / go directly, see §10); installing the language toolchains themselves (Node, Bun, Python, Go); version ranges (`^1.2`, `>=0.3`); pnpm, yarn, private registry settings (use the package managers' own config files and environment variables); reinstalling every recorded plugin on a deployment machine; hot upgrade of a running host (§7); remote runtimes and plugins in applications (Kotlin); downloading Swift / Go binaries from URLs; flags of `add` that set the row id or config; JSON output (the embedding API returns structures, §9).

## 1. The scenario: where it gets stuck now

The owner of deskmate wants two plugins others published: a TypeScript plugin `@ana/wordcount` on npm and a Python plugin `bo-llm` on PyPI (entry point `llm`). Today ([rutis-host guide](guide/rutis-host.en.md), "Quick start"):

```bash
npm init -y && npm install @ana/wordcount                          # know first that it is TypeScript
uv venv && uv pip install --python .venv/bin/python "rutis==0.19.0" bo-llm   # know first that it is Python, create the environment, pin the runtime
# rutis.json: runtimes node and py; one row named by the package, one by py: plus the entry point (not the package name)
```

In the #663 evaluation, the host author spent the most time on "where each language's plugins go and how rows are named"; a host embedding rutis, like deskmate, has to write discovery, installation and dependency handling itself (`deskmate install <dir>`, `deskmate setup`).

The goal:

```bash
rutis-host add npm:@ana/wordcount
rutis-host add pypi:bo-llm
rutis-host run
```

The host owner says only where the plugin comes from (whether they found it on npm or PyPI); they need not know its language, package manager, environment or row prefix. An embedding host builds its own "install a plugin" on the same code.

## 2. What exists

| Exists | Where | How this design uses it |
| --- | --- | --- |
| One `Language` per language: its runtime key (`node`, `bun`, `py`, `go`, `swift`), defaults (`enable_default`), recognizing a project by its files (`of_project`) | `crates/rutis-host/src/languages/` | A local directory's language is decided by `of_project`; the recorded language is the runtime key |
| Implicit runtimes: a runtime rows use and `runtimes` omits is enabled with its defaults (#666) | `HostConfig::enable_used_runtimes` | Recorded languages are enabled the same way |
| Telling a Bun project: it depends on `@arcships/rutis-bun` | `languages/bun.rs` `runs_in_bun` | Whether an npm package is a Bun or Node plugin, by the same rule |
| `rutis-host go add`: `GOBIN=<dir> go install` | `crates/rutis-host/src/main.rs` | Moves into the install code; `go add` stays |
| A runtime describing a row: version, `inject`, `provides`, `announces`, config schema (`RowSchema`) | `RuntimeResolver` → `describe_row` (`crates/rutis-loader/src/runtime.rs`) | The plugin description, which `check` already prints |
| Go and Swift binary manifests (plugin names, versions, SDK marker) | `crates/rutis-loader/src/runtime/binary.rs` | Finding the plugins in a binary; recognizing a local binary |
| The `rutis.plugins` entry point group of Python plugins (the template writes it) | `templates/python/pyproject.toml` | Finding the plugins in a Python distribution |
| The commands the Python runtime names when the environment or `rutis` is missing | `languages/python.rs` | `add` runs those commands |

Cordis 4 has no install command; Koishi's plugin market installs with npm into the project and then writes the config, the same approach as here, which this design carries to the other languages. Cordis behaviour is unchanged.

## 3. Decisions

| # | Decision | Section |
| --- | --- | --- |
| I1 | `rutis-host add <reference> [rutis.json]`, `remove <plugin or reference>`, `upgrade [<plugin or reference>[@version]]`; `add --dry-run` to see the description before installing; `go add` stays | 5, 7, 8 |
| I2 | A plugin reference names the source, not the language: `npm:`, `pypi:` prefixes; Go module paths and local paths say their source themselves; versions are always `@version`; a name without a source is an error | 4 |
| I3 | The package decides the language: an npm package depending on `@arcships/rutis-bun` is Bun, otherwise Node; PyPI is Python; a Go module is Go; a local directory by its project file; a local executable by its SDK marker (Go, Swift) | 4 |
| I4 | Install where the host already looks: where `runtimes.<language>` points when written, its default otherwise; Go and Swift default to `plugins/go`, `plugins/swift` | 5 |
| I5 | Prepare the runtime when missing: the Node project and `@arcships/rutis-runtime`, the Bun project and `@arcships/rutis-bun`, the Python environment and `rutis`, the Go / Swift directory; Node, Bun, Python and Go themselves are not installed | 5 |
| I6 | The install record is the top-level `installed` of `rutis.json`, keyed by plugin name; rows name only the plugin | 6 |
| I7 | A recorded language enables its runtime with its defaults, without a warning; Go and Swift use the recorded binaries | 6 |
| I8 | The plugin description is what plugins already declare (the runtime's description, which `check` prints); no new manifest format; `provides` stays optional | 8 |
| I9 | The embedding API is the `install` feature of `rutis-loader` (off by default): `Installer`; the host keeps the record itself; the `rutis` kernel is unchanged | 9 |
| I10 | All or nothing: when any step fails, `rutis.json` is unchanged and the package just installed is uninstalled | 5 |
| I11 | A running host picks up new rows by the existing reload rules (`run --watch-config`, `dev`); `upgrade` does not swap code in a running host | 7 |

## 4. Plugin references

| Written | Source | Language |
| --- | --- | --- |
| `npm:@ana/wordcount`, `npm:@ana/wordcount@1.2.0` | npm | Bun when the package's `dependencies` have `@arcships/rutis-bun`, otherwise Node (read before installing with `npm view <package>@<version> dependencies --json`) |
| `pypi:bo-llm`, `pypi:bo-llm@0.3.0` | PyPI | Python |
| `github.com/bo/netkit/cmd/netkit`, `…@v0.4.1` | Go module (through the Go module proxy) | Go |
| `./plugins/notes` (a directory) | local | By project file: `package.json` (Bun or Node as above), `pyproject.toml`, `go.mod` (the existing `of_project`) |
| `./wordcount-1.2.0.tgz`, `./bo_llm-0.3.0-py3-none-any.whl` | local package | `.tgz`: the `package.json` inside; `.whl` is Python |
| `./netkit` (an executable) | local binary | Go with the Go SDK marker, Swift with the Swift SDK marker (the existing manifest reading) |

- **Version**: after the last `@` that is not the first character (`@ana/wordcount@1.2.0`); none means the latest. A version is one exact version or a tag (npm's `latest`, Go's `latest`), passed to the package manager in its own form (PyPI gets `bo-llm==0.3.0`).
- **Local paths** start with `./`, `../` or `/` (or a drive on Windows), the same rule as for plugin files in rows, relative to the directory of `rutis.json`.
- **Go module paths** are recognized by Go's own rule: the first element contains a `.` (`github.com`, `example.org`). `go:` is not a source prefix: it is the old row prefix, which means the language.
- **A name without a source** (`wordcount`) is an error naming the forms to choose from. The same name may exist on npm and on PyPI; a wrong guess installs someone else's package.

Why the source and not the language: the source is what the host owner knows (the page where they found the plugin); the language belongs to the package. One source can carry two languages (an npm package may be a Node or a Bun plugin), so the language is read from the package, not written by a person.

## 5. What `add` does

1. Parse the reference into its source (§4).
2. Decide the language (§4).
3. Prepare the runtime and install the package:

| Language | Installed into | When the runtime is missing | Installing |
| --- | --- | --- | --- |
| Node | `runtimes.node.project` (default: the directory of `rutis.json`) | Without `package.json`, write `{ "private": true }`; when neither the project has `@arcships/rutis-runtime` nor the host carries one (the npm rutis-host does), install `@arcships/rutis-runtime@<host version>` with it | `npm install <package>[@version]` (npm links a local directory) |
| Bun | `runtimes.bun.project` (default as above) | As above, with `@arcships/rutis-bun@<host version>` | `bun add <package>[@version]` |
| Python | The interpreter the host's runtime will use: `runtimes.py.python`, else `$VIRTUAL_ENV`, else `<runtimes.py.project>/.venv` | When there is none, create `<project>/.venv` (`uv venv`; without uv, `python3 -m venv`); never install into the system Python | `uv pip install --python <interpreter> <package>` (without uv, `<interpreter> -m pip install`), with `rutis==<version>` in the same command: the host's version when the environment has no `rutis`, its current version when it has one, so installing a plugin never changes the runtime's version. A local directory is installed with `-e`, so code edits need no reinstall |
| Go | `runtimes.go.dir`, or `plugins/go` without `runtimes.go` | Create the directory | `GOBIN=<dir> go install <module>@<version>` (`@latest` without a version); a local directory: `go install ./...` inside it; a local binary is copied into the directory |
| Swift | `runtimes.swift.dir`, or `plugins/swift` | Create the directory | Local binaries only: copied into the directory |

   `runtimes.go` (`swift`) written without a `dir` is an error: `add` does not know where to install. The package manager's output is shown as it is.
4. Find the plugins in the package: Node and Bun, the package name (one plugin per package); Python, the distribution's entry points in the `rutis.plugins` group (a package may have several); Go and Swift, the plugins in the binary's manifest.
5. Describe each plugin: start its runtime and describe the row as `check` does (§8), and print it.
6. Write the file: a record per plugin in `installed`, and a row `{ "id": <id>, "name": <plugin name> }` per plugin at the end of `rows`. The id comes from the plugin name (the existing `id_of`: `@ana/wordcount` → `wordcount`); when another row has it, `-2`, `-3` is appended. When a row with that name already exists, none is added. Keys are written back in their order, indented by two spaces, to a temporary file that is then renamed (`--watch-config` never reads a half-written file).
7. Check the new rows with the same check `check` runs, and print the result: a service the row needs that nothing provides, a missing required config field (0.18), show here. `add` still exits 0: the row is written; add the dependency or the config next.

Which file: the second argument; without it `rutis.json` in the current directory; with no `rutis.json` but a `rutis.dev.json` (a plugin project), that one; with neither, a new `rutis.json` (`{ "format": 1, "installed": …, "rows": […] }`).

When any of steps 2 to 6 fails: the file is unchanged, the package step 3 just installed is uninstalled (a package that was there before is left alone), and `add` exits 1.

## 6. The install record and the rows

```json
{
  "format": 1,
  "installed": {
    "@ana/wordcount": { "from": "npm:@ana/wordcount", "version": "1.2.0", "language": "node" },
    "llm":            { "from": "pypi:bo-llm", "version": "0.3.0", "language": "py" },
    "dns":            { "from": "github.com/bo/netkit/cmd/netkit", "version": "v0.4.1", "language": "go", "binary": "plugins/go/netkit" }
  },
  "rows": [
    { "id": "wordcount", "name": "@ana/wordcount" },
    { "id": "llm", "name": "llm" },
    { "id": "dns", "name": "dns" }
  ]
}
```

- The key is the plugin name, which is the row's `name`. `from` is the reference without a version, `version` the exact version installed, `language` the runtime key; Go and Swift also have `binary` (relative to the file's directory). Several plugins of one package share `from`.
- Why in `rutis.json` and not in a file beside it: one file and one concept fewer; the host owner sees what is installed and what runs in one place; a deployment carries `rutis.json` anyway. `add` writes it; people can read and edit it.
- When a row's name is a recorded key, rutis-host hands it to that language's resolution by the record: Node as it is, Bun `bun:<package>`, Python `py:<entry point>`, Go `go-<binary>:<plugin>`, Swift `swift-<binary>:<plugin>`. Output still shows the name the row wrote. A name that is not a recorded key resolves as it does today.
- **Runtimes follow** (I7): a recorded language without an entry in `runtimes` is enabled with its defaults, as in #666, but without the warning (the record says so); one `runtimes` declares is used as declared. For Go and Swift without an entry, the runtimes are the recorded binaries (`binaries`): the whole directory is not trusted.
- **Name clashes**: when the plugin name is already another package's record, `add` refuses and names the package. Rows in the old form (`py:llm`) have nothing to do with the record and do not clash.
- An older rutis-host reading a file with `installed` reports an unknown field and lists the fields it takes, as it does today. `format` stays 1: `installed` changes the meaning of no other field.

## 7. `remove` and `upgrade`

- `rutis-host remove <plugin or reference> [rutis.json]`: uninstall the package the plugin is in (`npm uninstall`, `bun remove`, `uv pip uninstall`, deleting the binary), delete all the package's records and the rows using those plugin names (in groups too), printing each row removed. When a package has several plugins, naming one removes the whole package, and says so. A name not in the record is refused, saying it was not installed by `add` and to uninstall it with its package manager.
- `rutis-host upgrade [<plugin or reference>[@version]] [rutis.json]`: without a name, every record. Install the latest version from the recorded source (or the version given) and update `version`; a local reference is reinstalled from the same path. Rows are not changed. A plugin the new version adds gets one line saying how to add a row (`add` the same reference); a plugin it no longer has loses its record, and its rows get a line saying the new version does not have it.
- **A running host** (I11): `upgrade` replaces the package on disk only. A running host keeps the old code until it restarts; `upgrade` says so at the end. Why no hot swap: reloading a row re-imports only its own entry module (the existing `dev` rule), while an upgrade replaces the whole package; half old and half new is harder to diagnose than not swapping.

## 8. Seeing the description before installing

A plugin's description is what the plugin already declares: TypeScript `definePlugin({ inject, provides, config })`, Python `define_plugin(...)`, Go's manifest and `rutis.Plugin`. The runtime imports the plugin and describes the row (`RowSchema`); `check` already prints that as the `version`, `inject`, `provides` and `config` lines.

`rutis-host add --dry-run <reference>` installs the package into a temporary directory (a temporary Node project, Python environment or Go directory, by the same commands as step 3), describes each plugin in it, prints the description, and deletes the temporary directory; `rutis.json`, the host's project and its environment are untouched.

- No new manifest format: all four languages already declare these the same way, and `check` uses them; another manifest would be one more rutis-specific thing for plugin authors to write, and could disagree with the code.
- No registry metadata (`npm view`, PyPI's JSON): the declarations of TypeScript leaf plugins, Python and Go are in code, not in package metadata; only Cordis plugins have `rutis.provides` in `package.json`.
- `provides` stays optional (the 2026-10-18 decision: provided services are exposed automatically). Without it the description says `provides: none declared (it may provide more once it runs)`.
- The cost: `--dry-run` downloads and imports the plugin, and runs code from the package as `add` does (§10).

## 9. Embedding API

`rutis-loader` gets an `install` feature (off by default); with the language features (`node`, `python`, `bun`, `go`, `swift`) it decides which languages can be installed:

```rust
use rutis_loader::install::{Installer, Places, Reference};

let installer = Installer::new(Places {
    node: Some(home.join("plugins/node")),   // Node project
    python: Some(home.join("plugins/.venv")), // Python environment (created when missing)
    go: Some(home.join("plugins/go")),       // binaries directory
    ..Places::default()
});
let reference: Reference = "npm:@ana/wordcount".parse()?;
let about = installer.describe(&reference).await?;   // before installing: install in a temporary directory, describe, delete
let installed = installer.add(&reference).await?;    // Installed: source, version, language, plugins
for plugin in &installed.plugins {
    rows.push(json!({ "id": plugin.id, "name": plugin.row_name }));   // the loader's row name (py:llm and so on)
}
installer.upgrade(&installed, None).await?;
installer.remove(&installed).await?;
```

- **In rutis-loader**: embedding hosts depend on it and its language features already; the runtimes that describe a plugin are there too. The `rutis` kernel is unchanged and knows no package manager; `rutis-loader` too only runs npm, uv and go as external commands when called, and does not depend on them at build time. rutis-host's library is not a public API (`lib.rs` says it has no semver promise), so not there; and no new crate for it.
- **The host keeps the record**: `Installed` serializes, and the host stores it where it likes (rutis-host in `installed`, deskmate in its own plugin list). The API reads and writes no configuration file.
- **Row names**: the API returns the row names the loader takes (`py:llm`, `bun:@ana/x`, `go-netkit:dns`). An embedding host writes rows in code and needs no unprefixed names; those are a convenience of `rutis.json` for people.
- `describe` returns the existing `RowSchema`, not a new type.
- rutis-host's `add` / `remove` / `upgrade` and `go add` all call it: the install logic exists once.

## 10. Other designs, and security

- **Local paths and #185**: a plugin the host or its agent writes is a local directory, installed with the same `add ./plugins/notes`, its language decided by its project file. npm links it, Python installs it with `-e`, Go rebuilds it on every `upgrade`, so the agent's edits need no new `add`; they take effect when the row next reloads or restarts. How the agent triggers a reload and how services become tools belongs to the #185 design.
- **`dev` (#670)**: in a plugin project `add` writes `rutis.dev.json`, which `dev` reads again by the existing rules: the new row is `added` and starts, the other rows are untouched, shared runtimes do not restart. When the runtime the new row needs is not running (a TypeScript project's first Python plugin), the reload is refused with a restart hint, as adding a runtime is today, with the reason now naming the runtime: `reload rejected: rows now use the py runtime, which is not running: restart the host; nothing changed`. The same for `run --watch-config`.
- **#683 (governing plugins)**: which process a plugin runs in and which services it may use are #683's row options, unrelated to how it was installed.
- **#698 (remote runtimes)**: installs go to local runtimes only; plugins of a remote runtime are installed on that side.
- **Security**: `add`, `upgrade` and `--dry-run` run the package manager, which may run install scripts in the package, and import the plugin's code to describe it; the same as the host owner running `npm install`, `uv pip install` or `go install` directly. rutis adds no sandbox or review; the guide says: install only plugins you trust.

## 11. What developers see

```text
$ rutis-host add npm:@ana/wordcount
(npm's output)
installed @ana/wordcount 1.2.0 (node) into /app
wordcount (@ana/wordcount): cannot run: nothing provides notes
  version: "1.2.0"
  inject: ["notes"]
  provides: {"tools":{"register":"async"}}
  config: {"type":"object","properties":{"limit":{"type":"integer","default":100}}}
rutis.json: added row wordcount

$ rutis-host add pypi:bo-llm
created the Python environment /app/.venv
(uv's output)
installed bo-llm 0.3.0 (py) into /app/.venv: plugin llm
llm (llm): ok
  version: "0.3.0"
  provides: none declared (it may provide more once it runs)
rutis.json: added row llm

$ rutis-host add --dry-run github.com/bo/netkit/cmd/netkit
(go's output)
github.com/bo/netkit/cmd/netkit v0.4.1 (go): plugin dns
dns
  version: "0.4.1"
  provides: {"dns":{"lookup":"async"}}
nothing was installed (--dry-run)

$ rutis-host remove llm
uninstalled bo-llm from /app/.venv
rutis.json: removed row llm

$ rutis-host upgrade
@ana/wordcount: 1.2.0 → 1.3.0
bo-llm: 0.3.0 is the latest
a running host keeps the old versions until it restarts
```

On errors (stderr, exit 1, file unchanged):

| Case | Output |
| --- | --- |
| No source | `rutis-host: wordcount: say where it comes from: npm:wordcount, pypi:wordcount, a Go module path or a local path` |
| No package manager | `rutis-host: cannot run npm (No such file or directory): install Node.js 22 or later`; Python with neither uv nor `python3`: `cannot create the Python environment /app/.venv: install uv, or Python 3.10 or later`; Go keeps its existing text |
| The package manager fails | Its own output, then `rutis-host: npm install @ana/wordcount failed (exit status 1); rutis.json is unchanged` |
| Not a plugin | `rutis-host: bo-utils has no rutis.plugins entry point: it is not a plugin, or its author did not register one; uninstalled it again`; a Node package that is not a plugin once imported gets the 0.18 text (the default export is not the result of `definePlugin`), followed by the same `; uninstalled it again` |
| A local directory not recognized | `rutis-host: ./plugins/notes has no package.json, pyproject.toml or go.mod` |
| The name is another package's | `rutis-host: llm is already installed from pypi:other-llm: remove it first; uninstalled bo-llm again` |
| No Go directory | `rutis-host: runtimes.go has no dir to install into: add one, or remove runtimes.go to use plugins/go` |
| `remove` of a name not recorded | `rutis-host: llm was not installed by add: uninstall it with the package manager that installed it, and remove its row` |
| A reload needing a runtime that is not running | `rutis-host: reload rejected: rows now use the py runtime, which is not running: restart the host; nothing changed` |

## 12. Compatibility and migration

- Nothing existing breaks: a `rutis.json` without `installed` behaves exactly as before; `py:`, `bun:`, `go:`, `swift:` rows and npm-name rows resolve as they do; plugins installed by hand run as they do; `rutis-host go add` stays. Old files are not converted, and get no warning.
- The one text change is the refusal reason in §10 (only when a reload is refused for a new runtime).
- Not part of "Migrating to 0.19". The guide's "How a host installs plugins" puts `add` first and keeps the manual table as "without `add`".

## 13. Implementation

| Step | Content | Done when |
| --- | --- | --- |
| P1 | The `install` feature of `rutis-loader`: parsing `Reference`, `Places`, `Installer`'s `add` / `remove` / `upgrade` / `describe`, rollback on failure; `go add` calls it | Unit tests: for each form in §4, the source, language and package manager command (fake commands on `PATH` that record their arguments); on failure the package just installed is uninstalled and one already there is left alone |
| P2 | rutis-host: the `installed` field, resolving row names by the record, runtimes from the record, `add` (with `--dry-run`) / `remove` / `upgrade`, writing the file back, the §10 refusal text | `crates/rutis-host/tests/install.rs`: record → runtimes and row names; old forms beside records; write-back keeps key order; every error output of §11 compared word for word |
| P3 | End to end: `tests/e2e/tests/plugin_install.rs` (in ci.yml's e2e job, beside `dev_loop.rs`) | One scenario each for Node, Bun, Python and Go: local packages made from `rutis-host new` templates (`npm pack`, the wheel of `uv build`, a Go module directory) → `add --dry-run` → `add` → `check` → `run`, the probe calls its service → `upgrade` after a version change → `remove`, residue checks pass; the Python scenario starts with no environment (`add` creates it); the `pypi:` and Go module path forms run once against uv's local `--find-links` and `GOPROXY=file://`; one `dev` scenario: after `add` writes `rutis.dev.json` the new row is `added` and the project's own row does not restart. The install smoke test (`tests/e2e/tests/install.rs`) gets one step on the binary channel: `add` a wheel in a directory without `.venv`, with the environment's `rutis` from this build |
| P4 | Documentation | The [rutis-host guide](guide/rutis-host.en.md) (command table, "Quick start", "How a host installs plugins", the `installed` field), the host owner guide (#671, formerly #685), the `Installer` section of [Embedding in a Rust application](guide/rust-host.en.md), both languages in step; installation scripts and trust (§10) |

P1 → P2 → P3; P4 with P2. Against the issue's "Done": the commands, the API and the end-to-end tests of the four languages are P1–P3; the two guides are P4.
