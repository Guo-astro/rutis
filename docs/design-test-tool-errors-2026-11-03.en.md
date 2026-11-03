# 0.21's testing tools: say why it failed

[中文](design-test-tool-errors-2026-11-03.md)

Status: draft, not implemented. Date: 2026-11-03. Baseline: `main` `d2a8d069` (after 0.20.1). Release: 0.21.
Issue: [#933](https://github.com/arcships/rutis/issues/933) (part of [#663](https://github.com/arcships/rutis/issues/663)'s A5, D1, D2). Implemented as: #933's T-1 to T-6 (§7).
Basis: [diagnostics](design-diagnostics-2026-10-26.en.md) (error codes §4.3, speaking only with what the developer wrote §4.5, `status` §5, the fault table §8.1); [error messages](https://github.com/arcships/rutis/issues/937) (#937's design draft, in review; once merged: `docs/design-error-messages-2026-11-03.md`: the waiting wording of `dev`, `status` and `diagnostics()`, and the state words, are its; this document only repeats them); [config schema](design-config-schema-2026-10-27.en.md) §5.2 (testing tools throw errors with `bad-config`); [plugin API](guide/plugin-api.en.md) "Testing tools", "How testing tools differ from a real runtime"; [rutis-host](guide/rutis-host.en.md) "What `run` prints" (the table of row state lines — "what `dev` says" throughout this document means that table).

Out of scope:

- The Swift and Kotlin testing tools. They go through the same `rutis-host test`; the method and the wording §3 adds to the host hold for them too. Hooking them up is an issue each for Swift and Kotlin; when done, their wording is identical to §4's.
- Testing tools reading the whole `status` snapshot (diagnostics design, "out of scope"; #689 listed it).
- Remote runtimes, nodes, plugins on devices.
- How an error a plugin throws itself crosses languages and prints (diagnostics design §4.4, #675, #687 settled it).
- Rows a `rutis.json` loads through `rutis-loader` in Rust: there the loader gives the row's name and positions, as `dev` does.

## 1. Now

### 1.1 The issue's own words

| Case | What a test shows | What `dev` / `check` shows |
| --- | --- | --- |
| injecting a service nobody provides | TS: `the plugin does not provide weather (now)`; Go: `timed out (10s): the service weather`; Rust: `(&plugin).await` returns `Ok`, then `root.get::<Weather>()` panics | `waiting for lmm (no row provides it; did you mean llm?)` |
| two plugins injecting each other | Go, TS: a timeout and nothing else | `they inject each other`, and `check` tells you to `break the cycle` |
| a service call that never returns | TS: after `node:test`'s timeout the test process did not exit (CI hangs); Python: the test hangs with no output at all | `status` lists the calls in flight |
| `apply` fails | Python: `PluginTestError: plugin: KeyError: 'town'`, no stack; TS: no source position | with a stack, pointing at the `apply` line |
| the row's name | Python: `plugin`, TS: the file's name `index`, Rust: the type's name `WeatherPlugin`; the guides' examples write the plugin's name (`greeter: config: …`) | the plugin's name |

Also: in TS, a test that failed before `unload` left the services it gave on the shared test host, and the next test reported the unrelated `llm is already provided by rutis-test` (`node/rutis/src/testing.mjs`); the guides' test examples had no `try/finally`.

### 1.2 The two that are already fixed

- **0.19.1 (#943)**: in TypeScript, once `node:test`'s limit passes on a call that never returns, the test process exits (#997 then fixed the two cases where the limit lands inside `load` and inside the host's start).
- **0.19.2 (#971)**: the TS and Bun tools unload at the end of the test; a failed test no longer leaves its services to the next one. Python's asynchronous calls have a 30 second limit.

This document does not redo the two; it lines their wording up with the other languages (§3.4, §4).

### 1.3 What each tool does today

The words of `dev` live in `crates/rutis-loader/src/loader/status.rs` (`describe_waits`, `why`) and `crates/rutis-host/src/status.rs` (`describe`, `prefixed`); the positions of an `apply` failure in `crates/rutis-bridge/src/report.rs` (`headline`, `whereabouts`). None of the three tools goes through either: the host `rutis-host test` uses the `Failure` strings `LoaderError` has already flattened (`crates/rutis-host/src/test.rs:414`), and asks nothing when the plugin never starts.

| Tool | The plugin waits forever | Two plugins inject each other | A call never returns | `apply` fails | The row's name |
| --- | --- | --- | --- | --- | --- |
| TypeScript, Bun | `load` returns as usual; `t.service('weather')` throws `the plugin does not provide weather (now)` (`node/rutis/src/testing.mjs:359`) | the same: nothing says it is two plugins | `node:test`'s `timeout` (§1.2); the tool keeps no record of the calls out | `index: Error: boom`, no position | the file name without its extension: `src/index.ts` → `index` (`base_id` in `crates/rutis-host/src/test.rs:393`) |
| Python | `load` throws `weather did not start: it injects llm, which nothing provides: give the test services=` (`python/rutis/rutis/testing.py:518`): the tool guessing, not `dev`'s words | `a did not start: it injects bsvc, which nothing provides: give the test services=`: `b` does provide it, only `b` has not started either | after 30 seconds `PluginTestError: weather.today() did not return within 30 s` (`testing.py:398`) | `plugin: KeyError: 'town'`, no stack | the name the definition declares, else `plugin` (`testing.py:492`) |
| Go | `Load` returns as usual; `loaded.Service("weather")` after 10 seconds: `timed out (10s): the service weather` (`rutistest.go:320`, `internal/testwait/testwait.go:17`) | the same: nothing more | the `ctx` the caller gives; without one, until `go test`'s own limit | `rutistest: weather: Error: boom`, a panic without a position | `Name` (`rutistest.go:608`) |
| Rust | `(&plugin).await` returns `Ok` (documented at `crates/rutis/src/fiber.rs:1714`), `root.get::<Greeter>()` is `None`, `.unwrap()` panics | the same | the test's own `tokio::time::timeout` | `?` hands the `Error` back and `cargo test` prints its `Debug`: `Error: PluginFailed(…)` | `Plugin::name()`: the type name's last segment, `GreeterPlugin` (`crates/rutis/src/plugin.rs:57`) |

## 2. Decisions

| # | Decision | Section |
| --- | --- | --- |
| T1 | **One place writes the line**: the host `rutis-host test` computes the row's state line with the same code `dev` uses (`describe` + `describe_waits` + `prefixed`), and the SDKs print it unchanged. That sentence's wording and codes are #937's and the diagnostics design's; this document only repeats them (`state(id)` is host-local: the protocol, `rutis.json` and the plugin API are untouched) | 3.1, 3.3 |
| T2 | **The row's name is the plugin's name** (package, module, `Name`), as the language guides' examples write it; an `id` the test gives wins | 3.2 |
| T3 | **When the plugin is not up, say what the row waits for**: `t.service()` and `loaded.Service()` no longer answer "no such service" but the row's state line; `waiting()` lets a test look on purpose. `load` still returns (a test may load the provider afterwards and let the row start) | 3.3 |
| T4 | **An `apply` failure carries its position**: the loader's `Failure` stops using `Display` and uses `report()` (the same headline plus the position lines); `dev`'s `cannot reload`, `rutis/loader` and the testing tools then say the same thing | 3.1, 6 |
| T5 | **A call that never returns fails at the tool's own limit**: 30 seconds in TypeScript, Bun, Python and Go; in Go the caller's own `ctx` deadline wins when it has one; only that call fails, and the test process can exit | 3.4 |
| T6 | **The tools' own failures have error codes** (`not-running`, `no-service`, `timeout`), and a host refusal carries the plugin error's code (`bad-config` and so on). All three languages have text only today; `code` and `data` are this document's addition | 3.5 |
| T7 | **A test leaves nothing behind for the next**: TS and Bun already do (§1.2); Python's `async with` and Go's `t.Cleanup` stay. What this adds is the "a refused load that got halfway" path | 3.6 |
| T8 | **Rust has no tool**: `FiberView` gets a "wait until it runs" method (`running(limit)`) whose failure says what `diagnostics()` says for that one plugin (its wording is #937's E-2), so all four languages say the same sentence, with only the name and "what it waits for" written in each language's own terms | 3.7 |

Dropped:

- **Failing `load` while the plugin waits**: a test may load a row first and its provider afterwards, which starts it (`node/rutis/test/testing-plugins.test.mjs` tests exactly that: "waits, and starts when it arrives"). The failure moves to where the row is used, and to `waiting()`.
- **Failing the test at its end because a row never started**: a test that loads a row to assert it waits would then fail; and TS's `unload` also runs in a hook after the test ended, where throwing only muddles the report.
- **One sentence per language** (Python's `did not start: it injects llm…` today): four texts drift apart, and the guess is wrong: it looks at whether anyone in that runtime provides the service, not at why this row did not start (with two plugins injecting each other it says nothing provides `bsvc`, while `b` will).
- **Handing the testing tools a whole `status` snapshot**: the diagnostics design puts it out of scope, and a tool needs this one line.

## 3. Rules

### 3.1 The host writes the line, the SDK prints it unchanged

This line (`<row id>: <state>`, what it waits for, where it failed, with the position lines after it) is not written by the testing tool: the host `rutis-host test` computes it with the same code `dev` uses, and an SDK puts it in the error without changing a character.

| Case | What the host uses |
| --- | --- |
| the row runs | `describe` (`crates/rutis-host/src/status.rs:18`) |
| the row waits | `describe_waits` (`crates/rutis-loader/src/loader/status.rs:411`, `Loader::waits` / `waits_of`) |
| the row failed | `describe` → `report()` (`crates/rutis-bridge/src/report.rs:172`): the same sentence plus the position lines |
| the row's name and the indent of the position lines | `prefixed` (`crates/rutis-host/src/status.rs:297`) |

**Those sentences, their wording and their codes belong to [error messages](https://github.com/arcships/rutis/issues/937) (#937's design draft, in review) and [diagnostics](design-diagnostics-2026-10-26.en.md) §4.3 and §4.5.** This document writes none of them and changes none: the waiting words of `status` and `dev`, `failed in apply:`, the position lines and the state words are settled there (#937's E-2 changes `no row provides it` to `nothing provides it yet`, and that is what a testing tool shows). This document adds three things:

1. one method for the test host (below);
2. the wording and codes of the two failures the tool itself produces (`no-service`, `timeout`, §3.3 and §3.4), plus the code `not-running` for when the row is not up;
3. how each language hands the code over (§3.5).

### 3.1.1 The body is the underlying error's own words

A message has two parts: a **prefix** answering "which of my calls was this" (the row, the `apply` step, the service and method), and a **body that is the underlying error's own words, verbatim**. No paraphrasing, no summarising, and no repeating what the error already said; when in doubt, keep the original.

| What the plugin threw | What the test shows (prefix + the original) |
| --- | --- |
| `KeyError: 'town'` (Python) | `greeter: failed in apply: KeyError: 'town'`, then `in greeter:` and `File "/…/greeter/plugin.py", line 42, in apply` |
| `new Error('boom')` (Node) | `greeter: failed in apply: Error: boom`, then `in greeter:` and `at apply (/…/src/index.ts:42:11)` |
| serde's ``invalid type: integer `42`, expected a string`` | ``greeter: failed in apply: invalid type: integer `42`, expected a string`` |
| Go's `json: unsupported type: chan int` | `greeter: failed in apply: json: unsupported type: chan int` |

`failed in apply: `, `in greeter:` and the position lines are rutis's; the rest is not touched. For this document the rule has one landing place: in the `line` `state(id)` gives, and in `load`'s failures, everything after the prefix must be exactly what the plugin threw (§4's tables say so word for word, and T-1's case compares it verbatim) — this document does not rewrite it, and an implementation may not "clear it up a little" on the way.

Only **errors rutis itself made up** become plain words: the waiting reasons (`nothing provides it yet`), which step failed, a config that does not match (`config: greeting should be a string, got 3`), and this document's three sentences (`no such service …`, `did not return within 30s`, `not running (…)`). They have no underlying original to keep: they are rutis's judgement, not something a plugin said. (Diagnostics §4.5 owns that vocabulary; the rule above is about not rewriting the plugin's own words.)


`rutis-host/test` is `rutis-host test`'s service (`Harness` in `crates/rutis-host/src/test.rs`), used only between the test process and the host it started. It gains a method:

```text
state(id) -> null | { line, code?, data? }
```

- `null`: the row runs. Otherwise `line` is that line above (what the host writes for `dev`), and `code` and `data` are the row error's code and details (left out when a plugin's own error has no code).
- **Read-only**: it reads a snapshot inside the host (`describe` / `describe_waits`), awaits nothing and changes nothing.
- **Who asks it, and when**: to an SDK it is one round trip (TS's and Python's `waiting()` await it), so **the two synchronous `t.service()`s do not make that round trip**: `load` (`Load`) asks once before it returns, `waiting()` asks every time, and `t.service()` uses the line those two saw last (§3.3). Go's `Service()` already treats calls to the host as blocking calls (it does when it loads), so it asks directly.
- **Host-local only**: it touches none of three things — the session protocol (version 4, `ENDPOINT_PROTOCOL`, `crates/rutis-bridge/src/session/mod.rs:14`), `rutis.json`'s format (1, `FORMAT`, `crates/rutis-host/src/config.rs:78`), the plugin API (3, `node/rutis/src/advanced.mjs:15`, `python/rutis/rutis/plugin.py:125`, `go/rutis/internal/runtime/version.go:9`).
- `load`, `update` and `unload` fail with the same `{ line, code?, data? }` (whose Display is `line`).

The two versions meeting:

| Case | What you see |
| --- | --- |
| a 0.20 testing tool on a 0.21 host | unchanged: the old tool never asks for `state`, and cannot see the extra method |
| a 0.21 testing tool on a 0.20 host (and, in TS and Bun, with 0.20's runtime package, `@arcships/rutis-runtime` or `@arcships/rutis-bun`: their test driver has no `state`) | while the row is not up: `greeter: not running (this rutis-host is older than 0.21 and cannot say why; upgrade it)`, code `not-running`; `waiting()` gives `null`. The guide says to bring `rutis-host` (and each runtime package in TS and Bun) to one version |

### 3.2 The row's name

In a test a row's id is **the plugin's name**, as the guides' examples write it (`greeter: config: …`):

| Language | Order |
| --- | --- |
| TypeScript, Bun | the `id` the test gives → the plugin's declared `name` → the `name` in the `package.json` nearest the plugin's file (scope dropped: `@ana/wordcount` → `wordcount`) → the file name without its extension → `plugin` |
| Python | the `id` the test gives → the plugin's declared `name` → the package of the module its `apply` is defined in (`greeter.plugin` → `greeter`) → `plugin` |
| Go | `Row(key)` → `Name` → the package's name → `plugin` |
| Rust | `Plugin::name()`: the type name's last segment; a plugin's own `fn name` when it has one |

A name already taken gets `-2`, `-3`… as before (`free_id` in `crates/rutis-host/src/test.rs:382`). Two plugins of one package both want the package's name, so the second is `<package>-2`: a test that loads two gives `id` (§4.1 says so).

The host's `base_id` (a file's last segment without its extension) stays as the last resort, for a language that cannot pass a name.

Rust differs: it has no `rutis.json` row, and the kernel knows a plugin by its name alone, so the row's name is `Plugin::name()`.

### 3.3 When the plugin is not up

- `t.service(name)` and `t.methods(name)` (TS, Bun, Python), `loaded.Service(name)` (Go), when the row is **not running**, fail with the row's state line: `greeter: waiting for translate (nothing provides it yet)`, `a: waiting for bsvc (row b waits for asvc too: they inject each other)`, `greeter: failed in apply: …`. Code `not-running`, `data: {row}`.
  - The reasons in the parentheses are #937 §3.3's words (`nothing provides it yet`, `row b waits for asvc too: they inject each other`); this document only repeats them, and changes with the host.
- The row **never started at all** (a 0.20 host): `greeter: not running (this rutis-host is older than 0.21 and cannot say why; upgrade it)`, also code `not-running` (§3.1).
- The row runs but has no such service: `greeter: no such service forcast (it provides: forecast, today; did you mean forecast?)`; when it provides nothing, `(it provides nothing)`. Code `no-service`, `data: {row, service, services}`.
- `await t.waiting()` (TS, Bun, Python), `loaded.Waiting()` (Go): `null` / `""` while it runs, otherwise the same line. Each call asks the host, so once a provider loaded later starts the row it is `null` at once.
- `t.service()` (TS, Bun, Python) is synchronous and cannot make that round trip: it first looks at what the runtime knows (TS's `provided`, Python's `runtime.slots`) to tell whether the row runs, and when it does not, it says why with the line `load` and `waiting()` saw last. Once the row runs it hands out the service as usual, and that line is never used.
- `load` and `Load` still return: a waiting row is not a failed load. `apply` really failing and a config that does not match are still failed loads, whose text becomes this line (§3.1).
- Python's `weather did not start: it injects llm, which nothing provides: give the test services=` goes away: it says whether anyone in that runtime provides the service, not why this row did not start (with two plugins injecting each other it says nothing provides `bsvc`, while `b` provides it), and it is not `dev`'s wording.

### 3.4 A call that never returns

| Language | Limit | What fails at the limit |
| --- | --- | --- |
| TypeScript, Bun | 30 seconds, the tool's (calls of asynchronous methods; a shorter `node:test` `timeout` wins) | `PluginTestError: greeter.hello() did not return within 30s` |
| Python | 30 seconds (already; the text changes from `30 s` to `30s`) | `PluginTestError: greeter.hello() did not return within 30s` |
| Go | 30 seconds when the caller's `ctx` has no deadline; the caller's deadline when it has one | `rutistest: greeter.hello() did not return within 30s` (the caller's own limit in seconds when it has one) |
| Rust | the test's own `tokio::time::timeout` | the test writes it |

- Code `timeout`, `data: {service, method, limit}`; `limit` is in seconds.
- Only that call fails; no other call ends and no runtime restarts.
- The process can exit: at the limit TS and Bun take that call off the "keeps the process alive" account (`hold` in `node/rutis/src/testing.mjs`), and the timer itself holds nothing; Python cancels the call (already); Go uses a `ctx` with a deadline (the same as a plugin calling another).
- A TypeScript `sync` method (`provides` says `sync`) cannot be interrupted: once the JS main thread blocks there is nowhere to run a timer. The guide says so; `node:test`'s `timeout` cannot interrupt synchronous code either, for the same reason.

### 3.5 Error codes

This document adds three codes for the testing tools, to the table in [diagnostics](design-diagnostics-2026-10-26.en.md) §4.3 (that table says rows may be added at implementation time, existing rows unchanged; #937 adds `already-provided` to the same table, each document adding its own rows). The table's other rows, and the rule that codes do not appear in the text people read, belong to it; this document changes neither:

| code | When | `data` |
| --- | --- | --- |
| `not-running` | the row is not running (waiting, failed, disabled) and a test needs it | `{row}` |
| `no-service` | the row runs but provides no such service name | `{row, service, services}` |
| `timeout` | a call did not return within its limit (§3.4) | `{service, method, limit}` |

- **Today those errors carry a name and a text only; `code` and `data` are this document's addition**:

| Language | Today | Added here |
| --- | --- | --- |
| TypeScript, Bun | `class PluginTestError extends Error { name = 'PluginTestError' }` (`node/rutis/src/testing.mjs:32`), so a test can only compare text | the fields `code` and `data` |
| Python | `class PluginTestError(AssertionError)` with a docstring only (`python/rutis/rutis/testing.py:64`) | the attributes `code` and `data` |
| Go | no error type: `Load` calls `t.Fatalf`, `TryLoad` hands back an `error`; when the host refused the load that is a `*rutis.RemoteError` whose `Name` is `BindingError` and whose `Code` is already set (as [config schema](design-config-schema-2026-10-27.en.md) §5.2 says) | `*rutistest.Error{Code, Line, Data}` |
| Rust | no tool | `running`'s error is a `rutis::Error`, matched by `ErrorKind` |

- A host refusal carries the row error's own code: a config that does not match is `bad-config` (`data: {issues}`, config schema design §5.2); a plugin's own error carries none.
- Codes do not appear in the text people read (diagnostics §4.3's rule).
- A test that asserts on the code does not compare text: `assert.equal(error.code, 'not-running')` in TypeScript and Bun, `self.assertEqual(refused.exception.code, 'not-running')` in Python, `errors.As(err, &target)` and then `target.Code` in Go.

### 3.6 When a test ends

- None of the three tools leaves a row or a service it gave to the next test: TS and Bun unload by themselves (§1.2), Python's `async with` and the withdrawal in its `finally` (`python/rutis/rutis/testing.py:525`), Go's `t.Cleanup` (`go/rutis/rutistest/rutistest.go:141`) and `release`. This document changes none of that; it adds a test that locks it.
- A failure halfway cleans up too: when a load is refused, the services already given and the rows already loaded in that batch are withdrawn before the error is raised (TS's `withdrawOwn` when `ask('load', …)` fails, Python's `finally`, Go's `release` when `start` fails). `state` and `waiting()` change nothing.
- Each tool gets two tests for it: one loads a plugin that fails (`apply` throws) and the next one loads the same plugin with the same services and passes; one fails before `unload` (TS, Bun) and the next one loads as usual.

### 3.7 Rust

Rust has no testing tool: step 6 of the guide builds a root, mounts the plugin and asserts, with no `rutis-host test`, no loader and no rows. It gets one thing, in `crates/rutis/src/fiber.rs`:

```rust
/// Wait until the plugin runs: `apply` is done. A failure (`apply` failed,
/// the config does not match) comes back at once and says the row's line;
/// a plugin that keeps waiting comes back once `limit` passes, saying what
/// it waits for.
pub async fn running(&self, limit: Duration) -> Result<(), Error>
```

- It runs: `Ok(())`.
- It failed: `Err` whose text is `<name>: <what diagnostics() says for this one plugin>`: `GreeterPlugin: failed in apply: boom`, `GreeterPlugin: config: greeting should be a string, got 3`.
- It keeps waiting: `Err` with the same sentence: `GreeterPlugin: waiting for Arc<dyn Translate> (nothing provides it yet)`; several dependencies are joined with `; `.
- When every unmet dependency is `Missing` (no plugin provides it) it comes back **at once**, without waiting for `limit`: in a test the test itself mounts every plugin, so waiting longer cannot help.
- **That sentence's wording is not this document's**: it is what `RuntimeDiagnostics`'s `Display` says for this one plugin (`crates/rutis/src/diagnostics.rs:65`, the waiting arm at `:75`), and `running` shares that function with it, so the two cannot drift. #937's E-2 changes the wording there (to the same words as `status`), and `running` changes with it; this document writes none of it.
- Step 6 of the guide uses it (§4.4); the `emit`, `dispose` and `shutdown` lines stay.

**Why Rust is not a different behaviour.** For the same input — a plugin that needs a dependency nobody provides — all four languages say the same sentence, `<name>: waiting for <what it needs> (<why>)`, with only two places written in the developer's own terms:

| | The name | What it needs | Why |
| --- | --- | --- | --- |
| TypeScript, Bun, Python, Go | the row's name: the plugin's name (§3.2) | a service name, `translate` for example | `nothing provides it yet` (#937 §3.3) |
| Rust | `Plugin::name()`: the plugin's name | the type it declares, `Arc<dyn Translate>` for example | the same sentence |

Rust has no row layer: the process is the test's own, the plugin runs on a fiber, there is no `rutis-host test`, no `rutis.json` row, and no module name to take a row id from, so the name can only be the plugin's own. What it needs can only be written as a type, because that is what `type Deps` holds. What changes is those two subjects, not the sentence.

### 3.8 What it cannot say

- Calls inside one runtime, and functions passed by reference: `dev` cannot see them either (diagnostics design §5.2), so a tool writes this one line.
- A synchronous TypeScript method cannot be interrupted (§3.4).
- The host process killed, a runtime that ends: the words come from the host in one place (`runtime py: down (…)`) and the tool does not wrap them again; a test has local runtimes only, so this is rare there.
- Errors a plugin throws itself, and positions across languages: diagnostics settled them; this document leaves them alone.

## 4. What developers see

Only the roles affected: plugin authors writing tests (one per language), the people assembling plugins with `rutis-host` (`rutis-host test`'s output changes, and `dev`'s wording too), and the people deploying (no change). Error codes per §3.5; codes do not appear in the text.

Each line below is what the tool prints, after the test framework's own prefix (`go test`'s `file:line:`, `cargo test`'s `panicked at …`); tests compare it word for word, and position lines only by file name and method (diagnostics design §4.6).

After each line's prefix comes the plugin's own words, untouched (§3.1.1). The waiting and failed lines are the line the host writes for `dev` (§3.1); the reasons in the parentheses are written as [error messages](https://github.com/arcships/rutis/issues/937) (#937) §3.3 has them today, and change with it. The two sentences the tools write themselves (`no such service …`, `did not return within 30s`) are this document's.

### 4.1 TypeScript and Bun

Normal:

```ts
// test/index.test.ts
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { load } from '@arcships/rutis/testing'
import plugin from '../src/index.ts'

const translate = { text: async (text: string) => ({ Hello: 'Bonjour', Hi: 'Salut' })[text] ?? text }

test('greets with the translated greeting', async () => {
  const t = await load(plugin, { config: { greeting: 'Hi' }, services: { translate } })
  assert.equal(t.row, 'greeter')
  assert.equal(await t.waiting(), null)
  assert.equal(await t.service('greeter').hello('Ada'), 'Salut, Ada!')
  await t.unload()
})
```

`npm test` prints `✔ greets with the translated greeting` (`node --test`'s reporter; `ok 1 - …` in TAP when the output is piped). The tool prints nothing else.

| Trigger | What you see | Code |
| --- | --- | --- |
| `services` lacks a service the plugin injects | `PluginTestError: greeter: waiting for translate (nothing provides it yet)` | `not-running`, `data: {row: 'greeter'}` |
| two plugins inject each other (on a `host()`) | `PluginTestError: a: waiting for bsvc (row b waits for asvc too: they inject each other)` | `not-running` |
| the provider failed | `PluginTestError: chat: waiting for llm (row model failed: Error: no key)` | `not-running` |
| the plugin runs, the service name is misspelt | `PluginTestError: greeter: no such service greter (it provides: greeter; did you mean greeter?)` | `no-service`, `data: {row: 'greeter', service: 'greter', services: ['greeter']}` |
| `apply` fails | `PluginTestError: greeter: failed in apply: Error: boom`, then `in greeter:` and `at apply (/Users/me/greeter/src/index.ts:42:11)` and the frames after it | none (the plugin's own error) |
| the config does not match | `PluginTestError: greeter: config: greeting should be a string, got 3` | `bad-config`, `data: {issues: [{path, message}]}` (the shape in [diagnostics](design-diagnostics-2026-10-26.en.md) §4.3) |
| a call never returns | `PluginTestError: greeter.hello() did not return within 30s` | `timeout`, `data: {service: 'greeter', method: 'hello', limit: 30}` |
| `await t.waiting()` | running: `null`; waiting: `greeter: waiting for translate (nothing provides it yet)` | none (it returns a string, it does not throw) |
| the host is 0.20 (no `state`) | `PluginTestError: greeter: not running (this rutis-host is older than 0.21 and cannot say why; upgrade it)` | `not-running` |

`t.service()` and `t.methods()` stay synchronous and still throw `PluginTestError` synchronously: they say why with the line `load` and `waiting()` saw last, and await no round trip there (§3.3).

Two plugins of one package on a `host()` both want the package's name, and the second is `<package>-2`: give `id` (`h.load(plugin, { id: 'shout' })`).

### 4.2 Python

Normal: as today (`async with load(plugin, config={...}, services={...}) as t`), with `t.row` now `greeter` (in the guide's example its `apply` lives in `greeter/plugin.py`, whose package is `greeter`).

| Trigger | What you see | Code |
| --- | --- | --- |
| `services` lacks a service | `PluginTestError: greeter: waiting for translate (nothing provides it yet)` | `not-running` |
| two plugins inject each other | `PluginTestError: a: waiting for bsvc (row b waits for asvc too: they inject each other)` | `not-running` |
| the service name is misspelt | `PluginTestError: greeter: no such service greter (it provides: greeter; did you mean greeter?)` | `no-service` |
| `apply` fails | `PluginTestError: greeter: failed in apply: KeyError: 'town'`, then `in greeter:` and `File "/Users/me/greeter/plugin.py", line 42, in apply` | none |
| the config does not match | `PluginTestError: greeter: config: greeting should be a string, got 3` | `bad-config` |
| a call never returns | `PluginTestError: greeter.hello() did not return within 30s` | `timeout` |
| `await t.waiting()` | `None` / the same line | none |
| the host is 0.20 (no `state`) | `PluginTestError: greeter: not running (this rutis-host is older than 0.21 and cannot say why; upgrade it)` | `not-running` |

`PluginTestError` stays a subclass of `AssertionError` and gains the two attributes `code` and `data`.

### 4.3 Go

Normal:

```go
loaded := rutistest.Load(t, Plugin, Config{Greeting: "Hi"}, map[string]any{"translate": fakeTranslate{}})
if line := loaded.Waiting(); line != "" {
    t.Fatal(line)   // it should be running once loaded
}
```

`go test ./...` prints `ok`. Calls of `loaded.Service("greeter")` are unchanged.

| Trigger | What you see | Code |
| --- | --- | --- |
| `services` lacks a service | `--- FAIL: TestHello (0.01s)` / `plugin_test.go:23: rutistest: greeter: waiting for translate (nothing provides it yet)` | `not-running` |
| two plugins inject each other | the same, `a: waiting for bsvc (row b waits for asvc too: they inject each other)` | `not-running` |
| the service name is misspelt | `rutistest: greeter: no such service greter (it provides: greeter; did you mean greeter?)` | `no-service` |
| `apply` fails | `rutistest: greeter: failed in apply: Error: boom`, then `in greeter:` and `at Apply (/Users/me/greeter/plugin.go:31)` | none |
| `apply` panics | `rutistest: greeter: failed in apply: Panic: panic: boom`, then the whole `goroutine …` stack (as a service method's panic is today) | none |
| the config does not match | `rutistest: greeter: config: greeting should be a string, got 3` (`TryLoad` gives back the same error) | `bad-config` |
| a call never returns | `rutistest: greeter.hello() did not return within 30s`; the caller's own `ctx` limit in seconds when it has one | `timeout` |

`rutistest.Error`: the fields `Code`, `Line`, `Data`; `Loaded.Service`, `Load`, `TryLoad` and `Update` hand back either it or the host's `*rutis.RemoteError`.

### 4.4 Rust

Normal (step 6 of the guide):

```rust
let plugin = root.plugin(GreeterPlugin::new(&Config { greeting: "Hi".into() })?);
plugin.running(Duration::from_secs(30)).await.unwrap_or_else(|error| panic!("{error}"));

let greeter = root.get::<Greeter>().unwrap();
assert_eq!(greeter.hello("Ada")?, "Salut, Ada!");
```

`cargo test` prints `test tests::greets_with_the_configured_greeting ... ok`.

| Trigger | What you see |
| --- | --- |
| no plugin provides a dependency | `thread 'tests::greets…' panicked at src/lib.rs:…: GreeterPlugin: waiting for Arc<dyn Translate> (nothing provides it yet)` (at once, not after 30 seconds) |
| `apply` fails | `GreeterPlugin: failed in apply: boom` |
| the config does not match | `GreeterPlugin: config: greeting should be a string, got 3` |
| a call never returns | the test's own `tokio::time::timeout`: the guide says how, the tool does not take its place |

For other waiting reasons (a provider that failed, one waiting for its own dependency) the parentheses hold what `diagnostics()` says of that plugin, worded by #937's E-2; this document does not repeat it.

A Rust row's name is `Plugin::name()`: `GreeterPlugin` above. The guide adds one sentence at that step: to call it `greeter` (its package's name), write `fn name(&self) -> &str { "greeter" }` in `impl Plugin`.

### 4.5 People assembling plugins with rutis-host, host owners, people deploying

- **People assembling plugins with `rutis-host`**: `rutis-host test`'s output changes (it is the service the testing tools use); in `dev`, a row whose `apply` fails after an edit becomes more than one line (`weather: cannot reload: failed in apply: …` plus the position lines, `; the previous version keeps running` as before); `check` only dry-runs and does not call `apply`, so its wording is unchanged.
- **Host owners** (applications embedding rutis): no wording change; `Failure`'s `error` field gains the position lines (§6), so code matching on the text has to change.
- **People deploying**: no visible change.

### 4.6 Concepts to know

Against the concept budget #663 wrote down (7 to write a plugin, about 9 reasonable while debugging): this design asks plugin authors to know nothing more. `waiting()` and `running(limit)` are new ways into one concept they already need, the row's state (`dev`'s `waiting for …`); error codes are for programs, not a new concept. In a test, "row" gains one sentence: the row's name is the plugin's name.

## 5. Where the cases live

**No shared expectation file, and no new `specs/` directory**: unlike [`specs/config-schema/cases.json`](../specs/config-schema/cases.json), a case here is not a piece of data but "a plugin plus one action in a test", and a cross-language file would have to hold a plugin's code written three times over. The cases live in each tool's existing tests:

- **The line itself** is written in one place (§3.1) and locked in `crates/rutis-host`'s tests: one case runs `dev` and `state` and compares the same row (T-1's completion condition). Its words change with #937's E-2.
- **That the tool prints it unchanged** is one case per tool: a plugin that needs a service nobody provides, asserting that the text `t.service()` / `loaded.Service()` throws is exactly what the host gave.
- **The two sentences the tool writes itself** (`no such service …`, `did not return within 30s`) are one case per language, and the expected text is the sentence written down in §3.3 and §3.4; the three agree word for word, and reviews read them together.
- Rust is locked by `running`'s tests in `crates/rutis` (§3.7).

## 6. Compatibility and migration

Written into "Migrating to 0.21" (both languages):

- **Row names change**: in TypeScript and Bun a test's row id goes from the file name's first segment (`index`) to the package's name (`greeter`); in Python from `plugin` to the package's name. Tests asserting `t.row`, or matching rows in `reported()`, have to change. Tests that give `id` are unaffected.
- **Python's `load` no longer throws for "this row is waiting"**: `weather did not start: it injects llm, which nothing provides: give the test services=` is gone; the same mistake now appears on `t.service()` as the row's state line (`greeter: waiting for translate (nothing provides it yet)`), and in `await t.waiting()`.
- **Some wording changes** (tests matching text have to change): `the plugin does not provide X (now)` → the row's state line or `no such service …`; `timed out (10s): the service X` → the same; Python's `did not return within 30 s` → `30s`; `plugin: KeyError: 'town'` → `greeter: failed in apply: KeyError: 'town'` plus the position lines — only the prefix changes (the row, the step); `KeyError: 'town'` is not touched (§3.1.1).
- **`Failure`'s `error` carries positions**: a Rust host that reads `Failure.error` as one line has to read several (`error.lines()`); `dev`'s `cannot reload` line becomes more than one line with it. `check` is unchanged (it does not call `apply`).
- **`DependencyStatus` / `RuntimeDiagnostics`'s wording**: not this document's, but [#937](https://github.com/arcships/rutis/issues/937)'s E-2 (internal words such as `service missing` go); `running` follows it.
- **New**: `rutis-host/test`'s `state` (host-local, §3.1); `FiberView::running`; `PluginTestError.code` / `.data` (absent today, §3.5); `rutistest.Error`. The plugin API (3), `rutis.json`'s format (1) and the session protocol (4) are unchanged.
- Versions meeting: a 0.20 testing tool on a 0.21 host is unchanged; a 0.21 tool on a 0.20 host sees `greeter: not running (this rutis-host is older than 0.21 and cannot say why; upgrade it)` (§3.1).

## 7. Implementation

| Issue | What | Done when |
| --- | --- | --- |
| T-1 | `rutis-host` and `rutis-loader`: `state(id)` on `rutis-host/test` and its errors with `code`/`data`; `Failure`'s positions (`report()` instead of `Display`); reusing `describe`/`describe_waits`/`prefixed`; positions for a panicking Go `Apply` (the Go runtime) | the host's line is word for word what `dev` prints (one test runs `dev` and `state` and compares the same row); every text-matching test in `crates/rutis-host` and `crates/rutis-loader` is updated to the new wording, with the change called out as user-visible |
| T-2 | TypeScript, Bun: the row's name (the package); `waiting()`; the lines and codes of `service()`/`methods()`; the 30 second call limit and releasing `hold`; withdrawing the batch when a load is refused | step 6 of the guide's example passes as it is written; one case each in `node/rutis/test` and `bun/rutis-bun/test`: the text thrown is exactly the host's and the code is `not-running`; after a test fails, the next one loads the same plugin with the same services and passes; the 0.20 host case (§3.1) |
| T-3 | Python: the row's name (the package); `waiting()`; the lines and codes of `service()`/`methods()`; `30 s` → `30s`; dropping `did not start` | the same (`python/rutis/tests/test_testing.py` gains those three cases) |
| T-4 | Go: `Waiting()`; the lines and codes of `Service()`; 30 seconds for a `ctx` without a deadline; `rutistest.Error` | the same (`go/rutis/rutistest`'s tests gain those three cases) |
| T-5 | Rust: `FiberView::running(limit)`, sharing one sentence with `RuntimeDiagnostics`; step 6 of the guide's example. **After #937's E-2**, which settles `diagnostics()`'s wording | one test in `crates/rutis` locks the four texts (waiting, `apply` failed, config does not match, failing at once); the guide's example passes as it is written |
| T-6 | Documentation (both languages): [plugin API](guide/plugin-api.en.md)'s "Testing tools" and "How testing tools differ from a real runtime", step 6 of the four language guides, [rutis-host](guide/rutis-host.en.md)'s `test` and "What `run` prints" (`dev`'s wording is unchanged; add that the testing tools say the same), "Migrating to 0.21" | the four guides' step 6 examples and outputs agree; both languages in step |

T-1 merges first; T-2 to T-4 are one pull request each, in order; T-5 comes after #937's E-2 and may run alongside T-2 to T-4; T-6 goes with the last implementation.

#933 closes once all of it holds: every line in §4 has a word-for-word test in all four languages, and step 6 of the four guides' examples and outputs agree.
