# 会话追踪契约：帧、握手与恢复的证据

[English](formal-trace.en.md)

状态：当前实现追踪基线；验证器、一致性和变异证据待补。日期：2026-11-08。
议题：[#635](https://github.com/arcships/rutis/issues/635)，属于 [#183](https://github.com/arcships/rutis/issues/183) 的协议基础部分。相关实现与验证工作：[#613](https://github.com/arcships/rutis/issues/613)、[#624](https://github.com/arcships/rutis/issues/624)、[#631](https://github.com/arcships/rutis/issues/631)、[#639](https://github.com/arcships/rutis/issues/639)。本文是当前面向可执行实现的追踪契约；它记录追踪验证器必须接受和必须拒绝的内容。

追踪是协议帧与通道结束观察组成的有序序列。通道追踪器不记录负载值：`crates/rutis-bridge/src/channel/trace.rs:1-3` 说明追踪只含方向、长度和结束信息，从不含消息内容（配置可能含秘密）。协议追踪验证器可以在测试夹具中解码帧字节，但生产通道追踪必须继续使用只记录大小的形式。

## 1. 范围与证据

会话是承载不透明消息的可靠、有序、双工流（`crates/rutis-bridge/src/channel/mod.rs:4-9`）。`Channel` 保留消息边界；会话协议为每条消息使用一个 JSON `op`，调用号带端点前缀。协议版本 5 是 `ENDPOINT_PROTOCOL`（`crates/rutis-bridge/src/session/mod.rs:9-15`），WebSocket 子协议也使用同一个值（`rutis.5`）。

协议帧词汇是 Rust 的 `Frame` 枚举（`crates/rutis-bridge/src/session/protocol.rs:154-222`）：`hello`、`invoke`、`call`、`get`、`await`、`return`、`throw`、`release` 和 `cancel`。线上值带标签（`undefined`、`data`、`list`、`record`、`signal`、`reference` 以及协商出的值标签）；未知字段按帧的作用范围处理（§3.7）。

下面每个追踪用例包含四部分：

1. **刺激** — 测试提供的精确帧或承载事件。
2. **接受的追踪** — 允许的精确顺序和状态转换。
3. **拒绝的追踪** — 第一个违规及其失败范围。
4. **开发者看到的内容** — 从实现测试或公开实现注释逐字复制的输出。

### 规则状态

状态词汇有意保持严格。**当前**表示本文件锚点所实现并测试的规则。**已取代**表示旧规则或旧追踪形状不能作为协议 5 的证据。**不作保证（Not guaranteed）**表示实现有意不作承诺；某个承载上可能出现某追踪，但用户和验证器不得依赖它。

| 类别 | 当前规则 | 已取代的证据 | 不作保证 |
| --- | --- | --- | --- |
| 会话帧 | §3.1 的协议 5 `Frame` 词汇及有序处理 | 协议 5 以前的帧名，或依赖负载的通道日志 | 超出通道顺序的具体调度交错 |
| 握手 | 双方 `hello`、版本 4、端点核对与能力声明 | 在 `Greeted` 挂载前就登记/挂载插件的握手 | `ready` 之前成功调用 |
| 引用 | 带标签的引用、home 身份、计数式 `release` | 把每个引用都当作复制数据的旧假设；按 `objects` 能力决定对象发给谁（0.18） | 引用生命周期使运行时进程保持存活（Not guaranteed） |
| 取消 | `cancel` 逐跳传播；迟到回复被丢弃 | 把取消当作关闭承载 | 调用截止时间（本文不作保证） |
| 错误 | `Failure` 图及调用/会话范围的编码 | 按错误文字匹配重试或行为 | 畸形输入的稳定、平台特定承载原因 |
| 限制 | 每次调用的值/消息拒绝和明确的出站积压上限 | 发送超大帧后继续调用 | `outbound_limit` 为 `None` 时积压无界 |
| 畸形输入 | 对畸形/悬空帧故障闭合；未知字段按调用范围失败（`hello` 失败握手，`release`/`cancel` 忽略），竞态中的未知 `cancel` 也除外 | 静默忽略未知字段或操作 | 任意新字段的向前兼容 |
| 网络/重连 | 按类型分类、退避和替换顺序 | 把不兼容/身份失败当普通暂时失败重试 | 启用抖动时精确的墙上时钟重连时间 |

追踪证据不能替代模型或实现证据。它回答的是一次真实执行是否属于当前允许的追踪语言。追踪通过不能证明未测试路径、无界状态空间或任意插件代码。

## 2. 追踪事件格式

稳定的通道级事件文字由 `trace()` 产生（`crates/rutis-bridge/src/channel/trace.rs:12-31,41-79`）：

```text
<label>: sent <n> bytes
<label>: received <n> bytes
<label>: ended
<label>: send failed: <reason>
<label>: receive failed: <reason>
<label>: closed: done
<label>: closed: replaced
<label>: closed: rejected: <reason>
```

测试逐字固定输出，并验证秘密不会出现（`crates/rutis-bridge/src/channel/trace.rs:104-133`）：

```text
memory peer mac: sent 18 bytes
memory peer mac: received 18 bytes
memory peer mac: ended
memory peer mac: closed: done
```

因此，验证器比较事件类型、方向、标签和字节长度，但生产追踪绝不能期待负载。`Close::Done`、`Close::Replaced` 和 `Close::Rejected(reason)` 在 `crates/rutis-bridge/src/channel/mod.rs:57-75` 有承载映射：WebSocket 关闭码分别为 1000、4002 和 4003；内存通道分别暴露正常结束、`Closed` 和 `Rejected`；进程管道三者都暴露正常通道结束。

## 3. 九类追踪

每一类都包含成功、失败和边界用例。示例是当前可执行锚点，不是假设性的协议正文。

### 3.0 会话与 row 生命周期

**成功 — 问候、挂载与 active row。** `Session::handshake` 返回 `Greeted` 会话但不登记插件；仅握手 fixture 验证在挂载 greeted 值之前不会挂载或共享任何内容（`crates/rutis-bridge/tests/session_plugin.rs:224-255`）。替换连接只有在旧会话卸载后才挂载（`crates/rutis-bridge/tests/session_plugin.rs:257-270`）；普通 reconnect pair 在本端达到 `FiberState::Active`（`crates/rutis-bridge/tests/session_plugin.rs:215-222`）。运行时 row 通过 `rows.schema`/`rows.load` 加载；在加载成功路径上，Node、Bun、Python 和 Go 都在报告 row 加载完成前等待服务公告和 listener registration（`node/rutis-runtime/src/runner.mjs:706-715`；`bun/rutis-bun/src/runner.ts:424-429`；`python/rutis/rutis/runner.py:676-689`；`go/rutis/internal/runtime/runtime.go:559-570`）。这是顺序事实，不是跨 runtime 统一的失败策略。

**失败 — 卸载与远端离开。** `rows.unload` 是幂等的；已加载 row 被 dispose，其服务被 withdraw（`crates/rutis-bridge/src/session/plugin/rows.rs:197-207`；`crates/rutis-bridge/src/runtime/process.rs:1352-1365`）。远端离开时，会话被 dispose，已在途调用失败，远端插件被移除，其服务不再存在（`crates/rutis-bridge/tests/session_plugin.rs:750-772`）。row 自行结束时仅在 session 未取消的情况下报告 `rows.ended`；失败时在每次 failed state 报告一次 `rows.failed`（`crates/rutis-bridge/src/session/plugin/rows.rs:137-171`）。

**边界 — reconnect 是新会话。** 旧会话及其 row 先 dispose，新 pair 才能变为 active；仅握手的会话不共享插件或服务，替换连接只有在旧会话卸载后才挂载（`crates/rutis-bridge/tests/session_plugin.rs:182-270`）。正常承载结果是传输线程的 `Ok(())`/`Ok(None)`，而本地会话状态另行是 `Disposed`；不得把这些观察合并为一个握手结果。

生命周期追踪状态：上述会话/row 顺序是当前实现证据，但不主张跨 runtime 统一的 row-failure 策略；行协议的现行规则（装载、更新、卸载、服务与租约、自行结束与失败）见[行协议规格](protocol-rows.md)。公告错误由各 SDK 以不同方式处理，见服务公告设计。对象引用不按能力协商：协议 5 没有 `objects` 能力（[会话协议规格](protocol-session.md) `SESS-16`、`SESS-53`），所有协议 5 实现都收对象引用（`SESS-52`）；#639 的“Rust 中继不查 `objects`”在协议 5 没有可触发的检查点，0.18 及更早的对端在握手时已被拒绝（`SESS-79`）。

### 3.1 帧

**成功 — 有序请求与回复。** 调用是带 `id`、`path`、`target`、`method`、`args` 和可选 `sync` 的 `invoke` 帧；结果是带相同 `id` 的 `return`，或带 `Failure` 的 `throw`（`crates/rutis-bridge/src/session/protocol.rs:168-176,205-212`）。Go 记录器记录了顺序保证：“The session handles what it receives one frame at a time, in order”（`go/rutis/internal/peer/peer_test.go:55-58`）。

接受的追踪：

```json
{"op":"invoke","id":"main:1","path":[],"target":"x","method":"echo","args":{"type":"list","value":[]}}
{"op":"return","id":"main:1","value":{"type":"data","value":"ok"}}
```

**失败 — 未知调用的回复。** 本端不拥有该 id 时，针对它的 `return`/`throw` 是承载/协议失败，而不是调用级错误。Rust 用例 `malformed_and_dangling_frames_close_the_session` 包含“reply to an unknown call”（`crates/rutis-bridge/src/session/rpc/tests.rs:268-326`），并要求 `Error::Transport(_)`；随后本地调用也因会话已关闭而失败。

**边界 — `cancel` 与回复竞态。** 对当前不存在的 id 发送 `cancel` 会被有意忽略（`crates/rutis-bridge/src/session/rpc/tests.rs:329-359`），因为它可能越过自己要取消的回复。这是帧协议中唯一的悬空 id 例外。随后有效调用仍可使用。

### 3.2 握手

**成功 — 双方协议 5 问候。** 每一端发送带 `version`、端点、实现（如果有）和能力的 `hello`（`crates/rutis-bridge/src/session/protocol.rs:157-167`）。Go 原始 peer 发送 `{"op":"hello","version":5,"endpoint":"main"}` 并等待运行时的 hello（`go/rutis/internal/peer/objects_test.go:35-51`）。普通配对启动双方并等待两个 `Ready()` 结果（`go/rutis/internal/peer/peer_test.go:70-89`）。仅仅问候不会挂载插件：`Session::handshake` 返回 `Greeted` 值且不登记任何内容（`crates/rutis-bridge/src/session/plugin/mod.rs:102-114,178-180`）。

**失败 — 版本不匹配。** 说协议 3 的远端会产生精确的本地会话失败 `the far end speaks protocol 3, this side 4: upgrade the far end`。fixture 会分别观察本端 hello，以及远端线程正常的承载结果 `Ok(())`；这里的 `Ok(())` 是传输线程的关闭结果，不是握手成功（`crates/rutis-bridge/tests/session_plugin.rs:101-126`）。握手 10 秒内未完成会失败并显示 `the far end did not greet in time`，通道结束（`crates/rutis-bridge/tests/session_plugin.rs:128-152`；超时在 `crates/rutis-bridge/src/session/plugin/mod.rs:46-47` 定义）。没有 `LocalId` 的宿主失败并显示 `the host provides no id for its sessions`，且不发送问候（`crates/rutis-bridge/tests/session_plugin.rs:154-167`）。

**边界 — 能力只是声明。** `Greet` 在问候里声明 `node` 和配置的能力（`crates/rutis-bridge/src/session/plugin/greet.rs:31-36`），但不检查对端有没有声明本端需要的能力：没有实现提供“必需能力”列表的入口。Rust 曾有一个 `require` 字段，唯一的生产调用传空列表，已删除；Go 的 `peer.Options.Require` 在 `internal/peer` 里没有调用方；其余实现没有这项检查。对端缺少某个能力时，只有用到它的地方才失败；能力声明本身不授予权限（`crates/rutis-bridge/src/session/rpc.rs:1153-1158`），也不被验证。

### 3.3 引用

**成功 — 授予、身份与回到 home。** 引用带标签 `{type:"reference", value:{id, home, kind, origin}}`（`crates/rutis-bridge/src/session/protocol.rs:42-47`）。同一对象发送两次会得到一个代理，送回 home 会得到原值：Go 的 `TestAnObjectFromTheFarEndIsCalledAndGoesHome` 断言 `home == true`、两个收到的代理身份相同，以及名称为 `method` 的 `call` 帧（`go/rutis/internal/peer/crossing_test.go:96-150`）。

**失败 — 未知引用。** `call`、`await` 或 `release` 指向本会话未持有的引用时，会以承载错误关闭会话（`crates/rutis-bridge/src/session/rpc/tests.rs:296-306,312-326`）。这是畸形对端追踪，不是可恢复的 `no-method` 调用错误。

**边界 — release 计数与会话关闭。** `release` 计数允许移除导出；过度释放会故障闭合，会话关闭清空导出（`crates/rutis-bridge/src/session/rpc/tests.rs:240-266`）。正常释放后再调用在 Go 中可观察到：导出计数减一，但会话保持打开（`go/rutis/internal/peer/objects_test.go:71-103`）。

### 3.4 取消

**成功 — 调用方取消传播。** 帧是 `{"op":"cancel","id":...}`（`crates/rutis-bridge/src/session/protocol.rs:217-221`）。在 `TestAnObjectHereIsCancelledThroughAndReleased` 中，远端发送 `call` 后再发送 `cancel`；对象方法观察到上下文取消，已取消 id 的回复仍可能到达，随后 `invoke` 成功（`go/rutis/internal/peer/objects_test.go:66-103`）。

**失败 — 迟到回复被丢弃，而不是会话致命失败。** Go 测试说明方法可能迟到回复：宿主接收并丢弃该回复，然后按顺序处理下一调用（`go/rutis/internal/peer/peer_test.go:135-170`）。调用方得到其上下文错误，但会话保持打开。

**边界 — 取消越过回复。** 同上，未知 `cancel` 被忽略（`crates/rutis-bridge/src/session/rpc/tests.rs:329-359`）。不能仅因回复赢得竞态就关闭会话。取消不会撤销已经提交的引用授予；代理释放时接收方仍发送 `release`（`go/rutis/internal/peer/objects_test.go:129-147`）。

### 3.5 错误

**成功 — 编码错误图往返。** `Failure` 携带 `name`、`message` 和可选 `graph`（`crates/rutis-bridge/src/session/protocol.rs:87-94`）。rutis 的 `no-method` 错误跨端成为带代码 `no-method` 和 `data` 的 `BindingError`；`bad-value` 和承载错误也同样成为编码失败（`crates/rutis-bridge/src/session/protocol.rs:258-307`）。单元测试使用的精确 no-method 消息是 `weather has no method tomorow (methods: today, tomorrow)`（`crates/rutis-bridge/src/session/protocol.rs:269-290`）。

**失败 — 错误属于一个调用。** 无法跨端的值会在帧发送前被拒绝，代码为 `bad-value`；只有该调用失败。下一调用成功，且不会记录被拒方法的 `invoke`（`go/rutis/internal/peer/crossing_test.go:29-64`）。超限结果的精确前缀是 `conformance.big: the result cannot be sent: the message would be `，错误数据为 `{"limit":16777216,"part":"result"}`（`go/rutis/internal/peer/crossing_test.go:66-94`）。

**边界 — 畸形错误图。** 畸形图表现为收到的 `TypeError`，消息为 `bad graph`；它可以再次被抛出且不会结束会话（`go/rutis/internal/peer/crossing_test.go:199-220`）。这有意比畸形*帧*更窄：畸形帧语法或悬空 id 会关闭会话（`crates/rutis-bridge/src/session/rpc/tests.rs:268-326` 的 Rust 矩阵）。

### 3.6 限制

**成功 — 达到上限的消息。** 会话编码在发送前检查通道的 `max_message()`（`crates/rutis-bridge/src/channel/mod.rs:32-40`；调用编码和“只失败该调用”规则在 `crates/rutis-bridge/src/session/rpc.rs:1892-1905`）。适合的消息按顺序排队，调用可以收到回复。

**失败 — 超过 16 MiB 的值。** Go 的精确预期是 `svc.take: argument 2 cannot be sent: the message would be over the limit of 16777216 bytes (16 MiB)`，代码为 `bad-value`，数据为 `{"argument":2,"limit":16777216,"part":"argument"}`（`go/rutis/internal/peer/crossing_test.go:18-64`）。消息不会到达宿主，回调导出被释放，后续 `after` 调用返回 `taken`。这证明失败范围是一个调用，而不是一个会话。

**边界 — 出站积压。** `SessionOptions::outbound_limit` 限制写入器已持有的一个帧之后排队的字节；即使限制为 `0`，空队列也接受一个帧，`send` 中已持有的帧不计入（`crates/rutis-bridge/src/session/rpc.rs:1186-1213`）。101 字节 invoke 帧、202 字节上限的精确 fixture 输出是 `the far end is not reading: 2 messages, 202 bytes waited for it`（`crates/rutis-bridge/src/session/rpc/tests.rs:618-678`）；零上限 fixture 输出是 `the far end is not reading: 1 messages, 101 bytes waited for it`（`crates/rutis-bridge/src/session/rpc/tests.rs:692-741`）。一般规则中的数字按排队帧数和字节数参数化。该承载错误代码为 `peer-gone`；排队消息一并消失。

### 3.7 畸形输入

**成功 — 容忍取消竞态。** 唯一被容忍的、看似畸形/悬空的输入是未知 `cancel`，原因是回复竞态（`crates/rutis-bridge/src/session/rpc/tests.rs:329-359`）。随后有效 `invoke` 得到正常结果。

**失败 — 畸形或悬空帧故障闭合。** Rust 表覆盖畸形 JSON、空字节、开头 BOM、无效 UTF-8、非对象、未知 `op`、错误字段类型、缺少字段、未知线上值、对未知引用的 call/await/release，以及对未知调用的回复（`crates/rutis-bridge/src/session/rpc/tests.rs:268-326`）。每个用例都等待 `peer.closed()`，要求 `Error::Transport(_)`，并验证后续调用被拒绝。这阻止攻击者或有缺陷的对端把无效消息变成含义不明的部分状态。

**边界 — 未知字段不是扩展机制，但不再结束整个会话。** 未知字段按帧的作用范围处理：`hello` 失败握手；`invoke`/`call`/`get`/`await` 只失败该调用（`bad-frame`）；`return`/`throw` 只失败等待它的调用；`release`/`cancel` 忽略。实现见 `crates/rutis-bridge/src/session/rpc.rs` 的 `parse_frame` / `reject_unknown_fields`，测试 `unknown_frame_fields_fail_only_their_call`、`an_unknown_field_in_hello_fails_the_handshake`。新字段仍需协议规范/版本或明确兼容的解码器；静默忽略它不属于当前追踪语言。

### 3.8 网络与重连

**成功 — 承载结束分类且退避状态重置。** 拨号重试解析在覆盖值畸形时保留默认值；有效的 `100,400,60000,300` 覆盖会改变初始、最大、稳定和拒绝延迟，同时保留抖动 `0.2` 和握手 `10s`（`go/rutis/internal/runtime/dial_test.go:25-41`）。退避精确产生 `0.5s 1s 2s 4s 8s 16s 30s 30s 30s`，然后重置回 `500ms`；抖动边界在 `400ms` 和 `600ms` 测试（`go/rutis/internal/runtime/dial_test.go:43-68`）。会话失败按错误类型而非文字分类：rejected → `authRejected`，incompatible → `incompatible`，replaced/host-gone → `retryable`（`go/rutis/internal/runtime/dial_test.go:84-101`）。

**失败 — 不兼容或拒绝的连接不会空转。** 说错协议的远端被分类为 `incompatible`，重试停止直到配置改变；身份不匹配/认证失败是 `authRejected`，在报告状态的同时慢速重试。分类表及精确用例见 `go/rutis/internal/runtime/dial_test.go:84-101`；它明确包含误导性文字用例，证明决策不靠匹配文字。

**边界 — 替换顺序。** 宿主监听器接受带 WebSocket 子协议 `rutis.5` 的令牌（`crates/rutis-host/src/connect/listen.rs:53-80`）。对已有连接的 id，新连接仅在握手后替换旧连接：旧会话卸载，以 `Replaced` 关闭（WebSocket 4002），然后才安装新会话；握手失败不影响任何内容（`crates/rutis-host/src/connect/listen.rs:4-10`）。因此，断开或已消失的对端不能把迟到帧作用到替换后的会话。

## 4. 追踪不变量与反例

验证器必须在所有类别中断言这些不变量：

- **顺序：** 一个通道上的帧一次处理一个，并按到达顺序处理（`go/rutis/internal/peer/peer_test.go:55-58`）。
- **握手门：** `ready` 前不接受调用帧；`Connection::send` 返回 `Transport("protocol handshake incomplete")`（`crates/rutis-bridge/src/session/rpc.rs:1844-1848`）。
- **调用范围：** 编码/值/限制错误只失败该调用；畸形帧、悬空引用/调用和承载失败关闭会话。
- **身份：** 带端点前缀的 id 标识发起会话；转发会重建链（`crates/rutis-bridge/src/session/rpc.rs:1700-1734`），因此旧会话迟到 id 不能命名新会话的调用。
- **取消：** 取消到达执行中的调用；迟到回复被丢弃；取消不会关闭会话。
- **引用守恒：** 授予与释放配对；会话关闭清空未完成的导出/导入。
- **秘密安全：** 通道追踪输出只含大小和原因，从不含消息负载（`crates/rutis-bridge/src/channel/trace.rs:104-133`）。
- **替换隔离：** 同一 id 接受新 hello 前，旧会话关闭已经完成（`crates/rutis-host/src/connect/listen.rs:4-10`）。

必须捕获有意的变异：接受未知 `op`、让未知调用字段结束整个会话而不是失败该调用、把未知 `cancel` 视为致命错误、发送超限帧而不是让调用失败，或在卸载旧会话前安装替换会话。畸形帧矩阵、未知字段测试、超限测试和替换实现就是这些变异的回归证据。

## 5. 开发者看到的内容

协议契约不增加插件 API、宿主命令或配置字段，但会明确现有输出和追踪证据。

### 插件作者

**成功：** 现有调用、对象、取消和错误行为不变；通道追踪中不打印负载。普通成功调用不会给插件新增任何输出行。

**失败（逐字）：**

- 协议 3 对端：`the far end speaks protocol 3, this side 4: upgrade the far end`。
- 从未问候的对端：`the far end did not greet in time`。
- 宿主缺少身份：`the host provides no id for its sessions`。
- 值超过上限：`svc.take: argument 2 cannot be sent: the message would be over the limit of 16777216 bytes (16 MiB)`；代码 `bad-value`；数据 `{"argument":2,"limit":16777216,"part":"argument"}`。
- 调用帧带未知字段：`unknown field "x" in invoke`；代码 `bad-frame`；数据 `{"op":"invoke","fields":["x"]}`；只失败这次调用。嵌套值里的字段写作 `args.value.x` 这样的路径。
- 握手帧带未知字段：`incompatible session: unknown field "x" in hello`；握手失败，会话结束。
- 调用期间通道关闭：现有承载错误，代码为 `peer-gone`；本文不发明新的协议文字。

每次调用级失败都在调用或值这一层结束。握手/承载失败通过匹配协议/能力、恢复对端或修正部署解决；畸形输入是对端实现缺陷。

**需要学习的概念：** 无。追踪格式是贡献者证据，不是插件 API。

### 宿主作者与部署者

**成功：** 现有 `rutis-host` 命令、状态行、关闭码和重试策略保持不变。普通追踪通道逐字记录：

```text
memory peer mac: sent 18 bytes
memory peer mac: received 18 bytes
memory peer mac: ended
memory peer mac: closed: done
```

**失败（逐字）：**

- 协议错误：`the far end speaks protocol 3, this side 4: upgrade the far end`。
- 身份被拒绝是 `Close::Rejected(reason)` / WebSocket 关闭码 `4003`；不兼容承载分类为 `incompatible`；被替换连接使用码 `4002`（`crates/rutis-bridge/src/channel/mod.rs:57-75`）。
- 重试覆盖值畸形：被忽略并恢复默认值（`go/rutis/internal/runtime/dial_test.go:36-39`）；不保证新增警告行。

**需要学习的概念：** 用户无需学习；部署者读取状态时可使用现有重试类别 `retryable`、`authRejected` 和 `incompatible`。

### 运行时作者与贡献者

**成功：** 运行现有 Rust/Go 测试，比较不含负载的追踪事件。通道追踪器的精确成功输出就是上面的四行（`crates/rutis-bridge/src/channel/trace.rs:104-133`）。

**失败（逐字）：** 畸形/悬空输入以 `Error::Transport(_)` 关闭；测试有意不承诺平台特定的原因句子（`crates/rutis-bridge/src/session/rpc/tests.rs:312-326`）。协议版本失败是 `the far end speaks protocol 3, this side 4: upgrade the far end`。

**需要学习的概念：** 帧、追踪事件、允许的追踪、调用级失败、会话级失败和替换隔离。这些只是验证词汇；没有一个进入运行时 API。

## 6. 兼容性、迁移与实现

协议版本 4、帧名、能力、引用表示、错误图、取消和关闭语义是当前规则。本文不提升线上版本，也不增加字段。说其他版本的现有对端会以此前的精确不匹配输出失败握手；双方不会交换调用。

添加帧或字段的实现者必须同时更新 `Frame`/`WireValue`、本文、畸形输入矩阵和所有适用的一致性实现。未知字段按帧的作用范围失败（§3.7）；兼容性变更必须有意且有版本，不能静默接受。添加重连逻辑的实现者必须保持按类型分类、退避重置和替换顺序。

当上面九类的每个用例在所有适用实现中都有追踪验证器或一致性测试、每个不支持的能力/平台都明确标注，并且有意变异都会失败时，#635 才算有证据地完成。当前锚点包括 Rust 生命周期/会话测试（`crates/rutis-bridge/tests/session_plugin.rs:169-270,750-772`）、Rust 会话测试（`crates/rutis-bridge/src/session/rpc/tests.rs:268-359`）、Rust 握手测试（`crates/rutis-bridge/tests/session_plugin.rs:101-167`）、Go 引用/取消/限制测试（`go/rutis/internal/peer/crossing_test.go:29-64`、`go/rutis/internal/peer/objects_test.go:66-147`）和重试测试（`go/rutis/internal/runtime/dial_test.go:25-101`）。

## 相关文件

- [形式化验证设计](design-formal-verification-2026-11-08.md)
- [远程会话设计](design-remote-session-2026-10-28.md)
- [对象引用](design-object-refs-2026-10-27.md)
- [错误消息](design-error-messages-2026-11-03.md)
- `crates/rutis-bridge/src/channel/trace.rs`
- `crates/rutis-bridge/src/session/protocol.rs`
- `crates/rutis-bridge/src/session/rpc/tests.rs`
