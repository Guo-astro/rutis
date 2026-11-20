# Session Protocol Specification: Protocol 5

[中文](protocol-session.md)

Status: current specification. Date: 2026-11-19 (first version 2026-11-09; 2026-11-18 added protocol 5: the `ack` frame, `SESS-57`, `SESS-61`, §8 and G3; this revision adds the `chunk` frame and the `chunked` capability: `SESS-84`–`SESS-88`, §2.5 and G8).
Issue: [#635](https://github.com/arcships/rutis/issues/635), part of step 4 of [#183](https://github.com/arcships/rutis/issues/183); the work plan is [the 0.22 formal coverage plan](plan/formal-verification-0.22.en.md) WS2. Trace and verification vocabulary: [session trace contract](formal-trace.en.md).

This is the only current specification of the session layer: what two ends exchange on one channel, what happens on errors and cancellation, which messages fail only one call and which end the session. Rows, service announcement, events, authentication, reconnect backoff and runtime supervision are out of scope; each has its own current specification: the [row protocol](protocol-rows.en.md) for rows, host-service leases, and a row ending or failing; the [events protocol](design-events-protocol-2026-10-18.en.md) for events; the [remote lease model](../specs/remote-lease/README.en.md) and the [remote session design](design-remote-session-2026-10-28.en.md) for remote leases and reconnection. This file is also the source from which #634's trace table extracts its `session` partition.

Every rule has an id (`SESS-*`) and one of three states:

| State | Meaning |
| --- | --- |
| **Current** | Executed by the implementations and tests named here; a code location or test anchor is given. |
| **Superseded** | A rule of an older design, issue or protocol version; it is no longer evidence. |
| **Not guaranteed** | Deliberately not promised; callers and verifiers must not rely on it. |

The rules are the contract. Error **text** is a contract only where this file quotes it word for word; the same kind of error may read differently in different implementations (`SESS-77`).

## 1. Scope and transport

- `SESS-1` **Current.** A session runs on one [`Channel`](https://github.com/arcships/rutis/blob/main/crates/rutis-bridge/src/channel/mod.rs): one ordered, reliable, duplex stream of messages that keeps message boundaries and moves opaque bytes. The channel knows nothing of frames, calls or references. `crates/rutis-bridge/src/channel/mod.rs:1-9,20-48`
- `SESS-2` **Current.** Every message is a UTF-8 JSON object tagged by `op`; the session layer encodes, decodes and maps errors. Reads happen in arrival order, one frame at a time. `crates/rutis-bridge/src/session/rpc.rs:1557-1570,3207-3216`
- `SESS-3` **Current.** The protocol version is 5 (`ENDPOINT_PROTOCOL`). Rust, Node, Bun, Python, Go, Swift and Kotlin use the same constant; the WebSocket subprotocol is `rutis.5`. `crates/rutis-bridge/src/session/mod.rs:9-14`; `node/rutis-runtime/src/session.mjs:58`; `bun/rutis-bun/src/session.ts:57`; `python/rutis/rutis/peer.py:42`; `go/rutis/internal/peer/peer.go:34`; `swift/rutis/Sources/Rutis/Session/Peer.swift:13`; `kotlin/rutis/rutis/src/shared/kotlin/arcships/rutis/internal/peer/Peer.kt:42`; `crates/rutis-host/src/connect/listen.rs:74,408`
- `SESS-4` **Current.** A session has two endpoint ids (each side's self-named `endpoint`), and each side's call ids carry its own id as a prefix. An id is unique in a deployment; the deployment guarantees it. A new connection with the same id replacing the old one is a connection-layer rule, not this file. `crates/rutis-bridge/src/channel/mod.rs:77-109`; `crates/rutis-bridge/src/session/rpc.rs:1493-1499,2443-2489`
- `SESS-5` **Current.** Reconnecting makes a new session. The old session's calls, references and late frames do not enter the new session; no old state carries over. `docs/formal-trace.en.md` §3.0; `crates/rutis-bridge/tests/session_plugin.rs:182-270`
- **Superseded:** a session may use either of two wire formats, a "compat" format and an "endpoint" format. Protocol 4 has one format (§2.3).

## 2. Frames and wire format

### 2.1 Frames

`SESS-6` **Current.** The frame vocabulary is the ten `Frame` variants; `op` is snake_case, and an unknown `op` ends the session. The fields below are the fields; anything not listed does not exist (§9). One more frame, `chunk`, is read by the session's read layer before `Frame` decoding and carries a frame over the transport's limit (§2.5). `crates/rutis-bridge/src/session/protocol.rs:149-230`; `crates/rutis-bridge/src/session/chunks.rs`

| Frame | Fields | Meaning |
| --- | --- | --- |
| `hello` | `version: u32`; `endpoint?: string`; `implementation?: {name, version}`; `capabilities?: string[]` | The opening greeting. `version` is required and the other three may be absent; receivers require a valid `endpoint` (§4). |
| `invoke` | `id: string`; `path: string[]`; `target: string`; `method: string`; `args: WireValue`; `sync?: bool`; `awaited?: bool` | Call a service method. An empty `target` means a control operation (`events.*`, `rows.*`, …). |
| `call` | `id`; `path`; `reference: u64`; `method?: string`; `args: WireValue`; `sync?: bool`; `awaited?: bool` | Call a function reference; with `method`, call a method of an object reference. |
| `get` | `id`; `path`; `reference: u64`; `property: string`; `sync?: bool`; `awaited?: bool` | Read a live property of an object reference. |
| `await` | `id`; `path`; `reference: u64`; `sync?: bool`; `awaited?: bool` | Await an async result reference. |
| `return` | `id: string`; `value: WireValue` | The result of a call. |
| `throw` | `id: string`; `error: Failure` | The error of a call. `Failure` is `{name: string, message: string, graph?: JSON}`. |
| `release` | `reference: u64`; `count: u64` | Return a number of reference grants (§6). |
| `cancel` | `id: string` | The caller gives up call `id` (§7). |
| `ack` | `id: string` | The callee resolved the cancel of call `id`: it will send no `return`/`throw` for it (§7). |
| `chunk` | `transfer: string`; `seq: u64`; `last: bool`; `data: string` | One slice of a frame (§2.5). Not a `Frame` variant; read before `Frame` decoding. |

`SESS-7` **Current.** `sync` defaults to `false` and is not written when `false`. `awaited` defaults to `true` and is not written when `true`: only a call that does not wait for its result writes `awaited: false` (`SESS-83`). `hello`'s `endpoint`, `implementation` and `capabilities`, and `throw`'s `graph`, are not written when absent. `crates/rutis-bridge/src/session/protocol.rs:157-221`

`SESS-8` **Current.** Each `path` entry is a call id: `<endpoint>:<n>`, or a tagged entry after forwarding (§5.2). A `path` is the frame's call chain, not a free field. `crates/rutis-bridge/src/session/rpc.rs:2739-2742`; [session tags design](design-session-tags-2026-10-12.en.md) §2

### 2.2 Values

`SESS-9` **Current.** `WireValue` is a union tagged by `type` with its content in `value`; the table lists every tag. `crates/rutis-bridge/src/session/protocol.rs:26-85`

| `type` | `value` | Meaning |
| --- | --- | --- |
| `undefined` | none | A value not given. |
| `data` | any JSON | JSON data. |
| `list` | `WireValue[]` | A list. |
| `record` | `{string: WireValue}` | A plain object whose fields may hold references. |
| `signal` | none | The `AbortSignal` of this call; aborted when the call is cancelled. |
| `reference` | `{id: u64, home: bool, kind: one of function, future, object, plugin, origin: string[]}` | A function, async result, object or plugin definition (§6). |
| `instant`, `datetime`, `date`, `time`, `bytes`, `map`, `set`, `bigint`, `decimal`, `uuid` | their own text or structure | The ten tagged values; the wire formats are in [protocol plugin design](design-protocol-plugins-2026-09-25.en.md) §6.1. |

`SESS-10` **Current.** As a whole argument or a whole result, `undefined` stays `undefined`; at any depth inside a value it is `null`. `crates/rutis-bridge/src/session/rpc.rs:2356-2359,2567-2573`; `bridge/object-refs` in `docs/guarantees.en.md`

`SESS-11` **Current.** A tagged value is sent only to a far end that declared its capability `value-<tag>`; when the far end did not, the call carrying it fails before it is sent and the session goes on (§2.4, §10). `crates/rutis-bridge/src/session/rpc.rs:2204-2214,2298-2324`

### 2.3 Formats

`SESS-12` **Current.** There is one wire format: the endpoint format, protocol 5. A call id is always `<endpoint>:<n>`; there are no call ids without an endpoint prefix, and no second `hello`. `crates/rutis-bridge/src/session/mod.rs:9-14`; [migration to 0.19](migration-0.19.en.md) "Upgrading together"; `crates/rutis-bridge/src/session/rpc.rs:1493-1499,2456-2462`

`SESS-13` **Superseded.** The two formats of issue #635 and older designs: compat format 2 (no endpoint, `node:n` / `rust:n` call ids) and endpoint format 3. Since 0.19, protocol 4 has only the endpoint format; far ends of 0.18 and earlier are refused at the handshake and are fixed by upgrading every end to 0.19. `docs/migration-0.19.en.md` "Upgrading together"; `crates/rutis-bridge/src/session/rpc.rs:2456-2462`

`SESS-14` **Superseded.** "`sync` is sent only to a far end that declared `sync-wait`." Protocol 5 has no `sync-wait` capability; `sync` is a frame field every protocol-5 implementation knows, serialized as in §2.1.

### 2.4 Capabilities

`SESS-15` **Current.** A capability is a string in `hello.capabilities`; it declares what an end can do, not a permission. The session layer reads four kinds: `value-<tag>` (whether a value kind may be sent), `reentrant-sync` and `sync-stack` (the runtime's shape in the synchronous wait graph), and `chunked` (whether it reassembles `chunk` frames, §2.5). The plugin and mount layers read `node`, `runtime` and `plugins`. A capability is a declaration only: there is no "required capabilities" list, and a missing capability is handled by the layer that uses it (`SESS-25`). `crates/rutis-bridge/src/session/rpc.rs:1154-1178,1590-1611,2204-2214`; `crates/rutis-bridge/src/session/plugin/greet.rs:31-36`; `crates/rutis-bridge/src/session/plugin/mod.rs:352,368`; `bridge/sync-wait-cycle-refused` in `docs/guarantees.en.md`

| Capability | Declared by | Meaning |
| --- | --- | --- |
| `reentrant-sync` | Rust, Python, Go, Swift, Kotlin | Synchronous calls may re-enter on the same execution stack; every incoming call has its own thread or stack. |
| `sync-stack` | Node, Bun, Python | One thread, one stack; a synchronous wait stacks on that one stack. |
| `node` | The session plugin (the host-side `Greet`) adds it to its own declaration | This end is a full framework node. `crates/rutis-bridge/src/session/plugin/greet.rs:34-36`; `crates/rutis-bridge/src/session/plugin/mod.rs:352` |
| `runtime` | Node, Bun, Python, Go, Swift and Kotlin runtime processes | This end is a runtime process a host may mount or dial. `node/rutis-runtime/src/runner.mjs:39`; `bun/rutis-bun/src/main.ts:118`; `python/rutis/rutis/__main__.py:72`; `go/rutis/internal/runtime/serve.go:99`; `swift/rutis/Sources/Rutis/Remote/App.swift:356`; `kotlin/rutis/rutis/src/shared/kotlin/arcships/rutis/RutisApp.kt:122`; `crates/rutis-bridge/src/session/plugin/mod.rs:369` |
| `plugins` | Rust; Node/Bun when hosting | This end takes plugin definition values. `crates/rutis-bridge/src/session/rpc.rs:918-922`; `bridge/plugin-values` in `docs/guarantees.en.md` |
| `value-<tag>` | Every implementation, ten kinds | This end takes that kind of tagged value. `crates/rutis-bridge/src/session/rpc.rs:904-917` |
| `chunked` | All implementations: Rust (`rutis-bridge`), Node, Bun, Python, Go, Swift and Kotlin | This end reassembles `chunk` frames, so a frame over the limit may arrive in slices (§2.5, `SESS-85`). `Endpoint::rust` in `crates/rutis-bridge/src/session/rpc.rs`; `CAPABILITIES` in `node/rutis-runtime/src/session.mjs`; `CAPABILITIES` in `bun/rutis-bun/src/session.ts`; `CAPABILITIES` in `python/rutis/rutis/peer.py`; `Capabilities` in `go/rutis/internal/runtime/serve.go` and `TestCapabilities` in `go/rutis/internal/peer/peer.go`; `runtimeCapabilities` in `swift/rutis/Sources/Rutis/Session/Peer.swift`; `SESSION_CAPABILITIES` in `kotlin/rutis/rutis/src/shared/kotlin/arcships/rutis/internal/peer/Peer.kt` |

`SESS-16` **Superseded.** The capabilities `sync-wait`, `events-v2` and `objects`. Protocol-4 implementations have no such constants; object references cross to every protocol-4 implementation (§6.2). `sync-wait` and `events-v2` belong to the negotiations of 0.18 and earlier; `objects` is the 0.18 object capability ([object references](design-object-refs-2026-10-27.en.md) §3). Protocol-4 implementations no longer declare or check any of the three.

`SESS-17` **Not guaranteed.** Unknown capability names are ignored without an error; a far end's declaration is not verified (one that declares but cannot do it fails only when actually called); no capability grants a permission.

### 2.5 A frame across chunks

`SESS-84` **Current.** A frame over the transport's per-message limit may be carried in a run of `chunk` frames. A `chunk` frame is not a `Frame` variant: the session's read layer takes it before `Frame` decoding. Its fields are `transfer: string` (`<sending endpoint>:<n>`, increasing from 1 within the session on the sending side, non-empty), `seq: u64` (from 0, one more for each slice), `last: bool`, and `data: string` (one slice of the frame's UTF-8 JSON bytes, base64, RFC 4648 §4, padded, canonical, no whitespace). The `data` of every slice, in order, is that message. `hello` is never chunked. A `chunk` with a field of the wrong type, an extra field, or a `seq` that does not start at 0 is a malformed frame (§8). `Chunk` in `crates/rutis-bridge/src/session/chunks.rs`; `#receiveChunk` in `node/rutis-runtime/src/session.mjs`; `#receiveChunk` in `bun/rutis-bun/src/session.ts`; `_receive_chunk` in `python/rutis/rutis/peer.py`; `receiveChunk` in `go/rutis/internal/peer/chunks.go`; `acceptChunk` in `swift/rutis/Sources/Rutis/Session/Peer.swift`; `receiveChunk` in `kotlin/rutis/rutis/src/shared/kotlin/arcships/rutis/internal/peer/Peer.kt`

`SESS-85` **Current.** The capability `chunked` declares that this end reassembles `chunk` frames. Only when both this side and the far end declared it does the sender cut an oversized frame into chunks; otherwise the over-limit error of §10 stands and nothing is written. All seven implementations of 0.23 declare it: Rust (`rutis-bridge`), Node, Bun, Python, Go, Swift and Kotlin. Earlier builds do not, so sessions with them keep the 16 MiB behaviour and never receive a `chunk` frame. The side that reads `chunk` requires the far end to have declared it: a `chunk` from a far end that did not is a transport error. `Endpoint::rust`, `send_limit` and `accept_chunk` in `crates/rutis-bridge/src/session/rpc.rs`; `CAPABILITIES` and `#chunking` in `node/rutis-runtime/src/session.mjs`; `CAPABILITIES` and `#chunking` in `bun/rutis-bun/src/session.ts`; `CAPABILITIES` and `_chunking` in `python/rutis/rutis/peer.py`; `chunkingLocked` in `go/rutis/internal/peer/chunks.go`; `runtimeCapabilities` and `chunks` in `swift/rutis/Sources/Rutis/Session/Peer.swift`; `SESSION_CAPABILITIES` and `chunking` in `kotlin/rutis/rutis/src/shared/kotlin/arcships/rutis/internal/peer/Peer.kt`

`SESS-86` **Current.** The sender cuts when an encoded frame is over the transport's per-message limit (Rust: `Sender::max_message()`; Node, Bun, Go and Swift: `maxMessage`; Python: `_max_message()`; Kotlin: `channelLimit`, each matching its channel) and both sides declared `chunked`: each slice is at most 8 MiB (8388608 bytes), shrunk to the transport's limit so one `chunk` frame fits one of its messages; a computed slice of 0 means no chunking. All `chunk` frames of one frame are consecutive in the outbound order, with no other frame between, and one transfer runs at a time. A frame over the session's `max_transfer` (256 MiB, 268435456 bytes, by default) is not sent; only that call fails as in `SESS-71`, with `limit` set to the configured value. `CHUNK_SIZE`, `slice_size` and `Chunk::frames` in `crates/rutis-bridge/src/session/chunks.rs`; `#transmit` in `node/rutis-runtime/src/session.mjs`; `#transmit` in `bun/rutis-bun/src/session.ts`; `_transmit` in `python/rutis/rutis/peer.py`; `writeChunkedLocked` in `go/rutis/internal/peer/chunks.go`; `write(_:chunks:)` in `swift/rutis/Sources/Rutis/Session/Peer.swift`; `transmit` in `kotlin/rutis/rutis/src/shared/kotlin/arcships/rutis/internal/peer/Peer.kt`

`SESS-87` **Current.** The receiver buffers by `transfer`: `seq` must start at 0 and increase by one, and a second `transfer` while one is in progress (at most one unfinished transfer), more than the session's `max_transfer` accumulated, `data` that is not canonical base64, a `last` that is not a bool or a field of the wrong type all end the session with a transport error (a `chunk` belongs to no call, so there is no smaller scope); a `chunk` before the greeting is `request before protocol handshake`. The frame reaches the session only after `last`, the join and the parse all succeeded; a reassembled frame has exactly the rights of one that arrived whole (depth, unknown fields, call ids, references and cancellation unchanged), and may not itself be a `chunk` frame. When the session ends, an unfinished transfer goes with it: never dispatched, never answered. `Reassembly::accept` in `crates/rutis-bridge/src/session/chunks.rs`; the reader thread and `accept_chunk` in `crates/rutis-bridge/src/session/rpc.rs`; `#receiveChunk` in `node/rutis-runtime/src/session.mjs`; `#receiveChunk` in `bun/rutis-bun/src/session.ts`; `_receive_chunk` in `python/rutis/rutis/peer.py`; `receiveChunk` in `go/rutis/internal/peer/chunks.go`; `acceptChunk` in `swift/rutis/Sources/Rutis/Session/Peer.swift`; `receiveChunk` in `kotlin/rutis/rutis/src/shared/kotlin/arcships/rutis/internal/peer/Peer.kt`

## 3. Frame order and processing

`SESS-18` **Current.** Frames on one channel are processed one at a time, in arrival order. Calls may run concurrently on their own tasks, threads or stacks; frame reads and call-id admission are serial. `crates/rutis-bridge/src/session/rpc.rs:1557-1570,2792-2839`; `go/rutis/internal/peer/peer_test.go:55-58`

`SESS-19` **Current.** `events.*` and `rows.ended` enter the session's ordered queue in frame order; table changes and `emit` run in that order, the other modes start in it. This is where the events protocol lands in the session; this file only says they do not break frame order. `crates/rutis-bridge/src/session/rpc.rs:1244-1283,2767-2884`

## 4. Handshake

`SESS-20` **Current.** Both ends write `hello` before they start reading. The send happens before the reader thread starts, so the far end always reads this end's version before the channel ends. `crates/rutis-bridge/src/session/rpc.rs:1524-1541`; test `crates/rutis-bridge/src/session/rpc/tests.rs::endpoint_format::issue_239_an_incompatible_side_greets_before_it_reads_the_far_greeting`

`SESS-21` **Current.** `hello` comes before everything. Any other frame received before the far end greeted (`return`, `throw`, `cancel`, `release` included) ends the session with a transport error reading `request before protocol handshake`. `crates/rutis-bridge/src/session/rpc.rs:2535-2536,2616-2618`; `go/rutis/internal/peer/peer_test.go::TestACallBeforeTheHandshakeEndsTheSession`; the same check in Node, Bun, Python, Swift and Kotlin is at `node/rutis-runtime/src/session.mjs:709`, `bun/rutis-bun/src/session.ts:696`, `python/rutis/rutis/peer.py:1682`, `swift/rutis/Sources/Rutis/Session/Peer.swift:1086`, `kotlin/rutis/rutis/src/shared/kotlin/arcships/rutis/internal/peer/Peer.kt:1120`

`SESS-22` **Current.** A second `hello` ends the session, reading `duplicate protocol handshake`. `crates/rutis-bridge/src/session/rpc.rs:2602-2614`; `node/rutis-runtime/src/session.mjs:705`; `python/rutis/rutis/peer.py:1675`; `go/rutis/internal/peer/peer.go:1382`; `swift/rutis/Sources/Rutis/Session/Peer.swift:1078`; `kotlin/rutis/rutis/src/shared/kotlin/arcships/rutis/internal/peer/Peer.kt:1114`

`SESS-23` **Current.** `hello.version` must be 4. Otherwise the handshake fails (`Error::Handshake(Incompatible)`) and the text says which end to upgrade: when the far end is newer, upgrade this side and name this side's package; when it is older, upgrade the far end, naming its `implementation` when it gave one. `crates/rutis-bridge/src/session/rpc.rs:1224-1242,2456-2462`; test `crates/rutis-bridge/src/session/rpc/tests.rs::endpoint_format::a_far_end_of_another_protocol_version_is_told_which_end_to_upgrade`; `bridge/version-refusal` in `docs/guarantees.en.md`

`SESS-24` **Current.** `hello.endpoint` must exist and be a valid endpoint id: non-empty, only lowercase letters, digits and `-`. Missing or invalid ids, and an id other than the expected one when the opener expects one, all fail the handshake: missing and invalid are `Incompatible` (Rust's inner reasons: `the far end named no endpoint` / `invalid endpoint id …`), an unexpected id or this end's own id is `IdentityMismatch`. `crates/rutis-bridge/src/session/rpc.rs:2463-2479`; `crates/rutis-bridge/src/channel/mod.rs:77-109`; test `crates/rutis-bridge/src/session/rpc/tests.rs::endpoint_format::a_greeting_from_another_endpoint_fails_the_handshake`

`SESS-25` **Not guaranteed.** No implementation offers a way to declare required capabilities, and the handshake does not refuse a far end that lacks one. Rust's `Greet` had a `require` field whose only production call passed an empty list, with no entry point that could pass a non-empty one; it is removed. Go's `peer.Options.Require` sits in `internal/peer` and has no caller either; Node, Bun, Python, Swift and Kotlin have no such check. When the far end does not declare a capability, only the place that uses it fails (a tagged value is not sent, `SESS-11`). `crates/rutis-bridge/src/session/plugin/greet.rs:12-19`; `go/rutis/internal/peer/peer.go:111-114`

`SESS-26` **Current.** The session plugin waits at most 10 seconds for the greeting; a timeout ends the session with the transport error `the far end did not greet in time`. `crates/rutis-bridge/src/session/plugin/mod.rs:46-47,102-110`; test `crates/rutis-bridge/tests/session_plugin.rs:130-154`

`SESS-27` **Current.** A call this side starts before `ready` fails with `protocol handshake incomplete`, and no frame is written. `crates/rutis-bridge/src/session/rpc.rs:1844-1848`

`SESS-28` **Current.** A completed greeting does not mount the far end: the session plugin returns a `Greeted` value registering no plugin or service; mounting is a later step. `crates/rutis-bridge/src/session/plugin/mod.rs:102-153,178-184`; test `crates/rutis-bridge/tests/session_plugin.rs:228-270`

`SESS-29` **Current.** A failed handshake closes the session (`Close::Done`) and settles waiting calls. The far end learns from the channel end that the session was not established; when this end's greeting was written, the far end at least read this end's version. `crates/rutis-bridge/src/session/rpc.rs:1629-1682`; `crates/rutis-bridge/src/session/plugin/mod.rs:102-110`

`SESS-30` **Superseded.** "A handshake may register plugins or services before mounting" and "a call may succeed before `ready`". The rule-state table of `docs/formal-trace.en.md` §1 already lists these as old evidence.

`SESS-31` **Not guaranteed.** The 10-second limit is a scheduling deadline, not an exact wall-clock promise; load or suspension can make the actual time longer. Retry backoff after a failed handshake, and which failures stop retrying, belong to the connection layer (`bridge/dial-out-rules`) and are not a promise of this file.

## 5. Calls

### 5.1 Call ids

`SESS-32` **Current.** Each side of a session keeps its own call ids: starting at 1, strictly increasing, up to 2^53−1; on the wire they are `<this end's endpoint>:<n>`. A call id is unique only within its session. `crates/rutis-bridge/src/session/rpc.rs:1849-1859,1085-1095`; `node/rutis-runtime/src/session.mjs:346,734`; `bun/rutis-bun/src/session.ts:721`; `python/rutis/rutis/peer.py:1093,1735`; `go/rutis/internal/peer/peer.go:1256,1605`; `swift/rutis/Sources/Rutis/Session/Peer.swift:976,1258`; `kotlin/rutis/rutis/src/shared/kotlin/arcships/rutis/internal/peer/Peer.kt:781,1249`

`SESS-33` **Current.** On an incoming `invoke` / `call` / `get` / `await`, the call id must:

1. start with the far end's greeted endpoint id plus `:`; otherwise the session ends, reading `invalid invocation identity/path`.
2. be an unsigned decimal integer after the prefix (canonical form `[1-9][0-9]*`), greater than the last id admitted in this session, and at most 2^53−1; otherwise the session ends, reading `invalid or repeated invocation identity`.
3. not appear in the frame's own received `path`; if it does, the session ends, reading `invalid invocation identity/path`.

`crates/rutis-bridge/src/session/rpc.rs:1085-1095,2739-2742`; `node/rutis-runtime/src/session.mjs:732-734`; `python/rutis/rutis/peer.py:1732-1735`; `go/rutis/internal/peer/peer.go:1586-1605`; `swift/rutis/Sources/Rutis/Session/Peer.swift:1249-1258`; `kotlin/rutis/rutis/src/shared/kotlin/arcships/rutis/internal/peer/Peer.kt:1231-1249`

Receivers are not equally strict: Node, Bun, Go, Swift and Kotlin require `^[1-9][0-9]*$`; Rust and Python parse an integer and also accept leading zeros and `+`. A sender always writes the canonical form, so this affects malformed far ends only.

`SESS-34` **Current.** A call refused because an argument value cannot cross still consumes its call id; a far end repeating that id is making a repeated call, and the session ends. `crates/rutis-bridge/src/session/rpc.rs:2576-2593`; test `crates/rutis-bridge/src/session/rpc/tests.rs::a_refused_call_takes_its_id`

### 5.2 The call chain `path`

`SESS-35` **Current.** On an incoming call the receiver appends the frame's `id` to its `path` and runs with the whole chain. The chain lets a reverse call reach the side currently waiting synchronously, and lets the host build its wait graph. `crates/rutis-bridge/src/session/rpc.rs:2739-2742,2792-2815`

`SESS-36` **Current.** Forwarding to another session rewrites entries by session tag (`rebase`): an untagged entry gets the source session's tag; an entry tagged for the target session is restored to its native call id there; entries of other sessions pass through. A tag is `s` + a process random number (15 decimal digits) + a counter. `crates/rutis-bridge/src/session/rpc.rs:849-902`; [session tags design](design-session-tags-2026-10-12.en.md) §4; `bridge/call-chains-across-processes` in `docs/guarantees.en.md`

`SESS-37` **Not guaranteed.** Tag uniqueness is probabilistic (two processes' random numbers are equal with probability about 10⁻¹⁵); two processes of 0.9 or earlier still have the old tag problem. That belongs to the connection layer and historical compatibility, not to protocol 5.

### 5.3 Synchronous and asynchronous

`SESS-38` **Current.** `sync: true` means the caller waits on its calling thread. During the wait, reverse calls of the same call chain run on that waiting thread (nested synchronous callbacks); unrelated calls do not interrupt the wait. `crates/rutis-bridge/src/session/rpc.rs:1862-1866,2000-2019,2792-2839`; `go/rutis/internal/peer/peer_test.go::TestSynchronousCallsSayTheyWait`

`SESS-39` **Current.** A synchronous forward enters the wait graph. If the forward would close a cycle of synchronous waits (a call on the chain can only be advanced by the executor it is blocking), it is not sent and the call fails with `SyncWaitCycle`; the session goes on. `crates/rutis-bridge/src/session/rpc.rs:1867-1879`; `crates/rutis-bridge/src/session/rpc/waits.rs`; `bridge/sync-wait-cycle-refused` in `docs/guarantees.en.md`

`SESS-83` **Current.** An asynchronous call says on its frame whether its result will be waited for: `awaited: false` means the caller will not wait for it before the call that made it finishes (fired and forgotten); absent or `true` means it will. Only one kind of call enters the wait graph: `sync: true`, or an asynchronous call whose chain's last call is marked (`SESS-38`) and whose frame does not say `awaited: false`. A call that says `awaited: false` is recorded as an **entry**: nobody waits for it, so it is no edge, but calls made under it still carry its call id as part of their chain. `sync: true` already says the caller waits, so `awaited` is ignored there (senders do not write it). A receiver that forwards the call keeps the original frame's signal: when the host forwards a call for another session, the forwarded call is an edge only if the original caller really waits. All seven implementations (Rust, Node, Bun, Python, Go, Swift, Kotlin) know the field; a runtime that knows nobody waits for a call (`sendAnswered` / `request_answered` / `notify` and the like) writes `awaited: false`. The absent field keeps the old semantics: an old caller says nothing, the host records the conservative edge, and no real wait edge is lost; an old caller may still be refused by a false cycle. `crates/rutis-bridge/src/session/protocol.rs`; `forward` in `crates/rutis-bridge/src/session/rpc/waits.rs`; tests `crates/rutis-bridge/src/session/rpc/waits.rs::tests::an_asynchronous_forward_nobody_awaits_is_an_entry`, `::an_awaited_forward_closes_the_cycle`, `crates/rutis-bridge/src/session/rpc/tests.rs::a_forwarded_call_carries_the_callers_awaited_signal`; `bridge/sync-wait-cycle-refused`

`SESS-40` **Not guaranteed.** Synchronous calls that do not mark `sync` on the frame are outside the check and may deadlock; a call that says `awaited: false` but waits for its result anyway loses its wait edge and may deadlock (the signal is trusted; the host does not verify it); a cycle across hosts is invisible to every host; the cost of reentrancy and mixing versions are not promised. `bridge/sync-wait-cycle-refused` in `docs/guarantees.en.md`

### 5.4 Settling a call

`SESS-41` **Current.** Every call settles exactly once. A reply removes the waiter from the waiting table before settling; a second reply, or a reply to a call this side is not waiting for and did not cancel, is a protocol error and ends the session with `response for unknown call`. A reply to a call in the cancelled table is discarded and counted (§7); an `ack` resolves a cancelled call without settling it. `crates/rutis-bridge/src/session/rpc.rs:2490-2503`

`SESS-42` **Current.** The session layer never retries automatically. A call is written once; a failure is returned to the caller as `Err`. Resending the same call id is not a retry but a repeated call (§5.1). Reconnecting makes a new session; the old session's calls are not replayed on it. `crates/rutis-bridge/src/session/rpc.rs:1982,1085-1095`; `bridge/in-flight-call-fails-once` in `docs/guarantees.en.md`; `docs/formal-trace.en.md` §3.0

`SESS-43` **Current.** A business error affects only its call: the callee settles that call with `throw` and the session and other calls go on. The shape of an error graph is not a protocol error; when a graph is not in its shape, the receiver takes only the name and message and does not end the session. `crates/rutis-bridge/src/session/rpc.rs:2633-2638,3029-3083`; `bridge/bad-values` and `bridge/error-codes` in `docs/guarantees.en.md`

`SESS-44` **Current.** A value this side cannot encode, a tagged value the far end does not take, and a message over the channel's limit all fail before sending, failing only that call; reference grants already made are rolled back and no frame is written. `crates/rutis-bridge/src/session/rpc/crossing.rs:143-246`; `crates/rutis-bridge/src/session/rpc.rs:1892-1946,3063-3078`; `bridge/bad-values` in `docs/guarantees.en.md`

`SESS-45` **Not guaranteed.** The protocol has no call deadline; timeouts are the caller's (`docs/design-protocol-plugin-mount.en.md` §3.1). There is no promise about the start or execution order of concurrent calls; only frame read order and the ordered event queue (§3) are promised.

## 6. References

### 6.1 Grants and releases

`SESS-46` **Current.** Functions, async results, objects and plugin definitions cross by reference: `{id, home, kind, origin}`. The same object appearing repeatedly is the same reference; the same object given to the same session twice is one id with its grant count increased by one. `crates/rutis-bridge/src/session/protocol.rs:42-47`; `crates/rutis-bridge/src/session/rpc.rs:3095-3107`; `bridge/object-refs` and `bridge/function-refs` in `docs/guarantees.en.md`

`SESS-47` **Current.** The sender adds one grant per send. The receiver holds the number of grants it received; once every local proxy is gone it sends `release {reference, count}` with that number. `count` must be greater than 0 and at most the current grants; otherwise the session ends with `invalid reference release count`. `crates/rutis-bridge/src/session/rpc.rs:992-1010,2655-2676`; `bridge/release-count` and `bridge/reference-release-count` in `docs/guarantees.en.md`

`SESS-48` **Current.** A `release` for a reference this session does not have ends the session with `release of unknown reference`. A `call`, `get` or `await` for a reference it does not have ends it with `unknown or released reference`; a method call or property read on a non-object reference ends it with `method call or property read on a non-object reference`. `crates/rutis-bridge/src/session/rpc.rs:2504-2513,2655-2676,2699-2736`; `crates/rutis-bridge/src/session/testing.rs:176-191`

`SESS-49` **Current.** Reference ids run from 1 to 2^53−1; an id out of range, exhausted allocation or an overflowing grant count ends the session (`invalid reference identity`, `reference identifiers exhausted`, `reference grant overflow`). `crates/rutis-bridge/src/session/rpc.rs:2397-2398,3085-3108`

`SESS-50` **Current.** A reference coming home (`home: true`) becomes the original exported object with the same identity; no such export, or a `kind` mismatch, is a protocol error and ends the session. `crates/rutis-bridge/src/session/rpc.rs:2372-2390`

`SESS-51` **Current.** Closing a session clears the export and import tables without waiting for GC; unreleased references go with the session. `crates/rutis-bridge/src/session/rpc.rs:1663-1664,992-1009`; `docs/formal-trace.en.md` §4

### 6.2 Objects

`SESS-52` **Current.** An object with methods crosses by reference: `kind: "object"`, a method call is a `call` with `method`, a property read is `get`. Every protocol-4 implementation takes object references, and senders do not check a capability. `crates/rutis-bridge/src/session/rpc.rs:2690-2736`; `bridge/object-refs` in `docs/guarantees.en.md`

`SESS-53` **Superseded.** "Object references are sent only to a far end that declared `objects`", and the `objects` capability itself (a 0.18 compat-format rule). Protocol 5 has neither the capability nor a refusal branch for it. So the #639 finding that a Rust relay does not check `objects` has no reachable check point in a protocol-5 session; far ends of 0.18 and earlier are already refused at the handshake (`SESS-79`).

`SESS-54` **Current.** Forwarding: a reference imported on session A and given to session B reaches B as a relay of the same `kind`; B's calls, reads and awaits are forwarded to A with the chain rewritten; releasing the relay releases on A. A plugin definition is never relayed to a third session; sending it there fails only that call. `crates/rutis-bridge/src/session/rpc/relay.rs:19-78,93-175`; `bridge/object-refs` and `bridge/plugin-values` in `docs/guarantees.en.md`

`SESS-55` **Not guaranteed.** A reference does not keep a runtime's process alive; after the holder's process exits, calls on its objects fail with `peer-gone` rather than keeping the runtime running. `docs/formal-trace.en.md` §1

## 7. Cancellation

`SESS-56` **Current.** `cancel {id}` means the caller gives up call `id`. The caller sends it when it drops the future of an unfinished async call; a result already returned but not yet consumed by the caller may also be cancelled. From the moment it sends the cancel until the callee resolves it (a late reply or an `ack`), the caller keeps the call in its cancelled table. `crates/rutis-bridge/src/session/protocol.rs:210-230`; `crates/rutis-bridge/src/session/rpc.rs:2273-2300,2952-2996`

`SESS-57` **Current.** (Rust callee) On `cancel`:

- a call still queued and not started is dropped: the method never runs, its argument references go back, and the cancel is acknowledged; the exception is an admitted `emit`-mode `events.dispatch`: it starts as usual ([events protocol](design-events-protocol-2026-10-18.en.md) §5.7), is acknowledged, and its reply is dropped (#1121).
- an `await` in progress is dropped and acknowledged.
- a call already running completes (cancellation does not interrupt a method already running); its result is dropped and the cancel is acknowledged.
- if the reply was written before the `cancel` was read, that reply is the resolution: the cancel is not acknowledged, and the caller discards the reply.
- a late reply to a cancelled call is discarded and counted (`orphans()`), not treated as a protocol error and not closing the session.
- after writing an `ack` the callee sends no `return`/`throw` for that call.

`crates/rutis-bridge/src/session/rpc.rs:2273-2300,2952-2996,3384-3425`; tests `crates/rutis-bridge/src/session/rpc/tests.rs::a_queued_cancelled_call_is_dropped_and_acknowledged`, `::a_running_cancelled_call_is_acknowledged_and_its_reply_dropped`, `::a_cancelled_emit_dispatch_stays_queued`

`SESS-58` **Current.** A `cancel` for a call that does not exist is ignored and not acknowledged; it may have crossed the reply of the call it cancels. This is the only tolerated dangling call id. `crates/rutis-bridge/src/session/rpc.rs:2952-2996`; test `crates/rutis-bridge/src/session/rpc/tests.rs::a_cancel_for_an_unknown_call_is_ignored`

`SESS-59` **Current.** Cancellation propagates hop by hop: when a wait forwarding through a relay is cancelled, the dropped forwarded request sends `cancel` to the next hop; across a link likewise. Cancellation does not undo reference grants already made. `crates/rutis-bridge/src/session/rpc/relay.rs:148-173`; `crates/rutis-bridge/src/session/rpc.rs:2020-2031`; `bridge/multihop-cancel-and-release` in `docs/guarantees.en.md`

`SESS-60` **Current.** Cancellation itself does not close the session; other calls go on. `crates/rutis-bridge/src/session/rpc.rs:2702-2721`; `go/rutis/internal/peer/peer_test.go:135-170`; `go/rutis/internal/peer/objects_test.go:66-103`

`SESS-61` **Current.** A cancel is resolved exactly once by the callee: either it sends `ack {id}` and then no `return`/`throw` for that call, or the reply it wrote before reading the cancel is its resolution and no `ack` follows. A callee acknowledges every cancel it still knows (queued, running, or an await in progress); it does not acknowledge one for a call it already answered, dropped or never had. The caller must take either resolution, discards a reply that crossed the cancel (counting it in `orphans`), clears the cancelled entry on the `ack`, and ignores an `ack` whose entry a crossed reply already cleared. A reply after an `ack` is the protocol error of `SESS-41` and ends the session; a conforming callee never sends one. Every implementation acknowledges and suppresses the reply (Rust, Node, Bun, Python, Go, Swift and Kotlin); `bridge/cancel-reaches-callee` in `docs/guarantees.en.md` states the promise. `crates/rutis-bridge/src/session/rpc.rs:2273-2300,2952-2996,3384-3425`; `specs/call-lifecycle` configurations `call-gap-ack`, `call-mut-ack-then-reply`, `relay-healthy` and `relay-mut-ack-then-reply`

`SESS-62` **Superseded.** "Cancellation is closing the transport" and "a cancelled session is unusable". Cancellation affects only one call.

## 8. Malformed frames and failure scope

`SESS-63` **Current.** Protocol errors fail closed by default: a frame this end cannot read, or a dangling call id or reference, ends the whole session (a transport error) without a partial interpretation. The only tolerances are an unknown `cancel` (§7) and a too-deep frame whose call can still be told (§8.2). `crates/rutis-bridge/src/session/rpc.rs:2514-2561,2598-2618,2739-2741`

### 8.1 Ending the session

| Input | Result | Anchor |
| --- | --- | --- |
| Malformed JSON, an empty message, a leading BOM, invalid UTF-8, not an object | Session ends, transport error | `crates/rutis-bridge/src/session/rpc.rs:3207-3216`; `crates/rutis-bridge/src/session/testing.rs:151-192` |
| Unknown `op`; a field of the wrong type; a missing required field; an unknown wire tag | Session ends, transport error | same; `crates/rutis-bridge/src/session/protocol.rs:154-156` |
| `throw.error` missing `name`/`message` | Session ends, transport error | `crates/rutis-bridge/src/session/protocol.rs:87-94` |
| `throw.error` carrying unknown fields | Only the call waiting for that reply fails (`bad-frame`); the session continues | §9; `reject_unknown_fields` in `crates/rutis-bridge/src/session/rpc.rs` |
| `call` / `get` / `await` for a reference that does not exist | `unknown or released reference` | `crates/rutis-bridge/src/session/rpc.rs:2504-2513,2699-2736` |
| `release` for a reference that does not exist | `release of unknown reference` | `crates/rutis-bridge/src/session/rpc.rs:2655-2676` |
| `release.count` of 0 or above the grant count | `invalid reference release count` | same |
| A method call or property read on a non-object reference | `method call or property read on a non-object reference` | `crates/rutis-bridge/src/session/rpc/relay.rs:79-87` |
| A `return` / `throw` for an unknown call (not a late reply after cancellation) | `response for unknown call` | `crates/rutis-bridge/src/session/rpc.rs:2918-2943` |
| An `ack` of the wrong field type | Session ends, transport error | `MALFORMED_SEQUENCES` "ack of a wrong field type" |
| A second `hello` | `duplicate protocol handshake` | `crates/rutis-bridge/src/session/rpc.rs:2602-2614` |
| Any other frame before the greeting | `request before protocol handshake` | `crates/rutis-bridge/src/session/rpc.rs:2616-2618` |
| A call id that repeats, goes backwards, is not numeric, exceeds 2^53−1, or lacks the far end's prefix | `invalid or repeated invocation identity` / `invalid invocation identity/path` | `crates/rutis-bridge/src/session/rpc.rs:1085-1095,2739-2742` |
| A `path` containing the frame's own `id` | `invalid invocation identity/path` | same |
| A frame nested deeper than 208 JSON levels with no call to blame | `frame nested deeper than 208 levels: …` | `crates/rutis-bridge/src/session/rpc.rs:2514-2561,3126-3134` |
| A message over the channel's limit in either direction | The transport closes the channel; the session ends with a transport error | `crates/rutis-bridge/src/pipe/lines.rs:141-168`; `crates/rutis-bridge/src/websocket/connection.rs:29,87-99` |
| A `chunk` with a bad `seq`, `transfer`, `last` or `data`, from a far end that did not declare `chunked`, over 256 MiB reassembled, or a nested `chunk` | The session ends with a transport error | §2.5; `crates/rutis-bridge/src/session/chunks.rs`; `accept_chunk` in `crates/rutis-bridge/src/session/rpc.rs` |

### 8.2 Failing only the call involved

| Input | Result | Anchor |
| --- | --- | --- |
| Incoming call arguments holding a tagged value this end does not take, a value in the wrong format, or an invalid map key or set element | That call gets a `throw`; the session goes on | `crates/rutis-bridge/src/session/rpc.rs:2298-2324,2567-2596`; `bridge/value-tags` in `docs/guarantees.en.md` |
| A frame nested deeper than 208 levels whose call can be told: `invoke` / `call` / `get` / `await` with an id carrying this end's prefix, or `return` / `throw` with an `id` | The caller gets a `throw` / the waiting call fails with a value error; the session goes on | `crates/rutis-bridge/src/session/rpc.rs:2514-2561`; test `::a_frame_too_deep_fails_its_call_and_the_session_goes_on` |
| Arguments or a result this end cannot encode, a value the far end does not take, or a message over the channel's limit once encoded | Only that call fails; no frame is written and grants are rolled back | `crates/rutis-bridge/src/session/rpc/crossing.rs:143-246`; test `::a_message_over_the_limit_fails_only_its_call` |
| A `return` value this end cannot decode (missing capability or wrong format) | The waiting call fails with a value error; the session goes on | `crates/rutis-bridge/src/session/rpc.rs:2620-2631` |
| A `cancel` for a call that does not exist | Ignored | `crates/rutis-bridge/src/session/rpc/tests.rs:329-361` |
| A late reply to a cancelled call | Discarded and counted; the session is not closed | `crates/rutis-bridge/src/session/rpc.rs:2490-2501` |

`SESS-64` **Current.** The shared malformed-frame matrix has 27 cases: 14 single frames in `MALFORMED` (syntax, fields, a wrong `awaited` type, unknown references, unknown replies) and 13 multi-frame/ordered cases in `MALFORMED_SEQUENCES` (a request before the greeting, a second `hello`, an invocation with no `id`, `get` on an unknown reference, a released reference, a `release` count of 0 or above the grant, wrong field types in `cancel`/`ack`/`throw`, a `throw` for an unknown call, a `path` containing the frame's own id, a repeated call id). A case requires the session to end with a transport error; apart from the replies owed to the case's valid calls, the far end replies to nothing. All seven implementations have an entry point: Rust uses the memory channel, the others a Unix socket, loopback or WebSocket; Swift's cases compile only on macOS and run in CI's `runtimes-swift` job. `crates/rutis-bridge/src/session/testing.rs:151-392`; `crates/rutis-bridge/tests/runtime_conformance.rs:452-633`; `docs/ci.en.md` "Conformance matrix"

`SESS-65` **Superseded.** "A second `hello`, a request before the greeting, `get` on an unknown reference, a released reference, a `release` count of 0 or above the grants, wrong field types in `cancel`/`ack`/`throw`, a `throw` for an unknown call, a `path` containing the frame's id and a call id that does not increase are enforced by code but are not in the shared matrix run by the seven implementations." That is no longer the state: every rule in that list is now in the 27-case matrix (`SESS-64`, §14 G2), with the call-id item covering a repeated id. Still verified by code alone: a method call or property read on a non-object reference; a call id that goes backwards, is not numeric, exceeds 2^53−1 or lacks the far end's prefix; `throw.error` missing `name`/`message` (the matrix has an `error` that is not an object only); a frame deeper than 208 levels; a message over the channel's limit.

## 9. Unknown fields

`SESS-66` **Current (Rust).** A field the receiver does not know does not end the session; it is handled at the frame's own scope:

- `hello`: the handshake fails and the session ends, with the text `incompatible session: unknown field "x" in hello`; no `code`/`data`.
- `invoke`, `call`, `get`, `await`: only that call fails. The receiver sends a `throw` with code `bad-frame`, message `unknown field "x" in invoke`, and data `{"op":"invoke","fields":["x"]}`; other calls and the session continue. `x` is a field path; a field inside a value uses a path such as `args.value.x`.
- `return`, `throw`: only the call waiting for that reply fails, with code `bad-frame` and the same data; the session continues.
- `release`, `cancel`: there is no call to fail; the extra fields are ignored and the known fields are processed.
- A call before the handshake, an `id` without the far end's prefix, an unknown `op`, a missing `id`, a dangling reply or reference still end the session per §8; a frame whose call cannot be identified is not downgraded to a call-level error.

Implementation: `parse_frame` / `reject_unknown_fields` in `crates/rutis-bridge/src/session/rpc.rs`; tests `unknown_frame_fields_fail_only_their_call` and `an_unknown_field_in_hello_fails_the_handshake`. `awaited` is a known field of a call frame (`SESS-83`), not an unknown field; a value of another type ends the session as a wrong field type per this section.

`SESS-67` **Current (all seven).** Rust, Node, Bun, Python, Go, Swift and Kotlin all implement the call-level refusal of `SESS-66`; the verification column of `bridge/unknown-fields` lists each implementation's cases.

`SESS-68` **Superseded.** Two old behaviors: Rust treated an unknown field as a malformed whole frame and ended the session; the other six ignored it uniformly. Both are replaced by `SESS-66`.

`SESS-69` **Not guaranteed.** Treating unknown fields as an extension mechanism; a new optional field that does not change meaning reaching an old receiver without negotiation; an old implementation fails that call per `SESS-66` instead of silently ignoring it. Capability declarations are no substitute for field extension: negotiation can only use the known `capabilities` array and cannot make an old implementation accept a field it does not know. `throw.error.graph` is a known field today, not an unknown-field extension.

## 10. Message size, nesting depth and outbound backlog

`SESS-70` **Current.** The message size limit is the transport's rule, not the frame's. Process pipes and WebSockets default to 16 MiB (16777216 bytes); a WebSocket's `Limits.max_message` is configurable. The memory channel has no limit. When both sides declared `chunked`, a frame over the limit is carried by the `chunk` frames of §2.5 and one frame may reach the session's `max_transfer` (256 MiB by default, `SESS-88`). `crates/rutis-bridge/src/pipe/lines.rs:13-28`; `crates/rutis-bridge/src/websocket/config.rs:104-108,132-141`; `crates/rutis-bridge/src/channel/mod.rs:32-42`

`SESS-71` **Current.** At open the session records `Sender::max_message()` and compares every frame it is about to write against it. A frame over the limit is not written: the call fails with `bad-value`, `data` names the part (`part`, with `argument` for an argument) and the `limit`, and the text names the call, the argument or result, and the byte count; the session and other calls go on. When both sides declared `chunked`, the limit compared is the session's `max_transfer` (256 MiB by default), and only a frame over it fails (`SESS-88`). `send_limit` and `serialized` in `crates/rutis-bridge/src/session/rpc.rs`; `crates/rutis-bridge/src/session/rpc/crossing.rs:104-113,221-246`; tests `crates/rutis-bridge/src/session/rpc/tests.rs::a_message_over_the_limit_fails_only_its_call`, `::the_limit_and_the_call_are_named`; `bridge/bad-values` and `bridge/large-and-deep-values` in `docs/guarantees.en.md`

`SESS-72` **Current.** When the transport refuses a message over its own limit, the channel ends, the session ends with a transport error, every in-flight call fails with that error, and the message is not executed. The process pipe refuses oversized messages on both sending and receiving and closes the channel; a WebSocket closes with code 1009. `crates/rutis-bridge/src/pipe/lines.rs:141-168`; `crates/rutis-bridge/src/websocket/connection.rs:29,87-99`; test `crates/rutis-bridge/tests/websocket.rs:662-690`

`SESS-73` **Not guaranteed.** "A message over the limit fails only its call." A frame this side can check before writing fails only that call; when the far end's limit is smaller, when the far end does not follow this protocol, or when the transport refuses the message, the channel and session end and in-flight calls fail with them. `bridge/bad-values` in `docs/guarantees.en.md` lists this as not guaranteed.

`SESS-74` **Current.** The sender allows a value to nest 64 levels (each array and object is one level; an argument list is not a level); a deeper value is refused before sending, failing only that call, with `the value nests deeper than 64 levels`. A frame on the wire may nest up to 208 levels, leaving room for encoding; deeper frames follow §8. `crates/rutis-bridge/src/session/rpc.rs:3119-3148`; test `::a_value_deeper_than_the_limit_fails_its_call_before_it_is_sent`; `bridge/large-and-deep-values` in `docs/guarantees.en.md`

`SESS-75` **Current.** The outbound backlog is a different limit: `SessionOptions::outbound_limit` bounds the bytes queued behind the one frame the writer thread already holds; an empty queue takes one frame whatever the limit, `0` included. Over it, the session ends with `the far end is not reading: <n> messages, <bytes> bytes waited for it` (`peer-gone`), and what was queued goes with it. `crates/rutis-bridge/src/session/rpc.rs:1186-1214,1387-1404`; tests `crates/rutis-bridge/src/session/rpc/tests.rs:618-741`

`SESS-76` **Not guaranteed.** `outbound_limit` defaults to `None`: no bound, so a far end that does not read lets this process keep arbitrarily many queued bytes for it until it reads again or the session ends. A host may leave this limit off; with it on, `SESS-75` applies. `crates/rutis-bridge/src/session/rpc.rs:1186-1214`; `docs/formal-trace.en.md` §3.6

`SESS-82` **Not guaranteed.** Values received nest within 64 levels. The limit constrains sending; a value received from a far end that does not keep to it may nest deeper than 64 levels (the frame limit of 208 levels still applies).

`SESS-88` **Current.** One transfer reassembles to at most the session's `max_transfer`: 256 MiB by default, set per session or for a deployment with `RUTIS_MAX_TRANSFER` (a host passes it to the runtime processes it starts); never unlimited, and never truncated silently. The per-session entry point differs by implementation: Rust's `SessionOptions::max_transfer`, Node's and Bun's `maxTransfer` session option, Go's `Options.MaxTransfer`, Swift's `PeerOptions.maxTransfer` and Kotlin's `Peer.maxTransfer`; Python reads only `RUTIS_MAX_TRANSFER`. A sender over it fails only that call as in `SESS-71`, with `limit` set to the configured value; a receiver over it ends the session with a transport error naming that value. At most one transfer is unfinished at a time: a second `transfer` is a protocol error, the bound on concurrent unfinished transfers. **Not guaranteed:** that the two ends are configured with the same cap; the sender chunks up to its own, the receiver reassembles up to its own, and the side over its cap ends the session with a transport error. Chunking changes nothing else in `SESS-70`–`SESS-73`: the transport still refuses a single message over its own limit, so a `chunk` frame itself must fit; the memory channel has no transport limit and so is never chunked, but the session's own `max_transfer` still bounds the whole frame — no session is unlimited. One transfer counts as one message in the outbound backlog, its bytes as the original frame (`SESS-75` unchanged). **Not guaranteed:** between two ends that did not declare `chunked` (earlier builds), a value over the transport's limit still cannot cross (the behaviour of 0.22); reassembly needs the whole frame in the far end's memory, with a peak of about 1.4 times its size; a transfer whose `last` never comes produces no call. `MAX_TRANSFER` in `crates/rutis-bridge/src/session/chunks.rs`; `crates/rutis-bridge/tests/value_roundtrip.rs::node_carries_frames_over_the_message_limit_whole`; `node/rutis-runtime/test/chunked.test.mjs`, `bun/rutis-bun/test/chunked.test.ts`; `max_transfer_from_environment` in `python/rutis/rutis/_chunks.py` and `python/rutis/tests/test_chunked.py`; `maxTransferFromEnvironment` in `go/rutis/internal/peer/chunks.go` and `go/rutis/internal/peer/chunked_test.go`; `maxTransferFromEnvironment` in `swift/rutis/Sources/Rutis/Session/Peer.swift` and `swift/rutis/Tests/SelfTest/ChunkedTests.swift`; `maxTransferFrom` in `kotlin/rutis/rutis/src/shared/kotlin/arcships/rutis/internal/peer/Peer.kt` and `kotlin/rutis/rutis/src/jvmTest/kotlin/arcships/rutis/ChunkedTest.kt`; `bridge/large-and-deep-values` in `docs/guarantees.en.md`

## 11. Rule states

The table is this file's rule list, for #634's trace table and #613's models. Every **Current** row has a code or test anchor; **Not guaranteed** rows are collected here so they cannot be misread as promises.

| Id | Rule | State | Evidence |
| --- | --- | --- | --- |
| `SESS-1` | A session is one ordered, reliable, duplex, boundary-preserving message stream | Current | `crates/rutis-bridge/src/channel/mod.rs:1-9,20-48` |
| `SESS-2` | One UTF-8 JSON frame per message, one at a time in arrival order | Current | `crates/rutis-bridge/src/session/rpc.rs:1557-1570,3207-3216` |
| `SESS-3` | Protocol 5, one constant in seven implementations, WebSocket subprotocol `rutis.5` | Current | `crates/rutis-bridge/src/session/mod.rs:9-15` and each implementation's constant |
| `SESS-4` | Two endpoint ids; call ids carry this end's prefix | Current | `crates/rutis-bridge/src/channel/mod.rs:77-109`; `crates/rutis-bridge/src/session/rpc.rs:1493-1499` |
| `SESS-5` | A reconnect is a new session; old state does not carry over | Current | `docs/formal-trace.en.md` §3.0; `crates/rutis-bridge/tests/session_plugin.rs:182-270` |
| `SESS-6` | The ten `Frame` variants, plus the `chunk` frame read before decoding (§2.5) | Current | `crates/rutis-bridge/src/session/protocol.rs:149-230`; `crates/rutis-bridge/src/session/chunks.rs` |
| `SESS-7` | `sync` and optional fields omitted when absent | Current | `crates/rutis-bridge/src/session/protocol.rs:157-221` |
| `SESS-8` | `path` is the call chain; its entries are call ids | Current | `crates/rutis-bridge/src/session/rpc.rs:2739-2742` |
| `SESS-9` | The `WireValue` tag set | Current | `crates/rutis-bridge/src/session/protocol.rs:26-85` |
| `SESS-10` | `undefined` kept as a whole, `null` inside a value | Current | `crates/rutis-bridge/src/session/rpc.rs:2356-2359,2567-2573` |
| `SESS-11` | A tagged value goes only to an end declaring `value-<tag>` | Current | `crates/rutis-bridge/src/session/rpc.rs:2204-2214` |
| `SESS-12` | One format only: endpoint, protocol 5 | Current | `docs/migration-0.23.en.md` "Upgrading together"; `crates/rutis-bridge/src/session/rpc.rs:2456-2462` |
| `SESS-13` | Two formats (compat 2, endpoint 3) | Superseded | `docs/migration-0.19.en.md` "Upgrading together" |
| `SESS-14` | `sync` sent only to ends declaring `sync-wait` | Superseded | Protocol 4 has no such capability; `crates/rutis-bridge/src/session/protocol.rs:174-176` |
| `SESS-15` | Capability declarations and what reads them (including `chunked`) | Current | `crates/rutis-bridge/src/session/rpc.rs:1154-1178,1590-1611`; `crates/rutis-bridge/src/session/plugin/greet.rs:31-36` |
| `SESS-16` | The `sync-wait`, `events-v2`, `objects` capabilities | Superseded | No such constants; §6.2 |
| `SESS-17` | Unknown capability names ignored; declarations unverified; capabilities are not permissions | Not guaranteed | §2.4 |
| `SESS-18` | Frames in arrival order, one at a time; calls may run concurrently | Current | `crates/rutis-bridge/src/session/rpc.rs:1557-1570,2792-2839`; `go/rutis/internal/peer/peer_test.go:55-58` |
| `SESS-19` | Event operations enter the ordered queue in frame order | Current | `crates/rutis-bridge/src/session/rpc.rs:1244-1283,2767-2884` |
| `SESS-20` | Both ends greet before reading | Current | `crates/rutis-bridge/src/session/rpc.rs:1524-1541` |
| `SESS-21` | A request before the greeting ends the session | Current | `crates/rutis-bridge/src/session/rpc.rs:2616-2618`; Go `TestACallBeforeTheHandshakeEndsTheSession` |
| `SESS-22` | A second `hello` ends the session | Current | `crates/rutis-bridge/src/session/rpc.rs:2602-2614` |
| `SESS-23` | A version mismatch says which end to upgrade | Current | `crates/rutis-bridge/src/session/rpc.rs:1224-1242,2456-2462`; `bridge/version-refusal` |
| `SESS-24` | Endpoint id required, well-formed and checked | Current | `crates/rutis-bridge/src/session/rpc.rs:2463-2479`; `crates/rutis-bridge/src/channel/mod.rs:77-109` |
| `SESS-25` | A missing required capability refuses the handshake | Not guaranteed | No implementation offers an entry point for required capabilities: `crates/rutis-bridge/src/session/plugin/greet.rs:12-19`; `go/rutis/internal/peer/peer.go:111-114` |
| `SESS-26` | 10-second greeting deadline | Current | `crates/rutis-bridge/src/session/plugin/mod.rs:46-47`; `crates/rutis-bridge/tests/session_plugin.rs:130-154` |
| `SESS-27` | Sending before `ready` is refused | Current | `crates/rutis-bridge/src/session/rpc.rs:1844-1848` |
| `SESS-28` | A greeting does not mount | Current | `crates/rutis-bridge/src/session/plugin/mod.rs:102-153,178-184` |
| `SESS-29` | A failed handshake closes the session and settles waits | Current | `crates/rutis-bridge/src/session/rpc.rs:1629-1682` |
| `SESS-30` | Registering/mounting before greeting; a successful call before `ready` | Superseded | `docs/formal-trace.en.md` §1 |
| `SESS-31` | Exact wall-clock deadline; retry policy | Not guaranteed | §4 |
| `SESS-32` | Call ids start at 1, strictly increase, at most 2^53−1 | Current | `crates/rutis-bridge/src/session/rpc.rs:1849-1859,1085-1095` |
| `SESS-33` | Call-id prefix, increase and `path` self-reference checks | Current | `crates/rutis-bridge/src/session/rpc.rs:1085-1095,2739-2742` |
| `SESS-34` | A refused call still consumes its call id | Current | `crates/rutis-bridge/src/session/rpc.rs:2576-2593` |
| `SESS-35` | An incoming call appends its `id` to `path` | Current | `crates/rutis-bridge/src/session/rpc.rs:2739-2742` |
| `SESS-36` | Forwarding rewrites entries by session tag (`rebase`) | Current | `crates/rutis-bridge/src/session/rpc.rs:849-902`; `bridge/call-chains-across-processes` |
| `SESS-37` | Tag uniqueness | Not guaranteed | [session tags design](design-session-tags-2026-10-12.en.md) §4 |
| `SESS-38` | `sync` waits, and reverse calls of the chain run on the waiter | Current | `crates/rutis-bridge/src/session/rpc.rs:1862-1866,2000-2019,2792-2839` |
| `SESS-39` | A wait cycle is refused with `SyncWaitCycle` | Current | `crates/rutis-bridge/src/session/rpc.rs:1867-1879`; `bridge/sync-wait-cycle-refused` |
| `SESS-40` | Calls not marked `sync`; cycles across hosts | Not guaranteed | `bridge/sync-wait-cycle-refused` in `docs/guarantees.en.md` |
| `SESS-41` | Every call settles exactly once; an unknown reply ends the session | Current | `crates/rutis-bridge/src/session/rpc.rs:2490-2503` |
| `SESS-42` | The session layer never retries automatically | Current | `crates/rutis-bridge/src/session/rpc.rs:1982,1085-1095`; `bridge/in-flight-call-fails-once` |
| `SESS-43` | A business error affects only its call; an error graph's shape does not end the session | Current | `crates/rutis-bridge/src/session/rpc.rs:2633-2638,3029-3083`; `bridge/bad-values` |
| `SESS-44` | Encoding/value/limit failures fail only the call and roll back grants | Current | `crates/rutis-bridge/src/session/rpc/crossing.rs:143-246` |
| `SESS-45` | Call deadlines and concurrent-call order | Not guaranteed | `docs/design-protocol-plugin-mount.en.md` §3.1 |
| `SESS-46` | Reference wire form, identity, one reference per object | Current | `crates/rutis-bridge/src/session/protocol.rs:42-47`; `crates/rutis-bridge/src/session/rpc.rs:3095-3107` |
| `SESS-47` | Grant counts and counted release | Current | `crates/rutis-bridge/src/session/rpc.rs:992-1010,2655-2676`; `bridge/release-count` |
| `SESS-48` | Unknown references, non-object references and invalid `release` end the session | Current | `crates/rutis-bridge/src/session/rpc.rs:2504-2513,2655-2676,2699-2736` |
| `SESS-49` | Reference id range, allocation and grant overflow | Current | `crates/rutis-bridge/src/session/rpc.rs:2397-2398,3085-3108` |
| `SESS-50` | A home reference keeps its identity; an unknown export ends the session | Current | `crates/rutis-bridge/src/session/rpc.rs:2372-2390` |
| `SESS-51` | Closing a session clears the reference tables | Current | `crates/rutis-bridge/src/session/rpc.rs:1663-1664` |
| `SESS-52` | Objects cross by reference; every protocol-4 implementation takes them | Current | `crates/rutis-bridge/src/session/rpc.rs:2690-2736`; `bridge/object-refs` |
| `SESS-53` | Objects sent only to ends declaring `objects` | Superseded | Protocol 4 has no such capability; §6.2 |
| `SESS-54` | Relay forwarding and hop-by-hop release | Current | `crates/rutis-bridge/src/session/rpc/relay.rs:19-78,93-175` |
| `SESS-55` | A reference does not keep a runtime alive | Not guaranteed | `docs/formal-trace.en.md` §1 |
| `SESS-56` | Giving up an async wait sends `cancel` | Current | `crates/rutis-bridge/src/session/rpc.rs:2020-2049` |
| `SESS-57` | The Rust callee's cancellation handling and late replies | Current | `crates/rutis-bridge/src/session/rpc.rs:2639-2653,2490-2501` |
| `SESS-58` | An unknown `cancel` is ignored | Current | `crates/rutis-bridge/src/session/rpc.rs:2639-2653` |
| `SESS-59` | Cancellation propagates hop by hop and does not undo grants | Current | `crates/rutis-bridge/src/session/rpc/relay.rs:148-173`; `bridge/multihop-cancel-and-release` |
| `SESS-60` | Cancellation does not close the session | Current | `crates/rutis-bridge/src/session/rpc.rs:2639-2653` |
| `SESS-61` | A cancel is resolved once: `ack` and no reply, or the reply written before the cancel was read; the caller takes either | Current | §7; #1099; `specs/call-lifecycle` |
| `SESS-62` | Cancellation is closing the transport | Superseded | §7 |
| `SESS-63` | Malformed input fails closed | Current | `crates/rutis-bridge/src/session/rpc.rs:2514-2561,2598-2618`; `crates/rutis-bridge/src/session/testing.rs:151-392` |
| `SESS-64` | The shared malformed matrix: 27 cases (14 single-frame + 13 multi-frame/ordered), all seven implementations have an entry point | Current | `crates/rutis-bridge/src/session/testing.rs:154-370`; `crates/rutis-bridge/tests/runtime_conformance.rs:452-633` |
| `SESS-65` | The old "these rules are outside the shared matrix" state; the 27-case matrix covers that list (`SESS-64`) | Superseded | §8.1; §14 G2 |
| `SESS-66` | Rust: an unknown field fails only that call (`hello` fails the handshake, `release`/`cancel` ignore it) | Current | `parse_frame` / `reject_unknown_fields` in `crates/rutis-bridge/src/session/rpc.rs`; `rpc/tests.rs::unknown_frame_fields_fail_only_their_call`, `::an_unknown_field_in_hello_fails_the_handshake` |
| `SESS-67` | All seven runtimes implement the call-level refusal (`bad-frame`); `release`/`cancel` ignore extra fields | Current | `bridge/unknown-fields` in `docs/guarantees.en.md`; §9; §14 G1 |
| `SESS-68` | Unknown fields end the session / six runtimes ignore them uniformly | Superseded | §9; §14 G1 |
| `SESS-69` | Unknown fields as an extension mechanism | Not guaranteed | §9 |
| `SESS-70` | 16 MiB default; the memory channel has no limit; the session's `max_transfer` (256 MiB by default) a frame when both declared `chunked` | Current | `crates/rutis-bridge/src/pipe/lines.rs:13-28`; `crates/rutis-bridge/src/websocket/config.rs:104-108,132-141` |
| `SESS-71` | Checked before sending; over the limit fails only the call (`max_transfer` when `chunked`) | Current | `crates/rutis-bridge/src/session/rpc.rs`; `crates/rutis-bridge/src/session/rpc/crossing.rs:221-246` |
| `SESS-72` | A transport over-limit ends the channel and session | Current | `crates/rutis-bridge/src/pipe/lines.rs:141-168`; `crates/rutis-bridge/src/websocket/connection.rs:29,87-99` |
| `SESS-73` | Over the limit failing only this call | Not guaranteed | §10 |
| `SESS-74` | 64 levels for a value, 208 for a frame | Current | `crates/rutis-bridge/src/session/rpc.rs:3119-3148`; `bridge/large-and-deep-values` |
| `SESS-75` | Outbound backlog limit | Current | `crates/rutis-bridge/src/session/rpc.rs:1186-1214` |
| `SESS-76` | The default outbound backlog is unbounded | Not guaranteed | `crates/rutis-bridge/src/session/rpc.rs:1186-1214`; `docs/formal-trace.en.md` §3.6 |
| `SESS-77` | The same rule's error text word for word across implementations | Not guaranteed | §12; Node and Go read differently from Rust |
| `SESS-78` | `hello.implementation` may be absent; receivers do not require it | Current | `crates/rutis-bridge/src/session/protocol.rs:157-167`; `crates/rutis-bridge/src/session/rpc.rs:2450-2489` |
| `SESS-79` | Protocol 5 speaks only to protocol 5; 0.22 and earlier are refused | Current | `docs/migration-0.23.en.md` "Upgrading together"; `bridge/cross-version-meetings` |
| `SESS-80` | A new capability needs no version bump; a new frame joins the current protocol version before its release, or bumps it | Current | `crates/rutis-bridge/src/session/protocol.rs:154-156,210-226`; §13; `SESS-66` |
| `SESS-81` | Cross-version meetings beyond adjacent releases | Not guaranteed | `bridge/cross-version-meetings` promises adjacent releases only |
| `SESS-82` | Values received nest within 64 levels | Not guaranteed | §10; `bridge/large-and-deep-values` |
| `SESS-83` | `awaited` tells fired-and-forgotten from awaited; absent reads as awaited | Current | §5.3; `crates/rutis-bridge/src/session/protocol.rs`; `crates/rutis-bridge/src/session/rpc/waits.rs`; `bridge/sync-wait-cycle-refused` |
| `SESS-84` | The `chunk` frame's fields and where it is read (before decoding, not a `Frame` variant) | Current | §2.5; `Chunk` in `crates/rutis-bridge/src/session/chunks.rs` |
| `SESS-85` | The `chunked` capability; chunking only when both declared; all seven implementations declare it | Current | §2.5; `send_limit` in `crates/rutis-bridge/src/session/rpc.rs`; `#chunking` in `node/rutis-runtime/src/session.mjs`; `_chunking` in `python/rutis/rutis/peer.py`; `chunkingLocked` in `go/rutis/internal/peer/chunks.go`; `chunks` in `swift/rutis/Sources/Rutis/Session/Peer.swift`; `chunking` in `kotlin/rutis/rutis/src/shared/kotlin/arcships/rutis/internal/peer/Peer.kt` |
| `SESS-86` | Sending: at most 8 MiB a slice that fits the transport; consecutive slices; over `max_transfer` fails only the call | Current | §2.5; `slice_size` in `crates/rutis-bridge/src/session/chunks.rs`; test `a_frame_over_the_limit_is_chunked_to_a_far_end_that_declared_it` |
| `SESS-87` | Receiving: buffer by `transfer` and `seq`, at most one unfinished transfer, dispatch only after `last`; an unfinished transfer goes with the session; violations end it | Current | §2.5; `Reassembly` in `crates/rutis-bridge/src/session/chunks.rs`; `#receiveChunk` in `node/rutis-runtime/src/session.mjs` |
| `SESS-88` | The configurable `max_transfer` cap (256 MiB by default); a transfer counts as one message in the backlog; ends without `chunked` behave as before | Current | §10; `crates/rutis-bridge/tests/value_roundtrip.rs::node_carries_frames_over_the_message_limit_whole` |

## 12. What developers see

### Plugin authors

**When it works:** the plugin API, value mapping, objects and cancellation are unchanged. Ordinary calls, object methods, function references and async results work as before; there are no new configuration fields, no new output lines, and no need to learn "frames" or "call ids".

**When it fails (word for word, Rust side):**

| Situation | Output |
| --- | --- |
| The far end is older | `incompatible session: the far end speaks protocol 4, this side 5: upgrade the far end` |
| The far end is newer | `incompatible session: the far end speaks protocol 99, this side 5: upgrade this side (Rust crates rutis-host / rutis-bridge)` |
| The far end did not greet within 10 seconds | `the far end did not greet in time` |
| The host has no session id | `the host provides no id for its sessions` |
| A value is over the message limit | When the far end did not declare `chunked` (an earlier build): `svc.take: argument 2 cannot be sent: the message would be over the limit of 16777216 bytes (16 MiB)`; code `bad-value`; `data` is `{"argument":2,"limit":16777216,"part":"argument"}`. When it declared `chunked` (all seven implementations of 0.23), the same value crosses, up to 256 MiB |
| The value encodes to over the session's cap (256 MiB by default, a host may raise it) | `svc.take: argument 2 cannot be sent: the message would be 300000000 bytes, over the limit of 268435456 bytes (256 MiB)`; code `bad-value`; `data` is `{"argument":2,"limit":268435456,"part":"argument"}` |
| The runtime owning an object exits | `a call on an object from py failed: runtime py exited`; code `peer-gone` |

Every call-level failure ends at the call or the value. Handshake and transport failures are fixed by upgrading both ends together, checking the deployment's endpoint ids, removing the oversized value or raising the limit. **Do not branch on error text**: branch on `code`, or on the `Error::Handshake` / `Error::Transport` category (`SESS-77`).

**Concepts to learn:** none added. Frames, call ids and grants/releases are vocabulary for runtime authors and contributors, not for the plugin API. A parameter or result over 16 MiB needs no paging either: when the far end declared `chunked` (all seven implementations of 0.23) it crosses, up to 256 MiB.

### Host authors and deployers

**When it works:** the existing `rutis-host` commands, status lines, exit codes, close codes and retry policy are unchanged. The WebSocket subprotocol is `rutis.5`; transport close codes are still `1000` (normal), `4002` (replaced), `4003` (rejected) and `1009` (message over the limit, WebSocket).

**When it fails (word for word or by category):**

- The session is refused at the handshake: the error is `incompatible session: …`, saying which end to upgrade; a dialer classifies it as `incompatible` and stops retrying until the configuration changes.
- Identity mismatch: `endpoint mismatch: …`; a dialer classifies it as `auth-rejected` and retries slowly while reporting.
- A retryable transport failure: `retryable`, reconnecting with backoff; the new connection is a new session.
- The transport ends midway through a transfer: the session ends with a transport error, and the unfinished request is never dispatched or answered; in-flight calls fail with `peer-gone`. Between two ends that declared `chunked`, parameters and results over 16 MiB are usable directly and one call runs once; a far end without it (an earlier build) behaves as in 0.22.
- A protocol error: the session ends with a transport error, in-flight calls fail with `peer-gone`, and late frames of the old session never reach the replacement session.

**Concepts to learn:** none added; to change the cap, one knob, `max_transfer` / `RUTIS_MAX_TRANSFER`. Deployers need to know: endpoint ids are unique in a deployment, and every protocol-5 end must be upgraded together (§13).

### Runtime authors and contributors

**When it works:** run the seven-implementation conformance suite: `RUTIS_TEST_RUNTIMES=node,bun,python,go,swift,kotlin cargo test -p rutis-bridge --all-features --test runtime_conformance`. It runs every check of `session::testing::session` and `malformed`, and compares with the cells of `docs/ci.en.md` "Conformance matrix".

**When it fails:** each malformed case prints which message it was; the rule categories are in §8. Error text may differ from other implementations (`SESS-77`), but the session must still end with a transport error, or only the call involved must fail, with the scope of §8.

**Concepts to learn:** frames, `op`, call ids, `path`, reference grants/releases, call-level failure, session-level failure, capability declarations, the `chunk` frame and the `chunked` capability. They are for implementation and verification only and do not enter the user API.

## 13. Compatibility and migration

`SESS-79` **Current.** Protocol 5 speaks only to protocol 5. Hosts, runtimes, nodes and SDKs of 0.22 and earlier are refused at the handshake; migrating means upgrading every end to 0.23 or later. `docs/migration-0.23.en.md` "Upgrading together"; `bridge/cross-version-meetings` in `docs/guarantees.en.md`

`SESS-80` **Current.** Adding a capability does not need a protocol version: a sender checks `value-<tag>` before sending a tagged value, and does not send it when the far end does not declare it. Because old implementations fail a call with an unknown field (`SESS-66`) and reject unknown `op`s, a new **frame** or a **change in an existing field's meaning** can only join the current protocol version before its release, or ship with a version bump, updating this file, `Frame`/`WireValue`, the malformed matrix and the implementations together. The `chunk` frame (`SESS-84`) and `awaited` (`SESS-83`) joined protocol 5 before its release, so neither bumps it; an implementation must send and read it before declaring `chunked`. A new capability needs no version bump and is used only as declared (§2.4); there is no required capability to negotiate (`SESS-25`).

`SESS-81` **Not guaranteed.** Cross-version meetings beyond adjacent releases; mixing new and old implementations; a new field without a version. Protocol 5 gained `awaited` (`SESS-83`) and the `chunk` frame (`SESS-84`) before release: a far end built from an earlier protocol-5 prerelease fails a call carrying `awaited` per `SESS-66`; an earlier build does not declare `chunked`, so it is never chunked to and never receives a `chunk` frame it does not know. All ends must be upgraded to the 0.23 build that has them. `bridge/cross-version-meetings` in `docs/guarantees.en.md` promises only two adjacent releases with no incompatible change between them.

## 14. Specification gaps and decisions

### G1 Unknown fields: decided (2026-11-09)

- Decision: keep rejecting (B), but handle the field at the frame's scope instead of ending the whole session: `hello` fails the handshake; `invoke`/`call`/`get`/`await` fail only that call; `return`/`throw` fail only the call waiting for it; `release`/`cancel` ignore it. Code `bad-frame`, data `{op, fields}`.
- Done (2026-11-10): all seven implementations have the call-level refusal (`SESS-67`); the verification column of `bridge/unknown-fields` lists each implementation's cases, and Rust's tests are `unknown_frame_fields_fail_only_their_call`, `an_unknown_field_in_hello_fails_the_handshake` and `release_and_cancel_ignore_unknown_fields`. Plan G11 is closed.
- Unknown fields fail one call only, so they are not part of the session-ending `MALFORMED` matrix; each implementation's call-level tests are the evidence.
- The old A (ignore everything) and C (whitelist) directions are dropped.

### G2 The shared malformed matrix: filled (2026-11-11)

- Decision (#635): move malformed rules that only code enforced into the shared `session::testing::malformed` matrix, so the seven implementations run the same cases.
- Done: the matrix grew from 13 cases to 26, 13 single frames in `MALFORMED` and 13 multi-frame/ordered cases in `MALFORMED_SEQUENCES` (`SESS-83` added a single frame with a wrong `awaited` type, 27 now); the original list (a request before the handshake, a duplicate `hello`, `get` on an unknown reference, a released reference, a `release` count of 0 or above the grants, wrong field types in `cancel`/`ack`/`throw`, a `throw` for an unknown call, a `path` containing the frame's id, a call id that does not increase) is covered, with the call-id item covering a repeated id only, plus an invocation with no `id` (`SESS-21`, `SESS-22`, `SESS-33`, `SESS-47`, `SESS-48`; `SESS-64`). Implementation: `MALFORMED`, `MALFORMED_SEQUENCES` and `malformed` in `crates/rutis-bridge/src/session/testing.rs`; Swift's cases compile only on macOS and run in CI's `runtimes-swift` job.
- Still outside the matrix: a method call or property read on a non-object reference; a call id that goes backwards, is not numeric, exceeds 2^53−1 or lacks the far end's prefix; `throw.error` missing `name`/`message`; a frame deeper than 208 levels; a message over the channel's limit; a `chunk`'s malformed order and its cap (all seven implementations chunk, G8). Code and other tests verify these (§8.1, §8.2, §2.5).

### G3 The reply-after-cancel rule: decided (2026-11-16)

- Superseded (2026-11-18, #1099): "neither always reply nor never reply is a cross-implementation requirement" was the protocol-4 state. Protocol 5 makes the callee resolve the cancel once: `ack` and no reply, or the reply written before the cancel was read. The caller clears its cancelled table on either; an `ack` whose entry is gone is ignored; a reply after an `ack` is an unknown call and ends the session (`SESS-61`).
- Why: the caller's table could only be cleared by a reply, and no conforming callee replies to a call cancelled while queued, so a long session that cancelled queued calls grew it without bound (#1099; `specs/call-lifecycle`, `call-gap-cancel-table`). The bounded table was the fallback; the acknowledgement empties the table at rest without relaxing `SESS-41`.
- Both reply shapes stay legal when they happen before the cancel is read; the #626 model runs them (`call-cancel-reply`, `call-cancel-no-reply`) and the mutation `call-mut-ack-then-reply` shows why the acknowledgement and the silence are one rule.

### G4 Capabilities and formats that no longer exist in older documents

- The "objects and the `objects` capability" paragraph of [protocol plugin design](design-protocol-plugins-2026-09-25.en.md) §6.1 still describes the 0.18 compat handshake; the current implementation has no `objects` capability (`SESS-16`, `SESS-53`).
- Issue #635 and the [network stack design](design-protocol-channel-decoupling-2026-10-03.en.md) still mention `sync-wait`, `events-v2` and two formats; protocol 5 has none of them (`SESS-13`, `SESS-16`).

Both need a later documentation revision; this file only marks them superseded and does not edit the old design texts.

### G5 Rules without tests: closed (2026-11-11)

- `SESS-67`: all seven implementations have call-level refusal tests (the verification column of `bridge/unknown-fields`); plan G11 is closed.
- `SESS-22` (a second `hello`), `SESS-21` (a request before the greeting) and `SESS-33`'s repeated call id and `path` self-reference are now in the shared matrix (`SESS-64`); the other call-id forms are listed under G2's "Still outside the matrix".

### G6 Error text differences

The same kind of error reads differently in Rust, Node and Go (`SESS-77`). This file quotes Rust's text word for word only in §12 and §8; verifiers compare category, scope and error code, not text. The call-id number format differs in the same way (`SESS-33`): Rust and Python are lenient, the others strict. Unifying the text or the format later would be a separate behaviour change.

### G7 The wait signal of an asynchronous call: settled (2026-11-19, #1098)

- Decision: a call frame gets an optional boolean `awaited`, defaulting to `true` ("the result will be waited for"); only "fired and forgotten" writes `false`. It joined protocol 5 without a version bump: protocol 5 was still unreleased then (0.23 shipped on 2026-11-20) and all implementations upgrade together. A far end built from an earlier 0.23 prerelease treats `awaited` as an unknown field and fails the call per `SESS-66`; it must be upgraded to a build with the field.
- Why: before protocol 5's `awaited` the frame only had `sync`, so the host could not tell "fired and forgotten" from "awaited later". Recording every asynchronous call on a marked chain as a hop refuses calls that cannot deadlock (`specs/cross-runtime-sync` counterexample 8, `async-policy-b`); counting only `sync` loses the synchronous cycle through an object relay (a real deadlock). `awaited: false` lets the caller say "I do not wait", taking the call out of the edges; the `true` default keeps the old conservative reading, so no real edge is lost.
- Not adopted: a later `await` frame as the signal (the cycle has already formed before it, too late); capability negotiation (capabilities cannot extend fields, `SESS-69`); changing what `sync` means to "will be waited for" (`sync` also decides which thread waits; that would change `SESS-38`).
- Boundary: `awaited: false` is a trusted declaration; the host does not verify it. A false one loses a real wait edge (`SESS-40`'s explicit non-guarantee). `awaited` is ignored when `sync: true`. A receiver forwarding the call keeps the original signal.
- Verification: `an_asynchronous_forward_nobody_awaits_is_an_entry`, `an_awaited_forward_closes_the_cycle`, `a_forwarded_call_carries_the_callers_awaited_signal`, `a_call_says_when_its_result_is_not_waited_for` in `crates/rutis-bridge`; the shared malformed matrix of all seven implementations has the `awaited`-wrong-type frame; `specs/cross-runtime-sync`'s `async-forgotten-policy-b` (passes after the fix) and `async-policy-b` (counterexample 8 before it).

### G8 A frame across chunks: settled (2026-11-19, #1361, #1363)

- Decision: add the `chunk` frame and the `chunked` capability (`SESS-84`–`SESS-88`), implemented first in Rust (`rutis-bridge`) and the Node runtime (#1361), then in Bun, Python, Go, Swift and Kotlin (#1363); protocol 5 is unreleased, so no version bump. The reassembly cap is the session option `max_transfer` (256 MiB by default), which a host passes to runtime processes in `RUTIS_MAX_TRANSFER`; it is never unlimited. The design and the developer-visible output are in [parameters and results over one message's limit, carried across frames](design-chunked-frames-2026-11-19.en.md).
- Reason: for a cross-language value over one message's limit, business code can only slice, page and retry. Chunking in the session layer leaves the business API, service signatures and public API unchanged, and one call runs once.
- Not adopted: a protocol version bump (0.22 and 0.23 cannot run together anyway); raising only the transport's limit (it changes no other implementation's default); slices inside `args` (it would change `WireValue` and seven value codecs); concurrent transfers (both the receiver's ordering and the memory peak double); no reassembly cap (a far end that ignores the protocol makes this process allocate without bound).
- All seven implementations (Rust, Node, Bun, Python, Go, Swift and Kotlin) declare `chunked`; earlier builds do not, and sessions with them behave as in 0.22.
- Verification: `a_chunked_call_is_dispatched_once_after_its_last_chunk`, `an_incomplete_transfer_is_never_dispatched`, `a_chunk_out_of_order_ends_the_session`, `a_second_transfer_before_the_first_ends_ends_the_session`, `a_chunk_before_the_handshake_ends_the_session`, `a_chunk_from_a_far_end_without_the_capability_ends_the_session`, `a_chunk_with_invalid_base64_ends_the_session`, `a_frame_over_the_limit_is_chunked_to_a_far_end_that_declared_it` in `crates/rutis-bridge`; the real Rust↔Node test `crates/rutis-bridge/tests/value_roundtrip.rs::node_carries_frames_over_the_message_limit_whole`; each implementation's own tests: Node `node/rutis-runtime/test/chunked.test.mjs`, Bun `bun/rutis-bun/test/chunked.test.ts`, Python `python/rutis/tests/test_chunked.py` and `python/rutis/tests/test_chunked_cross_language.py`, Go `go/rutis/internal/peer/chunked_test.go` and `go/rutis/internal/peer/chunked_cross_test.go`, Swift `swift/rutis/Tests/SelfTest/ChunkedTests.swift` and `swift/rutis/Tests/SelfTest/ChunkedInteropTests.swift`, Kotlin `kotlin/rutis/rutis/src/jvmTest/kotlin/arcships/rutis/ChunkedTest.kt`.

## Related files

- [Session trace contract](formal-trace.en.md)
- [Parameters and results over one message's limit, carried across frames](design-chunked-frames-2026-11-19.en.md)
- [Migration to 0.19](migration-0.19.en.md)
- [Migration to 0.23](migration-0.23.en.md)
- [Protocol plugin design](design-protocol-plugins-2026-09-25.en.md) §6.1 (wire formats of tagged values)
- [Cross-process mount design](design-protocol-plugin-mount.en.md)
- [Session tags design](design-session-tags-2026-10-12.en.md)
- [Object references](design-object-refs-2026-10-27.en.md)
- [Error messages](design-error-messages-2026-11-03.en.md)
- [Guarantee registry](guarantees.en.md), the `bridge/*` rows
- [CI](ci.en.md) "Conformance matrix"
- [0.22 formal coverage plan](plan/formal-verification-0.22.en.md) WS2
- `crates/rutis-bridge/src/session/protocol.rs`
- `crates/rutis-bridge/src/session/rpc.rs`
- `crates/rutis-bridge/src/session/testing.rs`
