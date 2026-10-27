# Migrating to 0.18

[中文](migration-0.18.md)

0.18 makes failures understandable ([diagnostics design](design-diagnostics-2026-10-26.en.md)). This page has a section per change; each section is completed by its 0.18 issue.

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
