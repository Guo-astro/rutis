# Trying and testing plugins: `rutis-host call`, `dev` lists services, fakes in the plugin's own language, test tools listen to events

[中文](design-try-and-test-plugins-2026-10-27.md)

Status: draft, not implemented. Date: 2026-10-27. Baseline: `main` `103c0d22` (after 0.17.2). Scheduled for: 0.19.
Issue: [#670](https://github.com/arcships/rutis/issues/670) (A5, A6 and H5 of [#663](https://github.com/arcships/rutis/issues/663)).
Relation to 0.18: built on [diagnostics for 0.18](design-diagnostics-2026-10-26.en.md), without repeating or changing it: `call` uses the local socket and the way of finding it from its §5.3, the services `dev` lists are the `services` of its §5.1 `Loader::status()`, and error text follows its §4 (`site`, error codes, the banned words of §4.5). Depends on [#678](https://github.com/arcships/rutis/issues/678) being merged first.
Relation to other 0.19 designs: generating strict fakes from the host's `#[rutis::service]` and putting them into the test tools belongs to [#684](https://github.com/arcships/rutis/issues/684); this design only covers fakes plugin authors write themselves and the test tools' `on`. SDK default entry points belong to [#692](https://github.com/arcships/rutis/issues/692): this design adds no names to an SDK root; `on` goes on the object the test tool loads. Limiting which services a plugin may use belongs to [#683](https://github.com/arcships/rutis/issues/683); it limits plugins, and `call` is the host's own call.

Out of scope: `call` on Windows (0.18's `status` is not provided there either); writing values other than JSON on the command line (integers beyond 64 bits, instants, dates, bytes, Map, Set, Decimal, UUID, functions); calling services inside instances; emitting events from the command line (`rutis-host emit`); `call` starting a host (it only connects to a running one); `run` printing a service table (use `rutis-host status`); `new` generating fakes (template and guide examples belong to [#671](https://github.com/arcships/rutis/issues/671)); fakes in another language for TypeScript and Python projects (they can already use their own); a "collect the received events" shortcut in the test tools (`on` plus a list is enough); an install hint when the Node runtime lacks `@arcships/rutis` (error text, part of 0.18 #677's rewrite list). Removed in review (no user): a peer credential check for `call`, a dedicated table of how non-JSON result values print, rebuilding only the binaries that contain a file by `go list -deps` / `swift package describe`, and not printing a service line "identical to the last one".

## 1. What developers want to do, and where they get stuck

A Go author writes a `weather` plugin that uses an `llm` service. They want to:

1. call `weather.today("Oslo")` once it runs, and see what it returns or what it reports when it fails;
2. after a reload, confirm `weather` still provides `forecast`;
3. write a fake `llm` in Go and run it with `weather`;
4. test that "every call emits `weather/asked`".

The 0.17.0 evaluation (#663) got stuck on all four:

| # | Now | Cost |
| --- | --- | --- |
| 1 | No way to call | TypeScript, Python, Go and Rust authors each wrote a separate "caller" plugin; the host author wrote a probe plugin |
| 2 | `dev` only prints `weather: running` | No way to see which services are provided, and by whom |
| 3 | `dev` only builds the project's own Go binary; the guide says to write fakes in Python or TypeScript | The Go author created a Node project inside the Go project, installed npm packages, and hit two errors on the way |
| 4 | The TypeScript and Python test tools have no `on` (Go, Swift and Kotlin do); the Python test tool's interface is undocumented | A separate listener plugin on a shared bus; `dir()` to find the interface |

## 2. Existing seams

| Needed | Existing |
| --- | --- |
| The host calls a service by name | `rutis_bridge::session::call_service(&root, service, method, args)` ([Rust host guide](guide/rust-host.en.md#call-a-service-by-name-from-rust)): services of any language, remote runtime or node; sync methods on a blocking thread; the call comes from no row |
| Finding a running host from another process | 0.18 §5.3: `run` / `dev` open a local socket (0600, the system's runtime directory, a file name with a hash of the configuration path); `rutis-host status` finds it from the same configuration path |
| Services and providers | 0.18 §5.1: `Loader::status().services: [{name, provider}]` |
| "The change has finished" in `dev` | `dev` already does `loader.settled().await` after starting, reloading, rebuilding and rereading `rutis.dev.json` (`crates/rutis-host/src/dev.rs`) |
| Building Go and Swift binaries and running them as runtimes | `GoDev` / `SwiftDev` (`languages/go.rs`, `swift.rs`), `dev_rows` reads the rows from a binary's manifest, `GoRuntime.binaries` already lists several binaries, one runtime per binary; the caches of `go build` and `swift build` make an unchanged binary quick to build |
| Text for each kind of value in a result | `Value::json()`; each value's `Display`; `describe()` from error text |
| Test tools listening to events | Go `loaded.On(name, fn)`, Swift `loaded.on(name) { … }`, Kotlin `loaded.on(name) { … }` exist, with one rule: listen as the host does, at the root scope, returning a disposer |

So all four fill gaps in what exists: no new channel, no new state data, no change to the kernel or the loader.

## 3. Decisions

| # | Decision | Section |
| --- | --- | --- |
| T1 | `rutis-host call <service>.<method> [args…]`: connects to the same socket 0.18's `status` uses, which answers one more request, `call`; found as `status` finds it; Unix only | 4 |
| T2 | `dev` accepts `call` by default; `run` does not, unless started with `run --allow-call`; who can connect follows 0.18's socket (0600), with no extra check | 4 |
| T3 | Each argument is one JSON value, and argument types JSON cannot express are not supported; the result is printed as JSON, values JSON lacks in the text they already have | 4 |
| T4 | A call made by `call` has "the host" as its caller (as every call not made from a row in 0.18); the error's `site` is, as in 0.18, the called row | 4 |
| T5 | `dev` prints one service line after starting and after each change has settled, from `Loader::status().services` | 5 |
| T6 | Fakes in the plugin's own language: a row in `rutis.dev.json` whose `name` is a directory of the project (a Go main package, a Swift executable target) is built by `dev` as a second binary and run as its own runtime; `new` does not generate fakes | 6 |
| T7 | When a source file of a Go or Swift project changes, every binary of the project is rebuilt, and only the runtimes whose binary changed restart; `check` builds them too | 6 |
| T8 | The TypeScript (Node, Bun) and Python test tools get `t.on(name, listener, options)`, returning a disposer, the same form as the Go, Swift and Kotlin test tools and each language's `ctx.on`; on the loaded object, not on an SDK root | 7 |
| T9 | The Python test tool's interface goes into the Python guide (a reference table); each language guide's "Test" section mentions `on` | 7 |

## 4. `rutis-host call`

### 4.1 Usage

```text
rutis-host call [--config <path>] <service>.<method> [args…]
```

- `<service>.<method>` is split at the last `.` (service names may contain dots; method names do not).
- Each argument is one JSON value: `3`, `true`, `null`, `'"Oslo"'`, `'{"days": 3}'`, `'[1, 2]'`. An argument that is not JSON is an error, not guessed as a string (guessing `007` or `yes` wrong silently sends the wrong value).
- The host is found exactly as `rutis-host status` finds it (0.18 §5.3): the configuration path given with `--config`, else `rutis.json` in the current directory; in a plugin project, the project `dev` runs. Nothing is written into the project.

### 4.2 On the host's side

- 0.18's socket answers one more request: `call`, with the service name, the method name and the argument list. The format on the socket is rutis-host's own; `call` and `status` are the same version of rutis-host, and nothing is promised to others. 0.18's "answers only `status`" still holds for a `run` that does not accept `call`.
- The host runs `call_service::<Value>(&root, service, method, args)`, as a Rust host's call by name does: the service the root scope sees by that name, provided by a row of any language, a remote runtime or a node; sync methods on a blocking thread.
- No time limit (0.18 §6.1: how long to wait belongs to the caller). `rutis-host call` waits for the result; Ctrl-C ends the `call` process and closes the socket, the host drops the call's future, and cancellation reaches the other side as it already does (a TypeScript method is not interrupted, as in 0.18 §6.1).
- One call per connection; several `call`s at once do not affect each other.

### 4.3 Permissions

| | `dev` | `run` | `run --allow-call` |
| --- | --- | --- | --- |
| `status` | answered | answered | answered |
| `call` | accepted | refused, saying how to allow it | accepted |

- **Why `run` refuses by default**: `call` runs any method of any service; in production that would be a back door for any process of the same user. The default matches 0.18's read-only socket; operators allow it explicitly on the command line when they need it. The switch is a command-line flag (`--allow-call`), not a `rutis.json` field: it is a decision about this run, and does not travel with a configuration file copied elsewhere.
- **Who can connect**: as in 0.18, the socket file is 0600, so only the same user (and root) can connect; Linux and macOS both require write permission on the file to connect to a Unix socket, so no peer credential check is added.
- **Windows**: 0.18 provides no socket, so there is no `call`; it prints `rutis-host call is not available on Windows`.
- The rutis-dev channel is not reused, for 0.18's reason: it is not published, and it carries commands that change things; `call` only needs the one socket 0.18 already adds.

### 4.4 Argument and result values

Arguments support only what JSON can write: `null`, booleans, numbers, strings, arrays, objects. Integers within 64 bits follow the existing rule (beyond ±(2^53−1) they cross as big integers, [plugin API: the ten values](guide/plugin-api.en.md#the-ten-values)). Dates, bytes, Map, Set, Decimal, UUID, larger integers and functions cannot be written on the command line and are not supported: try such methods in a test. A string arrives as a string (a Python method expecting a `date` gets a `str`, as from a string JS sends).

The result is printed as JSON (two-space indent) on standard output with `Value::json()`; no result is `null`. Values JSON lacks get no new form, only the text they already have: instants, dates, big integers, Decimal and UUID their `Display` (such as `"2026-10-27"`); bytes, Map, Set and references the `describe()` text (such as `"<a map>"`). It is for people only, not guaranteed to be sendable back as it was.

### 4.5 Who the caller is

A call made by `call` comes from no row, and is recorded as "the host":

- in the state snapshot (0.18 §5.2) it is `host → weather.today  3s`;
- the error's `site` is, per 0.18 §4.1, the called row, and `call` prints its location; there is no caller location;
- a method that needs the calling row's `&Ctx` (registering a tool for a row, say) fails, as `call_service` already does; the text is in §8.

No separate "command line" identity: rutis-host itself calls no service, so in rutis-host the only calls the host makes are `call`s, and nothing is confused; a separate identity would change `caller_row()` and 0.18's snapshot, with no one to use it.

## 5. `dev` lists services

- Once started (every row `settled`), it prints one line:

  ```text
  rutis-host dev: services: forecast (weather), llm (llm)
  ```

  Sorted by service name; in parentheses the row providing it (0.18's `provider`; `host` for the host). Without services: `rutis-host dev: services: none`.
- After each reload, rebuild or reread of `rutis.dev.json` has finished (after the existing `settled`), it prints the line again, as the issue's "print after starting and reloading" says, without comparing with the last one.
- The data comes only from `Loader::status().services`. `run` does not print it (production output does not change); use `rutis-host status` there.
- It is a new kind of line in `dev`'s output, starting with `rutis-host dev: ` like the existing `rutis-host dev: running …`, not shaped like `<row>: <state>`, so scripts reading state lines are unaffected; existing lines do not change. It goes into the output table of "Developing plugins with dev" in rutis-host.md.

## 6. Fakes in the plugin's own language

### 6.1 Which form: `dev` builds another entry point

The issue gives two: `dev` builds another entry point of the project, or `new` generates a runnable fake. The first:

- `new` cannot know which services a plugin will use; a generated fake only helps the template's example, and a fake the author writes still needs a way to run, which is the first form.
- The first form matches what TypeScript and Python already do: one row in `rutis.dev.json` whose `name` points at the fake in the project (`./dev/fake-llm.ts`).
- Whether the template ships a fake is a template and guide question (#671), using this mechanism.

### 6.2 How it is written (Go)

```text
weather/
  go.mod  plugin.go  plugin_test.go
  cmd/weather/main.go        # the plugin
  dev/fakellm/main.go        # the fake llm: an ordinary Go main package
  rutis.dev.json
```

```go
// dev/fakellm/main.go
package main

import (
	"context"

	"github.com/arcships/rutis/go/rutis"
)

type llm struct{}

func (llm) Ask(ctx context.Context, question string) (string, error) { return "sunny", nil }

var plugin = rutis.Define(rutis.Plugin[rutis.NoConfig]{
	Name:  "fakellm",
	Apply: func(ctx *rutis.Ctx, _ rutis.NoConfig) error { ctx.Provide("llm", llm{}); return nil },
})

func main() { rutis.Serve(plugin) }
```

```json
{
  "format": 1,
  "rows": [
    { "id": "weather", "config": { "city": "Oslo" } },
    { "id": "llm", "name": "./dev/fakellm" }
  ]
}
```

Rules:

- A row in `rutis.dev.json` whose `name` starts with `./` and names a directory of the project: in a Go project, a main package under the same go.mod; in a Swift project, the directory's name is the name of an executable target in Package.swift. `dev` builds it into `.rutis/go/fakellm-<n>` (Swift: `.rutis/swift/<target>-<n>`, with `swift build --product <target>` as `SwiftDev` does today) and runs it as its own runtime (`go-fakellm`); the row is that binary's plugin.
- The binary must serve exactly one plugin; with several it is an error (§8). A runtime name equal to the project's own is an error that asks to rename the directory.
- Such rows exist only in `dev` and in `check` of a plugin project (both read `rutis.dev.json` through `project::dev_config`): reading turns them into the existing `go-<name>:<plugin>` rows and adds the build to the Go settings (`GoRuntime.dev` goes from one binary to a list). `rutis.json` and `run` do not accept this form; the loader does not change.
- `dev/fakellm` is an ordinary package of the module: `go build ./...` and `go test ./...` compile it; the release workflow builds only `cmd/<name>` and is unaffected.
- Tests still put a Go value into `rutistest.Load`'s `services`. To share one type with `dev`, put it in an ordinary package (such as `internal/fake`) imported by both `dev/fakellm/main.go` and the test; the guide says so in one sentence.

Swift is the same: add an executable target to Package.swift (`.executableTarget(name: "FakeLLM", dependencies: [.product(name: "Rutis", package: "rutis")], path: "dev/FakeLLM")`) and name the row `./dev/FakeLLM`. The extra Package.swift entry is SwiftPM's rule, not rutis's.

### 6.3 What a change rebuilds

Today any changed `.go` file of a Go project rebuilds the project's binary and restarts its runtime. With a second binary, changing the fake should not restart the plugin, nor the other way round ("one plugin's change does not restart others"):

- When a source changes (by today's `is_source`), every binary of the project is rebuilt; one the change did not touch is built quickly from the cache of `go build` / `swift build`.
- A new binary with the same content as the one its runtime runs is not swapped in and does not restart anything; only a different one is swapped in and restarted as today. Go builds give the same file for the same inputs; should a Swift build differ, it only restarts once more, and the result is still right.
- Each binary keeps "a failed build leaves the last build running", with the lines printed today (`<project>: building` uses the binary's directory name).

### 6.4 Other languages

TypeScript and Python projects can already write fakes in their own language (`./dev/fake-llm.ts`, `py:fake_llm`); unchanged. Kotlin does not run in `rutis-host dev` (it is an app's plugin). Rust plugins are compiled by their host, not through `dev`.

## 7. Test tools listen to events directly

### 7.1 Form

The Go, Swift and Kotlin test tools already have `on`; TypeScript (Node and Bun share `@arcships/rutis/testing`) and Python get the same:

```ts
const t = await load(weather, { config: { city: 'Oslo' }, services: { llm } })
const asked: string[] = []
const off = t.on('weather/asked', (city: string) => { asked.push(city) })
await t.service('weather').today()
assert.deepEqual(asked, ['Oslo'])
off()
```

```python
async with load(plugin, config={"city": "Oslo"}, services={"llm": FakeLlm()}) as t:
    asked = []
    off = t.on("weather/asked", lambda city: asked.append(city))
    await t.service("weather").today()
    assert asked == ["Oslo"]
```

- Name, arguments and options as the language's `ctx.on`: TypeScript `t.on(name, listener, { prepend, global })`, Python `t.on(name, listener, *, prepend=False, global_=False)`; the listener may be sync or async, and its return value counts in `serial`, `waterfall` and `bail` as usual.
- The rules of Go's `loaded.On`: it listens as the host does, at the root scope; the plugin's emissions of the name and the test's own reach it; in strict mode (the default) the arguments arrive as from another process; it returns a disposer; one not disposed is removed when the test tool unloads.
- On the loaded object: TypeScript's `Loaded` (every row a `host()` loads has it; they share one bus), Python's `Loaded`. Not on an SDK root and not on `ctx` (`ctx.on` exists), in line with #692's "plugin authors see 7 concepts by default": `on` is a method within the "test tool" concept.
- No second form that returns a list of received events: a list and one line suffice, and the four languages keep one form.

### 7.2 Documentation

- The Python guide's "Test" section gets a reference table: the parameters of `load(...)` (`config`, `services`, `strict`, `bus`, `scope`, `row`, `runtime`, `deadline`), the members of `Loaded` (`service`, `methods`, `provided`, `on`, `emit`, `parallel`, `serial`, `waterfall`, `bail`, `filter`, `reported`, `update`, `unload`), and `Bus`, `Reported`, `PluginTestError`. Taken from the docstrings of `rutis/testing.py`; the guide and they agree.
- The "Test" sections of the TypeScript, Bun, Python, Swift and Kotlin guides each mention `on` (the Go guide already does).
- Each language guide's "Run in a local host" section adds a `rutis-host call` step; the Go and Swift guides switch to fakes in their own language (§6.2).

## 8. What developers see

A plugin author in the Go project `weather`:

```console
$ rutis-host dev
weather: building
weather: built in 1.4s
fakellm: building
fakellm: built in 0.6s
weather: running
llm: running
rutis-host dev: running weather; changes reload it (Ctrl-C ends)
rutis-host dev: services: forecast (weather), llm (llm)
```

In another terminal:

| Input | Output (stdout / stderr) and exit status |
| --- | --- |
| `rutis-host call forecast.today '"Oslo"'` | `"sunny in Oslo"`, 0 |
| `rutis-host call forecast.week '{"days": 3}'` | JSON with a two-space indent, 0 |
| `rutis-host call forecast.today Oslo` | `rutis-host: argument 1 is not JSON: write a string in quotes, as '"Oslo"'`, 1 |
| `rutis-host call forecast` | `rutis-host: name a method, as forecast.<method>`, 1 |
| `rutis-host call forcast.today '"Oslo"'` | `rutis-host: no row provides forcast; did you mean forecast? (services: forecast, llm)`, 1 |
| `rutis-host call forecast.tomorow '"Oslo"'` | `rutis-host: forecast has no method tomorow (methods: today, week)`, 1 (0.18 `no-method`) |
| `rutis-host call forecast.today 3` | `rutis-host: forecast.today: argument 1 should be a string, got 3`, 1 (0.18 `bad-argument`) |
| The method throws | `rutis-host: call to forecast.today failed: KeyError: 'region'`, then `weather at plugin.go:31 in Today`, 1 |
| The provider waits for its runtime | `rutis-host: forecast is not available (row weather waits for its runtime go-weather)`, 1 (0.18 `unavailable` reason) |
| The method needs a calling row | `rutis-host: tools.register needs a calling row; call it from a plugin, not from rutis-host call`, 1 |
| No host running | `rutis-host: no rutis-host run or dev is running for /work/weather (start one, or name its configuration with --config)`, 1 |
| The host is `run` without `--allow-call` | `rutis-host: this rutis-host run does not accept calls; start it with rutis-host run --allow-call`, 1 |
| Windows | `rutis-host call is not available on Windows`, 1 |
| Ctrl-C while waiting | nothing printed; the host cancels the call; 130 |
| `rutis-host status` meanwhile | `host → forecast.today  12s` |

After changing `dev/fakellm/main.go`:

```text
weather: building
weather: built in 0.2s
fakellm: building
fakellm: built in 0.5s
go-fakellm: rebuilt and restarted
weather: restarted (go-fakellm restarted)
rutis-host dev: services: forecast (weather), llm (llm)
```

`weather`'s binary did not change and is not swapped; it uses `llm`, so it restarts by the existing rule.

A fake written wrong:

| Case | Output |
| --- | --- |
| The directory is not a main package | `rutis-host: rutis.dev.json: row llm: ./dev/fakellm is not a Go main package (its package is fakellm); a fake needs package main and a main that calls rutis.Serve` |
| Two plugins in the binary | `rutis-host: rutis.dev.json: row llm: ./dev/fakes serves 2 plugins (fakellm, fakedb); give each its own directory` |
| Runtime name equals the project's | `rutis-host: rutis.dev.json: row llm: ./dev/weather would run as go-weather, as the project does; rename the directory` |
| The build fails | `fakellm: the build failed; the last build keeps running`, then the compiler's errors (at start there is no last build: `dev` exits with 1) |
| Swift: no such executable target | `rutis-host: rutis.dev.json: row llm: Package.swift has no executable target FakeLLM; add .executableTarget(name: "FakeLLM", path: "dev/FakeLLM", …)` |

In tests: TypeScript's and Python's `t.on` receive the events; a name that breaks the rules (a reserved `rutis/` name, say) fails with the same error as `ctx.on`.

None of this text uses the banned words of 0.18 §4.5.

## 9. Compatibility and migration

All additions; nothing goes into "Migrating to 0.19":

- the new command `rutis-host call` and `run`'s new option `--allow-call`;
- one more kind of line in `dev`'s output, starting with `rutis-host dev: services:`; existing lines unchanged;
- one more `name` form for `rutis.dev.json` rows, valid only in `dev` / a project's `check`;
- in Go and Swift projects, a binary whose content did not change no longer restarts its runtime;
- one more method, `on`, in the test tools.

0.18's socket answers one more request; a `run` without `--allow-call` behaves exactly as in 0.18.

## 10. Self-review: is it too heavy?

| Item | User | Already there | Result |
| --- | --- | --- | --- |
| `call` | Authors in 4 languages and the host author each wrote a caller plugin | The socket, `call_service` | Kept; no new channel |
| `run --allow-call` | The issue asks for `run` too; operators checking a deployment | 0.18's read-only default | Kept, off by default |
| Peer credential check | None: 0600 already admits only the same user | 0.18's socket | Removed |
| Table for non-JSON results | No user of its own | `Value::json()`, each value's `Display`, `describe()` | Removed; existing text used |
| A separate "command line" caller | None | "the host" | Removed |
| Dates, bytes etc. on the command line | No concrete user; tests can do it | — | Removed, out of scope |
| `dev` service line | Item 2 of the issue | `Loader::status()` | Kept; printed every time |
| Second binary | The Go author made a Node project for a fake | `GoDev`, `dev_rows`, `binaries` | Kept, dev only |
| Rebuilding by dependency | Changing the fake does not restart the plugin | Build caches, comparing build results | Changed to: rebuild all, restart only changed ones |
| Several plugins per directory, `./dir:<plugin>` | None | — | Removed; an error explains |
| `new` generating fakes | Only helps the template's example | — | Removed; templates belong to #671 |
| Test tool `on` | TS and Python authors added listener plugins | Go, Swift, Kotlin have it | Kept, same form |
| A "collect events" shortcut | A list and one line suffice | — | Removed |

## 11. Implementation

| Step | Content | Done when (the issue's "Done" list) |
| --- | --- | --- |
| C1 | `rutis-host call`, the socket's `call` request, `run --allow-call`, printing results | Every `call` row of §8 is locked word for word by a test (including `run` refusing, not JSON, and both locations of a throwing method); one call each into services provided by Node, Bun, Python, Go and Swift; rutis-host.md's command table and a new section, and each language guide's "Run in a local host" gain `call` (done 1) |
| C2 | `dev`'s service line | Output tests for start and after a reload; rutis-host.md's output table (done 2) |
| C3 | Directory rows in `rutis.dev.json`, the second binary, restarting only the runtimes whose binary changed, `check` building them | Go: tests for §8's start, changing the fake swapping only its binary, and the four wrong cases; Swift: the same cases, alongside the existing Swift `dev` tests (needs macOS and the Swift toolchain); the Go and Swift guides switch to fakes in their own language (done 3) |
| C4 | `on` in the TypeScript and Python test tools; the Python test tool reference table; `on` in each language guide | Tests in both languages: the plugin emitting, the test emitting, arguments in strict mode, disposing, removal on unload, return values in `serial`; the guide examples run by the docs example tests (done 4) |

C1 and C2 depend on 0.18 #678 (the socket, `Loader::status()`); C3 and C4 depend on nothing else and can go first.
