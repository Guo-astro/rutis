# 迁移到 0.19

[English](migration-0.19.en.md)

## SDK 的默认入口（#692）

0.19 起，各语言 SDK 的根上只留写插件常用的名字，其余放进 advanced：TypeScript / Bun `@arcships/rutis/advanced`，Python `rutis.advanced`，Go `github.com/arcships/rutis/go/rutis/advanced`，Swift `Rutis.Advanced`，Kotlin `arcships.rutis.advanced`。只有 Swift 宏与 Kotlin KSP 生成的代码用的名字放进 `Rutis._Generated` 与 `arcships.rutis.generated`。

**不改也能跑**。旧的写法在 0.19 照常可用，编辑器、编译器或测试会提示新位置；到插件 API 版本增加时才删除，那一版的发布说明会再写一次。

要改的只有导入：根上移走的名字（值类型、`PLUGIN_API`、事件的描述类型、只在某种发出方式里出现的错误等）同名从 advanced 导入；完整列表见 [SDK 的默认入口](design-sdk-entry-convergence-2026-10-27.md) §三。在那之后加进 SDK 的几个名字按同样的规则放：Python 的 `LoadError` 在 `rutis.advanced`；Kotlin 的 `RemoteObject` 在 `arcships.rutis.advanced`，`ObjectProxy` 在 `arcships.rutis.generated`；Swift 的 `Rutis.located` 在 `Rutis._Generated`。

| 语言 | 旧写法 | 看到 | 改成 |
| --- | --- | --- | --- |
| TypeScript / Bun | `import { datetime } from '@arcships/rutis'` | 编辑器划线，悬停：`'datetime' is deprecated. Moved in 0.19: import it from '@arcships/rutis/advanced'. Removed when the plugin API version changes.` | `import { datetime } from '@arcships/rutis/advanced'` |
| Python | `from rutis import AggregateError`，在 pytest 里跑 | `DeprecationWarning: AggregateError moved in 0.19: import it from rutis.advanced. The old name is removed when the plugin API version changes.`（`-W error` 下失败） | `from rutis.advanced import AggregateError` |
| Go | `rutis.DateOf(t)`，跑 `staticcheck` | `SA1019: rutis.DateOf is deprecated: moved in 0.19: use advanced.DateOf.` | 导入 `github.com/arcships/rutis/go/rutis/advanced`，写 `advanced.DateOf(t)` |
| Swift | `let d: Rutis.DateTime` | `warning: 'DateTime' is deprecated: renamed to 'Rutis.Advanced.DateTime'`，附修改建议 | `let d: Rutis.Advanced.DateTime` |
| Kotlin | `RutisFormat.encode(…)` | `w: 'typealias RutisFormat = RutisFormat' is deprecated. Moved in 0.19: use arcships.rutis.advanced.RutisFormat.`；IDE 一键替换并补上导入 | `import arcships.rutis.advanced.RutisFormat` |

Kotlin 另有两点：

- 进阶的名字用显式导入（IDE 一键替换补的就是显式导入）。0.19 里旧名字和新名字都在，同时写 `import arcships.rutis.*` 和 `import arcships.rutis.advanced.*` 时，编译器对 `RutisFormat`、`Scope` 等报 `Overload resolution ambiguity`。只写 `import arcships.rutis.*` 的旧代码不受影响。
- 这些类换了包。单独发布、预先编译好的 Kotlin 插件库要用 0.19 的 SDK 重新编译后，才能和 0.19 的 SDK 放进同一个应用（源代码不用改）。

不变的：每种语言根上的 `definePlugin` / `define_plugin` / `Define` / `Rutis.define`，`ctx` 上的所有方法，测试工具，Rust。Swift 宏与 Kotlin KSP 生成的代码随 SDK 一起更新，不用改；0.17.2 的 `rutis-ksp` 生成的代码在 0.19 的 SDK 上照样编译，只有弃用警告。插件 API 版本不变，用旧 SDK 构建的插件在 0.19 的运行时上照常运行。
