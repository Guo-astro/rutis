# An instance's resident footprint: what each instance and each row holds (0.20)

Date: 2026-11-02. Code: main `be3e76a1` + #1016. Measured: one instance of an `instanced` group = one group fiber + eight ordinary Rust plugin rows, exactly the `plain` fixture of `crates/rutis-loader/tests/close_memory.rs`.

[中文](performance-instance-footprint-2026-11-02.md)

## How it was measured

- A global counting allocator sums **live allocated bytes** (the requested sizes, not RSS and not the allocator's arenas);
- the composition uses allocation-site sampling: allocations under 512 B are sampled 1/128, those of 512 B and more are all sampled, and the samples are scaled back by their weight; the empty loader's live set is measured first and subtracted;
- the same test (`close_memory`) gives the totals and the close residual; the sampled breakdown answers what those bytes are;
- machine: Linux x86-64, 32 cores, 62 GiB, rustc 1.98.1. Debug and release differ by under 3% in the per-instance total; the breakdown uses debug at N=400, where the stacks are complete, and the totals are cross-checked in debug and release at N=40 to 1000.

The breakdown's sum is about 2 KiB from the test's unsampled total (the 1/128 sampling error and the unlisted tail); rows called estimates below come from the sampling.

## What one instance holds

Plain, debug, N=400, per instance, live allocations:

| Part | Per instance | Share | What it is |
| --- | ---: | ---: | --- |
| tokio tasks and channels | 27.7 KiB | 51% | one drive task per fiber, nine of them (5.6); the intent mpsc block and channel (9.3); watch channels (`Shared<usize>` 4.5, `Shared<Snapshot>` 3.0, `Shared<()>` 0.1); cancellation tree nodes 0.9; one monitor task per row 4.2 (a 344 B future plus the task header) |
| kernel fiber / ctx / effect | 15.8 KiB | 30% | `FiberInner` 6.7, `EffectRecord` 4.0, `CtxInner` 1.4, `FactoryAdapter` 1.0, the cleanup list 0.7, mount effects and the effect index 1.0, load closures and `spawn_fiber_inner` 0.5, `mark_row`/scope 0.5 |
| loader records | 3.1 KiB | 6% | `EntryConfig` per row 0.9, the row-event closure 0.9, `Follower` 0.8, the group's `GroupPlugin` 0.2, `spawn_instance`'s future 0.1, row id strings 0.1 |
| hash-table capacity while running | 7.1 KiB | 13% | the loader's `running` (`Slot` → `Running`) 6.7, `followers` 0.35, the other tables 0.03; this is allocated capacity, not occupied entries |
| the rest | 0.6 KiB | 1% | service registrations, scope chains, instance id formatting, short row and plugin strings |

About 53.9 KiB in the sampling, with the monitor task counted at its final 344 B future; the same test without sampling gives 52.2 KiB per instance (debug, N=400) and 50.9 KiB (release, N=1000). The `watch` group (one more `FiberStatusChanged` listener per row) holds about 8.1 KiB more per instance: the listeners, their lists, and the queue while closing.

**Per row**: an instance has nine fibers (the group and eight rows), about 6.0 KiB per fiber on average; the eight rows' records differ only in the row config inside `EntryConfig`, and the group fiber has a `GroupPlugin` instead. A row id is only a 2–3 B heap string, not a major part.

## What can be saved

1. **The per-row monitor task (changed here).** Each row has an `Inner::monitor` task. Its async block inlined `set_disabled` (a whole loader operation), so its future was 1,872 B. Nine per instance came to about 16.5 KiB, 25% of the footprint before this change. Boxing the rare "a row disposes itself" branch brings the future to 344 B, saving about 13.4 KiB per instance; the box is allocated only when a row really disposes itself. The change is in `Inner::monitor` in `crates/rutis-loader/src/loader/reconcile.rs`; `the_monitor_future_stays_small` measures that future (344 B normally, 1,872 B when inlined) against a 512 B bound.
2. **The `running` table's spare capacity (not changed).** A thousand running instances hold 9,000 entries of about 328 B, and a `HashMap` grows its table in powers of two; at N=400 the table is 2.70 MB against 1.18 MB of entries, about 3.8 KiB per instance spare. There is no "a batch of instances has been created" boundary: `shrink_to_fit` after every insert would be O(N²), and `create_instance` is called one at a time. The shrink on closing in this pull request gives it back; while running it is the price of growth, not a leak.
3. **The per-row records (not changed).** An `EntryConfig`'s config can be evaluated against the copy's own instance through expressions, so it cannot be shared across instances; the `Follower` and the row-event closure are what row events are built on; `FiberInner` and every fiber's watch, mpsc and cancellation nodes are what the kernel's state and cancellation semantics need. No duplicated records, large shareable constants or fixed per-instance capacities turned up.

## Before and after (same test, same build)

`cargo test -p rutis-loader --release --test close_memory -- --nocapture` (release, plain/watch):

| Item | Before this pull request | After | Change |
| --- | ---: | ---: | ---: |
| plain, per instance, N=1000 | 63.3 KiB | 50.9 KiB | −20% |
| watch, per instance, N=1000 | 71.5 KiB | 59.1 KiB | −17% |
| plain, per instance, debug N=400 | 64.6 KiB | 52.2 KiB | −19% |
| watch, per instance, debug N=400 | 72.7 KiB | 60.3 KiB | −17% |
| plain, settled after closing, N=1000 | 6,001,908 B | 22,708 B | −99.6% |

The close numbers come from this pull request's table shrinking; the resident numbers from boxing the monitor future. With only the shrink, the resident numbers stayed the same (the reviewer verified: 6,610,672 / 64,809,972 B after creating N=100/1000); the box is what moves them.

The thresholds follow, per platform: on Linux, per instance while running it becomes 61.5 KiB, between the normal worst of 60.3 KiB and the cheapest rollback at 62.0 KiB (the worst rollback is 72.7 KiB); on macOS the allocator spends more, and CI measured 68,234–70,772 B (66.6–69.1 KiB) normally and about 84 KiB after the rollback, so its bound is 78 KiB. On both, removing the `Box` again fails here; `Inner::monitor` also has a unit test, `the_monitor_future_stays_small`, asserting the future itself is at most 512 B (344 B normally, 1,872 B when inlined), which pins the rollback directly and is platform independent. After closing, the fixed part plus 128 B per instance, and the fixed part becomes 128 KiB (one watch, N=40 run left 58 KiB, about twice the worst; the pre-fix 188 KiB still fails).

## What the test covers and does not

Covers:

- fibers, service bindings (`Ctx::diagnostics`), `FiberStatusChanged` and all event listeners, tokio task counts;
- the test process's OS thread count on Linux (`Threads:` in `/proc/self/status`);
- live allocated bytes: the per-instance footprint while running, the close peak, the residue after closing;
- after closing 40 to 1000 instances, every count above is back to what it was before they were created, and the residual bytes do not grow with N.

Does not cover:

- **language runtime processes**: this test has Rust rows only and starts no Node, Python or other runtime. Rows, services, proxies, export slots and object handles inside runtime processes after closing are covered by `crates/rutis-loader/tests/instance_runtimes.rs` (D2/D3) and by S7 (`tests/e2e/tests/embedding.rs`; 20 cycles on pull requests, 3000 nightly);
- **non-zero service bindings inside an instance**: `plain`/`watch` provide no service, so this test checks only that the binding count does not change; isolation between instances and removing an instance's bindings are covered by `instance_services.rs`, `instance_runtimes.rs` and S7;
- **OS thread counts on macOS and Windows**: `Counts::threads` is 0 off Linux and is not asserted there; the full runs on those platforms still check the other counts;
- fds, the heap's RSS, allocator fragmentation: outside this test.

The conclusion for item 3 of #979, "look at what each instance and each row holds while it runs, and save what can be saved": the composition and the candidates are above; the monitor task's 13.4 KiB per instance is saved in this pull request; the running `HashMap`'s spare capacity and the per-row records are judged to be a cost of growth and of per-copy data, and are left alone. Runtime processes and cross-language bindings remain guarded by the tests listed above.
