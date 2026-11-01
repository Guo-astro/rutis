# The testing tool: `load` mounts a plugin definition the plugin hands over

[中文](design-test-tool-definition-load-2026-11-01.md)

Status: draft, not implemented. Date: 2026-11-01 (revised the same day: the route is now "mount the definition as a row of the host" (C1), the same route the Python/Go/Swift/Kotlin testing tools take; the previous version's origin binding, `PluginRef` parameter kind, `OriginResolver`, the rutis-loader copy rows and `reconcile` rules, and the new `Owner` variant are gone. This document touches no Rust and has no incompatible change.)

Issue: [#1004](https://github.com/arcships/rutis/issues/1004) (the TypeScript testing tool: the test process never receives plugin definitions). Its PR1 and PR2 (§7) implement this document. The rules it builds on are R10 and §5.5 of [a plugin is a value](design-plugin-values-2026-10-25.en.md); §4 here states the differences from a real host, and R10 gained a sentence pointing to it.

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

The testing tools of Python, Go, Swift and Kotlin do not have this problem: their runtime is in the test process itself, so a double receives the definition itself, and the testing tool's `load` mounts it **as a row of the host** (Python in three lines: `sys.modules[entry] = holder; holder.plugin = plugin; id = await host.harness.load(entry, config, options)`, `python/rutis/rutis/testing.py:487–489`). This document takes Node/Bun the same way: the definition goes back to the runtime that defined it, the runtime resolves it under a registered name, and the host mounts it as a row as usual.

## 2. Decisions

| # | Decision |
| --- | --- |
| D1 | **The test-side session takes the definition.** A new session option `host` (used by the testing tool only, in `node/rutis-runtime/src/testing/start.mjs` and Bun's `testing/start.ts`): it declares `plugins`; `session.mjs` (Bun's `session.ts:443`) no longer `#refuse`s a `plugin` reference the far end sends, but keeps an import record (the same `#imports`/`#proxies`/`FinalizationRegistry` lifetime functions and object references use) and hands the test an opaque value: it carries only the marker `Symbol.for('rutis.plugin-definition')`, its fields cannot be read and it cannot be called. Handed straight back to any service it is encoded with `home: true`, and the far end gets its own value (`session.mjs:447`). Dropped: taking it in every session by default — a runtime cannot use a definition another runtime made, so other sessions keep refusing, and their error stays accurate. Dropped: giving the value an `apply` so the test process could mount it — a copy runs in the runtime that defined it; mounting it here would be a lie. |
| D2 | **`load` takes that value.** `@arcships/rutis/testing`'s `load()` and `host().load()` take D1's value besides a `definePlugin` result: they skip the "module default export" and plugin API checks and call the driver's new `loadDefinition(definition, config, options)`; the row object is the same as for `load(file)`, its method table comes from the declaration the driver returns (`spec.provides`), and `unload()` goes through the driver's `unloadDefinition`. Dropped: a separate `loadDefinition()` function — R10 says it goes straight to `load`. |
| D3 | **The runtime resolves a registered name, and the host mounts a row as usual.** New `node/rutis-runtime/src/testing/definitions.mjs` (Bun: `bun/rutis-bun/src/testing/definitions.ts`): a `name → definition` table with `register`/`take`/`release`. The places in `runner.mjs` that resolve a row's entry consult it first — `located()` (around `:452`), `pluginOf()` (`:461`), `declaredOf()` (`:526`), `describe()` (`:739`, which `rows.schema` goes through) and `realFile()` (`unhandled.mjs:137`; a registered entry has no file); in Bun, `locate()` in `bun/rutis-bun/src/plugin.ts` and `describe()` at `runner.ts:454`. The driver's two methods (`testing/rutis-test.mjs`, Bun's `testing/rutis-test.ts`):<br>· `loadDefinition(definition, config, options)`: check that this is a value this runtime handed over (if not, refuse — §5 rule 1), pick a registered name `rutis-test/definition-<n>`, `register` it, then `harness.load(name, config, options)` (the host is untouched: config checked against the schema and filled with defaults, row id, `isolate`, service announcements, `status`, `reported`); on failure `release` it and rethrow the host's words as they are. It returns `{ id, provides }`.<br>· `unloadDefinition(id)`: `harness.unload(id)`, then `release` that registration.<br>Dropped: letting the host/loader take definitions directly (`NewEntry` taking a definition, a `PluginRef` parameter, an `Owner` variant, loader copy rows and `reconcile` rules) — that means Rust changes and an incompatible change in 0.20, which three review rounds weighed. Dropped: letting the driver mount `ctx.plugin(definition, config)` itself — a leaf plugin's `ctx` has no `ctx.plugin` (`node/rutis-runtime/src/leaf.mjs`), so it needs a private mount hook or a driver rewritten as a Cordis plugin; and that is not a row: `status`, `provided`, `reported`, `isolate`, `update` and the host's JSON-Schema config check with defaults are all missing (`cordis-config.mjs:10` only covers Cordis's `Config`). |
| D4 | **The row id and the registered name are two things.** The registered name is only for the runtime's resolution (`rutis-test/definition-<n>`, never shown to developers). The row id follows `load(file)`'s rule: `options.id`, else the definition's name (`spec.name`, else the tail of the registered name), with the host's `-2`, `-3`… while taken — so `t.row` is a real row id, and so is the entry in `rutis-host status`'s `rows:`. Dropped: letting the test choose the id — it would fork from `load(file)`'s rule. |
| D5 | **Unloading and cleanup.** The test's `unload()`: `unloadDefinition(id)` → the row unloads and the registration goes. The test ending: the testing tool unloads the rows it did not (the existing `testEnded`, `node/rutis/src/testing.mjs:113`), each through `unloadDefinition`. The driver unloading, the host stopping, the test process dying: the runtime ends, and the rows and the table go with it (`rutis-test.mjs:126–130`). Dropped: having the runtime clean the table when a row unloads — the driver would not know which registration belongs to which row, which is a second record; doing it in `unloadDefinition` keeps it in one place. |
| D6 | **Not done:** having the runtime unload this row when the origin row ends (§4 rule 1); `ended()` and R4 ("only inside the origin's instance or deeper", §4 rule 3); plugin definitions in `rutis.json` or in `rutis/loader.create`; `rutis-host check` knowing definitions that arrive at run time (R8 unchanged); the Python/Go/Swift/Kotlin testing tools (they already work this way); mounting a definition into an instance (a test's rows are at the root; isolation is the `isolate` option). |

## 3. Rules

| Rule | Behaviour |
| --- | --- |
| Handing over | A plugin hands a `definePlugin` result to the test's service; the runtime sends it as a `plugin` reference (R10 §5.1), and the test process's session takes it only when it declared `plugins` (D1). |
| Handing back | The test gives that opaque value to `load` → the driver → the host. On the driver's side the value is the runtime's own `definePlugin` result again (`home: true`). |
| Mounting | The driver registers it under a name and goes through the host's existing `load`: the config is checked against the definition's schema and filled with defaults, a service in `inject` that is not there is waited for, `provides` is announced as a row's is, and `status`/`provided`/`reported`/`on`/`isolate`/`update` are exactly as for `load(file)`. |
| Row id | `options.id`, else the definition's name, with `-2`, `-3`… while taken (the host's existing rule). |
| Unloading | `unload()` → `unloadDefinition(id)`: the row unloads and the registration goes. |
| The test ends | Rows the test did not unload are unloaded by the testing tool (existing behaviour, nothing printed). |
| The runtime goes | The row and the registration go with the runtime; later calls fail as they do for an existing row. |
| The same value twice | Two `load`s are two rows, each with its own id and its own service announcements, independent of each other (§4 rule 2). |
| A value no double received | The driver refuses: `load needs a plugin definition a test double received` (§5 rule 1). |

## 4. Differences from a real host

R10 says "when a plugin definition is handed to a test double, the double receives a value it can pass straight to the testing tool's `load`, as with the real runtime". The agreement here is: **the definition runs as a row, in the runtime that defined it**, down the very path `load(file)` takes (config check and defaults, `inject` gating, service announcements, unloading) — not "a fake mounted in the test process, bypassing the host". Three things differ from the copy a plugin mounts with `ctx.plugin(def.at(&ctx, config)?)`:

1. **The origin row ending does not take this row with it.** R6 (a copy ends with the row that handed it over) binds only the copies a plugin mounts with `ctx.plugin(def.at(..))`: those belong to that generation of the origin row. The one here is **a row of the host**, and the test (or the testing tool when the test ends) unloads it. Dim's code unloads the Session first and the owner after, which agrees with this; the other way around, the Session row keeps running until the test unloads it.
2. **`load`ing one definition twice gives two rows.** Each has its own id and announces its services itself, just as `ctx.plugin(def.at(..))` twice makes two copies — only here the identity is a row id.
3. **`ended()` and R4 do not apply.** They are the kernel's rules for copies of a `Definition`; what a test holds is a value, and after `load` it is a row — there is no "the definition has ended", and no "only inside the origin's instance or deeper" (a test's rows are at the root; use the `isolate` option when isolation is wanted).

Why this keeps R10's promise: R10's "mount it" means "mount it as a row" — the testing tool's `load` has always been a row (`load(file)` returns one) — and the Python/Go/Swift/Kotlin testing tools do exactly this for a definition a double received (Python's `sys.modules` + `harness.load`, §1). R6 is not loosened by this: it binds the copies a plugin mounts, this document does not change it, and it does not pretend the testing tool mounts a kernel copy.

## 5. What developers see

### 5.1 The plugin author writing the test (TypeScript; Bun differs only in the runtime package's name)

**When it works.**

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

A passing test prints nothing. `parts[0]` is the definition the plugin made in its `apply`; `session` is a row of the host, `session.row` is its id, and `provided()`, `reported()`, `on` and `status` are the same as for a row of `load(file)`.

**When it fails.** One line each: how it is caused, the output word for word, where it is thrown, how to fix it.

| Cause | Output (word for word) | Where | How to fix |
| --- | --- | --- | --- |
| `load` is given a value no double received (hand-made, or from another runtime) | `PluginTestError: load needs a plugin definition a test double received` | thrown by `load` | use the value a double received and handed back as it came |
| The config the definition's schema declares refuses this one | `PluginTestError: SchedulerSession: config: depth should be a number, got "deep"` | thrown by `load` | fix the config as the definition's schema says |
| A row the test did not unload when the test ends | nothing is printed: the testing tool unloads it (`testEnded`) | — | unload it in the test to control when |
| Using the row object after unloading it | `PluginTestError: the plugin is unloaded` | thrown by that call | `load` it again |
| `RUTIS_HOST` names a `rutis-host` older than the testing tool (0.19 or earlier) | `PluginTestError: the test driver has no method loadDefinition` | thrown by `load` | use the same version of `@arcships/rutis-host`, or point `RUTIS_HOST` at its new binary |

A definition a test holds never expires: the value lives in the test process, and the runtime cannot release it (it releases only once the value is collected), so there is no "the definition has ended" line.

**Concepts to learn.** None: R10 already promised that a definition a double receives goes straight to `load`. A test author still only needs to know that it runs as a row in the runtime that defined it, and unloads with the test.

### 5.2 The author of the plugin under test

Nothing to change: `await ctx.use('parts').registerDefinition('session', definePlugin({ … }))` as before; what it hands over is the definition its `apply` made. Only the testing tool's side of mounting it differs (§4).

### 5.3 The host owner (an application embedding rutis)

Nothing to change: `rutis-host`, `rutis-loader`, `rutis-bridge` and Rust are untouched. The extra row in `rutis-host status` is an ordinary row (`SchedulerSession: running`), and `rutis-host check` prints the same (a definition that arrives at run time is not something it knows, R8).

### 5.4 Deploying

Nothing changes: no new frames, no new field in `rutis.json`.

## 6. Compatibility and migration

- **Incompatible: none.** `rutis-host`, the loader, the bridge and Rust are untouched; the protocol, `rutis.json` and the CLI's output are unchanged. No migration guide, and no `breaking` label.
- **Added (a behaviour extension):** `@arcships/rutis/testing`'s `load` takes one more kind of value (a definition a double received); the TypeScript declarations do not change (`Definition` has been written as TypeScript's plugin type since R9). The test-side session declares one more capability, `plugins` (already defined by R10 §5.3); a side that does not declare it behaves as before.
- **Versions:** an older runtime package (`@arcships/rutis-runtime` / `@arcships/rutis-bun`) knows no `loadDefinition`, and `load` reports the last line of §5.1; the testing tool and the runtime package must be the same version (they already are, as a dev dependency).

## 7. Implementation

**PR1: the test side declares `plugins` and takes the definition (D1).**
`node/rutis-runtime/src/testing/start.mjs` (the `host` option), `src/session.mjs` (taking it and handing it back); Bun's `src/testing/start.ts` and `src/session.ts`.
Done means: a service the test provides receives `register(definition)`, and when the double hands it straight back the plugin gets its own definition; one test each in `node/rutis/test` and `bun/rutis-bun/test`. **PR1 does not close #1004**.

**PR2: `load` mounts a definition, per D2–D5 (`Closes #1004` when it merges).**
`node/rutis-runtime/src/testing/definitions.mjs` (new), `src/runner.mjs` (`located`/`pluginOf`/`declaredOf`/`describe`), `src/unhandled.mjs` (`realFile`), `src/testing/rutis-test.mjs` (the two methods), `node/rutis/src/testing.mjs` (`load` taking the value, the method table, `unloadDefinition`); Bun's copies (`src/testing/definitions.ts`, `locate` in `src/plugin.ts`, `describe` in `src/runner.ts`, `src/testing/rutis-test.ts`).

What Bun and Node share, and what is written twice:

| | Files |
| --- | --- |
| Shared | `node/rutis/src/testing.mjs` (the SDK; `bun test` uses it too, choosing the runtime package by `typeof Bun`) |
| Written twice (two copies of one design) | the runtime packages: `node/rutis-runtime` (`src/session.mjs`, `src/testing/start.mjs`, `src/testing/rutis-test.mjs`, `src/runner.mjs`, `src/unhandled.mjs`, new `src/testing/definitions.mjs`) and `bun/rutis-bun` (`src/session.ts`, `src/testing/start.ts`, `src/testing/rutis-test.ts`, `src/plugin.ts`, `src/runner.ts`, new `src/testing/definitions.ts`) |
| Two test files, one set of assertions | `node/rutis/test` and `bun/rutis-bun/test`: the same test names and the same assertions (the existing convention) |

Done means (acceptance):

1. Dim's `runtime/plugins/scheduler/scheduler.test.mjs:174` written as it is runs: a double catches the definition, `load(definition, { services })` mounts the Session, `service()` reaches it, and `unload()` leaves nothing behind.
2. A synchronous method in the definition's declared `provides` still goes out as a synchronous call (the method table comes from the declaration the driver returns).
3. A config the schema refuses reports §5.1 rule 2; a value no double received reports rule 1; `load`ing one value twice gives two rows with their own `provided()`.
4. Rows the test did not unload are unloaded when the test ends, with nothing left in the runtime (`rows` empty).
5. `status` (`rutis-host status`) shows this row and drops it as it does for a row of `load(file)`.
6. Docs: the sentence in `docs/guide/plugin-api.md`/`.en.md` about a double's definition going straight to `load` gains Node and Bun; R10 in the plugin values design gains a sentence pointing to §4 here; one line each in the release notes (`load` takes a definition a double received).
