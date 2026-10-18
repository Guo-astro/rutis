# Migrating to 0.13

[中文](migration-0.13.md)

0.13 unifies events ([design](design-events-2026-10-18.en.md), [protocol spec](design-events-protocol-2026-10-18.en.md)): one logical bus, five modes that mean the same everywhere. Old behaviour is not kept. This page lists the changes by area.

## Rust kernel (`rutis`)

**Event types are serde.** `Event` now requires `Serialize + DeserializeOwned`, and so does `Event::Value`:

```rust
#[derive(Serialize, Deserialize)]          // new
struct Joined { room: String }
impl Event for Joined { const NAME: &'static str = "room/joined"; type Value = String; }
```

A value type that is not serde (a handle, a channel) does not belong in an event: put it in a service.

**Listeners are found by name.** `EventKey::<E>::of()` is named `E::NAME` (before, `NAME` was only for diagnostics and `of()` was a channel of its own). Two types with the same `NAME` now share listeners, converted through serde: give unrelated types distinct names. `named` / `dynamic` are unchanged.

**One listener table.** A listener registered once is called by every mode; the emitter chooses the mode.

| Before | Now |
| --- | --- |
| `on(ctx, key, \|ctx, e\| ..)` | `on(ctx, key, \|ctx, e, call: Call<'_, V>\| ..)`; `call.mode()`, `call.next()` (waterfall only) |
| `on_waterfall(ctx, key, \|ctx, e, next\| next.call())` | `on(ctx, key, \|ctx, e, call\| call.next())`; the value is `Option<V>` |
| `on_sync` / `on_waterfall_sync` of a `SyncEvent` | `on_sync(ctx, key, \|ctx, e, call: SyncCall<'_, V>\| ..)` for any event; `SyncEvent` is gone |
| `on_pattern(ctx, EventPattern::any_prefix([..]), ..)` | `on_pattern(ctx, &EventPattern::prefix(p), \|ctx, key, e, call\| ..)`, one prefix per registration |
| `*_keyed(name, ..)`, `*_instance(id, ..)`, `EventKey::instance(id)` | `EventKey::named` / `dynamic`; scopes instead of instance channels (below) |
| `serial(ctx, key, &e)` | `serial(ctx, key, Arc<E>)` |
| `waterfall(..) -> Result<V>`, `Terminal` returning `V` | `-> Result<Option<V>>`, `Terminal` returning `Option<V>`; `()` is "no terminal" |
| `waterfall_sync(.., \|ctx, e\| Ok(v))` | `\|ctx, e\| Ok(Some(v))` |

**Behaviour changes.**

- A listener is called with the `Ctx` that registered it, not the emitter's.
- `emit` starts every listener in registration order without waiting for any (before: one after another); one name's emits still start in emit order. A failure is reported to the error sink as `CordisError::ListenerFailed` and never stops the other listeners.
- `parallel` failures are always `CordisError::ListenersFailed` (AggregateError) in registration order, also for one failure (before: one error as is, several as `Aggregate` in completion order).
- `Some(false)` is no result in `serial` / `bail_sync`.
- `bail_sync` / `waterfall_sync` meeting an async (`on`) listener fail with `AsyncListener`.
- Every listener is skipped once its plugin is unloading, and the unload waits for its running calls (their `ctx.cancelled()` fires); before, only instance, pattern and synchronous listeners were.
- `rutis/*` names are reserved: emitting one fails with `ReservedEvent`.

**Scopes replace instance channels.** The loader marks each group instance with `Ctx::mark_scope(group)`; `Ctx::scope()` is the chain from the root. A listener hears events emitted in its scope and below it. To pick listeners further, `ctx.events().filter(|info| ..)` gives the same emitting methods filtered by each listener's `ListenerInfo`.

**Diagnostics.** `EventSubscription`, `EventBacklog` and `DispatchAttempt` carry the event `name` instead of a `TypeKey`; `DispatchMode` is `rutis::Mode`; `ReentrantEvent` carries a `name`.
