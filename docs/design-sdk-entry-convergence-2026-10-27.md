# 各语言 SDK 的默认入口：根上只留插件作者要的名字

[English](design-sdk-entry-convergence-2026-10-27.en.md)

状态：定稿，未实现。日期：2026-10-27。基准：main `103c0d22`。排期：0.19。
Issue：[#692](https://github.com/arcships/rutis/issues/692)（属于 [#663](https://github.com/arcships/rutis/issues/663) 体验要求 2、方向 3）。先例：[Rust API 收敛](design-rust-api-convergence-2026-10-24.md)（0.17）。
与 0.18 设计的关系：只换名字的位置，不改任何行为。错误类型名与字段沿用 [0.18 的诊断](design-diagnostics-2026-10-26.md) §十一 的"不变"；弃用提示的文字遵守它 §4.5 的禁用词；Go 的 `rutis.Ref` 沿用 [对象引用](design-object-refs-2026-10-27.md) O1 的写法，留在根上。

范围外：Rust（0.17 已收敛并由 `public-api.txt` 锁住，见 §2 E9）；测试工具上新增的名字（#670 做，本稿只把测试工具的公开面也锁进快照）；按层重写指南（#671，只用本稿 §三 定下的根上名字）；生成 Python 类型与假服务（#684）；Node 里的 Cordis 插件（用 Cordis 自己的 `ctx`）；改名 Python 的实现模块（`rutis.peer`、`rutis.events` 等）；迁移工具（§八）；在运行时打印弃用提示。

## 一、开发者卡在哪

一个第三方开发者要给宿主写 `weather` 插件（#663 的评估任务）：提供 `weather.today(city)`，用另一个插件的 `llm.ask`，配置 `{ city }`，每次调用发出 `weather/asked`，写测试，在 `dev` 下跑。他要的就是插件作者那一层的 7 个概念：插件（定义 + `apply`）、服务（提供 / 使用）、`inject`、配置、事件（`on` / `emit`）、测试工具、`dev` 与行。

他在编辑器里输入 `rutis.`（或从根上导入），补全里看到的却是：

| 语言 | 根上的名字 | 其中他用不到的 |
| --- | --- | --- |
| TypeScript / Bun（`@arcships/rutis`，两者同一个包） | 20（10 个值、10 个类型） | `datetime` 等 5 个值构造、`PLUGIN`、`VALUE`、`isPlugin`、`PLUGIN_API`、监听器描述 |
| Python（`rutis`） | 11 | 7 个错误类里的 5 个、`PLUGIN_API` |
| Go（`rutis`） | 52（issue 写时 49） | 值类型和它们的 14 个构造函数、`ServeArgs` / `ServeConn` / `Manifest`、版本常量 |
| Swift（`Rutis`） | 38（顶层 10、`Rutis.` 下 28） | 值类型、宏展开用的 `encode` / `export` 等 |
| Kotlin（`arcships.rutis`） | 30 顶层 + `Rutis` 对象 5 | KSP 生成代码用的 7 个类型、序列化工具 |

哪几个是"写插件就要的"，没有任何地方说。Rust 在 0.17 已经解决了同样的问题：根上只留作者要的名字，其余放进 `rutis::host`，公开面由快照锁住。本稿把同样的做法用到其他 5 种 SDK 的根上的名字。`ctx` 上的方法不动（§四）。

## 二、决策

| # | 决策 | 章节 |
| --- | --- | --- |
| E1 | 每种语言的根上只放 7 个概念要的名字，加上服务调用失败的错误类型。清单见 §三 | 三 |
| E2 | 其余名字放进每种语言的 advanced 子模块：`@arcships/rutis/advanced`、`rutis.advanced`、`github.com/arcships/rutis/go/rutis/advanced`、`Rutis.Advanced`（Swift 命名空间）、`arcships.rutis.advanced`（Kotlin 包）。不删能力，行为不变 | 三 |
| E3 | `ctx` 上的方法（五种发出方式、`filter`、作用域）不移、不弃用，和 Cordis、Rust 0.17 一样留在 `ctx` 上；指南（#671）只讲 `on` / `emit` | 四 |
| E4 | 旧位置在 0.19 照常可用，用各语言自带的弃用机制写明新位置；到插件 API 版本（`PLUGIN_API`）增加时，所有语言一起删除 | 五 |
| E5 | 0.19 不升 `PLUGIN_API` | 五 |
| E6 | Swift 宏、Kotlin KSP 生成的代码才用的名字移到 `Rutis._Generated`、`arcships.rutis.generated`，旧位置按 E4 处理 | 三 |
| E7 | 每种语言的公开面（根、advanced、`ctx`、测试工具、生成代码用的）由本语言最现成的工具锁住，CI 比对 | 六 |
| E8 | 不做迁移工具 | 八 |
| E9 | Rust 不动 | 三 |

## 三、根上的名字

规则：一个名字留在根上，要么它就是 7 个概念之一的写法，要么写这 7 个概念时必须出现。不满足的放进 advanced。下表是 0.19 的最终清单；#671 的指南只用这些名字。

| 语言 | 根上的名字 |
| --- | --- |
| TypeScript / Bun | `definePlugin`；类型 `Plugin`、`PluginSpec`、`Context`（4） |
| Python | `define_plugin`、`Plugin`、`Context`、`RemoteError`、`BindingError`（5） |
| Go | 作者写的：`Plugin`、`Define`、`Definition`、`Serve`、`NoConfig`、`Ctx`、`Provides`、`MethodsOf`、`Sync`、`Ref`、`RemoteError`、`ErrBinding`（12）；只出现在签名里的：`Shape`、`ShapeOption`、`ListenOption`、`Emitter`、`Listener`、`Terminal`、`Scope`（7） |
| Swift | `Plugin`、`Rutis.define`、`Rutis.Definition`、`Rutis.serve`、`Rutis.NoConfig`、`Rutis.Ctx`、`@RutisService`、`@RutisClient`、`@RutisConfig`、`RutisError`、`RutisApp`（11） |
| Kotlin | `Rutis.define`、`Rutis.NoConfig`、`Rutis.serve`、`Plugin`、`Ctx`、`EventArgs`、`service`、`@RutisService`、`@RutisClient`、`RutisException`、`RutisApp`、`RutisState`（12） |
| 测试工具（各语言已是单独的模块） | `@arcships/rutis/testing`、`rutis.testing`、`rutistest`、`RutisTest`、`arcships.rutis.testing`：名字不变 |

逐项说明：

- **签名里出现的类型**：TypeScript、Python、Swift、Kotlin 的根上签名（包括 `ctx` 的方法）可以引用 advanced 里的类型（`Emitter`、`ListenerOptions`、`MethodKind`、`ServiceShape`、`Rutis.Shape` 等），作者不手写这些名字。Go 的 advanced 包以别名指向根上的类型，根包不能反过来引用 advanced（会成环），所以 `Plugin.Provides` 和 `Ctx` 方法签名里的 7 个类型留在根上。
- **`provides` 的写法留在根上**：Go 的 `MethodsOf`、`Sync` 和 Kotlin 的 `service` 是声明提供的服务的写法，Go 模板（`crates/rutis-host/templates/go/plugin.go`）和 `plugin-api.md` 都直接写它们。Go 的 `Rename` 进 advanced。
- **`on` 的可选参数不算名字**：`prepend`、`global` 选项留在 `on` 上。只有 Go 的构造函数 `rutis.Prepend()`、`rutis.Global()` 进 advanced。
- **错误**：调用服务时会遇到的两种留在根上：对面抛出的（Python `RemoteError`、Go `RemoteError`）和平台产生、带 0.18 错误码的（Python `BindingError`、Go `ErrBinding`）。只在某种发出方式里出现的错误（`AggregateError`、`WaterfallNext`、`AsyncListener`、`ReservedEvent`）以及 `SyncWaitCycle`（它是 `RemoteError` 的子类，按 `RemoteError` 也能接住）进 advanced。Swift、Kotlin 只有一个错误类型，留在根上；TypeScript 按 `name` 匹配错误，没有错误类。
- **Go 的 `Ref`** 留在根上：Go 的结构体分不出数据和对象，按引用传对象只能写 `rutis.Ref(v)`（对象引用 O1）。
- **值类型**（TypeScript `datetime` 等、Go `Date` 等、Swift `CalendarDate` 等）进 advanced：#692 正文把"各种值类型的构造"列为进阶。
- **`RutisApp`、`RutisState`**（Swift、Kotlin）留在根上：它们是应用托管插件的入口，每个用 rutis 的应用都要写；Kotlin 只有这一种用法。
- **生成代码用的**（E6）：作者不写这些名字，但它们出现在 `Rutis.` 和 `arcships.rutis` 的补全里。Swift 的 `Rutis.encode`、`export`、`Encoded`、`Arguments`、`Method`、`Schema` 进 `Rutis._Generated`（下划线开头，补全不显示）；协议 `RutisExportable`、`RutisProxy`、`RutisConfigurable` 出现在根上的签名里，留在顶层。Kotlin 的 `ServiceTable`、`ServiceHandle`、`ClientFactory`、`Tables`、`Generated`、`ValueEncoder`、`ValueDecoder` 进 `arcships.rutis.generated`。旧位置也按 E4 保留：Kotlin 的 `rutis-ksp` 是单独的 Maven 坐标，版本由作者另写，旧版 KSP 生成的代码在 0.19 的 SDK 上要照样能编译。
- **Rust 不动**（E9）：0.17 的验收口径（作者手写的名字 ≤ 10、根与 `rutis::events` 由快照锁住）已经满足；Rust 的 `Ctx` 上也保留五种发出方式和 `filter`，与 §四 一致。

每种语言 advanced 里的名字：

| 语言 | advanced 里的名字 |
| --- | --- |
| TypeScript / Bun | `PLUGIN_API`、`PLUGIN`、`isPlugin`、`VALUE`、`datetime`、`date`、`time`、`decimal`、`uuid`；类型 `TaggedValue`、`MethodKind`、`ListenerDescriptor`、`ListenerOptions`、`Listener`、`Emitter`、`Waterfall` |
| Python | `PLUGIN_API`、`AggregateError`、`AsyncListener`、`ReservedEvent`、`SyncWaitCycle`、`WaterfallNext` |
| Go | `Implementation`、`PluginAPI`、`Version`；`ErrReservedEvent`、`ErrWaterfallNext`、`ErrAsyncListener`、`ErrSyncWaitCycle`、`AggregateError`；`Manifest`、`Project`、`ServeArgs`、`ServeConn`；`Date`、`DateOf`、`ParseDate`、`DateTime`、`DateTimeOf`、`ParseDateTime`、`Decimal`、`ParseDecimal`、`TimeOfDay`、`TimeOfDayOf`、`ParseTimeOfDay`、`UUID`、`ParseUUID`；`Next`、`Global`、`Prepend`、`Rename`；`Function`、`Future`、`Service`、`Schemer`（33） |
| Swift | `sdkVersion`、`pluginAPI`、`implementation`；`CalendarDate`、`TimeOfDay`、`DateTime`、`BigInt`；`serveArgs`、`manifest`；`Emitter`、`Scope`、`Listener`、`Next`；`Client`、`RemoteFunction`；`MethodKind`、`Shape` |
| Kotlin | `VERSION`、`PLUGIN_API`、`IMPLEMENTATION`；`project`、`manifest`；`Emitter`、`Scope`、`Listener`、`Next`；`RemoteFunction`；`Kind`、`ServiceShape`；`RutisFormat`、`AnySerializer`、`JdkSerializers` |

## 四、`ctx` 上的方法不动

`ctx` 上的 `parallel`、`serial`、`waterfall`、`bail`、`filter`，以及作用域（Go `Scope`、Swift `scope` / `project`、Kotlin `scope`），都留在 `ctx` 上，不移、不标弃用。理由：

- Cordis 4 的 `ctx` 上就有 `parallel`、`serial`、`bail`、`waterfall`（`cordis/src/events.ts`），Rust 0.17 的 `Ctx` 也有。把它们挪到 `advanced(ctx)` 一类的视图，作者要多学一种 rutis 特有的写法，各语言也和 Cordis、Rust 不一致。
- #663 指出的是"指南放在主路径上、却没人用到"；指南只讲 `on` / `emit`（#671）就解决了，方法在补全里出现不妨碍写插件。#692 的"要做"只要求根上的名字收进子模块。
- `ctx` 不变，运行时建出的 `ctx` 和 SDK 的类型就一致，不用考虑新 SDK 配旧运行时。

快照照样列出 `ctx` 的成员（§六），加方法要改快照。

## 五、旧位置与插件 API 版本

**规则**：旧位置在 0.19 照常可用，提示新位置；到 `PLUGIN_API` 增加时，所有语言一起删除。

为什么不在 0.19 直接删：

- Python 不能删。插件只写 `rutis>=` 下限，和运行时共用同一个包，所以 `PLUGIN_API` 增加之前不删、不改插件导入的名字（#692 评论，[release.md](release.md)、[plugin-api.md](guide/plugin-api.md) 已写明）。
- TypeScript 与 Go 的编译器对"找不到这个名字"只报名字，不能附上新位置，而 #692 要求旧写法"给出明确的报错或提示（写明新位置）"。保留旧名字并标为弃用，才能写明新位置。
- Swift、Kotlin 本来可以直接删，但统一成一条规则更简单。

所以 0.19 里 #692 对任何人都不是必须改的；作者到 `PLUGIN_API` 增加的那一版升级 SDK 时才需要改。

| 语言 | 旧写法 | 提示出现在 |
| --- | --- | --- |
| TypeScript / Bun | 根上的旧导出在 `.d.ts` 里标 `@deprecated`，写明新位置；运行时不变 | 编辑器（划线、悬停文字）；`typescript-eslint` 的 `no-deprecated` |
| Python | 旧的根上名字由模块的 `__getattr__`（PEP 562）给出，第一次取用时发 `DeprecationWarning`；它们继续留在 `__all__` 里（`from rutis import *` 也不能变） | pytest 的警告汇总、`python -W default`；`-W error` 可以让测试在用到旧名字时失败 |
| Go | 旧名字保留为别名或转发，注释 `// Deprecated: …` | gopls（划线）、`staticcheck` 的 SA1019 |
| Swift | `@available(*, deprecated, renamed:)` | 编译警告；Xcode 的修改建议 |
| Kotlin | `@Deprecated(message, ReplaceWith(…))` 的 `typealias` 或转发函数，级别 WARNING | 编译警告；IDE 一键替换 |

**0.19 不升 `PLUGIN_API`**：`PLUGIN_API` 只在插件看到的接口不兼容时增加（[release.md](release.md)）。本稿只在 SDK 一侧加了模块，运行时建出的 `ctx`、协议都不变；升了它反而要求所有已发布的插件重新构建（#663 的 A8）。实施时在 release.md 的那条规则后补一句："`PLUGIN_API` 增加时，删除 0.19 起各 SDK 标为弃用的旧位置。"

## 六、公开面快照与 CI

和 Rust 的 `public-api.txt` 一样：公开面写成提交进仓库的文本，CI 重新生成并比较，加一个名字就要改它，评审时看得见。每种语言用最现成、不加依赖的做法，全部放进已有的 CI 任务（[ci.md](ci.md) §6）。

| 语言 | 锁住公开面的文本 | 怎样检查 | CI 任务 | 有意改动时 |
| --- | --- | --- | --- | --- |
| TypeScript / Bun | 手写的 `.d.ts` 本身（`src/index.d.ts`、`src/advanced.d.ts`、`src/testing.d.ts`） | `node/rutis/test/public-api.test.mjs`：`package.json` 的 `exports` 只有 `.`、`./advanced`、`./testing`；导入每个入口，运行时的导出名与对应 `.d.ts` 里 `export` 的名字相同（按行读，不用编译器） | `js-py`（`npm test`） | 改 `.d.ts` |
| Python | `python/rutis/public-api.txt` | `python/rutis/tests/test_public_api.py`：`rutis`、`rutis.advanced`、`rutis.testing` 的 `__all__`，其中每个类的公开成员与 `inspect.signature`；`dir()` 里的公开名字都在 `__all__` 里或是子模块 | `js-py` | `RUTIS_PUBLIC_API_WRITE=1` 跑这个测试 |
| Go | `go/rutis/public-api.txt` | `go/rutis/public_api_test.go`：标准库 `go/parser`、`go/doc` 列出 `rutis`、`rutis/advanced`、`rutis/rutistest` 的导出声明、方法与签名、Deprecated 标记（golden 文件） | `runtimes-go` | `go test -run TestPublicAPI -update` |
| Swift | `swift/rutis/public-api.txt` | `tools/public-api-swift.sh`：`swift package dump-symbol-graph --minimum-access-level public`，用 `jq`（macOS 15 起系统自带）把 `Rutis`、`RutisTest` 的符号写成排好序的一行一个：路径（含参数标签）和种类。不记声明原文，因为不同版本的编译器写法不同（Swift 6.3 写 `any Encoder`，6.4 写 `Encoder`） | `runtimes-swift` | `tools/public-api-swift.sh --write` |
| Kotlin | `kotlin/rutis/rutis/api/` | Kotlin 官方的 Binary Compatibility Validator，`apiCheck` 挂在 `check` 上（Kotlin Gradle 插件 2.2 自带的 ABI 校验还是实验性的，不用） | `runtimes-kotlin` | `./gradlew apiDump` |
| Rust | `crates/rutis/public-api.txt` | 不变（`tools/public-api.sh`） | `public-api` | 不变 |

TypeScript 的 `.d.ts` 是手写的，本身就是公开面，再生成一份快照是重复；检查只需保证运行时的导出和它一致。弃用的名字在各文本里单独放一节，删除时整节去掉。测试工具也锁住：#670 给它加名字时在同一个 PR 里更新。实施时在 ci.md §4 的各任务行里写上这一项。

## 七、开发者看到什么

**写一个普通插件**：只用根上的名字，`ctx` 照旧。

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

**用到进阶的名字**：从 advanced 导入，方法照旧在 `ctx` 上。

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

**旧代码不改**：照常运行，提示新位置。

| 语言 | 输入 | 看到 |
| --- | --- | --- |
| TypeScript | `import { datetime } from '@arcships/rutis'` | 编辑器划线，悬停：`'datetime' is deprecated. Moved in 0.19: import it from '@arcships/rutis/advanced'. Removed when the plugin API version changes.` |
| Python | `from rutis import AggregateError`，在 pytest 里跑 | `DeprecationWarning: AggregateError moved in 0.19: import it from rutis.advanced. The old name is removed when the plugin API version changes.` |
| Go | `rutis.DateOf(t)`，跑 `staticcheck` | `SA1019: rutis.DateOf is deprecated: moved in 0.19: use advanced.DateOf.` |
| Swift | `let d: Rutis.DateTime` | `warning: 'DateTime' is deprecated: renamed to 'Rutis.Advanced.DateTime'`，附修改建议 |
| Kotlin | `RutisFormat.encode(…)` | `w: 'RutisFormat' is deprecated. Moved in 0.19: use arcships.rutis.advanced.RutisFormat.`；IDE 一键替换并补上导入 |

**rutis 的贡献者加了一个公开名字，没有更新快照**：CI 失败，输出差异和改法，例如：

```
--- python/rutis/public-api.txt
+++ current
@@ rutis.advanced @@
+def retry(…)
the public API of rutis changed: review the diff, then run RUTIS_PUBLIC_API_WRITE=1 on tests/test_public_api.py
```

其他语言一样，最后一行写出本语言的更新办法（§六）。

## 八、兼容与迁移

写进《迁移到 0.19》（两份）中"SDK 的默认入口（#692）"一节：

> 0.19 起，各语言 SDK 的根上只留写插件常用的名字，其余放进 advanced：TypeScript / Bun `@arcships/rutis/advanced`，Python `rutis.advanced`，Go `github.com/arcships/rutis/go/rutis/advanced`，Swift `Rutis.Advanced`，Kotlin `arcships.rutis.advanced`。
>
> **不改也能跑**。旧的写法在 0.19 照常可用，编辑器、编译器或测试会提示新位置；到插件 API 版本增加时才删除，那一版的发布说明会再写一次。
>
> 要改的只有导入：根上移走的名字（值类型、`PLUGIN_API`、事件的描述类型、只在某种发出方式里出现的错误等）同名从 advanced 导入；完整列表见 [SDK 的默认入口](design-sdk-entry-convergence-2026-10-27.md) §三。
>
> 不变的：每种语言根上的 `definePlugin` / `define_plugin` / `Define` / `Rutis.define`，`ctx` 上的所有方法，测试工具，Rust。Swift 宏与 Kotlin KSP 生成的代码随 SDK 一起更新，不用改。插件 API 版本不变，用旧 SDK 构建的插件在 0.19 的运行时上照常运行。

**不做迁移工具**（E8）：Rust 的 `rutis-migrate` 是因为 0.17 改了 trait 的签名，手改容易错；这里只是换导入的位置，旧写法还能用，Swift、Kotlin 的 IDE 有一键替换，其余三种语言每处改一行。#663 的评估里普通插件只用到根上的名字，大多数插件一处都不用改。

## 九、实施

每种语言一个 PR，互不依赖；第 6 步在最后。

| 步 | 内容 | 完成条件 |
| --- | --- | --- |
| 1 | TypeScript / Bun：`node/rutis` 加 `./advanced` 入口；根上的旧导出在 `.d.ts` 里标 `@deprecated`；导出一致性测试 | 测试通过；仓库自己的示例、模板、测试插件改用新位置 |
| 2 | Python：`rutis.advanced`；根的 `__getattr__`；快照测试 | 快照测试通过；旧名字的导入、`from rutis import *` 各有一个测试，确认可用且发出 `DeprecationWarning`，`-W error` 下失败 |
| 3 | Go：`advanced` 子包（以别名指向内部包或根上的类型）；根上旧名字加 `// Deprecated:`；快照测试 | 快照测试通过；`staticcheck` 对旧写法报 SA1019；`rutistest` 与 `examples` 改用新位置 |
| 4 | Swift：`Rutis.Advanced`、`Rutis._Generated`；宏改为引用 `_Generated`；旧名字标 `deprecated`；快照脚本 | `RutisSelfTest` 通过；快照比对通过；旧写法只有警告没有错误 |
| 5 | Kotlin：`arcships.rutis.advanced`、`arcships.rutis.generated`；KSP 改为引用新包；旧名字 `@Deprecated(WARNING)`；接入 Binary Compatibility Validator | `gradle check`（含 `apiCheck`）通过；0.17.2 的 `rutis-ksp` 生成的代码在新 SDK 上能编译；Android 示例照常构建 |
| 6 | 文档与 CI：《迁移到 0.19》两份写入 §八 的一节；`plugin-api.md` 的速查表把移走的名字改到新位置；ci.md §4 写上各任务的检查；release.md 补 §五 的一句 | 链接检查通过；对照 #692 的「完成」清单：默认入口与子模块（1–5）、快照与 CI（1–5 与 ci.md）、迁移指南两份（6） |
