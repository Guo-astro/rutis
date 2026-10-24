# Migrating to 0.16

[中文](migration-0.16.md)

0.16 keeps the flat event bus of 0.15.1 and removes the source-compatibility shims that release carried. The event API is now the simple one: delivery by name, a scope is only a descriptor for filters.

## Removed from Rust

- `rutis::events::Dispatch::scope`: the emitter's scope is not self-reported. Use `ListenerInfo::scope` in a filter, or distinct event names.
- `rutis::events::scope_contains`: use `info.scope` in a filter and compare it yourself.

`Dispatch` literals still use `Dispatch::new(..)`; there is no position for a scope.

## Removed from the SDKs and test tools

- Node/Bun `inScope`, Python, Kotlin and Swift `within`, and Go `events.Within`: these were scope-prefix predicates, not a delivery rule.
- Node's `h.at(scope)` and Python's `Bus.emitter(scope)`: emit with `h.parallel(..)` / `bus.emitter()`. A row's `scope` argument to `load` still sets its listeners' descriptors.

If you need the old helper, it is one predicate:

```rust
fn within(listener: &[Scope], emitter: &[Scope]) -> bool {
    listener.len() <= emitter.len()
        && listener.iter().zip(emitter).all(|(a, b)| a.id == b.id)
}
```

## What did not change

- `ctx.on` / `ctx.emit` / `parallel` / `serial` / `waterfall` / `bail` / `filter`.
- Flat delivery: every listener of the name on a root's bus receives the event; siblings and children included.
- Across hosts: only the link's `export` / `import` names cross, and events still travel one hop.
- The event wire format.

See [0.16.0 release notes](releases/0.16.0.en.md) and [the event protocol](design-events-protocol-2026-10-18.en.md) §3–§9.
