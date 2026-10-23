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
| `*_keyed(name, ..)`、`*_instance(id, ..)`、`EventKey::instance(id)` | `EventKey::named` / `dynamic`；事件按名字送达，实例信息用 `ListenerInfo::scope` 过滤（见下） |
| `serial(ctx, key, &e)` | `serial(ctx, key, Arc<E>)` |
| `waterfall(..) -> Result<V>`，`Terminal` 返回 `V` | `-> Result<Option<V>>`，`Terminal` 返回 `Option<V>`；`()` 表示没有终态 |
| `waterfall_sync(.., \|ctx, e\| Ok(v))` | `\|ctx, e\| Ok(Some(v))` |
| 自己实现 `Listener` / `SyncListener` / `PatternListener` / `SyncPatternListener` 的 `fn call(&self, ctx, e)`（模式监听器另有 `key`） | `call` 末尾多一个参数：`call: Call<'_, E::Value>`（同步的两个为 `SyncCall<'_, E::Value>`） |
| `WaterfallListener`、`SyncWaterfallListener`、`PatternWaterfallListener`、`SyncPatternWaterfallListener` | 删除：实现对应的 `Listener` / `SyncListener` / `PatternListener` / `SyncPatternListener`，在里面调用 `call.next()` |
| `EventKey::instance_id()` | 删除：事件键不再带实例；实例信息用 `Ctx::scope()` 作为过滤描述 |
| `EventPattern::prefixes()` | `EventPattern::as_str()`：一个模式只有一个前缀 |

**行为变化。**

- 监听器以登记它的 `Ctx` 被调用，不再是发出方的。
- `emit` 按登记顺序开始每个监听器，不等任何一个（以前逐个执行）；同一名字的多次 emit 仍按发出顺序开始。失败以 `CordisError::ListenerFailed` 报给错误汇集，从不中断其他监听器。
- `parallel` 的失败总是 `CordisError::ListenersFailed`（AggregateError），按登记顺序，只有一个失败也如此（以前单个错误原样、多个错误按完成顺序 `Aggregate`）。
- `serial` / `bail_sync` 中 `Some(false)` 是没有结果。
- `bail_sync` / `waterfall_sync` 遇到异步（`on`）监听器以 `AsyncListener` 失败。
- 插件开始卸载后，它的每个监听器都不再被调用，卸载等它们正在运行的调用结束（其 `ctx.cancelled()` 已触发）；以前只有实例、模式与同步监听器如此。
- `rutis/*` 是保留名字：发出它以 `ReservedEvent` 失败。

**作用域不再是事件送达规则。** 加载器对每个组实例调用 `Ctx::mark_scope(group)`；`Ctx::scope()` 是从根起的链，作为 `ListenerInfo` 的描述供过滤使用。事件在一个 root 的总线上按名字送达，所有监听器都收到（与 Cordis 相同）；要按实例挑监听器，用 `ctx.events().filter(|info| ..)` 按 `info.scope` 等描述筛，或用不同的事件名。默认不做实例隔离。*（0.13–0.15 按作用域送达；0.16 改为这条平总线，见[迁移到 0.16](migration-0.16.md)。）*

父级行发出、子级行监听不需要额外接线：

```rust
// 父级行（t1）：
ctx.events()
    .parallel(&ctx, &EventKey::named("session/created"), &created)
    .await?;

// 子级行（t1/s1），任意深度：
ctx.events().on(
    &ctx,
    &EventKey::named("session/created"),
    |_ctx, created: &Created| async move { Ok(()) },
)?;
```

子级的监听器收到父级的事件，兄弟实例的监听器也收到。`rutis/tests/events.rs::events_reach_every_listener_on_the_bus` 从每一侧运行这个流程。监听器按登记顺序运行；`parallel` 返回 `CordisError::ListenersFailed`，其子错误按登记顺序；`emit` 在监听器运行处报告失败并立即返回。

**诊断。** `EventSubscription` 删除 `key`（`TypeKey`）、`prefixes`、`selected`、`invoked`，改为 `name`（事件名，前缀条目为前缀）、`prefix`、`remote`（别处的监听器）、`global`、`calls`（已开始的调用数）；`owner` 改为 `Option<PluginId>`（登记它的 fiber 已不在时为 `None`）。`ListenerKind` 只有 `Sync`、`Async`、`Any`（`Any` 只来自别处的监听器）：`AsyncWaterfall`、`SyncWaterfall` 删除，按值匹配的代码要改。`EventBacklog` 与 `DispatchAttempt` 带事件的 `name`，不再带 `key`（`TypeKey`）；`DispatchMode` 改为 `rutis::Mode`；`CordisError::ReentrantEvent` 删除：同名的同步分发可以重入（与 Cordis 相同），只有真正的等待环以 `SyncWaitCycle` 失败。

**没有逐个类型的选择。** 每个事件类型都能跨进程，不需要也没有 `WIRE` 之类的开关（只在方案草稿里出现过，没有发布）。

## 先升级哪些

- **所有端一起升级到 0.13**：rutis-host（或嵌入的 `rutis-bridge` / `rutis-loader`）、每个运行时包（`@arcships/rutis-runtime`、`@arcships/rutis-bun`、Python 的 `rutis`、Go 模块 `github.com/arcships/rutis/go/rutis`）和每个相连的节点。事件协议没有旧路径：0.12 及更早的运行时或节点连进 0.13 的宿主时，在加载任何东西之前就被拒绝，信息写明升级哪一端：

  ```text
  incompatible session: the far end (<name> <version>) does not declare events-v2 (the event protocol of rutis 0.13): upgrade it to rutis 0.13 or later
  ```

  对端不报实现时（本地运行时的 compat 格式），括号里是 `a runtime older than rutis 0.13`。节点连接以 `link stopped: incompatible (…)` 停下，不重试。反过来，0.13 的运行时遇到旧宿主时以状态 1 退出，stderr：`rutis: refused: incompatible (this host does not declare events-v2 (the event protocol of rutis 0.13): upgrade rutis-host to 0.13 or later); stopping`。
- **插件 API 3**：0.13 的 SDK 写的插件标记为 API 3（多了 `parallel`、`serial`、`waterfall`、`bail` 与 `filter`），旧运行时以现有的信息拒绝它；0.13 的运行时仍加载 API 2 的插件。只用 `on` / `emit` 的插件不用改代码，但 `emit` 的行为变了（见下）。
- **`rutis.json` 里什么都不用声明**：监听器自动登记，宿主一侧也不再配置事件名。

## 叶子插件（Node、Bun、Python、Go）

`ctx.on` 的监听器现在收到这条总线上的所有同名事件，不论发出方在哪个进程、哪种语言、哪个节点；以前只收到同一个运行时进程里的。

**`emit` 不再等待，也不再抛出。** 它按登记顺序开始每个监听器后立即返回；监听器的失败在它所在的一侧写一行 `rutis: listener of <name> in row <row> failed: <ErrorName>: <message>`，不影响发出方、不影响其他监听器，也不再结束运行时进程。要等监听器结束、要拿到它们的错误，改用 `parallel`：

```ts
// 以前：同步抛错会停止后续监听器；未处理的异步失败结束进程
ctx.emit('session/created', id)
// 现在：等全部完成，失败为 AggregateError（按登记顺序）
await ctx.parallel('session/created', id)
```

要监听器的返回值，用 `serial`（第一个结果）、`waterfall`（中间件）或同步的 `bail`。以前手写的"一个监听器出错不影响别人"的循环可以直接换成 `emit`。

**Go。** 监听器多了返回值，`Emit` 不再等待：

```go
// 以前
ctx.On("session/created", func(ctx context.Context, args ...any) error { return nil })
err := ctx.Emit(callerCtx, "session/created", id) // 同步执行，返回第一个错误

// 现在
ctx.On("session/created", func(ctx context.Context, args ...any) (any, error) { return nil, nil })
err = ctx.Emit(callerCtx, "session/created", id)      // 立即返回；err 只报拒绝
err = ctx.Parallel(callerCtx, "session/created", id)  // 等全部；*rutis.AggregateError
```

监听器的 `ctx` 现在带着发出方的调用链，这一行卸载时取消；`Parallel`、`Serial`、`Waterfall`、`Bail` 的发出方放弃时也取消，`Emit` 的监听器不随发出方放弃而取消（`Emit` 不等它们）。`Filter` 的过滤函数在 `Emit` 中失败时不返回给发出方，而是报告一行 `rutis: filter of <name> from row <row> failed: …`。

**卸载等待正在运行的监听器**（各语言一致）：这一行的监听器先撤下，正在运行的收到取消信号（JavaScript `ctx.signal`、Python 任务取消、Go 的 `ctx`），最多等 `RUTIS_SHUTDOWN_TIMEOUT`，然后才运行清理。

**名字。** `internal/*` 仍不能监听或发出；`rutis/*` 现在是保留的框架事件（`rutis/service`、`rutis/row`、`rutis/runtime`）：可以监听，发出以 `ReservedEvent` 失败。插件自己的事件如果用了 `rutis/` 前缀，要改名。

## 测试工具

加载时声明宿主事件的写法删除：监听器自己登记，测试直接用五种方式发出。

| 以前 | 现在 |
| --- | --- |
| Node / Bun：`load(plugin, { emits: ['tick'] })`，`await t.emit('tick', 1)` 等待监听器 | `load(plugin)`；`t.emit('tick', 1)` 立即返回，`await t.parallel('tick', 1)` 等待；还有 `serial`、`waterfall`、`bail`、`filter`。几个插件共享一条总线：`const h = host(); await h.load(a); await h.load(b, { scope: 't1' })` |
| Python：`load(plugin, emits=["tick"])`，`await t.emit("tick", 1)` | `load(plugin)`；`t.emit("tick", 1)` 立即返回，`await t.parallel("tick", 1)` 等待。共享总线：`bus = Bus(); load(a, bus=bus)`、`load(b, bus=bus, scope="t1")` |
| Go：`rutistest.Load(t, plugin, config, services, "tick")`，`loaded.Emit(ctx, "tick", 1)` 等待监听器 | `rutistest.Load(t, plugin, config, services)`（之后是 `rutistest.InScope`、`rutistest.Row` 等选项）；`loaded.Emit` 立即返回，`loaded.Parallel` 等待；`loaded.On` 像宿主一样监听 |

还写着 `emits` 时：Node / Bun 报 `the config field emits was removed in rutis 0.13: listeners register themselves; emit with ctx.emit / parallel / serial / waterfall / bail (https://github.com/arcships/rutis/blob/main/docs/migration-0.13.md)`，Python 报 `the load option emits was removed in rutis 0.13: …`，Go 编译不过（参数类型变了）。`emit` 监听器的失败不再抛出，在 `t.reported`（Python `t.reported`，Go `loaded.Reported()`）里查看。Python 的 `t.emit` 现在返回 `None`，原来的 `await t.emit(...)` 以 `TypeError` 失败：改为 `await t.parallel(...)`。

## 嵌入 rutis 的 Rust 应用（`rutis-bridge`、`rutis-loader`）

宿主到运行时、运行时到宿主不再需要声明，也没有单独的发送入口：宿主里的 Rust 插件和运行时里的插件在同一个逻辑总线上。

| 删除 | 改用 |
| --- | --- |
| `LocalRuntime::emits`、`RuntimePlugin::emits` | 不写：运行时里的监听器自动登记 |
| `Mount.emits`、`Mount.events`（`EventSink`） | 不写；Rust 一侧用 `ctx.events().on(..)` 监听运行时发出的事件 |
| `Process::emit(name, args)` | `ctx.events()` 上的五种方式；`Process::emit` 等全部监听器，对应 `parallel`。事件是平总线，所以 `parallel` 送达这条总线上的所有同名监听器 |
| `runtime::events::{Events, EmitToCordis, EventSink}`（`rutis_bridge::cordis` 也再导出过） | 同上 |
| 节点：`EventsPlugin`、`NodeEvent`、`node_event(name)`、`Features { outbound, inbound }` | `Features { export, import }`（事件与服务共用，见下面的[节点连接](#节点连接)）；`PeerPlugin` 自己启动 `LinkEventsPlugin`；按名字收发用 `EventKey::<rutis_bridge::events::Args>::dynamic(name)` |

以前：

```rust
let node = LocalRuntime::node(runtime_dir, "app/package.json")
    .emits(["file-changed"]);
// …
process.emit("file-changed", vec![json!("a.txt").into()]).await?;
```

现在，按名字带任意参数：

```rust
use rutis::EventKey;
use rutis_bridge::events::Args;
use rutis_bridge::session::Value;

let node = LocalRuntime::node(runtime_dir, "app/package.json");
// …
let key = EventKey::<Args>::dynamic("file-changed");
ctx.events().parallel(&ctx, &key, Arc::new(Args(vec![Value::Data(json!("a.txt"))]))).await?;
```

**`Mount` 与 `Process` 的新参数。** `Mount` 多了两个字段：`ctx` 是运行时加入逻辑总线所经的上下文（它的总线，以及 `plugins` 里各插件的作用域），写 `ctx: Some(ctx.clone())`；不写（`None`）时运行时的事件只留在它自己的进程里，宿主和别的运行时都收不到。`runtime` 可选，是描述里这个运行时的名字，不写时为 `"runtime"`。`Process::load_row` 与 `load_row_exporting` 末尾多一个参数 `scope: &[rutis::Scope]`，即这一行的作用域链：根上的行传 `&[]`，实例里的行传 `&ctx.scope()`。

**运行时一致性套件**（`rutis_bridge::runtime::testing`，feature `testing`）：`weather_emits()` 删除（`Mount` 不再有 `emits`）；`runtime(process, entry, started_here)` 改为 `runtime(&ctx, process, entry, started_here)`，`ctx` 是挂载时作为 `Mount::ctx` 传入的那个上下文，`events` 检查在它的总线上发出。

或者用类型化事件（一个字段的元组结构体是一个参数）：

```rust
#[derive(Serialize, Deserialize)]
struct FileChanged(String);
impl Event for FileChanged { const NAME: &'static str = "file-changed"; type Value = (); }

ctx.events().parallel(&ctx, &EventKey::of(), Arc::new(FileChanged("a.txt".into()))).await?;
```

运行时里的插件 `ctx.emit("file-changed", …)` 时，宿主里用同样的键 `on` 的 Rust 监听器也收到；以前这个方向不存在。

## Cordis 挂载（`rutis-bridge` 的 `cordis` feature）

挂载清单与生成器的 `events`、`emits` 删除，生成器不再为事件生成类型：

```toml
[package.metadata.rutis-cordis.mounts.credentials]
plugin = "@deepseek-ai/dsh-credentials-local"
events = ["credentials/record-updated"]   # 删除
emits = ["file-changed"]                  # 删除
```

还写着时构建失败：`mount credentials: the config field events was removed in rutis 0.13: listeners register themselves; emit with ctx.emit / parallel / serial / waterfall / bail (https://github.com/arcships/rutis/blob/main/docs/migration-0.13.md)`。`rutis_bridge::cordis::build` 的 `.event(name)`、`.emit(name)` 同样删除。

Rust 一侧自己定义事件类型（名字就是 Cordis 的事件名），或用 `Args` 按名字收：

```rust
#[derive(Serialize, Deserialize)]
struct RecordUpdated(serde_json::Value);  // Cordis 插件发出的那个参数
impl Event for RecordUpdated { const NAME: &'static str = "credentials/record-updated"; type Value = (); }

ctx.events().on(&ctx, &EventKey::<RecordUpdated>::of(), listener)?;
```

挂载里的 Cordis 插件用 Cordis 自己的 `ctx.emit` / `parallel` / `serial` / `waterfall` / `bail`，Rust 的监听器按 Cordis 的语义被调用；Rust 发出的事件到达 Cordis 的监听器。以前只能转发没有返回值的事件，`waterfall` 变成一段异步，现在五种方式处处同义。

## Cordis 插件

不用改代码。变化：

- 事件跨进程：一个 Cordis 插件的 `ctx.on` 收到别的运行时、宿主和节点发出的事件，它的 `ctx.emit` 等也到达它们。
- 同步的 `ctx.bail` 遇到异步监听器（`async` 函数，或返回 Promise 的函数）以 `AsyncListener` 失败；以前 Cordis 把 Promise 当作结果。
- `ctx.emit` 的监听器同步抛错不再抛给发出方、不再停止其后的监听器：它在监听器所在的一侧报告一行（`rutis: listener of <name> in row <row> failed: …`），其余监听器照常执行。
- `parallel` 的失败仍是子错误按登记顺序的 `AggregateError`（与 Cordis 4.0.4 相同）；新的是信息为 `<n> listeners of <name> failed`，子错误也包括别处监听器的失败。
- `Context.filter` 对别处的监听器传入 `{ rutis: 描述 }`；每个 Context 上新增只读的 `ctx.rutis`。

**Cordis 应用作为节点**（`@arcships/rutis-runtime/bridge`）：`Events` 的 `out` / `in` 改为 `export` / `import`：

```js
// 以前
ctx.plugin(Events, { peer: 'main', out: ['tock'], in: ['tick'] })
// 现在
ctx.plugin(Events, { peer: 'main', export: ['tock'], import: ['tick'] })
```

还写着 `out` / `in` 时报 `the Events options out / in were removed in rutis 0.13: use export / import (https://github.com/arcships/rutis/blob/main/docs/migration-0.13.md)`；写着 `events` 时报节点行的那条信息（见下）。

## 节点连接

节点行的 `events: { out, in }` 删除，事件与服务共用 `export` / `import`：

以前：

```json
{ "id": "office", "name": "rutis-bridge/peer", "config": { "peer": "office", "dial": "wss://office.example.com/rutis", "export": ["clock"], "events": { "out": ["tick"], "in": ["tock"] } } }
```

现在：

```json
{ "id": "office", "name": "rutis-bridge/peer", "config": { "peer": "office", "dial": "wss://office.example.com/rutis", "export": ["clock", "tick"], "import": ["tock"] } }
```

还写着时报 `the peer config field events was removed in rutis 0.13: list events: { out } in export and events: { in } in import (https://github.com/arcships/rutis/blob/main/docs/migration-0.13.md)`。

- 一项是名字，或以 `*` 结尾的前缀（`session/*`；单独的 `*` 是所有名字）；前缀只用于事件。`export` 里的名字既导出这个服务，也让本节点发出的这个事件到达对端，所以一个名字同时是服务和事件时两者一起共享。
- 节点之间的事件现在是对端监听器在本节点总线上的条目：五种方式、过滤、所有值类型都与本机相同（以前一律当作 `parallel`、只能 JSON、断线时静默丢弃）。连接断开时它们离开总线，进行中的调用失败一次。事件只走一跳：引进来的不再往外共享。
- 对端的监听器是这条总线上的一条条目；它们的描述里带节点行所在的作用域，供过滤使用。

## 行为变化一览

| 变化 | 涉及 |
| --- | --- |
| `emit` 的监听器出错不再结束进程、不再中断其他监听器、不再抛给发出方，在监听器所在处报告一行 | Node、Bun、Python、Go、Rust |
| Go 的 `Emit` 不再等待监听器，错误只报拒绝 | Go |
| `parallel` 的失败总是 `AggregateError`，子错误按登记顺序，只有一个失败也如此 | Rust（以前单个错误原样、多个按完成顺序）；Cordis 的顺序本来如此，新的是信息 `<n> listeners of <name> failed` 和别处监听器的失败；叶子插件的 `parallel` 是新增的 |
| Cordis `ctx.emit` 的监听器同步抛错只在监听器一侧报告，其余监听器照常执行 | Cordis |
| `false` 是没有结果，Rust 的 `Some(false)` 也是 | `serial`、`bail` |
| 同步 `bail` / `waterfall_sync` 遇到异步监听器（或返回 Promise / 协程的函数）以 `AsyncListener` 失败 | 所有语言；Cordis 以前把 Promise 当结果 |
| Rust 监听器以登记它的 `ctx` 被调用，不再是发出方的 | Rust |
| 卸载等正在运行的监听器（最多 `RUTIS_SHUTDOWN_TIMEOUT`）并发出取消信号 | 所有语言 |
| 事件跨实例按名字送达；要实例隔离，用 `filter` 按 `ListenerInfo::scope` 筛或不同的事件名 | 所有语言 |
| `rutis/*` 是保留名字：只能监听 | 所有语言 |
| 插件 API 3；与 0.12 及更早的运行时、节点混用被拒绝 | 所有端 |
