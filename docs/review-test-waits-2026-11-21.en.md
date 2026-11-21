# rutis: an audit of tests that wait by time instead of by event

- Repository: `/Users/eric8810/Code/rutis`, branch `test/1373-test-reduction`, commit `2d245d3a`
- Date: 2026-11-21
- Nature: read-only audit. No cargo / swift / go / gradle / bun / node / python test was run, and no tracked file was modified.

> Note: this report is based on the tree of commit `2d245d3a` (after the test merge of PR #1386); every `file:line` below refers to that commit.

---

## 1. Conclusion

**1. The repository already has the right rule; the problem is that it is not applied everywhere.**

`crates/rutis-test-support` states the rule explicitly: a timeout only guards against a hang (`HANG_GUARD`), it never stands in for an event. It provides `scaled`, `within`, `eventually`, `until`, `still_pending`. All six languages have an equivalent:

| Language | Canonical implementation |
| --- | --- |
| Rust | `crates/rutis-test-support/src/lib.rs:28` (`HANG_GUARD`), `:61` (`within`), `:67` (`eventually`), `:72` (`until`), `:52` (`scaled`) |
| Go | `go/rutis/internal/testwait/testwait.go:18` (`HangGuard`), `:24` (`Within`), `:51` (`Eventually`) |
| Node | `node/rutis-runtime/test/support.mjs:8` (`HANG_GUARD_MS`), `:15` (`within`), `:32` (`eventually`) |
| Bun | `bun/rutis-bun/test/support.ts:13`, `:23`, `:39` |
| Python | `python/rutis/tests/support.py:29` (`within`), `:38` (`eventually`), `:51` (`eventually_blocking`) |
| Swift | `swift/rutis/Sources/RutisTest/RutisTest.swift:147` (`settle`), `:200` (`started`) |

**2. The audit found 336 call sites that wait by time** (Rust 144, Swift 61, Kotlin 37, Go 16, Node/Bun 45, Python 33), plus 13 shell/workflow sleeps. Classification: **A 74, B 116, C 5, D 21, E 3**; the other 117 are fixture scenario content (H), product code (P), or the wait helpers themselves (S), and are not test waits.

**3. What actually flakes is category E: 9 findings** (3 from sleep call sites, 6 from process-global state). Five of them are amplified by this round's merge of test binaries:

- **Process environment mutated concurrently.** `RUTIS_SHUTDOWN_TIMEOUT` is set with `set_var` inside the `core` and `rows` suites, and `shutdown_deadline()` re-reads the environment on every call (`crates/rutis/src/fiber.rs:1344-1351`). Every other test in the same binary therefore unloads with a 0.3 s deadline.
- **`RUTIS_TOKEN_*` is not always taken under the same lock.** `crates/rutis-host/tests/dial/dial_conformance.rs:245` writes without the lock, while the same suite's `lease_e2e` / `swift_app` / `kotlin` enter `common::environment()` (`crates/rutis-host/tests/common/mod.rs:33-44`), which **removes** every `RUTIS_TOKEN_*`. Interleave the two and the test fails.
- **Two sleeps whose assertion depends on winning a race.** `still_waiting.rs:124` (50 ms to beat a 300 ms timer) and `unload_deadline.rs:49` (a listener must already be running after 50 ms).

**4. Category B is the largest group (116) but is not unusable.** Most are bounded poll loops: correct in intent, hand-rolled 27 times (the test tree still defines its own `eventually` / `until` / `wait_for` / `soon` in 27 places). These are mechanical replacements. The payoff is readability and failure messages: `eventually_with` reports the state at the time, which is exactly what #238 / #249 / #1003 complained was missing.

**5. By language, Rust carries the most risk** (parallel threads inside a binary, 18 merged suites, process-level env). Kotlin and Swift are mostly at risk from **duration upper bounds** (`< 100 ms`, `< 1000 ms`) and **fixed waits for a positive event**. Go, Python and Node/Bun are broadly healthy; their issues are scattered.

**6. `RUTIS_TEST_TIME_SCALE` scales hang guards only, not bare sleeps** (`crates/rutis-test-support/src/lib.rs:33-54`; CI defaults it to 3 on self-hosted runners, e.g. `.github/workflows/ci.yml:339`). So any pattern of "a bare sleep racing an unscaled fixed duration" only gets worse under load. That is the mechanism by which the two race-based E findings will reproduce in CI.

---

## 2. Scope and method

### 2.1 Scope

Included:

- Waits in all integration and unit tests: `crates/*/tests/**`, `crates/*/src/**/tests*.rs`, `crates/*/src/**/testing*.rs`, `tests/e2e/**`.
- Sleeps inside fixtures where the test outcome depends on them.
- The test suites and test tooling of all six language SDKs: Rust, Swift, Kotlin, Go, Node, Bun, Python.
- Sleep-based waits in `tools/**` and `.github/workflows/*.yml`.

Excluded:

- Sleeps in product code (e.g. retries in `swift/rutis/Sources/Rutis/Remote/App.swift`, the grace close in `go/rutis/internal/runtime/listen.go:573`, `kotlin/.../Peer.kt:630`).
- Code snippets in documentation.

### 2.2 Search terms

Every pattern requested was run, plus the global-state searches relevant to #1373:

```
tokio::time::sleep   std::thread::sleep   thread::sleep   Task.sleep   asyncAfter
time.Sleep           time.After           Thread.sleep    delay(       setTimeout
asyncio.sleep        time.sleep           eventually      until(       within(
within_blocking      HANG_GUARD           scaled(         Duration::from_millis
Duration::from_secs  start_paused         RUTIS_TEST_TIME_SCALE
running_processes    processes_ended      DialedSocket.live   set_var    set_current_dir
fixed ports (127.0.0.1:<port>, :<port>)   temp dirs (/tmp, temp_dir)
```

Hits (whole tree, excluding `node_modules` / `build` / `.build` / `.gradle`):

| Pattern | Hits | Note |
| --- | --- | --- |
| `tokio::time::sleep` | 173 | Rust tests and product code |
| `std::thread::sleep` | 61 | same |
| `Task\.sleep` | 51 | Swift |
| `asyncAfter` | 10 | Swift |
| `Thread\.sleep` | 34 | Kotlin |
| `delay(` | 23 | Kotlin |
| `time\.Sleep` | 14 | Go |
| `time\.After` | 8 | Go |
| `setTimeout` | 114 | Node/Bun (mostly product code and fixtures) |
| `asyncio\.sleep` | 54 | Python |
| `time\.sleep` | 17 | Python |
| `HANG_GUARD` | 283 | the sanctioned facility — a positive sign |
| `eventually` | 378 | positive sign |
| `within(` | 557 | positive sign |
| `until(` | 358 | positive sign |
| `scaled(` | 44 | positive sign |
| `start_paused` | 38 | paused clock — positive sign |

Adoption of the sanctioned facility is already broad: **113 test files** use `within` / `eventually` / `until`. Event-driven waiting is the mainstream style here.

### 2.3 What #1373 did to parallelism

- There are now **18 `tests/<suite>/main.rs`** suites. For example `crates/rutis/tests/core/main.rs` packs 34 modules — `unload_deadline`, `contract`, `events`, `interleave`, and others — into one binary.
- Rust's libtest **runs tests inside a binary on parallel threads by default**. On self-hosted runners CI sets `RUST_TEST_THREADS=$jobs` explicitly (`jobs = CPU/4`, minimum 2; `.github/workflows/ci.yml:2016-2023`).
- `stress.yml` rotates `--test-threads` through 1, 2 and the physical core count every three rounds (`.github/workflows/stress.yml:136-143`). **The core-count round is exactly the configuration in which category-E global state fires.**
- Swift's `RutisSelfTest` is a custom sequential runner (`swift/rutis/Tests/SelfTest/main.swift:1`: `// Runs every test of the registry, one after another`), so Swift's global-counter risk is lower than Rust's.
- Go runs tests sequentially within a package and packages in parallel; Kotlin sets no `maxParallelForks`; Python's unittest is sequential; Node's `node --test` runs files in parallel, one process each.
- Therefore **the "test moves from its own process into a shared process" change happened only to Rust's 18 suites**, and the five worst E findings are all there.

### 2.4 How items were classified

Every hit was read in context, and classified by *what the wait asserts*, not by how it is written. Three `sleep(100ms)` calls can land in three categories:

- it is followed by "X did not happen", and X must never happen → A (can only under-detect);
- it is followed by "X did not happen", but X is expected to happen later → B or E (a slow machine makes it fail);
- it merely lets a later action land inside a window, and the assertion holds either way → D;
- it measures steady state or elapsed time → C.

---

## 3. Categories A–E

| Cat | Definition | Typical shape here | Action |
| --- | --- | --- | --- |
| **A** | **Legitimate negative assertion**: a fixed wait proving something did *not* happen, where it must never happen, so the wait can only under-detect | `sleep(500ms)`, then assert the row is still `Down` | Keep; add `// clock: weak-negative — <reason>` per the repo convention |
| **B** | **Waiting for an event or state by fixed polling (or a fixed duration)** | `while !cond { sleep(10ms) }`; `sleep(50ms)` then assert an async result arrived | Wait for the event; where none exists, use the shared `eventually` / `until`, and report the state on timeout |
| **C** | **Waiting for performance steady state** | `drained()` (5 quiet windows), `settle()` (fds/bytes stop moving), `< 100 ms` upper bounds | Keep, but only in its own binary or run serially; widen or delete duration upper bounds |
| **D** | **Deliberately creating a race window by timing**: the assertion holds whether or not the window is hit; the sleep only gives a path a chance to run | `// clock: widen — ...`, `sleep(50µs)` to "amplify the overlap" | Keep; require that the assertion is independent of the window, and add `// clock: widen — <reason>` |
| **E** | **Real defect**: global state, shared counters, fixed ports or temp dirs, or an assertion whose result is decided by a race window | process-env `set_var`, exact equality on `running_processes()`, `DialedSocket.live` counters, 50 ms racing 300 ms | Must fix |

**A note on A.** "Legitimate" means it cannot cause a flake; it does not mean it is free. Three A items are expensive in wall-clock time: `plugin_limits.rs:192` (12 s), the three 1.2 s sleeps in `config/dev.rs`, and the three 3 s sleeps in `cross_version.rs`. That is a CI cost, not a stability problem.

---

## 4. Statistics by language

### 4.1 Raw hits and classification

| Language | Raw sites | A | B | C | D | E | Excluded: H fixtures | Excluded: P product | Excluded: S helpers |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Rust | 144 | 46 | 66 | 5 | 8 | 2 | 17 | 0 | 0 |
| Swift | 61 | 9 | 15 | 0 | 7 | 1 | 17 | 8 | 4 |
| Kotlin | 37 | 6 | 12 | 0 | 4 | 0 | 12 | 3 | 0 |
| Go | 16 | 5 | 2 | 0 | 0 | 0 | 7 | 1 | 1 |
| Node/Bun | 45 | 3 | 10 | 0 | 1 | 0 | 26 | 0 | 5 |
| Python | 33 | 5 | 11 | 0 | 1 | 0 | 13 | 1 | 2 |
| **Total** | **336** | **74** | **116** | **5** | **21** | **3** | **92** | **13** | **12** |

Check: 74 + 116 + 5 + 21 + 3 + 92 + 13 + 12 = 336.

### 4.2 Disposition

| Language | Must fix (E) | Should change (B + D) | Annotate only (A) | Keep (C + H + S) |
| --- | ---: | ---: | ---: | ---: |
| Rust | 2 (+6 non-sleep) | 74 | 46 | 22 |
| Swift | 1 | 22 | 9 | 21 |
| Kotlin | 0 | 16 | 6 | 12 |
| Go | 0 | 2 | 5 | 8 |
| Node/Bun | 0 | 11 | 3 | 31 |
| Python | 0 | 12 | 5 | 15 |
| **Total** | **3 (+6)** | **137** | **74** | **109** |

Product code (P, 13 sites) is not in this table, so each row sums to "raw sites − P".

### 4.3 tools / workflows

| Location | Count | Verdict |
| --- | ---: | --- |
| `.github/workflows/ci.yml` | 6 | retry backoff / retried deletion — keep |
| `.github/workflows/release.yml` | 3 | retry backoff + one bounded poll — keep |
| `tools/*.sh`, `tools/bench/**` | 5 | 2 signal-escalation graces (keep), 3 bounded polls (B — keep) |

---

## 5. Item-by-item inventory

**Risk** = risk of flaking under load. **Effort**: S < 15 min, M 15–60 min, L > 1 h.

### 5.1 Rust

#### A — legitimate negative assertions (keep, annotate)

| Location | Snippet | Why it is legitimate | Risk | Action | Effort |
| --- | --- | --- | --- | --- | --- |
| `crates/rutis/tests/core/load_window.rs:269` | `sleep(100ms); assert!(!returned…)` | the consumer is gated on a `release` channel, so the assertion is deterministic | low | add `// clock: weak-negative — consumer is gated` | S |
| `crates/rutis/tests/core/evicted_apply.rs:481` | same | same | low | same | S |
| `crates/rutis/tests/core/evicted_apply.rs:709,739` | `sleep(100ms); assert_eq!(state, Failed)` | asserts "no retry" — can only under-detect | low | annotate | S |
| `crates/rutis/tests/core/still_waiting.rs:66,113,132,139` | `sleep(300–500ms); assert_eq!(len, n)` | asserts "not reported again" | low | annotate | S |
| `crates/rutis/tests/core/events.rs:944` | `sleep(20ms); assert!(entries.is_empty())` | a failing filter never enters the table | low | annotate | S |
| `crates/rutis/tests/core/embedding_output.rs:124` | `sleep(200ms); assert_eq!(said.len(), 1)` | asserts it is said only once | low | annotate | S |
| `crates/rutis/tests/core/transient_release.rs:66` | `select! { sleep(deadline), last.changed() }` | event first, sleep only as a bound | low | already correct; keep | — |
| `crates/rutis-bridge/tests/lang/event_hub.rs:538,611,623` | `eventually(...); sleep(50ms); assert_eq!` | waits for the positive event first, then confirms no extra | low | annotate | S |
| `crates/rutis-bridge/tests/session/session_plugin.rs:249,424,1047` | `sleep(100–200ms); assert!(none)` | asserts nothing was shared / sent extra | low | annotate | S |
| `crates/rutis-bridge/tests/native/bun_runtime.rs:387` | `sleep(300ms); assert!(exit_status().is_none())` | asserts the process did not die | low | annotate | S |
| `crates/rutis-loader/tests/rows/event_close_admission.rs:195,245,417,446` | `sleep(100ms); assert_eq!(calls, 0)` | a closed listener never runs; ordering is guaranteed by the protocol | low | annotate | S |
| `crates/rutis-loader/tests/go/go_rows.rs:395,517,526,534,1253` | `sleep(300–500ms); assert_eq!(runs, n)` | asserts "no restart" | low | annotate | S |
| `crates/rutis-loader/tests/lang/swift_rows.rs:315` | `sleep(200ms); assert_eq!(row_state(probe), Active)` | asserts another row was untouched | low | annotate | S |
| `crates/rutis-loader/tests/rows/plugin_limits.rs:192` | `sleep(12s); assert_eq!(line, "wordcount: starting …")` | asserts it did not time out; **costs 12 s** | low | keep but shorten: assert `starting` first, then wait for a later event that must follow | M |
| `crates/rutis-loader/tests/go_stop.rs:139` | `sleep(DEADLINE*2); assert_eq!(generation, …)` | asserts no restart | low | annotate | S |
| `crates/rutis-loader/tests/runtimes/multilang.rs:720` | `sleep(500ms); assert!(no "in tromso")` | asserts isolation | low | annotate | S |
| `crates/rutis-host/tests/process/runtime_exit.rs:293,373` | `sleep(1500ms); assert!(Down / empty)` | asserts no restart | low | annotate | S |
| `crates/rutis-host/tests/config/unhandled.rs:220,262` | `sleep(600ms); assert_eq!(stderr, …)` | asserts no extra output | low | annotate | S |
| `crates/rutis-host/tests/config/dev.rs:442,812,854` | `sleep(1200ms)` ×3 | asserts no extra lines | low | annotate; could wait for the next known line instead | S |
| `crates/rutis-host/tests/config/rows_file.rs:107` | `sleep(1500ms)` | debounce window | low | annotate, or wait for the first reload event | S |
| `crates/rutis-agent/tests/integration.rs:153,164,423` | `sleep(30–50ms); assert_eq!(Pending / len)` | asserts not started / no residue | low | annotate | S |
| `tests/e2e/tests/install/cross_version.rs:665,724,933` | `sleep(3s)` ×3 | asserts "no second time" | low | annotate; e2e owns its process, so the risk really is low | S |
| `tests/e2e/tests/install/ios_app.rs:380` | `sleep(2s)` | a system-UI precondition (local only, not in CI) | low | keep | — |
| `node/rutis-runtime/test/websocket.test.mjs:168` | `sleep(500ms); assert.equal(closed,false)` | "not closed within ten timeouts" | low | annotate | S |
| `node/rutis-runtime/test/cordis-events.test.mjs:330` | `sleep(10ms); assert.deepEqual(reported, [])` | asserts no duplicate report | low | annotate | S |
| `node/rutis/test/testing-on.test.mjs:59` | `sleep(50ms); assert.deepEqual(asked, [])` | nothing delivered after unload | low | annotate | S |
| `python/rutis/tests/test_events_session.py:233,762` | `sleep(0.05)` | asserts still pending | low | annotate | S |
| `python/rutis/tests/test_peer.py:85,97` | `sleep(0.01)` | asserts a dropped call never ran | low | annotate | S |
| `python/rutis/tests/test_testing.py:387` | `sleep(0.05)` | asserts cancelled | low | annotate | S |
| `kotlin/.../AppTest.kt:175` | `Thread.sleep(100)` | "installs nothing before rutis offers events" | low | annotate | S |
| `kotlin/.../ConnectionTest.kt:199,234,245,482` | `Thread.sleep(200–1500)` | asserts not blocked / not closed | low | annotate | S |
| `kotlin/.../EventsConformanceTest.kt:255` | `Thread.sleep(LACKS)` | the conformance settle window | low | keep (scaled by `RUTIS_EVENTS_SETTLE_SCALE`) | — |
| `go/rutis/rutistest/events_conformance_test.go:601,669` | `time.Sleep(eventLacks)` / `time.After(eventSettle)` | same | low | keep | — |
| `go/.../dial_test.go:379`, `events_host_test.go:56`, `plugin_values_test.go:214` | `time.After(3–10s)` | hang guards | low | keep | — |

#### B — bounded polling for an event (move to the shared helper)

| Location | Snippet | Risk | Replacement | Effort |
| --- | --- | --- | --- | --- |
| `crates/rutis/tests/core/load_window.rs:263,409,558,637,747` | `while cond { sleep(1ms) }` inside `timeout(LIMIT)` | low | `rutis_test_support::until` | S |
| `crates/rutis/tests/core/evicted_apply.rs:278,368,475` | same | low | `until` | S |
| `crates/rutis/tests/core/still_waiting.rs:61,79,98,106,128` | `within(loop { sleep(10ms) })` | low | `eventually` | S |
| `crates/rutis/tests/core/embedding_output.rs:120` | same | low | `eventually` | S |
| `crates/rutis/tests/core/service_emits.rs:66` | `while len<2 && now<deadline { sleep(10ms) }`, deadline hard-coded at 10 s | low | `Guard(HANG_GUARD*2).until` (scaled by `RUTIS_TEST_TIME_SCALE`) | S |
| `crates/rutis/tests/core/terminal_cleanup.rs:112` | `sleep(1ms)` waiting for the backlog to drain | low | `until` | S |
| `crates/rutis/tests/core/instance_subtrees.rs:589` | `timeout(10s, loop sleep(1ms))` | low | `until` | S |
| `crates/rutis/tests/core/event_backlog.rs:57` | `sleep(20ms)` only to let the oldest age | low | already a monotone lower bound; keep | — |
| `crates/rutis/tests/core/dispatch_chain_probe.rs:94` | `timeout(60s, loop sleep(5ms))`, 60 s and unscaled | low | `Guard(HANG_GUARD*2).until` | S |
| `crates/rutis/tests/core/transient_release.rs:100` | `loop { … sleep(5ms) }` | low | `until` | S |
| `crates/rutis-bridge/tests/lang/event_hub.rs:223,298,676` | local `eventually()` definition plus select polling | low | delete the local copy, use `rutis_test_support::eventually` | S |
| `crates/rutis-bridge/tests/session/session_plugin.rs:1002` | local `eventually_async()` | low | keep (async check), or extend the shared helper | S |
| `crates/rutis-bridge/tests/session/mount_host_service.rs:127,151,161,181` | `within(loop sleep(10ms))` | low | `until` | S |
| `crates/rutis-bridge/tests/session/session_future_release.rs:122` | `while weak.upgrade() { assert deadline; sleep(10ms) }` | low | `until` | S |
| `crates/rutis-bridge/tests/session/session_tags.rs:58,99` | waits for a `port` file / for a process to exit | low | keep (cross-process, no event exists) | — |
| `crates/rutis-bridge/tests/native/local_spawn.rs:134` | `loop { send; sleep(10ms) }` | low | keep (waits for the pipe to report the exit) | — |
| `crates/rutis-bridge/tests/lang/local_soak.rs:80` | `while children>0 && now<deadline { sleep(50ms) }` | low | keep (`#[ignore]` soak) | — |
| `crates/rutis-loader/tests/rows/optional_deps.rs:503`, `optional_deps_go.rs:260` | `while … { sleep(20ms) }` | low | `until` | S |
| `crates/rutis-loader/tests/lang/objects_cross.rs:667,804` | `loop { … sleep(50ms) }` | low | `until` | S |
| `crates/rutis-loader/tests/events_conformance.rs:888,1030,1376` | local `until()` and polling | low | use the shared helper, or keep (that suite serialises itself) | M |
| `crates/rutis-loader/tests/events_conformance/rust_participant.rs:735` | `loop { … sleep(10ms) }` | low | `until` | S |
| `crates/rutis-loader/tests/go_stop.rs:106` | `while watching { sleep(2ms) }` | low | keep (samples the global process count) | — |
| `crates/rutis-host/tests/config/unhandled.rs:167` | `while !files { sleep(50ms) }` | low | keep (waits for files, cross-process) | — |
| `crates/rutis-host/tests/config/call.rs:59,179` | `while … { sleep(200ms) }` | low | keep (waits for CLI output) | — |
| `crates/rutis-host/tests/config/status.rs:119` | same | low | keep | — |
| `crates/rutis-host/tests/process/nodes.rs:122` | `while … { sleep(20ms) }` | low | keep | — |
| `crates/rutis-host/tests/process/reload.rs:506` | `while !cleaned.exists() { sleep(10ms) }` | low | keep (waits for a file) | — |
| `crates/rutis-agent/tests/integration.rs:260,396`, `unit_loop.rs:199`, `minimal_tools.rs:492`, `order_probe.rs:53` | `sleep(2–5ms)` polling | low | `eventually` | S |
| `tests/e2e/src/residue.rs:205`, `host.rs:419` | process-table polling | low | keep (no event exists) | — |
| `tests/e2e/tests/install/embedding.rs:295,324`, `crashes/stray_runtime.rs:92,180` | polling | low | keep | — |
| `tests/e2e/tests/soak/soak.rs:1442`, `soak/netem.rs:392`, `develop/dev_loop.rs:625`, `develop/devx.rs:1003`, `develop/host_writes_plugin.rs:187`, `install/ios_app.rs:257` | polling | low | keep | — |
| `swift/rutis/Tests/SelfTest/AppTests.swift:216,420` | `for _ in 0..<500 where !cond { sleep(10ms) }` | low | already wrapped in `waitFor`; could share it | S |
| `swift/.../PeerTests.swift:217,253,284,319,350`, `ObjectTests.swift:221,229`, `UnloadOrderTests.swift:163,170`, `EventsConformanceTests.swift:236,248` | bounded polling | low | extract a shared `eventually` | M |
| `kotlin/.../AppTest.kt:71` | `until()`, 10 s deadline | low | keep | — |
| `kotlin/.../ConnectionTest.kt:387,404`, `ObjectsTest.kt:294`, `PeerTest.kt:147,160`, `UnknownFieldsTest.kt:184`, `RutisTestTest.kt:59` | bounded polling | low | keep | — |
| `kotlin/.../EventsConformanceTest.kt:252,264` | polling | low | keep | — |
| `go/rutis/rutistest/events_conformance_test.go:598,614` | polling | low | keep (uses `testwait`) | — |
| `python/.../test_peer.py:122,126,149,152,159,183,199,363` | `await asyncio.sleep(0)` yields to the loop | low | keep (single event loop, deterministic) | — |
| `python/.../test_testing.py:298`, `test_chunked.py:114` | polling / one yield | low | keep | — |

#### C — performance steady state (keep, isolate)

| Location | Snippet | Risk | Action | Effort |
| --- | --- | --- | --- | --- |
| `crates/rutis-loader/tests/close_cpu.rs:183` | `drained()`: five 10 ms quiet windows in a row | low | already its own binary; keep (the file header states "no other test may run beside the measured one") | — |
| `crates/rutis-loader/tests/close_cpu.rs:202` | `sleep(IDLE=1s)` measuring CPU | low | same | — |
| `crates/rutis-loader/tests/close_memory.rs:247` | `settle()`: waits for tasks/bytes to stop moving, scaled by `time_scale()` | low | already its own binary; keep | — |
| `crates/rutis-bridge/tests/lang/local_soak.rs:73` | `sleep(500ms)` for an fd baseline | medium | see E-06 (process-global measurement) | S |
| `tests/e2e/tests/soak/soak.rs:1589`, `soak/netem.rs:616` | soak idle sampling / a 35 s phase | low | e2e owns its process; keep | — |

#### D — deliberate race windows (the assertion must hold either way)

| Location | Snippet | Risk | Action | Effort |
| --- | --- | --- | --- | --- |
| `crates/rutis/tests/core/load_window.rs:643,753` | `sleep(50ms)` so the withdrawal queues its refresh | low | wait for an observable "queued" record, or mark `// clock: widen` | S |
| `crates/rutis/tests/core/events.rs:1716` | `sleep(50ms); // Give a delivery out of order its chance` | low | add `// clock: widen` | S |
| `crates/rutis/tests/core/terminal_cleanup.rs:210` | sampler coroutine `sleep(200µs)` | low | keep (diagnostics only) | — |
| `crates/rutis/tests/core/dispatch_chain_probe.rs:49` | `thread::sleep(50µs); // 放大重叠窗口` | low | add `// clock: widen` | S |
| `crates/rutis-bridge/tests/session/session_plugin.rs:1022` | `sleep(50ms)` before disposing, so the call is in flight | low | wait for a "call started" signal | S |
| `crates/rutis-loader/tests/rows/event_close_admission.rs:117` | `sleep(100ms); // Give the dispatch a bounded window` | low | add `// clock: widen` | S |
| `crates/rutis-bridge/tests/session/cancellation.rs:129` | `sleep(100ms)`, already marked `// clock: widen` | low | leave as is (a model) | — |
| `swift/.../AppTests.swift:318,350,355` | 50 ms / 0.3 ms jitter so the stop lands in each dial phase | low | keep (the assertion is independent of the hit); comment it | S |
| `swift/.../ConcurrencyTests.swift:21` | `Task.sleep(20ms)` so all 64 tasks reach the waiters | low | wait until the waiter count is N | S |
| `swift/.../ObjectTests.swift:218` | `sleep(100ms)` then cancel | low | wait for "the object started running" | S |
| `swift/.../PeerTests.swift:106,133` | 50/200 ms for the attach to complete | low | wait for the attach event | S |
| `kotlin/.../ConnectionTest.kt:399` | `Thread.sleep(100)` then `stop()` | low | wait for `listening` to have started | S |
| `kotlin/.../ObjectsTest.kt:257`, `PeerTest.kt:158`, `UnloadOrderTest.kt:173` | `sleep(100)` then cancel | low | wait for "the call started" | S |
| `node/rutis/test/events.test.mjs:69` | `sleep(20)` then unload | low | wait for the listener to have started | S |
| `python/.../test_objects.py:147` | `sleep(0.05)` then cancel | low | wait for `run` to have been entered | S |

#### E — real defects, sleep-related

| Location | Snippet | Problem | Risk | Replacement | Effort |
| --- | --- | --- | --- | --- | --- |
| `crates/rutis/tests/core/still_waiting.rs:124` | `sleep(50ms)` before `provide(Greeting)`, while the report threshold is 300 ms | if the 50 ms stretches past 300 ms the report fires early and `assert!(reported.is_empty())` fails. A bare sleep is not scaled by `RUTIS_TEST_TIME_SCALE`, so this happens under load | **high** | do not race the window: raise `report_waiting_after` (e.g. 10 s) and assert only that no report comes, or use a paused clock | S |
| `crates/rutis/tests/core/unload_deadline.rs:49` | `spawn(emit Hang); sleep(50ms); dispose(); assert(elapsed >= 250ms)` | if the listener has not been dispatched within 50 ms, dispose has no cleanup to wait for, `elapsed` < 250 ms, and the assertion fails | **high** | replace the sleep with a signal that the listener has entered (`Notify` / `oneshot`) | S |
| `swift/rutis/Tests/SelfTest/AppTests.swift:362` | `for _ in 0..<300 where DialedSocket.live.sessions != sessions { sleep(10ms) }`, then assert equality | `DialedSocket.live` is a process-global counter; any residue from an earlier test fails the assertion | medium | count instances injected by the test instead, or assert "did not grow" rather than "equals the baseline" | M |

#### E — real defects, not sleep-related (global state / shared counters / fixed ports)

| Location | Snippet | Problem | Risk | Replacement | Effort |
| --- | --- | --- | --- | --- | --- |
| `crates/rutis/tests/core/unload_deadline.rs:37` | `std::env::set_var("RUTIS_SHUTDOWN_TIMEOUT", "0.3")`, **never restored** | `shutdown_deadline()` (`crates/rutis/src/fiber.rs:1344`) reads the environment on every call, so the value leaks into the unload path of every concurrent and later test in the `core` binary | **high** | take a process-level env mutex (as `rutis-host/tests/common/mod.rs:33` `environment()` does), or pass the deadline into the object under test | S |
| `crates/rutis-loader/tests/rows/virtual_clock_contract.rs:184-200` (one env write at 185/187/189/192/199 each) | five `set_var`/`remove_var` statements inside a `#[test]`; the comment says "the variable belongs to the process" | other tests in the `rows` suite that unload plugins concurrently read 0.25 s / 3 s / invalid values | **high** | same: add a mutex, or make `shutdown_deadline()` injectable | M |
| `crates/rutis-host/tests/dial/dial_conformance.rs:245` | `std::env::set_var("RUTIS_TOKEN_PHONE", TOKEN)`, **without `environment()`** | the same suite's `lease_e2e.rs:433`, `swift_app.rs:206` and `kotlin.rs:91` run `environment()`, which removes every `RUTIS_TOKEN_*` (`common/mod.rs:39-41`). Interleaving leaves the `dial_conformance` host without its token | **high** | take `crate::common::environment()` before `listen()` and hold it until the test ends | S |
| `crates/rutis-host/tests/config/android.rs:156` | the same unlocked `set_var` | `feature = "android"` only, run locally, not in CI; but it collides with guarded tests such as `remote_runtimes.rs` the moment they share a run | medium | same fix | S |
| `crates/rutis-loader/tests/go_stop.rs:96,133` | `assert_eq!(running_processes(), vec![pid])` | exact equality on the process-global registry. Safe today only because the file holds a single test and is its own binary | low (latent) | assert containment instead, or state in a comment that it must stay its own binary | S |
| `crates/rutis-bridge/tests/lang/local_soak.rs:14-34`, `crates/rutis-bridge/tests/lang/websocket_soak.rs:15-20` | read `/proc/self/fd`, `/proc/self/task/*/children`, thread count | a process-global measurement placed in the `lang` suite. Safe today only because `stress.yml:506-507` filters by `local_soak::` / `websocket_soak::` and runs one test | medium (latent) | move back to its own binary, or state in the header that it must not run beside other modules | S |

**E-class items already handled correctly (use these as models):**

- Fixed ports 9230 and 2345: `crates/rutis-host/tests/config/debug.rs:35-45` serialises cross-process with `flock(/tmp/rutis-host-debug-ports.lock)`; 7 call sites.
- Process-list assertions no longer look at the whole list: `crates/rutis-host/tests/process/reload.rs:468-472` asserts only `!running.contains(&restarted)`, and the comment acknowledges that other tests may run their own runtimes.
- Using the global process list in diagnostics is safe: `crates/rutis-loader/tests/go/go_rows.rs:156`, `crates/rutis-bridge/tests/platform/host_gone.rs:49` only build failure messages.
- Measurements that need exclusivity already have their own binary: `crates/rutis-loader/tests/close_cpu.rs:6-7`, `crates/rutis-loader/tests/close_memory.rs`.

### 5.2 Swift

Swift's `RutisSelfTest` is a sequential runner, so global-state risk is lower than in Rust; the problems are mostly duration upper bounds and fixed waits for a positive event.

| Location | Snippet | Cat | Risk | Replacement / action | Effort |
| --- | --- | --- | --- | --- | --- |
| `swift/rutis/Tests/SelfTest/AppTests.swift:275` | `sleep(500ms); XCTAssertEqual(app.state, .connecting)` | A | low | keep, annotate | S |
| `AppTests.swift:326` | `sleep(50ms); XCTAssertEqual(discarded.all, ["socket"])` | A | low | keep | — |
| `AppTests.swift:438` | `sleep(100ms); XCTAssertEqual(lines, [])` | A | low | keep | — |
| `AppTests.swift:216,420` | `for _ in 0..<500 where !cond { sleep(10ms) }` | B | low | already wrapped in `waitFor`; share it | S |
| `AppTests.swift:351,354` | bounded polling for accept | B | low | keep | — |
| `AppTests.swift:318,350,355` | jittered sleeps so stop lands in each dial phase | D | low | keep (assertion holds either way); comment it | — |
| `AppTests.swift:362` | polls the global `DialedSocket.live` counters, then asserts equality | **E** | medium | count instances injected by the test, or assert "did not grow" | M |
| `AppTests.swift:75` | the `uncancellableSleep` helper | H | — | keep | — |
| `ChunkedTests.swift:167,229` | `sleep(100ms); XCTAssertEqual(tally, 0)` | A | low | keep | — |
| `ConcurrencyTests.swift:21` | `sleep(20ms)` so 64 tasks reach the waiters | D | low | wait for `waiters.count == N` | S |
| `EventsConformanceTests.swift:236,248` | bounded polling | B | low | keep | — |
| `EventsConformanceTests.swift:239,275` | settle / cancellation timer | A | low | keep | — |
| `LifetimeTests.swift:20,27` | a 100 ms slow handler in a fixture | H | — | keep | — |
| `LifetimeTests.swift:35` | a trailing `sleep(1s)`, no assertion | A | low | keep (lets queued calls run) | — |
| `ObjectTests.swift:218` | `sleep(100ms)` then cancel | D | low | wait for "the object started running" | S |
| `ObjectTests.swift:221,229` | bounded polling of the export table | B | low | keep | — |
| `ObjectTests.swift:117` | a 60 s fixture | H | — | keep | — |
| `PeerTests.swift:106,133` | 50/200 ms for the attach | D | low | wait for the attach event | S |
| `PeerTests.swift:217,253,284,319,350` | bounded polling | B | low | extract a shared `eventually` | M |
| `PeerTests.swift:67,94`, `UnknownFieldTests.swift:161` | 30 s fixtures | H | — | keep | — |
| `UnloadOrderTests.swift:163,170` | bounded polling, 50 ms × 100 | B | low | share the helper | S |
| `Sources/RutisTest/RutisTest.swift:151,206,227`, `RutisTestDevice.swift:98` | 1–2 ms polling | S | — | canonical implementation; keep | — |
| `Sources/RutisFixturePlugins/*` (8 sites in 6 files) | fixture delays (`Lease.swift:39`, `Weather.swift:45`, `PluginValues.swift:68`, `Objects.swift:86`, …) | H | — | keep. Note that `Objects.swift:86` (`exit(1)` after 20 ms) and `PluginValues.swift:68` (`exit(17)` after 200 ms) make the outcome depend on fixture timing; prefer exiting on a signal | M |

### 5.3 Kotlin

| Location | Snippet | Cat | Risk | Replacement | Effort |
| --- | --- | --- | --- | --- | --- |
| `kotlin/rutis/rutis/src/jvmTest/kotlin/arcships/rutis/ConnectionTest.kt:370,376` | `Thread.sleep(200)` for the server to accept two idle connections, then assert the third is refused | **B (the dangerous kind: a fixed duration waiting for a positive event)** | **medium-high** | wait for `server.accepted.size == 2` (the server already keeps a count) | S |
| `ConnectionTest.kt:286` | `assertTrue((nanoTime-start)/1e6 < 100, "closeWith blocked")` | **C (duration upper bound)** | **medium-high** | drop the bound for a causal assertion (it did not block), or raise it to seconds | S |
| `ConnectionTest.kt:300` | `assertTrue(... < 1000, "the other connection was not pinged")` | C | medium | wait for the ping frame instead of for a duration | S |
| `ConnectionTest.kt:199,234,245,482` | 200–1500 ms negative assertions | A | low | keep | — |
| `ConnectionTest.kt:387,404` | bounded polling | B | low | keep | — |
| `ConnectionTest.kt:399` | `sleep(100)` then `stop()` | D | low | wait for `listening` to have started | S |
| `ConnectionTest.kt:208,312,353,439` | fixture threads | H | — | keep | — |
| `ObjectsTest.kt:257` | `sleep(100)` then cancel | D | low | wait for the call to have started | S |
| `ObjectsTest.kt:294`, `PeerTest.kt:147,160`, `UnknownFieldsTest.kt:184` | bounded polling | B | low | keep | — |
| `PeerTest.kt:89` | fixture `delay(20)` | H | — | keep | — |
| `PeerTest.kt:158`, `UnloadOrderTest.kt:173` | `delay(100)` then cancel | D | low | wait for a "started" signal | S |
| `AppTest.kt:175` | `Thread.sleep(100)` negative assertion | A | low | keep | — |
| `AppTest.kt:71` | `until()` with a 10 s deadline | B | low | keep | — |
| `EventsConformanceTest.kt:252,264` | polling | B | low | keep | — |
| `EventsConformanceTest.kt:255` | settle | A | low | keep (scaled by `RUTIS_EVENTS_SETTLE_SCALE`) | — |
| `RutisTestTest.kt:59` | bounded polling | B | low | keep | — |
| `rutis-conformance/.../plugins/*` (6 sites in 5 files) | fixture delays | H | — | keep | — |

### 5.4 Go

| Location | Snippet | Cat | Risk | Replacement | Effort |
| --- | --- | --- | --- | --- | --- |
| `go/rutis/rutistest/events_conformance_test.go:598,614` | `time.Sleep(10ms)` / `5ms` polling | B | low | use `testwait.Eventually` | S |
| `events_conformance_test.go:601` | `time.Sleep(eventLacks)` | A | low | keep | — |
| `events_conformance_test.go:669`, `plugin_values_test.go:214`, `internal/runtime/dial_test.go:379`, `events_host_test.go:56` | `time.After(...)` hang guards | A | low | keep (the equivalent of `testwait.HangGuard`) | — |
| `internal/runtime/events_test.go:216`, `listen_test.go:427` | slow operations in fixtures | H | — | keep | — |
| `internal/fixtures/**` (5 sites) | fixture scenario | H | — | keep | — |
| `internal/testwait/testwait.go:62` | the canonical `Eventually` 10 ms poll | S | — | canonical implementation; keep | — |

No category-E finding in Go. `go/rutis/internal/testleak` verifies leaks with goleak from `TestMain`, and `SharedHost()` deliberately ignores the goroutines of the shared host. That is intentional, correct global-state handling.

### 5.5 Node / Bun

| Location | Snippet | Cat | Risk | Replacement | Effort |
| --- | --- | --- | --- | --- | --- |
| `node/rutis-runtime/test/cordis-events.test.mjs:322` | `sleep(10ms); assert.deepEqual(reported, [full list])` | **B (a fixed duration waiting for a positive event)** | medium | use `eventually` to wait for `reported.length === 1` | S |
| `cordis-events.test.mjs:330` | `sleep(10ms); assert.deepEqual(reported, [])` | A | low | keep | — |
| `node/rutis-runtime/test/websocket.test.mjs:168` | `sleep(500ms); assert.equal(closed,false)` | A | low | keep | — |
| `node/rutis-runtime/test/leaf.test.mjs:217,249` | `setTimeout` inside fixtures | H | — | keep | — |
| `node/rutis-runtime/test/fixtures/**` (10 sites) | fixtures | H | — | keep | — |
| `node/rutis-runtime/test/serve.test.mjs:124,138` | `took < 4500` / `took < 1000` | **C** | medium | widen the bound to at least 10× the expected duration, or delete it | S |
| `node/rutis/test/events.test.mjs:35` | bounded polling, 100 × 10 ms | B | low | use `eventually` | S |
| `node/rutis/test/events.test.mjs:69` | `sleep(20)` then unload | D | low | wait for the listener to have started | S |
| `node/rutis/test/testing-on.test.mjs:10,59` | polling / negative assertion | B / A | low | keep | — |
| `node/rutis/test/testing-*.test.mjs` (4 deadline polls) | bounded polling | B | low | keep | — |
| `node/rutis/test/fixtures/**` (3 sites) | fixtures | H | — | keep | — |
| `node/rutis-runtime/test/support.mjs:18,40`, `bun/rutis-bun/test/support.ts:24,26` | the canonical helper | S | — | canonical; keep | — |
| `bun/rutis-bun/test/remote.test.ts:61,73` | `toBeLessThan(4_500)` / `toBeLessThan(1_000)` | C | medium | as with serve.test.mjs | S |
| `bun/rutis-bun/test/sdk-definitions.test.ts:22` | bounded polling | B | low | use `eventually` | S |
| `bun/rutis-bun/test/bad-values.test.ts:342`, `node/rutis-runtime/test/session.test.mjs:836` | 5 × 10 ms after a GC | C (steady state) | low | keep | — |
| `bun/rutis-bun/test/fixtures/**`, `node/rutis-runtime/test/fixtures/**` | fixtures | H | — | keep | — |

### 5.6 Python

| Location | Snippet | Cat | Risk | Replacement | Effort |
| --- | --- | --- | --- | --- | --- |
| `python/rutis/tests/test_output.py:140` | `sleep(0.05)` waiting for the listener thread to write a line into the redirected stderr | **B (a fixed duration waiting for a positive event)** | medium | wait on a `threading.Event` set when the line is written, or poll the buffer until the line appears | S |
| `test_peer.py:122,126,149,152,159,183,199,363` | `await asyncio.sleep(0)` to yield one turn | B | low | keep (single event loop, deterministic) | — |
| `test_peer.py:85,97` | `sleep(0.01)` negative assertion | A | low | keep | — |
| `test_events_session.py:233,762` | `sleep(0.05)` negative assertion | A | low | keep | — |
| `test_testing.py:298` | polling, 100 × 10 ms | B | low | use `eventually` | S |
| `test_testing.py:387` | `sleep(0.05)` negative assertion | A | low | keep | — |
| `test_objects.py:147` | `sleep(0.05)` then cancel | D | low | wait for `run` to have been entered | S |
| `test_chunked.py:114` | `settle()` = `sleep(0.01)` | B | low | keep (same event loop) | — |
| `tests/support.py:48,60` | the canonical `eventually` poll | S | — | canonical; keep | — |
| `tests/fixtures/**`, `conformance_*.py` | fixture `after` delays | H | — | keep | — |

Python global state: `test_testing.py:510` writes `os.environ["RUTIS_HOST"]` and `PATH`; `test_websocket.py:475-479` sets and deletes `RUTIS_HEARTBEAT`. unittest runs sequentially, so there is no concurrency risk today — but `pytest-xdist` or a thread pool would turn these into category E.

### 5.7 tools / workflows

| Location | Snippet | Cat | Risk | Action | Effort |
| --- | --- | --- | --- | --- | --- |
| `.github/workflows/ci.yml:463,589,1126,1584,2122` | `sleep 20` inside `for attempt in 1 2 3`, retrying the `dlv` / Gradle fatJar download | retry backoff | low | **keep** (waiting for the network, not for an event) | — |
| `.github/workflows/ci.yml:2191` | `sleep 2` retrying `rm -rf "$GOCACHE"` | retry backoff | low | **keep** | — |
| `.github/workflows/release.yml:497` | `sleep $((attempt * 3))`, exponential upload retry | retry backoff | low | **keep** | — |
| `.github/workflows/release.yml:381` | `for _ in $(seq 90); do published && break; sleep 30` | B (bounded poll) | low | keep; the 45-minute bound is explicit | — |
| `.github/workflows/release.yml:568` | `while :; sleep 15`, 15-minute bound, commented "a hang guard, not a synchronization" | A | low | **keep** (a model of how to write it) | — |
| `tools/ci-self-hosted.sh:123` | `sleep 2` between SIGTERM and SIGKILL | signal-escalation grace | low | keep; could poll `kill -0` instead | S |
| `tools/android-emulator.sh:127` | checks the PID every 2 s, 5-minute bound | B (bounded poll) | low | keep | — |
| `tools/test-dylib-launcher.sh:163,183` | checks `tasklist` every 0.2 s, 50 attempts | B (bounded poll) | low | keep | — |
| `tools/bench/events/run.sh:65` | checks for the report file every 0.5 s, 600 attempts | B (bounded poll) | low | keep | — |

No workflow uses a sleep as a stand-in for synchronization; every sleep is either a network retry or an explicit bounded poll.

### 5.8 Non-sleep category-E summary

| ID | Location | Kind |
| --- | --- | --- |
| E-01 | `crates/rutis/tests/core/unload_deadline.rs:37` | process env leak |
| E-02 | `crates/rutis-loader/tests/rows/virtual_clock_contract.rs:184-200` (one env write at 185/187/189/192/199 each) | concurrent process-env write |
| E-03 | `crates/rutis-host/tests/dial/dial_conformance.rs:245` | unlocked env write colliding with a locked removal |
| E-04 | `crates/rutis-host/tests/config/android.rs:156` | same (feature-gated, local) |
| E-05 | `crates/rutis-loader/tests/go_stop.rs:96,133` | exact equality on the global process table (latent) |
| E-06 | `crates/rutis-bridge/tests/lang/local_soak.rs:14-34`, `websocket_soak.rs` | process-global fd/thread measurement inside a shared suite (latent) |

With the three sleep-related ones (`still_waiting.rs:124`, `unload_deadline.rs:49`, `AppTests.swift:362`), that is **9 category-E findings, 5 of them high risk**.

---

## 6. Priority and recommended order of work

### P0 — this week; without these, CI will flake

1. **E-01 `unload_deadline.rs:37`**: put `RUTIS_SHUTDOWN_TIMEOUT` behind a process-level mutex for the `core` suite and restore it afterwards. Copy `environment()` from `rutis-host/tests/common/mod.rs:33-44` (clean on entry, hold until the test ends).
2. **E-02 `virtual_clock_contract.rs:184-200`**: the `rows` suite needs the same `environment()`. The cleaner fix is to make `shutdown_deadline()` injectable, but that is a product change; take the lock first.
3. **E-03 `dial_conformance.rs:245`**: take `crate::common::environment()` before `listen()` and hold it to the end of the test.
4. **The two race-decided sleeps**:
   - `still_waiting.rs:124`: raise `report_waiting_after` (say 10 s) so the test only checks that a short wait says nothing, or use a paused clock;
   - `unload_deadline.rs:49`: replace the 50 ms with a signal that the listener has entered.
5. **E-04 `android.rs:156`**: the same fix (low priority, do it while you are there).

### P1 — next batch; remove the medium-high risks

6. **Duration upper bounds (C)**: `ConnectionTest.kt:286` (< 100 ms) is the tightest, followed by `ConnectionTest.kt:300`, `serve.test.mjs:124,138`, `remote.test.ts:61,73`. The repo rule (`docs/design-test-clocks-2026-10-11.md` §3.3) asks for a 10× gap between the expected duration and the hang guard; `< 100 ms` does not satisfy it. Replace the bound with a causal assertion.
7. **Fixed durations waiting for a positive event (the three worst B items)**: `ConnectionTest.kt:370,376` (the server already keeps an `accepted` count), `cordis-events.test.mjs:322`, `test_output.py:140`.
8. **E-05 / E-06 (latent)**: state in the headers of `go_stop.rs` and of `local_soak.rs` / `websocket_soak.rs` that they must stay their own binary or be run with a module filter, or move them back into their own binaries. Cheap, and it stops the next suite reshuffle from repeating #1373.

### P2 — mechanical cleanup, once

9. **Collapse the 116 B items onto the shared helper**: delete the 27 local `eventually` / `until` / `wait_for` / `soon` definitions and use `rutis_test_support::eventually(_with)` / `until` / `Guard`. The ones worth doing first:
   - `service_emits.rs:66` (hard-coded 10 s, not scaled by `RUTIS_TEST_TIME_SCALE`) → `Guard(HANG_GUARD * 2).until`;
   - `dispatch_chain_probe.rs:94` (60 s, unscaled) → the same;
   - `events_conformance.rs:888,1030,1376` (a local `until` returning `Result`) → unify.
   
   This also gives failure messages the state at the time, which is exactly what #238 / #249 / #1003 asked for.
10. **Annotate A and D with `clock:`**: the tree has only five such comments today (`crates/rutis-bridge/src/channel/testing.rs:89,113,142,156`, `crates/rutis-bridge/tests/session/cancellation.rs:127`). All 74 A items and 21 D items should carry one, otherwise the next reader cannot tell "deliberate" from "missed".
11. **Cost**: `plugin_limits.rs:192` spends 12 s on the critical path of the `rows` suite. Assert `starting` first, then wait for a later event that must follow.

### P3 — optional

12. Share the polling helpers in Swift / Kotlin / Node (1–2 hours each).
13. If Python ever gains a parallel runner, handle the `os.environ` writes in `test_testing.py:510` and `test_websocket.py:475` first.

### What not to do

- Do not convert A items into event waits. A negative assertion has no event to wait for; converting it invents a new false signal.
- Do not touch the already-isolated C items (`close_cpu`, `close_memory`). Their design — own binary, scaling by `time_scale()`, explicit stability detection — is correct.
- Do not touch the `#[ignore]` soak and `netem` tests; they only run from stress, filtered by name.

---

## 7. Legitimate sleeps that need no change

Listed explicitly so that a later reader does not "fix" them.

### 7.1 Hang guards (only guard a hang; never used as synchronization)

| Location | Note |
| --- | --- |
| `crates/rutis-test-support/src/lib.rs:28` `HANG_GUARD = 10s` | the facility itself |
| `crates/rutis/tests/core/transient_release.rs:66` `select!{ sleep(deadline), changed() }` | the event comes first; the sleep is only a bound |
| the `COLD` / `deadline` patterns in `crates/rutis-host/tests/config/dev.rs` and `unhandled.rs` | bounded waits |
| `go/rutis/internal/testwait/testwait.go:18` `HangGuard` | the facility itself |
| `go/.../dial_test.go:379`, `events_host_test.go:56`, `plugin_values_test.go:214`, `events_conformance_test.go:669` | `time.After` as a bound |
| `node/rutis-runtime/test/support.mjs:18`, `bun/.../support.ts:26`, `node/baseline/native.mjs:30`, `python/.../support.py:29-60`, `swift/.../RutisTest.swift:147-229` | the canonical implementation per language |
| `.github/workflows/release.yml:568` | a 15-minute bound, commented "a hang guard, not a synchronization" |

### 7.2 Fixture scenario content (simulating a slow operation is the scenario)

| Location | Note |
| --- | --- |
| `crates/rutis/tests/core/events.rs:334,404,969` | delays inside test plugins, to build event ordering |
| `crates/rutis-loader/tests/rows/settled.rs:38,43` | models a slow `apply` |
| `crates/rutis-loader/tests/events_conformance.rs:1549`, `events_conformance/rust_participant.rs:128,209` | conformance fixtures honouring `after` |
| `crates/rutis-bridge/tests/session/rpc_callbacks.rs:247,282` | a callback that answers late |
| `crates/rutis-agent/tests/unit_loop.rs:507`, `integration.rs:283`, `order_probe.rs:48` | agent tool / ordering fixtures |
| `crates/rutis-dsh/tests/common/mod.rs:72` | an endless stream fixture |
| `tests/e2e/fixture-toolhost/src/main.rs:238`, `fixture-tenants/src/main.rs:284` | e2e fixtures |
| `crates/rutis-bridge/tests/session/session_tags.rs:227` | the child process whose role is to serve forever |
| `swift/.../RutisFixturePlugins/*` (8 sites), `RutisSessionFixture/main.swift:45`, `RutisAppFixture/main.swift:74` | Swift fixtures |
| `kotlin/.../rutis-conformance/.../plugins/*` (6 sites) | Kotlin fixtures |
| `go/rutis/internal/fixtures/**` (5 sites) plus `runtime/events_test.go:216`, `listen_test.go:427` | Go fixtures |
| `node/rutis-runtime/test/fixtures/**`, `node/rutis/test/fixtures/**`, `bun/.../fixtures/**` | Node/Bun fixtures |
| `python/rutis/tests/fixtures/**`, `conformance_*.py`, `test_output.py:35-55`, `test_shutdown.py:43`, `test_objects.py:138` | Python fixtures |

### 7.3 Product code (not a test wait)

| Location | Note |
| --- | --- |
| `swift/rutis/Sources/Rutis/Remote/App.swift:406,447,517,549`, `Listen.swift:252,335`, `WebSocket.swift:260`, `Session/Value.swift:268` | dial retries, upgrade timeout, grace close |
| `kotlin/.../internal/peer/Peer.kt:630`, `remote/Dial.kt:300`, `remote/Frames.kt:126` | close wait, retry interval |
| `go/rutis/internal/runtime/listen.go:573` | WebSocket close grace |

### 7.4 Already-correct global-state handling (keep; use as models)

| Location | Note |
| --- | --- |
| `crates/rutis-host/tests/config/debug.rs:35-45` | `flock` serialises the fixed ports 9230 / 2345 |
| `crates/rutis-host/tests/process/reload.rs:468-472` | asserts non-membership instead of exact equality |
| `crates/rutis-loader/tests/go/go_rows.rs:156`, `crates/rutis-bridge/tests/platform/host_gone.rs:49` | the global process list is used only for failure diagnostics |
| `crates/rutis-loader/tests/close_cpu.rs`, `close_memory.rs` | process-level measurement kept in its own binary |
| `crates/rutis-loader/tests/rows/event_close_admission.rs:195,245,417,446` | `fired.notified()` establishes the cause, then 100 ms observes |
| `crates/rutis/tests/core/load_window.rs:269`, `evicted_apply.rs:481` | the consumer is gated on a channel, so the assertion is deterministic |
| `go/rutis/internal/testleak/testleak.go` | verifies leaks with goleak and explicitly ignores the shared host's goroutines |

### 7.5 CI / tool retry backoff (not synchronization)

| Location | Note |
| --- | --- |
| `.github/workflows/ci.yml:463,589,1126,1584,2122` | retried downloads / builds |
| `.github/workflows/ci.yml:2191` | retried `rm -rf` |
| `.github/workflows/release.yml:497` | exponential upload retry |
| `.github/workflows/release.yml:381` | Maven Central publication poll (45-minute bound) |
| `tools/ci-self-hosted.sh:123` | SIGTERM → SIGKILL escalation grace |
| `tools/android-emulator.sh:127`, `tools/test-dylib-launcher.sh:163,183`, `tools/bench/events/run.sh:65` | bounded polls |

---

## 8. Appendix: counting rules and what was not covered

- The counts come from a per-site human classification, cross-checked by a script: the classification table matches all 336 hits with no gap and no duplicate. Several lines of the same shape in one file are listed as one row, with all line numbers given.
- No test was run, so no observed flake is reported. The high-risk E judgements rest on a static question — "does the race window decide the assertion?" — plus the known parallelism facts (`RUST_TEST_THREADS`, the `--test-threads` rotation).
- Not covered: code samples under `docs/**`; `examples/**` (example projects, out of scope per the design document); `fuzz/**` (no timing assertions); `kotlin/rutis/android-sample` (needs a device).
- There is no sleep-based wait under `fuzz/`; `tools/bench/events/bench.mjs:48`'s `setTimeout(..., 0)` only yields to the event loop.
- After the P0 fixes, validate with the `--test-threads=$cores` round of `stress.yml`; that is the configuration in which #1373's regressions reproduce most readily.
