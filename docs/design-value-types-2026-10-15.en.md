# Cross-language value types: dates and times, binary, Map, Set, big integers, Decimal, UUID

[中文](design-value-types-2026-10-15.md)

Status: final, not implemented. Date: 2026-10-15. Base: `main` (after 0.10, with #400–#402 merged).
Sources: the draft and review in [#308](https://github.com/arcships/rutis/issues/308), [#355](https://github.com/arcships/rutis/issues/355), [#360](https://github.com/arcships/rutis/issues/360), maintainer decisions (2026-10-15, recorded in #308); "Passing values" in [the plugin API](guide/plugin-api.en.md); [the protocol design](design-protocol-plugins-2026-09-25.en.md) §6 (explicitly tagged values); [design philosophy](design-philosophy.en.md) §7.

Scope: the value layer of the session protocol, and the five implementations in Rust, Node, Bun, Python and Go together with their test tools; the Swift and Kotlin SDKs implement this document ([Swift SDK](design-swift-sdk-2026-10-12.en.md), [Android app hosting](design-android-runtime-2026-10-12.en.md)).

Out of scope: event forwarding between nodes (`NodeEvent` arguments are JSON; the new types are rejected there, see §7); whether data is equivalent or interchangeable between services (the service authors' job).

## 1. Decisions

| # | Decision |
| --- | --- |
| V1 | 0.11 does all nine values in one go: instant, date, time of day, binary, Map, Set, big integer, Decimal, UUID; together with the consistency fixes from #355 |
| V2 | Principle: **convert by default, never lose information**. Each value has one default counterpart in each language; when the other side has no identical type, it is converted to the closest representation that loses no information. An error is raised only when information really would be lost |
| V3 | Wire encoding: new tags at the same level as `reference`; ordinary JSON fields are never interpreted as types |
| V4 | Negotiation: one capability per value; when the peer has not declared it, **only this call fails** and the session continues; the protocol version number is not bumped |
| V5 | Date-times without a time zone (Python `datetime` without `tzinfo`, Kotlin `LocalDateTime`): interpreted as an instant in the **sender's local time zone** |
| V6 | Big integers are sent as big integers only when **explicit**; ordinary integers outside ±(2^53−1) are still an error. The type JS receives is therefore fixed: ordinary numbers are `number`, big integers are `bigint` |
| V7 | When instant precision differs, the receiver **truncates toward negative infinity** to its own precision; this is written into the boundary rules and is not an error |
| V8 | Map keys and Set elements can only be things that "are values in every language" (§5); if keys become duplicates after conversion (entries would be lost), this call fails |
| V9 | Go structs, typed slices and maps, and top-level `time.Time` all go through the same encoder and are checked one by one, no longer through `encoding/json` |
| V10 | `undefined` in data becomes `null` in every language (Python dicts no longer drop the key) |
| V11 | Rust `Value` gets new variants and is marked `#[non_exhaustive]`: the breaking change happens only this once |
| V12 | The test tools and the real runtimes are compared against the same expectation file, with all new values included |

## 2. Default counterparts

| Value | Tag | JS (Node, Bun) | Python | Go | Rust | Kotlin | Swift |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Instant | `instant` | `Date` | `datetime` (time-zone aware, UTC) | `time.Time` (UTC) | `Instant` | `java.time.Instant` | `Date` |
| Date | `date` | `"YYYY-MM-DD"` string | `date` | `rutis.Date` | `Date` | `LocalDate` | `Rutis.CalendarDate` |
| Time of day | `time` | `"HH:MM:SS[.f…]"` string | `time` (no time zone) | `rutis.TimeOfDay` | `TimeOfDay` | `LocalTime` | `Rutis.TimeOfDay` |
| Binary | `bytes` | `Uint8Array` | `bytes` | `[]byte` | `Vec<u8>` | `ByteArray` | `Data` |
| Map | `map` | `Map` | `dict` | `map[K]V` | `Vec<(Value, Value)>` | `Map` | `Dictionary` |
| Set | `set` | `Set` | `set` | slice (`map[K]struct{}` when declared as that) | `Vec<Value>` | `Set` | `Set` |
| Big integer | `bigint` | `bigint` | `rutis.BigInt` (a subclass of `int`) | `*big.Int` | `BigInt` | `BigInteger` | `Rutis.BigInt` |
| Decimal | `decimal` | string | `Decimal` | string | `Decimal` (string) | `BigDecimal` | `Decimal` |
| UUID | `uuid` | string (lowercase) | `UUID` | string (lowercase) | `Uuid` | `java.util.UUID` | `UUID` |

- Rust's `Instant`, `Date`, `TimeOfDay`, `BigInt`, `Decimal` and `Uuid` are small types of rutis-bridge's own (chrono and num-bigint are not pulled in), each with methods to convert to and from the common libraries; the same goes for Go's `rutis.Date` and `rutis.TimeOfDay`, and Swift's `Rutis.CalendarDate`, `Rutis.TimeOfDay` and `Rutis.BigInt`.
- **Strings in JS**: date, time of day, Decimal and UUID have no counterpart type in JS; they are received as strings with the full content. To send one as the original type, use the SDK constructors: `rutis.date('2026-10-15')`, `rutis.time('14:30')`, `rutis.decimal('1.10')`, `rutis.uuid('…')`; ordinary strings are still strings. The same goes for Go's Decimal and UUID (the named string types `rutis.Decimal` and `rutis.UUID`).
- **Big integers in Python**: receiving a `bigint` gives a `rutis.BigInt` (a subclass of `int`; arithmetic on it yields an ordinary `int`), and sending it back unchanged is still a big integer; an ordinary `int` out of range is an error that suggests `rutis.BigInt(n)`.
- **Decimal in Swift**: Foundation `Decimal` has only 38 significant digits; beyond that, this call fails (information would be lost).
- **Set in Go**: Go has no set type; it decodes according to the declared target type, and when the target is `any` it yields a deduplicated slice.

## 3. Wire encoding

The new values are at the same level as `reference` and `signal`; arrays and objects containing them are encoded as `list` / `record` instead (the same as when they contain references today).

| Tag | `value` | Rules |
| --- | --- | --- |
| `instant` | `"2026-10-15T03:22:15.123456789Z"` | RFC 3339, UTC (`Z`), 0–9 fractional digits, years 0001–9999 |
| `date` | `"2026-10-15"` | Years 0001–9999 |
| `time` | `"14:30:05.25"` | 24-hour clock, 0–9 fractional digits, no time zone |
| `bytes` | `"aGVsbG8="` | base64 (RFC 4648 §4, padded) |
| `bigint` | `"-123456789012345678901"` | Decimal, no leading zeros, at most 4300 digits (Python's default integer string limit) |
| `decimal` | `"-12.3400"` | Decimal, optionally with a fraction, no exponent; trailing zeros are kept (`1.10` differs from `1.1`); at most 4300 digits; NaN and infinity are rejected |
| `uuid` | `"0f8fad5b-d9cb-469f-a165-70867728950e"` | Lowercase, 8-4-4-4-12 |
| `map` | `[[k, v], …]` | Array of key-value pairs, in the sender's iteration order |
| `set` | `[e, …]` | Array of elements, in the sender's iteration order |

Examples: `{"type":"instant","value":"2026-10-15T03:22:15Z"}`; `{"type":"map","value":[[{"type":"data","value":"a"},{"type":"data","value":1}]]}`.

## 4. Capabilities and negotiation

- Capability names: `value-instant`, `value-date`, `value-time`, `value-bytes`, `value-map`, `value-set`, `value-bigint`, `value-decimal`, `value-uuid`. Declared in both the endpoint and the compat handshake (compat works like `sync-wait`: old implementations ignore extra fields in the handshake; Rust's `COMPAT_DECLARED` adds these names).
- A sender sends a tag only to a peer that has declared the matching capability; when the peer has not declared it, **this call fails** with an error saying which value the peer does not support, and the session continues.
- When the host forwards, it checks again against the capabilities of the **downstream** peer: when the upstream can send but the downstream cannot receive, the forwarded call fails.
- New implementations (Swift, Kotlin) may implement only a subset: a peer never sends them a type they have not declared.

## 5. Map keys and Set elements

- Allowed: strings, finite numbers, booleans, `bigint`, `date`, `time`, `decimal`, `uuid`.
- Not allowed: `instant` (a JS `Date` is an object, compared by identity as a key), `bytes`, `null`, arrays, objects, `map`, `set`, references. The sender rejects them before sending.
- When keys become duplicates after the receiver converts them (for example, the JS `Map` keys `1` and `true` merge in a Python dict), this call fails: entries would be lost.
- A `Map` with string keys is still sent with the `map` tag (not as an object), and the other side receives a `Map` / `dict`, not a plain object; plain JS objects are still sent as objects (`data`).

## 6. Sending rules per language

| Language | Rules |
| --- | --- |
| JS | `Date` → `instant` (invalid dates are an error); `Uint8Array`, `ArrayBuffer`, `DataView`, `Buffer` → `bytes`; `Map` → `map`; `Set` → `set`; `bigint` → `bigint`; SDK constructors → `date` / `time` / `decimal` / `uuid` |
| Python | Time-zone-aware `datetime` → `instant`; `datetime` without a time zone → in the local time zone (V5); `date` → `date`; `time` (time-zone-aware ones are rejected) → `time`; `bytes`, `bytearray`, `memoryview` → `bytes`; `set`, `frozenset` → `set`; `dict` with non-string keys and `rutis.Map` → `map`; `rutis.BigInt` → `bigint`; `Decimal` → `decimal`; `UUID` → `uuid`. `datetime`, `Decimal` and `UUID` are no longer passed as object references (#355) |
| Go | `time.Time` → `instant`; `rutis.Date` / `rutis.TimeOfDay` → `date` / `time`; `[]byte` → `bytes`; maps with non-string keys → `map`; `*big.Int` / `big.Int` → `bigint`; `rutis.Decimal` / `rutis.UUID` → `decimal` / `uuid`. Struct fields and typed slices and maps go recursively through the same encoder (V9), and no longer silently become strings or base64 |
| Rust | New variants of `Value` (V11); `Value::json()` returns an error on the new values (as it does for references); the Cordis generator maps `Uint8Array`, `Date`, `bigint`, `Map` and `Set` to the Rust types above |
| Kotlin, Swift | The types in §2; the value layer uses the SDK's own encoder and checks values one by one, without relying on the default encoding of `JSONEncoder` / kotlinx (which encodes dates as numbers and binary as base64) |

Instant precision (V7): JS milliseconds, Python microseconds, Swift roughly microseconds (`Date` is a `Double`), Go / Rust / Kotlin nanoseconds; a finer value received is truncated toward negative infinity.

## 7. Boundary rules (go into the plugin API docs)

1. Default conversion follows §2 and loses no information; when information would be lost, this call fails and the error says why.
2. Date-times without a time zone are interpreted in the sender's local time zone.
3. Instants are truncated toward negative infinity to the receiver's precision.
4. Ordinary integers outside ±(2^53−1) are an error; big integers must be explicit.
5. The restrictions on Map keys and Set elements are in §5; if keys become duplicates after conversion, the call fails.
6. When the peer does not support a value, this call fails and the session continues.
7. Event forwarding between nodes (`NodeEvent`, with JSON arguments) does not carry the new values: an event containing a new value is rejected when forwarded. Leaf events inside the same runtime process are delivered as usual.
8. `undefined` in data becomes `null`.

## 8. Breaking changes (0.11 migration notes)

- Python: `datetime`, `Decimal` and `UUID` used to be passed as object references and are now passed by value; the peer gets a value, not a proxy.
- Go: `time.Time`, `[]byte` and out-of-range integers inside structs used to go through `encoding/json` and become strings or base64, or go unchecked; now they are encoded per §6 or are an error.
- Keys whose value is `undefined` in a Python dict used to be dropped; now they are `null`.
- Rust: `Value` gets variants and is marked `#[non_exhaustive]`; exhaustive matches need one more arm.
- Values that used to be rejected (JS `Date`, `Map`, `Set`, binary, `bigint`, and so on) can now be passed, but only to peers that declared the capability: with runtimes of older versions, these calls still fail.

## 9. Tests

| Test | Content |
| --- | --- |
| Expectation file `weather.expected.json` | Round-trip samples for every value (sent from each language, received by each language), changing the current `throws` to `returns`; failure samples for truncation, local time zone, duplicate keys, out of range, and so on |
| `tests/value_roundtrip.rs` | The property-test generator adds the nine values and the key rules of §5 |
| Unit tests per language | Encoding and decoding, boundaries, error text |
| Cross-version (#209) | Runtime from the previous release ↔ current host: calls with new values fail and the session continues; the same when the host forwards to an old downstream |
| Test tools | Node `@arcships/rutis/testing`, Python `rutis.testing` and Go `rutistest` use the same expectations as the real runtimes |

No Q6.2.8 model check: only value tags are added; the rules for multi-party interaction are unchanged.

## 10. Phases

| Phase | Content |
| --- | --- |
| 1 | Protocol specification: the encoding and capabilities in this document go into protocol design §6; samples are added to the expectation file first (marked as not yet implemented) |
| 2 | Rust: `Value` variants, encoding and decoding, capabilities, forwarding checks |
| 3 | Node, Bun, Python, Go: their own encoding and decoding, capabilities, test tools (can run in parallel) |
| 4 | Consistency: all of the expectation file changed to `returns`, property tests, cross-version tests, docs (the plugin API value table, the per-language guides, guarantees) |

Phase 3 depends on phase 2; the consistency fixes from #355 are done with each language in phase 3 (not rejecting first and then supporting).

## 11. Differences from the draft

| Draft | This document | Reason |
| --- | --- | --- |
| 0.11 does only binary and dates; the rest is rejected | All nine values in 0.11 | Maintainer decision: plugin authors adapt only once, the protocol changes only once, and the Swift / Kotlin SDKs get the complete rules |
| `datetime` without a time zone is rejected | Interpreted in the sender's local time zone | Maintainer decision: default conversion first, no error |
| Python `date` and `time` are rejected | Two tags, `date` and `time` | #360 |
| The tag name `date` means an instant | Instants are `instant`; `date` is kept for calendar dates | Each of the three time values has its own name |
| An option for Python to send out-of-range integers as `bigint` automatically based on size | Only explicit big integers | #360: the type JS receives does not depend on the size of the number |
| Decimal and UUID use wrapper objects in languages without a counterpart type | Strings, with SDK constructors to send the original type | Maintainer decision: default conversion, no content lost |
