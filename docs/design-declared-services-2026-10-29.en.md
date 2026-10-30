# A row's `inject`, and `use` checking the declaration first

[中文](design-declared-services-2026-10-29.md)

Status: implemented (branch `claude/row-inject-declared`, not merged). Date: 2026-10-29; rebuilt on the new base 2026-10-30. Base: a follow-up on `claude/design-optional-deps` (optional dependencies, on main `d5aa3200`, 0.19.3, since 2026-10-31) (the two gaps [optional dependencies](design-optional-deps-2026-10-28.en.md) §4.5 and §6 record). The code §1 describes is that of the parent of the commit this document lands in (the optional-dependencies commit).
Issue: none yet; opened with this document. Related: [#676](https://github.com/arcships/rutis/issues/676) (the `not-declared` texts and the rule behind them).

## 1. Where things stand

The same plugin code and the same `rutis.json` give two answers in six languages.

A plugin that does not write `inject: ['llm']`, a row that writes `inject: ["llm"]`, and then `ctx.use('llm')`:

| Runtime | Service in this process | Service missing |
| --- | --- | --- |
| Node (leaf) | Gets it | The row waits at the Cordis gate inside the process; `apply` does not run |
| Bun, Python | Gets it | `service llm is not available` (`unavailable`) |
| Go, Swift, Kotlin | `service llm is not available to this plugin, which does not declare it in inject: add 'llm' to its inject` (`not-declared`; Go's is `… in Inject: add "llm" to its Inject`) | The same line |

The protocol is not the gap. The host has long sent the row's `inject` as `rows.load`'s fifth argument (`load_exporting` in `crates/rutis-bridge/src/runtime/process.rs`: `[key, entry, config, isolate, inject, exports, { scope, id }]`). Three runtimes take it and use it; three drop it:

- Python: `row.row_inject` (`Runtime.load` in `python/rutis/rutis/runner.py`), added to the declared sets `row.uses` and `row.inject`.
- Node: `rowInject` of `toCordis(spec, rowInject)` (`node/rutis-runtime/src/leaf.mjs`), added to `uses`; Bun: `uses = new Set([...plugin.inject, ...(inject ?? [])])` in `Runtime.load` (`bun/rutis-bun/src/runner.ts`).
- Go: in the `rows.load` case of `go/rutis/internal/runtime/runtime.go`, a comment says a leaf runtime's rows wait in rutis, and then `args[4]` is not read; the `rows.load` cases of Swift's `Runtime.swift` and Kotlin's `Control.kt` are the same.

The second difference is the order of `use`'s two steps. Go (`Ctx.Use` in `runtime.go`), Swift (`Ctx.use` in `Ctx.swift`) and Kotlin (`Ctx.use` in `Ctx.kt`) check the declaration first and refuse at once; Node, Bun and Python look in the process's service table first and give what they find, the declaration check deciding only what to say when nothing is there. What developers see is a **neighbour effect**: a plugin that forgot to declare `llm` gets it in Node, Bun and Python whenever another row of the same process happens to declare it, and never does in Go, Swift or Kotlin. The answer depends on what else runs in the process, not on this row's configuration. `get` already checks the declaration first in all six, so this is `use` alone.

[Optional dependencies](design-optional-deps-2026-10-28.en.md) records the row `inject` gap in §4.5 and §6 and leaves it open.

## 2. Decisions

| # | Decision | Why; what was dropped |
| --- | --- | --- |
| D1 | A row's `inject` counts as declared: the same in all six languages, `use`'s declared set being the plugin's `inject` ∪ the row's `inject`, and `get`'s adding the plugin's `optional` (`use` leaves `optional` out, as Rust's `require`; see [optional dependencies](design-optional-deps-2026-10-28.en.md) O4) | The row's `inject` is the required dependency the host wrote for this row; a Rust row adds it to the fiber's declarations (`combined_injects`), and saying "this plugin does not declare it" contradicts the row's configuration. This document changes the declared set only: `use` no longer says `not-declared`, and what the process already has (provided by a row of the same process, or leased in by another row for itself) is reached. A service of the host or another process that the process does not have yet still is not: rutis neither gates a runtime row on its row `inject` nor leases it in for the row (§7), and this document leaves that as it is. Dropped: fixing Go, Swift and Kotlin only — that is today's inconsistency. Dropped: removing the row's `inject` from the set so Node, Bun and Python refuse too — the host's required dependency for the row would then be unreadable from the plugin, and `inject` would mean nothing |
| D2 | `use` checks the declaration first, as `get` does: the same in all six languages | The same rule as `get` and as [optional dependencies](design-optional-deps-2026-10-28.en.md) §4.5: an undeclared name is never leased, so say what to declare (#676). Two lines each in Node, Bun and Python. Dropped: making Go, Swift and Kotlin look in the table first — that spreads the neighbour effect to three more runtimes, on behaviour that depends on who else is in the process and cannot be explained or tested |
| D3 | The errors' words do not change | `use` of an undeclared name keeps its line (`… does not declare it in inject: add 'llm' to its inject`; Go's `Inject` and `%q`, see [optional dependencies](design-optional-deps-2026-10-28.en.md) O9); `get` keeps the optional one. The existing word-for-word tests do not move |
| D4 | A name the plugin provides itself counts as declared while it provides it: `use` and `get` give the object itself; once withdrawn it is like a name never declared | A plugin reading what it provides is legitimate; before D2 only Bun's and Python's `use` allowed it, by looking in the table first; Node's `use` did only after `apply` (during `apply` the fiber is still loading, and Cordis's strict read does not give what it provides); every `get`, and Go's, Swift's and Kotlin's `use`, said `not-declared`. All six judge by whether it provides the name now, as Rust does (`require_inner` in `crates/rutis/src/ctx.rs`: `own_service` holds only while `fiber.provided` still has the key, which `evict_and_finalize` removes on withdrawal; a strict read then says `Undeclared`): Bun, Python, Go, Swift and Kotlin drop it from the row's `provided` on withdrawal; Node's adapter keeps an entry for each `provide` and drops that entry when the plugin calls the withdrawing function (the name may have been provided again, and an old withdrawal does not drop the new one). Dropped: reading `context[name]` — Cordis answers names such as `fiber`, `then` and `_u` itself, and once withdrawn it throws `cannot get property "<name>" without inject`. Dropped: asking the Cordis fiber's `store` — a withdrawn name leaves it only some microtasks later, and when the plugin withdraws and provides it again in one turn, the late removal drops the new entry |
| D5 | The test driver is outside the declared set: Node's and Bun's `rutis-test` (the plugin `rutis-host test` loads first) calls any service for the test | It is the test, not a plugin under test, and the test names services as it runs (`t.service(name)`); Python's test tool has always read the runtime's service table directly. The mark is a module-private `Symbol`, not `Symbol.for`, and not in the package's public `exports`; deep-importing the module by file path reaches it, and that goes around the public interface. Dropped: having the driver declare its names up front — there is no such list |

## 3. Rules

1. The declared sets, the same in all six languages: for `use`, the plugin's `inject` ∪ the row's `inject` ∪ the names the plugin provides itself; for `get`, that and the plugin's `optional`. Testing tools (Node/Bun `@arcships/rutis/testing`, Python `testing`, Go `rutistest`, Swift/Kotlin `RutisTest`) have no row `inject` in `load`, so theirs leaves it out; calling a plugin function directly has no row, as before. The test driver does not check the declared set (D5).
2. `rows.load`'s `args[4]` (the row's `inject`) is taken by Go, Swift and Kotlin, kept on the row and added to the declared set. Cordis rows are gated on it inside the process already, unchanged; rutis itself neither gates on it nor leases it (§7).
3. `use` and `get` both: ① check the declaration, and refuse when it is missing (`use` with its line, `get` with the optional one, both code `not-declared`, `data` `{service}`); ② look in the table: an object of this process, a leased proxy, else `use` says `unavailable` and `get` gives an empty value.
4. A name in both the row's `inject` and the plugin's `optional` fails as before ([optional dependencies](design-optional-deps-2026-10-28.en.md) §5 row 4b).
5. Against Rust: D1's declared set, D2 and D4 are what the Rust kernel does (the row's `inject` joins the fiber's `declared_injects` through `combined_injects`; `require_inner` decides `Undeclared` before it looks at the service's status). Rust also gates the row on the same `combined_injects`; the half SDK rows lack is §7. The one difference is `get` of an undeclared name: Rust's `ctx.get` does not check declarations because it reads the whole host's service table, where an undeclared service is still found, and records the undeclared read in diagnostics; an SDK process leases in only declared names, so returning the empty value would call a service the host has absent, and the SDK has no diagnostics channel, so it still says `not-declared`.
6. Whether a cleanup (`effect`) on unload reads what the row provides comes in three kinds. Rust keeps withdrawals and cleanups on one stack and runs them in reverse, one at a time (`drain_effects` in `crates/rutis/src/fiber.rs`; the comment in `provide_inner` in `crates/rutis/src/ctx.rs`): a cleanup registered after the `provide` runs before the withdrawal and reads it, one registered before does not. Node's adapter drops the entry it keeps only when the plugin calls the withdrawing function (D4); on unload Cordis withdraws directly (`Promise.all` in `Fiber._unload` in `@deepseek-ai/cordis`'s `src/fiber.ts` starts every disposal at once), not through that function: a cleanup reads what the row provides whether registered before or after the `provide`, and whether it reads at once or after an `await`. Bun, Python, Go, Swift and Kotlin withdraw everything the row provides first, so a cleanup reading it always gets `not-declared`. This document does not change the unload order (§7).

## 4. What developers see

### Plugin authors

With `inject: ["llm"]` on the row and `llm` already in the process, a plugin that declares nothing gets it:

```ts
export default definePlugin({
  apply(ctx) {
    ctx.use<Llm>('llm').ask('hi')     // llm, from the row's inject: given
  },
})
```

Go, Swift and Kotlin give it now too (D1). With the provider in the host or another process, and no other row having leased it into this process for itself, it is not given today (row 1 below, §7).

When it fails:

| # | How it happens | Output (word for word) | Code | How to fix |
| --- | --- | --- | --- | --- |
| 1 | The row's `inject` has `llm`, the plugin writes nothing, and the process has no `llm` (nothing has it; or the host or another process does, rutis does not lease a runtime row's row `inject`, and no other row leased it for itself, §7) | `service llm is not available` (a Node leaf does not get this far: the Cordis gate waits for `llm` in the process and `apply` does not run) | `unavailable` | The host or another process has it: write `llm` into the plugin's `inject` too, and rutis gates on it and leases it; nothing has it: wait for the provider, or take `llm` out of the row's `inject` |
| 2 | Neither the plugin nor the row declares `llm`, `use('llm')` is called, and **another row** in this process declares it | Before Node/Bun/Python: given. After: `service llm is not available to this plugin, which does not declare it in inject: add 'llm' to its inject` (Go: `… in Inject: add "llm" to its Inject`) | `not-declared`, `data` `{service}` | Write `llm` into the plugin's `inject` |
| 3 | As above, neither declares it, and nothing in the process provides it | The same line | `not-declared` | As above |
| 4 | The plugin writes `optional: ['llm']` and the row's `inject` writes `llm` too | `<row id>: unresolved: entry "<row id>" rejected: llm is in the row's inject and the plugin's optional; keep it in one` | The row fails | Remove one of the two |

Row 2 is D2's one behaviour change: a plugin that leaned on a neighbour's declaration starts to fail. That is what #676 asks for (say what to declare), not a regression.

### Those assembling plugins with `rutis-host`

No new configuration and no new output. One `rutis.json` gives the same declared set in all six languages. Where the row's `inject` names a service the process does not have yet, row states still differ: a Node leaf's row waits at the Cordis gate and `apply` does not run, the other five's `apply` gets `unavailable`; that waits for §7.

### New concepts to learn

None.

## 5. Compatibility

- D1 and D4 only loosen, each where `not-declared` used to be said: Go's, Swift's and Kotlin's `use` and `get` of a name in the row's `inject` now give it when the process has it, else `use` says `unavailable` and `get` gives an empty value; for a name the plugin provides itself, all six `get`s and Go's, Swift's and Kotlin's `use` now give the object. A call that succeeded does not start failing; code branching on `not-declared` sees something else.
- D2 tightens: a plugin that never declared a service and got it from a neighbour starts to get `not-declared`. It is a bug fix, not a migration; one line in the CHANGELOG and the release notes at release (as upstream does, a feature PR leaves the CHANGELOG alone).
- The protocol does not change: `rows.load`'s arguments were already sent, and `PROTOCOL` stays.

## 6. Implementation

| Step | Content | Done means |
| --- | --- | --- |
| 1 Go, Swift, Kotlin | Read `rows.load`'s `args[4]`, keep it on the row, add it to `declares`; and in all six, a name the plugin provides itself counts as declared too (before, only Bun's and Python's `use` had it, by looking in the table first, Node's `use` only after `apply`, the rest not at all) | Two cases each: one building the row directly, one sending it through `rows.load`'s argument list (`args[4]`) and then `rows.update`: a service in the row's `inject` that the same process provides (the case provides it in-process) is given to `use` and `get`, and still after the restart (Swift's `update` used to drop it); when the host lacks it, `unavailable`, not `not-declared`; a plugin reading what it provides is not refused, and is once withdrawn (Kotlin's case no longer lists it in `optional`) |
| 2 Node, Bun, Python | `use` checks the declaration first | A case each: undeclared, with a neighbour declaring it → `not-declared` word for word; in the row's `inject` → given; Node, Bun and Python, provided then withdrawn → `use` and `get` say `not-declared` at once; Node, withdrawn and provided again in one turn → the new one is read, as for a name never declared. Python's `test_scopes` plugin reading two names now declares them (after D2 an undeclared name fails before the lookup); Node's and Bun's test drivers carry the D5 mark, and both SDKs' tests pass as before |
| 3 Cross-language | One case per language (§4 rows 1, 2 and 3 word for word), the set [optional dependencies](design-optional-deps-2026-10-28.en.md) step 2 owes | The six languages agree. What landed is one case per language rather than one cargo set: a leaf runtime row hands its `inject` to the Cordis gate inside the process, and rutis leases in only the names the plugin declares, so "the row's `inject` names a service of another runtime" is not reached today (§7) and makes no cross-language case |
| 4 Docs | `plugin-api`'s `ctx.use` row says the declared set includes the row's `inject`; one sentence per language guide | Link check and doc examples pass |

Also: #683's `allowed_services` judges by the declared set, and comes right with this.

## 7. Next

A runtime row's row `inject` is not wired on rutis's side yet: `foreign_scope` (`crates/rutis-loader/src/loader/desired.rs`) gives a runtime row a `BoundScope` with an empty `inject`, and `JsRow::start` (`crates/rutis-loader/src/runtime.rs`) leases only the `gated` names the plugin declares. So for a service in the row's `inject` that the host or another process provides, rutis neither waits nor leases it into the process for the row (one another row leased in for itself, the row reads too). A Rust row has no such gap: `combined_injects` (`crates/rutis-loader/src/loader/reconcile.rs`) adds the row's `inject` to the fiber's injects, and the fiber starts once they are all there. The next PR brings runtime rows in line: rutis gates on the row's `inject` and leases it at start. §4 row 1 is then only "nothing has it", and `loader/rows-gate-on-services` holds for runtime rows too.

The other follow-up is the unload order (§3 rule 6): all six run withdrawals and cleanups in reverse, one at a time, as Rust does. It changes each runtime's unload, has nothing to do with the declared set, and is done on its own.
