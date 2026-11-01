# 测试工具：`load` 装接受测插件交出的插件定义

[English](design-test-tool-definition-load-2026-11-01.en.md)

状态：定稿，已实现（#1004 的实现 PR，§七）。日期：2026-11-01（同日第三稿：路线 C1——把定义装成宿主上的一行，和 Python/Go/Swift/Kotlin 的测试工具同一条路；删去来源绑定、`PluginRef`、`OriginResolver`、loader 的副本行与 `reconcile` 规则、`Owner` 新变体。本轮按复审四条改准：注册名与默认行 id 按 `crates/rutis-host/src/test.rs` 的实际算法写；旧运行时/旧 SDK/旧宿主分阶段给出逐字错误；身份校验写明是会话自己的引用表，公开 Symbol 只作 SDK 的路由标记；验收补齐配置默认值、门控、监听者清理、失败时释放、两次各自卸载、来源先卸的那一条。本稿不碰 Rust，无不兼容改动。实现时同日再修订：§5.1 第 4 条按实测改——本进程造的 `definePlugin` 结果交给**插件的服务**时，是发出那一侧报"对端没有声明 `plugins`"，`load` 收到它时仍按它所在的模块装载（和以前一样，不是错误）；§七 改成一个 PR。）

Issue：[#1004](https://github.com/arcships/rutis/issues/1004)（TypeScript 测试工具：测试进程收不到插件定义）。按本稿实施的是 #1004 的实现 PR（一个 PR，§七）。上游规则见[插件是一个值](design-plugin-values-2026-10-25.md)的 R10 与 §五.5；本文 §四 写明与真实宿主的差异，R10 处加了指向本节的一句。

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
| D1 | **测试侧的会话收下定义。** 会话新增选项 `host`（只有测试工具用，`node/rutis-runtime/src/testing/start.mjs` 与 Bun 的 `testing/start.ts`）：声明 `plugins`；`session.mjs`（Bun `session.ts:443`）收到对端发来的 `plugin` 引用时不再 `#refuse`，而是留一条 import 记录（和函数、对象引用同一套 `#imports`/`#proxies`/`FinalizationRegistry` 生命周期），交给测试一个不透明的句柄。原样交回任何服务时按 `home: true` 编码，对方拿到的是它自己那个值（`session.mjs:447`）。句柄上的标记 `Symbol.for('rutis.plugin-definition')` **只给 SDK 认路用**，不作校验（见 D3 第 2 条）。放弃：所有会话默认收下——运行时收到别的运行时的定义本来就用不了，别的会话维持现在的拒绝，错误才说得准。放弃：给句柄加 `apply`，让测试进程也能挂——副本要在定义它的运行时里跑，本地挂是假的。 |
| D2 | **`load` 认这个句柄。** `@arcships/rutis/testing` 的 `load()` 与 `host().load()` 除了 `definePlugin` 的结果，也认 D1 的句柄：带那个标记就不走"模块默认导出"和插件 API 的检查，调驱动的新方法 `loadDefinition(handle, config, options)`；行对象仍和 `load(file)` 的一样，方法表用驱动交回的定义声明（`spec.provides`），`unload()` 走驱动的 `unloadDefinition`。放弃：另加一个 `loadDefinition()` 函数——R10 要求直接交给 `load`。 |
| D3 | **运行时按注册名解析，宿主照旧装成一行；身份由会话的引用表保证。** <br>1. **注册名**（运行时内部，不面向开发者）：新增 `node/rutis-runtime/src/definitions.mjs`（Bun：`bun/rutis-bun/src/definitions.ts`），`register`/`take`/`release`（放在 `src/` 顶层而不是 `src/testing/`：`crates/rutis-bridge/tests/bun_runtime.rs:465–470` 复制运行时包时只拷 `src/` 下的文件，因为 `main.ts` 不 import 测试工具那个目录，`runner` 从那里 import 会让那份拷贝起不来）；名字取 `rutis-test/definition-<n>/<定义的名字>`，`<定义的名字>` 是 `spec.name`，没有时用 `plugin`。尾段是定义的名字，是为了让宿主的 `base_id`（`crates/rutis-host/src/test.rs:393–401`，取最后一段、去掉扩展名）算出默认行 id（D4）。`runner.mjs` 解析行入口的几处先查这张表——`located()`（`:452` 那一带）、`pluginOf()`（`:461`）、`declaredOf()`（`:526`）、`describe()`（`:739`，`rows.schema` 走它）、`realFile()`（`unhandled.mjs:137`，命中时没有文件）；Bun 的对应处是 `bun/rutis-bun/src/plugin.ts` 的 `locate()` 与 `runner.ts:454` 的 `describe()`。<br>2. **身份**：句柄是**测试侧会话自己造的**——它在 `#imports`/`#proxies` 里记着"这个 id 是运行时发来的 `plugin` 引用"，交回时按 `home: true` 发；运行时那一侧只有**自己发出的**引用能按 `home: true` 解回（`session.mjs:509` 的 `#export`），别的 id 一律"unknown, released or mismatched reference"。公开 Symbol 只是给 SDK 认路的标记，谁都造得出来，所以它不参与校验；校验全在这两张会话私有的表上。交回一个不是本会话收到过的值有两条路：<br>· 那是测试进程里造的 `definePlugin` 结果 → 测试侧会话把它当作**新导出**发给驱动，驱动那侧的会话按 `session.mjs:451` 拒绝：`received a plugin definition made in another process, which cannot be used here; pass plain data or a function instead`；<br>· 那是伪造的句柄（带标记的普通对象、函数）→ 作为对象/函数引用到达，驱动用运行时的 `isLeaf` 一查就知道不是定义，拒绝：`load needs a plugin definition a test double received`。<br>两条都以 `PluginTestError` 抛出（SDK 的 `host.load` 把驱动的错误包成它），没有单独的错误码。<br>3. **驱动**（`testing/rutis-test.mjs`，Bun `testing/rutis-test.ts`）：`loadDefinition(handle, config, options)` 确认是定义、取注册名、`register`、`harness.load(名字, config, options)`（宿主完全照旧），失败时 `release` 并把宿主的话原样抛出，成功交回 `{ id, provides }`；`unloadDefinition(id)` 先 `harness.unload(id)` 再 `release`。<br>放弃：让宿主/loader 直接收定义（`NewEntry` 收定义、`PluginRef` 参数、`Owner` 变体、loader 的副本行与 `reconcile` 规则）——要改 Rust，给 0.20 加不兼容。放弃：让驱动自己 `ctx.plugin(定义, config)`——叶子插件的 `ctx` 没有 `ctx.plugin`（`node/rutis-runtime/src/leaf.mjs`），而且那不是行：`status`、`provided`、`reported`、`isolate`、`update`、宿主的 JSON-Schema 配置校验与默认值都没有（`cordis-config.mjs:10` 只管 Cordis 的 `Config`）。 |
| D4 | **行 id 用宿主现有的算法。** 注册名只给运行时解析用。行 id 全交给 `harness.load`（`crates/rutis-host/src/test.rs:335–345、378–401`）：<br>· **没给 `id`**：`free_id` 取注册名的**最后一段、去掉扩展名**——也就是定义的名字（D3 的注册名就是为了这个）——被占用时加 `-2`、`-3`……；<br>· **给了 `id`**（`load(handle, { id })`）：原样使用，撞上已有的行/副本时**直接失败**，文字是 `a row <id> is loaded already`，**不会**自动加后缀——和 `load(file, { id })` 今天一样；<br>· 定义的名字里有 `/` 或 `.` 时按 `base_id` 的规则取尾段、去扩展名，和 `load(file)` 一样。<br>所以 `load` 一次不指定 id 得到 `SchedulerSession`，同一个定义再 `load` 一次得到 `SchedulerSession-2`。断言就按这两条写（§七 验收 3）。放弃：让测试自己起 id——和 `load(file)` 的规则分叉；放弃：驱动自己算后缀——宿主已经算了。 |
| D5 | **撤下与清理。** 测试 `unload()`：`unloadDefinition(id)` → 行卸载、注册表那条删掉。测试结束：测试工具卸掉它没卸的行（现有的 `testEnded`，`node/rutis/src/testing.mjs:113`），每条都 `unloadDefinition`。驱动卸载、宿主停、测试进程死：运行时结束，行与注册表一起没（`rutis-test.mjs:126–130`）。放弃：让运行时在行卸载时自己清注册表——驱动不知道哪条对应哪行，反过来多一份记录；`unloadDefinition` 一个地方做干净。 |
| D6 | **不做的事：** 让运行时在来源行结束时连带卸掉这一行（见 §四 第 1 条）；`ended()` 与 R4（"只能挂在来源实例里或更里面"，见 §四 第 3 条）；`rutis.json` 里写插件定义、`rutis/loader.create` 收定义；`rutis-host check` 认识运行期才拿到的定义（R8 不变）；Python/Go/Swift/Kotlin 的测试工具（它们本来就这么做）；把定义装进实例（测试的行在根上，隔离用 `isolate` 选项）。 |

## 三、规则

| 规则 | 行为 |
| --- | --- |
| 交出 | 插件把 `definePlugin` 的结果交给测试的服务；运行时按 `plugin` 引用发出（R10 §五.1），测试进程的会话只在声明了 `plugins` 时收下（D1）。 |
| 交回 | 测试把句柄交给 `load` → 驱动 → 宿主。到驱动手里还原成运行时里原来的 `definePlugin` 结果（`home: true`）。伪造或本进程造的值到不了装载（D3 第 2 条）。 |
| 装载 | 驱动按注册名登记，再走宿主现有的 `load`：配置按定义声明的 schema 校验并补默认值，`inject` 里的服务没到时等待，`provides` 按行公告，`status`/`provided`/`reported`/`on`/`isolate`/`update` 与 `load(file)` 完全相同。 |
| 行 id | 没给 `id`：定义的名字，被占用时 `-2`、`-3`……；给了 `id`：原样用，撞上就失败（`a row <id> is loaded already`）（D4）。 |
| 撤下 | `unload()` → `unloadDefinition(id)`：行卸载（监听者随行撤掉），注册表那条删掉。 |
| 装载失败 | 注册表那条当场 `release`，再抛宿主的话；运行时里不留行。 |
| 测试结束 | 没卸的行由测试工具卸掉（现有行为，不打印）。 |
| 运行时没了 | 行与注册表随运行时一起没；之后的调用与现有行一样失败。 |
| 同一个值两次 | 两次 `load` 是两行，各自的 id、各自的服务公告，各自卸载互不影响（§四 第 2 条）。 |

## 四、与真实宿主的差异

R10 说"替身收到的定义可以直接交给测试工具的 `load`，和真实运行时一致"。这里的一致是：**定义在定义它的运行时里，作为一行装载**，走的就是 `load(file)` 那条路（配置校验与默认值、`inject` 门控、服务公告、监听者随行、卸载），而不是"绕开宿主在测试进程里挂一个假的"。三处与插件代码里 `ctx.plugin(def.at(&ctx, config)?)` 挂出的副本不同：

1. **来源行结束不带走这一行。** 副本的 R6（随交出它的那一行结束）只约束插件用 `ctx.plugin(def.at(..))` 挂出的那些：它们属于来源行的这一代。这里的一份是**宿主上的一行**，卸载由测试（或测试结束时的测试工具）做。Dim 的写法先卸 Session、再卸 owner，与这条一致；反过来先卸 owner 时，Session 那一行会继续跑，直到测试卸它（§七 验收 8 守着这一条）。
2. **同一个定义 `load` 两次是两行。** 两份各自有 id、各自公告服务，和 `ctx.plugin(def.at(..))` 挂两次是两份副本同理，只是身份是行 id。
3. **`ended()` 与 R4 不适用。** 它们是内核 `Definition` 对副本的约定；测试拿到的是一个句柄，`load` 之后是一行——没有"定义结束"，也没有"只能挂在来源实例里或更里面"（测试的行在根上，需要隔离就用 `isolate`）。

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

测试通过时没有输出。`parts[0]` 就是插件 `apply` 里造的那个定义；`session` 是宿主上的一行，`session.row` 是它的 id（定义的名字，重名时 `-2`），`provided()`、`reported()`、`on`、`status` 与 `load(file)` 的行相同。

**出错时。** 每条一行：怎样触发、逐字输出、在哪条语句上抛、怎样改。版本不一致的三种情形分在**不同的阶段**失败，都列在下面。

| 触发 | 输出（逐字） | 位置 | 怎样改 |
| --- | --- | --- | --- |
| 旧的运行时包（`@arcships/rutis-runtime` / `@arcships/rutis-bun` 0.19）：测试侧的会话没声明 `plugins` | `registrar: TypeError: parts.register: argument 1 cannot be sent: cannot send a plugin definition: the far end does not accept plugin definitions (it did not declare the capability plugins)` | 插件交出定义的那次调用（到不了 `load`） | 把运行时包升到与 SDK 同版本 |
| 旧的 SDK（`@arcships/rutis` 0.19）配新运行时：句柄有了，旧 `load` 不认 | `PluginTestError: load needs a plugin made with definePlugin` | `load` 抛出的错误 | 把 `@arcships/rutis` 升到同版本 |
| 宿主比 0.19 还旧（没有 `rutis-host test`）：起不来 | `<host> exited (<code>) before the test could start`（或找不到宿主时 `<host> not found: install @arcships/rutis-host (a dev dependency) or set RUTIS_HOST to a rutis-host`） | `load` 第一次用到宿主时（`start.mjs:94–101`） | 换 0.19 或更新的 `rutis-host`。0.19 的宿主有 `test`，与新测试工具兼容 |
| 把一个本进程造的 `definePlugin` 结果交给**插件的服务**（不是交给 `load`） | `sink.register: argument 1 cannot be sent: cannot send a plugin definition: the far end does not accept plugin definitions (it did not declare the capability plugins)` | 那次服务调用 | 用替身收到、原样交回的那个句柄 |
| 交给 `load` 的不是替身收到的句柄：伪造的标记（带 `Symbol.for('rutis.plugin-definition')` 的普通对象、函数） | `PluginTestError: load needs a plugin definition a test double received` | `load` 抛出的错误 | 同上 |
| 定义声明的配置 schema 不接受这份配置 | `PluginTestError: SchedulerSession: config: depth should be a number, got "deep"` | `load` 抛出的错误 | 按定义声明的 schema 改配置 |
| 显式给的 `id` 已经有一行 | `PluginTestError: a row SchedulerSession is loaded already` | `load` 抛出的错误 | 换一个 id，或不给 id 让它加后缀 |
| 测试结束时没卸的行 | 没有输出：测试工具把它卸掉（`testEnded`） | — | 想自己控制就在测试里 `unload()` |
| 卸下之后再用这个行对象 | `PluginTestError: the plugin is unloaded` | 该调用抛出的错误 | 重新 `load` |

`load` 收到本进程造的 `definePlugin` 结果时按它所在的模块装载，和以前一样；那不是错误。`the test driver has no method loadDefinition` 只在运行时包半新半旧（测试侧会话新、驱动旧）时出现，正常安装不会碰到；`start.mjs:70` 用本地运行时包里的 `DRIVER`/`RUNNER` 启动，宿主的版本不决定这个方法在不在。

测试手里的句柄不会失效：值在测试进程里，运行时不能释放它（只有值被回收时才释放），所以没有"定义已失效"这一条。

**要懂的概念。** 没有新增：R10 已经要求"替身收到的定义可以直接交给 `load`"。测试作者仍只需知道"它作为一行跑在定义它的运行时里，随测试卸载"。

### 5.2 被测插件的作者

不需要改任何东西：照旧 `await ctx.use('parts').registerDefinition('session', definePlugin({ … }))`；交出的就是它在 `apply` 里造的那个定义。区别只有测试工具那一侧怎么装它（§四）。

### 5.3 宿主持有者（嵌入 rutis 的应用）

不需要改任何东西：`rutis-host`、`rutis-loader`、`rutis-bridge`、Rust 一行都没动。`rutis-host status` 里多出的一行是普通行（`SchedulerSession: running`），`rutis-host check` 的输出不变（运行期才拿到的定义它本来就不认识，R8）。

### 5.4 部署的人

看不到变化：协议没有新帧，`rutis.json` 没有新字段。

## 六、兼容与迁移

- **不兼容：无。** `rutis-host`、loader、bridge、Rust 都没动，协议、`rutis.json`、CLI 输出都不变；没有迁移指南，也没有 `breaking` 标签。
- **新增（行为扩展）：** `@arcships/rutis/testing` 的 `load` 多认一种值（替身收到的句柄）；TS 声明不变（`Definition` 早已写成 TS 的插件类型，R9）。测试侧的会话多声明一个能力 `plugins`（R10 §五.3 已经定义的能力）；不声明的那一端行为不变。
- **版本不一致时失败在哪一步**（逐字见 §5.1）：旧运行时包 → 插件**交出定义**那一步就失败（会话没声明 `plugins`），到不了 `load`；旧 SDK 配新运行时 → 句柄能拿到，旧 `load` 不认它（`load needs a plugin made with definePlugin`）；宿主比 0.19 还旧 → 起不来（`start.mjs:94–101` 的两句）；只有运行时包半新半旧（测试侧新、驱动旧）才可能碰到 `the test driver has no method loadDefinition`。测试工具、SDK 与运行时包同版本是今天就在的要求（都是 dev dependency）。

## 七、实施

**一个 PR（合并时 `Closes #1004`）。** 中间状态（能收下但装不上）对使用者没有价值，所以不拆。

改动：

- 测试侧：`node/rutis-runtime/src/testing/start.mjs`（`host` 选项）、`src/session.mjs`（声明 `plugins`；收下 `plugin` 引用成一个句柄；交回时按 `home: true`）；Bun 的 `src/testing/start.ts`、`src/session.ts`。
- 运行时：新增 `node/rutis-runtime/src/definitions.mjs`（注册表）；`src/runner.mjs`（`located`/`pluginOf`/`declaredOf`/`describe`）、`src/unhandled.mjs`（`realFile`）先查注册表；Bun 的同一份（新增 `src/definitions.ts`、`src/plugin.ts` 的 `locate`、`src/runner.ts` 的 `#plugin`/`describe`/`rowOfFiles`）。
- 驱动：`node/rutis-runtime/src/testing/rutis-test.mjs` 的 `loadDefinition`/`unloadDefinition`；Bun 的 `src/testing/rutis-test.ts`。
- SDK：`node/rutis/src/testing.mjs`（`load` 认句柄、方法表用驱动交回的定义声明、`unload` 走 `unloadDefinition`；`load()` 也接受 `id`/`isolate`）。

Bun 与 Node 共用与各写一份：

| | 文件 |
| --- | --- |
| 共用一份 | `node/rutis/src/testing.mjs`（SDK；Bun 下 `bun test` 跑的还是它，运行时包名按 `typeof Bun` 选） |
| 各写一份（同一逻辑的两份代码） | 运行时包：`node/rutis-runtime`（`src/session.mjs`、`src/testing/start.mjs`、`src/testing/rutis-test.mjs`、`src/runner.mjs`、`src/unhandled.mjs`、新增 `src/definitions.mjs`）与 `bun/rutis-bun`（`src/session.ts`、`src/testing/start.ts`、`src/testing/rutis-test.ts`、`src/plugin.ts`、`src/runner.ts`、新增 `src/definitions.ts`） |
| 两份用例、同一份断言 | `node/rutis/test`、`bun/rutis-bun/test`：同一个用例名、同一段断言（现有约定） |

完成条件（验收）：

1. **Dim 的写法**：`runtime/plugins/scheduler/scheduler.test.mjs:174` 那种写法跑通——替身接住定义、`load(definition, { services })` 装上 Session、`service()` 调得到、`unload()` 干净。
2. **配置**：定义声明了 `config` schema（带默认值）时，不给配置也补上默认值；给了不合的配置报 §5.1 那条（用 `PluginTestError` 包住宿主的话）。
3. **行 id**：不给 `id` 时是定义的名字，同一个定义第二次 `load` 是 `<名字>-2`，两份各自 `unload()` 互不影响；显式 `id` 撞上时报 `a row <id> is loaded already`。
4. **门控**：定义 `inject` 的服务没提供时这一行等待（`status` 里是等待），`services` 给了之后启动；和 `load(file)` 同一个表现。
5. **监听者**：定义的监听者随这一行卸载撤掉——卸载后测试用同一个名字 emit，它不再收到；行重装一次再 emit，只收到一份。
6. **失败释放**：装载失败（配置不合、名字被占）时注册表那条当场释放，之后同一个定义再 `load` 仍能按名字装上（不会因为残留的注册名解析到旧条目）。
7. **测试结束**：测试没卸的行在测试结束时被卸掉，运行时里没有残留（`rows` 为空），注册表也空。
8. **§四 的差异有用例守着**：先卸来源行、再卸这一行——这一行仍在跑（`service()` 还能调），直到测试卸它；同一份定义 `load` 两次是两行、各自的 `provided()`。
9. **文档**：`docs/guide/plugin-api.md`/`.en.md` 的"替身收到的定义可以直接装上"加上 Node 与 Bun；plugin values 设计稿 R10 那一行加一句指向本文 §四；发布说明各一条（`load` 收替身收到的句柄）。
