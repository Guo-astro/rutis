# Calls in a session: start, run, reply, cancel, references and a closed session (TLA+)

[中文](README.md)

TLC exhausts every interleaving of a call from its start to its settlement in one session: the call number, the request arriving, the method running, the reply, a cancel, a late reply, the grant and release of a reference, and the session closing. It corresponds to the call lifecycle part of issue [#613](https://github.com/arcships/rutis/issues/613), quality standard Q11.2; the design is [session protocol](../../docs/protocol-session.en.md) §5, §6, §7, §10, the [session trace contract](../../docs/formal-trace.en.md) §3.4, §3.5, and the [formal verification design](../../docs/design-formal-verification-2026-11-08.en.md) §3.4. This is the second block of #613: the first is [`row-lifecycle`](../row-lifecycle/README.en.md) (the rows of a session).

Two modules:

| Module | Participants | What it answers |
| --- | --- | --- |
| `CallLifecycle.tla` | the two endpoints of one session | one call from its start to its settlement; the reader write-back and backpressure; whether a callee answers after a cancel; a synchronous call abandoned locally; giving the argument references back; `PinnedTarget`; frames still on the channel after a close; the host's `outbound_limit`; the three rules for the caller's cancelled table |
| `CallRelay.tla` | A, the relay M and the owner C, over two sessions | a cancel over two hops (`CancelReaches`) and the release over two hops (`RelayReleasesImport`); the relay's own cancelled table |

This directory does not model the wait graph across runtimes, the call chain `path`, the cycle check of synchronous calls, the frame order of events, rows hosted by a node, the handshake or authentication; the synchronous wait appears only as one ghost edge for "abandoned locally". `CallRelay.tla` has no close (see "Not covered").

## The design and the code it stands for

| Model part | Design | Implementation |
| --- | --- | --- |
| A call number grows strictly per session and per side, from 1; one number belongs to one call, and a number that already appeared is not a new call | Session protocol §5.1 (`SESS-32`, `SESS-33`, `SESS-34`) | `Calls::admit` and `Calls::received` in `crates/rutis-bridge/src/session/rpc.rs` |
| A call is settled exactly once: a reply takes its waiter out first | Session protocol §5.4 (`SESS-41`, `SESS-42`) | `crates/rutis-bridge/src/session/rpc.rs:2494` (`reply_target`) |
| A cancel acts on one call: a queued one is dropped without a reply, a running one finishes and answers, a cancel never closes the session | Session protocol §7 (`SESS-56`, `SESS-57`, `SESS-60`) | `crates/rutis-bridge/src/session/rpc.rs:2693` (`Frame::Cancel`), `:2038` (`Connection::cancel`) |
| The late reply of a cancelled call is discarded and counted, not a protocol error | Session protocol §7 (third bullet of `SESS-57`), trace contract §3.4 | `cancelled`/`orphans` in `reply_target` (`crates/rutis-bridge/src/session/rpc.rs:2494`) |
| A `cancel` for a call this side does not have is ignored (a cancel that overtook its reply) | Session protocol §7 (`SESS-58`) | `crates/rutis-bridge/src/session/rpc.rs:2693`; test `a_cancel_for_an_unknown_call_is_ignored` |
| A cancel does not undo a grant already made; releasing the local proxy sends `release` as usual | Session protocol §7 (`SESS-59`), §6.1 (`SESS-47`) | `Import`'s `Drop` (`crates/rutis-bridge/src/session/rpc.rs:992`) writes `Frame::Release` |
| A reply's value is decoded before it is judged: a discarded late reply still releases the reference it carried | Trace contract §3.4; the rollback rule of `SESS-44` | `crates/rutis-bridge/src/session/rpc.rs:2675` ("Decode first" in `Frame::Return`) |
| Close: calls still waiting fail with the session error, the export and import tables are cleared, frames not yet written go with the session | Session protocol §5.4, §6.1 (`SESS-42`, `SESS-51`), trace contract §4 | `crates/rutis-bridge/src/session/rpc.rs:1634` (`Connection::close`) |
| After one side closes, the other learns it from the end of the channel and ends too | Trace contract §3.0 | the reader closes its side when the channel ends (the read loop and `closed()` in `crates/rutis-bridge/src/session/rpc.rs`) |
| A frame still on the channel after the close: the reader looks at `closed` first, so the whole frame is refused — it settles nothing and grants nothing | Session protocol §5.4, §6.1 (`SESS-51`) | the `calls.closed` check at the top of `receive` (`crates/rutis-bridge/src/session/rpc.rs:2652`); `close` calls `closer.close(Close::Done)` before clearing the tables (`:1634-1668`) |
| The reader's write-backs go to the writer thread: `throw` (bad arguments, too deep), the `release` of a discarded late reply and the `release` of a dropped queued call's argument references are all made on the reader; what waits in the channel's `send` under backpressure is the writer | Session protocol §10 (`SESS-75`, `SESS-76`); [remote session design](../../docs/design-remote-session-2026-10-28.en.md) §3, §7 (#1005) | `Connection::write` and `Outbound::push` (`crates/rutis-bridge/src/session/rpc.rs:1825-1840,1387-1404`), `write_frames` (`:1411-1444`) |
| Whether a callee answers after a cancel: both are allowed, and the caller must take either. Rust does not answer a queued call and answers a running synchronous one; the model runs both | Session protocol §7 (`SESS-57`, `SESS-61` "not guaranteed"); the cancelled table is #1099's | `Frame::Cancel` (`crates/rutis-bridge/src/session/rpc.rs:2693`), `respond_to` (`:3083`) |
| A synchronous call abandoned locally sends no `cancel`: the caller keeps its waiter until the reply, and the reader drops the wait-graph edge when it reads the reply; forgetting both without a `cancel` makes the reply an unknown call and ends the session | Session protocol §5.4 (`SESS-41`); trace contract §3.4 | `request_sync` (`crates/rutis-bridge/src/session/rpc.rs:2004-2035`, no drop guard), `CancelOnDrop` (`:1115`, async calls only), `waits::returned` in `reply_target` |
| When the callee drops a call still queued, the references its arguments carried go with `Accepted`: each `Import`'s `Drop` writes `release` on the same session, on the reader thread | Session protocol §6.1 (`SESS-47`); §8.2 (only the call fails) | `calls.incoming.remove` in `Frame::Cancel` (`crates/rutis-bridge/src/session/rpc.rs:2696-2699`), `Import`'s `Drop` (`:992-1009`) |
| `PinnedTarget`: an admitted call holds its target object (the `Arc<Object>` in `Accepted`), so the caller's `release` may drop the export entry without affecting it; grants are released by count, a release of an older grant does not take a newer one, and a release above the grants ends the session | Session protocol §6.1 (`SESS-47`, `SESS-48`) | `export_object` and `Accepted` (`crates/rutis-bridge/src/session/rpc.rs:2753`, `crates/rutis-bridge/src/session/rpc/relay.rs:79-87`), the count check in `Frame::Release` (`crates/rutis-bridge/src/session/rpc.rs:2709-2729`) |
| The host's `outbound_limit`: a writer queue at its bound that the channel cannot take ends the session (`the far end is not reading`) instead of queueing more | Session protocol §10 (`SESS-75`, `SESS-76`) | `Outbound::push` (`crates/rutis-bridge/src/session/rpc.rs:1387-1404`), `write_frames` (`:1411-1444`) |
| Multi-hop relay: the relay exports another session's reference as a relay of its own, rewrites the call chain and forwards the call; a cancel sends `cancel` for the forwarded request it drops; a release travels the relay chain, with the original reference's count | Session protocol §6.2 (`SESS-54`), §7 (`SESS-59`); `docs/guarantees.md` `bridge/multihop-cancel-and-release`; trace rules M1-7.1-1, M1-7.1-2 | `relay` and `encode_import` in `crates/rutis-bridge/src/session/rpc/relay.rs` (`:19-78,93-175`), `CancelOnDrop` (`crates/rutis-bridge/src/session/rpc.rs:1115`), `Awaiting`'s `_cancel` (`:1060-1064`), `Import`'s `Drop` |
| The cancelled table is open (#1099): the caller's `calls.cancelled` is cleared only by a late reply, and no implementation answers a call cancelled while queued; the reply rule itself is written down — either reply is allowed and the caller must take both (`SESS-61`) | Session protocol §7 (`SESS-57`, `SESS-61`), G3 | `calls.cancelled`, `orphans` (`crates/rutis-bridge/src/session/rpc.rs:1072-1080`), `Connection::cancel` (`:2038-2053`) |

## The model: one session (`CallLifecycle.tla`)

- **Participants**: the two endpoints of one session. A starts calls; B admits them, runs the method and answers. Each direction is FIFO and delayed arbitrarily long: `invoke`, `cancel` and `release` go A to B, `return`, `throw`, `ack` (only under `CancelTable = "ack"`) and `release` (the argument reference given back) go B to A. So a reply can still be on its way when a cancel is written, and a cancel can overtake the reply it meant to cancel.
- **A (the caller)**: `AStart` takes the next number and writes the `invoke`; `ADeliverLive` settles the call the reply names; `ACancel` gives up a call; `AAbandon` gives up a synchronous call **without** a `cancel`; `ADeliverLate` decodes the reply of a cancelled or abandoned call and discards it; `ADeliverForgotten` discards the reply of a call the bounded table has dropped; `ADeliverStray` handles a reply the caller can no longer place — `response for unknown call` per `SESS-41`, which ends the session; `ARelease` gives up what it holds; `AReadArgsRelease` reads the `release` of an argument reference; `AReadAck` reads a cancel acknowledgement; `AClose`/`ALearn` end this side; `AReadAfterClose` reads a frame left on the channel by the close.
- **B (the callee)**: `BReadInvoke` takes the call (still queued, and pins its target); `BRejectInvoke` refuses it as it reads it; `BRun` starts the method; `BReply` answers and writes out one grant; `BDrop` drops a call cancelled while running under the "never reply" rule; `BReadCancel` handles a cancel (a queued call is dropped without an answer, a running one answers or is dropped by the rule, an answered one is not touched, and under the ack rule it writes an `ack` back); `BReleaseArgs` writes `release` when a call lets its arguments go; `BReadRelease` gives up grants by count and `BReadReleaseBad` ends the session when the count is above them; `BClose`/`BLearn` end this side; `BReadAfterClose` reads the frames the close left.
- **Close**: either side may close. The closing side fails the calls still waiting, clears the reference table it holds and drops the frames not yet written; the other side does the same when the channel ends (`ALearn`/`BLearn`). With `AfterClose = "lost"` the frames already on the channel go with the close; with `"read"` they stay, and either side reads and refuses them (`receive` looks at `closed` first) — `AReadAfterClose`/`BReadAfterClose` change nothing but the channel, so `Done` stays reachable.
- **Channel and writer thread** (`Bound`, `Cap`): with `Bound = TRUE` each direction is a channel of `Cap` frames, fed from the writer thread's queue (`aout`, `bout`) by `FlushA`/`FlushB`. `ablock`/`bblock` are "the reader is stuck writing this frame".
- **Reader write-back** (`ReaderWrites`): `"direct"` is the shape before #1005; `"queued"` is today's. There are three reader write-backs: A's `release` when it discards a late reply, B's `throw` when it refuses a call it read, and B's `release` when it drops a queued call whose arguments carried a reference (`BReleaseArgs`, `c ∈ Args`).
- **Reply after a cancel** (`Replies`): `"always"` answers a call cancelled while running as usual; `"never"` drops it in `BDrop` without a frame. A call cancelled while queued is never answered under either rule (`SESS-57`).
- **A synchronous call abandoned locally** (`Sync`, `Abandon`): a call `c ∈ Sync` records a wait-graph edge at A (`aedge`) and a mark at B (`bmark`). `"keep"` leaves both until the reply; `"drop"` throws them away without a `cancel`.
- **Argument references** (`Args`): a call `c ∈ Args` carries one reference of A's in its `invoke`. A records what it granted, was given back and read back (`aArgGiven`, `aArgsOut`, `aArgBack`); B records that the call let its arguments go (`bArgsGiven`). `BReleaseArgs` is that write-back; the `argLeaks` mutation drops the arguments without writing anything back.
- **Pinned target** (`RefCalls`): a call `c ∈ RefCalls` names an object B exports, and A must hold a grant of it. `BReadInvoke` pins it; `BRun` never looks the entry up again, and `bLost` counts the calls dropped for want of a target (only in the mutation). `BReadRelease` subtracts the count and drops the entry only at zero: a release of an older grant leaves a newer one in place. The `releaseDropsAll` mutation drops the whole entry on any release, so the second release becomes `BReadReleaseBad` and ends the session.
- **The host's bound** (`OutboundLimit`, `Limit`): the writer queue holds at most `Limit` frames; `OverLimitA`/`OverLimitB` end the session with `"notReading"` when the queue is at the bound and the channel cannot take it. The `limitIgnored` mutation queues past the bound.
- **The caller's cancelled table** (`aTable`, `CancelTable`): `"late"` is today's — the entry stays until a late reply or the close; `"ack"` keeps it until the callee acknowledges, so the callee must not answer; `"bounded"` keeps at most `TableCap` entries, drops the oldest into `forgotten` and discards the replies to those calls (`SESS-41` has to tolerate it); `"forget"` drops the entry at once, so the late reply of a running cancelled call becomes an unknown call.
- **Termination**: `Done` (`AllSettled`) is available only when every call is settled on both sides, no channel or writer queue holds a frame, no reader is blocked, and every argument write-back is decided. TLC's deadlock check can then tell "the work is done" from "both readers wait for each other".

## The model: a relay (`CallRelay.tla`)

- **Participants**: A (the caller), M (the relay, the host) and C (the owner), over A–M and M–C. C granted the object to M (M holds an `Import`), and M exported it to A as a relay (an object whose body is that import). The initial state is that reference already established: `mImport = mRelay = aHeld = cGrants = 1`.
- **The call**: `AStart` writes `invoke` on A–M naming the relay (`aHeld > 0`); `MReadInvoke` takes it; `MForward` writes `call` on M–C with a number of M's own and waits in `mtable`; `CReadCall`/`CRun`/`CReply` run and answer; `MDeliverLive` settles M's own call and answers A with **A's** number; `ADeliverLive` settles A's call.
- **The cancel**: `ACancel` writes `cancel` on A–M; `MReadCancel` drops the incoming call still queued, or — when the forward is still waiting (`mfwd = "waiting"`) — marks it `"cancelled"` and **writes `cancel` to C**, which is what `CancelOnDrop` does when `Awaiting._cancel` is dropped; an answered call is not touched (`SESS-58`). `CReadCancel` drops a queued forward, answers or drops a running one by the `Replies` rule, and records `reachedC`.
- **The release**: `ARelease` writes `release` on A–M; `MReadRelease` subtracts from `mRelay`, and at zero drops the export entry — the relay object goes with it and its import's `Drop` writes `release {count: mImport}` on M–C, zeroing `mImport`; `CReadRelease` subtracts from `cGrants`. A cancel does not undo a grant: a `cancel` and a `release` may come in either order.
- **Termination**: `Done` (`Quiescent`) requires all four directions of both sessions to be empty and no call in flight at any of the three participants.

## Simplifications

- The scale is deliberately finite: most configurations have one caller, one callee, two calls, at most one cancel and one reference. `call-backpressure-*` use three calls, one cancel and capacity 1 per direction: to block both readers at once, the cancelled call gives A a late reply, the refused call gives B a `throw`, and a third call's reply refills the other direction after the reader there has read; with two calls that path is not reachable. With an argument reference (`Args = {"c2"}`) B's reader has one more write-back (the `release` of a dropped queued call's arguments), which is why `call-backpressure-*` grew (see "Results").
- The handshake is not modelled: the model starts with both sides greeted and the session usable. The version, the endpoint, the capabilities, the order of `hello` and the 10-second deadline are not modelled (`remote-lease` covers the takeover and the rest of the handshake); this model starts at the first call.
- The frames are `invoke`, `cancel`, `release` and `ack` (A to B) and `return`, `throw`, `ack` and `release` (B to A). `throw` only comes from a reader refusing a call it read; a method's own error settles as `return` does and is not modelled separately. `call`, `get`, `await`, `events.*` and `rows.*` are not modelled.
- Only "grows, never reused" of a call number is modelled: the prefix, the canonical decimal form, the 2^53 bound and "not in the frame's `path`" are not. The receiver's refusal of a number that does not grow has no reachable path either — a correct sender never repeats a number, and a repeated number is exactly the `reuseId` mutation.
- Of the protocol errors around references only "a release count above the grants" is modelled (`BReadReleaseBad`): a correct side never sends one, and the `releaseDropsAll` mutation is what makes it reachable.
- The call chain `path`, `rebase`, the session tags and the cycle check of synchronous waits (`SyncWaitCycle`) are not modelled: that is the scope of forwarding across runtimes (the `cross-runtime-sync` model). `Sync` only records an `aedge` ghost. The relay model has no `rebase` either: the relay only swaps A's number for its own and back.
- Values, error graphs, `bad-value`, `bad-frame` and the 16 MiB limit are not modelled. `Bound`/`Cap` model channel capacity and the writer thread's backpressure only; `outbound_limit` is modelled by its shape (a queue at the bound that the channel cannot take ends the session), not by bytes.
- There is one argument reference, on at most one call: `Args` names one call. In the implementation an argument list may carry any number of references, each `Import` writing its own `release`; the shape is the same.
- Pinning does not model the object's kind or a `kind` mismatch, only "the table entry disappeared before the method ran"; the other `SESS-48` errors are not modelled.
- The duration of a method is not modelled: a call goes from `queued` to `running` to its reply, and the reply may be written at any later point. There is no timing and no promise about the order calls run in.
- The cause of a close is not a protocol frame: each side ends its own session and the other learns it from the end of the channel. `closeCause` only tells the wrong designs apart ("a cancel closes the session", "an unknown call closes the session", "the far end is not reading", "a release above the grants").
- The relay model has no close: `CallLifecycle.tla` covers it. A relay whose own session closes does **not** pass the release on to C: `Connection::close` clears `imports` before dropping `exports` (`rpc.rs:1667-1682`), so the `release` the relay writes when it is finally dropped lands on a closed session and is swallowed; C keeps the grant until its own session ends (which `SESS-51` covers). This is read off the code, not modelled.
- Memory order, real scheduling and frame encoding are not modelled; every action is atomic. `Done` stops on "no work in flight", not on a state of the implementation.

## Invariants and liveness

`CallLifecycle.tla`:

| Name | Content |
| --- | --- |
| `CancelKeepsSession` | A cancel never ends the session: the close that ends it is the caller's, the callee's, or the one the channel end brings (`closeCause ≠ "cancel"`) |
| `NoUnknownCall` | A conforming pair never ends a session with a reply the caller cannot place (`closeCause ≠ "unknownCall"`); `SESS-41` read as "a correct sender never produces one" |
| `AbandonKeepsSession` | A locally abandoned call does not end the session (the instance of `NoUnknownCall` that concerns abandonment) |
| `SettlesAtMostOnce` | A call is settled at most once: a reply, a cancel, a local abandonment and a session end never settle one call twice |
| `LateReplyDropped` | A late reply is discarded: a cancelled or abandoned call keeps the outcome it got; each call has at most one reply to discard |
| `OwnReply` | The reply that settles a call is the reply of that call |
| `NoIdReuse` | A call number belongs to one call |
| `NoOverRelease` | A gives back only what it was granted: a grant is released, dropped or cleared once, never twice |
| `GrantsConserved` | At rest every unit B granted is still held by A or has been given back |
| `ClosedClearsTables` | A closed side holds no reference |
| `FailUnfinishedOnClose` | A closed session leaves no call unfinished on either side |
| `LateFramesInert` | A frame read after the close changes nothing: no call is settled after the close that failed it, and no reference is taken into a closed session |
| `PinnedTarget` | An admitted call is not dropped for want of its target, and a release of an older grant does not take a newer one (the session does not end on a release count it granted) |
| `ArgRefsReleased` | At rest A's export table holds none of the references it granted through arguments |
| `ArgRefsCounted` | The argument grant is counted once: given out, read back and still out agree |
| `BacklogBounded` | The writer queue never holds more than the host allows |
| `CancelledTableBounded` | The bounded table keeps at most `TableCap` entries (the "bounded table" option of #1099) |
| `CancelledEntriesDrain` | At rest A's cancelled table has no entry that only a late reply would clear (the `BoundedTables` of #626) |
| `AbandonedCallsClean` | At rest a locally abandoned call leaves no wait-graph edge |
| `WaitingSettles` | Liveness (only in configurations without a close): a call A waits for is answered, cancelled or abandoned |

`CallRelay.tla`:

| Name | Content |
| --- | --- |
| `CancelReaches` | A cancel M still had to forward reaches C over both hops: once M has forwarded and the sessions are quiet, C has read it (a call already answered needs no cancel) |
| `RelayReleasesImport` | At rest, A holding no grant of the relay means the relay dropped the import and C holds no grant either (M1-7.1-2) |
| `GrantsConserved` | The grant count travels the chain and is never counted twice: what A read back is at most what M took back, which is at most what C read back |
| `SettlesAtMostOnce` | A call is settled at most once at A |
| `LateReplyDropped` | A late reply is discarded at most once per hop |
| `RelayCancelledEntriesDrain` | At rest the relay's own cancelled table is empty (#1099: that entry exists once per hop) |
| `WaitingSettles` | Liveness: a call A waits for settles |

## Results

TLC 2.19 (tla2tools v1.7.4), Java 21.0.12 (Temurin), 32 cores, `-workers auto` (4 for the small configurations). `ok` means the exhaustive search completed and every listed property holds; a violated configuration stops at the first counterexample and a deadlock configuration at the first deadlock, so its state count depends on the search order and may differ from run to run. All 35 configurations were run with `run.sh` twice on 2026-11-10; the whole directory takes about 70 seconds (machine load varies).

| Configuration | Content | Result | Distinct states | Time |
| --- | --- | --- | --- | --- |
| `call-healthy` | Two calls, at most one cancel, both sides may close; every safety invariant (including `NoUnknownCall`, `LateFramesInert`, `PinnedTarget`, the `ArgRefs*` and `BacklogBounded`) | ok | 1,155 | 1 s |
| `call-progress` | No close; only the liveness property (`WaitingSettles`) | ok | 409 | 1 s |
| `call-backpressure-before` | Three calls, one cancel, capacity 1 per direction and one argument reference; the reader writes back itself (before #1005), deadlock check on | **deadlock** | about 23,000 (stops at the deadlock) | 1–2 s |
| `call-backpressure-after` | The same calls, cancel, capacity and argument reference; the reader hands its write-back to the writer thread's queue | ok (no deadlock) | 153,550 | 10–14 s |
| `call-backpressure-limit` | The same calls and the pre-#1005 shape, with the host's `outbound_limit = 2` | ok (no deadlock: a queue at the bound that the channel cannot take ends the session with `notReading`; `BacklogBounded` holds) | 55,276 | 2 s |
| `call-cancel-reply` | The replying implementation: a call cancelled while running answers as usual, the late reply is discarded and its reference given back; no close | ok | 409 | 1 s |
| `call-cancel-no-reply` | The non-replying implementation: a call cancelled while running is dropped by `BDrop`, no frame | ok | 437 | 1 s |
| `call-gap-cancel-table` | Today's caller cancelled table (cleared only by a late reply) plus a call cancelled while queued | violated `CancelledEntriesDrain` | about 300–400 (stops at the counterexample) | 1 s |
| `call-gap-ack` | A cancel acknowledgement: the callee answers every cancel with `ack` and the caller clears its entry on it; together with "never answer after a cancel" | ok (`CancelledEntriesDrain`, `NoUnknownCall`) | 545 | 0–1 s |
| `call-gap-unknown-late` | The caller forgets the cancelled call at once, the callee answers a cancelled running call as usual | violated `NoUnknownCall` | about 300 (stops at the counterexample) | 0–1 s |
| `call-gap-bounded` | A bounded table: at most `TableCap = 1` entries, the oldest dropped; the reply to a forgotten call is discarded | ok (`CancelledTableBounded`, `NoUnknownCall`) | 60,358 | 2–3 s |
| `call-sync-abandon` | A synchronous call given up locally without a `cancel`, the waiter kept until the reply | ok | 615 | 1 s |
| `call-after-close` | A frame left on the channel by the close is read and refused by either side | ok (`LateFramesInert`) | 2,669 | 1–2 s |
| `call-pinned-target` | A call names an object and the caller releases its grant before the method starts; a release of an older grant does not take a newer one | ok (`PinnedTarget`) | 209 | 1–2 s |
| `call-arg-release` | A queued call is cancelled and the reference its arguments carried is written back as `release` by the reader | ok (`ArgRefsReleased`, `ArgRefsCounted`) + `WaitingSettles` | 847 | 1 s |
| `call-mut-cancel-closes` | A cancel closes the session | violated `CancelKeepsSession` | 11 | 1 s |
| `call-mut-late-reply-twice` | A late reply is taken for a live one and settles the call again | violated `SettlesAtMostOnce` | about 500 (stops at the counterexample) | 1 s |
| `call-mut-late-reply-keeps-grant` | A discarded late reply does not release the reference it carried | violated `GrantsConserved` | 381 | 1–2 s |
| `call-mut-close-leaks` | Closing keeps the export table | violated `ClosedClearsTables` | about 350 (stops at the counterexample) | 0–1 s |
| `call-mut-reuse-id` | A new call may take a call number again | violated `NoIdReuse` | 11 | 1–2 s |
| `call-mut-reuse-id-settles` | The second half of the same mutation: the reused number lets a reply settle another call | violated `OwnReply` | about 550–800 (stops at the counterexample) | 1–2 s |
| `call-mut-abandon-drops` | A local abandonment throws the waiter and the edge away without a `cancel` | violated `AbandonKeepsSession` | about 400–550 (stops at the counterexample) | 1 s |
| `call-mut-abandon-leaks` | The late reply is absorbed but the wait-graph edge is not cleared | violated `AbandonedCallsClean` | 615 | 1 s |
| `call-mut-late-frame-acts` | A reply read after the close still settles the call | violated `SettlesAtMostOnce` | about 700–800 (stops at the counterexample) | 1 s |
| `call-mut-no-pin` | The target is looked up in the export table when the method starts | violated `PinnedTarget` | 71 | 0–2 s |
| `call-mut-release-drops-all` | Any release drops the whole entry | violated `PinnedTarget` | 447 | 1–2 s |
| `call-mut-arg-leak` | A dropped queued call's argument reference is not written back | violated `ArgRefsReleased` | about 550–650 (stops at the counterexample) | 0–1 s |
| `call-mut-limit-ignored` | The writer queue ignores the host's bound | violated `BacklogBounded` | about 130 (stops at the counterexample) | 0–1 s |
| `call-mut-forget-strict` | The bounded table forgot the entry but the caller stays strict about `SESS-41` | violated `NoUnknownCall` | about 1,400–2,100 (stops at the counterexample) | 1 s |
| `relay-healthy` | A call through the relay, a cancel and a release; `relay-healthy` and `relay-no-reply` are the two reply rules | ok | 5,464 | 1–2 s |
| `relay-progress` | Liveness on the relay | ok | 5,464 | 2–3 s |
| `relay-gap-cancel-table` | The relay's own cancelled table: a forwarded call C dropped without an answer leaves an entry | violated `RelayCancelledEntriesDrain` | about 600–1,500 (stops at the counterexample) | 1–2 s |
| `relay-mut-cancel-stops` | The relay drops the forwarded request but passes no cancel on to C | violated `CancelReaches` | about 500–1,200 (stops at the counterexample) | 0–1 s |
| `relay-mut-release-leaks` | The relay drops its export entry but keeps the import | violated `RelayReleasesImport` | about 300–400 (stops at the counterexample) | 1–2 s |

## Mutations and counterexamples

Every checked property has a deliberate error next to it, and TLC must find the counterexample (`violated:<name>` in `expected.txt`):

| Configuration | Mutation | Violated | Counterexample |
| --- | --- | --- | --- |
| `call-mut-cancel-closes` | `ACancel` closes A's session right there | `CancelKeepsSession` | one cancel leaves `aclosed = TRUE`, `closeCause = "cancel"` |
| `call-mut-late-reply-twice` | `ADeliverLate` takes the reply of a cancelled call for a result still waited for | `SettlesAtMostOnce` | the cancel gave one outcome and the reply a second: `settles["c1"] = 2` |
| `call-mut-close-leaks` | `CloseB` keeps `bgrants` | `ClosedClearsTables` | `bgrants = 1` stays in a closed session |
| `call-mut-reuse-id` | `AStart` does not advance `anext` | `NoIdReuse` | `aid = [c1 \|-> 1, c2 \|-> 1]` |
| `call-mut-reuse-id-settles` | The same mutation, looking at what follows | `OwnReply` | `settledBy["c1"] = "c2"` |
| `call-mut-late-reply-keeps-grant` | A discarded late reply does not release the reference it carried | `GrantsConserved` | `aheld = 0`, `aReleased = 0`, `granted = 1`, and at rest 0 ≠ 1 |
| `call-mut-abandon-drops` | `AAbandon` under `"drop"` clears the waiter and the edge without a `cancel` | `AbandonKeepsSession` | the reply becomes an unknown call: `closeCause = "unknownCall"` |
| `call-mut-abandon-leaks` | The late reply is absorbed but `aedge` is not cleared | `AbandonedCallsClean` | at rest `aedge["c1"] = TRUE` is still there |
| `call-mut-late-frame-acts` | A frame read after the close is applied as if the session were open | `SettlesAtMostOnce` | A starts a call, B answers, A closes; the reader reads that reply, `settles["c1"] = 2` and `aheld = 1` stay in a session with `aclosed = TRUE` |
| `call-mut-no-pin` | `BRun` looks the target up in the export table when the method starts | `PinnedTarget` | A gets a grant from call 2, starts call 1 (`aid = 2`) and releases the grant; when B starts call 1 `bgrants = 0`, so it drops the call with `throw c1` and `bLost = 1` |
| `call-mut-release-drops-all` | `BReadRelease` empties `bgrants` on any release | `PinnedTarget` | after two grants A releases once and the entry goes; the second release is above the grants and the session ends with `closeCause = "badRelease"` |
| `call-mut-arg-leak` | `BReleaseArgs` lets the arguments go without writing `release` | `ArgRefsReleased` | after a queued call is cancelled, at rest `aArgsOut = 1`, `aArgBack = 0`, `aArgGiven = 1`: A's export table keeps a grant nobody will give back |
| `call-mut-limit-ignored` | `RoomToB`/`RoomToA` check no bound | `BacklogBounded` | `Limit = 2` and `aout` holds three frames |
| `call-mut-forget-strict` | The bounded table forgot the entry, the caller still takes its reply for an unknown call | `NoUnknownCall` | the bounded table dropped call 1 (`forgotten = {"c1"}`), B answers it and A ends the session per `SESS-41`: `closeCause = "unknownCall"` |
| `relay-mut-cancel-stops` | `MReadCancel` drops the forwarded request but writes no `cancel` | `CancelReaches` | A cancels call 1, M reads the cancel while forwarding it, `wasOutstanding["c1"] = TRUE` and `reachedC["c1"] = FALSE`, C's call is still `"queued"`, and no frame is left in either session |
| `relay-mut-release-leaks` | `MReadRelease` drops the relay but keeps the import | `RelayReleasesImport` | `aHeld = 0`, `mRelay = 0`, `mImport = 1`, `cGrants = 1`: C's grant stays on a reference nobody holds |

The two reused-number configurations, the two abandonment configurations and the two `PinnedTarget` mutations are two conclusions of one rule each: first the rule itself is broken, then the wrong settlement or the wrong session end that follows.

## The cancelled table: input for #1099

The gap `call-gap-cancel-table` records: a call cancelled while queued is answered by nobody (`SESS-57`), while the caller must keep a `calls.cancelled` entry to absorb a late reply — so every cancellation of a call that never started adds an entry that stays until the session ends. Each of the three candidate rules has a configuration:

| Configuration | Rule | Cancelled table | Cost | Result |
| --- | --- | --- | --- | --- |
| `call-gap-cancel-table` | Today: cleared only by a late reply | grows without bound | memory only | violated `CancelledEntriesDrain` |
| `call-gap-ack` | A cancel acknowledgement frame: the callee answers every cancel with `ack`, the caller clears its entry on it | empty at rest | a new frame; **"never answer after a cancel" must come with it**, or a late reply lands on the cleared entry (the `call-gap-unknown-late` counterexample is that path); needs a version or capability negotiation | ok |
| `call-gap-unknown-late` | The caller clears the entry at once and treats a late reply as an unknown call | empty at rest | one conforming exchange can end the session with `response for unknown call` (`closeCause = "unknownCall"`) | violated `NoUnknownCall` |
| `call-gap-bounded` | A bounded table: at most `TableCap` entries, the oldest dropped; the replies to forgotten calls are discarded | at most `TableCap` | `SESS-41` must be relaxed to "the reply to a forgotten call may be discarded", no longer a protocol error; the implementation needs a watermark or a ring | ok (`CancelledTableBounded`, `NoUnknownCall`) |

`relay-gap-cancel-table` shows the entry exists **once per hop**: one in A's table, and one in the relay's table for the call it forwarded and then cancelled. The rule chosen from `call-gap-ack` or `call-gap-bounded` has to be written for the relay too.

## Backpressure and the host's bound

The two backpressure configurations write everything else the same way: the caller's and the callee's frames go into the writer thread's queue first and the writer puts them on the channel; the only difference is the reader's write-back — `"direct"` writes on the channel and stops reading when it is full, `"queued"` hands it to the writer. So the deadlock can only come from the reader's write-back, not from the caller's or the callee's writes.

| Configuration | Behaviour modelled | Result |
| --- | --- | --- |
| `call-backpressure-before` | Before #1005: the reader's write-back goes on the channel itself and blocks the reader when it is full | TLC finds a deadlock: `ablock = <<"release", 1>>` (A discarding a late reply's reference), `bblock = <<"release", 1>>` (B giving back a dropped queued call's argument reference), `toB = <<"cancel", "c1">>`, `toA = <<"throw", "c2", 2>>`, both channels full and one frame still in a writer queue. Freeing a direction needs the other side's reader to read, and it is stuck in its own write-back |
| `call-backpressure-after` | Today: the reader hands its write-back to the writer thread's queue | No deadlock; `WaitingSettles` holds. #1005 already has this shape, so **no code change is needed** |
| `call-backpressure-limit` | The same pre-#1005 shape with the host's `outbound_limit = 2` | No deadlock: the queue at its bound cannot take more and the session ends with `notReading` (`SESS-75`). This is what a bound does about a peer that stopped reading: end the session, not keep frames for it indefinitely |
| `call-mut-limit-ignored` | The queue ignores the bound | Violated `BacklogBounded`: the bound is the only thing that ends such a session, and without it nothing bounds the queue (`SESS-76`'s "no bound" is that shape) |

## Conclusion

- **A reader's write-back does not deadlock, provided the reader does not write back itself.** Before #1005 (`call-backpressure-before`), three calls, one cancel and capacity 1 per direction reach a deadlock: each reader is stuck on a write-back, both channels are full, and the reads that would free a channel belong to the other stuck reader. Today's shape (`call-backpressure-after`) has no deadlock and every call settles; the implementation already has this shape, so this model does not change code — it records why and keeps the regression configurations. The three write-back points have the same shape: the `release` of a discarded late reply, the `throw` of a refused call, and the `release` of a dropped queued call's arguments.
- **The host's bound turns such a session from "stuck" into "over".** `call-backpressure-limit` ends the session with `notReading` when the peer does not read and the queue is at its bound (`SESS-75`), and `BacklogBounded` keeps the queue inside the bound; `call-mut-limit-ignored` shows that without the bound nothing bounds it. The default `outbound_limit` is `None` (`SESS-76`): a host that sets no bound carries that risk itself.
- **Both reply-after-cancel implementations are allowed, and neither settles twice nor leaks a reference.** In `call-cancel-reply` the late reply is discarded and its reference released; in `call-cancel-no-reply` no reply and no grant are produced. Both pass `SettlesAtMostOnce`, `LateReplyDropped`, `OwnReply`, `NoOverRelease`, `GrantsConserved` and liveness. `SESS-61` stays "not guaranteed": both replies are allowed and the caller must take either; cleaning up the cancelled table is #1099's. This directory does not change the protocol text.
- **A synchronous call abandoned locally leaves no edge nothing can clean, provided the caller keeps the call until the reply.** Today's `request_sync` has no drop guard, so an abandoned synchronous call sends no `cancel`; the waiter stays in `calls.waiting` and the reader's `reply_target` takes it out and calls `waits::returned` when the reply is read. Throwing the waiter and the edge away without a `cancel` makes the reply `response for unknown call` and ends the session, so the rule is: **keep it until the reply, or send `cancel`**.
- **A reference sent as an argument is not lost, provided the call writes it back when it lets the arguments go.** In `call-arg-release` the reader drops a queued call on a cancel and the arguments' `Import` writes `release`, so A's export table is empty at rest; `call-mut-arg-leak` shows what is left when nothing is written back (`aArgsOut = 1` until the session ends). That write-back is the same path as the `throw` and the discarded reply's `release`, so the backpressure conclusion covers it too.
- **`PinnedTarget` holds, provided an admitted call holds its target instead of looking it up each time.** The call carries the object in `Accepted`, so the caller's `release` may drop the export entry without affecting it; grants are released by count, and a release of an older grant leaves a newer one in place. `call-mut-no-pin` shows that looking the target up when the method starts lets one release destroy an admitted call, and `call-mut-release-drops-all` that dropping the entry on any release ends the session on the second release (`release of unknown reference` / `invalid reference release count`).
- **A frame read after the close changes nothing, provided the reader looks at `closed` first.** `call-after-close` passes `LateFramesInert`: the frames the close left on the channel are read and refused, no call is settled after the close that failed it, and no reference is taken into a closed session. `call-mut-late-frame-acts` shows that without that check a call the close already failed is settled a second time and a reference stays in a closed session — which is what `SESS-51`'s "references go with the session" rests on.
- **A cancel and a release travel both hops of a relay, provided the relay sends `cancel` for the forwarded request it drops and gives the import back when it drops the relay.** `relay-healthy` and `relay-no-reply` pass `CancelReaches`, `RelayReleasesImport` and `GrantsConserved`; `relay-mut-cancel-stops` shows a relay that drops its own wait without telling C stops the cancel at the second hop; `relay-mut-release-leaks` shows that a relay that keeps the import leaves C's grant on a reference nobody holds. The counts travel the chain without a new frame kind: the release carries the import's count.
- **The input for #1099 is the cost of three rules, not one better rule.** Today's table grows without bound (`call-gap-cancel-table`); a cancel acknowledgement empties it, but only together with "never answer after a cancel", or a late reply ends the session (`call-gap-unknown-late` is that path); a bounded table caps it, at the price of relaxing `SESS-41` to "the reply to a forgotten call may be discarded". `relay-gap-cancel-table` shows the entry exists once per hop, so the rule has to cover the relay too. This directory only supplies the evidence; it changes neither the protocol text nor the code.
- With the rest of the rules of §5–§7 — a number only grows and is never reused, a call settles once, a cancel drops the call and not the session, a late reply is discarded, a close fails the unfinished calls and clears the reference tables — the following still holds in the exhausted range: no call is settled twice; a cancel never closes the session; the late reply of a cancelled call does not change its outcome; a close on one side ends the other side too, and neither keeps an unfinished call or a reference; every grant made before the close is given back once. The state count of `call-healthy` (1,155) is the same as before the extension, so with the new switches off the model behaves as it did.

## Not covered

- **A close in the relay model.** `CallRelay.tla` has none. A relay whose own session closes does **not** pass the release on to C: `Connection::close` clears `imports` before dropping `exports` (`rpc.rs:1667-1682`), so the `release` the relay writes when it is finally dropped is swallowed by `write_bytes_locked`'s `closed` check, and C keeps the grant until its own session ends (`SESS-51` closes it). This is read off the code; there is no model of it.
- **The other side still reading frames after a close.** The model makes "a frame read after the close" one rule: with `AfterClose = "read"` either side reads and refuses what the close left in either direction. In the implementation the peer may still execute an `invoke` it had already read before its own session ends (its `receive` looks only at its own `closed`) and its reply lands on a closed channel; that interleaving is not in the model, and `LateFramesInert` does not claim it.
- **`outbound_limit` in bytes.** The model counts frames (`Limit` is a number of frames); the implementation counts bytes. They agree on the shape ("a non-empty queue that cannot take one more frame ends the session").
- **The relay's chain, `rebase`, `path`, the wait graph and synchronous forwarding.** The relay model has one call, one object reference and one two-hop chain; the `path` rewrite, the synchronous forwarding thread and the cycle check are in `cross-runtime-sync`.
- **Reference identity through a relay and `home: true`.** The same import relayed twice sharing one relay (M1-7.1-3), a relay coming home (M1-7.1-4) and a relay of a relay (M1-7.1-5) are not modelled.
- **Value encoding, error graphs, `bad-value`/`bad-frame`, the 16 MiB and nesting limits**, the wait-graph cycle check, the handshake and authentication, real scheduling, memory order and timing.
- This directory is one block of the #613 call lifecycle and is not evidence for the whole call protocol.

## How to run it

Java 11 or newer and `tla2tools.jar` ([v1.7.4](https://github.com/tlaplus/tlaplus/releases/tag/v1.7.4), sha256 `936a262061c914694dfd669a543be24573c45d5aa0ff20a8b96b23d01e050e88`). Do not commit the jar.

```sh
curl -sSLo /tmp/tla2tools.jar https://github.com/tlaplus/tlaplus/releases/download/v1.7.4/tla2tools.jar
TLA2TOOLS=/tmp/tla2tools.jar specs/call-lifecycle/run.sh                    # every configuration
TLA2TOOLS=/tmp/tla2tools.jar specs/call-lifecycle/run.sh call-healthy       # one configuration
TLC_WORKERS=4 TLA2TOOLS=/tmp/tla2tools.jar specs/call-lifecycle/run.sh      # limit TLC's workers
```

`run.sh` picks the module by name: `call-*.cfg` runs `CallLifecycle.tla`, `relay-*.cfg` runs `CallRelay.tla`. It compares each configuration's outcome with `expected.txt` and exits 0 when every one matches; an outcome is `ok`, `deadlock` or `violated:<name>` (a violated temporal property is reported as `violated:liveness`). The logs and counterexamples are written under `TLC_OUT` (a temporary directory by default). CI's `specs` job matrix includes `call-lifecycle` (see [CI](../../docs/ci.en.md) §4).

## Maintenance

Change both modules and `expected.txt` together with session protocol §5, §6, §7, §10, or with `Calls::admit`, `reply_target`, the handling of `Frame::Cancel`/`Frame::Release`/`Frame::Return`, `Connection::cancel`, `Connection::close`, `Connection::write`/`Outbound::push`/`write_frames` (the reader write-back, the writer thread and `outbound_limit`), `request_sync` and `CancelOnDrop` (local abandonment), `Import`'s `Drop` (argument references and the relay's release), or `relay`/`encode_import` in `rpc/relay.rs` (multi-hop forwarding) in `crates/rutis-bridge/src/session/rpc.rs`, or with a runtime's (`node/rutis-runtime/src/session.mjs` and the rest) cancel, reply and reference-release behaviour (standard Q11.2.3).
