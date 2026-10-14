# 远程运行时：拨出与动态接入

[English](design-remote-runtime-dial-2026-10-14.en.md)

状态：定稿，未实现。日期：2026-10-14。基准：`main` `a763cdc`（0.10.0）。
依据：[远程插件](design-remote-plugins-2026-10-03.md)（本文修订其 §4 运行时接入的连接方式）、[连接节点](guide/nodes.md)（link）、[按需创建实例](design-loader-instance-mounts-and-services-2026-10-07.md)、[实例内服务名](design-instance-services-2026-10-08.md)、[Go 插件运行时](design-go-runtime-2026-10-10.md)、[Bun 运行时](design-bun-runtime-2026-10-09.md)。
使用者：[应用接入](design-app-integration-2026-10-14.md)、[Swift SDK](design-swift-sdk-2026-10-12.md)、[Android 应用托管](design-android-runtime-2026-10-12.md)。

范围外：身份、配对、吊销的实现（需要一个管理服务，不在本项目实现）；节点互联（`export` / `import` / `host` / `rows`）的连接方式；代码上传。

## 一、问题

现有远程运行时的前提是"几台固定的服务器"。在这个前提下没有问题，超出它就有三处限制：

| 限制 | 现状（`main` `a763cdc`） | 后果 |
| --- | --- | --- |
| 方向写死 | Python、Go、Bun、Node 的远程运行时只实现 `listen:`，由 rutis 拨入；Bun 明确拒绝拨出 | 远程机器必须能被 rutis 直接访问（固定地址、不在 NAT 后）；每种语言各自实现带 TLS 的 WebSocket 服务端，每台远程机器各自管证书（Python 3.13.6 的 TLS 死锁即其代价） |
| 拓扑静态 | 一个远程运行时 = 一行 link + 一条 `runtimes.remote` 声明 + 一个专属 token；实例里的 link 不能开 `runtime` | 数量会变的场景（扩缩容的 worker、多台设备）接不住 |
| 身份与连接耦合 | 监听方按每个对端的 token 认人，写在连接层 | 无法把身份交给独立的管理服务 |

## 二、决策

| # | 决策 | 章节 |
| --- | --- | --- |
| R1 | 运行时可主动拨出到 rutis 的接入点；推荐拨出，运行时侧监听长期保留 | 三 |
| R2 | rutis 的一个接入点接受多条运行时连接；每条连接是一个实例（`instanced` 分组），连接进来创建、断开关闭 | 四 |
| R3 | 连接以握手时自报的 id 区分；同一 id 的新连接接管旧连接 | 五 |
| R4 | 身份是可替换的一层：过渡期用共享 token + 自报 id（rutis 不核实）；管理服务接入后换成核实过的 id，其余不变 | 五 |
| R5 | 拨出与重连行为是一份与语言无关的规范，由 `dial_conformance` 一致性测试约束 | 六 |
| R6 | 现有方式（静态配置、rutis 拨入监听的运行时）行为不变，不强制迁移 | 八 |
| R7 | 传输安全规则不变：非回环必须 TLS；拨出时 TLS 服务端只在 rutis 一侧 | 三 |

## 三、方向

```text
现有：rutis ──拨入──▶ 运行时（监听，自带 TLS 服务端与证书）
新增：rutis（接入点，唯一的 TLS 服务端） ◀──拨出── 运行时 × N
```

- 拨出后，运行时一侧只需要 WebSocket **客户端**，各语言实现难度低于现有的服务端。
- rutis 对拨进来的运行时的控制权与现有一致：装载、配置、卸载、门控都由 rutis 驱动（远程插件 §4.1）。
- 运行时侧 channel 写作 `dial:wss://…`（与现有 `listen:wss://…` 对称）；只拨出的 SDK（Swift、Kotlin）对 `listen:` 报错。
- 监听侧 link 与 `runtime` 的组合在现有代码中未被拒绝、未实测：R-P0 实测，不通则在 R-P1 一并补上。

## 四、动态接入即实例

- 一个接入点对应一个 `instanced` 分组：分组里的行描述"一条运行时连接里装哪些插件、用什么配置"。
- 连接进来 → 以连接 id 创建一个实例；断开 → 关闭该实例，实例内服务全部撤销，使用者停下。
- 运行时在 `mount` 回复里报它带的插件（`plugins`，每项与 `rows.schema` 回复相同）；分组里的行在某实例中指向该连接没有的插件时，这一行在该实例里失败并说明"该连接不含插件 X"，不影响其他实例。`engine` 之类在 `mount` 回复里加字段不算协议变化（Bun §3.3 先例）。
- 实例内的服务名在本实例内解析（实例内服务名设计，无回退），多条连接带同名插件互不冲突，不设别名；实例外的行不能使用某个实例的服务，需要时把使用者放进同一分组。
- 配置：`runtimes.remote` 的条目加 `listen`（接入点名）与 `instances`（`instanced` 分组 id），二者同时出现即为动态接入；分组里的行用 `<运行时名>:<插件>`，在每个实例里解析到该实例的连接。不需要另写 link 行：rutis-host 为这类条目生成监听侧 link。

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
| 凭据被拒 | 每 30 秒重试 |
| 协议不兼容 | 停止并报告 |
| 断开 | rutis 侧关闭该连接的实例；运行时侧卸载全部行（清理逆序执行），然后按退避重连 |
| 重连 | 新会话、新实例、重新装载，不依赖上一次连接的状态 |
| 同 id 接管 | 新连接到达时旧连接被关闭，旧实例清理完再接入新连接 |
| 主动停止 | 运行时侧停止（如 SDK 的 `stop()`）：卸载全部行后关闭连接，不重连 |

一致性测试：`rutis-bridge/tests/dial_conformance.rs`，与 `websocket_conformance` 同形——rutis 侧起接入点，按语言拉起拨出方，逐条验证上表。

## 七、各方工作

| 一方 | 工作 |
| --- | --- |
| Rust 侧 | 接入点接受多条连接、连接即实例、运行时键按实例区分、身份层与过渡实现、`mount` 回复的 `plugins`、rutis-host 配置（`listen` / `instances`）、`runtimes.remote[].language` 接受 `swift`、`kotlin` / `kt` |
| Python | 拨出客户端（`dial:`）；首个现有语言（其 TLS 服务端问题最多，拨出收益最大） |
| Go、Bun、Node | 拨出客户端；有需求时做 |
| Swift、Kotlin | 只实现拨出（Swift SDK §六、Android 稿 §五） |
| 文档 | 连接节点指南加拨出与动态接入；推荐方式改为拨出 |

## 八、兼容

- 现有配置（`runtimes.remote` + link `dial` + `runtime`）行为不变。
- 运行时侧 `listen:` 长期保留，不计划弃用。

## 九、阶段

| 阶段 | 内容 | 验收 |
| --- | --- | --- |
| R-P0 | 实测监听侧 link + `runtime` 现状 | 结论回填 §三 |
| R-P1 | Rust 侧：§七 的 Rust 工作；`dial_conformance` 框架 | 两条连接带同名插件，各自实例内的使用者只用到本连接的服务；断开一条只关闭其实例；现有远程运行时测试全绿 |
| R-P2 | Python 拨出 | Python 运行时拨出接入，`dial_conformance` Python 列过 |
| R-P3 | Swift、Kotlin 随应用接入（Swift A1、Android K-P2）；其余语言有需求时 | 各语言过同一套测试 |
