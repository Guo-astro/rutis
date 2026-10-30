# A runtime row's own `inject`: rutis gates and leases it

[中文](design-row-inject-gate-2026-10-31.md)

Status: implemented (branch `claude/row-inject-gated`, not merged). Date: 2026-10-31. Base: the follow-up [declared services](design-declared-services-2026-10-29.en.md) §7 records, on the commit that design lands in. The code §1 describes is that of the parent of the commit this document lands in.
Issue: none yet, opened with this document. Related: [declared services](design-declared-services-2026-10-29.en.md), [optional dependencies](design-optional-deps-2026-10-28.en.md).

## 1. Where things stand

A Rust row's own `inject` joins the fiber's injects through `combined_injects` (`crates/rutis-loader/src/loader/reconcile.rs`), and the fiber starts once they are all there. A runtime's row (Node, Bun, Python, Go, Swift, Kotlin) goes another way:

```
Inner starts a row (reconcile.rs)
├─ ① Row::foreign_scope (desired.rs)      → BoundScope.inject is empty
├─ ② combined_injects(factory, scope)     → only what the plugin declares (JsFactory's gated)
└─ ③ JsRow::start (runtime.rs)            → leases only gated; the row's inject reaches the runtime only as rows.load's fifth argument
```

What follows depends on the runtime:

| Runtime | A service in the row's `inject` that the host or another process provides, not yet in the process |
| --- | --- |
| Node | The row shows as running in rutis but waits at the Cordis gate inside the process; `apply` does not run, and it waits for ever |
| Bun, Python, Go, Swift, Kotlin | Not waited for: `apply` runs, and `use` says `service <name> is not available` |

`Loader::check` (`rutis-host check`) has long reported by another rule: a runtime's row waits in rutis for the names of its own `inject` the service catalog shares (the row's `inject` in `Loader::check`, `crates/rutis-loader/src/loader/check.rs`, since `0606ace0`). What `check` says and what runs differ.

## 2. Decisions

| # | Decision | Why; what was dropped |
| --- | --- | --- |
| D1 | A runtime's row is gated in rutis on the names of its own `inject` the service catalog shares: `Row::foreign_scope` gives their keys, and `combined_injects` adds them to the fiber's injects | As for a Rust row (`combined_injects`), and as `check` already reports. The key is the one the row leases it under (`ServiceCatalog::shared_key_in`), inside instances the instance's own. Dropped: fixing it in the SDKs alone — an SDK process cannot reach a service rutis did not lease into it |
| D2 | At start these names are leased into the runtime as the plugin's `inject` is: one a row of the same process provides is used natively; a name the row itself isolates is leased in that isolate (a group's isolate: §6) | The gate only makes sure the service is in rutis; the row uses it once it is leased into the process. The same leasing code as the plugin's `inject` |
| D3 | A name the catalog does not share (one the application registered for a Rust type) is neither gated on nor leased, and reaches the runtime only through `rows.load` as before, for it to resolve natively (a Node row's Cordis gate waits for such a name in the process, for ever if no row of that process provides it, as before this design) | As `check` counts it; such a name is no `dyn HostDispatch` and cannot cross languages. Note: a leaf runtime's plugin `inject` gates on such a name too (`split_dependencies`'s `leaf \|\| is_shared`, waiting for `host_key(name)`), a row's `inject` does not; the two differ, and this design follows `check` |
| D4 | The Cordis gate inside the Node process stays | What it waits for is now leased in by rutis, so it passes; a service a row of the same process provides still resolves natively |

## 3. What developers see

### Plugin authors, and those assembling plugins with `rutis-host`

The row writes `inject`; the plugin declares nothing:

```json
{ "id": "chat", "name": "./chat.ts", "inject": ["llm"] }
```

```ts
export default definePlugin({
  apply(ctx) {
    ctx.use<Llm>('llm').ask('hi')     // llm of the row's inject: only once it is there does this run
  },
})
```

When it works: with no `llm` in the host or another row yet, `rutis-host status` prints `chat: waiting for llm (no row provides it)`, in all six languages, as `rutis-host check` says. `llm` comes: the row starts and its plugin has it ([declared services](design-declared-services-2026-10-29.en.md) D1). `llm` goes: the row stops; back, it starts again, as for a name in the plugin's `inject`. A row given an `inject` restarts once. Other languages differ only in the plugin's code (`ctx.use`/`Use`); configuration and output are the same.

When it fails:

| # | Cause | Output (word for word) | Code | Fix |
| --- | --- | --- | --- | --- |
| 1 | The row's `inject` has `llm`; no row provides it, nor the host | `chat: waiting for llm (no row provides it)`; with a similar name, `; did you mean <name>?` inside the parentheses; with disabled rows, see the waiting reasons of [rutis-host](guide/rutis-host.en.md) | the row waits, it does not fail | have a row or the host provide `llm`, or take it out of the row's `inject` |
| 2 | The row's `inject` has `clock`, which the application registered for a Rust type (not shared by the catalog, D3) | Bun, Python, Go, Swift, Kotlin: the row starts, and `use('clock')` says `service clock is not available`; Node: running in rutis, the Cordis gate in its process waits for `clock`, `apply` does not run, no output | `unavailable` (Node: none) | such a name does not cross languages: take it out of the runtime row's `inject` and use it in a Rust row |

### Host owners

A resolver of your own using `Resolved::with_foreign_scope` has its rows wait for the shared names of their own `inject` too (§4, third change).

| # | Cause | Output (word for word) | Code | Fix |
| --- | --- | --- | --- | --- |
| 3 | A custom resolver's plugin finds a shared name of its row's `inject` inside its own runtime and never has it announced to rutis | `<row id>: waiting for <name> (no row provides it)`, for ever | the row waits, it does not fail | announce the service to rutis (as `rows.announce` does), or leave it out of the row's `inject` |

### New concepts

None. No new configuration; the only new output is `waiting for <name>` for runtime rows too, the same line as for a Rust row.

## 4. Compatibility

Three visible changes, the first two as for a Rust row:

- Bun, Python, Go, Swift and Kotlin rows: with a service of the row's `inject` missing, a row used to start and `use` said `unavailable`; now it waits, `waiting for <name>`.
- When the provider withdraws, those rows used to keep running; now they stop until it is back.
- A custom resolver using `Resolved::with_foreign_scope`: its rows are gated on the shared names of their own `inject` too (as `check` has long reported). Leasing those into the runtime is the plugin's (this repository's runtimes lease through `JsRow::start`); a plugin that finds such a name inside its own runtime and never has it announced to rutis has a row that waits for ever. `Resolved::foreign_scope`'s doc comment is rewritten to say so. In this repository it is set only by the resolutions of runtime rows: `leaf_resolved_on`, `SessionResolver`'s `offline` while a session is not connected (the row waits for the session anyway), and `unavailable_resolved` when the runtime has no such plugin (it fails in `validate_config`, before any gate); the rows of the first two are leased by `JsRow::start`.

A Node row used to show as running in rutis while it waited at the gate inside its process; now rutis shows it waiting. The protocol does not change, and `PROTOCOL` stays.

## 5. Implementation

| Step | Content | Done means |
| --- | --- | --- |
| 1 | `Row::foreign_scope` in `desired.rs`: `inject` takes the names the catalog shares, keyed with `shared_key_in`, sorted by name and deduplicated | `combined_injects` gives one order for one configuration, so order alone restarts nothing |
| 2 | `runtime.rs`: `JsFactory` and `JsRow` get `row_inject` (name → the key it is leased under, none when not shared), which `JsModule::factory` builds from the catalog; `JsRow::start` leases the shared names of the row's `inject` not in `gated` after `gated` | The key it is gated on and the key it is leased under come from one function |
| 3 | Tests in `crates/rutis-loader/tests/row_inject_gate.rs`: Node and Python rows wait for the host's `llm`, use it, stop when it goes and start again when it is back; a Node row's `inject` names a service a Python row provides; a row `inject` the row isolates; adding an `inject` restarts once. One more in `instance_runtimes.rs`, inside instances | Without steps 1 and 2, the four of `row_inject_gate.rs` fail. The instance one asserts that `ri` calls `tools` through a lease of its own, in its instance's scope (`tool_callers`): with `row_inject` leasing nothing it can only use the shared one the `js` row of its process leased, and fails |
| 4 | Docs: `plugin-api`'s `ctx.use` row, guarantees `loader/rows-gate-on-services` and `loader/isolate-and-inject`, [declared services](design-declared-services-2026-10-29.en.md) §3, §4, §6 and §7, [optional dependencies](design-optional-deps-2026-10-28.en.md) §4.5 and §6, the doc comments of `Resolved::foreign_scope` and `RowInfo::isolate` | Link check and doc examples pass |

## 6. Follow-up

A group's `isolate` does not reach the runtime. The `isolate` `Loader::row` gives is the row's own (`RowInfo::isolate`), not looked up through its parent groups (as `Loader::check`'s `isolated_in` does). With a group writing `isolate: {llm: team}` and a runtime row inside it with `llm` in its `inject`: the gate in rutis waits in `team`, and the lease takes the one in `team`, so what the row reads through its own lease is right; but the shared registration has no label, sits at the process's root, and other rows of the process see it. A name of the plugin's `inject` goes through the same leasing code, so this was so before this design and is not of its making; fixing it means `RowInfo` carrying its groups' isolates, for another design.
