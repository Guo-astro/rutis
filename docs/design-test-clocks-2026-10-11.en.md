# Tests outside `crates/rutis` that depend on the real clock (design)

[中文](design-test-clocks-2026-10-11.md)

Status: design, under review. Date: 2026-10-11. Base: `main` `0941ef4`. Issue: #231 (part of #183).
Basis: [quality standard](quality-standard.en.md) Q7.1–Q7.5; [quality status](quality-status.en.md) K10, P11; #220 (merged, changed `crates/rutis` only).

Scope: `crates/rutis-bridge`, `crates/rutis-loader`, `crates/rutis-host`, `crates/rutis-dsh`, `crates/rutis-dev`, `tests/e2e`, and the Node (`node/`), Bun (`bun/`), Python (`python/`) and Go (`go/`) tests. `crates/rutis` was done in #220 and is out of scope.
Out of scope: the example projects `crates/rutis-agent` and `crates/rutis-cli` (Appendix A.3 lists them for reference only); changes to `stress.yml` and `ci.yml` (none expected; only if Bun 1.4.0 does not support the timeout in `bunfig.toml` does #256 add `--timeout` to `bun test`); the root-cause fixes for the flaky failures #249 and #250 (they are races in product code, not test clocks; see §2).

## 1. Conclusions

| # | Conclusion | Section |
| --- | --- | --- |
| C1 | Every wait falls into one of three classes, by whether the test can control the clock: in-process tokio (paused clock possible), across threads or processes (only observable events), and timing that is the behaviour under test (injectable durations or a written tolerance) | 3 |
| C2 | A bridge session reads frames on its own OS thread (`session/rpc.rs` `Connection::open`) and has its own background runtime thread. So even on the memory transport, bridge and loader tests that go through a session **cannot** use the paused clock; they are class two | 3 |
| C3 | Add an unpublished workspace crate `rutis-test-support` for the shared wait helpers; #220's `still_pending` and `on_thread` move into it | 4 |
| C4 | Product code gets 2 small seams (S1, S4); no general `Clock` trait | 5 |
| C5 | Hang guards under 10 s (class C) all become `HANG_GUARD`, a mechanical replacement; classes A, B, E, F, G and U are examined one by one | 3 |
| C6 | 4 PRs: bridge, loader, the other crates, the other languages | 7 |

## 2. Inventory summary

Counted: test code only (`tests/`, `src/**/tests.rs`, `src/**/testing.rs`, `#[cfg(test)]` modules, JS/Python/Go test files and fixtures), plus product duration constants that tests depend on. Line numbers are as of `0941ef4`. The full list is in Appendix A.

Categories:

- **A** a fixed sleep used for synchronization (waiting for something to happen)
- **B** a fixed wait followed by a "this did not happen" assertion
- **C** a hang guard or polling deadline under 10 s
- **D** a hang guard of 10 s or more (as required; counted only)
- **E** timing is the behaviour under test (heartbeat, backoff, idle stop, cleanup deadline)
- **F** an assertion on elapsed time (upper or lower bound)
- **G** randomness without a printed seed
- **H** a sleep inside a fixture that is scenario content (a slow operation), not synchronization
- **W** a sleep that only widens a race window; the assertion holds whether or not the window is hit
- **U** a wait with no bound at all (the test never ends if it hangs)

| Where | A | B | C | D | E | F | G | H | W | U |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| rutis-bridge | 7 | 7 | 21 | 27 | 4 | 4 | 1 | 9 | 5 | 0 |
| rutis-loader | 0 | 14 | 4 | 26 | 5 | 0 | 0 | 5 | 0 | 1 |
| rutis-dsh | 0 | 0 | 5 | 3 | 2 | 0 | 0 | 1 | 0 | 0 |
| rutis-host | 0 | 0 | 0 | 9 | 4 | 0 | 0 | 0 | 0 | 0 |
| rutis-dev | 0 | 0 | 1 | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| tests/e2e | 0 | 0 | 0 | 3 | 0 | 1 | 0 | 5 | 0 | 0 |
| Node (incl. fixtures, baseline) | 1 | 2 | 2 | 1 | 5 | 1 | 0 | 11 | 0 | 3 |
| Bun | 3 | 0 | 1 | 0 | 0 | 0 | 0 | 2 | 0 | 0 |
| Python | 1 | 0 | 11 | 7 | 1 | 1 | 0 | 1 | 0 | 6 |
| Go | 1 | 1 | 7 | 3 | 2 | 0 | 0 | 0 | 0 | 7 |
| **Total** | **13** | **24** | **52** | **79** | **23** | **7** | **1** | **34** | **5** | **17** |

What needs changing is A, B, C, E, F, G, U and some of W: about 137 places (excluding the example project `rutis-agent`), 52 of them class C and mechanical. D and H stay (4 of the H items decide the test's outcome by their duration and are treated as A; see A.9).

Other findings:

- Nothing uses a paused clock, a virtual clock or a seed (no Rust `start_paused`, no Node `mock.timers`, no Bun fake timers, no Go `testing/synctest`, no Python fake clock).
- Wait helpers are written again in each file: 11 copies of `eventually` in bridge; in loader 7 of `eventually`, 4 of `until` and about 9 of `Probe::wait_for`; 5 copies of `soon` in agent; one each in dsh and host. Their bounds range from 5 s to 30 s.
- The riskiest places, the ones most likely to be flaking in CI already: `python/rutis/tests/test_peer.py:146` (a blocking read on the event-loop thread), `go/rutis/internal/peer/peer_test.go:116` and `:190`, `node/rutis-runtime/test/serve.test.mjs:26`, `bun/rutis-bun/test/fixtures/crashing-worker.ts:12`, `crates/rutis-loader/tests/runtime_rows.rs:1101`/`:1119` (the fixture exits after 300 ms and the test waits 300 ms), and the elapsed-time bounds of the three WebSocket heartbeat tests.
- Relation to the flaky failures already filed: #238 (multilang_go 20 s timeout) and #249 (go_rows never sees the stop) only say "timed out" and do not show where they were stuck; #233 and #250 are races between the exit status and the channel end. None of them come from a test using too short a time; they are races in product code. They all show that a failure message must include the state at the time (`eventually_with`, §4.2).

## 3. Rules

### 3.1 First decide the class

| Class | Test | Positive wait (something happens) | Negative assertion (something does not happen) |
| --- | --- | --- | --- |
| One: in-process tokio | The code under test runs only on the test's runtime: no child process, no socket, no `std::thread`, no `spawn_blocking` (or `on_thread` instead), timing via `tokio::time` | A synchronization point (channel, `watch`, `Notify`) | `start_paused` + `still_pending` (as in #220) |
| Two: across threads or processes | Everything else. Bridge sessions are in this class (C2) | Wait for an observable event; poll only when there is no event, with `eventually`, bounded at 10 s | See §3.2 |
| Three: timing is the behaviour | The assertion is "it happens after so long" | See §3.3 | See §3.3 |

### 3.2 Negative assertions in class two

In order of preference:

1. **Marker**: do a later thing on the same ordered path, wait for its effect, then assert that the earlier thing did not happen. Example, `node.rs:674`: after `announce(2)`, announce another service `marker`; once `marker` appears, assert that `clock` is absent. This needs the path to guarantee order (frames on one session are handled in order; one loader's reconciles run in order). Each rewrite states in its PR which ordering guarantee it relies on; if there is no written guarantee, use option 2 instead.
2. **Observable state**: make the code under test leave a record when it decides not to act, and wait for that record. Examples: the loader's `LoaderChanged` and `SelfDisposed` events; in the memory transport's unit tests (white box), a `#[cfg(test)]` count of blocked senders.
3. **Weak negative check**: when neither is possible, keep a fixed wait, but only if it can only miss (find nothing on a slow machine) and never fail falsely, the same behaviour has a deterministic test or marker elsewhere, and the code carries `// clock: weak-negative — <reason>`. Items marked "weak" in Appendix A are the ones expected to stay: currently 2 (`channel/testing.rs:150`, `runtime/spawn.rs:459`), both across processes; any found during implementation need a reason in the PR, at most 6 in total.

### 3.3 Class three: timing is the behaviour

1. If the test can set the duration, it sets it; if it cannot, add a small seam (§5).
2. If the code under test times itself on the test's runtime (`tokio::time`), use the paused clock. Replacing `std::time::Instant` with `tokio::time::Instant` in product code does not change behaviour (they are the same when the clock is not paused) and can be done directly.
3. If the timing runs on another thread, another runtime or another process (WebSocket heartbeats, runtime processes, rutis-host's cleanup deadline), use a **tolerance**:
   - for what must happen on time, the expected duration and the hang guard differ by at least 10×;
   - for what must not happen, wait for an observable count (for example at least N pings through the relay), not a fixed time;
   - **turn the bad outcome into a hang**: make the wait that should not happen one hour long; the test ending normally then proves there was no wait, and a hang is reported by the 10 s hang guard. Example, `local_loopback.rs:100`: set the token wait to one hour (S4); a successful dial proves the silent connection did not hold it up.
   - for what must time out, make the other side never finish (the fixture never completes its cleanup), rather than "a bit slower than the deadline".
4. Elapsed-time upper bounds (F) are removed and replaced by the above. Lower bounds (`>=`) can only miss and may stay, with the reason written down (`memory/tests.rs:28`).

### 3.4 Other rules

- **Hang guards**: one `HANG_GUARD` (10 s); waits that need longer (several runtimes starting cold) use `HANG_GUARD * k` and say why. Every unbounded wait (U) gets a bound. Bun's default of 5 s per test becomes ≥ 10 s (`bunfig.toml` `[test] timeout`, or `--timeout` on the CI command if 1.4.0 does not support it).
- **Polling**: only when there is no event; 10 ms interval; on timeout the panic message includes the current state (`eventually_with`), as #238 and #249 ask.
- **Randomness**: a test that uses randomness reads its seed from `RUTIS_SEED`, generates one if unset, and prints `seed: N (replay with RUTIS_SEED=N)`, as `interleave.rs` does. Randomness in product code (backoff jitter in `link.rs`) is set to `jitter: 0.0` in tests; product code does not change.
- **Fixture sleeps (H)** may stay. If the test's outcome depends on their duration (a fixture that crashes after 20 ms or exits after 300 ms), treat them as A: the fixture acts on a signal or after something happened.
- **Window-widening sleeps (W)** are allowed only if the assertion holds whether or not the window is hit, annotated `// clock: widen — <reason>`; where the state can be made observable cheaply (memory transport), wait for the state instead.

## 4. Shared helpers

### 4.1 Where

A new crate `crates/rutis-test-support` (`publish = false`, depends only on `tokio`), referenced from each crate's `[dev-dependencies]`. It does not depend on `rutis`, so `crates/rutis` can use it too without a cycle. `crates/rutis/tests/common/mod.rs` re-exports its functions, so #220's tests keep their calls.

Not in `rutis-bridge`'s `testing` feature: that feature is the node conformance suite (for outside implementations), a different purpose.

### 4.2 API

```rust
/// Only a hang guard, not a speed check (Q7.1.2).
pub const HANG_GUARD: Duration = Duration::from_secs(10);

/// `f` finishes within HANG_GUARD, or panic: "timed out: {what}".
pub async fn within<F: Future>(what: &str, f: F) -> F::Output;

/// Check every 10 ms until Some; panic on timeout.
pub async fn eventually<T>(what: &str, check: impl FnMut() -> Option<T>) -> T;
/// The same; on timeout the panic message includes `describe()` (#238, #249).
pub async fn eventually_with<T>(what: &str, check: impl FnMut() -> Option<T>,
                                describe: impl FnOnce() -> String) -> T;
/// For std threads.
pub fn eventually_blocking<T>(what: &str, check: impl FnMut() -> Option<T>) -> T;
pub fn recv_within<T>(what: &str, rx: &std::sync::mpsc::Receiver<T>) -> T;


/// Moved from #220: "still waiting" on the paused clock; run a blocking function on its own thread.
pub async fn still_pending<F: Future + Unpin>(f: &mut F) -> bool;
pub fn on_thread<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> oneshot::Receiver<T>;
```

The per-file `eventually`, `until`, `soon` and `Probe::wait_for` are removed or call these. Helpers like `Probe::wait_for` that wait for a record to appear keep their own types and use `eventually_with` inside.

### 4.3 Other languages

One small file per language, not a package:

| Language | Where | Contents |
| --- | --- | --- |
| Node | `node/rutis-runtime/test/support.mjs` (reused by `node/rutis`) | `HANG_GUARD_MS = 10000`, `within(what, promise)`, `eventually(what, check)` |
| Bun | `bun/rutis-bun/test/support.ts` | The same; `bunfig.toml` sets the default timeout to ≥ 10 s |
| Python | `python/rutis/tests/support.py` | `HANG_GUARD = 10`, `within`, `eventually` (asyncio and thread versions) |
| Go | `go/rutis/internal/testwait` (also used by `rutistest`) | `HangGuard = 10 * time.Second`, `Within(t, what, ch)`, `Eventually(t, what, check)` |

Node 22+ has `mock.timers` and Bun has fake timers, but the heartbeats under test cross a socket or a process and fake timers do not reach the other side, so this design does not use them (§10 item 2).

## 5. Seams in product code

| # | Where | Change | Solves | Cost |
| --- | --- | --- | --- | --- |
| S1 | `crates/rutis-bridge/src/link.rs:11,256,361` | `Live::since` from `std::time::Instant` to `tokio::time::Instant` | The backoff reset after `stable` can be tested on the paused clock (only on the dial-failure path, which does not go through session threads) | None: the two are the same when not paused |
| S4 | `crates/rutis-bridge/src/transport/local/spawn.rs:337` | `TOKEN_WAIT` becomes a `LocalTransport` setting, default 10 s | `local_loopback.rs:100` uses "turn the bad outcome into a hang" and drops `elapsed < 5s` | One field |

Solved in the tests, without product changes:

- Backoff jitter in `link.rs` (G): integration tests' `quick()` sets `jitter: 0.0`.
- The idle stop in `go_rows.rs:358-371`: `idle` is already settable (`GoRuntimes::idle`); the test waits for the runtime's "stopped" state, bounded by `HANG_GUARD`, instead of asserting after a fixed 800 ms wait. The sweep interval (100 ms) stays.

Not done:

- **A general `Clock` trait**: it would have to cross bridge's session threads, the WebSocket transport's own runtime and child processes. That is a large change, and tolerances (§3.3) suffice there.
- **An injected clock for WebSocket heartbeats** (`transport/websocket/connection.rs:196-255`): heartbeats run on the transport's own multi-thread runtime (`websocket/mod.rs:129`), which the test's paused clock does not reach. Tolerance instead (§10 item 2).
- **rutis-host's `PROCESS_EXIT` (3 s) and `KILLED_EXIT` (5 s)**: the three `--shutdown-timeout 1` cases in `signals.rs` change their fixtures per §3.3 point 3 (the side that must time out never finishes); the constants stay.
- **`MANIFEST_TIMEOUT` (30 s)** and the loader's `generate_id`: no test depends on their duration or value.

## 6. Verifying there is no regression

1. **The rewritten tests still find the problem** (as in #220): for every B and E rewrite, the PR records one "temporarily break the code under test; the test fails at its new assertion" run. Example, `node.rs:200`: make `ExportPlugin` export without waiting for the far end's services; the test must fail at the assertion after the marker.
2. **Repeated runs**: each PR runs the affected test files 10 rounds locally, rotating `--test-threads` over 1, 2 and the core count; results go in the PR description.
3. **Nightly**: bridge and loader multi-process tests are already repeated every night by the `multiprocess` job in `stress.yml` (#245); after the series merges, watch it for new failures.

## 7. Steps

| PR | Content | Depends on |
| --- | --- | --- |
| 1 (this PR) | `rutis-test-support`; `crates/rutis/tests/common` re-exports it; bridge: the 11 copies of `eventually` replaced by the shared one, C bounds become `HANG_GUARD`; A, B, W, F (`node.rs:185` first, `:200`, `:556`, `:674`, `cancellation.rs`, `settle()` in `projection_lifecycle.rs` / `service_projection.rs`, `bun_runtime.rs:311`, the memory transport); E, G (heartbeat tolerance, S1, S4, `jitter: 0.0`) | — |
| 2 | Loader: the 14 B items, the unbounded loop in `bun_multilang.rs:313`, the fixture race at `runtime_rows.rs:994`, the idle stop in `go_rows.rs`; shared helpers | 1 |
| 3 | dsh, dev, host, e2e: the `signals.rs` fixtures; shared helpers | 1 |
| 4 | The Node, Bun, Python and Go suites and fixtures; Bun per-test timeout ≥ 10 s (`bunfig.toml`) | — |

## 8. Risks

| Risk | Mitigation |
| --- | --- |
| A rewritten negative assertion is weaker and tests nothing | §6 point 1: a "break it, it fails" record for each |
| The order a marker relies on is not actually guaranteed, so the rewrite adds a new flake | §3.2 point 1: name the ordering guarantee; without one, use option 2 or 3 |
| The paused clock does not advance, or advances early, around `spawn_blocking` or OS threads (#220 hit this) | Only for class one; not for bridge or loader tests that go through a session (C2); use `on_thread` |
| With bounds raised to 10 s a real hang fails later | Only affects time to failure, not time to pass |
| Test seams leak into the public API | Only S4 is a new setting, with a default; S1 changes no interface |
| Conflicts with parallel work: #245 (`stress.yml`), #249 / #250 (the same loader and bridge tests), #177 (typed dispatch tests) | This series does not touch `stress.yml`; fixes for #249 and #250 land first and this series rebases; PRs touching the same file reference each other |
| Go's `testing/synctest` needs Go 1.25 (`go.mod` says 1.24) | Not used by this design (§10 item 3) |

## 9. Acceptance criteria (automatable)

1. No `timeout(` / `recv_timeout(` / `settimeout(` / `WithTimeout(` with a literal under 10 s in the scope's test files (checked with `rg`, written into the PR description).
2. No elapsed-time upper bound of the form `elapsed() <`, `Date.now() - … <`, `time.monotonic() - … <` in the scope.
3. `rg 'fn eventually|fn until' crates tests` finds definitions only in `crates/rutis-test-support` (example projects aside).
4. Every A, B, F and U item of Appendix A is rewritten or marked weak with a reason (at most 6).
5. The Bun suite's per-test timeout is ≥ 10 s.
6. Each PR description has the "break it, it fails" records and the results of 10 repeated rounds.

## 10. Maintainer decisions (2026-10-11)

- Weak negative checks (§3.2 option 3) are allowed only in cross-process places with no ordering guarantee, annotated `// clock: weak-negative — <reason>`, at most 6.
- The four WebSocket heartbeat suites use tolerance: ping and timeout at least 10× apart, the "stays connected" side waits for a count of pings through the relay, elapsed bounds removed; no injected clock in the transports.
- Go stays on 1.24 for now; no `testing/synctest`.
- The shared helpers go in a new crate `rutis-test-support` (unpublished, a dev-dependency only; confirmed with `cargo publish --dry-run` during implementation); 4 PRs, this one first with `Refs #231`, the last with `Closes #231`; the example project `rutis-agent` is out of scope; no check script.

## Appendix A: full inventory

"Cat." uses the letters from §2. "Proc.": in = in-process tokio; thr = in-process but across OS threads (including bridge sessions); out = across processes or real sockets. "Weak" means expected to stay as a §3.2 option 3 check.

### A.1 rutis-bridge

| Where | What it does | Cat. | Proc. | Fix |
| --- | --- | --- | --- | --- |
| `tests/node.rs:185` | sleeps 200 ms for the import to finish, then reads the local service | A | thr | Wait for the far `ImportPlugin`'s state (import done) or an announcement marker, then assert the local clock still reads 7 |
| `tests/node.rs:200` | asserts nothing was imported after 100 ms | B | thr | Marker: the far end exports another provided service; once it appears, assert `clock` is absent |
| `tests/node.rs:556` | asserts after 200 ms that the row did not start (waits for injection) | B | thr | Wait for the loader to report the row Pending (row state), then assert `starts == 1` |
| `tests/node.rs:570` | `timeout(5s, pending)` | C | thr | `within` |
| `tests/node.rs:674` | asserts after 100 ms that the old announcement was dropped | B | thr | Marker: announce another service after `announce(2)` and wait for it |
| `tests/cancellation.rs:42` | a 100 ms timeout as "the call does not end by itself", also used to drop the call | B | out | Wait for the fixture to report `wait` started, then drop; "does not end by itself" is covered by the later `aborted` assertion |
| `tests/cancellation.rs:51-57`, `:97-101` | 50 × 10 ms polling | C | out | `eventually` |
| `tests/cancellation.rs:93` | after one poll, sleeps 100 ms so the reply arrives first | W | out | Keep and annotate, or wait for the connection's pending-reply count > 0 |
| `tests/projection_lifecycle.rs:94` `settle()` (9 calls) | yield + 100 ms of sleeps, then assert | A (:335 is B) | out | Positive ones become `eventually`; `:335` (publication denied) waits for the denial event or relies on the positive assertion after `:339` |
| `tests/projection_lifecycle.rs:217` | `timeout(5s, dispose)` | C | out | `within` |
| `tests/service_projection.rs:94` `settle()` (3 calls) | same | A | out | `eventually` |
| `tests/bun_runtime.rs:311` | sleeps 100 ms for `pass` to enter Bun's gate | A | out | `Gate` sets a `Signal` when called; the test waits for it |
| `tests/bun_runtime.rs:229` | asserts "nothing downloaded" took < 5 s | F | out | Drop the bound; use an unreachable registry via environment; keep "no node_modules/left-pad" |
| `tests/bun_runtime.rs:314` | 5 s | C | out | `within` |
| `tests/go_runtime.rs:215` | a 100 ms timeout used to drop the call | A | out | Wait for the fixture to report `wait` started, then drop |
| `tests/process_exit.rs:45`, `:146` | 2 s | C | out | `within` |
| `tests/rpc_callbacks.rs:514` | 2 s | C | out | `within` |
| `tests/event_forwarding.rs:91-96`, `:182-187` | 100 × 10 ms polling | C | out | `eventually` |
| `tests/websocket.rs:103,276,330,381,518,560` | `recv_timeout(5s)` | C | out | `recv_within` |
| `tests/websocket.rs:505-533` | heartbeat: ping 100 / timeout 400; sleeps 800 ms and asserts still connected; asserts disconnect took < 3 s | E, F | out | §3.3 tolerance: timeout 1 s; the relay counts pings, assert still connected after ≥ 15 pings passed; drop the elapsed bound |
| `tests/websocket_multihop.rs:148-170` | heartbeats still answered while the main thread waits 1.5 s in a sync call | E | out | Same: ping and timeout 10× apart, sync call ≥ 2 × timeout |
| `tests/local_loopback.rs:100-121` | a silent connection does not hold up the process: elapsed < 5 s | E, F | out | S4: token wait of one hour, drop the elapsed assertion |
| `tests/link.rs:29` | `quick()` handshake 2 s | C | thr | Handshake 10 s; `quick()` adds `jitter: 0.0` |
| `tests/memory_mux.rs:511`, `:591` | 5 s | C | thr | `recv_within` |
| `src/transport/memory/tests.rs:28` | delay fault: elapsed ≥ 50 ms | F (lower bound) | thr | Keep; note it is only a lower bound |
| `src/transport/memory/tests.rs:41` | nothing received within 100 ms while half-open | B | thr | Remove: the later "the first message received is `after`" already proves `lost` was not delivered |
| `src/transport/memory/tests.rs:78` | the send has not returned within 100 ms while the buffer is full | B | thr | A `#[cfg(test)]` blocked-sender count; wait for it to be 1 |
| `src/transport/memory/tests.rs:95` | sleeps 50 ms for two threads to block | W | thr | Same: wait for the blocked count |
| `src/channel/testing.rs:22` `PATIENCE` | 5 s | C | out | `HANG_GUARD` |
| `src/channel/testing.rs:88`, `:110`, `:137` | sleeps so the other side blocks or fills first | W | out | Keep and annotate (the conformance suite covers every channel; blocking cannot be observed uniformly) |
| `src/channel/testing.rs:150` | sleeps 20 ms "so the message leaves before the close" | A | out | Weak: whether `close` guarantees delivery of what was sent must be written in the channel contract; check each channel; where guaranteed, drop the sleep; otherwise annotate |
| `src/session/testing.rs:299` | a 50 ms timeout used to drop an async call | A | thr | Wait for the far end to report the call started |
| `src/session/testing.rs:302-307` | 100 × 20 ms polling | C | thr | `eventually_blocking` |
| `src/runtime/spawn.rs:459` | the channel does not end within 2 s (the process still runs) | B | out | Weak: no observable "waiting for the exit" event; keep and annotate; the positive part already asserts the exit status |
| `src/link.rs:540-550` | backoff unit test, seed from the system time | G | in | Assert properties that hold for any seed (each delay within range, never above the cap), not a specific sequence; no product change |
| Fixture sleeps (`rpc_callbacks.rs:160,192,446`, `event_forwarding.rs:49`, `node.rs:359`, `session/testing.rs:56`, `python_runtime.rs:32`, `bun_runtime.rs:30`, `websocket_multihop.rs:158`) | slow operations | H | — | Unchanged |
| 11 copies of `eventually` (`node.rs:24`, `link.rs:34`, `multihop.rs:26`, `websocket_link.rs:22`, `websocket_soak.rs:29`, `go_runtime.rs:72`, `bun_runtime.rs:75`, `python_runtime.rs:85`, `row_services.rs:77`, `src/runtime/testing.rs:48`, `src/testing.rs:61`) and others | ≥ 10 s | D | — | Replaced by the shared one in the first PR |
| Run durations of `local_soak.rs`, `websocket_soak.rs` | the duration is the soak's input | — | out | Unchanged |

### A.2 rutis-loader

| Where | What it does | Cat. | Proc. | Fix |
| --- | --- | --- | --- | --- |
| `tests/peer_rows.rs:178` | asserts after 100 ms that it started once | B | thr | Marker: add another row on the same host; once it starts, assert `starts == 1` |
| `tests/runtime_rows.rs:145`, `:271` | asserts after 100 ms | B | out | A marker row or the `LoaderChanged` event |
| `tests/runtime_rows.rs:618` | 200 ms | B | out | Same |
| `tests/runtime_rows.rs:1101`, `:1119` | 300 ms, racing the fixture's `setTimeout(quit, 300)` at `:994` | B (outcome depends on the fixture's duration) | out | The fixture exits on a message; the test waits for the process exit event |
| `tests/lifecycle.rs:227` | asserts after 50 ms that there is no `SelfDisposed` | B | in | Paused clock + `still_pending` (the best candidate) |
| `tests/multilang.rs:430` | 200 ms | B | out | Wait for `meta["inject"]` (as the file's other cases do) |
| `tests/multilang_go.rs:402` | 500 ms | B | out | Same |
| `tests/cordis_node.rs:193`, `:273`, `:298` | 300 ms each | B | out | Marker: assert after a later call on the same session returns |
| `tests/go_rows.rs:336` | 300 ms | B | out | Marker or runtime state |
| `tests/go_rows.rs:358-371` | idle 300 ms; sleeps 800 ms and asserts still running | E, B | out | "Not stopped while a row uses it" becomes a state check after a successful call; after removing the row, wait for the runtime's "stopped" state, bounded by `HANG_GUARD`; no product change |
| `tests/go_rows.rs:516` | a deliberate 1 ms timeout | E | out | Keep (the timeout itself is asserted; the other side never replies); annotate |
| `tests/bun_multilang.rs:313-320` | `loop { sleep(10ms) }` with no bound | U | out | `eventually` |
| `tests/instances.rs:390`, `tests/lifecycle.rs:154` | `eventually` 5 s | C | — | Shared helper |
| `tests/loader.rs:568`, `:592` | 5 s | C | — | `within` |
| `tests/leases.rs:138`, `:377`, `:400` | 600 / 400 ms stop delays (fixture) | E | out | The fixture is released by the test; the jitter noted at `:374`: `quick()` sets `jitter: 0.0` |
| `tests/fixtures/go/cmd/logger/main.go:34` | fixture timing | E | out | Same: released by the test |
| The other 4 fixture sleeps | slow operations | H | — | Unchanged |
| Wait helpers: 7 `eventually`, 4 `until`, ~9 `Probe::wait_for`, `multilang_go`'s `Fixture::until` (dumps state on timeout; the model for `eventually_with`) | 10–30 s | D | — | Replaced by the shared ones in the second PR |

### A.3 rutis-agent (example project, out of scope, for reference only)

| Where | What it does | Cat. | Proc. | Fix |
| --- | --- | --- | --- | --- |
| `tests/integration.rs:160`, `:171` | asserts still Pending after 30 ms | B | in | Paused clock + `still_pending` (check first that no OS thread is involved); or the file's existing `wait_state` (`:38`, a `watch` channel) |
| `tests/integration.rs:459` | asserts no listener left after 50 ms | B | in | Same |
| `src/tools/mod.rs:377` | asserts after 400 ms that a background child left no file | B | out | First confirm the process group is gone (`kill(-pgid, 0)` returns `ESRCH`), then assert no file |
| `src/tools/bash.rs:334-339`, `:356-363` | same | B | out | Same |
| `soon` (5 s): 5 copies, e.g. `tests/integration.rs:33`, `tests/unit_loop.rs:168`, `src/driver.rs:755` | 5 s | C | — | Shared helper |
| `src/tools/mod.rs:264` (2 s), `:315` (1 s), `:319` (3 s, against `CANCEL_JOIN_GRACE` 2 s), `:368` (2 s polling) | short bounds, compared with product constants | C, E | out | S5: the test sets the deadline; bounds become `HANG_GUARD` |
| `tests/minimal_tools.rs:114`, `:123` | elapsed < 5 s | F | out | Remove; replace with "after cancel the process group is gone" |
| 8 fixture sleeps | slow commands | H | — | Unchanged |

### A.4 rutis-dsh, rutis-host, rutis-dev, tests/e2e

| Where | What it does | Cat. | Fix |
| --- | --- | --- | --- |
| `crates/rutis-dsh/tests/profile_loader.rs:257`, `:267`, `:283` | 5 s polling | C | `eventually` |
| `crates/rutis-dsh/tests/stream.rs:86`, `tests/web.rs:127` | 5 s | C | `within` |
| `crates/rutis-dsh/src/profile/lock.rs:149`, `:154` | lock wait, watch interval (already configurable) | E | The test sets the durations, per §3.3 point 3 |
| `crates/rutis-host/tests/signals.rs:384`, `:413`, `:456` | `--shutdown-timeout 1` | E | Cases that must time out use fixtures that never finish their cleanup; cases that must finish in time use the default 10 s |
| `crates/rutis-host/src/main.rs:273` (400 ms), `src/status.rs:53` (200 ms) | product poll intervals | — | Unchanged (tests wait for results, not intervals) |
| `crates/rutis-dev/tests/channel.rs:97` | 5 s per line (real unix socket) | C | `HANG_GUARD` |
| `tests/e2e/src/residue.rs:356`, `:359` | a loose elapsed check | F | Only a hang guard |
| `tests/e2e/src/lib.rs:55` `hang_guard()` etc. | 30 s or `RUTIS_E2E_TIMEOUT` | D | Unchanged; e2e keeps its own (black box, separate crate) |

### A.5 Node (`node/`)

| Where | What it does | Cat. | Fix |
| --- | --- | --- | --- |
| `node/rutis-runtime/test/websocket.test.mjs:7` | `quick`: ping 50 / timeout 200 / handshake 2000 | E, C | ping and timeout 10× apart; handshake 10 s |
| `websocket.test.mjs:63` | the 1009 case fails falsely on a stall > 200 ms | E | Same |
| `websocket.test.mjs:95` | `setTimeout(500)` and asserts still connected past the timeout | E | Count pongs |
| `websocket.test.mjs:99-102` | elapsed < 2000 | F | Remove |
| `websocket.test.mjs` `closedWith` / `next()` | unbounded | U | `within` |
| `node/rutis-runtime/test/serve.test.mjs:26` | asserts after `setTimeout(100)` | B | Wait for the server socket's close event |
| `serve.test.mjs:56-62`, `handshake.test.mjs:20` | unbounded | U | `within` |
| `serve.test.mjs:66` | a 10 s race | D | Unchanged |
| `node/rutis-runtime/test/fixtures/rust-mount.test.mjs:21`, `:26` | 1 ms polling, 2 s bound; no user found in the repository | A, C | Delete once confirmed unused, otherwise `eventually` |
| `node/baseline/native.mjs:24` | `Promise.race(fibers, 500ms)` reports "unavailable" | B | Wait for the fiber's state event; or mark it as a benchmark script (not a test) and leave it out of scope |
| `fixtures/sync-heartbeat.mjs:7` (for `websocket_multihop.rs`), `rpc-client.mjs:12`, `cordis-node.mjs:14` (retry 50 / 500) | fixture timing | E | Adjusted with the matching Rust tests per §3.3 |
| The other 11 fixture sleeps | slow operations | H | Unchanged |

### A.6 Bun (`bun/rutis-bun`)

| Where | What it does | Cat. | Fix |
| --- | --- | --- | --- |
| `test/channel.test.ts:34` | `Bun.sleep(10)` to split a read in two | A | Write the second part after `one` is received |
| `test/client.test.ts:26` | `setTimeout(20)` before invoking | A | Invoke after `hello` |
| `test/fixtures/crashing-worker.ts:12` | crashes after 20 ms, racing the connect | A (outcome depends on the fixture's duration) | Crash after the connection is made (or on a command) |
| `test/fixtures/channel-relay.ts:15` (used by `crates/rutis-bridge/tests/bun_channel.rs:41`) | exits after 50 ms | A | Exit on channel close or once the data is sent |
| `bun test` default of 5 s per test | the only hang guard | C | ≥ 10 s (§3.4) |
| 2 fixture sleeps | slow operations | H | Unchanged |

### A.7 Python (`python/rutis/tests`)

| Where | What it does | Cat. | Fix |
| --- | --- | --- | --- |
| `test_peer.py:146` | `asyncio.sleep(0.05)`, then a **blocking** `lines.readline()` on the event-loop thread | A | `await asyncio.to_thread(lines.readline)` (as `:119` already does) |
| `test_peer.py:17`, `:134` | `settimeout(5)` | C | 10 s |
| `test_websocket.py:42,55,59,76,151,153,159,167,178` | 5 s gets / recvs; at `:153` and `:167` a timeout raises `TimeoutError` instead of the expected `ConnectionClosed` | C | `HANG_GUARD`; a clear failure message on timeout at `:153` and `:167` |
| `test_websocket.py:171` | `RUTIS_HEARTBEAT=100,400` | E | §3.3 tolerance |
| `test_websocket.py:182-185` | elapsed < 3 | F | Remove |
| `test_websocket.py:57,61,74,78,180,184` | unbounded `channel.recv()` | U | Add a bound |
| The rest (7 at ≥ 10 s, 1 fixture) | — | D, H | Unchanged |

### A.8 Go (`go/rutis`)

| Where | What it does | Cat. | Fix |
| --- | --- | --- | --- |
| `internal/peer/peer_test.go:42`, `:112`, `:271` | 5 s | C | `testwait.HangGuard` |
| `peer_test.go:105` | a 50 ms context timeout expecting `DeadlineExceeded` | E | The other side never replies; keep the short timeout and annotate (the timeout is what is tested) |
| `peer_test.go:116` | `Sleep(50ms)`, then asserts `host.Err() == nil` | B | Marker: one more round trip; assert after it succeeds |
| `peer_test.go:155-168` | GC polling every 10 ms, 5 s bound | C | 10 s |
| `peer_test.go:190` | `Sleep(50ms)`, then checks the notification order (which this cannot tell apart) | A | Compare the order after the second notification arrives |
| `peer_test.go:35` and others | unbounded | U | Add a bound |
| `listen_test.go:146`, `:184` | 5 s | C | 10 s |
| `listen_test.go:247` | `RUTIS_HEARTBEAT=20,150` (ticker in `listen.go:36-46,208-222`) | E | §3.3 tolerance |
| `listen_test.go:197,210,237,276`, `rutistest/rutistest.go:86` | unbounded | U | Add a bound |
| `rutistest/rutistest.go:207-221` `Service()` | waits 2 s and redundantly polls every 10 ms besides `l.changed` | C | Wait only on `l.changed`, bounded at 10 s |
| `rutistest.go:87`, `:231`, `:240` | ≥ 10 s | D | Unchanged |

### A.9 H items whose duration decides the outcome (treated as A)

`crates/rutis-loader/tests/runtime_rows.rs:994`, `bun/rutis-bun/test/fixtures/crashing-worker.ts:12`, `bun/rutis-bun/test/fixtures/channel-relay.ts:15`, `crates/rutis-loader/tests/leases.rs:138`.
