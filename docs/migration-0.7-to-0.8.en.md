# rutis 0.7 → 0.8

[中文](migration-0.7-to-0.8.md) · [Release notes](releases/0.8.0.en.md)

From 0.8, the `rutis` core and the dylib toolchain are part of the release train, and every rutis package shares one version. Most projects only change dependency versions. Projects that write their own resolver, match loader states exhaustively, or build dylib plugins should also read the matching section below.

## Dependency versions

Move every rutis package to 0.8:

```toml
# Cargo.toml
rutis = "0.8"            # was 0.6
rutis-loader = "0.8"
rutis-bridge = "0.8"
```

```json
"@arcships/rutis": "^0.8.0"
```

```toml
# pyproject.toml
dependencies = ["rutis>=0.8,<0.9"]
```

The core moves from 0.6.1 straight to 0.8.0 with no code changes. Upgrade it together with the train: `rutis-loader` 0.8 depends on `rutis` 0.8, and staying on `rutis = "0.6"` gives you two copies of the core whose types do not match.

Use the same version for the host and the language runtimes (`@arcships/rutis-runtime`, `rutis` on PyPI). Protocol versions are unchanged, so 0.7 runtimes still run rows outside instances; service names inside instances need 0.8 runtimes.

## Custom resolvers: build with `Resolved::new`

`Resolved` has a new field, `scoped`, and is `#[non_exhaustive]`, so it can no longer be built with a struct literal:

```rust
// 0.7
Arc::new(Resolved {
    factory,
    schema,
    meta,
    foreign_scope: false,
})

// 0.8
Arc::new(Resolved::new(factory).with_schema(schema).with_meta(meta))
```

`foreign_scope: true` becomes `.with_foreign_scope()`; a factory built per instance is `.with_scoped(scoped)`. The fields are still public and read as before.

## Matching loader states

`EntryStatus` has a new variant, `Stopped`: a copy inside an instance disposed itself, and only that copy stopped. `EntryStatus` is `#[non_exhaustive]`, so a `match` needs a wildcard arm:

```rust
match &entry.status {
    EntryStatus::Running(snapshot) => { /* … */ }
    EntryStatus::Stopped => { /* a copy in an instance stopped itself */ }
    _ => { /* Disabled, Inactive, Unresolved, and future states */ }
}
```

`EntryInfo` has a new field, `instance` (the instance a copy runs in). `EntryInfo` and the new `InstanceInfo` are `#[non_exhaustive]`: reading fields is unaffected; destructuring needs `..`.

## `Handover` in the local transport

`rutis_bridge::transport::local::Handover` has a new variant, `Loopback` (how processes start on Windows), and is `#[non_exhaustive]`. Code that matches on it needs a wildcard arm; code that only assigns `Spawn::handover` is unaffected.

## Dylib plugins

`rutis-sdk`, `rutis-dylib`, `rutis-dylib-meta` and `rutis-dylib-launcher` move to 0.8.0 and are published to crates.io for the first time. The SDK identity changes with the version, so **dylib plugins built in the 0.7 era cannot be loaded by a 0.8 host**: rebuild them with the 0.8.0 SDK bundle. Plugins are still built through the SDK bundle; do not depend on `rutis-sdk` from crates.io directly in a plugin (the packer rejects it).

## Behavior changes

- The Python runtime now applies a row's `isolate`. Configurations that set `isolate` on Python rows now isolate services as set, as Node rows do.
- `ServiceCatalog::key(name)` returns keys of global service names only; use `key_in(name, build)` for names inside instances. 0.7 had no names inside instances, so existing code is unaffected.

## Windows

Node / Python runtimes, remote runtimes, cross-language shared services, the `Process::launch` / `Process::mount` compatibility API, the Cordis mount bindings and the `rutis-host` command now run natively on Windows x64 with no code changes; WSL is no longer needed.

`rutis_bridge::include_mounts!()` now includes the generated mounts on Windows too (it compiled to nothing there before). A `build.rs` that skips `from_manifest()` on Windows must skip the macro there as well: `#[cfg(unix)] rutis_bridge::include_mounts!();`. A `build.rs` that always calls `from_manifest()`, as the Cordis guide shows, needs no change.
