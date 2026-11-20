# Parameters and results over one message's limit, carried across frames

[中文](design-chunked-frames-2026-11-19.md)

Status: implemented (released in 0.23; this joins session protocol 5); Rust and Node by #1361, the other five (Bun, Python, Go, Swift and Kotlin) by #1363. Date: 2026-11-19.
Issue: [#1361](https://github.com/arcships/rutis/issues/1361), [#1363](https://github.com/arcships/rutis/issues/1363), part of the 0.23 beta.

## 1. Where things stand

The transport sets one message's limit: process pipes and WebSockets default to 16 MiB (16777216 bytes), and the memory channel has none (session protocol `SESS-70`). The sender compares the whole encoded frame before writing it; over the limit, that call fails with code `bad-value`, and `data` names the part and the limit (`SESS-71`):

```text
svc.take: argument 2 cannot be sent: the message would be 17825803 bytes, over the limit of 16777216 bytes (16 MiB)
```

Plugin code gets that line and `{"argument":2,"limit":16777216,"part":"argument"}`. To move an image, an audio clip or an export file from one language to another, the business code has to slice, page, call several times and reassemble, and then handle what "the transfer broke halfway" means for retries. A result over 16 MiB fails the same way on the result side (`part: "result"`). #1361 asks that a cross-language call be transparent about a parameter or result over one message's limit: the business writes one call and the far end gets the whole value.

Scale: an encoded frame over 16 MiB hits the limit; a string value costs two quotes and escapes in JSON, and an array adds separators and `WireValue` wrappers. Before this change, no implementation could carry a 17 MiB string argument from Rust to Node.

## 2. Decisions

**D1 Add a `chunk` frame, in session protocol 5.** Protocol 5 is the version 0.23 introduces and has never been released; adding a frame to it before its first release does not bump the version (protocol specification `SESS-80`, the same treatment as G7). All seven implementations upgrade to 0.23 together, and older versions are refused at the handshake (`SESS-79`). **Dropped:** protocol 6 — 0.22 and 0.23 cannot run together anyway, so a bump would only add a migration step. **Dropped:** raising the transport's 16 MiB — the pipe and WebSocket limits are the transport's rule; raising this side's quota changes no other implementation's default, does not solve one message having to sit in memory whole, and a far end that does not understand it still ends the channel.

**D2 Negotiate with a capability, `chunked`; only when both sides declare it are frames chunked.** It declares "this side can reassemble `chunk` frames". The sender chunks an oversized frame only when the **far end** declared `chunked` (and this side did too); otherwise today's behaviour stays: the oversized call fails with `bad-value` and the frame is not written. All seven implementations declare it; an earlier build that does not behaves exactly as today and never receives a frame it does not know. **Dropped:** treat every protocol-5 implementation as taking `chunk` — an implementation that does not know it would end the session on an unknown `op`.

**D3 A chunk is a slice of the whole frame's JSON bytes, at most 8 MiB.** The sender cuts the already encoded frame bytes into consecutive slices and puts each, base64-encoded, into one `chunk` frame. The receiver buffers by `transfer`, and on `last: true` joins the bytes, decodes UTF-8 and hands the result to the existing session as one ordinary message. Frame boundaries, order and the decode path are unchanged. An 8 MiB slice is about 10.67 MiB in base64, inside the 16 MiB transport limit; when the transport limit is smaller (a WebSocket's `Limits.max_message` is configurable), the slice shrinks with it so one `chunk` frame fits. **Dropped:** a list of slices inside `args` — that changes `WireValue` and the value codecs of seven implementations and breaks "no business API change".

**D4 One transfer's chunks are sent consecutively, and one transfer runs at a time.** The sender queues every chunk of a frame together in the outbound order (one queue entry in Rust, one synchronous send in Node); other frames go before or after the group, never between. The receiver therefore keeps one transfer in progress: a new `transfer` before the current one's `last` is a protocol error. **Dropped:** concurrent transfers — that would make interleaving "the chunks of a frame" with "another frame" legal, force per-transfer ordering and flow control, and buy only two large frames in parallel at the cost of protocol complexity and twice the memory peak.

**D5 The reassembly and per-frame limit is 256 MiB by default, configurable per session or host, never unlimited.** A session's largest frame is set by each implementation's session option: Rust's `SessionOptions::max_transfer`, Node's and Bun's `maxTransfer`, Go's `Options.MaxTransfer`, Swift's `PeerOptions.maxTransfer` and Kotlin's `Peer.maxTransfer`; Python reads `RUTIS_MAX_TRANSFER`. The default is 256 MiB (268435456 bytes), sixteen times the 16 MiB transport limit, enough for file-sized data and a bound for one message. A deployment may also set it in one place with `RUTIS_MAX_TRANSFER`; the host passes it to the runtime processes it starts, so both ends use one value. The sender checks before slicing: a frame over the configured value is not sent, the call fails with `bad-value` and `limit` set to that value (268435456 by default):

```text
svc.take: argument 2 cannot be sent: the message would be 300000000 bytes, over the limit of 268435456 bytes (256 MiB)
```

The receiver adds up as it buffers: over the same configured value ends the session (it cannot tell which call it belongs to, so it cannot fail only that call), and nothing is truncated silently. **Dropped:** a fixed constant — a host such as Dim adjusts it to its memory budget; **Dropped:** no cap — a far end that ignores the protocol could make this process allocate without bound for one message.

**D6 An incomplete transfer is dropped when the session ends and never dispatched.** The receiver hands a frame to the session only after `last`, reassembly and decoding all succeeded. If the session ends midway (the transport closes, the far end exits, a protocol error), the buffer goes with it and the callee never ran. That is one acceptance item of #1361 and the other side of "exactly once": a chunk is not a call; only the whole frame is. **Dropped:** dispatch the chunks received so far — the business would see half a value, with no way to say "not finished".

**D7 All seven implementations land together.** `crates/rutis-bridge/src/session` (Rust) and `node/rutis-runtime/src/session.mjs` implement it first (#1361); #1363 has Bun (`bun/rutis-bun/src/session.ts`), Python (`python/rutis/rutis/peer.py`), Go (`go/rutis/internal/peer/chunks.go`), Swift (`swift/rutis/Sources/Rutis/Session/Peer.swift`) and Kotlin (`kotlin/rutis/rutis/.../peer/Peer.kt`) implement it too and declare `chunked`. All seven declare it in their greeting, so Rust chunks to any of them and the business API is unchanged. **Dropped:** stop once one real cross-language path passed — the others would keep the 16 MiB behaviour for a long time, while they upgrade to 0.23 anyway.

**D8 The plugin API, service signatures and public API are unchanged.** No new required configuration, no new plugin-visible concept, no signature change in `Endpoint`/`Connection`; what is added is protocol constants (`CHUNK_SIZE`, `DEFAULT_MAX_TRANSFER`), a session option (each implementation's `max_transfer` option and `RUTIS_MAX_TRANSFER`), a capability string and one `op` on the wire. The business keeps writing one call.

## 3. Rules

### 3.1 The `chunk` frame

A wire object with `op` `"chunk"`:

| Field | Type | Meaning |
| --- | --- | --- |
| `op` | `"chunk"` | The frame tag. |
| `transfer` | `string` | The transfer id, `<sending endpoint>:<n>`, increasing from 1 within the session on the sending side, non-empty. |
| `seq` | integer | The chunk's number, from 0, plus one each time. |
| `last` | bool | This is the last chunk. |
| `data` | `string` | One slice of the original frame's UTF-8 JSON bytes, base64 (RFC 4648 §4, padded, canonical, no whitespace). |

`chunk` is not a `Frame` variant: the session's read layer handles it before `Frame` decoding (the reader threads of Rust, Python, Go, Swift and Kotlin, Node's and Bun's `Session.receive`), and the bytes it produces are one ordinary message. Unknown fields, an unknown `transfer` or a field of the wrong type are malformed frames: the session ends with a transport error (there is no call to fail). `hello` is never chunked (it is tiny, and chunking depends on the capability declared in `hello`).

### 3.2 Sending

- Trigger: the encoded frame is over the transport's per-message limit, **and** both this side and the far end declared `chunked` in `hello.capabilities`. Each implementation takes the limit from its channel: Node and Bun from the `maxMessage` constructor option (16 MiB by default), Python from `_max_message()`, Go from the session's `maxMessage`, Swift from `maxMessage`, and Kotlin from the session's `channelLimit` (16 MiB by default).
- Slice size: `min(8 MiB, what the transport limit allows)`; the latter guarantees `base64(slice) + the fixed cost of a chunk frame <= the transport limit`. A computed size of 0 (a very small transport limit) means no chunking, handled as "the far end does not support it".
- Every `chunk` frame of one frame is consecutive in the outbound order, with no other frame between.
- Before slicing, the whole frame is checked against the session's `max_transfer` (256 MiB by default); over it the frame is not sent and the call fails as in `SESS-71`, with `limit` set to the configured value.
- The outbound backlog (`SessionOptions::outbound_limit`) counts one transfer as one message, its bytes as the original frame; an empty queue still takes one message however large (`SESS-75` unchanged).

### 3.3 Receiving

- On a `chunk`:
  1. `chunk` before the greeting: `request before protocol handshake` (as any other frame, `SESS-21`).
  2. The far end did not declare `chunked`: a protocol error, the session ends.
  3. `transfer` non-empty, `seq` a safe integer increasing one by one from 0, `last` a bool, `data` canonical base64; otherwise the session ends.
  4. Another `transfer` is already in progress: the session ends.
  5. The accumulated size is over the session's `max_transfer` (256 MiB by default): the session ends.
  6. `last: false`: read the next frame; `last: true`: join the bytes, decode UTF-8 strictly, and parse and process the result on the existing `parse_frame` path. If the reassembled result is itself a `chunk` frame, the session ends (no nesting).
- A reassembled frame has exactly the rights of an ordinary one: the depth limit (208 levels), call-level refusal of unknown fields (`SESS-66`), call ids, references and cancellation are unchanged.
- When the session ends (normally or not), every unfinished buffer is dropped; a transfer that never reached `last` is never dispatched and never answered.
- One frame's chunks are consecutive on the channel, so the receiver's "one frame at a time" order (`SESS-18`) is unchanged: no other frame appears between `chunk` frames.

### 3.4 Failure scope and text

| Situation | Result | Text (Rust, word for word) |
| --- | --- | --- |
| The sender's frame is over the session's cap (256 MiB by default) | Only that call fails, `bad-value`, `limit` is the configured value | `svc.take: argument 2 cannot be sent: the message would be 300000000 bytes, over the limit of 268435456 bytes (256 MiB)` |
| The far end does not declare `chunked` (an earlier build) and the frame is over the transport limit | As today: only that call fails, `limit` is the transport limit | `svc.take: argument 2 cannot be sent: the message would be 17825803 bytes, over the limit of 16777216 bytes (16 MiB)` |
| `seq` is not consecutive | The session ends, transport error | `chunked transfer node:3: expected chunk 2, got 3` |
| Another `transfer` starts before this one ended | The session ends, transport error | `chunked transfer node:3: transfer node:4 started before it ended` |
| Reassembly is over the session's cap | The session ends, transport error | `chunked transfer node:3: over the limit of 268435456 bytes (256 MiB)` |
| The far end did not declare `chunked` but sends `chunk` | The session ends, transport error | `chunk frame from a far end that did not declare chunked` |
| `data` is not canonical base64 | The session ends, transport error | `chunked transfer node:3: invalid bytes (11 characters): expected base64 (RFC 4648 §4) with padding` |
| The reassembled bytes are not valid UTF-8 or not a JSON frame | The session ends, transport error | As today for a malformed message (`SESS-63`) |
| The transport closes midway | The session ends with a transport error, nothing dispatched | `peer disconnected` (pipe), or the transport's own reason |

The same kind of error may read differently on Node's side (`SESS-77`); verification compares category, scope and error code.

### 3.5 Limits and what is not guaranteed

- One transfer's reassembly limit is the session's `max_transfer`, 256 MiB by default, set per session or host (`RUTIS_MAX_TRANSFER`); over it ends the session, and a sender over it fails only that call.
- At most one transfer is unfinished at a time; a second `transfer` is a protocol error. That is the bound on concurrent unfinished transfers and does not change with configuration.
- Chunking changes none of the depth, call-id, reference, cancellation or wait-graph rules.
- The memory channel has no transport limit and is never chunked, but the session's `max_transfer` still bounds one frame: no session is unlimited.
- Nothing guarantees that two ends that do not declare `chunked` (earlier builds) can carry a value over the transport limit (today's behaviour is unchanged).
- Nothing guarantees efficiency through middle layers beyond the transport: reassembly needs the whole frame in the far end's memory, with a peak of about 1.4 times the frame (bytes plus base64).
- Nothing guarantees that the `data` boundaries mean anything: the business still sees the whole frame.

## 4. What developers see

### Plugin authors

**When it works:** the code is unchanged. When a parameter or result encodes to over 16 MiB and the far end is a runtime that declared `chunked` (all seven implementations of 0.23), the call succeeds and the far end gets the whole value. No new configuration, no new output line, no frame concept to learn.

```ts
const bytes = await ctx.remote.node.readFile('export.bin') // 20 MiB
```

A 20 MiB value crosses between the Rust host and the Node runtime with one call from the business; do not slice, page or retry by hand.

**When it fails (word for word, Rust side):**

| Situation | Output |
| --- | --- |
| The value encodes to over the session's cap (256 MiB by default, a host may raise it) | `svc.take: argument 2 cannot be sent: the message would be 300000000 bytes, over the limit of 268435456 bytes (256 MiB)`; code `bad-value`; `data` is `{"argument":2,"limit":268435456,"part":"argument"}` |
| The far-end runtime did not declare `chunked` (an earlier build) and the value is over the transport limit | As in 0.22: `svc.take: argument 2 cannot be sent: the message would be 17825803 bytes, over the limit of 16777216 bytes (16 MiB)`; `data`'s `limit` is 16777216 |

**Concepts to learn:** none added. Chunking is between runtimes; a plugin still sees values and calls.

### Host authors and deployers

**When it works:** the session protocol gains no required configuration; the WebSocket subprotocol is still `rutis.5`. Between the Rust host and the other six runtimes of 0.23 (Node, Bun, Python, Go, Swift and Kotlin), parameters and results over 16 MiB are usable directly, and one call runs once. One frame is 256 MiB by default; a host raises or lowers it to its memory budget with the session option (`SessionOptions::max_transfer`, `maxTransfer`, `Options.MaxTransfer`, `PeerOptions.maxTransfer`, `Peer.maxTransfer`) or `RUTIS_MAX_TRANSFER`, and passes it to the runtime processes it starts.

**When it fails:**

- The session ends midway through a transfer: the session ends with a transport error and the unfinished request is not dispatched; in-flight calls fail with `peer-gone` as today.
- The far end does not declare `chunked` (an earlier build): behaviour is as in 0.22; an oversized call fails explicitly and no `chunk` frame is sent by mistake.
- `RUTIS_MAX_TRANSFER` is not a positive integer: the host or runtime fails to start, with `RUTIS_MAX_TRANSFER "abc": expected a positive number of bytes`; make it a positive integer or unset it.
- The two ends' caps differ: the sender chunks up to its own, the receiver reassembles up to its own, and the side over its cap ends the session with `chunked transfer node:3: over the limit of <n> bytes`. Set `RUTIS_MAX_TRANSFER` in the host process (it passes it to the runtime processes it starts), and the same value on runtimes started directly.

**Concepts to learn:** none added; to change the cap, one knob, `max_transfer` / `RUTIS_MAX_TRANSFER`. Deployment is still "upgrade every end of a deployment to 0.23 together".

### Runtime authors and contributors

**When it works:** after implementing `chunk` frames and declaring `chunked`, frames over the transport limit arrive whole at the far end; without the implementation, declare nothing and behaviour is as today.

**When it fails:** if the transport ends midway, the unfinished transfer is not dispatched; a non-consecutive `seq`, interleaved transfers, over the configured cap, or chunking to a far end that did not declare all end the session with a transport error. The text may differ from other implementations.

**Concepts to learn:** the `chunk` frame, `transfer`, `seq`, `last`, the `chunked` capability and the reassembly limit.

## 5. Compatibility and migration

- Protocol 5 is new (0.23 introduces it), and adding a frame does not bump it; 0.22 and earlier are still refused at the handshake (`SESS-79`).
- Capabilities are declarative: a 0.23 Rust host chunks only to a far end that declared `chunked`. All seven implementations of 0.23 (Rust, Node, Bun, Python, Go, Swift and Kotlin) declare it; earlier builds do not, so sessions with them keep the 16 MiB behaviour and never receive a `chunk` frame.
- A pre-release 0.23 build that predates `chunked`: it does not declare the capability, so this side does not chunk; if it did declare it, it would also understand `chunk` (declaring means implementing), so nothing is sent that it cannot read.
- Business code, service signatures, the plugin API and the `rutis.json` format are unchanged.
- Migration guide: [migrating to 0.23](migration-0.23.en.md) gains a section "Cross-language calls over 16 MiB".

## 6. Implementation

- `crates/rutis-bridge/src/session`: parsing and serializing `Chunk`, the sender's slicing, the receiver's reassembly, the `CHUNK_SIZE`/`DEFAULT_MAX_TRANSFER` constants, `SessionOptions::max_transfer` (with `from_environment` and `MAX_TRANSFER_ENV`), and `Endpoint::rust` declaring `chunked`.
- `crates/rutis-host/src/connect/spawn.rs`: the host passes `max_transfer` to the runtime process it starts in `RUTIS_MAX_TRANSFER`.
- `node/rutis-runtime/src/session.mjs`: `chunked` in `CAPABILITIES`, the sender's slicing, reassembly in `receive`, and the `maxTransfer` option and `RUTIS_MAX_TRANSFER` (256 MiB by default).
- `bun/rutis-bun/src/session.ts`, Python `python/rutis/rutis/peer.py` (with `rutis/_chunks.py`), Go `go/rutis/internal/peer/chunks.go`, Swift `swift/rutis/Sources/Rutis/Session/Peer.swift` and Kotlin `kotlin/rutis/rutis/src/shared/kotlin/arcships/rutis/internal/peer/Peer.kt` (#1363): `chunked` declared, the sender's slicing, reassembly on the read thread, each one's cap option (`maxTransfer` / `Options.MaxTransfer` / `PeerOptions.maxTransfer` / `maxTransfer`) and `RUTIS_MAX_TRANSFER` (256 MiB by default).
- Protocol specification `docs/protocol-session.md`/`.en.md`: the §2.1 frame table, the §2.4 capability table, the §10 limits, §11 rules (`SESS-84`–`SESS-88`) and the §12 output.
- Guarantee register `docs/guarantees.md`/`.en.md`, row `bridge/large-and-deep-values`: the promise after chunking and its verification.
- Migration guide `docs/migration-0.23.md`/`.en.md`: a new section.
- Tests:
  - Rust ↔ Node (the real Node runtime, a Unix socket, the 16 MiB pipe): a parameter over 16 MiB and a result over 16 MiB both cross whole; a `tally` method shows every call ran once.
  - Rust receiving: a scripted far end splits one frame into `chunk` frames; whole, it is dispatched once; when the channel closes midway it is never dispatched; a wrong `seq`, interleaved transfers, over the limit and `chunk` before the handshake all end the session.
  - Node receiving and sending: the same reassembly, failure and single-dispatch assertions; a far end that did not declare `chunked` (an earlier build) still gets the old over-limit error.
  - Bun, Python, Go, Swift and Kotlin (#1363): the same reassembly, failure and single-dispatch assertions; one session each with the real Node runtime, a parameter and a result over 16 MiB crossing whole, with `tally` showing each call ran once.
  - Configuration: with a smaller Rust `max_transfer`, a frame over it fails only its call and a transfer over it ends the session naming the configured value; the session options of Node, Bun, Go, Swift and Kotlin, and `RUTIS_MAX_TRANSFER` in Python, Go and Swift, have their own cases.
