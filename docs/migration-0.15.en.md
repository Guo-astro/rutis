# Migrating to 0.15

[中文](migration-0.15.md)

## rutis-loader: the Go runtime types are aliases

What compiled languages (Go, Swift later) share moved to `rutis_loader::BinaryResolver<L>`, `BinaryRuntimes<L>` and friends, where `L` is a language implementing `BinaryLanguage` (`rutis_loader::Go`). Every Go name is still there, as a type alias:

| Name | Now |
| --- | --- |
| `GoResolver` | `BinaryResolver<Go>` |
| `GoRuntimes` | `BinaryRuntimes<Go>` |
| `GoRuntimesHandle` | `BinaryRuntimesHandle<Go>` |
| `GoBinaries` | `Binaries` |
| `GoManifest` | `BinaryManifest` |
| `GoBinary` | `BinaryFile` |
| `GoRuntimeState` | `BinaryRuntimeState` |
| `GoRuntimeInfo` | `BinaryRuntimeInfo` |

**No code changes**: `GoResolver::new(..)`, `GoRuntimes::new(..)`, `GoRuntimeState::Stopped(_)` and the like compile as before and behave the same. `cargo semver-checks` reports `GoResolver`, `GoRuntimes` and `GoRuntimesHandle` as removed structs (it does not follow aliases of generic types); that is a false alarm.

Added (existing code is unaffected):

- `GoRuntimesHandle::supervise(RestartPolicy, callback)` / `unsupervise()`: a Go runtime whose process ends on its own is handled as `no` / `on-failure` / `exit` says; by default, still nothing.
- `rutis_loader::backoff`: the restart backoff (rutis-host uses it too).
- The feature `binary`: what compiled languages share; `go` enables it.

## rutis-host

`runtimes.go` takes `restart` (`"no"`, the default; `"on-failure"`; `"exit"`), as `node`, `py` and `bun` do; see [rutis-host](guide/rutis-host.en.md), "When a runtime ends". It is a new field only: existing `rutis.json` files are read as before, unchanged.

New (existing configurations are unaffected):

- `runtimes.swift`: Swift plugin binaries (macOS), with the same settings as `runtimes.go` (`start`, `idle`, `restart`); see [Swift plugins](guide/swift-plugin.en.md).
- A remote runtime's `language` accepts `"swift"` and `"kotlin"` / `"kt"`: app hosting ([Swift apps](guide/swift-app.en.md), [Kotlin apps](guide/kotlin-app.en.md)).

`rutis-host check` checks the plugin API of Go and Swift plugin binaries: below 3 (built with an SDK older than rutis 0.13) is an error with a non-zero exit status. Such binaries already could not connect since 0.13 (unified events); `check` now says so up front. Rebuild them with a current SDK.

## rutis-bridge

`session::Dispatch` and `peer::Handler` each gained a method with a default implementation (for the event test hooks): existing implementations compile unchanged.

## Events

Runtimes take emit replies in frame order, and rutis always answers a forwarded emit with a plain value ([event protocol](design-events-protocol-2026-10-18.en.md) §5.5). Plugin code is unchanged, and 0.13 hosts and runtimes connect as before.
