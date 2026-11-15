# 行生命周期：代次、两阶段、服务与版本、租约、崩溃与多运行时（TLA+）

[English](README.en.md)

用 TLC 穷举一个运行时会话里的行从解析、装载到清理的生命周期，以及断线、同 id 重连、旧代卸载与新代装载、服务公告（带版本）、撞名拒绝与它的作用域、宿主服务租约与 `hosts.provide`/`hosts.withdraw`、运行时崩溃/重启、两个运行时的冷启动、可被打断并重做的刷新、就地提交的 `rows.update` 和“装载中结束的行”。对应 issue [#613](https://github.com/arcships/rutis/issues/613) 的行生命周期部分（并入了 [#625](https://github.com/arcships/rutis/issues/625)）；质量规范 Q11.2；设计见[远程会话](../../docs/design-remote-session-2026-10-28.md) §4.2、§4.3、[远程插件](../../docs/design-remote-plugins-2026-10-03.md) §4.4、[多语言运行时](../../docs/design-multilang-runtimes-2026-10-03.md) §四、§五、§六、[会话追踪契约](../../docs/formal-trace.md) §3.0。

这是同一个模型的两块，靠常量开关区分：原有 22 个配置（9 个 ok 与 13 个变异）只打开第一块，语义与上一版相同（三个穷举完成的配置状态数逐字相同：73,768 / 191,830 / 110；反例配置的状态数本来就随搜索顺序变）；新配置逐个打开两阶段、服务与版本、拒绝作用域、依赖、租约（含 `hosts.provide`/`withdraw` 计数）、崩溃、第二个运行时、刷新任务、`rows.update` 和装载中结束的行。不建事件登记与 `rows.ended` 的帧序（#613 的事件表）、调用生命周期和节点托管行。

## 对应的设计和代码

| 模型部分 | 设计 | 实现 |
| --- | --- | --- |
| 宿主每个运行时一条会话，丢了才重拨；行通过会话加载，会话没了就停下等下一个 | 远程会话 §4.2；远程插件 §4.4 | `crates/rutis-host/src/connect/dial.rs` `Dialing::run`（`installed.unloaded().await` 之后才退避重拨）；`crates/rutis-loader/src/runtime/sessions.rs` `SessionRowsPlugin`（行等会话，会话结束就停下） |
| 宿主按代次键装载一行：每次装载一个新键，旧代的迟到回复不作用于新代 | 质量规范 §11.2；#625 `OwnGenerationOnly` | `crates/rutis-loader/src/runtime.rs:719-727`（`ctx.instance()` + `GENERATION`）；`crates/rutis-bridge/src/runtime/process.rs` 的 `on_row_ended` / `on_row_failed`（按键登记，`unload_row` 之后移除） |
| `rows.ended`：行自行结束，宿主结束这一行并把它卸掉（不再装载） | 远程会话 §4.3；`loader/self-dispose` | `crates/rutis-bridge/src/runtime/process.rs:531-541`；`node/rutis-runtime/src/runner.mjs:379-385`（`endRow`：先 `unloadRow`，再发 `rows.ended`） |
| 装载中结束的行：装载成功之后才报它的结束（54a7e824） | 远程会话 §4.3 | `crates/rutis-bridge/src/runtime/process.rs` 的 `on_row_ended`（登记在装载之前，回复之后才发）；`node/rutis-runtime/src/runner.mjs` 的 `endRow` 等 apply 结束 |
| `rows.failed`：行的代码失败，宿主重启它（卸载旧代、装载新代），运行时保留这一行 | 远程会话 §4.3；#688 | `crates/rutis-bridge/src/runtime/process.rs:543-568`；`node/rutis-runtime/src/runner.mjs:1120-1135`（`rows.failed` 之后行仍加载） |
| `rows.unload` 幂等：行已不在就只回答 | 行契约 `rows.v2` | `crates/rutis-bridge/src/session/plugin/rows.rs:197-207`；`node/rutis-runtime/src/runner.mjs:743-746`（`unloadRow` 找不到就返回） |
| 运行时顶替：关闭旧会话、不再读它的帧、清理旧会话的全部行、然后回答新 hello | 远程插件 §4.4；远程会话 §4.2 | 运行时监听端（Python `serve`、Go `go/rutis/internal/runtime/listen.go`、Node `node/rutis-runtime/src/serve.mjs` 同一顺序）；模型按 Node 的行处理建（`node/rutis-runtime/src/runner.mjs`） |
| 会话结束（断线或顶替）时清理它的全部行，每行一次 | 远程插件 §4.4；`loader/remote-runtime-rows`、`loader/row-unload-leaves-nothing` | 运行时的 `serve`/`listen`；`crates/rutis-bridge/src/session/plugin/rows.rs` 的 `Server`（每个会话一张已装载表） |
| 两阶段放行：进程起来先提供 `<语言>Runtime`；宿主用它刷新这些行的解析结果；再提供 `<语言>RuntimeRows`，行才按完整依赖等待和启动 | 多语言运行时 §四、§六；M1 §6.2；#625 `NoProvisional` | `crates/rutis-loader/src/runtime/sessions.rs` 开头的两阶段（`SessionRowsPlugin` 先等待、重新解析、再提供 `SessionRows`）；各语言运行时插件提供两个服务；刷新用 loader 的 `entries()` + `reload(id)` |
| 刷新是独立任务：运行时重启打断它，重启之后重做（REM-4.3-3） | 多语言运行时 §六；#625 | `crates/rutis-loader/src/runtime/sessions.rs` 的刷新任务（不在进行中的 reconcile 里同步等待） |
| 服务公告带版本：更老的公告不覆盖更新的；行可以再次提供同一个名字（服务更新）；行的清理在卸载之前撤下它的名字；宿主把公告发布成本地服务是单独一步，清理等它完成（#372） | 多语言运行时 §五；M1 §5.2、§5.3、§14；#625 `NewerWins`、`WithdrawBeforeUnload` | `crates/rutis-bridge/src/runtime/process.rs` 的 `Imports::update`（`version <= entry.1` 丢弃）、`announce`；`crates/rutis-bridge/src/runtime/projection.rs` 的 `publish` / `withdraw`（`withdraw` 等进行中的发布做完） |
| 撞名拒绝：第二个还在运行的提供者被拒绝；拒绝只让引起它的那次重启失败，等依赖而不是 apply 的重启不会失败（d94aae9） | 多语言运行时 §五；M1 §5.2；#625 `RefusalScope` | `crates/rutis-loader/src/runtime.rs` 的 `Refusals`（按 fiber 实例记，`arm` / `take` / `forget`） |
| 宿主服务租约与 `hosts.provide`/`hosts.withdraw` 的计数：行装载时租用它注入的宿主服务，清理时在 `rows.unload` 之后释放；第一个租约 provide，最后一个释放 withdraw；运行时记下这个服务的形状 | 多语言运行时 §五；M1 §5.4；#625 `LeaseWithinRow`、`HostLeases` | `crates/rutis-loader/src/runtime.rs:623-665`（`lease_host_for`、清理里的 `release`）、`:1027-1031`（`release`）；`crates/rutis-bridge/src/runtime/process.rs` 的 `Process::lease` / `release_host`、`host_changes` |
| 进程崩溃：两个服务都撤销、不自动重启，是否重启由应用决定；重启后行重新解析 | 多语言运行时 §四 | 各语言运行时插件；`crates/rutis-loader/src/runtime.rs` 的 `wait_for_the_runtime`（装载中运行时结束的行等待它的结束） |
| 两个运行时不互相等待：一个运行时的启动/会话/行不依赖另一个 | 多语言运行时 §四（运行时不依赖任何行的服务）；`loader/runtimes-do-not-wait-for-each-other`、SYN-2-14、SYN-4-4 | 运行时插件只依赖启动本身；门控落在消费行上（`crates/rutis-loader/src/runtime.rs` 的 `gated` / `row_inject`） |
| 只改易变字段时就地 `rows.update`，不卸载也不换代（SYN-4-8） | 多语言运行时 §四；#101 的做法 | `crates/rutis-loader/src/runtime.rs` 的 `Forward::apply`（`update_row`）；`crates/rutis-bridge/src/runtime/process.rs` 的 `Imports` → `rows.update` |

## 模型

- **参与者**：一个宿主（loader 的行、拨号、服务表与版本、宿主服务租约、发布步骤）和 1–2 个运行时进程（监听方、行表、两个放行阶段、一个刷新任务）。宿主对每个运行时各有一条连接，连接断了就重拨；运行时一次只服务一个会话，新连接到来时顶替旧的。行的归属由配置的 `RowSet2` 决定：不在其中的行属于运行时 `r1`。
- **连接**：每个运行时 `1..MaxConns`，每条连接两个方向各自 FIFO、延迟任意长；关闭了连接的一方不再读它，发给它的帧丢弃。
- **两阶段与刷新**：`StartRuntime(rt)` 让进程从 `down` 到 `up`（提供 `<语言>Runtime`）；刷新有两条路：`Refresh(rt)` 一步完成（原有配置），或 `RefreshBegin(rt)` / `RefreshDone(rt)` 把刷新建成 `refreshing` 阶段里的独立任务（`Refreshes`）。崩溃取消进行中的刷新（`refresh = "cancelled"`），再次启动之后 `RefreshBegin` 重做它（`noRefreshRedo` 变异不重做）。`HLoad(r)` 要求本运行时的阶段是 `rows` 且这一行的解析是 `final`。
- **宿主**（`HDial`、`HGreeted`、`HLoad`、`HLoaded`、`HUnloaded`、`HLoadFailed`、`HAnnounce`、`HProvide`、`HPublication`、`HPublicationDone`、`HEnded`、`HFailed`、`HLoadRefused`、`HWaitRefusal`、`HUpdate`、`HUpdated`、`HLose`）：
  - `HLoad(r)` 用 `<<运行时, 会话, 行, 代次>>` 作键发 `rows.load`；一个会话里一行最多装载 `MaxLoads` 次（失败重启用掉一次）。装载时同时记下这一代租用的宿主服务；`HostFrames` 打开时每个名字的第一个租约 `provide`、最后一个释放 `withdraw`，并记下运行时的形状。
  - `HLoaded` 处理装载回复；若这一代的服务公告被拒绝，行不进入 Running，而是进入卸载。打开 `Versions` 时它还要求这一代宣布的名字都发布完（或发布被拒绝），所以“行 Running”与“它的服务已发布或已被拒”一致。`HUnloaded` 处理卸载回复：清掉这一代的服务公告（版本、见下）、释放租约、行回 `idle`（`leaseLeak` 变异跳过释放）。`HLoadFailed` 处理 `loadfailed`：释放租约、行回 `idle`。
  - `HAnnounce`（不带版本）应用一条服务公告：名字已被另一个还活着的键占用就拒绝（后来的行失败，先来的不受影响）；公告在这一行进入 Running 之前处理（`announceAfterLoad` 变异让运行时先回 `loaded`，用来检查 `ServicesBeforeRunning`）。
  - `HProvide`（`Versions`）把一条带版本的公告写进宿主的表：版本不比表里已有的新就丢掉（`regardlessOfVersion` 变异照样应用，于是迟到的旧公告覆盖新版本，违反 `NewerWins`）。`HPublication` / `HPublicationDone` 是发布这一步：把这一代宣布且版本比已发布新的名字注册成本地服务；名字已被另一个还在运行的行占用就拒绝这一行。`HUnloaded` / `HLoadFailed` 在这一行卸载时把它的名字按“下一个版本”撤下——版本比任何在途公告都新，所以迟到的 `provide` 不会把它带回来；`loseWithdrawal` 变异在发布还没做完时丢掉这次撤下（#372），行卸载之后它的名字还留在宿主上。
  - `HLoadRefused` 让引起拒绝的那次重启失败一次，然后清掉记录（`refusalSticky` 变异让它一直留着）。`PreciseRefusal` 打开时，拒绝记在行（fiber 实例）上：`HLoadRefused` 只让冲突之后紧接的那次 apply 失败，`HWaitRefusal` 表示这次重启在等依赖、没有 apply，于是拒绝被忘掉，不再失败后面的代。
  - `HEnded` 只结束键等于这一行当前代次的行，并像真实卸载栈一样再发一次 `rows.unload`；`HFailed` 把当前代次置为卸载中并等回复。`HLose` 断开当前连接，这一会话上的行回 `idle`；这一会话的服务公告、租约和别的运行时上因此失去依赖的行一并处理。
  - `HUpdate`（`Updates`）把只改易变字段的更新发给运行时（同一代次、同一个键），`HUpdated` 收回复；`updateReload` 变异改为卸载再装载。
  - 依赖消失：提供者被卸载时，宿主在同一个动作里清掉名字，并把仍在 `loading`/`loaded`、因此少了依赖的行置为卸载（`Starved`）；行之后回到 `idle` 等名字回来（`keepOnDepGone` 变异不做这一步）。
- **运行时**（`TakeUp`、`Answer`、`EndSession`、`RLoad`、`RLoadFinish`、`RProvide`、`RFailLoad`、`RUnload`、`EndSelf`、`EndDuringLoad`、`SendEnding`、`RFail`、`RUpdate`、`Sweep`、`MarkSwept`）：
  - `TakeUp(s)`：读到新连接的 `hello`，关闭旧会话（不再读），开始清理旧会话的行。`Answer(s)`：只有本运行时更早会话的行都清理完才回答新 `hello`，所以两个会话的行不会同时存在；它不要求任何别的运行时（`mutualWait` 变异要求其他运行时也 fully ready 且全部 swept）。
  - `RLoad` 在不带版本时把这一行提供的每个名字作为 `announce` 帧放进同一会话的回队列，然后才放 `loaded` 回复（`announceAfterLoad` 变异反过来）。`Versions` 打开时，第一次 `provide` 是装载的一部分（在回复之前，`RProvide`），`RLoad` 只回 `loaded`；行运行期间还可以再 `provide` 一次（服务更新，`MaxProvides` 给上限）。`RLoad` / `RLoadFinish`（`LoadSteps`）把“读到装载”和“回复”分成两步。
  - `RUnload` 幂等，行在就清理一次、不在就只回答；`EndSelf` 是插件自行结束（行消失而 `rows.ended` 还要再发）；`EndDuringLoad`（`LoadSteps`）是插件在 apply 期间自行结束：装载照样成功，`SendEnding` 要等 `loaded` 回复发出去之后才发（`endedDuringLoad` 变异当场发，违反 `EndingAfterLoad`）；`RFail(k)` 每代最多发一次 `rows.failed`。
  - `RUpdate`（`Updates`）在同一代上提交 `rows.update`，回 `updated`。
- **崩溃**：`HCrash(rt)` 清掉这个运行时所有已装载的行（每行清理计数加一次）、租约和服务公告，关闭它的全部会话，阶段回 `down`，该运行时的行回 `idle`、解析回 `provisional`，进行中的刷新记为 `cancelled`；`StartRuntime` 的次数受 `Restarts` 限制，宿主重新拨号受 `MaxConns` 限制。一个运行时的崩溃只动它自己的状态（配置检查另一个运行时的行仍能 Running）。
- **顶替的顺序**：`TakeUp` 关旧、停止读，清行（`Swept`），`Answer` 回答新 `hello`。`Answer` 的前置条件仍是这个模型检查的核心规则。

## 简化

- 规模是有意的有限：旧配置 1 个运行时、1–2 行、最多 3 条连接；新配置最多 2 个运行时、每个运行时最多 2 行、每个会话每行最多 2 代、1 次崩溃、最多 2 次启动、2 次拨号、每代最多 2 次 `provide`、最多 2 次 `rows.update`。真实的重复崩溃、退避和 30 秒/60 秒计时不建：环境只体现为“某个时刻断线/崩溃”，比真实计时更一般。
- 握手只建“一端 `hello`、另一端回答 `hello`”：版本、能力、鉴权、10 秒截止不建（`remote-lease` 覆盖顶替与握手的其它部分）。
- 宿主→运行时的帧只有 `rows.load`、`rows.unload`、`rows.update`；`rows.schema`/`mount`、事件、调用链都不建。运行时→宿主只有 `hello`、`announce`、`provide`、`loaded`、`loadfailed`、`unloaded`、`ended`、`failed`、`updated`。
- 服务名最多两个（`ServiceName` 与 `GateName`）：一个被争夺的提供名和一个用来让被拒绝的行“等依赖”的名字；宿主服务只有一个名字。多名字、多方法的方法形状不建：形状随 `hosts.provide` 和公告一起带着，检查“有注册就有形状、形状与第一次 provide 的一致”；同步/异步调用的后果不在本模型（调用生命周期在 #613、#614）。
- 服务版本按“每个名字一个递增的计数器”建：每次 `provide`（含服务更新）和每次撤下各取一个版本，宿主只应用比表里新的。实现里的版本是 `Imports.slots[id]` 的最后一个版本，比较规则相同。
- 行运行期间撤下单个服务名（`ctx.unprovide`）不单独建：模型里的撤下只发生在行的清理里（`HUnloaded` / `HLoadFailed` 的 `Dropped`），用的是同一条版本规则；被撤下的名字也不再让消费行停下（依赖消失只在提供者卸载、掉线、崩溃时建模，配置 `row-deps`）。
- 撞名拒绝在带版本时发生在“发布”这一步（名字的本地键被另一个还在运行的行占着），与实现的 `provide_mut_at` 相同；不带版本时按“公告”这一步记（原有配置）。
- 行的装载失败（`loadfailed`）只表示运行时装载这一行失败；行的代码在运行中失败是 `rows.failed`，两者分开。
- 依赖出现只是 `HLoad` 的前置条件；依赖消失按“提供者卸载/掉线/崩溃时，宿主在同一个动作里让消费行卸载”建。刷新失败只失败这一行（M1-6.2-4）不建。
- 崩溃是一个原子动作（进程死、会话结束、行清理一起发生）；各语言卸载栈的差异、`shutdown` 截止、Node/Bun 的强杀时间不建（#625 的其余部分）。
- 宿主一行是一个 fiber：`idle | loading | loaded | unloading | gone`，键是 `<<运行时, 会话, 行, 代次>>`；loader 的完整解析器、配置分层、重启策略的计时不建。“行自行结束 = 禁用、不再装载”按 `loader/self-dispose`。
- 旧会话队列里还没读的帧留在模型里（设计里它们随连接关闭被丢弃）；设计侧由 `CanRead` 保证不再读，只有 `readOld` 变异会去读它们。
- 宿主在丢掉会话后不再读它，所以“旧会话的迟到 `rows.ended`”不会落到新会话的宿主上；宿主一侧的同一规则体现在同一会话里的代次竞态（`OwnFramesOnly`），这正是 a88fb65b 修的缺陷。
- 内存序、任务调度、真实计时、帧编码不建模；每个动作原子。
- **未覆盖**（#625 里仍没有形式化证据的）：`rows.ended` 与 `events.*` 的帧序（#613 的事件表）、调用生命周期与跨进程同步调用链、节点托管行、各语言运行时内部的 apply 取消与卸载栈差异、`rows.schema`/`mount` 的形状协商、同一名字第二个租约不重新 `hosts.provide` 时的形状刷新（同一名字只有一个服务对象，模型里形状不变化）、`runtimes_ready`（#314）、带 inject 的行的 apply 失败（#584）、以及 #247 / #191 的具体情形。

## 不变式与活性

| 名字 | 内容 |
| --- | --- |
| `OneRowPerName` | 运行时里一个名字的行最多一个代次处于加载状态：旧会话的行清理完之后新会话才装载；失败重启等旧代卸载的回答，不并存 |
| `NewSessionOnlyAfterSweep` | 一个会话被回答（`hello`）时，本运行时所有更早会话的行都已经清理完 |
| `NoRowsAfterSweep` | 一个会话被清理过一次之后，它的帧不会再装载任何行：运行时不再读旧会话的指令 |
| `OwnFramesOnly` | 宿主只对“这一行当前代次”的 `rows.ended`/`rows.failed` 起作用：旧代的迟到帧不结束、不重启新代 |
| `CleanupOnce` | 一个行代次最多清理一次：会话清理、`rows.unload`、行自行结束和崩溃不会把同一行处置两次 |
| `NoProvisional` | 行只在运行时完成两阶段、且自己的解析是 final 之后才装载/运行：不在临时解析上装载 |
| `ServicesBeforeRunning` | 宿主把一行记为 Running 时，这一行装载期间发出的服务公告都已处理：公告在装载的同步调用链里到达（帧序上先于 `loaded` 回复） |
| `OneProviderPerName` | 两个行不会同时以同一个名字 Running：后来的公告被拒绝，它的装载失败（带版本时按“已发布”算提供者） |
| `RefusalOnce` | 一个被拒绝的代次引起的重启最多失败一次：拒绝不会带给后面的代 |
| `RefusalScope` | `PreciseRefusal`：一次拒绝不会既让某次重启等依赖、又让它的 apply 失败——等依赖的那次不算，拒绝被忘掉（d94aae9） |
| `NewerWins` | `Versions`：宿主的表里始终是它应用过的最新版本，迟到的更老公告不会覆盖更新的 |
| `WithdrawBeforeUnload` | `Versions`：一行卸载完成之后，它的名字不再留在宿主上（清理在卸载之前撤下，且发布中途的撤下不会丢） |
| `HostLeases` | `HostFrames`：`hosts.provide`/`hosts.withdraw` 收支平衡——运行时持有某个名字当且仅当有代次租着它，注册期间正好有一个 provide，运行时对注册的名字有形状 |
| `EndingAfterLoad` | `LoadSteps`：装载中结束的行，结束只在装载回复之后才报出（54a7e824） |
| `UpdateInPlace` | `Updates`：只改易变字段的更新提交在这一行正在跑的那一代上：不卸载、不换代（SYN-4-8） |
| `LeaseWithinRow` | 租约只属于这一行的当前代次；卸载回复之后下一次装载之前必须已释放；失败的装载不留租约 |
| `LoadedNeedsDeps` | Running 的行注入的每个名字都还在：名字消失时宿主同步地把它置为卸载，不会留着一个缺依赖的行在跑 |
| `EndedLeadsToSwept` | 活性：一个会话结束（断线、顶替或崩溃）之后，它的行最终被清理 |
| `RestartRecovers` | 活性：运行时崩溃且还能再启动、宿主还能再拨号时，它的行最终重新 Running |
| `RuntimesDoNotWait` | 活性：一个运行时崩溃（不再启动）时，另一个运行时的行仍然最终 Running |
| `ColdStartLoads` | 活性：一个运行时完成两阶段、解析 final、会话就绪之后，它的行最终 Running；两个运行时同时冷启动时各自完成（SYN-2-14） |
| `RefreshRedone` | 活性：被运行时重启打断的刷新最终被重做（`stage = rows`）（REM-4.3-3） |

## 结果

TLC 2.19（tla2tools v1.7.4），Java 21.0.12（Temurin），32 核，`-workers auto`（下面用 8）。`通过` 指穷举完成、所列性质全部成立；被违反的配置找到第一个反例就停，状态数取决于搜索顺序，每次可能不同。整个目录约 30 秒。

| 配置 | 内容 | 结果 | 不同状态数 | 时间 |
| --- | --- | --- | --- | --- |
| `row-healthy` | 2 行，1 条连接；行可失败、可自行结束；原有 5 个安全不变式 | 通过 | 73,768 | 1–2 s |
| `row-reconnect` | 2 行，3 条连接（2 次重连），可断线 | 通过 | 191,830 | 2–5 s |
| `row-cleanup` | 2 行，1 条连接，可断线；只检查活性 | 通过 | 110 | 1 s |
| `row-generations` | 2 行，1 条连接，两阶段，行可失败（每行两代）；`NoProvisional` 等 | 通过 | 419 | 1–2 s |
| `row-services` | 2 行都提供 `s`，两阶段，行可失败；公告、撞名拒绝、拒绝只失败一次重启 | 通过 | 2,643 | 2 s |
| `row-versions` | 2 行都提供 `s`，公告带版本，每代可再 provide 一次（服务更新）；`NewerWins`、`WithdrawBeforeUnload` | 通过 | 64,953 | 2–3 s |
| `row-lease` | 2 行都租用宿主服务 `h`；行可失败、装载可失败；租约 | 通过 | 1,334 | 2 s |
| `row-host-frames` | 2 行都租用 `h`；`hosts.provide`/`withdraw` 的帧与计数、形状、失败的装载归还租约 | 通过 | 3,156 | 1–2 s |
| `row-deps` | `a` 提供 `s`、`b` 注入 `s`，`a` 可失败重启；依赖消失 | 通过 | 422 | 2 s |
| `row-restart` | 1 个运行时崩溃一次、再启动；宿主拨两次；旧行清理、新行装载 | 通过 | 1,232 | 1 s |
| `row-runtimes` | 2 个运行时各 2 行；`r1` 崩溃且不再启动，`r2` 照常；不互相等待 | 通过 | 1,988 | 1 s |
| `row-coldstart` | 2 个运行时、各 2 行同时冷启动；两个冷启动各自完成，行最终 Running | 通过 | 784 | 1 s |
| `row-refresh` | 1 行；刷新是独立任务，崩溃取消它、重启之后重做（活性） | 通过 | 358 | 1 s |
| `row-updates` | 1 行；`rows.update` 两次，都提交在同一代上 | 通过 | 23 | 1 s |
| `row-ended-during-load` | 1 行；插件在 apply 期间自行结束，结束在装载回复之后才报 | 通过 | 39 | 1 s |
| `row-refusal-scope` | 2 行争 `s`，其中一行另注入 `d`；被拒绝的重启在等 `d`，拒绝被忘掉 | 通过 | 628 | 1 s |
| `row-mut-accept-before-clean` | 顶替时不等旧会话的行清理完就回答新 `hello` | 违反 `NewSessionOnlyAfterSweep` | 158 | 2 s |
| `row-mut-accept-before-clean-loads` | 同上，接着新会话装载同名行 | 违反 `OneRowPerName` | 683 | 1 s |
| `row-mut-read-old` | 顶替后运行时继续读旧会话 | 违反 `NoRowsAfterSweep` | 278 | 1 s |
| `row-mut-keyed-by-row` | 宿主按行名而不是键匹配 `rows.ended`/`rows.failed` | 违反 `OwnFramesOnly` | 119 | 1 s |
| `row-mut-reclean` | 对已经不在的行再跑一次卸载清理 | 违反 `CleanupOnce` | 53 | 1 s |
| `row-mut-keep-rows` | 运行时关闭断线的会话但保留它的行 | 违反活性 | 29 | 0 s |
| `row-mut-load-provisional` | 行在临时解析、运行时还没放行行的时候就装载 | 违反 `NoProvisional` | 81 | 1 s |
| `row-mut-announce-after-load` | 运行时先回 `loaded`，之后再报服务公告 | 违反 `ServicesBeforeRunning` | 23 | 1 s |
| `row-mut-allow-clash` | 宿主接受第二个提供同名服务的公告 | 违反 `OneProviderPerName` | 117 | 1 s |
| `row-mut-newer-wins` | 宿主不看版本，应用读到的每一条公告 | 违反 `NewerWins` | 16,981 | 1 s |
| `row-mut-projection-close` | 发布中途关闭投影，丢掉撤下（#372） | 违反 `WithdrawBeforeUnload` | 21,607 | 1 s |
| `row-mut-lease-leak` | 卸载/装载失败后不释放宿主服务租约 | 违反 `LeaseWithinRow` | 94 | 1 s |
| `row-mut-lease-orphan` | 租约在等 `hosts.provide` 时被丢掉：计数留着、没有主、服务永不撤下 | 违反 `HostLeases` | 75 | 1 s |
| `row-mut-keep-on-dep-gone` | 依赖消失后不停止消费行 | 违反 `LoadedNeedsDeps` | 211 | 1 s |
| `row-mut-refusal-sticky` | 撞名拒绝一直留着，失败后面的重启 | 违反 `RefusalOnce` | 278 | 2 s |
| `row-mut-refusal-scope` | 拒绝在等依赖的重启之后还留着，失败再后面的 apply（d94aae9 之前） | 违反 `RefusalScope` | 465 | 1 s |
| `row-mut-ended-during-load` | 装载中结束的行当场报结束（54a7e824 之前） | 违反 `EndingAfterLoad` | 40 | 0 s |
| `row-mut-update-reload` | 易变字段的更新卸载再装载，而不是就地提交 | 违反 `UpdateInPlace` | 15 | 1 s |
| `row-mut-no-refresh-redo` | 被取消的刷新不再重做 | 违反活性（`RefreshRedone`） | 358 | 1 s |
| `row-mut-mutual-wait` | 一个运行时的回答等另一个运行时完全就绪且全部清理 | 违反活性（`RuntimesDoNotWait`、`ColdStartLoads`） | 84 | 1 s |

## 变异与反例

每个被检查的性质都配一个故意引入的错误，TLC 必须找到反例（`expected.txt` 的 `violated:<名字>`；活性为 `violated:liveness`）：

| 配置 | 变异 | 违反 | 反例 |
| --- | --- | --- | --- |
| `row-mut-accept-before-clean` | `Answer` 不要求更早的会话已 `Swept` | `NewSessionOnlyAfterSweep` | 会话 1 装载了行，断线，拨会话 2；`TakeUp(2)` 关闭会话 1，`Answer(2)` 立即回答 `hello`，此时 `Swept(1)` 还是假 |
| `row-mut-accept-before-clean-loads` | 同上 | `OneRowPerName` | 接着会话 2 装载行 `a`，会话 1 的 `a` 还在运行：同名两代并存 |
| `row-mut-read-old` | `CanRead` 允许读已结束会话的帧 | `NoRowsAfterSweep` | 会话 1 上没被读的 `rows.load` 在会话被清理并 `MarkSwept` 之后才开始装载 |
| `row-mut-keyed-by-row` | `HEnded`/`HFailed` 按行名匹配，不看键的代次（a88fb65b 之前） | `OwnFramesOnly` | 行 `a` 卸载后插件才发 `rows.ended`；宿主已经装载了新代，读到旧代的 `ended` 后把新代结束 |
| `row-mut-reclean` | 对已不在的行也执行一次清理 | `CleanupOnce` | 行失败；插件自行结束（清理一次），随后的 `rows.unload` 发现行已不在，变异又清理一次：`cleanCount = 2` |
| `row-mut-keep-rows` | `Sweep` 关闭（断线后不清理行） | 活性（`EndedLeadsToSwept`） | 会话 1 装载了行后断线，运行时不清理：`Swept(1)` 永远不成立 |
| `row-mut-load-provisional` | `HLoad` 不要求 `stage = rows` 也不要求解析 final（6f8fb5c7 之前） | `NoProvisional` | 初始阶段还是 `down`、行还是 `provisional`，变异让行直接 `loading`；`NoProvisional` 立即为假 |
| `row-mut-announce-after-load` | `RLoad` 先放 `loaded` 回复、再放 `announce` 帧（2f830f81 之前） | `ServicesBeforeRunning` | `sentAnn` 已含 `s`，宿主的 `processedAnn` 还是空，`HLoaded` 已把行标成 `loaded` |
| `row-mut-allow-clash` | 名字已被另一个活着的键占用时仍接受公告 | `OneProviderPerName` | `a`、`b` 都提供 `s`；两次公告都被应用，两次装载都回 `loaded`，宿主上两行同时 Running |
| `row-mut-newer-wins` | 宿主应用每一条读到的公告，不比较版本 | `NewerWins` | 行的清理把名字按版本 3 撤下之后，通道里迟到的 `provide`（版本 1）被应用：表里是旧版本，`ever` 还是 3 |
| `row-mut-projection-close` | 发布还在进行时行的清理不等待，撤下被丢掉（#372 之前） | `WithdrawBeforeUnload` | 行装载后再次 provide（版本 2），发布开始；行失败卸载，清理在发布中途完成，名字仍是这一代的行：`hrow = idle` 而 `claimed = 这一代的键` |
| `row-mut-lease-leak` | `HUnloaded`/`HLoadFailed` 让行回 `idle` 但不释放租约 | `LeaseWithinRow` | 行失败并卸载（或装载失败）；行已 `idle`，`leaseHeld` 仍非空 |
| `row-mut-lease-orphan` | 行重启时租约在等 `hosts.provide` 中被丢掉 | `HostLeases` | 装载行时 `aux.hosts` 的计数加一，但没有任何代次持有它：`count = 1` 而持有者为空 |
| `row-mut-keep-on-dep-gone` | 服务消失时不停止消费行 | `LoadedNeedsDeps` | `a` 失败并卸载，`s` 从表里清掉；`b` 仍 `loaded`，它的依赖已不在 |
| `row-mut-refusal-sticky` | 拒绝记录不按代清掉 | `RefusalOnce` | `b` 撞名被拒；第一次重试失败（计数 1），记录留着，第二次重试又失败（计数 2） |
| `row-mut-refusal-scope` | 拒绝在等依赖的重启之后仍留着 | `RefusalScope` | `b` 撞名被拒 → 它的重启在等 `d`（`HWaitRefusal`，`waited = 1`）→ `d` 回来以后 apply 又因同一个拒绝失败（`failed = 1`） |
| `row-mut-ended-during-load` | 装载中结束的行当场报结束 | `EndingAfterLoad` | `EndDuringLoad` 把键放进 `pendingEnding`，`SendEnding` 不等 `loaded` 回复：`endedSent` 有它，`replied` 还没有 |
| `row-mut-update-reload` | 易变字段的更新卸载再装载 | `UpdateInPlace` | 更新开始（`updated` 计数 1）之后行进入 `unloading`，不再 `loaded` |
| `row-mut-no-refresh-redo` | `RefreshBegin` 不接受一个被取消的刷新 | 活性（`RefreshRedone`） | 刷新进行中运行时崩溃（`refresh = cancelled`）；重启之后刷新不再开始，`stage` 停在 `up`，行永远不 Running |
| `row-mut-mutual-wait` | `Answer` 额外要求其他运行时 `stage = rows` 且全部会话 swept | 活性（`RuntimesDoNotWait`、`ColdStartLoads`） | `r1` 崩溃且不再启动，`r2` 的回答永远等不到 `r1` 就绪/清理，`r2` 的行永远不 Running |

`row-mut-accept-before-clean` 与 `row-mut-read-old` 也分别会让 `OneRowPerName` 或 `NewSessionOnlyAfterSweep` 失败；分开列配置是为了让每个反例只对应一个结论（同 `event-close` 的做法）。

## 结论

- 原有结论不变：按远程插件 §4.4 的两条规则（顶替时不再读旧会话、清理完旧会话的行才回答新 `hello`），在穷举范围内不出现同一个名字的两代行；旧会话剩下的帧不会在清理之后再装载行；断线后它的行被清理；宿主按键处置结束与失败，一行只被清理一次。
- 服务块在穷举范围内成立：行只在两阶段完成且解析 final 之后装载；装载期间的服务公告在 `loaded` 回复之前处理，宿主不会把还欠公告的行标成 Running；两个行不会同时以同一个名字 Running，第二个被拒；拒绝只让引起它的那一次重启失败，等依赖的重启不算（d94aae9）；宿主服务租约只属于当前代、在卸载/失败回复之后释放，`hosts.provide`/`withdraw` 的计数与注册一致；运行时的旧行在崩溃时只清理一次并能在重启后重新装载；服务消失时消费行同步停下；一个运行时崩溃且不再启动时，另一个运行时的行照常 Running；两个运行时同时冷启动时各自完成。
- 版本与后续动作在穷举范围内成立：宿主的表始终是它应用过的最新版本，迟到的旧公告不覆盖更新的（`NewerWins`）；行卸载完成之后它的名字不再留在宿主上，发布中途的撤下不会丢（`WithdrawBeforeUnload`，即 #372）；装载中结束的行等装载回复之后才报结束（54a7e824）；被运行时重启打断的刷新会被重做（REM-4.3-3）；`rows.update` 提交在正在跑的那一代上（SYN-4-8）。
- 仍未覆盖：见“简化”最后一条——事件与 `rows.ended` 的帧序（#613）、调用生命周期与跨进程同步调用链、节点托管行、各语言 apply 取消与卸载栈的差异、`rows.schema`/`mount` 的形状协商、同一名字第二个租约不重新 `hosts.provide` 时的形状刷新、`runtimes_ready`（#314）、带 inject 的行的 apply 失败（#584）、#247 / #191 的具体情形，以及真实计时。本目录不能当作完整行协议的证据。
- 模型里“服务公告在同步调用链内到达”表现为同一会话的帧序（公告先于 `loaded`），不是真正的跨进程调用链；真正的同步调用顺序在 `cross-runtime-sync` 和后续模型里。

## 怎么运行

需要 Java 11 以上和 `tla2tools.jar`（[v1.7.4](https://github.com/tlaplus/tlaplus/releases/tag/v1.7.4)，sha256 `936a262061c914694dfd669a543be24573c45d5aa0ff20a8b96b23d01e050e88`）。jar 不要提交进仓库。

```sh
curl -sSLo /tmp/tla2tools.jar https://github.com/tlaplus/tlaplus/releases/download/v1.7.4/tla2tools.jar
TLA2TOOLS=/tmp/tla2tools.jar specs/row-lifecycle/run.sh                    # 全部配置
TLA2TOOLS=/tmp/tla2tools.jar specs/row-lifecycle/run.sh row-runtimes       # 单个配置
TLC_WORKERS=4 TLA2TOOLS=/tmp/tla2tools.jar specs/row-lifecycle/run.sh      # 限定 TLC 的线程数
```

`run.sh` 把每个配置的结果和 `expected.txt` 比较，全部符合时退出码为 0；日志和反例写在 `TLC_OUT`（默认一个临时目录）。CI 的 `specs` 任务矩阵包含 `row-lifecycle`（见 [CI](../../docs/ci.md) §4）。

## 维护

远程会话 §4.2、§4.3、远程插件 §4.4、多语言运行时 §四、§五、§六 修改时，或 `crates/rutis-loader/src/runtime.rs` 的行键/`Refusals`/租约/投影、`crates/rutis-bridge/src/runtime/process.rs` 的 `rows.ended`/`rows.failed`/`unload_row`/服务表与 `Imports::update` 的版本规则、`crates/rutis-bridge/src/runtime/projection.rs` 的 `publish` / `withdraw`、运行时监听端的顶替顺序与两阶段、`node/rutis-runtime/src/runner.mjs` 的 `endRow`/`unloadRow`/`update` 改变时，同步修改这个模型和 `expected.txt`（规范 Q11.2.3）。新增特性时先加常量（默认关闭），保持原有配置的语义和状态数不变。
