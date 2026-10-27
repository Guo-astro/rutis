# Config schemas for 0.18: fill defaults, validate, then `apply`

[中文](design-config-schema-2026-10-27.md)

Status: final, not implemented. Date: 2026-10-27 (revised the same day: dropped the "keyword not checked" warning; decoder errors rewritten into the same sentences; extra keys warned about by the loader; the rules the 7 implementations must match word for word written down). Baseline: `main` `85678323` (after 0.17.2). Scheduled for: 0.18 (breaking, covered in "Migrating to 0.18").
Issue: [#669](https://github.com/arcships/rutis/issues/669). Consistent with [Diagnostics for 0.18](design-diagnostics-2026-10-26.en.md): errors carry an error code per its §4.3, are recorded on the row whose config is wrong per §4.7, print as in its §8.1 fault 12, and use none of the forbidden words of its §4.5.

Out of scope: finer constraints such as `pattern`, `format` and string lengths (anything outside the list in §3.1); type coercion (`"5"` does not become `5`); generating config forms or types (#684); Cordis rows are validated by Cordis itself (§2.3) and not again; Standard Schema as the config of `definePlugin`.

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
| C1 | Each language's SDK fills defaults and validates when it loads a row, before calling `apply` (the runtime and the testing tool run the same code); `check` does the same with the Rust implementation; the loader does not validate rows of other runtimes again | 2.1 |
| C2 | One JSON Schema subset of 14 keywords; other keywords are neither checked nor warned about | 3.1 |
| C3 | Extra keys: a warning, and passed to `apply` as they are; the loader works the warning out from `rows.schema` and prints it; an error when the schema says `additionalProperties: false`; `check --strict` treats the warning as an error | 3.2 |
| C4 | Typed configs (Go, Swift, Kotlin, Rust) get their schema from the type, stating only the rules the language's own decoder already has; decoder errors are rewritten into the same sentences | 3.3 |
| C5 | Defaults are filled level by level, "an object that is there gets its properties filled", following `$ref` and `allOf`, not `anyOf` / `oneOf`; with no config an object root is `{}` | 4 |
| C6 | All problems are listed at once, sorted by path, one per line: `<row>: config: <path> should be …, got …`; error code `bad-config` | 5 |
| C7 | With a wrong config the row fails, and rows depending on it show that they wait for it; `dev`, `run`, testing tools and `check` print the same | 5 |
| C8 | One shared case list `specs/config-schema/cases.json`, run case by case and compared word for word by every language and by `check`; every rule of §6 has cases | 6, 8 |

### 2.1 Why each language's SDK does it

- **Testing tools must behave as the real runtime.** A testing tool has no loader, only the SDK; the code with which the SDK loads a row is shared by the runtime and the testing tool. In the SDK, the rules a test exercised are the runtime's rules. In the loader, every language's testing tool would need its own copy anyway, split from the runtime.
- **Remote runtimes behave the same without extra work.** The config is checked where the row runs, whether the runtime is on this machine, on another one or in an app.
- **Typed languages decode here anyway.** Go, Swift, Kotlin and Rust decode the config into a type before `apply`; filling defaults and validating against the schema first makes the error text the same as in the other languages.
- **One set of rules, kept identical by the case list** (§6, §8) rather than by one piece of code: every language needs its own copy regardless (for its testing tool), and the case list makes the 7 copies and `check` agree word for word.
- Runtimes built with an SDK older than 0.18 do not check at run time; `check` checks their rows all the same.

### 2.2 The steps every row goes through

```text
the row's config (absent or null counts as none)
  → fill defaults (§4)
  → validate against the schema (§3.1); on errors → the row fails (§5), apply is not called
  → typed languages: decode into the type; an error the decoder still reports is also bad-config, rewritten into a §5.1 sentence (§3.3)
  → apply(ctx, config)        // a copy with defaults filled; extra keys kept as they are
```

A config change (the row restarts) goes through the same steps. A definition a plugin hands out (`ctx.plugin(def.at(&ctx, config))`) loads in the runtime where it was defined and goes through them too; a failure is recorded on that copy, and the caller of `ctx.plugin` gets the same error.

### 2.3 Cordis rows

Cordis rows (Cordis plugins that export `Config`) follow Cordis's rules: Cordis validates with `Config` when it loads them, and they are not validated again against the converted JSON Schema (the conversion loses information such as `transform`, so the two would disagree). The Node runtime turns Cordis's `ValidationError` into an error with code `bad-config`, written in the format of §5 (the path from the issue's `path`, the text from the issue's `message`). `check` does not check the config values of Cordis rows; it prints their schema, as today, and does not warn about their extra keys.

`definePlugin({ config })` takes JSON Schema only. Given a Standard Schema object (one with `~standard`) such as a schemastery one, the definition fails at once: `config of definePlugin should be a JSON Schema object, got a schemastery Schema` (`got a Standard Schema` when it is not schemastery). Such code was already wrong: the schema was only printed and never took effect.

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
| `anyOf`, `oneOf` | Matching one branch is enough. `oneOf` is checked as `anyOf`, without requiring exactly one match: this departs from JSON Schema, so that a `oneOf` generated by schemars does not fail a value that happens to match several branches |
| `allOf` | Every one must match |
| `$ref` | Only to `#/$defs/…` and `#/definitions/…` in the same document (enough for schemars and hand-written schemas) |

- Other keywords (`format`, `pattern`, `minLength`, `uniqueItems`, `prefixItems`, `if`, …) are neither checked nor warned about: the schemars and Kotlin generators write some of them on their own, and plugin authors cannot remove them. Each language's guide lists the keywords that are checked.
- No coercion: `"5"` does not match `number`.

### 3.2 Extra keys

- An object with `properties` and no `additionalProperties`: an extra key is a warning, and its value goes to `apply` as it is (what JSON Schema means; old configs keep starting). A spelling close to a known key gets a suggestion (§6.1).
- **The loader gives the warning**, not the runtime process: before sending `rows.load`, the loader works out the extra keys with the schema it already has (`rows.schema` / `plugins.read` / manifests) and the same Rust implementation, and writes them to the host's output (`rutis-host` to standard error, a Rust embedding to the default output of the diagnostics design §7), without the `[weather]` prefix of plugin output. Rows of remote runtimes and runtimes in apps are warned about by the local loader too, as are rows of pre-0.18 SDKs. SDKs report errors only.
- Testing tools have no loader and write the same warning to standard error themselves.
- `additionalProperties: false`: the SDK reports it as an error (§5); the row fails.
- `check --strict`: warnings count as errors, exit status 1 (as `--strict` treats its other warnings).
- Copies loaded by `ctx.plugin(def.at(...))` do not go through the loader's configuration: no warning, errors only.

### 3.3 How typed configs map

A generated schema states only the rules the language's own decoder already has; developers write no extra annotations for rutis:

| Language | Required | Defaults | Extra keys |
| --- | --- | --- | --- |
| Go (type `C`) | No `required` is generated: a field not written is its zero value, as with `encoding/json` and as the Go plugin guide says (**change**: fields without `omitempty` used to be listed as `required`); use a pointer to tell "not written" apart | Zero values; `default` in a hand-written `Schema` is filled as usual | Warning |
| Swift (`@RutisConfig`) | Optional types (`T?`) and `var`s with a literal initial value are not required; the rest are | The literal of `var units = "metric"` becomes the `default` (strings, numbers, booleans, and arrays and dictionaries of them); a non-literal initial value is not a default, and the field is required | Warning |
| Kotlin (`@Serializable`) | Not required when it has a default argument (`isElementOptional`) or is nullable | Filled by kotlinx when decoding (`SerialDescriptor` cannot see default values, so the schema has none and `check` does not show them) | Warning |
| Rust (`serde` + `schemars`) | schemars's existing rules: `Option<T>` and `#[serde(default)]` are not required | The `default` schemars writes into the schema; filled into the JSON first, so `serde(default)` still works and both are the same value | Warning; `#[serde(deny_unknown_fields)]` generates `additionalProperties: false`, an error |
| Rust (definitions made in Rust, no schema) | `serde` decoding | `serde(default)` | `serde`'s rules |

**Decoder errors are rewritten by kind**; the decoder's own text never appears:

| Kind | Sentence |
| --- | --- |
| Wrong type (serde `invalid type`; Go `UnmarshalTypeError`; Swift `typeMismatch`; Kotlin's type errors) | `<path> should be <expected>, got <value>` |
| Missing field (serde `missing field`; Swift `keyNotFound`; Kotlin `MissingFieldException`) | `<path> is required` |
| Unknown enum value (serde `unknown variant`; Kotlin's unknown enum) | `<path> should be "a" or "b", got "c"` (the `enum` sentence of §5.1) |
| Unknown key (serde `unknown field`; Kotlin's unknown key) | `<path> is not a config key (did you mean …?)` |
| Anything else | `<path> is not a value the plugin accepts` |

Rust decodes with an error type of its own (implementing `invalid_type`, `missing_field`, `unknown_variant` and `unknown_field` of `serde::de::Error`), so it gets the kind and the path directly instead of parsing serde's text; definitions made in Rust (`Definition`, whose config type only needs `DeserializeOwned` and has no schema) use it too. Other languages take the kind from their decoder's error type.

## 4. Defaults

- **No config** (no `config`, or `null`): the root schema's `default` when it has one; `{}` when the root is an object (its `type` is `"object"` or includes `"object"`); otherwise "no value" is checked (`config is required` when the root does not allow it). A root with `properties` and no `type` does not count as an object: any other rule would change what the JSON Schema means. A plugin without a schema is unchanged: it gets what it is given.
- **Level by level**: for an object that is there (including the initial `{}`), each missing property with a `default` is filled; then filling continues into the filled or existing child objects.
- **Through `$ref` and `allOf`, not into `anyOf` / `oneOf`** (as ajv does).
- **No objects out of nothing**: a missing child object without its own `default` is not filled, and `apply` does not see it; to always have the nested defaults, give the child object `default: {}`. This is ajv's `useDefaults` rule, the one JavaScript developers know best.
- What is filled is a copy: a plugin changing it does not affect the next load.
- Validation comes after filling: when a `default` does not match its schema, the runtime reports the ordinary error of that path (`units should be "metric" or "imperial", got "kelvin"`) and the row fails; `check` also says it is the plugin's `default` that is wrong (§7).
- Code that checks "is this key in the config" now sees the defaults filled: the behaviour this design wants, stated in the migration guide.

## 5. Errors

### 5.1 Text

One line per problem, all problems at once (order in §6.4; at most 20, the rest as `and N more`):

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
| `anyOf` / `oneOf` | `proxy should be a string or an object, got 3` (how branches are written: §6.5) |
| Key not allowed | `cty is not a config key (did you mean city?)` |

- Paths: `home.lat`, `cities[2]`; keys that are not identifiers (`[A-Za-z_][A-Za-z0-9_]*`) are written as a JSON string in brackets: `headers["x-api"]`. The root is `config`.
- How `got` is written: §6.2, §6.3.
- None of the forbidden words of the diagnostics design §4.5 appear (serde, `ValidationError`, internal type names, platform names).

### 5.2 Structure

- The error is a `BindingError` with code `bad-config`. The diagnostics design §4.3 says "rows may be added to the table during implementation; existing rows do not change"; this design adds one row to that table: `bad-config`, when: the config does not match the schema, `data`: `{ issues: [{ path: ["home", "lat"], message: "home.lat should be a number, got \"north\"" }] }` (the items of `path` are keys or indices).
- SDKs send only `code` and `data`; `site` (the row, `doing: "apply"`, diagnostics design §4.1) is filled by the loader at the boundary; testing tools have no `site`. When rutis prints a `bad-config` error it writes `config:`, not `failed in apply:`.
- Cordis rows' `ValidationError` is `bad-config` too (§2.3).
- Rust: a config error of a loader row stays `ErrorKind::Validation { issues }`, with each line above as an issue, displayed as `config: …` (no more `config validation failed: [...]`); the text of `Failure` in `ReconcileReport` changes with it, the types do not.
- **How a host recognises a config error**: the same way for every row, with `rutis_loader::config_issues(&rutis::Error) -> Option<Vec<String>>`: when the error a row failed to load with is `ErrorKind::Validation` (Rust rows) or wraps a cross-language error with code `bad-config` (rows of other languages, Cordis rows, through `rutis_bridge::thrown`), it gives the text of each problem. No matching on text.

### 5.3 Which row, and what then

- Recorded on the row whose config is wrong (diagnostics design §4.7): the row fails, `apply` is not called; rows depending on it show `chat: waiting for weather (row weather failed: config: units should be …)`, not an error of the caller.
- Once the config is fixed, the row reloads as usual (automatically when `dev` watches the config file; under `run` by the existing ways to reload).
- `dev` and `run` follow the same rules; they differ only in the output prefix of the diagnostics design §7; config errors and warnings are printed by rutis, without the plugin-output prefix.

## 6. Rules to match word for word

Where the 7 implementations and `check` must agree word for word; each rule has cases in the case list:

### 6.1 Spelling suggestions

The same as rutis-host's suggestions for service names (`closest` in `rutis-loader` `loader/waits.rs`, which `status.rs` and `check`'s `did you mean` use):

- Edit distance counted in Unicode scalar values; one edit is an insertion, a deletion, a substitution or a swap of two neighbours (OSA distance).
- Keys of 0–2 scalars: no suggestion; 3–6: distance at most 1; 7 or more: at most 2.
- Candidates are the keys in the same object's `properties` (not the key itself); the smallest distance wins, ties going to the smallest by Unicode code point.

### 6.2 The `got` value

- Strings, numbers, booleans and `null` are written as JSON; an object is `an object`, an array `an array of N` (`N` items). Contents of objects and arrays are not written, so key order never matters.
- Numbers: an integral value of magnitude at most 2^53 has no decimal point (`1.0` is written `1`); others are written in the shortest round-trip form (as JavaScript's `String(number)`: `1.5`, `1e+21`). Integers beyond 2^53 have different precision in different languages and are not in the case list.
- Strings: JSON escapes for `"`, `\` and control characters (short forms such as `\n` and `\t` where they exist, otherwise `\u00XX` in lower-case hex); non-ASCII characters are written as they are, not escaped.

### 6.3 Length limit

A string whose content is longer than 40 Unicode scalar values is written as its first 40, followed by `…` (U+2026), in quotes: `"<first 40>…"`. Counting happens before escaping.

### 6.4 Order of problems

Sorted by path: paths compare segment by segment, keys by Unicode code point, indices numerically, and a path that is a prefix of another comes first (parents before children). At the same path, type comes first, then `enum` / `const`, then bounds. A missing required key sorts by its own path (`home.lat`). This depends neither on the order of keys in the config (Go maps have none) nor on the order of `properties` in the schema (Go's generated schema is a map).

### 6.5 Branches written when `anyOf` / `oneOf` fails

- Each branch is followed through `$ref` and `allOf` to its `type` / `const` / `enum`, and written as: `type` → `a string`, `a number`, `an integer`, `a boolean`, `an object`, `an array`, `null`; `const` and `enum` → their values (written as in §6.2); a branch with none of the three is `another accepted value`.
- In branch order, without duplicates; two joined with `or`, three or more as `one of a, b, c` (as for `enum`).

## 7. `rutis-host check`

- For every row, with the schema it already prints, it fills defaults and validates by the same rules (the Rust implementation shared with loader rows), without running plugins.
- Errors: written in the existing format `<row> (<plugin>): cannot run: config: …`, one line per problem; exit status 1. Extra keys are warnings on standard error with exit status 0; 1 with `--strict`.
- When a plugin's `default` itself does not match its schema, `check` says it is the `default` that is wrong: `weather (weather): cannot run: config schema: the default of units should be "metric" or "imperial", got "kelvin"`. The runtime does not print this sentence (§4).
- Rows whose schema is unknown at check time (remote runtimes not connected, `peer:` rows) say `config not checked: its schema is known only when it runs`, which is not a warning. Cordis rows: see §2.3.
- `check` prints the same text as the runtime, word for word (locked by the same case list).

## 8. The case list

`specs/config-schema/cases.json`: each case is `{ name, schema, config, then: { config } | { errors: [...] }, warnings: [...] }`, with `errors` and `warnings` in the full text of §5.1. Every SDK's tests (Node, Bun, Python, Go, Swift, Kotlin), rutis-loader and `check` run every case and compare word for word (`warnings` against the warnings of the testing tool and of the loader). The cases include at least:

| # | Case |
| --- | --- |
| 1 | No config, object root: `{}` plus defaults; the same for a `["object", "null"]` root; a root with only `properties` and no `type`: no `{}` made |
| 2 | `null` is the same as none |
| 3 | A bare string as config: `config should be an object, got "Oslo"` |
| 4 | Missing `required` |
| 5 | Wrong types (one each for string, number, integer, boolean, array, object) |
| 6 | `enum` with two values and with three; `const` |
| 7 | `minimum`, `maximum` at the bounds (equal passes) |
| 8 | Nested object present: its defaults filled; absent: no object made; child `default: {}`: made and filled |
| 9 | Defaults follow `$ref` and `allOf`, not `anyOf` / `oneOf` |
| 10 | An array item wrong: path `cities[2]` |
| 11 | A dictionary (`additionalProperties` as a schema) |
| 12 | Extra keys: warning, kept as they are; the three length bands of suggestions, ties, a neighbour swap counting as one edit |
| 13 | `additionalProperties: false`: error |
| 14 | `anyOf` / `oneOf`; `["string", "null"]`; how branches that are a `$ref`, a `const` or neither are written; a value matching two `oneOf` branches passes |
| 15 | `$ref` to `$defs` and to `definitions` |
| 16 | Keywords not checked (`pattern`, `format`, `uniqueItems`): the value passes, no warning |
| 17 | Order of several problems (code-point order of keys, parents before children, order at one path); more than 20 truncated |
| 18 | A `default` that does not match its schema: at run time an ordinary path error |
| 19 | Paths with non-identifier keys |
| 20 | How `got` is written: `1.0` as `1`, `1.5`, `1e+21`, escapes (`"`, `\`, `\n`, `\u0001`), non-ASCII as is, truncation past 40 scalar values (including multi-byte characters), objects and arrays |
| 21 | Defaults are copies: a plugin changing one does not affect the next load |

Typed languages also have cases of their own (Go's zero values and pointers, Swift's literal initial values, Kotlin's default arguments, Rust's `serde(default)` and `deny_unknown_fields`, the decoder error kinds of §3.3), each in its own tests.

## 9. What developers see

With a wrong config for `weather` in `rutis.json` (`dev`, a TypeScript plugin; other languages print the same):

```text
# A value not in enum
weather: config: units should be "metric" or "imperial", got "kelvin"

# A bare string
weather: config: config should be an object, got "Oslo"

# A missing required field and wrong types, listed at once (sorted by path)
weather: config: city is required
weather: config: days should be an integer, got 1.5
weather: config: home.lat should be a number, got "north"

# A misspelled key (a warning: the row starts, city takes its default)
weather: config: cty is not a config key (did you mean city?); it is ignored

# A row depending on it
chat: waiting for weather (row weather failed: config: units should be "metric" or "imperial", got "kelvin")

# A Rust plugin, a decoder error (rewritten)
weather: config: city should be a string, got 42
```

`rutis-host check`:

```text
weather (weather): cannot run: config: units should be "metric" or "imperial", got "kelvin"
rutis-host: warning: weather: config: cty is not a config key (did you mean city?); it is ignored
rutis-host: 1 row(s) or binaries cannot run
$ echo $?
1
```

(The existing `check` format `<row> (<plugin>): cannot run: …` is kept; a row with several problems gets one line per problem.)

Testing tools (Node, for example): `load(plugin, { units: 'kelvin' })` fails, throwing a `BindingError` whose `code` is `bad-config` and whose message is `config: units should be "metric" or "imperial", got "kelvin"`; Go's `rutistest.Load` returns the same error (`errors.As` yields a `*rutis.BindingError`).

## 10. Compatibility and migration

In "Migrating to 0.18" (both languages):

- **Configs that used to run although wrong now fail their row**: wrong types, missing `required` fields (Python ignored `required` entirely), a bare string as config, extra keys under `additionalProperties: false`. **Before upgrading, run 0.18's `rutis-host check`**: it lists every row whose config does not pass.
- **Tests can fail too**: tests that pass partial or wrong configs to a testing tool now fail as the runtime does; complete the config per the schema, or let the schema say truthfully which fields may be left out.
- **Defaults are now filled**: plugin code that checks "is this key in the config" sees the defaults; fallbacks written for defaults can be removed or kept.
- **Go**: the generated schema no longer has `required` (matching Go's zero values); `default` in a hand-written `Schema` is now filled (the Go plugin guide's "does not fill defaults for you" is rewritten).
- **Swift**: `var` fields with a literal initial value are no longer required; when not written they take that value.
- **Rust**: config errors change from `config validation failed: [...]` to `config: <path> …`; the types `ErrorKind::Validation` and `ReconcileReport` do not change; recognise config errors with `rutis_loader::config_issues`.
- **Node, Bun**: `definePlugin` with a Standard Schema object as `config` fails. Only code that was already wrong is affected: such a schema was only printed and never took effect.
- **Runtimes built with a pre-0.18 SDK**: their rows' configs are not checked at run time; `check` still checks them. Checking at run time starts once the SDK is upgraded.
- **Extra keys**: only a warning; an error under `check --strict`.
- The release notes say this delivers what the documentation has long promised (the plugin API guide's "the host validates").

## 11. Implementation

| Step | PR |
| --- | --- |
| 1a | The case list `specs/config-schema/`; the Rust implementation (a `config` module of rutis-loader: filling defaults, validation, the rules of §6); `check`'s checking, warnings and exit status |
| 1b | Rust rows: `Builtins::register` / `register_with` / `register_raw`, the decoding path of `Definition` and the error rewriting of §3.3; the text of `ErrorKind::Validation`; the loader's warning about extra keys; filling `site`; `config_issues` |
| 2 | Node and Bun: rows and testing tools of `definePlugin`; Cordis's `ValidationError` turned into `bad-config`; Standard Schema objects refused |
| 3 | Python: module rows, `define_plugin` rows and the testing tool |
| 4 | Go: validate, then decode; no `required` in the generated schema; `rutistest` |
| 5 | Swift and Kotlin: literal defaults of `@RutisConfig`; both runtimes, running in apps, and their testing tools |
| 6 | Documentation: the plugin API guide (listing the keywords checked), the rutis-host guide's "Checking a configuration", each language's guide (withdrawing #714's "not checked yet"), the `bad-config` row in the §4.3 table of the diagnostics design (both languages), "Migrating to 0.18", a new `docs/guarantees.md` entry "the config is filled with defaults and validated before `apply`" (naming the case list that verifies it) |

Each step merges with all cases of its column passing; #669 closes when step 6 merges with all 7 columns passing.
