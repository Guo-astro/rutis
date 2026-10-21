# Rust services reach every language: `#[rutis::service]` on the type

[中文](design-rust-service-exposure-2026-10-21.md)

Status: proposal (2026-10-21), #510, 0.15. Replaces the `build.rs` list as the main way a Rust service reaches other languages; `rutis_bridge::build::Exports` stays, only for types whose source you cannot change.

## 1. Goal

Services should reach other languages automatically, with nothing for the developer to do. Node / Bun / Python / Go leaf plugins already work this way: after `ctx.provide(name, impl)`, other languages call it by name. The Rust side does not.

This design makes it so:

> Whoever writes a Rust service adds one line, `#[rutis::service]`, to its `impl` block and calls `ctx.provide(...)` as usual; rows of every language then call it by name. No list, no `build.rs`, no `HostDispatch`. Built-in plugins and dylib plugins work the same way.

## 2. Today

| Provider | Can other languages call it |
| --- | --- |
| Leaf plugin `ctx.provide(name, impl)` | Yes, automatically |
| Rust plugin `ctx.provide(value)` | No. The host must list the type in `build.rs` with `Exports` and `export_all`, or hand-write a `HostDispatch` |

Problems:
- The service is written in one crate, and its list lives in another crate, the host.
- When it is not listed, rows that depend on it just wait, with no hint why.
- The host should not need to know the types inside a dylib plugin at all.

As a result, downstream projects still hand-write bindings one by one.

Rust cannot do this with no marker at all. A running Rust program cannot see a type's methods, so the translation code must be generated at compile time, and something has to tell the compiler which types to generate it for. This design moves that something from the host's list onto the type itself, as one line. Swift's `@RutisService` and Kotlin's `@RutisService` work the same way, and Rust matches them. It is also a familiar idiom in Rust, for example `#[pymethods]`, `#[wasm_bindgen]` and `#[zbus::interface]`.

## 3. Usage

```rust
use rutis::service;

pub struct Calendar { /* … */ }

#[service]                       // exported as "calendar" by default (type name in camelCase)
impl Calendar {
    pub async fn add(&self, entry: NewEntry) -> Result<Entry, CalendarError> { /* … */ }
    pub fn list(&self, day: Date) -> Vec<Entry> { /* … */ }
    pub async fn watch(&self, ctx: &rutis::Ctx, handler: impl Fn(Entry) -> BoxFuture<'static, Result<(), Error>>) { /* … */ }
}

// Provided in the plugin as usual; nothing else to write
ctx.provide(Calendar::new())?;
```

```ts
// A row in any language, the same as calling a leaf plugin's service
const calendar = ctx.use('calendar')
await calendar.add({ title: 'Standup', startsAt: new Date() })
```

To rename it, use `#[service(name = "cal")]`. That is the only option.

## 4. Rules

| # | Decision |
| --- | --- |
| R1 | **What is exported:** the public `&self` methods in `impl` blocks marked `#[service]`. A type can have several marked `impl` blocks; together they are its methods. Unmarked types are not exported. Host-internal services (storage, locks, database handles and so on) stay private by default; this boundary is intentional. |
| R2 | **Provided means exported:** `ctx.provide(value)` (and `provide_as` when the key is the type itself) also provides its exported entry point in the same scope. One disposer removes both, and both go when the plugin unloads. No `export_all`, and no separate "export plugin". |
| R3 | **Scope follows where it is provided:** provided inside an instance, it is exported only in that instance (what `.service_in(…, group)` does today); provided outside instances, it is global. Instance groups are no longer written down. |
| R4 | **What crosses:** exactly what `Exports` supports today: serde parameters and results, `async` and sync methods (a `Promise` for every caller), callbacks by reference, the calling row's `&Ctx`, `Result` errors thrown with `name` / `code` / `data`, and tagged values. The rules are in the [Rust host guide](guide/rust-host.en.md#exported-services-reference) and do not change. |
| R5 | **Errors at compile time:** a public method in a marked `impl` that cannot be bound (`&mut self`, a borrowed result, an unsupported parameter) is a **compile error** pointing at that method, suggesting a fix or `#[service(skip)]` on the method. Today it is a build warning and the method is silently left out. |
| R6 | **Name conflicts:** when two types in one scope have the same exported name, the later `provide` fails, and the error names both types and plugins. This matches the leaf-plugin rule. |
| R7 | **Diagnosing an unmarked service:** when a row depends on the name `x`, no service named `x` exists, but an unmarked Rust service whose type name in camelCase is `x` does, the wait message says: "`x` is the Rust service `crate::X`, which is not exported; add `#[rutis::service]` to its `impl`". |
| R8 | **dylib plugins:** marked types in a dylib are exported once it loads; the host does not need to know them. On a version swap, the old generation's entry points go with its services, the new generation's appear when it provides them, and consumers reload under the existing rules. |
| R9 | **TypeScript declarations:** `rutis-host types <crate dir> -o services.d.ts` finds every marked type in the source and writes the same declarations as today (same generator). To refresh them on every build, write one line in `build.rs`, `rutis_bridge::build::Exports::new().typescript("services.d.ts").generate()`, with no type list. Calls themselves need no declarations. |
| R10 | **Compatibility:** `build.rs` `.service(…)` / `.service_in(…)` keep working, for third-party types you cannot change. A type that is both marked and listed is a build error asking to remove the list entry. Hand-written `HostDispatch` keeps working. |

## 5. Implementation

Three parts, all inside rutis; user code only sees the one-line marker from R1.

1. **Compile time: the `#[rutis::service]` procedural macro** (new crate `rutis-macros`, re-exported by `rutis`). It reads the method signatures in the `impl` block and generates the same dispatcher `Exports` generates today: decode arguments, call, encode the result, map errors. Decisions that depend on a type (is it a `Result`, how its error gets a `code`, is it a callback) are made by traits in the expanded, concrete code, so type aliases (`io::Result`, a custom `Result<T>`) are still recognized. The source-reading part of the generator is kept only for the declarations in R9.
2. **Registration:** the macro generates a static registration entry for the type (its `TypeId`, exported name, and the function that builds the dispatcher). The registry lives in `rutis` and is collected automatically when the process starts or a dylib loads; nobody lists anything. A dylib's entries record the module and generation they belong to, and lookup prefers the caller's own generation.
3. **Attaching at provide time:** when `Ctx::provide_as` provides a concrete type, it looks up its `TypeId` in the registry once. If the type is registered, it also provides a `host_key(name)` dispatcher in the same scope and instance, under the same disposer (R2, R3). `rutis` core holds only this table and this hook and does not depend on `rutis-bridge`; the dispatcher code lives where the macro expands and depends on `rutis-bridge`.

Today's `Exports` dispatcher code, value rules, error rules and `caller_row` are reused as they are. Only two things change: who decides which types are exported, and where they get attached.

## 6. Performance

In short: calls cost nothing extra, startup and provide costs are negligible, and builds are faster than with today's `build.rs`.

| Area | Effect |
| --- | --- |
| Cross-language calls | The same code as today's generated dispatcher, so the same cost. The cost is in crossing languages (encoding, inter-process transport), not in how the type was registered. |
| Rust calling Rust | Never goes through the dispatcher; the marker does not affect native calls. |
| `provide` | One hash lookup by `TypeId` (nanoseconds), plus one more `Arc` provided for a marked type. An unmarked type costs only the lookup. |
| Startup | Collecting entries is proportional to the number of marked types, one static record each, in microseconds. Today each export mounts a plugin that waits for the native service, one per instance group; attaching at provide time removes all of those plugins. |
| Compile time | The macro only processes marked `impl` blocks, at a cost similar to serde's derive. Today's `build.rs` runs `cargo metadata` and parses dependency source trees on each run; with the marker, that only happens when generating declarations (R9). |
| Binary size | One dispatcher per exported method, as today; unmarked types generate no code. |
| Memory | One dispatcher object per provided marked service. |

## 7. Alternatives considered

| Alternative | Why not |
| --- | --- |
| Keep the `build.rs` list | The list is far from the type, it must be written, and forgetting it gives no hint; the host should not know a dylib plugin's types. |
| No marker: export every type passed to `provide` | Not possible: with no marker there is no compile-time code, and a running Rust program cannot see methods. Even if it were, it would expose every host-internal service to plugins. |
| Scan the whole dependency graph and export every public type with `&self` methods | Slow builds, exposes internal types, and name conflicts between unrelated types. |
| Infer structure at runtime through serde reflection | It only infers the shape of data, not methods, so a marker is still needed; it also does not support common forms such as `untagged` and `flatten`. |
| Mark a trait and provide it with `provide_as::<dyn Trait>` | Adds a trait and a key, more ceremony than marking the `impl`; marking the type already covers this need. |
| Pass the name at provide time, `ctx.provide_named("calendar", v)` | Changes how Rust provides services, and the name still lives only at the provide site; the type name by default, renamed in the marker when needed, is more natural. |

## 8. Acceptance

- A built-in Rust plugin that only adds `#[rutis::service]` and calls `provide`, with no `build.rs`, no `export_all` and no `HostDispatch` in the host, is callable by name from Node, Bun, Python and Go rows. This covers serde parameters and results, sync and async methods, `Result` errors, callbacks and `&Ctx`.
- The same service provided in two instances: each instance's rows get their own; provided outside instances, it is available globally.
- The same service in a dylib plugin: callable after load; after a version swap the old entry point goes, the new one appears, and consumers reload.
- After calling the disposer `provide` returned, other languages can no longer call it.
- A public method that cannot be bound gets a compile error pointing at it; with `#[service(skip)]` it compiles.
- Two services with the same name: the second `provide` fails, and the error names both.
- An unmarked Rust service: rows that depend on it get the R7 diagnostic.
- `rutis-host types` writes exactly the declarations today's `Exports` writes for the same type, and they pass `tsc --strict`.
- Existing `build.rs` export examples and tests pass unchanged; `examples/rust-services` switches to the marker, keeping one list-based case for a third-party type.

## 9. Delivery

All in one PR:

1. The `rutis-macros` procedural macro, the registry in `rutis`, and the `provide_as` hook.
2. Extract dispatcher generation from `build::Exports` into a part shared by the macro and the generator.
3. Registration collection on dylib load, and per-generation lookup.
4. `rutis-host types`; the R7 diagnostic; the R10 duplicate check.
5. Docs: the Rust host guide's "Provide Rust services to plugins" leads with the marker, with the list form moved to "third-party types"; plugin API, agent skills; 0.15 release notes. Both languages.
