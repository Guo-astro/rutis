# Migrating to 0.23

[中文](migration-0.23.md)

0.23 is not released yet; this guide is written alongside the behaviour changes, a section with each one, and finished before the release as [#694](https://github.com/arcships/rutis/issues/694) says. It covers what differs from 0.22, and nothing else; a form that goes is deleted rather than deprecated.

0.23 **removes and renames no API a plugin sees**: the plugin API is still 3 and `rutis.json`'s `format` is still 1. The session protocol is **5**, and every end must be upgraded together: a protocol-4 host, runtime, node or SDK is refused at the handshake, and no mixed protocol-4/protocol-5 session exists.

## What to change

### Upgrade every end together: session protocol 4 → 5 (#1099)

Design: [session protocol](protocol-session.en.md) §2.1, §7, §8, §13; model: [`specs/call-lifecycle`](../specs/call-lifecycle/README.en.md) (`call-gap-ack`, `relay-healthy`).

#### What changed

Protocol 5 adds the `ack` frame and one rule: a cancel is resolved exactly once by the callee. The caller's cancelled table (`calls.cancelled`) was cleared only by a late reply, and no conforming callee replies to a call cancelled while queued, so a long session that cancelled queued calls kept every entry until it ended. Now:

| Situation | 0.22 (protocol 4) | 0.23 (protocol 5) |
| --- | --- | --- |
| A call still queued is cancelled | Dropped, no reply; the caller's entry stays until the session ends | Dropped, method never runs, arguments released, and the callee sends `ack`; the caller clears the entry |
| A call already running is cancelled | Runs to the end; the callee replies as usual or drops it, implementation by implementation; the caller discards a late reply | Runs to the end; the result is dropped and the callee sends `ack`; no reply follows |
| An `await` is cancelled | Dropped without a reply | Dropped and acknowledged |
| The reply was written before the cancel was read | The reply is the late reply, discarded | The reply is the resolution; no `ack` follows, and the caller discards the reply |
| An `ack` arrives for a call whose entry a crossed reply already cleared | (No `ack` existed) | Ignored |
| A reply arrives after an `ack` | (No `ack` existed) | `response for unknown call`: the session ends |
| A far end speaks protocol 4 | Met | `incompatible session: the far end speaks protocol 4, this side 5: upgrade the far end` |

The changes are wire-level only. The plugin API, call results, error values, reference grants and the `rutis.json` format do not change. An ordinary plugin call that is not cancelled behaves exactly as in 0.22.

#### What to change

- Upgrade the host and **every** runtime, node and SDK together: `rutis-host` / the Rust crates, the Node and Bun runtime packages, the Python package, the Go module, the Swift package and the Kotlin artifact. A deployment with a 0.22 end in it does not start: the handshake is refused, the link stops and is not retried as a protocol mismatch (an identity mismatch is retried slowly, as before).
- Plugin code needs no change; a plugin that relies on a cancelled running call still receiving its reply as a *value* was already not portable in 0.22 (SESS-61 allowed either reply), and in 0.23 the result is dropped and acknowledged.
- An implementation of the session protocol outside the seven in this repository must add the `ack` frame, acknowledge a cancel it still holds, suppress the reply after the acknowledgment, and ignore an `ack` whose entry is gone. It must also accept protocol 5 in the greeting and the `rutis.5` WebSocket subprotocol.
- A host or tool that pins the WebSocket subprotocol must say `rutis.5`; `rutis.4` is refused as an older dialer.

#### Who is affected

- Plugin authors: no source change. A plugin that cancels a call it started (by dropping the future / `Promise` / task) sees the same call result as before: its own cancellation error, never the far end's value.
- Host owners: the session protocol, the `hello.version` and the WebSocket subprotocol change; the `rutis-host` commands, status lines, exit codes and close codes do not. Upgrade all runtime images and SDKs in the same deployment.
- Deployers: a mixed 0.22/0.23 deployment is refused at the handshake, with the message above naming the end to upgrade. Upgrade both ends before rolling out.

#### What developers see

**When it works:** ordinary calls, objects, events and reference grants are unchanged; the only difference is that a cancelled call's table entry goes away when the callee acknowledges it, instead of at the session's end.

**When it fails (word for word, Rust side):**

| Situation | Output |
| --- | --- |
| The far end is protocol 4 | `incompatible session: the far end speaks protocol 4, this side 5: upgrade the far end` |
| The far end is protocol 99 | `incompatible session: the far end speaks protocol 99, this side 5: upgrade this side (Rust crates rutis-host / rutis-bridge)` |
| A non-conforming callee replies after its `ack` | the session ends with a transport error, `response for unknown call`; in-flight calls fail with `peer-gone` |
| A late reply to a cancelled call (before the `ack`) | no error; the reply is discarded and counted (`orphans()`), and the session goes on |

**Concepts to learn:** one new frame, `ack`, for runtime and session authors only. A plugin sees none of it.

#### Verification

```sh
cargo test -p rutis-bridge --all-features
cargo test -p rutis-bridge --all-features --test runtime_conformance
RUTIS_TEST_RUNTIMES=node,bun,python,go,swift,kotlin \
  cargo test -p rutis-bridge --all-features --test runtime_conformance
TLA2TOOLS=/tmp/tla2tools.jar specs/call-lifecycle/run.sh
node tools/check-guarantees.mjs
```

The session tests cover the queued cancel, the running cancel, the crossed reply, the `ack` that clears the table, an `ack` with no entry, and a reply after the `ack`; the TLA configurations `call-gap-ack`, `call-mut-ack-then-reply`, `relay-healthy` and `relay-mut-ack-then-reply` are the model's part.

### An asynchronous call says whether its result is awaited: `awaited` (#1098)

Specification: [session protocol](protocol-session.en.md) `SESS-83`, `SESS-7`, `SESS-66`, §14 G7; model: [`specs/cross-runtime-sync`](../specs/cross-runtime-sync/README.en.md) (`async-forgotten-policy-b`, `async-policy-b`).

#### What changed

A call frame (`invoke`, `call`, `get`, `await`) gains an optional boolean `awaited`: `false` means "fired and forgotten", the caller will not wait for this result before the call that made it finishes; absent or `true` means it will. It is meaningful only for asynchronous calls and is ignored when `sync: true`. The host counts a call in the wait graph only when it is `sync` or an asynchronous call whose result will be waited for; a call that says `awaited: false` is an entry, no longer a hop because it appears on a marked chain. The absent field keeps the old semantics, so an old caller still gets the conservative edge (and may be refused by a false cycle); saying `false` is what removes the edge.

#### What to change

- Upgrade the host and all seven runtimes together; the implementations in this repository already know the field. An earlier protocol-5 prerelease build treats `awaited` as an unknown field and fails the call (`SESS-66`); replace it with a build that has the field.
- A runtime that knows nobody waits for a call (`sendAnswered` / `request_answered` / `notify` and the like) writes `awaited: false`; user code's ordinary asynchronous calls omit it when the runtime cannot tell, and the host reads them as waited for.
- Plugin code needs no change and there is no new API: this is a session-layer field, not part of the plugin API.

#### Who is affected

- Plugin authors: no new concept. An asynchronous call nobody waits for can no longer be refused with a false `SyncWaitCycle`; a call that is awaited still has its real cycle refused as before.
- Runtime authors: write `awaited: false` on your own fired-and-forgotten paths and accept the field from the far end; a frame with another type ends the session (the shared malformed matrix's wrong-`awaited` case).
- Deployers: upgrade every protocol-5 end together; 0.23 prerelease builds are not compatible with the field.

#### Verification

```sh
cargo test -p rutis-bridge --all-features
TLA2TOOLS=/tmp/tla2tools.jar specs/cross-runtime-sync/run.sh async-policy-b async-forgotten-policy-b
node tools/check-guarantees.mjs
```

Rust regressions: `an_asynchronous_forward_nobody_awaits_is_an_entry` (fired and forgotten -> entry), `an_awaited_forward_closes_the_cycle` (awaited later -> the real cycle is refused), `a_forwarded_call_carries_the_callers_awaited_signal` (a forward keeps the original signal), `a_call_says_when_its_result_is_not_waited_for` (the wire default and `false`).
