# CI static checks and the minimum version matrix (design)

[中文](design-ci-checks-matrix-2026-10-11.md)

Status: design, under review (#204). Date: 2026-10-11. Base: `main` `0941ef4`.
Based on: [quality standard](quality-standard.en.md) Q5.3.7, Q10.2, Q10.3, §12; [CI](ci.en.md) (#237); #204's "scope change (2026-10-10)". Part of step 1 of #183.

Hard constraints (from #204): ordinary code PRs get a result within 10 minutes of the push, queueing included; no new macOS or Windows jobs on PRs; new checks go into parallel Linux jobs; replace rather than add.

Out of scope: actionlint; Go's `govulncheck`; Python style checks; license checks.

## 1. Decisions

| # | Decision | Section |
| --- | --- | --- |
| D1 | Versions by machine: Linux jobs use each language's minimum supported version, macOS and Windows the latest | 3 |
| D2 | websockets: Linux installs `websockets==15.*` (the declared lower bound), everywhere else `websockets>=15` | 3 |
| D3 | A new parallel Linux job `lint` (`code` switch): `cargo fmt --check`, `cargo clippy -D warnings`; this PR fixes the existing 4 format differences and 22 clippy warnings; no baseline | 4 |
| D4 | One MSRV, 1.88; the example projects `rutis-agent` and `rutis-cli` declare none; checked once on main | 5 |
| D5 | Vulnerability checks go into a new weekly workflow `deps.yml` and do not block merges; on failure GitHub's failed-workflow notification tells the maintainer | 6 |
| D6 | `cargo check --workspace --all-targets` is removed from `checks`: `lint`'s clippy already compiles the same code | 4 |
| D7 | `tools/ci-stats.mjs` can report ordinary code PRs on their own; acceptance looks at that class | 8 |
| D8 | CI changes other PRs need (#186's `e2e` job, #193's packaging and install) are added by this issue when those PRs land | 7 |

## 2. Current state (measured)

### 2.1 PR time

`node tools/ci-stats.mjs 40 ci.yml` (2026-10-11, the 40 runs after #237 merged) reports a `pull_request` median of 19.5 minutes, but that mixes PRs with `all` on (about 20–40 minutes), ordinary code PRs and re-run runs (a re-run counts from the first creation; run `38056277205` shows as 659 minutes). Ordinary code PRs only (7–9 jobs, first attempt, success):

| run | branch | push to result (minutes) |
| --- | --- | ---: |
| 38097374545 | fix/238-multilang-go-flaky | 5.2 |
| 38056775967 | fix/233-exit-status-flaky | 5.6 |
| 38055889193 | fix/239-handshake-flaky | 7.4 |
| 38098778901 | fix/173-local-line-limit | 8.7 |
| 38098889402 | fix/184-concurrent-launch | 8.7 |
| 38100944545 | fix/247-row-self-dispose | 8.8 |

Median about 8.0 minutes (6 runs, a small sample). The critical path is `rust` (about 4.5 minutes) and `runtimes-windows` (4.2–5.1 minutes). Linux jobs also queue (1.8–2.3 minutes) because every push to main takes 28 jobs at once.

### 2.2 fmt and clippy

- `cargo fmt --all --check` (1.98.1): 4 differences, in `crates/rutis-xtask/src/main.rs` (3) and `examples/native-mount/tests/cordis_mount.rs` (1).
- `cargo clippy --workspace --all-targets --all-features` (1.98.1, cold build 56 s): 22 warnings, 9 lints, no errors.

| Lint | Count | Where |
| --- | ---: | --- |
| `clippy::await_holding_lock` | 7 | `crates/rutis-agent/tests/session_persist.rs` |
| `dead_code` | 5 | `tests/dylib-fixtures/greeter-v2/src/lib.rs` (4, only with `--all-features`: some features are mutually exclusive versions); `crates/rutis-loader/tests/migration_example.rs` (1) |
| `clippy::doc_lazy_continuation` | 3 | `crates/rutis-agent/src/driver.rs`, `session.rs` |
| `clippy::missing_safety_doc` | 2 | `crates/rutis-sdk/src/lib.rs`, `crates/rutis-dylib/src/loader.rs` |
| `clippy::type_complexity` | 1 | `crates/rutis-bridge/tests/memory_mux.rs` |
| `clippy::map_flatten`, `unnecessary_mut_passed`, `items_after_test_module` | 1 each | `crates/rutis-agent/src/driver.rs`, `tui.rs` |
| `clippy::redundant_closure` | 1 | `crates/rutis-cli/src/main.rs` |

Two prerequisites, the same on CI: `--all-features` turns on the `export` feature of `tests/dylib-fixtures/greeter-*`, which reads `RUTIS_SDK_ARTIFACT_SHA256` at compile time; CI sets it to 64 `0`s as `sdk-repro` does. The build.rs of `examples/native-mount` and `examples/interop-experiments` needs `npm --prefix node/rutis-runtime ci` first, and `rutis-dsh` needs `npm --prefix crates/rutis-dsh/dsh ci`.

### 2.3 MSRV

Declared: `rust-version = "1.85"` in the root `Cargo.toml`, inherited by every crate. Checked with the current `Cargo.lock` (`--locked`):

| Command | Result |
| --- | --- |
| `cargo +1.85 check --workspace` | Fails: dependencies need a newer rustc (`icu_*` 2.3 and `darling` 0.24 need 1.88, `idna_adapter` 1.2.2 needs 1.86, `tree-sitter-language` 0.1.8 needs 1.90) |
| `cargo +1.85 check -p rutis -p rutis-bridge --all-features` | Passes |
| `cargo +1.88 check --workspace --exclude rutis-agent --exclude rutis-cli` | Passes |
| `cargo +1.88 check -p rutis-agent` | Fails: `tree-sitter-language` needs 1.90 |

`rutis-loader` and `rutis-host` need 1.88 because of `url` → `idna` → `icu_*`; `rutis-agent` and `rutis-cli` need 1.90 because of rutui's `tree-sitter-language`. Not verified: Linux and Windows targets; which versions users get when they resolve dependencies themselves.

### 2.4 Dependencies

- `cargo deny --all-features check advisories`: no vulnerabilities; 4 "unmaintained" advisories (async-std, bincode, paste, yaml-rust), all through `rutis-agent`'s rutui.
- `npm audit`: nothing in `node/rutis-runtime` or `node/baseline`; 13 in `crates/rutis-dsh/dsh` (not published).
- `pip-audit`: no known vulnerabilities in `websockets==15.0.1`.

### 2.5 Where versions are written

| Where | Node | Python | websockets |
| --- | --- | --- | --- |
| `ci.yml` `rust`, `js-py`, `checks`, `runtimes-go`, `runtimes-bun` | 24 | 3.12 | `>=13` |
| `ci.yml` `network-macos` | 26 | 3.12 | `>=13` |
| `ci.yml` `runtimes-windows` | 24 | 3.12 | `>=13` |
| `release.yml`, `ci.yml` `release-*` | 24 | — | — |
| `release-cli.yml` | — | — | — (Rust from `@stable`; 1.98.1 everywhere else) |
| `stress.yml` `multiprocess` (#245) | 24 | 3.12 | `>=13` |
| Declarations | `engines.node` `>=22` (#219) | `requires-python >=3.12` (before #196) | `websockets>=15` (#218) |

The declarations have changed and CI has not: the Node minimum is 22 but CI runs 24; the websockets lower bound is 15 but CI installs `>=13`.

## 3. Version matrix

Rule: for each language, Linux jobs use the minimum supported version, macOS and Windows the latest. Only version numbers change; no new jobs.

| Job | Machine | Node | Python | websockets |
| --- | --- | --- | --- | --- |
| `rust`, `js-py`, `checks` | Linux | 24 → **22** | 3.12 (**3.10** once #196 merges, changed by #196's PR) | `>=13` → **`==15.*`** |
| `runtimes-go`, `runtimes-bun` (Linux) | Linux | 24 → **22** | Same | `>=13` → **`==15.*`** |
| `network-macos` | macOS | 26 | 3.12 → **`3.x`** | `>=13` → **`>=15`** |
| `runtimes-windows` | Windows | 24 → **26** | 3.12 → **`3.x`** | `>=13` → **`>=15`** |
| `stress.yml` `multiprocess` | Linux | As `rust` | As `rust` | As `rust` |
| `release.yml`, `release-*` | — | 24 (the release tooling's version, not part of the support matrix; unchanged) | — | — |

Also: `dtolnay/rust-toolchain@stable` in `release-cli.yml` becomes `@1.98.1` (the root `rust-toolchain.toml` decides what cargo uses; `@stable` only installs an unused toolchain).

No effect on the critical path: the `ubuntu-24.04` image ships Node 22, and `pip install "websockets==15.*"` takes seconds.

## 4. fmt and clippy: the `lint` job

```yaml
lint:
  needs: changes
  if: needs.changes.outputs.code == 'true'
  runs-on: ubuntu-24.04
```

Steps: checkout; `dtolnay/rust-toolchain@1.98.1` (`rustfmt, clippy`); `Swatinem/rust-cache@v2` (saved on main only); `cargo fmt --all --check`; Node 22 and the two `npm ci` (the prerequisites in 2.2); `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings` with `RUTIS_SDK_ARTIFACT_SHA256` set to 64 `0`s. `-D warnings` goes after `--`, not into `RUSTFLAGS` (changing `RUSTFLAGS` rebuilds every dependency). Only clippy's default lint groups. Added to `ci-ok`.

This PR fixes the 4 format differences and 22 warnings of 2.2 (about 10 files, all mechanical). The `dead_code` in the greeter-v2 fixture comes from mutually exclusive features and gets a plain `#[allow(dead_code, reason = "…")]`. New lints from later toolchain upgrades are fixed in the upgrade PR.

Time: about 3–4 minutes with a cache, in parallel with `rust`, off the critical path; 5–7 minutes without. If after merging `lint`'s median exceeds `rust`'s, first check whether the cache hits, then consider splitting clippy into two parallel jobs.

D6: `cargo check --workspace --all-targets` is removed from `checks`. The same command stays in `network-macos` and `runtimes-windows`, which check their platform's build.

## 5. MSRV

- The root `Cargo.toml`'s `rust-version` becomes `"1.88"`.
- `crates/rutis-agent` and `crates/rutis-cli` are example projects: they drop `rust-version.workspace = true` and declare no MSRV.
- "Rust 1.85" in `README.md`, `README.zh-CN.md` and `docs/guide/README.md` / `.en.md` becomes 1.88.
- Check: a Linux job `msrv`, run with `all` (main, and pull requests that change `Cargo.lock` or `Cargo.toml`): `dtolnay/rust-toolchain@1.88`, `cargo +1.88 check --workspace --exclude rutis-agent --exclude rutis-cli --locked`. Libraries and binaries only (the test dependencies `rcgen` and `time` need 1.88 and pass too, but are not promised). About 3 minutes, not on ordinary PRs.

When it fails later: pin that dependency to an older version (`cargo update -p <dep> --precise <version>`), or raise `rust-version` and update the README and guide with it. Raising `rust-version` is user-visible and the maintainer's decision.

The MSRV is verified only against our `Cargo.lock`; the guide adds one sentence: users on older Rust can resolve dependencies with Cargo's `incompatible-rust-versions = "fallback"`.

## 6. Weekly dependency checks: `deps.yml`

| Job | Command |
| --- | --- |
| `cargo-deny` | `cargo deny --all-features --exclude rutis-agent --exclude rutis-cli check advisories` (the example projects are not checked; the 4 "unmaintained" advisories all come from them) |
| `npm-audit` | `npm --prefix node/rutis-runtime audit --omit=dev --audit-level=high` |
| `pip-audit` | Install `./python/rutis[network]`, then `pip-audit` |

- Triggers: weekly (`schedule`) and manual (`workflow_dispatch`).
- Not in `ci-ok`, does not block merges: the advisory databases change daily, so results change when the code does not; on PRs they would fail unrelated PRs.
- On failure: GitHub sends the failed-workflow notification to the maintainer (@eric8810). No issue-opening automation.
- `crates/rutis-dsh/dsh` (not published) and `node/baseline` (test only) are not checked.
- No `deny.toml`: `advisories` runs with the default configuration.

## 7. CI changes other PRs need

Under the convention that `ci.yml` is changed only by #203 / #204, these are added by this issue when the corresponding PRs land (#196 is the exception and changes its own Linux Python version):

| From | Change |
| --- | --- |
| #186 (#253) | New job `e2e` (Linux, `code` switch, in `ci-ok`) with the same minimum versions as `rust`, running `cargo test -p rutis-e2e`; `--exclude rutis-e2e` on `rust`'s `cargo test --workspace`; `cargo test -p rutis-e2e` in `network-macos` |
| #193 (#254) | On main, call `package.yml` and `install.yml`; remove `release-windows` and `release-wheel-aarch64` |
| #196 (#255) | Linux Python to 3.10 (done by #196's own PR) |

## 8. Measurement and `docs/ci.md`

`tools/ci-stats.mjs`:

- A class "ordinary code PR": `rust` ran, `checks` did not, no `dylib-` job ran, and it is the first attempt (`run_attempt == 1`);
- A `--since <date>` option;
- One output line: `ordinary code PRs: median X minutes, p90 Y, N runs; target ≤ 10`.

Acceptance: call the merge date of the first phase T; at T + 7 days run `node tools/ci-stats.mjs 100 ci.yml --since T`; the ordinary code PR median must be ≤ 10 minutes (with fewer than 10 runs, wait another week); post the result on #204.

`docs/ci.md` and `docs/ci.en.md`: add `lint` and `msrv` to the job table and `deps.yml` to the workflow table; a new "Versions" section with the rule (Linux minimum, macOS and Windows latest), where the declarations live, and "when you change `engines`, `requires-python`, the websockets lower bound or `rust-version`, change the CI versions in the same PR"; remove stale notes such as "change to 22 after #219".

## 9. Phases and acceptance

In this PR, as separate commits:

1. Versions (section 3, without Python 3.10); `release-cli.yml`.
2. The fmt and clippy fixes; the `lint` job; the duplicate command removed from `checks`.
3. MSRV (section 5).
4. `deps.yml`; one `workflow_dispatch` run on this branch.
5. `tools/ci-stats.mjs`; `docs/ci.md`, `docs/ci.en.md`.

Acceptance (all checkable by command):

1. `git grep -n 'websockets>=13' .github` prints nothing; the Linux jobs' `node-version` is 22.
2. The list of `ci.yml` jobs whose `runs-on` is macOS or Windows is the same as at `0941ef4`.
3. `lint` is in `ci-ok`'s `needs`, conditioned on `code`; `cargo fmt --all --check` and `cargo clippy … -- -D warnings` pass locally.
4. After merging, `msrv` succeeds on main; the `Cargo.toml` of `rutis-agent` and `rutis-cli` has no `rust-version`.
5. One manual run of `deps.yml` on this branch succeeds.
6. At T + 7 days: ordinary code PR median ≤ 10 minutes over ≥ 10 runs.

## 10. Maintainer decisions (2026-10-11)

- One rule: Linux uses the minimum versions, macOS and Windows the latest; on Windows Node becomes 26 and Python the latest.
- Python "latest" is written `3.x` and follows automatically; when a new release fails a PR, pin the previous version temporarily and fix it in a separate PR.
- One MSRV, 1.88; the example projects `rutis-agent` and `rutis-cli` declare none.
- The 22 clippy warnings are fixed at once; no baseline.
- Weekly dependency check failures are notified to @eric8810 by GitHub.
- #196 changes its own Linux Python version; the CI changes #186 and #193 need are added by this issue.
