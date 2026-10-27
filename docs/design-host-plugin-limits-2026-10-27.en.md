# The host keeps plugins in bounds: a process per plugin, call time limits, the greeting limit, the services a plugin may use

[中文](design-host-plugin-limits-2026-10-27.md)

Status: final, not implemented. Date: 2026-10-27 (reviewed the same day: removed the rutis.json forms, `runtimes.local`, `prefixed()`, the top-level default with per-field overrides, the options on remote runtimes, and the `not-allowed` code). Base: `main` `103c0d22`. Scheduled: 0.19.
Issue: [#683](https://github.com/arcships/rutis/issues/683). Source: the host owner's end-to-end evaluation in [#663](https://github.com/arcships/rutis/issues/663) ([report](https://github.com/arcships/rutis/issues/663#issuecomment-6411076725)).
Relation to 0.18: built on [0.18 diagnostics](design-diagnostics-2026-10-26.en.md). Error codes follow its §4.3 table, which gets one row, `timeout`. The calls that time out are the in-progress calls its §5.2 records. Error text follows its §4.5. Its §6.1, "the platform sets no call time limit", stays as it is; see §3.
Other 0.19 designs: [#185](https://github.com/arcships/rutis/issues/185) gives plugins the loader as a service named `loader`, as Cordis's `ctx.loader` does; a host that wants to limit which plugins may use it uses this design's `allowed_services` (§4.3). Section 3 of [#671](https://github.com/arcships/rutis/issues/671)'s host owner's guide, `docs/guide/open-host.md`, explains these options (§7).

Out of scope (none has a concrete user): rutis.json forms (the evaluated host embeds rutis in Rust), these options on remote runtimes, these options on Go/Swift `BinaryRuntimes`, resource limits (memory, CPU), time limits per service or method (the caller sets its own, 0.18 §6.1), time limits on listeners and `apply`, sandboxing, in-process Rust plugins.

## 1. The scenario and where it gets stuck

A host owner embeds rutis in Rust to build deskmate, a command-line assistant, and opens its extension points to third parties: plugins register "tools", and the host calls each tool's handler (a function passed by reference); the host provides two services, `tools` and `notes`. He wants:

1. A plugin that crashes or hangs does not take others down with it;
2. A handler that has not returned after 30 seconds is stopped, and a plugin process stuck in a busy loop is ended;
3. While debugging a plugin, its process can stop at the first line (`--inspect-brk`) until a debugger attaches;
4. Third-party plugins may use only `tools` and `notes`; using any other service is refused with a clear reason.

| Wanted | What he did in 0.17 | Where it gets stuck |
| --- | --- | --- |
| A process per plugin | A `LocalRuntime::node(..).named(..)` per plugin, plus a `Resolver` of his own (about 60 lines) | The `RuntimeResolver::node` he used accepts any npm name or file, so the first one took every row; no document says which resolver to use |
| Call time limit | A tokio timeout around every call, then `processes().kill()` (about 30 lines) | Covers only calls the host makes itself; a plugin calling a plugin is not covered |
| Greeting limit while debugging | Not possible | A local runtime that has not greeted within 30 seconds is ended (`LOCAL_RUNTIME_GREETING` is fixed) |
| Allowed services | Not possible | No documented way |

## 2. Existing seams

| Seam | Where | How this design uses it |
| --- | --- | --- |
| One process per runtime; several can run, each named | `LocalRuntime::named` | Unchanged; "a process per plugin" is a named runtime per plugin |
| Rows sent to a runtime by a `<runtime name>:` prefix | `RuntimeResolver::modules`: Python, Bun, remote runtimes and Go/Swift already use it. The Node runtime resolves npm names and `file:` itself (`located` in `runner.mjs`, which remote Node relies on), so `modules` already works for a local Node runtime | **No new form**: Node rows are written `<runtime name>:<npm name or file>` with `RuntimeResolver::modules`; only documentation and tests are added (§4.1) |
| Ending and restarting a process | `LocalRuntime::processes().kill()`; `FiberView::restart()` | Ends the process after a timeout; restarting is the host's decision, with the existing API |
| In-progress calls | Recorded by 0.18 in `RowService::invoke`, the caller from `caller_row()` | These are the calls that are timed; "who called whom" in the timeout message comes from here |
| Cancellation reaching the other side | Exists in every language (0.18 §6.1's table) | A timeout is the host cancelling on the caller's behalf |
| Greeting limit | Remote: `Retry::handshake`, `AccessPointConfig::handshake` can already be set; local: fixed at 30 s | Add a setting for local runtimes, written as for remote ones |
| A leaf runtime leases only the services in `inject` | `RuntimeResolver` gets the plugin's `inject` when resolving | The allowed services are checked here |
| `share_registered_only`, read interception | `ServiceCatalog`, `intercept_require_as` | Not used; they are not permissions, and the guide says so (§7) |

Cordis has no processes and no list of "only these services". Its rows have `inject` (adds dependencies) and `isolate` (gives a row a separate copy of a service name). Hiding a name with `isolate` leaves a required dependency waiting forever with no reason, which does not meet the issue's "refused with a clear reason", so a list is added. It is set on the process, because plugins in one process can reach each other through the language itself and cannot be limited there.

## 3. Why this does not conflict with 0.18's "the platform sets no call time limit"

0.18 §2 says "the platform only shows; it does not make business decisions", so rutis sets no call time limit and has no default one. This design keeps that: the host owner writes the value, and without it behaviour is the same as now and as in Cordis. The end of 0.18 §6.1 already assigns this to #683 (a host option, off by default). It adds the two things a caller cannot do by limiting its own calls: when the caller is not the host (plugin A calls plugin B), and when a plugin is stuck in synchronous code (a cancellation reaches it but cannot stop it; only ending the process can). When both limits apply, whichever comes first wins.

## 4. Decisions

| # | Decision | Section |
| --- | --- | --- |
| H1 | A process per plugin (or per group of plugins) = a named `LocalRuntime`; its rows use the existing `RuntimeResolver::modules`, written `<runtime name>:<npm name or file>`. No new API | 4.1 |
| H2 | Call time limit: `LocalRuntime::call_limit(Duration)`; a call into the runtime's plugin code that exceeds it is cancelled and fails with `timeout`; `kill_on_timeout()` also ends the process | 4.2 |
| H3 | Allowed services: `LocalRuntime::allowed_services([..])`, limiting only services from outside the runtime; a row whose required dependency is not listed fails to start, other calls fail with the existing `unavailable` | 4.3 |
| H4 | Greeting limit: `LocalRuntime::greeting(Duration)`, with `Duration::MAX` for no limit, as remote `Retry::handshake` does | 4.4 |
| H5 | All on `LocalRuntime` only, off by default, no existing behaviour changes; a whole-host setting is the same value given to every runtime, with no separate API | 6 |

### 4.1 A process per plugin

```rust
for plugin in &plugins {                      // a process per third-party plugin
    let runtime = LocalRuntime::node(&runtime_pkg, &plugin.dir)
        .named(&plugin.id)                    // runtime name: two or more of a-z, 0-9, -
        .call_limit(Duration::from_secs(30))
        .kill_on_timeout()
        .allowed_services(["tools", "notes"]);
    let rows = Arc::new(RuntimeResolver::modules(runtime.handle()).with_catalog(&catalog));
    root.plugin(runtime);
    chain = chain.with_shared(rows.clone());
    resolvers.push(rows);                     // one RuntimeRowsPlugin each after the loader, as now
}
// row: { "id": "wordcount", "name": "wordcount:wordcount-plugin" }
```

- A group of plugins sharing a process: their rows use the same prefix.
- `RuntimeResolver::node` is unchanged and stays the form for the default Node runtime (no prefix); it already refuses names with a runtime prefix, so it does not take other runtimes' rows.
- Cost: `modules` asks the runtime on every resolution (as for Python and Bun) instead of caching by the package's `version`.
- The work is only this: the documentation of `RuntimeResolver::modules` and the rutis-loader README say it is used for Node too (they mention only Python now), plus tests.

### 4.2 Call time limit

- **Which calls**: calls from this side into the runtime's plugin code — calling a service by name (`RowService::invoke`) and functions passed by reference (deskmate's tool handlers). The same whether the caller is the host, a plugin in another runtime or a Rust plugin. Timed from sending to completion.
- **Not covered**: calls between plugins in the same runtime (they do not go through the host); cleanup (shutdown limits belong to the host, 0.18 §6.2).
- **After a timeout**: the caller gets an error with code `timeout`, and the call is cancelled on the other side (each language's existing cancellation).
- **`kill_on_timeout()`**: also ends the runtime's process (`processes().kill()`). Other in-progress calls into that process fail with 0.18's existing `peer-gone`. Whether to restart is the host's decision: call the existing `FiberView::restart()` on the fiber `root.plugin(runtime)` returned. Why it is needed: a TypeScript promise cannot be cancelled, and a Python `def` method or a JS busy loop occupies the whole process, so a cancellation reaches it but cannot stop it; when a plugin calls a plugin, the host is not in the call chain and cannot end the process itself.

### 4.3 Allowed services

- `allowed_services` lists the services **from outside the runtime** (the host's services, services provided by rows of other runtimes) that the runtime's plugins may use. Services provided by rows of the same runtime to each other are not limited.
- The check runs when the loader resolves the row, using the `inject` the plugin declares:
  - A required dependency not on the list: the row fails to start with the reason, instead of waiting forever.
  - An optional dependency not on the list: not leased into the process, so the plugin sees it as absent; calling it anyway fails with the existing `unavailable`, its `reason` saying the host does not allow it.
- Limits plugins only, not the host's own calls (`call_service`).
- #185's `loader` service: a host that does not want some plugins to use it leaves it off those runtimes' lists. No limit by default, as in Cordis.

### 4.4 Greeting limit

- `LocalRuntime::greeting(Duration)`; 30 seconds when not set, as now; `Duration::MAX` for no limit, for a debugger stopped at the first line. The existing remote `Retry::handshake` and `AccessPointConfig::handshake` are unchanged.
- With no limit, a process that starts but never speaks stays "starting"; 0.18's `Loader::status()` shows which runtime its rows wait for.

## 5. What developers see

| # | Input | Output |
| --- | --- | --- |
| 1 | `wordcount`'s handler has not returned after 30 seconds | The host's caller: `a function of wordcount did not finish within 30s (the host's limit for runtime wordcount); the call was cancelled`; for a call by name, `chat: call to weather.tomorrow did not finish within 30s (the host's limit for runtime agent); the call was cancelled`. Code `timeout`, `data` `{runtime, limit, service?, method?}` |
| 2 | As above, with `kill_on_timeout()` | The previous message ends with `; runtime wordcount was ended`; the runtime's state is `down (ended by the host: a function of wordcount did not finish within 30s)` |
| 3 | A required dependency of `wordcount` is `llm`; the list is `tools`, `notes` | `wordcount: uses llm, which the host does not let runtime wordcount use (allowed: tools, notes)`; the row fails |
| 4 | An optional dependency `llm` is not on the list, and the plugin calls it anyway | An error in the plugin's code: `llm is not available to wordcount: the host does not let runtime wordcount use it`, code `unavailable` |
| 5 | `greeting(Duration::MAX)`, the runtime stopped at `--inspect-brk` | `Loader::status()`: `runtime wordcount: starting`, its rows `waiting for runtime wordcount` |

Plugin authors write nothing new: their code sees only ordinary errors (name, message, code) and handles them like any other.

## 6. Compatibility

- All of it is new `LocalRuntime` methods, off by default: without them, behaviour and output are those of 0.18, and nothing goes into "Migrating to 0.19".
- The error code table (0.18 §4.3) gets one row; existing rows do not change:

| code | When | `data` |
| --- | --- | --- |
| `timeout` | A call into a runtime exceeded the limit the host set | `{runtime, limit, service?, method?}` |

- No protocol change: cancellation and process exit are existing messages; runtimes need no upgrade.

## 7. Guide: "Keeping plugins in bounds"

As #671 arranges, the explanation goes into section 3 of `docs/guide/open-host.md` (both languages); rust-host.md's reference table gets four rows, `call_limit`, `kill_on_timeout`, `allowed_services` and `greeting`, and its `RuntimeResolver::modules` row says Node uses it too. Section 3 covers:

1. A process per plugin: §4.1's loop, rows written `<runtime name>:<package>`.
2. The call time limit and `kill_on_timeout`, and restarting afterwards with `FiberView::restart()`.
3. Allowed services.
4. The greeting limit: `Duration::MAX` while a debugger is stopped at the first line.
5. What is not a limit, a sentence each: `share_registered_only` and read interception; plugins in one process can reach each other; plugin processes have all the rights of the host's user.

The Plugin API guide's section "plugins in one runtime share a process" gets one sentence: the host can put plugins in their own processes and set time limits, and plugins then get `timeout` and `unavailable` errors.

## 8. Implementation

One issue (#683), three steps:

| Step | Content | Done when |
| --- | --- | --- |
| 1 rutis-bridge | `LocalRuntime::call_limit`, `kill_on_timeout`, `greeting`; at the two entries into a runtime (by name, by reference), timing, cancelling and ending the process as set; the `timeout` code | A test each for Node and Python: an async method times out and is cancelled; a synchronous one is ended by `kill_on_timeout` while other runtimes carry on; a function passed by reference times out; `greeting` at 1 second and at `Duration::MAX` |
| 2 rutis-loader | Checking `allowed_services` at resolution (required fails, optional not leased); documenting `RuntimeResolver::modules` for Node | Two Node runtimes with `modules`, each row in its own process, one crashing while the other carries on; the three list cases (allowed, required not allowed, optional not allowed then called); rows of the same runtime providing to each other unaffected; word-for-word assertions for every row of §5 |
| 3 Docs | open-host.md section 3 (#671), rust-host.md's reference table (§7), `host/plugin-limits` registered in `docs/guarantees.md` (each entry naming the test that verifies it) | Doc link check and doc example check pass |

The issue's "Done": steps 1–2 are "the options and tests", step 3 is "the guide section" (the issue says rust-host.md; per #671 it is section 3 of open-host.md).
