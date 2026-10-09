# Bun as a JS plugin runtime

[中文](design-bun-runtime-2026-10-09.md)

Design proposal · Related to [#194](https://github.com/arcships/rutis/issues/194)

## 1. Background

TS / JS plugins and Cordis plugins run in the Node runtime: `src/runner.mjs` of `@arcships/rutis-runtime`, started by the host with `node --import tsx` (`Launcher::node`, `crates/rutis-bridge/src/runtime/process.rs`).

Bun is another common runtime in the JS ecosystem. It runs TS natively (no tsx) and starts faster. Some projects have only Bun installed, or manage dependencies with `bun install`. Today such a project needs a separate Node install to use rutis.

Against the criteria of the [design philosophy](design-philosophy.en.md) §7:

1. It widens what a host can connect to: projects and machines that only have Bun.
2. It lives entirely outside the core: the launcher and the runtime package change, the core does not.
3. It holds across the process boundary: it is the same protocol, with the same boundary rules.
4. It has a concrete user: TS projects that use only Bun.
5. Its guarantees can be pinned by the existing conformance tests.

**Bun is not a new language.** It runs the same runtime package (`runner.mjs`), the same protocol and the same SDK (`@arcships/rutis`) as Node. This design adds no new kind of runtime. It gives JS runtimes an **engine** option, and lets a host have several named local JS runtimes, so Node and Bun can run side by side (§4).

## 2. Findings

Verified on 2026-10-09 with Bun 1.3.14 on macOS. A `node` shim replaced the runtime with `bun --no-install`, and the fixes in the table below were applied to a temporary copy of the runtime package. The repository itself was not changed.

**Results:**

- `cargo test -p rutis-bridge --all-features`: one test fails, `websocket_cross::node_dialing_rust_is_refused_by_category`, for the reason in the last row of the table. Everything else passes, including runtime_conformance, session_matrix, node_conformance, process_exit, live_objects, cancellation, group_mount, and websocket_conformance / link / multihop.
- `rutis-loader`: runtime_rows, multilang, cordis_node and instance_* pass.
- `rutis-host`: all 8 tests pass.

These work unchanged under Bun:

- **The synchronous call mechanism**: `worker_threads`, SharedArrayBuffer, `Atomics.wait` on the main thread and `receiveMessageOnPort`. A crashing worker still surfaces during a blocking wait.
- **Cordis 4.0.4.**

**Startup time** (median of 10): `node --import tsx` 85ms, `node` 36ms, `bun` 29ms. Throughput and memory were not measured.

What does not work as is:

| Dependency | Where | Under Bun | Fix |
| --- | --- | --- | --- |
| `new net.Socket({ fd: 3 })` | `channel/fd.mjs:11` | **Fails silently**: no error, no data, and the process exits on its own | Use `net.connect({ fd })` under Bun (verified). Node rejects that form, so branch on `process.versions.bun` |
| Hot reload via `import(href + '?rutis-reload=…')` | `runner.mjs:205-214` | **The query string is ignored**: the old module keeps coming back (.mjs and .ts alike) | Delete the entry's realpath from `require.cache`, then import (verified). The realpath matters: on macOS `/var` is `/private/var` |
| `createRequire(anchor).resolve('@deepseek-ai/cordis')` | `runner.mjs:33,38,174`, `bridge/features.mjs:123` | **Auto-install**: with no node_modules, Bun resolves a second Cordis from its global cache, and 10 runtime_rows tests fail | **Always start with `--no-install`** (verified). This is also a security requirement: without it, plugins could download packages at run time |
| `--import tsx` | `process.rs:167-170` | Not needed: enums, parameter properties, decorators, and `./a.js` resolving to `a.ts` all work | The Bun launcher leaves tsx out |
| The `ws` client's `upgrade` / `unexpected-response` events, `maxPayload`, `ca` | `channel/websocket.mjs:101-128` | Bun replaces `ws` with a built-in implementation that emits none of these. On a 401 there is neither an event nor an error, and the connection hangs | See §5 |
| Server-side `WebSocketServer`, listen:ws, and `serve.mjs`'s `spawn(process.execPath, execArgv)` | Several places | Fine; `execArgv` keeps `--no-install` | Nothing to do |
| Windows: loopback TCP with a one-time token | `channel/tcp.mjs` | Not verified | See §7 |

## 3. The engine

### 3.1 Rust

```rust
pub enum Engine { Node, Bun }

Launcher::node(package)            // unchanged: node --import tsx <package>/src/runner.mjs
Launcher::bun(package)             // new: bun --no-install <package>/src/runner.mjs
Launcher::js(package, engine)      // new: picks one of the two by engine

LocalRuntime::node(package, anchor)              // unchanged: named "node", engine Node
LocalRuntime::js(name, engine, package, anchor)  // new: any name, any engine
```

- **`program`**: by default `node` / `bun` found on `PATH` (with `.exe` on Windows). A path can be given instead, to pin a version or use a project-local Bun.
- **fd:3**: with Bun, the inherited socket is used only when the runtime package's `rutisChannels` lists `"fd"` **and** the package declares Bun support (§3.2). Otherwise the launcher falls back to dial-back (Unix) or loopback (Windows), so an old package never fails silently under Bun.
- **`RUTIS_JS_ENGINE`**: set to `bun`, it starts every JS runtime whose engine is not named explicitly with Bun. This is a switch for the test matrix and for trying Bun out, following `RUTIS_LOCAL_HANDOVER=loopback` (`transport/local/spawn.rs:101`). An engine named in configuration takes precedence.
- **No new cargo feature.** Everything stays under the `node` feature.

### 3.2 What the runtime package declares

`@arcships/rutis-runtime`'s package.json gains:

```json
"rutisEngines": ["node", "bun"]
```

The host reads this before starting Bun. A package that does not list `bun` (0.8 and earlier) is rejected without being started: "@arcships/rutis-runtime <version> does not support Bun; install 0.9 or later". At startup the runtime checks Bun's version; below the supported minimum (1.3 for now) it exits with a clear error before greeting, and the host reports `exited before connecting` with that error.

### 3.3 The JS runtime package

1. **`channel/fd.mjs`**: use `net.connect({ fd })` when `process.versions.bun` is set.
2. **`runner.mjs`'s `fresh()`**: under Bun, when the entry file has changed, delete `realpathSync(entry)` from `require.cache` and import again. A reload replaces only the entry module and the modules it imports stay cached, which is what the query string does under Node, so the semantics do not change.
3. **Diagnostics**: the runtime includes its engine and version (for example `bun 1.3.14`) when it describes itself; `rutis-host check` and `diagnostics()` list each runtime's name, engine and version. This is "understanding connections" in design philosophy §1.
4. **The WebSocket client**: see §5.

## 4. Several JS runtimes side by side

A host can have **any number of named local JS runtimes**, each with its own engine and project. Node next to Bun is one case; several instances of the same engine for isolation is another (design philosophy §6: "isolation means more runtime instances").

### 4.1 Which runtime a row runs in

The prefix rule that Python rows and remote runtimes already use (`<runtime name>:<module>`):

| The row's `name` | Runs in |
| --- | --- |
| `@foo/weather`, `./plugin.ts` (no prefix) | **The default JS runtime** |
| `bun:@foo/weather`, `bun:./plugin.ts` | The runtime named `bun` |
| `sandbox:@foo/weather` | The runtime named `sandbox` |

- **The default JS runtime**: the one marked `"default": true`; otherwise the one named `node`; otherwise the only local JS runtime, if there is just one. If none of these applies, unprefixed rows are invalid (`Unresolved`), and the error lists the prefixes available.
- npm package names contain no `:`, and a Windows drive letter is one character while runtime names are at least two, so nothing is ambiguous.
- Remote JS runtimes (`remote` with `language: "node"`) resolve as before.
- `RuntimeResolver::node(handle)` resolves unprefixed names and the `<name>:` prefix; the new `RuntimeResolver::node_prefixed(handle)` resolves only the `<name>:` prefix. In the implementation this is an optional prefix on `Naming::Npm` (`crates/rutis-loader/src/runtime.rs:69`).

### 4.2 Configuration

`runtimes.node` stays as it is (named `node`, engine Node); a new list `runtimes.js` is added:

```json
{
  "runtimes": {
    "node": { "project": "." },
    "js": [
      { "name": "bun", "engine": "bun", "project": "." },
      { "name": "sandbox", "engine": "node", "project": "./sandbox" }
    ]
  },
  "rows": [
    { "id": "weather", "name": "@foo/weather" },
    { "id": "report", "name": "bun:./report.ts", "inject": ["weather"] }
  ]
}
```

| Field | Default | Meaning |
| --- | --- | --- |
| `name` | Required | The runtime's name and row prefix; at least two characters, and not the name of `py` or of a remote runtime |
| `engine` | `"node"` | `"node"` or `"bun"` |
| `project` | `.` | Plugin packages resolve from this `package.json`; runtimes may share a project |
| `runtime` | The project's `@arcships/rutis-runtime` | As in `runtimes.node` |
| `program` | Looked up on `PATH` | The engine's executable |
| `default` | `false` | Unprefixed rows run here |

A Bun-only project writes a single `{ "name": "bun", "engine": "bun" }`; as the only JS runtime, it is the default.

### 4.3 Semantics when runtimes coexist

- **Services are shared across runtimes as usual**: a plugin in `node` injects `weather` provided in `bun` the same way it uses a service from another language (`host_key`), and dependency declarations do not change (principle 8).
- **Native Cordis sharing stays within one runtime**: each runtime has its own Cordis Context. Plugins that rely on Cordis's in-process features to work together (group mounts, direct replacement with `ctx.set` and so on) belong in the same runtime; across runtimes the boundary rules of the requirements §5 apply, as across any process.
- **Failure scope is per runtime**: a runtime that crashes withdraws only its own services; plugins in other runtimes wait by the dependency rules and are otherwise unaffected.
- **Processes**: one process per runtime. Whether a runtime with no rows exits or starts on demand follows the existing runtimes' behaviour and is not changed here.

### 4.4 Plugin projects and rutis-host

- `rutis-host new --lang node` generates the same project; the closing hint gains the Bun variant (`bun install`, and `{ "name": "bun", "engine": "bun" }` in `runtimes.js`). There is no `--lang bun`: the plugin code is the same.
- The template's `tsx` dependency is unused under Bun but kept, so the same project runs on either engine.
- `rutis-host dev`: the plugin under development runs in the default JS runtime; `rutis.dev.json` can choose the engine with the same `runtimes`.
- `bunx @arcships/rutis-host`: the platform binary packages are chosen through `optionalDependencies` with `os` / `cpu`. Bun supports this but it is unverified; it belongs to S9 ([#193](https://github.com/arcships/rutis/issues/193)).
- Whether plugins and the runtime resolve the same Cordis under `bun install`'s isolated linker layout is unverified (Bun resolves by realpath, so they are expected to); the loader row tests of §6 cover it.

## 5. The WebSocket client

The JS side uses the `ws` client only when it dials out:

- a runtime given a `ws://` or `wss://` channel to dial (`io-worker.mjs:47`);
- a Cordis application linking to a rutis node as a node itself (`bridge/worker.mjs:61`).

Local runtimes started by the host use fd, unix or tcp and never get here. A runtime listening as a remote runtime (`serve`) uses the server side, which works under Bun.

**This design handles it in two steps:**

1. **Step 1: fail explicitly.** Under Bun, dialing `ws://` or `wss://` fails at once with `ConnectError('incompatible', 'dialing a WebSocket is not supported on Bun yet')` instead of hanging. The documentation says Bun supports local channels and listening, not dialing out yet. This is principle 7: what cannot cross a boundary becomes a written rule, not a silent degradation.
2. **Step 2: implement dialing.** Classifying rejections needs the status code of the handshake response. Two options:
   - Send the Upgrade request with `node:https` / `node:http`, and after the 101 hand the socket to a `ws` `WebSocket.setSocket`-style interface. Whether Bun supports this needs checking.
   - Send a preflight `fetch` with the same headers to read 401, 403, 426 and so on, then connect with the native `WebSocket`. This costs one more round trip and races (a successful preflight does not guarantee the connection that follows).

   `maxPayload` is checked by our own framing when a message arrives; `ca` goes through Bun's `tls` options.

   Step 2 is a separate PR. Its acceptance criterion is `websocket_cross` passing under Bun.

## 6. Tests

These tests pin the promises above. The engine tests add a Bun column to existing tests rather than a new suite; coexistence needs new tests.

| Layer | Test | Change |
| --- | --- | --- |
| Runtime contract | `rutis-bridge/tests/session_matrix.rs` | Parameterise the Node column by engine; add {Bun} × {fd:3, dial-back, WebSocket listen} |
| Session contract | `runtime_conformance.rs` | Add a Bun endpoint |
| Loader rows | `rutis-loader/tests/runtime_rows.rs`, `multilang.rs`, `instance_runtimes.rs`, `cordis_node.rs` | Run again in CI with `RUTIS_JS_ENGINE=bun` (as with `RUTIS_LOCAL_HANDOVER`) |
| Host | `rutis-host`'s `a_reloaded_row_runs_the_edited_plugin` | Add a Bun version: the regression test for the reload fix |
| Launcher | New | A runtime package that does not declare Bun is refused with a hint; a Bun below the minimum is reported; `--no-install` is always present |
| Security | New | In a directory without node_modules, importing a package that is not installed fails rather than downloading it |
| WebSocket | `websocket_cross.rs` | Step 1: assert that dialing from Bun fails as `incompatible` instead of hanging. Step 2: the same categorisation assertions as Node |
| JS unit tests | `node/rutis-runtime/test` | Also run under `bun test`. Today 2 tests fail in each of channel and websocket, and handshake times out; not every cause is confirmed, so each needs fixing or marking |
| Coexistence | New in `rutis-loader/tests/runtime_rows.rs` | Runtimes `node` and `bun` in one host: unprefixed and `bun:` rows land in their runtimes; a service provided in `bun` is used by a plugin in `node` and the other way round; killing the `bun` process withdraws only its services while `node`'s rows wait by their dependencies; two instances of one engine are isolated from each other |
| Default runtime | New in `rutis-host`'s config / host tests | The default rule (`default`, the one named `node`, the only one); with several candidates and none chosen, unprefixed rows are invalid and the error lists the prefixes; duplicate runtime names and names shorter than two characters are configuration errors |
| E2E | S2 [#186](https://github.com/arcships/rutis/issues/186), S3 [#187](https://github.com/arcships/rutis/issues/187) | One Bun variant each |

**CI**: a new `runtimes-bun` job on Linux and macOS installs a pinned Bun with `oven-sh/setup-bun` and runs:

- `cargo test -p rutis-bridge --all-features`, Bun column;
- `RUTIS_JS_ENGINE=bun cargo test -p rutis-loader --features node,python,peer`;
- `cargo test -p rutis-host`;
- `bun test`.

Windows joins after item 2 of §7 is verified.

## 7. Open questions

1. **The supported Bun range.** 1.3 as the minimum for now. Bun releases often and its Node compatibility changes between versions, so CI pins one version and a nightly job tracks the latest.
2. **Windows.** `kill_on_drop`, adoption into the job object, and the loopback TCP handover are all unverified under Bun.
3. **Performance.** Only startup time is measured so far. The synchronous call round trip (about 30µs on Node) and throughput need measuring under Bun, with the results written up in a document like [performance-event-dispatch](performance-event-dispatch-2026-09-27.en.md).
4. **`bun test` versus `node:test`.** Whether the JS unit tests should be written so that both runners can run them.

## 8. Out of scope

- **Deno.** Its Node compatibility layer and permission model differ more; it gets its own evaluation when there is a need.
- **Bun-only APIs** (`Bun.spawn`, `Bun.serve` and so on). The runtime package uses Node-compatible APIs with a few branches, and both engines share one codebase.

## 9. Phases

| Phase | Contents |
| --- | --- |
| B1 | §4: several named local JS runtimes side by side (independent of the engine; useful with Node alone, for isolation) and its tests |
| B2 | §3: the Bun engine, the runtime package's declaration and fixes; step 1 of §5; the engine tests of §6; the CI job `runtimes-bun` (Linux, macOS) |
| B3 | Step 2 of §5: dialing WebSocket from Bun |
| B4 | Windows; performance numbers; the `bunx` install smoke test (part of S9) |
