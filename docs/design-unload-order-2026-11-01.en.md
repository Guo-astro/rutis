# Unload order: withdrawal and cleanups in one reverse stack

[中文](design-unload-order-2026-11-01.md)

Status: in progress (branch `claude/unload-lifo`, not merged). Date: 2026-11-01. Base: the follow-up [declared services](design-declared-services-2026-10-29.en.md) §7 records, on top of [a runtime row's own inject](design-row-inject-gate-2026-10-31.en.md). The code §1 describes is that of the parent of the commit this document lands in.
Issue: none yet, opened with this document. Related: [declared services](design-declared-services-2026-10-29.en.md) §3 item 6 and §7; [events protocol](design-events-protocol-2026-10-18.en.md) §7.1; `core/D7` and `core/D14` (`docs/guarantees.md`).

## 1. Where things stand

When a row unloads, when the services it provides leave its process, and whether its cleanups can still read them, gives two answers across the six languages.

Rust puts the withdrawal in the effect list:

```
Ctx::provide_native (crates/rutis/src/ctx.rs)
├─ ① origin = Origin::Provide(key)
├─ ② register_internal_effect(label, origin, …)     → the same effects list ctx.effect pushes to
└─ ③ returns Effect::AsyncDisposer(evict_and_finalize(key))

Fiber::drain_effects (crates/rutis/src/fiber.rs:1128-1140)
└─ for record in effects.into_iter().rev()          → in reverse, one at a time
   ├─ a cleanup: runs the closure
   └─ a withdrawal: evict_and_finalize (consumers stop first, then the entry goes)
```

So whether a cleanup can read a service it provided depends only on **registration order**: a cleanup registered after `provide` runs first and reads it; one registered before runs later and does not. `crates/rutis/tests/contract.rs:1396` (`self_access_during_cleanup`) asserts the first case (`Some(77)`), and `core/D14` in `docs/guarantees.md` states it as "the provider cleans up last and can still reach its own services meanwhile".

The reading side is the other half:

```
Ctx::read_binding_as (crates/rutis/src/ctx.rs:520-556)
├─ self_access = in_subtree_of(provider)            → is the reader in the provider's subtree
├─ not: provider.state() != Active or binding.removing → not visible
└─ yes: state is not consulted; visible until the withdrawal really removes the entry
```

Two different moments, then: **other readers lose the service the instant the fiber enters `Unloading`** (before any cleanup), and **the provider itself keeps it until the withdrawal record runs**.

The five SDKs hoist the withdrawal ahead of every cleanup:

| Runtime | Withdrawal | Cleanups |
| --- | --- | --- |
| Go | `unload` in `runtime.go`: `for _, id := range r.provided`, delete, `refresh`, unlock, then cancel | afterwards, `r.cleanups` in reverse |
| Python | `unload` in `runner.py`: `for id in list(row.provided)`, delete | afterwards, `row.cleanups[::-1]` |
| Bun | `unload` in `runner.ts`: `for (const id of row.provided)`, delete | afterwards, `row.cleanups.splice(0).reverse()` |
| Kotlin | `unloadRow` in `Runtime.kt`: `for (id in row.provided.toList())` under the lock, delete | afterwards, `row.cleanups.asReversed()` |
| Swift | `unload` in `Runtime.swift`: `for id in row.provided` under the lock, delete | afterwards, `cleanups.reversed()` |

The consequence: a cleanup can **never** read a service its row provided, not even one registered after `provide` (stricter than Rust); and other readers losing the service at `Unloading` happens to come out right, but only because the deletion is even earlier. Node is a third behavior: the adapter keeps its own `provided` map (`node/rutis-runtime/src/leaf.mjs`), and on unload Cordis's `_unload` starts every disposal at once with `Promise.all` (`@deepseek-ai/cordis` 4.0.4, `lib/index.js:1372`), so every cleanup reads it — but not in reverse and not one at a time, which Rust deliberately does not do (the comment on `drain_effects` in `fiber.rs`: Cordis runs them concurrently; here they run one after another).

[Declared services](design-declared-services-2026-10-29.en.md) §3 item 6 records these three outcomes, and §7 says: "the other follow-up is the unload order (§3 item 6): all six run withdrawal and cleanups in reverse, one at a time, as Rust does. It changes each runtime's unload and has nothing to do with the declared set, so it is done separately."

## 2. Decisions

| # | Decision | Why; what was dropped |
| --- | --- | --- |
| D1 | A withdrawal is one entry of the row's unload stack: `provide` registers one, `effect` registers one, and unloading runs the **same stack in reverse, one entry at a time**. A withdrawal happens where it was registered, never hoisted | As Rust's `drain_effects` (`Origin::Provide` and `ctx.effect` share one `effects` list). What it makes observable is `core/D14`'s "can still reach its own services meanwhile", which `contract.rs::self_access_during_cleanup` asserts. Dropped: "all cleanups before withdrawal" (Cordis's semantics in Node) — that runs every cleanup before any withdrawal, which is neither Rust's intermediate state nor this repository's kernel |
| D2 | Other readers lose the row's services when the unload starts, wherever the withdrawal sits in the stack | Rust's `read_binding_as`: for a reader outside the provider's subtree, `provider.state() != Active` is enough, and the state turns `Unloading` in the unload's first step. An SDK has no subtrees, and "is the reader the provider" is already decided by `row.key`, so the mapping is "this row is itself; every other row is another". Dropped: letting the withdrawal's position decide when others lose it — then whether a call a cleanup makes into another row finds a service would depend on registration order, which cannot be explained |
| D3 | Once the withdrawal has run, the row no longer counts the name as its own: `use` says `not-declared` | An existing rule ([declared services](design-declared-services-2026-10-29.en.md) D4): `own_service` is true only while `fiber.provided` still holds the key, and the withdrawal removes it. The same in all six, nothing new |
| D4 | The carrier: one stack per row, its entries either "run this cleanup" or "withdraw this service (the one registered)" | A withdrawal has to tell whether the entry in the table is still the one it registered — a plugin may `provide` the name again after calling the returned function (the mirror of `reentrant_provide_fails`). Comparing objects by value cannot tell two equal instances apart, so an entry carries the identity it was registered with |
| D5 | Registering a cleanup while unloading is refused, with the line `rutis: row <row> is not active: cleanup is not registered` (Node writes `plugin <plugin>`). All six gain this line in this design: before it, only Kotlin wrote it (`Ctx.kt`, where `addCleanup` returns `Boolean` and makes the caller handle the failure); Go, Python, Bun and Swift appended silently to a list the unload had already taken, so that cleanup never ran and nothing was said | Rust returns `InactiveEffect` at that moment (`ctx.rs:861`, `contract.rs::effect_during_cleanup_fails`). An SDK's `effect` returns nothing, so a line is all it can give. Dropped: dropping it silently — a registered cleanup that never runs leaves the plugin no way to know, which is what those five did |
| D6 | The host side is unchanged: when announced services are withdrawn and when `rows.unload` is answered stay as they are | A runtime row's host already withdraws the projection before sending `rows.unload` (`projection.withdraw().await` before `process.unload_row` in `crates/rutis-loader/src/runtime.rs`), and the host sees withdrawals before the reply (the comment on `unload_row` in `process.rs`). This design changes the in-process tables, not the frame order |
| D7 | Node runs one at a time in reverse too: the adapter keeps a stack of its own and drains it with one Cordis effect | Cordis's `_unload` is `Promise.all`, and its own disposal order cannot be changed; but the adapter can push `effect`, `provide` and the cleanup `apply` returns onto a stack of its own and register **one** Cordis effect that drains it. Listeners and `signal` stay with Cordis: they do not touch services. Dropped: leaving Node concurrent — five runtimes one at a time and one concurrently gives plugin authors no way to predict a cleanup's order |

## 3. What developers see

### Plugin authors

A row provides `counter`, then registers a cleanup that reads it:

```go
func (p *plugin) Apply(ctx *rutis.Ctx, cfg Config) error {
	err := ctx.Provide("counter", counter{p.inject.Listen()})
	if err != nil {
		return err
	}
	ctx.Effect(func(ctx context.Context) error {
		var n int
		return ctx.Use("counter", &service).Call(ctx, "Count", &n)
	})
	return nil
}
```

Normally: by the time unload reaches this cleanup, `counter` is still there and the call succeeds; the withdrawal is behind it, and only after it runs does the row no longer have `counter`. A cleanup registered **before** `Provide` does not read it: at that moment the row no longer counts `counter` as its own, and it gets `service counter is not available to this plugin, which does not declare it in Inject: add "counter" to its Inject` (`use`) or `nil` (`get`). Both orders give the same result in all six languages; what differs is only the API's name (`ctx.effect` / `Effect` / `effect`).

Another row reading it while this row unloads: `use` says `service counter is not available`, `get` returns `nil` — from the moment this row starts unloading, not from when the withdrawal record ran.

A cleanup registering a cleanup:

| Language | Output (verbatim) |
| --- | --- |
| Go | `rutis: row weather is not active: cleanup is not registered` |
| Python | `rutis: row weather is not active: cleanup is not registered` |
| Bun | `rutis: row weather is not active: cleanup is not registered` |
| Kotlin | `rutis: row weather is not active: cleanup is not registered` |
| Swift | `rutis: row weather is not active: cleanup is not registered` |
| Node | `rutis: plugin weather is not active: cleanup is not registered` |

None of these six lines existed before this design (Kotlin's came in its previous change), so the first time one is seen is after upgrading. Node says "plugin" rather than "row": a leaf plugin does not see its row's id and names itself by the name it declared, or `leaf` when it declared none.

It will not run: unload runs the stack as it stood when it started. So a cleanup should not expect to schedule another cleanup; do the work in the one cleanup.

### Host owners

The frame order does not change: `rows.unload` is still answered after every cleanup of the row has run, and announced services are still withdrawn before the reply. Unloading getting slower (cleanups now run one after another) affects `--shutdown-timeout` as before: all six give up the remaining cleanups at the deadline, with the same line.

### The new concept

The "unload stack": every entry a row's unload runs in reverse, cleanups and withdrawals mixed. It grows as `provide` and `effect` are called and is taken as a whole when the unload starts; anything registered after it is taken is not on it (the third paragraph of §3).

## 4. Compatibility

- Where results get looser: a cleanup registered after `provide` reading a service its row provided goes from failing to succeeding (all six). Calls that already succeeded do not start failing.
- Where results get stricter: in Go, Python, Bun, Kotlin and Swift, a cleanup registered before `provide` that used to read the service — it did not read it before this design either (the withdrawal came first), so no call becomes stricter there. Node is the one that does: its cleanups read the service even when registered before `provide`, and now do not, as in the other five.
- Order: cleanups go from "all at once" (Node) or "one by one, withdrawal first" (the five) to "in reverse, one by one, each withdrawal in its place". A plugin relying on cleanups **running in parallel** (several cleanups waiting on each other) hangs; the deadline gives up the rest and writes its line. That is a stated limit, with no switch.
- Registering a cleanup while unloading: only Kotlin wrote a line before; Go, Python, Bun and Swift dropped it silently and now write one. Node used to hand it to Cordis's `effect`; now it writes its own line and does not enter Cordis's disposal table.
- `docs/guarantees.md`: one new row (`sdk/unload-order`), and `core/D14` gains a test on the SDK side.

## 5. Implementation

One stack per row: `provide` and `effect` both push to it, unload takes it and runs it in reverse, and the reading side hides an unloading provider from everyone else.

| # | Where | Change | Test |
| --- | --- | --- | --- |
| 1 Go | `go/rutis/internal/runtime/runtime.go`: `row.steps`, `runtimeState.provide`, `Ctx.Effect`, `Ctx.service`, `refresh`, `unload` | The withdrawal closure and the cleanup go on one stack; `providedService` gains a "withdrawing" mark: the unload's first step sets it and calls `refresh` (the host still hears the withdrawal then), the deletion and the removal from `provided` wait for the withdrawal entry; `Ctx.service` skips marked entries of other rows; `refresh` treats a marked entry as absent | `internal/runtime/unload_order_test.go`: a cleanup registered after `provide` reads it, one registered before gets `not-declared`; another row's `use` says `unavailable` while this row unloads; a cleanup calling `Effect` writes that line |
| 2 Python | `python/rutis/rutis/runner.py`: `Row.steps`, `Runtime.provide`, `Context.effect`, `Context.use`/`get`, `Runtime.lookup`, `Runtime.unload` | as above | `python/rutis/tests/test_unload_order.py` |
| 3 Bun | `bun/rutis-bun/src/runner.ts`: `Row.steps`, `provide`, `effect`, `lookup`/`#find`, `unload` | as above | `bun/rutis-bun/test/unload-order.test.ts` |
| 4 Kotlin | `internal/Runtime.kt`: `Row.steps`, `provide`, `addCleanup`, `unloadRow`; `Ctx.use` in `Ctx.kt` | as above | `jvmTest/.../UnloadOrderTest.kt` |
| 5 Swift | `API/Runtime.swift`: `Row.steps`, `provide`, `effect`, `unload`; `Ctx.use` in `API/Ctx.swift` | as above | `Tests/SelfTest/UnloadOrderTests.swift` |
| 6 Node | `node/rutis-runtime/src/leaf.mjs`: one stack per fiber in `toCordis` and one Cordis effect draining it in reverse; `unloadRow` in `runner.mjs` drains before `fiber.dispose()` | `effect`, `provide` and the cleanup `apply` returns all go on the stack | `node/rutis-runtime/test/` (or where the existing ones live) |
| 7 Docs | `docs/design-events-protocol-2026-10-18.md` §7.1 steps 3 and 5, `docs/guide/*`, `docs/guarantees.md` | Step 3 becomes "not visible to others from the start of the unload, withdrawn in its place"; step 5 becomes "cleanups and withdrawals in reverse" | — |

[Declared services](design-declared-services-2026-10-29.en.md) §3 item 6 becomes one sentence saying all six match Rust, and §7's follow-up entry goes.

## 6. Follow-ups

- A group's `isolate` reaching runtime rows: still [a runtime row's own inject](design-row-inject-gate-2026-10-31.en.md) §6, unrelated to this design.
- The cleanups' deadline: all six give the remaining cleanups one budget; this design does not change it. Whether Rust gives it per entry or as one budget is a separate question.
- Node's listeners and `signal` are still disposed concurrently by Cordis; this design only lines up services and cleanups. Putting them on the stack too would mean replacing Cordis's `on` with the adapter's own table, which is another piece of work.
