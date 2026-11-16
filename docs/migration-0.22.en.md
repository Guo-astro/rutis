# Migrating to 0.22

[中文](migration-0.22.md)

0.22 is not released yet; this guide is written alongside the behaviour changes, a section with each one, and finished before the release as [#694](https://github.com/arcships/rutis/issues/694) says. It covers what differs from 0.21, and nothing else; a form that goes is deleted rather than deprecated.

0.22 **removes and renames no API a plugin sees**: the session protocol is still 4, `rutis.json`'s `format` is still 1, the plugin API is still 3, and the kernel's public API snapshot `crates/rutis/public-api.txt` is unchanged. The changes fall into two groups: code that relied on the old behaviour now meets a refusal or an empty result, and code that needs no change but no longer sees an old spurious failure or a silent ignore.

## What to change

### `filter(predicate)` must answer synchronously (#637)

Design: [events protocol](design-events-protocol-2026-10-18.en.md) §6. Node, Bun and Python agree.

#### What changed

A `filter(predicate)` predicate could return a Promise, a thenable or a coroutine; those are always truthy, so the filter passed every listener. The predicate has to answer synchronously now, and a Promise or thenable fails that dispatch:

| Language | 0.21 | 0.22 |
| --- | --- | --- |
| Node, Bun | An `async` predicate was truthy, so everything passed | `TypeError: a filter predicate must answer with a boolean, not a Promise` |
| Python | An `async def` predicate returned a coroutine, always truthy | `TypeError: a filter predicate must return a boolean, not an awaitable`, and the coroutine is closed first |

The error belongs to that dispatch; registration is unchanged and other dispatches run as before.

#### What to change

- Write the predicate as an ordinary function that returns a boolean synchronously: `h.filter(d => d.row === 'clock')`, not `async d => …`.
- To decide asynchronously before filtering, compute the answer before calling the emitter (await it once, keep a boolean), then pass that boolean as the predicate; `filter` has no asynchronous predicate.
- In Python, change `async def predicate(...)` to `def predicate(...)`; if it needs to `await`, move that work before `filter`.

#### Who is affected

- Plugin authors: only plugins that use `filter`; a synchronous `filter(() => false)` is unchanged.
- Host owners: the host does not call this API and sees no change.
- Deployers: no visible change.

### `ctx.provide` after a row starts unloading (#1143)

Design: [unload order](design-unload-order-2026-11-01.en.md). Rust, Go, Kotlin and Swift always refused it; from 0.22 Python and Bun agree.

#### What changed

Once a row starts unloading, the unload has taken the cleanup stack. A later `ctx.provide`:

| Case | 0.21 | 0.22 |
| --- | --- | --- |
| Python's `ctx.provide` | Accepted; the service stayed in `services` / `slots` with no row, and a `service` frame with a handle was sent to the host if the unload had not finished | `RuntimeError: row <row> is not active`; no withdrawal is registered and no frame is sent |
| Bun's `ctx.provide` | Accepted; the service stayed in the table with no row | `Error: row <row> is not active`; no withdrawal is registered and no frame is sent |

`<row>` is the row's name (when it has one) or its key. Rust's error type is still `InactiveEffect` (`fiber disposed`).

#### What to change

- Call `provide` in `apply`, or while the row is still loading or running; do not call it from a cleanup, a `stop` hook or an asynchronous callback after the unload started.
- If a service is only needed during cleanup, provide it in `apply` and let the existing reverse stack withdraw it.
- Go, Kotlin and Swift already did not register the service in 0.21 (Go and Kotlin print `row <row> is not active: service <name> is not provided`; Swift returns `row <row> is not active`); this fills in Python and Bun.

#### Who is affected

- Plugin authors: a plugin that calls `provide` from a cleanup (which never worked in Rust) now fails in Python and Bun too. This closes an old hole, not a new restriction.
- Host owners: they no longer see an "unloaded row still provides this service" announcement.
- Deployers: no visible change.

### A listener you withdrew yourself answers with no result (#1135, #1138)

Design: [events protocol](design-events-protocol-2026-10-18.en.md) §3 decision 9, §5.7. All seven runtimes agree.

#### What changed

The host owns the listener table: it may have selected a listener for a dispatch while the runtime has just withdrawn it and sent `events.off`. By the time the call arrives, the local entry is gone.

| Case | 0.21 | 0.22 |
| --- | --- | --- |
| A listener the runtime withdrew itself and told the host about | Node and Bun threw `BindingError: listener <id> of <name> is withdrawn`; the other runtimes also failed once | The call answers with no result (each language's own empty value); `waterfall` calls `next` and returns its value; the dispatch does not fail because of the near miss |
| An id this process never registered | `BindingError: listener <id> of <name> is withdrawn` | Unchanged |
| An entry the host itself withdrew (row unload, `rows.ended`, a failed load) | `BindingError: listener <id> of <name> is withdrawn` | Unchanged |

The runtime remembers the ids it withdrew and told the host about (with a limit, oldest forgotten), so the empty answer is only for "this side just withdrew it".

#### What to change

- Do not use `listener ... is withdrawn` to decide "this dispatch failed": a listener you withdrew yourself now answers with no result. Use your own disposer or local state to know about the withdrawal.
- `waterfall`: a late host call now calls `next` and the chain continues from where it should have been; before, the whole dispatch failed.
- Python, Go, Kotlin and Swift agree with Node and Bun in 0.22; their 0.21 "fail once" no longer happens.

#### Who is affected

- Plugin authors: only a plugin that withdraws a listener at the same moment a host call is on its way sees a difference; ordinary registration and withdrawal are unchanged.
- Host owners: no code change; a host log that counts `withdrawn` as a failure will count fewer.
- Deployers: no visible change.

### Kotlin and Swift no longer ignore unknown fields (#635)

Design: [session protocol](protocol-session.en.md) §9. All seven runtimes agree.

#### What changed

| Frame | Kotlin and Swift in 0.21 | Kotlin and Swift in 0.22 |
| --- | --- | --- |
| `invoke`, `call`, `get`, `await` with an unknown field | Ignored silently | Only that call fails: `code` is `bad-frame`, the message is `unknown field "x" in invoke`, and `data` is `{op, fields}`; `x` is the field path, and a field inside a value reads as `args.value.x` |
| `return`, `throw` with an unknown field | Ignored silently | Only the call waiting for that reply fails, with the same `code` and message |
| `hello` with an unknown field | Ignored silently | The handshake fails: `incompatible session: unknown field "x" in hello` |
| `release`, `cancel` with an unknown field | Ignored silently | Still ignored |

Rust, Node, Bun, Python and Go already behaved this way in 0.21; 0.22 brings Kotlin and Swift in line.

#### What to change

- Do not use unknown fields as an extension mechanism: `SESS-69` does not guarantee that a new optional field whose meaning does not change arrives silently at an old receiver. A new frame or a changed meaning needs a protocol version bump; a new capability is used only when declared in `capabilities`.
- If you sent frames with custom fields on Kotlin or Swift, the receiver now fails that call with `bad-frame` instead of ignoring it. Move the field into the known shape of `data`, or go through a new protocol version.

#### Who is affected

- Plugin authors: an ordinary plugin writes no protocol frames and sees no change.
- Runtime authors: the Kotlin and Swift implementations gained an `unknownFields` check, and all seven runtimes behave the same.
- Deployers: an old Kotlin/Swift still ignores unknown fields; when old and new are mixed, the old side keeps the old behaviour.

## Changes that need no code change

### The heartbeat starts at hello (#1102, #1182)

A listening Go, Swift, Node, Bun or Kotlin side now starts its heartbeat after hello is sent successfully; 0.21 started it at accept or connection construction. When an old-lease cleanup or the handshake was slow, the new connection used to be closed with 1001 before hello; it is not now. The close code is still 1001; Go, Swift and Kotlin use `the far end stopped answering` as the reason. No code change is needed; waits or retries added for that spurious close can go. Python's `websockets` heartbeat path is not part of this change.

### A node-imported service carries its row id (#1171)

`services.announce` gains an optional `row`: the provider's row id when it has one, absent when it has none. The importing side marks the remote plugin with `ctx.mark_row(row)`, so `rutis/service` events now carry the provider's row id instead of `null`. An old peer ignores the field; a new peer reading an old announcement gets `None`. Code that reads `row` by name is unaffected (there is one more key).

### An unload ahead of its load is refused (#1101)

When the host receives a row's `rows.unload` before the same key's `rows.load`, the later load now fails with `<key> was unloaded`. The tombstone is per session: the same key loads in a new session, and unloading a mounted row leaves no tombstone. Plugin authors do not see this; a host should load before unload, or accept that the load fails and retry under a new key.

### The failure text of a 4003 rejection (#1130)

When the far end rejects a connection with 4003, this end used to read `peer disconnected` (transport) and now reads `endpoint mismatch: <reason>`; both directions of a channel pair get the same close reason. Code that matches on the error type or code is unaffected; code that matches the text `peer disconnected` changes.

### `SyncWaitCycle` in Node and Bun (#639)

Node and Bun now count only unanswered synchronous calls when deciding whether a synchronous wait is a cycle; a call whose reply has been read no longer counts, so a callback after an `await` no longer reports `SyncWaitCycle` by mistake. A real cycle is still refused in the same place. No code change is needed.

### Other fixes

- **Python's first import of a plugin**: on Windows, when the directory mtime fell on the same tick, the import could fail with `ModuleNotFoundError`; the first `import_module` now calls `invalidate_caches()`. No code change ([#1139](https://github.com/arcships/rutis/issues/1139), [#1140](https://github.com/arcships/rutis/pull/1140)).
- **Kotlin app hosting**: after `stop()` returns, the app no longer dials or binds; the `RutisApp` start/stop race is fixed. No code change ([#1096](https://github.com/arcships/rutis/pull/1096)).
- **Node and Bun force-killing an old runtime**: a takeover waits `RUTIS_SHUTDOWN_TIMEOUT + 2s`, not a hard-coded 5 seconds. No code change (same #1096).
- **Go reading a token line on Windows**: it reads to the end of the line byte by byte and no longer swallows the greeting that follows; the test tool no longer reports `peer disconnected` ([#1087](https://github.com/arcships/rutis/pull/1087)).

## Who is affected

- **Plugin authors**: three things to change: a `filter` predicate must answer synchronously; `ctx.provide` must be called while the row is still loading or running; unknown fields on Kotlin and Swift are not an extension mechanism. A listener you withdrew yourself answering with no result is a loosening and needs no change. Heartbeats, a service's `row`, `SyncWaitCycle`, Python imports and Kotlin app hosting need no change.
- **Host owners**: no public Rust API is removed or renamed, so embedded code needs no change. The differences are in error texts: a 4003 rejection is `endpoint mismatch: <reason>` instead of `peer disconnected`; a node-imported `rutis/service` event carries a row id instead of `null`; an `unload` that overtakes its `load` gets `<key> was unloaded`. The Rust `public-api.txt` snapshot is unchanged.
- **Deployers**: no new configuration, `rutis.json` field, protocol version or migration setting; the session protocol is still 4, `format` is still 1 and the plugin API is still 3. An old runtime package next to a new host or runtime behaves as it did, and the session still comes up.
- **Runtime authors**: `docs/protocol-session.en.md` is the current session specification; unknown fields, the malformed-frame matrix, the heartbeat start, the withdrawal race, the `provide` refusal and a service's `row` all follow it. The tracker and `specs/` are contributor evidence, not runtime output.
- **Test tools**: a late call after a listener is withdrawn now answers with no result; a test that asserts the `withdrawn` failure changes to assert no result (`waterfall` asserts the value from `next`).

## Compatibility

- The session protocol is still 4, `rutis.json`'s `format` is still 1 and the plugin API is still 3: no new capability requirement, no new required frame field, and the handshake is unchanged.
- Old and new versions mix: an old runtime package keeps the old behaviour on its side (old Kotlin/Swift still ignore unknown fields, the old heartbeat still starts at accept, old Python/Bun still accept a `provide` after unload); the new side does not end the session because the other side is old. `services.announce`'s `row` is optional.
- `rutis.json` and the dsh user layer have no format change: the persisted samples are copied from 0.21.0 and the expectations do not change. The release PR creates `tests/fixtures/persisted/0.22.0/`.
- A dylib plugin is rebuilt with 0.22.0's SDK, as with every release.
- No network smoke or release-gate result is part of this draft; run the normal process in [release.md](release.en.md) before the release.
