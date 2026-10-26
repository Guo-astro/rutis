# Diagnostics for 0.18: errors, current state, time limits

[中文](design-diagnostics-2026-10-26.md)

Status: final, not implemented. Date: 2026-10-26. Baseline: `main` `ce131824` (after 0.17.1), assuming #704 from 0.17.2 (wait reasons, `Loader::waits`) is merged. Scheduled for: 0.18.
Issue: [#689](https://github.com/arcships/rutis/issues/689). Implemented per this document: [#677](https://github.com/arcships/rutis/issues/677) (errors, including the merged #680), [#678](https://github.com/arcships/rutis/issues/678) (state, time limits, output, including the merged #681), [#688](https://github.com/arcships/rutis/issues/688) (which row an unhandled error belongs to), the second half of [#676](https://github.com/arcships/rutis/issues/676) (`check` errors).
Basis: the 0.17.0 debugging evaluation in [#663](https://github.com/arcships/rutis/issues/663) (4 languages × 14 failures: 5 clear, 25 need guessing, 22 misleading or silent) and the host owner's end-to-end evaluation.

Out of scope: how debuggers attach (#679, which only references the output in this document); how values convert between languages (#675, #687); tracing systems (OpenTelemetry and others); an interface for the host to rewrite platform names (error messages do not contain platform names, so it is not needed); generated type declarations (#684); test tools reading the state snapshot (test tools have no loader, and each language already has `provided()` / `reported`; #689 listed it, this document does not do it).

## 1. What developers need

When something fails, the output tells them directly:

| Question | Example |
| --- | --- |
| Who | Which row |
| Doing what | Which method of which service, which argument; or `apply`, cleanup |
| Why | One plain sentence; rutis's own errors also carry an error code for programs |
| Where | File and line in their own code; on both sides when crossing languages |
| How to fix | Said when it can be said |

When something is stuck, they can see what is happening now: which rows are running, what they are waiting for, who is calling whom, and for how long.

Why this is not possible today:

1. **The structure exists but is lost on the way.** Cross-language errors already have a structure (the error graph: `name`, `message`, `stack`, `code`, `data`, `cause`, `errors`). The loader's `failed()` (`crates/rutis-loader/src/runtime.rs` and `peer.rs`) turns it into a string before handing it to the kernel; the `CallError` the host receives keeps the original error, but there is no way to read it. Go, Swift and Kotlin send a stack that is only "name: message"; Node, when it receives one, overwrites its own stack with the other side's.
2. **Nobody records "who, doing what".** Errors carry no row or method; rutis's own errors have no error code.
3. **State is scattered.** The kernel's `diagnostics()`, the loader's `entries()` together with the 0.17.2 wait reasons, and runtime state each cover one part; there are no "in-flight calls", and `rutis-host` has no `status` command.
4. **No time limits.** Calls have no limit and cannot be seen; `shutdown()` in Rust embedding can never return.

## 2. Decisions

| # | Decision | Section |
| --- | --- | --- |
| G1 | Errors keep the existing error graph; the loader no longer turns it into a string; rutis adds "who, doing what" (`site`) to errors at the boundary | 3 |
| G2 | `BindingError`s that rutis itself produces have 7 error codes; a plugin's own `code` is kept as is | 3 |
| G3 | Go, Swift and Kotlin send real locations; Node and Python keep the locations of both sides when receiving; rutis prints both sides when printing | 3 |
| G4 | Error messages speak only in terms the developer wrote, rewritten item by item per the list in #677 (including #680) | 3 |
| G5 | Current state is one snapshot, `Loader::status()`: rows, services, runtimes, in-flight calls | 4 |
| G6 | Calls have no time limit by default (same as Cordis); once `callTimeout` is set, rutis enforces it | 5 |
| G7 | `shutdown()` in Rust embedding times out after 10 seconds by default, naming the stuck plugin | 5 |
| G8 | When a Node, Bun or Python runtime is held by synchronous code for 5 seconds, it prints which method of which row | 5 |
| G9 | Plugin output carries a name prefix: on by default in `dev`, off by default in `run` | 6 |
| G10 | Rust embedding writes load failures and runtime exits to standard error by default | 6 |
| G11 | Error type names do not change, and loader types such as `Failure` do not change; what changes is the error text and the `plugin failed: ` prefix | 10 |

## 3. Errors

### 3.1 Structure: keep the error graph, add `site`

The error graph does not change ([plugin mount design](design-protocol-plugin-mount.en.md), `{root, nodes}`, node `{type: "error", name, message, stack, cause?, errors?, code?, data?}`). Error nodes get one optional field:

```text
site: {
  row:      "weather",                          // row name; a row in an instance is written "weather in phone-1"
  doing:    "call" | "apply" | "cleanup" | "listener",
  service?: "forecast",                         // doing = call
  method?:  "tomorrow",
  event?:   "weather/asked"                     // doing = listener
}
```

- **rutis fills it at the boundary**, not each SDK:
  - Calls: when the called row returns an error, rutis in `RowService::invoke` (`crates/rutis-loader/src/runtime/rows.rs`) knows which service and method of which row it is, and adds `site` to the error's root node;
  - `apply` and cleanup: the loader knows which row it is;
  - Listeners: where events are forwarded, it is already known which row the listener is in (today's `listener of <name> in row <row> failed`).
- A node that already has `site` is not overwritten (across several hops, the earliest failing hop is kept).
- What cannot be filled is written into the boundaries: functions passed by reference (through relay, no service or method, only the row); errors not inside any call (timers, promises that are not awaited, see §3.5); calls within the same runtime (these are already the language's own errors and stacks).
- Every language tolerates extra fields when decoding (verified: Node, Python, Go, Swift and Kotlin all read by key), so the protocol version is not raised, and an older side keeps working.

### 3.2 Structure kept all the way

- The loader's `failed()` (both places) wraps the original error in `ErrorKind::PluginFailed` instead of turning it into a string. The kernel's row state already stores `Arc<ErrorKind>`, so the structure is kept.
- `ErrorKind::PluginFailed` drops the `plugin failed: ` prefix when displayed.
- Hosts read the structure: rutis-bridge provides `rutis_bridge::thrown(&rutis::Error) -> Option<&session::Error>` (finds the wrapped cross-language error); `session::Error` already has `code()` and `data()`, and gets `stack()` and `site()`. No new kernel types.
- The loader's `Failure { id, error: String }` and `ReconcileReport` do not change (they are compared for equality); code that needs the structure reads it from the row's state (the snapshot error in `EntryInfo`).

### 3.3 Error codes

An error a plugin throws itself keeps whatever `code` it has. `BindingError`s that rutis itself produces use the 7 codes below, with details in the existing `data`:

| code | When | `data` |
| --- | --- | --- |
| `not-declared` | A service not listed in `inject` was used | `{service}` |
| `unavailable` | The service has no provider right now | `{service, reason}` (the reason from `Loader::waits`) |
| `no-method` | The method does not exist | `{service, method, methods}` |
| `bad-argument` | Wrong number or type of arguments | `{service, method, argument, expected}` |
| `bad-value` | A value cannot be sent, or a message exceeds the limit | `{argument?, part, limit?}` |
| `peer-gone` | The runtime on the other side exited or disconnected during the call | `{runtime, reason}` |
| `timeout` | `callTimeout` was exceeded | `{service, method, seconds}` |

`SyncWaitCycle` and `AggregateError` already have their own type names and get no code. Codes are for programs and do not appear in the line of text meant for people. Rows can be added to the table during implementation; existing rows do not change.

### 3.4 Locations across languages

| Language | `stack` sent | When receiving an error from the other side |
| --- | --- | --- |
| TypeScript (Node, Bun) | Real stack (exists) | `error.stack` = the other side's stack + `--- called from <row>` + the stack at its own call site (today its own is overwritten) |
| Python | `traceback` (exists) | Local traceback as usual; the other side's stack attached with `add_note`; 3.10 has no `add_note`, so it goes in a `remote_stack` attribute |
| Go | Ordinary errors: file:line of the declaration of the method that returned it (taken from `Type.Method(i).Func.Pointer()`, not the reflection stub); panic: full stack (exists) | `RemoteError` gets `Stack` |
| Swift | file:line of the declaration of the method that threw it (recorded when the `RutisService` macro expands); Swift errors have no stack, so it only reaches the method — this is a boundary | `RutisError` gets `stack` |
| Kotlin | Real JVM stack (today only "name: message") | The other side's stack attached as a suppressed exception |
| Rust | The method that returned the error (recorded by code generated by `#[rutis::service]`); panics carry `std::backtrace` when `RUST_BACKTRACE` is set | `stack()` |

When rutis prints an error (`dev`, `run`, Rust embedding's default output), it always prints the locations of both sides.

Also fix one item named in #677: Go's `Parallel` returns `*rutis.AggregateError` in a real host (`errors.As` holds, `Data` is not empty); today this is only correct in the test tools.

### 3.5 Text

Rule: speak only in terms the developer wrote.

- Never appears: rutis, Cordis, Rust (unless the plugin is written in Rust), binding, session, the raw error text of WireValue and serde, internal ids (`row:23.2`, `PluginId(3)`, labels containing NUL), internal plugins (`runtime-rows`, `rutis-bridge/link#py`), `<root>` / `<leaf>`, the host's private Rust type paths, SDK internal types (`HostProxy`, `RemoteFuture`, `e.graph`, `UNDEFINED`), `rollback failed`, `cannot resolve row … again`, `ThrownValue`.
- Rows are written by row name, runtimes by their name (`py`), and the host as "host".
- Rewrite every item listed in the body of #677 and in its "merged #680" section, each locked by a test on the new text; several of them must state the reason: say plainly when the default export is not the result of `definePlugin`; `entry file is gone` names the file; when a string or object is thrown, show it as is (`"just a string"`, `{code: 3}`), not `ThrownValue`; an unhandled rejection prints only the plugin's own frames.
- Consistency cases: each language runs the failure list once, and none of the words above appear in the output.
- Compile errors from Rust macros and traits speak plainly through `#[diagnostic::on_unimplemented]`.

### 3.6 Errors are recorded on the row that failed

- The called method fails: the caller sees "call to X failed", and `site` is the called row.
- Config does not match the schema: recorded on the row whose config is wrong, not on the caller (#669 makes schemas take effect; its errors belong to rows by the rule here).
- Unhandled rejections / exceptions (#688):
  - Node: in `dev`, `async_hooks` records which row a promise belongs to, and that row fails; in `run` this is off (it adds cost to every promise), and the message is "an unhandled error in runtime node, source unknown".
  - Python: traced to the row through the task's context (`Task.get_context()` from 3.12; `_context` on 3.10 and 3.11).
  - Bun: cannot be traced; always "source unknown".
  - When it cannot be traced, the process does not exit. When a plugin itself calls `process.exit` / `os._exit`, the process exits as usual, and the report names the row that called it.
  - This changes "boundary rule 8" (an uncaught error ends the runtime process) and the handling in Bun's `main.ts`; `docs/guarantees.md` and the "plugins in the same runtime share one process" section of the plugin API guide are rewritten to match.

## 4. Current state

### 4.1 One snapshot

`Loader::status() -> Status` (the parts are read one after another under a series of short locks, not guaranteed atomic, same as `diagnostics()`):

```text
Status {
  rows:     [{ id, instance?, state, reason? }]   // pending | starting | running | failed | stopped | disabled
                                                  // reason: the failure error, or the wait reason (Loader::waits)
  services: [{ name, provider }]                  // provider: row name | "host"
  runtimes: [{ name, state, reason?, pid? }]      // running | starting | down; only local runtimes have pid
  calls:    [{ from, to, service, method, elapsed }]
}
```

- Names only, no internal ids, no internal plugins. `Status` implements `Display` (for people, one item per line).
- Rows and wait reasons come from the loader (exists); services from `diagnostics()` (exists); runtimes from `RuntimeHandle` (exists).
- When a runtime exits, its rows immediately show as waiting for the runtime (read from `RuntimeHandle`'s state, not waiting for the kernel's `Pending` that changes a few seconds later; #681).

### 4.2 In-flight calls

- Registered in `RowService::invoke`: the called row and service are its own, the method is an argument, and the caller comes from `caller_row()` (a call not made from inside a row is recorded as "host"). For async calls, the registration wraps the returned future and is removed when it ends. `RowService` does not store the service name today; it needs to be added.
- What cannot be seen is written into the boundaries: calls within the same runtime; functions passed by reference (only "host → row" is visible, no method name); calls from in-process Rust plugins are recorded as "host".

### 4.3 Who reads it

| Where | How it reads |
| --- | --- |
| `rutis-host run` / `dev` | State lines print changes as before; waiting lines use `Loader::waits`; the line about what is stuck at shutdown uses the snapshot instead (counts rows only, names what they wait for) |
| `rutis-host status` | New command, prints one snapshot. `run` / `dev` open a read-only local socket in the directory of `rutis.json` (`.rutis-host.sock`, permissions 0600) that answers only the `status` request; `status` looks for it in the same directory (or the one `--config` points to). Not provided on Windows in 0.18; written into the boundaries |
| Rust embedding | `Loader::status()`; `diagnostics()` gets a readable `Display` (uses names, does not list internal plugins by default); `RuntimeState` implements `Debug` |

`rutis/*` events do not change (they report changes only). The rutis-dev channel is not reused (it does not publish, and it carries commands that change things).

## 5. Time limits

| Limit | Default | Enable / adjust | When reached |
| --- | --- | --- | --- |
| Call | No limit (same as Cordis) | Top-level `"callTimeout": 30` in `rutis.json` (seconds, calls made by all rows); `callTimeout` on a row covers calls made by that row and overrides the top level; Rust embedding: `LoaderOptions::call_timeout` | The caller gets `timeout` ("weather.tomorrow did not return in 30 seconds"), the callee receives a cancel |
| Shutdown | 10 seconds | `RUTIS_SHUTDOWN_TIMEOUT` / `--shutdown-timeout` (exists); also applies to Rust embedding | `shutdown()` returns an error naming the stuck plugin; cleanup registered with `ctx.effect` shows the file:line where it was registered (`#[track_caller]`), other cleanup shows the plugin or row name |
| Handshake | 30 seconds local, 10 seconds network | Unchanged | Unchanged, only the text changes |
| Synchronous blocking hint | 5 seconds, fixed | — | Node, Bun and Python runtimes print once: "runtime py has been held by weather.tomorrow for 5 seconds, and all other rows are waiting; it is a synchronous method (`def`), change it to `async def` and it will not hold the runtime" |

- The call time limit covers only calls that go through rutis (across runtimes, between the host and a runtime).
- An async call times out: the wait is dropped, and the existing mechanism sends a cancel. A sync call times out: the wait becomes time-limited, and when the time is reached it returns `timeout` and sends a cancel; the synchronous code running in the callee cannot be stopped, so that runtime stays held, and later calls sent to it also time out one by one. This is written into the boundaries and has tests.
- Callers that set their own limits in the language (Python `asyncio.wait_for`, Go `context`, Swift `Task`, Kotlin `withTimeout`) work as before. TypeScript callers cannot cancel today; 0.18 adds no API; use `callTimeout`.
- Synchronous methods in Go, Swift and Kotlin do not hold the whole runtime, so there is no blocking hint for them.

## 6. Output

- **Plugin output carries a prefix**:
  - Node, Bun, Python (several rows share one process): the runtime adds `[weather] ` for the row currently running. Node and Bun wrap these entry points in an async context: `apply`, service methods, listeners, functions exported by reference; Python uses contextvars. What cannot be traced (Python threads, Node child processes and native modules that write to fds directly) gets the runtime name.
  - Go, Swift, Kotlin (one plugin per process): the host adds the runtime name when reading the output pipe; the SDKs do not change.
  - On by default in `dev`, off by default in `run` (the output format in production does not change); switched with `--prefix-output` / `--no-prefix-output`.
- **Python `logging`**: lower the level of `logging.lastResort` to INFO (it prints when the plugin has configured no handler); no handler is added to the root logger, and a plugin's own `basicConfig` works as usual. INFO logs from dependencies will appear as a result; this goes into the guide.
- **Rust embedding is visible by default**: besides listener errors, the default error sink also writes plugin or row load failures and runtime exits (the hint for waits longer than 10 seconds already exists in 0.17.2); each names the plugin or row. `CtxHost`'s existing error sink interface can replace it or turn it off.
- **Providing by name does not depend on whether rutis-bridge is linked** (#681): when a type marked `#[rutis::service]` is provided but the name registration is not linked in, startup writes once: "service calendar is provided by type but not registered by name: depend on rutis-bridge to use it by name and to check for conflicts".
- **State lines go to standard error**; plugin output goes to whichever stream it was written to.

## 7. What developers will see

### 7.1 The 14 failures (#663 debugging evaluation)

TypeScript as the example; other languages show the same content, with locations written their own way.

| # | Failure | Output |
| --- | --- | --- |
| 1 | Misspelled dependency service name | `weather: waiting for lmm (no row provides it; did you mean llm?)` (0.17.2) |
| 2 | A service method throws, across languages | `chat: call to weather.tomorrow failed: KeyError: 'region'`, followed by two locations: `weather at weather.py:31 in tomorrow`, `called from chat at src/chat.ts:14:36` |
| 3 | Method does not exist / wrong argument | `chat: weather has no method tomorow (methods: today, tomorrow)`; `chat: weather.tomorrow: argument 1 should be a number, got "x"` |
| 4 | Error in `apply` | `weather: failed in apply: KeyError: 'region'` + `weather.py:12 in apply` |
| 5 | Import error | `weather: cannot load src/util.ts: SyntaxError … (src/util.ts:3:7)` (names the failing submodule) |
| 6 | A value cannot be sent | `chat: weather.tomorrow: argument 0 cannot be sent: a sparse array (index 1 is empty)`; the runtime does not exit (#675) |
| 7 | A listener fails | `weather: listener of weather/asked failed: …` + location; a `parallel` caller sees `2 listeners of weather/asked failed`, with each row and location listed below |
| 8 | A call does not return | `rutis-host status`: `chat → weather.tomorrow  42s`; with `callTimeout` set: `chat: weather.tomorrow did not return in 30s` |
| 9 | Mutual calls / mutual dependencies | `chat: weather.today would wait on itself: chat → weather → chat (synchronous calls)`; mutual `inject`: `check` reports an error (§8) |
| 10 | A runtime exits midway | `runtime py: down (exited with status 1)`; `weather: waiting for runtime py (down: exited with status 1)`; in-flight calls: `chat: weather.tomorrow failed: runtime py exited during the call` |
| 11 | Breakpoints, reading logs | Output has the `[weather]` prefix; how debuggers attach is in #679 |
| 12 | Wrong config | `weather: config: units should be "metric" or "imperial", got "kelvin"` (recorded on weather) |
| 13 | Two plugins provide a service with the same name | `check`: `rows a and b both provide forecast in the same scope` and exits with 1; at run time: `b: forecast is already provided by a` |
| 14 | Viewing the current state | `rutis-host status`: each row's state and reason, each service's provider, in-flight calls |

### 7.2 From the host owner's evaluation (#663)

| Failure | Output |
| --- | --- |
| The host receives a plugin's error | `rutis_bridge::thrown(&err)` gives name, code, message, stack, site; the host rewrites by code |
| A plugin throws a string / object | `wordcount: call to tools.run failed: "need 5 words"` |
| An unhandled rejection in one plugin | `dev`: only that row fails; `run`: `runtime node: an unhandled error from an unknown row: …`, other rows continue |
| The default export is not the result of `definePlugin` | `wordcount: the default export of src/index.ts is not a plugin made with definePlugin` |
| A Python `def` method blocks | The blocking hint (§5) |

## 8. `check`

The second half of #676: cycles of mutual `inject`, and several rows providing the same service in the same scope, are warnings in 0.17.2 and become errors in 0.18 (exit with 1); the message names the rows and how to fix it.

## 9. Four things to know across languages

| Thing | Where it is pointed out |
| --- | --- |
| Values are copied | Plugin API guide; the `bad-value` message says "copied by value, an object's methods do not go across" (except the by-reference part after #687) |
| The other side may be absent | The `unavailable` and `peer-gone` messages say why it is absent |
| Debugging is done per process | #679's debugging guide; the pid of local runtimes in the state snapshot |
| Events across processes have no guaranteed order | Events guide |

## 10. Compatibility and migration

Written into "Migrating to 0.18" (both languages):

- **Rust hosts**: `ErrorKind::PluginFailed` is displayed without the `plugin failed: ` prefix; plugin errors are no longer strings, read the structure with `rutis_bridge::thrown()`. Code that matches on text must switch to error codes. Loader types (`Failure`, `ReconcileReport`) do not change.
- **Rust embedding (behavior change)**: `shutdown()` times out after 10 seconds by default; load failures and runtime exits are written to standard error by default.
- **All languages**: error type names do not change; only fields are added (Go `RemoteError.Stack`, Swift `RutisError.stack`, Python `remote_stack`). Error text will change.
- **Node, Bun, Python runtimes**: unhandled errors no longer make the process exit ("boundary rule 8" rewritten).
- **`check`**: cycles and same-name providers exit with 1.
- **Protocol**: error nodes have one more optional field, `site`, which an older side ignores; the protocol version is not raised, and different versions work together as usual.

## 11. Implementation

| Issue | Content |
| --- | --- |
| #677 | §3 (3.1–3.5): `site`, error codes, locations in each language, loader no longer turning errors into strings, `thrown()`, Go `Parallel`, text rewrites and consistency cases, macro hints |
| #678 | §4, §5, §6: `Loader::status()`, in-flight calls, `rutis-host status`, `callTimeout`, shutdown time limit, blocking hint, output prefix, Python logging, Rust embedding default output, startup hint for providing by name |
| #688 | §3.6 which row unhandled errors belong to, boundary rule 8 |
| Second half of #676 | §8 |

Each issue is done when it matches §7: in each language, run the 14 failures and the failures from the host evaluation once, and the output matches the tables (locations written per language).
