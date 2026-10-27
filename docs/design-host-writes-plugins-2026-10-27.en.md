# The host writes its own plugins: the agent's tools, isolation, errors and iteration

[English](design-host-writes-plugins-2026-10-27.en.md) · [中文](design-host-writes-plugins-2026-10-27.md)

Status: draft, not implemented. Date: 2026-10-27. Base: `main` `103c0d22`. Scheduled: 0.19 (design only); implementation after 0.19.
Issue: [#185](https://github.com/arcships/rutis/issues/185) (including the merged [#693](https://github.com/arcships/rutis/issues/693)). Keeps and updates the tool draft of [#505](https://github.com/arcships/rutis/pull/505) (before 0.15, closed).
Relation to 0.18: errors and state come entirely from [0.18 diagnostics](design-diagnostics-2026-10-26.en.md) (`site` on the error graph, error codes, `Loader::status()`); no new structure. The agent is part of the host, so under that design's §2 it is on the side of "the person reading the output and the host".
0.19 work this depends on: [#683](https://github.com/arcships/rutis/issues/683) (keeping plugins in check; design in [PR #733](https://github.com/arcships/rutis/pull/733), `docs/design-host-plugin-limits-2026-10-27.en.md`) and [#691](https://github.com/arcships/rutis/issues/691) (plugin installation and its embedding API; design in [PR #734](https://github.com/arcships/rutis/pull/734), `docs/design-plugin-install-2026-10-27.en.md`). Every item needed from them is listed in §10.

Out of scope: the agent in rutis-dsh (§3.4); one tool per service method (§3.2); the agent installing plugins from npm, PyPI or other registries; agent-written plugins using each other's services by default; memory and CPU limits and a file or network sandbox (§5); agent-written Go and Swift plugins (one binary is one runtime there, and #683's named local runtimes are Node, Python and Bun only); services inside instances, and object references as `rutis_call` arguments or results; the known gap that a reload does not re-import the submodules a plugin imports (already in the [rutis-host guide](guide/rutis-host.en.md)); reloading on file changes in embedding hosts; a "try, then discard, never written to the config" mode; #505's `Loader::services()`, scoped registration and removal in `ToolRegistry`, and limits on references and in-flight calls.

## 1. What should work, and where it is stuck

The main line of the [design philosophy](design-philosophy.en.md) §1: whatever the host needs to reach, it writes a plugin for and connects it, often with an agent writing it; it tries it, changes it, replaces it, and the connection becomes part of the host.

The concrete scenario: an agent runs in a host. A user asks "what's the weather in Oslo today", and the agent has no tool that can answer. It should be able to:

1. write a Python plugin with its own file tool, providing the service `weather`;
2. install it into the host;
3. call `weather.today("Oslo")` to answer;
4. break it: learn which file and line is wrong while the old version keeps serving;
5. fix it and change its behaviour: put the new version in place;
6. drop it: remove it, and its process exits cleanly.

Where it is stuck today:

- **The agent cannot reach plugin services.** rutis-agent's tools come from `ToolRegistry` (`crates/rutis-agent/src/tools/mod.rs`), which holds hand-written tools such as `bash` and `replace_text`, not rutis services. [Session persistence and self-control tools](design-session-persist-and-self-tools-2026-08-23.en.md) §3 says "no dynamic loading of new code" and leaves it for later; this design is that later work. The agent does not load code into itself: it adds plugin rows to the host, whose lifecycle rutis already manages.
- **The agent cannot install, replace or remove plugins.** Embedding hosts have no "install a local directory" API (#691 adds it); `rutis-host dev` reloads, but follows one project directory.
- **Agent-written plugins are not isolated.** They share one Python process with other plugins; an infinite loop makes the plugins in that process wait with it.
- **The agent cannot read errors.** In 0.17 errors are strings by the time they reach the host; 0.18 keeps their structure, but nothing hands it to the agent yet.

## 2. Decisions

| # | Decision | Section |
| --- | --- | --- |
| H1 | The host gives the agent three fixed tools: `rutis_services` (list callable services) and `rutis_call` (call by name), kept from #505; plus `rutis_plugin` (add, reload, remove, status) | 3, 4 |
| H2 | No tool per service: the tool schemas are fixed, the service list is data | 3.2 |
| H3 | By default the agent can call services provided by plugin rows; the host's own services and peers' services must be listed by the host in `allow` | 3.3 |
| H4 | An agent-written plugin is a local directory, installed with #691's embedding API (`Installer`); `RutisTools` adds a named local runtime of its own (`agent-<plugin>`) and writes the row under a group in the host's editable config (the "agent group"), so it is still there after a restart | 4 |
| H5 | The agent group only marks which rows the agent installed: `rutis_plugin` reloads and removes only those; reload is the existing `Loader::reload` (all or nothing, the old version keeps serving) and reloads only that row; remove takes the plugin's runtime with it, and its process ends | 4 |
| H6 | Isolation uses only #683's options, set on each agent plugin's own runtime: one process per plugin; `limits` of a 30 s call limit, ending the process on timeout (`stop: "process"`), and `services` equal to `allow` (empty by default); the greeting limit at its default. No sandbox | 5 |
| H7 | An error in a tool result is the text rutis prints for people (with `site` and both locations); the agent gets it through the host's reading interfaces, plugin code does not | 6 |
| H8 | The implementation is an optional module of rutis-agent; the host hands the three tools to `ToolsPlugin::new` when assembling; `ToolRegistry` is unchanged | 3.4 |
| H9 | Scenario S1 uses rutis-agent with a scripted model; its E2E lives in `tests/e2e`, runs on three platforms, in nightly | 8 |

## 3. How the agent sees and calls services

### 3.1 Three tools

| Tool | Arguments | Result |
| --- | --- | --- |
| `rutis_services` | none | The services callable now: `[{name, provider, methods}]`; `provider` is a row name or "host", `methods` is `{method: "sync" \| "async"}`, `null` when the service does not say |
| `rutis_call` | `{service, method, arguments}` (`arguments` is a list) | The method's return value (JSON); on failure `error: ` followed by the error text rutis prints (§6) |
| `rutis_plugin` | `{action: "add", path}` / `{action: "reload" \| "remove", name}` / `{action: "status"}` | The outcome, one item per line (§4, §7) |

The data comes from existing seams; no new core API:

- `rutis_services`: names and providers from 0.18's `Loader::status().services`; `methods` from `HostDispatch::methods()` (`crates/rutis-bridge/src/session/services.rs`). The `Loader::services()` #505 wanted to add is replaced by the 0.18 snapshot. #505's `callable` field goes too: since 0.17 Rust services are exposed by name automatically (`#[rutis::service]`), so the list holds only what can be called.
- `rutis_call`: the existing `rutis_bridge::session::call_service`. It already does what #505 wanted fixed in the first version: sync methods run on a blocking thread, async results are awaited; the call comes from no row, so a method needing the caller's `&Ctx` fails clearly; the service is looked up on every call, so a replaced or withdrawn service is seen on the next call. Results decode as plain data; an object or function reference as a result fails the call with a reason (out of scope).
- Cancellation: when the agent's turn is cancelled, `ToolRegistry::execute` already drops the tool's future, and dropping `call_service`'s future cancels the call on the other side (0.18 §6.1). `rutis_call` sets no time limit of its own: under 0.18 §2 how long to wait is the caller's business; the limit for agent-written plugins is `limits.call` on their own runtime (§5).

### 3.2 Why not one tool per service

`ToolRegistry::register` can already add tools while running, so "install a plugin, get a tool" looks possible. We don't:

- service methods have no cross-language parameter schema (0.18's config schema covers configs; #684 generates Python types, not tool parameters), so each tool's parameters would be guessed;
- the tool list would change with every load, failed reload and replacement, so the model sees different tool definitions each turn and scripted tests are hard to reproduce; `ToolRegistry` would also need removal (#505's `register_scoped`);
- two generic tools need no change when a plugin changes language or location (a remote runtime); the agent reads the current state from `rutis_services` each turn.

Cost: one more step for the model (list, then call). Parameters come from the plugin's own description and from error text (`bad-argument` says which argument should be what).

### 3.3 Which services are callable by default

Kept from #505:

- **By default**: services provided by plugin rows (provider is a row name in `Loader::status()`), including hand-written rows, rows on remote runtimes and rows in the agent group. That is what the host installs plugins for.
- **Listed by the host**: services the host itself provides (provider "host") and services provided by peers. The host gives an `allow` list when assembling. Services outside it are not in `rutis_services`, and `rutis_call` returns `error: <service> is not open to the agent`.
- **Not listed**: services inside instances (the agent calls from the host's root and does not see them).

`allow` is also the value of `limits.services` on each agent plugin's runtime (§5). One list, meaning "the services opened to the agent's side".

The agent's file and command permissions come from its existing `bash` and `replace_text` tools; this design neither adds to nor weakens them.

### 3.4 Where it lives

- A new module `rutis_agent::rutis_tools` (rutis-agent's optional feature `plugins`, with optional dependencies on rutis-loader and rutis-bridge). It provides `RutisTools`, built from the root `Ctx`, the `Loader`, the agent group's row name and `allow`; `defs()` returns three `ToolDef`s. The host hands them to `ToolsPlugin::new(defs)` with its other tools. The tool set is fixed; `ToolRegistry` is unchanged.
- rutis-agent is an example project (outside MSRV, test hardening and audits), but #185's scenario uses it end to end, so the module lives there. The tool logic depends only on `Ctx`, `Loader`, `call_service` and #691's embedding API, not on rutis-agent's types; it moves out as a whole if it becomes a product API.
- **rutis-dsh is not connected in this round**: dsh's agent loop and tool service run in Node (TS), rutis-dsh builds only on Unix with the npm project installed, and S1 must run on three platforms. When connected later, the host gives dsh the same three operations as a service provided by name, forwarded by a dsh tool plugin; that waits for a user.

## 4. Add, reload, remove

### 4.1 The agent group and a runtime per plugin

- **The agent group**: a group row in the host's editable config (named by the host, for example `agent-plugins`); every plugin the agent installs is a child of it. It only marks which rows the agent installed: `rutis_plugin`'s `reload` and `remove` accept only rows in it, so the agent cannot touch hand-written rows. It carries no limits.
- **A runtime per plugin**: #683 sets limits on named runtimes (services rows of the same runtime provide each other are not limited by `services`). So `RutisTools` gives each agent plugin a named local runtime of its own, `agent-<plugin>` (Rust embedding's `LocalRuntime::named(..).limits(..)`), and writes the row as `agent-<plugin>:<plugin>`. Each plugin then has one process, and plugins cannot use each other's services (out of scope).
- **Records**: `RutisTools` stores each agent plugin's `{plugin, runtime name, #691 install record}` in a file the host gives it (for example `.rutis/agent-plugins.json`); #691's install records are kept by the host anyway. When the host builds the loader it puts #683's `RuntimeSet` in the `Chain` (`RutisTools` uses it to add and remove runtimes while running); at start-up `RutisTools` adds these runtimes again from the file, in §4.2's `add` order, and the rows in the config start once their runtime is there.

### 4.2 Four actions

| Action | What it does | Existing capability used |
| --- | --- | --- |
| `add {path}` | Installs the local directory with #691's `Installer::add` (language from the project file, environment prepared) and gets the row name the loader knows (such as `py:weather`); adds the runtime `agent-weather` in #683's order: first `root.plugin` with the `LocalRuntime` (language from the install result, with §5's `limits`) and its `RuntimeRowsPlugin`, then `RuntimeSet::add` with its resolver, then `Loader::create` writes the row `{"id": "weather", "name": "agent-weather:weather"}` under the agent group; stores the record; returns once the row is running or has failed | #691's embedding API; #683's named runtimes, `Limits` and `RuntimeSet`; the existing `Loader::create(entry, parent, ..)` |
| `reload {name}` | Reads the row's code again and puts it in place; when the new code does not import, its dry run fails, or it makes other rows fail, the old version keeps serving and the reason is returned. If its runtime's process was ended after a timeout, the runtime is started again first (as in `dev`, where a local runtime whose process ended is started again by the next change) | The existing `Loader::reload(id)` (`crates/rutis-loader/src/loader/api.rs`), the same one `rutis-host dev` and the rutis-dev channel's `swap` use |
| `remove {name}` | In #683's order: first `Loader::remove` deletes the row, then the `LocalRuntime` and `RuntimeRowsPlugin` fibers are unloaded (the process ends, `runtime agent-weather: stopped` is printed, other runtimes' pids are unchanged), then `RuntimeSet::remove`; after that `Installer::remove` removes the installation and the record is deleted | The existing `Loader::remove`; #683's `RuntimeSet`; #691's `Installer::remove` |
| `status` | Each agent-group row's state and reason, the services they provide, their runtimes, calls in progress | 0.18's `Loader::status()`, the agent group's part only |

- **Written to the config**: #691's embedding API reads and writes no config; `RutisTools` writes the row with `Loader::create` into the host's editable config, like any plugin the host owner installed, and it survives a restart. That is design philosophy §1's "the connection becomes part of the host". To drop it, the agent calls `remove`.
- **Only that row reloads**: `reload` does not restart a runtime process. Each agent plugin has its own runtime, but reloads are still per row, as in `dev` (a shared runtime is never restarted for one plugin), and only a per-row reload lets the old version keep serving when the new code fails. Known gap: only the row's own module is re-imported, not the submodules it imports (out of scope, already in the guide).
- **No new `add` after edits**: #691 installs local directories as a link (npm) or with `-e` (Python), so the agent edits the installed code.
- **No file watching**: embedding hosts have no file watcher; the agent calls `reload` after writing code. When each step takes effect is then deterministic. Under `rutis-host dev`, file changes reload anyway, and an extra `reload` is a reload with nothing changed.
- The `cannot reload` sentence is written today by `rutis-host`'s `dev.rs`; the implementation moves its wording into rutis-loader's `report` module, shared by `dev` and `rutis_plugin`, with the text unchanged.

## 5. Isolation: #683's options

An agent-written plugin is code nobody has reviewed; it is treated like a plugin the host opens to third parties. This design designs no mechanism; it only decides which #683 options each agent plugin's runtime uses, with which values:

| #683 option | On an agent plugin's runtime | Why |
| --- | --- | --- |
| Named local runtime (one process) | One per plugin: `agent-<plugin>` | A plugin that loops forever or crashes does not hold up others; `remove` takes the runtime with it and its process ends, which S1 step 6 asserts. Cost: one more process per plugin (tens of MB for Python) |
| `limits.call` | 30 s | Calls come in not only from the agent: hand-written plugins may use agent-written services too; a call over the limit fails with `timeout`, naming the runtime and call, and the host is not held up. 30 s equals the local greeting limit and is enough for weather or lookup calls |
| `limits.stop` | `"process"` | A plugin stuck in synchronous code cannot be cancelled (Python `def`, a JS busy loop); only ending the process stops it, and the process belongs to this plugin alone. When the agent fixes the code and calls `reload`, the runtime starts again (§4.2) |
| `limits.services` | Equal to `allow`, empty by default | The plugin's code later runs with nobody watching; by default it gets no service from outside its runtime, and using one is `not-allowed`. S1's `weather` uses no service. To let it use one (including a hand-written plugin's), the host adds the name to `allow` |
| `greeting` | Not set (30 s locally) | The agent does not start plugins under a debugger |

What is not provided, stated plainly: rutis provides no sandbox. Agent-written plugins run as the host process's user, with the same files and network; there are no memory or CPU limits (#683's guide describes limiting per process with operating-system tools, and each agent plugin has its own process, so that applies). A host that needs stronger isolation puts agent plugins on a remote runtime under another user or in a container (0.19 #695); this design adds nothing for that.

## 6. What went wrong, and how to fix it

The agent is part of the host, not the calling plugin code. Under 0.18 §2, `site` and code locations go to the person reading output and the host; the agent reads them through the host's reading interfaces, and sees the same text the host owner sees in a terminal. The errors plugin code the agent wrote receives at run time still carry only name, message, code and `data`, unchanged.

| Where it failed | Where the agent sees it | What is read |
| --- | --- | --- |
| A call fails (the method throws, no such method, wrong argument, `timeout`, `not-allowed`) | `rutis_call`'s result | `rutis_bridge::thrown()` gives the error graph, written as text by the same code 0.18 uses to print errors (#677): which row, which method, why, then the file and line in the plugin's code |
| Start-up fails after `add` (import error, `apply` error, config not matching the schema, service name conflict, a required service not in `allow`) | `add`'s result; later `status` | The row's state and reason in `Loader::status()` |
| A broken edit | `reload`'s result | The `cannot reload` sentence with reason and location; the old version keeps serving |
| Later events (runtime exits, process ended after a timeout, a call hangs) | `status` | The 0.18 snapshot: runtime `down` and why; calls in progress and how long they have waited |

Error codes (`no-method`, `bad-argument` and so on) are for programs; 0.18 keeps them out of the text for people. The agent reads text, so no codes are added; the text already says how to fix it (`methods: today, tomorrow`, `argument 1 should be a number`).

## 7. What developers see

### 7.1 The host owner

When assembling the agent:

```rust
let tools = rutis_agent::rutis_tools::RutisTools::new(&root, loader.clone(), "agent-plugins")
    .allow(["calendar"])            // empty by default
    .records(home.join(".rutis/agent-plugins.json"))
    .installer(installer)           // #691's Installer, built with the host's own places
    .runtimes(runtime_set.clone())  // #683's RuntimeSet, put in the Chain when the loader was built
    .await?;
let mut defs = vec![bash_tool(), replace_text_tool()];
defs.extend(tools.defs());
root.plugin(ToolsPlugin::new(defs))?;
```

- When the config has no `agent-plugins` yet, `RutisTools` creates the group in the editable config (`{"id": "agent-plugins", "group": true, "config": []}`); when it exists it is used as is. The group carries no settings.
- At start-up `RutisTools` adds each agent plugin's runtime back from the records file, with §5's `Limits` (`Limits::new().call(Duration::from_secs(30)).stop(Stop::Process).services(allow)`). To change the values the host passes its own `Limits` with `.limits(..)`; `services` still follows `allow`.
- The rows and errors the host owner sees in a terminal (`rutis-host status` or the host's own output) are the ones the agent sees in tool results.

### 7.2 The agent (tool results the model sees)

| Call | Result |
| --- | --- |
| `rutis_services` | `[{"name":"weather","provider":"weather","methods":{"today":"async"}}]`; `[]` when there are none |
| `rutis_call {service:"weather", method:"today", arguments:["Oslo"]}` | `"sunny (v1)"` |
| `rutis_plugin {action:"add", path:"weather"}` | `weather: running; provides weather` |
| Same, the directory is not a plugin project | `error: ./weather has no package.json, pyproject.toml or go.mod` (#691's text) |
| Same, `weather` requires `llm`, which is not in `allow` | `weather: uses llm, which the host does not let runtime agent-weather use (allowed: )` (#683's text) |
| Same, `apply` fails | `weather: failed in apply: KeyError: 'units'` + `src/weather/__init__.py:12 in apply` |
| `rutis_plugin {action:"reload", name:"weather"}` (syntax error) | `weather: cannot reload: SyntaxError: invalid syntax (src/weather/__init__.py, line 3); the previous version keeps running` |
| Same, fixed | `weather: reloaded` |
| Same, a row outside the agent group | `error: calendar was not added by the agent; only rows under agent-plugins can be reloaded or removed` |
| `rutis_plugin {action:"remove", name:"weather"}` | `weather: removed` |
| `rutis_plugin {action:"status"}` | The 0.18 snapshot's text, agent group only: `weather  running`, `  provides weather`, `runtime agent-weather  running  pid 4123` |
| `rutis_call`, the method throws | `error: call to weather.today failed: KeyError: 'region'` + `weather at src/weather/__init__.py:9 in today` |
| `rutis_call`, misspelled method | `error: weather has no method tody (methods: today)` |
| `rutis_call`, no answer in 30 s | `error: call to weather.today did not finish within 30s (the host's limit for runtime agent-weather); the call was cancelled; runtime agent-weather was ended` (#683's text, code `timeout`) |
| `weather`'s code uses an optional service not in `allow` | The plugin code gets `not-allowed`: `llm is not available to weather: the host does not let runtime agent-weather use it`; if it does not handle it, `rutis_call`'s result is that sentence after `error: call to weather.today failed: ` |
| `rutis_call`, the service does not exist or was removed | `error: no service named weather is provided` |
| `rutis_call`, a host service not in `allow` | `error: llm is not open to the agent` |

All text follows 0.18 §4.5's banned words (tool names are names the host gives the model, not error text).

### 7.3 The plugin author (the agent or a person)

An agent-written plugin is an ordinary Python plugin: a `pyproject.toml` and a plugin defined with the SDK that `provide`s a `weather` service. Nothing agent- or rutis-specific; the same plugin written by a person and placed outside the agent group behaves the same.

## 8. Scenario S1 (updated for this design)

Replaces the scenario and assertions in #185's body (the issue itself is not edited).

Roles: a new E2E fixture host `tests/e2e/fixture-agent` (Rust, embedding rutis-loader, rutis-bridge and rutis-agent), with `ScriptedLlm` as the model and a JSON script the scenario writes; its tools are only `replace_text` (works on all three platforms, no `bash`) and the three `rutis_*` tools. Each user message the scenario sends starts a turn; the fixture writes every tool call and result as a JSON line. The config has an empty `agent-plugins` group; Python is on the machine (#691 creates the environment when needed).

| Step | What the agent does | Assertions |
| --- | --- | --- |
| 1 | Turn 1: `rutis_services` gives `[]`; `replace_text create` writes `weather/pyproject.toml` and `weather/src/weather/__init__.py` (`today(city)` returns `"sunny (v1)"`) | The files exist; `rutis_services` returned `[]` |
| 2 | Same turn: `rutis_plugin add {path:"weather"}` | Result `weather: running; provides weather`; `status` shows the runtime `agent-weather` with the single row `weather`; its pid is recorded |
| 3 | Turn 2: `rutis_services`, then `rutis_call weather.today ["Oslo"]` | The list holds only `weather` (`today: async`); result `"sunny (v1)"` |
| 4 | Turn 3: `replace_text str_replace` writes a syntax error, `rutis_plugin reload weather`, then `rutis_call` | The `reload` result starts with `weather: cannot reload: SyntaxError`, contains `src/weather/__init__.py` and ends with `the previous version keeps running`; `rutis_call` still gives `"sunny (v1)"` |
| 5 | Turn 4: fixed and changed to return `"rainy (v2)"`, `reload`, then `rutis_call` | `reload` result `weather: reloaded`; `rutis_call` result `"rainy (v2)"` |
| 6 | Turn 5: `rutis_plugin remove weather`, `rutis_services`, `rutis_call` | `remove` result `weather: removed`; the list is `[]`; `rutis_call` result `error: no service named weather is provided`; `runtime agent-weather: stopped` is printed; `status` has no runtime `agent-weather` and other runtimes' pids are unchanged; the process recorded in step 2 exits within 5 s; the records file has no `weather` |

Further assertions:

- The tool definitions given to the model are the same every turn (the same four tools); `rutis_services` gives, turn by turn, `[]`, `[weather]`, `[weather]`, `[weather]`, `[]`.
- No interruption: from step 4 until step 5's `reload` returns, the fixture calls `weather.today` every 100 ms in the background, and no call fails; every call after step 5's `reload` returns gives `"rainy (v2)"`, with no alternation between old and new.
- Tool results contain none of 0.18 §4.5's banned words.
- After the fixture stops, `tests/e2e`'s residue checks pass (processes, sockets, temporary directories).

## 9. Compatibility and migration

Everything is new and not installed by default: a host without `RutisTools` behaves as before; the agent group and the `agent-*` runtimes exist only when the host installs it. The `cannot reload` sentence moves to another crate with its text unchanged. No incompatible change; nothing goes into "Migrating to 0.19".

## 10. Implementation

After 0.19. Four steps, by dependency:

| Step | Content | Can start when | Done when |
| --- | --- | --- | --- |
| 1 | `rutis_services`, `rutis_call`: the `RutisTools` skeleton, the `allow` filter, error text | 0.18's #677 (error structure, `thrown()`, printing) and #678 (`Loader::status()`) are released | Tests in rutis-agent: the list follows services appearing and disappearing; unknown service, unknown method, wrong argument and not in `allow` each have a test asserting the text exactly |
| 2 | `rutis_plugin`'s four actions (without `limits` yet), the records file and re-adding runtimes at start-up; the `cannot reload` sentence moved into rutis-loader | #691's `Installer` is done; #683's step 1 (`Limits`, named runtimes, `RuntimeSet`) is done | Tests: `add` succeeding and three failures; the old version keeps serving after a failed `reload`; rows outside the agent group are refused; after `remove` the row, runtime, installation and record are all gone; after a host restart agent plugins run as before |
| 3 | Each agent plugin's runtime gets §5's `Limits` | #683 is done | Tests: two agent plugins run in two processes and cannot use each other's services; a call to one over 30 s fails with `timeout`, its process is ended, the other carries on, and `reload` starts it again; using a service outside `allow` fails with `not-allowed` |
| 4 | `tests/e2e/fixture-agent` and `tests/e2e/tests/host_writes_plugin.rs` (§8's S1); a "let the agent write plugins" section in rutis-agent's README | 1–3 are done; the E2E framework #175 is done (closed; `tests/e2e`'s residue checks and fixture host can be used as they are) | Passes on Linux, macOS and Windows; runs in nightly as #175 assigns (PR CI runs only S2, S3, S4, S9) |

This covers #185's "Done": the two design documents (this one), and S1 updated per §8.

Needed from #683 (per its design in PR #733: limits are set on named runtimes):

1. rutis-loader's `RuntimeSet`: put in the `Chain` once when the loader is built, then `add(rows)` / `remove(name)` add and remove the resolvers of named local runtimes (`LocalRuntime::named(..)`, Node, Python, Bun) while running, sending `<runtime>:<plugin>` rows to their runtime, without restarting other runtimes;
2. A documented order: to add, first `root.plugin(LocalRuntime + RuntimeRowsPlugin)`, then `RuntimeSet::add`, then insert rows; to remove, first delete the rows, then unload those two fibers (the process ends, `runtime <name>: stopped` is printed, other runtimes' pids are unchanged), then `RuntimeSet::remove`;
3. `Limits` settable on such a runtime (`LocalRuntime::limits`): `call`, `stop`, `services`;
4. After `stop: "process"` ends the process, a Rust embedding can start that runtime again (for `reload`);
5. `timeout` and `not-allowed` errors carry `site` per 0.18's error structure and are readable by the host with `rutis_bridge::thrown()`;
6. Services that rows of the same runtime provide each other are not limited by `services` (each runtime here has one row, so this is unused, only not in conflict);
7. The greeting limit keeps its current default (nothing new).

Needed from #691 (per its design in PR #734):

1. `Installer::add` takes a local directory, detects the language from the project file and prepares the environment, with no language prefix from the caller;
2. The install result has the language and the row name the loader knows (`py:weather`); `RutisTools` takes the plugin name from it and replaces the prefix with its own runtime's name;
3. Local directories are installed as a link or with `-e`, so edits need no reinstall;
4. Install records are serializable and kept by the host; `Installer::remove` removes an installation by its record;
5. The API reads and writes no config (which group a row goes under and which config it is written to is up to `RutisTools`);
6. Failures return errors that can be printed as they are (following 0.18's text rules).

## 11. Self-review: what was removed

- #505's `Loader::services()`: 0.18's `Loader::status()` already has services and providers; removed.
- #505's `callable` field and "listed but not callable": Rust services are exposed by name since 0.17; removed.
- #505's `ToolRegistry::register_scoped` / `unregister`: the tool set is fixed and given once at assembly; removed.
- #505's resource-limit track (references, in-flight calls): belongs to #683 or later, not here.
- Limit settings on the agent group: #683 sets limits on runtimes; the group only marks the rows the agent installed.
- File watching: an explicit `reload` from the agent is deterministic; no watcher is added to embedding hosts.
- Agent-written plugins using each other's services by default, and installing from registries: no concrete user; the host can open the former with `allow`.
- Connecting rutis-dsh: S1 needs three platforms and dsh is Unix-only; waits for a user.
