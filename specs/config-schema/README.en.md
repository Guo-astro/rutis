# Shared cases for config schemas

[中文](README.md)

The case list of the [config schema design](../../docs/design-config-schema-2026-10-27.en.md) §8: the rules for filling defaults, validating and writing errors. Every language's SDK (Node, Bun, Python, Go, Swift, Kotlin), rutis-loader and `rutis-host check` run every case of [`cases.json`](cases.json) and compare word for word. This is not a TLA+ model and is not run by CI's `specs` job; each implementation's own tests run it.

## Format

```json
{ "cases": [ { "name": "…", "schema": { … }, "config": …, "then": { "config": … } | { "errors": ["config: …"] }, "warnings": ["config: …"] } ] }
```

- No `config`: no config (the same as `null`).
- `then.config`: it passes, and this is what `apply` gets (defaults filled; `1.0` passing `integer` written as the integer `1`). Compared by JSON equality; languages that tell integers from fractions (Rust, Go, Python) also compare whether it is an integer.
- `then.errors`: it fails; each line of the error, i.e. the text of the marked config error without the row name (design §5.1: sorted by path, at most 20 lines, the rest as `config: and N more problems`).
- `warnings`: extra keys, in the same sentences (the warnings of testing tools and of the loader).

## How each implementation runs them

- Run every case, compare `then` and `warnings`.
- Run every passing case a second time after changing every object and array of the first result in place (a key added, an item added): the second result still equals `then.config` (defaults are filled as copies, design §4, case 21).
- Only rules every language shares are here. Typed languages' own cases (Go's zero values and pointers, Swift's literal initial values, Kotlin's default arguments, Rust's `serde(default)` and `deny_unknown_fields`, the decoder errors of Rust `Definition`) and old Go manifests' `required` being ignored (design §7) are in their own tests.

## Details settled while implementing

What the design does not spell out word for word and was settled so that the 7 implementations agree; each has cases:

- Whether the root is an object, the root's `default` and a property's `default` are looked up through `$ref` and `allOf` (the schema itself, then its `$ref`, then its `allOf` members, the first found); a root schemars generates is often just a `$ref`.
- Defaults are also filled into objects that are items of an array or values of a dictionary (`additionalProperties` as a schema).
- The parts a schema takes in "through `$ref` and `allOf`" are in depth-first pre-order: the schema itself, then its `$ref` (with that one's own `$ref` and `allOf`), then each `allOf` member (expanded the same way), each once.
- Problems at one path: the type first (a wrong type reports only the type, whichever part's `type` it is); then part by part in the order above, each in the order `enum` / `const`, bounds, `anyOf` / `oneOf`; a problem exactly the same as another is reported once.
- `additionalProperties` is the first one written among the parts; when none is written, extra keys are warned about when one of the parts has `properties`; `true` or `{}` gives no warning.
- At one value, each `anyOf` / `oneOf` branch (told apart by where it is written in the schema document, i.e. its JSON Pointer; an object's identity works the same; `true` / `false` branches never recurse and need no key) is tried once: reaching it again through `$ref` while it is being tried does not match; reaching it again after it was tried gives that result again (with the value as it passed, integers normalised). This starts over at child keys and array items. So branches referring to themselves or to each other neither recurse without end nor take exponential time.
- A `$ref` that cannot be resolved (not `#/$defs/…` or `#/definitions/…`, or not found) is ignored, as if not written.
- Keys in paths are not cut (only `got` strings past 40 are).
- The root's path is `config`; `config` is also written before a first segment that is an index or a key that is not an identifier (`config["1st"]`).
- A branch of `anyOf` / `oneOf` is written by the first it has of `const`, `enum`, `type`.
- Names in `type` that are not known are ignored; an `items` that is an array (the old tuple form) is not checked.
