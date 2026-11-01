# 行的 `inject`，以及 `use` 先查声明

[English](design-declared-services-2026-10-29.en.md)

状态：已实现（分支 `claude/row-inject-declared`，未合并）。日期：2026-10-29；2026-10-30 在新基准上重建。基准：`claude/design-optional-deps`（可选依赖，2026-10-31 起在 main `d5aa3200`（0.19.3）之上）的 follow-up（[可选依赖](design-optional-deps-2026-10-28.md) §4.5、§六 记的两个缺口）。§一 描述的代码是本稿所在提交的父提交（可选依赖那个提交）里的。
Issue：尚无，随本稿开。相关：[#676](https://github.com/arcships/rutis/issues/676)（`not-declared` 的文字与原则）。

## 一、现状

同一段插件代码、同一个 `rutis.json`，在六种语言里给出两种结果。

插件没写 `inject: ['llm']`，行上写了 `inject: ["llm"]`，然后 `ctx.use('llm')`：

| 运行时 | 服务在本进程 | 服务缺 |
| --- | --- | --- |
| Node（leaf） | 拿到 | 这一行等在进程内的 Cordis 门控上，`apply` 不运行 |
| Bun、Python | 拿到 | `service llm is not available`（`unavailable`） |
| Go、Swift、Kotlin | `service llm is not available to this plugin, which does not declare it in inject: add 'llm' to its inject`（`not-declared`；Go 是 `… in Inject: add "llm" to its Inject`） | 同一句 |

不是协议没到。宿主早已把行 `inject` 放进 `rows.load` 的第五个参数（`crates/rutis-bridge/src/runtime/process.rs` 的 `load_exporting`：`[key, entry, config, isolate, inject, exports, { scope, id }]`），三家收下并用它，三家丢掉：

- Python：`row.row_inject`（`python/rutis/rutis/runner.py` 的 `Runtime.load`），并进声明集合 `row.uses` 与 `row.inject`。
- Node：`toCordis(spec, rowInject)` 的 `rowInject`（`node/rutis-runtime/src/leaf.mjs`），并进 `uses`；Bun：`Runtime.load` 里的 `uses = new Set([...plugin.inject, ...(inject ?? [])])`（`bun/rutis-bun/src/runner.ts`）。
- Go：`go/rutis/internal/runtime/runtime.go` 处理 `rows.load` 的分支里，注释写着"leaf 运行时的行在 rutis 里等"，然后不读 `args[4]`；Swift `Runtime.swift`、Kotlin `Control.kt` 的 `rows.load` 分支相同。

第二处是 `use` 的两步顺序。Go（`runtime.go` 的 `Ctx.Use`）、Swift（`Ctx.swift` 的 `Ctx.use`）、Kotlin（`Ctx.kt` 的 `Ctx.use`）先查声明，查不到直接报；Node、Bun、Python 先查进程里的服务表，查到了就给，声明检查只在查不到时决定说什么。可观察的后果是**邻居效应**：插件漏声明 `llm`，只要同一进程里恰好有别的行声明了它，Node/Bun/Python 就拿得到，而 Go/Swift/Kotlin 拿不到——结果取决于进程里还装着什么，与这一行自己的配置无关。`get` 六家都已经是先查声明，所以这是 `use` 独有的一处。

[可选依赖](design-optional-deps-2026-10-28.md) 把行 `inject` 的缺口记在 §4.5、§六，没有修。

## 二、决策

| # | 决策 | 为什么；放弃的做法 |
| --- | --- | --- |
| D1 | 行 `inject` 算已声明：六种语言一致，`use` 的声明集合 = 插件 `inject` ∪ 行 `inject`，`get` 的再加插件 `optional`（`use` 不认 `optional`，与 Rust 的 `require` 相同，见[可选依赖](design-optional-deps-2026-10-28.md) O4） | 行 `inject` 是宿主替这一行写的必需依赖；Rust 行把它并进 fiber 的声明（`combined_injects`），说"这个插件没声明它"与这一行的配置矛盾。本稿只改声明集合：`use` 不再报 `not-declared`，进程里已经有的（同一进程的行提供的，或别的行为自己租借进来的）直接拿到。进程里还没有的宿主或别的进程的服务仍拿不到：rutis 对运行时行的行 `inject` 既不门控也不为这一行租借（§七），本稿不改这一点。放弃"只给 Go/Swift/Kotlin 补上"：那正是现在的不一致。放弃"把行 `inject` 从声明集合里删掉，让 Node/Bun/Python 也拒绝"：会让宿主为一行配置的必需依赖在插件里读不到，`inject` 失去意义 |
| D2 | `use` 与 `get` 一样：先查声明，再查表。六种语言一致 | 与 `get`、与 [可选依赖](design-optional-deps-2026-10-28.md) §4.5 的原则一致（不声明的名字永远不会被租借，先说该声明什么，#676）。改动落在 Node、Bun、Python 各两行。放弃"反过来让 Go/Swift/Kotlin 先查表"：会把邻居效应推广到另外三家，而且依赖"进程里碰巧还有谁"的行为不可解释、不可测试 |
| D3 | 报错文字不变 | `use` 未声明仍是原来那句（`… does not declare it in inject: add 'llm' to its inject`；Go 的 `Inject` 与 `%q`，见[可选依赖](design-optional-deps-2026-10-28.md) O9）；`get` 未声明仍是可选那句。现有逐字测试不动 |
| D4 | 插件自己 `provide` 的名字，在提供着的时候算已声明：`use`、`get` 给它本身；撤回之后与没声明的名字一样 | 一个插件读自己提供的服务是正当的；D2 之前只有 Bun、Python 的 `use` 靠"先查表"顺带做到；Node 的 `use` 要到 `apply` 之后才读得到（`apply` 里 fiber 还在装载，Cordis 的严格读取不给出它自己提供的服务）；各家的 `get` 与 Go/Swift/Kotlin 的 `use` 都报 `not-declared`。六家都按"此刻是否在提供"判断，与 Rust 一致（`crates/rutis/src/ctx.rs` 的 `require_inner`：`own_service` 只在 `fiber.provided` 还记着这个键时为真，撤回时 `evict_and_finalize` 把它删去，之后严格读取报 `Undeclared`）：Bun、Python、Go、Swift、Kotlin 撤回时从行的 `provided` 里删去；Node 的适配层自己记下每次 `provide` 的条目，插件调用撤回函数时删去这一条（同一名字可能已被再次提供，旧的撤回不删新的）。放弃"读 `context[name]`"：Cordis 自己回答 `fiber`、`then`、`_u` 这类名字，撤回之后它抛 `cannot get property "<name>" without inject`。放弃"查 Cordis fiber 的 `store`"：撤回之后要过几个微任务 store 才删去这个名字，同一轮里撤回再提供时，迟到的删除会删掉新的条目 |
| D5 | 测试驱动不受声明集合约束：Node、Bun 的 `rutis-test`（`rutis-host test` 先装的那个插件）替测试调用任意服务 | 它是测试本身，不是被测插件，名字在测试运行中才给出（`t.service(name)`）；Python 的测试工具一直直接读运行时的服务表。标记用模块私有的 `Symbol`，不是 `Symbol.for`，也不在包的公开 `exports` 里；按文件路径深引用模块能拿到它，那是绕过公开接口。放弃"让驱动预先声明它用的名字"：写不出这份清单 |

## 三、规则

1. 声明集合，六种语言一致：`use` 的是插件的 `inject` ∪ 行 `inject` ∪ 插件自己 `provide` 的名字；`get` 的再加插件的 `optional`。测试工具（Node/Bun `@arcships/rutis/testing`、Python `testing`、Go `rutistest`、Swift/Kotlin `RutisTest`）的 `load` 没有行 `inject` 这一项，因此不含它；单独调用插件函数时不存在"行"，这一点不变。测试驱动不查声明集合（D5）。
2. `rows.load` 的 `args[4]`（行 `inject`）在 Go、Swift、Kotlin 收下，存到这一行上，并进声明集合。Cordis 行本来就靠它在进程内门控，不变；rutis 一侧不为它门控、也不租借（§七）。
3. `use` 与 `get` 都是：① 查声明，不在就报（`use` 用原句，`get` 用可选那句，码都是 `not-declared`，`data` `{service}`）；② 查表：本进程的对象、租借进来的代理、否则 `use` 报 `unavailable`、`get` 返回空值。
4. 行 `inject` 与插件 `optional` 同名的失败不变（[可选依赖](design-optional-deps-2026-10-28.md) §五 第 4b 条）。
5. 与 Rust 对照：D1 的声明集合、D2、D4 与 Rust 内核相同（行 `inject` 经 `combined_injects` 进 fiber 的 `declared_injects`；`require_inner` 先判 `Undeclared` 再看服务状态）。Rust 还按同一份 `combined_injects` 门控这一行，SDK 行缺的这一半见 §七。唯一不同是 `get` 一个未声明的名字：Rust 的 `ctx.get` 不查声明，因为它读的是整个宿主的服务表，没声明也找得到，未声明的读取另记进诊断；SDK 进程里只有声明过的名字会被租借进来，没声明时返回空值会把宿主上存在的服务说成不在，而 SDK 没有诊断通道，所以仍报 `not-declared`。
6. 卸载时的清理（`effect`）能不能读自己提供的服务，有三种情况。Rust 把提供的撤回与清理放在同一个栈里，逆序逐个运行（`crates/rutis/src/fiber.rs` 的 `drain_effects`；`crates/rutis/src/ctx.rs` 里 `provide_inner` 的注释）：在 `provide` 之后登记的清理先于撤回运行，读得到，之前登记的读不到。Node 的适配层只在插件调用撤回函数时删去自己记下的条目（D4），卸载时 Cordis 直接撤回（`@deepseek-ai/cordis` 的 `src/fiber.ts` 里 `Fiber._unload`，`Promise.all` 同时开始全部处置），不经这个函数：清理里读自己提供的服务，不论登记在 `provide` 之前还是之后、同步还是先 `await`，一律读得到。Bun、Python、Go、Swift、Kotlin 先撤回这一行提供的全部服务再跑清理，清理里读它一律报 `not-declared`。本稿不改卸载顺序（§七）。

## 四、开发者看到什么

### 插件作者

行上写了 `inject: ["llm"]`，进程里已经有 `llm`，插件自己不写也能用：

```ts
export default definePlugin({
  apply(ctx) {
    ctx.use<Llm>('llm').ask('hi')     // 行 inject 里的 llm：拿到
  },
})
```

Go、Swift、Kotlin 现在也拿到（D1）。提供方在宿主或另一个进程、也没有别的行为自己把它租借进这个进程时，今天拿不到（下表第 1 条，§七）。

出错时：

| # | 怎样引起 | 输出（逐字） | 码 | 怎样改 |
| --- | --- | --- | --- | --- |
| 1 | 行 `inject` 有 `llm`，插件没写，进程里没有 `llm`（哪里都没有；或它在宿主或另一个进程，rutis 不为运行时行租借行 `inject`，也没有别的行为自己租借它，§七） | `service llm is not available`（Node leaf 走不到这一步：Cordis 门控在进程里等 `llm`，`apply` 不运行） | `unavailable` | 宿主或另一个进程有它：把 `llm` 也写进插件的 `inject`，rutis 就为它门控并租借；哪里都没有：等提供方，或从行的 `inject` 里去掉 |
| 2 | 插件和行都没声明 `llm`，调 `use('llm')`，同一进程里**别的行**声明了它 | 改前 Node/Bun/Python：拿到；改后：`service llm is not available to this plugin, which does not declare it in inject: add 'llm' to its inject`（Go：`… in Inject: add "llm" to its Inject`） | `not-declared`，`data` `{service}` | 把 `llm` 写进插件的 `inject` |
| 3 | 同上，插件和行都没声明，进程里也没有 | 同一句 | `not-declared` | 同上 |
| 4 | 插件写了 `optional: ['llm']`，行 `inject` 也写了 `llm` | `<行 id>: unresolved: entry "<行 id>" rejected: llm is in the row's inject and the plugin's optional; keep it in one` | 行失败 | 删去一处 |

第 2 行是 D2 唯一的行为变化：依赖邻居声明的插件会开始报错。这是 #676 要的结果（没声明就说该声明什么），不是回归。

### 用 `rutis-host` 组装插件的人

没有新的配置，没有新的输出。同一份 `rutis.json` 在六种语言下的声明集合一致了。行 `inject` 指向进程里还没有的服务时，行的状态仍不一致：Node leaf 的行停在 Cordis 门控上、`apply` 不运行，其余五家的 `apply` 报 `unavailable`；这一处等 §七。

### 新增的概念

没有。

## 五、兼容

- D1、D4 只放宽，都落在原来报 `not-declared` 的地方：Go、Swift、Kotlin 的 `use`、`get` 对行 `inject` 里的名字，现在进程里有就拿到，否则 `use` 报 `unavailable`、`get` 返回空值；对插件自己正提供着的名字，六家的 `get` 与 Go、Swift、Kotlin 的 `use` 现在给它本身。原来成功的调用不会因此失败；按 `not-declared` 分支的代码会看到别的结果。
- D2 收紧：漏声明、靠邻居拿服务的插件会开始报 `not-declared`。这是修 bug，不进迁移指南；发布时 CHANGELOG 与发布说明写一句（照上游惯例，功能 PR 不改 CHANGELOG）。
- 协议不变：`rows.load` 的参数早就发了，`PROTOCOL` 不升。

## 六、实施

| 步 | 内容 | 完成条件 |
| --- | --- | --- |
| 1 Go、Swift、Kotlin | 读 `rows.load` 的 `args[4]`，存到行上，并进 `declares`；六家一致地，插件自己 `provide` 的名字也算已声明（改动前只有 Bun、Python 的 `use` 靠"先查表"顺带支持，Node 的 `use` 要到 `apply` 之后，其余不支持） | 三家各有两条用例：一条直接构造行，一条经 `rows.load` 的参数表（`args[4]`）送进来并再 `rows.update` 一次：行 `inject` 里、由同一进程提供的服务（用例在进程内提供它）`use`、`get` 都拿得到，重启之后仍在（Swift 的 `update` 原先丢了它）；宿主没有时报 `unavailable` 而不是 `not-declared`；插件读自己提供的服务不报 `not-declared`，撤回之后报（Kotlin 的用例不再把它写进 `optional`） |
| 2 Node、Bun、Python | `use` 改为先查声明 | 各自用例：漏声明、邻居声明了 → `not-declared` 逐字；行 `inject` 有 → 拿得到；Node、Bun、Python 自己提供后撤回 → `use`、`get` 立即报 `not-declared`；Node 同一轮里撤回再提供 → 读到新的那个，与没声明一样。Python `test_scopes` 里读两个名字的插件改为声明它们（D2 之后，未声明在查表之前就失败）；Node、Bun 的测试驱动按 D5 标记，两家 SDK 测试照旧通过 |
| 3 跨语言 | 六种语言各一条用例（§四第 1、2、3 条逐字），补上[可选依赖](design-optional-deps-2026-10-28.md) 实施步 2 欠的那组 | 六种语言输出一致。落地时改成了每语言各一条，而不是 cargo 里一组共用用例：Node leaf 的运行时行会把行 `inject` 交给进程内的 Cordis 门控，而 rutis 只把插件自己声明的名字租借进进程，所以「行 `inject` 指向另一个运行时的服务」这一组合今天拿不到（§七），写不成跨语言用例 |
| 4 文档 | `plugin-api` 的 `ctx.use` 一行写明声明集合含行 `inject`；各语言指南一句 | 链接检查、文档示例检查通过 |

另：#683 的 `allowed_services` 按声明集合判断，随本稿一起对。

## 七、后续

运行时行的行 `inject`，rutis 一侧还没接上：`crates/rutis-loader/src/loader/desired.rs` 的 `foreign_scope` 给运行时行的 `BoundScope` 里 `inject` 是空的，`crates/rutis-loader/src/runtime.rs` 的 `JsRow::start` 只租借插件自己声明的 `gated`。所以行 `inject` 里由宿主或另一个进程提供的服务，rutis 不等它，也不为这一行把它租借进进程（别的行为自己租借进来的，这一行也读得到）。Rust 行没有这个缺口：`crates/rutis-loader/src/loader/reconcile.rs` 的 `combined_injects` 把行 `inject` 并进 fiber 的 injects，fiber 等齐才启动。下一个 PR 让运行时行与之一致：rutis 为行 `inject` 门控、启动时租借。之后 §四 第 1 条只剩"哪里都没有"一种情况，`loader/rows-gate-on-services` 对运行时行也成立。

另一处后续是卸载顺序（§三 第 6 条）：六家都改成 Rust 那样逆序逐个运行撤回与清理。它改的是各运行时的卸载，与声明集合无关，单独做。
