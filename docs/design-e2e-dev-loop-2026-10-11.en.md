# S2 end to end: the plugin author's development loop (design)

[中文](design-e2e-dev-loop-2026-10-11.md)

Status: design, under review. Date: 2026-10-11. Base: `main` `0941ef4`.
Related: [#186](https://github.com/arcships/rutis/issues/186) (step 1 of [#183](https://github.com/arcships/rutis/issues/183)). Based on: [quality standard](quality-standard.en.md) ("the standard") Q6.6, Q7; [quality status](quality-status.en.md) ("the status") scenario A, §9.1; the E2E framework [#221](https://github.com/arcships/rutis/pull/221) (`tests/e2e/`); [rutis-host guide](guide/rutis-host.en.md); [CI](ci.en.md).

Out of scope: installing from npm / PyPI (S9 [#193](https://github.com/arcships/rutis/issues/193)); signal handling of `run` (covered by `crates/rutis-host/tests/signals.rs`); `check` output for hand-written configurations (covered by `crates/rutis-host/tests/check.rs` and #205); residue after a kill (covered by `tests/e2e/tests/cross_language.rs`); repeated reloads and long runs (S8 #192); changing `rutis-host` behaviour.

## 1. Conclusions

| # | Conclusion | Section |
| --- | --- | --- |
| C1 | One test file `tests/e2e/tests/dev_loop.rs`, one test per language: `node`, `python`, `bun`, `go`; each runs the same steps: `new → template tests → check → dev → change to v2 → break it → fix it → Ctrl-C` | 3 |
| C2 | v2, v3 and the broken version are written whole by the scenario; v1 is the template `new` generates; v2 and v3 print a line on apply and on cleanup, to assert "the old instance was unloaded, exactly once" | 3.1 |
| C3 | Step 2 uses the repository's packages (links to `node/rutis`, `bun/rutis-bun`, tsx; `RUTIS_PYTHON_PATH`; Go `replace`), no registry; installing from registries belongs to #193 | 3.3 |
| C4 | Only exit codes and the few lines rutis itself writes are asserted, with the expectations in the test code (as in `check.rs`); third-party tool errors are only checked to name the file and line | 5 |
| C5 | Every synchronization point is an output line or a process exit; files are changed by writing a dot file in the same directory and renaming it, with the mtime guaranteed to change; no sleep | 6 |
| C6 | This PR covers Linux and macOS; Windows comes in a later PR after #232 | 7 |
| C7 | CI: a new parallel Linux `e2e` job, macOS inside `network-macos`, added by #204 (#256) | 8 |

## 2. Current state (code checked)

| Existing test | Tests | Does not test |
| --- | --- | --- |
| `crates/rutis-host/src/new.rs` `projects_are_created_with_their_names_filled_in` | Generated file contents | Whether the generated project tests and runs |
| `crates/rutis-host/tests/project.rs` | `project::dev_config` for various projects (library calls) | The binary, reloading |
| `crates/rutis-host/tests/check.rs` | `check` stdout / stderr / exit code (hand-written configurations, including a missing package) | Template projects |
| `crates/rutis-host/tests/signals.rs` (Unix) | `run` / `dev` exit codes and cleanup counts under signals | Reloading after file changes |
| `tests/e2e/tests/cross_language.rs` | Cross-language calls under `run`; residue after a kill | `new` / `dev` / `check` |

**What `dev` does** (`crates/rutis-host/src/main.rs` `dev`, `project.rs`)

- On start it takes a file snapshot (`project::sources`), then prints `rutis-host dev: running <id>; changes reload it (Ctrl-C ends)`. Changes after that line are always seen.
- Every 400 ms it takes a new snapshot and compares `(path, mtime)`. It skips `node_modules`, `.git`, `.venv`, `venv`, `target`, `dist`, `build`, `__pycache__`, `.pytest_cache` and names starting with `.` (except `.env`).
- On a change (non-Go projects): `host.invalidate()`, then `loader.reload` for every non-peer row, printing `<id>: reloaded` or `<id>: cannot reload: <error>` per row. Rows added in `rutis.dev.json` (including the scenario's probe) are reloaded too. `Loader::reload` is all or nothing: when the new module fails to resolve, the old one keeps running.
- Go projects: `go build` again into `.rutis/go/<name>-<n>`; on success it swaps the binary, restarts the runtime and prints `<runtime>: rebuilt and restarted`; on failure it prints `<name>: the build failed; the last build keeps running` and the compiler output. Other rows of a Go project are not reloaded.
- Status lines (`status::follow`, compared every 200 ms) print only on change and may skip intermediate states; the scenario waits only for stable states.

**`check`**: in a plugin project without `rutis.json` it checks the project itself (`dev_config`). It does not check whether a service in `inject` is provided by some row; see #258.

## 3. Scenario

### 3.1 Common setup

- Scenario directory `<root>/<scenario>-<pid>-<n>/project/`; `rutis-host new demo --lang <language>` runs there and creates `project/demo/`; later commands run in `demo/`.
- Four versions of the plugin:

| Version | Source | `greeter.hello("Ada")` | Prints |
| --- | --- | --- | --- |
| v1 | The template from `new`, unchanged | `Hello, Ada!` | Nothing |
| v2 | Written whole by the scenario | `Hello, Ada! (v2)` | `demo v2 applied <interpreter>` on apply, `demo v2 cleanup` on cleanup |
| Broken | v2 with a syntax error on line 5 (Go: a compile error) | — | — |
| v3 | Written whole by the scenario | `Hello, Ada! (v3)` | `demo v3 applied …`, `demo v3 cleanup` |

  Written whole rather than by replacing strings in the template: rewording the template does not break the scenario, and the template itself is tested by the v1 steps.

- The probe (`tests/e2e/probes/`) is an extra row in `rutis.dev.json`, written after step 3, so step 3 checks the template project as generated.

| Project | Probe | Location | Runtimes added to `rutis.dev.json` |
| --- | --- | --- | --- |
| node | TS | `demo/probe.ts` | None |
| python | Python | `demo/src/probe.py` (the Python runtime's project is `src/`) | None |
| bun | Python | `demo/dev/probe.py` | `"py": { "project": "dev" }` |
| go | Python | `demo/dev/probe.py` | `"py": { "project": "dev" }` |

### 3.2 Steps

"Wait" always means waiting for an output line or a process exit (`Host::expect` / `wait_for` / `wait_exit`), with only a hang guard.

| # | Action | Assertions |
| --- | --- | --- |
| 1 | `rutis-host new demo --lang <language>` | Exit code 0; stdout contains `created demo/`; the generated file list equals the list in the test |
| 2 | The template's own tests (3.3) | Exit code 0 |
| 3 | `rutis-host check` (no arguments) | Exit code 0; stdout lists row `demo` and its `provides`; stderr empty |
| 4 | Write the probe and `rutis.dev.json`, run `rutis-host dev`; wait for `rutis-host dev: running demo; …` and the probe's `started` | `hello("Ada")` = `Hello, Ada!` |
| 5 | Change to v2; wait for `demo: reloaded`, probe `started`, `demo v2 applied` | Result = `… (v2)`; Python: the interpreter is under `demo/.venv` (A8) |
| 6 | Break it; wait for a line starting `demo: cannot reload: ` and the probe's `started` again | The error names the file and line 5 (A7); the result is still `… (v2)`; `demo v2 cleanup` appears 0 times (the old instance was not unloaded, A4) |
| 7 | Fix it (v3); wait for `demo: reloaded`, `demo v3 applied`, probe `started` | Result = `… (v3)`; `demo v2 cleanup` exactly once (A3) |
| 8 | Ctrl-C (`killpg(SIGINT)`); `wait_exit` | Exit code 0; `demo v3 cleanup` exactly once; probe `stopped` (B2) |
| — | `Scenario::finish()` | Residue checks pass |

### 3.3 Differences between languages

| | node | python | bun | go |
| --- | --- | --- | --- | --- |
| Step 2 setup | Link `node_modules/@arcships/rutis` → `node/rutis`, `node_modules/tsx` → `node/rutis-runtime/node_modules/tsx` | `python -m venv --without-pip .venv`; `rutis` from `RUTIS_PYTHON_PATH` | Link `@arcships/rutis`, `@arcships/rutis-bun` → `bun/rutis-bun` | Append `replace github.com/arcships/rutis/go/rutis => <repo>/go/rutis` to `go.mod` |
| Step 2 command | `npm test` | The `.venv` python `-m unittest discover -s tests`, `PYTHONPATH=src` + `python/rutis` | `bun test` | `go test ./...`, `GOTOOLCHAIN=local`, `GOPROXY=off` |
| Output in steps 5–7 | `demo: reloaded` / `demo: cannot reload: …` | Same | Same | `go-demo: rebuilt and restarted` / `demo: the build failed; the last build keeps running` |
| Extra assertion in step 7 | — | — | — | The old runtime process has exited; `.rutis/go/` holds only the newest binary |
| Probe in steps 5–7 | Restarted with every reload | Same | Same | Not reloaded; stops and starts with the Go runtime's restart |
| Not done | — | `uv sync` (needs a registry, #193) | `bun run check`, `bunx --bun` (need a registry, #193) | — |

The Bun runtime also compares the entry file's mtime and size, so `replace` guaranteeing an mtime change matters for Bun as well.

## 4. Harness additions

All in `tests/e2e/src/`, with no dependency on any rutis crate (Q7.6).

| Addition | Purpose |
| --- | --- |
| `Scenario::host_in(dir, args)` | Start the host in a project subdirectory; same environment as `host_with` |
| `Scenario::program(dir, program, args, env) -> Host` | Run npm / python / bun / go with the same temp directories, output capture, process group and residue registration |
| `Scenario::replace(relative, contents)` | Write `.<name>.part` in the same directory, then `rename`; if the new mtime equals the old one, `set_modified(old + 1 s)` |
| `Scenario::link(project, package, target)` | Link a repository package into a project (the general form of `link_node_sdk`) |
| `Host::count(text)` | Number of lines so far containing `text`; "exactly once" is counted after the host exits |
| `Host::ctrl_c()` | `killpg(SIGINT)` (what a terminal does) |

## 5. Which output is asserted

Only output rutis itself writes and documents, with the expectations in the test code: `created demo/` and the generated file list from `new`; the rows from `check`; `rutis-host dev: running demo; …`; `demo: reloaded`, the `demo: cannot reload: ` prefix and the two Go lines; the exit code of every step.

Not asserted: error text from tsx, Bun, Python and the Go compiler (only that it names the file and line); the order of status lines; output of tools such as `npm test`. Paths are compared after replacing the scenario directory with `<dir>`.

## 6. Determinism (Q7.1)

| What could vary | What we do |
| --- | --- |
| A file changes before `dev` takes its first snapshot | Change files only after `rutis-host dev: running …` |
| Two changes get the same mtime | `replace` checks and moves the mtime 1 s forward |
| A half-written file is seen by the poll | Dot file + `rename`; the replacement is atomic |
| Two changes fall in one poll | After each change, wait for that reload's result lines and the probe's `started` before the next |
| "The old instance was not unloaded" | Rewritten as "`cannot reload` and the probe's `started` have happened, and `cleanup` still has not" (Q7.1.1) |
| "Exactly once" | Counted after the host exits and its output closes |
| Timeouts | Only the harness hang guard (30 s by default, `RUTIS_E2E_TIMEOUT`); Go's step 2 `go test` warms the build cache first |

No sleep.

## 7. Platforms

Linux and macOS: all four languages, Ctrl-C via `killpg(SIGINT)`. Windows comes in a separate PR after #232 (Job Object residue checks): without the process check, B2 cannot be verified on Windows; how to send Ctrl-C is decided then. Bun does not yet declare Windows support (Bun design §10), so it is skipped there with the reason written (Q7.8).

## 8. CI and time

Estimates (measured on CI during implementation and written into the PR): about 10 s per language, Go about 15–20 s including builds; the four tests run in parallel, about 20–30 s on Linux.

CI changes needed (added by #204's PR #256; this PR does not touch `ci.yml`):

| Change | Time |
| --- | --- |
| New job `e2e` (Linux, `code` switch): the same minimum Node, Bun, Python, Go as `rust`; `cargo test -p rutis-e2e`; upload `RUTIS_E2E_DIR` on failure; add to `ci-ok` | Parallel with `rust`, about 3–5 minutes |
| Add `--exclude rutis-e2e` to the `rust` job's `cargo test --workspace` | One check runs in one place (Q12.7) |
| `network-macos`: add `cargo test -p rutis-e2e` | About +1 minute, no new macOS job |

## 9. Risks covered

| Risk | Pri | How |
| --- | --- | --- |
| A2 template project does not test or run | P1 | Steps 1–4 (with the repository's packages; registry install is #193) |
| A3 old code still runs after a reload / old instance not unloaded | P0 | Steps 5, 7: the result changes, the old instance is cleaned up exactly once; Go: the old process exits |
| A4 `dev` crashes or the old version stops serving after a broken change | P1 | Step 6: the old version keeps serving and is not unloaded; `dev` keeps running |
| A6 `check` misjudges | P1 | Step 3 (a template project passes); a missing package is covered by `check.rs`; an unprovided `inject`: #258 |
| A7 errors do not show file and line | P1 | Step 6 |
| A8 Python venv path | P1 | Step 5's interpreter assertion; the Windows part after #232 |
| B2 no cleanup or wrong exit code after Ctrl-C | P0 | Step 8 (under `dev`); under `run` covered by `signals.rs` |

Explicitly not covered: A1 (#206); A5 repeated reloads (#192); B3, B10 (`cross_language.rs`, #192); B1 and registry installs (#193); B4 (`run` does not watch its configuration).

## 10. Phases and acceptance

| Phase | Content |
| --- | --- |
| E1 | Harness additions; the node and python tests in `dev_loop.rs` |
| E2 | The bun and go tests |
| Later PR | Windows (after #232) |

Acceptance (all automatable):

1. `cargo test -p rutis-e2e --test dev_loop` passes on Linux and macOS; all four tests run, each ends with `finish()` and leaves no residue.
2. Each step's exit code matches 3.2.
3. It also passes under `RUTIS_LOCAL_HANDOVER=loopback` (Unix).
4. Five consecutive local runs pass (recorded in the PR description).
5. No `sleep` in the scenario code.

The implementation PR also records one manual reverse check: temporarily make `dev` unload the old instance on a failed reload, or skip cleanup on Ctrl-C, and confirm the scenario fails.

## 11. Maintainer decisions (2026-10-11)

- Step 2 links the repository's packages; no registry installs (those belong to #193).
- "Calls do not fail while broken" is tested as "after the failed reload the old version keeps serving and is not unloaded"; in Go projects the probe is not reloaded, which also covers calls during the rebuild.
- CI changes are added by #256; the `inject` check of `check` is #258.
