# CI redesign (2026-10-13)

[中文](design-ci-redesign-2026-10-13.md) · [CI](ci.en.md) · [Quality standard](quality-standard.en.md) §12

Status: agreed (2026-10-13). All five questions in section 7 were decided as recommended.

## 1. The problem

Triggers today (`main` `ea487b2`):

| Workflow | Pull request | Merge to main | Release tag | Scheduled |
| --- | --- | --- | --- | --- |
| `ci.yml` (24 jobs) | Only the jobs the change affects | **Every job** | — | — |
| `install.yml` (packages, then install and run on 5 platforms) | When packaging paths change | **Every merge** | Called by `release.yml` | — |
| `dylib-windows.yml` (GitHub's Windows, 25–40 minutes) | When dylib paths change | **Every merge** | **Every tag** | — |
| `release.yml` | — | — | Packages, release gate, publish, post-release check | — |
| `stress.yml`, `deps.yml`, `ci-stats.yml` | — | — | — | Nightly / weekly |

What goes wrong:

1. **A merge reruns CI that the pull request already ran, in full**, and adds the install checks and the Windows dylib checks on top. One merge queues more than thirty jobs on main. They compete for the same machines as open pull requests. When several pull requests merge in a row, main keeps a run queued all the time (#348 only made sure the running one is not cancelled and the waiting one is replaced).
2. **Some jobs still wait for GitHub's machines**: `dylib-windows` (`windows-2025`), the macOS entries of `runtimes-go` / `runtimes-bun` (`macos-15`), and most of `install.yml`'s packaging and install cells. On the self-hosted runners, the longest `ci.yml` job takes 3–9 minutes (`runtimes-windows`), so the wait in the queue is longer than the run.
3. **The release pipeline** (seen during the 0.10.0 release):
   - pushing a `v*` tag runs `dylib-windows` again, although main already ran it on the tagged commit;
   - the `go` job tags `go/rutis/vX.Y.Z` without waiting for the release gate;
   - the post-release check waits only for the main package, not for the npm platform packages;
   - before tagging, someone has to check by hand that main's CI and `install.yml` passed (step 2 of release.md).

## 2. Why every merge to main runs everything today

There are two reasons, and both still hold:

- **A pull request runs only the jobs its change affects** (`changes` picks them by path). When the path table misses a dependency, a job that should have run on the pull request did not. Running every job on main catches that (Q12.3.3).
- **A pull request is tested against main as it was when the pull request ran.** Two pull requests can each pass and still break when combined. Only a run after both merge shows that.

Neither needs a check after every single merge. A check once a day, plus one before every release, is enough.

## 3. New triggers

| When | What runs | Where |
| --- | --- | --- |
| Pull request | As now: `ci.yml` runs only the jobs the change affects; `install.yml` runs when packaging paths change; `dylib-windows` runs when dylib paths change | Self-hosted |
| Merge to main | **Nothing** | — |
| Nightly (new `nightly.yml`) | When main has new commits: every `ci.yml` job, `install.yml` (this build, 5 platforms), `dylib-windows`; the existing `stress.yml` moves in | Mostly self-hosted |
| Release tag | `release.yml`: full CI, packages and the release gate, then publish, then the post-release check | See section 5 |
| Manual | Any branch can run every job once (`workflow_dispatch`, `all=true`) | Self-hosted |

Details:

- Remove `push: main` from `ci.yml`, `install.yml` and `dylib-windows.yml`. Give `ci.yml` a `workflow_call` input `all` for `nightly.yml` and `release.yml`. With `all=true`, the `changes` job selects every job, as it does on a push to main today.
- **The nightly run only runs when there are new commits.** It compares main's head with the commit of the last successful nightly run, and skips when they are the same.
- **A failed nightly run opens an issue automatically.** If an issue for that workflow is already open, it adds a comment to it instead (the same approach as `release.yml`'s `report`). The issue names the failed jobs and the range of commits, from the last successful commit to this one.
- Pull request concurrency stays the same: a new push cancels the old run. With no runs on main, the main-specific rule in `concurrency` (#348) can go.

## 4. Machines

| Job | Now | Change to |
| --- | --- | --- |
| `dylib-windows` | GitHub `windows-2025` | Self-hosted Windows (`WINDOWS_RUNNER`), with the same target directory and parallelism rules as `runtimes-windows` |
| macOS entries of `runtimes-go` / `runtimes-bun` | GitHub `macos-15` | Self-hosted macOS (`MACOS_RUNNER`) |
| `install.yml` `darwin-arm64` | Self-hosted | Unchanged |
| Other `install.yml` install cells | GitHub (`ubuntu-24.04`, `ubuntu-24.04-arm`, `macos-15-intel`, `windows-2025`) | **Unchanged**. The install check needs a clean machine, and a self-hosted machine has too much installed to show what is missing. There are also no self-hosted Intel Mac or Linux arm64 machines. |
| `package.yml` packaging | GitHub (Linux needs Docker containers for musl cross-compiles) | Unchanged: the self-hosted Linux runners are containers without Docker |

From now on `install.yml` and packaging run only nightly, at release, and on pull requests that change packaging paths. That is rare enough that waiting for GitHub's machines is not a problem.

## 5. Release pipeline

`release.yml` becomes:

1. **Verify** that the versions match the tag (unchanged).
2. **Full CI**: call `ci.yml` with `all=true` on the self-hosted machines, in parallel with packaging. Nobody has to check main's CI by hand any more. Step 2 of release.md becomes "merge the release pull request".
3. **Packages and release gate** (unchanged): build the files for five platforms, then install and run them on five platforms. The published files must be exactly the files the gate checked, so the pipeline does not reuse files built by main or by the nightly run.
4. **Publish**: crates, npm, PyPI, and the **`go` job**. All of them wait for steps 2 and 3 to pass. `go` no longer tags early.
5. **GitHub Release** (unchanged).
6. **Post-release check**: first wait until every package can be fetched from its registry, **including the five npm platform packages**, then install and run from the registries.
7. Remove `tags: ["v*"]` from `dylib-windows.yml`. Step 2 already runs the Windows runtime tests, and the nightly run covers the Windows dylib checks. Whether `dylib-windows` should also be part of the release gate is question 2 in section 7.

## 6. Changes to the quality standard

| Clause | Now | Change to |
| --- | --- | --- |
| Q12.3 | Pull requests pick their jobs by change; main runs every job | Pull requests pick their jobs by change; every job runs **at least once a day** and **before every release** |
| Q12.4 | Every merge has a complete result; superseded runs are cancelled | The nightly run checks every commit merged since the last one; when it fails, it opens an issue naming the commit range; a new push to a pull request cancels the old run |
| Q12.6 | The critical-path limits apply to pull requests and main | They apply to pull requests and releases |

`docs/ci.md` §3 (triggers), §7 (failures) and §10–11 (self-hosted runners) change with it. So do steps 2–4 of `docs/release.md`.

## 7. The maintainer's decisions (2026-10-13, all as recommended)

1. Is it acceptable that a merge to main runs no checks at all, and that gaps are caught only by the nightly run and before releases?
2. Should `dylib-windows` be part of the release gate? Including it adds 25–40 minutes to a release (possibly less on the self-hosted Windows machine). Without it, only the nightly run covers it. Recommended: include it, because it is the only check of Windows compatibility for dylib plugins.
3. What time should the nightly run start? Recommended: 03:00 Beijing time (19:00 UTC), away from the current `stress.yml`.
4. Who should a failed nightly run's issue be assigned to? Recommended: the maintainer, with a `ci` label.
5. Is it acceptable that `install.yml` keeps using GitHub's clean machines, except `darwin-arm64`?

## 8. Implementation steps

| PR | What | How it is checked |
| --- | --- | --- |
| 1 | `ci.yml` gets `workflow_call` (`all`). New `nightly.yml`: runs only on new commits, opens an issue on failure. Remove `push: main` from the three workflows. Move `stress.yml` in. Update `ci.md`, the quality standard and `quality-status.md`. | Trigger `nightly.yml` by hand once and let every job run. Make one job fail on purpose and check that an issue is opened. |
| 2 | Move `dylib-windows` and the macOS matrix entries to the self-hosted runners. | Run `dylib-windows` in full on the pull request. The runner runs as `NETWORK SERVICE`, so file locks, the Job Object and the exported-symbol check may behave differently. |
| 3 | `release.yml`: full CI joins the release gate, `go` waits for the gate, the post-release check waits for the npm platform packages. Remove the tag trigger from `dylib-windows`. Update `release.md`. | With `workflow_dispatch`, run on a test tag up to just before publishing (add a `dry-run` input to the publish steps). |

Differences from the plan in PR 3: the rehearsal needs no test tag; the `workflow_dispatch` of `release.yml` runs on any branch (`dry-run` on by default; an optional `tag` input checks the version), and `dry-run=false` runs only on a tag. Besides the five npm platform packages, the check after publishing also waits until every package named in the optionalDependencies of `@arcships/rutis-host` can be fetched.
