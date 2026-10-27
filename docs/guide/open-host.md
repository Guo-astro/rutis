# 做一个开放插件的宿主

[English](open-host.en.md)

读者：在自己的 Rust 产品里接入 rutis，并把插件能力开放给别的开发者。全篇的例子是命令行记事工具 `jot`：宿主提供 `notes` 服务，第三方用 TypeScript 或 Python 写命令。

本指南的其他各节还在写（#671）；API 的完整参考见 [Rust 宿主](rust-host.md)。

## 3. 管住插件

第三方插件不归你管：它可能崩溃、卡死、调用你不想给它的服务。下面这些都是 `LocalRuntime` 上的选项，默认都关；不设时与没有这些选项时相同。

### 每个插件一个进程

同一个运行时里的插件共用一个进程：一个插件崩溃或卡死，其他插件跟着停下。给每个第三方插件一个自己的运行时（`named`），它的行用 `RuntimeResolver::modules`，行名写 `<运行时名>:<npm 名或文件>`：

```rust,no_run
# use std::sync::Arc;
# use std::time::Duration;
# use rutis::host::*;
# #[tokio::main]
# async fn main() -> Result<(), Box<dyn std::error::Error>> {
# let root = rutis::Ctx::root()?;
# let app = std::env::current_dir()?.join("app");
# struct Command { id: String, dir: std::path::PathBuf }
# let commands: Vec<Command> = Vec::new();
use rutis_bridge::runtime::LocalRuntime;
use rutis_loader::runtime::{RuntimeResolver, RuntimeRowsPlugin};
use rutis_loader::{Chain, LoaderOptions, LoaderPlugin, ServiceCatalog};

let catalog = ServiceCatalog::new();
let runtime_package = app.join("node_modules/@arcships/rutis-runtime");
let mut chain = Chain::new();
let mut resolvers = Vec::new();
let mut fibers = Vec::new();
for command in &commands {                                // 每个第三方命令一个进程
    let runtime = LocalRuntime::node(&runtime_package, command.dir.join("package.json"))
        .named(&command.id)                               // 运行时名：两个以上的 a-z、0-9、-
        .call_limit(Duration::from_secs(30))
        .kill_on_timeout()
        .allowed_services(["notes"]);
    let rows = Arc::new(RuntimeResolver::modules(runtime.handle()).with_catalog(&catalog));
    fibers.push(root.plugin(runtime));
    chain = chain.with_shared(rows.clone());
    resolvers.push(rows);
}
let loader = LoaderPlugin::new(chain, LoaderOptions { catalog, ..LoaderOptions::default() });
root.plugin(loader).await?;
for rows in resolvers {
    root.plugin(RuntimeRowsPlugin::new(rows));
}
// 行：{ "id": "wordcount", "name": "wordcount:jot-wordcount" }
# Ok(())
# }
```

- 几个插件共用一个进程：它们的行写同一个前缀。
- `RuntimeResolver::node` 不变，仍是默认 Node 运行时（不带前缀的行）的写法；它不接受带运行时前缀的行名，不会抢走别的运行时的行。
- `modules` 每次解析都问运行时，不按包的 `version` 缓存。

### 调用时限

`call_limit(时限)` 限制进入这个运行时里插件代码的每个调用：按名字调用它的行提供的服务，和调用它交出来的函数（比如插件注册给 `jot` 的命令处理函数）。调用方是宿主、别的运行时里的插件还是 Rust 插件都一样，从发出到结束计时，函数返回的 promise 也算在内。同一运行时里插件之间的调用不经过宿主，不受限。

超过时限，调用被取消（对面收到取消，与调用方自己放弃时相同），调用方收到错误码 `timeout`、`data` 为 `{runtime, limit, service?, method?}`（`limit` 是毫秒）的错误：

```text
chat: call to weather.tomorrow did not finish within 30s (the host's limit for runtime agent); the call was cancelled
a function of wordcount did not finish within 30s (the host's limit for runtime wordcount); the call was cancelled
```

取消停不下所有插件：TypeScript 的 promise 不能取消，Python 的 `def` 方法和 JavaScript 的忙循环占住整个进程。再加上 `kill_on_timeout()`，超时后结束这个运行时的进程：

```text
a function of wordcount did not finish within 30s (the host's limit for runtime wordcount); the call was cancelled; runtime wordcount was ended
```

运行时的状态（`Loader::status()`）是 `wordcount: down (ended by the host: a function of wordcount did not finish within 30s)`，它的行等它，同一进程里其他进行中的调用报 `peer-gone`。要不要重启由你决定：对 `root.plugin(runtime)` 返回的 fiber 调用 `restart()`：

```rust,no_run
# use rutis::host::*;
# async fn restart(fiber: FiberView) -> Result<(), Box<dyn std::error::Error>> {
fiber.restart().await?;   // 新的进程，同样的选项；它的行重新装载
# Ok(())
# }
```

调用方自己限时（Rust 的 `tokio::time::timeout`、Python 的 `asyncio.wait_for` 等）照常有效，两者先到的生效。

### 能用的服务

`allowed_services([...])` 列出这个运行时的插件能用的、来自运行时之外的服务：你的服务、Rust 插件的服务、别的运行时里的行提供的服务。同一运行时里的行互相提供的服务不受限；你自己调用插件（`call_service`）也不受限。

插件的 `inject` 里有名单之外的服务时，这一行启动失败并写明原因，错误码 `unavailable`：

```text
wordcount: uses llm, which the host does not let runtime wordcount use (allowed: tools, notes)
```

插件把 loader 当作服务用时（#185），不想让哪些插件用它，就不把它写进那些运行时的名单。

### 问候时限

本机运行时的进程启动后 30 秒内没有问候就被结束。调试器停在第一行（`--inspect-brk`）时，用 `greeting(Duration::MAX)` 不限时：

```rust,no_run
# use std::time::Duration;
# use rutis_bridge::runtime::LocalRuntime;
# let app = std::path::PathBuf::from("/srv/app");
let runtime = LocalRuntime::node(app.join("node_modules/@arcships/rutis-runtime"), app.join("package.json"))
    .named("wordcount")
    .env("NODE_OPTIONS", "--inspect-brk")
    .greeting(Duration::MAX);
```

不限时，一个启动了却一直不问候的进程会一直停在启动中；`Loader::status()` 里看得到：

```text
rows:
  wordcount: waiting for runtime wordcount (starting)
runtimes:
  wordcount: starting (pid 4242)
```

### 不是限制的东西

- `ServiceCatalog::share_registered_only` 与读取拦截决定服务按什么名字共享，不是权限：插件能不能用一个服务看 `allowed_services`。
- 同一进程里的插件能用语言本身的办法互相拿到对方（全局变量、模块缓存），所以名单只管来自进程之外的服务；要隔开，放进不同的运行时。
- 插件进程有宿主用户的全部权限（文件、网络、子进程）。rutis 不做沙箱，也不限内存、CPU。

各选项的参考见 [Rust 宿主：本机运行时的选项](rust-host.md#本机运行时的选项)。
