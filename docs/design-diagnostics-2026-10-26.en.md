# Diagnostics for 0.18: errors, current state, time limits

[中文](design-diagnostics-2026-10-26.md)

Status: final, not implemented. Date: 2026-10-26 (revised the same day: removed the call time limit and the Python logging setting, shutdown time limits belong to the host, error sites go only to people reading the output and to the host, added "formatting must not fail"). Baseline: `main` `ce131824` (after 0.17.1), assuming #704 from 0.17.2 (wait reasons, `Loader::waits`) is merged. Scheduled for: 0.18.
Issue: [#689](https://github.com/arcships/rutis/issues/689). Implemented per this document: [#677](https://github.com/arcships/rutis/issues/677) (errors, including the merged #680), [#678](https://github.com/arcships/rutis/issues/678) (state, time limits, output, including the merged #681), [#688](https://github.com/arcships/rutis/issues/688) (which row an unhandled error belongs to), the second half of [#676](https://github.com/arcships/rutis/issues/676) (`check` errors).
Basis: the 0.17.0 debugging evaluation in [#663](https://github.com/arcships/rutis/issues/663) (4 languages × 14 failures: 5 clear, 25 need guessing, 22 misleading or silent) and the host owner's end-to-end evaluation.

Out of scope: time limits on calls (they belong to the caller, see §5); how debuggers attach (#679, which only references the output in this document); how values convert between languages (#675, #687); tracing systems (OpenTelemetry and others); an interface for the host to rewrite platform names (error messages do not contain platform names, so it is not needed); generated type declarations (#684); test tools reading the state snapshot (test tools have no loader, and each language already has `provided()` / `reported`; #689 listed it, this document does not do it).

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
4. **When stuck, it does not say who.** There is no way to see which call has been waiting and for how long; a stuck shutdown reports the root, not the stuck plugin.

## 2. Principle: the platform only shows, it does not decide for the business

Diagnostics are about "being able to see", not "deciding for others". Each of the following has an owner, and this document does not go past it:

| Matter | Owner | What this document does |
| --- | --- | --- |
| How long a call waits | The caller's business code | No call time limit; the state only shows in-flight calls and how long they have been waiting |
| How long shutdown waits | The host (the program that embeds rutis) | The kernel sets no default; rutis-host, as one host, chooses 10 seconds (exists) |
| Language-wide settings in the plugin process (log level and so on) | The plugin | Not changed; the guide states each language's default rules |
| Which row was called, and its code location | People reading the output, and the host owner | Appears only in output that rutis prints and in the host's read interface, not handed to the caller's code (principle 8: dependencies contain only service names, never locations) |
| Lifecycle of runtime processes, the terminal and the platform itself | The platform | The platform decides: which row unhandled errors belong to, output prefixes, handshake time limits |

## 3. Decisions

| # | Decision | Section |
| --- | --- | --- |
| G1 | Errors keep the existing error graph; the loader no longer turns it into a string; rutis adds "who, doing what" (`site`) to errors at the boundary, used only for printing and for the host to read | 4 |
| G2 | `BindingError`s that rutis itself produces have 6 error codes; a plugin's own `code` is kept as is | 4 |
| G3 | Go, Swift and Kotlin send real locations; when any language receives an error from the other side, the error object handed to code has only the name, message, error code and data, the other side's error graph is kept internally and carried along unchanged when rethrown; rutis prints both sides when printing | 4 |
| G4 | Error messages speak only in terms the developer wrote, rewritten item by item per the list in #677 (including #680) | 4 |
| G5 | Formatting an error cannot fail: any bad data falls back to "name: message", and the original error is not lost | 4 |
| G6 | Current state is one snapshot, `Loader::status()`: rows, services, runtimes, in-flight calls | 5 |
| G7 | Calls have no time limit; the shutdown time limit is decided by the host, and the timeout report names the stuck plugin | 6 |
| G8 | When a Node, Bun or Python runtime is held by synchronous code for 5 seconds, it prints which method of which row, without interrupting it | 6 |
| G9 | Plugin output carries a name prefix: on by default in `dev`, off by default in `run` | 7 |
| G10 | Rust embedding writes load failures and runtime exits to standard error by default; the output does not contain platform names | 7 |
| G11 | Error type names and fields do not change, and loader types such as `Failure` do not change; what changes is the error text, the `plugin failed: ` prefix and Node's `error.stack` | 11 |

## 4. Errors

### 4.1 Structure: keep the error graph, add `site`

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
- **Who can see it**: output that rutis prints (`dev`, `run`, Rust embedding's default output) and the host's read interface (§4.2). The SDKs do not hand `site` to the caller's code.
- What cannot be filled is written into the boundaries: functions passed by reference (only the row); errors not inside any call (timers, promises that are not awaited, see §4.7); calls within the same runtime (these are already the language's own errors and stacks).
- Every language tolerates extra fields when decoding (verified: Node, Python, Go, Swift and Kotlin all read by key), so the protocol version is not raised, and an older side keeps working.

### 4.2 Structure kept all the way

- The loader's `failed()` (both places) wraps the original error in `ErrorKind::PluginFailed` instead of turning it into a string. The kernel's row state already stores `Arc<ErrorKind>`, so the structure is kept.
- `ErrorKind::PluginFailed` drops the `plugin failed: ` prefix when displayed.
- Hosts read the structure: rutis-bridge provides `rutis_bridge::thrown(&rutis::Error) -> Option<&session::Error>` (finds the wrapped cross-language error); `session::Error` already has `code()` and `data()`, and gets `stack()` and `site()`. No new kernel types.
- The loader's `Failure { id, error: String }` and `ReconcileReport` do not change (they are compared for equality); code that needs the structure reads it from the row's state (the snapshot error in `EntryInfo`).

### 4.3 Error codes

An error a plugin throws itself keeps whatever `code` it has. `BindingError`s that rutis itself produces use the 6 codes below, with details in the existing `data`:

| code | When | `data` |
| --- | --- | --- |
| `not-declared` | A service not listed in `inject` was used | `{service}` |
| `unavailable` | The service has no provider right now | `{service, reason}` (the reason from `Loader::waits`) |
| `no-method` | The method does not exist | `{service, method, methods}` |
| `bad-argument` | Wrong number or type of arguments | `{service, method, argument, expected}` |
| `bad-value` | A value cannot be sent, or a message exceeds the limit | `{argument?, part, limit?}` |
| `peer-gone` | The runtime on the other side exited or disconnected during the call | `{runtime, reason}` |

`SyncWaitCycle` and `AggregateError` already have their own type names and get no code. Codes are for programs and do not appear in the line of text meant for people. Rows can be added to the table during implementation; existing rows do not change.

### 4.4 Locations across languages

Locations sent:

| Language | `stack` sent |
| --- | --- |
| TypeScript (Node, Bun) | Real stack (exists) |
| Python | `traceback` (exists) |
| Go | Ordinary errors: file:line of the declaration of the method that returned it (taken from `Type.Method(i).Func.Pointer()`, not the reflection stub); panic: full stack (exists) |
| Swift | file:line of the declaration of the method that threw it (recorded when the `RutisService` macro expands); Swift errors have no stack, so it only reaches the method — this is a boundary |
| Kotlin | Real JVM stack (today only "name: message") |
| Rust | The method that returned the error (recorded by code generated by `#[rutis::service]`); panics carry `std::backtrace` when `RUST_BACKTRACE` is set |

When receiving an error from the other side:

- The error object handed to the caller's code: type name, message, error code, `data`, the same as today, with no new fields.
- The other side's error graph is kept inside the SDK (Python, Go and Kotlin already do this), and is carried along unchanged when the caller rethrows it.
- Node and Bun no longer overwrite `error.stack` with the other side's stack: `error.stack` is the caller's own stack.
- When rutis prints, it shows both sides: the row on the other side that failed and its location, followed by the caller's location.

Also fix one item named in #677: Go's `Parallel` returns `*rutis.AggregateError` in a real host (`errors.As` holds, `Data` is not empty); today this is only correct in the test tools.

### 4.5 Text

Rule: speak only in terms the developer wrote.

- Never appears: rutis, Cordis, Rust (unless the plugin is written in Rust), binding, session, the raw error text of WireValue and serde, internal ids (`row:23.2`, `PluginId(3)`, labels containing NUL), internal plugins (`runtime-rows`, `rutis-bridge/link#py`), `<root>` / `<leaf>`, the host's private Rust type paths, SDK internal types (`HostProxy`, `RemoteFuture`, `e.graph`, `UNDEFINED`), `rollback failed`, `cannot resolve row … again`, `ThrownValue`.
- Rows are written by row name, runtimes by their name (`py`), and the host as "host".
- Rewrite every item listed in the body of #677 and in its "merged #680" section, each locked by a test on the new text; several of them must state the reason: say plainly when the default export is not the result of `definePlugin`; `entry file is gone` names the file; when a string or object is thrown, show it as is (`"just a string"`, `{code: 3}`), not `ThrownValue`; an unhandled rejection prints only the plugin's own frames.
- Compile errors from Rust macros and traits speak plainly through `#[diagnostic::on_unimplemented]`.

### 4.6 Formatting cannot fail

The code that prints errors, reads `site` and joins the locations of both sides must not itself become a new failure:

1. **Does not fail**: missing fields, wrong types, cycles or broken references in the error graph all fall back to "name: message"; when there is not even a name or message, it writes "an unreadable error". No panic, no throw, and the original error is passed to the caller as usual.
2. **Has limits**: printing a single error shows at most 64 stack lines and a message of at most 4 KiB; the excess is cut and marked "truncated"; nested `cause` / `errors` go at most 8 levels deep.
3. **Removes control characters**: before printing, NUL and other control characters are replaced with visible escapes (the 0.17.0 internal labels contained NUL, which made `grep` treat the whole log as binary).
4. **Readable by old and new sides**: an error without `site` prints as usual, just without "who, doing what"; an older side ignores `site` when it receives it.

These are locked by tests:

| Test | What it locks |
| --- | --- |
| Exact-text assertions | The output of each failure in §8, in each language (locations compare only the file name and method name, not line numbers); the precedent is `rutis-host/tests/errors.rs` |
| Forbidden words | After running the failure list, none of the words in §4.5 appear in the output |
| Fuzzing | A new fuzz target: arbitrary bytes as an error graph; decoding and printing do not panic, always produce output, and respect the limits (the existing fuzzing only covers message frames) |
| Cross-version | Two more cross-version meetings in the release gate: a new side sends `site` to an old side, and an old side sends an error without `site` to a new side |

These are registered in `docs/guarantees.md` (new entries: error formatting, where errors go, forbidden words), each naming the test that verifies it.

### 4.7 Errors are recorded on the row that failed

- The called method fails: the caller sees "call to X failed", and `site` is the called row.
- Config does not match the schema: recorded on the row whose config is wrong, not on the caller (#669 makes schemas take effect; its errors belong to rows by the rule here).
- Unhandled rejections / exceptions (#688). A runtime process is shared by several plugins, and how the process handles such errors is the platform's business:
  - Node: in `dev`, `async_hooks` records which row a promise belongs to, and that row fails; in `run` this is off (it adds cost to every promise), and the message is "an unhandled error in runtime node, source unknown".
  - Python: traced to the row through the task's context (`Task.get_context()` from 3.12; `_context` on 3.10 and 3.11).
  - Bun: cannot be traced; always "source unknown".
  - When it cannot be traced, the process does not exit. When a plugin itself calls `process.exit` / `os._exit`, the process exits as usual, and the report names the row that called it.
  - This changes "boundary rule 8" (an uncaught error ends the runtime process) and the handling in Bun's `main.ts`; `docs/guarantees.md` and the "plugins in the same runtime share one process" section of the plugin API guide are rewritten to match.

## 5. Current state

### 5.1 One snapshot

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
- The snapshot is for people viewing the state and for the host; plugins cannot get it.

### 5.2 In-flight calls

- Registered in `RowService::invoke`: the called row and service are its own, the method is an argument, and the caller comes from `caller_row()` (a call not made from inside a row is recorded as "host"). For async calls, the registration wraps the returned future and is removed when it ends. `RowService` does not store the service name today; it needs to be added. It only records; it does not change the call.
- What cannot be seen is written into the boundaries: calls within the same runtime; functions passed by reference (only "host → row" is visible, no method name); calls from in-process Rust plugins are recorded as "host".

### 5.3 Who reads it

| Where | How it reads |
| --- | --- |
| `rutis-host run` / `dev` | State lines print changes as before; waiting lines use `Loader::waits`; the line about what is stuck at shutdown uses the snapshot instead (counts rows only, names what they wait for) |
| `rutis-host status` | New command, prints one snapshot. `run` / `dev` open a read-only local socket (permissions 0600) that answers only the `status` request; the socket is placed in the system runtime directory (`$XDG_RUNTIME_DIR`, or the temporary directory when it is not set), its file name contains a hash of the config file path, and no file is written into the project directory; `status` finds it from the same config path (by default `rutis.json` in the current directory, or `--config`). Not provided on Windows in 0.18; written into the boundaries |
| Rust embedding | `Loader::status()`; `diagnostics()` gets a readable `Display` (uses names, does not list internal plugins by default); `RuntimeState` implements `Debug` |

`rutis/*` events do not change (they report changes only). The rutis-dev channel is not reused (it does not publish, and it carries commands that change things).

## 6. Time limits

### 6.1 Calls: no time limit

How long a call should wait is decided by the caller's business, so rutis sets no call time limit and does not cancel calls on behalf of either side. A caller that wants a limit uses its own language's way, and the cancellation reaches the other side (exists):

| Language | How |
| --- | --- |
| Python | `asyncio.wait_for(...)` |
| Go | `context.WithTimeout` |
| Swift | Cancel after a time limit inside a `Task` |
| Kotlin | `withTimeout { ... }` |
| Rust | `tokio::time::timeout(...)` (dropping the future cancels it) |
| TypeScript | `Promise.race` with a timer; the other side is not cancelled, the same as native JS (a promise itself cannot be cancelled) |

The plugin API guide includes this table. What rutis does is only make it visible: the state snapshot lists in-flight calls and how long they have been waiting (§5). A host that wants to limit the resources of third-party plugins is covered by #683 in 0.19 (a host option, off by default).

### 6.2 Shutdown: the time limit belongs to the host

| | Today | 0.18 |
| --- | --- | --- |
| Kernel `shutdown()` | No time limit | Unchanged: how long to wait is decided by the program that embeds rutis |
| Kernel `shutdown_with_timeout(limit)` | Reports the root when reached (`fiber PluginId(1) … still Unloading`) | When reached, names the stuck plugin and cleanup: cleanup registered with `ctx.effect` shows the file:line where it was registered (`#[track_caller]`), other cleanup (withdrawal of `provide`, cleanup of loader rows) shows the plugin or row name |
| rutis-host | 10 seconds (`RUTIS_SHUTDOWN_TIMEOUT` / `--shutdown-timeout`) | Unchanged; the line about what is stuck uses the snapshot instead |

rust-plugin.md currently says "cleanup has a time limit"; change it to say what is true: the time limit is decided by the host, rutis-host defaults to 10 seconds, and Rust embedding uses `shutdown_with_timeout`.

### 6.3 Handshake

30 seconds local, 10 seconds network, unchanged; only the text changes. This is the platform's own connection process.

### 6.4 Synchronous blocking hint

- When a Node, Bun or Python runtime is held by one synchronous method for 5 seconds (fixed), the runtime prints once: "runtime py has been held by weather.tomorrow for 5 seconds, and all other rows are waiting; it is a synchronous method (`def`), change it to `async def` and it will not hold the runtime".
- It only prints; it does not interrupt the method. Synchronous methods in Go, Swift and Kotlin do not hold the whole runtime, so they do not need it.

## 7. Output

- **Plugin output carries a prefix** (the terminal belongs to the host):
  - Node, Bun, Python (several rows share one process): the runtime adds `[weather] ` for the row currently running. Node and Bun wrap these entry points in an async context: `apply`, service methods, listeners, functions exported by reference; Python uses contextvars. What cannot be traced (Python threads, Node child processes and native modules that write to fds directly) gets the runtime name.
  - Go, Swift, Kotlin (one plugin per process): the host adds the runtime name when reading the output pipe; the SDKs do not change.
  - On by default in `dev`, off by default in `run` (plugin output in production is left as is); switched with `--prefix-output` / `--no-prefix-output`.
- **Python `logging` does not change**: not showing INFO by default is Python's own rule. The guide states: to see INFO, configure logging in the plugin (`logging.basicConfig(level=logging.INFO)`).
- **Rust embedding is visible by default**: besides listener errors, the default error sink also writes plugin or row load failures and runtime exits (the hint for waits longer than 10 seconds already exists in 0.17.2); each names the plugin or row. The output carries no platform name such as `[rutis]`; it starts with the plugin or row name. `CtxHost`'s existing error sink interface can replace it or turn it off.
- **Providing by name does not depend on whether rutis-bridge is linked** (#681): when a type marked `#[rutis::service]` is provided but the name registration is not linked in, startup writes once: "service calendar is provided by type but not registered by name: depend on rutis-bridge to use it by name and to check for conflicts".
- **State lines go to standard error**; plugin output goes to whichever stream it was written to.

## 8. What developers will see

### 8.1 The 14 failures (#663 debugging evaluation)

TypeScript as the example; other languages show the same content, with locations written their own way. This is the output that rutis prints.

| # | Failure | Output |
| --- | --- | --- |
| 1 | Misspelled dependency service name | `weather: waiting for lmm (no row provides it; did you mean llm?)` (0.17.2) |
| 2 | A service method throws, across languages | `chat: call to weather.tomorrow failed: KeyError: 'region'`, followed by two locations: `weather at weather.py:31 in tomorrow`, `called from chat at src/chat.ts:14:36` |
| 3 | Method does not exist / wrong argument | `chat: weather has no method tomorow (methods: today, tomorrow)`; `chat: weather.tomorrow: argument 1 should be a number, got "x"` |
| 4 | Error in `apply` | `weather: failed in apply: KeyError: 'region'` + `weather.py:12 in apply` |
| 5 | Import error | `weather: cannot load src/util.ts: SyntaxError … (src/util.ts:3:7)` (names the failing submodule) |
| 6 | A value cannot be sent | `chat: weather.tomorrow: argument 0 cannot be sent: a sparse array (index 1 is empty)`; the runtime does not exit (#675) |
| 7 | A listener fails | `weather: listener of weather/asked failed: …` + location; a `parallel` caller sees `2 listeners of weather/asked failed`, with each row and location listed below |
| 8 | A call does not return | `rutis-host status`: `chat → weather.tomorrow  42s` |
| 9 | Mutual calls / mutual dependencies | `chat: weather.today would wait on itself: chat → weather → chat (synchronous calls)`; mutual `inject`: `check` reports an error (§9) |
| 10 | A runtime exits midway | `runtime py: down (exited with status 1)`; `weather: waiting for runtime py (down: exited with status 1)`; in-flight calls: `chat: weather.tomorrow failed: runtime py exited during the call` |
| 11 | Breakpoints, reading logs | Output has the `[weather]` prefix; how debuggers attach is in #679 |
| 12 | Wrong config | `weather: config: units should be "metric" or "imperial", got "kelvin"` (recorded on weather) |
| 13 | Two plugins provide a service with the same name | `check`: `rows a and b both provide forecast in the same scope` and exits with 1; at run time: `b: forecast is already provided by a` |
| 14 | Viewing the current state | `rutis-host status`: each row's state and reason, each service's provider, in-flight calls |

### 8.2 From the host owner's evaluation (#663)

| Failure | Output |
| --- | --- |
| The host receives a plugin's error | `rutis_bridge::thrown(&err)` gives name, code, message, stack, site; the host rewrites by code |
| A plugin throws a string / object | `wordcount: call to tools.run failed: "need 5 words"` |
| An unhandled rejection in one plugin | `dev`: only that row fails; `run`: `runtime node: an unhandled error from an unknown row: …`, other rows continue |
| The default export is not the result of `definePlugin` | `wordcount: the default export of src/index.ts is not a plugin made with definePlugin` |
| A Python `def` method blocks | The blocking hint (§6.4) |

## 9. `check`

The second half of #676: cycles of mutual `inject`, and several rows providing the same service in the same scope, are warnings in 0.17.2 and become errors in 0.18 (exit with 1); the message names the rows and how to fix it.

## 10. Four things to know across languages

| Thing | Where it is pointed out |
| --- | --- |
| Values are copied | Plugin API guide; the `bad-value` message says "copied by value, an object's methods do not go across" (except the by-reference part after #687) |
| The other side may be absent | The `unavailable` and `peer-gone` messages say why it is absent |
| Debugging is done per process | #679's debugging guide; the pid of local runtimes in the state snapshot |
| Events across processes have no guaranteed order | Events guide |

## 11. Compatibility and migration

Written into "Migrating to 0.18" (both languages):

- **Rust hosts**: `ErrorKind::PluginFailed` is displayed without the `plugin failed: ` prefix; plugin errors are no longer strings, read the structure with `rutis_bridge::thrown()`. Code that matches on text must switch to error codes. Loader types (`Failure`, `ReconcileReport`) do not change.
- **Rust embedding (behavior change)**: load failures and runtime exits are written to standard error by default (can be turned off); the default output no longer starts with `[rutis]`. `shutdown()` does not change.
- **Node, Bun**: for cross-language errors received, `error.stack` becomes the caller's own stack (previously the other side's).
- **All languages**: error type names and fields do not change. Error text will change.
- **Node, Bun, Python runtimes**: unhandled errors no longer make the process exit ("boundary rule 8" rewritten).
- **`check`**: cycles and same-name providers exit with 1.
- **Protocol**: error nodes have one more optional field, `site`, which an older side ignores; the protocol version is not raised, and different versions work together as usual.

## 12. Implementation

| Issue | Content |
| --- | --- |
| #677 | §4 (4.1–4.6): `site`, error codes, locations in each language and handling on receipt, loader no longer turning errors into strings, `thrown()`, Go `Parallel`, text rewrites, formatting cannot fail together with its tests and guarantee entries, macro hints |
| #678 | §5, §6, §7: `Loader::status()`, in-flight calls, `rutis-host status`, the guide on callers setting their own limits, `shutdown_with_timeout` naming the stuck plugin, blocking hint, output prefix, Rust embedding default output, startup hint for providing by name |
| #688 | §4.7 which row unhandled errors belong to, boundary rule 8 |
| Second half of #676 | §9 |

Each issue is done when it matches §8: in each language, run the 14 failures and the failures from the host evaluation once, and the output matches the tables (locations written per language).
