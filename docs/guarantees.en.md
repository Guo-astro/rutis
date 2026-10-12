# rutis guarantee registry

[中文](guarantees.md) · [Quality standard](quality-standard.en.md) · [Quality status](quality-status.en.md)

Status: skeleton (#181). This registry lists rutis's core promises and contract behaviours, one row each: number, source, content, level, status, and the verification. The rules come from the [quality standard](quality-standard.en.md) §3 (levels of promise) and §9.4 (Q9.4.1–Q9.4.4).

The registry is **maintained by hand**: it is not extracted from the design documents, and CI does not check that it matches the tests (too many false alarms, and more upkeep than it is worth; to be reconsidered past 300 rows). It stays correct because PR authors and reviewers update it with each change (§5).

This PR registers `core/D1`–`core/D33` and `boundary/5.1`–`boundary/5.8`. The other families (§2) are reserved and will be filled in by later PRs; #181 closes once all of them are in.

---

## 1. Release check

- **0.10**: released with this registry as the baseline, recording the status of each row at that point. The statuses here do not gate the 0.10 release.
- **From 0.11 on**, Q9.4.4 applies: before each release, check this registry by hand:
  1. rows at level "core promise" with status "not verified" must be zero;
  2. rows at level "contract behaviour" with status "not verified" must not be more than at the previous release.
- Write the result in the release PR's description (two numbers: unverified core promises and unverified contract behaviours, compared with the previous release).

"Partly verified" does not count towards those two numbers, but a core promise that is only partly verified should have a matching plan in the quality status document.

---

## 2. Numbering

Numbers have the form `<family>/<name>` and are never reused; when a guarantee is withdrawn its row stays, with the status saying so and why.

| Family | Form | Source | In this PR |
| --- | --- | --- | --- |
| `core/` | `core/D1`–`core/D31` | [design-rust-port](design-rust-port.en.md) §3 decision table | registered |
| `core/` | `core/D32`, `core/D33` | [design-config-hot-update-and-dynamic-events-2026-09-21](design-config-hot-update-and-dynamic-events-2026-09-21.en.md) §1, §2 | registered |
| `core/` | `core/features.<section>` | sections of [core-features](core-features.en.md) | reserved |
| `boundary/` | `boundary/5.1`–`boundary/5.8` | the 8 boundary rules in [requirements-protocol-plugins](requirements-protocol-plugins.en.md) §5 | registered |
| `loader/` | `loader/<name>` | rutis-loader designs | reserved |
| `instance/` | `instance/<name>` | instance subtree and instance service designs | reserved |
| `bridge/` | `bridge/<name>` | session protocol, runtime and link designs | reserved |
| `host/` | `host/<name>` | rutis-host designs and user documentation | reserved; `host/rutis-json-format` registered (#207) |
| `dsh/` | `dsh/<name>` | rutis-dsh designs and documentation | reserved; `dsh/user-layer-format` registered (#207) |

`D` numbers keep the decision numbers of the design documents (including D20 and D27, which restate other numbers, and D26, merged into D27), so they line up with the designs and the test comments. In new families `<name>` is a short lower-case English phrase joined by hyphens (e.g. `loader/rollback`).

---

## 3. Levels and statuses

**Levels**, as in the quality standard §3:

| Level | Meaning |
| --- | --- |
| core promise | Q3.1: users build on it and cannot work around it when it fails (dependency gating; cleanup exactly once; rollback on failure with the old state still working; users depend on service names, not implementations; what cannot cross a boundary fails explicitly instead of degrading silently; interrupted calls fail explicitly, never report false success, never retry side effects automatically; nothing left behind) |
| contract behaviour | Q3.2: specific behaviour written in a design or user document (order, parameters, defaults, error kinds, timeouts) |
| explicitly not guaranteed | Q3.4: behaviour the documents say is not provided |

Implementation details (Q3.3) are not registered. When one row has a guaranteed part and a "not guaranteed" part, its level follows the guaranteed part and the content states what is not guaranteed.

**Statuses**:

| Status | Meaning |
| --- | --- |
| verified | at least one automated check asserts every point of the behaviour |
| partly verified | checks exist but cover only some points, or assert in a way that hides the property; the notes say what is missing |
| not verified | no automated check asserts it |
| explicitly not guaranteed | rows at level "explicitly not guaranteed"; a check that pins "really not provided", if any, is listed under verification |

The status says only whether a check asserts the behaviour. Core promises have further requirements under Q3.1 (every supported platform, under fault injection, two independent methods); whether those are met is tracked in the [quality status](quality-status.en.md), not here.

---

## 4. How verification is written

**In this registry**: the verification column lists test functions as `<path>::<function>`, with paths relative to `crates/`, e.g. `rutis/tests/contract.rs::state_transitions`; files outside `crates/` are given from the repository root (e.g. `node/rutis-runtime/test/...`). Rows guaranteed at compile time say "compile time" and why.

**In tests**: write the number in the doc comment of the test function (or of the whole test file), so a test leads back to its row:

```rust
/// guarantee: core/D24
#[tokio::test]
async fn status_events_arrive_in_commit_order() { /* ... */ }
```

A test that verifies several guarantees lists them separated by commas: `guarantee: core/D6, core/D14`. Other languages use their own comments (`//`, `#`).

### Existing markers

Before this registry the repository had 3 `guarantee:` markers. They cite issues and risks, not numbers from this registry:

| Where | What it says now | Future number |
| --- | --- | --- |
| `rutis-bridge/tests/concurrent_launch.rs::spawned_processes_started_at_once_hold_only_their_own_channel` | `issue #184 (risk C11, Q6.5.2)` | assigned when the `bridge/` family is registered |
| `rutis-bridge/tests/concurrent_launch.rs::node_runtimes_launched_at_once_hold_only_their_own_channel` | same | same |
| `rutis-host/tests/signals.rs` (file level) | every cleanup runs exactly once, runtime processes have exited when a signal ends the host (Q5.4.4, Q6.6.3) | assigned when the `host/` family is registered; the exactly-once part also maps to `core/D6` |

This PR does not touch test code; the PRs that register the `bridge/` and `host/` families also change these markers to registry numbers.

---

## 5. When to update the registry

- A guarantee is added to or changed in a design or user document: the same PR adds or changes its row (with its level, as Q3 requires).
- A test verifying a guarantee is added: write `guarantee: <number>` in the test, and in the same PR add the test to the verification column and update the status if needed.
- A listed test is removed, skipped (`#[ignore]`, platform conditions) or weakened: the same PR updates the verification column and the status (Q9.4.3).
- A test is renamed or moved: update the verification column.

The PR template has a checklist item for this.

---

## 6. Core: `core/D1`–`core/D33`

Sources: D1–D31 in [design-rust-port](design-rust-port.en.md) §3; D32 and D33 in [design-config-hot-update-and-dynamic-events-2026-09-21](design-config-hot-update-and-dynamic-events-2026-09-21.en.md) §1, §2. Baseline `main` `2e77fdc`.

Core promises state observable behaviour; mechanisms (locks, tasks, triples) stay in the design documents (Q3.3). D20, D26 and D27 restate or were merged into other numbers; their numbers are kept and not counted.

| Number | Content | Level | Status | Verification | Notes |
| --- | --- | --- | --- | --- | --- |
| `core/D1` | Traits that need dyn use the hand-written alias `BoxFuture<'a, T>`, not the async-trait macro | contract behaviour | verified | compile time: every test implementing `Plugin` / `Listener` (e.g. `rutis/tests/contract.rs`) | a type-level rule; no behaviour test |
| `core/D2` | Typed event payloads (`Event` trait), erased internally as `Arc<dyn Any + Send + Sync>`; `serde_json::Value` only at serialization boundaries, not in the core | contract behaviour | verified | `rutis/tests/contract.rs::emit_async_safe`, `rutis/tests/typed_dispatch.rs::typed_identity_and_prefix_scope_are_distinct`; `crates/rutis/Cargo.toml` has no serde dependency | "not in the core" shows only in the manifest; no automated check |
| `core/D3` | Callback-registry bus (hook semantics), listeners are owned `Arc<dyn Listener>`, no channels | contract behaviour | verified | `rutis/tests/contract.rs::plugin_registers_n_listeners`, `::prepend_order`, `::once_once` | |
| `core/D4` | Four dispatch modes: `emit` fire-and-forget (Err / panic to the ErrorSink); `parallel` waits for all and aggregates errors; `serial` in order until short-circuit; `waterfall` continuation style, not calling `next` vetoes | contract behaviour | verified | `rutis/tests/contract.rs::emit_async_safe`, `::emit_error_sink`, `::parallel_aggregates`, `::parallel_waits_all`, `::serial_bails`, `::waterfall_veto_around` | |
| `core/D5` | Fiber state changes within one lock; inside the lock only FIFO enqueueing, never holding a lock across await; callbacks run outside the lock | contract behaviour | partly verified | `rutis/tests/contract.rs::mixed_concurrent_ops_complete`, `rutis/tests/interleave.rs::restart_settle_and_dispose_interleave`, `::dependency_churn_interleaves_with_restart`; CI clippy `-D warnings` includes `await_holding_lock` | "lock across await" is checked statically by clippy (std locks only); the FIFO order inside the lock is not asserted directly (see D24) |
| `core/D6` | The cleanups a plugin registered run exactly once when it unloads: concurrent disposes wait for the same unload and get the same error; one generation never runs them twice, and the next generation after a restart starts afresh | core promise | verified | `rutis/tests/contract.rs::concurrent_dispose_join`, `::exactly_once_same_error`, `::dispose_idempotent`, `::cross_generation_isolation`, `rutis/tests/parity.rs::returns_one_disposal_promise_and_joins_cleanup_already_in_progress` | the mechanism (one `Arc<TransitionTask>` in the lock) is in the design; D20 is the same row |
| `core/D7` | Cooperative cancellation: a child `CancellationToken` per fiber generation, exposed as `ctx.cancelled()`; five-step unload (mark unloading → cancel the current generation → wait for apply to exit → clean the EffectRecord → publish the final state); a plugin that ignores the token makes dispose wait forever (documented limit) | core promise | partly verified | `rutis/tests/contract.rs::cancel_wakes_awaiters`, `::cancel_during_loading`, `::cancel_idempotent`, `::cancel_cascades`, `::disposal_deadline_reports_loading_and_later_joins` | only outcomes are asserted; the order of the five steps is not (#177); D27 (and D26, merged into it) is the same row |
| `core/D8` | Runtime access: inject a `Handle` at construction first (`Ctx::root_with`); the automatic path `Handle::try_current()` returns a clear error on failure and never creates a runtime implicitly | contract behaviour | not verified | — | implemented in `rutis/src/ctx.rs` (`Ctx::root`); no test calls `Ctx::root()` outside a runtime (#177) |
| `core/D9` | Operations from a stale context (an earlier generation) are rejected, independent of scheduling order; **listener completion order is not part of the contract** (explicitly not guaranteed) | contract behaviour | verified | `rutis/tests/contract.rs::cross_generation_isolation`, `rutis/tests/generation_registration.rs::rejects_late_registration_current_thread`, `::rejects_late_registration_multi_thread`, `::rejects_registration_after_failed_apply`, `rutis/tests/host_control.rs::a_stale_context_cannot_dispose_the_next_generation` | graded by the guaranteed part. `yield_now` as a fairness hint is implementation. No test pins "completion order is not fixed" |
| `core/D10` | Plugin identity = `FiberView.id: PluginId`, display name `Plugin::name()`; `is_unique`-style deduplication optional | contract behaviour | verified | `rutis/tests/host_control.rs::view_finds_a_fiber_by_id` | `is_unique` not implemented (optional in the design) |
| `core/D11` | Error type `CordisError`: derived with thiserror (an ordinary dependency), `Error + Send + Sync + 'static`, **not Clone**, shared through `Arc`, aggregated as a `Vec` | contract behaviour | partly verified | compile time: `Send + Sync + 'static` follow from passing `Arc<CordisError>` across tasks | only indirect compile-time evidence; "not Clone" is not asserted (a #177 PR adds a compile_fail doc test) |
| `core/D12` | Config validation: `Plugin::validate` checks the config the instance holds; stored only after validation passes; no schema library | contract behaviour | verified | `rutis/tests/contract.rs::validate_panic_marks_failed`, `rutis/tests/host_control.rs::set_config_stores_without_restarting`, `rutis/tests/config_update.rs::update_dry_run_failure_leaves_state_untouched` | |
| `core/D13` | Service locator `get::<T>() -> Option<Arc<T>>`: explicit, keyed by type, failure is `None`; walks up the parent chain | contract behaviour | verified | `rutis/tests/contract.rs::typed_roundtrip`, `::parent_chain_lookup`, `::keyed_multi_instance` | |
| `core/D14` | Dependency-gated reload: when a provider withdraws a service, the consumers depending on it stop first (back to Pending), the provider cleans up last and can still reach its own services meanwhile; only consumers of the withdrawn service are evicted; they start again when the service is back | core promise | verified | `rutis/tests/contract.rs::eviction_order`, `::self_access_during_cleanup`, `::single_service_evict`, `::provider_reload_reactivates`, `::check_evicts`, `rutis/tests/instance_subtrees.rs::closing_subtree_disposes_consumer_before_provider` | the design's "consumers drain concurrently" is mechanism and not asserted: `eviction_order` asserts the order along a dependency chain (C → M → P), not concurrency |
| `core/D15` | tower compatibility only as an optional adapter, **not in the core**; lifecycle hooks are not forced into `Service` / `Layer` | explicitly not guaranteed | explicitly not guaranteed | — | the core provides no tower compatibility; no adapter exists either |
| `core/D16` | Fully asynchronous event system with four dispatch modes; listener helper trait `call<'a>`; `Event::Value` associated type; `emit` / `parallel` payloads are `Arc<E>` | contract behaviour | verified | `rutis/tests/contract.rs::emit_async_safe`, `::parallel_aggregates`, `::serial_bails`, `::waterfall_veto_around` | after bail was removed, 0.5 added the synchronous `bail_sync` / `waterfall_sync` (see [core-features](core-features.en.md)); they belong to `core/features.*`, not yet registered |
| `core/D17` | waterfall has its own registration surface (`on_waterfall` + `WaterfallListener`); `next` is the fallback continuation supplied by the caller | contract behaviour | verified | `rutis/tests/contract.rs::waterfall_veto_around`, `rutis/tests/event_keys.rs::keyed_waterfall_veto_and_chain` | |
| `core/D18` | Both Disposer variants of `Effect` return `Result<(), CordisError>`; closures capture an owned `Ctx` | contract behaviour | verified | `rutis/tests/contract.rs::effect_yields_disposer`, `::aggregate_no_flatten`, `::restart_cleanup_errors_to_sink`, `rutis/tests/cleanup_errors.rs::unconsumed_early_cleanup_error_reaches_unload_with_same_identity` | |
| `core/D19` | Config baked into the instance: `Plugin` has no associated type, a concrete plugin holds its config from `new(config)`; the registry stores only `Arc<dyn Plugin>` | contract behaviour | verified | `rutis/tests/parity.rs::plugin_apply_functional_plugin`, `rutis/tests/contract.rs::plugin_provides_n_services` | the design's "`update` deferred to M4" is implemented as D32 |
| `core/D20` | same as D6 | — | — | — | the design says "see D6"; number kept, not counted |
| `core/D21` | Eviction is exact to the provider's registration: only consumers that depend on the withdrawn provider (same plugin, same generation, same service key) are evicted; `isolate(key, label)` scopes by service key, the same label shares, a child scope falls back to its parent; users depend only on the service key, not on which plugin provides it | core promise | verified | `rutis/tests/contract.rs::isolate_scoping`, `::isolate_no_cross_evict`, `::mismatched_service_key_never_satisfies_injection`, `rutis/tests/parity.rs::isolate_isolated_context`, `::isolate_shared_label`, `::distinguishes_provider_incarnations_without_a_global_counter` | the mechanism (the `(PluginId, generation, ServiceKey)` triple) is in the design |
| `core/D22` | A plugin whose dependencies are not ready stays Pending, does not start and reports no error, and starts once they arrive; **dependency cycles are not detected** (services register dynamically, a cycle cannot be proven; explicitly not guaranteed) | core promise | partly verified | `rutis/tests/contract.rs::pending_not_failed`, `::waits_for_dependency`, `::late_provider_activates` | the core promise is the gating part, which is verified; "cycles not detected" has no regression test pinning it (#177) |
| `core/D23` | `Ctx::effect()` registers a cleanup and hands back a Disposer; incremental (async-iterable) effects are deliberately not provided | contract behaviour | verified | `rutis/tests/contract.rs::effect_yields_disposer`, `::effect_during_cleanup_fails`, `rutis/tests/parity.rs::rejects_effect_registration_during_unload` | incremental effects are not provided; nothing to test |
| `core/D24` | `FiberStatusChanged`: FIFO enqueued inside the lock, dispatched outside it; commit order guaranteed (generation as sequence), listener completion order not | contract behaviour | partly verified | `rutis/tests/contract.rs::state_transitions` | the test sorts by `seq` before asserting, which hides the delivery order (#177) |
| `core/D25` | Error layers: identity belongs to the `TransitionTask` (cached `Arc<CordisError>`), wrapping to `PluginFailed` (`#[source]`), not recursive | contract behaviour | verified | `rutis/tests/contract.rs::exactly_once_same_error`, `rutis/src/error.rs::plugin_failure_display_and_sink_preserve_each_cause_once`, `::sink_recurses_into_aggregate_used_as_plugin_failure_source`, `rutis/tests/parity.rs::keeps_plugin_execution_failure_separate_from_rollback_cleanup_failure` | |
| `core/D26` | merged into D27 (that is, D7) | — | — | — | number kept, not counted |
| `core/D27` | same as D7 | — | — | — | the design says "see D7"; number kept, not counted |
| `core/D28` | Resource ownership: resources registered through `Ctx` belong to the fiber automatically; a `Disposer` only releases early; a `Ctx` from isolate keeps the original fiber's ownership | core promise | partly verified | `rutis/tests/contract.rs::auto_ownership`, `::effect_yields_disposer`, `rutis/tests/event_keys.rs::keyed_listener_removed_with_owner_fiber` | "a `Ctx` from isolate keeps the original fiber's ownership" has no test (a plugin registering through isolate, its resources reclaimed on unload) |
| `core/D29` | Events are not filtered across isolate; isolate only isolates the service registry; scoped events need a sub-bus built by the user | contract behaviour | not verified | — | no regression test pins "not filtered" (#177) |
| `core/D30` | A failing or panicking plugin neither hangs nor spreads: when apply fails or panics the plugin becomes Failed and what it registered is rolled back, and waiters get an explicit error; a listener panic reaches the ErrorSink; a cleanup panic is caught and wrapped as `PluginFailed` | core promise | partly verified | `rutis/tests/contract.rs::apply_panics_complete_and_roll_back_current_thread`, `::apply_panics_complete_and_roll_back_multi_thread`, `::driver_panic_completes_waiters_with_error`, `::apply_failure_rolls_back`, `::emit_error_sink`, `::cleanup_panics_contained`, `::serial_panic_contained`, `rutis/tests/parity.rs::keeps_plugin_execution_failure_separate_from_rollback_cleanup_failure` | covers Q3.1's "roll back on failure" and "interrupted calls fail explicitly". A panic in an async waterfall callback reaching the caller is tested only for the synchronous version (#177) |
| `core/D31` | `emit` dispatch order: one event type dispatches serially in emission order; a panic does not break the chain; a listener emitting the same type again goes to the tail without deadlock; **no order across event types**; listener completion order still not part of the contract | contract behaviour | partly verified | `rutis/tests/dispatch_chain_probe.rs::dispatch_chain_does_not_fork_under_concurrent_emit`, `rutis/tests/event_keys.rs::keyed_emit_preserves_emission_order_per_name`, `rutis-agent/tests/order_probe.rs::emit_delivers_in_order_back_to_back_multi_thread` | only a probabilistic probe and single-emitter order; per-emitter order with several emitters, delivery after a panic, and reentrant emit going to the tail are not tested. `rutis-agent` is an example project, outside the test scope; the first #177 PR adds deterministic D31 tests in `crates/rutis` |
| `core/D32` | Config hot update: `update(config)` while running; a dry run first (`validate_config` → `build` → instance `validate`), on failure an error and nothing stored or restarted, the old state keeps working; on success a restart per the state matrix; config is not part of dependency identity; **no `internal/update` waterfall, persistence belongs to the host layer** (D32d, explicitly not guaranteed) | core promise | verified | all 15 tests in `rutis/tests/config_update.rs`, e.g. `::update_active_applies_new_config_and_cleans_once`, `::update_dry_run_failure_leaves_state_untouched`, `::update_during_loading_converges_to_new_config`, `::concurrent_update_and_dispose_settle_correctly` | dropping the `update` future is not tested (#177) |
| `core/D33` | Dynamic event keys: `TypeKey` qualified names accept runtime strings; a keyed surface on the event bus inherits the four modes, once / prepend and cleanup with the fiber; the same name with different types, and different names, do not cross; **events are still not filtered by isolate** (D33d, same as D29) | contract behaviour | verified | all 11 tests in `rutis/tests/event_keys.rs`, e.g. `::keyed_channels_are_isolated_by_name`, `::runtime_constructed_name_dispatches`, `::same_name_different_event_types_do_not_cross` | for D33d see the status of D29 |

---

## 7. Boundary rules for Cordis plugins: `boundary/5.1`–`boundary/5.8`

Source: [requirements-protocol-plugins](requirements-protocol-plugins.en.md) §5.

| Number | Content | Level | Status | Verification | Notes |
| --- | --- | --- | --- | --- | --- |
| `boundary/5.1` | Services: published and replaced through `ctx.provide` / property assignment; replacing directly with `ctx.set` sends no notification, and the rutis side switches to the new object after the next call to that service | contract behaviour | verified | `rutis-bridge/tests/service_projection.rs::projected_service_follows_replacement_and_withdrawal`, `rutis-bridge/tests/projection_lifecycle.rs::a_throwing_method_still_refreshes_the_slot`, `::a_call_on_an_exported_object_refreshes_the_slots`, `::withdrawal_and_immediate_reregistration_keep_the_service` | |
| `boundary/5.2` | Events across the boundary: `emit` is only a notification and does not guarantee the rutis side has run when it returns; use `parallel` / `serial` / `bail` to wait or get a result | contract behaviour | partly verified | `rutis-bridge/tests/event_forwarding.rs::cordis_events_reach_rutis_listeners`, `::rutis_events_reach_cordis_listeners` | only `emit` and `parallel` are verified; `serial` / `bail` across the boundary are not tested |
| `boundary/5.3` | Event order: native order only within one side; the other side's listeners run as a group at a fixed position; code relying on interleaved cross-side order is not supported | contract behaviour | not verified | — | #178 |
| `boundary/5.4` | Middleware chains: waterfall / bail chains do not interleave across sides; the other side's group takes part as one link | contract behaviour | not verified | — | #178 |
| `boundary/5.5` | Framework-internal events `internal/*` do not cross the boundary | contract behaviour | not verified | — | #178 |
| `boundary/5.6` | Synchronous waits: a synchronous method that waits, while running, for a result needing the Node event loop to advance (timers, Promises) gets a `SyncWaitCycle` error instead of hanging | core promise | verified | `rutis-bridge/tests/rpc_callbacks.rs::node_sync_wait_pumps_callbacks_and_reports_its_executor_cycle` (the fixture `node/rutis-runtime/test/fixtures/rpc-client.mjs` asserts `SyncWaitCycle`), `::sync_await_reports_known_executor_cycle_but_allows_ready_future`, `::sync_wait_detects_an_async_callback_already_running_on_its_runtime` | what cannot cross a boundary fails explicitly (Q3.1) |
| `boundary/5.7` | Services provided by the host are proxies generated from the interface on the Cordis side: calls through the interface behave natively, `instanceof` does not hold | contract behaviour | not verified | — | #178 |
| `boundary/5.8` | Event loop: plugins must not block it for long; the compatibility layer adds no timeouts to calls, and a rutis-side caller that needs one uses the async methods and sets it itself (timing out cancels), while the application decides how long unloading waits; an uncaught exception / rejection ends the whole Node process, and the services of that mount are withdrawn with it | core promise | partly verified | `rutis-bridge/tests/projection_lifecycle.rs::a_crashed_process_withdraws_its_services`, `rutis-bridge/tests/bun_runtime.rs::an_uncaught_error_ends_the_runtime` | withdrawal after the process exits is verified (Node via `process.exit`); an uncaught exception ending the process is verified only on the Bun runtime; "no timeouts" is not asserted |

---

## 8. Persisted formats: `host/rutis-json-format`, `dsh/user-layer-format`

Source: [design-quality-step2-2026-10-11](design-quality-step2-2026-10-11.md) §7; the `format` section of the user documentation [rutis-host](guide/rutis-host.en.md); the release process in "Persisted samples" of [release](release.en.md) (#207, risk J3).

| Number | Content | Level | Status | Verification | Notes |
| --- | --- | --- | --- | --- | --- |
| `host/rutis-json-format` | The top-level `format` of `rutis.json` is its format version, optional and read as 1 when absent; a newer one than this rutis-host supports is refused before any other field is read, saying to upgrade rutis-host; `rutis-host new` writes it in the files it generates; every released version's `rutis.json` (from 0.8.0) still reads with the meaning it had then | contract behaviour | verified | `rutis-host/tests/config.rs::the_format_is_checked`, `rutis-host/tests/errors.rs::a_configuration_that_is_not_there_or_not_valid`, `rutis-host/tests/persisted.rs::every_release_s_rutis_json_reads_as_it_did`, `::formats_no_longer_read_are_refused`, `rutis-host/src/new.rs::projects_are_created_with_their_names_filled_in` | samples and expectations in `tests/fixtures/persisted/<version>/`; the expectations of 0.8.0, 0.9.0 and 0.9.1 were produced by each tag's own parser |
| `dsh/user-layer-format` | The dsh user layer `<profile>/cordis.patch.yml` (a Cordis configuration format) has no format version; the user layer each released version wrote (from 0.8.0) still reads as the same patches | contract behaviour | verified | `rutis-dsh/tests/persisted.rs::every_release_s_user_layer_reads_as_it_did` | no version field (Cordis configuration is not versioned), so compatibility rests on the samples alone; an old format that no longer reads is not refused explicitly |

---

## 9. Counts (baseline `2e77fdc`)

| Scope | Verified | Partly verified | Not verified | Explicitly not guaranteed | Total |
| --- | --- | --- | --- | --- | --- |
| `core/D*` (without D20, D26, D27) | 19 | 8 | 2 | 1 | 30 |
| `boundary/5.*` | 2 | 2 | 4 | 0 | 8 |
| `host/`, `dsh/` (§8) | 2 | 0 | 0 | 0 | 2 |
| Total | 23 | 10 | 6 | 1 | 40 |

Of these, 10 are core promises: 5 verified (D6, D14, D21, D32, 5.6), 5 partly verified (D7, D22, D28, D30, 5.8), 0 not verified. The 6 unverified contract behaviours (D8, D29, 5.3, 5.4, 5.5, 5.7) are the 0.10 baseline. The maintainer confirms which rows are core promises and may change them.
