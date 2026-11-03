# Error messages: whose error it is, a name two rows provide, rows that wait for each other, calls in flight, frames from inside

[中文](design-error-messages-2026-11-03.md)

Status: draft. Date: 2026-11-03. Base: `main` `d2a8d069` (0.20.1).
Issue: [#937](https://github.com/arcships/rutis/issues/937), part of [#663](https://github.com/arcships/rutis/issues/663) (D2, D3, D5, O6); it is also the issue this design is implemented under, in PRs for E-1 to E-8 of §6, each with the completion criteria written there.
Basis: [#663](https://github.com/arcships/rutis/issues/663)'s 0.19.0 evaluation (14 faults, 34 / 56 written clearly in all four languages); the [diagnostics design](design-diagnostics-2026-10-26.md) §4.1 (`site`), §4.4 (`rethrown`), §4.5 (words), §4.6 (printing never fails), §5.2 (calls in flight, and the boundaries it writes down); [declared services](design-declared-services-2026-10-29.md) (the wording of `not-declared`); [object references](design-object-refs-2026-10-27.md) §4 (in one process a service is the object itself).

Out of scope:

- How the platform attributes **unhandled** errors (#688, diagnostics design §4.7): this design changes only what those rows print when they fail.
- Call and shutdown deadlines (diagnostics design §6): the deadlines stay as they are; only the lines printed when they run out change.
- The messages of a remote (another machine) node: the same rules as a local runtime, changed together with each runtime; not written out separately here.
- An interface for a host to rewrite platform names: not needed while no platform name is printed.
- The words of events, config validation and value conversion: those change in their own designs.

## 1. Now, and what to solve

The issue's five groups, in order. Each with today's output and the code that produces it.

### 1.1 Inside one runtime, the error is attributed to the caller

> Inside one runtime, a service the caller's row calls fails, and the error is attributed to the caller: llm throws and the output says `in weather:`, with no `in llm:` / `called from:` (Go, TS). Across processes both parts are there.

Three pieces of code make that:

1. **`site` is filled by the host, and only where a call crosses.** When the host hands a call to a runtime row's `RowService`, it adds `site` from the called row and `service.method` (`crates/rutis-bridge/src/runtime/rows.rs:52`, used at `:119`, `:144`). Of one call the host knows only **the row it handed it to** (weather).
2. **A call inside one process does not go through rutis.** What a row gets from `ctx.use` is the object itself when its provider is in the same process: Python's `Runtime.lookup` returns `provided.value` (`python/rutis/rutis/runner.py:412`), Node's leaf adapter returns the Cordis service object (`node/rutis-runtime/src/leaf.mjs:98`), Go hands the object itself over when the target can hold it (`go/rutis/internal/runtime/runtime.go:1287`). weather calling llm is a plain method call, so llm's hop has no `site` and never reaches `rethrown`.
3. **An existing `site` is not overwritten** (`crates/rutis-bridge/src/session/mod.rs:253`). That rule is right (the first hop to fail matters most), but nothing fills that first hop today, so only the weather hop the host filled is left.

### 1.2 A name two rows provide: nobody says who holds it

> A Python row provides a name a Rust row provides, and the error says `weather is already provided by the host` (it is a row); two Rust rows with one name say `service "weather::Weather in scope <default>" already registered in scope`, without saying who holds it; when the other provider has only other methods, `rutis-host call weather.today` says `weather has no method today (methods: ask)`, pointing at the method name.

Three sources:

- **A runtime row**: when the host answers that the key is taken, it names the holder by `HostDispatch::provider()`, and only `row …` and `peer …` count; anything else becomes `the host` (`crates/rutis-bridge/src/runtime/rows.rs:296`). Half of the repository already gets this right: when one of the host's own services is taken, the holder's row is found through `diagnostics().bindings` (`crates/rutis-bridge/src/session/plugin/services.rs:216`). A `#[rutis::service]` type's `provider()` gives its type name (`crates/rutis-bridge/src/marked.rs:110`), so a name a row provides is reported as one the host provides. The row is known, though: `crates/rutis-bridge/src/services.rs:154` already computes `plugin.row`.
- **Two Rust rows**: the kernel's same-name error carries only the key (`crates/rutis/src/registry.rs:134`, `crates/rutis/src/ctx.rs:694`), printed as `service {0:?} already registered in scope` (`crates/rutis/src/error.rs:144`), which holds `TypeKey::describe()` and the scope's name, and no one.
- **The name is someone else's and the method is not there**: `no-method` says only that this method is missing (`crates/rutis-bridge/src/runtime/rows.rs:149`), not whose the **name** is now. The developer reads it as a misspelt method.

### 1.3 A misspelt `ctx.use("lmm")` tells you to add the typo

> When only `ctx.use("lmm")` is misspelt (the `inject` is right), the message says `add 'lmm' to its inject`, telling the developer to add the typo, with no did you mean.

One sentence in six languages (`python/rutis/rutis/runner.py:98`, `node/rutis-runtime/src/errors.mjs:220`, `bun/rutis-bun/src/errors.ts:220`, `go/rutis/internal/runtime/runtime.go:988` with `Inject`, `swift/rutis/Sources/Rutis/API/Runtime.swift:1075`, `kotlin/rutis/rutis/src/shared/kotlin/arcships/rutis/Refusals.kt:144`). `check` has long said did you mean (`crates/rutis-host/src/main.rs:976`); a runtime does not.

### 1.4 `check`'s warning lists unrelated rows

> `check`'s warning `nothing declares lmm; it may be provided at runtime by llm` lists every row that may provide services at run time, unrelated to this name.

`undeclared_line` (`crates/rutis-host/src/main.rs:953`) is given `Supply::Announced(rows)`, which is **every row that does not declare `provides`** (`:831`), not the rows that may provide this name. When a close name exists it still lists them first and appends the did you mean after (`:907`).

### 1.5 Two Rust rows that inject each other

> Two Rust rows inject each other: both say `waiting for … (no row provides it)`, without "they inject each other", and that sentence is wrong (the other row does provide it). Rust builtin rows do not declare what they provide.

The wait reason looks for a provider among **declarations** (`crates/rutis-loader/src/loader/waits.rs:344`: `fact.provides`). `fact.provides` comes from the row's `provides` declaration: a runtime row's from its plugin manifest, a builtin row's from nowhere (`crates/rutis-loader/src/resolver.rs:310` has `declares: None`). So two rows waiting for each other never reach `Cycle` (`waits.rs:365`) and land in `NoProvider`, which says `no row provides it` — while the other row does provide it, it only does not declare it. `Builtins::declare` (`crates/rutis-loader/src/resolver.rs:319`) has always let a builtin row declare; nobody calls it.

### 1.6 After a runtime exits, `status` takes about half a second to say so

> With a runtime down, `diagnostics` writes `pyhello: waiting for py (service missing)`; reading `status` right after the runtime exits still says `running`, and about 0.5 s later it is right.

- **Words**: the kernel's `RuntimeDiagnostics` `Display` prints dependency states as they are (`crates/rutis/src/diagnostics.rs:22`, `:80`, `:198`, `:200`): `(service missing)` and `provider inactive (Pending)` are internal ways of speaking, while `rutis-host`'s `status` has long said `waiting for runtime py (down: exited with status 1)` (`crates/rutis-loader/src/loader/status.rs:432`). One host, two vocabularies.
- **Lag**: `status`'s runtimes come from `RuntimeHandle::state()` alone (`crates/rutis-loader/src/loader/status.rs:278`), and that watch changes only when the pipe notices the exit (`crates/rutis-bridge/src/runtime/state.rs:235`). What decides "it has ended" already exists in `ended_status` (`state.rs:180`); `status` just does not use it to answer now.

### 1.7 Waiting for a runtime is described as waiting for a service

> The line for waiting on a runtime treats the runtime as a service: `hang: still waiting after 10s for hang: it starts once that is provided`.

`StillWaiting` is given only key names (`crates/rutis/src/error.rs:214`), and the names come from the describer the host installs (`crates/rutis/src/host.rs:116`, `name_keys_with`). A runtime row's session key is named after the runtime row's id, so it is described like any service, and the tail adds "it starts once that is provided". The same key is described correctly in `status`; here it cannot be.

### 1.8 Calls in flight do not show calls into Rust rows

> `status`'s `calls` does not show calls inside one runtime (the docs say so), nor calls into Rust rows in the host's process (the docs do not say so): with host → Python → Rust weather → Rust llm (stuck) only `host → pyhello.forecast 4s` appears.

Calls in flight are recorded per runtime process (`crates/rutis-bridge/src/runtime/process.rs:1414`), and only when the host hands a call to a **runtime row** (`crates/rutis-bridge/src/runtime/rows.rs:87`). A call handed to one of the host's own services (a `#[rutis::service]` row, a service the host registered itself) is routed in `crates/rutis-bridge/src/session/services.rs`, where nothing is recorded. A call between two rows of one runtime does not go through the host at all (§1.1, point 2).

### 1.9 Frames from inside appear in the stacks developers read

> A Python error's stack starts with 5–15 frames from inside rutis (`_unhandled.py`, `_output.py`, `runner.py`, `peer.py`), the plugin's own line last; a Go panic is about 40 lines, mostly the SDK and reflect; the cross-language error a Rust host gets has a `Debug` of thousands of characters, mostly those frames.

- Python: the error leaves the process as a whole `traceback.format_exception` (`python/rutis/rutis/peer.py:292`), while the filter for "the plugin's own frames only" exists (`python/rutis/rutis/runner.py:1341`, `_plugin_frame`, which `_unhandled.py:109`'s `report_text` already uses).
- Go: a panic carries the whole `debug.Stack()` (`go/rutis/internal/peer/peer.go:1712`).
- Rust: `session::Error`'s `Debug` is derived (`crates/rutis-bridge/src/session/mod.rs:35`), so `Remote` prints the whole error graph's JSON.
- Node and Bun already filter (`node/rutis-runtime/src/errors.mjs:22`); this item only brings the other three in line with them.

### 1.10 Stop timeouts and state words

> The output of a stop timeout includes `still stopping: rutis-loader`; the state words `unloading` and `inactive` are in no document.

`still_running` lists plugins that are not rows too, by their name inside rutis (`crates/rutis-host/src/host.rs:853`), which is how `rutis-loader` appears. State words: `unloading` is the catch-all print of `FiberState` (`crates/rutis-host/src/status.rs:41`), `inactive` is a literal translation of `EntryStatus::Inactive` (`crates/rutis-host/src/status.rs:21`); neither is in the guides.

### 1.11 A listener still running at shutdown loses its own error

> A Python listener still running at shutdown fails, and what is reported is `the connection closed`: the original error is gone.

After the session closes, a reply still awaited gets `Error::Transport("the connection closed")` (`crates/rutis-bridge/src/session/rpc.rs:941`; on the event side `crates/rutis-bridge/src/events/hub.rs:1189`). The listener's real error is on the far side, where nobody is left to take it.

## 2. Decisions

| # | Decision | Why; what was dropped |
| --- | --- | --- |
| D1 | When an error crosses rows inside one runtime, the **runtime** fills in the first hop to fail from its stack: `site` names it (the row, and `service.method` when it can say), every hop outward becomes one `rethrown` item, and `stack` keeps only the frames of the first row | The first hop to fail is the row the developer is looking for (the diagnostics design §4.1 rule); nothing fills it today. The stack is already in the runtime's hands, and so is a file → row mapping (Python's `charged_row`, `runner.py:649`). Dropped: "make `ctx.use` return a proxy" — that turns the semantics of one process into those across processes, and the plugin loses the object itself (data fields, `===`, type assertions), the opposite of [object references](design-object-refs-2026-10-27.md) §4 |
| D2 | The service and method are named only on **the hop rutis handed over**; a hop read from a stack names the row alone | On a hop rutis hands over it holds `service.method` (Go's `*Service` and struct-of-function-fields targets, `runtime.go:1216`; calls forwarded by the host, `rows.rs:52`). Guessing a method from a stack is a guess, and a wrong name is worse than none. With the row alone, the headline is the error's own text and `in <row>:` is followed by that row's frames, which still give the position |
| D3 | A name two providers share says one sentence: `<name> is already provided by <holder>`, the holder being a row's name or `the host` (when the host provides it itself), then how to fix it (`rename one of them` / `rename it`). Runtime rows, two Rust rows and the test tools all use it | A service name has one provider, so the later one fails, and it needs "who" and "what now". Three parties writing three sentences means the developer has to learn three. Dropped: "let the host add a clause" — a clause the host adds never reaches the row's own state line |
| D4 | `not-declared` says `did you mean llm?` when a close name exists, and only says to add it to `inject` when none does | Telling someone to add their typo is wrong advice. #676 meant "say what to declare", and a close name is the thing to say. The candidates are the names the row already has (in this process, leased in, declared by the plugin and by the row), by the rule of `rutis::config::closest` (`crates/rutis/src/config/mod.rs:267`: one edit for 3–6 characters, two beyond that) |
| D5 | The same warning from `check` no longer lists rows: `nothing declares lmm; did you mean llm?`, and without a close name "a row may provide it once it runs" | `check` runs no plugin, so it does not know who will provide; the rows it lists have nothing to do with the name. The Cordis-rows clause stays (that one is true) |
| D6 | A builtin Rust row declares what it provides (`Builtins::declare`), so a row's provider table is as complete as a runtime row's; a name nobody declares still says `nothing provides it yet`, not `no row provides it` | "No row provides it" is a claim about **providing**, while at that moment rutis knows only about **declarations**. With the declaration, two builtin rows waiting for each other reach `Cycle`, and `check` sees them too. Dropped: "infer the provider from a stack or from the runtime" — a wrong guess is just another wrong sentence |
| D7 | `diagnostics()`'s `Display` and `status` share one vocabulary for waiting: the host can install a **describer** on the root (`describe_keys_with`) that writes a key as `runtime py (down: exited with status 1)` or a service name with its reason; `StillWaiting` uses it too, and loses the tail `: it starts once that is provided` | One key is described two ways because one place has the name only and the other has the loader's `waits`. Letting the kernel ask the host (there is a precedent in `name_keys_with`) is simpler than teaching the kernel about runtimes. That tail is false while waiting for a runtime |
| D8 | A runtime's state in `status` follows the process: a `Ready` runtime whose connection has closed, or whose exit status is known, is shown down at once, by `ended_status` | A status snapshot is read "now" (O6). Waiting for the pipe's notice (about 0.5 s) makes the `status` right after it lie. Dropped: "make the notice faster" — that moves a reaping decision, and still leaves a window |
| D9 | Calls in flight: **everything that crosses the host** is recorded, including the call handed to one of the host's own services and to a `#[rutis::service]` row; a call between two rows of one runtime is still not recorded, and that goes into the guide and the `status` section | That call really does not go through rutis (§1.1, point 2); recording it means changing what `ctx.use` returns (dropped in D1). The half that crosses the host is what the issue names (`host → pyhello.forecast` is there, `pyhello → weather.forecast` is not), and it can say who is waiting for whom |
| D10 | The `stack` sent out holds only frames of the row that failed; when none is left, the innermost frame alone. `session::Error` stops deriving `Debug`: `Remote` prints the name, the message, `site`, the number of frames and the code | Frames are for the developer to find their own code; the SDK's, `reflect`'s and `runtime`'s frames are only length (D5). The error graph stays for the reading interface (`thrown()`) |
| D11 | A stop timeout names only what the developer wrote: a row by its id, a runtime as `runtime <name>`, and `the host` once when only rutis's own plugins are stuck; `unloading` becomes `stopping`, `inactive` becomes `not running (its group is not running)`, and both words go into the guide | `rutis-loader` is an internal plugin name (diagnostics design §4.5's forbidden words). State words are read every day: either a guide explains them or they become plain |
| D12 | A listener still running at shutdown that fails is written by the runtime under the row's name (`runtime py: row weather: listener of weather/asked failed: …`); the host no longer puts `the connection closed` in its place | Only the far side has the original error, so only the far side can say it; once the session is closed it writes to its own standard error (which `run` still forwards) |
| D13 | One new error code, `already-provided` (`data {service, provider}`); `no-method` gains `provider` in its `data` (the row's name when a row provides, `null` when the host does). No other text changes its code | Hosts branch on codes, not text (diagnostics design §4.3). A name clash is a configuration mistake the developer has to act on, and it deserves a code |

## 3. Rules

**The body does not change; only a prefix is added.** The body of a message is the underlying error's own words, kept verbatim (serde's ``invalid type: integer `42`, expected a string``, Python's `KeyError: 'town'`, Go's `json: unsupported type: chan int`); rutis adds only a short prefix that answers "which of my calls caused this" (`call to weather.tomorrow failed: `, `in llm: `, `called from: `), never paraphrasing, summarising, or repeating what the underlying error already said. Only errors rutis made up itself (`SyncWaitCycle …`, `row <x> is not loaded in this runtime`, `service missing` — rutis's own ways of speaking) are rewritten into plain words, because they are not the underlying error. When in doubt, keep the original.

The sections below obey this: the `site` and `rethrown` of §3.1 add only which row and which call, and §3.5 trims frames; neither touches the body. The sentences of §3.2 and §3.3 are rutis's own (a shared name, a missing method, what a row waits for), so they may speak plainly under this rule.

### 3.1 Whose error it is, inside one runtime

1. **When to label**: once per error as it leaves a row — the reply of a `dispatch`, the reply of a listener, an `apply` failure, a cleanup failure, an unhandled error. An error the plugin catches and handles is not labelled.
2. **How to label** (the same in every runtime):
   1. Walk the stack from the innermost frame outward; the first frame that belongs to **another row** names the row that failed first (`R`). The mapping already exists: a row's entry module file → the row's id (Python's `charged_row` in `runner.py:649` does exactly this; Node, Bun and Go have the entry they were given at `rows.load`).
   2. Found: the error's root node gets `site = {row: R, doing: "call"}`; the hop rutis handed over also gets `service` and `method` (D2).
   3. `stack` keeps only `R`'s frames (from the innermost up to `R`'s first frame); each stretch outward, cut where the row changes, becomes one `rethrown` item (at most 8, `crates/rutis-bridge/src/exports.rs:79`).
   4. Not found (the error is this row's own): nothing changes.
3. **No overwriting**: the host does not overwrite a `site` that is there (`crates/rutis-bridge/src/session/mod.rs:253`), so the hop the runtime labelled stays.
4. **Only printing and the host see it**: `site`, `rethrown` and the split `stack` are not handed to plugin code (as in diagnostics design §4.1). How a runtime keeps them inside its process does not enter any language SDK's public types.
5. **Differences between languages (written in the guides)**:
   - Go with a `*Service` or a struct-of-function-fields target: rutis hands the call over itself, so `service.method` is always known; with the service bound to an interface (`runtime.go:1287` hands the object itself over) there is no stack to read, so that hop is not attributed to its provider, as today.
   - Python, Node and Bun object calls have no service and method names (D2).
   - Between Rust rows everything is already in the kernel, the host fills `site`, and nothing changes.

### 3.2 A name two providers share

1. One sentence, the same in the six languages and in Rust: `<name> is already provided by <holder>`.
   - The holder is a row: the row's id (`a`).
   - The holder is the host itself: `the host`.
   - Then how to fix it: `rename one of them` when the holder is a row, `rename it` when it is the host.
2. It is printed on **the row that provided it later**, not in words of the terminal's own: `b: weather is already provided by a: rename one of them`.
3. A `#[rutis::service]` type's `provider()` returns `row <row>` when a row provides it, and no longer the type name (`crates/rutis-bridge/src/marked.rs:110`). A service the host registered itself still has no row and is still `the host`.
4. The kernel's same-name error carries the holder: `ServiceExists` holds the whole sentence (`<name> is already provided by <holder>: rename …`), and its `Display` no longer wraps it in `service "…" already registered in scope` (`crates/rutis/src/error.rs:144`). The name is the one the catalog gives (the one the plugin author wrote); without one, `TypeKey::describe()` (a Rust row's own type, which its author wrote too).
5. `check`'s same-name error: `rows a and b provide weather in the same scope: only the first to start provides it, the others fail; rename one of them, or isolate the name in each row` (rewritten in `crates/rutis-host/src/main.rs:1001`, opening like the runtime's sentence).
6. The name is someone else's and the method is missing: the `no-method` sentence is followed by `; it is provided by row <row>` when the provider is not the calling row. It is added for a caller with no row too (`rutis-host call`, Rust embedding).
7. In the test tools, "the plugin provides a name the test gives" still says `the host`: there really is no row there (the output in `docs/design-host-contract-types-2026-10-27.md` section 6 is unchanged).

### 3.3 Why a row waits

1. About a **service**: `waiting for <name> (<reason>)`, with these reasons:
   - Nothing declares it and nothing provides it: `nothing provides it yet`, with a close name `nothing provides it yet; did you mean llm?`.
   - The provider row waits for a service this row provides: `row b waits for a too: they inject each other`.
   - The provider row failed, is disabled, is not running, is starting, or waits for its own runtime: the existing words (`crates/rutis-loader/src/loader/status.rs:446` onward), with no new ones.
2. About a **runtime**: `waiting for runtime py (down: exited with status 1)`, `(starting)`, `(stopped)`, `(running)`, as `own_runtime_line` has it (`status.rs:432`).
3. A builtin row's `provides`: `rutis-host` calls `Builtins::declare` for each of its own rows; a row that does not declare still names nothing, and so falls into the first case above.
4. `StillWaiting` (the 10 s notice) uses the same words: `hang: still waiting after 10s for runtime py (down: exited with status 1)`, `chat: still waiting after 10s for llm (nothing provides it yet)`. The tail `: it starts once that is provided` goes: a runtime does not "start once it is provided", that only holds for a service, and a service's reason is already in the parentheses.
5. `check` keeps its own words: `check` speaks of declarations (`nothing declares …`), `status` of this moment (`nothing provides it yet`). Neither word can stand in for the other.

### 3.4 Calls in flight

1. Two places record: the host handing a call to a runtime row (already, `rows.rs:87`), and the host handing a call to one of its own services (new, in the dispatch of `crates/rutis-bridge/src/session/services.rs`).
2. A call records: the caller (`caller_row()`, `services.rs:242`; `host` when there is no row), the called row (the provider's row from `diagnostics().bindings`, `services.rs:154`), the service, the method and when it started.
3. `status`'s `calls:` merges both, longest first: `pyhello → weather.forecast (row weather)  4s`.
4. A call between two rows of one runtime is not recorded (D9). The `status` section of the guide says that `calls:` lists calls that cross the host.

### 3.5 Frames and state words

1. The `stack` sent out keeps only the frames of the row that failed (the same thing as Python's `_plugin_frame`, Go's module filter and Node's `pluginStack`); when none is left, the innermost frame alone (it is then the only thing that says where the error came from).
2. Frames inside `rethrown` are filtered the same way (an older error from a far end arrives unfiltered: printing cuts it at `MAX_FRAMES`, diagnostics design §4.6).
3. Rust's `session::Error`: a hand-written `Debug`, with `Remote` printed as `Remote { name, message, code, site, frames }`, `frames` being a count and `graph` not printed. The structure is read through `thrown()`.
4. State words: `unloading` → `stopping`; `inactive` → `not running (its group is not running)`. `running`, `waiting`, `starting`, `failed`, `disabled`, `stopped in its instance` and `stopped` stay, and all of them go into the `status` section of the [rutis-host guide](guide/rutis-host.md).
5. Stop timeouts: `still stopping: weather`; when rutis's own plugins are the stuck ones, `still stopping: the host`; a runtime process still there, `still stopping: runtime py`.
6. Where only one line fits (a status line, a line of `check`, the parentheses of `still waiting`), it is the error's own first line (`KeyError: 'town'`), never a sentence of rutis's summarising it; where the whole error is printed (`report`), a multi-line error stays multi-line.

### 3.6 Error codes

The table in diagnostics design §4.3 is changed by two designs at once: **this one changes only the two rows about service names** (a new `already-provided` row, and `no-method`'s `data` gaining `provider`), while #936 changes the two rows about values (`bad-value`'s `data` gaining `reason`, and a new `bad-result`). Both end up in the same table: whichever lands second rebases onto the first. This design does not copy the whole table.

After this design, the two rows read as follows (the other columns and rows are unchanged: see diagnostics design §4.3):

| code | `data` |
| --- | --- |
| `no-method` | `{service, method, methods, provider}`: `provider` is the provider's row, `null` when the host provides it; an object call keeps `{object, method, methods}` (an object is no row's service) |
| `already-provided` (new) | `{service, provider}` (as above) |

Codes are for programs; they do not appear in the line people read (diagnostics design §4.3).

## 4. What developers see

### 4.1 Plugin authors

Nothing to write when all goes well: errors are thrown as before, `ctx.use` gives the service as before. What changes is what is printed when something fails.

The examples below have Python and TypeScript rows; the other languages say the same things with their own frame style (`at f (x.ts:1:2)` / `File "x.py", line 1, in f` / `    at T.M (x.go:1)`).

**Another row throws, this row called it, and both are in one runtime** (in the example llm and weather are in one Python runtime, chat is elsewhere; D1):

```text
chat: KeyError: 'region'
    in llm:
    File "llm.py", line 31, in ask
    called from:
    File "weather.py", line 12, in today
    called from:
    at reply (src/chat.ts:14:36)
```

The first line is the error's own text (this hop knows the row's name but not the service's and method's, D2); below it, every hop, the earliest first. What follows `chat: ` is that text itself (across processes it opens with `call to … failed: `): rutis changes not a word and adds no explanation.

**In one runtime, Go calling through a `*Service` or a struct of function fields** (D2 names the service):

```text
chat: call to llm.ask failed: no API key
    in llm:
        at Llm.Ask (llm.go:31)
    called from:
        at Weather.Today (weather.go:12)
```

**Every failure**:

| # | How to cause it | Output (word for word) | Code | Recorded on | How to fix it |
| --- | --- | --- | --- | --- | --- |
| 1 | Another row in one runtime throws (Python, Node, Bun, a Go interface target) | `chat: KeyError: 'region'` followed by `in llm:`, llm's frames, `called from:`, weather's frames | as before | the calling row (when it does not catch it) | look at the frame under `in llm:` |
| 2 | Another row in one runtime throws, Go through a `*Service` | `chat: call to llm.ask failed: …` and the same below | as before | same | same |
| 3 | A row already provides the name | `b: weather is already provided by a: rename one of them` | `already-provided`, `data {service: "weather", provider: "a"}` | the row that provided it later | rename one of them |
| 4 | The host itself provides the name | `b: weather is already provided by the host: rename it` | `already-provided`, `provider: null` | same | rename this row's service |
| 5 | Two Rust rows (`#[rutis::service]`) provide one name | `b: weather is already provided by a: rename one of them` | as above | the row that provided it later | rename one of them |
| 6 | The name is someone else's and the method is not there, `rutis-host call weather.today` | `weather has no method today (methods: ask); it is provided by row a` | `no-method`, `data {service, method, methods, provider: "a"}` | on this call | see whose `weather` is now: rename it, or find your row |
| 7 | `ctx.use("lmm")` while `inject` says `llm` | `service lmm is not available to this plugin, which does not declare it in inject: did you mean llm?` (Go: `… in Inject: did you mean llm?`) | `not-declared`, `data {service: "lmm"}` | this call | write `llm` |
| 8 | As above, with no close name anywhere (`ctx.use("zzz")`) | `service zzz is not available to this plugin, which does not declare it in inject: add 'zzz' to its inject` (unchanged) | `not-declared` | same | declare it, or wait for the row that provides it |
| 9 | `ctx.get("lmm")` (optional) | `service lmm is not available to this plugin, which does not declare it: did you mean llm?` | `not-declared` | same | write `llm` |
| 10 | Two Rust rows inject each other | the row says `a: waiting for b (row b waits for a too: they inject each other)` | none (a state) | `status`, the state lines of `dev` | break one of the two `inject`s |
| 11 | Waiting for a name nobody declares and nobody provides | `weather: waiting for llm (nothing provides it yet)` | none | same | wait for a provider, or change the name |
| 12 | A misspelt name nobody declares | `weather: waiting for lmm (nothing provides it yet; did you mean llm?)` | none | same | write `llm` |
| 13 | Waiting for its own runtime for 10 s | `hang: still waiting after 10s for runtime py (down: exited with status 1)` | none | standard error | read why the runtime ended |
| 14 | A listener still running at shutdown fails | `runtime py: row weather: listener of weather/asked failed: KeyError: 'region'` | none | the runtime's standard error (forwarded under `run`) | look at the row |

Row 1's first line is the error's own text; across processes that part is still `call to weather.tomorrow failed: …` (row 2 of diagnostics design §8.1 is unchanged).

Only three kinds of words in this section are rutis's own; everything else is carried verbatim from the underlying error: **prefixes** (`chat: `, `call to weather.tomorrow failed: `, `in llm: `, `called from: `, `listener of weather/asked failed: `, `runtime py: row weather: `), **errors rutis made up** (rows 3 to 13: a shared name, a missing method, nothing declared, what a row waits for, a runtime that is not there), and **short phrases taken from a fact** (`down: exited with status 1` in row 13 is the process's own exit, not a summary). An underlying error of several lines stays several lines: not flattened, not reworded, no sentence of explanation added.

### 4.2 People assembling plugins with `rutis-host`

The configuration does not change. What changes is `check`, `status`, the output at a stop, and a row's state when a name is taken twice.

```json
{ "rows": [ { "id": "a", "name": "weather" }, { "id": "b", "name": "weather" } ] }
```

`rutis-host check`:

```text
rutis-host: error: rows a and b provide weather in the same scope: only the first to start provides it, the others fail; rename one of them, or isolate the name in each row
```

`rutis-host check`, a name nobody declares:

```text
rutis-host: warning: weather: nothing declares lmm; did you mean llm?
rutis-host: warning: weather: nothing declares zzz; a row that does not declare its services may provide it once it runs
```

(The second gains `; between Cordis rows of one runtime it resolves in Cordis` only when one row is a Cordis row and so is another.)

`rutis-host status`:

```text
rows:
  weather: running
  pyhello: waiting for runtime py (down: exited with status 1)
services:
  weather: a
  llm: llm
runtimes:
  py: down (exited with status 1)
calls:
  host → pyhello.forecast  4s
```

`rutis-host call`, the name being someone else's:

```text
weather has no method today (methods: ask); it is provided by row a
```

A stop timeout:

```text
rutis-host: 10s passed: cleanups did not finish
  still stopping: weather
  still stopping: the host
```

State words (`run`, `dev` and `status` all use these):

| Output | When | Before |
| --- | --- | --- |
| `running` | running | unchanged |
| `waiting for …` | waiting for a dependency or a runtime | unchanged |
| `starting` | loading | unchanged |
| `stopping` | unloading | `unloading` |
| `not running (its group is not running)` | the group around it is not running | `inactive` |
| `failed: …` | failed, with the reason | unchanged |
| `disabled` | disabled in the configuration | unchanged |
| `stopped` / `stopped in its instance` | stopped | unchanged |

### 4.3 Host owners (an application embedding rutis)

```rust
let diagnostics = rutis::host::CtxHost::diagnostics(&root);
eprintln!("{diagnostics}");
```

```text
plugins:
  weather: running
  pyhello: waiting for runtime py (down: exited with status 1)
services:
  weather: provided by a
  llm: provided by llm
```

The structure of an error and the way it is read do not change (`rutis_bridge::thrown`):

| Case | What is seen |
| --- | --- |
| Printing a cross-language error | `rutis_bridge::report::report(&error)`: `call to llm.ask failed: …`, or the error's own first line, then `in llm:`, the frames, `called from:` |
| `Debug` of a cross-language error | `Remote { name: "KeyError", message: "'region'", code: None, site: Some(Site { row: "llm", .. }), frames: 3 }`, with no error graph JSON |
| A name clash | `thrown(&error)` gives none (it is the kernel's error); `ErrorKind::ServiceExists`'s `to_string()` is `weather is already provided by a: rename one of them`; `diagnostics()`'s `services` still names the earlier provider |
| Calls in flight | `Loader::status().calls` holds the call handed to one of the host's own services: `pyhello → weather.forecast (row weather)  4s` |

### 4.4 Concepts to know

None added. A row's name, a service's name, `inject`, `status` and `check` are what each role already knows ([#663](https://github.com/arcships/rutis/issues/663)'s budgets: plugin authors 7, `rutis-host` users 6, host owners 10). What this design does is use the existing concepts correctly: the row it names becomes the right one, "no row provides it" becomes "nothing provides it yet", and internal names such as `rutis-loader` go away.

Host owners gain one thing to know: `describe_keys_with` (D7). It is optional; without it the diagnostics output still reads, only the runtime's line does not say `down`. It counts inside the "debugging" budget (at most 3 extra concepts per role), not as a layer of its own.

## 5. Compatibility and migration

To go into "Migrating to 0.21" and the release notes (both languages):

- **The words change, the structure does not**: the error types, fields, `site` and `rethrown` the diagnostics design lists are unchanged; what changes is what fills them and what is printed.
- **An error inside one runtime changes hands**: before, `in weather:`; now `in llm:`, with `called from:`. Anyone matching output by row name has to follow. Plugin code does not see any of this.
- **The words for a shared name**: `service "weather::Weather in scope <default>" already registered in scope` → `weather is already provided by a: rename one of them`; `weather is already provided by the host` becomes the row's name when a row provides it. Code matching on text moves to the `already-provided` code (D13).
- **The words of `no-method`**: `; it is provided by row <row>` is appended when the provider is a row and not the calling row. `data.provider` is a new key; old readers are unaffected.
- **The words of `not-declared`**: with a close name it no longer says `add '…'` but `did you mean …?`. `data` and the code are unchanged.
- **`check`'s warning**: it no longer lists rows; `--strict` behaves as before (still an error, still exit status 1).
- **Status and notice words**: `no row provides it` → `nothing provides it yet` (only when nothing provides it); `still waiting … for hang: it starts once that is provided` → `… for runtime py (down: …)`; `still stopping: rutis-loader` → `still stopping: the host`; `unloading` → `stopping`, `inactive` → `not running (its group is not running)`. Scripts matching those words have to follow; the shape of `status --json` is unchanged (the state words follow).
- **The body of an underlying error never changes**: this design adds a prefix and which row and call (`§3`, first rule); the body is carried verbatim. The one place that used to lose it was a listener still running at shutdown, reported as `the connection closed` while the error in hand was dropped; it now shows the listener's own error (D12).
- **Rust**: `session::Error`'s `Debug` no longer prints the error graph (read the structure through `thrown()`); the text of `ErrorKind::ServiceExists`'s `Display` changes (the variant and its field do not).
- **Builtin rows should declare `provides`**: without it, `check` and `status` still say "nothing declares it" / "nothing provides it yet" about the names they provide. That is a relaxation (a wrong sentence stops), not a tightening.
- **No version changes**:
  - the session protocol is still 4 (`crates/rutis-bridge/src/session/mod.rs:14`, `ENDPOINT_PROTOCOL`): no new frame, no new capability name;
  - the `rutis.json` format is still 1 (`crates/rutis-host/src/config.rs:78`, `FORMAT`): no new configuration field;
  - the plugin API is still 3 (`node/rutis-runtime/src/leaf.mjs:16`, `bun/rutis-bun/src/plugin.ts:21`, `python/rutis/rutis/plugin.py:125`, `go/rutis/internal/runtime/version.go:9`, `swift/rutis/Sources/Rutis/Version.swift:28`, `kotlin/rutis/rutis/src/shared/kotlin/arcships/rutis/advanced/Versions.kt:7`): plugins need no release.
- **The error graph gains no field**: `site` and `rethrown` have been optional fields of an error node since 0.18 (read at `crates/rutis-bridge/src/session/mod.rs:289` and `:228`). A `throw` frame carries `Failure { name, message, graph }`, and `graph` is a `serde_json::Value` (`crates/rutis-bridge/src/session/protocol.rs:88`): `deny_unknown_fields` covers `Failure`'s own three keys only, and never looks at or refuses what is inside the graph. So this design makes the runtimes start filling those two fields; it does not change the protocol.
- **Old runtime with a new host**: the old runtime sends no `site` and splits no stack, so the host prints what it prints today — the hop that crossed still reads `call to weather.tomorrow failed: …` (the host filled that `site`), only without `in llm:` and `called from:`.
- **New runtime with an old host**: the old host decodes the `throw` frame into `Failure` and keeps the graph as it is (`graph: Option<Value>`): it neither knows it nor drops it. It has no `report::whereabouts`, so it prints `name: message` and `site`/`rethrown` do not appear. When an old end throws that error on again, the graph travels as received (Node splices it, `node/rutis-runtime/src/errors.mjs:63`; Python's `python/rutis/rutis/peer.py:370`; Rust's `crates/rutis-bridge/src/exports.rs:104`), without appending a segment of its own (which is what diagnostics design §4.4 says).
- **A call between two rows of one runtime still does not appear in `calls:`**: this goes into the guide (until now it was only in a design document).

## 6. Implementation

The host and the kernel first (codes, shared names, wait reasons, calls in flight), then attribution and frames per language, then the documents. The word-for-word cases go into each language's existing text tests and into `rutis-host`'s integration tests — the files named in each row below; no separate per-language case table is added. The test-tool column builds no shared expectation file either: #933 locks the line itself in `rutis-host`'s integration tests (`crates/rutis-host/tests/test_command.rs`, one case running `dev` and the host's line), and each tool has one test that prints that line verbatim (those that exist today: `node/rutis/test/testing-provide.test.mjs` and `testing-taken.test.mjs`, `bun/rutis-bun/test/sdk-taken.test.ts`, `python/rutis/tests/test_testing.py`; Go's, in `go/rutis/rutistest`'s tests, is added by #933's T-4).

| # | What (and where) | Done when |
| --- | --- | --- |
| E-1 kernel and host | `ServiceExists` carries the whole sentence (`crates/rutis/src/registry.rs`, `ctx.rs`, `crates/rutis/src/error.rs:144`); its code and `data`; `no-method`'s `provider` (`crates/rutis-bridge/src/runtime/rows.rs:149`); `provider()` gives the row (`marked.rs:110`, through `crates/rutis-bridge/src/services.rs:154`); the `already-provided` code added to diagnostics design §4.3 | one word-for-word test per source (runtime row, `#[rutis::service]` row, the host itself), in the unit tests of `crates/rutis-bridge/src/report.rs` and in `crates/rutis-host/tests/clash.rs`; that `site`/`rethrown` survive one runtime round trip, added to `crates/rutis-bridge/tests/error_shape.rs` |
| E-2 waiting and diagnostics | builtin rows `Builtins::declare` (`rutis-host` calls it for its own rows); `describe_keys_with` (`crates/rutis/src/host.rs`); `RuntimeDiagnostics`'s `Display` uses the describer; `StillWaiting` loses its tail; `status`'s runtime state follows the process (`crates/rutis-loader/src/loader/status.rs:278`, through `crates/rutis-bridge/src/runtime/state.rs:180`); the `Cycle` wording (`status.rs:474`); `NoProvider` says `nothing provides it yet` (`status.rs:453`) | two builtin rows injecting each other say `they inject each other` in `status` and in `dev`; `check` reports a cycle for the same two rows; a runtime that has just exited is down in `status` at once (a test that does not sleep: it closes the process and reads the state); `diagnostics()` and `status` describe one key the same way (a test comparing them) |
| E-3 `check` and the command line | `undeclared_line` no longer lists rows (`crates/rutis-host/src/main.rs:953`); `clash_line` rewritten (`:1001`); `call.rs`'s `failed` no longer adds the provider itself (the host already wrote it into the error); `still_running` and `describe` (`crates/rutis-host/src/host.rs:828`, `crates/rutis-host/src/status.rs:18`) | the two lines in `rutis-host/tests/check.rs` become the new words; one for `call`; one for a stop timeout |
| E-4 calls in flight | record when the host hands a call to one of its own services (`crates/rutis-bridge/src/session/services.rs`); `Loader::status` merges both (`crates/rutis-loader/src/loader/status.rs:313`) | one test: host → runtime row → host service (stuck), both calls in `status`, `from` the row's name |
| E-5 Python | attribution and splitting the stack (`runner.py`'s `charged_row` reused, `peer.py`'s `encode_error`); frames filtered by `_plugin_frame` (`runner.py:1341`); `not-declared`'s did you mean (`runner.py:98`); a listener's error at shutdown (`runner.py`'s `charge`); `no-method`'s `provider` | `python/rutis/tests/test_plain_texts.py` (a shared name, attribution inside one runtime), `test_undeclared_use.py` (did you mean), `test_optional_deps.py` (`no-method`'s `provider`), `test_events_reports.py` (a listener still running at shutdown); "no frame from inside" is checked by `crates/rutis-loader/tests/fault_texts.rs` |
| E-6 Node, Bun | attribution and splitting the stack (`node/rutis-runtime/src/unhandled.mjs`, `errors.mjs`; `bun/rutis-bun/src/errors.ts`); `not-declared`'s did you mean; `no-method`'s `provider`; one wording for a shared name (`leaf.mjs:63`, `runner.ts:239`) | `node/rutis-runtime/test/texts.test.mjs` and `errors.test.mjs` (a shared name, did you mean, `no-method`'s `provider`), `runner-events.test.mjs` (a listener), `bun/rutis-bun/test/errors.test.ts`; forbidden words as above |
| E-7 Go, Swift, Kotlin | attribution: Go writes `site` in `localService` (`runtime.go:1201`) with the row, service and method, and an interface target is written down as a boundary; Swift and Kotlin read the stack (each keeps its rows' files); a panic's frames filtered (`peer/peer.go:1712`); `not-declared`'s did you mean (`runtime.go:988`, `Runtime.swift:1075`, `Refusals.kt:144`); one wording for a shared name (`runtime.go:435`, `:528`, `Runtime.kt:379-380`, `:497`, `:606`) | `go/rutis/internal/runtime/errors_text_internal_test.go` (words), `errors_location_test.go` and `local_calls_test.go` (attribution and position), `swift/rutis/Tests/SelfTest/ErrorTextTests.swift` and `ThrowSiteTests.swift`, `kotlin/rutis/rutis/src/jvmTest/kotlin/arcships/rutis/ErrorTextTest.kt`; for Go one more test where an interface target is not attributed (the output as today, and written down in the guide) |
| E-8 documents | the `status` section of [rutis-host](guide/rutis-host.md) (state words, what `calls:` lists), [plugin API](guide/plugin-api.md) (the words of a failure, attribution inside one runtime and its boundaries), the language guides (Go's interface-target boundary), "Migrating to 0.21", [guarantees](guarantees.md) (new entries `bridge/error-ownership`, `loader/wait-words`) | the link check passes, both languages written |

Done when (#937 closes when all hold):

- [ ] every word-for-word line of §4 is asserted in the files named in E-1 to E-7; the test-tool column follows #933: the line itself locked in `crates/rutis-host/tests/test_command.rs`, and one case printing it verbatim per tool, in `node/rutis/test/testing-provide.test.mjs` and `testing-taken.test.mjs`, `bun/rutis-bun/test/sdk-taken.test.ts`, `python/rutis/tests/test_testing.py` and `go/rutis/rutistest`'s tests;
- [ ] forbidden words are checked by `crates/rutis-loader/tests/fault_texts.rs` (the new lines are added to its list);
- [ ] the two codes of §3.6 are asserted by code in tests, not by text;
- [ ] the faults #937 names are re-run from the 0.19.0 evaluation, and the output matches §4;
- [ ] "Migrating to 0.21" and the release notes are written, both languages.

## 7. Follow-ups

- **A call between two rows of one runtime** is still absent from `calls:` (D9). Seeing it means `ctx.use` giving out a proxy instead of the object, which is a change to the semantics of object references ([object references](design-object-refs-2026-10-27.md) §4); a design of its own.
- **Go's interface targets** are not attributed to their provider (§3.1, point 5): a Go error has no stack, and the only fix is to make `ctx.Use` wrap interface targets too, which again changes what `ctx.use` hands over.
- **`check` cannot see a builtin row's `provides`** unless the host declares it (§3.3, point 3). `#[rutis::plugin]` and `#[rutis::service]` filling `provides` in by themselves is another matter — it answers "which names will this plugin provide" — and deserves its own design.
