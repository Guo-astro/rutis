# rutis 0.22 design and delivery roadmap

[中文](roadmap-0.22-2026-11-08.md)

Status: design baseline, not implemented. Date: 2026-11-08; revised 2026-11-09: the 0.23 formal coverage is merged into 0.22.
Tracking: [#694](https://github.com/arcships/rutis/issues/694) (release planning), [#1025](https://github.com/arcships/rutis/issues/1025) (event close admission CPU) and [#183](https://github.com/arcships/rutis/issues/183) (quality control). Issues #613, #622, #624, #630, #631, #634, #635, #639 and #776 from the 0.23 milestone join #1025 in 0.22's release scope; the execution plan is the [formal-coverage execution plan](plan/formal-verification-0.22.en.md). The [formal-verification redesign](design-formal-verification-2026-11-08.en.md) is the verification baseline; #1040 remains explicitly deferred.

## 1. Purpose and current state

0.22 starts from the `v0.21.0` tag and has no release branch or implementation yet. This document freezes the design scope before code work begins. 0.23 is merged into 0.22, so the scope is no longer split across two releases:

1. Remove the ancestor walk from event-listener liveness checks while preserving the existing close/admission boundary ([event close design](design-event-close-admission-2026-11-08.en.md)).
2. Complete the four-layer formal coverage: a current session protocol specification and a rule-to-evidence tracker, design models for events and lifecycle, controlled scheduling for the Rust kernel, real-execution trace and cross-language conformance, controlled scheduling for non-Rust runtimes, property/fuzzing/Kani/Miri, virtual clocks and ordinary tests ([formal-coverage execution plan](plan/formal-verification-0.22.en.md)).

Dynamic service announcements remain as the existing Cordis-compatibility behavior. The proposed host-side refusal policy remains deferred: defense against untrusted plugins does not belong in this layer without a concrete requirement.

The current workspace package declaration is `version = "0.6.0"` in `Cargo.toml`; release work must first confirm whether this is an intentional workspace convention or a stale declaration. Do not change it as part of either feature until the release procedure is decided.

## 2. Release promise

0.22 promises:

- lower event-listener admission CPU on the `close_cpu` workload without changing public event, shutdown, cancellation or protocol behavior;
- fold the 0.23 formal coverage into this release: a current session protocol specification, a rule-to-evidence tracker, and the four-layer verification completed with reviewable results as laid out in the [execution plan](plan/formal-verification-0.22.en.md);
- bilingual documentation, migration notes and release notes that state the exact visible output.

0.22 does **not** add a service namespace per process, a global allowlist, `rutis.json` configuration for undeclared announcements, dynamic-name `isolate`, or a cross-language terminal “policy refused” row state. Existing Cordis-compatible dynamic announcements keep their current behavior; #1040 remains outside 0.22.

## 3. Workstreams and gates

### W0 — Verification baseline and execution plan (#183, #776, #634, #635)

0.22 no longer leaves formal coverage to a later release. First write the current session protocol specification and the rule list, link the entries in `docs/guarantees.md` into the tracker, and reconcile `specs/cross-runtime-sync`, `specs/remote-lease` and the three shuttle models against the current specification; an old-architecture model is not proof of the new implementation. The execution plan and gaps are in the [formal-coverage execution plan](plan/formal-verification-0.22.en.md).

Gate: the protocol specification covers frames, handshake, calls, references, cancellation, malformed frames, unknown fields and size limits; the tracker lists evidence and gaps per rule; every old model is marked current, superseded or explicitly not guaranteed; the #1025 close/admission model, counterexamples and deterministic race test exist first.

### W1 — Event close admission (#1025)

Design and implementation follow the linked design document:

- Add permanent `FiberInner::lineage_closed` markers.
- Under `Shared.admission`, mark the complete fixed subtree first; publish existing `closing` flags only after the marker pass; then perform cleanup/shutdown.
- Make `Entry::live()` read owner marker state instead of walking ancestors. Keep `CallFlight::begin(owner)` and the final `live()` check in `Entry::start()`.
- Keep registration checks, instance-event permits, callback ordering, cancellation, shutdown output and cross-process behavior unchanged.

Gate: deterministic ancestor-close races and existing sync/async dispatch modes show that no new callback is admitted after the marker commit; an admitted callback may begin after the commit but still drains. The issue's `creation <= 30s` is a performance target to record, not an absolute-time assertion in the current `close_cpu` test; the automated gate continues to compare `CREATE_RATIO`, `CLOSE_RATIO` and `IDLE_SHARE` without regressing queued-emits memory.

The CI acceptance job is the Ubuntu 24.04 release job in `.github/workflows/stress.yml:381-417`; it runs `close_cpu` and `queued_emits_memory`, and both results must remain in the summary.

### W2 — Four-layer formal coverage (#613, #622, #624, #630, #631, #635, #639)

With 0.23 merged in, W2 is 0.22's complete coverage work, advanced layer by layer:

- **A — protocol foundations:** the current session protocol specification; frame, error, reference, cancellation and limit rules; cross-implementation conformance cases.
- **B — concurrency and lifecycle:** the shuttle model and deterministic tests for event close admission; TLA+ models for events, cross-runtime calls, row lifecycle, call lifecycle and node-hosted rows.
- **C — cross-process and deployment:** real-execution trace against the models; leases, host-gone, sync-cycles, AccessEvent and the applicable cross-language columns.
- **D — inputs and long-run reliability:** property tests, fuzzing, Kani/Miri, virtual clocks, missing conformance and ordinary tests, and resource trends after repeated create/close/reload/reconnect.

Gate: per §10 of the [formal-verification redesign](design-formal-verification-2026-11-08.en.md) and §2 of the [execution plan](plan/formal-verification-0.22.en.md), every applicable rule has a current source, implementation evidence, applicability matrix and status; model, trace, conformance, property/fuzz, virtual-clock, platform and long-run results all exist; deliberately injected errors are caught by the corresponding evidence.

### W3 — Release, migration and documentation (#694)

Before implementation is called complete:

1. Decide and document the package/version update procedure, including the `Cargo.toml` workspace declaration and the `v0.21.0` tag.
2. Add `migration-0.22.md` and `migration-0.22.en.md`; state that dynamic announcement behavior is unchanged and that the event optimization has no API migration.
3. Add `docs/releases/0.22.0.md` and `.en.md`, with measured benchmark values only after CI runs.
4. Update the event, host and plugin guidance plus `release.md` and `release.en.md` in both languages.
5. Keep this roadmap and the event-close design synchronized; #1040 remains deferred and must not add a JSON field or wire-visible behavior to 0.22.
6. If a future trust-boundary design is reopened, write a new design document with a concrete requirement and layer ownership before implementation.

Gate: bilingual review finds matching decisions, exact output/error/code/data, role-specific “What developers see”, migration instructions and acceptance criteria.

## 4. Order of work

1. **Baseline:** verify the tag, package-version convention, current status serialization and benchmark environment; record measurements before changing code.
2. **Verification baseline:** write the current session protocol specification and the tracker skeleton; reconcile the existing TLA+/shuttle models; write the #1025 close/admission model, counterexamples and deterministic race test first.
3. **Contracts:** land the event-close design, this roadmap and the [execution plan](plan/formal-verification-0.22.en.md); split core, race-test, benchmark and documentation tasks under #1025. Keep #1040 out of the 0.22 implementation plan.
4. **Core change:** implement the marker publication independently and keep it revertible.
5. **Four-layer coverage:** advance the protocol specification, TLA+/shuttle models, trace validation, non-Rust controlled scheduling, property/fuzzing/Kani/Miri, virtual clocks and conformance cases in parallel per the execution plan.
6. **Acceptance:** run focused event race tests, `cargo test -p rutis-loader --release --test close_cpu -- --nocapture`, and the CI stress pair; record model, implementation-test and benchmark results separately.
7. **Release preparation:** update migration/release documents from the tracker and measured results, resolve package-version policy, then perform the normal release review.

No release commit, tag, package-version change or public API implementation is part of this design-only checkpoint.

## 5. What developers see in 0.22

### Plugin authors

Existing Cordis-compatible dynamic announcement behavior remains unchanged. The event-close optimization has no new plugin API, output or error.

### Host owners and deployers

There is no new trust-boundary setting in 0.22. The event-close optimization has no new output, status field, protocol version or migration setting; existing status lines and failure text remain unchanged.

### Rust host authors

Public Rust APIs, the event protocol and shutdown results are unchanged. Closed registration/effect/provide-style operations still return `ErrorKind::Closed`, with the text `root has been shut down` and no new `code` or `data` (`crates/rutis/tests/instances/instance_subtrees.rs:291-325`). The event-close design proposes that `emit()` itself not fail merely because a subtree is closed, but this delivery result is not a 0.22 guarantee: the existing service-after-stop test leaves delivery unspecified (`crates/rutis/src/events/api.rs:453-480`; `crates/rutis/tests/instances/service_emits.rs:70-83`).

### Runtime authors and contributors

This roadmap adds no plugin-user-visible runtime command, exit code, success output or failure output; existing model, shuttle and CI output remains unchanged. What W2 adds is contributor-facing specifications, models, trace and tests; they are not runtime output.

## 6. Explicit follow-ups outside 0.22 (0.23 is merged in and is no longer listed separately)

- Any host-side refusal policy or trust-boundary defense for dynamic announcements; #1040 is deferred because this layer is not the right owner without a concrete requirement.
- A service namespace, global allowlist, `rutis.json` row-level policy or dynamic-name isolation model.
- Typed wire rejection and a terminal policy-refused row state across all SDKs and loader restart handling.
- New memory-order schemes beyond the initial `SeqCst` marker implementation.
- Any package-version change made without first reconciling the repository's release convention.

These are separate designs, not implied by the 0.22 promise.
