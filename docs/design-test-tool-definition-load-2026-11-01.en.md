# The testing tool: `load` mounts a plugin definition the plugin hands over

[中文](design-test-tool-definition-load-2026-11-01.md)

Status: draft, not implemented. Date: 2026-11-01 (revised the same day for the five review points: how the origin generation is kept and restored (§2 D6, D7), how a copy's identity reaches service and error queries (D8), how a run-time-only row behaves across `reconcile` (D9), and locating the origin by scope (D7); §4 now covers `move`/`rename`, a refused position, the errors a host gets, the `status`/`entries` output word for word, and the host-owner role; D10 gained its dropped alternative.)

Issue: [#1004](https://github.com/arcships/rutis/issues/1004) (the TypeScript testing tool: the test process never receives plugin definitions). Its PR1 and PR2 (§6) implement this document. The rules it builds on are R10 and §5.5 of [a plugin is a value](design-plugin-values-2026-10-25.en.md); this document **narrows** the sentence in R5 that names copies after the row that handed them over (D8).

## 1. Where things stand

The TypeScript and Bun testing tool runs the plugin under test in **another process** started by `rutis-host test`, in a real runtime; the test process is the far end (`node/rutis-runtime/src/testing/start.mjs`). When a plugin hands a plugin definition to a service the test provides (Dim's `partRegistry.registerDefinition('session', definition)`), the argument does not cross the wire:

```
registrar: TypeError: parts.register: argument 1 cannot be sent: cannot send a plugin definition: the far end does not accept plugin definitions (it did not declare the capability plugins)
```

The session on the testing tool's side never declared `plugins`. Declaring it is not enough: the test process's session refuses every `plugin` reference it receives (`node/rutis-runtime/src/session.mjs:451`, the same in Bun's `bun/rutis-bun/src/session.ts:443`), and the error only becomes

```
received a plugin definition made in another process, which cannot be used here; pass plain data or a function instead
```

`load` also takes only values made by `definePlugin` in this process (`node/rutis/src/testing.mjs:222`), so the value the double holds cannot be mounted either.

The testing tools of Python, Go, Swift and Kotlin do not have this problem: their runtime is in the test process itself (`rutis-host test --channel`), so a double receives the definition itself. The Node/Bun runtime is another process, so the most a double can hold is "the definition, over there", and mounting it means handing it back to the runtime that defined it.

R10 already states the behaviour; what is missing is the Node/Bun part of it: a definition a double receives can go straight to the testing tool's `load`, as with a real host.

### 1a. Why the first decode in Rust binds the wrong origin

A call between a service the test provides and a plugin never goes through the host: the test's service is a row of the runtime (`node/rutis-runtime/src/testing/service.mjs`), and its methods go to the test process over the driver's control session. So the host does not see the handover at all; the first time it reads the definition is when the driver calls `load_definition(definition, …)`. That is where a `Definition` parameter of a `#[rutis::service]` reads its origin (`caller_row()` at `crates/rutis-bridge/src/marked.rs:642–655`), and it reads **the driver's row**; `crates/rutis-bridge/src/runtime/definitions.rs:128–147` then builds the definition on the driver's generation: the copies would end with the test, not with the row of the plugin that handed them over. D6 and D7 answer this.

## 2. Decisions

| # | Decision |
| --- | --- |
| D1 | **The test-side session takes the definition.** A new session option `host` (used by the testing tool only, in `node/rutis-runtime/src/testing/start.mjs` and Bun's copy): it declares `plugins`, and keeps a `plugin` reference the far end sends as a value that can be handed straight back (D2). Dropped: taking it in every session by default — a runtime cannot use a definition another runtime made anyway, so the refusal stays, and its error stays accurate. |
| D2 | **What the double receives is opaque.** It is neither a `definePlugin` result nor a callable proxy: it carries only the marker `Symbol.for('rutis.plugin-definition')`; its fields cannot be read and it cannot be called; handed back to any service it goes out as the original reference (`home: true`), and the plugin gets its own definition back. Dropped: giving it an `apply` so the test process could mount it — a copy runs in the runtime that defined it (R5); mounting it here would be a lie. |
| D3 | **`load` takes that value.** `@arcships/rutis/testing`'s `load()` and `host().load()` take D2's value besides a `definePlugin` result: they skip the "module default export" and plugin API checks and hand the definition itself to the driver's new `loadDefinition(definition, config, options)`. Dropped: a separate `loadDefinition()` function — R10 says it goes straight to `load`, and one thing should not have two ways to write it. |
| D4 | **The host gets a new method; `load` is unchanged.** `rutis-host/test` gains<br>`pub async fn load_definition(&self, plugin: Definition, config: Option<Value>, options: Option<Options>, owner: Option<Owner>) -> Result<Loaded, String>`<br>where `Loaded { id, provides }` (`provides` is the services the definition declares and whether each method is `sync` or `async`, already read by `plugins.read`), `Owner { row: String, scope: Vec<Scope> }` is D6/D7's, and `Options` stays `{ id, isolate }`. Dropped: widening `load`'s `name: String` to `Value` and branching — a parameter that wants a name would take anything, the two entries would look alike in `rutis-host types`, and an error would no longer point at what was written wrong. Dropped: returning only the row id and letting the testing tool ask for the declarations again — without a method table `service()` cannot call the synchronous methods (rule 3 in §3). |
| D5 | **rutis-loader gets a new method, not a wider `NewEntry`.**<br>`pub async fn load_copy(&self, definition: &Definition, config: Value, options: CopyOptions, owner: Owner) -> Result<(String, FiberView), LoaderError>`<br>Why: `NewEntry` is input to the **editable layer**; it is written out as a JSON row (`id`/`name`/`config`/`isolate`), and `name` is what a resolver turns into a module or a runtime. A plugin definition is a value: it cannot be written into a layer or resolved by name, and `reconcile` cannot rebuild it from one. So a copy is a **run-time-only** row (D9). Dropped: `NewEntry { entry: Module \| Definition }` — the layer would keep a row that can never be rebuilt. Dropped: letting `Harness` mount `ctx.plugin(def.at(..))` at its root itself — the copy would appear in neither `status` nor `entries()`, and unloading it would have no single entry. |
| D6 | **The runtime records the origin generation, hands it over with `load`, and the host binds to it again.** When a plugin hands a definition over, the runtime knows which row did it: it keeps that row's runtime key (`{instance}.{generation}`) together with the loader row id and the scope chain `rows.load` gave it (`place`) on the exported reference; when the test hands the definition back, the driver passes them as `load_definition(…, owner)` and the host rebuilds the definition on **that row's current generation** (D7). Three parts make D6:<br>· the runtime: row tracking (`rowTracking` in `node/rutis-runtime/src/unhandled.mjs`) is on while the test driver is loaded (other runs leave it off; the cost is D11), a leaf plugin's `apply` and its listeners run inside `tracking.within(row, …)` (`toCordis` in `node/rutis-runtime/src/leaf.mjs`), and the driver's session carries `owner: tracking.running` (which `session.mjs` already uses, an option of `Process.connect`), so every plugin definition it exports records its generation; the driver reads it back with `ownerOf(value)`.<br>· the runtime: in `loadDefinition` the driver first asks the runtime whether that generation still runs (`running(key)`); if not, it refuses (§4), so a value handed over before a restart cannot be mounted.<br>· the host: the `Definition` parameter was already decoded on the driver's generation (§1a), and the method body binds it again by `owner`: `rutis_bridge::runtime` gains a read with an explicit origin (which replaces the entry cached for that value; the declarations read earlier are kept), and the origin is that row's **current generation** (the `FiberView` in the `Loader` → its `Ctx` through an internal accessor, `CallerRow::new(row id, key, ctx)`); `Definition::from_source` then registers the definition on that generation's fiber, and R6 holds: the copies end first and are waited for, then the origin row ends, then `ended()` completes, and the old value cannot be mounted.<br>Dropped: not binding the origin, so copies end with the test — R6 would not hold, and Dim's "unload the copies before the owner" would only be the test's own convention. Dropped: routing the handover through the host (the test's services provided by the host) so that `caller_row()` reads the owner directly — a `plugin` reference cannot be relayed (§5.2), so the host cannot forward a generic service call along with the definition to the driver, and #988 is what moved the test's services to rows of the runtime. Dropped: asking the test to write `load(definition, { handedOverBy: 'index' })` — R10 says the definition goes straight to `load`; it stays as the fallback if D6's cost is unacceptable, and §6 measures it. **D6 is the only place this document changes the runtime's structure: if the review prefers the explicit origin from the test, D6 becomes that fallback, and §6's acceptance is the same.** |
| D7 | **The origin generation is located by scope, not by a row id string.** In `Owner { row, scope }`, `scope` is the instance chain `rows.load` gave the runtime: when one row id runs in several instances the chains differ (D6's acceptance covers two instances), and an origin an application marked with its own `mark_scope` has no row in the loader, so its `scope` matches nothing. The host finds that row's current `Running` record by `(row, scope)` and mounts the copy with it: the position is the group context inside that row's instance chain, and the kernel's R4 check still applies (D5's `load_copy` uses the same check and the same words as `at`), never relaxed. No matching record is refused (§4). Dropped: finding it by row id alone — with one row running in several instances it finds the wrong instance, the copy mounts inside another instance's scope, and the isolation of instances stops holding. Dropped: relaxing R4 to "mount anywhere" — with an origin inside an instance the copy would see another instance's services, unlike the copy a plugin mounts with `ctx.plugin(def.at(..))`. |
| D8 | **A copy carries its own row id in services, errors and `status`.** A definition row is a row: the `row` of `rutis/service`, listener descriptions, `CallerRow`, error lines and `status`'s `services:` use its own row id (§5.5: "this copy is a row in the runtime, and `rows.unload` and `rows.ended` are as they were"). R5's sentence "a copy's `row` is the row it came from" still holds for the copy **a plugin mounts with `ctx.plugin(def.at(..))`**; a definition row is not that copy (PR2 adds a sentence to R5 and to §5.5 of the plugin values design, in both languages). Dropped: naming it after the origin row, as R5 says — two copies would share one name (`provided()` could not tell them apart, a service clash would name the wrong row), and `status`'s `services:` would say the origin row provides them. Dropped: making `provided()` look under the origin row — two copies of one definition would be mixed together, which is what this is meant to avoid. |
| D9 | **A copy exists only at run time and no `reconcile` touches it.** Copies are kept in the loader's `State.copies` (`id → { fiber, definition, owner, config }`), not in `desired` and not in `running` (`crates/rutis-loader/src/loader/reconcile.rs:821–854` keeps only the records of `desired`, and a later lookup by `desired` has an `unwrap` a copy in `running` would trip). The rules:<br>· `create`/`update`/`remove`/`move`/`rename` of ordinary rows, and every `reconcile`, leave copies alone: not unloaded, not restarted, no `rows.update` sent (a copy's config is fixed when it is mounted, D10).<br>· Re-keying: none. A copy changes no identity with the layers: it has no `name` to resolve again, its config does not change, and its instance comes from D7's scope.<br>· Ids share one namespace with rows: `free_id`, `Loader::get` and `create`'s duplicate-id check all count `copies`; an `options.id` that is already a row or a copy is refused with `create`'s words today (`a row <id> is loaded already`).<br>· `entries()` and `status` list copies: `EntryInfo.owner` is a new `Owner::Copy` (a variant on the public enum at `crates/rutis-loader/src/patch.rs:62`), `options` is `{ id, name }`, and the state is that fiber's.<br>· Sweeping: a copy whose fiber is `Disposed` is dropped from `copies` on the next `reconcile` or `status` (the origin row ended, the runtime went, or a host unloaded it); a `Failed` one stays, visible in `status` as a row is, until `remove`.<br>Dropped: keeping copies in `running` behind an empty `desired` row — `reconcile`'s `unwrap`, `Slot`s, instances and config evaluation would all have to change, which is giving copies a fake layer. Dropped: keeping copies outside the loader (a host holding the `FiberView` itself) — `status`, `entries()` and `remove` would have no single entry, and `rutis-host status` would not show it. |
| D10 | **The config is fixed when it is mounted** (§5.5). `update` is refused for a definition row, and so are `set_disabled`, `reload`, `move` and `rename` (a copy is in no layer); the reports are in §4. Dropped: forwarding `update` to the runtime — a definition row has no `rows.update` in the runtime (§5.5), so it would only produce a more distant error, and R5's way to change a config is to unmount the copy and mount it again. |
| D11 | **The cost: row tracking is turned on for tests only.** D6 needs the runtime to know "which row runs now", which is `rowTracking` (today on only with `RUTIS_DEV` or `--prefix-output`, `node/rutis-runtime/src/runner.mjs:106`). The criterion is that the test driver is loaded (`RUTIS_TEST_CONTROL` in the environment), so ordinary runs pay nothing. PR2 records that switch's measured cost (one `AsyncLocalStorage` run per plugin call, plus a promise-init hook); over budget, D6's fallback (the test names the origin) takes over. |
| D12 | **Not done:** plugin definitions in `rutis.json` or in `rutis/loader.create` (a definition is not a name that can be written into a layer and rebuilt); one row shared by `load`ing the same definition twice (each is a copy, as with `ctx.plugin(def.at(..))`); `rutis-host check` knowing definitions handed over at run time (R8 unchanged); `rutis-host types` generating a client declaration for `load_definition` (`rutis-host/test` is the host's own service, and the testing tool calls it from JavaScript); the testing tools of Python, Go, Swift and Kotlin (their doubles already receive the definition itself; nothing changes for them). |

## 3. Rules

| Rule | Behaviour |
| --- | --- |
| Entry | `load(definition, { config, services, id, isolate })` (the SDK) → the driver's `loadDefinition` → `rutis-host/test`'s `load_definition` → `Loader::load_copy`. `config` and `id` go down unchanged; `services` are still provided first by the testing tool as rows of the runtime, as today. |
| 1. Origin | The origin generation is recorded by the runtime on the exported reference and handed to the host by the driver as `owner { row, scope }` (D6, D7); a generation the host cannot find is refused; when the origin row ends or restarts, the copies end first (R6), and an old value cannot be mounted (D6's runtime check). |
| 2. Result | `Loaded { id, provides }`: the row id, and the services the definition declares with their method kinds. The testing tool's row object is the same as for a row of `load`. |
| 3. `service()` | The testing tool builds its method table from `Loaded.provides` (synchronous methods still go out as synchronous calls) and no longer reads `plugin.provides`, which an opaque value does not have. `methods(name)` comes from the same place. |
| 4. Waiting | As for a row of `load`: a service in the declared `inject` that is not there is waited for, and the copy starts when it is; it does not fail. |
| Row id | `options.id`, else the definition's name, with `-2`, `-3`… while taken — the same rule `load` follows for a name, in a namespace that includes copies (D9). |
| Name (services, errors) | The copy's own row id (D8): the `row` of `rutis/service`, error lines, `status`'s `services:`, `provided()` and `reported()`. |
| `status` / `entries()` | Listed with the rows: `id` is the row id, `name` is the definition's name (the origin row's id when it has none), `owner` is `Owner::Copy`, and the state is that fiber's (D9). |
| Unloading | `unload` / `Loader::remove` ends this copy only, as for a row: its services are withdrawn, then `rows.unload`, then the leases are released. |
| Origin ends | The origin row unloads, restarts or ends: the copies end first and are waited for, then the origin row ends; then `ended()` completes, and the copies' records go on the next sweep. |
| Runtime ends | The runtime that defined it goes: the copy ends with it (the connection is gone), and its record goes on the next sweep. |
| Edits in the layers | `create`/`update`/`remove`/`move`/`rename` of other rows and every `reconcile` leave copies alone and send them no `rows.update` (D9). |
| A definition that has ended | Its origin has ended, or it is handed back after it did: the runtime refuses (§4 rule 1); one whose origin ended in between is refused by `load_copy` with the same words `at` uses (§4 rule 3). |

Across languages: these rules apply only to the Node and Bun testing tools and their host entry; the other languages' testing tools are untouched (D12).

## 4. What developers see

Three roles are affected: the plugin author writing the test; the person assembling plugins with `rutis-host`; the host owner (an application embedding rutis). Deploying changes nothing.

### 4.1 The plugin author writing the test (TypeScript; Bun differs only in the runtime package's name)

**When it works.** The plugin under test hands a definition to the test's double, and the double hands it to `load`:

```ts
import { load } from '@arcships/rutis/testing'

const parts = []
const owner = await load(ownerPlugin, {
  services: {
    partRegistry: {
      async registerDefinition(scope, name, definition) { parts.push(definition) },
    },
  },
})
const session = await load(parts[0], { config: { depth: 2 }, services: { history: fakeHistory } })
await session.service('gitStatus').refresh()
await session.unload()
await owner.unload()
```

A passing test prints nothing. `parts[0]` is the definition the plugin made in its `apply`, mounted in the runtime that defined it, as `rutis-host` mounts it: its config is checked against the definition's schema and filled with the defaults, it starts once the services in `inject` are all there, and it ends with the row that handed it over (that copy ends first, and then that row).

**When it fails.** One line each: how it is caused, the output word for word, where it is thrown, how to fix it.

| Cause | Output (word for word) | Where | How to fix |
| --- | --- | --- | --- |
| The row that handed the definition over has unloaded or restarted (an old value), then `load(definition)` | `PluginTestError: the plugin definition was handed over by a row that is no longer running: load the definition that row hands over now` | thrown by `load` | use the definition that row hands over now, or `load` it before that row unloads |
| The row that handed it over is not a row of this host (a definition an application's own plugin handed over) | `PluginTestError: part: the row index that handed the plugin definition over is not a row of this host: only a plugin loaded as a row of the host can hand one over` | thrown by `load` | hand it over from a row of the host; in an application, mount it with `ctx.plugin(def.at(..))` |
| The origin row ended between the runtime's check and the load (rare) | `PluginTestError: part: plugin definition part has ended with its origin` | thrown by `load` | as the first line |
| The definition's instance chain is not the instance the origin row runs in | `PluginTestError: part: plugin definition part can only be mounted inside <origin instance> or deeper, not inside <here>` | thrown by `load` | `load` it from a test in the instance that handed it over |
| `update(config)` on a definition row | `PluginTestError: part: a copy of a plugin definition has the config it was loaded with: unload it and load it again` | thrown by `update` | unload the row and `load` it again with the new config |
| `move`/`rename`/`set_disabled`/`reload` on a definition row (methods of `rutis/loader`) | `part: a copy of a plugin definition is not a row of the configuration: unload it and load it again` | thrown by that call | as above |
| `isolate` given for a definition row | `PluginTestError: part: a plugin definition runs where the row that handed it over runs: load it without isolate` | thrown by `load` | leave `isolate` out |
| A config the definition's schema refuses | `PluginTestError: part: config: depth should be a number, got "deep"` | thrown by `load` | fix the config as the definition's schema says |
| `RUTIS_HOST` names a `rutis-host` older than the testing tool (0.19 or earlier, with no `load_definition`) | `PluginTestError: the test driver has no method loadDefinition` | thrown by `load` | use the same version of `@arcships/rutis-host`, or point `RUTIS_HOST` at its new binary |

The output names only what the developer wrote: row ids, the definition's name, config paths, instance chains. The implementing PRs' acceptance tests compare this table word for word.

**Concepts to learn.** None: R10 already promised that a definition a double receives goes straight to `load`. A test author still only needs to know that a definition runs as a row in the runtime that defined it, and ends with the row that handed it over.

### 4.2 The person assembling plugins with `rutis-host`

`rutis.json` gains no field, and `rutis-host check` prints the same (R8: it does not know definitions handed over at run time). `rutis-host status` shows one more kind of row — a definition row, in `rows:` as any row, with its services under `services:` naming its own row id as the provider (D8):

```text
rows:
  index: running
  part: running
services:
  echo: part
  scheduleJobs: index
runtimes:
  none
calls:
  none
```

When the origin row unloads, `part` goes with it (R6); a `part` that ran and then failed stays, showing `failed: …` as any row does.

### 4.3 The host owner (an application embedding rutis, Rust)

```rust
// Mount a definition another language handed over as a row; the origin is
// the row that handed it over (D6, D7).
let (id, _view) = loader
    .load_copy(&definition, json!({ "depth": 2 }), CopyOptions::default(), owner)
    .await?;
// Unloading goes through the same entry as any row.
loader.remove(&id).await?;
```

In `entries()` that row's `owner` is `Owner::Copy` (D9) and its `options` is `{ id, name }`; `status()`'s `rows` has `{ id, state }`. Errors: the text in `Err(LoaderError)`/`Err(String)` is §4.1's line without the `PluginTestError: ` prefix; neither `rutis-host/test` nor `Loader::load_copy` adds an error code, as `load` does not. Incompatible: `rutis_loader::patch::Owner` gains a variant (§5).

### 4.4 Deploying

Nothing changes: no new frames, no new field in `rutis.json`, and `rutis-host run`/`dev`/`check` print what they printed.

## 5. Compatibility and migration

- **Protocol:** no new frames, no change to the handshake's shape. The test-side session declares one more capability, `plugins` (already defined by R10 §5.3; the testing tool's side simply did not declare it). A side that does not declare it behaves as before.
- **The host's interface:** `rutis-host/test` gains a method, `load_definition` (with `owner`). A host from 0.19 or earlier does not have it, and the new `load` fails with the last line of §4.1. Needing the same version is true today (`@arcships/rutis-host` is a dev dependency of the testing tool; `RUTIS_HOST` is the user's to name).
- **The runtime package:** the definition is handed over by the runtime package (`@arcships/rutis-runtime` / `@arcships/rutis-bun`), which records its origin too. An older runtime records no origin and knows no `loadDefinition`, and fails the same way.
- **Public API:** `@arcships/rutis/testing`'s `load` takes one more kind of value (the TypeScript declarations do not change: `Definition` has been written as TypeScript's plugin type since R9); `rutis_loader::Loader::load_copy` and `CopyOptions` are new; `rutis_loader::patch::Owner` gains a variant (the enum is not `#[non_exhaustive]`, so exhaustive matches gain an arm; the callers in this repository are updated in PR2); `rutis_bridge::runtime` gains a function that reads `ForeignPlugin` out of a `Definition` and a read with an explicit origin; `rutis::internal::definitions` gains a few items (internal, no stability promise).
- **Incompatible changes:** only the new `Owner` variant — visible to the compiler, no behaviour change; the 0.20 release notes say so. No migration guide.
- **Docs:** a sentence each in R5 and §5.5 of the plugin values design (D8 narrows R5's scope; a definition row is named by its own row id), in both languages.

## 6. Implementation

**PR1: the test side declares `plugins` and takes the definition (D1, D2).**
`node/rutis-runtime` (`src/testing/start.mjs`, `src/session.mjs`) and Bun's copies of them.
Done means: a service the test provides receives `register(definition)`, and when the double hands it straight back the plugin gets its own definition; one test each in `node/rutis/test` and `bun/rutis-bun/test`; `load` behaves as before.
**PR1 does not close #1004** (Dim's code still cannot run); PR2 does (`Closes #1004` when it merges).

**PR2: `load` mounts a definition, per D3–D11.**
`crates/rutis-loader` (`Loader::load_copy`, `CopyOptions`, `State.copies`, `entries`/`get`/`remove`/`status` knowing copies, `Owner::Copy`, taking the current generation's `Ctx` from a `FiberView`), `crates/rutis` (a few items in `internal`: the scope and the check `at` runs in `definitions`, and a `FiberView`'s `Ctx`), `crates/rutis-bridge` (`Definition` → `ForeignPlugin`, a read with an explicit origin), `crates/rutis-host` (`load_definition`), `node/rutis-runtime` (the driver's `loadDefinition`, `ownerOf`, `running`, the row-tracking switch; `within` in `leaf.mjs`), `node/rutis` (`load` taking the definition, `service()` using `Loaded.provides`), and Bun's copies.

Done means, one line per review point:

1. **Origin:** when the origin row unloads, the copies end first and their cleanups are waited for before the origin row's unload completes; then `ended()` completes and `load`ing the old value reports §4.1's first line; after the origin row restarts, the definition its new generation hands over loads, and the old value does not. Tests: `Loader` unit tests (the copies end with the origin, in that order) and one in `node/rutis/test` (the owner unloads → the copy ends, the old value reports).
2. **Identity:** when a definition row provides `echo`, `status`'s `services:` says `echo: part`, `provided()` lists `echo`, and `reported()` on that row gets a failing listener of it; two copies of one definition have their own ids and their own `provided()`. Tests: two in `node/rutis/test` (`provided`/`reported`; two copies).
3. **Across `reconcile`:** after an unrelated `create`/`update`/`remove` and a `reconcile`, the copy's fiber is the same (`PluginId` unchanged) and the runtime received no `rows.update` for it; `create` with a copy's id is refused; `Loader::remove(copy id)` ends it. Tests: a set of unit tests under `rutis-loader/tests/`.
4. **Instances:** two rows in an instanced group each hand over a definition; each copy mounts in its own instance and services stay isolated per instance (the same name in two instances is each one's own); mounting a definition at another instance's position reports §4.1's position line; R4's check is the same code `ctx.plugin(def.at(..))` runs. Tests: a two-instance set added to `rutis-loader/tests/plugin_values.rs`.
5. **Docs:** the sentence in `docs/guide/plugin-api.md`/`.en.md` about a double's definition going straight to `load` gains Node and Bun; a sentence each in R5 and §5.5 of the plugin values design (D8); the `loader/plugin-value-copies` entry in `docs/guarantees.md`/`.en.md` gains a paragraph for "a definition as a row", naming this PR's tests; one line each in the release notes (the new `Owner` variant).

Also: D11's measurement (row tracking on in tests, the cost of one plugin call) goes in PR2's description; `cargo fmt`, `cargo test -p rutis-loader -p rutis-bridge`, `npm --prefix node/rutis test` and `(cd bun/rutis-bun && bun test)` run as CONTRIBUTING says for the parts it touches.
