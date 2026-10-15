# 远程运行时：拨出与动态接入

[English](design-remote-runtime-dial-2026-10-14.en.md)

状态：定稿；R-P1（Rust 侧）、R-P2（Python 拨出）已实现，其余见 §七。日期：2026-10-14。基准：`main` `a763cdc`（0.10.0）。
依据：[远程插件](design-remote-plugins-2026-10-03.md)（本文修订其 §4 运行时接入的连接方式）、[连接节点](guide/nodes.md)（link）、[按需创建实例](design-loader-instance-mounts-and-services-2026-10-07.md)、[实例内服务名](design-instance-services-2026-10-08.md)、[Go 插件运行时](design-go-runtime-2026-10-10.md)、[Bun 运行时](design-bun-runtime-2026-10-09.md)。
使用者：[应用接入](design-app-integration-2026-10-14.md)、[Swift SDK](design-swift-sdk-2026-10-12.md)、[Android 应用托管](design-android-runtime-2026-10-12.md)。

范围外：身份、配对、吊销的实现（需要一个管理服务，不在本项目实现）；节点互联（`export` / `import` / `host` / `rows`）的连接方式；代码上传。

## 一、问题

现有远程运行时的前提是"几台固定的服务器"。在这个前提下没有问题，超出它就有三处限制：

| 限制 | 现状（`main` `a763cdc`） | 后果 |
| --- | --- | --- |
| 方向写死 | Python、Go、Node 的远程运行时只实现 `listen:`，由 rutis 拨入；Bun 运行时还不支持远程（监听与拨出都拒绝） | 远程机器必须能被 rutis 直接访问（固定地址、不在 NAT 后）；每种语言各自实现带 TLS 的 WebSocket 服务端，每台远程机器各自管证书（Python 3.13.6 的 TLS 死锁即其代价） |
| 拓扑静态 | 一个远程运行时 = 一行 link + 一条 `runtimes.remote` 声明 + 一个专属 token；实例里的 link 不能开 `runtime` | 数量会变的场景（扩缩容的 worker、多台设备）接不住 |
| 身份与连接耦合 | 监听方按每个对端的 token 认人，写在连接层 | 无法把身份交给独立的管理服务 |

## 二、决策

| # | 决策 | 章节 |
| --- | --- | --- |
| R1 | 每个远程运行时都支持两个方向：监听（rutis 拨入）与拨出（运行时连到 rutis 的接入点）；用哪个是部署选择，不是语言限制 | 三 |
| R2 | rutis 的一个接入点接受多条运行时连接；每条连接是一个实例（`instanced` 分组），连接进来创建、断开关闭 | 四 |
| R3 | 连接以握手时自报的 id 区分；同一 id 的新连接接管旧连接 | 五 |
| R4 | 身份是可替换的一层：过渡期用共享 token + 自报 id（rutis 不核实）；管理服务接入后换成核实过的 id，其余不变 | 五 |
| R5 | 拨出与重连行为是一份与语言无关的规范，由 `dial_conformance` 一致性测试约束 | 六 |
| R6 | 现有方式（静态配置、rutis 拨入监听的运行时）行为不变，不强制迁移 | 八 |
| R7 | 传输安全规则不变：非回环必须 TLS；拨出时 TLS 服务端只在 rutis 一侧 | 三 |
| R8 | 运行时侧全部用平台语言自己实现，不依赖 Rust（Python、Node、Bun、Go、Swift、Kotlin） | 七 |

## 三、方向

```text
现有：rutis ──拨入──▶ 运行时（监听，自带 TLS 服务端与证书）
新增：rutis（接入点，唯一的 TLS 服务端） ◀──拨出── 运行时 × N
```

- 拨出后，运行时一侧只需要 WebSocket **客户端**，各语言实现难度低于现有的服务端。
- rutis 对拨进来的运行时的控制权与现有一致：装载、配置、卸载、门控都由 rutis 驱动（远程插件 §4.1）。
- 运行时侧 channel 写作 `dial:wss://…`，与现有 `listen:wss://…` 对称；每种语言两种都支持。
- 选择：远程机器有固定地址、能被 rutis 直接访问时用监听；在 NAT 后、地址会变、数量会变（设备、扩缩容的 worker）时用拨出。
- 监听侧 link 与 `runtime` 的组合现在就可用（R-P0，2026-10-14）：运行时连进来后，rutis 照常发 `mount`、装载行、门控服务，断开即撤销；握手端到端核对运行时自报的 id 与 link 的对端。还没有运行时能拨出，测试用一个中继分别拨两端的监听来代替（`rutis-loader/tests/remote_rows.rs::a_runtime_link_can_listen`）。R-P1 无需为此改动 link。

## 四、动态接入即实例

- 一个接入点对应一个 `instanced` 分组：分组里的行描述"一条运行时连接里装哪些插件、用什么配置"。
- 连接进来 → 以连接 id 创建一个实例；断开 → 关闭该实例，实例内服务全部撤销，使用者停下。
- 运行时在 `mount` 回复里报它带的插件（`plugins`，每项与 `rows.schema` 回复相同）；分组里的行在某实例中指向该连接没有的插件时，这一行在该实例里失败并说明"该连接不含插件 X"，不影响其他实例。`engine` 之类在 `mount` 回复里加字段不算协议变化（Bun §3.3 先例）。
- 实例内的服务名在本实例内解析（实例内服务名设计，无回退），多条连接带同名插件互不冲突，不设别名；实例外的行不能使用某个实例的服务，需要时把使用者放进同一分组。
- 配置：`runtimes.remote` 的条目加 `listen`（接入点名）与 `instances`（`instanced` 分组 id），二者同时出现即为动态接入；分组里的行用 `<运行时名>:<插件>`，在每个实例里解析到该实例的连接。不需要另写 link 行：rutis-host 为这类条目在该监听器上起一个接入点（实现见下）。

  ```json
  {
    "listen": [{ "name": "public", "address": "0.0.0.0:7443", "cert": "server.pem", "key": "server.key" }],
    "runtimes": { "remote": [{ "name": "phone", "language": "kotlin", "listen": "public", "instances": "phone" }] },
    "rows": [
      { "id": "phone", "group": true, "instanced": true, "config": [
        { "id": "contacts", "name": "phone:contacts" },
        { "id": "digest", "name": "py:digest", "inject": ["contacts"] }
      ] }
    ]
  }
  ```

- 需要补的三处：
  1. 接入点接受多条连接（现在 link 一对一）；
  2. 连接触发实例创建与关闭（现在只有业务调用 `create_instance`）；
  3. 实例内允许运行时 link（现在 `RuntimeSession#<名>` 是全局键，`peer.rs` 拒绝），运行时键按实例区分。

- 实现（R-P1，#382）：
  - 接入点是 `rutis-bridge` 的 `AccessPointPlugin`（不是 link 行，`peer.rs` 对实例内 `runtime` link 的禁止保留）。每条连接由 `rutis-loader` 的 `RuntimeInstances` 处理：在连接的会话上挂载运行时，以连接（`RuntimeConnection`）为值 `create_instance`；释放时 `remove_instance`。连接的运行时进程属于它的实例，行直接持有它，不经运行时服务键，因此不需要按实例区分的 `RuntimeSession` 键。
  - `mount` 回复的 `plugins` 是 `{ "<插件>": <rows.schema 回复> }`，键是行名去掉 `<运行时名>:` 之后的部分；可选，没有时逐个问 `rows.schema`（远端 `NotFound` 即"不含"）。每条连接各读一次。
  - 连接的插件提供的服务发布在 `host_key_in(名字, 实例)`。共享名字在实例链上遇到提供它的连接时解析为该连接实例的键（Rust 行与各语言的行都一样），所以本地运行时的行（如 `py:digest`）也用到本连接的服务，并以实例标签隔离（需要 `scopes`）；连接没有提供的名字照常是全局的。
  - "该连接不含插件 X"是副本级的 `Unavailable`：只让这个副本失败；编辑的 dry run 不因它拒绝，reconcile 也不把它算作新失败而回滚。
  - `<运行时名>:` 行放在分组外时失败，说明它只在该分组里运行。

## 五、身份层

- rutis 侧把"认证并给出连接 id"抽成一层：输入握手里的凭据与自报 id，输出接受（连接 id）或拒绝（原因）。
- 过渡实现：凭据等于共享 token（环境变量 `RUTIS_TOKEN_<运行时名>`，与现有命名一致）即接受，连接 id = 自报 id；自报 id 须非空、只含 `[a-z0-9-]`。自报 id 只用于区分连接、命名实例，不作为身份凭据。
- 将来实现：管理服务核实凭据并给出 id；接入点、实例、配置不变。
- 现有每对端 token 的静态 link（R6）是这一层的另一种实现，保留。

## 六、拨出与重连规范

与语言无关，各语言按此实现：

| 项 | 规则 |
| --- | --- |
| 退避 | 可重试失败从 0.5 秒起翻倍至最长 30 秒，连接稳定后重置（与现有 link 一致） |
| 凭据被拒 | 每 30 秒重试。拨出方据此判断：升级时 401 / 403，或接入点在 hello 之后拒绝这个 id（WebSocket 关闭码 4003） |
| 协议不兼容 | 停止并报告。hello 未给出 id 或 id 不合法（`[a-z0-9-]+`）属于此类：重试修不好 |
| 断开 | rutis 侧关闭该连接的实例；运行时侧卸载全部行（清理逆序执行），然后按退避重连 |
| 重连 | 新会话、新实例、重新装载，不依赖上一次连接的状态 |
| 同 id 接管 | 新连接到达时旧连接被关闭，旧实例清理完再接入新连接 |
| 主动停止 | 运行时侧停止（如 SDK 的 `stop()`）：卸载全部行后关闭连接，不重连 |

一致性测试：`rutis-bridge/tests/dial_conformance.rs`，与 `websocket_conformance` 同形——rutis 侧起接入点，按语言拉起拨出方，逐条验证上表。

- 实现（R-P2，#383）：
  - 拨出方的约定（每种语言相同，`dial_conformance` 按此检查）：命令行 `dial:<地址> --id <id> --peer <rutis 的 id> <项目>`；凭据 `RUTIS_TOKEN`，额外信任的 CA `RUTIS_CA`；非回环的 `ws://` 与不合法的 id 启动时即拒绝（退出状态非零）。状态行写 stderr：`rutis: connected to …`、`rutis: cannot connect (…); retrying in <秒>s`、`rutis: connection lost (…); retrying in <秒>s`、`rutis: refused: auth-rejected (…); retrying in <秒>s`、`rutis: refused: incompatible (…); stopping`（随后以状态 1 退出）。SIGTERM / Ctrl-C 是主动停止，退出状态 0。
  - 等待时间与 `link.rs` 的 `Retry` 默认值相同：退避 ±20% 抖动；"连接稳定"是一次会话持续 60 秒；凭据被拒固定 30 秒；接入点 10 秒内不打招呼算可重试失败。测试用 `RUTIS_RETRY=<初始 ms>,<上限 ms>,<稳定 ms>,<被拒 ms>` 缩短，与 `RUTIS_HEARTBEAT` 同类，只供测试。
  - 分类与 Rust 拨出方相同，按类型或状态码，不看文字：升级时 401 / 403、本端拒绝服务端证书为凭据被拒；400 / 404 / 405 / 426、子协议不符、对端 hello 的版本本端不会说为不兼容；对端以不同于 `--peer` 的 id 打招呼为凭据被拒（同 `IdentityMismatch`）；其余（连不上、超时、断开、被接管 4002）可重试。接入点在拨出方收到 rutis 的 hello 之后才以 4003 拒绝 id，所以被拒前可能先出现一行 `connected to`。
  - 同 id 接管不设特例：被接管的一方按退避重连，又接管对方。两个进程用同一个 id 时会一直轮流顶掉对方，节奏随退避放慢到每方约 30 秒一次（会话不满 60 秒，退避不重置）。自报 id 不由 rutis 核实（§五），id 唯一由部署保证。
  - Python：`rutis/dialing.py`（规则与重连循环，纯 Python）、`rutis/websocket.py` 的 `dial`。连接由 websockets 的 asyncio 客户端在一个专用事件循环线程上完成全部读写：它的线程客户端从两个线程读写同一个 TLS socket，OpenSSL 不支持，实测约 5% 的 `wss://` 握手丢失响应。同一原因，拨出不受 CPython 3.13.6 的 TLS 死锁影响（监听仍拒绝该版本）。

## 七、各方工作

| 一方 | 工作 | 状态 |
| --- | --- | --- |
| Rust 侧 | 接入点接受多条连接、连接即实例、运行时键按实例区分、身份层与过渡实现、`mount` 回复的 `plugins`、rutis-host 配置（`listen` / `instances`）、`runtimes.remote[].language` 接受 `swift`、`kotlin` / `kt` | 已实现（#396、#399），`swift`、`kotlin` 随应用接入 |
| Python | 拨出客户端（`dial:`）；首个现有语言（其 TLS 服务端问题最多，拨出收益最大） | 已实现（#383），`dial_conformance` Python 列通过 |
| Node、Go | 拨出客户端，紧随 Python | 未实现（#384） |
| Bun | 监听与拨出都要补（现在都不支持），紧随 Python | 未实现（#384） |
| Swift、Kotlin | 监听与拨出都实现（Swift SDK §六、Android 稿 §五） | 未实现 |
| 文档 | 连接节点指南加拨出与动态接入；推荐方式改为拨出 | 已写拨出与动态接入（Python）；推荐方式待各语言都能拨出后再改 |

## 八、兼容

- 现有配置（`runtimes.remote` + link `dial` + `runtime`）行为不变。
- 运行时侧 `listen:` 是两个方向之一，长期保留。

## 九、阶段

| 阶段 | 内容 | 验收 |
| --- | --- | --- |
| R-P0 | 实测监听侧 link + `runtime` 现状 | 结论回填 §三 |
| R-P1 | Rust 侧：§七 的 Rust 工作；`dial_conformance` 框架 | 两条连接带同名插件，各自实例内的使用者只用到本连接的服务；断开一条只关闭其实例；现有远程运行时测试全绿 |
| R-P2 | Python 拨出 | Python 运行时拨出接入，`dial_conformance` Python 列过 |
| R-P3 | Node、Go 拨出；Bun 监听与拨出；Swift、Kotlin 随应用接入（Swift A1、Android K-P2） | 每种语言都过 `dial_conformance`（拨出）与 `websocket_conformance`（监听） |
