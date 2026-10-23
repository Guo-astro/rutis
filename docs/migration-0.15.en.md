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

Behaviour changes (no code changes):

- **A failed start is no longer retried every 100 ms**: when the binary is gone or its manifest cannot be read, the runtime shows as stopped with why (`Stopped(why)` in `states()` and `runtimes()`) and is tried again only when that file changes or on `restart()`.
- **A restart reads the new binary first**: if it cannot, the old runtime keeps running and serving its rows.
- **A runtime that does not stop is ended**: on a restart or idle stop, a runtime's cleanups get `RUTIS_SHUTDOWN_TIMEOUT` (10 s by default) plus 2 s; then its process is killed, and the next one starts only once it is gone. Never two processes under one runtime name.
- **Manifest output is bounded**: a binary printing more than 16 MiB of manifest on stdout is refused; only the first 64 KiB of stderr is kept, and writing more no longer kills it with SIGPIPE.
- **Ends before supervising**: for a runtime that ends before the first `supervise`, only its latest end is kept; an application that will not supervise calls `unsupervise()` once, and none is kept from then on.

## rutis-host

`runtimes.go` takes `restart` (`"no"`, the default; `"on-failure"`; `"exit"`), as `node`, `py` and `bun` do; see [rutis-host](guide/rutis-host.en.md), "When a runtime ends". It is a new field only: existing `rutis.json` files are read as before, unchanged.

New (existing configurations are unaffected):

- `runtimes.swift`: Swift plugin binaries (macOS), with the same settings as `runtimes.go` (`start`, `idle`, `restart`); see [Swift plugins](guide/swift-plugin.en.md).
- A remote runtime's `language` accepts `"swift"` and `"kotlin"` / `"kt"`: app hosting ([Swift apps](guide/swift-app.en.md), [Kotlin apps](guide/kotlin-app.en.md)).

`rutis-host check` checks the plugin API of Go and Swift plugin binaries: below 3 (built with an SDK older than rutis 0.13) is an error with a non-zero exit status. Such binaries already could not connect since 0.13 (unified events); `check` now says so up front. Rebuild them with a current SDK.

## rutis-bridge

`session::Dispatch` and `peer::Handler` each gained a method with a default implementation (for the event test hooks): existing implementations compile unchanged.

Behaviour changes (no code changes):

- **A local runtime has 30 s to greet, not 10** (`LinkConfig::local_runtime()`): counted from starting the process, interpreter and JVM start-up included, so a slow but healthy machine is no longer taken for a broken runtime. A process that exits still fails at once; network links (remote runtimes, nodes) keep 10 s.
- **Error codes of `build.rs` exports**: for an error that is a struct with one field, `code` is now its type name (a string for a transparent newtype), not the field's name; enum variants keep the variant name. A plugin that checks the old `code` must change.
- **Wait cycles**: a listener run on top of a synchronous wait (on reentrant runtimes such as Python and Bun) that calls back into the waiting chain used to hang; now that call fails with `SyncWaitCycle` and the rest completes.
- **Closing a session on macOS**: when the host closes a session without exiting, a local runtime always reads the end and exits after its cleanup deadline; before, it now and then never read it and stayed.

## Events

Runtimes take emit replies in frame order, and rutis always answers a forwarded emit with a plain value ([event protocol](design-events-protocol-2026-10-18.en.md) §5.5). Plugin code is unchanged, and 0.13 hosts and runtimes connect as before.

For authors of runtimes and testing tools (protocol §5.5, [runtime contract](../.agents/skills/rutis-new-runtime/references/runtime-contract.md) §6):

- **The host always answers an `emit` with a plain value.** An `emit`-mode `events.dispatch` is answered `undefined` once its listeners have started, also when it waited in the session's queue (a filter selector still pending). 0.13 answered a queued emit with a future.
- **Runtimes apply that answer in frame order.** Take the reply as its frame is read, before the next frame, not on a later task, tick or thread. Start `emit`-mode `events.call`s in frame order too. A runtime that is not reentrant still holds them back while one of its synchronous calls waits. It must then run a fast-path emit of the same name behind the held calls, never before them. The Node runtime now does so; until now a later emit could take the fast path past them.
- **`events.expected.json`.** It has the step expectation `answered`: wait until the host has replied to every `events.dispatch` it received. It also adds the spec field `then` (`{until, args}`), the `unload` step's `then`, and the sync method `events-log.until(entry)`. A testing tool that reads the file must accept these keys. The cases that use `then` are `tool: false`.
- **`Tap::answered`** (`rutis_bridge::events::testing`, feature `testing`): waits for those replies, for your own host-side tests.
- **Node stays non-reentrant.** During a synchronous wait the Node runtime still holds emit-mode listener calls back and runs them in frame order once the wait ends, so a plugin never sees its state half changed by a synchronous call.
- **A row's queued emits go when it ends.** When a filter never answers and its row unloads, that row's queued emits are dropped without a report, and the session's later emits go on.
- **Compatibility.** A 0.13 runtime still works with a 0.15 host but keeps its timing-dependent order: it counts the answer on a later tick. A 0.15 runtime still settles a 0.13 host's future answer, and with such a host keeps 0.13's order.

## Go SDK

`ctx.Provide` or `ctx.On` on a row already unloaded (from a plain goroutine or `time.AfterFunc`, say) no longer panics: it writes `rutis: row <key> is not active: …` and returns a function that does nothing. Reserved names (`rutis/*`) still panic.

## Bun runtime

Dialing works again on Bun 1.4.4: an access point that refuses the upgrade fails the dial at once (it used to wait for the 10 s handshake deadline), and a rejected certificate is `auth-rejected` (it used to be retryable). Older Bun versions are unaffected.

## Rust services: mark the type `#[rutis::service]`

Add `#[rutis::service]` to a Rust service's `impl` block and provide it as usual: the rows of every language call it by name ([#510](https://github.com/arcships/rutis/issues/510), [Rust host guide](guide/rust-host.en.md#mark-the-type-rutisservice)). No `build.rs` list, `export_all` or hand-written `HostDispatch`; built-in and dylib plugins alike.

**No code changes needed**: `build.rs` `Exports::service(..)` / `service_in(..)` and hand-written `HostDispatch`es keep working. To switch a type to the marker: mark its `impl` and remove its line from `build.rs` (having both is a build error); a service provided in an instance needs no instance group any more. When `build.rs` only writes declarations, write `Exports::new().typescript("…").generate()`, or use `rutis-host types <crate dir> -o services.d.ts` instead.

New (existing code is not affected):

- `rutis::service` (the macro) and `rutis::CallError`: what a function a marked method was passed fails with; a function parameter's error type converts from it (`CordisError`, `anyhow::Error` and boxed errors do).
- `rutis::TypeKey::type_name()` is public.
- `rutis_loader::Loader::waiting_for(&entry)`: which services a row waits for, and how to fix it when one is an unmarked Rust service.
- `rutis-host types`.
- `rutis-host`'s `rutis-bridge` dependency enables the `exports` feature.
- `rutis_loader::Provider::Marked(type)`: `Loader::check` reports a marked type's name with it (served once provided). `Provider` is `#[non_exhaustive]`, so matches already have a catch-all.
- A marked type in a dylib plugin is served by the build that provided the value: rebuilding at the same version and reloading needs no version bump.
- Calls by name to a marked service pick read interceptors (`intercept_require_as`) by the **calling** row, as a native `require` does.
- The new crate `rutis-macros` holds the macro; `rutis` depends on it and re-exports it, so nothing is added to `Cargo.toml`. A dylib plugin reaches it as `rutis_sdk::rutis::service`.
