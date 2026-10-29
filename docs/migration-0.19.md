# 迁移到 0.19

[English](migration-0.19.en.md)

0.19 面向宿主持有者。不兼容的有四处：远程运行时与设备的连接（下面第一节）、宿主与运行时要一起升级（会话协议 4）、宿主一侧的 token，与平台清理（最后一节：删掉的写法直接删，不标弃用，不做转换或提示）；其余是默认关闭的新选项，各节随 0.19 的改动补齐。

## 远程运行时与设备（#695、#696、#698）

设计：[远程连接：只留一个会话插件](design-remote-session-2026-10-28.md)。

### 新写法：连接行

远程运行时与设备都写成连接行：`rutis-host/dial`（rutis 去连）或 `rutis-host/listen`（对面连进来）。`runtimes.remote` 删除，不标弃用，不做转换（rutis-host 自带两个连接插件：dial 与 listen）；写了它的 rutis.json 按 0.18 的规则报不认识的字段，没有专门的提示：

```
rutis-host: rutis.json: unknown field `remote`, expected one of …
```

本机运行时（`runtimes.node` / `py` / `go` / `swift` / `bun`）的写法不变。

```json
// 0.18
"runtimes": { "remote": [{ "name": "gpu", "language": "python" }] },
"rows": [
  { "id": "gpu", "name": "rutis-bridge/peer", "config": { "peer": "gpu", "dial": "wss://gpu.example.com:7443/rutis", "runtime": "gpu" } },
  { "id": "embedder", "name": "gpu:embedder" }
]

// 0.19
"rows": [
  { "id": "gpu", "name": "rutis-host/dial", "config": { "address": "wss://gpu.example.com:7443/rutis" } },
  { "id": "embedder", "name": "gpu:embedder" }
]
```

| 0.18 | 0.19 |
| --- | --- |
| `runtimes.remote` 一条 + 带 `runtime` 的 `rutis-bridge/peer` 行，`dial` | 一行 `{ "id": "gpu", "name": "rutis-host/dial", "config": { "address": "<地址>" } }` |
| 同上，`listen` | 一行 `rutis-host/listen`，`"listener": "<监听器>"` |
| `runtimes.remote` 的 `listen` + `instances` 与 `instanced` 分组里的行 | 一行 `rutis-host/listen`；分组里的远程插件行删掉，改由应用自己装插件（见下面的[应用](#应用kotlinswift)）；宿主上每台设备一份的行（`py:coach`）按同一节改 |
| `runtimes.remote` 的 `language` | 不写：插件名在对面解析 |

- 行 id 是连接的名字：运行时的 `--id` 写它，token 是 `RUTIS_TOKEN_<行 id>`（见[宿主一侧的 token](#宿主一侧的-token695)），输出以它开头。
- 在对面运行的插件仍写成 `<连接行>:<插件>`（`gpu:embedder`），配置写在这一行里。 这样的行用于 `rutis-host/dial`：0.18 里由运行时连进来、rutis 决定它跑什么插件的，改为运行时监听、rutis 去连。
- `rutis-host/listen` 上同一个 id 的新连接顶替旧的：旧连接的远程插件先全部卸载，旧连接再以关闭码 4002 关闭，然后才注册新连接的插件。宿主停止时连接以关闭码 1000 关闭。
- 宿主不再为每条连接建实例：对面带来的插件注册为 `<对面的 id>:<插件>`，服务全局可见，同名的按内核规则，后注册的被拒绝。

### 节点

节点之间也用连接行连：一端 `rutis-host/dial`，另一端 `rutis-host/listen`（[连接节点](guide/nodes.md)）。`rutis-bridge/peer` 行与 `peer:` 行删除，不标弃用，不做转换；`rutis-bridge/peer` 是 0.19 不认识的插件名，按 0.18 的规则报错，没有专门的提示：

```
office (rutis-bridge/peer): no plugin named "rutis-bridge/peer"
```

```json
// 0.18：main 一侧
{ "id": "office", "name": "rutis-bridge/peer", "config": { "peer": "office", "dial": "wss://office.example.com/rutis", "export": ["weather"], "import": ["clock"], "rows": true } },
{ "id": "scanner", "name": "peer:office/scanner", "config": { "dpi": 300 } }

// 0.19：main 一侧
{ "id": "office", "name": "rutis-host/dial", "config": { "address": "wss://office.example.com/rutis" } },
{ "id": "scanner", "name": "office:scanner", "config": { "dpi": 300 } }

// 0.19：office 一侧（0.18 是带 listen 的 rutis-bridge/peer 行，host: true）
{ "id": "main", "name": "rutis-host/listen", "config": { "listener": "public" } }
```

| 0.18 | 0.19 |
| --- | --- |
| 节点行 `rutis-bridge/peer` 的 `dial` | 一行 `rutis-host/dial`，`"address": "<地址>"` |
| 节点行 `rutis-bridge/peer` 的 `listen` | 一行 `rutis-host/listen`，`"listener": "<监听器>"` |
| `export`、`import`（服务与事件，含 `*` 前缀） | 不写：服务两边都全局可见，事件一条总线 |
| `host: true` + `rows: true` + 行 `peer:office/scanner` | 行 `office:scanner`（对面不用打开什么） |
| `peer` | 不写：对面的 id 来自握手（它 rutis.json 的 `id`） |
| `require`、`declare`、`transport`、`identity` | 不写；自己的连接方式与凭据写成连接插件，经 `Embedding.builtins` 注册 |
| `@arcships/rutis-runtime/bridge`（Cordis 应用作为节点：`Link`、`Export`、`Import`、`Host`、`Events`） | 删除：Cordis 应用接 rutis：用 Node 运行时跑它的插件（`<包名>` 行） |

- token 按连接行的 id：`main` 的 `office` 行出示 `RUTIS_TOKEN_OFFICE`，`office` 的 `main` 行核对 `RUTIS_TOKEN_MAIN`（以前是节点行的 `peer`，名字通常相同）。
- 同名服务：以前 `import` 一个本地已有的名字时拒绝这次导入；现在按内核规则后注册的被拒，那个远程插件失败：`office:weather: weather is provided here by row weather: not registered from office`。
- **给出 token 等于允许对方在本节点运行它能解析的任何插件**（以前要对方打开 `host: true`；谁能连上由连接行的 token 决定，连上即信任）。相连的节点互相看到对方全部可共享的服务：只连可信的节点；不给对面看的服务在提供它的行上用 `isolate`。
- 一个节点经一条连接收到的服务，经别的连接继续到达第三个节点（以前事件与服务只走一跳）。
- `run` 的输出：`office: link connected` / `link down (…)` / `link refused: …` / `link stopped: …` 改为 `office: connected` / `office: disconnected (…); retrying in …` / `office: disconnected (incompatible: …); stopped`；listen 一侧写 `main: connected main`、`main: disconnected main (…)`。

### 行为变化

1. **远程运行时缺插件，只是这一行失败。** 对面没有某一行的插件时，以前这一行解析失败（`unresolved: no plugin named "gpu:embedder"`），编辑时新增这样的行被拒绝；现在只有这一行失败，其他行照常，新增这样的行不被拒绝，对面下次连上时重新检查：

   ```
   embedder: unresolved: entry "embedder" rejected: runtime gpu: this connection does not contain plugin embedder
   ```

   用 Rust 嵌入、按 `LoaderError::NotFound` 判断这种情况的代码，改为 `rutis_loader::report::Unavailable::is(&error)`。

2. **运行时缺行约定时的文字去掉版本号：** `the gpu runtime lacks rows.v2 and hosts: a runtime speaking the row contract is required`。

3. **输出。** 连接的状态行以连接行的 id 开头，只用 `connected`、`disconnected` 两个词，原因写在括号里（以前是 `runtime <名>: link …` 与 `<行 id>: link …`）：

   ```
   gpu: connected
   gpu: disconnected (gpu.example.com:7443: Connection refused); retrying in 2.1s
   gpu: disconnected (auth-rejected: gpu.example.com:7443: upgrade refused with 403 Forbidden); retrying in 30s
   gpu: disconnected (incompatible: this endpoint speaks rutis.3, the dialer offered rutis.4: upgrade this endpoint (PyPI rutis)); stopped
   phone: connected phone-1
   phone: disconnected phone-1 (connection lost)
   phone: disconnected phone-1 (replaced by a new connection)
   ```

   listen 上握手失败的连接（token 不对、协议不兼容）不打印，由拨号方写出原因。`rutis-host status` 的 `connections:` 一节每个连接行一行：dial 行写 `connected` / `disconnected`，listen 行写连着的 id（没有时 `none connected`）：

   ```
   connections:
     gpu: connected
     phone: kim, lee
   ```

   读 `run` 输出的脚本按 `runtime <名>: link` 或 `link connected` 匹配的，改为按 `<行 id>: connected` / `disconnected` 匹配。

4. **持久化的 rutis.json。** 0.8–0.18 写了 `runtimes.remote` 的文件 0.19 不再读（上面的不认识的字段）；写了节点行 `rutis-bridge/peer` 的文件，0.19 读得出、但这些行以不认识的插件名失败（见[节点](#节点)）。这是 rutis.json 跨版本可读的例外，`format` 仍为 1。

### 应用（Kotlin、Swift）

- 应用自己装它带的插件，配置写进 `RutisApp(configs: …)`（不写时用插件 Schema 的默认值），连上后报出它们提供的服务，宿主注册为 `<应用的 id>:<插件>`。rutis 一侧 `instanced` 分组里的 `<运行时>:<插件>` 行删掉，只留一行 `rutis-host/listen`（应用拨出）或 `rutis-host/dial`（应用监听）。
- 应用里的插件装不上（配置不合 Schema、`apply` 失败、服务名在 rutis 上已有）时，0.19 只在设备的日志里写出，宿主上不打印。
- 宿主上"每台设备一份"的行（`instanced` 分组里的 `py:coach`）不再支持。改法：把它放到设备上（应用里的一个插件）；或者设备上的插件以 `health-<设备>` 为名提供服务，宿主上的一个插件监听 `rutis/service` 事件，在 `health-<设备>` `available: true` 时调用服务 `loader`：`loader.create({id: "coach-<设备>", name: "py:coach-one", config: {device}, inject: ["health-<设备>"]})`，`available: false` 时 `loader.remove("coach-<设备>")`；在事件处理里调用，不在 `apply` 里。完整例子见 [Swift 应用](guide/swift-app.md#每台设备各算一份)、[Kotlin 应用](guide/kotlin-app.md)。

### Rust 嵌入

- `rutis_host::control::serve` 在 `Loader` 之后多了一个参数 `Report`（一个返回状态文字的函数，`status` 用它回答）；用 `Host::status_report()` 得到它。
- rutis 的 Rust API 不变。rutis-bridge 与 rutis-loader 的连接与会话部分改了，本机的语言运行时也和远程的一样，成了一个会话的对面（#695，[远程连接：只留一个会话插件](design-remote-session-2026-10-28.md) §三、§六、§八、§十三 S4）：

  | 0.18 | 0.19 |
  | --- | --- |
  | `rutis_bridge::transport::{local, memory, websocket}` | `rutis_bridge::{pipe, memory, websocket}` |
  | `Transport`、`Dial`、`transport_key`；`LocalPlugin`、`MemoryPlugin`、`WebSocketPlugin` 与它们的 `*Transport`；`websocket::Config` | `websocket::dial`、`websocket::listen`、`pipe::spawn`、`memory::pair` 直接给出 `Channel` |
  | `LinkPlugin` + `PeerPlugin { runtime }` + `RuntimePlugin::remote`；`RuntimeSession`、`runtime_session_key`、`RuntimeAccessPlugin` | 连接插件里 `ctx.plugin(Session::new(websocket::dial(...)?))`（或 `Session::handshake(&ctx, channel).await` 后 `ctx.plugin(greeted)`），行 `<名>:<插件>` |
  | `AccessPointPlugin`、`AccessPointConfig`、`AccessEvent`、`Accept`、`Accepted`、`Release`；loader 的 `RuntimeInstances`、`AcceptInstances`、`RuntimeConnection` | `rutis-host/listen`，或自己的连接插件 |
  | `rutis_bridge::runtime::LocalRuntime`（`node`、`python`、`go`、`swift`、`bun`、`launcher`、`named`、`env`、`interpreter`、`python_path`、`stdin` / `stdout` / `stderr`、`greeting`、`handle`、`processes`） | `rutis_host::connect::SpawnPlugin::new(名字, launcher, anchor)`，即插件 `rutis-host/spawn`（依赖 `rutis-host`）：`launcher` 是 `rutis_bridge::runtime::Launcher::node(&包)`、`::python`、`::go`、`::swift`、`::bun` 或 `Launcher::new`，`名字` 是原来 `named` 给的（默认 `node`、`py` 等）；`.interpreter`、`.python_path`、`.env`、`.stdio(stdin, stdout, stderr)`、`.greeting(时限)`、`.handle()`、`.processes()`；`output_prefix` 在 `Launcher` 上。用 `root.plugin(spawn)` 挂上；根上提供会话问候用的 id：`root.provide(rutis_bridge::LocalId(PeerId::new("main")?))` |
  | `RuntimePlugin`（`session`、`named`、`host`、`starting`、`stopped`、`failed`、`handle`），服务 `runtime::Runtime`（`Runtime::key`、`host_methods`） | 删：运行时是一个会话的对面。它的状态由 `rutis_bridge::runtime::RuntimeStates` 保存（`new(名字, anchor)`、`starting`、`ready`、`failed`、`stopped`，`announce(ctx)` 发 `rutis/runtime` 事件），通过它的 `RuntimeHandle`（`handle()`）读取；`ended_status(&process)` 说明进程怎样结束 |
  | `RuntimePlugin::host(名字, methods)`、`LocalRuntime::host` | 宿主服务用 `HostDispatch::methods()` 给出它的方法 |
  | `rutis_loader::runtime::RuntimeResolver::node(handle)` / `::modules(handle)`、`with_catalog`；`invalidate`、`invalidate_all`，rutis-host 的 `Host::invalidate`；`runtime::resolve_entry` | 所有运行时共用一个 `rutis_loader::runtime::SessionResolver`，放在 chain 最后（`Chain::new().with(builtins).with_shared(sessions.clone())`）：`SessionResolver::npm(&catalog, "node")`（npm 名、子路径、`file:` URL、绝对路径也认，放在行 `node` 上）或 `SessionResolver::new(&catalog)`（行 `<行>:<插件>`，如 `py:shout`）；每个本机运行时调一次 `.runtime(handle)`（`Loader::status` 列出它，等它的行写 `waiting for runtime py (starting)`）；挂载 loader 之前 `.attach(&loader)`。Rust 不再解析 npm 名，也不再按 `package.json` 缓存：由 Node 运行时在每次解析时做；改了插件代码直接 `Loader::reload(行)` |
  | `RuntimeRows`（`RuntimeRows::key`）、`RowsSource` | `SessionRows`、`SessionRows::key(行)` |
  | `RuntimeRowsPlugin::new(resolver)` | loader 运行后 `SessionResolver::follow_row(名字)`；运行时重启后，它的行自己重新解析 |
  | `BinaryLanguage::launch` 返回 `LocalRuntime`；`BinaryRuntimes::new(resolver, project)` | `launch` 返回 `Launcher`；`BinaryRuntimes::new(resolver, project, spawn)`，`spawn` 是 `rutis_loader::runtime::binary::Spawn`（`Arc<dyn Fn(&str, Launcher, &Path) -> Spawned>`，`Spawned { plugin, handle, processes }`）；rutis-host 传入的是做出 `SpawnPlugin` 的那个 |
  | `LocalRuntime::go(binary, project)` 加 `RuntimeResolver::modules`（一个固定的二进制） | `SpawnPlugin::new(名字, Launcher::go(&binary, &project), &project)`，行 `<名字>:<插件>` 经 `SessionResolver` 解析 |
  | `Admission`、`AdmissionPlugin`、`admission_key`、`SharedToken`、`Rejection` | 连接插件自己的认证；id 来自握手 |
  | `ListenerConfig::new(名字, 地址, 本端 id)` | `ListenerConfig::new(名字, 地址)` |
  | `Closer::replaced()` / `rejected(r)` / `close(&str)` | `close(Close::Replaced)` / `close(Close::Rejected(r))` / `close(Close::Done)` |
  | `Channel` 的 `info`（`ChannelInfo`）、`Spawn::new(程序, peer)` | 删；对方 id 只来自会话握手；`Spawn::new(程序)` |
  | `rutis_host::host::Embedding` 的 `transports` | 删；自己的连接方式与凭据写成连接插件，经 `Embedding.builtins` 注册 |
  | `Connection::open(channel, dispatch)`、`Connection::open_with(channel, dispatch, Format::Endpoint(e))`、`Format` | `Connection::open(channel, dispatch, e)`：只有 endpoint 格式，本端总要报自己的 id |
  | `Process::attach(channel, mount, Format::Endpoint(e))` | `Process::attach(channel, mount, e)` |
  | `session::PROTOCOL`（2）、`cordis::PROTOCOL`；`session::ENDPOINT_PROTOCOL` 为 3 | 删；`session::ENDPOINT_PROTOCOL` 与 `cordis::ENDPOINT_PROTOCOL` 为 4 |
  | `LinkPlugin`、`LinkConfig`（及其 `compat`）、`LinkState`、`Connect`、`Dialer`、`Failure`、`Retry`；`pipe::dial`、`memory::dialer`、`websocket::admit_registered`、`protocol()` | 删：去连与重连在 `rutis-host/dial`，或自己的连接插件；子协议是 `format!("rutis.{}", session::ENDPOINT_PROTOCOL)` |
  | `PeerPlugin`、`PeerHandle`、`Features`；`Peer`、`peer_key`、`Handler`、`Offered`、`Offers`、`family()` | 删：节点也是一个 `Session`，对面的插件是 `<id>:<插件>` |
  | `ExportPlugin`、`ImportPlugin`、`HostPlugin`、`LinkEventsPlugin`、`ServiceKeys` | 不需要：服务两边都全局可见，事件一条总线，对面经 `run` 运行插件 |
  | `Identity`、`IdentityPlugin`、`identity_key`、`StaticIdentity`、`Presented`；`Registration`、`Registered`、`RegistrationError`、`Registrations`、`Accepting`、`Ticket`、`Refusal`、`Deliver` | 连接插件自己的认证；id 来自握手。`fingerprint` 保留 |
  | `rutis_bridge::Credential` | `rutis_bridge::websocket::Credential`（`websocket::dial` 出示的凭据） |
  | `Installed { described: Described { schema, version, integrity }, factory }`；`StaticCatalog::with(名, described, factory)` | `Installed { schema, version, factory }`；`StaticCatalog::with(名, Installed::new(factory))` |
  | rutis-loader 的 feature `peer` 与 `rutis_loader::peer`（`PeerResolver`、`PeerRows`、`PeerRowsPlugin`、`register_peer_node`、`node_schema`、`LoaderCatalog`） | 删；`<连接行>:<插件>` 的行由 `runtime::SessionResolver` 解析，`LoaderCatalog` 在 `rutis_loader::runtime` |
  | `rutis_host::host::Embedding` 的 `identity` | 删 |
  | 自建连接实现 `Transport` | 交出一条 `Channel`：`ctx.plugin(Session::new(channel))`，或先 `Session::handshake(&ctx, channel).await` 再 `ctx.plugin(greeted)`；其他语言用服务 `sessions` 的 `open` |

- 自己挂 `Session` 的 Rust 嵌入要在根 ctx 提供本端在握手里报的 id：`root.provide(rutis_bridge::LocalId(id))`；没有时会话失败：`the host provides no id for its sessions`。rutis-host 已经提供（rutis.json 的 `id`，默认 `main`）。

- 自建连接：交出一条 `Channel`：写一个连接插件，调 `Session::new(channel)`；其他语言用 `sessions.open`。

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

`<名字>` 是连接行（`rutis-host/dial`、`rutis-host/listen`）的 id，大写，`-` 写作 `_`：

```bash
# 0.18：一个 RUTIS_TOKEN 给所有对端
RUTIS_TOKEN=secret rutis-host run rutis.json
# 0.19：每个连接行一个
RUTIS_TOKEN_OFFICE=secret RUTIS_TOKEN_GPU=secret rutis-host run rutis.json
```

没改时看到的：

- `rutis-host/listen` 的行没有 token 时这一行失败：

  ```
  phone: set RUTIS_TOKEN_PHONE: the token the far ends present when they connect
  ```

- `rutis-host/dial` 的行不出示 token，对面拒绝，`run` 每 30 秒重试一次：`office: disconnected (auth-rejected: …); retrying in 30s`。

同一个监听器上两个条目的 `RUTIS_TOKEN_<名字>` 设成同一个值时仍在启动前报错（#702），文字改为写明两个变量：

```
rutis-host: listener public: row main accepts RUTIS_TOKEN_MAIN and row phone accepts RUTIS_TOKEN_PHONE, the same token, so a connection presenting it cannot be told apart: give RUTIS_TOKEN_MAIN or RUTIS_TOKEN_PHONE another value
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
| 生成类型 `ErrorThrown`，指 rutis 以外、名叫 `Error` 的错误类型时（`io::Error`、`anyhow::Error` 等） | `<模块名>ErrorThrown`（`IoErrorThrown`、`AnyhowErrorThrown`）：`ErrorThrown` 只留给 rutis 自己的错误；抛出时的 `name` 不变，仍是 `Error`（#771 X4） |
| `ServiceCatalog::share_by_name()`、`share_registered_only()` | 删掉这一行（默认按名字共享） |
| `Loader::waiting_for(&entry)` | `Loader::waits(&entry)`（没标注的 Rust 服务的说明在 `WaitReason::Explained`） |
| `Loader::locate(plugin)` | `Loader::row(instance)` |
| `Loader::schema_of(name)` | `loader.resolve(name).await?.schema` |
| `rutis_loader::runtime::go::{GoBinaries, GoManifest, GoBinary, GoResolver, GoRuntimes, GoRuntimesHandle, GoRuntimeState, GoRuntimeInfo}` 与 Swift 同名 8 个 | `rutis_loader::runtime::binary::{Binaries, BinaryManifest, BinaryFile, BinaryResolver<Go>, BinaryRuntimes<Go>, BinaryRuntimesHandle<Go>, BinaryRuntimeState, BinaryRuntimeInfo}`，`Go`、`Swift` 在 `runtime::go`、`runtime::swift` |
| `ctx.plugin_from(\|cfg\| …)` | 实现 `rutis::host::PluginFactory`，`ctx.plugin_with(factory, config)` |
| `rutis_loader::status::RowState`（`status::Row::state` 的类型） | `rutis_loader::report::EntryStatus`：等待是 `Running` 加 `Pending` 阶段，启动中是 `Loading` 阶段；`Display` 打出的文字不变 |
| `rutis_loader::report::RowState`（`rutis/row` 事件的取值） | 改名 `rutis_loader::report::RowEventState`，取值不变 |
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
| `rutis_loader::backoff`、`runtime::binary::{RestartPolicy, RuntimeEnd}`、`BinaryRuntimesHandle::supervise` / `unsupervise` | 删：加载器不再重启崩溃的 Go/Swift 运行时。自己调用 `BinaryRuntimesHandle::restart(name)`；握手被拒的二进制用新的 `runtime::binary::refused(why)` 判断，重启也起不来。用 rutis-host 时写 `runtimes.<key>.restart` |

### 工具

- `rutis-dev`（dev 通道）删除；开发时用 `rutis-host dev`。
- `rutis-migrate` 不再随 0.19 发布。0.17 的迁移仍可用 `cargo install rutis-migrate --version 0.18.0`。
- `rutis-host go add <模块>@<版本>` 删除；用 `rutis-host add <模块>@<版本>`，它用同样的 `go install` 装到 `runtimes.go.dir`，并加上行。只要二进制、不要行时，手动 `GOBIN=<runtimes.go.dir> go install <模块>@<版本>`。

### Go 插件

用 0.18 及以后的 SDK 重新构建（0.18 前构建的二进制的 `required` 例外删除）。

行名 `go-<二进制>:<插件>`、`swift-<二进制>:<插件>` 删除，写 `go:<插件>`、`swift:<插件>`；`rutis.json` 里还有旧写法时，读文件就报错并给出新写法。两个二进制有同名插件时这一行失败、写出两个文件名，由插件作者改名。

- `rutistest` 在真正的内核下运行插件：需要 `PATH` 上的 `rutis-host`，或在 `RUTIS_HOST` 里给出它的路径（模板的发布 workflow 在 `go test` 之前安装它）。`Load`、`TryLoad`、`Loaded` 的形状不变；`TryLoad` 交回的错误现在是宿主为这一行写的那一行（`weather: config: city is required`）。测试工具不再有作用域：删掉 `InScope`、`InChain` 和 `Loaded.Scope`，也删掉 `RuntimeName`（运行时由宿主命名）。测试与插件之间的调用，是同一个 Go 运行时里两行之间的调用。同时加载着的两行，测试给的同名服务是同一个提供者：给同一个值。
- 运行时不再按 Schema 检查行的配置，也不再补默认值：由宿主做，并发来补好的配置；运行时只解码（解不成配置类型时是 `config: <路径> is not a value the plugin accepts`）。
- 所有发出都经过宿主，监听器全在同一个二进制里时也一样。会话结束后（包括那时运行的清理里）`ctx.Emit` 等发出以 `the host disconnected` 失败，不再送到同一个二进制里的监听器。

### TypeScript 与 Bun 插件

`@arcships/rutis/testing` 在真实的宿主上运行插件（`rutis-host test`，见 [插件 API：TypeScript 与 Bun 的测试工具](guide/plugin-api.md#typescript-与-bun-的测试工具)）：

- 开发依赖加上 `@arcships/rutis-runtime`（Bun 下 `@arcships/rutis-bun`）与 `@arcships/rutis-host`（`rutis-host new` 生成的项目已经有），或用 `RUTIS_HOST` 指向一个 `rutis-host`。
- 插件要是它那个模块的默认导出：在测试文件里直接 `definePlugin` 的插件挪到自己的文件里，否则 `load` 报 `load needs a plugin that its module exports as default`。
- 删去的选项：`strict`、`scope`、`scopes`、`deadline`、`runtime`、`row`，以及 `h.at(scope)`。
- `t.reported`、`h.reported` 改为方法：`await t.reported()`，每项是 `{ line }`，`line` 是宿主打印的那一行。
- 测试与插件之间的值按跨进程的规则传递；`t.on` 收到的事件经宿主送达，比发出方的返回晚一点到。
- 配置由宿主检查并补上默认值；装载失败是 `PluginTestError`，文字是这一行的状态，如 `weather: config: city is required`。
- 运行时不再在本进程里分发事件：每次发出都经过宿主（`emit` 的中位数约 71 微秒，见 [事件基准](performance-events-2026-10-28.md)）。

### Python

最低版本 3.12（3.10、3.11 不再支持）。

### Swift

- `RutisTest` 把插件作为真实宿主 `rutis-host test` 的一行运行，宿主由它启动：把 `rutis-host` 放在 `PATH` 上（`npm install --global @arcships/rutis-host`），或用 `RUTIS_HOST` 给出路径。整个测试进程共用一个宿主；`close()` 卸掉这个测试的行。
- 测试工具不再有 scope：`RutisTest.scope(_:groups:)`、各处的 `scope:` 参数和 `Loaded.scope` 删除，`runtimeName:` 也删除。`row:` 是行 id（默认是插件名）；`isolate:` 是这一行的 `isolate`，与 rutis.json 相同。
- `loaded.reported` 要 `await` 读。宿主不接受的配置让 `load` 以宿主为这一行写的行失败：`weather: config: city is required`。
- 测试的服务、`on` 与发出是插件所在运行时里的一个插件：测试与插件之间的值不再跨进程。
- 设备与 iOS 模拟器上，`RutisTest` 是一个自己装插件的应用，连到开发机上的宿主（`RUTIS_TEST_CONNECT`、`RUTIS_TEST_TOKEN`；见 [Swift 应用指南](guide/swift-app.md)）；那里不能用 `Loaded.load` 与 `update`。
- Swift SDK 不再校验配置：宿主检查一行的配置并填好默认值再交给运行时。应用自己装的插件只解码：用 `@RutisConfig` 时，缺的键取类型里写的字面初始值；缺必填的键或类型不对时以 `config: <键> is required` 或 `config: <键> should be <类型>, got <值>` 失败。
- Swift 插件的事件都由宿主分发；应用自己装的插件的监听与发出算作这条连接的。

### Kotlin

- 测试工具是单独的构件 `io.github.arcships:rutis-testing`（jvm）：`testImplementation("io.github.arcships:rutis-testing:<版本>")`，并 `kspTest("io.github.arcships:rutis-ksp:<版本>")`。它在测试进程里跑插件真实的运行时，作为 `rutis-host test` 的一行，需要 `rutis-host`（`PATH` 上或 `RUTIS_HOST`）。
- 测试工具不再有 scope：`RutisTest.load(scope:)`、`Loaded.load(scope:)`、`RutisTest.scope(...)` 删除；`RutisTest.load(runtime:)` 与 `Loaded.provided` 删除。行 id 不给时由 rutis 按插件名取（不再是 `test`）；装载失败的文字是该行的状态行（`weather: config: city is required`）；`loaded.reported()` 是 rutis 报告的行（`weather: listener of x failed: …`）。
- 测试给的服务、`loaded.on`、`emit` 等是同一个运行时里的插件，值不跨进程。
- 设备上的测试不再用 `RutisTest`：用 `RutisApp` 装自己的插件，拨到开发机上的 rutis。
- 事件一律经宿主分发：Android 主线程上的同步 `bail` 总是被拒绝（以前本进程里有监听器时可以）。
- 配置的校验与补默认值只在 rutis 里做，SDK 只解码。应用用 `RutisApp` 自己装的插件不经 rutis 校验：没给的键用构造参数的默认值，缺必填或类型不对时以点名键的 `config: …` 装载失败（`config: city is required`）。
