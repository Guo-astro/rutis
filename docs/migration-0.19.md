# 迁移到 0.19

[English](migration-0.19.en.md)

0.19 面向宿主持有者。不兼容的有四处：远程运行时的连接（下面第一节）、宿主与运行时要一起升级（会话协议 4）、宿主一侧的 token，与平台清理（最后一节：删掉的写法直接删，不标弃用，不做转换或提示）；其余是默认关闭的新选项，各节随 0.19 的改动补齐。

## 远程运行时（#696、#698）

设计：[远程运行时：一条配置写全，一台与多台行为一致](design-remote-runtime-consistency-2026-10-26.md)。

### 新写法：`runtimes.remote` 一条写全

以前一台远程运行时要写两处：`runtimes.remote` 一条，再加一条带 `runtime` 的 `rutis-bridge/peer` 行，两处之间不核对，少写一处行就一直等待。0.19 起一条写全：

```json
// 0.18
"runtimes": { "remote": [{ "name": "gpu", "language": "python" }] },
"rows": [
  { "id": "gpu", "name": "rutis-bridge/peer", "config": { "peer": "gpu", "dial": "wss://gpu.example.com:7443/rutis", "runtime": "gpu" } },
  { "id": "embedder", "name": "gpu:embedder" }
]

// 0.19
"runtimes": { "remote": [{ "name": "gpu", "language": "python", "dial": "wss://gpu.example.com:7443/rutis" }] },
"rows": [
  { "id": "embedder", "name": "gpu:embedder" }
]
```

| 0.18 的节点行 | 0.19 的 `runtimes.remote` |
| --- | --- |
| `"dial": "<地址>"` | `"dial": "<地址>"` |
| `"listen": "<监听器>"` | `"listen": "<监听器>"`（不带 `instances`：一台，对方的 `--id` 是运行时名） |

- 多台（`listen` + `instances`）的写法不变。
- **旧写法照常可用，不用改，也不会被转换。** 同一个运行时不要两种都写：`runtimes.remote` 有 `dial` 或 `listen`，又有一条 `"runtime"` 指向它的节点行时，宿主启动报错：

  ```
  remote runtime gpu: link row gpu connects it too: remove the link row, as runtimes.remote connects it itself
  ```

- 旧写法里节点行的 `export`、`import`、`host`、`rows` 等字段，新写法没有：连接只用来运行这个运行时的行。还要这些字段的，继续用节点行。
- token 是 `RUTIS_TOKEN_<名字>`（见[宿主一侧的 token](#宿主一侧的-token695)）。新写法的一台 `listen` 没有 token 时启动报错。

### 行为变化

1. **一台远程运行时缺插件，只是这一行失败。** 远端没有某一行的插件时，以前这一行解析失败（`unresolved: no plugin named "gpu:embedder"`），编辑时新增这样的行被拒绝；现在和多台一样：只有这一行失败，其他行照常，新增这样的行不被拒绝，运行时下次连上时重新检查。`run` 打印的变成：

   ```
   embedder: unresolved: entry "embedder" rejected: runtime gpu: this connection does not contain plugin embedder
   ```

   用 Rust 嵌入、按 `LoaderError::NotFound` 判断这种情况的代码，改为 `rutis_loader::report::Unavailable::is(&error)`。

2. **多台缺插件的文字多了运行时名。** `this connection does not contain plugin X` 改为 `runtime <名>: this connection does not contain plugin X`（只按后半句匹配的不受影响）。

3. **只写 `listen`、不写 `instances` 不再报错。** 以前启动报 `listen needs instances`，现在是一台运行时连进来。

4. **运行时缺行约定时的文字统一。** 一台和多台都写 `the <名> runtime lacks rows.v2 and hosts: a runtime speaking the row contract is required (for Node, @arcships/rutis-runtime 0.3.0 or later)`。

5. **新的输出。** 新写法的连接状态行以 `runtime <名>` 开头（旧写法的节点行仍用行 id）；多台的连接开始打印上线、断开、被拒：

   ```
   runtime gpu: link connected
   runtime gpu: link down (gpu.example.com:7443: Connection refused); retrying in 2.1s
   runtime worker: connected worker-1
   runtime worker: disconnected worker-1 (connection lost)
   runtime worker: refused (the worker runtime lacks rows.v2 and hosts: …)
   ```

   `rutis-host status` 在有这类运行时时多出 `connections:` 一节。读 `run` 输出的脚本按行首前缀区分，不受新行影响。

### Rust 嵌入

- `rutis_host::control::serve` 在 `Loader` 之后多了一个参数 `Report`（一个返回状态文字的函数，`status` 用它回答）；用 `Host::status_report()` 得到它。
- rutis、rutis-loader 的 Rust API 不变。rutis-bridge 的连接与会话部分改了（#695，[远程连接：只留一个会话插件](design-remote-session-2026-10-28.md) §三、§十一）：

  | 0.18 | 0.19 |
  | --- | --- |
  | `rutis_bridge::transport::{local, memory, websocket}` | `rutis_bridge::{pipe, memory, websocket}` |
  | `Transport`、`Dial`、`transport_key`；`LocalPlugin`、`MemoryPlugin`、`WebSocketPlugin` 与它们的 `*Transport`；`websocket::Config` | `websocket::dial`、`websocket::listen`、`pipe::spawn`、`memory::pair` 直接给出 `Channel` |
  | `ListenerConfig::new(名字, 地址, 本端 id)` | `ListenerConfig::new(名字, 地址)` |
  | `Closer::replaced()` / `rejected(r)` / `close(&str)` | `close(Close::Replaced)` / `close(Close::Rejected(r))` / `close(Close::Done)` |
  | `Channel` 的 `info`（`ChannelInfo`）、`Spawn::new(程序, peer)` | 删；对方 id 只来自会话握手；`Spawn::new(程序)` |
  | `rutis_host::host::Embedding` 的 `transports` | 删；自己的连接方式写成连接插件（远程连接的重做落地后） |
  | `Connection::open(channel, dispatch)`、`Connection::open_with(channel, dispatch, Format::Endpoint(e))`、`Format` | `Connection::open(channel, dispatch, e)`：只有 endpoint 格式，本端总要报自己的 id |
  | `Process::attach(channel, mount, Format::Endpoint(e))` | `Process::attach(channel, mount, e)` |
  | `session::PROTOCOL`（2）、`cordis::PROTOCOL`；`session::ENDPOINT_PROTOCOL` 为 3 | 删；`session::ENDPOINT_PROTOCOL` 与 `cordis::ENDPOINT_PROTOCOL` 为 4 |
  | `LinkConfig` 的 `compat` | 删 |

- `rutis-bridge/peer` 行的 `transport` 字段与 `runtimes.remote` 的 `transport` 字段删除：写了它们的 rutis.json 在启动时报不认识的字段。

## 一起升级（#695）

0.19 在握手时拒绝 0.18 及更早的宿主、运行时、节点与 SDK：会话协议从版本 3 升到 4，只留 endpoint 格式（[远程连接：只留一个会话插件](design-remote-session-2026-10-28.md) §三）。拒绝的消息说明升级哪一端：

- 0.19 的宿主起 0.18 的本机运行时（它说本机的协议 2），宿主写出：

  ```
  incompatible session: the far end speaks protocol 2, this side 4: upgrade the far end
  ```

- 0.18 的节点或远程运行时拨号 0.19 的监听方，拨号方写出：

  ```
  this endpoint speaks rutis.4, the dialer offered rutis.3: upgrade the dialer
  ```

- 0.19 拨号 0.18 的监听方，被拒的文字以监听方说的为准，例如 0.18 的远程 Python 运行时：

  ```
  this endpoint speaks rutis.3, the dialer offered rutis.4: upgrade this endpoint (PyPI rutis)
  ```

- 0.18 的宿主起 0.19 的本机运行时（不传 `--id`），运行时在握手之前停下：

  ```
  a local channel needs --id <endpoint>: fd:3; a host older than rutis 0.19 started this runtime: upgrade the host
  ```

宿主与各语言运行时、SDK（Node、Bun、Python、Go、Swift、Kotlin）要一起升到 0.19；用旧 SDK 构建的 Go、Swift 二进制要用 0.19 的 SDK 重新构建。跨版本相遇只在不含不兼容改动的相邻版本之间成立。

自己启动运行时进程、或自己实现运行时的：

- 本机通道（`fd:3`、socket 路径、`tcp:`）也要带 `--id <名字>`：`<通道> --id <名字> [--peer <id>] <项目>`。宿主起本机运行时时传 `--id <运行时名>`，运行时在握手里报这个名字。不带 `--id` 时运行时拒绝启动：`a local channel needs --id <endpoint>: <channel>; a host older than rutis 0.19 started this runtime: upgrade the host`。
- Node、Bun 运行时的 `--format` 参数删除；`package.json` 的 `rutisProtocol` 为 4。
- WebSocket 子协议为 `rutis.4`。
- Go：`advanced.ServeConn(conn, defs...)` 改为 `advanced.ServeConn(conn, id, defs...)`，`id` 即握手里报的名字，同 `--id`。

## 宿主一侧的 token（#695）

**token**：宿主一侧只认 `RUTIS_TOKEN_<名字>`；只设了 `RUTIS_TOKEN` 的宿主改为按名字设。运行时一侧的 `RUTIS_TOKEN` 不变。

`<名字>` 是节点行的 `peer` 或远程运行时的 `name`，大写，`-` 写作 `_`：

```bash
# 0.18：一个 RUTIS_TOKEN 给所有对端
RUTIS_TOKEN=secret rutis-host run rutis.json
# 0.19：每个对端、每个远程运行时一个
RUTIS_TOKEN_OFFICE=secret RUTIS_TOKEN_GPU=secret rutis-host run rutis.json
```

没改时看到的：

- 连进来的远程运行时（`listen`）没有 token，宿主启动报错：

  ```
  rutis-host: remote runtime phone: set RUTIS_TOKEN_PHONE: the token its runtimes present when they dial in
  ```

- 拨出的节点行不出示 token，对端拒绝，`run` 每 30 秒重试一次：`office: link refused: auth-rejected (…); retrying in 30s`。监听的节点行没有 token 时，对端连进来同样被拒。

同一个监听器上两个条目的 `RUTIS_TOKEN_<名字>` 设成同一个值时仍在启动前报错（#702），文字改为写明两个变量：

```
rutis-host: listener public: link row main accepts RUTIS_TOKEN_MAIN and remote runtime phone accepts RUTIS_TOKEN_PHONE, the same token, so a connection presenting it cannot be told apart: give RUTIS_TOKEN_MAIN or RUTIS_TOKEN_PHONE another value
```

## SDK 的默认入口（#692）

0.19 起，各语言 SDK 的根上只留写插件常用的名字，其余放进 advanced：TypeScript / Bun `@arcships/rutis/advanced`，Python `rutis.advanced`，Go `github.com/arcships/rutis/go/rutis/advanced`，Swift `Rutis.Advanced`，Kotlin `arcships.rutis.advanced`。只有 Swift 宏与 Kotlin KSP 生成的代码用的名字放进 `Rutis._Generated` 与 `arcships.rutis.generated`。

**直接移动，无旧名**。旧位置在 0.19 删除，用到旧名字时编译或导入报找不到这个名字，按下表改导入。Python 插件和运行时共用宿主环境里的同一个 `rutis`，要与宿主一起升级到 0.19。

要改的只有导入：根上移走的名字（值类型、`PLUGIN_API`、事件的描述类型、只在某种发出方式里出现的错误等）同名从 advanced 导入；完整列表见 [SDK 的默认入口](design-sdk-entry-convergence-2026-10-27.md) §三。在那之后加进 SDK 的几个名字按同样的规则放：Python 的 `LoadError` 在 `rutis.advanced`；Kotlin 的 `RemoteObject` 在 `arcships.rutis.advanced`，`ObjectProxy` 在 `arcships.rutis.generated`；Swift 的 `Rutis.located` 在 `Rutis._Generated`。

| 语言 | 旧写法 | 改成 |
| --- | --- | --- |
| TypeScript / Bun | `import { datetime } from '@arcships/rutis'` | `import { datetime } from '@arcships/rutis/advanced'` |
| Python | `from rutis import AggregateError` | `from rutis.advanced import AggregateError` |
| Go | `rutis.DateOf(t)` | 导入 `github.com/arcships/rutis/go/rutis/advanced`，写 `advanced.DateOf(t)` |
| Swift | `let d: Rutis.DateTime` | `let d: Rutis.Advanced.DateTime` |
| Kotlin | `RutisFormat.encode(…)` | `import arcships.rutis.advanced.RutisFormat` |

Kotlin 另有两点：

- `rutis-ksp` 与 SDK 用同一个版本：0.19 之前的 `rutis-ksp` 生成的代码引用根上的旧位置，在 0.19 的 SDK 上不能编译。
- 这些类换了包。单独发布、预先编译好的 Kotlin 插件库要用 0.19 的 SDK 重新编译后，才能和 0.19 的 SDK 放进同一个应用。

不变的：每种语言根上的 `definePlugin` / `define_plugin` / `Define` / `Rutis.define`，`ctx` 上的所有方法，测试工具，Rust。Swift 宏与 Kotlin KSP 生成的代码随 SDK 一起更新，不用改。插件 API 版本不变，用旧 SDK 构建的插件在 0.19 的运行时上照常运行。

## 服务名 `loader`（#185）

用 `LoaderPlugin` 的宿主（包括 rutis-host）现在提供一个名叫 `loader` 的服务（[用 `loader` 管理插件](guide/plugin-api.md#用-loader-管理插件)）；自己也提供名叫 `loader` 的服务的行，现在以已有的同名错误（`service loader is already provided by …: row <行> cannot provide it too`）失败，给它的服务改名。

## 平台清理（#694）

0.19 删去不属于平台的东西（[平台清理](design-platform-cleanup-2026-10-28.md)）。不做兼容：删掉的 Rust API 是编译错误。

| 0.18 | 0.19 |
| --- | --- |
| `LocalRuntime::call_limit`、`kill_on_timeout`、`allowed_services`，错误码 `timeout` | 调用方自己限时（Rust 的 `tokio::time::timeout`、Python 的 `asyncio.wait_for` 等）；超时后插件仍卡住时，宿主用 `processes().kill()` 结束它的进程，再对 `root.plugin(runtime)` 返回的 fiber 调用 `FiberView::restart()`；能用哪些服务由宿主自己决定 |
| `Loader::set_overlay`、`overlays()`、`LoaderChanged::Overlay` | 删；要覆盖一行就在自己的层里写（`reconcile` 的层列表） |
| 线上错误名 `CordisError`、生成类型 `CordisErrorThrown` | `Error`、`ErrorThrown` |
| `ServiceCatalog::share_by_name()`、`share_registered_only()` | 删掉这一行（默认按名字共享） |
| `Loader::waiting_for(&entry)` | `Loader::waits(&entry)`（没标注的 Rust 服务的说明在 `WaitReason::Explained`） |
| `Loader::locate(plugin)` | `Loader::row(instance)` |
| `Loader::schema_of(name)` | `loader.resolve(name).await?.schema` |
| `rutis_loader::runtime::go::{GoBinaries, GoManifest, GoBinary, GoResolver, GoRuntimes, GoRuntimesHandle, GoRuntimeState, GoRuntimeInfo}` 与 Swift 同名 8 个 | `rutis_loader::runtime::binary::{Binaries, BinaryManifest, BinaryFile, BinaryResolver<Go>, BinaryRuntimes<Go>, BinaryRuntimesHandle<Go>, BinaryRuntimeState, BinaryRuntimeInfo}`，`Go`、`Swift` 在 `runtime::go`、`runtime::swift` |
| `ctx.plugin_from(\|cfg\| …)` | 实现 `rutis::host::PluginFactory`，`ctx.plugin_with(factory, config)` |
| `rutis_loader::status::RowState`（`status::Row::state` 的类型） | `rutis_loader::report::EntryStatus`：等待是 `Running` 加 `Pending` 阶段，启动中是 `Loading` 阶段；`Display` 打出的文字不变 |
| `rutis_loader::report::RowState`（`rutis/row` 事件的取值） | 改名 `rutis_loader::report::RowEventState`，取值不变 |
| `rutis_loader::internal::*` | `rutis_loader::patch::{apply_patches, Composed, ComposedRow, Owner, PatchWarning}`、`rutis_loader::runtime::is_runtime_name`、`rutis_loader::runtime::binary::read_binary_manifest`；插件 API 版本用 `Go::PLUGIN_API`、`Swift::PLUGIN_API`（`BinaryLanguage` 的常量） |
| `take_cleanup_errors()` | 清理的错误在卸载结果里（`dispose` 返回的错误） |
| `FiberView::effects()`、`EffectMeta`、`EffectPhase` | 无替代；诊断用 `diagnostics()` |
| `EventBus::observe_dispatch` | 无替代；要观察就注册一个普通监听器 |
| feature `anyhow`、`Error::from_anyhow` | `.map_err(\|e\| Error::msg(format!("{e:#}")))` |
| `rutis_bridge::cordis::build::rutis_plugin`、`cordis::server` | 删（Cordis 宿主挂 Rust 插件的方向不再支持） |
| `waterfall_sync` | 改用 `waterfall`（异步）或同步 `bail` |
| `internal::intercept_require_as`、`intercept_set_as`、`ServiceIntercept`，`ServiceReadFailure` / `ServiceWriteFailure` 的 `Intercept*` 变体 | 无替代 |
| `rutis_bridge::build::Exports`，build-dependency `rutis-bridge = { features = ["exports"] }` | `rutis_types::Exports`，build-dependency `rutis-types`；写法不变 |
| `internal::config::{closest, decode, …}` | `rutis::config::closest`；校验 `rutis::config::check` |
| `rutis_loader::config::check(schema, config, options)` | `rutis::config::check(schema, config)` |
| `internal::provide_checked`、`refresh`，`DependencyStatus::CheckPending`、`CheckRejected`、`CheckPanicked` | 无替代 |

### 工具

- `rutis-dev`（dev 通道）删除；开发时用 `rutis-host dev`。
- `rutis-migrate` 不再随 0.19 发布。0.17 的迁移仍可用 `cargo install rutis-migrate --version 0.18.0`。
- `rutis-host go add <模块>@<版本>` 删除；用 `rutis-host add <模块>@<版本>`，它用同样的 `go install` 装到 `runtimes.go.dir`，并加上行。只要二进制、不要行时，手动 `GOBIN=<runtimes.go.dir> go install <模块>@<版本>`。

### Go 插件

用 0.18 及以后的 SDK 重新构建（0.18 前构建的二进制的 `required` 例外删除）。
