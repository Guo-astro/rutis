# 测试工具：`load` 装接受测插件交出的插件定义

[English](design-test-tool-definition-load-2026-11-01.en.md)

状态：草稿，未实现。日期：2026-11-01（同日改稿：路线定为"把定义装成宿主上的一行"（C1），和 Python/Go/Swift/Kotlin 的测试工具同一条路；删去上一版的来源绑定、`PluginRef` 参数种类、`OriginResolver`、`rutis-loader` 的副本行与 `reconcile` 规则、`Owner` 新变体。本稿不碰 Rust，无不兼容改动。）

Issue：[#1004](https://github.com/arcships/rutis/issues/1004)（TypeScript 测试工具：测试进程收不到插件定义）。按本稿实施的是它的 PR1、PR2（§七）。上游规则见[插件是一个值](design-plugin-values-2026-10-25.md)的 R10 与 §五.5；本文 §四 写明与真实宿主的差异，R10 处加了指向本节的一句。

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

Python、Go、Swift、Kotlin 的测试工具没有这个问题：它们的运行时就在测试进程里，替身收到的是同一份定义，测试工具的 `load` 把它**装成宿主上的一行**（Python 就是三行：`sys.modules[entry] = holder; holder.plugin = plugin; id = await host.harness.load(entry, config, options)`，`python/rutis/rutis/testing.py:487–489`）。本稿让 Node/Bun 走同一条路：定义交回定义它的运行时，运行时按一个注册名解析它，宿主照旧装成一行。

## 二、做法

| # | 决定 |
| --- | --- |
| D1 | **测试侧的会话收下定义。** 会话新增选项 `host`（只有测试工具用，`node/rutis-runtime/src/testing/start.mjs` 与 Bun 的 `testing/start.ts`）：声明 `plugins`；`session.mjs`（Bun `session.ts:443`）收到对端发来的 `plugin` 引用时不再 `#refuse`，而是留一条 import 记录（和函数、对象引用同一套 `#imports`/`#proxies`/`FinalizationRegistry` 生命周期），交给测试一个不透明的值：只有标记 `Symbol.for('rutis.plugin-definition')`，不能读字段、不能调用。原样交回任何服务时按 `home: true` 编码，对方拿到的是它自己那个值（`session.mjs:447`）。放弃：所有会话默认收下——运行时收到别的运行时的定义本来就用不了，别的会话维持现在的拒绝，错误才说得准。放弃：给这个值加 `apply`，让测试进程也能挂——副本要在定义它的运行时里跑，本地挂是假的。 |
| D2 | **`load` 认这个值。** `@arcships/rutis/testing` 的 `load()` 与 `host().load()` 除了 `definePlugin` 的结果，也认 D1 的值：跳过"模块默认导出"和插件 API 的检查，调驱动的新方法 `loadDefinition(definition, config, options)`；行对象仍和 `load(file)` 的一样，方法表用驱动交回的定义声明（`spec.provides`），`unload()` 走驱动的 `unloadDefinition`。放弃：另加一个 `loadDefinition()` 函数——R10 要求直接交给 `load`。 |
| D3 | **运行时按注册名解析，宿主照旧装成一行。** 新增 `node/rutis-runtime/src/testing/definitions.mjs`（Bun：`bun/rutis-bun/src/testing/definitions.ts`）：一张 `名字 → 定义` 的表，`register`/`take`/`release`。`runner.mjs` 解析行入口的几处先查这张表——`located()`（`:452` 那一带）、`pluginOf()`（`:461`）、`declaredOf()`（`:526`）、`describe()`（`:739`，`rows.schema` 走它）、`realFile()`（`unhandled.mjs:137`，命中时没有文件）；Bun 的对应处是 `bun/rutis-bun/src/plugin.ts` 的 `locate()` 与 `runner.ts:454` 的 `describe()`。驱动（`testing/rutis-test.mjs`，Bun `testing/rutis-test.ts`）的两个方法：<br>· `loadDefinition(definition, config, options)`：确认这是本运行时交出过的值（不是就拒绝，§五 第 1 条），取一个注册名 `rutis-test/definition-<n>`，`register`，然后 `harness.load(名字, config, options)`（宿主完全照旧：配置按 schema 校验并补默认值、行 id、`isolate`、服务的公告、`status`、`reported`）；失败时 `release` 并把宿主的话原样抛出。交回 `{ id, provides }`。<br>· `unloadDefinition(id)`：`harness.unload(id)`，然后 `release` 那条注册。<br>放弃：让宿主/loader 直接收定义（`NewEntry` 收定义、`PluginRef` 参数、`Owner` 变体、loader 的副本行与 `reconcile` 规则）——要改 Rust，给 0.20 加不兼容，三轮评审的量。放弃：让驱动自己 `ctx.plugin(定义, config)`——叶子插件的 `ctx` 没有 `ctx.plugin`（`node/rutis-runtime/src/leaf.mjs`），要么给驱动开一个私有挂载口，要么把驱动改成 Cordis 插件；而且这样不是行：`status`、`provided`、`reported`、`isolate`、`update`、宿主的 JSON-Schema 配置校验与默认值全都没有（`cordis-config.mjs:10` 只管 Cordis 的 `Config`）。 |
| D4 | **行 id 与注册名分开。** 注册名只给运行时解析用（`rutis-test/definition-<n>`，不出现在开发者面前）。行 id 沿用 `load(file)` 的规则：`options.id`，否则定义的名字（`spec.name`，没有就用注册名的尾巴），被占用时宿主照旧加 `-2`、`-3`……所以 `t.row` 是真行 id，`rutis-host status` 里的 `rows:` 也是它。放弃：让测试自己起 id——和 `load(file)` 的规则分叉。 |
| D5 | **撤下与清理。** 测试 `unload()`：`unloadDefinition(id)` → 行卸载、注册表那条删掉。测试结束：测试工具卸掉它没卸的行（现有的 `testEnded`，`node/rutis/src/testing.mjs:113`），每条都 `unloadDefinition`。驱动卸载、宿主停、测试进程死：运行时结束，行与注册表一起没（`rutis-test.mjs:126–130`）。放弃：让运行时在行卸载时自己清注册表——驱动不知道哪条对应哪行，反过来多一份记录；`unloadDefinition` 一个地方做干净。 |
| D6 | **不做的事：** 让运行时在来源行结束时连带卸掉这一行（见 §四 第 1 条）；`ended()` 与 R4（"只能挂在来源实例里或更里面"，见 §四 第 3 条）；`rutis.json` 里写插件定义、`rutis/loader.create` 收定义；`rutis-host check` 认识运行期才拿到的定义（R8 不变）；Python/Go/Swift/Kotlin 的测试工具（它们本来就这么做）；把定义装进实例（测试的行在根上，隔离用 `isolate` 选项）。 |

## 三、规则

| 规则 | 行为 |
| --- | --- |
| 交出 | 插件把 `definePlugin` 的结果交给测试的服务；运行时按 `plugin` 引用发出（R10 §五.1），测试进程的会话只在声明了 `plugins` 时收下（D1）。 |
| 交回 | 测试把那个不透明值交给 `load` → 驱动 → 宿主。值到驱动手里还原成运行时里原来的 `definePlugin` 结果（`home: true`）。 |
| 装载 | 驱动按注册名登记，再走宿主现有的 `load`：配置按定义声明的 schema 校验并补默认值，`inject` 里的服务没到时等待，`provides` 按行公告，`status`/`provided`/`reported`/`on`/`isolate`/`update` 与 `load(file)` 完全相同。 |
| 行 id | `options.id`，否则定义的名字，被占用时 `-2`、`-3`……（宿主现有的规则）。 |
| 撤下 | `unload()` → `unloadDefinition(id)`：行卸载，注册表那条删掉。 |
| 测试结束 | 没卸的行由测试工具卸掉（现有行为，不打印）。 |
| 运行时没了 | 行与注册表随运行时一起没；之后的调用与现有行一样失败。 |
| 同一个值两次 | 两次 `load` 是两行，各自的 id、各自的服务公告，互不影响（§四 第 2 条）。 |
| 不是替身收到的值 | 驱动拒绝：`load needs a plugin definition a test double received`（§五 第 1 条）。 |

## 四、与真实宿主的差异

R10 说"替身收到的定义可以直接交给测试工具的 `load`，和真实运行时一致"。这里的一致是：**定义在定义它的运行时里，作为一行装载**，走的就是 `load(file)` 那条路（配置校验与默认值、`inject` 门控、服务公告、卸载），而不是"绕开宿主在测试进程里挂一个假的"。三处与插件代码里 `ctx.plugin(def.at(&ctx, config)?)` 挂出的副本不同：

1. **来源行结束不带走这一行。** 副本的 R6（随交出它的那一行结束）只约束插件用 `ctx.plugin(def.at(..))` 挂出的那些：它们属于来源行的这一代。这里的一份是**宿主上的一行**，卸载由测试（或测试结束时的测试工具）做。Dim 的写法先卸 Session、再卸 owner，与这条一致；反过来先卸 owner 时，Session 那一行会继续跑，直到测试卸它。
2. **同一个定义 `load` 两次是两行。** 两份各自有 id、各自公告服务，和 `ctx.plugin(def.at(..))` 挂两次是两份副本同理，只是身份是行 id。
3. **`ended()` 与 R4 不适用。** 它们是内核 `Definition` 对副本的约定；测试拿到的是一个值，`load` 之后是一行——没有"定义结束"，也没有"只能挂在来源实例里或更里面"（测试的行在根上，需要隔离就用 `isolate`）。

为什么这与 R10 的承诺一致：R10 说的"装上"就是"作为一行装载"，测试工具的 `load` 从来是行（`load(file)` 返回的就是一行）；Python/Go/Swift/Kotlin 的测试工具对替身收到的定义做的也是这件事（Python 的 `sys.modules` + `harness.load`，见 §一）。R6 不因此被放宽：它约束的是插件挂副本，本稿没有改它，也没有让测试工具假装挂的是内核副本。

## 五、开发者看到什么

### 5.1 写测试的插件作者（TypeScript；Bun 只有运行时包名不同）

**正常时。**

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

测试通过时没有输出。`parts[0]` 就是插件 `apply` 里造的那个定义；`session` 是宿主上的一行，`session.row` 是它的 id，`provided()`、`reported()`、`on`、`status` 与 `load(file)` 的行相同。

**出错时。** 每条一行：怎样触发、逐字输出、在哪条语句上抛、怎样改。

| 触发 | 输出（逐字） | 位置 | 怎样改 |
| --- | --- | --- | --- |
| 交给 `load` 的不是替身收到的值（自己拼的、或从别的运行时来的） | `PluginTestError: load needs a plugin definition a test double received` | `load` 抛出的错误 | 用替身收到、原样交回的那个值 |
| 定义声明的配置 schema 不接受这份配置 | `PluginTestError: SchedulerSession: config: depth should be a number, got "deep"` | `load` 抛出的错误 | 按定义声明的 schema 改配置 |
| 测试结束时没卸的行 | 没有输出：测试工具把它卸掉（`testEnded`） | — | 想自己控制就在测试里 `unload()` |
| 卸下之后再用这个行对象 | `PluginTestError: the plugin is unloaded` | 该调用抛出的错误 | 重新 `load` |
| `RUTIS_HOST` 指向比测试工具旧的 `rutis-host`（0.19 及更早） | `PluginTestError: the test driver has no method loadDefinition` | `load` 抛出的错误 | 换成同版本的 `@arcships/rutis-host`，或把 `RUTIS_HOST` 指向新版的二进制 |

测试手里的定义不会失效：值在测试进程里，运行时不能释放它（只有值被回收时才释放），所以没有"定义已失效"这一条。

**要懂的概念。** 没有新增：R10 已经要求"替身收到的定义可以直接交给 `load`"。测试作者仍只需知道"它作为一行跑在定义它的运行时里，随测试卸载"。

### 5.2 被测插件的作者

不需要改任何东西：照旧 `await ctx.use('parts').registerDefinition('session', definePlugin({ … }))`；交出的就是它在 `apply` 里造的那个定义。区别只有测试工具那一侧怎么装它（§四）。

### 5.3 宿主持有者（嵌入 rutis 的应用）

不需要改任何东西：`rutis-host`、`rutis-loader`、`rutis-bridge`、Rust 一行都没动。`rutis-host status` 里多出的一行是普通行（`SchedulerSession: running`），`rutis-host check` 的输出不变（运行期才拿到的定义它本来就不认识，R8）。

### 5.4 部署的人

看不到变化：协议没有新帧，`rutis.json` 没有新字段。

## 六、兼容与迁移

- **不兼容：无。** `rutis-host`、loader、bridge、Rust 都没动，协议、`rutis.json`、CLI 输出都不变；没有迁移指南，也没有 `breaking` 标签。
- **新增（行为扩展）：** `@arcships/rutis/testing` 的 `load` 多认一种值（替身收到的定义）；TS 声明不变（`Definition` 早已写成 TS 的插件类型，R9）。测试侧的会话多声明一个能力 `plugins`（R10 §五.3 已经定义的能力）；不声明的那一端行为不变。
- **版本：** 旧的运行时包（`@arcships/rutis-runtime` / `@arcships/rutis-bun`）不认 `loadDefinition`，`load` 报 §5.1 的最后一条；测试工具与运行时包要同版本（今天就是 dev dependency）。

## 七、实施

**PR1：测试侧声明并收下定义（D1）。**
`node/rutis-runtime/src/testing/start.mjs`（`host` 选项）、`src/session.mjs`（收下、交回）；Bun 的 `src/testing/start.ts`、`src/session.ts`。
完成条件：测试给的服务收到 `register(定义)`，替身把它原样交回时插件拿到的是它自己那个定义；`node/rutis/test` 与 `bun/rutis-bun/test` 各一个用例。**PR1 不关闭 #1004**。

**PR2：按 D2–D5 让 `load` 装上定义（合并时 `Closes #1004`）。**
`node/rutis-runtime/src/testing/definitions.mjs`（新）、`src/runner.mjs`（`located`/`pluginOf`/`declaredOf`/`describe`）、`src/unhandled.mjs`（`realFile`）、`src/testing/rutis-test.mjs`（两个方法）、`node/rutis/src/testing.mjs`（`load` 认值、方法表、`unloadDefinition`）；Bun 的同一份（`src/testing/definitions.ts`、`src/plugin.ts` 的 `locate`、`src/runner.ts` 的 `describe`、`src/testing/rutis-test.ts`）。

Bun 与 Node 共用与各写一份：

| | 文件 |
| --- | --- |
| 共用一份 | `node/rutis/src/testing.mjs`（SDK；Bun 下 `bun test` 跑的还是它，运行时包名按 `typeof Bun` 选） |
| 各写一份（同一逻辑的两份代码） | 运行时包：`node/rutis-runtime`（`src/session.mjs`、`src/testing/start.mjs`、`src/testing/rutis-test.mjs`、`src/runner.mjs`、`src/unhandled.mjs`、新增 `src/testing/definitions.mjs`）与 `bun/rutis-bun`（`src/session.ts`、`src/testing/start.ts`、`src/testing/rutis-test.ts`、`src/plugin.ts`、`src/runner.ts`、新增 `src/testing/definitions.ts`） |
| 两份用例、同一份断言 | `node/rutis/test`、`bun/rutis-bun/test`：同一个用例名、同一段断言（现有约定） |

完成条件（验收）：

1. Dim 的 `runtime/plugins/scheduler/scheduler.test.mjs:174` 那种写法跑通：替身接住定义、`load(definition, { services })` 装上 Session、`service()` 调得到、`unload()` 干净。
2. 定义声明的 `provides` 里的同步方法仍按同步调用（方法表来自驱动交回的声明）。
3. 配置不合 schema 报 §5.1 第 2 条；不是替身收到的值报第 1 条；同一个值 `load` 两次得到两行、各自的 `provided()`。
4. 测试没卸的行在测试结束时被卸掉，运行时里没有残留（`rows` 为空）。
5. `status`（`rutis-host status`）里这一行与 `load(file)` 的行一样出现、一样消失。
6. 文档：`docs/guide/plugin-api.md`/`.en.md` 的"替身收到的定义可以直接装上"加上 Node 与 Bun；plugin values 设计稿 R10 那一行加一句指向本文 §四；发布说明各一条（`load` 收替身收到的定义）。
