# 测试工具：`load` 装接受测插件交出的插件定义

[English](design-test-tool-definition-load-2026-11-01.en.md)

状态：草稿，未实现，含 **breaking** 改动（`rutis_loader::patch::Owner` 加变体，§五）。日期：2026-11-01（同日两轮修订。第一轮：来源这一代的保存与恢复、副本身份贯穿服务与错误查询、只在运行期存在的行跨 `reconcile` 的规则、按作用域定位来源，补齐「开发者看到什么」。第二轮：来源改成"一次解码、按运行时给的那一代绑定"，去掉重启窗口与 R3/R6 的冲突（D4、D6、D7）；副本身份怎么进到构建路径（D8）；`reconcile` 与活副本撞 id 的规则（D9）；实例验收改为同一个行在两个实例并补逐字位置错误（§六）；`Owner::Copy` 标 breaking 并配《迁移到 0.20》（§五）。）

Issue：[#1004](https://github.com/arcships/rutis/issues/1004)（TypeScript 测试工具：测试进程收不到插件定义）。按本稿实施的是它的 PR1、PR2（§六）。上游规则见[插件是一个值](design-plugin-values-2026-10-25.md)的 R10 与 §五.5；本稿**收窄** R5 里"副本按来源行命名"一句的适用范围（D8）。

## 一、现在怎样

TypeScript 和 Bun 的测试工具让被测插件在 `rutis-host test` 起的**另一个进程**里，在真实运行时中运行；测试进程是对端（`node/rutis-runtime/src/testing/start.mjs`）。插件把一份插件定义交给测试提供的服务时（Dim 的 `partRegistry.registerDefinition('session', definition)`），这个参数过不了线上边界：

```
registrar: TypeError: parts.register: argument 1 cannot be sent: cannot send a plugin definition: the far end does not accept plugin definitions (it did not declare the capability plugins)
```

测试工具那一侧的会话没有声明 `plugins`。只补声明不够：测试进程的会话收到 `plugin` 引用时一律拒绝（`node/rutis-runtime/src/session.mjs:451`，Bun 的 `bun/rutis-bun/src/session.ts:443` 相同），错误只是换成

```
received a plugin definition made in another process, which cannot be used here; pass plain data or a function instead
```

而 `load` 只收本进程里 `definePlugin` 造的值（`node/rutis/src/testing.mjs:222`），替身手里那个值也装不上。

Python、Go、Swift、Kotlin 的测试工具没有这个问题：它们的运行时就在测试进程里（`rutis-host test --channel`），替身收到的是同一份定义本身。Node/Bun 的运行时是另一个进程，替身能拿到的只能是"定义在那边的那个值"，要装上必须交回定义它的运行时。

R10 已经写明这件事，缺的是 Node/Bun 的做法：替身收到的定义可以直接交给测试工具的 `load`，和真实宿主一致。

### 一之一、为什么来源会绑错

测试提供的服务与插件之间的调用不经过宿主：测试的服务是运行时里的一行（`node/rutis-runtime/src/testing/service.mjs`），它的方法经驱动的控制会话直接到测试进程。所以定义被交出的那一刻，宿主看不到这一次调用。`#[rutis::service]` 的 `Definition` 参数在**第一次解码**时读来源（`crates/rutis-bridge/src/marked.rs:642–655` 的 `caller_row()`），而第一次解码是驱动调宿主的时候，读到的必然是**驱动那一行**；`crates/rutis-bridge/src/runtime/definitions.rs:128–147` 于是按驱动这一代建定义：副本随测试结束，而不是随交出它的插件那一行结束。

反过来说：只要第一次解码发生在驱动那一行，之后无论怎么做都要"改绑"，而同一个值改绑成另一代就同时违反 R3（同一个定义交出几次都是同一个，按值缓存）和 R6（属于第一次交出它的那一行）。所以 D4、D6 的做法是**从源头不让它绑错**：拿到定义的那一次调用不解码它，方法体里带着运行时给的那一代解码一次，之后按值缓存。

## 二、决策

| # | 决定 |
| --- | --- |
| D1 | **测试侧的会话收下定义。** 会话新增一个选项 `host`（只有测试工具用，`node/rutis-runtime/src/testing/start.mjs` 与 Bun 的同一份）：声明 `plugins`，并把对端发来的 `plugin` 引用留成一个能原样交回的值（D2）。放弃：所有会话默认收下——运行时收到别的运行时的定义本来就用不了，维持现在的拒绝，错误才说得准。 |
| D2 | **替身收到的是一个不透明的值。** 它既不是 `definePlugin` 的结果，也不是能调用的代理：只有标记 `Symbol.for('rutis.plugin-definition')`，不能读字段、不能调用；原样交给任何服务时按 `home: true` 发回原来的引用，插件拿回的是它自己那个定义。放弃：给它加 `apply`，让测试进程也能挂——副本要在定义它的运行时里跑（R5），本地挂是假的。 |
| D3 | **`load` 收这个值。** `@arcships/rutis/testing` 的 `load()` 与 `host().load()` 除了 `definePlugin` 的结果，也收 D2 的值：跳过"模块默认导出"和插件 API 的检查，把定义本身交给驱动的新方法 `loadDefinition(definition, config, options, owner)`。放弃：另加一个 `loadDefinition()` 函数——R10 要求直接交给 `load`，同一件事不该有两种写法。 |
| D4 | **宿主入口：一个不解码插件引用的参数，加运行时给的那一代。** `rutis-host/test` 增加<br>`pub async fn load_definition(&self, plugin: PluginRef, config: Option<Value>, options: Option<Options>, owner: Option<String>) -> Result<Loaded, String>`<br>· `PluginRef` 是 `crates/rutis-bridge` 的新参数种类：只拿着会话的引用，**不读声明、不建定义**（宏里与 `Definition` 一样按类型名认，`crates/rutis-macros/src/lib.rs:514`、`:946` 那一带加一支）。<br>· `owner` 是运行时给出的**这一代**：运行时里那一行的键（`rows.load` 的第一个参数，`{instance}.{generation}`），由宿主自己在 `crates/rutis-loader/src/runtime.rs` 里生成，所以宿主认得它。<br>· 方法体里调用一次 `rutis_bridge::runtime::read_definition(process, reference, owner)`（见 D6），得到的 `Definition` 与真实服务调用拿到的完全一样。<br>· `Loaded { id, provides }`：行 id，以及定义声明的服务与方法种类（`plugins.read` 已经读过）。`Options` 仍是 `{ id, isolate }`。<br>放弃：参数就用 `Definition`——它在驱动那一行解码，方法体只能改绑（D6 里说明为什么违反 R3/R6）。放弃：把 `load` 的 `name: String` 改成 `Value` 再分支——"要名字"的参数变成什么都收，`rutis-host types` 里两个入口看不出区别，传错时错误也不再指向写错的地方。放弃：返回值只给行 id，让测试工具再问一次声明——`service()` 拿不到方法表就调不了同步方法（§三 第 3 条）。 |
| D5 | **rutis-loader 的入口是新方法，不扩 `NewEntry`。**<br>`pub async fn load_copy(&self, definition: &Definition, config: Value, options: CopyOptions, owner: &str) -> Result<(String, FiberView), LoaderError>`<br>理由：`NewEntry` 是**可编辑层**的输入，会被写成一行 JSON（`id`/`name`/`config`/`isolate`），`name` 由 resolver 解析成模块或运行时；插件定义是一个值，写不进层、也解析不了，`reconcile` 无法从层里重建它。所以副本是**只在运行期存在**的行（D9）。放弃：`NewEntry { entry: Module \| Definition }`——层里会留下一个永远重建不出来的行。放弃：让 `Harness` 自己 `ctx.plugin(def.at(..))` 挂在宿主根上——副本不出现在 `status` 和 `entries()`，卸载也没有统一入口。 |
| D6 | **一次解码，按运行时给的那一代绑定；协议不变。** 宿主经 `rutis_bridge::runtime` 里新的一支读定义：它与 `read()` 共用同一段（校验、`plugins.read`、`from_source`、按值缓存），只有来源不同——有 `owner` 时按它取这一代，没有时照旧取 `caller_row()`。取这一代经一个**钩子**：`rutis_bridge::runtime` 新增 `trait OriginResolver { fn origin(&self, key: &str) -> Option<Origin> }`（`Origin { row, ctx }`），由 rutis-loader 在根上提供（和 `PluginMounter` 同一处，`crates/rutis-loader/src/loader/plugins.rs:206`），实现是查 loader 的运行记录：`(key → Running)`（D9 里 `Running` 记下这个键），命中就交回那一代的 `Ctx`（D8 的 `fiber_ctx`），未命中就是这一代已经没了。<br>· **没有窗口**：键是**这一代**的，不是"某个行的当前代"，重启产生新键；查表与取 `Ctx` 在 loader 的状态锁里一次完成，取到的 `Ctx` 是那一代的。查不到就拒绝（§四 第 1 条）；查到之后那一代随即结束也无妨：定义注册在那一代的 fiber 上，副本先结束（R6）。<br>· **R3 与 R6 同时成立**：一个值只解码一次（`known_definition` 按值缓存，`crates/rutis-bridge/src/runtime/process.rs:1150` 那一带），第一次解码时来源就是交出它的那一代；第二次 `load` 同一个值时 `owner` 被忽略，拿到的是缓存里同一个 `Definition`，`ended()` 也仍然是同一个。<br>· 运行时的 `owner` 从哪来：行跟踪（D11）打开时，驱动器会话的每个导出都记着当前行（`session.mjs` 的 `owner()` 已经为宿主会话这么做），驱动经 `ownerOf(value)` 取回这个键（`node/rutis-runtime/src/testing/rutis-test.mjs`），作为 `loadDefinition` 的参数交给宿主。<br>放弃：带 `Definition` 参数再改绑缓存（第一轮的写法）——重启窗口里旧值能绑到新代，且改绑与 R3/R6 冲突。放弃：按 `(行 id, 作用域链)` 找"当前这一代"——从交出到绑定之间那一行重启，旧值会绑到新代；同一个值的第二次 `load` 也可能绑到更新的代。放弃：引用里带这一代的不透明标记（协议里给 `plugin` 引用加字段）——做法可行（**选项 A**，代价与验收见 §六），但它要改 plugin values §五.1 的线上格式，每个持有插件引用的对端都要原样带回；本稿选不改协议（**选项 B**，即本条）。 |
| D7 | **位置由解析出的这一代决定，R4 不放宽。** 副本挂在 `OriginResolver` 交回的那个 `Ctx` 的组上下文里（那一代所在的位置）；内核 R4 的检查照旧（`load_copy` 调 `at` 的同一段代码与文字，`rutis::internal::definitions` 里那一项）。定义的作用域不在这个位置的前缀里时按内核的话拒绝（§四 第 4 条）。放弃：把 R4 放宽成"任意位置都能挂"——来源在实例里时，副本会看到别的实例的服务，和插件里 `ctx.plugin(def.at(..))` 不一致。放弃：用一个"根的上下文"兜底——同样破坏实例隔离。 |
| D8 | **副本以自己的行 id 出现在服务、错误与 `status` 里，并从构建路径一路带着它。** 定义行是行：`rutis/service` 的 `row`、监听器描述、`CallerRow`、错误行与 `status` 的 `services:` 都用它的行 id（§五.5）。R5 那句"副本的 `row` 是来源行的 id"仍适用于**插件用 `ctx.plugin(def.at(..))` 挂出来的**那一份（PR2 在 plugin values 设计稿的 R5 与 §五.5 各加一句，中英两份）。名字怎么进到构建路径：<br>· `crates/rutis-loader/src/runtime.rs:893–960` 的 `DefinitionMounter::mount` 里算键、算作用域、建 `JsFactory` 的那一段抽成 `fn copy_factory(loader, at, plugin: &ForeignPlugin, config, name: Option<&str>) -> Result<Box<dyn DynPlugin>, Error>`；<br>· `DefinitionMounter::mount`（`crates/rutis-loader/src/loader/plugins.rs:206` 提供的那一个）改成 `copy_factory(…, name = plugin.row().or(Some(plugin.name())))`——**`ctx.plugin(def.at(..))` 的行为与命名一字不变**（R5）；<br>· `Loader::load_copy` 用 `copy_factory(…, name = Some(副本的行 id))`，行 id 由 `CopyOptions.id` 给，否则按定义的名字生成（`free_id`，命名空间含 `desired` 与 `copies`）；随后 `rutis::internal::definitions::mounted(&definition, &view)` 把它记成这个定义的一份（内核 R6），因为这一份不是经 `Definition::at` 挂的。<br>放弃：按 R5 用来源行 id——两份副本共用一个名字（`provided()` 分不开、服务撞名的错误指错行），`status` 的 `services:` 会显示成来源行提供的。放弃：`provided()` 映射到来源行——同一份定义的两份副本混在一起。放弃：让 `DefinitionMounter` 认一个"当前要用的名字"——那是跨线程的全局状态，比多一个参数更糟。 |
| D9 | **副本只在运行期存在；它的 id 与层里的行共用一个命名空间。** 副本记在 loader 的 `State.copies`（`id → { fiber, definition, owner, config }`），不在 `desired`、不在 `running`（`crates/rutis-loader/src/loader/reconcile.rs:821–854` 只保留 `desired` 的记录，后面还有按 `desired` 查找的 `unwrap`）；`Running` 记下它这一代的键（D6）。规则：<br>· 普通行的 `create`/`update`/`remove`/`move`/`rename` 与任何一次 `reconcile` 都不动副本：不卸载、不重启、不发 `rows.update`。<br>· **`Loader::reconcile(layers)` 撞上活副本的 id**：层里那一行失败、副本留着。`ReconcileReport` 里多一条 `Failure { row: <id>, error: "a row <id> is loaded already: a copy of a plugin definition has that id" }`，其余行照常起；副本结束之后再 `reconcile`，那一行就起得来。放弃：让副本让位（删掉它）——副本是测试正在用的那一份，`reconcile` 不该动它；而且校验发生在一瞬间，删掉之后层里那一行要的是同名新行，和副本没有任何关系。<br>· `create` 用副本的 id、`load_copy` 用层里行或另一份副本的 id：都拒绝，文字同 `create` 今天的那一句（`a row <id> is loaded already`）。<br>· `entries()` 与 `status` 列出副本：`EntryInfo.owner` 是新的 `Owner::Copy`，`options` 是 `{ id, name }`，状态取那份 fiber。<br>· 清扫：fiber 进入 `Disposed` 的副本在下一次 `reconcile` 或 `status` 时从 `copies` 里去掉；`Failed` 的留着，和行一样能在 `status` 里看到，直到 `remove`。<br>放弃：把副本放进 `running` 并用空的 `desired` 行兜住——`reconcile` 的 `unwrap`、`Slot`、实例与配置求值都要跟着改，等于给副本一套假的层。放弃：副本不记在 loader 里——`status`、`entries()`、`remove` 就没有统一入口。 |
| D10 | **配置在挂上时定下**（§五.5）。`update` 对定义行拒绝，`set_disabled`、`reload`、`move`、`rename` 也拒绝（副本不在层里，见 D9）；报告见 §四。放弃：把 `update` 转给运行时——定义行在运行时里没有 `rows.update`（§五.5），转过去只会得到更远的错误；R5 说的做法是卸掉再按新配置挂。 |
| D11 | **代价：行跟踪只在测试里打开。** D6 要让运行时知道"现在跑的是哪一行"，用的是 `rowTracking`（今天只在 `RUTIS_DEV` 或 `--prefix-output` 时打开，`node/rutis-runtime/src/runner.mjs:106`）。判据是测试驱动在装载（`RUTIS_TEST_CONTROL` 在环境里），普通运行不付这个开销。PR2 记一次实测开销（每个插件调用一次 `AsyncLocalStorage`，外加 promise 初始化钩子）。放弃：常开——0.20 有 #980 在收 CPU，普通运行不该为测试工具付钱。 |
| D12 | **不做的事：** `rutis.json` 里写插件定义、`rutis/loader.create` 收定义（不是一个能写进层、能重建的名字）；同一份定义 `load` 两次共享一行（每次一份副本，和 `ctx.plugin(def.at(..))` 一样）；`rutis-host check` 认识运行期才交出的定义（R8 不变）；`rutis-host types` 为 `load_definition` 生成客户端声明（`rutis-host/test` 是宿主自己的服务，测试工具用 JS 调它）；Python/Go/Swift/Kotlin 的测试工具（它们的替身早就能收到定义本身，行为不变）。 |

## 三、规则

| 规则 | 行为 |
| --- | --- |
| 入口 | `load(definition, { config, services, id, isolate })`（SDK）→ 驱动 `loadDefinition(definition, config, options, owner)` → `rutis-host/test` 的 `load_definition(plugin: PluginRef, …, owner)` → `read_definition` 一次解码 → `Loader::load_copy`。`config`、`id` 一样传到底；`services` 仍由测试工具作为运行时里的行先提供（和今天一样）。 |
| 1. 来源 | 运行时记下交出它的那一代（`owner` 是运行时那一行的键）；宿主经 `OriginResolver` 取到那一代的 `Ctx`，取不到就拒绝；这一代结束、重启时先结束副本（R6），旧值装不上。一个值只解码一次，第二次 `load` 拿到同一个 `Definition`（R3）。 |
| 2. 返回 | `Loaded { id, provides }`：行 id 与定义声明的服务及方法种类。测试工具的行对象与 `load` 的行相同。 |
| 3. `service()` | 测试工具用 `Loaded.provides` 建方法表（同步方法仍走同步调用），不读 `plugin.provides`（不透明值没有这个字段）。`methods(name)` 同源。 |
| 4. 等待 | 和 `load` 的行一样：声明的 `inject` 里的服务没到时等它，到了再启动，不失败。 |
| 行 id | `options.id`，否则定义的名字，被占用时 `-2`、`-3`……（`free_id`，命名空间含 `desired` 与 `copies`）。 |
| 名字（服务、错误） | 副本自己的行 id（D8）：`rutis/service` 的 `row`、错误行、`status` 的 `services:`、`provided()`、`reported()`。 |
| `status` / `entries()` | 与行一起列出：`id` 是行 id，`name` 是定义的名字（没有名字时是来源行 id），`owner` 是 `Owner::Copy`，状态取那份 fiber（D9）。 |
| 卸载 | `unload` / `Loader::remove` 只结束这一份，和行一样：先撤它提供的服务，再 `rows.unload`，最后释放租约。 |
| 来源结束 | 来源行卸载、重启或结束：先结束副本并等它们清理完，再结束来源行；之后 `ended()` 完成，副本的记录在下一次清扫时去掉。 |
| 运行时结束 | 定义它的运行时进程没了：副本随之结束（连接断了），记录在下一次清扫时去掉。 |
| 层里的编辑 | 与副本无关的编辑、以及任何一次 `reconcile` 都不动副本，也不给它发 `rows.update`；层里出现与活副本同 id 的行时那一行失败（D9）。 |
| 已结束的定义 | 这一代已经结束（或键已不存在）：`read_definition` 拒绝（§四 第 1 条）；这一代在解码与挂载之间结束的，`load_copy` 用 `at` 的同一句话拒绝（§四 第 3 条）。 |

跨语言：这些规则只加在 Node 和 Bun 的测试工具与它们的宿主入口上；其他语言的测试工具不受影响（D12）。

## 四、开发者看到什么

三个角色受影响：写测试的插件作者；用 `rutis-host` 组装插件的人；宿主持有者（嵌入 rutis 的应用）。部署的人看不到变化。

### 4.1 写测试的插件作者（TypeScript；Bun 只有运行时包名不同）

**正常时。** 被测插件把一份定义交给测试的替身，替身把它交给 `load`：

```ts
import { load } from '@arcships/rutis/testing'

const parts = []
const owner = await load(ownerPlugin, {
  services: {
    partRegistry: {
      async registerDefinition(scope, name, definition) { parts.push(definition) },
    },
  },
})
const session = await load(parts[0], { config: { depth: 2 }, services: { history: fakeHistory } })
await session.service('gitStatus').refresh()
await session.unload()
await owner.unload()
```

测试通过时没有输出。`parts[0]` 就是插件 `apply` 里造的那个定义，装在定义它的运行时里，和 `rutis-host` 装它时一样：配置按定义的 schema 校验并补上默认值，`inject` 的服务都在了才启动，随交出它的那一行结束（那份副本先结束，那一行才结束）。

**出错时。** 每条一行：怎样触发、逐字输出、在哪条语句上抛、怎样改。

| 触发 | 输出（逐字） | 位置 | 怎样改 |
| --- | --- | --- | --- |
| 交出定义的那一代已经结束（卸载或重启），再 `load(definition)` | `PluginTestError: the plugin definition was handed over by a row that is no longer running: load the definition that row hands over now` | `load` 抛出的错误 | 用这一行现在交出的那份定义，或先把它 `load` 起来再卸那一行 |
| 交出定义的行不是这个宿主上的行（应用自己装的插件交出的定义） | `PluginTestError: part: the row index that handed the plugin definition over is not a row of this host: only a plugin loaded as a row of the host can hand one over` | `load` 抛出的错误 | 改用宿主上的一行交出定义；应用里用 `ctx.plugin(def.at(..))` 挂 |
| 这一代在解码与挂载之间结束（少见） | `PluginTestError: part: plugin definition part has ended with its origin` | `load` 抛出的错误 | 同第一条 |
| 定义的作用域不在解析出的那一代所在位置的前缀里（Rust 侧可直接碰到，测试工具不会） | `LoaderError: part: plugin definition part can only be mounted inside instance g 1 or deeper, not inside instance g 2` | `Loader::load_copy` 返回的错误 | 在交出定义的实例里挂；应用里用 `ctx.plugin(def.at(..))` |
| 对定义行 `update(config)` | `PluginTestError: part: a copy of a plugin definition has the config it was loaded with: unload it and load it again` | `update` 抛出的错误 | 卸掉这一行，用新配置重新 `load` |
| 对定义行 `move`/`rename`/`set_disabled`/`reload`（`rutis/loader` 的方法） | `part: a copy of a plugin definition is not a row of the configuration: unload it and load it again` | 该调用抛出的错误 | 同上 |
| 对定义行给 `isolate` | `PluginTestError: part: a plugin definition runs where the row that handed it over runs: load it without isolate` | `load` 抛出的错误 | 去掉 `isolate` |
| 配置不合定义声明的 schema | `PluginTestError: part: config: depth should be a number, got "deep"` | `load` 抛出的错误 | 按定义声明的 schema 改配置 |
| `RUTIS_HOST` 指向比测试工具旧的 `rutis-host`（0.19 及更早，没有 `load_definition`） | `PluginTestError: the test driver has no method loadDefinition` | `load` 抛出的错误 | 换成同版本的 `@arcships/rutis-host`，或把 `RUTIS_HOST` 指向新版的二进制 |

输出里出现的只有开发者写过的东西：行 id、定义的名字、配置的路径、实例链（`instance <组> <实例号>`，见 `crates/rutis/src/definition.rs:418`）。实现的 PR 用验收测试逐字比较这张表。

**要懂的概念。** 没有新增：R10 已经要求"替身收到的定义可以直接交给 `load`"。测试作者仍只需知道"定义在定义它的运行时里作为一行运行，随交出它的那一行结束"。

### 4.2 用 `rutis-host` 组装插件的人

`rutis.json` 没有新字段；`rutis-host check` 的输出不变（R8）。`rutis-host status` 多一种行——定义行，在 `rows:` 里和其他行一样，它的服务在 `services:` 里以它的行 id 为提供方（D8）：

```text
rows:
  index: running
  part: running
services:
  echo: part
  scheduleJobs: index
runtimes:
  none
calls:
  none
```

来源行卸载后 `part` 随之消失（R6）；跑过又失败的 `part` 留着，和其它行一样显示 `failed: …`。同一个行在两个实例里各交出一份定义时，`rows:` 里是两行（`part` 与 `part-2`），各自在 `rows` 的实例里（`part in <实例>`）。

### 4.3 宿主持有者（嵌入 rutis 的应用，Rust）

```rust
// 把一份别的语言交来的定义装成一行；owner 是交出它的那一代（D6、D7）。
let (id, _view) = loader
    .load_copy(&definition, json!({ "depth": 2 }), CopyOptions::default(), owner)
    .await?;
// 卸载：和其它行同一个入口。
loader.remove(&id).await?;
```

`entries()` 里这一行的 `owner` 是 `Owner::Copy`（D9），`options` 是 `{ id, name }`；`status()` 的 `rows` 里有 `{ id, state }`。错误：`Err(LoaderError)`/`Err(String)` 里的文字就是 §4.1 那些行去掉 `PluginTestError: ` 前缀；`rutis-host/test` 与 `Loader::load_copy` 都不新增错误码。**不兼容**：`rutis_loader::patch::Owner` 多一个变体（§五）。

### 4.4 部署的人

看不到变化：协议没有新帧，`rutis.json` 没有新字段，`rutis-host run`/`dev`/`check` 的输出不变。

## 五、兼容与迁移

- **协议：** 没有新帧，线上格式不变（本稿选的是不改协议的那一条；给引用加来源标记的做法见 §六 的**选项 A**）。测试侧的会话多声明一个能力 `plugins`（R10 §五.3 已经定义的能力，测试工具这一侧之前没声明）；不声明的那一端行为不变。
- **宿主接口：** `rutis-host/test` 多一个方法 `load_definition`（参数是 `PluginRef` 与 `owner`）。0.19 及更早的宿主没有它，新 `load` 遇到定义时报 §4.1 的最后一条；版本要一致这件事今天就在（`@arcships/rutis-host` 是测试工具的 dev dependency，`RUTIS_HOST` 由使用者给）。
- **运行时包：** 定义由运行时包（`@arcships/rutis-runtime` / `@arcships/rutis-bun`）交出来，来源也由它记下；旧运行时既不记来源也不认 `loadDefinition`，报同一条。
- **公开 API：**
  - **breaking**：`rutis_loader::patch::Owner` 是公开、可穷尽匹配的枚举，多一个变体 `Copy`（`crates/rutis-loader/src/patch.rs:62`）。穷尽匹配它的代码要加一条 `Owner::Copy`（一份副本，不在任何层里）。0.20 的《迁移到 0.20》写这一条（中英两份，`docs/migration-0.20.md` / `.en.md`；文件不存在就在 PR2 里新建），#1004 带 `breaking` 标签并在 issue 里写明迁移要求（#694 规则 2）。
  - 新增（不 breaking）：`rutis_loader::Loader::load_copy`、`CopyOptions`；`rutis_bridge::runtime` 的 `read_definition`、`OriginResolver`、`ForeignPlugin` 的取用，以及 `PluginRef` 参数种类；`rutis::internal` 里几项（内部，无稳定性承诺）。
  - `@arcships/rutis/testing` 的 `load` 多收一种值，TS 声明不变（`Definition` 早已写成 TS 的插件类型，R9）。
- **迁移：** 只有 `Owner` 那一条，内容见上；其余没有写法变了。PR1 与 PR2 的正文按这个写：PR1 无兼容变化；PR2 有一处 Rust 侧的 breaking。

## 六、实施

**PR1：测试侧声明并收下定义（D1、D2）。**
`node/rutis-runtime`（`src/testing/start.mjs`、`src/session.mjs`）与 `bun/rutis-bun` 的同一份。
完成条件：测试给的服务收到 `register(定义)`，替身把它原样交回时插件拿到的是它自己的那个定义；`node/rutis/test` 与 `bun/rutis-bun/test` 各一个用例；`load` 的行为不变。
**PR1 不关闭 #1004**（Dim 的写法仍然跑不起来），PR2 才关（合并时写 `Closes #1004`）。

**PR2：按 D3–D11 让 `load` 装上定义。**
`crates/rutis`（`internal` 里几项：`definitions` 的作用域与 `at` 的那段检查、`FiberView` 取它的 `Ctx`、`definitions::mounted`）、`crates/rutis-bridge`（`PluginRef` 参数种类、`read_definition`、`OriginResolver`、`ForeignPlugin`/`foreign()`）、`crates/rutis-loader`（`Loader::load_copy`、`CopyOptions`、`copy_factory`、`State.copies`、`Running` 记下这一代的键、`OriginResolver` 的实现、`entries`/`get`/`remove`/`status`/`reconcile` 认副本、`Owner::Copy`）、`crates/rutis-host`（`load_definition`）、`node/rutis-runtime`（驱动的 `loadDefinition`、`ownerOf`、行跟踪的开关；`leaf.mjs` 的 `within`）、`node/rutis`（`load` 收定义、`service()` 用 `Loaded.provides`）、Bun 的同一份，以及 `docs/migration-0.20.md`/`.en.md`。

完成条件（逐条对评审的五点）：

1. **来源**：`owner` 是这一代的键；宿主查不到就拒绝；查到就绑在那一代的 `Ctx` 上。用例：同一份定义在来源行卸载后 `load` 报 §4.1 第 1 条；来源行重启后，新一代的定义装得上、旧值报同一条；同一个值 `load` 两次拿到同一个 `Definition`（`ended()` 也是同一个，R3）；来源行卸载时先结束副本、等清理跑完才结束来源行（R6）。
2. **身份**：定义行提供 `echo` 时 `status` 的 `services:` 显示 `echo: part`、`provided()` 列出 `echo`、`reported()` 拿得到该行的失败；同一份定义的两份副本各有各的 id，`provided()` 分得开；普通 `ctx.plugin(def.at(..))` 挂出的副本仍按 R5 用来源行命名（用 `rutis-loader/tests/plugin_values.rs` 现有用例守着，不改它们）。
3. **跨 `reconcile`**：无关的 `create`/`update`/`remove` 与一次 `reconcile` 之后副本的 `PluginId` 不变、运行时没收到这一行的 `rows.update`；层里出现与活副本同 id 的行时 `ReconcileReport` 里有一条 `Failure`（文字见 D9）、副本照旧、其余行起得来，副本结束后再 `reconcile` 那一行起得来；`create` 用副本的 id 被拒；`remove(副本 id)` 结束它。
4. **实例**：实例化的组 `g` 有两个实例，**同一个行 id** `index` 在实例 1 与实例 2 里各交出一份定义，得到 `part` 与 `part-2`，各自挂在自己的实例里（同名服务按实例隔离，两份各自的 `provided()`）；`Loader::load_copy` 收到与定义作用域无关的 `owner` 时逐字报 `part: plugin definition part can only be mounted inside instance g 1 or deeper, not inside instance g 2`（R4 与 `ctx.plugin(def.at(..))` 同一段代码）。
5. **迁移与文档**：`Owner` 标 breaking，`docs/migration-0.20.md`/`.en.md` 写一句（PR2 新建文件），PR2 在 #1004 上留言写明迁移要求、给 issue 加 `breaking` 标签；`docs/guide/plugin-api.md`/`.en.md` 的"替身收到的定义可以直接装上"加上 Node 与 Bun；plugin values 设计稿的 R5 与 §五.5 各加一句（D8）；`docs/guarantees.md`/`.en.md` 的 `loader/plugin-value-copies` 补"定义作为行"一段；发布说明各一条。

**D6 的两条路（评审若要换，另一条整段替换 D4、D6 的读法）**

| | 选项 A：引用里带这一代 | 选项 B（本稿）：参数里带这一代 |
| --- | --- | --- |
| 做法 | `plugin` 引用的线上格式加一个字段（来源这一代），交出时由运行时附上；宿主在**唯一的第一次解码**里按它绑定；持有引用的对端原样带回 | `load_definition` 的参数不解码定义，方法体里按 `owner` 解码一次（D4、D6） |
| 代价 | 改 plugin values §五.1 的线上格式：六个 SDK 的引用解析都要接受这个字段，Node/Bun 的会话在交回时原样带回；协议一旦发出就有兼容包袱 | 新增一个参数种类 `PluginRef`（bridge + 宏），`rutis-host types` 不生成它（D12）；协议不动 |
| 验收 | 同本节第 1 条；另加：一个带来源标记的引用经测试进程交回后，宿主绑到那一代 | 本节第 1 条 |
| 放弃的做法 | 不带标记而靠 `caller_row()`——就是 §一之一 的绑错 | 带 `Definition` 参数再改绑——重启窗口与 R3/R6 冲突 |

另外：D11 的度量（行跟踪开在测试里，一次插件调用的开销）写进 PR2 的描述；`cargo fmt`、`cargo test -p rutis-loader -p rutis-bridge -p rutis-host`、`npm --prefix node/rutis test`、`(cd bun/rutis-bun && bun test)` 按 CONTRIBUTING 跑相关的那些。
