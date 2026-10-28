# 远程连接：只留一个会话插件

[English](design-remote-session-2026-10-28.en.md)

状态：草案，未实现。日期：2026-10-28。基准：`main` `3d19634f`。排期：0.19。
Issue：[#695](https://github.com/arcships/rutis/issues/695)（实施 [#696](https://github.com/arcships/rutis/issues/696) loader、[#698](https://github.com/arcships/rutis/issues/698) rutis-host）。依据：#695 的 2026-10-28 两条评论。
与旧设计的关系：取代 [远程运行时一致性](design-remote-runtime-consistency-2026-10-26.md) 的实施部分（§四"rutis-host 内部"一列、§五、§七、§八）；它的"缺插件一律 `Unavailable`"保留，`runtimes.remote` 的写法删掉（§六）。[决策记录](decision-remote-runtime-consistency-2026-10-26.md) 里"不新增交接接口、不重写 link 与接入点"一条由本稿推翻，理由见 §一。

范围外：会话协议（不改）；各语言运行时与 SDK 的会话一侧（不改）；认证放开（自定义凭据交给连接插件自己做，不在核心里开接口）；运行中吊销（见 §八 `Revocations`）；loader 的行模型与 `instanced` 分组本身。

## 一、现状

### 1.1 四条生命周期路径

同一件事——在一条连接上把对面的插件接进本地内核——现在有四套代码，各自管连接、准入、顶替、状态：

| # | 场景 | 连接 | 交给插件管理的方式 | 插件管理 |
| --- | --- | --- | --- | --- |
| L1 | 本机运行时 | `LocalRuntime`：本机传输起进程，`LinkPlugin` 承载会话 | 进程内直接持有 | `RuntimePlugin` + 各语言的 `RuntimeResolver` |
| L2 | 一台远程运行时 | `LinkPlugin`（dial 或 listen）→ `PeerPlugin { runtime }` → `RuntimeAccessPlugin` | 服务 `RuntimeSession#<名>` | `RuntimePlugin::remote` + `RuntimeResolver` |
| L3 | 多台连进来的运行时 | `AccessPointPlugin`（在 `Transport` 上登记 `Registration`，`Admission` 准入，每个 id 一个 slot） | 回调 `Accept` / `Accepted` / `Release` | `RuntimeInstances`、`AcceptInstances`、实例值 `RuntimeConnection` |
| L4 | 节点互联 | `PeerPlugin`（`LinkPlugin` + `ExportPlugin` + `ImportPlugin` + `LinkEventsPlugin` + `HostPlugin`） | `Peer#<id>` 服务 | loader 的 `PeerRows` / `PeerResolver`（`peer:<对端>/<插件>` 行） |

### 1.2 两种交接形状

- **服务形状**（L2、L4）：连接一侧提供 `RuntimeSession#<名>` 或 `Peer#<id>`，插件管理一侧 `inject` 它。
- **回调形状**（L3）：接入点把每条连接交给外部传进来的 `Accept`，拿回一个 `Release`。

### 1.3 写了多份的东西

| 职责 | 现在的份数 | 在哪 |
| --- | --- | --- |
| 准入 | 3 | WebSocket 升级里的 `Identity::accept`；接入点 hello 之后的 `Admission::admit`；`Revocable` 包在两者外面 |
| 顶替 | 3 | link 监听端的 `Registration` 代次；接入点按 id 的 slot；运行时一侧的租约（这一份在对面，保留） |
| 状态 | 3 | `LinkState`、`AccessEvent`、`RuntimeState` |
| 选传输 | 3 | `Transport#<kind>` 服务、节点行与 `runtimes.remote` 的 `transport` 字段、`Embedding.transports` |
| 发现与检查 | 1 份代码，2 个调用点 | #768 合成了一份（`discover.rs`、`Process::check_rows`），仍由 L2、L3 各调一次 |

### 1.4 "现在"的结构

```text
rutis-host
 ├─ Transport#local、Transport#websocket、Embedding.transports 的 Transport#<kind>
 ├─ Identity#host（或 Embedding.identity）             ← 升级时比对 token
 ├─ L1 每个本机运行时：RuntimePlugin ─ LocalRuntime ─ LinkPlugin ─ 子进程
 ├─ L2 每个一台远程运行时：PeerPlugin { runtime }
 │     ├─ LinkPlugin（dial / listen，Registration）   ← LinkState
 │     └─ RuntimeAccessPlugin ── 提供 RuntimeSession#<名>
 │   RuntimePlugin::remote ── inject RuntimeSession#<名> ← RuntimeState
 ├─ L3 每个多台远程运行时：AdmissionPlugin（SharedToken）
 │     AccessPointPlugin（Registration，slot）        ← AccessEvent
 │       └─ Accept = RuntimeInstances::accept ── 每条连接一个实例（RuntimeConnection）
 └─ L4 每条节点行：PeerPlugin
       ├─ LinkPlugin ─ ExportPlugin ─ ImportPlugin ─ LinkEventsPlugin ─ HostPlugin
       └─ PeerRowsPlugin ── peer:<对端>/<插件> 行
```

上一轮的决策记录认为"已经分好了层、有两个交接点"，所以只补一致性。复查 0.19 时看清：两个交接点本身就是重复（一件事两种形状），而准入、顶替、状态各写三份，分层没有减少代码，只是把重复分到了不同文件。

## 二、新架构

原则：只留协议本身需要的和有使用方的；照搬旧功能语义再补规则的不做。默认与 Cordis 一致：服务全局可见，同名按内核规则冲突，事件是一条平的总线，`isolate` 由宿主在自己的行上用。

### 2.1 插件

| 插件 | 归属 | 作用 |
| --- | --- | --- |
| `rutis-bridge/session` | rutis-bridge，核心里唯一与远程有关的插件 | 在一条 `Channel` 上把对面的插件接进本地内核；每条 Channel 一个 |
| 远程插件 `<id>:<插件名>` | session 的子插件 | 对面的一个插件在本地的样子；远程插件只有这一种名字 |
| 连接插件 | 核心之外，谁写都行 | 拿到一条 `Channel`，挂一个 session |
| └ `rutis-host/spawn`、`rutis-host/dial`、`rutis-host/listen` | rutis-host 自带 | 起本机进程、去连、监听；本机运行时、远程运行时、设备、节点都用它们 |

### 2.2 结构

```text
连接插件（rutis-host/dial、rutis-host/listen、rutis-host/spawn，或别人写的）
 └─ rutis-bridge/session             每条 Channel 一个
     ├─ <id>:weather                 对面的插件
     └─ <id>:llm
```

### 2.3 关系

- **连接插件 → session**：`ctx.plugin(Session::new(channel))`。Channel 关闭，session 卸载；session 卸载，Channel 关闭。
- **session → 远程插件**：对面的插件有两种来源，都注册为 `<id>:<插件>`，结束或 session 卸载时卸载：
  - 本端让它运行：行 `<连接行>:<插件>`，loader 找到 session 调 `run`；
  - 对面本来就在运行、提供着服务的插件（对面是 rutis 节点时）。
- **服务**：经 session 两边都全局可见。对面插件 `inject` 的服务从本端借（现在本机运行时就是这样）；对面提供的服务由它的远程插件在本端 `provide`。同名按内核规则：后注册的被拒绝。经同一个 session 来的服务不再发回这个 session。
- **事件**：一条平的总线。对面的监听器是本端总线上的条目，与现在的运行时相同。
- **远程插件 ↔ 本地插件**：与普通插件相同（`inject` / `provide`），生命周期全部用内核原有的。
- **顶替**：只在 `rutis-host/listen` 内部（§4.2）。session 核心不记 id，不做顶替。

### 2.4 API 入口

Rust（编进宿主的连接插件）：

```rust
pub struct Channel { /* §三 */ }

impl Session {
    pub fn new(channel: Channel) -> Self;
}
impl SessionHandle {
    /// 对方在握手里报的 id。
    pub fn id(&self) -> &str;
    /// 让对面运行插件 `plugin`，用 `config`；成功后它注册为 `<id>:<plugin>`，句柄放下即停止。
    pub fn run(&self, plugin: &str, config: Value) -> Result<RowHandle, Error>;
}
```

任何语言（运行中加载的连接插件）：`rutis-bridge/session` 提供服务 `sessions`，一个方法：

```ts
type Close = { kind: 'done' } | { kind: 'replaced' } | { kind: 'rejected'; reason: string }
interface Far  { send(message: Uint8Array): void; close(why: Close): void }
interface Near { receive(message: Uint8Array): Promise<void>; end(why: Close): void }
sessions.open(far: Far): Near
```

- `far`、`near` 里的函数按引用跨语言（0.18 #687）。
- `open` 建的 session 挂在调用它的插件下面：这个插件卸载，它的 session 全部卸载。

session 的公开 API 只有这四个名字。`run` 留下的理由：让对面运行一个插件是会话协议里的一步（`rows.v2`），对面是语言运行时或应用时只有这一条路；对面是 rutis 节点时也用它，不另开一条。

### 2.5 "以后"的结构

```text
rutis-host
 ├─ rutis-host/spawn  py        ── pipe::spawn --id py ─▶ Channel ─▶ session py
 │                                                                         └─ py:coach …（run）
 ├─ rutis-host/dial   gpu       ── websocket::dial ────▶ Channel ─▶ session gpu
 │                                                                         └─ gpu:embedder（run）
 ├─ rutis-host/listen phone     ── websocket::listen ──▶ 每条连接一条 Channel
 │     ├─ session phone-1   ── 实例 phones #1：phone:health（run）、py:coach
 │     └─ session phone-2   ── 实例 phones #2：phone:health（run）、py:coach
 ├─ rutis-host/dial   office    ── websocket::dial ────▶ Channel ─▶ session office
 │                                                                         └─ office:clock（office 提供着的）
 └─ agent 运行中写的 TS 插件 relay ── sessions.open(far) ──▶ session（id 由对面报）
```

本机运行时、远程运行时、设备、节点都是同一种形状；没有节点或远程运行时专用的代码。

### 2.6 两个待定项的决定

**决定 1：节点互联去掉 `export` 和 `import`。** 服务经 session 两边都全局可见，同名按内核规则；事件是平的总线。这与 Cordis 和现在的运行时一致，节点不需要专门的代码。宿主不想让某些服务被对面看到时，在自己的行上用 `isolate`，这是宿主自己的事，不是远程连接的规则。

逐个核对 [连接节点](guide/nodes.md) 与测试里的用法：

| 现在的用法 | 在哪 | 改成 |
| --- | --- | --- |
| `export` + 对端 `import` 共享服务 | nodes.md；`node.rs`、`websocket_soak.rs`、`cordis_node.rs` | 不写：两边的服务互相可见 |
| `import` 与本地同名服务冲突时拒绝 | nodes.md | 内核规则：后注册的被拒绝，那个远程插件失败并说明（§七） |
| 事件的 `export` / `import` 与前缀 | nodes.md；`events_nodes.rs` | 不写：对面的监听器在本端总线上 |
| 多跳：a 导入 b 的 `clock` 再导出给 c | `multihop.rs`、`websocket_multihop.rs`、`fault_links.rs` | 不写：b 的 `clock` 在 a 上全局可见，经 a 与 c 的 session 到 c |
| `host: true` + `rows: true` + 行 `peer:office/scanner` | nodes.md；`peer_rows.rs`、`node.rs` | 行 `office:scanner`，loader 调 `run` |

**决定 2：插件的配置归启动它的一方。**

- 对面本来就在运行的插件，配置在对面。
- 本端决定对面跑什么、用什么配置时，写行 `<连接行>:<插件>` 带 `config`，loader 调 `run`。
- 现在应用怎样拿配置（[应用接入](design-app-integration-2026-10-14.md) D2、D6，Kotlin 与 Swift 的 `RutisApp`）：应用只报带了哪些插件（`mount` 回复的 `plugins`），装哪个、用什么配置都由 rutis 配置里 `instanced` 分组的行给出，SDK 收到 `rows.v2` 的开始请求才 `apply`。这就是 `run`。
- 迁移：**应用与 SDK 不改**。`instanced` 分组里的 `phone:health` 行在每条应用连接的实例里变成一次 `run("health", config)`；使用它的本地行（`py:coach`）照旧在同一个实例里。

## 三、Channel

```rust
pub struct Channel {
    pub sender: Box<dyn Sender>,
    pub receiver: Box<dyn Receiver>,
    pub closer: Arc<dyn Closer>,
}

pub trait Sender: Send {
    fn send(&mut self, message: &[u8]) -> Result<(), ChannelError>;   // 背压时阻塞
    fn max_message(&self) -> Option<usize> { None }
}
pub trait Receiver: Send {
    fn recv(&mut self) -> Result<Option<Vec<u8>>, ChannelError>;      // Ok(None)：对面正常结束
}
pub trait Closer: Send + Sync {
    fn close(&self, why: Close);                                      // 幂等，唤醒 send / recv
}

pub enum Close { Done, Replaced, Rejected(String) }
```

- 沿用 `Sender` / `Receiver` / `Closer` 三分：发送与接收各在一个线程，关闭在任何线程。按消息、同步阻塞，与 `rpc.rs` 的 `open(channel, dispatch)` 现在的用法相同。
- **id 一律来自握手**：会话协议只留 endpoint 格式（协议版本 3），双方都在握手里报自己的 id。删掉 compat 格式：`rpc.rs` 的 `Format::Compat`（`rpc.rs:1208`）与 `COMPAT_CAPABILITIES`、调用 id 的 `rust:` / `node:` 前缀、`PROTOCOL = 2` 的本机握手、`link.rs:361` 的 `compat` 与 `name_far_end`。本机运行时由 `rutis-host/spawn` 用 `--id <运行时名>` 起，握手里就报这个名字。运行时期望的控制方（它的 `--peer`）仍由现有的握手检查（`rpc.rs:2482` 的 `Handshake::IdentityMismatch`）核对。
- 各语言运行时的本机模式改为 endpoint 格式（逐个核实过现状）：

  | 运行时 | 现在 | 要改的 |
  | --- | --- | --- |
  | Node（`node/rutis-runtime/src/runner.mjs`、`session.mjs`） | 本机默认 compat，`--format endpoint` 时用 endpoint（`runner.mjs:29`） | 只留 endpoint：去掉 `--format`，本机也要求 `--id`；删 `session.mjs` 的 compat 分支与 `node:` 前缀 |
  | Bun（`bun/rutis-bun/src/main.ts`） | 同 Node：`--format endpoint` 可选（`main.ts:107`、`:119`） | 同 Node |
  | Python（`python/rutis/rutis/__main__.py`、`peer.py`） | 本机只说 compat（`__main__.py:14`），`--id` 只用于网络通道 | 本机通道也按 `--id` 建 endpoint；删 `peer.py` 的 compat 握手 |
  | Go（`go/rutis/serve.go` 与 `peer` 包） | `parseArgs` 只给网络通道 endpoint（`serve.go:164`），`ServeConn` 说 compat（`serve.go:87`） | 本机通道也用 endpoint；删 compat |
  | Swift（`swift/rutis/Sources/Rutis/API/Serve.swift`） | 本机 `serve` 说 compat（`Serve.swift:58`），endpoint 只用于 `listen:` / `dial:` | 本机也用 endpoint；删 compat |
  | Kotlin（`internal/peer/Peer.kt`、`internal/Sessions.kt`、jvm `Serve.kt`） | `Peer.kt:33` 有 compat（版本 2）与 `compatOrigin`；`Sessions.kt:53` 按有无 endpoint 选格式；Android 应用只拨出（已是 endpoint） | jvm 的本机通道用 endpoint；删 compat 与 `compatOrigin` |

- 删 `ChannelInfo`：对方 id 由握手给出；`transport` 与 `label` 只用于诊断，改由连接插件自己的输出说。
- `Closer::close(&str)`、`replaced()`、`rejected(&str)` 合成 `close(Close)`。保留三种，因为 4002、4003 是协议里的关闭码，各语言的拨出方按它们决定重试（`bridge/dial-out-rules`）：

  | `Close` | WebSocket | 本机管道（子进程） | 内存 |
  | --- | --- | --- | --- |
  | `Done` | 关闭码 1000（宿主停止时 1001） | 关闭子进程的 stdin | 对端 `recv` 得到 `Ok(None)` |
  | `Replaced` | 关闭码 4002 | 同 `Done` | 对端 `recv` 得到 `Closed` |
  | `Rejected(r)` | 关闭码 4003，原因 `r` | 同 `Done` | 对端 `recv` 得到 `Rejected` |

- `ChannelError { Closed, Rejected }` 不变：`Rejected` 是对面以 4003 拒绝了本端。
- `ConnectError` 移出核心，放进 `rutis-host/dial` 内部：三类（可重试、认证被拒、不兼容）与重连数字（0.5 → 30 秒，认证被拒每 30 秒，不兼容停止，持续 60 秒后重置）不变，它们是各语言拨出方共用的约定。
- 交给 `Session::new` 之后，只有 session 关闭 Channel；交出之前连接插件可以自己关（例如升级时 token 不对）。

## 四、生命周期

### 4.1 Channel 与 session

一对一，同生同灭，不复用。

| 发生了什么 | 谁先知道 | 结果 |
| --- | --- | --- |
| 握手失败（协议版本、缺能力、超时 10 秒） | session | 对面先收到协议自己的错误帧，然后 `close(Done)`；session 以这个原因失败 |
| 对面先走（`recv` 返回 `Ok(None)` 或错误） | session | session 卸载：远程插件逆序卸载，进行中的调用失败一次 |
| 本地先走（连接插件卸载、行删掉、宿主停止） | 内核 | session 卸载：远程插件逆序卸载，然后 `close(Done)` |
| 重连 | 连接插件 | 新 Channel、新 session；旧 session 已经卸载 |

连接插件用 `ctx.plugin(...)` 返回的内核句柄（`FiberView::watch()`）等 session 卸载，再决定是否重连（dial 退避重连；spawn 按运行时的 `restart`；listen 什么都不做）。session 不重连。

### 4.2 顶替

- `rutis-host/spawn`、`rutis-host/dial` 同时只有一条 Channel：旧的卸载完才起新的，不发生顶替。
- 顶替只在 `rutis-host/listen` 内部。listen 按 id 记着自己的连接；同一个 id 的新连接握手完成后：
  1. 卸载旧 session（远程插件逆序卸载，清理全部完成），删掉旧的实例；
  2. 旧 Channel `close(Replaced)`（4002）；
  3. 为新连接建实例，实例里的行开始 `run`。
- 新连接在握手完成之前没有任何远程插件（远程插件只来自 `run` 与对面提供的服务，都在第 3 步之后），所以一条握手失败的连接不会影响旧的。
- 与 TLA 的对应：变异 `access-mut-accept-before-release` 把"接受新连接"挪到"释放旧连接"之前，模型找出反例；listen 里 1、2 → 3 的顺序就是它守的顺序。
- 名字不在宿主里记：spawn、dial、单台 listen 的名字是行 id（loader 本来要求唯一）；同一个监听器上两条 token 相同时启动报错（#702）。

### 4.3 远程插件映射到内核的生命周期

| 对面 | 本地的远程插件 `<id>:<插件>` |
| --- | --- |
| `run` 成功，或对面的插件开始提供服务 | 注册为 session 的子插件（`run` 的结果是那一行本身），`apply` 完成；它提供的服务以普通 `provide` 出现 |
| 运行失败（对面抛出） | 这个插件失败，错误带对面的名字、消息、`code`、`site`（0.18 §4.2） |
| 对面结束了它 | 卸载；使用它的服务的行按内核规则停下或等待 |
| 连接断开 | session 卸载，远程插件全部卸载 |

- 没有单独的"连接状态"：连着就是 session 在，断开就是 session 不在。"运行时状态 + 会话是否连着"两项代替 `LinkState`、`AccessEvent`、`RuntimeState`。
- 运行在对面的行（`gpu:embedder`）：对面不在时这一行等待（"waiting for gpu"），session 出现后 loader 调 `run`。`run` 之前先看 `mount` 回复的 `plugins`，没有时问 `rows.schema`（#768 的共用发现，协议里各运行时都这样报）；对面不含这个插件时这一行 `Unavailable`。
- `check_rows`：对面缺 `rows.v2` 与 `hosts` 能力时拒绝（行约定是协议的一部分）；错误文字去掉旧的版本号。

## 五、扩展

只有一个扩展点：写连接插件，拿到 `Channel`，挂 session。

### 5.1 Rust 连接插件

```rust
// 经中继连到一台运行时
async fn apply(ctx: &Ctx, config: Relay) -> Result<(), Error> {
    loop {
        let channel = relay::connect(&config.address, &config.token).await?;  // 自己的认证与传输
        let session = ctx.plugin(Session::new(channel));
        disposed(session.watch()).await;                 // 内核的 FiberView::watch()：等 session 卸载
        sleep(backoff.next()).await;
    }
}
```

### 5.2 任何语言的连接插件

```ts
export default definePlugin({
  name: 'relay',
  inject: ['sessions'],
  async apply(ctx, config: { url: string }) {
    const ws = new WebSocket(config.url)
    ws.binaryType = 'arraybuffer'
    await new Promise(ok => ws.addEventListener('open', ok, { once: true }))
    const near = ctx.sessions.open({
      send: m => ws.send(m),
      close: why => ws.close(why.kind === 'rejected' ? 4003 : why.kind === 'replaced' ? 4002 : 1000),
    })
    ws.onmessage = e => near.receive(new Uint8Array(e.data))
    ws.onclose = () => near.end({ kind: 'done' })
    ctx.effect(() => () => ws.close())
  },
})
```

- `receive` 在消息进入 session 的接收队列后完成；队列满（64 条，与内存通道相同）时等待，背压由此传回连接插件。
- 底下与 `Session::new` 是同一份实现：`sessions.open` 用 `far` 做出一个 `Sender` / `Closer`，用 `near` 的队列做出一个 `Receiver`。
- 代价：这种连接插件的每条消息多经过一次它所在的运行时进程。

### 5.3 动态加载

连接插件的加载方式与普通插件相同：rutis.json 里的一行、loader 的 `create`、#185 的 `loader` 服务。agent 运行中写的 TS / Python 连接插件经这些方式装上，`sessions.open` 建的 session 挂在它下面；卸载它，它的 session 与其下的远程插件全部卸载。

## 六、自带的连接插件与 rutis.json

三个插件在 rutis-host 里（不进 rutis-bridge），底层是 rutis-bridge 里的普通函数：`websocket::dial`、`websocket::listen`、`pipe::spawn`（测试用 `memory::pair`）。

| 插件 | 做什么 | 配置 |
| --- | --- | --- |
| `rutis-host/spawn` | `pipe::spawn` 以 `--id <运行时名>` 起本机运行时进程，挂 session；进程退出时按 `restart` 重起 | `runtimes.*` 现有的字段 |
| `rutis-host/dial` | `websocket::dial`，挂 session；session 卸载后按 `ConnectError` 的类别退避重连 | `address`；token 只认 `RUTIS_TOKEN_<行 id>`；`RUTIS_CA` |
| `rutis-host/listen` | `websocket::listen`：升级时按 token 比对（401 / 403 不变）；一台时每次一个 session，多台时每条连接一个 session 与一个实例（§4.2） | `listener`；多台时 `group`（下文）；token 同 dial |

- 宿主一侧只认 `RUTIS_TOKEN_<名字>`，不再退回公共的 `RUTIS_TOKEN`（`config.rs:912` 的兜底与 `config.rs:969` 起"谁在共用 `RUTIS_TOKEN`"的检查删掉）；运行时一侧的 `RUTIS_TOKEN` 不变。一个端口按 token 分给多个条目（现在 `Registrations` 做的事）与 `RUTIS_TOKEN_<名字>` 保留，因为它们有使用方（Swift、Kotlin 应用与各语言拨出方都这样连）；分发的代码在 `rutis-host/listen` 里。
- `runtimes.node` / `py` / `go` / `swift` / `bun` 等仍是 `rutis-host/spawn` 行的简写，不变。
- `runtimes.remote` 删掉：远程运行时与节点是同一种写法，直接写 `rutis-host/dial` 或 `rutis-host/listen` 的行。`language` 字段不再需要：远程的插件名在对面解析（`mount` 回复与 `rows.schema`）。
- 三个插件的配置 Schema 都写 `additionalProperties: false`（[配置 Schema](design-config-schema-2026-10-27.md) C3）。

```json
{
  "id": "main",
  "listen": [{ "name": "public", "address": "0.0.0.0:7443", "cert": "server.pem", "key": "server.key" }],
  "runtimes": { "py": { "project": "plugins" } },
  "rows": [
    { "id": "gpu", "name": "rutis-host/dial", "config": { "address": "wss://gpu.example.com:7443/rutis" } },
    { "id": "embedder", "name": "gpu:embedder" },
    { "id": "office", "name": "rutis-host/dial", "config": { "address": "wss://office.example.com/rutis" } },
    { "id": "scanner", "name": "office:scanner", "config": { "dpi": 300 } },
    { "id": "phone", "name": "rutis-host/listen", "config": { "listener": "public", "group": "phones" } },
    { "id": "phones", "group": true, "instanced": true, "config": [
      { "id": "health", "name": "phone:health" },
      { "id": "coach", "name": "py:coach", "inject": ["health"] }
    ] }
  ]
}
```

**多台设备（核实结果）**：listen 为每条连接用 loader 已有的 `create_instance` 建实例，在代码里核实过能直接做的部分与要补的部分：

- 能直接做：`Loader::create_instance(ctx, group)` 不拿操作锁，插件可以在自己的 `apply` 与之后调用；父级从 `ctx` 往上找（`instances.rs:83`）；`.with(value)` 把一个值交给实例里各行的工厂（`Build::value`），现在 `AcceptInstances` 就是这样把连接交进去的（`connections.rs:529`）；`remove_instance` 关闭实例。listen 用 `.with(SessionHandle)` 建实例，实例里的 `phone:health` 行从这个值拿到本实例的 session 调 `run`；它是实例里的一行，所以它提供的服务按已有的实例服务规则只在本实例可见。
- 做不到的："listen 放在 `instanced` 分组里面"不行：`instanced` 分组不由 reconcile 装载，只有建实例时它的行才运行（`instances.rs:3`），放在里面的 listen 永远不会起来去建第一个实例。
- 最小的补充：listen 放在分组旁边，用一个配置字段 `group` 说明为哪个 `instanced` 分组建实例。这是让 listen 知道分组的唯一信息，不加别的。

## 七、开发者看到什么

遵守 [诊断设计](design-diagnostics-2026-10-26.md) §4.5：输出里不出现 session、`rutis-bridge/session`、内部 id；行写行名，远程插件写 `<id>:<插件>`。

**输入**

- rutis.json：`runtimes.*`（本机运行时）；`rutis-host/dial`、`rutis-host/listen` 的行（远程运行时、设备、节点）；运行在对面的行写 `<连接行>:<插件>`。
- Rust：`Session::new(channel)`、`SessionHandle::{id, run}`；`websocket::{dial, listen}`、`pipe::spawn`、`memory::pair`。
- 其他语言：服务 `sessions` 的 `open(far)`。

**`run` 的输出**：前缀是连接行的 id，只用 connected / disconnected 两个词，原因写在括号里。

```text
gpu: connected
gpu: disconnected (gpu.example.com:7443: Connection refused); retrying in 2.1s
gpu: disconnected (auth-rejected: upgrade refused with 403 Forbidden); retrying in 30s
gpu: disconnected (incompatible: the far end speaks protocol 1, this side 3); stopped
phone: connected phone-1
phone: disconnected phone-1 (connection lost)
phone: disconnected phone-1 (replaced by a new connection)
office: connected
```

**`rutis-host status`**：`connections:` 一节，每个连接行一行，连着的写它注册了哪些远程插件：

```text
connections:
  gpu: connected (gpu:embedder)
  phone: phone-1 (phone-1:health), phone-2 (phone-2:health)
  office: connected (office:clock, office:scanner)
```

**出错**

| 情况 | 文字 |
| --- | --- |
| 对面不含行要的插件 | `embedder: unresolved: entry "embedder" rejected: runtime gpu: this connection does not contain plugin embedder`（#768） |
| 对面的服务本地已有同名 | `office:weather: weather is provided here by row weather: not registered from office` |
| 对面缺行约定 | `the gpu runtime lacks rows.v2 and hosts: a runtime speaking the row contract is required`（去掉现在的"for Node, @arcships/rutis-runtime 0.3.0 or later"） |
| 0.18 的宿主或运行时 | 握手时按协议版本拒绝（`the far end speaks protocol 2, this side 3: upgrade the far end`），不重试 |
| 0.18 的远程写法（`runtimes.remote`、`rutis-bridge/peer` 行及其字段） | 没有专门的提示：`runtimes.remote` 是 rutis-host 配置不认识的字段，`rutis-bridge/peer` 是不认识的插件名，按 0.18 的规则报错 |
| 对面运行插件时抛出 | 远程插件 `<id>:<插件>` 失败，按 0.18 §4.2 打印对面的错误 |

## 八、删除与收回清单

旧写法在 0.19 直接删，不标弃用，不做专门转换或提示。

| 名字 | 在哪 | 为什么 | 谁受影响 |
| --- | --- | --- | --- |
| `Transport`、`Dial`、`transport_key` | rutis-bridge `lib.rs` | 底层直接产出 `Channel` | 嵌入 rutis-bridge 的 Rust 程序；测试；文档里"实现 Transport"的承诺 |
| `Embedding.transports`、`Embedding.identity` | rutis-host `host.rs` | 自己的连接方式与凭据写在自己的连接插件里 | 嵌入 rutis-host 的 Rust 程序；e2e `fixture-host` |
| `runtimes.remote`（`name`、`language`、`dial`、`listen`、`instances`、`transport`）与 `connect()` 的字段组合校验 | rutis-host `config.rs`、`host.rs` | 远程运行时直接写连接插件的行 | 写了它的 rutis.json；持久化样本 0.10–0.18 |
| `Identity`、`IdentityPlugin`、`identity_key`、`StaticIdentity`、`Credential`、`Presented` | rutis-bridge `identity.rs` | 认证在连接插件里 | 测试；嵌入方 |
| `Admission`、`AdmissionPlugin`、`admission_key`、`SharedToken`、`Rejection` | rutis-bridge `identity.rs` | listen 按 token 比对；id 来自握手 | rutis-host；测试 |
| `Revocations`、`Revocable` | rutis-bridge `identity.rs` | 生产无调用方；断开现有连接靠停掉那一行 | `revocation.rs`；guarantees 三行；TLA 模型 |
| `Registration`、`Registered`、`RegistrationError`、`Registrations`、`Ticket`、`Refusal`、`Deliver` | rutis-bridge `registration.rs` | 按 token 分发并入 `rutis-host/listen`；注册代次没有了 | WebSocket 监听；`memory_mux.rs`；TLA 模型 |
| `AccessPointPlugin`、`AccessPointConfig`、`AccessEvent`、`Accept`、`Accepted`、`Release` | rutis-bridge `access_point.rs` | 回调形状的交接 | rutis-host；loader `connections.rs`；测试 |
| `LinkPlugin`、`LinkConfig`、`LinkState`、`Connect`、`Failure`、`Retry`、`protocol()` | rutis-bridge `link.rs` | 去连与重连移到 `rutis-host/dial`，监听移到 `rutis-host/listen`；`protocol()` 无调用方 | rutis-host；测试；`examples/smoke.rs` |
| `PeerPlugin`、`PeerHandle`、`Features` | rutis-bridge `compose.rs` | 组合插件；`PeerPlugin::new` 恒返回 `Ok` | rutis-host；loader；测试 |
| `Peer`、`peer_key`、`Handler`、`Offered`、`Offers`、`family()` | rutis-bridge `peer.rs` | 并入 session；`family()` 无调用方 | bridge 内部；测试 |
| `ExportPlugin`、`ImportPlugin` 与 `export`、`import` | rutis-bridge `services.rs`、`events/link.rs` 的名单部分 | 决定 1 | 节点配置；测试；`check.rs` 的提示文字 |
| `HostPlugin`、`PluginCatalog`、`StaticCatalog`、`Described`、`Installed`、`ServiceKeys` 与 `host`、`rows` | rutis-bridge `host.rs` | 对面跑什么只走 `run` | 节点配置；`peer_rows.rs`、`node.rs` |
| `peer:<对端>/<插件>` 行、`rutis-bridge/peer` 行名（及其 `runtime`、`transport`、`identity` 字段）、`PeerRows`、`PeerResolver`、`PeerRowsPlugin`、`LoaderCatalog`、`register_peer_node`、`node_schema` | rutis-loader `peer.rs` | 远程插件只有 `<id>:<插件>`；节点是连接插件的行 | 节点配置；持久化样本 |
| `RuntimeSession`、`runtime_session_key`、`RuntimeAccessPlugin` | rutis-bridge `runtime/` | 服务形状的交接 | loader；测试 |
| `RuntimePlugin::remote` | rutis-bridge `runtime/plugin.rs` | 远程与本机走同一个 session | rutis-host；loader；测试 |
| `LocalRuntime` | rutis-bridge `runtime/local.rs` | 起进程移到 `rutis-host/spawn` | rutis-host；测试 |
| `RuntimeInstances`、`AcceptInstances`、`RuntimeConnection` | loader `runtime/connections.rs` | listen 直接用 `create_instance` + `with` | rutis-host；loader 测试；rutis-migrate 的名字表 |
| `LinkState`、`AccessEvent`、`RuntimeState` 的远程部分 | bridge、rutis-host `status.rs` | 合成"运行时状态 + 会话是否连着" | rutis-host 的输出代码 |
| `ChannelInfo`、`Closer::replaced`、`Closer::rejected` | rutis-bridge `channel/mod.rs` | §三 | 各底层；测试 |
| `ConnectError`（移出 `channel`） | rutis-bridge `channel/mod.rs` | 放进 `rutis-host/dial` 内部 | 底层与 dial |
| `MemoryPlugin`、`MemoryTransport`、`LocalPlugin`、`WebSocketPlugin`、`WebSocketTransport` | `transport/*` | 只为提供 `Transport#<kind>` 存在 | 测试；rutis-host |
| compat 格式：`Format::Compat`、`COMPAT_CAPABILITIES`、`rust:` / `node:` 调用前缀、`PROTOCOL = 2`、`link.rs` 的 `compat` 与 `name_far_end` | rutis-bridge `session/rpc.rs`、`session/mod.rs`、`link.rs`、`runtime/process.rs:919` | 所有运行时都用 endpoint 格式，id 来自握手 | 本机运行时的启动参数；fuzz 目标；测试 |
| 各语言运行时的 compat 路径与 Node、Bun 的 `--format` | `node/rutis-runtime`、`bun/rutis-bun`、`python/rutis`、`go/rutis`、`swift/rutis`、`kotlin/rutis` | 同上 | 各语言运行时与 SDK（0.19 一起发布） |
| 宿主一侧退回公共 `RUTIS_TOKEN` 的兜底 | rutis-host `config.rs` | 每个名字一个 token；兜底只带来"谁在共用"的额外检查 | 只设了 `RUTIS_TOKEN` 的宿主 |
| `ServiceCatalog::share_registered_only` 与它的 `by_name` 开关 | rutis-loader `catalog.rs` | 0.12 之前的旧行为开关，生产无调用方 | 三个测试（`check.rs`、`scope.rs`、`marked_services.rs`）；open-host、rust-host 指南 |
| 测试 `revocation.rs` 全部、`an_id_revoked_while_its_connection_takes_over_is_not_accepted`、Registration 代次的单元测试 | rutis-bridge | 随吊销与 Registration 删，不改名续用 | — |

## 九、改前改后

现在的行数用 `wc -l` 在 `3d19634f` 上实测（含文件里的单元测试）；"公开项"是以 `pub fn|struct|enum|trait|type|const|use|mod` 开头的行数（含 impl 里的公开方法）。改后的数字是**估计**。

| 文件 | 现在行数 | 现在公开项 | 改后行数（估计） |
| --- | ---: | ---: | ---: |
| rutis-bridge `lib.rs` | 177 | 29 | 50 |
| `channel/mod.rs` | 165 | 13 | 110 |
| `access_point.rs` | 607 | 16 | 0 |
| `compose.rs` | 270 | 6 | 0 |
| `identity.rs` | 564 | 27 | 40（只留 `fingerprint`） |
| `registration.rs` | 409 | 13 | 0 |
| `link.rs` | 751 | 15 | 0 |
| `peer.rs` | 382 | 14 | 0 |
| `services.rs` | 543 | 6 | 0 |
| `host.rs` | 359 | 10 | 0 |
| `runtime/access.rs` | 109 | 2 | 0 |
| `runtime/plugin.rs` | 667 | 23 | 350 |
| `runtime/local.rs` | 417 | 22 | 0 |
| `runtime/mod.rs` | 142 | 14 | 110 |
| `events/link.rs` | 795 | 2 | 400 |
| `transport/websocket/mod.rs` | 262 | 8 | 150 |
| `transport/memory/mod.rs` | 409 | 11 | 200 |
| `transport/local/mod.rs` | 206 | 13 | 150 |
| `session/rpc.rs`（删 compat） | 3267 | 49 | 3120 |
| 新增 `session/plugin.rs`（Session、SessionHandle、`sessions`、两边的服务与监听器） | — | — | 550 |
| **rutis-bridge 小计** | **10501** | **293** | **5230** |
| rutis-loader `runtime/connections.rs` | 550 | 10 | 0 |
| `runtime.rs` | 1621 | 14 | 1400 |
| `peer.rs` | 998 | 14 | 0 |
| `runtime/discover.rs` | 53 | 0 | 53 |
| `catalog.rs` | 528 | 23 | 505 |
| **rutis-loader 小计** | **3750** | **61** | **1958** |
| rutis-host `host.rs` | 2089 | 27 | 1650 |
| `config.rs` | 1434 | 42 | 1100 |
| `status.rs` | 979 | 18 | 780 |
| 新增 `connect.rs`（spawn、dial、listen） | — | — | 650 |
| **rutis-host 小计** | **4502** | **87** | **4180** |
| **合计** | **18753** | **441** | **约 11370** |

- 本表比上一版多算了 `session/rpc.rs` 与 `catalog.rs`（本轮删 compat 与 `share_registered_only` 涉及它们）。
- rutis-bridge 根上导出的名字：现在 59 个，改后约 7 个（`Channel`、`Close`、`PeerId`、`Session`、`SessionHandle`、`RowHandle`、`thrown`），估计。
- session 的公开 API：`Session::new`、`SessionHandle::id`、`SessionHandle::run`、`sessions.open` 四个。
- 生命周期路径从 4 条变 1 条，交接形状从 2 种变 1 种，准入、顶替、状态各从 3 份变 1 份（顶替只在 listen 里）；节点与远程运行时没有专门的代码。
- 各语言运行时的入口（删 compat 后，估计）：Node `runner.mjs` 1162 → 1140、`session.mjs` 810 → 740；Bun `main.ts` 203 → 195；Python `__main__.py` 352 → 340、`peer.py` 1903 → 1820；Go `serve.go` 295 → 270；Swift `Serve.swift` 263 → 240；Kotlin `Peer.kt` 1438 → 1360、jvm `Serve.kt` 217 → 205。
- 测试：只测被删接口的文件删掉或改写，约 5500 行（`access_point.rs` 584、`revocation.rs` 798、`link.rs` 547、`fault_links.rs` 959、`memory_mux.rs` 730、`runtime_instances.rs` 703、`peer_rows.rs` 556、`node.rs` 1011 的大部分、`remote_runtimes.rs` 的 `runtimes.remote` 部分等），估计。

## 十、已合并的 PR 怎么处理

**结论：不 revert，在新实现里替换。** 理由：

- 混乱的主体（四条路径、两种形状、三份准入与状态）早于 0.19；revert #768、#791 只回到 0.18 的同样状态，什么都没解决。
- 两个 PR 里仍有新架构要用的部分（下表"保留"一列），revert 再实现会让 CI、迁移指南、指南各多改一轮。

| PR | 保留 | 被替换或删掉 |
| --- | --- | --- |
| #768（R1 loader） | `discover.rs`（`mount` 回复的 `plugins` 与 `rows.schema` 兜底）；`Process::check_rows`（错误文字去掉版本号）；缺插件 `Unavailable` 与它的文字；`remote_modes.rs` 的用例（改为经 spawn / dial / listen 跑） | 两个调用点合成一个按 session 找的解析器 |
| #791（R2 rutis-host） | 一台 listen 与同一监听器的 token 检查（#702）；`status` 的 `connections:` 一节与 0.18 的 status 通道；`control::serve` 的 `Report` 参数；《迁移到 0.19》这份文件（内容按 §十一 重写） | `runtimes.remote` 的写法（`dial` / `listen` / `instances` / `transport` / `language`）与它的校验；`Embedding.transports`；`host.rs` 里的连接搭法；`runtime <名>: link …` 的输出（改为 §七）；`remote_runtimes.rs` 按新写法重写 |
| #799 | — | 已关闭，不重开。它带的 guarantees §15 计数重算（`2d817aa3`）在改 guarantees 的实施 PR 里重做 |
| 分支 `claude/remote-drop-transport`（未推送，`9a5543cc`） | 去掉 `transport` 字段、`Embedding.transports` 的改动 | 并入 §十三 的 S4（`runtimes.remote` 整个删掉，它的改动只用到一部分），不单独推送 |

## 十一、兼容与《迁移到 0.19》

不做兼容。不变的只有：会话协议的 endpoint 格式；应用代码（Kotlin、Swift 应用不改代码，但要用 0.19 的 SDK 重新构建）；rutis.json 的 `format` 仍为 1；`runtimes.node` / `py` / `go` / `swift` / `bun` 等本机运行时的写法。

《迁移到 0.19》的"远程运行时"一节重写，只写新写法与对照：

1. **远程运行时与节点**：

   | 0.18 | 0.19 |
   | --- | --- |
   | `runtimes.remote` 一条 + 带 `runtime` 的 `rutis-bridge/peer` 行，`dial` | 一行 `{ "id": "gpu", "name": "rutis-host/dial", "config": { "address": "<地址>" } }` |
   | 同上，`listen` | 一行 `rutis-host/listen`，`"listener": "<监听器>"` |
   | `runtimes.remote` 的 `listen` + `instances` | 一行 `rutis-host/listen`，`"group": "<instanced 分组>"` |
   | 节点行 `rutis-bridge/peer` 的 `dial` / `listen` | 同上两行 |
   | `export`、`import`（服务与事件） | 不写：服务两边都全局可见，事件一条总线 |
   | `host: true` + `rows: true` + 行 `peer:office/scanner` | 行 `office:scanner` |
   | `transport`、`identity`、`Embedding.transports`、`Embedding.identity` | 自己的连接方式与凭据写成连接插件（§五），经 `Embedding.builtins` 注册 |

2. **自建连接**："实现 `Transport` 挂到 `Embedding.transports`"改为"交出一条 `Channel`：写一个连接插件，调 `Session::new(channel)`；其他语言用 `sessions.open`"。
3. **Rust API**："rutis、rutis-loader、rutis-bridge 的 Rust API 都不变"改为对照表：

   | 0.18 | 0.19 |
   | --- | --- |
   | `LinkPlugin` + `PeerPlugin { runtime }` + `RuntimePlugin::remote` | 连接插件里 `Session::new(websocket::dial(...)?)`，行 `<名>:<插件>` |
   | `AccessPointPlugin` + `RuntimeInstances` | `rutis-host/listen`，或自己的连接插件 + `create_instance(...).with(handle)` |
   | `ExportPlugin` / `ImportPlugin` / `HostPlugin` | 不需要 |
   | `Identity` / `Admission` / `Revocations` | 连接插件自己的认证；id 来自握手 |
   | `Transport` | `websocket::dial` / `websocket::listen` / `pipe::spawn` 直接给出 `Channel` |
   | `Closer::replaced()` / `rejected(r)` | `close(Close::Replaced)` / `close(Close::Rejected(r))` |
   | `rutis_bridge::ConnectError` | `rutis-host/dial` 内部 |

4. **一起升级**：0.19 的宿主与 0.18 及更早的运行时、SDK 不能相遇（compat 格式删掉，握手时按协议版本拒绝）；宿主与各语言运行时、SDK 要一起升到 0.19。跨版本保证只在不含不兼容改动的相邻版本之间成立。
5. **token**：宿主一侧只认 `RUTIS_TOKEN_<名字>`；只设了 `RUTIS_TOKEN` 的宿主改为按名字设。运行时一侧的 `RUTIS_TOKEN` 不变。
6. **`ServiceCatalog::share_registered_only` 删除**：Rust 嵌入里调用它的，去掉这一行（0.12 起默认按名字共享）。
7. **持久化样本**：0.8–0.18 的样本里有 `runtimes.remote`、`rutis-bridge/peer` 行与 `peer:` 行，0.19 不再读。样本里删掉这些写法（期望同步），加 `rejected/` 用例，错误是 0.18 规则给出的原样文字（不认识的字段、不认识的插件名）；guarantees 里持久化样本那一条写明 0.19 的这一处例外。
8. **输出**：连接的状态行改为 §七 的样子（前缀是行 id，connected / disconnected）；`status` 的 `connections:` 每个连接后列出远程插件。

## 十二、要同步改的文档

| 文档 | 改什么 |
| --- | --- |
| `docs/guide/nodes.md` / `.en.md` | 重写：远程运行时与节点都是 `rutis-host/dial` / `rutis-host/listen` 的行；`export` / `import` / `host` / `rows` / `runtime` 一表与"节点之间的事件"的名单规则删掉，写"服务两边都全局可见，事件一条总线"；"在别的节点上运行插件"改为 `office:scanner`；"实现一个 `Transport`"改为"写一个连接插件"；"安全"一节的吊销钩子删掉，写"断开现有连接靠停掉那一行" |
| `docs/guide/rutis-host.md` / `.en.md` | `runtimes.remote` 一节删掉，换成三个连接插件的行；启动错误列表同步；`Embedding` 的 `transports`、`identity` 删除；"run 打印什么"按 §七 |
| `docs/guide/rust-host.md` / `.en.md` | `PeerPlugin`、`RuntimePlugin::remote` 的例子改为连接插件 + `Session::new` |
| `docs/guide/swift-app.md`、`kotlin-app.md`（两种语言） | rutis 一侧的配置改为 `rutis-host/listen` + `group` 与 `rutis-host/dial`；应用代码不变 |
| `docs/guide/open-host.md`、`rust-host.md`（两种语言）与 `design-host-writes-plugins-2026-10-27.md` §九 | 删掉 `share_registered_only` 的提及 |
| 各语言运行时的指南（`typescript-plugin`、`bun-plugin`、`python-plugin`、`go-plugin`、`swift-plugin`、`kotlin-app`）与 `.agents/skills/rutis-new-runtime/references/runtime-contract.md` | 本机启动参数一律带 `--id`；只有 endpoint 格式；删掉 compat 与 `--format` |
| `docs/guide/cordis.md`、`plugin-api.md`、`bun-plugin.md` 等提到节点、`peer:`、`runtimes.remote` 的地方 | 逐处改为新写法 |
| `.agents/skills/rutis-remote-plugin/SKILL.md` 与 `references/nodes.md`、`rutis-json.md`、`plugin-api.md` | 同指南；"实现 Transport 挂到 Embedding.transports"改为"交出一条 Channel / 写连接插件"，加 `sessions.open` 的最小例子 |
| `.agents/skills/rutis-new-runtime/references/plugin-api.md` | 让已有运行时远程可用的一节：运行时一侧不变，rutis 一侧写 `rutis-host/dial` / `listen` 的行 |
| `docs/migration-0.19.md` / `.en.md` | §十一 |
| `docs/guarantees.md` / `.en.md` | `bridge/cross-version-meetings` 收窄为只在不含不兼容改动的相邻版本之间成立，写明 0.19 与 0.18 不保证相遇、宿主与各语言运行时要一起升到 0.19；`events/old-peer-refused` 同步（0.18 及更早都在握手时被拒）；删：`bridge/revocation-closes-only-its-connections`、`bridge/revocation-refuses-reconnects`、`host/env-tokens-not-revocable`、`events/link-export-import`、`events/node-config`、`events/no-multi-hop`；`host/remote-runtimes` 换成三个连接插件的一行；`loader/runtime-connection-instances` 改写为 listen + `group`；`loader/peer-rows` 改写为 `<id>:<插件>` 行经 `run`；`events/node-links` 改写为"对面的监听器是本端总线上的条目"；`bridge/dial-out-rules` 里 4003 的来源改为 listen 与握手检查；新增 listen 的顶替顺序一行；持久化样本一行写明 0.19 例外；每行的测试名随测试改；§15 计数重算 |
| `specs/remote-lease/`（README 两份、两个 `.tla`、配置、`expected.txt`） | 随吊销与 Registration 删掉：吊销动作、注册代次与移交复核，性质 `RevokedStaysOut`、`RevocationScoped`、`NoStaleHandOver`，配置 `access-revoke-same`、`access-revoke-distinct`、`access-mut-revoke-cuts-all`、`access-mut-route-by-id`、`access-mut-no-gen-check`、`access-registration`，`RemoteLease.tla` 里的吊销远端；`access-mut-accept-before-release` 留作 listen 顶替顺序的变异。listen 的新代码写出来后按它建一个最小模型（同 id 顶替：先卸旧再接新），替换 `AccessPoint.tla` |
| `crates/rutis-bridge/README.md`、`lib.rs` 的模块说明、根 `README` | 模块介绍改为 Channel、session、底层函数 |
| 旧设计稿 `design-remote-runtime-consistency-2026-10-26.md` / `.en.md` | 头部加一行"被 [远程连接：只留一个会话插件](design-remote-session-2026-10-28.md) 取代" |

## 十三、实施

先加后删，每个 PR 合并后 main 可用、CI 通过。

| PR | 内容 | 完成条件 | 依赖 |
| --- | --- | --- | --- |
| S1 rutis-bridge：Channel 与底层函数 | `Close`、`close(Close)`；删 `ChannelInfo`；新增 `websocket::dial`、`websocket::listen`、`pipe::spawn`、`memory::pair`（现有 `Transport` 实现改为调用它们） | 现有测试不改用例通过（只改到 `close` 的调用）；新单元测试：三种 `Close` 在三种底层上的表现（4002、4003、stdin 关闭） | — |
| S2 rutis-bridge：session 插件 | `Session::new`、`SessionHandle::{id, run}`；远程插件 `<id>:<插件>`；两边的服务全局可见（借与提供）、对面的监听器在本端总线上（运行时已有的那份，推广到对面是 rutis 节点）；服务 `sessions` 与 `open(far)`；旧的 link / 接入点不动 | 测试：四种结束；两个 rutis 内核经内存 Channel 互相使用服务、同名按内核规则被拒、事件互达；Node 与 Python 测试插件经 `sessions.open` 跑通 `run` | S1 |
| R1–R6 各语言运行时：本机用 endpoint 格式 | 每种语言一个 PR（Node、Bun、Python、Go、Swift、Kotlin），按 §三 的表：本机通道也按 `--id` 建 endpoint；compat 暂时保留，宿主未改时照常运行 | 每种语言的现有测试通过；新测试：本机通道以 `--id` 握手报出名字 | — |
| S2b 本机运行时改走 endpoint，删 compat | 宿主起本机运行时的代码改为传 `--id`、用 endpoint 格式；删 `Format::Compat`、`COMPAT_CAPABILITIES`、`rust:` / `node:` 前缀、`PROTOCOL = 2`、`link.rs` 的 `compat`；各语言删掉 compat 路径与 `--format` | 全部运行时测试与 `runtime_conformance` 在 endpoint 格式上通过；fuzz 目标改为 endpoint；`grep -i compat` 在 `crates/rutis-bridge/src/session` 与各语言运行时的会话代码下无结果 | R1–R6、S1 |
| S3 rutis-loader：统一解析器 | 行 `<名>:<插件>` 按连接行找 session 调 `run`，实例里从 `Build::value` 拿本实例的 session；旧路径并存 | `remote_modes.rs` 的用例在 session 上跑：spawn、dial、listen、listen + `group` 四种接法缺插件文字逐字相同；实例服务的现有用例通过 | S2 |
| S4 rutis-host：三个连接插件 | `rutis-host/spawn`、`dial`、`listen`（Schema `additionalProperties: false`；listen 的 token 分发、同 id 顶替、`group` 建实例）；`runtimes.*` 展开成 spawn；删 `runtimes.remote`、`Embedding.transports`、`Embedding.identity`（并入 drop-transport 分支）与宿主一侧的 `RUTIS_TOKEN` 兜底；状态合成两项，§七的输出；持久化样本与 `rejected/` 用例 | `remote_runtimes.rs`、`dial_in.rs`、`kotlin.rs` 按新写法端到端通过；节点端到端（两个 rutis-host 互相使用服务、事件、`office:scanner`）；`dial_conformance` 各语言对 `rutis-host/listen` 通过（4002、4003 照旧）；`persisted` 测试通过；e2e `fixture-host` 编译；#702 的 token 检查用例通过 | S3、S2b |
| S5 删除 | 删 §八 全部条目（含 `share_registered_only`）与只测它们的测试；cross_version e2e 里与 0.18 相遇的场景删掉（0.19 与 0.18 不保证相遇）；删 TLA 模型里吊销与 Registration 的部分及 §十二 列出的配置；按 S4 的 listen 代码建最小模型 | `grep` §八 的名字在 `crates/` 下无结果；`cargo test --workspace` 与 clippy 通过；新模型与 `access-mut-accept-before-release` 跑通，`expected.txt` 更新 | S4 |
| S6 文档 | §十二 全部条目；《迁移到 0.19》§十一 | 两种语言内容一致；文档里不再出现 `Transport`、`Embedding.transports`、`Revocations`、`export` / `import`、`rutis-bridge/peer`、`runtimes.remote` 的承诺；guarantees 每行的测试名都存在 | S5（与 S4 同时可见的行为可以随 S4 先改指南） |
