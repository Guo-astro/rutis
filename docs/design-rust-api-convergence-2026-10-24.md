# Rust API 收敛：写插件只有一条路

[English](design-rust-api-convergence-2026-10-24.en.md)

状态：设计（2026-10-24），#617，目标版本 0.17。只改 Rust 公开 API；协议和其他语言的 SDK 不变，运行时行为只改两处：按名字提供的 Rust 服务对其他语言公开，插件停下时等待它起的任务。

## 一、目标

> 写 Rust 插件的人只学一种插件写法、四个服务方法，以及和其他语言同名的事件方法。最小的插件不出现 `BoxFuture`、生命周期参数和 `Effect`。宿主用的东西放进 `rutis::host`，rutis 自己的 crate 之间用的东西放进隐藏的 `rutis::internal`。能力一项不少，只是默认路径变短；迁移由工具完成大部分。

Python 插件已经做到了：同样的门控、重载、清理、跨进程和五种事件方式，`Context` 只有 10 个方法。所以 Rust 的公开面大（根上 71 个名字、`Ctx` 40 个方法、`EventBus` 28 个方法、四种插件 trait），原因不在于跨语言、跨进程，而是三处可以修的问题：

- 宿主和 rutis 自己的 crate 要调用的方法，挂在了插件作者用的对象上；
- 绕 Rust 语言限制时用了最笨的办法，例如每个 `on*` 配一个 `_opt`、让作者手写 `BoxFuture`；
- 每加一项能力就多一个名字，从来没有合并过，例如 typed 依赖另起了一套写法。

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

### 同一类型的两个服务，也给其他语言用

提供方用 `provide_at` 加一个名字；名字可以写死，也可以来自配置：

```rust
use std::sync::Arc;
use rutis::{Ctx, Key, Keyed, Plugin, Result};

pub struct LlmGateway { /* … */ }

#[rutis::service]                     // 方法表；不带名字提供时，对外名字是 "llmGateway"
impl LlmGateway {
    pub async fn ask(&self, prompt: String) -> String { /* … */ }
}

struct Gateways {
    local: String,                    // 来自配置，例如 "local"
}

impl Plugin for Gateways {
    type Deps = ();

    async fn apply(&self, ctx: &Ctx, _: ()) -> Result {
        ctx.provide_at(Key::named("openai"), Arc::new(LlmGateway::openai()))?;
        ctx.provide_at(Key::dynamic(self.local.clone()), Arc::new(LlmGateway::local()))?;
        Ok(())
    }
}
```

Rust 的消费方用 `Keyed<T>` 依赖，名字由 `keys()` 给出：

```rust
struct Router {
    local: String,
}

impl Plugin for Router {
    type Deps = (Keyed<LlmGateway>, Keyed<LlmGateway>);

    fn keys(&self) -> (Key<LlmGateway>, Key<LlmGateway>) {
        (Key::named("openai"), Key::dynamic(self.local.clone()))
    }

    async fn apply(&self, _: &Ctx, (smart, fast): Self::Deps) -> Result {
        /* 两个都就绪才启动；任何一个被替换就重启 */
        Ok(())
    }
}
```

其他语言按名字使用，和使用任何服务一样：

```python
def apply(ctx, config):
    fast, smart = ctx.use("local"), ctx.use("openai")

plugin = define_plugin(apply, inject=["openai", "local"])
```

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

```rust
impl Plugin for Server {
    type Deps = Arc<dyn Store>;

    async fn apply(&self, ctx: &Ctx, store: Arc<dyn Store>) -> Result {
        let listener = tokio::net::TcpListener::bind(&self.addr).await?; // io::Error 直接 `?`
        ctx.spawn(|ctx| async move {
            loop {
                tokio::select! {
                    _ = ctx.cancelled() => return Ok(()),           // 插件停下：正常结束
                    accepted = listener.accept() => {
                        let (conn, _) = accepted?;
                        let store = store.clone();
                        ctx.spawn(move |_| serve(conn, store))?;    // 每个连接也归插件所有
                    }
                }
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

`rutis` 根上 11 个：

| 名字 | 是什么 |
| --- | --- |
| `Plugin` | 唯一的插件 trait |
| `Deps` | `type Deps` 能写什么；作者不实现它 |
| `Ctx` | 见下表 |
| `Event` | 事件类型 |
| `Key`、`Keyed` | 带名字的服务：`Key<T>` 是键，`Keyed<T>` 是按键找到的依赖 |
| `Disposer` | `provide`、`on`、`effect`、`spawn` 返回的句柄，用来提前撤销 |
| `Error`、`Result` | 唯一的错误类型 |
| `service` | `#[rutis::service]`，让服务对其他语言公开 |
| `serde` | 给事件派生 `Serialize` / `Deserialize` |

`rutis::events` 里 9 个，都出现在事件的签名里：`Call`、`SyncCall`、`Mode`、`ListenerInfo`、`Scope`，`ctx.listen()`、`ctx.filter()`、`ctx.waterfall()` 返回的构建器 `Listen`、`Filtered`、`Waterfall`，以及异步监听器闭包自动满足的 `Listener`。作者手写的只有 `Mode`，其余由编译器推断。

合计 20 个。上面的例子里作者手写的是 `Plugin`、`Ctx`、`Result`、`Event`、`Key`、`Keyed`、`Mode`、`service`、`serde` 这 9 个（`Error` 只在 `Error::msg` 时才写）。

`Ctx` 上给插件用的方法有 17 个：

| 组 | 方法 |
| --- | --- |
| 服务 | `provide`、`provide_at`、`get`、`get_at` |
| 生命周期 | `effect`、`spawn`、`cancelled`、`plugin` |
| 事件 | `on`、`on_sync`、`listen`、`emit`、`parallel`、`serial`、`waterfall`、`bail`、`filter` |

和 Python 对照：

| 要做的事 | Python | Rust |
| --- | --- | --- |
| 定义插件 | `define_plugin(apply, inject=[…])` | `impl Plugin`：`type Deps` + `async fn apply` |
| 依赖 | `inject=["llm"]`，`ctx.use("llm")` | `type Deps = Arc<dyn Llm>` 或 `Keyed<T>`，作为 `apply` 的参数传入 |
| 提供服务 | `ctx.provide("weather", obj)` | `ctx.provide(value)`；trait 对象或带名字时 `ctx.provide_at(key, arc)` |
| 清理 | `ctx.effect(cleanup)` | `ctx.effect(async { … })` |
| 后台任务 | asyncio 任务 | `ctx.spawn(\|ctx\| async move { … })`（Go 是 `ctx.Go`，Kotlin 是 `ctx.launch`） |
| 监听 | `ctx.on(name, fn, prepend=…)` | `ctx.on::<E>(…)`、`ctx.on_sync::<E>(…)`、`ctx.listen::<E>()` |
| 发出 | `emit` / `parallel` / `serial` / `waterfall` / `bail` | 同名 |
| 过滤 | `ctx.filter(keep)` | `ctx.filter(\|info\| …)` |
| 配置 | `apply(ctx, config)` | 结构体字段 + `new(&Config)` |

Python 的 `use` 在 Rust 里由 `Deps` 代替。Rust 的 17 个方法里有 8 个是 Python 没有的，每个都对应一处语言差异：

- `provide_at`：Rust 要写明 trait 对象的类型，名字也随键给出；
- `get`、`get_at`：可选读取，Python 的 `use` 只读声明过的依赖；
- `spawn`、`cancelled`：Rust 自己的任务和取消写法；
- `on_sync`：Rust 不能在运行时分辨闭包是同步的还是异步的；
- `listen`：Rust 没有关键字参数；
- `plugin`：Rust 应用自己装子插件。

## 三、规则

### 插件

```rust
pub trait Plugin: Send + Sync + 'static {
    type Deps: Deps;
    fn apply(&self, ctx: &Ctx, deps: Self::Deps) -> impl Future<Output = Result> + Send;
    fn keys(&self) -> <Self::Deps as Deps>::Keys { Default::default() } // 只在有 Keyed 时写
    fn name(&self) -> &str { /* 类型名的最后一段 */ }
    fn validate(&self) -> Result { Ok(()) }
}
```

- 在实现里写 `async fn apply`（Rust 1.75 起就可以，rutis 的 MSRV 是 1.88）。
- `type Deps` 必须写，没有依赖时写 `()`。关联类型的默认值在稳定版 Rust 里还不能用，这一行省不掉（第四节）。
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
| `Keyed<T>` | `keys()` 给出的键 | 服务 |
| `Option<Arc<T>>` | 无 | 装载时可见就有；之后出现或消失都不重载 |
| 至多 8 个成员的元组 | 全部成员 | 各自的值 |

- `T` 可以是 trait 对象：`Arc<dyn Llm>`、`Keyed<dyn Llm>`。
- `keys()` 按 `Deps` 的形状返回：`Keyed<T>` 对应 `Key<T>`，其他成员对应 `()`。例如 `(Arc<A>, Keyed<B>)` 的 `keys()` 返回 `((), Key::named("b"))`。
- 写了 `Keyed` 却没实现 `keys()` 时，装载失败，错误说明要在 `keys()` 里给出名字。
- 依赖声明在插件的一生里不变（D32f）。插件由配置构造时（`plugin_from`、`Builtins::register`、`export_plugin!`），名字取自第一次构造出的实例。之后 `update` 构造出的实例给出不同的名字时，这次更新在试构造阶段就失败，旧的一代继续运行。
- `Deps` 是密封的 trait，作者不实现它。
- 门控之后、`apply` 读取之前依赖被撤销的，回到 Pending，与现在相同。

### 服务

- `ctx.provide(value)`：以 `value` 自己的类型提供。
- `ctx.provide_at(key, arc)`：以键提供。`Key::of()` 用于 trait 对象（`ctx.provide_at(Key::<dyn Llm>::of(), Arc::new(MyLlm))`），`Key::named("openai")`、`Key::dynamic(name)` 用于带名字的服务。键带着值的类型，类型不匹配是编译错误。
- `ctx.get::<T>()`、`ctx.get_at(&key)`：可选读取。它们不门控，也不建立重载关系，诊断里记为未声明的访问。用在"有就用"的场合；需要的服务写进 `Deps`。
- 对其他语言公开：带 `#[rutis::service]` 的类型，`provide(value)` 以类型的名字公开（不变）；`provide_at(Key::named(n), …)` 和 `Key::dynamic(n)` 以 `n` 公开。原来带名字的绑定不对外（[Rust 服务自动对外](design-rust-service-exposure-2026-10-21.md) R2），这里改成对外。名字被占时 provide 被拒（R6），在实例里提供时由实例认领（R3），这两条不变。以 trait 对象为键的绑定照旧不对外。
- 原来的 `require`、按实例的读写、带 `check()` 的提供、可写绑定，移到 `rutis::host` 和 `rutis::internal`（附录 A）。

### 清理和后台任务

- `ctx.effect(future)`：插件停下时执行这个 future，和其他登记一起按相反顺序执行。如果插件已经在停下时才登记，就立即执行它，并返回 `Inactive` 错误，所以刚拿到的资源一定会被释放。清理返回的错误照旧汇集（`take_cleanup_errors`）。
- `ctx.spawn(|ctx| async move { … })`：立即执行任务。任务拿到自己的 `Ctx`，其中的 `ctx.cancelled()` 在插件停下、或者这个任务的 `Disposer` 被撤销时完成。
- 插件停下时：
  1. `cancelled()` 先完成；
  2. 然后按登记的相反顺序撤销；轮到一个任务时，等它返回，最多等到关闭截止时间（`RUTIS_SHUTDOWN_TIMEOUT`，默认 10 秒），和正在执行的监听器调用是同一条规则；
  3. 到时间还没返回的任务，在当前的 `.await` 处被丢弃，以 `TasksStillRunning` 报告（与监听器调用的 `ListenersStillRunning` 相同）。
- 所以任务要看 `ctx.cancelled()`；不看的任务会让插件停下拖到截止时间。
- 任务返回的错误进入错误汇集（与 `emit` 的监听器失败相同），插件继续运行。任务里再 `spawn` 的任务同样归插件所有。
- `ctx.cancelled()` 也给执行时间长的监听器调用用；`ctx.plugin(child)` 装一个随这个插件一起停下的子插件。

现在 `ctx.effect(f)` 的意思是"立即执行 `f`，再把它返回的 `Effect` 作为清理"，和其他所有语言的 `effect(cleanup)` 都不一样。改了以后，`Effect` 只留给 `host::DynPlugin` 使用。

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
- 名字：事件以 `E::NAME` 登记和发出。没有固定名字的事件类型（例如 rutis-bridge 的 `Args`）重写 `Event::name(&self)`，让值自己带上名字；监听这种事件时用 `ctx.listen::<Args>().name("file-changed")`。
- 五种方式的语义、作用域、过滤、卸载等待，都和[统一事件设计](design-events-2026-10-18.md)一致，不变。

### 错误

- 所有签名里只有一种错误类型 `rutis::Error`，`rutis::Result<T = (), E = Error>`。
- `?` 能把任何 `std::error::Error + Send + Sync + 'static` 转成 `rutis::Error`，rutis 自己的错误也一样。`Error::msg("…")` 用字符串构造一个错误。
- `rutis::Error` 本身不实现 `std::error::Error`，这是 `?` 能接住任何错误的前提（`anyhow::Error` 和 Bevy 的 `BevyError` 也是这么做的）。
- 宿主要区分 rutis 的失败时，`err.kind()` 返回 `Option<&host::ErrorKind>`。`ErrorKind` 就是现在的 `CordisError` 枚举，改了名字，变体不变。来自插件代码的错误，`kind()` 是 `None`；它报到宿主时被包在 `ErrorKind::PluginFailed` 里。
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

- **只加 `rutis::prelude`，其他不动**：`Ctx` 和 `EventBus` 上的方法不会因为多一个 prelude 而变少，文档里照样是 40 个和 28 个。所以根本身就当 prelude 用，不再另设一个模块。
- **新写法当默认，旧写法也保留**：这样同一件事又多了一种写法，#617 说的问题正是写法太多。所以 0.17 一次改完，不保留旧名字；迁移交给工具（附录 B）。
- **`#[rutis::plugin]` 宏**：`async fn` 已经能写在 trait 里，`BoxFuture` 不用宏也能去掉。加了宏，作者看到的就不是真正的 trait，出错时的提示也更难懂。
- **`type Deps = ()` 省掉**：没有依赖的插件也要写这一行和 `_: ()` 参数。省掉它要么等关联类型默认值稳定，要么再分出一个"没有依赖的插件" trait；后者正是要去掉的那种平行写法。所以保留这一行。
- **把配置做成关联类型、`apply(ctx, config, deps)`（Cordis / Python 的写法）**：那样每个插件都要多写一个关联类型，没有配置的插件装起来也得写 `ctx.plugin(P, ())`。在 Rust 里，配置本来就写在结构体的字段上；配置变了重建插件，是装插件那一方的事。
- **同一类型的两个服务只用新类型区分**（上一版的做法）：提供方是第三方插件时，消费方没法要求它改类型；两个插件各自提供 `dyn Llm` 时也区分不了。所以留下 `Key` / `Keyed`。
- **名字写在类型里（`Named<T, OpenAi>`）**：要么用稳定版还没有的 `&str` 常量泛型，要么为每个名字定义一个标记类型；而且名字常常来自配置。所以名字由 `keys()` 在运行期给出。
- **插件停下时直接丢弃任务**（上一版的做法）：丢弃会执行 Drop，但需要异步收尾的资源来不及收尾；而且正在执行的监听器调用本来就会被等待。所以任务和监听器调用用同一条规则：先发取消，再等，最多到截止时间。
- **`once`、`prepend`、`global` 放进 `host`**（上一版的做法）：这三个是插件作者会用的功能，要求插件作者 `use rutis::host::*` 会让分层失去意义。所以它们留在 `listen` 构建器上，只在文档里排在后面。
- **用一个 `on` 同时接受同步和异步闭包**（axum、Bevy 那样的标记类型技巧）：试过。候选实现有两个，编译器就推断不出闭包参数的类型，每个监听器都得写成 `|e: Arc<Created>, call: Call<Created>|`。分成两个方法以后，`on::<E>(|e, call| async move { …? … })` 不用写任何类型标注（两种写法都在 Rust 1.88 上验证过）。
- **合并 `Call` 和 `SyncCall`，凑够 15 个名字**：同步监听器不能 `.await`，异步监听器不在同步方式里被调用；合并以后，用错的地方要到运行时才报错。所以不为凑数合并，名字按实际数（第五节）。
- **`get` 改名 `get_optional`**：返回 `Option` 已经说明它是可选的。真正的坑是"不会重载"，这一点由诊断里的"未声明访问"提示。
- **一个 `dispatch(mode, …)` 代替五个分发方法**：五种方式的返回值不同（不等待、汇总错误、第一个结果、链式、同步），合成一个以后，要么丢掉类型，要么返回一个枚举让调用方再拆。
- **`rutis::Error` 也实现 `std::error::Error`**：这样就和 `impl<E: std::error::Error> From<E>` 冲突，`?` 又接不住任意错误了，作者只能回到 `map_err`。

## 五、验收

对照 #617 的验收项：

| #617 | 本方案 |
| --- | --- |
| 插件作者默认看到的入口 ≤ 15 个 | 根上 11 个 + `rutis::events` 9 个，共 20 个，没有达到。多出来的是事件签名里由编译器推断的类型，以及为按名字依赖留下的 `Key`、`Keyed`；作者在例子里手写的是 9 个。不为凑数再合并类型（第四节）；这 20 个由公开面快照锁住 |
| 最小的 Rust 插件不手写 `BoxFuture` 和生命周期 | 第二节的例子；README、指南、示例都用这种写法 |
| `EventBus` 的公开方法 ≤ 10 个 | 插件只用 `Ctx` 上的 9 个事件方法，`EventBus` 移进 `internal` |
| 服务的 provide / read 方法 ≤ 5 个 | 4 个：`provide`、`provide_at`、`get`、`get_at` |
| `rutis-loader` 有明确的 prelude 和不稳定边界 | 根上 11 个名字，其余按模块分组，`internal` 不承诺稳定 |
| 文档第一页有"只需要这些"的清单 | 新增 `docs/guide/rust-plugin.md`（写一个 Rust 插件），指南首页加一行 |

另外：

- 用 `cargo public-api` 给 `rutis` 根、`rutis::events` 和 `rutis::host` 生成公开面快照，提交进仓库，CI 比对。以后加一个名字就要改快照，评审时看得见，公开面不会再悄悄变大。
- `examples/` 和指南里的 Rust 代码不出现 `rutis::internal`，由 CI 用 grep 检查。
- `docs/guide/rust-plugin.md` 和 README 的 Rust 代码作为 doctest 编译，与现在开发手册的做法相同。
- 第二节的每个写法都写成测试，包括：
  - `?` 接住 `io::Error`；
  - 不写类型标注的监听器闭包能编译；
  - `Keyed` 依赖按 `keys()` 的名字门控，没写 `keys()` 时装载失败，`update` 改名字时更新失败；
  - 按名字提供的带标注服务以这个名字被其他语言调用；
  - `effect` 在插件停下以后才登记时立即执行；
  - 插件停下时等待 `spawn` 的任务，超过截止时间时丢弃并报告 `TasksStillRunning`；
  - `DynPlugin` 与 `Plugin` 的插件能互相依赖；
  - 第三节错误表里的五种场景。
- 迁移工具先迁移仓库自己的 crate，再在 0.16 的 `examples/` 和 aimux-llm 上单独跑一遍，结果能编译、测试通过（附录 B）。
- 重新跑一遍事件分发的基准（[事件分发性能](performance-event-dispatch-2026-09-27.md)）。`Call` 改成拥有所有权以后，每次调用多一次 `Arc` 克隆，要把前后的数字写进 PR。

## 附录 A：名字去向

### `rutis::host`

| 内容 | 名字 |
| --- | --- |
| 根与关闭（`CtxHost`） | `Ctx::root`、`root_with`、`root_with_sink`、`root_view`、`root_ctx`、`view`、`shutdown`、`shutdown_with_timeout`、`is_shutting_down`、`handle`、`error_sink`、`take_cleanup_errors`、`diagnostics` |
| 严格读取（`CtxHost`） | `require::<T>()`、`require_at(&key)` |
| 作用域与实例（`CtxHost`） | `isolate`、`isolate_label`、`instance`、`scope`、`row`、`dispose_self`、`cancellation_token` |
| 装载（`CtxHost`） | `plugin_from(build, config)`（类型化的配置闭包，依赖从 `P::Deps` 和第一次构造出的实例的 `keys()` 得出）、`plugin_with(factory, config)` |
| 事件（`CtxHost`） | `waterfall_sync`、`subscriptions` |
| 插件的对象安全形式 | `DynPlugin`、`PluginFactory`、`Effect`、`EffectMeta`、`EffectPhase`、`BoxFuture` |
| fiber | `FiberView`、`FiberState`、`FiberStatusChanged`、`PluginId`、`Snapshot`、`DisposeWaitError`、`shutdown_deadline` |
| 键 | `TypeKey`、`InstanceId`（`Key::instance(id)` 用它） |
| 错误 | `ErrorKind`、`ServiceReadError`、`ServiceReadFailure`、`NextMisuse`、`CallError`、`ErrorSink`、`default_sink` |
| 诊断 | `host::diagnostics::*`（现在的 10 个诊断类型） |
| 事件描述 | `EventSubscription`、`ListenerKind` |

- **`DynPlugin`** 就是现在的 `Plugin` trait，改了名字，只有一处不同：`injects()` 返回 `Vec<TypeKey>`，不再返回借用的切片。rutis 为每个 `Plugin` 提供 `impl<P: Plugin> DynPlugin for P`，所以 `ctx.plugin(…)` 只有一个，什么插件都能装。依赖要在运行期才定下来的插件（例如 rutis-loader 的 `JsFactory`）、要放进 `Vec<Box<dyn DynPlugin>>` 的插件列表，用的都是 `DynPlugin`。迁移工具也靠它：旧的 `impl Plugin` 改名成 `DynPlugin` 就能编译（附录 B）。
- **`Key<T>`**（在根上）合并了现在的 `Key<T>` 和 `DepKey<T>`：`Key::of()`、`const fn Key::named("primary")`、`Key::dynamic(name)`、`.instance(id)`，可以转成 `TypeKey`。
- **`ServiceKey`** 删除，它只是 `TypeKey` 的别名。

### `rutis::internal`

`EventBus` 和 `events::{remote, wire}`、`EventKey`、`EventPattern`、`EventOptions`、`DispatchAttempt`、`ServiceIntercept`、`ServiceWriter`、`ServiceWriteError`、`ServiceWriteFailure`、`__sdk_service`，以及 `CtxInternal`：`events()`、`mark_scope`、`mark_row`、`provide_checked`（原 `provide_as_with_check`）、`provide_mut_at`（原 `provide_mut_as`）、`require_provided_as`、`effect_named`、`refresh`、`in_instance_key`。

`EventBus` 上只有 rutis-bridge / rutis-loader 用的 10 个方法（`on_remote`、`watch_table`、`observe_dispatch`、`emit_framework`、`dispatch`、`dispatch_sync`、`invoke_entry`、`invoke_entry_sync`、`transcoder`、`set_transcoder`）连同 `try_filter` 留在这里，签名不变。插件和宿主不再拿到 `EventBus`，原来的 `ctx.events().on(&ctx, &key, …)` 都改成 `ctx.on…` 系列方法。

### `rutis` 根上现在的 71 个名字

| 现在 | 0.17 |
| --- | --- |
| `Plugin` | `host::DynPlugin`（`injects` 返回 `Vec`）；新的 `Plugin` 就是原来的 `TypedPlugin`，按第三节去掉了 `BoxFuture` 和 `Effect`，加上 `keys()` |
| `TypedPlugin` | `Plugin` |
| `Typed` | 删除（`impl<P: Plugin> DynPlugin for P`） |
| `PluginFactory` | `host::PluginFactory`（`build` 返回 `Box<dyn DynPlugin>`） |
| `TypedPluginFactory`、`TypedFactory` | 删除；用 `plugin_from`、`Builtins::register`、`export_plugin!(build:)` |
| `Deps` | `Deps`（密封；`Keys` 由 `Plugin::keys()` 给出，不再由 `Typed::with_keys` 给出） |
| `Key`、`DepKey` | `Key<T>`（根，两者合并） |
| `Keyed` | `Keyed<T>`（根） |
| `Gate`、`KeyedGate` | 删除；写 `Arc<T>` 或 `Keyed<T>` 而不使用它 |
| `BoxFuture` | `host::BoxFuture` |
| `Effect`、`EffectMeta`、`EffectPhase` | `host::` |
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
| `provide_as(key, arc)` | `provide_at(Key<T>, arc)` |
| `get_as(key)` | `get::<T>()` 或 `get_at(&Key<T>)` |
| `effect(f)` | `effect(future)`，见第三节 |
| `require`、`require_as` | `host::require`、`host::require_at` |
| `root`、`root_with`、`root_with_sink`、`root_view`、`root_ctx`、`view`、`shutdown`、`shutdown_with_timeout`、`is_shutting_down`、`handle`、`error_sink`、`take_cleanup_errors`、`diagnostics`、`isolate`、`isolate_label`、`instance`、`scope`、`row`、`dispose_self`、`cancellation_token`、`plugin_with`、`plugin_from` | `host::CtxHost` |
| `events` | `internal::CtxInternal`；事件方法在 `Ctx` 上 |
| `mark_scope`、`mark_row`、`provide_as_with_check`、`provide_mut_as`、`require_provided_as`、`effect_named`、`refresh`、`in_instance_key` | `internal::CtxInternal`（两个改名：`provide_checked`、`provide_mut_at`） |
| 新增 | `get_at`、`spawn`、`on`、`on_sync`、`listen`、`emit`、`parallel`、`serial`、`waterfall`、`bail`、`filter` |

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

- `register(name, build)`：`build: Fn(&C) -> Result<P>`，其中 `P: Plugin`、`C: DeserializeOwned + JsonSchema`。config 的 schema 从 `C` 生成，依赖从 `P::Deps` 和第一次构造出的实例的 `keys()` 生成。不需要 schema 的，把 `C` 写成 `serde_json::Value`。
- `register_with(name, build)`：`build: Fn(&Build, &C) -> Result<P>`，用于每个实例各构造一份。

`register_fn` 删除，用 `register` 代替。`register_raw` 保留，给运行期才知道依赖的 `PluginFactory<Value>` 使用。

### rutis-sdk（dylib 插件）

`export_plugin!(id: "weather", build: Weather::new)` 接受和 `Builtins::register` 相同的构造函数，`schema:` 照旧可选。原来的 `factory:` 形式保留，给需要 `PluginFactory<Value>` 的插件用。dylib 插件本来就要用和宿主完全相同的 SDK 构建，所以 0.17 的插件照常重新构建即可。

## 附录 B：迁移与实施

### 迁移

0.17 一次改完，不保留旧名字，和 0.13 的统一事件做法相同。不用 `#[deprecated]` 别名过渡：留着旧名字，旧名字就还会出现在文档和补全里，这正是 #617 要去掉的。协议不变，所以和其他语言、其他节点的互通不受影响。

迁移分两步：先用迁移工具让 0.16 的代码在 0.17 上编译通过，再按需要把插件换成新写法。第一步之后，旧插件以 `host::DynPlugin` 的身份照常运行，行为不变。

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
- 改不了的地方插入 `// TODO(rutis 0.17): …` 注释，编译错误会指向它们。

| 改动 | 自动 |
| --- | --- |
| `CordisError` 在类型位置改成 `Error`；`.map_err(\|e\| CordisError::PluginFailed(e.into()))?` 改成 `?`；构造 `CordisError::PluginFailed(…)` 改成 `Error::from(…)` | 是 |
| 旧的 `impl Plugin` 改成 `impl host::DynPlugin`，`injects` 的签名改成返回 `Vec<TypeKey>`（函数体包一层 `.to_vec()`） | 是 |
| `impl TypedPlugin`：去掉 `BoxFuture` 和 `Box::pin`，改成 `async fn apply`；末尾的 `Ok(Effect::Done)` 改成 `Ok(())`；`Typed::new(p)` 改成 `p`；`Typed::with_keys(p, keys)` 改成在 `impl Plugin` 里生成 `keys()` | 是 |
| `ctx.events().on(&ctx, &key, \|c, e, call\| Box::pin(async move { … }))` 改成 `ctx.on::<E>(\|e, call\| async move { … })`，块里用到的 `c` 改成 `call.ctx()`；`on_sync`、`on_pattern`、`on_opt` 同理；`E` 取自 `EventKey::<E>` 或键变量的类型 | 是（键的类型推不出来时留 TODO） |
| `emit` / `parallel` / `serial`（`&ctx, &key, Arc::new(v)`）改成 `ctx.emit(v)` 等；`bail_sync` 改成 `bail`；`waterfall(&ctx, &key, &e, t)` 改成 `ctx.waterfall(e).terminal(t)` | 是 |
| `provide_as(key, arc)` 改成 `provide_at(…)`；`get_as(TypeKey::of::<T>())` 改成 `get::<T>()` | 是 |
| 用到 `root`、`shutdown`、`require` 等宿主方法的文件加 `use rutis::host::*;`；rutis-loader 根上移走的名字改成模块路径 | 是 |
| `Builtins::register_fn(name, \|c\| Ok(Box::new(P::new(c))))` 改成 `register(name, \|c\| Ok(P::new(c)))` | 是 |
| 旧的 `ctx.effect(\|\| { …; Effect::Disposer(…) })`、手写 `impl Listener` 的结构体、`match` `CordisError` 的变体（要改成 `match err.kind()`）、`Gate` / `KeyedGate` | 否，留 TODO |

验证：仓库自己的 crate 就用这个工具迁移（内核之外 42 处 `impl Plugin`、14 处 `impl PluginFactory`、1277 处 `CordisError`），再在 0.16 的 `examples/` 和 aimux-llm 上单独跑一遍。结果要能编译、测试通过，剩下的 TODO 数量写进发布说明。

### 写 Rust 插件的人

工具跑完以后，把插件换成新写法时改三处：

1. 把 `impl DynPlugin` 改成 `impl Plugin`：`injects()` 里列的类型移进 `type Deps`，`apply` 里对应的 `ctx.require::<T>()` 改成参数。
2. 清理改成 `ctx.effect(async move { … })`；起的后台任务改成 `ctx.spawn(|ctx| async move { … })`，并在任务里看 `ctx.cancelled()`。
3. 按名字读的服务改成 `Keyed<T>` 加 `keys()`。

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

**仓库里的 crate** 在同一个版本里一起改：rutis-bridge、rutis-loader、rutis-dylib、rutis-sdk、rutis-macros、rutis-host、rutis-dsh、rutis-dev、rutis-xtask、aimux-llm，以及示例 rutis-agent、rutis-cli 和 `examples/`。工具跑完之后，只有 aimux-llm 和示例手工换成新的 `Plugin`，因为它们就是给插件作者看的写法；其余保持 `DynPlugin`。真正要重写的只有内核里事件、清理、任务和插件包装这几处。

其他语言的 SDK、运行时、协议、`rutis.json` 都不受影响。

### 实施

在一个集成分支上按下面的顺序提交，整体合入，随 0.17 发布：

1. 内核：新的 `Plugin`（含 `keys()`）/ `Deps` / `DynPlugin` 和 blanket 实现；`Error` / `ErrorKind` 和 `anyhow` feature；`Ctx` 上的服务、`effect`、`spawn`（停下时等待）和事件方法（`Call` 改成拥有所有权）；根上的 `Key<T>` / `Keyed<T>`；建立 `host` / `internal` 两个模块和扩展 trait；删除附录 A 列出的名字。
2. `rutis-migrate`，先用它迁移第 3、5 步的 crate。
3. rutis-bridge、rutis-loader、rutis-dylib、rutis-sdk、rutis-macros 改用 `DynPlugin` 和 `internal`；rutis-bridge 的 `Exposer` 按名字公开带名字的绑定（改 R2）；`Builtins::register`、`export_plugin!(build:)` 改用类型化的构造函数。
4. rutis-loader 的根和模块重新分组。
5. rutis-host、rutis-dsh、rutis-dev、rutis-xtask、aimux-llm、示例（rutis-agent、rutis-cli、`examples/`）。
6. 文档，中英两份：README 的 Rust 部分、新增 `guide/rust-plugin.md`、`guide/README.md`、`guide/rust-host.md`、`guide/plugin-api.md` 的 Rust 部分、开发手册、`design-rust-service-exposure` 的 R2、`migration-0.17.md`。
7. 公开面快照和 CI 检查。
