# Rust services reach every language: `#[rutis::service]` on the type

[中文](design-rust-service-exposure-2026-10-21.md)

Status: implemented (2026-10-21), #510, 0.15. Replaces the `build.rs` list as the main way a Rust service reaches other languages; `rutis_bridge::build::Exports` stays, only for types whose source you cannot change.

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
| R2 | **Provided means exported:** `ctx.provide(value)` (and `provide_as` when the key is the type itself) also provides its exported entry point in the same scope. One disposer removes both, and both go when the plugin unloads. No `export_all`, and no separate "export plugin". A mutable binding (`provide_mut_as`) is not served (the entry point holds the value as provided). |
| R3 | **Scope follows where it is provided:** provided inside an instance, it is exported only in that instance (what `.service_in(…, group)` does today); provided outside instances, it is global. Instance groups are no longer written down. |
| R4 | **What crosses:** exactly what `Exports` supports today: serde parameters and results, `async` and sync methods (a `Promise` for every caller), callbacks by reference, the calling row's `&Ctx`, `Result` errors thrown with `name` / `code` / `data`, and tagged values. The rules are in the [Rust host guide](guide/rust-host.en.md#exported-services-reference) and do not change. |
| R5 | **Errors at compile time:** a public method in a marked `impl` that cannot be bound (`&mut self`, a borrowed result, an unsupported parameter) is a **compile error** pointing at that method, suggesting a fix or `#[service(skip)]` on the method. Today it is a build warning and the method is silently left out. |
| R6 | **Name conflicts:** when two types in one scope have the same exported name, the later `provide` fails, and the error names both types. This matches the leaf-plugin rule. |
| R7 | **Diagnosing an unmarked service:** when a row depends on the name `x`, no service named `x` exists, but an unmarked Rust service the default rule would name `x` does, `Loader::waiting_for(&entry)` lists what the row waits for and says: "`x` is the Rust service `crate::X`, which is not exported: add `#[rutis::service]` to its impl". |
| R8 | **dylib plugins:** marked types in a dylib are exported once it loads; the host does not need to know them (the plugin crate depends on `rutis-sdk` only, or, built against an sdk-bundle, on nothing written; it reaches the marker as `rutis_sdk::rutis::service`, whose generated code goes through `rutis_sdk::rutis`). On a version swap, the old generation's entry points go with its services, the new generation's appear when it provides them, and consumers reload under the existing rules. The host links `rutis-bridge`. |
| R9 | **TypeScript declarations:** `rutis-host types <crate dir> -o services.d.ts` finds every marked type in the source and writes the same declarations as today (same generator). To refresh them on every build, write one line in `build.rs`, `rutis_bridge::build::Exports::new().typescript("services.d.ts").generate()`, with no type list. Calls themselves need no declarations. |
| R10 | **Compatibility:** `build.rs` `.service(…)` / `.service_in(…)` keep working, for third-party types you cannot change. A type that is both marked and listed is a build error asking to remove the list entry. Hand-written `HostDispatch` keeps working. |

## 5. Implementation

Three parts, all inside rutis; user code only sees the one-line marker from R1.

1. **Compile time: the `#[rutis::service]` procedural macro** (new crate `rutis-macros`, re-exported by `rutis`). It reads the method signatures in the `impl` block and generates, next to the block, a dispatch function: decode arguments, call, encode the result, map errors. Decisions that depend on a type (is it a `Result`, how its error gets a `code`, does a parameter take a function) are made by traits in the expanded, concrete code, so type aliases (`io::Result`, a custom `Result<T>`) are still recognized. The generated code depends on `rutis` only: values pass through object-safe interfaces in `rutis::service` (the `Wire`, carrying values as erased serde) to the host's `rutis-bridge`, which encodes and decodes them, so a dylib plugin depending on `rutis-sdk` alone works too.
2. **Registration:** the macro generates a static registration entry for each marked block (its `TypeId`, exported name, method names and dispatch function), placed in the program's or library's initializer section (Linux `.init_array`, macOS `__mod_init_func`, Windows `.CRT$XCU`), and added to a registry in `rutis` when it loads; nobody lists anything. A dylib type's `TypeId` differs per version (the crate's version is part of it), and old versions are never unloaded, so each generation's entries stay valid. `rutis-bridge` installs its `Exposer` into `rutis` the same way, when it loads.
3. **Attaching at provide time:** when `Ctx::provide_as` provides a type, it looks its `TypeId` up in the registry. If the type is registered, the `Exposer` also provides a `host_key(name)` dispatcher (`host_key_in(name, id)` in an instance) in the same scope and instance, under the same disposer (R2, R3). Provided in an instance, the instance *claims* the name: rutis-loader resolves the names of the rows in that instance through the claims first (the same rule as for names a runtime connection announces), and a new claim rebuilds the copies already built with the global key. `rutis` core holds only the registry, the hook and the interfaces, and does not depend on `rutis-bridge`.

The value rules, error rules and `caller_row` are those of the code `build.rs` generates; the TypeScript declarations are still written by the source-reading generator, which finds the marked blocks in the crates that depend on rutis (R9).

## 6. Performance

In short: calls cost about what they cost today, startup and provide costs are negligible, and builds are faster than with today's `build.rs`.

| Area | Effect |
| --- | --- |
| Cross-language calls | The same encoding rules as today; values go through erased serde, one more dynamic dispatch (a few virtual calls per value), negligible next to crossing languages (encoding, inter-process transport). |
| Rust calling Rust | Never goes through the dispatcher; the marker does not affect native calls. |
| `provide` | One pass over the registry comparing `TypeId`s (as many entries as marked blocks: nanoseconds to microseconds for a few dozen), plus one more `Arc` provided for a marked type. |
| Startup | One atomic insert per marked block when it loads. Today each export mounts a plugin that waits for the native service, one per instance group; attaching at provide time removes all of those plugins. |
| Compile time | The macro only processes marked `impl` blocks, at a cost similar to serde's derive. Today's `build.rs` runs `cargo metadata` and parses dependency source trees on each run; with the marker, that only happens when generating declarations (R9). |
| Binary size | One dispatch arm per exported method, about as today; unmarked types generate no code. |
| Memory | One dispatcher object per provided marked service; a claim is one name per instance. |

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
- Existing `build.rs` export examples and tests pass unchanged (`examples/rust-services`); the new example `examples/marked-services` uses the marker.

## 9. Delivery

One PR: `rutis-macros` and `rutis::service` (registry, hook, `Wire`), `rutis-bridge`'s `Exposer` and claims, rutis-loader resolving through claims and `Loader::waiting_for`, the generator finding marked blocks and the duplicate check, `rutis-host types`, tests (Node, Bun, Python and Go rows, instances, a dylib swap), docs.
