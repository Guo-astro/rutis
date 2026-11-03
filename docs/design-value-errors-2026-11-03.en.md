# One shape when a value cannot cross: NaN, cycles, Go's encoding errors, results that do not decode

[中文](design-value-errors-2026-11-03.md)

Status: draft, not implemented. Date: 2026-11-03. Baseline: `main` `d2a8d069` (0.20.1). Planned for: 0.21.

Issue: [#936](https://github.com/arcships/rutis/issues/936) (part of [#663](https://github.com/arcships/rutis/issues/663), D1 and D5). The issues implementing it are in §7.
Based on: [Plugin API](guide/plugin-api.en.md) "How values cross"; [diagnostics](design-diagnostics-2026-10-26.en.md) §4.3, §4.5; the 0.19.0 developer experience evaluation (#663).

## 1. Where things stand

What the 0.19 evaluation said: when a value cannot cross, most exits report `<service>.<method>: argument N cannot be sent: …` with `bad-value`, as the plugin API guide says; these do not. Each row was checked against the source; the third column is **what developers see today, word for word**.

| Exit | What developers see today | Why (checked against the source) |
| --- | --- | --- |
| Node, Bun sending NaN / Infinity | `TypeError: non-finite number`, `code` and `data` both empty | The whole frame is encoded outside the per-argument wrap: `checkData` does not look at numbers (`node/rutis-runtime/src/session.mjs:188`), and the `JSON.stringify` hook refuses it (`codec.mjs:6`), which `#request` calls at `session.mjs:551` |
| Node, Bun sending a Map that holds itself | `test.take: argument 1 cannot be sent: a value nested deeper than 64 levels cannot cross between processes` | A Map or Set goes through `#encodeTagged`, which checks only the depth (`session.mjs:360-363`) |
| Python sending a cycle | `weather.tomorrow: argument 1 cannot be sent: value nested deeper than 64 levels, the most a value crossing processes may nest` | `_data` checks the depth only, not a cycle (`python/rutis/rutis/peer.py:648-675`) |
| Go sending a cycle (a pointer to itself, a slice holding itself) | `… cannot be sent: value nested deeper than 64 levels, the most a value crossing processes may nest` | `go/rutis/internal/convert/encode.go:142-147`: a struct field also adds a level, so 64 levels is hit before `maxHops` and its sentence is never reached |
| Swift sending a cycle | `weather.tomorrow: argument 1 cannot be sent: value nested deeper than 64 levels, the most a value crossing processes may nest (a cyclic reference?)` | Only the depth is checked, and the parenthesis guesses "a cycle?": `swift/rutis/Sources/Rutis/Codec/ValueCodec.swift:147-148`, `Codec/Encoder.swift:55-57`; taking a value in does the same (`Session/Peer.swift:662,741-742`) |
| Go returning a channel / NaN | `chan int cannot cross between processes`, `the number NaN is not finite: only finite numbers cross`: no `bad-value`, no method, no "the result" | A method's value is turned into a peer value inside the method call (`convert.Results`, `convert.go:244-258`), so the failure becomes that method's own error and takes the throw path of `respondTo`, which `ofResult` (`peer/crossing.go:68-75`) never sees |
| Go sending NaN as an argument | `argument 1: the number NaN is not finite: only finite numbers cross`: no method, no `bad-value` | `Service.Call` returns the error of `convert.ToPeerArgs` as it is (`go/rutis/internal/runtime/runtime.go:1153`) |
| A result that does not decode (Go gets an object where it wants a string; a Rust host gets 42 where it wants a string) | `cannot decode {"text":"cloudy"} as string: json: cannot unmarshal object into Go value of type string`; ``invalid type: integer `42`, expected a string`` | Go: `decode.go:70`, returned as it is by `Service.Call`; Rust: `decode_value` puts serde's own text into `Error::Value` (`crates/rutis-bridge/src/session/objects.rs:784`, `:809`). The text itself is right (the principle in §4); what is missing is the prefix saying which call's result it is |
| Python receiving a TypeScript function | `<function from the host>` | `peer.py:694-695` |
| Python returning a function into a Rust `serde_json::Value` | `the value holds a live Cordis object or function that its Rust type cannot represent` | `objects.rs:809`: the sentence has nothing to do with Cordis |
| Node receiving another row's function | `console.log` prints `[Function (anonymous)]`; `String(fn)` prints `() => { check(); return this.#requestSync('call', { reference: id }, args) }` | A received function is a plain arrow function with no display of its own (`session.mjs:474`) |
| Kotlin receiving a function or object | `RemoteFunction(7)`, `RemoteObject(7)` | `advanced/RemoteFunction.kt:25`, `advanced/RemoteObject.kt:29` |

Checking the source also found two things the evaluation's table does not have:

- **Rust does not refuse a NaN; it sends `null`.** `Exact` hands scalars to `serde_json::value::Serializer` (`objects.rs:380-396`), which uses `Value::from` for an f64, and a non-finite number becomes `Json::Null` (serde_json's `From<f64> for Value` says so). Every other language refuses it; Rust quietly sends a different value.
- **Node's `String(fn)` prints the runtime's own source to the developer.** The line above is what it prints.
- **"Nested too deep" is worded four ways today**, and a cycle can only be said through it; Swift even guesses in a parenthesis:
  - Node, Bun: `a value nested deeper than 64 levels cannot cross between processes` (`session.mjs:102`);
  - Python, Go, Kotlin: `value nested deeper than 64 levels, the most a value crossing processes may nest` (`peer.py:520-523`, `peer.go:607`, `internal/peer/Peer.kt:63-65`);
  - Swift: the last one with ` (a cyclic reference?)` (`Codec/ValueCodec.swift:148`);
  - Rust: `value nested deeper than 64 levels, the most a value crossing a session may nest` (`rpc.rs:3136`).

Also, a `bad-value`'s `data` is `{part, argument?, limit?}` today (diagnostics §4.3): a program cannot tell a NaN from a cycle without reading the text.

## 2. What to solve

1. When a value cannot cross, there is one shape: the prefix (which call, which argument or the result), one sentence of reason, the code `bad-value`, and `data`.
2. A cycle is reported as a cycle, not as "nested deeper than 64 levels".
3. A value that arrives and does not decode says which method's argument or result it is, and then repeats the decoder's own sentence (serde says ``invalid type: integer `42`, expected a string``, Go says `json: cannot unmarshal object into Go value of type string`); the prefix says only where.

## 3. Decisions

**D1 One frame, one sentence of reason.** Every exit where a value cannot cross reads `<call>: argument N cannot be sent: <reason>`, `<call>: the result cannot be sent: <reason>`, `<call>: the error <Name> cannot be sent: <reason>`, or `the payload of <event> cannot be sent: <reason>`; the code is always `bad-value`, `data` is always `{part, argument?, reason, limit?}`. The reason is one sentence that names only what the developer wrote (service, method, argument, type name) and says `cannot cross between processes` (Python's "cannot cross the boundary" goes). Rewording is allowed here because rutis made the sentence itself: there is no underlying error (the principle in §4, D6). **Dropped**: a wording per exit — programs would still read text, and every row of the evaluation came from exactly that.

**D2 The check is before encoding, the same in every language.** The walk over the value happens before the value is encoded, and it is the walk the runtime already makes (data, lists, records, tagged values, Map / Set), not a failure of `JSON.stringify`, `json.Marshal` or serde. Node and Bun's `checkData` therefore walks into Map / Set and records (today plain data only), and the number check moves out of `codec.mjs`; Python's `_data`, `_encode` and `_encode_tagged` carry `seen`; Go's `toPeerValue` carries `seen` beside `levels` and `hops`. **Dropped**: catching it in the codec — by then it no longer knows which call's argument it is.

**D3 A cycle before the depth.** Each node asks first "is it already on this path", then the depth; `seen` holds the containers on the path and is emptied on the way back up, so the same object in two sibling places (`[shared, shared]`) is not a cycle. A cycle longer than 64 levels (it repeats only after 64) is still reported as too deep: that is the one thing it can say. **Dropped**: leaving it to the depth — that is row 2 of #936, where the developer is told to change a structure that is a cycle.

**D4 The depth limit stays at 64 levels, in one sentence.** It is worded four ways today (§1), and Swift guesses at a cycle in a parenthesis; it becomes `the value nests deeper than 64 levels`. The limit stays: it keeps one send from running a process's stack out, and it bounds what the far end takes; without it a value is bounded only by the 16 MiB message limit. **Dropped**: removing the limit.

**D5 Non-finite numbers: `the number NaN is not finite`.** "NaN", "Infinity" and "-Infinity" are the value the developer wrote; Python's `NaN cannot cross processes` today and Go's `… : only finite numbers cross` both become this sentence. Rust's `Exact` refuses in `serialize_f32` / `serialize_f64` (today it becomes `null`, §6). **Dropped**: letting a NaN become `null` — that is what Rust does today, and two calls of one plugin then get different values instead of an error. Today's `non-finite number` is what Node's encoder says of itself, with no underlying error behind it, so rewording it is no paraphrase (the principle in §4).

**D6 A value that does not decode: the raw sentence, one prefix, one code each.** A value that arrived and does not decode was refused by serde, `encoding/json`, kotlinx.serialization or Swift's `ValueCodec`, and their sentence is more exact than any rewrite: ``invalid type: integer `42`, expected a string`` says at once what was expected and what came, `json: cannot unmarshal object into Go value of type string` says what Go saw. So the body of the message is that sentence, and rutis adds only a prefix answering "which of my calls, which argument": `<service>.<method>: argument N: <the sentence>` (code `bad-argument`), `<service>.<method>: the result: <the sentence>` (code `bad-result`). `data` stays `{service|object, method, argument?, expected?}`, where `expected` comes from the declared type, not from the message. **Dropped**: paraphrasing it as `<call>: argument 1 should be a string, got an object` (what Rust's `marked.rs`, Go's `refusals.go`, Kotlin's `Refusals.kt` and Swift do today) — `got an object` repeats what the decoder already said, and often says less; this design changes that back. **Dropped**: reusing `bad-value` — its frame says "cannot be sent", but the value did arrive and this type does not take it. **Dropped**: the raw sentence with no prefix — the developer would not know which call; or no code — a program still needs one.

**D7 An event's payload gets its own name.** An event that crosses a session is a control call (`events.dispatch`), and a bad payload reports `events.dispatch: argument 1 cannot be sent: …` today: an internal name, and an argument number that means nothing to the developer. The emitting side **describes that call as the event's name** (the session's call methods take one more description), so it reads `the payload of weather/asked cannot be sent: …` with `part` `payload`. An event dispatched in this process does not cross a session and has no such error.

**D8 A reference shows as `<function of another process>` (with a row, `<function of row weather in node>`).** Three kinds, one shape: `<function of …>`, `<object of …>`, `<asynchronous result of …>`. Today Python says "from the host" (it came from another row), Node prints `[Function (anonymous)]` and the runtime's source, Kotlin prints `RemoteFunction(7)` (an internal id). A row's name is not on the wire: a reference carries only `origin` (a chain of call ids), which names the calling endpoints, not the row that made the value. **Dropped**: putting the row in a reference now — `WireValue::Reference` is `deny_unknown_fields` (`crates/rutis-bridge/src/session/protocol.rs:8,31,42-47`), so one more key ends the session on an older host; it needs a protocol version first. This round's wording promises no row name; the row name belongs to the protocol change that wants it (S7 of §7).

**D9 Only that call fails.** Every row above fails only that call (the sender gets an error, the receiver gets a throw) and the session goes on; only "over the 16 MiB message limit" is still decided after the whole frame is encoded (`part` plus `limit`). **Dropped**: ending the session on a bad value — that was the way before 0.18.

## 4. Rules

### The principle: the raw error first

The body of a message is the underlying error's own words, verbatim; rutis adds only a short prefix answering "which of my calls is this, which argument".

- What the underlying error says (serde, `encoding/json`, kotlinx.serialization, Swift's `ValueCodec`, Go's `reflect`) is what the developer reads: ``invalid type: integer `42`, expected a string``, `json: cannot unmarshal object into Go value of type string`, `KeyError: 'town'`.
- The prefix says only where: `weather.ask: `, `argument 2: `, `the result: `, `the payload of weather/asked: `. No paraphrase, no summary, nothing the underlying error already said repeated.
- Only an error rutis made up itself is rewritten into plain words — it has no underlying error: a synchronous wait cycle's `… path: test:5 -> …`, Node's bare `non-finite number`, `row <x> is not loaded in this runtime`, the four wordings of "nested too deep", `a symbol cannot cross between processes`. Those sentences are this design's to set (§4.2); they have no original to keep.
- When in doubt, keep the original.

### 4.1 The frame

When a value cannot cross, the error the sending side throws has one of these `message`s, word for word:

| What cannot cross | The text | `part` |
| --- | --- | --- |
| Argument N of a call (from 1) | `<service>.<method>: argument N cannot be sent: <reason>` | `argument`, `argument: N` |
| A control call (no service, `events.on` say) | `<method>: argument N cannot be sent: <reason>` | as above |
| A call of a function reference | `a function: argument N cannot be sent: <reason>` | as above |
| The result | `<service>.<method>: the result cannot be sent: <reason>` | `result` |
| An error returned (over the message limit) | `<service>.<method>: the error <Name> cannot be sent: <reason>` | `error` |
| An event's arguments (D7) | `the payload of <event> cannot be sent: <reason>` | `payload` |
| The arguments as a whole (not a list of them) | `<service>.<method>: the arguments cannot be sent: <reason>` | `arguments` |

The same in every language, in the same order of words. Node and Bun keep the class they throw today (`TypeError` / `RangeError`), Python `ValueError`, Go, Swift and Kotlin `Error`, Rust `Error::Value`: **the class is the language's habit, and a program decides by `code` only**. **Dropped**: one class everywhere — code using `instanceof` or `except` would change behaviour silently.

### 4.2 The reasons (a closed set)

A reason is one sentence that says nothing about the runtime's internals. `data.reason` is the name a program uses:

| `reason` | The one sentence | Who reports it |
| --- | --- | --- |
| `non-finite` | `the number NaN is not finite`, `the number Infinity is not finite`, `the number -Infinity is not finite` | every language (Rust says nothing today, D5) |
| `cycle` | `the value holds itself (a cycle)` | every language (Python, Go, Kotlin and Swift say too deep today) |
| `too-deep` | `the value nests deeper than 64 levels` | every language (four wordings today, Swift's with `(a cyclic reference?)`, §1) |
| `type` | `a <type name> cannot cross between processes`: `a symbol`, `a chan int`, `a module`; where Node says today what to do instead in the plain-data position (`a WeakMap cannot cross between processes: pass plain data instead`), that tail stays | each language's existing sentence, with the article and `between processes`; the type name is written as that language writes it (Go `chan int`, Python `module`) |
| `tag` | `the far end does not take <tag> values (it did not declare the capability value-<tag>)` | the sentence it has today, unchanged |
| `limit` | `the message would be <n> bytes, over the limit of <limit> bytes (<n> MiB)` | the sentence it has today, unchanged |

The `type` row is each language's own rule about which values do not cross; this design only unifies the sentence: `a <type name> cannot cross between processes`. Apart from these three (`non-finite`, `cycle`, `too-deep`) this design changes no existing sentence of a reason.

### 4.3 Codes and `data`

- Cannot be sent: `bad-value`, `data` = `{part, argument?, reason, limit?}`. `part` is `argument`, `arguments`, `result`, `error` or `payload`; `argument` counts from 1; `reason` is one of the six in §4.2; `limit` (bytes) is there only when `reason` is `limit`.
- An argument that does not decode: `bad-argument` (unchanged), `data` = `{service|object, method, argument, expected}`.
- A result that does not decode: `bad-result` (new), `data` = `{service|object, method, expected}`.

In the diagnostics design's code table (§4.3) this design **touches two rows only**: the `bad-value` row (`data` gains `reason`, `part` gains `payload`) and the new `bad-result` row. The rest of the table is not this design's — `already-provided`, `no-method`'s `provider` and the others belong to the draft of [#937](https://github.com/arcships/rutis/issues/937) (`docs/design-error-messages-2026-11-03.md`); the two land in one table, and whichever PR lands second rebases. This design does not copy the table.

### 4.4 Where the check happens

Every language checks in the same step and the same place: while turning the value into its wire form, before the value is done.

- **Node / Bun**: `checkData` walks into Map, Set, records and tagged values (plain data only today), asks `seen` before the depth, and checks numbers there; the hook in `codec.mjs` stays as a backstop (for frames that name no call, such as the handshake), which the call path no longer reaches.
- **Python**: `_data`, `_encode` and `_encode_tagged` all carry `seen` and `depth`, in the same order as Node.
- **Go**: `convert.toPeerValue` carries `seen` (`map[uintptr]bool`, deleted on the way back up) beside `levels` and `hops`; numbers are checked in the `float32` / `float64` arm.
- **Rust**: `Exact`'s `serialize_f32` / `serialize_f64` check finiteness before `serde_json` sees them (they do not today); levels stay where they are today (`fits` against `MAX_DEPTH` before sending, `rpc.rs:3139`; `MAX_FRAME_DEPTH` before taking a frame in), with only the sentence changed; `Exact` counts levels too, or a value pointing at itself through an `Rc` recurses inside `arg` and never reaches `fits`.
- **Swift / Kotlin**: their encoders make the same three steps in the same order. Both check the depth today (`Codec/ValueCodec.swift:115`, `Codec/Encoder.swift:55`, `internal/peer/Peer.kt:420,508`) and refuse non-finite numbers (the cases `swift/nan` and `kotlin/nan` of the case table), but neither looks for a cycle, and both report one as too deep (Swift even guesses in a parenthesis); `seen` and the three sentences are added as this rule says, checked by S8's cases.

Levels stay as they are: an array, an object, a Map, a Set and a record are each one level; a call's argument list is not a level.

### 4.5 A value that arrives and does not decode: the raw sentence, one prefix

- Only a runtime that decodes into declared types has this error: Go, Rust, Swift, Kotlin. Node, Bun and Python declare no types, so a received value is what it is and this line cannot happen.
- The body of the message is the decoder's own sentence, verbatim; rutis adds a prefix: `<service>.<method>: argument N: <the sentence>` (code `bad-argument`), `<service>.<method>: the result: <the sentence>` (code `bad-result`); a method of an object reads `<Type>.<method>: …`. The prefix says where, the sentence is untouched, and no summary of ours is added.
- Which sentence that is, per language:
  - **Rust**: serde's, verbatim — ``invalid type: integer `42`, expected a string``, ``invalid type: null, expected a string``. `of_decoding` in `marked.rs` stops paraphrasing it into `should be a string, got an integer`.
  - **Go**: `encoding/json`'s, verbatim — `json: cannot unmarshal object into Go value of type string`, `json: cannot unmarshal array into Go value of type int`; `convert.FromPeer`'s own refusals (`a list cannot be decoded as string`, `a function cannot be decoded as int` and the like, `convert/decode.go:70`) are rutis's, and keep the words they have today.
  - **Swift**: `ValueCodec`'s own `cannot decode an object into String` (`Codec/ValueCodec.swift:232`) — rutis's, already clear, and only the prefix is added.
  - **Kotlin**: generated code keeps the decoder's sentence (kotlinx.serialization's `SerializationException`); `Refusals.kt:32-45` no longer builds `should be … got …`. Only a case with no underlying error, such as a missing argument, is said in plain words.
- What rutis made up itself, with no underlying error, keeps its plain words today, verbatim: `argument 1 is missing`, `<service>.<method> takes 1 argument, got 2`, `the value holds a function that its Rust type cannot represent` (the `live Cordis` of today's sentence goes), `the value holds a tagged value (an instant, bytes, a map, ...) that its Rust type cannot represent`.
- `data` keeps its part: `{service|object, method, argument?, expected?}`. `expected` is the declared type (Go's `Expected(t)`, the `expected` Rust reads out of serde's sentence); it is for programs, not a substitute for the message.

### 4.6 An event's payload

The emitting side describes the dispatch as the event's name (D7), so any value in the payload that cannot cross reports `the payload of <event> cannot be sent: <reason>` with `part` `payload`. `emit` does not wait for a result, so that error is reported on the emitting side as one line (`rutis: emit of <event> from row <row> failed: <error>`); `parallel`, `serial`, `bail` and `waterfall` throw it to the caller.

### 4.7 How a reference shows

A received function, object or asynchronous result shows, wherever the language prints values (`util.inspect` and `String` in Node, `repr` in Python, `toString` / `description` in Kotlin and Swift, `%v` in Go), as one line:

- `<function of another process>`, `<object of another process>`, `<asynchronous result of another process>`: the form today (D8).
- Once the wire carries the row (another protocol change): `<function of row weather in node>`, where the word after `in` is the `implementation.name` the endpoint that made the value greeted with.

Neither prints the runtime's own source, an internal id, `Cordis`, or `host` — words the developer did not write.

### 4.8 Where the languages must differ

| Where | Why | Example |
| --- | --- | --- |
| The type name | It is written as the developer wrote the type | Go `a chan int cannot cross between processes`; Python `a module cannot cross between processes`; Node `a symbol cannot cross between processes` |
| Whether there is a decode at all | Only a runtime with declared types has one | Node, Bun and Python have none; Go, Rust, Swift and Kotlin have one |
| The error class | The language's habit (D1) | `TypeError` / `RangeError` (Node, Bun), `ValueError` (Python), `Error` (Go, Swift, Kotlin), `Error::Value` (Rust) |
| The words of what is expected | Each language's existing `Expected` | Go expects `a list of integers` for `[]int`; Rust `a list` for `Vec<i64>` |

Everywhere else — the frame, the three sentences of reason, the code, `data`, `part`, arguments counted from 1 — every language is the same, word for word.

## 5. What developers see

### 5.1 Plugin authors (TypeScript)

The plugin of the row `weather`, calling the `forecast` of another row:

```ts
// rows/weather/src/index.ts
import { definePlugin } from '@arcships/rutis'

export default definePlugin({
  apply(ctx) {
    const forecast = ctx.use('forecast')

    ctx.provide('weather', {
      // Works: the result comes back as data ({city, slug})
      today(city: string) {
        return { city, slug: forecast.tomorrow(city) }
      },
      // Works: the event's payload crosses, its listener is in another row
      asked(city: string) {
        ctx.emit('weather/asked', { city, degrees: 18 })
      },
    })

    ctx.on('weather/answered', (answer: { city: string; degrees: number }) => {
      console.log(answer.city, answer.degrees)   // Oslo 18
    })
  },
})
```

When it works, `ctx.use('forecast').tomorrow('Oslo')` gives `"Oslo tomorrow"`; `ctx.emit('weather/asked', …)` returns at once; `await ctx.parallel('weather/asked', { city: 'Oslo', degrees: 18 })` waits for every listener.

When it fails, one row each, word for word:

| How it is caused | The output | Code and `data` | How to fix it |
| --- | --- | --- | --- |
| `forecast.week('Oslo', Number.NaN)` | `forecast.week: argument 2 cannot be sent: the number NaN is not finite` (`TypeError`) | `bad-value`, `{part: 'argument', argument: 2, reason: 'non-finite'}` | Send no NaN: `null`, or no such argument |
| A `-Infinity` somewhere in the value | `forecast.week: argument 1 cannot be sent: the number -Infinity is not finite` | `bad-value`, `{part: 'argument', argument: 1, reason: 'non-finite'}` | as above |
| The argument holds itself: `const a = {}; a.self = a` | `forecast.week: argument 1 cannot be sent: the value holds itself (a cycle)` (`TypeError`) | `bad-value`, `{part: 'argument', argument: 1, reason: 'cycle'}` | Drop the field that points back; `structuredClone` keeps the cycle too, so build a new object |
| The argument is a Map holding itself (the far end declared `value-map`) | the same sentence | as above, `reason: 'cycle'` | as above |
| The argument nests 70 levels | `forecast.week: argument 1 cannot be sent: the value nests deeper than 64 levels` (`RangeError`) | `bad-value`, `{part: 'argument', argument: 1, reason: 'too-deep'}` | One level fewer: flatten the middle one into an object |
| The argument holds a `Symbol('x')` | `forecast.week: argument 1 cannot be sent: a symbol cannot cross between processes` (`TypeError`) | `bad-value`, `{part: 'argument', argument: 1, reason: 'type'}` | Send a string or a number |
| The result holds a NaN: `today() { return { degrees: NaN } }` | the caller gets `weather.today: the result cannot be sent: the number NaN is not finite` | `bad-value`, `{part: 'result', reason: 'non-finite'}` | as above; this row itself goes on, and so does the session |
| `await ctx.parallel('weather/asked', { city: 'Oslo', degrees: Number.NaN })` | `the payload of weather/asked cannot be sent: the number NaN is not finite` | `bad-value`, `{part: 'payload', reason: 'non-finite'}` | as above |
| `ctx.emit('weather/asked', { city: 'Oslo', degrees: Number.NaN })` | the emitting side reports one line: `rutis: emit of weather/asked from row weather failed: TypeError: the payload of weather/asked cannot be sent: the number NaN is not finite` | as above | as above |
| `console.log(forecast.tomorrow)` | `<function of another process>` | — | — |
| `console.log(forecast)` | `<object of another process>` | — | — |

`String(forecast.tomorrow)` prints the same `<function of another process>`, no longer the runtime's source.

### 5.2 Python: only where it differs

```python
# rows/forecast/src/forecast.py
from rutis import define_plugin


class Forecast:
    async def tomorrow(self, city):
        return f"{city} tomorrow"


plugin = define_plugin(name="forecast", provides={"forecast": Forecast})
```

| How it is caused | The output | Code and `data` |
| --- | --- | --- |
| `ctx.use('forecast').week('Oslo', float('nan'))` | `forecast.week: argument 2 cannot be sent: the number NaN is not finite` | `bad-value`, `{part: 'argument', argument: 2, reason: 'non-finite'}` |
| A cycle (in a list, a dict or a dataclass alike) | `forecast.week: argument 1 cannot be sent: the value holds itself (a cycle)` | `bad-value`, `{part: 'argument', argument: 1, reason: 'cycle'}` |
| 65 levels | `forecast.week: argument 1 cannot be sent: the value nests deeper than 64 levels` | `bad-value`, `{part: 'argument', argument: 1, reason: 'too-deep'}` |
| A module object, a generator | `forecast.week: argument 1 cannot be sent: a module cannot cross between processes` | `bad-value`, `reason: 'type'` |
| `repr(forecast.tomorrow)` (a function of another row) | `<function of another process>` | — |

Python has no "does not decode": a received value is not decoded into a declared type.

### 5.3 Go: only where it differs

```go
// a result to decode into a string
var text string
if err := forecast.Call(ctx, "tomorrow", &text, "Oslo"); err != nil {
	return err
}
```

| How it is caused | The output | Code and `data` |
| --- | --- | --- |
| `forecast.Call(ctx, "week", nil, "Oslo", math.NaN())` | `forecast.week: argument 2 cannot be sent: the number NaN is not finite` | `Code()` is `bad-value`, `Data()` is `{part: "argument", argument: 2, reason: "non-finite"}` |
| The method returns `make(chan int)` | `weather.today: the result cannot be sent: a chan int cannot cross between processes` | `Code()` is `bad-value`, `Data()` is `{part: "result", reason: "type"}` |
| The method returns a `*Node` pointing at itself, a `[]any` holding itself | `weather.today: the result cannot be sent: the value holds itself (a cycle)` | `Code()` is `bad-value`, `Data()` is `{part: "result", reason: "cycle"}` |
| The method returns 65 levels | `weather.today: the result cannot be sent: the value nests deeper than 64 levels` | `Code()` is `bad-value`, `Data()` is `{part: "result", reason: "too-deep"}` |
| A method that wants a `string` is given back an object | `forecast.tomorrow: the result: json: cannot unmarshal object into Go value of type string` | `Code()` is `bad-result`, `Data()` is `{service: "forecast", method: "tomorrow", expected: "a string"}` |
| An argument is not the declared type | `weather.today: argument 1: json: cannot unmarshal object into Go value of type string` | `Code()` is `bad-argument`, `Data()` is `{service, method, argument: 1, expected: "a string"}` |

### 5.4 Rust: only where it differs

```rust
// calling another row's service, the result decoded into the declared type
let text: String = call_service(&ctx, "forecast", "tomorrow", ("Oslo",)).await?;
```

| How it is caused | The output | Code and `data` |
| --- | --- | --- |
| `call_service(&ctx, "forecast", "week", ("Oslo", f64::NAN)).await` | `forecast.week: argument 2 cannot be sent: the number NaN is not finite` | `bad-value`, `{part: "argument", argument: 2, reason: "non-finite"}` |
| The result is 42 and a `String` is wanted | ``forecast.tomorrow: the result: invalid type: integer `42`, expected a string`` | `bad-result`, `{service: "forecast", method: "tomorrow", expected: "a string"}` |
| The result holds a function and a `serde_json::Value` is wanted | `forecast.tomorrow: the result: the value holds a function that its Rust type cannot represent` | `bad-result`, `{service: "forecast", method: "tomorrow"}` |
| The result holds an instant and a `serde_json::Value` is wanted | `forecast.tomorrow: the result: the value holds a tagged value (an instant, bytes, a map, ...) that its Rust type cannot represent` | `bad-result`, `{service: "forecast", method: "tomorrow"}` |

Rust sends a NaN as `null` today (§1); afterwards it reports the error like every other language, and serde's own text appears in no line.

### 5.5 Host owners (applications embedding rutis, products using `rutis-host` as their runtime)

- One error across processes: `code` and `data` arrive at the caller as they are (diagnostics §4.4). A program that switches on `code` gains one arm, `bad-result`; a `bad-value`'s `data` gains a key, `reason` (code that reads by name need not change).
- `rutis-host call weather.today Oslo` prints the line of §5.1 as it is, with no wrapper of its own.
- A host sees serde's and `encoding/json`'s own text (``invalid type: integer `42`, expected a string``, say). That is intended: the original is the most exact, and rutis adds only a prefix (the principle in §4).
- The line reported when an emit fails (§4.6) is in the same place with the same prefix as today; only its inner sentence changes.

### 5.6 Those deploying

Nothing visible changes: no new configuration key, no new file, no new port, no protocol version; `rutis-host` and the runtimes are upgraded as usual.

### 5.7 Concepts to learn

This round adds: one error code (`bad-result`), one `data` field (`reason`), one display form (`<… of another process>`). Against the per-layer concept budget of #663, the "values" layer goes from 2 concepts (`bad-value`, `bad-argument`) to 3, which is within it; and one misunderstanding goes away: "nested deeper than 64 levels" no longer also means a cycle. No new configuration key, no new API.

## 6. Compatibility and migration

**The text changes** (comparisons and documents are locked word for word):

| Today | Becomes |
| --- | --- |
| `TypeError: non-finite number` (Node, Bun, an argument) | `<call>: argument N cannot be sent: the number NaN is not finite`, and so on |
| `NaN cannot cross processes` (Python) | `the number NaN is not finite` |
| `a value that holds itself (a cycle) cannot cross between processes` (Node, Bun) | `the value holds itself (a cycle)` |
| `a value nested deeper than 64 levels cannot cross between processes` (Node, Bun) | `the value nests deeper than 64 levels` |
| `value nested deeper than 64 levels, the most a value crossing processes may nest` (Python, Go, Kotlin) | the same |
| `value nested deeper than 64 levels, the most a value crossing processes may nest (a cyclic reference?)` (Swift) | `the value nests deeper than 64 levels` (the parenthesis guessing at a cycle goes: a cycle is reported as one) |
| `value nested deeper than 64 levels, the most a value crossing a session may nest` (Rust) | the same |
| `module cannot cross the boundary` (Python) | `a module cannot cross between processes` |
| `argument 1: the number NaN is not finite: only finite numbers cross` (a Go caller) | `forecast.week: argument 2 cannot be sent: the number NaN is not finite` |
| `chan int cannot cross between processes` (a Go result) | `weather.today: the result cannot be sent: a chan int cannot cross between processes` |
| `cannot decode {…} as string: json: …` (a Go result) | ``forecast.tomorrow: the result: json: cannot unmarshal object into Go value of type string`` (the sentence kept, only the prefix added) |
| ``invalid type: integer `42`, expected a string`` (a Rust result) | ``forecast.tomorrow: the result: invalid type: integer `42`, expected a string`` (the sentence kept, only the prefix added) |
| `forecast.tomorrow: argument 1 should be a string, got an object` (arguments in Rust, Go, Kotlin and Swift today) | `forecast.tomorrow: argument 1: <the decoder's sentence>` — the paraphrase goes |
| `the value holds a live Cordis object or function that its Rust type cannot represent` | `forecast.tomorrow: the result: the value holds a function that its Rust type cannot represent` (`live Cordis` goes, the rest stays) |
| `<function from the host>` (Python) | `<function of another process>` |
| `[remote object]` (a Node object), `[Function (anonymous)]` (a Node function) | `<object of another process>`, `<function of another process>` |
| `RemoteFunction(7)`, `RemoteObject(7)` (Kotlin) | `<function of another process>`, `<object of another process>` |

**Behaviour changes**:

- Rust sending NaN or Infinity changes from "becomes `null`" to an error. Code relying on the `null` changes it itself.
- A Go method's value is no longer turned into a peer value inside the method: `Results` gives the value back as it is and `respondTo` encodes it (the only place that knows whose result it is). A method returning a channel now fails its call with `bad-value` instead of its own error.
- A cycle in Python, Go, Kotlin and Swift is reported as a cycle instead of too deep (Swift's parenthesis guessing goes too); `reason` lets a program tell them apart.

**The codes do not change**: `bad-value`, `bad-argument` and `bad-result`, and the fields of `data`, all stay; where the underlying error speaks, what changes is the sentence — it is now the original, with only a prefix saying where (`weather.ask: `, `argument 2: `, `the result: `). A program that decides by the code changes nothing.

**The documents**: the plugin API guide's "How values cross" (`docs/guide/plugin-api.en.md:938`) writes `data` as `{part, argument?, limit?}`; it gains `reason`, the `payload` part, and `bad-result` in its "the name is not guaranteed, look at the code" sentence, in both languages.

**The protocol does not change**: no new frame, no new field, no version raised. A `bad-value`'s `data` gains a `reason` key, and an older end ignores it, because an error's `data` is never decoded by structure: it travels as plain JSON with the error's graph.

- Rust, taking an error in, keeps the whole `graph` as `Option<serde_json::Value>` (`crates/rutis-bridge/src/session/protocol.rs:87-95`), and `Error::data()` picks from it by key (`session/mod.rs:199-213`, `graph_root` / `graph_value`);
- Every other language reads the graph by key: Node and Bun (`node/rutis-runtime/src/errors.mjs:170`, `bun/rutis-bun/src/errors.ts:170`), Python (`peer.py:114-135`, `_code_and_data`), Go (`peer/errors.go:350`, `codeAndData`), Kotlin (`internal/peer/Errors.kt:408`), Swift (`Errors.swift:385`).

Where `deny_unknown_fields` is, and where it is not: it is on the **shape of a frame** — the keys of `Frame`, the top-level keys of `Failure` (`name`, `message`, `graph`), and the keys of `WireValue` (`protocol.rs:31,88,155`); it is not inside `graph`, and not inside a node's `data` value, which is plain JSON (the `value` of `{type: "data", value: …}` travels as it is). So a key may be added inside `data`, as `limit` was in 0.18; a field added to a frame or to the top of `Failure` may not.

One exception to state plainly: a `bad-value` the Rust side made itself does not read its `data` from the wire, it works it back out of the text (`rpc/crossing.rs:61-95`, `value_data`). A `reason` cannot be worked out there, so `Error::Value` carries `reason` explicitly instead of guessing it from the text (S4).

**Each line's version**:

| Line | Version | This design | An old end and a new one |
| --- | --- | --- | --- |
| Session protocol | 4 (`node/rutis-runtime/package.json:18`, `bun/rutis-bun/package.json:18`, `ENDPOINT_PROTOCOL` in `node/rutis-runtime/src/session.mjs:58`) | unchanged: no new frame, no new field, no new capability | An old runtime with a new host keeps its old wording, and the host gets the same codes and `data` as today; a new runtime with an old host sends the new wording and `reason` as usual, and the old host reads `data` from the error's graph by key (the paragraph above), where one more key is neither looked at nor refused |
| `rutis.json` | 1 (`FORMAT` in `crates/rutis-host/src/config.rs:78`) | unchanged: no new key | An old file is read, and a new file is read by an old host |
| Plugin API | 3 (`PLUGIN_API` in `crates/rutis-loader/src/runtime/go.rs:34` and `swift.rs:33`; `pluginApi` in a manifest) | unchanged: nothing a plugin declares changes, only an error's text and code | An old manifest with a new host, and a new manifest with an old host, both install |
| Manifest format | 1 (`MANIFEST_FORMAT` in `crates/rutis-loader/src/runtime/binary.rs:186`) | unchanged | as above |

**An old end and a new one**: every check is local, so mixing them is safe; an older end keeps its old wording (it cannot know how the new one words a value in its hands) and the new end words its own. The word-for-word cases of §5 therefore require both ends at this version.

**The migration guide** (0.21, both languages) gains five entries: Node and Bun's NaN error; Python's cycle and NaN sentences; Go's result and caller-argument wording; Rust's NaN; Swift's and Kotlin's depth sentence and how a reference shows. **Dropped**: raising the protocol version to align the text — the text is not negotiated between versions, so it would change nothing.

## 7. Implementation

| Issue | What it does | What done means |
| --- | --- | --- |
| S1 (#936) | Node, Bun: `checkData` walks into Map / Set / records / tagged values; numbers, cycles (before the depth) and the depth in one sentence (four wordings today, §1); the event's call described as the event's name (D7); the display (D8) | `node/rutis-runtime/test/session.test.mjs`, `node/rutis/test/values.test.mjs` and `bun/rutis-bun/test/session.test.ts` lock every line of §5.1 with its `code` and `data` word for word; the event's line is in the `bad-values` cases |
| S2 (#936) | Python: `seen` through `_data`, `_encode` and `_encode_tagged`; the three sentences; `repr` becomes `<… of another process>` | `python/rutis/tests/test_plain_texts.py` locks §5.2 word for word |
| S3 (#936) | Go: `seen` in `toPeerValue`; `ToPeerArgs` given the call's name (from `Service.Call` and wherever the runtime knows the service and method); `Results` no longer converts early, so the result is encoded and `ofResult` naming happens in `respondTo`; `badResult` beside `refusals.go`; `badArgument` no longer paraphrases `encoding/json`, it adds `argument N: ` and keeps the sentence (`Expected` / `actual` are left to `data`) | the cases of `go/rutis/internal/convert` and `internal/peer` lock §5.3 word for word; `go/rutis/internal/fixtures/session` gains a case returning a channel |
| S4 (#936) | Rust: finiteness in `Exact::serialize_f32` / `serialize_f64`; `Exact` counts levels itself (an `Rc` pointing at itself fails as `cycle`) and `fits`' sentence changes; `call_service` wraps a result that does not decode as `bad-result`, keeping serde's sentence word for word and adding the `the result: ` prefix; `of_decoding` stops paraphrasing into `should be … got …`; the `live Cordis` of `objects.rs:809` goes | the cases of `crates/rutis-bridge/src/session/rpc/tests.rs` and `objects.rs` lock §5.4 word for word |
| S5 (#936) | Swift: `seen` in `Codec/ValueCodec.swift` (encoding, `:147-148`) and `Session/Peer.swift` (taking a value in, `:662,741-742`), a cycle before the depth, the three sentences; `ValueCodec`'s `cannot decode … into …` is Swift's own sentence and stays, only the prefix is added; `data` at `Errors.swift:385` is still read as data | `swift/rutis/Tests/SelfTest/PeerTests.swift` (today `:96` only checks that the text contains "nested deeper than 64") locks the Swift lines word for word |
| S6 (#936) | Kotlin: `seen` at both encoding places, `internal/peer/Peer.kt:420,508`, and a new sentence in `tooDeep()` (`:63-65`); generated argument decoding keeps kotlinx.serialization's sentence (`Refusals.kt:32-45` stops building `should be … got …`); `toString` of `advanced/RemoteFunction.kt:25` and `RemoteObject.kt:29` becomes `<… of another process>` | `kotlin/rutis/rutis/src/jvmTest/kotlin/arcships/rutis/PeerTest.kt` and `ErrorTextTest.kt` lock the Kotlin lines word for word |
| S7 (`bad-result` and the row name) | The diagnostics design's code table (§4.3) gets only this design's two rows (§4.3; the rest belongs to #937, and whichever lands second rebases); a reference carries the row that made it (a protocol change of its own: `deny_unknown_fields` means it needs a version or a capability first, and is designed separately) | the code table is updated; the row name is decided in its own design |
| S8 (the completion condition of #936) | `weather.expected.json`'s `throws` gains `code` and `data` (the section "How S8 is added" below says how, who produces them and who runs them); one sample per row of §1, each of the six runtimes naming its own | the matrix's six cells (Node, Python, Go, Swift, Bun, Kotlin) are green, and a text or code changed without its case goes red |

"Done" of every issue includes: every line of §5 that belongs to it is compared word for word by a case; the old wording (the table of §6) appears nowhere in the repository.

#### How S8 is added (`weather.expected.json`)

A sample's `throws` compares `name` and `message` only today (`crates/rutis-bridge/src/runtime/testing.rs:178-195`), and, when it compares anything, requires the error to be one the plugin threw (`Error::Remote`). Two keys are added: `code` (a string) and `data` (JSON), compared in the same place with `Error::code()` and `Error::data()` of the error received (`crates/rutis-bridge/src/session/mod.rs:181-213`) — both are read from the error's graph, so what is compared is the `data` that crossed.

- **Who runs it**: `rutis_bridge::runtime::testing::runtime`, which `crates/rutis-bridge/tests/session_matrix.rs` runs on six runtimes: Node, Python, Go, Swift, Bun, Kotlin. Rust is the host: it reports no samples, it reads them. Whether a cell runs is still decided by `RUTIS_TEST_RUNTIMES`.
- **Who produces `code`**: a sample's value is the **result** of `sample(name)`, so the failure happens in the result path of the runtime that made it, and `code` is `bad-value` with `data.part` `result` everywhere: Node and Bun's `#respond` → `ofResult` (`node/rutis-runtime/src/session.mjs:750-754`; the graph is written at `errors.mjs:170` / `errors.ts:170`); Python's `_respond` → `_cannot_send(…, "result")` (`peer.py:1804`); Go's `respondTo` → `ofResult` (`peer.go:1749`; `*ValueError`'s `Code()` / `Data()` at `values.go:52-90`); Swift's `respond` → `ofResult` (`Peer.swift:1391`; `ValueError`'s `code` at `TaggedValues.swift:431-441`); Kotlin's `respond` → `ofResult` (`Peer.kt:1324`; `ValueException`'s `code` / `data` at `Tagged.kt:61-66`, the graph written at `Errors.kt:213-221`). Kotlin's `tooDeep()` and Swift's depth `ValueError` carry no `data` themselves; that path adds `{part: "result"}` for them, and `reason` comes from this design (S1–S6).
- **The testing tools**: the three SDK tools read the same JSON (Go `rutistest/conformance_test.go:132-196`, Kotlin `WeatherConformanceTest.kt:53-67`, Swift `WeatherConformanceTests.swift:64-72`), compare `name` and `message` only, and skip `crossing: true`. They do not simulate crossing, so a value that cannot cross never fails there; such a sample is written with `crossing: true`, the tool skips it, and only the matrix checks it — the switch the file already has, with no new field.

