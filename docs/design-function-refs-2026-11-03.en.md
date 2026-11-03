# How a function passed by reference is called: asynchronous by default, synchronous inside a synchronous call

[中文](design-function-refs-2026-11-03.md)

Status: draft. Date: 2026-11-03. Baseline: `main` `d2a8d069` (0.20.1).
Issue: [#932](https://github.com/arcships/rutis/issues/932) (part of [#663](https://github.com/arcships/rutis/issues/663)'s O2, O5, A5). Implemented by: F-1 to F-4 (§7), all under #932. Schedule: 0.21.
Sources: [plugin API](guide/plugin-api.en.md) "How values cross", "Synchronous calls and re-entrancy", "Python's test tool"; [a host that opens up plugins](guide/open-host.en.md) §5; [object references](design-object-refs-2026-10-27.en.md) §4 (an object's methods are asynchronous calls) and §6 (wait cycles); the [diagnostics design](design-diagnostics-2026-10-26.en.md) §4.3, §4.5; [#918](https://github.com/arcships/rutis/pull/918) (Node's Cordis synchronous chains); the [release gate](ci.en.md) and the [install scenarios](design-e2e-install-2026-10-11.en.md).

Out of scope:

- The side that **hands a function out**: the arguments, results, errors and release of a plugin's function called by a host or another plugin keep their rules ([plugin API](guide/plugin-api.en.md) "How values cross"). This design does not change them.
- The protocol: the call frame's `sync` marker and the wait-cycle check (`bridge/sync-wait-cycle-refused`) exist; no field, no capability and no protocol version changes.
- Calls of objects (objects with methods, class instances): already asynchronous ([object references](design-object-refs-2026-10-27.en.md) O2). Unchanged.
- The Go, Swift, Kotlin and Rust forms: they already write a call of a received function in two ways (`Call(ctx, args, true)`, suspend / blocking, `call` / `call_async`). Neither their forms nor their behaviour change.
- In Python's test tool, a value inside one process is the object itself (Cordis's semantics, [object references](design-object-refs-2026-10-27.en.md) §4): unchanged. A function a plugin hands to the test is the plugin's own function, called directly (§3.5); making that look like a call across processes belongs to [#933](https://github.com/arcships/rutis/issues/933).
- Call deadlines and cancellation: none added, none changed ([diagnostics](design-diagnostics-2026-10-26.en.md) §2).

## 1. Where things stand

### 1.1 Node and Bun: a received function is called synchronously, so a test's fake cannot be `async`

In [#663](https://github.com/arcships/rutis/issues/663)'s 0.19.0 evaluation (O2, O5, A5), both the host owner and third-party plugin authors ran into this:

> The TypeScript test example open-host.md §5 gives plugin authors fails as written. The plugin registers a handler with the host (`commands.register(name, handler)`), the test calls that handler through a fake host service, and calling another fake service from inside the handler reports `SyncWaitCycle`.

Following the guide as written today, the test under `npm test` fails and prints:

```text
SyncWaitCycle: await requires the Node thread occupied by its parent synchronous call; path: test:5 -> rutis-test:3 -> rutis-test:4
```

The cause is in the runtime, not in the example:

- A received function is a `#requestSync` proxy (`node/rutis-runtime/src/session.mjs:473`): calling it waits for the result on the spot, and the Node thread is occupied while it waits (`#requestSync`, `session.mjs:573`).
- While it waits, only frames belonging to that call chain run; any other frame is queued until the stack empties (`session.mjs:690`).
- So when the handler does `await notes.list()`, the Promise the fake `notes.list` returned on the test's side needs this thread to make progress to settle, and this thread is waiting; the `await` is refused (`session.mjs:707`), which is the line above.

Bun's runtime has the same shape (`bun/rutis-bun/src/session.ts:468`, `:568`, `:694`).

That synchronous call also decides how the guide is written: the example's fakes must return their values directly (`list: () => [...]`, not `async`), which is not what a real service looks like. [#942](https://github.com/arcships/rutis/pull/942) changed the example to return directly and [#967](https://github.com/arcships/rutis/pull/967) stopped `node --test` from cancelling the test while the test tool waited (0.19.2), but **the call itself is still synchronous** and the fakes still cannot be `async`:

```text
- The test calls the command it received as a function, and such a call waits for its result on the test's thread; while it waits, that thread cannot settle a Promise. So a fake the command calls while it runs answers at once (`list: () => [...]`, not `async`). Written `async`, the call fails:
```

(`docs/guide/open-host.en.md:798`; the Chinese guide says the same.)

### 1.2 Python: a received function is called synchronously and blocks the event loop

A function received by Python is a `RemoteFunction` (`python/rutis/rutis/peer.py:678`). Calling it goes through `__call__` → `_call_sync` (`peer.py:686`, `:1438`): it blocks on the event loop's thread, pumping frames until the result arrives. When the other side's function returns a plain value, `await f(...)` gets Python's own error, word for word `TypeError: object str can't be used in 'await' expression` (3.12, run here): what came back is not awaitable. The same class has `call_async` (`peer.py:690`), and the Python guide never mentions it.

Authors writing plugins in Python meet one more layer: what a received function returns depends on whether it is `async def` or `def`. With `async def` the result of `f(...)` is awaitable; with `def` it is the value itself. The same expression behaves differently in the two cases.

### 1.3 Neither the guides nor the release gate caught it

The example carries `<!-- example: jot-command run -->` (`docs/guide/open-host.md:780`), but the release gate only checks that the markers parse (`.github/workflows/install.yml:89`); the examples themselves run only after a release, with the packages from the registries (`install.yml:263`–`270`, `tests/e2e/tests/doc_examples.rs:13`). So an example that cannot run against the packages a release gate builds can reach users and be found only afterwards: the checks after the 0.19.0 and 0.19.1 releases both stopped here ([#966](https://github.com/arcships/rutis/issues/966)) and the guides after it were not reached.

### 1.4 The half that cannot change

Synchronous calls themselves were settled by #918: Cordis's `emit` and `waterfall` are synchronous in Node, synchrony holds only for the emitter, and while the emitter waits, calls the dispatch makes back into the emitter's process are synchronous calls (`node/rutis-runtime/src/events.mjs:361`–`370`). `sync` methods, `SyncWaitCycle`, and the wait-cycle check the host makes before forwarding a synchronous call (`crates/rutis-bridge/src/session/rpc.rs:1869`) are written behaviour too. So received functions cannot simply all become asynchronous: in those places the code cannot `await` in the first place.

## 2. Decisions

| # | Decision | Section |
| --- | --- | --- |
| F1 | **A received function is asynchronous by default**: like a service method, it returns a Promise (Node, Bun) or an awaitable (Python); `await` it to get the value of the other side's function | 3.1, 3.2 |
| F2 | **Synchronous only inside a synchronous call**: when this call happens inside one (we are synchronously waiting, or we are serving the other side's synchronous call), the call returns the result itself instead of a Promise. The runtime knows which; the developer does not write it twice | 3.2, 3.3 |
| F3 | **No new names**: no `callSync` on received functions in Node or Bun; Python's `call_async` is the same thing as `await f(...)` and goes into its guide | 3.2 |
| F4 | **Cordis's synchronous chains stay as they are** (#918): where the runtime itself calls `next` in a synchronous chain it uses an internal synchronous form; what plugin authors write does not change | 3.3 |
| F5 | **The test tools follow the real host**: in Node and Bun the plugin's row runs in another runtime process, so a test `await`s a received function; in Python the plugin runs in the test's process, so the function is the plugin's own (a `def` is used directly, an `async def` is awaited). Both are written in the guides | 3.5 |
| F6 | **When it fails**: a real cycle still fails with `SyncWaitCycle` (same `name`) and never hangs; the wording is the [diagnostics design](design-diagnostics-2026-10-26.en.md)'s, and this draft proposes a sentence there (§4.3's dependency) | 4.3 |
| F7 | **The release gate runs the guide examples that install only npm / PyPI packages once**, with the packages the gate itself builds, on Linux (including both versions of open-host §5); after a release the whole set still runs with the published packages | 6 |
| F8 | **The wire format, the capabilities and the Rust side do not change**: `call` / `call_async`, the protocol's `sync` marker and the wait-cycle check stay as they are | 5 |

Alternatives dropped:

- **Only change the documentation (teach people to write synchronous tests)**: the example would keep differing from the real host, an `async` fake could never be written, and third-party authors could only skip that test (the evaluation's words). Dropped.
- **Give received functions an explicit synchronous entry (`await f(...)` plus `f.callSync(...)`)**: one more concept, and a synchronous chain (`sync` method, Cordis's synchronous dispatch) that forgets it silently gets a Promise. Dropped.
- **Make everything asynchronous and drop synchronous chains**: it would break `sync` methods and #918's Cordis synchronous dispatch. Dropped.
- **Keep a separate synchronous name next to Python's `__call__` (`call_sync`)**: two names for one kind of call, unlike "await a service method" everywhere else, and a synchronous call that forgets it only gets a coroutine. Dropped.

## 3. Rules

### 3.1 Calling a received function is calling the other side's function

- Arguments, results and thrown errors cross as "How values cross" says ([plugin API](guide/plugin-api.en.md)); unchanged.
- The same function handed over twice is the same proxy; handed back to its process, it is the original function.
- Lifetime and release are unchanged: when the receiver no longer holds the proxy it is released, and the sender lets go ([object references](design-object-refs-2026-10-27.en.md) §5 gives the same treatment).
- No deadline, and no cancellation (a JS Promise cannot be cancelled), as today.

### 3.2 When it is asynchronous, when synchronous

**Asynchronous**: the call is not inside a synchronous call — a plugin's `apply`, an `async` service method, a listener, a timer, any asynchronous work. Then:

| Language | Calling the received function `f` | What it gives |
| --- | --- | --- |
| TypeScript, Bun | `await f(...)`, or pass the Promise on | A Promise: the other side's result; when the other side returns a Promise or a coroutine, `await` waits through to its value |
| Python | `await f(...)` (`f.call_async(...)` is the same thing) | An awaitable |
| Go, Swift, Kotlin, Rust | Unchanged: `*advanced.Service`'s `Call(ctx, args, false)` (the synchronous form is `true`), a struct of function fields (blocking), `ctx.Call`, a suspending method, `call_async` | Each language's asynchronous form |

**Synchronous**: the call happens inside a synchronous call. Two sources, both known to the runtime:

1. **We are synchronously waiting**: we sent a synchronous call (a `sync` method, `bail`, the dispatch a Cordis synchronous `emit` / `waterfall` sends), and while we wait for it this thread / event loop is occupied.
2. **We are serving the other side's synchronous call**: the other side called our `sync` method with `sync` (or our listener is being called synchronously), and we are inside that call.

Then the call of a received function goes out synchronously and gives the result itself (in Python, `f(...)` without `await`) — because there is nothing to await with: a JS thread is occupied and microtasks do not run; Python's event loop is occupied and coroutines do not run. This is the same rule #918 settled: **synchronous waiting only inside calls that are already synchronous.**

Where the runtime keeps the state (its own, not exposed):

| Runtime | The marker |
| --- | --- |
| Node (`session.mjs`), Bun (`session.ts`) | A counter: one more while `#requestSync` waits, one more while executing an incoming frame that carries `sync`; a received function's proxy reads it to pick `#requestSync` or `#requestAsync` |
| Python (`peer.py`) | A counter: one more while `_call_sync` waits, one more while `_execute` runs a frame that carries `sync` (a coroutine method stops counting at its first suspension, which is when the loop can run again); `RemoteFunction.__call__` reads it to return an awaitable or the value |

- **One expression, two results**: in JS and Python a call of a received function is the same expression, and whether it returns a Promise depends on the context. That matches the types a host generates: a function parameter's return type is already written `string | Promise<string>` (what `rutis-host types` writes, [a host that opens up plugins](guide/open-host.en.md) §2). What the guides teach is `await` (in Python `await f(...)`, the same as `f.call_async(...)`); inside a synchronous call the `await` goes away and a bare `f(...)` is written (it could not run there anyway).
- **Why the runtimes do not share one treatment**: in JS and Python the call expression does not tell synchronous from asynchronous, and a `sync` method must still be able to call a received function, so the runtime has to decide from the context. Go, Swift, Kotlin and Rust already write the two forms apart and need none of this.

### 3.3 Nothing can `await` inside a synchronous call

Awaiting the result of a received function inside a synchronous call is today's `SyncWaitCycle`:

- when the runtime itself notices (the result needs this thread / loop to make progress to settle), §4.3 row 1 says so and only that call fails; nothing hangs;
- when the host notices a cycle before forwarding a synchronous call, §4.3 row 2 says so and the call is not forwarded.

Both are "what cannot be done is reported, never replaced silently" ([design principles](design-philosophy.en.md) §7). The behaviour is today's; only the new ways of writing it become rarer.

### 3.4 Calls back into us (re-entrancy)

- **While we call asynchronously**: our thread / event loop is free, so the other side may call us at any time (service methods, events, a function handed back to us), as with any service call. It may also do so before answering our call.
- **While we call synchronously**: calls back into us still run — frames of this chain first (Node, Bun; `session.mjs:690`), and Python runs other incoming calls during a synchronous wait too (`peer.py:1688`, "re-entrant"). A **synchronous** call back into us that in turn waits for us is refused by the host as a wait cycle (§4.3 row 2).
- **Do not wait, inside one synchronous call, for a Promise only the other side can settle**: that is a real cycle, see above. Ask the other side to be quick, or make the method `async`.

### 3.5 The test tools

| Language | A received function in a test | How to write it |
| --- | --- | --- |
| TypeScript, Bun | The plugin's row runs in another runtime process (the `rutis-host test` host plus the test's own runtime), so the function crosses processes | As with the real host: `await` it; a fake may be `async` |
| Python | The plugin's row runs in the test's own process ([Python's test tool](guide/plugin-api.en.md#the-python-test-tool)), so the function is the plugin's own | `await` an `async def`, use a plain `def`'s value directly; a fake may be `async def`. One from another process (another plugin, a host's callback) is a `RemoteFunction`: write it asynchronously, `await f(...)` (the same as `f.call_async(...)`); a bare `f(...)` is for a synchronous call only (§3.2, §3.3) |

Neither test tool changes its semantics; Python's difference goes into the Python guide and the plugin API guide, as §4.2 says.

### 3.6 Cordis (Node)

- Cordis's synchronous `emit` / `waterfall` stay as #918 left them: the emitter waits synchronously, and `next` returns the chain's value inside a synchronous chain.
- Where the runtime itself calls `next` in a synchronous chain it uses an internal synchronous form (`Session.callFunctionSync`, the mirror of the existing `callFunctionAsync`); what plugin authors write does not change (`events.mjs:361`–`370`). `next` is a received function, so 3.2 applies to it: synchronous inside a synchronous chain, a Promise elsewhere.
- Cordis code in a plugin that calls a function from another process follows 3.2.

### 3.7 The wording of an error: the underlying error, as it is

- **The body of a message is the underlying error's own words, verbatim**: serde's ``invalid type: integer `42`, expected a string``, Python's `KeyError: 'town'`, Go's `json: unsupported type: chan int`, an error the plugin threw itself. Runtimes and hosts carry it across unchanged: no rewriting, no translation, no summary.
- **rutis adds one short prefix**, answering "which of my calls caused this" (which service, which method, which argument, which row).
- **It does not repeat what the underlying error already said**, and does not explain it either. Everything past the prefix is the error's own words.
- **Only an error rutis made up is rewritten into plain words**: its own jargon, internal ids (the `path: test:5 -> …` in the `SyncWaitCycle` line), implementation words that should not show. That is not an underlying error; rewriting it is what this rule asks for, not an exception to it.
- When in doubt, keep the original.

## 4. What developers see

### 4.1 Plugin authors: TypeScript (the normal case)

The example in the guide's §5 (`docs/guide/open-host.md` §5): the template and the generated types are unchanged, as they are today. The test's fakes go back to `async`, as a real service is — the only line of the example that changes today:

```ts
// count/test/index.test.ts
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { load } from '@arcships/rutis/testing'
import plugin from '../src/index.ts'

test('counts the notes and their words', async () => {
  const commands = new Map<string, (args: string[]) => Promise<string>>()
  const t = await load(plugin, {
    services: {
      notes: { list: async () => ['buy milk', 'call Ada back'] },
      commands: { register: async (name: string, run: (args: string[]) => Promise<string>) => { commands.set(name, run) } },
    },
  })
  assert.equal(await commands.get('count')!([]), '2 notes, 5 words')
  await t.unload()
})
```

```console
$ npm test
✔ counts the notes and their words (127.752458ms)
ℹ tests 1
ℹ suites 0
ℹ pass 1
ℹ fail 0
ℹ cancelled 0
ℹ skipped 0
ℹ todo 0
ℹ duration_ms 161.385083
```

In `dev`, as before:

```console
$ npx rutis-host dev
count: running
$ npx rutis-host call commands.run '"count"'
"2 notes, 5 words"
```

The guide's §5 paragraph saying fakes must return directly and that `async` fails, and the `SyncWaitCycle` output under it, go away; one sentence takes their place: a test `await`s a function the plugin received, and a fake may be `async`.

Elsewhere a plugin calls a received function the same way:

```ts
// A function from another plugin, or a callback a host gave: await it
const answer = await answerFor('what is 6 times 7?')
```

### 4.2 Plugin authors: Python (the normal case, and where it differs from TypeScript)

The second version of the same example, for Python authors (new, marked `jot-command-python`, run by the release gate):

```python
# count/src/count/__init__.py
from rutis import define_plugin


async def apply(ctx, config):
    commands = ctx.use("commands")
    notes = ctx.use("notes")

    async def count(args):
        all_notes = await notes.list()
        words = len(" ".join(all_notes).split())
        return f"{len(all_notes)} notes, {words} words"

    await commands.register("count", count)


plugin = define_plugin(apply, inject=["notes", "commands"])
```

The fake jot `dev` uses (the `py:fake_jot` row; the same fake jot the TypeScript version has, written in Python):

```python
# count/dev/fake_jot.py
from rutis import define_plugin


class Jot:
    def __init__(self):
        self.notes = ["buy milk", "call Ada back"]
        self.commands = {}

    async def list(self):
        return list(self.notes)

    async def add(self, text):
        self.notes.append(text)

    async def register(self, name, run):
        self.commands[name] = run

    async def run(self, name, args=None):
        return await self.commands[name](args or [])


def apply(ctx, config):
    jot = Jot()
    ctx.provide("notes", jot)
    ctx.provide("commands", jot)


plugin = define_plugin(
    apply,
    provides={"notes": {"add": "async", "list": "async"}, "commands": {"register": "async", "run": "async"}},
)
```

`count/rutis.dev.json`:

```json
{
  "format": 1,
  "rows": [
    { "id": "count" },
    { "id": "jot", "name": "py:fake_jot" }
  ]
}
```

```python
# count/tests/test_plugin.py: the plugin runs in the test's own process,
# so the function it handed over is its own
import asyncio
import unittest

from rutis.testing import load

from count import plugin


class FakeNotes:
    async def list(self):
        return ["buy milk", "call Ada back"]


class FakeCommands:
    def __init__(self):
        self.runs = {}

    async def register(self, name, run):
        self.runs[name] = run


class Plugin(unittest.TestCase):
    def test_counts_the_notes_and_their_words(self):
        async def go():
            commands = FakeCommands()
            async with load(plugin, services={"notes": FakeNotes(), "commands": commands}) as t:
                self.assertEqual(await commands.runs["count"]([]), "2 notes, 5 words")

        asyncio.run(go())


if __name__ == "__main__":
    unittest.main()
```

```console
$ uv run python -m unittest discover -s tests
.
----------------------------------------------------------------------
Ran 1 test in 0.412s

OK
```

One difference from the TypeScript example, written in the guide: **a test gets the plugin's own function**. An `async def` is awaited (the example above); a plain `def` the plugin handed over returns its value, so the test uses it directly; awaiting a `str` raises Python's own `TypeError: object str can't be used in 'await' expression`, nothing to do with rutis. **A function from another process is called asynchronously**: another plugin's function or a host's callback is a `RemoteFunction`; `await f(...)` (the same as `f.call_async(...)`) gives its result. A bare `f(...)` returns the result synchronously and holds the event loop, so it is only for a synchronous call (a `sync` method, Cordis's synchronous dispatch; §3.3), where there is nothing to await with. Today (0.20) a cross-process `f(...)` always returns synchronously, which is why `await f(...)` on a plain value raises the `TypeError` above; that is exactly what this design removes (§3.2). In `dev`, with a `py:fake_jot` row it is the same process too, and `npx rutis-host call commands.run '"count"'` gives what the TypeScript example gives:

```console
$ npx rutis-host call commands.run '"count"'
"2 notes, 5 words"
```

### 4.3 When it fails

This design writes neither error codes nor the wording people read: those belong to the [diagnostics design](design-diagnostics-2026-10-26.en.md) §4.3, §4.5. What is here is how the two failures appear, which row each is recorded on, and how to fix them; the wording follows that design.

| Case | How it is caused | Which row it is recorded on | How to fix |
| --- | --- | --- | --- |
| Awaiting a received function's result inside a synchronous call | `await f()` in a `sync` method (or in Cordis's synchronous dispatch), where `f`'s result needs this thread / loop to make progress to settle | the calling row; in a test, that test fails | Inside a synchronous call the `await` goes away: the received function gives its result there, so use it directly; or make the method `async` |
| Two runtimes calling each other's received functions synchronously | A calls B's received function inside a synchronous call, and B calls A's inside its own | the row that sent the synchronous call | Use an `async` method, or another moment ([plugin API](guide/plugin-api.en.md) "Synchronous calls and re-entrancy"); do not retry the same call at once |

- Both failures end as `SyncWaitCycle`, with the `name` it has today; its code and its sentence follow the [diagnostics design](design-diagnostics-2026-10-26.en.md). Today the Node and Bun line carries internal call ids (quoted as it is in §1.1), and the host's line names the rows the call went through.
- **Dependency**: this draft proposes that the diagnostics design add a row to its §4.3 and a sentence to its §4.5 for the case the runtime notices itself — the sentence this draft proposes is `SyncWaitCycle: <what> would wait on itself: it is awaited inside the synchronous call that produces it (synchronous calls)` (`<what>` is the call: `a function`, `the method get`, `weather.tomorrow`). Whether to take it, and its wording, are that design's owner's decision. Rewriting this one is exactly §3.7's last item: `SyncWaitCycle` is an error rutis made up, and today's line carries rutis's internal ids; an error a plugin threw, Python's `TypeError` and a Go encoder's error are underlying errors and stay as they are — this draft does not re-word them.
- Every other error is as before (`bad-value`, `no-method`, `peer-gone`; that design's §4.3).
- In the normal case (§4.1, §4.2) there is no new output at all: calls return, tests pass.

### 4.4 Host owners

- Calling a function a plugin handed over is written as before: `call` (synchronous) and `call_async` (asynchronous) in Rust, each language's own form in Go, Swift and Kotlin.
- When a host hands a callback to a plugin, the plugin's code awaits it; in a Node, Bun or Python plugin's `sync` method it still gets the result synchronously. No host code changes.
- `rutis-host status` still cannot see calls of functions passed by reference ([a host that opens up plugins](guide/open-host.en.md) §6): they are calls between a local runtime and itself.
- How `SyncWaitCycle` crosses a session is unchanged (`crates/rutis-bridge/src/session/protocol.rs:89`–`111`, `:140`–`144`):
  - a refusal the host makes itself is `session::Error::SyncWaitCycle(message)` (`crates/rutis-bridge/src/session/rpc.rs:1875`); it reaches a runtime as `Failure { name: "SyncWaitCycle", message, graph: None }`, with **no `code`, no `data` and no graph**;
  - the one a runtime throws carries an error graph, and the host receives it as `session::Error::Remote { name: "SyncWaitCycle", message, graph }` (`crates/rutis-bridge/tests/rpc_callbacks.rs:221`, `:480`); without a graph it becomes `Error::SyncWaitCycle` again;
  - what runtime code sees: in Node and Bun an `Error` whose `name` is `SyncWaitCycle` and which has no `code` (`node/rutis-runtime/src/errors.mjs:133`–`140`); in Python `rutis.advanced.SyncWaitCycle` (`python/rutis/rutis/peer.py:416`); in Go a `*rutis.RemoteError` for which `errors.Is(err, advanced.ErrSyncWaitCycle)` holds (`go/rutis/internal/errs/errs.go:38`);
- nothing new goes on the wire: `code` and `data` live in the graph only (that design's §4.3), and `SyncWaitCycle` has none.

### 4.5 People assembling plugins with `rutis-host`

- The project `rutis-host new --lang node|bun|python` makes, `rutis.dev.json`, `dev` and `call` are unchanged.
- The fakes they write (`dev` rows) may be `async`; `dev` and `call` print what they print today (§4.1).

### 4.6 People deploying

No change: the processes, the packages, the configuration, stopping and exit codes are the same.

### 4.7 Concepts to learn

- **Plugin authors** (budget 7, [#663](https://github.com/arcships/rutis/issues/663)): no new concept. The rule uses "an asynchronous method / `async`" and "a `sync` method", already on the main path; the guide says it in a sentence.
- **Plugin authors writing Python tests**: one more thing known (a test gets the plugin's own function; one from another process is written as `await f(...)`), not a new name, in the same section.
- **Host owners** (budget 10): no new concept.
- **People assembling plugins with `rutis-host`** (budget 6), **people deploying** (budget 5): none.

## 5. Compatibility and migration

### 5.1 The protocol and versions, item by item

| Item | Now | This design |
| --- | --- | --- |
| Session protocol | 4: one constant in each of the six runtimes and in the host (5.1a) | Unchanged: no frame, no field and no capability added (the `sync` marker and `sync-stack` / `reentrant-sync` stay as they are) |
| `rutis.json` | `FORMAT` = 1 (`crates/rutis-host/src/config.rs:78`) | Unchanged: no new row field and no new `runtimes` form |
| Plugin API | 3: six SDKs declare it, Rust has none (5.1b) | Unchanged: nothing is added to or taken from `definePlugin` / `define_plugin`; how a received function is called is not in the API version (it is the runtime's behaviour) |
| Errors on the wire | `Failure { name, message, graph? }` (`crates/rutis-bridge/src/session/protocol.rs:89`) | Unchanged: `SyncWaitCycle` is still the `name`, with no `code`, no `data` and no graph (§4.4) |
| Generated types | what `rutis-host types` writes, `.d.ts` and `.py` (`run: (arg0: string[]) => string \| Promise<string>`) | Unchanged, no regeneration needed: a callback's return type already writes both, and `await` type-checks in an asynchronous and in a synchronous context alike (§3.2) |
| Rust's `call` / `call_async`, the Go / Swift / Kotlin forms, Python's `call_async` | — | Unchanged |

#### 5.1a Session protocol 4: the one constant in each language

| Language | Constant | Value |
| --- | --- | --- |
| Node | `ENDPOINT_PROTOCOL`, `node/rutis-runtime/src/session.mjs:58` | 4 |
| Bun | `ENDPOINT_PROTOCOL`, `bun/rutis-bun/src/session.ts:57` | 4 |
| Python | `ENDPOINT_PROTOCOL`, `python/rutis/rutis/peer.py:42` | 4 |
| Go | `EndpointProtocol`, `go/rutis/internal/peer/peer.go:34` | 4 |
| Kotlin | `ENDPOINT_PROTOCOL`, `kotlin/rutis/rutis/src/shared/kotlin/arcships/rutis/internal/peer/Peer.kt:42` | 4 |
| Swift | `endpointProtocol`, `swift/rutis/Sources/Rutis/Session/Peer.swift:13` | 4 |
| Rust (the host) | `ENDPOINT_PROTOCOL`, `crates/rutis-bridge/src/session/mod.rs:14` | 4 |

A far end whose value is not this one is refused, saying which end to upgrade: Node's line begins `incompatible session: the far end speaks protocol <far>, this side <local>` (`node/rutis-runtime/src/session.mjs:86`), Python (`python/rutis/rutis/peer.py:50`) and Rust (`crates/rutis-bridge/src/session/rpc.rs:1229`) have the same shape, and each of the six runtimes and the host has its own check. This design does not touch it.

#### 5.1b Plugin API 3: who declares it

| Language | Constant | Value |
| --- | --- | --- |
| Node | `PLUGIN_API`, `node/rutis/src/advanced.mjs:15` | 3 |
| Bun | `PLUGIN_API`, `bun/rutis-bun/src/plugin.ts:21` | 3 |
| Python | `PLUGIN_API`, `python/rutis/rutis/plugin.py:125` | 3 |
| Go | `PluginAPI`, `go/rutis/internal/runtime/version.go:9` (the manifest's `pluginApi`, `go/rutis/internal/runtime/serve.go:133`) | 3 |
| Kotlin | `PLUGIN_API`, `kotlin/rutis/rutis/src/shared/kotlin/arcships/rutis/advanced/Versions.kt:7` | 3 |
| Swift | `Advanced.pluginAPI`, `swift/rutis/Sources/Rutis/Version.swift:28` | 3 |
| Rust | **no such constant**: a Rust plugin is compiled into its host, with no manifest and no `pluginApi`; the host reads the manifest of a binary runtime only, whose constant is `crates/rutis-loader/src/runtime/go.rs:34` and `swift.rs:33`, and refuses one larger than its own (`crates/rutis-loader/src/runtime/binary.rs:1111`) | — |

A plugin declaring a newer API than the runtime is refused, naming the version the plugin asks for and the runtime's own (Node's line also says which package to upgrade): Node `node/rutis-runtime/src/leaf.mjs:25`, Bun `bun/rutis-bun/src/plugin.ts:100`, Python `python/rutis/rutis/runner.py:1417`, Go `go/rutis/internal/runtime/runtime.go:280`, Kotlin `kotlin/rutis/rutis/src/shared/kotlin/arcships/rutis/internal/Runtime.kt:267`, Swift `swift/rutis/Sources/Rutis/API/Runtime.swift:420`. This design changes neither the version number nor that check.

### 5.2 Mixing: an old end meeting a new one

The difference lives inside this side's runtime and is no agreement across a process, so every combination works as before:

| Combination | Result |
| --- | --- |
| A 0.21 host with a 0.20 runtime | The host is unchanged; a received function in that runtime is still called synchronously (0.20's behaviour). A plugin written for 0.21 with `await` still works on a 0.20 Node or Bun (`await` of a value that is not a Promise gives the value); in Python a 0.20 `f(...)` returns the value synchronously and awaiting a plain value fails, so one Python plugin should not run on both |
| A 0.20 host with a 0.21 runtime | The host is unchanged; the runtime follows this design (asynchronous by default, synchronous inside a synchronous call). The wait-cycle check and `SyncWaitCycle` are the host's, already there, and behave as today |
| A 0.21 caller calling a function in a 0.20 runtime | When the caller is not in a synchronous call it sends an asynchronous call, and the 0.20 runtime answers it as usual, so `await` gives the result; in a synchronous call it is synchronous, as today |
| A 0.20 caller calling a function in a 0.21 runtime | The 0.20 caller sends a synchronous call (its rule); the 0.21 runtime returns the result synchronously inside it, as today |

### 5.3 Written into "Migrating to 0.21" (both languages, new files)

- **A received function is asynchronous by default (Node, Bun, Python)**: calling a received function in `apply`, an `async` method, a listener or a timer now gives a Promise / awaitable. The old way (waiting for the result on the spot) still holds inside a synchronous call; code that does not wait inside a synchronous call and uses the value must `await`:
  - TypeScript, Bun: `const text = f()` → `const text = await f()` (`await` of a value that is not a Promise gives the value, so the changed code also runs on 0.20);
  - Python: `text = f()` → `text = await f()` (the same as `f.call_async(...)`); inside a synchronous call keep `f()`, which returns the result there and leaves nothing else to write.
- **A test's fakes may be `async`**: what had to return directly may now do either. A test whose fake is `async` and which awaits the plugin's function passes on 0.21 and fails on 0.20 — write it for 0.21 and do not mix.
- **The `SyncWaitCycle` line** (Node, Bun): as the diagnostics design takes it (§4.3's dependency); its `name`, and code that branches on it, do not change.
- **One incompatible change, and only that**: the first item above (a received function becomes asynchronous in an asynchronous context). The rest is unchanged or more permissive.

### 5.4 The guarantees table

[guarantees](guarantees.en.md) gains `bridge/function-refs`: a received function is asynchronous in an asynchronous context, synchronous inside a synchronous call, and a real cycle fails with `SyncWaitCycle`; `boundary/5.6` gains a sentence pointing here.

## 6. The release gate runs the guide examples once

Today (`.github/workflows/install.yml`): the `plan` job only runs `node tools/doc-examples.mjs --check` (line 89), and the marked examples run only when `RUTIS_E2E_SOURCE == 'registry'`, after a release (`install.yml:263`–`270`). The packages a release gate builds (`RUTIS_E2E_ARTIFACTS`) already exist — `tests/e2e/tests/devx.rs` runs the developer scenarios on an unreleased version with them — but the examples do not use them.

Decision (F7):

1. **A local package source**: `tools/install/artifact-registry.mjs` serves the tarballs in `RUTIS_E2E_ARTIFACTS/npm` as a local npm registry: a packument per package name (one version, `dist.tarball` pointing at itself) and the tarballs as they are. The example keeps its commands (`npx @arcships/rutis-host new …`, `npm install`, `npx rutis-host …`); `npm_config_registry` in the environment points them here. Python uses the existing approach: `UV_FIND_LINKS` pointing at `RUTIS_E2E_ARTIFACTS/wheels`, an empty index, and `UV_INDEX_URL` / `PIP_NO_INDEX` as `find_links` in `tests/e2e/tests/plugin_install.rs:385` does.
2. **The examples run as written**: `Scenario::installed` (`tests/e2e/src/lib.rs:265`) gains a variant that uses this build's packages: a new home directory with the npm and uv caches inside it, npm and uv pointed at item 1, and otherwise the same steps as the post-release run (not one line of the examples changes).
3. **Which examples the release gate runs**: those that install only npm and PyPI packages — `jot-command`, `jot-command-python` (with each guide's `-en` name), `plugin-api-ts`, `plugin-api-py`, `typescript`, `bun`, `python`, `rutis-host` — on Linux; those needing crates.io or the Go module proxy (`jot`, `rust`, `go`, `swift`) still run after a release with the published packages, because the gate has none of those.
4. **After a release, as before**: the step at `install.yml:263`–`270` is unchanged and runs every example.

So open-host §5's example runs in every release gate, with the packages about to be published, and the post-release run confirms the published packages behave the same.

## 7. Implementation

| Stage | Contents |
| --- | --- |
| F-1 | Node and Bun runtimes: a received function's proxy picks `#requestSync` / `#requestAsync` from the context; the synchronous-call marker (§3.2's table); `events.mjs`'s synchronous `next` uses the internal `callFunctionSync`; the `SyncWaitCycle` line as the diagnostics design takes it (§4.3); the runtimes' tests |
| F-2 | Python runtime: `RemoteFunction.__call__` returns an awaitable or the value from the context, `call_async` the same; `_execute` counts a frame carrying `sync` as a synchronous call; the runtime's tests |
| F-3 | The documentation (both languages): open-host §5 (the `SyncWaitCycle` paragraph goes, the fakes go back to `async`, the new Python example and its one difference); the [plugin API](guide/plugin-api.en.md) ("How values cross"'s function row, "Synchronous calls and re-entrancy", the Python and TypeScript test-tool sections); the test sections of the [TypeScript](guide/typescript-plugin.en.md), [Bun](guide/bun-plugin.en.md) and [Python](guide/python-plugin.en.md) guides; "Migrating to 0.21"; the guarantees table |
| F-4 | The release gate: `tools/install/artifact-registry.mjs`, the local-packages variant of `Scenario`, `install.yml` running item 3's examples when `source=local`, and `doc_examples.rs`'s local mode |

F-1 and F-2 can go in parallel; F-3 changes both documents after them; F-4 is independent of the rest.

Done (when all hold, #932 closes):

- [ ] open-host §5's TypeScript example passes `npm test` as written with an `async` fake, and the Python example passes too (one test each).
- [ ] Node, Bun and Python each have tests: a received function is a Promise / awaitable in an asynchronous context, gives its result inside a synchronous call, and a real cycle fails with `SyncWaitCycle` without hanging.
- [ ] The diagnostics design takes the sentence §4.3 proposes (a dependency); the Node, Bun and Python lines match it, compared word for word against that design.
- [ ] Both documents are changed together, and §5's example has run in the release gate.
- [ ] With `source=local`, the release gate runs item 3's examples against this build's packages and passes; the post-release run still passes with all of them.
- [ ] The guarantees table has `bridge/function-refs`, naming tests that pass (`node tools/check-guarantees.mjs`).
