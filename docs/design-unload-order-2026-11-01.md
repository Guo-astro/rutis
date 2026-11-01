# 卸载顺序：撤回与清理同一个逆序栈

[English](design-unload-order-2026-11-01.en.md)

状态：实现中（分支 `claude/unload-lifo`，未合并）。日期：2026-11-01。基准：[声明集合](design-declared-services-2026-10-29.md) §七 记的后续，叠在[运行时行的行 inject](design-row-inject-gate-2026-10-31.md) 之上。§一 描述的代码是本稿所在提交的父提交里的。
Issue：尚无，随本稿开。相关：[声明集合](design-declared-services-2026-10-29.md) §三 第 6 条、§七；[事件协议](design-events-protocol-2026-10-18.md) §7.1；`core/D7`、`core/D14`（`docs/guarantees.md`）。

## 一、现状

一行卸载时，它提供的服务什么时候从这一行的进程里消失、它的清理还能不能读到这些服务，六种语言给出两种结果。

Rust 的做法是把撤回当成一条 effect：

```
Ctx::provide_native（crates/rutis/src/ctx.rs）
├─ ① origin = Origin::Provide(key)
├─ ② register_internal_effect(label, origin, …)     → 与 ctx.effect 进同一个 effects 列表
└─ ③ 返回 Effect::AsyncDisposer(evict_and_finalize(key))

Fiber::drain_effects（crates/rutis/src/fiber.rs）
└─ for record in effects.into_iter().rev()          → 逆序，一条一条
   ├─ 清理：运行闭包
   └─ 撤回：evict_and_finalize（消费者先停，再从表里删去）
```

于是“清理读不读得到自己提供的服务”只由**登记先后**决定：`provide` 之后登记的清理先跑，读得到；之前登记的后跑，读不到。`crates/rutis/tests/contract.rs` 的 `self_access_during_cleanup` 断言的就是前者（`Some(77)`），`docs/guarantees.md` 的 `core/D14` 把它写成“提供方最后清理，清理期间仍能访问自身服务”。

另一半在读取侧：

```
Ctx::read_binding_as（crates/rutis/src/ctx.rs）
├─ self_access = in_subtree_of(provider)            → 是不是提供方的子树
├─ 不是：provider.state() != Active 或 binding.removing → 读不到
└─ 是：不看状态，一直读到撤回真的删去这一条
```

两次 `Unloading` 与撤回因此是两件事：**别的读者在 fiber 一进入 `Unloading` 就看不见了**（早于任何清理），**提供方自己到撤回那一条跑完才看不见**。

五个 SDK 把撤回提到了所有清理之前：

| 运行时 | 撤回 | 清理 |
| --- | --- | --- |
| Go | `runtime.go` 的 `unload`：`for _, id := range r.provided` 删除、`refresh`，解锁后才取消 | 之后 `r.cleanups` 逆序 |
| Python | `runner.py` 的 `unload`：`for id in list(row.provided)` 删除 | 之后 `row.cleanups[::-1]` |
| Bun | `runner.ts` 的 `unload`：`for (const id of row.provided)` 删除 | 之后 `row.cleanups.splice(0).reverse()` |
| Kotlin | `Runtime.kt` 的 `unloadRow`：锁内 `for (id in row.provided.toList())` 删除 | 之后 `row.cleanups.asReversed()` |
| Swift | `Runtime.swift` 的 `unload`：锁内 `for id in row.provided` 删除 | 之后 `cleanups.reversed()` |

后果是：清理**一律**读不到自己提供的服务，登记在 `provide` 之后的也一样（比 Rust 严格），而别的读者在 `Unloading` 之后就看不见这一条是对的——但那是顺带的对，因为删除发生得更早。Node 是第三种：适配层自己记着 `provided` 表（`node/rutis-runtime/src/leaf.mjs`），卸载时 Cordis 的 `_unload` 用 `Promise.all` 同时开始全部处置（`@deepseek-ai/cordis` 4.0.4 的 `lib/index.js:1372`），于是清理一条也读得到，但不是逆序、也不是一条一条，Rust 明确不这样做（`fiber.rs` `drain_effects` 的注释：Cordis 并发，这里依次）。

[声明集合](design-declared-services-2026-10-29.md) §三 第 6 条记下了这三种结果，§七 说“六家都改成 Rust 那样逆序逐个运行撤回与清理……单独做”。

## 二、决策

| # | 决策 | 为什么；放弃的做法 |
| --- | --- | --- |
| D1 | 撤回是这一行卸载栈里的一条：`provide` 登记撤回，`effect` 登记清理，卸载时**同一个栈逆序、一条一条**运行。撤回在自己的位置上发生，不再统一提前 | 与 Rust 的 `drain_effects` 相同（`Origin::Provide` 与 `ctx.effect` 同一个 `effects` 列表）。可观察的结果是 `core/D14` 的“清理期间仍能访问自身服务”，`contract.rs::self_access_during_cleanup` 断言着它。放弃“清理全在撤回前”（Node 的 Cordis 语义）：那等于每次卸载都先跑全部清理再撤服务，与 Rust 的中间态不同，也和本仓内核的语义相反 |
| D2 | 别的读者在卸载开始时就看不懂这一行的服务，不看撤回那一条排在哪里 | Rust 的 `read_binding_as`：不是提供方的子树，`provider.state() != Active` 就读不到，而状态在卸载的第一步就变成 `Unloading`。SDK 里没有子树的概念，读者是不是提供方本身就是拿 `row.key` 比的，所以对应关系是“同一个行算自己，其余算别人”。放弃“让撤回那一条的位置决定别人什么时候看不见”：那样一个清理里调用的另一个行，能不能拿到服务取决于登记顺序，无法解释 |
| D3 | 撤回跑完之后，这一行自己也不再把名字算作“我提供的”：`use` 报 `not-declared` | 已有规则（[声明集合](design-declared-services-2026-10-29.md) D4）：`own_service` 只在 `fiber.provided` 还记着这个键时为真，撤回时删去。六家一致，不新增规则 |
| D4 | 载体：每一行一个栈，栈里的条目是“运行这个清理”或“撤回这个服务（带着它登记时的那一份）” | 撤回要能认出“表里现在这一条还是不是我登记的那一份”——插件可以撤回后再提供同名服务（`reentrant_provide_fails` 的反面：撤回函数被调用后重新 `provide`）。按值比较对象不足以区分两个相等的实例，所以条目里带着登记时的指针/对象身份 |
| D5 | 卸载期间再登记清理：拒绝，并写一行 `rutis: row <行> is not active: cleanup is not registered`（Node 写 `plugin <插件>`）。六家都是这一稿新加的：此前只有 Kotlin 写这一句（`Ctx.kt`，`addCleanup` 返回 `Boolean` 逼着调用方处理），Go、Python、Bun、Swift 静默 append 进一个卸载时已被取走的列表，那条清理永远不会跑也没有任何输出 | Rust 在这一刻返回 `InactiveEffect`（`crates/rutis/src/ctx.rs` 的 `ctx.effect`，`contract.rs::effect_during_cleanup_fails`）。SDK 的 `effect` 没有返回值，只能写一行。放弃“静默丢弃”：登记了却永远不跑，插件无从知道，正是前五家的现状 |
| D6 | 宿主那一侧不改：撤下宣布的服务、回复 `rows.unload` 的时机都不动 | 运行时行的宿主持有方本来就先撤投影再发 `rows.unload`（`crates/rutis-loader/src/runtime.rs` 的 `projection.withdraw().await` 在 `process.unload_row` 之前），宿主在回复前看到撤回（`process.rs` 的 `unload_row` 注释）。本稿改的是进程内的表，不影响帧的顺序 |
| D7 | Node 也一条一条逆序：适配层自己持有一个栈，用一个 Cordis effect 逆序排干 | Cordis 的 `_unload` 是 `Promise.all`，它自己的处置顺序改不了；但适配层可以把 `effect`、`provide`、`apply` 返回的清理都放进自己的栈，注册**一个** Cordis effect 排干它。监听器、`signal` 这些仍交给 Cordis，它们不与服务交互。放弃“Node 保持并发”：六家里五种一条一条、一种并发，插件作者写清理时无法预期 |

## 三、开发者看到什么

### 插件作者

一行提供 `counter`，之后登记一个清理，清理里读 `counter`：

```go
func (p *plugin) Apply(ctx *rutis.Ctx, cfg Config) error {
	ctx.Provide("counter", counter{p.inject.Listen()})
	ctx.Effect(func(c context.Context) error {
		var service *rutis.Service
		if err := ctx.Use("counter", &service); err != nil {
			return err
		}
		var n int
		return service.Call(c, "Count", &n)
	})
	return nil
}
```

正常时：卸载到这个清理，`counter` 还在，调用成功；撤回排在它后面，跑完这一行才没有 `counter`。登记在 `Provide` **之前**的清理读不到：那一刻这一行已经不把 `counter` 算作自己提供的了，报的是 `service counter is not available to this plugin, which does not declare it in Inject: add "counter" to its Inject`（`use`）或返回 `nil`（`get`）。两种顺序在六种语言里结果相同，写法不同之处只在 API 名字（`ctx.effect` / `Effect` / `effect`）。

别的行在这一行卸载期间读它：`use` 报 `service counter is not available`，`get` 返回 `nil`——从这一行开始卸载那一刻起，不是从撤回那一条跑完起。

清理里再登记清理：

| 语言 | 输出（逐字） |
| --- | --- |
| Go | `rutis: row weather is not active: cleanup is not registered` |
| Python | `rutis: row weather is not active: cleanup is not registered` |
| Bun | `rutis: row weather is not active: cleanup is not registered` |
| Kotlin | `rutis: row weather is not active: cleanup is not registered` |
| Swift | `rutis: row weather is not active: cleanup is not registered` |
| Node | `rutis: plugin weather is not active: cleanup is not registered` |

六行在这一稿之前都不存在（Kotlin 那句是它上一稿就有的），所以第一次看到它们是升级之后。Node 写“插件”不写“行”：叶插件看不到自己的行 id，写的是插件自己声明的名字（未声明时是 `leaf`）。

它不会运行：卸载跑的是开始卸载那一刻的栈。写清理时因此不要指望“清理再挂清理”，要在一个清理里做完。

### 宿主持有者

帧的顺序不变：`rows.unload` 的回复仍在这一行的全部清理跑完之后；宣布过的服务仍在回复之前撤回。卸载变慢（清理改成一条一条跑）对 `--shutdown-timeout` 的影响与之前相同：六家都是按截止时间放弃余下的清理，超时的那一句不变。

### 新增的概念

“卸载栈”：一行卸载时逆序运行的全部条目，清理与撤回混在一起。它在 `provide` / `effect` 被调用时增长，在卸载开始时被取走；取走之后再登记的不进这个栈（§三 的第三段）。

## 四、兼容

- 结果变宽松的地方：`provide` 之后登记的清理读自己提供的服务，从失败变成成功（六家）。已经成功的调用不会因此失败。
- 结果变严格的地方：Go、Python、Bun、Kotlin、Swift 里，一个清理如果在 `provide` 之前登记、却在这之前读到了自己提供的服务——现在读不到。这在本稿之前是读不到的（撤回在清理之前），所以没有变严格的调用；变严格的是 Node：它的清理以前在 `provide` 之前登记也读得到，现在读不到，与另外五家一致。
- 顺序：清理从“同时开始”（Node）或“逐条但撤回提前”（五家）变成“逆序逐条、撤回在自己的位置”。依赖清理**并行**跑（比如几个清理互相等对方）的插件会卡住；截止时间会放弃余下的清理并写出那一行。这是写明的限制，不提供开关。
- 卸载期间再登记清理：此前只有 Kotlin 写一行；Go、Python、Bun、Swift 静默丢弃，现在写一行。Node 以前交给 Cordis 的 `effect`，现在写自己的一句、不再进 Cordis 的处置表。
- `docs/guarantees.md`：新增一行（`sdk/unload-order`），`core/D14` 的断言在 SDK 一侧补上测试。

## 五、实施

一行一个栈，`provide` 与 `effect` 都往里放，卸载逆序取走并逐条运行；读取侧加“卸载中的提供方对别人不可见”。

| # | 位置 | 改动 | 用例 |
| --- | --- | --- | --- |
| 1 Go | `go/rutis/internal/runtime/runtime.go`：`row.steps`、`runtimeState.provide`、`Ctx.Effect`、`Ctx.service`、`refresh`、`unload` | 撤回闭包与清理进同一个栈；`providedService` 带一个“正在撤回”的标记：卸载的第一步打上标记并 `refresh`（宿主照旧在这一刻收到撤回），删除与 `provided` 的移除留到撤回那一条；`Ctx.service` 对不是自己的行跳过带标记的条目；`refresh` 视带标记的条目为不存在 | `internal/runtime/unload_order_test.go`：`provide` 之后登记的清理读得到，之前登记的在六种语言里报 `not-declared`；别的行在这一行卸载期间 `use` 报 `unavailable`；清理里再 `Effect` 写出那一行 |
| 2 Python | `python/rutis/rutis/runner.py`：`Row.steps`、`Runtime.provide`、`Context.effect`、`Context.use`/`get`、`Runtime.lookup`、`Runtime.unload` | 同上 | `python/rutis/tests/test_unload_order.py` |
| 3 Bun | `bun/rutis-bun/src/runner.ts`：`Row.steps`、`provide`、`effect`、`lookup`/`#find`、`unload` | 同上 | `bun/rutis-bun/test/unload-order.test.ts` |
| 4 Kotlin | `internal/Runtime.kt`：`Row.steps`、`provide`、`addCleanup`、`unloadRow`；`Ctx.kt` 的 `use` | 同上 | `jvmTest/.../UnloadOrderTest.kt` |
| 5 Swift | `API/Runtime.swift`：`Row.steps`、`provide`、`effect`、`unload`；`API/Ctx.swift` 的 `use` | 同上 | `Tests/SelfTest/UnloadOrderTests.swift` |
| 6 Node | `node/rutis-runtime/src/leaf.mjs` 的 `toCordis`：每 fiber 一个栈，最先注册**一个** Cordis effect 负责排干——Cordis 的 `_unload` 会 `Promise.all` 同时开始全部处置，这个 effect 也在其中，但它把栈逆序逐条跑完才返回 | `effect`、`provide`、`apply` 返回的清理都进栈；排干本身不改 `runner.mjs` 的 `unloadRow` | `node/rutis-runtime/test/leaf.test.mjs` 的两条 |
| 7 文档 | `docs/design-events-protocol-2026-10-18.md` §7.1 第 3、5 步、`docs/guide/*`、`docs/guarantees.md` | 第 3 步改成“卸载开始时对别人不可见，撤回在自己的位置上”；第 5 步改成“逆序运行清理与撤回” | — |

`[声明集合](design-declared-services-2026-10-29.md)` §三 第 6 条改成一句“六家都与 Rust 相同”，§七 的后续条目删去。

## 六、后续

- 分组上的 `isolate` 到达运行时行：仍见[运行时行的行 inject](design-row-inject-gate-2026-10-31.md) §六，与本稿无关。
- 清理的截止时间：六家现在都是“给余下的清理一个总预算”，本稿不改。Rust 是按条给还是按总预算，另说。
- Node 的监听器与 `signal` 仍由 Cordis 并发处置，本稿只把服务与清理排成一条一条；要让它们也进这个栈，得把监听器从 Cordis 的 `on` 换成适配层自己的表，那是另一件事。
