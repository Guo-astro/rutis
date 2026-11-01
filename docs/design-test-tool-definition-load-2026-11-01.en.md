# The testing tool: `load` mounts a plugin definition the plugin hands over

[中文](design-test-tool-definition-load-2026-11-01.md)

Status: draft, not implemented, with a **breaking** change (`rutis_loader::patch::Owner` gains a variant, §5). Date: 2026-11-01 (revised twice the same day. First round: how the origin generation is kept and restored, a copy's identity through service and error queries, a run-time-only row across `reconcile`, locating the origin by scope, and a fuller "what developers see". Second round: the origin becomes "one decode, bound to the generation the runtime names", removing the restart window and the R3/R6 conflict (D4, D6, D7); how a copy's identity reaches the build path (D8); `reconcile` meeting a live copy's id (D9); the instance acceptance is one row in two instances plus a position error word for word (§6); `Owner::Copy` is marked breaking and gets a migration note (§5).)

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

### 1a. Why the origin gets bound wrong

A call between a service the test provides and a plugin never goes through the host: the test's service is a row of the runtime (`node/rutis-runtime/src/testing/service.mjs`), and its methods go to the test process over the driver's control session. So the host does not see the handover at all. A `Definition` parameter of a `#[rutis::service]` reads its origin at its **first decode** (`caller_row()` at `crates/rutis-bridge/src/marked.rs:642–655`), and that first decode is the driver calling the host, so it necessarily reads **the driver's row**; `crates/rutis-bridge/src/runtime/definitions.rs:128–147` then builds the definition on the driver's generation: the copies would end with the test, not with the row of the plugin that handed them over.

Put the other way: once the first decode happens on the driver's row, everything after it has to *re-bind*, and re-binding one value to another generation breaks both R3 (one value handed over several times is one definition, cached by value) and R6 (it belongs to the row that first handed it over). So D4 and D6 are shaped so that it is never bound wrong in the first place: the call that receives the definition does not decode it, and the method body decodes it once with the generation the runtime names; from then on it is cached by value.

## 2. Decisions

| # | Decision |
| --- | --- |
| D1 | **The test-side session takes the definition.** A new session option `host` (used by the testing tool only, in `node/rutis-runtime/src/testing/start.mjs` and Bun's copy): it declares `plugins`, and keeps a `plugin` reference the far end sends as a value that can be handed straight back (D2). Dropped: taking it in every session by default — a runtime cannot use a definition another runtime made anyway, so the refusal stays, and its error stays accurate. |
| D2 | **What the double receives is opaque.** It is neither a `definePlugin` result nor a callable proxy: it carries only the marker `Symbol.for('rutis.plugin-definition')`; its fields cannot be read and it cannot be called; handed back to any service it goes out as the original reference (`home: true`), and the plugin gets its own definition back. Dropped: giving it an `apply` so the test process could mount it — a copy runs in the runtime that defined it (R5); mounting it here would be a lie. |
| D3 | **`load` takes that value.** `@arcships/rutis/testing`'s `load()` and `host().load()` take D2's value besides a `definePlugin` result: they skip the "module default export" and plugin API checks and hand the definition itself to the driver's new `loadDefinition(definition, config, options, owner)`. Dropped: a separate `loadDefinition()` function — R10 says it goes straight to `load`, and one thing should not have two ways to write it. |
| D4 | **The host entry: a parameter that does not decode the reference, plus the generation the runtime names.** `rutis-host/test` gains<br>`pub async fn load_definition(&self, plugin: PluginRef, config: Option<Value>, options: Option<Options>, owner: Option<String>) -> Result<Loaded, String>`<br>· `PluginRef` is a new parameter kind in `crates/rutis-bridge`: it holds the session's reference and **reads no declarations and builds no definition** (the macro recognises it by type name, next to `Definition`, around `crates/rutis-macros/src/lib.rs:514` and `:946`).<br>· `owner` is **the generation** the runtime names: the key of that row in the runtime (the first argument of `rows.load`, `{instance}.{generation}`), which the host itself generates in `crates/rutis-loader/src/runtime.rs` and so knows.<br>· The body calls `rutis_bridge::runtime::read_definition(process, reference, owner)` once (D6) and gets exactly the `Definition` a real service call would.<br>· `Loaded { id, provides }`: the row id, and the services the definition declares with their method kinds (already read by `plugins.read`). `Options` stays `{ id, isolate }`.<br>Dropped: a `Definition` parameter — it decodes on the driver's row, and the body could only re-bind (D6 says why that breaks R3/R6). Dropped: widening `load`'s `name: String` to `Value` and branching — a parameter that wants a name would take anything, the two entries would look alike in `rutis-host types`, and an error would no longer point at what was written wrong. Dropped: returning only the row id and letting the testing tool ask for the declarations again — without a method table `service()` cannot call the synchronous methods (rule 3 in §3). |
| D5 | **rutis-loader gets a new method, not a wider `NewEntry`.**<br>`pub async fn load_copy(&self, definition: &Definition, config: Value, options: CopyOptions, owner: &str) -> Result<(String, FiberView), LoaderError>`<br>Why: `NewEntry` is input to the **editable layer**; it is written out as a JSON row (`id`/`name`/`config`/`isolate`), and `name` is what a resolver turns into a module or a runtime. A plugin definition is a value: it cannot be written into a layer or resolved by name, and `reconcile` cannot rebuild it from one. So a copy is a **run-time-only** row (D9). Dropped: `NewEntry { entry: Module \| Definition }` — the layer would keep a row that can never be rebuilt. Dropped: letting `Harness` mount `ctx.plugin(def.at(..))` at its root itself — the copy would appear in neither `status` nor `entries()`, and unloading it would have no single entry. |
| D6 | **One decode, bound to the generation the runtime names; the protocol is unchanged.** The host reads the definition through a new branch of `rutis_bridge::runtime`: it shares everything with `read()` (the checks, `plugins.read`, `from_source`, the cache by value) and differs only in where the origin comes from — with an `owner` it is that generation, without one it is `caller_row()` as before. The generation is found through a **hook**: `rutis_bridge::runtime` gains `trait OriginResolver { fn origin(&self, key: &str) -> Option<Origin> }` (`Origin { row, ctx }`), provided by rutis-loader at its root (the same place as `PluginMounter`, `crates/rutis-loader/src/loader/plugins.rs:206`); its implementation looks the key up in the loader's running records (`key → Running`, D9 has `Running` keep the key) and hands back that generation's `Ctx` (D8's `fiber_ctx`); no hit means that generation is gone.<br>· **No window:** the key is **that generation's**, not "some row's current one"; a restart makes a new key; the lookup and the `Ctx` it returns happen once under the loader's state lock, and the `Ctx` is that generation's. A miss is refused (§4 rule 1); a hit whose generation ends right after is fine: the definition is registered on that generation's fiber and the copies end with it (R6).<br>· **R3 and R6 both hold:** a value is decoded once (`known_definition` caches by value, around `crates/rutis-bridge/src/runtime/process.rs:1150`), and at that first decode the origin is the generation that handed it over; a second `load` of the same value ignores `owner` and gets the same cached `Definition`, with the same `ended()`.<br>· Where the runtime's `owner` comes from: with row tracking on (D11), every export of the driver's session records the current row (`session.mjs` already does this for the host session through `owner()`), and the driver reads the key back with `ownerOf(value)` (`node/rutis-runtime/src/testing/rutis-test.mjs`) and passes it to `loadDefinition`.<br>Dropped: a `Definition` parameter plus re-binding the cache (the first round's shape) — a restart window lets an old value bind to a new generation, and re-binding contradicts R3/R6. Dropped: finding "the current generation" by `(row id, scope chain)` — if that row restarts between the handover and the bind, the old value binds to the new generation, and a second `load` of the same value could bind to a newer one still. Dropped: an opaque generation marker on the reference itself (a field on `plugin` references on the wire) — it works (**option A**, costs and acceptance in §6), but it changes the wire format in plugin values §5.1 and every peer holding a plugin reference must carry it back; this document chooses not to change the protocol (**option B**, this rule). |
| D7 | **The position comes from the resolved generation, and R4 is not relaxed.** The copy is mounted on the group context of the `Ctx` `OriginResolver` returns (where that generation runs); the kernel's R4 check applies as before (`load_copy` runs the same code and words as `at`, an item in `rutis::internal::definitions`). A definition whose scope is not a prefix of that position is refused with the kernel's words (§4 rule 4). Dropped: relaxing R4 to "mount anywhere" — with an origin inside an instance the copy would see another instance's services, unlike `ctx.plugin(def.at(..))`. Dropped: falling back to a root context — it breaks instance isolation just the same. |
| D8 | **A copy carries its own row id in services, errors and `status`, and that id travels the build path.** A definition row is a row: the `row` of `rutis/service`, listener descriptions, `CallerRow`, error lines and `status`'s `services:` use its own row id (§5.5). R5's "a copy's `row` is the row it came from" still holds for the copy **a plugin mounts with `ctx.plugin(def.at(..))`** (PR2 adds a sentence to R5 and to §5.5, in both languages). How the name reaches the build path:<br>· the key computation, scope computation and `JsFactory` build in `DefinitionMounter::mount` (`crates/rutis-loader/src/runtime.rs:893–960`) become `fn copy_factory(loader, at, plugin: &ForeignPlugin, config, name: Option<&str>) -> Result<Box<dyn DynPlugin>, Error>` in the same file;<br>· `DefinitionMounter::mount` (the one provided at `crates/rutis-loader/src/loader/plugins.rs:206`) calls `copy_factory(…, name = plugin.row().or(Some(plugin.name())))` — **`ctx.plugin(def.at(..))` behaves and is named exactly as before** (R5);<br>· `Loader::load_copy` calls `copy_factory(…, name = Some(the copy's row id))`, the id from `CopyOptions.id` or generated from the definition's name (`free_id`, over `desired` and `copies`); it then calls `rutis::internal::definitions::mounted(&definition, &view)` to record this copy on the definition (the kernel's R6), since it was not mounted through `Definition::at`.<br>Dropped: naming it after the origin row, as R5 says — two copies would share one name (`provided()` could not tell them apart, a service clash would name the wrong row), and `status`'s `services:` would say the origin row provides them. Dropped: making `provided()` look under the origin row — two copies of one definition would be mixed together. Dropped: letting `DefinitionMounter` read a "current name" — that is thread-crossing global state, worse than one more parameter. |
| D9 | **A copy exists only at run time, and its id shares one namespace with the rows of the layers.** Copies are kept in the loader's `State.copies` (`id → { fiber, definition, owner, config }`), not in `desired` and not in `running` (`crates/rutis-loader/src/loader/reconcile.rs:821–854` keeps only the records of `desired`, and a later lookup by `desired` has an `unwrap`); `Running` keeps its generation's key (D6). The rules:<br>· `create`/`update`/`remove`/`move`/`rename` of ordinary rows, and every `reconcile`, leave copies alone: not unloaded, not restarted, no `rows.update`.<br>· **`Loader::reconcile(layers)` meeting a live copy's id**: the layer's row fails and the copy stays. `ReconcileReport` gains a `Failure { row: <id>, error: "a row <id> is loaded already: a copy of a plugin definition has that id" }`, the other rows start as usual, and once the copy has ended the next `reconcile` starts that row. Dropped: letting the copy give way (remove it) — the copy is what a test is using, and `reconcile` must not touch it; the layer's row is a same-named new row with nothing to do with the copy.<br>· `create` with a copy's id, or `load_copy` with the id of a layer row or another copy: refused, with `create`'s words today (`a row <id> is loaded already`).<br>· `entries()` and `status` list copies: `EntryInfo.owner` is the new `Owner::Copy`, `options` is `{ id, name }`, and the state is that fiber's.<br>· Sweeping: a copy whose fiber is `Disposed` is dropped from `copies` on the next `reconcile` or `status`; a `Failed` one stays, visible in `status` as a row is, until `remove`.<br>Dropped: keeping copies in `running` behind an empty `desired` row — `reconcile`'s `unwrap`, `Slot`s, instances and config evaluation would all change, which is giving copies a fake layer. Dropped: keeping copies outside the loader — `status`, `entries()` and `remove` would have no single entry. |
| D10 | **The config is fixed when it is mounted** (§5.5). `update` is refused for a definition row, and so are `set_disabled`, `reload`, `move` and `rename` (a copy is in no layer, D9); the reports are in §4. Dropped: forwarding `update` to the runtime — a definition row has no `rows.update` in the runtime (§5.5), so it would only produce a more distant error, and R5's way to change a config is to unmount the copy and mount it again. |
| D11 | **The cost: row tracking is turned on for tests only.** D6 needs the runtime to know "which row runs now", which is `rowTracking` (today on only with `RUTIS_DEV` or `--prefix-output`, `node/rutis-runtime/src/runner.mjs:106`). The criterion is that the test driver is loaded (`RUTIS_TEST_CONTROL` in the environment), so ordinary runs pay nothing. PR2 records the measured cost (one `AsyncLocalStorage` run per plugin call, plus a promise-init hook). Dropped: leaving it on always — 0.20 is cutting CPU (#980), and ordinary runs should not pay for the testing tool. |
| D12 | **Not done:** plugin definitions in `rutis.json` or in `rutis/loader.create` (a definition is not a name that can be written into a layer and rebuilt); one row shared by `load`ing the same definition twice (each is a copy, as with `ctx.plugin(def.at(..))`); `rutis-host check` knowing definitions handed over at run time (R8 unchanged); `rutis-host types` generating a client declaration for `load_definition` (`rutis-host/test` is the host's own service, and the testing tool calls it from JavaScript); the testing tools of Python, Go, Swift and Kotlin (their doubles already receive the definition itself; nothing changes for them). |

## 3. Rules

| Rule | Behaviour |
| --- | --- |
| Entry | `load(definition, { config, services, id, isolate })` (the SDK) → the driver's `loadDefinition(definition, config, options, owner)` → `rutis-host/test`'s `load_definition(plugin: PluginRef, …, owner)` → one decode in `read_definition` → `Loader::load_copy`. `config` and `id` go down unchanged; `services` are still provided first by the testing tool as rows of the runtime, as today. |
| 1. Origin | The runtime records the generation that handed it over (`owner` is that row's key in the runtime); the host gets that generation's `Ctx` through `OriginResolver`, and refuses when there is none; when that generation ends or restarts the copies end first (R6), and an old value cannot be mounted. A value is decoded once, so a second `load` gets the same `Definition` (R3). |
| 2. Result | `Loaded { id, provides }`: the row id, and the services the definition declares with their method kinds. The testing tool's row object is the same as for a row of `load`. |
| 3. `service()` | The testing tool builds its method table from `Loaded.provides` (synchronous methods still go out as synchronous calls) and does not read `plugin.provides`, which an opaque value does not have. `methods(name)` comes from the same place. |
| 4. Waiting | As for a row of `load`: a service in the declared `inject` that is not there is waited for, and the copy starts when it is; it does not fail. |
| Row id | `options.id`, else the definition's name, with `-2`, `-3`… while taken (`free_id`, over `desired` and `copies`). |
| Name (services, errors) | The copy's own row id (D8): the `row` of `rutis/service`, error lines, `status`'s `services:`, `provided()` and `reported()`. |
| `status` / `entries()` | Listed with the rows: `id` is the row id, `name` is the definition's name (the origin row's id when it has none), `owner` is `Owner::Copy`, and the state is that fiber's (D9). |
| Unloading | `unload` / `Loader::remove` ends this copy only, as for a row: its services are withdrawn, then `rows.unload`, then the leases are released. |
| Origin ends | The origin row unloads, restarts or ends: the copies end first and are waited for, then the origin row ends; then `ended()` completes, and the copies' records go on the next sweep. |
| Runtime ends | The runtime that defined it goes: the copy ends with it (the connection is gone), and its record goes on the next sweep. |
| Edits in the layers | Edits about other rows and every `reconcile` leave copies alone and send them no `rows.update`; a layer row that takes a live copy's id fails (D9). |
| A definition that has ended | That generation has ended (or its key is gone): `read_definition` refuses (§4 rule 1); one whose generation ends between the decode and the mount is refused by `load_copy` with the same words `at` uses (§4 rule 3). |

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
| The generation that handed the definition over has ended (unloaded or restarted), then `load(definition)` | `PluginTestError: the plugin definition was handed over by a row that is no longer running: load the definition that row hands over now` | thrown by `load` | use the definition that row hands over now, or `load` it before that row unloads |
| The row that handed it over is not a row of this host (a definition an application's own plugin handed over) | `PluginTestError: part: the row index that handed the plugin definition over is not a row of this host: only a plugin loaded as a row of the host can hand one over` | thrown by `load` | hand it over from a row of the host; in an application, mount it with `ctx.plugin(def.at(..))` |
| That generation ended between the decode and the mount (rare) | `PluginTestError: part: plugin definition part has ended with its origin` | thrown by `load` | as the first line |
| The definition's scope is not a prefix of the position its generation resolves to (reachable from Rust, not from the testing tool) | `LoaderError: part: plugin definition part can only be mounted inside instance g 1 or deeper, not inside instance g 2` | returned by `Loader::load_copy` | mount it in the instance that handed it over; in an application, use `ctx.plugin(def.at(..))` |
| `update(config)` on a definition row | `PluginTestError: part: a copy of a plugin definition has the config it was loaded with: unload it and load it again` | thrown by `update` | unload the row and `load` it again with the new config |
| `move`/`rename`/`set_disabled`/`reload` on a definition row (methods of `rutis/loader`) | `part: a copy of a plugin definition is not a row of the configuration: unload it and load it again` | thrown by that call | as above |
| `isolate` given for a definition row | `PluginTestError: part: a plugin definition runs where the row that handed it over runs: load it without isolate` | thrown by `load` | leave `isolate` out |
| A config the definition's schema refuses | `PluginTestError: part: config: depth should be a number, got "deep"` | thrown by `load` | fix the config as the definition's schema says |
| `RUTIS_HOST` names a `rutis-host` older than the testing tool (0.19 or earlier, with no `load_definition`) | `PluginTestError: the test driver has no method loadDefinition` | thrown by `load` | use the same version of `@arcships/rutis-host`, or point `RUTIS_HOST` at its new binary |

The output names only what the developer wrote: row ids, the definition's name, config paths, instance chains (`instance <group> <id>`, see `crates/rutis/src/definition.rs:418`). The implementing PRs' acceptance tests compare this table word for word.

**Concepts to learn.** None: R10 already promised that a definition a double receives goes straight to `load`. A test author still only needs to know that a definition runs as a row in the runtime that defined it, and ends with the row that handed it over.

### 4.2 The person assembling plugins with `rutis-host`

`rutis.json` gains no field, and `rutis-host check` prints the same (R8). `rutis-host status` shows one more kind of row — a definition row, in `rows:` as any row, with its services under `services:` naming its own row id as the provider (D8):

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

When the origin row unloads, `part` goes with it (R6); a `part` that ran and then failed stays, showing `failed: …` as any row does. When one row hands over a definition in each of two instances, `rows:` has two rows (`part` and `part-2`), each shown in its instance (`part in <instance>`).

### 4.3 The host owner (an application embedding rutis, Rust)

```rust
// Mount a definition another language handed over as a row; owner is the
// generation that handed it over (D6, D7).
let (id, _view) = loader
    .load_copy(&definition, json!({ "depth": 2 }), CopyOptions::default(), owner)
    .await?;
// Unloading goes through the same entry as any row.
loader.remove(&id).await?;
```

In `entries()` that row's `owner` is `Owner::Copy` (D9) and its `options` is `{ id, name }`; `status()`'s `rows` has `{ id, state }`. Errors: the text in `Err(LoaderError)`/`Err(String)` is §4.1's lines without the `PluginTestError: ` prefix; neither `rutis-host/test` nor `Loader::load_copy` adds an error code. **Incompatible:** `rutis_loader::patch::Owner` gains a variant (§5).

### 4.4 Deploying

Nothing changes: no new frames, no new field in `rutis.json`, and `rutis-host run`/`dev`/`check` print what they printed.

## 5. Compatibility and migration

- **Protocol:** no new frames, and the wire format is unchanged (this document takes the option that leaves it alone; a generation marker on the reference is **option A** in §6). The test-side session declares one more capability, `plugins` (already defined by R10 §5.3; the testing tool's side simply did not declare it). A side that does not declare it behaves as before.
- **The host's interface:** `rutis-host/test` gains a method, `load_definition` (taking a `PluginRef` and `owner`). A host from 0.19 or earlier does not have it, and the new `load` fails with the last line of §4.1; needing the same version is true today (`@arcships/rutis-host` is a dev dependency of the testing tool; `RUTIS_HOST` is the user's to name).
- **The runtime package:** the definition is handed over by the runtime package (`@arcships/rutis-runtime` / `@arcships/rutis-bun`), which records its origin too. An older runtime records no origin and knows no `loadDefinition`, and fails the same way.
- **Public API:**
  - **breaking**: `rutis_loader::patch::Owner` is a public, exhaustively matchable enum and gains a variant, `Copy` (`crates/rutis-loader/src/patch.rs:62`). Code that matches it exhaustively gains an arm, `Owner::Copy` (a copy, in no layer). 0.20's "Migration to 0.20" says so (`docs/migration-0.20.md` / `.en.md`, in both languages; PR2 creates the file if it does not exist yet), and #1004 carries the `breaking` label with the migration requirement stated on the issue (#694 rule 2).
  - new, not breaking: `rutis_loader::Loader::load_copy`, `CopyOptions`; `rutis_bridge::runtime`'s `read_definition`, `OriginResolver`, the `ForeignPlugin` accessor and the `PluginRef` parameter kind; a few items in `rutis::internal` (internal, no stability promise).
  - `@arcships/rutis/testing`'s `load` takes one more kind of value, and the TypeScript declarations do not change (`Definition` has been written as TypeScript's plugin type since R9).
- **Migration:** only the `Owner` line above; nothing else changes shape. PR1 and PR2's bodies are written to this: PR1 changes no compatibility, PR2 has one Rust-side break.

## 6. Implementation

**PR1: the test side declares `plugins` and takes the definition (D1, D2).**
`node/rutis-runtime` (`src/testing/start.mjs`, `src/session.mjs`) and Bun's copies of them.
Done means: a service the test provides receives `register(definition)`, and when the double hands it straight back the plugin gets its own definition; one test each in `node/rutis/test` and `bun/rutis-bun/test`; `load` behaves as before.
**PR1 does not close #1004** (Dim's code still cannot run); PR2 does (`Closes #1004` when it merges).

**PR2: `load` mounts a definition, per D3–D11.**
`crates/rutis` (a few items in `internal`: the scope and the check `at` runs in `definitions`, a `FiberView`'s `Ctx`, `definitions::mounted`), `crates/rutis-bridge` (the `PluginRef` parameter kind, `read_definition`, `OriginResolver`, `ForeignPlugin`/`foreign()`), `crates/rutis-loader` (`Loader::load_copy`, `CopyOptions`, `copy_factory`, `State.copies`, `Running` keeping its generation's key, the `OriginResolver` implementation, `entries`/`get`/`remove`/`status`/`reconcile` knowing copies, `Owner::Copy`), `crates/rutis-host` (`load_definition`), `node/rutis-runtime` (the driver's `loadDefinition`, `ownerOf`, the row-tracking switch; `within` in `leaf.mjs`), `node/rutis` (`load` taking the definition, `service()` using `Loaded.provides`), Bun's copies, and `docs/migration-0.20.md`/`.en.md`.

Done means, one line per review point:

1. **Origin:** `owner` is that generation's key; the host refuses a missing one and binds to the `Ctx` of the one it finds. Tests: `load` after the origin row unloads reports §4.1's first line; after the origin row restarts, the new generation's definition loads and the old value reports the same line; `load`ing one value twice yields one `Definition` (`ended()` too, R3); when the origin row unloads, the copies end first and their cleanups are waited for before it does (R6).
2. **Identity:** when a definition row provides `echo`, `status`'s `services:` says `echo: part`, `provided()` lists `echo`, and `reported()` gets that row's failure; two copies of one definition have their own ids and their own `provided()`; a copy a plugin mounts with `ctx.plugin(def.at(..))` is still named after the origin row, as R5 says (the existing `rutis-loader/tests/plugin_values.rs` tests guard it, unchanged).
3. **Across `reconcile`:** after an unrelated `create`/`update`/`remove` and a `reconcile`, the copy's `PluginId` is unchanged and the runtime received no `rows.update` for it; a layer row that takes a live copy's id shows up as one `Failure` in `ReconcileReport` (D9's words) while the copy stays and the other rows start, and after the copy ends the next `reconcile` starts that row; `create` with a copy's id is refused; `remove(copy id)` ends it.
4. **Instances:** an instanced group `g` with two instances has **the same row id** `index` hand over a definition in instance 1 and in instance 2, giving `part` and `part-2`, each mounted in its own instance (the same service name isolated per instance, each with its own `provided()`); `Loader::load_copy` given an `owner` unrelated to the definition's scope reports, word for word, `part: plugin definition part can only be mounted inside instance g 1 or deeper, not inside instance g 2` (R4 is the same code `ctx.plugin(def.at(..))` runs).
5. **Migration and docs:** `Owner` is marked breaking, `docs/migration-0.20.md`/`.en.md` says so (PR2 creates the file), PR2 comments the migration requirement on #1004 and adds the issue's `breaking` label; the sentence in `docs/guide/plugin-api.md`/`.en.md` about a double's definition going straight to `load` gains Node and Bun; a sentence each in R5 and §5.5 of the plugin values design (D8); the `loader/plugin-value-copies` entry in `docs/guarantees.md`/`.en.md` gains a paragraph for "a definition as a row"; one line each in the release notes.

**D6's two ways (if the review prefers the other one, it replaces the reading of D4 and D6 whole)**

| | Option A: the generation on the reference | Option B (this document): the generation in an argument |
| --- | --- | --- |
| How | the `plugin` reference's wire format gains a field (the origin generation), set by the runtime when it hands it over; the host binds by it in the **single first decode**; a peer holding the reference sends it back as it came | `load_definition`'s parameter does not decode the definition, and the body decodes it once with `owner` (D4, D6) |
| Cost | a change to the wire format in plugin values §5.1: all six SDKs' reference parsing must accept the field, and the Node/Bun sessions must carry it back on the way home; once on the wire it is a compatibility burden | one new parameter kind, `PluginRef` (bridge + macro), which `rutis-host types` does not generate (D12); the protocol is untouched |
| Acceptance | §6 rule 1; plus: a reference carrying a generation marker, handed back through the test process, binds to that generation | §6 rule 1 |
| Dropped alternative | no marker and relying on `caller_row()` — the mis-binding of §1a | a `Definition` parameter and re-binding — the restart window and the R3/R6 conflict |

Also: D11's measurement (row tracking on in tests, the cost of one plugin call) goes in PR2's description; `cargo fmt`, `cargo test -p rutis-loader -p rutis-bridge -p rutis-host`, `npm --prefix node/rutis test` and `(cd bun/rutis-bun && bun test)` run as CONTRIBUTING says for the parts it touches.
