# 做一个开放插件的宿主

[English](open-host.en.md)

读者：在自己的 Rust 产品里接入 rutis，并把插件能力开放给别的开发者。全篇的例子是命令行记事工具 `jot`：宿主提供 `notes` 服务，第三方用 TypeScript 或 Python 写命令。

本指南的其他各节还在写（#671）；API 的完整参考见 [Rust 宿主](rust-host.md)。

## 3. 管住插件

第三方插件不归你管：它可能崩溃、卡死。rutis 给你的是把插件放进各自的进程，和本机运行时的问候时限；调用限时、能用哪些服务由你自己决定（见本节末尾）。

### 每个插件一个进程

同一个运行时里的插件共用一个进程：一个插件崩溃或卡死，其他插件跟着停下。给每个第三方插件一个自己的运行时（`named`），它的行用 `RuntimeResolver::modules`，行名写 `<运行时名>:<npm 名或文件>`：

```rust,no_run
# use std::sync::Arc;
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
        .named(&command.id);                              // 运行时名：两个以上的 a-z、0-9、-
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

Windows 上进程连上宿主之后才有进程号，这一行只写 `wordcount: starting`。

### 不是限制的东西

- rutis 不给调用限时：调用方自己限时（Rust 的 `tokio::time::timeout`、Python 的 `asyncio.wait_for` 等）。插件卡在同步代码里、取消停不下时，用 `LocalRuntime::processes()` 的 `kill()` 结束它的进程，再对 `root.plugin(runtime)` 返回的 fiber 调用 `restart()`：新的进程，它的行重新装载。
- rutis 不管插件能用哪些服务：哪些服务给哪个插件是你的业务，由你自己分配。
- 服务按名字共享，这不是权限。
- 同一进程里的插件能用语言本身的办法互相拿到对方（全局变量、模块缓存）；要隔开，放进不同的运行时。
- 插件进程有宿主用户的全部权限（文件、网络、子进程）。rutis 不做沙箱，也不限内存、CPU。

各选项的参考见 [Rust 宿主：本机运行时的选项](rust-host.md#本机运行时的选项)。
