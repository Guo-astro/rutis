# 在 Rust 应用里嵌入

应用要给插件提供自己的 Rust 服务、或者把 rutis 放进已有的 Rust 程序时，直接用 crate 组装宿主。不需要这些时用 [rutis-host](rutis-host.md) 即可，配置的行格式相同。

本文是 API 参考。从头做一个开放插件的宿主，按 [做一个开放插件的宿主](open-host.md) 一步步来。

## 依赖

```toml
[dependencies]
rutis = "0.19"
rutis-loader = { version = "0.19", features = ["node", "python"] }
rutis-bridge = { version = "0.19", features = ["python", "websocket"] }   # node 默认开
rutis-host = "0.19"                # connect::SpawnPlugin starts the local language runtimes
schemars = "1"                     # Builtins::register 解码的配置类型派生 schemars::JsonSchema
serde = { version = "1", features = ["derive"] }
tokio = { version = "1", features = ["rt-multi-thread", "macros"] }
```

| crate / feature | 内容 |
| --- | --- |
| `rutis` | 内核：插件、依赖、服务 |
| `rutis-loader` | 按数据（行）加载插件。`node` / `python` / `go`：这几种语言的行（以及 `<连接行>:<插件>` 行） |
| `rutis-host` | 它的 `connect::SpawnPlugin` 启动本机的语言运行时、在它的通道上开一个会话，与 rutis-host 的 `runtimes` 相同 |
| `rutis-bridge` | 连接其他进程、语言、机器。`node`（默认）、`python`、`go`：本机运行时；`websocket`：WebSocket 承载；`cordis`：挂载 Cordis 插件并生成 Rust 绑定（见 [Cordis](cordis.md)）；`testing`：一致性测试 |
| `rutis-types` | 放在 `[build-dependencies]`：在 `build.rs` 里为[带标注的 Rust 服务](#在类型上标注-rutisservice)写 TypeScript 声明与 Python 类型 |

只用其中一种语言时只开那一个 feature，应用只编译、只启动它用到的部分。

## 装 Rust 插件

Rust 插件怎样写见 [写一个 Rust 插件](rust-plugin.md)，下面的 `Greeter`、`Printer`、`Weather` 是几个示意的插件。宿主用的方法（建根、关闭、按键读写、按配置装插件）在 `rutis::host` 里，`use rutis::host::*;` 之后就能在 `Ctx` 上调用：

```rust
use rutis::host::*;
use rutis::Ctx;
use rutis_loader::Builtins;

/// 从配置构造 Weather；`update(config)` 也用它重建。
struct WeatherFactory;
impl PluginFactory<WeatherConfig> for WeatherFactory {
    fn name(&self) -> &str {
        "weather"
    }
    fn build(&self, config: &WeatherConfig) -> rutis::Result<Box<dyn DynPlugin>> {
        Ok(Box::new(Weather::new(config)?))
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = Ctx::root()?;                       // 用当前的 tokio 运行时
    root.plugin(Greeter { lang: "Rust".into() }).await?; // 提供 Greeting
    root.plugin(Printer).await?;                   // 它的 Deps（Greeting）已经在：打印 "hello from Rust"
    let weather = root.plugin_with(WeatherFactory, WeatherConfig { city: "Oslo".into() });
    weather.update(WeatherConfig { city: "Bergen".into() }).await?; // 用 WeatherFactory 重建

    let mut builtins = Builtins::new();            // rutis-loader 的行按名字找到它
    builtins.register("weather", Weather::new);
    // …放进加载器的 Chain，见下面的“运行时与加载器”

    root.shutdown().await?;                        // 等所有清理
    Ok(())
}
# use std::sync::Arc;
# use rutis::Plugin;
# pub struct Greeting(pub String);
# pub struct Greeter { pub lang: String }
# impl Plugin for Greeter {
#     type Deps = ();
#     async fn apply(&self, ctx: &Ctx, _: ()) -> rutis::Result {
#         ctx.provide(Greeting(format!("hello from {}", self.lang)))?;
#         Ok(())
#     }
# }
# pub struct Printer;
# impl Plugin for Printer {
#     type Deps = Arc<Greeting>;
#     async fn apply(&self, _: &Ctx, greeting: Arc<Greeting>) -> rutis::Result {
#         println!("{}", greeting.0);
#         Ok(())
#     }
# }
# #[derive(serde::Deserialize, schemars::JsonSchema)]
# pub struct WeatherConfig { pub city: String }
# pub struct Weather { city: String }
# impl Weather {
#     pub fn new(config: &WeatherConfig) -> rutis::Result<Self> {
#         Ok(Weather { city: config.city.clone() })
#     }
# }
# impl Plugin for Weather {
#     type Deps = ();
#     async fn apply(&self, _: &Ctx, _: ()) -> rutis::Result {
#         println!("weather in {}", self.city);
#         Ok(())
#     }
# }
```

| 要做的事 | 写法 |
| --- | --- |
| 建根 | `Ctx::root()`（当前的 tokio 运行时，没有时返回错误）、`Ctx::root_with(handle)`、`Ctx::root_with_sink(handle, sink)`（自己的错误汇集） |
| 装插件 | `root.plugin(p)`：任何 `Plugin`，或者 `host::DynPlugin`；`Box<dyn DynPlugin>` 也是，运行时才选定的插件列表用它 |
| 按配置装 | 实现 `host::PluginFactory<C>`，`root.plugin_with(factory, config)`：每次装载用 `build` 从当前配置构造插件，返回的 `FiberView` 可以 `update(config)`；依赖写在 `injects()` 里，诊断里的名字是 `name()` |
| rutis-loader 的行 | `Builtins::register(name, build)`：配置按 `C` 解码，Schema 从 `C` 生成（`C` 是 `serde_json::Value` 时没有）；`register_with(name, \|build, config\| …)`：每个实例各构造一份；`register_raw(name, factory, schema)`：`PluginFactory<serde_json::Value>` |
| 读服务 | `get::<T>()`；按键 `get_at(&key)`、提供 `provide_at(key, arc)`，键是 `host::Key<T>`（见[按名字依赖](#按名字依赖)）；`require` / `require_at` 是严格读取，在 `DynPlugin` 的 `apply` 里读声明过的依赖 |
| 停下 | `root.shutdown().await` 卸载全部并等清理，不限时（等多久由宿主决定）；`shutdown_with_timeout(limit)` 限制等待，到时的错误写明还没结束的插件和清理（[默认输出](#默认输出)）；单个插件 `view.dispose()`、`view.restart()`、`view.dispose_with_timeout(limit)` |
| 观察 | `view.state()`、`view.watch()`、`root.diagnostics()`、`root.subscriptions()` |

等一个装上的插件，等的是什么：

- `view.await`（以及 `update`、`restart`）在 fiber 处理完排给它的事（装载、这次更新、这次重启）后返回。`Deps` 没有到齐的插件不装载，停在 `Pending`，`view.await` 照样返回 `Ok(())`，插件并没有运行。
- 要等到插件运行，看它的状态：`view.watch()` 收到每一次状态变化，等到 `Active`（在运行）或 `Failed`（装载失败，错误在快照的 `error` 里）。依赖缺着时会一直等，用 `tokio::time::timeout` 限制；超时后 `view.state()` 和 `root.diagnostics()` 说明插件在等什么。
- 等了 10 秒还在等的插件向错误汇集报告一次：`ErrorKind::StillWaiting` 写明插件和它缺的服务（默认的 sink 打印 `printer: still waiting after 10s for app::Greeting: …`），插件照常继续等。rutis 自己的插件（传输、链路、运行时、加载器）不报，它们在等什么看它们自己的状态。`root.report_waiting_after(Some(时长))` 改这个时间，对整棵树生效，`None` 不报（[依赖诊断](../core-features.md#依赖诊断)）。
- 装的顺序不影响结果：先装的使用者会等后装的提供者。上面的例子先装提供者，所以 `view.await` 返回时使用者已经在运行。

### 按名字依赖

`type Deps` 按类型依赖。同一个类型的两个服务以不同名字提供（两个插件各自以 `openai`、`local` 提供 `dyn Llm`），插件按名字依赖其中一个时，实现插件的对象安全形式 `host::DynPlugin`：在 `injects()` 里列出键，在 `apply` 里用 `require_at` 读：

```rust
use rutis::host::*;
use rutis::Ctx;

pub trait Llm: Send + Sync {}

const OPENAI: Key<dyn Llm> = Key::named("openai");

struct Router {
    local: Key<dyn Llm>,              // Key::dynamic(配置里的名字)
}

impl DynPlugin for Router {
    fn name(&self) -> &str {
        "router"
    }

    fn injects(&self) -> Vec<TypeKey> {
        vec![OPENAI.erase(), self.local.erase()]
    }

    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, rutis::Result<Effect>> {
        Box::pin(async move {
            let smart = ctx.require_at(&OPENAI)?;
            let fast = ctx.require_at(&self.local)?;
            // 两个都就绪才启动；任何一个被替换就重启
            Ok(Effect::Done)
        })
    }
}
```

- 提供方用 `ctx.provide_at(Key::named("openai"), arc)?`；不门控的读取用 `ctx.get_at(&key)`。
- `Key<T>` 带着值的类型：`Key::of()`、`Key::named("x")`（可以写成 `const`）、`Key::dynamic(name)`（运行时才定的名字）、`.instance(id)`（某个实例自己的）；`.erase()` 得到 `injects` 要的 `TypeKey`。值的类型和键不符是编译错误。
- 每个 `Plugin` 都是一个 `DynPlugin`，所以两种插件可以互相依赖。`DynPlugin::apply` 返回的 `Effect` 是清理，没有时是 `Effect::Done`；也可以在 `apply` 里调用 `ctx.effect`。

### 宿主看到的错误

- 所有签名里的错误都是 `rutis::Error`。宿主的函数返回 `Box<dyn Error>`（或 `Box<dyn Error + Send + Sync>`）时直接 `?`；遇到 `anyhow::Error` 时写 `.map_err(|e| Error::msg(format!("{e:#}")))?`（`anyhow::Error` 不实现 `std::error::Error`，写不成 `?`）。
- `err.kind()` 返回 `Option<&host::ErrorKind>`：rutis 自己的失败是 `Some`（`Closed`、`InjectUnsatisfied`、`ListenersFailed` 等），来自插件代码的错误是 `None`，它报到宿主时被包在 `ErrorKind::PluginFailed` 里。
- 错误不能 `Clone`。几方共同持有的错误是 `Arc<ErrorKind>`：错误汇集（`ErrorSink`）收到的、等待 `FiberView`、`dispose()`、`shutdown()` 得到的。

```rust,no_run
use std::sync::Arc;
use rutis::host::*;
use rutis::Ctx;

fn describe(error: &rutis::Error) -> &'static str {
    match error.kind() {
        Some(ErrorKind::Closed) => "root is shut down",
        Some(ErrorKind::ListenersFailed { .. }) => "some listeners failed",
        Some(_) => "another rutis failure",
        None => "the plugin's own error",
    }
}

let sink: ErrorSink = Arc::new(|error: Arc<ErrorKind>| eprintln!("[app] {error}"));
let root = Ctx::root_with_sink(tokio::runtime::Handle::current(), sink);
```

### 默认输出

`Ctx::root()`、`Ctx::root_with(handle)` 装的错误汇集（`host::default_sink()`）把收到的每个错误写到标准错误，不带 `[rutis]` 之类的前缀；一个错误通常一行，几个错误聚在一起（`multiple errors:`）时每个一行。没人读返回值时也看得到出了什么事：

| 事 | 一行，以名字开头 | 错误 |
| --- | --- | --- |
| 插件或 rutis-loader 的行启动失败（`apply` 出错、行的插件找不到、配置不对） | `weather: failed: no API key` | `ErrorKind::StartFailed { plugin, error }`；等它的 `view.await` 照样收到错误 |
| 语言运行时的进程结束或起不来 | `runtime py: down (runtime process exited with exit status: 1)` | `ErrorKind::RuntimeDown { runtime, reason }`；它的行等它 |
| 插件等了 10 秒还在等 | `printer: still waiting after 10s for app::Greeting: …` | `ErrorKind::StillWaiting` |
| 标了 `#[rutis::service]` 的类型被提供，但没链接 rutis-bridge | `service calendar is provided by type but not registered by name: …` | `ErrorKind::NotServedByName`，每个服务一次 |
| 监听器出错 | `listener of … failed: …` | `ErrorKind::ListenerFailed` |

名字是行名（行装载后），否则是插件名；实例里的行被拒时写 `weather in session 7`（组名和实例的编号）。rutis 自己的插件（传输、链路、运行时、加载器）启动失败不在这里报：它们的失败已经写在运行时那一行或链路状态里。因为被停下（`dispose`、关闭）而失败的 `apply`、整棵树关闭时结束的运行时都不报。正在运行的行改配置被拒、仍按原来的配置运行时，不算启动失败，这里不报，`reconcile` 的结果里有它。

同一件事可能打印几行：插件启动失败时先报回滚中清理的错误，再报 `StartFailed`（它的错误里也有那些清理错误）；父插件因子插件失败而失败时两者各报一次。

汇集是在 rutis 内部被调用的：不要在汇集里等 rutis 的操作。rutis-loader 报被拒的行时还拿着它的操作锁，汇集里再调用这个加载器的 `reconcile` 等修改会一直等下去；要做这类事，把错误送到别的任务里处理。

要关掉或改写，换掉汇集：`Ctx::root_with_sink(handle, sink)`。想保留其余输出、只去掉某一种，在自己的汇集里按变体过滤，其余交给 `default_sink()`：

```rust,no_run
use std::sync::Arc;
use rutis::host::*;
use rutis::Ctx;

let fallback = default_sink();
let sink: ErrorSink = Arc::new(move |error: Arc<ErrorKind>| {
    if !matches!(*error, ErrorKind::StartFailed { .. }) {
        fallback(error);
    }
});
let root = Ctx::root_with_sink(tokio::runtime::Handle::current(), sink);
```

关闭不限时：`root.shutdown()` 一直等到所有清理结束，等多久由宿主决定（rutis-host 默认 10 秒）。要限时用 `root.shutdown_with_timeout(时长)`；到时关闭在后台继续，错误（`DisposeWaitError::TimedOut`）的 `stuck` 列出还没结束的插件和它在做什么，显示为一行：

```text
not finished after 3s: weather: still running the cleanup registered at src/weather.rs:12:9
```

用 `ctx.effect` 登记的清理写出登记处的文件、行号和列；其他清理写插件或行名和它在做什么：`still withdrawing calendar`（撤下它提供的服务）、`still running the cleanup its apply returned`（`apply` 返回的清理，rutis-loader 的行就是这种）、`its apply has not returned`、`still waiting for its event calls, or the copies of its plugin definitions, to end`。根不是插件：只有宿主在根上用 `ctx.effect` 登记的清理会列出，写作 `the host`。rutis 自己的插件（链路、运行时等）只在没有开发者的插件卡住时列出，那时它们是仅有的线索。

## 给插件提供 Rust 服务

### 在类型上标注 `#[rutis::service]`

Rust 服务在它的 `impl` 块上加 `#[rutis::service]`，照常 `provide`，各语言的行就能按名字调用它（[#510](https://github.com/arcships/rutis/issues/510)）。不用 `build.rs`，不用写 `HostDispatch`；定义服务的 crate 只依赖 `rutis`。

```rust,ignore
use rutis::host::BoxFuture;
use rutis::service;

#[service]                         // 对外名字 "calendar"：类型名，第一个词小写
impl Calendar {
    pub fn new() -> Self { /* … */ }                          // 没有 self：不对外
    pub async fn add(&self, entry: NewEntry) -> Result<Entry, CalendarError> { /* … */ }
    pub fn get(&self, id: u64) -> Result<Entry, CalendarError> { /* … */ }
    pub async fn subscribe(&self, ctx: &rutis::Ctx, handler: impl Fn(Change) -> BoxFuture<'static, rutis::Result>) { /* … */ }
    #[service(skip)]
    pub fn compact(&mut self) { /* … */ }                     // 留在 Rust 里
}

root.provide(Calendar::new())?;    // 就这些
```

```ts
const calendar = ctx.use<Services['calendar']>('calendar')
const entry = await calendar.add({ title: 'Standup', startsAt: Date.now() })
```

- **对外的方法**：带标注的 `impl` 块里公开的 `&self` 方法；一个类型可以有几个带标注的块。对调用方都是 `async`（JavaScript 里是 Promise，Python 里是 awaitable），Node、Bun、Python、Go 的行调用的是同一个 dispatcher。方法名按 camelCase 对外（`add_entry` 是 `addEntry`）。带 `#[cfg(...)]` 的方法在包含它的构建里对外，也只在这些构建里检查能不能绑定（同名的两个方法，`#[cfg]` 让每次构建只留一个时可以）；同一个类型的不同标注块不能有两个方法映射到同一个对外名字，`provide` 会失败并指出那个名字。参数类型可以写生命周期或 `Self`。
- **名字**：默认是类型名、第一个词小写（`Calendar` 是 `calendar`，`TreeCommands` 是 `treeCommands`，`HTTPClient` 是 `httpClient`）；`#[service(name = "cal")]` 改名。
- **提供即对外**：`ctx.provide(value)`（以及以这个类型自己提供的 `provide_as::<T>`）同时按名字提供它，同一个撤销函数撤下两者，插件卸载时一起撤下；依赖它的行随之停止，与其他服务相同。可写绑定不对外，带名字的（`provide_at(Key::named(…), …)`）也不对外：同一类型的第二个值会占用同一个名字。每次按名字的调用都经提供方上下文的严格读取路径取服务，读到的只能是入口提供时的那个值：服务撤下后仍被保留的引用会被拒绝，即使槽位又被提供。因原生服务或这个名字已存在而被拒的 `provide` 什么都不提供。名字在原生服务之后提供、在它之前撤下（撤销函数与插件卸载都是这样），被名字唤醒的行一定调得到它。
- **作用域跟着提供的位置**：在实例里提供（`ctx.provide_at(Key::of().instance(build.instance("session")?), notes)`），就是那个实例自己的，实例里的行拿到它；在实例外提供就是全局的。在隔离作用域里提供（`ctx.isolate(Key::<Notes>::of(), "a")`）时，名字在同一个作用域里对外（`isolate(host_key("notes"), "a")`），两个隔离作用域各自对外。不用写实例组。实例里提供过之后，实例里的行用的是这个实例自己的，不会退回到同名的全局服务：实例里的服务撤下时，它们等它在这个实例里重新提供（设计 R3：作用域跟着提供的位置）。
- **能传什么、错误怎么抛**：见[参考](#导出服务参考)：serde 参数与返回值、按引用的回调、`&Ctx` 调用方行、`Err(e)` 抛出时带 `name`、`code`、`message`、`data`。
- **编译期报错**：带标注的块里有公开方法绑定不了（`&mut self`、返回借用、可变引用参数、不是函数的泛型参数等），是指到那个方法的编译错误；不想对外的，在方法上加 `#[service(skip)]`。
- **重名**：同一作用域里两个类型对外名字相同，后一个 `provide` 失败，错误写明双方；它什么都没提供，原生服务也不提供。
- **没标注的不对外**：宿主内部的服务（存储、锁、数据库句柄等）不会暴露给插件。一行等着的名字恰好是某个没标注的 Rust 服务的名字时，`Loader::waits(&entry)` 列出这一行等的名字，原因是 `WaitReason::Explained`，说明在那个类型的 `impl` 上加 `#[rutis::service]`。`Loader::check` 把标注类型的名字报告为 `Provider::Marked(类型)`：类型被提供后才对外，检查时无从知道是否提供（`rutis-host check` 也这样输出）。
- **dylib 插件**：dylib 里带标注的类型（crate 只依赖 `rutis-sdk`，写 `#[rutis_sdk::rutis::service]` 或 `use rutis_sdk::rutis::service`）加载后同样对外，宿主不需要知道它；换代时旧代的入口随旧代的服务撤下，新代提供时重新出现。重新构建不用换版本号：同一插件的两次构建类型的 `TypeId` 相同，每个值按提供它的那次构建的代码对外，两次构建可以同时加载。去掉标注的构建，它的值不再按名字对外；名字以最新的构建为准。宿主要链接 `rutis-bridge`。
- **没链接 rutis-bridge**：按名字对外和重名检查都由它做。程序没链接它时，标注的类型只按类型提供，第一次提供时错误汇集收到一次 `ErrorKind::NotServedByName`（默认写 `service calendar is provided by type but not registered by name: depend on rutis-bridge to use it by name and to check clashes`）。
- **声明与 Python 类型**：`rutis-host types <crate 目录> -o services.d.ts` 从源码里找出这个 crate 和它用到的、依赖 rutis 的 crate 里全部带标注的类型，写出 TypeScript 声明（`Services['calendar']`、各数据类型、`<Name>Thrown`）；加 `--python services.py` 同时写出 Python 类型。想每次构建都更新，`build.rs` 里写 `rutis_types::Exports::new().typescript("rows/services.d.ts").generate()`（构建依赖 `rutis-types`），`.python(…)` 同理。调用本身不需要它们。见[声明](#声明)。

完整示例：[`examples/marked-services`](../../examples/marked-services)。

### 第三方类型：包一层自己的类型

改不了源码的类型（别人 crate 里的），Rust 不允许在你的 crate 里给它写 `impl`。在自己的 crate 里包一层新类型，标上 `#[rutis::service]`，要公开的方法逐个转发给被包的那个：

```rust,ignore
use rutis::service;

/// The calendar.                          // 文档注释写进 TypeScript 声明
pub struct Calendar(pub calendar_services::Calendar);

#[service]                                 // 对外名字 "calendar"
impl Calendar {
    /// Add an entry; returns it with its id.
    pub fn add(&self, entry: NewEntry) -> Result<Entry, CalendarError> {
        self.0.add(entry)
    }

    pub async fn get(&self, id: u64) -> Result<Entry, CalendarError> {
        self.0.get(id).await
    }
}

root.provide(Calendar(calendar_services::Calendar::new()))?;
```

- 包好的类型就是一个普通的标注类型，上面的规则都适用：名字默认取包装类型的名字（`#[service(name = "…")]` 改名）；在实例里提供，就是那个实例自己的。
- 参数和返回值直接用依赖 crate 里的类型；生成 TypeScript 声明时，生成器跟着 `use` 读依赖 crate 的源码。
- 只转发要公开的方法；转发方法上的文档注释写进声明。
- 完整示例：[`examples/rust-services`](../../examples/rust-services)：依赖 crate 里的 `Calendar` 给所有行用，`Notes` 每个 `session` 实例一份，`build.rs` 每次构建更新声明。

0.16 在 `build.rs` 里列类型的写法（`Exports::service` / `service_in`）在 0.17 删除了，改法见[迁移到 0.17](../migration-0.17.md#服务清单)。

### 手写 `HostDispatch`

插件按名字使用的服务，在 Rust 里是 `host_key(名字)` 下的 `dyn HostDispatch`；上面的标注就是生成它。分发方式由服务自己决定的（例如给每一行一个自己的对象），手写：

```rust
# use rutis::host::CtxHost;
use std::sync::Arc;
use rutis_bridge::session::{host_key, HostDispatch, Reply, Value};
use serde_json::json;

struct Clock;

impl HostDispatch for Clock {
    fn invoke(&self, method: &str, _args: Value) -> Reply {
        match method {
            "now" => Ok(json!(42).into()),
            other => Err(rutis_bridge::session::Error::Value(format!("no method {other}"))),
        }
    }
    // 方法形状：插件按它决定同步还是异步调用。
    fn methods(&self) -> Option<serde_json::Value> {
        Some(json!({ "now": "sync" }))
    }
}

# fn demo(root: &rutis::Ctx) -> Result<(), Box<dyn std::error::Error>> {
root.provide_at(host_key("clock"), Arc::new(Clock))?; // host_key 返回 Key<dyn HostDispatch>
# Ok(())
# }
```

要让错误像标注服务的 `Err(e)` 那样带 code 和 data，返回 `rutis_bridge::exports::failure(name, code, message, data)`：调用方语言里抛出名为 `name`、消息为 `message` 的错误，附带 `code` 和 `data`（`serde_json::Value` 或 `None`）。

插件提供的服务同样以 `host_key(名字)` 出现在 rutis 里，Rust 插件可以按名字使用它们。

### 在 Rust 里按名字调用服务

宿主或测试用 `rutis_bridge::session::call_service` 一行就能调用任何按名字提供的服务（标注类型、任何语言的行提供的服务、相连节点的、手写的 `HostDispatch`），和别的语言的行调用它一样：

```rust
# use rutis::host::*;
# use rutis::Ctx;
# pub struct Weather;
# #[rutis::service]
# impl Weather {
#     pub async fn today(&self, city: String) -> String { format!("sunny in {city}") }
# }
use rutis_bridge::session::call_service;

# #[tokio::main]
# async fn main() -> Result<(), Box<dyn std::error::Error>> {
# let root = Ctx::root()?;
root.provide(Weather)?;                                          // 一个 #[rutis::service] 类型
let today: String = call_service(&root, "weather", "today", ("Oslo",)).await?;
assert_eq!(today, "sunny in Oslo");
# root.shutdown().await?;
# Ok(())
# }
```

- 参数是元组（`("Oslo", 3)`；一个参数写 `("Oslo",)`，没有参数写 `()`）或列表，像别的语言的调用一样编码；结果解码成你写的类型（任意值用 `serde_json::Value`，不要结果用 `()`）。
- 调用的是 `ctx` 按名字看到的那个服务（`host_key(名字)`）；服务声明为 `sync` 的方法在阻塞线程上执行，其余的按 async 等待。`methods()` 返回 `None` 的手写 `HostDispatch` 全部按 async 算：它的 `invoke` 在调用方的任务上执行，不能阻塞。
- 失败都是 `Err`（`rutis_bridge::session::Error`）：方法自己的错误，带 `code()` 和 `data()`；同步方法 panic 时带着 panic 的信息；没有这个方法；没有提供这个名字的服务时是 `ServiceNotFound`（行还没运行：先等它，见下面的 `reconcile`）。
- 调用不来自任何行：要调用方行的 `&Ctx` 的方法会失败。

### 调用方的行

替插件登记东西（工具、路由、监听）或按插件授权的服务，需要知道是哪一行在调用。`invoke` 期间 `session::caller_row()` 返回调用方的行；把登记挂在这一行的 `Ctx` 上，这一行卸载或重启时它就撤掉，运行时照常运行：

```rust
use rutis_bridge::session::{caller_row, native_error, HostDispatch, Reply, Value};
# #[derive(Clone)]
# struct Tools;
# impl Tools {
#     fn add(&self, _row: &str, _name: &str) {}
#     fn remove(&self, _row: &str, _name: &str) {}
# }

impl HostDispatch for Tools {
    fn invoke(&self, method: &str, args: Value) -> Reply {
        let [name]: [String; 1] = rutis_bridge::session::decode_value(args)?;
        if let Some(row) = caller_row() {
            // row.id()：loader 的行 id；row.key()：它在运行时里的 key（每次重启一个新的）；
            // row.scope()：它隔离这个服务所用的标签（在实例里是实例的），不在作用域里为 None。
            self.add(row.id(), &name);
            let (tools, id) = (self.clone(), row.id().to_owned());
            row.ctx()
                .effect(|| async move {
                    tools.remove(&id, &name);
                    Ok(())
                })
                .map_err(native_error)?;
        }
        Ok(Value::Undefined)
    }
    // methods …
}
```

- 同一个运行时里的两行是两个调用方，`key()` 和 `instance()` 各不相同。
- 来自 Rust 的调用、来自相连节点的调用、运行时里不属于任何行的调用（`Mount::hosts` 提供的服务）、行里没有声明就使用这个服务的代码的调用（Cordis 的 `ctx.get`，找到的是所有行共享的那份注册）都是 `None`。
- 只在 `invoke` 本身里读它：`invoke` 返回的 future 之后在别处运行，读不到。
- 它来自 rutis-loader 的行租用服务的方式（`Process::lease_host_for`）：每一行把它租用的服务注册在自己的作用域里（另有与以前一样、所有行共享的那份注册），所以线上格式不变，Node、Bun、Python、Go 的运行时都可以用，0.11 的也可以。自己用 `Process` 加载行时，用 `lease_host_for` 并按 `HostLease::label()` 隔离这个名字；`lease_host` / `lease_host_in` 的调用是 `None`。

### 收插件定义

其他语言的行可以把插件定义交给 Rust 服务（[插件 API](plugin-api.md#插件定义)）。参数类型写 `rutis::Definition`：方法运行前，宿主先向定义它的运行时读一次它的声明；挂的时候用 `ctx.plugin(def.at(&ctx, config)?)`：

```rust,ignore
use rutis::{Ctx, Definition};

#[rutis::service]
impl Parts {
    pub fn register(&self, scope: String, def: Definition) {
        self.add(scope, def); // def.ended() 完成时撤掉
    }
}

// Session 打开时，在 Session 自己的 ctx 上挂
for def in parts.of("session") {
    ctx.plugin(def.at(&ctx, config.clone())?);
}
```

- 每一份在定义它的运行时里运行，就像那个运行时里的一行：`def.at(&ctx, …)` 按 `ctx` 所在的实例算出键，共享名字按实例隔离，按它声明的 `inject` 门控。挂载需要树上有 rutis-loader。
- 插件定义属于交出它的那一行的这一代（作为参数交出的，是调用方的行）：这一行卸载、重载或结束，或者运行时进程退出时，从它挂出的每一份先结束；之后 `at` 失败，`ended()` 完成。只能挂在那一行所在的实例里，或者更里面。
- 同一个值交出几次都是同一个定义：两个 `Definition` 相等。
- 其他语言的服务返回插件定义时，用 `rutis_bridge::runtime::returned(&*service, value).await` 读出它；它属于提供这个服务的行。
- `rutis-host types` 把 `Definition` 参数和返回值写成 TS 的插件类型。
- 插件定义只交回定义它的运行时：把它交给第三个运行时的调用失败。

## 运行时与加载器

```rust,no_run
# use rutis::host::*;
# use rutis::Ctx;
# use rutis_loader::Builtins;
# use serde_json::json;
# #[tokio::main]
# async fn main() -> Result<(), Box<dyn std::error::Error>> {
# let root = Ctx::root()?;
# let mut builtins = Builtins::new();
# builtins.register("weather", Weather::new);
use std::sync::Arc;
use rutis_bridge::runtime::Launcher;
use rutis_bridge::{LocalId, PeerId};
use rutis_host::connect::SpawnPlugin;
use rutis_loader::runtime::SessionResolver;
use rutis_loader::{Chain, Layer, LoaderOptions, LoaderPlugin, Patch, ServiceCatalog};

// 写绝对路径：每个运行时的进程在它自己的目录里启动（见下面）。
let app = std::env::current_dir()?.join("app");

// 服务名到键：所有名字都按名字跨语言共享，与 rutis-host 相同。
let catalog = ServiceCatalog::new();
// 这一端与每个运行时问候时用的 id：运行时的进程是一个会话的对面。
root.provide(LocalId(PeerId::new("main")?))?;

// 本机的 Node 运行时（行 `node`）：运行的 @arcships/rutis-runtime 包，和插件解析所用的 package.json。
// 事件不用配置：运行时里的监听器自动登记到宿主的事件总线上（见下面的“事件”）。
let node = SpawnPlugin::new("node", Launcher::node(&app.join("node_modules/@arcships/rutis-runtime")), app.join("package.json"));
// 本机的 Python 运行时：插件模块所在的目录，装着 rutis 的解释器。
let python = SpawnPlugin::new("py", Launcher::python(&app.join("plugins")), app.join("plugins"))
    .interpreter(app.join(".venv/bin/python"));
// 运行时上的行：`py:<模块>` 在 py 上；npm 名和文件在 node 上。
let sessions = Arc::new(SessionResolver::npm(&catalog, "node"));
sessions.runtime(node.handle());
sessions.runtime(python.handle());
let runtimes = [node.handle(), python.handle()];
root.plugin(node);
root.plugin(python);

let loader = LoaderPlugin::new(
    // 按顺序问：先是 `builtins` 里登记的 Rust 插件（上面的“装 Rust 插件”），各运行时的行放在最后。
    Chain::new().with(builtins).with_shared(sessions.clone()),
    LoaderOptions { catalog, ..LoaderOptions::default() },
);
let handle = loader.handle();
sessions.attach(&handle);
root.plugin(loader).await?;
// 运行时一运行起来就放行它的行。
sessions.follow_row("node");
sessions.follow_row("py");

let rows: Vec<Patch> = serde_json::from_value(json!([{ "insert": [
    { "id": "forecast", "name": "weather", "config": { "city": "Oslo" } },   // builtins 里的 Rust 插件
    { "id": "web", "name": "weather-plugin", "config": { "city": "Oslo" } },
    { "id": "llm", "name": "py:llm_gateway" }
] }]))?;
// 等运行时起来再 reconcile：报告里才有运行时里的行装载的结果（不等时，这些行先在等运行时）。
for runtime in &runtimes {
    runtime.ready().await;
}
// 每一行都装载过一遍才返回：见下面。
let report = handle.reconcile(vec![Layer::new("app", rows)], None).await?;
for failure in &report.failures {
    eprintln!("{failure}");                                                  // "<行 id>: <错误>"
}
# root.shutdown().await?;
# Ok(())
# }
# #[derive(serde::Deserialize, schemars::JsonSchema)]
# pub struct WeatherConfig { pub city: String }
# pub struct Weather;
# impl Weather {
#     pub fn new(_: &WeatherConfig) -> rutis::Result<Self> { Ok(Weather) }
# }
# impl rutis::Plugin for Weather {
#     type Deps = ();
#     async fn apply(&self, _: &Ctx, _: ()) -> rutis::Result { Ok(()) }
# }
```

- `reconcile` 在每一行都装载过一遍后返回，对每一行就像 `view.await`：服务都在的行已经运行了插件的 `apply`（别的语言的行在它的运行时里运行），或者失败了，列在报告的 `failures` 里；等着一个还没有行提供的服务的行继续等，`Loader::waits(&entry)` 说明它在等什么。还在启动的运行时（比如在等调试器）不等：它的行等它，其他行照常启动，报告里没有它的行装载的结果（还没装载）；要完整的报告，像上面那样先等 `RuntimeHandle::ready()`。`Chain` 按顺序问它的解析器，`builtins` 和运行时都认识的行名归 `builtins`。
- 配置：凡是有 Schema 的行（`Builtins` 的行、写了 `schema:` 的 dylib 行、你自己的 resolver 用 `Resolved::with_schema` 给了 Schema 的行、别的语言运行时的行；Cordis 行除外），loader 在 `build` 之前补默认值并校验，把补过默认值的配置交给插件，不合格的行以 `config: …` 失败，列在 `failures` 里（[插件 API：配置](plugin-api.md#配置)）。多余的键默认写到标准错误（`<行>: config: …`），每行每个配置值一次；`Loader::on_config_warning(Arc::new(|row, issue| …))` 改为交给你。
- 路径：每个运行时的进程在它自己的目录里启动（Python、Go：它的项目目录；Node：运行时的包目录），收到的路径照原样使用。相对路径会从那个目录算起，而不是应用的目录：照上面那样写绝对路径。
- 本机运行时是一个插件 `SpawnPlugin`（即 rutis-host 的 `rutis-host/spawn`）：它启动进程（`Launcher::start`，最后的参数是 `--id <名字>` 和 anchor），在自己下面挂 `rutis_bridge::Session::new(channel)`，它的 fiber 就是以运行时命名的那一行。进程随插件启动、随插件卸载而结束。进程意外结束时，运行时停下，它的行等待；`RuntimeHandle::state()` 是 `Down(原因)`，原因说明进程怎样结束。是否重启由应用决定：对运行时的 fiber 调用 `restart`。
- 运行时结束不算到行头上：结束时还在加载的行和其他行一样等它，不会失败。运行时的 `Down` 原因写明进程怎样结束，正在进行的、对它的行的调用失败时也这样写明。
- 运行时进程不会比应用活得久。丢掉应用的 tokio 运行时会杀掉它们。应用被它没处理的信号（Ctrl-C、SIGTERM）或 `kill -9` 结束时，运行时在 `RUTIS_SHUTDOWN_TIMEOUT`（默认 10 秒）内卸载各行后退出；插件让运行时一直在忙时，再过一秒运行时自己结束。插件让运行时一直在忙时，`shutdown_with_timeout` 可能结束不了（它的行卸载不了）：剩下的用 `SpawnPlugin::processes().kill()` 结束，或者用 `rutis_bridge::pipe::kill_processes()` 结束所有运行时进程，和 rutis-host 一样。
- 远程运行时由连接插件接上：连接插件拿到一条 Channel，调 `ctx.plugin(rutis_bridge::Session::new(channel))`（或先 `Session::handshake(&ctx, channel).await` 看对面的 id，再 `ctx.plugin(greeted)`）；行 `<连接行>:<插件>` 由 loader 找到这个会话、让对面运行。rutis-host 的 `rutis-host/dial`、`rutis-host/listen` 就是这样写的。会话断开又回来时，它的行在启动前由新会话重新解析：远端升级了插件，就用上新的配置 schema 和声明。
- 行的完整格式（分组、`isolate`、`inject`、表达式、命令式修改与持久化）见 [rutis-loader 设计](../design-rutis-loader-2026-10-02.md)。
- 改了插件代码：`Loader::reload(行)`，新代码生效（Node 与 Python 都只重新导入插件的入口模块）。
- 设置 `RUTIS_TRACE` 时，运行时通道上的每条消息在 stderr 记一行（方向和长度，不含内容）。
- 运行时进程默认没有标准输入，标准输出和标准错误用应用自己的。读终端的插件（终端界面、Python 的 `input()`）需要终端本身：它会检查输入是不是终端，并自己设置终端模式。用 `stdio(Stdio::Inherit, Stdio::Inherit, Stdio::Inherit)` 把本进程的标准流交给它的运行时。这一设置作用于整个运行时进程，进程里的每个插件都能读到输入，所以把这类插件放进单独的运行时（另起一个名字的 `SpawnPlugin`），并且终端最多交给一个运行时：多个进程读同一个终端，会争抢每一次按键。其他运行时的标准输出和标准错误设成 `Stdio::Null`，就不往终端上写。`Launcher` 上有同样的设置（`stdin`、`stdout`、`stderr`）。

```rust,no_run
# use rutis_bridge::runtime::Launcher;
# use rutis_host::connect::SpawnPlugin;
# let app = std::path::PathBuf::from("/srv/app");
use rutis_bridge::runtime::Stdio;

// 终端界面放在单独的运行时里，读键盘；插件所在的运行时不往终端写标准输出。
let runtime_package = app.join("node_modules/@arcships/rutis-runtime");
let tui = SpawnPlugin::new("tui", Launcher::node(&runtime_package), app.join("package.json"))
    .stdio(Stdio::Inherit, Stdio::Inherit, Stdio::Inherit);
let node = SpawnPlugin::new("node", Launcher::node(&runtime_package), app.join("package.json"))
    .stdio(Stdio::Null, Stdio::Null, Stdio::Inherit);
```

### 本机运行时的选项

管住第三方插件时用，怎样用见 [做一个开放插件的宿主：管住插件](open-host.md#3-管住插件)：

| `SpawnPlugin` 的方法 | 作用 |
| --- | --- |
| `new(名字, launcher, anchor)` | 运行时 `名字`：它的 fiber 就是这一行，进程问候时也用这个 id。`launcher` 启动进程（`Launcher::node`、`::python`、`::bun`、`::go`、`::swift`，或 `Launcher::new`）；`anchor` 是插件从哪里解析 |
| `.interpreter(程序)` | 用 `程序` 代替 launcher 的程序运行进程（Python 解释器、Node 可执行文件） |
| `.python_path(目录)` | 把 `目录` 放到进程的 `PYTHONPATH` 前面 |
| `.env(名字, 值)` | 设置进程的环境变量 |
| `.stdio(stdin, stdout, stderr)` | 进程的标准流去哪里 |
| `.handle()` | 运行时的状态 `RuntimeHandle`：`state()`、`ready()`、`pid()`；交给 `SessionResolver::runtime` |
| `.processes()` | 它启动的进程：`kill()` 结束它们 |
| `.greeting(时限)` | 进程启动后问候的时限，默认 30 秒；`Duration::MAX` 不限（调试器停在第一行时） |

每个插件一个进程：每个插件一个用它自己名字的 `SpawnPlugin`，行写 `<运行时名>:<npm 名或文件>`，由 `SessionResolver` 解析（Node、Python 都这样）。

### Go 运行时

Go 插件是编译好的二进制，一个二进制一个运行时（features：`rutis-loader` 的 `go`）。`BinaryResolver<Go>` 读二进制的清单（`<二进制> --rutis-manifest`），把 `go:<插件>` 路由到含有它的二进制；`BinaryRuntimes<Go>` 在有行用到时启动它，空闲后停下：

```rust,no_run
# use std::sync::Arc;
# use std::time::Duration;
# use rutis::host::*;
# use rutis_loader::ServiceCatalog;
# #[tokio::main]
# async fn main() -> Result<(), Box<dyn std::error::Error>> {
# let root = rutis::Ctx::root()?;
# let catalog = ServiceCatalog::new();
# let app = std::env::current_dir()?.join("app");
# root.provide(rutis_bridge::LocalId(rutis_bridge::PeerId::new("main")?))?;
use std::path::Path;
use rutis_host::connect::SpawnPlugin;
use rutis_loader::runtime::binary::{Binaries, BinaryResolver, BinaryRuntimes, Spawn, Spawned};
use rutis_loader::runtime::go::Go;

let go = Arc::new(
    BinaryResolver::<Go>::new(Binaries::new().dir(app.join("plugins/go")).file(app.join("bin/netkit")))
        .with_catalog(&catalog)
        .reserve(["node", "py"]),              // 其他运行时的名字，Go 运行时不能用
);
// 启动二进制的运行时的东西：一个 SpawnPlugin，与 rutis-host 相同。
let spawn: Spawn = Arc::new(|name: &str, launcher, anchor: &Path| {
    let plugin = SpawnPlugin::new(name, launcher, anchor);
    Spawned { handle: plugin.handle(), processes: plugin.processes(), plugin: Box::new(plugin) }
});
// 放进加载器的 Chain：Chain::new().with_shared(go.clone())…，加载器挂载之后：
let go_runtimes = BinaryRuntimes::<Go>::new(go, &app, spawn).idle(Some(Duration::from_secs(60)));
let control = go_runtimes.handle();            // restart("go-netkit")、runtimes()
root.plugin(go_runtimes).await?;
# let _ = control;
# Ok(())
# }
```

- 运行时名由文件名得出（`netkit` → `go-netkit`）。两个二进制有同名插件时这一行失败，写出两个文件名。
- 进程运行期间按它启动时的清单解析；换了二进制，`control.restart("go-netkit")` 后生效。
- 加载器不重启崩溃的运行时：它显示为已停止（`control.runtimes()`），它的行等待。`control.restart("go-netkit")` 再启动它。rutis-host 的 `restart` 设置自动做这件事（见 [rutis-host：运行时结束时](rutis-host.md#运行时结束时)）。握手被拒（旧 SDK）的二进制重启也起不来，`rutis_loader::runtime::binary::refused(why)` 判断这种情况，它在文件改变前不再启动。
- Go 之外的编译型语言（之后的 Swift）用同一套实现：`rutis_loader::runtime::binary` 的 `BinaryResolver<L>`、`BinaryRuntimes<L>`，`L` 实现 `BinaryLanguage`（行名前缀、SDK 标记串、启动方式：`launch` 返回一个 `Launcher`）；Go 就是 `L` = `Go`。

- 只运行一个固定的二进制时，也可以像 Python 一样：`SpawnPlugin::new(名字, Launcher::go(&binary, &project), &project)` 加 `SessionResolver`（行名 `<运行时名>:<插件>`）。

## 调试

宿主是一个普通的 Rust 程序，编进去的 Rust 插件和 dylib 插件都在它的进程里：用 IDE、`rust-lldb` 或 `rust-gdb` 运行宿主，断点、调用栈照常。

看宿主里发生了什么，有三样：

- **默认输出**：加载失败、长时间等不到依赖、语言运行时退出，默认写到标准错误，每行以插件、行或运行时的名字开头，不带 `[rutis]` 前缀（可以关掉，见[默认输出](#默认输出)）。`shutdown_with_timeout` 到时写明还没结束的插件和清理。
- **当前状态**：`Loader::status()` 返回一份快照：每一行的状态和原因、每个服务由谁提供、每个运行时（状态、原因、本机运行时的 pid）、正在进行的调用。它实现 `Display`，一项一行，直接打印（格式见 [rutis-host 指南「查看当前状态」](rutis-host.md#查看当前状态)）。用 `rutis-host` 运行时，`rutis-host status` 打印同样的内容。运行时来自 loader 的解析器：`SessionResolver` 列出用 `runtime(handle)` 交给它的本机运行时，Go 与 Swift 的 `BinaryResolver` 列出它们的运行时；自己写的 `Resolver` 实现 `runtimes()` 就会列出自己的。调用记录在运行时一侧，看不到同一个运行时里的调用。`rutis_bridge::runtime::RuntimeState` 实现 `Debug`。
- **内核诊断**：`root.diagnostics()` 的 `Display` 一项一行，用插件和行的名字列出插件（状态、在等什么）和服务（由谁提供），默认不列 rutis 自己的插件和根；`{:#}` 也列出它们。字段（`PluginDiagnostics::platform`、`row` 等）给程序读。

语言运行时（Node、Bun、Python、Go）是单独的进程，各用各的调试器：在插件项目里用 `rutis-host dev --debug` 运行（见 [TypeScript](typescript-plugin.md#调试)、[Bun](bun-plugin.md#调试)、[Python](python-plugin.md#调试)、[Go](go-plugin.md#调试) 指南），或者用状态里的 pid 把调试器接到运行着的进程上。

rutis 不带 `tracing` / `log` 集成，宿主自己的日志照常用。环境变量 `RUTIS_TRACE=1` 在标准错误上打印经过运行时通道的每条消息（只有方向和长度）。

## 节点

0.19 的远程连接只有一种形状（[远程连接：只留一个会话插件](../design-remote-session-2026-10-28.md)）：连接插件拿到一条 `Channel`，交给 `rutis_bridge::Session`。对面是另一个 rutis、语言运行时或设备都一样；对面提供的服务在这里叫 `<对面 id>:<插件>`，这里的服务对面也看得到，事件在一条总线上。宿主要先在根 ctx 提供本端在握手里报的 id：

```rust,ignore
use rutis::host::FiberState;
use rutis_bridge::{websocket, LocalId, PeerId, Session};

root.provide(LocalId(PeerId::new("main")?));

// 一个连接插件的 apply：去连 office，会话卸载后再连。
loop {
    let channel = websocket::dial(
        "wss://office.example.com/rutis",
        &format!("rutis.{}", rutis_bridge::session::ENDPOINT_PROTOCOL),
        Some(&websocket::Credential::Bearer(token.clone())),
        &websocket::Trust::default(),
        &websocket::Limits::default(),
    )
    .await?;
    let session = ctx.plugin(Session::new(channel));
    let mut state = session.watch();
    let _ = state.wait_for(|s| s.state == FiberState::Disposed).await;
    tokio::time::sleep(backoff.next()).await;
}
```

- 要让对面运行一个插件，在 rutis-loader 的配置里写行 `<连接行>:<插件>`，对面不在时这一行等待；Rust 里也可以用 `Session::handshake(&ctx, channel).await?` 拿到 `Greeted`，`greeted.handle().run(插件, 配置)`，再 `ctx.plugin(greeted)` 装上。
- 本端 id 没有提供时，会话失败：`the host provides no id for its sessions`。rutis-host 已经提供（rutis.json 的 `id`，默认 `main`）。
- 别的语言的插件经服务 `sessions` 的 `open` 交出连接，见 [插件 API：自建连接](plugin-api.md#自建连接sessionsopen)。

用 rutis-loader 时，`<连接行>:<插件>` 的行由 `rutis_loader::runtime::SessionResolver` 解析：`SessionResolver::new(&catalog)` 放在解析链最后（`Chain::with_shared`），挂上加载器之前 `attach(&loader)`。加载器把自己能解析的插件提供给会话（`rutis_loader::runtime::LoaderCatalog`），所以对面的 rutis 可以让这里运行它们：只连可信的节点。rutis-host 的 `rutis-host/dial`、`rutis-host/listen` 行就是这样的连接插件。概念与安全见 [连接节点](nodes.md)。

## 事件

宿主里的 Rust 插件、每个运行时里的插件（任何语言）和相连节点上的监听器在一个逻辑总线上（[插件 API：事件](plugin-api.md#事件)）。运行时的监听器自动登记，宿主一侧什么都不用配置：`ctx.emit` 等发出的事件到达所有语言的监听器，它们发出的事件也到达这里的 Rust 监听器。插件里的写法见 [写一个 Rust 插件：事件](rust-plugin.md#5-发出和监听事件)；宿主在根 `Ctx` 上用同样的方法。

事件类型以 `Event::NAME` 作为跨语言的名字，参数按 serde 编解码，所以它和它的 `Value` 都派生 `Serialize` / `Deserialize`。两个及以上字段的元组结构体对应同样多个参数，所以下面的 `Created` 就是别的语言里的 `emit("session/created", id, size)`：

```rust
use rutis::events::Mode;
use rutis::host::*;
use rutis::Event;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
struct Created(String, u32);
impl Event for Created {
    const NAME: &'static str = "session/created";
    type Value = String;
}

# async fn demo(ctx: &rutis::Ctx) -> Result<(), Box<dyn std::error::Error>> {
let off = ctx.on::<Created>(|event, call| async move {                // 监听器以这个 ctx 被调用
    if call.mode() == Mode::Waterfall {
        // 中间件：执行链上其余部分，再改写它的结果
        let inner = call.next().await?;
        return Ok(inner.map(|text| format!("[{text}]")));
    }
    Ok(Some(format!("audited {}", event.0)))                          // Ok(None) 是没有结果
})?;
ctx.on_sync::<Created>(|event, _| Ok(Some(format!("sync {}", event.1))))?;

ctx.emit(Created("s1".into(), 3))?;                                   // 立即返回
ctx.parallel(Created("s1".into(), 3)).await?;                         // 等全部；ListenersFailed
let first: Option<String> = ctx.serial(Created("s1".into(), 3)).await?;
let out = ctx
    .waterfall(Created("s1".into(), 3))
    .terminal(|e| async move { Ok(Some(format!("saved {}", e.0))) })  // 可省略：没有终态
    .await?;
let sync = ctx.bail(Created("s1".into(), 3));                         // 遇到上面的异步监听器：AsyncListener
let python_only = ctx
    .filter(|l| l.runtime.as_deref() == Some("py"))                   // ListenerInfo
    .serial(Created("s1".into(), 3))
    .await?;
off.dispose().await?;
# let _ = (first, out, sync, python_only);
# Ok(())
# }
```

| 事项 | 规则 |
| --- | --- |
| 监听器 | 一个监听器登记一次，五种方式都能调用它；`call.mode()` 是这次的方式，`call.next()` 只在 `waterfall` 里可用（否则 `WaterfallNext`）。`on_sync` 登记同步监听器，它能被所有方式调用，但在异步 `waterfall` 里不能调 `next`；`on` 的（异步）监听器不能被 `bail` 调用（`AsyncListener`） |
| 选项与前缀 | `ctx.listen::<E>()` 后接 `.prefix("session/")`（收这个前缀下的所有名字，`call.name()` 是实际的名字）、`.name(n)`、`.once()`、`.prepend()`、`.global()`，最后是 `.on(…)` 或 `.on_sync(…)` |
| 按名字 | 不定义类型时用 `rutis_bridge::events::Args` 监听：`ctx.listen::<Args>().name("file-changed")`，参数是 `Args(vec![Value::Data(json!(…))])`，结果是 `ArgValue`。同名的类型化监听器与 `Args` 监听器互相收到对方的事件，经 serde 转换。名字由值决定的事件类型把 `NAME` 写成 `""`，重写 `Event::name` |
| 作用域 | 发出方的作用域是 `ctx.scope()`：加载器把每个组实例标为一个作用域。事件按名字送达所有监听器；作用域只是描述，按 `ListenerInfo::scope` 过滤 |
| 失败 | `emit` 的监听器失败以 `ErrorKind::ListenerFailed` 进入错误汇集（`Ctx::root_with_sink`），`rutis_bridge::events::report_line` 给出 rutis-host 打印的那一行；`parallel` 是 `ErrorKind::ListenersFailed`（按登记顺序）；别处监听器的错误带着名字与信息到达 |
| 卸载与取消 | 插件卸载时它的监听器撤下，正在运行的调用看到 `ctx.cancelled()`，卸载等它们结束（最多到关闭截止时间）。丢弃 `parallel` / `serial` / `waterfall` 的 future 即取消这次分发 |
| 框架事件 | `rutis_loader::RowEvent`（`rutis/row`）、`rutis_bridge::events::{ServiceEvent, RuntimeEvent}`（`rutis/service`、`rutis/runtime`）：用 `ctx.on` 监听；发出 `rutis/*` 以 `ReservedEvent` 失败 |
| 节点 | 相连节点的监听器是这条总线上的条目，每个名字都在，不用列名单，见[节点](#节点)与[连接节点](nodes.md#连上之后) |

测试不需要单独的工具：在 `#[tokio::test]` 里用 `Ctx::root()`（或 `Ctx::root_with_sink` 收集 `emit` 的失败），加载插件，再按上面的方式发出、断言（[写一个 Rust 插件：测试](rust-plugin.md#6-测试)）。运行时里的监听器要用真实的运行时（`SpawnPlugin`），与生产相同。

## 导出服务：参考

Rust 服务由各语言的行（TypeScript、JavaScript、Python、Go）按名字使用：带 `#[rutis::service]` 的类型，dispatcher 由宏在编译期生成，提供即对外（[上文](#在类型上标注-rutisservice)）；改不了源码的类型包一层自己的类型再标注（[上文](#第三方类型包一层自己的类型)）。能传什么、错误怎么抛都按下面的规则。TypeScript 声明和 Python 类型由生成器从源码写出：`rutis-host types`，或者 `build.rs` 里的 `rutis_types::Exports`（构建依赖 `rutis-types`）。完整示例：[`examples/marked-services`](../../examples/marked-services)、[`examples/rust-services`](../../examples/rust-services)。

| `Exports` | 作用 |
| --- | --- |
| `.typescript(file)` | 声明写到 `file`（相对 `Cargo.toml`），内容变了才写 |
| `.python(file)` | Python 类型写到 `file`（相对 `Cargo.toml`），内容变了才写 |
| `.declared(path, typescript)` | 生成器推不出时，`path` 处 Rust 类型对应的 TypeScript 类型 |
| `.declared_python(path, python)` | 同一个 `path` 的 Python 类型（要先有 `.declared`）；不给时写 `typing.Any` 并警告 |
| `.generate()` | 在 `build.rs` 里写出给了的文件（至少给 `.typescript(…)`、`.python(…)` 之一），源码变了 Cargo 会重新运行它 |
| `.write(crate 目录)` | 不在构建脚本里时，写出给了的文件（相对当前目录），返回声明的内容（`rutis-host types` 用的就是它） |
| `.declarations(crate 目录)` | 不在构建脚本里时，返回声明的内容 |

### 源码从哪来

生成器从这个 crate 和它用到的、依赖 rutis 的 crate 里找出带标注的类型。`.declared` 的 `path` 按应用自己的写法写：自己的类型写 `crate::…`，依赖的类型写依赖的 crate 名（按 `Cargo.toml` 里的名字，含改名）。生成器运行 `cargo metadata`（只取正在构建的目标平台，先离线），得到每个 crate 的入口文件和 Cargo 解析出的 feature，再用 syn 读模块树：`mod` 文件（含 `#[path]`）、`use`、glob 和 `pub use` 再导出（从私有模块再导出的类型也能找到）、类型别名（含泛型别名）。rutis 本身的源码不读：它的名字按已知路径认（`rutis::Error`、`Result`、`Ctx`、`Definition`，`rutis::host::BoxFuture`、`ErrorKind`），经 `use rutis::*;`、`use rutis::host::*;` 引入的也认。方法是该类型带标注的固有 `impl` 块里公开的 `&self` 方法（`#[service(skip)]` 的除外）。`#[cfg]` 会求值：`feature = "…"` 按 Cargo 解析出的 feature，目标平台的 `cfg` 按 `CARGO_CFG_*`，`test` 和 `doc` 视为关闭。只用稳定版 Rust：不为此编译或展开任何东西。

### 能传什么

| Rust | TypeScript | |
| --- | --- | --- |
| `bool`、`String`、`&str`、`char`、`PathBuf`、`&Path` | `boolean`、`string` | |
| `f32`、`f64`、`u8`–`u32`、`i8`–`i32` | `number` | |
| `u64`、`i64`、`usize`、`isize`、`u128`、`i128` | `number \| bigint` | 超出 ±(2^53−1) 时以 bigint 传（0.11 的值规则）；两种都接受 |
| `Option<T>` | `T \| null` | 末尾的 `Option` 参数可以不传；`Option` 字段是可选的 |
| `Vec<T>`、`&[T]`、集合、`VecDeque<T>`、`[T; N]` / 元组 | `T[]` / `[A, B]` | |
| `HashMap<K, V>`、`BTreeMap<K, V>` | `Record<string, V>` | |
| `Box<T>`、`Arc<T>`、`Rc<T>`、`Cow<T>` | `T` | |
| `serde_json::Value` | `unknown` | |
| `rutis_bridge::session::{Instant, Bytes, BigInt, ValueMap, ValueSet}` | `Date`、`Uint8Array`、`bigint`、`Map`、`Set` | 带标签的值 |
| derive 了 `Serialize` / `Deserialize` 的 struct 或 enum | `interface` 或联合类型，按 serde 写出的样子 | `rename`、`rename_all`、`rename_all_fields`、`tag`、`content`、`untagged`、`transparent`、`flatten`、`skip`、`default`、`skip_serializing_if`；泛型类型在 TypeScript 里也是泛型；文档注释变成 JSDoc |
| `impl Fn(A, ..) -> BoxFuture<'static, Result<X, E>>`、泛型 `F: Fn(..)`、`Arc<dyn Fn .. + Send + Sync>`、`Box<dyn Fn .. + Send>`（含别名） | `(arg0: A, ..) => X \| Promise<X>` | 按引用传的函数；最多四个参数，按值取 |
| `async fn`，或返回 `BoxFuture<'_, T>`、`Pin<Box<dyn Future<Output = T> ..>>`、`impl Future<Output = T>` | `Promise<T>` | 对调用方而言每个方法都返回 `Promise` |

参数用 serde 解码（任意 `Deserialize`），结果用 serde 编码（任意 `Serialize`）；编译器按真实签名检查宏生成的每个调用。

**按引用传的函数：** Rust 一侧拿到的函数会调用调用方的函数：它的 future 在函数返回、且返回的 Promise settle 之后才完成，所以订阅者处理完事件，原生一侧才继续；抛错或 reject 是这个 future 的 `Err`（要求 `E: From<rutis::host::CallError>`，`rutis::Error`、`anyhow::Error`、装箱的错误都满足），走原生的失败路径。Rust 期望 `()` 时，函数返回的值被忽略。

**取消：** 调用方不再等待的调用（`AbortSignal`、丢弃 future）会在 Rust 一侧丢弃方法的 future，与所有调用相同。

**调用方行的上下文：** `&rutis::Ctx`（或 `Ctx`）参数在 TypeScript 里不是参数：它是发起调用的那一行的上下文（`rutis_bridge::session::caller_row()`），方法在上面登记的东西（`ctx.effect`、监听）属于那一行，随该行卸载或重启而移除，例如订阅。不在任何行里调用时（从 Rust、从相连节点），这样的方法失败，错误写明它需要调用方行的上下文。

### 错误

返回 `Result<T, E>` 的方法（`std::result::Result`、它的别名、`anyhow::Result`、`io::Result`）把 `Err(e)` 作为调用方语言里抛出的错误：JavaScript 的 `Error`、Python 的 `rutis.RemoteError`、Go 的 `*rutis.RemoteError`。它不是未分类的调用错误：

| `E` | `name` | `code` | `data` |
| --- | --- | --- | --- |
| 有 serde 的枚举 | 类型名 | serde 写出的变体：序列化成的字符串、外部标签变体的唯一键、内部或相邻标签变体的 `tag` 字段（内部或相邻标签枚举里单独 `rename` 的变体例外，见[限制](#限制)） | serde 值 |
| 没有 serde（或 `untagged`）的枚举 | 类型名 | 变体的 Rust 名（取自 derive 的 `Debug`） | 有 serde 时为 serde 值 |
| 有 serde 的结构体 | 类型名 | 序列化成的字符串，否则为类型名 | serde 值 |
| `String` | `Error` | `Error`（字符串是 `message`） | — |
| `rutis::Error`、`rutis::host::ErrorKind` | `Error`（声明里是 `ErrorThrown`） | `ErrorKind` 的变体名（插件代码的错误是 `PluginFailed`） | — |
| `rutis_bridge::session::Error` | 原样抛出（转发的调用保留远端的错误） | | |
| 其他（`anyhow::Error`、`io::Error`） | 类型名 | 类型名 | — |

`message` 总是 `e` 的 `Display`。JavaScript 读 `error.code`、`error.data`；Python 读 `error.code`、`error.data`；Go 读 `remote.Code`、`remote.Data`（大整数为 `*big.Int`）；Rust 用 `Error::code()`、`Error::data()`。它们放在错误图里传，不认识它们的运行时仍拿到名字和消息。声明里每个错误类型有一个 `<Name>Thrown` 类型 `RustError<'Name', 'Code1' | 'Code2', Data>`，写在方法的 JSDoc 里（`@throws {CalendarErrorThrown}`）。`ErrorThrown` 留给 rutis 自己的错误；别的叫 `Error` 的类型、以及与已有的同名而不同的错误类型，按它所在的模块命名：`io::Error` 是 `IoErrorThrown`，`anyhow::Error` 是 `AnyhowErrorThrown`，`notes::Error` 是 `NotesErrorThrown`。抛出时的 `name` 仍是类型自己的名字（`Error`）。

### 声明

`rutis-host types` 和 `.typescript(…)` 写出的文件包含每个服务类型的接口、它用到的数据类型、`RustError`、各 `<Name>Thrown` 类型，以及按名字列出服务的 `Services`：`ctx.use<Services['calendar']>('calendar')`。它能通过 `tsc --strict`（两个示例都会检查）。写出的文件要提交：Rust 一侧变了，`build.rs` 会改写它；用 `rutis-host types` 的，再运行一次。

宿主把插件交给别人（或别的团队）用 Python 写时，同时写出 Python 类型，和插件模板一起给出：

```sh
rutis-host types . -o types/deskmate.d.ts --python types/deskmate.py
```

`build.rs` 里同样是 `Exports::new().typescript(…).python(…)`。

**Python 类型**（Python 3.10 起）：服务是 `typing.Protocol`，结构体是 `TypedDict`（可省略的字段放进 `total=False` 的父类；字段名不是标识符时用函数式写法），只有名字的枚举是 `Literal`，带数据的枚举是 `TypedDict` 的联合，`Option<T>` 是 `T | None`，`Vec<T>`、`HashMap<String, T>`、元组是 `list`、`dict[str, T]`、`tuple`，整数、浮点是 `int`、`float`，`serde_json::Value` 是 `Any`，回调是 `Callable[[A], R | Awaitable[R]]`，`rutis::Definition` 是 `rutis.Plugin`，错误类型 `<Name>Thrown` 是 `rutis.RemoteError` 的别名、`<Name>ThrownCode` 是它的错误码 `Literal`。泛型结构体在 Python 里不带类型参数（3.10 的 `TypedDict` 不能是泛型），参数是 `Any`。插件作者写 `notes: Notes = ctx.use("notes")`，由编辑器或 pyright 检查调用，不写也照样运行（见 [Python 插件：宿主给的类型](python-plugin.md#宿主给的类型)）。它能通过 pyright 的严格模式（`examples/marked-services` 会检查），同样要提交。`.declared` 的类型没有 Python 版本时写 `Any` 并警告：

```text
$ rutis-host types . --python types/deskmate.py
warning: notes::Text has no Python type: declare it with .declared_python (written as Any)
```

### 限制

- 生成器看不到宏写出的东西：宏（derive 除外）写出的类型、`impl` 块或方法找不到。服务类型必须是具体的 struct 或 enum（不能是泛型，不能是 trait 对象）。
- 取 `&mut self`、返回借用、函数参数取引用（`Fn(&A)`）、除函数以外的泛型参数、除 `Fn` 以外的 `impl Trait`、不返回 `Result` 的 future 的函数参数，这样的方法绑定不了：宏报编译错误（见[上文](#在类型上标注-rutisservice)），生成器写声明时也跳过它并给出警告。
- 数据类型需要生成器能读懂的 serde derive：手写的 `Serialize`、`#[serde(with / serialize_with / into / remote)]`，或者不读源码的 crate 的类型（`std`、`serde_json`、`chrono`、`uuid`、`url` 和带标签的值除外）需要 `.declared(path, typescript)`；在那之前声明里没有用到它的方法（调用不受影响），警告里写明类型。写 Python 类型时，再用 `.declared_python(path, python)` 给出它的 Python 类型。
- `#[cfg]` 按上面的规则求值；生成器看不到的 `cfg` 视为关闭。
- 方法的 future 必须是 `Send`（否则编译失败）。
- 宏只看到 `impl` 块本身，类型上的判断（是不是 `Result`、错误怎么取 `code`、参数是不是函数）交给展开后的具体代码，所以类型别名照样识别。异步方法写成 `async fn`，或返回 `BoxFuture<'_, T>`、`Pin<Box<dyn Future<Output = T> + Send>>`、`impl Future<Output = T>`；返回 future 的别名是编译错误。`&Ctx` 参数按名字 `Ctx` 识别。
- 错误的 `code` 在抛出时由值得出，规则同上表；内部或相邻标签枚举的变体用 `#[serde(rename = "...")]` 改成与 Rust 名无关的名字时，`code` 是 Rust 名。
- 宏生成的代码只依赖 `rutis`（dylib 插件写 `#[rutis_sdk::rutis::service]` 或 `use rutis_sdk::rutis::service`，生成的代码就经 `rutis_sdk::rutis`：按标注是从哪条路径拿到的来定，所以对着 sdk-bundle 构建、没写依赖的插件也一样；别的路径写 `#[service(crate = "my::rutis")]`）；值经宿主链接的 `rutis-bridge` 编解码。
