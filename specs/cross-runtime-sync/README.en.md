# Cross-runtime synchronous calls and the deadlock policy (TLA+)

[中文](README.md)

TLC explores every interleaving of language runtimes calling each other synchronously through the Rust host, to answer the question left open in §九 of the multi-language design: which of the two candidate policies removes the deadlocks. Issue #208; quality standard Q11.2, Q6.2.8, Q6.8.3; status §7.1 (risks P10, C6).

## Design and code it corresponds to

| Part of the model | Design | Implementation |
| --- | --- | --- |
| Forwarding, `path` rewriting | [Multi-language design](../../docs/design-multilang-runtimes-2026-10-03.en.md) §5; [M1](../../docs/design-multilang-m1-2026-10-04.en.md) §7 | `crates/rutis-bridge/src/session/rpc.rs` `rebase`, `forward`; `rpc/relay.rs`; `runtime/rows.rs` `RowService::invoke` |
| Which Rust thread gets an incoming call | M1 §7.2 | `rpc.rs` `receive`: the last id in `path` that a thread is synchronously waiting for |
| Node runtime: non-reentrant | Multi-language design §9; [plugin API](../../docs/guide/plugin-api.en.md), "sync calls and reentrancy" | `node/rutis-runtime/src/session.mjs` `#requestSync`, `#run`: while waiting, runs only calls whose `path` names a call it waits for; puts the rest aside until its stack is empty |
| Python runtime: reentrant | same | `python/rutis/rutis/peer.py` `_call_sync`, `_run`: while waiting, runs every incoming call |
| Policy B (A is not adopted) | Multi-language design §9 | `crates/rutis-bridge/src/session/rpc/waits.rs` (#228); the `sync` field of call frames and the `sync-wait` capability in `session.mjs`, `peer.py`, `bun/rutis-bun/src/session.ts` |

## Model

- **Participants**: the Rust host (relay) and 3 runtimes, each reentrant or not. One session per runtime; each direction is delivered in order (socket order).
- **Runtime**: one thread, one call stack; only the top runs. When the top makes a synchronous call, the thread reads incoming frames while it waits (pumping):
  - a reply marks its call as returned; that activation continues only once it is on top;
  - a call runs immediately when the runtime is idle; while waiting, a reentrant runtime runs every call nested, a non-reentrant one only calls whose `path` names one of its waits, and puts the others aside until its stack is empty;
  - a top that is running (not waiting) reads nothing.
- **Rust** reads each session in order. It forwards a synchronous call to the target runtime's session with the call id appended and the `path` rewritten by `rebase`, and sends a returned result back to the caller. Rust never makes a runtime wait longer: each forwarded call runs on its own thread or on the thread already waiting for its chain.
- **Call ids** are numbered per session (`node:n` by the runtime, `rust:n` by Rust), so `node:1` exists on two sessions at once. The model also keeps each call's true ancestry (used only by invariants), separately from the wire `path`, so it can check what the runtimes and Rust conclude from the path.
- **Policies** (constant `Policy`):
  - `current`: today's behaviour, no detection;
  - `A`: Rust refuses synchronous calls between two non-reentrant runtimes (`refused`);
  - `B`: before forwarding a synchronous call, Rust builds a wait graph from what it can see; if forwarding would close a cycle, it answers `SyncWaitCycle` instead;
  - `Bchain`: B without the stack-order edges, to show they are needed.

### Wait graph

Nodes are calls Rust forwarded that have not returned; an edge c → d means c cannot finish before d. A cycle is a deadlock.

| Edge | Meaning | Visible to Rust? |
| --- | --- | --- |
| chain | d was made under c (d's `path` names c) | yes: rewritten paths carry session tags Rust can decode |
| put aside | c entered a non-reentrant runtime y that is waiting and c is not on its chain; c waits for every call y has out | yes: y's outstanding calls are Rust's own state; reentrancy comes from capability `reentrant-sync` |
| stack order | c and then d entered the same runtime y, and d has started there (some call out of y names d); c waits for d because the stack is LIFO | yes: frames of a session are ordered. Had c returned first, its reply would reach Rust before any frame made under d |

The model has two graphs: `TrueSucc` from the runtimes' real stacks and queues (only for judging), and `RustSucc` from Rust's own state (what policy B can do).

`TrueSucc` is the criterion for `NoFalseCycle`, so it may contain only waits that really exist. The first version missed two kinds of real edges: a deferred call also waits for incoming calls that started later and are running on the stack; a call on the stack also waits for incoming calls stacked above it that have not called out yet. Both were added after review (`OnStack` and the last kind of edge); rerunning every configuration gave the same results. Any edge still missing can only make `NoFalseCycle` stricter (a real cycle judged as none, reported as a false positive); it cannot hide a false positive.

## Simplifications

- Every cross-runtime call is synchronous. Async calls do not block their caller and cannot cause a deadlock on their own, but an implementation of B must tell them apart; see "Open questions".
- Calls inside one runtime do not cross Rust and are not modelled.
- Service calls and function (callback) calls are one action: both are "forward from one session to another" in Rust. `await`, reference counts, cancellation and session close are not modelled.
- Rust threads are not a bounded resource: a forwarded call runs on the thread waiting for its chain or on a new thread. In the implementation a synchronous `RowService` forward occupies a worker of the session's executor; with a `current_thread` or small executor Rust itself can become the bottleneck. Not covered.
- Synchronous calls originated by Rust (a Rust plugin calling a runtime, not forwarding) are not modelled.
- Each activation makes one synchronous call by default; `policy-b-2node-seq` allows two in sequence.
- A runtime may start a top-level activation (timer, event) whenever idle, `MaxRoots` in total.
- Scale: 3 runtimes, chains ≤ 3 hops, ≤ 2 concurrent top-level calls; B was also run with 3 top-level calls, chains ≤ 4, and two calls per activation.

## Invariants

| Name | Meaning |
| --- | --- |
| no deadlock | TLC deadlock check: while calls are unfinished, some participant can act. The finished state stutters (`Done`) and is not a deadlock. Every behaviour of the model is finite (top-level calls, call ids, chain length and calls per activation are bounded, and every step except `Done` moves these counters or queues towards completion), so "no deadlock" is equivalent to "every call eventually completes"; no separate liveness property is needed |
| `RoutingSound` | A call run as part of a waiting chain (nested by a non-reentrant runtime, or handed to a waiting Rust thread) really belongs to that chain: a callback reaches the side that issued it, even with `node:1` on both sessions |
| `ChainRecognised` | A non-reentrant runtime never puts aside a call of its own chain |
| `NoFalseCycle` | Policy B answers `SyncWaitCycle` only if the real wait graph plus this call has a cycle |
| `NoLegitimateRefused` | Policy A refuses no legitimate call. **Legitimate**: forwarding it closes no cycle in the real wait graph |
| `NeverCycle` | Only to confirm B's check fires (expected to be violated) |

## Results

TLC 2.19 (tla2tools v1.7.4), Temurin 21, 16 cores; `MaxRoots = 2`, `MaxDepth = 3`, `MaxCalls = 1` unless noted.

| Configuration | Runtimes (`n` non-reentrant, `p` reentrant) | Result | Distinct states | Time |
| --- | --- | --- | --- | --- |
| `current-1node-1python` | n, p | pass | 606 | <1 s |
| `current-2node` | n1, n2, p | **deadlock** (counterexample 1) | 1,006 | <1 s |
| `current-1node` | n, p1, p2 | **deadlock** (counterexample 2) | 8,908 | 1 s |
| `current-reentrant` | p1, p2, p3 | pass | 88,824 | <1 s |
| `current-norebase` | n1, n2, p, no `path` rewriting | **`RoutingSound` violated** (counterexample 3) | 268 | <1 s |
| `policy-a-1node` | n, p1, p2 (the combination A intends) | **deadlock** (as counterexample 2) | 9,706 | <1 s |
| `policy-a-2node` | n1, n2, p | **deadlock** (shape of counterexample 2) | 3,066 | <1 s |
| `policy-a-refusals` | n1, n2, p | **`NoLegitimateRefused` violated** (counterexample 4) | 93 | <1 s |
| `policy-bchain-2node` | n1, n2, p | **deadlock** (as counterexample 2) | 5,798 | 1 s |
| `policy-b-2node` | n1, n2, p | pass | 37,311 | 1 s |
| `policy-b-1node` | n, p1, p2 | pass | 55,327 | 1 s |
| `policy-b-nonreentrant` | n1, n2, n3 | pass | 27,000 | 1 s |
| `policy-b-2node-depth4` | n1, n2, p; chains ≤ 4 | pass | 149,659 | 1 s |
| `policy-b-2node-3roots` | n1, n2, p; 3 top-level | pass | 1,190,945 | 8 s |
| `policy-b-1node-3roots` | n, p1, p2; 3 top-level | pass | 2,468,607 | 16 s |
| `policy-b-2node-seq` | n1, n2, p; 2 calls per activation, chains ≤ 2 | pass | 2,549,258 | 19 s |
| `policy-b-acts` | n1, n2, p | `NeverCycle` violated (expected: B fires) | 408 | <1 s |

"Pass" means exhaustive: no deadlock, all listed invariants hold; state counts on these rows are exact. On deadlock and violation rows TLC stops at the first counterexample, so the count depends on the order of the multi-threaded search and varies between runs (for example 977 or 984 for `current-2node`); treat it as indicative. `current-1node-1python` backs the statement in §9 of the multi-language design that one Node and one Python runtime do not deadlock. With chains ≤ 3, `policy-b-2node-seq` passed 44 million states in 10 minutes and was still growing, so it runs with chains ≤ 2.

### Counterexamples

1. **Two Node runtimes calling each other** (`current-2node`). n1 synchronously calls a service of n2 while n2 synchronously calls a service of n1. Neither call is on the other's chain; each is put aside; both wait forever. The case §9 already knows.
2. **Stack order on a reentrant runtime** (`current-1node`; A and Bchain alike).
   1. n synchronously calls service `s1` of p2;
   2. `s1` synchronously calls p1 (anything that takes a moment);
   3. p1 itself (a timer, say) synchronously calls service `s2` of p2. p2 is waiting and reentrant, so it runs `s2` nested above `s1`;
   4. `s2` synchronously calls a service of n. n is waiting for `s1`; the call is not on its chain and is put aside;
   5. `s1`'s call has returned, but `s2` is on top, so `s1` waits for `s2`; `s2` waits for n; n waits for `s1`. Deadlock.

   So "all leaf runtimes reentrant" does not remove deadlocks by itself: reentrancy turns "put aside" into "buried under", and one non-reentrant runtime is enough to close a cycle. It happens with a single Cordis runtime.
3. **No `path` rewriting** (`current-norebase`). n1 and n2 each wait for their own `node:1`; n1's call reaches n2 with `path = ["node:1"]`, and n2 takes it for a call on its own `node:1` chain and runs it nested. This is what today's `rebase` prevents; with rewriting on, `RoutingSound` holds in every configuration.
4. **A refuses a call that cannot deadlock** (`policy-a-refusals`). n1 synchronously calls n2 while n2 is idle and nothing else runs; A refuses it.

## Recommendation: policy B (with stack order)

Adopted by the maintainer (written into §9 of the [multi-language design](../../docs/design-multilang-runtimes-2026-10-03.en.md); implementation in #228).

- **A is not enough**: the combination A intends (one Cordis runtime, everything else reentrant) still deadlocks (counterexample 2); A refuses calls that cannot deadlock (counterexample 4); and it requires every future language runtime to be reentrant, pushing "do not hold a lock across a call into rutis" onto every plugin author.
- **B removes the deadlocks within the model, without false positives**: every B configuration (mixed, all non-reentrant, larger scales) has no deadlock and `NoFalseCycle` holds. Only the call that would close a cycle is refused; its caller gets `SyncWaitCycle` and returns, and everything else continues.
- **B uses only what Rust already has**: its forwarding records, rewritten `path`s (with session tags), frame order within each session, and whether each runtime is reentrant. Runtimes need not report their stacks; no existing wire field changes.
- **The stack-order edges are required**: chain and put-aside edges alone (`Bchain`) miss counterexample 2.
- A new language runtime may be reentrant or not, as long as it says which.

What an implementation of B must provide (the model's assumptions):

1. A host-wide table of forwarded synchronous calls that have not returned (source session and id, target session and id, decoded chain);
2. Returns recorded in frame-reading order: within a session, a return read earlier leaves the table before a call read later is checked;
3. A check before every synchronous forward; on a cycle, do not forward, answer `SyncWaitCycle` naming the calls on the cycle;
4. Reliable reentrancy per runtime: `reentrant-sync` in the endpoint-format handshake; the compat format has no capabilities, so the runtime plugin's configuration says it (Node: no);
5. Only **synchronous** waits in the graph (see below).

## Open questions

- **Telling sync from async calls (decided)**: today's `invoke`/`call` frames do not say whether the caller waits synchronously, and counting an async call as a wait gives false errors. The maintainer decided to add a field to call frames marking that the caller waits synchronously; only calls carrying it count in the wait graph (#228, which also settles compatibility). The model assumes this field exists: every modelled call is synchronous and carries it, so Rust knows directly which calls are synchronous waits. Async calls are not modelled.
- **Rust-originated synchronous calls** and **bounded Rust executors** (`current_thread`) can put Rust itself on a cycle; not covered.
- **Cost of B**: a reachability check over the global table on every synchronous forward. The table holds the synchronous calls in progress, usually few; to be measured.
- A caller that retries immediately after `SyncWaitCycle` may close the same cycle again; back-off is the caller's business and not modelled.

## How to run

Needs Java 11+ and `tla2tools.jar` ([v1.7.4](https://github.com/tlaplus/tlaplus/releases/tag/v1.7.4), sha256 `936a262061c914694dfd669a543be24573c45d5aa0ff20a8b96b23d01e050e88`). Do not commit the jar.

```sh
mise install java@temurin-21
curl -sSLo /tmp/tla2tools.jar https://github.com/tlaplus/tlaplus/releases/download/v1.7.4/tla2tools.jar
TLA2TOOLS=/tmp/tla2tools.jar mise exec java@temurin-21 -- specs/cross-runtime-sync/run.sh                  # all, about 1 minute
TLA2TOOLS=/tmp/tla2tools.jar mise exec java@temurin-21 -- specs/cross-runtime-sync/run.sh policy-b-2node   # one
```

`run.sh` compares each outcome with `expected.txt` and exits 0 when all match; logs and counterexamples go to `TLC_OUT` (a temporary directory by default). Without the script, from this directory:

```sh
java -cp /tmp/tla2tools.jar tlc2.TLC -workers auto -config current-1node.cfg CrossRuntimeSync.tla
```

## Maintenance

When §5 or §9 of the multi-language design or §7 of M1 change, or when `rpc.rs`, `session.mjs` or `peer.py` change which calls run during a synchronous wait, update this model and `expected.txt` (standard Q11.2.3).
