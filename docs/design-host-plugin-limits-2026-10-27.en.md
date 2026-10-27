# The host keeps plugins in bounds: a process per plugin, call time limits, the greeting limit, the services a plugin may use

[中文](design-host-plugin-limits-2026-10-27.md)

Status: draft, not implemented. Date: 2026-10-27. Base: `main` `103c0d22`. Scheduled: 0.19.
Issue: [#683](https://github.com/arcships/rutis/issues/683). Source: the host owner's end-to-end evaluation in [#663](https://github.com/arcships/rutis/issues/663) ([report](https://github.com/arcships/rutis/issues/663#issuecomment-6411076725)).
Relation to 0.18: built on [0.18 diagnostics](design-diagnostics-2026-10-26.en.md). Error codes follow its §4.3 table, which gets two rows. The calls that time out are the in-progress calls its §5.2 records. Error text follows its §4.5. Its §6.1, "the platform sets no call time limit", stays as it is; §3 says why the two do not conflict.
Other 0.19 designs: the `runtimes.remote` entries of [remote runtime consistency](design-remote-runtime-consistency-2026-10-26.en.md) (#695) take this design's `limits` and `greeting`. [#185](https://github.com/arcships/rutis/issues/185) (the host writes its own plugins) gives plugins the loader as a cross-language service named `loader`, as Cordis's `ctx.loader` does. A host that wants to limit which plugins may use it uses this design's `limits.services` (§4.4). Section 3 of [#671](https://github.com/arcships/rutis/issues/671)'s host owner's guide, `docs/guide/open-host.md`, explains these options (§7). [#670](https://github.com/arcships/rutis/issues/670)'s `rutis-host call` is a call by the host (§4.3, §4.4).

Out of scope:
- **Resource limits** (memory, CPU, open files): not an option. Nobody has asked to limit a particular resource. The operating system's tools (cgroups, `systemd-run` and `prlimit` on Linux; macOS and Windows each differ) already work per process, so once each plugin can have its own process, they work per plugin. The guide shows how (§7).
- **Time limits per service (or method)**: the one who cares how long a service may take is its caller, and 0.18 §6.1 already shows how each language does it. What the host guards against is runaway third-party code, and a limit per plugin (process) covers that.
- **A time limit or service list for one row in a shared process**: plugins in one process can reach each other through the language itself (the Cordis context, Python's `import`), so a service list cannot hold there. Ending the process on a timeout would also end its neighbours. So the options are set on a process (a runtime), and a row that needs its own limits gets its own runtime.
- **Limiting which services a plugin provides or which events it emits**: a second provider of a name already fails the later row (0.17.2, 0.18 §9), and the host's own services are provided before the loader starts. Nobody has asked to limit events.
- **Sandboxing**: plugin processes still run with all of the host user's permissions (files, network). This design covers what a plugin can do through rutis.
- In-process Rust plugins (dylibs included): they share the host's process, so they cannot be ended or blocked. They are not covered, and the host's own Rust code is trusted anyway.
- Starting Node or Python runtimes on demand, a `rutis-host` switch that gives every row its own process, a command to cancel one in-progress call: nobody has asked for these.

## 1. The scenario and where it gets stuck

A host owner builds deskmate, a command-line assistant, and opens it to third parties. Plugins register "tools", and the host calls the tools' handlers. The host provides two services to plugins: `tools` and `notes`. The owner wants:

1. a plugin that crashes or hangs not to take others down with it;
2. a handler that has not returned after 30 seconds to be stopped, and a plugin process stuck in a busy loop to be ended and restarted if needed;
3. to stop a plugin's process at its first line (`--inspect-brk`) while a debugger attaches;
4. third-party plugins to use only `tools` and `notes`, and to be refused, with a clear message, when they use anything else.

In 0.17 the owner read crate sources and wrote about 160 lines to get part of the first two. The other two could not be done at all:

| Need | How it is done now | Where it gets stuck |
| --- | --- | --- |
| A process per plugin | One `LocalRuntime::node(..).named(..)` per plugin, plus a hand-written `Resolver` that sends each row to its own process (about 60 lines) | `RuntimeResolver::node` accepts any npm name or file, so a row cannot say which runtime it uses. rutis.json allows one runtime per language |
| A call time limit | A tokio timeout around each call, then `processes().kill()` on timeout (about 30 lines) | This covers only the calls the host itself makes, not plugin-to-plugin calls or a hung `apply` |
| The greeting limit while debugging | Not possible | A local runtime that has not greeted within 30 s is ended (fixed in `LinkConfig::local_runtime()`) |
| Which services a plugin may use | Not possible | There is no documented way |

## 2. Existing seams

| Seam | Where | How this design uses it |
| --- | --- | --- |
| One process per runtime; several runtimes can run, each with its own name | `LocalRuntime::named` (`crates/rutis-bridge/src/runtime/local.rs`) | Unchanged. "A process per plugin" means a named runtime per plugin |
| A `<runtime name>:` prefix sends a row to a runtime | `RuntimeResolver::modules` (Python, Bun, remote); Go/Swift already run one runtime per binary | Node takes this form too (§4.1). This is the only missing piece |
| Ending a process | `LocalRuntime::processes().kill()`; a runtime's `restart` policy (rutis-host), `BinaryRuntimes::supervise` (Go, Swift) | Used to end the process after a timeout. Afterwards the existing `restart` applies |
| In-progress calls | 0.18 records them in `RowService::invoke`, with the caller from `caller_row()` | These are the calls that get the limit. The timeout message takes "who called whom" from here |
| Cancellation reaches the far side | Already in every language (the 0.18 §6.1 table) | A timeout is the host cancelling on the caller's behalf |
| Greeting limit | Remote: `Retry::handshake` and `AccessPointConfig::handshake` can already be set. Local: `LOCAL_RUNTIME_GREETING`, fixed at 30 s | Add a setting for local runtimes. rutis.json can set both |
| Only the services named in `inject` are leased | A leaf runtime leases only the services in `inject` into its process. Calling any other service gives `not-declared` | The service list adds one more check here |
| `share_registered_only`, read interceptors | `ServiceCatalog`, `intercept_require_as` | **Not used.** `share_registered_only` decides which names cross languages and leaves the rest to Cordis, which still serves them to Node plugins, so it limits nothing. Read interceptors match on Rust types and block Rust readers only, so they cannot reach rows of other languages. The guide says that neither one is a permission |

## 3. Why this does not conflict with 0.18's "the platform sets no call time limit"

0.18 §2 says "the platform observes; it does not make business decisions". How long a call may take is a business decision, so rutis sets no call time limit and has no default one. This design keeps that rule:

- **The host owner makes the decision.** The owner runs the business. When a host opens itself to third parties, "third-party code gets at most 30 seconds" is the owner's business decision. rutis gives the owner a place to write that decision down. It does not choose the number.
- **Off by default.** Without `limits`, everything works as it does now, and as in Cordis.
- **There are two cases a caller's own time limit cannot handle.** In the first, the host is not the caller: plugin A calls plugin B, and the host is not in the call chain. In the second, a plugin is stuck in synchronous code: cancellation reaches it but cannot stop it, and only ending the process can.

0.18 §6.1 ends by saying "a host that wants to limit third-party plugins' resources belongs to 0.19's #683 (a host option, off by default)". This design is that option. When both a caller's limit and the host's limit apply, the one that runs out first wins. The handshake limits in 0.18 §6.3 (30 s local, 10 s network) remain the platform's defaults. This design only lets the host change them.

## 4. Decisions

| # | Decision | Section |
| --- | --- | --- |
| H1 | A process per plugin (or per group of plugins) is a named runtime. A Node row can be written `<runtime name>:<npm name>` to choose its runtime. rutis.json gets `runtimes.local`, a list of further local runtimes | 4.1 |
| H2 | Time limits and allowed services are one set of options on a runtime, `limits`, covering all its plugins. A top-level `limits` in rutis.json is the default for every runtime | 4.2 |
| H3 | Call limit `call`: any call into plugin code in the runtime (by name, by reference, a listener, a row starting) that runs past it fails with code `timeout`. `stop` decides whether a timeout stops just the call (the default) or ends the process | 4.3 |
| H4 | Allowed services `services`: the runtime's plugins may use only the listed services from outside the runtime. A row whose required dependency is not listed fails to start. Any other such call fails with `not-allowed` | 4.4 |
| H5 | Greeting limit `greeting`: every runtime entry takes a number of seconds or `"off"`. Local and remote use the same field. The defaults stay 30 s and 10 s | 4.5 |
| H6 | All of these are options, off by default, and no existing behaviour changes. The error-code table gets two rows, `timeout` and `not-allowed` | 6 |

### 4.1 A process per plugin

**Rust embedding**: `RuntimeResolver::node(handle)` still accepts npm names and files, so existing embeddings are unaffected. It also accepts `<its runtime name>:<npm name>`. A new `.prefixed()` makes it accept only the prefixed form. Python and Bun already use prefixes and do not change.

```rust
for plugin in &plugins {                      // one process per third-party plugin
    let runtime = LocalRuntime::node(&runtime_pkg, &plugin.dir).named(&plugin.id).limits(limits.clone());
    let rows = Arc::new(RuntimeResolver::node(runtime.handle()).prefixed().with_catalog(&catalog));
    root.plugin(runtime);
    chain = chain.with_shared(rows.clone());
    resolvers.push(rows);                     // one RuntimeRowsPlugin each after the loader, as now
}
// row: { "id": "wordcount", "name": "wordcount:wordcount-plugin" }
```

**rutis.json**: `runtimes.node`, `py` and `bun` remain each language's default runtime. A new `runtimes.local` lists named local runtimes, one per entry:

```json
"runtimes": {
  "node": { "project": "." },
  "local": [
    { "name": "wordcount", "language": "node", "restart": "on-failure" },
    { "name": "agent",     "language": "python", "limits": { "services": ["notes"] } }
  ]
},
"rows": [
  { "id": "wordcount", "name": "wordcount:wordcount-plugin" },
  { "id": "weather",   "name": "agent:weather" }
]
```

- `language` is `node`, `python` or `bun`. The other fields are the same as that language's entry (`project`, `runtime`, `python`, `program`, `restart`). A field left out takes its value from `runtimes.<language>`, or the default if that entry does not set it either. Go and Swift already run one runtime per binary, so they are not in `local`.
- `name` follows the rules for runtime names: two or more of `a-z`, `0-9` and `-`. It must not repeat `node`, `py`, `bun`, a Go or Swift runtime, or a `runtimes.remote` name. A repeated name is an error at start.
- To put a group of plugins in one process, give their rows the same prefix. To give each plugin its own process, write one `local` entry per row.
- These runtimes start with the host, like the language defaults. `runtimes.local` is read only at start: adding an entry while the host runs asks for a host restart, the same rule `runtimes.remote` and the language entries follow today, which #691 relies on. When `dev` sees a plugin change, it restarts only that plugin's runtime (the existing rule: a shared runtime is never restarted for one plugin).
- A remote Node runtime also accepts `<name>:<npm name>`, so one machine and many are written the same way (#695).

### 4.2 Options live on the runtime

```jsonc
"limits": {
  "call": 30,                  // seconds; no limit when left out
  "stop": "call",              // what a timeout stops: "call" (default) or "process"
  "services": ["tools", "notes"]  // no limit when left out
}
```

- `limits` can be written at the top level, where it is the default for every runtime, and on any runtime entry. That means `runtimes.node`, `py`, `bun`, `go` and `swift` (for each of their binaries), each `local` entry, and each `remote` entry (including every connection of an `instances` entry). A field set on an entry overrides the same field at the top level.
- In Rust, the type is `rutis_bridge::runtime::Limits`. It is set with `RuntimePlugin::limits`, `LocalRuntime::limits` or `BinaryRuntimes::limits`. Rust has no host-wide setting: the host gives the same `Limits` to each runtime.

```rust
let limits = Limits::new().call(Duration::from_secs(30)).stop(Stop::Process).services(["tools", "notes"]);
```

### 4.3 Call time limit

- **Which calls it covers**: every call from this side into plugin code in the runtime. That means:
  - a service called by name (`RowService::invoke`, where 0.18 records in-progress calls);
  - a function passed by reference (deskmate's tool handlers are these);
  - an event listener;
  - a row starting: importing its entry module and running `apply`.

  It does not matter whether the caller is the host, a plugin in another runtime, or a Rust plugin in the host. `rutis-host call` (#670) goes through `call_service` and is recorded as "the host". When it enters a runtime with a limit, the limit applies to it too. The time runs from when the call is sent until it ends, so for a remote runtime it includes the network.
- **What it does not cover**: calls between plugins in the same runtime, which do not pass through the host and do not show in 0.18's status either. Cleanup is not covered either, because the host owns the shutdown limit (0.18 §6.2).
- **After a timeout**: the caller gets an error with code `timeout`, and the call is cancelled on the far side, using each language's existing cancellation. A row that times out while starting fails.
- **`stop: "process"`**: also ends the runtime's process with `processes().kill()`. After that, the runtime's `restart` applies: rutis-host's `no`, `on-failure` or `exit`, or Go/Swift's `RestartPolicy`. In a Rust embedding, the host decides whether to restart it, with an existing interface: `FiberView::restart()` on the fiber `root.plugin(runtime)` returned starts a new process, and its rows come back. For Go/Swift, use the existing `BinaryRuntimesHandle::restart(name)`. No new interface is needed. Other in-progress calls into that process fail with 0.18's existing `peer-gone`. This works only for local runtimes. On a remote runtime it is an error at start, because the host cannot end a process on another machine.
- **Why `process` is needed**: cancellation depends on the plugin's cooperation. A TypeScript promise cannot be cancelled (0.18 §6.1). A Python `def` method or a JS busy loop holds the whole process, and cancellation cannot stop it. Then only ending the process lets the other plugins go on.
- If the caller also set its own limit, the one that runs out first wins.

### 4.4 Allowed services

- `services` lists the service names, **from outside the runtime**, that the runtime's plugins may use: the host's services and services provided by rows of other runtimes. Services that rows of the same runtime provide to each other are always allowed, since nothing can stop that inside one process.
- The loader checks this when it resolves the row, against the `inject` the plugin declares:
  - A required dependency that is not listed: the row fails to start, and the error's code is `not-allowed`. The row is not left waiting, because waiting for a service that will never come only looks like a hang.
  - An optional dependency that is not listed: it is not leased into the process, so the plugin sees it as absent. If the plugin calls it anyway, the call fails with `not-allowed`.
- `rutis-host check` applies the same rule without starting anything, and exits with 1.
- [#185](https://github.com/arcships/rutis/issues/185) provides the loader to plugins as the service `loader`. A host that wants to limit which plugins may use it sets `limits.services` on those plugins' runtimes and leaves `loader` out. By default nothing is limited, as in Cordis.
- It limits what plugins use, not the host. The host's own calls (`call_service` in Rust, `rutis-host call`) are not limited by `services`.
- Without `services`, nothing changes.

### 4.5 Greeting limit

- rutis.json: every runtime entry (`node`, `py`, `bun`, `go`, `swift`, `local`, `remote`) can set `"greeting": 120` (seconds) or `"greeting": "off"` (no limit, for a debugger stopped at the first line). Without it, the limits stay 30 s local and 10 s remote. It is not part of the top-level `limits`: it is about starting a runtime, and is set per runtime.
- In Rust: `LocalRuntime::greeting(Option<Duration>)`, where `None` means no limit, and `BinaryRuntimes::greeting` in the same way. The existing remote settings, `Retry::handshake` and `AccessPointConfig::handshake`, do not change (#695 M7: no Rust API changes). For no limit there, use `Duration::MAX`.
- With the limit off, a process that starts and never speaks stays "starting" for good. 0.18's status shows which runtime its rows wait for.

## 5. What developers see

What the host owner writes and the output that follows (rutis-host). A Rust embedding prints the same error text, without the hint line in item 6.

| # | Input | Output |
| --- | --- | --- |
| 1 | `wordcount`'s handler has not returned after 30 s (`call: 30`) | The host as caller: ``a function of wordcount did not finish within 30s (the host's limit for runtime wordcount); the call was cancelled``. A call by name: `chat: call to weather.tomorrow did not finish within 30s (the host's limit for runtime agent); the call was cancelled`. Code `timeout`, `data` `{runtime, limit, service?, method?}`, `site` the called row |
| 2 | As above, with `stop: "process"` and `restart: "on-failure"` | The line ends `; runtime wordcount was ended` instead, followed by `runtime wordcount: down (ended by the host: a function of wordcount did not finish within 30s)`, then the existing restart lines |
| 3 | `apply` hangs | `weather: did not finish starting within 30s (the host's limit for runtime agent)`. That row fails, the others run |
| 4 | `wordcount` requires `llm`; `services: ["tools", "notes"]` | `wordcount: uses llm, which the host does not let runtime wordcount use (allowed: tools, notes)`. The row fails. `check` prints the same line and exits with 1 |
| 5 | An optional dependency `llm` is not listed, and the plugin calls it anyway | The error in the plugin's code: `llm is not available to wordcount: the host does not let runtime wordcount use it`, code `not-allowed`, `data` `{service, runtime}` |
| 6 | A local runtime has not greeted within 30 s | 0.18's line (the runtime is `down`, and the line says it did not greet within 30 s). rutis-host adds `to wait longer, or not at all while debugging, set "greeting" on runtime wordcount` |
| 7 | `greeting: "off"`, the runtime stopped at `--inspect-brk` | The runtime shows as starting. `rutis-host status`: `runtime wordcount: starting`, and its rows show `waiting for runtime wordcount` |
| 8 | `stop: "process"` on a remote runtime | An error at start: `runtime gpu: limits.stop "process" needs a runtime this host starts; gpu is remote` |
| 9 | A repeated `local` name, or a wrong language | `runtimes.local: wordcount is already the name of another runtime`; `runtime wordcount: the language is node, python or bun, not go` |
| 10 | A row written `wordcunt:wordcount-plugin` (misspelt prefix) | The same as now when no resolver accepts a name (0.18's text). Nothing new is printed |

Plugin authors write nothing new. Their code sees ordinary errors (name, message, code) and handles them like any other error.

## 6. Compatibility

- Everything is a new option, off by default. Without the options, behaviour and output are as in 0.18, so nothing goes into "Migrating to 0.19".
- `RuntimeResolver::node` now also accepts `<its own name>:<npm name>`. No resolver used to accept such a name (the row failed with "no such plugin"). Now the runtime of that name accepts it, and existing rows are unaffected.
- The error-code table (0.18 §4.3) gets two rows, and the existing rows do not change:

| code | When | `data` |
| --- | --- | --- |
| `timeout` | A call into a runtime ran past the host's limit | `{runtime, limit, service?, method?}` |
| `not-allowed` | A service the host does not let the runtime use | `{service, runtime, allowed?}` |

- The protocol does not change: cancellation and ending a process use existing messages, and runtimes need no upgrade.

## 7. Guide: "Keeping plugins in bounds"

As #671 arranges it, the explanation goes into section 3 of the host owner's guide, `docs/guide/open-host.md` ("Keep plugins in check", both languages, using #671's `jot` example). rust-host.md and rutis-host.md only get this design's options in their reference tables: `Limits`, `greeting`, `prefixed()`, and rutis.json's `limits`, `greeting` and `runtimes.local`. Section 3 covers:

1. A process per plugin: `runtimes.local` and the prefixed row names in rutis.json, and in Rust, the loop with `named` + `prefixed()` (the example in §4.1).
2. `limits`: the three fields, where they can be written, and which setting overrides which. Use `stop: "process"` together with `restart`.
3. `greeting`: turn it off when a debugger stops at the first line.
4. What is not a limit: one sentence each on `share_registered_only` and read interceptors. Plugins in one process can reach each other. Plugin processes have all of the host user's permissions.
5. How to set resource limits: once a plugin has its own process, use the operating system's tools on that process.
   - For the whole host on Linux: `systemd-run --scope -p MemoryMax=2G -p CPUQuota=200% rutis-host run`. The cgroup covers every runtime process the host starts.
   - For one runtime: replace the interpreter with a script that sets the limit and then `exec`s the interpreter, for example `prlimit --as=1073741824 -- python3 "$@"`. Point `runtimes.py.python`, `runtimes.bun.program` or Rust's `LocalRuntime::interpreter` at the script.
   - macOS and Windows have no command-line equivalent. The host uses the system's own interfaces (on Windows, job objects).
6. The plugin API guide's section "plugins in one runtime share a process" gets one sentence: the host may put plugins in their own processes and set time limits, and plugins then receive `timeout` and `not-allowed` errors.

## 8. Implementation

One issue (#683), in four steps:

| Step | Content | Done when |
| --- | --- | --- |
| 1 rutis-bridge | `Limits` and `Stop`; `RuntimePlugin::limits`, `LocalRuntime::limits` / `greeting`. At the places a call enters a runtime (`RowService::invoke`, calls of functions passed by reference, listeners, loading a row and its `apply`): time the call, cancel it, and end the process when `stop` says so. The `timeout` and `not-allowed` codes | For each language (Node, Bun, Python, Go, Swift), tests show: an async method that times out is cancelled; with `stop: process`, a method holding the process synchronously gets the process ended while other runtimes are unaffected; a function passed by reference times out; `apply` times out. One test each for a local `greeting` of 1 s and of `None` |
| 2 rutis-loader | Prefixed Node names and `prefixed()`. The `services` check at resolution: a required dependency fails the row, an optional one is not leased. `BinaryRuntimes::limits` / `greeting` | Two Node runtimes each run their own rows in their own process, and one crashing leaves the other running. Tests cover the three `services` cases: allowed, a required one not allowed, and an optional one not allowed and then called. Rows of the same runtime that provide services to each other are not limited |
| 3 rutis-host | Top-level and per-entry `limits` and `greeting`; `runtimes.local`; the static check in `check`; the §5 output; `dev` restarts only the changed plugin's runtime | Each §5 item has a test asserting its exact text, and the error text contains none of 0.18 §4.5's banned words. The deskmate scenario runs end to end: two TS plugins in their own processes, the one in a busy loop ended and restarted, and a service the host does not allow refused |
| 4 Docs | Section 3 of open-host.md (#671's guide, written after this step); the options in the reference tables of rust-host.md and rutis-host.md (§7); a `host/plugin-limits` entry in `docs/guarantees.md` that names the test verifying each rule | The doc link check and the doc example check pass |

The issue's "Done" list maps as follows: steps 1–3 are "the options and tests", and step 4 is "the guide section". The issue puts that section in rust-host.md; following #671, it goes into section 3 of open-host.md instead.
