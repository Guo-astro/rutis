# Events protocol: host rules, listener table, emit and node links (TLA+)

[中文](README.md)

These models are issue #613's `specs/events/`: the [events protocol spec](../../docs/design-events-protocol-2026-10-18.en.md) §4 (registration), §5 (dispatch), §6 (filters), §7 (lifecycle) and §9 (node links) written as finite models whose interleavings TLC explores. Background: [unified events](../../docs/design-events-2026-10-18.en.md) and [formal verification redesign](../../docs/design-formal-verification-2026-11-08.en.md) §3.2.

One directory, four modules:

| Module | Contents | Participants and scale | Configurations |
| --- | --- | --- | --- |
| `EventsHost.tla` | Definitions only, no state: name/prefix matching, registration order, elsewhere, filters, the start rules of the five modes, `export` / `import` and narrowing, the epoch of an announced id and what a newer offer keeps | none | `host-defs` evaluates them with `EventsHostCheck.tla` on the examples of the spec |
| `EventsTable.tla` | registration, withdrawal, elsewhere, the fast path and the via-host path, a `rows.load` reply, a row ending, unloading and loading again, a session ending and starting again, the host's prefix entry, cross-process call causality | host + 1–2 runtimes; `table-multi` is 2 runtimes × 2 rows in one configuration; the healthy ones issue at most 1 dispatch | 19 |
| `EventsEmit.tla` | the host's per-session emit queue whose items remember their row, a call held in a synchronous wait, the reply read in place, a late filter, a row unloading, three emits of two rows, `R2a`/`R2b`, a call started on a later turn (`python-deferred`), a host that replies before the calls started (`before-future-reply`), deadlock checking | host + `R1` (rows A/B) + `R2`; three emits | 10 |
| `EventsLink.tla` | `link.offers` snapshots, only the highest version, `<epoch>.<n>` announcements, an announcement before its offer, the three accepting steps, independent interleavings of `events.off` and `link.offers`, the current bare-id rule, the peer not forwarding an entry it imported from the next hop | host + one peer (plus one entry it imported from the node behind it); two of its own entries, two families, two offers | 7 |

`EventsHost.tla` is not a runnable spec: the three stateful modules `EXTENDS` it and each configuration uses a part of it. `host-defs` checks by `ASSUME`: the examples of §4, §5, §6 and §9 must evaluate to the answers the spec writes, and then the run stays in one state. It does not check those definitions for arbitrary inputs, only that they agree with the examples.

## Design and code it corresponds to

| Model part | Design | Implementation |
| --- | --- | --- |
| host table, positions, `prepend`, prefixes, liveness | §4.2, §4.3, §5.1 | `crates/rutis/src/bus/table.rs` (`select`, `insert`, `Entry::start`) |
| snapshot, filters, the five modes, the emit tail and backlog | §5.1, §5.4, §5.5, §6 | `crates/rutis/src/bus.rs` (`emit_erased`, `snapshot`, `start`), `crates/rutis/src/bus/run.rs` |
| a session's `events.on` / `events.off` / `events.dispatch`, row withdrawal, the `events.on` reply before elsewhere, the emit queue | §4.4, §4.5, §5.3, §7.1 | `crates/rutis-bridge/src/events/hub.rs` (`SessionEvents::on`, `off`, `dispatch`, `run_emit`, `next_emit`, `row_unloading`, `row_ended`, `let_go_of_its_emits`) |
| frame order (`events.*`, `rows.ended`) | §4.3, §7.1 | `crates/rutis-bridge/src/session/rpc.rs` |
| cross-process causality (§4.4 step 3) | §4.4 | the registration path (`SessionEvents::on` in `crates/rutis-bridge/src/events/hub.rs`) and the order of the reply and elsewhere; the call frame is relayed by the session |
| a row ending, unloading and loading again | §7.1 | `crates/rutis-bridge/src/events/hub.rs` `row_unloading` / `row_ended`; a re-registration enters with a new id, the model distinguishes the two registrations with `MaxLoads` |
| node link, accepting, withdrawal, not forwarding | §9 | `crates/rutis-bridge/src/events/link.rs` (`accept`, `announced_under`, `peer_offer`, `offer_now`, `older_than`) |
| versions and `since` of `link.offers` | §9 | `crates/rutis-bridge/src/peer.rs` (`Offers`, `next_offers`, `send_if_modified`) |

## Simplified

Shared by every module:

- One name (`EventsTable`, `EventsEmit`) or two entries (`EventsLink`); `EventsTable`'s host prefix entry has the fixed `"session/"` prefix and every other entry matches by exact name. No multiple names or further prefixes.
- No scope (it is only a descriptor), values, error contents, `AggregateError` aggregation, cancellation, timeouts, or the rule that `next` may be called once.
- Frames in one direction arrive in order and may be delayed arbitrarily; some modules reduce "order" to one FIFO per direction.
- Time is a ghost set ("what has happened"), not a clock: a state does not become different because something arrived later.

`EventsTable.tla`:

- `table-multi` puts 2 runtimes × 2 rows in one configuration, but allows one dispatch and a queue depth of 1; `table-order` is still 1 runtime × 2 rows for `OneOrder`.
- A cross-process call is one `call` frame: the caller sends it after its `events.on` reply and the frame carries the entries the caller has seen a reply for; the receiver may then dispatch, and `CausallyAfterCallSees` requires those entries. No real service call chain, `path` or synchronous wait is built.
- The `events.on` reply itself is only a ghost (`repliedTo`); a `rows.load` reply still counts registrations issued and accepted, without the reply's contents.
- Prefix entries only come from the host's own Rust listener; runtimes never register one. `HostOn` only accepts the entry that is the row's current registration, so a late `events.on` for an unloaded/reloaded row does not enter the table (§7.1).
- One row of one runtime registers at most `MaxLoads` times per session; `table-reload` uses `MaxLoads=2` for "ended/unloaded, then loaded again". A reload needs the old entry to have left the host table (the old `rows.ended` or unload was processed), so two registrations of the same row are never in the table together. The in-place reordering of `rows.update` is not built.
- A session ends first (`EndBeforeRestart`): its entries leave the table and its frames are dropped; a dispatch that already took a snapshot checks the entry is still in the table in `HostStart`, so it starts no call for the old session. A restart still waits until every snapshot has finished.
- `events.off` and `rows.ended` only say which entry; unknown-id replies, errors and batch withdrawal are not built.
- Filters are not modelled (no `filter` in the configurations); `emit` appears only as "calls start in order", without `serial` / `parallel` / `waterfall`.

`EventsEmit.tla`:

- The scenario: `R1`'s row A emits `E1` through the host while it is in a synchronous wait, so `E1`'s call at `L1` is held; `R1` reads the reply in place; the wait chain emits `E2`, and row B emits `E3`. The host has one queue per session and takes one item at a time; an item remembers its row, so unloading row A drops only A's items.
- All three filters exist only on the via-host path; filters on the fast path, several filters per emit, and the other modes are not built.
- `R2a` / `R2b` differ by the set of calls still running at `L2`: `R2a` allows overlap, `R2b` waits until the call at the same listener ends. The two configurations have different state counts, so the difference is built; ending a call is an arbitrary step, not real scheduling.
- Unloading a row drops its queued emits, lets go of the emit waiting for its filter, and drops calls already held; after A unloads, `L1` starts no call. The `unloadQueue` mutant keeps all of them (#536). The model drops every item of the row at the unload, so `UnloadedRowEmitsNothing` speaks of the items queued before it; the emissions an unloading row hands over are delivered even once the row is off the session (event protocol §7.1 step 5, #1327), and the model does not build that.
- Deadlock check: the healthy configurations use `CHECK_DEADLOCK TRUE`, and `Done` stutters only when nothing is queued, no filter/call is running, and nothing is held. A `never` filter is not in those configurations — by the spec it hangs forever and is not a deadlock to check.
- No panic, task scheduling, real Promise, Node/Bun/Python frame-pump differences, or emit failure reporting.
- `python-deferred` (`Defer = "nextRound"`): Python hands a received invoke frame (including `events.call`) to `loop.call_soon` (`python/rutis/rutis/peer.py:1886`) and runs it in place only while a synchronous wait is in progress (`_run`, lines 1892–1896 of the same file); `call_soon` is first in, first out. In the model an `R2` call enters `deferred` and starts only from its head, so a call starts after its frame was read and the order at one listener does not change. The `emit-python-deferred` configuration passes: the delay only pushes `StartCall` later, and the model already allows it to be arbitrarily late, so no property changes. What it does not build is the split between "the host counts this call as started" and "the listener body begins" (today `StartCall` is both) and a property for "a frame is handled in place, then the next one is read" (EVP-4.1-2), so it cannot check that rule. Python has no fast path either (`send` in `python/rutis/rutis/runner.py` always emits `events.dispatch`, and `python/rutis/rutis/` has no code handling `events.elsewhere`).
- `before-future-reply` (`Mutation = "futureReply"`): the host's half before c2c1b7e8 (#513): the host writes the reply to an emit that is still queued, its calls not started, and R1 reads it in place, so the name counts as done for the emitter. The mutation builds that in two steps: `ReplyEarly` records the reply as read while `phase` is still `queued`/`filtering`/`starting`; `PendingAt` no longer counts such an emit as "an earlier call has not started", so a later emit of the name takes the fast path without queueing behind it. `emit-mut-future-reply` violates `EmitOrder`.

`EventsLink.tla`:

- One peer, its two own entries, two families, two offers; `events.on` / `events.off` share one ordered `wire`, `link.offers` snapshots are a second column that may come at any position (`peer.rs` sends `announce` on a task). That is where #593's timing comes from.
- The offer boundary itself withdraws: when the peer offers `events` again it does not send an `events.off` per old entry, the old entries go by epoch (`link.rs`'s `offered()`); a single entry is withdrawn with `events.off`, and either message may interleave with a snapshot.
- `PeerIds = "bare"` is the **current rule** of another implementation: ids carry no epoch and the receiver replaces by id only. `link-bare-id` is a configuration of the healthy rule, not a mutation; it expects `CurrentOffer` to be violated because that rule cannot promise that an entry not in a newer offer disappears (EVP-9-17). The separate `bareIds` mutation no longer exists.
- Multi-hop is built only as "the peer has one entry imported from the next hop": `link-multihop` lets it exist without announcing it to the host, and `NoForward` holds; the `forward` mutation announces it. No full three-node message chain, multiple peers or merged forwarding table.
- `export` / `import` narrowing lives in `EventsHost.tla`'s definitions and is checked by `host-defs`; the stateful `EventsLink` has one peer direction, without bidirectional sharing, service items or prefix items as boundaries.
- The three accepting steps are split only in the `acceptUnlocked` mutation; the base model checks the epoch, registers and inserts in one step.
- Deadlock check: the healthy, bare-id and three mutation configurations use `CHECK_DEADLOCK TRUE`; `Done` stutters only when the link is quiet and nothing is left to send. The `forward` mutation turns deadlock checking off to find its counterexample sooner.

## Properties

| Name | Module | What it says | Guarantee |
| --- | --- | --- | --- |
| `RegisteredThenSeen` | Table | a registration its issuer knew when issuing, not withdrawn anywhere by the snapshot, is in the snapshot (§4.4 steps 1 and 3) | `events/presence`, `events/hub-dispatch` |
| `CausallyAfterCallSees` | Table | when a dispatch is caused by a cross-process call, every registration whose reply the caller had seen before the call is in the snapshot (§4.4 step 3) | added with the model; `events/hub-dispatch` |
| `WithdrawnStaysOut` | Table | an entry its issuer knew withdrawn at issue starts no call of that dispatch (§4.4 step 4, §7.1) | `events/hub-lifecycle`, `events/lifecycle` |
| `OneOrder` | Table | two entries both dispatches start keep the same order (§4.3, §5.4) | `events/order`, `events/one-bus` |
| `OneCallPerEntry` | Table | at most one call per (dispatch, entry) (half of `SettlesOnce`) | `events/hub-lifecycle` |
| `RunningRowHeard` | Table | a row is Running only after the entries its apply registered were accepted (§4.4 step 2) | no table row; added with the model |
| `NoDispatchBeforeElsewhere` | Table | a new session does not dispatch before it applied the full elsewhere set (§4.5) | — |
| `PrefixEntrySeen` | Table | a matching prefix entry in the host's table at snapshot time is in the host's snapshot (§4.3, §5.1) | `events/presence` |
| `SeesAll` | Table | every dispatch sees every entry registered before it and not withdrawn at issue; the protocol does **not** promise this, the configuration expects it violated | `events/one-bus` (counterexample below) |
| `EmitOrder` | Emit | for one emitter and one listener, `emit` calls start in the order the emits were issued (§5.5) | `events/emit`, `core/D31` |
| `FastPathByFrame` | Emit | once the reply of a via-host emit was read, a later emit of the same name is not left on the via-host path (§5.5 last rule) | `events/one-bus` (`emit/order-across-fast-path`) |
| `UnloadedRowEmitsNothing` | Emit | an emit of a row that unloaded starts no call, even after its filter answers (§5.5, §7.1) | no table row; added with the model |
| `NoCallToUnloadedListener` | Emit | a listener owned by a row that unloaded starts no call after the unload (§7.1) | added with the model |
| `CurrentOffer` | Link | with the link quiet and nothing left to send, the peer's entries in the host's table are exactly those its current offer announced and still has (§9) | `events/node-links` |
| `NoEcho` | Link | the host does not announce an entry it took from the peer back to the peer (one hop) | `events/no-multi-hop` |
| `NoForward` | Link | the peer does not announce an entry it imported from the next hop to the host (multi-hop) | `events/no-multi-hop` |

## Results

TLC 2.19 (tla2tools v1.7.4), Temurin 21.0.12, 32 cores, `-workers 4`, 2026-11-10; `table-mut-on-late`, `table-mut-dup-local`, `emit-python-deferred` and `emit-mut-future-reply` were measured on the same machine with the same flags on 2026-11-16. `passes` means the run completed and every listed property holds; a violated configuration stops at the first counterexample, so its state count depends on the search order. The whole directory takes about 2 minutes. Every configuration below was run on this machine; what was not run are larger parameter combinations (for example `table-multi` with `MaxDispatch>1` or `MaxLoads>2`, or `EventsLink` with `MaxEpoch>2`).

| Configuration | Contents | Result | Distinct states | Time |
| --- | --- | --- | --- | --- |
| `host-defs` | the definitions agree with the examples of the spec | pass | 1 | <1 s |
| `table-healthy` | 2 runtimes × 1 row, the host's own listener, withdrawal, row unload, at most 1 dispatch | pass | 966,166 | 17 s |
| `table-multi` | 2 runtimes × 2 rows in one configuration, 1 dispatch, queue depth 1 | pass | 960,076 | 13 s |
| `table-causal` | a cross-process call, the receiver dispatches after the reply; causality | pass | 150,908 | 6 s |
| `table-prefix` | host prefix entry, `SnapshotOf` matching | pass | 87,665 | 3 s |
| `table-reload` | 1 runtime × 2 rows, a row may end, unload and load again (two registrations) | pass | 81,054 | 2 s |
| `table-end-session` | a session ends before it starts again; snapshots still in flight at the end | pass | 566,207 | 12 s |
| `table-sees-all` | same as `table-healthy`, checking `SeesAll` | violates `SeesAll` | 134 (stops at the counterexample) | <1 s |
| `table-ended` | the runtime reports `rows.ended` | pass | 199,102 | 5 s |
| `table-restart` | the session ends, the new one applies the full elsewhere set first | pass | 673,801 | 8 s |
| `table-order` | 1 runtime × 2 rows, `prepend`, two dispatches (fast path + via host) | pass | 22,081 | 1 s |
| `table-mut-local-order` | the runtime's local order ignores `prepend` | violates `OneOrder` | 24,670 | 1 s |
| `table-mut-out-of-order` | the host does not process a session's frames in order (before #587) | violates `WithdrawnStaysOut` | 4,537 | 1 s |
| `table-mut-stale-fast` | the fast path takes its snapshot and liveness from the host's table, not its own registrations | violates `WithdrawnStaysOut` | 275 | <1 s |
| `table-mut-load-reply` | `rows.load` is answered before the registration is accepted | violates `RunningRowHeard` | 51 | <1 s |
| `table-mut-fresh` | a new session dispatches without the full elsewhere set | violates `NoDispatchBeforeElsewhere` | 2 | <1 s |
| `table-mut-causal` | the host replies `events.on` before queueing elsewhere for the other sessions | violates `CausallyAfterCallSees` | 2,476 | 1 s |
| `table-mut-prefix` | the host snapshot matches exact names only, dropping the prefix entry | violates `PrefixEntrySeen` | 87 | <1 s |
| `table-mut-on-late` | the host handles `events.dispatch` on its own, taking it before the earlier `events.on` of the same session | violates `RegisteredThenSeen` | 95 (stops at the counterexample) | 1 s |
| `table-mut-dup-local` | the runtime's own order list keeps both registrations of one listener | violates `OneCallPerEntry` | 89 (stops at the counterexample) | 1 s |
| `emit-r2a` | held call, reply read in place, late filter; `R2` rule a; deadlock check | pass | 34,857 | 2 s |
| `emit-r2b` | same, `R2` rule b; deadlock check | pass | 24,893 | 2 s |
| `emit-multi` | two rows, three emits, the host's per-session queue, either row may unload; rule a | pass | 127,576 | 4 s |
| `emit-multi-r2b` | same as `emit-multi`, rule b | pass | 91,688 | 3 s |
| `emit-python-deferred` | same as `emit-multi`, an `R2` call starts on a later turn (`Defer = "nextRound"`, FIFO); deadlock check | pass | 141,978 | 2 s |
| `emit-mut-held` | a later fast-path emit passes the held call (#557 before its fix) | violates `EmitOrder` | 701 | 1 s |
| `emit-mut-reply-async` | the reply is recorded one step after it was read (#513 before its fix) | violates `FastPathByFrame` | 318 | 1 s |
| `emit-mut-future-reply` | the host replies to an emit still queued, its calls not started (the host's half before c2c1b7e8, #513) | violates `EmitOrder` | 526 (stops at the counterexample) | 1 s |
| `emit-mut-unload` | unloading does not drop queued emits (#536 before its fix) | violates `UnloadedRowEmitsNothing` | 428 | 1 s |
| `emit-mut-dead-listener` | a listener of an unloaded row is still called | violates `NoCallToUnloadedListener` | 159 | 1 s |
| `link-healthy` | two offers, an announcement before its offer, another family's snapshot; deadlock check | pass | 293,064 | 5 s |
| `link-multihop` | the peer has an entry from the next hop but does not forward it | pass | 586,035 | 8 s |
| `link-bare-id` | `PeerIds = "bare"`: the current rule replaces by id only; `CurrentOffer` is not promised | violates `CurrentOffer` (expected) | 3,263 | 1 s |
| `link-mut-accept-unlocked` | the epoch check and the insert are separated (the old `link.rs` `accept`; the current code rechecks under the same lock) | violates `CurrentOffer` | 6,748 | 2 s |
| `link-mut-offer-race` | a snapshot without `events` withdraws everything (before #593) | violates `CurrentOffer` | 11,923 | 2 s |
| `link-mut-echo` | the host announces an imported entry back | violates `NoEcho` | 677 | 1 s |
| `link-mut-forward` | the peer forwards an entry from the next hop | violates `NoForward` | 152 | 1 s |

## Counterexamples

Every checked property has a deliberate error (`violated:<name>` in `expected.txt`), including `RegisteredThenSeen` in `table-mut-on-late` and `OneCallPerEntry` in `table-mut-dup-local`. Besides the mutations, `table-sees-all` violates a property the protocol explicitly does not promise, and `link-bare-id` violates a consequence of the current bare-id rule:

- **`table-sees-all`**: `r1` registered a listener and its `events.on` was accepted into the host's table; `r2`'s elsewhere is not updated yet, so it takes the fast path and sees only its own entry. "Every dispatch sees every entry registered before it" does not hold. This is the example at the start of issue #613: `SeesAll` is not a promise; `RegisteredThenSeen` and `WithdrawnStaysOut` are.
- **`table-mut-out-of-order`**: `r1` sends `events.on`, then `off`, then `events.dispatch`; the host takes the `events.dispatch` first, its snapshot has the entry, and then starts a call for it before processing `off`. `r1` already knew it had withdrawn it at issue, so `WithdrawnStaysOut` does not hold. This is the frame-order problem before #587.
- **`table-mut-stale-fast`**: after `r1` sends `off` and before the host processes it, the fast path takes its snapshot from the host's accepted table and asks the same table about liveness, so it starts a call for an entry it had withdrawn.
- **`table-mut-local-order`**: the local order ignores `prepend`: the fast path follows arrival order and the via-host dispatch follows positions, so two dispatches order the same two entries differently and `OneOrder` does not hold.
- **`table-mut-load-reply`**: `rows.load` is answered before `events.on` is accepted; `RunningRowHeard` does not hold.
- **`table-mut-fresh`**: a new session dispatches before it receives the full elsewhere set; `NoDispatchBeforeElsewhere` does not hold.
- **`table-mut-causal`**: `r1`'s listener is accepted, the host replies `events.on` before queueing elsewhere for `r2`; `r1` then calls a service in `r2`, `r2` dispatches after reading the call, and its fast path still has the old elsewhere, so it misses `r1`'s entry. `CausallyAfterCallSees` does not hold. In the base model the host queues elsewhere first and then replies, so this path does not appear in `table-causal`.
- **`table-mut-prefix`**: the host snapshot matches exact names only; the table has a prefix entry matching `session/created` but the snapshot does not, so `PrefixEntrySeen` does not hold. The base model keeps the prefix entry through `EntryMatches`.
- **`table-mut-on-late`**: the host treats `events.dispatch` as its own task and does not wait for the frames the same session sent before it. `r1` sends `events.on` (`<<r1,a,1,1>>`) and then `events.dispatch`; the host takes the latter first and snapshots its table, which does not hold that registration yet, only the host's own `<<host,host,0,0>>`. `r1` knew its registration when it issued the dispatch, and nothing withdrew it by the snapshot (`Withdraw` and `Unload` are off, so `everOff` stays empty), so `RegisteredThenSeen` does not hold. §4.4 item 1 has the snapshot hold what the issuer knew, and EVP-4.1-3 has the host handle a session's event operations in frame order; this mutation lets `events.dispatch` overtake the frame before it.
- **`table-mut-dup-local`**: one row registers the same listener twice in one session, and the runtime's order list (`local`) keeps two copies of `<<r1,a,1,1>>` (`Register` no longer checks that the entry is already in the list); the fast path starts a call per copy, so `OneCallPerEntry` (`NoDup(order[d])`) does not hold. In the counterexample `tbl` is empty: the first `events.on` has not reached the host yet and the runtime has already started two calls. The host side never keeps one entry twice (`HostOn`'s `kept` requires the entry not to be in the table, and a duplicate id is refused, EVP-4.2-1), so the mutation drops the "one registration per listener" premise and checks the property on the dispatch side itself.
- **`emit-mut-held`**: `E1`'s call at `L1` is held in a wait; after `R1` reads the reply, a fast-path `E2` starts its `L1` call at once; `E1`'s call runs after the wait ends. The later emit starts first at `L1`, so `EmitOrder` does not hold (before #557).
- **`emit-mut-reply-async`**: `R1` has read `E1`'s reply frame but records it one step later; the wait chain emits `E2` in between and the runtime still sends it to the host. `FastPathByFrame` does not hold (before #513).
- **`emit-mut-future-reply`**: row A's `E1` went through the host and is still in its queue, none of its calls started; the host writes the reply anyway and `R1` reads it in place (`read[E1]`). Row B's `E3` therefore takes the fast path and starts its `L1` call at once; only then does the host take `E1`, and `E1`'s `L1` call starts after `E3`'s. The later emit starts first at the same listener, so `EmitOrder` does not hold (the host's half before #513).
- **`emit-mut-unload`**: unloading does not drop queued emits; the filter answers later and `E1`/`E2` still start calls, so `UnloadedRowEmitsNothing` does not hold (before #536).
- **`emit-mut-dead-listener`**: row A has unloaded; row B's emit still sends a call to A's old `L1`, so `NoCallToUnloadedListener` does not hold.
- **`link-bare-id`**: `PeerIds = "bare"` is the peer implementation's current rule: an announced id carries no epoch and the host replaces by id only. After the peer re-offers, an entry announced under an older offer that the new offer no longer has stays in the table, so `CurrentOffer` does not hold (EVP-9-17). This is not an implementation bug: the rule does not promise `CurrentOffer`; the epoch configurations and the mutations are checked separately.
- **`link-mut-accept-unlocked`**: the epoch check for `<<1,2>>` finishes, and before the insert the host processes a higher-version snapshot with `events: 2` (withdrawing the old offer); the insert then puts `<<1,2>>` back. With the link quiet, the table has an entry the peer's current offer did not announce, so `CurrentOffer` does not hold. The old `link.rs` `accept()` did exactly this: the epoch check under the lock, the registration outside it, the insert under another lock. The current code rechecks under the same lock with the `gone` boundary and has the `an_entry_of_an_offer_gone_while_it_registered_is_not_taken` regression test; the mutation keeps the old behaviour to guard that window.
- **`link-mut-offer-race`**: a snapshot without `events` withdraws all entries; an announcement that arrived before its offer is taken away by that withdrawal even though its offer was not listed yet (before #593).
- **`link-mut-echo`**: the host announces an entry it took from the peer back to the peer, so `NoEcho` does not hold.
- **`link-mut-forward`**: the peer announces an entry it imported from the next hop to the host, so `NoForward` does not hold; in the healthy configuration the peer only holds it without announcing it.

## Conclusion

- Against the current spec, `EventsHost.tla`'s definitions agree with its examples; `EventsTable.tla`'s safety properties hold in the exhausted range: register then emit in one process (both paths), a cross-process call's receiver sees the registrations whose reply the caller had seen, a withdrawn entry starts no call, two entries keep the same order everywhere, a `rows.load` reply comes after the registrations were accepted, a new session applies the full elsewhere set first, and the host snapshot keeps a matching prefix entry.
- `table-multi` runs 2 runtimes × 2 rows in one configuration; `table-reload` covers a row ending, unloading and loading again (two registrations); `table-end-session` covers a session ending before it starts again with snapshots still in flight. A late old `events.on` after a row unload is kept out by "only the row's current registration is accepted".
- `EventsEmit.tla`'s two order properties, the unload property and the listener property hold in its scenario; the host's per-session queue drops an unloaded row's items and keeps the other row's; `R2a` and `R2b` have different state counts, so overlapping execution is really modelled; all five mutants have counterexamples. `emit-python-deferred` starts an `R2` call on a later turn (FIFO) and still passes everything: the delay only pushes the start, and the model already allows any lateness, so for the current implementation this doubt (EVP-5.5-9) does not hold in the model; the split it does not build is under "Simplified". `emit-mut-future-reply` finds an `EmitOrder` counterexample with a host that replies before the calls started, so the host half of c2c1b7e8 is not empty.
- `EventsLink.tla`: keeping only the highest snapshot version, withdrawing an older offer's entries by epoch, keeping an announcement that arrived before its offer, and interleaving a single `events.off` with snapshots all hold under the current rules; a bare id (EVP-9-17) is the current rule and has one expected counterexample; the pre-fix `accept()` window, `offerRace`, `echo` and `forward` each have one; `NoForward` covers "the peer does not forward an entry from the next hop".
- What the models do not prove (issue #613's scope): scope, values, errors, cancellation, the in-place reordering of `rows.update`, filters on the fast path, the full semantics of the five modes (only `emit`), real task scheduling and frame-pump differences, a full multi-hop message chain, multiple peers, bidirectional narrowing of `export` / `import` in the stateful model, shared service items, the split in `python-deferred` between "the host counts this call as started" and "the listener body begins" (EVP-4.1-2's "handled in place, then the next frame is read" is not built either), `before-future-reply` covers the host half only, and liveness/deadlock of the implementation (this directory's `CHECK_DEADLOCK TRUE` only answers "this finite protocol state machine has no unintended terminal state in these configurations", not that the implementation cannot deadlock). This directory is not the whole of #613's evidence.

## How to run

Needs Java 11+ and `tla2tools.jar` ([v1.7.4](https://github.com/tlaplus/tlaplus/releases/tag/v1.7.4), sha256 `936a262061c914694dfd669a543be24573c45d5aa0ff20a8b96b23d01e050e88`). Do not commit the jar.

```sh
curl -sSLo /tmp/tla2tools.jar https://github.com/tlaplus/tlaplus/releases/download/v1.7.4/tla2tools.jar
TLA2TOOLS=/tmp/tla2tools.jar specs/events/run.sh                 # all configurations
TLA2TOOLS=/tmp/tla2tools.jar specs/events/run.sh table-healthy    # one configuration
TLC_WORKERS=2 TLA2TOOLS=/tmp/tla2tools.jar specs/events/run.sh    # limit workers
```

`run.sh` picks the module by the configuration's prefix (`host-defs` → `EventsHostCheck.tla`, `table-*` → `EventsTable.tla`, `emit-*` → `EventsEmit.tla`, `link-*` → `EventsLink.tla`), compares each outcome with `expected.txt`, and exits 0 when all agree; logs and counterexamples go to `TLC_OUT` (a temporary directory by default). CI's `specs` job matrix already has `events` ([CI](../../docs/ci.en.md) §4).

## Maintenance

When §4, §5, §6, §7 or §9 of the protocol spec changes, or `bus/table.rs`'s `select` / `Entry::start`, `bus.rs`'s `emit_erased` / snapshot, `hub.rs`'s `events.on` reply and elsewhere queue / row withdrawal / emit queue, `link.rs`'s `accept` / announcement withdrawal / not forwarding, or `peer.rs`'s `Offers` changes, update this directory and `expected.txt` (quality standard Q11.2.3). A new constant (`MaxLoads`, `HostPrefix`, `Reload`, `EndBeforeRestart`, `AllowCrossCall`, `PeerIds`, `PeerImports`, `Defer`) must be added to every affected `.cfg`. After a model change, rerun every configuration: mutations must still find their counterexample, healthy ones must still pass, and a `CHECK_DEADLOCK TRUE` configuration must confirm `Done` only stutters when there is really no work left.
