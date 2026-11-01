# The testing tool: `load` mounts a plugin definition the plugin hands over

[中文](design-test-tool-definition-load-2026-11-01.md)

Status: draft, not implemented. Date: 2026-11-01

Issue: [#1004](https://github.com/arcships/rutis/issues/1004) (the TypeScript testing tool: the test process never receives plugin definitions). Its PR1 and PR2 (§6) implement this document. The rules it builds on are R10 and §5.5 of [a plugin is a value](design-plugin-values-2026-10-25.en.md).

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

## 2. Decisions

| # | Decision |
| --- | --- |
| D1 | **The test-side session takes the definition.** A new session option `host` (used by the testing tool only, in `node/rutis-runtime/src/testing/start.mjs` and Bun's copy): it declares `plugins`, and keeps a `plugin` reference the far end sends as a value that can be handed straight back (D2). Dropped: taking it in every session by default — a runtime cannot use a definition another runtime made anyway, so the refusal stays, and its error stays accurate. |
| D2 | **What the double receives is opaque.** It is neither a `definePlugin` result nor a callable proxy: it carries only the marker `Symbol.for('rutis.plugin-definition')`; its fields cannot be read and it cannot be called; handed back to any service it goes out as the original reference (`home: true`), and the plugin gets its own definition back. Dropped: giving it an `apply` so the test process could mount it — a copy runs in the runtime that defined it (R5); mounting it here would be a lie. |
| D3 | **`load` takes that value.** `@arcships/rutis/testing`'s `load()` and `host().load()` take D2's value besides a `definePlugin` result: they skip the "module default export" and plugin API checks and hand the definition itself to the driver's new `loadDefinition(definition, config, options)`. Dropped: a separate `loadDefinition()` function — R10 says it goes straight to `load`, and one thing should not have two ways to write it. |
| D4 | **The host gets a new method; `load` is unchanged.** `rutis-host/test` gains<br>`pub async fn load_definition(&self, plugin: Definition, config: Option<Value>, options: Option<Options>) -> Result<String, String>`<br>`Definition` is a parameter type `#[rutis::service]` takes (R9); `Options` stays `{ id, isolate }`. Dropped: widening `load`'s `name: String` to `Value` and branching — a parameter that wants a name would take anything, the two entries would look alike in `rutis-host types`, and an error would no longer point at what was written wrong. |
| D5 | **rutis-loader gets a new method, not a wider `NewEntry`.**<br>`pub async fn load_copy(&self, definition: &Definition, config: Value, options: CopyOptions) -> Result<(String, FiberView), LoaderError>`<br>Why: `NewEntry` is input to the **editable layer**; it is written out as a JSON row (`id`/`name`/`config`/`isolate`), and `name` is what a resolver turns into a module or a runtime. A plugin definition is a value: it cannot be written into a layer or resolved by name, and `reconcile` cannot rebuild it from one. So a copy is a **run-time-only** row: it is in the running records, appears in `entries()` and `status`, can be removed, and is in no layer. Dropped: `NewEntry { entry: Module \| Definition }` — the layer would keep a row that can never be rebuilt. Dropped: letting `Harness` mount `ctx.plugin(def.at(..))` at its root itself — the copy would appear in neither `status` nor `entries()`, and unloading it would have no single entry. |
| D6 | **A copy is a copy the kernel knows.** `load_copy` mounts through `ctx.plugin(definition.at(&ctx, config)?)`: the kernel records it as one copy of the definition (R6), so when the origin row unloads, restarts or ends, the copies end first and are waited for, then the origin row ends, and then `ended()` completes; when the runtime that defined it goes, the copy ends with it. The row in the runtime still comes from the existing `DefinitionMounter` (`crates/rutis-loader/src/loader/plugins.rs:206` → the `JsFactory` at `crates/rutis-loader/src/runtime.rs:945`; `RowEntry::Definition`; `rows.load`'s entry is the definition itself, §5.5), and `rows.unload` is unchanged. Dropped: keeping a copy table of our own in the loader and ordering the unloads ourselves — the kernel already does it by R6, and a copy of that only adds a second set of rules. |
| D7 | **Where it is mounted: the group of the row that handed it over.** The copy is mounted on the group context of the row that handed the definition over (found by `ForeignPlugin::row()` in the running records); a definition that belongs to the process (no origin row) is mounted on the loader's root group. The kernel's position check (R4) still applies, and its words are returned as they are when it fails. Dropped: always mounting at the root — when the origin row is inside an instance, `at` refuses ("can only be mounted inside … or deeper"). |
| D8 | **The row id and the name in events are two things.** The row id is `options.id`, else the definition's name (the origin row's id when it has none), with `-2`, `-3`… while taken, as `load` gives a name an id; the id is the handle a host lists, unloads and shows in `status`. In `rutis/service` events, listener descriptions, `CallerRow` and errors, rutis still names this copy after **the row that handed it over** (R5), and after the definition's name for one that belongs to the process. Dropped: naming the copy by its own row id in events — R5 names copies after the origin row, and changing that would give two copies of one definition two names, and would not match the copy a plugin mounts with `ctx.plugin(def.at(..))`. |
| D9 | **The config is fixed when it is mounted** (§5.5). `update` (`Loader::update`, the testing tool's `update()`) is refused for a definition row, as are `isolate`, `move` and `rename`. The errors are in §4. |
| D10 | **Not done:** plugin definitions in `rutis.json` or in `rutis/loader.create` (a definition is not a name that can be written into a layer and rebuilt); one row shared by `load`ing the same definition twice (each is a copy, as with `ctx.plugin(def.at(..))`); `rutis-host check` knowing definitions handed over at run time (R8 unchanged); the testing tools of Python, Go, Swift and Kotlin (their doubles already receive the definition itself; nothing changes for them). |

## 3. Rules

| Rule | Behaviour |
| --- | --- |
| Entry | `load(definition, { config, services, id, isolate })` (the SDK) → the driver's `loadDefinition` → `rutis-host/test`'s `load_definition` → `Loader::load_copy`. `config` and `id` go down unchanged; `services` are still provided first by the testing tool as rows of the runtime, as today. |
| Result | The row id the host gives. The testing tool's row object is unchanged: `row`, `service`, `methods`, `provided`, `reported`, `on`, `update`, `unload`, `host`. |
| Waiting | As for a row of `load`: a service in the declared `inject` that is not there is waited for, and the copy starts when it is; it does not fail. |
| Row id | `options.id`, else the definition's name, with `-2`, `-3`… while taken — the same rule `load` follows for a name. |
| Name (events, errors) | The id of the row that handed it over (R5); the definition's name for one that belongs to the process. So `provided()` and `reported()` see the services and failures under the origin row. |
| `status` / `entries()` | The copy is listed with the rows: `id` is the row id above, `name` is the definition's name (the origin row's id when it has none), and its state is that fiber's. A copy that has ended is dropped from the records on the next `reconcile` or `status`. |
| Unloading | `unload` / `Loader::remove` ends this copy only, as for a row: its services are withdrawn, then `rows.unload`, then the leases are released. |
| Origin ends | The origin row unloads, restarts or ends: the copies end first and are waited for, then the origin row ends; then `ended()` completes, and the records of the copies go on the next sweep. |
| Runtime ends | The runtime that defined it goes: the copy ends with it, and its record goes on the next sweep. |
| A definition that has ended | The origin has ended (or the copy is loaded after it did): `at` fails, and the host returns the kernel's words as they are (§4). |

Across languages: these rules apply only to the Node and Bun testing tools and their host entry; the other languages' testing tools are untouched (D10).

## 4. What developers see

Role: a plugin author writing a test (TypeScript; Bun differs only in the runtime package's name, and the code is the same).

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

A passing test prints nothing. `parts[0]` is the definition the plugin made in its `apply`, mounted in the runtime that defined it, as `rutis-host` mounts it: its config is checked against the definition's schema and filled with the defaults, it starts once the services in `inject` are all there, and it ends with the origin row.

**When it fails.** One line each: how it is caused, the output word for word, where it is thrown, how to fix it.

| Cause | Output (word for word) | Where | How to fix |
| --- | --- | --- | --- |
| The row that handed the definition over has unloaded, then `load(definition)` | `PluginTestError: part: plugin definition part has ended with its origin` | thrown by `load` | `load` while the row that handed it over is still there; unload the copies before the origin row |
| `update(config)` on a definition row | `PluginTestError: part: a copy of a plugin definition has the config it was loaded with: unload it and load it again` | thrown by `update` | unload the row and `load` it again with the new config |
| `isolate` given for a definition row | `PluginTestError: part: a plugin definition runs where the row that handed it over runs: load it without isolate` | thrown by `load` | leave `isolate` out |
| A config the definition's schema refuses | `PluginTestError: part: config: depth should be a number, got "deep"` | thrown by `load` | fix the config as the definition's schema says |
| `RUTIS_HOST` names a `rutis-host` older than the testing tool (0.19 or earlier, with no `load_definition`) | `PluginTestError: the test driver has no method loadDefinition` | thrown by `load` | use the same version of `@arcships/rutis-host`, or point `RUTIS_HOST` at its new binary |

The output names only what the developer wrote: row ids, the definition's name, config paths. The implementing PRs' acceptance tests compare this table word for word.

**Concepts to learn.** None: R10 already promised that a definition a double receives goes straight to `load`; this only adds it for Node and Bun. A test author still only needs to know that a definition runs as a row in the runtime that defined it, and ends with the row that handed it over.

## 5. Compatibility and migration

- **Protocol:** no new frames, no change to the handshake's shape. The test-side session declares one more capability, `plugins` (already defined by R10 §5.3; the testing tool's side simply did not declare it). A side that does not declare it behaves as before.
- **The host's interface:** `rutis-host/test` gains a method, `load_definition`. A host from 0.19 or earlier does not have it, and the new `load` fails with the last line of §4. Needing the same version is true today (`@arcships/rutis-host` is a dev dependency of the testing tool; `RUTIS_HOST` is the user's to name).
- **The runtime package:** the definition is handed over by the runtime package (`@arcships/rutis-runtime` / `@arcships/rutis-bun`). An older runtime hands over no definitions and knows no `loadDefinition`, and fails the same way.
- **Public API:** `@arcships/rutis/testing`'s `load` takes one more kind of value; the TypeScript declarations do not change (`Definition` has been written as TypeScript's plugin type since R9). `rutis_loader::Loader::load_copy` is new, and `rutis_bridge::runtime` gains a function that reads `ForeignPlugin` out of a `Definition`.
- **Incompatible changes:** none. No migration guide.

## 6. Implementation

**PR1: the test side declares `plugins` and takes the definition (D1, D2).**
`node/rutis-runtime` (`src/testing/start.mjs`, `src/session.mjs`) and Bun's copies of them.
Done means: a service the test provides receives `register(definition)`, and when the double hands it straight back the plugin gets its own definition; one test each in `node/rutis/test` and `bun/rutis-bun/test`; `load` behaves as before.

**PR2: `load` mounts a definition, per D3–D9.**
`crates/rutis-loader` (`Loader::load_copy`; `entries`, `get`, `remove` and `status` know copies), `crates/rutis-bridge` (`Definition` → `ForeignPlugin`), `crates/rutis-host` (`load_definition`), `node/rutis-runtime/src/testing/rutis-test.mjs` and `node/rutis` (`load` takes the definition), and Bun's copies.
Done means: `Loader` unit tests (mounting, unloading, ending with the origin, visible in `status`, `update` and `isolate` refused); a `rutis-host` testing-tool test; the reproduction test for this issue in `node/rutis/test` (a double catches the definition, `load` mounts the Session part, that shape) and its Bun twin; Dim's `runtime/plugins/scheduler/scheduler.test.mjs` written as it is runs; docs: the sentence in `docs/guide/plugin-api.md` about a double's definition going straight to `load` gains Node and Bun, one line each in the release notes, and the `loader/plugin-value-copies` entry in the guarantees table gains a paragraph for "a definition as a row", naming the tests PR2 adds.
