# 节点托管行：会话表、行键、`link.offers`、代装操作与两节点互装（TLA+）

[English](README.en.md)

用 TLC 穷举一个节点替另一个节点运行行时的交错：旧会话断线、新会话顶替，每个会话自己的已装载表，行装载时用的键，`rows.ended`/`rows.failed` 的迟到通知，行服务的导入与撤销；再加上 #640 的其余部分：`link.offers` 的 epoch 与撤回、`PeerRows` 门控、对端离线、`plugins.describe`/`schema`/`load`/`update`/`unload`、装载中被取消的托管行、两个节点互相代装与同时冷启动、服务的版本与墓碑。对应 issue [#613](https://github.com/arcships/rutis/issues/613)“并入 #640”一节；质量规范 Q11.2；设计见[远程插件](../../docs/design-remote-plugins-2026-10-03.md) §4.4、§5.1–§5.3，[远程会话](../../docs/design-remote-session-2026-10-28.md) §4.2、§4.3，[会话追踪契约](../../docs/formal-trace.md) §3.0。

这是行生命周期的第二块：`row-lifecycle` 建运行时一侧的行（一个运行时会话里的加载、卸载与重连），这一块建的是“宿主替对端装载行”。目录按问题分成五个模块；`PeerRows.tla` 是原有的会话/代次模型，其余四个补 #640 的门控、宿主操作、两节点互装和服务版本。

`peer:` 行与 `PeerRow` 已被 0.19 的远程连接定稿删除（远程会话 §八），等价路径是 `<连接行>:<插件>` 经 `run`；`run` 行的键每次装载都不同（`run` API 是 `<插件>#<n>`，loader 行是 `<实例>.<代次>`），两次装载不会共用键。#1100 定下了这条规则：键按每次装载，不用裸实例；模型用 `KeyPerLoad` 把两种规则都写着——稳定实例键（需要代次）和代码采用的每次新键。见下面“键：按 #1100 定案（每次装载一个键）”一节；本目录不改实现。

## 模块

| 模块 | 内容 | 参与者与规模 | 配置 |
| --- | --- | --- | --- |
| `PeerRows.tla` | 每个控制会话自己的已装载表、每次装载的键（以及旧的稳定实例键）、代次、迟到的 `rows.ended`/`rows.failed`、行服务随行、顶替时先清理旧会话 | 1 个宿主 + 1 个对端；2 行、2 条会话、每行 2 代 | 8 个（`peer-*`） |
| `PeerOffers.tla` | `link.offers` 的版本、`since`/epoch、旧宣告被忽略；`plugins` 族的登记、撤回、重新登记；`PeerRows` 门控、schema 刷新、离线等待、每行只启动一次 | 1 个对端 + 1 个 loader 侧；1 行、2 次登记、链接可断 | 7 个（`offers-*`） |
| `PeerHosted.tla` | 宿主侧的 `describe`/`schema`、`load`、`update`、`unload`；装载中被取消、卸载先于装载到达；`NoOrphan` | 1 个对端 + 1 个宿主；1 个键、目录 1 个插件 | 6 个（`hosted-*`） |
| `PeerPair.tla` | 两个节点互相代装、同时冷启动、断线重连；回答 `hello` 不等待自己的行；旧会话的行清理完才回答 | 2 个节点、1 条连接；每节点 1 行 | 5 个（`pair-*`） |
| `PeerServices.tla` | 服务公告/撤销的版本、导入方保留撤销的版本（墓碑）、旧公告不恢复服务 | 1 个提供方 + 1 个导入方、1 个服务 | 3 个（`svc-*`） |

## 对应的设计和代码

| 模型部分 | 设计 | 实现 |
| --- | --- | --- |
| 每个控制会话有自己的已装载表；同一个键在一个会话里只装载一次 | 远程插件 §4.4（每个运行时实例只有一个活动会话） | `crates/rutis-bridge/src/session/plugin/rows.rs:67-76`（`Table.loaded`、`Table.unloaded`）、`:125-129`（已卸载则拒绝、已装载则拒绝） |
| 行按对端给的键装载：loader 行的 `<实例>.<代次>`、`run` API 行的 `<插件>#<n>`，每次装载新取，键就是这次装载的身份；代次是模型给一次装载起的名字 | 远程插件 §5.3；#1100（定案：每次装载） | `crates/rutis-loader/src/runtime.rs:722-727`（loader 行的键）、`crates/rutis-bridge/src/session/plugin/run.rs:168`（`run` API 的）、`crates/rutis-bridge/src/session/plugin/rows.rs`（对面的 `Table.loaded` 按它记） |
| `rows.ended` 只由这一代还没被对端卸载的行发出；会话取消后不发 | 远程会话 §4.3 | `crates/rutis-bridge/src/session/plugin/rows.rs:155-193`（`follow`：`loaded.remove(&key)`，`ended && !cancelled`） |
| `rows.failed` 每代一次，宿主保留这一行，由对端卸载后重装 | 远程插件 §4.2；#688 | `crates/rutis-bridge/src/runtime/process.rs:1400-1418`；`crates/rutis-bridge/src/session/plugin/run.rs:171-175` |
| `rows.unload` 幂等：行已不在就只回答，并记下这个键 | 行契约 `rows.v2`；#1101 | `crates/rutis-bridge/src/session/plugin/rows.rs:216-228`（不在表里就只回答，并记墓碑；后到的同名 `load` 在 `:125-126` 被拒绝） |
| 顶替：关旧会话、不再读它的帧、清理旧租约的行和服务，再回答新 `hello` | 远程插件 §4.4；远程会话 §4.2 | 运行时监听端的顺序；`crates/rutis-bridge/src/session/plugin/rows.rs` 每个会话一个 `Server`；`crates/rutis-bridge/src/session/plugin/mod.rs:334-430` |
| 行随会话停止、下一个会话再装载 | 远程会话 §4.3、§7 | `crates/rutis-bridge/src/session/plugin/run.rs` 的 `RowHandle`/`RunRow`；`crates/rutis-loader/tests/session_rows.rs::rows_run_on_the_session_of_their_connection_row`、`::a_far_session_unloading_first_leaves_the_row_waiting` |
| 服务的导入随行：行卸载、结束或会话没了就撤销 | 远程插件 §2.6 决定 1、§5.3 | `crates/rutis-bridge/src/session/plugin/services.rs:232-378`（每个会话的 `Imports`）；`crates/rutis-bridge/src/runtime/process.rs:1306-1314`（`forget_exports`）、`:1356-1366`（`unload_row` 之后 `forget_exports`/`imports.forget_row`） |
| 旧会话的迟到指令不作用于新租约 | 远程插件 §4.4 | 会话各自一个处理器；旧装载的通知带的是旧键，本端没有这个键的回调（`crates/rutis-bridge/src/runtime/process.rs:531-541` 按 key 记）；`PeerRows.tla` 用 `KeyPerLoad` 写同一规则，稳定键分支靠代次 |
| `link.offers` 版本递增、旧宣告被忽略；每个族带登记编号 `since`，撤回后重新登记是新的一次提供 | 远程插件 §5.2（`link.offers`、`since`） | `crates/rutis-bridge/src/peer.rs:44-67`（`Offers`、`epoch`）、`:131-153`（`announce`）、`:160-178`（旧版本忽略）、`:253-289`（`register` 把 handler 与 `since` 一起记）；回归用例 `:309` `a_family_is_announced_only_with_its_offer` |
| 对端宣告 `plugins` 后查 schema、刷新解析再提供 `PeerRows`；行在声明完整前不启动；离线时等待而不是 Unresolved；撤回 `plugins` 时撤销 `PeerRows`，Peer 与其他功能照常 | 远程插件 §5.3 | 当前等价路径是 `run` 行：`crates/rutis-bridge/src/session/plugin/run.rs:89` 先 `describe`，`:168` 换新键，`:186` 装载；`crates/rutis-bridge/src/session/plugin/mod.rs:432-461` 等对端提供 `rows` 后才建立进程；`crates/rutis-bridge/src/peer.rs` 的 `offers` 观察 `plugins` 的 epoch |
| `plugins.describe`/`rows.schema` 返回插件声明，未安装为 NotFound；`load` 按 `isolate`/`inject` 装载；`update`、`unload` 只作用于已装载的键 | 远程插件 §5.2 | `crates/rutis-bridge/src/session/plugin/rows.rs:96-110`（`schema`）、`:112-153`（`load`）、`:195-214`（`update`）、`:216-237`（`unload`） |
| 装载中被取消、或卸载先于装载到达时，不留下没人管的插件（#625 的疑点、#1101） | 远程插件 §5.1、§5.3 | `crates/rutis-bridge/src/session/plugin/rows.rs:112-153`（键先插入表再等 apply；失败时移除并 dispose）、`:216-228`（不在表里的卸载记墓碑，后到的 `load` 拒绝），墓碑随会话的表消失；`crates/rutis-bridge/src/session/plugin/run.rs:42-51`（`RowHandle::drop` 中止装载并 dispose 已挂上的 fiber）；回归用例 `crates/rutis-bridge/tests/session_plugin.rs::an_unload_ahead_of_its_load_leaves_no_row`、`::an_old_sessions_unload_does_not_refuse_a_new_sessions_load` |
| 服务公告/撤销的版本取自会话级单一递增序列；导入方保留撤销的版本，旧公告不恢复 | 远程插件 §5.2 | `crates/rutis-bridge/src/services.rs:33-135`（`ExportOne`、`next_version`）；`crates/rutis-bridge/src/session/plugin/services.rs:239-245`（版本不增则忽略）、`:335-378`（撤回与墓碑） |
| 两个节点互相代装；功能插件只依赖会话就绪，不等待远程行 | 远程插件 §5.1 第 1 条、§10 | `crates/rutis-bridge/src/session/plugin/mod.rs:334-430`（`mount` 先登记 rows 再回答）、`crates/rutis-bridge/src/session/plugin/rows.rs:318-333`（每个会话的 `Rows`） |
| 断开时本链接代装的行停下等待；重连后重新装载 | 远程插件 §3.2、§5.3 | `crates/rutis-loader/tests/session_rows.rs` 的两个用例；`crates/rutis-bridge/src/session/plugin/run.rs` 的行随会话 |

## 模型

### `PeerRows.tla`：会话、键、代次

- **参与者**：一个宿主（每个会话一张表、行的结束/失败通知、服务导入）、一个对端节点（自己的两行，跨会话重装）和它的两条控制会话：会话 1（旧）与会话 2（新，顶替用）。两行 `a`、`b`，每行最多 2 代（每个会话一代）；通知按哪一代的键寻址由 `KeyPerLoad` 决定（每次装载一个键，或旧规则的稳定实例键）。
- **会话**：`1..MaxSessions`（2 条）。对端按序拨号；一条会话两个方向各自 FIFO、延迟任意长。断开后对端不再读它（`PLoss`），宿主也不再读它的帧（`CanRead`）；已经写出、还没读到的 `rows.ended`/`rows.failed` 可能迟到（`late`）。
- **对端**（`PDial`、`PGreeted`、`PLoad`、`PLoaded`、`PUnload`、`PUnloaded`、`PEnded`、`PFailed`、`PLoss`、`PLateEnded`、`PLateFailed`）：`PLoad(r)` 在会话上装载一行，代次加一；`PEnded`/`PFailed` 只对“这一行当前代次”的帧起作用；`PLoss(s)` 让这一会话上的行回到 `idle`，导入的服务消失，没读到的结束/失败记成迟到通知；`PLateEnded`/`PLateFailed` 在正确协议下什么也不做，变异 `lateEnding` 在“通知的键就是当前已装载行的键”时照样作用（`RowKey`：`KeyPerLoad` 时为代次键，稳定键时为行）。
- **宿主**（`HTakeUp`、`HAnswer`、`HEndSession`、`HReadLoad`、`HReadUnload`、`HEndSelf`、`HFail`、`HSweep`、`Break`）：`HTakeUp(s)` 读到新连接的 `hello`，关闭旧会话并开始清理；`HAnswer(s)` 所有更早会话的行都清理完（`Swept`）才回答；`HReadLoad` 把行装进这一会话自己的表；`HReadUnload` 幂等；`HEndSelf`/`HFail` 是行自行结束/代码失败；`HEndSession`/`HSweep` 结束会话并逐行清理；`Break(s)` 是环境。

### `PeerOffers.tla`：`link.offers`、门控、离线

- **对端**（`BOffer`、`BWithdraw`）：登记 `plugins` 时把族和它的 `since` 放进同一份快照；第二次登记是更高的 `since`；撤回是一份不带该族的快照。`twoOffers` 变异是 #435/#437 的旧形状：快照先列出族但没有 `since`（epoch 0），下一份才带真正的 `since`。
- **loader 侧**（`ADeliver`、`ARefresh`、`ARowLoaded`、`ALinkDown`、`ALinkUp`）：只应用版本更高的快照；`plugins` 的 epoch 变了就关旧行、把门控置为 `refreshing`，`ARefresh` 完成 schema 刷新后开门，`ARowLoaded` 让行运行；撤回把 epoch 变成 0、门控关闭、行回到 `idle`；链接断开时行等待、忘掉已应用的快照，链接回来时对端重发当前快照。
- **性质**：`EpochWithFamily`（快照列出族就一定带它的登记号）、`GatedStart`（行只在门控开、`plugins` 在、epoch 非 0 时启动）、`WithdrawalStops`、`NoFailedRow`（#438：门控在行 apply 前撤下不会让行停在 Failed）、`OfflineWaits`（离线时行等待，不是运行）、`ReloadsOnOffer`（新登记最终被跟上）。

### `PeerHosted.tla`：代装操作与孤儿

- **对端**：`Describe(k)`（`plugins.describe`/`rows.schema`）、`Load(k)`（`plugins.load`）、`Update(k)`/`UpdateEarly(k)`、`Unload(k)`、`CancelLoad(k)`（行在装载中被取消）、`Close`（会话结束）。
- **宿主**：`loading`（已接受、apply 未完成）、`loaded`（已 apply）、`tomb`（装载期间收到卸载：apply 后立即 dispose）、`orphan`（已装载但对端已不要）、`schema`/`described`（describe 的结果）、`badLoad`/`badUpdate`（幽灵）。
- **装载被取消**：`CancelLoad` 在基础模型里发卸载；宿主若还在 apply，就记墓碑，`FinishLoad` 完成时 dispose。`cancelNoUnload` 变异是 #625 的另一半：对端没有 disposer，什么也不发，宿主 apply 完成后留下孤儿。`Unload` 在基础模型里对“还在装载”的键记墓碑；`noTombstone` 变异忘掉它，后到的 `FinishLoad` 插入这个键，留下孤儿。
- **性质**：`NoOrphan`、`DescribeExact`（describe 的结果与目录一致）、`LoadOnlyDescribed`（load 只装载 describe 找到的插件）、`UpdatingLoadedOnly`（update 不更新还在 apply 的行）。

### `PeerPair.tla`：两节点互装、同时冷启动

- **两个节点** A、B，一条连接；每个节点有一条自己的行在对方那里（`out`），一张对方的行表（`inn`）。`Connect` 建立连接并清空回答状态，但保留旧会话留在 `inn` 里的行；`Clean(n)` 清理旧会话的行；`Answer(n)` 回答 `hello`，基础规则只要求 `inn[n]` 已清空，不要求自己的 `out[n]` 已装载；`StartOut(n)` 在对方回答后装载自己的行；`Loaded(n)` 完成；`Break` 断链，行停下等待。
- **性质**：`CleanedBeforeAnswer`（回答时旧行已清理）、`RowsStopOnBreak`、`NoWaitingForOut`（回答不等待自己的行；`waitOut` 变异记录这个等待）、`EventuallyBothLoaded`（链接保持时两行最终都运行）。

### `PeerServices.tla`：服务版本与墓碑

- 提供方用一条递增的版本序列发 `announce`/`withdraw`；导入方只应用比 `seen` 新的消息，并保留撤销的版本（`withdrawnAt`）；比撤销更旧的公告不恢复服务。`staleWins` 变异让旧公告照样提供；`noTombstone` 变异在“撤销先到、此前没有公告”时忘掉撤销的版本。
- **性质**：`NoStaleRestore`。

## 键：按 #1100 定案（每次装载一个键）

| | 定案 |
| --- | --- |
| 行的键 | loader 行：`<实例>.<代次>` —— 行光纤的 `InstanceId` 加一个进程内递增的代次（`crates/rutis-loader/src/runtime.rs:722-727`）；`run` API 的行：`<插件>#<n>`（`crates/rutis-bridge/src/session/plugin/run.rs:168`） |
| 一次装载 | 一个键；代次让键唯一于这次装载。loader 行重载时实例部分不变（loader 在同一 fiber 上换插件），所以键不会重复 |
| 迟到的 `rows.ended`/`rows.failed` | 带旧键；不是新行的键，本端也没有它的回调（`crates/rutis-bridge/src/runtime/process.rs:531-541`），不结束也不重启任何行 |
| 迟到的 `rows.unload`/`rows.update` | 带旧键；对面的 `Table.loaded` 没有这个键（卸载幂等，不在表里时只记墓碑；更新被拒：`crates/rutis-bridge/src/session/plugin/rows.rs:195-237`） |
| 同一实例跨重启（重载、重连） | 不靠键识别：每次装载都是新键；跨装载识别一行的是 loader 的行 id（事件描述符用行 id，不用键） |
| 一个会话里同键的第二次装载 | 拒绝（`Table.loaded`/`Table.unloaded`）；也不会发生：每次装载都取新键；旧键的墓碑只拒绝那个键，不拒绝下一代的键（#1101） |

不选“只用实例键”的理由：loader 行重载时行光纤不变（loader 在同一 fiber 上 `update` 插件），`ctx.instance()` 也就不变；只用它当键，上一代的迟到通知会命中新代——这正是 `PeerRows.tla` 的 `KeyPerLoad = FALSE` 分支和 `peer-mut-late-ending` 的反例。代码已经在实例后面加了代次（`run` API 用新计数器）：每次装载都取新键，不需要改实现。行跨装载的稳定身份是 loader 的行 id，不是键；`RowHandle` 和 loader 持有当前键，`unload`/`update` 不需要稳定名字。

模型里的选择：`PeerRows.tla` 用 `KeyPerLoad` 把两种规则都写着。`FALSE` 是只用稳定实例当键，通知必须带它属于哪一代——只按行键匹配就是 `peer-mut-late-ending` 的反例。`TRUE` 是代码的每次装载新键：一个已装载的行只有当前这一代的键，`peer-fresh-keys` 用同一个“只按键匹配”的策略，不违反任何不变式。`PeerRows.tla` 的其余规则（会话表、先清理再回答、服务随行、卸载幂等）两种键都适用。

实现证据：`crates/rutis-loader/tests/session_rows.rs::a_reload_takes_a_new_key`（重载后新键）、`::two_sessions_do_not_share_a_key`（同一插件在两个会话里键不同）、`::a_late_notice_of_an_old_key_reaches_no_row`（旧键的 `rows.ended`/`rows.unload` 不动新行，当前键的 `rows.unload` 确实卸载）。

## 修复前的配置（#438、#435/#437）

- **#435 / #437**（两个族同时登记被宣告成两次提供，`peer:` 行被重启）：当前 `peer.rs` 的回归用例 `a_family_is_announced_only_with_its_offer`（`:309`）就是这件事；`BOffer` 的 `twoOffers` 变异复现它的形状——一份快照列出 `plugins` 但没有 `since`（epoch 0），下一份才带真正的 `since`，loader 看到同一登记的两个 offer。`offers-mut-two-offers` 预期违反 `EpochWithFamily`。当前代码把 handler 与 `since` 一起注册（`register`，`:253`），所以这个变异不是当前行为。
- **#438**（`PeerRows` 在行排上之后、应用之前被撤下，行停在 Failed）：`PeerRowsPlugin`/`PeerRow` 已随 0.19 删除，**照旧配置无法对当前代码复现**；当前等价路径是 `run` 行，没有这个门控。模型保留这条设计规则，用 `offers-mut-gate-fail` 锁定它的形状：`plugins` 在行 `starting` 时被撤回（`ADeliver` 的 `gateCloseFails`），行如果被置为 `failed` 就违反 `NoFailedRow`；基础模型让它回到 `idle` 等待下一次提供。当前 `run` 行的对应证据是 `session_rows.rs::a_far_session_unloading_first_leaves_the_row_waiting`（远端会话先卸载时行等待），不是旧 `PeerRows` 配置。

## 简化

- 规模是有意的有限：`PeerRows` 是 1 宿主 + 1 对端、2 行、2 会话、每行 2 代；`PeerOffers` 1 行、2 次登记；`PeerHosted` 1 个键、目录 1 个插件；`PeerPair` 2 个节点、1 条连接、最多 2 轮；`PeerServices` 1 个服务。真实实现的重连次数、退避和计时不建：环境只体现为“某个时刻断线/链接回来”，比真实计时更一般。
- `PeerRows` 的两条会话是顺序的：新会话只在旧会话断线之后才拨。同一 id 的并发握手、握手期间旧会话仍活着、接入点的接管顺序不建（`remote-lease` 与 `access-*` 覆盖）；这里只保留“宿主回答新 `hello` 时旧会话的行可能还没清完”这个交错。
- 代次是模型给一次装载起的名字：稳定键（`KeyPerLoad = FALSE`）时 `PeerRows` 的帧里带它，变异 `lateEnding` 就是“只按行键、不看它属于哪次装载”；每次装载一个新键（`TRUE`）时键就是代次，帧里的键已经指明装载。实现里 `run` 行靠新键区分（见上节），通知还只落在登记它的那个会话的 `Process` 上。
- 会话取消时不发 `rows.ended`（`rows.rs` 的 `!cancelled`）：行按断线处理，等下一个会话再装载。`PLateEnded`/`PLateFailed` 的正确行为因此是什么也不做；迟到是环境可能做出的事，不是协议允许的指令。
- 服务公告/撤销在 `PeerRows` 里不是帧：装载被接受时导入、行消失时撤销，各是一个原子步。`PeerServices` 只建版本与墓碑，不建服务调用的形状、引用计数和真实网络延迟；多跳不建（见“未覆盖”）。
- `PeerOffers` 的链接断开/回来是原子的：`ALinkDown` 忘掉已应用的快照，`ALinkUp` 由对端重发当前快照；真实拨号、重连退避、握手计时不建。`PeerOffers` 只建一个 loader 侧的行，schema 刷新是一步；`plugins.describe` 的字段（版本、integrity）不建，只建 found/missing。
- `PeerHosted` 把一个键的装载拆成“接受”和“apply 完成”两步，卸载和更新各一步；目录只有“有/没有”，不建插件的配置、依赖和真实 apply 时间。会话结束是原子的；`NoOrphan` 的结论针对这个抽象。
- `PeerPair` 的一条连接、两个节点、每节点一行是有限的；鉴权、10 秒握手截止、能力协商不建；“清理完旧行”是一步，真实实现里是逐行 dispose。`waitOut` 变异只记录“回答等自己的行”这个错误策略，不建超时。
- `PeerServices` 的一条会话、一个服务、一条消息序列；消息可乱序到达，版本排序；不建多跳、引用转发、调用链路由和多条链接（#626）。
- 事件、代理、`plugins.update` 的配置差异、租约升级、内存序、任务调度和帧编码不建；每个动作原子。
- 一个会话里同键的第二次装载（`rows.rs` 的拒绝）在 `PeerRows` 里不可达：对端的行在 `idle` 时才会发装载，一个会话里每行只装载一次；模型把这条写成“已装载就忽略”，不建拒绝的回复。`PeerHosted` 的基础模型要求 describe 找到插件才 load；重复 load 由 `loads[k] < MaxLoads` 限次，不单独建拒绝路径。

## 不变式与活性

| 名字 | 模块 | 内容 |
| --- | --- | --- |
| `OneInstanceAtATime` | PeerRows | 一个行的实例同时只在一个会话里装载：旧会话的行清理完之后，新会话才装载同一实例 |
| `OwnGenerationOnly` | PeerRows | 对端只对当前代次的 `rows.ended`/`rows.failed` 起作用：旧代迟到的通知不结束、不重启新代 |
| `ServicesFollowRows` | PeerRows | 一行的服务只在提供它的那一代装载期间被导入：行卸载、自行结束或会话结束时撤销，导入不留在行之外 |
| `CleanupOnce` | PeerRows | 一行的同一代最多处置一次：`rows.unload`、行自行结束和会话清理不会处置两次 |
| `EndedRowsCleaned` | PeerRows | 活性：一个会话结束之后，它装载过的行最终被清理（服务随之撤销） |
| `EpochWithFamily` | PeerOffers | 快照列出 `plugins` 时一定带它的登记号：不会出现“有族、epoch 为 0”的中间快照 |
| `GatedStart` | PeerOffers | 行只在门控开、`plugins` 在、epoch 非 0 时启动（`starting`/`running`） |
| `WithdrawalStops` | PeerOffers | 撤回 `plugins` 后门控关闭、行不运行，Peer 不受影响 |
| `NoFailedRow` | PeerOffers | 门控在行 apply 前撤下不会让行停在 `failed`（#438 的形状） |
| `OfflineWaits` | PeerOffers | 链接断开时门控关闭、行等待，不运行 |
| `ReloadsOnOffer` | PeerOffers | 活性：链接保持时，loader 最终运行对端当前的 `plugins` 登记 |
| `NoOrphan` | PeerHosted | 对端已不要的行不会留在宿主：装载中被取消的行在 apply 后 dispose，先到的卸载留下墓碑 |
| `DescribeExact` | PeerHosted | describe 的结果与目录一致：有则 found，没有则 missing |
| `LoadOnlyDescribed` | PeerHosted | load 只装载 describe 找到的插件 |
| `UpdatingLoadedOnly` | PeerHosted | update 不更新还在 apply 的行 |
| `CleanedBeforeAnswer` | PeerPair | 回答 `hello` 时，旧会话留在本节点的行已经清理 |
| `RowsStopOnBreak` | PeerPair | 断链后两个节点的出站行都停下等待 |
| `NoWaitingForOut` | PeerPair | 回答不等待自己的出站行（`waitOut` 变异设置这个幽灵） |
| `EventuallyBothLoaded` | PeerPair | 活性：链接保持时两个节点的行最终都运行 |
| `NoStaleRestore` | PeerServices | 撤销是墓碑：比撤销更旧的公告不会恢复服务 |

## 结果

TLC 2.19（tla2tools v1.7.4），Java 21.0.12（Temurin），32 核，`-workers 4`，2026-11-10；`peer-fresh-keys` 2026-11-16。`通过` 指穷举完成、所列性质全部成立；被违反的配置找到第一个反例就停，状态数取决于搜索顺序，每次可能不同。本目录的配置都很小，全部跑过；整个目录约 30 秒。

| 配置 | 模块 | 内容 | 结果 | 不同状态数 | 时间 |
| --- | --- | --- | --- | --- | --- |
| `peer-healthy` | PeerRows | 2 行，2 条会话，可断线；行可自行结束；全部安全不变式 | 通过 | 36,083 | 1–2 s |
| `peer-failure` | PeerRows | 2 行，2 条会话；行的代码可失败，对端卸载后重装；全部安全不变式 | 通过 | 161,503 | 2 s |
| `peer-clean` | PeerRows | 同 `peer-healthy`，只检查活性 | 通过 | 36,083 | 1–4 s |
| `peer-fresh-keys` | PeerRows | 每次装载一个键（如现行 `run` 行）；旧装载的迟到结束/失败不命中任何行，即使对端只按键匹配 | 通过 | 1,269,856 | 5 s |
| `peer-mut-late-ending` | PeerRows | 迟到的旧代 `rows.ended`/`rows.failed` 按行键作用于新代 | 违反 `OwnGenerationOnly` | —（找到反例即停） | 0–1 s |
| `peer-mut-two-sessions` | PeerRows | 顶替时不等旧会话的行清理完就回答新 `hello` | 违反 `OneInstanceAtATime` | —（找到反例即停） | 0–1 s |
| `peer-mut-keep-rows` | PeerRows | 宿主结束会话但不清理它的行 | 违反活性 | 36,083（找到反例即停） | 1–3 s |
| `peer-mut-service-residue` | PeerRows | 行卸载或会话结束时不撤销它的服务 | 违反 `ServicesFollowRows` | —（找到反例即停） | 0–1 s |
| `offers-healthy` | PeerOffers | 一次登记、schema 刷新、门控、行启动；链接保持 | 通过 | 5 | 0–1 s |
| `offers-withdraw` | PeerOffers | 登记、撤回、重新登记；门控与行跟随，最后一次登记最终被跟上 | 通过 | 29 | 0–1 s |
| `offers-offline` | PeerOffers | 链接断开再回来：行等待，回来后重新装载 | 通过 | 18 | 0–1 s |
| `offers-mut-two-offers` | PeerOffers | 一份快照列出 `plugins` 但没有 `since`，同一登记出现两个 offer（#435/#437） | 违反 `EpochWithFamily` | —（找到反例即停） | 0–1 s |
| `offers-mut-gate-fail` | PeerOffers | 门控在行 apply 前撤下，行被置为 Failed（#438 的形状） | 违反 `NoFailedRow` | —（找到反例即停） | 0–1 s |
| `offers-mut-keep-row` | PeerOffers | 撤回 `plugins` 后门控不关、行继续运行 | 违反 `WithdrawalStops` | —（找到反例即停） | 0–1 s |
| `offers-mut-no-reload` | PeerOffers | 新登记不被跟上 | 违反活性 | —（找到反例即停） | 0–1 s |
| `hosted-healthy` | PeerHosted | describe、load、update、unload、装载中取消；无孤儿 | 通过 | 42 | 0–1 s |
| `hosted-mut-cancel` | PeerHosted | 装载中被取消但不发卸载（没有 disposer） | 违反 `NoOrphan` | —（找到反例即停） | 0–1 s |
| `hosted-mut-unload-first` | PeerHosted | 卸载先于装载到达，宿主不记墓碑 | 违反 `NoOrphan` | —（找到反例即停） | 0–1 s |
| `hosted-mut-update` | PeerHosted | update 到达还在 apply 的行 | 违反 `UpdatingLoadedOnly` | —（找到反例即停） | 0–1 s |
| `hosted-mut-load-missing` | PeerHosted | 没有 describe 找到就 load | 违反 `LoadOnlyDescribed` | —（找到反例即停） | 0–1 s |
| `hosted-mut-describe` | PeerHosted | describe 对不存在的插件回答 found | 违反 `DescribeExact` | —（找到反例即停） | 0–1 s |
| `pair-healthy` | PeerPair | 两个节点同时冷启动，互相代装 | 通过 | 26 | 0–1 s |
| `pair-reconnect` | PeerPair | 断线一次再回来：行停下、旧行清理、两边重装 | 通过 | 70 | 0–1 s |
| `pair-mut-wait-out` | PeerPair | 回答等自己的行装载完，两个冷节点互等 | 违反活性 | —（找到反例即停） | 0–1 s |
| `pair-mut-keep-rows` | PeerPair | 断链后出站行仍标记为已装载 | 违反 `RowsStopOnBreak` | —（找到反例即停） | 0–1 s |
| `pair-mut-answer-before-clean` | PeerPair | 旧会话的行未清理就回答新 `hello` | 违反 `CleanedBeforeAnswer` | —（找到反例即停） | 0–1 s |
| `svc-healthy` | PeerServices | 公告、撤销、再公告；乱序到达，旧公告被忽略 | 通过 | 74 | 0–1 s |
| `svc-mut-stale` | PeerServices | 比当前版本旧的公告仍然提供 | 违反 `NoStaleRestore` | —（找到反例即停） | 0–1 s |
| `svc-mut-no-tombstone` | PeerServices | 撤销先到时不保留版本，旧公告恢复服务 | 违反 `NoStaleRestore` | —（找到反例即停） | 0–1 s |

## 变异与反例

每个被检查的性质都配一个故意引入的错误，TLC 必须找到反例（`expected.txt` 的 `violated:<名字>`；活性为 `violated:liveness`）：

| 配置 | 变异 | 违反 | 反例 |
| --- | --- | --- | --- |
| `peer-mut-late-ending` | `PLateEnded`/`PLateFailed` 不看代次：稳定键下按行键命中，结束或重启这一行 | `OwnGenerationOnly` | 会话 1 上 `a` 是第 1 代；插件自行结束，`rows.ended(a,1)` 写进会话 1 的队列但没被读；连接断，对端拨会话 2，把 `a` 装载成第 2 代；迟到的结束到达，把新代结束（`pEndedFrom = 1`，`pkey = 2`） |
| `peer-mut-two-sessions` | `HAnswer` 不要求更早会话已 `Swept` | `OneInstanceAtATime` | 会话 1 上 `a` 是第 1 代；断线；对端拨会话 2；宿主关旧会话但不等清理就回答；`loaded[1][a] = 1` 与 `loaded[2][a] = 2` 同时存在 |
| `peer-mut-keep-rows` | `HSweep` 关闭 | 活性 | 会话 1 装载了行后断线；宿主结束会话，但行永远留在表里，`Swept(1)` 永远不成立 |
| `peer-mut-service-residue` | 行卸载、自行结束或会话清理时不撤销服务 | `ServicesFollowRows` | 会话 1 上 `a` 装载，服务导入为 `<<1,1>>`；插件自行结束，宿主去掉行但导入还在 |
| `offers-mut-two-offers` | 登记快照先列族、不带 `since`，下一份才带真 `since`（#435/#437） | `EpochWithFamily` | `aFams = {"rows","plugins"}` 而 `aEpoch = 0`：同一登记被看成两个 offer |
| `offers-mut-gate-fail` | 门控在行 `starting` 时撤下，行置 `failed`（#438） | `NoFailedRow` | `plugins` 登记后 loader 开始 `starting`；撤回到达，行变 `failed`，不再等待下一次提供 |
| `offers-mut-keep-row` | 撤回 `plugins` 不关门、不停行 | `WithdrawalStops` | 快照已不带 `plugins`，但 `aGate = "open"`、`aRow = "running"` |
| `offers-mut-no-reload` | 新 epoch 只改 `aEpoch`，不开门、不刷新 | 活性（`ReloadsOnOffer`） | `bSince = 1`、`aEpoch = 1`，但门控一直 `closed`、行一直 `idle` |
| `hosted-mut-cancel` | 装载中被取消，对端不发卸载（没有 disposer） | `NoOrphan` | `Load(a)`、`CancelLoad(a)`、`FinishLoad(a)`：宿主 `loaded = {"a"}`，`orphan = {"a"}` |
| `hosted-mut-unload-first` | 卸载先于装载到达，宿主不记墓碑 | `NoOrphan` | `Load(a)` 还在 `loading`，`Unload(a)` 被忘掉；`FinishLoad(a)` 插入 `a`，`orphan = {"a"}` |
| `hosted-mut-update` | update 到达还在 apply 的行 | `UpdatingLoadedOnly` | `Load(a)` 后、`FinishLoad(a)` 前 `UpdateEarly(a)`，`badUpdate = TRUE` |
| `hosted-mut-load-missing` | 允许没有 describe 找到就 load | `LoadOnlyDescribed` | 目录为空，`Load(a)` 仍被接受，`badLoad = TRUE` |
| `hosted-mut-describe` | 对不存在的插件回答 found | `DescribeExact` | 目录为空，`Describe(a)` 后 `schema[a] = "found"` |
| `pair-mut-wait-out` | 回答要求自己的出站行已装载 | 活性（`EventuallyBothLoaded`） | 两个节点都冷，双方的 `Answer` 都在等自己的 `out`，`StartOut` 又在等对方回答；链接保持但两行永远不装载 |
| `pair-mut-keep-rows` | 断链后出站行仍为 `loaded` | `RowsStopOnBreak` | `Break` 后 `out[A] = out[B] = "loaded"` |
| `pair-mut-answer-before-clean` | 回答不要求旧会话的行已清理 | `CleanedBeforeAnswer` | 第一轮装载后断链，第二轮 `Answer` 时 `inn` 还有旧行，`badAnswer = TRUE` |
| `svc-mut-stale` | 比当前版本旧的公告仍然提供 | `NoStaleRestore` | 撤销版本 2 后，公告版本 1 到达，`state = "provided"`、`bad = TRUE` |
| `svc-mut-no-tombstone` | 撤销先到时不保留版本 | `NoStaleRestore` | 撤销版本 2 先到，`seen` 仍为 0；公告版本 1 到达后提供，`bad = TRUE` |

`peer-mut-two-sessions` 的变异同时会让“回答前必须清理完”的规则不成立；分开列配置是为了让每个反例只对应一个结论（同 `row-lifecycle`、`event-close` 的做法）。`peer-fresh-keys` 是同一个 `lateEnding` 策略在 `KeyPerLoad = TRUE` 下的样子，找不到反例：旧装载的通知带的是 `<<行, 代次>>`，不是当前已装载行的键。`CleanupOnce` 没有单独的变异：`PeerRows` 里三条处置路径都先检查这一代还在表里，第二次处置不可达；任何一条忘了检查都会让它失败。`PeerHosted` 不重复建“处置一次”，它由 `PeerRows` 的 `CleanupOnce` 覆盖。

## 结论

- 按远程插件 §4.4 的规则——顶替时不再读旧会话、清理完旧会话的行才回答新 `hello`——在穷举范围内成立：同一个实例不会同时在两个会话里运行；旧会话清理完之后新会话才装载；行随会话停止、下一个会话再装载。
- 对端只按“这条通知属于哪一次装载”处置 `rows.ended`/`rows.failed`：旧代迟到的结束不会结束新代，迟到失败的不重启新代。稳定键下这必须靠通知里的代次，只按行键就正好是 `lateEnding` 的反例；按定案的每次新键（`peer-fresh-keys`），键本身就指明装载，旧装载的迟到通知带的是当前任何已装载行都没有的键；代价是同一实例跨重启不可识别（#1100）。
- `link.offers` 的版本和 `since` 在穷举范围内足够：旧宣告被忽略；同一登记不会出现“有族、无 epoch”的中间快照；撤回只撤 `PeerRows` 和它的行，Peer 不受影响；新登记最终被跟上。
- `PeerRows` 门控的规则成立：schema 刷新完才开门；离线时行等待、回来再装载；门控在 apply 前撤下时行不会停在 Failed（#438 的形状）。
- 宿主的代装操作按规则成立：describe 与目录一致；load 只装载 describe 找到的插件；update 不作用于还在 apply 的行；取消装载或先到的卸载不会留下孤儿——模型要求宿主记墓碑、对端为装载中的行发卸载（#625 的两半）。宿主现在记墓碑（#1101）：`rows.unload` 找不到行时记下这个键，后到的同名 `load` 被拒绝；墓碑按会话的表存放，随会话消失，也不跨键，`run` 行的下一代用新键照常装载。对端为装载中的行发卸载那一半仍未由代码保证（`RowHandle::drop` 中止装载，不发卸载），见下面“模型没有证明的”。
- 两个节点互相代装、同时冷启动时不会互相等待：回答 `hello` 不依赖自己的出站行；断链后行停下，旧行清理完才回答下一轮，之后两行重新运行。
- 服务的版本与墓碑成立：旧公告不恢复已撤销的服务。
- 模型没有证明的：同一 id 的并发握手与接入点接管顺序（`remote-lease`、`access-*` 覆盖）、真实退避与计时、多跳服务（#626）、`plugins.describe` 的字段内容、服务调用的形状与引用释放、事件与代理、各语言运行时的监听端差异、内存序和真实调度；以及“卸载先于装载插入”的交错是否真会由某个对端发出——模型允许它，`rows.rs` 现在靠墓碑关闭它（#1101），但 `RowHandle::drop` 在装载中不发卸载（#625 的另一半），不依赖这一点的对端（例如重试或另一个 SDK）仍可能走到它。

## 怎么运行

需要 Java 11 以上和 `tla2tools.jar`（[v1.7.4](https://github.com/tlaplus/tlaplus/releases/tag/v1.7.4)，sha256 `936a262061c914694dfd669a543be24573c45d5aa0ff20a8b96b23d01e050e88`）。jar 不要提交进仓库。

```sh
curl -sSLo /tmp/tla2tools.jar https://github.com/tlaplus/tlaplus/releases/download/v1.7.4/tla2tools.jar
TLA2TOOLS=/tmp/tla2tools.jar specs/peer-rows/run.sh                     # 全部配置
TLA2TOOLS=/tmp/tla2tools.jar specs/peer-rows/run.sh offers-healthy      # 单个配置
TLC_WORKERS=4 TLA2TOOLS=/tmp/tla2tools.jar specs/peer-rows/run.sh      # 限定 TLC 的线程数
```

`run.sh` 按前缀选模块：`peer-*` → `PeerRows.tla`，`offers-*` → `PeerOffers.tla`，`hosted-*` → `PeerHosted.tla`，`pair-*` → `PeerPair.tla`，`svc-*` → `PeerServices.tla`。它把每个配置的结果和 `expected.txt` 比较，全部符合时退出码为 0；日志和反例写在 `TLC_OUT`（默认一个临时目录）。本目录没有需要放到每周运行的大配置；CI 的 `specs` 任务矩阵还没加 `peer-rows`（见 [CI](../../docs/ci.md) §4，属于 #640 的收口）。

## 维护

远程插件 §4.4、§5.1–§5.3、远程会话 §4.2、§4.3 或 `peer.rs` 的 `Offers`/`since`/`register`、`session/plugin/rows.rs` 的每会话表/装载/卸载、`session/plugin/run.rs` 的行键与 `describe`、`session/plugin/services.rs` 的导入版本、`runtime/process.rs` 的 `rows.ended`/`rows.failed`/`unload_row`、`services.rs` 的 `ExportOne`/`next_version` 改变时，同步修改对应的模型、`expected.txt`、`run.sh` 和本文件（规范 Q11.2.3）。
