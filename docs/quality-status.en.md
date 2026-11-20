# rutis quality status

[中文](quality-status.md) · [Quality standard](quality-standard.en.md)

Status: kept up to date with the code. Last updated: 2026-11-20 (#1375: moved OS-independent jobs back to Linux, ran `rust` and `rust-runtimes` in parallel, added Gradle caches, and removed duplicate PR work; the first ordinary code PR after #1375, #1384, took 10.0 minutes from push to result, meeting the ≤ 10-minute target, §8.5). Previous: 2026-11-20 (#1372: retired the 1,825-row static rule tracker `docs/formal-trace/`, `tools/formal-trace*.mjs`, and `model-pr.yml`; trimmed `docs/guarantees.en.md` to the 40 active core promises with a forward form check; wired the 6 `rutis-loader` and `rutis-host` shuttle models into `ci.yml` and `stress.yml` (all 65 across four crates); `specs/*/run.sh` skips the 95 `*-mut-*` mutation configs by default (224 → 129) and `nightly.yml` triggers `specs` only on `specs/**` changes; kept the 11 passing 0.23 Kani harnesses and 6 Kotlin Fray suites as on-demand toolboxes). Earlier: 2026-11-18 (#1270 second half + #1099 protocol 5: 15 kernel/event rules decided one by one, model 28 and the two-consumer eviction test added, kernel models 27 → 28 and four crates 63 → 65 (bridge gained one in #1331); protocol 5's cancel acknowledgement and the migration guide landed). Earlier history is in `git log`.
Companion to: [Quality standard](quality-standard.en.md). This document maps the standard, clause by clause, to the current implementation: which clauses are met, which partly, what is missing and how it will be added. The standard describes rules that stay the same; the file names, test names, issue numbers and versions here change with the implementation. Which tests verify each guarantee is in the [guarantee register](guarantees.en.md).

---

## 1. Status by clause

Status: **met**, **partly**, **not met**, **n/a**. The gaps column refers to risk numbers in §4 and to issues.

### 1.1 Cross-cutting requirements

| Clause | Content | Status | Current state and gaps |
| --- | --- | --- | --- |
| Q3.1 | Core guarantees verified at the highest level | Partly | "Nothing left behind" and "explicit failure on interruption" are now verified as a black box on all three platforms: E2E scenarios end with residue checks (#221, #277); S3, S5 and `fault_links.rs` verify that interrupted calls fail explicitly and are not retried (#317, #322, #285). Missing: `core/D22`, `core/D28` and `boundary/5.8` are "partly verified" in the register; on Unix a runtime whose event loop is not empty does not exit after its host is `kill -9`ed (B3); long runs are verified nightly on all three platforms by S8 (#192) |
| Q3.1, Q8.3 | Core guarantees verified in two independent ways | Partly | The Python SDK's session tests now match Node's (release counting, grant rollback, protocol version, #288); cross-runtime synchronous calls, remote leases, unified events, row and call lifecycles and peer-hosted rows have TLA+ design models (7 directories, 17 modules, §7.2), the session channel has trace validation (§7.5), and the core has 28 shuttle models, `rutis-bridge` 31, `rutis-host` 3 and `rutis-loader` 3 (§7.3). Nobody has checked, guarantee by guarantee, that each core guarantee has two independent verifications (the register has a single "verification" column) |
| Q3.4 | Behavior explicitly not guaranteed is pinned | Partly | D29 (events not filtered by isolate) has a regression test (`event_order.rs`, #280); the limits on values are registered as not guaranteed and tested (`bridge/value-limits`, #296). D22 (no dependency cycle detection) is still not pinned by a test |
| Q4.5 | Risk assessment recorded | Partly | §4 of this document is updated at the end of each step (this time 2026-11-17); design documents do not state risks (Q9.1.1) |
| Q5.1.2 | Results are checked | Partly | The two known places are fixed: `typed_plugin.rs` asserts what the ErrorSink receives (#286); D24 in `contract.rs` asserts the arrival order and no longer sorts first (#280). Other tests have not been reviewed systematically |
| Q5.2 | Each fault on the list injected | Partly | Injected: a link dropping mid-call, cancellation and release under delay, new calls during a reconnect, multiple hops (`fault_links.rs`, #285); `kill -9` of a runtime (S3, #317); a network cut between two hosts, the far end restarting, a changed token (S5, #322); a real network between two containers: 200 ms delay + 5% loss, an outage and its recovery, a changed token (S5 in containers, #433, nightly); concurrent starts (`concurrent_launch.rs`, #215); the host receiving signals (`signals.rs` #216; Ctrl-C and Windows Ctrl-Break in E2E, #277); protocol version mismatch (#288, #335); malformed frames (`malformed`, 25 cases × seven implementations). a configuration broken while running (`host/reload-config`, #274); repeated faults during long runs: random rows added, removed and changed with reloads, rejected configurations, runtimes `kill -9`ed and restarted, links cut and redialed (S8, #192). Missing: a plugin blocking the event loop (`boundary/5.8` partly verified) |
| Q5.3.2 | Encryption across machines, TLS off loopback | Partly | Negative TLS: expired certificate, hostname mismatch, untrusted CA, verified on the Rust side (`websocket.rs`) and the Python side (`test_websocket.py`) (#297), and between two host processes (S5 step 6); `wss://` on a non-loopback address between two containers, with a CA made at test time (S5 in containers, #433). The category for an invalid client certificate on Windows is fixed (#300 → #334). Missing: whether a listener without TLS on a non-loopback address is refused has not been checked (B8) |
| Q5.3.3 | Credentials kept out of output | Partly | Every E2E scenario ends by checking that no captured output contains the scenario's credentials (#221, #277); in S5 the token is absent from the output when it is refused (#322). Library-level logs, traces and diagnostics have no common check |
| Q5.3.4 | External input bounded, malformed input does not crash | Partly | WebSocket has a 16 MiB limit; local line framing has a limit (#173 → #214); value nesting is limited to 64 levels, and going over fails only that call (#279 → #338); the malformed-frame check covers seven implementations (including Go after #294 → #305); fuzzing: session frames, local line framing, error graphs, WebSocket upgrade requests, dylib metadata and binary manifests, dsh profile YAML and expressions, and the hand-written RFC 6455 frame parser in Go (Go's native fuzzing), run by `stress.yml` every night (#179 → #425, #624, #631) |
| Q5.3.6 | Unsafe code concentrated, checked before loading | Partly | `unsafe` is concentrated in dylib loading and process spawning; dylib checks verified by scripts; 5 items in `design-dylib-sdk` §13 unchecked, need reconciling |
| Q5.3.7 | Dependency vulnerability and license checks | Partly | Weekly `deps.yml`: `cargo deny check advisories`, `npm audit`, `pip-audit` (#256); does not block merging. Licenses are not checked; there is no fixed check for the reason behind a new dependency |
| Q5.4.2 | Residue checked at the end of each scenario | Partly | The E2E harness checks at the end of each scenario: processes the host started (Linux, macOS by scenario marker, Windows by Job Object), socket files, ports, temporary directory, credentials (#221, #277). fds and threads of the test process are checked only in the two soak tests; tokio task counts and, on Linux, OS thread counts are checked in `instance_subtrees.rs`, `instance_runtimes.rs` and `close_memory.rs` (#286, #191, #979); registrations inside runtimes are queried with the Node and Python runtimes' test control method `inspect`, used by S7 only (§5, #191) |
| Q5.4.3 | Resources do not grow with repeated creation and destruction | Partly | Two bridge-level soaks (links made and cut, processes started and ended, over and over; the test process's fds and threads); the host-process soak S8 (`tests/e2e/tests/soak/soak.rs`, #192): one `rutis-host run` runs for an hour each night, rows added, removed and changed with reloads, runtimes killed and restarted by the host, the link to a node cut, while the host, its runtimes and the node are sampled for process count, fds (handles on Windows), threads and resident memory; the median of the last 25 % of the time is compared with that of the early window (the first 10 % skipped as warm-up) and may not pass the bounds (one more process at most: the Node runtime's TypeScript loader starts an esbuild process when it needs one; the other bounds and why in `Bounds` in `soak.rs`), on Linux, macOS and Windows. Missing: instances created and destroyed over and over on the kernel's root (K5, #191); tokio task counts only in `instance_subtrees.rs`, `instance_runtimes.rs` and `close_memory.rs` |
| Q5.4.4 | No orphans when the host is killed | Met | `cross_language.rs` `kill -9`s the host on all three platforms and the residue checks confirm the runtimes exit (Unix through the channel closing, Windows through rutis-host's Job Object). On Unix, a runtime whose plugin keeps a timer, or whose cleanup never returns, also unloads its rows and exits within the deadline (#353, fixed by #354; `stray_runtime.rs`, `rutis-bridge/tests/platform/host_gone.rs`, Node, Python, Go and Bun). S8 ends hosts nightly on all three platforms by Ctrl-C (Ctrl-Break on Windows), SIGTERM and `kill -9` in turn; before a `kill -9` it adds a row whose cleanup never returns in each of the Node, Python and Bun runtimes, and checks that every process of the host is gone within a bound, the runtimes unloading their rows themselves and keeping the deadline (#192). It found that after its host was `kill -9`ed, the Python runtime could not write the line saying it gave up the cleanups (the read end of its stderr pipe went with the host) and never exited; fixed (the Python case in `stray_runtime.rs`) |
| Q5.5.2 | Conflict detection with several writers | Partly | The loader implements CAS and replay; the multi-writer tests listed in its design §17 need checking item by item |
| Q5.5.3 | Persisted formats forward compatible | Partly | `rutis.json` has a top-level `format` (absent means 1; newer than supported is refused with a hint to upgrade); `tests/fixtures/persisted/` holds samples from 0.8.0, 0.9.0, 0.9.1, and `rejected/` asserts the error messages; `docs/release.md` adds a sample per release (#207 → #284). By decision, dsh's `cordis.patch.yml` gets no version field (it follows Cordis, plan Q4) and relies on the samples alone, which does not fully match "every format has a version" |
| Q5.5.4 | Cross-language value round trips verified | Met | The conversion rules are in `guide/plugin-api.md`; `value_roundtrip.rs` generates values with proptest on the Rust side, round-trips them through Node, Python, Go and Bun and compares them, 256 cases each, printing the seed on failure, plus a release counting invariant (#296); the problems it found are fixed (#290 → #312, #302 → #311, #279 → #338). Values outside the table (Date, Map, Set, binary, big integers) are refused explicitly and registered as not guaranteed (plan Q5; whether to support them is #308). Runs on Unix only |
| Q5.6.1, Q5.6.3 | Error content and pinning | Partly | Common `rutis-host` errors are pinned word for word (`rutis-host/tests/config/errors.rs`, #205 → #217); a protocol version mismatch says which end to upgrade (#293 → #335); the category of a refused link appears in `run`'s output (#321 → #336). Library-level errors in the SDKs and the bridge are not pinned |
| Q5.6.2 | No branching on error text | Met | Connection errors are classified by structured category, with cross-implementation tests |
| Q5.7.1 | Performance figures marked as guarantee or reference | Partly | "About 30 µs per synchronous call" is not marked in the design philosophy; benchmarks have no fixed environment |
| Q5.7.2 | Declared scale | Not met | Nothing declared externally; the scale in §2.2 is an assumption to confirm. Continuous running time: S8 verifies nightly that one host shows no growing resources over an hour (#192); longer is not verified |

### 1.2 Component types

| Clause | Component | Status | Current state and gaps |
| --- | --- | --- | --- |
| Q6.1 | Core `rutis` | Partly | Contract, Cordis parity, interleaving tests; #177 completed event ordering (D24, D31, D29) and lifecycle and reclamation (D7, D8, dropped futures, task counts for instance-subtrees §5) (#280, #286, follow-up #345). Model checking with controlled scheduling: 28 shuttle models of the core (K2, #622, §7.3), plus 31 in `rutis-bridge`, 3 in `rutis-host` and 3 in `rutis-loader` (#622, #627, #628); all 65 models across four crates now run in `ci.yml` (1000 iterations on PRs) and `stress.yml` (a million nightly, #1372). Missing: no model for dylib; contract tests mostly use the single-threaded runtime only; D22's cycles and D28's isolate ownership are untested |
| Q6.2 | Session protocol, runtime control operations, node operations | Partly | Conformance suites; malformed frames, unknown ops, unknown references (`malformed`, 25 cases × seven implementations); protocol version mismatch (unit tests in three SDKs #288, message #335); multi-hop cancellation and release, fault injection (#285); the cross-runtime synchronous call policy chosen with TLA+ and implemented (#222, #261; models T1 and T5 in §7.2); the previous release meeting the current code (#209 → #426). Missing: resource limits on references and calls (waiting for design); cleaning the cancel table and a cancel acknowledgement are undecided (after a cancel the callee may or may not reply, and the caller must take either, `SESS-61`; #1099). Session frame decoding is fuzzed (#179 → #425) |
| Q6.3 | Local socket, inherited fd, loopback, WebSocket, memory channel | Partly | All Rust channels run the channel contract; the local channel has a size limit (#214). The Node and Python channel implementations do not run the channel contract (P1) |
| Q6.4 | Frame decoding, framing, handshakes, configuration, manifests, patch merging | Partly | Patch merging has differential verification (against cordis-plugin-include); the line framing limit and the nesting depth limit have boundary value tests (#214, #338); every `rutis.json` field has unit tests (#217); frame decoding (with the ten tagged values of 0.11, decode→encode→decode stable), local line framing and WebSocket upgrade requests are fuzzed, with the corpus in the repository (#179 → #425). Missing: fuzzing of configuration files (low priority) |
| Q6.5 | Parts that spawn processes and hold connections | Partly | The concurrent start race is fixed (#184 → #215); E2E scenarios end with process residue checks on three platforms; runtimes exit after their host is killed (Q5.4.4); long runs and every way of ending (S8, #192). Missing: the baseline of registrations inside runtimes in Q5.4.2 (no query, §5) |
| Q6.6 | `rutis-host`, `rutis.json`, project templates | Partly | Black-box tests: the E2E harness starts a real `rutis-host` (#221); S2 development loop, S3, S5, S9 installation (#253, #317, #322, #254) on three platforms; signal handling (#174 → #216); configuration, assembly and common error output (#217); projects generated from templates are installed, tested and run in S2. changing the configuration while running (#274). Missing: on Windows only Ctrl-Break is verified, not Ctrl-C or closing the console |
| Q6.7 | Node / Python SDKs and test tools | Partly | Both SDKs have test tools with a strict mode; the test tools and the real runtimes compare against the same expectations file (#206 → #298, `sdk/testing-matches-runtime`). Go's `rutistest` and Bun's test tool are not compared |
| Q6.8 | Node, Python, Go runtimes; Bun | Partly | Node, Python and Go pass conformance; on Windows the loopback and WebSocket columns of session conformance and of the runtime conformance matrix run (#176 → #295, #309; matrix in [ci.md](ci.en.md) §4). Cross-runtime synchronous calls: policy B implemented (#228 → #261). Bun: B1 merged (#200); B2 and B3 for admission under Q6.8 not done (#194); Windows is not a declared platform for Bun |
| Q6.9 | Service contracts across implementations | Partly | Method shapes can be declared; every language can name errors; value round trips have property tests (#296); the Node and Python test tools match the real runtimes (#298); replacement is verified end to end (S4, #326). Missing: comparing the Go and Bun test tools; Date, Map, Set, binary and big integers are always refused (#308, waiting for design). Whether implementations return the same data is out of scope (item 4, decided 2026-10-11) |
| Q6.10 | Loader editable layers, dsh profile files | Partly | Atomic writes and conflict replay implemented; the dsh user layer has samples per release (#284). S7: an embedding application's own `Persist` (a JSON file, written beside it and renamed, its content's hash as version) restores the editable layer after a real process restart, with the same state as before; edits refused or rolled back store nothing (#191). Multi-writer tests not checked item by item (Q5.5.2) |
| Q6.11 | Cordis binding generation | Met | The generator has unit tests; real plugin baselines are compiled and called; every night the newest published plugins are generated, compiled and called again, report only (F4, `cordis-latest` in `nightly.yml`, #190 → #429) |
| Q6.12 | dylib loading | Met | Linux and macOS run on PRs that change dylib code and nightly; Windows (`dylib-windows.yml`) runs nightly and in the release gate (not on pull requests, #569; ci.md §2). Reconciling the check before loading is under Q5.3.6 |
| Q6.13 | Node links, remote runtimes, leases | Partly | Library tests complete; two independent host processes verified over loopback (S5, `nodes.rs`, #322, #336): a call in flight during a network cut fails once, the link comes back, the far end restarted gives a new lease, a changed token is refused, a certificate for another name or an expired one is refused; between two containers over a real network (E12, #341 → #433, nightly, report only): non-loopback + TLS, calls succeed and run once and the link stays up under 200 ms delay + 5% loss, a call in flight during an outage fails once, the heartbeat timeout is reported, the link reconnects within the backoff bound. Missing: revoking credentials while running (#273); the link's actual retry timing and the reset after 60 s stable (E3). Leases, takeover, reconnection and revocation have TLA+ models (E4, E5, #210) |
| Q6.14 | Cordis mounts and Cordis nodes | Partly | Differential verification against native Cordis; boundary rules 3, 4, 5, 7 have cross-process tests (`boundary_rules.rs` and others, #283), and the silent degradation they found now fails explicitly (#281, #282 → #337). The black-box scenario S6 mounts published plugins from `rutis.json` (`tests/e2e/tests/install/cordis.rs`, #190 → #429): a plugin using a service of the rutis side makes the native calls, stops and restarts by native rules as the service is withdrawn and restored, and an uncaught error fails explicitly; published plugins mounted from `rutis.json` use each other's services by native rules and their services reach rutis by themselves, a probe's calls through them giving the native results (#428); new releases run through it every night, report only (F4). Missing: a `serial` across the boundary running as serial on the far side needs a protocol change (#340); `boundary/5.8` partly verified |
| Q6.15 | npm, PyPI, binaries, crates.io | Partly | `install.yml`: npm, PyPI and release binaries installed and run in a clean directory, on five platforms nightly, linux-x64 on PRs, five platforms before a release; after a release the guides' examples run with the registry packages (#193 → #254, #266 → #324). Missing: crates.io and the Go module have no installation check; marking the Python guide's examples (#266 (d), waits for #259) |
| Q6.16 | `rutis-dev` dev channel | Not applicable | `rutis-dev` removed in 0.19 (#694) |

### 1.3 Test rules, process, matrix, release

| Clause | Content | Status | Current state and gaps |
| --- | --- | --- | --- |
| Q7.1 | Determinism | Met | Assertions depending on the real clock removed: core #182 → #220; other crates and Node, Python, Bun, Go #231 → #257, #278, #299, #301; shared waiting helpers in `rutis-test-support`. Intermittent failures found since are recorded and fixed under Q7.3 (§9.2) |
| Q7.2 | Reproducibility | Met | `interleave.rs` and `stress.yml` print a seed per round, and the job summary gives the replay command (#220, #245); property tests and `fault_links.rs` print their seeds (#296, #285) |
| Q7.3 | No masking of failures | Met | CI does not retry; intermittent failures are recorded as issues and then fixed (ci.md §7). During 0.10: #289, #303, #306, #307, #314, #316, #342 and others recorded and fixed |
| Q7.4 | Background exceptions fail tests | Partly | Exceptions in background threads fail Python tests (#197 → #218); the E2E harness fails on a split probe line; the PR working rules include this item. Rust and Node tests have no common mechanism |
| Q7.6 | Black box and white box separate | Met | Black-box layer `tests/e2e`: the harness depends on no rutis crate and works only through the binaries, configuration files and probe plugins (#175 → #221); S4's fixture host is part of the system under test |
| Q7.7 | References can be regenerated | Partly | The patch reference file has generation instructions; CI does not check it |
| Q7.8 | Skips explained | Partly | Test files that run on some platforms only state the reason next to their `cfg`; skipped cells use `ignore = "reason"`; the conformance matrix is in ci.md §4 (#176); the residue checks report which checks did not run on the platform. The `cfg(unix)` of `rutis-dev/tests/channel.rs` gives no reason |
| Q8.1 | Three layers | Partly | Rules layer thorough; composition layer has black-box E2E and fault injection (S2–S5, `fault_links.rs`); environment layer has the support matrix, clean installation (S9) and a real network (S5 with containers + netem, #433); long runs (S8, #192) |
| Q9.1.1 | Designs state levels, risks, acceptance | Partly | Most designs have acceptance sections; the PR template gained items for persisted formats and the guarantee register (#284, #276); the former `model-pr.yml` (`tools/check-model-pr.mjs`) was retired in #1372. Missing: designs do not state levels or risks |
| Q9.3.2 | Incompatible changes detected; versions meeting across releases | Partly | `cargo semver-checks` only warns; a protocol version mismatch says which end to upgrade (#335); the previous release (runtimes, Go SDK, `rutis-host`, nodes) meeting the current code, asserted to work or to say at load or connection which end to upgrade (`tests/e2e/tests/install/cross_version.rs`, full runs, #209 → #426) |
| Q9.4 | Guarantee register | Partly | The [core promise register](guarantees.en.md) is trimmed to the 40 active core promises (P0, #1372), checked forward by `check-guarantees.mjs`; the 1,825-row static rule tracker (`docs/formal-trace/`) and `tools/formal-trace*.mjs` were retired in #1372 |
| Q10.2 | Minimum and latest versions in the support matrix | Met | Linux jobs use the minimum versions (Node 22, Python 3.12, websockets 15, Bun 1.4.0, Go oldstable), macOS and Windows the latest; MSRV 1.88 verified by the `msrv` job (#204 → #256; ci.md §9). The runtime × channel × platform matrix is in ci.md §4, with a reason for each cell that does not run (#176) |
| Q11.3.2 | No coverage gate | Met | — |
| Q12.1 | Verification at levels | Partly | PRs select by change; nightly (`nightly.yml`, when main has new commits) every job, installation on five platforms (`install.yml`), Windows dylib, MSRV, feature combinations, two-machine reproducibility, and `stress.yml` (core rounds, multi-process test rounds #245, two soaks, all 65 shuttle models across four crates at a million iterations each per night, nine fuzz targets #425, every event placement #464); versions meeting across releases (`e2e` in `ci.yml`'s full run, #209), the baseline with the newest published Cordis plugins (report only, #190), S5 with containers + netem (report only, #341 → #433); weekly dependency checks and CI measurements. Missing: weekly long host soaks and benchmarks (#192) |
| Q12.3 | Selection by change | Met | The `changes` job turns switches on by change; nightly `nightly.yml` runs every job when main has new commits (`ci.yml`'s `all`), and it can be run by hand (#203 → #213, #369; ci.md §3). Before a release: step 2 of release.md checks that the commit released has a passing nightly run; `release.yml` calling the full CI itself is step 3 of the CI redesign |
| Q12.4 | Cancel superseded runs | Met | A new push to a PR cancels the old run (#224). A merge to main runs nothing; the nightly run checks every commit merged since the last successful nightly run and is not cancelled; a failure opens an issue (label `ci`) naming the failed jobs and the commit range (#369; ci.md §7) |
| Q12.5 | One merge condition | Met | `ci-ok` is the only required check (#213; ci.md §5) |
| Q12.6 | Time as developers wait | Partly | Target: an ordinary code PR gets a result within 10 minutes of the push, queueing included (ci.md §1); the time requirements apply to PRs and releases only, not to the nightly run. Measured on 2026-10-12: median 15.6 minutes, p90 42.9 (9 runs, most before or during the move to self-hosted runners, §8.3). Not enough data since the move |
| Q12.7 | No duplication | Partly | Duplicate release builds removed (#265 → #275); on PRs the reproducibility check runs only when `repro` is on, as a single-machine check inside the dylib jobs, and the two-machine `sdk-repro*` comparison runs only in full runs (#213, #275). In full runs the single-machine and two-machine reproducibility checks both run |
| Q12.8 | Main line failures first | Met | Process written in ci.md §7 |
| Q12.9 | Measure CI | Met | `tools/ci-stats.mjs` and the weekly `ci-stats.yml` (#213); §8.1 and §8.3 are two measurements |
| Q12.10 | CI configuration is code | Met | CI changes go in their own PR and run every job (ci.md §6) |
| Q13 | Release gate | Partly | Version consistency check, packaging, two-machine smoke test, installation on five platforms before a release, verification with registry packages after a release (#254), persisted samples saved (#284), versions meeting across releases (the full `ci.yml` run the release gate calls, #209). Missing: checking the register starts with 0.11 (plan Q3) |

---

## 2. The project today

### 2.1 Parts

| Part | Content | Used directly by |
| --- | --- | --- |
| Core `rutis` | Plugins, fiber lifecycle, dependency gating, services, events | Rust embedders; every other part |
| `rutis-loader` | Manages a set of plugins from data (rows): layered configuration, imperative edits, persistence, instances | Rust embedders; rutis-host |
| `rutis-bridge` | Channels, the session protocol, language runtimes, node links, Cordis mounts | Rust embedders; rutis-host |
| `rutis-host` | A host without writing Rust: `run` / `dev` / `check` / `new` + `rutis.json` | Plugin authors; operators |
| Node packages | `@arcships/rutis` (plugin SDK and test tool), `@arcships/rutis-runtime` (runtime, Cordis node bridge), `@arcships/rutis-host` (distribution) | Plugin authors; Cordis applications |
| Bun package | `@arcships/rutis-bun` (Bun runtime, #194 B1) | Plugin authors |
| Python packages | `rutis` (plugin SDK, test tool, runtime), `rutis-host` (distribution) | Plugin authors |
| Go module | `github.com/arcships/rutis/go/rutis` (plugin SDK, `rutistest`, runtime) | Plugin authors |
| dylib toolchain | `rutis-sdk`, `rutis-dylib*` | Rust plugin authors |
| Applications | `rutis-dsh`, `rutis-agent` | Used in this repository; also the framework's first users |

### 2.2 Operating conditions and scale assumptions

| Condition | Value | Source |
| --- | --- | --- |
| Operating systems | Linux, macOS, Windows x64 (MSVC); Bun not on Windows | guide/README |
| Language environments | Node ≥ 22, Python ≥ 3.12, Rust ≥ 1.88, Bun ≥ 1.4, Go ≥ 1.24 | guides, 0.10 release notes |
| Optional dependencies | `websockets` ≥ 15 (Python networking) | pyproject |
| Processes | The host is one process; each language runtime is at least one; plugins may start their own children | multilanguage design |
| Network | Local: Unix socket / inherited fd / loopback TCP; across machines: WebSocket + TLS, possibly behind a reverse proxy, possibly disconnected, half-open, delayed | network stack design |
| Trust | Plugins trusted; networks and remote machines not fully trusted; running out of process is not a sandbox | remote design §7 |
| Versions | 0.x, a release every few days, minors may break; host, runtimes and SDKs upgrade independently | release.md |

**Scale assumptions** (to be confirmed; they affect the ratings of performance and soak risks in §4):

| Item | Assumed typical | Assumed upper bound |
| --- | --- | --- |
| Rows in one host | 10–50 | 500 |
| Language runtime instances | 1–3 | 10 |
| Nodes | 1–3 | 20 |
| Instances (tenants, sessions) in one embedding application | tens | thousands, created and closed repeatedly |
| Continuous running time | weeks | months |
| Hot reloads during development | hundreds per session | — |
| Cross-process call rate | tens per second | thousands per second (about 30 µs per synchronous call) |

### 2.3 Existing tests

Counted on 2026-11-15 (baseline `origin/main` `1dd73888`). Method: Rust counts `#[test]` and `#[tokio::test]` attributes, Node / Bun count `test(` / `it(`, Python counts `def test_`, Go counts `func Test`, Kotlin counts `@Test`, Swift counts `func test`. The line coverage column is still the measurement of 2026-10-10 on two platforms (#183, including Node / Python child processes started by Rust tests); it has not been re-measured and is here for magnitude only.

| Part | Tests | Test functions | Line coverage (2026-10-10) |
| --- | --- | ---: | ---: |
| Core `rutis` | 46 files: contract `contract.rs`, Cordis parity `parity.rs`, interleaving `interleave.rs`, the listener-table model `listener_table_model.rs` and others; 8 doctests, 4 `compile_fail` | 413 | 92.8% |
| `rutis-bridge` | 80 files; four conformance suites (channel / session / runtime / node); `fault` decorator; `trace_validation.rs`; two soak tests | 439 | 87.8% |
| `rutis-loader` | 61 files; patch merging against cordis-plugin-include; per-language row contracts (`*_rows.rs`) | 335 | 89.8% |
| `rutis-host` | 56 files, including the black-box tests under `tests/` | 291 | 69.8% (`main.rs` 0%) |
| `tests/e2e` | The black-box layer, depending on no rutis crate (Q7.6) | 91 | — |
| dylib toolchain | `tools/test-dylib*.sh`, `tools/test-sdk-bundle.sh`; SDK hashes compared across two runners | — | — |
| Node packages | `node/*/test/`, 48 files | 268 | 86% |
| Bun packages | `bun/rutis-bun/test/`, 23 files | 159 | — |
| Python packages | `python/rutis/tests/`, 27 files | 284 | 81% |
| Go module | `go/`, 39 files | 176 | — |
| Kotlin | `kotlin/`, 23 files | 124 | — |
| Swift | `swift/`, 4 files | 32 | — |
| Design models | `specs/`: 7 directories, 17 `.tla` modules, 224 configurations (§7.2) | — | — |
| Controlled scheduling | 65 shuttle models across four crates (core 28, `rutis-bridge` 31, `rutis-host` 3, `rutis-loader` 3), compiled only with `--cfg rutis_shuttle` (§7.3) | 65 | — |
| Fuzzing | `fuzz/fuzz_targets/`, 9 targets (§7.4); plus Go's native fuzzing | 9 | — |

2026-11-21 (#1373): the Rust integration tests are grouped by domain into 4–8 `tests/<suite>/main.rs` suites per crate, and the independent Cargo integration-test binaries go from 205 to 25; the test functions and assertions stay the same, and `cargo test --no-run` links fewer binaries.

What 0.11 and later added and still holds (details in §4, §5 and §7):

- cross-language value types (date and time, binary, Map, Set, big integers, Decimal, UUID, 0.11) and the value round-trip property test over seven languages;
- remote runtime dial-out, access points with Swift / Kotlin, iOS / Android app hosting (0.13–0.18);
- 65 shuttle models across four crates (core 28, `rutis-bridge` 31, `rutis-loader` 3, `rutis-host` 3), all covered by `ci.yml` and `stress.yml` (#1372); a fourth fuzz target (error graphs);
- `specs/` grown from 2 directories to 7 directories, 17 modules and 224 configurations;
- the first slice of trace validation (the session channel, `trace_validation.rs`);
- test clocks (`virtual_clock_contract.rs`, `design-test-clocks-2026-10-11.md`) and the previous release meeting the current code (`cross_version.rs`).

Added in 0.9 and 0.10 (historical record, as of `f8ce69b`):

| Part | Content | PR |
| --- | --- | --- |
| E2E harness `tests/e2e` | Starts real `rutis-host` processes and probe plugins in each language; depends on no rutis crate (Q7.6). Each scenario ends with residue checks: processes (Linux, macOS by the scenario marker `RUTIS_E2E_SCENARIO`, Windows by Job Object), socket files, ports, temporary directory, credentials in the output. Stops the host with Ctrl-Break on Windows; `descendants` lists the host's descendant processes; a TCP proxy (`cut`, `restore`) simulates network cuts | #221, #277 |
| Scenario S2, development loop | `dev_loop.rs`: `new` → template tests → `dev` → hot reload → a broken file → `check` → `run` → Ctrl-C; Node, Python, Bun, Go; three platforms (no Bun on Windows) | #253, #323 |
| Scenario S3, languages and crashes | `runtime_crash.rs`: Python, TS and Bun rows; the Python runtime `kill -9`ed during a call, the call fails explicitly, other runtimes' pids unchanged, a host restart recovers; `cross_language.rs`: TS ↔ Python, runtimes exit after the host is `kill -9`ed | #317, #221 |
| Scenario S4, moving an implementation | `migration.rs` + the fixture Rust host `tests/e2e/fixture-host`: `calendar` moves from Python to node B and then to Rust; `planner` unchanged, host pid unchanged, every call during a switch succeeds or fails explicitly, no residue of the old implementation | #326, #330 |
| Scenario S5, two host processes | `nodes.rs`: two hosts over loopback; network cut, the far end `kill -9`ed and restarted, a changed token, a certificate for another name or expired; an execution count on B confirms no retry | #322, #336 |
| Scenario S5, two containers (0.12, nightly) | `netem.rs`: two Docker containers, B listening on `wss://` on a non-loopback address (its certificate signed by a CA made at test time), A dialing it; netem 200 ms delay + 5% loss, an outage both ways and its recovery, a changed token; a call in flight fails once, an execution count on B, the heartbeat timeout reported, a reconnect within the backoff bound, nothing left in the containers after Ctrl-C to both. `netem.yml`, nightly, report only | #341 → #433 |
| Scenario S7, Rust embedding and multiple tenants (0.12) | `embedding.rs` + the fixture Rust application `tests/e2e/fixture-tenants` (assembled from the crates as the rust-host guide says, one instance subtree per tenant, with TS and Python rows): tenants do not see each other's services of the same name; an admin edit reaches only its tenant, and edits refused by validation or rolled back after failing to start apply nothing and store nothing; a closed tenant leaves nothing in the kernel, the loader or either runtime process; after a restart the editable layer restores the same state; creating and closing tenants over and over (20 times on pull requests, 3000 nightly) keeps processes, counts, descriptors and memory flat | #191 → #430 |
| Scenario S9, installation | `install.rs`: npm, PyPI and release binaries installed and run in a clean directory (`install.yml`); `doc_examples.rs`: the guides' examples run with registry packages after a release | #254, #324 |
| Windows and loopback columns of the conformance suites | Session conformance and the runtime conformance matrix no longer whole-file `cfg(unix)`; loopback and WebSocket columns run on Windows; added session contract × WebSocket and loopback, runtime contract × `Handover::Loopback`, node contract × Rust↔Rust WebSocket. Matrix in ci.md §4 | #295, #309, #313, #328 |
| Fault injection | `fault_links.rs`: the `fault` decorator on sessions and links (dropped mid-call, cancellation and release under delay, calls during a reconnect, multiple hops, interleaved replies, large and deep values); `concurrent_launch.rs` (concurrent starts) | #285, #215 |
| Property tests | `value_roundtrip.rs` (proptest): values generated on the Rust side, round-tripped through Node, Python, Go, Bun and compared; nesting depth limit; release counting invariant | #296 |
| Other protocol tests | `boundary_rules.rs` (boundary rules 3, 4, 5, 7); `sdk_protocol.rs` and protocol version 99 tests in both SDKs; Python release counting and grant rollback; Python column in `websocket_cross.rs`; negative TLS in Rust and Python | #283, #288, #297 |
| Core | `event_order.rs` (D24, D31, D29, panics in async waterfall); `lifecycle_order.rs` (D7, dropped restart / update futures); additions to `typed_plugin.rs`, `instance_subtrees.rs` | #280, #286, #345 |
| `rutis-host` | 8 files under `tests/`: signals, configuration, `check`, common error output, project templates, persisted samples, link state, whole output lines | #216, #217, #284, #336, #339 |
| Test tools and real runtimes | One expectations file, `weather.expected.json`: real runtimes through `session_matrix.rs`, one test each for the Node and Python test tools | #298 |
| Persisted samples | `tests/fixtures/persisted/`: 0.8.0, 0.9.0, 0.9.1, `rejected/` | #284 |
| TLA+ | The 7 model directories of `specs/`, 17 modules, 224 configurations (95 `*-mut-*` mutation configs skipped by default, 129 run, §7.2). CI's `specs` job triggers only when `specs/**` changes (#1372) | #222, #210, then #613, #1372 |
| Guarantee register | [guarantees.en.md](guarantees.en.md) trimmed to the 40 active core promises (P0), forward-checked by `tools/check-guarantees.mjs`; the 1,825-row rule tracker was retired (§7.6) | #276, #310 → #634 → #1372 |

CI: `ci.yml` (PRs select jobs by change, nightly runs everything; full Linux run on the minimum versions, E2E, lint, platform-specific tests on macOS and Windows, Bun and Go runtimes, dylib, packaging, semver, MSRV, feature combinations, reproducibility, the `specs` job over 7 models); `install.yml` (installation smoke tests); `nightly.yml` (calls the three above and `stress.yml` every night, opens an issue on failure); `stress.yml` (nightly: core rounds, multi-process test rounds, two soaks, all 65 shuttle models across four crates, 9 fuzz targets, every event placement); `dylib-windows.yml`; `deps.yml` and `ci-stats.yml` (weekly). Details in [ci.md](ci.en.md). CI still has no actionlint and no license checks.

## 3. Control codes

The risk tables in §4 refer to controls by these codes; how to choose among them is in standard §11.

| Code | Method | Finds | Does not find | Cost |
| --- | --- | --- | --- | --- |
| **U** | Unit tests | Logic errors in a single function or module | Problems between components or processes | Low |
| **K** | Contract and conformance suites (the existing channel / session / runtime / node suites, core contract / parity) | Deviations of different implementations and channels from one specification | Errors in the specification; behavior the suite does not cover | Medium (written once, reused by every implementation) |
| **X** | Differential tests against a reference implementation (native Cordis, cordis-plugin-include, dsh) | Deviations from the reference, including ones nobody thought to test | Errors in the reference; parts without a reference | Medium |
| **E** | Black-box end to end: start a real `rutis-host` or installed packages, drive with configuration files and probe plugins | Assembly, binary behavior, signals, exit codes, multi-process interaction | Hard-to-construct timings; internal state | High (needs a harness) |
| **R** | Residue check: processes, sockets, ports, fds, threads, tasks, registries at the end of a scenario | Resource leaks and orphan processes | Why something leaked | Low (written once, used everywhere) |
| **FI** | Fault injection: the `fault` channel decorator, killing processes, breaking configuration, occupying ports | Whether behavior under faults matches the guarantees | Faults not on the injection list | Medium |
| **FZ** | Fuzzing (cargo-fuzz) | Crashes, unbounded memory, endless loops when parsing external input | Logic errors (wrong results without a crash) | Medium (target written once, runs nightly) |
| **SH** | Controlled scheduling: takes over scheduling, systematically tries execution orders, reports the exact order on failure. Rust uses shuttle and Kotlin Fray; Python a steppable `SelectorEventLoop`; Node / Bun an injectable frame pump. Go and Swift have none | Races, lost wakeups, deadlocks between tasks / threads in one process | Wide interactions; external events not in the exploration (real processes, the OS, the runtime itself); code must be extracted into small models | Medium |
| **TLA** | TLA+ design model checking | Deadlocks and invariant violations in a protocol design | Implementation diverging from design | High (learning; specs maintained with the design) |
| **TV** | Trace validation: a real run's log is a trace, checked event by event against what the model allows | Whether the implementation follows the model's rules | Interleavings not run; an incorrect model | Medium (the implementation needs recording points first) |
| **PT** | Property tests (random input, checked properties) | Logic with clear properties: value round trips, counting invariants, matching rules | Behavior without clear properties | Low to medium |
| **KANI** | Bounded model checking (Kani): every input in a range of a tiny pure or `unsafe` function | Functions small enough to enumerate | Code that allocates, formats or needs a runtime (Kani 0.68.0 does not converge on this machine); concurrency | Medium (toolchain install; harnesses must stay tiny) |
| **SK** | Soak: long repeated operation with sampled resource curves | Slowly growing leaks | Errors that show up fast | Medium (CI time) |
| **MX** | Version and platform matrix: every declared OS, language version, dependency lower bound | Problems specific to a version or platform | — | Medium (CI time) |
| **IN** | Installation smoke test: install this build's packages in a clean directory and run the minimal path | Packaging, distribution, platform package selection | Functional logic | Medium |
| **G** | Output snapshots: error messages, status output, `check` output compared with stored expected text | Worse error messages, changed output formats | Whether the message is actually useful (needs review) | Low |
| **ST** | Static checks: fmt, clippy, `cargo deny`, feature combinations | Common coding errors, vulnerable dependencies, conditional compilation gaps | Logic errors | Low |
| **SV** | API compatibility: `cargo semver-checks`; migration guide code compiled and run | Undeclared breaking changes; outdated migration guides | Behavioral breakage | Low |
| **DOC** | Running documentation examples | Documentation out of step with reality | — | Medium |
| **DR** | Design review and checklists: acceptance sections, PR template items | Design gaps; changes without tests or docs | Depends on people | Low |
| **BM** | Benchmarks | Performance changes | Small changes under CI noise | Medium |

---

## 4. Risk analysis

Rating as in standard §4 (impact, likelihood, P0 / P1 / P2). The "Current" column: the scenario tables in §4.2 and the first two tables of §4.1 are the state on 2026-10-12; the third table of §4.1 (events, rows and the call lifecycle) was checked against the tree on 2026-11-15.

### 4.1 Base layer: core and protocols

Every scenario depends on this layer, so it is analyzed on its own.

#### Core `rutis`

| # | What can go wrong | Dimension | Impact | Likelihood | Pri | Control | Stage | Current |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| K1 | Lifecycle rules do not hold: a cleanup runs zero or two times, a plugin starts before dependencies are ready, wrong eviction order | Correctness | High | Medium | P0 | K (65 contract, 57 parity), U | PR | Yes |
| K2 | Races under concurrent interleaving: lost wakeups, driver exit races, a forked tail chain | Correctness | High | Medium (happened 3 times) | P0 | SH (28 models running the core's own code, not copies); multi-threaded repetition with seeds | PR (1000 iterations), nightly (a million per model) | Yes: `interleave.rs` seeded, `stress.yml` changes thread count and seed each round (#220); the 28 models in `fiber/shuttle_tests.rs` cover every join of a TransitionTask, the `post_join` exit race, concurrent dispose, publication order of the lineage marker and closing, effect registration and cleanup, unloading while listener calls are in flight, concurrent subtree shutdowns, root dispose and shutdown, dependency withdrawal, emit-chain order, the listener-table watcher, an unload cancelling the running generation, a restart publishing the live generation, the child a closing root mounted, a dispose result across the driver's exit (#1194), a restart inside the dependency window, spawned tasks on shuttle's executor, a mount racing the root's close, a deadline over two fibers' calls, a restart losing to a dispose and `Provided` before `Removed` (#179 → #425, #622, #628), three fibers restarting at once so `FiberStatusChanged` is delivered in order (`core/D24a`) and outside the transition lock (`core/D5a`) (#622), a mount plugin's cleanup effect and its service through the real reverse pass (#622) and a method-registered `Ctx`'s row unloaded while another thread registers (`MNT-11-9b`, #622). Building the models against the code confirmed two core defects (#619, #620, both fixed). Missing: no model for dylib; the session layer, the event host, runtime supervision and host reloads now have models in `rutis-bridge`, `rutis-host` and `rutis-loader` (§7.3), and all 65 models across the four crates now run in `ci.yml` and `stress.yml` (#1372); the forked tail chain (D31) still has only `dispatch_chain_probe.rs` |
| K3 | Event ordering guarantees only half hold: per-emitter order with several emitters, delivery after a panic | Correctness | Medium | Medium | P1 | K | PR | Yes (`event_order.rs`, #280) |
| K4 | Behavior written as "not guaranteed" changes silently (D29 events not filtered by isolate, D22 no cycle detection) | Correctness | Medium | Low | P2 | U (regression tests pin current behavior) | PR | Partly: D29 yes (#280); D22 no |
| K5 | Repeated creation and destruction on a long-lived root grows internal tables and tasks | Residue | High | Medium | P0 | U + R (task counts and table sizes back to baseline); SK | PR, nightly | Partly: U + R yes (task counts and capacity in `instance_subtrees.rs`, #286); tenants created and closed over and over in an embedding application have E (S7, 3000 nightly, #191); no host-level SK (#192) |
| K6 | A user callback panic crosses the boundary and kills a driver task | Faults | High | Low | P1 | U | PR | Yes (panic propagation in async waterfall, #280) |
| K7 | Wrong behavior without a tokio runtime or on a current_thread runtime | Compatibility | Medium | Medium | P1 | U (D8); contract tests in both flavors | PR | Partly: D8 tested (#286); contract tests mostly use the single-threaded runtime only |
| K8 | Dropping a waiting future interrupts a cleanup already started | Correctness | High | Low | P1 | U | PR | Yes (`lifecycle_order.rs`, #286) |
| K9 | `diagnostics()` disagrees with the actual state | Diagnosability | Medium | Medium | P1 | U | PR | Yes (`lifecycle_diagnostics.rs`) |
| K10 | Tests depend on machine speed: false alarms under CI load, or testing nothing on fast machines | — | Medium | High | P0 | Determinism rules (standard Q7.1) | PR | Yes (#220, #231) |

#### Channels and the session protocol

| # | What can go wrong | Dimension | Impact | Likelihood | Pri | Control | Stage | Current |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| P1 | A channel breaks its contract: lost or reordered messages, backpressure failing, close not waking | Correctness | High | Low | P1 | K (channel contract, 6 checks) | PR | Rust channels yes; Node and Python channels do not run the contract |
| P2 | Malformed frames, oversized lines, writes without newlines cause a crash or unbounded memory | Faults, security | High | Medium | P0 | U (boundary values); FZ (frame decoding, line framing) | PR, nightly | Yes: U (WebSocket and the local channel both have limits, #214); FZ's `session_frame`, `local_lines` and `error_graph` run nightly in `stress.yml` (#179 → #425, #624) |
| P3 | Unknown op, wrong field types, nonexistent reference ids make the session hang or are silently ignored | Faults | Medium | Medium | P1 | U (each of the seven implementations) | PR | Yes: the conformance suites' `malformed` check, 25 cases × seven implementations (#214; Go #305) |
| P4 | Reference counting errors: released too early (the far end's calls fail) or never (leak) | Correctness, residue | High | Medium | P0 | K; PT (after any grant / release sequence, table size = number not released) | PR | Yes (Python #288; PT #296; `grant_rollback.rs` grants nothing when a value cannot cross, a capability is missing, a limit is exceeded or a result cannot cross). #621 fixed a deadlock on this path: decoding with a mismatched reference kind dropped an `Import` while holding the `imports` lock, and its destructor took the same lock again |
| P5 | Call chain rewriting is wrong when forwarding across sessions: callbacks reach the wrong runtime or deadlock | Correctness | High | Medium | P0 | K (`rpc_callbacks.rs`, same call ids on both sides) | PR | Yes |
| P6 | Cancellation does not reach the far end, or is lost across hops | Correctness | Medium | Medium | P1 | K (`abortable`); one new multi-hop cancellation test | PR | Yes (`fault_links.rs`, #285) |
| P7 | Values convert differently across the three languages: `undefined` / `null` / absent, large integers, dataclasses, nesting depth | Correctness | High | High | P0 | PT (the same random values round-tripped Rust↔Node and Rust↔Python, compared); K | PR | Yes (`value_roundtrip.rs`, Go and Bun included, #296; fixes #312, #311, #338); Unix only |
| P8 | On a protocol version or capability mismatch, things hang or fail only after loading | Compatibility, diagnosability | Medium | High (runtimes and hosts upgrade independently) | P1 | U (each language); MX (previous release's runtime vs current host) | PR (U); nightly, release gate (MX) | Yes: U (#288; the message says which end to upgrade, #335); MX (#209 → #426: previous runtimes, Go binaries, host and nodes with the current code; a missing capability is refused at load saying which end to upgrade, a new value fails that call only) |
| P9 | During a synchronous call the far end calls back: Node should return `SyncWaitCycle`, Python should execute; behavior differs from the docs | Correctness | Medium | Medium | P1 | K (`reenter`) | PR | Yes |
| P10 | The cross-runtime synchronous call policy itself has holes (two non-reentrant runtimes calling each other) | Correctness | High | Medium | P0 | TLA (design time); then K with counterexample tests | Design | Yes: policy B chosen with TLA+ (#222), implemented and tested (`rpc_callbacks.rs`, `sync_cycles.rs`, #261). Two known limits: a runtime that does not declare itself reentrant is treated as non-reentrant; a runtime that does not declare `sync-wait` is outside detection |
| P11 | Exceptions in background threads or workers are swallowed and tests pass anyway | — | High | High (#197 happened) | P0 | Test rule: background exceptions fail tests (standard Q7.4) | PR | Partly: Python yes (#218); Rust and Node have no common mechanism |

#### Events, rows and the call lifecycle

These are the parts and protocols added in 0.13–0.18 (unified events, node links, app hosting, binary runtime supervision, marked services); §4 did not have them before 0.22. The ids and the controls come from #612; the "Current" column was checked against the current tree.

| # | What can go wrong | Dimension | Impact | Likelihood | Level | Controls | Stage | Current |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| EV1 | The guarantee between registration and dispatch (event protocol §4.4) differs between the fast path and the path through the host | Correctness | High | Medium | P0 | TLA (`specs/events/`); K (`events.expected.json`) | PR, design | Partly: `EventsTable.tla` / `EventsHost.tla` check "registered before emitted" on both paths, and `events.expected.json` is run by `events_conformance` in all seven languages (`stress.yml`'s `events-all` samples placements nightly, all of them on Mondays). Missing: filters on the fast path and the full semantics of the five dispatch modes (only `emit` is modelled) |
| EV2 | `emit` order broken by a path switch, a filter waiting or a non-reentrant runtime holding a call (happened: #513, #527, #557, #519, #554) | Correctness | High | Medium | P0 | TLA; K; nightly `events-all`; TV (#624) | PR, nightly | Partly: both order properties of `EventsEmit.tla` pass, and an unload dropping queued emits has a configuration; `emit-python-deferred` passes (`Defer = "nextRound"` only pushes the start to the next turn, and the model already allows any lateness); `before-future-reply` is built as the mutation `emit-mut-future-reply` (the host replies while the emit is still queued and none of its calls started), which violates `EmitOrder`; `events.expected.json` and nightly `events-all` exist. Missing: a configuration for the Node per-key hold; the event host's trace validation is not done |
| EV3 | After an unload or a disconnect, withdrawn entries still start calls, or a call in flight fails zero or twice; an unloaded row holds the queue (happened: #536 (fixed by #564), #587) | Correctness, residue | High | Medium | P0 | TLA; K; TV (#624) | PR | Partly: `EventsTable.tla`'s "only the row's current registration is accepted", plus the `table-reload` and `table-end-session` configurations; on the implementation side `rows.ended` is applied in frame order (#587) and unloading drops queued emits (#536). Missing: trace validation |
| EV4 | Offering and announcing on node links: entries of an old offer linger, an announcement that arrives before its offer is withdrawn (happened: #593; the root cause of #435 is registration inside the host, covered by `a_family_is_announced_only_with_its_offer` in `peer.rs`) | Correctness | Medium | Medium | P1 | TLA (`EventsLink.tla`); K (`crates/rutis-bridge/src/peer.rs`, the `node/` cases of `events.expected.json`, `node/rutis-runtime/test/cordis-events.test.mjs`) | PR | Partly: `EventsLink.tla` checks that only the highest version is kept, old offers are withdrawn by epoch and an early announcement is kept; `events.expected.json` has 5 `node/` cases. Missing: multiple peers and multiple hops are not modelled |
| EV5 | Synchronous dispatch (`bail`, `waterfall-sync`), the host's own synchronous calls and runtimes with a thread per call (Go, Swift, Kotlin) make the wait cycle missed or falsely reported (happened: #537) | Correctness | High | Medium | P0 | TLA (`specs/cross-runtime-sync/`); K (`crates/rutis-loader/tests/lang/sync_cycles.rs`); TV (#624) | PR | Partly: the 44 configurations of `cross-runtime-sync` include `Threaded` (a thread per call) and a host-initiated `host-call`; `sync_cycles.rs` has Node / Python / Go / Swift columns; the differential property test for `cycle_among` (`cycle_among_agrees_with_the_models_graph` and `the_model_check_agrees_on_the_small_graphs` in `waits.rs`) and the wait-graph trace validation (`waits/trace.rs` records, `waits/trace/check.rs` replays as `RustSucc`, run from `sync_cycles.rs` and `rpc_callbacks.rs`) are done. Missing: `sync_cycles` has no Kotlin column (Kotlin needs JDK 17 and `:rutis-conformance:fatJar`; #624) |
| RL1 | Row lifecycle between host and runtime: a late `rows.ended`, a refusal or a service announcement acting on another generation; a lease outliving its row (happened: a88fb65b, 54a7e824, d94aae9a, #247, #314, #372, #584, #191) | Correctness, residue | High | Medium | P1 | TLA (`specs/row-lifecycle/`); K (per-language row contracts, `crates/rutis-loader/tests/*_rows.rs`) | PR | Partly: the 36 configurations of `RowLifecycle.tla` check that one row runs at most one generation, a late ending ends only its own generation, a lease never outlives its row, and a service is withdrawn before its provider unloads; per-language row contracts exist for Node / Python / Bun / Go / Swift. Missing: `rows.schema` / `mount`, call chains, the differences between unload stacks and the shutdown deadline are not modelled; #1100 is decided (a key per load); #1101 (unload before load) is fixed (#1185), and both issues are closed |
| CL1 | Call lifecycle: after a cancellation the callee may reply or not (written down, `SESS-61`), and the caller's cancel table only grows; read threads writing back can deadlock when both sides back up | Correctness, residue | High | Medium | P1 | TLA (`specs/call-lifecycle/`); K (`rpc_callbacks.rs`, `multihop.rs`) | PR | Partly: the 35 configurations of `CallLifecycle.tla` / `CallRelay.tla` verify both cancellation-reply implementations as "settled at most once, no reference leaked", and prove the premise that read-thread write-back does not deadlock (read threads do not write directly), plus hop-by-hop cancellation through a relay. Missing: the caller's cancel table may only grow, and its cleanup and a cancel acknowledgement are #1099's; the relay's close and a peer still reading after close are not modelled |
| AH1 | App hosting: `RutisApp` starts and stops not in call order, a stopped port is not released, a restart sees the previous round's state (happened: #519, #521, #532, #554) | Correctness, residue | High | Medium | P1 | SH: Fray on Kotlin (#630); Swift has no controlled scheduling, so strict concurrency checking plus TSan (#630); TLA for the connection layer (`remote-lease`); simulator and device tests run locally ([CI](ci.en.md) §12) | PR, design | Partly: the `lease-app-restart` / `access-app-restart` configurations of `remote-lease` are built on "stop, then start", and the `RutisApp` thread interleaving is fixed in f57b4138; Kotlin's Fray cases (§7.3) do not cover `RutisApp`'s start and stop; Swift's TSan job runs AppTests (§7.3); iOS / Android app-hosting tests run locally only |
| AH2 | Swift / Kotlin takeover order and redial rules differ from the other languages | Correctness | Medium | Medium | P1 | Checking the TLA correspondence tables; K (#624) | PR | Partly: the correspondence table in `specs/remote-lease/README.md` now lists Swift / Kotlin; the Kotlin heartbeat start point is fixed (#1181), and the model is built to the correct rule with the old implementation kept as the `lease-mut-heartbeat-from-accept` counterexample. `sync_cycles` has no Kotlin column (#624) |
| RS1 | Binary runtimes (Go, Swift) and their supervision, host reloads, local runtime supervision: an ending before supervision starts lingers unbounded, a restart starts a new process before the old one ends, a missing binary is retried every round, a failed `dev` rebuild throws away the old build, a removed runtime waits for the wrong process (happened: #524, #563, #565, #566, 57f536a0) | Correctness, residue | Medium | Medium | P1 | SH (shuttle after the Launcher seam, #622); U (test clocks) | PR | Partly: `crates/rutis-loader/tests/rows/virtual_clock_contract.rs` and the [test-clock design](design-test-clocks-2026-10-11.en.md) cover the timing rules; a binary runtime's restart, stop and install are checked by the 3 models in `rutis-loader/src/runtime/binary/shuttle_tests.rs`, and the host waiting for a runtime's own process by the 3 in `rutis-host/src/restart/shuttle_tests.rs` and `rutis-host/src/host_shuttle_tests.rs` (#627, §7.3), now wired into `ci.yml` and `stress.yml` (#1372). Missing: `dev` rebuilds, the rounds and dylib still rest on real Go / Swift processes |
| SV1 | Marked services: a dylib rebuilt from the same version is still served by the old build, and a call by name picks its interceptor from the wrong row (happened: #522, #531, #534) | Correctness | Medium | Low | P1 | U; E | PR | Yes: `crates/rutis-bridge/tests/session/marked_services.rs` and `crates/rutis-loader/tests/rows/marked_services.rs` cover generations and interceptor selection for marked services; all three defects are fixed. Missing: no E2E scenario dedicated to marked services |

---

### 4.2 Scenarios

Each scenario first says what it is, which parts it goes through and under what conditions, then lists risks.

#### Scenario A: the plugin author's development loop

**What**: `rutis-host new` creates a project → unit tests with the SDK test tool → `rutis-host dev` runs it in a local host and reloads on change → `check` → publish to npm / PyPI → used by a host. Both TS and Python.

**Through**: `rutis-host` (new / dev / check), templates, SDKs and test tools, runtimes, loader reloads.

**Conditions**: three platforms; minimum and latest Node and Python; hundreds of reloads in a session; code often in a broken state.

| # | What can go wrong | Dimension | Impact | Likelihood | Pri | Control | Stage | Current |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| A1 | Test tool and real runtime disagree: a plugin passing in `load()` fails in a host (or the other way) | Correctness | High | Medium | P0 | X: run the runtime conformance fixture (`conformance-weather`) both through the test tool and in real runtimes and compare | PR | Partly: Node and Python yes (one expectations file, #298); Go `rutistest` and Bun no |
| A2 | A generated project does not install, test and run as is | Compatibility, docs | Medium | High (dependencies and toolchains move fast) | P1 | E (S2 steps 1–2); MX | Merge | Yes (S2, #253) |
| A3 | After a reload the old code still runs, or the old instance is not unloaded | Correctness, residue | High | Medium | P0 | E + R | PR | Yes (S2, #253) |
| A4 | A syntax error crashes `dev`, or the old version stops serving | Faults | Medium | High | P1 | E (break a file, assert the old version keeps serving and `cannot reload` is printed) | PR | Yes (S2, #253) |
| A5 | Resource use grows after hundreds of reloads | Residue | Medium | Medium | P1 | E + R, looped N times | Nightly | No |
| A6 | `check` misjudges: errors on something that runs, passes something that does not | Diagnosability | Medium | Medium | P1 | G (a set of good and bad projects, output and exit codes compared) | PR | Yes (`rutis-host/tests/config/check.rs`, #217; #258) |
| A7 | Error messages do not say which file, line or service | Diagnosability | Medium | High | P1 | G (snapshots of common errors); DR | PR | Yes (`rutis-host/tests/config/errors.rs`, #217) |
| A8 | On Windows, paths, venv (`Scripts\python.exe`), line endings parsed wrong | Compatibility | Medium | Medium | P1 | E on Windows | PR | Yes (S2 on Windows, #323) |
| A9 | On a plugin API version mismatch, the error does not say what to upgrade | Compatibility, diagnosability | Medium | Low | P2 | U (both SDKs) | PR | Yes |
| A10 | A declared method shape disagrees with the implementation (declared sync but returns a Promise) and the test tool misses it | Correctness | Medium | Medium | P1 | U (test tool strict mode) | PR | Yes |
| A11 | Python re-imports only the entry module (known limitation); users expect changes to other modules to apply | Docs | Low | High | P2 | Documented; `dev` hints when other modules change | — | Documented |
| A12 | The publishing workflow in templates does not work | Docs | Low | Medium | P2 | ST (actionlint on workflow syntax) | PR | No |

#### Scenario B: operators deploying with rutis-host

**What**: install from npm / PyPI / binary / crates.io → write `rutis.json` → `rutis-host run` under a process manager such as systemd → edit configuration while running → stop, restart, upgrade.

**Through**: distribution packages, `rutis-host` configuration parsing and assembly, loader, runtimes, nodes.

**Conditions**: three platforms; may be ended by SIGTERM or SIGKILL; configuration may be wrong; runs for weeks.

| # | What can go wrong | Dimension | Impact | Likelihood | Pri | Control | Stage | Current |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| B1 | A channel's package is broken: platform package not selected, wheel missing files, binary does not run on a clean machine | Compatibility | High | Medium | P0 | IN (three platforms × npm / PyPI / binary, this build's packages); again with registry packages after release | Merge, after release | Yes (S9, #254, #324); marking the Python guide's examples waits for #259 (#266) |
| B2 | On SIGINT / SIGTERM (Windows: Ctrl-C, Ctrl-Break, console close) cleanup does not run; exit codes differ from the docs | Residue | High | High (#174 happened) | P0 | E + R (each signal in turn) | PR | Partly: SIGINT and SIGTERM on Unix yes (#216); Ctrl-Break on Windows yes (#277, #324); Ctrl-C and console close on Windows no |
| B3 | After the host is SIGKILLed, runtime children are orphaned | Residue | High | Medium | P0 | E + R (Unix through channel close, Windows through a Job Object) | PR | Yes: on three platforms (`cross_language.rs`); on Unix, runtimes whose event loop stays busy or whose cleanup never returns also exit within the deadline (#353, #354; `stray_runtime.rs`, `host_gone.rs`) |
| B4 | After a configuration is broken, old rows are unloaded and new ones do not start | Faults | High | Medium | P0 | E (write a broken configuration, assert the old version keeps serving); loader K exists | PR | Loader yes; `run` reads its configuration again while running (SIGUSR1, `--watch-config`, #274), all or nothing; E: `host/reload-config` |
| B5 | `rutis.json` parsing: unknown fields silently ignored, wrong base for relative paths, wrong precedence of credential environment variables | Correctness, diagnosability | Medium | Medium | P1 | U (each field); FZ (configuration, low priority) | PR | Yes (U, `rutis-host/tests/config/config.rs`, #217; #226); no FZ |
| B6 | Missing runtime package, interpreter not found, port in use, bad certificate file give insufficient errors | Diagnosability | Medium | High | P1 | G (a snapshot per error) | PR | Yes (`errors.rs`, #217) |
| B7 | Credentials appear in logs, errors, status output, traces | Security | High | Low | P1 | Common E assertion: token values absent from all captured output | PR | Yes (E2E residue checks, #221, #277; S5 #322) |
| B8 | A listener without TLS binds to a non-loopback address | Security | High | Low | P1 | U | PR | To check |
| B9 | Status output format changes break operators' scripts | Compatibility | Low | Medium | P2 | G | PR | Partly: the connection rows' state lines and the listening address line are tested (`rutis-host/src/connect/dial.rs`, `remote_runtimes.rs`, #336) |
| B10 | When the process manager restarts the host, the previous socket file or port is not released and the new process fails to start | Residue | Medium | Medium | P1 | E (restart right after kill -9) | PR | Partly: restarting with the same configuration right after a Ctrl-C stop, binding the listener's port again (S3, #317); restarting right after the host is `kill -9`ed is not verified (the residue checks only confirm the ports can be bound again) |

#### Scenario C: composing languages

**What**: in one host, Rust, TS and Python plugins provide and use each other's services. Design philosophy §2: "multilanguage plugins are not an add-on; they are the reason it exists".

**Through**: core gating, loader rows, two-phase readiness (`SessionRows`), runtime processes, session forwarding, service projection.

**Conditions**: several runtimes cold-starting together; runtime processes may crash; synchronous and asynchronous calls interleave; three platforms (Windows over loopback TCP).

| # | What can go wrong | Dimension | Impact | Likelihood | Pri | Control | Stage | Current |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| C1 | Runtimes wait for each other on a cold start | Correctness | High | Low | P1 | K (the M1/M2 boundary conformance test) | PR | Yes |
| C2 | A row starts before its declared dependencies are complete (two-phase readiness fails) | Correctness | High | Medium | P0 | K (`runtime_rows.rs`, resolve before start) | PR | Yes |
| C3 | One runtime crashing affects others, or unrelated rows stop | Faults | High | Medium | P0 | K (M2 §3.4); E (kill -9, assert other runtimes' pids unchanged) | PR | Yes (S3, #317; no Bun row on Windows) |
| C4 | When a runtime crashes, calls in flight hang instead of failing explicitly | Faults | High | Medium | P0 | FI (the conformance fixture's `crash()` during a call) | PR | Yes (S3 #317; `fault_links.rs` #285) |
| C5 | Values convert inconsistently across languages (see P7) | Correctness | High | High | P0 | PT | PR | Yes (#296) |
| C6 | Two Node runtimes deadlock on crossing synchronous calls (known limitation); the limitation grows with a third language | Correctness | High | Medium | P0 | TLA (choose a policy); docs; K counterexamples once chosen | Design | Yes: policy B, the host detects wait cycles and returns `SyncWaitCycle` (#222, #261); limits in P10 |
| C7 | The Python runtime is reentrant, so a plugin's service is called in the middle of its own synchronous call and its state is changed concurrently | Correctness | Medium | Medium | P1 | Docs; optionally simulate reentry in the test tool's strict mode | — | Documented |
| C8 | Wrong withdrawal order: providers stop before consumers | Correctness | High | Low | P1 | K | PR | Yes |
| C9 | Direct objects within one runtime vs proxies across runtimes differ beyond what the docs say | Correctness | Medium | Medium | P1 | K (`multilang.rs`) | PR | Yes |
| C10 | Any of the above fails on Windows (loopback TCP + token handover) | Compatibility | High | Medium | P0 | MX: the loopback and WebSocket columns of the conformance suites run on Windows | PR | Yes (#295, #309; matrix in ci.md §4; Bun does not support Windows) |
| C11 | Starting several runtimes concurrently in one process fails intermittently | Faults | Medium | High (#184 about 50% locally) | P0 | FI: N threads starting together, 100 rounds | PR | Yes (`concurrent_launch.rs`, #215) |
| C12 | A runtime writing heavily to stdout / stderr fills the pipe and blocks | Faults | Medium | Low | P2 | FI (fixture writing continuously) | Nightly | No |

#### Scenario D: embedding in a Rust application, multi-tenant instances

**What**: a Rust application assembles the host from crates and provides its own Rust services; creates instance subtrees per business object (tenant, session, document), with one loader managing global and instance rows; configuration is persisted and restored after restart.

**Through**: core instance subtrees, loader instances, persistence, `Persist` implementations, instance scopes inside runtimes, instances in nodes.

**Conditions**: long-lived process; tens to thousands of instances created and closed repeatedly; embedders configure tokio differently; embedders upgrade crate versions.

| # | What can go wrong | Dimension | Impact | Likelihood | Pri | Control | Stage | Current |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| D1 | Service visibility leaks between instances: tenant A reads tenant B's service | Security, correctness | High | Low | P1 | K (`instances.rs`, `instance_services.rs`, `instance_runtimes.rs`) | PR | Yes |
| D2 | Residue after closing an instance: core registrations, registrations inside runtimes, export slots, proxies | Residue | High | Medium | P0 | U + R (registrations inside runtimes too, not only diagnostics) | PR | Yes: `instance_runtimes.rs`; in S7, after a tenant closes, the plugins under its instance, the bindings with its instance key, the loader's entries, and in the Node and Python runtime processes the rows, services, host proxies, export slots, object handles, Cordis plugins and fibers are back to their numbers without it, and none of its ids is left (the runtimes' test control method `inspect`, #191) |
| D3 | Thousands of create/close cycles grow tasks, memory, table capacity | Residue | High | Medium | P0 | SK (1000 rounds, task counts and table capacity back to baseline) | Nightly | Yes: task counts and capacity for instance-subtrees §5 have U (#286); S7 creates and closes tenants over and over next to a resident one (20 times on pull requests, 3000 nightly): the same processes, the numbers held in the kernel, the loader and the runtimes back to baseline, at most 8 more descriptors, memory within a threshold (the application: 1 MiB and 256 B per cycle). Found and fixed this way: every language row loaded left a task in the application waiting for its runtime's session to end, and the host side's table of export slots only grew (about 1.4 KiB per cycle, #191); `instance_runtimes.rs` asserts the task count back to baseline. The instance resource baseline (`close_memory`) runs in a debug build on pull requests and as `stress.yml`'s `memory` job in a release build nightly: one instance of eight rows went from 62.0–72.7 KiB to 49.6–60.3 KiB (each row's monitor future from 1,872 B to 344 B), and closing 40 to 1000 leaves 18–58 KiB (bounds: 61.5 KiB per instance running on Linux, 78 KiB on macOS, 128 KiB + 128 B per instance after closing; `the_monitor_future_stays_small` also asserts the monitor future is at most 512 B), with fibers, service bindings, listeners, tasks and, on Linux, OS threads back to baseline; the composition, what can be saved, before/after and what it does not cover are in [an instance's resident footprint](performance-instance-footprint-2026-11-02.en.md). Found and fixed this way: the loader's `running`, `groups`, `instances` and follower tables kept their peak capacity after instances closed (about 6 MB for 1000, now about 22 KB, #979) |
| D4 | One edit affects several instances, some succeed and some fail | Correctness | High | Low | P1 | K | PR | Yes |
| D5 | After persisting and restarting, state differs from before | Correctness | High | Medium | P0 | K (restart consistency in loader §17); E (S7) | PR | Partly: E yes, S7 restarts the application, which opens its tenants again from the editable layer alone, with the same configuration, row states and answers of each language's services as before, and starting does not rewrite the store (#191); §17 still needs checking item by item |
| D6 | Several processes write the same storage and changes are lost | Correctness | High | Medium (dsh's CLI and web UI open together) | P0 | K (loader multi-writer CAS, replay drops); PT (random edits with simulated conflicts, final storage = all edits) | PR | To check |
| D7 | Undeclared breaking changes in public APIs | Compatibility | Medium | High (0.x) | P1 | SV (semver-checks exists); migration guide code compiled | PR | Partly |
| D8 | Embedders calling a synchronous method from a current_thread runtime or a blocked thread deadlock | Faults | Medium | Medium | P1 | K (the network stack's requirement that the channel progresses while the caller's current_thread is blocked) | PR | To check |
| D9 | Builds fail with only some features enabled | Compatibility | Medium | Medium | P1 | ST (feature combinations, the nightly `checks` job) | PR | Yes |
| D10 | `shutdown` does not finish by its deadline and embedders cannot tell the state | Diagnosability | Medium | Low | P2 | U | PR | Yes |
| D11 | Latency grows abnormally at the upper bound of instances | Performance | Medium | Low | P2 | BM (create, close, read services with 1000 instances) | Weekly | No |

#### Scenario E: nodes across machines and remote runtimes

**What**: nodes connect over WebSocket + TLS, import and export services, run plugins for each other, forward events; a GPU machine runs only a Python runtime, managed by the main node.

**Through**: WebSocket transport, identity, link, session, node feature plugins, remote runtime leases, `peer:` rows in the loader.

**Conditions**: real networks (delay, loss, disconnection, half-open, NAT, reverse proxies); the two ends may run different versions; credentials may be revoked; remote machines are not fully trusted.

| # | What can go wrong | Dimension | Impact | Likelihood | Pri | Control | Stage | Current |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| E1 | When the network drops, calls in flight report success, or are retried automatically so side effects run twice | Faults | High | Medium | P0 | FI (`fault::drop_and_close` after frame N, the far end counts side effects) | PR | Yes (`fault_links.rs` #285; between two processes in S5, #322) |
| E2 | A half-open connection is not found within 30 s | Faults | Medium | Medium | P1 | FI (`half_open` with a paused clock) | PR | Yes |
| E3 | Reconnection backoff differs from the docs (from 0.5 s, capped at 30 s, ±20%, reset after 60 s stable; 30 s after rejected credentials; stop on incompatibility) | Correctness | Medium | Medium | P1 | U (paused clock, exact intervals asserted) | PR | Partly: the tests of `rutis-host/src/connect/dial.rs` cover the backoff sequence (from 0.5 s, 30 s cap, ±20%, reset), the reset after 60 s stable and the 30 s after rejected credentials (paused clock) |
| E4 | During takeover of a remote runtime, old and new leases coexist; late instructions of the old session act on the new lease | Correctness | High | Low | P1 | TLA (design); K (`crates/rutis-host/tests/dial/lease_e2e.rs`, Python, Go and Bun) | Design, PR | Yes: TLA+ (`specs/remote-lease/`, #210) checks at most one lease during takeover, at most one instance per id at an access point, and that an old session's commands never act on a new lease; K (`crates/rutis-host/tests/dial/lease_e2e.rs`, Python, Go and Bun) |
| E5 | Connections still possible after credentials are revoked; a registration revoked mid-handshake still lets the old handshake hand over | Security | High | Low | P1 | K (`link.rs`); TLA | PR | Partly: TLA+ (#210) checks that an old handshake is not handed over after its registration is revoked; the identity layer's hook for revoking while running (#273) was removed in 0.19 (no production caller): existing connections are disconnected by stopping that row, changing a token takes a restart of rutis-host, and S5 has no such step |
| E6 | TLS check holes: still connects with an expired certificate, wrong CA, hostname mismatch | Security | High | Medium | P0 | U (three negative cases, Rust and Python each) | PR | Yes (#297; S5 step 6, #322) |
| E7 | A malformed WebSocket upgrade request crashes the listener | Security, faults | High | Medium | P0 | FZ | Nightly | Yes: FZ `websocket_upgrade` (tungstenite's handshake and the listener's reading of path, bearer token and subprotocols), 20 minutes nightly (#179 → #425) |
| E8 | Rejection categories differ between implementations (Python↔Rust) | Compatibility | Medium | Medium | P1 | K (a Python column in `websocket_cross.rs`) | PR | Yes (#288) |
| E9 | Cancellation and release not passed hop by hop | Correctness, residue | Medium | Medium | P1 | K (new multi-hop cancellation and release tests) | PR | Yes (`fault_links.rs`, #285) |
| E10 | Service announcements arrive out of order and an old one restores a withdrawn service | Correctness | Medium | Low | P2 | U (constructed reordering) | PR | To check |
| E11 | Behavior between two independent host processes (not within one test process) differs from library-level tests | Correctness | Medium | Medium | P1 | E (S5: two `rutis-host` processes over loopback) | PR | Yes (S5, #322, #336) |
| E12 | Under real conditions (200 ms delay, 5% loss), timeouts and frequent reconnects | Faults, performance | Medium | Medium | P1 | E + containers + netem | Nightly | Yes (S5 in containers, `netem.rs`, #341 → #433, nightly, report only): under 35 s of 200 ms delay + 5% loss the link stays up and no call repeats; after an outage the heartbeat timeout is reported and the link reconnects within the backoff bound. Found and fixed: a Python `listen:` with its controller connected did not stop on Ctrl-C and skipped its cleanups on SIGTERM; B printing websockets' traceback on an outage is #434 |
| E13 | When the two ends run different versions, the incompatibility message does not tell which end to upgrade | Compatibility, diagnosability | Medium | High | P1 | MX (previous release's node vs current); G | Nightly, release gate | Yes: the message says which end to upgrade (#335); MX (#209 → #426: a previous `rutis-host` node and the current one link in both directions, services work both ways, a new value fails that call only) |
| E14 | A remote runtime accepts `file:` or absolute plugin paths | Security | High | Low | P1 | U | PR | To check |
| E15 | Resource use grows as one link drops and reconnects repeatedly | Residue | Medium | Medium | P1 | SK (`websocket_soak.rs` exists) | Nightly | Yes |

#### Scenario F: existing Cordis plugins and Cordis applications

**What**: two ways — Cordis plugins as rutis-loader rows; Cordis plugins mounted in a Rust application with Rust types generated at build time. Existing Cordis applications joined as nodes (`@arcships/rutis-runtime/bridge`) were removed in 0.19: a Cordis application runs its plugins in the Node runtime.

**Through**: Node runtime, Cordis 4.0.4, the generator (`generate.mjs`), service projection, event forwarding, boundary rules.

**Conditions**: plugins are third-party published npm packages whose versions users pin and which keep changing; Cordis itself changes too.

| # | What can go wrong | Dimension | Impact | Likelihood | Pri | Control | Stage | Current |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| F1 | Mounted behavior differs from native Cordis | Correctness | High | Medium | P0 | X (`dsh_baseline.rs` compares the same scenarios with native Cordis) | PR | Yes |
| F2 | Generator type mapping errors (optional vs nullable, newtypes, unions of live objects) | Correctness | High | Medium | P0 | U (15 in `generate.test.mjs`); generate from baseline plugins, compile and call | PR | Yes |
| F3 | Violations of boundary rules 3, 4, 5, 7 not detected, silently degraded | Correctness | Medium | Medium | P1 | K (one cross-process test per rule, compared with native) | PR | Yes (`boundary_rules.rs` #283; silent degradation now fails explicitly, #337); the protocol change for `serial` across the boundary waits for design (#340) |
| F4 | A new Cordis or plugin release breaks mounting | Compatibility | Medium | High | P1 | X: nightly baseline with the latest published versions (lock files unchanged), report only | Nightly | Yes (`cordis-latest` in `nightly.yml`: the library mount against native Cordis, the generated bindings, S6; #190 → #429) |
| F5 | After the Node process exits on an uncaught plugin exception, services are not withdrawn or dependents do not stop | Faults | High | Medium | P0 | K | PR | Yes |
| F6 | After moving to another directory (`RUTIS_CORDIS_ROOT`), the runtime or plugins cannot be found | Compatibility | Medium | Medium | P1 | E (build, copy elsewhere, run) | Merge | No |
| F7 | Members that cannot be bound are not reported at build time | Diagnosability | Low | Medium | P2 | G (snapshot of build warnings) | PR | To check |
| F8 | Mounting through `rutis.json` (not at library level) behaves differently | Correctness | Medium | Medium | P1 | E (S6) | PR | Partly (S6, `tests/e2e/tests/install/cordis.rs`, #190 → #429); found and fixed: published plugins could not use each other's services by native rules, nor offer them to rutis (#428) |

#### Scenario G: moving an implementation (try → prove → solidify)

**What**: design philosophy §4: `planner` depends on the service name `calendar`; the implementation moves from a Python plugin to a plugin on an intranet node, then to Rust; through all three stages `planner` changes no line and the host does not restart.

**Through**: loader edits, the service name catalog, core eviction and reload, node imports, runtimes.

**Conditions**: implementations of the same service written by different people in different languages; the switch happens while running.

| # | What can go wrong | Dimension | Impact | Likelihood | Pri | Control | Stage | Current |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| G1 | Implementations of the same service in different languages return differently shaped data (field names, optional fields, error types); consumers break after the switch | Correctness | — | — | Out of scope | The service's authors are responsible (standard Q6.9.4, decided 2026-10-11). The part rutis owns: the same value converts the same way between languages (P7), and test tools behave like real runtimes (A1) | — | — |
| G2 | Calls hang during the switch | Faults | Medium | Medium | P1 | E (S4: continuous calls during the switch, each succeeds or fails explicitly) | PR | Yes (S4, #326; stages 1 and 2 also with `rutis-host run` + SIGUSR1, #274) |
| G3 | The old implementation leaves residue after the switch (processes, links) | Residue | Medium | Medium | P1 | E + R | PR | Yes (S4, #326) |
| G4 | Consumers do not restart and keep the old implementation | Correctness | High | Low | P1 | K (core eviction) | PR | Yes |
| G5 | The host needs a restart during the switch (breaks the guarantee) | Correctness | High | Low | P1 | E (host pid unchanged) | PR | Yes (S4, #326) |

#### Scenario H: an agent writes a plugin, connects it, iterates

**What**: the main line of design philosophy §1: whatever the host needs to reach, an agent writes a plugin for, tries, changes, replaces.

**Today**: a product capability is missing. `design-session-persist-and-self-tools` §3 says "no dynamic loading of new code"; the agent's tools come from `ToolRegistry` and are not connected to rutis services (#185). Most controls for this scenario wait for that design. Risks known now:

| # | What can go wrong | Dimension | Impact | Likelihood | Pri | Control | Stage | Current |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| H1 | A plugin written by an agent loops forever, blocks the event loop or crashes the runtime, taking other plugins in the same runtime down | Faults | High | High | P0 (when the capability ships) | Design: agent-written plugins go into a separate runtime instance by default; E | Design | No design |
| H2 | Agent-written code runs with the host's privileges (not a sandbox) | Security | High | High | P0 (when the capability ships) | Design decision (design philosophy §8 "permissions"); DR | Design | Open question |
| H3 | Why loading failed is not returned in a form the agent can use | Diagnosability | Medium | High | P1 | G (snapshots of structured error output) | PR | No |
| H4 | Resource use grows after hundreds of generate/load/unload cycles | Residue | Medium | Medium | P1 | SK + R | Nightly | No |
| H5 | Tests with real models are not repeatable | — | Medium | High | P1 | Drive with a scripted model backend (`ScriptedLlm`); real models only in `#[ignore]` manual tests | — | Scripted backend exists |

#### Scenario I: dylib plugins

**What**: Rust plugins compiled as dynamic libraries, loaded after the host checks their identity.

**Through**: `rutis-sdk`, `rutis-dylib`, the launcher, `xtask`.

**Conditions**: toolchain versions, build paths and platforms vary; loading the wrong library can cause memory errors.

| # | What can go wrong | Dimension | Impact | Likelihood | Pri | Control | Stage | Current |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| I1 | A library with mismatched identity (SDK, toolchain, artifact hash) is loaded and its initializers run | Security, faults | High | Medium | P0 | E (`tools/test-dylib*.sh`, `test-sdk-bundle.sh`) | PR (Linux, macOS) | Yes; 5 items in `design-dylib-sdk` §13 unchecked, need reconciling |
| I2 | After a failed swap the old version stops serving | Faults | High | Low | P1 | E | PR | Yes |
| I3 | SDK builds are not reproducible | Compatibility | Medium | Medium | P1 | Two runners build independently and compare hashes | Nightly (on PRs, a single-machine check only when `repro` is on) | Yes |
| I4 | dylib problems on Windows | Compatibility | Medium | Medium | P1 | E (dylib-windows) | Nightly and the release gate | Yes |
| I5 | A toolchain mismatch is not caught by `xtask dev` before compiling | Diagnosability | Medium | Medium | P1 | E | PR | No (#260, outside this plan) |
| I6 | The dev channel is present in production builds | Security | High | Low | P1 | ST (production builds contain no dev listener symbols or feature) | Merge | No |
| I7 | dev channel commands behave unclearly with missing arguments, missing targets, or a closing host | Diagnosability | Low | Medium | P2 | U (commands × errors) | PR | Partly (5 tests) |

#### Scenario J: upgrading rutis

**What**: users move from 0.x to 0.(x+1): Rust crates, npm packages, PyPI packages, `rutis-host`. Host, runtimes and plugin SDKs may not be upgraded together.

**Through**: the release train, protocol versions, plugin API versions, persisted formats, migration guides.

**Conditions**: 0.x allows breaking changes; plugin authors depend on SDK version ranges, not in step with hosts.

| # | What can go wrong | Dimension | Impact | Likelihood | Pri | Control | Stage | Current |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| J1 | Code in a migration guide is outdated and following it does not compile | Docs | Medium | Medium | P1 | SV ("after" code from migration guides compiled and run; one exists in `migration_example.rs`) | PR | Partly |
| J2 | A host upgraded without its runtime (or the reverse) behaves unclearly: hangs, or fails after loading | Compatibility | Medium | High | P1 | MX: the previous release's runtime package with the current host must either work or fail clearly at start | Nightly, release gate | Yes (#209 → #426): Node, Bun, PyPI (local and remote) runtimes and Go SDK binaries with the current host, the previous host with the current runtimes; with the previous release (and the current one once published) all work or say at load which end to upgrade |
| J3 | Persisted configuration written by an older version (loader layers, dsh profiles) cannot be read by the new one, or changes meaning | Compatibility | High | Medium | P0 | U: each release stores a persisted sample as a fixture; new versions must read it | PR | Yes (samples from 0.8.0, 0.9.0, 0.9.1; `format` in `rutis.json`, #284) |
| J4 | Release train versions are inconsistent and users get two copies of the core | Compatibility | High | Low | P1 | ST (`scripts/train.mjs`) | PR | Yes |
| J5 | A new plugin SDK marks a higher plugin API and old hosts give an unclear error | Compatibility, diagnosability | Medium | Medium | P1 | U | PR | Yes |

#### Scenario K: long runs

**What**: a host runs for weeks or months, with configuration edits, reloads, runtime crashes and network reconnects along the way.

| # | What can go wrong | Dimension | Impact | Likelihood | Pri | Control | Stage | Current |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| L1 | The host's fds, threads, tasks, memory grow over time | Residue, performance | High | Medium | P0 | SK: host-process soak with random operations (edits, reloads, killing runtimes, dropping links), resource curves sampled, no sustained growth asserted | Nightly (1 h), weekly (longer) | Partly: S8 (#192) soaks the host for an hour each night on three platforms, sampling process count, fds, threads and memory of the host, its runtimes and the node, and asserting the late window stays within the early one plus a bound; the two bridge-level soaks besides. Missing: longer weekly runs |
| L2 | A slow listener makes the event backlog grow without bound | Performance | Medium | Medium | P1 | U (backlog diagnostics); docs | PR | Diagnostics yes, no limit (by design) |
| L3 | After runtime processes are killed repeatedly, host state or counters go wrong | Faults | Medium | Medium | P1 | SK (random operations include kills) | Nightly | Yes: S8's random operations (#192) `kill -9` the Node and Python runtimes over and over (restarted `on-failure`), and at each host's end check that its output agrees with what was done: how many `runtime <name>: down`, `restarting`, `running`, reloads accepted and rejected, link reconnects; no row failed, no restart given up |
| L4 | Failures in random operation sequences cannot be reproduced | — | Medium | High | P1 | Sequences generated from seeds; seed and sequence saved on failure | Nightly | Yes: S8's operations (#192) depend on the seed alone (not on timing, unit-tested); a failure writes the seed, the operations done and the resource samples into the scenario's `logs/` and prints a replay command (`RUTIS_S8_SEED`, `RUTIS_S8_OPS`); CI uploads them as an artifact |

---

## 5. Residue checks: approach and state

How standard Q5.4.2 is checked on the current implementation. The E2E harness implements five checks — processes, sockets, ports, temporary directory, credentials (`tests/e2e/src/residue.rs`, #221, #277) — and runs them at the end of every E2E scenario. After reporting residue it ends those processes, so a failed scenario leaves nothing running; checks that do not run on the platform are listed in the report (Q7.8).

| Item | How | State |
| --- | --- | --- |
| Every process the host started has exited | Linux, macOS: processes the harness starts carry the environment variable `RUTIS_E2E_SCENARIO` and are found by it (Linux reads `/proc/<pid>/environ`, macOS uses `KERN_PROCARGS2`), plus the host's process group and the children of everything found; on Linux the test process sets `PR_SET_CHILD_SUBREAPER` to reap orphans. Windows: the host runs in a Job Object whose process list must be empty | Implemented (E2E). On macOS the environment of Apple's own programs cannot be read, so such a program is missed once it leaves the host's group and is orphaned (stated in the module comment) |
| Socket files removed | No socket file under the scenario's temporary directory (Windows: by the `*.sock` name) | Implemented (E2E) |
| Ports released | `bind` on recorded ports (the proxies' included) succeeds again | Implemented (E2E) |
| The hosts' temporary directory is empty | — | Implemented (E2E) |
| Credentials absent from output | No token value of the scenario in any captured output | Implemented (E2E) |
| fds and threads of the test process | Back to baseline, tolerance to be decided (the existing soak uses +16); the sampling code in `local_soak.rs` is to be moved into a shared test module | Only the two soak tests |
| Live tokio tasks | `Handle::metrics().num_alive_tasks()` back to baseline | `instance_subtrees.rs` (#286), `instance_runtimes.rs` (#191) |
| Core registries | Fibers, bindings, listeners, event backlog in `diagnostics()` back to baseline | Asserted by some tests on their own; no shared check |
| Registrations inside runtimes | Runtimes provide a test query returning how many rows, proxies and export slots they hold | Node and Python runtimes: with `RUTIS_TEST_INSPECT` set they answer the control method `inspect` (rows, services, host proxies, export slots, object handles; Node also its Cordis plugins, fibers and services, Python its listeners), not part of the protocol; used by S7 (#191). Not in the Bun and Go runtimes |

## 6. Controls to build

The controls used by the P0 and P1 risks in §4, merged into what needs building or extending.

| Control | Risks served | State | Still to do |
| --- | --- | --- | --- |
| **Determinism rules** | K10, P11, and the credibility of every test | Yes (#220, #231; Python background exceptions #218) | No common mechanism for background exceptions in Rust and Node tests (P11) |
| **E + R: black-box end-to-end harness and residue checks** | A3–A5, A8, B2–B4, B10, C3, E11, F6, G2, G3, G5, H4 | Yes (`tests/e2e`, #221, #277; three platforms) | Checks for the test process's fds and threads and for registrations inside runtimes (§5); runtimes that outlive a killed host on Unix (B3) |
| **Scenarios** | As above | S2, S3, S4, S5 (loopback; containers + netem, nightly), S6, S7, S8 (nightly), S9 done | Waiting for design: S1 (#185) |
| **FI: fault injection on sessions and links** | C4, C11, E1, E2 | Yes (`fault_links.rs` #285; `concurrent_launch.rs` #215; S3, S5; repeated faults during long runs, S8 #192) | — |
| **MX: version and platform matrix** | C10, A8, E13, J2, P8 | Minimum and latest versions, MSRV (#256); loopback and WebSocket columns on Windows (#295, #309); the previous release with the current code (#209 → #426, Linux full runs) | Versions meeting across releases do not run on macOS or Windows |
| **IN: installation smoke tests** | B1 | Yes (#254, #324) | crates.io, the Go module; marking the Python guide's examples (#266 (d), waits for #259) |
| **PT: cross-language value round trips** | P7, C5, P4 | Yes (#296) | Does not run on Windows |
| **X: test tools vs real runtimes** | A1 | Node, Python yes (#298) | Go `rutistest`, Bun test tool |
| **X: nightly baseline with the latest Cordis plugins** | F4 | Yes (`cordis-latest` in `nightly.yml`, report only, #190 → #429) | — |
| **FZ** | P2, E7, B5 | Yes: session frames, local line framing, error graphs, WebSocket upgrade requests (`fuzz/`, nightly, #179 → #425, #624) | Configuration files (B5, low priority); dylib object-file parsing (#631) |
| **SH** | K2, RS1 | Yes: 28 models of the core, 31 in `rutis-bridge`, 3 in `rutis-host` and 3 in `rutis-loader` (§7.3), all 65 wired into `ci.yml` and `stress.yml` (#1372); Python's steppable event loop; Node's frame pump; Kotlin's Fray cases (including `RutisAppFrayTest`, on-demand toolbox); Swift's TSan | A model for dylib (#622); explicit gates in Go (#630); Kotlin's `frayTest` runs on demand outside CI |
| **TLA** | EV1–EV5, RL1, CL1, AH1, AH2, P10, C6, E4, E5 | 7 directories, 17 modules, 224 configurations (§7.2); CI's `specs` job | Per row in §7.2 |
| **TV** | EV2, EV3, EV5, AH2 | Trace validation of the session channel (`trace_validation.rs`, #624); the wait graph (`waits/trace.rs`, `waits/trace/check.rs`, #613) | The event host's action log (#624); the access point went with 0.19 (#695), so its TV is withdrawn, not pending |
| **KANI** | Value ranges and the boundaries of pure functions | 11 harnesses in `fuzz/kani/`: the date and time predicates and round trips, and the boot blob's const ABI; all `VERIFICATION:- SUCCESSFUL` on 2026-11-18 (`fuzz/KANI-MIRI.md`) | The constructors themselves through the `format!` in their error path, `parse_boot` (`str::from_utf8`), the `&str` wrappers of `boot_meta`/`is_sha256`; the rest of the `unsafe` code (#631) |
| **SK: host-level soak** | L1, L3, D3, A5, H4 | Yes: two at bridge level; the host-process S8 (#192), seeded, nightly on three platforms | Longer weekly runs |
| **G: output snapshots** | A6, A7, B6, B9, E13, F7, H3 | Common `rutis-host` errors, `check`, link state (#217, #336) | Status output as a whole; SDK errors; H3 waits for design |
| **J3: persisted format samples** | J3 | Yes (#284) | Add a sample with each release (`docs/release.md`) |
| **Completing ST** | All | fmt and clippy (`lint`), MSRV, feature combinations, weekly vulnerability checks (#256) | actionlint, license checks |
| **More unit and contract tests** | K3–K8, P3, P6, P8, E3, E6, E8, E9, F3 and others | 0.10 completed K3, K6, K8, P3, P6, E6, E8, E9, F3 (#280, #286, #285, #283, #288, #297) | D22 for K4, both flavors for K7, the actual retry timing for E3 |

---

## 7. Formal-verification coverage map

Covers standard Q9.1.2 (a multi-party protocol is model-checked before its design is final), Q11.2 (when model checking applies), Q11.2.3 (models change with the design) and Q9.1.1 item 5. Scope: every crate except the examples `rutis-agent` and `rutis-cli` (including `rutis-dsh`), and the seven language runtimes.

"Current" column: **Yes** = the method exists and its conclusions match the current code; **Partly** = it exists but lags the code or misses cases; **No**.

### 7.1 What each of the five methods covers

| Method | Used for | Proves | Does not prove |
| --- | --- | --- | --- |
| **TLA**: TLA+ design models (TLC exhaustive search) | Protocols between processes / hosts, where messages may be delayed, reordered or lost (standard Q11.2.1) | Within a bounded scale, the protocol rules themselves do not deadlock or violate an invariant | Whether the implementation follows the rules; cases beyond the scale |
| **SH**: controlled scheduling of real code | Races between tasks / threads in one process (§7.3). Rust uses shuttle and Kotlin Fray; Python a steppable `SelectorEventLoop`; Node an injectable frame pump. Go and Swift have none | Under the explored schedules, real code does not lose a wakeup, run a step twice or deadlock | External events not in the exploration (real processes, the OS, the runtime itself) |
| **PT / KANI / FZ**: property tests, bounded model checking, fuzzing (§7.4) | Logic with clear properties: state machines, matching rules, codecs, backoff, config evaluation. KANI only for tiny pure or `unsafe` functions; FZ for parsing external input | Under random (or exhaustively bounded) input, results match the reference model and nothing panics | The reference model making the same mistake; concurrency |
| **TV + K**: trace validation and conformance suites (§7.5) | Between implementation and model: a real run's log is checked as one behavior of the model; the same cases run on every implementation | The implementation does follow the model's rules (only for the runs made) | Interleavings not run |
| **Tests**: end to end, fault injection, virtual clocks, matrices, soaks | OS and platform behavior, timing and deadlines, resource limits, packaging and releases, CLI output (§7.7) | — | — |

### 7.2 Cross-process protocols: TLA+

`specs/` has 7 model directories, 17 `.tla` modules and 224 configurations (plus the non-model `specs/config-schema/`). Each directory's `run.sh` skips the 95 `*-mut-*` mutation configs by default (running 129 positive configs; pass `--mutations` or explicit names when editing a model). CI's `specs` job ([CI](ci.en.md) §4) triggers only when `specs/**` changes or when invoked by hand (#1372).

| # | Protocol | Directory | Modules | Configurations | Proved within the explored state space | Current and gaps |
| --- | --- | --- | --- | ---: | --- | --- |
| T1 | Cross-runtime synchronous calls and the wait graph | [cross-runtime-sync](../specs/cross-runtime-sync/README.en.md) | `CrossRuntimeSync.tla` | 44 | Policy B forms no cycle, a reverse call returns to its issuer, `SyncWaitCycle` is returned only for a real cycle | **Partly**: the differential property test for `cycle_among` and the wait-graph trace validation are done (#613); the Node per-key hold, cross-host cycles (registered as explicitly not guaranteed) and runtime thread pools are not modelled |
| T2 | Remote leases, link reconnects, access points | [remote-lease](../specs/remote-lease/README.en.md) | `RemoteLease.tla`, `AccessPoint.tla` | 32 | At most one active lease per runtime, at most one instance per access-point id, an old session's commands do not reach a new lease, no redial after a stop | **Partly**: the Kotlin heartbeat start point is fixed (#1181), and the heartbeat of Go, Swift, Node and Bun now starts after the hello too (#1183), with `lease-mut-heartbeat-from-accept` kept as the model's counterexample; a `.accepted` connection arriving after a Swift stop has not been checked line by line; the Cordis node's `link.mjs` is not in this repository; time is not modelled |
| T3 | Unified events: registration and dispatch, emit order, node offers, close admission | [events](../specs/events/README.en.md), [event-close](../specs/event-close/README.en.md) | `EventsHost`, `EventsTable`, `EventsEmit`, `EventsLink`, `EventsHostCheck`, `CloseAdmission` | 37 + 7 | Registered before emitted (both paths), no call starts after a withdrawal, one pair of entries keeps one order everywhere, both emit order properties, only the highest offer version is kept, the close marker and exactly-once cleanup | **Partly**: `RegisteredThenSeen` and `OneCallPerEntry` each have a mutation counterexample (`table-mut-on-late`, `table-mut-dup-local`); `emit-python-deferred` passes and `emit-mut-future-reply` violates `EmitOrder`; scopes, values, errors, cancellation, the full semantics of the five dispatch modes, multiple hops and multiple peers are not modelled |
| T4 | Row lifecycle between host and runtime | [row-lifecycle](../specs/row-lifecycle/README.en.md) | `RowLifecycle.tla` | 36 | One row runs at most one generation, a late ending ends only its own generation, a lease never outlives its row, a service is withdrawn before its provider unloads | **Partly**: `rows.schema` / `mount`, call chains, differences between unload stacks and the shutdown deadline are not modelled; #1100 is decided, #1101 is fixed (#1185) |
| T5 | Call lifecycle (two ends plus a one-hop relay) | [call-lifecycle](../specs/call-lifecycle/README.en.md) | `CallLifecycle.tla`, `CallRelay.tla` | 35 | Every call settles at most once, a late reply is dropped, reference counts are conserved, a frame after close changes nothing, a relay cancels hop by hop | **Partly**: both reply rules are verified and the reply rule is written into the specification (`SESS-61`); cleaning the cancel table and a cancel acknowledgement are undecided (#1099); the relay's close and a peer still reading after close are not modelled |
| T6 | Peer-hosted rows | [peer-rows](../specs/peer-rows/README.en.md) | `PeerRows`, `PeerOffers`, `PeerHosted`, `PeerPair`, `PeerServices` | 29 | One instance never runs in two sessions at once, a late ending of an old generation does not end the new one, gating and offline waiting, two nodes hosting each other do not wait on each other, service versions and tombstones | **Partly**: the current code no longer has `peer:` rows (removed in 0.19), so #438 cannot be reproduced against it; #1100 is decided (a key per load, `peer-fresh-keys`); #1101 (unload before load) is fixed (#1185), its issue closed |

Not every cross-process part needs TLA: the handshake and capability negotiation are two messages between two parties (PT over the decision table); exporting / importing node services is "highest version wins" (PT); a listening link is an access point without a self-reported id; cross-session reference counting is a two-party protocol per session, and the relay only holds references in-process (SH and PT).

### 7.3 In-process concurrency: controlled scheduling (SH)

| Area | Parts | Current |
| --- | --- | --- |
| Core | The 28 models in [`crates/rutis/src/fiber/shuttle_tests.rs`](../crates/rutis/src/fiber/shuttle_tests.rs): every join of a TransitionTask, `post_join` across a driver exit, exactly-once concurrent dispose, the lineage marker published before closing, effect registration and cleanup, unloading while listener calls are in flight, concurrent subtree shutdowns, root dispose and shutdown sharing one close, dependency withdrawal, emit-chain order, the listener-table watcher, an unload cancelling the running generation, effect cleanup errors reported once, several completions, a restart publishing the live generation, closing a real fiber tree, the child a closing root mounted, a dispose result surviving the driver's exit, a restart inside the dependency window, spawned tasks interleaving on shuttle's executor, a mount racing the root's close, a deadline over two fibers' calls, a restart losing to a dispose, `Provided` before `Removed`, three fibers restarting at once so `FiberStatusChanged` is delivered in order (`core/D24a`) and outside the transition lock (`core/D5a`), a mount plugin's cleanup effect and its service through the real reverse pass (`MNT-6-4`), a method-registered `Ctx`'s row unloaded while another thread registers (`MNT-11-9b`) | **Yes**: `stress.yml` runs each model a million iterations nightly and prints a replayable schedule on failure; compiled only with `RUSTFLAGS="--cfg rutis_shuttle"`, never by a plain build. #1194 (when a clean child dispose races the root's close, the same transition's join gets `Err(Closed)`) is fixed by #1196 and pinned by model 18 |
| Session and event host (`rutis-bridge`, 31 models) | 8 models in [`pipe/spawn/shuttle_tests.rs`](../crates/rutis-bridge/src/pipe/spawn/shuttle_tests.rs) (the subprocess end state, #623), 6 in [`session/rpc/shuttle_tests.rs`](../crates/rutis-bridge/src/session/rpc/shuttle_tests.rs) (the session's call and reference tables, a returned call claimed once, event operations taking their turn, a future's last waiter, #628), 1 in [`session/rpc/waits/shuttle_tests.rs`](../crates/rutis-bridge/src/session/rpc/waits/shuttle_tests.rs) (the wait graph's fast path), 3 in [`memory/shuttle_tests.rs`](../crates/rutis-bridge/src/memory/shuttle_tests.rs) (a memory channel's close, send, receive and drop in both directions), 3 in [`events/hub/shuttle_tests.rs`](../crates/rutis-bridge/src/events/hub/shuttle_tests.rs) (the event host: a row's listeners registering and leaving, the emit queue), 2 in [`websocket/pipe/shuttle_tests.rs`](../crates/rutis-bridge/src/websocket/pipe/shuttle_tests.rs) (a WebSocket pipe's blocking side), 2 in [`runtime/projection/shuttle_tests.rs`](../crates/rutis-bridge/src/runtime/projection/shuttle_tests.rs) (a service projection), 1 in [`pipe/lines/shuttle_tests.rs`](../crates/rutis-bridge/src/pipe/lines/shuttle_tests.rs) (the line framing's writer) and 4 in [`marked/shuttle_tests.rs`](../crates/rutis-bridge/src/marked/shuttle_tests.rs) (a marked service: the native service serves before the name, withdrawal goes in reverse and leaves nothing; a refused provide leaves nothing; two provides of one name leave one service; an instance claims its name once and releases it once; #622) | **Yes** (#622, #623, #628): `ci.yml` runs 1000 iterations of each scheduler per model on pull requests, `stress.yml` a million nightly |
| Runtime supervision and host reloads (`rutis-host`) | 2 models in [`crates/rutis-host/src/restart/shuttle_tests.rs`](../crates/rutis-host/src/restart/shuttle_tests.rs) (the backoff's cap, an end the next start took over) and 1 in [`crates/rutis-host/src/host_shuttle_tests.rs`](../crates/rutis-host/src/host_shuttle_tests.rs) (a reload removing a runtime waiting for its own process) | **Yes** (#627, #1372): `ci.yml` runs 1000 iterations per model on pull requests, `stress.yml` runs them at scale nightly |
| Binary runtimes (`rutis-loader`) | 3 models in [`crates/rutis-loader/src/runtime/binary/shuttle_tests.rs`](../crates/rutis-loader/src/runtime/binary/shuttle_tests.rs): a restart installing the new runtime only after the old process ended, a replaced binary installed only after the old runtime stopped, a row wanted during the restart served by the new runtime | **Yes** (#627, #1372): `ci.yml` runs 1000 iterations per model on pull requests, `stress.yml` runs them at scale nightly |
| dylib | Loading and symbol lookup in `rutis-dylib` | **No** (#622's remaining item) |
| Python | `SteppedLoop` and `ScriptedChannel` in [`python/rutis/tests/scheduling.py`](../python/rutis/tests/scheduling.py): ready callbacks run only when the test steps the loop, and the clock moves only when the test advances it | **Partly**: used by the scheduling and close tests; it controls an extracted event loop, not the whole runtime |
| Node / Bun | The injectable frame pump (`node/rutis-runtime/test/frame-pump.test.mjs` and others) | **Partly**: frames are pushed in the test's order; `fc.scheduler` is not used to generate schedules |
| Kotlin | Fray, Lincheck | **Yes** (#630): Fray 0.9.2's `frayTest` task runs 9 cases in 5 classes (Counter, Queue, RuntimeState, MemoryChannel, Peer); `jvmTest` excludes them by the `FrayTest` tag; Lincheck is not used. Missing: no workflow runs `frayTest` |
| Swift | No controlled scheduling; data races are meant to be excluded by Swift 6 strict concurrency, ordering races by TSan under stress | **Yes** (#630, #1273): `Package.swift` is `swift-tools-version:6.0` with `swiftLanguageModes: [.v6]`, the four SDK targets compile in the Swift 6 language mode, and a strict-concurrency diagnostic is an error that fails `swift build`; the shared state the compiler cannot see is `@unchecked Sendable`, each with its invariant written in place (`tools/swift-strict-concurrency.sh` lists what is left, target by target); CI's `runtimes-swift` job runs the Peer, Writer, EventsConformance, App and Concurrency suites under `swift run --sanitize=thread` (`TSAN_OPTIONS=halt_on_error=1`) |
| Go | No controlled scheduling; `testing/synctest` is not used, so a specific interleaving would need explicit gates in test builds | **No** (#630) |

### 7.4 Pure logic and parsing: PT / KANI / FZ

| Area | Parts | Method | Current |
| --- | --- | --- | --- |
| Core | Registration, withdrawal, `prepend`, prefixes, `once` and four dispatch modes of the listener table | PT | **Yes**: [`crates/rutis/tests/events/listener_table_model.rs`](../crates/rutis/tests/events/listener_table_model.rs) compares the table with an ordered-list model after every operation (256 cases, `PROPTEST_CASES`); a failure prints the seed and the shrunk operations |
| Session | Value round trips (seven languages), grants and rollback, frame and value codecs | PT | **Yes**: `value_roundtrip.rs`, `grant_rollback.rs`, `session/protocol.rs` (the last also runs under Miri) |
| Session | `cycle_among`'s cycle detection, compared with a naive implementation that builds every edge as the TLA definition does | PT | **Yes** (#613): `cycle_among_agrees_with_the_models_graph` in [`crates/rutis-bridge/src/session/rpc/waits.rs`](../crates/rutis-bridge/src/session/rpc/waits.rs) (random graphs of up to 20 hops with random profiles and entries, 256 cases by default, `PROPTEST_CASES`) and `the_model_check_agrees_on_the_small_graphs` |
| Loader, host, dsh | Edit pipeline, pending-save queue and multi-writer CAS, configuration evaluation, profile layer composition | PT | **No** (#629, merged into #631) |
| Value functions | Date and time constructors and round trips | KANI | **Yes** (2026-11-18): 11 harnesses in `fuzz/kani/`; the date and time predicates, the round trips and the boot blob's const ABI all report `VERIFICATION:- SUCCESSFUL`. The constructors themselves through the `format!` in their error path, and `parse_boot`, still have no verdict: `fuzz/KANI-MIRI.md` |
| Transport | Session frames, local line framing, error graphs, WebSocket upgrade requests | FZ | **Yes**: the five targets `session_frame`, `session_frames`, `local_lines`, `websocket_upgrade` and `error_graph` in `fuzz/fuzz_targets/`, each run for `fuzz_secs` nightly by `stress.yml`; three of them also run under Miri on fixed inputs (`fuzz/miri/`). The hand-written RFC 6455 frame parser in Go has `FuzzFrameParsing` (Go's native fuzzing; the seeds run with `go test`) |
| dylib | Object-file parsing (ELF / Mach-O / PE) and the manifest format | FZ | **Yes**: `dylib_meta` and `binary_manifest` (#631) |

### 7.5 Does the implementation follow the models: TV and conformance suites

| Object | How | Current |
| --- | --- | --- |
| Session channel | `crates/rutis-bridge/tests/events/trace_validation.rs`: the channel, frames and lease steps are written as a trace, and every event is checked against what the earlier events allow ([session tracing contract](formal-trace.en.md) §2–§4) | **Yes** (first slice of #624): channel direction and end, hello once and first, call ids and `path`, at most one reply, the one allowed dangling cancel, lease takeover steps |
| Access points (withdrawn) | The object was deleted in 0.19 (#695): the `AccessEvent` log, the access point and the identity-layer revocation hook went with it; `AccessPoint.tla` is now a minimal model built from `rutis-host/listen`, and this side sees only how an admission ends | **Not applicable** (the access-point TV of #624 is withdrawn): the §3.8 cases in `trace_validation.rs` check the 4003 revocation and 403 credential refusal as seen here; there is no access-point action log to compare |
| Wait graph | Record the six operations of `waits.rs` and every decision, and compare with `CrossRuntimeSync.tla` | **Yes** (#613): `waits/trace.rs` records, `waits/trace/check.rs` replays as `RustSucc`; every event is checked on the real runs of `sync_cycles.rs` and `rpc_callbacks.rs`, and corrupting the same log must be reported |
| Event host | Compare the action log of a host running `events-all` with the host model | **No** (#616, merged into #624) |
| Language runtimes | Conformance suites: `runtime_conformance`, `events_conformance`, `sync_cycles`, `host_gone` | **Partly**: `sync_cycles` has Node / Python / Go / Swift columns but no Kotlin; `host_gone` has Node / Python / Go / Bun but no Swift / Kotlin (not done in #624: Swift plugins build on macOS only, Kotlin needs JDK 17 and `:rutis-conformance:fatJar`); the lease suite `crates/rutis-host/tests/dial/lease_e2e.rs` has Python, Go and Bun columns, missing Swift and Kotlin for the same reasons; the reference-counting PT now covers all seven languages (`value_roundtrip.rs`'s `*_releases_balance` / `*_reference_counts`, Go, Bun and Kotlin checking their own side, #624); the grant-and-rollback PT is `grant_rollback.rs` (Rust side) |
| Test tools and real runtimes | The cases of `weather.expected.json` and `events.expected.json` without `tool: false` | **Partly**: the Node and Python test tools and real runtimes read the same expectations file; Go `rutistest` already matches the whole weather file; tools that share code with a runtime (JS `events.mjs`, Python `Bus`, Go `internal/events`) can make the same mistake as it |
| Model correspondence tables | The "corresponding design and code" table in each model README | **Yes**: all seven model READMEs use repository-wide paths (`model-pr.yml` / `tools/check-model-pr.mjs` were retired in #1372) |

### 7.6 Rule tracker (retired, #1372)

The 1,825-row static rule tracker (`docs/formal-trace/`), `tools/formal-trace*.mjs`, and `model-pr.yml` were retired in #1372. `docs/guarantees.en.md` is trimmed to the 40 active core promises (#1376), all 65 shuttle models across the four crates (`rutis`, `rutis-bridge`, `rutis-loader`, `rutis-host`) now run in `ci.yml` and `stress.yml`, and `specs/*/run.sh` skips the 95 `*-mut-*` mutation configs by default while `nightly.yml` triggers `specs` only on `specs/**` changes. The 11 passing 0.23 Kani harnesses and 6 Kotlin Fray suites are kept as on-demand toolboxes.

### 7.7 What no formal method reaches

- Operating systems and platforms: macOS fork / CLOEXEC, Windows TLS and paths, CPython and V8 behavior differences → support matrix, end to end.
- Timing and deadlines: the greeting deadline, the 30 / 60 second backoff rules, waiting after a refusal → virtual-clock tests ([test-clock design](design-test-clocks-2026-10-11.en.md), `virtual_clock_contract.rs`; backoff doubling, the 30-second retry after a refusal and the 60-second reset are `start_paused` cases in `crates/rutis-host/src/connect/dial.rs`, and the 10-second greeting deadline is in `crates/rutis-bridge/tests/session/session_plugin.rs`; #633 was merged into #639 and closed with it).
- Resource limits and slow growth → residue checks (§5) and soaks (S8).
- Flaky failures caused by test design, packaging and CI, logic errors in the CLI and code generation.
- `unsafe` problems visible by reading the code: `crates/rutis-dylib/src/unix.rs:47` calls `CStr::from_ptr(libc::dlerror())` without a null check (`find_symbol` in the same file does check).

A classification made on 2026-10-24 over the 150 fixes from 2026-10-02 to 10-24 found 109 product defects, 45 of which no method would have caught ([#612](https://github.com/arcships/rutis/issues/612)). It has not been redone for later versions.

### 7.8 Practice

- A model lives in `specs/<name>/`: `.tla`, `.cfg`, `expected.txt`, `run.sh`, and a README in both languages (which design document and section, what was simplified, which invariants are checked, mutations and counterexamples). All 7 model directories follow this.
- When a design change touches a modelled protocol, the same pull request changes the corresponding `specs/` directory (the former `model-pr.yml` / `tools/check-model-pr.mjs` PR-body gate was retired in #1372).
- Trace validation starts with the session channel (§7.5); the rules come from the [session tracing contract](formal-trace.en.md), not a second copy.
- Timing: Spec 1 (cross-runtime synchronous calls) was finished before Bun, Spec 2 (remote leases) in 0.12; the models for events, row lifecycle, call lifecycle and peer-hosted rows were finished before 0.22 (#613).

### 7.9 The first two models' scope records (2026-10)

Kept as a record; the model itself, its simplifications and invariants are in each directory's README.

#### Spec 1: cross-runtime synchronous calls and the deadlock policy (P10, C6)

**Result** (#208 → #222, 2026-10-11): the model is in [`specs/cross-runtime-sync/`](../specs/cross-runtime-sync/README.en.md). Policy B was chosen: the host tracks waits across sessions, and when forwarding a call would close a wait cycle it does not forward it and returns `SyncWaitCycle` instead. The conclusion is written back into multilanguage design §9. The implementation is #228 → #261: the `sync` field on frames and the `sync-wait` and `sync-stack` capabilities; tests in `rpc_callbacks.rs` and `rutis-loader/tests/lang/sync_cycles.rs`. Two points decided during implementation: a runtime that does not declare itself reentrant is treated as non-reentrant; a runtime that does not declare `sync-wait` is outside detection (no false errors).

**Why first**: multilanguage design §9 states this is unresolved. The Node runtime is not reentrant, the Python one is, and two Node runtimes making crossing synchronous calls deadlock. A policy must be chosen before a third language (Swift, Go, or Bun #194). Deadlocks appear only in particular interleavings, which tests hit unreliably.

**Model**

- Participants: the Rust host (relay), 2–3 runtimes, each reentrant or not.
- Actions: a runtime makes a synchronous call (possibly to a service of another runtime); the host forwards it and rewrites the call chain `path`; while waiting synchronously, the callee receives incoming calls and handles them by its own rule (non-reentrant: only calls on its own chain, others deferred; reentrant: all); return; return `SyncWaitCycle`.
- Invariants: no deadlock (while calls are unfinished, some participant can proceed); a callback always returns to the runtime that made it; `SyncWaitCycle` is returned only when the wait really forms a cycle.

**The two candidate policies compared** (from multilanguage design §9):

| Policy | Content | Question to answer |
| --- | --- | --- |
| A | All leaf runtimes reentrant, only the Cordis runtime not; Rust refuses synchronous calls between two non-reentrant runtimes | Can it be decided when the call is made; does it refuse legitimate calls |
| B | Rust tracks waits across sessions and returns `SyncWaitCycle` on a cycle | Does the host see enough to detect cycles; does it miss or misreport |

**Scale**: 3 runtimes, call chains of depth ≤ 3, ≤ 2 concurrent calls.

**Output**: the chosen policy written back into multilanguage design §9; counterexamples turned into tests in `rpc_callbacks.rs`.

#### Spec 2: remote lease takeover, link reconnection and registration revocation (E4, E5)

**Result** (#210 → #432, 2026-10-17): the models are in [`specs/remote-lease/`](../specs/remote-lease/README.en.md), two of them: `RemoteLease.tla` (a listening remote runtime and two dialing controller links) and `AccessPoint.tla` (the access point that landed after the issue: registration generations and hand-over, self-declared ids, takeover by the same id, the access point stopping and starting again). Invariants checked: at most one active lease per runtime, at most one instance per id at an access point; commands (calls) of an old session never act on a new lease (instance); after a registration is revoked an old handshake never completes hand-over; a stopped link (dialer) never reconnects; a late handshake result never revives a session given up on. Liveness: without faults a controller eventually holds the lease, and someone still holds it at the end. Scale: 2 controllers (dialers), up to 3 reconnects each (2 in the access point's configurations with lost connections), unbounded message delay; the largest configuration has about 1.2 million states, all 16 configurations take under a minute. Each property has a deliberately introduced bug (a mutation) for which TLC finds a counterexample: the model is not vacuous.

TLC found two places where revoking an identity during a handshake still set a session up, fixed (#432); identity revocation (#273) was removed in 0.19, and its properties, configurations and tests with it. The §4.4 takeover order, the generation re-check, reading frames by connection and cancelling the retry on stop agree with the model. Two controllers configured for the same runtime (or two dialers with the same id) keep replacing each other, slowed by backoff (written in dial-out §6); liveness holds in the model because reconnects are bounded. CI: the `specs` job ([CI](ci.en.md) §4).

**Why**: remote design §4.4 defines the takeover order (authenticate → close the old session → clean up the old lease → answer the new hello); the network stack design defines registration generations, revocation during handshakes, and that late results are not handed over. The existing `leases.rs` and `link.rs` cover only the timings they can construct (`leases.rs` was deleted in #695 and replaced by `crates/rutis-host/tests/dial/lease_e2e.rs` in #1113).

**Model**

- Participants: 2 controller links, 1 remote runtime (listening), Identity (revocable).
- Actions: dial, authenticate, hello, disconnect, reconnect with backoff, takeover by a new connection, lease cleanup, identity revocation, late handshake results, late instructions from an old session.
- Invariants: at any time a runtime instance has at most one active lease; instructions of an old session never act on a new lease; after a registration is revoked, an old handshake never completes handover; a stopped link does not reconnect.
- Liveness: with both controllers healthy, one eventually holds the lease.

**Scale**: 2 controllers, up to 3 reconnects each, messages delayed by up to 2 steps.

Revoking while running was the identity layer's hook (#273), removed in 0.19; "identity revocation" was removed from the model with it.

---

## 8. Continuous integration

Standard §12. How CI runs today and what to do when changing it is in [ci.md](ci.en.md). This section records measurements and the gaps against the standard.

### 8.1 Measurements before the changes (history: 2026-10-10, last 30 runs of `ci.yml`)

The data below is from before the changes, kept for comparison; it does not describe the current state.

Push to result: 22–25 minutes without queueing, 35–46 minutes when busy, once more than 2 hours.

Job durations in one run without queueing (#202, run `38024950688`):

| Job | Duration | Breakdown |
| --- | ---: | --- |
| dylib-macos | 25.1 min | reproducible build 9.8, external plugin build 7.6, launcher checks 5.3 |
| dylib-linux | 20.9 min | reproducible build 9.1, external plugin build 6.1, launcher checks 4.4 |
| test (full Linux) | 7.7 min | `cargo test --workspace` 3.3 |
| sdk-repro ×2, sdk-repro-macos ×2 | 3.6–4.9 min each | — |
| runtimes-windows | 4.2 min | — |
| network-macos | 2.2 min | — |
| the other 8 | < 3 min each | — |

With repeated pushes to one branch (`feat/bun-runtime` pushed 6 times between 02:10 and 02:52, 6 runs at once), macOS queue times (run `38016607150`): sdk-repro-macos 32 minutes, dylib-macos 23.5, static-platforms 14.7, network-macos 10.7.

The problems then: no `concurrency`; no selection by change, so a docs-only PR ran all 17 jobs; the dylib jobs on the critical path; 6–8 macOS jobs per run; build reproducibility checked in three places; packaging jobs on every PR; no aggregate check; no measurements.

### 8.2 What was done

| Change | Clauses | PR |
| --- | --- | --- |
| A new push to a PR cancels the old run; on main runs are grouped by ref and a running one is not cancelled | Q12.4 | #224 |
| The `changes` job turns switches on by change, PRs run only the related jobs, main runs everything; the aggregate check `ci-ok` is the only required check; dylib on Linux split into launcher, sdk-bundle and repro jobs; `sdk-repro*` and packaging moved to main; weekly CI measurements (`ci-stats.yml`, `tools/ci-stats.mjs`) | Q12.1, Q12.3, Q12.5, Q12.6.1, Q12.7, Q12.9 | #213 |
| CI design and process document `docs/ci.md` | Q12.8, Q12.10 | #237 |
| `lint` (fmt, clippy `-D warnings`); MSRV 1.88; Linux on the minimum versions (Node 22, Python 3.12, websockets 15), macOS and Windows on the latest; weekly dependency vulnerability checks in `deps.yml` | Q5.3.7, Q10.2, ST | #256 |
| Installation smoke tests in `install.yml`; installation on five platforms before a release, verification with registry packages after it | Q6.15, Q13 | #254 |
| The `e2e` job (in parallel with `rust`); duplicate release builds removed | Q12.7 | #265 → #275 |
| Platform-specific tests on macOS and Windows: first `value_roundtrip` and `fault_links` added, then every test file by default, excluding only the platform-independent ones (`PURE_TESTS_*`) | Q10.2, Q7.8 | #313, #328 |
| A `multiprocess` job in nightly `stress.yml`: the bridge's and loader's multi-process tests repeated over many rounds | Q7.3 | #245 |
| Self-hosted runners: macOS jobs such as `dylib-macos` and `sdk-repro-macos`, and jobs that do not depend on the OS (`links`, `js-py`, `msrv`, `checks`, `release-dry-run`, `semver-rutis`, `changes`, `ci-ok`) run on self-hosted macOS by default (repository variables `MACOS_RUNNER` and `GATE_RUNNER` switch them back); the Linux jobs and `runtimes-windows` can be switched to self-hosted runners with `LINUX_RUNNER` and `WINDOWS_RUNNER` (GitHub's runners by default) | Q12.6.2 | #333, #348, #350 |
| Lighter ordinary-PR critical path: OS-independent jobs moved back to Linux and no longer take macOS slots; `rust` and `rust-runtimes` run in parallel; Gradle caches added; PRs no longer rebuild the Kotlin fatJar or repeat the Node/Python unit tests `js-py` already ran; nightly and stress jobs tiered by frequency | Q12.2, Q12.6, Q12.7 | #1375 |

`network-macos` runs on the self-hosted macOS machine too (repository variable `NETWORK_MACOS_RUNNER`; moved once #344 fixed #287, ci.md §10).

### 8.3 Measurements (2026-10-12, the last 40 completed runs of `ci.yml` on 2026-10-12)

From `node tools/ci-stats.mjs 40 ci.yml --since 2026-10-11`. Jobs were being moved to self-hosted runners that day (#333, #348, #350), so old and new runners are mixed in the data; it is a record of the move only.

| Runs | Count | Push to result, median | p90 | Max |
| --- | ---: | ---: | ---: | ---: |
| main (push) | 13 | 26.3 min | 48.6 min | 48.6 min |
| PR | 27 | 14.3 min | 38.2 min | 42.9 min |
| PR, ordinary code change | 9 | 15.6 min | 42.9 min | 42.9 min |

- The target for ordinary code PRs is 10 minutes (ci.md §1); the median is 15.6 minutes, so it is not met. Queueing is the main cause: several jobs waited 20–40 minutes at most.
- The last two runs after the move: #350's PR run took 10.1 minutes (every job, since it changed CI), the main run after merging 7.2 minutes.
- Longest median durations: `runtimes-windows` 8.2 minutes, `dylib-linux-repro` 7.5, `rust` 6.2, `network-macos` 4.5.
- main failed 3 times (after merging #339, #349, #348); 6 runs were rerun (4 passed on rerun; the intermittent failures behind them are recorded and fixed, §9.2).

### 8.4 Gaps against the standard

| Clause | Gap |
| --- | --- |
| Q12.4 | ~~On main, runs are grouped by ref, so not every merge gets its own complete result~~: the standard now has the nightly run check every commit since the last one (#369); met |
| Q12.6 | The old median for ordinary code PRs was 15.6 minutes (§8.3); the first ordinary code PR after #1375 measured 10.0 minutes (§8.5), meeting the target; there is one sample so far, keep tracking with `ci-stats` |
| Q12.7 | In full runs the single-machine and two-machine reproducibility checks both run |
| Q12.1 | The old record said nightly lacked TLC, but current CI runs the `specs` job on PRs that change `specs/**`; nightly runs it only when the corresponding paths changed (or a workflow/configuration path that triggers the full plan changed), while Monday's weekly run unconditionally runs all existing models. The full model matrix, trace validation and cross-language conformance still need one daily/release report. Weekly still lacks longer host soaks and benchmarks (the hour-long host soak S8 now runs nightly, #192). A real network (S5 with containers + netem) now runs nightly (report only, #341 → #433); the newest Cordis-plugin baseline also runs nightly (report only, #190 → #429); fuzzing and the existing models run in stress/nightly (#179 → #425) |

### 8.5 Measurement (2026-11-20, the first ordinary code PR after #1375)

#1384 is the first ordinary code PR after #1375 merged (it changes only `node/**`). Run `44772162074`: **10.0 minutes** from push to result (16:19:07 → 16:29:07), meeting the 10-minute target, with all 39 checks passing.

| Job | Duration |
| --- | ---: |
| `rust` | 9.6 min |
| `rust-runtimes` | 8.4 min |
| `network-macos` | 5.9 min |
| `runtimes-windows` | 5.6 min |
| `e2e`, `js-py`, `lint` | 2.1–2.2 min |
| the rest | < 1 min |

- Compared with §8.3, an ordinary PR no longer waits 15–45 minutes behind OS-independent jobs holding macOS slots.
- The critical path is now `rust`; #1373 will compress compilation and linking further by merging integration-test binaries.
- There is one sample only: the target is reachable, and the median still needs to accumulate through `node tools/ci-stats.mjs`.

## 9. Plan

Ordered by risk level, P0 first; within a level, infrastructure other work depends on first. Aligned with the current release plan. Progress follows #183 and the [formal-verification redesign](design-formal-verification-2026-11-08.en.md); the old 0.11–0.14 schedule is no longer the current plan.

As of 2026-11-08, formal verification is organised into four layers with these dependencies:

| Layer | Main issues | Current gate and dependency |
| --- | --- | --- |
| A. Protocol foundations | #635, #634 | Unify session frame/handshake/error/reference/cancellation/limit rules first, then clean up old sources and build the rule tracker |
| B. Concurrency and lifecycle | #622, #613, #630 | Depends on A; #1025 first models and deterministically tests marker commit/admission |
| C. Cross-process deployment | #613, #624 | Depends on A and B; TLA+ proves the design, trace validation checks real executions, and conformance cases cover each language |
| D. Inputs and long-term reliability | #631, #639 | Depends on the rules from A, B and C; property/fuzz testing, Kani/Miri, virtual clocks, ordinary tests and E2E cover model boundaries |

#634 is shared tracking and close-out work, not another feature. 0.23 is merged into 0.22: 0.22 completes both #1025's high-risk close-semantics verification and the full four-layer coverage. The execution plan is the [formal-coverage execution plan](plan/formal-verification-0.22.en.md). Do not bring #1040 back into 0.22.

The following §§9.1–9.3 are retained only as historical quality-plan records, not as the current release schedule. “Done” there describes the acceptance conclusion at that time; it does not replace the new four-layer evidence of rules, models, traces and cross-implementation conformance.

### 9.1 Historical record: step one (0.8.1 → 0.9) — infrastructure and P0s that already happened

| Item | Risks covered | issue → PR |
| --- | --- | --- |
| Four known bugs fixed with regression tests | P2, B2, C11, P11 | #173 → #214, #174 → #216, #184 → #215, #197 → #218 |
| Determinism rules; seeds in `interleave.rs` | K10, P11 | #182 → #220 |
| CI: cancel old runs, select by change, aggregate check, merge macOS jobs, remove duplication | Feedback speed for every risk | #203 → #213, #224, #237; finishing #265 → #275 |
| Completing ST (fmt, clippy, dependency checks, MSRV); minimum version matrix | All; MX | #204 → #256 |
| E2E harness + residue checks + S2 | A3–A5, A8, B2, B3, B10 | #175 → #221, #186 → #253 |
| S9 installation smoke tests | B1; acceptance of #195, #196 | #193 → #254 |
| Unit tests for rutis-host configuration and assembly; snapshots of common errors | B5, B6, A6, A7 | #205 → #217; #258 |

Found by E2E and fixed: #225, #226 (#261, #217). Minimum Node lowered to 22 (#195 → #219), Python to 3.10 (#196 → #255).

### 9.2 Step two (0.10): P0s across processes and platforms — done (2026-10-12)

Plan: [design-quality-step2-2026-10-11](design-quality-step2-2026-10-11.en.md) (#272).

| Item | Risks covered | issue → PR | State |
| --- | --- | --- | --- |
| S3 composing languages and crashes | C3, C4, B10 | #187 → #317 | Done; recovery by restarting the host (plan Q1); no Bun row on Windows |
| S4 moving an implementation | G2, G3, G5 | #188 → #326, #330 | Done; with a fixture Rust host (plan Q1) |
| S5 two host processes over loopback | E11, E1, E6 | #189 → #322, #336 | Done; revoking while running is #273, containers + netem #341 → #433 |
| Protocol layer: fault injection and links, boundary rules 3/4/5/7, SDK alignment, negative TLS, value round-trip property tests | C4, E1, E6, E8, E9, P3, P4, P6, P7, P8, F3 | #178 → #285, #283, #288, #297, #296 | Done; resource limits wait for design |
| Windows columns of the conformance suites; missing cells | C10 | #176 → #295, #309 | Done |
| Test tools vs real runtimes | A1 | #206 → #298 | Done (Node, Python); Go and Bun test tools follow later |
| Persisted format samples and the `format` field | J3 | #207 → #284 | Done; `format` stated in the 0.10 release notes |
| Completing core guarantees | K3–K8 | #177 → #280, #286; follow-up #345 | Done; D22's cycles and both flavors for K7 not done |
| Real clocks in other crates and languages | K10 | #231 → #257, #278, #299, #301 | Done |
| E2E residue checks on Windows (Job Object); S2 on Windows, S9 stop on Windows | B3, C10 | #232 → #277, #323, #324 | Done |
| Follow-up cells of the S9 installation smoke tests | B1 | #266 → #324 | Bun row, websockets 15.0, Go row and stop on Windows done; marking the Python guide's examples waits for #259 (in progress) |
| Guarantee register | All | #181 → #276, #310 | In progress: the `loader/` family and the rest of `host/` not registered; 0.10 uses the current state as its baseline (plan Q3) |
| TLA+ Spec 1 and the implementation of policy B | P10, C6 | #208 → #222; #228 → #261 | Done |

Defects found by step two's tests and fixed: #279 (#338), #281 #282 (#337), #287 (#344), #290 (#312), #291 #292 (#345), #293 (#335), #294 (#305), #300 (#334), #302 (#311), #304 (#346), #321 (#336), #325 (#330), #327 (#331), #332 (#339). Intermittent failures: #289 (#301), #303 (#319), #306 #307 #316 (#320), #314 (#318), #342 (#347), the depth limit test (#349); also #233 (#243), #234 (#244), #246 #249 (#261), #247 (#248), #250 (#252).

Moved out or opened during step two: revoking while running (#273), changing `run`'s configuration while running (#274), Date / Map / Set / binary / big integers (#308) and emit carrying its dispatch mode (#340) went to "waiting for design"; S5 with containers + netem (#341) went to step three.

### 9.3 Step three (0.12; planned for 0.11, moved on 2026-10-17): long runs, fuzzing, concurrency models, undecided designs

| Item | Risks covered | issue → PR |
| --- | --- | --- |
| Three FZ targets; three SH models | P2, E7, K2 | #179 → #425. Done (0.12) |
| Host-level soak (S8) | L1, L3, L4 | #192 → #431. Done (0.12): nightly on three platforms; found and fixed a Python runtime that did not exit after its host was `kill -9`ed |
| S6 Cordis plugins; nightly baseline with the latest Cordis plugins | F8, F4 | #190 → #429. Done (0.12); the gap it found, #428, is fixed (0.12) |
| S7 Rust embedding and multiple tenants | D2, D3, D5 | #191 → #430. Done (0.12): found and fixed a projection task and export slots leaked per language row |
| Previous release's runtime vs current host | J2, E13, P8 | #209 → #426. Done (0.12): `tests/e2e/tests/install/cross_version.rs`, the previous release derived from the versions on npm, run in the `e2e` job of `ci.yml`'s full runs (nightly, release gate); no combination failed after loading or hung |
| TLA+ Spec 2 | E4, E5 | #210 → #432. Done (0.12): two models, CI's `specs` job; found and fixed two places where revoking during a handshake still set a session up |
| Bun runtime admitted under Q6.8 (B2, B3) | The Bun variant of C10 | #194 → #200; remote listening and dialing out #402; #236 → #261. Done |
| S5 nightly: two containers, non-loopback + TLS, netem | E12 | #341 → #433. Done (0.12): `tests/e2e/tests/soak/netem.rs`, `netem.yml`, nightly, report only; found and fixed a Python `listen:` with its controller connected not stopping on Ctrl-C and skipping its cleanups on SIGTERM; B printing websockets' traceback on an outage (#434) |
| Values consistent across languages: Python `datetime` by reference, Go structs bypassing the checks, Python dicts dropping undefined keys | P7, A1 | #355 → #410, #412, #413. Done (0.11) |
| Passing dates and times, binary data, maps, sets, big integers, decimals and UUIDs across languages | P7, A1 | #308, #360 → designs #404, #414, implementation #406, #415, #410, #412, #413. Done (0.11) |
| Revoking a peer's credentials while running, without restarting; S5 gains a step afterwards | E5 | #273 → #424. Mechanism done (0.12): the identity layer's revocation hook with library tests; rutis-host has no trigger, so neither S5 (over loopback, nor in containers #433) has the step: it is left to the management service |
| `rutis-host run` changing its configuration while running; restarting a crashed runtime | B4 | #274 → #421, #422, #423. Done (0.12): SIGUSR1 / `--watch-config` rereads the configuration, all or nothing (`host/reload-config`); automatic restart after a crash is opt-in, off by default (`host/runtime-restart`) |
| Language runtimes organised as a registry, test helpers moved to `common` | — | #230 → #420. Done (0.12) |

Found and fixed in step three: #407, #408, #409 (#419), #411 (#427), #428 (#443), #434 (#436), #435 (#437), #438 (#439), the release review's fixes (#450).

"CI changes §8.6 steps 3–4" (splitting the dylib jobs on Linux, CI measurements), formerly in step three, were done in #213.

### 9.4 Current formal-verification work

The old “waiting for design” list is no longer the current schedule. The current design and implementation boundaries are in the [formal-verification redesign](design-formal-verification-2026-11-08.en.md), closing around these eight issues:

| Layer/work | Issue | Status and next step |
| --- | --- | --- |
| Protocol baseline and rule tracker | #635, #634 | Closed (2026-11-16, COMPLETED): the first session protocol specification (`docs/protocol-session.md`) and the first row protocol specification (`docs/protocol-rows.md`) are written; the rule tracker has 1825 rows (including 1368 protocol rules, §7.6). Unknown fields are decided: reject but fail only that call (`bad-frame`); all seven runtimes implement it and run in CI |
| Event, call and lifecycle models | #613 | Closed (2026-11-16, COMPLETED): 7 model directories, 17 modules, 224 configurations: `cross-runtime-sync`, `remote-lease`, `event-close`, the four events modules, `row-lifecycle`, `call-lifecycle` and `peer-rows`; per-directory status and gaps in §7.2. The bail/async/cancel-table/peer-row-key findings are tracked as issues or fixed in code |
| Rust implementation interleavings | #622 | Closed (2026-11-16, COMPLETED): 65 shuttle models across four crates pass (§7.3: core 28, `rutis-bridge` 31, `rutis-host` 3, `rutis-loader` 3; #622, #627, #628), all now wired into `ci.yml` and `stress.yml` (#1372); the sequential reference model for `interleave.rs` is merged (#1190); #1194 is fixed by #1196. Remaining: dylib |
| Non-Rust runtime scheduling | #630 | Closed (2026-11-16, COMPLETED), first slice done: four Node frame-pump cases and Python's `SteppedLoop`; Bun is inventory-only; Kotlin's Fray cases and Swift's TSan came later (§7.3); Go still has no controlled scheduling |
| Trace and conformance | #624 | Closed (2026-11-16, COMPLETED), first slice done: Rust channel/frame validator with 23 cases (reference counts, error graphs, call scope, limits, replacement/reconnect classification); the lease suite gained Go and Bun columns (Python, Go and Bun); the access-point TV is withdrawn with 0.19 (#695) and no longer applies; the event host's action log, `sync_cycles`'s Kotlin column and `host_gone`'s Swift / Kotlin columns remain (why in §7.5); the wait graph is done (#613, §7.5) |
| Property, fuzz, Kani/Miri | #631 | Closed (2026-11-16, COMPLETED), first slice done: six grant-rollback cases, five value/frame/bad-frame properties (proptest and corpus replay) and the listener-table model property; cargo-fuzz ran four targets with no crash; Miri ran the session frame and value path (no UB); the 6 Kani harnesses have no verdict (0.68.0 does not converge) and are not reported as passing (`fuzz/KANI-MIRI.md`). The reference models for service slots and the five dispatch kinds are merged (#1200: `crates/rutis/src/registry/prop_tests.rs`, `crates/rutis/src/bus/prop_tests.rs`) |
| Conformance and ordinary tests | #639 | Closed (2026-11-15, COMPLETED): the conformance cases from the tracker landed in batches, and the virtual-clock cases are in `rutis-host/src/connect/dial.rs` and `rutis-bridge/tests/session/session_plugin.rs` (§7.7); the tracker's statuses still come from the re-check (§7.6), so rows such as `M1-13-3` remain unverified |
| Shared close-out | #634 | Closed (2026-11-16, COMPLETED): the 1,825-row static rule tracker and `tools/formal-trace*.mjs` were retired in #1372 in favour of a lean 40-row core-promise forward form check |

All eight issues (#613, #622, #624, #630, #631, #634, #635 and #639) are closed; the gaps the table names are tracked by #776 and the remaining #1270–#1274.

#1040 remains deferred; do not bring the service-announcement policy back into 0.22.

---

## 10. Assumptions to confirm and open questions

1. **Scale assumptions** (§2.2): affect the ratings of D11, L1, L2 and the length and size of soaks.
2. ~~Whether `rutis-host run` watches its configuration file~~: checked (2026-10-11): it does not. 0.10 leaves `run` as it is; S3 recovers by restarting the host, S4 uses a fixture host (step two plan Q1); whether `run` should support changing its configuration while running is #274.
3. ~~Service contract test tool (G1)~~: decided (2026-10-11). Whether implementations of one service return the same data is up to the service's authors; rutis provides no contract tool (standard Q6.9.4). rutis only guarantees that the rules for converting values across languages hold in every runtime and test tool (P7, A1).
4. **Isolation and permissions for agent-written plugins (H1, H2)**: design philosophy §8 "permissions" lists this as open.
5. ~~Adopting TLA+~~: Spec 1 is done (#222). Spec 2 is done (#210). CI's `specs` job runs TLC on pull requests (that change `specs/**`) and nightly.
6. ~~Revoking the token in S5~~: decided (step two plan Q2): revoking while running is a requirement on remote, opened as #273; S5 in 0.10 tests "after a restart with another token, the reconnect is refused".
7. ~~From which release the register's release gate applies~~: decided (plan Q3): 0.10 sets the baseline, Q9.4.4 applies from 0.11.
8. ~~Whether persisted formats get a version field~~: decided (plan Q4): `rutis.json` gets a top-level `format`; dsh's `cordis.patch.yml` is Cordis's configuration format and gets none.
9. ~~What to do with values outside the conversion table~~: decided (plan Q5): state the current behavior in the table first and register it as not guaranteed; changing the implementation is a separate issue (#308).
10. ~~How step two is implemented~~: decided (plan Q6): one PR per item; the plan PR keeps only the plan.
11. ~~The concurrency group on main and Q12.4~~: resolved (2026-10-13, [CI redesign](design-ci-redesign-2026-10-13.en.md), #369): a merge to main no longer runs checks; the nightly run checks every commit since the last successful one, and a failure opens an issue naming the commit range; the standard's Q12.3 and Q12.4 changed with it.
12. ~~Runtimes that outlive a killed host on Unix (Q5.4.4, B3)~~: fixed (#353, #354): once the session ends, a runtime unloads its rows within `RUTIS_SHUTDOWN_TIMEOUT` (passed on from rutis-host's `--shutdown-timeout`, 10 seconds by default) and exits.
13. ~~Whether to move `network-macos` to the self-hosted runner~~: moved (repository variable `NETWORK_MACOS_RUNNER`, ci.md §10).

---

## Appendix: methods evaluated and their current conclusions

Evaluated under standard Q11.3. **Adopted** methods already run; **partly adopted** rows say where they are used and where they are not; **not adopted** rows have no matching P0 / P1 risk in §4 or duplicate a chosen method.

| Method | Current conclusion | Where it is used, why not, when to reconsider |
| --- | --- | --- |
| Coverage gates | Not adopted | Coverage shows code ran, not that results were checked; a gate invites tests written for the number. The coverage in §2.3 is for magnitude only |
| Mutation testing as a gate | Not adopted | Runs take hours; a gate needs someone to keep handling surviving mutants. The TLA "one mutation per property" practice is inside the models and is not this |
| proptest state machine for the core | Adopted | The listener table already has a model-based property test (`crates/rutis/tests/events/listener_table_model.rs`); the core's sequential operation batches live in `interleave.rs` (seeded) and the reference model landed in #1190. Bug A is reachable with sequential batches, so "overlaps with the contract tests" no longer excludes this |
| Bounded model checking (Kani) | Partly adopted | 11 harnesses in `fuzz/kani/` cover the date and time predicates and round trips and the boot blob's const ABI (#1271); all reported `VERIFICATION:- SUCCESSFUL` on 2026-11-18 (`fuzz/KANI-MIRI.md`). The constructors themselves through their error messages, `parse_boot`, and the `&str` wrappers of `boot_meta`/`is_sha256` still have no verdict; the rest of the `unsafe` code and dylib remain in #631 |
| Miri | Partly adopted | Used on the session frame and value path: `fuzz/miri/` runs 30 fixed inputs (13 frames, one past the depth limit, 9 framing seeds, 7 error graphs) with no UB, and the property tests in `session/protocol.rs` also run under Miri with small case counts (one recorded run took 122 s). It does not cover tokio, the network or files, so it is not used for dylib |
| ASan | Not adopted | `unsafe` is concentrated in dylib loading and process spawning (FFI and system calls); there is no Rust-side code to run it on. Memory problems in dylib are covered by Miri's scope and by end-to-end tests |
| TSan | Adopted (Swift only) | Swift data-race stress testing (#630): CI's `runtimes-swift` job runs the Peer, Writer, EventsConformance, App and Concurrency suites under `swift run --sanitize=thread` (`TSAN_OPTIONS=halt_on_error=1`); `Package.swift` is in the Swift 6 language mode, so strict concurrency is an error (§7.3). Not used on the Rust side |
| Exhaustive loom | Not adopted | shuttle already covers the 27 core models and loom needs larger code changes; reconsider when shuttle finds a problem it cannot reproduce reliably |
| SDKs running the channel contract themselves; cross-language frame reference samples | Not adopted | Overlap with the Rust-driven conformance suites; the malformed-frame part is in P3 and the `malformed` matrix |
| `cargo public-api` | Not adopted | Duplicates `cargo semver-checks` |
| TLA+ trace validation | Adopted | The condition to reconsider (spec and implementation diverging repeatedly) has been met: the session-channel validator exists (`crates/rutis-bridge/tests/events/trace_validation.rs`, #624) and the wait graph is done too (#613); the access-point TV was withdrawn with 0.19 (#695, §7.5), and the event host's action log remains per §7.5. The rules come from the [session tracing contract](formal-trace.en.md) |
| TLA+ for the core state machine | Not adopted | Single process, and past problems were all in the implementation; in-process concurrency uses shuttle (§7.3) |
| Performance regression thresholds in CI | Not adopted | CI noise exceeds 15% and would alarm repeatedly; performance is not a guarantee yet. Reconsider with a fixed benchmark machine |
| Full disk, fd limit, OOM injection | Not adopted | The matching risks in §4 are all low likelihood; reconsider when users report such problems |
| Extracting the register from docs automatically, enforced in CI | Not adopted | The content-level rule tracker and `tools/formal-trace*.mjs` were retired in #1372; `tools/check-guarantees.mjs` now runs only a lightweight forward form check on the 40 core promises (#1376) |
| A TOML DSL for end-to-end scenarios | Not adopted | Few scenarios; Rust is simpler. Reconsider over 15 scenarios, or when non-Rust contributors need to write them |
