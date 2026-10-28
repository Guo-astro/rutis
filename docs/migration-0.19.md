# 迁移到 0.19

[English](migration-0.19.en.md)

0.19 面向宿主持有者。不兼容的有三处：远程运行时的连接（下面第一节）、宿主一侧的 token，与平台清理（最后一节：删掉的写法直接删，不标弃用，不做转换或提示）；其余是默认关闭的新选项，各节随 0.19 的改动补齐。

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
| `"transport": "<kind>"` | `"transport": "<kind>"`（默认 `websocket`） |

- 多台（`listen` + `instances`）的写法不变。
- **旧写法照常可用，不用改，也不会被转换。** 同一个运行时不要两种都写：`runtimes.remote` 有 `dial` 或 `listen`，又有一条 `"runtime"` 指向它的节点行时，宿主启动报错：

  ```
  remote runtime gpu: link row gpu connects it too: remove the link row, as runtimes.remote connects it itself
  ```

- 旧写法里节点行的 `export`、`import`、`host`、`rows` 等字段，新写法没有：连接只用来运行这个运行时的行。还要这些字段的，继续用节点行。
- token 是 `RUTIS_TOKEN_<名字>`（见[宿主一侧的 token](#宿主一侧的-token695)）。新写法的一台 `listen` 没有 token 时启动报错。
- 自建的连接方式（中继、QUIC 等）实现 `Transport`，由嵌入 rutis-host 的 Rust 程序挂上（`Embedding` 新增的 `transports`），`transport` 写它的名字；一台、多台都用它。

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

- `rutis_host::host::Embedding` 新增字段 `transports`。用结构体字面量写全所有字段的代码要加上 `transports: Vec::new()`，或改用 `..Embedding::default()`。
- `rutis_host::control::serve` 在 `Loader` 之后多了一个参数 `Report`（一个返回状态文字的函数，`status` 用它回答）；用 `Host::status_report()` 得到它。
- 运行时一侧、会话协议与 rutis、rutis-loader、rutis-bridge 的 Rust API 都不变。

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

- 进阶的名字用显式导入（IDE 一键替换补的就是显式导入）。0.19 里旧名字和新名字都在，同时写 `import arcships.rutis.*` 和 `import arcships.rutis.advanced.*` 时，编译器对 `RutisFormat`、`Scope` 等报 `Overload resolution ambiguity`。只写 `import arcships.rutis.*` 的旧代码不受影响。
- 这些类换了包。单独发布、预先编译好的 Kotlin 插件库要用 0.19 的 SDK 重新编译后，才能和 0.19 的 SDK 放进同一个应用（源代码不用改）。

不变的：每种语言根上的 `definePlugin` / `define_plugin` / `Define` / `Rutis.define`，`ctx` 上的所有方法，测试工具，Rust。Swift 宏与 Kotlin KSP 生成的代码随 SDK 一起更新，不用改；0.17.2 的 `rutis-ksp` 生成的代码在 0.19 的 SDK 上照样编译，只有弃用警告。插件 API 版本不变，用旧 SDK 构建的插件在 0.19 的运行时上照常运行。

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
| `rutis_bridge::cordis::build::rutis_plugin`、`cordis::server` | 删（Cordis 宿主挂 Rust 插件的方向不再支持） |

### 工具

- `rutis-dev`（dev 通道）删除；开发时用 `rutis-host dev`。
- `rutis-migrate` 不再随 0.19 发布。0.17 的迁移仍可用 `cargo install rutis-migrate --version 0.18.0`。
