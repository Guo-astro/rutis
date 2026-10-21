# 在 Rust 应用里嵌入

应用要给插件提供自己的 Rust 服务、或者把 rutis 放进已有的 Rust 程序时，直接用 crate 组装宿主。不需要这些时用 [rutis-host](rutis-host.md) 即可，配置的行格式相同。

## 依赖

```toml
[dependencies]
rutis = "0.13"
rutis-loader = { version = "0.13", features = ["node", "python", "peer"] }
rutis-bridge = { version = "0.13", features = ["python", "websocket"] }   # node 默认开
```

| crate / feature | 内容 |
| --- | --- |
| `rutis` | 内核：插件、依赖、服务 |
| `rutis-loader` | 按数据（行）加载插件。`node` / `python` / `go`：这几种语言的行；`peer`：节点行和 `peer:` 行 |
| `rutis-bridge` | 连接其他进程、语言、机器。`node`（默认）、`python`、`go`：本机运行时；`websocket`：WebSocket 承载；`cordis`：挂载 Cordis 插件并生成 Rust 绑定（见 [Cordis](cordis.md)）；`exports`（放在 `[build-dependencies]`）：为[导出的 Rust 服务](#导出已有的-rust-服务)生成 dispatcher；`testing`：一致性测试 |

只用其中一种语言时只开那一个 feature，应用只编译、只启动它用到的部分。

## 给插件提供 Rust 服务

### 在类型上标注 `#[rutis::service]`

Rust 服务在它的 `impl` 块上加 `#[rutis::service]`，照常 `provide`，各语言的行就能按名字调用它（[#510](https://github.com/arcships/rutis/issues/510)）。不用清单、不用 `build.rs`、不用 `export_all`、不用写 `HostDispatch`；定义服务的 crate 只依赖 `rutis`。

```rust
use rutis::{service, BoxFuture, CordisError};

#[service]                         // 对外名字 "calendar"：类型名，第一个词小写
impl Calendar {
    pub fn new() -> Self { /* … */ }                          // 没有 self：不对外
    pub async fn add(&self, entry: NewEntry) -> Result<Entry, CalendarError> { /* … */ }
    pub fn get(&self, id: u64) -> Result<Entry, CalendarError> { /* … */ }
    pub async fn subscribe(&self, ctx: &rutis::Ctx, handler: impl Fn(Change) -> BoxFuture<'static, Result<(), CordisError>>) { /* … */ }
    #[service(skip)]
    pub fn compact(&mut self) { /* … */ }                     // 留在 Rust 里
}

root.provide(Calendar::new())?;    // 就这些
```

```ts
const calendar = ctx.use<Services['calendar']>('calendar')
const entry = await calendar.add({ title: 'Standup', startsAt: Date.now() })
```

- **对外的方法**：带标注的 `impl` 块里公开的 `&self` 方法；一个类型可以有几个带标注的块。对调用方都是 `async`（JavaScript 里是 Promise，Python 里是 awaitable），Node、Bun、Python、Go 的行调用的是同一个 dispatcher。方法名按 camelCase 对外（`add_entry` 是 `addEntry`）。
- **名字**：默认是类型名、第一个词小写（`Calendar` 是 `calendar`，`TreeCommands` 是 `treeCommands`，`HTTPClient` 是 `httpClient`）；`#[service(name = "cal")]` 改名。
- **提供即对外**：`ctx.provide(value)`（以及键是这个类型自己的 `provide_as`）同时按名字提供它，同一个撤销函数撤下两者，插件卸载时一起撤下；依赖它的行随之停止，与其他服务相同。带 `check()` 谓词提供（`provide_as_with_check`）时，按名字的入口也受它门控：谓词不成立期间，按名字用它的行与原生服务的使用者一样停下。`provide_mut_as` 提供的可变绑定不对外，带限定名的（`TypeKey::keyed`）也不对外：同一类型的第二个值会占用同一个名字。
- **作用域跟着提供的位置**：在实例里提供（`TypeKey::instance::<Notes>(build.instance("session")?)`），就是那个实例自己的，实例里的行拿到它；在实例外提供就是全局的。不用写实例组。实例里提供过之后，实例里的行用的是这个实例自己的，不会退回到同名的全局服务：实例里的服务撤下时，它们等它在这个实例里重新提供（设计 R3：作用域跟着提供的位置）。
- **能传什么、错误怎么抛**：与下面"第三方类型"的生成规则相同（[参考](#导出服务参考)）：serde 参数与返回值、按引用的回调、`&Ctx` 调用方行、`Err(e)` 抛出时带 `name`、`code`、`message`、`data`。
- **编译期报错**：带标注的块里有公开方法绑定不了（`&mut self`、返回借用、可变引用参数、不是函数的泛型参数等），是指到那个方法的编译错误；不想对外的，在方法上加 `#[service(skip)]`。
- **重名**：同一作用域里两个类型对外名字相同，后一个 `provide` 失败，错误写明双方；它什么都没提供，原生服务也不提供。
- **没标注的不对外**：宿主内部的服务（存储、锁、数据库句柄等）不会暴露给插件。一行等着的名字恰好是某个没标注的 Rust 服务的名字时，`Loader::waiting_for(&entry)` 列出这一行等的名字，并说明在那个类型的 `impl` 上加 `#[rutis::service]`。
- **dylib 插件**：dylib 里带标注的类型（crate 只依赖 `rutis-sdk`，写 `#[rutis_sdk::rutis::service]` 或 `use rutis_sdk::rutis::service`）加载后同样对外，宿主不需要知道它；换代时旧代的入口随旧代的服务撤下，新代提供时重新出现。宿主要链接 `rutis-bridge`。
- **TypeScript 声明**：`rutis-host types <crate 目录> -o services.d.ts` 从源码里找出这个 crate 和它用到的、依赖 rutis 的 crate 里全部带标注的类型，写出声明（`Services['calendar']`、各数据类型、`<Name>Thrown`）。想每次构建都更新，`build.rs` 里写 `rutis_bridge::build::Exports::new().typescript("rows/services.d.ts").generate()`，不用列类型。调用本身不需要声明。

完整示例：[`examples/marked-services`](../../examples/marked-services)。

### 第三方类型：在 `build.rs` 里列出

改不了源码的类型（别人 crate 里的），在 `build.rs` 里写出它的类型，构建时从 Rust 定义生成它的 dispatcher 和 TypeScript 声明（[#440](https://github.com/arcships/rutis/issues/440)）。带标注的类型不要再列：列了会构建报错。

```toml
[dependencies]
rutis-bridge = "0.13"

[build-dependencies]
rutis-bridge = { version = "0.13", features = ["exports"] }
```

```rust
// build.rs
fn main() {
    rutis_bridge::build::Exports::new()
        // 依赖 crate 里的类型，按应用自己的写法写路径。
        .service("calendar", "calendar_services::Calendar")
        // 在实例组 `session` 的每个实例里：每个实例的行拿到自己的那一份。
        .service_in("notes", "calendar_services::Notes", "session")
        // 把声明再写一份到行的 TypeScript 引用它的地方（可选）。
        .typescript("rows/services.d.ts")
        .generate()
        .expect("generate the exported services");
}
```

```rust
rutis_bridge::include_exports!();          // 生成的模块 `exports`

let mut catalog = ServiceCatalog::new();
catalog.export_all(exports::all());         // 这些名字由加载器提供
// 原生服务照常提供：在 TypeKey::of::<Calendar>() 下。
root.provide(Calendar::new())?;
// 实例的 Notes：由组内的行提供，在 TypeKey::instance::<Notes>(build.instance("session")?) 下。
```

- 原生服务在时，加载器就提供这个导出：`inject` 这个名字的行等它出现、随它撤下而停止，与其他服务相同。实例内的导出按 `register_shared_instance` 的规则解析：行拿到离它最近的那个组实例的服务。不用加载器时，自己挂上 `exports::calendar().plugin(None)`。
- 每个公开的 `&self` 方法都会导出，规则与标注的相同；生成器绑定不了的方法会被跳过，构建警告写明位置和原因。完整示例：[`examples/rust-services`](../../examples/rust-services)。

### 手写 `HostDispatch`

插件按名字使用的服务，在 Rust 里是 `host_key(名字)` 下的 `dyn HostDispatch`；上面的导出就是生成它。分发方式由服务自己决定的（例如给每一行一个自己的对象），手写：

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

要让错误像导出服务的 `Err(e)` 那样带 code 和 data，返回 `rutis_bridge::exports::failure(name, code, message, data)`：调用方语言里抛出名为 `name`、消息为 `message` 的错误，附带 `code` 和 `data`（`serde_json::Value` 或 `None`）。

插件提供的服务同样以 `host_key(名字)` 出现在 rutis 里，Rust 插件可以按名字使用它们。

### 调用方的行

替插件登记东西（工具、路由、监听）或按插件授权的服务，需要知道是哪一行在调用。`invoke` 期间 `session::caller_row()` 返回调用方的行；把登记挂在这一行的 `Ctx` 上，这一行卸载或重启时它就撤掉，运行时照常运行：

```rust
use rutis::Effect;
use rutis_bridge::session::{caller_row, native_error, HostDispatch, Reply, Value};

impl HostDispatch for Tools {
    fn invoke(&self, method: &str, args: Value) -> Reply {
        let [name]: [String; 1] = rutis_bridge::session::decode_value(args)?;
        if let Some(row) = caller_row() {
            // row.id()：loader 的行 id；row.key()：它在运行时里的 key（每次重启一个新的）；
            // row.scope()：它隔离这个服务所用的标签（在实例里是实例的），不在作用域里为 None。
            self.add(row.id(), &name);
            let (tools, id) = (self.clone(), row.id().to_owned());
            row.ctx()
                .effect(move || Effect::Disposer(Box::new(move || Ok(tools.remove(&id, &name)))))
                .map_err(native_error)?;
        }
        Ok(Value::Undefined)
    }
    // methods …
}
```

- 同一个运行时里的两行是两个调用方，`key()` 和 `instance()` 各不相同。
- 来自 Rust 的调用、来自对端节点的调用（包括 `peer:` 行，它们的调用经节点之间的服务到达）、运行时里不属于任何行的调用（`Mount::hosts` 提供的服务）、行里没有声明就使用这个服务的代码的调用（Cordis 的 `ctx.get`，找到的是所有行共享的那份注册）都是 `None`。
- 只在 `invoke` 本身里读它：`invoke` 返回的 future 之后在别处运行，读不到。
- 它来自 rutis-loader 的行租用服务的方式（`Process::lease_host_for`）：每一行把它租用的服务注册在自己的作用域里（另有与以前一样、所有行共享的那份注册），所以线上格式不变，Node、Bun、Python、Go 的运行时都可以用，0.11 的也可以。自己用 `Process` 加载行时，用 `lease_host_for` 并按 `HostLease::label()` 隔离这个名字；`lease_host` / `lease_host_in` 的调用是 `None`。

## 运行时与加载器

```rust
use std::sync::Arc;
use rutis_bridge::runtime::LocalRuntime;
use rutis_loader::{
    Chain, Layer, LoaderOptions, LoaderPlugin, Patch, RuntimeResolver, RuntimeRowsPlugin, ServiceCatalog,
};

// 服务名到键：所有名字都按名字跨语言共享，与 rutis-host 相同。只共享登记为共享的名字
// （register_shared、export）时调用 catalog.share_registered_only()。
let mut catalog = ServiceCatalog::new();

// 本机的 Node 运行时：@arcships/rutis-runtime 所在的目录，和插件解析所用的 package.json。
// 事件不用配置：运行时里的监听器自动登记到宿主的事件总线上（见下面的“事件”）。
let node = LocalRuntime::node("app/node_modules/@arcships/rutis-runtime", "app/package.json");
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
- 进程运行期间按它启动时的清单解析；换了二进制，`control.restart("go-netkit")` 后生效。
- 崩溃的运行时默认不自动重启。`control.supervise(RestartPolicy::OnFailure, |end| …)` 之后，进程自己结束的运行时等一会儿后重新启动（1 秒起每次翻倍、最多 30 秒，运行满 60 秒后从 1 秒重来，5 分钟内重启 5 次后再结束就放弃），每个运行时各算各的；`RestartPolicy::Exit` 只把结束报告给回调（`RuntimeEnd::Exit`），由应用决定停下。`end.line()` 是 rutis-host 打印的那一行；`control.unsupervise()` 停止监督，正在等待的重启不再发生。握手被拒（旧 SDK）的二进制不算自己结束，不重启。
- Go 之外的编译型语言（之后的 Swift）用同一套实现：`BinaryResolver<L>`、`BinaryRuntimes<L>`，`L` 实现 `BinaryLanguage`（行名前缀、SDK 标记串、启动方式）；`GoResolver` 就是 `BinaryResolver<Go>`。

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

## 事件

宿主里的 Rust 插件、每个运行时里的插件（任何语言）和相连节点上的监听器在一个逻辑总线上（[插件 API：事件](plugin-api.md#事件)）。运行时的监听器自动登记，宿主一侧什么都不用配置：`ctx.events()` 上发出的事件到达所有语言的监听器，它们发出的事件也到达这里的 Rust 监听器。

事件类型以 `Event::NAME` 作为跨语言的名字，参数按 serde 编解码，所以它和它的 `Value` 都派生 `Serialize` / `Deserialize`。两个及以上字段的元组结构体对应同样多个参数，所以下面的 `Created` 就是别的语言里的 `emit("session/created", id, size)`：

```rust
use std::sync::Arc;
use rutis::{BoxFuture, Call, CordisError, Ctx, Event, EventKey, Listener, Mode, SyncCall};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
struct Created(String, u32);
impl Event for Created {
    const NAME: &'static str = "session/created";
    type Value = String;
}

struct Audit;
impl Listener<Created> for Audit {
    fn call<'a>(&'a self, _ctx: &'a Ctx, event: &'a Created, call: Call<'a, String>)
        -> BoxFuture<'a, Result<Option<String>, CordisError>> {
        Box::pin(async move {
            if call.mode() == Mode::Waterfall {
                // 中间件：执行链上其余部分，再改写它的结果
                let inner = call.next().await?;
                return Ok(inner.map(|text| format!("[{text}]")));
            }
            Ok(Some(format!("audited {}", event.0)))      // Ok(None) 是没有结果
        })
    }
}

fn terminal<'a>(_: &'a Ctx, e: &'a Created) -> BoxFuture<'a, Result<Option<String>, CordisError>> {
    Box::pin(async move { Ok(Some(format!("saved {}", e.0))) })
}

let key = EventKey::<Created>::of();
let events = ctx.events();
let off = events.on(&ctx, &key, Audit)?;                                   // 监听器以这个 ctx 被调用
events.on_sync(&ctx, &key, |_: &Ctx, e: &Created, _: SyncCall<'_, String>| Ok(Some(format!("sync {}", e.1))))?;

events.emit(&ctx, &key, Arc::new(Created("s1".into(), 3)))?;              // 立即返回
events.parallel(&ctx, &key, Arc::new(Created("s1".into(), 3))).await?;    // 等全部；ListenersFailed
let first: Option<String> = events.serial(&ctx, &key, Arc::new(Created("s1".into(), 3))).await?;
let out = events.waterfall(&ctx, &key, &Created("s1".into(), 3), terminal).await?;   // `()`：没有终态
let sync = events.bail_sync(&ctx, &key, &Created("s1".into(), 3));        // 遇到 Audit（异步）：AsyncListener
let python_only = events
    .filter(|l| l.runtime.as_deref() == Some("py"))                       // ListenerInfo
    .serial(&ctx, &key, Arc::new(Created("s1".into(), 3)))
    .await?;
off.dispose().await?;
```

| 事项 | 规则 |
| --- | --- |
| 监听器 | 一个监听器登记一次，五种方式都能调用它；`call.mode()` 是这次的方式，`call.next()` 只在 `waterfall` 里可用（否则 `WaterfallNext`）。`on_sync` 登记同步监听器，它能被所有方式调用，但在异步 `waterfall` 里不能调 `next`；`on` 的（异步）监听器不能被 `bail_sync` / `waterfall_sync` 调用（`AsyncListener`） |
| 选项与前缀 | `on_opt` / `on_sync_opt` 取 `EventOptions`，用构建方法写：`EventOptions::default().prepend(true)`（还有 `.once(true)`、`.global(true)`）；`on_pattern(&ctx, &EventPattern::prefix("session/"), ..)` 收这个前缀下的所有名字 |
| 按名字 | 不定义类型时用 `rutis_bridge::events::Args`：`EventKey::<Args>::dynamic("file-changed")`，参数是 `Args(vec![Value::Data(json!(…))])`，结果是 `ArgValue`。同名的类型化监听器与 `Args` 监听器互相收到对方的事件，经 serde 转换 |
| 作用域 | 发出方的作用域是 `ctx.scope()`：加载器为每个组实例调用 `Ctx::mark_scope`，监听器收到自己作用域及其内的事件。没有向别的作用域发出的 API，按 `ListenerInfo::scope` 过滤 |
| 失败 | `emit` 的监听器失败以 `CordisError::ListenerFailed` 进入错误汇集（`Ctx::root_with_sink`），`rutis_bridge::events::report_line` 给出 rutis-host 打印的那一行；`parallel` 是 `CordisError::ListenersFailed`（按登记顺序）；别处监听器的错误带着名字与信息到达 |
| 卸载与取消 | 插件卸载时它的监听器撤下，正在运行的调用看到 `ctx.cancelled()`，卸载等它们结束（最多到关闭截止时间）。丢弃 `parallel` / `serial` / `waterfall` 的 future 即取消这次分发 |
| 框架事件 | `rutis_loader::RowEvent`（`rutis/row`）、`rutis_bridge::events::{ServiceEvent, RuntimeEvent}`（`rutis/service`、`rutis/runtime`）：用 `on` 监听；发出 `rutis/*` 以 `ReservedEvent` 失败 |
| 节点 | `PeerPlugin` 的 `Features { export, import }` 同时选择事件（名字或 `*` 前缀），见[节点](#节点)与[连接节点](nodes.md#节点之间的事件) |

测试不需要单独的工具：在 `#[tokio::test]` 里用 `Ctx::root()`（或 `Ctx::root_with_sink` 收集 `emit` 的失败），加载插件，再按上面的方式发出、断言。运行时里的监听器要用真实的运行时（`LocalRuntime`），与生产相同。

## 导出服务：参考

Rust 服务由各语言的行（TypeScript、JavaScript、Python、Go）按名字使用。带 `#[rutis::service]` 的类型，dispatcher 由宏在编译期生成，提供即对外（[上文](#在类型上标注-rutisservice)）；改不了源码的类型，`build.rs` 用 `rutis_bridge::build::Exports` 列出（构建依赖开 `exports` feature），应用用 `rutis_bridge::include_exports!()` 声明生成的模块，再用 `ServiceCatalog::export_all` 登记 `exports::all()`（[上文](#第三方类型在-buildrs-里列出)）。两者能传什么、错误怎么抛都按下面的规则；TypeScript 声明都由生成器从源码写出。完整示例：[`examples/marked-services`](../../examples/marked-services)、[`examples/rust-services`](../../examples/rust-services)。

| `Exports` | 作用 |
| --- | --- |
| `.service(name, path)` | 把 `path` 处的 struct 或 enum 导出为服务 `name`，在实例之外 |
| `.service_in(name, path, group)` | 导出在实例组 `group` 的每个实例里（同 `register_shared_instance`） |
| `.declared(path, typescript)` | 生成器推不出时，`path` 处 Rust 类型对应的 TypeScript 类型 |
| `.typescript(file)` | 声明再写一份到 `file`（相对 `Cargo.toml`），内容变了才写 |
| `.generate()` | 写出 `OUT_DIR/rutis_exports.rs` 和 `rutis_exports.d.ts`；`exports::TYPESCRIPT` 是声明的内容 |

### 源码从哪来

`path` 按应用自己的写法写：自己的类型写 `crate::…`，依赖的类型写依赖的 crate 名（按 `Cargo.toml` 里的名字，含改名）。构建脚本运行 `cargo metadata`（只取正在构建的目标平台，先离线），得到每个 crate 的入口文件和 Cargo 解析出的 feature，再用 syn 读模块树：`mod` 文件（含 `#[path]`）、`use`、glob 和 `pub use` 再导出（从私有模块再导出的类型也能找到）、类型别名（含泛型别名）。方法是该类型在其 crate 内任意位置的固有 `impl` 块里的公开 `&self` 方法。`#[cfg]` 会求值：`feature = "…"` 按 Cargo 解析出的 feature，目标平台的 `cfg` 按 `CARGO_CFG_*`，`test` 和 `doc` 视为关闭。只用稳定版 Rust：不为此编译或展开任何东西。

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

参数用 serde 解码（任意 `Deserialize`），结果用 serde 编码（任意 `Serialize`）；Cargo 按真实签名检查每个生成的调用。

**按引用传的函数：** Rust 一侧拿到的函数会调用调用方的函数：它的 future 在函数返回、且返回的 Promise settle 之后才完成，所以订阅者处理完事件，原生一侧才继续；抛错或 reject 是这个 future 的 `Err`（要求 `E: From<rutis_bridge::session::Error>`，`CordisError`、`anyhow::Error`、装箱的错误都满足），走原生的失败路径。Rust 期望 `()` 时，函数返回的值被忽略。

**取消：** 调用方不再等待的调用（`AbortSignal`、丢弃 future）会在 Rust 一侧丢弃方法的 future，与所有调用相同。

**调用方行的上下文：** `&rutis::Ctx`（或 `Ctx`）参数在 TypeScript 里不是参数：它是发起调用的那一行的上下文（`rutis_bridge::session::caller_row()`），方法在上面登记的东西（`ctx.effect`、监听）属于那一行，随该行卸载或重启而移除，例如订阅。不在任何行里调用时（从 Rust、从对端节点），这样的方法失败，错误写明它需要调用方行的上下文。

### 错误

返回 `Result<T, E>` 的方法（`std::result::Result`、它的别名、`anyhow::Result`、`io::Result`）把 `Err(e)` 作为调用方语言里抛出的错误：JavaScript 的 `Error`、Python 的 `rutis.RemoteError`、Go 的 `*rutis.RemoteError`。它不是未分类的调用错误：

| `E` | `name` | `code` | `data` |
| --- | --- | --- | --- |
| 有 serde 的枚举 | 类型名 | serde 写出的变体：序列化成的字符串、外部标签变体的唯一键、内部或相邻标签变体的 `tag` 字段 | serde 值 |
| 没有 serde（或 `untagged`）的枚举 | 类型名 | 变体的 Rust 名（取自 derive 的 `Debug`） | 有 serde 时为 serde 值 |
| 有 serde 的结构体 | 类型名 | 序列化成的字符串，否则为类型名 | serde 值 |
| `String` | `Error` | `Error`（字符串是 `message`） | — |
| `rutis::CordisError` | `CordisError` | 变体名 | — |
| `rutis_bridge::session::Error` | 原样抛出（转发的调用保留远端的错误） | | |
| 其他（`anyhow::Error`、`io::Error`） | 类型名 | 类型名 | — |

`message` 总是 `e` 的 `Display`。JavaScript 读 `error.code`、`error.data`；Python 读 `error.code`、`error.data`；Go 读 `remote.Code`、`remote.Data`（大整数为 `*big.Int`）；Rust 用 `Error::code()`、`Error::data()`。它们放在错误图里传，不认识它们的运行时仍拿到名字和消息。声明里每个错误类型有一个 `<Name>Thrown` 类型 `RustError<'Name', 'Code1' | 'Code2', Data>`，写在方法的 JSDoc 里（`@throws {CalendarErrorThrown}`）。

### 声明

`rutis_exports.d.ts`（以及 `rutis-host types` 写出的文件）包含每个服务类型的接口、它用到的数据类型、`RustError`、各 `<Name>Thrown` 类型，以及按名字列出服务的 `Services`：`ctx.use<Services['calendar']>('calendar')`。它能通过 `tsc --strict`（`examples/rust-services` 会检查）。`.typescript(…)` 写出的那份要提交：Rust 一侧变了，构建会改写它。

### 限制

- 宏写出的东西看不到：宏（derive 除外）写出的类型、`impl` 块或方法找不到。服务类型必须是具体的 struct 或 enum（不能是泛型，不能是 trait 对象）。
- 取 `&mut self`、返回借用、函数参数取引用（`Fn(&A)`）、除函数以外的泛型参数、除 `Fn` 以外的 `impl Trait`、不返回 `Result` 的 future 的函数参数，这样的方法会被跳过并给出警告。
- 数据类型需要生成器能读懂的 serde derive：手写的 `Serialize`、`#[serde(with / serialize_with / into / remote)]`，或者不读源码的 crate 的类型（`std`、`serde_json`、`chrono`、`uuid`、`url` 和带标签的值除外）需要 `.declared(path, typescript)`；在那之前用到它的方法被跳过，警告里写明类型。
- `#[cfg]` 按上面的规则求值；构建脚本看不到的 `cfg` 视为关闭。
- 方法的 future 必须是 `Send`（否则编译失败）。

带 `#[rutis::service]` 的类型另外：

- 宏只看到 `impl` 块本身，类型上的判断（是不是 `Result`、错误怎么取 `code`、参数是不是函数）交给展开后的具体代码，所以类型别名照样识别。异步方法写成 `async fn`，或返回 `BoxFuture<'_, T>`、`Pin<Box<dyn Future<Output = T> + Send>>`、`impl Future<Output = T>`；返回 future 的别名是编译错误。`&Ctx` 参数按名字 `Ctx` 识别。
- 错误的 `code` 在抛出时由值得出，规则同上表；内部或相邻标签枚举的变体用 `#[serde(rename = "...")]` 改成与 Rust 名无关的名字时，`code` 是 Rust 名。
- 宏生成的代码只依赖 `rutis`（dylib 插件写 `#[rutis_sdk::rutis::service]` 或 `use rutis_sdk::rutis::service`，生成的代码就经 `rutis_sdk::rutis`：按标注是从哪条路径拿到的来定，所以对着 sdk-bundle 构建、没写依赖的插件也一样；别的路径写 `#[service(crate = "my::rutis")]`）；值经宿主链接的 `rutis-bridge` 编解码，比 `build.rs` 生成的多一层动态分发，相对跨进程的开销可以忽略。
