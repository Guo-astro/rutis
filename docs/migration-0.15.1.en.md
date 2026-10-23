# Migrating to 0.15.1

[中文](migration-0.15.1.md)

## Events are one flat bus

0.13–0.15.0 delivered an event only to listeners in the emitter's scope and below it. 0.15.1 removes that rule and restores Cordis's simple model: every listener of the name on the root's bus receives the event, whatever its plugin, instance, runtime or linked node. This is a behaviour change, not an opt-in.

A parent row emitting and a child row listening now needs no wiring:

```rust
// In the parent row (`t1`):
ctx.events()
    .parallel(&ctx, &EventKey::named("session/created"), &created)
    .await?;

// In the child row (`t1/s1`), at any depth:
ctx.events().on(
    &ctx,
    &EventKey::named("session/created"),
    |_ctx, created: &Created| async move { Ok(()) },
)?;
```

A sibling instance's listener receives it too. Listeners run in registration order; `parallel` returns `CordisError::ListenersFailed` with the failures in registration order, and `emit` reports failures where they run and returns at once.

### If you relied on instance isolation

Use a filter or distinct names:

```rust
ctx.events()
    .filter(|info| info.scope.iter().any(|scope| scope.group == "tenant"))
    .parallel(&ctx, &EventKey::named("session/created"), &created)
    .await?;
```

`Ctx::scope()` and `ListenerInfo::scope` remain: a scope is a descriptor for filters, not a delivery rule. Across hosts, a linked node still sees only the names its link's `export` / `import` carry; links do not relay further.

## Deprecated, still source-compatible

- `Dispatch.scope` is ignored: the emitter's scope is not self-reported; use `ListenerInfo::scope` in a filter.
- The scope helpers `scope_contains` (Rust), `inScope` (Node/Bun), `within` (Python, Kotlin, Swift) and `events.Within` (Go) remain as descriptor predicates for filters. They do not select listeners.
- The testing tools' emitter scope is accepted and ignored: `h.at(scope)` (Node) and `Bus.emitter(scope)` (Python). A row's `scope` argument to `load` still sets its listeners' descriptors.

See [the event protocol](design-events-protocol-2026-10-18.en.md) §3–§9 and [migrating to 0.13](migration-0.13.en.md) for the rest of the event model.
