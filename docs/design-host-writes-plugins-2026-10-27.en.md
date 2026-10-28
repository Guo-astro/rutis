# The host writes its own plugins: the loader as a service for plugins of every language

[English](design-host-writes-plugins-2026-10-27.en.md) · [中文](design-host-writes-plugins-2026-10-27.md)

Status: final, not implemented. Date: 2026-10-27; revised 2026-10-28 against [Remote connections: one session plugin](design-remote-session-2026-10-28.en.md) (#811) and [0.19 platform cleanup](design-platform-cleanup-2026-10-28.en.md) (#813): implemented within 0.19 in three PRs; the removed `share_by_name` / `share_registered_only`, #683's `allowed_services`, `run --allow-call` and the embedding `Installer` are gone from it; writing `rutis.json` back reuses #805's writer; `status()` follows the 0.19 snapshot; "What developers see" covers every role and error. Revised 2026-10-29 (H3): §7's text corrected word for word to the implementation — the error code is Rust services' generic `Error`; an absent `id` is generated as 8 hex digits; Python's syntax errors, `apply` failures and missing modules read as the runtime writes them; a Python row whose new code fails in `apply` on reload runs the new code and fails (as under `dev`), the old version is not put back; a successful `reload` prints no terminal line (callers outside rows, `rutis-host call` say, must be able to call it, so the methods take no calling row). Base: `main` `212ac7ce`. Scheduled: 0.19.
Issue: [#185](https://github.com/arcships/rutis/issues/185) (including the merged [#693](https://github.com/arcships/rutis/issues/693)); implementation tracked in [#771](https://github.com/arcships/rutis/issues/771) rows H1–H4. Replaces the [#505](https://github.com/arcships/rutis/pull/505) draft.
Relation to 0.18: state is [0.18 diagnostics](design-diagnostics-2026-10-26.en.md) §5's `Loader::status()`, error text follows its §4; no new structure.

Out of scope: installing from registries (#691's `rutis-host add`, inside rutis-host); the other `Loader` methods such as `update`, `set_disabled`, `move_to`, `check` (§3, S1 does not use them); any permission or limit (which plugin may use which service is the business's allocation, not the platform's; see #813 §5.5); an agent's tools, prompts or approvals (the agent product's business); a row reloading or removing itself (it would wait for its own call in progress; S1 does not do it); an editable layer for `rutis-host dev` (§5); S1 on Windows (#670's `call` is not on Windows).

## 1. What should work, and where it is stuck

[Design philosophy](design-philosophy.en.md) §1: whatever the host needs to reach, it writes a plugin for and connects it, often with an agent writing it; it tries, changes and replaces it.

The scenario: the host has a Python plugin (an agent, say). It writes a new plugin `weather`, has the host load it, learns where it broke while the old version keeps serving, puts the fixed version in place, and removes it when no longer needed.

Today (`crates/rutis-loader/src/loader/`):

- `LoaderPlugin::apply` (`plugins.rs`) already does `ctx.provide(self.loader.clone())`. A **Rust plugin** gets the `Loader` from its ctx and manages plugins with `create`, `update`, `set_disabled`, `remove`, `reload`, `entries`, `get`, `check` (`api.rs`, `check.rs`).
- Edits are all or nothing (`commit` in `commit.rs`): rewrite the editable layer → dry run (resolves and imports the new module) → reconcile; if rows newly fail, the previous layer is restored, reconciled again, and the error returned. Changes are stored through `Persist`; when storing finds the file changed by someone else (another version), it reads the file's rows back, reconciles, and redoes the edit (`persist_queue`).
- `reload(id)`: resolves the row's module again, dry-runs it and puts it in place; on failure the old module stays and the old version keeps serving.

Two things are missing:

1. `Loader` is provided only by Rust type, **not by name**, so plugins of other languages cannot reach it.
2. **rutis-host has no editable layer**: it hands `rutis.json` to the loader with `reconcile(layers, None)` (`crates/rutis-host/src/host.rs`), so `create` / `remove` inside rutis-host get `no editable layer`.

## 2. How Cordis does it

The Cordis 4 loader (`@deepseek-ai/cordis-plugin-loader`, shipped with dsh) provides itself as the service `loader` (`ctx.reflect.provide('loader', …)`). A plugin writes `inject: ['loader']` and then uses `ctx.loader.create(options, parent, position)`, `ctx.loader.update(id, options, parent?, position?)`, `ctx.loader.remove(id)` (`config/tree.ts`). Every change is written back to the config file (`tree.write()`). There is no permission layer: any plugin may use it. Reloading code is another plugin's job (HMR), not the loader's.

rutis does the same: the same name, the same method shapes, usable by anyone; with one extra method, `reload`, because rutis already reloads code in the loader (`dev` uses it). This extends through plugins: `loader` is an ordinary service `LoaderPlugin` provides, not a dedicated entry in the host or the kernel.

## 3. Decisions

| # | Decision |
| --- | --- |
| L1 | `LoaderPlugin` also provides a service `loader` by name, along the path that exists since 0.17: a type marked `#[rutis::service(name = "loader")]` is exposed automatically when `ctx.provide`d. No separate channel |
| L2 | Only the 4 methods S1 needs, with names and arguments close to Cordis's `ctx.loader` (table below) |
| L3 | Any plugin may `inject: ["loader"]`, as in Cordis. The platform adds no permission or limit: a host that does not want some plugin to manage plugins does not give it the row, or does not put it in its configuration |
| L4 | Checking and replacing use only what the loader already does (§5); the only addition is rutis-host's editable layer |
| L5 | A plugin in a local directory loads by writing it in the row name, without installing; installing from registries is rutis-host's `add` (#691) and not part of the `loader` service |

### Methods

| `loader` method | Arguments | Result | Rust `Loader` | Cordis `ctx.loader` |
| --- | --- | --- | --- | --- |
| `create(options, parent?, position?)` | `options`: `{id?, name, config?, disabled?, inject?}`, written as a row of `rutis.json` | row id | `create(NewEntry, parent, position)` | `create(options, parent, position)` |
| `reload(id)` | row id | none | `reload(id)` | none: Cordis reloads code in its HMR plugin; rutis in the loader |
| `remove(id)` | row id | none | `remove(id)` | `remove(id)` |
| `status()` | none | the state snapshot `rutis-host status` reads: rows with state and reason, services and providers, calls in progress (fields as the 0.19 snapshot has them, after #813 CL21 and #811 S4) | `status()` | no method (Cordis plugins read the loader's entry objects directly); objects do not cross processes, so a snapshot |

- A failure throws an error whose message is the text rutis prints for people (0.18 §4: which row, why, file and line) and whose `data` is `{row}`. Its code is Rust services' generic `Error` (as for a method returning a string error); no new code. Its name is the wire name `Error` after #813 CL5 (in Python a `rutis.RemoteError` whose `name` is `"Error"`).
- `update`, `set_disabled`, `move_to`, `set_inject`, `entries`, `get` and `check` stay Rust-only; when something needs them they are added the same way (Cordis's `update` handles both config and moving; follow it then).

## 4. How it crosses languages

- rutis-loader adds a type `LoaderService` (wrapping `Loader`), marked `#[rutis::service(name = "loader")]`, whose methods are the 4 above as `async fn` with serde arguments and results. `LoaderPlugin::apply` adds `ctx.provide(LoaderService(..))` next to the existing `ctx.provide(self.loader.clone())`. Under the 0.17 rule ([Rust services exposed automatically](design-rust-service-exposure-2026-10-21.en.md)) a marked type is exposed by name when provided, so Node, Bun, Python, Go, Swift and Kotlin plugins can `inject` it; Rust plugins keep taking `Loader` directly.
- Why a separate type: `Loader`'s existing methods of the same names (`create(NewEntry, …)`) take Rust types that other languages cannot pass, and the macro does not rename methods, so a thin wrapper type.
- From 0.19 services are always shared by name (#813 CL17 removes `share_by_name` / `share_registered_only`), so every host using `LoaderPlugin` exposes `loader`; there is no switch to turn it off. rutis-dsh's production code does not use `LoaderPlugin` (only the test `profile_loader.rs` does), and its Cordis's own `loader` is unaffected.
- When another row also provides a service named `loader`, the kernel's existing rule for duplicate names applies (the later provider fails); no extra check or dedicated error (§7 "People assembling plugins with rutis-host").
- Connection plugins (#811 §5.3) load through this service too: a TS / Python connection plugin an agent writes is added with `create`, and `remove` unloads its sessions and the remote plugins under them. No extra method for it.

## 5. Checking and replacing: what exists and what is missing

| Needed | Today | This design |
| --- | --- | --- |
| Check before `create` | Exists: `commit` dry-runs first (resolves and imports the new module; syntax and import errors are reported here) | Unchanged |
| `create` fails | Exists: if rows newly fail, the previous layer is **restored**, no failed row is left, the error is returned | Unchanged; error text per 0.18 |
| `reload` fails | Exists: when the new module does not resolve or its dry run fails (a syntax or import error) the old module stays; when the new code fails rows in `apply`, the previous resolution is put back and the rows reconciled again | Unchanged, said as `Loader::reload`'s documentation says it: on a syntax or import error the old version keeps running; on a failure in `apply`, the previous resolution put back is, for runtimes that load a module from its file (Python, Node), the new code in the file, so the row runs the new code and fails with that error until it is fixed and reloaded — as [`dev`](guide/rutis-host.en.md#developing-a-plugin-with-dev) reloads. No rollback of its own |
| `remove` | Exists: unloads the row, each cleanup runs once; the runtime process does not exit because of it (it may have other rows) | Unchanged |
| Changes stored | Exists: `Persist`; **rutis-host lacks an editable layer** | Added: `rutis-host run` uses the `rows` of `rutis.json` as the editable layer, with a `Persist` that writes rows back to `rutis.json`. It writes with the writer #805's `rutis-host add` already has (only `rows` change; other fields and key order stay), not a second one. After writing, the host records what it wrote as "last read", so `--watch-config` does not count it as a change |
| The file changed after the host read it (by hand, by `rutis-host add`) | Exists: `persist_queue`'s conflict handling | Unchanged: `Persist::load` reads the file's `rows` back, the loader reconciles and redoes the edit. Changes to fields other than `rows` still need SIGUSR1 or a restart ([Changing the configuration while it runs](guide/rutis-host.en.md#changing-the-configuration-while-it-runs)) |
| `create` / `reload` / `remove` called in a row's own `apply` | The loader's operations wait for every row's `apply` to end | A row calling `loader`'s `create` / `remove` / `reload` in its own `apply` waits until its own `apply` ends, so it hangs; `loader`'s methods are called after `apply` returns (in a service method or an event); to mount child plugins in `apply`, use `ctx.plugin` (Rust; Cordis plugins in Node), which belong to the row and are unloaded with it. No error or check added |
| `rutis-host dev` | Reads `rutis.dev.json` | No editable layer: `dev` develops one plugin project, and rows are added or removed in `rutis.dev.json`. Under `dev`, `create` / `remove` get §7's "no editable configuration" error; `reload` and `status` work |

## 6. How a plugin in a local directory loads

No install; the row name points at local code ([rutis-host guide](guide/rutis-host.en.md)'s row name table):

| Language | Row name |
| --- | --- |
| Node | `./plugins/weather.ts` (a plugin file relative to `rutis.json`; from 0.19 resolved by the Node runtime itself, #813 CL19) |
| Bun | `bun:./weather.ts` (relative to `runtimes.bun.project`) |
| Python | `py:weather`: module `weather.py` or `weather/__init__.py` in `runtimes.py.project` (the project directory comes first on the runtime's `PYTHONPATH`) |

These rows are exactly like hand-written ones; the plugin code writes nothing rutis-specific.

## 7. What developers see

Roles affected: **plugin authors** (writing a plugin that manages plugins, an agent say) and **people assembling plugins with `rutis-host`** (who watch the terminal and `rutis.json`). Host owners embedding rutis: a host using `LoaderPlugin` has one more service, `loader`; nothing else changes. Deployers see no change.

### Plugin authors

When it works (Python; TypeScript and Go differ only in spelling). `loader`'s methods are called after `apply` returns (in a service method or an event), here in the service method `step`; to mount child plugins in `apply`, use `ctx.plugin` (§5):

```python
class Maker:
    def __init__(self, loader):
        self.loader = loader

    async def step(self):
        row = await self.loader.create({"id": "weather", "name": "py:weather"})
        await self.loader.reload(row)
        snapshot = await self.loader.status()
        await self.loader.remove(row)


def apply(ctx, config):
    ctx.provide("maker", Maker(ctx.use("loader")))


plugin = define_plugin(apply, name="maker", inject=["loader"])
```

```ts
const loader = ctx.use<Loader>('loader')               // inject: ['loader']
const row = await loader.create({ name: 'py:weather' })   // in a service method
```

```go
var loader struct {                                    // Inject: []string{"loader"}
	Create func(ctx context.Context, options map[string]any) (string, error)
	Reload func(ctx context.Context, id string) error
	Status func(ctx context.Context) (map[string]any, error)
	Remove func(ctx context.Context, id string) error
}
if err := ctx.Use("loader", &loader); err != nil { return err }
row, err := loader.Create(ctx, map[string]any{"name": "py:weather"})   // in a service method
```

| Call | Returns |
| --- | --- |
| `create({"id": "weather", "name": "py:weather"})` | `"weather"` |
| `create({"name": "py:weather"})` | a generated id, 8 hex digits, `"3f2a91c0"` say |
| `create({"id": "w2", "name": "py:weather", "config": {"units": "metric"}})` | `"w2"` |
| `reload("weather")` (code fixed) | `None` |
| `remove("weather")` | `None` |
| `status()` | `{"rows": [{"id": "weather", "state": "running"}, …], "services": [{"name": "weather", "provider": "weather"}, …], "calls": […]}`, the same snapshot as `rutis-host status` |

When it fails: each case throws one error whose `name` is `"Error"`, `code` `"Error"`, `data` `{"row": <row id>}` (the generated id when none was given), and `message` word for word as below. The fix is in the last column.

| Trigger | `message` thrown | Where it is recorded | Fix |
| --- | --- | --- | --- |
| `create`, `weather.py` has a syntax error | `weather: cannot load weather.py: SyntaxError: expected ':' (weather.py:3)` | no row left | fix the code and `create` again |
| `create`, `apply` raises | `weather: KeyError: 'units'` | the row is rolled back, none left | fix the code or config and `create` again |
| `create`, config does not match the schema | `weather: config: units should be "metric" or "imperial", got "kelvin"` | none left | fix `config` |
| `create`, no such module | `weather: cannot load "py:wether": ModuleNotFoundError: No module named 'wether'` | none left | fix `name` |
| `create`, the `id` exists | `a row weather already exists` | the existing row unchanged | another `id`, or `remove` first |
| `reload("weather")`, syntax error | `weather: cannot reload: cannot load weather.py: SyntaxError: expected ':' (weather.py:3); the previous version keeps running` | the previous version keeps running | fix the code and `reload` again |
| `reload("weather")`, `apply` raises | `weather: cannot reload: KeyError: 'units'` | the row runs the new code and fails (§5) | as above |
| `reload` / `remove` of a row that does not exist | `no row wether` | — | use an id from `status()` |
| `create` / `remove` when the host has no editable configuration (`rutis-host dev`, an embedding host without `Persist`) | `this host keeps no editable configuration, so rows cannot be added or removed` | — | add the row in `rutis.json` / `rutis.dev.json` |
| `create` took effect but writing `rutis.json` failed | `weather: added, but rutis.json could not be written: <the operating system's reason>` | the row runs; the file lacks it, so it is gone after the host restarts | fix the file's permissions and change again, or add the row by hand |

A duplicate id and a missing row are said by the loader itself, so Rust calling `Loader::create` / `reload` / `remove` directly gets the same sentence (`LoaderError::RowExists`, `UnknownEntry`). The text follows 0.18 §4.5's banned words: the loader's former `no editable layer`, `no entry "x"`, `apply failed, rolled back: …` and `invalid entry: duplicate id "x"` become the sentences above; `<row>: cannot reload: …` is written by the same function `dev` prints with. The plugin receives the same text the host prints in its terminal.

Concepts to learn: one service `loader` and its 4 methods; rows and row names are already known (`rutis.json`). No new concept.

### People assembling plugins with rutis-host

The terminal (standard error, in the format of [What run prints](guide/rutis-host.en.md#what-run-prints)):

| What the plugin did | Terminal |
| --- | --- |
| `create` succeeded | `weather: running` (or `weather: waiting for …`, as for a hand-written row) |
| `create` failed | two lines: first `fails: failed: KeyError: 'units'` (the row cannot start), then `fails: removed` (rolled back); the plugin gets the error above |
| `reload` succeeded, or a syntax error (the previous version keeps running) | nothing: the row stays `running` |
| `reload`, the new code fails in `apply` | `weather: failed: KeyError: 'units'`; after the fix and another `reload`, `weather: running` |
| `remove` | `weather: removed` |

`rutis.json`: after `create`, `rows` ends with `{"id": "weather", "name": "py:weather"}` (`id` always written, the rest only as given); after `remove` the row is gone; other fields and key order unchanged. `rutis-host check rutis.json` treats the written file like a hand-written one.

The name `loader` taken: when another row of `rutis.json` provides a service named `loader`, that row fails with the existing duplicate-name error ([Plugin API](guide/plugin-api.en.md): "a service name has one provider": `service loader is already provided by …: row <row> cannot provide it too`), and the terminal prints `<row>: failed …` as usual. Fix: rename that row's service. No extra check or dedicated error.

Concepts to learn: plugins may rewrite `rutis.json` (only `rows`).

## 8. Scenario S1 (updated for this design)

Replaces the scenario and assertions in #185's body (the issue itself is not edited).

A scripted Python plugin `maker` stands in for the agent: `inject: ["loader"]`, provides service `maker`, whose `step(n)` does step n of a fixed script. The test starts `rutis-host run`, drives it with `rutis-host call maker.step <n>` (#670), and uses the new service with `rutis-host call weather.today '"Oslo"'`.

| Step | What `maker.step(n)` does | Assertion |
| --- | --- | --- |
| 1 | Writes `weather.py` in the Python project (provides `weather`, `today` returns `"sunny (v1)"`) | The file exists; no `weather` in `status()` |
| 2 | `loader.create({"id": "weather", "name": "py:weather"})` | Returns `"weather"`; `rutis.json` has the row; terminal `weather: running` |
| 3 | (test) `rutis-host call weather.today '"Oslo"'` | `"sunny (v1)"` |
| 4 | Writes a syntax error, `loader.reload("weather")` | The thrown text starts with `weather: cannot reload: cannot load weather.py: SyntaxError`, contains `weather.py` and ends with `; the previous version keeps running`; `weather.today` is still `"sunny (v1)"` |
| 5 | Changes it to return `"rainy (v2)"`, `loader.reload("weather")` | Returns; `weather.today` is `"rainy (v2)"` |
| 6 | `loader.remove("weather")`, `status()` | No `weather` row or service; `rutis.json` byte for byte as at step 1; terminal `weather: removed`; `rutis-host call weather.today '"Oslo"'` prints `rutis-host: no row provides weather (services: loader, maker)` |

Also: during step 4 the test calls `weather.today` every 100 ms and none fails; after step 5 all are v2; the leftover check passes after the host stops. In `tests/e2e/tests/host_writes_plugin.rs` (E2E framework #175 is done); Linux and macOS, in nightly.

## 9. Compatibility and "Migrating to 0.19"

- Hosts using `LoaderPlugin` (rutis-host included) gain a service `loader` every plugin can use.
- A row already providing `loader` by name collides with it; under the existing duplicate-name rule the later one fails. Both "Migrating to 0.19" documents get one sentence: "`loader` is now a service rutis provides itself; rename a plugin's service of that name". 0.19 keeps no compatibility and no switch.
- rutis-host: rows added or removed through `loader` while running are written back to `rutis.json`; other fields and key order stay.

## 10. Implementation

Three PRs, each closed (what it replaces is removed and the docs are changed in the same PR), tracked in #771:

| PR | Content | Done when | Depends on (#771 rows) |
| --- | --- | --- | --- |
| H2 | rutis-host's editable layer: `run` uses the `rows` of `rutis.json` as the editable layer, `Persist` with #805's writer; none under `dev`; the error text `this host keeps no editable configuration, …` and the write-failure line | After adding and removing rows the file is byte-for-byte right with key order kept; `--watch-config` does not count the write as a change; an edit after an outside change merges by the conflict handling; restarts as usual; rutis-host.md "Changing the configuration while it runs" says so | P2 (#805) |
| H3 | rutis-loader's `LoaderService` with its 4 methods, provided by `LoaderPlugin`; §7's error text (replacing `no editable layer`, `no entry`, `apply failed, rolled back` with every user changed in the same PR); the `cannot reload` text written by the loader, `dev` using it | One test each in Node, Python and Go making the same calls of `create` / `reload` / `remove` / `status` after `inject`, comparing every row of §7's plugin-author table word for word; an end-to-end test each for the rows "no editable configuration" and "writing back failed"; guarantee `loader/service` added | CL5, CL17 (with C2), CL21, CL23 |
| H4 | S1 (§8); a section "Managing plugins with `loader`" in rutis-host.md and plugin-api.md (both languages); the sentence in "Migrating to 0.19"; closes #185 | Passes on Linux and macOS, in nightly | H2, H3; changes the same docs as G3 and G5, not in parallel with them |

H1 is this revision (the two design documents only, and rows H2–H4 added to #771).
