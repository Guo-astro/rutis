# Plugin installation: `rutis-host add` / `remove` / `upgrade`

[中文](design-plugin-install-2026-10-27.md)

Status: final, not implemented. Date: 2026-10-27. Base: `main` `103c0d22` (after 0.17.2). Scheduled: 0.19.
Issue: [#691](https://github.com/arcships/rutis/issues/691) (part of [#663](https://github.com/arcships/rutis/issues/663), H2 and O4).
Changed 2026-10-28 (I6, §6): `add` writes rows with the existing row names (the Node package name, `bun:`, `py:`, `go:`, `swift:`); `installed` is keyed by row name and records `from` only; the layer "a row whose name is a record's key resolves by the record" is gone, and row names resolve exactly as before. Why: rewriting names by the record gave one plugin two row names, and added a naming rule duplicating the existing forms.
Changed again 2026-10-28 (I1, I8, §8, §9): the embedding API (the `install` feature of `rutis-loader`) and seeing the description before installing (`add --dry-run`, `describe`) are gone; the install code lives inside rutis-host only. Why: see Out of scope.
Reviewed 2026-10-28 (§5, §8, §11): `add` starts no runtime of its own to describe plugins: before writing the file it works out in memory the configuration as it will be written, checks the new rows with the start and check `run` / `check` use, and writes the file only when they pass; it prints with `check`'s own code, word for word the same.
Relation to the 0.18 designs: the plugin description `add` prints is the description `check` already prints (version, `inject`, `provides`, config schema), not a second one; rows `add` writes carry no `config`, defaults are filled by the SDK as in [0.18 config schema](design-config-schema-2026-10-27.en.md) C1, and when a required field is missing `add` says so with the same check `check` runs; error text follows [0.18 diagnostics](design-diagnostics-2026-10-26.en.md) §4.5.
Changed 2026-10-29 (I1, Compatibility): `rutis-host go add` is removed ([platform cleanup](design-platform-cleanup-2026-10-28.en.md) §6.4 item 1, CL25). `add <Go module>` installs with the same `go install` and adds the row; installing without a row is no longer a use: a plugin needs a row to be loaded anyway.
Compatibility: fully compatible apart from removing `go add` (in the platform cleanup section of "Migrating to 0.19"). The old row names (`py:`, `bun:`, `go:`, `swift:` prefixes, npm package names) and manual installation keep working, unconverted.

Out of scope: sandboxing or reviewing install scripts; installing the language toolchains; version ranges; pnpm, yarn, private registry settings (use the package managers' own config); reinstalling every recorded plugin; hot upgrade of a running host; remote runtimes and plugins in applications; downloading binaries from URLs; undoing what the package manager already did when a later step fails (running npm / uv directly does not either, §5); an embedding API (install code in `rutis-loader` for other hosts to call): rutis-host is its only user, deskmate in the evaluation is a hypothetical host, and without a real need there is no public API; seeing the description before installing (`add --dry-run`): `add` prints the plugin's description once installed anyway, which is not worth a second path installing into a temporary directory.

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

The host owner says only where the plugin comes from; they need not know its language, package manager, environment or row prefix.

## 2. What exists

| Exists | Where | How this design uses it |
| --- | --- | --- |
| One `Language` per language: its runtime key (`node`, `bun`, `py`, `go`, `swift`), recognizing a project by its files (`of_project`) | `crates/rutis-host/src/languages/` | A local directory's language is decided by `of_project`; the row name's prefix is the runtime key |
| Telling a Bun project: it depends on `@arcships/rutis-bun` | `languages/bun.rs` `runs_in_bun` | Whether an npm package is a Bun or Node plugin, by the same rule |
| The commands a runtime names when something is missing: Node `npm init -y`, `npm install @arcships/rutis-runtime`; Bun `bun add -d @arcships/rutis-bun`; Python creating `.venv` and installing `rutis==<host version>` (`create_venv`, `missing_rutis`) | `languages/node.rs`, `bun.rs`, `python.rs` | `add` runs those commands; nothing new is defined |
| `rutis-host go add`: `GOBIN=<dir> go install` | `crates/rutis-host/src/main.rs` | Moves into the install code, called by `add`; `go add` removed (CL25) |
| A runtime describing a row: version, `inject`, `provides`, config schema (`RowSchema`) | `RuntimeResolver` → `describe_row` (`crates/rutis-loader/src/runtime.rs`) | The plugin description, which `check` already prints |
| Go and Swift binary manifests (plugin names, versions, SDK marker); a `go:<plugin>` row finds its binary in the directory by manifest | `crates/rutis-loader/src/runtime/binary.rs` | Finding the plugins in a binary; recognizing a local binary; the record needs no binary file name |
| The `rutis.plugins` entry point group of Python plugins (the template writes it) | `templates/python/pyproject.toml` | Finding the plugins in a Python distribution |
| A running host refusing a `runtimes` change, naming it (`runtimes.py added: restart the host`) | `host::restart_needed` | A running `add` of a new language's plugin is refused by this rule |

Cordis 4 has no install command. Koishi's plugin market (the installer service of `@koishijs/plugin-market`) is a layer over npm / yarn: it edits `package.json` dependencies, runs the package manager and writes the plugin into the config; on failure it reports the package manager's exit code and undoes nothing; after dependencies change it needs a restart. This design's defaults match it, carried to the other languages. Cordis behaviour is unchanged.

## 3. Decisions

| # | Decision | Section |
| --- | --- | --- |
| I1 | `rutis-host add <reference> [rutis.json]`, `remove <plugin or reference>`, `upgrade [<plugin or reference>[@version]]`; `go add` removed (CL25) | 5, 7 |
| I2 | A plugin reference names the source, not the language: `npm:`, `pypi:` prefixes; Go module paths and local paths say their source themselves; versions are always `@version`; a name without a source is an error | 4 |
| I3 | The package decides the language: an npm package depending on `@arcships/rutis-bun` is Bun, otherwise Node; PyPI is Python; a Go module is Go; a local directory by its project file; a local executable by its SDK marker (Go, Swift) | 4 |
| I4 | Install where the host already looks: where `runtimes.<language>` points when written; otherwise its default (`plugins/go`, `plugins/swift` for Go and Swift), and the settings used are written into `runtimes` (`"py": {}`, `"go": { "dir": "plugins/go" }`), so the runtime starts by the existing rules, with no new inference rule | 5 |
| I5 | Prepare the runtime when missing: run the commands the runtime names today when something is missing (§2), and create the Go / Swift directory; Node, Bun, Python and Go themselves are not installed | 5 |
| I6 | The install record is the top-level `installed` of `rutis.json`, keyed by row name, with only `from`; rows use the existing row names, which `add` writes | 6 |
| I7 | The plugin description is what plugins already declare (the description `check` prints); no new manifest format; `provides` stays optional | 8 |
| I8 | The install code is inside rutis-host only (a private module): no `rutis-loader` feature, no new public types; the `rutis` kernel is unchanged | 9 |
| I9 | `rutis.json` is written last, so it is unchanged when any earlier step fails; what the package manager already installed is not undone, and the error says how to uninstall it | 5 |
| I10 | A running host picks up new rows by the existing reload rules (`run --watch-config`, `dev`); `upgrade` only replaces the packages on disk, taking effect on restart | 7 |

## 4. Plugin references

| Form | Source | Language |
| --- | --- | --- |
| `npm:@ana/wordcount`, `npm:@ana/wordcount@1.2.0` | npm | Bun when the package's `dependencies` include `@arcships/rutis-bun`, otherwise Node (read before installing with `npm view <package>@<version> dependencies --json`) |
| `pypi:bo-llm`, `pypi:bo-llm@0.3.0` | PyPI | Python |
| `github.com/bo/netkit/cmd/netkit`, `…@v0.4.1` | A Go module (through the Go module proxy) | Go |
| `./plugins/notes` (a directory) | Local | By its project file: `package.json` (Bun / Node as above), `pyproject.toml`, `go.mod` (the existing `of_project`) |
| `./wordcount-1.2.0.tgz`, `./bo_llm-0.3.0-py3-none-any.whl` | A local package | `.tgz` by the `package.json` inside; `.whl` is Python |
| `./netkit` (an executable) | A local binary | Go with the Go SDK marker, Swift with the Swift SDK marker (the existing manifest reading) |

- **Version**: after the last `@` that is not the first character; none means the latest. A version is an exact version or a tag, written for the package manager in its own form (`bo-llm==0.3.0` for PyPI).
- **Local paths** start with `./`, `../` or `/` (plus drive letters on Windows), the same rule as plugin files in rows; relative to the directory of `rutis.json`. A local path is handed to the package manager as it is, and installed the package manager's default way (npm links it, uv copies it).
- **Go module paths** by Go's own rule: the first element contains a `.`. `go:` is not a source prefix; it is the old row prefix and means a language.
- **A name without a source** (`wordcount`) is an error naming the forms to choose from. The same name may exist on both npm and PyPI; a wrong guess installs someone else's package.

Why the source and not the language: the source is what the host owner knows (the page where they found the plugin); the language is the package's business. One source can hold two languages (an npm package can be a Node or a Bun plugin), so the language is read from the package, not written by people.

## 5. What `add` does

1. Parse the reference into its source and language (§4).
2. Prepare the runtime and install the package:

| Language | Installs into | When the runtime is missing | Install |
| --- | --- | --- | --- |
| Node | `runtimes.node.project` (default: the directory of `rutis.json`) | Without a `package.json`, `npm init -y`; when the runtime finds no `@arcships/rutis-runtime` (the existing lookup in `node.rs`), install it too, at the host's version | `npm install <package>[@version]` |
| Bun | `runtimes.bun.project` (the same default) | The same, `bun add -d @arcships/rutis-bun@<host version>` | `bun add <package>[@version]` |
| Python | The interpreter the host's runtime will use: `runtimes.py.python`, else `$VIRTUAL_ENV`, else `<runtimes.py.project>/.venv` (the existing order in `python.rs`) | When there is none, create `<project>/.venv` with `create_venv`'s commands (which install `rutis==<host version>`); when the environment lacks `rutis`, install it with `missing_rutis`'s command | `uv pip install --python <interpreter> <package>` (without uv, `<interpreter> -m pip install`) |
| Go | `runtimes.go.dir`; `plugins/go` when `runtimes.go` is absent | Create the directory | `GOBIN=<dir> go install <module>@<version>` (`@latest` without a version); a local directory with `go install ./...` in it; a local binary copied in |
| Swift | `runtimes.swift.dir`; `plugins/swift` when absent | Create the directory | Local binaries only: copied in |

   `runtimes.go` (`swift`) written without a `dir` is an error: `add` does not know where to install. The package manager's output is shown as it is.
3. Find the plugins in the package: for Node and Bun, the package name (one plugin per package); for Python, the distribution's entry points in the `rutis.plugins` group (a package can have several); for Go and Swift, the plugins in the binary's manifest.
4. Check each plugin: work out in memory the file as it will be written (the file plus the records, rows and `runtimes` of step 5), start the host's runtimes for it (as `run` and `check` do; binding no `listen` port and connecting no remote runtime: the new rows are all in local runtimes, and a host running this file keeps its ports and links, I10), and check the new rows with `check`'s own check (§8); a row that does not resolve (a Node package that is not a plugin when imported, say) is not a plugin (§11), and the file is left as it was.
5. Write the file: add each plugin's record to `installed` (keyed by row name); append a row `{ "id": <id>, "name": <row name> }` per plugin to `rows`, the row name in its existing form (§6), the id from the plugin name (the existing `id_of`), with `-2`, `-3` when taken, and no row when one with the same name exists; when `runtimes` has no entry for the language, write the settings used (I4). Keys keep their order, two-space indent, written to a temporary file and renamed (`--watch-config` never reads a half-written file).
6. Print what step 4 found of the new rows, word for word as `check` prints them (the same code): a missing service it needs, or a missing required config field (0.18), shows here. `add` still exits 0: the row is written, and the dependency or config can follow.

Which file: the second argument; else `rutis.json` in the current directory; else, without `rutis.json` but with `rutis.dev.json` (a plugin project), that one; else a new `rutis.json`.

When any of steps 2 to 4 fails: `rutis.json` is unchanged, exit 1. Packages the package manager already installed stay, as with running npm / uv directly; the error names the command that uninstalls them.

## 6. The install record and the rows

```json
{
  "format": 1,
  "runtimes": { "node": {}, "py": {}, "go": { "dir": "plugins/go" } },
  "installed": {
    "@ana/wordcount": { "from": "npm:@ana/wordcount" },
    "py:llm":         { "from": "pypi:bo-llm" },
    "go:dns":         { "from": "github.com/bo/netkit/cmd/netkit" }
  },
  "rows": [
    { "id": "wordcount", "name": "@ana/wordcount" },
    { "id": "llm", "name": "py:llm" },
    { "id": "dns", "name": "go:dns" }
  ]
}
```

- **Rows use the existing row names**: the package name for Node, `bun:<package>` for Bun, `py:<entry point>` for Python, `go:<plugin>` for Go, `swift:<plugin>` for Swift. `add` writes the row, so the host owner still need not know the prefixes; rutis-host resolves row names exactly as before, with a record or without, and a plugin has one row name only.
- The key is the plugin's row name, the same as its row's `name` (rather than the plugin name: the key is the row name, nothing to translate, and same-named plugins of two languages do not collide). `from` is the reference without its version, which `remove` / `upgrade` use to find the package (a PyPI package name differs from the plugin name). The language comes from the key's prefix (none for Node) and is not recorded. Several plugins of one package share `from`.
- A key in none of these forms is refused when the file is read: `installed.ruby:geo: name it by its row: a Node package, or bun:, py:, go: or swift: and the plugin`.
- No version is recorded: the package manager's own files keep the installed version (`package.json` and the lock file, the Python environment); a second copy goes stale after a manual `npm update`. `check` shows versions.
- Why inside `rutis.json`: one file fewer; the host owner sees what is installed and what runs in one place; deployments carry it anyway. Koishi also writes installed plugins straight into its config.
- Two records cannot share a key: when the row name is already another package's record, `add` refuses, naming that package (otherwise the second would overwrite the first, whose package could then not be removed).
- An older rutis-host reading a file with `installed` reports an unknown field by the existing rule. `format` stays 1.

## 7. `remove` and `upgrade`

- `rutis-host remove <row name, plugin name or reference> [rutis.json]`: uninstall the plugin's package (`npm uninstall`, `bun remove`, `uv pip uninstall`, deleting the binary whose manifest has it), delete all of the package's records and the rows with those row names (in groups too), printing each row removed. When a package has several plugins, the whole package goes, and it says so. `runtimes` is left alone. A name not recorded is refused, saying to uninstall with the package manager directly.
- `rutis-host upgrade [<row name, plugin name or reference>[@version]] [rutis.json]`: without a name, every record. Install the latest version (or the one given) from the recorded source; rows are not changed. A plugin the new version adds gets one line on how to add its row; for a plugin it drops, the record is deleted and each row using it gets one line, "the new version has no such plugin".
- **A running host**: `upgrade` only replaces the packages on disk; a running host keeps running the old code and the new code takes effect on restart, which `upgrade` says at the end (Koishi needs a restart after a dependency update too). Why no hot swap: reloading a row re-imports only its own entry module (`dev`'s existing rule), while an upgrade replaces the whole package.

## 8. The plugin description

The description `add` prints is what plugins already declare: TypeScript `definePlugin({ inject, provides, config })`, Python `define_plugin(...)`, Go's manifest and `rutis.Plugin`. `check` already prints it as `version`, `inject`, `provides` and `config` lines, and `add` calls the code `check` prints a row with (§5 step 6), not a second copy.

- No new manifest format: the existing declarations exist in all four languages, and `check` already uses them; a second one would be one more rutis-specific thing for plugin authors to write.
- Registry metadata (`npm view`, PyPI's JSON) is not read: the declarations of TypeScript leaf plugins, Python and Go are in code, not in package metadata.
- Seeing the description before installing (`add --dry-run`) was removed on 2026-10-28; see Out of scope.

## 9. Where the install code lives

- The install code is a private module of rutis-host (`crates/rutis-host/src/installer/`): parsing references, setting up runtimes and installing per language, finding a package's plugins, uninstalling and upgrading. `add` / `remove` / `upgrade` all call it, so there is one copy of the install logic.
- It is not a public API of `rutis-loader` (removed on 2026-10-28; see Out of scope).
- Only what exists is used: a binary's plugins are found with `rutis-loader`'s `BinaryResolver::binaries` (as the Go and Swift runtimes find binaries); the Python environment and the Node runtime are located as rutis-host already does. npm, uv and go run as external commands only when called. The `rutis` kernel is unchanged.
- rutis-host reads and writes the records (`installed`); the install code itself reads and writes no configuration file.

## 10. Other designs, and security

- **#185**: #185's `loader` service installs no packages; installing from package registries belongs to this design.
- **`dev` (#670)**: in a plugin project `add` writes `rutis.dev.json`, which `dev` reads again by the existing rules: the new row is `added` and starts, the other rows are untouched. When the runtime the new row needs is not running, the `runtimes.<language>` that `add` wrote is refused by the existing rule with a restart hint (`reload rejected: runtimes.py added: restart the host; nothing changed`). The same for `run --watch-config`.
- **#683**: which process a plugin runs in and which services it may use are #683's options on the runtime (process), unrelated to how it was installed.
- **#698**: installs go to local runtimes only.
- **Security**: `add` and `upgrade` run the package manager, which may run install scripts in the package, and `add` imports the plugin's code to check it; the same as running `npm install`, `uv pip install` or `go install` directly. rutis adds no sandbox or review; the guide says: install only plugins you trust.

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
  notes: add a row that provides it, or import it from a peer (import: ["notes"])
rutis.json: added row wordcount, runtimes.node

$ rutis-host add pypi:bo-llm
(uv's output)
installed bo-llm 0.3.0 (py) into /app/.venv: plugin llm
llm (py:llm): ok
  version: "0.3.0"
  inject: []
  provides: {}
rutis.json: added row llm, runtimes.py

$ rutis-host remove llm
uninstalled bo-llm from /app/.venv
rutis.json: removed row llm

$ rutis-host upgrade
@ana/wordcount: 1.2.0 → 1.3.0
bo-llm: 0.3.0 is the latest
a running host keeps the old versions until it restarts
```

On errors (stderr, exit 1, `rutis.json` unchanged):

| Case | Output |
| --- | --- |
| No source | `rutis-host: wordcount: say where it comes from: npm:wordcount, pypi:wordcount, a Go module path or a local path` |
| No package manager | `rutis-host: cannot run npm (No such file or directory): install Node.js 22 or later`; Python with neither uv nor `python3`: `cannot create the Python environment /app/.venv: install uv, or Python 3.10 or later`; Go keeps its existing text |
| The package manager fails | Its own output, then `rutis-host: npm install @ana/wordcount failed (exit status 1); rutis.json is unchanged` |
| Not a plugin | `rutis-host: bo-utils has no rutis.plugins entry point: it is not a plugin; it stays installed: uv pip uninstall --python /app/.venv/bin/python bo-utils`; a Node package that is not a plugin once imported gets the 0.18 text, followed by the same uninstall command |
| A local directory not recognized | `rutis-host: ./plugins/notes has no package.json, pyproject.toml or go.mod` |
| The row name is another package's | `rutis-host: py:llm is already installed from pypi:other-llm: remove it first` (when known only after installing, followed by the same uninstall command) |
| No Go directory | `rutis-host: runtimes.go has no dir to install into: add one, or remove runtimes.go to use plugins/go` |
| `remove` of a name not recorded | `rutis-host: llm was not installed by add: uninstall it with the package manager that installed it, and remove its row` |

An npm package's row name is its package name (`bun:<package>` for Bun), so a clash is reported before installing; Python entry point names and Go plugin names are known only after installing, so the clash is reported then, and as with "not a plugin" the package stays and the uninstall command is given.

## 12. Compatibility and migration

- Nothing existing breaks: a `rutis.json` without `installed` behaves exactly as before; old-prefix rows and npm-name rows resolve as they do; plugins installed by hand run as they do. Old files are not converted, get no warning, and no existing text changes.
- Not part of "Migrating to 0.19". The guide's "How a host installs plugins" leads with `add`, keeping the manual table as "without `add`".

## 13. Implementation

| Step | Content | Done when |
| --- | --- | --- |
| P1 | Withdrawn on 2026-10-28: it was the `install` feature of `rutis-loader` (the embedding API); its code moved into P2 | — |
| P2 | rutis-host: the install code (a private module, §9), the `installed` field (keyed by row name), `add` / `remove` / `upgrade`, writing the file back (with `runtimes`) | Unit tests: for each form in §4, the source, language and package manager command (fake commands on `PATH` that record their arguments); `crates/rutis-host/tests/install.rs`: rows with records resolve as usual; a key not in a row-name form is refused; write-back keeps key order; every error output of §11 compared word for word |
| P3 | End to end: `tests/e2e/tests/plugin_install.rs` (in ci.yml's e2e job, beside `dev_loop.rs`) | One scenario each for Node, Bun, Python and Go: local packages made from `rutis-host new` templates (`npm pack`, the wheel of `uv build`, a Go module directory) → `add` → `check` → `run`, the probe calls its service → `upgrade` after a version change → `remove`; the Python scenario starts with no environment; the `pypi:` and Go module path forms run once against uv's local `--find-links` and `GOPROXY=file://`; one `dev` scenario: after `add` writes `rutis.dev.json` the new row is `added` and the project's own row does not restart |
| P4 | Documentation | The [rutis-host guide](guide/rutis-host.en.md) (command table, "Quick start", "How a host installs plugins", the `installed` field), the host owner guide (#671, formerly #685), both languages in step; installation scripts and trust (§10) |

P1 → P2 → P3; P4 goes with P2. The issue's "done": the commands and the end-to-end tests of the four languages are P2 and P3; the two guides are P4.
