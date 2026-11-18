# Session trace contract: evidence for frames, handshakes and recovery

[中文](formal-trace.md)

Status: current implementation trace baseline; validator, conformance and mutation evidence pending. Date: 2026-11-08.
Issue: [#635](https://github.com/arcships/rutis/issues/635), the protocol-foundations part of [#183](https://github.com/arcships/rutis/issues/183). Related implementation and verification work: [#613](https://github.com/arcships/rutis/issues/613), [#624](https://github.com/arcships/rutis/issues/624), [#631](https://github.com/arcships/rutis/issues/631), [#639](https://github.com/arcships/rutis/issues/639). This document is the current, executable-facing trace contract; it records what a trace validator must accept and what it must reject.

A trace is an ordered sequence of protocol frames plus channel-end observations. Payload values are not logged by the channel tracer: `crates/rutis-bridge/src/channel/trace.rs:1-3` says that traces contain direction, length and ending, never message content (configuration can contain secrets). A protocol trace validator may decode the frame bytes in a test fixture, but production channel traces must continue to use the size-only form.

## 1. Scope and evidence

The session is a reliable, ordered, duplex stream of opaque messages (`crates/rutis-bridge/src/channel/mod.rs:4-9`). A `Channel` preserves message boundaries; the session protocol gives each message one JSON `op` and uses endpoint-prefixed call ids. Protocol version 5 is `ENDPOINT_PROTOCOL` (`crates/rutis-bridge/src/session/mod.rs:9-15`), and the same value is used by the WebSocket subprotocol (`rutis.5`).

The protocol frame vocabulary is the Rust `Frame` enum (`crates/rutis-bridge/src/session/protocol.rs:154-222`): `hello`, `invoke`, `call`, `get`, `await`, `return`, `throw`, `release`, and `cancel`. Wire values are tagged (`undefined`, `data`, `list`, `record`, `signal`, `reference`, and negotiated value tags); an unknown field is handled at its frame's scope (§3.7).

A trace case below has four parts:

1. **Stimulus** — the exact frame or transport event supplied by the test.
2. **Accepted trace** — the exact ordering and state transition that is allowed.
3. **Rejected trace** — the first violation and its failure scope.
4. **What developers see** — output copied word for word from the implementation tests or public implementation comments.

### Rule status

The status vocabulary is deliberately strict. **Current** means that the rule is the one implemented and tested by the anchors in this document. **Superseded** means an older rule or trace shape must not be used as evidence for protocol 5. **Not guaranteed** means that the implementation intentionally makes no promise; a trace may occur on one transport, but users and validators must not rely on it.

| Category | Current rule | Superseded evidence | Not guaranteed |
| --- | --- | --- | --- |
| Session frames | The protocol-4 `Frame` vocabulary and ordered handling in §3.1 | Pre-protocol-4 frame names or payload-dependent channel logs | A particular scheduler interleaving beyond channel order |
| Handshake | Mutual `hello`, version 4, endpoint check and capability declarations | A handshake that registers/mounts plugins before `Greeted` is mounted | Successful calls before `ready` |
| References | Tagged references, home identity, counted `release` | The old assumption that every reference is copied data; choosing the far end by the `objects` capability (0.18) | Reference lifetime keeping a runtime process alive (Not guaranteed) |
| Cancellation | `cancel` is hop-by-hop; late replies are discarded | Treating cancellation as a transport close | A deadline for calls (none is promised here) |
| Errors | `Failure` graphs and coded call/session scopes | Matching retry or behaviour by error text | A stable platform-specific transport reason for malformed input |
| Limits | Per-call value/message refusal and explicit outbound backlog limit | Sending an oversized frame and then continuing the call | An unbounded backlog when `outbound_limit` is `None` |
| Malformed input | Fail closed for malformed/dangling frames; unknown fields fail at their frame scope (`hello` fails the handshake, `release`/`cancel` ignore them), and a racing unknown `cancel` is tolerated | Silently ignoring unknown fields or operations | Forward compatibility for arbitrary new fields |
| Network/reconnect | Type-based classification, backoff and replacement ordering | Retrying incompatible protocol/identity failures as ordinary transient failures | Exact wall-clock reconnect timing when jitter is enabled |

Trace evidence is not a replacement for model or implementation evidence. It answers whether a real execution belongs to the current allowed trace language. A passing trace does not prove untested paths, an unbounded state space, or arbitrary plugin code.

## 2. Trace event format

The stable channel-level event text is produced by `trace()` (`crates/rutis-bridge/src/channel/trace.rs:12-31,41-79`):

```text
<label>: sent <n> bytes
<label>: received <n> bytes
<label>: ended
<label>: send failed: <reason>
<label>: receive failed: <reason>
<label>: closed: done
<label>: closed: replaced
<label>: closed: rejected: <reason>
```

The test fixes the output exactly and verifies that secrets do not appear (`crates/rutis-bridge/src/channel/trace.rs:104-133`):

```text
memory peer mac: sent 18 bytes
memory peer mac: received 18 bytes
memory peer mac: ended
memory peer mac: closed: done
```

A validator therefore compares event kind, direction, label and byte length, but never expects a payload in a production trace. `Close::Done`, `Close::Replaced`, and `Close::Rejected(reason)` have transport mappings in `crates/rutis-bridge/src/channel/mod.rs:57-75`: WebSocket close codes 1000, 4002 and 4003 respectively; memory channels expose normal end, `Closed`, and `Rejected`; process pipes expose normal channel end for all three.

## 3. Nine trace categories

Each category includes a success, failure and boundary case. The examples are current executable anchors, not hypothetical protocol prose.

### 3.0 Session and row lifecycle

**Success — greeting, mount, and active row.** `Session::handshake` returns a `Greeted` session without registering plugins; the handshake-only fixture verifies that nothing is mounted or shared until the greeted value is mounted (`crates/rutis-bridge/tests/session_plugin.rs:224-255`). A replacement mounts only after the old session unloads (`crates/rutis-bridge/tests/session_plugin.rs:257-270`); an ordinary reconnect pair reaches `FiberState::Active` at its local end (`crates/rutis-bridge/tests/session_plugin.rs:215-222`). A runtime row is loaded through `rows.schema`/`rows.load`; on the load-success path, its services are announced before the load completes and listener registrations are awaited in Node, Bun, Python, and Go (`node/rutis-runtime/src/runner.mjs:706-715`; `bun/rutis-bun/src/runner.ts:424-429`; `python/rutis/rutis/runner.py:676-689`; `go/rutis/internal/runtime/runtime.go:559-570`). This is a sequencing fact, not a uniform cross-runtime failure policy.

**Failure — unload and remote departure.** `rows.unload` is idempotent; a loaded row is disposed and its services are withdrawn (`crates/rutis-bridge/src/session/plugin/rows.rs:197-207`; `crates/rutis-bridge/src/runtime/process.rs:1352-1365`). If the far end leaves, the session is disposed, a call already in flight fails, the remote plugin is removed, and its service is absent (`crates/rutis-bridge/tests/session_plugin.rs:750-772`). A row that ends on its own reports `rows.ended` only when the session is not being cancelled; a row that fails reports `rows.failed` once per failed state (`crates/rutis-bridge/src/session/plugin/rows.rs:137-171`).

**Boundary — reconnect is a new session.** The old session and its rows are disposed before a new pair becomes active; a handshake-only session shares no plugins or services, and replacement mounts only after the old session unloads (`crates/rutis-bridge/tests/session_plugin.rs:182-270`). The normal channel result is `Ok(())`/`Ok(None)` for the transport thread, while the local session state is separately `Disposed`; those observations must not be merged into one handshake result.

Lifecycle trace status: the session/row sequencing above is current implementation evidence. It does not assert a single cross-runtime row-failure policy; the current row rules (load, update, unload, services and leases, ending and failing on one's own) are in the [row protocol](protocol-rows.en.md). Announcement errors are handled differently by the SDKs, as recorded in the service-announcement design. Object references are not negotiated by capability: protocol 5 has no `objects` capability ([session protocol specification](protocol-session.md) `SESS-16`, `SESS-53`), and every protocol-5 implementation accepts object references (`SESS-52`); the #639 finding that a Rust relay does not check `objects` has no reachable check point in protocol 5, and far ends of 0.18 or earlier are already refused at the handshake (`SESS-79`).

### 3.1 Frames

**Success — ordered request and reply.** A call is an `invoke` frame with `id`, `path`, `target`, `method`, `args`, and optional `sync`; a result is `return` with the same `id`, or `throw` with a `Failure` (`crates/rutis-bridge/src/session/protocol.rs:168-176,205-212`). The Go recorder documents the ordering guarantee: “The session handles what it receives one frame at a time, in order” (`go/rutis/internal/peer/peer_test.go:55-58`).

Accepted trace:

```json
{"op":"invoke","id":"main:1","path":[],"target":"x","method":"echo","args":{"type":"list","value":[]}}
{"op":"return","id":"main:1","value":{"type":"data","value":"ok"}}
```

**Failure — reply for an unknown call.** `return`/`throw` for an id this side does not own is a transport/protocol failure, not a call-level error. The Rust case `malformed_and_dangling_frames_close_the_session` includes “reply to an unknown call” (`crates/rutis-bridge/src/session/rpc/tests.rs:268-326`) and requires `Error::Transport(_)`; the next local invocation also fails because the session is closed.

**Boundary — `cancel` races its reply.** A `cancel` for an id not currently present is deliberately ignored (`crates/rutis-bridge/src/session/rpc/tests.rs:329-359`), because it may cross the reply it cancels. This is the only dangling-id exception in the frame protocol. A later valid invocation remains usable.

### 3.2 Handshake

**Success — mutual protocol-4 greeting.** Each side sends `hello` with `version`, endpoint, implementation (when available), and capabilities (`crates/rutis-bridge/src/session/protocol.rs:157-167`). The Go raw peer sends `{"op":"hello","version":5,"endpoint":"main"}` and waits for the runtime's hello (`go/rutis/internal/peer/objects_test.go:35-51`). The ordinary pair starts both peers and waits for both `Ready()` results (`go/rutis/internal/peer/peer_test.go:70-89`). No plugin is mounted merely by greeting: `Session::handshake` returns a `Greeted` value and registers nothing (`crates/rutis-bridge/src/session/plugin/mod.rs:102-114,178-180`).

**Failure — version mismatch.** A far end speaking version 3 produces the exact local session failure `the far end speaks protocol 3, this side 4: upgrade the far end`. The fixture separately observes the local side's hello and the far-end thread's normal channel result `Ok(())`; that `Ok(())` is the transport thread's close result, not a successful handshake (`crates/rutis-bridge/tests/session_plugin.rs:101-126`). A handshake that does not finish within 10 seconds fails with `the far end did not greet in time` and the channel ends (`crates/rutis-bridge/tests/session_plugin.rs:128-152`; timeout is defined at `crates/rutis-bridge/src/session/plugin/mod.rs:46-47`). A host without `LocalId` fails with `the host provides no id for its sessions` and sends no greeting (`crates/rutis-bridge/tests/session_plugin.rs:154-167`).

**Boundary — capabilities are declarations.** `Greet` declares `node` and the configured capabilities in its greeting (`crates/rutis-bridge/src/session/plugin/greet.rs:31-36`), but does not check whether the far end declares what this end needs: no implementation offers an entry point for a "required capabilities" list. Rust had a `require` field whose only production call passed an empty list; it is removed. Go's `peer.Options.Require` sits in `internal/peer` with no caller; the other implementations have no such check. When the far end lacks a capability, only the place that uses it fails; a capability declaration grants no permission by itself (`crates/rutis-bridge/src/session/rpc.rs:1153-1158`) and is not verified.

### 3.3 References

**Success — grant, identity and return home.** A reference is tagged `{type:"reference", value:{id, home, kind, origin}}` (`crates/rutis-bridge/src/session/protocol.rs:42-47`). Sending the same object twice gives one proxy and returning it to its home gives the original value: `TestAnObjectFromTheFarEndIsCalledAndGoesHome` asserts `home == true`, identity of two received proxies, and `call` frames naming `method` (`go/rutis/internal/peer/crossing_test.go:96-150`).

**Failure — unknown reference.** `call`, `await`, or `release` naming a reference not held by this session closes the session with a transport error (`crates/rutis-bridge/src/session/rpc/tests.rs:296-306,312-326`). This is a malformed peer trace, not a recoverable `no-method` call error.

**Boundary — release count and session close.** Release count is allowed to remove the export; an over-release is fail-closed, and session close clears exports (`crates/rutis-bridge/src/session/rpc/tests.rs:240-266`). A normal release followed by another invocation is observable in Go: the export count drops by one while the session remains open (`go/rutis/internal/peer/objects_test.go:71-103`).

### 3.4 Cancellation

**Success — caller cancellation propagates.** The frame is `{"op":"cancel","id":...}` (`crates/rutis-bridge/src/session/protocol.rs:217-221`). In `TestAnObjectHereIsCancelledThroughAndReleased`, the far end sends `call`, then `cancel`; the object's method observes context cancellation, a reply for the cancelled id can still arrive, and a subsequent `invoke` succeeds (`go/rutis/internal/peer/objects_test.go:66-103`).

**Failure — late reply is discarded, not session-fatal.** The Go test states that a method may reply late: the host receives and drops that reply, then handles the next call in order (`go/rutis/internal/peer/peer_test.go:135-170`). The caller gets its context error, while the session remains open.

**Boundary — cancel crossing reply.** As above, an unknown `cancel` is ignored (`crates/rutis-bridge/src/session/rpc/tests.rs:329-359`). It must not close a session merely because the reply won the race. Cancellation does not undo a reference grant already committed; the receiver still sends `release` when its proxy is released (`go/rutis/internal/peer/objects_test.go:129-147`).

### 3.5 Errors

**Success — coded error graph round-trips.** `Failure` carries `name`, `message`, and optional `graph` (`crates/rutis-bridge/src/session/protocol.rs:87-94`). A rutis `no-method` error crosses as `BindingError` with code `no-method` and its `data`; `bad-value` and transport errors similarly become coded failures (`crates/rutis-bridge/src/session/protocol.rs:258-307`). The exact no-method message used by the unit test is `weather has no method tomorow (methods: today, tomorrow)` (`crates/rutis-bridge/src/session/protocol.rs:269-290`).

**Failure — an error belongs to one call.** A value that cannot cross is rejected before its frame is sent, with code `bad-value`; only that call fails. The next call succeeds and no `invoke` for the rejected method is recorded (`go/rutis/internal/peer/crossing_test.go:29-64`). For a result over the limit, the exact prefix is `conformance.big: the result cannot be sent: the message would be ` and the error data is `{"limit":16777216,"part":"result"}` (`go/rutis/internal/peer/crossing_test.go:66-94`).

**Boundary — malformed error graph.** A malformed graph is represented as the received `TypeError` with message `bad graph`; it can be thrown on again and does not end the session (`go/rutis/internal/peer/crossing_test.go:199-220`). This is deliberately narrower than a malformed *frame*: malformed frame syntax or a dangling id closes the session (the Rust matrix at `crates/rutis-bridge/src/session/rpc/tests.rs:268-326`).

### 3.6 Limits

**Success — a message at the limit.** Session encoding checks the channel's `max_message()` before sending (`crates/rutis-bridge/src/channel/mod.rs:32-40`; call encoding and the “fails only this call” rule are in `crates/rutis-bridge/src/session/rpc.rs:1892-1905`). A message that fits is queued in order and the call can receive its reply.

**Failure — a value over 16 MiB.** The exact Go expectation is `svc.take: argument 2 cannot be sent: the message would be over the limit of 16777216 bytes (16 MiB)` with code `bad-value` and data `{"argument":2,"limit":16777216,"part":"argument"}` (`go/rutis/internal/peer/crossing_test.go:18-64`). Nothing reaches the host, the callback export is released, and the following `after` call returns `taken`. This proves the failure scope is one call, not one session.

**Boundary — outbound backlog.** `SessionOptions::outbound_limit` bounds queued bytes behind the one frame already held by the writer; an empty queue accepts one frame even when the limit is `0`, and the frame already in `send` is not counted (`crates/rutis-bridge/src/session/rpc.rs:1186-1213`). The exact fixture with 101-byte invoke frames and a limit of 202 bytes is `the far end is not reading: 2 messages, 202 bytes waited for it` (`crates/rutis-bridge/src/session/rpc/tests.rs:618-678`); the zero-limit fixture is `the far end is not reading: 1 messages, 101 bytes waited for it` (`crates/rutis-bridge/src/session/rpc/tests.rs:692-741`). In the general rule the numbers are parameterized by queued frame count and bytes. The transport error has code `peer-gone`; queued messages go with it.

### 3.7 Malformed input

**Success — tolerated cancellation race.** The only malformed-looking/dangling input tolerated is an unknown `cancel`, for the reply-race reason (`crates/rutis-bridge/src/session/rpc/tests.rs:329-359`). A valid subsequent `invoke` receives its normal result.

**Failure — fail closed on malformed or dangling frames.** The Rust table covers malformed JSON, empty bytes, a leading BOM, invalid UTF-8, a non-object, unknown `op`, wrong field type, missing field, unknown wire value, call/await/release of unknown references, and reply to an unknown call (`crates/rutis-bridge/src/session/rpc/tests.rs:268-326`). Every case waits for `peer.closed()`, requires `Error::Transport(_)`, and verifies a later invocation is rejected. This prevents an attacker or buggy peer from turning an invalid message into an ambiguous partial state.

**Boundary — unknown fields are not an extension mechanism, but no longer end the session.** They are handled at the frame's scope: `hello` fails the handshake; `invoke`/`call`/`get`/`await` fail only that call (`bad-frame`); `return`/`throw` fail only the call waiting for it; `release`/`cancel` ignore them. Implementation: `parse_frame` / `reject_unknown_fields` in `crates/rutis-bridge/src/session/rpc.rs`; tests `unknown_frame_fields_fail_only_their_call` and `an_unknown_field_in_hello_fails_the_handshake`. A new field still requires a protocol specification/version or an explicitly compatible decoder; silently ignoring it is not part of the current trace language.

### 3.8 Network and reconnect

**Success — transport end is classified and retry state resets.** Dial retry parsing preserves defaults when an override is malformed; a valid `100,400,60000,300` override changes initial, maximum, stable and rejected delays while retaining jitter `0.2` and handshake `10s` (`go/rutis/internal/runtime/dial_test.go:25-41`). Backoff emits exactly `0.5s 1s 2s 4s 8s 16s 30s 30s 30s`, then reset returns to `500ms`; jitter bounds are tested at `400ms` and `600ms` (`go/rutis/internal/runtime/dial_test.go:43-68`). Session failure classification is by error type, not text: rejected → `authRejected`, incompatible → `incompatible`, replaced/host-gone → `retryable` (`go/rutis/internal/runtime/dial_test.go:84-101`).

**Failure — incompatible or rejected connections do not spin.** A far end with the wrong protocol is classified `incompatible` and retry stops until configuration changes; identity mismatch/authentication is `authRejected` and is retried slowly while reporting. The classification table and its exact cases are at `go/rutis/internal/runtime/dial_test.go:84-101`; it explicitly includes a misleading text case to prove decisions are not made by matching words.

**Boundary — replacement ordering.** The host listener admits a token with WebSocket subprotocol `rutis.5` (`crates/rutis-host/src/connect/listen.rs:53-80`). For an id already connected, a newer connection replaces the older only after its handshake: the old session unloads, closes as `Replaced` (WebSocket 4002), and only then is the new session installed; a handshake failure touches nothing (`crates/rutis-host/src/connect/listen.rs:4-10`). A disconnected or gone peer therefore cannot apply late frames to the replacement session.

## 4. Trace invariants and counterexamples

A validator must assert these invariants across all categories:

- **Ordering:** frames on one channel are handled one at a time and in arrival order (`go/rutis/internal/peer/peer_test.go:55-58`).
- **Handshake gate:** no call frame is accepted before `ready`; `Connection::send` returns `Transport("protocol handshake incomplete")` (`crates/rutis-bridge/src/session/rpc.rs:1844-1848`).
- **Call scope:** encoding/value/limit errors fail only the call; malformed frame, dangling reference/call, and transport failure close the session.
- **Identity:** endpoint-prefixed ids identify the originating session; forwarding rebases the chain (`crates/rutis-bridge/src/session/rpc.rs:1700-1734`) so a late old-session id cannot name a new session's call.
- **Cancellation:** cancellation reaches the executing call; a late reply is discarded; cancellation does not close the session.
- **Reference conservation:** grants are paired with releases; session close clears outstanding exports/imports.
- **Secret safety:** channel trace output contains sizes and reasons, never message payloads (`crates/rutis-bridge/src/channel/trace.rs:104-133`).
- **Replacement isolation:** old-session close completes before the new hello is accepted for the same id (`crates/rutis-host/src/connect/listen.rs:4-10`).

Deliberate mutations must be caught: accept an unknown `op`, make an unknown call field end the session instead of failing its call, treat an unknown `cancel` as fatal, send an over-limit frame instead of failing its call, or install the replacement before unloading the old session. The malformed-frame matrix, unknown-field tests, over-limit test and replacement implementation are the regression evidence for those mutations.

## 5. What developers see

The protocol contract adds no plugin API, host command or configuration field. It does make existing output and trace evidence explicit.

### Plugin authors

**Success:** existing calls, objects, cancellation and errors behave as before; no payload is printed in channel traces. For an ordinary successful call, no new line is emitted to the plugin.

**Failure (verbatim):**

- A protocol-3 peer: `the far end speaks protocol 3, this side 4: upgrade the far end`.
- A peer that never greets: `the far end did not greet in time`.
- A missing host identity: `the host provides no id for its sessions`.
- A value over the limit: `svc.take: argument 2 cannot be sent: the message would be over the limit of 16777216 bytes (16 MiB)`; code `bad-value`; data `{"argument":2,"limit":16777216,"part":"argument"}`.
- A call frame carrying an unknown field: `unknown field "x" in invoke`; code `bad-frame`; data `{"op":"invoke","fields":["x"]}`; only that call fails. A field inside a value is written as a path such as `args.value.x`.
- A greeting carrying an unknown field: `incompatible session: unknown field "x" in hello`; the handshake fails and the session ends.
- A closed channel during a call: the existing transport error, whose code is `peer-gone`; no new protocol text is invented here.

Each call-level failure is fixed in the call or value. A handshake/transport failure is fixed by matching protocol/capabilities, restoring the peer, or correcting deployment; malformed input is a peer implementation defect.

**Concepts to learn:** none. The trace format is contributor evidence, not a plugin API.

### Host authors and deployers

**Success:** the existing `rutis-host` commands, status lines, close codes and retry policy remain unchanged. A normal traced channel records, word for word:

```text
memory peer mac: sent 18 bytes
memory peer mac: received 18 bytes
memory peer mac: ended
memory peer mac: closed: done
```

**Failure (verbatim):**

- Wrong protocol: `the far end speaks protocol 3, this side 4: upgrade the far end`.
- A rejected identity is a `Close::Rejected(reason)` / WebSocket close code `4003`; an incompatible transport is classified `incompatible`; a replaced connection uses code `4002` (`crates/rutis-bridge/src/channel/mod.rs:57-75`).
- Retry override malformed: it is ignored and defaults are restored (`go/rutis/internal/runtime/dial_test.go:36-39`); no new warning line is promised.

**Concepts to learn:** none for users; deployers may use the existing retry categories `retryable`, `authRejected`, and `incompatible` when reading status.

### Runtime authors and contributors

**Success:** run the existing Rust/Go tests and compare trace events without payloads. The channel tracer's exact success output is the four lines above (`crates/rutis-bridge/src/channel/trace.rs:104-133`).

**Failure (verbatim):** malformed/dangling input closes with an `Error::Transport(_)`; the tests intentionally do not promise a platform-specific reason sentence (`crates/rutis-bridge/src/session/rpc/tests.rs:312-326`). A protocol-version failure is `the far end speaks protocol 3, this side 4: upgrade the far end`.

**Concepts to learn:** frame, trace event, allowed trace, call-level failure, session-level failure, and replacement isolation. These are verification vocabulary only; none enters the runtime API.

## 6. Compatibility, migration and implementation

Protocol version 4, frame names, capabilities, reference representation, error graphs, cancellation and close semantics are current. This document does not bump the wire version or add a field. Existing peers that speak another version fail the handshake with the exact mismatch output above; they do not exchange calls.

Implementers adding a frame or field must update `Frame`/`WireValue`, this document, the malformed-input matrix, and every applicable conformance implementation together. An unknown field fails at its frame's scope (§3.7); a compatibility change must be deliberate and versioned, not silently accepted. Implementers adding reconnect logic must preserve type-based classification, backoff reset, and replacement ordering.

Completion for #635 is evidence-backed when every nine-category case above has a validator or conformance test in each applicable implementation, every unsupported capability/platform is marked explicitly, and deliberate mutations fail. Current anchors include Rust lifecycle/session tests (`crates/rutis-bridge/tests/session_plugin.rs:169-270,750-772`), Rust session tests (`crates/rutis-bridge/src/session/rpc/tests.rs:268-359`), Rust handshake tests (`crates/rutis-bridge/tests/session_plugin.rs:101-167`), the Go reference/cancellation/limits tests (`go/rutis/internal/peer/crossing_test.go:29-64`, `go/rutis/internal/peer/objects_test.go:66-147`), and retry tests (`go/rutis/internal/runtime/dial_test.go:25-101`).

## Related files

- [Formal verification design](design-formal-verification-2026-11-08.en.md)
- [Remote session design](design-remote-session-2026-10-28.en.md)
- [Object references](design-object-refs-2026-10-27.en.md)
- [Error messages](design-error-messages-2026-11-03.en.md)
- `crates/rutis-bridge/src/channel/trace.rs`
- `crates/rutis-bridge/src/session/protocol.rs`
- `crates/rutis-bridge/src/session/rpc/tests.rs`
