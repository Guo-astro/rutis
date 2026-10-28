# Trying and testing plugins: `rutis-host call`, `dev` lists services, test tools listen to events

[中文](design-try-and-test-plugins-2026-10-27.md)

Status: final, not implemented. Date: 2026-10-27. Baseline: `main` `103c0d22` (after 0.17.2). Scheduled for: 0.19.
Issue: [#670](https://github.com/arcships/rutis/issues/670) (A5, A6 and H5 of [#663](https://github.com/arcships/rutis/issues/663)).
Relation to 0.18: built on [diagnostics for 0.18](design-diagnostics-2026-10-26.en.md), without repeating or changing it: `call` uses the local socket and the way of finding it from its §5.3, the services `dev` lists are the `services` of its §5.1 `Loader::status()`, and error text follows its §4 (`site`, error codes, the banned words of §4.5). Depends on [#678](https://github.com/arcships/rutis/issues/678) being merged first.
Relation to other 0.19 designs: generating strict fakes from the host's `#[rutis::service]` and putting them into the test tools belongs to [#684](https://github.com/arcships/rutis/issues/684); this design only covers fakes plugin authors write themselves and the test tools' `on`. SDK default entry points belong to [#692](https://github.com/arcships/rutis/issues/692): this design adds no names to an SDK root; `on` goes on the object the test tool loads. Limiting which services a plugin may use belongs to [#683](https://github.com/arcships/rutis/issues/683); it limits plugins, and `call` is the host's own call.

Out of scope: `call` on Windows (0.18's `status` is not provided there either); writing values other than JSON on the command line (integers beyond 64 bits, instants, dates, bytes, Map, Set, Decimal, UUID, functions); calling services inside instances; emitting events from the command line (`rutis-host emit`); `call` starting a host (it only connects to a running one); `run` printing a service table (use `rutis-host status`); `new` generating fakes (template and guide examples belong to [#671](https://github.com/arcships/rutis/issues/671)); a "collect the received events" shortcut in the test tools (`on` plus a list is enough); an install hint when the Node runtime lacks `@arcships/rutis` (error text, part of 0.18 #677's rewrite list). Removed in review (no user): a peer credential check for `call`, a dedicated table of how non-JSON result values print, and not printing a service line "identical to the last one". Cut in review (2026-10-28; what was merged is removed by a deletion PR): `run --allow-call` (the same user can already change the configuration and end the process, so the switch stops no one; `call` is accepted under both `run` and `dev`, and who can connect is still decided by the 0600 socket); fakes in the plugin's own language (Go directory rows, the second binary, restarting only the runtimes whose binary changed: fakes in another language already work today with one row in `rutis.dev.json`, which does not justify another form).

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
| Running a fake with the plugin | `dev` already runs rows of several languages in one project: a row of `rutis.dev.json` can be `./dev/fake-llm.ts` or `py:fake_llm`, with its runtime's settings |
| Text for each kind of value in a result | `Value::json()`; each value's `Display`; `describe()` from error text |
| Test tools listening to events | Go `loaded.On(name, fn)`, Swift `loaded.on(name) { … }`, Kotlin `loaded.on(name) { … }` exist, with one rule: listen as the host does, at the root scope, returning a disposer |

So all four fill gaps in what exists: no new channel, no new state data, no change to the kernel or the loader.

## 3. Decisions

| # | Decision | Section |
| --- | --- | --- |
| T1 | `rutis-host call <service>.<method> [args…]`: connects to the same socket 0.18's `status` uses, which answers one more request, `call`; found as `status` finds it; Unix only | 4 |
| T2 | `dev` and `run` both accept `call`; who can connect follows 0.18's socket (0600), with no extra check | 4 |
| T3 | Each argument is one JSON value, and argument types JSON cannot express are not supported; the result is printed as JSON, values JSON lacks in the text they already have | 4 |
| T4 | A call made by `call` has "the host" as its caller (as every call not made from a row in 0.18); the error's `site` is, as in 0.18, the called row | 4 |
| T5 | `dev` prints one service line after starting and after each change has settled, from `Loader::status().services` | 5 |
| T6 | Fakes in any language: one more row in `rutis.dev.json` (such as `./dev/fake-llm.ts`), no new form; `new` does not generate fakes | 6 |
| T7 | (Cut in review: restarting only the Go runtimes whose binary changed went with fakes in the plugin's own language; see Out of scope at the top) | — |
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

- 0.18's socket answers one more request: `call`, with the service name, the method name and the argument list. The format on the socket is rutis-host's own; `call` and `status` are the same version of rutis-host, and nothing is promised to others.
- The host runs `call_service::<Value>(&root, service, method, args)`, as a Rust host's call by name does: the service the root scope sees by that name, provided by a row of any language, a remote runtime or a node; sync methods on a blocking thread.
- No time limit (0.18 §6.1: how long to wait belongs to the caller). `rutis-host call` waits for the result; Ctrl-C ends the `call` process and closes the socket, the host drops the call's future, and cancellation reaches the other side as it already does (a TypeScript method is not interrupted, as in 0.18 §6.1).
- One call per connection; several `call`s at once do not affect each other.

### 4.3 Permissions

| | `dev` | `run` |
| --- | --- | --- |
| `status` | answered | answered |
| `call` | accepted | accepted |

- **Why `run` accepts it too, with no switch**: only the same user can connect to the socket, and that user can already change the configuration and end the process; a switch stops no one and only gives operators one more option to remember.
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

## 6. Fakes in any language

No new form. `dev` already runs rows of several languages in one project, so a fake can be written in any of them: add a row to `rutis.dev.json` with its runtime's settings, such as `{ "id": "llm", "name": "./dev/fake-llm.ts" }` (with `runtimes.node`) or `{ "id": "llm", "name": "py:fake_llm" }` (with `runtimes.py`) in a Go project. TypeScript and Python projects keep writing them in their own language. The Go guide says so.

`new` does not generate fakes: it cannot know which services a plugin will use; whether the template ships one is a template and guide question (#671).

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
- Each language guide's "Run in a local host" section adds a `rutis-host call` step; the Go guide says fakes can be written in any language (§6).

## 8. What developers see

A plugin author in the Go project `weather`:

```console
$ rutis-host dev
weather: building
weather: built in 1.4s
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
| Windows | `rutis-host call is not available on Windows`, 1 |
| Ctrl-C while waiting | nothing printed; the host cancels the call; 130 |
| `rutis-host status` meanwhile | `host → forecast.today  12s` |

In tests: TypeScript's and Python's `t.on` receive the events; a name that breaks the rules (a reserved `rutis/` name, say) fails with the same error as `ctx.on`.

None of this text uses the banned words of 0.18 §4.5.

## 9. Compatibility and migration

All additions; nothing goes into "Migrating to 0.19":

- the new command `rutis-host call`;
- one more kind of line in `dev`'s output, starting with `rutis-host dev: services:`; existing lines unchanged;
- one more method, `on`, in the test tools.

0.18's socket answers one more request; without `call` everything behaves exactly as in 0.18.


## 10. Implementation

| Step | Content | Done when (the issue's "Done" list) |
| --- | --- | --- |
| C1 | `rutis-host call`, the socket's `call` request, printing results | Every `call` row of §8 is locked word for word by a test (including not JSON, and both locations of a throwing method); one call each into services provided by Node, Bun, Python, Go and Swift; rutis-host.md's command table and a new section, and each language guide's "Run in a local host" gain `call` (done 1) |
| C2 | `dev`'s service line | Output tests for start and after a reload; rutis-host.md's output table (done 2) |
| C3 | The Go guide says fakes can be written in any language | The "Run in a local host" section of the Go guide (both languages) (done 3); the implementation of fakes in the plugin's own language (#797) was cut in review and is removed by a deletion PR |
| C4 | `on` in the TypeScript and Python test tools; the Python test tool reference table; `on` in each language guide | Tests in both languages: the plugin emitting, the test emitting, arguments in strict mode, disposing, removal on unload, return values in `serial`; the guide examples run by the docs example tests (done 4) |

C1 and C2 depend on 0.18 #678 (the socket, `Loader::status()`); C3 and C4 depend on nothing else and can go first. `run --allow-call` (part of C1) was cut in review and is removed by a deletion PR.
