# 远程连接分层：远程插件层与连接插件

[English](design-remote-connection-layering-2026-10-26.en.md)

状态：定稿，未实现。日期：2026-10-26。基准：`main` `b4eee45e`（0.17.1）。排期：0.19。
Issue：[#695](https://github.com/arcships/rutis/issues/695)（实施 [#696](https://github.com/arcships/rutis/issues/696)–[#699](https://github.com/arcships/rutis/issues/699)）。决策记录：[远程连接分层](decision-remote-connection-layering-2026-10-26.md)。
取代：[远程插件](design-remote-plugins-2026-10-03.md) 中运行时接入的连接与管理部分（§3.1 的会话来源、§4.1 的连接方向、§4.4、§8 的远程语言插件一行）；[远程运行时：拨出与动态接入](design-remote-runtime-dial-2026-10-14.md) 中 rutis 一侧的部分（§三的 rutis 侧、§四、§五的接入点部分）。两篇中其余内容仍然有效，见 §十一。

范围外：本机运行时（`LocalRuntime`、Go / Swift 二进制运行时）的接入方式；节点互联（`export` / `import` / `host` / `rows`）；会话协议；各语言运行时一侧的 SDK（拨出、监听、重连规范不变）；身份与配对的管理服务。

## 一、问题

同一个远程语言运行时，只是换了谁主动连接，在 rutis 里的表现就不同：

| 同一个远程插件 | rutis 去连它（link 行 + `runtime`） | 它连进 rutis（接入点） |
| --- | --- | --- |
| 它提供的服务谁能用 | 所有行 | 只有同一分组里的行 |
| 远端没装某个插件 | 整行失败 | 只在这条连接上失败 |
| 运行状态输出 | 只有 link 行 | 什么都不打印 |
| 配置 | 两处：`runtimes.remote` + 一条 `rutis-bridge/peer` 行，两处之间不核对 | 一处 |
| `language` | 起作用 | 不起作用 |
| 重连后 | 只重新解析离线时解析过的行 | 每条连接重新描述 |

原因有两个：

1. **连接侧做了插件管理的事。** 接入点在连接进来后直接让 loader 创建实例，并处理同 id 的顶替；link 带着 `runtime` 开关，要求对方声明自己是运行时。
2. **插件管理侧写了两份。** rutis 去连的运行时走 `RuntimePlugin::remote` + `RuntimeResolver`；连进来的运行时走 `RuntimeInstances`，不经过 `RuntimePlugin`，发现、就绪和功能检查各写一遍。

后果：

- 连接方向本应只是部署选择（拨出稿 R1），现在却决定了服务的可见范围和出错方式。
- 宿主持有者想自建连接（经中继转发、接外部认证服务、换成 QUIC 或蓝牙）时没有入口：传输和认证可以替换，但"谁连谁、何时重连、几条连接"只有内置的两种写法。

## 二、决策

| # | 决策 | 章节 |
| --- | --- | --- |
| C1 | 分成两层：**远程插件层**负责发现、绑定、生命周期、握手；**连接插件**负责拨出或监听、认证、重连、传输方式 | 三 |
| C2 | 两层之间只交接"一条连接"：运行时名、连接 id、消息通道；连上时交出，断开时收回 | 四 |
| C3 | 远程插件层只有一份，所有连接方向、所有连接插件共用 | 五 |
| C4 | 一台还是多台由配置决定（`instances`），与连接方向无关 | 五 |
| C5 | 同 id 的新连接顶替旧连接，由远程插件层决定 | 五 |
| C6 | 握手和协议核对归远程插件层；连接插件不实现 rutis 协议，只交消息通道 | 四、五 |
| C7 | 连接对插件作者和写配置的人透明：配置只写运行时名、插件和内置连接的地址；连接插件由宿主挂载，不出现在行配置里 | 七 |
| C8 | 认证归连接插件；内置连接的认证层放开凭据类型，允许等待外部服务应答 | 六、九 |
| C9 | 运行时侧不变：会话协议、`listen:` / `dial:` 命令行、重连规范、`dial_conformance` / `websocket_conformance` 用例都不改 | 十 |
| C10 | 旧配置继续接受，启动时转换并提示新写法 | 十 |

## 三、两层

```text
远程插件层（rutis 统一一份）
  发现：对方带了哪些插件        绑定：哪些行跑在这条连接上
  生命周期：装、停、重连后重装    握手：核对协议版本与能力
        ▲
        │  交接：一条连接（运行时名、连接 id、消息通道；连上 / 断开）
        │
连接插件（可替换，可以有多个）
  拨出 / 监听    认证    重连    传输方式（WebSocket、中继、QUIC……）
```

| 事情 | 归属 | 现在在哪 |
| --- | --- | --- |
| 读 `mount` 回复、检查 `rows.v2` / `hosts`、读带的插件 | 远程插件层 | `RuntimePlugin` 与 `AcceptInstances` 各一份 |
| 按 `instances` 创建实例或提供全局 `Runtime` | 远程插件层 | 接入点的 `Accept` 回调 |
| 同 id 顶替 | 远程插件层 | 接入点、监听的 link |
| 对方自称是谁、是不是运行时 | 远程插件层（握手） | link 的 `require`、接入点的 `require("runtime")` |
| 何时拨号、退避、凭据被拒后等待 | 连接插件 | link |
| 凭据是否有效、吊销 | 连接插件 | `Identity` / `Admission` |
| 消息怎样送达 | 连接插件（借助 `Transport`） | `Transport` |

## 四、交接

远程插件层为每个远程运行时提供一个**连接登记处**服务，键是运行时名。连接插件依赖它，每建立一条消息通道就交一次：

```rust
// rutis-bridge：连接插件只依赖这里
pub trait RuntimeConnections: Send + Sync + 'static {
    /// 交出一条连接，握手完成后返回。
    fn offer(&self, offer: Offer) -> BoxFuture<'_, Result<Connected, Refused>>;
}

pub struct Offer {
    pub channel: Channel,
    pub who: Who,
}

pub enum Who {
    /// 对方必须以这个 id 打招呼（rutis 去连一个已知的运行时）。
    Known(PeerId),
    /// 对方自报 id，由这条准入规则决定接不接受、算作哪个连接 id（等它连进来）。
    Greeted(Arc<dyn Admission>),
}

/// 一条被接纳的连接。丢弃它 = 连接插件收回这条连接。
pub struct Connected { /* id(), ended() */ }

pub enum Refused {
    Incompatible(String), // 协议或能力不符：不再重试
    Rejected(String),     // 身份不符或准入拒绝：按凭据被拒等待
    Conflict(String),     // 单连接模式下已有另一个 id 的连接：可重试
    Retryable(String),    // 对方没打招呼、断开等
}
```

- `Connected::ended()` 在连接结束时给出原因与类别（同 `Refused`，外加"被同 id 的新连接顶替"），连接插件据此决定是否、何时重连。
- `offer` 返回前，远程插件层已完成握手、功能检查和发现；返回 `Connected` 时这条连接上的行已经开始装载。
- 为什么是"登记处"而不是"每条连接提供一个服务"：连接数量会变、id 由对方自报，每条连接一个服务键会让对方选择的名字占用应用的服务键（拨出稿 §四已有这个约束），也需要一套观察服务出现的机制。登记处由远程插件层提供，连接插件依赖它，正好是普通的依赖关系：运行时声明被移除时登记处撤销，连接插件随之停下并断开。
- 连接插件不读写消息内容，不实现 rutis 协议。拒绝时由远程插件层经通道的 `Closer` 告诉对方（4003 拒绝、4002 顶替），运行时侧看到的与现在相同。

## 五、远程插件层

一份实现，放在 rutis-loader（绑定要用 loader 的实例），接口定义在 rutis-bridge（连接插件不依赖 loader）。合并现在的 `RuntimePlugin::remote` + `RuntimeResolver` 远程分支与 `RuntimeInstances`。

每条连接：

1. **握手**：按 `Who` 核对对方 id，要求对方声明 `runtime` 能力，检查 `rows.v2`、`hosts`。不符即 `Refused`。
2. **发现**：读 `mount` 回复的 `plugins`；没有时按行逐个问 `rows.schema`。每条连接各读一次。
3. **绑定**，只看配置：

   | 运行时声明 | 一条连接进来 | 第二个 id 的连接 |
   | --- | --- | --- |
   | 没有 `instances` | 提供 `Runtime#<名>`，行按现有两段就绪（`Runtime` → `RuntimeRows` → 行），服务全局可见 | `Refused::Conflict`，说明已有哪条连接 |
   | 有 `instances` | 以连接 id 在该 `instanced` 分组里创建实例，行在实例里运行，服务只在实例内可见 | 另一个实例 |

4. **生命周期**：
   - 断开：撤销 `Runtime` 或关闭实例，使用者停下；运行时侧按现有租约清理。
   - 同 id 的新连接：先结束旧连接（停行、释放实例，旧的 `ended()` 报"被顶替"），再接纳新的。
   - 重连：重新发现，这个运行时的**所有行**按新描述重新解析，不再只刷新离线时解析过的行。远端插件升级后，新配置项得到校验。
5. **远端缺插件**：两种模式都是这一行在这条连接上不可用（`Unavailable`），说明"该连接不含插件 X"，不影响其他行和其他连接。

同一个运行时可以同时有多个连接插件交连接（例如内置监听加一个中继插件）：多连接模式下每条都是实例；单连接模式下先到的被接纳，其余是 `Conflict`。

## 六、连接插件

连接插件是普通 rutis 插件：依赖 `RuntimeConnections#<运行时名>` 和它自己需要的传输、认证，建立通道后 `offer`，按 `Refused` / `ended()` 的类别重连。

rutis 内置两个，替换现在的 link（运行时用法）与接入点：

| 内置连接插件 | 取代 | 认证 | 重连 |
| --- | --- | --- | --- |
| 拨出：rutis 去连运行时 | `rutis-bridge/peer` 行 + `runtime` | `Identity`（每个运行时一个 token） | 沿用 link 的退避与分类（0.5 秒起翻倍至 30 秒，60 秒稳定后重置；被拒每 30 秒；不兼容停止） |
| 监听：等运行时连进来 | `AccessPointPlugin` | `Admission`（共享 token + 自报 id） | 不主动重连，等对方再连 |

- 两者都在共享监听器的注册（`Registrations`）上工作，规则不变：一个监听器上节点 link、拨出与监听的运行时互不干扰。
- 吊销（#273）留在连接插件：被吊销的 id 由连接插件丢弃 `Connected`，之后的连接按凭据被拒拒绝，保证不变。
- 节点互联的 `LinkPlugin` 与 `PeerPlugin` 保持现状，只去掉 `runtime` 功能（`RuntimeAccessPlugin` 退休）。

## 七、配置

`runtimes.remote` 的一条写全一个远程运行时，不再需要额外的 link 行。三个字段互相独立：

| 字段 | 含义 |
| --- | --- |
| `dial` | 内置拨出：rutis 去连这个地址 |
| `listen` | 内置监听：在这个监听器上等它连进来 |
| `instances` | 多连接模式：每条连接是该 `instanced` 分组的一个实例 |

`dial` 与 `listen` 至多写一个；都不写时不起内置连接，连接由宿主挂的连接插件交来。

```json
{
  "id": "main",
  "listen": [{ "name": "public", "address": "0.0.0.0:7443", "cert": "server.pem", "key": "server.key" }],
  "runtimes": { "remote": [
    { "name": "gpu",    "language": "python", "dial": "wss://gpu.example.com:7443/rutis" },
    { "name": "mac",    "language": "swift",  "listen": "public" },
    { "name": "worker", "language": "python", "listen": "public", "instances": "worker" }
  ] },
  "rows": [
    { "id": "embedder", "name": "gpu:embedder" },
    { "id": "calendar", "name": "mac:calendar" },
    { "id": "worker", "group": true, "instanced": true, "config": [
      { "id": "chunker", "name": "worker:chunker" }
    ] }
  ]
}
```

- `mac` 是一台设备连进来、服务全局可见。现在要另写一条监听的 `rutis-bridge/peer` 行（带 `runtime`）；新写法只有这一条。
- token 规则不变：`RUTIS_TOKEN_<名字>`，没有时退回 `RUTIS_TOKEN`。同一个监听器上两个条目会接受同一个 token 时，启动即报错并说明给哪一个设单独的 token（现在这种情况是运行时 403）。
- `language` 在所有模式都起作用（例如 `node` 按 npm 名解析行名）。
- 行配置只出现运行时名，看不到连接。换连接方式（例如从 `listen` 换成中继插件）不改任何一行。

## 八、输出

`rutis-host run` 对每个远程运行时打印：

```text
runtime gpu: connected gpu
runtime gpu: link down (gpu.example.com:7443: Connection refused); retrying in 2.1s
runtime worker: connected worker-1
runtime worker: disconnected worker-1 (connection lost)
runtime mac: refused mac-2 (already connected: mac)
embedder: gpu does not contain plugin embedder
```

- `connected` / `disconnected` / `refused` 与缺插件由远程插件层给出，对所有连接插件都一样，包括连进来的连接（现在不打印）。
- `link …` 行是内置连接插件的重试状态，格式与现在相同，前缀从 link 行 id 改为 `runtime <名>`。宿主自建的连接插件自己决定输出什么。

## 九、宿主自建连接

宿主持有者在 Rust 里挂一个连接插件，交给某个运行时名：

```rust
let relay = RelayConnection::new("relay.example.com", token);   // 宿主自己写的插件
host.plugin(relay.for_runtime("gpu"));                          // 依赖 RuntimeConnections#gpu
```

- 只需要产生一条有序、可靠、保留消息边界的通道（现有 `Channel` 契约），不需要了解 rutis 协议。
- 认证放开：`Presented` 加入自定义凭据（例如请求头）；`Identity::verify` 与 `Admission::admit` 改为可等待，内置连接也能去问外部认证服务。
- 契约测试：rutis-bridge 提供一套连接插件契约测试（基于内存传输），自建插件照此自测：握手失败的类别、`ended()`、顶替、断开即收回。
- 示例：经中继服务器转发的连接插件，rutis 与运行时都连到中继，运行时侧不改。
- 在 rutis-host 下，`runtimes.remote` 不写 `dial` / `listen` 的条目等待其他插件（例如以 dylib 装的连接插件）交来连接。

## 十、兼容与迁移

- 运行时侧完全不变（C9）：已部署的 Python、Node、Bun、Go、Swift、Kotlin 运行时和应用不需要升级。
- 旧配置继续接受（C10）：
  - 带 `runtime` 的 `rutis-bridge/peer` 行：启动时转换成对应 `runtimes.remote` 条目的 `dial` 或 `listen`，并打印新写法。
  - `listen` + `instances`：写法不变，含义不变。
- 行为变化（写进《迁移到 0.19》）：
  - 输出前缀从 link 行 id 变为 `runtime <名>`；连进来的连接开始有输出。
  - rutis 去连的运行时缺插件时，从整行失败改为 `Unavailable`（与连进来的一致）。
  - 重连后所有行重新解析。
  - 同一监听器上的 token 冲突从运行时 403 改为启动报错。
- Rust API：`AccessPointPlugin`、`Accept`、`Accepted`、`RuntimeAccessPlugin`、`Features::runtime`、`RuntimePlugin::remote` 移除（不兼容）；替代是内置连接插件与 `RuntimeConnections`。

## 十一、取代的旧设计

| 旧文档 | 被取代的部分 | 仍有效的部分 |
| --- | --- | --- |
| [远程插件](design-remote-plugins-2026-10-03.md) | 运行时的会话来源与连接方向（§3.1 末条、§4.1 末条、§4.4 的连接方式）、§8 远程语言插件一行 | 运行时契约（§4.2）、两段就绪（§4.3）、租约语义（§4.4 的清理顺序）、节点互联（§5）、会话与兼容（§6）、生命周期与安全（§7） |
| [远程运行时：拨出与动态接入](design-remote-runtime-dial-2026-10-14.md) | §四的接入点与"连接即实例"的实现、§五的接入点部分、§七的 Rust 侧工作 | 两个方向都支持（R1）、身份过渡实现（§五）、拨出与重连规范（§六）、各语言运行时侧实现 |
| [网络栈](design-protocol-channel-decoupling-2026-10-03.md) | link 作为运行时会话来源的说法 | 通道契约、分帧、连接器、共享监听器注册、WebSocket 绑定 |

## 十二、阶段

| 阶段 | 内容 | 验收 |
| --- | --- | --- |
| L1 [#696](https://github.com/arcships/rutis/issues/696) | 交接接口与统一的远程插件层 | 同一组行测试经两种连接跑出相同结果；单连接模式拒绝第二个 id；远端升级后重连按新 schema 校验 |
| L2 [#697](https://github.com/arcships/rutis/issues/697) | 内置拨出、监听连接插件；接入点与 `RuntimeAccessPlugin` 退休 | `websocket_conformance`、`dial_conformance` 不改用例通过；吊销保证不变 |
| L3 [#698](https://github.com/arcships/rutis/issues/698) | rutis-host 配置、输出、旧配置转换、迁移指南与指南更新 | 三种写法与旧写法端到端；旧配置不改一字能跑 |
| L4 [#699](https://github.com/arcships/rutis/issues/699) | 认证放开、Rust 嵌入接口、中继示例、连接插件契约测试 | 中继示例接 Python 运行时端到端；要等外部应答的认证通过契约测试 |

0.20 的形式化覆盖（[#613](https://github.com/arcships/rutis/issues/613) 远程租约、[#624](https://github.com/arcships/rutis/issues/624) 接入点）按本设计覆盖。
