# 远程运行时：一条配置写全，一台与多台行为一致

[English](design-remote-runtime-consistency-2026-10-26.en.md)

状态：定稿，未实现。日期：2026-10-26。基准：`main` `b4eee45e`（0.17.1）。排期：0.19。
Issue：[#695](https://github.com/arcships/rutis/issues/695)（实施 [#696](https://github.com/arcships/rutis/issues/696) loader、[#698](https://github.com/arcships/rutis/issues/698) rutis-host）。决策记录：[远程运行时一致性](decision-remote-runtime-consistency-2026-10-26.md)。
修订：[远程运行时：拨出与动态接入](design-remote-runtime-dial-2026-10-14.md) §四"`listen` 与 `instances` 同时出现即为动态接入"一条，见 §六。

范围外：新的交接接口或"连接插件"一层、认证放开（自定义凭据、可等待的判断）、中继示例（理由见决策记录）；配置里可选的 `transport` 字段（2026-10-28 修订：现在没有任何自定义 `Transport`，宿主自建连接 [#699](https://github.com/arcships/rutis/issues/699) 已决定不做，这个字段没有用处；可替换的 `Transport` 接口留在代码里，以后真有需要再在配置里暴露）；本机运行时；节点互联；会话协议与各语言运行时一侧。

## 一、现状：已经分好的层

```text
插件管理    一台：RuntimePlugin + RuntimeResolver        多台：RuntimeInstances
               ▲ 依赖 RuntimeSession#<名>                    ▲ 实现接入点的 Accept 回调
连接管理    一台：link（rutis 去连，或监听等它连进来）    多台：接入点（监听，每条连接一个 id）
               ▲                                              ▲
传输        Transport#<kind>：WebSocket、本机进程、内存；宿主可以挂自己的
```

- 连接与插件管理之间有两个交接点：一台是 `RuntimeSession#<名>` 服务，多台是接入点的 `Accept` 回调。link 和接入点都不认识 loader。
- 消息怎样送达由 `Transport` 决定，可替换。中继、QUIC、蓝牙都是一种 `Transport`。

所以"连接与插件管理解耦"、"宿主自建连接"在结构上已经具备，缺的是一致性和配置入口。

## 二、问题

| # | 问题 | 现在 |
| --- | --- | --- |
| P1 | 一台要写两处配置 | `runtimes.remote` 一条 + 一条带 `runtime` 的 `rutis-bridge/peer` 行；两处之间不核对，少写一处行就一直等待 |
| P2 | 远端缺插件，一台与多台报错不同 | 一台：解析时 `NotFound`，整行失败；多台：`Unavailable`，只在这条连接上不可用 |
| P3 | 多台的连接没有输出 | 接入点有 `AccessEvent`，rutis-host 没有订阅 |
| P4 | 多台写死 WebSocket | rutis-host 给接入点固定 `"websocket"`；宿主自建的传输只能用于一台的 link（不处理，见 M4） |
| P5 | 发现与检查写了两份 | `RuntimePlugin` 与 `AcceptInstances` 各读一次 `mount` 回复、各查一次 `rows.v2` / `hosts`，离线占位也各有一份 |

服务的可见范围不同（一台全局可见，多台只在实例内）不是问题：这正是一台与多台的区别，由 `instances` 决定。

## 三、决策

| # | 决策 |
| --- | --- |
| M1 | 保持现有分层与两个交接点；不新增交接接口，不重写 link 与接入点 |
| M2 | `runtimes.remote` 一条写全一个远程运行时；rutis-host 在内部生成现在的 link 与运行时接入；旧的 link 行写法照常可用 |
| M3 | 一台还是多台由 `instances` 决定，谁主动连由 `dial` / `listen` 决定，二者独立 |
| M4 | 传输用已有的 WebSocket；可替换的 `Transport` 接口保留在代码里，不在配置里暴露（2026-10-28 修订，原为可选的 `transport` 字段，理由见范围外） |
| M5 | 一台与多台共用一份发现与检查；缺插件统一为 `Unavailable` |
| M6 | 多台的连接打印上线、断开、被拒 |
| M7 | 运行时侧、会话协议、Rust API 不变 |

## 四、配置

| `dial` | `listen` | `instances` | 含义 | rutis-host 内部 |
| --- | --- | --- | --- | --- |
| 地址 | — | — | 一台，rutis 去连 | 拨出的 link + 运行时接入 |
| — | 监听器 | — | 一台，等它连进来 | 监听的 link + 运行时接入；对方的 `--id` 须等于运行时名 |
| — | 监听器 | 分组 | 多台，每条连接一个实例 | 接入点（现状） |
| — | — | — | 不起连接，等别处提供 `RuntimeSession#<名>` | 无（现状，供 Rust 嵌入） |

- `dial` 与 `listen` 同时写、`dial` 与 `instances` 同时写：启动报错（rutis 去连的只能是一台）。
- 连接走 WebSocket；没有 `transport` 字段（M4）。
- token 规则不变：`RUTIS_TOKEN_<名字>`，没有时退回 `RUTIS_TOKEN`。

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

`gpu` 与 `mac` 现在各要另写一条 `rutis-bridge/peer` 行；新写法只有 `runtimes.remote` 这一条。

## 五、行为

- **发现与检查（M5）**：一份代码完成"读 `mount` 回复 → 检查 `rows.v2` / `hosts` → 读带的插件，没有时问 `rows.schema`"。一台由 `RuntimePlugin` 调用，多台由 `RuntimeInstances` 调用。
- **缺插件（M5）**：两种模式都是 `Unavailable`，说明"<运行时> 不含插件 X"；编辑的 dry run 不因它拒绝，其他行不受影响。
- **输出（M6）**：

  ```text
  runtime gpu: link connected
  runtime gpu: link down (gpu.example.com:7443: Connection refused); retrying in 2.1s
  runtime worker: connected worker-1
  runtime worker: disconnected worker-1 (connection lost)
  runtime worker: refused (auth-rejected)
  ```

  一台沿用现在 link 的状态行，前缀从 link 行 id 改为 `runtime <名>`（旧写法的 link 行仍用行 id）；多台订阅接入点的 `AccessEvent`。
- 断开、重连、同 id 顶替、吊销：与现在相同。

## 六、与旧设计

- [远程运行时：拨出与动态接入](design-remote-runtime-dial-2026-10-14.md) §四"`runtimes.remote` 的条目加 `listen` 与 `instances`，二者同时出现即为动态接入"改为本稿 §四：三个字段独立，`listen` 不带 `instances` 是一台。该稿其余内容不变。
- [远程插件](design-remote-plugins-2026-10-03.md)、[网络栈](design-protocol-channel-decoupling-2026-10-03.md)、[应用接入](design-app-integration-2026-10-14.md) 不受影响。

## 七、兼容

- 旧配置不改一字能跑；运行时侧不需要升级；Rust API 不变。
- 行为变化（写进《迁移到 0.19》）：一台模式下远端缺插件从整行 `NotFound` 改为 `Unavailable`；新写法的状态行前缀为 `runtime <名>`；多台的连接开始有输出。

## 八、实施

| Issue | 内容 | 验收 |
| --- | --- | --- |
| [#696](https://github.com/arcships/rutis/issues/696) loader | M5 | 同一组行测试经 link dial、link listen、接入点三种接法跑，缺插件报错一致；`ROW_FEATURES` 检查只剩一处；现有远程测试与 `websocket_conformance`、`dial_conformance` 不改用例通过 |
| [#698](https://github.com/arcships/rutis/issues/698) rutis-host | M2、M3、M4、M6，文档与迁移指南 | `dial`、`listen`、`listen` + `instances`、旧 link 行四种写法端到端（2026-10-28 修订：自定义 `Transport` 的验收随 `transport` 字段删去） |

审阅中另发现两个独立的小问题，不在本稿：[#701](https://github.com/arcships/rutis/issues/701) 重连后连着时解析过的行不重新解析（0.17.2），[#702](https://github.com/arcships/rutis/issues/702) 同一监听器上 token 冲突改为启动报错（0.18）。
