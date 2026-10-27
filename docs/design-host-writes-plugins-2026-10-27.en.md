# The host writes its own plugins: how an agent in the host operates and manages plugins

[English](design-host-writes-plugins-2026-10-27.en.md) · [中文](design-host-writes-plugins-2026-10-27.md)

Status: draft, not implemented. Date: 2026-10-27. Base: `main` `103c0d22`. Scheduled: 0.19 (design only); implementation after 0.19.
Issue: [#185](https://github.com/arcships/rutis/issues/185) (including the merged [#693](https://github.com/arcships/rutis/issues/693)). Replaces [#505](https://github.com/arcships/rutis/pull/505)'s "two tools for an agent framework" draft.
Relation to 0.18: state and errors come entirely from [0.18 diagnostics](design-diagnostics-2026-10-26.en.md) (`Loader::status()`, `site` on the error graph, error codes); no new structure.
0.19 designs this depends on: [#683](https://github.com/arcships/rutis/issues/683) (keeping plugins in check, `docs/design-host-plugin-limits-2026-10-27.en.md` in [PR #733](https://github.com/arcships/rutis/pull/733)), [#691](https://github.com/arcships/rutis/issues/691) (plugin installation, `docs/design-plugin-install-2026-10-27.en.md` in [PR #734](https://github.com/arcships/rutis/pull/734)), [#670](https://github.com/arcships/rutis/issues/670) (`rutis-host call`, `docs/design-try-and-test-plugins-2026-10-27.en.md` in [PR #731](https://github.com/arcships/rutis/pull/731)). What they need to provide is listed in §10.

Out of scope: tools for one particular agent framework (if rutis-agent's `ToolRegistry` wants them, they are a few lines adapting this service, not in this design); one tool per service; enable / disable a row, `upgrade`, and a separate `check` method in the management service (§11); a separate trial runtime, and the host running the plugin's own tests (§5); installing from npm or PyPI (local directories only); agent-written Go and Swift plugins (#683's named local runtimes are Node, Python and Bun only); agent-written plugins using each other's services; memory and CPU limits and a sandbox; services inside instances; object or function references as `call` arguments or results; the known gap that submodule edits do not reload (already in the [rutis-host guide](guide/rutis-host.en.md)); S1 on Windows (#670's `call` is not on Windows).

## 1. What should work, and where it is stuck

[Design philosophy](design-philosophy.en.md) §1: whatever the host needs to reach, it writes a plugin for and connects it, often with an agent writing it; it tries, changes and replaces it, and the connection becomes part of the host.

The scenario: a `rutis-host run` host runs an agent. The agent is itself a plugin row in the host (written in Python here; any language works). A user asks "what's the weather in Oslo today", and no service can answer. The agent should:

1. write a Python plugin `weather` (with Python's own file operations, into a directory);
2. have the host check and install it;
3. call `weather.today("Oslo")`;
4. break it: learn which file and line is wrong while the old version keeps serving;
5. fix it and put the new version in place;
6. drop it: remove it, and its process exits;
7. see at any time which rows, services and errors there are.

Where it is stuck: the host has these operations, but only for the command line and the person at the terminal — `rutis-host check`, `status` (0.18), `call` (#670), `add` / `remove` (#691), `dev`'s reload. A plugin row can reach none of them. [Session persistence and self-control tools](design-session-persist-and-self-tools-2026-08-23.en.md) §3 left "dynamic loading of new code" for later; this design is that work, done by handing the host's existing operations to a plugin row, not by having the agent load code into itself.

## 2. Decisions

| # | Decision | Section |
| --- | --- | --- |
| H1 | The host provides a management service by name (default `plugins`) with 5 methods: `status`, `call`, `add`, `reload`, `remove`. The agent uses it through `inject`, which every SDK has, whatever its language or agent framework | 3 |
| H2 | These methods, the command line and #670's socket are one implementation: all call `Loader::status()`, `call_service`, #691's `Installer`, `Loader::check`, `Loader::reload`; the layer combining them lives in rutis-loader (`admin`), shared by rutis-host and embedding hosts | 3 |
| H3 | Not provided by default. The host names the row it is given to in its config (`admin.for`); calls from other rows are refused with `not-allowed` | 4 |
| H4 | By default the agent reloads and removes only rows it added itself (kept in the `agent-plugins` group); the host can widen this with `manage: "all"` | 4 |
| H5 | `call` reaches services provided by plugin rows by default; the host's own services and peers' services must be listed in `allow` | 4 |
| H6 | Each plugin the agent adds gets a named local runtime `agent-<plugin>` with default `limits`: `call` 30 s, `stop: "process"`, `services` equal to `allow` (empty by default) | 4 |
| H7 | Before `add` or `reload` takes effect, checks run in a fixed order: install → runtime → `Loader::check` → load, or swap all or nothing; a failing step stops there, undoes what this call did, and the old version keeps serving | 5 |
| H8 | Results are plain data: `{ok, text, error?}`; `error` is the root of 0.18's error graph (with `site` and locations), `text` is what rutis prints for people | 6 |
| H9 | What is added is written to the host's config and survives a restart: rutis-host writes `rutis.json` (row, `runtimes.local`, `installed`); embedding hosts store it through a save callback | 7 |
| H10 | S1's agent is a scripted Python plugin, driven step by step by the test with `rutis-host call agent.step <n>`; no model | 9 |

## 3. The management service

### 3.1 Methods

| Method | Arguments | What it does | Result |
| --- | --- | --- | --- |
| `status()` | none | 0.18's `Loader::status()`: each row's state and reason, each service's provider and methods (`{method: "sync" \| "async"}`), runtimes, calls in progress | The snapshot (plain data) plus `text` (what `rutis-host status` prints) |
| `call(service, method, args)` | service, method, argument list | Calls by name as "host", like `rutis-host call` | `{ok: true, value}` or `{ok: false, text, error}` |
| `add(path)` | local directory (relative to the host config's directory) | §5.1's checks and load | `{ok, row, text, error?}` |
| `reload(row)` | row name | §5.2's checks and swap | `{ok, text, error?}` |
| `remove(row)` | row name | §5.3 | `{ok, text}` |

- Failures of plugin code (import fails, `apply` fails, a method throws, a timeout) are **not thrown**; they are in the result: the agent reads them and changes the code, which is the normal flow. Only crossing a permission throws (not the row it was given to, a row it may not manage), with #683's code `not-allowed`, see §8; a misspelled row name is also a result.
- The method set holds only what the scenario uses: each of the 7 steps maps to a method.

### 3.2 One implementation with the existing operations

| Operation | Command line | #670's socket | Management service | Implementation (existing or 0.19) |
| --- | --- | --- | --- | --- |
| State | `rutis-host status` | `status` | `status()` | `Loader::status()` (0.18) |
| Call | `rutis-host call` | `call` | `call()` | `rutis_bridge::session::call_service` |
| Check | `rutis-host check` | — | inside `add`, `reload` | `Loader::check(&layers)` (`crates/rutis-loader/src/loader/check.rs`), including 0.18's config-schema validation |
| Install | `rutis-host add <path>` (#691) | — | `add()` | #691's `Installer::add`; runtimes added while running with #683's `RuntimeSet`; rows written with `Loader::create` |
| Reload | `dev`'s file changes, the rutis-dev channel's `swap` | — | `reload()` | `Loader::reload` (`crates/rutis-loader/src/loader/api.rs`) |
| Remove | `rutis-host remove` (#691) | — | `remove()` | `Loader::remove`, `RuntimeSet::remove`, `Installer::remove` |

- The only new thing is the layer chaining these steps in §5's order: `rutis_loader::admin::Admin` (rutis-loader's `admin` feature, which brings #691's `install`). It has Rust versions of the 5 methods plus a `HostDispatch` that provides them by name. rutis-host uses it; embedding hosts use the same one. rutis-host's library is not a public API, so it does not go there (the same reason as #691's `Installer`).
- The socket gets no `add` / `reload` / `remove`: the command line's `add` / `remove` edit the config file (#691); nobody needs to install into a running host from the command line.
- `rutis-host call` and `call()` produce the same text: both are written by 0.18's error-printing code.

### 3.3 How the agent uses it

The agent's row says `inject: ["plugins"]`, and its code uses the service the way its language does, with nothing rutis-specific (Python sketch):

```python
plugins = ctx.use("plugins")
result = await plugins.add("agent-plugins/weather")
if not result["ok"]:
    fix_from(result["error"])        # error carries site, file and line
```

The agent writes plugin files with its language's own file operations; plugin processes already run with the host user's permissions (no sandbox, out of scope in #683), so rutis adds no file-writing interface.

## 4. Permissions and limits

### 4.1 Who may use the management service

- **Not provided by default**: without `admin` in the config the host does not provide it, and a row that `inject`s it gets 0.18's "no provider".
- **Given to one row**: `admin.for` names a row id. The service identifies the calling row with the existing `caller_row()` (`crates/rutis-bridge/src/session/services.rs`); any other row is refused with `not-allowed`, the text saying whom it is given to. Calls from no row (the host's own Rust code) work as usual.
- **Why not #683's `services`**: it lists what one runtime may use, and is unlimited when not written; making "no other row can use `plugins`" would need a list on every runtime, so it cannot be the default. Recognizing the calling row is one check, with the existing `caller_row()`, and no new permission system.

### 4.2 Which rows the agent may manage

| `admin.manage` | Rows it may `reload` / `remove` | For |
| --- | --- | --- |
| `"own"` (default) | The rows it added with `add`: all in the `agent-plugins` group (created by `Admin`, with no settings, only to mark these rows) | A mistake by the agent cannot touch the hand-written plugins the host owner relies on |
| `"all"` | Every plugin row (except its own) | When the agent maintains the whole host, and the host owner widens it explicitly |

`status` and `call` are not affected by `manage`: the agent works for the host and sees all of it.

### 4.3 Which services `call` reaches

By default services provided by plugin rows (provider is a row name in `Loader::status()`), which is what the host installs plugins for. The host's own services and peers' services must be listed in `admin.allow`; others are refused with `not-allowed`. `call` calls as "host", and the callee's limits (#683) apply as usual.

### 4.4 Runtimes of plugins the agent adds

Since #683 sets limits on named runtimes, `add` gives each plugin a named local runtime of its own, `agent-<plugin>`:

| `limits` | Value | Why |
| --- | --- | --- |
| (one runtime per plugin) | yes | A plugin that loops or crashes takes no one with it; `remove` takes the runtime too and its process exits. Cost: one process per plugin |
| `call` | 30 s | Calls come in not only from the agent; a timeout fails with `timeout` and the host is not held up |
| `stop` | `"process"` | A plugin stuck in synchronous code cannot be cancelled, only ended, and the process is its own. The next `reload` starts the runtime again |
| `services` | Equal to `admin.allow`, empty by default | The plugin's code later runs with nobody watching; by default it gets no service from outside its runtime. One row per runtime, so plugins cannot use each other's either |
| `greeting` | not set | The agent does not start plugins under a debugger |

The host can change these with `admin.limits` (`services` still follows `allow`).

## 5. Internal checks: before a change takes effect

### 5.1 `add(path)`

| Step | Who, with what | On failure |
| --- | --- | --- |
| 1 Install | `Installer::add` (#691): language from the project file, environment prepared; local directories as a link or with `-e`, so later edits need no reinstall. Yields the language and the row name under the language runtime (`py:weather`); `Admin` replaces the prefix with the plugin's own named runtime, `agent-weather:weather` (otherwise #691's rules would enable the language runtime `runtimes.py`) | Returns #691's error (`./agent-plugins/weather has no package.json, pyproject.toml or go.mod`); nothing changed |
| 2 Runtime | Adds `agent-weather` in #683's order: `root.plugin(LocalRuntime + RuntimeRowsPlugin)`, `RuntimeSet::add` | Undoes step 1; returns why |
| 3 Static check | `Loader::check` (the one `rutis-host check` uses) with the new row `agent-weather:weather` added to the current config: imports and describes it in the new runtime (syntax and import errors), dry run (config schema), every dependency has a provider, no dependency cycle, no duplicate provider, no required service blocked by `services` | Undoes steps 2 and 1; returns this row's problems from the report (as `check` prints them) |
| 4 Load | `Loader::create` writes the row under `agent-plugins` and waits until it runs or fails (`apply` error, start-up timeout) | Deletes the row, undoes steps 2 and 1; returns the 0.18 error |
| 5 Save | §7 | — |

A new plugin has no old version, so a failure undoes everything and leaves no failed row behind.

### 5.2 `reload(row)`

| Step | Who, with what | On failure |
| --- | --- | --- |
| 1 Runtime | If `stop: "process"` ended its process, start `agent-<name>` again first (as `dev` starts a local runtime whose process ended at the next change) | Returns why the runtime does not start; the old version was not running anyway |
| 2 Static check | `Loader::check`: imports and describes the row's new code again, the checks of §5.1 step 3 | Returns the problems; the old version keeps serving, nothing was swapped |
| 3 Swap | `Loader::reload`: all or nothing. When the new module does not resolve, its dry run fails, or other rows newly fail, the old module is restored | Returns the `cannot reload` sentence and the 0.18 error; the old version keeps serving |

- Step 2 looks like a repeat of `Loader::reload`'s own resolve and dry run, but it checks two things `reload` does not: whether services the new code newly uses have a provider, and whether it makes a cycle. Without it the new version would be swapped in and wait forever for a missing service, with the old version gone.
- Known boundary: when the new code fails in `apply`, `Loader::reload` has already unloaded the old version and loads it again; the service is briefly unavailable in between. Syntax and import errors are stopped at step 2 and never get here.
- Only that row reloads; its runtime process is not restarted, as in `dev`.

### 5.3 `remove(row)`

In #683's order: `Loader::remove` deletes the row → the `LocalRuntime` and `RuntimeRowsPlugin` are unloaded (the process exits, `runtime agent-weather: stopped` is printed, other runtimes' pids are unchanged) → `RuntimeSet::remove` → `Installer::remove` → save. The agent's source directory is not deleted; those are the agent's own files.

### 5.4 Steps removed

- **A separate trial runtime** (import and `apply` the new code in another runtime first, then swap): not done. `apply` opens ports, files and outside connections, and two copies applying at once conflict; import errors are already stopped at step 2 in the plugin's own runtime; it would only catch "`apply` fails", at the cost of another process and a real `apply` every time.
- **The host running the plugin's own tests** (including #684's contract check): the agent is a plugin process and can run its language's test command itself (`pytest`); #684's contract describes the host's Rust services, which S1's plugin does not use. Nobody needs the host to run them.

## 6. The state and errors the agent reads

- 0.18 §2 gives `site` and code locations to the person reading output and to the host, and does not hand them to callers' code by itself. The management service is the host's reading interface, given to one row by the host owner in the config, a decision of his like opening #670's socket; errors plugin code gets from ordinary calls still carry no `site`, unchanged.
- `error` is the root of 0.18's error graph (`name`, `message`, `code`, `data`, `site`, `stack`), taken with `rutis_bridge::thrown()`; `text` is what 0.18 prints, with locations on both sides. Agent code decides by `code` and `site`; a model reads `text`.

## 7. Written to the config

| Host | Where | After a restart |
| --- | --- | --- |
| rutis-host | `rutis.json`: the row in the `agent-plugins` group, `agent-<name>` in `runtimes.local` (with `limits`), #691's `installed` record. These are exactly the entries rutis-host gets when it reads its config again, so its next reread (`--watch-config`, SIGUSR1) sees no change | Starts from the config, like hand-written rows |
| Embedding host | `Admin` hands `{row, runtime entry, install record}` to a save callback the host gives it; the host stores it where it likes (as #691's "records belong to the host") | The host hands the records back to `Admin`, which adds the runtimes back in §5.1 step 2's order |

The host owner opens `rutis.json` and sees what the agent added, and can delete it himself.

## 8. What developers see

The host owner's config:

```json
{
  "runtimes": { "py": { "project": "." } },
  "admin": { "for": "agent", "allow": [] },
  "rows": [
    { "id": "agent", "name": "py:my_agent", "inject": ["plugins"] },
    { "id": "notes", "name": "py:notes" }
  ]
}
```

`admin`'s fields: `for` (required), `service` (default `plugins`), `manage` (`"own"` / `"all"`), `allow` (default empty), `limits` (default §4.4). Rust embedding: `Admin::new(&root, loader, installer, runtime_set).serve_to("agent").allow([..]).persist(hook)`, then `root.plugin(admin.service("plugins"))`.

What the agent gets (the `text` part; `ok` and `error` come with it):

| Call | Result |
| --- | --- |
| `add("agent-plugins/weather")` | `ok: true`, `weather: running; provides weather` |
| Same, a syntax error (step 3) | `ok: false`, `weather: SyntaxError: invalid syntax (src/weather/__init__.py, line 3)`; `status()` has no `weather` |
| Same, `apply` fails (step 4) | `ok: false`, `weather: failed in apply: KeyError: 'units'` + `src/weather/__init__.py:12 in apply` |
| Same, required `llm` not in `allow` | `ok: false`, `weather: uses llm, which the host does not let runtime agent-weather use (allowed: )` (#683's text) |
| Same, a dependency nobody provides | `ok: false`, `weather: waits for geo, which no row provides` |
| `call("weather", "today", ["Oslo"])` | `ok: true`, `value: "sunny (v1)"` |
| Same, the method throws | `ok: false`, `call to weather.today failed: KeyError: 'region'` + `weather at src/weather/__init__.py:9 in today` |
| Same, no answer in 30 s | `ok: false`, `call to weather.today did not finish within 30s (the host's limit for runtime agent-weather); the call was cancelled; runtime agent-weather was ended` (`timeout`) |
| Same, a host service not in `allow` | `ok: false`, `llm is not open to the agent` (`not-allowed`) |
| `reload("weather")`, syntax error | `ok: false`, `weather: cannot reload: SyntaxError: invalid syntax (src/weather/__init__.py, line 3); the previous version keeps running` |
| Same, fixed | `ok: true`, `weather: reloaded` |
| `remove("weather")` | `ok: true`, `weather: removed; runtime agent-weather: stopped` |
| `remove("notes")` (`manage: "own"`) | throws `not-allowed`: `notes was not added by agent; the host lets agent manage only the rows it added` |
| `reload("wether")` | `ok: false`, `no row wether (rows agent added: weather)` |
| Another row calls `plugins.status()` | throws `not-allowed`: `plugins is given to agent only` |

What the host owner sees in the terminal: rutis-host prints row changes as usual (`weather: running`, `runtime agent-weather: stopped`), the same text the agent gets. No text contains 0.18 §4.5's banned words.

An agent-written plugin is an ordinary Python plugin, with nothing agent- or rutis-specific in it.

## 9. Scenario S1 (updated for this design)

Replaces the scenario and assertions in #185's body (the issue itself is not edited).

A scripted Python plugin stands in for the agent: it provides the service `agent` whose method `step(n)` does step n of a fixed script and returns what it got. This is simpler than wiring a model and takes the same path (a plugin row `inject`ing the management service); the agent's reasoning is not what this scenario tests. The test starts `rutis-host run --allow-call` (#670) and drives each step with `rutis-host call agent.step <n>`. The config is §8's, plus the hand-written `notes` row and an `intruder` row (which also `inject`s `plugins`).

| Step | What `agent.step(n)` does | Assertions |
| --- | --- | --- |
| 1 | Writes `agent-plugins/weather/pyproject.toml` and `src/weather/__init__.py` with Python (`today` returns `"sunny (v1)"`); `status()` | The files exist; no `weather` among the services |
| 2 | `add("agent-plugins/weather")` | `ok`; `rutis.json` gained the row in the `agent-plugins` group, `agent-weather` in `runtimes.local`, and an `installed` record; in `status`, `agent-weather` has the single row `weather`; its pid is recorded |
| 3 | `call("weather", "today", ["Oslo"])` | `"sunny (v1)"` |
| 4 | Writes a syntax error, `reload("weather")`, then `call` | `reload`'s `ok` is false, `text` starts with `weather: cannot reload: SyntaxError` and contains `src/weather/__init__.py`; `call` still gives `"sunny (v1)"` |
| 5 | Changes it to return `"rainy (v2)"`, `reload`, then `call` | `ok`; `"rainy (v2)"` |
| 6 | `remove("weather")`, `status()` | `ok`; no `weather` and no `agent-weather`; the recorded process exits within 5 s; other runtimes' pids unchanged; `rutis.json` is as it was at step 1 |
| 7 | `remove("notes")` | Throws `not-allowed`; `notes` keeps running |

Also:

- `rutis-host call intruder.try_status` (which calls `plugins.status()`) gets `not-allowed`.
- No interruption: from step 4 until step 5's `reload` returns, the test calls `rutis-host call weather.today '"Oslo"'` every 100 ms and no call fails; after step 5 every call gives `"rainy (v2)"`.
- No result contains 0.18 §4.5's banned words.
- After the host stops, `tests/e2e`'s residue checks pass.
- Location: `tests/e2e/tests/host_writes_plugin.rs`, with the scripted agent plugin in the scenario directory. The E2E framework #175 is done (closed). Linux and macOS (`call` is not on Windows), in nightly.

## 10. Implementation

After 0.19:

| Step | Content | Can start when | Done when |
| --- | --- | --- | --- |
| 1 | `Admin`'s `status` and `call`, providing it by name, the `admin.for` check, `allow` | 0.18's #677, #678; #670 | Unit tests: the snapshot and methods; `call` succeeding and four failures; other rows refused |
| 2 | `add`, `reload`, `remove` with §5's order; the save callback | #691's `Installer`; #683's `RuntimeSet` and `Limits` | Every row of §8 has a test asserting the text exactly; after each failing step nothing is left behind (no extra runtime, installation or row); both `manage` values |
| 3 | rutis-host reads `admin`, writes `rutis.json`, and works after a restart | Step 2; #683's `runtimes.local` | A reread after writing is no change; after a restart the agent's plugins run as before |
| 4 | S1 (§9); a section "letting an agent in the host manage plugins" in rutis-host.md and rust-host.md (both languages) | 1–3 | Passes on Linux and macOS |

This covers #185's "Done": the two design documents (this one), and S1 updated per §9.

Needed from #683:

1. rutis-loader's `RuntimeSet`: adding and removing named local runtimes while running without restarting others, with the order documented (already in PR #733);
2. `LocalRuntime::limits` with `call`, `stop`, `services`; the `timeout` and `not-allowed` codes carrying 0.18's `site`;
3. After `stop: "process"` ends a process, a Rust embedding can start that runtime again;
4. `Loader::check` reports "a required service is blocked by `services`" (PR #733 §4.4 already has `check` do the same check);
5. rutis-host builds its loader with a `RuntimeSet`, and a `runtimes.local` entry written into `rutis.json` while running is treated by the next reread as an existing runtime, not restarted.

Needed from #691: `Installer::add` takes a local directory and returns the language and row name; local directories are installed as a link or with `-e`; records are serializable, and rutis-host's `installed` field has the same shape as `Installer`'s record; `Installer::remove` removes by record; the API reads and writes no config.

Needed from #670: `run --allow-call` and the `call` request (S1 drives with them); `call`'s results and error text written by 0.18's printing code, which the management service's `call()` shares. Nothing new is asked of it.

## 11. Self-review: is it too heavy?

| Item | User | Conclusion |
| --- | --- | --- |
| The service's 5 methods | S1's 7 steps | Kept |
| Enable / disable a row | Not in S1; `remove` already stops it running | Removed |
| `upgrade` | For local directories, `reload` after an edit is enough | Removed |
| A separate `check` method | Checks run inside `add` and `reload`; the agent has no "check without installing" step | Removed |
| `add` / `reload` / `remove` over the socket | The command line edits the config file, not a running host | Removed |
| Trial runtime, host-run tests | See §5.4 | Removed |
| `Loader::check` before `reload` | Stops a new version waiting for a missing service or making a cycle | Kept |
| `manage: "all"` | An agent maintaining the whole host | Kept, off by default |
| One runtime per plugin | Isolation, process exits on `remove` | Kept |
| Tools for rutis-agent | A thin adapter over this service | Removed, out of scope |
| A model driving S1 | The test is of the management service, not of reasoning | Removed, scripted plugin instead |
