# Migrating to 0.14

[中文](migration-0.14.md)

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
