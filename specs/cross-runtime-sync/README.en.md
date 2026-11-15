# Cross-runtime synchronous calls and the deadlock policy (TLA+)

[中文](README.md)

TLC explores every interleaving of language runtimes calling each other synchronously through the Rust host, to answer the question left open in §9 of the multi-language design: which of the two candidate policies removes the deadlocks. Issue #208; quality standard Q11.2, Q6.2.8, Q6.8.3; status §7.1 (risks P10, C6).

**Reconciliation (2026-11-09, #614).** The wait graph is now the one `crates/rutis-bridge/src/session/rpc/waits.rs` has: #561's slow path keeps its chain, #597's host-originated synchronous call is a hop, and 854bef5d's asynchronous call on a marked chain is a hop too. The model adds Go / Swift / Kotlin's `Profile` (`stacked = false`, the constant `Threaded`), `bail` / `waterfall-sync` synchronous dispatch, a caller that gives up a synchronous call locally (no `cancel`), Python's synchronous call from another thread (an edge the host cannot see), session end, and SYN-9-3's explicit non-guarantee. What changed since #535 is in "Results" and "Counterexamples".

**Addendum (2026-11-10, #614 continued).** The host is a participant now: the number of threads running Rust code is bounded (`Workers`); a synchronous Rust service call, the host's own synchronous call and a `bail` / `waterfall-sync` dispatch each hold one, and wait in a queue when none is free (`policy-b-workers1`). An asynchronous `serial` / `parallel` / `waterfall` dispatch waits inside a host execution for its listener calls; its first poll may suspend (the filter), and a chain lost across that suspension is `before-lost-chain`. `cancel` follows #561: the caller stops waiting, the callee keeps its mark until it really ends, and a late `ret` is dropped (`before-cancel` is the pre-fix behaviour, pinned by `MarkedWhileRunning`). A Cordis node's connectors share one pump (`Linked`): while one connection waits it reads no other, a cross-session wait the host cannot see (`link-policy-b`); a cycle across hosts is built as "one segment of the wait is in another table" (`Remote`, `cross-host`).

## Design and code it corresponds to

| Part of the model | Design | Implementation |
| --- | --- | --- |
| Forwarding, `path` rewriting | [Multi-language design](../../docs/design-multilang-runtimes-2026-10-03.en.md) §5; [M1](../../docs/design-multilang-m1-2026-10-04.en.md) §7 | `crates/rutis-bridge/src/session/rpc.rs` `rebase`, `forward`; `crates/rutis-bridge/src/session/rpc/relay.rs`; `crates/rutis-bridge/src/runtime/rows.rs` `RowService::invoke` |
| Which Rust thread gets an incoming call | M1 §7.2 | `crates/rutis-bridge/src/session/rpc.rs` `receive`: the last id in `path` that a thread is synchronously waiting for |
| The six operations of the wait table (`profile`, `marked`, `answered`, `returned`, `ended`, `forward`) and the three kinds of call (synchronous, asynchronous on a marked chain, entry) | Multi-language design §9, "implementation" | `crates/rutis-bridge/src/session/rpc/waits.rs`; `crates/rutis-bridge/src/session/rpc.rs` `receive`/`send` |
| Runtimes with a thread per call (Go, Swift, Kotlin): `sync-wait`, `reentrant-sync`, and not `sync-stack` | Multi-language design §9 (2026-10-14) | `crates/rutis-bridge/src/session/rpc/waits.rs` `Profile.stacked`; `go/rutis/internal/peer/peer.go`, `swift/rutis/Sources/Rutis/Session/Peer.swift`, `kotlin/rutis/rutis/src/shared/kotlin/arcships/rutis/internal/peer/Peer.kt` |
| A runtime that does not declare `sync-wait` is outside the check | Multi-language design §9, "mixing versions" | `crates/rutis-bridge/src/session/rpc/waits.rs` `forward`: a call without `sync` is not recorded; `profile` |
| Synchronous event dispatch: `bail`, `waterfall-sync` | [Events protocol](../../docs/design-events-protocol-2026-10-18.en.md) §5.4 | `crates/rutis-bridge/src/events/hub.rs` `dispatch` (`Mode::Bail`, `Mode::Waterfall if sync`), `call_sync`, `Waits`/`wait_here` |
| The host's bounded worker threads (`Workers`): a Rust service call, the host's own synchronous call and a synchronous dispatch each hold one | Multi-language design §9; events protocol §5.4 | `crates/rutis-bridge/src/session/rpc.rs` `receive`; `crates/rutis-bridge/src/events/hub.rs` `dispatch`; `crates/rutis-bridge/src/session/rpc/waits.rs` (it records edges per session; the host's threads are not in the graph) |
| Asynchronous dispatch `serial` / `parallel` / asynchronous `waterfall`: the filter may suspend once, listener calls are asynchronous and the execution awaits them | Events protocol §5.4, §5.5; multi-language design §9 (#561) | `crates/rutis-bridge/src/events/hub.rs` `dispatch` (`Mode::Serial`, `Mode::Parallel`, `Mode::Waterfall` when not sync), `call_async` |
| `cancel`, a late `ret`, the callee keeping its mark until it ends | Multi-language design §9 (#561) | `crates/rutis-bridge/src/session/rpc/waits.rs` `marked` / `answered` / `returned`; `crates/rutis-bridge/src/session/rpc.rs`; Go's context, Swift's Task, Kotlin's interrupt |
| A Cordis node's connector pump: while one connection waits, it reads no other | Multi-language design §9 | `crates/rutis-bridge/src/peer.rs` (`link.offers`); one pump per connector on the Cordis side |
| Node runtime: non-reentrant | Multi-language design §9; [plugin API](../../docs/guide/plugin-api.en.md), "sync calls and reentrancy" | `node/rutis-runtime/src/session.mjs` `#requestSync`, `#run` |
| Python runtime: reentrant; a synchronous call from another thread | same | `python/rutis/rutis/peer.py` `_call_sync`, `_run`; `run_coroutine_threadsafe` |
| Giving up a synchronous call locally (no `cancel`) | Multi-language design §9 (after #561) | Go's context, Swift's Task, Kotlin's interrupt |
| Policy B (A is not adopted) | Multi-language design §9 | `crates/rutis-bridge/src/session/rpc/waits.rs` (#228); the `sync` field of call frames in `node/rutis-runtime/src/session.mjs`, `python/rutis/rutis/peer.py`, `bun/rutis-bun/src/session.ts` |
| Trace validation of the wait graph (#613) | "Trace validation" in this file | `crates/rutis-bridge/src/session/rpc/waits/trace.rs` (records the six operations and each decision of `forward`), `crates/rutis-bridge/src/session/rpc/waits/trace/check.rs` (replays them as `RustSucc`); the differential test of `waits.rs`'s `cycle_among` against the model, `cycle_among_agrees_with_the_models_graph`; the runs are `crates/rutis-loader/tests/sync_cycles.rs` and `crates/rutis-bridge/tests/rpc_callbacks.rs` |

## Model

- **Participants**: the Rust host and 3 runtimes. A runtime is either single-threaded with one stack (outside `Threaded`, declares `sync-stack`) or one thread or task per incoming call (`Threaded`: Go / Swift / Kotlin). One session per runtime; each direction is delivered in order.
- **A stacked runtime**: only the top runs. When the top makes a synchronous call, the thread reads incoming frames while it waits (pumping): a reply marks its call as returned; a call runs at once when idle; while waiting, a reentrant runtime runs it nested and a non-reentrant one puts it aside unless its path names one of its waits; a top that is running reads nothing.
- **A runtime with a thread per call**: every incoming call starts on its own thread at once; nothing waits for anything, there is no stack order and no call is put aside. Its own synchronous calls still wait.
- **Rust** reads each session in order, forwards calls, rewrites `path`, and returns results to their callers. It also makes its own synchronous calls (a Rust plugin's), runs `bail` / `waterfall-sync` synchronous dispatches, and is called synchronously by a runtime (a Rust service or function): all of these run on the host's `Workers` threads, a call that waits holds one, and a call with none free queues (`policy-b-workers1`). Read threads are not counted.
- **The host as a participant**: a host execution (`kind` = `"service"`, `"root"`; a synchronous dispatch lives in `dispatches`) is a piece of Rust code. A synchronous call it makes is the host's own (`src` empty, #597) and is recorded in the wait graph as usual; the call that brought it in from a runtime is not (`waits.rs` records `forward` per session). An asynchronous dispatch inside it does not hold a thread while it awaits.
- **How a call is classified** (waits.rs `forward`): the caller waits synchronously and the last call of its chain is marked → a **hop** (`kind` = `"hop"`); it waits synchronously but no marked call is on its chain → a hop with an empty `src` (#597); an asynchronous call whose chain's last call is marked → a hop too (`kind` = `"async"`, though nobody waits for it); otherwise an **entry**. A synchronous call from a runtime that does not declare `sync-wait` (not in `Declared`) is recorded as an entry (`kind` = `"hidden"`): the host does not see that wait.
- **Emits and posts**: a running activation may emit an event (`MaxPosts` in total), an `events.dispatch` nobody waits for, and it goes on. Rust then posts the listener's call to the listener's runtime (an entry). `EmitChain` says whether that call carries the emitter's chain (#561: it does not).
- **Synchronous dispatch**: `bail`, and a Cordis plugin's `waterfall`, make the emitter wait for the whole dispatch; the host calls one listener and waits for it. `Dispatch` happens at a top-level activity only (a plugin's `ctx.emit` / `ctx.bail`); `DispatchHop` says whether Rust records the dispatch itself as a hop. A `bail`'s listener call carries no chain, a `waterfall`'s does (calls back into the emitter's process have to land on the chain it waits for).
- **Asynchronous dispatch**: `serial`, `parallel` and an asynchronous `waterfall` happen inside a host execution (a Rust service handling a runtime's synchronous call). The dispatch suspends once first (the filter, `st = "filter"`), then sends one asynchronous `events.call` to a listener and awaits it. With `KeepChain` the listener call carries the emitter's chain; without it (#561 before) the chain is lost across the suspension and a listener calling the waiting side back is put aside (`before-lost-chain`).
- **Cancellation**: a runtime in `Cancellers` may cancel a synchronous wait (`cancel`): the caller stops waiting; the forwarded call stays in Rust's table but nobody waits for it (`real = false`) and a late `ret` is dropped; the callee (a runtime or a Rust service) keeps its mark until it really ends (`KeepMark`; `before-cancel` clears it on the cancel). A call cancelled before it starts still runs in the model, and only its reply is dropped ("it may have started").
- **A shared pump**: `Linked` groups sessions (a Cordis node's connectors). While any session of a group waits, the others read no frames (`Group`); the host sees independent sessions and not this wait (`link-policy-b`).
- **A session on another host**: a session in `Remote` is on the other host; its synchronous waits are in the other host's table, not this one, and the path this host writes carries none of its call ids, so a callback cannot land on its chain (`cross-host`).
- **Session end**: with `Ends`, a session may end while it holds no other session and no host execution is live: its calls leave the table and calls into it are answered with an error (SYN-9-30).
- **Call ids** are numbered per session; the model also keeps every call's true ancestry (`truth`), apart from the wire `path`.
- **Policies** (`Policy`): `current` detects nothing; `A` refuses synchronous calls between two non-reentrant runtimes; `B` builds a wait graph before each forward and answers `SyncWaitCycle` on a cycle; `Bchain` is B without the stack-order edges.

### Wait graph

Nodes are the calls Rust forwarded and has not answered (`hops`), plus posts nobody waits for (`posts`) and dispatches it does not record (`dispatches`). An edge c → d means c cannot finish before d. A cycle is a deadlock.

| Edge | Meaning | Visible to Rust? |
| --- | --- | --- |
| chain | d was made under c (d's `path` names c) | yes: rewritten paths carry session tags |
| put aside | c entered a non-reentrant, single-stack runtime y that is waiting, c is not on its chain and has not started there; c waits for every call y has out | yes: y's outstanding calls are Rust's own state; `reentrant-sync` and `sync-stack` say which |
| stack order | c and then d entered the same single-stack runtime y and d has started there; c waits for d | yes: frames of a session are ordered |
| dispatch | the host calls a listener inside a synchronous dispatch, and the dispatch waits for that call; a call back into the emitter lands on the chain it waits for | the dispatch itself is not in the table (`DispatchHop` = FALSE); the listener call is the host's own synchronous call (a hop) |
| entry | a call nobody waits for synchronously (`emit`'s `events.call`, an asynchronous call on a marked chain, a synchronous call the host cannot see) is a boundary of chains | it is recorded (`entry`), but it does not count as a call out of its runtime |
| host thread | a queued host execution waits for every host execution or synchronous dispatch holding a worker | no: the host's threads are not in the table (`policy-b-workers1`) |
| host wait | a host execution waits for the calls it makes; an asynchronous dispatch waits for its listener call | only the forwarded calls are in it (as hops); the service call itself and the dispatch are not |
| cancel | a cancelled hop stays a node with `real` false: it waits for nothing and nothing waits for it | it stays in the table until the reply, which is then dropped |
| connector pump | while one session of a group waits, the other sessions' incoming calls do not start (TLC's deadlock check sees it directly) | no: `Linked` sessions are independent in the table (`link-policy-b`) |
| another host | a wait that comes from another host is only in that table; this one sees it as an entry, and the chain carries none of its call ids | no: `Remote` calls are recorded as entries (`cross-host`) |

`Linked` adds no edge: the model disables `Receive` for a session while another session of its group waits, and TLC's deadlock check sees "frames but nobody can read them". The host has no record of it.

The model has two graphs: `TrueSucc`, from the runtimes' real stacks, queues and each node's `real` (someone really waits for it) — only for judging; and `RustSucc`, from Rust's own state and the `entry` flags it recorded — what policy B can do. A node whose `real` is false (a post, an asynchronous call on a marked chain, a call given up locally) is not on a real cycle, but it may be on one in Rust's graph.

## Simplifications

- A runtime's calls may be synchronous, asynchronous (`Async`) or "it waits but the frame does not say so" (`Hidden`); Rust's posts and synchronous dispatches are built as described.
- Calls inside one runtime do not cross Rust and are not modelled.
- Service calls and function (callback) calls are one action; `await`, reference counts and value encoding are not modelled.
- A synchronous dispatch happens at a top-level activity only; an asynchronous one only inside a Rust service's execution. `next` (a waterfall's chain), several listeners, a filter's veto and its exceptions are not built: one dispatch has one listener, and the filter only chooses "suspend once, then go on". A listener in the emitter's own runtime is modelled; `next` would add the listener-to-host edge, which the model does not have.
- A host execution has two sources: a runtime's synchronous service call, or the host's own synchronous call (`HostRoot`). The host's `Workers` is bounded and read threads are not counted; an asynchronous dispatch awaiting its listener does not hold a thread. Only a synchronous `call` into the host is modelled (`acall` / `hcall` into the host are not: an asynchronous reply has no node to land in).
- Cancellation is modelled in two forms: with `cancel` (`Cancellers`) and a local give-up without it (`Abandoners`, open to `Threaded` runtimes only: what Go / Swift / Kotlin do). A `cancel` that reaches a call before it starts lets it run and drops its late reply; a `ret` blocking the reader is not built.
- A session ends only while it has no call out, takes part in no dispatch and no host execution is live (`Ends`); calls into it are answered with an error. A break, a reconnect and `rows.ended` are not modelled.
- **Not built**: Node's keyed holds (a later call of the same key waits behind an earlier emit); a behind task is `deferred` (it runs only when the stack is empty and is dropped when the session ends); a Cordis connector's inline handling and holds are only the shared pump; a cycle across hosts is built as one segment (`Remote`), not as two real hosts.
- Evidence outside the model: the differential test for `cycle_among` and the trace validation of the wait graph are both done (#613); see the done items under "Open questions" and "Trace validation".
- Each activation makes one call by default; `policy-b-2node-seq` allows two in sequence.
- Scale: 3 runtimes, chains ≤ 3 hops (`async-policy-b` up to 4), ≤ 2 concurrent top-level activities, ≤ 1 post or dispatch.

## Invariants

| Name | Meaning |
| --- | --- |
| no deadlock | TLC deadlock check: while calls are unfinished, some participant can act. The finished state stutters (`Done`). Every behaviour is finite, so "no deadlock" is equivalent to "every call eventually completes" |
| `RoutingSound` | A call run as part of a waiting chain (nested by a non-reentrant runtime, or handed to a waiting Rust thread) really belongs to that chain by true ancestry, whether or not every node in between is waited for |
| `ChainRecognised` | A non-reentrant runtime never puts aside a call of its own chain: true ancestry, every node of which someone really waits for (posts and the like do not count) |
| `NoFalseCycle` | Policy B answers `SyncWaitCycle` only if the real wait graph plus this call has a cycle |
| `NoLegitimateRefused` | Policy A refuses no legitimate call (forwarding it closes no cycle in the real wait graph) |
| `MarkedWhileRunning` | A synchronous call received and run by Rust code that is cancelled stays marked until it really ends; a call it forwards afterwards is still a hop. `before-cancel` clears the mark on the cancel and violates this |
| `NeverCycle` | Only to confirm B's check fires (expected to be violated) |
| `NeverNested` | SYN-9-3 (expected to be violated): a reentrant runtime's service is called in the middle of its own synchronous call. Not a defect: an explicit non-guarantee, pinned by a property |

## Results

TLC 2.19 (tla2tools v1.7.4), Temurin 21, 32 cores, `run.sh` with `-workers auto`; `MaxRoots = 2`, `MaxDepth = 3`, `MaxCalls = 1` unless noted. Every configuration was rerun on 2026-11-10 and the counts and times below are from that run; the host-related ones set `Workers` (1 for `policy-b-workers1`, 4 for the asynchronous dispatches, 2 otherwise).

| Configuration | Runtimes (`n` non-reentrant, `p` reentrant, `g` a thread per call) | Result | Distinct states | Time |
| --- | --- | --- | --- | --- |
| `current-1node-1python` | n, p | pass | 652 | <1 s |
| `current-2node` | n1, n2, p | **deadlock** (counterexample 1) | 915 | <1 s |
| `current-1node` | n, p1, p2 | **deadlock** (counterexample 2) | 12,340 | 1 s |
| `current-reentrant` | p1, p2, p3 | pass | 113,191 | 1 s |
| `current-norebase` | n1, n2, p, no `path` rewriting | **`RoutingSound` violated** (counterexample 3) | 389 | <1 s |
| `policy-a-1node` | n, p1, p2 (the combination A intends) | **deadlock** (as counterexample 2) | 7,058 | <1 s |
| `policy-a-2node` | n1, n2, p | **deadlock** (shape of counterexample 2) | 2,687 | <1 s |
| `policy-a-refusals` | n1, n2, p | **`NoLegitimateRefused` violated** (counterexample 4) | 212 | <1 s |
| `policy-bchain-2node` | n1, n2, p | **deadlock** (as counterexample 2) | 7,730 | <1 s |
| `policy-b-2node` | n1, n2, p | pass | 41,107 | 1 s |
| `policy-b-1node` | n, p1, p2 | pass | 65,400 | 1 s |
| `policy-b-nonreentrant` | n1, n2, n3 | pass | 27,012 | <1 s |
| `policy-b-2node-depth4` | n1, n2, p; chains ≤ 4 | pass | 163,909 | 3 s |
| `policy-b-2node-3roots` | n1, n2, p; 3 top-level | pass | 1,398,345 | 21 s |
| `policy-b-1node-3roots` | n, p1, p2; 3 top-level | pass | 3,584,445 | 45 s |
| `policy-b-2node-seq` | n1, n2, p; 2 calls per activation, chains ≤ 2 | pass | 3,120,478 | 35 s |
| `policy-b-acts` | n1, n2, p | `NeverCycle` violated (expected: B fires) | 793 | <1 s |
| `current-posts` | n, p, q (p, q reentrant); 2 top-level calls, 1 emit | **deadlock** (counterexample 5) | 135,882 | 2 s |
| `policy-b-posts` | as `current-posts`, policy B with entries; CI size with `MaxDepth=2` | pass | 1,152,323 | 11 s |
| `policy-b-posts-node` | n, p, q (p non-reentrant, `q` reentrant), policy B; CI size with `MaxDepth=2` | pass | 641,339 | 7 s |
| `threaded-policy-b` | n, g, p (g a thread per call) | pass | 62,128 | 1 s |
| `unmarked-policy-b` | n, g, p; g does not declare `sync-wait` | **deadlock** (counterexample 9, explicit non-guarantee) | 760 | <1 s |
| `bail-policy-b` | r, t; a `bail` dispatch Rust does not record (pre-fix behaviour; current code carries the chain) | **deadlock** (counterexample 6) | 282 | <1 s |
| `waterfall-policy-b` | r, t; a `waterfall` dispatch | pass | 3,277 | <1 s |
| `bail-recorded` | r, t; `bail`, Rust records the dispatch as a hop | pass | 3,494 | <1 s |
| `emit-unchained` | n, p, q; an emit's listener call carries no chain (`EmitChain` FALSE), 1 top-level | pass | 69,591 | 2 s |
| `host-call` | n1, p; the host's own synchronous call (holds one `Workers` thread) | pass | 5,939 | 1 s |
| `before-host-call` | as above, before #597 (an entry) | pass | 5,939 | 1 s |
| `abandon-policy-b` | n, g, p; g gives up a synchronous call locally | **`NoFalseCycle` violated** (counterexample 7) | 59,335 | 1 s |
| `hidden-policy-b` | n, p, q; p calls synchronously from another thread | **`ChainRecognised` violated** (counterexample 10, explicit non-guarantee) | 941 | <1 s |
| `async-policy-b` | n, p, q; an asynchronous call on a marked chain (implementation keeps the conservative edge; known false alarm) | **`NoFalseCycle` violated** (counterexample 8) | 27,725 | 1 s |
| `nested-service` | p1, p2 | `NeverNested` violated (expected, SYN-9-3) | 118 | <1 s |
| `end-session` | n1, n2, p; a session may end | pass | 84,695 | 1 s |
| `policy-b-workers1` | p, q, r; `Workers = 1`, the host's own call holds the only thread | **deadlock** (counterexample 12, explicit non-guarantee) | 950 | <1 s |
| `serial-policy-b` | r, t; a `serial` dispatch, chain kept across the suspension | pass | 1,601 | <1 s |
| `parallel-policy-b` | r, t; a `parallel` dispatch (one listener) | pass | 1,601 | <1 s |
| `waterfall-async-policy-b` | r, t; an asynchronous `waterfall` (one listener, `next` not built) | pass | 1,601 | <1 s |
| `before-lost-chain` | as `serial-policy-b`, chain lost across the suspension (before #561) | **deadlock** (counterexample 13) | 599 | <1 s |
| `cancel-policy-b` | r, t; cancel a running Rust service call (`KeepMark`) | pass | 1,613 | <1 s |
| `cancel-forward-policy-b` | r, t; cancel a forwarded synchronous call, late reply dropped | pass | 480 | <1 s |
| `before-cancel` | as `cancel-policy-b`, the mark cleared on the cancel (before #561) | **`MarkedWhileRunning` violated** (counterexample 14) | 426 | <1 s |
| `link-policy-b` | l1, l2 (one pump), x; l1 waits for x, x calls l2 | **deadlock** (counterexample 15, explicit non-guarantee) | 94 | <1 s |
| `link-alone` | as `link-policy-b` with `Linked = {}` (a pump per connection) | pass | 468 | <1 s |
| `cross-host` | a (on the other host), b; a waits for b, b calls a back | **deadlock** (counterexample 16, explicit non-guarantee) | 34 | <1 s |

"Pass" means exhaustive: no deadlock, all listed invariants hold; state counts on these rows are exact. On deadlock and violation rows TLC stops at the first counterexample, so the count depends on the order of the search and varies between runs; treat it as indicative.

The state counts differ from the first #614 reconciliation (for example `current-1node` 8,908 → 12,340, `policy-b-2node` 37,311 → 41,107): those were the #535 model, and `entry`, `real`, `mk`, `cancelled` and the rest widened the state space. `host-call` and `before-host-call` also carry a worker id now. `policy-b-posts` and `policy-b-posts-node` use `MaxDepth=2` for CI; with chains ≤ 3, `policy-b-2node-seq` passed 44 million states in 10 minutes and was still growing, so it runs with chains ≤ 2. Every configuration finishes within a minute except `policy-b-1node-3roots` (45 s), `policy-b-2node-seq` (35 s) and `policy-b-2node-3roots` (21 s).

`current-1node-1python` backs the statement in §9 of the multi-language design that one Node and one Python runtime do not deadlock.

`before-host-call` is expected to pass: with no call marked, the host's own synchronous call is an entry; with a complete chain, the only incoming edge a top-level call can have is stack order, and a mark already exists then, so it is an entry, and entries have that kind of edge, so no cycle is missed. #597's unit test is about the truncated-chain case (`session/rpc/tests.rs`, `rutis-loader/tests/sync_cycles.rs`).

### Counterexamples

1. **Two Node runtimes calling each other** (`current-2node`). n1 synchronously calls a service of n2 while n2 synchronously calls a service of n1. Neither call is on the other's chain; each is put aside; both wait forever. The case §9 already knows.
2. **Stack order on a reentrant runtime** (`current-1node`; A and Bchain alike).
   1. n synchronously calls service `s1` of p2;
   2. `s1` synchronously calls p1;
   3. p1 itself (a timer, say) synchronously calls service `s2` of p2. p2 is waiting and reentrant, so it runs `s2` nested above `s1`;
   4. `s2` synchronously calls a service of n. n is waiting for `s1`; the call is not on its chain and is put aside;
   5. `s1`'s call has returned, but `s2` is on top, so `s1` waits for `s2`; `s2` waits for n; n waits for `s1`. Deadlock.

   So "all leaf runtimes reentrant" does not remove deadlocks by itself: reentrancy turns "put aside" into "buried under", and one non-reentrant runtime is enough to close a cycle. It happens with a single Cordis runtime.
3. **No `path` rewriting** (`current-norebase`). n1 and n2 each wait for their own `node:1`; n1's call reaches n2 with `path = ["node:1"]` and n2 takes it for a call on its own `node:1` chain and runs it nested. This is what today's `rebase` prevents.
4. **A refuses a call that cannot deadlock** (`policy-a-refusals`). n1 synchronously calls n2 while n2 is idle and nothing else runs; A refuses it.
5. **A post on top of a wait** (`current-posts`). n synchronously calls `x` in p, and `x` synchronously calls q. Meanwhile q emits an event whose listener is in p, and Rust posts the listener's call to p. p is reentrant, so it runs the call on top of `x`. The listener synchronously calls n; n is waiting for `x` and puts the call aside. `x` is buried: n waits for `x`, `x` for the listener, the listener for n. No caller waits for the post, so the old graph does not have it and misses the cycle; recorded as an entry (`policy-b-posts`), the listener's call is refused. With no chain on the listener call (`emit-unchained`, #561) the cycle is still found at this scale: those edges come from "put aside" and stack order, not from the emitter's chain.
6. **A blind spot on synchronous dispatch** (`bail-policy-b`, 2026-11-09). r (non-reentrant) makes a `bail` dispatch: r waits for it, the host calls a listener in t and waits for it, and the listener synchronously calls r. The model assumes `waits.rs` does not record the dispatch as a hop and the listener call carries no chain, so r does not recognise the callback and puts it aside; policy B misses the cycle and deadlocks. The current compliant runtime already puts the dispatch id on the listener's `path` (equivalent to `bail-recorded`); `bail-policy-b` is the pre-fix configuration. Only a runtime that sends `events.dispatch` without declaring `sync-wait` (an old SDK, `unmarked-policy-b`, SYN-9-26) degrades to this blind spot, recorded as explicitly not guaranteed.
   - `waterfall-policy-b` is the same shape with `waterfall-sync`: its calls back into the emitter carry the emitter's wait chain, so r runs the callback nested; it passes.
   - `bail-recorded` matches the current implementation: Rust puts the dispatch's call id on the listener's `path` (as if the dispatch were a forwarded call), so r recognises the callback; it passes. Conclusion: a `bail` dispatch has to be visible in the wait graph; compliant runtimes already satisfy this, and runtimes without `sync-wait` are outside the check.
7. **The edge left after a local give-up** (`abandon-policy-b`). Go's context, Swift's Task and Kotlin's interrupt give up a synchronous call without a `cancel`: the caller's thread goes on, and the call stays in the host's table until the reply arrives. In the graph it is still "a call out of that runtime", so B may refuse a call that would have finished (violating `NoFalseCycle`). The fix is to send `cancel` when giving up (the rule #561 settled), or to accept the false positive.
8. **An asynchronous call on a marked chain** (`async-policy-b`, 854bef5d). The caller does not wait for it, but on a marked chain `waits.rs` still records it as a hop. n is waiting for a call; p's asynchronous call enters n and is put aside, while Rust's graph has it as "a call out of n" plus this new call, so a call that would finish is refused (violating `NoFalseCycle`). The implementation keeps the conservative edge from 854bef5d for now: the frame only has `sync`, which cannot tell "fired and forgotten" from "awaited later"; removing the edge loses a real wait (the synchronous object-relay cycle test hangs). The false alarm is recorded as known until the protocol carries a "waits for the result" signal.
9. **A runtime without `sync-wait`** (`unmarked-policy-b`, SYN-9-26). g's synchronous calls carry no `sync`, the host does not record them, and the whole session is outside the check; the n → g → p → n cycle is not found and deadlocks. This is an **explicit non-guarantee**: an older SDK can still deadlock as before, but nothing is falsely refused.
10. **A synchronous call from another thread** (`hidden-policy-b`). Python calls synchronously from another thread (`run_coroutine_threadsafe`); the frame carries no `sync`, so the host does not see the wait. The call enters a waiting non-reentrant runtime and is put aside; `ChainRecognised` is violated (the first step of a deadlock). This is an **explicit non-guarantee**.
11. **A reentrant runtime called mid-call** (`nested-service`, SYN-9-3). p1 synchronously calls a service of p2; inside that call p2 synchronously calls p1; another activity of p1 synchronously calls p2, and p2, reentrant, nests it above the call it is running. `NeverNested` is violated: an **explicit non-guarantee** ("do not hold a lock while calling a rutis service" cannot be enforced), pinned by a property that is expected to be violated.
12. **The host's worker threads run out** (`policy-b-workers1`, 2026-11-10). With `Workers = 1`, the host's own synchronous call (`HostRoot`) holds the only thread and waits for r; r, inside the wait, calls a Rust service; the service is not on r's chain (`waits.rs` records edges per session) and needs a worker, and the only worker is held. r waits for the service, the service for a worker, the worker for r: deadlock. The state of the host's threads is not in the wait graph, so B does not see it. This is an **explicit non-guarantee**: Rust code waiting synchronously holds a worker; concurrent waits like this can fill the pool, and if one of them also waits for an incoming call that is not on its chain and needs a new thread, the host stalls. The smallest shape the model finds is the host's own call, one runtime's synchronous service call and one service call; the issue's three-runtime example is the same cycle.
13. **A chain lost after an asynchronous dispatch suspended** (`before-lost-chain`). r (non-reentrant) synchronously calls a Rust service; the service starts a `serial` dispatch; the listener call is made only after the dispatch suspended (the filter). With `KeepChain` FALSE (before #561) the listener call carries no id of r's wait; when the listener is in r (or calls r back), r does not recognise it and puts it aside. r waits for the service, the service for the dispatch, the dispatch for the listener: deadlock. `serial-policy-b` / `parallel-policy-b` / `waterfall-async-policy-b` keep the chain and pass. This configuration checks the deadlock only, not `ChainRecognised`: the lost chain also makes a non-reentrant runtime put a call of its own chain aside. It corresponds to #561's slow-path tests (`session/rpc/tests.rs`, `waits.rs`).
14. **The mark cleared on a cancel** (`before-cancel`). r synchronously calls a Rust service; after the call has started, r sends `cancel`. Before the fix (`KeepMark` false) the service drops its mark on the cancel; a call it forwards afterwards is no longer a hop under a marked chain, violating `MarkedWhileRunning`. `cancel-policy-b` keeps the mark until the call really ends and passes; `cancel-forward-policy-b` is the same rule on a forwarded call (the late `ret` is dropped). It corresponds to #561's cancel rules (`waits.rs` `marked` / `answered` / `returned`).
15. **A Cordis node's connections share one pump** (`link-policy-b`). l1 synchronously calls x; inside that call x synchronously calls l2; l1 and l2 are two connectors of one Cordis node, sharing a pump, and while l1 waits it reads no frames of l2. The host takes l1 and l2 for independent sessions: l1 waits for x and x for l2 are in its table, but "l2 cannot read" is not, so it deadlocks. `link-alone` drops `Linked` (a pump per connector) and passes. This is an **explicit non-guarantee**: the host cannot see a cross-session wait inside one node.
16. **A cycle across hosts** (`cross-host`). a is on the other host: its synchronous wait for b is in the other table, this one records it as an entry, and the path this host writes carries none of a's call ids (`Remote`). a waits for b, and b synchronously calls a back; a does not recognise the callback and puts it aside: deadlock. Each host sees only its own segment, and `SyncWaitCycle` fires on neither side. This is an **explicit non-guarantee**.

## Recommendation: policy B (with stack order)

Adopted by the maintainer (written into §9 of the [multi-language design](../../docs/design-multilang-runtimes-2026-10-03.en.md); implementation in #228).

The reasons (as of #535): A is not enough; within the model B removes the deadlocks and raises no false alarm; B uses only what Rust has; the stack-order edges cannot be left out; a new language's runtime may choose to be reentrant or not, as long as it says so.

What an implementation of B needs (the model's premises):

1. one host-wide table of every forwarded synchronous call not yet returned (source session and id, target session and id, the chain with tags resolved);
2. replies recorded in frame order: on one session, an earlier reply leaves the table before a later call does;
3. a check before each forward, answering `SyncWaitCycle` with the calls of the cycle instead of forwarding;
4. reliable reentrancy and stack information: `reentrant-sync`, `sync-stack` (Node declares no `reentrant-sync`; Go / Swift / Kotlin declare no `sync-stack`);
5. only **synchronous** waits in the graph (see "Open questions").
6. the number of threads running Rust code on the host must be bounded (`Workers`); B builds no edges for the host's threads, so blocking on them is outside the check (counterexample 12).

## Trace validation (#613)

The six operations of `waits.rs` and each decision can be recorded in test mode and checked against this model event by event. The record points are in `crates/rutis-bridge/src/session/rpc/waits/trace.rs`, compiled only under the `testing` feature; with no capture alive each operation adds one atomic read, no allocation, and no change in behaviour.

Recorded are `profile` (a session's `sync-stack` / `reentrant-sync`), `marked`, `answered`, `returned`, `ended`, and every decision `forward` makes: a hop (who made it, whether it is an `entry`), back on its caller's own chain (not recorded), nobody waiting synchronously (not recorded), or a refusal (the calls of the cycle, the new one first). A run starts the record with `capture()` and replays it with `check()` (`trace/check.rs`):

- A session's `profile` is a constant of the model: a second one that says otherwise is reported. A session no profile was recorded for is taken as `Profile::default()`, as `waits.rs` reads it.
- A marked call is not marked twice and is not a call this table sent; `answered` never answers a call this side sent; `ended` holds only while the session has no call out (the premise of `End`), and it drops that session's marks, profile and the hops on both sides, after which no event names it.
- Each `forward` rebuilds the graph as `RustSucc` (chain, put aside, stack order) and then compares the decision: a chain whose last call is marked, on another session, must be a hop with that call as `src`; a synchronous call with no marked chain is the host's (`src` empty); an asynchronous one with nothing marked is an `entry`. A hop may not wait on itself (the model does not forward such a call); a refusal must name a cycle that exists in the graph; the call numbers of one session grow.
- What the model explicitly does not guarantee (a synchronous call that was not recorded, SYN-9-26) comes back as a note; the tests assert there are none.

The runs checked: `two_node_runtimes_calling_each_other` (counterexample 1) and `one_node_and_two_python_runtimes` (counterexample 2: put aside, stack order, a reentrant Python) in `sync_cycles.rs`, and `a_function_forwarded_between_two_node_runtimes_calls_back_its_waiting_owner` in `rpc_callbacks.rs` (a forwarded call that comes back on its caller's chain). The other tests of the same binary run at once and their events are in the record too (a test captures, the operations of the others still land in it); every session's tag is one the process assigns (`next_tag` in `rpc.rs`), and the replay keeps them apart by it.

The check is not vacuous: `must_report` in `sync_cycles.rs` breaks one decision of the real log (the refusal turned into a hop, the caller dropped, a mark dropped) and the replay must report it; `waits/trace/check/tests.rs` covers, on traces written by hand, an allowed trace, a cycle forwarded, a refusal naming the wrong calls, an `entry` recorded as a hop, a changed profile, a session ending with its call out, and call numbers that do not grow.

Not covered:

- **A caller waits synchronously with no marked call on its chain**: the model records `src = s` (the session that made the call), `waits.rs` records the host's (`src` empty, #597), and the record names only the target session, so it cannot tell the two apart. The model's own comment also says "a hop of the host's", against `src |-> s` in `RustForward`: the two need to agree before this can be checked.
- **Cancel**: the model keeps a cancelled hop in the table (`real = FALSE`, a late reply dropped); the table removes it (`returned`). These runs do not cancel, so it is not covered.
- The premises `End` puts on dispatches and host executions (`hosts = {}`, no dispatch through the session), `Ends`, `Workers`, `Linked` and `Remote` are outside these runs.

## Open questions and what is not covered

- **Telling synchronous from asynchronous (settled, modelled)**: the call frame's `sync` field says the caller waits synchronously; only calls with it, and calls whose chain's last call is marked, enter the graph (#228, 854bef5d). The model shows that recording an asynchronous call on a marked chain as a hop raises false alarms (counterexample 8): either fix the implementation or count only `sync` calls.
- **Synchronous dispatch has to be visible (new, counterexample 6)**: a `bail` dispatch and its listener call must be connected in the graph (recorded as a hop, with the emitter's wait id on the listener's `path`), or a deadlock goes undetected. `waterfall-sync` passes today because calls back into the emitter's process carry its innermost wait chain.
- **Node's keyed holds are not built**: a later call of the same key waits behind an earlier emit; that needs two emits on one key and their order in flight. `deferred` builds a behind task only (it runs when the stack is empty and is dropped when the session ends), not the per-key order. The other Node detail from #614, "a `SyncWaitCycle` when a waiting chain awaits an unresolved promise", is not built either: it is a `waits.rs` decision, not the shape of the wait graph.
- **A cycle across hosts is built as one segment**: `Remote` records "this session's wait is in another table" as an entry, and the path carries none of its call ids; `cross-host` is the counterexample. The real topology of two hosts, the path rewriting on a node link and the refusal on the other side are not built. §9 of the multi-language design and `bridge/sync-wait-cycle-refused` carry it as an explicit non-guarantee.
- **The premise of `Workers`**: the model assumes a synchronous wait in Rust code holds a worker thread (a Rust service runs on the calling thread, and the synchronous call blocks it). If in the implementation a Rust service runs on an asynchronous executor and the wait holds no thread, `policy-b-workers1` becomes expected to pass and the worker edge goes away. The asynchronous dispatch holding no thread while it awaits is the other half of the same judgement.
- **Bounded thread pools on the runtime side are not built**: Kotlin's IO pool (64) and Swift's cooperative pool are not resources the host can see; a `Threaded` runtime is assumed to have a thread or task for every incoming call. A full pool queues inside the runtime, invisible to the host, the same kind of problem as the host's workers in `policy-b-workers1`.
- **The differential test for `cycle_among` (done, #613)**: `cycle_among_agrees_with_the_models_graph` in `waits.rs` compares `cycle_among` (sorted by (dst, seq), only the nearest chain hop and the next started call) with a depth-first search of every edge the model's `RustSucc` builds (chain, put aside, stack order, entry boundary) on random graphs of up to 20 hops: the two agree on a cycle or its absence, and the cycle found starts at the new call and has an edge of the model's graph for every pair of calls in it, with random `Profile` (`stacked` / `reentrant`) and random entries. The case count is `PROPTEST_CASES` (256 by default); a failure prints the seed, replay it with `RUTIS_PROPTEST_SEED=<seed>` (Q7.2). `the_model_check_agrees_on_the_small_graphs` pins the shapes one by one: an empty graph, a chain closing a cycle, a chain naming itself, a chain cut by an entry, a chain naming a call that has returned.
- **Trace validation of the wait graph (done, #613)**: the six operations of `waits.rs` and each decision are recorded under the `testing` feature and replayed event by event against the model on runs of `sync_cycles.rs` and `rpc_callbacks.rs`, with negative cases that break the same log. See "Trace validation"; what it does not cover (the `src` of a synchronous wait, cancel, the dispatch premises of `End`) is listed there.
- **Asynchronous dispatch is only built inside a Rust service's execution**: `serial` / `parallel` at a top-level activity (a Cordis plugin's `ctx.parallel`, say) are not built; one dispatch has one listener, and `next`, a filter's veto and its exceptions are not built.
- **Synchronous calls made by Rust itself** are built as host executions (`host-call`, `before-host-call`) and hold one `Workers` thread; read threads and asynchronous executions hold none.
- **B's cost**: a reachability check on the global table before every synchronous forward; the table's size is the number of synchronous calls in progress. The model does not measure time.
- A caller that retries at once after `SyncWaitCycle` may cycle again; whether to back off is the caller's decision, not modelled.

## How to run

Java 11 or later and `tla2tools.jar` ([v1.7.4](https://github.com/tlaplus/tlaplus/releases/tag/v1.7.4), sha256 `936a262061c914694dfd669a543be24573c45d5aa0ff20a8b96b23d01e050e88`). Do not commit the jar.

```sh
mise install java@temurin-21
curl -sSLo /tmp/tla2tools.jar https://github.com/tlaplus/tlaplus/releases/download/v1.7.4/tla2tools.jar
TLA2TOOLS=/tmp/tla2tools.jar mise exec java@temurin-21 -- specs/cross-runtime-sync/run.sh            # every configuration
TLA2TOOLS=/tmp/tla2tools.jar mise exec java@temurin-21 -- specs/cross-runtime-sync/run.sh bail-policy-b   # one
```

`run.sh` compares each configuration's result with `expected.txt` and exits 0 when all match; logs and counterexamples go to `TLC_OUT` (a temporary directory by default). Without the script, in this directory:

```sh
java -cp /tmp/tla2tools.jar tlc2.TLC -workers auto -config bail-policy-b.cfg CrossRuntimeSync.tla
```

Each new configuration finishes within a minute; `policy-b-posts` and `policy-b-posts-node` take about ten seconds locally after reducing to `MaxDepth=2` (estimated 1–2 minutes on CI 4 cores); `policy-b-1node-3roots`, `policy-b-2node-3roots` and `policy-b-2node-seq` take 20–45 s on 32 cores and longer on CI.

## Maintenance

When §5 or §9 of the multi-language design or §7 of M1 changes, or when `rpc.rs`, `waits.rs`, `events/hub.rs`, `session.mjs` or `peer.py` change which calls run during a synchronous wait, or which calls enter the wait graph, change this model and `expected.txt` together (standard Q11.2.3) and run the trace validation of `sync_cycles.rs` and `rpc_callbacks.rs` ("Trace validation"); an operation or decision `waits.rs` gains needs its record point in `trace.rs` and its replay in `trace/check.rs`; a change to `cycle_among` or the edges it uses needs `cycle_among_agrees_with_the_models_graph` in `waits.rs` (`PROPTEST_CASES`, replayed with `RUTIS_PROPTEST_SEED`). Host executions, `Workers`, asynchronous dispatch and `cancel` follow `rpc.rs` `receive`, `events/hub.rs` `dispatch` and `waits.rs` `marked` / `answered` / `returned`.
