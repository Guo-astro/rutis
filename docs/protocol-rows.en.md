# Row protocol: runtime rows (protocol 4)

[中文](protocol-rows.md)

Status: current specification. Date: 2026-11-16. Baseline: `main` `b8fb89ba`.
Issues: [#635](https://github.com/arcships/rutis/issues/635) (the current specification) and [#613](https://github.com/arcships/rutis/issues/613) (the row-lifecycle model), part of [#183](https://github.com/arcships/rutis/issues/183) step 4; the model is [row-lifecycle](../specs/row-lifecycle/README.en.md), the trace vocabulary is the [session trace contract](formal-trace.en.md).

This is the current specification of the row protocol between a host and a runtime: loading, updating and unloading a row on one session, the services a row provides and the host services it leases, a row ending or failing on its own, and the row lifecycle on the host (loader) side. It writes down only behaviour the current implementation has evidence for.

Out of scope: session frames themselves ([session protocol](protocol-session.en.md)); events ([events protocol](design-events-protocol-2026-10-18.en.md)); remote leases, reconnection and identity ([remote lease model](../specs/remote-lease/README.en.md)); peer-hosted rows ([peer-rows](../specs/peer-rows/README.en.md)); supervision, restart and idle stop of runtime processes ([quality status](quality-status.en.md) §7.2, RS1).

Each rule has a number (`ROWS-*`). There are four statuses:

| Status | Meaning |
| --- | --- |
| **Current** | Executed by the implementation and tests named here; code location or test anchor given. |
| **Superseded** | An old design, issue or protocol version; no longer evidence. |
| **Not guaranteed** | Deliberately not promised; verifiers must not rely on it. |
| **Unconfirmed** | Only one implementation or only the model, with no cross-implementation case; not a promise of all seven runtimes until the evidence is there. |

Error **text** is a contract only where this file gives it word for word; the same kind of error may read differently in different implementations (§10 records the known differences).

## 1. Scope and carrier

`ROWS-1` **Current.** The row protocol is a set of control operations on a session: an `invoke` frame with an empty `target`, the control operation in `method`, its arguments in `args`. The host sends `mount`, `rows.load`, `rows.update`, `rows.unload`, `rows.schema`, `hosts.provide`, `hosts.withdraw` and `plugins.read` (`plugin-values`); the runtime sends `rows.ended`, `rows.failed` and `service` (slot changes). An `invoke` with a non-empty `target` is an application service call and has nothing to do with the row protocol. `crates/rutis-bridge/src/runtime/process.rs:480-608`; `node/rutis-runtime/src/runner.mjs:1008-1041`

`ROWS-2` **Current.** One session of a runtime serves all of its rows; no session or channel is opened per row. Control operations are handled in the session's frame order, with no other ordering against the frames around them. `rows.ended` and the `events.*` of the same session are handled in frame order (session protocol `SESS-19`). `crates/rutis-bridge/tests/conformance_session_tracker.rs::a_remote_runtime_runs_rows_with_the_same_control_operations`, `::the_management_frames_do_not_depend_on_the_carrier`; `crates/rutis-bridge/tests/conformance_events_tracker.rs::rows_ended_is_a_table_change_in_frame_order`

`ROWS-3` **Current.** A runtime that runs rows must declare the capabilities `rows.v2` and `hosts`. The host checks before it lets the runtime run rows and refuses with this text, word for word:

```text
the <runtime name> runtime lacks <the missing capabilities, joined with and>: a runtime speaking the row contract is required (for Node, @arcships/rutis-runtime 0.3.0 or later)
```

A static mount (a `Mount` that runs no rows) is not checked. `crates/rutis-bridge/src/runtime/process.rs:13,1452-1472`; `crates/rutis-host/src/status.rs:474-476`

`ROWS-4` **Current.** All six runtimes report the same capabilities in the `mount` reply: `rows.v2`, `hosts`, `leaf`, `scopes`, `rows.announce`, `plugin-values`; Node reports `leaf.js` instead of `leaf`. The host reads only the capabilities it uses, and does not judge by "all lists equal". `node/rutis-runtime/src/runner.mjs:89`; `python/rutis/rutis/runner.py:75`; `bun/rutis-bun/src/runner.ts:60`; `go/rutis/internal/runtime/runtime.go:28`; `swift/rutis/Sources/Rutis/API/Runtime.swift:13`; `kotlin/rutis/rutis/src/shared/kotlin/arcships/rutis/internal/Runtime.kt:41`; `crates/rutis-bridge/src/session/plugin/rows.rs:47`

## 2. Keys, generations and row ids

`ROWS-5` **Current.** A row is named by its `key` on one session; one `key` is at most one row. Loading the same `key` again fails that call, with the text `row <key> is already loaded`. `node/rutis-runtime/src/runner.mjs:647`; `bun/rutis-bun/src/runner.ts:358`; `python/rutis/rutis/runner.py:616`; `go/rutis/internal/runtime/runtime.go:520`; `swift/rutis/Sources/Rutis/API/Runtime.swift:792`; `kotlin/rutis/rutis/src/shared/kotlin/arcships/rutis/internal/Runtime.kt:615`

`ROWS-6` **Current.** Every load takes a new `key`, so the old and the new generation of a row never share one: loader rows use `<instance>.<generation>` (`crates/rutis-loader/src/runtime.rs:719-727`), `run` API rows use `<plugin>#<n>` (`crates/rutis-loader/src/run.rs:168`). A late `rows.ended` / `rows.failed` for an older generation acts on the key it names and neither ends nor restarts the new generation. `crates/rutis-loader/tests/session_rows.rs::a_late_notice_of_an_old_key_reaches_no_row`, `crates/rutis-loader/tests/runtime_rows.rs::a_row_restarted_while_loading_is_unloaded_in_the_runtime`; the model's `OwnFramesOnly`

`ROWS-7` **Current.** The seventh argument of `rows.load` is `{scope, id}`: `scope` is the row's scope chain (`[{id, group}]`, `[]` at the root), `id` is the id the configuration gives it; without `id` the `key` is used. `id` need not be unique (instances of one group may copy a row's id); `key` must be. `crates/rutis-bridge/src/runtime/process.rs:1094-1111`; `crates/rutis-bridge/tests/conformance_events_tracker.rs::rows_load_carries_seven_arguments_with_the_scope_and_the_id`

## 3. Loading

`ROWS-8` **Current.** `rows.load` takes exactly seven arguments, in this order:

```text
[key, entry, config, isolate, inject, exports, {scope, id}]
```

The runtime interprets `entry` itself (a module name, a bundle, a plugin name inside a binary, or a plugin definition reference); `config` is the config with defaults filled in and validated; `isolate` is a list of `[service, label]` pairs; `inject` is a list of service names this row waits for; `exports` is `{service: {method: "sync" | "async"}}`, `{}` when it exports none. The host always sends seven arguments. `crates/rutis-bridge/src/runtime/process.rs:1068-1079,1284-1294`; `node/rutis-runtime/src/runner.mjs:647`; `python/rutis/rutis/runner.py:1277-1287`; the test `crates/rutis-bridge/tests/conformance_events_tracker.rs::rows_load_carries_seven_arguments_with_the_scope_and_the_id`

`ROWS-9` **Current.** The reply to `rows.load` comes only after: the plugin's `apply` returned; the services it declares are offered to rutis; the services it provides without declaring them are announced and accepted; its listeners are registered on the bus. An error the plugin throws while loading (including a rejection its `apply` returns) fails the load; there is no "loaded and then failed". `node/rutis-runtime/src/runner.mjs:673-742`; `python/rutis/rutis/runner.py:671-700`; events protocol §4.4 rule 2; `crates/rutis-bridge/src/runtime/process.rs:531-541`

`ROWS-10` **Current.** A failed load fails only that call: the runtime unloads the half-built row (cleanups, services, listeners), the host forgets the row and releases the host services it had leased, and nothing of the row is left. The failure is the `throw` of this `rows.load`, not a separate frame. `node/rutis-runtime/src/runner.mjs:745-756`; `crates/rutis-loader/src/runtime.rs:613-640`; `crates/rutis-bridge/src/runtime/process.rs:1068-1079,1305-1315`; the test `crates/rutis-loader/tests/runtime_rows.rs::a_row_that_fails_to_load_releases_its_leases`

`ROWS-11` **Current.** `rows.schema` takes `entry` and replies with the description of that row: `config` (the config's JSON Schema, `null` when it has none), `inject`, `optional`, `provides` (`{name: {method: "sync" | "async"}}`), `announces` (the runtime announces services it does not declare), `version` (the package's or module's version, `null` when unknown); Node also sends `cordis: true`. Missing fields of the description read as empty. `crates/rutis-bridge/src/runtime/process.rs:730-757,1417-1421`; `node/rutis-runtime/src/runner.mjs:785-817`; `python/rutis/rutis/runner.py:970-980`; `kotlin/rutis/rutis/src/shared/kotlin/arcships/rutis/internal/Runtime.kt:978-990`

`ROWS-12` **Current.** `mount` takes `{plugins, services, provided, announce, scope, runtime}`; its reply has at least `services` and `features`, and may have `plugins` (`{name: description}`), `implementation` and `engine`. The host reads `features` and checks `ROWS-3` first, then learns which plugins the runtime has from `plugins` or by asking `rows.schema` one by one. Go, Swift and Kotlin list `plugins` in the reply; Node, Bun and Python do not, and the host asks `rows.schema` for each. `crates/rutis-bridge/src/runtime/process.rs:1014-1045`; `node/rutis-runtime/src/runner.mjs:938-977`; `go/rutis/internal/runtime/serve.go:136`; `swift/rutis/Sources/Rutis/API/Runtime.swift:964-971`; `kotlin/rutis/rutis/src/shared/kotlin/arcships/rutis/internal/Control.kt:34`

## 4. Updates

`ROWS-13` **Current.** `rows.update` takes `[key, config]`; the key and generation stay. When only volatile fields (the schema's `x-volatile`) changed, Node's Cordis rows commit in place without unloading or a new generation; every other update is "unload the row and load it again under the same `key`". All leaf runtimes (Python, Bun, Go, Swift, Kotlin) have no volatile fields, so every update restarts the row. `crates/rutis-bridge/src/runtime/process.rs:1347-1349`; `node/rutis-runtime/src/runner.mjs:837-886`; `python/rutis/rutis/runner.py:863-875`; `bun/rutis-bun/src/runner.ts:504-511`; `go/rutis/internal/runtime/runtime.go:644-666`; `swift/rutis/Sources/Rutis/API/Runtime.swift:892-905`; `kotlin/rutis/rutis/src/shared/kotlin/arcships/rutis/internal/Runtime.kt:875-887`; the tests `crates/rutis-loader/tests/runtime_rows.rs::volatile_changes_reach_cordis_in_place`, `crates/rutis-bridge/tests/conformance_session_tracker.rs::a_config_change_is_one_update`

`ROWS-14` **Current.** Updating a row that is not loaded (never loaded, or unloading) fails that call, with the text `row <key> is not loaded`. All six runtimes use the same sentence. `node/rutis-runtime/src/runner.mjs:852`; `bun/rutis-bun/src/runner.ts:506`; `python/rutis/rutis/runner.py:870`; `go/rutis/internal/runtime/runtime.go:646-650`; `swift/rutis/Sources/Rutis/API/Runtime.swift:893-896`; `kotlin/rutis/rutis/src/shared/kotlin/arcships/rutis/internal/Runtime.kt:876-879`

`ROWS-15` **Current.** A restarting update runs as an unload followed by a load: the row's services and listeners leave, running listeners are cancelled and waited for, then the new `apply` runs; the reply comes after the new listeners are registered (`ROWS-9`). `node/rutis-runtime/src/runner.mjs:860-884`; `bun/rutis-bun/src/runner.ts:504-511`; `python/rutis/rutis/runner.py:863-875`; `crates/rutis-loader/tests/runtime_rows.rs::volatile_changes_reach_cordis_in_place` (updates of other fields restart)

## 5. Unloading

`ROWS-16` **Current.** `rows.unload` takes `[key]` and is idempotent: a row that is not there is answered without an error. Unloading a row that is already unloading is answered the same way. `node/rutis-runtime/src/runner.mjs:747-750`; `python/rutis/rutis/runner.py:815-819`; `bun/rutis-bun/src/runner.ts:513-517`; `go/rutis/internal/runtime/runtime.go:859-863`; `swift/rutis/Sources/Rutis/API/Runtime.swift:837-841`; `kotlin/rutis/rutis/src/shared/kotlin/arcships/rutis/internal/Runtime.kt:716-720`

`ROWS-17` **Current.** When the unload starts, the row's listeners leave the bus and other rows no longer see its services (a call fails with `unavailable`, wherever the withdrawal sits on the unload stack); the row's own cleanups may still emit, and other rows hear them, until it answers `rows.unload`. After the reply its context emits nothing more: a `BindingError` reading `row <row> is not loaded in this runtime`. The host's entries leave when it sends `rows.unload`. `crates/rutis-bridge/src/runtime/process.rs:1121-1131,1352-1365`; `node/rutis-runtime/src/runner.mjs:747-783`; `sdk/unload-order` in `docs/guarantees.en.md`; events protocol §7.1

`ROWS-18` **Current.** The runtime withdraws the row's services before it answers `rows.unload`, and the reply comes after all cleanups ran; the host forgets the row's services and registrations on the reply. A session ending midway (a drop, or a takeover) withdraws the services first as well. `crates/rutis-bridge/src/runtime/process.rs:1298-1315`; `node/rutis-runtime/src/runner.mjs:747-783`; `sdk/unload-order` in `docs/guarantees.en.md`; `crates/rutis-loader/src/runtime.rs:649-665`

`ROWS-19` **Current.** The session-end side: when a session ends (a drop, or a newer connection taking over), the runtime cleans up all of its rows, each once; the new session's `hello` is answered only after the old session's rows are cleaned up; the old session is no longer read, and its late frames do not act on the new session. `NewSessionOnlyAfterSweep`, `NoRowsAfterSweep` and `CleanupOnce` in `specs/row-lifecycle/README.en.md`; `node/rutis-runtime/src/serve.mjs:1-18` (the takeover order); `python/rutis/rutis/__main__.py:212` (the same order in `serve`); `go/rutis/internal/runtime/listen.go` (the same order)

`ROWS-20` **Current.** The unload's wait has a deadline: it waits for running listeners at most until the shutdown deadline (`RUTIS_SHUTDOWN_TIMEOUT`, 10 seconds by default), then goes on, and cleanups may not have finished; the host's stop deadline is separate, `host/stop-deadline` in `docs/guarantees.en.md`. `go/rutis/internal/runtime/runtime.go:669-686`; `node/rutis-runtime/src/runner.mjs:149,762`; `sdk/unload-order` and `host/stop-deadline` in `docs/guarantees.en.md`

## 6. Services and host-service leases

`ROWS-21` **Current.** A service a row provides is identified by "name + `isolate` label": the name itself without a label, the name and the label (`name\0label`) with one. One id has at most one provider on a session; a clash fails the row loaded later, with the text `service <id> is already exported by row <key>` (Go, Swift and Kotlin: `<name> is already provided by <row>: rename one of them`). `crates/rutis-bridge/src/runtime/process.rs:43-54`; `node/rutis-runtime/src/runner.mjs:650-657`; `go/rutis/internal/runtime/runtime.go:530-538`; `swift/rutis/Sources/Rutis/API/Runtime.swift:53-59`; `kotlin/rutis/rutis/src/shared/kotlin/arcships/rutis/Refusals.kt:81-87`

`ROWS-22` **Current.** The runtime reports slot changes with the `service` control operation: `[id, handle, version]`; for a service provided without a declaration, a fourth argument `{row, methods}` follows, where `row` is the row's key and `methods` is `{method: "sync" | "async"}`. `handle` is `null` for a withdrawal. Declared services are sent before the reply to `rows.load` (`ROWS-9`); services provided without a declaration are announced and accepted before that reply too. `crates/rutis-bridge/src/runtime/process.rs:570-608`; `node/rutis-runtime/src/runner.mjs:266-274,706-714`; `crates/rutis-bridge/tests/row_services.rs::services_a_row_does_not_declare_are_announced_with_their_methods`

`ROWS-23` **Current.** A slot carries a version, one increasing version per change of that id; the host applies only announcements newer than the version it knows and drops a late older one. `crates/rutis-bridge/src/runtime/process.rs:453-472`; `node/rutis-runtime/src/runner.mjs:266`; the model's `NewerWins`

`ROWS-24` **Current.** Host services a row leases go through `hosts.provide` (`[name, methods, label?]`) and `hosts.withdraw` (`[id]`): the first lease of an id provides it, and `withdraw` comes when the last one is released; the id is built as in `ROWS-21`. Providing an id twice fails that call, with the text `host service <id> is already provided`. `crates/rutis-bridge/src/runtime/process.rs:1546-1625`; `node/rutis-runtime/src/runner.mjs:1021-1039`; `python/rutis/rutis/runner.py:1294-1306`; `go/rutis/internal/runtime/runtime.go:890-920`

`ROWS-25` **Current.** A lease does not outlive its row: a row leases the host services it injects when it loads, and they are released after `rows.unload` in its cleanup; a failed load releases them too (`ROWS-10`). `crates/rutis-loader/src/runtime.rs:623-665,1027-1031`; `crates/rutis-bridge/src/runtime/process.rs:1546-1625`; the model's `LeaseWithinRow` and `HostLeases`

## 7. Rows ending and failing on their own

`ROWS-26` **Current.** When a plugin ends on its own (a Cordis plugin disposing its own fiber), the runtime unloads what is left of the row and then sends `rows.ended` (`[key]`); the host ends the row and unloads it, and does not load it again. `rows.ended` is a table change in the session's frame order, in order with `events.*` (`SESS-19`). `crates/rutis-bridge/src/runtime/process.rs:531-541`; `node/rutis-runtime/src/runner.mjs:372-382`; `crates/rutis-bridge/tests/conformance_events_tracker.rs::rows_ended_is_a_table_change_in_frame_order`; `crates/rutis-loader/tests/runtime_rows.rs::a_plugin_that_disposes_itself_ends_its_row`

`ROWS-27` **Unconfirmed.** A plugin ending while it loads: under Node's rule the load still succeeds, and `rows.ended` is sent only after the load's reply (otherwise a failed load would report more than its own error). That is the rule the model's `EndingAfterLoad` builds, on Node's implementation and test alone; the same timing in the other runtimes is unconfirmed. `node/rutis-runtime/src/runner.mjs:737,372-377`; `EndingAfterLoad` in `specs/row-lifecycle/README.en.md`

`ROWS-28` **Current.** (the frame and the host's handling; the senders are below) When a row's code leaves an error unhandled while it runs, the runtime sends `rows.failed`: `[key, why, error?]`, where `why` says what happened (such as `unhandled rejection: Error: boom`) and `error` is the thrown value the runtime encodes. The host fails the row with an `UnhandledError`; the runtime keeps running and keeps the row loaded (the loader decides to restart it, and the restart fails with this error). At most once per generation (the model's `RFail`; Node's `failing` makes it once). `crates/rutis-bridge/src/runtime/process.rs:543-568`; `node/rutis-runtime/src/runner.mjs:1130-1132`; `python/rutis/rutis/runner.py:785-796`; `loader/unhandled-errors` in `docs/guarantees.en.md`

`ROWS-29` **Unconfirmed.** (across implementations) Only Node (tracing rows under `dev`) and Python send `rows.failed`; Bun cannot trace the row and writes `runtime <name>: an unhandled error from an unknown row: …` without failing any row; Go, Swift and Kotlin have no such path. "An error a row leaves unhandled fails that row" is therefore Node's (`dev`) and Python's behaviour today, not a promise of seven runtimes. `bun/rutis-bun/src/main.ts:50-65`; `loader/unhandled-errors` in `docs/guarantees.en.md`

`ROWS-30` **Current.** A `rows.failed` for a row key the host has no handler for is answered with the error `rows.failed: nothing fails row <key> here`; a `rows.ended` for an unregistered key is ignored. `crates/rutis-bridge/src/runtime/process.rs:531-568`

## 8. Row lifecycle on the host side

`ROWS-31` **Current.** Two stages: once the runtime process is up and has greeted, it provides `<language>Runtime`; the host uses that to refresh the resolution of the rows bound to this runtime, and then provides `<language>RuntimeRows`; rows wait for the latter before they load. The refresh is a task of its own: a runtime restart interrupts it, and it is redone after the restart. `crates/rutis-loader/src/runtime/sessions.rs:450-520`; the model's `NoProvisional` (the pre-fix configuration is `before-stale`, 6f8fb5c7); `loader/rows-gate-on-services` in `docs/guarantees.en.md` §14

`ROWS-32` **Current.** A host row is one fiber, with at most one generation running; a late `rows.ended` / `rows.failed` ends only its own generation; a service is withdrawn before its provider unloads; a lease does not outlive its row. `OneRowPerName`, `OwnFramesOnly`, `WithdrawBeforeUnload` and `LeaseWithinRow` in `specs/row-lifecycle/README.en.md`; 36 configurations (`specs/row-lifecycle/expected.txt`)

`ROWS-33` **Current.** A clash refusal is recorded on the load that caused it (the fiber instance): only the next `apply` fails; when the restart is waiting for a dependency and runs no `apply`, the refusal is forgotten and later generations are not failed. `crates/rutis-loader/src/runtime.rs:371-405`; `crates/rutis-loader/tests/fault_containment.rs::node_a_clashing_provide_fails_only_its_row`; the model's `RefusalScope`

`ROWS-34` **Current.** When the runtime process ends, `Runtime` and `RuntimeRows` are both withdrawn, its rows stop and wait, and a load in progress waits too; nothing restarts it automatically — the application decides; other runtimes and rows that do not use its services keep running. After a restart the rows are resolved and loaded again. `wait_for_the_runtime` in `crates/rutis-loader/src/runtime.rs:1013-1035`; `loader/runtime-down-stops-its-rows` and `loader/runtime-start-failure-isolated` in `docs/guarantees.en.md`

## 9. Unconfirmed items and gaps

The following are **not** promises of this specification, and neither verifiers nor implementations may rely on them; the evidence known today is in parentheses:

- **The seven runtimes' row control has no shared conformance matrix** (the note on `SYN-2-13`, #625): the cases for `rows.load` / `update` / `unload` / `schema` are spread over the loader's `*_rows.rs` and each language's own tests; Swift and Kotlin have no cross-language row-control case.
- **The timing of ending while loading** is Node's and the model's only (`ROWS-27`).
- **A row failing on an unhandled error while running** is Node's (`dev`) and Python's only (`ROWS-29`).
- **The differences between the languages' unload stacks, the shape negotiation of `rows.schema` and `mount`, and the call chain** are not in the `specs/row-lifecycle` model ("not covered" in `specs/row-lifecycle/README.en.md`); this file writes only what the implementation and its tests let us check.
- **The takeover order at session end in Swift and Kotlin** is not checked row by row in the model README's correspondence table as it is for Node, Python and Go (`ROWS-19`).
- **Volatile fields of `rows.update`** exist only in Node's Cordis rows; leaf runtimes always restart (`ROWS-13`). That half is "not provided", not unconfirmed.

## 10. What developers see

### Plugin authors

When things go well: `apply(ctx, config)` returns a cleanup; after `ctx.provide(name, object)` the service reaches rutis by `ROWS-22`, and rows of other languages call it by name; services declared in `ctx.inject` gate the row until they are there (the loader, `ROWS-31`). No row protocol is written by hand.

When they go wrong:

- Two rows provide the same name (under the same `isolate` label): the row loaded later fails, the host prints `service <id> is already exported by row <key>` (Go, Swift, Kotlin: `<name> is already provided by <row>: rename one of them`), and `rutis-host run` indents the row (`in <row>:`) and the location the plugin language gives. Rename the service or isolate it.
- A promise rejection nobody handles: under Node (`dev`) and Python the row fails and `rutis-host` prints `failed in apply: <the plugin's own error>` (or `call to <service>.<method> failed: …` from a call), indented with `in <row>:`. Under `run`, and for Bun, only `runtime <name>: an unhandled error from an unknown row: …` is printed and no row fails (`ROWS-29`).
- Emitting from a row's context after it unloaded: a `BindingError` reading `row <row> is not loaded in this runtime`; emissions from its cleanups still reach other rows (`ROWS-17`).

### Host authors and deployers

When things go well: write a row by the name the loader uses (`py:weather.plugin`, `go:tools/ping`, `<connection row>:<plugin>`); the loader loads, updates and unloads it under the keys of `ROWS-6`. When a runtime process ends unexpectedly its rows stop and wait, and it is not restarted automatically (`ROWS-34`).

When they go wrong:

- A runtime that does not declare `rows.v2` or `hosts`: `the <runtime name> runtime lacks rows.v2 and hosts: a runtime speaking the row contract is required (for Node, @arcships/rutis-runtime 0.3.0 or later)`, and the runtime's status line reads `down (<reason>)`. Upgrade the runtime or use another one.
- Two services with one name under one `isolate` label: the row loaded later fails with the text above; the first provider's row keeps running.
- One runtime's rows do not use up another's: another runtime crashing only makes its own rows wait (`ROWS-34`).

### Runtime authors and contributors

A runtime implements the control operations of `ROWS-1`, reports `features` in `mount` (at least `rows.v2` and `hosts`), and gives `ROWS-5`, `ROWS-14` and `ROWS-16`'s errors and idempotence for one `key`. Check the 36 configurations of [`specs/row-lifecycle`](../specs/row-lifecycle/README.en.md) and the "Unconfirmed" list here; when adding a cross-implementation rule, add the shared case first and then change this file.

## 11. Compatibility and migration

- This file describes the row control operations of protocol 4 and does not bump the protocol for them. Adding a control operation, or changing the meaning of an existing argument, follows session protocol `SESS-80`: raise the protocol version and update this file, `Frame`, the malformed matrix and all seven implementations together.
- A new capability follows `SESS-80` too: the sender uses it only as declared, with no version negotiation.
- A runtime of an older SDK that lacks `rows.v2` / `hosts` is refused by `ROWS-3`, and the message says to upgrade the runtime; far ends of 0.18 and earlier are already refused at the session handshake (`SESS-79`).

## Related files

- [Session protocol](protocol-session.en.md)
- [Events protocol](design-events-protocol-2026-10-18.en.md)
- [Row-lifecycle model](../specs/row-lifecycle/README.en.md)
- [Peer-hosted rows](../specs/peer-rows/README.en.md)
- [Guarantee registry](guarantees.en.md) §14 "Loader"
