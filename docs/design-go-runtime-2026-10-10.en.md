# The Go Plugin Runtime (Design Draft)

[中文](design-go-runtime-2026-10-10.md)

Status: G1 and G2 implemented (#227; differences in §17). Date: 2026-10-10. Baseline: `main` `fafc595`.
Based on: [the multilingual design](design-multilang-runtimes-2026-10-03.en.md) ("the overall design"; this is its M4 and revises two points, see Appendix A.3), [M1](design-multilang-m1-2026-10-04.en.md), [M2](design-multilang-m2-2026-10-04.en.md), [instance services](design-instance-services-2026-10-08.en.md), [the plugin API](guide/plugin-api.en.md), [the Bun runtime](design-bun-runtime-2026-10-09.en.md) (aligned on channels, the `mount` reply, remote listening, name conflicts, tests).

Out of scope: Cordis in Go; Go's `plugin` package; the host compiling Go code at run time.

## 1. Decisions

| # | Decision | Appendix |
| --- | --- | --- |
| D1 | One runtime process per binary; the host runs several Go runtimes at once | A |
| D2 | A binary holds one or more plugins, as its `main.go`'s `rutis.Serve(...)` says | A |
| D3 | Rows are `go:<plugin>`, routed by `GoResolver` through manifests; qualified form `<runtime name>:<plugin>` | — |
| D4 | The runtime name comes from the file name and is the only user-visible name | — |
| D5 | Binaries describe themselves: `--rutis-manifest` prints a manifest; resolution needs no running process | — |
| D6 | While a process runs, resolution uses its launch manifest; a replaced file takes effect on restart; no automatic restart | E |
| D7 | Start on demand, stop when idle; `eager` starts all and never idle-stops | — |
| D8 | Calls within a binary skip IPC; across binaries and languages they go through rutis by name | — |
| D9 | One goroutine per incoming call: the runtime is reentrant | G |
| D10 | The call chain and cancellation live in `context.Context`; plugins must pass it on | D |
| D11 | Leaf plugins; plugin API 1; configuration is a struct, schema by reflection | — |
| D12 | Service methods are `async` by default; `rutis.Sync(...)` marks synchronous ones | B |
| D13 | Structs are copied as data; objects cross by reference only via `rutis.Ref(v)` | C |
| D14 | Services are used by binding a struct of function fields | — |
| D15 | The host does not compile; `run` runs existing binaries, `dev` rebuilds only the project's own | — |
| D16 | Binaries built with different SDK versions coexist; host and binary share only the wire protocol, manifest format and plugin API version | — |
| D17 | The SDK uses the standard library only and lives in `go/rutis` | — |

## 2. Runtime model

```text
rutis host
 ├─ LocalRuntime::node / ::python ...
 ├─ GoRuntimes (GoResolver + lifecycle)
 │    ├─ plugins/go/weather ── go-weather ── weather
 │    ├─ plugins/go/netkit  ── go-netkit  ── ping, dns, traceroute
 │    └─ plugins/go/k8s     ── not started (no row uses it)
 └─ LoaderPlugin + one RuntimeRowsPlugin per running runtime
```

- A row depends on `RuntimeRows#<runtime name>` and on every name its plugin injects (leaf runtime: rutis gates every name).
- Services in `provides` are published from the row's fiber at `host_key(name)`.
- Process exit: all its rows stop, and so do rows using their services.
- `isolate`, instance labels, service ids (name + NUL + label), export handles: as in the Python runtime.

## 3. Binaries and manifests

### 3.1 Binaries

- Authors distribute a Go package (exporting `Plugin`) and `cmd/<name>/main.go` (serving their plugins).
- The host accepts any executable answering `--rutis-manifest` (sources: Appendix H).

```go
// example.com/netkit/cmd/netkit/main.go
func main() { rutis.Serve(ping.Plugin, dns.Plugin) }
```

| `Serve` rule | |
| --- | --- |
| Arguments | `--rutis-manifest`: print the manifest and exit; otherwise `<channel> [--id <endpoint>] [--peer <endpoint>] <project>` |
| Duplicate plugin names | exit at start, naming both packages |
| Registration | only `Serve`'s arguments; no `init()` |
| Subcommands | `rutis.ServeArgs(args []string) error` |

### 3.2 Manifest

```json
{
  "manifest": 1,
  "sdk": "0.9.0",
  "pluginApi": 1,
  "plugins": {
    "weather": {
      "config": { "type": "object", "properties": { "city": { "type": "string" } } },
      "inject": ["llm"],
      "provides": { "weather": { "today": "async", "unit": "sync" } },
      "version": "v1.2.0"
    }
  }
}
```

| Item | Rule |
| --- | --- |
| Output | stdout, exit code 0; no channel, no `Apply` |
| Plugin entry | identical to the `rows.schema` reply, produced by the same code |
| `sdk` | the constant `rutis.Version` |
| Plugin `version` | `debug.ReadBuildInfo()`: the `Deps` version for a dependency module; `Main.Version` for the main module; `vcs.revision` (`+dirty` if modified) when that is `(devel)`; else `null` |
| Execution timeout | 30 seconds (see §17) |
| Start retried | while the file is busy (ETXTBSY: a file just written, still held open for writing by a process another thread is starting), at most 5 tries, 10 ms apart at first, doubling |
| Wrong platform | "not built for <os>/<arch>" |
| `pluginApi` too new | every plugin of the binary fails to resolve, naming what to upgrade |
| Cache key | path, size, mtime; plus ctime on Unix |
| Cache bypassed | starting a runtime, `GoRuntimes::restart`, `rutis-host check`; also, when a row asks for a plugin no binary has, manifests whose last read failed at least 5 seconds ago are read again (the failure may have passed; not sooner, so lookups of missing names do not rerun a binary that keeps failing or hangs) |
| Trust | only configured sources are executed (`binaries` files, `dir` directories); a `dir` is trusted as a whole and should be dedicated |

## 4. `GoResolver`

In rutis-loader (feature `go`), implementing `Resolver`.

| Item | Rule |
| --- | --- |
| Sources | files + directories; an unknown plugin name triggers one rescan |
| Candidates in a directory | executable, not starting with `.`, containing the SDK marker string (e.g. `rutis-go-runtime:1`); only candidates are executed |
| Candidate's manifest fails | skipped with a diagnostic, not a configuration error; `check` lists it and exits non-zero |
| A `binaries` file's manifest fails | configuration still loads; all its plugins fail to resolve, with the reason |
| Runtime name | strip extension (`.exe`) → lowercase → non-`[a-z0-9-]` to `-` → prefix `go-` (`net.kit_v1.exe` → `go-net-kit-v1`) |
| Runtime name used for | `Runtime#…`, `RuntimeRows#…` keys; endpoint id; the only name in qualified rows, `restart`, `runtimes()`, `check`, ambiguity messages. The file name appears only in meta `binary` |
| Name clash | with another runtime (`node`, `py`, `bun`, remote) or each other: configuration error for `binaries`; skip + diagnostic for directories |
| `go:<plugin>` | search all manifests. None: `NotFound`. Several: ambiguity error listing runtimes, suggesting `<runtime name>:<plugin>` |
| `<runtime name>:<plugin>` | claimed when it starts with one of its runtime names and `:`; only that manifest is searched |
| Resolution | as `RuntimeResolver`'s leaf result; the code building `Resolved` from declarations is shared |
| meta | `{ source: "go", binary, runtime, version, sdk }` |
| File replaced while running | resolution uses the launch manifest while the process runs; plugins only the new binary has fail with "replaced; takes effect after `restart(\"<runtime name>\")`"; `check` / `runtimes()` show "replaced, restart pending". Without a process, the manifest on disk is used |
| Second release stage | `RuntimeRows` stays; `RuntimeRowsPlugin` takes a trait (runtime name + rows to refresh), which `GoResolver` implements per runtime; rows to refresh = those whose manifest changed after launch |

## 5. `GoRuntimes`

In rutis-loader, mounted after the loader.

```rust
let go = Arc::new(GoResolver::new(GoBinaries::new().dir("plugins/go").file("bin/weather")).with_catalog(&catalog));
chain = chain.with_shared(go.clone());
root.plugin(GoRuntimes::new(go, project).idle(Duration::from_secs(60)));
```

| Item | Rule |
| --- | --- |
| Structure | each running runtime is a child `Ctx`: `LocalRuntime::go(binary, project).named(name)` + `RuntimeRowsPlugin`; stopping disposes the child `Ctx` |
| Start | when `GoResolver` resolves a row whose runtime is not running; cold starts run in parallel |
| Idle stop | on-demand runtimes only: after the last row unloads, wait `idle`; stop if no enabled entry in `Loader::entries()` resolves to it; a new resolution meanwhile cancels the stop |
| Crash | rows stop, reason recorded, no automatic restart; resolution does not restart it. Restarted by `GoRuntimes::restart(name)` or a changed binary file |
| `eager()` | starts all at mount; no idle stop |
| `runtimes()` | runtime name, state (not started / starting / running / stopped + reason / replaced, restart pending), plugins, versions |

An application with one fixed binary may skip `GoResolver`: `LocalRuntime::go` + `RuntimeResolver::modules` (rows `<runtime name>:<plugin>`).

## 6. The Go SDK

### 6.1 A plugin

```go
type Config struct {
	City string `json:"city,omitempty" doc:"the city to report on"`
}

type LLM struct {
	Ask func(ctx context.Context, question string) (string, error)
}

type Weather struct{ llm LLM; city string }

func (w *Weather) Today(ctx context.Context) (string, error) {
	answer, err := w.llm.Ask(ctx, "weather in "+w.city)
	if err != nil {
		return "", err
	}
	return answer + " in " + w.city, nil
}

func (w *Weather) Unit() string { return "celsius" }

var Plugin = rutis.Define(rutis.Plugin[Config]{
	Name:     "weather",
	Inject:   []string{"llm"},
	Provides: rutis.Provides{"weather": rutis.MethodsOf[*Weather](rutis.Sync("Unit"))},
	Apply: func(ctx *rutis.Ctx, config Config) error {
		var llm LLM
		if err := ctx.Use("llm", &llm); err != nil {
			return err
		}
		ctx.Provide("weather", &Weather{llm: llm, city: config.City})
		return nil
	},
})
```

| API | Rule |
| --- | --- |
| `rutis.Define` | returns `*rutis.Definition`, without a type parameter |
| `Apply` returns an error | the load fails; registered cleanups run |
| `*rutis.Ctx` | implements `context.Context`, cancelled at row unload |
| `ctx.Use(name, &target)` | `target`: a function-field struct or `*rutis.Service`; `name` must be in `Inject` |
| `ctx.Provide(name, value)` | until unload or the returned function is called; `value`'s type must match `Provides` |
| `ctx.Effect(cleanup)` | `cleanup(context.Context) error`, run in reverse at unload |
| `ctx.Go(fn)` | runs on a goroutine, recovers panics to stderr; cancelled and awaited at unload |
| `*rutis.Service` | `svc.Call(ctx, "ask", &answer, args...)` |
| `rutis.NoConfig` | no configuration |
| `Plugin.Schema` | a given schema; no reflection |

### 6.2 Function-field structs

```go
type LLM struct {
	Ask    func(ctx context.Context, question string) (string, error)
	Models func(ctx context.Context) ([]string, error) `rutis:"listModels"`
}
```

- Field names map to method names by §6.4; `rutis:"…"` overrides.
- List only the methods used; each field must match a declared method, else `Use` fails.
- Signature: optional first `context.Context`; last result `error`, at most one result before it.
- Provider in the same process: its methods are called directly, without copying; differing signatures are converted once in-process by §6.5.

### 6.3 Configuration schema

| Go | JSON Schema |
| --- | --- |
| `string` / `bool` / integers / floats | `string` / `boolean` / `integer` / `number` |
| slices, arrays | `array` + `items` |
| `map[string]T` | `object` + `additionalProperties` |
| structs | `object` + `properties`; fields without `omitempty` are `required` |
| `json:"-"` | omitted |
| `doc:"…"` | `description` |
| implements `rutis.Schemer` | its own schema |

`encoding/json` decodes the configuration before `Apply`; failure fails the load.

### 6.4 Methods

| Item | Rule |
| --- | --- |
| Service methods | exported methods of the type in `Provides` |
| Wire name | first letter lowercased; a leading run of capitals lowercased as a whole, except that if a lowercase letter follows, the run's last capital starts the next word (`Today`→`today`, `ID`→`id`, `URLFor`→`urlFor`, `HTTPServer`→`httpServer`, `GetURL`→`getURL`); `rutis.Rename` overrides; clashes fail `Define` |
| Signature | `func (T) M([ctx context.Context,] params...) ([result,] [error])` |
| Arguments | decoded by position; extra is an error; missing are zero values |
| Errors | non-nil `error` is thrown; panics become a `Panic` error with the stack |
| Shape | `MethodsOf` defaults to `async`; `rutis.Sync(...)` marks `sync` |
| `async` execution | runs on a goroutine, replies with an asynchronous result reference; caller cancellation cancels the method's ctx |

### 6.5 Values

| Go value | Across processes |
| --- | --- |
| `nil`, booleans, numbers, strings | copied; integers beyond ±2^53−1 are errors |
| slices, arrays, `map[string]T`, structs | copied recursively per `json` tags; `json.Marshaler` encodes itself |
| `error` | name and message copied; name = type name (`*fs.PathError`→`PathError`) or `Name() string` |
| functions | by reference |
| `rutis.Ref(v)` | by reference; the proxy calls `v`'s exported methods |
| values given to `ctx.Provide` | always by reference |
| `[]byte`, channels, `complex`, non-string-key maps, cycles | cannot cross |

Receiving: function types → Go functions calling the remote one; `*rutis.Future` → `Await(ctx)`; otherwise `encoding/json`. Object references are not received (`objects` not declared).

## 7. Concurrency, call chain, cancellation

| Item | Rule |
| --- | --- |
| Session structure | one reader goroutine; one reply channel per outgoing call; a new goroutine per incoming `call` / `invoke` / `get` / `await` / control operation; one write lock; separate locks per table |
| Reentrancy | any incoming call runs at any time; declares `reentrant-sync` |
| Plugin obligations | service objects safe for concurrent use; no lock held across calls into other services |
| Call chain | incoming: `path + [id]` in the method's ctx; outgoing: `path` from the ctx given; `Apply` / cleanup ctx carry the `rows.load` / `rows.unload` chain |
| ctx checks | `nil` ctx panics; in development mode (`RUTIS_DEV=1`), a chainless ctx while an incoming synchronous call runs warns once with the call site |
| Incoming cancellation | a `cancel` frame cancels the method's ctx |
| Outgoing cancellation | `async`: send `cancel`, return `ctx.Err()` at once, drop the late result; `sync`: return `ctx.Err()` at once, drop the late result, the far end runs on |
| Row unload | cancels the row's `*rutis.Ctx` |
| `SyncWaitCycle` | never produced; received as `*rutis.RemoteError`, `errors.Is(err, rutis.ErrSyncWaitCycle)` |

## 8. The runtime process

### 8.1 Channels

| channel | Use | Implementation |
| --- | --- | --- |
| `fd:3` | Unix, inherited | `os.NewFile(3)` + `net.FileConn` |
| `unix:<path>` | dial back | `net.Dial("unix", …)` |
| `tcp:127.0.0.1:<port>` | Windows, local | send `RUTIS_CHANNEL_TOKEN` + newline first, then remove it from the environment |
| `listen:ws://…` / `listen:wss://…` | remote (G2) | §9 |
| `ws://…` | — | error at startup: listen only |

- Line framing; per-message length limit as in [#173](https://github.com/arcships/rutis/issues/173); exceeding it closes the channel.
- `project` is accepted and offered as `rutis.Project()`.

### 8.2 Contract

Features `["rows.v2", "hosts", "leaf", "scopes"]`; capabilities `signals`, `reentrant-sync`.

| Control operation | Behaviour |
| --- | --- |
| `mount` | `{ services: {}, features, implementation: { name, version }, engine: { name: "go", version: runtime.Version() } }` |
| `rows.schema(entry)` | `{ config, inject, provides, version }`; unknown entries are errors listing the plugins there are |
| `rows.load(key, entry, config, isolate, inject, exports)` | create the row and export slots, decode the configuration, run `Apply`; on failure unload the row, then report |
| `rows.update(key, config)` | unload, load again |
| `rows.unload(key)` | withdraw services → cancel the row ctx → cleanups in reverse |
| `hosts.provide(name, methods, label)` / `hosts.withdraw(id)` | register / withdraw rutis service proxies (`scopes` rules) |
| `release` / `get` | release an exported object / read an exported field |
| `dispose` | unload every row, wait for calls in progress |

- `service(id, handle, version)` goes out before `ctx.Provide` returns (before the `rows.load` reply).
- `implementation` / `engine` land with Bun B1 (#200); Python adds them then (`engine`: `python` + `platform.python_version()`).
- Implementation name = published package name: Python `rutis`, Bun `@arcships/rutis-bun`, Go `github.com/arcships/rutis/go/rutis`. `check` prints them in one format.

### 8.3 Exit and cleanup limits

| Case | Rule |
| --- | --- |
| Panic in a call, `Apply`, cleanup | recovered as an error |
| Unrecovered panic in a plugin goroutine | the process exits; all rows stop |
| `rows.unload`, `dispose` | no limit (as Python); rutis decides how long to wait |
| Session already ended | unload every row with 5 seconds in total for cleanups; then abandon the rest, log the rows on stderr, `os.Exit(0)` |
| Goroutines ignoring `ctx.Done()` | keep running after unload; not handled by the SDK (in the guide) |

## 9. Remote (G2)

`<binary> listen:wss://0.0.0.0:7443/rutis --id office-go --peer main <project>`

| Item | Rule |
| --- | --- |
| Protocol | endpoint format (protocol 3), subprotocol `rutis.3` |
| Controller | one at a time; a new connection takes over, greeted once the old lease is cleaned up; the old connection closes with 4002 |
| Credentials | `RUTIS_TOKEN`, `RUTIS_CERT`, `RUTIS_KEY`; constant-time token comparison |
| Without TLS | loopback only |
| Refusals | 404 path, 401 no credential, 403 wrong token, 400 wrong subprotocol |
| Message limit | 16 MiB, larger closes with 1009 |
| Heartbeat | disconnect after 30 seconds without an answer; `RUTIS_HEARTBEAT` |
| Startup output | stderr `rutis: listening on …` |
| Implementation | own server on `net/http`, text frames only |
| Host side | one `RuntimePlugin::remote(name)` per remote binary; rows `<name>:<plugin>`; declarations via `rows.schema` |

## 10. Rust changes

| Change | Where |
| --- | --- |
| `Launcher::go(binary, project)`: the program is the binary, `cwd = project`, `inherit_fd` on Unix | `rutis-bridge/src/runtime/process.rs` |
| `LocalRuntime::go(binary, project)`, named after the file by default | `rutis-bridge/src/runtime/local.rs` |
| features: rutis-bridge `go = []`; rutis-loader `go = ["runtimes", "rutis-bridge/go"]`; rutis-host's dependencies enable `go` | three `Cargo.toml`s |
| `GoResolver`, `GoBinaries`, `GoRuntimes` | `rutis-loader/src/runtime/go.rs` (new) |
| `RuntimeRowsPlugin` takes a trait | `rutis-loader/src/runtime/rows.rs` |
| `RowSchema.version` into meta (shared with Bun B1; whichever lands first) | `rutis-bridge`, `rutis-loader` |
| remote `Naming::Npm` refuses `go:` and `<Go runtime name>:` prefixes | `rutis-loader/src/runtime.rs` |

## 11. rutis-host

```json
{
  "runtimes": { "go": { "dir": "plugins/go", "binaries": ["bin/weather"], "start": "on-demand", "idle": 60 } },
  "rows": [{ "id": "weather", "name": "go:weather", "config": { "city": "Oslo" } }]
}
```

| Field | Rule |
| --- | --- |
| `dir` / `binaries` | at least one |
| `start` | `on-demand` (default) / `eager` |
| `idle` | seconds; `on-demand` only; with `eager` it is a configuration error |
| `remote[].language` | gains `go` |

| Command | Behaviour | Phase |
| --- | --- | --- |
| `check` | per binary: runtime name, platform, SDK / plugin API versions, plugins, implementation and engine; ambiguities; each row's binary; failed candidates | G2 |
| `dev` | without `rutis.dev.json`, a `go.mod` means a Go project; rebuild and restart per §12 | G2 |
| `new <name> --lang go` | plugin package, `cmd/<name>/main.go`, tests, `rutis.dev.json`, release workflow (tag → `go test`, `check`, binaries per platform) | G2 |
| `go add <module>@<version>` | `go install` into `dir`; an error without a toolchain | G2 |
| `go add <URL>` | download a prebuilt binary into `dir`, verify SHA-256 | G3 |
| `run` | needs no Go toolchain | — |

macOS quarantine: checked by `check` and at startup (reusing the dylib quarantine check), reported with the fix; never removed automatically.

## 12. New code

| Case | Flow |
| --- | --- |
| `dev` | watch `.go`, `go.mod`, `go.sum` → build to a new cache file `<name>-<n>` → on success replace the runtime's file, restart it, delete the old file; on failure print errors, the old process keeps running |
| Deployment | replace the file by renaming → `GoRuntimes::restart("<runtime name>")` (rutis-host: restart the host); before the restart, the launch manifest applies |
| Effect of a restart | the runtime's rows and their users stop; the new process's manifest is resolved and rows start in dependency order; other runtimes unaffected |

## 13. Repository and release

| Item | Rule |
| --- | --- |
| Location | `go/rutis`, module `github.com/arcships/rutis/go/rutis` |
| Packages | `rutis` (API, `Serve`, manifest), `rutis/rutistest`; session layer `internal/peer` |
| Version | `const Version` in `go/rutis/version.go`; used by `implementation.version` and manifest `sdk`; checked by regex in `scripts/train.mjs` |
| Release | tag `go/rutis/vX.Y.Z`, via the Go module proxy |
| Dependencies | standard library only; Go versions: the two current stable releases |
| Compatibility | within a protocol version, a binary from any SDK version runs on any host version, provided its plugin API is not above the host's |
| Docs | `docs/guide/go-plugin.md` (Chinese, English); a Go column in the plugin API document |

## 14. Tests

| Test | Where | Content |
| --- | --- | --- |
| Session unit | `go/rutis/internal/peer` | encoding, reference counting, callbacks during sync calls, cancellation, chain from ctx, concurrency |
| Session conformance | `rutis-bridge/tests/runtime_conformance.rs` + a Go `conformance` target | `session::testing::session` |
| Contract × channels | `rutis-bridge/tests/session_matrix.rs` gains `Go` | `runtime::testing::runtime`: `fd:3`, dial-back; WebSocket in G2 |
| Go versions of Python tests | a Go `python_runtime.rs`; Go columns in `cancellation.rs`, `error_shape.rs`, `rpc_callbacks.rs`, `process_exit.rs`, `live_objects.rs` | features; cancellation to ctx; error shapes (type names, `Panic`); services withdrawn on panic exit; references and release; fixtures `conformance-session/weather/greeter`, `crash()` exits 17 |
| Plugin tests without a host | `rutistest.Load(t, plugin, config, services)` | only `Inject` services; declared methods provided; cleanups ran; strict mode encodes per §6.5 |
| Manifest | `go/rutis` + `rutis-loader/tests/go_rows.rs` | matches `rows.schema`; cache hits do not execute; too-new plugin API and wrong platform errors |
| Several binaries | `go_rows.rs` | routing by plugin name; ambiguity and the qualified form; unmarked files not executed, failing files skipped; file replaced while running uses the launch manifest, works after `restart`; in-place replacement keeping size / mtime still read by `restart`; sync / async calls across binaries; one crash affects only its rows and users |
| Start on demand | `go_rows.rs` | unused not started; exits after idle; a new row meanwhile cancels; no restart after a crash, starts after a file change |
| Runtime conformance | `multilang.rs` (feature `go`) | Go leaf plugins; three languages cold-start and call each other; removing a provider stops only its users; `inject` gating |
| Crossing sync calls | `multilang.rs` | Node→Go→callback into Node (with ctx), Go↔Python, Go↔Go: no deadlock |
| Instances | `instance_runtimes.rs` | label isolation, per-instance services |
| Remote leases (G2) | Go columns in `leases.rs`, `remote_rows.rs` | as the Bun design's §8 |
| Windows | `runtimes-windows` | loopback; `dev` file replacement and restart |

- Fixtures: `crates/rutis-loader/tests/fixtures/go`, built into at least two binaries when tests start.
- CI `runtimes-go` (Linux, macOS, `actions/setup-go` two stable releases): `go test ./...`, `cargo test -p rutis-bridge --features go,…`, `-p rutis-loader --features go,…`, `-p rutis-host`; `cargo check --no-default-features --features go`.
- `test`, `network-macos`, `runtimes-windows` gain `actions/setup-go`; `runtimes-windows` gains `cargo test -p rutis-host`.
- E2E: S2 ([#186](https://github.com/arcships/rutis/issues/186)) the `new --lang go` loop; S3 ([#187](https://github.com/arcships/rutis/issues/187)) cross-language and crash recovery; S9 ([#193](https://github.com/arcships/rutis/issues/193)) a downloaded binary without a toolchain.

## 15. Phases

| Phase | Content | Done when |
| --- | --- | --- |
| G1 | SDK (session, leaf API, `Serve` over `fd`/`unix`/`tcp`, manifest, `rutistest`); `Launcher::go`, `LocalRuntime::go`, feature `go`; `GoResolver`; `GoRuntimes` (`eager` only); `RuntimeRowsPlugin` trait | §14 passes except start on demand and remote; several binaries in parallel; Linux / macOS / Windows |
| G2 | start on demand and idle stop; rutis-host (`runtimes.go`, `dev`, `check`, `new --lang go`, `go add <module>@<version>`); guide; remote | start-on-demand tests pass; `dev` restarts only its runtime after an edit; a remote Go runtime is controllable |
| G3 (on demand) | prebuilt downloads; receiving object references; generating a composing `main.go` | decided on demand |

## 16. Open questions

| Item | Current choice |
| --- | --- |
| How manifests are read | execute the binary; alternative: written into the file at build time and read directly (needs `go generate`) |
| `idle` default | 60 seconds; alternative: never stop by default |
| Whether `run` builds | no |
| Default method shape | `async` |
| Typed binding beyond function fields | not now (alternative: `go generate`) |
| Receiving object references | G3 |
| Lost-chain detection | development-mode warning only |

## 17. Implementation notes

G1 and G2 are implemented by #227. Where it differs from this document:

| Item | This document | Implementation |
| --- | --- | --- |
| `async` replies | an asynchronous result reference | the result itself, when the method returns; a `cancel` frame cancels the method's ctx (Rust settles an async method's reply whether it is a value or a future) |
| Manifest | §3.2's fields | also `runtime: "rutis-go-runtime:1"` (the SDK marker, which keeps it in the binary) and `platform` (`GOOS/GOARCH`, which `check` prints) |
| `ServeArgs` | `ServeArgs(args []string) error` | `ServeArgs(args []string, defs ...*Definition) error`; also `ServeConn(net.Conn, defs...)`, `Manifest(defs...)` |
| `*rutis.Service` | `Call`, `Methods` | also `Local()`: whether a plugin in this process provides the service |
| Test tool | `rutistest.Load` | `Load(t, plugin, config, services) *Loaded`; `Service(name)` with `Call` / `Bind`; the plugin runs in its real runtime across an in-memory session |
| §14's "Go columns" | in `cancellation.rs` and the like | those files are Node-only; Go's checks are in `rutis-bridge/tests/go_runtime.rs` |
| Start trigger | a row resolving | also a check every 100 ms: the loader may reuse a resolution without asking the resolver |
| After an idle stop | stopped + reason | back to "not started" |
| `runtimes.go` | `dir`, `binaries`, `start`, `idle` | also `project` (the runtimes' working directory) |
| Development builds | — | `GoBinaries::file_named`, `GoResolver::replace`, `read_go_manifest` |
| Remote npm naming | refuses `go:` and `<Go runtime name>:` | refuses any `<name>:` prefix |
| macOS quarantine | reuse the dylib check | `rutis-host check` checks with `xattr` and says what to do |
| Manifest timeout | 5 seconds | 30 seconds: on macOS a new binary's first run waits for the system's assessment, over 5 seconds when several start at once |
| Starting, restarting, stopping one runtime | §5: a row resolving to it during an idle stop cancels the stop | one at a time (the runtime is marked busy, a mark always cleared); a new process starts once the old one is gone; a restart whose caller stops waiting (a timeout around it) still goes on to the end, and the next restart waits for it. A row resolving to it during an idle stop does not cancel the stop: the runtime starts again as soon as the stop is over |
| `rows.load`'s `inject` | in the signature | ignored, as in Python: a row's own `inject` is what a Cordis row waits for in Cordis; a leaf runtime's rows are gated in rutis on what their plugins inject |
| `check` | prints implementation and engine | does not: they are in the `mount` reply and `check` starts no Go runtime; with Bun B1 |
| Not done | | Python's `mount` reply `implementation` / `engine` and `check` printing them (with Bun B1); G3 |

---

## Appendix

### A. One runtime per binary

**A.1 Go versus Node / Python**

| | Node / Python | Go |
| --- | --- | --- |
| Loading code at run time | yes | unreliable: `plugin` needs the same toolchain and dependencies, and cannot unload |
| Distributed as | source packages (npm / PyPI), installed into one environment | compiled binaries, per platform or via `go install` |
| Combining several authors' plugins | install into one environment | needs a toolchain and a full rebuild, and dependencies must merge (MVS: one version per module), which nobody can guarantee |

**A.2 Compared with the Python runtime**

| | Python | Go |
| --- | --- | --- |
| Processes | one per environment | one per binary, started when used |
| Loadable plugins | whatever the environment imports | fixed at build, listed in the manifest |
| Row name → process | prefix is the runtime name | routed through manifests |
| Declarations before start | none; second release stage | yes (manifest) |
| Dependency conflicts | one copy per environment | each binary independent |
| New code | re-import the module | replace the binary, restart that runtime |

**A.3 Revisions to the overall design**: it made Go "one executable per group of plugins, more only for isolation". Revised: (1) the author or composer of a binary decides the grouping, and several Go runtimes at once is normal; (2) row names usually carry no runtime name; `GoResolver` routes them.

**A.4 Cost of processes**: Go processes start in milliseconds to tens of milliseconds and idle at a few MB to a dozen or so; start on demand keeps unused binaries without a process; a deployment wanting one process can compose one binary, with the model unchanged.

### B. `async` by default

- To a Go caller there is no difference (a call always blocks the current goroutine).
- A `sync` method blocks a Node caller's event loop; Go methods often do network I/O.
- Not inferred from signatures: a synchronous method may call back into other services and needs ctx for the chain too.

### C. Structs as data, objects via `rutis.Ref`

A Go struct can be data or an object; "does it have methods" cannot tell them apart as it does in Python, so data is the default and references are explicit.

### D. ctx must be passed on

Node calls a Go method synchronously; the method synchronously calls back a function Node passed in, using `context.Background()`. The callback carries no chain, so Node defers it as unrelated while waiting for Go to return: each waits for the other. Go itself never deadlocks (it is always reentrant); the far end does.

### E. Launch manifest while running; cache key

- Newly resolved rows must agree with the running process; resolving by the file on disk would load newly declared rows into the old process, or report missing plugins.
- In-place replacement keeping size and mtime (`rsync -t`, unpacking, CI caches) would hit a stale cache: ctime still changes on Unix; Windows has no usable ctime, so the cache is bypassed at the moments that matter.
- Running executables: Windows cannot overwrite them (so `dev` builds to new file names); Unix cannot write them in place (so deployments rename over them).

### F. Cleanup limits

`rows.unload` / `dispose` have rutis waiting, so rutis sets the limit (`dispose_with_timeout`, process management). Once the session has ended nobody waits, so the runtime limits itself, lest stray goroutines keep the process from exiting. Work that must happen should not rely on cleanup alone.

### G. Reentrancy and the overall design's §9

- A goroutine per call and synchronous waits that block only the caller mean crossing synchronous calls Go↔Go, Go↔Node and Go↔Python cannot deadlock; the cost is as in Python: services are called concurrently.
- Every leaf runtime integrated or designed (Python, Bun, Go) is reentrant. This document proposes the rule "every leaf runtime is reentrant; only Cordis is not", pending Swift (M3), recorded in the overall design's §9. Crossing synchronous calls between two Cordis runtimes and Rust-side wait-cycle detection stay open there.

### H. Where binaries come from

1. Prebuilt binaries authors publish per platform;
2. `GOBIN=plugins/go go install <module>/cmd/<name>@<version>`;
3. A binary the deployer composes from several plugin packages (dependencies must merge; the deployer's responsibility);
4. The project binary `rutis-host dev` builds.
