# 写一个 Rust 插件

从起项目到测试、在一个宿主里运行，再到发布。需要 Rust 1.88 或更高。

Rust 插件和其他语言的插件规则相同（见 [插件 API](plugin-api.md)），不同的是：**Rust 插件编进 Rust 宿主**，没有 `rutis-host new` 的模板和 `rutis-host dev`。插件是一个普通的 library crate，宿主把它加进依赖，在自己的代码里装上它（[在 Rust 应用里嵌入](rust-host.md)）。

全篇做同一个插件 `greeter`：它提供服务 `Greeter`（`hello(name)` 返回问候），用另一个插件提供的 `Translate` 服务翻译问候语，问候语来自配置，每次问候发出事件 `greeter/greeted`。

## 1. 起项目

<!-- example: rust run -->
```bash
cargo new --lib greeter
cd greeter
cargo add rutis serde --features serde/derive
cargo add --dev tokio --features macros,rt-multi-thread
cargo test
```

- `rutis`：插件 API。
- `serde`：配置和事件的类型派生 `Serialize` / `Deserialize`。
- `tokio`（开发依赖）：测试和第 7 步的宿主用它运行。插件本身不依赖 tokio。

`cargo test` 跑的是 `cargo new` 生成的测试；第 2 步起换成插件的代码。

## 2. 提供服务

服务就是一个类型；插件实现 `rutis::Plugin`，在 `apply` 里用 `ctx.provide` 提供它。`src/lib.rs` 换成：

<!-- example: rust file=greeter/src/lib.rs -->
```rust
use rutis::{Ctx, Plugin, Result};

/// greeter 提供的服务。
pub struct Greeter;

impl Greeter {
    pub fn hello(&self, name: &str) -> String {
        format!("Hello, {name}!")
    }
}

/// 插件。
pub struct GreeterPlugin;

impl Plugin for GreeterPlugin {
    type Deps = ();

    async fn apply(&self, ctx: &Ctx, _: ()) -> Result {
        ctx.provide(Greeter)?;
        Ok(())
    }
}
```

- `apply` 里经 `ctx` 登记的一切（服务、监听器）在插件停下时自动撤销，不用自己清理。
- `type Deps` 必须写；没有依赖时写 `()`，`apply` 的参数写 `_: ()`。
- 插件和它提供的服务要是 `Send + Sync`：它们在 tokio 上运行，可能同时被调用。
- 让别的语言的插件也能按名字调用这个服务，见[对其他语言提供服务](#对其他语言提供服务)。

## 3. 使用服务

要用的服务写进 `type Deps`，`apply` 收到它。`Translate` 是一个 trait，由别的插件提供（第 6 步的测试和第 7 步的宿主各提供一个假的）：

<!-- example: rust file=greeter/src/lib.rs -->
```rust
use std::sync::Arc;

use rutis::{Ctx, Plugin, Result};

/// greeter 用的服务，由别的插件提供。
pub trait Translate: Send + Sync {
    fn text(&self, text: &str) -> String;
}

/// greeter 提供的服务。
pub struct Greeter {
    translate: Arc<dyn Translate>,
}

impl Greeter {
    pub fn hello(&self, name: &str) -> String {
        format!("{}, {name}!", self.translate.text("Hello"))
    }
}

/// 插件。
pub struct GreeterPlugin;

impl Plugin for GreeterPlugin {
    type Deps = Arc<dyn Translate>;

    async fn apply(&self, ctx: &Ctx, translate: Arc<dyn Translate>) -> Result {
        ctx.provide(Greeter { translate })?;
        Ok(())
    }
}
```

`Deps` 里的服务都就绪之后 `apply` 才执行；其中一个被撤销或替换时，插件先停下，等它重新就绪再执行一次 `apply`。几个服务写成元组：`type Deps = (Arc<A>, Arc<dyn B>);`。

## 4. 配置

配置是一个结构体，插件从它构造：`GreeterPlugin::new(&config)`。不写的字段取 `Default` 里的值，这里 `greeting` 默认是 `"Hello"`：

<!-- example: rust file=greeter/src/lib.rs -->
```rust
use std::sync::Arc;

use rutis::{Ctx, Plugin, Result};
use serde::Deserialize;

/// greeter 用的服务，由别的插件提供。
pub trait Translate: Send + Sync {
    fn text(&self, text: &str) -> String;
}

/// 配置。不写的字段取 Default 里的值。
#[derive(Clone, Deserialize)]
#[serde(default)]
pub struct Config {
    pub greeting: String,
}

impl Default for Config {
    fn default() -> Self {
        Config { greeting: "Hello".into() }
    }
}

/// greeter 提供的服务。
pub struct Greeter {
    greeting: String,
    translate: Arc<dyn Translate>,
}

impl Greeter {
    pub fn hello(&self, name: &str) -> String {
        format!("{}, {name}!", self.translate.text(&self.greeting))
    }
}

/// 插件。
pub struct GreeterPlugin {
    config: Config,
}

impl GreeterPlugin {
    pub fn new(config: &Config) -> Result<Self> {
        Ok(GreeterPlugin { config: config.clone() })
    }
}

impl Plugin for GreeterPlugin {
    type Deps = Arc<dyn Translate>;

    async fn apply(&self, ctx: &Ctx, translate: Arc<dyn Translate>) -> Result {
        ctx.provide(Greeter {
            greeting: self.config.greeting.clone(),
            translate,
        })?;
        Ok(())
    }
}
```

配置从哪里来：测试里自己写（第 6 步），宿主里由宿主传入（第 7 步）。宿主用 rutis-loader 按 `rutis.json` 的行装插件时，行里的 `config` 按 `Config` 解码，`#[serde(default)]` 补上没写的字段；那时 `Config` 还要派生 `schemars::JsonSchema`，见[由 rutis-loader 按配置装](#由-rutis-loader-按配置装)。

## 5. 发出和监听事件

事件是一个类型：`NAME` 是它在所有语言里的名字，字段按 serde 编解码。`Greeter` 保存插件的 `ctx`，在 `hello` 里用 `ctx.emit` 发出 `greeter/greeted`；插件在 `apply` 里用 `ctx.on` 监听同一个事件，数一共问候了几次，`count()` 返回这个数：

<!-- example: rust file=greeter/src/lib.rs -->
```rust
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use rutis::{Ctx, Event, Plugin, Result};
use serde::{Deserialize, Serialize};

/// greeter 用的服务，由别的插件提供。
pub trait Translate: Send + Sync {
    fn text(&self, text: &str) -> String;
}

/// 配置。不写的字段取 Default 里的值。
#[derive(Clone, Deserialize)]
#[serde(default)]
pub struct Config {
    pub greeting: String,
}

impl Default for Config {
    fn default() -> Self {
        Config { greeting: "Hello".into() }
    }
}

/// 每次问候发出，带着名字。
#[derive(Serialize, Deserialize)]
pub struct Greeted(pub String);

impl Event for Greeted {
    const NAME: &'static str = "greeter/greeted";
    type Value = (); // 只发出，不要结果
}

/// greeter 提供的服务。
pub struct Greeter {
    greeting: String,
    translate: Arc<dyn Translate>,
    greeted: Arc<AtomicU64>,
    ctx: Ctx,
}

impl Greeter {
    pub fn hello(&self, name: &str) -> Result<String> {
        self.ctx.emit(Greeted(name.into()))?; // 立即返回，不等监听器
        Ok(format!("{}, {name}!", self.translate.text(&self.greeting)))
    }

    pub fn count(&self) -> u64 {
        self.greeted.load(Ordering::SeqCst)
    }
}

/// 插件。
pub struct GreeterPlugin {
    config: Config,
}

impl GreeterPlugin {
    pub fn new(config: &Config) -> Result<Self> {
        Ok(GreeterPlugin { config: config.clone() })
    }
}

impl Plugin for GreeterPlugin {
    type Deps = Arc<dyn Translate>;

    async fn apply(&self, ctx: &Ctx, translate: Arc<dyn Translate>) -> Result {
        let greeted = Arc::new(AtomicU64::new(0));
        let count = greeted.clone();
        ctx.on::<Greeted>(move |_, _| {
            count.fetch_add(1, Ordering::SeqCst);
            async { Ok(None) } // Ok(None)：没有结果
        })?;
        ctx.provide(Greeter {
            greeting: self.config.greeting.clone(),
            translate,
            greeted,
            ctx: ctx.clone(),
        })?;
        Ok(())
    }
}
```

- 一个宿主上所有语言的监听器都在一条事件总线上：别的语言的插件 `on('greeter/greeted', …)` 同样收到，参数是名字。一个字段的结构体是一个参数，两个及以上字段的元组结构体是同样多个参数，unit 结构体没有参数。
- 服务保存插件 `ctx` 的克隆没有问题：它只是一个句柄，总以这个插件的身份发出。插件停下后，用它登记的东西以 `InactiveEffect` 被拒。
- 其余发出方式（`parallel`、`serial`、`waterfall`、`bail`）、`on_sync`、前缀监听器与过滤见 [插件 API：事件](plugin-api.md#rust)。

## 6. 测试

测试不需要单独的工具：在 `#[tokio::test]` 里建一个根 `Ctx`，提供一个假的 `Translate`，装上插件，再断言。建根、装插件、关闭是宿主的方法，要 `use rutis::host::*;`。在 `src/lib.rs` 末尾加上测试模块，整个文件是：

<!-- example: rust file=greeter/src/lib.rs -->
```rust
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use rutis::{Ctx, Event, Plugin, Result};
use serde::{Deserialize, Serialize};

/// greeter 用的服务，由别的插件提供。
pub trait Translate: Send + Sync {
    fn text(&self, text: &str) -> String;
}

/// 配置。不写的字段取 Default 里的值。
#[derive(Clone, Deserialize)]
#[serde(default)]
pub struct Config {
    pub greeting: String,
}

impl Default for Config {
    fn default() -> Self {
        Config { greeting: "Hello".into() }
    }
}

/// 每次问候发出，带着名字。
#[derive(Serialize, Deserialize)]
pub struct Greeted(pub String);

impl Event for Greeted {
    const NAME: &'static str = "greeter/greeted";
    type Value = (); // 只发出，不要结果
}

/// greeter 提供的服务。
pub struct Greeter {
    greeting: String,
    translate: Arc<dyn Translate>,
    greeted: Arc<AtomicU64>,
    ctx: Ctx,
}

impl Greeter {
    pub fn hello(&self, name: &str) -> Result<String> {
        self.ctx.emit(Greeted(name.into()))?; // 立即返回，不等监听器
        Ok(format!("{}, {name}!", self.translate.text(&self.greeting)))
    }

    pub fn count(&self) -> u64 {
        self.greeted.load(Ordering::SeqCst)
    }
}

/// 插件。
pub struct GreeterPlugin {
    config: Config,
}

impl GreeterPlugin {
    pub fn new(config: &Config) -> Result<Self> {
        Ok(GreeterPlugin { config: config.clone() })
    }
}

impl Plugin for GreeterPlugin {
    type Deps = Arc<dyn Translate>;

    async fn apply(&self, ctx: &Ctx, translate: Arc<dyn Translate>) -> Result {
        let greeted = Arc::new(AtomicU64::new(0));
        let count = greeted.clone();
        ctx.on::<Greeted>(move |_, _| {
            count.fetch_add(1, Ordering::SeqCst);
            async { Ok(None) } // Ok(None)：没有结果
        })?;
        ctx.provide(Greeter {
            greeting: self.config.greeting.clone(),
            translate,
            greeted,
            ctx: ctx.clone(),
        })?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rutis::host::*;
    use tokio::sync::mpsc;

    /// 假的 Translate：认得两个词，其余原样返回。
    struct FakeTranslate;

    impl Translate for FakeTranslate {
        fn text(&self, text: &str) -> String {
            match text {
                "Hello" => "Bonjour".into(),
                "Hi" => "Salut".into(),
                other => other.into(),
            }
        }
    }

    #[tokio::test]
    async fn greets_with_the_configured_greeting() -> Result {
        let root = Ctx::root()?;
        root.provide_as::<dyn Translate>(Arc::new(FakeTranslate))?;
        let plugin = root.plugin(GreeterPlugin::new(&Config { greeting: "Hi".into() })?);
        (&plugin).await?; // apply 执行完

        // 像宿主一样在根上监听插件发出的事件
        let (sender, mut greeted) = mpsc::unbounded_channel();
        root.on::<Greeted>(move |event, _| {
            let _ = sender.send(event.0.clone());
            async { Ok(None) }
        })?;

        let greeter = root.get::<Greeter>().unwrap();
        assert_eq!(greeter.hello("Ada")?, "Salut, Ada!");
        assert_eq!(greeted.recv().await.as_deref(), Some("Ada"));
        assert_eq!(greeter.count(), 1);

        plugin.dispose().await?; // 停下：服务和监听器撤销
        assert!(root.get::<Greeter>().is_none());
        root.shutdown().await?;
        Ok(())
    }
}
```

<!-- example: rust run -->
```bash
cargo test
```

- `root.provide_as::<dyn Translate>(…)` 以 trait 对象的类型提供服务；`root.get::<T>()` 取一个服务，没有时是 `None`。
- `(&plugin).await` 在插件装载处理完后返回。依赖缺着的插件这时还没运行，所以先提供假服务，再装插件。
- `emit` 不等监听器，所以测试等自己的监听器收到事件，再看 `count()`。
- `emit` 的监听器失败进入根的错误汇集；要在测试里断言它们，用 `Ctx::root_with_sink(handle, sink)` 建根。

## 7. 在一个宿主里运行

Rust 插件由 Rust 宿主装。项目的 `examples/` 里写一个最小的宿主：建根，装上一个提供假 `Translate` 的插件和 `greeter`，调用一次。`examples/host.rs`：

<!-- example: rust file=greeter/examples/host.rs -->
```rust,ignore
use std::sync::Arc;

use greeter::{Config, Greeter, GreeterPlugin, Translate};
use rutis::host::*;
use rutis::{Ctx, Plugin, Result};

/// 假的 Translate：认得两个词，其余原样返回。
struct FakeTranslate;

impl Translate for FakeTranslate {
    fn text(&self, text: &str) -> String {
        match text {
            "Hello" => "Bonjour".into(),
            "Hi" => "Salut".into(),
            other => other.into(),
        }
    }
}

/// 提供假的 Translate 的插件。
struct FakeTranslatePlugin;

impl Plugin for FakeTranslatePlugin {
    type Deps = ();

    async fn apply(&self, ctx: &Ctx, _: ()) -> Result {
        ctx.provide_as::<dyn Translate>(Arc::new(FakeTranslate))?;
        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result {
    let root = Ctx::root()?; // 用当前的 tokio 运行时
    root.plugin(FakeTranslatePlugin).await?;
    root.plugin(GreeterPlugin::new(&Config::default())?).await?;

    let greeter = root.get::<Greeter>().unwrap();
    println!("{}", greeter.hello("Ada")?);
    root.shutdown().await?; // 等所有插件停下
    Ok(())
}
```

<!-- example: rust run -->
```bash
cargo run --example host
```

```console
$ cargo run --example host
Bonjour, Ada!
```

- 宿主决定装哪些插件、什么时候装：`root.plugin(插件)` 装一个，返回的句柄 `.await` 等它装载处理完，`dispose()` 卸载它，`root.shutdown()` 卸载全部。
- 装的顺序不要紧：先装 `greeter` 时它等着，`Translate` 出现后才执行 `apply`。上面先装假的 `Translate`，所以 `greeter` 装完就能用。
- 改了插件的代码，再 `cargo run --example host` 一次：Rust 插件随宿主一起编译，没有运行中的重新加载。
- 宿主还能用 rutis-loader 按 `rutis.json` 装别的语言的插件，与 Rust 插件在同一条事件总线上互相调用：见 [在 Rust 应用里嵌入](rust-host.md#运行时与加载器)。

## 8. 发布

发布到 crates.io：`Cargo.toml` 补上 `description` 与 `license`（crates.io 要求），包名在 crates.io 上全局唯一，例如 `your-name-greeter`。依赖范围写 `rutis = "0.19"`，即 `>=0.19.0, <0.20.0`：rutis 0.x 的小版本可能有不兼容的改动。

```toml
[package]
name = "your-name-greeter"
version = "0.1.0"
edition = "2024"
description = "Greets in the configured language"
license = "MIT"

[lib]
name = "greeter"

[dependencies]
rutis = "0.19"
serde = { version = "1", features = ["derive"] }

[dev-dependencies]
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
```

打 `v*` tag 时测试并发布（`CARGO_REGISTRY_TOKEN` 在仓库的 Secrets 里设置）：

<!-- example: rust file=greeter/.github/workflows/publish.yml -->
```yaml
name: publish

on:
  push:
    tags: ["v*"]

permissions:
  contents: read

jobs:
  publish:
    runs-on: ubuntu-24.04
    steps:
      - uses: actions/checkout@v4
      - run: cargo test
      - run: cargo publish
        env:
          CARGO_REGISTRY_TOKEN: ${{ secrets.CARGO_REGISTRY_TOKEN }}
```

宿主怎样装你的插件：加进它的 `[dependencies]`，在代码里 `root.plugin(…)` 装上，或者登记给 rutis-loader 按配置装，见 [在 Rust 应用里嵌入：装 Rust 插件](rust-host.md#装-rust-插件)。

## 调试

插件在宿主的进程里运行：用 IDE、`rust-lldb` 或 `rust-gdb` 运行宿主（第 7 步的 `target/debug/examples/host`，或者 `cargo test` 的测试二进制），在插件代码里下断点。宿主里每一行的状态、服务的提供者和进行中的调用怎样看，见 [在 Rust 应用里嵌入：调试](rust-host.md#调试)。

## Rust 特有的事

### 对其他语言提供服务

服务类型的 `impl` 块上标 `#[rutis::service]`，照常 `provide`，各语言的行就能按名字调用它；名字来自类型（`Calendar` 是 `calendar`），`#[service(name = "…")]` 可以改。宿主要链接 `rutis-bridge`：

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

没标注的类型不对外。能传什么、错误怎样抛、带 `&Ctx` 参数的方法，见 [在类型上标注 `#[rutis::service]`](rust-host.md#在类型上标注-rutisservice)。服务的参数收数据和函数，不收带方法的对象：别的语言把对象交给这样的参数时，这次调用以 `bad-argument` 失败（[插件 API：带方法的对象](plugin-api.md#带方法的对象)）。

### `type Deps` 能写的

| `type Deps =` | 等什么 | 传入 |
| --- | --- | --- |
| `()` | 不等 | `()` |
| `Arc<T>` | `T` | 服务 |
| `Option<Arc<T>>` | 不等 | 装载时可见就有；之后出现或消失都不重载 |
| 至多 8 个成员的元组 | 全部成员 | 各自的值 |

`T` 可以是 trait 对象（`Arc<dyn Translate>`）。`ctx.get::<T>()` 是可选读取，不等、不建立重载关系；需要的服务写进 `Deps`。同一个类型的两个服务按名字区分时，见 [在 Rust 应用里嵌入：按名字依赖](rust-host.md#按名字依赖)。

插件的其余可选项：`fn name(&self) -> &str`（日志和诊断里的名字，默认是类型名的最后一段）；`fn validate(&self) -> Result`（装载时、`apply` 之前执行，失败时插件进入 Failed，不执行 `apply`）。

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

- `ctx.effect(|| { …; async move { … } })` 立即执行闭包，把它返回的 future 登记为清理；插件停下时和其他登记一起按相反顺序执行。插件已经在停下时，`effect`（以及 `provide`、`on`）返回 `InactiveEffect`，闭包不执行，所以在闭包里拿的资源不会泄漏。
- 后台任务用 `tokio::spawn`：在 `effect` 的闭包里起，在清理里等它退出；任务里看 `ctx.cancelled()`，插件开始停下时它完成。停下等清理多久由宿主决定：Rust 宿主的 `shutdown()` 一直等，`shutdown_with_timeout(时长)` 限时。
- 执行时间长的监听器同样看 `call.ctx().cancelled()`。`ctx.plugin(child)` 装一个随这个插件一起停下的子插件；要交给别处、挂很多次的插件用 `rutis::Definition`，见 [插件 API：插件定义](plugin-api.md#插件定义)。

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
    check_name("greeter").map_err(Error::msg)?;     // String 不是错误类型
    Ok(limit - used)
}

fn check_name(_: &str) -> std::result::Result<(), String> {
    Ok(())
}
```

`anyhow::Error` 不实现 `std::error::Error`，写不成 `?`：用 `.map_err(|e| Error::msg(format!("{e:#}")))?`。插件的错误报到宿主时包在 `PluginFailed` 里，带着原来的错误作为 source，见 [在 Rust 应用里嵌入：宿主看到的错误](rust-host.md#宿主看到的错误)。

### 由 rutis-loader 按配置装

宿主用 rutis-loader 按 `rutis.json` 的行装插件时，把构造函数登记成一个名字。配置的 JSON Schema 从 `Config` 生成，所以 `Config` 还要派生 `schemars::JsonSchema`（`cargo add schemars@1`，与 rutis-loader 用的版本相同）：

```rust,ignore
#[derive(Clone, Deserialize, schemars::JsonSchema)]
#[serde(default)]
pub struct Config {
    pub greeting: String,
}

let mut builtins = Builtins::new();
builtins.register("greeter", GreeterPlugin::new); // 行：{ "id": "greeter", "name": "greeter", "config": { "greeting": "Hi" } }
```

`new` 之前，loader 给这一行的配置补上 Schema 里的默认值并校验：配置不合格的行以 `config: …` 失败，不调用构造函数（[插件 API：配置](plugin-api.md#配置)）。加载器、运行时和其他语言的行见 [在 Rust 应用里嵌入](rust-host.md#运行时与加载器)。

### dylib 插件

同一个插件也可以编译成 dylib，由宿主在运行时加载。crate 只依赖 `rutis-sdk`，经 `rutis_sdk::rutis` 用 rutis（`use rutis_sdk::rutis::{Ctx, Plugin, Result};`），再用构造函数导出：

```rust,ignore
rutis_sdk::export_plugin!(id: "greeter", build: GreeterPlugin::new);
```

- `build:` 接受和 `Builtins::register` 相同的构造函数；`schema:` 可选，给出配置的 JSON Schema。
- 事件和配置类型用 SDK 里的 serde 派生：`#[derive(rutis_sdk::rutis::serde::Deserialize)]` 加 `#[serde(crate = "rutis_sdk::rutis::serde")]`。
- dylib 插件要用和宿主完全相同的 SDK 构建。工作区、打包和排错见 [外部插件开发指南](../external-plugin-guide.md)。

## 进阶：插件 API

插件作为值、其余事件方式、作用域、取消、值怎样跨语言传递：见 [插件 API](plugin-api.md)。
