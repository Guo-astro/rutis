# Session tags in call chains that cross processes (design)

[中文](design-session-tags-2026-10-12.md)

Status: implemented (this PR). Date: 2026-10-12. Base: `main` `41c4c00`. Issue: #325 (found by the S4 scenario, #188).
Basis: [quality standard](quality-standard.en.md) Q6.2.8, Q6.13.2, Q9.1, Q9.2.2, Q9.2.3, §9.3; [multi-language M1 design](design-multilang-m1-2026-10-04.en.md) §7.2 (`rebase`); [multi-language runtimes design](design-multilang-runtimes-2026-10-03.en.md) §9 (sync-wait cycle detection, #228 policy B); [remote plugins design](design-remote-plugins-2026-10-03.en.md) §6.2.

## 1. Conclusions

| # | Conclusion | Section |
| --- | --- | --- |
| C1 | Cause: session tags (`s1`, `s2`, …) come from an in-process counter and are unique only within one process; call chains cross links to other processes unchanged, and another process takes the far side's `s1/…` for an entry of its own session `s1`, restoring it into a collision with its own call ids | 2 |
| C2 | An entry of a call chain must name "which call of which session" uniquely among **every process the chain passes**, and only the process owning that session restores it to a native call id | 3 |
| C3 | Option A is chosen: a tag is `s` + a per-process random nonce (15 decimal digits) + a counter. Neither the wire format nor the meaning of any frame changes; `rebase` and the wait-cycle check are unchanged | 4, 5 |
| C4 | No protocol version change (§9.3): tags are still `s[0-9]+`, which the origin checks of all four runtimes accept; with mixed versions, new tags never equal old ones | 5 |
| C5 | Existing verification missed it because every multi-hop test ran in one process (tags unique by construction), short of Q6.13.2 "verify between two independent host processes"; a two-process reproduction is added and the full S4 scenario is no longer `#[ignore]` | 6 |

## 2. Current state (base `41c4c00`)

- `crates/rutis-bridge/src/session/rpc.rs:495`: `static NEXT_TAG: AtomicU64 = AtomicU64::new(1)`; `:806` each session gets `tag: format!("s{}", NEXT_TAG.fetch_add(1, …))`. Every new process starts at `s1`.
- `rpc.rs:485` `rebase(path, from, to)`: untagged entries get `from`'s tag; entries tagged `to` lose the tag and return to `to`'s native ids; other tagged entries pass through. Callers: `rpc.rs:994` `forward`, `:1000` `forward_async` (`services.rs:297`, `:306`, `runtime/rows.rs:56`), `rpc/relay.rs:51` (a reference's origin), `:117` (relay forwarding).
- `rpc/waits.rs:135` `decode`: reads `s1/node:3` as "call `node:3` of session `s1`" for the wait-cycle check (`:149` `forward`). The wait table is host-wide (per process).
- The receiver's check: Rust `rpc.rs:1589`, Node `node/rutis-runtime/src/session.mjs:320` (and the same check in Python, Bun, Go): a call frame whose `path` contains its own `id` is invalid and **ends the whole session**.
- The format check on origin entries: Node `session.mjs:143-144`, Bun `bun/rutis-bun/src/session.ts:149-150`, Python `python/rutis/rutis/peer.py:51`, `:54`, Go `go/rutis/internal/peer/peer.go:40-41`: `^(node|rust|s[0-9]+/[a-z0-9-]+):[1-9][0-9]*$` (compat), `^(s[0-9]+/)?[a-z0-9-]+:[1-9][0-9]*$` (endpoint format). Nested tags such as `s1/s2/node:1` are explicitly refused by their tests (`node/rutis-runtime/test/session.test.mjs:92`, `python/rutis/tests/test_origin_ids.py:14`, `go/rutis/internal/peer/peer_test.go:367`).

What happens in #325 (two processes A and B, each opening its runtime session first and its link second, so their tags are numbered alike):

1. A's runtime calls its host (`node:6` in A's session `s1`, with `rust:7` sent by A's host earlier in the chain); the host forwards over the link: after `rebase` the chain is `["s1/rust:7", "s1/node:6"]`.
2. B receives it on the link (B's `s3`) and forwards to its runtime session (B's `s1`): `rebase(…, "s3", "s1")` takes `s1/rust:7` for B's own entry and restores `rust:7`.
3. The id B's host uses for this call into its runtime happens to be `rust:7` too: the frame `{"id":"rust:7","path":["rust:7","node:6","s3/a:3"]}` is invalid to the runtime, which ends the session.

When the ids differ it is still wrong: B takes A's calls for calls it is waiting on synchronously (a reverse call goes to the wrong thread), and the wait-cycle check (`decode`) counts A's calls as B's, which can report a false cycle or miss one.

## 3. What an entry must identify

A call chain (`path`) lets a callee recognise "this incoming call belongs to the call I am waiting on synchronously" (it runs nested on the waiting thread instead of deadlocking), and lets the host build the wait graph (chain edges). So:

- R1 An entry names (session, call id) uniquely among every process the chain passes; chains crossing any number of processes, and coming back any number of times, stay unambiguous.
- R2 Back at its own session it is restored to the native call id (runtimes know only their own native ids, `session.mjs:342` and the like).
- R3 Wait-cycle detection (#228 policy B) keeps working: each host builds its graph from its own sessions only; another process's entries must never decode to a session of this one.
- R4 Keep the wire format and the meaning of frames if at all possible; if they must change, it is a protocol version change under §9.3.

## 4. Options

| Option | How | Wire format | Assessment |
| --- | --- | --- | --- |
| **A Per-process random prefix** (chosen) | A tag is `s` + a 15-digit random decimal nonce drawn when the process starts + a counter. Entries cross processes unchanged; only the process the tag belongs to restores them | Unchanged (still `s[0-9]+/<id>`) | One change (how tags are made); `rebase`, `decode` and the runtimes are untouched. Uniqueness is probabilistic: two processes' nonces are equal with probability about 10⁻¹⁵; tags get longer (about 17 characters) |
| B Qualify by endpoint id | Entries become `<endpoint id>/s1/rust:7` or `s1@a/…` | **Changes**: the origin patterns of all four runtimes refuse it (several `/` segments, or a non-digit tag); compat sessions have no endpoint id; a host embedding rutis outside `rutis-host` need not have one | Deterministically unique, but a protocol version change with every SDK upgraded in step |
| C Nest at the link boundary | Tagged entries received from a link get one more level, the link session's tag (`s3/s1/rust:7`), removed when sent back | **Changes**: nested tags are refused by the runtimes (see 2); and the receiver cannot tell whether `s1/…` on the wire is "the far side's s1" or "my own s1 coming back": it needs yet another marker | A round trip must tell "yours" from "mine", exactly the information the wire format lacks |
| D Alias table at the link boundary | Each link keeps a (far tag ↔ local alias) table, translated on send and receive | Same shape, **different meaning**: both sides must agree whose namespace a tag on the wire is in; an older far end does not know the convention | Stateful (cleanup, reconnects); like C it must mark direction; does not work with mixed versions |

Limits of option A:

- The nonce comes from `getrandom` (already a dependency of rutis-bridge); without system randomness it falls back to a hash of the standard library's random `RandomState` keys, the process id and the time, which still differs between processes.
- Within a process tags still differ by the counter; the fixed-width 15-digit nonce comes first, so tags of two processes can be equal only if their nonces are.
- Older tags (counter only, `s1`, …) are at most a dozen digits or so and never equal new ones (16 digits or more): with mixed versions, #325 remains only between two old processes, and upgrading either side removes it.

## 5. Decision

Option A (`crates/rutis-bridge/src/session/rpc.rs` `next_tag`).

- Wire format, frame fields, handshake and capabilities are unchanged: not an incompatible change under §9.3, no protocol version change, and no change to the Node, Python, Bun or Go runtimes (they check tags only against `s[0-9]+`, which new tags satisfy).
- The rules of multi-party interaction are unchanged (Q6.2.8): the three rules of `rebase` and the three kinds of edges of the wait graph stay as they are. The model in [`specs/cross-runtime-sync`](../specs/cross-runtime-sync/README.md) treats every call as a distinct identity; this fix restores exactly that premise, so the model needs no new check. Wait cycles spanning processes remain outside detection (each host sees only its own table, as in #228).

Behaviours and levels (Q9.1.1):

| Behaviour | Level |
| --- | --- |
| A call chain crossing any number of processes over links, there and back any number of times: each process restores only its own sessions' entries to native call ids; another process's call is never taken for one of this process's (it does not end a session, reach the wrong waiting thread or enter this process's wait graph) | Contract behaviour (`bridge/call-chains-across-processes`) |
| The exact form of a tag (`s` + 15-digit nonce + counter) | Implementation detail (only "still matches `s[0-9]+`" is pinned, being a format constraint visible on the wire) |

Risk (Q4): impact high (the far runtime ends its whole session, every call through it fails, and a local runtime is not restarted); likelihood medium (two processes, each with a local runtime, a chain crossing a link: 3 runs out of 5 in S4) — P0. Controls: the deterministic two-process test runs in every CI run (rutis-bridge tests); the full S4 scenario runs in the `e2e` job.

Component type: Q6.2 cross-process protocol, Q6.13 distributed and network nodes. Q6.13.2 requires verification between two independent host processes: the new test is two processes.

Acceptance:

1. `crates/rutis-bridge/tests/session_tags.rs` fails on `main` (B's runtime session ends with `invalid invocation identity/path`) and passes with the fix;
2. `tests/e2e/tests/migration.rs::an_implementation_moves_from_python_to_a_node_to_rust_while_the_host_runs` is no longer `#[ignore]` and passes ≥ 20 times in a row locally;
3. tags match `s[0-9]+` and tags of one process share the nonce (`rpc/tests.rs::sessions_have_distinct_tags_and_forward_rebases_the_current_chain`);
4. the existing rutis-bridge and rutis-loader tests (`sync_cycles` included) pass unchanged.

## 6. Verification

- Reproduction (Q9.2.2): `session_tags.rs::a_call_chain_crossing_processes_keeps_each_process_s_calls_apart`. The test starts its own executable as two child processes a and b (fresh processes, so tag numbering is deterministic); each has an in-process "runtime" session (an endpoint-format Rust far end), and they are linked over TCP. a's runtime calls through the link into b's runtime synchronously, and b's runtime calls back through the link into a's runtime, which still waits for the first call. On `main` it fails every time, with the failure of #325.
- Why existing verification missed it (Q9.2.3): the multi-hop scenarios of `multihop.rs`, `websocket_multihop.rs`, `fault_links.rs` and rutis-loader's `sync_cycles.rs` all run every node in one test process, sharing one counter, so tags are unique by construction. What was missing is the class "a call chain crossing two independent processes" (Q6.13.2). That class is what is added: the two-process test is part of the rutis-bridge tests and runs whenever `rebase`, tags or relays change; the full S4 scenario covers the same point end to end.
