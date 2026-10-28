# The default entry of each SDK: only what plugin authors need at the root

[中文](design-sdk-entry-convergence-2026-10-27.md)

Status: final, not implemented. Date: 2026-10-27. Base: main `103c0d22`. Scheduled: 0.19.
Issue: [#692](https://github.com/arcships/rutis/issues/692) (part of [#663](https://github.com/arcships/rutis/issues/663), experience requirement 2, direction 3). Precedent: [Rust API convergence](design-rust-api-convergence-2026-10-24.en.md) (0.17).
Relation to the 0.18 designs: only where names live changes, no behaviour. Error type names and fields stay as [0.18 diagnostics](design-diagnostics-2026-10-26.en.md) §11 keeps them; the deprecation texts follow its §4.5 list of words not to use; Go's `rutis.Ref` keeps the spelling of [object references](design-object-refs-2026-10-27.en.md) O1 and stays at the root.

Out of scope: Rust (converged in 0.17 and locked by `public-api.txt`, see §2 E9); names added to the test kits (#670 adds them; this design only locks the test kits' public surface too); rewriting the guides by layer (#671, which uses only the root names fixed in §3); generated Python types and fakes (#684); Cordis plugins in Node (they use Cordis's own `ctx`); renaming Python's implementation modules (`rutis.peer`, `rutis.events`, …); a migration tool (§8); printing deprecation notices at run time.

## 1. Where the developer gets stuck

A third-party developer writes a `weather` plugin for a host (the #663 evaluation task): provide `weather.today(city)`, use another plugin's `llm.ask`, take a config `{ city }`, emit `weather/asked` on each call, write tests, run it under `dev`. What they need is the plugin-author layer's 7 concepts: plugin (definition + `apply`), services (provide / use), `inject`, config, events (`on` / `emit`), the test kit, `dev` and rows.

They type `rutis.` (or import from the root) and completion shows:

| Language | Root names | Of which they never use |
| --- | --- | --- |
| TypeScript / Bun (`@arcships/rutis`, one package for both) | 20 (10 values, 10 types) | the 5 value constructors (`datetime`, …), `PLUGIN`, `VALUE`, `isPlugin`, `PLUGIN_API`, the listener descriptors |
| Python (`rutis`) | 11 | 5 of the 7 error classes, `PLUGIN_API` |
| Go (`rutis`) | 52 (49 when the issue was written) | the value types and their 14 constructors, `ServeArgs` / `ServeConn` / `Manifest`, the version constants |
| Swift (`Rutis`) | 38 (10 top level, 28 under `Rutis.`) | the value types, `encode` / `export` and others the macro expansions use |
| Kotlin (`arcships.rutis`) | 30 top level + 5 on the `Rutis` object | 7 types the KSP-generated code uses, serialization helpers |

Nothing says which of these "writing a plugin needs". Rust solved the same problem in 0.17: the root keeps what authors need, the rest went to `rutis::host`, and a snapshot locks the public surface. This design applies the same method to the root names of the other 5 SDKs. The methods on `ctx` stay (§4).

## 2. Decisions

| # | Decision | Section |
| --- | --- | --- |
| E1 | Each language's root holds only the names the 7 concepts need, plus the error types a failed service call gives. The lists are in §3 | 3 |
| E2 | Every other name goes into the language's advanced submodule: `@arcships/rutis/advanced`, `rutis.advanced`, `github.com/arcships/rutis/go/rutis/advanced`, `Rutis.Advanced` (a Swift namespace), `arcships.rutis.advanced` (a Kotlin package). No capability is removed; behaviour is unchanged | 3 |
| E3 | The methods on `ctx` (the five ways to emit, `filter`, the scope) are neither moved nor deprecated; they stay on `ctx` as in Cordis and Rust 0.17. The guides (#671) teach only `on` / `emit` | 4 |
| E4 | In 0.19 the old locations keep working and name the new one through each language's own deprecation mechanism; they are removed in every language together, when the plugin API version (`PLUGIN_API`) increases | 5 |
| E5 | 0.19 does not increase `PLUGIN_API` | 5 |
| E6 | Names only the Swift macros' and Kotlin KSP's generated code use move to `Rutis._Generated` and `arcships.rutis.generated`; their old locations follow E4 | 3 |
| E7 | Each language's public surface (root, advanced, `ctx`, test kit, generated-code names) is locked with the language's most ready-made tool and compared in CI | 6 |
| E8 | No migration tool | 8 |
| E9 | Rust does not change | 3 |

## 3. The root names

Rule: a name stays at the root if it is how one of the 7 concepts is written, or if writing the 7 concepts makes it appear. Anything else goes to advanced. This is the final list for 0.19; #671's guides use only these names.

| Language | Root names |
| --- | --- |
| TypeScript / Bun | `definePlugin`; types `Plugin`, `PluginSpec`, `Context` (4) |
| Python | `define_plugin`, `Plugin`, `Context`, `RemoteError`, `BindingError` (5) |
| Go | written by authors: `Plugin`, `Define`, `Definition`, `Serve`, `NoConfig`, `Ctx`, `Provides`, `MethodsOf`, `Sync`, `Ref`, `RemoteError`, `ErrBinding` (12); only in signatures: `Shape`, `ShapeOption`, `ListenOption`, `Emitter`, `Listener`, `Terminal`, `Scope` (7) |
| Swift | `Plugin`, `Rutis.define`, `Rutis.Definition`, `Rutis.serve`, `Rutis.NoConfig`, `Rutis.Ctx`, `@RutisService`, `@RutisClient`, `@RutisConfig`, `RutisError`, `RutisApp` (11) |
| Kotlin | `Rutis.define`, `Rutis.NoConfig`, `Rutis.serve`, `Plugin`, `Ctx`, `EventArgs`, `service`, `@RutisService`, `@RutisClient`, `RutisException`, `RutisApp`, `RutisState` (12) |
| Test kits (already modules of their own) | `@arcships/rutis/testing`, `rutis.testing`, `rutistest`, `RutisTest`, `arcships.rutis.testing`: names unchanged |

Item by item:

- **Types that appear in signatures**: in TypeScript, Python, Swift and Kotlin a root signature (the methods of `ctx` included) may refer to a type in advanced (`Emitter`, `ListenerOptions`, `MethodKind`, `ServiceShape`, `Rutis.Shape`, …); authors do not write those names. Go's advanced package aliases the root's types and the root package cannot refer back to advanced (a cycle), so `Plugin.Provides` and the 7 types in the signatures of `Ctx`'s methods stay at the root.
- **How `provides` is written stays at the root**: Go's `MethodsOf`, `Sync` and Kotlin's `service` are how a provided service is declared; the Go template (`crates/rutis-host/templates/go/plugin.go`) and `plugin-api.md` write them directly. Go's `Rename` goes to advanced.
- **`on`'s optional parameters are not names**: the `prepend` and `global` options stay on `on`. Only Go's constructors `rutis.Prepend()`, `rutis.Global()` go to advanced.
- **Errors**: the two a service call can give stay at the root: the one the other side threw (Python `RemoteError`, Go `RemoteError`) and the one the platform makes, with the 0.18 error codes (Python `BindingError`, Go `ErrBinding`). Errors that belong to one way of emitting (`AggregateError`, `WaterfallNext`, `AsyncListener`, `ReservedEvent`) and `SyncWaitCycle` (a subclass of `RemoteError`, caught as one too) go to advanced. Swift and Kotlin have one error type each, which stays; TypeScript matches errors by `name` and has no error classes.
- **Go's `Ref`** stays at the root: Go structs do not tell data from objects, so passing an object by reference can only be written `rutis.Ref(v)` (object references O1).
- **Value types** (TypeScript `datetime` and the others, Go `Date` and the others, Swift `CalendarDate` and the others) go to advanced: #692 lists "the constructors of the value types" as advanced.
- **`RutisApp`, `RutisState`** (Swift, Kotlin) stay at the root: they are how an app hosts plugins, and every app using rutis writes them; in Kotlin that is the only use.
- **Generated-code names** (E6): authors do not write them, but they show in completion under `Rutis.` and `arcships.rutis`. Swift's `Rutis.encode`, `export`, `Encoded`, `Arguments`, `Method`, `Schema` go to `Rutis._Generated` (a leading underscore hides it from completion); the protocols `RutisExportable`, `RutisProxy`, `RutisConfigurable` appear in root signatures and stay at the top level. Kotlin's `ServiceTable`, `ServiceHandle`, `ClientFactory`, `Tables`, `Generated`, `ValueEncoder`, `ValueDecoder` go to `arcships.rutis.generated`. The old locations are kept per E4 as well: Kotlin's `rutis-ksp` is a separate Maven coordinate whose version the author writes separately, so code generated by an older KSP must still compile against the 0.19 SDK.
- **Rust does not change** (E9): the 0.17 acceptance bar (at most 10 names written by hand, the root and `rutis::events` locked by the snapshot) is met; Rust's `Ctx` also keeps the five ways to emit and `filter`, consistent with §4.

The names in each language's advanced:

| Language | Names in advanced |
| --- | --- |
| TypeScript / Bun | `PLUGIN_API`, `PLUGIN`, `isPlugin`, `VALUE`, `datetime`, `date`, `time`, `decimal`, `uuid`; types `TaggedValue`, `MethodKind`, `ListenerDescriptor`, `ListenerOptions`, `Listener`, `Emitter`, `Waterfall` |
| Python | `PLUGIN_API`, `AggregateError`, `AsyncListener`, `ReservedEvent`, `SyncWaitCycle`, `WaterfallNext` |
| Go | `Implementation`, `PluginAPI`, `Version`; `ErrReservedEvent`, `ErrWaterfallNext`, `ErrAsyncListener`, `ErrSyncWaitCycle`, `AggregateError`; `Manifest`, `Project`, `ServeArgs`, `ServeConn`; `Date`, `DateOf`, `ParseDate`, `DateTime`, `DateTimeOf`, `ParseDateTime`, `Decimal`, `ParseDecimal`, `TimeOfDay`, `TimeOfDayOf`, `ParseTimeOfDay`, `UUID`, `ParseUUID`; `Next`, `Global`, `Prepend`, `Rename`; `Function`, `Future`, `Service`, `Schemer` (33) |
| Swift | `sdkVersion`, `pluginAPI`, `implementation`; `CalendarDate`, `TimeOfDay`, `DateTime`, `BigInt`; `serveArgs`, `manifest`; `Emitter`, `Scope`, `Listener`, `Next`; `Client`, `RemoteFunction`; `MethodKind`, `Shape` |
| Kotlin | `VERSION`, `PLUGIN_API`, `IMPLEMENTATION`; `project`, `manifest`; `Emitter`, `Scope`, `Listener`, `Next`; `RemoteFunction`; `Kind`, `ServiceShape`; `RutisFormat`, `AnySerializer`, `JdkSerializers` |

## 4. The methods on `ctx` stay

`parallel`, `serial`, `waterfall`, `bail`, `filter` and the scope (Go `Scope`, Swift `scope` / `project`, Kotlin `scope`) all stay on `ctx`, neither moved nor deprecated. Why:

- Cordis 4 has `parallel`, `serial`, `bail`, `waterfall` on `ctx` (`cordis/src/events.ts`), and so does Rust 0.17's `Ctx`. Moving them to a view such as `advanced(ctx)` would make authors learn one more rutis-specific spelling, and the languages would no longer match Cordis and Rust.
- What #663 found is "on the guides' main path but used by no one"; guides that teach only `on` / `emit` (#671) fix that, and the methods showing in completion do not get in the way of writing a plugin. #692's "to do" asks only for the root names to go into submodules.
- With `ctx` unchanged, the `ctx` the runtime builds and the SDK's types agree, and a new SDK on an old runtime needs no thought.

The snapshots still list `ctx`'s members (§6), so adding a method changes them.

## 5. Old locations and the plugin API version

**Rule**: in 0.19 the old locations keep working and name the new one; they are removed in every language together when `PLUGIN_API` increases.

Why not remove them in 0.19:

- Python cannot. Plugins state only a lower bound `rutis>=` and share one package with the runtime, so the names plugins import are not removed or changed before `PLUGIN_API` increases (#692 comment; written in [release.md](release.en.md) and [plugin-api.md](guide/plugin-api.en.md)).
- The TypeScript and Go compilers report "no such name" with the name only and cannot add the new location, while #692 asks that old spellings "give a clear error or notice (naming the new location)". Keeping the old name and marking it deprecated is what lets the new location be named.
- Swift and Kotlin could remove them outright, but one rule for all is simpler.

So in 0.19 #692 requires a change from nobody; authors change their code when they upgrade the SDK in the release that increases `PLUGIN_API`.

| Language | Old spelling | Where the notice shows |
| --- | --- | --- |
| TypeScript / Bun | the old root exports are marked `@deprecated` in the `.d.ts`, naming the new location; the runtime is unchanged | the editor (strike-through, hover text); `typescript-eslint`'s `no-deprecated` |
| Python | the old root names come from the module's `__getattr__` (PEP 562), which issues a `DeprecationWarning` on first access; they stay in `__all__` (`from rutis import *` must not change either) | pytest's warnings summary, `python -W default`; `-W error` makes a test fail on an old name |
| Go | old names kept as aliases or forwarders with a `// Deprecated: …` comment | gopls (strike-through), `staticcheck` SA1019 |
| Swift | `@available(*, deprecated, renamed:)` | compiler warning; Xcode's fix-it |
| Kotlin | a `typealias` or forwarding function with `@Deprecated(message, ReplaceWith(…))`, level WARNING | compiler warning; the IDE's one-click replace |

**0.19 does not increase `PLUGIN_API`**: `PLUGIN_API` increases only when the interface a plugin sees becomes incompatible ([release.md](release.en.md)). This design only adds modules on the SDK side; the `ctx` the runtime builds and the protocol are unchanged, and increasing it would instead force every published plugin to be rebuilt (#663 A8). During implementation, release.md gets one sentence after that rule: "When `PLUGIN_API` increases, the old locations each SDK has marked deprecated since 0.19 are removed."

## 6. Public-surface snapshots and CI

As with Rust's `public-api.txt`: the public surface is text committed to the repository, CI regenerates and compares it, and adding a name changes it, so a review sees it. Each language uses its most ready-made approach with no new dependency, all inside existing CI jobs ([ci.md](ci.en.md) §6).

| Language | Text that locks the surface | How it is checked | CI job | On an intended change |
| --- | --- | --- | --- | --- |
| TypeScript / Bun | the hand-written `.d.ts` files themselves (`src/index.d.ts`, `src/advanced.d.ts`, `src/testing.d.ts`) | `node/rutis/test/public-api.test.mjs`: `package.json`'s `exports` holds only `.`, `./advanced`, `./testing`; each entry, imported, exports the same names its `.d.ts` declares with `export` (read line by line, no compiler) | `js-py` (`npm test`) | edit the `.d.ts` |
| Python | `python/rutis/public-api.txt` | `python/rutis/tests/test_public_api.py`: the `__all__` of `rutis`, `rutis.advanced`, `rutis.testing`, each class's public members and `inspect.signature`; every public name in `dir()` is in `__all__` or is a submodule | `js-py` | run this test with `RUTIS_PUBLIC_API_WRITE=1` |
| Go | `go/rutis/public-api.txt` | `go/rutis/public_api_test.go`: the standard library's `go/parser` and `go/doc` list the exported declarations, methods with signatures and Deprecated marks of `rutis`, `rutis/advanced`, `rutis/rutistest` (a golden file) | `runtimes-go` | `go test -run TestPublicAPI -update` |
| Swift | `swift/rutis/public-api.txt` | `tools/public-api-swift.sh`: `swift package dump-symbol-graph --minimum-access-level public`, then `jq` (part of macOS since 15) writes the symbols of `Rutis` and `RutisTest` one per line, sorted: the path (with argument labels) and the kind. Not the declaration text, which compilers spell differently (Swift 6.3 writes `any Encoder`, 6.4 `Encoder`) | `runtimes-swift` | `tools/public-api-swift.sh --write` |
| Kotlin | `kotlin/rutis/rutis/api/` | Kotlin's official Binary Compatibility Validator, `apiCheck` hooked into `check` (the ABI validation built into Kotlin Gradle plugin 2.2 is still experimental, so not that) | `runtimes-kotlin` | `./gradlew apiDump` |
| Rust | `crates/rutis/public-api.txt` | unchanged (`tools/public-api.sh`) | `public-api` | unchanged |

TypeScript's `.d.ts` files are written by hand and are the public surface; generating another snapshot from them would repeat them, so the check only makes sure the runtime exports agree with them. Deprecated names get a section of their own in each text, removed whole when they go. The test kits are locked too: when #670 adds names to them, the same PR updates the text. During implementation, ci.md §4 notes this check on each job's row.

## 7. What the developer sees

**Writing an ordinary plugin**: only root names, `ctx` as before.

```ts
import { definePlugin } from '@arcships/rutis'

export default definePlugin({
  inject: ['llm'],
  config: { type: 'object', properties: { city: { type: 'string', default: 'Oslo' } } },
  apply(ctx, config) {
    ctx.provide('weather', {
      async today(city = config.city) {
        ctx.emit('weather/asked', city)
        return ctx.use('llm').ask(`weather in ${city}`)
      },
    })
  },
})
```

**Using an advanced name**: import it from advanced; the methods are still on `ctx`.

```python
from rutis.advanced import AggregateError

async def apply(ctx, config):
    try:
        await ctx.parallel("cache/flush")
    except AggregateError as error:
        ...
```

```go
import "github.com/arcships/rutis/go/rutis/advanced"

day := advanced.DateOf(time.Now())
```

**Old code, unchanged**: runs as before and names the new location.

| Language | Input | Sees |
| --- | --- | --- |
| TypeScript | `import { datetime } from '@arcships/rutis'` | strike-through in the editor; hover: `'datetime' is deprecated. Moved in 0.19: import it from '@arcships/rutis/advanced'. Removed when the plugin API version changes.` |
| Python | `from rutis import AggregateError` | `ImportError: cannot import name 'AggregateError' from 'rutis' (…/rutis/__init__.py)` |
| Go | `rutis.DateOf(t)`, run `staticcheck` | `SA1019: rutis.DateOf is deprecated: moved in 0.19: use advanced.DateOf.` |
| Swift | `let d: Rutis.DateTime` | `warning: 'DateTime' is deprecated: renamed to 'Rutis.Advanced.DateTime'`, with a fix-it |
| Kotlin | `RutisFormat.encode(…)` | `w: 'RutisFormat' is deprecated. Moved in 0.19: use arcships.rutis.advanced.RutisFormat.`; the IDE replaces it in one click and adds the import |

**A rutis contributor adds a public name without updating the snapshot**: CI fails and prints the difference and the fix, for example:

```
--- python/rutis/public-api.txt
+++ current
@@ rutis.advanced @@
+def retry(…)
the public API of rutis changed: review the diff, then run RUTIS_PUBLIC_API_WRITE=1 on tests/test_public_api.py
```

The other languages are the same; the last line gives that language's way to update (§6).

## 8. Compatibility and migration

Goes into "Migrating to 0.19" (both languages), section "The SDKs' default entry (#692)":

> From 0.19, each SDK's root keeps only the names commonly needed to write a plugin; the rest is in advanced: TypeScript / Bun `@arcships/rutis/advanced`, Python `rutis.advanced`, Go `github.com/arcships/rutis/go/rutis/advanced`, Swift `Rutis.Advanced`, Kotlin `arcships.rutis.advanced`.
>
> **It runs without changes.** The old spellings keep working in 0.19, and the editor, compiler or tests name the new location; they are removed only when the plugin API version increases, and that release's notes say so again.
>
> Only imports change: names that left the root (value types, `PLUGIN_API`, the event descriptor types, errors that belong to one way of emitting, …) are imported from advanced under the same name; the full list is in [The default entry of each SDK](design-sdk-entry-convergence-2026-10-27.en.md) §3.
>
> Unchanged: each root's `definePlugin` / `define_plugin` / `Define` / `Rutis.define`, every method on `ctx`, the test kits, Rust. Code generated by the Swift macros and Kotlin KSP updates with the SDK and needs no change. The plugin API version is unchanged, and plugins built with an older SDK run on the 0.19 runtime as before.

**No migration tool** (E8): Rust's `rutis-migrate` exists because 0.17 changed trait signatures, which are easy to get wrong by hand; here only import locations change, the old spellings still work, the Swift and Kotlin IDEs replace in one click, and in the other three languages each place is a one-line change. In the #663 evaluation an ordinary plugin used only root names, so most plugins need no change at all.

## 9. Implementation

One PR per language, independent of each other; step 6 comes last.

| Step | Content | Done when |
| --- | --- | --- |
| 1 | TypeScript / Bun: `node/rutis` gets the `./advanced` entry; old root exports marked `@deprecated` in the `.d.ts`; the export-agreement test | the test passes; the repository's own examples, templates and test plugins use the new locations |
| 2 | Python: `rutis.advanced`; the root's `__getattr__`; the snapshot test | the snapshot test passes; an old-name import and `from rutis import *` each have a test that they work and issue a `DeprecationWarning`, failing under `-W error` |
| 3 | Go: the `advanced` subpackage (aliases to the internal packages or the root's types); `// Deprecated:` on the old root names; the snapshot test | the snapshot test passes; `staticcheck` reports SA1019 on old spellings; `rutistest` and `examples` use the new locations |
| 4 | Swift: `Rutis.Advanced`, `Rutis._Generated`; the macros refer to `_Generated`; old names marked `deprecated`; the snapshot script | `RutisSelfTest` passes; the snapshot comparison passes; old spellings give warnings only, no errors |
| 5 | Kotlin: `arcships.rutis.advanced`, `arcships.rutis.generated`; KSP refers to the new packages; old names `@Deprecated(WARNING)`; Binary Compatibility Validator wired in | `gradle check` (with `apiCheck`) passes; code generated by the 0.17.2 `rutis-ksp` compiles against the new SDK; the Android sample builds as before |
| 6 | Docs and CI: the §8 section in both "Migrating to 0.19"; `plugin-api.md`'s quick reference moves the names that left the root to their new locations; ci.md §4 notes each job's check; release.md gets the §5 sentence | the link check passes; against #692's "Done" list: default entry and submodules (1–5), snapshots and CI (1–5 and ci.md), both migration guides (6) |
