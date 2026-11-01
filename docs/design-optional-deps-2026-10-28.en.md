# Optional dependencies for plugins in other languages

[中文](design-optional-deps-2026-10-28.md)

Status: implemented (branch `claude/design-optional-deps`). Date: 2026-10-28 (revised the same day: optional dependencies are now a snapshot at load, as Rust's `Option<Arc<T>>`, instead of one child fiber per optional name; added the other three paths that build rows, where "in both lists" fails, the row's own `inject`, Go's text, the testing tools, and Swift/Kotlin's method check. Reviewed the same day: 12 dimensions plus a completeness check; of 15 findings, 12 confirmed and 3 discarded, plus 8 completeness gaps and 1 unverified; revised for the confirmed ones; a further pass, from a Rust-vs-leaf comparison, splits the replaced case by the service's form — a Marked call's provision identity check makes it say `unavailable`, only a hand-written `HostDispatch` keeps the old object, and step 1's cases are split to match). Base: `main` `4c58a3c5` (0.19.1). Ported from `7e57170d` on 2026-10-30: upstream replaced `RuntimeResolver` with `SessionResolver`, removed `RuntimeInstances`, and moved every language's testing tool onto a real host; this design's construction paths (§2, §4.3) and cases follow, with no change in behaviour. Optional dependencies ship in the next minor version (0.20). Revised 2026-10-31, §4.4 "It is replaced": when another runtime's row replaces its own object while running, a handle held keeps the old object until that row unloads (the original said `unavailable`, against the registered `boundary/5.1`, "a snapshot keeps its object"; in Rust an `Arc<T>` already taken is not invalidated by a replacement either); only once the row unloads (stops or restarts) does it say `unavailable`. Revised the same day, O4: `use` no longer counts names in `optional` as declared, as in Rust (`Option<Arc<T>>` is not in the fiber's `declared_injects`, and `ctx.require` says `Undeclared` for it; measured with a temporary test: `Deps` and `ctx.get` both have it, `require` says `dependency not declared`); optional services are read with `get` only, and `use` of a name only in `optional` says `use`'s existing line (§5 row 2). Rebased onto main `d5aa3200` (0.19.3) on 2026-10-31: upstream made each service a test gives in Node's and Bun's testing tools a row of its own (#988), and §4.6 says so; no behaviour changes.
Issue: none yet, to be opened with this design. Related: [#683](https://github.com/arcships/rutis/issues/683)'s `allowed_services` ([design](design-host-plugin-limits-2026-10-27.en.md) §4.3), whose rule for "an optional dependency not on the list" has nothing to apply to until this design; [#676](https://github.com/arcships/rutis/issues/676) / #704's `not-declared`.

Out of scope: Rust plugins (they have `Option<Arc<T>>` and `ctx.get`, unchanged); an optional form of a row's own `inject` (the one on a rutis.json row); Cordis 3 / Koishi's `inject: { required, optional }`; rerunning `apply`, or leasing and giving back, as an optional dependency comes and goes (Rust's `Option` does neither, §3 O3).

## 1. Where things stand

What a plugin author wants: summarize with `llm` when there is one, otherwise cut the text short.

| Where | How | What happened before this design (base 0.19.1) |
| --- | --- | --- |
| Cordis | Leave it out of `inject`, use `ctx.get('llm')` | Not gated; `get` reads the service table, returns the service while its provider is ACTIVE, else `undefined`; nothing reruns when it comes or goes |
| Rust | `type Deps = Option<Arc<Llm>>`, or `ctx.get::<Llm>()` | Not gated; `Option` is a snapshot at load: it is not in `declared_injects`, so no reverse edge is made, the plugin is not evicted when the provider goes and keeps its `Arc`, and nothing wakes it when the provider comes later ([deps.rs](../crates/rutis/src/deps.rs)); `ctx.get` reads at any time |
| TypeScript (leaf), Bun, Python, Go, Swift, Kotlin | Only `inject` is available | Without `llm` on the host the row waits forever: `Loader::status()` says `waiting for llm` |
| Node (TS leaf) / Bun / Python | Leave it out of `inject`, call `ctx.use('llm')` | Not found even when the host provides `llm`: `service llm is not available to this plugin, which does not declare it in inject: add 'llm' to its inject`. Found when another row in the same process happens to declare `llm` |
| Go, Swift, Kotlin | Leave it out of `Inject` | `Use` checks the plugin's declaration before the service table and refuses outright; another row's declaration does not help |
| A Cordis plugin in the Node runtime | Leave it out of `inject`, use `ctx.get('llm')` | The host's `llm` is not leased into the process: `undefined`; present only when another row declares it |

There are two causes.

- **Declaring has only required.** M1 designed optional dependencies with `required: false` ([M1 §5.2](design-multilang-m1-2026-10-04.en.md)) and dropped them when implementing: Cordis 4's `inject` is an array or a map from names to intercept config and cannot say optional ([M1 §13](design-multilang-m1-2026-10-04.en.md)). The leaf runtimes since then kept "`inject` only".
- **Leasing follows gating.** M1 §5.4 moved host services from "registered for the whole process when the runtime starts" to "leased by each row for its own dependencies". In the code one list, `gated`, is both the row's `injects` (gating) and the names `JsRow::start` leases ([runtime.rs](../crates/rutis-loader/src/runtime.rs)). A name that is not gated has nowhere to be leased. `lease_host_for` also makes a registration shared in the process, so another row may read it without declaring it, but that depends on the other row.

## 2. Existing seams

| Seam | Where | How this design uses it |
| --- | --- | --- |
| `rows.schema`, `plugins.read`, `--rutis-manifest` report `inject`, `provides` | `RowSchema` (rutis-bridge `process.rs`; `PluginSchema` and `BinaryManifest` embed it), each runtime's `describe` | Adds `optional` |
| Four paths build a row from a declaration | In the loader: `SessionResolver` (`runtime/sessions.rs`) and `BinaryResolver` (both through `leaf_resolved_on`), `DefinitionMounter::mount` (through `JsFactory::with_keys`), copies in instances through `JsModule::scoped`; in the bridge: `SessionHandle::run` (`RunRow` in `session/plugin/run.rs`, which `sessions.open` uses) | The loader's three use one function for `optional`; `RunRow`, in the bridge, cannot use it and makes the same duplicate check and leases itself. `RunRow` leases only for a runtime reporting the `leaf` feature (Bun, Python, Go, Swift, Kotlin); the Node runtime reports `leaf.js`, for which `RunRow` already leases no `inject`, and no `optional` either (doing so needs the bridge to judge by the service catalog as the loader does, outside this design) |
| Gating and leasing share `gated` | `JsFactory.gated` → `injects()` and `JsRow::start`'s lease loop | `optional` is kept apart, used for leasing only, not in `injects` |
| Leasing a host service for a row | `Process::lease_host_for`: a shared registration plus the row's own label; the last lease released sends `hosts.withdraw`; when the runtime has no `scopes`, only the shared lease is made and calls name no row | Optional names use the same function; with no `scopes` they degrade to the shared lease as required ones do |
| Leases go back after the row unloads | In the cleanup `JsRow::apply` returns: `unload_row` first, then `release(leases)` ("the plugin never sees a service it injects go away before it does") | Optional leases go into the same `leases`; the order is unchanged |
| Each language's `use` | Looks the service up in the process; when missing, `not-declared` if undeclared, `unavailable` if declared; "declared" = the plugin's `inject` ∪ `optional`; Node/Bun/Python also the row's `inject`, while Go/Swift/Kotlin currently drop the row's `inject` | `use`'s declared set is unchanged (no `optional`, as Rust's `require`); `get` is added, with `inject` ∪ `optional` as its declared set; the row-`inject` gap in Go/Swift/Kotlin is not fixed here (since closed by [declared services](design-declared-services-2026-10-29.en.md): in all six languages the declared set includes the row's `inject`) |

## 3. Decisions

| # | Decision | Why; alternatives dropped |
| --- | --- | --- |
| O1 | Every language adds a list `optional` beside `inject`; `rows.schema` (and `plugins.read`, `--rutis-manifest`) report `optional: string[]` | `inject` means required in every language and keeps that meaning. Dropped `inject: { llm: { required: false } }`: Cordis 4 takes the object's values as intercept config. Dropped a mark on the name (`'llm?'`): the checks, errors and testing tools of seven languages would all parse it. Dropped each language's own nullable type: Go has none, and `rows.schema` needs a list of names anyway. Dropped declaring nothing and leasing every service the host allows into every process (so `get` reads a whole table as in Rust and Cordis): every process would `hosts.provide` every host service, and without the row's own lease a call cannot tell which row makes it, so host methods that need the calling row fail |
| O2 | Optional names are not gated: not in `injects` or `requires`, never in `waiting for …` or `StillWaiting` | As in Rust and Cordis |
| O3 | As Rust's `Option<Arc<T>>`: a snapshot at load. Leased when the row starts if visible then, the lease kept until the row stops; not leased when it comes later, not given back when its provider goes; the row restarts for neither. Across processes a call after the provider goes does not reach the old object (except a hand-written `HostDispatch`, which its host owns); when a Marked service is withdrawn, or another runtime's row stops while its process lives, the call says `service <name> is not available` (`unavailable`), and when the process is gone, `peer-gone`; `get` still returns the leased proxy, the failure comes at the call | The same as Rust, so plugin authors learn no new rule; implementing it adds a few lines to the existing lease loop. The cost is Rust's: when another runtime's row provides the optional service, whether the row gets it depends on startup order, and using it means restarting the row. The one difference across processes: an `Arc` cannot be held to keep the old object alive, so the failure is pinned down here (§4.4, §5 row 3) instead of copying "the old object". Dropped "follow the service" (a child fiber per optional name, leasing when it comes and giving back when it goes): another mechanism, semantics unlike Rust's, and the child's row, diagnostics and unload order would all need rules of their own |
| O4 | Reading: every language adds `get(name)`, an empty value when absent; `get`'s declared set is `inject` ∪ `optional`, and an undeclared name says `not-declared`. `use(name)`'s declared set does not include `optional`: for a name only in `optional`, `use` says its existing line, as for an undeclared one | `get`: an undeclared name is never leased, so `get` would return empty forever without saying why (#676's rule: say what to declare). Rust and Cordis allow undeclared reads because they read the whole service table of their own process. `use`: as in Rust, `Option<Arc<T>>` is not in the fiber's `declared_injects` and `ctx.require` says `Undeclared` for it; optional services are read with `get`. Rejected: "`use` counts `optional` too and says `unavailable` when absent" (the original), which differs from the Rust kernel |
| O5 | A name in both the plugin's `inject` and `optional`: the row fails; a name in the row's own `inject` and the plugin's `optional` also fails (the row's `inject` is a required dependency of the host's configuration and cannot also be optional); the plugin's two lists are checked before filtering, the row's `inject` before the loader merges it into `requires` | The plugin's two lists are checked in one function of the loader, the same for every language and path; the row's `inject` must be checked before `combined_injects`, not in `JsRow::start` — that step may never be reached because the name is not ready. Before filtering: in the Cordis runtime a name that is not shared, written in both, is a mistake too. Dropped "treat it as required": it hides what the author has not decided |
| O6 | A Cordis plugin in the Node runtime writes `optional` on the plugin (where `inject` is) and reads with Cordis's own `ctx.get` | Cordis ignores the unknown `optional`; services of Cordis rows in the same process are already `get`-able in Cordis, and `optional` only says which shared names to lease in |
| O7 | An optional name #683's `allowed_services` does not allow: not leased | As #683 §4.3 and §5 row 4 say; this design only says where it is checked |
| O8 | `get` on an undeclared name says a new line (§5 row 1); `use` keeps its existing line, for an undeclared name and for a name only in `optional` | For a `get` caller, "add it to its inject" is the wrong advice: it would make the row wait for it. A `use` caller wants a required dependency, so its line is right; leaving it keeps the existing word-for-word tests |
| O9 | Go's text follows Go's existing form: field names `Optional`, `Inject`, the name with `%q` | Go's `Use` already says `does not declare it in Inject: add "llm" to its Inject`, since Go authors write field names; the cross-language case compares Go with Go's line |

## 4. Rules

### 4.1 Declaring

In every language a list of service names, checked as `inject` is (each a string, else an error at definition, with the text of that language's existing `inject` check, `inject` replaced by `optional`).

| Language | How |
| --- | --- |
| TypeScript / JavaScript (`definePlugin`) | `optional: ['llm']` |
| A Cordis plugin in the Node runtime | `optional = ['llm']` on the plugin object, function or class, where `inject` is |
| Bun (module exports) | `export const optional = ['llm']` |
| Python | Module attribute `optional = ["llm"]`; or `define_plugin(apply, inject=…, optional=["llm"])` |
| Go | `rutis.Plugin{..., Optional: []string{"llm"}}`; each plugin in `--rutis-manifest` carries `optional` |
| Swift | `Plugin(name:, inject:, optional: ["llm"], …)`; `--rutis-manifest` as above |
| Kotlin | `Rutis.define(name, inject = …, optional = listOf("llm"), …)` |

### 4.2 Protocol

- Each plugin in `rows.schema`, `plugins.read` and `--rutis-manifest` adds `optional: string[]`, `[]` when left out. On the Rust side `RowSchema` adds `#[serde(default)] pub optional: Vec<String>` (`PluginSchema` and `BinaryManifest` embed it and so have it too).
- `rutis-host check` also reads the plugin's declaration and prints `inject:` (`main.rs`'s `check`): `meta` gains `optional`, printed when non-empty in the same JSON form as `inject` (a line `optional: [...]`; omitted when empty, so plugins without `optional` print as before); an absent optional dependency is not a failure.
- No new control operation, no `PROTOCOL` bump: leasing and giving back use the existing `hosts.provide` / `hosts.withdraw`.

### 4.3 Loader

1. One function splits a declaration in two; the loader's three paths that build rows (§2) use it (the bridge's `RunRow` follows the same rules itself):
   1. Duplicates first: a name in both the plugin's `inject` and `optional` fails (§5 row 4).
   2. Then filtering: `gated` as now; `optional` by the same rule (all of it in a leaf runtime; in the Cordis runtime only names shared in the catalog).
2. `JsFactory` (and `JsModule`) gain a field `optional: Vec<(String, TypeKey)>`, its keys computed as `gated`'s are (shared names keyed by instance). It is not in `injects()` nor in `Declared.requires`, so the row is never Pending on these names, `Loader::status()` does not list them, and there is no `StillWaiting`.
3. Where duplicates fail: `SessionResolver` and `BinaryResolver` when resolving, as the existing resolve error (the row is `unresolved`); `DefinitionMounter` when building, as the existing build error; `RunRow` before loading, as `RowHandle::loaded`'s error. The message is the same, its prefix each path's existing one (§5 row 4).
4. A name in the row's own `inject` and the plugin's `optional` is checked before the loader merges the row's `inject` into `requires` (around `combined_injects`), not in `JsRow::start` — that step may never be reached because the name is not ready; its message is §5 row 4b.

### 4.4 Starting and stopping a row

One step is added to `JsRow::start`'s existing order ([runtime.rs](../crates/rutis-loader/src/runtime.rs)). A name in the row's own `inject` and in `optional` is checked before the loader merges `requires` (§4.3.4); `JsRow::start` does not check it:

```
① for (name, key) in &self.gated                      (unchanged)
     require_key → skip if provided in the same process → lease_host_for → leases.push
② for (name, key) in &self.optional                   ← new
     not allowed by #683 → skip
     get_key::<dyn HostDispatch>(key)
       None → skip (not looked at again while this row runs)
       Some(dispatch): skip if provided in the same process; else lease_host_for → leases.push
       when leased, the label is handled as in ① (the row's own label pushed into isolate; a runtime without scopes gets only the shared lease, and calls name no row)
③ rows.load                                           (unchanged)
Unloading: unload_row, then release(leases)           (unchanged: optional leases go back with them)
```

- It comes later: not leased for the row. The row is sure to see it after a restart (`FiberView::restart()`, a config change, `rutis-host` reloading the row). Until then `get` still reads one the process already has — provided by a row of the same process, or leased in by another row of the process for itself (a lease also registers a shared one at the process's root, see `Process::lease_host_for`) — as Rust's `ctx.get` reads the service table as it is now.
- Its provider goes: not given back, and the row does not stop. `get` still returns the leased proxy; calls depend on the service's form: a hand-written `HostDispatch` reaches the old object (its host owns that); Marked says `service <name> is not available` (`unavailable`) once its provider is withdrawn, and another runtime's row says the same when its provider's row stopped but its process lives, and `peer-gone` when the process is gone. Node currently throws a `TypeError` after the row stops; this design requires the uniform error above.
- It is replaced: also by the service's form. A hand-written `HostDispatch` keeps the old one until the row restarts; a Marked call goes through the provision identity check ([ctx.rs](../crates/rutis/src/ctx.rs): the slot is provided again, `provision.is(binding)` no longer matches) and says `service <name> is not available` (`unavailable`); another runtime's row: when the provider row restarts (unloaded, then loaded again), as the line above, `unavailable`; when the row replaces its own object while running, a handle held keeps the old object until that row unloads, as a hand-written `HostDispatch`, an `Arc<T>` already taken in Rust, and `boundary/5.1` do (the six runtimes put the handles of a row's slots out of use only when the row unloads or restarts; a replacement only changes the slot's current handle, and an old handle keeps its own object's methods). This is visible behaviour and step 1 needs a case for each form.
- The row fails while leasing or in `rows.load`: the existing failure path gives back `leases`, optional ones included.

### 4.5 Reading

| Language | Optional read (new) | Required read (unchanged) |
| --- | --- | --- |
| TypeScript (leaf) | `ctx.get<T>(name): T \| undefined` | `ctx.use<T>(name): T` |
| Cordis plugin | Cordis's `ctx.get(name)` | Cordis's `ctx[name]` |
| Bun | `ctx.get(name): unknown \| undefined` | `ctx.use(name)` |
| Python | `ctx.get(name) -> Any \| None` | `ctx.use(name)` |
| Go | `ctx.Get(name, target) (bool, error)`: `false, nil` when absent | `ctx.Use(name, target) error` |
| Swift | `ctx.get(name, P.self) throws -> P.Service?`, `ctx.get(name) throws -> Client?` | `ctx.use(…)` |
| Kotlin | `ctx.get(name, type): I?` | `ctx.use(name, type): I` |

- It reads what the process's service table holds now: the object itself for a provider in the same process (as `use`); a proxy for a leased one; an empty value when there is none. So host services are as §4.4's snapshot left them, while services of other rows in the same process change as the runtime's own table does (as Cordis's `ctx.get`).
- Declared for `get` = the plugin's `inject` ∪ the plugin's `optional`; declared for `use` leaves `optional` out (next item); Node/Bun/Python also the row's `inject`. Go/Swift/Kotlin currently know only the plugin's `inject`; this design adds `optional` to `get`'s set and does not fix the row's `inject` (a separate gap, noted in §6). (Since closed by [declared services](design-declared-services-2026-10-29.en.md): in all six languages the declared set includes the row's `inject` and the names the plugin provides itself, and `use` checks the declaration first; rutis gating a runtime's row on its row `inject` and leasing it is the follow-up that design's §7 records.) `get` on an undeclared name: `not-declared` (§5 row 1). Except a Cordis plugin: it uses Cordis's `ctx.get`, which rutis does not change, so an undeclared name gives `undefined` as in Cordis.
- `use` on a name only in `optional`: as for an undeclared name, `service llm is not available to this plugin, which does not declare it in inject: add 'llm' to its inject`, code `not-declared` (`use`'s existing line, §5 row 2), as Rust's `ctx.require` says `Undeclared` for an `Option<Arc<T>>`. Optional services are read with `get`.
- Swift and Kotlin's typed `get`: when the service is there but lacks a method the proxy needs, it throws as `use`'s check does, starting with `get` instead of `use` (§5 row 6); when the service is absent it returns `nil` / `null` without checking methods.

### 4.6 Testing tools

Every language's testing tool (Node / Bun `@arcships/rutis/testing`, Bun under `bun test`; Python `testing`; Go `rutistest`; Swift / Kotlin `RutisTest`) runs on a real host on the desktop (`rutis-host test`, see the 2026-10-30 port in the status line; Swift's `RutisTest` on macOS only, and which desktop platforms the others support is their guides' to say): the services the test gives are provided on the host by the testing tool (in Node and Bun each a row of its own, `rutis-test/<name>`; in Go another row; in Python, Swift and Kotlin the test's own plugin), and the plugin's row loads and leases as usual. Swift's `RutisTest` on a device and in the iOS simulator differs: the test is an app that installs its own plugins (`RutisTestDevice.swift`), connected to the development machine's host but not loading the row through it; item 4's check and the leases for the row happen as the host loads a row, so not on this path.

- Names in `optional` need no service from the test; without one, `load` succeeds and `get` returns empty.
- With one, `get` finds it (`use` does not read names in `optional`, §4.5).
- `get` on an undeclared name says `not-declared`, with the runtime's text.
- In both lists (desktop): the host refuses the row as it loads it, as under `rutis-host run`; the tool throws the host's words with their prefix, ending in §5 row 4's message (`…: llm is in both inject and optional; keep it in one`).

### 4.7 `allowed_services` (#683)

An optional name not on the list: skipped in §4.4 ②; `get` returns empty; `use` and calls say the line of #683 §5 row 4. If #683 is implemented first, this rule is done with this design.

## 5. What developers see

### Plugin authors

When it works (TypeScript; the other languages differ only as §4.1 and §4.5 say):

```ts
import { definePlugin } from '@arcships/rutis'

interface Llm {
  ask(question: string): Promise<string>
}

export default definePlugin({
  inject: ['notes'],                    // Required: start when all are there; stop if one is withdrawn
  optional: ['llm'],                    // Optional: not waited for; leased in if there at start, kept until the row stops
  provides: { summary: { of: 'async' } },
  apply(ctx) {
    ctx.provide('summary', {
      async of(text: string) {
        const llm = ctx.get<Llm>('llm')
        return llm ? await llm.ask(`summarize: ${text}`) : text.slice(0, 200)
      },
    })
  },
})
```

- The host has `llm` when the row starts: `of` summarizes with it.
- The host has no `llm` then: the row is `active` all the same, and `of` returns the text cut short. When the host provides `llm` later, `of` still cuts the text short until the row restarts (unless another row of the same process leased `llm` for itself, see §4.4), and uses `llm` after.
- `llm` is withdrawn while the row runs: the row does not stop.

Go:

```go
var llm struct{ Ask func(string) (string, error) }
ok, err := ctx.Get("llm", &llm)
if err != nil { return "", err }
if !ok { return text[:min(len(text), 200)], nil }
return llm.Ask("summarize: " + text)
```

When it fails:

| # | Caused by | Output (word for word) | Code | Recorded on | Fix |
| --- | --- | --- | --- | --- | --- |
| 1 | `get('llm')` with `llm` undeclared (not for Cordis plugins, §4.5) | `service llm is not available to this plugin, which does not declare it: add 'llm' to its optional (or to its inject, if it cannot run without it)`; Go: `service llm is not available to this plugin, which does not declare it: add "llm" to its Optional (or to its Inject, if it cannot run without it)` | `not-declared`, `data` `{service}` | Thrown in the plugin's code | Add it to `optional` as it says |
| 2 | `use('llm')` with `llm` only in `optional` | `service llm is not available to this plugin, which does not declare it in inject: add 'llm' to its inject` (Go: `… in Inject: add "llm" to its Inject`) (`use`'s existing line) | `not-declared`, `data` `{service}` | Thrown in the plugin's code | Use `get` and handle the empty value; if the plugin cannot run without it, move it to `inject` |
| 3 | `llm`, leased at start from another runtime's row or from a `#[rutis::service]`/Marked service, called after its provider was withdrawn or its row stopped, with the process alive | `service llm is not available`; when the provider's process is gone, the existing `runtime <name> exited/disconnected during the call`; a hand-written `HostDispatch` keeps the old object (its host owns that) | `unavailable` / `peer-gone` | Thrown in the plugin's code | Restart the row, or depend on it with `inject` |
| 4 | `llm` in both the plugin's `inject` and `optional` | The message `llm is in both inject and optional; keep it in one`. Through `SessionResolver` / `BinaryResolver`: `<row id>: unresolved: cannot load "<module name>": llm is in both inject and optional; keep it in one` (`Loader::status()` also has two spaces and ` in <instance>`); through `DefinitionMounter`: `failed: llm is in both inject and optional; keep it in one` (no headline); through `SessionHandle::run`: `RowHandle::loaded` fails with it | The row fails | The row's status | Remove one |
| 4b | `llm` in the row's `inject` and the plugin's `optional` | `<row id>: unresolved: entry "<row id>" rejected: llm is in the row's inject and the plugin's optional; keep it in one` | The row fails | The row's status | Remove one |
| 5 | The host does not allow `llm` with `allowed_services` (#683; the example row id and runtime name are both `summarizer`) | `get` returns empty; `use` and calls: `llm is not available to summarizer: the host does not let runtime summarizer use it` | `unavailable` | Thrown in the plugin's code | The host's choice; the plugin handles it as absent |
| 6 | Swift / Kotlin typed `get`, the service lacking a method the proxy needs | Swift: `get llm: LlmProxy calls ask, which the service does not have`; Kotlin: `get llm: Llm calls ask, which the service does not have` | As `use`'s same check | Thrown in the plugin's code | Change the proxy interface, or the provider |

`use`'s existing line for an undeclared name (`… does not declare it in inject: add 'llm' to its inject`, Go `… in Inject: add "llm" to its Inject`) is unchanged.

### Those assembling plugins with `rutis-host`, host owners

When it works: a row with optional dependencies no longer shows in `waiting for …`. No new configuration. When an optional service appears after the row, restart the row to use it. Host owners meet §4.7 only when using #683's `allowed_services`.

### Concepts to learn

One for plugin authors: the `optional` list and `get`, with the semantics of Rust's `Option<Arc<T>>` (what the row sees at load is what it has while it runs). None for the other roles.

## 6. Compatibility

- All additive: plugins without `optional` behave and print as before; nothing goes into a migration guide.
- Old runtime, new host: `rows.schema` has no `optional`, taken as `[]`.
- New SDK, old host: the old host's `RowSchema` ignores the unknown field and leases no optional name; the plugin's `get` always returns empty, without an error. Each language's guide says this needs rutis-host 0.20 or later. Step 2 planned a case where `get` is empty on an old host; it was not written (it needs a 0.19 host in the test).
- Go/Swift/Kotlin's `use` does not know the row's `inject`; this design adds `optional` to the set and leaves that unchanged. (Since closed by [declared services](design-declared-services-2026-10-29.en.md): in all six languages the declared set includes the row's `inject` and the names the plugin provides itself, and `use` checks the declaration first; rutis gating a runtime's row on its row `inject` and leasing it is the follow-up that design's §7 records.)
- The behaviour of Cordis 3 / Koishi's object form together with `optional` is unverified and out of scope.
- Protocol version and control operations unchanged.

## 7. Implementation

| Step | What | Done when |
| --- | --- | --- |
| 1 rutis-bridge, rutis-loader | `RowSchema.optional`; §4.3's splitting function, used by the loader's three paths, and the same check and leases in the bridge's `RunRow`; `optional` on `JsFactory` and `JsModule`; §4.4 ②; the row's `inject` ∩ `optional` check | Rust tests with the Node leaf runtime: there at start (`get` finds it in `apply`); not there at start (the row is `active`, not in `waiting for`, no `StillWaiting` after `report_waiting_after`; still empty when it appears later, found after a restart); provided by a Rust service and withdrawn while the row runs (the row stays `active`; a hand-written `HostDispatch` reaches the old object, `#[rutis::service]`/Marked says `service <name> is not available`); provided by another runtime's row that stops while the row runs (the row stays `active`; the call says `service <name> is not available`, `peer-gone` when the process is gone); replaced (by the service's form: a hand-written `HostDispatch` keeps the old one until a restart, Marked and another runtime's row say `unavailable` — a case for each); optional leases given back after `unload_row` when the row unloads; given back when leasing or `rows.load` fails; duplicates on all three paths for the plugin's `inject`/`optional` (§5 row 4 word for word); the row's `inject` and `optional` duplicate checked before `combined_injects` (§5 row 4b); a runtime without `scopes` makes only the shared lease; `rutis-host check` prints `optional`; the Cordis runtime leases only shared names; `RowSchema.optional` is `[]` when left out |
| 2 Each language's SDK and testing tool | Node (leaf and Cordis plugins), Bun, Python, Go, Swift, Kotlin: §4.1's declaration, the schema, §4.5's `get` and `use`'s declared sets (`get` counts `optional`, `use` does not); §4.6's testing tools; each runtime's `RowService`/handle reporting `unavailable` after the provider row stops (Node's `TypeError` fixed) | Each language's own cases (not one set in rutis-bridge run by all six, as planned: once the testing tools ran on a real host, each language's cases already go through the host and the real runtime; plus the loader's `optional_deps.rs` (Node and Python runtimes) and `optional_deps_go.rs` (Go, `--features go`)): §5 rows 1, 2, 4 and 6 word for word, `get` present / absent, `use` saying `not-declared` for an optional name, the row's `inject` counting as declared only in Node/Bun/Python, Go/Swift/Kotlin unchanged (this clause since changed by [declared services](design-declared-services-2026-10-29.en.md): it counts in all six); one set per language's testing tool, each asserting §4.6's item 4 (a name in both lists refused by the host, asserting the host's line: its end in Bun, Python, Go and Kotlin, by a pattern in Node, contained in Swift; Bun through `@arcships/rutis/testing`); §4.6's items 1–3 are the `get` present / absent and §5 row 1 above, not all through the testing tool (Kotlin's item 3 asserts on `RuntimeState` directly); assertions for §4.1's non-string check and `optional` as `[]`, the Cordis form's in a `rutis-host check` case (its `optional` listed as usual, one with a non-string unable to load); no case of its own for Cordis's undeclared `get` giving `undefined` (as Cordis does; rutis does not change it) or for `get` empty on an old host (§6) |
| 3 Docs | A `ctx.get` row in the plugin API guide's table; a paragraph on optional dependencies where each language's plugin guide covers using services (including "restart to use one that came later"); `optional` in the Cordis guide; entries in `docs/guarantees.md`, each naming the test that checks it | Doc link check and doc example check pass |
| 4 #683 | §4.7 | With #683's step 2 |
