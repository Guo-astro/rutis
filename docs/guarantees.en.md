# rutis guarantee registry

[中文](guarantees.md) · [Quality standard](quality-standard.en.md) · [Quality status](quality-status.en.md)

Status: being filled in (#181). This registry lists rutis's core promises and contract behaviours, one row each: number, source, content, level, status, and the verification. The rules come from the [quality standard](quality-standard.en.md) §3 (levels of promise) and §9.4 (Q9.4.1–Q9.4.4).

The registry is **maintained by hand**: it is not extracted from the design documents, and CI does not check that it matches the tests (too many false alarms, and more upkeep than it is worth; to be reconsidered past 300 rows). It stays correct because PR authors and reviewers update it with each change (§5).

Registered: `core/` (the D numbers, and the `core/features.*` and `core/typed.*` rows that have tests), `boundary/`, and the `instance/`, `bridge/`, `host/`, `dsh/` and `sdk/` rows that have tests. The `loader/` family and the rest of `host/` will be filled in by later PRs; #181 closes once all of them are in.

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

| Family | Form | Source | Status |
| --- | --- | --- | --- |
| `core/` | `core/D1`–`core/D31` | [design-rust-port](design-rust-port.en.md) §3 decision table | registered |
| `core/` | `core/D32`, `core/D33` | [design-config-hot-update-and-dynamic-events-2026-09-21](design-config-hot-update-and-dynamic-events-2026-09-21.en.md) §1, §2 | registered |
| `core/` | `core/features.<section>` | sections of [core-features](core-features.en.md) | partly registered |
| `core/` | `core/typed.<name>` | [design-typed-plugin-2026-10-02](design-typed-plugin-2026-10-02.en.md) | registered |
| `boundary/` | `boundary/5.1`–`boundary/5.8` | the 8 boundary rules in [requirements-protocol-plugins](requirements-protocol-plugins.en.md) §5 | registered |
| `instance/` | `instance/<design>.<section>` | instance designs: `subtrees` = [design-instance-subtrees](design-instance-subtrees.en.md), `services` = [design-instance-services-2026-10-08](design-instance-services-2026-10-08.en.md) | partly registered |
| `bridge/` | `bridge/<name>` | sessions, links, carriers, cross-language values: [design-remote-plugins-2026-10-03](design-remote-plugins-2026-10-03.en.md), [requirements-protocol-plugins](requirements-protocol-plugins.en.md) §6, [plugin API](guide/plugin-api.en.md), quality standard Q6.2, Q6.5 | partly registered |
| `loader/` | `loader/<name>` | rutis-loader designs | reserved |
| `host/` | `host/<name>` | rutis-host designs and user documentation | reserved; `host/rutis-json-format` registered (#207) |
| `dsh/` | `dsh/<name>` | rutis-dsh designs and documentation | reserved; `dsh/user-layer-format` registered (#207) |
| `sdk/` | `sdk/<name>` | plugin SDKs and test tools: [plugin API](guide/plugin-api.en.md), quality standard Q6.7 | `sdk/testing-matches-runtime` registered (#206) |

How `<name>` is formed in each family:

- `core/D<n>`: the decision numbers of the design documents (including D20 and D27, which restate other numbers, and D26, merged into D27), so they line up with the designs and the test comments.
- `core/features.<section>`: one short lower-case English name per core-features section, e.g. `errors` = "Errors and panics". A section with several guarantees adds `.<name>`.
- `core/typed.<name>`, `bridge/<name>`, `loader/<name>`, `host/<name>`, `dsh/<name>`, `sdk/<name>`: a lower-case English phrase joined by hyphens that states the guarantee (e.g. `bridge/in-flight-call-fails-once`).
- `instance/<design>.<section>`: the design's short name and section number, e.g. `instance/subtrees.2.2` is design-instance-subtrees §2.2. A section with several guarantees adds `.<name>`.

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

### Early markers

The 3 `guarantee:` markers written before this registry cited issues and risks:

| Where | Number now |
| --- | --- |
| `rutis-bridge/tests/concurrent_launch.rs::spawned_processes_started_at_once_hold_only_their_own_channel`, `::node_runtimes_launched_at_once_hold_only_their_own_channel` | changed to `bridge/spawned-processes-own-channel` |
| `rutis-host/tests/signals.rs` (file level: every cleanup runs exactly once, runtime processes have exited when a signal ends the host, Q5.4.4, Q6.6.3) | unchanged: the `host/` family is not registered yet and will change it; the exactly-once part is `core/D6` in the kernel |

---

## 5. When to update the registry

- A guarantee is added to or changed in a design or user document: the same PR adds or changes its row (with its level, as Q3 requires).
- A test verifying a guarantee is added: write `guarantee: <number>` in the test, and in the same PR add the test to the verification column and update the status if needed.
- A listed test is removed, skipped (`#[ignore]`, platform conditions) or weakened: the same PR updates the verification column and the status (Q9.4.3).
- A test is renamed or moved: update the verification column.

The PR template has a checklist item for this.

---

## 6. Core: `core/D1`–`core/D33`

Sources: D1–D31 in [design-rust-port](design-rust-port.en.md) §3; D32 and D33 in [design-config-hot-update-and-dynamic-events-2026-09-21](design-config-hot-update-and-dynamic-events-2026-09-21.en.md) §1, §2. Baseline `main` `121eded`.

Core promises state observable behaviour; mechanisms (locks, tasks, triples) stay in the design documents (Q3.3). D20, D26 and D27 restate or were merged into other numbers; their numbers are kept and not counted.

| Number | Content | Level | Status | Verification | Notes |
| --- | --- | --- | --- | --- | --- |
| `core/D1` | Traits that need dyn use the hand-written alias `BoxFuture<'a, T>`, not the async-trait macro | contract behaviour | verified | compile time: every test implementing `Plugin` / `Listener` (e.g. `rutis/tests/contract.rs`) | a type-level rule; no behaviour test |
| `core/D2` | Typed event payloads (`Event` trait), erased internally as `Arc<dyn Any + Send + Sync>`; `serde_json::Value` only at serialization boundaries, not in the core | contract behaviour | verified | `rutis/tests/contract.rs::emit_async_safe`, `rutis/tests/typed_dispatch.rs::typed_identity_and_prefix_scope_are_distinct`; `crates/rutis/Cargo.toml` has no serde dependency | "not in the core" shows only in the manifest; no automated check |
| `core/D3` | Callback-registry bus (hook semantics), listeners are owned `Arc<dyn Listener>`, no channels | contract behaviour | verified | `rutis/tests/contract.rs::plugin_registers_n_listeners`, `::prepend_order`, `::once_once` | |
| `core/D4` | Four dispatch modes: `emit` fire-and-forget (Err / panic to the ErrorSink); `parallel` waits for all and aggregates errors; `serial` in order until short-circuit; `waterfall` continuation style, not calling `next` vetoes | contract behaviour | verified | `rutis/tests/contract.rs::emit_async_safe`, `::emit_error_sink`, `::parallel_aggregates`, `::parallel_waits_all`, `::serial_bails`, `::waterfall_veto_around` | |
| `core/D5` | Fiber state changes within one lock; inside the lock only FIFO enqueueing, never holding a lock across await; callbacks run outside the lock | contract behaviour | partly verified | `rutis/tests/contract.rs::mixed_concurrent_ops_complete`, `rutis/tests/interleave.rs::restart_settle_and_dispose_interleave`, `::dependency_churn_interleaves_with_restart`; CI clippy `-D warnings` includes `await_holding_lock` | "lock across await" is checked statically by clippy (std locks only); the FIFO order inside the lock is not asserted directly (see D24) |
| `core/D6` | The cleanups a plugin registered run exactly once when it unloads: concurrent disposes wait for the same unload and get the same error; one generation never runs them twice, and the next generation after a restart starts afresh | core promise | verified | `rutis/tests/contract.rs::concurrent_dispose_join`, `::exactly_once_same_error`, `::dispose_idempotent`, `::cross_generation_isolation`, `rutis/tests/parity.rs::returns_one_disposal_promise_and_joins_cleanup_already_in_progress` | the mechanism (one `Arc<TransitionTask>` in the lock) is in the design; D20 is the same row |
| `core/D7` | Cooperative cancellation and unload order: each plugin generation has its own cancellation token (`ctx.cancelled()`); on unload, Unloading is published and the current generation cancelled, apply is awaited, cleanups then run LIFO, and Disposed is published only after the last one; a plugin that ignores the token makes dispose wait forever (documented limit) | core promise | verified | `rutis/tests/lifecycle_order.rs::unload_publishes_unloading_and_cancels_before_cleanup_and_disposed_after`, `::unload_waits_for_apply_to_exit_before_cleanup`, `rutis/tests/contract.rs::cancel_wakes_awaiters`, `::cancel_during_loading`, `::cancel_idempotent`, `::cancel_cascades`, `::disposal_deadline_reports_loading_and_later_joins` | asserts that publishing Unloading, cancelling and waiting for apply all come before cleanup, cleanup is LIFO, and Disposed comes after all of it. dispose and the like cancel up front when called, so the order between publishing Unloading and cancelling cannot be observed and is not asserted. D27 (and D26, merged into it) is the same row |
| `core/D8` | Runtime access: inject a `Handle` at construction first (`Ctx::root_with`); the automatic path `Handle::try_current()` returns a clear error on failure and never creates a runtime implicitly | contract behaviour | verified | `rutis/src/ctx.rs::root_without_a_runtime_is_an_explicit_error` | |
| `core/D9` | Operations from a stale context (an earlier generation) are rejected, independent of scheduling order; **listener completion order is not part of the contract** (explicitly not guaranteed) | contract behaviour | verified | `rutis/tests/contract.rs::cross_generation_isolation`, `rutis/tests/generation_registration.rs::rejects_late_registration_current_thread`, `::rejects_late_registration_multi_thread`, `::rejects_registration_after_failed_apply`, `rutis/tests/host_control.rs::a_stale_context_cannot_dispose_the_next_generation` | graded by the guaranteed part. `yield_now` as a fairness hint is implementation. No test pins "completion order is not fixed" |
| `core/D10` | Plugin identity = `FiberView.id: PluginId`, display name `Plugin::name()`; `is_unique`-style deduplication optional | contract behaviour | verified | `rutis/tests/host_control.rs::view_finds_a_fiber_by_id` | `is_unique` not implemented (optional in the design) |
| `core/D11` | Error type `CordisError`: derived with thiserror (an ordinary dependency), `Error + Send + Sync + 'static`, **not Clone**, shared through `Arc`, aggregated as a `Vec` | contract behaviour | verified | the two doc tests on `CordisError` in `rutis/src/error.rs` (`compile_fail,E0277` asserts it is not Clone; a positive one asserts `Error + Send + Sync + 'static` and sharing through `Arc`) | |
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
| `core/D24` | `FiberStatusChanged` is delivered in commit order (per plugin, seq is consecutive and each `from` is the previous `to`); listener completion order is not guaranteed | contract behaviour | verified | `rutis/tests/contract.rs::state_transitions`, `rutis/tests/event_order.rs::status_events_arrive_in_commit_order_across_concurrent_fibers` | |
| `core/D25` | Error layers: identity belongs to the `TransitionTask` (cached `Arc<CordisError>`), wrapping to `PluginFailed` (`#[source]`), not recursive | contract behaviour | verified | `rutis/tests/contract.rs::exactly_once_same_error`, `rutis/src/error.rs::plugin_failure_display_and_sink_preserve_each_cause_once`, `::sink_recurses_into_aggregate_used_as_plugin_failure_source`, `rutis/tests/parity.rs::keeps_plugin_execution_failure_separate_from_rollback_cleanup_failure` | |
| `core/D26` | merged into D27 (that is, D7) | — | — | — | number kept, not counted |
| `core/D27` | same as D7 | — | — | — | the design says "see D7"; number kept, not counted |
| `core/D28` | Resource ownership: resources registered through `Ctx` belong to the fiber automatically; a `Disposer` only releases early; a `Ctx` from isolate keeps the original fiber's ownership | core promise | partly verified | `rutis/tests/contract.rs::auto_ownership`, `::effect_yields_disposer`, `rutis/tests/event_keys.rs::keyed_listener_removed_with_owner_fiber` | "a `Ctx` from isolate keeps the original fiber's ownership" has no test (a plugin registering through isolate, its resources reclaimed on unload) |
| `core/D29` | Events are not filtered across isolate; isolate only isolates the service registry; scoped events need a sub-bus built by the user | contract behaviour | verified | `rutis/tests/event_order.rs::events_are_not_filtered_by_isolate` | |
| `core/D30` | A failing or panicking plugin neither hangs nor spreads: when apply fails or panics the plugin becomes Failed and what it registered is rolled back, and waiters get an explicit error; a listener panic reaches the ErrorSink; a cleanup panic is caught and wrapped as `PluginFailed`; a panic in a waterfall callback reaches the caller as it is | core promise | verified | `rutis/tests/contract.rs::apply_panics_complete_and_roll_back_current_thread`, `::apply_panics_complete_and_roll_back_multi_thread`, `::driver_panic_completes_waiters_with_error`, `::apply_failure_rolls_back`, `::emit_error_sink`, `::cleanup_panics_contained`, `::serial_panic_contained`, `rutis/tests/parity.rs::keeps_plugin_execution_failure_separate_from_rollback_cleanup_failure`, `rutis/tests/event_order.rs::async_waterfall_panic_reaches_the_caller`, `tests/e2e/tests/runtime_crash.rs::a_killed_runtime_fails_its_calls_and_a_restart_recovers` | covers Q3.1's "roll back on failure" and "interrupted calls fail explicitly" |
| `core/D31` | `emit` dispatch order: one event type dispatches serially in emission order (each of several emitters keeps its own order); a panic does not break the chain; a listener emitting the same type again goes to the tail without deadlock; **no order across event types**; listener completion order still not part of the contract | contract behaviour | verified | `rutis/tests/event_order.rs::emit_order_is_kept_per_emitter`, `::emit_after_listener_panic_is_still_delivered`, `::emit_inside_listener_goes_to_the_back`, `rutis/tests/dispatch_chain_probe.rs::dispatch_chain_does_not_fork_under_concurrent_emit`, `rutis/tests/event_keys.rs::keyed_emit_preserves_emission_order_per_name`, `rutis-agent/tests/order_probe.rs::emit_delivers_in_order_back_to_back_multi_thread` | `rutis-agent` is an example project, outside the test scope; listed for reference only |
| `core/D32` | Config hot update: `update(config)` while running; a dry run first (`validate_config` → `build` → instance `validate`), on failure an error and nothing stored or restarted, the old state keeps working; on success a restart per the state matrix; config is not part of dependency identity; **no `internal/update` waterfall, persistence belongs to the host layer** (D32d, explicitly not guaranteed) | core promise | verified | all 15 tests in `rutis/tests/config_update.rs`, e.g. `::update_active_applies_new_config_and_cleans_once`, `::update_dry_run_failure_leaves_state_untouched`, `::update_during_loading_converges_to_new_config`, `::concurrent_update_and_dispose_settle_correctly`; `rutis/tests/lifecycle_order.rs::an_update_dropped_after_its_cleanup_started_still_completes`, `::an_update_dropped_before_its_first_poll_changes_nothing` | |
| `core/D33` | Dynamic event keys: `TypeKey` qualified names accept runtime strings; a keyed surface on the event bus inherits the four modes, once / prepend and cleanup with the fiber; the same name with different types, and different names, do not cross; **events are still not filtered by isolate** (D33d, same as D29) | contract behaviour | verified | all 11 tests in `rutis/tests/event_keys.rs`, e.g. `::keyed_channels_are_isolated_by_name`, `::runtime_constructed_name_dispatches`, `::same_name_different_event_types_do_not_cross` | D33d is covered by the D29 test |

---

## 7. Core: `core/features.*`, `core/typed.*`

Sources: `core/features.errors` in [core-features](core-features.en.md) "Errors and panics"; `core/typed.*` in [design-typed-plugin-2026-10-02](design-typed-plugin-2026-10-02.en.md) "Dependency lost after gating".

| Number | Content | Level | Status | Verification | Notes |
| --- | --- | --- | --- | --- | --- |
| `core/features.errors` | Dropping the future that waits on `restart` / `update` does not stop a cleanup already started: the cleanup finishes and the plugin moves to its next generation | core promise | verified | `rutis/tests/lifecycle_order.rs::a_restart_dropped_after_its_cleanup_started_still_completes`, `::an_update_dropped_after_its_cleanup_started_still_completes`, `::a_restart_dropped_before_its_first_poll_starts_nothing`, `::an_update_dropped_before_its_first_poll_changes_nothing` | the last two also pin "dropped before its first poll, nothing happens" (the `restart` / `update` futures are lazy). That is not documented and is an implementation detail (Q3.3); if registration moves to call time, those two tests change with it |
| `core/typed.lost-dependency-pending` | When a required dependency fails after gating and before it is read (withdrawn, provider inactive, or `check` refusing), the plugin returns to Pending instead of Failed and loads again when the dependency is back; without a failed strict read in that generation it is a plain failure (no loops) | core promise | verified | `rutis/tests/typed_plugin.rs::a_dependency_lost_before_it_is_read_returns_the_plugin_to_pending`, `::a_dependency_back_before_the_failure_is_handled_still_reloads`, `::a_dependency_withdrawn_before_apply_starts_returns_the_plugin_to_pending`, `::a_check_rejected_after_the_gate_returns_the_plugin_to_pending_even_if_it_recovers`, `::the_claim_without_a_failed_read_is_a_plain_failure` | dependency gating (Q3.1) |
| `core/typed.lost-dependency-silent` | In that case the generation's `InjectUnsatisfied` does not reach the ErrorSink | contract behaviour | verified | `rutis/tests/typed_plugin.rs::a_lost_dependency_error_does_not_reach_the_error_sink` | |
| `core/typed.lost-dependency-restart` | A `restart()` falling in that window returns Ok, and the plugin goes back to Pending or loads again | contract behaviour | verified | `rutis/tests/typed_plugin.rs::a_restart_in_the_lost_dependency_window_returns_ok` | |
| `core/typed.get-not-evidence` | An untyped plugin that returns `InjectUnsatisfied` after a failed `get` gives no evidence: a plain failure, Failed | contract behaviour | verified | `rutis/tests/typed_plugin.rs::a_get_read_is_not_evidence_of_a_lost_dependency` | |
| `core/typed.untyped-require` | An untyped plugin that returns the error of a failed `require` returns to Pending and loads when a new provider appears | contract behaviour | verified | `rutis/tests/typed_plugin.rs::an_untyped_plugin_whose_require_fails_returns_to_pending` | |

---

## 8. Boundary rules for Cordis plugins: `boundary/5.1`–`boundary/5.8`

Source: [requirements-protocol-plugins](requirements-protocol-plugins.en.md) §5.

| Number | Content | Level | Status | Verification | Notes |
| --- | --- | --- | --- | --- | --- |
| `boundary/5.1` | Services: published and replaced through `ctx.provide` / property assignment; replacing directly with `ctx.set` sends no notification, and the rutis side switches to the new object after the next call to that service | contract behaviour | verified | `rutis-bridge/tests/service_projection.rs::projected_service_follows_replacement_and_withdrawal`, `rutis-bridge/tests/projection_lifecycle.rs::a_throwing_method_still_refreshes_the_slot`, `::a_call_on_an_exported_object_refreshes_the_slots`, `::withdrawal_and_immediate_reregistration_keep_the_service` | |
| `boundary/5.2` | Events across the boundary: `emit` is only a notification and does not guarantee the rutis side has run when it returns; use `parallel` / `serial` / `bail` to wait or get a result | contract behaviour | partly verified | `rutis-bridge/tests/event_forwarding.rs::cordis_events_reach_rutis_listeners`, `::rutis_events_reach_cordis_listeners`, `rutis-bridge/tests/boundary_rules.rs::a_cordis_serial_chain_takes_the_rutis_side_as_one_link` | `emit`, `parallel` and a Cordis-side `serial` are verified; a Cordis-side `bail` on a forwarded event degrades silently (#281, `boundary_rules.rs::a_cordis_bail_across_the_boundary_fails_or_behaves_natively` is `#[ignore]`) |
| `boundary/5.3` | Event order: native order only within one side; the other side's listeners run as a group at a fixed position; code relying on interleaved cross-side order is not supported | contract behaviour | partly verified | `rutis-bridge/tests/boundary_rules.rs::cordis_events_reach_rutis_listeners_as_one_group`, `::rutis_events_reach_cordis_listeners_as_one_group` | a rutis `serial` reaching the Cordis side does not run the Cordis group in native serial order (#281, `::a_rutis_serial_runs_the_cordis_group_in_serial_order` is `#[ignore]`) |
| `boundary/5.4` | Middleware chains: waterfall / bail chains do not interleave across sides; the other side's group takes part as one link | contract behaviour | partly verified | `rutis-bridge/tests/boundary_rules.rs::a_cordis_serial_chain_takes_the_rutis_side_as_one_link`, `::a_rutis_serial_chain_takes_the_cordis_side_as_one_link` | a synchronous Cordis-side `bail` / `waterfall` on a forwarded event degrades silently (#281, `::a_cordis_bail_across_the_boundary_fails_or_behaves_natively` and `::a_cordis_waterfall_across_the_boundary_fails_or_behaves_natively` are `#[ignore]`) |
| `boundary/5.5` | Framework-internal events `internal/*` do not cross the boundary | contract behaviour | partly verified | `rutis-bridge/tests/boundary_rules.rs::internal_events_stay_on_the_cordis_side`, `node/rutis-runtime/test/generate.test.mjs` (`internal events are not forwarded in either direction`) | only the bindings generator refuses; the runtime accepts a mount listing `internal/*` (#282, `::a_mount_listing_an_internal_event_fails` is `#[ignore]`) |
| `boundary/5.6` | Synchronous waits: a synchronous method that waits, while running, for a result needing the Node event loop to advance (timers, Promises) gets a `SyncWaitCycle` error instead of hanging | core promise | verified | `rutis-bridge/tests/rpc_callbacks.rs::node_sync_wait_pumps_callbacks_and_reports_its_executor_cycle` (the fixture `node/rutis-runtime/test/fixtures/rpc-client.mjs` asserts `SyncWaitCycle`), `::sync_await_reports_known_executor_cycle_but_allows_ready_future`, `::sync_wait_detects_an_async_callback_already_running_on_its_runtime` | what cannot cross a boundary fails explicitly (Q3.1) |
| `boundary/5.7` | Services provided by the host are proxies generated from the interface on the Cordis side: calls through the interface behave natively, `instanceof` does not hold | contract behaviour | verified | `rutis-bridge/tests/boundary_rules.rs::a_host_service_is_a_proxy_not_an_instance_of_the_declared_class` | |
| `boundary/5.8` | Event loop: plugins must not block it for long; the compatibility layer adds no timeouts to calls, and a rutis-side caller that needs one uses the async methods and sets it itself (timing out cancels), while the application decides how long unloading waits; an uncaught exception / rejection ends the whole Node process, and the services of that mount are withdrawn with it | core promise | partly verified | `rutis-bridge/tests/projection_lifecycle.rs::a_crashed_process_withdraws_its_services`, `rutis-bridge/tests/bun_runtime.rs::an_uncaught_error_ends_the_runtime`, `tests/e2e/tests/runtime_crash.rs::a_killed_runtime_fails_its_calls_and_a_restart_recovers` | withdrawal after the process exits is verified (Node via `process.exit`; a Python runtime killed with `kill -9`, through the rutis-host binary, S3); an uncaught exception ending the process is verified only on the Bun runtime; "no timeouts" is not asserted |

---

## 9. Instances: `instance/*`

Source: [design-instance-subtrees](design-instance-subtrees.en.md).

| Number | Content | Level | Status | Verification | Notes |
| --- | --- | --- | --- | --- | --- |
| `instance/subtrees.2.2` | An instance ID is assigned when a plugin is created and is unique in the process; restart / update / dependency reload of the same plugin keep it, a re-created child gets a new one; running out panics with a clear reason instead of wrapping; an old `Ctx` kept around still reports the old ID, and the old ID does not reach the new plugin's services | contract behaviour | verified | `rutis/src/key.rs::instance_id_exhaustion_panics_instead_of_wrapping`, `rutis/tests/instance_subtrees.rs::a_recreated_child_gets_a_new_instance_id`, `::fiber_instance_stays_stable_across_restart_update_and_dependency_reload` | the exhaustion panic happens while the root's admission lock is held and poisons it (unreachable with u64; recorded only, see #286) |
| `instance/subtrees.5` | Creating and shutting down subtrees over and over on a long-lived root (also next to a resident sibling) leaves nothing behind: plugins are released, internal entries and the task count return to baseline, parent table capacity does not grow with the rounds, and the resident sibling does not reload | core promise | verified | `rutis/tests/instance_subtrees.rs::thousand_subtrees_next_to_a_resident_leave_no_tasks_behind`, `::thousand_subtrees_leave_no_live_fibers`, `rutis/src/fiber/transient_tests.rs::parent_tables_shrink_back_after_every_batch_of_subtrees`, `::thousand_shutdowns_reclaim_all_root_side_records`, `rutis/tests/transient_release.rs::churn_on_long_lived_root_releases_instances` | nothing left behind (Q3.1). The consumer / listener list under a single key keeps its peak capacity (it does not grow with the rounds, so §4.6 holds; see #286) |

---

## 10. Sessions and links: `bridge/*`

Sources are given per row. "Runtimes" are the session implementations of each language: Rust (`rutis-bridge`), Node, Python, Bun, Go. The notes say which runtimes a row covers; whether the runtime matrix is complete is tracked in the [quality status](quality-status.en.md) (§3).

| Number | Source | Content | Level | Status | Verification | Notes |
| --- | --- | --- | --- | --- | --- | --- |
| `bridge/in-flight-call-fails-once` | [design-remote-plugins](design-remote-plugins-2026-10-03.en.md) §7; [requirements-protocol-plugins](requirements-protocol-plugins.en.md) §6 | When a link drops, a call in flight fails explicitly with a transport error (no success, no hang); the far end ran it once, and it is not sent again after reconnecting | core promise | verified | `rutis-bridge/tests/fault_links.rs::a_call_in_flight_when_the_link_drops_fails_and_ran_once`, `tests/e2e/tests/runtime_crash.rs::a_killed_runtime_fails_its_calls_and_a_restart_recovers` | interrupted calls fail explicitly, never report false success, never retry side effects (Q3.1); also when a local runtime process is killed with `kill -9` and its session ends with it, through the rutis-host binary (S3) |
| `bridge/no-call-on-a-dead-session` | [design-remote-plugins](design-remote-plugins-2026-10-03.en.md) §3.2, §7 | Calls made while a link is down or reconnecting either fail explicitly or reach the new session, never the old one; no `Peer` service is provided while it is down, and references from the old session keep failing | core promise | verified | `rutis-bridge/tests/fault_links.rs::calls_while_a_link_reconnects_fail_or_reach_the_new_session` | |
| `bridge/cancel-reaches-callee` | [design-remote-plugins](design-remote-plugins-2026-10-03.en.md) §6.2; [plugin API](guide/plugin-api.en.md) "Cancellation" | When the caller stops waiting, the cancellation reaches the callee; when the result is released, the release reaches it too and the callee drops the future | contract behaviour | verified | `rutis-bridge/tests/fault_links.rs::under_delay_a_cancellation_and_a_release_reach_the_callee`, `rutis-bridge/tests/cancellation.rs` | a Node callee receiving `AbortSignal` is covered by `cancellation.rs` |
| `bridge/multihop-cancel-and-release` | [design-remote-plugins](design-remote-plugins-2026-10-03.en.md) §6.2; quality standard Q6.2.7 | Across several hops (b → a → c), cancellation and release pass hop by hop | contract behaviour | verified | `rutis-bridge/tests/fault_links.rs::cancellation_and_release_pass_hop_by_hop` | |
| `bridge/call-chains-across-processes` | [session tags design](design-session-tags-2026-10-12.en.md) §5; [multi-language M1 design](design-multilang-m1-2026-10-04.en.md) §7.2; quality standard Q6.13.2 | When a call chain crosses any number of processes over links, there and back any number of times, each process restores only its own sessions' entries to native call ids; another process's call is never taken for one of this process's (it does not end a session, reach the wrong waiting thread or enter this process's wait graph) | contract behaviour | verified | `rutis-bridge/tests/session_tags.rs::a_call_chain_crossing_processes_keeps_each_process_s_calls_apart`, `tests/e2e/tests/migration.rs::an_implementation_moves_from_python_to_a_node_to_rust_while_the_host_runs` | #325. The first is two child processes, deterministic; the second goes through rutis-host and the fixture host (S4). Tag uniqueness is probabilistic (equal process nonces: about 10⁻¹⁵); between two processes of 0.9 or earlier #325 remains |
| `bridge/replies-match-calls` | [design-remote-plugins](design-remote-plugins-2026-10-03.en.md) §6.2 | With many calls in flight returning in any order, each reply reaches its own call | contract behaviour | verified | `rutis-bridge/tests/fault_links.rs::interleaved_replies_reach_their_own_calls` | release order shuffled by seed (`RUTIS_SEED`) |
| `bridge/large-and-deep-values` | [design-remote-plugins](design-remote-plugins-2026-10-03.en.md) §7 (message size limit); [plugin API](guide/plugin-api.en.md) "Passing values" | Values close to the message limit and values nested up to the limit (64 levels) arrive whole both ways; a call over the message limit fails explicitly and the far end does not run it; a value nested more than 64 levels deep is refused by the side that would send it, before sending, failing only that call (the error names the limit), and the session and other calls on it go on; a frame too deep from a far end that does not keep to the limit fails only the call it belongs to, or, when it shows no call, ends the session saying why | contract behaviour | verified | `rutis-bridge/tests/fault_links.rs::large_and_deep_values_cross_a_session_whole`, `::over_websocket_a_value_near_the_limit_passes_and_one_over_it_fails`, `::a_value_too_deep_fails_its_call_and_the_session_goes_on`; `rutis-bridge/tests/value_roundtrip.rs::rust_depth_limit`, `::node_depth_limit`, `::python_depth_limit`, `::go_depth_limit`, `::bun_depth_limit`; `rutis-bridge/src/session/rpc/tests.rs::a_value_deeper_than_the_limit_fails_its_call_before_it_is_sent`, `::a_frame_too_deep_fails_its_call_and_the_session_goes_on`, `::the_deepest_frame_allowed_parses_on_the_readers_stack` | #279. All 5 runtimes are verified as the sender of a value too deep (arguments and results; Node, Bun, Python and Go through their conformance fixtures' `wrap` and `applyWrapped`); only the Rust side implements receiving a frame too deep (the language runtimes open sessions only with the Rust host) |
| `bridge/value-roundtrip` | [plugin API](guide/plugin-api.en.md) "Passing values" | null, booleans, numbers, strings, arrays and objects round-trip across a session per the value table, except what `bridge/value-limits` lists | contract behaviour | verified | `rutis-bridge/tests/value_roundtrip.rs::rust_values_round_trip`, `::node_values_round_trip`, `::python_values_round_trip`, `::go_values_round_trip`, `::bun_values_round_trip` | property tests; a failure prints `RUTIS_PROPTEST_SEED`. All 5 runtimes; the Go column tests its session layer (it echoes `json.RawMessage`) |
| `bridge/value-limits` | [plugin API](guide/plugin-api.en.md) "Passing values" | **Explicitly not guaranteed**: integers beyond ±(2^53−1), `-0` and integral floats on the JS side; NaN, ±Infinity | explicitly not guaranteed | explicitly not guaranteed | the comparison rules in `rutis-bridge/tests/value_roundtrip.rs` relax for these limits | the tests only relax; they do not pin "really not provided". Nesting depth used to be in this row (#279); it is fixed and moved to `bridge/large-and-deep-values`: the limit is 64 levels, and a deeper value fails only its own call |
| `bridge/release-count` | [design-remote-plugins](design-remote-plugins-2026-10-03.en.md) §6.2; quality standard Q6.2.6 | After any sequence of grants, releases and hand-backs, both reference tables hold only unreleased references, and return to the start once all are released | contract behaviour | verified | `rutis-bridge/tests/value_roundtrip.rs::rust_releases_balance`, `::node_releases_balance`, `::python_releases_balance` | Rust, Node, Python; the Go and Bun fixtures have no way to observe the reference table, not covered |
| `bridge/reference-release-count` | quality standard Q6.2.6 | Releases are counted: a call already admitted is unaffected by a later release; an old release does not take from a new grant; a function exported again after full release gets a new id; repeated imports are released by count | contract behaviour | verified | `python/rutis/tests/test_peer.py::ReferenceCountTests` (`test_an_admitted_callback_survives_a_following_counted_release`, `test_an_old_release_preserves_a_new_grant_and_identifiers_are_not_reused`, `test_an_explicit_release_accounts_repeated_live_imports`); `node/rutis-runtime/test/session.test.mjs` (`admitted callback survives a following counted release before dispatch`, `old release preserves a new grant and identifiers are not reused`, `explicit release accounts repeated live imports without relying on GC`) | Node, Python; the Rust side is covered by `bridge/release-count`; no matching tests for Go, Bun |
| `bridge/grant-rollback` | [plugin API](guide/plugin-api.en.md) "Passing values" (values that cannot cross) | When arguments contain a value that cannot cross processes, the call fails explicitly, the reference grants already made are rolled back, and nothing is sent | core promise | verified | `python/rutis/tests/test_peer.py::ReferenceCountTests::test_an_unsupported_nested_value_fails_explicitly_and_rolls_back_partial_grants`; `node/rutis-runtime/test/session.test.mjs` (`unsupported nested references fail explicitly and roll back partial grants`) | what cannot cross a boundary fails explicitly instead of degrading silently (Q3.1). Node, Python; no matching tests for Rust, Go, Bun |
| `bridge/version-refusal` | [design-remote-plugins](design-remote-plugins-2026-10-03.en.md) §6.1; quality standard Q6.2.4 | Different session protocol versions are refused explicitly at handshake, saying which end must upgrade; each runtime's protocol constants equal Rust's | contract behaviour | partly verified | `python/rutis/tests/test_peer.py::VersionMismatchTests`; `node/rutis-runtime/test/session.test.mjs` (`a far end of protocol version 99 is refused, naming both versions`); `rutis-bridge/tests/sdk_protocol.rs` (4 tests: Python, Node, Bun, Go) | the refusal is verified; the message names both versions but not which end to upgrade (#293) |
| `bridge/websocket-refusal-categories` | [design-remote-plugins](design-remote-plugins-2026-10-03.en.md) §3.2 | WebSocket connection refusals are reported by structured category: wrong token, no token and untrusted CA as `AuthRejected`, a mismatched subprotocol version as `Incompatible` | contract behaviour | verified | `rutis-bridge/tests/websocket_cross.rs::rust_dialing_python_is_refused_by_category`, `::rust_dialing_node_is_refused_by_category`, `::node_dialing_rust_is_refused_by_category` | Python has no dialing side (it only listens), so there is no "Python dials Rust" column |
| `bridge/tls-server-verified` | [design-remote-plugins](design-remote-plugins-2026-10-03.en.md) §7 | The dialer verifies the server certificate (trusted CA, validity period, name); if any check fails the connection is refused as `AuthRejected`, and the token is neither sent nor shown in the reason | contract behaviour | verified | `rutis-bridge/tests/websocket.rs::wss_verifies_the_server_and_accepts_client_certificates`, `::wss_refuses_an_expired_or_not_yet_valid_server_certificate`, `::wss_refuses_a_server_certificate_for_another_name`; `python/rutis/tests/test_websocket.py::WssTest` | |
| `bridge/tls-client-certificate-verified` | [design-remote-plugins](design-remote-plugins-2026-10-03.en.md) §7 | The listener verifies the client certificate (CA, validity period); an invalid certificate does not authenticate even when its fingerprint is accepted, and the dialer gets `AuthRejected` | contract behaviour | partly verified | `rutis-bridge/tests/websocket.rs::wss_refuses_an_expired_client_certificate` | on Windows it is refused but reported as `Retryable`, so a link keeps retrying (#300); on Windows the test asserts only the refusal |
| `bridge/spawned-processes-own-channel` | quality standard Q6.5.2; #184 | Runtime processes started at once by one host each hold only their own channel; when one ends, its channel ends too, not held open by a descriptor another process inherited | core promise | verified | `rutis-bridge/tests/concurrent_launch.rs::spawned_processes_started_at_once_hold_only_their_own_channel`, `::node_runtimes_launched_at_once_hold_only_their_own_channel` | nothing left behind (Q3.1). Tested on Unix only: Windows hands no descriptor over |

---

## 11. Persisted formats: `host/rutis-json-format`, `dsh/user-layer-format`

Source: [design-quality-step2-2026-10-11](design-quality-step2-2026-10-11.md) §7; the `format` section of the user documentation [rutis-host](guide/rutis-host.en.md); the release process in "Persisted samples" of [release](release.en.md) (#207, risk J3).

| Number | Content | Level | Status | Verification | Notes |
| --- | --- | --- | --- | --- | --- |
| `host/rutis-json-format` | The top-level `format` of `rutis.json` is its format version, optional and read as 1 when absent; a newer one than this rutis-host supports is refused before any other field is read, saying to upgrade rutis-host; `rutis-host new` writes it in the files it generates; every released version's `rutis.json` (from 0.8.0) still reads with the meaning it had then | contract behaviour | verified | `rutis-host/tests/config.rs::the_format_is_checked`, `rutis-host/tests/errors.rs::a_configuration_that_is_not_there_or_not_valid`, `rutis-host/tests/persisted.rs::every_release_s_rutis_json_reads_as_it_did`, `::formats_no_longer_read_are_refused`, `rutis-host/src/new.rs::projects_are_created_with_their_names_filled_in` | samples and expectations in `tests/fixtures/persisted/<version>/`; the expectations of 0.8.0, 0.9.0 and 0.9.1 were produced by each tag's own parser |
| `dsh/user-layer-format` | The dsh user layer `<profile>/cordis.patch.yml` (a Cordis configuration format) has no format version; the user layer each released version wrote (from 0.8.0) still reads as the same patches | contract behaviour | verified | `rutis-dsh/tests/persisted.rs::every_release_s_user_layer_reads_as_it_did` | no version field (Cordis configuration is not versioned), so compatibility rests on the samples alone; an old format that no longer reads is not refused explicitly |

---

## 12. Plugin SDKs: `sdk/*`

Source: [plugin API](guide/plugin-api.en.md), quality standard Q6.7.

| Number | Content | Level | Status | Verification | Notes |
| --- | --- | --- | --- | --- | --- |
| `sdk/testing-matches-runtime` | The test tools (`@arcships/rutis/testing`, `rutis.testing`, strict mode) give the same results for calls on a plugin as a real runtime: sync, async, callbacks, errors crossing the boundary (name and message only), values copied or refused; known differences are bounded and written down ([plugin API](guide/plugin-api.en.md), "Where the test tool differs from a real runtime"; marked `allowed` in the expectations file) | contract behaviour | partly verified | `rutis-bridge/src/runtime/testing.rs::runtime` (run by `rutis-bridge/tests/session_matrix.rs` on every channel of Node, Python, Go and Bun), `node/rutis/test/conformance.test.mjs`, `python/rutis/tests/test_conformance.py::Conformance::test_the_testing_tool_returns_what_the_real_runtime_does` | Both sides are checked against one expectations file, `rutis-bridge/src/runtime/testing/weather.expected.json` (Q6.7.2). Go's `rutistest` and the Bun test tool are not yet compared; Maps, Sets, Dates, binary data and bigints are refused under the current rules, pending #308 |

---

## 13. Counts (baseline: `main` after `121eded` with #284 and #298 merged)

| Scope | Verified | Partly verified | Not verified | Explicitly not guaranteed | Total |
| --- | --- | --- | --- | --- | --- |
| `core/D*` (without D20, D26, D27) | 26 | 3 | 0 | 1 | 30 |
| `core/features.*`, `core/typed.*` | 6 | 0 | 0 | 0 | 6 |
| `boundary/5.*` | 3 | 5 | 0 | 0 | 8 |
| `instance/*` | 2 | 0 | 0 | 0 | 2 |
| `bridge/*` | 13 | 3 | 0 | 1 | 17 |
| `host/*`, `dsh/*` (§11) | 2 | 0 | 0 | 0 | 2 |
| `sdk/*` (§12) | 0 | 1 | 0 | 0 | 1 |
| Total | 52 | 12 | 0 | 2 | 66 |

17 core promises: 14 verified, 3 partly verified (D22, D28, 5.8), 0 not verified. 47 contract behaviours: 38 verified, 9 partly verified (D5, 5.2, 5.3, 5.4, 5.5, `bridge/large-and-deep-values`, `bridge/version-refusal`, `bridge/tls-client-certificate-verified`, `sdk/testing-matches-runtime`), 0 not verified.

**0.10 baseline**: 0 unverified core promises, 0 unverified contract behaviours. From 0.11 on, a newly registered row (`loader/`, `host/`, …) that is "not verified" would push the contract number above the baseline (Q9.4.4), so new rows should be registered together with their tests. The maintainer confirms which rows are core promises and may change them.
