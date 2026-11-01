# Migrating to 0.20

[中文](migration-0.20.md)

0.20 is not released yet; this guide is written together with the incompatible changes, one section per change, and completed before the release as [#694](https://github.com/arcships/rutis/issues/694) says. Only the changes since 0.19 are written here, and removed forms are removed outright, not deprecated.

The incompatible changes, by kind, each with the section saying how to change:

- **Unload order**: cleanups and service withdrawals share one stack and run in reverse, one at a time ([Unload order](#unload-order-1010)).
- **`use` checks declarations first**: a plugin that declared a name in neither its `inject` nor its row's `inject` and got it from another row in the same process now gets `not-declared` ([`use` checks declarations first](#use-checks-declarations-first-1000)).
- **A runtime row's row `inject`**: when a service in it is not there, the rows of five languages no longer start, and they stop when the provider goes ([A runtime row's row `inject`](#a-runtime-rows-row-inject-1001)).
- **The greeting limit of a runtime process**: `Process::mount` fails when no greeting comes in time, and `Mount` has one more public field ([The greeting limit of a runtime process](#the-greeting-limit-of-a-runtime-process-1014)).

Two changes in Go and Kotlin only loosen: what used to fail now works, and what used to work gives the same result; only the error texts they print changed ([Go: interface and `any` targets](#go-interface-and-any-targets-1015), [Kotlin: `use` gives the object of this process first](#kotlin-use-gives-the-object-of-this-process-first-1006)).

The rest is new, and changes nothing already written.

## Unload order (#1010)

Design: [Unload order: withdrawals and cleanups on one reverse stack](design-unload-order-2026-11-01.en.md). All six languages (Go, Python, Bun, Kotlin, Swift, Node) behave this way.

### What changed

A row's unload runs **one** stack: `provide` puts "withdraw this service" on it, `ctx.effect` and the cleanup `apply` returns put cleanups on it. Unloading runs it in reverse, one entry at a time, each withdrawal in the place it was registered.

| Case | 0.19 | 0.20 |
| --- | --- | --- |
| A cleanup registered **after** `provide` reads the service its row provided | Go, Python, Bun, Kotlin, Swift: `not-declared`; Node: it reads it | All six read it: the object itself |
| A cleanup registered **before** `provide` reads it | As above: the five say `not-declared`, Node reads it | None of the six reads it: `use` says `not-declared`, `get` returns the empty value |
| Another row reads the service while this row unloads | The five: the withdrawal deleted it before the cleanups, `unavailable`; Node: it still read it until this row's disposals finished | All six say `unavailable`, from the moment this row starts unloading |
| A cleanup registering another cleanup | Kotlin wrote a line and did not run it; Go, Python, Bun, Swift dropped it silently; Node handed it to Cordis's disposals | All six write `rutis: row <row> is not active: cleanup is not registered` (Node: `plugin <plugin>`), and that cleanup does not run |
| Several cleanups starting together (Node) | Node's `_unload` starts every disposal at once with `Promise.all` | All six run one at a time, latest first |

### What to change

- **Do not wait for another cleanup inside a cleanup.** With one entry at a time, a cleanup waiting for another one (a promise the later cleanup would resolve, say) waits for good: one `rows.unload` has no per-entry budget in any of the six, so the row never answers and the host waits too.
- **Do not register a cleanup from a cleanup.** Unloading runs the stack as it stood when it started, so a cleanup registered while unloading does not run (the fourth row above). Do the work in one cleanup.
- **Reading a service your row provides from a cleanup no longer fails** (when the cleanup is registered after `provide`): no code change is needed, but do not conclude from it that the withdrawal already ran — to read the service, register the cleanup after `provide`; one registered before still does not read it.
- **Do not rely on other rows reading it while the cleanups run**: Node used to allow that and now does not, as in the other five.
- **Unloading may get slower**: cleanups go from "all at once" (Node) or "one by one, withdrawal first" (the five) to "in reverse, one at a time", so a row with many slow cleanups unloads more slowly. The six do not agree on a cleanup deadline: see the table under "Host owners" in §3 of [Unload order](design-unload-order-2026-11-01.en.md) — Kotlin times each entry in `dispose`, Go and Swift give the whole unloading one budget when the session ends, and Python, Node and Bun give up only when the process exits.

### What it affects

- Plugin authors: a plugin working on 0.19 needs changes only if it relies on one of the differences in the table above: cleanups running in parallel, registering a cleanup from a cleanup (all six), and the two Node-only ones — a cleanup registered before `provide` reading its own service, and another row reading the service while this row's cleanups run (Node read it before and does not now, as in the other five). None of them was promised on 0.19 either; it just happened to hold then.
- Host owners: the frame order does not change — announced services are still withdrawn before `rows.unload` is answered, and the reply still comes after every cleanup has run; `--shutdown-timeout` means the same, and slow cleanups behave as the table above says.

## `use` checks declarations first (#1000)

Design: [A row's `inject`, and `use` checking declarations first](design-declared-services-2026-10-29.en.md). The same in all six languages.

### What changed

`use` and `get` both check the declarations first and the table second. Declared is the plugin's `inject`, the row's `inject`, and the names the plugin **provides now**; `get` also takes the plugin's `optional` ([Optional dependencies](design-optional-deps-2026-10-28.en.md) O4: `use` does not take `optional`).

| Case | 0.19 | 0.20 |
| --- | --- | --- |
| Go, Swift and Kotlin's `use` and `get`, for a name in the row's `inject` | `not-declared` (those three dropped the row's `inject` from `rows.load`) | What this process has is given: the object itself from a provider here, the proxy for one leased in; when there is none, `use` says `unavailable` and `get` returns the empty value |
| A plugin reading what it provides now | Bun and Python's `use` got it, Node's only after `apply`; `get` and Go, Swift and Kotlin's `use` said `not-declared` | The object itself from `use` and `get` while it provides it; `not-declared` after the withdrawal |
| Node, Bun and Python's `use`, with the plugin and the row declaring nothing, and **another row in the same process** declaring the name | Got it | `not-declared` |

### What to change

- **Write the services you use into the plugin's `inject`.** The third row is the only one that breaks a working plugin: a plugin that relied on "another row in the process happens to declare it" now gets

  `service llm is not available to this plugin, which does not declare it in inject: add 'llm' to its inject`

  (Go: `… which does not declare it in Inject: add "llm" to its Inject`.) If it cannot run without it, put it in `inject`; if it can, put it in `optional` and read it with `get` ([Optional services](guide/plugin-api.en.md#optional-services)).
- Code that reads `not-declared` as "the service is not there" now sees `unavailable` (the first row): the line differs when the service is there but not leased in.
- A test tool has no "row": what `load` may read is the plugin's `inject` and `optional`, with no row `inject` item, as before.

### What it affects

- Plugin authors: this is what [#676](https://github.com/arcships/rutis/issues/676) asks for, not a regression — whether a plugin got the service used to depend on what else was loaded, which is neither explainable nor testable. The first two rows only loosen, so a call that worked does not fail because of them.
- People assembling plugins with `rutis-host`: one `rutis.json` now declares the same set in all six languages. No new configuration and no new output.

## A runtime row's row `inject` (#1001)

Design: [A runtime row's row `inject`](design-row-inject-gate-2026-10-31.en.md).

### What changed

A runtime row (a Node, Bun, Python, Go, Swift or Kotlin row) is gated in rutis by the **names the service catalog shares** in the row's `inject`, and those names are leased into the runtime when it starts, through the same leasing as the plugin's `inject`. Before:

| Runtime | A service in the row's `inject` provided by the host or another process, not in this process yet |
| --- | --- |
| Node | Showed as running in rutis, and in fact sat in the Cordis gate inside the process: `apply` did not run, waiting for good |
| Bun, Python, Go, Swift, Kotlin | Did not wait: `apply` ran as usual and `use` said `service llm is not available` |

All six agree now: `chat: waiting for llm (no row provides it)` (with `; did you mean <name>?` when a close name exists; when a disabled row provides it, see the table of waiting reasons in [rutis-host](guide/rutis-host.en.md)), which is what `rutis-host check` always said. The row starts when the service arrives and the plugin gets it ([`use` checks declarations first](#use-checks-declarations-first-1000)); it stops when the provider goes and starts again when it comes back, and adding `inject` to a row restarts it once.

A **name the service catalog does not share** (one an application registers as a Rust type) is neither gated nor leased and is left to the runtime, as before: Bun, Python, Go, Swift and Kotlin rows start, and `use('clock')` says `service clock is not available`; a Node row shows as running while the Cordis gate inside the process waits for `clock`, so `apply` does not run and there is no output. Such a name is not a `dyn HostDispatch` and does not cross languages.

### What to change

- **Do not put a name the catalog does not share in a row's `inject`.** A service an application registers as a Rust type crosses no language: take it out of the runtime row's `inject` and use it in a Rust row.
- **Do not rely on "the row starts and `use` failing is caught".** The row no longer reaches `apply`. Have a row or the host provide the service, or take it out of the row's `inject`.
- **If you write your own resolver (`Resolved::with_foreign_scope`)**: its rows are gated by the shared names in the row's `inject` too (`rutis-host check` always reported it that way). Announce those names to rutis (as `rows.announce` does), or the row waits for good: `<row id>: waiting for <name> (no row provides it)`.

### What it affects

- Plugin authors: a plugin that needs the services in its row's `inject` now always runs after they are there; a name only in the row's `inject`, provided by the host or another process, is given too (the five languages did not get it before).
- People assembling plugins with `rutis-host`: waiting and restarting are now the same in all six; a Node row used to show as running while it waited and now shows as waiting. `rutis-host check` prints the same as before.

## Kotlin: `use` gives the object of this process first (#1006)

### What changed

The order `ctx.use(name, type)` takes:

| Step | 0.19 | 0.20 |
| --- | --- | --- |
| Declarations | Not in the plugin's `inject`: `not-declared` | The same, with the row's `inject` and what the plugin provides now in the set ([`use` checks declarations first](#use-checks-declarations-first-1000)) |
| Proxy table | Taken first, failing with an `IllegalArgumentException` when there is none: `<type> has no proxy: annotate it @RutisClient and apply the rutis-ksp processor` | Taken last, only when a proxy is really needed |
| This process's object | Looked at only after the proxy table | Looked at first: when `type.isInstance(object)`, the object itself |

### What to change

Nothing: it only loosens.

- An interface used only in this process needs no `@RutisClient`: a provider marked `@RutisService` and `ctx.use(name, Interface)` are enough.
- `provide` of an object not marked `@RutisService` still fails: `rutis: provide <name>: <class> is not a @RutisService class (no generated method table: apply the rutis-ksp processor)`. A service used only in this process still needs `@RutisService`.

### What it affects

Plugin authors: this process's object still wins; an interface across processes still needs a `@RutisClient` proxy, and those errors are unchanged.

## Go: interface and `any` targets (#1015)

Design: [Go: the provider's object itself in this process](design-go-local-objects-2026-11-01.en.md).

### What changed

The target of `ctx.Use(name, &target)` is now settled in this order:

1. `*advanced.Service` (unchanged);
2. the target can hold this process's provider object (Go's own assignment rule: an interface it implements, its own type, `any`) — the object itself is given, and calls are plain Go calls, with arguments and results not encoded;
3. a non-empty interface with the provider in another process: an error (below);
4. a struct of function fields: a field whose type is exactly the method a provider in this process declares takes that method as it is, with no `error` needed last; the rest follow the cross-process rules (an `error` last, arguments that can be encoded).

Step 2 did not exist, and step 4's "the types are the same" came after the cross-process signature check, so these forms failed in `bind`:

```text
use hello: the target must point to a struct of function fields or a *advanced.Service, not rutis.Greeter
use canvas: field Draw: func(string) string must return an error last
```

Now:

| Target | Provider in this process | Provider in another process |
| --- | --- | --- |
| Interface | The object itself; an error when the object does not implement it (below) | An error (below) |
| `any` | The object itself | `*advanced.Service` |
| Function fields | A field of the same type takes the method as it is; the rest follow the cross-process rules | As before |
| `*advanced.Service` | As before (`Local()` is `true`) | As before |

All of these are `error`s that `Use` and `Get` return, without an error code, and are not written to stderr:

| How it happens | Output (verbatim) |
| --- | --- |
| Interface target, provider in another process | `use workspace: editor.Workspace is an interface, which only a service provided in this process fills: for one of another process, use a struct of function fields or *advanced.Service` |
| Interface target, this process's object lacks a method | `use workspace: *editor.files does not implement editor.Workspace (missing method OnDraw)` |
| Interface target, the method is there with another signature | `use workspace: *editor.files does not implement editor.Workspace (wrong type for method Open)` |
| Interface target, `Provide` gave a value and the method is on the pointer | `use workspace: editor.files does not implement editor.Workspace (method OnDraw has pointer receiver)` |

### What to change

- **An interface target holds only in this process.** A plugin that `Use`s a service through an interface fails with the first line when that service moves to another process at deployment. Use a struct of function fields or `*advanced.Service` across processes.
- Code matching the old two lines changes: an interface and `any` no longer say `must point to a struct of function fields …`, and a function field of the same type no longer says `must return an error last`.
- The providing side is unchanged: `MethodsOf` still requires methods that cross processes (an `error` last, arguments that can be encoded); a service used only in this process is not written into `Provides`.

### What it affects

Plugin authors: with these three forms unavailable, objects had to be passed around rutis (through package-level variables); they now give the object itself, and so does `ctx.Get`. A form that worked gives the same result as before.

## The greeting limit of a runtime process (#1014)

### What changed

`Process::mount` used to wait for the runtime process's greeting with no limit (`peer.ready()`): a runtime that started and never greeted made it wait for ever. 0.19 gave `Greet::open`, the loader and rutis-host a greeting limit; this one was missed. Now:

- 30 s by default, the constant `rutis_bridge::runtime::GREETING`; rutis-host's `SpawnPlugin` used to have 30 s of its own and now uses the same constant.
- `Mount` has one more public field, `greeting: Option<Duration>`: unset uses `GREETING`, `Duration::MAX` means no limit (a process stopped at its first line for a debugger).
- With no greeting in time: `Error::Transport`, `the runtime process <name> started but did not greet within <n>s`, and the session is closed, which ends that process.
- Three limits: `Session::handshake` 10 s (the channel was opened by someone else, no process to start), `Process::mount` and rutis-host's `SpawnPlugin` 30 s (a cold process to start, the same constant).

### What to change

- A `Mount { … }` literal that names every field: add `greeting: None`, or use `..Mount::default()` (`Mount` implements `Default`).
- A process stopped at the runtime's first line for a debugger: set `greeting: Some(Duration::MAX)` on that mount, or the mount fails after 30 s and the process is ended.

### What it affects

Host owners: a runtime that does not greet now fails at its limit, with an error naming which runtime's process and how long it was given, and the process is ended; it used to wait for ever. With rutis-host nothing is visible: it was 30 s already.
