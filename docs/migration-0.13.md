# 迁移到 0.13

[English](migration-0.13.en.md)

0.13 统一了事件（[设计](design-events-2026-10-18.md)、[协议规格](design-events-protocol-2026-10-18.md)）：一个逻辑总线，五种方式处处同义。不保留旧行为。本页按部分列出变化。

## Rust 内核（`rutis`）

**事件类型是 serde 的。** `Event` 现在要求 `Serialize + DeserializeOwned`，`Event::Value` 也一样：

```rust
#[derive(Serialize, Deserialize)]          // 新增
struct Joined { room: String }
impl Event for Joined { const NAME: &'static str = "room/joined"; type Value = String; }
```

不能 serde 的值（句柄、通道）不该放进事件：改用服务。

**监听器按名字查找。** `EventKey::<E>::of()` 的名字是 `E::NAME`（以前 `NAME` 只用于诊断，`of()` 是单独的通道）。`NAME` 相同的两个类型现在共享监听器，经 serde 转换：无关的类型请取不同的名字。`named` / `dynamic` 不变。

**一张监听器表。** 一个监听器登记一次，任何方式都能调用它；由发出方选择方式。

| 以前 | 现在 |
| --- | --- |
| `on(ctx, key, \|ctx, e\| ..)` | `on(ctx, key, \|ctx, e, call: Call<'_, V>\| ..)`；`call.mode()`、`call.next()`（仅 waterfall） |
| `on_waterfall(ctx, key, \|ctx, e, next\| next.call())` | `on(ctx, key, \|ctx, e, call\| call.next())`；值为 `Option<V>` |
| `SyncEvent` 的 `on_sync` / `on_waterfall_sync` | 任何事件都可 `on_sync(ctx, key, \|ctx, e, call: SyncCall<'_, V>\| ..)`；`SyncEvent` 删除 |
| `on_pattern(ctx, EventPattern::any_prefix([..]), ..)` | `on_pattern(ctx, &EventPattern::prefix(p), \|ctx, key, e, call\| ..)`，每次登记一个前缀 |
| `*_keyed(name, ..)`、`*_instance(id, ..)`、`EventKey::instance(id)` | `EventKey::named` / `dynamic`；实例通道改用作用域（见下） |
| `serial(ctx, key, &e)` | `serial(ctx, key, Arc<E>)` |
| `waterfall(..) -> Result<V>`，`Terminal` 返回 `V` | `-> Result<Option<V>>`，`Terminal` 返回 `Option<V>`；`()` 表示没有终态 |
| `waterfall_sync(.., \|ctx, e\| Ok(v))` | `\|ctx, e\| Ok(Some(v))` |

**行为变化。**

- 监听器以登记它的 `Ctx` 被调用，不再是发出方的。
- `emit` 按登记顺序开始每个监听器，不等任何一个（以前逐个执行）；同一名字的多次 emit 仍按发出顺序开始。失败以 `CordisError::ListenerFailed` 报给错误汇集，从不中断其他监听器。
- `parallel` 的失败总是 `CordisError::ListenersFailed`（AggregateError），按登记顺序，只有一个失败也如此（以前单个错误原样、多个错误按完成顺序 `Aggregate`）。
- `serial` / `bail_sync` 中 `Some(false)` 是没有结果。
- `bail_sync` / `waterfall_sync` 遇到异步（`on`）监听器以 `AsyncListener` 失败。
- 插件开始卸载后，它的每个监听器都不再被调用，卸载等它们正在运行的调用结束（其 `ctx.cancelled()` 已触发）；以前只有实例、模式与同步监听器如此。
- `rutis/*` 是保留名字：发出它以 `ReservedEvent` 失败。

**作用域取代实例通道。** 加载器对每个组实例调用 `Ctx::mark_scope(group)`；`Ctx::scope()` 是从根起的链。监听器收到自己作用域及其下各作用域发出的事件。要进一步挑选监听器，`ctx.events().filter(|info| ..)` 给出按每个监听器的 `ListenerInfo` 过滤的同样的发出方法。

**诊断。** `EventSubscription`、`EventBacklog` 与 `DispatchAttempt` 带事件的 `name`，不再带 `TypeKey`；`DispatchMode` 改为 `rutis::Mode`；`ReentrantEvent` 带 `name`。
