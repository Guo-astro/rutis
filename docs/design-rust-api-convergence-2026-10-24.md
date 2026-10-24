# Rust API 收敛：写插件只有一条路

[English](design-rust-api-convergence-2026-10-24.en.md)

状态：设计（2026-10-24），#617，目标版本 0.17。只改 Rust 公开 API，协议、其他语言的 SDK 和运行时行为都不变。

## 一、目标

> 写 Rust 插件的人只学一种插件写法、三个服务方法，以及和其他语言同名的事件方法。最小的插件不出现 `BoxFuture`、生命周期参数和 `Effect`。宿主用的东西放进 `rutis::host`，rutis 自己的 crate 之间用的东西放进隐藏的 `rutis::internal`。能力一项不少，只是默认路径变短。

Python 插件已经做到了：同样的门控、重载、清理、跨进程和五种事件方式，`Context` 只有 10 个方法。所以 Rust 的公开面大（根上 71 个名字、`Ctx` 40 个方法、`EventBus` 28 个方法、四种插件 trait），原因不在于跨语言、跨进程，而是三处可以修的问题：

- 宿主和 rutis 自己的 crate 要调用的方法，挂在了插件作者用的对象上；
- 绕 Rust 语言限制时用了最笨的办法，例如每个 `on*` 配一个 `_opt`、让作者手写 `BoxFuture`；
- 每加一项能力就多一个名字，从来没有合并过。例如 typed 依赖另起了一套写法，而在内核之外没人用。

## 二、插件作者的写法

### 最小的插件

```rust
use std::sync::Arc;
use rutis::{Ctx, Plugin, Result};

/// 服务就是一个类型。
struct Greeting(String);

/// 提供 Greeting。apply 里登记的东西，插件停下时自动撤销。
struct Greeter {
    lang: String,
}

impl Plugin for Greeter {
    type Deps = ();

    async fn apply(&self, ctx: &Ctx, _: ()) -> Result {
        ctx.provide(Greeting(format!("hello from {}", self.lang)))?;
        Ok(())
    }
}

/// 依赖 Greeting：它出现时启动，被替换时重启。
struct Printer;

impl Plugin for Printer {
    type Deps = Arc<Greeting>;

    async fn apply(&self, _: &Ctx, greeting: Arc<Greeting>) -> Result {
        println!("{}", greeting.0);
        Ok(())
    }
}
```

现在的 README 示例要写 `BoxFuture`、`Box::pin(async move { … })`、`Ok(Effect::Done)`，导入 `CordisError`，还要在 `Plugin` 和 `TypedPlugin` 之间选一个。

### 事件

```rust
use rutis::serde::{Deserialize, Serialize};
use rutis::{events::Mode, Ctx, Event, Plugin, Result};

#[derive(Serialize, Deserialize)]
struct Created(String, u32); // 别的语言里的 emit("session/created", id, size)

impl Event for Created {
    const NAME: &'static str = "session/created";
    type Value = String;
}

struct Audit;

impl Plugin for Audit {
    type Deps = ();

    async fn apply(&self, ctx: &Ctx, _: ()) -> Result {
        ctx.on::<Created>(|event, call| async move {
            if call.mode() == Mode::Waterfall {
                let inner = call.next().await?; // 中间件：先执行链上其余部分
                return Ok(inner.map(|text| format!("[{text}]")));
            }
            Ok(Some(format!("audited {}", event.0))) // Ok(None) 是没有结果
        })?;
        ctx.on_sync::<Created>(|event, _| Ok(Some(format!("sync {}", event.1))))?;
        ctx.listen::<Created>().prefix("session/").on(|_, call| async move {
            println!("saw {}", call.name());
            Ok(None)
        })?;
        Ok(())
    }
}
```

发出事件用五种方式，名字和其他语言相同：

```rust
ctx.emit(Created("s1".into(), 3))?;                         // 立即返回
ctx.parallel(Created("s1".into(), 3)).await?;               // 等全部完成
let first = ctx.serial(Created("s1".into(), 3)).await?;     // 第一个结果
let out = ctx
    .waterfall(Created("s1".into(), 3))
    .terminal(|e| async move { Ok(Some(format!("saved {}", e.0))) }) // 可省略
    .await?;
let quick = ctx.bail(Created("s1".into(), 3))?;             // 同步
let from_python = ctx
    .filter(|l| l.runtime.as_deref() == Some("py"))
    .serial(Created("s1".into(), 3))
    .await?;
```

### 清理和后台任务

```rust
impl Plugin for Server {
    type Deps = Arc<dyn Store>;

    async fn apply(&self, ctx: &Ctx, store: Arc<dyn Store>) -> Result {
        let listener = tokio::net::TcpListener::bind(&self.addr).await?; // io::Error 直接 `?`
        ctx.spawn(async move {
            loop {
                let (conn, _) = listener.accept().await?;
                tokio::spawn(serve(conn, store.clone()));
            }
        })?;
        ctx.effect(async move { flush_logs().await })?;
        Ok(())
    }
}
```

### 配置

配置写成结构体的字段，再提供一个 `fn new(config: &Config) -> Result<Self>`。由装插件的一方用它构造插件，配置变了也用它重建：

```rust
#[derive(Deserialize, JsonSchema)]
struct WeatherConfig {
    city: String,
}

struct Weather {
    city: String,
}

impl Weather {
    fn new(config: &WeatherConfig) -> Result<Self> {
        Ok(Weather { city: config.city.clone() })
    }
}

builtins.register("weather", Weather::new);                // rutis-loader
rutis_sdk::export_plugin!(id: "weather", build: Weather::new); // dylib 插件
let view = root.plugin_from(Weather::new, config);         // 宿主直接装，view.update(新配置)
```

### 用到的全部名字

`rutis` 根只留下 9 个：

| 名字 | 是什么 |
| --- | --- |
| `Plugin` | 唯一的插件 trait |
| `Deps` | `type Deps` 能写什么（`()`、`Arc<T>`、`Option<Arc<T>>`、它们的元组）；作者不实现它 |
| `Ctx` | 见下表 |
| `Event` | 事件类型 |
| `Disposer` | `provide`、`on`、`effect`、`spawn` 返回的句柄，用来提前撤销 |
| `Error`、`Result` | 唯一的错误类型 |
| `service` | `#[rutis::service]`，让服务对其他语言公开 |
| `serde` | 给事件派生 `Serialize` / `Deserialize` |

事件签名里出现的类型放在 `rutis::events`：`Call`、`SyncCall`、`Mode`、`ListenerInfo`、`Scope`，以及 `ctx.listen()`、`ctx.filter()`、`ctx.waterfall()` 返回的构建器 `Listen`、`Filtered`、`Waterfall`，还有异步监听器闭包自动满足的 `Listener`。这些类型都由编译器推断，作者平时只会手写 `Mode`。

`Ctx` 上给插件用的方法有 16 个：

| 组 | 方法 |
| --- | --- |
| 服务 | `provide`、`provide_as`、`get` |
| 生命周期 | `effect`、`spawn`、`cancelled`、`plugin` |
| 事件 | `on`、`on_sync`、`listen`、`emit`、`parallel`、`serial`、`waterfall`、`bail`、`filter` |

和 Python 对照：

| 要做的事 | Python | Rust |
| --- | --- | --- |
| 定义插件 | `define_plugin(apply, inject=[…])` | `impl Plugin`：`type Deps` + `async fn apply` |
| 依赖 | `inject=["llm"]`，`ctx.use("llm")` | `type Deps = Arc<dyn Llm>`，作为 `apply` 的参数传入 |
| 提供服务 | `ctx.provide("weather", obj)` | `ctx.provide(value)`；trait 对象用 `ctx.provide_as::<dyn T>(arc)`；对其他语言公开的名字来自 `#[rutis::service]` |
| 清理 | `ctx.effect(cleanup)` | `ctx.effect(async { … })` |
| 后台任务 | asyncio 任务 | `ctx.spawn(async { … })`（Go 是 `ctx.Go`，Kotlin 是 `ctx.launch`） |
| 监听 | `ctx.on(name, fn)` | `ctx.on::<E>(…)`、`ctx.on_sync::<E>(…)`、`ctx.listen::<E>()` |
| 发出 | `emit` / `parallel` / `serial` / `waterfall` / `bail` | 同名 |
| 过滤 | `ctx.filter(keep)` | `ctx.filter(|info| …)` |
| 配置 | `apply(ctx, config)` | 结构体字段 + `new(&Config)` |

Python 的 `use` 在 Rust 里由 `Deps` 代替。Rust 的 16 个方法里有 7 个是 Python 没有的，每个都对应一处语言差异：

- `provide_as`：Rust 要写明 trait 对象的类型；
- `get`：可选读取，Python 的 `use` 只读声明过的依赖；
- `spawn` 和 `cancelled`：Rust 自己的任务和取消写法；
- `on_sync`：Rust 不能在运行时分辨闭包是同步的还是异步的；
- `listen`：按前缀或另一个名字监听；
- `plugin`：Rust 应用自己装子插件。

## 三、规则

### 插件

```rust
pub trait Plugin: Send + Sync + 'static {
    type Deps: Deps;
    fn apply(&self, ctx: &Ctx, deps: Self::Deps) -> impl Future<Output = Result> + Send;
    fn name(&self) -> &str { /* 类型名的最后一段 */ }
    fn validate(&self) -> Result { Ok(()) }
}
```

- 在实现里写 `async fn apply`（Rust 1.75 起就可以，rutis 的 MSRV 是 1.88）。
- `type Deps` 必须写，没有依赖时写 `()`。关联类型的默认值在稳定版 Rust 里还不能用，这一行省不掉。
- `name` 默认是类型名的最后一段（`Greeter`）；loader 的行有自己的 id。
- `validate` 在装载时、`apply` 之前执行；失败时插件进入 Failed，不执行 `apply`。

插件作者只需要知道三件事：

1. `apply` 在 `Deps` 里所有必需的服务都就绪之后执行；可选的服务不等。
2. `apply` 里经 `ctx` 登记的一切（服务、监听器、清理、任务、子插件）在插件停下时按登记的相反顺序撤销；`apply` 失败时，已经登记的部分同样撤销。
3. 必需的服务被撤销或替换时，插件先停下，等服务重新就绪后再执行一次 `apply`。

六种状态、恰好一次清理、撤销时消费者先停这些保证不变，写在[保证](guarantees.md)里，给宿主作者和排查问题的人看，写插件不需要先读。

### 依赖

| `type Deps =` | 门控 | 传入 |
| --- | --- | --- |
| `()` | 无 | `()` |
| `Arc<T>` | `T` | 服务 |
| `Option<Arc<T>>` | 无 | 装载时可见就有；之后出现或消失都不重载 |
| 至多 8 个成员的元组 | 全部成员 | 各自的值 |

- `T` 可以是 trait 对象：`Arc<dyn Llm>`。
- 依赖只按类型区分。同一类型要两个服务时，用两个新类型（`struct Primary(Db)`、`struct Replica(Db)`）：类型就是名字。按运行期名字区分的服务（传输、身份、运行时会话）属于宿主一层（附录 A）。
- `Deps` 是密封的 trait，作者不实现它。
- 门控之后、`apply` 读取之前依赖被撤销的，回到 Pending，与现在相同。

### 服务

- `ctx.provide(value)`：以 `value` 的类型提供。
- `ctx.provide_as::<dyn T>(arc)`：以 trait 对象的类型提供，例如 `ctx.provide_as::<dyn Llm>(Arc::new(MyLlm))`。
- `ctx.get::<T>()`：可选读取，`T` 可以是 trait 对象。它不门控，也不建立重载关系，诊断里记为未声明的访问。用在"有就用"的场合；需要的服务写进 `Deps`。
- 原来的 `require`、按名字 / 实例的读写、带 `check()` 的提供、可写绑定，移到 `rutis::host` 和 `rutis::internal`（附录 A）。

### 清理和后台任务

- `ctx.effect(future)`：插件停下时执行这个 future，和其他登记一起按相反顺序执行。如果插件已经在停下时才登记，就立即执行它，并返回 `Inactive` 错误，所以刚拿到的资源一定会被释放。清理返回的错误照旧汇集（`take_cleanup_errors`）。
- `ctx.spawn(task)`：立即在运行时上执行 `task`。插件停下时，`task` 在它当前的 `.await` 处被丢弃，停下的过程不等它跑完。需要异步收尾的，把收尾写进 `effect`。`task` 返回的错误进入错误汇集（与 `emit` 的监听器失败相同），插件继续运行。
- `ctx.cancelled()`：插件停下时完成的 future，给执行时间长的监听器调用用。
- `ctx.plugin(child)`：装一个子插件，它随这个插件一起停下。

现在 `ctx.effect(f)` 的意思是"立即执行 `f`，再把它返回的 `Effect` 作为清理"，和其他所有语言的 `effect(cleanup)` 都不一样。改了以后，`Effect` 只留给 `host::DynPlugin` 使用。

### 事件

- 监听：
  - `ctx.on::<E>(|event, call| async move { … })` 注册异步监听器；
  - `ctx.on_sync::<E>(|event, call| …)` 注册同步监听器，所有方式都能调用它；
  - 按前缀或另一个名字监听时用 `ctx.listen::<E>()`，写 `.prefix(p)` 或 `.name(n)`，最后写 `.on(…)` 或 `.on_sync(…)`。
  - `prepend`、`once`、`global` 三个选项放在 `host::ListenExt` 里，`use rutis::host::*` 之后可以接在同一个构建器上。
- 监听器是闭包，参数是 `(Arc<E>, Call<E>)`；同步监听器的参数是 `(Arc<E>, SyncCall<E>)`。返回 `Result<Option<E::Value>>`，`Ok(None)` 表示没有结果。闭包的参数类型由编译器推断，块里可以对任何错误用 `?`。
- `call.mode()` 是这次分发的方式；`call.name()` 是分发时用的名字（前缀监听器用得到）；`call.next()` 只在 `waterfall` 里可用；`call.ctx()` 是登记这个监听器的 `Ctx`。
- 发出：
  - `emit`、`parallel`、`serial`、`waterfall`、`bail` 都接受 `E` 或 `Arc<E>`；
  - `waterfall` 返回一个构建器，可以先写 `.terminal(…)` 再 `.await`；
  - `bail` 是原来的 `bail_sync`，改成和其他语言同名；
  - `ctx.filter(pred)` 给出同样这五个方法。
- 名字：事件以 `E::NAME` 登记和发出。没有固定名字的事件类型（例如 rutis-bridge 的 `Args`）重写 `Event::name(&self)`，让值自己带上名字；监听这种事件时用 `ctx.listen::<Args>().name("file-changed")`。
- 五种方式的语义、作用域、过滤、卸载等待，都和[统一事件设计](design-events-2026-10-18.md)一致，不变。

### 错误

- 所有签名里只有一种错误类型 `rutis::Error`，`rutis::Result<T = (), E = Error>`。
- `?` 能把任何 `std::error::Error + Send + Sync + 'static` 转成 `rutis::Error`，rutis 自己的错误也一样。`Error::msg("…")` 用字符串构造一个错误。
- `rutis::Error` 本身不实现 `std::error::Error`，这是 `?` 能接住任何错误的前提（`anyhow::Error` 和 Bevy 的 `BevyError` 也是这么做的）。它可以转成 `Box<dyn std::error::Error + Send + Sync>`，所以宿主在返回 `Box<dyn Error>` 的函数里照常用 `?`。
- 宿主要区分 rutis 的失败时，`err.kind()` 返回 `Option<&host::ErrorKind>`。`ErrorKind` 就是现在的 `CordisError` 枚举，改了名字，变体不变。来自插件代码的错误，`kind()` 是 `None`；它报到宿主时被包在 `ErrorKind::PluginFailed` 里。
- 错误仍然不能 `Clone`，要共享时用 `Arc<Error>`（D11、D25 不变）。

### 三层

| 层 | 给谁 | 文档 | 稳定性 |
| --- | --- | --- | --- |
| `rutis`（根）和 `rutis::events` | 插件作者 | 插件指南 | 随版本说明变更 |
| `rutis::host` | 嵌入 rutis 的应用、测试 | 宿主指南 | 同上 |
| `rutis::internal` | rutis-bridge、rutis-loader、rutis-dylib、rutis-sdk、rutis-macros、rutis-host | `#[doc(hidden)]` | 不承诺；仓库外有人需要时，提 issue 把它移到 `host` |

- 划分规则：写插件用的放根；应用嵌入 rutis 时要用的放 `host`；仓库里除了 rutis 自己的 crate 之外没人用的放 `internal`。
- `Ctx` 的方法也按这个规则分开：`host` 和 `internal` 的方法放在扩展 trait 里（`host::CtxHost`、`internal::CtxInternal`），要先 `use rutis::host::*` 才能调用。rustdoc 把扩展 trait 的方法放在 `Ctx` 页面下单独的一节。
- rutis-loader 也这样分：根上留下组装加载器的 11 个名字，其余按主题放进模块，只有 rutis-host 和测试用的放进 `internal`。

每一层的完整清单、现在每个名字的去向见附录 A；迁移和实施步骤见附录 B。

## 四、取舍

- **只加 `rutis::prelude`，其他不动**：`Ctx` 和 `EventBus` 上的方法不会因为多一个 prelude 而变少，文档里照样是 40 个和 28 个。所以根本身就当 prelude 用，不再另设一个模块。
- **新写法当默认，旧写法也保留**：这样同一件事又多了一种写法，#617 说的问题正是写法太多。所以 0.17 一次改完，不保留旧名字（附录 B）。
- **`#[rutis::plugin]` 宏**：`async fn` 已经能写在 trait 里，`BoxFuture` 不用宏也能去掉。加了宏，作者看到的就不是真正的 trait，出错时的提示也更难懂。
- **把配置做成关联类型、`apply(ctx, config, deps)`（Cordis / Python 的写法）**：那样每个插件都要多写一个关联类型，没有配置的插件装起来也得写 `ctx.plugin(P, ())`。在 Rust 里，配置本来就写在结构体的字段上；配置变了重建插件，是装插件那一方的事。
- **用一个 `on` 同时接受同步和异步闭包**（axum、Bevy 那样的标记类型技巧）：试过。候选实现有两个，编译器就推断不出闭包参数的类型，每个监听器都得写成 `|e: Arc<Created>, call: Call<Created>|`。分成两个方法以后，`on::<E>(|e, call| async move { …? … })` 不用写任何类型标注（两种写法都在 Rust 1.88 上验证过）。
- **`listen` 带上全部选项**：`prepend`、`once`、`global` 在内核之外没有 Rust 代码用过；Cordis 插件用的是 Cordis 自己的 API，不经过这里。所以这三个选项放进 `host`，插件作者看到的构建器只有 `prefix` 和 `name`。
- **一个 `dispatch(mode, …)` 代替五个分发方法**：五种方式的返回值不同（不等待、汇总错误、第一个结果、链式、同步），合成一个以后，要么丢掉类型，要么返回一个枚举让调用方再拆。
- **依赖里保留名字（`Keyed<T>`、`DepKey<T>`）**：内核之外没有用户。在类型里写死名字需要稳定版还没有的 `&str` 常量泛型；运行期才定的名字要另带一份键，`Typed::with_keys` 就是这么来的。同一类型要两个服务时，用新类型就够了。
- **`rutis::Error` 也实现 `std::error::Error`**：这样就和 `impl<E: std::error::Error> From<E>` 冲突，`?` 又接不住任意错误了，作者只能回到 `map_err`。

## 五、验收

对照 #617 的验收项：

| #617 | 本方案 |
| --- | --- |
| 插件作者默认看到的入口 ≤ 15 个 | 根上 9 个；`rutis::events` 里的类型都由编译器推断 |
| 最小的 Rust 插件不手写 `BoxFuture` 和生命周期 | 第二节的例子；README、指南、示例都用这种写法 |
| `EventBus` 的公开方法 ≤ 10 个 | 插件只用 `Ctx` 上的 9 个事件方法，`EventBus` 移进 `internal` |
| 服务的 provide / read 方法 ≤ 5 个 | 3 个：`provide`、`provide_as`、`get` |
| `rutis-loader` 有明确的 prelude 和不稳定边界 | 根上 11 个名字，其余按模块分组，`internal` 不承诺稳定 |
| 文档第一页有"只需要这些"的清单 | 新增 `docs/guide/rust-plugin.md`（写一个 Rust 插件），指南首页加一行 |

另外：

- 用 `cargo public-api` 给 `rutis` 根、`rutis::events` 和 `rutis::host` 生成公开面快照，提交进仓库，CI 比对。以后加一个名字就要改快照，评审时看得见，公开面不会再悄悄变大。
- `examples/` 和指南里的 Rust 代码不出现 `rutis::internal`，由 CI 用 grep 检查。
- `docs/guide/rust-plugin.md` 和 README 的 Rust 代码作为 doctest 编译，与现在开发手册的做法相同。
- 第二节的每个写法都写成测试，包括：`?` 接住 `io::Error`；不写类型标注的监听器闭包能编译；`effect` 在插件停下以后才登记时立即执行；`spawn` 的任务在插件停下时被丢弃；`DynPlugin` 与 `Plugin` 的插件能互相依赖。
- 重新跑一遍事件分发的基准（[事件分发性能](performance-event-dispatch-2026-09-27.md)）。`Call` 改成拥有所有权以后，每次调用多一次 `Arc` 克隆，要把前后的数字写进 PR。

## 附录 A：名字去向

### `rutis::host`

| 内容 | 名字 |
| --- | --- |
| 根与关闭（`CtxHost`） | `Ctx::root`、`root_with`、`root_with_sink`、`root_view`、`root_ctx`、`view`、`shutdown`、`shutdown_with_timeout`、`is_shutting_down`、`handle`、`error_sink`、`take_cleanup_errors`、`diagnostics` |
| 按键读写服务（`CtxHost`） | `require`、`require_at`、`get_at`、`provide_at`；键是 `Key<T>` |
| 作用域与实例（`CtxHost`） | `isolate`、`isolate_label`、`instance`、`scope`、`row`、`dispose_self`、`cancellation_token` |
| 装载（`CtxHost`） | `plugin_from(build, config)`（类型化的配置闭包，依赖从 `P::Deps` 得出）、`plugin_with(factory, config)` |
| 事件（`CtxHost`） | `waterfall_sync`、`subscriptions` |
| 监听选项（`ListenExt`） | `ctx.listen::<E>()` 上的 `prepend`、`once`、`global` |
| 插件的对象安全形式 | `DynPlugin`、`PluginFactory`、`Effect`、`EffectMeta`、`EffectPhase`、`BoxFuture` |
| fiber | `FiberView`、`FiberState`、`FiberStatusChanged`、`PluginId`、`Snapshot`、`DisposeWaitError`、`shutdown_deadline` |
| 键 | `Key<T>`、`TypeKey`、`InstanceId` |
| 错误 | `ErrorKind`、`ServiceReadError`、`ServiceReadFailure`、`NextMisuse`、`CallError`、`ErrorSink`、`default_sink` |
| 诊断 | `host::diagnostics::*`（现在的 10 个诊断类型） |
| 事件描述 | `EventSubscription`、`ListenerKind` |

- **`DynPlugin`** 就是现在的 `Plugin` trait，改了名字，只有一处不同：`injects()` 返回 `Vec<TypeKey>`，不再返回借用的切片。rutis 为每个 `Plugin` 提供 `impl<P: Plugin> DynPlugin for P`，所以 `ctx.plugin(…)` 只有一个，什么插件都能装。依赖要在运行期才定下来的插件（例如 rutis-loader 的 `JsFactory`）、要放进 `Vec<Box<dyn DynPlugin>>` 的插件列表，用的都是 `DynPlugin`。
- **`Key<T>`** 合并了现在的 `Key<T>` 和 `DepKey<T>`：`Key::of()`、`const fn Key::named("primary")`、`Key::dynamic(name)`、`.instance(id)`，可以转成 `TypeKey`。键本身带着值的类型，所以 `provide_at(key, arc)` 的类型不匹配会在编译期报错，现在要到运行时才报。
- **`ServiceKey`** 删除，它只是 `TypeKey` 的别名。

### `rutis::internal`

`EventBus` 和 `events::{remote, wire}`、`EventKey`、`EventPattern`、`EventOptions`、`DispatchAttempt`、`ServiceIntercept`、`ServiceWriter`、`ServiceWriteError`、`ServiceWriteFailure`、`__sdk_service`，以及 `CtxInternal`：`events()`、`mark_scope`、`mark_row`、`provide_checked`（原 `provide_as_with_check`）、`provide_mut_at`（原 `provide_mut_as`）、`require_provided_as`、`effect_named`、`refresh`、`in_instance_key`。

`EventBus` 上只有 rutis-bridge / rutis-loader 用的 10 个方法（`on_remote`、`watch_table`、`observe_dispatch`、`emit_framework`、`dispatch`、`dispatch_sync`、`invoke_entry`、`invoke_entry_sync`、`transcoder`、`set_transcoder`）连同 `try_filter` 留在这里，签名不变。插件和宿主不再拿到 `EventBus`，原来的 `ctx.events().on(&ctx, &key, …)` 都改成 `ctx.on…` 系列方法。

### `rutis` 根上现在的 71 个名字

| 现在 | 0.17 |
| --- | --- |
| `Plugin` | `host::DynPlugin`（`injects` 返回 `Vec`）；新的 `Plugin` 就是原来的 `TypedPlugin`，按第三节去掉了 `BoxFuture` 和 `Effect` |
| `TypedPlugin` | `Plugin` |
| `Typed` | 删除（`impl<P: Plugin> DynPlugin for P`） |
| `PluginFactory` | `host::PluginFactory`（`build` 返回 `Box<dyn DynPlugin>`） |
| `TypedPluginFactory`、`TypedFactory` | 删除；用 `plugin_from`、`Builtins::register`、`export_plugin!(build:)` |
| `Deps` | `Deps`（密封；去掉 `Keys`） |
| `DepKey`、`Keyed`、`KeyedGate` | 删除；按名字的依赖用 `host::DynPlugin` + `host::Key<T>` |
| `Gate` | 删除；写 `Arc<T>` 而不使用它 |
| `BoxFuture` | `host::BoxFuture` |
| `Effect`、`EffectMeta`、`EffectPhase` | `host::` |
| `Disposer` | `Disposer` |
| `Ctx` | `Ctx`（方法按第二节和本附录分开） |
| `TypeKey`、`InstanceId` | `host::` |
| `Key` | `host::Key<T>`（与 `DepKey` 合并） |
| `ServiceKey` | 删除（`TypeKey` 的别名） |
| `ServiceIntercept`、`ServiceWriter` | `internal::` |
| `CallError` | `host::CallError` |
| `service` | `service` |
| `CordisError` | `Error`；枚举本身改名为 `host::ErrorKind` |
| `ServiceReadError`、`ServiceReadFailure`、`NextMisuse`、`ErrorSink`、`default_sink` | `host::` |
| `ServiceWriteError`、`ServiceWriteFailure` | `internal::` |
| `EventBus`、`DispatchAttempt` | `internal::` |
| `EventSubscription`、`ListenerKind` | `host::` |
| `Filtered` | `events::Filtered` |
| `Event` | `Event`（增加 `fn name(&self)`，默认返回 `NAME`） |
| `EventKey`、`EventPattern`、`EventOptions` | `internal::`；插件用 `ctx.listen::<E>()` |
| `Listener` | `events::Listener`（由异步监听器闭包自动满足，不手写实现） |
| `SyncListener`、`PatternListener`、`SyncPatternListener` | 删除；监听器只用闭包，前缀监听器从 `call.name()` 读名字 |
| `Terminal` | 删除；用 `ctx.waterfall(e).terminal(…)` |
| `Call`、`SyncCall` | `events::Call<E>`、`events::SyncCall<E>`（拥有所有权，不再带生命周期参数） |
| `Mode`、`ListenerInfo`、`Scope` | `events::` |
| `FiberView`、`FiberState`、`FiberStatusChanged`、`PluginId`、`Snapshot`、`DisposeWaitError`、`shutdown_deadline` | `host::` |
| 诊断类型（`RuntimeDiagnostics` 等 10 个） | `host::diagnostics::` |
| `serde` | `serde` |

### `Ctx` 现在的 40 个方法

| 现在 | 0.17 |
| --- | --- |
| `provide`、`get`、`plugin`、`cancelled` | 不变（`get` 允许 trait 对象） |
| `provide_as(key, arc)` | `provide_as::<T>(arc)`；带键的改用 `host::provide_at(Key<T>, arc)` |
| `get_as(key)` | 不带名字的用 `get::<T>()`；带键的用 `host::get_at` |
| `effect(f)` | `effect(future)`，见第三节 |
| `require`、`require_as` | `host::require`、`host::require_at` |
| `root`、`root_with`、`root_with_sink`、`root_view`、`root_ctx`、`view`、`shutdown`、`shutdown_with_timeout`、`is_shutting_down`、`handle`、`error_sink`、`take_cleanup_errors`、`diagnostics`、`isolate`、`isolate_label`、`instance`、`scope`、`row`、`dispose_self`、`cancellation_token`、`plugin_with`、`plugin_from` | `host::CtxHost` |
| `events` | `internal::CtxInternal`；事件方法在 `Ctx` 上 |
| `mark_scope`、`mark_row`、`provide_as_with_check`、`provide_mut_as`、`require_provided_as`、`effect_named`、`refresh`、`in_instance_key` | `internal::CtxInternal`（两个改名：`provide_checked`、`provide_mut_at`） |
| 新增 | `spawn`、`on`、`on_sync`、`listen`、`emit`、`parallel`、`serial`、`waterfall`、`bail`、`filter` |

### `EventBus` 现在的 28 个方法

| 现在 | 0.17 |
| --- | --- |
| `on`、`on_opt`、`once`、`on_sync`、`on_sync_opt`、`on_pattern`、`on_pattern_opt`、`on_sync_pattern`、`on_sync_pattern_opt` | `ctx.on`、`ctx.on_sync`、`ctx.listen::<E>()`（`.prefix()`、`.name()`；`.prepend()`、`.once()`、`.global()` 在 `host::ListenExt`） |
| `emit`、`parallel`、`serial`、`waterfall`、`bail_sync` | `ctx.emit`、`parallel`、`serial`、`waterfall`、`bail` |
| `filter` | `ctx.filter` |
| `waterfall_sync`、`subscriptions` | `host::CtxHost` |
| `try_filter`、`on_remote`、`watch_table`、`observe_dispatch`、`emit_framework`、`dispatch`、`dispatch_sync`、`invoke_entry`、`invoke_entry_sync`、`transcoder`、`set_transcoder` | `internal`（签名不变） |

### rutis-loader

根只留下组装一个加载器要用的名字：`LoaderPlugin`、`LoaderOptions`、`Loader`、`Layer`、`Patch`、`Builtins`、`Chain`、`Resolver`、`ServiceCatalog`、`LoaderError`、`RowEvent`，共 11 个。其余按主题放进模块：

| 模块 | 内容 |
| --- | --- |
| `runtime` | `RuntimeResolver`、`RuntimeRowsPlugin`、`RuntimeRows` 等；子模块 `runtime::binary`、`runtime::go`、`runtime::swift` 放各语言的 `*Resolver`、`*Runtimes`、`*Manifest` |
| `peer` | `PeerResolver`、`register_peer_node`、`PeerRows`、`PeerRowsPlugin`、`LoaderCatalog` |
| `report` | 加载器方法返回的类型：`CheckReport`、`ReconcileReport`、`RowInfo`、`RowStatus`、`RowCheck`、`Isolate`、`EntryInfo`、`EntryStatus`、`Failure`、`Unknown`、`Unavailable`、`Supply`、`Provider`、`Requirement`、`InstanceInfo`、`LoaderChanged`、`PendingEditDropped` |
| `edit` | 命令式修改与持久化：`Edit`、`Editable`、`NewEntry`、`CreateInstance`、`Instance`、`InstanceResult`、`Persist`、`NoPersist`、`PersistError`、`Version`、`apply_edit`、`VolatileUpdate`、`volatile_key`、`volatile_paths` |
| `resolve` | 自己写解析器时用：`Build`、`Resolved`、`Declarations`、`Declared`、`ScopedFactory`、`Expressions`、`ExprScope` |
| `internal`（`#[doc(hidden)]`） | 只有 rutis-host 和测试在用的：`apply_patches`、`Composed`、`ComposedRow`、`Owner`、`PatchWarning`、`is_runtime_name`、`resolve_entry`、`PLUGIN_API`、`SWIFT_PLUGIN_API`、`GO_MARKER`、`SWIFT_MARKER`、`read_*_manifest`、`node_schema`、`REMOVED_EVENTS` |

`Builtins` 给插件用的注册方法合并成两个：

- `register(name, build)`：`build: Fn(&C) -> Result<P>`，其中 `P: Plugin`、`C: DeserializeOwned + JsonSchema`。config 的 schema 从 `C` 生成，依赖从 `P::Deps` 生成。不需要 schema 的，把 `C` 写成 `serde_json::Value`。
- `register_with(name, build)`：`build: Fn(&Build, &C) -> Result<P>`，用于每个实例各构造一份。

`register_fn` 删除，用 `register` 代替。`register_raw` 保留，给运行期才知道依赖的 `PluginFactory<Value>` 使用。

### rutis-sdk（dylib 插件）

`export_plugin!(id: "weather", build: Weather::new)` 接受和 `Builtins::register` 相同的构造函数，`schema:` 照旧可选。原来的 `factory:` 形式保留，给需要 `PluginFactory<Value>` 的插件用。dylib 插件本来就要用和宿主完全相同的 SDK 构建，所以 0.17 的插件照常重新构建即可。

## 附录 B：迁移与实施

### 迁移

0.17 一次改完，不保留旧名字，和 0.13 的统一事件做法相同：旧写法编译不过，按 `docs/migration-0.17.md` 里的对照表修改。不用 `#[deprecated]` 别名过渡：留着旧名字，旧名字就还会出现在文档和补全里，这正是 #617 要去掉的。协议不变，所以和其他语言、其他节点的互通不受影响。

**写 Rust 插件的人**改三处：

1. `impl Plugin` / `impl TypedPlugin` 改成新的 `impl Plugin`：加上 `type Deps`，把 `fn apply<'a>(…) -> BoxFuture<…>` 改成 `async fn apply(&self, ctx: &Ctx, deps: Self::Deps) -> Result`，去掉 `Box::pin` 和 `Ok(Effect::Done)`。`injects()` 里列的类型移进 `type Deps`，`apply` 里对应的 `ctx.require::<T>()` 改成参数。
2. `ctx.events().on(&ctx, &EventKey::of(), |ctx, e, call| Box::pin(async move { … }))` 改成 `ctx.on::<E>(|e, call| async move { … })`；`bail_sync` 改成 `bail`；分发时传值。
3. `CordisError` 改成 `rutis::Error`；`.map_err(|e| CordisError::PluginFailed(e.into()))?` 改成 `?`。`Effect` 返回值里的清理改成 `ctx.effect(async move { … })`。

改之前和改之后：

```rust
// 0.16
impl Plugin for Weather {
    fn name(&self) -> &str { "weather" }
    fn injects(&self) -> &[TypeKey] { &self.injects } // [TypeKey::of::<dyn Llm>()]
    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, CordisError>> {
        Box::pin(async move {
            let llm = ctx.require_as::<dyn Llm>(TypeKey::of::<dyn Llm>())?;
            let file = std::fs::read_to_string(&self.path)
                .map_err(|e| CordisError::PluginFailed(e.into()))?;
            ctx.provide(Forecast::new(llm, file))?;
            Ok(Effect::Done)
        })
    }
}

// 0.17
impl Plugin for Weather {
    type Deps = Arc<dyn Llm>;

    async fn apply(&self, ctx: &Ctx, llm: Arc<dyn Llm>) -> Result {
        let file = std::fs::read_to_string(&self.path)?;
        ctx.provide(Forecast::new(llm, file))?;
        Ok(())
    }
}
```

**嵌入 rutis 的应用**：加 `use rutis::host::*;`；`PluginFactory` 和自己实现 `Plugin` 的对象安全写法改成 `host::DynPlugin`（`injects` 返回 `Vec`）；`Builtins::register_fn` 改成 `register`；从 rutis-loader 根上消失的名字到附录 A 的模块里找。

**仓库里的 crate** 在同一个版本里一起改：rutis-bridge、rutis-loader、rutis-dylib、rutis-sdk、rutis-macros、rutis-host、rutis-dsh、rutis-dev、rutis-xtask、aimux-llm，以及示例 rutis-agent、rutis-cli 和 `examples/`。内核之外有 42 处 `impl Plugin`、14 处 `impl PluginFactory`，都机械地改成 `host::DynPlugin` / `host::PluginFactory`；只有 aimux-llm 和示例改用新的 `Plugin`，因为它们就是给插件作者看的写法。只改名的部分（`CordisError` 出现 1277 次）用脚本批量替换。真正要重写的只有内核里事件、清理和插件包装这几处。

其他语言的 SDK、运行时、协议、`rutis.json` 都不受影响。

### 实施

在一个集成分支上按下面的顺序提交，整体合入，随 0.17 发布：

1. 内核：新的 `Plugin` / `Deps` / `DynPlugin` 和 blanket 实现；`Error` / `ErrorKind`；`Ctx` 上的 `effect`、`spawn` 和事件方法（`Call` 改成拥有所有权）；合并 `Key<T>`；建立 `host` / `internal` 两个模块和扩展 trait；删除附录 A 列出的名字。
2. rutis-bridge、rutis-loader、rutis-dylib、rutis-sdk、rutis-macros 改用 `DynPlugin` 和 `internal`；`Builtins::register`、`export_plugin!(build:)` 改用类型化的构造函数。
3. rutis-loader 的根和模块重新分组。
4. rutis-host、rutis-dsh、rutis-dev、rutis-xtask、aimux-llm、示例（rutis-agent、rutis-cli、`examples/`）。
5. 文档，中英两份：README 的 Rust 部分、新增 `guide/rust-plugin.md`、`guide/README.md`、`guide/rust-host.md`、`guide/plugin-api.md` 的 Rust 部分、开发手册、`migration-0.17.md`。
6. 公开面快照和 CI 检查。
