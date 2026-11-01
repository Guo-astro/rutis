# The testing tool: `load` mounts a plugin definition the plugin hands over

[中文](design-test-tool-definition-load-2026-11-01.md)

Status: final, implemented (the pull request for #1004, §7). Date: 2026-11-01 (third revision the same day: route C1 — mount the definition as a row of the host, the same route the Python/Go/Swift/Kotlin testing tools take; the origin binding, `PluginRef`, `OriginResolver`, the rutis-loader copy rows and `reconcile` rules, and the new `Owner` variant are gone. This revision fixes the four review points: the registered name and the default row id follow `crates/rutis-host/src/test.rs`'s actual algorithm; an old runtime, an old SDK and an old host fail in different stages, each with its output word for word; the identity check is the session's own reference tables, and the public Symbol only routes the SDK; the acceptance covers config defaults, gating, listener cleanup, release on a failed load, two independent unloads, and the origin unloading first. This document touches no Rust and has no incompatible change. Revised again during implementation the same day: §5.1's rule 4 now says what a `definePlugin` result made in this process meets when it is handed to a **plugin's service** — the sending side reports that the far end declared no `plugins`, and `load`ing that result still goes by its module, as before, which is not an error; and §7 is one pull request.)

Issue: [#1004](https://github.com/arcships/rutis/issues/1004) (the TypeScript testing tool: the test process never receives plugin definitions). The pull request for #1004 implements this document (§7). The rules it builds on are R10 and §5.5 of [a plugin is a value](design-plugin-values-2026-10-25.en.md); §4 here states the differences from a real host, and R10 gained a sentence pointing to it.

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
| D1 | **The test-side session takes the definition.** A new session option `host` (used by the testing tool only, in `node/rutis-runtime/src/testing/start.mjs` and Bun's `testing/start.ts`): it declares `plugins`; `session.mjs` (Bun's `session.ts:443`) no longer `#refuse`s a `plugin` reference the far end sends, but keeps an import record (the same `#imports`/`#proxies`/`FinalizationRegistry` lifetime functions and object references use) and hands the test an opaque handle. Handed straight back to any service it is encoded with `home: true`, and the far end gets its own value (`session.mjs:447`). The handle's marker, `Symbol.for('rutis.plugin-definition')`, is **only there to route the SDK**, never to check anything (D3 rule 2). Dropped: taking it in every session by default — a runtime cannot use a definition another runtime made, so other sessions keep refusing, and their error stays accurate. Dropped: giving the handle an `apply` so the test process could mount it — a copy runs in the runtime that defined it; mounting it here would be a lie. |
| D2 | **`load` takes that handle.** `@arcships/rutis/testing`'s `load()` and `host().load()` take D1's handle besides a `definePlugin` result: with that marker they skip the "module default export" and plugin API checks and call the driver's new `loadDefinition(handle, config, options)`; the row object is the same as for `load(file)`, its method table comes from the declaration the driver returns (`spec.provides`), and `unload()` goes through the driver's `unloadDefinition`. Dropped: a separate `loadDefinition()` function — R10 says it goes straight to `load`. |
| D3 | **The runtime resolves a registered name, the host mounts a row as usual, and the session's reference tables prove the identity.** <br>1. **The registered name** (internal to the runtime, never shown to developers): new `node/rutis-runtime/src/definitions.mjs` (Bun: `bun/rutis-bun/src/definitions.ts`) with `register`/`take`/`release` (at the top of `src/`, not under `src/testing/`: `crates/rutis-bridge/tests/bun_runtime.rs:465–470` copies only the files of `src/` when it copies the runtime package, since `main.ts` imports nothing of the testing tool's folder, so a `runner` importing from there would leave that copy unable to start); the name is `rutis-test/definition-<n>/<the definition's name>`, the name being `spec.name`, or `plugin` when it has none. Its last segment is the definition's name so that the host's `base_id` (`crates/rutis-host/src/test.rs:393–401`, the last segment without its extension) derives the default row id (D4). The places in `runner.mjs` that resolve a row's entry consult the table first — `located()` (around `:452`), `pluginOf()` (`:461`), `declaredOf()` (`:526`), `describe()` (`:739`, which `rows.schema` goes through) and `realFile()` (`unhandled.mjs:137`; a registered entry has no file); in Bun, `locate()` in `bun/rutis-bun/src/plugin.ts` and `describe()` at `runner.ts:454`.<br>2. **The identity**: the handle is **made by the test-side session** — in `#imports`/`#proxies` it knows "this id is a `plugin` reference the runtime sent", and it encodes the value back with `home: true`; on the runtime's side only **its own** exports decode as `home: true` (`#export` at `session.mjs:509`), and any other id is `unknown, released or mismatched reference`. The public Symbol is a marker anyone can forge, so it takes no part in checking; the checking is entirely in those two session-private tables. A value that is not one this session received comes back one of two ways:<br>· a `definePlugin` result made in the test process → the test-side session sends it as a **new export**, and the driver's session refuses it at `session.mjs:451`: `received a plugin definition made in another process, which cannot be used here; pass plain data or a function instead`;<br>· a forged handle (a plain object or a function carrying the marker) → it arrives as an object or function reference, the driver asks the runtime's `isLeaf` and refuses: `load needs a plugin definition a test double received`.<br>Both are thrown as `PluginTestError` (the SDK's `host.load` wraps the driver's error), with no separate error code.<br>3. **The driver** (`testing/rutis-test.mjs`, Bun's `testing/rutis-test.ts`): `loadDefinition(handle, config, options)` checks that it is a definition, picks the registered name, `register`s it and calls `harness.load(name, config, options)` (the host untouched); on failure it `release`s the entry and rethrows the host's words as they are, and on success it returns `{ id, provides }`. `unloadDefinition(id)` calls `harness.unload(id)` and then `release`s.<br>Dropped: letting the host/loader take definitions directly (`NewEntry` taking a definition, a `PluginRef` parameter, an `Owner` variant, loader copy rows and `reconcile` rules) — that means Rust changes and an incompatible change in 0.20. Dropped: letting the driver mount `ctx.plugin(definition, config)` itself — a leaf plugin's `ctx` has no `ctx.plugin` (`node/rutis-runtime/src/leaf.mjs`), and it would not be a row: `status`, `provided`, `reported`, `isolate`, `update` and the host's JSON-Schema config check with defaults would all be missing (`cordis-config.mjs:10` only covers Cordis's `Config`). |
| D4 | **The row id is the host's existing algorithm.** The registered name only serves the runtime's resolution. The row id is left entirely to `harness.load` (`crates/rutis-host/src/test.rs:335–345, 378–401`):<br>· **no `id` given**: `free_id` takes the registered name's **last segment without its extension** — that is, the definition's name (which is why D3 names it so) — with `-2`, `-3`… while taken;<br>· **an `id` given** (`load(handle, { id })`): used as it is, and a clash with an existing row or copy **fails**: `a row <id> is loaded already`, with **no** suffix added — exactly as `load(file, { id })` behaves today;<br>· a definition's name containing `/` or `.` goes through `base_id`'s rule (last segment, no extension), as for `load(file)`.<br>So one `load` without an id gives `SchedulerSession`, and loading the same definition again gives `SchedulerSession-2`. The tests assert these two lines (§7 acceptance 3). Dropped: letting the test choose the id — it would fork from `load(file)`'s rule; dropped: the driver computing the suffix — the host already does. |
| D5 | **Unloading and cleanup.** The test's `unload()`: `unloadDefinition(id)` → the row unloads and the registration goes. The test ending: the testing tool unloads the rows it did not (the existing `testEnded`, `node/rutis/src/testing.mjs:113`), each through `unloadDefinition`. The driver unloading, the host stopping, the test process dying: the runtime ends, and the rows and the table go with it (`rutis-test.mjs:126–130`). Dropped: having the runtime clean the table when a row unloads — the driver would not know which registration belongs to which row, which is a second record; doing it in `unloadDefinition` keeps it in one place. |
| D6 | **Not done:** having the runtime unload this row when the origin row ends (§4 rule 1); `ended()` and R4 ("only inside the origin's instance or deeper", §4 rule 3); plugin definitions in `rutis.json` or in `rutis/loader.create`; `rutis-host check` knowing definitions that arrive at run time (R8 unchanged); the Python/Go/Swift/Kotlin testing tools (they already work this way); mounting a definition into an instance (a test's rows are at the root; isolation is the `isolate` option). |

## 3. Rules

| Rule | Behaviour |
| --- | --- |
| Handing over | A plugin hands a `definePlugin` result to the test's service; the runtime sends it as a `plugin` reference (R10 §5.1), and the test process's session takes it only when it declared `plugins` (D1). |
| Handing back | The test gives the handle to `load` → the driver → the host. On the driver's side it is the runtime's own `definePlugin` result again (`home: true`). A forged value or one made in the test process never reaches the mount (D3 rule 2). |
| Mounting | The driver registers it under a name and goes through the host's existing `load`: the config is checked against the definition's schema and filled with defaults, a service in `inject` that is not there is waited for, `provides` is announced as a row's is, and `status`/`provided`/`reported`/`on`/`isolate`/`update` are exactly as for `load(file)`. |
| Row id | Without an `id`: the definition's name, with `-2`, `-3`… while taken. With an `id`: used as it is, and a clash fails (`a row <id> is loaded already`) (D4). |
| Unloading | `unload()` → `unloadDefinition(id)`: the row unloads (its listeners go with it) and the registration goes. |
| A failed load | The registration is released on the spot, then the host's words are rethrown; nothing is left in the runtime. |
| The test ends | Rows the test did not unload are unloaded by the testing tool (existing behaviour, nothing printed). |
| The runtime goes | The row and the registration go with the runtime; later calls fail as they do for an existing row. |
| The same value twice | Two `load`s are two rows, each with its own id and its own service announcements, and each unloads independently (§4 rule 2). |

## 4. Differences from a real host

R10 says "when a plugin definition is handed to a test double, the double receives a value it can pass straight to the testing tool's `load`, as with the real runtime". The agreement here is: **the definition runs as a row, in the runtime that defined it**, down the very path `load(file)` takes (config check and defaults, `inject` gating, service announcements, listeners going with the row, unloading) — not "a fake mounted in the test process, bypassing the host". Three things differ from the copy a plugin mounts with `ctx.plugin(def.at(&ctx, config)?)`:

1. **The origin row ending does not take this row with it.** R6 (a copy ends with the row that handed it over) binds only the copies a plugin mounts with `ctx.plugin(def.at(..))`: those belong to that generation of the origin row. The one here is **a row of the host**, and the test (or the testing tool when the test ends) unloads it. Dim's code unloads the Session first and the owner after, which agrees with this; the other way around, the Session row keeps running until the test unloads it (§7 acceptance 8 guards this).
2. **`load`ing one definition twice gives two rows.** Each has its own id and announces its services itself, just as `ctx.plugin(def.at(..))` twice makes two copies — only here the identity is a row id.
3. **`ended()` and R4 do not apply.** They are the kernel's rules for copies of a `Definition`; what a test holds is a handle, and after `load` it is a row — there is no "the definition has ended", and no "only inside the origin's instance or deeper" (a test's rows are at the root; use the `isolate` option when isolation is wanted).

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

A passing test prints nothing. `parts[0]` is the definition the plugin made in its `apply`; `session` is a row of the host, `session.row` is its id (the definition's name, with `-2` when taken), and `provided()`, `reported()`, `on` and `status` are the same as for a row of `load(file)`.

**When it fails.** One line each: how it is caused, the output word for word, where it is thrown, how to fix it. The three version mismatches fail in **different stages** and are all listed.

| Cause | Output (word for word) | Where | How to fix |
| --- | --- | --- | --- |
| An old runtime package (`@arcships/rutis-runtime` / `@arcships/rutis-bun` 0.19): the test-side session never declared `plugins` | `registrar: TypeError: parts.register: argument 1 cannot be sent: cannot send a plugin definition: the far end does not accept plugin definitions (it did not declare the capability plugins)` | the plugin's call that hands the definition over (it never reaches `load`) | bring the runtime package to the SDK's version |
| An old SDK (`@arcships/rutis` 0.19) with a new runtime: the handle exists, the old `load` does not know it | `PluginTestError: load needs a plugin made with definePlugin` | thrown by `load` | bring `@arcships/rutis` to the same version |
| A host older than 0.19 (no `rutis-host test`): it cannot start | `<host> exited (<code>) before the test could start` (or, when the host is missing, `<host> not found: install @arcships/rutis-host (a dev dependency) or set RUTIS_HOST to a rutis-host`) | the first `load` that needs the host (`start.mjs:94–101`) | use `rutis-host` 0.19 or later; a 0.19 host has `test` and works with the new testing tool |
| A `definePlugin` result made in this process handed to a **plugin's service** (not to `load`) | `sink.register: argument 1 cannot be sent: cannot send a plugin definition: the far end does not accept plugin definitions (it did not declare the capability plugins)` | that service call | use the handle a double received and handed back as it came |
| `load` is given something that is not a handle a double received: a forged marker (a plain object or function carrying `Symbol.for('rutis.plugin-definition')`) | `PluginTestError: load needs a plugin definition a test double received` | thrown by `load` | as above |
| The config the definition's schema declares refuses this one | `PluginTestError: SchedulerSession: config: depth should be a number, got "deep"` | thrown by `load` | fix the config as the definition's schema says |
| An explicit `id` that a row already has | `PluginTestError: a row SchedulerSession is loaded already` | thrown by `load` | use another id, or leave it out and let it suffix |
| A row the test did not unload when the test ends | nothing is printed: the testing tool unloads it (`testEnded`) | — | unload it in the test to control when |
| Using the row object after unloading it | `PluginTestError: the plugin is unloaded` | thrown by that call | `load` it again |

`load`ing a `definePlugin` result made in this process goes by its module, as before; that is not an error. `the test driver has no method loadDefinition` appears only when the runtime package is half updated (a new test-side session, an old driver), which a normal install does not produce; `start.mjs:70` starts the host with `DRIVER`/`RUNNER` from the local runtime package, so the host's version does not decide whether that method exists.

A handle a test holds never expires: the value lives in the test process, and the runtime cannot release it (it releases only once the value is collected), so there is no "the definition has ended" line.

**Concepts to learn.** None: R10 already promised that a definition a double receives goes straight to `load`. A test author still only needs to know that it runs as a row in the runtime that defined it, and unloads with the test.

### 5.2 The author of the plugin under test

Nothing to change: `await ctx.use('parts').registerDefinition('session', definePlugin({ … }))` as before; what it hands over is the definition its `apply` made. Only the testing tool's side of mounting it differs (§4).

### 5.3 The host owner (an application embedding rutis)

Nothing to change: `rutis-host`, `rutis-loader`, `rutis-bridge` and Rust are untouched. The extra row in `rutis-host status` is an ordinary row (`SchedulerSession: running`), and `rutis-host check` prints the same (a definition that arrives at run time is not something it knows, R8).

### 5.4 Deploying

Nothing changes: no new frames, no new field in `rutis.json`.

## 6. Compatibility and migration

- **Incompatible: none.** `rutis-host`, the loader, the bridge and Rust are untouched; the protocol, `rutis.json` and the CLI's output are unchanged. No migration guide, and no `breaking` label.
- **Added (a behaviour extension):** `@arcships/rutis/testing`'s `load` takes one more kind of value (a handle a double received); the TypeScript declarations do not change (`Definition` has been written as TypeScript's plugin type since R9). The test-side session declares one more capability, `plugins` (already defined by R10 §5.3); a side that does not declare it behaves as before.
- **Which stage a version mismatch fails in** (word for word in §5.1): an old runtime package fails at the plugin's **handover** (its session declares no `plugins`) and never reaches `load`; an old SDK with a new runtime gets the handle but its `load` does not know it (`load needs a plugin made with definePlugin`); a host older than 0.19 cannot start (`start.mjs:94–101`'s two lines); only a half-updated runtime package (new test side, old driver) can meet `the test driver has no method loadDefinition`. The testing tool, the SDK and the runtime package being the same version is already required today (all dev dependencies).

## 7. Implementation

**One pull request (`Closes #1004` when it merges).** The intermediate state (taking the definition but not mounting it) is worth nothing to a user, so it is not split.

What changes:

- the test side: `node/rutis-runtime/src/testing/start.mjs` (the `host` option), `src/session.mjs` (declaring `plugins`; keeping a `plugin` reference as a handle; handing it back with `home: true`); Bun's `src/testing/start.ts` and `src/session.ts`;
- the runtime: new `node/rutis-runtime/src/definitions.mjs` (the registry); `src/runner.mjs` (`located`/`pluginOf`/`declaredOf`/`describe`) and `src/unhandled.mjs` (`realFile`) consult it first; Bun's copies (new `src/definitions.ts`, `locate` in `src/plugin.ts`, `#plugin`/`describe`/`rowOfFiles` in `src/runner.ts`);
- the driver: `loadDefinition`/`unloadDefinition` in `node/rutis-runtime/src/testing/rutis-test.mjs`, and Bun's `src/testing/rutis-test.ts`;
- the SDK: `node/rutis/src/testing.mjs` (`load` taking the handle, its method table from the declaration the driver returns, `unload` going through `unloadDefinition`; `load()` taking `id`/`isolate` too).

What Bun and Node share, and what is written twice:

| | Files |
| --- | --- |
| Shared | `node/rutis/src/testing.mjs` (the SDK; `bun test` uses it too, choosing the runtime package by `typeof Bun`) |
| Written twice (two copies of one design) | the runtime packages: `node/rutis-runtime` (`src/session.mjs`, `src/testing/start.mjs`, `src/testing/rutis-test.mjs`, `src/runner.mjs`, `src/unhandled.mjs`, new `src/definitions.mjs`) and `bun/rutis-bun` (`src/session.ts`, `src/testing/start.ts`, `src/testing/rutis-test.ts`, `src/plugin.ts`, `src/runner.ts`, new `src/definitions.ts`) |
| Two test files, one set of assertions | `node/rutis/test` and `bun/rutis-bun/test`: the same test names and the same assertions (the existing convention) |

Done means (acceptance):

1. **Dim's code**: `runtime/plugins/scheduler/scheduler.test.mjs:174` written as it is runs — a double catches the definition, `load(definition, { services })` mounts the Session, `service()` reaches it, and `unload()` leaves nothing behind.
2. **Config**: when the definition declares a `config` schema (with defaults), no config given still gets the defaults filled; a config it refuses reports §5.1's line (the host's words wrapped in `PluginTestError`).
3. **Row id**: without an `id` it is the definition's name, a second `load` of the same definition is `<name>-2`, and the two unload independently; an explicit `id` that is taken reports `a row <id> is loaded already`.
4. **Gating**: when a service in the definition's `inject` is not provided, the row waits (`status` shows it waiting) and starts once `services` provides it — the same as `load(file)`.
5. **Listeners**: the definition's listeners go when the row unloads — after unloading, an `emit` of the same name does not reach it; mounting it again receives exactly one.
6. **Release on failure**: when a load fails (a config the schema refuses, a taken name), the registration is released on the spot, and the same definition can be loaded again under its name (no stale registration left to resolve).
7. **The test ends**: rows the test did not unload are unloaded when the test ends, with nothing left in the runtime (`rows` empty) and the registry empty.
8. **§4's differences are guarded by tests**: unloading the origin row first leaves this row running (`service()` still reaches it) until the test unloads it; loading one definition twice gives two rows with their own `provided()`.
9. **Docs**: the sentence in `docs/guide/plugin-api.md`/`.en.md` about a double's definition going straight to `load` gains Node and Bun; R10 in the plugin values design gains a sentence pointing to §4 here; one line each in the release notes (`load` takes a handle a double received).
