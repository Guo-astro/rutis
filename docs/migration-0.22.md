# 迁移到 0.22

[English](migration-0.22.en.md)

0.22 还没发布，这份指南随行为变化一起写：每合并一件加一节，发布前按 [#694](https://github.com/arcships/rutis/issues/694) 补齐。下面只写与 0.21 相比的变化，删掉的写法直接删，不标弃用。

0.22 **没有删除或改名插件看得到的 API**：会话协议仍是 4、`rutis.json` 的 `format` 仍是 1、插件 API 仍是 3，内核的公开 API 快照 `crates/rutis/public-api.txt` 没有变化。下面按「要不要改代码」分成两类：第一类是靠旧行为写下去的代码会碰到新的拒绝或新的空结果；第二类不用改代码，只是不再有旧的误失败或误忽略。

## 要改的写法

### `filter(predicate)` 必须同步回答（#637）

设计：[事件协议](design-events-protocol-2026-10-18.md) §6。Node、Bun、Python 一致。

#### 改了什么

`filter(predicate)` 的谓词以前可以返回 Promise / thenable / 协程；它们恒真，于是这个过滤器等于没写，所有监听器都放行。现在谓词必须同步回答，返回 Promise 或 thenable 会让这次分发失败：

| 语言 | 0.21 | 0.22 |
| --- | --- | --- |
| Node、Bun | `async` 谓词恒真，全部放行 | `TypeError: a filter predicate must answer with a boolean, not a Promise` |
| Python | `async def` 谓词返回协程，恒真 | `TypeError: a filter predicate must return a boolean, not an awaitable`，协程先被关掉 |

错误属于这次分发；监听的登记不变，别的分发照常。

#### 要改什么

- 把谓词写成普通函数，同步返回布尔值：`h.filter(d => d.row === 'clock')`，不要写 `async d => …`。
- 要在过滤前做异步判断，先在调用 emitter 之前把结果算好（例如先 `await` 一次，再用布尔变量做谓词）；`filter` 本身不提供异步谓词。
- Python 的 `async def predicate(...)` 改成 `def predicate(...)`；如果它内部要 `await`，把那段逻辑移到 `filter` 之前。

#### 影响

- 插件作者：只有用了 `filter` 的插件受影响；`filter(() => false)` 这种同步写法不变。
- 宿主持有者：宿主不调用这个 API，看不到变化。
- 部署的人：看不到变化。

### 行卸载开始后不能再 `ctx.provide`（#1143）

设计：[卸载顺序](design-unload-order-2026-11-01.md)。Rust、Go、Kotlin、Swift 一直拒绝；0.22 起 Python、Bun 与它们一致。

#### 改了什么

一行开始卸载后，卸载已经取走了清理栈。这时再 `ctx.provide`：

| 情况 | 0.21 | 0.22 |
| --- | --- | --- |
| Python 的 `ctx.provide` | 接受；服务留在 `services` / `slots` 里，没有行；卸载没走完时还会向宿主发带 handle 的 `service` 帧 | `RuntimeError: row <row> is not active`；不登记撤回、不发帧 |
| Bun 的 `ctx.provide` | 接受；服务留在表里，没有行 | `Error: row <row> is not active`；不登记撤回、不发帧 |

`<row>` 是行的名字（有名字时）或行 key。Rust 的错误类型仍是 `InactiveEffect`（`fiber disposed`）。

#### 要改什么

- 把 `provide` 放在 `apply` 里，或放在行仍处于加载/运行状态时；不要在清理函数、`stop` 钩子或卸载开始后的异步回调里 `provide`。
- 如果某个服务只在清理期间才需要，改成在 `apply` 里提前提供，卸载时按原来的逆序栈撤回。
- Go、Kotlin、Swift 在 0.21 就不登记这个服务（Go、Kotlin 打一行 `row <row> is not active: service <name> is not provided`，Swift 返回 `row <row> is not active`）；这次只补齐 Python 与 Bun。

#### 影响

- 插件作者：在清理里 `provide` 的插件（在 Rust 上本来就不成立）会在 Python、Bun 上开始失败；这是把旧漏洞关掉，不是新的限制。
- 宿主持有者：不会再看到“已卸载的行仍提供某服务”的宣告。
- 部署的人：看不到变化。

### 自己撤下的监听器回「没有结果」（#1135、#1138）

设计：[事件协议](design-events-protocol-2026-10-18.md) §3 决定 9、§5.7。七种运行时一致。

#### 改了什么

宿主是监听器表的主人：它可能已经把某个监听器选进一次分发，而运行时这边刚把它撤下并发出 `events.off`。这次调用到达运行时的时候，本地条目已经没了。

| 情况 | 0.21 | 0.22 |
| --- | --- | --- |
| 运行时自己撤下、并已通知宿主的监听器 | Node、Bun 抛 `BindingError: listener <id> of <name> is withdrawn`；其他运行时也失败一次 | 这次调用回「没有结果」（各语言自己的空值）；`waterfall` 调 `next` 并返回它的值；这次分发不因擦肩而失败 |
| 本进程从没登记过的 id | `BindingError: listener <id> of <name> is withdrawn` | 不变 |
| 宿主自己撤下的条目（行卸载、`rows.ended`、加载失败） | `BindingError: listener <id> of <name> is withdrawn` | 不变 |

运行时记住自己撤下并已通知宿主的 id（有上限，最旧的忘掉），所以只在“确实是自己刚撤下”的窗口里回没有结果。

#### 要改什么

- 不要按 `listener ... is withdrawn` 判断“这次分发失败了”：你自己撤下的监听器现在会正常回没有结果。要判断撤回，用你自己的 disposer 或本地状态。
- `waterfall` 的谓词/回调：宿主迟到的调用现在会走 `next`，链从它本该在的位置继续；以前这里会整条失败。
- Python、Go、Kotlin、Swift 在 0.22 也跟 Node、Bun 一致；0.21 上它们的“失败一次”不再出现。

#### 影响

- 插件作者：只有在撤下监听器的同一刻还有宿主调用在路上的插件会看到差别；正常登记/撤下不变。
- 宿主持有者：不用改代码；如果宿主日志里把 `withdrawn` 当失败统计，那个数字会变小。
- 部署的人：看不到变化。

### Kotlin、Swift 不再忽略未知字段（#635）

设计：[会话协议](protocol-session.md) §9。七种运行时一致。

#### 改了什么

| 帧 | 0.21 的 Kotlin、Swift | 0.22 的 Kotlin、Swift |
| --- | --- | --- |
| `invoke`、`call`、`get`、`await` 带未知字段 | 静默忽略 | 只失败这次调用：`code` 是 `bad-frame`，消息是 `unknown field "x" in invoke`，`data` 是 `{op, fields}`；`x` 是字段路径，嵌套值里的字段写成 `args.value.x` |
| `return`、`throw` 带未知字段 | 静默忽略 | 只失败等待这次回复的调用，`code` 与消息同上 |
| `hello` 带未知字段 | 静默忽略 | 握手失败：`incompatible session: unknown field "x" in hello` |
| `release`、`cancel` 带未知字段 | 静默忽略 | 仍忽略 |

Rust、Node、Bun、Python、Go 在 0.21 就已经这样；0.22 把 Kotlin、Swift 补齐。

#### 要改什么

- 不要把未知字段当扩展机制：`SESS-69` 明确不保证不改含义的新可选字段能静默到达旧接收方。新增帧或改变已有字段含义要提升协议版本；新增能力要按 `capabilities` 声明使用。
- 如果你在 Kotlin、Swift 上发过带自定义字段的帧，接收方在 0.22 会以 `bad-frame` 失败该调用，而不是忽略。把字段挪进 `data` 的已知结构，或按新协议版本走。

#### 影响

- 插件作者：正常插件不写协议帧，看不到变化。
- 运行时作者：Kotlin、Swift 的实现新增了 `unknownFields` 检查；七种运行时的行为一致。
- 部署的人：旧 Kotlin/Swift 仍忽略未知字段；新旧混用时旧的那一端保持旧行为。

## 不用改代码的行为变化

### 心跳从 hello 起算（#1102、#1182）

Go、Swift、Node、Bun、Kotlin 的监听端现在在 hello 成功发出后才起算心跳；0.21 从 accept 或构造连接时起算。旧租约清理慢、握手慢时，新连接以前会在 hello 前被 1001 关掉，现在不会。关闭码仍是 1001；Go、Swift、Kotlin 的 reason 是 `the far end stopped answering`。不用改代码；为这个误关加的等待或重试可以删掉。Python 的 `websockets` 心跳路径不在这次改动内。

### 节点导入的服务带行 id（#1171）

`services.announce` 增加可选的 `row`：提供方有行时带该行的 id，没有行时不带。导入侧的远程插件用 `ctx.mark_row(row)` 标记为同一行，因此 `rutis/service` 事件里的 `row` 从 `null` 变成提供方的行 id。旧对端会忽略这个字段；新对端读旧宣告得到 `None`。按名读 `row` 的代码不受影响（多一个键）。

### unload 先于 load 会被拒（#1101）

宿主先收到某行的 `rows.unload`、后收到同一 key 的 `rows.load` 时，后到的装载现在失败，消息是 `<key> was unloaded`。墓碑按会话存放：同一 key 在新会话里照常装载；卸载一张已装载的行不记墓碑。插件作者看不到；宿主要保证先 `load` 后 `unload`，或接受这次 `load` 失败后按新 key 重试。

### 4003 拒绝的失败文字（#1130）

对端以 4003 拒绝连接时，本端以前读到 `peer disconnected`（transport），现在读到 `endpoint mismatch: <原因>`；一对通道的两个方向拿到同一个关闭原因。按错误类型/码匹配的代码不受影响；按文字匹配 `peer disconnected` 的代码要改。

### Node、Bun 的 `SyncWaitCycle`（#639）

Node、Bun 判断同步等待成环时只看尚未回复的同步调用；回复已读的调用不再算在等待链里，所以 `await` 之后的回调不再偶发误报 `SyncWaitCycle`。真正的环仍在原处拒绝。不用改代码。

### 其他修复

- **Python 首次导入插件**：Windows 上目录 mtime 落在同一刻度时可能报 `ModuleNotFoundError`；首次 `import_module` 前现在会 `invalidate_caches()`。不用改代码（[#1139](https://github.com/arcships/rutis/issues/1139)、[#1140](https://github.com/arcships/rutis/pull/1140)）。
- **Kotlin 应用托管**：`stop()` 返回后不再拨号或绑定；`RutisApp` 的起停竞态修好。不用改代码（[#1096](https://github.com/arcships/rutis/pull/1096)）。
- **Node、Bun 强杀旧运行时**：接管时等 `RUTIS_SHUTDOWN_TIMEOUT + 2s`，不再写死 5 秒。不用改代码（同 #1096）。
- **Go 在 Windows 上读 token 行**：逐个字节读到行尾，不再把紧跟的问候吞掉；测试工具不再报 `peer disconnected`（[#1087](https://github.com/arcships/rutis/pull/1087)）。

## 影响

- **插件作者**：只有三类要改：`filter` 的谓词要同步回答；`ctx.provide` 要在行仍加载/运行时调用；不要把 Kotlin、Swift 的未知字段当扩展。自己撤下的监听器回没有结果是更宽松的行为，不用改。心跳、服务 `row`、`SyncWaitCycle`、Python 导入、Kotlin 应用托管都不用改。
- **宿主持有者**：Rust 公开 API 没有删除或改名，嵌入代码不用改。会碰到的是错误文字：4003 拒绝从 `peer disconnected` 变成 `endpoint mismatch: <原因>`；节点导入的 `rutis/service` 事件 `row` 从 `null` 变成行 id；先 `unload` 后 `load` 同一 key 会收到 `<key> was unloaded`。Rust 的 `public-api.txt` 没有变化。
- **部署的人**：没有新的配置、`rutis.json` 字段、协议版本或迁移设置；会话协议仍是 4、`format` 仍是 1、插件 API 仍是 3。旧运行时包在新宿主/新运行时旁边按自己的旧行为工作，会话照常建立。
- **运行时作者**：`docs/protocol-session.md` 是现行会话规格；未知字段、畸形帧矩阵、心跳起算、撤销竞态、`provide` 拒绝、服务 `row` 都要按它实现。追踪表与 `specs/` 是贡献者用的证据，不是运行时输出。
- **测试工具**：自己撤下监听器后的迟到调用现在回没有结果；测试里如果按 `withdrawn` 断言这次失败，要改成断言没有结果（`waterfall` 断言 `next` 的值）。

## 兼容

- 会话协议仍是 4，`rutis.json` 的 `format` 仍是 1，插件 API 仍是 3：没有新的能力要求，没有新的必需帧字段，握手不变。
- 新旧版本可以混用：旧运行时包在它那一侧保持旧行为（旧 Kotlin/Swift 仍忽略未知字段，旧心跳仍从 accept 起算，旧 Python/Bun 仍接受卸载后的 `provide`）；新对端不会因为对方是旧版本而断会话。`services.announce` 的 `row` 是可选字段。
- `rutis.json` 与 dsh 用户层没有格式变化：持久化样本从 0.21.0 复制，期望不变。发布 PR 里建 `tests/fixtures/persisted/0.22.0/`。
- dylib 插件照每次发布一样用 0.22.0 的 SDK 重新构建。
- 这一版没有网络冒烟或发布门结果；发布前按 [release.md](release.md) 的正常流程跑。
