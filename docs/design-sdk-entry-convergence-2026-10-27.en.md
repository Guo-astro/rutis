# The default entry of each SDK: only what plugin authors need at the root

[中文](design-sdk-entry-convergence-2026-10-27.md)

Status: draft, not implemented. Date: 2026-10-27. Base: main `103c0d22`. Scheduled: 0.19.
Issue: [#692](https://github.com/arcships/rutis/issues/692) (part of [#663](https://github.com/arcships/rutis/issues/663), experience requirement 2, direction 3). Precedent: [Rust API convergence](design-rust-api-convergence-2026-10-24.en.md) (0.17).
Relation to the 0.18 designs: only where names live changes, no behaviour. Error type names and fields stay as [0.18 diagnostics](design-diagnostics-2026-10-26.en.md) §11 keeps them; the deprecation texts follow its §4.5 list of words not to use; Go's `rutis.Ref` keeps the spelling of [object references](design-object-refs-2026-10-27.en.md) O1 and stays at the root.

Out of scope: Rust (converged in 0.17 and locked by `public-api.txt`; this design leaves it alone, see §2 E9); names added to the test kits (#670 adds them; this design only puts the test kits' public surface into the snapshots too); rewriting the guides by layer (#671, which uses only the root names fixed in §3); generated Python types and fakes (#684); Cordis plugins in Node (they use Cordis's own `ctx`); renaming Python's implementation modules (`rutis.peer`, `rutis.events`, …, bound by the Python rule in §5); a migration tool (§8); printing deprecation notices at run time; a CI check that guides do not use advanced (the guides are #671's).

## 1. Where the developer gets stuck

A third-party developer writes a `weather` plugin for a host (the #663 evaluation task): provide `weather.today(city)`, use another plugin's `llm.ask`, take a config `{ city }`, emit `weather/asked` on each call, write tests, run it under `dev`. What they need is the plugin-author layer's 7 concepts: plugin (definition + `apply`), services (provide / use), `inject`, config, events (`on` / `emit`), the test kit, `dev` and rows.

They open completion in the editor and see:

| Language | Root names | Members of `ctx` | Of which they never use |
| --- | --- | --- | --- |
| TypeScript / Bun (`@arcships/rutis`, one package for both) | 20 (10 values, 10 types) | 11 | the 5 value constructors (`datetime`, …), `PLUGIN`, `VALUE`, `isPlugin`, `PLUGIN_API`, the listener descriptors; `parallel`, `serial`, `waterfall`, `bail`, `filter` |
| Python (`rutis`) | 11 | 10 | 5 of the 7 error classes, `PLUGIN_API`; the same 5 methods |
| Go (`rutis`) | 52 (49 when the issue was written) | 12 | the value types and their 14 constructors, `ServeArgs` / `ServeConn` / `Manifest`, the `Shape` family; `Parallel` and the other 4 methods, and `Scope` |
| Swift (`Rutis`) | 38 (10 top level, 28 under `Rutis.`) | 13 | the value types, `encode` / `export` and others the macro expansions use; the same 5 methods, `scope`, `project` |
| Kotlin (`arcships.rutis`) | 30 top level + 5 on the `Rutis` object | 11 | 7 types the KSP-generated code uses, serialization helpers; the same 5 methods, `scope` |

Nothing says which of these "writing a plugin needs"; the guides put them on the main path too (#663: "on the guides' main path but used by no one"). Rust solved the same problem in 0.17: the root keeps what authors need, the rest went to `rutis::host`, and a snapshot locks the public surface. This design applies the same method to the other 5 SDKs.

## 2. Decisions

| # | Decision | Section |
| --- | --- | --- |
| E1 | Each language's root holds only the names the 7 concepts need, plus the language's own way of "the plugin stops" (`signal`, `ctx.Go`, `spawn`, the coroutine scope) and the error types a failed service call gives. The lists are in §3 | 3 |
| E2 | Every other name goes into the language's advanced submodule: `@arcships/rutis/advanced`, `rutis.advanced`, `github.com/arcships/rutis/go/rutis/advanced`, `Rutis.Advanced` (a Swift namespace), `arcships.rutis.advanced` (a Kotlin package). No capability is removed; behaviour is unchanged | 3 |
| E3 | Methods on `ctx` move like this: TypeScript and Python use `advanced(ctx)`, Go `advanced.Of(ctx)`, Swift and Kotlin `ctx.advanced`. In TypeScript, Python and Go this lives in the SDK only; the `ctx` the runtime builds does not change | 4 |
| E4 | In 0.19 the old locations keep working and name the new one through each language's own deprecation mechanism; they are removed in every language together, when the plugin API version (`PLUGIN_API`) increases | 5 |
| E5 | 0.19 does not increase `PLUGIN_API` | 5 |
| E6 | Names only the Swift macros' and Kotlin KSP's generated code use leave the root (`Rutis._Generated`, `arcships.rutis.generated`). Generated code has the SDK's version, so authors' code does not change and no old location is kept | 3 |
| E7 | Each language's public surface (root, advanced, test kit, generated-code names) has a snapshot made with the language's own tools, compared in CI | 6 |
| E8 | No migration tool | 8 |
| E9 | Rust does not change | 3 |

## 3. The root names

Rule: a name stays at the root if it is how one of the 7 concepts is written, or if writing the 7 concepts makes it appear. Anything else goes to advanced. This is the final list for 0.19; #671's guides use only these names.

| Language | Root names | Members of `ctx` |
| --- | --- | --- |
| TypeScript / Bun | `definePlugin`; types `Plugin`, `PluginSpec`, `Context` (4) | `use`, `provide`, `effect`, `signal`, `on`, `emit` (6) |
| Python | `define_plugin`, `Plugin`, `Context`, `RemoteError`, `BindingError` (5) | `use`, `provide`, `effect`, `on`, `emit` (5) |
| Go | `Plugin`, `Define`, `Definition`, `Serve`, `NoConfig`, `Ctx`, `Provides`, `ListenOption`, `Ref`, `RemoteError`, `ErrBinding` (11) | `Use`, `Provide`, `Effect`, `Go`, `On`, `Emit` (6), and `context.Context`'s own methods |
| Swift | `Plugin`, `Rutis.define`, `Rutis.Definition`, `Rutis.serve`, `Rutis.NoConfig`, `Rutis.Ctx`, `@RutisService`, `@RutisClient`, `@RutisConfig`, `RutisError`, `RutisApp` (11) | `use`, `provide`, `effect`, `spawn`, `on`, `emit`, `advanced` (7) |
| Kotlin | `Rutis.define`, `Rutis.NoConfig`, `Rutis.serve`, `Plugin`, `Ctx`, `EventArgs`, `@RutisService`, `@RutisClient`, `RutisException`, `RutisApp`, `RutisState` (11) | `use`, `provide`, `effect`, `on`, `emit` (5), and the coroutine scope (`launch`); `ctx.advanced` is an extension property in the advanced package |
| Test kits (already modules of their own) | `@arcships/rutis/testing`, `rutis.testing`, `rutistest`, `RutisTest`, `arcships.rutis.testing`: names unchanged | — |

Item by item:

- **Optional parameters are not names**: `on`'s `prepend` and `global` options (TypeScript's third argument, Python's keyword arguments, Swift / Kotlin default arguments) stay on `on`, spelled as today. Only the options' type name (TypeScript's `ListenerOptions`) and Go's constructors `rutis.Prepend()`, `rutis.Global()` go to advanced.
- **Types that appear in signatures**: in TypeScript, Python, Swift and Kotlin a root signature may refer to a type in advanced (`on`'s options type, `provides`'s `MethodKind` / `ServiceShape` / `Rutis.Shape`); authors do not write those names. Go packages cannot import each other in a cycle, so the `Plugin.Provides` field's type `Provides` and `On`'s parameter type `ListenOption` must stay at the root; `MethodsOf`, `Sync`, `Rename`, `Prepend`, `Global`, which fill them, are in advanced.
- **Errors**: the two a service call can give stay at the root: the one the other side threw (Python `RemoteError`, Go `RemoteError`) and the one the platform makes, with the 0.18 error codes (Python `BindingError`, Go `ErrBinding`). Errors that belong to one way of emitting (`AggregateError` to `parallel`, `WaterfallNext` to `waterfall`, `AsyncListener` and `SyncWaitCycle` to `bail` and synchronous calls, `ReservedEvent` to emitting `rutis/*`) go to advanced with those ways. Swift and Kotlin have one error type each, which stays; TypeScript matches errors by `name` and has no error classes.
- **Go's `Ref`** stays at the root: Go structs do not tell data from objects, so passing an object by reference can only be written `rutis.Ref(v)` (object references O1), where other languages write nothing at all.
- **Value types** (TypeScript `datetime` and the others, Go `Date` and the others, Swift `CalendarDate` and the others) go to advanced: #692 lists "the constructors of the value types" as advanced. Python and Kotlin use the language's own types and have no such names.
- **`RutisApp`** (Swift, Kotlin) stays at the root: it is the only way an app hosts plugins and every app using rutis writes it; moving it would only add an import to every app.
- **Generated-code names** (E6): Swift's `Rutis.encode`, `export`, `Encoded`, `Arguments`, `Method`, `Schema` (checked against the macros' actual output when implementing) go to `Rutis._Generated` (a leading underscore: Swift completion and documentation leave it out); the protocols `RutisExportable`, `RutisProxy`, `RutisConfigurable` appear in root signatures and stay at the top level. Kotlin's `ServiceTable`, `ServiceHandle`, `ClientFactory`, `Tables`, `Generated`, `ValueEncoder`, `ValueDecoder` go to `arcships.rutis.generated`.
- **Rust does not change** (E9): 0.17's Rust `Ctx` still has the five ways to emit and `filter`. 0.17's acceptance measure, "at most 10 names the author writes; the root and `rutis::events` locked by the snapshot", is met; moving them again would be Rust's second incompatible change, and the guides' short path (#671) teaches only `on` / `emit` in Rust as well.

The names in each language's advanced:

| Language | Names in advanced |
| --- | --- |
| TypeScript / Bun | `advanced`, type `Advanced`; `PLUGIN_API`, `PLUGIN`, `isPlugin`, `VALUE`, `datetime`, `date`, `time`, `decimal`, `uuid`; types `TaggedValue`, `MethodKind`, `ListenerDescriptor`, `ListenerOptions`, `Listener`, `Emitter`, `Waterfall` |
| Python | `advanced`, `Advanced`, `PLUGIN_API`, `AggregateError`, `AsyncListener`, `ReservedEvent`, `SyncWaitCycle`, `WaterfallNext` |
| Go | `Of`, `Ctx`; `Implementation`, `PluginAPI`, `Version`; `ErrReservedEvent`, `ErrWaterfallNext`, `ErrAsyncListener`, `ErrSyncWaitCycle`, `AggregateError`; `Manifest`, `Project`, `ServeArgs`, `ServeConn`; `Date`, `DateOf`, `ParseDate`, `DateTime`, `DateTimeOf`, `ParseDateTime`, `Decimal`, `ParseDecimal`, `TimeOfDay`, `TimeOfDayOf`, `ParseTimeOfDay`, `UUID`, `ParseUUID`; `Emitter`, `Listener`, `Next`, `Terminal`, `Scope`, `Global`, `Prepend`; `Function`, `Future`, `Service`, `Schemer`; `Shape`, `ShapeOption`, `MethodsOf`, `Sync`, `Rename` (44) |
| Swift | `Rutis.Advanced.Ctx` (the type of `ctx.advanced`); `sdkVersion`, `pluginAPI`, `implementation`; `CalendarDate`, `TimeOfDay`, `DateTime`, `BigInt`; `serveArgs`, `manifest`; `Emitter`, `Scope`, `Listener`, `Next`; `Client`, `RemoteFunction`; `MethodKind`, `Shape` |
| Kotlin | `Advanced` (the type of `ctx.advanced`), the extension property `advanced`; `VERSION`, `PLUGIN_API`, `IMPLEMENTATION`; `project`, `manifest`; `Emitter`, `Scope`, `Listener`, `Next`; `RemoteFunction`; `Kind`, `ServiceShape`, `service`; `RutisFormat`, `AnySerializer`, `JdkSerializers` |

## 4. How methods move

A method cannot be put into a submodule, so each language gets an "advanced view": it carries the methods moved off the root, with the same names and parameters as before.

| Language | Spelling | Why |
| --- | --- | --- |
| TypeScript / Bun | `import { advanced } from '@arcships/rutis/advanced'`, `await advanced(ctx).serial('x', 1)` | The runtime (`@arcships/rutis-runtime`, `@arcships/rutis-bun`) builds `ctx`, and each plugin carries its own copy of the SDK. `advanced(ctx)` lives only in the SDK and calls methods `ctx` already has, so a plugin on the 0.19 SDK still runs on an older runtime and `PLUGIN_API` need not change. A new member on `ctx` would require the runtime to upgrade too |
| Python | `from rutis.advanced import advanced`, `await advanced(ctx).serial("x", 1)` | The same spelling as TypeScript |
| Go | `import "github.com/arcships/rutis/go/rutis/advanced"`, `advanced.Of(ctx).Serial(ctx, "x", 1)` | Go's root package cannot refer to a subpackage's types (it would be a cycle), so it cannot be `ctx.Advanced()`; a function in the subpackage is the usual Go way |
| Swift | `try await ctx.advanced.serial("x", 1, as: Int.self)` | Swift has no submodules; a namespace plus one property is the Swift way. The SDK is compiled into the plugin, so a new member affects no other version |
| Kotlin | `import arcships.rutis.advanced.advanced`, `ctx.advanced.serial("x", 1)` | An extension property is visible only once imported, like Rust 0.17's extension traits; the IDE adds the import |

The methods on the advanced view:

| Language | Methods |
| --- | --- |
| TypeScript / Bun, Python | `parallel`, `serial`, `waterfall`, `bail`, `filter` |
| Go | `Parallel`, `Serial`, `Waterfall`, `Bail`, `Filter`, `Scope` |
| Swift | `parallel`, `serial`, `waterfall`, `bail`, `filter` (with their `arguments:` forms), `scope`, `project` |
| Kotlin | `parallel`, `serial`, `waterfall`, `bail`, `filter`, `scope` |

`emit` stays at the root, the only way to emit there; the emitter `filter` returns still has all five. The listening side does not change: a `waterfall` listener still gets `next`.

## 5. Old locations and the plugin API version

**Rule**: in 0.19 the old locations keep working and point to the new one; they are removed in every language together, when `PLUGIN_API` increases.

Why not remove them in 0.19:

- Python cannot. Plugins depend on `rutis>=` with a lower bound only and share one package with the runtime, so until `PLUGIN_API` increases, names plugins import are neither removed nor changed (#692 comment, already in [release.md](release.md) and [plugin-api.md](guide/plugin-api.en.md)).
- The TypeScript and Go compilers report a missing name with the name only and cannot add where it went, while #692 asks for "a clear error or notice (naming the new location)" on old code. Keeping the old name, marked deprecated, is how to name the new location.
- Swift and Kotlin could remove them right away with their "error with a message" mechanisms. But they would be the only two incompatible languages, buying a few struck-out names fewer in completion at the cost of one more rule; one rule for all is simpler.

So in 0.19 nobody is required to change anything for #692; the incompatibility moves to the release where `PLUGIN_API` increases. That release's runtimes keep loading plugins of the older plugin API ([release.md](release.md)'s rule), and authors change their code only when they upgrade the SDK.

Each language uses its own mechanism:

| Language | Old spelling | Where the notice shows |
| --- | --- | --- |
| TypeScript / Bun | The old root exports and the old `Context` members are marked `@deprecated` in the `.d.ts`, naming the new location; run time unchanged | The editor (struck out, hover text); `typescript-eslint`'s `no-deprecated` |
| Python | The old root names come from the module's `__getattr__` (PEP 562), which raises a `DeprecationWarning` on first use; they stay in `__all__` (`from rutis import *` must not change either). `ctx.parallel` and the others work as before and warn the same way | pytest's warnings summary, `python -W default`; `-W error` makes a test fail on an old name |
| Go | The old names stay as aliases or forwarders, commented `// Deprecated: …` | gopls (struck out), `staticcheck`'s SA1019 |
| Swift | `@available(*, deprecated, message:)`; `renamed:` for types | Compiler warnings; Xcode's fix-its |
| Kotlin | `@Deprecated(message, ReplaceWith(…))`, level WARNING | Compiler warnings; the IDE's one-click replace |

**0.19 does not increase `PLUGIN_API`**: `PLUGIN_API` is the contract between plugins and runtimes and increases only when what plugins see becomes incompatible ([release.md](release.md)). This design adds a module and a view on the SDK side only; the `ctx` runtimes build and the protocol do not change, so nothing is incompatible. Increasing it would instead require every published plugin to be rebuilt (#663, A8). When implementing, add one sentence after that rule in release.md: "When `PLUGIN_API` increases, remove the old locations the SDKs have marked deprecated since 0.19."

The snapshots list the deprecated names in a section of their own (§6): it cannot grow, and the whole section goes when they are removed.

## 6. Public-surface snapshots and CI

As with Rust's `public-api`: each language writes its public surface as a text snapshot committed to the repository, which CI regenerates and compares. A new name changes the snapshot, so review sees it. The snapshots have sections: root, advanced, test kit, generated-code names (Swift, Kotlin), deprecated old locations.

The test kits are in the snapshots too. They are not the SDK root, but plugin authors write code against them directly; when #670 adds `on` and other methods to them, it updates the snapshot in the same PR.

| Language | Snapshot | How it is made | CI job | On an intended change |
| --- | --- | --- | --- | --- |
| TypeScript / Bun | `node/rutis/public-api.txt` | `node/rutis/test/public-api.test.mjs`: reads, with the TypeScript compiler API, the `.d.ts` of every entry in `exports` (`.`, `./advanced`, `./testing`) and lists exported names, interface and class members with their signatures, and `@deprecated` marks; then imports every entry and checks its runtime exports match the `.d.ts`. `typescript` is a dev dependency at `rutis-runtime`'s version (6.0.3) | `js-py` (`npm test` of `node/rutis`) | `RUTIS_PUBLIC_API_WRITE=1 npm test` |
| Python | `python/rutis/public-api.txt` | `python/rutis/tests/test_public_api.py`: the `__all__` of `rutis`, `rutis.advanced`, `rutis.testing`, with each class's public members and `inspect.signature`; also checks every public name in `dir()` is in `__all__` or is a submodule | `js-py` (Python unit tests) | run that test with `RUTIS_PUBLIC_API_WRITE=1` |
| Go | `go/rutis/public-api.txt` | `go/rutis/public_api_test.go`: with the standard library's `go/parser` and `go/doc`, the exported declarations, methods, signatures and Deprecated marks of `rutis`, `rutis/advanced`, `rutis/rutistest` (Go's usual golden-file pattern) | `runtimes-go` (`go test`) | `go test -run TestPublicAPI -update` |
| Swift | `swift/rutis/public-api.txt` | `tools/public-api-swift.sh`: `swift package dump-symbol-graph --minimum-access-level public`, then `tools/public-api/swift.mjs` writes the symbol graphs of `Rutis` and `RutisTest` as sorted lines, one declaration each | `runtimes-swift` | `tools/public-api-swift.sh --write` |
| Kotlin | `kotlin/rutis/rutis/api/` | Kotlin's official Binary Compatibility Validator (a Gradle plugin), with `apiCheck` attached to `check` | `runtimes-kotlin` | `./gradlew apiDump` |
| Rust | `crates/rutis/public-api.txt` | unchanged (`tools/public-api.sh`) | `public-api` | unchanged |

Why these: TypeScript's `.d.ts` files are hand-written, and api-extractor needs compiler output and much configuration, while the compiler API reads the hand-written `.d.ts` directly; in Python only `__all__` is the agreed public surface; Go uses the standard library and adds no dependency; Swift uses the symbol graphs its toolchain ships; Kotlin uses the official plugin. All of it runs in existing jobs, no new job ([ci.md](ci.en.md) §6). When implementing, add this to each job's row in ci.md §4.

## 7. What developers see

**Writing an ordinary plugin** (nothing imported from advanced):

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

Completion after `ctx.` shows `use`, `provide`, `effect`, `signal`, `on`, `emit`, and the 5 old methods struck out (from 0.19 until `PLUGIN_API` increases). Python, Go, Swift and Kotlin are the same, with the names in §3.

**Using something advanced**:

```python
from rutis.advanced import advanced, AggregateError

async def apply(ctx, config):
    try:
        await advanced(ctx).parallel("cache/flush")
    except AggregateError as error:
        ...
```

```go
first, err := advanced.Of(ctx).Serial(ctx, "session/pick", id)
```

**Old code left as it is**: it runs as before and points to the new location.

| Language | Input | Output |
| --- | --- | --- |
| TypeScript | `await ctx.parallel('cache/flush')` | Struck out in the editor; hover: `'parallel' is deprecated. Moved in 0.19: advanced(ctx).parallel(…), with advanced from '@arcships/rutis/advanced'. Removed when the plugin API version changes.` |
| Python | `from rutis import AggregateError`, run under pytest | `DeprecationWarning: AggregateError moved in 0.19: import it from rutis.advanced. The old name is removed when the plugin API version changes.` |
| Python | `await ctx.parallel("cache/flush")` | `DeprecationWarning: ctx.parallel moved in 0.19: use advanced(ctx).parallel, with advanced from rutis.advanced. …` |
| Go | `ctx.Parallel(ctx, "cache/flush")`, run `staticcheck` | `SA1019: ctx.Parallel is deprecated: moved in 0.19: use advanced.Of(ctx).Parallel.` |
| Swift | `try await ctx.parallel("cache/flush")` | `warning: 'parallel' is deprecated: moved in 0.19: use ctx.advanced.parallel` |
| Swift | `let d: Rutis.DateTime` | `warning: 'DateTime' is deprecated: renamed to 'Rutis.Advanced.DateTime'`, with a fix-it |
| Kotlin | `ctx.parallel("cache/flush")` | `w: 'suspend fun parallel(name: String, vararg args: Any?): Unit' is deprecated. Moved in 0.19: use ctx.advanced.parallel.`; the IDE's "Replace with 'advanced.parallel(name, *args)'" adds the import too |

**A rutis contributor adds a public name without updating the snapshot**: CI fails with the snapshot's diff and what to do, for example:

```
--- node/rutis/public-api.txt
+++ current
@@ ./advanced @@
+export function retry(…)
the public API of @arcships/rutis changed: review the diff, then run RUTIS_PUBLIC_API_WRITE=1 npm test
```

The other languages are the same, the last line naming the language's command to update the snapshot (§6).

## 8. Compatibility and migration

Goes into "Migrating to 0.19" (both copies), section "The SDKs' default entry (#692)":

> From 0.19 the root of each SDK keeps only the names plugins commonly need; the rest is in advanced: TypeScript / Bun `@arcships/rutis/advanced`, Python `rutis.advanced`, Go `github.com/arcships/rutis/go/rutis/advanced`, Swift `Rutis.Advanced`, Kotlin `arcships.rutis.advanced`.
>
> **Nothing has to change.** The old spellings keep working in 0.19, and the editor, compiler or tests point to the new location; they are removed only when the plugin API version increases, which that release's notes will say again.
>
> What to change:
>
> | Old | New |
> | --- | --- |
> | `ctx.parallel / serial / waterfall / bail / filter(…)` (TypeScript, Python) | `advanced(ctx).parallel(…)` and so on, `advanced` imported from the advanced submodule |
> | `ctx.Parallel / Serial / Waterfall / Bail / Filter / Scope(…)` (Go) | `advanced.Of(ctx).Parallel(…)` and so on |
> | `ctx.parallel … / ctx.scope / ctx.project` (Swift), `ctx.parallel … / ctx.scope` (Kotlin) | `ctx.advanced.parallel …` (Kotlin needs `import arcships.rutis.advanced.advanced`) |
> | Other root names (value types, `PLUGIN_API`, the event descriptor types, errors that belong to one way of emitting, …) | The same name, imported from advanced; the full list is in [The default entry of each SDK](design-sdk-entry-convergence-2026-10-27.en.md) §3 |
>
> Unchanged: each language's root `definePlugin` / `define_plugin` / `Define` / `Rutis.define`, `use`, `provide`, `effect`, `on`, `emit` on `ctx`, `on`'s `prepend` and `global` options, the test kits, Rust. Code the Swift macros and Kotlin KSP generate is updated with the SDK and needs no change. The plugin API version does not change: plugins built with an older SDK run on 0.19 runtimes as before.

**No migration tool** (E8): Rust had `rutis-migrate` because 0.17 changed trait signatures (`BoxFuture`, `Effect`, how dependencies are written), easy to get wrong by hand; here only import locations move, the old spellings still work, the Swift and Kotlin IDEs replace them with one click, and in the other three languages each place is a one-line change. In #663's evaluation ordinary plugins use only root names, so most plugins need no change at all.

## 9. Implementation

One PR per language, independent of each other; step 6 comes last.

| Step | Content | Done when |
| --- | --- | --- |
| 1 | TypeScript / Bun: `node/rutis` gets the `./advanced` entry and `advanced(ctx)`; old root exports and old `Context` members marked `@deprecated` in the `.d.ts`; the snapshot test | The snapshot test passes; a plugin on the 0.19 SDK that uses only `advanced(ctx)` passes the event conformance cases on the 0.17.2 Node and Bun runtimes; the repository's own examples, templates and test plugins use the new locations |
| 2 | Python: `rutis.advanced`; the root's `__getattr__`; old `Context` methods warn; the snapshot test | The snapshot test passes; one test each for importing an old name, `from rutis import *` and `ctx.parallel`, checking they work and raise a `DeprecationWarning`, and fail under `-W error` |
| 3 | Go: the `advanced` subpackage (types aliased to the internal packages, as the root does today); `// Deprecated:` on the old root names; the snapshot test | The snapshot test passes; `staticcheck` reports SA1019 on the old spellings; `rutistest` and `examples` use the new locations |
| 4 | Swift: `Rutis.Advanced`, `ctx.advanced`, `Rutis._Generated`; the macros refer to `_Generated`; old names marked `deprecated`; the snapshot script | `RutisSelfTest` passes; the snapshot comparison passes; old spellings give warnings and no errors |
| 5 | Kotlin: `arcships.rutis.advanced`, `arcships.rutis.generated`; KSP refers to the new packages; old names `@Deprecated(WARNING)`; the Binary Compatibility Validator | `gradle check` (with `apiCheck`) passes; the Android sample still builds |
| 6 | Docs and CI: both copies of "Migrating to 0.19" get §8's section; `plugin-api.md`'s quick reference moves the advanced spellings to their new location (the guides' short path is #671's); ci.md §4 lists each job's snapshot check; release.md gets §5's sentence | The link check passes; against #692's "Done" list: default entry and submodules (1–5), snapshots and CI (1–5 and ci.md), both migration guides (6) |
