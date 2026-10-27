# Migrating to 0.19

[中文](migration-0.19.md)

## The SDKs' default entry points (#692)

From 0.19, the root of each language's SDK keeps only the names plugins are commonly written with; the rest is in advanced: TypeScript / Bun `@arcships/rutis/advanced`, Python `rutis.advanced`, Go `github.com/arcships/rutis/go/rutis/advanced`, Swift `Rutis.Advanced`, Kotlin `arcships.rutis.advanced`. The names only the code Swift macros and Kotlin KSP generate uses are in `Rutis._Generated` and `arcships.rutis.generated`.

**Nothing has to change.** The old forms still work in 0.19, and the editor, compiler or tests point to the new place; they are removed only when the plugin API version increases, and that release's notes say so again.

Only imports change: a name moved off the root (value types, `PLUGIN_API`, the types that describe events, the errors only one way of emitting raises, and so on) is imported from advanced under the same name; the full list is in [SDK entry points](design-sdk-entry-convergence-2026-10-27.en.md) §3. The few names added to the SDKs since follow the same rule: Python's `LoadError` is in `rutis.advanced`; Kotlin's `RemoteObject` in `arcships.rutis.advanced` and `ObjectProxy` in `arcships.rutis.generated`; Swift's `Rutis.located` in `Rutis._Generated`.

| Language | Old form | What you see | New form |
| --- | --- | --- | --- |
| TypeScript / Bun | `import { datetime } from '@arcships/rutis'` | Struck through in the editor; on hover: `'datetime' is deprecated. Moved in 0.19: import it from '@arcships/rutis/advanced'. Removed when the plugin API version changes.` | `import { datetime } from '@arcships/rutis/advanced'` |
| Python | `from rutis import AggregateError`, run under pytest | `DeprecationWarning: AggregateError moved in 0.19: import it from rutis.advanced. The old name is removed when the plugin API version changes.` (fails under `-W error`) | `from rutis.advanced import AggregateError` |
| Go | `rutis.DateOf(t)`, run `staticcheck` | `SA1019: rutis.DateOf is deprecated: moved in 0.19: use advanced.DateOf.` | Import `github.com/arcships/rutis/go/rutis/advanced` and write `advanced.DateOf(t)` |
| Swift | `let d: Rutis.DateTime` | `warning: 'DateTime' is deprecated: renamed to 'Rutis.Advanced.DateTime'`, with a fix-it | `let d: Rutis.Advanced.DateTime` |
| Kotlin | `RutisFormat.encode(…)` | `w: 'typealias RutisFormat = RutisFormat' is deprecated. Moved in 0.19: use arcships.rutis.advanced.RutisFormat.`; the IDE replaces it in one step and adds the import | `import arcships.rutis.advanced.RutisFormat` |

Two more points for Kotlin:

- Import the advanced names explicitly (the IDE's replacement adds explicit imports). In 0.19 both the old and the new names exist, so with both `import arcships.rutis.*` and `import arcships.rutis.advanced.*` the compiler reports `Overload resolution ambiguity` for `RutisFormat`, `Scope` and the like. Old code with only `import arcships.rutis.*` is not affected.
- These classes changed packages. A Kotlin plugin library published on its own, already compiled, has to be recompiled with the 0.19 SDK before it goes into an application with the 0.19 SDK (its source does not change).

Unchanged: `definePlugin` / `define_plugin` / `Define` / `Rutis.define` at each language's root, every method on `ctx`, the testing tools, Rust. The code Swift macros and Kotlin KSP generate is updated with the SDK and needs no change; the code 0.17.2's `rutis-ksp` generated still compiles against the 0.19 SDK, with deprecation warnings only. The plugin API version is unchanged: plugins built with an older SDK run on the 0.19 runtimes as before.
