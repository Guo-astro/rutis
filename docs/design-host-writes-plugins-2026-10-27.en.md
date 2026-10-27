# The host writes its own plugins: how an agent in the host operates and manages plugins

[English](design-host-writes-plugins-2026-10-27.en.md) · [中文](design-host-writes-plugins-2026-10-27.md)

Status: draft, not implemented. Date: 2026-10-27. Base: `main` `103c0d22`. Scheduled: 0.19 (design only); implementation after 0.19.
Issue: [#185](https://github.com/arcships/rutis/issues/185) (including the merged [#693](https://github.com/arcships/rutis/issues/693)). Replaces [#505](https://github.com/arcships/rutis/pull/505)'s "two tools for an agent framework" draft.
Relation to 0.18: state and errors come entirely from [0.18 diagnostics](design-diagnostics-2026-10-26.en.md) (`Loader::status()`, `site` on the error graph, error codes) and [0.18 config schema](design-config-schema-2026-10-27.en.md); no new structure.
0.19 designs this depends on: [#683](https://github.com/arcships/rutis/issues/683) (`docs/design-host-plugin-limits-2026-10-27.en.md` in [PR #733](https://github.com/arcships/rutis/pull/733)), [#691](https://github.com/arcships/rutis/issues/691) (`docs/design-plugin-install-2026-10-27.en.md` in [PR #734](https://github.com/arcships/rutis/pull/734)), [#684](https://github.com/arcships/rutis/issues/684) (contract descriptions, [PR #730](https://github.com/arcships/rutis/pull/730)); S1 is driven with [#670](https://github.com/arcships/rutis/issues/670)'s `rutis-host call` ([PR #731](https://github.com/arcships/rutis/pull/731)). What they need to provide is in §11.

Out of scope: a generic call in the management service (plugins hand their abilities to the agent the Cordis way, §4.4; people trying a call use #670's `rutis-host call`); tools for one agent framework (if rutis-agent's `ToolRegistry` wants them, they are a few lines adapting this service); enable / disable a row (dsh's `set_plugin`), `upgrade`, version exemptions (§12); API documentation generation; agent-specific skills (§4.5); per-call user approval (belongs to the agent framework, §5.3); a separate trial runtime and the host running the plugin's own tests (§6.4); installing from npm or PyPI (local directories only); agent-written Go and Swift plugins (#683's named local runtimes are Node, Python and Bun only); agent-written plugins using each other's services; memory and CPU limits and a sandbox; the known gap that submodule edits do not reload (already in the [rutis-host guide](guide/rutis-host.en.md)); S1 on Windows (#670's `call` is not on Windows).

## 1. What should work, and where it is stuck

[Design philosophy](design-philosophy.en.md) §1: whatever the host needs to reach, it writes a plugin for and connects it, often with an agent writing it; it tries, changes and replaces it, and the connection becomes part of the host.

The scenario: a `rutis-host run` host runs an agent, which is itself a plugin row in the host (Python here; any language works) and provides a tool service `tools`. A user asks "what's the weather in Oslo today", and it has no tool that can answer. It should:

1. look up what the host has (services, methods, config schemas, plugin directories), write a Python plugin `weather` that registers a `weather` tool into the agent's `tools`;
2. have the host check and install it;
3. call the tool;
4. break it: learn which file and line is wrong while the old version keeps serving;
5. fix it and put the new version in place;
6. drop it: remove it, and its process exits;
7. see at any time which rows, services and errors there are.

Where it is stuck: the host has these operations, but only for the command line and the person at the terminal — `rutis-host check`, `status` (0.18), `add` / `remove` (#691), `dev`'s reload. A plugin row can reach none of them. [Session persistence and self-control tools](design-session-persist-and-self-tools-2026-08-23.en.md) §3 left "dynamic loading of new code" for later; this design is that work.

## 2. How Cordis does it

dsh (an agent product on Cordis 4) already does this. Package by package:

| dsh | What it does | This design | Why different |
| --- | --- | --- | --- |
| `dsh-agent-preset` (creator mode), `dsh-tool-cordis`'s skills and templates | The agent writes a bundle following the shipped skills and templates and installs it from a local path into the profile; no model tool creates temporary dynamic definitions (`dsh-cordis-host-runner`) | The agent writes a local directory and `add` installs it into the host config; no temporary definitions. Writing relies on `rutis-host new`'s templates and the guides (§4.5) | Same |
| `dsh-tool-cordis` | Two read-only queries: service and event API declarations, current tool schemas, running entries' config JSON Schemas, the package directory `packageDir`; queries call no service method | `describe(name)`: methods and sync/async, #684's contract types, 0.18's config schema, the project directory (§4.1) | No API documentation generation: rutis has no general type layer (design philosophy §7 principle 6); these are what it can give |
| The `tools` service | Plugins `inject` the host's `tools` and register tools; no generic call tool | The agent's row `provide`s a tool service; plugins `inject` it and register callbacks (passed by reference) (§4.4) | Same; rutis adds nothing |
| `dsh-plugin-manager` (service `pluginManager`) | The web page, the agent tool `plugin_manager` and the `dsh plugin` command line share one service: `list`, `set_plugin`, `set_bundle`, `install_bundle`, `remove_bundle`, version exemptions; manages the whole profile; writes the profile's patch file, applied at once by HMR | Management service `plugins`: `status`, `describe`, `add`, `reload`, `remove`; one implementation with the command line; written to `rutis.json`, applied at once (§4, §8) | By default only the rows the agent added (§5.2): an agent mistake cannot touch the hand-written plugins the host relies on. No `set_plugin` or exemptions: S1 needs neither |
| The `plugin_manager` tool | Enabled only in creator-mode presets; each call needs `danger-full-access` or approval of this call; the service itself checks no identity | The service is given only to the row named in the config (`admin.for`); approval is the agent framework's, and results say the change persists and affects the whole host (§5.3) | rutis has no presets or session permissions; "which row it is given to" is its "which preset enables it" |
| `dsh-plugin-manager`'s `inspect` and version check | Reads what a reference points to before installing, with `problem` codes; refuses before pnpm runs when the plugin's declared dsh peer is incompatible | `add` step 1: #691 recognizes the directory and installs the SDK at the host's version; incompatible means failure before anything is loaded (§6.1) | Same |
| `dsh-plugin-manager`'s restore | Restores `package.json` and the lockfile on failure or cancel; dependency build scripts need separate approval | Each failing step undoes what this call did (§6.1); build scripts follow #691's rules | Same |
| `dsh-config-editor` | Validates the whole candidate value before writing; on a failed apply restores the previous document and reloads the previous patch | `Loader::check` checks the candidate config before anything changes; `Loader::reload` is all or nothing (§6.2) | Same |
| `dsh-hmr` | Module replacement and config changes share one serial queue; package installs run outside it | The management service's operations and rutis-host's config reread share one queue, one thing at a time (§6.5) | Installs are inside the queue: an operation's steps must run together, and a host does not install many plugins at once |
| `dsh-host-plugin-inventory` | `pluginInventory` reads `loader.entries()` every time; a read-only snapshot, no cache | `status()` reads 0.18's `Loader::status()` every time | Same |
| (one process) | All plugins in the host process | Each plugin the agent adds has its own named runtime and process, with #683's limits (§5.4) | Only rutis, cross-language and multi-process, has this layer; a bad plugin takes no one with it, and its process exits on `remove` |

## 3. Decisions

| # | Decision | Section |
| --- | --- | --- |
| H1 | The host provides a management service by name (default `plugins`) with 5 methods: `status`, `describe`, `add`, `reload`, `remove`. The agent uses it through `inject`, which every SDK has | 4 |
| H2 | One implementation with the command line: all call `Loader::status()`, `Loader::check`, `Loader::reload`, #691's `Installer`; the layer chaining them is rutis-loader's `admin` | 4 |
| H3 | Plugins hand their abilities to the agent the Cordis way: the agent's row provides a tool service, plugins `inject` it and register callbacks; the management service has no generic call | 4.4 |
| H4 | Not provided by default; the host names the row it is given to (`admin.for`), other rows are refused with `not-allowed`; whether each call needs user approval is the agent framework's decision | 5 |
| H5 | By default the agent reloads and removes only rows it added; `manage: "all"` widens this | 5 |
| H6 | Each plugin the agent adds gets a named local runtime `agent-<plugin>` with `limits`: `call` 30 s, `stop: "process"`, `services` equal to `admin.allow` | 5 |
| H7 | Before a change takes effect, checks run in a fixed order: install (with version compatibility) → runtime → `Loader::check` → load, or swap all or nothing; a failing step stops there, undoes, and the old version keeps serving; all operations go through one queue | 6 |
| H8 | Results are plain data `{ok, text, error?, persisted}`; `error` is the root of 0.18's error graph (with `site`) | 7 |
| H9 | Written to the host config; survives a restart | 8 |
| H10 | S1's agent is a scripted Python plugin, driven by the test with `rutis-host call agent.step <n>` | 10 |

## 4. The management service

### 4.1 Methods

| Method | Arguments | What it does | Result |
| --- | --- | --- | --- |
| `status()` | none | 0.18's `Loader::status()`: each row's state and reason, services and providers, runtimes, calls in progress | The snapshot (plain data) and `text` (what `rutis-host status` prints) |
| `describe(name)` | service or row name | Read-only: a service's methods and `sync` / `async`; when the host gave #684's contract description, argument and result types; a row's config schema (0.18) and project directory | Plain data |
| `add(path)` | local directory (relative to the host config's directory) | §6.1's checks and load | `{ok, row, text, error?, persisted}` |
| `reload(row)` | row name | §6.2's checks and swap | `{ok, text, error?, persisted}` |
| `remove(row)` | row name | §6.3 | `{ok, text, persisted}` |

- Failures of plugin code are in the result, not thrown: the agent reads them and changes the code, which is the normal flow. Only crossing a permission throws `not-allowed` (§9).
- Each of the 7 steps maps to a method; there is nothing else.

### 4.2 One implementation with the existing operations

| Operation | Command line | Management service | Implementation (existing or 0.19) |
| --- | --- | --- | --- |
| State | `rutis-host status` | `status()` | `Loader::status()` (0.18) |
| Look up a service or row | the schema `rutis-host check` prints, `rutis-host types --contract` (#684) | `describe()` | `HostDispatch::methods()`, `Loader::schema_of`, #684's contract description, the directory in #691's install record |
| Check | `rutis-host check` | inside `add`, `reload` | `Loader::check(&layers)` (`crates/rutis-loader/src/loader/check.rs`), including 0.18's config validation |
| Install | `rutis-host add <path>` (#691) | `add()` | `Installer::add`; runtimes added while running with #683's `RuntimeSet`; rows written with `Loader::create` |
| Reload | `dev`'s file changes, the rutis-dev channel's `swap` | `reload()` | `Loader::reload` (`crates/rutis-loader/src/loader/api.rs`) |
| Remove | `rutis-host remove` (#691) | `remove()` | `Loader::remove`, `RuntimeSet::remove`, `Installer::remove` |

The only new thing is the layer chaining these steps in §6's order: `rutis_loader::admin::Admin` (rutis-loader's `admin` feature, which brings #691's `install`), with Rust versions of the 5 methods and a `HostDispatch` providing them by name. rutis-host and embedding hosts use the same one. rutis-host's library is not a public API, so it does not go there (as with #691's `Installer`). #670's socket gets none of these: the command line's `add` / `remove` edit the config file, and nobody needs to install into a running host from the command line.

### 4.3 How the agent uses it

The agent's row says `inject: ["plugins"]`, and uses it the way its language does (Python sketch):

```python
plugins = ctx.use("plugins")
info = await plugins.describe("notes")            # methods, types, schema, directory
result = await plugins.add("agent-plugins/weather")
if not result["ok"]:
    fix_from(result["error"])                     # site, file and line
```

The agent writes plugin files with its language's own file operations; plugin processes already run with the host user's permissions (no sandbox), so rutis adds no file-writing interface.

### 4.4 How a plugin's abilities reach the agent

As in dsh: the agent's row `provide`s a tool service (named by the agent, for example `tools`); the plugins it writes `inject` it and register their tools in `apply`, with handlers passed by reference ([plugin API](guide/plugin-api.en.md), "How values are passed": functions go by reference, and the other side calls the original); when a plugin unloads, the cleanup returned from `apply` unregisters. These are `provide` / `inject` / cleanups, which every SDK has; rutis adds nothing, and the management service needs no generic call. What the tool service looks like is the agent author's business.

The `weather` plugin (Python sketch):

```python
async def apply(ctx, config):
    async def today(city: str) -> str:
        return f"sunny in {city} (v1)"
    return await ctx.use("tools").register("weather", "Today's weather for a city", today)  # returns the unregister function
```

When calling the handler fails, the agent's code gets an ordinary error (name, message, code, `data`), as every caller does (0.18 §4.4), with no location. The errors the agent fixes in S1 (syntax errors, `apply` failures) come from `add` / `reload` results, with locations. "Hand an error received from a call to the host to read its location" would need another method, with no user now; it is in §12.

### 4.5 What the agent writes plugins from

The counterpart of dsh's skills and templates: rutis already has `rutis-host new`'s templates and the plugin author guides (#671's short path). Handing them to the agent (in its prompt, or for it to read) is the host product's business; rutis provides no agent-specific skill.

## 5. Permissions and limits

### 5.1 Who may use the management service

- **Not provided by default**: without `admin` in the config the host does not provide it, and a row that `inject`s it keeps waiting (0.18's "no provider").
- **Given to one row**: `admin.for` names a row id. The service identifies the calling row with the existing `caller_row()` (`crates/rutis-bridge/src/session/services.rs`) and refuses any other row with `not-allowed`. This is dsh's "enabled only in creator-mode presets". Calls from no row (the host's own Rust code) work as usual.
- **Why not #683's `services`**: it lists what one runtime may use and is unlimited when not written; keeping every other row away from `plugins` would need a list on every runtime, so it cannot be the default.

### 5.2 Which rows the agent may manage

| `admin.manage` | Rows it may `reload` / `remove` | For |
| --- | --- | --- |
| `"own"` (default) | Rows it added with `add`: all in the `agent-plugins` group (created by `Admin`, with no settings, only to mark them) | An agent mistake cannot touch the hand-written plugins the host relies on. Narrower than dsh (whole profile) |
| `"all"` | Every plugin row (except its own) | When the agent maintains the whole host, widened explicitly by the host owner |

`status` and `describe` are not affected by `manage`: the agent works for the host and sees all of it.

### 5.3 Per-call approval

dsh requires `danger-full-access` or approval of this call at the tool layer, and the service checks no identity. rutis does not do it either: approval belongs to the host product or agent framework. To support it, results of `add`, `reload` and `remove` carry `persisted: true`, and the service's description (in the guide) says these operations write the host config, affect the whole host, and that installed code runs with the host user's permissions; the agent layer decides from that whether to ask the user first.

### 5.4 Runtimes of plugins the agent adds

Since #683 sets limits on named runtimes, `add` gives each plugin a named local runtime of its own, `agent-<plugin>`:

| `limits` | Value | Why |
| --- | --- | --- |
| (one runtime per plugin) | yes | A plugin that loops or crashes takes no one with it; `remove` takes the runtime too and its process exits. Cost: one process per plugin |
| `call` | 30 s | The agent calling its tool or another plugin using its service fails with `timeout` past the limit; the host is not held up |
| `stop` | `"process"` | A plugin stuck in synchronous code cannot be cancelled, only ended, and the process is its own. The next `reload` starts the runtime again |
| `services` | Equal to `admin.allow`, empty by default | The plugin's code later runs with nobody watching; it gets only the services the host lists. The agent's tool service must be listed (S1: `["tools"]`). One row per runtime, so plugins cannot use each other's |
| `greeting` | not set | The agent does not start plugins under a debugger |

The host can change these with `admin.limits` (`services` still follows `allow`).

## 6. Internal checks: before a change takes effect

### 6.1 `add(path)`

| Step | Who, with what | On failure |
| --- | --- | --- |
| 1 Install | `Installer::add` (#691): language from the project file, environment prepared, local directories as a link or with `-e` (later edits need no reinstall). **Version compatibility** is here: #691 installs the runtime's SDK pinned to the host's version together with the plugin, so a plugin whose SDK floor is not met fails package-manager resolution and nothing is installed (dsh's refusal by peer before pnpm runs); Go and Swift binaries' `PLUGIN_API` is checked by the existing manifest check (`crates/rutis-loader/src/runtime/binary.rs`), and the agent writes neither | Returns #691's error; nothing changed |
| 2 Runtime | Adds `agent-weather` in #683's order: `root.plugin(LocalRuntime + RuntimeRowsPlugin)`, `RuntimeSet::add` | Undoes step 1 |
| 3 Static check | `Loader::check` (the one `rutis-host check` uses) on the candidate config with the new row `agent-weather:weather`: imports and describes it in the new runtime (syntax and import errors; the runtime handshake refuses an incompatible SDK), dry run (config schema), every dependency has a provider, no cycle, no duplicate provider, no required service blocked by `services` | Undoes steps 2 and 1; returns this row's problems (as `check` prints them) |
| 4 Load | `Loader::create` writes the row under `agent-plugins` and waits until it runs or fails (`apply` error, start-up timeout) | Deletes the row, undoes steps 2 and 1; returns the 0.18 error |
| 5 Save | §8 | — |

A new plugin has no old version, so a failure undoes everything and leaves no failed row (dsh restores `package.json` and the lockfile).

### 6.2 `reload(row)`

| Step | Who, with what | On failure |
| --- | --- | --- |
| 1 Runtime | If `stop: "process"` ended its process, start `agent-<name>` again first (as `dev` starts a local runtime whose process ended at the next change) | Returns why; the old version was not running anyway |
| 2 Static check | `Loader::check`, §6.1 step 3's checks, with the row's new code | Returns the problems; the old version keeps serving, nothing was swapped |
| 3 Swap | `Loader::reload`: all or nothing; when the new module does not resolve, its dry run fails, or other rows newly fail, the old module is restored (dsh restores the previous document and reloads the previous patch) | Returns the `cannot reload` sentence and the 0.18 error; the old version keeps serving |

- Step 2 checks two things `Loader::reload` does not: whether services the new code newly uses have a provider, and whether it makes a cycle. Without it the new version would be swapped in and wait forever for a missing service, with the old version gone.
- Known boundary: when the new code fails in `apply`, `Loader::reload` has already unloaded the old version and loads it again, so the service is briefly unavailable. Syntax and import errors are stopped at step 2.
- Only that row reloads; its runtime process is not restarted, as in `dev`.

### 6.3 `remove(row)`

In #683's order: `Loader::remove` deletes the row → the `LocalRuntime` and `RuntimeRowsPlugin` are unloaded (the process exits, `runtime agent-weather: stopped` is printed, other runtimes' pids are unchanged) → `RuntimeSet::remove` → `Installer::remove` → save. The agent's source directory is not deleted.

### 6.4 Steps removed

- **A separate trial runtime** (import and `apply` in another runtime first): `apply` opens ports, files and outside connections, and two copies applying at once conflict; import errors are already stopped at step 2 in the plugin's own runtime. dsh has no such step either.
- **The host running the plugin's own tests** (including #684's contract check): the agent is a plugin process and can run `pytest` itself; #684's contract describes the host's Rust services, whose types the agent can read from `describe`. dsh does not run plugins' tests either.

### 6.5 One thing at a time

Each `Admin` operation holds one lock from step 1 to the save; rutis-host's config reread (`--watch-config`, SIGUSR1) takes the same lock. Two operations, or an operation and a reread, never interleave (dsh-hmr's serial queue). The loader's own edits are already serial (`Loader`'s operation lock); this lock keeps an operation's steps together.

## 7. The state and errors the agent reads

- 0.18 §2 gives `site` and code locations to the person reading output and to the host, and does not hand them to callers' code by itself. The management service is the host's reading interface, given to one row by the host owner in the config; errors plugin code gets from ordinary calls still carry no `site`, unchanged.
- `error` is the root of 0.18's error graph (`name`, `message`, `code`, `data`, `site`, `stack`), taken with `rutis_bridge::thrown()`; `text` is what 0.18 prints, with locations on both sides. Agent code decides by `code` and `site`; a model reads `text`.

## 8. Written to the config

| Host | Where | After a restart |
| --- | --- | --- |
| rutis-host | `rutis.json`: the row in the `agent-plugins` group, `agent-<name>` in `runtimes.local` (with `limits`), #691's `installed` record. These are exactly the entries a reread produces, so the next reread sees no change | Starts from the config, like hand-written rows |
| Embedding host | `Admin` hands `{row, runtime entry, install record}` to a save callback, and the host stores it itself (as #691's "records belong to the host") | The host hands the records back to `Admin`, which adds the runtimes back as in §6.1 step 2 |

The host owner opens `rutis.json` and sees what the agent added, and can delete it himself.

## 9. What developers see

The host owner's config:

```json
{
  "runtimes": { "py": { "project": "." } },
  "admin": { "for": "agent", "allow": ["tools"] },
  "rows": [
    { "id": "agent", "name": "py:my_agent", "inject": ["plugins"] },
    { "id": "notes", "name": "py:notes" }
  ]
}
```

`admin`'s fields: `for` (required), `service` (default `plugins`), `manage` (`"own"` / `"all"`), `allow` (default empty), `limits` (default §5.4), `contract` (a #684 contract description file, optional). Rust embedding: `Admin::new(&root, loader, installer, runtime_set).serve_to("agent").allow(["tools"]).persist(hook)`, then `root.plugin(admin.service("plugins"))`.

What the agent gets (the `text` part; `ok`, `error` and `persisted` come with it):

| Call | Result |
| --- | --- |
| `describe("notes")` | `{service: "notes", provider: "notes", methods: {put: "async", get: "async"}, row: {config_schema: {...}, dir: "plugins/notes"}}`; `types` too when there is a contract description |
| `add("agent-plugins/weather")` | `ok: true`, `weather: running` |
| Same, the directory is not a plugin project | `ok: false`, `./agent-plugins/weather has no package.json, pyproject.toml or go.mod` (#691's text) |
| Same, the SDK version is not met | `ok: false`, the package manager's resolution failure as #691 reports it |
| Same, a syntax error (step 3) | `ok: false`, `weather: SyntaxError: invalid syntax (src/weather/__init__.py, line 3)`; `status()` has no `weather` |
| Same, `apply` fails (step 4) | `ok: false`, `weather: failed in apply: KeyError: 'units'` + `src/weather/__init__.py:12 in apply` |
| Same, required `llm` not in `allow` | `ok: false`, `weather: uses llm, which the host does not let runtime agent-weather use (allowed: tools)` (#683's text) |
| Same, a dependency nobody provides | `ok: false`, `weather: waits for geo, which no row provides` |
| `reload("weather")`, syntax error | `ok: false`, `weather: cannot reload: SyntaxError: invalid syntax (src/weather/__init__.py, line 3); the previous version keeps running` |
| Same, fixed | `ok: true`, `weather: reloaded` |
| `remove("weather")` | `ok: true`, `weather: removed; runtime agent-weather: stopped` |
| `remove("notes")` (`manage: "own"`) | throws `not-allowed`: `notes was not added by agent; the host lets agent manage only the rows it added` |
| `reload("wether")` | `ok: false`, `no row wether (rows agent added: weather)` |
| Another row calls `plugins.status()` | throws `not-allowed`: `plugins is given to agent only` |
| The agent calls the `weather` tool and it does not return in 30 s | The agent's code gets `timeout`: `a function of weather did not finish within 30s (the host's limit for runtime agent-weather); the call was cancelled; runtime agent-weather was ended` (#683's text) |

What the host owner sees in the terminal: rutis-host prints row changes as usual (`weather: running`, `runtime agent-weather: stopped`), the same text the agent gets. No text contains 0.18 §4.5's banned words. An agent-written plugin is an ordinary Python plugin, with nothing agent- or rutis-specific in it.

## 10. Scenario S1 (updated for this design)

Replaces the scenario and assertions in #185's body (the issue itself is not edited).

A scripted Python plugin stands in for the agent: it provides `tools` (a small service recording registered tools and calling them by name) and `agent`, whose `step(n)` does step n of a fixed script and returns the result. This is simpler than wiring a model and takes the same path; the agent's reasoning is not what this scenario tests. The test starts `rutis-host run --allow-call` (#670) and drives each step with `rutis-host call agent.step <n>`. The config is §9's, plus an `intruder` row that also `inject`s `plugins`.

| Step | What `agent.step(n)` does | Assertions |
| --- | --- | --- |
| 1 | `describe("notes")`; writes `agent-plugins/weather/pyproject.toml` and `src/weather/__init__.py` with Python (registers the tool `weather`, returning `"sunny (v1)"`) | `describe` has `notes`'s methods and directory; the files exist; no `weather` in the tool list |
| 2 | `add("agent-plugins/weather")` | `ok`; `rutis.json` gained the row in the `agent-plugins` group, `agent-weather` in `runtimes.local`, and an `installed` record; in `status`, `agent-weather` has the single row `weather`; its pid is recorded; `weather` is in the tool list |
| 3 | Calls the `weather` tool with `"Oslo"` | `"sunny in Oslo (v1)"` |
| 4 | Writes a syntax error, `reload("weather")`, then calls the tool | `ok` is false, `text` starts with `weather: cannot reload: SyntaxError` and contains `src/weather/__init__.py`; the tool still returns v1 |
| 5 | Changes it to return `"rainy … (v2)"`, `reload`, then calls the tool | `ok`; one `weather` in the tool list; returns v2 |
| 6 | `remove("weather")`, `status()` | `ok`; no `weather` in the tool list; no `agent-weather`; the recorded process exits within 5 s; other runtimes' pids unchanged; `rutis.json` is as it was at step 1 |
| 7 | `remove("notes")` | Throws `not-allowed`; `notes` keeps running |

Also:

- `rutis-host call intruder.try_status` (which calls `plugins.status()`) gets `not-allowed`.
- No interruption: during step 4 the test calls `rutis-host call agent.use_tool '"weather"' '"Oslo"'` every 100 ms and no call fails; every call after step 5's `reload` returns gives v2.
- No result contains 0.18 §4.5's banned words; after the host stops, `tests/e2e`'s residue checks pass.
- Location: `tests/e2e/tests/host_writes_plugin.rs`, with the scripted agent plugin in the scenario directory. The E2E framework #175 is done (closed). Linux and macOS, in nightly.

## 11. Implementation

After 0.19:

| Step | Content | Can start when | Done when |
| --- | --- | --- | --- |
| 1 | `Admin`'s `status` and `describe`, providing it by name, the `admin.for` check | 0.18's #677, #678; #684's contract description format | Unit tests: the snapshot, `describe`'s four items, other rows refused |
| 2 | `add`, `reload`, `remove` with §6's order, the queue, the save callback | #691's `Installer`; #683's `RuntimeSet` and `Limits` | Every row of §9 has a test asserting the text exactly; after each failing step there is no extra runtime, installation or row; both `manage` values; two concurrent `add`s finish one after the other |
| 3 | rutis-host reads `admin`, writes `rutis.json`, its config reread takes the same lock, and it works after a restart | Step 2; #683's `runtimes.local` | A reread after writing is no change; after a restart the agent's plugins run as before |
| 4 | S1 (§10); a section "letting an agent in the host manage plugins" in rutis-host.md and rust-host.md (both languages), with §5.3's approval advice | 1–3; #670's `call` | Passes on Linux and macOS |

This covers #185's "Done": the two design documents (this one), and S1 updated per §10.

Needed from #683:

1. rutis-loader's `RuntimeSet`: adding and removing named local runtimes while running without restarting others, with the order documented (already in PR #733);
2. `LocalRuntime::limits` with `call`, `stop`, `services`; `timeout` and `not-allowed` carrying 0.18's `site`;
3. After `stop: "process"` ends a process, a Rust embedding can start that runtime again;
4. `Loader::check` reports "a required service is blocked by `services`" (already in PR #733 §4.4);
5. rutis-host builds its loader with a `RuntimeSet`, and a `runtimes.local` entry written into `rutis.json` while running is treated by the next reread as an existing runtime, not restarted.

Needed from #691: `Installer::add` takes a local directory and returns the language and row name; local directories are installed as a link or with `-e`; the SDK is installed pinned to the host's version, so a plugin whose SDK floor is not met fails before loading; records are serializable, have the same shape as rutis-host's `installed` field, and include the project directory; `Installer::remove` removes by record; the API reads and writes no config.

Needed from #684: a contract description from which one service's methods and argument and result types can be read by service name (for `describe`).

Needed from #670: `run --allow-call` and `call`, only for driving S1; the management service no longer depends on it.

## 12. Self-review: is it too heavy?

| Item | User | Conclusion |
| --- | --- | --- |
| 5 methods | S1's 7 steps | Kept |
| `describe` | Step 1: the agent needs to know what the host has and where to read source (dsh-tool-cordis) | Kept, read-only |
| A generic `call` | Plugins register abilities with the agent the Cordis way; people use `rutis-host call` | Removed |
| A method reading the location of an error the agent got from a call | Finding where a tool call failed; S1's errors all come from `add` / `reload` results | Removed until there is a user |
| Enable / disable a row (dsh's `set_plugin`) | Not in S1; `remove` already stops it running | Removed |
| `upgrade`, version exemptions | For local directories `reload` after an edit is enough; the agent installs nobody else's packages | Removed |
| A separate `check` method | Checks run inside `add` and `reload` | Removed |
| Trial runtime, host-run tests | See §6.4; dsh has neither | Removed |
| `Loader::check` before `reload` | Stops a new version waiting for a missing service or making a cycle | Kept |
| One lock | Operations and rereads never interleave (dsh-hmr) | Kept |
| `manage: "all"` | An agent maintaining the whole host | Kept, off by default |
| One runtime per plugin | Isolation, process exits on `remove` | Kept |
| rutis-agent tool adapter, agent-specific skills, per-call approval | Belong to the agent framework and host product | Removed, out of scope |
