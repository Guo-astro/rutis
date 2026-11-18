# Service announcement policy at the host boundary

[中文](design-service-announcement-policy-2026-11-08.md)

Status: superseded, implemented as a host option. Date: 2026-11-08 (superseded 2026-11-18).
Issue: [#1040](https://github.com/arcships/rutis/issues/1040).

Decision note: superseded on 2026-11-18. #1040 is implemented as a host option, `SpawnPlugin::undeclared` / `Session::undeclared` / `Mount::undeclared` with `UndeclaredPolicy::{Publish, Refuse, Allow}`; the default, `Publish`, is Cordis's behaviour. A refused name is not published, and its row fails naming the name and the row; there is no `HostStatus.announcements` view, and peer kernel imports are not covered (their plugins' declarations are not known here). [Build a host open to plugins](guide/open-host.en.md#3-keep-plugins-in-check) and the [guarantees](guarantees.en.md) row `loader/announced-services` are the contract. The rest of this document is the earlier deferred proposal, kept as it was.

The earlier note follows: existing Cordis-compatible dynamic announcements remain unchanged; this proposal's host-side refusal policy was shelved without a concrete requirement.

## 1. Where things stand / what to solve

A runtime row can provide a service name that is not in its declared `provides` set. `rows.announce` currently follows every such name and publishes it with the host-wide `host_key(name)`. A package without `rutis.provides` can therefore announce `llm`, `storage` or `http`; the first provider wins and a less-trusted row can occupy a name before the intended provider starts. A separate process contains crashes, but it does not create a service namespace.

The relevant paths are `row_projection_of` (`crates/rutis-bridge/src/runtime/rows.rs`), runtime-row loading (`crates/rutis-loader/src/runtime.rs`), `Process::announce_row_service` (`crates/rutis-bridge/src/runtime/process.rs`), and peer import (`crates/rutis-bridge/src/session/plugin/services.rs`). `rutis-host check` cannot know a future dynamic name. The existing `announces` capability says that a runtime can send dynamic announcements; it is not host authorization. The existing loader `Status` and connection-row state are separate; the design therefore proposes a host-level `AnnouncementDiagnostics` store and a new `HostStatus` aggregate projection rather than pretending peer refusals fit in loader-only status. `HostStatus` is a proposed design type, not the current `Shown` status type.

## 2. Decisions

| Decision | Choice | Why; what is dropped |
| --- | --- | --- |
| D1 | Add a host-side `undeclared` policy with exactly `"publish"` and `"refuse"`; default `"publish"`. | `publish` preserves the 0.21 behaviour. `isolate`, `isolate: "*"`, an allowlist and an implicit runtime namespace are not part of this deferred proposal: their scope and visibility are undefined. |
| D2 | Configure the receiving process/connection boundary through the embedding API: `SpawnPlugin::undeclared(UndeclaredPolicy::...)`. One boundary policy covers all child runtime rows; per-row overrides are deferred. `SessionOptions` remains transport limits. A `rutis.json` row field and language-runtime default are deferred. | Threading a row field through Desired/RowInfo/reconcile and all deny-unknown runtime schemas is a separate configuration change; silently documenting a JSON field that is not accepted is dropped. |
| D3 | The receiving host applies the policy to local runtime announcements, connection-boundary announcements and peer `services.announce`. `publish` keeps the current host-wide key and first-provider-wins rule. `refuse` never creates a host slot. | Sending-side flags cannot decide the receiver's trust boundary. A policy only in the sender would miss peer and connection-boundary imports. |
| D4 | In the deferred proposal, `refuse` is a release-and-report decision. The receiver would release the announced object before `provide_at`, avoid creating or replacing a slot, and record a structured host diagnostic. | A typed wire rejection is not specified here: Node, Bun, Python and Go all wait for announcement completion on the load-success path (Node `node/rutis-runtime/src/runner.mjs:706-715`; Bun `bun/rutis-bun/src/runner.ts:424-429`; Python `python/rutis/rutis/runner.py:676-689`; Go `go/rutis/internal/runtime/runtime.go:559-570`). Node/Bun attach `.catch(() => {})` to the announcement call itself (Node `node/rutis-runtime/src/runner.mjs:268-274`; Bun `bun/rutis-bun/src/runner.ts:289-298`), so that announcement rejection is swallowed before the row awaits completion. Python/Go await pending announcements and only propagate failures explicitly marked as registration refusals. A uniform terminal/no-restart row state would require a separate SDK and loader protocol design. |
| D5 | Add code `undeclared-service`, data `{service, row, policy: "refuse"}` and a bounded host-level `HostStatus.announcements` view. Here `row` is the receiving SpawnPlugin/connection boundary identifier (for example `thirdparty`), not one child runtime row. Each record also carries an opaque boundary generation for deduplication and clearing. Local loader status contributes its records; connection/peer sessions contribute theirs; `rutis-host status` presents the merged view. The exact line is `thirdparty: refused undeclared service llm (policy: refuse)`. | Reusing `already-provided` would make a policy refusal look like a provider collision. An unbounded log or a loader-only field that hides peer refusals is dropped. |

## 3. Precise rules

1. `undeclared` is host authorization, not a manifest field. `announces: true` still only says that a runtime may send a dynamic service; it does not authorize publication.
2. In this deferred proposal, the policy would be set on the receiving `SpawnPlugin`/connection boundary. One boundary policy would cover all child runtime rows; per-row overrides are not part of the proposed API. If unset it would be `publish`. The proposal does not add a `rutis.json` field, `SessionOptions` field or global allowlist. Invalid API input would fail before the boundary starts with: `config: undeclared must be "publish" or "refuse"` (code `bad-config`); no boundary identifier would exist yet.
3. Declared services are unaffected. Their existing catalog/instance/`isolate` keys and declaration/use rules remain unchanged. Only the first announcement of a name absent from the originating child row's declared set is evaluated:
   - `publish`: accept and publish under the current key (normally `host_key(name)`);
   - `refuse`: release the object exactly once before `provide_at`, record one diagnostic and do not create or replace a host slot.
4. The diagnostic line is exactly `thirdparty: refused undeclared service llm (policy: refuse)`. Its code is `undeclared-service`; its data is `{ "service": "llm", "row": "thirdparty", "policy": "refuse" }`, where `row` is the receiving boundary identifier.
5. `HostStatus.announcements` contains at most one record for a `(row, boundary-generation, service, policy)` tuple. Here `row` is the receiving boundary identifier. A duplicate announcement in that boundary generation is deduplicated. Records are removed when the boundary unloads or disconnects; a new generation starts with an empty set. The proposed `rutis-host status` and status socket would serialize these records after row/service state. When there are no records, the proposed text is exactly `announcements: none`; otherwise the text block is `announcements:` followed by two-space-indented payload lines. The proposed status-socket JSON has an `announcements` array. Each record would expose `row: String`, `service: String`, `policy: UndeclaredPolicy` (serialized as `publish` or `refuse`), `code: String`, `data: serde_json::Value`, and an opaque `generation: u64`; `generation` is for deduplication/clearing only. Local loader records and connection/peer records would be merged into this host-level view by `AnnouncementDiagnostics`. The human line is the exact line in rule 4; code/data would be available to embedding callers. This is a proposed bounded diagnostic view, not a current status field or an unbounded audit log.
6. A refused object is released exactly once. Later withdrawal is idempotent and cannot create a slot. An absent or intentionally filtered observer keeps the existing ignore-and-release result and creates no policy diagnostic; policy refusal is a separate internal decision.
7. A connection boundary uses the same receive-side policy for runtime announcements. A peer session checks its receiver's policy before `ctx.provide_at(host_key(name), ...)`; under `refuse`, no import is inserted and the wrapper is disposed. The peer announcement call still completes as it does today, so an old or swallowing sender may continue running. A local provider is never failed by this policy.
8. An ordinary duplicate is unchanged. Existing tests remain literal, for example `node-late: weather is already provided by py-early: rename one of them` and `store is already provided by the host: rename it`, code `already-provided`, data `{service, provider}`. A peer-import collision keeps its existing session text `far:plugin: name is provided here by holder: not registered from far` and the existing conflict/restart handling; it is not rewritten as `undeclared-service`. The later provider fails; the earlier provider and its users continue.
9. `rutis-host check` cannot discover a future dynamic name. It validates only known configuration and continues to warn that a dynamic service may appear at runtime. It does not promise to preflight a future refusal.
10. `isolate: "*"`, implicit runtime namespaces, group/instance isolation for dynamic names, a global allowlist and a row field in `rutis.json` are out of scope. This design does not alter `inject` or declared-service isolation.
11. This is an admission boundary, not a process sandbox and not a guarantee that code cannot use services already leased to its row. A host needing stronger separation must use separate runtimes and `refuse`; `publish` must never be documented as isolation.
12. A future protocol design may add a structured rejection and terminal row state. It must update Node/Bun (`node/rutis-runtime/src/runner.mjs`, `bun/rutis-bun/src/runner.ts`), Python, Go, the peer target and loader restart handling before row failure can be promised.

## 4. What developers see

### Plugin authors

With the default, existing code is unchanged. A plugin may call `ctx.provide('metrics', service)` without `rutis.provides`; the first announcement is published under host-wide `metrics`.

If this proposal were implemented and a host configured through `SpawnPlugin::undeclared(UndeclaredPolicy::Refuse)`, the object would be released and the proposed `announcements` part of `rutis-host status` would show exactly:

```
announcements:
  thirdparty: refused undeclared service llm (policy: refuse)
```

The record payload (without the heading and two-space indentation) is `thirdparty: refused undeclared service llm (policy: refuse)`.

The proposed status record would have code `undeclared-service` and data `{ "service": "llm", "row": "thirdparty", "policy": "refuse" }`, associated with the current `thirdparty` boundary generation if the proposal is implemented. The boundary and its child rows would not be failed or restarted by this policy. Fix: declare `llm` in `rutis.provides`, or choose `publish` for this receiving boundary. This is a proposed host diagnostic, not a plugin exception.

A real collision remains a different error: `store is already provided by the host: rename it` (code `already-provided`); fix the service name or its declared `isolate` configuration.

Proposed concepts: one receiving-host policy, one diagnostic code and one bounded status diagnostic view. `announces` keeps its existing capability meaning.

### People assembling plugins with `rutis-host`

This deferred proposal does not add an `undeclared` field to `rutis.json`; if revisited, its configuration location must be decided by a new design. Existing configuration and Cordis-compatible dynamic announcement output remain unchanged.

Errors and records:

| Cause | Exact output and code | Recorded / fix |
| --- | --- | --- |
| Invalid API policy (proposed) | `config: undeclared must be "publish" or "refuse"` / `bad-config` | The proposed boundary builder, before an identifier exists; use `publish` or `refuse`. |
| `llm` announced under proposed `refuse` | `thirdparty: refused undeclared service llm (policy: refuse)` / `undeclared-service`, data `{ "service": "llm", "row": "thirdparty", "policy": "refuse" }` | The proposed `HostStatus.announcements` for the current boundary generation; declare `llm` or choose `publish`. |
| Duplicate published name | `store is already provided by the host: rename it` / `already-provided` | The later provider's existing failure; rename it or use declared `isolate`. |
| Peer import collision | `far:plugin: name is provided here by holder: not registered from far` / existing session conflict | The existing peer conflict/restart record; rename the service or resolve the existing provider. It is not `undeclared-service`. |

### Host owners embedding rutis

If implemented, `SpawnPlugin::undeclared(UndeclaredPolicy::...)` would supply the receiving policy for a local process or connection boundary. `SessionOptions` would remain transport limits. The receiver would apply it to local runtime rows and peer imports; the sender could not override it. With `publish`, current `host_key(name)` and first-provider-wins semantics would remain. With `refuse`, the object/wrapper would be released, the diagnostic would be stored in the proposed `HostStatus.announcements`, and no local provider would be replaced.

If this proposal is ever implemented, the host would receive the exact line and structured code/data through status. It must not relabel it `unavailable` or `already-provided`, or claim that the remote row failed or restarted. These are deferred semantics, not a 0.22 promise.

Proposed concepts: one host admission policy, one diagnostic code and one bounded status field; no process or protocol namespace concept.

### People deploying

A separate process is crash/hang isolation, not service-name isolation. With default `publish`, an undeclared service is host-wide and first provider wins. For a less-trusted process/connection boundary, configure `SpawnPlugin::undeclared(UndeclaredPolicy::Refuse)`, then declare intended public services. If implemented, deployment would see the refusal line in the proposed status view; it is not a silent private fallback and would not restart the row.

Concept budget: this deferred proposal would add one deployment rule—choose `publish` or `refuse` at the receiving process/connection boundary. `isolate` is not part of the proposal.

## 5. Compatibility and migration

- The default is `publish`; existing rows, Cordis packages and peers keep host-wide behaviour and duplicate-provider output.
- `refuse` is opt-in through the embedding API only. No existing `rutis.json` is made invalid or silently reinterpreted.
- If the proposal is revisited, wire compatibility and announcement completion require a new review. No receiver-side refusal behavior is added in 0.22; existing Cordis-compatible dynamic announcements remain unchanged.
- If implemented, the proposed `HostStatus.announcements` and its status-socket serialization would be additive. Records would be bounded to the current boundary generation and would not be a durable audit log.
- `already-provided` text/code/data, `announces`, `inject`, declared-service keys and `isolate` semantics do not change. No `allowed_services` API returns.
- A future typed rejection/terminal-row design requires a new document and migration review; it is not implied by this policy.

## 6. If this proposal is reopened

No implementation work is planned. If a concrete requirement reopens #1040, first decide the owning layer and then split policy, diagnostics, bridge, test and documentation work. Any implementation must revisit the exact output, wire behavior, status lifecycle and SDK handling; no part of this proposal is required for 0.22.

## 7. Design-philosophy check for a possible future proposal

If this topic is reopened, the policy must be assigned to the correct layer and keep the extension point narrow. Dynamic services remain compatible and name-based; no language-specific permission hook or broad allowlist should be introduced without a concrete requirement. The JSON/API boundary must be explicit rather than silently pretending to support a field. This review is a prerequisite for future work, not an implementation decision for 0.22.
