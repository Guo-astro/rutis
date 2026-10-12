# Continuous integration (CI)

[中文](ci.md)

This document describes how CI runs today and what to do when changing it. The rules come from [quality standard](quality-standard.en.md) §12; timings are in [quality status](quality-status.en.md) §8.

## 1. Rules

1. A pull request runs only the jobs related to its change; main runs every job on every push.
2. A new commit on a pull request cancels the older run; runs on main are not cancelled.
3. Merging depends on one check: `ci-ok`.
4. macOS runners are few, so a run uses as few macOS jobs as possible, ideally one.
5. A check runs in one job only.
6. A pull request that changes the CI configuration runs every job.
7. Rust build caches are saved on main only; pull requests read main's caches and save none of their own.
8. Time target: an ordinary code pull request gets its result within 10 minutes of the push, queueing included.

## 2. Workflows

| Workflow | When it runs | What it does |
| --- | --- | --- |
| `ci.yml` | Every pull request; every push to main | Tests and checks (below) |
| `dylib-windows.yml` | Pull requests touching dylib files (the same files as the `dylib` switch); every push to main; `v*` tags; by hand | Dylib tests on Windows |
| `stress.yml` | Nightly; by hand | Kernel tests and multi-process tests repeated; long network tests (below) |
| `ci-stats.yml` | Weekly; by hand | CI timings and failures |
| `deps.yml` | Weekly; by hand | Dependency vulnerability checks: `cargo deny check advisories` (without the example projects), `npm audit` of `@arcships/rutis-runtime`, `pip-audit` of the Python package. Not on pull requests and does not block merges; GitHub notifies the maintainer on failure |
| `install.yml` | Every push to main; pull requests touching packaging, build configuration or the install tests (`package.yml`, `install.yml`, `tests/e2e/**`, the packages' `package.json` and `pyproject.toml`, `node/rutis-host/scripts/**`, `Cargo.toml`, `Cargo.lock` and others); called by `release.yml` on a release; by hand | Builds this commit's packages with `package.yml`, installs and runs them: npm, PyPI, the release binary. Three platforms on main (linux-x64, darwin-arm64, win32-x64), only linux-x64 on pull requests. linux-arm64 and darwin-x64 (the aarch64 wheel among them) are built only in the five-platform install check before a release (`release.yml`) |
| `release.yml`, `release-cli.yml` | Tags | Releases |

### The nightly `stress.yml`

A test that fails only now and then, when it happens to fail on a pull request, blocks someone whose change has nothing to do with it. `stress.yml` runs such tests many times over every night, so that these failures show up at night first. It does not run on pull requests and does not add to their time. Three jobs run side by side:

| Job | What it does |
| --- | --- |
| `rutis` | The kernel tests, `rounds` times (default 20). Each round changes the number of test threads (1, 2, all cores), shuffles the order and takes a new seed; every fifth round is a release build. Stops at the first failure |
| `multiprocess` | The bridge and loader tests that start processes, open local channels or start runtime processes: the test files pull requests run on macOS and Windows, every test file but the `PURE_TESTS_BRIDGE` and `PURE_TESTS_LOADER` lists of `ci.yml` (§4; a new test file runs here too). The same runtimes as the `rust` job (§9). The tests are built once, then run `rounds` times, each round with a different number of test threads, a shuffled order and a new seed. A failing round does not stop the others; the job fails at the end |
| `soak` | The bridge's long WebSocket and local-channel tests, `soak_secs` seconds each (default 600) |

The slowest job sets the time of the workflow: `soak` takes about 21 minutes; `multiprocess` an estimated 30–45 minutes (about 10 minutes to build, 1–1.5 minutes a round).

- Failures are not retried (standard Q7.3).
- Each round prints its thread count and seed in the log. The job summary has a table of every round's result, the failing tests and the rounds they failed in, and replay commands that rerun the test file with the same seed, thread count and order.
- When `multiprocess` fails, the output of every test file in every round is uploaded as the `multiprocess-logs` artifact, kept for 14 days.
- To run it by hand: on the Actions page pick `stress` and "Run workflow", where `rounds` and `soak_secs` can be changed; or `gh workflow run stress.yml -f rounds=50`. All three jobs run.
- A failure found at night is handled as in §7: open an issue, then find a way to reproduce it.

## 3. How a pull request picks its jobs

The first job of `ci.yml`, `changes`, looks at the files the pull request changed and sets the switches below. Each later job runs or not depending on a switch. On a push to main every switch is on.

| Switch | On when these files change |
| --- | --- |
| `all` | `Cargo.lock`, the root `Cargo.toml`, `rust-toolchain.toml`, `.github/workflows/**`. Turns every other switch on too |
| `code` | Any file except `docs/**` and `*.md` |
| `dylib` | The dylib crates: `rutis-dylib*`, `rutis-sdk`, `rutis-dev`, `rutis-xtask`, and `rutis-cli`, which links the SDK and holds the dylib host tests; `tools/*dylib*`, `tools/build-dylib-bundle.sh`, `tools/test-sdk-bundle.sh`, `tools/lib/**`, `tests/dylib-fixtures/**`. **Not the kernel `crates/rutis`**: kernel changes reach the SDK too, but the dylib jobs have not failed in the last 88 runs, so kernel pull requests no longer wait 20 minutes for them; main checks them after the merge |
| `packaging` | `scripts/train.mjs`, every `pyproject.toml` and `package.json`, `crates/*/Cargo.toml`, `node/rutis-host/scripts/**` |
| `docs` | Any `.md` file |
| `bun` | `bun/**`, `node/rutis/**`, `crates/rutis-bridge`, `crates/rutis-loader`, `crates/rutis-host` |
| `go` | `go/**`, `crates/rutis-bridge`, `crates/rutis-loader`, `crates/rutis-host` |
| `repro` | `crates/rutis-sdk`, `crates/rutis-cli/build.rs`, `tools/test-dylib-repro.sh`, `tools/build-dylib-bundle.sh`, `tools/lib/**`. Kernel changes reach the SDK too, but its reproducibility check for them runs on main |

## 4. Jobs

The "Pull requests" column says when a job runs on a pull request and what it runs there. On main (and on pull requests with `all` on) every job runs, in full.

| Job | Runner | Pull requests | Added on main | What it does |
| --- | --- | --- | --- | --- |
| `links` | macOS (self-hosted, §10) | `docs` | — | Checks relative links in Markdown |
| `rust` | Linux | `code` | The rerun with runtimes started over loopback, on Linux | Every Rust test, once: the workspace (but bridge, loader and `rutis-e2e`), the loader with every row kind, the bridge with every feature. With the oldest supported versions (§9) |
| `e2e` | Linux | `code` | — | The end-to-end scenarios, `cargo test -p rutis-e2e` (`tests/e2e`: real rutis-host processes, project directories, probe plugins of every language), beside `rust`, with the oldest supported versions (§9). On failure it uploads `RUTIS_E2E_DIR` (the failed scenarios' directories and logs) as the `e2e-linux` artifact. The install scenarios run in `install.yml`, not here. The same scenarios run on macOS in `network-macos` and on Windows in `runtimes-windows` (only those not limited to Unix) |
| `js-py` | macOS (self-hosted, §10) | `code` | — | `scripts/train.mjs`; the Node packages' tests (`node/rutis`, `rutis-runtime`, `rutis-host`) and rutis-dsh's dsh tests; the Python unit tests |
| `lint` | Linux | `code` | — | `cargo fmt --check`; every name in `PURE_TESTS_*` has its test file (§4); `cargo clippy --workspace --all-targets --all-features -- -D warnings`. In parallel with the tests. No baseline: a warning is fixed in the PR that introduces it, and new lints from a toolchain upgrade in the upgrade PR |
| `msrv` | macOS (self-hosted, §10) | Only with `all` (`Cargo.lock`, `Cargo.toml` and the like changed) | The whole job | Builds the libraries and binaries with the declared MSRV (`rust-version` in `Cargo.toml`) and the repository's `Cargo.lock` (`cargo +<version> check --workspace`, without the example projects `rutis-agent` and `rutis-cli`) |
| `checks` | macOS (self-hosted, §10) | Only with `all` | The whole job | Build checks and `--no-run` of the bridge and loader feature combinations (the whole workspace's build is covered by `lint`'s clippy). They find rare feature-combination build errors and run no tests |
| `network-macos` | macOS | `code`: only the tests that depend on the platform | All bridge and loader tests; the `node/rutis` tests | Process start, local channels, runtime processes, WebSocket and rutis-host tests with the latest versions (§9); the end-to-end scenarios (on failure, the `e2e-macos` artifact); the Node runtime's and the Python package's tests; a build check of the whole workspace |
| `runtimes-windows` | Windows | `code`: only the tests that depend on the platform | All bridge and loader tests | The same on Windows (processes start over loopback); the Go SDK tests; the E2E harness and the E2E scenarios not limited to Unix (`rutis-e2e`: Job Objects, Ctrl-Break, residue checks); a build check of the whole workspace |
| `runtimes-bun` | Linux | `bun`: Bun 1.4.0 | Linux latest; macOS 1.4.0 | Every Bun runtime test, with the reruns over loopback |
| `runtimes-go` | Linux | `go`: Go oldstable | Linux stable; macOS oldstable; the reruns over loopback on Linux | Every Go SDK and Go runtime test |
| `semver-rutis` | macOS (self-hosted, §10) | `packaging` | The whole job | Whether the public API changed incompatibly since the last release (warning only) |
| `dylib-linux-launcher`, `dylib-linux-sdk-bundle` | Linux | `dylib` | — | Dylib tests, split into two jobs that run at the same time |
| `dylib-linux-repro` | Linux | `repro` | The whole job | Reproducible SDK builds (built twice on one machine, in two directories) |
| `dylib-macos` | macOS (self-hosted, §10) | `dylib` | The reproducible-build step | The same dylib tests, one after another in one job, sharing one bundle build; the reproducible-build step runs only when `repro` is on |
| `release-dry-run` | macOS (self-hosted, §10) | `packaging` | The whole job | The release packages build (crates, npm packages, Python packages). `install.yml` builds and installs the binaries and wheels of each platform (§2): only Linux x64 on pull requests; Windows and macOS packaging is checked on main after the merge, aarch64 before a release |
| `sdk-repro`, `sdk-repro-macos`, and their comparisons | Linux, macOS (self-hosted, §10) | Only with `all` | The whole jobs | Builds the SDK on two machines and checks the results are identical (the comparisons run on the self-hosted macOS) |
| `ci-ok` | macOS (self-hosted, §10) | Always | — | The summary (§5) |

### Tests that depend on the platform

On pull requests, macOS and Windows run every bridge and loader test file and both crates' unit tests (`--lib`), except the test files listed in `PURE_TESTS_BRIDGE` and `PURE_TESTS_LOADER` at the top of `ci.yml`; rutis-host's tests all run. The listed tests depend only on the Rust code: protocol over memory channels, row configuration and lifecycle, and so on. They start no process, open no socket, touch no file but fixtures they read, and depend on nothing of the operating system, so they behave the same on every platform; pull requests run them on Linux in `rust`, and main runs them on all three platforms. Skipping them keeps the macOS and Windows time bounded.

`tools/platform-tests.sh` builds the `--lib --test <name> ...` flags for `cargo test` when the job runs: every test file under `crates/<crate>/tests/` but the listed names. A new test file therefore runs on all three platforms without anyone remembering to register it; only a test that starts nothing and opens no channel or file goes into `PURE_TESTS_*`. A listed name without its file is an error of the script and fails the `lint` job, so a renamed or removed test file cannot leave a list silently stale. The `multiprocess` job of the nightly `stress.yml` repeats the same tests with the same script (§2).

### The conformance matrix

Where bridge's conformance suites run: which implementations, which channels, which platforms (risk C10, #176). Kept by hand: a change to a cell or a skip reason changes this table too. A skipped cell is marked `ignore = "reason"` in its test, which `cargo test` prints; where a whole platform does not compile a test, the reason is a comment next to its `cfg`.

The session contract (`session::testing::session` and the malformed-frame checks `malformed`, `tests/runtime_conformance.rs`):

| Implementation | Channel | Linux | macOS | Windows |
| --- | --- | --- | --- | --- |
| Rust | memory | Runs | Runs | Runs |
| Node, Python | Unix socket | Runs | Runs | Skipped: no Unix sockets on Windows, where runtimes connect on loopback |
| Node, Python | loopback TCP (with a one-time token) | Runs | Runs | Runs |
| Go | Unix socket | Runs | Runs | Skipped: as above |
| Go | loopback TCP | Runs | Runs | Runs |
| Node, Python | WebSocket (the fixture listens, Rust dials) | Runs | Runs | Runs |
| Go | WebSocket | Not run: the Go SDK's WebSocket listener is internal to the runtime (`package rutis`), out of reach of the session fixture, a separate program; the Go runtime's WebSocket column is in the runtime contract | As Linux | As Linux |
| Bun | Unix socket, loopback TCP | Runs (malformed frames on the Unix socket only) | Runs | Not compiled: Bun on Windows is not a declared platform |
| Bun | WebSocket | Not run: the Bun runtime has no WebSocket channel | As Linux | As Linux |

The runtime contract (`runtime::testing::runtime`, `tests/session_matrix.rs`):

| Implementation | Channel | Linux | macOS | Windows |
| --- | --- | --- | --- | --- |
| Node, Python, Go | inherited socket (`fd:3`) | Runs | Runs | Skipped: Windows hands no descriptor over, only loopback |
| Node, Python, Go | socket path dialed back | Runs | Runs | Skipped: as above |
| Node, Python, Go | loopback TCP (the local transport's `Handover::Loopback`, as `LocalRuntime` starts runtimes, then `Process::attach`) | Runs | Runs | Runs |
| Node, Python, Go | WebSocket (the runtime listens, Rust dials) | Runs | Runs | Runs |
| Bun | `fd:3`, socket path dialed back | Runs; `runtimes-bun` reruns with `RUTIS_LOCAL_HANDOVER=loopback` | Runs | Not compiled: as above |
| Bun | loopback TCP, WebSocket | No column of their own: the rerun of the row above covers loopback; the Bun runtime has no WebSocket channel | As Linux | Not compiled: as above |
| A Node package without `fd` falls back to dialing back | — | Runs | Runs | Skipped: Windows has loopback only |

The first two columns and the last row start the process through the `Process` facade, and the contract also checks the exit status after the crash; the loopback and WebSocket columns attach to a channel and check only that the crash ends the session. On Windows, `python_runtime.rs`, `go_runtime.rs` and others cover the facade's loopback start.

The node contract (`rutis_bridge::testing::node`):

| Implementation | Channel | Linux | macOS | Windows |
| --- | --- | --- | --- | --- |
| Rust node | memory (`tests/node_conformance.rs`) | Runs | Runs on main | Runs on main |
| Rust node | WebSocket (`tests/websocket_conformance.rs`, two rutis roots linked over a loopback WebSocket) | Runs | Runs | Runs |
| Cordis node | WebSocket (`tests/websocket_conformance.rs`) | Runs | Runs | Runs |

Other bridge test files compiled on Unix only (each says why next to its `cfg`): `bun_runtime.rs`, `bun_channel.rs` (Bun); `local_spawn.rs` (the `fd:3` and dial-back handovers; `local_loopback.rs` covers Windows' loopback handover); `local_plugin.rs`, `value_roundtrip.rs` (Unix sockets); `local_stdio.rs` (`dup2`); `concurrent_launch.rs` (descriptors inherited across fork/exec); the two soaks on Linux only (they read `/proc`).

### What each kind of change runs on a pull request

| Change | Jobs | Expected time (push to result) |
| --- | --- | --- |
| Documentation only | `changes`, `links` | About 1 minute |
| Kernel `crates/rutis` | `rust`, `e2e`, `js-py`, `lint`, `network-macos`, `runtimes-windows` | About 5–8 minutes |
| Bridge, loader, host | Adds `runtimes-bun`, `runtimes-go` (one version each, Linux) | About 6–9 minutes |
| Python package only | `rust`, `e2e`, `js-py`, `lint`, `network-macos`, `runtimes-windows` | About 5–8 minutes |
| `package.json`, `pyproject.toml`, `crates/*/Cargo.toml` | Adds `release-dry-run` and `semver-rutis`; `install.yml` (Linux x64) when a package's `package.json` or `pyproject.toml` changes | Unchanged; they run beside the tests |
| Dylib | Adds the dylib jobs; `dylib-windows.yml` | About 20–25 minutes (the dylib budget) |
| CI configuration, `Cargo.lock`, the root `Cargo.toml` | Everything, as on main | About 25 minutes |

## 5. Merge condition

- `ci-ok` is the only check required on main.
- `ci-ok` looks only at the jobs listed in its `needs`. It passes when each of them succeeded or was skipped, and fails when any of them failed or was cancelled.
- A job not listed in `ci-ok` does not stop a merge, even when it fails.

## 6. Changing CI

### Adding a CI check

CI gets slow one added check at a time. Before adding a check (a new job, a new step in a job, a new version or platform), answer these in order:

1. **What problem does it prevent, and in which tier does it run?** The tiers are pull requests, main, daily (`stress.yml`) and weekly. The default is main or daily; only problems that must be stopped before a merge go on pull requests. A check on main still finds the problem, a little later, and a failure there is fixed first (§7).
2. **On pull requests, how much time does it take?** It must not make the critical path (the slowest job) longer: put it in a parallel Linux job, or replace something in an existing job. No new macOS or Windows jobs; macOS and Windows run only the tests that depend on the platform (§4).
3. **Replace rather than add.** For example, the oldest version of a support matrix: switch the Linux job to the oldest version and keep macOS on the latest, instead of adding a job for the oldest version.
4. **Target: an ordinary code pull request gets its result within 10 minutes of the push, queueing included.** Measure it with `node tools/ci-stats.mjs [runs] [workflow] [--since date]` (the weekly `ci-stats.yml` runs it too) and read the "ordinary code" row: first attempts that ran `rust` but neither `checks` nor a `dylib-` job. When it is over, start with the job on the critical path: split it into parallel jobs, or move costly parts that rarely fail to main.
5. **Checks whose result changes with outside data stay off pull requests.** Advisory databases and new releases can fail a PR whose code did not change; such checks run weekly (`deps.yml`).

Write the answers in the pull request description.

### Adding a job

1. First decide its tier with the steps above.
2. Give it `needs: changes` and `if: needs.changes.outputs.<switch> == 'true'` (`all` for a job that runs only on main). If no switch fits, add one to `changes` and to §3.
3. Add it to `ci-ok`'s `needs`.
4. Check whether an existing job already does the same check. If the work fits in an existing job, put it there instead of adding a job.
5. Before using macOS, see whether the work fits in `network-macos`. An ordinary change uses at most 1 macOS job; a dylib change at most 2.
6. Add it to §4.

### Adding a language runtime

1. Add a switch for the language to `changes`, covering its runtime directory and `crates/rutis-bridge`, `crates/rutis-loader`, `crates/rutis-host`.
2. Install the language in `rust` and `e2e` (the oldest supported version) and `network-macos` (the latest) (§9), and add its feature to their existing bridge, loader and host test steps; its test files run on macOS and Windows by default (§4), with nothing to register.
3. To test several versions, add a `runtimes-<language>` job: it runs on the switch from step 1, is listed in `ci-ok`, and on pull requests runs only the oldest supported version on Linux. Other versions and macOS run only on main (see `runtimes-bun` and the `bun-matrix` output of `changes`).
4. Releases: add its package to `release-dry-run`, `scripts/train.mjs` and `release.yml`.
5. Update §3 and §4.

### Changing which files turn a switch on

- Changing `changes` changes `.github/workflows/**`, so that pull request runs every job.
- If a job was skipped on a pull request and then failed on main after the merge, a switch is missing some files. After fixing the failure, add those files to the switch.

### Changing the CI configuration

- Use a pull request of its own, with commit type `ci:`.
- A pull request that changes the CI configuration runs everything, so what runs only on pull requests (such as the macOS and Windows test subsets) does not run in it; check the next ordinary pull request's run after the merge.
- After it merges, check the first run on main.

## 7. Failures

- Fix failures on main first. Find which merge caused them; until they are fixed, do not merge changes they may affect.
- If a run on main is cancelled by hand, run it again. Every merge needs a complete result.
- On main, `ci.yml`, `install.yml` and `dylib-windows.yml` each have at most one run in progress and one waiting (the concurrency group is per ref, not per commit): the run in progress is never cancelled; another merge replaces the waiting run. So during a burst of merges not every merge commit gets a run of its own; the next run checks every merge before it, and when main fails, the merges a failed run covers are those between its commit and the previous run's. On a pull request, a new push cancels the run in progress. A release (a tag) has its own ref: the `install.yml` that `release.yml` calls never replaces main's runs or is replaced by them.
- For tests that fail now and then, first find a way to reproduce the failure, then fix it. Do not hide it with automatic retries.

## 8. Build caches

- Every job uses `Swatinem/rust-cache` with `save-if: github.ref == 'refs/heads/main'`: only runs on main save caches.
- A pull request can read only main's caches and its own. Pull requests save none, so that they do not fill the repository's 10 GB and push main's caches out.
- The self-hosted macOS runners use no `rust-cache` and keep their own target directory (§10).
- New jobs do the same (see the note at the `rust-cache` of the `rust` job in `ci.yml`).
- Checks that cannot use caches (reproducible SDK builds) run on main, and on pull requests only when `repro` is on.

## 9. Versions

Rule: for each language, Linux jobs use the oldest supported version, macOS and Windows jobs the latest. The minimum is verified on every pull request, the latest on macOS and Windows, with no job added just for a version.

| | Declared in | Linux (`rust`, `e2e`, `js-py`, `checks`, `lint`, `runtimes-go`, `runtimes-bun`, `stress.yml`'s `multiprocess`) | `network-macos`, `runtimes-windows` |
| --- | --- | --- | --- |
| Node | `engines.node` in each npm package's `package.json` | The declared oldest major | The latest even major, bumped by hand |
| Python | `requires-python` in `python/rutis/pyproject.toml` | The declared oldest | `3.x`, follows automatically; when a new release fails a PR, pin the previous one temporarily and fix it in a separate PR |
| websockets | The `network` extra in `python/rutis/pyproject.toml` | `==<lower bound>.*` | `>=<lower bound>` |
| Bun | `engines.bun` in `bun/rutis-bun/package.json` | The declared oldest | `latest` |
| Go | `go/rutis/go.mod` | `oldstable` | `stable` |
| Rust toolchain | `rust-toolchain.toml` | Every workflow's `dtolnay/rust-toolchain@<version>` equals it | Same |
| MSRV | `rust-version` in the root `Cargo.toml` | The `msrv` job (main) | — |

The end-to-end scenarios (`rutis-e2e`) run on all three platforms: on Linux in `e2e` (oldest versions), on macOS in `network-macos` and on Windows in `runtimes-windows` (latest versions; on Windows only the scenarios not limited to Unix).

In the macOS entries of `runtimes-go` and `runtimes-bun` on main, Node and Python are only the far end of cross-language rows and use the oldest versions, as on Linux. The Node used for releases (`release.yml`, `package.yml` and `ci.yml`'s `release-dry-run`) is the release tooling's version and not part of the support matrix.

**Change CI versions in the same PR as a declaration**: a PR that changes `engines`, `requires-python`, the websockets lower bound or `rust-version` also changes the versions of the matching jobs in the table above; otherwise the declared minimum is unverified (Q10.3).

## 10. Self-hosted macOS runners

`ci.yml`'s `dylib-macos` and `sdk-repro-macos`, and `install.yml`'s `darwin-arm64` on main and pull requests, run on self-hosted macOS runners: one dedicated, isolated Mac with one or more runner instances, all labelled `self-hosted, macOS, ARM64` (the first is `macos-1`). The jobs whose results do not depend on the OS run there too, so that they do not queue for GitHub's Linux runners: `ci.yml`'s `links`, `js-py` (Node 22, Python 3.10, websockets 15; Python from uv), `msrv`, `checks`, `release-dry-run`, `semver-rutis` and the two SDK comparisons, and the orchestration jobs `changes` and `ci-ok`, and the `plan` of `install.yml` and `package.yml` (for a release, on a tag, `plan` stays on `ubuntu-24.04`). With a single instance they wait behind a running macOS job, hence more instances (now `macos-1` to `macos-4`). `msrv` and `checks` compile on macOS and so skip Linux-only cfg code; `lint` (clippy, every feature) on Linux compiles it. On Linux stay `rust`, `e2e`, `lint`, the ubuntu entries of `runtimes-go`/`runtimes-bun`, `dylib-linux-*` and `sdk-repro`. `network-macos` runs on the self-hosted machine too (the repository variable `NETWORK_MACOS_RUNNER` is `["self-hosted","macOS","ARM64"]`; delete it to go back to GitHub's `macos-15`; it stayed on GitHub because of #287 until #344 fixed it). The other jobs stay on GitHub's runners: `darwin-x64` (it needs the Intel `macos-15-intel`), `package.yml`'s macOS builds (release files), `darwin-arm64` when `release.yml` calls `install.yml` (the pre-release gate uses `macos-15`), the macOS entries of `runtimes-go`/`runtimes-bun` on main; for the Linux and Windows jobs see §11.

**Back to GitHub's runners without a code change**: the repository variable `MACOS_RUNNER` (Settings → Secrets and variables → Actions → Variables) is the JSON value of `runs-on`; unset, it is `["self-hosted","macOS","ARM64"]`. Set it to `"macos-15"` (with the quotes) and every one of these jobs goes back to GitHub's macOS; delete it to come back. `gh variable set MACOS_RUNNER --body '"macos-15"'`. The orchestration jobs follow another variable, `GATE_RUNNER`: set it to `"ubuntu-24.04"` to move them back.

A self-hosted machine keeps its state between jobs, and several instances run at once. These jobs run `tools/ci-self-hosted.sh` at their start and their end (`if: always()`):

- it ends the processes an earlier job of this instance left: those whose command line or working directory is under this instance's workspace, target directory or scenario directory; the runner itself and other instances' jobs are left alone;
- the scenario directory `RUTIS_E2E_DIR` is the instance's own `/tmp/rutis-e2e-<runner name>` (not `runner.temp`: a Unix socket path is at most 104 bytes on macOS, see `tests/e2e/src/lib.rs`), emptied at the start and the end;
- builds use no `rust-cache` (it would upload the cache to GitHub again); `CARGO_TARGET_DIR` is the instance's kept `~/.cache/rutis-ci/<runner name>/target`, emptied above 30 GB (`RUTIS_CI_TARGET_MAX_GB`); when the machine's `cargo` is the shim of mbx (a build cache), `CARGO_TARGET_DIR` is not set and mbx keeps the target directory, and this directory is removed (used by both, one target directory came to hold a `rutis-sdk` and a `rutis-cli` built against two different `serde_json`); the caches of `setup-node` and `setup-go` are used on GitHub's runners only;
- Python comes not from `setup-python` (its macOS builds install only under `/Users/runner`) but from uv: its latest Python, in a virtual environment with pip that each job makes in `runner.temp` and puts on `PATH`;
- directories `tools/*.sh` leave in `/tmp/rutis-*` are removed after 12 hours;
- several instances compiling at once overload the machine (4 instances on 10 cores each running 10 rustc reached a load of 79): each job's `CARGO_BUILD_JOBS` and `RUST_TEST_THREADS` are the CPU count divided by the machine's instances (the `~/actions-runner*/` directories with a `.runner` file), at least 2; the start prints them and the load. When instances live elsewhere, or for another value, set the repository variable `RUTIS_CI_JOBS`. `sdk-repro-macos` is limited the same way, which does not change what it builds.

**Before adding an instance, the machine needs**:

1. The runner registered with `./config.sh --labels self-hosted,macOS,ARM64 --name macos-<n>` (a name unique on the machine: directories and caches go by it) and run as a service: `./svc.sh install && ./svc.sh start`.
2. A file descriptor limit of 10240: `SoftResourceLimits` and `HardResourceLimits`, each with `NumberOfFiles` = `10240`, in `~/Library/LaunchAgents/actions.runner.*.plist`, then `./svc.sh stop && ./svc.sh start`.
3. rustup (`dtolnay/rust-toolchain` installs the `rust-toolchain.toml` version with it): `curl https://sh.rustup.rs -sSf | sh -s -- -y --default-toolchain none`.
4. The Xcode command line tools (`xcode-select --install`): `cc`, `codesign`, `otool`, `lipo`. `sdk-repro-macos` prints the Xcode version, or the command line tools' when there is no Xcode.
5. At least 100 GB of free disk (up to 30 GB of target directory per instance, plus Node, Python, Go and Bun in `_work/_tool`).
6. No sleep: `sudo pmset -a sleep 0 disksleep 0` (or in System Settings).
7. No Homebrew: these jobs use only the system's `bash` (3.2), `perl`, `tar`, `shasum`, `lsof` and `jq` (`/usr/bin/jq` comes with macOS 15 and later; `ci-ok` uses it); Node, Go, Bun and uv come from their setup actions into `_work/_tool`, Python from uv into `~/.local/share/uv`.

## 11. Self-hosted Linux runners

Where `ci.yml`'s Linux jobs (`rust`, `e2e`, `lint`, the Linux entries of `runtimes-go`/`runtimes-bun`, `dylib-linux-*`, `sdk-repro`) run is the repository variable `LINUX_RUNNER` (the JSON value of `runs-on`): unset, GitHub's `"ubuntu-24.04"`; set to `["self-hosted","Linux"]`, the organization's self-hosted Linux runners; delete it to go back to GitHub. The other workflows (`package.yml`, which needs Docker containers, `install.yml`, `stress.yml` and the rest) are not affected.

These runners are 4 Docker containers on one machine (32 cores, 60 GB of memory), one runner each, user `runner` with passwordless sudo, with Rust, Node, Go and Python installed and no `lsof`, Docker or Bun (`setup-bun` installs Bun). The jobs run `tools/ci-self-hosted.sh` at their start and their end as well (previous section), with these differences:

- a container has one runner and its own `/tmp`: the scenario directory is the shortest, `/tmp/rutis-e2e`;
- the processes' working directories come from `/proc` (there is no `lsof`);
- a container sees the whole machine's CPUs but not the other containers: each job's parallelism is the repository variable `LINUX_CI_JOBS` (`8` with 4 containers), or every CPU when it is unset;
- `setup-python` works on self-hosted Linux, and Python still comes from it.

`runtimes-windows` likewise goes by the repository variable `WINDOWS_RUNNER`: unset, `"windows-2025"`; set to `["self-hosted","Windows"]`, the organization's self-hosted Windows runners (4 instances on one 16-core machine, run as `NETWORK SERVICE`; Rust in `C:\Rust`, with the VS Build Tools, Git, Node, Go and Python). Its steps use Git's bash: the machine's system `PATH` needs `C:\Program Files\Git\bin` (restart the runner services after changing it). Self-hosted, each instance keeps its target and scenario directories under `_work/_rutis-ci/` (`checkout` cleans a `target` in the checkout); Python comes not from `setup-python` (its Windows installer needs an administrator) but from the machine, in a virtual environment in `runner.temp`; and its parallelism is the CPU count divided by 4, or the repository variable `WINDOWS_CI_JOBS`. The machine needs long paths: `LongPathsEnabled` = 1 in the registry, and `git config --system core.longpaths true`.
