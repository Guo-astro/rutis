# 会话协议规格：协议 5

[English](protocol-session.en.md)

状态：现行规格。日期：2026-11-19（首版 2026-11-09；2026-11-18 加入协议 5：`ack` 帧、`SESS-57`、`SESS-61`、§8 与 G3；本次加入 `chunk` 帧与 `chunked` 能力：`SESS-84`–`SESS-88`、§2.5 与 G8）。
议题：[#635](https://github.com/arcships/rutis/issues/635)，属于 [#183](https://github.com/arcships/rutis/issues/183) 第四步；执行计划见 [0.22 形式化覆盖执行计划](plan/formal-verification-0.22.md) WS2。追踪与验证词汇见[会话追踪契约](formal-trace.md)。

本文是会话层唯一的现行规格：两端在一条通道上交换什么、出错和取消时发生什么、哪些消息只让一次调用失败、哪些结束整个会话。行、服务公告、事件、身份认证、重连退避和运行时监督不在本文范围内；它们各自有现行规格：行与宿主服务的租约、行的结束与失败见[行协议规格](protocol-rows.md)，事件见[事件协议](design-events-protocol-2026-10-18.md)，远端租约与重连见[远端租约模型](../specs/remote-lease/README.md)与[远程会话设计](design-remote-session-2026-10-28.md)。本文也是 #634 追踪表 `session` 分区的抽取来源。

每条规则有一个编号（`SESS-*`），状态只有三种：

| 状态 | 含义 |
| --- | --- |
| **当前** | 本文指定的实现和测试所执行的规则；给出代码位置或测试锚点。 |
| **已取代** | 旧设计、旧 issue 或旧协议版本的规则，不能再作为证据。 |
| **明确不保证** | 实现有意不承诺；调用方和验证器不得依赖。 |

规则本身是契约。错误**文字**只有在本文逐字给出时才是契约；同一类错误在不同实现里的句子可以不同（`SESS-77`）。

## 1. 范围与承载

- `SESS-1` **当前。** 一个会话跑在一条 [`Channel`](https://github.com/arcships/rutis/blob/main/crates/rutis-bridge/src/channel/mod.rs) 上：一条有序、可靠、双工的消息流，保留消息边界，移动不透明字节。通道不懂帧、调用或引用。`crates/rutis-bridge/src/channel/mod.rs:1-9,20-48`
- `SESS-2` **当前。** 每条消息是一个 UTF-8 JSON 对象，带 `op` 标签；会话层负责编码、解码和错误映射。读取顺序就是到达顺序，一次一帧。`crates/rutis-bridge/src/session/rpc.rs:1557-1570,3207-3216`
- `SESS-3` **当前。** 协议版本是 5（`ENDPOINT_PROTOCOL`）。Rust、Node、Bun、Python、Go、Swift、Kotlin 使用同一个常量；WebSocket 子协议是 `rutis.5`。`crates/rutis-bridge/src/session/mod.rs:9-14`；`node/rutis-runtime/src/session.mjs:58`；`bun/rutis-bun/src/session.ts:57`；`python/rutis/rutis/peer.py:42`；`go/rutis/internal/peer/peer.go:34`；`swift/rutis/Sources/Rutis/Session/Peer.swift:13`；`kotlin/rutis/rutis/src/shared/kotlin/arcships/rutis/internal/peer/Peer.kt:42`；`crates/rutis-host/src/connect/listen.rs:74,408`
- `SESS-4` **当前。** 每个会话有两个端点 id（自报的 `endpoint`），每端的调用号以本端 id 为前缀。id 在一个部署内唯一，由部署保证；同一 id 的新连接顶替旧连接属于连接层规则，不在本文。`crates/rutis-bridge/src/channel/mod.rs:77-109`；`crates/rutis-bridge/src/session/rpc.rs:1493-1499,2443-2489`
- `SESS-5` **当前。** 重连产生新会话。旧会话的调用、引用和迟到帧不进入新会话；旧状态不延续。`docs/formal-trace.md` §3.0；`crates/rutis-bridge/tests/session_plugin.rs:182-270`
- **已取代：** 一个会话可以带“兼容格式”（compat）与“端点格式”两种线格式。协议 5 只有一种格式（§2.3）。

## 2. 帧与线格式

### 2.1 帧

`SESS-6` **当前。** 帧词汇是 `Frame` 的十个变体；`op` 用 snake_case，未知 `op` 结束会话。字段以下面为准，未列出的字段一律不存在（§9）。另有一种 `chunk` 帧，由会话读取层在 `Frame` 解码之前处理，承载超过通道上限的整帧（§2.5）。`crates/rutis-bridge/src/session/protocol.rs:149-230`；`crates/rutis-bridge/src/session/chunks.rs`

| 帧 | 字段 | 含义 |
| --- | --- | --- |
| `hello` | `version: u32`；`endpoint?: string`；`implementation?: {name, version}`；`capabilities?: string[]` | 开场的问候。`version` 必填，其余三个字段可缺省；接收方要求 `endpoint` 合法存在（§4）。 |
| `invoke` | `id: string`；`path: string[]`；`target: string`；`method: string`；`args: WireValue`；`sync?: bool`；`awaited?: bool` | 调用服务方法。`target` 为空时是控制操作（`events.*`、`rows.*` 等）。 |
| `call` | `id`；`path`；`reference: u64`；`method?: string`；`args: WireValue`；`sync?: bool`；`awaited?: bool` | 调用函数引用；带 `method` 时调用对象引用的方法。 |
| `get` | `id`；`path`；`reference: u64`；`property: string`；`sync?: bool`；`awaited?: bool` | 读取对象引用的实时属性。 |
| `await` | `id`；`path`；`reference: u64`；`sync?: bool`；`awaited?: bool` | 等待一个异步结果引用。 |
| `return` | `id: string`；`value: WireValue` | 调用的结果。 |
| `throw` | `id: string`；`error: Failure` | 调用的错误。`Failure` 是 `{name: string, message: string, graph?: JSON}`。 |
| `release` | `reference: u64`；`count: u64` | 归还引用的授予次数（§6）。 |
| `cancel` | `id: string` | 调用方放弃调用 `id`（§7）。 |
| `ack` | `id: string` | 被调方已结算对调用 `id` 的取消：不会再为它发 `return`/`throw`（§7）。 |
| `chunk` | `transfer: string`；`seq: u64`；`last: bool`；`data: string` | 整帧的一片（§2.5）。不是 `Frame` 的变体，在 `Frame` 解码之前读取。 |

`SESS-7` **当前。** `sync` 缺省为 `false`，为 `false` 时不写到线上。`awaited` 缺省为 `true`，为 `true` 时不写到线上：只有“发出后不等结果”的调用写 `awaited: false`（`SESS-83`）。`hello` 的 `endpoint`、`implementation`、`capabilities` 与 `throw` 的 `graph` 在缺省时不写。`crates/rutis-bridge/src/session/protocol.rs:157-221`

`SESS-8` **当前。** `path` 的每个条目是一个调用号：`<endpoint>:<n>`，或转发后带会话标签的条目（§5.2）。`path` 是本帧所属的调用链，不是自由字段。`crates/rutis-bridge/src/session/rpc.rs:2739-2742`；[会话标签设计](design-session-tags-2026-10-12.md) §二

### 2.2 值

`SESS-9` **当前。** `WireValue` 是带 `type` 标签、以 `value` 为内容的联合；下表是全部标签。`crates/rutis-bridge/src/session/protocol.rs:26-85`

| `type` | `value` | 含义 |
| --- | --- | --- |
| `undefined` | 无 | 未给出的值。 |
| `data` | 任意 JSON | JSON 数据。 |
| `list` | `WireValue[]` | 列表。 |
| `record` | `{string: WireValue}` | 字段里可以含引用的普通对象。 |
| `signal` | 无 | 本次调用的 `AbortSignal`；调用被取消时中止。 |
| `reference` | `{id: u64, home: bool, kind: function, future, object 或 plugin 之一, origin: string[]}` | 函数、异步结果、对象或插件定义（§6）。 |
| `instant`、`datetime`、`date`、`time`、`bytes`、`map`、`set`、`bigint`、`decimal`、`uuid` | 各自文本或结构 | 十种带标记的值，线上格式见[协议插件设计](design-protocol-plugins-2026-09-25.md) §6.1。 |

`SESS-10` **当前。** 作为整个参数或整个返回值，`undefined` 保留为 `undefined`；值内部任何深度的 `undefined` 都是 `null`。`crates/rutis-bridge/src/session/rpc.rs:2356-2359,2567-2573`；`docs/guarantees.md` 的 `bridge/object-refs`

`SESS-11` **当前。** 带标记的值只发给声明了对应能力 `value-<tag>` 的对端；对端没声明时，含该值的调用在发出前失败，会话继续（§2.4、§10）。`crates/rutis-bridge/src/session/rpc.rs:2204-2214,2298-2324`

### 2.3 格式

`SESS-12` **当前。** 只有一种线格式：端点格式、协议 5。调用号总是 `<endpoint>:<n>`；不存在不带端点前缀的调用号，也没有第二种 `hello`。`crates/rutis-bridge/src/session/mod.rs:9-14`；[迁移到 0.19](migration-0.19.md)「一起升级」；`crates/rutis-bridge/src/session/rpc.rs:1493-1499,2456-2462`

`SESS-13` **已取代。** issue #635 与旧设计里的两种格式：兼容格式 2（无端点、`node:n` / `rust:n` 调用号）与端点格式 3。0.19 起协议 4 只留端点格式；0.18 及更早的对端在握手时被拒，由各端同时升级到 0.19 解决。`docs/migration-0.19.md`「一起升级」；`crates/rutis-bridge/src/session/rpc.rs:2456-2462`

`SESS-14` **已取代。** “`sync` 只发给声明了 `sync-wait` 的对端”。协议 4 没有 `sync-wait` 能力；`sync` 是每个协议 5 实现都认识的帧字段，按 §2.1 序列化。

### 2.4 能力

`SESS-15` **当前。** 能力是 `hello.capabilities` 里的字符串；能力声明的是“这一端能做什么”，不是权限。会话层读取四类：`value-<tag>`（能否发某种值）、`reentrant-sync` 与 `sync-stack`（同步等待图里的运行时形状）、`chunked`（能否重组 `chunk` 帧，§2.5）。插件与挂载层读取 `node`、`runtime`、`plugins`。能力只是声明：没有“必需能力”列表，缺少某种能力时的处理在用到它的那一层（`SESS-25`）。`crates/rutis-bridge/src/session/rpc.rs:1154-1178,1590-1611,2204-2214`；`crates/rutis-bridge/src/session/plugin/greet.rs:31-36`；`crates/rutis-bridge/src/session/plugin/mod.rs:352,368`；`docs/guarantees.md` 的 `bridge/sync-wait-cycle-refused`

| 能力 | 声明者 | 含义 |
| --- | --- | --- |
| `reentrant-sync` | Rust、Python、Go、Swift、Kotlin | 同步调用可以在同一执行栈上重入；每个进来的调用有自己的线程或栈。 |
| `sync-stack` | Node、Bun、Python | 单线程、单栈；同步等待会叠在这一个栈上。 |
| `node` | 会话插件（宿主一侧的 `Greet`）加在自己的声明里 | 这一端是完整的框架节点。`crates/rutis-bridge/src/session/plugin/greet.rs:34-36`；`crates/rutis-bridge/src/session/plugin/mod.rs:352` |
| `runtime` | Node、Bun、Python、Go、Swift、Kotlin 运行时进程 | 这一端是运行时进程，可被宿主挂载或拨号。`node/rutis-runtime/src/runner.mjs:39`；`bun/rutis-bun/src/main.ts:118`；`python/rutis/rutis/__main__.py:72`；`go/rutis/internal/runtime/serve.go:99`；`swift/rutis/Sources/Rutis/Remote/App.swift:356`；`kotlin/rutis/rutis/src/shared/kotlin/arcships/rutis/RutisApp.kt:122`；`crates/rutis-bridge/src/session/plugin/mod.rs:369` |
| `plugins` | Rust；Node/Bun 作为宿主时 | 这一端收插件定义值。`crates/rutis-bridge/src/session/rpc.rs:918-922`；`docs/guarantees.md` 的 `bridge/plugin-values` |
| `value-<tag>` | 所有实现，十种 | 这一端收该种带标记的值。`crates/rutis-bridge/src/session/rpc.rs:904-917` |
| `chunked` | Rust（`rutis-bridge`）、Node 运行时 | 这一端重组 `chunk` 帧，超限的整帧可分片发来（§2.5，`SESS-85`）。`crates/rutis-bridge/src/session/rpc.rs` 的 `Endpoint::rust`；`node/rutis-runtime/src/session.mjs` 的 `CAPABILITIES` |

`SESS-16` **已取代。** 能力 `sync-wait`、`events-v2` 和 `objects`。协议 4 的实现里没有这三个常量；对象引用对所有协议 4 实现都可传（§6.2）。`sync-wait` 与 `events-v2` 属于 0.18 及更早的协商；`objects` 是 0.18 的对象能力（[对象引用](design-object-refs-2026-10-27.md) §三）。协议 4 的实现不再声明或检查这三个能力。

`SESS-17` **明确不保证。** 未知能力名会被忽略，不报错；对端声明的能力不被验证（声明了却做不到，只有真正调用时才失败）；任何能力都不授予权限。

### 2.5 跨帧的整帧

`SESS-84` **当前。** 超过通道单条消息上限的帧可以由一串 `chunk` 帧承载。`chunk` 帧不是 `Frame` 的变体：它在 `Frame` 解码之前由会话读取层处理，字段是 `transfer: string`（`<发送端 endpoint>:<n>`，发送端在会话内从 1 递增、非空）、`seq: u64`（从 0 开始逐个加一）、`last: bool`、`data: string`（原帧 UTF-8 JSON 字节的一片，base64，RFC 4648 §4，带填充、规范形式、无空白）。所有片的 `data` 依次拼接就是那条消息。`hello` 永远不分片。`chunk` 的字段类型不对、多了字段、或 `seq` 不从 0 开始，都是畸形帧（§8）。`crates/rutis-bridge/src/session/chunks.rs` 的 `Chunk`；`node/rutis-runtime/src/session.mjs` 的 `#receiveChunk`

`SESS-85` **当前。** 能力 `chunked` 声明“这一端能重组 `chunk` 帧”。只有本端与对端都在 `hello.capabilities` 里声明了它，发送方才把超限的帧切片；否则保持 §10 的超限错误，帧不写出。0.23 声明它的是 Rust（`rutis-bridge`）与 Node 运行时；Bun、Python、Go、Swift、Kotlin 不声明，因此它们的会话行为不变，也不会收到 `chunk` 帧。读取 `chunk` 的一端要求对端声明过：没有声明却发来 `chunk` 是传输错误。`crates/rutis-bridge/src/session/rpc.rs` 的 `Endpoint::rust`、`send_limit`、`accept_chunk`；`node/rutis-runtime/src/session.mjs` 的 `CAPABILITIES`、`#chunking`

`SESS-86` **当前。** 发送方在整帧字节数超过 `Sender::max_message()`（Node 是 `maxMessage`，与所在通道一致）且双方声明了 `chunked` 时切片：每片至多 8 MiB（8388608 字节），并按通道上限缩小，使单个 `chunk` 帧装得下通道的一条消息；片长算出来为 0 时不切片。一帧的所有 `chunk` 帧在出站顺序里连续，中间没有别的帧；同一时刻只有一个传输。整帧超过会话的 `max_transfer`（默认 256 MiB，268435456 字节）时不发送，按 `SESS-71` 只让这次调用失败，`limit` 为配置值。`crates/rutis-bridge/src/session/chunks.rs` 的 `CHUNK_SIZE`、`slice_size`、`Chunk::frames`；`node/rutis-runtime/src/session.mjs` 的 `#transmit`

`SESS-87` **当前。** 接收方按 `transfer` 缓冲：`seq` 必须从 0 逐个加一，进行中已有另一个 `transfer`（同一时刻至多一个未完成传输）、累计超过会话的 `max_transfer`、`data` 不是规范 base64、`last` 不是布尔或字段类型不对，都以传输错误结束会话（`chunk` 不属于任何调用，没有更小的失败范围）；问候之前收到 `chunk` 是 `request before protocol handshake`。只有 `last` 到达、拼接和解析都成功之后，帧才交给会话；重组出的帧与整条到达的帧完全同权（深度、未知字段、调用号、引用、取消规则不变），且不能本身又是 `chunk` 帧。会话结束时未完成的传输随会话丢弃，从不派发、从不回复。`crates/rutis-bridge/src/session/chunks.rs` 的 `Reassembly::accept`；`crates/rutis-bridge/src/session/rpc.rs` 的读线程与 `accept_chunk`；`node/rutis-runtime/src/session.mjs` 的 `#receiveChunk`

## 3. 帧的顺序与处理

`SESS-18` **当前。** 一个通道上的帧一次处理一个，按到达顺序。调用可以在各自的任务、线程或栈上并发执行；帧的读取和调用号接纳是串行的。`crates/rutis-bridge/src/session/rpc.rs:1557-1570,2792-2839`；`go/rutis/internal/peer/peer_test.go:55-58`

`SESS-19` **当前。** `events.*` 与 `rows.ended` 按帧序进入会话的有序队列；表变更和 `emit` 就地按顺序执行，其余模式按顺序开始。这是事件协议的规则在会话层的落点；本文只规定它们不破坏帧序。`crates/rutis-bridge/src/session/rpc.rs:1244-1283,2767-2884`

## 4. 握手

`SESS-20` **当前。** 双方都先写 `hello`，再开始读取。发送放在读线程启动之前，所以对端总能在通道结束之前读到本端的版本。`crates/rutis-bridge/src/session/rpc.rs:1524-1541`；测试 `crates/rutis-bridge/src/session/rpc/tests.rs::endpoint_format::issue_239_an_incompatible_side_greets_before_it_reads_the_far_greeting`

`SESS-21` **当前。** `hello` 必须先于一切。对端未问候时收到任何其他帧（包括 `return` / `throw` / `cancel` / `release`），会话以传输错误结束，文字是 `request before protocol handshake`。`crates/rutis-bridge/src/session/rpc.rs:2535-2536,2616-2618`；`go/rutis/internal/peer/peer_test.go::TestACallBeforeTheHandshakeEndsTheSession`；Node、Bun、Python、Swift、Kotlin 的同一检查见 `node/rutis-runtime/src/session.mjs:709`、`bun/rutis-bun/src/session.ts:696`、`python/rutis/rutis/peer.py:1682`、`swift/rutis/Sources/Rutis/Session/Peer.swift:1086`、`kotlin/rutis/rutis/src/shared/kotlin/arcships/rutis/internal/peer/Peer.kt:1120`

`SESS-22` **当前。** 第二次 `hello` 结束会话，文字是 `duplicate protocol handshake`。`crates/rutis-bridge/src/session/rpc.rs:2602-2614`；`node/rutis-runtime/src/session.mjs:705`；`python/rutis/rutis/peer.py:1675`；`go/rutis/internal/peer/peer.go:1382`；`swift/rutis/Sources/Rutis/Session/Peer.swift:1078`；`kotlin/rutis/rutis/src/shared/kotlin/arcships/rutis/internal/peer/Peer.kt:1114`

`SESS-23` **当前。** `hello.version` 必须等于 4。不等时握手失败（`Error::Handshake(Incompatible)`），文字说明升级哪一端：对端更新时说升级本端并写出本端的包；对端更旧时说升级对端，带上它的 `implementation` 名字和版本（如果它说了）。`crates/rutis-bridge/src/session/rpc.rs:1224-1242,2456-2462`；测试 `crates/rutis-bridge/src/session/rpc/tests.rs::endpoint_format::a_far_end_of_another_protocol_version_is_told_which_end_to_upgrade`；`docs/guarantees.md` 的 `bridge/version-refusal`

`SESS-24` **当前。** `hello.endpoint` 必须存在且是合法端点 id：非空，只含小写字母、数字和 `-`。缺失、非法，或（设置了期望时）与本端期望的不一致都使握手失败：缺失与非法是 `Incompatible`（Rust 内层原因：`the far end named no endpoint` / `invalid endpoint id …`），期望不符或自称本端 id 是 `IdentityMismatch`。`crates/rutis-bridge/src/session/rpc.rs:2463-2479`；`crates/rutis-bridge/src/channel/mod.rs:77-109`；测试 `crates/rutis-bridge/src/session/rpc/tests.rs::endpoint_format::a_greeting_from_another_endpoint_fails_the_handshake`

`SESS-25` **明确不保证。** 没有实现提供“声明必需能力”的入口，握手不会因为对端缺少某个能力而拒绝。Rust 的 `Greet` 曾有一个 `require` 字段，唯一的生产调用传的是空列表，没有入口能传入非空能力，已删除；Go 的 `peer.Options.Require` 在 `internal/peer` 里，同样没有调用方；Node、Bun、Python、Swift、Kotlin 没有这项检查。对端不声明某种能力时，只有用到它的地方才失败（例如带标记的值不发送，`SESS-11`）。`crates/rutis-bridge/src/session/plugin/greet.rs:12-19`；`go/rutis/internal/peer/peer.go:111-114`

`SESS-26` **当前。** 会话插件等待问候最多 10 秒；超时以传输错误 `the far end did not greet in time` 结束会话。`crates/rutis-bridge/src/session/plugin/mod.rs:46-47,102-110`；测试 `crates/rutis-bridge/tests/session_plugin.rs:130-154`

`SESS-27` **当前。** 本端在 `ready` 之前发起调用会失败，文字是 `protocol handshake incomplete`，帧不写出。`crates/rutis-bridge/src/session/rpc.rs:1844-1848`

`SESS-28` **当前。** 仅仅完成问候不会挂载对端：会话插件返回一个 `Greeted` 值，不登记任何插件或服务；挂载是之后的一步。`crates/rutis-bridge/src/session/plugin/mod.rs:102-153,178-184`；测试 `crates/rutis-bridge/tests/session_plugin.rs:228-270`

`SESS-29` **当前。** 握手失败关闭会话（`Close::Done`）并撤销等待中的调用。对端从通道结束得知会话没有建立；在本端问候已经写出时，对端至少读到本端的版本。`crates/rutis-bridge/src/session/rpc.rs:1629-1682`；`crates/rutis-bridge/src/session/plugin/mod.rs:102-110`

`SESS-30` **已取代。** “握手可以在挂载前登记插件或服务”“`ready` 之前可以成功调用”。`docs/formal-trace.md` §1 的规则状态表已把它们列为旧证据。

`SESS-31` **明确不保证。** 10 秒时限是调度上的期限，不是墙上时钟的精确承诺；负载或暂停可能让实际时间更长。握手失败后的重试退避与“哪些失败停止重试”属于连接层（`bridge/dial-out-rules`），不在本文的承诺内。

## 5. 调用

### 5.1 调用号

`SESS-32` **当前。** 每个会话、每一端各自维护调用号：从 1 开始，严格递增，最大到 2^53−1；线上写作 `<本端 endpoint>:<n>`。调用号只在本会话内唯一。`crates/rutis-bridge/src/session/rpc.rs:1849-1859,1085-1095`；`node/rutis-runtime/src/session.mjs:346,734`；`bun/rutis-bun/src/session.ts:721`；`python/rutis/rutis/peer.py:1093,1735`；`go/rutis/internal/peer/peer.go:1256,1605`；`swift/rutis/Sources/Rutis/Session/Peer.swift:976,1258`；`kotlin/rutis/rutis/src/shared/kotlin/arcships/rutis/internal/peer/Peer.kt:781,1249`

`SESS-33` **当前。** 收到 `invoke` / `call` / `get` / `await` 时，调用号必须：

1. 以对端问候的端点 id 加 `:` 为前缀；否则会话结束，文字是 `invalid invocation identity/path`。
2. 去掉前缀后是十进制无符号整数（规范形式是 `[1-9][0-9]*`），大于本会话已接纳的最后一个号，且不超过 2^53−1；否则会话结束，文字是 `invalid or repeated invocation identity`。
3. 不在本帧收到的 `path` 里；出现时会话结束，文字是 `invalid invocation identity/path`。

`crates/rutis-bridge/src/session/rpc.rs:1085-1095,2739-2742`；`node/rutis-runtime/src/session.mjs:732-734`；`python/rutis/rutis/peer.py:1732-1735`；`go/rutis/internal/peer/peer.go:1586-1605`；`swift/rutis/Sources/Rutis/Session/Peer.swift:1249-1258`；`kotlin/rutis/rutis/src/shared/kotlin/arcships/rutis/internal/peer/Peer.kt:1231-1249`

接收方的检查宽严不完全一致：Node、Bun、Go、Swift、Kotlin 要求正则 `^[1-9][0-9]*$`；Rust 与 Python 用整数解析，额外接受前导零和 `+`。发送方总是发规范形式，所以这只影响畸形对端。

`SESS-34` **当前。** 一个调用即使因为参数里的值传不过去而被拒绝，也占用它的调用号；对端重发同一个号仍是重复调用，会话结束。`crates/rutis-bridge/src/session/rpc.rs:2576-2593`；测试 `crates/rutis-bridge/src/session/rpc/tests.rs::a_refused_call_takes_its_id`

### 5.2 调用链 `path`

`SESS-35` **当前。** 收到调用时，接收方在自己的 `path` 末尾追加本帧的 `id`，然后带着整条链执行。链用来把反向调用交给正在同步等待的那一方，并让宿主建立等待图。`crates/rutis-bridge/src/session/rpc.rs:2739-2742,2792-2815`

`SESS-36` **当前。** 转发到另一个会话时，条目按会话标签重写（`rebase`）：不带标签的条目加上来源会话的标签；带目标会话标签的条目还原成目标会话里的原生调用号；其他会话的条目原样保留。标签是 `s` + 进程随机数（15 位十进制）+ 计数器。`crates/rutis-bridge/src/session/rpc.rs:849-902`；[会话标签设计](design-session-tags-2026-10-12.md) §四；`docs/guarantees.md` 的 `bridge/call-chains-across-processes`

`SESS-37` **明确不保证。** 会话标签的唯一性是概率性的（两个进程随机数相等的概率约 10⁻¹⁵）；两个 0.9 及更早的进程之间仍有旧标签问题。这属于连接层与历史兼容，不是协议 5 的规则。

### 5.3 同步与异步

`SESS-38` **当前。** `sync: true` 表示调用方在调用线程上同步等待结果。等待期间，属于同一调用链的反向调用交给这个等待线程执行（嵌套同步回调）；不相关的调用不打断等待。`crates/rutis-bridge/src/session/rpc.rs:1862-1866,2000-2019,2792-2839`；`go/rutis/internal/peer/peer_test.go::TestSynchronousCallsSayTheyWait`

`SESS-39` **当前。** 同步转发进入等待图。若这次转发会让等待成环（链上的同步调用只能由被它自己阻塞的执行器推进），转发不发出，调用以 `SyncWaitCycle` 失败，会话继续。`crates/rutis-bridge/src/session/rpc.rs:1867-1879`；`crates/rutis-bridge/src/session/rpc/waits.rs`；`docs/guarantees.md` 的 `bridge/sync-wait-cycle-refused`

`SESS-83` **当前。** 异步调用在帧上说明它的结果会不会被等待：`awaited: false` 表示调用方在发起它的调用结束之前不会等这个结果（发出即不管），`awaited` 缺省或 `true` 表示会等。只有一种调用进入等待图：`sync: true`，或链上最后一跳已标记（`SESS-38`）且帧没有说 `awaited: false` 的异步调用；写明 `awaited: false` 的调用记为**入口**（`entry`）：没人等它，所以它不是一条边，但在它之下发出的调用仍以它的调用号为链的一部分。`sync: true` 已表示等待，`awaited` 此时被忽略（发送方不写）。接收方在转发这次调用时沿用原帧的信号：宿主为另一个会话转发调用时，被转发的调用只在原调用方确实等待时才算边。七种实现（Rust、Node、Bun、Python、Go、Swift、Kotlin）都认识这个字段；运行时在确定没人等结果时（`sendAnswered` / `request_answered` / `notify` 一类）写 `awaited: false`。字段缺省保持旧语义：旧调用方不说，宿主按“会等”保守记边，不误删真实等待边；这也意味着旧调用方仍可能被误判成环。`crates/rutis-bridge/src/session/protocol.rs`；`crates/rutis-bridge/src/session/rpc/waits.rs` 的 `forward`；测试 `crates/rutis-bridge/src/session/rpc/waits.rs::tests::an_asynchronous_forward_nobody_awaits_is_an_entry`、`::an_awaited_forward_closes_the_cycle`、`crates/rutis-bridge/src/session/rpc/tests.rs::a_forwarded_call_carries_the_callers_awaited_signal`；`bridge/sync-wait-cycle-refused`

`SESS-40` **明确不保证。** 没有在帧上标 `sync` 的同步调用不在检测范围内，可能死锁；帧上说 `awaited: false` 却实际等待结果的调用同样漏掉等待边，可能死锁（信号被信任，宿主不验证）；跨主机的环每台主机都看不见；可重入的代价、新旧版本混用不在承诺内。`docs/guarantees.md` 的 `bridge/sync-wait-cycle-refused`

### 5.4 一次调用的结算

`SESS-41` **当前。** 每次调用恰好结算一次。回复到达时先从等待表摘除再结算；第二次回复、或对端本端没在等的调用回复，都是协议错误，会话以 `response for unknown call` 结束。`crates/rutis-bridge/src/session/rpc.rs:2490-2503`

`SESS-42` **当前。** 会话层不自动重试。一次调用只写一次；失败以 `Err` 返回给调用方。重发同一个调用号不是重试，而是重复调用（§5.1）。重连建立新会话，旧会话的调用不会在新会话上重放。`crates/rutis-bridge/src/session/rpc.rs:1982,1085-1095`；`docs/guarantees.md` 的 `bridge/in-flight-call-fails-once`；`docs/formal-trace.md` §3.0

`SESS-43` **当前。** 业务错误只影响它的调用：被调方用 `throw` 结算该调用，会话和其他调用继续。错误图的形状不是协议错误；收到形状不对的图时，接收方只取名字和消息，不结束会话。`crates/rutis-bridge/src/session/rpc.rs:2633-2638,3029-3083`；`docs/guarantees.md` 的 `bridge/bad-values`、`bridge/error-codes`

`SESS-44` **当前。** 发送方不能编码的值、对端不收的带标记值、超过通道上限的消息，都在发出前失败，只让这次调用失败；已经做出的引用授予回滚，帧不写出。`crates/rutis-bridge/src/session/rpc/crossing.rs:143-246`；`crates/rutis-bridge/src/session/rpc.rs:1892-1946,3063-3078`；`docs/guarantees.md` 的 `bridge/bad-values`

`SESS-45` **明确不保证。** 协议没有调用截止时间；超时由调用方自己实现（`docs/design-protocol-plugin-mount.md` §3.1）。并发调用之间的开始顺序、执行顺序没有承诺，只有帧的读取顺序和事件操作的有序队列（§3）是承诺。

## 6. 引用

### 6.1 授予与释放

`SESS-46` **当前。** 函数、异步结果、对象和插件定义以引用传递：`{id, home, kind, origin}`。同一个对象重复出现是同一个引用；同一对象交给同一会话两次是同一个 id、授予次数加一。`crates/rutis-bridge/src/session/protocol.rs:42-47`；`crates/rutis-bridge/src/session/rpc.rs:3095-3107`；`docs/guarantees.md` 的 `bridge/object-refs`、`bridge/function-refs`

`SESS-47` **当前。** 发送方每授予一次加一。接收方按收到的次数持有；本地代理全部释放后，按收到的次数发 `release {reference, count}`。`count` 必须大于 0 且不超过当前的授予次数；否则会话以 `invalid reference release count` 结束。`crates/rutis-bridge/src/session/rpc.rs:992-1010,2655-2676`；`docs/guarantees.md` 的 `bridge/release-count`、`bridge/reference-release-count`

`SESS-48` **当前。** `release` 指向本会话没有的引用时，会话以 `release of unknown reference` 结束。`call`、`get`、`await` 指向没有的引用时，会话以 `unknown or released reference` 结束；用方法调用或属性读取指向非对象引用时，会话以 `method call or property read on a non-object reference` 结束。`crates/rutis-bridge/src/session/rpc.rs:2504-2513,2655-2676,2699-2736`；`crates/rutis-bridge/src/session/testing.rs:176-191`

`SESS-49` **当前。** 引用 id 从 1 到 2^53−1；超出范围或分配耗尽使会话结束（`invalid reference identity`、`reference identifiers exhausted`）；授予次数溢出同样结束会话（`reference grant overflow`）。`crates/rutis-bridge/src/session/rpc.rs:2397-2398,3085-3108`

`SESS-50` **当前。** 回到 home 的引用（`home: true`）还原为原来的导出对象，身份保持不变；本端没有这个导出、或 `kind` 不一致，都是协议错误，会话结束。`crates/rutis-bridge/src/session/rpc.rs:2372-2390`

`SESS-51` **当前。** 会话关闭清空导出表和导入表，不等待 GC；未释放的引用随会话一起消失。`crates/rutis-bridge/src/session/rpc.rs:1663-1664,992-1009`；`docs/formal-trace.md` §4

### 6.2 对象

`SESS-52` **当前。** 带方法的对象按引用传：`kind: "object"`，方法调用是带 `method` 的 `call`，属性读取是 `get`。协议 5 的所有实现都收对象引用，发送方不按能力检查。`crates/rutis-bridge/src/session/rpc.rs:2690-2736`；`docs/guarantees.md` 的 `bridge/object-refs`

`SESS-53` **已取代。** “对象引用只发给声明了 `objects` 的对端”；`objects` 能力本身（0.18 的兼容格式规则）。协议 5 没有这个能力，也没有对应的拒绝分支。因此 #639 记录的“Rust 中继不查 `objects`”在协议 5 的会话里没有可触发的检查点；0.18 及更早的对端在握手时已被拒绝（`SESS-79`）。

`SESS-54` **当前。** 转发：从会话 A 导入的引用交给会话 B 时，B 得到同一个 `kind` 的中继；B 的调用、读取和等待按链重写后转给 A；释放中继会把释放传到 A。插件定义不中继给第三个会话；发到第三个会话时只让那次调用失败。`crates/rutis-bridge/src/session/rpc/relay.rs:19-78,93-175`；`docs/guarantees.md` 的 `bridge/object-refs`、`bridge/plugin-values`

`SESS-55` **明确不保证。** 引用生命周期不使运行时进程保持存活；持有方进程退出后，对它的对象的调用以 `peer-gone` 失败，而不是让运行时继续运行。`docs/formal-trace.md` §1

## 7. 取消

`SESS-56` **当前。** `cancel {id}` 表示调用方放弃调用 `id`。调用方丢弃尚未完成的异步调用的 future 时发这个帧；已经返回、但调用方还没消费的结果也可以取消。从发出取消到被调方结算它（迟到回复或 `ack`）之间，调用方把这次调用留在取消表里。`crates/rutis-bridge/src/session/protocol.rs:210-230`；`crates/rutis-bridge/src/session/rpc.rs:2273-2300,2952-2996`

`SESS-57` **当前。**（Rust 被调方）收到 `cancel` 时：

- 还在排队、尚未开始的调用被丢掉：方法不运行，参数引用归还，取消被确认；例外是已接纳的 `emit` 方式的 `events.dispatch`：它照常开始（[事件协议](design-events-protocol-2026-10-18.md) §5.7），取消被确认，回复被丢弃（#1121）。
- 正在等待的 `await` 被丢掉并确认。
- 已经开始运行的调用照常完成（取消不打断已经在运行的方法）；结果被丢弃，取消被确认。
- 如果回复在读到 `cancel` 之前已经写出，那个回复就是结算：不发明 `ack`，调用方丢弃该回复。
- 对已取消调用的迟到回复被丢弃并计数（`orphans()`），不当作协议错误，也不关闭会话。
- 写出 `ack` 之后，被调方不再为这次调用发 `return`/`throw`。

`crates/rutis-bridge/src/session/rpc.rs:2273-2300,2952-2996,3384-3425`；测试 `crates/rutis-bridge/src/session/rpc/tests.rs::a_queued_cancelled_call_is_dropped_and_acknowledged`、`::a_running_cancelled_call_is_acknowledged_and_its_reply_dropped`、`::a_cancelled_emit_dispatch_stays_queued`

`SESS-58` **当前。** 对不存在的调用发 `cancel` 被忽略，也不确认；这可能是取消越过了它要取消的回复。这是唯一被容忍的悬空调用号。`crates/rutis-bridge/src/session/rpc.rs:2952-2996`；测试 `crates/rutis-bridge/src/session/rpc/tests.rs::a_cancel_for_an_unknown_call_is_ignored`

`SESS-59` **当前。** 取消逐跳传播：中继转发中的等待被取消时，被丢弃的转发请求会向下一跳发出 `cancel`；跨 link 时同样。取消不撤销已经提交的引用授予。`crates/rutis-bridge/src/session/rpc/relay.rs:148-173`；`crates/rutis-bridge/src/session/rpc.rs:2020-2031`；`docs/guarantees.md` 的 `bridge/multihop-cancel-and-release`

`SESS-60` **当前。** 取消本身不关闭会话；取消后其他调用照常进行。`crates/rutis-bridge/src/session/rpc.rs:2702-2721`；`go/rutis/internal/peer/peer_test.go:135-170`；`go/rutis/internal/peer/objects_test.go:66-103`

`SESS-61` **当前。** 一次取消由被调方恰好结算一次：要么回 `ack {id}` 且此后不再为这次调用发 `return`/`throw`，要么读到取消之前已经写出的回复就是它的结算、不再发 `ack`。被调方对它仍持有的取消都确认（排队中、运行中、正在等待的 `await`）；对已经答复、已经丢掉或从未有过的调用不确认。调用方必须接受任一种结算，丢弃越过取消的回复（计入 `orphans`），收到 `ack` 清掉取消表项，表项已被交叉回复清掉时忽略该 `ack`。`ack` 之后再收到回复是 `SESS-41` 的协议错误，结束会话；合规的被调方不会发。七种实现（Rust、Node、Bun、Python、Go、Swift、Kotlin）都确认并抑制回复；`docs/guarantees.md` 的 `bridge/cancel-reaches-callee` 给出承诺。`crates/rutis-bridge/src/session/rpc.rs:2273-2300,2952-2996,3384-3425`；`specs/call-lifecycle` 的 `call-gap-ack`、`call-mut-ack-then-reply`、`relay-healthy`、`relay-mut-ack-then-reply`

`SESS-62` **已取代。** “取消等于关闭承载”；“取消后会话进入不可用状态”。取消只作用于一次调用。

## 8. 畸形帧与失败范围

`SESS-63` **当前。** 协议错误默认故障闭合：本端认不出的帧、悬空的调用号或引用，结束整个会话（传输错误），不做部分解释。容忍的是未知 `cancel`（§7）、能看出所属调用的未知字段（§9）和“能看出所属调用”的过深帧（§8.2）。`crates/rutis-bridge/src/session/rpc.rs:2514-2561,2598-2618,2739-2741`

### 8.1 结束会话

| 输入 | 结果 | 锚点 |
| --- | --- | --- |
| 畸形的 JSON、空消息、开头 BOM、无效 UTF-8、不是对象 | 会话结束，传输错误 | `crates/rutis-bridge/src/session/rpc.rs:3207-3216`；`crates/rutis-bridge/src/session/testing.rs:151-192` |
| 未知 `op`；字段类型错；缺必需字段；未知线上值标签 | 会话结束，传输错误 | 同上；`crates/rutis-bridge/src/session/protocol.rs:154-156` |
| `throw.error` 缺 `name`/`message` | 会话结束，传输错误 | `crates/rutis-bridge/src/session/protocol.rs:87-94` |
| `throw.error` 带未知字段 | 只失败等待该回复的调用（`bad-frame`），会话继续 | §9；`crates/rutis-bridge/src/session/rpc.rs` 的 `reject_unknown_fields` |
| `call` / `get` / `await` 指向没有的引用 | `unknown or released reference` | `crates/rutis-bridge/src/session/rpc.rs:2504-2513,2699-2736` |
| `release` 指向没有的引用 | `release of unknown reference` | `crates/rutis-bridge/src/session/rpc.rs:2655-2676` |
| `release.count` 为 0 或大于授予次数 | `invalid reference release count` | 同上 |
| 方法调用或属性读取指向非对象引用 | `method call or property read on a non-object reference` | `crates/rutis-bridge/src/session/rpc/relay.rs:79-87` |
| 对未知调用的 `return` / `throw`（不是取消后的迟到回复） | `response for unknown call` | `crates/rutis-bridge/src/session/rpc.rs:2918-2943` |
| 字段类型不对的 `ack` | 结束会话，传输错误 | `MALFORMED_SEQUENCES`「ack of a wrong field type」 |
| 重复 `hello` | `duplicate protocol handshake` | `crates/rutis-bridge/src/session/rpc.rs:2602-2614` |
| 问候之前收到任何其他帧 | `request before protocol handshake` | `crates/rutis-bridge/src/session/rpc.rs:2616-2618` |
| 调用号重复、倒退、非数字、超过 2^53−1，或前缀不是对端 id | `invalid or repeated invocation identity` / `invalid invocation identity/path` | `crates/rutis-bridge/src/session/rpc.rs:1085-1095,2739-2742` |
| `path` 含本帧自己的 `id` | `invalid invocation identity/path` | 同上 |
| 帧的 JSON 嵌套超过 208 层，且看不出所属调用 | `frame nested deeper than 208 levels: …` | `crates/rutis-bridge/src/session/rpc.rs:2514-2561,3126-3134` |
| 任一方向的消息超过通道上限 | 承载关闭通道，会话以传输错误结束 | `crates/rutis-bridge/src/pipe/lines.rs:141-168`；`crates/rutis-bridge/src/websocket/connection.rs:29,87-99` |
| `chunk` 的 `seq`、`transfer`、`last`、`data` 不合规，对端未声明 `chunked`，重组超过 256 MiB，或嵌套 `chunk` | 会话结束，传输错误 | §2.5；`crates/rutis-bridge/src/session/chunks.rs`；`crates/rutis-bridge/src/session/rpc.rs` 的 `accept_chunk` |

### 8.2 只让所属调用失败

| 输入 | 结果 | 锚点 |
| --- | --- | --- |
| 进来的调用参数含本端不收的带标记值、格式不对的值、不合法的 map 键或 set 元素 | 该调用得到 `throw`，会话继续 | `crates/rutis-bridge/src/session/rpc.rs:2298-2324,2567-2596`；`docs/guarantees.md` 的 `bridge/value-tags` |
| 帧嵌套超过 208 层，但能看出所属调用：`invoke` / `call` / `get` / `await` 有本端前缀的 `id`，或 `return` / `throw` 有 `id` | 调用方收到 `throw` / 等待中的调用以值错误失败；会话继续 | `crates/rutis-bridge/src/session/rpc.rs:2514-2561`；测试 `::a_frame_too_deep_fails_its_call_and_the_session_goes_on` |
| 本端要发的参数或结果不能编码、对端不收该值、编码后超过通道上限 | 只让这次调用失败，帧不写出，授予回滚 | `crates/rutis-bridge/src/session/rpc/crossing.rs:143-246`；测试 `::a_message_over_the_limit_fails_only_its_call` |
| `return` 的值本端解不开（缺能力或格式不对） | 等待中的调用以值错误失败；会话继续 | `crates/rutis-bridge/src/session/rpc.rs:2620-2631` |
| 对不存在调用的 `cancel` | 忽略 | `crates/rutis-bridge/src/session/rpc/tests.rs:329-361` |
| 已取消调用的迟到回复 | 丢弃并计数，不结束会话 | `crates/rutis-bridge/src/session/rpc.rs:2490-2501` |

`SESS-64` **当前。** 共享的畸形帧矩阵有 27 个用例：`MALFORMED` 的 14 个单帧（语法、字段、`awaited` 类型错、未知引用、未知回复）和 `MALFORMED_SEQUENCES` 的 13 个多帧/顺序用例（握手前的请求、重复 `hello`、没有 `id` 的调用、未知引用的 `get`、已释放的引用、`release` 计数为 0 或超额、`cancel`/`ack`/`throw` 字段类型错、未知调用的 `throw`、`path` 含本帧 id、重复调用号）。用例要求会话以传输错误结束；除用例中有效调用欠下的回复外，对端不再回复。七种实现都有入口：Rust 走内存通道，其余走 Unix socket、loopback 或 WebSocket；Swift 的用例只在 macOS 上编译，由 CI 的 `runtimes-swift` 作业运行。`crates/rutis-bridge/src/session/testing.rs:151-392`；`crates/rutis-bridge/tests/runtime_conformance.rs:452-633`；`docs/ci.md`「一致性矩阵」

`SESS-65` **已取代。** “重复 `hello`、问候前的请求、`get` 未知引用、已释放引用、`release` 计数为 0 或超额、`cancel`/`ack`/`throw` 字段类型错、未知调用的 `throw`、`path` 含本帧 id、调用号不递增这些规则由代码执行，但没有进入七种实现的共享矩阵。”现状不是这样：清单里的规则都已进入 27 个用例的矩阵（`SESS-64`、§14 G2），调用号一项覆盖的是重复号。仍只在代码层验证的还有：方法调用或属性读取指向非对象引用；调用号倒退、非数字、超过 2^53−1 或前缀不是对端 id；`throw.error` 缺 `name`/`message`（矩阵只含 `error` 不是对象）；超过 208 层的帧；超通道上限的消息。

## 9. 未知字段

`SESS-66` **当前（Rust）。** 接收方不认识某个字段时，不结束会话，按帧的作用范围处理：

- `hello`：握手失败，会话结束，文字 `incompatible session: unknown field "x" in hello`；没有 `code`/`data`。
- `invoke`、`call`、`get`、`await`：只失败这次调用。接收方回 `throw`，错误码 `bad-frame`，message 为 `unknown field "x" in invoke`，data 为 `{"op":"invoke","fields":["x"]}`；其他调用和会话继续。`x` 是字段路径；嵌套值里的字段用 `args.value.x` 这样的路径。
- `return`、`throw`：只失败等待这次回复的调用，错误码 `bad-frame`，data 同上；会话继续。
- `release`、`cancel`：没有调用可失败，忽略多余字段，按已知字段处理。
- 握手前的调用、`id` 不是对端前缀、未知 `op`、缺 `id`、悬空回复/引用：仍按 §8 结束会话；无法定位调用的帧不能降级为调用级错误。

实现：`crates/rutis-bridge/src/session/rpc.rs` 的 `parse_frame` / `reject_unknown_fields`；测试 `unknown_frame_fields_fail_only_their_call`、`an_unknown_field_in_hello_fails_the_handshake`。`awaited` 是调用帧的已知字段（`SESS-83`），不是未知字段；它的类型不是布尔值时按本节的“字段类型错”结束会话。

`SESS-67` **当前（全部七种）。** Rust、Node、Bun、Python、Go、Swift、Kotlin 都已实现 `SESS-66` 的调用级拒绝；`bridge/unknown-fields` 的验证列列出各实现的用例。

`SESS-68` **已取代。** 两种旧行为：Rust 把未知字段当整帧畸形、结束会话；其他六种无差别忽略。两种都由 `SESS-66` 取代。

`SESS-69` **明确不保证。** 把未知字段当扩展机制；不改变含义的新可选字段不经协商就到达旧接收方；旧实现会按 `SESS-66` 失败该调用，不会静默忽略。能力声明不是字段扩展的替代：能力协商只能用已知的 `capabilities` 数组，不能让旧实现接受一个它不认识的字段。`throw.error.graph` 是当前已知字段，不属于未知字段扩展。

## 10. 消息大小、嵌套深度与出站积压

`SESS-70` **当前。** 消息大小上限是承载的规则，不是帧的规则。进程管道与 WebSocket 默认 16 MiB（16777216 字节）；WebSocket 的 `Limits.max_message` 可配置。内存通道没有上限。双方声明 `chunked` 时，超过上限的帧由 §2.5 的 `chunk` 帧承载，单帧上限变为会话的 `max_transfer`（默认 256 MiB，`SESS-88`）。`crates/rutis-bridge/src/pipe/lines.rs:13-28`；`crates/rutis-bridge/src/websocket/config.rs:104-108,132-141`；`crates/rutis-bridge/src/channel/mod.rs:32-42`

`SESS-71` **当前。** 会话在开场时记下 `Sender::max_message()`，并对它要写的每个帧先比较字节数。超限的帧不写出：调用以 `bad-value` 失败，`data` 指明部位（`part`，参数时还带 `argument`）和 `limit`，文字写明哪个调用、哪个参数或结果、多少字节；会话和其他调用继续。双方声明 `chunked` 时，比较的上限是会话的 `max_transfer`（默认 256 MiB），超过才失败（`SESS-88`）。`crates/rutis-bridge/src/session/rpc.rs` 的 `send_limit`、`serialized`；`crates/rutis-bridge/src/session/rpc/crossing.rs:104-113,221-246`；测试 `crates/rutis-bridge/src/session/rpc/tests.rs::a_message_over_the_limit_fails_only_its_call`、`::the_limit_and_the_call_are_named`；`docs/guarantees.md` 的 `bridge/bad-values`、`bridge/large-and-deep-values`

`SESS-72` **当前。** 承载层拒收超过自身上限的消息时，通道结束，会话随之以传输错误结束，在途调用全部以该错误失败，这条消息不被执行。进程管道在读写两侧都拒绝超限消息并关闭通道；WebSocket 以关闭码 1009 关闭。`crates/rutis-bridge/src/pipe/lines.rs:141-168`；`crates/rutis-bridge/src/websocket/connection.rs:29,87-99`；测试 `crates/rutis-bridge/tests/websocket.rs:662-690`

`SESS-73` **明确不保证。** “超过上限只让这次调用失败”。本端能在写出前检查的帧只失败该调用；对端的上限更小、对端不遵守本协议、或消息在承载层被拒时，通道与会话会结束，在途调用一起失败。`docs/guarantees.md` 的 `bridge/bad-values` 把这一点列为明确不保证。

`SESS-74` **当前。** 发送方允许的值嵌套上限是 64 层（数组和对象各算一层，参数列表不算一层）；超过的值在发出前拒绝，只让这次调用失败，错误说明 `the value nests deeper than 64 levels`。线上的帧最多允许 208 层，给编码留余量；超过的帧按 §8 处理。`crates/rutis-bridge/src/session/rpc.rs:3119-3148`；测试 `::a_value_deeper_than_the_limit_fails_its_call_before_it_is_sent`；`docs/guarantees.md` 的 `bridge/large-and-deep-values`

`SESS-75` **当前。** 出站积压是另一条限制：`SessionOptions::outbound_limit` 限制写线程已经持有一个帧之后还在排队的字节；空队列无论上限（包括 0）都接受一个帧。超过时会话以 `the far end is not reading: <n> messages, <bytes> bytes waited for it` 结束（`peer-gone`），排队的消息一起消失。`crates/rutis-bridge/src/session/rpc.rs:1186-1214,1387-1404`；测试 `crates/rutis-bridge/src/session/rpc/tests.rs:618-741`

`SESS-76` **明确不保证。** 默认 `outbound_limit` 是 `None`：没有上限，一个不读的对端会让本进程为它保留任意多的已排队字节，直到对端重新读取或会话结束。宿主可以不开启这个上限；开启后按 `SESS-75` 结束。`crates/rutis-bridge/src/session/rpc.rs:1186-1214`；`docs/formal-trace.md` §3.6

`SESS-82` **明确不保证。** 收到的值都在 64 层以内。上限约束的是发送；从不遵守上限的对端收到的值可能超过 64 层（线上仍有 208 层的帧上限）。

`SESS-88` **当前。** 一次传输重组后的上限是会话的 `max_transfer`：默认 256 MiB，宿主可用 `SessionOptions::max_transfer` 按会话设置，或用 `RUTIS_MAX_TRANSFER` 设置一个部署值（宿主把它传给启动的运行时进程）；不做无上限，也不静默截断。发送方超过它按 `SESS-71` 只让这次调用失败，`limit` 是配置值；接收方超过它以传输错误结束会话并说出配置值。同一时刻至多一个未完成的传输，第二个 `transfer` 是协议错误——这就是并发未完成传输的上界。**明确不保证**：两端配置的上限一致；发送方按本端上限分片，接收方按自己的上限重组，超过的一端以传输错误结束会话。分片不改变 `SESS-70`–`SESS-73` 的其余部分：通道仍按自己的上限拒收单条消息，`chunk` 帧本身必须装得下；内存通道没有承载上限，因此不会分片，但会话自己的 `max_transfer` 仍然约束整帧——没有无上限的会话。一个传输在出站积压里记作一条消息，字节数按原帧计（`SESS-75` 不变）。**明确不保证**：没有声明 `chunked` 的两端之间，超过通道上限的值仍然传不过去（行为与 0.22 相同）；重组要求对端把整帧收进内存，峰值约为帧大小的 1.4 倍；`last` 永远不来的传输不产生任何调用。`crates/rutis-bridge/src/session/chunks.rs` 的 `MAX_TRANSFER`；`crates/rutis-bridge/tests/value_roundtrip.rs::node_carries_frames_over_the_message_limit_whole`；`docs/guarantees.md` 的 `bridge/large-and-deep-values`

## 11. 规则状态

下表是本文的规则清单，供 #634 追踪表与 #613 模型抽取。每条“当前”都有代码或测试锚点；“明确不保证”集中登记，避免被误读成保证。

| 编号 | 规则 | 状态 | 证据 |
| --- | --- | --- | --- |
| `SESS-1` | 会话是一条有序、可靠、双工、保留边界的消息流 | 当前 | `crates/rutis-bridge/src/channel/mod.rs:1-9,20-48` |
| `SESS-2` | 每条消息一个 UTF-8 JSON 帧，按到达顺序一次一帧处理 | 当前 | `crates/rutis-bridge/src/session/rpc.rs:1557-1570,3207-3216` |
| `SESS-3` | 协议 5，七种实现同一常量，WebSocket 子协议 `rutis.5` | 当前 | `crates/rutis-bridge/src/session/mod.rs:9-15` 与各实现常量 |
| `SESS-4` | 两个端点 id，调用号带本端前缀 | 当前 | `crates/rutis-bridge/src/channel/mod.rs:77-109`；`crates/rutis-bridge/src/session/rpc.rs:1493-1499` |
| `SESS-5` | 重连是新会话，旧状态不延续 | 当前 | `docs/formal-trace.md` §3.0；`crates/rutis-bridge/tests/session_plugin.rs:182-270` |
| `SESS-6` | 十种 `Frame` 变体，加解码前的 `chunk` 帧（§2.5） | 当前 | `crates/rutis-bridge/src/session/protocol.rs:149-230`；`crates/rutis-bridge/src/session/chunks.rs` |
| `SESS-7` | `sync` 与可选字段的缺省省略 | 当前 | `crates/rutis-bridge/src/session/protocol.rs:157-221` |
| `SESS-8` | `path` 是调用链，条目是调用号 | 当前 | `crates/rutis-bridge/src/session/rpc.rs:2739-2742` |
| `SESS-9` | `WireValue` 的标签集合 | 当前 | `crates/rutis-bridge/src/session/protocol.rs:26-85` |
| `SESS-10` | `undefined` 作为整体保留，值内为 `null` | 当前 | `crates/rutis-bridge/src/session/rpc.rs:2356-2359,2567-2573` |
| `SESS-11` | 带标记值只发给声明了 `value-<tag>` 的对端 | 当前 | `crates/rutis-bridge/src/session/rpc.rs:2204-2214` |
| `SESS-12` | 只有端点格式、协议 5 | 当前 | `docs/migration-0.23.md`「一起升级」；`crates/rutis-bridge/src/session/rpc.rs:2456-2462` |
| `SESS-13` | 两种格式（compat 2、endpoint 3） | 已取代 | `docs/migration-0.19.md`「一起升级」 |
| `SESS-14` | `sync` 只发给声明 `sync-wait` 的对端 | 已取代 | 协议 4 无此能力；`crates/rutis-bridge/src/session/protocol.rs:174-176` |
| `SESS-15` | 能力声明及其被读取的范围（含 `chunked`） | 当前 | `crates/rutis-bridge/src/session/rpc.rs:1154-1178,1590-1611`；`crates/rutis-bridge/src/session/plugin/greet.rs:31-36` |
| `SESS-16` | `sync-wait`、`events-v2`、`objects` 能力 | 已取代 | 实现无此常量；§6.2 |
| `SESS-17` | 未知能力名忽略；声明不被验证；能力不是权限 | 明确不保证 | §2.4 |
| `SESS-18` | 帧按到达顺序一次一帧；调用可并发 | 当前 | `crates/rutis-bridge/src/session/rpc.rs:1557-1570,2792-2839`；`go/rutis/internal/peer/peer_test.go:55-58` |
| `SESS-19` | 事件操作按帧序进入有序队列 | 当前 | `crates/rutis-bridge/src/session/rpc.rs:1244-1283,2767-2884` |
| `SESS-20` | 双方先 `hello` 再读 | 当前 | `crates/rutis-bridge/src/session/rpc.rs:1524-1541` |
| `SESS-21` | 问候前的请求结束会话 | 当前 | `crates/rutis-bridge/src/session/rpc.rs:2616-2618`；Go `TestACallBeforeTheHandshakeEndsTheSession` |
| `SESS-22` | 重复 `hello` 结束会话 | 当前 | `crates/rutis-bridge/src/session/rpc.rs:2602-2614` |
| `SESS-23` | 版本不匹配时说明升级哪一端 | 当前 | `crates/rutis-bridge/src/session/rpc.rs:1224-1242,2456-2462`；`bridge/version-refusal` |
| `SESS-24` | 端点 id 必填、格式与身份核对 | 当前 | `crates/rutis-bridge/src/session/rpc.rs:2463-2479`；`crates/rutis-bridge/src/channel/mod.rs:77-109` |
| `SESS-25` | 必需能力缺失时拒绝握手 | 明确不保证 | 没有实现提供声明必需能力的入口：`crates/rutis-bridge/src/session/plugin/greet.rs:12-19`；`go/rutis/internal/peer/peer.go:111-114` |
| `SESS-26` | 10 秒问候时限 | 当前 | `crates/rutis-bridge/src/session/plugin/mod.rs:46-47`；`crates/rutis-bridge/tests/session_plugin.rs:130-154` |
| `SESS-27` | `ready` 前发送被拒 | 当前 | `crates/rutis-bridge/src/session/rpc.rs:1844-1848` |
| `SESS-28` | 握手成功不挂载 | 当前 | `crates/rutis-bridge/src/session/plugin/mod.rs:102-153,178-184` |
| `SESS-29` | 握手失败关闭会话并撤销等待 | 当前 | `crates/rutis-bridge/src/session/rpc.rs:1629-1682` |
| `SESS-30` | 握手前登记/挂载、`ready` 前成功调用 | 已取代 | `docs/formal-trace.md` §1 |
| `SESS-31` | 精确的墙上时钟时限；重试策略 | 明确不保证 | §4 |
| `SESS-32` | 调用号从 1 起严格递增，上限 2^53−1 | 当前 | `crates/rutis-bridge/src/session/rpc.rs:1849-1859,1085-1095` |
| `SESS-33` | 调用号的前缀、递增与 `path` 自指检查 | 当前 | `crates/rutis-bridge/src/session/rpc.rs:1085-1095,2739-2742` |
| `SESS-34` | 被拒绝的调用仍占用调用号 | 当前 | `crates/rutis-bridge/src/session/rpc.rs:2576-2593` |
| `SESS-35` | 收到调用时把自己的 `id` 追加到 `path` | 当前 | `crates/rutis-bridge/src/session/rpc.rs:2739-2742` |
| `SESS-36` | 转发时按会话标签 `rebase` | 当前 | `crates/rutis-bridge/src/session/rpc.rs:849-902`；`bridge/call-chains-across-processes` |
| `SESS-37` | 标签唯一性 | 明确不保证 | [会话标签设计](design-session-tags-2026-10-12.md) §四 |
| `SESS-38` | `sync` 的同步等待与链上反向调用 | 当前 | `crates/rutis-bridge/src/session/rpc.rs:1862-1866,2000-2019,2792-2839` |
| `SESS-39` | 等待成环时拒绝 `SyncWaitCycle` | 当前 | `crates/rutis-bridge/src/session/rpc.rs:1867-1879`；`bridge/sync-wait-cycle-refused` |
| `SESS-40` | 未标 `sync` 的调用、跨主机环 | 明确不保证 | `docs/guarantees.md` 的 `bridge/sync-wait-cycle-refused` |
| `SESS-41` | 每次调用恰好结算一次；未知回复结束会话 | 当前 | `crates/rutis-bridge/src/session/rpc.rs:2490-2503` |
| `SESS-42` | 会话层不自动重试 | 当前 | `crates/rutis-bridge/src/session/rpc.rs:1982,1085-1095`；`bridge/in-flight-call-fails-once` |
| `SESS-43` | 业务错误只影响该调用；错误图形状不结束会话 | 当前 | `crates/rutis-bridge/src/session/rpc.rs:2633-2638,3029-3083`；`bridge/bad-values` |
| `SESS-44` | 编码/值/上限失败只让该调用失败并回滚授予 | 当前 | `crates/rutis-bridge/src/session/rpc/crossing.rs:143-246` |
| `SESS-45` | 调用截止时间与并发调用顺序 | 明确不保证 | `docs/design-protocol-plugin-mount.md` §3.1 |
| `SESS-46` | 引用的线格式、身份与同一对象同一引用 | 当前 | `crates/rutis-bridge/src/session/protocol.rs:42-47`；`crates/rutis-bridge/src/session/rpc.rs:3095-3107` |
| `SESS-47` | 授予计数与计数释放 | 当前 | `crates/rutis-bridge/src/session/rpc.rs:992-1010,2655-2676`；`bridge/release-count` |
| `SESS-48` | 未知引用、非对象引用、非法 `release` 结束会话 | 当前 | `crates/rutis-bridge/src/session/rpc.rs:2504-2513,2655-2676,2699-2736` |
| `SESS-49` | 引用 id 范围、分配与授予溢出 | 当前 | `crates/rutis-bridge/src/session/rpc.rs:2397-2398,3085-3108` |
| `SESS-50` | 回到 home 的引用还原身份；未知导出结束会话 | 当前 | `crates/rutis-bridge/src/session/rpc.rs:2372-2390` |
| `SESS-51` | 会话关闭清空引用表 | 当前 | `crates/rutis-bridge/src/session/rpc.rs:1663-1664` |
| `SESS-52` | 对象按引用传，所有协议 4 实现都收 | 当前 | `crates/rutis-bridge/src/session/rpc.rs:2690-2736`；`bridge/object-refs` |
| `SESS-53` | 对象只发给声明 `objects` 的对端 | 已取代 | 协议 4 无此能力；§6.2 |
| `SESS-54` | 中继转发与逐跳释放 | 当前 | `crates/rutis-bridge/src/session/rpc/relay.rs:19-78,93-175` |
| `SESS-55` | 引用不让运行时保持存活 | 明确不保证 | `docs/formal-trace.md` §1 |
| `SESS-56` | 放弃异步等待时发 `cancel` | 当前 | `crates/rutis-bridge/src/session/rpc.rs:2020-2049` |
| `SESS-57` | Rust 被调方的取消处理与迟到回复 | 当前 | `crates/rutis-bridge/src/session/rpc.rs:2639-2653,2490-2501` |
| `SESS-58` | 未知 `cancel` 被忽略 | 当前 | `crates/rutis-bridge/src/session/rpc.rs:2639-2653` |
| `SESS-59` | 取消逐跳传播，不撤销已授予引用 | 当前 | `crates/rutis-bridge/src/session/rpc/relay.rs:148-173`；`bridge/multihop-cancel-and-release` |
| `SESS-60` | 取消不关闭会话 | 当前 | `crates/rutis-bridge/src/session/rpc.rs:2639-2653` |
| `SESS-61` | 取消恰好结算一次：`ack` 且不再回复，或取消读到前已写出的回复；调用方接受任一种 | 当前 | §7；#1099；`specs/call-lifecycle` |
| `SESS-62` | 取消等于关闭承载 | 已取代 | §7 |
| `SESS-63` | 畸形输入故障闭合 | 当前 | `crates/rutis-bridge/src/session/rpc.rs:2514-2561,2598-2618`；`crates/rutis-bridge/src/session/testing.rs:151-392` |
| `SESS-64` | 共享畸形帧矩阵 27 个用例（14 单帧 + 13 多帧/顺序），七种实现都有入口 | 当前 | `crates/rutis-bridge/src/session/testing.rs:154-370`；`crates/rutis-bridge/tests/runtime_conformance.rs:452-633` |
| `SESS-65` | 旧状态“这些规则未进共享矩阵”；27 个用例的矩阵已覆盖该清单（`SESS-64`） | 已取代 | §8.1；§14 G2 |
| `SESS-66` | Rust：未知字段只失败该调用（`hello` 失败握手，`release`/`cancel` 忽略） | 当前 | `crates/rutis-bridge/src/session/rpc.rs` 的 `parse_frame` / `reject_unknown_fields`；`rpc/tests.rs::unknown_frame_fields_fail_only_their_call`、`::an_unknown_field_in_hello_fails_the_handshake` |
| `SESS-67` | 七种运行时都实现调用级拒绝（`bad-frame`）；`release`/`cancel` 忽略多余字段 | 当前 | `docs/guarantees.md` 的 `bridge/unknown-fields`；§9；§14 G1 |
| `SESS-68` | 未知字段结束整个会话 / 六种无差别忽略 | 已取代 | §9；§14 G1 |
| `SESS-69` | 未知字段可作扩展机制 | 明确不保证 | §9 |
| `SESS-70` | 16 MiB 默认上限；内存通道无上限；双方声明 `chunked` 时单帧上限为会话的 `max_transfer`（默认 256 MiB） | 当前 | `crates/rutis-bridge/src/pipe/lines.rs:13-28`；`crates/rutis-bridge/src/websocket/config.rs:104-108,132-141` |
| `SESS-71` | 发送前检查，超限只让该调用失败（声明 `chunked` 时上限为 `max_transfer`） | 当前 | `crates/rutis-bridge/src/session/rpc.rs`；`crates/rutis-bridge/src/session/rpc/crossing.rs:221-246` |
| `SESS-72` | 承载层超限结束通道与会话 | 当前 | `crates/rutis-bridge/src/pipe/lines.rs:141-168`；`crates/rutis-bridge/src/websocket/connection.rs:29,87-99` |
| `SESS-73` | 超限只让这次调用失败 | 明确不保证 | §10 |
| `SESS-74` | 值 64 层、帧 208 层 | 当前 | `crates/rutis-bridge/src/session/rpc.rs:3119-3148`；`bridge/large-and-deep-values` |
| `SESS-75` | 出站积压上限 | 当前 | `crates/rutis-bridge/src/session/rpc.rs:1186-1214` |
| `SESS-76` | 默认出站积压无界 | 明确不保证 | `crates/rutis-bridge/src/session/rpc.rs:1186-1214`；`docs/formal-trace.md` §3.6 |
| `SESS-77` | 同一规则的错误文字逐字一致 | 明确不保证 | §12；Node、Go 与 Rust 的文字不同 |
| `SESS-78` | `hello` 的 `implementation` 可缺省，接收方不要求它 | 当前 | `crates/rutis-bridge/src/session/protocol.rs:157-167`；`crates/rutis-bridge/src/session/rpc.rs:2450-2489` |
| `SESS-79` | 协议 5 只与协议 5 通信；0.22 及更早被拒 | 当前 | `docs/migration-0.23.md`「一起升级」；`bridge/cross-version-meetings` |
| `SESS-80` | 新增能力不升版本；新增帧只能在当前协议版本发布前加入，否则升版本 | 当前 | `crates/rutis-bridge/src/session/protocol.rs:154-156,210-226`；§13；`SESS-66` |
| `SESS-81` | 相邻版本之外的跨版本相遇 | 明确不保证 | `bridge/cross-version-meetings` 只承诺相邻发布版本 |
| `SESS-82` | 收到的值都在 64 层以内 | 明确不保证 | §10；`bridge/large-and-deep-values` |
| `SESS-83` | `awaited` 区分发出即不管与之后等待；缺省按会等 | 当前 | §5.3；`crates/rutis-bridge/src/session/protocol.rs`；`crates/rutis-bridge/src/session/rpc/waits.rs`；`bridge/sync-wait-cycle-refused` |
| `SESS-84` | `chunk` 帧的字段与解码位置（解码前，不是 `Frame` 变体） | 当前 | §2.5；`crates/rutis-bridge/src/session/chunks.rs` 的 `Chunk` |
| `SESS-85` | `chunked` 能力；双方声明才分片；Rust 与 Node 声明 | 当前 | §2.5；`crates/rutis-bridge/src/session/rpc.rs` 的 `send_limit`；`node/rutis-runtime/src/session.mjs` 的 `#chunking` |
| `SESS-86` | 发送：每片至多 8 MiB 且装得下通道；片连续；超过 `max_transfer` 只让该调用失败 | 当前 | §2.5；`crates/rutis-bridge/src/session/chunks.rs` 的 `slice_size`；测试 `a_frame_over_the_limit_is_chunked_to_a_far_end_that_declared_it` |
| `SESS-87` | 接收：按 `transfer` 与 `seq` 缓冲，至多一个未完成传输，`last` 后才派发；未完成随会话丢弃；违规结束会话 | 当前 | §2.5；`crates/rutis-bridge/src/session/chunks.rs` 的 `Reassembly`；`node/rutis-runtime/src/session.mjs` 的 `#receiveChunk` |
| `SESS-88` | 重组上限为可配置的 `max_transfer`（默认 256 MiB）；积压记作一条消息；未声明 `chunked` 的两端行为不变 | 当前 | §10；`crates/rutis-bridge/tests/value_roundtrip.rs::node_carries_frames_over_the_message_limit_whole` |

## 12. 开发者看到什么

### 插件作者

**成功：** 插件 API、值映射、对象和取消行为不变。普通调用、对象方法、函数引用和异步结果照常工作；没有新增配置字段、没有新的输出行，也不需要学习“帧”或“调用号”。

**失败（逐字，Rust 一侧）：**

| 情况 | 输出 |
| --- | --- |
| 对端版本更旧 | `incompatible session: the far end speaks protocol 4, this side 5: upgrade the far end` |
| 对端版本更新 | `incompatible session: the far end speaks protocol 99, this side 5: upgrade this side (Rust crates rutis-host / rutis-bridge)` |
| 对端没在 10 秒内问候 | `the far end did not greet in time` |
| 宿主没有会话 id | `the host provides no id for its sessions` |
| 值超过消息上限 | 对端没有声明 `chunked`（Bun、Python、Go、Swift、Kotlin）时：`svc.take: argument 2 cannot be sent: the message would be over the limit of 16777216 bytes (16 MiB)`；错误码 `bad-value`；`data` 是 `{"argument":2,"limit":16777216,"part":"argument"}`。对端声明了 `chunked`（0.23 Node 运行时）时，同一值照常跨过，直到 256 MiB |
| 值编码后超过会话上限（默认 256 MiB，宿主可调大） | `svc.take: argument 2 cannot be sent: the message would be 300000000 bytes, over the limit of 268435456 bytes (256 MiB)`；错误码 `bad-value`；`data` 是 `{"argument":2,"limit":268435456,"part":"argument"}` |
| 对象所在的运行时退出 | `a call on an object from py failed: runtime py exited`；错误码 `peer-gone` |

每次调用级失败都在调用或值这一层结束。握手/承载失败通过同时升级两端、核对部署里的端点 id、去掉超限值或调整上限来解决。**不要按错误文字分支**：按 `code`，或按 `Error::Handshake` / `Error::Transport` 的类别（`SESS-77`）。

**需要学习的概念：** 无新增。帧、调用号、授予/释放是运行时与贡献者的词汇，不进入插件 API。参数或返回值超过 16 MiB 也不需要分页：对端声明 `chunked` 时（0.23 Node 运行时）它们照常跨过，最多 256 MiB。

### 宿主作者与部署者

**成功：** 现有 `rutis-host` 命令、状态行、退出码、关闭码和重试策略不变。WebSocket 子协议是 `rutis.5`；传输关闭码仍是 `1000`（正常）、`4002`（被顶替）、`4003`（被拒绝）、`1009`（消息超限，WebSocket）。

**失败（逐字或类别）：**

- 会话在握手阶段被拒：错误是 `incompatible session: …`，说明升级哪一端；拨出方把它归为 `incompatible`，停止重试直到配置改变。
- 身份不符：`endpoint mismatch: …`，拨出方归为 `auth-rejected`，慢速重试并继续报告。
- 可重试的承载失败：`retryable`，按退避重连；新连接是新会话。
- 传输中途断开：会话以传输错误结束，未完成的请求不派发、不回复；在途调用以 `peer-gone` 失败。已声明 `chunked` 的两端之间，超过 16 MiB 的参数和返回值直接可用，同一次调用只执行一次；没有声明 `chunked` 的对端行为与 0.22 相同。
- 协议错误：会话以传输错误结束，在途调用带 `peer-gone` 失败，旧会话的迟到帧不会作用到替换后的会话。

**需要学习的概念：** 无新增；调整最大帧时知道 `max_transfer` / `RUTIS_MAX_TRANSFER` 一个旋钮即可。部署者需要知道的是：同一部署里端点 id 唯一；协议 5 的所有端要一起升级（§13）。

### 运行时作者与贡献者

**成功：** 运行七种实现的一致性套件：`RUTIS_TEST_RUNTIMES=node,bun,python,go,swift,kotlin cargo test -p rutis-bridge --all-features --test runtime_conformance`。它逐条运行 `session::testing::session` 与 `malformed`，并比对 `docs/ci.md`「一致性矩阵」的格子。

**失败：** 畸形帧用例逐条打印是哪一条消息；规则类别见 §8。错误文字可以与其他实现不同（`SESS-77`），但必须仍以传输错误结束会话，或只让所属调用失败，范围与 §8 一致。

**需要学习的概念：** 帧、`op`、调用号、`path`、引用授予/释放、调用级失败、会话级失败、能力声明、`chunk` 帧与 `chunked` 能力。它们只用于实现与验证，不进入用户 API。

## 13. 兼容性与迁移

`SESS-79` **当前。** 协议 5 只与协议 5 通信。0.22 及更早的宿主、运行时、节点和 SDK 在握手时被拒；迁移是把所有端一起升级到 0.23 或更高。`docs/migration-0.23.md`「一起升级」；`docs/guarantees.md` 的 `bridge/cross-version-meetings`

`SESS-80` **当前。** 新增能力不需要提升协议版本：发送方按 `value-<tag>` 判断，对端不声明就不发。由于旧实现会失败带未知字段的调用（`SESS-66`）、拒绝未知 `op`，新增**帧**或**改变已有字段含义**只能在发布前加入当前协议版本，或提升版本，并同时更新本文、`Frame`/`WireValue`、畸形帧矩阵和各实现。`chunk` 帧（`SESS-84`）与 `awaited`（`SESS-83`）都是在协议 5 发布前加入的，因此不升版本；实现必须先能收发它，再声明 `chunked`。新增能力不必提升版本，只按声明使用（§2.4）；没有“必需能力”可协商（`SESS-25`）。

`SESS-81` **明确不保证。** 相邻版本之外的跨版本相遇；新旧实现混用；不带版本号的新字段。0.23 的协议 5 在发布前加入了 `awaited`（`SESS-83`）和 `chunk` 帧（`SESS-84`）：见过更早协议 5 预发布构建的对端会按 `SESS-66` 失败带 `awaited` 的调用；更早的构建不声明 `chunked`，所以不会被分片，也不会收到它不认识的 `chunk` 帧。必须一起升级到含它们的 0.23 构建。`docs/guarantees.md` 的 `bridge/cross-version-meetings` 只承诺相邻、其间没有不兼容变更的两个发布版本。

## 14. 规格缺口与待决定

### G1 未知字段：已定（2026-11-09）

- 决定：采用 B（拒绝），但按帧的作用范围处理，不结束整个会话：`hello` 失败握手；`invoke`/`call`/`get`/`await` 只失败该调用；`return`/`throw` 只失败等待它的调用；`release`/`cancel` 忽略。错误码 `bad-frame`，data `{op, fields}`。
- 已完成（2026-11-10）：七种实现都实现了调用级拒绝（`SESS-67`）；`bridge/unknown-fields` 的验证列列出各实现的用例，Rust 的测试见 `unknown_frame_fields_fail_only_their_call`、`an_unknown_field_in_hello_fails_the_handshake`、`release_and_cancel_ignore_unknown_fields`。计划 G11 关闭。
- 未知字段只失败一条调用，不属于会话结束的 `MALFORMED` 矩阵；各实现的调用级测试就是证据。
- 旧 A（全部忽略）和 C（白名单）不再采用。

### G2 共享畸形矩阵：已补齐（2026-11-11）

- 决定（#635）：把只由代码执行的畸形规则放进 `session::testing::malformed` 的共享矩阵，让七种实现跑同一组用例。
- 已完成：矩阵从 13 个用例增加到 26 个，`MALFORMED` 13 个单帧、`MALFORMED_SEQUENCES` 13 个多帧/顺序（`SESS-83` 又加了一个 `awaited` 类型错的单帧，现为 27 个）；原清单（握手前的请求、重复 `hello`、对未知引用的 `get`、已释放的引用、`release` 计数为 0 或超额、`cancel`/`ack`/`throw` 字段类型错、未知调用的 `throw`、`path` 含本帧 id、调用号不递增）中，除调用号只覆盖重复号外全部覆盖，另加没有 `id` 的调用（`SESS-21`、`SESS-22`、`SESS-33`、`SESS-47`、`SESS-48`；`SESS-64`）。协议 5 的矩阵加了字段类型不对的 `ack`。实现见 `crates/rutis-bridge/src/session/testing.rs` 的 `MALFORMED`、`MALFORMED_SEQUENCES`、`malformed`；Swift 的用例只在 macOS 上编译，由 CI 的 `runtimes-swift` 作业运行。
- 仍不在矩阵内：方法调用或属性读取指向非对象引用；调用号倒退、非数字、超过 2^53−1 或前缀不是对端 id；`throw.error` 缺 `name`/`message`；超过 208 层的帧；超通道上限的消息；`chunk` 帧的畸形顺序与上限（只有 Rust 与 Node 实现分片，见 G8）。这些由代码和其他测试验证（§8.1、§8.2、§2.5）。

### G3 取消后的回复规则：已定（2026-11-16；2026-11-18 由 #1099 改为协议 5）

- 已取代（2026-11-18，#1099）：“不把回复或不回复任一种定为跨实现要求”是协议 4 的状态。协议 5 让被调方恰好结算一次：回 `ack` 且不再回复，或取消读到之前已经写出的回复。调用方在任一种上清掉取消表项；表项已不在时收到的 `ack` 被忽略；`ack` 之后的回复是未知调用并结束会话（`SESS-61`）。
- 原因：调用方的表只能由回复清理，而排队中被取消的调用没有合规实现会回复，长会话反复取消排队调用就会一直涨（#1099；`specs/call-lifecycle` 的 `call-gap-cancel-table`）。有界表是退路；取消确认让表在静止时为空，且不必放宽 `SESS-41`。
- 取消读到之前发生的两种回复形状仍然合法；#626 的模型跑它们（`call-cancel-reply`、`call-cancel-no-reply`），变异 `call-mut-ack-then-reply` 说明“确认”和“此后不答复”是同一条规则。

### G4 旧文档里已不存在的能力与格式

- [协议插件设计](design-protocol-plugins-2026-09-25.md) §6.1 的“对象与能力 `objects`”一段仍按 0.18 的兼容握手写；当前实现没有 `objects` 能力（`SESS-16`、`SESS-53`）。
- issue #635 与[网络栈设计](design-protocol-channel-decoupling-2026-10-03.md) 仍提到 `sync-wait`、`events-v2` 和两种格式；协议 5 都没有（`SESS-13`、`SESS-16`）。

这两处需要后续的文档修订；本文只把它们标为已取代，不改旧设计正文。

### G5 缺少测试的规则：已关闭（2026-11-11）

- `SESS-67`：七种实现都有调用级拒绝的测试（`bridge/unknown-fields` 的验证列）；计划 G11 关闭。
- `SESS-22`（重复 `hello`）、`SESS-21`（问候前的请求）和 `SESS-33` 的重复调用号、`path` 自指已进入共享矩阵（`SESS-64`）；其余调用号形式见 G2 的“仍不在矩阵内”。

### G6 错误文字的差异

同一类错误在 Rust、Node、Go 里的句子不同（`SESS-77`）。本文只在 §12 和 §8 逐字引用 Rust 的文字；验证器应比较类别、范围和错误码，不比较文字。调用号的数字格式也有同类差异（`SESS-33`）：Rust 与 Python 宽松，其余实现严格。若以后要统一文字或格式，那是另一项行为变更。

### G7 异步调用的等待信号：已定（2026-11-19，#1098）

- 决定：调用帧加可选布尔字段 `awaited`，缺省 `true`（“结果会被等待”），只有“发出即不管”写 `false`。它并入协议 5，不升版本：协议 5 尚未发布（0.23 未发布），所有实现一起升级；见过 0.23 预发布构建的对端按 `SESS-66` 会把 `awaited` 当未知字段失败该调用，必须升级到含本字段的构建。
- 原因：协议 5 之前帧上只有 `sync`，宿主无法区分“发出即不管”和“之后 await”。一律把已标记链上的异步调用记成一跳会误拒不会卡死的调用（`specs/cross-runtime-sync` 的反例 8，`async-policy-b`）；只把 `sync` 当跳会漏掉对象中继的同步环（真实死锁）。`awaited: false` 让调用方明确说“不等”，把这条调用排除出边；缺省 `true` 保留旧保守语义，不会漏边。
- 不采用：后续 `await` 帧作为信号（环在 `await` 之前就已形成，太晚）；能力协商（能力不能扩展字段，`SESS-69`）；把 `sync` 的含义改成“会被等待”（`sync` 还决定在哪个线程上等，改了会动 `SESS-38`）。
- 语义边界：`awaited: false` 是信任声明，宿主不验证；说假话会漏掉真实等待边（`SESS-40` 的明确不保证里）。`sync: true` 时忽略 `awaited`。接收方转发时沿用原帧的信号。
- 验证：`crates/rutis-bridge` 的 `an_asynchronous_forward_nobody_awaits_is_an_entry`、`an_awaited_forward_closes_the_cycle`、`a_forwarded_call_carries_the_callers_awaited_signal`、`a_call_says_when_its_result_is_not_waited_for`；七种实现的共享畸形矩阵加了 `awaited` 类型错；`specs/cross-runtime-sync` 的 `async-forgotten-policy-b`（修复后通过）与 `async-policy-b`（修复前反例 8）。

### G8 跨帧的整帧：已定（2026-11-19，#1361）

- 决定：加 `chunk` 帧与 `chunked` 能力（`SESS-84`–`SESS-88`），先实现 Rust（`rutis-bridge`）与 Node 运行时；协议 5 未发布，不升版本。重组上限是会话选项 `max_transfer`（默认 256 MiB），宿主可经 `RUTIS_MAX_TRANSFER` 把它传给运行时进程；不做无上限。设计与开发者可见的输出见[跨帧传输超过单条消息上限的参数和返回值](design-chunked-frames-2026-11-19.md)。
- 原因：跨语言调用超过单条消息上限时，业务方只能自己切片、分页、重试；分片放在会话层，业务 API、服务签名和公开 API 都不变，同一次调用只执行一次。
- 不采用：升协议版本（0.22 与 0.23 本就不能混跑）；只提高承载上限（改不了其他实现的默认值）；把分片放进 `args`（要改 `WireValue` 与七种实现的值编解码）；并发传输（接收端排序与内存峰值都翻倍）；无重组上限（不守协议的对端可以让本进程分配任意内存）。
- 其余五种实现（Bun、Python、Go、Swift、Kotlin）不声明 `chunked`，行为与 0.22 相同；实现后补声明即可，不改业务 API。
- 验证：`crates/rutis-bridge` 的 `a_chunked_call_is_dispatched_once_after_its_last_chunk`、`an_incomplete_transfer_is_never_dispatched`、`a_chunk_out_of_order_ends_the_session`、`a_second_transfer_before_the_first_ends_ends_the_session`、`a_chunk_before_the_handshake_ends_the_session`、`a_chunk_from_a_far_end_without_the_capability_ends_the_session`、`a_chunk_with_invalid_base64_ends_the_session`、`a_frame_over_the_limit_is_chunked_to_a_far_end_that_declared_it`；真实 Rust↔Node：`crates/rutis-bridge/tests/value_roundtrip.rs::node_carries_frames_over_the_message_limit_whole`；Node 自己的 `node/rutis-runtime/test/chunked.test.mjs`。

## 相关文件

- [会话追踪契约](formal-trace.md)
- [跨帧传输超过单条消息上限的参数和返回值](design-chunked-frames-2026-11-19.md)
- [迁移到 0.19](migration-0.19.md)
- [迁移到 0.23](migration-0.23.md)
- [协议插件设计](design-protocol-plugins-2026-09-25.md) §6.1（带标记值的线上格式）
- [跨进程挂载设计](design-protocol-plugin-mount.md)
- [会话标签设计](design-session-tags-2026-10-12.md)
- [对象引用](design-object-refs-2026-10-27.md)
- [错误消息](design-error-messages-2026-11-03.md)
- [保证登记表](guarantees.md) 的 `bridge/*`
- [CI](ci.md)「一致性矩阵」
- [0.22 形式化覆盖执行计划](plan/formal-verification-0.22.md) WS2
- `crates/rutis-bridge/src/session/protocol.rs`
- `crates/rutis-bridge/src/session/rpc.rs`
- `crates/rutis-bridge/src/session/testing.rs`
