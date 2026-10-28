# 远程连接：只留一个会话插件

[English](design-remote-session-2026-10-28.en.md)

状态：草案，未实现。日期：2026-10-28。基准：`main` `3d19634f`。排期：0.19。
Issue：[#695](https://github.com/arcships/rutis/issues/695)（实施 [#696](https://github.com/arcships/rutis/issues/696) loader、[#698](https://github.com/arcships/rutis/issues/698) rutis-host）。依据：#695 的 2026-10-28 两条评论。
与旧设计的关系：取代 [远程运行时一致性](design-remote-runtime-consistency-2026-10-26.md) 的实施部分（§四"rutis-host 内部"一列、§五、§七、§八）；它的配置写法（`runtimes.remote` 的 `dial` / `listen` / `instances`）与"缺插件一律 `Unavailable`"保留。[决策记录](decision-remote-runtime-consistency-2026-10-26.md) 里"不新增交接接口、不重写 link 与接入点"一条由本稿推翻，理由见 §一。

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

### 2.1 插件

| 插件 | 归属 | 作用 |
| --- | --- | --- |
| `rutis-bridge/session` | rutis-bridge，核心里唯一与远程有关的插件 | 在一条 `Channel` 上把对面的插件接进本地内核；每条 Channel 一个 |
| 远程插件 `<id>:<插件名>` | session 的子插件，自动注册 | 对面的一个插件在本地的样子；远程插件只有这一种名字 |
| 连接插件 | 核心之外，谁写都行 | 拿到一条 `Channel`，挂一个 session |
| └ `rutis-host/spawn`、`rutis-host/dial`、`rutis-host/listen` | rutis-host 自带 | 起本机进程、去连、监听；本机运行时、远程运行时、节点都用它们 |

### 2.2 结构

```text
分组（可选，带 isolate 时就是这条连接能借的范围）
 ├─ 连接插件（rutis-host/dial、rutis-host/listen、rutis-host/spawn，或别人写的）
 │   └─ rutis-bridge/session             每条 Channel 一个
 │       ├─ <id>:weather                 对面的插件，自动注册
 │       └─ <id>:llm
 └─ 本端放在这条连接上的插件（只对节点有意义，§2.3）
```

### 2.3 关系

- **连接插件 → session**：`ctx.plugin(Session::new(channel))`。Channel 关闭，session 卸载；session 卸载，Channel 关闭。
- **session → 远程插件**：对面的插件在这条连接上开始运行就注册为子插件 `<id>:<插件>`，结束就卸载；session 卸载时它们全部随之卸载。"在这条连接上运行"只有两种来源：
  - 本端让它运行：行 `<id>:<插件>`，loader 找到 session 调 `run`（对面是语言运行时或应用），或用对面借来的 `loader` 服务建行（对面是 rutis 节点，§2.6 决定 1）；
  - 对面自己放上来：对面是 rutis 节点时，它的连接插件所在分组里的其他行（不含 `<id>:<插件>` 行）。
- **借服务（按需租借）**：对面的插件按 `inject` 声明要什么，session 就从连接插件所在作用域里借什么，与现在本机运行时一样。能借的范围就是连接插件所在作用域里看得到的服务；要限制，在连接插件的行（或它所在的分组）上写 `isolate`。
- **事件**：对面的插件监听什么，session 就把连接插件所在作用域里发出的、这个名字的事件发过去；它们发出的事件出现在本端的总线上，作用域是它们自己的。事件不受 `isolate` 影响（现有规则，[插件 API](guide/plugin-api.md)"作用域"），要限制用监听器上已有的过滤。
- **远程插件 ↔ 本地插件**：远程插件提供的服务以普通 `provide` 出现，本地行照常 `inject`；生命周期全部用内核原有的。
- **顶替**：只在同一个连接插件下的 session 之间（§4.2）。

### 2.4 API 入口

Rust（编进宿主的连接插件）：

```rust
pub struct Channel { /* §三，含可选的 id */ }

impl Session {
    pub fn new(channel: Channel) -> Self;
}
impl SessionHandle {
    /// 对方在握手里报的 id（Channel 带 id 时两者相同）。
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
- `open` 建的 session 挂在调用它的插件下面：这个插件卸载，它的 session 全部卸载。不带 id，以 hello 为准（与多台设备相同）。

只有这四个名字。曾考虑过的三个选项都用已有的东西做了：

| 需求 | 不加的 | 用什么做 |
| --- | --- | --- |
| id 不对的连接不能先顶掉同 id 的正常连接；设备的 id 不能顶掉本机运行时 | 回调 `admit` | `Channel` 的可选 `id`（连接层担保的 id）；顶替只在同一个连接插件下发生（§三、§4.2） |
| 节点之间限定给对方哪些服务、事件 | 名单 `export` | 对面按 `inject` 借，范围是连接插件所在作用域，限制用 `isolate`（2.3） |
| 让对面节点在本机启动插件 | `host(catalog)` 与 `PluginCatalog` 参数 | 对面的插件（或对面的 session）借本机的 `loader` 服务（#185 定下的保留名服务）；作用域里看得到就能用，不想让对面用就在连接行上 `isolate: ["loader"]` |

`run` 留下的理由：对面可能是语言运行时或应用而不是 rutis 内核，那里没有 `loader` 服务可借；`run` 是会话协议里让对面运行一个插件的那一步（`rows.v2`）。对面是 rutis 节点时，`run` 就是借对面的 `loader` 建一行。

### 2.5 "以后"的结构

```text
rutis-host
 ├─ rutis-host/spawn  py        ── pipe::spawn ─────────▶ Channel(id py)  ─▶ session py
 │                                                                         └─ py:coach …（run）
 ├─ rutis-host/dial   gpu       ── websocket::dial ────▶ Channel(id gpu) ─▶ session gpu
 │                                                                         └─ gpu:embedder（run）
 ├─ rutis-host/listen worker    ── websocket::listen ──▶ 每条连接一条 Channel（不带 id）
 │     ├─ session worker-1   └─ worker-1:chunker …
 │     └─ session worker-2   └─ worker-2:chunker …
 ├─ 分组 office（isolate: ["loader"]）
 │     ├─ rutis-host/dial office ─ websocket::dial ─▶ Channel(id office) ─▶ session office
 │     │                                                                   └─ office:clock（对面放上来的）
 │     └─ weather                本端放在这条连接上的插件，对面看到 main:weather
 └─ agent 运行中写的 TS 插件 relay ── sessions.open(far) ──▶ session（id 由对面报）
```

本机运行时、远程运行时、设备、节点都是同一种形状；没有节点专用的代码。

### 2.6 两个待定项的决定

**决定 1：节点互联去掉 `export` 和 `import`，用作用域。**

- 规则：对面的插件 `inject` 什么就借什么，能借的是本端连接插件所在作用域里看得到的服务；对面监听什么就发什么。要限制，在连接行或它所在的分组上写 `isolate`。哪些插件在这条连接上运行，见 2.3。
- 依据：本机运行时一直就是这样（按 `inject` 借，作用域决定范围），节点与它一致后，节点不需要专门的代码。
- 逐个核对 [连接节点](guide/nodes.md) 与测试里的用法：

  | 现在的用法 | 在哪 | 改成 |
  | --- | --- | --- |
  | `export: ["weather"]` + 对端 `import: ["weather"]`：对端的本地行用这个节点的 `weather` | nodes.md "在 link 上做什么"；`node.rs`、`websocket_soak.rs`、`cordis_node.rs` | 对端把用 `weather` 的行放进它的连接分组：这些行在这条连接上运行，`inject: ["weather"]` 从这个节点借。或者这个节点把提供 `weather` 的行放进自己的连接分组，对端看到 `main:weather`，它的服务在对端全局可见（对端要限制就 `isolate`） |
  | `import` 与本地同名服务冲突时拒绝 | nodes.md | 借的规则：本地作用域里有的用本地的；对面放上来的插件提供的同名服务按内核原有规则（后注册的被拒绝），那个远程插件失败并说明（§七） |
  | 事件 `export: ["session/*"]` + 对端 `import: ["session/*"]` | nodes.md "节点之间的事件"；`events_nodes.rs` | 对端监听 `session/…` 的行放进它的连接分组，就收到这个节点发出的 `session/…`；要只收一部分，用监听器的过滤（按名字或描述里的作用域） |
  | `import: ["t/s/*"]` 让监听端只宣告一部分监听器 | `events_nodes.rs` | 只宣告连接分组里那些插件的监听器；分组之外的监听器本来就不宣告 |
  | 多跳：a 导入 b 的 `clock` 再导出给 c | `multihop.rs`、`websocket_multihop.rs`、`fault_links.rs` | c 的连接分组里的行 `inject: ["clock"]`，从 a 借；a 的作用域里有 `b:clock`（a 的 b 连接放上来的），所以借得到。事件仍只走一跳：c 的监听器没有放在 a 与 b 之间的连接上 |
  | `host: true` + 对端 `rows: true` + 行 `peer:office/scanner` | nodes.md "在别的节点上运行插件"；`peer_rows.rs`、`node.rs` | 行 `office:scanner`：本端 session 借 office 的 `loader` 建行。office 不想被用就在连接行上 `isolate: ["loader"]`（默认借得到，与 #185 L3"默认任何插件都能 inject loader"一致） |

- 做不到的现有用法：一处。现在任何本地行都能用对端 `export` 的服务（`import` 后注册为全局服务）；改后只有放在连接分组里的行（在这条连接上运行的）借得到，分组外的行要用就得放进分组，或者由对面把提供者放上来（对端看到 `<id>:<插件>`，它的服务全局可见）。这不是新机制，只是"在哪条连接上"写在分组里而不是名单里。
- 协议不变：借服务与宣告监听器用现有的帧（运行时一直这么用）。

**决定 2：插件的配置归启动它的一方。**

- 对面自己放到连接上的插件（节点连接分组里的行），配置在对面。
- 本端决定对面跑什么、用什么配置时，写行 `<id>:<插件>` 带 `config`：对面是运行时或应用时 loader 调 `run`，是节点时借对面的 `loader` 建行。配置都来自本端。
- 现在应用怎样拿配置（[应用接入](design-app-integration-2026-10-14.md) D2、D6，Kotlin 与 Swift 的 `RutisApp`）：应用只报带了哪些插件（`mount` 回复的 `plugins`），装哪个、用什么配置都由 rutis 配置里 `instanced` 分组的行给出，SDK 收到 `rows.v2` 的开始请求才 `apply`。这正是"本端启动"，即 `run`。
- 迁移：**应用与 SDK 不改**。`listen` + `instances` 分组里的 `phone:health` 行在每条应用连接的实例里变成一次 `run("health", config)`；使用它的本地行（`py:coach`）照旧在同一个实例里。每台设备的配置仍是分组里那一份，与现在相同；按设备不同由插件自己读设备上的设置（D7）。
- 不做：应用自己启动插件报给 rutis（没有使用方）。

## 三、Channel

```rust
pub struct Channel {
    pub sender: Box<dyn Sender>,
    pub receiver: Box<dyn Receiver>,
    pub closer: Arc<dyn Closer>,
    /// 连接层担保的对方 id；None 时以 hello 为准。
    pub id: Option<PeerId>,
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
- `id`：谁填。

  | 连接插件 | `id` |
  | --- | --- |
  | `rutis-host/spawn` | 它起的进程的名字（运行时名） |
  | `rutis-host/dial` | 配置的名字 |
  | `rutis-host/listen`，一台 | token 对应的那一条的名字 |
  | `rutis-host/listen`，多台 | 不填，以 hello 为准 |
  | `sessions.open` | 不填 |

- 删 `ChannelInfo`：对方 id 由上面的 `id` 与握手给出；`transport` 与 `label` 只用于诊断，改由连接插件自己的输出说。
- `Closer::close(&str)`、`replaced()`、`rejected(&str)` 合成 `close(Close)`。各底层怎样表达：

  | `Close` | WebSocket | 本机管道（子进程） | 内存 |
  | --- | --- | --- | --- |
  | `Done` | 关闭码 1000（宿主停止时 1001） | 关闭子进程的 stdin | 对端 `recv` 得到 `Ok(None)` |
  | `Replaced` | 关闭码 4002 | 同 `Done` | 对端 `recv` 得到 `Closed` |
  | `Rejected(r)` | 关闭码 4003，原因 `r` | 同 `Done` | 对端 `recv` 得到 `Rejected` |

- `ChannelError { Closed, Rejected }` 不变：`Rejected` 是对面以 4003 拒绝了本端。
- `ConnectError` 移出核心，成为 `websocket::dial` 的返回错误 `websocket::DialError`；`rutis-host/dial` 按它退避（可重试 0.5 → 30 秒，认证被拒每 30 秒，不兼容停止，规则不变）。
- 交给 `Session::new` 之后，只有 session 关闭 Channel；交出之前连接插件可以自己关（例如升级时 token 不对）。

## 四、生命周期

### 4.1 Channel 与 session

一对一，同生同灭，不复用。

| 发生了什么 | 谁先知道 | 结果 |
| --- | --- | --- |
| 握手完成 | session | 按 4.2 核对 id；通过则顶替，然后开始注册远程插件 |
| 核对不通过 | session | `close(Rejected(原因))`；不卸载任何东西，不注册任何东西 |
| 握手失败（协议版本、缺能力、超时 10 秒） | session | 对面先收到协议自己的错误帧，然后 `close(Done)`；session 以这个原因失败 |
| 对面先走（`recv` 返回 `Ok(None)` 或错误） | session | session 卸载：远程插件逆序卸载，进行中的调用失败一次 |
| 本地先走（连接插件卸载、行删掉、宿主停止） | 内核 | session 卸载：远程插件逆序卸载，然后 `close(Done)` |
| 重连 | 连接插件 | 新 Channel、新 session；旧 session 已经卸载 |

连接插件用 `ctx.plugin(...)` 返回的内核句柄（`FiberView::watch()`）等 session 卸载，再决定是否重连（dial 退避重连；spawn 按运行时的 `restart`；listen 什么都不做）。session 不重连。

### 4.2 核对 id 与顶替

session 核心有一张 id 表（每个宿主一张），记着每个 id 属于哪个连接插件：从这个连接插件的第一个 session 起，到这个连接插件卸载为止。`<id>:<插件>` 在整个宿主里唯一。

握手完成后，依次：

1. Channel 带 `id` 而 hello 报的 id 与它不同：`close(Rejected("expected <id>, the far end says <hello id>"))`。
2. id 属于另一个连接插件：`close(Rejected("<id> is taken by <那个连接插件的行名>"))`。
3. id 属于同一个连接插件、而且有 session 连着：卸载旧 session（远程插件逆序卸载，清理全部完成），旧 Channel `close(Replaced)`。
4. 新 session 记入表，开始注册远程插件。

- 拒绝（1、2）都在卸载旧的之前，所以被拒的连接不会先顶掉正常连接；被接受的连接在旧的卸载完之后才开始（3 在 4 之前）。这两条就是 TLA 变异 `access-mut-accept-before-release` 守的顺序：它把"接受新连接"挪到"释放旧连接"之前，模型找出反例；新实现里 3 → 4 的顺序与模型里 `TakeSlot` → `Accept` 一一对应，1、2 对应模型里在 `TakeSlot` 之前的准入检查。
- 设备报 `py`：`py` 属于 `rutis-host/spawn` 的那一行，第 2 步拒绝，本机运行时不受影响。
- 同一个 id 的顶替一个接一个；两个连接同时到达时，后完成握手的顶替先完成的。
- 边界：id 从第一个 session 起才记入表。宿主启动时 `rutis-host/listen` 的多台行在 spawn 行的第一个 session 之后才开始接受（本机进程起得快）；dial 行的对面一直连不上时，一台设备可以先占用这个名字，之后 dial 行被拒（`refused (gpu is taken by worker)`）。设备的 id 由开发者在应用里给出，与现在"两个安装用同一个 id 会轮流顶掉"一样由开发者保证不重名。
- 保留的两条保证：关闭码 4002（被顶替）与 4003（被拒绝），"先卸旧再接新"。测试 `an_id_revoked_while_its_connection_takes_over_is_not_accepted` 改为 `a_connection_whose_listener_stopped_while_it_takes_over_is_not_accepted`（见 §十二）。

### 4.3 远程插件映射到内核的生命周期

| 对面 | 本地的远程插件 `<id>:<插件>` |
| --- | --- |
| 插件在这条连接上开始运行（`run` 成功、借 `loader` 建的行运行、对面分组里的行运行） | 注册为 session 的子插件，`apply` 完成；它提供的服务以普通 `provide` 出现 |
| 运行失败（对面抛出） | 这个子插件失败，错误带对面的名字、消息、`code`、`site`（0.18 §4.2） |
| 对面结束了它 | 子插件卸载；使用它的服务的行按内核规则停下或等待 |
| 连接断开 | session 卸载，子插件全部卸载；它们从本端借的服务随之归还 |

- 没有单独的"连接状态"：连着就是 session 在，断开就是 session 不在。运行时的状态（本机进程起没起来）是连接插件的事；"运行时状态 + 会话是否连着"两项代替 `LinkState`、`AccessEvent`、`RuntimeState`。
- 写在 rutis.json 里、运行在对面的行（`gpu:embedder`）：对面不在时这一行等待（"waiting for runtime gpu"），session 出现后 loader 调 `run`；对面不含这个插件时这一行 `Unavailable`（#768 的文字不变）。
- 本端借 `loader` 在对面节点上建的行，挂在对面这条连接的 session 之下，不写进对面的配置文件，会话结束即卸载（与现在 `host` 托管的行相同）。这要求 #185 的 `loader.create` 接受调用方所在的作用域作为父级、这样建的行不进可编辑层（与 `instanced` 分组的副本一样不写回）；随 S3 实现。

## 五、扩展

只有一个扩展点：写连接插件，拿到 `Channel`，挂 session。

### 5.1 Rust 连接插件

```rust
// 经中继连到一台运行时
async fn apply(ctx: &Ctx, config: Relay) -> Result<(), Error> {
    loop {
        let mut channel = relay::connect(&config.address, &config.token).await?;  // 自己的认证与传输
        channel.id = Some(PeerId::new(&config.name)?);   // 中继担保对面是谁
        let session = ctx.plugin(Session::new(channel));
        disposed(session.watch()).await;                 // 内核的 FiberView::watch()：等 session 卸载
        sleep(backoff.next()).await;
    }
}
```

要限制对面能借什么，在这个连接插件的行上写 `isolate`；它所在的分组就是这条连接能借的范围。

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
- 底下与 `Session::new` 是同一份实现：`sessions.open` 用 `far` 做出一个 `Sender` / `Closer`，用 `near` 的队列做出一个 `Receiver`，`id` 不填。
- 代价：这种连接插件的每条消息多经过一次它所在的运行时进程。

### 5.3 动态加载

连接插件的加载方式与普通插件相同：rutis.json 里的一行、loader 的 `create`、#185 的 `loader` 服务。agent 运行中写的 TS / Python 连接插件经这些方式装上，`sessions.open` 建的 session 挂在它下面；卸载它，它的 session 与其下的远程插件全部卸载。

## 六、自带的连接插件与 rutis.json 的展开

三个插件在 rutis-host 里（不进 rutis-bridge），底层是 rutis-bridge 里的普通函数：`websocket::dial`、`websocket::listen`、`pipe::spawn`（测试用 `memory::pair`）。

| 插件 | 做什么 | Channel 的 `id` |
| --- | --- | --- |
| `rutis-host/spawn` | `pipe::spawn` 起本机运行时进程，挂 session；进程退出时按运行时的 `restart` 重起 | 运行时名 |
| `rutis-host/dial` | `websocket::dial`，挂 session；session 卸载后按 `DialError` 的类别退避重连 | 配置的名字 |
| `rutis-host/listen` | `websocket::listen`：升级时按当前名单比对 token（401 / 403 不变）；每条 Channel 挂一个 session；有 `instances` 时，先用 loader 已有的 `create_instance` 在那个 `instanced` 分组里建一个以 id 命名的实例，session 挂在实例里，session 卸载时关闭实例 | 一台：token 对应的那一条的名字；多台：不填 |

一个监听器上的多行按 token 分给各行（现在 `Registrations` 做的事），属于 `rutis-host/listen`；同一个监听器上 token 冲突仍在启动时报错（#702）。三个插件的配置 Schema 都写 `additionalProperties: false`（[配置 Schema](design-config-schema-2026-10-27.md) C3）。

rutis.json：

| 写法 | 展开成 |
| --- | --- |
| `runtimes.py`（以及 `node`、`bun`、`go`、`swift`、`kotlin` 等本机运行时） | `rutis-host/spawn`，`id` 为运行时名 |
| `runtimes.remote` 一条，`dial` | `rutis-host/dial`，`id` 为运行时名 |
| `runtimes.remote` 一条，`listen` | `rutis-host/listen`，一台 |
| `runtimes.remote` 一条，`listen` + `instances` | `rutis-host/listen`，多台，在 `instances` 分组里每条连接一个实例 |
| 节点 | 不展开：直接写 `rutis-host/dial` 或 `rutis-host/listen` 的行；要把本端的插件放到连接上，或要用 `isolate`，把这一行与那些行写进一个分组 |

节点的写法：

```json
{ "id": "office", "group": true, "isolate": ["loader"], "config": [
  { "id": "link", "name": "rutis-host/dial", "config": { "name": "office", "address": "wss://office.example.com/rutis" } },
  { "id": "weather", "name": "weather" },
  { "id": "scanner", "name": "office:scanner", "config": { "dpi": 300 } }
] }
```

- `weather` 是本端放在这条连接上的插件，office 那边看到 `main:weather`；`office:scanner` 借 office 的 `loader` 在那边运行（office 没有隔离 `loader` 时）；`isolate: ["loader"]` 让 office 借不到本端的 `loader`。
- 行 `<id>:<插件>` 由 loader 找到名字对应的 session：对面是运行时或应用调 `run`，是节点借 `loader`。在 `instanced` 分组的实例里，找到的是这个实例里的 session。本机与远程、一台与多台、运行时与节点走同一个解析器。
- 远程插件在实例里提供的服务只在这个实例里可见，沿用已有的实例服务规则（[实例服务](design-instance-services-2026-10-08.md)）。
- 嵌入 rutis-host 的 Rust 程序要自己的连接方式时，经 `Embedding.builtins` 注册一个连接插件，在 rutis.json 里写它的行。

## 七、开发者看到什么

遵守 [诊断设计](design-diagnostics-2026-10-26.md) §4.5：输出里不出现 session、`rutis-bridge/session`、内部 id；行写行名，运行时写它的名字，远程插件写 `<id>:<插件>`。

**输入**

- rutis.json：`runtimes.remote` 的 `dial` / `listen` / `instances`（#791）；节点是 `rutis-host/dial`、`rutis-host/listen` 的行，与要放到连接上的行写进一个分组；运行在对面的行写 `<id>:<插件>`。
- Rust：`Session::new(channel)`、`SessionHandle::{id, run}`、`Channel` 的 `id`；`websocket::{dial, listen}`、`pipe::spawn`、`memory::pair`。
- 其他语言：服务 `sessions` 的 `open(far)`。

**`run` 的输出**（与 #791 相同的保持相同，新增的标出）

```text
runtime gpu: link connected
runtime gpu: link down (gpu.example.com:7443: Connection refused); retrying in 2.1s
runtime gpu: link refused: auth-rejected (gpu.example.com: upgrade refused with 403 Forbidden); retrying in 30s
runtime worker: connected worker-1
runtime worker: disconnected worker-1 (connection lost)
runtime worker: disconnected worker-1 (replaced by a new connection)            ← 新增
runtime worker: refused py (py is taken by runtime py)                          ← 新增
runtime gpu: refused (expected gpu, the far end says gpu-2)                      ← 新增
link: link connected                                                             （节点行，前缀是行 id）
```

**`rutis-host status`**：`connections:` 一节照旧，每个连着的 id 后面写它注册了哪些远程插件：

```text
connections:
  gpu: link connected (gpu:embedder)
  worker: worker-1 (worker-1:chunker), worker-2 (worker-2:chunker)
  link: link connected (office:clock, office:scanner)
```

**出错**

| 情况 | 文字 |
| --- | --- |
| 对面不含行要的插件 | `embedder: unresolved: entry "embedder" rejected: runtime gpu: this connection does not contain plugin embedder`（#768，不变） |
| 对面放上来的插件提供的服务本地已有 | `office:weather: weather is provided here by row weather: not registered from office` |
| 对面借不到要的服务（被 `isolate` 或本端没有） | 对面那一行按它自己的规则等待或失败；对面是运行时时，文字与现在本机运行时借不到服务相同 |
| 节点借 `loader` 被隔离 | `scanner: unresolved: entry "scanner" rejected: office does not lend loader here` |
| 0.18 的远程写法（`rutis-bridge/peer` 行、`import`、`runtime`、`transport`、`identity`） | 没有专门的提示：`rutis-bridge/peer` 是不认识的插件名，按 0.18 的行解析失败报；`runtimes.remote` 里多的字段按 rutis-host 配置的未知字段报 |
| 对面运行插件时抛出 | 远程插件 `<id>:<插件>` 失败，按 0.18 §4.2 打印对面的错误 |

## 八、删除与收回清单

旧写法在 0.19 直接删，不标弃用，不做专门转换或提示。

| 名字 | 在哪 | 为什么 | 谁受影响 |
| --- | --- | --- | --- |
| `Transport`、`Dial`、`transport_key` | rutis-bridge `lib.rs` | 底层直接产出 `Channel` | 嵌入 rutis-bridge 的 Rust 程序；测试；文档里"实现 Transport"的承诺 |
| `Embedding.transports`、`runtimes.remote.transport` | rutis-host | 同上；只在未发布的 main 上存在（#791） | 无已发布用户；e2e `fixture-host` |
| `Embedding.identity` | rutis-host `host.rs` | 自己的凭据写在自己的连接插件里 | 嵌入 rutis-host 的 Rust 程序 |
| `Identity`、`IdentityPlugin`、`identity_key`、`StaticIdentity`、`Credential`、`Presented` | rutis-bridge `identity.rs` | 认证在连接插件里：`websocket::dial` 收 token，`rutis-host/listen` 升级时比对名单 | 测试；嵌入方 |
| `Admission`、`AdmissionPlugin`、`admission_key`、`SharedToken`、`Rejection` | rutis-bridge `identity.rs` | 改为 Channel 的 `id` 与 §4.2 的核对 | rutis-host；测试 |
| `Revocations`、`Revocable` | rutis-bridge `identity.rs` | 生产无调用方；断开现有连接靠停掉那一行 | `revocation.rs`；guarantees 两行；TLA 模型 |
| `Registration`、`Registered`、`RegistrationError`、`Registrations`、`Ticket`、`Refusal`、`Deliver` | rutis-bridge `registration.rs` | 按 token 分发并入 `rutis-host/listen` | WebSocket 监听；`memory_mux.rs` |
| `AccessPointPlugin`、`AccessPointConfig`、`AccessEvent`、`Accept`、`Accepted`、`Release` | rutis-bridge `access_point.rs` | 回调形状的交接 | rutis-host；loader `connections.rs`；测试 |
| `LinkPlugin`、`LinkConfig`、`LinkState`、`Connect`、`Failure`、`Retry`、`protocol()` | rutis-bridge `link.rs` | 去连与重连移到 `rutis-host/dial`，监听移到 `rutis-host/listen`；`protocol()` 无调用方 | rutis-host；测试；`examples/smoke.rs` |
| `PeerPlugin`、`PeerHandle`、`Features` | rutis-bridge `compose.rs` | 组合插件；节点不再有专门的功能；`PeerPlugin::new` 恒返回 `Ok` | rutis-host；loader；测试 |
| `Peer`、`peer_key`、`Handler`、`Offered`、`Offers`、`family()` | rutis-bridge `peer.rs` | 并入 session；`family()` 无调用方 | bridge 内部；测试 |
| `ExportPlugin`、`ImportPlugin` 与 `export`、`import` | rutis-bridge `services.rs`、`events/link.rs` 的名单部分 | 决定 1：按 `inject` 借，作用域定范围 | 节点配置；测试；`check.rs` 的提示文字 |
| `HostPlugin`、`PluginCatalog`、`StaticCatalog`、`Described`、`Installed`、`ServiceKeys` 与 `host: true`、`rows: true` | rutis-bridge `host.rs` | 借对方的 `loader` 服务 | 节点配置；`peer_rows.rs`、`node.rs` |
| `peer:<对端>/<插件>` 行、`rutis-bridge/peer` 行名、`PeerRows`、`PeerResolver`、`PeerRowsPlugin`、`LoaderCatalog`、`register_peer_node`、`node_schema` | rutis-loader `peer.rs` | 远程插件只有 `<id>:<插件>` 一种名字；节点是连接插件的行 | 节点配置；持久化样本 |
| 节点行上的 `runtime`、`transport`、`identity` | rutis-loader `peer.rs` | 随 `rutis-bridge/peer` 行一起删 | 写了它们的 rutis.json |
| `RuntimeSession`、`runtime_session_key`、`RuntimeAccessPlugin` | rutis-bridge `runtime/` | 服务形状的交接；被 `SessionHandle` 代替 | loader；测试 |
| `RuntimePlugin::remote` | rutis-bridge `runtime/plugin.rs` | 远程与本机走同一个 session | rutis-host；loader；测试 |
| `LocalRuntime` | rutis-bridge `runtime/local.rs` | 起进程移到 `rutis-host/spawn` | rutis-host；测试 |
| `RuntimeInstances`、`AcceptInstances`、`RuntimeConnection` | loader `runtime/connections.rs` | 远程专用的实例代码；listen 用 loader 已有的 `create_instance`，解析器统一 | rutis-host；loader 测试；rutis-migrate 的名字表 |
| `LinkState`、`AccessEvent`、`RuntimeState` 的远程部分 | bridge、rutis-host `status.rs` | 合成"运行时状态 + 会话是否连着" | rutis-host 的输出代码 |
| `ChannelInfo`、`Closer::replaced`、`Closer::rejected` | rutis-bridge `channel/mod.rs` | §三 | 各底层；测试 |
| `ConnectError`（移出 `channel`） | rutis-bridge `channel/mod.rs` | 归去连的函数 | 底层与 dial |
| `MemoryPlugin`、`MemoryTransport`、`LocalPlugin`、`WebSocketPlugin`、`WebSocketTransport` | `transport/*` | 只为提供 `Transport#<kind>` 存在；留下 `memory::pair`、`pipe::spawn`、`websocket::{dial, listen}` | 测试；rutis-host |

## 九、改前改后

现在的行数用 `wc -l` 在 `3d19634f` 上实测（含文件里的单元测试）；"公开项"是以 `pub fn|struct|enum|trait|type|const|use|mod` 开头的行数（含 impl 里的公开方法）。改后的数字是**估计**。

| 文件 | 现在行数 | 现在公开项 | 改后行数（估计） |
| --- | ---: | ---: | ---: |
| rutis-bridge `lib.rs` | 177 | 29 | 50 |
| `channel/mod.rs` | 165 | 13 | 130 |
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
| `events/link.rs` | 795 | 2 | 450 |
| `transport/websocket/mod.rs` | 262 | 8 | 150 |
| `transport/memory/mod.rs` | 409 | 11 | 200 |
| `transport/local/mod.rs` | 206 | 13 | 150 |
| 新增 `session/plugin.rs`（Session、SessionHandle、`sessions`、id 表、按 inject 借、连接上的插件） | — | — | 650 |
| **rutis-bridge 小计** | **7234** | **244** | **2280** |
| rutis-loader `runtime/connections.rs` | 550 | 10 | 0 |
| `runtime.rs` | 1621 | 14 | 1450 |
| `peer.rs` | 998 | 14 | 0 |
| `runtime/discover.rs` | 53 | 0 | 53 |
| **rutis-loader 小计** | **3222** | **38** | **1503** |
| rutis-host `host.rs` | 2089 | 27 | 1750 |
| `config.rs` | 1434 | 42 | 1250 |
| `status.rs` | 979 | 18 | 800 |
| 新增 `connect.rs`（spawn、dial、listen） | — | — | 700 |
| **rutis-host 小计** | **4502** | **87** | **4500** |
| **合计** | **14958** | **369** | **约 8280** |

- rutis-bridge 根上导出的名字：现在 59 个，改后约 7 个（`Channel`、`Close`、`PeerId`、`Session`、`SessionHandle`、`RowHandle`、`thrown`），估计；`fingerprint` 留在 `websocket` 模块里。
- session 的公开 API：`Session::new`、`SessionHandle::id`、`SessionHandle::run`、`sessions.open` 四个。
- 生命周期路径从 4 条变 1 条，交接形状从 2 种变 1 种，准入、顶替、状态各从 3 份变 1 份；节点没有专门的代码。
- 测试：只测被删接口的文件删掉或改写，约 5000 行（`access_point.rs` 584、`revocation.rs` 798、`link.rs` 547、`fault_links.rs` 959、`memory_mux.rs` 730、`runtime_instances.rs` 703、`peer_rows.rs` 556、`node.rs` 1011 的大部分等），行为测试改到新接口上，估计。

## 十、已合并的 PR 怎么处理

**结论：不 revert，在新实现里替换。** 理由：

- 混乱的主体（四条路径、两种形状、三份准入与状态）早于 0.19；revert #768、#791 只回到 0.18 的同样状态，什么都没解决。
- #791 的 `runtimes.remote` 写法、token 检查与《迁移到 0.19》在新架构里仍然保留，revert 再实现会让 CI、迁移指南、指南各多改一轮。

| PR | 保留 | 被替换 |
| --- | --- | --- |
| #768（R1 loader） | `discover.rs`（共用发现）；`Process::check_rows`（一处检查）；缺插件一律 `Unavailable` 与它的文字；`remote_modes.rs` 的用例（改为经 spawn / dial / listen 跑） | 两个调用点（`RuntimeResolver` 的远程分支、`RuntimeInstances`）合成一个按 session 找的解析器 |
| #791（R2 rutis-host） | `runtimes.remote` 的 `dial` / `listen` / `instances`；同一监听器与一台 `listen` 的 token 检查；`runtime <名>` 的状态行与 `status` 的 `connections:`；`control::serve` 的 `Report` 参数；《迁移到 0.19》（按 §十一改写）；`remote_runtimes.rs` 的大部分用例 | `transport` 字段与 `Embedding.transports`；`host.rs` 里用 `PeerPlugin { runtime }`、`AccessPointPlugin`、`AdmissionPlugin`、`SharedToken` 搭的连接，换成三个连接插件；"旧节点行写法照常可用"删掉；用例 `a_link_row_still_connects_a_runtime` 删掉，`an_added_transport_carries_instances` 改为"嵌入程序注册的连接插件承载多台" |
| #799 | — | 已关闭，不重开。它带的 guarantees §15 计数重算（`2d817aa3`）在改 guarantees 的实施 PR 里重做 |
| 分支 `claude/remote-drop-transport`（未推送，`9a5543cc`） | 去掉 `transport` 字段、`Embedding.transports` 与相关测试、文档的改动 | 并入 §十三 的 S4，不单独推送 |

## 十一、兼容与《迁移到 0.19》

不做兼容。不变的只有：会话协议；各语言运行时与 SDK（Kotlin、Swift 应用不改代码）；rutis.json 的 `format` 仍为 1；`runtimes.remote` 的 #791 写法。

《迁移到 0.19》的"远程运行时"一节改为只写新写法与对照：

1. **远程运行时**：`runtimes.remote` 一条写全（#791 的对照表保留）。删掉"旧写法照常可用，不用改，也不会被转换"、"还要这些字段的，继续用节点行"、同时写两种时的报错与 `transport` 一条。
2. **节点**：对照表。

   | 0.18 | 0.19 |
   | --- | --- |
   | `{ "name": "rutis-bridge/peer", "config": { "peer": "office", "dial": "<地址>" } }` | `{ "name": "rutis-host/dial", "config": { "name": "office", "address": "<地址>" } }` |
   | `"listen": "<监听器>"` | `rutis-host/listen`，`"listener": "<监听器>"` |
   | `export: [...]` | 提供者的行放进连接分组（对面看到 `<本端 id>:<插件>`），或什么都不写，让对面放进分组里的行按 `inject` 借 |
   | `import: [...]` | 使用者的行放进连接分组，按 `inject` 借；限制用分组上的 `isolate` |
   | 事件的 `export` / `import` | 监听的行放进连接分组；只收一部分用监听器的过滤 |
   | `host: true` | 不写：对面默认借得到 `loader`；不让用写 `isolate: ["loader"]` |
   | `rows: true` + 行 `peer:office/scanner` | 行 `office:scanner` |
   | `runtime: "gpu"` | `runtimes.remote` 的 `dial` 或 `listen` |
   | `transport`、`identity` | 自己的连接方式与凭据写成连接插件（§五），经 `Embedding.builtins` 注册 |

3. **自建连接**："实现 `Transport` 挂到 `Embedding.transports`"改为"交出一条 `Channel`：写一个连接插件，调 `Session::new(channel)`；其他语言用 `sessions.open`"。
4. **Rust API**："rutis、rutis-loader、rutis-bridge 的 Rust API 都不变"改为对照表：

   | 0.18 | 0.19 |
   | --- | --- |
   | `LinkPlugin` + `PeerPlugin { runtime }` + `RuntimePlugin::remote` | 连接插件里 `Session::new(websocket::dial(...)?)`，行 `<名>:<插件>` |
   | `AccessPointPlugin` + `RuntimeInstances` | `rutis-host/listen` 的写法，或自己的连接插件 + `instanced` 分组 |
   | `ExportPlugin` / `ImportPlugin` / `HostPlugin` | 作用域与 `isolate`；对方借 `loader` |
   | `Identity` / `Admission` / `Revocations` | 连接插件自己的认证；`Channel` 的 `id` |
   | `Transport` | `websocket::dial` / `websocket::listen` / `pipe::spawn` 直接给出 `Channel` |
   | `Closer::replaced()` / `rejected(r)` | `close(Close::Replaced)` / `close(Close::Rejected(r))` |
   | `ConnectError`（`rutis_bridge::`） | `websocket::DialError` |
   | `Embedding.identity` | 连接插件，经 `Embedding.builtins` |

5. **持久化样本**：0.8–0.18 的样本里有 `rutis-bridge/peer` 行（带 `import`、`runtime`、`transport`、`identity`）与 `peer:` 行，0.19 不再读。样本里删掉这些写法（期望同步），加 `rejected/` 用例，错误是 0.18 配置 Schema 给出的原样文字（不认识的插件名、不认识的字段）；guarantees 里持久化样本那一条写明 0.19 的这一处例外。
6. **新输出**：§七标为"新增"的三行；`status` 的 `connections:` 每个 id 后列出远程插件。

## 十二、要同步改的文档

| 文档 | 改什么 |
| --- | --- |
| `docs/guide/nodes.md` / `.en.md` | 重写"一条 link"与"在 link 上做什么"：节点是 `rutis-host/dial` / `rutis-host/listen` 的行与连接分组；`export` / `import` / `host` / `rows` / `runtime` 表删除，换成"按 `inject` 借、作用域定范围、`isolate` 限制"；"在别的节点上运行插件"改为 `office:scanner` 与借 `loader`；"实现一个 `Transport`"改为"写一个连接插件"；"安全"一节的吊销钩子改为"断开现有连接靠停掉那一行"，`host: true` 一条改为 `isolate: ["loader"]` |
| `docs/guide/rutis-host.md` / `.en.md` | `runtimes.remote` 表去掉 `transport` 与"都不写"一行；启动错误列表同步；`Embedding` 的 `transports`、`identity` 删除；节点行一节改为连接插件的行 |
| `docs/guide/rust-host.md` / `.en.md` | `PeerPlugin`、`RuntimePlugin::remote` 的例子改为连接插件 + `Session::new` |
| `docs/guide/swift-app.md`、`kotlin-app.md`（两种语言） | 监听示例里的 `rutis-bridge/peer` 行改为 `runtimes.remote` 的 `dial`；其余不变 |
| `docs/guide/cordis.md`、`plugin-api.md`、`bun-plugin.md` 等提到节点、`peer:` 的地方 | 逐处改为新写法 |
| `.agents/skills/rutis-remote-plugin/SKILL.md` 与 `references/nodes.md`、`rutis-json.md`、`plugin-api.md` | 同指南；"实现 Transport 挂到 Embedding.transports"改为"交出一条 Channel / 写连接插件"，加 `sessions.open` 的最小例子 |
| `.agents/skills/rutis-new-runtime/references/plugin-api.md` | 让已有运行时远程可用的一节：运行时一侧不变，rutis 一侧不再需要 `RuntimePlugin::remote` |
| `docs/migration-0.19.md` / `.en.md` | §十一 |
| `docs/guarantees.md` / `.en.md` | `bridge/revocation-closes-only-its-connections`、`bridge/revocation-refuses-reconnects` 改写为"名单变了后新连接被拒（升级时 403，或 hello 之后 4003）；断开现有连接靠停掉那一行"；`host/env-tokens-not-revocable` 同步；`host/remote-runtimes` 去掉 `transport` 与旧节点行；`loader/runtime-connection-instances` 改写为 listen + `instanced` 分组；`loader/peer-rows` 改写为 `<id>:<插件>` 行借 `loader`；`events/node-links`、`events/link-export-import`、`events/node-config` 改写为连接分组里的插件按名字收发，`node-config` 删除；`events/no-multi-hop` 措辞按 §2.6 的多跳一行；`bridge/dial-out-rules` 里 4003 的来源改为 §4.2；新增一行：§4.2 的核对与顶替顺序；持久化样本一行写明 0.19 例外；每行的测试名随测试改；§15 计数重算 |
| `specs/remote-lease/`（README 两份、`AccessPoint.tla`、配置、`expected.txt`） | 去掉吊销动作与 `RevokedStaysOut`、`RevocationScoped`，删 `access-revoke-same`、`access-revoke-distinct`、`access-mut-revoke-cuts-all`；准入检查改为 §4.2 的 1、2 两步（担保的 id、别的连接插件占用）；`NoStaleHandOver` 保留，说法改为"停掉的监听不再交出连接"；`access-mut-accept-before-release` 保留；"对应的代码"一表改指 `rutis-bridge/src/session/plugin.rs`（id 表）与 `rutis-host/src/connect.rs`（listen）；`RemoteLease.tla` 的吊销远端改为"停掉的 dial 行" |
| `crates/rutis-bridge/README.md`、`lib.rs` 的模块说明、根 `README` | 模块介绍改为 Channel、session、底层函数 |
| 旧设计稿 `design-remote-runtime-consistency-2026-10-26.md` / `.en.md` | 头部加一行"实施部分被 [远程连接：只留一个会话插件](design-remote-session-2026-10-28.md) 取代" |

## 十三、实施

先加后删，每个 PR 合并后 main 可用、CI 通过。

| PR | 内容 | 完成条件 | 依赖 |
| --- | --- | --- | --- |
| S1 rutis-bridge：Channel 与底层函数 | `Close`、`close(Close)`、`Channel.id`；删 `ChannelInfo`；`ConnectError` 移到 `websocket::DialError`；新增 `websocket::dial`、`websocket::listen`、`pipe::spawn`、`memory::pair`（现有 `Transport` 实现改为调用它们） | 现有测试不改用例通过（只改到 `close` 的调用）；新单元测试：三种 `Close` 在三种底层上的表现（4002、4003、stdin 关闭） | — |
| S2 rutis-bridge：session 插件 | `Session::new`、`SessionHandle::{id, run}`；id 表、核对与顶替（§4.2）；远程插件 `<id>:<插件>`；按 `inject` 借服务、把事件发给对面的监听器（运行时已有的那份，推广到对面是 rutis 节点）；连接分组里的插件注册到对面；服务 `sessions` 与 `open`；旧的 link / 接入点不动 | 测试：§4.2 每一步（担保 id 不符、别的连接插件占用、同插件顶替先卸旧再接新，移植 `access-mut-accept-before-release` 的场景）；四种结束；两个 rutis 内核经内存 Channel 互借服务、收发事件、`isolate` 限制；Node 与 Python 测试插件经 `sessions.open` 跑通 `run` | S1 |
| S3 rutis-loader：统一解析器 | 行 `<id>:<插件>` 按名字找 session：运行时调 `run`，节点借 `loader`；实例里找实例里的 session；`loader.create` 接受调用方作用域为父级且不进可编辑层（§4.3）；旧路径并存 | `remote_modes.rs` 的用例在 session 上跑：spawn、dial、listen、listen + `instances` 四种接法缺插件文字逐字相同；节点上 `office:scanner` 经 `loader` 运行、会话结束即卸载、对面配置文件不变；实例服务的现有用例通过 | S2 |
| S4 rutis-host：三个连接插件 | `rutis-host/spawn`、`dial`、`listen`（Schema `additionalProperties: false`）；`runtimes.*`、`runtimes.remote` 展开成它们；状态合成两项，§七的输出；删 `transport` 字段、`Embedding.transports`、`Embedding.identity`（并入 drop-transport 分支）；持久化样本与 `rejected/` 用例 | `remote_runtimes.rs`、`dial_in.rs`、`kotlin.rs` 端到端在新插件上通过；节点端到端（两个 rutis-host，连接分组、借服务、事件、`office:scanner`）；`dial_conformance` 各语言对 `rutis-host/listen` 通过；`persisted` 测试通过；e2e `fixture-host` 编译；#702 的 token 检查用例通过 | S3 |
| S5 删除 | 删 §八 全部条目与只测它们的测试；改写 TLA 模型并跑 `specs/remote-lease/run.sh` | `grep` §八 的名字在 `crates/` 下无结果；`cargo test --workspace` 与 clippy 通过；TLA `expected.txt` 更新并通过 | S4 |
| S6 文档 | §十二 全部条目；《迁移到 0.19》§十一 | 两种语言内容一致；文档里不再出现 `Transport`、`Embedding.transports`、`Revocations`、`export` / `import`、`rutis-bridge/peer` 的承诺；guarantees 每行的测试名都存在 | S5（与 S4 同时可见的行为可以随 S4 先改指南） |
