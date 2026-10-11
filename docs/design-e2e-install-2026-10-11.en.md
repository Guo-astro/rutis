# S9 install and run: install smoke tests and documentation examples (design)

[中文](design-e2e-install-2026-10-11.md)

Status: design, under review. Date: 2026-10-11. Base: `main` `0941ef4`.
Issue: #193 (step 1 of the [quality tracker #183](https://github.com/arcships/rutis/issues/183); acceptance for #195 and #196). Takes over the install smoke test, documentation example and E7 items of the closed #180.
Based on: [quality standard](quality-standard.en.md) Q6.15, Q2.13, Q10.2, Q12.1, Q13.1.6, Q13.2; [quality status](quality-status.en.md) risk B1, controls IN and DOC; [CI](ci.en.md); the black-box framework `tests/e2e/` (#221).

Out of scope: `new`, `check`, tests and the `dev` loop of template projects (S2 #186); the minimum-version matrix of the CI test jobs (#204); `cargo xtask dev` (#260); distributing rutis-dsh (#89); cross-version meetings (#209).

## 1. Conclusions

| # | Decision | Section |
| --- | --- | --- |
| D1 | Install smoke tests are a set of `tests/e2e` scenarios run in an "installed" mode: the host and runtimes come from the installed packages; the repository's runtimes are not injected | 3 |
| D2 | Install straight from the built files: npm installs tgz files, PyPI uses `--no-index --find-links`, binaries are unpacked from the archive; no local registry | 3 |
| D3 | Each cell: install → check version and location → run a minimal project → stop → residue checks | 4 |
| D4 | The build steps move out of release.yml into a reusable workflow shared by main and releases | 6 |
| D5 | When: 3 platforms on main; before a release all 5 platforms must pass; after a release, each channel once more on Linux with the registry packages, plus crates.io and the Go module | 7 |
| D6 | Minimum versions go in the Linux cells: Node 22, Python 3.10 (3.12 until #196 lands), websockets 15.0 | 5 |
| D7 | Documentation examples: only runnable examples get a marker and run on main with the installed packages; unmarked code blocks are not checked | 8 |
| D8 | E7 may no longer be skipped | 9 |

## 2. Current state

| Existing | Where | Does | Missing |
| --- | --- | --- | --- |
| `release-dry-run` | `.github/workflows/ci.yml` | `cargo package` for each crate; `npm pack --dry-run`; `uv build`; `maturin build` | No install, no run; Linux only |
| `release-windows` | `ci.yml` | Builds the Windows binary and runs `help`; zips it; builds the wheel | No install |
| `release-wheel-aarch64` | `ci.yml` | Cross-compiles the aarch64 wheel | No install |
| Platform package tests | `node/rutis-host/test/platform.test.mjs` | Name, `os`, `cpu`, version of the platform packages | — |
| Version consistency | `scripts/train.mjs` | Package versions agree in the source | — |
| Pre-release smoke | `docs/release.md` "Smoke" | A manual "from scratch with the released packages" walk-through | Manual, unrecorded |
| Black-box framework | `tests/e2e/` (#221) | `Scenario`, `Host`, probes, residue checks | Always injects the repository's runtimes (`tests/e2e/src/lib.rs` sets `RUTIS_NODE_RUNTIME` / `RUTIS_PYTHON_PATH`); the host comes from `cargo build` |
| E7 | `tools/test-sdk-bundle.sh` | `pack-plugin` refuses when the toolchain pin differs from the bundle | Prints "skipped" and goes on when no second toolchain is installed |

Problems found while reading the code that bear on this design:

1. websockets lower bound: the declaration is `websockets>=15` (`python/rutis/pyproject.toml`), CI installs `websockets>=13`, which resolves to the latest; 15.0 has never run. #204 fixes the CI test jobs; this design's Linux PyPI cell installs `==15.0`.
2. `src/fake_llm.py` in the Python guide §4 has no content: #259.
3. "The PyPI distribution brings its own Python runtime" needs checking: the host looks for an interpreter in the order `runtimes.py.python` → `$VIRTUAL_ENV` → `<project>/.venv` → `python3` (`crates/rutis-host/src/host.rs`). Whether `uvx rutis-host run` finds the bundled `rutis` is answered by S9's uvx step; if not, fix the code or the documentation.
4. The documentation runs `rutis-host run` in the project directory, which depends on #226; until #226 lands that step is marked as a known failure pointing at #226 (Q7.3).
5. The post-release check cannot be triggered by the `release` event: release.yml creates the Release with `GITHUB_TOKEN`, which does not trigger other workflows. It goes in a final job of release.yml.

## 3. The "installed" mode and install sources

`Scenario::installed(name, channel)` (`tests/e2e/src/install.rs`):

- The scenario directory is a new temporary directory; a new `HOME` (on Windows also `USERPROFILE`, `APPDATA`, `LOCALAPPDATA`), with `npm_config_cache` and `UV_CACHE_DIR` inside the scenario directory.
- `RUTIS_NODE_RUNTIME` and `RUTIS_PYTHON_PATH` are not set; inherited `RUTIS_*`, `NODE_PATH`, `PYTHONPATH`, `VIRTUAL_ENV` are removed.
- `~/.cargo/bin` and the repository's `target/` are removed from `PATH`.
- The host command comes from the channel: `npx rutis-host`, `uv run rutis-host`, `.venv/bin/rutis-host`, or `rutis-host` in the unpacked directory.

| Channel | How this build is installed | How we know it is what runs |
| --- | --- | --- |
| npm | `npm install --omit=optional <rutis-host tgz> <this platform's rutis-host-<platform> tgz> <rutis-runtime tgz>`, the three tgz files from the build artifacts | `rutis-host --version` equals the train version; the resolved binary path is under the scenario's `node_modules` |
| PyPI | `uv venv`, `uv pip install --no-index --find-links <wheel dir> rutis-host==<version>`; on Linux also once with `python -m venv` + pip | Same; `rutis.__file__` is inside the venv |
| Binary | Unpack the built archive (`.zip` on Windows); for Node rows `@arcships/rutis-runtime` is installed into the project from its tgz, for Python rows `rutis` into a venv from its wheel | `--version`; the binary path is under the unpacked directory |
| crates.io, Go module | After a release only (7). Before a release the existing `cargo package` covers them | `--version` / `go list -m` |

When npm installs from tgz files, the other platforms' packages in `rutis-host`'s `optionalDependencies` do not exist on the registry yet: use `--omit=optional` and pass this platform's tgz explicitly. The implementation first checks that npm satisfies `rutis-host`'s dependency on `@arcships/rutis-runtime` with the tgz passed in rather than fetching from the registry; only if that fails is a local registry considered.

## 4. Steps of each cell

| Step | What | Assertions |
| --- | --- | --- |
| 1 Install | As in 3 | Exit code 0 |
| 2 Identity | `<host command> --version`; the actual path of the host binary | The train version; the path is under the scenario directory |
| 3 Minimal project | A fixed project built into the scenario: a `greeter` row and a probe row. npm: a TS row; PyPI: a `py:` row; binary: TS and Python rows, plus a Go row on Linux | After `run` (in the project directory, no arguments, as documented) the probe gets a result from `greeter.hello("Ada")`; in the binary cell TS calls Python and gets a result |
| 4 Stop | SIGTERM to the main process only: the Node launcher for npm, the `uv` process for uv, `rutis-host` itself for the binary | Exit code 0; probe `stopped`; the npm launcher and the `uv` process have exited too; residue checks pass (`tests/e2e/src/residue.rs`) |

A few steps only on some channels:

| Channel | Step | Assertions |
| --- | --- | --- |
| npm | Without installing `@arcships/rutis-runtime`, run a Node row | The row runs: the runtime comes from `@arcships/rutis-host`'s dependency |
| npm | Without this platform's tgz | Exit code 1, output `no binary for <platform>` |
| npm (Linux) | Install the `@arcships/rutis-bun` tgz, add a `bun:` row, Bun 1.4.0 | The probe's call gets a result |
| PyPI | `uvx rutis-host@<version> --version` (`--find-links`); `uvx … run` in a `py:` project without `.venv` | The first prints the train version; the second: see 2-3 |
| PyPI (Linux) | `websockets==15.0`; `python -m rutis listen:ws://127.0.0.1:<port>/rutis …`, the host connects with a node row | The probe's call gets a result |
| Binary | Run a Node row without `@arcships/rutis-runtime` in the project | Exit code 1, output `npm install @arcships/rutis-runtime` |

On Windows step 4 uses kill for now, and the process item of the residue check reports skipped per #232 (the framework already does this).

## 5. Platforms and versions

| Platform | main | Before release | Versions |
| --- | --- | --- | --- |
| linux-x64 (`ubuntu-24.04`) | Yes | Yes | Minimum: Node 22, Python 3.10 (3.12 until #196 lands), websockets 15.0, Bun 1.4.0, Go 1.24 |
| darwin-arm64 (`macos-15`) | Yes | Yes | Latest: Node 26, latest Python |
| win32-x64 (`windows-2025`) | Yes | Yes | Node 24, Python 3.12 |
| linux-arm64 | — | Yes | As linux-x64 |
| darwin-x64 | — | Yes | As darwin-arm64 |

The three channels run one after another on the same machine, each in a new scenario directory: one job per platform.

Not included: musl Linux (Alpine) × PyPI — there is no musllinux wheel; the documentation states that the PyPI distribution supports glibc Linux only. pnpm, yarn, global installs, upgrades, proxies and offline installs — the documentation does not describe them.

#195's install acceptance comes from the linux-x64 npm cell; #196's from the linux-x64 PyPI cell once #196 lands.

## 6. Where the artifacts come from

```text
package.yml (reusable, shared by release.yml and main)
  ├─ build-<target>: the existing binaries / wheels steps of release.yml → archives, wheels, platform package directories
  └─ pack: npm pack for each package and platform package; uv build python/rutis
         │  upload-artifact
         ▼
install.yml (reusable; inputs: source local | registry, version)
  └─ install-<platform>: checkout (to compile the scenarios), download the artifacts,
       RUTIS_E2E_ARTIFACTS=<dir> cargo test -p rutis-e2e --test install -- --ignored
       upload the scenario directory on failure
```

- One copy of the build steps: release.yml's `binaries` and `wheels` move into `package.yml`, which release.yml calls. On main, `package.yml` replaces ci.yml's `release-windows` and `release-wheel-aarch64` (they build without verifying). `release-dry-run` stays on PRs under the `packaging` switch.
- The scenarios live in `tests/e2e/tests/install.rs`, marked `#[ignore = "needs this build's packages: run by install.yml with RUTIS_E2E_ARTIFACTS"]`, so `cargo test --workspace` skips them (Q7.8).
- The scenarios run in a temporary directory outside the repository; the installed mode injects none of the repository's runtimes, and the binary path assertion makes sure the repository's host is not the one running.

## 7. When it runs

| When | What | Time |
| --- | --- | --- |
| PR | No new jobs | Unchanged |
| main | `package` (3 platforms) + `install` (3 platforms) + documentation examples (Linux) | About 20 minutes, within main's 60-minute budget |
| Before release | release.yml: `verify` → `package` (5 targets) → `install` (source local, 5 platforms) → each publish job | About 10 minutes before publishing; all 5 platforms must pass |
| After release | Last job of release.yml: on Linux, the npm, PyPI and binary channels with the registry packages, plus `cargo install rutis-host@<version>` and `go list -m`; first polls the registries until the version is available (every 15 s, at most 15 minutes, as a hang guard); on failure opens an issue with the version in the title | About 10 minutes |
| Manual | `install.yml` has `workflow_dispatch` with version and source inputs | — |

When the post-release check fails, withdrawing or marking (`npm deprecate`, PyPI yank) is the maintainer's decision; the steps go into `docs/release.md` (Q13.2).

## 8. Documentation examples

Only examples that can be followed as written get a marker (not shown on GitHub):

| Marker | Meaning |
| --- | --- |
| `<!-- example: <name> file=<path> -->` | Write the following code block to `<path>` of example `<name>` |
| `<!-- example: <name> run -->` | Run each line of the following shell block in order |
| `<!-- example: <name> run until="<text>" -->` | A long-running command (`dev`, `run`): wait for `<text>` in the output, send SIGINT, assert exit code 0 |

`tools/doc-examples.mjs --out <dir>` extracts the example projects and steps; `tests/e2e/tests/doc_examples.rs` runs them with the installed packages in main's Linux install job. Unmarked code blocks are not checked.

First markers: one complete example each in the `rutis-host.md` quick start, `typescript-plugin.md` and `python-plugin.md` (after #259), in both languages. New runnable examples get a marker as they are written.

## 9. E7

`pack-plugin` compares the `rust-toolchain.toml` channel of the plugin workspace and of the bundle before compiling; it is a plain string comparison and needs no second toolchain. The change: the test writes a non-existent pin `0.0.0-e7` into the workspace, sets `RUSTUP_AUTO_INSTALL=0`, asserts the full refusal message (with both pins) and that no rustup install happened; the "skipped" branch is removed. Nothing is downloaded and the result is deterministic.

## 10. Risks covered

| Risk / clause | How | When |
| --- | --- | --- |
| **B1** (P0) | Three channels × 3 platforms with this build's artifacts; the other 2 platforms before release; registry packages after release | main, before release, after release |
| Q6.15.1 | Install → minimal project → stop, in a new directory with a new user directory | Same |
| Q6.15.3, Q13.2 | Post-release check, opens an issue on failure | After release |
| Q13.1.6 | All 5 platforms before release | Before release |
| B6, partly | Error messages for a missing platform package and a missing Node runtime package | main |
| B2, partly | The npm launcher and `uv run` clean up and exit on SIGTERM | main (Unix) |
| MX, partly | Node 22, Python 3.10, websockets 15.0, Bun 1.4.0 in installed scenarios | main |
| Q2.13 | Marked documentation examples run | main |
| I5, partly | E7 is no longer skipped | PRs touching dylib |

Not covered: the macOS quarantine attribute of browser downloads; musl Linux; pnpm, yarn, global installs; upgrades and side-by-side versions (#209); corporate proxies and offline installs; Windows process residue and Ctrl-C (#232); correctness beyond the minimal project.

## 11. Files added and changed

| File | Content |
| --- | --- |
| `tests/e2e/src/install.rs` | Channels, the installed-mode environment, identity checks, one-shot commands |
| `tests/e2e/tests/install.rs`, `tests/e2e/fixtures/install/` | Scenarios and the minimal project |
| `tests/e2e/tests/doc_examples.rs`, `tools/doc-examples.mjs` | Documentation examples |
| `docs/guide/*.md` (both languages) | The first markers |
| `tools/test-sdk-bundle.sh` | E7 without the skip |
| `.github/workflows/package.yml`, `install.yml`, `release.yml` | Reusable build and install; before and after release |
| `docs/release.md` (both languages) | What to do when the pre- or post-release check fails |

The `ci.yml` changes — removing `release-windows` and `release-wheel-aarch64`, calling `package.yml` and `install.yml` on main — are listed in the PR description and made by #204 (#256).

## 12. Phases (all in this PR, as separate commits)

1. The installed mode; scenarios for the three channels; `package.yml`, `install.yml`; 3 platforms on main; E7.
2. Before and after release; `docs/release.md`.
3. Documentation examples and the first markers.

## 13. Acceptance (all automatable)

1. Every platform of `install` passes on main; the log shows the language versions, `rutis-host <train version>` and the path of the host binary.
2. The checks catch problems: removing the `rutis` wheel fails the PyPI cell; leaving out this platform's tgz gives the expected error in the "missing platform package" step of the npm cell.
3. Every cell's residue report is empty (on Windows the process item reports skipped per #232).
4. In release.yml every publish job `needs` the pre-release install jobs.
5. The post-release check runs automatically; `workflow_dispatch` with a non-existent version opens an issue.
6. Marked documentation examples pass on main.
7. E7 in `tools/test-sdk-bundle.sh` has no skip branch.
8. The critical path of ordinary PRs is unchanged; `package` + `install` on main ≤ 25 minutes.

## 14. Maintainer decisions (2026-10-11)

- Node in the Linux minimum cell: the latest 22 patch (`setup-node` with `22`).
- PyPI on musl Linux: the documentation states that the PyPI distribution supports glibc Linux; no musllinux wheels.
- The workflows (`package.yml`, `install.yml`, `release.yml`) change in this PR, the `ci.yml` changes in #256; a failed post-release check opens an issue automatically and withdrawing is a human decision; the `xtask dev` SDK_ID test moved to #260.
