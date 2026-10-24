# Rust API 收敛：写插件只有一条路

[English](design-rust-api-convergence-2026-10-24.en.md)

状态：设计（2026-10-24），#617，目标版本 0.17。只把 Rust 公开 API 分层、缩短默认路径：不加新能力，不改运行时语义；协议、其他语言的 SDK 和 `rutis.json` 都不变。

## 一、目标

> 写 Rust 插件的人只学一种插件写法、三个服务方法，以及和其他语言同名的事件方法。最小的插件不出现 `BoxFuture`、生命周期参数和 `Effect`。宿主用的东西放进 `rutis::host`，rutis 自己的 crate 之间用的东西放进隐藏的 `rutis::internal`。每项能力都还在，行为和 0.16 相同，只是换了位置、换了写法。

Python 插件已经做到了：同样的门控、重载、清理、跨进程和五种事件方式，`Context` 只有 10 个方法。所以 Rust 的公开面大（根上 71 个名字、`Ctx` 40 个方法、`EventBus` 28 个方法、四种插件 trait），原因不在于跨语言、跨进程，而是三处可以修的问题：

- 宿主和 rutis 自己的 crate 要调用的方法，挂在了插件作者用的对象上；
- 绕 Rust 语言限制时用了最笨的办法，例如每个 `on*` 配一个 `_opt`、让作者手写 `BoxFuture`；
- 每加一项能力就多一个名字，从来没有合并过，例如 typed 依赖另起了一套写法。

这三处都靠分层和合并写法解决，不需要新能力。凡是"顺便加一项能力"或"顺便改一条语义"的想法，都不在本方案里（第四节）。

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

### 对其他语言提供服务

和现在一样：类型上标 `#[rutis::service]`，照常 `provide`，名字来自类型（或 `#[service(name = "…")]`）：

```rust
pub struct Calendar { /* … */ }

#[rutis::service]                     // 对外名字 "calendar"
impl Calendar {
    pub async fn add(&self, title: String) -> u64 { /* … */ }
}

impl Plugin for Calendars {
    type Deps = ();

    async fn apply(&self, ctx: &Ctx, _: ()) -> Result {
        ctx.provide(Calendar::open(&self.path)?)?;
        Ok(())
    }
}
```

```python
async def apply(ctx, config):
    await ctx.use("calendar").add("standup")

plugin = define_plugin(apply, inject=["calendar"])
```

同一个服务要多份时，用 loader 的实例：每个实例里各装一份，实例里的行用自己那份（[Rust 服务自动对外](design-rust-service-exposure-2026-10-21.md) R3，不变）。

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
        ctx.listen::<Created>().prefix("session/").once().on(|_, call| async move {
            println!("first {}", call.name());
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

`ctx.effect` 和现在一样：先执行闭包，再把它返回的东西登记为清理。区别只是清理写成一个 future，不再包进 `Effect`：

```rust
impl Plugin for Syncer {
    type Deps = Arc<dyn Store>;

    async fn apply(&self, ctx: &Ctx, store: Arc<dyn Store>) -> Result {
        let file = tokio::fs::File::open(&self.path).await?; // io::Error 直接 `?`
        ctx.effect(|| {
            let stop = ctx.clone();
            let task = tokio::spawn(async move {
                let mut tick = tokio::time::interval(Duration::from_secs(60));
                loop {
                    tokio::select! {
                        _ = stop.cancelled() => break,          // 插件停下
                        _ = tick.tick() => { let _ = store.flush().await; }
                    }
                }
            });
            async move {
                let _ = task.await;                            // 停下时等它退出
                drop(file);
                Ok(())
            }
        })?;
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

`rutis` 根上 9 个：

| 名字 | 是什么 |
| --- | --- |
| `Plugin` | 唯一的插件 trait |
| `Deps` | `type Deps` 能写什么（`()`、`Arc<T>`、`Option<Arc<T>>`、它们的元组）；作者不实现它 |
| `Ctx` | 见下表 |
| `Event` | 事件类型 |
| `Disposer` | `provide`、`on`、`effect` 返回的句柄，用来提前撤销 |
| `Error`、`Result` | 唯一的错误类型 |
| `service` | `#[rutis::service]`，让服务对其他语言公开 |
| `serde` | 给事件派生 `Serialize` / `Deserialize` |

`rutis::events` 里 9 个，都出现在事件的签名里：`Call`、`SyncCall`、`Mode`、`ListenerInfo`、`Scope`，`ctx.listen()`、`ctx.filter()`、`ctx.waterfall()` 返回的构建器 `Listen`、`Filtered`、`Waterfall`，以及异步监听器闭包自动满足的 `Listener`。作者手写的只有 `Mode`，其余由编译器推断。

合计 18 个。上面的例子里作者手写的是 `Plugin`、`Ctx`、`Result`、`Event`、`Mode`、`serde` 这 6 个，对其他语言提供服务时再加 `service`。`ctx.plugin(child)` 返回 `host::FiberView`，装子插件的作者会见到它。

`Ctx` 上给插件用的方法有 15 个：

| 组 | 方法 |
| --- | --- |
| 服务 | `provide`、`provide_as`、`get` |
| 生命周期 | `effect`、`cancelled`、`plugin` |
| 事件 | `on`、`on_sync`、`listen`、`emit`、`parallel`、`serial`、`waterfall`、`bail`、`filter` |

和 Python 对照：

| 要做的事 | Python | Rust |
| --- | --- | --- |
| 定义插件 | `define_plugin(apply, inject=[…])` | `impl Plugin`：`type Deps` + `async fn apply` |
| 依赖 | `inject=["llm"]`，`ctx.use("llm")` | `type Deps = Arc<dyn Llm>`，作为 `apply` 的参数传入 |
| 提供服务 | `ctx.provide("weather", obj)` | `ctx.provide(value)`；trait 对象用 `ctx.provide_as::<dyn T>(arc)` |
| 清理 | `ctx.effect(cleanup)` | `ctx.effect(\|\| { …; async move { 清理 } })` |
| 后台任务 | asyncio 任务 | tokio 任务，在 `effect` 里起、在它的清理里等 |
| 监听 | `ctx.on(name, fn, prepend=…)` | `ctx.on::<E>(…)`、`ctx.on_sync::<E>(…)`、`ctx.listen::<E>()` |
| 发出 | `emit` / `parallel` / `serial` / `waterfall` / `bail` | 同名 |
| 过滤 | `ctx.filter(keep)` | `ctx.filter(\|info\| …)` |
| 配置 | `apply(ctx, config)` | 结构体字段 + `new(&Config)` |

Python 的 `use` 在 Rust 里由 `Deps` 代替。Rust 的 15 个方法里有 6 个是 Python 没有的，每个都对应一处语言差异：

- `provide_as`：Rust 要写明 trait 对象的类型；
- `get`：可选读取，Python 的 `use` 只读声明过的依赖；
- `cancelled`：Rust 的取消写法（Python 是任务取消，Go 是 `ctx` 本身）；
- `on_sync`：Rust 不能在运行时分辨闭包是同步的还是异步的；
- `listen`：Rust 没有关键字参数；
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
- `type Deps` 必须写，没有依赖时写 `()`。关联类型的默认值在稳定版 Rust 里还不能用，这一行省不掉（第四节）。
- `name` 默认是类型名的最后一段（`Greeter`）；loader 的行有自己的 id。
- `validate` 在装载时、`apply` 之前执行；失败时插件进入 Failed，不执行 `apply`。
- 这就是现在的 `TypedPlugin`，去掉了 `BoxFuture` 和 `Effect`。门控、驱逐、重载、卸载期不可见这些规则都和现在一样。

插件作者只需要知道三件事：

1. `apply` 在 `Deps` 里所有必需的服务都就绪之后执行；可选的服务不等。
2. `apply` 里经 `ctx` 登记的一切（服务、监听器、清理、子插件）在插件停下时按登记的相反顺序撤销；`apply` 失败时，已经登记的部分同样撤销。
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
- `Deps` 是密封的 trait，作者不实现它。
- 门控之后、`apply` 读取之前依赖被撤销的，回到 Pending，与现在相同。
- 按名字（或运行期才定的键）依赖的插件，用宿主层的 `host::DynPlugin`：在 `injects()` 里列出 `Key::named(…)`，在 `apply` 里用 `require_at` 读（附录 A 有例子）。这和 rutis-bridge 今天按名字依赖传输、身份的写法相同。能力不变，只是不在默认路径上：内核之外还没有插件按名字依赖。

### 服务

- `ctx.provide(value)`：以 `value` 自己的类型提供。
- `ctx.provide_as::<dyn T>(arc)`：以 trait 对象的类型提供，例如 `ctx.provide_as::<dyn Llm>(Arc::new(MyLlm))`。
- `ctx.get::<T>()`：可选读取，`T` 可以是 trait 对象。它不门控，也不建立重载关系，诊断里记为未声明的访问。用在"有就用"的场合；需要的服务写进 `Deps`。
- 按名字的读写（`provide_at`、`get_at`、`require_at`）和严格读取 `require`，移到 `rutis::host`；带 `check()` 的提供、可写绑定移到 `rutis::internal`（附录 A）。
- 对其他语言公开的规则都不变（[Rust 服务自动对外](design-rust-service-exposure-2026-10-21.md) R1–R7）：带标注的类型以类型的名字公开，带名字的绑定不对外。

### 清理

- `ctx.effect(|| { …; async move { … } })`：立即执行闭包，把它返回的 future 登记为清理。插件停下时，清理和其他登记一起按相反顺序执行。
- 插件已经在停下或已停下时，`effect`（以及 `on`、`provide` 等所有登记）照旧返回 `InactiveEffect`，闭包不执行。正因为闭包不执行，被拒时什么资源都还没拿到，不会泄漏。这是 Rust 的 `effect` 保持"先执行、再登记"的原因：它与 Cordis 的 `ctx.effect` 相同；其他语言的 `effect(cleanup)` 只登记清理。
- 不需要清理时不调用 `effect`；多个清理就调用多次。清理返回的错误照旧汇集（`take_cleanup_errors`）。
- 后台任务用 tokio 自己的 `tokio::spawn`（需要成组管理时用 `JoinSet`）：在 `effect` 的闭包里起，在它返回的清理里等它退出；任务里看 `ctx.cancelled()`，插件开始停下时它就完成。停下的过程等清理完成，最多到关闭截止时间，与现在相同。
- `ctx.cancelled()` 也给执行时间长的监听器调用用；`ctx.plugin(child)` 装一个随这个插件一起停下的子插件，返回 `host::FiberView`。

### 事件

- 监听：
  - `ctx.on::<E>(|event, call| async move { … })` 注册异步监听器；
  - `ctx.on_sync::<E>(|event, call| …)` 注册同步监听器，所有方式都能调用它；
  - 需要选项时用 `ctx.listen::<E>()`：`.prefix(p)`、`.name(n)`、`.once()`、`.prepend()`、`.global()`，最后写 `.on(…)` 或 `.on_sync(…)`。插件指南先讲 `prefix` 和 `name`，后三个放在"高级"一节。
- 监听器是闭包，参数是 `(Arc<E>, Call<E>)`；同步监听器的参数是 `(Arc<E>, SyncCall<E>)`。返回 `Result<Option<E::Value>>`，`Ok(None)` 表示没有结果。闭包的参数类型由编译器推断，块里可以对任何错误用 `?`。
- `call.mode()` 是这次分发的方式；`call.name()` 是分发时用的名字（前缀监听器用得到）；`call.next()` 只在 `waterfall` 里可用；`call.ctx()` 是登记这个监听器的 `Ctx`。
- 发出：
  - `emit`、`parallel`、`serial`、`waterfall`、`bail` 都接受 `E` 或 `Arc<E>`；
  - `waterfall` 返回一个构建器，可以先写 `.terminal(…)` 再 `.await`；
  - `bail` 是原来的 `bail_sync`，改成和其他语言同名；
  - `ctx.filter(pred)` 给出同样这五个方法。
- 名字：事件以 `E::NAME` 登记和发出。没有固定名字的事件类型（例如 rutis-bridge 的 `Args`）重写 `Event::name(&self)`，让值自己带上名字；监听这种事件时用 `ctx.listen::<Args>().name("file-changed")`。这只是代替了现在的 `EventKey::dynamic`。
- 五种方式的语义、作用域、过滤、卸载等待，都和[统一事件设计](design-events-2026-10-18.md)一致，不变。

### 错误

- 所有签名里只有一种错误类型 `rutis::Error`，`rutis::Result<T = (), E = Error>`。
- `?` 能把任何 `std::error::Error + Send + Sync + 'static` 转成 `rutis::Error`，rutis 自己的错误也一样。`Error::msg("…")` 用字符串构造一个错误。
- `rutis::Error` 本身不实现 `std::error::Error`，这是 `?` 能接住任何错误的前提（`anyhow::Error` 和 Bevy 的 `BevyError` 也是这么做的）。
- 宿主要区分 rutis 的失败时，`err.kind()` 返回 `Option<&host::ErrorKind>`。`ErrorKind` 就是现在的 `CordisError` 枚举，改了名字，变体不变。来自插件代码的错误，`kind()` 是 `None`；它报到宿主时被包在 `ErrorKind::PluginFailed` 里，与现在相同。
- 错误仍然不能 `Clone`，要共享时用 `Arc<Error>`（D11、D25 不变）。

和常用错误库一起用（Rust 1.88、anyhow 1.0.104、thiserror 2.0.21 上实测）：

| 场景 | 写法 |
| --- | --- |
| 宿主返回 `Box<dyn Error>` 或 `Box<dyn Error + Send + Sync>` | 直接 `?` |
| 自己的 thiserror 错误类型包住 rutis 的错误 | `#[error(transparent)] Rutis(#[from] rutis::Error)` 或 `#[source] cause: rutis::Error`，都可以 |
| 插件代码里对自己的 thiserror 错误 | 直接 `?` |
| 宿主返回 `anyhow::Result` | 打开 rutis 的 `anyhow` feature 后直接 `?` |
| 插件代码里遇到 `anyhow::Error` | `.map_err(Error::from_anyhow)?`（`anyhow` feature）。`anyhow::Error` 不实现 `std::error::Error`，Rust 的一致性规则不允许再给它单独的 `From`（E0119），这里写不成 `?` |

### 三层

| 层 | 给谁 | 文档 | 稳定性 |
| --- | --- | --- | --- |
| `rutis`（根）和 `rutis::events` | 插件作者 | 插件指南 | 随版本说明变更 |
| `rutis::host` | 嵌入 rutis 的应用、测试 | 宿主指南 | 同上 |
| `rutis::internal` | rutis-bridge、rutis-loader、rutis-dylib、rutis-sdk、rutis-macros、rutis-host | `#[doc(hidden)]` | 不承诺；仓库外有人需要时，提 issue 把它移到 `host` |

- 划分规则：写插件用的放根；应用嵌入 rutis 时要用的放 `host`；仓库里除了 rutis 自己的 crate 之外没人用的放 `internal`。
- `Ctx` 的方法也按这个规则分开：`host` 和 `internal` 的方法放在扩展 trait 里（`host::CtxHost`、`internal::CtxInternal`），要先 `use rutis::host::*` 才能调用。rustdoc 把扩展 trait 的方法放在 `Ctx` 页面下单独的一节。
- rutis-loader 也这样分：根上留下组装加载器的 11 个名字，其余按主题放进模块，只有 rutis-host 和测试用的放进 `internal`。

每一层的完整清单、现在每个名字的去向见附录 A；迁移工具、宿主的改前改后和实施步骤见附录 B。

## 四、取舍

本方案只分层、合并写法。下面几项都考虑过，因为属于"加能力"或"改语义"而不做；需要时各自开 issue，按具体的使用方设计：

- **`ctx.spawn`（插件拥有的后台任务）**：任务归谁、结束后怎么清、停下时等不等，每一条都是新语义。现有的 `effect` 加 `cancelled()` 已经能写，只是多几行（第二节）。
- **插件停下时登记 `effect`，也保证清理执行**：现在的规则是拒绝、闭包不执行。只要 `effect` 保持"先执行、再登记"，被拒时就没有资源要收，不需要新规则。
- **带名字的 Rust 服务对其他语言公开**：这是新能力，要改 R2，还牵扯 `rutis-host check` 怎样认识从配置来的名字。
- **插件层的按名字依赖（`Keyed<T>` 加 `keys()`）**：要新设计名字从哪来、忘写怎么查、配置更新时能不能变。内核之外没有使用方，能力在宿主层（`host::DynPlugin` 加 `host::Key`）保留。

其余取舍：

- **只加 `rutis::prelude`，其他不动**：`Ctx` 和 `EventBus` 上的方法不会因为多一个 prelude 而变少，文档里照样是 40 个和 28 个。所以根本身就当 prelude 用，不再另设一个模块。
- **新写法当默认，旧写法也保留**：这样同一件事又多了一种写法，#617 说的问题正是写法太多。所以 0.17 一次改完，不保留旧名字；迁移交给工具（附录 B）。
- **`#[rutis::plugin]` 宏**：`async fn` 已经能写在 trait 里，`BoxFuture` 不用宏也能去掉。加了宏，作者看到的就不是真正的 trait，出错时的提示也更难懂。
- **`type Deps = ()` 省掉**：没有依赖的插件也要写这一行和 `_: ()` 参数。省掉它要么等关联类型默认值稳定，要么再分出一个"没有依赖的插件" trait；后者正是要去掉的那种平行写法。所以保留这一行。
- **把配置做成关联类型、`apply(ctx, config, deps)`（Cordis / Python 的写法）**：那样每个插件都要多写一个关联类型，没有配置的插件装起来也得写 `ctx.plugin(P, ())`。在 Rust 里，配置本来就写在结构体的字段上；配置变了重建插件，是装插件那一方的事。
- **`effect(cleanup)` 只登记清理（其他语言的写法）**：插件停下时登记会被拒绝；如果资源在调用 `effect` 之前就拿到了，被拒时它就没人收。保持 Cordis 的"先执行、再登记"，规则不用改。
- **用一个 `on` 同时接受同步和异步闭包**（axum、Bevy 那样的标记类型技巧）：试过。候选实现有两个，编译器就推断不出闭包参数的类型，每个监听器都得写成 `|e: Arc<Created>, call: Call<Created>|`。分成两个方法以后，`on::<E>(|e, call| async move { …? … })` 不用写任何类型标注（两种写法都在 Rust 1.88 上验证过）。
- **合并 `Call` 和 `SyncCall`，凑够 15 个名字**：同步监听器不能 `.await`，异步监听器不在同步方式里被调用；合并以后，用错的地方要到运行时才报错。所以不为凑数合并（第五节）。
- **`get` 改名 `get_optional`**：返回 `Option` 已经说明它是可选的。真正的坑是"不会重载"，这一点由诊断里的"未声明访问"提示。
- **一个 `dispatch(mode, …)` 代替五个分发方法**：五种方式的返回值不同（不等待、汇总错误、第一个结果、链式、同步），合成一个以后，要么丢掉类型，要么返回一个枚举让调用方再拆。
- **`rutis::Error` 也实现 `std::error::Error`**：这样就和 `impl<E: std::error::Error> From<E>` 冲突，`?` 又接不住任意错误了，作者只能回到 `map_err`。

## 五、验收

对照 #617 的验收项：

| #617 | 本方案 |
| --- | --- |
| 插件作者默认看到的入口 ≤ 15 个 | 根上 9 个 + `rutis::events` 9 个，共 18 个，没有达到。多出来的是事件签名里由编译器推断的类型；作者在例子里手写的是 6 个。建议把这一项改成"作者手写的名字 ≤ 10，根和 `rutis::events` 的公开面由快照锁住"，与 #617 里"写一个普通插件实际只需要约 10–12 个名字"是同一个口径；这一改动由 #617 决定 |
| 最小的 Rust 插件不手写 `BoxFuture` 和生命周期 | 第二节的例子；README、指南、示例都用这种写法 |
| `EventBus` 的公开方法 ≤ 10 个 | 插件只用 `Ctx` 上的 9 个事件方法，`EventBus` 移进 `internal` |
| 服务的 provide / read 方法 ≤ 5 个 | 3 个：`provide`、`provide_as`、`get` |
| `rutis-loader` 有明确的 prelude 和不稳定边界 | 根上 11 个名字，其余按模块分组，`internal` 不承诺稳定 |
| 文档第一页有"只需要这些"的清单 | 新增 `docs/guide/rust-plugin.md`（写一个 Rust 插件），指南首页加一行 |

另外：

- 用 `cargo public-api` 给 `rutis` 根、`rutis::events` 和 `rutis::host` 生成公开面快照，提交进仓库，CI 比对。以后加一个名字就要改快照，评审时看得见，公开面不会再悄悄变大。
- `examples/` 和指南里的 Rust 代码不出现 `rutis::internal`，由 CI 用 grep 检查。
- `docs/guide/rust-plugin.md` 和 README 的 Rust 代码作为 doctest 编译，与现在开发手册的做法相同。
- 语义不变由现有测试保证：内核、loader、bridge 现有的测试迁移后全部通过，[保证](guarantees.md) 里每一条的测试照旧绑定。
- 新写法各写一个测试：`?` 接住 `io::Error`；不写类型标注的监听器闭包能编译；`effect` 在插件停下时返回 `InactiveEffect`、闭包不执行；`DynPlugin` 与 `Plugin` 的插件能互相依赖；第三节错误表里的五种场景。
- 迁移工具先迁移仓库自己的 crate，再在 0.16 的 `examples/` 和 aimux-llm 上单独跑一遍（附录 B）。
- 重新跑一遍事件分发的基准（[事件分发性能](performance-event-dispatch-2026-09-27.md)）。`Call` 改成拥有所有权以后，每次调用多一次 `Arc` 克隆，要把前后的数字写进 PR。

## 附录 A：名字去向

### `rutis::host`

| 内容 | 名字 |
| --- | --- |
| 根与关闭（`CtxHost`） | `Ctx::root`、`root_with`、`root_with_sink`、`root_view`、`root_ctx`、`view`、`shutdown`、`shutdown_with_timeout`、`is_shutting_down`、`handle`、`error_sink`、`take_cleanup_errors`、`diagnostics` |
| 按键读写和严格读取（`CtxHost`） | `provide_at(Key<T>, arc)`、`get_at(&key)`、`require::<T>()`、`require_at(&key)` |
| 作用域与实例（`CtxHost`） | `isolate`、`isolate_label`、`instance`、`scope`、`row`、`dispose_self`、`cancellation_token` |
| 装载（`CtxHost`） | `plugin_from(build, config)`（类型化的配置闭包，依赖从 `P::Deps` 得出）、`plugin_with(factory, config)` |
| 事件（`CtxHost`） | `waterfall_sync`、`subscriptions` |
| 插件的对象安全形式 | `DynPlugin`、`PluginFactory`、`Effect`、`EffectMeta`、`EffectPhase`、`BoxFuture` |
| fiber | `FiberView`、`FiberState`、`FiberStatusChanged`、`PluginId`、`Snapshot`、`DisposeWaitError`、`shutdown_deadline` |
| 键 | `Key<T>`、`TypeKey`、`InstanceId` |
| 错误 | `ErrorKind`、`ServiceReadError`、`ServiceReadFailure`、`NextMisuse`、`CallError`、`ErrorSink`、`default_sink` |
| 诊断 | `host::diagnostics::*`（现在的 10 个诊断类型） |
| 事件描述 | `EventSubscription`、`ListenerKind` |

- **`DynPlugin`** 就是现在的 `Plugin` trait，改了名字，只有一处不同：`injects()` 返回 `Vec<TypeKey>`，不再返回借用的切片。rutis 为每个 `Plugin` 提供 `impl<P: Plugin> DynPlugin for P`，所以 `ctx.plugin(…)` 只有一个，什么插件都能装。依赖要在运行期才定下来的插件（按名字依赖的、rutis-loader 的 `JsFactory`）、要放进 `Vec<Box<dyn DynPlugin>>` 的插件列表，用的都是 `DynPlugin`。迁移工具也靠它：旧的 `impl Plugin` 改名成 `DynPlugin` 就能编译（附录 B）。
- **`Key<T>`** 合并了现在的 `Key<T>` 和 `DepKey<T>`：`Key::of()`、`const fn Key::named("primary")`、`Key::dynamic(name)`、`.instance(id)`，`.erase()` 得到 `TypeKey`。键带着值的类型，`provide_at` 的类型不匹配是编译错误，现在要到运行时才报。
- **`ServiceKey`** 删除，它只是 `TypeKey` 的别名。

按名字依赖两个同类型的服务（两个插件各自以 `openai`、`local` 提供 `dyn Llm`）：

```rust
use rutis::host::*;

const OPENAI: Key<dyn Llm> = Key::named("openai");

struct Router {
    local: Key<dyn Llm>,              // Key::dynamic(配置里的名字)
}

impl DynPlugin for Router {
    fn name(&self) -> &str { "router" }
    fn injects(&self) -> Vec<TypeKey> { vec![OPENAI.erase(), self.local.erase()] }
    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect>> {
        Box::pin(async move {
            let smart = ctx.require_at(&OPENAI)?;
            let fast = ctx.require_at(&self.local)?;
            /* 两个都就绪才启动；任何一个被替换就重启 */
            Ok(Effect::Done)
        })
    }
}
```

提供方用 `ctx.provide_at(Key::named("openai"), arc)`。

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
| `Key`、`DepKey` | `host::Key<T>`（两者合并） |
| `Keyed`、`KeyedGate` | 删除；按名字的依赖用 `host::DynPlugin` + `require_at`（本附录的例子） |
| `Gate` | 删除；写 `Arc<T>` 而不使用它 |
| `BoxFuture` | `host::BoxFuture` |
| `Effect`、`EffectMeta`、`EffectPhase` | `host::`（`ctx.effect` 的闭包改为返回 future） |
| `Disposer` | `Disposer` |
| `Ctx` | `Ctx`（方法按第二节和本附录分开） |
| `TypeKey`、`InstanceId` | `host::` |
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
| `Event` | `Event`（增加 `fn name(&self)`，默认返回 `NAME`，代替 `EventKey::dynamic`） |
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
| `provide_as(key, arc)` | 不带名字：`provide_as::<T>(arc)`；带键：`host::provide_at(Key<T>, arc)` |
| `get_as(key)` | 不带名字：`get::<T>()`；带键：`host::get_at` |
| `effect(f)` | `effect(f)`，`f` 改为返回清理的 future，执行时机和停止时的规则不变 |
| `require`、`require_as` | `host::require`、`host::require_at` |
| `root`、`root_with`、`root_with_sink`、`root_view`、`root_ctx`、`view`、`shutdown`、`shutdown_with_timeout`、`is_shutting_down`、`handle`、`error_sink`、`take_cleanup_errors`、`diagnostics`、`isolate`、`isolate_label`、`instance`、`scope`、`row`、`dispose_self`、`cancellation_token`、`plugin_with`、`plugin_from` | `host::CtxHost` |
| `events` | `internal::CtxInternal`；事件方法在 `Ctx` 上 |
| `mark_scope`、`mark_row`、`provide_as_with_check`、`provide_mut_as`、`require_provided_as`、`effect_named`、`refresh`、`in_instance_key` | `internal::CtxInternal`（两个改名：`provide_checked`、`provide_mut_at`） |
| 新增 | `on`、`on_sync`、`listen`、`emit`、`parallel`、`serial`、`waterfall`、`bail`、`filter`（从 `EventBus` 搬过来） |

### `EventBus` 现在的 28 个方法

| 现在 | 0.17 |
| --- | --- |
| `on`、`on_opt`、`once`、`on_sync`、`on_sync_opt`、`on_pattern`、`on_pattern_opt`、`on_sync_pattern`、`on_sync_pattern_opt` | `ctx.on`、`ctx.on_sync`、`ctx.listen::<E>()`（`.prefix()`、`.name()`、`.once()`、`.prepend()`、`.global()`） |
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

0.17 一次改完，不保留旧名字，和 0.13 的统一事件做法相同。不用 `#[deprecated]` 别名过渡：留着旧名字，旧名字就还会出现在文档和补全里，这正是 #617 要去掉的。

本方案不改运行时语义，所以迁移后能编译的代码，行为和 0.16 相同。迁移分两步：先用迁移工具让 0.16 的代码在 0.17 上编译通过（旧插件以 `host::DynPlugin` 的身份照常运行），再按需要把插件换成新写法。

### 迁移工具 `rutis-migrate`

新增一个随发布列车发布的二进制 crate：

```bash
cargo install rutis-migrate --version 0.17.0
```

```bash
rutis-migrate 0.17 path/to/crate
```

- 它用 `syn`（打开行列位置）找到要改的地方，在原文上做文本替换，注释和格式都保留；改完运行 `cargo check`，把剩下的错误位置列出来。
- 默认先显示差异，`--write` 才写回文件。
- 它不是全自动的：改不了的地方插入 `// TODO(rutis 0.17): …` 注释，编译错误会指向它们。下表是实现时的目标，实际覆盖以发布说明为准。

| 改动 | 自动 |
| --- | --- |
| `CordisError` 在类型位置改成 `Error`；`.map_err(\|e\| CordisError::PluginFailed(e.into()))?` 改成 `?`；构造 `CordisError::PluginFailed(…)` 改成 `Error::from(…)` | 是 |
| 旧的 `impl Plugin` 改成 `impl host::DynPlugin`，`injects` 的签名改成返回 `Vec<TypeKey>`（函数体包一层 `.to_vec()`） | 是 |
| `impl TypedPlugin`：去掉 `BoxFuture` 和 `Box::pin`，改成 `async fn apply`；末尾的 `Ok(Effect::Done)` 改成 `Ok(())`；`Typed::new(p)` 改成 `p` | 是 |
| `ctx.effect(\|\| { …; Effect::Disposer(Box::new(move \|\| B)) })` 改成 `ctx.effect(\|\| { …; async move { B } })`；`AsyncDisposer`、`Done` 同理 | 是（`Effect::Many` 留 TODO） |
| `ctx.events().on(&ctx, &key, \|c, e, call\| Box::pin(async move { … }))` 改成 `ctx.on::<E>(\|e, call\| async move { … })`，块里用到的 `c` 改成 `call.ctx()`；`on_sync`、`on_pattern`、`on_opt` 同理；`E` 取自 `EventKey::<E>` 或键变量的类型 | 是（键的类型推不出来时留 TODO） |
| `emit` / `parallel` / `serial`（`&ctx, &key, Arc::new(v)`）改成 `ctx.emit(v)` 等；`bail_sync` 改成 `bail`；`waterfall(&ctx, &key, &e, t)` 改成 `ctx.waterfall(e).terminal(t)` | 是 |
| `provide_as(TypeKey::of::<T>(), arc)` 改成 `provide_as::<T>(arc)`，带名字的键改成 `provide_at(…)`；`get_as(TypeKey::of::<T>())` 改成 `get::<T>()` | 是 |
| 用到 `root`、`shutdown`、`require` 等宿主方法的文件加 `use rutis::host::*;`；rutis-loader 根上移走的名字改成模块路径 | 是 |
| `Builtins::register_fn(name, \|c\| Ok(Box::new(P::new(c))))` 改成 `register(name, \|c\| Ok(P::new(c)))` | 是 |
| 手写 `impl Listener` 的结构体、`match` `CordisError` 的变体（要改成 `match err.kind()`）、`Gate` / `Keyed` / `KeyedGate` / `Typed::with_keys` | 否，留 TODO |

验证：仓库自己的 crate 就用这个工具迁移（内核之外 42 处 `impl Plugin`、14 处 `impl PluginFactory`、1277 处 `CordisError`），再在 0.16 的 `examples/` 和 aimux-llm 上单独跑一遍。结果要能编译、测试通过，剩下的 TODO 数量写进发布说明。

### 写 Rust 插件的人

工具跑完以后，把插件换成新写法时：

1. 把 `impl DynPlugin` 改成 `impl Plugin`：`injects()` 里列的类型移进 `type Deps`，`apply` 里对应的 `ctx.require::<T>()` 改成参数。按名字依赖的插件保持 `DynPlugin`。
2. `apply` 返回的 `Effect` 改成在 `apply` 里调用 `ctx.effect`。

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

### 嵌入 rutis 的应用

```rust
// 0.16
use std::sync::Arc;
use rutis::{CordisError, Ctx, EventKey, Plugin, PluginFactory, TypeKey, Typed};
use rutis_loader::{Builtins, Chain, Layer, LoaderOptions, LoaderPlugin, ReconcileReport, RuntimeResolver};

struct WeatherFactory;

impl PluginFactory<WeatherConfig> for WeatherFactory {
    fn injects(&self) -> &[TypeKey] { &[] }
    fn build(&self, config: &WeatherConfig) -> Result<Box<dyn Plugin>, CordisError> {
        Ok(Box::new(Weather::new(config)?))
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = Ctx::root()?;
    let mut builtins = Builtins::new();
    builtins.register("weather", WeatherFactory);
    root.plugin(Typed::new(Printer));
    let weather = root.plugin_with(WeatherFactory, config);
    weather.update(new_config).await?;
    let llm = root.require_as::<dyn Llm>(TypeKey::of::<dyn Llm>())?;
    root.events().emit(&root, &EventKey::<Created>::of(), Arc::new(Created("s1".into(), 3)))?;
    let report: ReconcileReport = handle.reconcile(layers, None).await?;
    root.shutdown().await?;
    Ok(())
}

// 0.17
use rutis::host::*;                                   // Ctx::root、require、shutdown 等
use rutis::Ctx;
use rutis_loader::report::ReconcileReport;
use rutis_loader::runtime::RuntimeResolver;
use rutis_loader::{Builtins, Chain, Layer, LoaderOptions, LoaderPlugin};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = Ctx::root()?;
    let mut builtins = Builtins::new();
    builtins.register("weather", Weather::new);       // 不再写工厂
    root.plugin(Printer);                             // 不再写 Typed::new
    let weather = root.plugin_from(Weather::new, config);
    weather.update(new_config).await?;
    let llm = root.require::<dyn Llm>()?;
    root.emit(Created("s1".into(), 3))?;
    let report: ReconcileReport = handle.reconcile(layers, None).await?;
    root.shutdown().await?;
    Ok(())
}
```

依赖要在运行期才定下来的工厂，照旧实现 `host::PluginFactory`，用 `plugin_with` 或 `Builtins::register_raw` 装。

**仓库里的 crate** 在同一个版本里一起改：rutis-bridge、rutis-loader、rutis-dylib、rutis-sdk、rutis-macros、rutis-host、rutis-dsh、rutis-dev、rutis-xtask、aimux-llm，以及示例 rutis-agent、rutis-cli 和 `examples/`。工具跑完之后，只有 aimux-llm 和示例手工换成新的 `Plugin`，因为它们就是给插件作者看的写法；其余保持 `DynPlugin`。内核里要重写的只有插件包装、`effect` 的闭包形状和事件方法的搬家，语义都不动。

其他语言的 SDK、运行时、协议、`rutis.json` 都不受影响。

### 实施

在一个集成分支上按下面的顺序提交，整体合入，随 0.17 发布：

1. 内核：新的 `Plugin` / `Deps` / `DynPlugin` 和 blanket 实现；`Error` / `ErrorKind` 和 `anyhow` feature；`Ctx` 上的服务方法、`effect` 的新闭包形状和事件方法（`Call` 改成拥有所有权）；`host::Key<T>`；建立 `host` / `internal` 两个模块和扩展 trait；删除附录 A 列出的名字。
2. `rutis-migrate`，先用它迁移第 3、5 步的 crate。
3. rutis-bridge、rutis-loader、rutis-dylib、rutis-sdk、rutis-macros 改用 `DynPlugin` 和 `internal`；`Builtins::register`、`export_plugin!(build:)` 改用类型化的构造函数。
4. rutis-loader 的根和模块重新分组。
5. rutis-host、rutis-dsh、rutis-dev、rutis-xtask、aimux-llm、示例（rutis-agent、rutis-cli、`examples/`）。
6. 文档，中英两份：README 的 Rust 部分、新增 `guide/rust-plugin.md`、`guide/README.md`、`guide/rust-host.md`、`guide/plugin-api.md` 的 Rust 部分、开发手册、`migration-0.17.md`。
7. 公开面快照和 CI 检查。
