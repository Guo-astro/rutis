# rutis’s own mechanism problems, and which of them are the price of Cordis compatibility

[中文](review-cordis-architecture-2026-11-18.md)

Status: review record, 2026-11-18. Not a design document; it makes no new design decisions.
Scope: **rutis’s own mechanisms only**. Cordis’s own defects are not rutis’s problems; only costs kept or introduced for Cordis compatibility are counted.

Labels:

- **R: a rutis mechanism problem.** It exists without Cordis compatibility.
- **C: a cost of Cordis compatibility.** Without compatibility, the design would be different.
- **R/C: both.** The mechanism has a gap, and Cordis compatibility makes it harder to fix.
- **D: a test of the developer’s ability.** Not a framework defect; the framework leaves the decision to the developer. A competent developer avoids it; someone who does not know the rule steps on it silently. D is not a fourth kind of defect but a viewpoint: one item can be R/C and D at the same time.
- **X: Cordis’s own behaviour.** We choose to run real Cordis, so we do not fix it; we only state the boundary.

## 0. Bottom line

rutis’s kernel lifecycle is steadier than Cordis’s, but “steady” must be scoped: in the Rust kernel one `EffectRecord` runs once, and a normal unload waits for the records in reverse order, one at a time; registration during unload is refused; a failed load rolls back what was registered; a config update runs `validate_config → build → validate` first (it does not dry-run `apply`); dependency identity carries the provider generation and scope; services, events and plugins cross processes. It does not guarantee that every SDK’s registrations share one stack, that a cleanup succeeds, or that a normal reference already handed out is revoked.

**The real rutis problems are these seven:**

1. Bare timers, sockets and pools are not cleaned up automatically.
2. Dependencies are a “state marker”, not a graph; static cycles are checked, a two-row mutual wait is reported as `Cycle`, but more complex dynamic cycles have no complete diagnosis.
3. The kernel update is not an atomic transaction; the loader has rollback, but a rollback can fail, and a module reload does not guarantee the old file code comes back.
4. The default fault domain is one runtime process; plugins in one runtime share it.
5. A cleanup can hang; a single row’s unload in Node / Python / Bun has no per-entry budget.
6. Events have no “this event must be dispatched with which mode” contract; interception still depends on the dispatch site.
7. The effect APIs differ by SDK, and the resource body is the plugin’s to keep; `isolate` is visible only at run time, not in the type system.

**The Cordis compatibility costs are these five:**

1. Five event modes, `emit` dropping return values, `parallel` dropping return values, and `serial` / `bail` being separate are all kept to preserve Cordis’s API shape.
2. `bail` now fails on an async listener instead of treating a Promise as a result; this is deliberately incompatible, to avoid the `isBailed(Promise)` trap.
3. Cordis’s `intercept` config (merging config by service) is not supported.
4. A direct `ctx.set` on the Cordis side emits no notification; the runner refreshes a slot after one object method call and does not guarantee the next call already hits the new object.
5. A host-provided service is a proxy on the Cordis side, so `instanceof` fails; a synchronous wait can fail with `SyncWaitCycle`.

**Cordis’s own defects are not charged to rutis:**

- A native Cordis fiber’s `_unload` uses `Promise.all` to run cleanups concurrently. We run real Cordis, so its cleanup order is Cordis’s; rutis does not fix it. The only thing to do is not let the docs or guarantees claim to cover it.
- Cordis’s `bail` treating a Promise as a value, and Cordis’s paper/naming problems, are not rutis mechanism problems.

## 0.1 Short answers to the three questions

### What we need to fix

- **P0: state the guarantees correctly first.** Not every SDK’s registrations share one stack; `dispose` can run early; a plugin definition copy ends before the normal effect stack; a Node leaf puts only `provide` withdrawal, `effect` and the `apply` cleanup on its own `steps`, while `on` goes through the Cordis fiber; the runner withdraws listeners and exporters before disposing the fiber. After the deadline a listener may still run; arbitrary background tasks are not joined; an `apply` that ignores cancellation blocks the unload. The loader does not guarantee the old executable code comes back: a module resolve failure can keep the old instance, but after a new `apply` fails it restores the previous resolution, and a Node/Python row started from a file reads the bad code again and stays Failed until fixed. These change how users handle a failed hot reload, a force stop and a resource withdrawal.
- **P1: single-row unload diagnosability and timeout strategy** (R5). Node/Bun/Python wait directly for user cleanup; a per-entry timeout does not stop old code, and cannot both “solve the hang” and “keep strict serial order”. A deployment that needs controlled hot unload can raise it to P0.
- **P1/P2: tools backed by evidence.** R1 bare resources are an ownership boundary, not a failure of registered cleanup; a managed timer and examples are valuable, but there is no evidence to make all socket/pool lint P0. R2 only needs long cycles or unknown providers — `Loader::waits` already reports `WaitReason::Cycle` from running Pending rows. R4 is deployment advice first; “high risk” is not an attribute the framework can detect automatically. R6 should start as an optional event contract and be upgraded after real misuse evidence. R3 is a documentation distinction between config edits and module reload. R7/R8/`collect` should not block a release.

**Not fixed, only documented as boundaries**: native Cordis `_unload` concurrency, `intercept` config, `ctx.set` not notifying, `instanceof`, `SyncWaitCycle`.

### Choices made for Cordis compatibility

- **Run real Cordis**, no fork of upstream; but the Node runtime saves the original `events.dispatch` and five `events[mode]` methods and then overwrites them (`internal/*` alone delegates to the original). That is a real compatibility-maintenance cost, not just “depending on a package”. The Node runtime depends on `@deepseek-ai/cordis` 4.0.4, one copy, protocol version checked at the handshake.
- **Keep Cordis’s five event methods**: `emit` returns `void`, `parallel` collects no values, `serial`/`bail` separate.
- **`bail` deliberately breaks from old Cordis**: an async listener fails with `AsyncListener` instead of treating a Promise as a result; this fixes Cordis’s silent trap.
- **Keep `ctx.provide` / `ctx.get` / `ctx.set` semantics**: a host service is a proxy, so `instanceof` fails; a synchronous wait may fail with `SyncWaitCycle`.
- **Put Cordis listeners on the logical bus**: `internal/listener` registers, `internal/dispatch` fires locally; `internal/*` does not cross processes.
- **Do not normalize a native Cordis fiber’s `_unload`**: its cleanup order is Cordis’s; `sdk/unload-order` covers only rows the leaf/SDK manages.
- **`intercept` config is not supported**: the loader marks such a row `Unresolved`.

**If compatibility were dropped**: keep one `dispatch` returning every listener’s result; `serial`/`bail` become helpers; require a mode in the event definition; use a native typed service API instead of the `ctx.get/set` proxy; remove the `internal/*` bridge and `intercept`. Cross-process proxies, synchronous waits and cancellation boundaries remain; they are not Cordis-specific.

### What isolation and reclamation actually achieve

- **Registered entries**: in the Rust kernel one `EffectRecord` has one drainer; a repeated drain joins the same result; a normal fiber unload waits for its records in reverse, one at a time. Not every SDK’s registrations share one total stack: `dispose` can run early, a plugin definition copy ends first, a Node leaf puts only `provide` withdrawal, `effect` and the `apply` cleanup on its `steps`, `on` goes through the Cordis fiber, and the runner withdraws listeners and exporters before disposing the fiber. Withdrawing a registration, invoking its cleanup, the cleanup succeeding, and the resource being unusable are four different results; an `Arc`/ordinary reference already handed out is not revoked by the withdrawal.
- **Service scope**: yes. `isolate` scopes by service key; the loader’s `isolate` also covers rows of other languages.
- **Event scope**: no. Events are delivered by name on one bus and are not filtered by `isolate` (`events_are_not_filtered_by_isolate`); isolate by filtering or distinct names.
- **Fault/CPU/memory/security**: not by default. Rows in one runtime share a process, heap and event loop; separate runtimes/nodes give fault isolation; it is not a sandbox, plugins are trusted code.
- **Unregistered resources**: no. Bare timers/sockets/pools are not on the stack and are not reclaimed.
- **Running listeners/tasks**: the deadline covers in-flight calls the runtime tracks; after the deadline a listener may still run and the unload continues. Arbitrary background tasks are not joined; an `apply` that ignores cancellation blocks the unload.
- **External side effects**: no. A sent message, a written file, a committed database transaction cannot be rolled back; cleanup can only compensate; there is no distributed transaction across processes.

**One sentence**: for “impacts registered through rutis” we achieve **owner-scoped, per-layer-contract** lifecycle management and reclamation, and in the Rust kernel the same `EffectRecord` runs once; it is not one total stack across every SDK, and not every resource is necessarily reclaimed. For “anything a plugin can do to the world” we do not, and should not pretend we do. The architecture should state: we guarantee cleanup deduplication for managed registrations and dependency-scoped service isolation, not fault/CPU/memory/security isolation, and not a distributed transaction.

## 1. R: rutis’s own mechanism problems

| ID | Problem | Current state | Why it is rutis’s problem | Impact |
| --- | --- | --- | --- | --- |
| R1 | Bare resources are not cleaned up automatically | `ctx.effect` / `ctx.provide` / `ctx.on` enter the same effects list; bare timers / sockets / pools do not; `ctx.signal` only gives a cancellation signal | The framework only guarantees what goes through effect; there is no unified resource wrapper and no leak detection | In a long-running host, a bare timer keeps running after the row unloads, with no error |
| R2 | Dependencies are a marker, not a graph | `last_deps` is a set of `(provider id, generation, TypeKey, scope)`; it gates, coalesces equal sets and reloads on change; `Loader::waits` already reports `WaitReason::Cycle` from running Pending rows | No general cycle detection or transitive closure; long cycles and unknown providers have no complete dynamic diagnosis | Two-row mutual waits are reported; more complex dynamic cycles can still sit in Pending, found only by diagnostics or manual inspection |
| R3 | The kernel update is not an atomic transaction | `FiberView::update` runs `validate_config → build → validate` (it does not dry-run `apply`); only Active/Loading/Failed unload and load again, while Pending rechecks deps and keeps the new config. The loader has config-edit and module-reload rollback | There is no cross-fiber transaction; the loader rollback is a re-reconcile, not a compensation protocol; a module reload restores the previous resolution, not necessarily the old file code | A failed rollback leaves a partial state; a Node/Python row started from a file reads the bad code until fixed |
| R4 | The default fault domain is one runtime process | Each language runtime is at least one process; a row can be moved to another runtime or node, but by default rows in one runtime share a process | The default is shared; isolation must be configured explicitly | One plugin looping or crashing affects every row in that runtime |
| R5 | A cleanup can hang; a single row’s unload has no per-entry budget | `dispose_with_timeout` / `shutdown_with_timeout` bound the wait; `rutis-host --shutdown-timeout` bounds the whole cleanup; Node/Python/Bun `rows.unload` has no per-entry budget | The kernel waits for the cleanup; the runtime only falls back at process exit | A cleanup that never returns leaves the row Unloading; only stopping the host or killing the process recovers |
| R6 | Events have no “mode declaration” contract | `Event` has only `NAME` and `Value`; a listener reads `call.mode()` at run time; the dispatch site chooses the mode | rutis does not record “which mode this event must use” in the event definition or catalogue | A listener wants to intercept, the dispatch site uses `emit`, the return value is dropped; neither compile nor run reports it |
| R7 | Single-callback effect; the resource itself is the plugin’s | Rust `ctx.effect` runs a synchronous factory immediately and keeps the returned future as the cleanup; Node leaf `effect(cleanup)` only pushes the function; native Cordis has an async effect overload. The three are not the same API or the same problem | An SDK-specific API choice: acquiring, holding and registering a cleanup are three things; there is no unified `effect(acquire, release)` | Cross-lifetime resources need closures / IIFE / a base class; verbose and easy to forget; R7 is closer to SDK usability than a common mechanism defect |
| R8 | `isolate` is visible only at run time | `isolate` is a runtime scope; diagnostics show the scope; the type system does not encode it | The type layer cannot express “which isolation scope this service is in” | Reading the wrong scope is found only at run time; code review cannot see it |

### R1: bare resources

This is the most concrete one. The framework manages what you tell it how to clean; if you do not tell it, it does not manage it. rutis adds `ctx.signal` and process-exit cleanup over Cordis, but there is no timer wrapper and no leak detection.

**To fix**: provide `ctx.timeout` / `ctx.interval` or a unified resource wrapper; or warn about bare timers in `rutis-host check` / lint. If not, write it as a boundary: a bare resource is the author’s responsibility.

### R2: dependency graph / cycles

rutis’s dependency identity is stronger than Cordis’s: a new provider generation or a different scope is distinguished; the common A→B→A is distinguished by the provider generation. **But a same-generation, same-key, same-scope re-provide can produce an identical four-tuple, so it does not recognize arbitrary ABA**; `evicted_apply` tests “withdrawal marked, old consumer exiting, same-generation re-provide triggers one reload”; an old withdrawal leaving a new binding alone relies on `Arc::ptr_eq`. It still only answers “did the provider set change?”, not “does this graph have a cycle or a transitive closure?”. `Loader::waits` already reports `WaitReason::Cycle` from running Pending rows (A/B mutual wait has a test); what is missing is complete dynamic diagnosis for long cycles or unknown providers.

**To fix**: kernel D22 need not promise general cycle detection, but `Loader::waits` already has the two-row `Cycle`; add suspected-cycle reports for long cycles or unknown providers. Static cycles are already covered by `rutis-host check`.

### R3: transactionality

The kernel `update` runs `validate_config → build → validate` first (it does **not** dry-run `apply`); only Active/Loading/Failed unload and load again, while Pending rechecks deps and keeps the new config. The loader’s all-or-nothing has two forms: a config edit restores the old editable layer; a module `reload` restores the previous resolution after a new `apply` fails. A Node/Python row started from a file reads the bad code on disk again and stays Failed until fixed; the test asserts it must not print “previous version keeps running”. A failed rollback returns `RollbackFailed`; it is not a cross-plugin transaction.

**To fix**: either accept “the kernel has no transaction; the transaction is in the loader” and state it, or design a two-phase cross-plugin replacement. The latter is expensive; do not do it just for elegance.

### R4: default fault domain

Isolation is a first-class rutis capability, but it is not the default. Rows in one runtime share memory, global state and the event loop; one plugin blocking the event loop stops the others.

**To fix**: not by changing the kernel, but by changing the default deployment/docs: high-risk plugins go in separate runtimes by default; `rutis-host` makes the isolation setting more visible.

### R5: cleanup budget

`dispose_with_timeout` / `shutdown_with_timeout` let you stop waiting, but the cleanup itself keeps running; Node/Python/Bun `rows.unload` has no per-entry budget, only process exit. A cleanup that never returns leaves the row Unloading forever.

**To fix**: add a per-entry budget to a runtime row’s unload, or at least state “a single row’s unload has no per-entry budget” as a boundary. Go/Swift already have a budget for the whole unload; use it as a reference.

### R6: event mode contract

The kernel bottom layer is `EventBus::dispatch(Dispatch { mode, .. })`, so the bottom itself is neutral; the problem is that the **event definition** has no mode. A listener can read `call.mode()` at run time, but cannot declare at registration “I must be called by `bail`”.

R6 and C3 are two sides of the same problem: R6 is rutis lacking a “mode declaration” mechanism; C3 is Cordis’s event declaration having no mode, which makes the mechanism harder to enforce.

**To fix**: at minimum, record the mode in the event catalogue / generated output and compare it with the dispatch site in `check`; a stronger version is to put the mode on `Event`, or require a declaration on `ctx.on`. At least make “the listener wants to intercept, the dispatch site uses emit” visible during development.

### R7: single-callback effect

Rust `ctx.effect` runs a synchronous factory immediately and keeps the returned future as the cleanup; Node leaf `effect(cleanup)` only pushes the cleanup function; native Cordis has the async effect overload. **Synchronous acquisition, asynchronous acquisition and whether the resource body is returned are three different questions**; they cannot be merged into one “single-callback defect”. Async acquire is fine in Rust (the Promise overload exists); the real friction is “no body” and “acquire and hold are two steps”.

**To fix**: not a signature defect; encapsulation solves it: IIFE, a base class, or a service wrapper outside `ctx.effect`. The docs already have such patterns; promote them as recommended patterns.

### R8: isolate invisible at the type level

`isolate` is a runtime scope; the type system does not encode it. Diagnostics can show the scope, but code does not show “which isolation scope this service is in”.

**To fix**: hard at the type level; more realistically, mark the scope in code generation/diagnostics, or require an explicit scope in the API. Do not change the runtime semantics just to make the types pretty.

## 2. C: costs of Cordis compatibility

| ID | Cost | Compatibility requirement | Without compatibility | Can it change now? |
| --- | --- | --- | --- | --- |
| C1 | Five event modes; `emit` drops return values; `parallel` drops values; `serial`/`bail` separate | Cordis’s API is five methods, `emit` returns `void`, `parallel` takes no values | Could keep one `dispatch` returning every listener’s result; `serial`/`bail` could become helpers | Cannot remove; `collect` is not a thin wrapper over `parallel`; cross-runtime collection needs a new result path |
| C2 | `bail` fails an async listener with `AsyncListener` | Cordis used to treat a Promise as a short-circuit value; rutis chooses an explicit error | Could keep the old behaviour, but reintroduces the `isBailed(Promise)` silent error | Do not change back; document the difference |
| C3 | The event mode is not in the type contract; interception is decided by the dispatch site | Cordis’s event declaration has no mode; the dispatch site picks a method | Could require a mode in the event definition and check it at compile time | Can add an event catalogue + `check` comparison without changing the Cordis API |
| C4 | Cordis `intercept` config is not supported | The compatibility layer does not merge config by service; bare `null`, `true`, `{}` are accepted, only non-empty merged config is rejected | Not needed without Cordis compatibility | Can be done, but low priority; dsh plugins do not appear to use it |
| C5 | A direct `ctx.set` does not notify | Cordis’s `ctx.set` semantics do not notify; the runner refreshes slots after one object method call | Not needed without Cordis compatibility | Can hook it, but it reaches into Cordis; document the boundary first |
| C6 | A host service is a proxy, `instanceof` fails; a synchronous wait may fail with `SyncWaitCycle` | Cross-process/language cannot preserve JS object identity and the event loop | All Rust plugins in one process would not have it | Cannot remove; state the boundary and call through the interface |

### C1: five modes / `emit` dropping values

This is the largest compatibility tax. The article says Cordis’s `emit` is both the foundation and one of the five usages; rutis’s kernel turns the foundation into `dispatch(mode)`, but the public API still keeps five methods, because Cordis plugins are written that way.

- **Without Cordis compatibility**: keep one `dispatch` that returns every listener’s result; `serial`/`bail` become helpers; `parallel` can collect values.
- **Now**: cannot remove. Removing it breaks every Cordis plugin. Also, `collect` is not a thin wrapper over `parallel`: `serial`/`bail` stop before later listeners execute, while `parallel` runs all and drops the successes; an ordinary helper cannot reconstruct each listener’s return value. Cross-runtime collection needs a new result path or an explicit application protocol.
- **Conclusion**: keeping the method names and return shapes is C; putting the five execution semantics into the common protocol, and whether a native SDK gets another interface, is an R design choice, not all compatibility tax.

### C2: `bail`’s async semantics

Cordis’s `bail` treats a returned Promise itself as the result; rutis changes it to “an async listener fails with `AsyncListener`”. This is deliberately incompatible, because the old behaviour silently treats an unsettled Promise as a truthy value.

- **Without Cordis compatibility**: redesign `bail` without the historical baggage.
- **Now**: keep the explicit error; a plugin relying on the old behaviour must change.
- **Conclusion**: a cost of fixing Cordis’s trap, not a regression in rutis.

### C3: event mode / interception

Cordis’s event declaration has no mode; the dispatch site picks a method; rutis inherits this. A listener can read `call.mode()` at run time, but cannot declare “I must be called by `bail`”.

- **Without Cordis compatibility**: require a mode in the event definition and check it at compile time.
- **Now**: add an event catalogue + `check` comparison without changing the Cordis API; `observe_dispatch` was removed in 0.19 (no production caller), so observe with an ordinary listener.
- **Conclusion**: half compatibility tax, half a missing declaration contract in rutis.

### C4: `intercept` config

Cordis’s object form of `inject` can merge config by service; the rutis loader accepts bare declarations `null`, `true`, `{}`, but rejects a non-empty merged config and marks such a row `Unresolved`. This is an **unimplemented compatibility capability**, not a rejection of the entire object form.

- **Without Cordis compatibility**: this feature is not needed.
- **Now**: can be done, but low priority; dsh plugins do not appear to use it.
- **Conclusion**: a compatibility gap, not a kernel problem.

### C5: `ctx.set`

Replacing a service object directly on the Cordis side emits no notification; the runner refreshes all slots after one object method call, and **does not guarantee the next call already hits the new object**.

- **Without Cordis compatibility**: this semantics does not need handling.
- **Now**: can hook it, but it reaches into Cordis; document the boundary first.
- **Conclusion**: a compatibility boundary, not a rutis mechanism error.

### C6: proxies / synchronous waits

A host-provided service is a proxy on the Cordis side, so `instanceof` fails; a synchronous method cannot wait for a result that needs the Node event loop, and a cycle fails with `SyncWaitCycle`.

- **Without Cordis compatibility**: all Rust plugins in one process would not have it.
- **Now**: cross-process/language cannot fully preserve JS object identity and the event loop; state the boundary and call through the interface.
- **Conclusion**: an inherent cross-process cost, made more visible by Cordis compatibility.

## 3. D: the parts that really test the developer’s ability

Some “problems” are not the framework breaking; the framework leaves the decision to the developer. A competent developer avoids them; someone who does not know the rule steps on them silently. Listing them separately is how we decide “fix the framework” vs “write docs/tools”.

| ID | What it tests | What the developer must do | What the framework does now | What it could add |
| --- | --- | --- | --- | --- |
| D1 | Cleanup order | Register in dependency order; know that only a cleanup registered after `provide` can read the row’s own service | The docs state LIFO; `sdk/unload-order` has tests | Dependency-aware cleanup diagnostics; `check` hints on a suspicious order |
| D2 | Resource lifetime | Wrap bare timers/sockets in `ctx.effect`, or use `ctx.signal` to cooperate with unload | `ctx.effect`, `ctx.signal`, process-exit fallback | Timer/socket wrappers; lint/check for bare resources |
| D3 | Holding the resource body | Keep pools and other cross-lifetime resources in a closure / IIFE / base class | The single-callback API and doc examples | A resource-wrapper base class/macro; recommended patterns |
| D4 | Event mode / interception | Know which method the dispatch site uses; write the listener for that mode | `call.mode()` at run time; explicit `bail`/`waterfall` errors | Event catalogue + `check` comparing the mode; a type-level mode |
| D5 | `waterfall`’s await | In Cordis JS, `await next()` yourself; in Rust, use `Option` to avoid Promise truthiness | Rust types; `WaterfallNext` errors | JS lint/template; doc examples |
| D6 | Service replacement | Use `provide`/property assignment, not a direct `ctx.set` | The compatibility proxy; refreshes after one object method call | Hook `ctx.set`, or a `check` hint |
| D7 | Proxies / synchronous waits | Call through the interface, no `instanceof`; a synchronous method does not wait for the event loop | Explicit boundary errors `SyncWaitCycle` / `AsyncListener` | Static checks/docs; a timeout template for callers |
| D8 | Dependency cycles | Do not write mutually waiting `inject`s; dynamic dependencies are your responsibility | `rutis-host check` finds static cycles | A runtime suspected-cycle report |
| D9 | `parallel`’s return values | Know `parallel` does not collect values; use a shared container or a helper with a new result path | Docs; an explicit `AggregateError` | A `collect`/`all` that needs a new result path, not a thin wrapper |
| D10 | Plugin description | Write `name` / `inject` / `apply`, tool descriptions and parameters | Generators, templates, docs | Better scaffolding; generate descriptions from types |

**How to use this table**:

- D1 / D3 / D9 are “explicit contracts”: the docs state them, and following the rule avoids the problem. Acceptable, but examples and checks must be complete.
- D2 / D4 / D5 / D6 are “silent failures”: not following the rule reports nothing. These are the dangerous ones; make them loud (check, types, errors) instead of only documenting them.
- D7 is “the boundary is explicit, but you only see it when you hit it”: `SyncWaitCycle` / `AsyncListener` are errors, not silent; the problem is the API shape and the lack of development-time visibility.
- D8 is “the graph problem”: static checks help; dynamic cycles can only be reported.
- D10 is pure cost, not a pitfall: a plugin has to describe itself to be called by others.

## 4. X: Cordis’s own behaviour, not rutis’s problem

| ID | Cordis behaviour | Why it is not rutis’s problem | The only thing to do |
| --- | --- | --- | --- |
| X1 | A native Cordis fiber’s `_unload` uses `Promise.all` to run cleanups concurrently | We run real Cordis, no upstream fork; the Node runtime saves and overwrites the `events` methods but does not take over the native fiber’s `_unload`; its cleanup order is its own | Do not let `sdk/unload-order` claim to cover native Cordis plugins; state it in `guide/cordis` |
| X2 | Cordis’s `bail` treats a Promise as a result | That is Cordis’s old semantics; rutis already makes it an explicit error | Document the difference |
| X3 | Cordis’s paper, naming and version novelty | Not a rutis mechanism problem | Out of scope |

### X1 in detail

- A rutis leaf plugin (`definePlugin`) has its own `steps` stack in `node/rutis-runtime/src/leaf.mjs`, running in reverse, one at a time; `sdk/unload-order` covers this kind.
- A native Cordis plugin does not go through that stack; when a row unloads, `runner.mjs` does `await row.fiber.dispose()`, which goes through Cordis’s own `_unload`.
- This is Cordis’s mechanism, not rutis’s. rutis does not fix it; fixing it would mean taking over Cordis’s `ctx.effect` disposal, reaching into Cordis internals, which is expensive.
- **The only thing to do**: do not let the docs/guarantees imply `sdk/unload-order` covers native Cordis plugins; write it as an explicit compatibility boundary.

## 5. If Cordis compatibility were dropped

- **Would disappear or change**: C1 (five modes, emit/parallel dropping values), C2 (bail async semantics), C3 (event mode declaration), C4 (intercept), C5 (ctx.set).
- **Would remain**: R1 (bare resources), R2 (dependency graph/cycles), R3 (transactionality), R4 (default fault domain), R5 (cleanup budget), R7 (single-callback effect), R8 (isolate invisible at the type level).
- **C6 (proxies/synchronous waits)**: much lighter if every plugin were in one process; still present across processes, not Cordis-specific.

## 6. What to fix first

After Astra’s review, order by “does this make a user rely on the wrong guarantee”, not by “sounds most architectural”:

1. **P0: state the guarantees correctly first.** Not every SDK’s registrations share one stack; `dispose` can run early, a plugin definition copy ends first, a Node leaf’s `on` goes through the Cordis fiber, and the runner withdraws listeners/exporters before disposing; after the deadline a listener may still run; a module reload does not guarantee the old file code comes back. These change how users handle a failed hot reload, a force stop and a resource withdrawal.
2. **P1: R5 single-row unload diagnosability and timeout strategy.** Node/Bun/Python wait directly for user cleanup; a per-entry timeout does not stop old code and cannot both “solve the hang” and “keep strict serial order”. A deployment that needs controlled hot unload can raise it to P0.
3. **P1/P2: R1 managed timer / examples.** Bare resources are an ownership boundary, not a failure of registered cleanup; a managed timer and examples are valuable, but there is no evidence to make all socket/pool lint P0.
4. **P1/P2: R2 long cycles / unknown providers.** `Loader::waits` already has the two-row `Cycle`; add only the more complex dynamic graph.
5. **P1/P2: R4 deployment advice.** Put high-risk plugins in separate runtimes; do not treat “high risk” as something the framework can detect automatically.
6. **P1/P2: R6 optional event contract.** Start with an optional event catalogue + `check` comparison; upgrade after real misuse evidence. `Event::name()` allows runtime names, so a cross-language check is not unconditionally cheap.
7. **P1/P2: R3 transaction-boundary docs.** Separate config edit, module reload, old-code restoration and seamless switch.
8. **Not release blockers**: R7 (SDK usability), R8 (type-level isolate), `collect` (needs a new result path, not a thin wrapper).
9. **Compatibility boundaries on one page**: C4/C5/C6, the `bail` difference and native Cordis `_unload`, readable at selection time.

From the D viewpoint, the point is not “take the responsibility away from the developer” but “make it loud when the rule is not followed”: D2 (bare resources), D4 (event mode), D5 (waterfall await) and D6 (`ctx.set`) should at least appear in `check`, lint or an error, not only in the docs; D7 already has explicit errors and needs earlier development-time visibility.

These P0/P1 calls are engineering judgements, not production incident-frequency or plugin-usage data; they cannot support a claim that bare resources or event mode is the most common failure.

## 7. Evidence index

Kernel:

- `crates/rutis/src/effect.rs:66` `EffectRecord`; `:111` `drain`; `:202` `run_cleanups`; `:238` `Disposer`.
- `crates/rutis/src/fiber.rs:856` `resolve_deps`; `:909` `refresh_deps`; `:1141` `fail_load`; `:1211` `drain_effects`; `:1239` `unload`; `:2046` `dispose_with_timeout`; `:2120` `update`.
- `crates/rutis/src/ctx.rs:634` `ctx.effect`; `:993` `register_effect_named`; `:1015-1017` refusal during unload; `:1028-1041` late record drained.
- `crates/rutis/src/events.rs:101` `Event`; `:116` `Mode`.
- `crates/rutis/src/events/api.rs:83,132` `Call::mode` / `SyncCall::mode`; `:454-486` the five modes; `:654-690` the five modes on `Ctx`.
- `crates/rutis/src/bus.rs:916` `EventBus::dispatch`; `:958` `dispatch_sync`.
- `crates/rutis/src/bus/run.rs` `run_parallel` (errors only).
- `crates/rutis/src/events/remote.rs:224` `Dispatch { mode, .. }`.

Compatibility layer:

- `node/rutis-runtime/src/leaf.mjs:112-130`: the leaf plugin’s own reverse stack.
- `node/rutis-runtime/src/cordis-events.mjs:84-96`: for lifecycle, only `internal/listener` is taken over.
- `node/rutis-runtime/src/runner.mjs:775-797`: dispose exporters first, then `row.fiber.dispose()`.
- `node/rutis-runtime/package.json`: `@deepseek-ai/cordis` 4.0.4.
- `crates/rutis-loader/src/loader/check.rs:467`: static cycle detection.
- `crates/rutis-loader/src/loader/waits.rs:354-372`: `WaitReason::Cycle` for running Pending rows; test `crates/rutis-loader/tests/waits.rs:224`.
- `crates/rutis-loader/src/loader/desired.rs:309-321`: `intercept` rejects only non-empty merged config; bare declarations are accepted.
- `crates/rutis-host/tests/dev.rs:395-417`: after an apply failure it must not print “previous version keeps running”.
- `crates/rutis-host/src/main.rs:1001`: the cycle error text.
- `crates/rutis-loader/tests/loader.rs:212,245`: apply failure rollback / rollback failure.
- `crates/rutis-loader/tests/reload.rs:43`: reload all-or-nothing.
- `node/rutis-runtime/src/cordis-events.mjs:100-143`: saves the original methods and overwrites `events.dispatch` and the five `events[mode]`.
- `node/rutis-runtime/src/runner.mjs:270-285`: `ctx.set` slots refresh after one object method call.

Guarantees and guides:

- `docs/guarantees.md`: `core/D6`, `core/D7`, `core/D14`, `core/D22`, `core/D32`, `events/emit`, `events/waterfall`, `sdk/unload-order`, `loader/apply-failure-rolls-back`, `loader/reload-all-or-nothing`, `host/stop-deadline`.
- `docs/guide/cordis.en.md`: running Cordis plugins, events, unload, boundaries.
- `docs/requirements-protocol-plugins.en.md` §4–§5: synchronous waits, `instanceof`, `ctx.set`, the `bail` difference.
- `docs/design-unload-order-2026-11-01.md`: background of the unload stack; it records the state at design time; the current implementation follows `guarantees.md` `sdk/unload-order`.

Tests:

- `crates/rutis/tests/parity.rs:1141` `rejects_effect_registration_during_unload`.
- `crates/rutis/tests/contract.rs:700` `effect_during_cleanup_fails`; `:690-735` events and unload.
- `crates/rutis/tests/evicted_apply.rs:356` withdrawn while loading, service returns first and still reloads once.
- `crates/rutis/tests/config_update.rs:133,158,548,626` config update, dry-run, update while loading / unloading.
- `crates/rutis/tests/events.rs:274` `emit_does_not_wait_and_contains_failures`; `:511` `bail_meets_an_async_listener`; `:550` waterfall; `:956` unload waits for listeners.
- `crates/rutis/tests/unload_deadline.rs:36` listener deadline.
- `crates/rutis-host/tests/check.rs:576` cycle output.
- `crates/rutis-loader/tests/events_conformance.rs:1491` onward: `events_with_x_*` for Cordis / leaf / Python / Go / Bun / Swift / Kotlin.
- `tests/e2e/tests/cordis.rs`: real DSH plugins in the Node runtime; a Node runtime crash does not affect Python rows.
