# 各语言 SDK 的默认入口：根上只留插件作者要的名字

[English](design-sdk-entry-convergence-2026-10-27.en.md)

状态：草案，未实现。日期：2026-10-27。基准：main `103c0d22`。排期：0.19。
Issue：[#692](https://github.com/arcships/rutis/issues/692)（属于 [#663](https://github.com/arcships/rutis/issues/663) 体验要求 2、方向 3）。先例：[Rust API 收敛](design-rust-api-convergence-2026-10-24.md)（0.17）。
与 0.18 设计的关系：只换名字的位置，不改任何行为。错误类型名与字段沿用 [0.18 的诊断](design-diagnostics-2026-10-26.md) §十一 的"不变"；弃用提示的文字遵守它 §4.5 的禁用词；Go 的 `rutis.Ref` 沿用 [对象引用](design-object-refs-2026-10-27.md) O1 的写法，留在根上。

范围外：Rust（0.17 已收敛并由 `public-api.txt` 锁住，本稿不再动它，见 §2 E9）；测试工具上新增的名字（#670 做，本稿只把测试工具的公开面也锁进快照）；按层重写指南（#671，只用本稿 §三 定下的根上名字）；生成 Python 类型与假服务（#684）；Node 里的 Cordis 插件（用 Cordis 自己的 `ctx`）；改名 Python 的实现模块（`rutis.peer`、`rutis.events` 等，受 §五 的 Python 规则约束）；迁移工具（§八）；在运行时打印弃用提示；CI 检查指南里不出现 advanced（指南由 #671 写）。

## 一、开发者卡在哪

一个第三方开发者要给宿主写 `weather` 插件（#663 的评估任务）：提供 `weather.today(city)`，用另一个插件的 `llm.ask`，配置 `{ city }`，每次调用发出 `weather/asked`，写测试，在 `dev` 下跑。他要的就是插件作者那一层的 7 个概念：插件（定义 + `apply`）、服务（提供 / 使用）、`inject`、配置、事件（`on` / `emit`）、测试工具、`dev` 与行。

他在编辑器里打开补全，看到的却是：

| 语言 | 根上的名字 | `ctx` 上的成员 | 其中他用不到的 |
| --- | --- | --- | --- |
| TypeScript / Bun（`@arcships/rutis`，两者同一个包） | 20（10 个值、10 个类型） | 11 | `datetime` 等 5 个值构造、`PLUGIN`、`VALUE`、`isPlugin`、`PLUGIN_API`、监听器描述；`parallel`、`serial`、`waterfall`、`bail`、`filter` |
| Python（`rutis`） | 11 | 10 | 7 个错误类里的 5 个、`PLUGIN_API`；同上 5 个方法 |
| Go（`rutis`） | 52（issue 写时 49） | 12 | 值类型和它们的 14 个构造函数、`ServeArgs` / `ServeConn` / `Manifest`、`Shape` 一族；`Parallel` 等 5 个方法和 `Scope` |
| Swift（`Rutis`） | 38（顶层 10、`Rutis.` 下 28） | 13 | 值类型、宏展开用的 `encode` / `export` 等；同上 5 个方法、`scope`、`project` |
| Kotlin（`arcships.rutis`） | 30 顶层 + `Rutis` 对象 5 | 11 | KSP 生成代码用的 7 个类型、序列化工具；同上 5 个方法、`scope` |

哪几个是"写插件就要的"，没有任何地方说；指南也把这些放在主路径上（#663："指南放在主路径上、但没人用到的"）。Rust 在 0.17 已经解决了同样的问题：根上只留作者要的名字，其余放进 `rutis::host`，公开面由快照锁住。本稿把同样的做法用到其他 5 种 SDK。

## 二、决策

| # | 决策 | 章节 |
| --- | --- | --- |
| E1 | 每种语言的根上只放 7 个概念要的名字，加上本语言自己的"插件停下"写法（`signal`、`ctx.Go`、`spawn`、协程作用域）和服务调用失败的错误类型。清单见 §三 | 三 |
| E2 | 其余名字放进每种语言的 advanced 子模块：`@arcships/rutis/advanced`、`rutis.advanced`、`github.com/arcships/rutis/go/rutis/advanced`、`Rutis.Advanced`（Swift 命名空间）、`arcships.rutis.advanced`（Kotlin 包）。不删能力，行为不变 | 三 |
| E3 | `ctx` 上的方法这样移走：TypeScript、Python 用 `advanced(ctx)`，Go 用 `advanced.Of(ctx)`，Swift、Kotlin 用 `ctx.advanced`。TypeScript、Python、Go 的写法只在 SDK 一侧，运行时建出的 `ctx` 不变 | 四 |
| E4 | 旧位置在 0.19 照常可用，用各语言自带的弃用机制写明新位置；到插件 API 版本（`PLUGIN_API`）增加时，所有语言一起删除 | 五 |
| E5 | 0.19 不升 `PLUGIN_API` | 五 |
| E6 | Swift 宏、Kotlin KSP 生成的代码才用的名字移出根（`Rutis._Generated`、`arcships.rutis.generated`）。生成代码与 SDK 同版本，作者的代码不用改，不留旧位置 | 三 |
| E7 | 每种语言的公开面（根、advanced、测试工具、生成代码用的）各有一份快照，用本语言现成的工具生成，CI 比对 | 六 |
| E8 | 不做迁移工具 | 八 |
| E9 | Rust 不动 | 三 |

## 三、根上的名字

规则：一个名字留在根上，要么它就是 7 个概念之一的写法，要么写这 7 个概念时必须出现。不满足的放进 advanced。下表是 0.19 的最终清单；#671 的指南只用这些名字。

| 语言 | 根上的名字 | `ctx` 上的成员 |
| --- | --- | --- |
| TypeScript / Bun | `definePlugin`；类型 `Plugin`、`PluginSpec`、`Context`（4） | `use`、`provide`、`effect`、`signal`、`on`、`emit`（6） |
| Python | `define_plugin`、`Plugin`、`Context`、`RemoteError`、`BindingError`（5） | `use`、`provide`、`effect`、`on`、`emit`（5） |
| Go | `Plugin`、`Define`、`Definition`、`Serve`、`NoConfig`、`Ctx`、`Provides`、`ListenOption`、`Ref`、`RemoteError`、`ErrBinding`（11） | `Use`、`Provide`、`Effect`、`Go`、`On`、`Emit`（6），以及 `context.Context` 自己的方法 |
| Swift | `Plugin`、`Rutis.define`、`Rutis.Definition`、`Rutis.serve`、`Rutis.NoConfig`、`Rutis.Ctx`、`@RutisService`、`@RutisClient`、`@RutisConfig`、`RutisError`、`RutisApp`（11） | `use`、`provide`、`effect`、`spawn`、`on`、`emit`、`advanced`（7） |
| Kotlin | `Rutis.define`、`Rutis.NoConfig`、`Rutis.serve`、`Plugin`、`Ctx`、`EventArgs`、`@RutisService`、`@RutisClient`、`RutisException`、`RutisApp`、`RutisState`（11） | `use`、`provide`、`effect`、`on`、`emit`（5），以及协程作用域（`launch`）；`ctx.advanced` 是 advanced 包里的扩展属性 |
| 测试工具（各语言已是单独的模块） | `@arcships/rutis/testing`、`rutis.testing`、`rutistest`、`RutisTest`、`arcships.rutis.testing`：名字不变 | — |

逐项说明：

- **方法上的可选参数不算名字**：`on` 的 `prepend`、`global` 选项（TypeScript 的第三个参数、Python 的关键字参数、Swift / Kotlin 的默认参数）留在 `on` 上，不改写法。只有选项的类型名（TypeScript 的 `ListenerOptions`）和 Go 的构造函数 `rutis.Prepend()`、`rutis.Global()` 进 advanced。
- **签名里出现的类型**：TypeScript、Python、Swift、Kotlin 的根上签名可以引用 advanced 里的类型（`on` 的选项类型、`provides` 的 `MethodKind` / `ServiceShape` / `Rutis.Shape`），作者不手写这些名字。Go 的包不能互相导入，`Plugin.Provides` 字段和 `On` 的参数类型 `Provides`、`ListenOption` 只能留在根上；填它们用的 `MethodsOf`、`Sync`、`Rename`、`Prepend`、`Global` 在 advanced。
- **错误**：调用服务时会遇到的两种留在根上：对面抛出的（Python `RemoteError`、Go `RemoteError`）和平台产生、带 0.18 错误码的（Python `BindingError`、Go `ErrBinding`）。只在某种发出方式里出现的错误（`AggregateError` 属于 `parallel`，`WaterfallNext` 属于 `waterfall`，`AsyncListener`、`SyncWaitCycle` 属于 `bail` 与同步调用，`ReservedEvent` 属于发出 `rutis/*`）随这些方式进 advanced。Swift、Kotlin 只有一个错误类型，留在根上；TypeScript 按 `name` 匹配错误，没有错误类。
- **Go 的 `Ref`** 留在根上：Go 的结构体分不出数据和对象，按引用传对象只能写 `rutis.Ref(v)`（对象引用 O1），而这在其他语言里不用写任何东西。
- **值类型**（TypeScript `datetime` 等、Go `Date` 等、Swift `CalendarDate` 等）进 advanced：#692 正文把"各种值类型的构造"列为进阶。Python、Kotlin 用本语言自己的类型，没有这类名字。
- **`RutisApp`**（Swift、Kotlin）留在根上：它是应用托管插件的唯一入口，每个用 rutis 的应用都要写；移走只会让每个应用多一行导入。
- **生成代码用的**（E6）：Swift 的 `Rutis.encode`、`export`、`Encoded`、`Arguments`、`Method`、`Schema`（实施时以宏的实际输出为准）进 `Rutis._Generated`（下划线开头，Swift 的补全与文档不显示）；协议 `RutisExportable`、`RutisProxy`、`RutisConfigurable` 出现在根上的签名里，留在顶层。Kotlin 的 `ServiceTable`、`ServiceHandle`、`ClientFactory`、`Tables`、`Generated`、`ValueEncoder`、`ValueDecoder` 进 `arcships.rutis.generated`。
- **Rust 不动**（E9）：0.17 的 Rust `Ctx` 上仍有五种发出方式和 `filter`。0.17 的验收口径是"作者手写的名字 ≤ 10、根与 `rutis::events` 由快照锁住"，已经满足；再挪一次就是 Rust 的第二次不兼容，而指南的短路径（#671）在 Rust 里同样只讲 `on` / `emit`。

每种语言 advanced 里的名字：

| 语言 | advanced 里的名字 |
| --- | --- |
| TypeScript / Bun | `advanced`、类型 `Advanced`；`PLUGIN_API`、`PLUGIN`、`isPlugin`、`VALUE`、`datetime`、`date`、`time`、`decimal`、`uuid`；类型 `TaggedValue`、`MethodKind`、`ListenerDescriptor`、`ListenerOptions`、`Listener`、`Emitter`、`Waterfall` |
| Python | `advanced`、`Advanced`、`PLUGIN_API`、`AggregateError`、`AsyncListener`、`ReservedEvent`、`SyncWaitCycle`、`WaterfallNext` |
| Go | `Of`、`Ctx`；`Implementation`、`PluginAPI`、`Version`；`ErrReservedEvent`、`ErrWaterfallNext`、`ErrAsyncListener`、`ErrSyncWaitCycle`、`AggregateError`；`Manifest`、`Project`、`ServeArgs`、`ServeConn`；`Date`、`DateOf`、`ParseDate`、`DateTime`、`DateTimeOf`、`ParseDateTime`、`Decimal`、`ParseDecimal`、`TimeOfDay`、`TimeOfDayOf`、`ParseTimeOfDay`、`UUID`、`ParseUUID`；`Emitter`、`Listener`、`Next`、`Terminal`、`Scope`、`Global`、`Prepend`；`Function`、`Future`、`Service`、`Schemer`；`Shape`、`ShapeOption`、`MethodsOf`、`Sync`、`Rename`（44） |
| Swift | `Rutis.Advanced.Ctx`（`ctx.advanced` 的类型）；`sdkVersion`、`pluginAPI`、`implementation`；`CalendarDate`、`TimeOfDay`、`DateTime`、`BigInt`；`serveArgs`、`manifest`；`Emitter`、`Scope`、`Listener`、`Next`；`Client`、`RemoteFunction`；`MethodKind`、`Shape` |
| Kotlin | `Advanced`（`ctx.advanced` 的类型）、扩展属性 `advanced`；`VERSION`、`PLUGIN_API`、`IMPLEMENTATION`；`project`、`manifest`；`Emitter`、`Scope`、`Listener`、`Next`；`RemoteFunction`；`Kind`、`ServiceShape`、`service`；`RutisFormat`、`AnySerializer`、`JdkSerializers` |

## 四、方法怎样移走

方法不能放进子模块，所以每种语言给一个"进阶视图"：它上面是从根上移走的那些方法，名字和参数与原来相同。

| 语言 | 写法 | 为什么这样 |
| --- | --- | --- |
| TypeScript / Bun | `import { advanced } from '@arcships/rutis/advanced'`，`await advanced(ctx).serial('x', 1)` | `ctx` 由运行时（`@arcships/rutis-runtime`、`@arcships/rutis-bun`）建出，插件各带一份 SDK。`advanced(ctx)` 只在 SDK 里，调用的是 `ctx` 已有的方法，所以用 0.19 SDK 的插件在旧运行时上照样能跑，不用升 `PLUGIN_API`。在 `ctx` 上加成员就要求运行时也升级 |
| Python | `from rutis.advanced import advanced`，`await advanced(ctx).serial("x", 1)` | 与 TypeScript 同一种写法 |
| Go | `import "github.com/arcships/rutis/go/rutis/advanced"`，`advanced.Of(ctx).Serial(ctx, "x", 1)` | Go 的根包不能引用子包的类型（会成环），所以不能是 `ctx.Advanced()`；子包里的函数是 Go 的常见写法 |
| Swift | `try await ctx.advanced.serial("x", 1, as: Int.self)` | Swift 没有子模块；命名空间加一个属性是 Swift 的写法。SDK 编进插件，加成员不影响别的版本 |
| Kotlin | `import arcships.rutis.advanced.advanced`，`ctx.advanced.serial("x", 1)` | 扩展属性要导入才看得见，和 Rust 0.17 的扩展 trait 一样；IDE 会自动补上导入 |

进阶视图上的方法：

| 语言 | 方法 |
| --- | --- |
| TypeScript / Bun、Python | `parallel`、`serial`、`waterfall`、`bail`、`filter` |
| Go | `Parallel`、`Serial`、`Waterfall`、`Bail`、`Filter`、`Scope` |
| Swift | `parallel`、`serial`、`waterfall`、`bail`、`filter`（各自的 `arguments:` 形式一起）、`scope`、`project` |
| Kotlin | `parallel`、`serial`、`waterfall`、`bail`、`filter`、`scope` |

`emit` 留在根上，只有它一种发出方式；`filter` 返回的发出器仍有五种方式。监听一侧不变：`waterfall` 的监听器照样收到 `next`。

## 五、旧位置与插件 API 版本

**规则**：旧位置在 0.19 照常可用，提示新位置；到 `PLUGIN_API` 增加时，所有语言一起删除。

为什么不在 0.19 直接删：

- Python 不能删。插件只写 `rutis>=` 下限，和运行时共用同一个包，所以 `PLUGIN_API` 增加之前不删、不改插件导入的名字（#692 评论，[release.md](release.md)、[plugin-api.md](guide/plugin-api.md) 已写明）。
- TypeScript 与 Go 的编译器对"找不到这个名字"只报名字，不能附上新位置，而 #692 要求旧写法"给出明确的报错或提示（写明新位置）"。保留旧名字并标为弃用，才能写明新位置。
- Swift、Kotlin 本来可以用"报错并附文字"的机制直接删。但只有它们俩不兼容，换来的只是补全里少几个划掉的名字，规则却要多一条；统一成一条规则更简单。

所以 0.19 里 #692 对任何人都不是必须改的；不兼容推迟到 `PLUGIN_API` 增加的那一版，那时运行时照常加载旧插件 API 的插件（[release.md](release.md) 的规则），作者升级 SDK 时才需要改。

各语言用自带的机制：

| 语言 | 旧写法 | 提示出现在 |
| --- | --- | --- |
| TypeScript / Bun | 根上的旧导出和 `Context` 上的旧成员在 `.d.ts` 里标 `@deprecated`，写明新位置；运行时不变 | 编辑器（划线、悬停文字）；`typescript-eslint` 的 `no-deprecated` |
| Python | 旧的根上名字由模块的 `__getattr__`（PEP 562）给出，第一次取用时发 `DeprecationWarning`；它们继续留在 `__all__` 里（`from rutis import *` 也不能变）。`ctx.parallel` 等照常调用，发同样的警告 | pytest 的警告汇总、`python -W default`；`-W error` 可以让测试在用到旧名字时失败 |
| Go | 旧名字保留为别名或转发，注释 `// Deprecated: …` | gopls（划线）、`staticcheck` 的 SA1019 |
| Swift | `@available(*, deprecated, message:)`；类型用 `renamed:` | 编译警告；Xcode 的修改建议 |
| Kotlin | `@Deprecated(message, ReplaceWith(…))`，级别 WARNING | 编译警告；IDE 一键替换 |

**0.19 不升 `PLUGIN_API`**：`PLUGIN_API` 是插件与运行时之间的约定，只在插件看到的接口不兼容时增加（[release.md](release.md)）。本稿只在 SDK 一侧加了模块和视图，运行时建出的 `ctx`、协议都不变，没有不兼容；升了它反而要求所有已发布的插件重新构建（#663 的 A8）。实施时在 release.md 的那条规则后补一句："`PLUGIN_API` 增加时，删除 0.19 起各 SDK 标为弃用的旧位置。"

快照把弃用的名字单独列成一节（§六）：它们不能再增加，删除时整节去掉。

## 六、公开面快照与 CI

和 Rust 的 `public-api` 一样：每种语言把公开面写成一份文本快照，提交进仓库，CI 重新生成并比较。加一个名字就要改快照，评审时看得见。快照分节：根、advanced、测试工具、生成代码用的（Swift、Kotlin）、弃用的旧位置。

测试工具也锁进快照。它不算 SDK 根，但它是插件作者直接写的代码；#670 给它加 `on` 等方法时，在同一个 PR 里更新快照。

| 语言 | 快照 | 怎样生成 | 跑在哪个 CI 任务 | 有意改动时 |
| --- | --- | --- | --- | --- |
| TypeScript / Bun | `node/rutis/public-api.txt` | `node/rutis/test/public-api.test.mjs`：用 TypeScript 编译器 API 读 `exports` 里每个入口（`.`、`./advanced`、`./testing`）的 `.d.ts`，列出导出的名字、接口与类的成员和签名、`@deprecated` 标记；再导入每个入口，检查运行时的导出与 `.d.ts` 一致。`typescript` 作为开发依赖，版本与 `rutis-runtime` 相同（6.0.3） | `js-py`（`node/rutis` 的 `npm test`） | `RUTIS_PUBLIC_API_WRITE=1 npm test` |
| Python | `python/rutis/public-api.txt` | `python/rutis/tests/test_public_api.py`：`rutis`、`rutis.advanced`、`rutis.testing` 的 `__all__`，其中每个类的公开成员与 `inspect.signature`；并检查 `dir()` 里的公开名字都在 `__all__` 里或是子模块 | `js-py`（Python 单元测试） | `RUTIS_PUBLIC_API_WRITE=1` 跑这个测试 |
| Go | `go/rutis/public-api.txt` | `go/rutis/public_api_test.go`：用标准库 `go/parser`、`go/doc` 列出 `rutis`、`rutis/advanced`、`rutis/rutistest` 的导出声明、方法与签名、Deprecated 标记（Go 常用的 golden 文件写法） | `runtimes-go`（`go test`） | `go test -run TestPublicAPI -update` |
| Swift | `swift/rutis/public-api.txt` | `tools/public-api-swift.sh`：`swift package dump-symbol-graph --minimum-access-level public`，再由 `tools/public-api/swift.mjs` 把 `Rutis`、`RutisTest` 的符号图写成排好序的一行一个声明 | `runtimes-swift` | `tools/public-api-swift.sh --write` |
| Kotlin | `kotlin/rutis/rutis/api/` | Kotlin 官方的 Binary Compatibility Validator（Gradle 插件），`apiCheck` 挂在 `check` 上 | `runtimes-kotlin` | `./gradlew apiDump` |
| Rust | `crates/rutis/public-api.txt` | 不变（`tools/public-api.sh`） | `public-api` | 不变 |

选择的理由：TypeScript 的 `.d.ts` 是手写的，api-extractor 要先有编译输出、配置也多，编译器 API 直接读手写的 `.d.ts` 更轻；Python 只有 `__all__` 是约定的公开面；Go 用标准库，不加依赖；Swift 用工具链自带的符号图；Kotlin 用官方插件。全部放进已有的任务，不新开任务（[ci.md](ci.md) §6）。实施时在 ci.md §4 的各任务行里写上这一项。

## 七、开发者看到什么

**写一个普通插件**（不导入 advanced）：

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

在 `ctx.` 后面补全，看到 `use`、`provide`、`effect`、`signal`、`on`、`emit`，以及划掉的 5 个旧方法（0.19 起，到 `PLUGIN_API` 增加时消失）。Python、Go、Swift、Kotlin 相同，名字见 §三。

**用到进阶的东西**：

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

**旧代码不改**：照常运行，提示新位置。

| 语言 | 输入 | 看到 |
| --- | --- | --- |
| TypeScript | `await ctx.parallel('cache/flush')` | 编辑器划线，悬停：`'parallel' is deprecated. Moved in 0.19: advanced(ctx).parallel(…), with advanced from '@arcships/rutis/advanced'. Removed when the plugin API version changes.` |
| Python | `from rutis import AggregateError`，在 pytest 里跑 | `DeprecationWarning: AggregateError moved in 0.19: import it from rutis.advanced. The old name is removed when the plugin API version changes.` |
| Python | `await ctx.parallel("cache/flush")` | `DeprecationWarning: ctx.parallel moved in 0.19: use advanced(ctx).parallel, with advanced from rutis.advanced. …` |
| Go | `ctx.Parallel(ctx, "cache/flush")`，跑 `staticcheck` | `SA1019: ctx.Parallel is deprecated: moved in 0.19: use advanced.Of(ctx).Parallel.` |
| Swift | `try await ctx.parallel("cache/flush")` | `warning: 'parallel' is deprecated: moved in 0.19: use ctx.advanced.parallel` |
| Swift | `let d: Rutis.DateTime` | `warning: 'DateTime' is deprecated: renamed to 'Rutis.Advanced.DateTime'`，附修改建议 |
| Kotlin | `ctx.parallel("cache/flush")` | `w: 'suspend fun parallel(name: String, vararg args: Any?): Unit' is deprecated. Moved in 0.19: use ctx.advanced.parallel.`；IDE 的 "Replace with 'advanced.parallel(name, *args)'" 一并补上导入 |

**rutis 的贡献者加了一个公开名字，没有更新快照**：CI 失败，输出快照的差异和改法，例如：

```
--- node/rutis/public-api.txt
+++ current
@@ ./advanced @@
+export function retry(…)
the public API of @arcships/rutis changed: review the diff, then run RUTIS_PUBLIC_API_WRITE=1 npm test
```

其他语言一样，最后一行写出本语言更新快照的命令（§六）。

## 八、兼容与迁移

写进《迁移到 0.19》（两份）中"SDK 的默认入口（#692）"一节：

> 0.19 起，各语言 SDK 的根上只留写插件常用的名字，其余放进 advanced：TypeScript / Bun `@arcships/rutis/advanced`，Python `rutis.advanced`，Go `github.com/arcships/rutis/go/rutis/advanced`，Swift `Rutis.Advanced`，Kotlin `arcships.rutis.advanced`。
>
> **不改也能跑**。旧的写法在 0.19 照常可用，编辑器、编译器或测试会提示新位置；到插件 API 版本增加时才删除，那一版的发布说明会再写一次。
>
> 要改的写法：
>
> | 旧 | 新 |
> | --- | --- |
> | `ctx.parallel / serial / waterfall / bail / filter(…)`（TypeScript、Python） | `advanced(ctx).parallel(…)` 等，`advanced` 从 advanced 子模块导入 |
> | `ctx.Parallel / Serial / Waterfall / Bail / Filter / Scope(…)`（Go） | `advanced.Of(ctx).Parallel(…)` 等 |
> | `ctx.parallel … / ctx.scope / ctx.project`（Swift）、`ctx.parallel … / ctx.scope`（Kotlin） | `ctx.advanced.parallel …`（Kotlin 要 `import arcships.rutis.advanced.advanced`） |
> | 根上的其他名字（值类型、`PLUGIN_API`、事件的描述类型、只在某种发出方式里出现的错误等） | 同名，从 advanced 导入；完整列表见 [SDK 的默认入口](design-sdk-entry-convergence-2026-10-27.md) §三 |
>
> 不变的：每种语言根上的 `definePlugin` / `define_plugin` / `Define` / `Rutis.define`，`ctx` 上的 `use`、`provide`、`effect`、`on`、`emit`，`on` 的 `prepend`、`global` 选项，测试工具，Rust。Swift 宏与 Kotlin KSP 生成的代码随 SDK 一起更新，不用改。插件 API 版本不变，用旧 SDK 构建的插件在 0.19 的运行时上照常运行。

**不做迁移工具**（E8）：Rust 的 `rutis-migrate` 是因为 0.17 改了 trait 的签名（`BoxFuture`、`Effect`、依赖的写法），手改容易错；这里只是换导入的位置，旧写法还能用，Swift、Kotlin 的 IDE 有一键替换，其余三种语言每处改一行。#663 的评估里普通插件只用到根上的名字，大多数插件一处都不用改。

## 九、实施

每种语言一个 PR，互不依赖；第 6 步在最后。

| 步 | 内容 | 完成条件 |
| --- | --- | --- |
| 1 | TypeScript / Bun：`node/rutis` 加 `./advanced` 入口与 `advanced(ctx)`；根上的旧导出与 `Context` 的旧成员在 `.d.ts` 里标 `@deprecated`；快照测试 | 快照测试通过；一个用 0.19 SDK、只用 `advanced(ctx)` 的插件在 0.17.2 的 Node 与 Bun 运行时上通过事件一致性用例；仓库自己的示例、模板、测试插件改用新位置 |
| 2 | Python：`rutis.advanced`；根的 `__getattr__`；`Context` 旧方法发警告；快照测试 | 快照测试通过；旧名字的导入、`from rutis import *`、`ctx.parallel` 各有一个测试，确认可用且发出 `DeprecationWarning`，`-W error` 下失败 |
| 3 | Go：`advanced` 子包（类型以别名指向内部包，与现在的根相同）；根上旧名字加 `// Deprecated:`；快照测试 | 快照测试通过；`staticcheck` 对旧写法报 SA1019；`rutistest` 与 `examples` 改用新位置 |
| 4 | Swift：`Rutis.Advanced`、`ctx.advanced`、`Rutis._Generated`；宏改为引用 `_Generated`；旧名字标 `deprecated`；快照脚本 | `RutisSelfTest` 通过；快照比对通过；旧写法只有警告没有错误 |
| 5 | Kotlin：`arcships.rutis.advanced`、`arcships.rutis.generated`；KSP 改为引用新包；旧名字 `@Deprecated(WARNING)`；接入 Binary Compatibility Validator | `gradle check`（含 `apiCheck`）通过；Android 示例照常构建 |
| 6 | 文档与 CI：《迁移到 0.19》两份写入 §八 的一节；`plugin-api.md` 的速查表把进阶写法改到新位置（指南的短路径由 #671 改）；ci.md §4 写上各任务的快照检查；release.md 补 §五 的一句 | 链接检查通过；对照 #692 的「完成」清单：默认入口与子模块（1–5）、快照与 CI（1–5 与 ci.md）、迁移指南两份（6） |
