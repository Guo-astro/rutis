# Means review: the rules in the #622 trace table that had only named cases (#1270)

[中文](means-0.23.md) · [Index](README.en.md) · [Re-check table](recheck-0.22.md) · [Guarantee register](../guarantees.en.md) · [Quality standard](../quality-standard.en.md)

Date: 2026-11-18. Baseline: `origin/main` `31047fc4`. Issue: [#1270](https://github.com/arcships/rutis/issues/1270) (parent [#622](https://github.com/arcships/rutis/issues/622)). This file is the record of which means each rule has and why; the per-rule decisions and the commands that check them are here.

## 1. Two readings

#622's completion condition says: every rule that should have a formal check is “covered” in the trace table (a mutation or the pre-fix configuration shows the check is not empty), **a rule that rests on tests alone names its tests**, and **a rule that moves to another means writes down why**.

- **Reading A** (the `upgrade` entry #1265 introduced and #1269 used): a semantic entry may carry `upgrade` — evidence is a model, a property test or a named test, it must resolve in the tree, a mutation must be written, and the row moves from “unverified”/“partly verified” to “verified”. All 21 rules were registered this way.
- **Reading B** (#622's own “means” column): a rule whose means is `SH`, `PT`, `TLA`, `TV` or `K` must have that check; a named test alone is partial, or the means changes to tests with a written reason.

Reading A's “verified” is not reading B's “covered”: a rule can be verified because its tests assert it fully while its means column still says `SH`. This file decides the means for each rule, which is the column #1270 is about.

The means column is one of the ten columns of #634's issue snapshot, exported verbatim into `rules/protocol-*.md`, and the exports are not edited by hand ([README](README.en.md) §5). The current decision therefore lives in two places:

- §2 of this file (per-rule decision and reason);
- the `why`/`whyEn` of the same-numbered entries in `tools/formal-trace-semantic.mjs`, which generate [re-check table](recheck-0.22.md) §5 and the [English re-check page](recheck-0.22.en.md); `node tools/formal-trace.mjs --check` checks the entries and their evidence.

## 2. The rules

The issue writes this batch as 21 rules, counting `loader/rows-gate-on-services a/b` as one line. Split by id it is **22 rules: 7 loader/host integration rules + 15 kernel/event rules**.

- “What it has” = the evidence registered in the trace table (`upgrade.evidence` in `tools/formal-trace-semantic.mjs`);
- “Means column” = what #634's snapshot says;
- “Decision” = the means under reading B;
- the commands run from the repository root; `RUTIS_TEST_RUNTIMES=node,python` is the gate of the loader/host runtime tests (unset, they return at once and nothing ran).

### 2.1 Seven loader/host integration rules: tests

| Rule | What it has | Means column | Decision | Why (§3) |
| --- | --- | --- | --- | --- |
| `loader/rows-gate-on-services a` | `rutis-loader/tests/scope.rs::inject_gates_a_row`, `rutis-loader/tests/runtime_rows.rs::rows_resolved_offline_wait_for_what_they_inject`, `rutis-loader/tests/row_inject_gate.rs::a_runtime_rows_inject_gates_it_and_is_leased_to_it`, `rutis-loader/tests/loader.rs::provider_update_reloads_consumers` | `SH + PT` | **tests** | The gate itself is the kernel's state machine (`core/D14`, `core/D22`, modelled); what this rule adds is the loader putting the name into the gate and leasing it into the runtime's process, which only a real runtime shows |
| `loader/rows-gate-on-services b` | `rutis-loader/tests/multilang.rs::a_provider_that_goes_stops_only_its_users`, `row_inject_gate.rs::a_runtime_rows_inject_gates_it_and_is_leased_to_it`, `runtime_rows.rs::a_plugin_that_disposes_itself_ends_its_row`, `loader.rs::provider_update_reloads_consumers` | `SH + TLA` | **tests** (the TLA half points at #625's `WithdrawBeforeUnload` design model) | A withdrawal stopping the consumer behind the kernel gate is model 9's job; this rule asserts “only the rows of that runtime that use it stop, the others keep running”, across processes |
| `loader/row-unload-leaves-nothing a` | `runtime_rows.rs::rows_unload_before_the_runtime` | `SH` | **tests** | The order is asserted by when the runtime's process delivers its cleanup to Cordis; a stand-in process tests the stand-in |
| `host/dev-reload b` | `rutis-host/tests/dev.rs::dev_follows_a_node_project`, `::dev_follows_a_python_project`, `::dev_follows_a_bun_project` (all three through `the_dev_loop`, whose fixture records cleanups and asserts the replaced instance is cleaned exactly once), `rutis-host/tests/reload.rs::rows_added_removed_and_changed_apply` | `PT + SH` | **tests** | One dev reload is a real build plus a process swap; #627's host models cover reload supervision, not “the old instance is cleaned once after the code changed” |
| `MNT-5-3` | `examples/dsh-baseline/tests/host.rs::persona_uses_the_system_prompt_the_rutis_host_provides` | `SH` | **tests** | The generated `injects` gate a real mount and a real subprocess; the kernel gate is modelled already |
| `MNT-5-7` | `rutis-bridge/tests/mount_host_service.rs::withdrawing_a_host_service_closes_the_mounts_process`, `examples/dsh-baseline/tests/host.rs::persona_uses_the_system_prompt_the_rutis_host_provides` | `SH` | **tests** | Half the rule is “the Node process closes with it and is not replaced in place”, whose evidence is process identity and end state — operating-system objects `SH` cannot replace |
| `MNT-11-4` | `runtime_rows.rs::rows_wait_for_their_hosts_not_the_runtime`, `instance_runtimes.rs::a_language_rows_inject_inside_an_instance_is_the_instances`, `row_inject_gate.rs::a_runtime_rows_inject_gates_it_and_is_leased_to_it` (this PR adds the replacement half), `instance_runtimes.rs::a_rows_instance_host_service_going_stops_it` (new in this PR), kernel model 9 | `SH` | **tests** | The same dispatcher's key is `host_key` globally and `host_key_in` in an instance; waiting and stopping are the kernel's rules, and the loader's part — putting the dispatcher on the right key and scope — needs the real runtime |

### 2.2 Fifteen kernel/event rules: models, PT, and tests with a reason

| Rule | What it has | Means column | Decision |
| --- | --- | --- | --- |
| `core/D7a` | model 28 (new, `rutis/src/fiber/shuttle_tests.rs::shuttle_an_unload_cancels_and_cleans_up_before_it_disposes`: the kernel's own `unload`, Unloading before the cleanups, the cancellation seen by them, LIFO, Disposed last), model 26, two `rutis/tests/lifecycle_order.rs` tests, `rutis/tests/interleave.rs` | `PT + SH` | **models + tests**: `PT` is `interleave.rs`'s sequential reference model, `SH` is models 28 and 26, and "waits for apply to exit" is asserted by `lifecycle_order::unload_waits_for_apply_to_exit_before_cleanup` (a public `FiberView`, which a shuttle model cannot drive) |
| `core/D7b` | models 12, 23, 9; four `rutis/tests/evicted_apply.rs` tests; three `rutis/tests/load_window.rs` tests | `SH` | **models + tests**: dispose, restart and eviction are models 12, 23 and 9; `update` shares the critical section with `restart`, but its dry-run and the two terminal checks around it have no model, so that half rests on tests (`load_window::an_update_right_after_mounting_is_taken_up_on_current_thread`, `evicted_apply::updating_a_cancelled_loading_consumer_runs_one_apply_without_failing`) |
| `core/D14a` | model 9; `rutis/tests/contract.rs::eviction_order`; the new `rutis/tests/contract.rs::eviction_drains_every_consumer_before_the_provider` (both consumers back at Pending, both cleanups before the provider's; mutation measured); `instance_subtrees::closing_subtree_disposes_consumer_before_provider` | `SH` | **models + tests**: model 9 covers one consumer stopping; "two consumers draining at once" is the new test |
| `core/D14b` | `rutis/tests/contract.rs::self_access_during_cleanup` | `PT` | **tests**: deterministic (reading one's own service during cleanup); a model would only add a stand-in; the mutation is to refuse `Ctx::get` while Unloading |
| `core/D21a` | model 19; `rutis/tests/contract.rs::single_service_evict`, `::isolate_no_cross_evict`; `rutis/tests/parity.rs::distinguishes_provider_incarnations_without_a_global_counter`; the registry-slot PT | `SH + PT` | **models + PT**: model 19 covers the restart in the window; "two generations of one key at once" is unreachable in this design (one binding per key per scope), so no model is forced and the boundary is in the register |
| `core/D30c` | `rutis/tests/contract.rs::cleanup_panics_contained` | `PT` | **tests**: `catch_unwind` → `PluginFailed` is deterministic, and model 13's cleanups only return `Err` |
| `core/D31b` | `rutis/tests/event_order.rs::emit_after_listener_panic_is_still_delivered`; `rutis-bridge/src/events/hub/shuttle_tests.rs::a_cancelled_or_panicking_emit_does_not_stop_the_queue` | `SH` | **tests + the host-queue model**: the kernel emit chain got no model this round — driving the real chain under the spawn stand-in reversed the start order of two same-name emits in one schedule (also without a panicking listener), and whether that is the stand-in or a real disorder is not settled (§5), so it is not `SH` evidence |
| `events/remote e` | model 10; `rutis/tests/events.rs::raw_dispatch_and_entry_calls` | `SH + PT` | **model + tests**: when `emit` completes is model 10; `invoke_entry` is a deterministic lookup the test asserts |
| `events/framework c` | model 24; K's `rutis/service`, `rutis/service-of-rust`, `rutis/service-scoped` | `K + SH` | **K + model**: K spans seven implementations and is the primary evidence; model 24 asserts one binding's `Provided` before its `Removed`; "every host service" is representative |
| `instance/subtrees.5` | models 16, 17; two `rutis/tests/instance_subtrees.rs` resource tests; two `rutis/src/fiber/transient_tests.rs` tests; `rutis/tests/transient_release.rs::churn_on_long_lived_root_releases_instances` | `SH + PT + tests` | **models + tests**: the means column already names tests — tasks, fibers, capacity and root-side records are resource checks; models 16 and 17 cover concurrent closes; the `PT` half has no property test of its own |
| `EVP-7.1-5` | model 6; K `unload/waits-and-cancels`; `node/rutis/test/events.test.mjs::unload aborts ctx.signal and waits for running listeners`; `rutis/tests/events.rs::unload_cancels_and_waits_for_running_listeners` | `K + SH` | **K + model**: one K case spans seven implementations; model 6 is the kernel's in-flight count |
| `EVP-12.1-6` | `rutis-bridge/tests/event_hub.rs::dropping_a_dispatch_cancels_the_remote_listener_it_started`, `::dispatches_cross_between_host_and_runtime`; `rutis/tests/events.rs::remote_entries_take_part_like_any_listener` | `SH + TV` | **tests + TV (not built)**: the deterministic cases in both directions are the current evidence; `TV` belongs to #624's trace validator (the event host's action log is still missing), and this row has no `SH` model of its own |
| `EVP-12.1-10` | model 10; `rutis/tests/events.rs::remote_entries_in_synchronous_modes`; `rutis/tests/waterfall_sync.rs::sync_waterfall_chains_listeners_into_the_terminal`; `rutis/src/bus/prop_tests.rs::the_five_dispatch_modes_match_a_reference_interpreter` | `SH + TV` | **model + PT + tests; TV to #624**: `dispatch_sync` and the synchronous waterfall are asserted by tests, and the five dispatch modes have a reference-interpreter differential |
| `INS-6-1` | `rutis/tests/instance_subtrees.rs::closing_a_subtree_withdraws_the_instance_keys_it_provided`, `::instance_keys_enforce_subtree_and_explain_pending_dependency`, `::a_recreated_child_gets_a_new_instance_id` | `SH` | **tests**: the instance key carrying its id and the subtree close withdrawing it are deterministic; concurrent subtree closes are models 16 and 17 (which serve `instance/subtrees.5`, not this row) |
| `MSV-4-2g` | `rutis-bridge/src/marked/shuttle_tests.rs::shuttle_a_marked_service_is_served_after_its_native_and_withdrawn_before_it`; `rutis-loader/tests/marked_services.rs::rows_of_every_language_call_a_marked_rust_service` | `SH + TLA` | **model + tests**: model 1 covers the withdrawal order; the consumer half spans three languages and is asserted by the test; the `TLA` half has no model |

## 3. Why the seven loader/host rules are tests

### 3.1 A kernel-style shuttle would test a stand-in

`SH` replaces the executor, the locks and the clock and re-runs **in-process** code under different schedules (quality standard §7.1, §7.3). None of these seven rules is judged in-process:

- They are about **across processes**: a name is leased into the Node/Python/Bun process, the mounted Node process ends with the host service, a dev reload swaps code in another process. These need the real subprocess; a stand-in `Process`/`Launcher` would have the model check the stand-in, not “the real runtime does this”.
- The quality standard §7.1 already says `SH` cannot prove anything about “external events it does not explore (real processes, the operating system, the runtime itself)”. These seven rules are exactly that column.
- #627 already attached the part that can be attached: the three `rutis-loader` models (binary runtime restart, replacement, the rows that asked for it) and the three `rutis-host` models (a reload waits for the process of the runtime it removed) model **supervision**, not the gate and the exit wiring in these crates.

So #622's third completion item (change the means and write down why) applies: the means is tests, the test names are in the trace table, and the mutation is in `upgrade.mutation`.

### 3.2 The mutation of each (what the tests catch)

| Rule | Mutation (these tests fail) |
| --- | --- |
| `loader/rows-gate-on-services a` | start the row before its `inject` resolves (`transitions.loads` above 1); or leave the declared dependencies out of the gate (the row is Active before the service arrives) |
| `loader/rows-gate-on-services b` | stop every row of the runtime on a withdrawal (the others are no longer Active); or do not start the row again when the service returns (it stays Pending) |
| `loader/row-unload-leaves-nothing a` | unload the runtime row before the row (`c: bye` lands after the process closed) |
| `host/dev-reload b` | skip the old instance's unload (0 cleanups) or unload it twice (2) |
| `MNT-5-3` | drop the host service from the generated `injects` (the mount applies before the service is provided, so no Pending is seen) |
| `MNT-5-7` | the mount's cleanup does not close the process (`first.ended()` is false); or the withdrawal only disables the service without unloading the mount (the view stays Active) |
| `MNT-11-4` | provide the dispatcher on another key (the row never starts); do not withdraw it (the row stays Active); keep the old binding after a replacement (the new value is not read); a dispatcher withdrawn inside an instance does not stop its row |

### 3.3 The two assertions this PR adds

`MNT-11-4`'s “follows a replacement” and “an instance's wait and stop” had no separate assertion; both are added as tests, not models:

- `rutis-loader/tests/row_inject_gate.rs::a_runtime_rows_inject_gates_it_and_is_leased_to_it`: after the host service is withdrawn a **different** dispatcher is provided; both rows start again and read the new value (`43`, not `42`). Mutation: keep the old binding — the test then reads `42` and fails (measured).
- `rutis-loader/tests/instance_runtimes.rs::a_rows_instance_host_service_going_stops_it`: disabling the provider row of an instance group stops both instances' `reader`, which wait for that name (`loader.waits` lists `js_timeline`); enabling it starts them again and each reads its own instance's timeline. Mutation: the rows keep running after the withdrawal — the `!active` assertion fails (measured, §4).

## 4. Checks

From the repository root:

```sh
node tools/check-guarantees.mjs                       # 219 rows, 1232 markers, 0 problems
node tools/formal-trace.mjs --check                   # 0 problems

# the evidence of the seven loader/host rules
RUTIS_TEST_RUNTIMES=node,python cargo test -p rutis-loader --features runtimes \
  --test scope --test runtime_rows --test row_inject_gate --test loader \
  --test multilang --test instance_runtimes
RUTIS_TEST_RUNTIMES=node,python cargo test -p rutis-bridge --test mount_host_service
RUTIS_TEST_RUNTIMES=node,python cargo test -p rutis-host --test dev -- --test-threads=1
RUTIS_TEST_RUNTIMES=node,python cargo test -p rutis-host --test reload -- --test-threads=1

# the fifteen kernel/event rules (the second PR)
RUSTFLAGS="--cfg rutis_shuttle" cargo test -p rutis --lib shuttle_ \
  # includes model 28 `shuttle_an_unload_cancels_and_cleans_up_before_it_disposes`
cargo test -p rutis --test contract eviction_drains_every_consumer_before_the_provider
cargo test -p rutis --test lifecycle_order --test evicted_apply --test load_window --test event_order
cargo test -p rutis --test instance_subtrees --test waterfall_sync --test parity

# `MNT-5-3`'s dsh case runs nightly; it needs node/baseline's pinned versions, and the mount
# `version` pins of examples/dsh-baseline dropped first (what nightly.yml does):
npm --prefix node/baseline ci
awk '/^\[/ { mounts = /^\[package\.metadata\.rutis-cordis\.mounts\./ } !(mounts && /^version = /)' \
  examples/dsh-baseline/Cargo.toml > /tmp/dsh-Cargo.toml && cp /tmp/dsh-Cargo.toml examples/dsh-baseline/Cargo.toml
RUTIS_TEST_RUNTIMES=node cargo test -p dsh-baseline
```

The two `rutis-host` files need `--test-threads=1`: run in parallel on one machine, three `dev` cases build TypeScript at once and interrupt each other (unrelated to this change; serially all ten pass).

## 5. Not done

- **The kernel emit chain's panic model was not written** (`core/D31b`). The first attempt drove the real `Ctx::emit` under a `SpawnScope` (the tail wait and the start loop are the kernel's) and the start order of two same-name emits came out reversed in some schedules (four emits delivered `[0, 2, 1, 3]`), and that reproduces **without a panicking listener** too (once in 300 random plus 300 PCT iterations). The cause is not settled: either `emit_erased` can lose the order while it holds the bus's std lock across `crate::sync::spawn` (the scope may run another task inline), or the stand-in does not compose with that lock (with the shuttle lock the deadlock went away, but a panic was then reported as a failed task). Until it is settled the chain gets no model evidence; it needs a reproduction of its own.
- The `TV` halves of `EVP-12.1-6` and `EVP-12.1-10` are still #624's: the event host's action log is not built, and the trace validator covers the session channel and the wait graph only.
- The `PT` halves of `events/remote e` and of `instance/subtrees.5` have no property test of their own, and this file does not call them covered.
- `core/D7b`'s `update` itself (the dry-run and the two terminal checks) has no model and rests on tests.
- `MNT-5-3` has one host service on nightly; the same case for the other implementations (Node, Bun, Python, Go, Swift, Kotlin) is not built, and the trace table says so.
- The TLA half of `loader/rows-gate-on-services b` points at #625's `WithdrawBeforeUnload` (a design model), not at a model in this repository's `specs/`; calling it “covered by TLA” needs a model first.
