# 写一个 Rust 插件

从依赖到写插件、测试，再到被宿主使用。需要 Rust 1.88 或更高。

Rust 插件和 TypeScript、Python、Go 插件的规则相同（见 [插件 API](plugin-api.md)），不同的是分发方式：**Rust 插件编进 Rust 宿主**（[在 Rust 应用里嵌入](rust-host.md)），**或者编译成 dylib 插件**，由宿主在运行时加载（[外部插件开发指南](../external-plugin-guide.md)）。两种方式的插件写法相同。

写插件只需要这些：

| 名字 | 用来 |
| --- | --- |
| `rutis::Plugin` | 唯一的插件 trait：`type Deps` 加 `async fn apply` |
| `rutis::Ctx` | `apply` 收到的上下文，见下表 |
| `rutis::Result`、`rutis::Error` | 唯一的错误类型 |
| `rutis::Event` | 事件类型 |
| `rutis::events::Mode` | 监听器里看这次分发的方式 |
| `rutis::Disposer` | `provide`、`on`、`effect` 返回的句柄，用来提前撤销 |
| `#[rutis::service]` | 让服务对其他语言公开 |
| `rutis::Definition` | 插件定义：可以交出、挂很多次的插件，见[插件定义](#插件定义) |

`Ctx` 上给插件用的方法：

| 组 | 方法 |
| --- | --- |
| 服务 | `provide`、`provide_as`、`get` |
| 生命周期 | `effect`、`cancelled`、`plugin` |
| 事件 | `on`、`on_sync`、`listen`、`emit`、`parallel`、`serial`、`waterfall`、`bail`、`filter` |

监听器、调用对象这些类型由编译器推断，不用写出来。

## 1. 依赖

```toml
[dependencies]
rutis = "0.17"
serde = { version = "1", features = ["derive"] }             # 事件和配置的类型派生 Serialize / Deserialize
tokio = { version = "1", features = ["rt-multi-thread", "macros", "time"] }  # 后台任务、测试
```

## 2. 写插件

```rust
use std::sync::Arc;
use rutis::{Ctx, Plugin, Result};

/// 服务就是一个类型。
pub struct Greeting(pub String);

/// 提供 Greeting。apply 里登记的东西，插件停下时自动撤销。
pub struct Greeter {
    pub lang: String,
}

impl Plugin for Greeter {
    type Deps = ();

    async fn apply(&self, ctx: &Ctx, _: ()) -> Result {
        ctx.provide(Greeting(format!("hello from {}", self.lang)))?;
        Ok(())
    }
}

/// 依赖 Greeting：它出现时启动，被替换时重启。
pub struct Printer;

impl Plugin for Printer {
    type Deps = Arc<Greeting>;

    async fn apply(&self, _: &Ctx, greeting: Arc<Greeting>) -> Result {
        println!("{}", greeting.0);
        Ok(())
    }
}
```

插件不决定自己什么时候运行，宿主决定。写插件只需要知道三件事：

1. `apply` 在 `Deps` 里所有必需的服务都就绪之后执行；可选的服务不等。
2. `apply` 里经 `ctx` 登记的一切（服务、监听器、清理、子插件）在插件停下时按登记的相反顺序撤销；`apply` 失败时，已经登记的部分同样撤销。
3. 必需的服务被撤销或替换时，插件先停下，等服务重新就绪后再执行一次 `apply`。

| 事项 | 规则 |
| --- | --- |
| `type Deps` | 必须写；没有依赖时写 `()`，`apply` 的参数写 `_: ()` |
| `name` | `ctx.plugin(p)` 装的插件在日志和诊断里的名字，默认是类型名的最后一段（`Greeter`），可以重写 `fn name(&self) -> &str`。由构造函数装的插件用装它时的名字：`Builtins::register` 登记的名字、`export_plugin!` 的 `id`，`plugin_from` 是类型名 |
| `validate` | `fn validate(&self) -> Result`，在装载时、`apply` 之前执行；失败时插件进入 Failed，不执行 `apply` |
| 配置 | 写成结构体的字段，再提供 `fn new(config: &Config) -> Result<Self>`；装插件的一方用它构造插件，配置变了也用它重建（见[第 4 节](#4-被宿主使用)） |
| 并发 | `apply`、监听器和服务的方法在 tokio 上执行，可能同时被调用：插件和它提供的服务要是 `Send + Sync` |

六种状态、恰好一次清理、撤销时消费者先停这些保证写在[保证](../guarantees.md)里，写插件不需要先读。

### 服务

```rust
use std::sync::Arc;
use rutis::{Ctx, Plugin, Result};

pub trait Llm: Send + Sync {
    fn ask(&self, question: &str) -> String;
}

struct Echo;

impl Llm for Echo {
    fn ask(&self, question: &str) -> String {
        question.to_owned()
    }
}

/// 有就用，没有也能运行。
pub struct Cache;

pub struct Forecast {
    pub llm: Arc<dyn Llm>,
}

struct LocalLlm;

impl Plugin for LocalLlm {
    type Deps = ();

    async fn apply(&self, ctx: &Ctx, _: ()) -> Result {
        ctx.provide_as::<dyn Llm>(Arc::new(Echo))?; // 以 trait 对象的类型提供
        Ok(())
    }
}

struct Weather;

impl Plugin for Weather {
    type Deps = (Arc<dyn Llm>, Option<Arc<Cache>>);

    async fn apply(&self, ctx: &Ctx, (llm, cache): Self::Deps) -> Result {
        if cache.is_some() {
            // 装载时有缓存
        }
        ctx.provide(Forecast { llm })?;
        Ok(())
    }
}
```

| 方法 | 作用 |
| --- | --- |
| `ctx.provide(value)` | 以 `value` 自己的类型提供，直到插件停下，或返回的 `Disposer` 调用 `dispose()` |
| `ctx.provide_as::<dyn T>(arc)` | 以 trait 对象的类型提供：`ctx.provide_as::<dyn Llm>(Arc::new(MyLlm))` |
| `ctx.get::<T>()` | 可选读取，`T` 可以是 trait 对象。它不门控，也不建立重载关系，诊断里记为未声明的访问。用在"有就用"的场合；需要的服务写进 `Deps` |

`type Deps` 能写的：

| `type Deps =` | 门控 | 传入 |
| --- | --- | --- |
| `()` | 无 | `()` |
| `Arc<T>` | `T` | 服务 |
| `Option<Arc<T>>` | 无 | 装载时可见就有；之后出现或消失都不重载 |
| 至多 8 个成员的元组 | 全部成员 | 各自的值 |

`T` 可以是 trait 对象（`Arc<dyn Llm>`）。同一个类型的两个服务以不同名字提供、插件按名字依赖其中一个时，用宿主层的写法，见 [在 Rust 应用里嵌入：按名字依赖](rust-host.md#按名字依赖)。

### 对其他语言提供服务

服务类型的 `impl` 块上标 `#[rutis::service]`，照常 `provide`，各语言的行就能按名字调用它；名字来自类型（`Calendar` 是 `calendar`），`#[service(name = "…")]` 可以改：

```rust
use std::sync::atomic::{AtomicU64, Ordering};
use rutis::{Ctx, Plugin, Result};

#[derive(Default)]
pub struct Calendar {
    next: AtomicU64,
}

#[rutis::service]                     // 对外名字 "calendar"
impl Calendar {
    pub async fn add(&self, title: String) -> u64 {
        let id = self.next.fetch_add(1, Ordering::SeqCst);
        println!("{id}: {title}");
        id
    }
}

struct Calendars;

impl Plugin for Calendars {
    type Deps = ();

    async fn apply(&self, ctx: &Ctx, _: ()) -> Result {
        ctx.provide(Calendar::default())?; // 提供即对外
        Ok(())
    }
}
# fn main() {}
```

```python
async def apply(ctx, config):
    await ctx.use("calendar").add("standup")

plugin = define_plugin(apply, inject=["calendar"])
```

没标注的类型不对外。能传什么、错误怎样抛、带 `&Ctx` 参数的方法，见 [在类型上标注 `#[rutis::service]`](rust-host.md#在类型上标注-rutisservice)。同一个服务要多份时，用 rutis-loader 的实例：每个实例里各装一份，实例里的行用自己那份。

### 插件定义

`Plugin` 是一个实例：配置写在字段里，交给 `ctx.plugin` 就用掉了。要把插件交给别处、由那边挂很多次（例如每个 Session 一份），用 `rutis::Definition`：`Definition::new(ctx, build)` 造一个定义，`build` 拿到挂载位置的 `ctx` 和这一份的配置，返回一个 `Plugin`；挂的一方写 `ctx.plugin(def.at(&ctx, config)?)`。

```rust
use std::sync::{Arc, Mutex};
use rutis::{Ctx, Definition, Plugin, Result};

/// 每个 Session 一份，`depth` 来自它自己的配置。
struct Part {
    depth: u32,
}

impl Plugin for Part {
    type Deps = ();

    async fn apply(&self, _: &Ctx, _: ()) -> Result {
        println!("depth {}", self.depth);
        Ok(())
    }
}

/// 把 `Part` 的定义交给挂部件的一方（这里只是一个列表）。
struct Git {
    parts: Arc<Mutex<Vec<Definition>>>,
}

impl Plugin for Git {
    type Deps = ();

    async fn apply(&self, ctx: &Ctx, _: ()) -> Result {
        let pool = Arc::new(String::from("pool")); // apply 里准备好的东西
        let definition = Definition::new(ctx, move |_at: &Ctx, depth: u32| {
            let _pool = pool.clone();
            Ok(Part { depth })
        });
        self.parts.lock().unwrap().push(definition);
        Ok(())
    }
}

/// 挂的一方：每挂一次是一份，配置各自给。
fn mount(at: &Ctx, definition: &Definition) -> Result {
    at.plugin(definition.at(at, 3u32)?);
    Ok(())
}
# fn main() {}
```

- 定义属于造它的插件的这一代：这个插件卸载、重启或结束时，先结束从它挂出的每一份，等它们清理完，再运行它自己的清理；之后 `at` 失败，`ended()` 完成。
- 只能挂在造它的插件所在的实例里，或者更里面。
- 其他语言交来的插件定义也是 `Definition`，挂法相同，见 [插件 API](plugin-api.md#插件定义)；Rust 造的定义不能交给其他语言。

### 事件

一个宿主上所有语言、所有进程的监听器都在一条事件总线上（[插件 API：事件](plugin-api.md#事件)）：事件在哪里发出、监听器在哪里，结果都一样。

事件是一个类型，`NAME` 是它在所有语言里的名字，参数按 serde 编解码。两个及以上字段的元组结构体对应同样多个参数，所以下面的 `Created` 就是别的语言里的 `emit("session/created", id, size)`；unit 结构体没有参数；其余（普通结构体、newtype）是一个参数。

```rust
use rutis::events::Mode;
use rutis::{Ctx, Event, Plugin, Result};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub struct Created(pub String, pub u32);

impl Event for Created {
    const NAME: &'static str = "session/created";
    type Value = String; // serial、bail、waterfall 返回的结果
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
            println!("{}", call.name()); // 前缀监听器：实际的名字
            Ok(None)
        })?;
        Ok(())
    }
}
```

发出事件用五种方式，名字和其他语言相同：

```rust
# use rutis::{Ctx, Event};
# use serde::{Deserialize, Serialize};
# #[derive(Serialize, Deserialize)]
# pub struct Created(pub String, pub u32);
# impl Event for Created {
#     const NAME: &'static str = "session/created";
#     type Value = String;
# }
# async fn demo(ctx: &Ctx) -> rutis::Result {
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
# let _ = (first, out, quick, from_python);
# Ok(())
# }
```

| 事项 | 规则 |
| --- | --- |
| 监听器 | 闭包，参数是 `(Arc<E>, Call<E>)`，同步监听器是 `(Arc<E>, SyncCall<'_, E>)`；返回 `Result<Option<E::Value>>`，`Ok(None)` 和 `Ok(Some(false))` 是没有结果。参数类型由编译器推断，块里可以对任何错误用 `?`。闭包会被调用多次，要移进 `async move` 的东西先在闭包里克隆 |
| `call` | `mode()` 是这次分发的方式；`name()` 是分发用的名字；`next()` 只在 `waterfall` 里可用（否则 `WaterfallNext`），执行链上其余部分，不调用就否决其余；`ctx()` 是登记这个监听器的 `Ctx`，在它上面登记的东西属于这个插件 |
| 同步与异步 | `on_sync` 的监听器所有方式都能调用，但在异步 `waterfall` 里不能调 `next`；`on` 的监听器不能被 `bail` 调用（`AsyncListener`） |
| 名字 | 以 `E::NAME` 登记和发出。`listen::<E>().name(n)` 在另一个名字下监听；`.prefix(p)` 收这个前缀下的所有名字，都按 `E` 解码，形状不同的事件让这次调用以 `EventValue` 失败 |
| 发出 | `emit`、`parallel`、`serial`、`waterfall`、`bail` 接受 `E` 或 `Arc<E>`；`waterfall` 返回一个构建器，可以先写 `.terminal(…)` 再 `.await`，没有终态时最后一个 `next()` 得到 `None`。`ctx.filter(keep)` 给出同样五个方法，`keep` 收到每个监听器的 `ListenerInfo` |
| 失败 | `emit` 的监听器失败不影响发出方，报给宿主；`parallel` 全部跑完，返回 `ListenersFailed`（按登记顺序，只有一个失败也是）；`serial`、`bail`、`waterfall` 遇到第一个错误停止并返回它 |
| 卸载 | 插件停下时它的监听器撤下；正在运行的调用看到 `call.ctx().cancelled()`，卸载等它们结束，最多到关闭截止时间。丢弃 `parallel`、`serial`、`waterfall` 的 future 即取消这次分发 |
| 保留的名字 | `rutis/*`（`rutis/service`、`rutis/row`、`rutis/runtime`）可以监听，不能发出（`ReservedEvent`） |

#### 高级：`once`、`prepend`、`global`

```rust
# use rutis::{Ctx, Event};
# use serde::{Deserialize, Serialize};
# #[derive(Serialize, Deserialize)]
# pub struct Created(pub String, pub u32);
# impl Event for Created {
#     const NAME: &'static str = "session/created";
#     type Value = String;
# }
# fn demo(ctx: &Ctx) -> rutis::Result {
ctx.listen::<Created>().once().on(|_, _| async move { Ok(None) })?;    // 最多调用一次
ctx.listen::<Created>().prepend().on_sync(|_, _| Ok(None))?;           // 排在这个名字已有的监听器之前
ctx.listen::<Created>().global().on(|_, _| async move { Ok(None) })?;  // 不经过滤
# Ok(())
# }
```

选项可以连着写，最后是 `.on(…)` 或 `.on_sync(…)`。没有固定名字的事件类型（名字由值决定）把 `NAME` 写成 `""`，重写 `fn name(&self) -> &str` 返回这个值的名字，监听时用 `.name(…)` 给出名字。

### 清理和后台任务

```rust
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use rutis::{Ctx, Plugin, Result};

pub trait Store: Send + Sync {
    fn flush(&self);
}

struct Syncer {
    path: PathBuf,
}

impl Plugin for Syncer {
    type Deps = Arc<dyn Store>;

    async fn apply(&self, ctx: &Ctx, store: Arc<dyn Store>) -> Result {
        let file = std::fs::File::open(&self.path)?; // io::Error 直接 `?`
        ctx.effect(|| {
            let stop = ctx.clone();
            let task = tokio::spawn(async move {
                let mut tick = tokio::time::interval(Duration::from_secs(60));
                loop {
                    tokio::select! {
                        _ = stop.cancelled() => break,         // 插件开始停下
                        _ = tick.tick() => store.flush(),
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

- `ctx.effect(|| { …; async move { … } })` 立即执行闭包，把它返回的 future 登记为清理。插件停下时，清理和其他登记一起按相反顺序执行。不需要清理时不调用 `effect`；多个清理就调用多次。
- 插件已经在停下或已停下时，`effect`（以及 `provide`、`on` 等所有登记）返回 `InactiveEffect` 错误，闭包不执行。所以资源在闭包里拿到，被拒时什么都还没拿，不会泄漏。
- 后台任务用 tokio 自己的 `tokio::spawn`（成组管理时用 `JoinSet`）：在 `effect` 的闭包里起，在它返回的清理里等它退出；任务里看 `ctx.cancelled()`，插件开始停下时它就完成。停下的过程等清理完成，最多到关闭截止时间。
- 执行时间长的监听器同样看 `call.ctx().cancelled()`。`ctx.plugin(child)` 装一个随这个插件一起停下的子插件。
- 清理返回的错误报给宿主。`effect` 返回的 `Disposer` 调用 `dispose()` 时提前执行清理。

### 错误

所有签名里只有一种错误类型 `rutis::Error`，`rutis::Result<T = ()>` 就是 `Result<T, rutis::Error>`。`?` 能把任何 `std::error::Error + Send + Sync + 'static` 转成它；`Error::msg("…")` 用字符串构造一个错误：

```rust
use rutis::{Error, Result};

#[derive(Debug, thiserror::Error)]
#[error("quota exceeded: {used} of {limit}")]
pub struct QuotaExceeded {
    used: u32,
    limit: u32,
}

fn remaining(limit: &str, used: u32) -> Result<u32> {
    let limit: u32 = limit.trim().parse()?;        // ParseIntError 直接 `?`
    if limit == 0 {
        return Err(Error::msg("limit must not be 0"));
    }
    if used > limit {
        return Err(QuotaExceeded { used, limit }.into()); // 自己的错误类型
    }
    check_name("weather").map_err(Error::msg)?;     // String 不是错误类型
    Ok(limit - used)
}

fn check_name(_: &str) -> std::result::Result<(), String> {
    Ok(())
}
```

| 场景 | 写法 |
| --- | --- |
| 插件代码里的 `io::Error`、解析错误、自己的 thiserror 错误类型 | 直接 `?` |
| 只有一段文字，或别的库返回 `Result<_, String>` | `Error::msg("…")`、`.map_err(Error::msg)?` |
| 插件代码里遇到 `anyhow::Error` | 打开 rutis 的 `anyhow` feature，`.map_err(Error::from_anyhow)?`。`anyhow::Error` 不实现 `std::error::Error`，写不成 `?` |
| 自己的 thiserror 错误类型包住 rutis 的错误 | `#[error(transparent)] Rutis(#[from] rutis::Error)` 或 `#[source] cause: rutis::Error` |

插件代码里的错误报到宿主时包在 `PluginFailed` 里，带着原来的错误作为 source。宿主怎样区分 rutis 自己的失败，见 [在 Rust 应用里嵌入：宿主看到的错误](rust-host.md#宿主看到的错误)。

## 3. 测试

测试不需要单独的工具：在 `#[tokio::test]` 里建一个根 `Ctx`，装上插件和它要的服务，再断言。建根、关闭这些是宿主的方法，要先 `use rutis::host::*;`：

```rust
# use std::sync::Arc;
# use rutis::events::Mode;
# use rutis::{Event, Plugin};
# use serde::{Deserialize, Serialize};
# pub struct Greeting(pub String);
# pub struct Greeter { pub lang: String }
# impl Plugin for Greeter {
#     type Deps = ();
#     async fn apply(&self, ctx: &Ctx, _: ()) -> rutis::Result {
#         ctx.provide(Greeting(format!("hello from {}", self.lang)))?;
#         Ok(())
#     }
# }
# #[derive(Serialize, Deserialize)]
# pub struct Created(pub String, pub u32);
# impl Event for Created {
#     const NAME: &'static str = "session/created";
#     type Value = String;
# }
# struct Audit;
# impl Plugin for Audit {
#     type Deps = ();
#     async fn apply(&self, ctx: &Ctx, _: ()) -> rutis::Result {
#         ctx.on::<Created>(|event, call| async move {
#             if call.mode() == Mode::Waterfall {
#                 return Ok(call.next().await?.map(|text| format!("[{text}]")));
#             }
#             Ok(Some(format!("audited {}", event.0)))
#         })?;
#         Ok(())
#     }
# }
use rutis::host::*;
use rutis::Ctx;

#[tokio::test]
async fn greets() -> Result<(), Box<dyn std::error::Error>> {
    let root = Ctx::root()?;
    let greeter = root.plugin(Greeter { lang: "Rust".into() });
    (&greeter).await?; // apply 执行完
    assert_eq!(root.get::<Greeting>().unwrap().0, "hello from Rust");

    greeter.dispose().await?; // 停下：服务撤销
    assert!(root.get::<Greeting>().is_none());
    root.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn audits() -> Result<(), Box<dyn std::error::Error>> {
    let root = Ctx::root()?;
    let audit = root.plugin(Audit);
    (&audit).await?;
    let out = root.serial(Created("s1".into(), 3)).await?;
    assert_eq!(out.as_deref(), Some("audited s1"));
    root.shutdown().await?;
    Ok(())
}
# fn main() {}
```

```bash
cargo test
```

- 依赖别的服务的插件，在测试里先装一个提供假服务的插件，或在根上 `root.provide_as::<dyn Llm>(Arc::new(FakeLlm))?`。
- `emit` 的监听器失败进入根的错误汇集：用 `Ctx::root_with_sink(handle, sink)` 建根，把它们收集起来断言。
- 别的语言的监听器、跨进程传值要用真实的运行时，与生产相同，见 [在 Rust 应用里嵌入](rust-host.md#运行时与加载器)。

## 4. 被宿主使用

### 编进 Rust 宿主

插件的配置写成结构体，构造函数从配置构造插件：

```rust
use rutis::host::*;
use rutis::{Ctx, Plugin, Result};
use serde::Deserialize;

#[derive(Deserialize)]
pub struct WeatherConfig {
    pub city: String,
}

pub struct Forecast(pub String);

pub struct Weather {
    city: String,
}

impl Weather {
    pub fn new(config: &WeatherConfig) -> Result<Self> {
        Ok(Weather { city: config.city.clone() })
    }
}

impl Plugin for Weather {
    type Deps = ();

    async fn apply(&self, ctx: &Ctx, _: ()) -> Result {
        ctx.provide(Forecast(format!("sunny in {}", self.city)))?;
        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = Ctx::root()?;
    let weather = root.plugin_from(Weather::new, WeatherConfig { city: "Oslo".into() });
    (&weather).await?;
    weather.update(WeatherConfig { city: "Bergen".into() }).await?; // 用 Weather::new 重建
    root.shutdown().await?;
    Ok(())
}
```

用 rutis-loader 按配置（`rutis.json` 的行）装时，把同一个构造函数登记成一个名字；配置按 `WeatherConfig` 解码，配置的 JSON Schema 从它生成，所以它还要派生 `schemars::JsonSchema`：

```rust,ignore
let mut builtins = Builtins::new();
builtins.register("weather", Weather::new); // 行：{ "id": "weather", "name": "weather", "config": { "city": "Oslo" } }
```

每个实例各构造一份时用 `register_with(name, |build, config| …)`。加载器、运行时和其他语言的行见 [在 Rust 应用里嵌入](rust-host.md)。

### dylib 插件

同一个插件也可以编译成 dylib，由宿主在运行时加载。crate 只依赖 `rutis-sdk`，经 `rutis_sdk::rutis` 用 rutis（`use rutis_sdk::rutis::{Ctx, Plugin, Result};`），再用构造函数导出：

```rust,ignore
rutis_sdk::export_plugin!(id: "weather", build: Weather::new);
```

- `build:` 接受和 `Builtins::register` 相同的构造函数；`schema:` 可选，给出配置的 JSON Schema（`serde_json::Value`）。
- dylib 插件的事件和配置类型用 SDK 里的 serde 派生：`#[derive(rutis_sdk::rutis::serde::Deserialize)]` 加 `#[serde(crate = "rutis_sdk::rutis::serde")]`。插件自己的 serde 和 SDK 的不是同一份。
- dylib 插件要用和宿主完全相同的 SDK 构建。工作区、打包和排错见 [外部插件开发指南](../external-plugin-guide.md)。

从 0.16 升级见 [迁移到 0.17](../migration-0.17.md)。
