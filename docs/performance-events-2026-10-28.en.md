# Event dispatch inside a runtime: benchmark before local dispatch is removed

Date: 2026-10-28. Code: main `212ac7ce` (after rutis 0.18, before 0.19); the language SDKs still dispatch events in their own process ("local dispatch").

[Platform cleanup](design-platform-cleanup-2026-10-28.en.md) §7.5 removes local dispatch from every language: events always go to the host. This document records the numbers before that and through the host, judged by the conditions of §7.5:

- the median of `emit` through the host is accepted at the measured values: Node 71 microseconds, Python 139 microseconds (see "After"). The path through the host can be optimized later;
- one run of the dsh baseline (`node/baseline/`) takes at most 1.1 times the number below.

## What is measured

- **Events**: one host, one plugin row, with both the listeners and the emitter in that plugin, so in one runtime. One for Node (`tools/bench/events/bench.mjs`) and one for Python (`tools/bench/events/bench_events.py`). Each mode warms up 1 000 times, then runs 10 000 times; the median and p99 are reported in microseconds:
  - `emit`: from calling `ctx.emit` until the listener has run (the listener settles a pending Promise / future the emitter awaits);
  - `serial`: `await ctx.serial(...)`, the listener returns its argument;
  - `bail`: the synchronous `ctx.bail(...)`, a synchronous listener returns its argument.

  The plugins use only the public plugin API (`ctx.on`, `ctx.emit`, `ctx.serial`, `ctx.bail`) and run under `rutis-host run` from a `rutis.json`; they do not touch the SDK's internal dispatch code, so they run unchanged after local dispatch is removed.
- **dsh baseline**: the total time of one run of the `rutis-bridge` test `dsh_baseline` (`crates/rutis-bridge/tests/dsh_baseline.rs`): it runs the scenarios of `node/baseline` in native Cordis, then mounts the same published Cordis plugins through rutis, calls them one by one and compares. Five runs, median. The native Cordis half (`node native.mjs`) is recorded on its own for reference.

## How to run

```sh
npm --prefix node/rutis-runtime ci
npm --prefix node/baseline ci            # only dsh needs it
tools/bench/events/run.sh                # node, python and dsh; or name some of them
```

- `RUTIS_HOST`: use an existing `rutis-host`; without it, cargo builds one with the profile `RUTIS_BENCH_PROFILE` (default `release`).
- `RUTIS_PYTHON`: the Python interpreter; without it the host finds one by its default rules. The script sets `RUTIS_PYTHON_PATH` to this repository's `python/rutis`, so the interpreter needs no `rutis` installed.
- `RUTIS_BENCH_ITERATIONS`, `RUTIS_BENCH_WARMUP`: counts, default 10 000 and 1 000.

This run used `RUTIS_BENCH_PROFILE=dev tools/bench/events/run.sh` (a debug build of the host). Before the removal, event dispatch stays inside the runtime process and never reaches the host, so the host's build does not affect these numbers. After the removal the host is on the path; that run uses the default `release`. The dsh baseline test is built by `cargo test` (dev profile), the same before and after.

## Machine

macOS 26.6.2, Apple M4 Max (16 cores), 128 GB memory; rustc 1.98.1, Node v24.19.0, Python 3.12.2. No CPU pinning, no fixed frequency, other builds running on the machine at the same time. The numbers are samples from this machine, not CI thresholds.

## Before (local dispatch)

Two runs, close to each other; the table is the second. Unit: microseconds.

| Runtime | Mode | Median | p99 |
| --- | --- | ---: | ---: |
| Node | `emit` | 3.4 | 6.5 |
| Node | `serial` | 2.8 | 4.2 |
| Node | `bail` | 2.8 | 3.9 |
| Python | `emit` | 2.1 | 2.7 |
| Python | `serial` | 1.8 | 2.3 |
| Python | `bail` | 2.5 | 3.0 |

First run: Node `emit` 3.8 / 15.0, `serial` 2.8 / 5.7, `bail` 2.8 / 5.8; Python `emit` 2.1 / 2.3, `serial` 1.8 / 2.1, `bail` 2.5 / 2.7. Node's p99 differs a lot between the runs, affected by other work on the machine.

dsh baseline, unit: milliseconds, median of five runs:

| Item | Median | Five runs |
| --- | ---: | --- |
| One run of the `dsh_baseline` test (native Cordis + mounted through rutis) | 1133 | 1235, 1144, 1133, 1118, 1124 |
| Of which the native Cordis half (`node native.mjs`) | 176 | 341, 176, 227, 169, 118 |

The first run's `dsh_baseline` median was 1194 milliseconds (its first round 4045 milliseconds, a cold start). The limit after the removal is taken from 1133 milliseconds: 1.1 times is 1246 milliseconds.

## After (through the host)

Code: main `b502303d`, with local dispatch switched off temporarily so that every emission goes through the host (a `release` build of `rutis-host`); the same command, two runs, close to each other. Unit: microseconds.

| Runtime | Mode | Median before | Median through the host | p99 through the host |
| --- | --- | ---: | ---: | ---: |
| Node | `emit` | 3.4 | 71 | 125 |
| Node | `serial` | 2.8 | 131 | 245 |
| Node | `bail` | 2.8 | 85 | 178 |
| Python | `emit` | 2.1 | 139 | 240 |
| Python | `serial` | 1.8 | 307 | 600 |
| Python | `bail` | 2.5 | 105 | 195 |

Judgement: Node's `emit` is within the original 100 microseconds; Python's `emit` is over the original limit. Having seen these numbers, the user decided to remove local dispatch anyway, and the limit becomes the measured values above. The path through the host can be optimized later; optimizing it does not bring local dispatch back.

Removing local dispatch only removes the local check; the path through the host is unchanged, so these are the numbers after the removal. The dsh baseline is filled in by the PR that removes local dispatch, with the same command.
