# 远程连接：只留一个会话插件

[English](design-remote-session-2026-10-28.en.md)

状态：定稿，未实现。日期：2026-10-28。基准：`main` `3d19634f`。排期：0.19。
Issue：[#695](https://github.com/arcships/rutis/issues/695)（实施 [#696](https://github.com/arcships/rutis/issues/696) loader、[#698](https://github.com/arcships/rutis/issues/698) rutis-host）。依据：#695 的 2026-10-28 两条评论。
与旧设计的关系：取代 [远程运行时一致性](design-remote-runtime-consistency-2026-10-26.md) 的实施部分（§四"rutis-host 内部"一列、§五、§七、§八）；它的"缺插件一律 `Unavailable`"保留，`runtimes.remote` 的写法删掉（§六）。[决策记录](decision-remote-runtime-consistency-2026-10-26.md) 里"不新增交接接口、不重写 link 与接入点"一条由本稿推翻，理由见 §一。

范围外：会话协议的帧（不改；只删 compat 格式，§三）；各语言运行时与 SDK 只改 §三 与 §十三 写到的部分；认证放开（自定义凭据交给连接插件自己做，不在核心里开接口）；运行中吊销（见 §八 `Revocations`）；loader 的行模型与 `instanced` 分组本身。

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
  - 对面自己装着、提供着服务的插件（对面是 rutis 节点，或自己装插件的设备应用）。
- **服务**：经 session 两边都全局可见。对面插件 `inject` 的服务从本端借（现在本机运行时就是这样）；对面提供的服务由它的远程插件在本端 `provide`。同名按内核规则：后注册的被拒绝。经同一个 session 来的服务不再发回这个 session。做法：session 监听内核的 `ServiceChanged`（服务提供与撤下时发），把本端可共享的服务（`host_key(<名>)`、不在实例里、不是 `isolate` 的、提供者不是平台插件）经 `services.announce` 发给对面，提供者是本 session 自己的远程插件的不发；对面发来的全部收下。`services.announce` 带 `plugin` 字段（提供它的插件名，rutis 节点发提供者所在的行 id），本端注册为远程插件 `<对面 id>:<plugin>`。
- **事件**：一条平的总线。对面的监听器是本端总线上的条目，与现在的运行时相同。
- **远程插件 ↔ 本地插件**：与普通插件相同（`inject` / `provide`），生命周期全部用内核原有的。
- **顶替**：只在 `rutis-host/listen` 内部（§4.2）。session 核心不记 id，不做顶替。

### 2.4 API 入口

Rust（编进宿主的连接插件）：

```rust
pub struct Channel { /* §三 */ }

/// 本端在握手里报的 id：宿主放进根 ctx（rutis-host 用 rutis.json 的 `id`，默认 `main`）。
pub struct LocalId(pub PeerId);

impl Session {
    /// 先握手（10 秒），不注册任何东西。
    pub async fn handshake(ctx: &Ctx, channel: Channel) -> Result<Greeted, Error>;
    /// "握手 + 装上"的简写，是一个插件。
    pub fn new(channel: Channel) -> Self;
}
/// 握完手、还没装上的会话；`ctx.plugin(greeted)` 装上之后才注册对面的插件、开始服务互通。
/// 没装上就放下时以 `Close::Done` 关闭 Channel。
impl Greeted {
    pub fn id(&self) -> &str;
    pub fn handle(&self) -> SessionHandle;
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

- 握手分出来是为了 `rutis-host/listen` 的顶替（§4.2）：listen 握完手拿到 `greeted.id()`，先卸掉同 id 的旧会话，再装上新的。spawn、dial 直接用 `Session::new`。
- 本端 id 来自根 ctx 的 `LocalId`；取不到时握手前就失败：`the host provides no id for its sessions`。rutis-bridge 不设默认值。
- 对面是 rutis 节点时，节点一侧的 session 在自己的 ctx 里找 loader 提供的可运行插件目录：有就回答 `run`（`mount` / `rows.*`），没有就答"不含这个插件"。`Session::new` 不另加参数。

session 的公开 API 是 `Session::{handshake, new}`、`Greeted::{id, handle}`、`SessionHandle::{id, run}`、`sessions.open`，加上 `LocalId`。`run` 留下的理由：让对面运行一个插件是会话协议里的一步（`rows.v2`），对面是语言运行时或应用时只有这一条路；对面是 rutis 节点时也用它，不另开一条。

### 2.5 "以后"的结构

```text
rutis-host
 ├─ rutis-host/spawn  py        ── pipe::spawn --id py ─▶ Channel ─▶ session py
 │                                                                         └─ py:coach …（run）
 ├─ rutis-host/dial   gpu       ── websocket::dial ────▶ Channel ─▶ session gpu
 │                                                                         └─ gpu:embedder（run）
 ├─ rutis-host/listen phone     ── websocket::listen ──▶ 每条连接一条 Channel
 │     ├─ session phone-1   └─ phone-1:health（设备自己装的）
 │     └─ session phone-2   └─ phone-2:health
 ├─ rutis-host/dial   office    ── websocket::dial ────▶ Channel ─▶ session office
 │                                                                         └─ office:clock（office 提供着的）
 └─ agent 运行中写的 TS 插件 relay ── sessions.open(far) ──▶ session（id 由对面报）
```

本机运行时、远程运行时、设备、节点都是同一种形状；没有节点或远程运行时专用的代码。

### 2.6 节点互联与配置的决定

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
- 现在应用怎样拿配置（[应用接入](design-app-integration-2026-10-14.md) D2、D6，Kotlin 与 Swift 的 `RutisApp`）：应用只报带了哪些插件（`mount` 回复的 `plugins`），装哪个、用什么配置都由 rutis 配置里 `instanced` 分组的行给出，SDK 收到 `rows.v2` 的开始请求才 `apply`。宿主不再为每条连接建实例（决定 3），这条路对多台设备不再成立。
- 改为：设备上的应用自己装它带的插件，配置写在应用里（`RutisApp` 的每个插件可带配置，不带时用 Schema 的默认值）；连上后经现有的 `services.announce` 帧（节点之间已经用它）报出它们提供的服务，宿主注册为 `<id>:<插件>`。这是 Kotlin、Swift SDK 在 0.19 的改动（§十三 S4）；应用接入设计的 D2、D6 随之改写。
- 一台、宿主自己连的运行时（`rutis-host/spawn`、`dial`、单台 `listen` 后面的语言运行时）照旧由宿主写 `<连接行>:<插件>` 行、走 `run`。

**决定 3：按设备区分是设备上插件自己的事。**

- `rutis-host/listen` 只为每条通过的连接建一个 session，对面的插件注册为普通插件 `<id>:<插件>`；它们提供的服务全局可见，同名按内核规则冲突（后来的被拒，并说明原因）。宿主不为每条连接建实例。
- 要每台设备各算一份的逻辑，放在设备上；宿主上的插件要处理多台，由它自己按设备区分，或由设备上的插件各用不同的服务名。平台不替插件做隔离。
- loader 已有的 `instanced` 分组照旧是通用功能，远程不再专门用它。
- 0.18 的"宿主上 `py:coach` 每台设备一份"（Swift、Kotlin 应用指南的例子）在 0.19 不再支持。改法：把 coach 放到设备上（应用里的一个插件），或者 coach 自己按设备区分（例如设备上的插件以 `health-<设备>` 为名提供服务，coach 按名字找）。

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
- **id 一律来自握手**：会话协议只留 endpoint 格式，双方都在握手里报自己的 id。endpoint 协议版本从 3 升到 4：0.18 的运行时与节点也说 endpoint 版本 3，只删 compat 挡不住它们；升到 4 后，0.18 连上时在握手里被拒，消息照协议现有的写法说明升级哪一端（`rpc.rs:1277` 的 `version_mismatch`）。删掉 compat 格式：`rpc.rs` 的 `Format::Compat`（`rpc.rs:1208`）与 `COMPAT_CAPABILITIES`、调用 id 的 `rust:` / `node:` 前缀、`PROTOCOL = 2` 的本机握手、`link.rs:361` 的 `compat` 与 `name_far_end`。本机运行时由 `rutis-host/spawn` 用 `--id <运行时名>` 起，握手里就报这个名字。运行时期望的控制方（它的 `--peer`）仍由现有的握手检查（`rpc.rs:2482` 的 `Handshake::IdentityMismatch`）核对。
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
  1. 卸载旧 session（远程插件逆序卸载，清理全部完成）；
  2. 旧 Channel `close(Replaced)`（4002）；
  3. 新 session 开始注册对面的插件。
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
| `rutis-host/listen` | `websocket::listen`：升级时按 token 比对（401 / 403 不变）；每条通过的连接一个 session，同 id 顶替（§4.2） | `listener`；token 同 dial |

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
    { "id": "phone", "name": "rutis-host/listen", "config": { "listener": "public" } }
  ]
}
```

`phone` 这一行之后，每台连进来的设备装着的插件注册为 `<设备 id>:<插件>`（`phone-1:health`）；设备不必在 rutis.json 里列出。

## 七、开发者看到什么

遵守 [诊断设计](design-diagnostics-2026-10-26.md) §4.5：输出里不出现 session、`rutis-bridge/session`、内部 id；行写行名，远程插件写 `<id>:<插件>`。

**输入**

- rutis.json：`runtimes.*`（本机运行时）；`rutis-host/dial`、`rutis-host/listen` 的行（远程运行时、设备、节点）；运行在对面的行写 `<连接行>:<插件>`。
- Rust：`LocalId`（宿主放进根 ctx）、`Session::handshake(&ctx, channel)` 与 `ctx.plugin(greeted)`、`Session::new(channel)`、`SessionHandle::{id, run}`；`websocket::{dial, listen}`、`pipe::spawn`、`memory::pair`。
- 其他语言：服务 `sessions` 的 `open(far)`。

**`run` 的输出**：前缀是连接行的 id，只用 connected / disconnected 两个词，原因写在括号里。

```text
gpu: connected
gpu: disconnected (gpu.example.com:7443: Connection refused); retrying in 2.1s
gpu: disconnected (auth-rejected: upgrade refused with 403 Forbidden); retrying in 30s
gpu: disconnected (incompatible: this endpoint speaks rutis.3, the dialer offered rutis.4: upgrade this endpoint (PyPI rutis)); stopped
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
| 宿主没有在根 ctx 提供 `LocalId` | 会话失败：`the host provides no id for its sessions` |
| 对面的服务本地已有同名 | `office:weather: weather is provided here by row weather: not registered from office` |
| 对面缺行约定 | `the gpu runtime lacks rows.v2 and hosts: a runtime speaking the row contract is required`（去掉现在的"for Node, @arcships/rutis-runtime 0.3.0 or later"） |
| 0.18 与 0.19 相遇 | 握手时（WebSocket 在子协议那一步）拒绝，不重试，说明升级哪一端。0.19 的宿主起 0.18 的本机运行时（它说本机的协议 2）：`incompatible session: the far end speaks protocol 2, this side 4: upgrade the far end`；0.18 拨号 0.19 的监听方，拨号方写出 `this endpoint speaks rutis.4, the dialer offered rutis.3: upgrade the dialer`；0.19 拨号 0.18 的监听方，监听方答 `this endpoint speaks rutis.3, the dialer offered rutis.4: upgrade this endpoint (<它的包>)`（例如 `(PyPI rutis)`）。例外：0.18 的宿主起 0.19 的本机运行时不传 `--id`，运行时在握手之前只写一行 `a local channel needs --id <endpoint>: fd:3; a host older than rutis 0.19 started this runtime: upgrade the host` 并退出 |
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
| `HostPlugin`、`Described`、`Installed`、`ServiceKeys` 与 `host`、`rows`（`PluginCatalog`、`StaticCatalog` 留下：节点一侧的 session 用它回答 `run`） | rutis-bridge `host.rs` | 对面跑什么只走 `run` | 节点配置；`peer_rows.rs`、`node.rs` |
| `peer:<对端>/<插件>` 行、`rutis-bridge/peer` 行名（及其 `runtime`、`transport`、`identity` 字段）、`PeerRows`、`PeerResolver`、`PeerRowsPlugin`、`register_peer_node`（`LoaderCatalog` 留下，搬出 `peer.rs`）、`node_schema` | rutis-loader `peer.rs` | 远程插件只有 `<id>:<插件>`；节点是连接插件的行 | 节点配置；持久化样本 |
| `RuntimeSession`、`runtime_session_key`、`RuntimeAccessPlugin` | rutis-bridge `runtime/` | 服务形状的交接 | loader；测试 |
| `RuntimePlugin::remote` | rutis-bridge `runtime/plugin.rs` | 远程与本机走同一个 session | rutis-host；loader；测试 |
| `LocalRuntime` | rutis-bridge `runtime/local.rs` | 起进程移到 `rutis-host/spawn` | rutis-host；测试 |
| `RuntimeInstances`、`AcceptInstances`、`RuntimeConnection` | loader `runtime/connections.rs` | 决定 3：宿主不为每条连接建实例 | rutis-host；loader 测试；rutis-migrate 的名字表 |
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
| 新增 `connect.rs`（spawn、dial、listen） | — | — | 600 |
| **rutis-host 小计** | **4502** | **87** | **4130** |
| **合计** | **18753** | **441** | **约 11320** |

- 本表比上一版多算了 `session/rpc.rs` 与 `catalog.rs`（本轮删 compat 与 `share_registered_only` 涉及它们）。
- rutis-bridge 根上导出的名字：现在 59 个，改后约 7 个（`Channel`、`Close`、`PeerId`、`Session`、`SessionHandle`、`RowHandle`、`thrown`），估计。
- session 的公开 API：`Session::{handshake, new}`、`Greeted::{id, handle}`、`SessionHandle::{id, run}`、`sessions.open`，加上 `LocalId`。
- 生命周期路径从 4 条变 1 条，交接形状从 2 种变 1 种，准入、顶替、状态各从 3 份变 1 份（顶替只在 listen 里）；节点与远程运行时没有专门的代码。
- 各语言运行时的入口（删 compat 后，估计）：Node `runner.mjs` 1162 → 1140、`session.mjs` 810 → 740；Bun `main.ts` 203 → 195；Python `__main__.py` 352 → 340、`peer.py` 1903 → 1820；Go `serve.go` 295 → 270；Swift `Serve.swift` 263 → 240；Kotlin `Peer.kt` 1438 → 1360、jvm `Serve.kt` 217 → 205。Kotlin、Swift SDK 加上应用自己装插件与 `services.announce`（S4），各约多 150 行。
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
| 分支 `claude/remote-drop-transport`（未推送，`9a5543cc`） | 去掉 `transport` 字段、`Embedding.transports` 的改动 | 并入 §十三 的 S2（删 `Transport`），不单独推送 |

## 十一、兼容与《迁移到 0.19》

不做兼容。不变的只有：会话协议的 endpoint 格式；rutis.json 的 `format` 仍为 1；`runtimes.node` / `py` / `go` / `swift` / `bun` 等本机运行时的写法。

《迁移到 0.19》的"远程运行时"一节重写，只写新写法与对照：

1. **远程运行时与节点**：

   | 0.18 | 0.19 |
   | --- | --- |
   | `runtimes.remote` 一条 + 带 `runtime` 的 `rutis-bridge/peer` 行，`dial` | 一行 `{ "id": "gpu", "name": "rutis-host/dial", "config": { "address": "<地址>" } }` |
   | 同上，`listen` | 一行 `rutis-host/listen`，`"listener": "<监听器>"` |
   | `runtimes.remote` 的 `listen` + `instances` 与 `instanced` 分组里的行 | 一行 `rutis-host/listen`；分组里的远程插件行删掉，改由应用自己装插件（决定 2）；宿主上每台设备一份的行（`py:coach`）按决定 3 改 |
   | 节点行 `rutis-bridge/peer` 的 `dial` / `listen` | 同上两行 |
   | `export`、`import`（服务与事件） | 不写：服务两边都全局可见，事件一条总线 |
   | `host: true` + `rows: true` + 行 `peer:office/scanner` | 行 `office:scanner` |
   | `transport`、`identity`、`Embedding.transports`、`Embedding.identity` | 自己的连接方式与凭据写成连接插件（§五），经 `Embedding.builtins` 注册 |

2. **自建连接**："实现 `Transport` 挂到 `Embedding.transports`"改为"交出一条 `Channel`：写一个连接插件，调 `Session::new(channel)`；其他语言用 `sessions.open`"。
3. **Rust API**："rutis、rutis-loader、rutis-bridge 的 Rust API 都不变"改为对照表：

   | 0.18 | 0.19 |
   | --- | --- |
   | `LinkPlugin` + `PeerPlugin { runtime }` + `RuntimePlugin::remote` | 连接插件里 `Session::new(websocket::dial(...)?)`，行 `<名>:<插件>` |
   | `AccessPointPlugin` + `RuntimeInstances` | `rutis-host/listen`，或自己的连接插件 |
   | `ExportPlugin` / `ImportPlugin` / `HostPlugin` | 不需要 |
   | `Identity` / `Admission` / `Revocations` | 连接插件自己的认证；id 来自握手 |
   | `Transport` | `websocket::dial` / `websocket::listen` / `pipe::spawn` 直接给出 `Channel` |
   | `Closer::replaced()` / `rejected(r)` | `close(Close::Replaced)` / `close(Close::Rejected(r))` |
   | `rutis_bridge::ConnectError` | `rutis-host/dial` 内部 |

4. **应用（Kotlin、Swift）**：插件的配置写进 `RutisApp`，应用连上后自己装插件（决定 2）；rutis 一侧 `instanced` 分组里的 `<运行时>:<插件>` 行删掉。宿主上"每台设备一份"的行（`py:coach`）不再支持，按决定 3 改：放到设备上，或由它自己按设备区分。
5. **一起升级**：0.19 在握手时拒绝 0.18 及更早的宿主、运行时与 SDK（endpoint 协议版本 4 对 3），并提示升级哪一端；宿主与各语言运行时、SDK 要一起升到 0.19。跨版本保证只在不含不兼容改动的相邻版本之间成立。
6. **token**：宿主一侧只认 `RUTIS_TOKEN_<名字>`；只设了 `RUTIS_TOKEN` 的宿主改为按名字设。运行时一侧的 `RUTIS_TOKEN` 不变。
7. **`ServiceCatalog::share_registered_only` 删除**：Rust 嵌入里调用它的，去掉这一行（0.12 起默认按名字共享）。
8. **持久化样本**：0.8–0.18 的样本里有 `runtimes.remote`、`rutis-bridge/peer` 行与 `peer:` 行，0.19 不再读。样本里删掉这些写法（期望同步），加 `rejected/` 用例，错误是 0.18 规则给出的原样文字（不认识的字段、不认识的插件名）；guarantees 里持久化样本那一条写明 0.19 的这一处例外。
9. **输出**：连接的状态行改为 §七 的样子（前缀是行 id，connected / disconnected）；`status` 的 `connections:` 每个连接后列出远程插件。

## 十二、要同步改的文档

| 文档 | 改什么 |
| --- | --- |
| `docs/guide/nodes.md` / `.en.md` | 重写：远程运行时与节点都是 `rutis-host/dial` / `rutis-host/listen` 的行；`export` / `import` / `host` / `rows` / `runtime` 一表与"节点之间的事件"的名单规则删掉，写"服务两边都全局可见，事件一条总线"；"在别的节点上运行插件"改为 `office:scanner`；"实现一个 `Transport`"改为"写一个连接插件"；"安全"一节的吊销钩子删掉，写"断开现有连接靠停掉那一行" |
| `docs/guide/rutis-host.md` / `.en.md` | `runtimes.remote` 一节删掉，换成三个连接插件的行；启动错误列表同步；`Embedding` 的 `transports`、`identity` 删除；"run 打印什么"按 §七 |
| `docs/guide/rust-host.md` / `.en.md` | `PeerPlugin`、`RuntimePlugin::remote` 的例子改为连接插件 + `Session::new` |
| `docs/guide/swift-app.md`、`kotlin-app.md`（两种语言） | rutis 一侧改为 `rutis-host/listen` 与 `rutis-host/dial` 的行；应用在 `RutisApp` 里给插件配置、自己装插件；"每台设备一份 `py:coach`"的例子删掉，换成决定 3 的两种改法 |
| `docs/design-app-integration-2026-10-14.md` / `.en.md` | D2、D6 改写：设备上的插件由应用自己装、自己配置；§五"每条连接一个实例"删掉 |
| `docs/guide/open-host.md`、`rust-host.md`（两种语言）与 `design-host-writes-plugins-2026-10-27.md` §九 | 删掉 `share_registered_only` 的提及 |
| 各语言运行时的指南（`typescript-plugin`、`bun-plugin`、`python-plugin`、`go-plugin`、`swift-plugin`、`kotlin-app`）与 `.agents/skills/rutis-new-runtime/references/runtime-contract.md` | 本机启动参数一律带 `--id`；只有 endpoint 格式；删掉 compat 与 `--format` |
| `docs/guide/cordis.md`、`plugin-api.md`、`bun-plugin.md` 等提到节点、`peer:`、`runtimes.remote` 的地方 | 逐处改为新写法 |
| `.agents/skills/rutis-remote-plugin/SKILL.md` 与 `references/nodes.md`、`rutis-json.md`、`plugin-api.md` | 同指南；"实现 Transport 挂到 Embedding.transports"改为"交出一条 Channel / 写连接插件"，加 `sessions.open` 的最小例子 |
| `.agents/skills/rutis-new-runtime/references/plugin-api.md` | 让已有运行时远程可用的一节：运行时一侧不变，rutis 一侧写 `rutis-host/dial` / `listen` 的行 |
| `docs/migration-0.19.md` / `.en.md` | §十一 |
| `docs/guarantees.md` / `.en.md` | `bridge/cross-version-meetings` 收窄为只在不含不兼容改动的相邻版本之间成立，写明 0.19 在握手时拒绝 0.18 并提示升级、宿主与各语言运行时要一起升到 0.19；`events/old-peer-refused` 同步（0.18 及更早都在握手时被拒）；删：`bridge/revocation-closes-only-its-connections`、`bridge/revocation-refuses-reconnects`、`host/env-tokens-not-revocable`、`events/link-export-import`、`events/node-config`、`events/no-multi-hop`；`host/remote-runtimes` 换成三个连接插件的一行；删 `loader/runtime-connection-instances`；`loader/peer-rows` 改写为 `<id>:<插件>` 行经 `run`；`events/node-links` 改写为"对面的监听器是本端总线上的条目"；`bridge/dial-out-rules` 里 4003 的来源改为 listen 与握手检查；新增 listen 的顶替顺序一行；持久化样本一行写明 0.19 例外；每行的测试名随测试改；§15 计数重算 |
| `specs/remote-lease/`（README 两份、两个 `.tla`、配置、`expected.txt`） | 随吊销与 Registration 删掉：吊销动作、注册代次与移交复核，性质 `RevokedStaysOut`、`RevocationScoped`、`NoStaleHandOver`，配置 `access-revoke-same`、`access-revoke-distinct`、`access-mut-revoke-cuts-all`、`access-mut-route-by-id`、`access-mut-no-gen-check`、`access-registration`，`RemoteLease.tla` 里的吊销远端；`access-mut-accept-before-release` 留作 listen 顶替顺序的变异。listen 的新代码写出来后按它建一个最小模型（同 id 顶替：先卸旧再接新），替换 `AccessPoint.tla` |
| `crates/rutis-bridge/README.md`、`lib.rs` 的模块说明、根 `README` | 模块介绍改为 Channel、session、底层函数 |
| 旧设计稿 `design-remote-runtime-consistency-2026-10-26.md` / `.en.md` | 头部加一行"被 [远程连接：只留一个会话插件](design-remote-session-2026-10-28.md) 取代" |

## 十三、实施

### 13.1 规则

- **按闭环划范围**：一个 PR 删一样东西，就在同一个 PR 里把它在所有 crate、各语言 SDK、测试、文档（指南、`.agents/skills`、guarantees、《迁移到 0.19》、设计稿的引用）、示例（`examples/`、`swift/examples`、`kotlin/rutis/android-sample`）、模板（`rutis-host new` 的模板）、持久化样本、`specs/` 里的用处全部改完。每个 PR 的描述里列出它的闭环清单，合并前逐项核对；`grep` 被删的名字在整个仓库里无结果（历史设计稿与发布说明除外）。
- **不加兼容**：不得为了让测试或 CI 通过而加转发、兜底分支、旧名、feature 开关，或"暂时保留旧格式"。因删掉的行为而失败的测试，在同一个 PR 里改写或删除；改不了就停下来报告，不绕过。
- **先加后删**只用于新东西：新增的部分（新插件、新函数、新的 SDK 能力）可以先合并、暂时没有调用方；被替换的旧东西在替换它的那个 PR 里删完，不留两条路径。
- 每个 PR 合并后 main 可用、CI 通过。
- 与 [平台清理设计稿](https://github.com/arcships/rutis/tree/claude/design-platform-cleanup)（分支 `claude/design-platform-cleanup`）碰同一处（内核、rutis-bridge、rutis-loader、各语言 SDK）的，以闭环为单位决定合并先后，见那份稿子。

### 13.2 PR

**C1 删 `Revocations` / `Revocable`**（独立，可随时合并）
- 闭环清单：`identity.rs` 里的两个类型与 `Revocable` 的包装；`tests/revocation.rs`；`registration.rs` 与 `identity.rs` 里只测吊销的单元测试；guarantees 的 `bridge/revocation-closes-only-its-connections`、`bridge/revocation-refuses-reconnects`、`host/env-tokens-not-revocable`；`specs/remote-lease` 的吊销动作、`RevokedStaysOut`、`RevocationScoped`、`access-revoke-same`、`access-revoke-distinct`、`access-mut-revoke-cuts-all`、`RemoteLease.tla` 的吊销远端与 `expected.txt`；`guide/nodes.md` "安全"一节的吊销钩子（两种语言）；`design-remote-runtime-dial` 等设计稿里指向它的链接改为说明已删除。
- 完成：`grep -rn "Revocations\|Revocable"` 只剩历史设计稿；TLA `run.sh` 通过。

**C2 删 `ServiceCatalog::share_registered_only`**（独立）
- 闭环清单：`catalog.rs` 的方法与 `by_name` 开关；`rutis-loader/tests` 的 `check.rs`、`scope.rs`、`marked_services.rs`；`runtime.rs` 的文档注释；`guide/open-host.md`、`guide/rust-host.md`（两种语言）；`design-host-writes-plugins-2026-10-27.md` §九（两种语言）；《迁移到 0.19》§十一 第 7 条。
- 完成：`grep` 无结果；loader 全部测试通过。

**C3 宿主一侧只认 `RUTIS_TOKEN_<名字>`**（独立）
- 闭环清单：`rutis-host/src/config.rs` 的兜底（约 912 行）与"谁在共用 `RUTIS_TOKEN`"的检查（约 969 行起）及其单元测试；`rutis-host/tests` 里只设 `RUTIS_TOKEN` 的用例改为按名字设；e2e 与 CI 脚本里给宿主设的 `RUTIS_TOKEN`；`guide/nodes.md`、`guide/rutis-host.md`、Swift / Kotlin 应用指南、`.agents/skills/rutis-remote-plugin`；guarantees 里提到兜底的行；《迁移到 0.19》§十一 第 6 条。运行时一侧的 `RUTIS_TOKEN` 不动。
- 完成：`rutis-host` 全部测试通过；文档里宿主一侧不再出现单独的 `RUTIS_TOKEN`。

**S1 Channel：`Close` 与底层函数**
- 加：`websocket::dial`、`websocket::listen`、`pipe::spawn`、`memory::pair`（现有的传输实现改为调用它们）。
- 删并闭环：`Closer::replaced`、`Closer::rejected`、`close(&str)` 换成 `close(Close)`，`ChannelInfo` 删掉——所有实现与调用方（`transport/*`、`link.rs`、`access_point.rs`、`registration.rs`、`session/rpc.rs`、测试替身 `channel/testing`、`memory_mux.rs` 等全部测试、fuzz 目标）同一个 PR 改完。
- 完成：三种 `Close` 在三种底层上的单元测试（4002、4003、stdin 关闭）；`grep "ChannelInfo\|fn replaced\|fn rejected"` 无结果；全部测试通过。
- 依赖：—

**S2 删 `Transport`**
- 删并闭环：`Transport`、`Dial`、`transport_key`、`MemoryPlugin`、`MemoryTransport`、`LocalPlugin`、`WebSocketPlugin`、`WebSocketTransport`；`link.rs`、`access_point.rs`、`LocalRuntime` 改为直接调用 S1 的函数；rutis-host 的 `transport` 字段与 `Embedding.transports`（并入分支 `claude/remote-drop-transport`）；节点行的 `transport` 字段与持久化样本里的它（去掉并加 `rejected/` 用例，guarantees 写明 0.19 例外）；e2e `fixture-host`；全部用到它们的测试（bridge、loader、host）；`guide/rutis-host.md`、`guide/nodes.md`、`.agents/skills/rutis-remote-plugin` 两份 references 里"实现 Transport 挂到 Embedding.transports"的承诺改为"交出一条 Channel"；《迁移到 0.19》里 `transport` 一条删掉；guarantees `host/remote-runtimes` 去掉 `transport`。
- 完成：`grep "Transport\b\|transport_key\|Embedding.transports"` 无结果；全部测试通过。
- 依赖：S1

**R1–R6 各语言运行时：加 endpoint 的本机模式**（每种语言一个 PR：Node、Bun、Python、Go、Swift、Kotlin）
- 只加：本机通道按 `--id` 建 endpoint（§三 的表）。不碰 compat 的删除，所以合并后各运行时两种格式都能说，宿主仍用 compat；这不是兼容层，是 S2b 要用的新能力。
- 完成：每种语言的现有测试通过；新测试：本机通道以 `--id` 用 endpoint 握手并报出这个名字。
- 依赖：—

**S2b 删 compat 格式**（一个 PR，闭环）
- 宿主切到 endpoint：起本机运行时的代码传 `--id`、用 endpoint 格式。
- 删并闭环：`Format::Compat`、`COMPAT_CAPABILITIES`、`rust:` / `node:` 前缀、`PROTOCOL = 2`、`link.rs` 的 `compat` 与 `name_far_end`、`runtime/process.rs:919` 的 compat；六种 SDK 里的 compat 路径（Node `session.mjs` 与 `runner.mjs` 的 `--format`、Bun `main.ts` 的 `--format`、Python `peer.py` 与 `__main__.py`、Go `peer` 包与 `serve.go`、Swift `Serve.swift`、Kotlin `Peer.kt` 的 compat 与 `compatOrigin`、`Sessions.kt`），各自的测试；fuzz 目标；`runtime_conformance`；`ENDPOINT_PROTOCOL` 从 3 升到 4（`session/mod.rs:18`）与六种 SDK 各自的协议版本号（Node、Bun、Python、Go、Swift、Kotlin 的版本常量与握手检查）同步升到 4，各自的版本不符测试；cross_version e2e 里与 0.18 相遇的场景改为断言 0.19 在握手时拒绝 0.18 并提示升级；guarantees `bridge/cross-version-meetings` 收窄、`events/old-peer-refused` 同步；各语言运行时指南与 `rutis-new-runtime` 的 `runtime-contract.md`；《迁移到 0.19》§十一 第 5 条。
- 完成：`grep -i compat` 在 `crates/rutis-bridge/src/session` 与六种 SDK 的会话代码下无结果；全部运行时测试在 endpoint 格式上通过。
- 依赖：R1–R6 全部合并。

**S3 session 插件与统一解析器**（新增，先加）
- 加：`rutis-bridge/session`（`Session::new`、`SessionHandle::{id, run}`、服务 `sessions` 与 `open(far)`、两边的服务与监听器）；loader 里 `<名>:<插件>` 按连接行找 session 调 `run` 的解析器（只对 session 生效，现有的远程路径在 S4 删）；`guide/plugin-api.md` 与 `.agents/skills/rutis-remote-plugin` 加 `sessions.open` 一节。
- 完成：四种结束；两个 rutis 内核经内存 Channel 互相使用服务、同名按内核规则被拒、事件互达；Node 与 Python 测试插件经 `sessions.open` 跑通 `run`。
- 依赖：S1、S2b

**S4 远程运行时与设备改用连接插件**（闭环）
- 加：`rutis-host/spawn`、`rutis-host/dial`、`rutis-host/listen`（§六）；Kotlin、Swift SDK 的应用自己装插件、经 `services.announce` 报服务（决定 2，原 A1、A2，与宿主停止驱动应用插件必须在同一个 PR，否则同一个插件会被装两次）。
- 删并闭环：`runtimes.remote` 与 `connect()` 的校验；`AccessPointPlugin` 一族与 `Accept` / `Accepted` / `Release`；`Admission`、`AdmissionPlugin`、`SharedToken`；`RuntimeSession`、`runtime_session_key`、`RuntimeAccessPlugin`、`RuntimePlugin::remote`、`LocalRuntime`；loader 的 `RuntimeInstances`、`AcceptInstances`、`RuntimeConnection` 与 rutis-migrate 的名字表；`AccessEvent` 与 `RuntimeState` 的远程部分，§七 的输出；SDK 里等 `rows.v2` 装应用插件的路径；测试 `access_point.rs`、`runtime_instances.rs`、`remote_rows.rs`、`remote_modes.rs`、`leases.rs`、`dial_in.rs`、`kotlin.rs`、`remote_runtimes.rs`、`dial_conformance.rs`、Swift / Kotlin SDK 的应用测试按新写法改写或删除；示例 `swift/examples/HealthApp`、`kotlin/rutis/android-sample`；持久化样本去掉 `runtimes.remote`，加 `rejected/` 用例，guarantees 写明 0.19 例外；guarantees `host/remote-runtimes`、`loader/runtime-connection-instances`、`loader/remote-runtime-rows`、`bridge/dial-out-rules`；`specs/remote-lease` 的 `AccessPoint.tla` 换成按 listen 代码建的最小模型，删 `access-mut-route-by-id`、`access-mut-no-gen-check`、`access-registration`，保留 `access-mut-accept-before-release`；指南 `rutis-host.md`、`nodes.md` 里远程运行时的部分、Swift / Kotlin 应用指南、`design-app-integration` 的 D2、D6、`.agents/skills`；《迁移到 0.19》§十一 第 1（远程运行时部分）、3、4、8、9 条；模板里的 `runtimes.remote`。
- 完成：上列名字 `grep` 无结果；端到端：spawn、dial、单台 listen、多台设备（两台连同一个 listen，同名服务后来的被拒并说明原因）；`dial_conformance` 各语言对 `rutis-host/listen` 通过（4002、4003 照旧）；`persisted` 测试通过；TLA 新模型与 `access-mut-accept-before-release` 通过。
- 依赖：S2、S3

**S5 节点改用连接插件**（闭环）
- 删并闭环：`rutis-bridge/peer` 行名与 `peer:<对端>/<插件>` 行（改为 `rutis-host/dial` / `listen` 行与 `<连接行>:<插件>`）；`export`、`import`、`host`、`rows`、`runtime`、`identity` 字段；`PeerPlugin`、`PeerHandle`、`Features`；`LinkPlugin` 一族（`ConnectError` 移进 `rutis-host/dial`）；`Peer`、`peer_key`、`Handler`、`Offered`、`Offers`、`family()`；`ExportPlugin`、`ImportPlugin`、`events/link.rs` 的名单部分；`HostPlugin` 与目录类型；`Identity` 一族、`Registration` 一族、`Embedding.identity`；loader 的 `peer.rs`；`LinkState` 与 link 的状态行；测试 `link.rs`、`fault_links.rs`、`memory_mux.rs`、`node.rs`、`node_conformance.rs`、`multihop.rs`、`websocket_*.rs`、`peer_rows.rs`、`instance_peers.rs`、`cordis_node.rs`、`events_nodes.rs`、`events_conformance` 的节点部分、`examples/smoke.rs`、`check.rs` 的 `import` 提示；Cordis 节点（`@arcships/rutis-runtime/bridge`）里对应的写法；持久化样本去掉 `rutis-bridge/peer` 与 `peer:` 行，加 `rejected/` 用例；guarantees `events/node-links`、`events/link-export-import`、`events/node-config`、`events/no-multi-hop`、`loader/peer-rows`；`specs/remote-lease` 里注册代次的剩余部分与 `NoStaleHandOver`；指南 `nodes.md`、`rust-host.md`、`cordis.md` 与其余提到节点的指南、`.agents/skills`；《迁移到 0.19》§十一 第 1（节点部分）、2 条。
- 完成：上列名字 `grep` 无结果；两个 rutis-host 的节点端到端（互相使用服务、同名被拒、事件、`office:scanner`）；`cargo test --workspace` 与 clippy 通过。
- 依赖：S4

合并顺序：C1、C2、C3、R1–R6 随时；S1 → S2；R1–R6 全部 → S2b；S1、S2b → S3；S2、S3 → S4 → S5。没有单独的"删除"或"文档"PR：每样东西在替换它的 PR 里删完、文档改完。
