# Quality control step two (0.10): cross-process and cross-platform P0s (plan)

[中文](design-quality-step2-2026-10-11.md)

Status: plan, under review. Date: 2026-10-11. Base: `main` `ebf0711` (0.9.1).
Related: [#183](https://github.com/arcships/rutis/issues/183) step two. Based on: [quality standard](quality-standard.en.md) ("the standard"); [quality status](quality-status.en.md) ("status") §4 risks, §9.2 plan.

This document schedules the 12 issues left in step two together: what each one does and does not do, what it depends on, which PRs it splits into, and what the maintainer needs to decide first. Details of each PR (test names, assertions, output formats) are written in that PR; the ones that need a design of their own (S4, S5, #178) only get scope and key choices here.

Out of scope: the step-one leftover #265 (the CI `e2e` job; this plan depends on it); step three (0.11); #185 and "resource limits on references and calls" (second half of #178 item 5), which wait for design.

## 1. Conclusions

| # | Conclusion | § |
| --- | --- | --- |
| C1 | 12 items, about 21 PRs, in three waves: first the foundations others depend on (registry skeleton, E2E residue on Windows and macOS, #265), then unit / contract tests for the protocol and the core, last the three black-box scenarios | 9 |
| C2 | **S3 and S4 rely on editing the configuration while running, but `rutis-host run` does not watch `rutis.json`, and a runtime killed with `kill -9` is never started again.** Proposal: don't add config watching to `run` in 0.10; S3 recovers by restarting the host; S4 uses a fixture Rust host that re-reads its configuration on a stdin command (decision Q1) | 3 |
| C3 | **There is no mechanism behind S5's "revoke the token"** (the identity is read from environment variables once, at start). Proposal: rewrite it as "after B restarts with another token, A's reconnect is refused with a readable category"; no revocation feature (decision Q2) | 3.3 |
| C4 | The E2E harness has a gap to close first: `rutis-host`'s runtime processes each get a process group of their own, so **on macOS a runtime left behind after its host died is not found** (Linux finds it through the subreaper). Fixed in the same PR as #232's Windows Job Object | 3.4 |
| C5 | #178 splits into 5 PRs: fault injection and links; boundary rules 3/4/5/7; SDK alignment; negative TLS; cross-language value round-trip property tests. **There are 8 boundary rules** (not 7); 1, 2, 6 and 8 already have tests | 4 |
| C6 | Value round-trips use `proptest` (new dev dependency), generating values only on the Rust side and passing them through the existing `conformance-session` echo fixtures for Node, Python, Go and Bun; the comparison rules come from the value conversion table in `guide/plugin-api.md` | 4.5 |
| C7 | #176 only splits `cfg(unix)` and adds the missing cells, no generated matrix; the matrix report is a hand-written table in `docs/ci.md` | 5 |
| C8 | #206 doesn't compare the two sides with each other; the testing tool and the real runtime are each compared **against the same expectations file**. The Node `conformance-weather` fixture cannot be loaded by the testing tool today and gets a default export first | 6 |
| C9 | #207: none of the persisted formats has a format version field. Don't add one; keep a sample per version, back-filled from the 0.8.0 and 0.9.0 tags; one more step in `docs/release.md` | 7 |
| C10 | #181 lands its skeleton and numbering first, so #177's and #178's tests cite numbers directly; at the 0.10 release the registry is the baseline, and the Q9.4.4 release gate applies from 0.11 (decision Q3) | 8.2 |

## 2. Remaining items and the risks they cover

| issue | Content | Risks (status §4) | Level | PRs |
| --- | --- | --- | --- | --- |
| #187 | S3 cross-language composition and crashes | C3, C4, B10 | P0 | 1 |
| #188 | S4 implementation migration | G2, G3, G5 | P1 | 1 (with the fixture host) |
| #189 | S5 two host processes, loopback | E11, E1, E6 | P1 / P0 | 1 |
| #178 | Missing protocol cases, fault injection, negative TLS, SDK alignment, value round-trips | C4, E1, E6, E8, E9, P3, P4, P6, P7, P8, F3 | P0 / P1 | 5 |
| #176 | Windows column and missing cells of the conformance suites | C10 | P0 | 2 |
| #206 | Testing tools agree with real runtimes | A1 | P0 | 1 |
| #207 | Persisted formats stay readable | J3 | P0 | 1 |
| #177 | Core guarantees | K3–K8 | P0 / P1 | 2 |
| #181 | Guarantee registry | all | — | 1 (then updated by each PR) |
| #231 | Real clocks in other crates and languages (3 of 4 left) | K10 | P0 | 3 |
| #232 | E2E residue checks on Windows | B3, E2E part of C10 | P0 | 1 |
| #266 | Remaining install-smoke cells | B1 | P0 | 2 |

## 3. The three black-box scenarios (#187, #188, #189)

### 3.1 Facts checked first (in the code)

| Fact | Where |
| --- | --- |
| `run` reads the configuration once, loads, follows the status, then only waits for a signal; it doesn't watch `rutis.json` | `crates/rutis-host/src/main.rs` `run` |
| Only `dev` polls, and it watches the project's sources, not the configuration | `main.rs` `dev` |
| A local runtime is never started again; its link doesn't reconnect; when its process dies the runtime is `Down`, its service is withdrawn and its rows go back to Pending (status line `waiting for its services`); rows in other runtimes are unaffected | `rutis-bridge/src/runtime/local.rs`, `runtime/plugin.rs`; `rutis-loader/tests/runtime_rows.rs`, `multilang.rs` |
| Recovery needs `FiberView::restart` (or Go's `GoRuntimesHandle::restart`); `run` calls neither | `docs/guide/rust-host.en.md` |
| The requirements list "automatic restart, reconnect recovery" as not done | `requirements-protocol-plugins.en.md` §8 |
| The identity is a `StaticIdentity` built from environment variables at start; nothing re-checks the token of an established session | `rutis-host/src/host.rs` |
| Status lines don't show link state (waiting, retrying, refused) or the address a listener actually bound | `rutis-host/src/status.rs` |
| No fixture Rust host exists; `rutis_host::host::Host::start` is usable as a library (no semver promise), and the loader can register Rust rows with `Builtins::register` | `rutis-host/src/lib.rs`, `rutis-loader/src/resolver.rs` |
| `cross_language.rs` only kills the host; it never kills a runtime or compares other runtimes' pids; probes don't report pids | `tests/e2e/tests/cross_language.rs` |

### 3.2 S3 cross-language composition and crashes (#187)

One `rutis.json`: a Python row `store`; a TS row `report` depending on `store`; a TS probe depending on `report`; a Bun row `audit` depending on `report` (the Bun variant from the #200 review).

| # | Action | Assertions |
| --- | --- | --- |
| 1 | `rutis-host run`; wait for every row `running` and the probe `started` | the probe's call to `report` returns a result that went through `store` |
| 2 | While a call is in progress (a `store` method blocked on a gate file), `kill -9` the Python runtime | the call in progress fails explicitly (C4); `store`, `report`, `audit` show `waiting for its services`; later calls fail explicitly, not hang; Node and Bun runtime pids unchanged (C3) |
| 3 | Ctrl-C; immediately restart the host with the same configuration (B10: sockets and ports released) | exit code 0; in the new host every row is `running` and calls work again |
| 4 | Ctrl-C | residue check passes |

- The probe can't report runtime pids (it runs inside the Node runtime); the harness finds the runtimes from the process tree instead (the residue code in 3.4 provides "list a host's descendants").
- The issue's "a config edit removes the Python row and the Python process exits" moves to S4, where the host can change its configuration.
- Platforms: Linux and macOS; Windows after #232, with the Bun row skipped there and the reason written down (Bun design §10).

### 3.3 S5 two host processes, loopback (#189)

Node B: `python -m rutis listen:ws://127.0.0.1:<port>/rutis …` (per `guide/nodes.en.md`, `rutis[network]`), providing `infer`; each execution prints a line for `Host::count`. Host A: `rutis-host run` with B declared in `runtimes.remote` and a `gpu:` row using `infer`.

**Network loss comes from a TCP proxy in the harness**: A connects to the proxy, the proxy to B; `cut()` drops every connection and refuses new ones, `restore()` undoes it. That keeps "network down" (steps 2–3) apart from "B restarts" (step 4), with no containers.

| # | Action | Assertions |
| --- | --- | --- |
| 1 | Start B, then A | `infer` succeeds |
| 2 | `cut()` during a call | the call fails explicitly; B's execution count for it = 1 (E1, no automatic retry); A's `gpu:` row waits |
| 3 | `restore()` | after reconnecting the row is `running` again and calls succeed |
| 4 | `kill -9` B, start B again | A gets a new lease, calls succeed |
| 5 | Restart B with another token | A's reconnect is refused; A's output shows a readable refusal category; the token value appears in no output (B7) |
| 6 | Switch to wss: B's certificate names a host other than `localhost`; another run with an expired certificate | A's connection is refused (E6 between two processes) |

- The issue's "revoke A's token" has no mechanism behind it and is rewritten as step 5 (decision Q2).
- A's link state isn't in the status lines; it can only be judged from row states and A's error output. If the review wants it, a separate issue adds link state to the status lines (not in this PR).
- The listener's address isn't printed, so the harness reserves a port with `port()` instead of using `:0`.
- The nightly version with containers and netem (E12) is in step three.

### 3.4 What the E2E harness needs first (in #232's PR)

| Addition | Why |
| --- | --- |
| macOS residue: the host starts with `RUTIS_E2E_SCENARIO=<id>` in its environment and the residue check finds processes by that variable (`ps -E` / `ps eww`) instead of by process group | runtimes have process groups of their own; after the host dies launchd adopts them and a group lookup misses them. Linux already finds them through the subreaper; it switches to the variable too so both platforms agree |
| Windows: each host in a Job Object (started with `CREATE_SUSPENDED`, assigned to the Job, then resumed, so it can't start children before the assignment); the Job is closed on Drop; the residue check uses the process list from `QueryInformationJobObject` | #232 as written |
| Windows Ctrl-C: `GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT, pid)` (the host is already its own process group and `stop.rs` handles Ctrl-Break) | Windows stop checks for S2, S3 and #266 |
| `Scenario::descendants(host)`: a host's descendant processes and their command lines | S3 step 2 compares pids |
| `Scenario::proxy(target) -> Proxy` (`cut()`, `restore()`), with the proxy's connections included in the residue check | S5 |

The comment at `cross_language.rs:107` ("`run` has no signal handling yet") is out of date and gets fixed on the way.

### 3.5 S4 implementation migration (#188)

Host A is a fixture Rust host `tests/e2e/fixture-host` (a bin crate in the workspace, not published): it reads `rutis.json` through `rutis_host::host::Host::start` and also registers a built-in `calendar` row implemented in Rust; on a `reload` line on stdin it re-reads `rutis.json`, applies the difference as one loader edit and prints `applied` or `rejected: <reason>`. The harness itself still depends on no rutis crate (standard Q7.6); the fixture host is on the side under test.

| Stage | A's configuration | Assertions |
| --- | --- | --- |
| 1 | `planner` (TS) + `calendar` (Python) | results come from Python |
| 2 | Remove the Python row; add a link to node B (`rutis-host run`, WebSocket + token) importing B's `calendar` | results come from B; A's Python runtime exits (former S3 step 4) |
| 3 | Remove the link; switch to the built-in Rust `calendar` | results come from Rust; the connection to B is closed |
| Throughout | `planner`'s row unchanged | A's pid unchanged (G5); the probe keeps calling during each switch and every call either succeeds or fails explicitly (G2); nothing of the old implementation remains after each switch (G3) |

A stdin command rather than watching the file: the scenario decides when the switch happens, independent of a polling interval (standard Q7.1). If decision Q1 chooses "`run` watches the configuration", stages 1 and 2 could use `rutis-host`, but stage 3 still needs a host that can register Rust rows.

## 4. Protocol layer (#178)

### 4.1 Fault injection and links (PR 1)

`channel::testing::fault(channel) -> (Channel, Faults)` already has `delay`, `half_open`, `drop_and_close` and `heal`, and is only used by its own tests in `transport/memory/tests.rs`. Wire it into sessions and links:

- a call in progress meets `drop_and_close`: it fails explicitly, the far end's execution count = 1 (E1);
- calls and cancellation under `delay`: the cancellation reaches the far end and its `AbortSignal` fires;
- new calls made while disconnected and reconnecting: they fail explicitly or succeed on the new session, never land on the old one;
- multi-hop (b → a → c) cancellation and release passed on hop by hop (P6, E9);
- N concurrent calls returning interleaved, each result matched to its call;
- large payloads (near the limit) and deeply nested values at the session layer.

### 4.2 Boundary rules 3, 4, 5, 7 (PR 2)

`requirements-protocol-plugins.en.md` §5 has 8 rules; 1, 2, 6 and 8 have tests (`projection_lifecycle.rs`, `event_forwarding.rs`, `rpc_callbacks.rs`, `bun_runtime.rs` and others), 3, 4, 5 and 7 don't. One cross-process test per rule, with the same operations run on native Cordis for comparison (as `dsh_baseline.rs` does):

| Rule | Test |
| --- | --- |
| 3 event order | listeners on both sides; the other side's listeners run as one group at a fixed position |
| 4 middleware chains | a waterfall / bail chain with links on both sides; the other side takes part as a single link |
| 5 `internal/*` | one side emits `internal/*`, the other side doesn't receive it |
| 7 host services | `instanceof` fails on the Cordis side for a service rutis provides; calls through the interface work |

Test comments cite `guarantee: boundary/5.3` and so on.

### 4.3 SDK alignment (PR 3)

| Gap | Fix |
| --- | --- |
| Python's session tests have no release-count or grant-rollback cases | add the counterparts of Node's `session.test.mjs` cases |
| Neither SDK tests an unknown session protocol version; Python hard-codes it (`peer.py` `PROTOCOL = 2`) | each gets a handshake test with version 99, asserting an explicit refusal that says which end to upgrade (P8); Python keeps its constant, plus a test comparing it with Rust's `session::PROTOCOL` so one side can't change alone |
| `websocket_cross.rs` only has Node↔Rust | add a Python column (E8) |

### 4.4 Negative TLS (PR 4)

Rust already has "CA not trusted". Add expired certificate and hostname mismatch (generated with rcgen); on the Python side all three (`test_websocket.py` only has `ws://` today). The two-process version of E6 is S5 step 6.

### 4.5 Cross-language value round-trips (PR 5)

- `proptest` as a dev dependency of `rutis-bridge` (checked by `cargo deny`).
- Values generated only on the Rust side: strings (with newlines and non-BMP characters), integers (around ±2^53), floats (including -0; whether NaN and Infinity are generated follows the conversion table), null, booleans, arrays, objects, nesting depth.
- Through the existing `conformance-session` echo fixtures in `runtime_conformance.rs`, for Node, Python, Go and Bun; one runtime start each, 256 cases; on failure the seed and the shrunk value are printed (standard Q7.2).
- Comparison rules come from the value conversion table in `guide/plugin-api.en.md`; cases the table doesn't cover (such as integers above 2^53) make the test fail first, then we decide whether to change the implementation or document the behaviour.
- Release-count invariant (P4): after any sequence of grants and releases, each side's reference table size = the number not yet released.

Not done: generating values inside the SDKs (overlaps with the Rust-driven suite, status appendix); resource limits (waiting for design).

## 5. Windows column and missing cells of the conformance suites (#176)

Why whole files are `cfg(unix)`:

| File | What really needs Unix | Handling |
| --- | --- | --- |
| `runtime_conformance.rs` | `far_end` uses `UnixListener` | add a loopback-TCP `far_end`; the Unix socket column gets `cfg_attr(windows, ignore = "…")` |
| `session_matrix.rs` | `symlink`, `fd:3` assertions | Loopback and WebSocket columns run on Windows; Inherit and DialBack are skipped with a reason |
| `local_runtime.rs` | `sh -c`, `inherit_fd()` | per-test annotations |
| `bun_runtime.rs`, `bun_channel.rs` | Bun isn't declared supported on Windows | kept, reason written |
| `concurrent_launch.rs`, `local_stdio.rs`, `local_spawn.rs`, `local_plugin.rs`, the two soaks | `/proc`, `dup2`, `fd:3`, Unix sockets | kept, reason written |

Cells added: session contract × WebSocket and loopback; runtime contract × `Handover::Loopback` (on Unix too, not only indirectly through `RUTIS_LOCAL_HANDOVER`); node contract × Rust↔Rust over WebSocket.

Two PRs: splitting `cfg` with the Windows columns; the new cells. The matrix report is a hand-written table in `docs/ci.md` (implementation × channel × platform, skipped cells with their reason), updated whenever a cell changes. Also fixes the stale feature name in the `rutis_bridge::testing` module docs (it says `conformance`; the gate is `testing`).

## 6. Testing tools and real runtimes (#206)

- A new expectations file `conformance-weather.expected.json`: a list of calls (`today`, `later`, `each`, with callbacks, errors and value conversions) and the expected result of each.
- Real runtime side: `runtime::testing::runtime` in `session_matrix.rs` reads the same file.
- Testing tool side: one test each in `node/rutis/test` and `python/rutis/tests` loads the same fixture with `load()` (strict mode), makes the calls from the file and compares.
- Node's `conformance-weather.mjs` exports only `inject` and `apply`, without `provides` or a default export, so the testing tool's `load()` refuses it; it is rewritten with `definePlugin` first (the real runtime loads either form).
- Allowed differences (such as `crash()` not ending the process in the testing tool) are marked `allowed` one by one in the expectations file and written into `guide/plugin-api.en.md` (standard Q6.7.2).

Go's `rutistest` and Bun also have testing tools; not in this round, recorded in the issue as follow-ups (Bun's admission under Q6.8 is in step three).

## 7. Persisted formats (#207)

| Format | Today |
| --- | --- |
| `rutis.json` | no version field; unknown fields at the top level, in runtimes and in listen are errors; unknown row fields only warn |
| dsh user layer `<profile>/cordis.patch.yml` | an array of `Patch`, no version field; unknown keys of a `Patch` go into `overrides` |
| the loader's `Persist` | `Version` is an optimistic-concurrency token, not a format version; the loader writes no files itself |

Approach:

- Samples in `tests/fixtures/persisted/<version>/`: a `rutis.json` using as many fields as possible and a `cordis.patch.yml`, next to the normalized JSON of their parsed result (the expectation).
- Test: current code reads every version's samples and gets that version's expectation; samples no longer supported go in `rejected/`, asserting the error explains the migration.
- Back-fill samples from the 0.8.0, 0.9.0 and 0.9.1 tags (from that time's documentation examples and test configurations); one more per release from now on.
- `docs/release.md` step 1 gains "save this version's samples"; the PR template gains "Changed a persisted format?".
- No format version field (decision Q4).

## 8. Core and the registry

### 8.1 Core guarantees (#177)

Checked state:

| Item | Today | Approach |
| --- | --- | --- |
| D8 `Ctx::root()` errors without a tokio runtime | implemented, no test | unit test |
| typed plugin rules: `InjectUnsatisfied` not sent to the ErrorSink; `restart()` in the window returns Ok; a `get` read in an untyped plugin is not evidence; an untyped `require` failure returns to Pending | none tested | 4 tests in `typed_plugin.rs` |
| async waterfall callback panic reaches the caller | only the sync version tested | add the async one |
| D29 events not filtered by isolate | no regression test | add |
| instance ID exhaustion panics | the counter is a private static | crate-internal unit test that sets it near the limit |
| instance-subtrees §5: task count, capacity trend, resident sibling | only drop counts and entry counts checked | task count through `Handle::metrics().num_alive_tasks()`; capacity read in a crate-internal unit test |
| D11 `CordisError` not Clone | no assertion | compile_fail doctest (no new dependency) |
| D24 | `contract.rs:334` sorts before asserting, hiding delivery order | drop the sort, assert arrival order; seq monotonic across fibers |
| D31 | one probabilistic probe | per-emitter order with several emitters; the next emit is delivered after a panic; a same-type emit inside a listener goes to the end of the queue |
| D7 five-step unload | only the outcome asserted | an observable record per step, order asserted |
| dropped `restart` / `update` futures | not tested | one before the first poll, one after |
| docs | config-hot-update §7 says 16 and §11 says 14, actually 15; design-rust-port §5 lists a non-existent `agent_stop` | fix the docs |

Two PRs: events and ordering (D24, D31, D29, async waterfall panic); lifecycle and reclamation (the rest).

### 8.2 Guarantee registry (#181)

- `docs/guarantees.md` (Chinese and English), one row each: number, source, content, level, status (verified / partly / not verified / explicitly not guaranteed), tests.
- Numbers: `core/D1`–`core/D33` (D32 and D33 are in the config-hot-update document), `core/features.<section>`, `boundary/5.1`–`boundary/5.8`, `loader/<name>`, `instance/<name>`, `bridge/<name>`, `host/<name>`. About 100 rows, maintained by hand.
- Test comments cite `guarantee: <number>` (3 places in the whole repository today); one more item in the PR template.
- **The skeleton lands first** (numbering + D numbers + boundary rules); #177's and #178's PRs update the status of their rows in the same PR.
- Q9.4.4 ("not verified" core promises must be zero at every release): the registry is the baseline at 0.10 and the gate applies from 0.11 (decision Q3).

### 8.3 Real clocks (#231, 3 PRs left)

Continues the merged [design](design-test-clocks-2026-10-11.en.md): the loader (about 14 fixed waits, moving to `rutis-test-support`); other crates (dsh, dev, host; not the example project rutis-agent); other languages (Node, Python, Bun, Go; the last one says `Closes #231`). No new decisions needed.

## 9. Order and PRs

```
Wave 1 (foundations, in parallel)
  #265 CI e2e job (step-one leftover)
  #181 registry skeleton
  #232 harness: Windows Job Object + macOS residue + proxy + descendants
  #231 2/4 loader
Wave 2 (unit / contract, in parallel)
  #177 ×2   #178 ×5   #176 ×2   #206   #207   #231 3/4, 4/4
Wave 3 (black-box scenarios)
  #187 S3 → #189 S5 → #188 S4 (with the fixture host)
  after #232: S2 on Windows, #266 Windows stop checks
  #266 other cells (Bun row, websockets 15.0, Go row; Python guide examples wait for #259)
```

| Dependency | Why |
| --- | --- |
| scenarios → #265 | new E2E tests run in the `e2e` job, not squeezed into `rust` |
| S3, S5 → the harness part of #232 | macOS residue, `descendants`, proxy |
| S4 → S5 | reuses how S5 starts node B |
| #177, #178 → #181 skeleton | test comments use the registry's numbers |
| #206 and #176 | independent, but both touch `session_matrix.rs`; whichever merges second rebases |

CI changes needed (stated in each PR, added by a CI PR): `rutis[network]` in the `e2e` job (S5); a Windows E2E job (after #232); optionally, more `proptest` cases at nightly than the 256 on PRs.

## 10. Decisions for the maintainer

| # | Question | Proposal |
| --- | --- | --- |
| Q1 | S3 and S4 need configuration edits while running, which `run` doesn't support. Add config watching to `run`, or test with a fixture host? | Don't change `run` in 0.10: S3 recovers by restarting the host (which is the current promise), S4 uses the fixture host. Whether `run` should watch its configuration goes into a separate issue together with status §10 item 2 (B4) |
| Q2 | S5's "revoke the token" has no mechanism | rewrite as "after B restarts with another token, the reconnect is refused"; live revocation becomes a separate feature issue, not in 0.10 |
| Q3 | From which version does the registry's release gate (Q9.4.4) apply | baseline at 0.10, enforced from 0.11 |
| Q4 | Add a version field to the persisted formats? | No; the sample tests catch changes; add one under standard §9.3 when an incompatible change is really needed |
| Q5 | What to do when a value round-trip goes beyond the conversion table (large integers and so on) | first document the current behaviour in the table and register it as "explicitly not guaranteed"; changing the implementation is a separate issue |
| Q6 | After this plan is merged, does each item get its own PR, or does implementation continue in this PR? | One PR each (each stays small, and CI selects by change); this PR holds only the plan |
