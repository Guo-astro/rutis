# Migrating to 0.18

[中文](migration-0.18.md)

0.18 makes failures understandable ([diagnostics design](design-diagnostics-2026-10-26.en.md)). This page grows with the changes of 0.18; below is what is done so far.

## Errors (#677)

### Every language

- **Error texts change.** A failed row shows the plugin's own words, with no `plugin failed: ` before them; rutis's own messages are rewritten as in §4.5 of the diagnostics design. Code that matches errors by their text should decide by error code instead.
- **rutis's own `BindingError`s carry an error code.** The type name is unchanged; `code` and `data` are new (read as for any error a plugin throws: JavaScript `error.code`, Python `error.code`, Go `RemoteError.Code`, …):

  | code | When | `data` |
  | --- | --- | --- |
  | `not-declared` | a service used without being in `inject` | `{service}` |
  | `unavailable` | a service with no provider now | `{service, reason}` |
  | `no-method` | a method that does not exist | `{service, method, methods}` |
  | `bad-argument` | arguments of the wrong number or type | `{expected, received}` and the like |
  | `bad-value` | a value that cannot cross, or a message over the limit | — |
  | `peer-gone` | the far end's runtime ended or went away during the call | — |

  `not-declared` and the kinds each language's SDK raises itself follow later; protocol refusals (one end not keeping to the protocol) have no code.

### Rust hosts

- **`ErrorKind::PluginFailed` shows no `plugin failed: ` prefix.**
- **A plugin's error is no longer a string.** When a row fails or a call to a plugin's service fails, `rutis_bridge::thrown(&error)` gives the error the plugin threw (`name`, `message`, `code()`, `data()`, `stack()`, `site()`: which row, doing what). The loader's `Failure` and `ReconcileReport` are unchanged.
- **`rutis_bridge::session::Error` has a new variant, `Binding { code, message, data }`.** rutis uses it when it refuses a call: a method that does not exist, wrong arguments, a service that is not there (with a code), and protocol refusals (`code` is `None`). `Error::Value` now means only a value that cannot cross. Code that `match`es every variant of `session::Error` needs the new arm; to tell kinds apart use `Error::code()` (`bad-value` for `Value`, `peer-gone` for `Transport`):

  | 0.17 | 0.18 |
  | --- | --- |
  | `Err(Error::Value(m)) if m.starts_with("unknown service method")` | `Err(e) if e.code() == Some("no-method")` |
  | `Err(Error::Value(m)) if m.contains("is not available")` | `Err(e) if e.code() == Some("unavailable")` |
  | `Err(Error::Value(m))` (a value that cannot cross) | unchanged; or `e.code() == Some("bad-value")` |
