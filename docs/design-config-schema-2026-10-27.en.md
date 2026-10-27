# Config schemas for 0.18: fill defaults, validate, then `apply`

[中文](design-config-schema-2026-10-27.md)

Status: final, not implemented. Date: 2026-10-27 (revised the same day: dropped the "keyword not checked" warning; decoder errors rewritten into the same sentences; extra keys warned about by the loader; the rules the 7 implementations must match word for word written down; revised again: the kernel's `Validation` text unchanged, `config_issues` removed, dylib rows, rows of app runtimes and pre-0.18 binaries covered, decoder errors simplified; third revision: where the loader does this and how its failure text is written, the exceptions for old dylibs and old Swift binaries removed; fourth revision: two principles for the loader, with call sites listed only in the checklist of §11; fifth revision: config errors become text through their own `Display`, and how they read inside wrappers is stated; sixth revision: a CI check that existing configs are not rejected, and verification on Dim before release). Baseline: `main` `85678323` (after 0.17.2). Scheduled for: 0.18 (breaking, covered in "Migrating to 0.18").
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
| C3 | Extra keys: a warning, and passed to `apply` as they are; the loader's config step (§2.1) works the warning out and prints it once per row and config value; an error when the schema says `additionalProperties: false`; `check --strict` treats the warning as an error | 3.2 |
| C4 | Typed configs (Go, Swift, Kotlin, Rust) get their schema from the type, stating only the rules the language's own decoder already has; a decoder error after the schema passed is written `is not a value the plugin accepts`, never the decoder's text | 3.3 |
| C5 | Defaults are filled level by level, "an object that is there gets its properties filled", following `$ref` and `allOf`, not `anyOf` / `oneOf`; with no config an object root is `{}` | 4 |
| C6 | All problems are listed at once, sorted by path, one per line: `<row>: config: <path> should be …, got …`; error code `bad-config` | 5 |
| C7 | With a wrong config the row fails, and rows depending on it show that they wait for it; `dev`, `run`, testing tools and `check` print the same | 5 |
| C8 | One shared case list `specs/config-schema/cases.json`, run case by case and compared word for word by every language and by `check`; every rule of §6 has cases | 6, 8 |

### 2.1 Why each language's SDK does it

- **Testing tools must behave as the real runtime.** A testing tool has no loader, only the SDK; the code with which the SDK loads a row is shared by the runtime and the testing tool. In the SDK, the rules a test exercised are the runtime's rules. In the loader, every language's testing tool would need its own copy anyway, split from the runtime.
- **Remote runtimes behave the same without extra work.** The config is checked where the row runs, whether the runtime is on this machine, on another one or in an app.
- **Typed languages decode here anyway.** Go, Swift, Kotlin and Rust decode the config into a type before `apply`; filling defaults and validating against the schema first makes the error text the same as in the other languages.
- **One set of rules, kept identical by the case list** (§6, §8) rather than by one piece of code: every language needs its own copy regardless (for its testing tool), and the case list makes the 7 copies and `check` agree word for word.
- Runtimes built with an SDK older than 0.18 do not check at run time; `check` checks their rows all the same (§7).
- **In-process Rust rows have no runtime of their own**: rows registered with `Builtins`, dylib rows and rows from an embedder's own resolver (in the loader, the rows that belong to no other runtime) are filled with defaults and validated by the loader with the Rust implementation; rows of other runtimes are done by their SDKs and not again by the loader (otherwise they would be checked twice, rows of pre-0.18 SDKs would fail, and Cordis rows would be checked against the converted schema). Two principles:
  - **The config step lives in one place.** One loader function fills a copy, validates, works out the extra-key warning and returns the marked error; every place that checks config (the check before the first load, `commit`'s dry run, the real load and config changes, rows given per connection by runtimes that dial in) calls it, and none has its own copy. The record of what was warned about lives in the loader's shared state, shared by dry runs and the real commit. The list of call sites is in step 1b of §11.
  - **A config error writes itself as text.** The marked config error type (in `rutis`) has a `Display` that is `config: <problem>`, one line per problem. So wherever a row's error is turned into text (the failure report, wait reasons, `commit`'s comparison deciding "this row was already failing", the display of `LoaderError::Rejected`, the `error` of the `rutis/row` event, errors the host prints), the text is the same, with no formatter function that every place must remember to call.
  - How it reads inside wrappers: `LoaderError::Rejected` wrapping a config error drops `entry "weather" rejected: ` and shows the inner text as it is (the places that print it, `check`, the check before a reload and `dev`'s `cannot reload`, already write the row name); once the diagnostics design §4.2 drops the `plugin failed: ` prefix, `ErrorKind::PluginFailed` shows it as it is too. rutis-host prints every line of a multi-line error with the row's prefix (as in the `check` and `dev` output of §9); places that hold one line only (wait reasons, status lines) show the first problem. The `error` of the `rutis/row` event is `config: …` itself, with the row name in the event's `row`.
- The loader keeps the raw config value, not the filled one: otherwise every config comparison would see the two differ and update the row again.

### 2.2 The steps every row goes through

```text
the row's config (absent or null counts as none)
  → fill defaults (§4)
  → validate against the schema (§3.1); on errors → the row fails (§5), apply is not called
  → typed languages: decode into the type; an error the decoder still reports is also bad-config, rewritten into a §5.1 sentence (§3.3)
  → apply(ctx, config)        // a copy with defaults filled; extra keys kept as they are
```

A config change (the row restarts) goes through the same steps. A definition a plugin hands out (`ctx.plugin(def.at(&ctx, config))`) loads in the runtime where it was defined and goes through them too; a failure is recorded on that copy, and the caller of `ctx.plugin` gets the same error.

A row updated in place because only `x-volatile` fields changed also gets the filled copy, so the plugin sees the same defaults as in `apply` (rows the loader fills are filled by the loader; rows of other runtimes by their SDK or by Cordis).

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
- **Known keys**: the union of the `properties` keys of the object's schema and of what it takes in through `$ref` and `allOf`; they are also the candidates for suggestions. Objects inside `anyOf` / `oneOf` branches get no extra-key warnings (which branch the value was written for is unknown).
- **The loader gives the warning**, not the runtime process: in the config step (§2.1), for every row with a schema (whichever runtime it belongs to), it works out the extra keys and writes them to the host's output (`rutis-host` to standard error, a Rust embedding to the default output of the diagnostics design §7), without the `[weather]` prefix of plugin output. One warning per row and config value: restarts do not repeat it. Rows of remote runtimes and of pre-0.18 SDKs are warned about by the local loader too. For runtimes that dial in (in apps) the schema is known per connection, and the warning is given once per row, instance and config value. SDKs report errors only.
- Cordis rows get no extra-key warnings: Cordis's rules apply (§2.3).
- Testing tools have no loader and write the same warning to standard error themselves.
- `additionalProperties: false`: the SDK reports it as an error (§5); the row fails.
- `check --strict`: warnings count as errors, exit status 1 (as `--strict` treats its other warnings).
- Copies loaded by `ctx.plugin(def.at(...))` do go through rutis-loader (`PluginMounter`, with the schema from `ForeignPlugin.schema()`), but **are not warned about**, errors only: a copy's config is written by plugin code, not a config file a person edits, so no one would act on the warning; testing tools do not warn for copies either.

### 3.3 How typed configs map

A generated schema states only the rules the language's own decoder already has; developers write no extra annotations for rutis:

| Language | Required | Defaults | Extra keys |
| --- | --- | --- | --- |
| Go (type `C`) | No `required` is generated: a field not written is its zero value, as with `encoding/json` and as the Go plugin guide says (**change**: fields without `omitempty` used to be listed as `required`); use a pointer to tell "not written" apart | Zero values; `default` in a hand-written `Schema` is filled as usual | Warning |
| Swift (`@RutisConfig`) | Optional types (`T?`) and `var`s with a literal initial value are not required; the rest are | The literal of `var units = "metric"` becomes the `default` (strings, numbers, booleans, and arrays and dictionaries of them); a non-literal initial value is not a default, and the field is required | Warning |
| Kotlin (`@Serializable`) | Not required when it has a default argument (`isElementOptional`) or is nullable | Filled by kotlinx when decoding (`SerialDescriptor` cannot see default values, so the schema has none and `check` does not show them) | Warning |
| Rust (`serde` + `schemars`) | schemars's existing rules: `Option<T>` and `#[serde(default)]` are not required | The `default` schemars writes into the schema; filled into the JSON first, so `serde(default)` still works and both are the same value | Warning; `#[serde(deny_unknown_fields)]` generates `additionalProperties: false`, an error |
| Rust (no schema: `Definition`, and dylibs whose `export_plugin!` has no `schema:`) | `serde` decoding | `serde(default)` | `serde`'s rules |

**Decoder errors**: with a schema, validation runs first; a decoder error after the schema passed means the schema and the type disagree, and is always written `<path> is not a value the plugin accepts`, never the decoder's own text. The path comes from each language's decoder; Kotlin's `RutisFormat` needs one added (its `ValueException` has no path today).

Rust decoding without a schema is rewritten by kind: `Definition` (whose config type only needs `DeserializeOwned`), and dylibs whose `export_plugin!` has no `schema:` (it is optional, and most leave it out):

| Kind | Sentence |
| --- | --- |
| `invalid_type` | `<path> should be <expected>, got <value>`; `<expected>` comes from the `deserialize_*` method that was called (`deserialize_str` → `a string`), not from serde's `expected`, which is an internal type name (diagnostics design §4.5) |
| `missing_field` | `<path> is required` |
| `unknown_variant` | `<path> should be "a" or "b", got "c"` (the `enum` sentence of §5.1) |
| `unknown_field` | `<path> is not a config key (did you mean …?)` |
| Anything else | `<path> is not a value the plugin accepts` |

For this Rust needs a `Deserializer` over `serde_json::Value` with an error type of its own (`serde::de::value` is generic over the error type) that records the path and the method called while decoding, instead of parsing serde's text. It and the marked config error type live in `rutis`: `Definition`, rutis-sdk (dylibs) and the loader's `Builtins` rows all use it. With a schema the same decoder gives only the one sentence `is not a value the plugin accepts`; without one it goes by the kinds above.

## 4. Defaults

- **No config** (no `config`, or `null`): the root schema's `default` when it has one; `{}` when the root is an object (its `type` is `"object"` or includes `"object"`); otherwise "no value" is checked (`config is required` when the root does not allow it). A root with `properties` and no `type` does not count as an object: any other rule would change what the JSON Schema means. A plugin without a schema is unchanged: it gets what it is given.
- **Level by level**: for an object that is there (including the initial `{}`), each missing property with a `default` is filled; then filling continues into the filled or existing child objects.
- **Through `$ref` and `allOf`, not into `anyOf` / `oneOf`** (as ajv does).
- **No objects out of nothing**: a missing child object without its own `default` is not filled, and `apply` does not see it; to always have the nested defaults, give the child object `default: {}`. This is ajv's `useDefaults` rule, the one JavaScript developers know best.
- What is filled is a copy: a plugin changing it does not affect the next load.
- A value that passes `integer` (such as `1.0`) is written into the copy handed to the plugin as an integer (`1`): otherwise Go's `encoding/json` and serde reject it when decoding into an integer type.
- Validation comes after filling: when a `default` does not match its schema, the error is the ordinary one of that path (`units should be "metric" or "imperial", got "kelvin"`) and the row fails; the runtime and `check` say the same.
- Code that checks "is this key in the config" now sees the defaults filled: the behaviour this design wants, stated in the migration guide.

## 5. Errors

### 5.1 Text

One line per problem, all problems at once (order in §6.4; at most 20, the rest as a last line `<row>: config: and N more problems`):

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
| Key not allowed (error), extra key (warning) | `cty is not a config key (did you mean city?)`; without a close key, no parenthesis. The warning is the same sentence as the error; it only does not fail the row (the value still reaches `apply`) |

- Paths: `home.lat`, `cities[2]`; keys that are not identifiers (`[A-Za-z_][A-Za-z0-9_]*`) are written as a JSON string in brackets: `headers["x-api"]`. The root is `config`.
- How `got` is written: §6.2, §6.3.
- At a path whose type is wrong, only the type is reported: not its `enum` or bounds, and nothing in its properties or items.
- None of the forbidden words of the diagnostics design §4.5 appear (serde, `ValidationError`, internal type names, platform names).

### 5.2 Structure

- The error is a `BindingError` with code `bad-config`. The diagnostics design §4.3 says "rows may be added to the table during implementation; existing rows do not change"; this design adds one row to that table: `bad-config`, when: the config does not match the schema, `data`: `{ issues: [{ path: ["home", "lat"], message: "home.lat should be a number, got \"north\"" }] }` (the items of `path` are keys or indices).
- SDKs send only `code` and `data`; `site` (the row, `doing: "apply"`, diagnostics design §4.1) is filled by the loader at the boundary; testing tools have no `site`. When rutis prints a `bad-config` error it writes `config:`, not `failed in apply:`.
- Cordis rows' `ValidationError` is `bad-config` too (§2.3).
- Rust rows (the rows the loader fills and validates): when the config step fails it returns a marked error (a type in `rutis` carrying the text of each problem), passed as the plugin's error inside `ErrorKind::PluginFailed`; dylibs pass Rust values directly (the SDK is checked), so it is recognised there too. Its own `Display` (§2.1) gives `config: <problem>` in the failure report, in wait reasons (`chat: waiting for weather (row weather failed: config: …)`) and in the `rutis/row` event, and it stays distinct from errors a plugin returns from its own config check and from the kernel's `config type mismatch`. The kernel's `ErrorKind::Validation` is used for more than config, and its display text does not change.
- A host recognises config errors of rows of other languages (Cordis rows included) by the error code, with `rutis_bridge::thrown(&e)` of the diagnostics design §4.2 / §8.2: `code() == "bad-config"`. No matching on text.

### 5.3 Which row, and what then

- Recorded on the row whose config is wrong (diagnostics design §4.7): the row fails, `apply` is not called; rows depending on it show `chat: waiting for weather (row weather failed: config: units should be …)`, not an error of the caller.
- Once the config is fixed, the row reloads as usual (automatically when `dev` watches the config file; under `run` by the existing ways to reload).
- `dev` and `run` follow the same rules; they differ only in the output prefix of the diagnostics design §7; config errors and warnings are printed by rutis, without the plugin-output prefix.

## 6. Rules to match word for word

Where the 7 implementations and `check` must agree word for word; each rule has cases in the case list:

### 6.1 Spelling suggestions

The same as rutis-host's suggestions for service names, with rutis-loader's `closest` (rutis-host only prints what the loader worked out):

- Edit distance counted in Unicode scalar values; one edit is an insertion, a deletion, a substitution or a swap of two neighbours (OSA distance).
- Keys of 0–2 scalars: no suggestion; 3–6: distance at most 1; 7 or more: at most 2.
- Candidates are the object's known keys (§3.2, not the key itself); the smallest distance wins, ties going to the smallest by Unicode code point (the config module sorts the candidates before handing them to `closest`, which itself does not change).

### 6.2 The `got` value

- Strings, numbers, booleans and `null` are written as JSON; an object is `an object`, an array `an array of N` (`N` items). Contents of objects and arrays are not written, so key order never matters.
- Numbers: an integral value of magnitude at most 2^53 has no decimal point (`1.0` is written `1`); others are written in the shortest round-trip form (as JavaScript's `String(number)`: `1.5`, `1e+21`). Integers beyond 2^53 have different precision in different languages and are not in the case list.
- Strings: JSON escapes for `"`, `\` and control characters (short forms such as `\n` and `\t` where they exist, otherwise `\u00XX` in lower-case hex); non-ASCII characters are written as they are, not escaped.

### 6.3 Length limit

A string whose content is longer than 40 Unicode scalar values is written as its first 40, followed by `…` (U+2026), in quotes: `"<first 40>…"`. Counting happens before escaping.

### 6.4 Order of problems

Sorted by path: paths compare segment by segment, keys by Unicode code point, indices numerically, and a path that is a prefix of another comes first (parents before children). At the same path the order is: type, `enum` / `const`, bounds, `anyOf` / `oneOf`, then problems from `allOf` members in member order. Missing required keys and extra keys are at child paths and sort by key with the other child paths. A missing required key sorts by its own path (`home.lat`). This depends neither on the order of keys in the config (Go maps have none) nor on the order of `properties` in the schema (Go's generated schema is a map).

### 6.5 Branches written when `anyOf` / `oneOf` fails

- Each branch is followed through `$ref` and `allOf` to its `type` / `const` / `enum`, and written as: `type` → `a string`, `a number`, `an integer`, `a boolean`, `an object`, `an array`, `null`; `const` and `enum` → their values (written as in §6.2); a branch with none of the three is `another accepted value`.
- In branch order, without duplicates; two joined with `or`, three or more as `one of a, b, c` (as for `enum`).

## 7. `rutis-host check`

- For every row, with the schema it already prints, it fills defaults and validates by the same rules (the Rust implementation shared with loader rows), without running plugins.
- Errors: written in the existing format `<row> (<plugin>): cannot run: config: …`, one line per problem; exit status 1. Extra keys are warnings on standard error with exit status 0; 1 with `--strict`.
- Go binaries built with a pre-0.18 SDK (the manifest's `sdk` older than 0.18): `check` ignores `required` in the manifest and checks the rest as usual. Old Go manifests list every field without `omitempty` as required, which would give false failures. Old Swift manifests need no exception: the old macro writes `required` by whether a field is optional, and the synthesized `Decodable` requires exactly those keys.
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
| 12 | Extra keys: warning, kept as they are; the three length bands of suggestions, ties, a neighbour swap counting as one edit; the text without a close key |
| 13 | `additionalProperties: false`: error |
| 14 | `anyOf` / `oneOf`; `["string", "null"]`; how branches that are a `$ref`, a `const` or neither are written; a value matching two `oneOf` branches passes |
| 15 | `$ref` to `$defs` and to `definitions` |
| 16 | Keywords not checked (`pattern`, `format`, `uniqueItems`): the value passes, no warning |
| 17 | Order of several problems (code-point order of keys, parents before children, order at one path); more than 20 truncated |
| 18 | A `default` that does not match its schema: at run time an ordinary path error |
| 19 | Paths with non-identifier keys |
| 20 | How `got` is written: `1.0` as `1`, `1.5`, `1e+21`, escapes (`"`, `\`, `\n`, `\u0001`), non-ASCII as is, truncation past 40 scalar values (including multi-byte characters), objects and arrays |
| 21 | Defaults are copies: a plugin changing one does not affect the next load |
| 22 | Known keys include `properties` taken in through `$ref` and `allOf`; objects inside `anyOf` / `oneOf` branches get no warnings |
| 23 | `1.0` passing `integer` is `1` in the resulting config |
| 24 | At a path with a wrong type, no `enum`, bounds or child-path problems |

Typed languages also have cases of their own (Go's zero values and pointers, Swift's literal initial values, Kotlin's default arguments, Rust's `serde(default)` and `deny_unknown_fields`, the decoder error kinds of Rust `Definition`), each in its own tests.

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
weather: config: cty is not a config key (did you mean city?)

# A row depending on it
chat: waiting for weather (row weather failed: config: units should be "metric" or "imperial", got "kelvin")

# A Rust plugin (the type error is caught by the schema first, as in other languages)
weather: config: city should be a string, got 42

# A definition made in Rust (no schema), the decoder error rewritten
demo: config: city should be a string, got 42
```

`rutis-host check`:

```text
weather (weather): cannot run: config: units should be "metric" or "imperial", got "kelvin"
rutis-host: warning: weather: config: cty is not a config key (did you mean city?)
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
- **Go**: the generated schema no longer has `required` (matching Go's zero values; for pre-0.18 binaries `check` ignores their `required` too); `default` in a hand-written `Schema` is now filled (the Go plugin guide's "does not fill defaults for you" is rewritten).
- **Swift**: `var` fields with a literal initial value are no longer required; when not written they take that value.
- **Rust**: the text of loader rows' config errors in `ReconcileReport` changes from `config validation failed: [...]` to `config: <path> …`; the types do not change, nor does the display text of the kernel's `ErrorKind::Validation`. Dylib rows are validated by the loader before `build`; dylibs are rebuilt against 0.18 as with every release (rutis-dylib checks the SDK's id and hash, so old dylibs are not loaded).
- **An embedder's own resolver**: a row given a schema with `with_schema` now has defaults filled and is validated against it (the doc comment of `Resolved.schema` says so too).
- **Node, Bun**: `definePlugin` with a Standard Schema object as `config` fails. Only code that was already wrong is affected: such a schema was only printed and never took effect.
- **Runtimes built with a pre-0.18 SDK**: their rows' configs are not checked at run time; `check` still checks them. Checking at run time starts once the SDK is upgraded.
- **Verified on Dim before release**: Dim (github.com/arcships/dim, the main downstream user) embeds rutis-loader and has no `rutis.json`; its configs are built in code. Before release, run Dim's tests and one start-up against 0.18: no `config:` failures in `ReconcileReport` and no extra-key warnings, with the result in the release checklist. For a failing row, fix rutis when its rule is wrong, or tell Dim to change its config when the config is wrong.
- **Extra keys**: only a warning; an error under `check --strict`.
- The release notes say this delivers what the documentation has long promised (the plugin API guide's "the host validates").

## 11. Implementation

| Step | PR |
| --- | --- |
| 1a | The case list `specs/config-schema/`; the Rust implementation (a `config` module of rutis-loader: filling defaults, validation, the rules of §6); `check`'s checking, warnings and exit status |
| 1b | The Rust parts in the loader and `rutis`. The config step function (§2.1) and its call sites: the check before spawning a row (`reconcile.rs` ~372, which today calls the inner factory's `validate_config` directly), the dry run `dry_run_copies` (`commit.rs` 256–261, which builds no `EntryFactory`), `EntryFactory`'s `validate_config` / `build` (`loader/plugins.rs`, only for rows whose `Resolved.foreign_scope` is false; the schema from `config.resolved.schema`, Cordis rows by `Resolved.meta["cordis"]`), and the per-connection factory returned by `RuntimeInstances::factory` (`runtime/connections.rs`, warning from its `validate_config` / `build`, deduped per row, instance and value); the dedup record lives in the loader's `Inner` / `State`. The `Display` of the marked config error (§2.1): the failure report, wait reasons, the `tolerated` comparison and the `rutis/row` event need no changes of their own because of it; `LoaderError::Rejected` wrapping it shows the inner text as it is; rutis-host prints every line of a multi-line error with the row name. `VolatileUpdate { config }` sends the filled copy. The `Value` `Deserializer` and the marked error type in `rutis`; their users: `Definition`, rutis-sdk's dylib decoding (`crates/rutis-sdk/src/lib.rs`), `Json<C>::decode` in `resolver.rs`. Filling `site`; the doc comment of `Resolved.schema` |
| 1c | Existing configs still pass under 0.18 (the case list only proves the 7 implementations agree with each other, not that correct configs in use today are not rejected). In CI: the project each `rutis-host new` template makes, with its `rutis.dev.json` (such as `greeting: "Hello"`), passes `check` with no config errors. Node, Python and Go are added to the existing `crates/rutis-host/tests/templates.rs`; the Bun template (in a job with Bun) and the Swift template (in the macOS job) get the same test. The examples under `examples/` have no `rutis.json`, since their configs are built in Rust code; their existing tests run the loader's config step and must keep passing. The plugins and binaries that `tests/fixtures/persisted/*/rutis.json` refers to do not exist, so `check` cannot get their schemas; they are not run through `check` and stay covered by the existing `persisted.rs`, which must keep reading them (`rejected/` holds samples that are meant not to be read and is not among them) |
| 2 | Node and Bun: rows and testing tools of `definePlugin`; Cordis's `ValidationError` turned into `bad-config`; Standard Schema objects refused |
| 3 | Python: module rows, `define_plugin` rows and the testing tool |
| 4 | Go: validate, then decode; no `required` in the generated schema; `rutistest` |
| 5 | Swift and Kotlin: literal defaults of `@RutisConfig`; paths in Kotlin `RutisFormat` errors; both runtimes, running in apps, and their testing tools |
| 6 | Documentation: the plugin API guide (listing the keywords checked), the rutis-host guide's "Checking a configuration", each language's guide (withdrawing #714's "not checked yet"), the `bad-config` row in the §4.3 table of the diagnostics design (both languages), "Migrating to 0.18", removing defaults duplicated in code from each language's templates (written in the schema or the type instead), a new `docs/guarantees.md` entry "the config is filled with defaults and validated before `apply`" (naming the case list that verifies it) |

Each step merges with all cases of its column passing, and turns on the 1c template check for its language; #669 closes when step 6 merges with all 7 columns passing.
