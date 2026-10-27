# Config schemas for 0.18: fill defaults, validate, then `apply`

[中文](design-config-schema-2026-10-27.md)

Status: final, not implemented. Date: 2026-10-27. Baseline: `main` `85678323` (after 0.17.2). Scheduled for: 0.18 (breaking, covered in "Migrating to 0.18").
Issue: [#669](https://github.com/arcships/rutis/issues/669). Consistent with [Diagnostics for 0.18](design-diagnostics-2026-10-26.en.md): errors carry an error code per its §4.3, are recorded on the row whose config is wrong per §4.7, and print as in its §8.1 fault 12.

Out of scope: finer constraints such as `pattern`, `format` and string lengths (anything outside the list in §3.1); type coercion (`"5"` does not become `5`); generating config forms or types (#684); Cordis rows are validated by Cordis itself (§2.3), and rutis does not validate them again.

## 1. Where things stand

The plugin API guide has long said "JSON Schema; the host validates and shows the config with it". In practice it is only shown (#714 corrected the guides to say so):

| Language | Today |
| --- | --- |
| TypeScript (Node, Bun), Python | The config goes to `apply` as is: no defaults, neither `required` nor types are looked at, a bare string `"Oslo"` is accepted |
| Go | `json.Unmarshal` into type `C`: type errors name the field, extra keys (`cty`) are silently ignored; the generated schema lists every field without `omitempty` as `required`, but it is never checked |
| Swift, Kotlin | Decoded into a `Decodable` / `@Serializable` type; errors are the decoder's own text |
| Rust (loader rows) | `serde` decoding; errors appear only in `ReconcileReport`: `config validation failed: ["invalid type: integer 42, expected a string"]`, with no field path; extra keys ignored |
| Cordis rows | Cordis fills defaults and validates with the plugin's `Config` (a Standard Schema such as schemastery), as in Cordis |
| `rutis-host check` | Prints the schema only |

## 2. Decisions

| # | Decision | Section |
| --- | --- | --- |
| C1 | Each language's SDK does it when it loads a row, before calling `apply` (the runtime and the testing tool run the same code); `check` does the same with rutis's Rust implementation; the loader does not do it again for rows of other runtimes | 2.1 |
| C2 | One JSON Schema subset of 14 keywords; other keywords are ignored, with a `check` warning | 3.1 |
| C3 | Extra keys: a warning, and passed to `apply` as they are; an error when the schema says `additionalProperties: false`; `check --strict` treats the warning as an error | 3.2 |
| C4 | Typed configs (Go, Swift, Kotlin, Rust) get their schema from the type, stating only the rules the language's own decoder already has | 3.3 |
| C5 | Defaults are filled level by level, "an object that is there gets its properties filled"; with no config the root is `{}` | 4 |
| C6 | All problems are listed at once, one per line: `<row>: config: <path> should be …, got …`; error code `bad-config` | 5 |
| C7 | With a wrong config the row fails, and rows depending on it show that they wait for it; `dev`, `run`, testing tools and `check` print the same | 5 |
| C8 | One shared case list `specs/config-schema/cases.json`, run case by case by every language and by `check` | 7 |

### 2.1 Why each language's SDK does it

- **Testing tools must behave as the real runtime.** A testing tool has no loader, only the SDK; the code with which the SDK loads a row is shared by the runtime and the testing tool. In the SDK, the rules a test exercised are the runtime's rules. In the loader, every language's testing tool would need its own copy anyway, split from the runtime.
- **Remote runtimes behave the same without extra work.** The config is checked where the row runs, whether the runtime is on this machine, on another one or in an app.
- **Typed languages decode here anyway.** Go, Swift, Kotlin and Rust decode the config into a type before `apply`; filling defaults and validating against the schema first makes the error text the same as in the other languages.
- **One set of rules, kept identical by the case list** (§7) rather than by one piece of code: every language needs its own copy regardless (for its testing tool), and the case list makes the 7 copies and `check` agree word for word.
- Runtimes built with an SDK older than 0.18 do not check; `check` checks their rows all the same.

### 2.2 The steps every row goes through

```text
the row's config (absent or null counts as none)
  → fill defaults (§4)
  → validate against the schema (§3.1); on errors → the row fails (§5), apply is not called
  → typed languages: decode into the type; an error the decoder still reports is also bad-config, with a path when it has one
  → apply(ctx, config)        // a copy with defaults filled; extra keys kept as they are
```

A config change (the row restarts) goes through the same steps. A definition a plugin hands out (`ctx.plugin(def.at(&ctx, config))`) loads in the runtime where it was defined and goes through them too; a failure is recorded on that copy, and the caller of `ctx.plugin` gets the same error.

### 2.3 Cordis rows

Cordis rows (Cordis plugins that export `Config`) follow Cordis's rules: Cordis validates with `Config` when it loads them, and rutis does not validate them again against the converted JSON Schema (the conversion loses information such as `transform`, so the two would disagree). rutis only prints Cordis's `ValidationError` in the format of §5 (the path taken from the issue's `path`). `check` does not check the config values of Cordis rows; it prints their schema, as today.

`definePlugin({ config })` takes JSON Schema only. Given a Standard Schema object such as a schemastery one, the definition fails at once: `config of definePlugin is a JSON Schema; a schemastery Schema belongs in a Cordis plugin's exported Config`.

## 3. Rules

### 3.1 Supported keywords

| Keyword | How it is checked |
| --- | --- |
| `type` | `string`, `number`, `integer`, `boolean`, `object`, `array`, `null`, or an array of them (`["object", "null"]`). `integer` accepts `1.0` |
| `properties` | Each property is checked |
| `required` | A missing one is an error |
| `additionalProperties` | `false`: extra keys are errors; a schema: extra keys are checked against it (a dictionary) |
| `items` | Every item of an array is checked against it (one schema) |
| `enum`, `const` | Compared by JSON equality |
| `minimum`, `maximum` | Bounds of a number (inclusive) |
| `default` | Fills defaults (§4) |
| `anyOf`, `oneOf` | Matching one branch is enough (`oneOf` is checked as `anyOf`); defaults inside branches are not filled |
| `allOf` | Every one must match |
| `$ref` | Only to `#/$defs/…` and `#/definitions/…` in the same document (enough for schemars and hand-written schemas) |

- Keywords that only describe are not checked and not warned about: `$schema`, `$id`, `title`, `description`, `examples`, `deprecated`, `readOnly`, `writeOnly`, and those starting with `x-` (such as `x-volatile`).
- Other keywords (`pattern`, `minLength`, `format`, `prefixItems`, `if`, …) are not checked. `check` warns once per schema: `weather: config schema: pattern is not checked by rutis; check it in the plugin`; the runtime does not warn.
- No coercion: `"5"` does not match `number`.

### 3.2 Extra keys

- An object with `properties` and no `additionalProperties`: an extra key is a warning, and its value goes to `apply` as it is (what JSON Schema means; old configs keep starting). A spelling close to a known key gets a suggestion.
- The warning is written to standard error by whoever checks (the runtime, the testing tool, `check`, once each): `weather: config: cty is not a config key (did you mean city?); it is ignored`.
- `additionalProperties: false`: the same text as an error; the row fails.
- `check --strict`: warnings count as errors, exit status 1 (as `--strict` treats its other warnings).

### 3.3 How typed configs map

A generated schema states only the rules the language's own decoder already has; developers write no extra annotations for rutis:

| Language | Required | Defaults | Extra keys |
| --- | --- | --- | --- |
| Go (type `C`) | No `required` is generated: a field not written is its zero value, as with `encoding/json` and as the Go plugin guide says (**change**: fields without `omitempty` used to be listed as `required`); use a pointer to tell "not written" apart | Zero values; `default` in a hand-written `Schema` is filled as usual | Warning |
| Swift (`@RutisConfig`) | Optional types (`T?`) and `var`s with a literal initial value are not required; the rest are | The literal of `var units = "metric"` becomes the `default` (strings, numbers, booleans, and arrays and dictionaries of them); a non-literal initial value is not a default, and the field is required | Warning |
| Kotlin (`@Serializable`) | Not required when it has a default argument (`isElementOptional`) or is nullable | Filled by kotlinx when decoding (`SerialDescriptor` cannot see default values, so the schema has none and `check` does not show them) | Warning |
| Rust (`serde` + `schemars`) | schemars's existing rules: `Option<T>` and `#[serde(default)]` are not required | The `default` schemars writes into the schema; filled into the JSON first, so `serde(default)` still works and both are the same value | Warning; `#[serde(deny_unknown_fields)]` generates `additionalProperties: false`, an error |
| Rust (definitions made in Rust, no schema) | `serde` decoding | `serde(default)` | `serde`'s rules |

Definitions made in Rust (`Definition`, whose config type only needs `DeserializeOwned`) have no schema and are only decoded; decoding errors are written in the format of §5 too, with the path recorded while decoding (`serde_path_to_error`).

## 4. Defaults

- **No config** (no `config`, or `null`): the root schema's `default` when it has one; `{}` when the root is an object; otherwise "no value" is checked (an error when the root does not allow it). A plugin without a schema is unchanged: it gets what it is given.
- **Level by level**: for an object that is there (including the initial `{}`), each missing property with a `default` is filled; then filling continues into the filled or existing child objects.
- **No objects out of nothing**: a missing child object without its own `default` is not filled, and `apply` does not see it; to always have the nested defaults, give the child object `default: {}`. This is ajv's `useDefaults` rule, the one JavaScript developers know best.
- What is filled is a copy: a plugin changing it does not affect the next load.
- Validation comes after filling: a `default` that does not match its schema is the plugin's error; the row fails, and `check` reports it: `weather: config schema: the default of units ("kelvin") should be "metric" or "imperial"`.
- Code that checks "is this key in the config" now sees the defaults filled: the behaviour this design wants, stated in the migration guide.

## 5. Errors

### 5.1 Text

One line per problem, all problems at once (at most 20, the rest as `and N more`):

```text
<row>: config: <path> <reason>
```

| Problem | Text |
| --- | --- |
| Wrong type | `city should be a string, got 42` |
| Wrong root type | `config should be an object, got "Oslo"` |
| Missing required | `city is required` |
| Not in `enum` | `units should be "metric" or "imperial", got "kelvin"`; three or more: `should be one of "a", "b", "c"` |
| `const` | `mode should be "fast", got "slow"` |
| Bounds | `days should be at least 1, got 0`; `days should be at most 14, got 30` |
| Not an integer | `days should be an integer, got 1.5` |
| `anyOf` / `oneOf` | `proxy should be a string or an object, got 3` |
| Key not allowed | `cty is not a config key (did you mean city?)` |
| Decoder error (§2.2) | `home.lat: <the decoder's words, without internal type names>` |

- Paths: `home.lat`, `cities[2]`; keys that are not identifiers are quoted: `headers["x-api"]`.
- `got` is JSON; past 40 characters it is the type (`an object`, `an array of 12`).
- None of the forbidden words of the diagnostics design §4.5 appear (serde, `ValidationError`, internal type names).

### 5.2 Structure

- The error is a `BindingError` with code `bad-config` (one more row in the table of the diagnostics design §4.3) and `data` `{ issues: [{ path: ["home", "lat"], message: "home.lat should be a number, got \"north\"" }] }`. Its `site` is the row with `doing: "apply"` (diagnostics design §4.1); when rutis prints a `bad-config` error it writes `config:`, not `failed in apply:`.
- Rust: a config error of a loader row stays `ErrorKind::Validation { issues }`, with each line above as an issue, displayed as `config: …` (no more `config validation failed: [...]`); the text of `Failure` in `ReconcileReport` changes with it, the types do not.

### 5.3 Which row, and what then

- Recorded on the row whose config is wrong (diagnostics design §4.7): the row fails, `apply` is not called; rows depending on it show `chat: waiting for weather (row weather failed: config: units should be …)`, not an error of the caller.
- Once the config is fixed, the row reloads as usual (automatically when `dev` watches the config file; under `run` by the existing ways to reload).
- `dev` and `run` follow the same rules; they differ only in the output prefix of the diagnostics design §7.

## 6. `rutis-host check`

- For every row, with the schema it already prints, it fills defaults and validates by the same rules (the Rust implementation shared with loader rows), without running plugins.
- Errors: written in the existing format `<row> (<plugin>): cannot run: config: …`, one line per problem; exit status 1. Warnings (extra keys, keywords not checked) go to standard error with exit status 0; 1 with `--strict`.
- Rows whose schema is unknown at check time (remote runtimes not connected, `peer:` rows) say `config not checked: its schema is known only when it runs`, which is not a warning. Cordis rows: see §2.3.
- `check` prints the same text as the runtime, word for word (locked by the same case list).

## 7. The case list

`specs/config-schema/cases.json`: each case is `{ name, schema, config, then: { config } | { errors: [...] }, warnings: [...] }`, with `errors` and `warnings` in the full text of §5.1. Every SDK's tests (Node, Bun, Python, Go, Swift, Kotlin), rutis-loader and `check` run every case and compare word for word. The cases include at least:

| # | Case |
| --- | --- |
| 1 | No config, object root: `{}` plus defaults |
| 2 | `null` is the same as none |
| 3 | A bare string as config: `config should be an object, got "Oslo"` |
| 4 | Missing `required` |
| 5 | Wrong types (one each for string, number, integer, boolean, array, object) |
| 6 | `enum` with two values and with three; `const` |
| 7 | `minimum`, `maximum` at the bounds (equal passes) |
| 8 | Nested object present: its defaults filled; absent: no object made; child `default: {}`: made and filled |
| 9 | An array item wrong: path `cities[2]` |
| 10 | A dictionary (`additionalProperties` as a schema) |
| 11 | Extra keys: warning, kept as they are, a suggestion |
| 12 | `additionalProperties: false`: error |
| 13 | `anyOf` / `oneOf`; `["string", "null"]` |
| 14 | `$ref` to `$defs` and to `definitions` |
| 15 | A keyword not checked (`pattern`): the value passes |
| 16 | Several problems at once; more than 20 truncated |
| 17 | A `default` that does not match its schema |
| 18 | Paths with non-identifier keys; a long `got` |
| 19 | Defaults are copies: a plugin changing one does not affect the next load |

Typed languages also have cases of their own (Go's zero values and pointers, Swift's literal initial values, Kotlin's default arguments, Rust's `serde(default)` and `deny_unknown_fields`), each in its own tests.

## 8. What developers see

With a wrong config for `weather` in `rutis.json` (`dev`, a TypeScript plugin; other languages print the same):

```text
# A value not in enum
weather: config: units should be "metric" or "imperial", got "kelvin"

# A bare string
weather: config: config should be an object, got "Oslo"

# A missing required field and wrong types, listed at once
weather: config: city is required
weather: config: days should be an integer, got 1.5
weather: config: home.lat should be a number, got "north"

# A misspelled key (a warning: the row starts, city takes its default)
[weather] weather: config: cty is not a config key (did you mean city?); it is ignored

# A row depending on it
chat: waiting for weather (row weather failed: config: units should be "metric" or "imperial", got "kelvin")

# The plugin's own default is wrong
weather: config schema: the default of units ("kelvin") should be "metric" or "imperial"
```

`rutis-host check`:

```text
weather (weather): cannot run: config: units should be "metric" or "imperial", got "kelvin"
rutis-host: warning: weather: config: cty is not a config key (did you mean city?); it is ignored
rutis-host: warning: weather: config schema: pattern is not checked by rutis; check it in the plugin
rutis-host: 1 row(s) or binaries cannot run
$ echo $?
1
```

(The existing `check` format `<row> (<plugin>): cannot run: …` is kept; a row with several problems gets one line per problem.)

Testing tools (Node, for example): `load(plugin, { units: 'kelvin' })` fails, throwing a `BindingError` whose `code` is `bad-config` and whose message is `config: units should be "metric" or "imperial", got "kelvin"`; Go's `rutistest.Load` returns the same error (`errors.As` yields a `*rutis.BindingError`).

## 9. Compatibility and migration

In "Migrating to 0.18" (both languages):

- **Configs that used to run although wrong now fail their row**: wrong types, missing `required` fields, a bare string as config, extra keys under `additionalProperties: false`. **Before upgrading, run 0.18's `rutis-host check`**: it lists every row whose config does not pass.
- **Defaults are now filled**: plugin code that checks "is this key in the config" sees the defaults; fallbacks written for defaults can be removed or kept.
- **Go**: the generated schema no longer has `required` (matching Go's zero values); `default` in a hand-written `Schema` is now filled (the Go plugin guide's "does not fill defaults for you" is rewritten).
- **Swift**: `var` fields with a literal initial value are no longer required; when not written they take that value.
- **Rust**: config errors change from `config validation failed: [...]` to `config: <path> …`; the types `ErrorKind::Validation` and `ReconcileReport` do not change.
- **Node, Bun**: `definePlugin` with a Standard Schema object as `config` fails.
- **Extra keys**: only a warning; an error under `check --strict`.
- The release notes say this delivers what the documentation has long promised (the plugin API guide's "the host validates").

## 10. Implementation

| Step | PR |
| --- | --- |
| 1 | The case list `specs/config-schema/`; the Rust implementation (a `config` module of rutis-loader): rows of `Builtins::register` / `register_with` / `register_raw`, the decoding path of `Definition`; the text of `ErrorKind::Validation`; `check`'s checking, warnings and exit status |
| 2 | Node and Bun: rows and testing tools of `definePlugin`; Cordis's `ValidationError` printed per §5.1; Standard Schema objects refused |
| 3 | Python: module rows, `define_plugin` rows and the testing tool |
| 4 | Go: validate, then decode; no `required` in the generated schema; `rutistest` |
| 5 | Swift and Kotlin: literal defaults of `@RutisConfig`; both runtimes, running in apps, and their testing tools |
| 6 | Documentation: the plugin API guide, the rutis-host guide's "Checking a configuration", each language's guide (withdrawing #714's "not checked yet"), "Migrating to 0.18", a new `docs/guarantees.md` entry "the config is filled with defaults and validated before `apply`" (naming the case list that verifies it) |

Each step merges with all cases of its column passing; #669 closes when step 6 merges with all 7 columns passing.
