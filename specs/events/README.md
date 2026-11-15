# 事件协议：宿主规则、监听器表、emit、节点连接（TLA+）

[English](README.en.md)

这套模型对应 issue #613 的 `specs/events/`：把[统一事件的协议规格](../../docs/design-events-protocol-2026-10-18.md)第四节（登记）、第五节（分发）、第六节（过滤）、第七节（生命周期）和第九节（节点连接）按参与者写成可以穷举交错的有限模型。设计背景见[统一事件](../../docs/design-events-2026-10-18.md)与[形式化验证重新设计](../../docs/design-formal-verification-2026-11-08.md) §3.2。

四个模块共用一个目录：

| 模块 | 内容 | 参与者和规模 | 配置 |
| --- | --- | --- | --- |
| `EventsHost.tla` | 只有定义，没有状态：名字/前缀匹配、登记顺序、elsewhere、过滤器、五种方式的开始规则、`export` / `import` 与收窄、公告 id 的 epoch 与保留规则 | 无 | `host-defs` 用 `EventsHostCheck.tla` 把规格里的例子算一遍 |
| `EventsTable.tla` | 登记、撤下、elsewhere、快速路径与经宿主路径、`rows.load` 回复、行卸载/结束/重装、会话结束与重开、宿主前缀条目、跨进程调用的因果 | 宿主 + 1–2 个运行时；`table-multi` 是 2 个运行时 × 2 行；健康配置最多 1 次分发 | 19 |
| `EventsEmit.tla` | 宿主按会话排队的 emit（每项记住自己的行）、扣住的调用、就地读回复、过滤器迟到、行卸载、两个行的三个 emit、`R2a`/`R2b`、推迟到下一轮开始的调用（`python-deferred`）、调用未开始就回复的宿主（`before-future-reply`）、死锁检查 | 宿主 + `R1`（行 A/B） + `R2`；三次 emit | 10 |
| `EventsLink.tla` | `link.offers` 快照、版本只保留最高、`<epoch>.<n>` 公告、宣告早于提供、接收三步、`events.off` 与 `link.offers` 的独立交错、bare-id 的当前规则、对端不转发从下一跳引进的条目 | 宿主 + 一个对端（另有它从下一跳引进的一个条目）；两个自有条目、两个族、两次提供 | 7 |

`EventsHost.tla` 不是一份可执行的规格：三个有状态的模块 `EXTENDS` 它，各自的配置只用到其中一部分。`host-defs` 的检查方式是 `ASSUME`：规格第四、五、六、九节的例子必须算得出文档写的答案，然后在唯一的状态里停住。它不检查那些定义在任意输入上都成立，只检查它们和规格写的例子一致。

## 对应的设计和代码

| 模型部分 | 设计 | 实现 |
| --- | --- | --- |
| 宿主表、位置、`prepend`、前缀、存活检查 | §4.2、§4.3、§5.1 | `crates/rutis/src/bus/table.rs`（`select`、`insert`、`Entry::start`） |
| 快照、过滤、五种方式、emit 的尾巴与 backlog | §5.1、§5.4、§5.5、§6 | `crates/rutis/src/bus.rs`（`emit_erased`、`snapshot`、`start`）、`crates/rutis/src/bus/run.rs` |
| 会话的 `events.on` / `events.off` / `events.dispatch`、行撤下、`events.on` 回复与 elsewhere 的先后、emit 队列 | §4.4、§4.5、§5.3、§7.1 | `crates/rutis-bridge/src/events/hub.rs`（`SessionEvents::on`、`off`、`dispatch`、`run_emit`、`next_emit`、`row_unloading`、`row_ended`、`let_go_of_its_emits`） |
| 帧序（`events.*`、`rows.ended`） | §4.3、§7.1 | `crates/rutis-bridge/src/session/rpc.rs` |
| 跨进程调用的因果（§4.4 第 3 条） | §4.4 | 登记路径（`crates/rutis-bridge/src/events/hub.rs` 的 `SessionEvents::on`）与回复/elsewhere 的先后；调用帧经会话转发 |
| 行的结束、卸载与重装 | §7.1 | `crates/rutis-bridge/src/events/hub.rs` 的 `row_unloading` / `row_ended`；重装的监听器以新的登记 id 进表，模型用 `MaxLoads` 区分同一行的两次登记 |
| 节点连接、接受、撤下、不转发 | §9 | `crates/rutis-bridge/src/events/link.rs`（`accept`、`announced_under`、`peer_offer`、`offer_now`、`older_than`） |
| `link.offers` 的版本与 `since` | §9 | `crates/rutis-bridge/src/peer.rs`（`Offers`、`next_offers`、`send_if_modified`） |

## 简化

所有模块共有的：

- 只有一个名字（`EventsTable`、`EventsEmit`）或两个条目（`EventsLink`）；`EventsTable` 的宿主前缀条目用固定的 `"session/"` 前缀，其他条目按精确名匹配。配置里没有多个名字或更多前缀。
- 不建作用域（它只是描述）、值、错误内容、`AggregateError` 的聚合、取消、超时、`next` 只能调一次的规则。
- 每个方向的消息按顺序送达、延迟任意；部分模块把“顺序”进一步缩小成两边各一条 FIFO 队列。
- 时间是“到过什么”的幽灵集合，不是时钟：同一个状态不会因为晚一步到达而不同。

`EventsTable.tla`：

- `table-multi` 把 2 个运行时 × 2 行放进同一个配置，但只允许 1 次分发、队列深度 1；`table-order` 仍是 1 个运行时 × 2 行检查 `OneOrder`。
- 跨进程调用只建一条 `call` 帧：调用方在收到 `events.on` 回复后发出，帧带着“调用方已经收到回复的条目集合”；接收方读到这条帧后，`CausallyAfterCallSees` 才把集合里的条目算作必须看见。没有建真实的服务调用链、`path` 或同步等待。
- `events.on` 的回复本身只记在 `repliedTo`（幽灵）里；`rows.load` 的回复仍只数“已发出/已接受”的登记数，不建回复内容。
- 前缀条目只来自宿主自己的 Rust 监听器；运行时从不登记前缀。`HostOn` 只接受等于这一行当前登记的条目，所以行卸载/重装后迟到的旧 `events.on` 不会进表（§7.1）。
- 一个运行时的一行在每个会话里最多登记 `MaxLoads` 次；`table-reload` 用 `MaxLoads=2` 覆盖“结束/卸载后重新装载”。重装要求旧条目已经出了宿主表（旧 `rows.ended` 或卸载已处理），所以同一行的两次登记不会同时在表里。`rows.update` 的就地重排不建。
- 会话结束（`EndBeforeRestart`）先撤下这个会话的条目、丢掉它的帧；已经取过快照的分发在 `HostStart` 检查条目仍在表里，因此不会为旧会话开始调用。重启仍要求所有快照都已走完。
- `events.off`、`rows.ended` 的帧内容只表示“哪一条”，不建未知 id 的回复、错误或批量撤下。
- 过滤器不建模（配置里没有 `filter`）；`emit` 的方式只表现为“调用按顺序开始”，没有 `serial` / `parallel` / `waterfall`。

`EventsEmit.tla`：

- 场景是：`R1` 的行 A 在同步等待中经宿主发 `E1`，`E1` 在 `L1` 的调用被扣住；`R1` 就地读回复后，等待链发 `E2`，行 B 发 `E3`。宿主按会话一条队列，一次只取一项；队列项记住自己的行，所以卸载行 A 只丢掉 A 的项。
- 三个 emit 的过滤器都只建经宿主的路径；快速路径上的过滤、每个 emit 多个过滤器、`serial` 等其他方式不建。
- `R2a` / `R2b` 用“`L2` 上正在跑的调用集合”区分：`R2a` 允许交叠，`R2b` 要等同一监听器的调用结束。两个配置状态数不同，说明区别确实建了；调用结束是任意一步，不是真实调度。
- 行卸载丢弃它排队的 emit、放开正在等过滤器的 emit、丢掉已经扣住的调用；A 卸载后 `L1` 不再开始任何调用。`unloadQueue` 变异保留它们（#536）。
- 死锁检查：健康配置 `CHECK_DEADLOCK TRUE`，`Done` 只在没有排队、没有过滤器/调用在跑、没有被扣住的调用时停住。`never` 过滤器不在这些配置里——它按协议就是永远挂着，不是要检查的死锁。
- 不建 panic、任务调度、真实 Promise、Node/Bun/Python 的帧泵差别，也不建 `emit` 的失败报告。
- `python-deferred`（`Defer = "nextRound"`）：Python 把收到的 invoke 帧（含 `events.call`）交给 `loop.call_soon`（`python/rutis/rutis/peer.py:1886`），只有同步等待中的帧就地处理（`_run`，同文件 1892–1896）；`call_soon` 先进先出。模型里 `R2` 的调用先进 `deferred`，只从队头开始，所以调用比读到帧晚，同一监听器上的先后不变。配置 `emit-python-deferred` 通过：延迟只推后 `StartCall`，模型本来就允许它任意晚，性质不因此改变。它没有建的是“宿主把这次调用算作已开始”和“监听器函数体开始”两步的分离（现在 `StartCall` 同时是这两件事），也没有 EVP-4.1-2“帧就地处理、处理完再读下一帧”这条性质，所以它不能验证那条规则。Python 也没有快速路径（`python/rutis/rutis/runner.py` 的 `send` 总是发 `events.dispatch`，`python/rutis/rutis/` 里没有处理 `events.elsewhere` 的代码）。
- `before-future-reply`（`Mutation = "futureReply"`）：c2c1b7e8 之前的宿主一半（#513）：宿主对一个还在排队、调用尚未开始的 emit 就写出回复，运行时就地读到它，这次 emit 的名字在发出方看来已经结束。变异用两步建这件事：`ReplyEarly` 在 `phase` 还是 `queued`/`filtering`/`starting` 时记下已读回复；`PendingAt` 不再把这种 emit 算作“更早的调用还没开始”，后来的同名 emit 走快速路径时不再排在它后面。`emit-mut-future-reply` 违反 `EmitOrder`。

`EventsLink.tla`：

- 一个对端、它的两个自有条目、两个族、两次提供；`events.on` / `events.off` 在一条按序的 `wire` 上，`link.offers` 快照是另一列，可以排在任何位置（`peer.rs` 的 `announce` 在任务里发）。这正是 #593 的时序来源。
- 提供边界本身就是撤下：对端重新提供 `events` 时不发每个旧条目的 `events.off`，旧条目靠 epoch 撤下（`link.rs` 的 `offered()`）；单个条目的撤下则用 `events.off`，两种消息可以和快照任意交错。
- `PeerIds = "bare"` 是另一种实现的**当前规则**：id 不带 epoch，接收方只按 id 替换。`link-bare-id` 是健康规则下的配置，不是变异；它预期违反 `CurrentOffer`，因为这条规则本来就无法保证旧提供下不再宣告的条目会消失（EVP-9-17）。变异 `bareIds` 不再单独存在。
- 多跳只建到“对端从下一跳引进了一个条目”：`link-multihop` 让这个条目存在但不对宿主宣告，`NoForward` 成立；`forward` 变异让对端把它宣告出去。没有建完整的三节点消息链、多个对端或转发表的合并。
- `export` / `import` 的收窄在 `EventsHost.tla` 的定义里，由 `host-defs` 检查；有状态的 `EventsLink` 只建一个对端方向，没有建双向共享、服务项或前缀项当边界。
- 三个接受步骤只在 `acceptUnlocked` 变异里拆开；基础模型的查代次、登记、插入是一步。
- 死锁检查：健康、bare-id 和三个变异配置都 `CHECK_DEADLOCK TRUE`；`Done` 只在连接安静、没有待发消息时停住。`forward` 变异为了尽快找反例关闭死锁检查。

## 性质

| 名字 | 模块 | 内容 | 对应保证 |
| --- | --- | --- | --- |
| `RegisteredThenSeen` | Table | 分发方在发出时知道的登记，到取快照时没有在任何地方被撤下，快照就包含它（§4.4 第 1、3 条） | `events/presence`、`events/hub-dispatch` |
| `CausallyAfterCallSees` | Table | 一次分发由跨进程调用引起时，调用方在发出调用前已经收到回复的登记都在快照里（§4.4 第 3 条） | 随模型补登记；`events/hub-dispatch` |
| `WithdrawnStaysOut` | Table | 分发方在发出时已知撤下的条目，这次分发不为它开始调用（§4.4 第 4 条、§7.1） | `events/hub-lifecycle`、`events/lifecycle` |
| `OneOrder` | Table | 两次分发都开始的两个条目，先后一致（§4.3、§5.4） | `events/order`、`events/one-bus` |
| `OneCallPerEntry` | Table | 每个（分发，条目）最多一个调用（`SettlesOnce` 的一半） | `events/hub-lifecycle` |
| `RunningRowHeard` | Table | 一行被记为 Running 时，它 apply 期间登记的条目都已被接受（§4.4 第 2 条） | 登记表没有对应条目；随模型补登记 |
| `NoDispatchBeforeElsewhere` | Table | 新会话没有应用 elsewhere 全集就不分发（§4.5） | — |
| `PrefixEntrySeen` | Table | 取快照时宿主表里有匹配的前缀条目，宿主快照就包含它（§4.3、§5.1） | `events/presence` |
| `SeesAll` | Table | 每次分发看到发出前已登记、发出时未撤下的全部条目；协议**不**承诺这一点，配置预期被违反 | `events/one-bus`（反例写进本文件） |
| `EmitOrder` | Emit | 同一发出方、同一监听器上，`emit` 的调用按发出顺序开始（§5.5） | `events/emit`、`events/emit-order` |
| `FastPathByFrame` | Emit | 读到经宿主的 emit 的回复之后，这次 emit 不再让同名的后续 emit 留在经宿主路径上（§5.5 最后一条） | `events/one-bus`（`emit/order-across-fast-path`） |
| `UnloadedRowEmitsNothing` | Emit | 一行卸载之后，它排队或正在等过滤器的 emit 不开始任何调用（§5.5、§7.1） | 登记表没有对应条目；随模型补登记 |
| `NoCallToUnloadedListener` | Emit | 一行卸载之后，它拥有的监听器不再开始任何调用（§7.1） | 随模型补登记 |
| `CurrentOffer` | Link | 连接上和在发的消息都空了以后，宿主表里来自对端的条目恰好是对端当前提供下宣告、仍然存活的条目（§9） | `events/node-links` |
| `NoEcho` | Link | 宿主不把从对端引进的条目宣告回去（一跳） | `events/no-multi-hop` |
| `NoForward` | Link | 对端不把它从下一跳引进的条目宣告给宿主（多跳） | `events/no-multi-hop` |

## 结果

TLC 2.19（tla2tools v1.7.4），Temurin 21.0.12，32 核，`-workers 4`，2026-11-10；`table-mut-on-late`、`table-mut-dup-local`、`emit-python-deferred`、`emit-mut-future-reply` 是 2026-11-16 在同一台机器上按同样参数测的。`通过`指穷举完成、所列性质全部成立；被违反的配置找到第一个反例就停，状态数取决于搜索顺序。整个目录约 2 分钟。下面每个配置都在本机跑过；没有跑的是更大的参数组合（例如 `table-multi` 的 `MaxDispatch>1`、`MaxLoads>2`，或 `EventsLink` 的 `MaxEpoch>2`）。

| 配置 | 内容 | 结果 | 不同状态数 | 时间 |
| --- | --- | --- | --- | --- |
| `host-defs` | 定义与规格例子一致 | 通过 | 1 | <1 s |
| `table-healthy` | 2 个运行时 × 1 行，宿主自己的监听器，撤下、行卸载，最多 1 次分发 | 通过 | 966,166 | 17 s |
| `table-multi` | 2 个运行时 × 2 行，同一个配置，1 次分发、队列深度 1 | 通过 | 960,076 | 13 s |
| `table-causal` | 跨进程调用，回包后接收方分发；因果性质 | 通过 | 150,908 | 6 s |
| `table-prefix` | 宿主前缀条目、`SnapshotOf` 的匹配 | 通过 | 87,665 | 3 s |
| `table-reload` | 1 个运行时 × 2 行，行可结束、卸载、重装（两次登记） | 通过 | 81,054 | 2 s |
| `table-end-session` | 会话先结束再重开；结束时的在途快照 | 通过 | 566,207 | 12 s |
| `table-sees-all` | 同 `table-healthy`，检查 `SeesAll` | 违反 `SeesAll` | 134（反例即停） | <1 s |
| `table-ended` | 运行时报告 `rows.ended` | 通过 | 199,102 | 5 s |
| `table-restart` | 会话结束、新会话先收 elsewhere 全集 | 通过 | 673,801 | 8 s |
| `table-order` | 1 个运行时 × 2 行、`prepend`，两次分发（快速路径 + 经宿主） | 通过 | 22,081 | 1 s |
| `table-mut-local-order` | 运行时的本地顺序忽略 `prepend` | 违反 `OneOrder` | 24,670 | 1 s |
| `table-mut-out-of-order` | 宿主不按帧序处理一个会话的帧（#587 之前） | 违反 `WithdrawnStaysOut` | 4,537 | 1 s |
| `table-mut-stale-fast` | 快速路径的快照与存活检查读宿主的表，不读自己的登记 | 违反 `WithdrawnStaysOut` | 275 | <1 s |
| `table-mut-load-reply` | `rows.load` 在登记被接受之前就回复 | 违反 `RunningRowHeard` | 51 | <1 s |
| `table-mut-fresh` | 新会话没有 elsewhere 全集就可以分发 | 违反 `NoDispatchBeforeElsewhere` | 2 | <1 s |
| `table-mut-causal` | 宿主在 elsewhere 排进其他会话之前就回复 `events.on` | 违反 `CausallyAfterCallSees` | 2,476 | 1 s |
| `table-mut-prefix` | 宿主快照只按精确名匹配，丢掉前缀条目 | 违反 `PrefixEntrySeen` | 87 | <1 s |
| `table-mut-on-late` | 宿主把 `events.dispatch` 单独处理：先取它，后取同一会话更早发出的 `events.on` | 违反 `RegisteredThenSeen` | 95（反例即停） | 1 s |
| `table-mut-dup-local` | 运行时的顺序表把同一个监听器的两次登记都收下 | 违反 `OneCallPerEntry` | 89（反例即停） | 1 s |
| `emit-r2a` | 扣住的调用、就地读回复、迟到的过滤器；`R2` 规则 a；死锁检查 | 通过 | 34,857 | 2 s |
| `emit-r2b` | 同上，`R2` 规则 b；死锁检查 | 通过 | 24,893 | 2 s |
| `emit-multi` | 两个行、三个 emit、宿主按会话排队、行 A/B 都可卸载；规则 a | 通过 | 127,576 | 4 s |
| `emit-multi-r2b` | 同 `emit-multi`，规则 b | 通过 | 91,688 | 3 s |
| `emit-python-deferred` | 同 `emit-multi`，`R2` 的调用推到下一轮开始（`Defer = "nextRound"`，FIFO）；死锁检查 | 通过 | 141,978 | 2 s |
| `emit-mut-held` | 快速路径上同名的 emit 抢在被扣住的调用前面（#557 之前） | 违反 `EmitOrder` | 701 | 1 s |
| `emit-mut-reply-async` | 读到 emit 回复后过一步才记为已回复（#513 之前） | 违反 `FastPathByFrame` | 318 | 1 s |
| `emit-mut-future-reply` | 宿主对还在排队、调用尚未开始的 emit 就回复（c2c1b7e8 之前的宿主一半，#513） | 违反 `EmitOrder` | 526（反例即停） | 1 s |
| `emit-mut-unload` | 行卸载时不丢弃排队的 emit（#536 之前） | 违反 `UnloadedRowEmitsNothing` | 428 | 1 s |
| `emit-mut-dead-listener` | 行卸载后仍调用它拥有的监听器 | 违反 `NoCallToUnloadedListener` | 159 | 1 s |
| `link-healthy` | 两次提供、宣告早于提供、另一个族引起的快照；死锁检查 | 通过 | 293,064 | 5 s |
| `link-multihop` | 对端有下一跳引进的条目，但不向宿主转发 | 通过 | 586,035 | 8 s |
| `link-bare-id` | `PeerIds = "bare"`：当前规则只按 id 替换；`CurrentOffer` 本就不保证 | 违反 `CurrentOffer`（预期） | 3,263 | 1 s |
| `link-mut-accept-unlocked` | 接受的查代次与插入之间解锁（修复前 `link.rs` `accept` 的行为；当前实现已在同一把锁内重检） | 违反 `CurrentOffer` | 6,748 | 2 s |
| `link-mut-offer-race` | 没有 `events` 的快照撤下全部条目（#593 之前） | 违反 `CurrentOffer` | 11,923 | 2 s |
| `link-mut-echo` | 宿主把引进的条目再宣告回去 | 违反 `NoEcho` | 677 | 1 s |
| `link-mut-forward` | 对端把下一跳引进的条目转发给宿主 | 违反 `NoForward` | 152 | 1 s |

## 反例

每个被检查的性质都有一个故意引入的错误（`expected.txt` 的 `violated:<名字>`），包括 `table-mut-on-late` 的 `RegisteredThenSeen` 和 `table-mut-dup-local` 的 `OneCallPerEntry`。除了变异，`table-sees-all` 违反的是协议**明确不承诺**的性质，`link-bare-id` 违反的是“不带 epoch 的 id 只按 id 替换”这条当前规则本身的后果：

- **`table-sees-all`**：`r1` 登记了监听器、`events.on` 已经发出，宿主也接受进表；`r2` 的 elsewhere 还没更新，它走快速路径，只看到自己的条目。于是“每次分发看到发出前登记的全部条目”不成立。这就是 issue #613 开头“协议不承诺顺序一致”的例子，`SeesAll` 不是承诺，`RegisteredThenSeen` 和 `WithdrawnStaysOut` 才是。
- **`table-mut-out-of-order`**：`r1` 先发 `events.on`、再发 `off`、再发 `events.dispatch`；宿主先取 `events.dispatch`，快照里有这个条目，随后（仍在处理 `off` 之前）为它开始调用。`r1` 在发出这次分发时已经知道自己撤下了它，`WithdrawnStaysOut` 不成立。这对应 #587 之前的帧序问题。
- **`table-mut-stale-fast`**：`r1` 发出 `off` 后，宿主还没处理；快速路径从宿主已接受的表取快照，存活检查也问那张表，于是为一个它已经撤下的条目开始了调用。
- **`table-mut-local-order`**：本地顺序忽略 `prepend`：快速路径按到达顺序，经宿主的分发按位置顺序，两次分发对同两个条目的先后不同，`OneOrder` 不成立。
- **`table-mut-load-reply`**：`rows.load` 在 `events.on` 被接受之前回复；`RunningRowHeard` 不成立。
- **`table-mut-fresh`**：新会话还没收到 elsewhere 全集就分发；`NoDispatchBeforeElsewhere` 不成立。
- **`table-mut-causal`**：`r1` 的监听器登记被接受，宿主在把 elsewhere 排进 `r2` 之前就回复了 `events.on`；`r1` 随后调用 `r2` 的服务，`r2` 在读到调用后分发，快速路径的 elsewhere 仍是旧的，看不到 `r1` 的条目。`CausallyAfterCallSees` 不成立。基准模型里宿主先排 elsewhere、再回复，所以这条路径在 `table-causal` 里不出现。
- **`table-mut-prefix`**：宿主快照只按精确名匹配；表里明明有匹配 `session/created` 的前缀条目，快照里却没有它，`PrefixEntrySeen` 不成立。基准模型用 `EntryMatches` 保留前缀条目。
- **`table-mut-on-late`**：宿主把 `events.dispatch` 当成自己的任务处理，不等同一会话先发出的帧。`r1` 先发出 `events.on`（`<<r1,a,1,1>>`）、再发出 `events.dispatch`；宿主先取后者、从表里取快照，表里还没有这条登记，只有宿主自己的 `<<host,host,0,0>>`。`r1` 在发出这次分发时已经知道自己的登记，到取快照时也没有在任何地方被撤下（`Withdraw`、`Unload` 都关，`everOff` 一直是空集），`RegisteredThenSeen` 不成立。§4.4 第 1 条要求快照包含发出方已知的登记，EVP-4.1-3 要求宿主按帧序处理同一会话的事件操作；这个变异让 `events.dispatch` 插到前面那帧之前。
- **`table-mut-dup-local`**：同一行在同一会话里把同一个监听器登记两次，运行时的顺序表（`local`）收下同一条 `<<r1,a,1,1>>` 的两份（`Register` 不再检查它已在列表里）；快速路径按这份列表开始调用，两次都是它，`OneCallPerEntry`（`NoDup(order[d])`）不成立。反例里 `tbl` 是空的：第一条 `events.on` 还没到宿主，运行时已经开始了两次调用。宿主一侧本来就不重复收同一条（`HostOn` 的 `kept` 要求条目还不在表里，重复 id 也被拒，EVP-4.2-1），所以这个变异拿掉的是“一个监听器只登记一次”这个前提，检查的是分发一侧的性质本身。
- **`emit-mut-held`**：`E1` 在 `L1` 的调用被等待扣住；`R1` 读到回复后，快速路径上的 `E2` 立刻开始 `L1` 的调用；等待结束后 `E1` 的调用才跑。同一个发出方的两次 emit，在 `L1` 上 `E2` 先开始，`EmitOrder` 不成立（#557 之前）。
- **`emit-mut-reply-async`**：`R1` 已经读到 `E1` 的回复帧，但过一步才记为已回复；等待链在这之间发出 `E2`，运行时仍把它送去宿主。`FastPathByFrame` 不成立（#513 之前）。
- **`emit-mut-future-reply`**：行 A 的 `E1` 经宿主发出、还在宿主队列里，它的调用一个都还没开始；宿主照样写出回复，`R1` 就地读到（`read[E1]`）。行 B 的 `E3` 因此走快速路径，`L1` 的调用立刻开始；随后宿主才取 `E1`，`E1` 的 `L1` 调用排在 `E3` 之后。同一监听器上后来的 emit 先开始，`EmitOrder` 不成立（#513 之前宿主那一半）。
- **`emit-mut-unload`**：行卸载时不丢弃排队的 emit；过滤器之后回答，`E1`/`E2` 仍开始调用，`UnloadedRowEmitsNothing` 不成立（#536 之前）。
- **`emit-mut-dead-listener`**：行 A 已经卸载；行 B 的 emit 仍把调用送到 A 留下的 `L1`，`NoCallToUnloadedListener` 不成立。
- **`link-bare-id`**：`PeerIds = "bare"` 是对端实现的当前规则：公告 id 不带 epoch，宿主只按 id 替换；对端重新提供后，旧提供下宣告、新提供不再有的条目留在表里，`CurrentOffer` 不成立（EVP-9-17）。这不是实现缺陷，而是这条规则不承诺 `CurrentOffer`；带 epoch 的配置和变异分开检查。
- **`link-mut-accept-unlocked`**：接受 `<<1,2>>` 的查代次做完、插入之前，宿主处理了版本更高的、`events: 2` 的快照（撤下旧提供）；插入随后把 `<<1,2>>` 放回表里。连接安静下来后，表里多出一个对端当前提供没有宣告的条目，`CurrentOffer` 不成立。修复前的 `link.rs` `accept()` 就是这样：查代次在锁内，登记条目在锁外，插入在另一次锁内。当前实现用 `gone` 边界在插入的同一把锁内重检，并有 `an_entry_of_an_offer_gone_while_it_registered_is_not_taken` 回归测试；变异配置仍按修复前行为构造，用来守住这个窗口。
- **`link-mut-offer-race`**：一个没有 `events` 的快照撤下全部条目；一个先于它的提供到达的宣告被这次撤下带走，即使它的提供还没被列出（#593 之前）。
- **`link-mut-echo`**：宿主把从对端接受的条目宣告回去，`NoEcho` 不成立。
- **`link-mut-forward`**：对端把从下一跳引进的条目宣告给宿主，`NoForward` 不成立；健康配置里对端只是持有这个条目，不宣告。

## 结论

- 按当前规格，`EventsHost.tla` 的定义与规格例子一致；`EventsTable.tla` 的安全性质在穷举范围内成立：同一进程里先登记后发出（两条路径）、跨进程调用后接收方看到调用方已收到回复的登记、撤下后不再开始调用、同一对条目处处同一先后、`rows.load` 回复前登记已被接受、新会话必须先后收到 elsewhere 全集、宿主快照包含匹配的前缀条目。
- `table-multi` 把 2 个运行时 × 2 行放进同一个配置跑通；`table-reload` 覆盖行结束/卸载/重装（两次登记）的交错；`table-end-session` 覆盖会话先结束再重开时仍在途的快照。行卸载后迟到的旧 `events.on` 由“只接受这一行当前登记”挡住。
- `EventsEmit.tla` 的两个顺序性质、行卸载性质和监听器性质在它的场景里成立；宿主按会话的队列会丢弃卸载行的项而保留其他行的项；`R2a` 与 `R2b` 的状态数不同，交叠执行确实建了；五个变异都被找到反例。`emit-python-deferred` 把 `R2` 的调用推到下一轮开始（FIFO），全部通过：延迟只推后开始，模型本来就允许任意晚，所以按现在的实现这条疑点（EVP-5.5-9）在模型里不成立；它没有建的“宿主算作开始”与“函数体开始”两步分离见“简化”。`emit-mut-future-reply` 用“调用未开始就回复”的宿主找到了 `EmitOrder` 的反例：c2c1b7e8 的宿主一半不是空的。
- `EventsLink.tla`：只保留最高版本的快照、旧提供的条目按 epoch 撤下、先于提供到达的宣告保留、单个 `events.off` 与快照独立交错，这些在当前规则下成立；不带 epoch 的 id（EVP-9-17）作为当前规则有一个预期反例，修复前 `accept()` 的解锁窗口、`offerRace`、`echo` 和 `forward` 各有一个反例；`NoForward` 覆盖“对端不转发下一跳引进的条目”。
- 模型没有证明的（按 issue #613 的范围）：作用域、值、错误、取消、`rows.update` 的就地重排、快速路径上的过滤器、五种方式的完整语义（只有 `emit`）、真实的任务调度与帧泵差别、完整的多跳消息链、多个对端、`export` / `import` 在状态模型里的双向收窄、服务项共享、`python-deferred` 里“宿主把这次调用算作已开始”与“监听器函数体开始”两步的分离（EVP-4.1-2 的“就地处理、处理完再读下一帧”也不建）、`before-future-reply` 只建了宿主一半，以及整个实现的活性/死锁（本目录的 `CHECK_DEADLOCK TRUE` 只回答“这个有限协议状态机在这些配置里没有意外的终态”，不是实现没有死锁的证据）。不能把这个目录当成 #613 的完整证据。

## 怎么运行

需要 Java 11 以上和 `tla2tools.jar`（[v1.7.4](https://github.com/tlaplus/tlaplus/releases/tag/v1.7.4)，sha256 `936a262061c914694dfd669a543be24573c45d5aa0ff20a8b96b23d01e050e88`）。jar 不要提交进仓库。

```sh
curl -sSLo /tmp/tla2tools.jar https://github.com/tlaplus/tlaplus/releases/download/v1.7.4/tla2tools.jar
TLA2TOOLS=/tmp/tla2tools.jar specs/events/run.sh                 # 全部配置
TLA2TOOLS=/tmp/tla2tools.jar specs/events/run.sh table-healthy    # 一个配置
TLC_WORKERS=2 TLA2TOOLS=/tmp/tla2tools.jar specs/events/run.sh    # 限定线程数
```

`run.sh` 按配置名的前缀选模块（`host-defs` → `EventsHostCheck.tla`，`table-*` → `EventsTable.tla`，`emit-*` → `EventsEmit.tla`，`link-*` → `EventsLink.tla`），把结果和 `expected.txt` 比较，全部符合时退出码为 0；日志和反例写在 `TLC_OUT`（默认临时目录）。CI 的 `specs` 任务矩阵已包含 `events`（[CI](../../docs/ci.md) §4）。

## 维护

协议规格第四、五、六、七、九节改了，或 `bus/table.rs` 的 `select` / `Entry::start`、`bus.rs` 的 `emit_erased` / 快照、`hub.rs` 的 `events.on` 回复与 elsewhere 排队 / 行撤下 / emit 队列、`link.rs` 的 `accept` / 公告撤下 / 不转发、`peer.rs` 的 `Offers` 改了，就同步改这里和 `expected.txt`（质量规范 Q11.2.3）。新增常量（`MaxLoads`、`HostPrefix`、`Reload`、`EndBeforeRestart`、`AllowCrossCall`、`PeerIds`、`PeerImports`、`Defer`）要同时加进每个受影响的 `.cfg`。改了模型记得重跑全部配置：变异配置要仍然找到反例，健康配置要仍然通过；`CHECK_DEADLOCK TRUE` 的配置要确认 `Done` 只在真正没工作时停住。
