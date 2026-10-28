# 内核能力一览

[English](core-features.en.md) · [开发手册](development-handbook.md) · [API 文档](https://docs.rs/rutis)

`rutis` 内核在插件、fiber、服务和事件之外提供的能力，以及它们的使用边界。入门请先看 [README](../README.zh-CN.md) 的快速开始、[写一个 Rust 插件](guide/rust-plugin.md)和[应用设计指南](development-guide.md)。

下文的宿主方法（`plugin_with`、`shutdown`、`diagnostics`、`require` 等）来自 `rutis::host::CtxHost`，先写 `use rutis::host::*;`。标明在 `rutis::internal` 里的，是 rutis 自己的 crate（rutis-bridge、rutis-loader 等）之间用的，不承诺稳定。

## 配置热更新

运行中改配置,复用状态机的恰好一次清理,受影响的消费者自动跟随:

```rust
struct Weather { city: String }

struct WeatherFactory;
impl PluginFactory<WeatherConfig> for WeatherFactory {
    /// 纯构造：预检查和正式装载都会调用它。
    fn build(&self, config: &WeatherConfig) -> rutis::Result<Box<dyn DynPlugin>> {
        Ok(Box::new(Weather { city: config.city.clone() }))
    }
}

let view = ctx.plugin_with(WeatherFactory, cfg_v1);
view.update(cfg_v2).await?;   // dry-run 不过则现状不动;通过则卸载重载
```

`Weather` 是普通的 `Plugin`；工厂（`host::PluginFactory<C>`）每次装载从当前配置构造它，依赖写在工厂的 `injects()` 里。

## 事件：一张监听器表，五种方式

事件按**名字**分发（`E::NAME`；没有固定名字的类型重写 `Event::name`，让每个值带上自己的名字，监听时写 `ctx.listen::<E>().name(n)`）。事件类型派生 `Serialize` / `Deserialize`，所以同名的另一个 Rust 类型、另一个进程或节点上的监听器都收到同一个事件（[统一事件设计](design-events-2026-10-18.md)、[协议规格](design-events-protocol-2026-10-18.md) §12.1）。

```rust
#[derive(Serialize, Deserialize)]
struct RoomEvent { room: String }
impl Event for RoomEvent { const NAME: &'static str = "room/joined"; type Value = String; }

// 登记一次，任何方式都能调用它；call.mode() 是这次的方式，call.next() 只在 waterfall 里可用
ctx.on::<RoomEvent>(|e, _call| async move { Ok(Some(format!("hi {}", e.room))) })?;
let hi = ctx.serial(RoomEvent { room: "a".into() }).await?;
```

- `emit` 立即返回，按登记顺序开始每个监听器，不等任何一个；同一名字的多次 emit 按发出顺序开始；监听器失败进 ErrorSink（`ListenerFailed`），不影响其他监听器。
- `parallel` 按登记顺序开始、等全部完成；有失败时总是 `ListenersFailed`（AggregateError），按登记顺序。
- `serial` / `bail` 取第一个"有结果"的返回值（`None` 与 `Some(false)` 是没有结果）；错误停止。`bail` 遇到异步监听器报 `AsyncListener`。
- `waterfall` 由外向内，监听器经 `call.next()` 调用其余；不调即否决；终态可选（`ctx.waterfall(e).terminal(…)`，不写就没有终态）；返回最外层的值。
- 同步监听器用 `on_sync` 登记，所有方式都能调用它。`ctx.listen::<E>().prefix("room/")` 登记前缀监听器，它从 `call.name()` 读到实际的名字；`listen()` 还有 `.once()`、`.prepend()`、`.global()`。
- **作用域**：加载器把每个组实例标为作用域（`rutis::internal` 的 `mark_scope`），`ctx.scope()` 是从根起的作用域链。事件在一个 root 的总线上按名字送达，所有监听器都收到；作用域只是 `ListenerInfo` 里的描述，供过滤使用。
- **过滤**：`ctx.filter(|info: &ListenerInfo| ..)` 给出同样的五个发出方法；过滤在任何调用之前、在发出方进程里看每个非 `global` 监听器的描述（插件、行、运行时、节点、作用域）。
- **生命周期**：监听器以登记它的 `Ctx` 被调用；它的 fiber 卸载时不再开始新的调用，并等正在运行的调用结束（它们的 `ctx.cancelled()` 已触发），最多到宿主的关闭截止时间。
- `rutis/*` 是框架事件的保留名字：插件可以监听，发出则报 `ReservedEvent`。

## 清理的名字

关闭超时时，Stuck 提示写出还在运行的清理：`ctx.effect` 登记的写登记它的位置，`rutis::internal` 的 `effect_named("cache watcher", || effect)` 登记的写这个名字。

## 可写服务

可写绑定在 `rutis::internal` 里：`provide_mut_at(key, value)` 返回绑定代次限定的 `ServiceWriter<T>`，由提供者使用 `writer.set(&ctx, Arc::new(value))` 更新；旧 `Arc<T>` 快照保持旧值，写入不自动重载消费者。

## 使用边界

插件需要的服务写进 `type Deps`，`apply` 收到的就是严格读取的结果。宿主的 `require` / `require_at` 是同样的严格读取，对应 Cordis 普通插件访问服务时的声明检查；它沿 fiber 祖先链核对依赖声明（`Deps`，或 `DynPlugin::injects()`），并区分未声明、未就绪、实例越界和上下文失活，错误保留调用位置。同一次读取同时越界且上下文失活时，先报实例越界；登记和实例派发的错误优先级单独定义。`get` / `get_at` 对应 Cordis 显式 `ctx.get()` 定位器：返回 `Option`，不强制依赖声明，也不建立重载关系。provider 未 Active 或读取方正在卸载时通常不可见，provider 子树在清理期间仍可读取自己提供的服务。实例键另有子树可见性检查。监听器由登记它的 `Ctx`（调用 `on` 的那个）持有，回调也以这个 `Ctx` 被调用：回调登记的资源属于登记监听器的插件。可编译示例见 [listener_ctx_ownership.rs](../crates/rutis/examples/listener_ctx_ownership.rs)。

## 依赖诊断

`ctx.diagnostics()` 列出存活 fiber 的身份、状态、声明与已绑定依赖，以及服务绑定。`injects[].status` 可区分缺失、实例越界、provider 未就绪、摘除中、check 拒绝或 panic；`TypeKey::describe()` 给出类型名、限定名和实例号。读取只使用已登记的元数据与最近一次门控检查结果，不调用插件或 check，也不触发生命周期转换。它逐个读取 fiber 和绑定，**不是全树原子快照**：并发生命周期变化时，同一结果中的状态、依赖和绑定可能来自不同时刻。check 状态（`check()` 谓词由 `rutis::internal` 的 `provide_checked` 登记）可能停留在上次门控结果；调用 `refresh()` 后应等待相关 fiber 收敛，再重新读取诊断。

插件等的服务（`Deps` 或 `injects`）一直没有提供者时，它停在 Pending，本身不报错。等了 10 秒仍缺时，ErrorSink 收到一次 `ErrorKind::StillWaiting`，写明插件和缺的类型（默认的 sink 打印 `printer: still waiting after 10s for app::Greeting: it starts once that is provided`）；插件照常继续等，每次重新开始等待最多报一次。rutis 自己的插件（链路等对端、运行时的行等运行时）不报，它们在等什么看它们自己的状态（0.18 起，#678）。`ctx.report_waiting_after(Some(时长))` 改这个时间，`None` 不报，对整棵树生效（0.17.2 起，#676）。缺的键默认按 `TypeKey::describe()` 写；`ctx.name_keys_with(&namer)` 加一个命名函数，rutis-loader 挂上时就加了一个：服务写它的名字（`llm`），行的运行时写 `runtime py`。rutis-host 关掉它：它的行状态已经写明在等什么。

`ctx.is_shutting_down()` 直接读取 root 的关闭标志。调用 `Ctx::shutdown()` 时即变为 true，无需等待清理完成，关闭结束后仍为 true。子上下文和隔离上下文共享该标志；仅卸载或关闭子插件不会将它置为 true。只需要关闭状态时（例如 loader 修改或创建实例前），可用它避免收集诊断快照。

## 错误与 panic

所有签名里的错误都是 `rutis::Error`，`?` 能接住任何 `std::error::Error`。宿主用 `err.kind()`（`Option<&host::ErrorKind>`，插件代码自己的错误是 `None`）区分 rutis 的失败；要共享的错误（错误汇集、`FiberView` 的结果）是 `Arc<ErrorKind>`。

`apply` 的同步及异步 panic 会变成插件错误；`check()` panic 视为依赖未就绪；`waterfall` 回调 panic 向调用方传播。`settle` 仅是该 fiber 的 FIFO 栅栏，Pending 也可能是稳定结果。装载失败（构造、`validate` 或 `apply` 出错）的插件停在 Failed，只在依赖变化（提供者换代、摘除后恢复）时重新装载；同一组依赖的重复通知不会让它重试。root 的 `dispose()` 后仍可重启，`shutdown()` 是最终关闭；丢弃等待 future 不会停止已启动的清理。`update(config)` 重新装配插件，不替换进程中的代码。提前 `Disposer::dispose()` 的失败立即返回给调用方，不自动通知 ErrorSink；这些错误在终态卸载时进入结果，在重载时交给 ErrorSink。

## 与 Cordis 的关系

rutis 是 [Cordis](https://github.com/shigma/cordis) 范式的 Rust 惯用实现,不是翻译:96 个原版 spec 逐条审阅,58 个语言无关不变量全部自动化对拍;其余差异全部显式声明(决策表 + 不移植清单 + 对照审计)。已知的刻意强化:跨 effect 清理严格串行 LIFO(cordis 并发)、同一名字的多次 emit 按发出顺序开始。
