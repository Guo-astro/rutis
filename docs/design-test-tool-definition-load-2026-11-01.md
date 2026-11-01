# 测试工具：`load` 装接受测插件交出的插件定义

[English](design-test-tool-definition-load-2026-11-01.en.md)

状态：草稿，未实现。日期：2026-11-01（同日修订：评审五条。补上来源这一代的保存与恢复（§二 D6、D7）、副本身份贯穿服务与错误查询（D8）、只在运行期存在的行跨 `reconcile` 的规则（D9）、按作用域定位来源（D7）；§四 补齐 `move`/`rename`、位置拒绝、宿主拿到的错误、`status`/`entries` 的逐字输出与宿主持有者角色；D10 补上放弃的做法。）

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

### 一之一、为什么"第一次在 Rust 解码"会绑错来源

测试提供的服务与插件之间的调用不经过宿主：测试的服务是运行时里的一行（`node/rutis-runtime/src/testing/service.mjs`），它的方法经驱动的控制会话直接到测试进程。所以定义被交出的那一刻，宿主看不到这一次调用；宿主第一次读到这个定义，是驱动随后调 `load_definition(definition, …)` 的时候。`#[rutis::service]` 的 `Definition` 参数在这一刻读来源（`crates/rutis-bridge/src/marked.rs:642–655` 的 `caller_row()`），读到的是**驱动那一行**，`crates/rutis-bridge/src/runtime/definitions.rs:128–147` 就按驱动这一代建定义：副本会随测试结束，而不是随交出它的插件的那一行结束。D6、D7 解决这一条。

## 二、决策

| # | 决定 |
| --- | --- |
| D1 | **测试侧的会话收下定义。** 会话新增一个选项 `host`（只有测试工具用，`node/rutis-runtime/src/testing/start.mjs` 与 Bun 的同一份）：声明 `plugins`，并把对端发来的 `plugin` 引用留成一个能原样交回的值（D2）。放弃：所有会话默认收下——运行时收到别的运行时的定义本来就用不了，维持现在的拒绝，错误才说得准。 |
| D2 | **替身收到的是一个不透明的值。** 它既不是 `definePlugin` 的结果，也不是能调用的代理：只有标记 `Symbol.for('rutis.plugin-definition')`，不能读字段、不能调用；原样交给任何服务时按 `home: true` 发回原来的引用，插件拿回的是它自己那个定义。放弃：给它加 `apply`，让测试进程也能挂——副本要在定义它的运行时里跑（R5），本地挂是假的。 |
| D3 | **`load` 收这个值。** `@arcships/rutis/testing` 的 `load()` 与 `host().load()` 除了 `definePlugin` 的结果，也收 D2 的值：跳过"模块默认导出"和插件 API 的检查，把定义本身交给驱动的新方法 `loadDefinition(definition, config, options)`。放弃：另加一个 `loadDefinition()` 函数——R10 要求直接交给 `load`，同一件事不该有两种写法。 |
| D4 | **宿主入口是新方法，不改 `load`。** `rutis-host/test` 增加<br>`pub async fn load_definition(&self, plugin: Definition, config: Option<Value>, options: Option<Options>, owner: Option<Owner>) -> Result<Loaded, String>`<br>其中 `Loaded { id, provides }`（`provides` 是定义声明的服务与方法是 `sync` 还是 `async`，`plugins.read` 已经读过），`Owner { row: String, scope: Vec<Scope> }` 见 D6、D7，`Options` 仍是 `{ id, isolate }`。放弃：把 `load` 的 `name: String` 改成 `Value` 再分支——"要名字"的参数变成什么都收，`rutis-host types` 里两个入口看不出区别，传错时错误也不再指向写错的地方。放弃：返回值只给行 id，让测试工具再去问一次声明——`service()` 拿不到方法表就调不了同步方法（§三 第 3 条）。 |
| D5 | **rutis-loader 的入口是新方法，不扩 `NewEntry`。**<br>`pub async fn load_copy(&self, definition: &Definition, config: Value, options: CopyOptions, owner: Owner) -> Result<(String, FiberView), LoaderError>`<br>理由：`NewEntry` 是**可编辑层**的输入，会被写成一行 JSON（`id`/`name`/`config`/`isolate`），`name` 由 resolver 解析成模块或运行时；插件定义是一个值，写不进层、也解析不了，`reconcile` 无法从层里重建它。所以副本是**只在运行期存在**的行（D9）。放弃：`NewEntry { entry: Module \| Definition }`——层里会留下一个永远重建不出来的行。放弃：让 `Harness` 自己 `ctx.plugin(def.at(..))` 挂在宿主根上——副本不出现在 `status` 和 `entries()`，卸载也没有统一入口。 |
| D6 | **来源这一代由运行时记下，随 `load` 交给宿主，宿主按它重新绑定。** 运行时在插件交出定义的那一刻就知道它是哪一行交的，把这一行的运行时键（`{instance}.{generation}`）连同它的 loader 行 id 与作用域链（`rows.load` 给的 `place`）记在这次导出的引用上；测试把定义交回来时，驱动据此调 `load_definition(…, owner)`，宿主在这一行**当前这一代**上重建定义（D7）。三处一起构成 D6：<br>· 运行时：行跟踪（`node/rutis-runtime/src/unhandled.mjs` 的 `rowTracking`）在装载测试驱动时打开（别的运行不打开，见 D11 的代价），叶子插件的 `apply` 与它的监听者在 `tracking.within(row, …)` 里跑（`node/rutis-runtime/src/leaf.mjs` 的 `toCordis`），驱动的会话带上 `owner: tracking.running`（`session.mjs` 已经在用它，`Process.connect` 的选项），于是每个导出的插件定义都记着自己那一代；驱动经 `ownerOf(value)` 取回它。<br>· 运行时：驱动在 `loadDefinition` 里先问运行时"这一代还在不在"（`running(key)`）；不在就拒绝（§四），这样重启前交出的旧值装不上。<br>· 宿主：`Definition` 参数已经按驱动这一代解过一次（§一之一），方法体按 `owner` 重新绑定：`rutis_bridge::runtime` 新增带显式来源的读取（把按值缓存的那一条换掉，声明沿用已读到的），来源是这一行的**当前这一代**（`Loader` 里的 `FiberView` → 内核内部取出它的 `Ctx`，`CallerRow::new(行 id, 键, ctx)`）；`Definition::from_source` 随后把这一定义注册在这一代的 fiber 上，R6 于是成立：先结束副本、等它们清理完，再结束来源行，之后 `ended()` 完成，旧值装不上。<br>放弃：不绑来源，副本随测试结束——R6 不成立，Dim 的"先卸副本再卸 owner"就只是测试自己的约定。放弃：让交出定义的调用经过宿主（测试的服务改由宿主提供），好让 `caller_row()` 直接读到 owner——`plugin` 引用不能转交（§五.2），宿主没法把一个泛用的服务调用连同定义转发给驱动，而 #988 才把测试的服务改成运行时里的行。放弃：要求测试显式写 `load(definition, { handedOverBy: 'index' })`——R10 说定义直接交给 `load`；作为 D6 成本不可接受时的退路保留，见 §六 的度量项。**D6 是这份稿子唯一改动运行时结构的地方：评审若更愿意改成测试显式给来源，D6 整段换成那条退路，§六 的验收不变。** |
| D7 | **按作用域身份定位来源这一代，不用行 id 字符串。** `Owner { row, scope }` 里 `scope` 是 `rows.load` 交给运行时的那条实例链：同一个行 id 在多个实例里同时运行时，链不同（D6 的验收含双实例）；应用自己 `mark_scope` 出来的来源在 loader 里没有行的，`scope` 也对不上任何一条。宿主按 `(row, scope)` 找那一行当前的 `Running` 记录，用它挂副本：位置是这一行的实例链里的组上下文，内核 R4 的检查照旧（D5 的 `load_copy` 用 `at` 的同一段检查与文字），不放宽。找不到匹配的记录时拒绝（§四）。放弃：只按行 id 找——同一行在多个实例里跑时找错实例，副本会挂进另一个实例的作用域，实例隔离随之失效。放弃：把 R4 放宽成"任意位置都能挂"——来源在实例里时，副本会看到别的实例的服务，和插件里 `ctx.plugin(def.at(..))` 的行为不一致。 |
| D8 | **副本以自己的行 id 出现在服务、错误与 `status` 里。** 定义行是行：`rutis/service` 事件的 `row`、监听器描述、`CallerRow`、错误行和 `status` 的 `services:` 都用它的行 id（§五.5 "这一份在运行时里就是一行"）。R5 那句"副本的 `row` 是来源行的 id"仍适用于**插件用 `ctx.plugin(def.at(..))` 挂出来的**那一份；定义行不是它（PR2 在 plugin values 设计稿的 R5 与 §五.5 各加一句说明，中英两份）。放弃：按 R5 用来源行 id——两份副本会共用一个名字（`provided()` 分不开、服务撞名的错误指错行），而 `status` 的 `services:` 会显示成来源行提供的。放弃：`provided()` 映射到来源行——同一份定义的两份副本混在一起，正是要避免的。 |
| D9 | **副本只在运行期存在，跨 `reconcile` 不被碰。** 副本记在 loader 的 `State.copies`（`id → { fiber, definition, owner, config }`），不在 `desired`、不在 `running`（`crates/rutis-loader/src/loader/reconcile.rs:821–854` 只保留 `desired` 里的记录，后面还有按 `desired` 查找的 `unwrap`，副本进 `running` 会踩到它）。规则：<br>· 普通行的 `create`/`update`/`remove`/`move`/`rename`、以及任何一次 `reconcile` 都不动副本：不卸载、不重启、不发 `rows.update`（副本的配置在挂上时定下，D10）。<br>· 重键：没有。副本不随层里的行改身份：它没有 `name` 可重解析，配置不变，实例由 D7 的作用域定。<br>· id 与行共用一个命名空间：`free_id`、`Loader::get`、`create` 的重复 id 检查都把 `copies` 算进去；`options.id` 撞上任何一行或一份副本时按 `create` 现在的说法拒绝（`a row <id> is loaded already`）。<br>· `entries()` 与 `status` 列出副本：`EntryInfo.owner` 是新的 `Owner::Copy`（`crates/rutis-loader/src/patch.rs:62` 的公开枚举加一个变体），`options` 是 `{ id, name }`，`status` 取那份 fiber。<br>· 清扫：fiber 进入 `Disposed` 的副本在下一次 `reconcile` 或 `status` 时从 `copies` 里去掉（来源行结束、运行时没了、或宿主卸掉它都是这一条）；`Failed` 的留着，和行一样能在 `status` 里看到，直到 `remove`。<br>放弃：把副本放进 `running` 并用一个空的 `desired` 行兜住——`reconcile` 的 `unwrap`、`Slot`、实例与配置求值都会跟着改，等于给副本一套假的层。放弃：副本不记在 loader 里（宿主自己拿着 `FiberView`）——`status`、`entries()`、`remove` 就没有统一入口，`rutis-host status` 里看不到它。 |
| D10 | **配置在挂上时定下**（§五.5）。`update` 对定义行拒绝，`set_disabled`、`reload`、`move`、`rename` 也拒绝（副本不在层里）；报告见 §四。放弃：把 `update` 转给运行时——定义行在运行时里没有 `rows.update`（§五.5），转过去只会得到一个更远的错误；R5 说的做法是卸掉再按新配置挂。 |
| D11 | **代价：行跟踪只在测试里打开。** D6 要让运行时知道"现在跑的是哪一行"，用的是 `rowTracking`（今天只在 `RUTIS_DEV` 或 `--prefix-output` 时打开，`node/rutis-runtime/src/runner.mjs:106`）。判据是测试驱动在装载（`RUTIS_TEST_CONTROL` 在环境里），普通运行不付这个开销。PR2 记一次这个开关的实测开销（每个插件调用一次 `AsyncLocalStorage`，外加 promise 初始化钩子），超出预算就按 D6 的退路改成测试显式给来源。 |
| D12 | **不做的事：** `rutis.json` 里写插件定义、`rutis/loader.create` 收定义（不是一个能写进层、能重建的名字）；同一份定义 `load` 两次共享一行（每次一份副本，和 `ctx.plugin(def.at(..))` 一样）；`rutis-host check` 认识运行期才交出的定义（R8 不变）；`rutis-host types` 为 `load_definition` 生成新的客户端声明（`rutis-host/test` 是宿主自己的服务，测试工具用 JS 调它）；Python/Go/Swift/Kotlin 的测试工具（它们的替身早就能收到定义本身，行为不变）。 |

## 三、规则

| 规则 | 行为 |
| --- | --- |
| 入口 | `load(definition, { config, services, id, isolate })`（SDK）→ 驱动 `loadDefinition` → `rutis-host/test` 的 `load_definition` → `Loader::load_copy`。`config`、`id` 一样传到底；`services` 仍由测试工具作为运行时里的行先提供（和今天一样）。 |
| 1. 来源 | 来源这一代由运行时记在导出的引用上，驱动经 `owner { row, scope }` 交给宿主（D6、D7）；宿主找不到这一代就拒绝；来源行结束、重启时先结束副本（R6），旧值装不上（D6 的运行时检查）。 |
| 2. 返回 | `Loaded { id, provides }`：行 id 与定义声明的服务及方法种类。测试工具的行对象与 `load` 的行相同。 |
| 3. `service()` | 测试工具用 `Loaded.provides` 建方法表（同步方法仍走同步调用），不再读 `plugin.provides`（不透明值没有这个字段）。`methods(name)` 同源。 |
| 4. 等待 | 和 `load` 的行一样：声明的 `inject` 里的服务没到时等它，到了再启动，不失败。 |
| 行 id | `options.id`，否则定义的名字，被占用时 `-2`、`-3`……；和 `load` 给名字起的 id 一样规则，命名空间含副本（D9）。 |
| 名字（服务、错误） | 副本自己的行 id（D8）：`rutis/service` 的 `row`、错误行、`status` 的 `services:`、`provided()`、`reported()` 都用它。 |
| `status` / `entries()` | 副本与行一起列出：`id` 是行 id，`name` 是定义的名字（没有名字时是来源行 id），`owner` 是 `Owner::Copy`，状态取那份 fiber（D9）。 |
| 卸载 | `unload` / `Loader::remove` 只结束这一份，和行一样：先撤它提供的服务，再 `rows.unload`，最后释放租约。 |
| 来源结束 | 来源行卸载、重启或结束：先结束副本并等它们清理完，再结束来源行；之后 `ended()` 完成，副本的记录在下一次清扫时去掉。 |
| 运行时结束 | 定义它的运行时进程没了：副本随之结束（连接断了），记录在下一次清扫时去掉。 |
| 层里的编辑 | 与副本无关的 `create`/`update`/`remove`/`move`/`rename` 与任何一次 `reconcile` 都不动副本，也不给它发 `rows.update`（D9）。 |
| 已结束的定义 | 来源已经结束、或在它结束后才交回来：运行时拒绝（§四 第 1 条）；两者之间才结束的，`load_copy` 用 `at` 的同一句话拒绝（§四 第 3 条）。 |

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
| 交出定义的那一行已经卸载或重启（旧值），再 `load(definition)` | `PluginTestError: the plugin definition was handed over by a row that is no longer running: load the definition that row hands over now` | `load` 抛出的错误 | 用这一行现在交出的那份定义，或先把它 `load` 起来再卸那一行 |
| 交出定义的行不是这个宿主上的行（应用自己的插件交出的定义） | `PluginTestError: part: the row index that handed the plugin definition over is not a row of this host: only a plugin loaded as a row of the host can hand one over` | `load` 抛出的错误 | 改用宿主上的一行交出定义；应用里用 `ctx.plugin(def.at(..))` 挂 |
| 来源行在运行时检查和装载之间结束（少见） | `PluginTestError: part: plugin definition part has ended with its origin` | `load` 抛出的错误 | 同第一条 |
| 定义的实例链与来源行所在的实例对不上 | `PluginTestError: part: plugin definition part can only be mounted inside <来源实例> or deeper, not inside <这里>` | `load` 抛出的错误 | 从交出它的那个实例里的测试去 `load` |
| 对定义行 `update(config)` | `PluginTestError: part: a copy of a plugin definition has the config it was loaded with: unload it and load it again` | `update` 抛出的错误 | 卸掉这一行，用新配置重新 `load` |
| 对定义行 `move`/`rename`/`set_disabled`/`reload`（`rutis/loader` 的方法） | `part: a copy of a plugin definition is not a row of the configuration: unload it and load it again` | 该调用抛出的错误 | 同上 |
| 对定义行给 `isolate` | `PluginTestError: part: a plugin definition runs where the row that handed it over runs: load it without isolate` | `load` 抛出的错误 | 去掉 `isolate` |
| 配置不合定义声明的 schema | `PluginTestError: part: config: depth should be a number, got "deep"` | `load` 抛出的错误 | 按定义声明的 schema 改配置 |
| `RUTIS_HOST` 指向比测试工具旧的 `rutis-host`（0.19 及更早，没有 `load_definition`） | `PluginTestError: the test driver has no method loadDefinition` | `load` 抛出的错误 | 换成同版本的 `@arcships/rutis-host`，或把 `RUTIS_HOST` 指向新版的二进制 |

输出里出现的只有开发者写过的东西：行 id、定义的名字、配置的路径、实例链。实现的 PR 用验收测试逐字比较这张表。

**要懂的概念。** 没有新增：R10 已经要求"替身收到的定义可以直接交给 `load`"。测试作者仍只需知道"定义在定义它的运行时里作为一行运行，随交出它的那一行结束"。

### 4.2 用 `rutis-host` 组装插件的人

`rutis.json` 没有新字段；`rutis-host check` 的输出不变（R8：运行期才交出的定义它不知道）。`rutis-host status` 多一种行——定义行，和其他行一样在 `rows:` 里，它的服务在 `services:` 里以它的行 id 为提供方（D8）：

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

来源行卸载后，`part` 那一行随之消失（R6）；`part` 跑过又失败时留着，和其它行一样显示 `failed: …`。

### 4.3 宿主持有者（嵌入 rutis 的应用，Rust）

```rust
// 把一份别的语言交来的定义装成一行，来源是交出它的那一行（D6、D7）。
let (id, _view) = loader
    .load_copy(&definition, json!({ "depth": 2 }), CopyOptions::default(), owner)
    .await?;
// 卸载：和其它行同一个入口。
loader.remove(&id).await?;
```

`entries()` 里这一行的 `owner` 是 `Owner::Copy`（D9），`options` 是 `{ id, name }`；`status()` 的 `rows` 里有 `{ id, state }`。错误：`Err(LoaderError)`/`Err(String)` 里的文字就是 §4.1 那一行去掉 `PluginTestError: ` 前缀；`rutis-host/test` 与 `Loader::load_copy` 都不新增错误码，和 `load` 一样。不兼容：`rutis_loader::patch::Owner` 多一个变体（§五）。

### 4.4 部署的人

看不到变化：协议没有新帧，`rutis.json` 没有新字段，`rutis-host run`/`dev`/`check` 的输出不变。

## 五、兼容与迁移

- **协议：** 没有新帧，握手格式不变。测试侧的会话多声明一个能力 `plugins`（R10 §五.3 已经定义的能力，测试工具这一侧之前没声明）；不声明的那一端行为不变。
- **宿主接口：** `rutis-host/test` 多一个方法 `load_definition`（带 `owner`）。0.19 及更早的宿主没有它，新 `load` 遇到定义时报 §4.1 的最后一条。版本要一致这件事今天就在（`@arcships/rutis-host` 是测试工具的 dev dependency，`RUTIS_HOST` 由使用者给）。
- **运行时包：** 定义由运行时包（`@arcships/rutis-runtime` / `@arcships/rutis-bun`）交出来，来源也由它记下；旧运行时既不记来源也不认 `loadDefinition`，报同一条。
- **公开 API：** `@arcships/rutis/testing` 的 `load` 多收一种值（TS 声明不变：`Definition` 早已写成 TS 的插件类型，R9）；`rutis_loader::Loader::load_copy` 与 `CopyOptions` 是新的；`rutis_loader::patch::Owner` 多一个变体（枚举不是 `#[non_exhaustive]`，穷尽匹配的代码要加一条，仓内的调用方在 PR2 里改完）；`rutis_bridge::runtime` 多一个从 `Definition` 取 `ForeignPlugin` 的函数与一个带显式来源的读取函数；`rutis::internal::definitions` 多几项（内部，无稳定性承诺）。
- **不兼容的改动：** 只有 `Owner` 的新变体，属编译期可见、行为不变；0.20 的发布说明写一条。迁移指南不写。
- **文档：** plugin values 设计稿的 R5 与 §五.5 各加一句（D8 收窄 R5 的适用范围；定义行按自己的行 id 命名，中英两份）。

## 六、实施

**PR1：测试侧声明并收下定义（D1、D2）。**
`node/rutis-runtime`（`src/testing/start.mjs`、`src/session.mjs`）与 `bun/rutis-bun` 的同一份。
完成条件：测试给的服务收到 `register(定义)`，替身把它原样交回时插件拿到的是它自己的那个定义；`node/rutis/test` 与 `bun/rutis-bun/test` 各一个用例；`load` 的行为不变。
**PR1 不关闭 #1004**（Dim 的写法仍然跑不起来），PR2 才关（PR2 合并时写 `Closes #1004`）。

**PR2：按 D3–D11 让 `load` 装上定义。**
`crates/rutis-loader`（`Loader::load_copy`、`CopyOptions`、`State.copies`、`entries`/`get`/`remove`/`status` 认副本、`Owner::Copy`、从 `FiberView` 取这一代的 `Ctx`）、`crates/rutis`（`internal` 里几项：`definitions` 的作用域与 `at` 的那段检查、`FiberView` 取它的 `Ctx`）、`crates/rutis-bridge`（`Definition` → `ForeignPlugin`、带显式来源的读取）、`crates/rutis-host`（`load_definition`）、`node/rutis-runtime`（驱动的 `loadDefinition`、`ownerOf`、`running`、行跟踪的开关；`leaf.mjs` 的 `within`）、`node/rutis`（`load` 收定义、`service()` 用 `Loaded.provides`）、Bun 的同一份。

完成条件（逐条对评审的五点）：

1. **来源**：来源行卸载时，先结束副本并等它们的清理跑完，来源行的卸载才完成；之后 `ended()` 完成，旧值再 `load` 报 §4.1 第 1 条；来源行重启后，新的一代交出的定义装得上，旧值装不上。用例：`Loader` 的单测（来源结束的连带与顺序）+ `node/rutis/test` 的一个用例（owner 卸载 → 副本结束、旧值报错）。
2. **身份**：定义行提供 `echo` 时，`status` 的 `services:` 显示 `echo: part`；`provided()` 列出 `echo`；该行的监听者失败时 `reported()` 拿得到那一行；同一份定义的两份副本各有各的 id，各自的 `provided()` 分得开。用例：`node/rutis/test` 两个用例（`provided`/`reported`；两份副本）。
3. **跨 `reconcile`**：无关的 `create`/`update`/`remove` 与一次 `reconcile` 之后，副本的 fiber 身份不变（同一个 `PluginId`），运行时没有收到这一行的 `rows.update`；`create` 用副本的 id 被拒；`Loader::remove(副本 id)` 结束它。用例：`rutis-loader/tests/` 里的一组单测。
4. **实例**：一个实例化的组里两行各交出一份定义，两份副本各自挂在自己的实例里，服务的隔离按实例（两个实例的同名服务各是自己的）；把一份定义装到另一个实例的位置上报 §4.1 的位置那一条；R4 的检查与 `ctx.plugin(def.at(..))` 同一段代码。用例：`rutis-loader/tests/plugin_values.rs` 里补双实例的一组。
5. **文档**：`docs/guide/plugin-api.md`/`.en.md` 的"替身收到的定义可以直接装上"那句加上 Node 与 Bun；plugin values 设计稿的 R5 与 §五.5 各加一句（D8）；`docs/guarantees.md`/`.en.md` 的 `loader/plugin-value-copies` 补"定义作为行"一段，用例指向本 PR；发布说明各一条（`Owner` 新变体）。

另外：D11 的度量（行跟踪开在测试里，一次插件调用的开销）写进 PR2 的描述；`cargo fmt`、`cargo test -p rutis-loader -p rutis-bridge`、`npm --prefix node/rutis test`、`(cd bun/rutis-bun && bun test)` 按 CONTRIBUTING 跑相关的那些。
