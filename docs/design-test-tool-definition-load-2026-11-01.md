# 测试工具：`load` 装接受测插件交出的插件定义

[English](design-test-tool-definition-load-2026-11-01.en.md)

状态：草稿，未实现。日期：2026-11-01

Issue：[#1004](https://github.com/arcships/rutis/issues/1004)（TypeScript 测试工具：测试进程收不到插件定义）。按本稿实施的是它的 PR1、PR2（§六）。上游规则见[插件是一个值](design-plugin-values-2026-10-25.md)的 R10 与 §五.5。

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

## 二、决策

| # | 决定 |
| --- | --- |
| D1 | **测试侧的会话收下定义。** 会话新增一个选项 `host`（只有测试工具用，`node/rutis-runtime/src/testing/start.mjs` 与 Bun 的同一份）：声明 `plugins`，并把对端发来的 `plugin` 引用留成一个能原样交回的值（D2）。放弃：所有会话默认收下——运行时收到别的运行时的定义本来就用不了，维持现在的拒绝，错误才说得准。 |
| D2 | **替身收到的是一个不透明的值。** 它既不是 `definePlugin` 的结果，也不是能调用的代理：只有标记 `Symbol.for('rutis.plugin-definition')`，不能读字段、不能调用；原样交给任何服务时按 `home: true` 发回原来的引用，插件拿回的是它自己那个定义。放弃：给它加 `apply`，让测试进程也能挂——副本要在定义它的运行时里跑（R5），本地挂是假的。 |
| D3 | **`load` 收这个值。** `@arcships/rutis/testing` 的 `load()` 与 `host().load()` 除了 `definePlugin` 的结果，也收 D2 的值：跳过"模块默认导出"和插件 API 的检查，把定义本身交给驱动的新方法 `loadDefinition(definition, config, options)`。放弃：另加一个 `loadDefinition()` 函数——R10 要求直接交给 `load`，同一件事不该有两种写法。 |
| D4 | **宿主入口是新方法，不改 `load`。** `rutis-host/test` 增加<br>`pub async fn load_definition(&self, plugin: Definition, config: Option<Value>, options: Option<Options>) -> Result<String, String>`<br>`Definition` 是 `#[rutis::service]` 认的参数类型（R9），`Options` 仍是 `{ id, isolate }`。放弃：把 `load` 的 `name: String` 改成 `Value` 再分支——"要名字"的参数变成什么都收，`rutis-host types` 里两个入口看不出区别，传错时错误也不再指向写错的地方。 |
| D5 | **rutis-loader 的入口是新方法，不扩 `NewEntry`。**<br>`pub async fn load_copy(&self, definition: &Definition, config: Value, options: CopyOptions) -> Result<(String, FiberView), LoaderError>`<br>理由：`NewEntry` 是**可编辑层**的输入，会被写成一行 JSON（`id`/`name`/`config`/`isolate`），`name` 由 resolver 解析成模块或运行时；插件定义是一个值，写不进层、也解析不了，`reconcile` 无法从层里重建它。所以副本是**只在运行期存在**的行：进运行记录、出现在 `entries()` 和 `status`、能 `remove`，但不在任何层里。放弃：`NewEntry { entry: Module \| Definition }`——层里会留下一个永远重建不出来的行。放弃：让 `Harness` 自己 `ctx.plugin(def.at(..))` 挂在宿主根上——副本不出现在 `status` 和 `entries()`，卸载也没有统一入口。 |
| D6 | **副本是内核记着的一份。** `load_copy` 走 `ctx.plugin(definition.at(&ctx, config)?)`：内核把它记成这个定义的一份（R6），来源行卸载、重启或结束时先结束副本、等它们清理完，再结束来源行，之后 `ended()` 完成；定义它的运行时进程没了，副本随之结束。运行时里那一行仍由现有的 `DefinitionMounter` 建（`crates/rutis-loader/src/loader/plugins.rs:206` → `crates/rutis-loader/src/runtime.rs:945` 的 `JsFactory`，`RowEntry::Definition`，`rows.load` 的 entry 是定义本身，§五.5），`rows.unload` 照旧。放弃：在 loader 里另记一份副本表、自己排卸载顺序——内核已按 R6 排好，复刻只会多一套规则。 |
| D7 | **位置：来源行所在的组。** 副本挂在交出定义的那一行所在的组上下文里（按 `ForeignPlugin::row()` 在运行记录里找那一行）；属于进程的定义（没有来源行）挂在 loader 的根组。内核的位置检查（R4）照旧，失败时把内核的话原样交回。放弃：总挂在根组——来源行在实例里时 `at` 会拒绝（"can only be mounted inside … or deeper"）。 |
| D8 | **行 id 与事件里的名字分开。** 行 id 由 `options.id` 给，否则用定义的名字（定义没有名字时用来源行的 id），被占用时 `-2`、`-3`……，和 `load` 的 `free_id` 一样；这个 id 是宿主卸载、列举、`status` 用的句柄。rutis 在 `rutis/service` 事件、监听器描述、`CallerRow` 和错误里仍按 R5 用**交出它的那一行的 id** 命名这一份（属于进程的定义用定义的名字）。放弃：让副本用自己的行 id 出现在事件里——R5 已定下副本按来源行命名，改了两份副本会有两种名字，和插件里 `ctx.plugin(def.at(..))` 挂出来的那一份也对不上。 |
| D9 | **配置在挂上时定下**（§五.5）。`update`（`Loader::update`、测试的 `update()`）对定义行拒绝；`isolate`、`move`、`rename` 同样拒绝。报错见 §四。 |
| D10 | **不做的事：** `rutis.json` 里写插件定义、`rutis/loader.create` 收定义（不是一个能写进层、能重建的名字）；同一份定义 `load` 两次共享一行（每次一份副本，和 `ctx.plugin(def.at(..))` 一样）；`rutis-host check` 认识运行期才交出的定义（R8 不变）；Python/Go/Swift/Kotlin 的测试工具（它们的替身早就能收到定义本身，行为不变）。 |

## 三、规则

| 规则 | 行为 |
| --- | --- |
| 入口 | `load(definition, { config, services, id, isolate })`（SDK）→ 驱动 `loadDefinition` → `rutis-host/test` 的 `load_definition` → `Loader::load_copy`。`config` 与 `id` 一样传到底；`services` 仍由测试工具作为运行时里的行先提供（和今天一样）。 |
| 返回 | 宿主给的行 id。测试工具的行对象照旧：`row`、`service`、`methods`、`provided`、`reported`、`on`、`update`、`unload`、`host`。 |
| 等待 | 和 `load` 的行一样：声明的 `inject` 里的服务没到时等它，到了再启动，不失败。 |
| 行 id | `options.id`，否则定义的名字，被占用时 `-2`、`-3`……；和 `load` 给名字起的 id 一样规则。 |
| 名字（事件、错误） | 交出它的那一行的 id（R5）；属于进程的定义用定义的名字。所以 `provided()`、`reported()` 看到的是来源行名下的服务和失败。 |
| `status` / `entries()` | 副本与行一起列出：`id` 是上面的行 id，`name` 是定义的名字（没有名字时是来源行的 id），状态取那份 fiber。已结束的副本在下一次 `reconcile` 或 `status` 时从记录里去掉。 |
| 卸载 | `unload` / `Loader::remove` 只结束这一份，和行一样：先撤它提供的服务，再 `rows.unload`，最后释放租约。 |
| 来源结束 | 来源行卸载、重启或结束：先结束副本并等它们清理完，再结束来源行；之后 `ended()` 完成，副本的记录在下一次扫描时去掉。 |
| 运行时结束 | 定义它的运行时进程没了：副本随之结束，记录在下一次扫描时去掉。 |
| 已结束的定义 | 来源已经结束（或副本在它结束后才 `load`）：`at` 失败，宿主把内核的话原样交回（§四）。 |

跨语言：这些规则只加在 Node 和 Bun 的测试工具与它们的宿主入口上；其他语言的测试工具不受影响（D10）。

## 四、开发者看到什么

角色：写测试的插件作者（TypeScript；Bun 只有运行时包名不同，写法相同）。

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

测试通过时没有输出。`parts[0]` 就是插件 `apply` 里造的那个定义，装在定义它的运行时里，和 `rutis-host` 装它时一样：配置按定义的 schema 校验并补上默认值，`inject` 的服务都在了才启动，随来源行结束。

**出错时。**每条一行：怎样触发、逐字输出、在哪条语句上抛、怎样改。

| 触发 | 输出（逐字） | 位置 | 怎样改 |
| --- | --- | --- | --- |
| 交出定义的那一行已经卸载，再 `load(definition)` | `PluginTestError: part: plugin definition part has ended with its origin` | `load` 抛出的错误 | 在交出定义的行还在时 `load`；先卸副本，再卸来源行 |
| 对定义行 `update(config)` | `PluginTestError: part: a copy of a plugin definition has the config it was loaded with: unload it and load it again` | `update` 抛出的错误 | 卸掉这一行，用新配置重新 `load` |
| 对定义行给 `isolate` | `PluginTestError: part: a plugin definition runs where the row that handed it over runs: load it without isolate` | `load` 抛出的错误 | 去掉 `isolate` |
| 配置不合定义声明的 schema | `PluginTestError: part: config: depth should be a number, got "deep"` | `load` 抛出的错误 | 按定义声明的 schema 改配置 |
| `RUTIS_HOST` 指向比测试工具旧的 `rutis-host`（0.19 及更早，没有 `load_definition`） | `PluginTestError: the test driver has no method loadDefinition` | `load` 抛出的错误 | 换成同版本的 `@arcships/rutis-host`，或把 `RUTIS_HOST` 指向新版的二进制 |

输出里出现的只有开发者写过的东西：行 id、定义的名字、配置的路径。实现的 PR 用验收测试逐字比较这张表。

**要懂的概念。** 没有新增：R10 已经要求"替身收到的定义可以直接交给 `load`"，这里只是 Node/Bun 补上。测试作者仍只需知道"定义在定义它的运行时里作为一行运行，随交出它的那一行结束"。

## 五、兼容与迁移

- **协议：** 没有新帧，握手格式不变。测试侧的会话多声明一个能力 `plugins`（R10 §五.3 已经定义的能力，测试工具这一侧之前没声明）；不声明的那一端行为不变。
- **宿主接口：** `rutis-host/test` 多一个方法 `load_definition`。0.19 及更早的宿主没有它，新 `load` 遇到定义时报 §四的最后一条。版本要一致这件事今天就在（`@arcships/rutis-host` 是测试工具的 dev dependency，`RUTIS_HOST` 由使用者给）。
- **运行时包：** 定义由运行时包（`@arcships/rutis-runtime` / `@arcships/rutis-bun`）交出来，旧版本的运行时不交定义，也不认 `loadDefinition`，报同一条。
- **公开 API：** `@arcships/rutis/testing` 的 `load` 多收一种值；TS 声明不变（`Definition` 早已写成 TS 的插件类型，R9）。`rutis_loader::Loader::load_copy` 是新方法，`rutis_bridge::runtime` 多一个从 `Definition` 取 `ForeignPlugin` 的函数。
- **不兼容的改动：** 没有。迁移指南不写。

## 六、实施

**PR1：测试侧声明并收下定义（D1、D2）。**
`node/rutis-runtime`（`src/testing/start.mjs`、`src/session.mjs`）与 `bun/rutis-bun` 的同一份。
完成条件：测试给的服务收到 `register(定义)`，替身把它原样交回时插件拿到的是它自己的那个定义；`node/rutis/test` 与 `bun/rutis-bun/test` 各一个用例；`load` 的行为不变。

**PR2：按 D3–D9 让 `load` 装上定义。**
`crates/rutis-loader`（`Loader::load_copy`；`entries`、`get`、`remove`、`status` 认副本）、`crates/rutis-bridge`（`Definition` → `ForeignPlugin`）、`crates/rutis-host`（`load_definition`）、`node/rutis-runtime/src/testing/rutis-test.mjs` 与 `node/rutis`（`load` 收定义）、Bun 的同一份。
完成条件：`Loader` 的单测（装上、卸载、来源结束连带结束、`status` 里看得见、`update` 与 `isolate` 被拒）；`rutis-host` 的测试工具用例；`node/rutis/test` 里针对本 issue 的复现测试（替身接住定义、`load` 装上 Session 部件那种形状）与 Bun 的同名用例；Dim 的 `runtime/plugins/scheduler/scheduler.test.mjs` 那种写法能跑通；文档：`docs/guide/plugin-api.md` 的"替身收到的定义可以直接装上"那句加上 Node 与 Bun，发布说明各写一条；保证表 `loader/plugin-value-copies` 那一条补上"定义作为行"的一段，用例指向 PR2 的测试。
