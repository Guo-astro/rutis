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
| `release.yml`, `release-cli.yml` | Tags | Releases |

### The nightly `stress.yml`

A test that fails only now and then, when it happens to fail on a pull request, blocks someone whose change has nothing to do with it. `stress.yml` runs such tests many times over every night, so that these failures show up at night first. It does not run on pull requests and does not add to their time. Three jobs run side by side:

| Job | What it does |
| --- | --- |
| `rutis` | The kernel tests, `rounds` times (default 20). Each round changes the number of test threads (1, 2, all cores), shuffles the order and takes a new seed; every fifth round is a release build. Stops at the first failure |
| `multiprocess` | The bridge and loader tests that start processes, open local channels or start runtime processes: the `PLATFORM_TESTS_BRIDGE` and `PLATFORM_TESTS_LOADER` lists of `ci.yml` (a test file added to them runs here too). The same runtimes as the `rust` job: Node 24, Bun 1.4.0, Python 3.12 with websockets, Go oldstable. The tests are built once, then run `rounds` times, each round with a different number of test threads, a shuffled order and a new seed. A failing round does not stop the others; the job fails at the end |
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
| `links` | Linux | `docs` | — | Checks relative links in Markdown |
| `rust` | Linux | `code` | The rerun with runtimes started over loopback, on Linux | Every Rust test, once: the workspace (but bridge and loader), the loader with every row kind, the bridge with every feature. With the oldest supported versions: Node 24 (22 once #219 merges), Bun 1.4.0, Python 3.12, Go oldstable |
| `js-py` | Linux | `code` | — | `scripts/train.mjs`; the Node packages' tests (`node/rutis`, `rutis-runtime`, `rutis-host`) and rutis-dsh's dsh tests; the Python unit tests |
| `checks` | Linux | Only with `all` | The whole job | `cargo check --workspace --all-targets`; build checks and `--no-run` of the bridge and loader feature combinations. They find rare feature-combination build errors and run no tests |
| `network-macos` | macOS | `code`: only the tests that depend on the platform | All bridge and loader tests; the `node/rutis` tests | Process start, local channels, runtime processes, WebSocket and rutis-host tests with Node 26, the latest Bun and Go stable; the Node runtime's and the Python package's tests; a build check of the whole workspace |
| `runtimes-windows` | Windows | `code`: only the tests that depend on the platform | All bridge and loader tests | The same on Windows (processes start over loopback); the Go SDK tests; a build check of the whole workspace |
| `runtimes-bun` | Linux | `bun`: Bun 1.4.0 | Linux latest; macOS 1.4.0 | Every Bun runtime test, with the reruns over loopback |
| `runtimes-go` | Linux | `go`: Go oldstable | Linux stable; macOS oldstable; the reruns over loopback on Linux | Every Go SDK and Go runtime test |
| `semver-rutis` | Linux | `packaging` | The whole job | Whether the public API changed incompatibly since the last release (warning only) |
| `dylib-linux-launcher`, `dylib-linux-sdk-bundle` | Linux | `dylib` | — | Dylib tests, split into two jobs that run at the same time |
| `dylib-linux-repro` | Linux | `repro` | The whole job | Reproducible SDK builds (built twice on one machine, in two directories) |
| `dylib-macos` | macOS | `dylib` | The reproducible-build step | The same dylib tests, one after another in one job, sharing one bundle build; the reproducible-build step runs only when `repro` is on |
| `release-dry-run`, `release-windows`, `release-wheel-aarch64` | Linux, Windows, Linux | `packaging` | The whole jobs | The release packages build |
| `sdk-repro`, `sdk-repro-macos`, and their comparisons | Linux, macOS | Only with `all` | The whole jobs | Builds the SDK on two machines and checks the results are identical |
| `ci-ok` | Linux | Always | — | The summary (§5) |

### Tests that depend on the platform

`PLATFORM_TESTS_BRIDGE` and `PLATFORM_TESTS_LOADER`, at the top of `ci.yml`, list the test files macOS and Windows run on pull requests: those that start processes, open local channels (Unix sockets, an inherited socket, loopback handover), start runtime processes or use WebSocket, plus both crates' unit tests (`--lib`); rutis-host's tests all run. The other tests (protocol over memory channels, row configuration and lifecycle, and so on) depend only on the Rust code; pull requests run them on Linux in `rust`, and main runs them on all three platforms.

A new test file that starts processes or opens channels goes into these lists. The `multiprocess` job of the nightly `stress.yml` repeats the tests of these lists too (§2).

### What each kind of change runs on a pull request

| Change | Jobs | Expected time (push to result) |
| --- | --- | --- |
| Documentation only | `changes`, `links` | About 1 minute |
| Kernel `crates/rutis` | `rust`, `js-py`, `network-macos`, `runtimes-windows` | About 5–8 minutes |
| Bridge, loader, host | Adds `runtimes-bun`, `runtimes-go` (one version each, Linux) | About 6–9 minutes |
| Python package only | `rust`, `js-py`, `network-macos`, `runtimes-windows` | About 5–8 minutes |
| `package.json`, `pyproject.toml`, `crates/*/Cargo.toml` | Adds the three packaging jobs and `semver-rutis` | Unchanged; they run beside the tests |
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
4. **Target: an ordinary code pull request gets its result within 10 minutes of the push, queueing included.** Measure it with `node tools/ci-stats.mjs [runs]` (the weekly `ci-stats.yml` runs it too). When it is over, start with the job on the critical path: split it into parallel jobs, or move costly parts that rarely fail to main.

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
2. Install the language in `rust` (the oldest supported version) and `network-macos` (the latest), and add its feature to their existing bridge, loader and host test steps; add its test files to `PLATFORM_TESTS_*` as §4 says.
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
- For tests that fail now and then, first find a way to reproduce the failure, then fix it. Do not hide it with automatic retries.

## 8. Build caches

- Every job uses `Swatinem/rust-cache` with `save-if: github.ref == 'refs/heads/main'`: only runs on main save caches.
- A pull request can read only main's caches and its own. Pull requests save none, so that they do not fill the repository's 10 GB and push main's caches out.
- New jobs do the same (see the note at the `rust-cache` of the `rust` job in `ci.yml`).
- Checks that cannot use caches (reproducible SDK builds) run on main, and on pull requests only when `repro` is on.
