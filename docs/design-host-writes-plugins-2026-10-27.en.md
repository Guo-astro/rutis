# The host writes its own plugins: the loader as a service for plugins of every language

[English](design-host-writes-plugins-2026-10-27.en.md) · [中文](design-host-writes-plugins-2026-10-27.md)

Status: final, not implemented. Date: 2026-10-27. Base: `main` `103c0d22`. Scheduled: 0.19 (design only); implementation after 0.19.
Issue: [#185](https://github.com/arcships/rutis/issues/185) (including the merged [#693](https://github.com/arcships/rutis/issues/693)). Replaces the [#505](https://github.com/arcships/rutis/pull/505) draft.
Relation to 0.18: state is [0.18 diagnostics](design-diagnostics-2026-10-26.en.md) §5's `Loader::status()`, error text follows its §4; no new structure.

Out of scope: installing from registries (#691's `rutis-host add` and embedding `Installer`, the host's business); the other `Loader` methods such as `update`, `set_disabled`, `move_to`, `check` (§3, S1 does not use them); any permission layer (a host that wants limits uses #683's `allowed_services`); an agent's tools, prompts or approvals (the agent product's business); a row reloading or removing itself (it would wait for its own call in progress; S1 does not do it); S1 on Windows (#670's `call` is not on Windows).

## 1. What should work, and where it is stuck

[Design philosophy](design-philosophy.en.md) §1: whatever the host needs to reach, it writes a plugin for and connects it, often with an agent writing it; it tries, changes and replaces it.

The scenario: the host has a Python plugin (an agent, say). It writes a new plugin `weather`, has the host load it, learns where it broke while the old version keeps serving, puts the fixed version in place, and removes it when no longer needed.

Today (`crates/rutis-loader/src/loader/`):

- `LoaderPlugin::apply` (`plugins.rs`) already does `ctx.provide(self.loader.clone())`. A **Rust plugin** gets the `Loader` from its ctx and manages plugins with `create`, `update`, `set_disabled`, `remove`, `reload`, `entries`, `get`, `check` (`api.rs`, `check.rs`).
- Edits are all or nothing (`commit` in `commit.rs`): rewrite the editable layer → dry run (resolves and imports the new module) → reconcile; if rows newly fail, the previous layer is restored, reconciled again, and the error returned. Changes are stored through `Persist`.
- `reload(id)` resolves the row's module again, dry-runs and swaps it in; on failure the old module stays and the old version keeps serving.
- `check(&layers)` checks a candidate configuration without running anything (each row resolves and passes the dry run, every dependency has a provider, no dependency cycle, no duplicate provider).

Two things are missing:

1. `Loader` is provided only by its Rust type, **not by name**, so plugins of other languages cannot reach it.
2. **rutis-host has no editable layer**: it hands `rutis.json` to the loader with `reconcile(layers, None)` (`crates/rutis-host/src/host.rs`), so `create` / `remove` in rutis-host answer `no editable layer`.

## 2. How Cordis does it

Cordis 4's loader (dsh ships `@deepseek-ai/cordis-plugin-loader`) provides itself as the service `loader` (`ctx.reflect.provide('loader', …)`). A plugin writes `inject: ['loader']` and uses `ctx.loader.create(options, parent, position)`, `ctx.loader.update(id, options, parent?, position?)`, `ctx.loader.remove(id)` (`config/tree.ts`). Each change is written back to the config file (`tree.write()`). There is no permission layer: any plugin may use it. Reloading code is another plugin's job (HMR), not the loader's.

rutis does the same: the same name, the same method shapes, usable by anyone by default; plus one `reload`, because in rutis code reloading already lives on the loader (`dev` uses it).

## 3. Decisions

| # | Decision |
| --- | --- |
| L1 | `LoaderPlugin` also provides a service named `loader`, by the path that exists since 0.17: a type marked `#[rutis::service(name = "loader")]`, exposed by name automatically when `ctx.provide`d. No new channel |
| L2 | Only the 4 methods S1 uses, named and shaped after Cordis's `ctx.loader` (table below) |
| L3 | Any plugin may `inject: ["loader"]` by default, as in Cordis; a host that wants limits uses #683's `allowed_services`. No separate permissions |
| L4 | Checks and swaps use only the loader's existing behaviour (§5); the only addition is rutis-host's editable layer |
| L5 | A plugin in a local directory loads by naming it in the row; no installation. Installing from registries is #691's and stays out of the `loader` service |

### Methods

| `loader` method | Arguments | Result | Rust `Loader` | Cordis `ctx.loader` |
| --- | --- | --- | --- | --- |
| `create(options, parent?, position?)` | `options`: `{id?, name, config?, disabled?, inject?}`, written as a row of `rutis.json` | row id | `create(NewEntry, parent, position)` | `create(options, parent, position)` |
| `reload(id)` | row id | none | `reload(id)` | none: Cordis reloads code in its HMR plugin; rutis does it on the loader |
| `remove(id)` | row id | none | `remove(id)` | `remove(id)` |
| `status()` | none | 0.18's snapshot: rows with state and reason, services and providers, runtimes, calls in progress | `status()` (0.18) | no method (Cordis plugins read the loader's entry objects directly); objects do not cross processes, so a snapshot |

- Failures throw: the message is the text rutis prints for people (0.18 §4: which row, why, file and line), and `data` is `{row}`. No new error code.
- `update`, `set_disabled`, `move_to`, `set_inject`, `entries`, `get`, `check` stay Rust-only; they are added the same way when someone needs them (Cordis's `update` handles both config and moving; follow it then).

## 4. How it is provided across languages

- rutis-loader adds a type `LoaderService` (wrapping a `Loader`), marked `#[rutis::service(name = "loader")]`, with the table's 4 methods as `async fn`s taking and returning serde types. `LoaderPlugin::apply` provides it next to the existing `ctx.provide(self.loader.clone())`. Under the 0.17 rule ([Rust service exposure](design-rust-service-exposure-2026-10-21.en.md)), a marked type is exposed by name when provided, so plugins in Node, Bun, Python, Go, Swift and Kotlin can `inject` it; Rust plugins keep taking `Loader` directly.
- Why a separate type: `Loader`'s existing methods of the same names (`create(NewEntry, …)`) take Rust types that other languages cannot send, and the macro cannot rename methods, so a thin wrapper type it is.
- It is visible only where the host shares services by name (rutis-host does, `share_by_name`). Hosts using `share_registered_only` (rutis-dsh, whose Cordis has its own `loader`) are unaffected.

## 5. Checks and swaps: what exists and what is missing

| Needed | Today | This design |
| --- | --- | --- |
| A check before `create` | Exists: `commit` dry-runs first (resolves and imports the new module; syntax and import errors are reported here) | Unchanged |
| A failed `create` | Exists: if rows newly fail, the previous layer is **restored**, no failed row is left behind, and the error is returned | Unchanged; the error text follows 0.18 |
| The old version keeps serving after a failed `reload` | Exists: `reload` is all or nothing and keeps the old module on failure | Unchanged. Known boundary: when the new code fails in `apply`, the old version is unloaded and loaded again, briefly unavailable in between (syntax and import errors are stopped in the dry run and never get here) |
| `remove` | Exists: unloads the row, each cleanup running once; its runtime process does not exit for it (it may have other rows) | Unchanged |
| Checking a candidate configuration before applying | Exists: `Loader::check` | Not in the service (S1 does not use it) |
| Storing changes | Exists: `Persist`; **rutis-host lacks an editable layer** | Added: rutis-host makes `rutis.json`'s `rows` the editable layer, with a `Persist` writing rows back to `rutis.json` (only `rows`, other fields untouched). What it writes is what a reread produces, so `--watch-config` sees no change next time |

## 6. How plugins in local directories load

No installation; the row name points at the local code ([rutis-host guide](guide/rutis-host.en.md), row names):

| Language | Row name |
| --- | --- |
| Node | `./plugins/weather.ts` (a plugin file relative to `rutis.json`) |
| Bun | `bun:./weather.ts` (relative to `runtimes.bun.project`) |
| Python | `py:weather`: the module `weather.py` or `weather/__init__.py` in `runtimes.py.project` (the project directory is ahead on the runtime's `PYTHONPATH`) |

These rows are exactly like hand-written ones, and the plugin code has nothing rutis-specific in it.

## 7. What developers see

The plugin author (Python sketch):

```python
plugin = define_plugin(name="maker", inject=["loader"], apply=apply)

async def apply(ctx, config):
    loader = ctx.use("loader")
    row = await loader.create({"name": "py:weather"})
    await loader.reload(row)
    await loader.remove(row)
```

| Call | Result |
| --- | --- |
| `create({"name": "py:weather"})` | `"weather"` (generated when no `id` is given); `rutis.json` gains a row; the terminal shows `weather: running` |
| Same, `weather.py` has a syntax error | Throws: `weather: cannot load weather.py: SyntaxError: invalid syntax (weather.py, line 3)`; `rutis.json` unchanged |
| Same, `apply` fails | Throws: `weather: failed in apply: KeyError: 'units'` + `weather.py:12 in apply`; the row is rolled back |
| Same, the `id` exists | Throws: `a row weather already exists` |
| `reload("weather")`, syntax error | Throws: `weather: cannot reload: SyntaxError: invalid syntax (weather.py, line 3); the previous version keeps running` |
| Same, fixed | Returns; the terminal shows `weather: reloaded` |
| `remove("weather")` | Returns; `rutis.json` loses the row; the terminal shows `weather: removed` |
| `reload("wether")` | Throws: `no row wether` |
| `create` in an embedding host with no editable layer | Throws: `this host keeps no editable configuration, so rows cannot be added or removed` |
| `status()` | `{rows: [{id: "weather", state: "running"}, …], services: [{name: "weather", provider: "weather"}, …], runtimes: […], calls: […]}` |

The text follows 0.18 §4.5's banned words (loader-internal wording such as `no editable layer` becomes the sentences above). The host owner sees in the terminal the same text the plugin receives.

## 8. Scenario S1 (updated for this design)

Replaces the scenario and assertions in #185's body (the issue itself is not edited).

A scripted Python plugin `maker` stands in for the agent: `inject: ["loader"]`, providing the service `maker` whose `step(n)` does step n of a fixed script. The test starts `rutis-host run --allow-call` (#670), drives with `rutis-host call maker.step <n>`, and uses the new service with `rutis-host call weather.today '"Oslo"'`.

| Step | What `maker.step(n)` does | Assertions |
| --- | --- | --- |
| 1 | Writes `weather.py` in the Python project (provides `weather`, `today` returns `"sunny (v1)"`) | The file exists; no `weather` in `status()` |
| 2 | `loader.create({"id": "weather", "name": "py:weather"})` | Returns `"weather"`; `rutis.json` gained the row |
| 3 | (test) `rutis-host call weather.today '"Oslo"'` | `"sunny (v1)"` |
| 4 | Writes a syntax error, `loader.reload("weather")` | The thrown text starts with `weather: cannot reload: SyntaxError` and contains `weather.py`; `weather.today` still gives `"sunny (v1)"` |
| 5 | Changes it to return `"rainy (v2)"`, `loader.reload("weather")` | Returns; `weather.today` gives `"rainy (v2)"` |
| 6 | `loader.remove("weather")`, `status()` | No `weather` row or service; `rutis.json` as at step 1; `weather.today` answers `no row provides weather` |

Also: during step 4 the test calls `weather.today` every 100 ms and no call fails; after step 5 every call gives v2; after the host stops the residue checks pass. Location `tests/e2e/tests/host_writes_plugin.rs` (the E2E framework #175 is done); Linux and macOS, in nightly.

## 9. Compatibility

- New, available by default, as in Cordis: every host that shares names gains a service `loader`.
- A plugin or host that already provides `loader` by name now clashes with it (the later provider fails, the existing same-name rule). `loader` is a reserved name, as in Cordis; this is not a 0.19 breaking item, only one note in "Migrating to 0.19": rename it.
- rutis-host: rows added or removed through `loader` while running are written back to `rutis.json`; other fields of the file are unchanged.

## 10. Implementation

After 0.19, one PR:

| Content | Done when |
| --- | --- |
| `LoaderService` and its 4 methods, provided by `LoaderPlugin`; error text rewritten per 0.18 | One test each in Node, Python, Go: `inject`, then `create` / `reload` / `remove` / `status`, and the text of every row of §7 |
| rutis-host's editable layer and the `Persist` writing `rutis.json` | After adding and removing rows the file is right, a reread is no change, and a restart works |
| S1 (§8); a section "managing plugins with `loader`" in rutis-host.md and the plugin API guide (both languages) | Passes on Linux and macOS |

Dependencies: 0.18's `Loader::status()` (#678) and error text (#677); S1 uses #670's `call`. No dependency on #683 (only a host that wants limits uses its `allowed_services`) or #691.
