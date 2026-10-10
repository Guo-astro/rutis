# The Bun runtime

[中文](design-bun-runtime-2026-10-09.md)

Design proposal · Related to [#194](https://github.com/arcships/rutis/issues/194)

## 1. Background

The [multi-language plugin design](design-multilang-runtimes-2026-10-03.en.md) sets out how a language is connected: a runtime process implements the runtime contract (§5), alongside a leaf SDK (§6). The rutis core does not change, and plugins run as loader rows. The Python runtime was connected this way, and it is the reference implementation this design is checked against.

Bun is a JS / TS runtime of its own. It has its own module resolution, package manager and networking APIs, runs TS natively, and can compile a program into a single-file executable. This design connects a **Bun runtime** the same way:

- **The runtime**: `rutis-bun`, which implements the runtime contract in a Bun process with its own session implementation.
- **Rows**: named `bun:<module>`.
- **Rust**: its own launcher, cargo feature and configuration.
- **Distribution**: an npm package (needs Bun installed) and a single-file executable (does not).
- **Verification**: the same conformance tests as the other runtimes.

[M1](design-multilang-m1-2026-10-04.en.md) §11.4 chose not to "open a separate pure JS runtime" at the time. This design is a new decision, made in [#194](https://github.com/arcships/rutis/issues/194): Bun is added as a runtime of its own. §11.4 was about M1's approach to leaf plugins and does not constrain this design.

## 2. Bun findings

Tested with Bun 1.3.14 on macOS and checked against Bun's official documentation (bun.com/docs). "Docs" marks what the documentation says; "tested" marks what was verified locally.

| Capability | Under Bun | Consequence for the design |
| --- | --- | --- |
| `worker_threads`, SharedArrayBuffer, `Atomics.wait` on the main thread, `receiveMessageOnPort` | Tested: work. Docs: `worker_threads` is partial (`moveMessagePortToContext` and others are missing), and terminating a Web Worker is marked experimental | I/O lives in a worker and the main thread blocks (§3.4). Worker crash and exit paths need tests |
| An inherited socket (fd:3) | Docs: `new net.Socket({ fd })` cannot read an existing fd; tested: it fails silently. Tested: `net.connect({ fd })` and `Bun.connect({ fd, socket })` work | Use `Bun.connect({ fd })`. Docs: native sockets do not buffer, so the runtime handles backpressure itself |
| Unix sockets, loopback TCP | Work | Dial-back and the loopback handover |
| WebSocket server (`Bun.serve`) | Works. Docs, defaults: `maxPayloadLength` 16MB, `idleTimeout` 120s, `backpressureLimit` 16MB | Remote runtimes listen with it. `idleTimeout` must be off or longer than the heartbeat, and the limits must match rutis's (§4) |
| Importing a module again | Tested: a `file://` URL with a query string returns the old module. An absolute path with a query string gives a new instance, but the old instances stay cached for good. Deleting `require.cache[realpath]` and importing again gives the new module, reloading only the entry. Docs: there is no public way to invalidate modules programmatically (`--hot` uses `Loader.registry` internally, which is not exposed) | Use `require.cache` (§3.8). The docs do not promise this behaviour, so the nightly job on the latest Bun must cover it |
| Auto-install | Docs: `install.auto` defaults to `auto`, which installs automatically when there is no node_modules. Tested: `--no-install` overrides `auto = "force"` in the project's bunfig | Always pass `--no-install` |
| bunfig and `.env` | Docs and tested: `bunfig.toml` is read from the **working directory**, and its `preload` runs before the entry. `.env` is loaded automatically | The working directory is the project, and the project's bunfig counts as the project's trusted configuration. Always pass `--no-env-file`, so the environment comes only from the host (§3.1) |
| Lifecycle scripts in `bun install` | Docs: by default only dependencies on a built-in trusted list run their scripts. A project's `trustedDependencies` **replaces** that list | Goes in the plugin project guide |
| TS | Docs and tested: runs natively. tsconfig `paths` are supported. Decorator semantics follow tsconfig: since 1.3.10, without `experimentalDecorators`, Bun uses TC39 standard decorators. **No type checking** | The template includes `tsc --noEmit` |
| WebSocket client | Tested: a refused handshake fires `error` and then closes with 1002; **the status code is not available** | The runtime never dials (§4), so this does not matter |
| `bun build --compile` | Tested: a single file of about 61MB that dynamically imports external `.ts` plugins from disk and reloads them after a `require.cache` delete. Docs: since 1.4, compiled executables do not read tsconfig or package.json unless built with `--compile-autoload-tsconfig` / `--compile-autoload-package-json`. `BUN_BE_BUN=1` makes the executable behave as the full bun CLI | The second form of distribution (§6) |
| `--no-orphans` | Docs: when the parent exits, the process exits and SIGKILLs all its descendants (Linux, macOS) | Passed at launch, on top of `kill_on_drop` |
| `node:test` / `async_hooks` | Docs: `node:test` is partial; `async_hooks` implements only `AsyncLocalStorage` | The runtime's own tests use `bun:test`; async context uses only ALS |
| Versions | Docs: no LTS and no semver promise; the latest is 1.4.2 | See §7 |

## 3. The runtime `rutis-bun`

### 3.1 The process

```
bun --no-install --no-env-file --no-orphans <package>/src/main.ts <channel> [--id <endpoint> --peer <endpoint>] <project>
rutis-bun <channel> [--id <endpoint> --peer <endpoint>] <project>          # single-file executable
```

- **Arguments**: the same as the Python runtime's (`python3 -m rutis <channel> [--id] [--peer] <project>`).
- **Working directory**: the project directory.
- **`--no-install` and `--no-env-file`**: always passed, and configuration cannot remove them. The executable is built with auto-install and `.env` loading off, and ignores `BUN_BE_BUN`.
- **Plugins are trusted code**: the runtime is not a sandbox ([requirements](requirements-protocol-plugins.en.md) §7). The project's `bunfig.toml` is project configuration, and its `preload` is trusted like plugin code.

### 3.2 Channels and handover

| Channel | How |
| --- | --- |
| `fd:3` (Unix: the host creates the socket and the child inherits it) | `Bun.connect({ fd: 3 })`. The Rust launcher always uses inheritance, as for Python; it does not read a declaration in the package |
| A socket path (dial-back: the runtime connects to a socket the host gives it) | `Bun.connect({ unix })` |
| `tcp:<address>` (the loopback handover: the default on Windows, and on Unix with `RUTIS_LOCAL_HANDOVER=loopback`) | Sends `RUTIS_CHANNEL_TOKEN` first, then **deletes it from `process.env` at once**, as Python's `__main__.py` does |
| `listen:ws://…` / `listen:wss://…` | A remote runtime; see §4 |
| `ws://…` (dialing out) | Refused at startup with "the Bun runtime only listens" (remote plugins design §4.4) |

- **Framing**: messages are framed by lines, with a length limit consistent with [#173](https://github.com/arcships/rutis/issues/173). A message over the limit closes the channel.
- **Half-close**: needs its own tests. Node has a known half-close problem on macOS, recorded in `node/rutis-runtime/src/channel/unix.mjs:20`.

### 3.3 The contract

**Protocol formats**: compat (protocol 2) locally; endpoint format (protocol 3, WebSocket subprotocol `rutis.3`) over the network.

**Capabilities** in the greeting: `signals` and `reentrant-sync`.

**The reply to `mount`**: `{ services: {}, features: ["rows.v2", "hosts", "leaf", "scopes"], implementation: { name: "rutis-bun", version }, engine: { name: "bun", version: Bun.version } }`. The Rust side parses the `mount` reply as loose JSON, so `implementation` and `engine` are added fields, not a protocol change. `rutis-host check` prints them.

**Control operations**: each one matches the Python runtime (`python/rutis/rutis/runner.py`).

| Operation | Notes |
| --- | --- |
| `mount` | See above |
| `dispose` | Unloads every row and waits for calls in flight to drain |
| `rows.load` | `(key, entry, config, isolate, inject, exports)`. `entry` is a module name that Bun resolves from the project: an npm package name, a subpath, or `./relative/path`. The services listed in `exports` are registered as export slots |
| `rows.update` | For a leaf plugin, a configuration change restarts the row |
| `rows.unload` | Runs the cleanup and withdraws the row's services |
| `rows.schema` | Returns the configuration schema, `inject`, `provides`, whether each method is synchronous or asynchronous, and `version` (from the plugin package's package.json) |
| `hosts.provide` / `hosts.withdraw` | When a host service appears or is withdrawn: `[name, methods, label?]` / `[id]` |
| `release` / `get` | Reference counting and reading properties |

**Plugin API version**: a plugin that declares an `api` newer than the runtime supports is refused, with an error that says to upgrade `rutis-bun` ([developer packages design](design-developer-packages-2026-10-06.en.md) §5.1).

### 3.4 Synchronous waits and re-entry

I/O runs in a worker. When a plugin makes a synchronous call, the main thread blocks in `Atomics.wait`. **Every call that arrives meanwhile runs on the main thread**, whether or not it belongs to the waiting call chain, as in the Python runtime (`peer.py:9-14`).

- **Why**: synchronous calls between Bun and any other runtime, in either direction, cannot deadlock.
- **The cost**: a plugin's service may be called while that plugin is itself inside a synchronous call. The guide says so: do not hold a lock across a call into rutis.

`SyncWaitCycle` is returned in exactly one case: the awaited result needs the event loop, which the parent synchronous call is holding (compare `peer.py:843-850`).

### 3.5 Services inside instances

Implemented as in the [instance services design](design-instance-services-2026-10-08.en.md) §4.2:

- A service's identity is `(name, label)`, with the id `name\0label`.
- Export slots, handles, host proxies and `host:<id>` are all registered by id.
- Handles carry a generation, `id#N`, and `\0N` when scoped.

Plugins in the same runtime that use each other get the object directly, but only after looking up the id in the row's isolate table.

### 3.6 Cancellation, errors and process exit

- **Cancellation**: a `signal` in the arguments is decoded into a real `AbortSignal` that lives until the result settles, and a `cancel` aborts it.
- **Errors**: error graphs round-trip intact, including cause, AggregateError, custom names and fields. The extra fields Bun puts on errors (such as `line`, `column`, `sourceURL`) must **not** leak into the error shape.
- **Uncaught exceptions and rejections**: they end the process, and all of this runtime's services are withdrawn with it (requirements §5, rule 8).

### 3.7 Remote leases

See §4.

### 3.8 Hot reload

When the entry file changes (by mtime and size), the runtime deletes `realpathSync(entry)` from `require.cache` and imports the entry again.

- Only the entry module is replaced; the modules it imports stay cached. To replace those too, restart the runtime.
- No query strings: old instances imported with a query string stay in the cache for good.
- No `--hot`: it re-runs the whole process, which does not fit reloading one row at a time.

### 3.9 Plugin SDK and runtime code

- **Plugin declarations**: plugins are declared with `definePlugin` from `@arcships/rutis`. That package holds only the declaration shape and the testing helpers (`index.mjs`, `testing.mjs`), and depends on no runtime. Its `engines` lists only `node` today; it gains `bun`.
- **Testing helpers**: that `@arcships/rutis/testing` works under `bun test` is an acceptance criterion.
- **Session layer, channels and leaf loading**: `rutis-bun` **keeps its own copy** and does not depend on `@arcships/rutis-runtime`. That package's leaf loading depends on a Cordis Context, and it does not export its internal modules. The existing code can serve as a reference, but the code belongs to `rutis-bun`, and the conformance tests decide what is correct.

### 3.10 Scope

The first version runs only leaf plugins (`definePlugin`). Mounting Cordis plugins, and connecting a Cordis application as a node, are outside this design.

## 4. Remote runtimes (listening and leases)

`rutis-bun listen:ws://… --id <endpoint> [--peer <controller>] <project>` is implemented per the [remote plugins design](design-remote-plugins-2026-10-03.en.md) §4.4, matching Python's `rutis[network]`:

- **Listening**: it only listens and never dials. Without TLS, it accepts only loopback addresses.
- **Authentication**:
  - Credentials come from `RUTIS_TOKEN`, `RUTIS_CERT` and `RUTIS_KEY`.
  - Tokens are compared in a way that resists timing attacks.
  - Refusals are told apart, as in `python/rutis/rutis/websocket.py:147-161`: 404 for the wrong path, 401 for missing credentials, 403 for a wrong token, 400 for the wrong subprotocol.
- **Messages and heartbeat**:
  - A message is limited to 16 MiB; one over the limit closes with 1009.
  - The heartbeat defaults are Python's (dropped after 30 s without an answer), adjustable with `RUTIS_HEARTBEAT`.
  - `Bun.serve`'s `idleTimeout` is turned off; the heartbeat is what detects broken connections.
- **Takeover**: a new connection is handled in this order:
  1. authenticate it;
  2. close the old session's channel and read no further frames from it;
  3. clean up every row and proxy of the old lease;
  4. answer the new session's `hello`.

  The replaced connection closes with 4002. Leases are cleaned up in the process (as in Python), and the module cache survives across leases.
- **Startup output**: once listening, it prints `rutis: listening on …` on stderr.

## 5. Rust and configuration

### 5.1 rutis-bridge and rutis-loader

```rust
Launcher::bun(program, package)        // bun --no-install --no-env-file --no-orphans <package>/src/main.ts, always inherits fd:3
Launcher::bun_executable(program)      // the rutis-bun single-file executable
LocalRuntime::bun(launcher, project)   // named "bun"
RuntimeResolver::modules(handle)       // existing: rows "bun:<module>"
```

- **Cargo feature**: a new feature `bun` in rutis-bridge and rutis-loader, alongside `node` and `python`.
- **Row names**: resolve through the existing `Naming::Modules`, with the runtime name `bun:` as the prefix.
- **Versions**: come from `version` in `rows.schema`. `RowSchema` gains a `version` field, written into the row's meta. Python entry point versions are dropped today, and this fixes that too.
- **Name collisions**:
  - The row `bun:sqlite` names the project's module `sqlite`, **not** Bun's built-in `bun:sqlite`. A plugin that wants a built-in imports it in its own code. The guide says so.
  - `Naming::Npm` of a remote node runtime currently accepts any name that is not a file (`rutis-loader/src/runtime.rs:90-94`), including `bun:x`. It stops accepting names that carry a known runtime prefix.
  - Duplicate runtime names, among the local `node`, `py` and `bun` and the remote runtimes, are configuration errors.

### 5.2 rutis.json

```json
{
  "runtimes": {
    "bun": { "project": "." }
  },
  "rows": [
    { "id": "weather", "name": "bun:@foo/weather" },
    { "id": "report", "name": "bun:./report.ts", "inject": ["weather"] }
  ]
}
```

| Field | Default | Meaning |
| --- | --- | --- |
| `project` | `.` | Plugins resolve from this `package.json`; also the working directory |
| `runtime` | The project's `@arcships/rutis-bun`, then a `rutis-bun` executable on `PATH` | Where the runtime is: an npm package directory or an executable |
| `program` | `bun` on `PATH` | The Bun executable; unused when `runtime` is the single-file executable |

- **A missing runtime**: startup fails and names both ways to install one.
- **Remote runtimes**: `remote` gains the `language` `"bun"` (`rutis-host/src/host.rs:78-83`), with rows named `<remote runtime name>:<module>`.
- **Other runtimes**: `runtimes.bun` is independent of `runtimes.node` and `runtimes.py`, and they can all be configured together. Services are shared by name through `host_key`.

### 5.3 rutis-host

- **`new <name> --lang bun`**: generates `package.json`, `src/index.ts`, a `bun:test` test, `tsconfig.json` (with a `tsc --noEmit` check script) and `rutis.dev.json`.
- **`dev`**: reads `rutis.dev.json` before choosing the runtime; without it, a `bun.lock` marks a Bun project. Today `project.rs:21-22` chooses Node as soon as it sees `package.json`, and that changes.
- **`check`**: lists each `bun:` row's version, dependencies, services and configuration schema, and the runtime's implementation and Bun version.

## 6. Distribution

What is distributed is the runtime (the protocol implementation plus plugin loading); the plugin SDK is still `@arcships/rutis`. It comes in two forms:

| Form | Contents | Needs |
| --- | --- | --- |
| The npm package `@arcships/rutis-bun` | `src/main.ts` and the rest of the source | Bun installed |
| The single-file executable `rutis-bun-<version>-<platform>` | Built by `bun build --compile`, with Bun embedded | Nothing |

- **Publishing the executable**: it goes into GitHub Releases (Linux and macOS on x64 / arm64, Windows x64), and can also ship as npm platform packages the way `@arcships/rutis-host` does. It is always built with auto-install off, `--compile-autoload-tsconfig` and `--compile-autoload-package-json`. Plugins are imported dynamically from the project directory, so the project's tsconfig `paths` still apply.
- **Machines with only Bun**: the npm `rutis-host` starts with `#!/usr/bin/env node` and does not run on such machines. Bun users take `rutis-host` from its binary distributions (GitHub Releases, `cargo install`). Whether `bunx --bun @arcships/rutis-host` works is to be checked in B3.
- **The release train**: gains `@arcships/rutis-bun` (`scripts/train.mjs`), and checks the implementation version constant in the runtime's code, as it does Python's `IMPLEMENTATION`.

## 7. Bun versions

Nothing this design uses depends on a recent version: `Bun.connect({ fd })`, `worker_threads` with `Atomics.wait` on the main thread, `require.cache`, `--no-install`, and WebSocket in `Bun.serve`. So **there is no artificial minimum**:

- **The npm form**: the supported minimum is the oldest version that passes in the CI matrix. It goes into `engines.bun`, and the runtime checks it at startup. The matrix has an older version (the oldest from 1.1.x / 1.2.x that passes), the latest stable release, and a nightly job tracking the newest.
- **The executable**: the embedded Bun is fixed at build time and does not depend on the user's machine.
- **Newer flags such as `--no-orphans`**: passed only when the Bun version supports them. Either the Rust side asks `bun --version` once, or the runtime handles it itself. They never raise the minimum.

## 8. Tests

**Conformance**

| Test | What it covers |
| --- | --- |
| Session contract, `runtime_conformance.rs` | A Bun endpoint, running every check of `session::testing` |
| Runtime contract, `session_matrix.rs` | {Bun} × {fd:3, dial-back, WebSocket listen}, plus a loopback column (run on Unix with `RUTIS_LOCAL_HANDOVER=loopback`) |
| Channel contract | `channel::testing::contract` takes only Rust channel pairs today; it first needs a harness with the Bun side as the echoing peer |
| Conformance fixtures | Bun versions of `conformance-session`, `conformance-weather` and `conformance-greeter`; `crash()` exits with status 17 |

**Session layer**

| Test | What it covers |
| --- | --- |
| `cancellation.rs` | `AbortSignal` |
| `error_shape.rs` | Error graphs round-trip intact, without Bun's extra fields |
| `rpc_callbacks.rs` | Callbacks and synchronous waits |
| `process_exit.rs` | An uncaught exception ends the process and its services are withdrawn |
| `live_objects.rs` | References and release |

**A Bun counterpart of `python_runtime.rs`**

- The features include `rows.v2`, `hosts`, `leaf` and `scopes`.
- A runtime that does not support the row contract fails to start.
- Unloading withdraws the services without waiting for the runtime.

**Loader**

- **`bun_rows.rs`**: loading; hot reload, including the realpath behind a `bun link` symlink; the old version serving on after a broken edit; the process exiting and restarting; a failed start not blocking resolution.
- **`leases.rs` and `remote_rows.rs`**: a Bun column. Controllers that connect one after another each get a clean lease; the old lease is cleaned up before a takeover; slow starts with slow cleanups; a reconnect gets a new lease.
- **`multilang.rs`**: Bun, Python and Node using each other's services. **Synchronous calls in both directions at a cold start do not deadlock** (§3.4). M2's four rows starting together cold is the acceptance case.
- **`instance_runtimes.rs`**: service names inside instances.

**Host and launcher**

| Test | What it covers |
| --- | --- |
| rutis-host | Hot reload of a `bun:` row; a Bun version of `a_remote_runtime_runs_rows_named_after_it`; a project from `new --lang bun` passes `check`; `dev` recognises a Bun project; name collisions are configuration errors |
| Launcher | The three mandatory flags are always present; without node_modules, importing a package that is not installed fails rather than downloading it; the project's `.env` is not loaded; the hint for a missing runtime; dialing `ws://` is refused; `RUTIS_CHANNEL_TOKEN` is deleted after use |

**The runtime itself and the SDK** (`bun test`, with `bun:test`)

- Channels, session, synchronous re-entry, hot reload, schema, and instance scopes (compare `test_scopes.py`).
- WebSocket listening: authentication, the loopback restriction, the size limit, 4002, the heartbeat (compare `test_websocket.py`).
- Versions (compare `test_entry_points.py`).
- `@arcships/rutis/testing` under `bun test`.

**E2E**

- S2 ([#186](https://github.com/arcships/rutis/issues/186)): the development loop of `new --lang bun`.
- S3 ([#187](https://github.com/arcships/rutis/issues/187)): Bun rows in cross-language composition and crash recovery.
- S9 ([#193](https://github.com/arcships/rutis/issues/193)): the single-file executable in a clean environment without Bun.

**CI**

- **The `runtimes-bun` job**: on Linux and macOS, installs each version in the §7 matrix with `oven-sh/setup-bun`, then runs `bun test`, `cargo test -p rutis-bridge --features bun,…`, `cargo test -p rutis-loader --features bun,…` and `cargo test -p rutis-host`.
- **Single-feature build**: `cargo check --no-default-features --features bun`.
- **Executable smoke test**: build the executable and run the conformance tests in an environment without Bun.
- **Nightly**: tracks the latest Bun, watching the `require.cache` reload, `Bun.connect({ fd })` and the WebSocket server defaults.
- **Windows**: the existing `runtimes-windows` job already runs the Python runtime. Bun joins it in B3, starting with the loopback column.

## 9. Open questions

1. **Windows**: the loopback handover, job object adoption and `kill_on_drop` under Bun.
2. **Performance**: numbers for the synchronous call round trip and for throughput.
3. **Cordis plugins**: whether they need mounting in the Bun runtime (§3.10).
4. **`bunx --bun @arcships/rutis-host`**: whether it works decides whether the npm `rutis-host` can serve Bun-only users.

## 10. Phases

| Phase | Contents |
| --- | --- |
| B1 | The npm form of `rutis-bun`: local channels (fd, dial-back, loopback), the session, every control operation, synchronous re-entry, instance scopes, cancellation and errors, hot reload. Rust: the `bun` feature, the launcher, `LocalRuntime::bun`, `RowSchema.version`, the name collision checks, and `runtimes.bun`. The local tests of §8; the CI job `runtimes-bun` |
| B2 | Remote runtimes: listening and leases (§4); `language: "bun"` for `remote`, with the lease tests |
| B3 | The single-file executable and its releases; `rutis-host new --lang bun` / `dev`; Windows; performance numbers |
