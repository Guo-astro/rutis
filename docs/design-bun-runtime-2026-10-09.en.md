# The Bun runtime

[中文](design-bun-runtime-2026-10-09.md)

Design proposal · Related to [#194](https://github.com/arcships/rutis/issues/194)

## 1. Background

The [multi-language plugin design](design-multilang-runtimes-2026-10-03.en.md) sets out how a language is connected: a runtime process implements the runtime contract (§5), plus a leaf SDK (§6). The rutis core does not change, and plugins run as loader rows. The Python runtime was connected this way.

Bun is a JS / TS runtime of its own: it runs TS natively and has its own module resolution, package manager and networking APIs. This design connects a **Bun runtime** in the same way:

- its own runtime package, `@arcships/rutis-bun` (`bun/rutis-bun`), which implements the runtime contract in a Bun process;
- rows named `bun:<module>`, written the same way as Python's `py:<module>`;
- its own launcher, cargo feature and configuration on the Rust side;
- verified by the same conformance tests.

Against the criteria of the [design philosophy](design-philosophy.en.md) §7:

1. It lets a host connect to projects and machines that have only Bun.
2. It lives outside the core.
3. The protocol and the boundary rules do not change.
4. It has concrete users.
5. Its guarantees can be pinned by the existing conformance tests.

## 2. What Bun does

Tested on 2026-10-09 with Bun 1.3.14 on macOS, by running a JS prototype that implements the runtime contract:

| Capability | Under Bun | Consequence for this design |
| --- | --- | --- |
| Re-entry during a synchronous wait: `worker_threads`, SharedArrayBuffer, `Atomics.wait` on the main thread, `receiveMessageOnPort` | Works; a crashing worker still surfaces during a blocking wait | I/O lives in a worker; the main thread blocks and runs reverse calls of its own call chain (§3.3) |
| An inherited socket (fd:3) | `net.connect({ fd })` works. `new net.Socket({ fd })` **fails silently**: no error, no data, and the process exits on its own | Use only `net.connect({ fd })` |
| Unix sockets, loopback TCP | `net.createConnection` / `createServer` work | Dial-back and the Windows loopback handover are available (Windows is unverified) |
| WebSocket server | Works | A remote runtime can listen |
| WebSocket client | The built-in replacement for `ws` emits neither `upgrade` nor `unexpected-response`. A refused handshake (for example a 401) gives neither an event nor an error, and the connection hangs | Dialing has to be implemented by hand; see §5 |
| Importing a module again | `import(url + '?v=…')` **ignores the query string** and returns the old module. Deleting `require.cache[realpath]` and importing again returns the new one | Hot reload uses `require.cache`, keyed by the realpath (on macOS `/var` is `/private/var`) |
| Auto-install | With no node_modules, Bun resolves packages from its global cache and may even download them | **Always start with `bun --no-install`**. This keeps resolution within the project and is also a security requirement |
| TS | Runs natively: enums, parameter properties, decorators, `./a.js` resolving to `a.ts` | No extra TS loader is needed |
| Startup time | 29ms (median of 10) | — |

Throughput, memory and Windows have not been measured.

## 3. The runtime package `@arcships/rutis-bun`

### 3.1 The process

```
bun --no-install <package>/src/main.ts <channel> <anchor>
```

- `<channel>` uses the same handovers as the other runtimes: `fd:3` (inherited on Unix), a socket path (dial-back), or `tcp:<address>` (the Windows loopback, with a one-time token).
- `<anchor>` is the project's `package.json`; modules resolve from it.
- At startup the runtime checks Bun's version. Below the supported minimum (1.3 for now) it exits with a clear error before greeting.
- The greeting reports the implementation and its version (`rutis-bun 0.9.0`, `bun 1.3.14`), visible in `rutis-host check` and `diagnostics()`.

### 3.2 The contract

The runtime implements the runtime contract of the [multi-language plugin design](design-multilang-runtimes-2026-10-03.en.md) §5:

- **The session layer** (values, references, callbacks, cancellation, release counts). `rutis-bridge`'s session conformance tests are the reference.
- **`rows.load` / `rows.schema`.** `entry` is a module name, resolved by Bun from the anchor: an npm package name, a subpath of a package, or `./relative/path`. `rows.schema` returns the configuration schema, the dependencies, the services provided, and whether each method is synchronous or asynchronous.
- **Services a row provides** are reported to rutis through service slot notifications.
- **`ctx.use(name)`** returns a proxy that calls by name, through `host:<name>`.
- **Plugins in the same runtime** that use each other get the object directly.

### 3.3 Synchronous waits

I/O runs in a worker. After the main thread makes a synchronous call, it blocks in `Atomics.wait`. While it waits:

- a reverse call that belongs to its call chain (judged by `path`) runs on the main thread;
- any other call is queued.

This is the same model as the Python runtime's I/O thread plus main thread. A wait that is not part of the call chain and needs the event loop to advance returns `SyncWaitCycle` (requirements §5, rule 6).

### 3.4 Hot reload

When the entry file changes (by mtime and size), the runtime deletes `realpathSync(entry)` from `require.cache` and imports the entry again. Only the entry module is reloaded; the modules it imports stay cached. To replace those too, restart the runtime the row runs in.

### 3.5 Plugin SDK

Plugins are declared with `definePlugin` from `@arcships/rutis` (`inject`, `apply`, the configuration schema). That package only defines the shape of a plugin declaration and depends on no runtime. The Bun runtime loads plugins of that shape, so plugin authors do not need a separate SDK. That the testing helpers (`@arcships/rutis/testing`) work under `bun test` is one of the acceptance criteria.

### 3.6 Scope

The first version runs **leaf plugins** only: plugins declared with `definePlugin`. Mounting Cordis plugins, and connecting a Cordis application as a node, are outside this design. They can be designed separately if a need appears.

## 4. Rust and configuration

### 4.1 rutis-bridge and rutis-loader

```rust
Launcher::bun(package)               // bun --no-install <package>/src/main.ts
LocalRuntime::bun(package, project)  // named "bun"
RuntimeResolver::modules(handle)     // existing: rows "bun:<module>", resolved by the runtime
```

- **A new cargo feature `bun`** in rutis-bridge and rutis-loader, alongside `node` and `python`.
- **Row names** resolve through the existing `Naming::Modules` (`crates/rutis-loader/src/runtime.rs:78`). The prefix is the runtime name `bun:`; the runtime resolves the module itself and is asked every time (nothing is cached).
- **Inheriting fd:3** is decided by the runtime package's `rutisChannels`, as for the other runtimes.
- **`program`**: by default `bun` (`bun.exe` on Windows) found on `PATH`; a path can be given instead.
- **`--no-install` is always passed** and cannot be configured away.

### 4.2 rutis.json

```json
{
  "runtimes": {
    "bun": { "project": ".", "program": "/opt/bun/bin/bun" }
  },
  "rows": [
    { "id": "weather", "name": "bun:@foo/weather" },
    { "id": "report", "name": "bun:./report.ts", "inject": ["weather"] }
  ]
}
```

| Field | Default | Meaning |
| --- | --- | --- |
| `project` | `.` | Plugin packages resolve from this `package.json` |
| `runtime` | The project's `@arcships/rutis-bun` | Where the runtime package is |
| `program` | `bun` looked up on `PATH` | The Bun executable |

- **A missing runtime package** fails at startup with the hint `bun add -d @arcships/rutis-bun`.
- **Remote runtimes**: `remote` gains the `language` `"bun"`, with rows named `<remote runtime name>:<module>`.
- **Other runtimes**: `runtimes.bun` is independent of `runtimes.node` and `runtimes.py`, and they can all be present. Services are shared across runtimes by name, the same way as across languages (`host_key`).

### 4.3 rutis-host

- `rutis-host new <name> --lang bun` generates a Bun plugin project: `package.json`, `src/index.ts`, a `bun test` test and `rutis.dev.json`.
- `rutis-host dev` recognises a Bun project (`runtimes.bun` in `rutis.dev.json`) and reloads when files change.
- `rutis-host check` lists each `bun:` row's version, dependencies, services and configuration schema.
- The npm distribution of `@arcships/rutis-host` does not bundle the Bun runtime. A Bun project installs `@arcships/rutis-bun` itself.

## 5. WebSocket

**Listening** (a remote runtime, `serve listen:ws://…` / `wss://…`) is built on Bun's server APIs and is in the first version.

**Dialing** (the runtime connecting to a rutis node with a `ws://` / `wss://` channel) is not. Bun's built-in client gives no status code when a handshake is refused, so the refusal cannot be classified as `auth-rejected`, `incompatible` or `retryable`. It is handled in two steps:

1. **First version: fail explicitly.** Dialing fails at once with `incompatible: dialing a WebSocket is not supported by the Bun runtime yet` instead of hanging. This follows principle 7: what cannot cross a boundary becomes a written rule.
2. **Second phase: implement the handshake by hand.** Connect with `Bun.connect` / `tls.connect`, send the Upgrade request, read the status code, and after a 101 hand the connection to the framing layer. The acceptance criterion is that `websocket_cross`'s refusal categories pass in the Bun column.

## 6. Tests

| Layer | Test | What it covers |
| --- | --- | --- |
| Session contract | `rutis-bridge/tests/runtime_conformance.rs` | A Bun endpoint, running every check of `session::testing` |
| Runtime contract | `rutis-bridge/tests/session_matrix.rs` | {Bun} × {fd:3, dial-back, WebSocket listen} |
| Channel contract | New | Bun's channel implementations run the channel contract (ordering, backpressure, large messages, close semantics, the length limit) |
| Loader rows | New `rutis-loader/tests/bun_rows.rs` | Loading, hot reload, the old version serving on after a broken edit, the process exiting and restarting, a failed start not blocking resolution (the checks of `python_rows.rs` and `runtime_rows.rs`) |
| Across languages | `rutis-loader/tests/multilang.rs` | Bun, Python and Node plugins using each other's services; when a provider goes, only its users stop |
| Instances | `instance_runtimes.rs` | Service names of Bun rows inside instances |
| Host | `rutis-host` | Hot reload of a `bun:` row; a project generated by `new --lang bun` passes `check` |
| Launcher | New | `--no-install` is always present; without node_modules, importing a package that is not installed fails rather than downloading it; a Bun that is too old is reported; the hint for a missing runtime package |
| WebSocket | `websocket_cross.rs` | First version: dialing from Bun fails as `incompatible` instead of hanging. Second phase: the same refusal categories as the other implementations |
| The runtime itself | `bun/rutis-bun/test` | `bun test`: channels, session, synchronous waits, hot reload, schema |
| E2E | S2 [#186](https://github.com/arcships/rutis/issues/186), S3 [#187](https://github.com/arcships/rutis/issues/187) | The development loop of `new --lang bun`; Bun rows in cross-language composition and crash recovery |

**CI**: a new `runtimes-bun` job on Linux and macOS installs a pinned Bun with `oven-sh/setup-bun` and runs:

- `bun test`;
- `cargo test -p rutis-bridge --features bun,…`;
- `cargo test -p rutis-loader --features bun,…`;
- `cargo test -p rutis-host`.

A nightly job tracks the latest Bun. `release-dry-run` packs `@arcships/rutis-bun`, and the release train (`scripts/train.mjs`) includes the package.

## 7. Open questions

1. The supported Bun range (1.3 as the minimum for now).
2. Windows: the loopback handover, adoption into the job object, and `kill_on_drop` under Bun.
3. Performance: numbers for the synchronous call round trip and for throughput.
4. Whether Cordis plugins need to be mountable in the Bun runtime (§3.6).

## 8. Phases

| Phase | Contents |
| --- | --- |
| B1 | The runtime package: local channels (fd, dial-back, loopback), session, rows, synchronous waits, hot reload. On the Rust side: the `bun` feature, the launcher and `LocalRuntime::bun`; `runtimes.bun` in `rutis.json`. The tests of §6 except the second WebSocket phase; the CI job `runtimes-bun` |
| B2 | WebSocket listening (remote runtimes) and dialing (§5, second phase); `language: "bun"` for `remote` |
| B3 | `rutis-host new --lang bun` and dev; npm publishing; Windows; performance numbers |
