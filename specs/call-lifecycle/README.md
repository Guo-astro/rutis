# 会话中的调用：发起、执行、答复、取消、引用与会话关闭（TLA+）

[English](README.en.md)

用 TLC 穷举一个会话里调用从发起到结算的全部交错：调用号、请求到达、方法执行、答复、取消、迟到答复、引用的授予与释放，以及会话关闭。对应 issue [#613](https://github.com/arcships/rutis/issues/613) 的调用生命周期部分；质量规范 Q11.2；设计见[会话协议](../../docs/protocol-session.md) §5、§6、§7、§10，[会话追踪契约](../../docs/formal-trace.md) §3.4、§3.5，[形式化验证设计](../../docs/design-formal-verification-2026-11-08.md) §3.4。这是 #613 的第二块：第一块是 [`row-lifecycle`](../row-lifecycle/README.md)（一个会话的行）。

两个模块：

| 模块 | 参与者 | 回答的问题 |
| --- | --- | --- |
| `CallLifecycle.tla` | 一个会话的两个端点 | 一次调用从发起到结算；读线程写回与背压；取消如何结算（协议 5）；本地放弃的同步调用；参数引用的归还；`PinnedTarget`；关闭后仍在通道上的帧；宿主 `outbound_limit`；调用方的取消表 |
| `CallRelay.tla` | A、中继 M、持有对象的 C，两条会话 | 多跳中继的取消（`CancelReaches`）与释放（`RelayReleasesImport`）；两跳各自的取消表与确认 |

本目录不建跨运行时的等待图与调用链 `path`、同步调用的成环判断、事件帧序、节点托管行、握手与鉴权；同步等待只在“本地放弃”这一处记一条 ghost 边。`CallRelay.tla` 不建关闭（见“未覆盖”）。

## 对应的设计和代码

| 模型部分 | 设计 | 实现 |
| --- | --- | --- |
| 调用号每端每会话从 1 严格递增，一个号只属于一次调用；已有号再次出现不是新调用 | 会话协议 §5.1（`SESS-32`、`SESS-33`、`SESS-34`） | `crates/rutis-bridge/src/session/rpc.rs` 的 `Calls::admit` 与 `Calls::received` |
| 一次调用恰好结算一次：回复到达时先从等待表摘除 | 会话协议 §5.4（`SESS-41`、`SESS-42`） | `crates/rutis-bridge/src/session/rpc.rs:2494`（`reply_target`） |
| 取消只作用于一次调用：还在排队的被丢掉、不回复；已经在运行的照常完成并回复；取消不关闭会话 | 会话协议 §7（`SESS-56`、`SESS-57`、`SESS-60`） | `crates/rutis-bridge/src/session/rpc.rs:2693`（`Frame::Cancel`）、`:2038`（`Connection::cancel`） |
| 对已取消调用的迟到答复被丢弃并计数，不当协议错误 | 会话协议 §7（`SESS-57` 第三条）、追踪契约 §3.4 | `reply_target` 的 `cancelled`/`orphans`（`crates/rutis-bridge/src/session/rpc.rs:2494`） |
| 对不存在的调用发 `cancel` 被忽略（取消越过它要取消的回复） | 会话协议 §7（`SESS-58`） | `crates/rutis-bridge/src/session/rpc.rs:2693`；测试 `a_cancel_for_an_unknown_call_is_ignored` |
| 取消不撤销已经提交的引用授予；本地代理释放时接收方照常发 `release` | 会话协议 §7（`SESS-59`）、§6.1（`SESS-47`） | `Import` 的 `Drop`（`crates/rutis-bridge/src/session/rpc.rs:992`）写出 `Frame::Release` |
| 答复里带的值先解码再判断：被丢弃的迟到答复也会释放它带来的引用 | 追踪契约 §3.4；`SESS-44` 的回滚规则 | `crates/rutis-bridge/src/session/rpc.rs:2675`（`Frame::Return` 的 “Decode first”） |
| 会话关闭：等待中的调用以会话错误结算，导出表和导入表清空，未写出的帧随会话消失 | 会话协议 §5.4、§6.1（`SESS-42`、`SESS-51`）、追踪契约 §4“引用守恒” | `crates/rutis-bridge/src/session/rpc.rs:1634`（`Connection::close`） |
| 一端关闭后，另一端从通道结束得知并同样结束 | 追踪契约 §3.0（会话结束先 dispose 再谈别的） | 读线程在通道结束后关闭本端（`crates/rutis-bridge/src/session/rpc.rs` 的读循环与 `closed()`） |
| 关闭之后仍在通道上的帧：读线程读到它时先看 `closed`，整个帧被拒绝，不结算、不授予 | 会话协议 §5.4、§6.1（`SESS-51`） | `receive` 开头 `calls.closed` 的检查（`crates/rutis-bridge/src/session/rpc.rs:2652`）；`close` 先 `closer.close(Close::Done)` 再清表（`:1634-1668`） |
| 读线程的写回交给写线程：`throw`（解码失败、过深）、丢弃迟到答复的 `release`、丢掉排队调用时参数引用的 `release` 都由读线程产生，读线程不等对端；在通道的 `send` 上按背压等待的是写线程 | 会话协议 §10（`SESS-75`、`SESS-76`）；[远程会话设计](../../docs/design-remote-session-2026-10-28.md) §三、§七（#1005） | `Connection::write` 与 `Outbound::push`（`crates/rutis-bridge/src/session/rpc.rs:1825-1840,1387-1404`）、`write_frames`（`:1411-1444`） |
| 取消如何结算：被调方对还在排队或运行中的调用回 `ack` 并不再答复；取消读到之前已经写出的答复先到，就是另一种结算。调用方接受任一种并清表项 | 会话协议 §7（`SESS-57`、`SESS-61`；协议 5，#1099） | `Frame::Cancel`/`Frame::Ack`（`crates/rutis-bridge/src/session/rpc.rs`）、`respond_to`（丢弃被抑制的结果）、`Connection::cancel` |
| 本地放弃的同步调用不发 `cancel`：调用方保留等待表项到回复，读线程读回复时清等待图的边；把表项和边一起丢掉而不发 `cancel`，回复就成了“未知调用”并结束会话 | 会话协议 §5.4（`SESS-41`）；追踪契约 §3.4 | `request_sync`（`crates/rutis-bridge/src/session/rpc.rs:2004-2035`，没有 Drop 守卫）、`CancelOnDrop`（`:1115`，只有异步调用有）、`reply_target` 的 `waits::returned` |
| 被调方丢掉还在排队的调用时，参数里的引用随 `Accepted` 一起丢掉：每个 `Import` 的 `Drop` 向同一个会话写 `release`，写回点在读线程上 | 会话协议 §6.1（`SESS-47`）；§8.2（只让所属调用失败） | `Frame::Cancel` 里 `calls.incoming.remove`（`crates/rutis-bridge/src/session/rpc.rs:2696-2699`）、`Import` 的 `Drop`（`:992-1009`） |
| `PinnedTarget`：已接纳的调用持有目标对象（`Accepted` 里的 `Arc<Object>`），调用方的 `release` 让导出表项消失也不影响它；授予按计数释放，旧的 `release` 不拿走新的授予，超出授予的 `release` 结束会话 | 会话协议 §6.1（`SESS-47`、`SESS-48`） | `export_object` 与 `Accepted`（`crates/rutis-bridge/src/session/rpc.rs:2753`、`crates/rutis-bridge/src/session/rpc/relay.rs:79-87`）、`Frame::Release` 的计数检查（`crates/rutis-bridge/src/session/rpc.rs:2709-2729`） |
| 宿主 `outbound_limit`：写线程队列在达到上限且通道写不进去时结束会话（`the far end is not reading`），而不是继续排队 | 会话协议 §10（`SESS-75`、`SESS-76`） | `Outbound::push`（`crates/rutis-bridge/src/session/rpc.rs:1387-1404`）、`write_frames`（`:1411-1444`） |
| 多跳中继：中继把另一个会话的引用导出成本会话的中继，调用按链改写后转发；取消把被丢弃的转发请求的 `cancel` 发给下一跳；释放沿中继链传递，计数用原引用的计数 | 会话协议 §6.2（`SESS-54`）、§7（`SESS-59`）；`docs/guarantees.md` 的 `bridge/multihop-cancel-and-release`；追踪规则 M1-7.1-1、M1-7.1-2 | `crates/rutis-bridge/src/session/rpc/relay.rs` 的 `relay` 与 `encode_import`（`:19-78,93-175`）、`CancelOnDrop`（`crates/rutis-bridge/src/session/rpc.rs:1115`）、`Awaiting` 的 `_cancel`（`:1060-1064`）、`Import` 的 `Drop` |
| 取消表会排空（#1099）：`calls.cancelled` 从取消起保留到被调方结算它——迟到答复或 `ack`；表项已不在时收到的 `ack` 被忽略；合规的被调方在 `ack` 之后不再答复 | 会话协议 §7（`SESS-57`、`SESS-61`）、G3 | `calls.cancelled`/`running`/`suppressed`、`orphans`、`Connection::cancel` |

## 模型：一个会话（`CallLifecycle.tla`）

- **参与者**：一个会话的两个端点。A 发起调用，B 接收、执行并答复。两个方向各自 FIFO、延迟任意长：A→B 上是 `invoke`、`cancel`、`release`，B→A 上是 `return`、`throw`、`ack`（只在 `CancelTable = "ack"` 时）、`release`（参数引用的归还）。因此答复可以在取消写出时还在路上，取消也可以越过它要取消的答复。
- **A（调用方）**：`AStart` 取下一个号并写 `invoke`；`ADeliverLive` 把答复（`return` 或 `throw`）结算给等待它的调用；`ACancel` 放弃一次调用；`AAbandon` 放弃一次同步调用但**不**写 `cancel`；`ADeliverLate` 把已取消或已放弃调用的迟到答复解码后丢弃；`ADeliverForgotten` 丢弃“有界表已经忘掉”的调用的答复；`ADeliverStray` 处理“调用方已经放不下”的答复——按 `SESS-41` 是 `response for unknown call`，会话结束；`ARelease` 放弃它持有的引用并写 `release`；`AReadArgsRelease` 读回参数引用的 `release`；`AReadAck` 读取消确认；`AClose`/`ALearn` 结束这一端；`AReadAfterClose` 读关闭后仍在通道上的帧。
- **B（被调方）**：`BReadInvoke` 收下调用（还排队，并钉住目标）；`BRejectInvoke` 在读到时拒绝；`BRun` 开始执行；`BReply` 答复并授予一次引用；`BDrop` 按“取消后不答复”的规则丢掉一次运行中被取消的调用；`BReadCancel` 处理取消（排队中的丢掉；已在运行的按规则答复或丢掉；已答复的不动）；在 `ack` 规则下，只要 B 还持有这次调用（排队或运行）就写回一个 `ack`，此后不再答复；`BReleaseArgs` 在调用放下参数时写回 `release`；`BReadRelease` 按计数给出授予，`BReadReleaseBad` 在计数超出授予时结束会话；`BClose`/`BLearn` 结束这一端；`BReadAfterClose` 读关闭后的帧。
- **关闭**：任一端可以关闭。关闭的一端把等待中的调用以错误结算、清空自己持有的引用表、丢掉两个方向上还没写出的帧；另一端通过通道结束（`ALearn`/`BLearn`）做同样的事。`AfterClose = "lost"` 时通道上已有的帧随关闭一起消失；`"read"` 时它们留在通道上，由任一端读出并拒绝（`receive` 先看 `closed`），`AReadAfterClose`/`BReadAfterClose` 在固定的 `Done` 之外不改变任何东西。
- **通道与写线程**（`Bound`、`Cap`）：`Bound = FALSE` 时没有通道上限；`Bound = TRUE` 时每个方向是一条容量 `Cap` 的通道，帧先进写线程的队列（`aout`、`bout`），写线程（`FlushA`、`FlushB`）在通道有空位时把队首写进去。`ablock`/`bblock` 是“读线程正卡在这一帧上写不出去”。
- **读线程写回**（`ReaderWrites`）：`"direct"` 是 #1005 之前的形状——读线程在通道上直接写，满就卡在 `send` 里；`"queued"` 是现在的形状——读线程把帧交给写线程的队列就继续读。三个读线程写回点：A 丢弃迟到答复时的 `release`、B 读时拒绝一次调用写回的 `throw`、B 丢掉排队调用时参数引用的 `release`（`BReleaseArgs`，`c ∈ Args`）。
- **取消后的回复**（`Replies`、`CancelTable`）：`"always"` 时运行中被取消的调用照常答复；`"never"` 时方法结束后丢掉、不写帧。排队中被取消的调用在两种规则下都不回复（`SESS-57`）。`"ack"` 下被调方对还在排队或运行中的调用回 `ack` 并不再答复；取消读到之前已经写出的答复是另一种合法结算。
- **本地放弃的同步调用**（`Sync`、`Abandon`）：`c ∈ Sync` 的调用在 A 记一条等待图的边（`aedge`，ghost），B 收下时记一条 mark（`bmark`，ghost）。`"keep"` 保留等待表项和边直到回复；`"drop"` 把两者一起丢掉、不发 `cancel`。
- **参数引用**（`Args`）：`c ∈ Args` 的调用在 `invoke` 里带一个 A 导出的引用。A 用 `aArgsOut`/`aArgGiven`/`aArgBack` 记它授予、给过、读回的量；B 用 `bArgsGiven` 记这次调用的参数已经放下。`BReleaseArgs` 是那条写回；`argLeaks` 变异让参数被丢掉而不写回。
- **目标钉住**（`RefCalls`）：`c ∈ RefCalls` 的调用点名 B 导出的一个对象，A 必须持有它的一个授予。`BReadInvoke` 收下即钉住，`BRun` 不再查导出表；`bLost` 记“因为没有目标被丢掉”的次数（只有变异里会发生）。`BReadRelease` 按计数减，减到 0 才丢表项：旧的 `release` 不拿走新的授予；`releaseDropsAll` 变异让任何一次 `release` 都丢整个表项，第二次 `release` 就成了 `BReadReleaseBad` 并结束会话。
- **宿主上限**（`OutboundLimit`、`Limit`）：写线程队列一次只放 `Limit` 帧；`OverLimitA`/`OverLimitB` 在队列到顶而通道写不进去时以 `"notReading"` 结束会话。`limitIgnored` 变异让队列越过上限。
- **调用方的取消表**（`aTable`、`CancelTable`）：`"ack"` 是协议 5 的规则——表项留到被调方结算取消（`ack`，此后不再有答复；或取消读到之前已经写出的答复）；`"late"` 是协议 5 之前的形状——表项留到迟到答复或关闭，而排队中被取消的调用没有合规实现会答复，所以会一直涨；`"bounded"` 最多留 `TableCap` 条，满了就丢掉最旧的一条并记进 `forgotten`，对这些调用的答复按 `ADeliverForgotten` 丢弃（`SESS-41` 必须容忍）；`"forget"` 立刻丢掉表项，于是运行中被取消的调用迟到答复就成了“未知调用”。
- **终止**：`Done`（`AllSettled`）只在两端所有调用都已结算、所有通道和写线程队列为空、没有卡住的读线程、参数写回都已决定时可用。这样 TLC 的死锁检查能区分“做完了”和“两个读线程互相等”。

## 模型：中继（`CallRelay.tla`）

- **参与者**：A（调用方）、M（中继，宿主）、C（对象的持有方），两条会话 A–M 与 M–C。C 把对象授予过 M（M 有一个 `Import`），M 把它作为中继导出给 A（M 的导出表项里是一个 body 为该 import 的对象）。初始状态就是这条引用已经建立：`mImport = mRelay = aHeld = cGrants = 1`。
- **调用**：`AStart` 在 A–M 上写 `invoke`（带中继引用，需要 `aHeld > 0`）；`MReadInvoke` 收下；`MForward` 用一个属于 M 的调用号在 M–C 上写出 `call`，并在 `mtable` 里等答复；`CReadCall`/`CRun`/`CReply` 执行并答复；`MDeliverLive` 结算 M 自己的调用，再用 **A 的号**答复 A；`ADeliverLive` 结算 A 的调用。
- **取消**：`ACancel` 在 A–M 上写 `cancel`；`MReadCancel` 丢掉还在排队的进来调用，或者当转发还在等（`mfwd = "waiting"`）时把它标记为 `"cancelled"` 并**向 C 写一个 `cancel`**——这正是 `Awaiting._cancel` 被丢掉时 `CancelOnDrop` 的行为；已答复的调用不动（`SESS-58`）。只要 M 还持有这次调用，它就向 A 回 `ack`（`AReadAck`）并不再答复。`CReadCancel` 丢掉排队中的转发、运行中的按 `Replies` 规则答复或丢掉，记下 `reachedC`，并对它还持有的调用向 M 回 `ack`（`MReadAck`）；`ack` 之后到达的答复是 `MDeliverStray`（`NoStrayReply`）。
- **释放**：`ARelease` 在 A–M 上写 `release`；`MReadRelease` 减 `mRelay`，减到 0 时丢掉导出表项，中继对象随之丢掉，它持有的 import 的 `Drop` 在 M–C 上写出 `release {count: mImport}`，`mImport` 归零；`CReadRelease` 减 `cGrants`。取消不撤销授予：一条 `cancel` 和一条 `release` 可以任意先后。
- **终止**：`Done`（`Quiescent`）要求两条会话的四个方向都空、三方的调用都不在进行。

## 简化

- 规模是有意的有限：多数配置是 1 个调用方、1 个被调方、2 次调用、最多 1 次取消、1 个引用。`call-backpressure-*` 用 3 次调用、1 次取消、每方向容量 1：要把两个读线程同时卡住，取消后答复的那次调用给 A 一个迟到答复，被拒绝的那次给 B 一个 `throw`，第三次的答复用来在被读走后重新填满另一方向——2 次调用时这条路径不可达。加了参数引用（`Args = {"c2"}`）之后 B 的读线程多一个写回点（丢排队调用时参数引用的 `release`），`call-backpressure-*` 的状态数因此变大（见“结果”）。
- 握手不建：模型从两端都已问候、会话可用开始。版本、端点、能力、`hello` 的顺序和 10 秒截止不建（`remote-lease` 覆盖顶替与握手的其它部分）；本文只从第一次调用开始。
- 帧只有 `invoke`、`cancel`、`release`、`ack`（A→B）和 `return`、`throw`、`ack`、`release`（B→A）。`throw` 只建“读线程拒绝读到的调用”这一种来源；方法自己抛出的错误与 `return` 同一条结算路径，不单独建。`call`、`get`、`await`、`events.*`、`rows.*` 不建。
- 调用号只建“递增、不复用”：前缀、规范十进制形式、2^53 上限、`path` 里不得出现自己的号都不建。接收方对“号不增长”的拒绝也没有可达路径——正确的发送方不会重复号，重复号正是 `reuseId` 变异。
- `release` 指向本端没有的引用、`call`/`get`/`await` 指向没有的引用这类协议错误只建“释放计数超出授予”（`BReadReleaseBad`）：正确的一侧不会发出它，`releaseDropsAll` 变异才让它可达。
- 调用链 `path`、`rebase`、会话标签、同步等待的成环判断（`SyncWaitCycle`）不建：那是跨运行时转发的范围（`cross-runtime-sync` 模型）。`Sync` 只用来记一条 `aedge` ghost。中继模型里也不建 `rebase` 与 `path`：中继只把 A 的号换成自己的号，再换回来。
- 值、错误图、`bad-value`、`bad-frame`、16 MiB 上限不建。`Bound`/`Cap` 只建通道容量与写线程的背压；`outbound_limit` 只建它的形状（队列到顶且通道写不进去就结束会话），不建字节数。
- 参数引用只有一条、只在一个调用上：模型里调用最多带一个引用，`Args` 选一次调用。真实代码里一条参数可以带任意多个引用，每个 `Import` 各自写一次 `release`；形状相同。
- 目标钉住不建对象的种类与 `kind` 不匹配的失败，只建“表项在调用运行前消失”这一种；`SESS-48` 的其它错误不建。
- 方法执行时间不建：调用从 `queued` 到 `running` 再到答复，答复可以发生在任意晚的时刻。没有计时、没有调度顺序承诺。
- 关闭的原因不是协议帧：两端各自结束，另一端从通道结束得知。`closeCause` 只为区分“取消导致的关闭”“未知调用导致的关闭”“超出站上限导致的关闭”“计数超出授予导致的关闭”这些错误设计。
- 中继模型不建关闭：`CallLifecycle.tla` 覆盖它。中继在自己的会话关闭时**不**把释放传给 C：`Connection::close` 先清 `imports` 再丢 `exports`（`rpc.rs:1667-1682`），中继最后丢掉时写出的 `release` 落在已经关闭的会话上被吞掉，C 的授予留到它自己的会话结束（由 `SESS-51` 收尾）。这条只按代码读出，不在模型里。
- 内存序、真实调度、帧编码不建；每个动作原子。`Done` 的停机条件是按“没有在途工作”写的。

## 不变式与活性

`CallLifecycle.tla`：

| 名字 | 内容 |
| --- | --- |
| `CancelKeepsSession` | 取消不结束会话：会话的关闭只来自调用方、被调方或通道结束（`closeCause ≠ "cancel"`） |
| `NoUnknownCall` | 一对正常的端点不会以“回复了未知的调用”结束会话（`closeCause ≠ "unknownCall"`）；`SESS-41` 读作“正确的发送方不产生它” |
| `AbandonKeepsSession` | 本地放弃的调用不结束会话（`NoUnknownCall` 在本地放弃这一处的实例） |
| `SettlesAtMostOnce` | 一次调用最多结算一次：答复、取消、本地放弃、会话结束四者不会让同一个调用结算两次 |
| `LateReplyDropped` | 迟到答复被丢弃：已取消或已放弃的调用保持当时的结果；每次调用最多丢弃一个答复 |
| `OwnReply` | 结算一个调用的答复就是这次调用自己的答复 |
| `NoIdReuse` | 一个调用号只属于一次调用 |
| `NoOverRelease` | 调用方只归还它被授予的量：一份授予被释放、丢弃或清空一次，不会两次 |
| `GrantsConserved` | 静止时 B 授予的每一单位都还在 A 手里或已经归还 |
| `ClosedClearsTables` | 关闭的一端不再持有引用 |
| `FailUnfinishedOnClose` | 关闭后两端都不留未完成的调用 |
| `LateFramesInert` | 关闭后读到一帧不改变任何东西：没有调用在关闭之后被再结算一次，也没有引用被收进一个已经关闭的会话 |
| `PinnedTarget` | 已接纳的调用不因为没有目标被丢掉；旧的 `release` 不拿走新的授予（会话不会以“超出授予的释放”结束） |
| `ArgRefsReleased` | 静止时 A 的导出表里不再留有参数给出的引用 |
| `ArgRefsCounted` | 参数引用的授予被记一次：给出的、读回的和仍在外面的一致 |
| `BacklogBounded` | 写线程队列不超过宿主给的上限 |
| `CancelledTableBounded` | 有界表最多留 `TableCap` 条（#1099 的“有界表”方案） |
| `CancelledEntriesDrain` | 静止时 A 的取消表里没有只等迟到答复来清理的表项（#626 的 `BoundedTables`） |
| `AbandonedCallsClean` | 静止时本地放弃的调用不留下等待图的边 |
| `WaitingSettles` | 活性（只在没有关闭的配置里检查）：A 等待的调用最终被答复、被取消或被放弃 |

`CallRelay.tla`：

| 名字 | 内容 |
| --- | --- |
| `CancelReaches` | 中继还在转发时收到的取消沿两跳都到达 C：M 转发之后、两条会话静下来时 C 已经读到它（已答复的调用不需要取消） |
| `RelayReleasesImport` | 静止时 A 不再持有中继的授予，就意味着中继已经丢掉 import、C 也没有授予了（M1-7.1-2） |
| `GrantsConserved` | 授予计数沿中继链传递且不被重复计：A 读回的不少于 M 收到的，M 收到的不少于 C 读回的 |
| `SettlesAtMostOnce` | A 侧一次调用最多结算一次 |
| `LateReplyDropped` | 迟到答复在两跳上都最多被丢弃一次 |
| `RelayCancelledEntriesDrain` | 静止时中继自己的取消表为空（#1099：这条表项也是每跳一份） |
| `WaitingSettles` | 活性：A 等待的调用最终有结果 |

## 结果

TLC 2.19（tla2tools v1.7.4），Java 21.0.12（Temurin），32 核，`-workers auto`（小配置用 4）。`通过` 指穷举完成、所列性质全部成立；被违反的配置找到第一个反例就停，死锁配置找到第一个死锁就停，状态数取决于搜索顺序，每次可能不同。全部 37 个配置在 2026-11-18 用 `run.sh` 跑过，整个目录约 70 秒（随机器负载）。

| 配置 | 内容 | 结果 | 不同状态数 | 时间 |
| --- | --- | --- | --- | --- |
| `call-healthy` | 2 次调用，最多 1 次取消，两端都可以关闭；全部安全不变式（含 `NoUnknownCall`、`LateFramesInert`、`PinnedTarget`、`ArgRefs*`、`BacklogBounded`） | 通过 | 1,155 | 1 s |
| `call-progress` | 不允许关闭，只检查活性（`WaitingSettles`） | 通过 | 409 | 1 s |
| `call-backpressure-before` | 3 次调用、1 次取消、每方向容量 1、一个参数引用；读线程直接写回（#1005 之前），死锁检查开着 | **死锁** | 约 23,000（找到死锁即停） | 1–2 s |
| `call-backpressure-after` | 同一组调用、取消、容量与参数引用；读线程把写回交给写线程的队列 | 通过（无死锁） | 153,550 | 10–14 s |
| `call-backpressure-limit` | 同一组调用与 #1005 之前的形状，加上宿主 `outbound_limit = 2` | 通过（不卡死：队列到顶、通道写不进去时会话以 `notReading` 结束；`BacklogBounded` 成立） | 55,276 | 2 s |
| `call-cancel-reply` | 取消后回复的实现：运行中被取消的调用照常答复，迟到答复丢弃、引用归还；不允许关闭 | 通过 | 409 | 1 s |
| `call-cancel-no-reply` | 取消后不回复的实现：运行中被取消的调用由 `BDrop` 丢掉，不写帧 | 通过 | 437 | 1 s |
| `call-gap-cancel-table` | 协议 5 之前的调用方取消表（只由迟到答复清理）+ 排队中被取消的调用 | 违反 `CancelledEntriesDrain` | 73 | 0–1 s |
| `call-gap-ack` | 协议 5：被调方对它结算的取消回 `ack` 并不再答复；越过取消的答复先清表项，其后的 `ack` 被忽略 | 通过（`CancelledEntriesDrain`、`NoUnknownCall`） | 533 | 0–1 s |
| `call-mut-ack-then-reply` | 被调方回了 `ack` 之后仍然答复 | 违反 `NoUnknownCall` | 268 | 0–1 s |
| `call-gap-unknown-late` | 调用方取消时立刻清表项，被调方照常答复运行中被取消的调用 | 违反 `NoUnknownCall` | 约 300（找到反例即停） | 0–1 s |
| `call-gap-bounded` | 有界表：最多 `TableCap = 1` 条，满了丢最旧的；被忘掉的调用的答复按迟到处理 | 通过（`CancelledTableBounded`、`NoUnknownCall`） | 60,358 | 2–3 s |
| `call-sync-abandon` | 同步调用在本地放弃、不发 `cancel`，保留表项到答复 | 通过 | 615 | 1 s |
| `call-after-close` | 关闭后仍在通道上的帧由任一端读出并拒绝 | 通过（`LateFramesInert`） | 2,669 | 1–2 s |
| `call-pinned-target` | 调用点名一个对象，调用方在方法开始前释放授予；旧的 `release` 不拿走新的授予 | 通过（`PinnedTarget`） | 209 | 1–2 s |
| `call-arg-release` | 排队中的调用被取消，参数里的引用由读线程写回 `release` | 通过（`ArgRefsReleased`、`ArgRefsCounted`）+ `WaitingSettles` | 847 | 1 s |
| `call-mut-cancel-closes` | 取消时关闭会话 | 违反 `CancelKeepsSession` | 11 | 1 s |
| `call-mut-late-reply-twice` | 迟到答复被当成正常答复再结算一次 | 违反 `SettlesAtMostOnce` | 约 500（找到反例即停） | 1 s |
| `call-mut-late-reply-keeps-grant` | 丢弃迟到答复但不释放它带的引用 | 违反 `GrantsConserved` | 381 | 1–2 s |
| `call-mut-close-leaks` | 关闭时保留导出表 | 违反 `ClosedClearsTables` | 约 350（找到反例即停） | 0–1 s |
| `call-mut-reuse-id` | 调用号可以被新调用复用 | 违反 `NoIdReuse` | 11 | 1–2 s |
| `call-mut-reuse-id-settles` | 同一个变异的后半段：复用的号让答复结算了另一个调用 | 违反 `OwnReply` | 约 550–800（找到反例即停） | 1–2 s |
| `call-mut-abandon-drops` | 本地放弃时把等待表项和边一起丢掉，不发 `cancel` | 违反 `AbandonKeepsSession` | 约 400–550（找到反例即停） | 1 s |
| `call-mut-abandon-leaks` | 迟到答复被吸收，但等待图的边没有清掉 | 违反 `AbandonedCallsClean` | 615 | 1 s |
| `call-mut-late-frame-acts` | 关闭后读到的答复仍然被结算 | 违反 `SettlesAtMostOnce` | 约 700–800（找到反例即停） | 1 s |
| `call-mut-no-pin` | 方法开始时才去导出表找目标 | 违反 `PinnedTarget` | 71 | 0–2 s |
| `call-mut-release-drops-all` | 任何一次 `release` 都丢掉整个表项 | 违反 `PinnedTarget` | 447 | 1–2 s |
| `call-mut-arg-leak` | 排队调用被丢掉时参数引用不写回 | 违反 `ArgRefsReleased` | 约 550–650（找到反例即停） | 0–1 s |
| `call-mut-limit-ignored` | 写线程队列无视宿主上限 | 违反 `BacklogBounded` | 约 130（找到反例即停） | 0–1 s |
| `call-mut-forget-strict` | 有界表忘掉了表项，但调用方仍按 `SESS-41` 严格处理 | 违反 `NoUnknownCall` | 约 1,400–2,100（找到反例即停） | 1 s |
| `relay-healthy` | 中继上的调用、每跳用 `ack` 结算的取消与释放 | 通过（`RelayCancelledEntriesDrain`、`NoStrayReply`） | 10,180 | 1–2 s |
| `relay-no-reply` | 同一个模型，验证不回复的被调方路径 | 通过 | 10,180 | 1–2 s |
| `relay-progress` | 中继上的活性 | 通过 | 10,756 | 2–3 s |
| `relay-gap-cancel-table` | 两跳都没有确认：转发出去、被 C 丢掉且不回复的调用在每跳留下表项 | 违反 `RelayCancelledEntriesDrain` | 99 | 0–1 s |
| `relay-mut-ack-then-reply` | C 回了 M 的 `ack` 之后仍然答复 | 违反 `NoStrayReply` | 1,649 | 0–1 s |
| `relay-mut-cancel-stops` | 中继丢掉转发请求但不把取消传给 C | 违反 `CancelReaches` | 约 500–1,200（找到反例即停） | 0–1 s |
| `relay-mut-release-leaks` | 中继丢掉导出表项但不归还 import | 违反 `RelayReleasesImport` | 约 300–400（找到反例即停） | 1–2 s |

## 变异与反例

每个被检查的性质都配一个故意引入的错误，TLC 必须找到反例（`expected.txt` 的 `violated:<名字>`）：

| 配置 | 变异 | 违反 | 反例 |
| --- | --- | --- | --- |
| `call-mut-cancel-closes` | `ACancel` 直接把 A 的会话关掉 | `CancelKeepsSession` | 一次取消让 `aclosed = TRUE`、`closeCause = "cancel"` |
| `call-mut-late-reply-twice` | `ADeliverLate` 把已取消调用的答复当成仍然在等的结果结算 | `SettlesAtMostOnce` | 取消给了结果、答复又给了一个：`settles["c1"] = 2` |
| `call-mut-close-leaks` | `CloseB` 不清 `bgrants` | `ClosedClearsTables` | 关闭后 `bgrants = 1` 留在一个已关闭的会话里 |
| `call-mut-reuse-id` | `AStart` 不推进 `anext` | `NoIdReuse` | `aid = [c1 \|-> 1, c2 \|-> 1]` |
| `call-mut-reuse-id-settles` | 同一个变异，看它的后果 | `OwnReply` | `settledBy["c1"] = "c2"` |
| `call-mut-late-reply-keeps-grant` | 丢弃迟到答复但不释放它带的引用 | `GrantsConserved` | `aheld = 0`、`aReleased = 0`、`granted = 1`，静止时 0 ≠ 1 |
| `call-mut-abandon-drops` | `AAbandon` 在 `"drop"` 下丢掉表项和边、不发 `cancel` | `AbandonKeepsSession` | 答复成了未知调用：`closeCause = "unknownCall"` |
| `call-mut-abandon-leaks` | 吸收迟到答复但不清 `aedge` | `AbandonedCallsClean` | 静止时 `aedge["c1"] = TRUE` 仍在 |
| `call-mut-late-frame-acts` | 关闭后读到的帧照样处理 | `SettlesAtMostOnce` | A 发起调用、B 答复、A 关闭；读线程读到那帧答复，`settles["c1"] = 2`、`aheld = 1` 留在一个 `aclosed = TRUE` 的会话里 |
| `call-mut-no-pin` | `BRun` 在方法开始时查导出表里的目标 | `PinnedTarget` | A 从调用 2 拿到授予、发起调用 1（`aid = 2`）、释放授予；B 开始调用 1 时 `bgrants = 0`，丢掉它并写 `throw c1`，`bLost = 1` |
| `call-mut-release-drops-all` | `BReadRelease` 任何一次都清空 `bgrants` | `PinnedTarget` | 两次授予过后 A 释放一次，表项消失；第二次释放超出授予，会话以 `closeCause = "badRelease"` 结束 |
| `call-mut-arg-leak` | `BReleaseArgs` 放下参数但不写 `release` | `ArgRefsReleased` | 排队中被取消的调用之后，静止时 `aArgsOut = 1`、`aArgBack = 0`、`aArgGiven = 1`：A 的导出表里留着一个没人会归还的授予 |
| `call-mut-limit-ignored` | `RoomToB`/`RoomToA` 不检查上限 | `BacklogBounded` | `Limit = 2` 而 `aout` 里有三帧 |
| `call-mut-forget-strict` | 有界表忘掉表项后，调用方仍把它的答复当未知调用 | `NoUnknownCall` | 有界表丢掉了调用 1（`forgotten = {"c1"}`），B 答复它，A 按 `SESS-41` 结束会话：`closeCause = "unknownCall"` |
| `call-mut-ack-then-reply` | 被调方回了 `ack` 之后仍然答复 | `NoUnknownCall` | `ack` 已清掉 A 的表项，答复随后到达：`closeCause = "unknownCall"` |
| `relay-mut-cancel-stops` | `MReadCancel` 丢掉转发请求但不写 `cancel` | `CancelReaches` | A 取消调用 1，M 在转发中收到并丢掉它，`wasOutstanding["c1"] = TRUE`、`reachedC["c1"] = FALSE`，C 的调用还停在 `"queued"`，两条会话里没有在途的帧 |
| `relay-mut-release-leaks` | `MReadRelease` 丢掉中继但不归还 import | `RelayReleasesImport` | `aHeld = 0`、`mRelay = 0`、`mImport = 1`、`cGrants = 1`：C 的授予留在一个已经没有持有者的引用上 |
| `relay-mut-ack-then-reply` | C 回了 M 的 `ack` 之后仍然答复转发的调用 | `NoStrayReply` | `mStray = TRUE`：答复到达时表项已不在，M 也不会转发给 A |

两个复用号的配置、两个放弃的配置、`PinnedTarget` 的两个变异都是同一条规则的两种结论：先是不变式本身被破坏，然后是它带来的错误结算或错误结束。

## 取消表：已决定（协议 5，#1099）

协议 5 把取消结算放在被调方：还在排队的调用被丢掉，运行中的继续跑，被调方回 `ack` 且不再答复。取消读到之前已经写出的答复仍然合法，会先到并清掉调用方的表项；其后的 `ack` 被忽略。合规的被调方不会在 `ack` 之后答复，因此表会排空，合法的迟到答复也不会结束会话。配置：

| 配置 | 规则 | 取消表 | 代价 | 结果 |
| --- | --- | --- | --- | --- |
| `call-gap-cancel-table` | 协议 5 之前的形状：只由迟到答复清理 | 无界增长 | 只涨内存 | 违反 `CancelledEntriesDrain` |
| `call-gap-ack` | 协议 5：被调方对它结算的取消回 `ack` 并不再答复；调用方收到 `ack` 清表项，表项已被交叉答复清掉时忽略该 `ack` | 静止时为空 | 新帧 `ack` 与协议版本 5；所有实现一起升级 | 通过（`CancelledEntriesDrain`、`NoUnknownCall`） |
| `call-mut-ack-then-reply` | 破坏 `ack` 规则：被调方回了 `ack` 仍然答复 | 答复到达时表项已不在 | 一次合规交换就结束会话 | 违反 `NoUnknownCall` |
| `call-gap-unknown-late` | 调用方取消时立刻清表项，迟到答复按未知调用处理 | 静止时为空 | 一次合法的交换就可能以 `response for unknown call` 结束会话（`closeCause = "unknownCall"`） | 违反 `NoUnknownCall` |
| `call-gap-bounded` | 有界表：最多 `TableCap` 条，满了丢最旧的；被忘掉的调用的答复丢弃 | 不超过 `TableCap` | `SESS-41` 要放宽成“被忘掉的调用的答复可以丢弃”，不再算协议错误；实现需要一个水位或环形记录 | 通过（`CancelledTableBounded`、`NoUnknownCall`），未采用 |

有界表是退路；取消确认让表在静止时为空且不必放宽 `SESS-41`，所以 #1099 选了它。`relay-gap-cancel-table` 说明这条表项**每跳一份**：A 的表里一条，中继替它转发、又取消掉的那次调用在中继的表里还有一条。协议 5 把规则写进两跳：`MReadCancel` 向 A 回 `ack` 并取消自己转发出去的调用，C 再向 M 回 `ack`；`relay-mut-ack-then-reply` 是第二跳“`ack` 之后还答复”的变异。

## 背压与宿主上限

两个背压配置的其余写路径相同：调用方和被调方的帧都先进入写线程的队列，由写线程写进通道；差别只在读线程的写回——`"direct"` 让它在通道上直接写、满了就停下读，`"queued"` 让它交给写线程。所以死锁只能归因于读线程写回这一处，而不是调用方或被调方的写。

| 配置 | 建模的行为 | 结果 |
| --- | --- | --- |
| `call-backpressure-before` | #1005 之前：读线程的写回直接写通道，通道满就卡住读线程 | TLC 找到死锁：`ablock = <<"release", 1>>`（A 丢弃迟到答复时的 `release`）、`bblock = <<"release", 1>>`（B 丢掉排队调用时参数引用的 `release`），`toB = <<"cancel", "c1">>`、`toA = <<"throw", "c2", 2>>`，两个通道都是满的，写线程队列里还有一帧没写进去。要腾出一个方向只能让对面的读线程读，而它正卡在自己的写回上 |
| `call-backpressure-after` | 现在：读线程把写回交给写线程的队列 | 无死锁，`WaitingSettles` 成立；实现在 #1005 已经是这个形状，**不需要改代码** |
| `call-backpressure-limit` | 同一个 #1005 之前的形状，加宿主 `outbound_limit = 2` | 不卡死：队列到顶、通道写不进去时写线程队列不再接受，会话以 `notReading` 结束（`SESS-75`）。这是宿主给上限之后对“对端不读”的处理：结束会话，而不是让本进程无限期地替它留帧 |
| `call-mut-limit-ignored` | 队列无视上限 | 违反 `BacklogBounded`：上限是唯一让这种会话结束的东西，去掉它就没有边界（`SESS-76` 的“没有上限”就是这种形状） |

## 结论

- **读线程写回不会卡死，前提是读线程不直接写。** #1005 之前（`call-backpressure-before`）用 3 次调用、1 次取消、每方向容量 1 就能走到死锁：两个读线程各卡在一帧写回上，两个通道都是满的，能腾出通道的读动作都在对方卡住的读线程手里。现在的形状（`call-backpressure-after`，读线程把帧交给写线程的队列）没有死锁，所有调用结算；实现已经是这个形状，本模型不改它，只把结论和回归配置留下来。三个写回点形状相同：丢弃迟到答复的 `release`、拒绝调用的 `throw`、丢掉排队调用时参数引用的 `release`。
- **宿主的上限把这种会话从“卡死”变成“结束”。** `call-backpressure-limit` 在对端不读、队列到顶时以 `notReading` 结束会话（`SESS-75`），`BacklogBounded` 保证队列不超过上限；`call-mut-limit-ignored` 说明去掉上限就没有边界。默认 `outbound_limit` 是 `None`（`SESS-76`）：不设上限的宿主要自己承担这个风险。
- **取消恰好结算一次，调用方的表会排空。** `call-gap-ack` 里被调方对排队或运行中的调用回 `ack` 并不再答复，表项随 `ack` 清掉；越过取消的答复仍被丢弃并释放引用，其后的 `ack` 不改变任何东西。`SESS-61` 现在写的就是这条规则（协议 5，#1099）。`call-cancel-reply` 与 `call-cancel-no-reply` 仍保留“取消前已写出答复”的两种形状：都至多结算一次、不漏引用。
- **本地放弃的同步调用不留无法清理的边，前提是调用方保留调用记录直到答复。** 现在的 `request_sync` 没有 Drop 守卫，同步调用被放弃时不发 `cancel`；等待表项留在 `calls.waiting`，读线程读到答复时 `reply_target` 摘掉它并 `waits::returned` 清边。把表项和边一起丢掉而不发 `cancel` 会让答复成为 `response for unknown call` 并结束会话，所以规则是：**要么保留到答复，要么发 `cancel`**。
- **参数里的引用不会漏，前提是调用放下参数时写回。** `call-arg-release` 里排队中被取消的调用由读线程丢掉，参数的 `Import` 写回 `release`，静止时 A 的导出表为空；`call-mut-arg-leak` 说明不写回就留下一个没人会归还的授予（`aArgsOut = 1` 到会话结束）。这条写回与 `throw`、丢弃迟到答复的 `release` 是同一条路径，因此背压的结论同样适用于它。
- **`PinnedTarget` 成立，前提是调用持有目标而不是每次去查表。** 已接纳的调用在 `Accepted` 里带着对象，调用方的 `release` 让导出表项消失也不影响它；授予按计数释放，旧的 `release` 不拿走新的授予。`call-mut-no-pin` 说明运行前才查表会让一次释放毁掉一个已经接纳的调用，`call-mut-release-drops-all` 说明按“有释放就清表”处理会让第二次释放把会话结束掉（`release of unknown reference` / `invalid reference release count`）。
- **关闭之后读到一帧不改变任何东西，前提是读线程先看 `closed`。** `call-after-close` 通过 `LateFramesInert`：关闭后仍在通道上的帧被读出并拒绝，没有调用被再结算一次，也没有引用被收进一个已关闭的会话。`call-mut-late-frame-acts` 说明少了这道检查就会把一个已经被关闭结算掉的调用再结算一次，并让一份引用留在一个关闭了的会话里（`SESS-51` 的“引用随会话消失”要靠它才成立）。
- **多跳中继的取消与释放逐跳传递，前提是中继在丢掉转发请求时发 `cancel`、在丢掉中继时归还 import。** `relay-healthy` 与 `relay-no-reply` 通过 `CancelReaches`、`RelayReleasesImport`、`GrantsConserved`；`relay-mut-cancel-stops` 说明中继自己丢掉等待而不告诉 C，取消就停在第二跳；`relay-mut-release-leaks` 说明中继不归还 import 时 C 的授予留在一个已经没人持有的引用上。计数沿链传递不需要新帧：释放写的是 import 的计数。
- **#1099 选了取消确认，而且每一跳都要。** 协议 5 之前的表无界增长（`call-gap-cancel-table`）；`ack` 只有在同时保证“`ack` 之后不再答复”时才能清空它（`call-mut-ack-then-reply` 是反例）；有界表能封顶，代价是放宽 `SESS-41`，因此只作退路。`relay-gap-cancel-table` 与 `relay-mut-ack-then-reply` 说明规则按跳生效；协议 5 与七种实现都带上了它。
- 按协议 §5–§7 的其余规则——号只增不复用、一次调用只结算一次、取消只丢调用不丢会话、迟到答复丢弃、关闭时失败未完成调用并清空引用表——在穷举范围内仍然成立；`call-healthy` 的状态数（1,155）与扩展前一致，说明新开关关掉时模型的行为没有变。

## 未覆盖

- **中继模型里的关闭**。`CallRelay.tla` 不建关闭。中继在自己的会话关闭时**不**把释放传给 C：`Connection::close` 先 `imports.clear()` 再 `drop(exports)`（`rpc.rs:1667-1682`），中继最后丢掉时写出的 `release` 落在已经关闭的会话上被 `write_bytes_locked` 的 `closed` 检查吞掉，C 的授予留到它自己的会话结束（`SESS-51` 收尾）。这条只按代码读出，没有模型。
- **关闭后另一侧继续读帧**。模型把“关闭后读到一帧”统一成“拒绝”：`AfterClose = "read"` 时两个方向的帧都由任一端读出并拒绝。真实实现里对端在自己的会话结束之前仍可能执行已经读到的 `invoke`（它的 `receive` 只看自己的 `closed`），它的答复落在已关闭的通道上；这条交错不在模型里，`LateFramesInert` 也不声称覆盖它。
- **`outbound_limit` 的字节数**。模型按帧数算（`Limit` 是帧数），实现按字节；两者只在“队列非空且再加一帧就超”这一点上同形。
- **中继的链、`rebase`、`path`、等待图与同步转发**。中继模型只有一条调用、一个对象引用、一条两跳链；`path` 改写、同步转发的线程与成环判断在 `cross-runtime-sync` 的范围。
- **中继的引用身份与 `home: true`**。同一引用多次转交复用同一个中继（M1-7.1-3）、中继回家（M1-7.1-4）、中继的中继（M1-7.1-5）都不建。
- **值编码、错误图、`bad-value`/`bad-frame`、16 MiB 与嵌套上限**，以及等待图的成环判断、握手与鉴权、真实调度、内存序与计时。
- 本目录只是 #613 调用生命周期的一块，不能当作完整调用协议的证据。

## 怎么运行

需要 Java 11 以上和 `tla2tools.jar`（[v1.7.4](https://github.com/tlaplus/tlaplus/releases/tag/v1.7.4)，sha256 `936a262061c914694dfd669a543be24573c45d5aa0ff20a8b96b23d01e050e88`）。jar 不要提交进仓库。

```sh
curl -sSLo /tmp/tla2tools.jar https://github.com/tlaplus/tlaplus/releases/download/v1.7.4/tla2tools.jar
TLA2TOOLS=/tmp/tla2tools.jar specs/call-lifecycle/run.sh                    # 全部配置
TLA2TOOLS=/tmp/tla2tools.jar specs/call-lifecycle/run.sh call-healthy       # 单个配置
TLC_WORKERS=4 TLA2TOOLS=/tmp/tla2tools.jar specs/call-lifecycle/run.sh      # 限定 TLC 的线程数
```

`run.sh` 按名字选模块：`call-*.cfg` 跑 `CallLifecycle.tla`，`relay-*.cfg` 跑 `CallRelay.tla`。它把每个配置的结果和 `expected.txt` 比较，全部符合时退出码为 0；结果是 `ok`、`deadlock` 或 `violated:<名字>`（时序性质违反时是 `violated:liveness`）。日志和反例写在 `TLC_OUT`（默认一个临时目录）。CI 的 `specs` 任务矩阵包含 `call-lifecycle`（见 [CI](../../docs/ci.md) §4）。

## 维护

会话协议 §5、§6、§7、§10 修改时，或 `crates/rutis-bridge/src/session/rpc.rs` 的 `Calls::admit`、`reply_target`、`Frame::Cancel`/`Frame::Ack`/`Frame::Release`/`Frame::Return` 的处理、`Connection::cancel`、`Connection::close`、`Connection::write`/`Outbound::push`/`write_frames`（读线程写回、写线程与 `outbound_limit`）、`request_sync` 与 `CancelOnDrop`（本地放弃）、`Import` 的 `Drop`（参数引用与中继的释放）、`rpc/relay.rs` 的 `relay`/`encode_import`（多跳转发）改变时，或运行时一侧（`node/rutis-runtime/src/session.mjs` 等）的取消、回复与引用释放行为改变时，同步修改这两个模块和 `expected.txt`（规范 Q11.2.3）。
