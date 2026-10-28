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
| 远程插件 `<id>:<插件名>` | session 的子插件，自动注册 | 对面的一个插件在本地的样子 |
| 连接插件 | 核心之外，谁写都行 | 拿到一条 `Channel`，挂一个 session |
| └ `rutis-host/spawn`、`rutis-host/dial`、`rutis-host/listen` | rutis-host 自带 | 起本机进程、去连、监听 |

### 2.2 结构

```text
连接插件（rutis-host/dial、rutis-host/listen、rutis-host/spawn，或别人写的）
 └─ rutis-bridge/session             每条 Channel 一个
     ├─ <id>:weather                 对面的插件，自动注册
     └─ <id>:llm
```

### 2.3 关系

- **连接插件 → session**：`ctx.plugin(Session::new(channel))`。Channel 关闭，session 卸载；session 卸载，Channel 关闭。
- **session → 远程插件**：对面的插件开始运行就注册为子插件，结束就卸载；session 卸载时它们全部随之卸载。
- **远程插件 ↔ 本地插件**：与普通插件相同（`inject` / `provide`），生命周期全部用内核原有的。
- **顶替**：同一个 id 再连进来，不论经哪个连接插件，session 核心先卸载旧的，再挂新的。
- **隔离**：在连接插件的行上写 `isolate`；每条连接要一个单独的作用域时，用 loader 已有的 `instanced` 分组（§六）。

### 2.4 API 入口

Rust（编进宿主的连接插件）：

```rust
pub struct Channel { /* §三 */ }

pub struct Session;
impl Session {
    pub fn new(channel: Channel) -> Self;
    /// 对方报出 id 之后、顶替与注册之前调用；返回 Err 时以 Close::Rejected 关闭。
    pub fn admit(self, check: impl Fn(&str) -> Result<(), String> + Send + Sync + 'static) -> Self;
    /// 本端在这条连接上提供的服务与事件（名字或 `*` 前缀），节点互联用。
    pub fn export(self, names: impl IntoIterator<Item = String>) -> Self;
    /// 允许对面在本端运行这里装着的插件（节点行的 `host: true`）。
    pub fn host(self, catalog: Arc<dyn PluginCatalog>) -> Self;
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
interface Options { admit?(id: string): void; export?: string[] }
sessions.open(far: Far, options?: Options): Near
```

- `far`、`near` 里的函数按引用跨语言（0.18 #687）。
- `open` 建的 session 挂在调用它的插件下面：这个插件卸载，它的 session 全部卸载。
- `admit`、`export` 与 Rust 的同名方法相同；`host` 只在 Rust 里有（插件目录是宿主的东西）。

`admit` 与 `export` 是 #695 评论之外加的两项，理由：对方的 id 在 hello 里才知道，而拒绝必须发生在顶替之前（否则一个会被拒绝的连接已经把同 id 的旧连接卸掉了，正是 TLA 变异 `access-mut-accept-before-release` 抓的错），所以连接插件要有一个在 hello 之后、顶替之前的判断点；`export` 是待定项 1 定下的提供方名单（§2.6）。

### 2.5 "以后"的结构

```text
rutis-host
 ├─ rutis-host/spawn  py          ── pipe::spawn ──────────▶ Channel ─▶ session（id py）
 │                                                                       └─ py:coach …（run）
 ├─ rutis-host/dial   gpu         ── websocket::dial ─────▶ Channel ─▶ session（id gpu）
 │                                                                       └─ gpu:embedder（run）
 ├─ rutis-host/listen worker      ── websocket::listen ──▶ 每条连接一条 Channel
 │     ├─ session（id worker-1）   └─ worker-1:chunker …
 │     └─ session（id worker-2）   └─ worker-2:chunker …
 ├─ rutis-host/dial   office（节点）── export: [weather] ─▶ session（id office）
 │                                                        └─ office:clock …（对面 export 的）
 └─ agent 运行中写的 TS 插件 relay ── sessions.open(far) ──▶ session（id 由对面报）
```

所有连接都是同一种形状；session 之下没有按场景分叉的代码。

### 2.6 两个待定项的决定

**决定 1：节点互联只保留提供方的 `export`，去掉 `import`。**

- 规则：一端 `export` 的服务与事件，对端全部注册；要限制，在对端的连接行上用 `isolate`。
- 依据（查过 [连接节点](guide/nodes.md) 与代码里的全部用法：`node.rs`、`multihop.rs`、`fault_links.rs`、`websocket_multihop.rs`、`websocket_soak.rs`、`cordis_node.rs`、`events_nodes.rs`、持久化样本）：
  - `import` 的三种用途都有不加机制的替代：
    1. 只要对端的一部分服务：由提供方的 `export` 定（每条连接一行，提供方本来就按对端写）；接收方不信任提供方时在连接行上写 `isolate`，对端的这些服务只留在这一行之下，本地其他行看不到。
    2. 与本地同名服务冲突时拒绝：保留，按内核原有规则（后注册的被拒绝），对端那个远程插件失败并说明原因（§七）。
    3. 事件：现在要发出端 `export`、监听端 `import` 两边都写。改为只看发出端的 `export`：监听端把自己的监听器全部宣告给对端，发出端只把 `export` 里的名字发过去。监听端要按来源筛选，用已有的过滤（对端监听器的描述带它所在的作用域，[统一事件](design-events-2026-10-18.md)）。
  - 运行时（本机与远程）本来就不用 `export` / `import`：它们的服务与监听器直接在宿主上。去掉 `import` 后节点与运行时一样。
- 做不到的现有用法：一处。`events_nodes.rs` 的 `import: ["t/s/*"]` 让监听端只把一部分监听器的名字宣告给对端；改后监听端宣告它全部监听器的名字，对端能看到本端在听哪些事件名。宣告的只是事件名，不是数据，数据仍只有发出端 `export` 了才过来；没有使用方要求隐藏这些名字（[设计哲学](design-philosophy.md) §七"判断一项能力"第 4 条），所以不给替代、不另加机制，在 guarantees 的 `events/link-export-import` 一行写明。
- 协议不变：宣告与提供用的是现有的帧，只是内容由 `import` 名单变成全部。0.18 的节点与 0.19 的节点相遇时，0.18 一侧仍按它的 `import` 筛选，能正常工作。

**决定 2：插件的配置归启动它的一方。**

- 对面自己启动的插件（节点 `export` 的、设备上应用自己启动的），配置在对面；本地只看到注册好的 `<id>:<插件>`。
- 宿主要决定对面跑什么、用什么配置时，用 `run(plugin, config)`；rutis.json 里写行 `<运行时名>:<插件>` 带 `config`，就是对这个运行时的 session 调 `run`。
- 现在应用怎样拿配置（[应用接入](design-app-integration-2026-10-14.md) D2、D6，Kotlin `RutisApp`、Swift `RutisApp`）：应用只报它带的插件（`mount` 回复的 `plugins`），装哪个、用什么配置都由 rutis 配置里 `instanced` 分组的行给出，SDK 收到 `rows.v2` 的开始请求才 `apply`。这正是"宿主启动"，在新架构里是 `run`。
- 迁移：**应用与 SDK 不改**。`listen` + `instances` 的分组里写的 `phone:health` 行，在每条应用连接的实例里变成一次 `run("health", config)`；使用它的本地行（`py:coach`）照旧在同一个实例里。每台设备的配置仍是分组里那一份，与现在相同（现在也没有按设备区分的配置）；要按设备不同，插件读设备自己的设置，这是插件的事（D7）。
- 不做：应用自己启动插件并报给 rutis（需要 SDK 新增导出一侧，没有使用方）。以后有需要时它走的就是节点的 `export`，不需要新机制。

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
- 删 `ChannelInfo`：对方 id 由 session 握手给出；`transport` 与 `label` 只用于诊断，改由连接插件自己的输出说。
- `Closer::close(&str)`、`replaced()`、`rejected(&str)` 合成 `close(Close)`。各底层怎样表达：

  | `Close` | WebSocket | 本机管道（子进程） | 内存 |
  | --- | --- | --- | --- |
  | `Done` | 关闭码 1000（宿主停止时 1001） | 关闭子进程的 stdin | 对端 `recv` 得到 `Ok(None)` |
  | `Replaced` | 关闭码 4002 | 同 `Done`（子进程归这一端，不会被顶替） | 对端 `recv` 得到 `Closed` |
  | `Rejected(r)` | 关闭码 4003，原因 `r` | 同 `Done` | 对端 `recv` 得到 `Rejected` |

- `ChannelError { Closed, Rejected }` 不变：`Rejected` 是对面以 4003 拒绝了本端。
- `ConnectError` 移出核心：它是"去连"失败的分类，只有去连的函数和它的重试用。改为 `websocket::dial` 的返回错误，`rutis-host/dial` 按它退避（可重试 0.5 → 30 秒，认证被拒每 30 秒，不兼容停止，规则不变）。
- 交给 `Session::new` 之后，只有 session 关闭 Channel；交出之前连接插件可以自己关（例如升级时 token 不对）。

## 四、生命周期

### 4.1 Channel 与 session

一对一，同生同灭，不复用。

| 发生了什么 | 谁先知道 | 结果 |
| --- | --- | --- |
| 握手完成 | session | 调 `admit(id)`；通过则按 4.2 顶替，然后开始注册远程插件 |
| `admit` 拒绝 | session | `close(Rejected(原因))`；不卸载任何东西，不注册任何东西 |
| 握手失败（协议版本、缺能力、超时 10 秒） | session | 对面先收到协议自己的错误帧，然后 `close(Done)`；session 以这个原因失败 |
| 对面先走（`recv` 返回 `Ok(None)` 或错误） | session | session 卸载：远程插件逆序卸载，进行中的调用失败一次 |
| 本地先走（连接插件卸载、行删掉、宿主停止） | 内核 | session 卸载：远程插件逆序卸载，然后 `close(Done)` |
| 重连 | 连接插件 | 新 Channel、新 session；旧 session 已经卸载 |

连接插件用 `ctx.plugin(...)` 返回的内核句柄等 session 卸载，再决定是否重连（dial 退避重连；spawn 按运行时的 `restart`；listen 什么都不做）。session 不重连。

### 4.2 顶替

- session 核心有一张 id 表（每个宿主一张，在 `rutis-bridge/session` 自己的共享状态里）。`<id>:<插件>` 在整个宿主里唯一。
- 新 session 的 `admit` 通过后，表里已有这个 id 时：
  1. 卸载旧 session（它的远程插件逆序卸载，清理全部完成）；
  2. 旧 Channel `close(Replaced)`；
  3. 新 session 记入表，开始注册远程插件。
- 同一个 id 的顶替一个接一个；两个连接同时到达时，后完成握手的顶替先完成的。
- 连接插件只负责拒绝（`admit`）。`rutis-host/listen` 在多台模式下拒绝与本宿主其他连接行同名的 id（例如一台设备报 `py`），所以设备不能顶掉本机运行时；同一个连接插件下的同 id 照常顶替（设备重连）。
- 保留的两条保证：关闭码 4002（被顶替）与 4003（被拒绝），"先卸旧再接新"。对应 TLA 变异 `access-mut-accept-before-release` 与测试 `an_id_revoked_while_its_connection_takes_over_is_not_accepted`（改名为 `a_connection_whose_listener_stopped_while_it_takes_over_is_not_accepted`，见 §十二）。

### 4.3 远程插件映射到内核的生命周期

| 对面 | 本地的远程插件 `<id>:<插件>` |
| --- | --- |
| 插件开始运行（`run` 成功，或对面 `export` 了它） | 注册为 session 的子插件，`apply` 完成；它提供的服务以普通 `provide` 出现 |
| `run` 失败（对面抛出） | 这个子插件失败，错误带对面的名字、消息、`code`、`site`（0.18 §4.2） |
| 对面结束了它（`rows.ended`，或不再 `export`） | 子插件卸载；使用它的服务的行按内核规则停下或等待 |
| 连接断开 | session 卸载，子插件全部卸载 |

- 没有单独的"连接状态"给插件看：连着就是 session 在，断开就是 session 不在。运行时的状态（本机进程起没起来）是连接插件的事；"运行时状态 + 会话是否连着"两项代替 `LinkState`、`AccessEvent`、`RuntimeState` 三种。
- 写在 rutis.json 里、运行在对面的行（`gpu:embedder`）：对面不在时这一行等待它（"waiting for runtime gpu"），session 出现后由 loader 调 `run`；对面不含这个插件时这一行 `Unavailable`（#768 的文字不变）。

## 五、扩展

只有一个扩展点：写连接插件，拿到 `Channel`，挂 session。

### 5.1 Rust 连接插件

```rust
// 经中继连到一台运行时
async fn apply(ctx: &Ctx, config: Relay) -> Result<(), Error> {
    loop {
        let channel = relay::connect(&config.address, &config.token).await?;  // 自己的认证与传输
        let session = ctx.plugin(Session::new(channel).admit({
            let want = config.name.clone();
            move |id| (id == want).then_some(()).ok_or_else(|| format!("expected {want}, got {id}"))
        }));
        disposed(session.watch()).await; // 内核的 FiberView::watch()：等 session 卸载
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

## 六、自带的连接插件与 rutis.json 的展开

三个插件在 rutis-host 里（不进 rutis-bridge），底层是 rutis-bridge 里的普通函数：`websocket::dial`、`websocket::listen`、`pipe::spawn`（测试用 `memory::pair`）。

| 插件 | 做什么 | `admit` |
| --- | --- | --- |
| `rutis-host/spawn` | `pipe::spawn` 起本机运行时进程，挂 session；进程退出时按运行时的 `restart` 重起 | id 是这一行起的进程的名字 |
| `rutis-host/dial` | `websocket::dial`，挂 session；session 卸载后按 `ConnectError` 的类别退避重连 | id 等于这一行的名字 |
| `rutis-host/listen` | `websocket::listen`：升级时按当前名单比对 token（401 / 403 不变）；每条 Channel 挂一个 session；有 `instances` 时，先用 loader 已有的 `create_instance` 在那个 `instanced` 分组里建一个以 id 命名的实例，session 挂在实例里，session 卸载时关闭实例 | 一台：id 等于这一行的名字；多台：id 不是本宿主其他连接行的名字 |

一个监听器上的多行按 token 分给各行（现在 `Registrations` 做的事），属于 `rutis-host/listen`；同一个监听器上 token 冲突仍在启动时报错（#702）。

rutis.json 的写法不变，宿主把它们展开成这三个插件的行：

| rutis.json | 展开成 |
| --- | --- |
| `runtimes.py`（以及 `node`、`bun`、`go`、`swift`、`kotlin` 等本机运行时） | `rutis-host/spawn`，`id` 为运行时名 |
| `runtimes.remote` 一条，`dial` | `rutis-host/dial`，`id` 为运行时名 |
| `runtimes.remote` 一条，`listen` | `rutis-host/listen`，一台 |
| `runtimes.remote` 一条，`listen` + `instances` | `rutis-host/listen`，多台，在 `instances` 分组里每条连接一个实例 |
| 节点行 `rutis-bridge/peer`，`dial` | `rutis-host/dial`，带 `export`、`host`、`rows` |
| 节点行 `rutis-bridge/peer`，`listen` | `rutis-host/listen`，一台，带 `export`、`host`、`rows` |

- 行 `<运行时名>:<插件>`（以及节点的 `peer:<对端>/<插件>`）由 loader 找到名字对应的 session，调 `run`。在 `instanced` 分组的实例里，找到的是这个实例里的 session。本机与远程、一台与多台走同一个解析器。
- 远程插件在实例里提供的服务只在这个实例里可见，沿用已有的实例服务规则（[实例服务](design-instance-services-2026-10-08.md)）。
- `rutis-bridge/peer` 这个行名保留：它是节点配置的写法，改名只会让每份节点配置都要改。
- 嵌入 rutis-host 的 Rust 程序要自己的连接方式时，经 `Embedding.builtins` 注册一个连接插件，在 rutis.json 里写它的行；不再有 `Embedding.transports`、`Embedding.identity`。

## 七、开发者看到什么

遵守 [诊断设计](design-diagnostics-2026-10-26.md) §4.5：输出里不出现 session、`rutis-bridge/session`、内部 id；行写行名，运行时写它的名字，远程插件写 `<id>:<插件>`（开发者写的插件名加对方报的 id）。

**输入**

- rutis.json：与 #791 之后相同（`runtimes.remote` 的 `dial` / `listen` / `instances`，节点行的 `dial` / `listen` / `export` / `host` / `rows`）。去掉的字段见 §八。
- Rust：`Session::new(channel)`、`admit`、`export`、`host`、`SessionHandle::{id, run}`；`websocket::{dial, listen}`、`pipe::spawn`、`memory::pair`。
- 其他语言：服务 `sessions` 的 `open`。

**`run` 的输出**（与 #791 相同的保持相同，新增的两行标出）

```text
runtime gpu: link connected
runtime gpu: link down (gpu.example.com:7443: Connection refused); retrying in 2.1s
runtime gpu: link refused: auth-rejected (gpu.example.com: upgrade refused with 403 Forbidden); retrying in 30s
runtime worker: connected worker-1
runtime worker: disconnected worker-1 (connection lost)
runtime worker: disconnected worker-1 (replaced by a new connection)        ← 新增
runtime worker: refused py (py is runtime py of this host)                   ← 新增
office: link connected
```

**`rutis-host status`**：`connections:` 一节照旧（一台写 link 的状态，多台写连着的 id），每个连着的 id 后面写它注册了哪些远程插件：

```text
connections:
  gpu: link connected (gpu:embedder)
  worker: worker-1 (worker-1:chunker), worker-2 (worker-2:chunker)
  office: link connected (office:clock, office:weather)
```

**出错**

| 情况 | 文字 |
| --- | --- |
| 对面不含行要的插件 | `embedder: unresolved: entry "embedder" rejected: runtime gpu: this connection does not contain plugin embedder`（#768，不变） |
| 对面 `export` 的服务本地已有 | `office:weather: weather is provided here by row weather: not registered from office` |
| 节点行写了 `import` | `office: config: import was removed in 0.19: what office exports is registered here; to keep it inside this row, use isolate` |
| 节点行写了 `runtime` | `office: config: runtime was removed in 0.19: write the runtime in runtimes.remote with dial or listen` |
| 节点行写了 `transport` 或 `identity` | `office: config: transport was removed in 0.19: a host's own way of connecting is a plugin of its own (docs/migration-0.19.md)`（`identity` 同样的话） |
| `runtimes.remote` 写了 `transport` | 不出现：这个字段只在未发布的 main 上存在过，直接作为未知字段报错 |
| 对面运行插件时抛出 | 远程插件 `<id>:<插件>` 失败，按 0.18 §4.2 打印对面的错误 |

## 八、删除与收回清单

| 名字 | 在哪 | 为什么 | 谁受影响 |
| --- | --- | --- | --- |
| `Transport`、`Dial`、`transport_key` | rutis-bridge `lib.rs` | 底层直接产出 `Channel`，不需要按 kind 查找的服务 | 嵌入 rutis-bridge 的 Rust 程序；bridge 与 loader 的测试；文档里"实现 Transport"的承诺 |
| `Embedding.transports` | rutis-host `host.rs` | 同上；只在未发布的 main 上存在（#791） | 无已发布用户；e2e `fixture-host` |
| `runtimes.remote.transport` | rutis-host `config.rs` | 同上；未发布 | 无 |
| 节点行的 `transport`、`identity` | rutis-host / loader `peer.rs` | 指向被删的 `Transport#<kind>`、`Identity#<name>` | 写了它们的 rutis.json（持久化样本 0.10–0.18 的 `branch` 行） |
| `Embedding.identity` | rutis-host `host.rs` | 自己的凭据写在自己的连接插件里 | 嵌入 rutis-host 的 Rust 程序 |
| `Identity`、`IdentityPlugin`、`identity_key`、`StaticIdentity`、`Credential`、`Presented` | rutis-bridge `identity.rs` | 认证在连接插件里：`websocket::dial` 收 token，`rutis-host/listen` 升级时比对名单 | 测试（大量用 `StaticIdentity`）；嵌入方 |
| `Admission`、`AdmissionPlugin`、`admission_key`、`SharedToken`、`Rejection` | rutis-bridge `identity.rs` | 改为 `admit` 与 listen 的名单比对 | rutis-host；测试 |
| `Revocations`、`Revocable` | rutis-bridge `identity.rs` | 生产无调用方；断开现有连接靠停掉那一行 | `revocation.rs` 测试；guarantees 两行；TLA 模型 |
| `Registration`、`Registered`、`RegistrationError`、`Registrations`、`Ticket`、`Refusal`、`Deliver` | rutis-bridge `registration.rs` | 按 token 分发并入 `rutis-host/listen` | WebSocket 监听；`memory_mux.rs` |
| `AccessPointPlugin`、`AccessPointConfig`、`AccessEvent`、`Accept`、`Accepted`、`Release` | rutis-bridge `access_point.rs` | 回调形状的交接；被 session 代替 | rutis-host；loader `connections.rs`；测试 |
| `LinkPlugin`、`LinkConfig`、`LinkState`、`Connect`、`Failure`、`Retry`、`protocol()` | rutis-bridge `link.rs` | 去连与重连移到 `rutis-host/dial`，监听移到 `rutis-host/listen`；`protocol()` 无调用方 | rutis-host；测试；`examples/smoke.rs` |
| `PeerPlugin`、`PeerHandle`、`Features` | rutis-bridge `compose.rs` | 组合插件；功能成了 session 的 `export` / `host`；`PeerPlugin::new` 恒返回 `Ok` | rutis-host；loader `peer.rs`；测试 |
| `Peer`、`peer_key`、`Handler`、`Offered`、`Offers`、`family()` | rutis-bridge `peer.rs` | 并入 session；`family()` 无调用方 | bridge 内部；测试 |
| `ImportPlugin` 与节点行的 `import` | rutis-bridge `services.rs`、loader `peer.rs` | 决定 1 | 节点配置；持久化样本；`check.rs` 的提示文字 |
| `RuntimeSession`、`runtime_session_key`、`RuntimeAccessPlugin` | rutis-bridge `runtime/mod.rs`、`runtime/access.rs` | 服务形状的交接；被 `SessionHandle` 代替 | loader；测试 |
| `RuntimePlugin::remote` | rutis-bridge `runtime/plugin.rs` | 远程与本机走同一个 session，行由 `run` 运行 | rutis-host；loader；测试 |
| `LocalRuntime` | rutis-bridge `runtime/local.rs` | 起进程移到 `rutis-host/spawn`（底层 `pipe::spawn`） | rutis-host；测试 |
| `RuntimeInstances`、`AcceptInstances`、`RuntimeConnection` | loader `runtime/connections.rs` | 远程专用的实例代码；改为 listen 用 loader 已有的 `create_instance`，解析器统一 | rutis-host；loader 测试；rutis-migrate 的名字表 |
| `RuntimeState` 的远程部分、`LinkState`、`AccessEvent` 三种状态 | bridge、rutis-host `status.rs` | 合成"运行时状态 + 会话是否连着" | rutis-host 的输出代码 |
| `ChannelInfo`、`Closer::replaced`、`Closer::rejected` | rutis-bridge `channel/mod.rs` | §三 | 各底层；测试 |
| `ConnectError`（移出 `channel`） | rutis-bridge `channel/mod.rs` | 归去连的函数 | 底层与 dial |
| 节点行上的 `runtime` | loader `peer.rs`、rutis-host | 0.19 起由 `runtimes.remote` 写；旧写法直接删，不标弃用 | 写了它的 rutis.json |
| `MemoryPlugin`、`MemoryTransport`、`LocalPlugin`、`WebSocketPlugin`、`WebSocketTransport` | `transport/*` | 只为提供 `Transport#<kind>` 而存在；留下 `memory::pair`、`pipe::spawn`、`websocket::{dial, listen}` | 测试；rutis-host |

旧写法在 0.19 直接删（0.19 本来是远程这块的不兼容版本），不标弃用。

## 九、改前改后

现在的行数用 `wc -l` 在 `3d19634f` 上实测（含文件里的单元测试）；"公开项"是以 `pub fn|struct|enum|trait|type|const|use|mod` 开头的行数（含 impl 里的公开方法）。改后的数字是**估计**。

| 文件 | 现在行数 | 现在公开项 | 改后行数（估计） |
| --- | ---: | ---: | ---: |
| rutis-bridge `lib.rs` | 177 | 29 | 70 |
| `channel/mod.rs` | 165 | 13 | 130 |
| `access_point.rs` | 607 | 16 | 0 |
| `compose.rs` | 270 | 6 | 0 |
| `identity.rs` | 564 | 27 | 40（只留 `fingerprint`） |
| `registration.rs` | 409 | 13 | 0 |
| `link.rs` | 751 | 15 | 0 |
| `peer.rs` | 382 | 14 | 0 |
| `services.rs` | 543 | 6 | 300 |
| `host.rs` | 359 | 10 | 300 |
| `runtime/access.rs` | 109 | 2 | 0 |
| `runtime/plugin.rs` | 667 | 23 | 350 |
| `runtime/local.rs` | 417 | 22 | 0 |
| `runtime/mod.rs` | 142 | 14 | 110 |
| `events/link.rs` | 795 | 2 | 700 |
| `transport/websocket/mod.rs` | 262 | 8 | 150 |
| `transport/memory/mod.rs` | 409 | 11 | 200 |
| `transport/local/mod.rs` | 206 | 13 | 150 |
| 新增 `session/plugin.rs`（Session、SessionHandle、`sessions`、id 表） | — | — | 600 |
| **rutis-bridge 小计** | **7234** | **244** | **3100** |
| rutis-loader `runtime/connections.rs` | 550 | 10 | 0 |
| `runtime.rs` | 1621 | 14 | 1450 |
| `peer.rs` | 998 | 14 | 450 |
| `runtime/discover.rs` | 53 | 0 | 53 |
| **rutis-loader 小计** | **3222** | **38** | **1953** |
| rutis-host `host.rs` | 2089 | 27 | 1800 |
| `config.rs` | 1434 | 42 | 1350 |
| `status.rs` | 979 | 18 | 850 |
| 新增 `connect.rs`（spawn、dial、listen） | — | — | 700 |
| **rutis-host 小计** | **4502** | **87** | **4700** |
| **合计** | **14958** | **369** | **约 9750** |

- rutis-bridge 根上导出的名字：现在 59 个，改后约 13 个（`Channel`、`Close`、`PeerId`、`Session`、`SessionHandle`、`RowHandle`、`thrown`、`fingerprint` 与节点 `host` 用的目录类型），估计。
- 生命周期路径从 4 条变 1 条，交接形状从 2 种变 1 种，准入、顶替、状态各从 3 份变 1 份。
- 测试：只测被删接口的文件删掉或改写，约 4000 行（`access_point.rs` 584、`revocation.rs` 798、`link.rs` 547、`fault_links.rs` 959、`memory_mux.rs` 730、`runtime_instances.rs` 703 等），行为测试改到新接口上，估计。

## 十、已合并的 PR 怎么处理

**结论：不 revert，在新实现里替换。** 理由：

- 混乱的主体（四条路径、两种形状、三份准入与状态）早于 0.19；revert #768、#791 只回到 0.18 的同样状态，什么都没解决。
- #791 的 `runtimes.remote` 写法、token 检查与《迁移到 0.19》在新架构里仍然保留，revert 再实现会让 CI、迁移指南、指南各多改一轮。

| PR | 保留 | 被替换 |
| --- | --- | --- |
| #768（R1 loader） | `discover.rs`（共用发现）；`Process::check_rows`（一处检查）；缺插件一律 `Unavailable` 与它的文字；`remote_modes.rs` 的用例（改为经 spawn / dial / listen 三个连接插件跑） | 两个调用点（`RuntimeResolver` 的远程分支、`RuntimeInstances`）合成一个按 session 找的解析器 |
| #791（R2 rutis-host） | `runtimes.remote` 的 `dial` / `listen` / `instances`；同一监听器与一台 `listen` 的 token 检查；`runtime <名>` 的状态行与 `status` 的 `connections:`；`control::serve` 的 `Report` 参数；《迁移到 0.19》（按 §十一改写）；`remote_runtimes.rs` 的大部分用例 | `transport` 字段与 `Embedding.transports`；`host.rs` 里用 `PeerPlugin { runtime }`、`AccessPointPlugin`、`AdmissionPlugin`、`SharedToken` 搭的连接，换成三个连接插件；"旧节点行写法照常可用"改为启动报错；用例 `a_link_row_still_connects_a_runtime` 改为报错用例，`an_added_transport_carries_instances` 改为"嵌入程序注册的连接插件承载多台" |
| #799 | — | 已关闭，不重开：弃用被直接删除代替。它带的 guarantees §15 计数重算（`2d817aa3`）在改 guarantees 的实施 PR 里重做 |
| 分支 `claude/remote-drop-transport`（未推送，`9a5543cc`） | 去掉 `transport` 字段、`Embedding.transports` 与相关测试、文档的改动 | 并入 §十三 的 S4，不单独推送 |

## 十一、兼容与《迁移到 0.19》

不变的：会话协议；各语言运行时与 SDK（Kotlin、Swift 应用不改代码）；rutis.json 的 `format` 仍为 1；`runtimes.remote` 的 #791 写法；`peer:` 行；输出的已有行。

不兼容的（全部写进《迁移到 0.19》"远程运行时"一节）：

1. **节点行的 `runtime` 删除**：改写到 `runtimes.remote`（已有对照表保留）。原文"旧写法照常可用，不用改，也不会被转换"与"还要这些字段的，继续用节点行"删掉，换成：写了 `runtime` 启动报错（§七的文字）；要在同一条连接上同时用 `export`、`host` 的，写一条节点行和一条 `runtimes.remote` 是两条连接，或者不需要运行时的行时只写节点行。
2. **节点行的 `import` 删除**（决定 1）：对方 `export` 的全部注册；要限制用 `isolate`；事件只看发出端的 `export`。给出改写例子。
3. **节点行的 `transport`、`identity`，`Embedding.identity` 删除**：自己的连接方式与凭据写成连接插件（§五），经 `Embedding.builtins` 注册。
4. **自建连接**："实现 `Transport` 挂到 `Embedding.transports`"一段改为"交出一条 `Channel`：写一个连接插件，调 `Session::new(channel)`；其他语言用 `sessions.open`"。`Embedding.transports` 这一条从迁移说明里删掉（它没有发布过）。
5. **Rust API**："rutis、rutis-loader、rutis-bridge 的 Rust API 都不变"改为一张对照表：

   | 0.18 | 0.19 |
   | --- | --- |
   | `LinkPlugin` + `PeerPlugin { runtime }` + `RuntimePlugin::remote` | 连接插件里 `Session::new(websocket::dial(...)?)`，行 `<名>:<插件>` |
   | `AccessPointPlugin` + `RuntimeInstances` | `rutis-host/listen` 的写法，或自己的连接插件 + `instanced` 分组 |
   | `ExportPlugin` / `ImportPlugin` | `Session::export`；接收方不写 |
   | `HostPlugin` | `Session::host(catalog)` |
   | `Identity` / `Admission` / `Revocations` | 连接插件自己的认证；`Session::admit` |
   | `Transport` | `websocket::dial` / `websocket::listen` / `pipe::spawn` 直接给出 `Channel` |
   | `Closer::replaced()` / `rejected(r)` | `close(Close::Replaced)` / `close(Close::Rejected(r))` |
   | `ConnectError`（`rutis_bridge::`） | `websocket::DialError` |

6. **持久化样本**：0.8–0.18 的样本里有 `import`、`runtime`、`transport`、`identity`，0.19 不再读。照 0.13 删除 `events` 的先例：样本里去掉这几处（期望同步），每种写法加一个 `rejected/` 用例，带 §七的文字；guarantees 里持久化样本那一条写明 0.19 的这一处例外。
7. **新输出**：§七标为"新增"的两行；`status` 的 `connections:` 每个 id 后列出远程插件。

## 十二、要同步改的文档

| 文档 | 改什么 |
| --- | --- |
| `docs/guide/nodes.md` / `.en.md` | `export` / `import` 表去掉 `import`，事件一节改为只看发出端；`runtime` 字段一行删除；"都不写，由节点行提供会话"删除；"实现一个 `Transport`"改为"写一个连接插件"；"安全"一节的吊销钩子改为"断开现有连接靠停掉那一行" |
| `docs/guide/rutis-host.md` / `.en.md` | `runtimes.remote` 表去掉 `transport` 与"都不写"一行；启动错误列表同步；`Embedding` 的 `transports`、`identity` 删除；"实现 Transport"改为连接插件 |
| `docs/guide/rust-host.md` / `.en.md` | `PeerPlugin`、`RuntimePlugin::remote` 加节点行的例子改为连接插件 + `Session::new` |
| `docs/guide/swift-app.md`、`kotlin-app.md`（两种语言） | 监听示例里带 `runtime` 的节点行改为 `runtimes.remote` 的 `dial`；其余不变 |
| `docs/guide/cordis.md`、`plugin-api.md`、`bun-plugin.md` 等提到 `instances` / 节点的地方 | 逐处核对用词，行为不变的不改 |
| `.agents/skills/rutis-remote-plugin/SKILL.md` 与 `references/nodes.md`、`rutis-json.md`、`plugin-api.md` | 同指南；"实现 Transport 挂到 Embedding.transports"改为"交出一条 Channel / 写连接插件"，加 `sessions.open` 的最小例子 |
| `.agents/skills/rutis-new-runtime/references/plugin-api.md` | 让已有运行时远程可用的一节：运行时一侧不变，rutis 一侧不再需要 `RuntimePlugin::remote` |
| `docs/migration-0.19.md` / `.en.md` | §十一 |
| `docs/guarantees.md` / `.en.md` | `bridge/revocation-closes-only-its-connections`、`bridge/revocation-refuses-reconnects` 改写为"名单变了后新连接被拒（升级时 403，或 hello 之后 4003）；断开现有连接靠停掉那一行"；`host/env-tokens-not-revocable` 同步；`host/remote-runtimes` 去掉 `transport` 与旧节点行；`loader/runtime-connection-instances` 改写为 listen + `instanced` 分组；`events/node-links`、`events/link-export-import`、`events/node-config` 改为只看 `export`，`node-config` 加 `import` 被拒；`loader/peer-rows` 去掉 `import`；`bridge/dial-out-rules` 里 4003 的来源改为 `admit`；每行的测试名随测试改；§15 计数重算 |
| `specs/remote-lease/`（README 两份、`AccessPoint.tla`、配置、`expected.txt`） | 去掉吊销动作与 `RevokedStaysOut`、`RevocationScoped` 两条性质，删 `access-revoke-same`、`access-revoke-distinct`、`access-mut-revoke-cuts-all` 配置；`NoStaleHandOver` 保留，说法改为"停掉的监听不再交出连接"；`access-mut-accept-before-release` 保留；"对应的代码"一表改指 `rutis-bridge/src/session/plugin.rs`（id 表）与 `rutis-host/src/connect.rs`（listen）；`RemoteLease.tla` 的吊销远端改为"停掉的 dial 行"，其余不变 |
| `crates/rutis-bridge/README.md`、`lib.rs` 的模块说明、根 `README` | 模块介绍改为 Channel、session、底层函数 |
| 旧设计稿 `design-remote-runtime-consistency-2026-10-26.md` / `.en.md` | 头部加一行"实施部分被 [远程连接：只留一个会话插件](design-remote-session-2026-10-28.md) 取代" |

## 十三、实施

先加后删，每个 PR 合并后 main 可用、CI 通过。

| PR | 内容 | 完成条件 | 依赖 |
| --- | --- | --- | --- |
| S1 rutis-bridge：Channel 与底层函数 | `Close`、`close(Close)`；删 `ChannelInfo`；`ConnectError` 移到 `websocket::DialError`；新增 `websocket::dial`、`websocket::listen`、`pipe::spawn`、`memory::pair`（现有 `Transport` 实现改为调用它们） | 现有测试不改用例通过（只改到 `close` 的调用）；新单元测试：三种 `Close` 在三种底层上的表现（4002、4003、stdin 关闭） | — |
| S2 rutis-bridge：session 插件 | `Session`、`SessionHandle::{id, run}`、`admit`、`export`、`host`；id 表与顶替；远程插件 `<id>:<插件>`；服务 `sessions` 与 `open`；旧的 link / 接入点不动 | 测试：先卸旧再接新（移植 `access-mut-accept-before-release` 的场景与 `an_id_revoked_while_its_connection_takes_over_is_not_accepted`）；对面先走 / 本地先走 / `admit` 拒绝 / 握手失败四种结束；Node 与 Python 测试插件经 `sessions.open` 接一条内存 Channel 跑通 `run` | S1 |
| S3 rutis-loader：统一解析器 | 行 `<名>:<插件>`、`peer:<对端>/<插件>` 按名字找 session 调 `run`；实例里找实例里的 session；旧路径并存 | `remote_modes.rs` 的用例在 session 上跑：spawn、dial、listen、listen + `instances` 四种接法缺插件文字逐字相同；实例服务的现有用例通过 | S2 |
| S4 rutis-host：三个连接插件 | `rutis-host/spawn`、`dial`、`listen`；rutis.json 展开成它们；状态合成两项，§七的输出；删 `transport` 字段、`Embedding.transports`、`Embedding.identity`（并入 drop-transport 分支）；节点行 `runtime`、`import`、`transport`、`identity` 报错；持久化样本与 `rejected/` 用例 | `remote_runtimes.rs`、`dial_in.rs`、`kotlin.rs` 端到端在新插件上通过；`dial_conformance` 各语言对 `rutis-host/listen` 通过；`persisted` 测试通过；e2e `fixture-host` 编译；#702 的 token 检查用例通过 | S3 |
| S5 删除 | 删 §八 全部条目与只测它们的测试；改写 TLA 模型并跑 `specs/remote-lease/run.sh` | `grep` §八 的名字在 `crates/` 下无结果；`cargo test --workspace` 与 clippy 通过；TLA `expected.txt` 更新并通过 | S4 |
| S6 文档 | §十二 全部条目；《迁移到 0.19》§十一 | 两种语言内容一致；文档里不再出现 `Transport`、`Embedding.transports`、`Revocations` 的承诺；guarantees 每行的测试名都存在 | S5（指南里与 S4 同时可见的行为可以随 S4 先改） |
