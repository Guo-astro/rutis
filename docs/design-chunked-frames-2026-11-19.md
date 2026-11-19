# 跨帧传输超过单条消息上限的参数和返回值

[English](design-chunked-frames-2026-11-19.en.md)

状态：已实现（0.23 未发布，并入会话协议 5）；#1363 让 Bun 与 Kotlin 也实现。日期：2026-11-19。
议题：[#1361](https://github.com/arcships/rutis/issues/1361)、[#1363](https://github.com/arcships/rutis/issues/1363)，属于 0.23 beta。

## 1. 现在是什么样

会话的一条消息由承载决定上限：进程管道和 WebSocket 默认 16 MiB（16777216 字节），内存通道没有上限（会话协议 `SESS-70`）。发送方在写出前比较整帧的字节数，超限就让这次调用失败，错误码 `bad-value`，`data` 写明部位和上限（`SESS-71`）：

```text
svc.take: argument 2 cannot be sent: the message would be 17825803 bytes, over the limit of 16777216 bytes (16 MiB)
```

插件代码拿到的是这一行和 `{"argument":2,"limit":16777216,"part":"argument"}`。要把一张图、一段音频、一份导出文件从一个语言传到另一个语言，业务方只能自己切片、分页、多次调用、自己拼回来，还要自己处理“传到一半断了”的重试语义。返回值超过 16 MiB 时同样的错误写在结果一侧（`part: "result"`）。#1361 要求跨语言调用对超过单条消息上限的参数和返回值是透明的：业务代码写一次调用，对端拿到完整的值。

规模：编码后的帧超过 16 MiB 就到上限；一个字符串值在 JSON 里比原文多两个引号和转义，数组还要多加元素分隔符和 `WireValue` 包装。今天没有一个实现能把 17 MiB 的字符串参数从 Rust 送到 Node。

## 2. 决定

**D1 加一种 `chunk` 帧，在会话协议 5 里。** 协议 5 是 0.23 才发布的版本，还没有发布过；发布前给它加帧不需要升版本（协议规格 `SESS-80`、G7 的同类处理）。七种实现一起升级到 0.23，旧版本在握手时被拒（`SESS-79`）。**不采用**：升到协议 6——0.22 与 0.23 本来就不能混跑，升版本只会让迁移文档多一步。**不采用**：只提高承载的 16 MiB 上限——管道和 WebSocket 的上限是承载的规则，提高它对端的配额，既改不了其他实现的默认值，也没有解决“单条消息要整块驻留内存”的问题，而且对端不理解时仍然断链。

**D2 用一个能力 `chunked` 协商，只有双方声明才分片。** 声明的是“这一端能重组 `chunk` 帧”。发送方只有在**对端**声明了 `chunked`（并且本端也声明）时才把超限的帧切片；否则保持今天的行为：超限的调用以 `bad-value` 失败，帧不写出。这样 Python、Go、Swift 不声明时行为完全不变，也不会收到它们不认识的帧。**不采用**：默认所有协议 5 实现都收 `chunk`——那会让不认识它的实现对超限帧直接按未知 `op` 结束会话。

**D3 分片是整帧 JSON 字节的切片，每片至多 8 MiB。** 发送方把已经编码好的整帧字节切成连续的片，每片 base64 后放进一个 `chunk` 帧。接收方按 `transfer` 缓冲，收到 `last: true` 后拼回原字节、按 UTF-8 解码、当作一条普通消息交给现有会话。帧的边界、顺序、解码路径都不变。8 MiB 的片 base64 后约 10.67 MiB，在 16 MiB 的承载上限内；承载上限更小时（WebSocket 的 `Limits.max_message` 可配置），片长按上限缩小，保证一个 `chunk` 帧自己装得下。**不采用**：在 `args` 里放分片列表——那要改 `WireValue` 和七种实现的值编解码，违反“不改业务 API”。

**D4 同一传输的片连续发送，同一时刻只有一个传输。** 发送端在同一个出站顺序里把一帧的所有片排在一起（Rust 在队列的一条条目里，Node 在一次同步发送里），其他帧排在这组片之前或之后，不能插进中间。接收端因此只维护一个进行中的传输：遇到新的 `transfer` 而旧的还没 `last`，是协议错误。**不采用**：允许并发传输——那要把“一帧的片”和“别的帧”的交错做成合法，接收端要为每个传输排序、限流，收益只是让两个大帧并行，代价是协议复杂度和内存峰值翻倍。

**D5 重组与单帧上限默认 256 MiB，可按会话/宿主配置，不做无上限。** 一次会话的最大帧由 `SessionOptions::max_transfer`（Rust 宿主）与 Node 会话的 `maxTransfer` 设定：默认 256 MiB（268435456 字节），是 16 MiB 承载上限的 16 倍，足够文件级数据，同时给单条消息一个上界。部署也可以用 `RUTIS_MAX_TRANSFER` 一处设置；宿主把它传给启动的运行时进程，两端用同一个值。发送方在切片前比较：超过配置值的帧不发送，调用以 `bad-value` 失败，`limit` 写配置值（默认 268435456），文字是：

```text
svc.take: argument 2 cannot be sent: the message would be 300000000 bytes, over the limit of 268435456 bytes (256 MiB)
```

接收方在缓冲时累计：超过同一配置值就结束会话（它看不出所属调用，没法降级为调用级错误）；不静默截断。**不采用**：写死常量——Dim 等宿主按自己的内存预算调整；**不采用**：无上限——一个不遵守协议的对端可以让本进程为一条消息分配任意内存。

**D6 未完成的传输在会话结束时丢弃，绝不派发。** 接收方只在 `last` 到达、重组和解析都成功后才把帧交给会话。会话在传输中途结束（承载断开、对端退出、协议错误），缓冲随会话一起丢弃；被调方法从未运行。这是 #1361 的验收项之一，也是“恰好执行一次”的另一面：片不是调用，只有完整的帧是。**不采用**：把已收到的片当作部分参数先派发——业务方会看到半截值，且无法表达“还没收完”。

**D7 先落在 Rust 宿主 ↔ Node 运行时，其余实现不声明；#1363 补齐 Bun 与 Kotlin。** `crates/rutis-bridge/src/session`（Rust）与 `node/rutis-runtime/src/session.mjs` 实现收发；#1363 让 Bun（`bun/rutis-bun/src/session.ts`）与 Kotlin（`kotlin/rutis/rutis/.../peer/Peer.kt`）同样实现并声明 `chunked`。Python、Go、Swift 不改代码，也不声明 `chunked`，因此 Rust 不会向它们分片，行为与今天相同。它们以后实现时同样只需加帧和能力，不改业务 API。**不采用**：一次改七种实现——那会把 #1361 变成七个 PR 的串联，而验收只需要一条真实的跨语言路径。

**D8 插件 API、服务签名、公开 API 不变。** 没有新的必填配置项、没有新的插件可见概念、没有改变 `Endpoint`/`Connection` 的方法签名；新增的只有协议常量（`CHUNK_SIZE`、`DEFAULT_MAX_TRANSFER`）、会话选项（`SessionOptions::max_transfer` 与 `RUTIS_MAX_TRANSFER`）、能力字符串，以及线格式里的一个 `op`。业务方继续写一次调用。

## 3. 规则

### 3.1 `chunk` 帧

线上对象，`op` 为 `"chunk"`：

| 字段 | 类型 | 含义 |
| --- | --- | --- |
| `op` | `"chunk"` | 帧标签。 |
| `transfer` | `string` | 传输 id，`<发送端 endpoint>:<n>`，发送端在会话内从 1 严格递增，非空。 |
| `seq` | 整数 | 片序号，从 0 开始，逐个加一。 |
| `last` | 布尔 | 这是最后一片。 |
| `data` | `string` | 原帧 UTF-8 JSON 字节的一个片段，base64（RFC 4648 §4，带填充，规范形式，无空白）。 |

`chunk` 不是 `Frame` 的变体：它在 `Frame` 解码之前由会话读取层处理（Rust 的读线程、Node/Bun 的 `Session.receive`、Kotlin 的读线程），重组完成后产出的字节就是一条普通消息。未知字段、未知 `transfer`、类型不对的字段都是畸形帧：会话以传输错误结束（没有所属调用可以降级）。`hello` 永远不分片（它很小，而且分片依赖对端的能力声明，声明在 `hello` 里）。

### 3.2 发送

- 触发条件：整帧字节数超过 `Sender::max_message()`，**并且**本端与对端都在 `hello.capabilities` 里声明了 `chunked`。Node 的会话上限取自构造参数 `maxMessage`（默认 16 MiB），与它所在通道的上限一致；Bun 同样取 `maxMessage`，Kotlin 取会话的 `channelLimit`（默认 16 MiB）。
- 片长：`min(8 MiB, 按通道上限算出的最大值)`；后者保证 `base64(片长) + chunk 帧的固定开销 <= 通道上限`。片长算出来为 0（通道上限极小）时不切片，按“对端不支持”处理。
- 一帧的所有 `chunk` 帧在出站顺序里连续，中间不插别的帧。
- 切片前检查整帧不超过会话的 `max_transfer`（默认 256 MiB）；超过则不发送，按 `SESS-71` 的方式让这次调用失败，`limit` 为配置值。
- 出站积压（`SessionOptions::outbound_limit`）把一次传输记作一条消息、字节数按原帧计；空队列仍接受一条消息，无论多大（`SESS-75` 不变）。

### 3.3 接收

- 收到 `chunk` 时：
  1. 问候之前收到 `chunk`：`request before protocol handshake`（与任何其他帧相同，`SESS-21`）。
  2. 对端没有声明 `chunked`：协议错误，结束会话。
  3. `transfer` 非空、`seq` 为安全整数且从 0 开始逐个加一、`last` 为布尔、`data` 是规范 base64；否则结束会话。
  4. 进行中已有另一个 `transfer`：结束会话。
  5. 累计字节数超过会话的 `max_transfer`（默认 256 MiB）：结束会话。
  6. `last: false`：继续读下一帧；`last: true`：拼回字节，按 UTF-8 严格解码，用现有 `parse_frame` 路径解析并处理。重组结果本身若是 `chunk` 帧：结束会话（不允许嵌套）。
- 重组完成的帧与普通帧完全同权：深度上限（208 层）、未知字段的调用级拒绝（`SESS-66`）、调用号、引用、取消规则都不变。
- 会话结束（正常或异常）丢弃所有未完成的缓冲；从未 `last` 的传输不派发、不回复。
- 一帧的片在通道上连续，所以接收端的“一帧一次”顺序（`SESS-18`）不变：`chunk` 帧之间不会有别的帧。

### 3.4 失败范围与文字

| 情形 | 结果 | 文字（Rust，逐字） |
| --- | --- | --- |
| 发送方整帧超过会话上限（默认 256 MiB） | 只让这次调用失败，`bad-value`，`limit` 是配置值 | `svc.take: argument 2 cannot be sent: the message would be 300000000 bytes, over the limit of 268435456 bytes (256 MiB)` |
| 对端不支持 `chunked`、整帧超过通道上限 | 与今天相同：只让这次调用失败，`limit` 是通道上限 | `svc.take: argument 2 cannot be sent: the message would be 17825803 bytes, over the limit of 16777216 bytes (16 MiB)` |
| `seq` 不连续 | 会话结束，传输错误 | `chunked transfer node:3: expected chunk 2, got 3` |
| 传输未结束就来了另一个 `transfer` | 会话结束，传输错误 | `chunked transfer node:3: transfer node:4 started before it ended` |
| 重组超过会话上限 | 会话结束，传输错误 | `chunked transfer node:3: over the limit of 268435456 bytes (256 MiB)` |
| 对端没声明 `chunked` 却发来 `chunk` | 会话结束，传输错误 | `chunk frame from a far end that did not declare chunked` |
| `data` 不是规范 base64 | 会话结束，传输错误 | `chunked transfer node:3: invalid bytes (11 characters): expected base64 (RFC 4648 §4) with padding` |
| 重组出的字节不是合法 UTF-8 或不是 JSON 帧 | 会话结束，传输错误 | 与今天收到畸形消息相同（`SESS-63`） |
| 传输中途承载断开 | 会话以传输错误结束，未派发 | `peer disconnected`（管道），或承载自己的原因 |

同一类错误在 Node 一侧的句子可以不同（`SESS-77`）；验证比较类别、范围和错误码。

### 3.5 限制与明确不保证

- 一次传输的重组上限是会话的 `max_transfer`，默认 256 MiB，可按会话或宿主配置（`RUTIS_MAX_TRANSFER`）；超限结束会话，发送方超限只让这次调用失败。
- 同一时刻至多一个未完成的传输；第二个 `transfer` 是协议错误。这是并发未完成传输的上界，不随配置改变。
- 分片不改变深度、调用号、引用、取消、等待图的任何规则。
- 内存通道没有承载上限，不会分片，但会话的 `max_transfer` 仍约束整帧：没有无上限的会话。
- 不保证未声明 `chunked` 的实现之间能传超过通道上限的值（今天的行为不变）。
- 不保证分片能穿过承载之外的中间层仍保持效率：重组要求对端把整帧收进内存，内存峰值约为帧大小的 1.4 倍（原字节 + base64）。
- 不保证 `chunk` 帧的 `data` 边界有意义：业务方看到的仍是完整的帧。

## 4. 开发者看到什么

### 插件作者

**成功：** 代码不变。参数或返回值编码后超过 16 MiB、且对端是声明了 `chunked` 的运行时（0.23 的 Node、Bun、Kotlin 运行时），调用照常成功，对端拿到完整值。没有新配置、没有新输出行、没有需要学习的帧概念。

```ts
const bytes = await ctx.remote.node.readFile('export.bin') // 20 MiB
```

20 MiB 的值在 Rust 宿主与 Node 运行时之间往返，业务方写一次调用；不要自己切片、分页或重试。

**失败（逐字，Rust 一侧）：**

| 情形 | 输出 |
| --- | --- |
| 值编码后超过会话上限（默认 256 MiB，宿主可调大） | `svc.take: argument 2 cannot be sent: the message would be 300000000 bytes, over the limit of 268435456 bytes (256 MiB)`；错误码 `bad-value`；`data` 是 `{"argument":2,"limit":268435456,"part":"argument"}` |
| 对端运行时没有声明 `chunked`（Python、Go、Swift），值超过通道上限 | 与 0.22 相同：`svc.take: argument 2 cannot be sent: the message would be 17825803 bytes, over the limit of 16777216 bytes (16 MiB)`；`data` 的 `limit` 是 16777216 |

**需要学习的概念：** 无新增。分片是运行时之间的事；插件看到的仍是值和调用。

### 宿主作者与部署者

**成功：** 会话协议没有新增必填配置；WebSocket 子协议仍是 `rutis.5`。Rust 宿主与 0.23 的 Node、Bun、Kotlin 运行时之间，超过 16 MiB 的参数和返回值直接可用；同一次调用只执行一次。最大帧默认 256 MiB，宿主可按内存预算用 `SessionOptions::max_transfer` 或 `RUTIS_MAX_TRANSFER` 调大或调小，并传给启动的运行时进程。

**失败：**

- 会话在传输中途断开：会话以传输错误结束，未完成的请求不派发；在途调用按今天的规则以 `peer-gone` 失败。
- 对端不声明 `chunked`：行为与 0.22 相同，超限的调用显式失败，不会误发 `chunk` 帧。
- `RUTIS_MAX_TRANSFER` 不是正整数：宿主或运行时启动失败，文字是 `RUTIS_MAX_TRANSFER "abc": expected a positive number of bytes`；改成正整数或删除该变量。
- 两端上限不一致：发送方按本端上限分片，接收方按自己的上限重组；超过的一端以 `chunked transfer node:3: over the limit of <n> bytes` 结束会话。把 `RUTIS_MAX_TRANSFER` 设在宿主进程（宿主会传给启动的运行时进程），直接启动的运行时也设同一个值。

**需要学习的概念：** 无新增；要调整上限时知道 `max_transfer` / `RUTIS_MAX_TRANSFER` 一个旋钮即可。部署仍是“同一部署所有端一起升级到 0.23”。

### 运行时作者与贡献者

**成功：** 实现 `chunk` 帧的收发并声明 `chunked` 后，超过承载上限的帧在对端完整可用；不实现就不要声明，行为与今天相同。

**失败：** 传输中途断开时，未完成的传输不派发；`seq` 不连续、传输交错、超过配置上限、对端未声明却分片都以传输错误结束会话。错误文字可以与其他实现不同。

**需要学习的概念：** `chunk` 帧、`transfer`、`seq`、`last`、`chunked` 能力、重组上限。

## 5. 兼容性与迁移

- 协议 5 是新版本（0.23 才发布），加帧不升版本；0.22 及更早仍在握手时被拒（`SESS-79`）。
- 能力是声明式的：0.23 的 Rust 宿主只向声明 `chunked` 的对端分片。0.23 的 Node、Bun 与 Kotlin 运行时声明 `chunked`；Python、Go、Swift 不声明，因此与它们的会话保持 16 MiB 行为，不会收到 `chunk` 帧。
- 见过 0.23 预发布构建、但那个构建没有 `chunked` 的对端：它不声明，本端不分片；它若声明了却不认识 `chunk`（不会发生，声明即实现），本端不会发它。
- 业务代码、服务签名、插件 API、`rutis.json` 格式都不变。
- 迁移指南：[迁移到 0.23](migration-0.23.md) 新增一节「跨语言调用超过 16 MiB」。

## 6. 实现

- `crates/rutis-bridge/src/session`：`Chunk` 帧的解析与序列化、发送端切片、接收端重组、常量 `CHUNK_SIZE`/`DEFAULT_MAX_TRANSFER`、`SessionOptions::max_transfer`（及 `from_environment`、`MAX_TRANSFER_ENV`）、`Endpoint::rust` 声明 `chunked`。
- `crates/rutis-host/src/connect/spawn.rs`：宿主把 `max_transfer` 放进 `RUTIS_MAX_TRANSFER` 传给启动的运行时进程。
- `node/rutis-runtime/src/session.mjs`：`CAPABILITIES` 加 `chunked`、发送端切片、`receive` 的重组、`maxTransfer` 选项与 `RUTIS_MAX_TRANSFER`（默认 256 MiB）。
- `bun/rutis-bun/src/session.ts` 与 Kotlin `kotlin/rutis/rutis/src/shared/kotlin/arcships/rutis/internal/peer/Peer.kt`（#1363）：同样声明 `chunked`，实现发送端切片、读线程重组、`maxTransfer` / `maxTransferFrom`（默认 256 MiB，读 `RUTIS_MAX_TRANSFER`）。
- 协议规格 `docs/protocol-session.md`/`.en.md`：§2.1 帧表、§2.4 能力表、§10 上限、§11 规则（`SESS-84`–`SESS-88`）、§12 输出。
- 保证登记表 `docs/guarantees.md`/`.en.md` 的 `bridge/large-and-deep-values`：补上分片后的承诺与验证列。
- 迁移指南 `docs/migration-0.23.md`/`.en.md`：新增一节。
- 测试：
  - Rust ↔ Node（真实 Node 运行时、Unix socket、16 MiB 管道）：参数 > 16 MiB、返回值 > 16 MiB 都完整往返；`tally` 方法证明每次调用只执行一次。
  - Rust 接收：脚本化对端把一帧拆成 `chunk` 帧；完整时派发一次；中途关闭通道时不派发；`seq` 错、交错传输、超过上限、握手前 `chunk` 都结束会话。
  - Node 接收与发送：同样的重组、失败与单次派发断言；不声明 `chunked` 的对端仍走原来的超限错误。
  - Bun 与 Kotlin（#1363）：同样的重组、失败与单次派发断言；各自与真实 Node 运行时的会话各一条，参数与返回值 > 16 MiB 完整往返，`tally` 证明每次调用只执行一次。
  - 配置：Rust 的 `max_transfer` 调小后超过它的帧只让这次调用失败、重组超过它结束会话并说出配置值；Node 的 `maxTransfer` 同理。
