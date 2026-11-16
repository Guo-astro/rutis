# 规则：协议清单 remote（#634 评论）

[中文索引](../README.md) · [English](../README.en.md) · 来源：[评论 6384934401](https://github.com/arcships/rutis/issues/634#issuecomment-6384934401) · [保证登记表](../../guarantees.md)

本文件把 #634 的 `remote` 分区评论（[评论 6384934401](https://github.com/arcships/rutis/issues/634#issuecomment-6384934401)）里的 Markdown 表格导出到仓库。表格内容原样保留；单元格里的 `|` 按 Markdown 表格规则转义为 `\|`，渲染后文字不变。列的含义见 #634 正文：编号、来源、规则、参与方、手段、对应、现有形式化、状态、归属 issue、备注。

抽取基线：`main` `3496669b`（issue 正文的统计口径）。**这批行没有按 0.22 的现行规格复核**：状态列仍是 issue 的词汇，`node tools/formal-trace.mjs` 在统计时按 `README.md` §3 的映射换成追踪表状态；映射只降不升，不改写本文件。

补充评论 6415164837：远程运行时在 0.19 补齐一致性（#695，PR #700）；link、接入点、远程租约的规则本身不变，追踪表按 #695 之后的行为覆盖（#613、#624）。本文件的表格没有逐条体现这次变化。

分区合计：现行 231 条，已取代 17 条，共 248 行（与 issue 正文的统计表一致）。

## 分区 `remote`

> 分区 `remote`：远程运行时、拨出、应用托管与节点（REM、DIA、APP、NOD）
>
> 现行 231 条，已取代 17 条（列出、不计入）。"归属 issue"一列带 ※ 的是这次新分配或调整的。列的含义见 #634 正文。

| 编号 | 来源 | 规则 | 参与方 | 手段 | 对应 | 现有形式化 | 状态 | 归属 issue | 备注 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| REM-1-1 | REM §1 端点表；§3.3 末条；§10 远程运行时 | 远程运行时的行与本机运行时的行按同一管理语义运行（由控制方 rutis + loader 管理行、依赖、配置、生命周期），会话来源不改变管理语义 | 跨进程 | K | loader/remote-runtime-rows | — | 未覆盖 | #639 ※ | "本机与远程使用同一组行测试"（§10）没有一致性套件形式；`remote_rows.rs` 为普通测试 |
| REM-1-2 | REM §1；§4.4 第2条；§10 | 叶子执行端（如只装 Python 的远程机器）不需要部署 rutis 或本地依赖图即可被管理 | 外部 | 测试 | loader/remote-runtime-rows | — | 不适用 | — | 部署条件；`remote_rows.rs::a_remote_python_runtime_runs_loader_rows` |
| REM-1-3 | REM §1 | 完整框架节点各自管理自己的插件，对端只能经 host 代装 | 跨进程 | K | loader/peer-rows | K-node（plugins.load/update/unload） | 部分 | #639 ※ | 只查代装本身；"节点独立插件不受对端影响"无形式化（节点托管行 T6 暂缓，#612） |
| REM-1-4a | REM §1 | 明确不做：代码分发、插件市场、部署平台、操作系统沙箱、独立强制终止 | 外部 | 测试 | — | — | 不适用 | — | 范围声明，未登记为明确不保证 |
| REM-1-4b | REM §1；§7 表"本机进程退出"；§10 | rutis 不自动重启运行时进程 | — | — | host/runtime-restart | — | 已取代 | — | 被 rutis-host 的 `restart: on-failure`（`docs/guide/rutis-host.md`「运行时结束时」）取代；默认 `no` 仍不重启；远程运行时进程见 REM-4.4-3 |
| REM-1-4c | REM §1"会话恢复"；§4.4 末条；§7 表"重连" | 不做会话恢复：重连是新会话，不恢复旧引用、不重放结果未知的调用 | 跨进程 | TLA + K | bridge/no-call-on-a-dead-session、bridge/in-flight-call-fails-once | RL `OldCommandsStayOld`；K-leases `*_reconnecting_controller_gets_a_new_lease` | 部分 | #615、#626 | 租约模型只建行指令，不建引用和在途调用；"不重放"由 #626 `SettlesOnce` 计划 |
| REM-1-4d | REM §1"失联宽限期" | 没有失联宽限期：会话一结束执行端即清理租约，不等对端回来 | 跨进程 | TLA | loader/remote-runtime-lease | RL `RRead`(close) 即清理 | 部分 | #615 | 模型把清理当作瞬时；Node/Bun 接管时旧子进程 5 秒后被强杀（`node/rutis-runtime/src/serve.mjs:103`、`bun/rutis-bun/src/remote.ts:179`） |
| REM-2-1 | REM §2 叶子侧"已有"；§4.4 第3条；§7 末段 | 本机父进程拉起的叶子运行时：会话结束后清理全部行并退出；一个进程只服务一个会话 | 跨进程 | K | host/killed-host-leaves-no-runtimes、loader/row-unload-leaves-nothing | K-hg（Node、Python、Go、Bun 列） | 部分 | #624 | 缺 Swift、Kotlin 列；Go 与 Swift 的本地通道不理 SIGTERM 待确认（#624） |
| REM-2-2 | REM §2 网络接入；§4.4 第1条；§7 安全第6条；§10 远程运行时 | 远程运行时只接受配置并验证通过的控制方（监听与承载鉴权）；未验证的控制方被拒绝 | 跨进程 | K | bridge/websocket-refusal-categories | — | 未覆盖 | #639 ※ | RL 把升级成功当作已鉴权，未建鉴权失败；`websocket_cross.rs` 是普通测试，只有 Rust 拨 Python/Node，Go、Bun、Swift、Kotlin 监听端无对照 |
| REM-2-3 | REM §2；§3 共享表；§4.4 第1条；NOD「别的机器上的语言运行时」 | 网络叶子运行时常驻，依次服务多个控制会话，每个实例同一时刻只有一个活动控制会话 | 跨进程 | TLA + K | loader/remote-runtime-lease | RL `OneLease`（`lease-healthy`、`lease-faults`）；K-leases `*_successive_controllers_get_clean_leases` | 部分 | #615、#624 | 模型对应表只列 Python/Go/Node（Bun"同"）；Swift `Remote/Listen.swift`、Kotlin `internal/remote/Listen.kt` 未核对（#615），`leases.rs` 缺 Swift/Kotlin 列（#624） |
| REM-2-4 | REM §2；§4.4 第3条 | 网络叶子运行时会话结束后清理租约并继续监听，不退出 | 跨进程 | TLA + K | loader/remote-runtime-lease | RL（close 后清理、回到空闲）；K-leases `*_successive_*` | 部分 | #615、#624 | 同 REM-2-3 的语言缺口 |
| REM-2-5 | REM §2 网络接入末项 | 网络承载下叶子只监听、不持有重连退避，重连由控制方 link 负责 | — | — | — | — | 已取代 | — | 被 DIA R1、§六 取代：运行时可拨出并自己持有退避（`python/rutis/rutis/dialing.py`、`go/rutis/dial.go` 等） |
| REM-2-6 | REM §2 crate 归属 | 核心 `rutis` 不增加网络、运行时或重连概念，不反向依赖扩展 | 外部 | 测试 | — | — | 不适用 | — | 构建依赖关系由编译保证 |
| REM-2-7 | REM §2；§10 公共机制 | `rutis-bridge` 不依赖 `rutis-loader`；link 不查询 schema、不刷新行、不提供 RuntimeRows/PeerRows；任何行未解析时仍可建立会话 | 外部 | 测试 | loader/peer-rows | — | 不适用 | — | 构建依赖由编译保证；"行未解析也能建会话"有普通测试 |
| REM-2-8 | REM §2 crate 归属 | channel、interop、各 transport、runtime-local 为独立 crate；bridge 不依赖任何具体 transport crate | — | — | — | — | 已取代 | — | 2026-10-06 合并为 `rutis-bridge` 的模块（文档页首注；`crates/` 下无这些 crate），具体承载改为 feature（`crates/rutis-bridge/Cargo.toml` 的 `websocket`） |
| REM-2-9 | REM §2 末段；§10 公共机制 | 只用本机运行时不带入 WebSocket 依赖；memory 承载只由测试或显式配置选用 | 外部 | 测试 | — | — | 不适用 | — | feature 组合，由 CI 的 feature 矩阵检查 |
| REM-2-10 | REM §2 承载 crate 段；§3.2 末条；§10 公共机制 | 承载插件校验配置、提供 Transport、持有资源，随卸载撤销服务并关闭其连接；依赖它的 link 经原生门控停下 | 进程内并发 | 测试 | — | — | 不适用 | — | 涉及真实套接字与子进程：`memory_plugin.rs`、`local_plugin.rs`、`websocket.rs` 的装卸测试；门控本身归内核（#622） |
| REM-2-11 | REM §2 `rutis-transport-local` | 本机承载：通道拥有进程，通道的结束原因说明进程怎样结束 | 进程内并发 | SH | bridge/spawned-processes-own-channel | — | 未覆盖 | #623 | #623 计划 `transport/local/spawn.rs` 的 shuttle 模型 |
| REM-2-12 | REM §2 `rutis-transport-local` | 本端关闭通道后宽限 2 秒再结束进程 | 外部 | 测试 | — | — | 不适用 | — | 计时（`spawn.rs:982` `GRACE`）；`local_spawn.rs` 有相关用例；#633 的计时清单不含此项 |
| REM-3.1-1 | REM §3.1 | 承载插件提供 `Transport#<种类>`；网络 link 依赖 Transport 与 Identity；本机执行端身份由拉起方指定 | 进程内并发 | 测试 | — | — | 不适用 | — | 服务键与依赖声明，由编译与装配测试保证 |
| REM-3.1-2 | REM §3.1；§4.3 第1步；§7 表"会话就绪" | link 在身份与协议握手完成后才提供 `Peer#<id>`；Peer 只表示会话就绪，不表示授权 | 跨进程 | TLA + TV | bridge/no-call-on-a-dead-session | RL `ReadyIsCurrent`（变异 `lease-mut-late-hello`）、`RevokedStaysOut` | 部分 | #615、#624 | RL 只建拨号侧；监听的静态 link 只作为接入点的特例（README"简化"），无单独模型；#624 的 TV 只做接入点 |
| REM-3.1-3 | REM §3.1；§5.1 | 功能插件与运行时 Adapter 在 Peer 上登记控制操作；处理器随所属插件卸载注销，不影响会话 | 进程内并发 | SH | — | — | 未覆盖 | #628 | #628 要求抓住 #437（`Peer::register` 跨两把锁） |
| REM-3.1-4 | REM §3.1；§6.1 第1条 | 同一链接明确选择运行时或节点接入契约，不对同一实例同时启用两套管理归属；运行时用 rows/hosts，节点用 plugins/services/events | 纯逻辑 | PT | — | — | 未覆盖 | #631 ※ | 配置校验（`rutis-loader/src/peer.rs`），无 PT 计划 |
| REM-3.1-5 | REM §3.1 | 本机兼容模式 link：协议 2、不校验端点契约、不发 `link.offers`、会话结束后不重连 | 纯逻辑 | PT | — | — | 未覆盖 | #631 | `link.rs` `local_runtime`、`Greet::open`；#631 的握手判定表。注：compat 握手现在必须宣告 `events-v2`（`session/rpc.rs` 约 2060 行），"不校验契约"只指 `runtime`/`node` |
| REM-3.1-6 | REM §3.1 | 本机运行时进程结束时 link 停止，运行时状态为 `Down(原因)`；`LocalRuntime::restart` 拉起新进程 | 进程内并发 | SH | host/runtime-restart | — | 未覆盖 | #623、#627 | #627 本地运行时监督的 shuttle；结束原因见 #623 |
| REM-3.1-7 | REM §3.1 | `LocalRuntime` 的 apply 等到运行时就绪或 link 停止；启动期间可 dispose/restart；启动失败即失败 | 进程内并发 | SH | loader/runtime-start-failure-isolated | — | 未覆盖 | #627 | #627 疑点：`RuntimeHandle::changed()` 丢唤醒，apply 可能卡住 |
| REM-3.1-8 | REM §3.1 末句 | `RuntimePlugin::node`/`python`/`launcher` 自行拉起进程的路径保留为弃用兼容层 | — | — | — | — | 已取代 | — | 已移除：`RuntimePlugin` 只剩 `remote`、`session`（`crates/rutis-bridge/src/runtime/plugin.rs:214–221`），拉起归 `LocalRuntime` |
| REM-3-1 | REM §3"Session 共享"首段；§10 公共机制 | Session 按运行时实例或节点 link 建立；插件装卸不创建、不关闭共享 Session | 跨进程 | K | loader/peer-rows、loader/remote-runtime-rows | — | 未覆盖 | #639 ※ | `peer_rows.rs::hosted_rows_share_the_session_of_their_link`、`remote_rows.rs::remote_rows_share_the_runtime_session` 是普通测试 |
| REM-3-2 | REM §3 共享表 | 每个运行时实例一个活动控制 Session；每个节点 link 一个活动 Session | 跨进程 | TLA | loader/remote-runtime-lease | RL `OneLease`（运行时侧）、`ReadyIsCurrent`（link 只在当前连接上 Ready） | 部分 | #615 | 节点 link 监听侧的接管未建模 |
| REM-3-3 | REM §3 共享 | 调用、服务与对象引用按各自 ID 路由，实例 key 与引用只在所属会话的命名空间内解释 | 跨进程 | TLA + PT | bridge/call-chains-across-processes | CRS `RoutingSound`（`current-norebase` 违反） | 部分 | #626、#631 | CRS 只覆盖同步调用链的反向路由；#631 path 改写 PT，#626 调用表 |
| REM-3-4 | REM §3 共享；§10 公共机制 | 卸载业务插件只清理其资源；卸载桥功能只注销其操作并清理所属资源；都不关闭共享 Session，不清理无关插件 | 跨进程 | TLA | loader/row-unload-leaves-nothing、loader/peer-rows | — | 未覆盖 | #625 | #625 只建运行时行（`WithdrawBeforeUnload`）；节点桥功能一半无计划 |
| REM-3-5 | REM §3 共享；§4.4 第2条；§7 表"断链" | link 结束时统一关闭 Session；运行时接入清理该会话租约下全部行 | 跨进程 | TLA + K | loader/remote-runtime-lease | RL `OneLease`、`OldCommandsStayOld`；K-leases | 部分 | #615、#624 | 同 REM-2-3 的语言缺口 |
| REM-3-6 | REM §3 共享；§7 表"断链"；§7 末段第2条；§10 节点互联 | 节点 link 结束时清理该链接代装和导入导出的资源，不卸载节点独立管理的插件 | 跨进程 | TLA | loader/peer-rows | — | 未覆盖 | #640 ※ | 节点托管行（T6）按 #612 在 peer 设计恢复前不建模；`peer_rows.rs::rows_stop_with_the_host_and_the_link_and_the_peer_stays` 普通测试 |
| REM-3-7 | REM §3 共享；§4.1；§4.2 `origin()` 句；§10 远程运行时 | 同运行时（按会话 tag 判断，不依赖本机进程对象）服务交付原生对象，不经网络；跨运行时或 Rust 调用经控制方 rutis 路由 | 跨进程 | K | loader/services-by-name | — | 未覆盖 | #639 ※ | 无一致性用例检查"同运行时拿到原生对象"；判断见 `runtime/rows.rs:81` |
| REM-3.2-1 | REM §3.2 | 每个 Session 一个逻辑 Channel；物理连接的池化、复用、多路复用归承载 Adapter | 进程内并发 | 测试 | — | — | 不适用 | — | 职责划分；`memory_mux.rs` |
| REM-3.2-2 | REM §3.2 | 连接器每次调用只返回一次逻辑 Channel 建立结果；拨号侧 link 唯一持有会话重建的退避状态，Adapter 不另起重试循环、不恢复已终止的 Channel | 进程内并发 | 测试 | — | — | 不适用 | — | 接口契约，channel 契约测试 |
| REM-3.2-3 | REM §3.2；§4.4 末条；§7 末段第1条 | Adapter 与 link 不重放结果未知的调用 | 跨进程 | TLA | bridge/in-flight-call-fails-once | — | 未覆盖 | #626 | #626 `SettlesOnce` |
| REM-3.2-4 | REM §3.2；§10 公共机制；DIA §六 实现（"按类型或状态码，不看文字"） | 重试策略只由结构化结果（Retryable / AuthRejected / Incompatible）决定，诊断文字不参与 | 纯逻辑 | PT + K | bridge/websocket-refusal-categories、bridge/dial-out-rules | K-dial（403、4003→auth-rejected，400→incompatible，6 列） | 部分 | #615、#631 | Rust 侧分类（`link.rs` `classify`、`transport/websocket/dial.rs` `classify`）无 PT；#631 握手判定表"类别一致" |
| REM-3.2-5 | REM §3.2 | 共享监听器的验证规则与接收路由经可撤销注册句柄交给 link 持有；句柄丢弃即撤销 | 进程内并发 | TLA + PT | — | AP 注册与代次（`StopAP`、`StartAP`） | 部分 | #615、#624、#631 ※ | #631 `registration.rs` 有状态 PT；静态 link 的注册未单独建模 |
| REM-3.2-6 | REM §3.2；NOD「一条 link」末条 | 无有效注册直接拒绝；凭据有效但对端的 link 尚未注册时要求拨号方重试，两端可按任意顺序启动 | 跨进程 | TLA + PT | — | AP `DoDial`（`reg = 0` 可重试） | 部分 | #631 | 代码：凭据未验证 403，已验证但无注册 503 可重试（`transport/websocket/listen.rs:247–260`）；REM 未写类别（见问题 9） |
| REM-3.2-7 | REM §3.2；§10 公共机制 | 承载握手结束、移交前复核身份与注册代次；旧注册不得移交迟到的连接 | 跨进程 | TLA + TV | bridge/revocation-refuses-reconnects | AP `NoStaleHandOver`（`access-registration`；变异 `access-mut-no-gen-check`） | 已覆盖 | #624 | 设计层已穷举；实现对照靠 #624 的 TV |
| REM-3.2-8 | REM §3.2；§10 公共机制 | link 停止（含所依赖的 Transport、Identity 服务撤下）时取消重试、撤销注册、关闭所属会话与握手中的逻辑通道 | 跨进程 | TLA | — | RL `StoppedStaysDown`（变异 `lease-mut-stop-keeps-retry`）；AP `StopAP` | 部分 | #615 | 静态监听 link 的停止未建模；#615 把停止改为可再开始 |
| REM-3.2-9 | REM §3.2（英文版 "when … Identity is revoked"） | Identity 吊销时取消重试、撤销注册 | — | — | bridge/revocation-closes-only-its-connections | — | 已取代 | — | 被 DIA §五 取代：吊销后监听的 link 保留注册并拒绝（4003/403），拨号的 link 不拨号、每 30 秒按认证失败等待（`link.rs:493–499`、`link.rs:646–660`） |
| REM-3.2-10 | REM §3.2 | link 停止或吊销不关闭其他链接使用的共享物理连接 | 跨进程 | TLA | bridge/revocation-closes-only-its-connections | AP `RevocationScoped`（只覆盖同一监听器上其他 id） | 部分 | #636 ※ | 物理连接共享（memory 多路复用）不在任何模型 |
| REM-3.2-11 | REM §3.2 | 底层凭据撤销使物理连接整体失效时，Adapter 通知所有受影响通道 | 进程内并发 | SH | — | — | 未覆盖 | #636 ※ | 未找到对应实现或测试；按 Q5.1.1 需核实或删去 |
| REM-3.2-12 | REM §3.2 | 身份与授权绑定逻辑 Channel，复用物理连接不能串用权限 | 跨进程 | TLA | — | — | 未覆盖 | #636 ※ |  |
| REM-3.2-13 | REM §3.2 | 物理连接故障终止受影响的通道，由各 link 分别处理；关闭单个 Channel 不影响同连接的其他通道 | 进程内并发 | SH | — | — | 未覆盖 | #628 | #628 只含 WebSocket `Pipe`、内存 `Direction` 的不丢唤醒，不含"不影响其他通道" |
| REM-3.3-1 | REM §3.3；§10 节点互联末条 | 组合入口只组合已有实现（权限与行为与单独装配一致）；自建实例归组合子树，引用的共享承载、身份不归 | 进程内并发 | 测试 | loader/peer-rows | — | 不适用 | — | 装配结构；`peer_rows.rs::loader_rows_compose_links_and_change_features_in_place` |
| REM-3.3-2 | REM §3.3；§10 节点互联末条；NOD「在 link 上做什么」 | 只改导出、导入列表时保留 link 与原会话，只重启对应部分，不重建组合子树 | 进程内并发 | PT | loader/peer-rows | — | 未覆盖 | #629 | #629 reconcile"保留/更新/重建"PT。REM 原文含"事件列表"：`events` 字段 0.13 起被拒（events/node-config），登记表 loader/peer-rows 仍写 `events`（问题 8） |
| REM-4.1-1 | REM §4.1 | 一个运行时实例承载多个插件；运行时实例名与会话端点 ID 分开管理 | 纯逻辑 | 测试 | — | — | 不适用 | — | 命名约定；接入点里实例名即连接 id（DIA §四） |
| REM-4.1-2 | REM §4.1 | 运行时插件只依赖执行端接入条件，不依赖业务行提供的服务 | 进程内并发 | 测试 | loader/runtimes-do-not-wait-for-each-other | — | 不适用 | — | 依赖声明（`injects`）静态可见 |
| REM-4.1-3 | REM §4.1 | 叶子插件不提供子插件树、事件系统或独立依赖图 | — | — | — | — | 已取代 | — | 叶子插件已有事件（`ctx.on` 与五种方式，sdk/leaf-events，统一事件 0.13）；子插件树与依赖图仍没有 |
| REM-4.1-4 | REM §4.1 | 远端运行时不得自行决定依赖满足、启动顺序或重启策略，只按控制方指令装卸行 | 跨进程 | K | — | — | 未覆盖 | #639 ※ | 否定式；可改写为"运行时只在收到 `rows.load` 时装载"再由一致性套件检查（问题 6） |
| REM-4.2-1 | REM §4.2 表 mount/features | mount 时校验运行时功能（`rows.v2`、`hosts`、`leaf`、`scopes`），缺少的功能在使用前拒绝 | 纯逻辑 | PT | — | — | 未覆盖 | #631 ※ | `runtime/process.rs` `require`；#631 的握手判定表只含会话能力 |
| REM-4.2-2 | REM §4.2 表 | `rows.schema` 返回配置 schema、inject、provides 及方法形状 | 纯逻辑 | K | loader/schema | — | 未覆盖 | #639 ※ |  |
| REM-4.2-3 | REM §4.2 表 | `rows.load/update/unload` 按代次装载、更新、卸载指定行 | 跨进程 | TLA | — | — | 未覆盖 | #625 | #625 `OneGeneration`、`OwnGenerationOnly` |
| REM-4.2-4 | REM §4.2 表与列表 | 宿主服务按消费行租用：首个租用注册（`hosts.provide`），最后一个释放撤销（`hosts.withdraw`） | 跨进程 | TLA | bridge/caller-row | — | 未覆盖 | #625 | #625 `LeaseWithinRow`；有 `scopes` 时每行在自己作用域租用 |
| REM-4.2-5 | REM §4.2 | 租用操作串行，租用返回前注册完成 | 进程内并发 | SH | — | — | 未覆盖 | #625 | #625 疑点：`lease()` 等 `hosts.provide` 时被丢弃，计数留在 1 |
| REM-4.2-6 | REM §4.2 表"行服务通知与调用"；列表 | 行提供的服务投到 rutis；重复服务导出按冲突失败 | 跨进程 | TLA | loader/announced-services | — | 未覆盖 | #625 | #625"宿主拒绝撞名的服务"、`NewerWins` |
| REM-4.2-7 | REM §4.2 | 同运行时原生服务不重复注册代理 | 跨进程 | K | — | — | 未覆盖 | #639 ※ |  |
| REM-4.2-8 | REM §4.2 | `origin()` 返回 `Option<&Process>`，N2 改为实例名 + 会话代次 | — | — | — | — | 已取代 | — | 已改为会话 tag：`HostDispatch::origin() -> Option<&str>`（`crates/rutis-bridge/src/session/services.rs:28`） |
| REM-4.2-9 | REM §4.2 | 运行时功能由 Runtime 服务在契约校验后公开，loader 行集成从 Runtime 读取，不直接访问 Process | 进程内并发 | 测试 | — | — | 不适用 | — | 结构约束；接入点的连接是例外：行直接持有 `RuntimeConnection`（DIA §四 实现） |
| REM-4.2-10 | REM §4.2 | `leaf` 运行时的全部 inject 由 rutis 门控；Cordis 只把 `register_shared` 的名字交给 rutis | 跨进程 | K | loader/rows-gate-on-services | — | 未覆盖 | #639 ※ |  |
| REM-4.2-11 | REM §4.2；§10 远程运行时 | 行卸载先撤销服务投影并等待消费者停止，再卸载提供者插件 | 跨进程 | TLA | loader/row-unload-leaves-nothing | — | 未覆盖 | #625 | #625 `WithdrawBeforeUnload` |
| REM-4.2-12 | REM §4.2 末条；§10 远程运行时末条 | 更新按运行时能力：无 volatile 的运行时（Python）就地更新按卸载后重装，不承诺不断实例；Node 仅在能力支持时用 volatile | 纯逻辑 | PT | loader/update-in-place-or-respawn | — | 未覆盖 | #631 | #631"volatile 判定" |
| REM-4.3-1 | REM §4.3 第1步；§7 表"会话就绪" | RuntimePlugin 校验运行时契约后才提供 `Runtime::key(实例名)` | 进程内并发 | SH | — | — | 未覆盖 | #627 | `RuntimeHandle` 的状态与等待（#627 第 5 项） |
| REM-4.3-2 | REM §4.3 第2、3步；§10 远程运行时 | `RuntimeRowsPlugin` 刷新离线解析与声明变化的行后才提供 `RuntimeRows`；行在声明完整前不得启动，冷启动刷新后每行只启动一次 | 跨进程 | TLA | loader/rows-gate-on-services | — | 未覆盖 | #625 | #625 `NoProvisional` 与活性 |
| REM-4.3-3 | REM §4.3 | 解析刷新在独立任务中执行，不在进行中的 reconcile 内同步等待 reload | 进程内并发 | SH | — | — | 未覆盖 | #625 ※ |  |
| REM-4.3-4 | REM §4.3 末句；§7 表"断链"；§10 远程运行时 | 运行时失联撤销 Runtime，门控撤销 RuntimeRows，相关行与服务投影停止；运行时崩溃只影响相关行 | 跨进程 | TLA | loader/runtime-down-stops-its-rows、loader/runtime-start-failure-isolated | — | 未覆盖 | #625 | #625 含"运行时崩溃与重启" |
| REM-4.4-1 | REM §4.4 第2条；§10 远程运行时 | 装载的行、注入的代理、导出的引用都属于该控制会话的租约 | 跨进程 | TLA | loader/remote-runtime-lease | RL `OldCommandsStayOld`（只建行） | 部分 | #615 | 代理与引用未建模 |
| REM-4.4-2 | REM §4.4 第2条；§10；NOD「别的机器上的语言运行时」 | 控制会话断开后执行端清理租约内全部行和代理 | 跨进程 | TLA + K | loader/remote-runtime-lease | RL（close 后清理）；K-leases `*_successive_*`、`*_reconnecting_*` | 部分 | #615、#624 | 缺 Swift/Kotlin；清理在模型里瞬时 |
| REM-4.4-3 | REM §4.4 第3条 | 明确不保证：远程运行时进程不由 rutis 自动重启 | 外部 | 测试 | — | — | 不适用 | — | 未登记为明确不保证 |
| REM-4.4-4 | REM §4.4 第4条；§7 表"替换旧会话"；§10 远程运行时 | 新控制会话接管前先结束旧租约，新租约不与旧租约并存 | 跨进程 | TLA + K | loader/remote-runtime-lease | RL `OneLease`（变异 `lease-mut-start-before-clean`）；K-leases `*_takeover_after_the_old_lease_is_gone`、`*_takeover_with_a_slow_cleanup` | 部分 | #615、#624 | Node/Bun 接管 5 秒后强杀旧子进程，短于清理截止（默认 10 秒）；Swift/Kotlin 未核对（问题 3） |
| REM-4.4-5 | REM §4.4 第4条；§10 远程运行时 | 迟到的旧会话指令不能作用于新租约 | 跨进程 | TLA | loader/remote-runtime-lease | RL `OldCommandsStayOld`（变异 `lease-mut-read-old`） | 部分 | #615 | 对应表缺 Swift/Kotlin |
| REM-4.4-6 | REM §4.4 第4条 顺序① | 接管顺序①：先完成承载鉴权，未通过鉴权的连接不影响当前会话 | 跨进程 | TLA | loader/remote-runtime-lease | RL（只有升级成功的连接进入队列） | 部分 | #615 | 鉴权失败的连接未建模 |
| REM-4.4-7 | REM §4.4 第4条 顺序② | 接管顺序②：关闭旧会话通道（4002），不再读取其后续帧 | 跨进程 | TLA | loader/remote-runtime-lease | RL `TakeoverClose`（变异 `lease-mut-read-old`；`lease-mut-wait-for-old` 违反活性） | 部分 | #615 | 对应表缺 Swift/Kotlin |
| REM-4.4-8 | REM §4.4 第4条 顺序③ | 接管顺序③：清理旧租约内全部行和代理 | 跨进程 | TLA | loader/remote-runtime-lease | RL `TakeoverClean` | 部分 | #615 | 清理建为瞬时；#615 改为"开始清理 → 完成或超时被强杀" |
| REM-4.4-9 | REM §4.4 第4条 顺序④；§10 远程运行时 | 接管顺序④：旧租约清理完成后才回应新会话的 hello | 跨进程 | TLA + K | loader/remote-runtime-lease | RL `OneLease`（变异 `lease-mut-start-before-clean`）、`NoHeartbeatBeforeHello`（变异 `lease-mut-heartbeat-from-accept`）；K-leases `takeover`（`stop a` 先于 `start b`） | 部分 | #615、#624、#1102、#1182 | 新连接的心跳从回应 hello 起算，不从 accept 起算；Kotlin #1102、Go / Swift / Node / Bun #1182 已修 |
| REM-4.4-10 | REM §4.4 第5条 | 网络承载下远程叶子只作监听方，由控制方 link 拨号并负责重连；穿越 NAT 用反向代理或隧道，不提供叶子主动拨号 | — | — | — | — | 已取代 | — | 被 DIA R1 取代；REM §4.1 末条已加修订注，§4.4 未改 |
| REM-4.4-11 | REM §4.4 第6条；§7 表"重连"；§10 远程运行时 | 重连后重新查询声明并按期望状态装载（新租约、新行实例），不复用旧行、旧引用 | 跨进程 | TLA + K | loader/remote-runtime-lease | RL（每个连接新租约）；K-leases `*_reconnecting_controller_gets_a_new_lease` | 部分 | #615、#624 | 声明的重新查询未建模 |
| REM-5.1-1 | REM §5.1 表 export 行；NOD「在 link 上做什么」export 行、「安全」末条 | export 只公告列出的本地服务（确切名字），跟踪换值与撤销 | 跨进程 | PT + K | — | K-node（`calendar` 同步与异步调用） | 部分 | #631 | K-node 不查换值与撤销；#631"版本最高者胜、墓碑" |
| REM-5.1-2 | REM §5.1 表 import 行；§8 末条 | import 把指定远端服务注册为本地原生服务（原生服务系统与 loader 目录，运行时投影用 `host_key`） | 跨进程 | K | — | K-node（代装的插件经导入的 `probe` 记录） | 部分 | #639 ※ | 只有 Rust 与 Cordis 两种节点实现 |
| REM-5.1-3 | REM §5.1 表 host 行；§7 表"断链" | host 接受代装，装出的插件归自身子树，host 走（断链或卸载）时一并卸载 | 跨进程 | K + TLA | loader/peer-rows | K-node（load/update/unload） | 部分 | #640 ※ | "随 host 卸载"无形式化；#625 疑点：加载中被取消的代装留下孤儿插件（`host.rs`） |
| REM-5.1-4 | REM §5.1 表 events 行 | events 插件按事件名和方向转发通知 | — | — | — | — | 已取代 | — | 被统一事件（协议规格 §9，0.13）取代：`export`/`import` 选择，对端监听器是本节点总线上的条目（`crates/rutis-bridge/src/events/link.rs`） |
| REM-5.1-5 | REM §5.1 第1条；§10 节点互联 | 功能插件只依赖会话就绪，不等待远程行；双方互相代装同时冷启动、重连不死锁，host 不等待行就绪 | 跨进程 | TLA | loader/peer-rows | — | 未覆盖 | #640 ※ | T6 暂缓；`peer_rows.rs::nodes_hosting_for_each_other_start_and_reconnect_without_deadlock` 普通测试 |
| REM-5.1-6 | REM §5.1 | 默认不装功能插件；装了哪些功能插件就是该链接的授权 | 跨进程 | K | — | — | 未覆盖 | #639 ※ |  |
| REM-5.1-7 | REM §5.1；§6.2 能力段末 | 功能登记与注销更新 `link.offers`，由 link 转达；功能族不作为会话能力，link 不解释行管理语义 | 跨进程 | TLA | events/node-links | — | 未覆盖 | #613 | #613 `EventsLink` 把 offers 建为可跳过的快照 |
| REM-5.1-8 | REM §5.1；NOD「在 link 上做什么」import 行 | 导入的服务本节点已有同名时，后注册的 import 失败并同时标明双方链接，先注册者保留 | 进程内并发 | PT | — | — | 未覆盖 | #631 ※ | `services.rs:391`；#631 的节点服务 PT 只含版本规则 |
| REM-5.1-9 | REM §5.1 | 明确不保证：冷启动时两个链接导入同名服务，哪一方保留不确定 | 进程内并发 | 测试 | — | — | 不适用 | — | 未登记为明确不保证（问题 7） |
| REM-5.2-1 | REM §5.2 首句 | 节点操作双向可用；未注册的操作返回错误，会话继续 | 跨进程 | K | — | — | 未覆盖 | #639 ※ | `peer.rs` 文档注释；无一致性用例 |
| REM-5.2-2 | REM §5.2 announce 行 | `services.announce` 的版本取自会话级单一递增序列（`Peer::next_version`），export 改列表重启后继续递增 | 纯逻辑 | PT | — | — | 未覆盖 | #631 |  |
| REM-5.2-3 | REM §5.2 表后第1条 | 换值公告新引用，按 version 排序，更旧的公告不覆盖更新的；旧引用按计数释放 | 纯逻辑 | PT | bridge/release-count | PT-val `*_releases_balance`（只查引用计数） | 部分 | #631 |  |
| REM-5.2-4 | REM §5.2 withdraw 行 | 导入方保留撤销的版本（墓碑），晚到的更旧公告不得恢复服务 | 纯逻辑 | PT | — | — | 未覆盖 | #631 |  |
| REM-5.2-5 | REM §5.2 describe 行 | `plugins.describe(插件)` 返回 `{ schema, version, integrity }`；未安装为 NotFound | 跨进程 | K | loader/peer-rows | K-node（describe、未安装 NotFound） | 已覆盖 | — | Rust 与 Cordis 两种节点 |
| REM-5.2-6 | REM §5.2 load 行；NOD「在别的节点上运行插件」 | `plugins.load(key, 插件, config, isolate, inject)` 按行的 isolate（标签按对端区分）隔离、按 inject 门控 | 跨进程 | K | loader/peer-rows、loader/isolate-and-inject | K-node（带与不带 inject 的 load，#584） | 部分 | #639 ※ | isolate 标签按对端隔离无检查 |
| REM-5.2-7 | REM §5.2 load 行 | 服务名经宿主的映射转为键，无法映射即拒绝 load | 纯逻辑 | K | — | — | 未覆盖 | #639 ※ | `host.rs` `services` |
| REM-5.2-8 | REM §5.2 update/unload 行 | `plugins.update(key, config)`、`plugins.unload(key)` 更新、卸载代装的插件 | 跨进程 | K | loader/peer-rows | K-node | 已覆盖 | — |  |
| REM-5.2-9 | REM §5.2 events.forward 行 | `events.forward { name, args }` 以 parallel 发通知并等待监听完成 | — | — | — | — | 已取代 | — | 被事件协议 §9 取代（`events.on`/`off`/`call`）；代码中已无 `events.forward` |
| REM-5.2-10 | REM §5.2 link.offers 行 | `link.offers` 宣告功能族，版本递增，接收方忽略旧宣告 | 跨进程 | TLA | events/node-links | — | 未覆盖 | #613 | `peer.rs` `send_if_modified`；#613 `EventsLink` |
| REM-5.2-11 | REM §5.2 link.offers 行 | `since`：各族登记编号递增、与处理者一起记下；族撤销后重新登记是新的一次提供，跟随方按 since（而非族在不在）重新提供依赖它的东西 | 跨进程 | TLA | events/node-links | — | 未覆盖 | #613 | #613 `CurrentOffer`（#435、#593） |
| REM-5.2-12 | REM §5.2 link.offers 行 | 不带 `since` 的一端各族视为 0 | 纯逻辑 | PT | — | — | 未覆盖 | #631 ※ | `Offers::epoch` |
| REM-5.2-13 | REM §5.2 表后第2条 | Cordis 服务经 export 的导出 fiber 读取，调用产生的 effect 归该 fiber | 进程内并发 | K | — | — | 未覆盖 | #639 ※ | 只在 Cordis 节点（`node/rutis-runtime/src/bridge/`） |
| REM-5.2-14 | REM §5.2 表后第3条 | 仅转发通知事件；同链接、同事件名只允许一个转发方向 | — | — | — | — | 已取代 | — | 被统一事件取代：五种方式都跨节点，同一名字可双向共享（NOD「节点之间的事件」，events/node-links） |
| REM-5.2-15 | REM §5.2 表后第4条 | 节点可组成树；跨链接导入再导出的服务经引用转发与调用链路由多跳可用 | 跨进程 | TLA | bridge/multihop-cancel-and-release | — | 未覆盖 | #626 | #626 只建一跳中继；事件只走一跳（NOD-3-5） |
| REM-5.3-1 | REM §5.3 第1–3步；NOD「在别的节点上运行插件」 | 对端宣告 plugins 族后，`PeerRowsPlugin` 查 schema、刷新解析再提供 `PeerRows#<id>`；PeerRow 在声明完整前不启动 | 跨进程 | TLA | loader/peer-rows | — | 未覆盖 | #640 ※ | T6 暂缓（#612） |
| REM-5.3-2 | REM §5.3 | `peer:<id>/<插件>` 由 PeerResolver 解析，插件名由目标节点解释 | 纯逻辑 | PT | loader/peer-rows | — | 未覆盖 | #631 ※ |  |
| REM-5.3-3 | REM §5.3；§10 节点互联；NOD「在别的节点上运行插件」 | 对端无 plugins 或离线：解析标为离线、schema 为空，行等待 PeerRows，不标 Unresolved；回来后重新装载 | 跨进程 | TLA | loader/peer-rows | — | 未覆盖 | #640 ※ |  |
| REM-5.3-4 | REM §5.3 | 刷新以独立任务用 `entries()` + `reload(id)`；门控开放前不启动旧解析；每行只启动一次 | 进程内并发 | TLA | loader/peer-rows | — | 未覆盖 | #640 ※ |  |
| REM-5.3-5 | REM §5.3 | PeerRow 的 apply 发 load，disposer 发 unload；已断链的卸载按成功处理 | 跨进程 | TLA | loader/peer-rows | — | 未覆盖 | #640 ※ | 只有 #625 的疑点（加载中被取消的托管行）涉及宿主一侧 |
| REM-5.3-6 | REM §5.3；§10 节点互联 | 对端撤回 plugins 时撤销 PeerRows，Peer 与其他功能照常运行 | 跨进程 | TLA | loader/peer-rows | — | 未覆盖 | #640 ※ |  |
| REM-6.1-1 | REM §6.1 第2条；§10 公共机制 | 启动时校验版本与能力，不靠调用失败猜测端点种类，不静默切换模式；不符即拒绝并说明 | 纯逻辑 | PT | bridge/version-refusal | — | 未覆盖 | #631 | #631 握手判定表 |
| REM-6.1-2 | REM §6.1 第3、4条 | 端点身份握手与新调用号用不同主版本（现为 3），WebSocket 子协议随之为 `rutis.3`，不沿用 `rutis.2` | 纯逻辑 | PT + FZ | bridge/version-refusal | FZ-up（只接受 `rutis.3`） | 部分 | #631 | `session/mod.rs:18`；`registration.rs` 注释仍写 `rutis.2` |
| REM-6.1-3 | REM §6.1 第5、6条；§10 公共机制 | 两种格式并存：各实现同时支持 2 与 3；格式由会话来源决定（本机兼容 2、网络 3），握手时确定、会话内不切换；旧执行端不支持时直接报不兼容 | 纯逻辑 | PT + K | bridge/version-refusal | K-sess（本机与监听两种连接） | 部分 | #631 |  |
| REM-6.1-4 | REM §6.1 第7条 | 本机兼容会话的身份取自启动方指定的 `ChannelInfo.peer`，不核对 hello 端点 id；兼容 hello 不含端点 id | 纯逻辑 | PT | — | — | 未覆盖 | #631 | 兼容 hello 现带能力（`events-v2`、`sync-wait`、`value-*`，`session/rpc.rs` `COMPAT_CAPABILITIES`），"只含版本"已过时 |
| REM-6.1-5 | REM §6.1 第7条；§6.2 第2条 | 该豁免只限启动方持有的继承 fd 或私有 socket；网络会话一律核对端点 id 与验证过的身份 | 纯逻辑 | PT | — | — | 未覆盖 | #631 | 与 DIA §五 矛盾：接入点不核对自报 id（`access_point.rs` 置 `info.peer = None`、`expected: None`）（问题 2） |
| REM-6.2-1 | REM §6.2 第1条 | 握手双方声明会话版本、端点 ID、实现名称/版本与能力 | 纯逻辑 | PT + FZ | — | FZ-frame（帧解析往返，不查 hello 字段） | 部分 | #631 |  |
| REM-6.2-2 | REM §6.2 第2条 | 端点 ID 仅含小写字母、数字、`-`，部署内唯一 | 纯逻辑 | PT + K | — | K-dial `an_invalid_id_is_refused_before_dialing`（拨出方） | 部分 | #631 ※ | 唯一性只由部署保证（DIA §六），应登记为明确不保证 |
| REM-6.2-3 | REM §6.2 第3条 | 新格式调用号为 `<端点 id>:<n>`，校验前缀与递增序号 | 纯逻辑 | PT + FZ | — | — | 未覆盖 | #631 | #631"帧接收的有状态 fuzz" |
| REM-6.2-4 | REM §6.2 第3、4条 | 引用 origin 与调用链 path 一致；兼容路径保留会话标签与调用链 rebase | 跨进程 | TLA + PT | bridge/call-chains-across-processes | CRS `RoutingSound`（`current-norebase` 违反） | 部分 | #614、#631 |  |
| REM-6.2-5 | REM §6.2 第5条 | 两种格式间的引用与调用链转换只在 rutis 中继（`rpc/relay.rs`），执行端不感知另一种格式；网络多跳只在新格式会话之间定义 | 跨进程 | TLA + PT | bridge/call-chains-across-processes | — | 未覆盖 | #626、#631 |  |
| REM-6.2-6 | REM §6.2 第6条 | 函数、异步结果、对象引用按能力声明；对象首次授予附形状；未实现对象接收的实现不得宣告支持 | 跨进程 | K | bridge/value-roundtrip | K-sess（函数、future、hold/fire） | 部分 | #639 ※ | 宣告与实际能力是否一致无检查 |
| REM-6.2-7 | REM §6.2 第7条 | 取消双向落到 AbortSignal、取消令牌或等价物；丢弃 future 发送取消 | 跨进程 | TLA + K | bridge/cancel-reaches-callee | K-sess（`abortable`/`aborted`） | 部分 | #626 | #626 `CancelReaches` |
| REM-6.2-8 | REM §6.2 第8条 | 转发按（会话，引用号）保持身份，释放沿中继链传递 | 跨进程 | TLA + PT | bridge/multihop-cancel-and-release、bridge/release-count | PT-val（两方） | 部分 | #626、#631 | 中继三方的引用计数 PT 在 #631 |
| REM-6.2-9 | REM §6.2 第9条前半 | 各实现宣告的会话能力列表（Rust：objects、signals、reentrant-sync、forwarding、sync-wait；Node、Python、Bun、Go 各列） | — | — | — | — | 已取代 | — | Rust 现另宣告 `events-v2` 与十个 `value-*`（`session/rpc.rs` 约 1002 行），`events-v2` 强制；Swift、Kotlin 未列 |
| REM-6.2-10 | REM §6.2 第9条 sync-wait | `sync-wait`：同步调用帧带 `sync: true`，宿主据此检测跨运行时等待环 | 跨进程 | TLA + K | bridge/sync-wait-cycle-refused | CRS `NoFalseCycle`、`RoutingSound` 等；K-sync | 部分 | #614、#624 | 每调用一线程的运行时、取消不在模型（#614）；`sync_cycles` 缺 Kotlin 列（#624） |
| REM-6.2-11 | REM §6.2 第9条 sync-stack | `sync-stack`：一个线程一个栈，同步等待期间执行的调用压在等待之上 | 跨进程 | TLA | bridge/sync-wait-cycle-refused | CRS（单栈运行时） | 部分 | #614 |  |
| REM-6.2-12 | REM §6.2 第9条 | compat 握手也可宣告 `sync-wait`、`sync-stack`、`reentrant-sync`，旧实现忽略 | 纯逻辑 | PT | — | — | 未覆盖 | #631 |  |
| REM-6.2-13 | REM §6.2 第9条 | 端点契约以能力 `runtime`、`node` 宣告；link 以 `require` 在握手后校验，缺少即按不兼容停止 | 纯逻辑 | PT | — | — | 未覆盖 | #631 | `link.rs` `Greet::open` |
| REM-6.2-14 | REM §6.2 第9条末句 | 能力不授予权限；缺少必需能力时在使用前拒绝（带标记的值在发出前失败） | 纯逻辑 | PT | bridge/value-tags | PT-val（缺 `value-bigint` 时发出前失败） | 部分 | #631 |  |
| REM-6.2-15 | REM §6.2 第10条 | 不支持同步重入时，同步等待期间同链的反向调用立即返回 SyncWaitCycle，不排队 | 跨进程 | TLA + K | bridge/sync-wait-cycle-refused | CRS（策略 B 配置）；K-sync | 部分 | #614 |  |
| REM-6.3-1 | REM §6.3 第1条 | 形状描述 sync/async、属性及对象形状；无声明成员不得跨端点暴露 | — | — | loader/announced-services | — | 已取代 | — | 被自动暴露取代（2026-10-18）：没有声明就提供的服务按对象的公开方法投出 |
| REM-6.3-2 | REM §6.3 第3条 | 放置不改变方法的同步/异步类型 | 跨进程 | K | — | — | 未覆盖 | #639 ※ |  |
| REM-6.3-3 | REM §6.3 第3条 | 明确不保证：远程同步调用至少阻塞一个 RTT，不自动添加调用超时 | 外部 | 测试 | — | — | 不适用 | — | 未登记（问题 7） |
| REM-6.3-4 | REM §6.3 第5条；§10 公共机制末条 | 可重入运行时（Python）同步等待时可执行其他进来的调用；同步回调在阻塞的 current_thread 上仍可推进 | 进程内并发 | TLA + SH | bridge/sync-wait-cycle-refused | CRS `current-reentrant`；K-sess `reenter` | 部分 | #614、#628 | #628 `ready_on_blocked_executor` |
| REM-6.3-5 | REM §6.3 末条 | 两个 Node 运行时无关调用链交叉同步等待仍会死锁 | — | — | bridge/sync-wait-cycle-refused | — | 已取代 | — | 被 #228/#261 取代：宿主检测等待环并返回 SyncWaitCycle |
| REM-7-1 | REM §7 表"本机进程退出" | 本机运行时进程退出时报告退出状态 | 进程内并发 | SH | host/runtime-state-lines | — | 未覆盖 | #623 |  |
| REM-7-2 | REM §7 末段第1条；§3.2 | 在途调用断链后以 Transport 失败一次，结果未知，不自动重试；旧引用返回 session closed | 跨进程 | TLA | bridge/in-flight-call-fails-once、bridge/no-call-on-a-dead-session | — | 未覆盖 | #626 |  |
| REM-7-3 | REM §7 第3条 | 明确不保证：插件可信，网络与远端机器不完全可信；进程外执行不是沙箱 | 外部 | 测试 | — | — | 不适用 | — | 信任模型 |
| REM-7-4 | REM §7 第4条；NOD「一条 link」、「安全」第1条 | 非回环网络必须 TLS（可前置反向代理）；不带 TLS 的监听器只能绑回环，拨号方拒绝非回环 `ws://` | 外部 | 测试 | host/listen-tls | — | 不适用 | — | 地址判定加真实套接字：`websocket.rs::listeners_off_loopback_need_tls`、`transport/websocket/dial.rs:90` |
| REM-7-5 | REM §7 第4条 | 承载必须验证身份；权限与拨号方向无关 | 跨进程 | TLA | bridge/tls-server-verified、bridge/tls-client-certificate-verified | — | 未覆盖 | #615 ※ | "与方向无关"无检查 |
| REM-7-6 | REM §7 第5条；NOD「在 link 上做什么」host 行、「安全」 | 节点 host 只能装当地已安装的插件，拒绝文件与绝对路径；开放 host 只给可信对端 | 纯逻辑 | K | loader/peer-rows | K-node（`/etc/plugin` 被拒、未安装 NotFound） | 已覆盖 | — |  |
| REM-7-7 | REM §7 第6、7条 | 远程运行时只能加载执行环境中已部署的插件，不提供代码上传；远程加载拒绝 `file:` 与绝对路径插件名（显式本机运行时可保留 `file://`） | 纯逻辑 | PT | loader/remote-runtime-rows | — | 未覆盖 | #631 ※ | `rutis-loader/src/runtime.rs:137` |
| REM-7-8 | REM §7 第8条 | 求值后的配置可能含密钥，诊断和追踪不得记录秘密 | 外部 | 测试 | host/link-state-output | — | 不适用 | — | 文字输出；只有 link 状态行与拨出状态行查过不含 token（K-dial `a_wrong_token_*`）；"诊断和追踪"范围过宽（问题 6） |
| REM-7-9 | REM §7 第9条 | 消息大小上限由各承载配置，超限时承载关闭通道 | 纯逻辑 | FZ + 测试 | bridge/large-and-deep-values | FZ-lines（本机分帧 TooBig） | 部分 | #631 | WebSocket 发送超限即关闭（`transport/websocket/connection.rs:88`）；#631 各语言帧解码 FZ |
| REM-7-10 | REM §7 第9条 | 明确不保证：不设调用数与引用数配额 | 外部 | 测试 | — | — | 不适用 | — | 未登记 |
| REM-8-1 | REM §8 表与第2条 | 本机语言插件保留 `py:<模块>`、npm 名及具名 Runtime 绑定；不带前缀的 npm 名解析到绑定的 Node Runtime 实例，没有隐式本机节点 | 纯逻辑 | PT | — | — | 未覆盖 | #631 ※ |  |
| REM-8-2 | REM §8 表"远程语言插件"；第1条 | 远程语言插件用同一语言行解析规则，不另引入远程前缀 | — | — | loader/remote-runtime-rows | — | 已取代 | — | 远程运行时的行名为 `<运行时名>:<模块或插件>`（NOD「别的机器上的语言运行时」、`docs/guide/rutis-host.md` 示例 `gpu:embedder`） |
| REM-8-3 | REM §8 第3条；§10 远程运行时 | 远程 Runtime 的插件入口由执行端在 `rows.schema` 中解析，控制方不按本地文件系统判断存在性；未找到返回结构化结果，行标为 Unresolved | 跨进程 | K | loader/remote-runtime-rows、loader/unresolved-rows | — | 未覆盖 | #639 ※ | `remote_rows.rs::a_remote_node_runtime_resolves_and_runs_npm_rows` 普通测试 |
| REM-8-4 | REM §8 第4条 | Runtime 的远程会话来源是拟新增能力，不宣称现有 launcher 支持网络 | — | — | — | — | 已取代 | — | 已实现：`runtimes.remote`、link `runtime`、接入点 |
| REM-8-5 | REM §8 第5条 | 配置表达式在控制方求值，插件在执行方解释 | 纯逻辑 | PT | loader/expressions | — | 未覆盖 | #631 | #631"宿主配置求值"（部分） |
| REM-8-6 | REM §8 第6条 | isolate 标签只在同一执行框架实例内共享；更换执行实例重建行 | 跨进程 | K | loader/isolate-and-inject | — | 未覆盖 | #639 ※ |  |
| REM-10-1 | REM §10 公共机制第3、10条 | 同一套会话测试覆盖 memory、本机 IPC、网络承载；新增承载不增加管理操作 | 跨进程 | K | — | K-sess（`runtime_conformance.rs` 本机与监听连接） | 部分 | #639 ※ | memory 承载上的会话检查在普通测试 `memory_session.rs` |
| REM-10-2 | REM §10 公共机制第9条 | 半开连接在心跳期限内清理，旧引用失效 | 外部 | 测试 | bridge/in-flight-call-fails-once | — | 不适用 | — | 计时；每晚 `tests/e2e/tests/netem.rs`；#633 不含心跳 |
| DIA-2-1 | DIA R1；§三；§七；§八；APP D9；NOD「连进来的运行时」 | 每种远程运行时都支持监听与拨出两个方向，用哪个是部署选择；监听方向长期保留 | 外部 | K | bridge/dial-out-rules、loader/remote-runtime-lease | K-dial（6 列）；K-leases（Python、Node、Go、Bun 监听） | 部分 | #624 | 监听方向缺 Swift、Kotlin 列（#624） |
| DIA-2-2 | DIA R2；§四 第2条；APP §六；NOD「连进来的运行时」 | 一个接入点接受多条运行时连接；每条连接是 `instanced` 分组的一个实例，连进来创建、断开关闭 | 跨进程 | TLA + TV | loader/runtime-connection-instances | AP `Accept`、`EndLive`、`OneInstancePerId` | 部分 | #624 | 模型只到"实例存在"；loader 一侧建实例、关实例（`RuntimeInstances`）未建模 |
| DIA-2-3 | DIA R3；§六 同 id 接管；APP D8、§六；NOD「连进来的运行时」 | 连接以握手时自报的 id 区分；同一 id 的新连接接管：旧连接以 4002 关闭并释放其实例后才接受新连接 | 跨进程 | TLA + K + TV | loader/runtime-connection-instances、bridge/dial-out-rules | AP `OneInstancePerId`（变异 `access-mut-accept-before-release`）、`OldCallsStayOld`（变异 `access-mut-route-by-id`）；K-dial `the_same_id_takes_over`（6 列） | 已覆盖 | #624 | 实现对照靠 TV |
| DIA-2-4 | DIA R6；§八 | 现有方式（静态配置、rutis 拨入监听的运行时）行为不变，不强制迁移 | 跨进程 | K | loader/remote-runtime-lease | K-leases | 部分 | #624 | 同 DIA-2-1 |
| DIA-2-5 | DIA R7；§三；§六 实现；APP §六；NOD「连进来的运行时」 | 非回环必须 TLS，拨出时 TLS 服务端只在 rutis 一侧；拨出方对非回环 `ws://` 启动即拒绝（退出非零） | 纯逻辑 | K | bridge/dial-out-rules | — | 未覆盖 | #639 ※ | `dial_conformance` 只查不合法 id，不查非回环 `ws://`；各语言单元测试 |
| DIA-2-6 | DIA R8；APP D10、§八 | 运行时侧（Python、Node、Bun、Go、Swift、Kotlin）全部用平台语言实现，不依赖 Rust | 外部 | 测试 | — | — | 不适用 | — | 构建事实 |
| DIA-3-1 | DIA §三；APP D2 | rutis 对拨进来的运行时的控制权与现有一致：装载、配置、卸载、门控都由 rutis 驱动 | 跨进程 | K | loader/runtime-connection-instances | K-dial（`conformance: loaded <config>` 带 rutis 给的配置） | 部分 | #639 ※ | 门控与卸载顺序无一致性用例 |
| DIA-3-2 | DIA §三 R-P0 | 监听侧 link + `runtime`：运行时连进来后照常 mount、装载行、门控服务，断开即撤销；握手端到端核对运行时自报 id 与 link 的对端 | 跨进程 | TLA + K | loader/remote-runtime-rows | — | 未覆盖 | #615 ※ | 静态监听 link 只被 README 当作接入点的特例（问题 10）；`remote_rows.rs::a_runtime_link_can_listen` 普通测试 |
| DIA-4-1 | DIA §四 第3条；APP D5、§五 | 运行时在 `mount` 回复里报带的插件（`plugins`，每项与 `rows.schema` 回复相同，不含设备信息） | 纯逻辑 | K | loader/runtime-connection-instances | — | 未覆盖 | #639 ※ | 可在 `runtime_conformance` 对比 mount 的 `plugins` 与逐个 `rows.schema` |
| DIA-4-2 | DIA §四 实现 | `plugins` 的键是行名去掉 `<运行时名>:` 后的部分；可选，没有时逐个问 `rows.schema`（远端 NotFound 即"不含"）；每条连接读一次 | 纯逻辑 | PT | loader/runtime-connection-instances | — | 未覆盖 | #631 ※ | `rutis-loader/src/runtime/connections.rs:145–160` |
| DIA-4-3 | DIA §四 第3条 | `mount` 回复加字段（如 `engine`）不算协议变化，接收方忽略不认识的字段 | 纯逻辑 | PT + FZ | — | — | 未覆盖 | #631 ※ |  |
| DIA-4-4 | DIA §四 第3条；§四 实现；APP §五 | 分组里的行在某实例中指向该连接没有的插件时，只在该实例里失败（"该连接不含插件 X"，副本级 `Unavailable`），不影响其他实例 | 进程内并发 | PT | loader/runtime-connection-instances | — | 未覆盖 | #629 | #629 reconcile 与组的 PT（`new_failures`） |
| DIA-4-5 | DIA §四 实现 | "不含插件"不让编辑的 dry run 拒绝，reconcile 也不把它算作新失败而回滚 | 进程内并发 | PT | loader/runtime-connection-instances、loader/edit-dry-run-first | — | 未覆盖 | #629 |  |
| DIA-4-6 | DIA §四 第4条；APP D8、§五 | 实例内的服务名在本实例内解析（无回退）；多条连接带同名插件互不冲突，不设别名 | 进程内并发 | PT | loader/instance-services、loader/runtime-connection-instances | — | 未覆盖 | #631 | #631 运行时作用域 id PT（部分） |
| DIA-4-7 | DIA §四 第4条；APP §五；NOD「连进来的运行时」 | 实例外的行不能使用某个实例的服务；连接没有提供的名字照常是全局的 | 进程内并发 | PT | loader/instance-services | — | 未覆盖 | #631 ※ |  |
| DIA-4-8 | DIA §四 实现；NOD「连进来的运行时」 | 连接的插件提供的服务（含没有声明就提供的）发布在 `host_key_in(名字, 实例)`；同实例的本地运行时行也用到本连接的服务，并以实例标签隔离（需 `scopes`） | 跨进程 | K | loader/runtime-connection-instances | — | 未覆盖 | #639 ※ | `runtime_instances.rs::undeclared_services_are_each_instances_own` 普通测试 |
| DIA-4-9 | DIA §四 配置；NOD「连进来的运行时」 | `runtimes.remote` 条目同时写 `listen` 与 `instances` 即动态接入；分组里的行用 `<运行时名>:<插件>`，在每个实例里解析到该实例的连接；不需另写 link 行 | 纯逻辑 | PT | loader/runtime-connection-instances | — | 未覆盖 | #631 | #631 宿主配置求值（部分） |
| DIA-4-10 | DIA §四 实现末条 | `<运行时名>:` 行放在分组外时失败，并说明它只在该分组里运行 | 纯逻辑 | PT | loader/runtime-connection-instances | — | 未覆盖 | #631 ※ | `connections.rs:375` |
| DIA-4-11 | DIA §四 需要补的第3处；§七 Rust 行 | 实例内允许运行时 link，运行时键按实例区分 | — | — | — | — | 已取代 | — | 被同节"实现"取代：`peer.rs` 的禁止保留（`rutis-loader/src/peer.rs:550`），不需要按实例的 `RuntimeSession` 键；§七 仍写"已实现"（问题 5） |
| DIA-5-1 | DIA §五 第1条 | 身份层：输入握手的凭据与自报 id，输出接受（连接 id）或拒绝（原因）；凭据在升级时检查（无凭据 401、被拒 403），id 在 hello 后准入（拒绝 4003） | 纯逻辑 | PT + TLA | bridge/revocation-refuses-reconnects | AP `DoDial`（凭据）、`HandOver`（准入） | 部分 | #631 | 状态码未建模；#631 注册路由 PT |
| DIA-5-2 | DIA §五 第2条；APP §六；NOD「连进来的运行时」 | 过渡实现：凭据等于共享 token（`RUTIS_TOKEN_<运行时名>` 或 `RUTIS_TOKEN`）即接受，连接 id = 自报 id | 纯逻辑 | K + PT | bridge/dial-out-rules、host/credentials-from-env | K-dial `connects_and_is_accepted_with_its_id`、`a_wrong_token_is_retried_as_rejected` | 部分 | #631 | 环境变量优先级只有普通测试（#631"token 优先级"） |
| DIA-5-3 | DIA §五 第2条；§六 不兼容行；NOD「连进来的运行时」 | 自报 id 须非空、只含 `[a-z0-9-]`；hello 未给出 id 或 id 不合法按不兼容处理 | 纯逻辑 | PT + K | bridge/dial-out-rules | K-dial `an_invalid_id_is_refused_before_dialing`（拨出方启动前） | 部分 | #631 ※ | 接入点一侧（`access_point.rs` `reported`）只有单元测试与普通测试 |
| DIA-5-4 | DIA §五 第2条；§六 实现 | 明确不保证：自报 id 不经 rutis 核实，只用于区分连接、命名实例；id 唯一由部署保证 | 跨进程 | 测试 | bridge/revocation-refuses-reconnects | — | 不适用 | — | 只写在登记表"说明"里，没有单独的明确不保证条目（问题 2、7） |
| DIA-5-5 | DIA §五 吊销钩子接口 | `Revocations::revoked(&id)` 在 id 被吊销时完成（已吊销则立即），不丢吊销 | 进程内并发 | SH | — | — | 未覆盖 | #628 ※ | `identity.rs` 先订阅再检查；无 shuttle 模型 |
| DIA-5-6 | DIA §五 吊销钩子接口 | `Identity`/`Admission` 的 `revocations()` 默认 `None`（不能吊销）；`Revocable` 包住任一实现后可吊销 | 纯逻辑 | 测试 | — | — | 不适用 | — | 类型组合，单元测试 `identity.rs::tests` |
| DIA-5-7 | DIA §五 第三项；NOD「安全」 | 吊销一个 id 只关闭以它接受的连接（静态 link 的会话、接入点里这个 id 的实例）；同一监听器上的其他 link、注册与本机运行时不受影响 | 跨进程 | TLA + TV | bridge/revocation-closes-only-its-connections | AP `RevocationScoped`（变异 `access-mut-revoke-cuts-all`）；RL `Cut`（只切被吊销的控制方） | 部分 | #615、#624 ※ | RL 没有"只切自己"的性质；静态 link 与接入点共享监听器的情形未建模 |
| DIA-5-8 | DIA §五 第四项；NOD「安全」 | 吊销后先关会话再撤销 Peer、释放实例；进行中的调用以传输错误失败一次 | 跨进程 | TLA | bridge/revocation-closes-only-its-connections、bridge/in-flight-call-fails-once | — | 未覆盖 | #626 | 在途调用不在租约模型 |
| DIA-5-9 | DIA §五 第四项 | 吊销前已路由、还没移交的握手在 `Registrations::hand_over` 复核时被拒（4003） | 跨进程 | TLA + TV | bridge/revocation-refuses-reconnects | AP `HandOver`、`RevokedStaysOut` | 已覆盖 | #624 |  |
| DIA-5-10 | DIA §五（#432 修复后的规则；登记表 bridge/revocation-refuses-reconnects） | 已移交、还在握手或等待接管时被吊销的，在提供 Peer、创建实例之前被拒（拨号 link 按认证失败等待，监听 link 与接入点以 4003 关闭） | 跨进程 | TLA + TV | bridge/revocation-refuses-reconnects | RL `RevokedStaysOut`（`lease-before-fix` 违反）；AP `RevokedStaysOut`（`access-before-fix` 违反） | 部分 | #615、#624 ※ | 监听 link 以 4003 关闭这一半没有建模（#615）； |
| DIA-5-11 | DIA §五 第三、四项；NOD「安全」 | 吊销后以这个 id 打招呼的连接在 hello 之后以 4003 拒绝；`revoke_all()` 关闭全部连接，之后的连接在升级时 403；对外与凭据错误相同（"not accepted here"） | 跨进程 | TLA + K | bridge/revocation-refuses-reconnects | AP `RevokedStaysOut`、`RevokeAll`（状态码与文字不建） | 部分 | #615 ※ | 4003/403 与文字一致只在 `revocation.rs`（普通测试） |
| DIA-5-12 | DIA §五 第四项；NOD「安全」 | 拨号方把吊销归为 `AuthRejected`，每 30 秒重试 | 跨进程 | TLA + 测试 | bridge/revocation-refuses-reconnects | RL（被吊销即按认证失败等待、不拨） | 部分 | #633 | 30 秒为计时（#633） |
| DIA-5-13 | DIA §五 第四项 | 监听的 link 吊销后保留注册，状态为 `Waiting { AuthRejected, "<id>: identity revoked" }` | 进程内并发 | TLA | bridge/revocation-closes-only-its-connections | — | 未覆盖 | #615 ※ | 静态监听 link 无模型（问题 10） |
| DIA-5-14 | DIA §五 第四项 | 拨号的 link 吊销后关闭会话、不再拨号，按认证失败等待 | 跨进程 | TLA | bridge/revocation-closes-only-its-connections | RL `Dial`（被吊销不拨）、`Cut`、`RevokedStaysOut` | 已覆盖 | — | 等待的 30 秒见 DIA-5-12（#633） |
| DIA-5-15 | DIA §五 第五项；NOD「安全」 | 吊销只在内存里、不撤回，进程重启即恢复；rutis-host 的环境变量 token 运行中不能吊销 | 外部 | 测试 | host/env-tokens-not-revocable、bridge/revocation-refuses-reconnects | — | 不适用 | — | 明确不保证（已登记 host/env-tokens-not-revocable） |
| DIA-5-16 | DIA §五 第三项 | 明确不保证：持有共享 token 的一方换 id 仍能连进来，只有 `revoke_all()` 能切断它 | 跨进程 | 测试 | bridge/revocation-refuses-reconnects | — | 不适用 | — | 只在登记表"说明"里 |
| DIA-6-1 | DIA §六 退避；§六 实现；NOD「一条 link」、「连进来的运行时」 | 可重试失败从 0.5 秒起翻倍至最长 30 秒，±20% 抖动 | 纯逻辑 | PT + K | bridge/dial-out-rules | K-dial `it_backs_off_and_reconnects_as_a_new_session`（缩短参数，查翻倍、上限与 ±20%，6 列） | 部分 | #631 | 默认值只在各语言单元测试；Rust `link.rs` `Backoff` 的 PT 在 #631（含 `jitter > 1` 疑点） |
| DIA-6-2 | DIA §六 退避；§六 实现；NOD「连进来的运行时」 | 一次会话持续 60 秒（连接稳定）后重置退避 | 外部 | 测试 | bridge/dial-out-rules | — | 不适用 | #633 | 计时；#633 计划 link 循环与 Go 60 秒分支的虚拟时钟测试 |
| DIA-6-3 | DIA §六 凭据被拒；§六 实现分类；NOD「一条 link」、「连进来的运行时」 | 升级时 401/403、本端拒绝服务端证书、接入点在 hello 后以 4003 拒绝这个 id，都归为凭据被拒 | 纯逻辑 | K + PT | bridge/dial-out-rules、bridge/websocket-refusal-categories | K-dial `a_wrong_token_is_retried_as_rejected`（403）、`a_rejected_id_is_retried_as_rejected`（4003） | 部分 | #615、#639 ※ | 401 与证书被拒不在 dial_conformance（`kotlin_remote.rs::a_dialer_trusts_rutis_ca` 普通测试） |
| DIA-6-4 | DIA §六 凭据被拒；NOD | 凭据被拒后每 30 秒重试，不放弃 | 外部 | 测试 | bridge/dial-out-rules | K-dial（缩短为 0.3 秒并查仍在运行） | 不适用 | #633 | 计时 |
| DIA-6-5 | DIA §六 不兼容；§六 实现分类；NOD | 400/404/405/426、子协议不符、对端 hello 的版本本端不会说：不兼容，停止并报告 | 纯逻辑 | K + PT | bridge/dial-out-rules | K-dial `an_incompatible_access_point_stops_the_dialer`（400） | 部分 | #615、#639 ※ | 只测了 400 |
| DIA-6-6 | DIA §六 实现分类 | 对端以不同于 `--peer` 的 id 打招呼归为凭据被拒（同 `IdentityMismatch`） | 纯逻辑 | K | bridge/dial-out-rules | — | 未覆盖 | #615、#639 ※ | #615 逐语言核对"按失败类型等待" |
| DIA-6-7 | DIA §六 实现分类 | 其余（连不上、超时、断开、被接管 4002）为可重试 | 纯逻辑 | K | bridge/dial-out-rules | K-dial `it_backs_off_*`（断开、连不上）、`the_same_id_takes_over`（4002） | 部分 | #615、#639 ※ | 超时未测 |
| DIA-6-8 | DIA §六 断开；NOD「连进来的运行时」 | 断开时 rutis 侧关闭该连接的实例；运行时侧先卸载全部行（清理逆序），再按退避重连 | 跨进程 | TLA + K | bridge/dial-out-rules、loader/runtime-connection-instances | AP `EndLive`；K-dial `it_backs_off_*`（先 `unloaded` 再重连） | 部分 | #615、#624 | 运行时一侧的行不在 AP 模型；"逆序"无一致性检查 |
| DIA-6-9 | DIA §六 重连；APP §四、§五；NOD | 重连是新会话、新实例、重新装载，不依赖上一次连接的状态 | 跨进程 | TLA + K | bridge/dial-out-rules | AP `OldCallsStayOld`；K-dial `it_backs_off_*`（新 generation 重新 loaded） | 部分 | #615 | #615 `RestartIsFresh` |
| DIA-6-10 | DIA §六 主动停止；§六 实现；APP §四 stop()；NOD | 主动停止（SDK `stop()`、SIGTERM/Ctrl-C）：卸载全部行后关闭连接，不再重连，退出状态 0 | 跨进程 | TLA + K | bridge/dial-out-rules、swift/app-start-stop | AP `StoppedStaysDown`（变异 `access-mut-stop-keeps-retry`）；K-dial `stopping_unloads_and_ends_without_reconnecting`（Unix） | 部分 | #615、#630 | "先卸载后关闭"不在模型；停止在模型里是终态，应用可再 `start()`（问题 4） |
| DIA-6-11 | DIA §六 实现 | 拨出方命令行 `dial:<地址> --id <id> --peer <rutis 的 id> <项目>`，凭据 `RUTIS_TOKEN`，额外 CA `RUTIS_CA` | 外部 | K | bridge/dial-out-rules | K-dial（每列按此命令启动） | 部分 | #639 ※ | `RUTIS_CA` 不在 dial_conformance |
| DIA-6-12 | DIA §六 实现；NOD「连进来的运行时」 | 状态行写 stderr：`connected to …`、`cannot connect (…); retrying in <秒>s`、`connection lost (…); retrying in …`、`refused: auth-rejected (…); retrying in …`、`refused: incompatible (…); stopping` 后以状态 1 退出 | 外部 | K | bridge/dial-out-rules | K-dial（逐条匹配前缀与退出状态） | 已覆盖 | — | 文字输出由一致性套件检查 |
| DIA-6-13 | DIA §六 实现 | 拨出方 10 秒内收不到接入点的 hello 算可重试失败 | 外部 | 测试 | bridge/dial-out-rules | — | 不适用 | #633 | 计时（#633"对端升级后不说话，10 秒后放弃"） |
| DIA-6-14 | DIA §六 实现 | 明确不保证：接入点在拨出方收到 rutis 的 hello 之后才以 4003 拒绝，被拒前可能先出现 `connected to` | 跨进程 | 测试 | bridge/dial-out-rules | — | 不适用 | — | AP 把 rutis 的 hello 放在准入之后（README"简化"），TV 需按实现顺序处理 |
| DIA-6-15 | DIA §六 实现；NOD「连进来的运行时」 | 明确不保证：两个进程用同一 id 会一直轮流顶掉对方，节奏随退避放慢到每方约 30 秒一次 | 跨进程 | 测试 | bridge/dial-out-rules | — | 不适用 | — | 模型的 `EndsHeld` 只在有限拨号额度下成立；只写在登记表"说明"里（问题 7） |
| DIA-6-16 | DIA §六 实现 Python；NOD「安全」 | Python 监听 `wss://` 在 CPython 3.13.6 上拒绝启动；拨出不受影响 | 外部 | 测试 | — | — | 不适用 | — | 平台缺陷（python/cpython#137583），`python/rutis/rutis/websocket.py:57–66` |
| DIA-6-17 | DIA §六 实现 Node、Bun；NOD「连进来的运行时」 | Node、Bun 每个会话由一个子运行时进程运行：会话的行、代理、引用随子进程结束，重连即新进程、新会话 | 跨进程 | K | bridge/dial-out-rules、loader/remote-runtime-lease | K-dial（node、bun 列）；K-leases（node、bun 列） | 部分 | #615 | 监听接管时旧子进程 5 秒后被强杀，短于清理截止（问题 3） |
| DIA-6-18 | DIA §六 实现 Node、Bun | Node、Bun 主动停止：父进程给子进程 SIGTERM，子进程在会话仍开着时卸载全部行、结束会话，父进程再关连接 | 跨进程 | K | bridge/dial-out-rules | K-dial `stopping_*`（node、bun 列，Unix） | 已覆盖 | — |  |
| DIA-6-19 | DIA §六 实现 Bun | Bun：TLS 握手失败（关闭码 1015）记为凭据被拒 | 外部 | 测试 | bridge/websocket-refusal-categories | — | 不适用 | — | Bun 平台行为，`bun/rutis-bun/test/websocket.test.ts` |
| DIA-6-20 | DIA §六 实现；NOD「连进来的运行时」 | `RUTIS_CA` 与平台默认信任的根证书一起信任 | 外部 | 测试 | bridge/tls-server-verified | — | 不适用 | — | TLS 平台配置 |
| DIA-7-1 | DIA §七 Rust 行；NOD「连进来的运行时」末段 | `runtimes.remote[].language` 接受 `swift`、`kotlin`/`kt` | 纯逻辑 | PT | loader/swift-runtimes | — | 未覆盖 | #631 | 宿主配置求值（部分） |
| APP-1-1 | APP D1；§三 | 应用里的能力按插件范式编写（inject、provides、apply、effect），同一定义既可 serve 成二进制，也可交给应用托管 | 外部 | 测试 | swift/app-start-stop | — | 不适用 | — | API 形态 |
| APP-1-2 | APP D2；§三 | 插件的装载、配置、卸载由 rutis 驱动，应用不自行决定 | 跨进程 | K | swift/app-start-stop | K-dial（swift、kotlin 列：按 rutis 的配置 loaded） | 部分 | #639 ※ | "应用不自行决定"的一半无法直接验证 |
| APP-1-3 | APP D3；范围外 | 明确不做：插件编译进应用随应用发布，设备上不装载新代码 | 外部 | 测试 | — | — | 不适用 | — |  |
| APP-1-4 | APP D4；§四"SDK 不做" | SDK 对应用只提供 `start`/`stop`；不做前后台切换、后台保活、推送唤醒、电池优化引导、权限对账 | 外部 | 测试 | swift/app-start-stop | — | 不适用 | — | 明确不保证，未登记 |
| APP-1-5 | APP D5 | 应用报的插件（`mount` 回复的 `plugins`）与 `--rutis-manifest` 的 `plugins` 内容相同 | 纯逻辑 | K | — | — | 未覆盖 | #639 ※ | 两个出口（Swift `API/Serve.swift:179`、`API/Runtime.swift:669`；Kotlin `internal/Control.kt:33`）无差分检查 |
| APP-1-6 | APP D6 | 装哪些插件、用什么配置由 rutis 配置里的行决定；应用带了而配置里没有的插件不装 | 跨进程 | K | loader/runtime-connection-instances | — | 未覆盖 | #639 ※ |  |
| APP-1-7 | APP D7；§三 | rutis 不建模设备：设备号、型号等由普通插件提供，rutis 只做插件发现与连接 | 外部 | 测试 | — | — | 不适用 | — | 范围声明 |
| APP-2-1 | APP §二 表 | rutis 不对是否在线做假设，只看服务"在"或"不在" | 跨进程 | 测试 | — | — | 不适用 | — | 无法验证的表述（Q5.1.1），实质是 DIA-2-2"实例随连接存在"（问题 6） |
| APP-2-2 | APP §二 表；§九 P2 | 系统权限被拒是 `apply` 或方法调用的普通错误，错误传回 rutis | 外部 | 测试 | swift/app-access-belongs-to-the-app | — | 不适用 | — | 需真机或模拟器授权，只在本地跑（`docs/ci.md` §12） |
| APP-3-1 | APP §三 | 同一应用内插件互调拿对象本身；跨连接、跨语言按名字经 rutis | 跨进程 | K | — | — | 未覆盖 | #639 ※ | REM-3-7 的应用版本 |
| APP-4-1 | APP §四 start() | `start()` 连上 rutis、报带的插件，之后插件由 rutis 驱动；断线自动重连，直到 `stop()` | 跨进程 | TLA + K | swift/app-start-stop | AP（拨出方重拨）；K-dial（swift 列在 `RutisApp` 上运行；kotlin 列是 jvm `Rutis.serve`，直接用 `Dialing`） | 部分 | #615、#630 | Kotlin `RutisApp` 本身不在 dial_conformance |
| APP-4-2 | APP §四 stop() | `stop()` 全部插件逆序清理，然后断开 | 跨进程 | K + SH | swift/app-start-stop | K-dial `stopping_*`（SIGTERM 路径） | 部分 | #630 | "逆序"无检查；`RutisApp.stop()` 与 SIGTERM 不是同一路径 |
| APP-4-3 | APP §四 stop() | `stop()` 之后可再次 `start()`，新一轮不带上一轮的状态；`start`/`stop` 按调用顺序生效 | 进程内并发 | TLA + SH | swift/app-start-stop | — | 未覆盖 | #615、#630 | #615 `RestartIsFresh`；#630 Kotlin Fray；疑点 `RutisApp.kt:142–144` 的 join 链；"按调用顺序生效"只在登记表与 `swift_app.rs`，APP 未写（问题 4） |
| APP-4-4 | APP §四 SDK 保证第1条 | 无论 `stop`、网络中断还是进程被系统终止，rutis 侧看到的都是：实例关闭、服务撤销、使用者停下 | 跨进程 | TLA + K | loader/runtime-connection-instances | AP `EndLive`（会话因任何原因关闭）；K-dial（停止、断开） | 部分 | #624 | 进程被杀只有 iOS 模拟器测试（本地） |
| APP-4-5 | APP §四 SDK 保证第3条 | 明确不保证：插件要持久化的数据由插件自己处理，SDK 不提供存储契约 | 外部 | 测试 | — | — | 不适用 | — |  |
| APP-4-6 | APP §四 SDK 保证第4条 | 连接状态（stopped/connecting/connected）可观察，供业务层显示 | 进程内并发 | SH | swift/app-start-stop | — | 未覆盖 | #630 | Swift 严格并发与 TSan，Kotlin Fray |
| APP-4-7 | APP §四 SDK 保证第4条 | SDK 不据连接状态做任何生命周期决定 | 进程内并发 | 测试 | swift/app-start-stop | — | 不适用 | — | 否定式、无法验证（Q5.1.1），宜删去或改写为明确不保证（问题 6） |
| APP-5-1 | APP §五 末条 | 实例外（全局）同名服务仍按现有规则：后注册的被拒绝 | 进程内并发 | PT + SH | — | — | 未覆盖 | #622、#631 | #631 内核服务注册槽 PT；#622 登记准入 shuttle |
| APP-6-1 | APP §六 末条；§八 | Swift、Kotlin 与其他语言过同一套一致性测试（`runtime_conformance`、`multilang`、`dial_conformance`） | 外部 | K | bridge/dial-out-rules | K-sess（swift、kotlin 列）；K-dial（swift、kotlin 列） | 部分 | #624 | `leases`、`host_gone` 缺 Swift/Kotlin，`sync_cycles` 缺 Kotlin；登记表没有 `kotlin/app-start-stop` |
| NOD-1-1 | NOD 开头；「一条 link」 | 节点之间一端拨号一端监听，连上后共享服务、代运行插件、共享事件；断开后拨号侧自动重连 | 跨进程 | TLA | loader/peer-rows | RL（拨号 link 重连） | 部分 | #615 | Cordis 节点的 `node/rutis-runtime/src/bridge/link.mjs` 自带退避与接管，不在对应表（#615） |
| NOD-1-2 | NOD「一条 link」；「安全」 | 拨号方出示 `RUTIS_TOKEN_<对端>`（优先）或 `RUTIS_TOKEN`，监听方用它核对来者是谁 | 纯逻辑 | PT | host/credentials-from-env | — | 未覆盖 | #631 | #631"token 优先级" |
| NOD-1-3 | NOD「一条 link」 | link 凭据被拒（403、证书不受信任）时每 30 秒重试；协议不兼容时停止并报告 | 跨进程 | TLA + 测试 | host/link-state-output、bridge/websocket-refusal-categories | RL（被吊销按认证失败等待；不兼容未建） | 部分 | #631、#633 | 30 秒计时 #633；分类见 REM-3.2-4 |
| NOD-1-4 | NOD「一条 link」 | `rutis-host run` 在 link 状态变化时打印一行（connected / down / refused: auth-rejected / stopped: incompatible），带可读类别，不带 token | 外部 | 测试 | host/link-state-output | — | 不适用 | — | 文字输出；`rutis-host/tests/links.rs` |
| NOD-1-5 | NOD「一条 link」 | 明确不保证：节点行自己的 `running` 只说明这一行在运行，不说明连上了 | 外部 | 测试 | host/row-state-lines | — | 不适用 | — |  |
| NOD-3-1 | NOD「节点之间的事件」 | `export`/`import` 的一项是名字或以 `*` 结尾的前缀（单独 `*` 为所有名字）；前缀只选事件，服务仍按确切名字 | 纯逻辑 | PT | events/link-export-import | — | 未覆盖 | #631 | #631 export/import 收窄 PT |
| NOD-3-2 | NOD「节点之间的事件」；「安全」末条 | 一个名字要在发出端 `export`、监听端 `import` 都写了才共享；同一个名字可以双向共享 | 跨进程 | K + TLA | events/link-export-import、events/node-links | K-ev `node/not-shared`、`node/shared-both-ways` | 已覆盖 | #613 |  |
| NOD-3-3 | NOD「节点之间的事件」 | 对端的监听器是本节点总线上的条目，规则与本机相同：五种方式、过滤、所有值类型 | 跨进程 | K | events/node-links、events/one-bus | K-ev（`node` 位置的放置、`node/values`、`node/function-argument`、`node/rutis-events`） | 已覆盖 | — |  |
| NOD-3-4 | NOD「节点之间的事件」 | 连接断开时对端条目离开总线，进行中的调用失败一次 | 跨进程 | K + TLA | events/node-links | K-ev `disconnect/node-link` | 部分 | #613 | 断开与宣告交错由 #613 `EventsLink` |
| NOD-3-5 | NOD「节点之间的事件」 | 事件只走一跳：一个节点导入的东西不再往外共享 | 跨进程 | TLA | events/no-multi-hop | — | 未覆盖 | #613 | #613 `NoEcho`；`events_nodes.rs::dispatch::an_imported_listener_is_not_passed_on` 普通测试 |
| NOD-3-6 | NOD「节点之间的事件」 | 对端监听器的描述带节点行所在的作用域（实例外的连接是根），只供过滤 | 跨进程 | K | events/flat、events/node-links | K-ev `node/root-hears-instances` | 部分 | #639 ※ |  |
| NOD-3-7 | NOD「节点之间的事件」示例段 | 分发按登记顺序逐个问各节点上的监听器（如 `serial`）；对端断开后只问本节点的 | 跨进程 | K + TLA | events/one-bus、events/order | K-ev（含 `node` 位置的顺序用例） | 部分 | #613 | #613 `OneOrder` |
| NOD-3-8 | NOD「节点之间的事件」 | 运行时（本机、远程、连进来的）不是节点连接，监听器直接在宿主总线上，不需要 `export`/`import` | 跨进程 | K | events/one-bus | K-ev（X、Y 为本机各语言运行时） | 部分 | #639 ※ | 远程与连进来的运行时不在 K-ev 的放置里 |
| NOD-3-9 | NOD「节点之间的事件」末段 | `events: { out, in }` 在 0.13 删除，写了报错并说明替代写法 | 纯逻辑 | 测试 | events/node-config | — | 不适用 | — | 单一配置分支，固定用例即完整（`events_nodes.rs::the_old_events_config_of_a_link_is_refused`） |
| NOD-3-10 | NOD「节点之间的事件」末段 | 0.12 及更早的节点连进来时被拒绝（`link stopped: incompatible`） | 跨进程 | 测试 | events/old-peer-refused | — | 不适用 | — | 需要已发布的旧版本二进制（`tests/e2e` 跨版本） |
| NOD-4-1 | NOD「在别的节点上运行插件」 | 对端开 `host: true`、本端开 `rows: true` 时，`peer:<对端>/<插件>` 的行在对端加载；插件用对端的服务，配置与启停由本端决定 | 跨进程 | K + TLA | loader/peer-rows | K-node（代装） | 部分 | #639、#640 ※ | 行级（PeerRows）部分属 T6 |
| NOD-5-1 | NOD「别的机器上的语言运行时」 | 远程运行时的行名为 `<运行时名>:<模块>`，模块在远程机器上解析 | 纯逻辑 | PT | loader/remote-runtime-rows | — | 未覆盖 | #631 ※ |  |
| NOD-5-2 | NOD「别的机器上的语言运行时」 | 每种远程运行时开始监听后在 stderr 打印实际绑定的地址 `rutis: listening on <地址>`；端口写 0 由系统挑选 | 外部 | 测试 | — | — | 不适用 | — | 文字输出与操作系统端口分配 |
| NOD-5-3 | NOD「别的机器上的语言运行时」 | Python 运行时会话结束打印 `rutis: session ended (<原因>); waiting for a controller`，被接替时打印 `… (replaced by a new connection)` | 外部 | 测试 | — | — | 不适用 | — | 文字输出 |
| NOD-6-1 | NOD「连进来的运行时」 | Go、Swift 二进制与 Node、Bun、Kotlin（jvm `Rutis.serve`）的 `dial:`/`listen:` 参数、环境变量、重连规则和状态行与 Python 相同 | 外部 | K | bridge/dial-out-rules | K-dial（6 列） | 部分 | #624 | 监听方向的 `leases` 缺 Swift、Kotlin |
