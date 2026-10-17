# 在 Rust 应用里嵌入

应用要给插件提供自己的 Rust 服务、或者把 rutis 放进已有的 Rust 程序时，直接用 crate 组装宿主。不需要这些时用 [rutis-host](rutis-host.md) 即可，配置的行格式相同。

## 依赖

```toml
[dependencies]
rutis = "0.11"
rutis-loader = { version = "0.11", features = ["node", "python", "peer"] }
rutis-bridge = { version = "0.11", features = ["python", "websocket"] }   # node 默认开
```

| crate / feature | 内容 |
| --- | --- |
| `rutis` | 内核：插件、依赖、服务 |
| `rutis-loader` | 按数据（行）加载插件。`node` / `python` / `go`：这几种语言的行；`peer`：节点行和 `peer:` 行 |
| `rutis-bridge` | 连接其他进程、语言、机器。`node`（默认）、`python`、`go`：本机运行时；`websocket`：WebSocket 承载；`cordis`：挂载 Cordis 插件并生成 Rust 绑定（见 [Cordis](cordis.md)）；`testing`：一致性测试 |

只用其中一种语言时只开那一个 feature，应用只编译、只启动它用到的部分。

## 给插件提供 Rust 服务

插件按名字使用的服务，在 Rust 里是 `host_key(名字)` 下的 `dyn HostDispatch`：

```rust
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

root.provide_as::<dyn HostDispatch>(host_key("clock"), Arc::new(Clock))?;
```

插件提供的服务同样以 `host_key(名字)` 出现在 rutis 里，Rust 插件可以按名字使用它们。

宿主事件的 `emits` 声明通过下述 Rust API 配置；`rutis-host` 的 JSON 配置暂未提供这个入口。同进程插件的本地事件不需要该声明。

## 运行时与加载器

```rust
use std::sync::Arc;
use rutis_bridge::runtime::LocalRuntime;
use rutis_loader::{
    Chain, Layer, LoaderOptions, LoaderPlugin, Patch, RuntimeResolver, RuntimeRowsPlugin, ServiceCatalog,
};

// 服务名到键：所有名字都按名字跨语言共享（rutis-host 的做法）；
// 也可以只登记需要的：catalog.register_shared("clock")。
let mut catalog = ServiceCatalog::new();
catalog.share_by_name();

// 本机的 Node 运行时：@arcships/rutis-runtime 所在的目录，和插件解析所用的 package.json。
// `emits` 声明宿主会向该运行时发哪些事件（`Process::emit`）：运行时拒绝未声明的事件，
// 插件用 `ctx.on`（同名事件总线）接收。在运行时挂载前设置。只有报告 `events` 能力的运行时
// （Node、Bun、Python、Go）接受 `emits`，其他运行时在挂载时失败；`internal/*` 不能声明。
let node = LocalRuntime::node("app/node_modules/@arcships/rutis-runtime", "app/package.json")
    .emits(["file-changed", "active-leaf-changed"]);
let node_rows = Arc::new(RuntimeResolver::node(node.handle()).with_catalog(&catalog));
// 本机的 Python 运行时：插件模块所在的目录，装着 rutis 的解释器。
let python = LocalRuntime::python("app/plugins").interpreter("app/.venv/bin/python");
let python_rows = Arc::new(RuntimeResolver::modules(python.handle()).with_catalog(&catalog));
root.plugin(node);
root.plugin(python);

let loader = LoaderPlugin::new(
    Chain::new().with_shared(python_rows.clone()).with_shared(node_rows.clone()),
    LoaderOptions { catalog, ..LoaderOptions::default() },
);
let handle = loader.handle();
root.plugin(loader).await?;
root.plugin(RuntimeRowsPlugin::new(node_rows));
root.plugin(RuntimeRowsPlugin::new(python_rows));

let rows: Vec<Patch> = serde_json::from_value(json!([{ "insert": [
    { "id": "weather", "name": "weather-plugin", "config": { "city": "Oslo" } },
    { "id": "llm", "name": "py:llm_gateway" }
] }]))?;
handle.reconcile(vec![Layer::new("app", rows)], None).await?;
```

- 运行时是一个插件，它的进程随插件启动、随插件卸载而结束。进程意外结束时，运行时停下，它的行等待；`RuntimeHandle::state()` 是 `Down(原因)`，原因说明进程怎样结束。是否重启由应用决定：对运行时的 fiber 调用 `restart`。
- 行的完整格式（分组、`isolate`、`inject`、表达式、命令式修改与持久化）见 [rutis-loader 设计](../design-rutis-loader-2026-10-02.md)。
- 改了插件代码：`RuntimeResolver::invalidate_all()` 后 `Loader::reload(行)`，新代码生效（Node 与 Python 都只重新导入插件的入口模块）。
- 设置 `RUTIS_TRACE` 时，运行时通道上的每条消息在 stderr 记一行（方向和长度，不含内容）。
- 运行时进程默认没有标准输入，标准输出和标准错误用应用自己的。读终端的插件（终端界面、Python 的 `input()`）需要终端本身：它会检查输入是不是终端，并自己设置终端模式。用 `stdin(Stdio::Inherit)` 把本进程的输入交给它的运行时。这一设置作用于整个运行时进程，进程里的每个插件都能读到输入，所以把这类插件放进单独的运行时（`named`），并且终端最多交给一个运行时：多个进程读同一个终端，会争抢每一次按键。`stdout(Stdio::Null)` 和 `stderr(Stdio::Null)` 让其他运行时不往终端上写。`Launcher` 上有同样的设置。

```rust
use rutis_bridge::runtime::Stdio;

// 终端界面放在单独的运行时里，读键盘；插件所在的运行时不往终端写标准输出。
let tui = LocalRuntime::node("app/node_modules/@arcships/rutis-runtime", "app/package.json")
    .named("tui")
    .stdin(Stdio::Inherit);
let node = LocalRuntime::node("app/node_modules/@arcships/rutis-runtime", "app/package.json")
    .stdout(Stdio::Null);
```

### Go 运行时

Go 插件是编译好的二进制，一个二进制一个运行时（features：`rutis-loader` 的 `go`）。`GoResolver` 读二进制的清单（`<二进制> --rutis-manifest`），把 `go:<插件>` 路由到含有它的二进制；`GoRuntimes` 在有行用到时启动它，空闲后停下：

```rust
use rutis_loader::{GoBinaries, GoResolver, GoRuntimes};

let go = Arc::new(
    GoResolver::new(GoBinaries::new().dir("app/plugins/go").file("app/bin/netkit"))
        .with_catalog(&catalog)
        .reserve(["node", "py"]),              // 其他运行时的名字，Go 运行时不能用
);
// 放进加载器的 Chain：Chain::new().with_shared(go.clone())…，加载器挂载之后：
let go_runtimes = GoRuntimes::new(go, "app").idle(Some(Duration::from_secs(60)));
let control = go_runtimes.handle();            // restart("go-netkit")、runtimes()
root.plugin(go_runtimes).await?;
```

- 运行时名由文件名得出（`netkit` → `go-netkit`），两个二进制有同名插件时行写 `go-netkit:<插件>`。
- 进程运行期间按它启动时的清单解析；换了二进制，`control.restart("go-netkit")` 后生效。崩溃的运行时不自动重启。
- 只运行一个固定的二进制时，也可以像 Python 一样：`LocalRuntime::go(binary, project)` 加 `RuntimeResolver::modules`（行名 `<运行时名>:<插件>`）。

## 节点

```rust
use rutis_bridge::transport::websocket::{Config, WebSocketPlugin};
use rutis_bridge::{Credential, Features, IdentityPlugin, LinkConfig, PeerId, PeerPlugin, StaticIdentity};

let (main, office) = (PeerId::new("main")?, PeerId::new("office")?);
root.plugin(WebSocketPlugin::new(Config::new())?).await?;
root.plugin(IdentityPlugin::new(
    "main",
    StaticIdentity::new(main).present(office.clone(), Credential::Bearer(token)),
));
root.plugin(PeerPlugin::new(
    LinkConfig::dial(office, "websocket", "main", "wss://office.example.com/rutis"),
    Features { export: vec!["clock".into()], import: vec!["calendar".into()], ..Features::default() },
)?);
```

用 rutis-loader 配置时，`rutis_loader::register_peer_node` 注册 `rutis-bridge/peer` 行（与 rutis.json 相同），`PeerResolver` 解析 `peer:` 行。远程运行时：`RuntimePlugin::remote(名字)` 加一个 `runtime` 设为同名的节点行，行名由 `RuntimeResolver::modules(handle)` 解析为 `<名字>:<模块>`。概念与安全见 [连接节点](nodes.md)。
