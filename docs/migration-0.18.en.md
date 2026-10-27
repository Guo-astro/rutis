# Migrating to 0.18

[中文](migration-0.18.md)

0.18 makes failures understandable ([diagnostics design](design-diagnostics-2026-10-26.en.md)). This page grows with the changes of 0.18; below is what is done so far.

## `rutis-host check`: cycles and services provided twice are errors (#676)

In 0.17.2 these two cases were warnings and left the exit status unchanged; from 0.18 they say `rutis-host: error: …` and exit with 1 (with or without `--strict`). A configuration that passed `check` in CI may now fail:

- **Rows whose `inject` wait for each other's services (a cycle)**: none of them starts.

  ```
  rutis-host: error: rows a, b wait for each other (a waits for bsvc from b, b waits for asvc from a): none of them starts; break the cycle in their inject
  ```

  Fix: remove from one row's `inject` a service it does not need; when both need each other, move the shared part into a third row both wait for.

- **Rows declaring the same service in one scope**: only the first to start provides it, the others fail.

  ```
  rutis-host: error: rows one, two all provide llm: only the first to start provides it, the others fail; keep one, or isolate the name in each row
  ```

  Fix: keep one row; when you need both, `isolate` the name: with `{ "llm": "<label>" }` the row providing it and the rows using it share a label, each group with its own copy (`{ "llm": true }` is a copy for that row alone, which other rows cannot use).

Before upgrading, run `rutis-host check` with 0.17.2: the warnings it writes for these two cases are the errors 0.18 reports.

## Errors (#677)

### Every language

- **Error type names and fields are unchanged.** `BindingError`, `AggregateError`, `SyncWaitCycle` and the others keep their names, and the error objects given to your code have no new fields.
- **Error texts change.** Code that matches errors by their text stops working; decide by the error code instead (next item). The texts use only what the developer wrote: row names, service names, method names, which argument, files and lines in the developer's code, and how to fix it; rutis, Cordis, binding, session, internal ids (`row:23.2`) and SDK internals no longer appear. The main changes:

  | 0.17 | 0.18 |
  | --- | --- |
  | `plugin failed: KeyError: 'region'` | `KeyError: 'region'` |
  | `weather row:23.2.tomorrow is not provided by the rutis host`, `unknown service method …` | `weather has no method tomorow (methods: today, tomorrow)` |
  | `2 arguments given, 1 expected`, `expected at most 1 arguments, received 2` | `weather.tomorrow takes 1 argument, got 2` |
  | `invalid type: string "x", expected u32` | `weather.tomorrow: argument 1 should be an integer, got "x"` |
  | `invalid type: null` (a missing argument) | `weather.tomorrow: argument 2 is missing` |
  | `argument 2 of weather.store: …`, `the result of weather.store: …` | `weather.store: argument 2 cannot be sent: …`, `weather.store: the result cannot be sent: …` |
  | `Out of range float values are not JSON compliant` (Python sending NaN) | `weather.store: argument 1 cannot be sent: NaN cannot cross processes` |
  | `Rust process disconnected`, `the rutis host disconnected` | `the host disconnected` |
  | `the session ended` | `the connection ended` (likewise `rutis-host`'s `link down (the connection ended)`) |
  | `2 listeners of weather/asked failed`; for one, `1 listeners …` | for one, `1 listener of weather/asked failed` |
  | `service "forecast" has been registered at <root>`, `service forecast is already provided by row a: row b cannot provide it too` | `forecast is already provided by a` (`by the host` for a service the host provides) |
  | TypeScript throwing a string: `ThrownValue: need 5 words` | `"need 5 words"` |
  | A default export not made with `definePlugin`: `cannot get property "use" without inject` | `the default export of src/index.ts is not a plugin made with definePlugin` |
  | A submodule failing at import, reported against the entry file | `cannot load src/util.ts: SyntaxError: … (src/util.ts:3:7)` (error name `LoadError`; Python too) |
  | `entry file is gone` | `the entry file src/index.ts is gone` |
  | `apply failed (…) and rollback failed (…)` | `the change failed (…), and the rows as they were did not start again either (…)` |
  | `and 17 plugin(s) waiting for them` | `and 5 rows waiting for them` (rows are counted) |

- **rutis's own `BindingError`s carry an error code.** The code is for programs to decide by and is not part of the text; the details are in `data`. Read them as for any error a plugin throws: JavaScript `error.code` / `error.data`, Python `error.code` / `error.data`, Go `*rutis.RemoteError`'s `Code` / `Data`, Swift and Kotlin errors' `code` / `data`, Rust `session::Error::code()` / `data()`:

  | code | When | `data` |
  | --- | --- | --- |
  | `not-declared` | a service used without being in `inject` | `{service}` |
  | `unavailable` | a service with no provider now | `{service, reason?}` |
  | `no-method` | a method that does not exist | `{service, method, methods}`; for a method on an object, `{object, method, methods}` |
  | `bad-argument` | arguments of the wrong number or type | number: `{service, method, expected, received}`; type or missing: `{service, method, argument, expected}` (`argument` counts from 1) |
  | `bad-value` | a value that cannot cross, or a message over the limit | `{part, argument?, limit?}` (`part` is `argument`, `result` or `error`) |
  | `peer-gone` | the far end's process exited or went away during the call | on the host's side `{runtime, reason}`; on a plugin's side `{reason}` |

  Protocol refusals (one end not keeping to the protocol) have no code.

  ```ts
  // 0.17
  if (/is not available/.test(error.message)) { … }
  // 0.18
  if (error.code === 'unavailable' || error.code === 'not-declared') { … }
  ```

  ```python
  # 0.18
  except BindingError as error:
      if error.code == "no-method": ...
  ```

  ```go
  // 0.18
  var remote *rutis.RemoteError
  if errors.As(err, &remote) && remote.Code == "peer-gone" { … }
  ```

- **Printed errors say who, doing what, and where.** A failed row as `rutis-host run` / `dev` print it first says which step failed, then, indented, the row it came from and the location the plugin's language gave, and across languages the caller's location too:

  ```text
  weather: failed in apply: KeyError: 'region'
      in weather:
      File "weather.py", line 12, in apply
  chat: call to forecast.tomorrow failed: KeyError: 'region'
      in weather:
      File "weather.py", line 31, in tomorrow
      called from:
      at ask (src/chat.ts:14:36)
  ```

  Scripts parsing this output need the new format. A stack sent across processes holds only the plugin's own frames (no SDK or Cordis frames).

### Node, Bun

- **For an error received from another language, `error.stack` is the caller's own stack** (it used to be replaced by the far end's). rutis prints the far end's stack; when you throw the error on, the SDK carries the far end's error with it unchanged. Code relying on the far end's frames in `error.stack` should read rutis's output instead.
- **The testing tool (`@arcships/rutis/testing`) gives the same texts, codes and `data` as the real runtime**: not in `inject`, service not there, no such method, a value that cannot be sent (arguments count from 1; it used to be `svc.m argument 0: …`).

### Python

- A host service's methods show as `weather.today` in tracebacks, no longer `HostProxy.__getattr__.<locals>.call`; a call that returns no value gives `None`, and `UNDEFINED` no longer shows.
- When the host disconnects, calls raise `ConnectionError` with code `peer-gone`.
- `str(SyncWaitCycle)` is the message alone, with no `SyncWaitCycle: ` before it (as for the other errors).
- The `cause` of `raise … from cause` crosses as the error graph's `cause` (as a JavaScript error's `cause` does); an error left unhandled while loading (`UnhandledError`) has the original error as its `cause`, so that error's own `cause` and an `AggregateError`'s `errors` cross too.
- An error while importing a plugin's module (or a module it imports) raises `LoadError` (`from rutis import LoadError`, a subclass of `Exception`), naming the file and line that raised; a missing entry module itself is still the original `ModuleNotFoundError`.

### Go

- `Parallel` returns `*rutis.AggregateError` on a real host (before, only in the testing tool): `errors.As` holds and `Data` is not empty.
- The error types a developer meets print as `*rutis.RemoteError`, `*rutis.AggregateError` and so on, no longer `*peer.RemoteError`.
- Too many arguments is `bad-argument`; missing arguments are still filled with zero values.

### Kotlin

- Not in `inject` and service not there throw the new `NotDeclaredException` (a subclass of `IllegalArgumentException`) and `UnavailableException` (a subclass of `IllegalStateException`); a host that went away throws `HostGoneException` (a subclass of `IOException`). They carry a code and `data` (`code`, `data` properties); code catching the former types works as before.

### Rust plugins

- Compile errors for common mistakes speak plainly: a service method parameter that cannot cross languages, an error type without `Display`, a service type that is not `Send + Sync`, an event type that does not implement `rutis::Event`, a `type Deps` member that is not `Arc<T>` and so on now say on their first line what is wrong and how to fix it, without macro or trait internals. No code needs changing.
- `ctx.on(async |e, _| …)` still gives E0282 (argument types cannot be inferred); write `async |event: Arc<Ping>, _: Call<Ping>| …` or `|event, _| async move { … }`, as the documentation of `Listener` and `Ctx::on` shows.

### Rust hosts

- **`ErrorKind::PluginFailed` shows no `plugin failed: ` prefix.**
- **A plugin's error is no longer a string.** When a row fails or a call to a plugin's service fails, `rutis_bridge::thrown(&error)` gives the error the plugin threw (`name`, `message`, `code()`, `data()`, `stack()`, `site()`: which row, doing what). To print it, `rutis_bridge::report::report(&error)` writes it as `rutis-host` does. The loader's `Failure` and `ReconcileReport` are unchanged; read the structure from the row's state (the snapshot error in `EntryInfo`).
- **`rutis_bridge::session::Error` has a new variant, `Binding { code, message, data }`.** rutis uses it when it refuses a call: a method that does not exist, wrong arguments, a service that is not there (with a code), and protocol refusals (`code` is `None`). `Error::Value` now means only a value that cannot cross. Code that `match`es every variant of `session::Error` needs the new arm; to tell kinds apart use `Error::code()` (`bad-value` for `Value`, `peer-gone` for `Transport`):

  | 0.17 | 0.18 |
  | --- | --- |
  | `Err(Error::Value(m)) if m.starts_with("unknown service method")` | `Err(e) if e.code() == Some("no-method")` |
  | `Err(Error::Value(m)) if m.contains("is not available")` | `Err(e) if e.code() == Some("unavailable")` |
  | `Err(Error::Value(m))` (a value that cannot cross) | unchanged; or `e.code() == Some("bad-value")` |

- `SyncWaitCycle` shows no `synchronous wait cycle: ` prefix; it reads `weather.today would wait on itself: chat → weather → chat (synchronous calls)`.
- A row that fails because its service already has a provider fails with a plugin error named `ProvideError` (it used to be a string). `BindingError`, `LoadError`, `ProvideError` and thrown strings or objects show their message alone, with no `<name>: ` before it.
- The header of type declarations `build::Exports` generates is now `// Generated from the service declarations; do not edit.`; checks that compare generated files need one regeneration.

### Protocol

- Error nodes have two more optional fields: `site` (the row it came from, doing what) and `rethrown` (the frames of callers that threw it on). An older end ignores them; the protocol version is unchanged and 0.17 ends meet as before.

## Node, Bun and Python runtimes: errors nobody handled are handled differently (#688)

In 0.17 a promise rejection a Node or Bun plugin left unhandled (or an exception thrown in a timer) ended its runtime's process, and every row of that runtime stopped and waited; an exception nobody retrieved from a Python task was only logged by asyncio, and the row ran on. From 0.18:

- The process goes on. Where the row that produced the error is traced (Node under `rutis-host dev`; in Python, tasks and callbacks that run in a row), that row fails, saying what and where; the other rows of the runtime run on.

  ```
  weather: failed: unhandled rejection: Error: no region
      at Timeout._onTimeout (file:///…/weather.mjs:12:9)
  ```

- Where it is not (Node under `rutis-host run`, Bun, Python code that runs in no row), the runtime writes a line saying the source is unknown, and no row fails:

  ```
  runtime node: an unhandled error from an unknown row: Error: no region
      at …
  ```

  Only the plugin's own frames are listed under it; when none of the frames is the plugin's, the whole stack is, as it is then the only clue to the source.

- A plugin calling `process.exit` / `os._exit` still ends the process, and the runtime first writes `runtime node: row weather called process.exit(1)`.

What to change:

- **Python**: a row that ran on after one of its tasks failed now fails (the host restarts it, and the restart fails). Handle or retrieve the exceptions of the tasks you start (`await` them, or read `exception()` in an `add_done_callback`).
- **Node, Bun**: a project that recovered by letting the runtime exit on an error and having the host restart it no longer sees the runtime exit: `restart: "on-failure"` does not restart the runtime for it, and a host with `restart: "exit"` no longer stops with status 3 for it. To recover after an error, handle the error in the plugin; to end the process, call `process.exit` explicitly.

See [Plugins in one runtime share its process](guide/plugin-api.en.md#plugins-in-one-runtime-share-its-process) in the plugin API guide.

## Two entries on one listener accepting one token: an error at start (#702)

When a listening peer row and a runtime that connects in (`listen` + `instances` of `runtimes.remote`) would accept the same token on one listener (say neither has its own and both fall back to `RUTIS_TOKEN`), 0.17 started, but refused each connection with 403 (it could not tell which entry it was for), and the runtime retried every 30 seconds. From 0.18 `run`, `check` and a configuration reload fail before starting. The same holds for any two entries on one listener: two runtimes that connect in (refused with 403 in 0.17 too), two listening peer rows (in 0.17 every connection reached the later row, and the earlier one never connected).

```
rutis-host: listener public: link row main and remote runtime phone both accept RUTIS_TOKEN, so a connection presenting it cannot be told apart: set RUTIS_TOKEN_MAIN or RUTIS_TOKEN_PHONE to a token of its own
```

Fix: as the error says, set `RUTIS_TOKEN_<name>` for one of them (and have the runtime that connects in present the new token). Such a configuration could not connect in 0.17 either; once fixed, it does.

## Objects with methods go to every language (#687)

In 0.17 objects with methods (class instances and the like) went only to Rust; handed to Node, Bun, Python, Go, Swift or Kotlin, the call failed (`argument 1 of warmer.warm: an object with methods cannot cross to py; pass plain data or a function instead`). From 0.18 they go to every language by reference: the far end gets a proxy whose methods call the original object ([design](design-object-refs-2026-10-27.en.md); the rules are in the Plugin API's [Objects with methods](guide/plugin-api.en.md#objects-with-methods)). What goes by reference and what is copied has not changed: JS class instances and plain objects with function-valued properties, Python instances other than dataclasses, Go's `rutis.Ref(v)`, and Swift and Kotlin `@RutisService` instances. What Rust hosts and Cordis bindings receive does not change.

What to change:

- **Code that relied on the failure**: code that passed an object, and on failure passed data instead (turning it into a dict or plain object in a `catch` and calling again), no longer reaches the `catch`: the far end gets a proxy, whose methods are all asynchronous calls and which has no data fields. When the far end wants data, pass data.
- **The receiver's parameter types**: in Go, Swift and Kotlin, a parameter of a data type that receives an object fails the call with `bad-argument` (before, the sender's `bad-value`), and the error says how to receive an object: in Go a struct of function fields or `any`, in Swift a `@RutisClient` protocol or `Any`, in Kotlin a `@RutisClient` interface or `Any?`. A Rust `#[rutis::service]` parameter takes no objects and fails with `bad-argument`; pass functions instead.
- **Upgrade both ends**: runtimes older than 0.18 do not take objects. When a 0.18 host would forward an object to one, the call fails with `bad-value`, and the session goes on:

  ```
  chat: warmer.warm: argument 1 cannot be sent: an object from node cannot cross to py: py does not take objects (older than rutis 0.18); upgrade it
  ```

  Connected directly to an old end, the SDK refuses by itself: `… cannot cross to main; pass plain data or a function instead, or upgrade main to rutis 0.18`. A 0.17.2 host does not know that 0.18 runtimes take objects, so objects between runtimes under it still fail with 0.17.2's text; upgrade the host.

### `undefined`

`undefined` is kept only as a whole argument or a whole result; inside arrays, objects, Maps and Sets it is `null` at any depth (`None` in Python), in every language. Before, `undefined` inside an array passed as an argument was kept as a special value (a placeholder in Python, `undefined` in JS), against the docs. JS code that checks whether an array element is `undefined` should check for `null` as well (`x == null`).

### Swift

- A parameter, or the result of a `@RutisClient` method, typed with a `@RutisClient` protocol (`any Cache`) receives an object as that protocol's proxy; typed `Any`, as a `Rutis.Client` (`call(_:_:as:)`, `call(_:values:as:)`). Objects inside arrays, dictionaries and event arguments arrive through `Any` as `Rutis.Client`; `[any Cache]` is not supported.
- Only a `@RutisClient` protocol declared at file level receives objects; a protocol nested in another type still works with `ctx.use`, but as a parameter type it takes no object (`bad-argument`). To receive objects, move the protocol to file level.
- `RutisProxy` gains `rutisClient`: a proxy passed on sends the original object. Code implementing `RutisProxy` itself (not through `@RutisClient`) must add it.

### Kotlin

- New type `arcships.rutis.RemoteObject`: a parameter of type `Any?` receives an object as one; `call(method, vararg args)` suspends, `callBlocking(method, vararg args)` blocks, and `release()` gives the reference back now. It is the only new type plugin code uses for objects in 0.18 (`arcships.rutis.ObjectProxy` is public too, but only the proxies KSP generates implement it; plugin code does not use it, as with `ServiceHandle`).
- A parameter, or the result of a `@RutisClient` method, typed with a `@RutisClient` interface receives an object as the KSP-generated proxy.
- Code that treated an `Any?` parameter as receiving only data (`Map`, `List`, primitives) may now receive a `RemoteObject` too.
