# 远端租约：接管、重连与起停（TLA+）

[English](README.en.md)

用 TLC 穷举远程运行时的租约、新连接接管、link 重连与停止、应用托管停止后再开始（含多次起停与接管途中的停止）的交错。对应 issue #210 与 #615 的并入项，质量规范 Q11.2、Q6.13.3，现状 §7.2（风险 E4、E5）。两个模型：

- `RemoteLease.tla`：监听的远端运行时，和拨号连它的控制方 link；
- `AccessPoint.tla`：远端拨入 `rutis-host/listen` 的一行：握手并行、每个 id 一个 session、同 id 的新连接顶替旧的，以及拨出方的重连与起停。

**对账（2026-11-10）。** 本模型现在覆盖 [#615](https://github.com/arcships/rutis/issues/615) 的以下各项：接管顺序里的鉴权一步（`Auth = "step"`）、旧租约清理的截止（`Cleanup = "deadline"`）、应用托管停止后再开始（`MaxRounds` / `MaxStops`，性质 `RestartIsFresh`、`FinalStopStaysDown`、`RestartRecovers`，含 4 轮、每轮一次停止（3 次再开始）和接管进行中的停止）、故障之后的活性（`MaxFaults`，`lease-faults-live` / `access-faults-live`）、各运行时接管时新连接心跳从回应 hello 起算（Kotlin #1102；Go、Swift、Node、Bun #1182；性质 `NoHeartbeatBeforeHello`，变异 `heartbeatFromAccept`）、静态监听 link 被吊销后的注册、4003 与握手途中的吊销（`access-static-revoke`、`access-mut-revoke-after-handshake`）、`RutisApp` 多次起停与最终停止后的再次开始/拨号（`lease-app-restart-3`、`access-app-restart`、`lease-mut-start-after-final-stop`、`access-mut-final-stop-dials`），以及停止之后才到的 `.accepted` 连接（`lease-mut-late-accept`）。对应表补了 Swift / Kotlin / 应用托管 / Cordis 节点连接。仍未覆盖：Cordis 节点连接的代码（`node/rutis-runtime/src/bridge/link.mjs`）不在本仓库，只能按同一套拨出规则核对；计时（退避、心跳、强杀期限）不在模型里，见"简化"。Node / Bun 的强杀时间已改为 `RUTIS_SHUTDOWN_TIMEOUT + 2 s`（本分支 f57b4138，`serve.mjs` / `remote.ts`）；Kotlin、Go、Swift、Node、Bun 的心跳都从回应 hello 起算（见下文"简化"）。

## 对应的设计和代码

| 模型部分 | 设计 | 实现 |
| --- | --- | --- |
| 控制方（`rutis-host/dial` 行）：拨号、退避重连、按失败类型等待、停止 | [远程插件](../../docs/design-remote-plugins-2026-10-03.md) §3.2；[拨出稿](../../docs/design-remote-runtime-dial-2026-10-14.md) §六；[远程会话](../../docs/design-remote-session-2026-10-28.md) §4.1 | `crates/rutis-host/src/connect/dial.rs`：拨号、握手、装上会话、等它卸载、退避 |
| 运行时的租约与接管顺序（鉴权 → 关闭旧会话 → 清理旧租约 → 回应新 hello）；清理的截止；新连接心跳的起算点 | 远程插件 §4.4；[远程会话](../../docs/design-remote-session-2026-10-28.md) §4.3 | `python/rutis/rutis/__main__.py` `serve`；`go/rutis/internal/runtime/listen.go` `listen`；`node/rutis-runtime/src/serve.mjs` `serve`；`bun/rutis-bun/src/remote.ts`；Swift `swift/rutis/Sources/Rutis/Remote/Listen.swift`、`swift/rutis/Sources/Rutis/Remote/App.swift`；Kotlin `kotlin/rutis/rutis/src/shared/kotlin/arcships/rutis/internal/remote/Listen.kt`、`kotlin/rutis/rutis/src/shared/kotlin/arcships/rutis/internal/remote/Frames.kt` |
| 应用托管：停止后再开始（多次），stop 期间的 start 在清理后生效；最后一轮之后不再开始、不再绑定；拨号次数与轮次从头数 | [Swift SDK](../../docs/design-swift-sdk-2026-10-12.md) §七；[Android SDK](../../docs/design-android-runtime-2026-10-12.md) §五 | `RutisApp.start` / `stop`：Kotlin `kotlin/rutis/rutis/src/shared/kotlin/arcships/rutis/RutisApp.kt`、Swift `swift/rutis/Sources/Rutis/Remote/App.swift`；本分支 f57b4138 修了 `run(previous)` 的线程交错 |
| 拨号的应用侧（`connect`）：起停、每轮拨号从头数、停止后不再拨 | 拨出稿 §六；Android SDK §五 | Kotlin `kotlin/rutis/rutis/src/shared/kotlin/arcships/rutis/internal/remote/Dialing.kt`、`kotlin/rutis/rutis/src/shared/kotlin/arcships/rutis/RutisApp.kt`；Swift `swift/rutis/Sources/Rutis/Remote/Dial.swift`、`swift/rutis/Sources/Rutis/Remote/App.swift` |
| listen：握手并行，同 id 顶替（旧 session 卸载完 → 旧 Channel 以 4002 关闭 → 装新 session）；已离开的对端的握手结果不装 | 远程会话 §4.2 | `crates/rutis-host/src/connect/listen.rs`：`Listening::run`、`Replaceable` |
| listen 的静态 link 被吊销：保留注册、`Waiting { AuthRejected }`、之后同 id 的连接在 hello 后以 4003 关闭，握手途中被吊销的也一样 | 拨出稿 §五、§六（规则 DIA-5-7 / 5-10 / 5-11 / 5-13） | 0.19 删除吊销钩子（#273），没有实现可对；断开现有连接靠停掉那一行。吊销发生在握手途中的这一半按 DIA-5-10 在模型里 |
| 拨出方的重连规则 | 拨出稿 §六 | `python/rutis/rutis/dialing.py` `keep_dialing`；`go/rutis/internal/runtime/dial.go`；`node/rutis-runtime/src/dialing.mjs`；`bun/rutis-bun/src/dialing.ts`；Swift `swift/rutis/Sources/Rutis/Remote/Dial.swift`；Kotlin `kotlin/rutis/rutis/src/shared/kotlin/arcships/rutis/internal/remote/Dialing.kt` |
| Cordis 节点连接：退避（500 ms 起、30 s 上限、稳定 60 s 重置、被拒 30 s）与接管 | 远程插件 §4.4、§5；拨出稿 §六 | `node/rutis-runtime/src/bridge/link.mjs`（Cordis 仓库的代码，#615 引用；**不在本仓库**，没有可核对的实现。规则与 `node/rutis-runtime/src/dialing.mjs` 相同，模型的控制方是它的规格） |

**实现范围（静态监听 link）。** DIA-5-7、DIA-5-10、DIA-5-11、DIA-5-13 的吊销钩子在 0.19 删除（#273），没有当前实现可对：`access-static-revoke` 与 `access-mut-revoke-after-handshake` 检查的是规格规则（保留注册、之后同 id 的连接在 hello 后以 4003 关闭、握手途中或等待接管时被吊销的连接不装上——DIA-5-10 的监听侧）。断开现有连接现在靠停掉那一行。

## 模型

两个模型共同的约定：每条连接每个方向按顺序送达，延迟任意长；一方关闭连接后不再读它，发给它的帧丢弃（只有需要读它们的变异保留）。控制方 / 拨出方按轮计：每轮有 `MaxConns` 次付费连接；被故障断开的连接，它的重拨不占额度（`free`）。瞬时故障（拨号失败、断线）总数为 `MaxFaults`；应用起停的轮数为 `MaxRounds`、停止次数为 `MaxStops`。这些是模型的界，不是实现的行为。

### `RemoteLease.tla`

- **参与者**：控制方 `c1`、`c2`（rutis 的 `rutis-host/dial` 行），一个监听的远端运行时。连接 `<<c, n>>` 是 `c` 第 n 次拨号。
- **控制方**（`connect/dial.rs`）：拨号成功后升级完成，运行时的监听器把连接排进队列；等运行时的 hello（握手），收到后装上会话、状态 `Ready`；之后 loader 经会话装载行（`load` 帧）。会话结束（被接管 4002，或运行时结束会话）按可重试处理，退避后重连。停止（dispose）中止任务：关闭会话、取消重试。环境：拨号失败、握手超时或断线（`Lose`）、停止。
- **故障之后的活性**：拨号失败和断线合计不超过 `MaxFaults`；失败的拨号不占连接额度，被断开的连接的下一拨免费。故障用完之后，最后建立的连接不会被断开，或它还剩下一次免费的重拨，所以 `lease-faults-live` 能检查 `EndsHeld`（不用对称性；TLC 带对称时检查活性可能漏报）。
- **运行时**（Python `serve`、Swift `RutisApp` 的监听循环、Kotlin `Listening` 等）：一次只服务一个控制方。取下一个连接时若有当前会话，按 §4.4：① 鉴权（`Auth = "step"` 时是单独一步：连接先排队，运行时接受后才进入接管队列；拒绝只关闭这条连接，不动当前会话）；② 关闭旧会话的通道（4002），不再读它的帧；③ 清理旧租约（行全部卸载）；④ 新会话开始，回应 hello。会话的对端关闭时清理它的租约。
- **清理的截止**（`Cleanup = "deadline"`）：③ 分成"清理开始"与"完成或超时强杀"两步。强杀按旧进程消失处理：旧会话的行随它消失，之后才回应新 hello；每次运行最多一次强杀。`Cleanup = "instant"` 保留旧模型的一步清理。
- **新连接的心跳**：连接在等待旧租约清理时不算"沉默"；心跳从回应它的 hello 起算（`NoHeartbeatBeforeHello`）。变异 `heartbeatFromAccept` 从接受连接起算，正在清理期间就会以 1001 关闭新连接。
- **停止后再开始**（应用托管的 `RutisApp.listen`）：停止关闭当前会话和已接受但未服务的连接（包括接管正在等的那条）、清理租约、不再接受拨号；开始开出新一轮。停止可以发生在接管的任何阶段（`closing`、`cleaning`、`starting`）；停止清理期间请求的开始在清理完成后生效；旧一轮的会话和排队连接不会在新一轮被服务（`RestartIsFresh`）。每轮最多停一次，总共 `MaxStops` 次；最后一轮的停止是最终的，之后不再开始、不再绑定（`FinalStopStaysDown`）。`lease-app-restart-3` 是 4 轮、每轮停一次（3 次再开始，最后一次停止是最终的）的交错。
- **重连次数**：每个控制方每轮最多 4 次付费连接（3 次重连）；`lease-faults` 是 3 次付费 + 3 次故障，见"结果"。

### `AccessPoint.tla`

- **参与者**：拨出的远端 `d1`、`d2`，自报同一个 id（`SameId`）或各自的 id；rutis 侧是一行 `rutis-host/listen`。
- **拨出方**（`dialing.py` 及各语言同物）：拨号即连接送到 listen、开始握手。收到 rutis 的 hello 即"connected"；连接被关闭（4002、断线、握手失败）后等待再拨。停止后不再拨号。一个可 `connect` 的应用（`RutisApp`）停止后可以再开始：新一轮的拨号编号从头数，旧一轮的连接不再读；最后一轮停止之后不再开始、不再拨（`FinalStopStaysDown`）。
- **故障之后的活性**：与 `RemoteLease` 相同，`MaxFaults` 用完之后 `access-faults-live` 检查每个 id 的 `EndsHeld`（不用对称性）。
- **listen**（`Listening::run`）：
  - 握手在 `JoinSet` 里并行：`HandshakeDone`（回 hello，远端的插件这时一个都还没注册）或 `HandshakeFails`（对端已关闭，这条连接失败）。
  - `Join`：循环取一个握手完成的连接。这个 id 没有 session 就装上；有就把旧连接的 closer 标为 replaced、从 `live` 拿掉、开始卸载旧 session，然后循环停在 `unload().await` 上，别的什么都不做。对端已经离开的连接不装（它的 hello 结果随会话消失）。
  - `Replace`：旧 session 卸载完（`Unloaded`：远端插件全部卸载，Channel 最后关闭，因为标了 replaced 所以是 4002），装新 session；等待期间对端离开的，直接丢掉。
  - `Register`：装上的 session 注册远端的插件。
  - `EndSelf`、`Unloaded`、`ReadEnding`：session 自己结束（远端关闭或断线），卸载；循环读到它的结束时，只有 `live` 里还是它（同一个 serial）才删掉。
- **吊销**（`Faults = {"revoke"}`）：环境吊销一个 id。已装上的 session 卸载，Channel 以 4003（模型里的 `"rejected"` 帧）关闭，拨出方按认证失败等待（`outcome = "authRejected"`）。监听行保留它的注册（`registered`），所以之后这个 id 的连接照样握手、收到 rutis 的 hello，然后在 `Join` 里以 4003 关闭，不装 session、不注册插件。吊销可以发生在握手途中或等待接管时（DIA-5-10 的监听侧）：`Join` / `Replace` 到装的时候再查一次 `revoked`，变异 `revokeAfterHandshake` 只查握手完成时的快照，会漏掉这时机的吊销。
- **重连次数**：每个拨出方每轮最多 4 次付费连接；打开断线的配置为 3 次付费 + 1 次故障，全部故障一起的为 2 + 1，静态吊销的为 2，见"结果"。

## 简化

- 时间不建模：退避、30 秒、握手 10 秒、"连接稳定 60 秒后重置退避"、心跳的 10/30 秒都只体现为"之后某个时刻"。超时是任意时刻都可能发生的 `Lose`，比真实的计时更一般。清理的截止也一样：进入清理后，"完成"和"超时强杀"都是可发生的动作，没有计时；每次运行最多一次强杀（`ghost.killed`）。强杀按旧进程消失处理，旧会话的行随之消失（变异 `killLeavesRows` 是反例）。Node / Bun 的强杀期限改为 `RUTIS_SHUTDOWN_TIMEOUT + 2 s` 这件事落在计时上：模型的 deadline 配置只检查"完成或被杀"的两种结果，期限本身由 `serve.test.mjs`、`remote.test.ts` 验证；5 秒与 12 秒在模型里是同一个动作。
- 心跳：只建"新连接在等待旧清理期间不会被心跳关掉"这一条顺序规则（`NoHeartbeatBeforeHello`）。模型按"从回应 hello 起算"建；变异 `heartbeatFromAccept` 是修复前的实现：Kotlin 的 `FrameConnection` 曾在构造时（接受连接）设 `lastSeen`，Go 的 `startWSConn`、Swift 的 `WebSocket.init`、Node 的 `wrap`、Bun 的 `wrap` 也一样。#1102 修了 Kotlin（`Frames.kt` / `Listen.kt`），#1182 修了 Go / Swift / Node / Bun：心跳都改为在会话的 hello 之后开始计时。
- 应用托管的停止与开始：`MaxRounds` 轮、`MaxStops` 次停止，一次运行里每轮最多停一次；停止可以在接管的任何阶段。停止的清理一步完成（真实实现的 `shutdownTimeout` 到时会强杀，见上一条）；"停止清理期间请求开始"建成 `pending`，清理后开下一轮。最后一轮的停止是最终的；再开始/再拨的变异是 `startAfterFinalStop` / `finalStopDials`。`RutisApp.kt` 的线程交错已在 f57b4138 修复，`access-app-restart` / `lease-app-restart-3` 按修复后的规则建。
- 有限的重连次数、故障次数和起停次数是模型的边界，不是实现的行为。活性只在没有 stop、故障有界（`MaxFaults`）的运行里检查；在那里每个行为都是有限的，所以 `WF_vars(Next)` 足以让每个能走的步骤走到。`lease-app-restart` 的 `RestartRecovers` 要求"还有下一轮且控制方还有连接可用"：一个已经把连接额度用完的控制方在开始后没法再拨，属性不要求它。`lease-app-restart-3` 只查安全：允许多次停止时，后一次停止会打断前一次停止之后要求的活性。两个控制方配置了同一个运行时（或两个拨出方用同一个 id）时会互相顶掉（拨出稿 §六 已写明）；模型里额度用完时最后一个连上的持有租约，实现里靠退避把节奏放慢到每方约 30 秒一次。
- 跨主机的租约竞争就是两个控制方配置（`lease-healthy`、`lease-faults`、`lease-faults-live`）：每台主机各自退避，先后不保证；模型把任意延迟当作它的交错，`EndsHeld` 说明额度用完后租约落到某个人手上。跨主机的时钟偏差和网络分区不单独建，断线用 `Lose` 表示。
- Cordis 节点连接（`node/rutis-runtime/src/bridge/link.mjs`）不在本仓库，没有代码可对；它的退避与接管规则按拨出稿 §六，模型的控制方就是这份规格。见"未覆盖"。
- `RemoteLease`：控制方的 hello 不建模（运行时开始会话时读它，没有别的作用）。
- `AccessPoint`：token 路由（`Router`）不建模：拨号即送到这一行；`registered` 是一个布尔，表示这一行的注册在不在。远端插件只建模"全部注册"与"全部卸载"两个状态，逆序卸载合成一步；远端经会话的调用不建模。`old.closer.close(Replaced)` 在 `Unloaded` 之后，Channel 已经关闭，模型里不单独成步。没有关闭码：`"close"` 是 4002 或普通关闭，`"rejected"` 是 4003。监听行自身的状态没有单独的变量：`registered` 表示注册还在，`Waiting { AuthRejected }` 记在拨出方的 `outcome = "authRejected"` 上。
- 每个动作是原子的。

### 未覆盖

- Cordis 节点连接（`bridge/link.mjs`）的实现核对：代码在 Cordis 仓库，不在本仓库。模型只能说明它的规则应当与 `dialing.mjs` 相同（500 ms 起、30 s 上限、稳定 60 s 重置、被拒 30 s），接管按 §4.4；没有跑过它的代码。
- 计时：退避的各档、握手 10 秒、心跳 10/30 秒、清理截止、Node / Bun 强杀期限。模型只用"某个时刻"和"完成或被杀"表示；这些由各语言的测试覆盖（`node/rutis-runtime/test/serve.test.mjs`、`bun/rutis-bun/test/remote.test.ts`、Kotlin `AppTest` 等）。
- Swift 停止之后才到的 `.accepted` 连接的关闭：只建了 `lateAccept` 变异（排队即违反 `RuntimeStopped`），没有对 Swift 的 drain 代码逐行核对。
- 运行时一侧的线程池、网络承载与 TLS、真实调度与内存序。

## 不变式与活性

| 名字 | 模型 | 内容 |
| --- | --- | --- |
| `OneLease` | RemoteLease | 运行时最多一个活动租约：正在服务的会话和还没清理的行加起来只属于一个连接 |
| `OldCommandsStayOld` | RemoteLease | 一个会话的指令（`load`）只作用于这个会话的租约，旧会话迟到的指令不作用于新租约 |
| `CleanBeforeHello` | RemoteLease | 运行时进入"回应新 hello"之前，旧租约的行已经全部清掉（强杀也一样） |
| `StoppedStaysDown` | RemoteLease | 停止的控制方 link 不再拨号，也不留打开的连接 |
| `ReadyIsCurrent` | RemoteLease | 控制方只在当前、仍打开的连接上 `Ready`：放弃了的握手迟到的 hello 不会让它复活 |
| `RuntimeStopped` | RemoteLease | 停下的运行时不服务会话、不持有租约，停止前接受但未服务的连接（包括停止之后才到的那条）也不留着 |
| `RestartIsFresh` | RemoteLease | 运行时服务或即将服务的连接属于当前一轮；控制方仍 `Ready` 的旧连接必须已被运行时关闭 |
| `FinalStopStaysDown` | 两者 | 最后一轮的停止是最终的：运行时不再开始，拨出方不再开始/拨号 |
| `NoHeartbeatBeforeHello` | RemoteLease | 新连接不会在回应 hello 之前被心跳关闭：心跳从回应 hello 起算 |
| `OnePluginSetPerId` | AccessPoint | 同一个 id 永远不会有两个 session 的远端插件同时注册着：新 session 只在旧的完全卸载之后才注册 |
| `StoppedRoundsStayDown` | AccessPoint | 停下的那一轮不再拨号，也不留打开的连接；再开始只在新一轮拨 |
| `StaysRegistered` | AccessPoint | 吊销一个 id 不撤掉监听行的注册：之后的连接还能到达它 |
| `RevokedStaysOut` | AccessPoint | 被吊销的 id 不再有装上的 session：之后同 id 的连接（含握手途中被吊销的）在 hello 之后以 4003 关闭 |
| `RestartIsFresh` | AccessPoint | 拨出方的 `ready` 属于当前一轮、当前那条连接，且这条连接真的收到过 hello；上一轮的迟到 hello 不会让新一轮的同号连接变 `Ready` |
| `EventuallyRevokedWaits` | AccessPoint，吊销 | 一个 id 被吊销后，拨出方最终以认证失败等待（`Waiting { AuthRejected }`） |
| `EventuallyHeld` | 两者，无故障 | 最终有一个控制方 / 每个 id 持有租约 |
| `EndsHeld` | 两者，无故障或故障有界 | `[]<>Held`：在有限的运行里即"结束时有人持有租约"，排除"互相顶掉后谁都没拿到" |
| `RestartRecovers` | RemoteLease，起停 | 停止之后还有下一轮、且还有控制方有连接可用时，运行时最终重新开始并有人持有租约；额度用完的控制方不作要求 |

`NoHeartbeatBeforeHello` 在基础配置里成立；模型的 `HeartbeatOut` 只在变异 `heartbeatFromAccept` 下可发生，基础配置不建真实心跳（见"简化"）。

## 结果

TLC 2.19（tla2tools v1.7.4），Temurin 21.0.12，32 核（`-workers 4`），2026-11-10。"通过"指穷举完成、所列性质全部成立；变异和修复前的配置找到第一个反例就停，状态数取决于搜索顺序。

| 配置 | 内容 | 结果 | 不同状态数 | 时间 |
| --- | --- | --- | --- | --- |
| `lease-healthy` | 2 个控制方，各 4 次付费连接，无故障；含活性 | 通过 | 791 | 1 s |
| `lease-faults` | 同上，各 3 次付费连接、3 次瞬时故障，link 可停止 | 通过 | 122,217 | 2 s |
| `lease-takeover` | 1 个控制方，最多 3 次连接（2 次接管）、2 次故障；鉴权是单独一步 | 通过 | 2,256 | 1 s |
| `lease-cleanup-deadline` | 1 个控制方，2 次连接、1 次故障；清理有截止，强杀旧会话 | 通过 | 45 | 1 s |
| `lease-app-restart` | 1 个控制方；运行时停一次、开下一轮，清理期间可以请求开始；`RestartRecovers` | 通过 | 1,391 | 1 s |
| `lease-app-restart-3` | 4 轮、4 次停止（每轮一次，最后一次是最终的），之后不再开始/绑定 | 通过 | 8,634 | 1 s |
| `lease-stop-during-takeover` | 2 个控制方；接管途中停止运行时，再开一轮 | 通过 | 79,521 | 2 s |
| `lease-faults-live` | 2 个控制方，各 2 次付费连接、2 次故障；故障之后的 `EndsHeld`，不用对称 | 通过 | 15,314 | 2 s |
| `access-healthy-same` | 2 个拨出方同一 id，各 4 次连接，无故障；含活性 | 通过 | 16,020 | 2 s |
| `access-healthy-distinct` | 各自的 id；含活性 | 通过 | 64 | 1 s |
| `access-lose` | 同一 id，各 3 次付费连接、1 次故障 | 通过 | 130,710 | 3 s |
| `access-all` | 同一 id，各 2 次付费连接、1 次故障，拨出方可停止 | 通过 | 132,693 | 3 s |
| `access-static-revoke` | 1 个拨出方，2 次拨号；id 被吊销；含活性 | 通过 | 51 | 1 s |
| `access-faults-live` | 各自的 id，各 2 次付费连接、2 次故障；故障之后的 `EndsHeld` | 通过 | 48,745 | 6 s |
| `access-app-restart` | 1 个拨出方，4 轮、4 次停止（最后一次是最终的）；之后不再拨 | 通过 | 66,988 | 2 s |

本表来自本机一次完整的 `run.sh`：32 个配置（含下面的变异）全部符合 `expected.txt`，约 30 秒。没有加更大的规模。

## 实现里的反例与疑点

TLC 找到过两处握手期间吊销身份仍建立会话（link 提供了 Peer、接入点在接管期间接受了被吊销的 id），已修复（#210）。身份吊销（`Revocations` / `Revocable`，#273）在 0.19 删除，配置 `lease-before-fix`、`access-before-fix`、`access-revoke-same`、`access-revoke-distinct`、`access-mut-revoke-cuts-all` 与性质 `RevocationScoped` 随之删除。`RevokedStaysOut` 随 #615 按追踪表的规则（DIA-5-7、DIA-5-10、DIA-5-11、DIA-5-13）重新加入（`access-static-revoke`），对的是规则，当前实现没有吊销钩子。

0.19 删掉接入点（`access_point.rs`）和注册代次（`registration.rs`），`AccessPoint.tla` 改为按 `rutis-host/listen` 的代码重建；配置 `access-registration`、`access-mut-route-by-id`、`access-mut-no-gen-check` 与性质 `OneInstancePerId`、`OldCallsStayOld`、`NoStaleHandOver` 随之删除。

本目录新增的变异对应 #615 的疑点，结论如下：

- `lease-mut-heartbeat-from-accept`：这是修复前的实现——连接在构造时（接受）设 `lastSeen`，旧租约清理超过心跳超时时间时新连接会在 hello 前被 1001 关闭。Kotlin #1102、Go / Swift / Node / Bun #1182 已改为从回应 hello 起算；变异保留为模型的反例。
- `lease-mut-late-accept`：停止之后才到的 `.accepted` 连接如果被排队，`RuntimeStopped` 立刻被违反。Kotlin `Listening` 的 `drain()` 会关闭它们；Swift 一侧没有逐行核对。
- `lease-mut-start-after-final-stop`、`access-mut-final-stop-dials`、`access-mut-match-by-number`：`RutisApp` 的多次起停与按编号匹配握手结果。f57b4138 已修 `RutisApp.kt` 的线程交错（stop 返回后不再拨/绑），模型按修复后的规则建，变异是修复前的行为。
- `access-mut-revoke-after-handshake`：吊销发生在握手完成之后、`Join` 之前时，只查握手完成时的快照会把它漏掉（DIA-5-10 修复前的规则）。当前实现没有吊销钩子，配置对的是规则。
- Node / Bun 的强杀时间：f57b4138 改为 `RUTIS_SHUTDOWN_TIMEOUT + 2 s`（`serve.mjs` / `remote.ts` 的 `shutdownDeadline() + KILL_MARGIN`），测试 `serve.test.mjs`、`remote.test.ts`。模型不建计时，见"简化"。

除此之外 TLC 没有找到与模型不符的反例：§4.4 的接管顺序（含鉴权一步）、listen 的顶替顺序、停止时取消重试、清理超时强杀不留下旧租约、应用托管停止后再开始的新一轮都不服务旧连接、故障额度用完后租约仍然落到某人手上，都与模型一致。新增的配置没有对实现运行；Swift / Kotlin / 应用托管的顺序是照代码读出来的（见上表）。

## 变异：确认模型不是空的

每个主要的不变式和活性性质配一个故意引入的错误，TLC 必须找到反例（`expected.txt` 里的 `violated:<名字>`；活性为 `violated:liveness`）：

| 配置 | 变异 | 被违反的性质 |
| --- | --- | --- |
| `lease-mut-start-before-clean` | 运行时在清理旧租约之前回应新 hello（§4.4 的 ③ ④ 对调） | `OneLease` |
| `lease-mut-read-old` | 运行时继续读被接管的会话，把它的指令作用到当前租约 | `OldCommandsStayOld` |
| `lease-mut-stop-keeps-retry` | 停止一个正在等待重连的 link 时没有取消重试 | `StoppedStaysDown` |
| `lease-mut-late-hello` | 已放弃的握手迟到的 hello 让 link 进入 `Ready` | `ReadyIsCurrent` |
| `lease-mut-wait-for-old` | 接管时停止读旧会话，但不关闭它，等旧控制方自己断开 | 活性（`EndsHeld`） |
| `lease-mut-kill-leaves-rows` | 清理超时强杀旧会话时，旧租约的行没有随进程消失 | `OneLease` |
| `lease-mut-stale-round` | 停止时保留已接受但未服务的连接，新一轮把它们服务了 | `RestartIsFresh` |
| `lease-mut-start-after-final-stop` | 最后一轮停止之后还能再开一轮 | `FinalStopStaysDown` |
| `lease-mut-heartbeat-from-accept` | 新连接的心跳从接受连接起算，清理期间被 1001 关闭 | `NoHeartbeatBeforeHello` |
| `lease-mut-late-accept` | 停止之后才到的连接被排队 | `RuntimeStopped` |
| `access-mut-accept-before-release` | 顶替时先装新 session，再卸载旧的（§4.2 的 3 挪到 1 之前） | `OnePluginSetPerId` |
| `access-mut-stop-keeps-retry` | 停止一个正在等待再拨的拨出方时没有取消再拨 | `StoppedRoundsStayDown` |
| `access-mut-revoke-installs` | 被吊销 id 的连接握手完成后照样装上 | `RevokedStaysOut` |
| `access-mut-revoke-unregisters` | 吊销时把监听行的注册也撤了，之后的连接在升级时 403 | `StaysRegistered` |
| `access-mut-revoke-after-handshake` | 只查握手完成时的吊销快照，等待接管时被吊销的连接照样装上 | `RevokedStaysOut` |
| `access-mut-match-by-number` | 按连接编号而不是轮次匹配握手结果 | `RestartIsFresh` |
| `access-mut-final-stop-dials` | 最后一轮停止之后还能再开一轮并拨号 | `FinalStopStaysDown` |

每个变异都在 2 秒内找到反例（几十到几百个状态）。

## 怎么运行

需要 Java 11 以上和 `tla2tools.jar`（[v1.7.4](https://github.com/tlaplus/tlaplus/releases/tag/v1.7.4)，sha256 `936a262061c914694dfd669a543be24573c45d5aa0ff20a8b96b23d01e050e88`）。jar 不要提交进仓库。

```sh
curl -sSLo /tmp/tla2tools.jar https://github.com/tlaplus/tlaplus/releases/download/v1.7.4/tla2tools.jar
TLA2TOOLS=/tmp/tla2tools.jar specs/remote-lease/run.sh                     # 全部配置
TLA2TOOLS=/tmp/tla2tools.jar specs/remote-lease/run.sh lease-faults        # 单个配置
TLC_WORKERS=4 TLA2TOOLS=/tmp/tla2tools.jar specs/remote-lease/run.sh       # 限定 TLC 的线程数
```

`run.sh` 按名字选模型（`lease-*` → `RemoteLease.tla`，`access-*` → `AccessPoint.tla`），把每个配置的结果和 `expected.txt` 比较，全部符合时退出码为 0；日志和反例写在 `TLC_OUT`（默认一个临时目录）。CI 的 `specs` 任务在改了 `specs/**` 的 PR 和每晚的完整运行里跑它（[CI](../../docs/ci.md) §4）。

## 维护

远程插件 §3.2、§4.4、拨出稿 §五、§六 或远程会话 §4.2、§4.3 修改时，或 `connect/dial.rs`、`listen.rs`（`Listening::run`）、各语言监听端的接管顺序与清理截止、`RutisApp.start/stop`、拨出方的重连规则改变时，同步修改这个模型和 `expected.txt`（规范 Q11.2.3）。静态监听 link 的吊销没有再实现；追踪表的 DIA-5-7 / 5-10 / 5-11 / 5-13 修改时改 `access-static-revoke` 与 `access-mut-revoke-after-handshake`。计时规则（强杀期限、心跳起点）修改时，改 README 的"简化"和对应的测试，模型只保留顺序规则。
