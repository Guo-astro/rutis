# 统一事件的协议规格

[English](design-events-protocol-2026-10-18.en.md)

状态：规格（2026-10-18），[#464](https://github.com/arcships/rutis/issues/464) 第 1 步。它把已定稿的设计[统一事件：一个逻辑总线](design-events-2026-10-18.md)写到足够精确，使 Rust 内核与 bridge、Node / Bun、Python、Go 能只凭本文并行实现。本文不重开设计的决定；设计没有说到的地方，本文的选择列在第十五节。

为什么单独成文，而不是作为[协议插件设计](design-protocol-plugins-2026-09-25.md)的附录：那份文档是历史方案，只有 §6.1（值标记）是现行规范；它的第十一节（事件）整体被本文取代。把这么大的规格放进一份历史文档，读者分不清哪一半是现行的。那份文档的第十一节现在指向这里，[运行时契约](../.agents/skills/rutis-new-runtime/references/runtime-contract.md)新增"Events"一节，概括运行时要实现的部分。

本文中"必须"是每个实现的硬性要求。帧、字段名、错误名和引号里的消息是确切的；它们周围的说明文字不是规范。

## 一、术语

| 术语 | 含义 |
| --- | --- |
| **宿主** | rutis 进程（rutis-host，或嵌入 rutis-bridge 的应用）。它是逻辑总线的中心 |
| **进程** | 宿主，或一个运行时会话（本地运行时进程、远程运行时、接入的运行时）。每个都与宿主有一个会话 |
| **节点** | 经节点连接到达的另一个 rutis 宿主 |
| **条目** | 逻辑总线上登记的一个监听器：宿主的 Rust 插件的、运行时某一行的，或经节点连接引进来的 |
| **监听器表** | 宿主上全部条目的有序列表 |
| **描述** | 总线对一个条目的说明（§4.2）；过滤函数看到的就是它 |
| **作用域链** | 从根到某个作用域的作用域列表（第三节） |
| **一次分发** | 一个事件的一次发出：名字、参数、方式、发出方作用域，可选的过滤与终态 |
| **分发方** | 执行一次分发的一方：快速路径上是发出方自己的进程（§5.2），否则是宿主 |
| **一次调用** | 一次分发中对一个条目的一次调用 |
| **没有结果** | 返回值是 `undefined` / `null`（JavaScript）、`None`（Python、Rust）、`nil`（Go），或 `false`（任何语言，包括 Rust 的 `Some(false)`）。其余都是结果 |

## 二、版本、能力与对旧端的拒绝

### 2.1 能力 `events-v2`

会话协议的版本号不变：compat 格式仍是协议 2，endpoint 格式仍是协议 3，WebSocket 子协议仍是 `rutis.3`。事件走现有的 `invoke` / `call` 帧，不增加帧类型。变化的是事件协议，由一个**必需的能力**把关：

- 每个 0.13 端（宿主、运行时、节点）在两种格式的 `hello.capabilities` 里都声明 `events-v2`。Rust 把它加进 `COMPAT_CAPABILITIES`、`COMPAT_DECLARED` 和 `Endpoint::rust`。
- 对端没有声明 `events-v2` 时，每个 0.13 端在握手之后立即结束会话，不再发任何东西：没有 `mount`，没有 `link.offers`。
- 0.12 及更早的对端不声明它，所以混用在加载任何东西之前就失败，所有路径都如此：本地运行时、远程运行时、接入的运行时、节点连接。

### 2.2 失败信息

| 拒绝方 | 信息（确切） | 之后 |
| --- | --- | --- |
| Rust（宿主或节点），对端报了自己（endpoint 格式） | `incompatible session: the far end (<name> <version>) does not declare events-v2 (the event protocol of rutis 0.13): upgrade it to rutis 0.13 or later` | `Handshake::Incompatible`；运行时以此错误启动失败；节点连接停止（状态 `link stopped: incompatible (…)`），不重试 |
| Rust，compat 格式（对端不报实现） | `incompatible session: the far end (a runtime older than rutis 0.13) does not declare events-v2 (the event protocol of rutis 0.13): upgrade it to rutis 0.13 or later` | 同上 |
| 0.13 运行时，宿主没有声明 | stderr：`rutis: refused: incompatible (this host does not declare events-v2 (the event protocol of rutis 0.13): upgrade rutis-host to 0.13 or later); stopping` | 退出状态 1；拨出的运行时不再重试 |

0.12 运行时遇到 0.13 宿主时，在 hello 之后看到会话被关闭，按任何会话丢失的方式退出；用户读到的是宿主一侧的上述信息。

### 2.3 插件 API 3

插件 API 版本升为 **3**（Node `PLUGIN_API`、Python `PLUGIN_API`、Go 清单的 `pluginApi`）：插件多了 `parallel`、`serial`、`waterfall`、`bail` 与 `filter`。0.13 运行时加载声明 API 2 或 3 的插件；声明 3 的插件被旧运行时以现有的信息拒绝。

### 2.4 删除的内容

| 删除 | 所在 | 仍然使用时的错误（确切） |
| --- | --- | --- |
| `mount` 参数 `events`、`emits`；`mount` 特性 `events`；运行时 → 宿主的 `event`；宿主 → 运行时的 `emit` | 协议 | —（到不了：§2.1 先拒绝了对端） |
| 节点的 `events.forward`、`NodeEvent`、`EventsPlugin` | rutis-bridge | —（从 API 删除） |
| `RuntimePlugin::emits`、`LocalRuntime::emits`、`Mount.emits`、`Mount.events`、`Process::emit`、`runtime::events::{Events, EmitToCordis}` | Rust API | 删除；编译错误 |
| Cordis 挂载 / 生成器配置的 `events`、`emits` | `rutis-bridge` 生成器配置 | `the config field <events\|emits> was removed in rutis 0.13: listeners register themselves; emit with ctx.emit / parallel / serial / waterfall / bail (https://github.com/arcships/rutis/blob/main/docs/migration-0.13.md)` |
| 节点连接配置 `events: { out, in }` | `rutis-bridge/peer` 行的配置 | `the peer config field events was removed in rutis 0.13: list events: { out } in export and events: { in } in import (https://github.com/arcships/rutis/blob/main/docs/migration-0.13.md)` |
| `on_waterfall*`、`on_waterfall_sync*`、`*_instance`、`*_keyed`、`EventKey::instance`、`SyncEvent` | rutis 内核 | 删除；编译错误（替代写法见 §12.1） |

## 三、作用域

**作用域**是加载器的一个组实例（一个 `group` 的一份副本，如一个租户、一个会话），或者根。每个插件、行和每次发出都恰好在一个作用域里：包含它的最内层实例，或者根。

- **线上的作用域链**：JSON 数组，元素是 `{ "id": <字符串>, "group": <字符串> }`，最外层在前。根是 `[]`。`id` 是不透明的字符串，在宿主现在和过去的全部实例中唯一（Rust：组 fiber 的 `InstanceId` 的十进制）；`group` 是加载器组的名字。例：`[{"id":"17","group":"tenant"},{"id":"42","group":"session"}]`。
- **规则**：作用域链为 `L` 的条目收到在链 `E` 发出的分发，当且仅当 `L` 是 `E` 的前缀（按 `id` 比较）。根上的条目收到全部；租户 `t1` 里的条目收不到 `t2` 的。
- **进程怎样知道作用域链**：宿主告诉每一行它的链。`rows.load` 增加第七个参数 `{ "scope": <链>, "id": <行的 id> }`（0.13 宿主总是发七个参数；行不导出服务时 `exports` 是 `{}`）。`id` 是配置给这一行的 id（加载器的行 id；同一组的各实例复制的行 id 相同），与会话内唯一的行键不同：描述（§4.2）与报告的各条信息（§5.6、第十一节）用它称呼这一行，协议消息仍用行键。没有 `id` 时用行键。经 `mount` 的 `plugins` 列表加载的插件，得到 `mount` 参数 `scope`（挂载它的插件的链）。Rust 插件用 `Ctx::scope()`（§12.1）。`mount` 还带 `runtime`，即描述里给出的运行时名字（§4.2）：运行时在快速路径上给自己的条目写描述时用它，与经宿主时的描述一致。
- **发出方的作用域从不由发出方自报**：运行时的 `events.dispatch` 写明发出的 `row`，宿主用这一行的链；这一行没有加载在这个会话上时拒绝（`BindingError`，`row <key> is not loaded in this runtime`）。挂载方式加载的插件写 `row: null`，用挂载的链。
- 叶子插件的 `isolate` 标签（服务作用域、U+0001 的租约）与事件作用域无关。

## 四、监听器登记

### 4.1 消息

所有事件消息都是 `target` 为空的 `invoke` 帧（控制操作），属于 `events` 族。接收方必须**就地**、按帧的顺序处理 `events.on`、`events.off` 和 `events.elsewhere`，处理完再读下一帧（它们只改表）。每条都以 `return undefined` 或 `throw` 回复。

| 方向 | 方法 | 参数 | 回复时机 |
| --- | --- | --- | --- |
| 运行时 → 宿主 | `events.on` | `[登记]`（§4.2） | 条目已进表，且它引起的 `events.elsewhere` 已排进其他每个会话之后 |
| 运行时 → 宿主 | `events.off` | `[[id, …]]` | 条目已出表之后；未知的 id 忽略 |
| 宿主 → 运行时 | `events.elsewhere` | `[{ "add": [名字], "remove": [名字], "addPrefixes": [前缀], "removePrefixes": [前缀] }]`（缺少的字段即 `[]`） | 应用之后 |

### 4.2 登记与描述

登记（运行时 → 宿主）：

```json
{ "id": "7", "name": "session/created", "row": "chat", "plugin": "chat-ui", "kind": "any", "prepend": false, "global": false }
```

| 字段 | 规则 |
| --- | --- |
| `id` | 运行时选定的字符串，在会话存续期间唯一，从不复用 |
| `name` | 事件名。`internal/*` 被拒绝（`BindingError`，`event <name> is internal to its runtime`）。`rutis/*` 可以监听 |
| `row` | 登记它的行的键；挂载方式加载的插件为 `null`。不是这个会话上加载的行时宿主拒绝。描述里的 `row` 不是它，而是这一行的 id（§3） |
| `plugin` | 登记它的插件的名字（Cordis 插件的名字，可以是这一行的子插件；叶子插件的名字） |
| `kind` | `"sync"`：已知同步（Go 的每个监听器）；`"async"`：已知异步（JavaScript 的 `AsyncFunction`、Python 的 `async def`）；`"any"`：要看它返回什么才知道（其余 JavaScript 和 Python 函数） |
| `prepend` | 监听器选项 `prepend`（所有语言，第十二节；Cordis 插件上就是 Cordis 自己的选项） |
| `global` | 监听器选项 `global`（所有语言，第十二节；Cordis 插件上就是 Cordis 自己的）：条目不受自定义过滤，但仍受作用域规则 |

运行时从不登记前缀；Rust 的模式监听器是宿主上的前缀条目（`prefix: true`，`name` 为前缀）。

总线保存、过滤函数看到的**描述**（JSON；Rust 为 `ListenerInfo`，Go 为 `rutis.Listener`）：

```json
{ "name": "session/created", "prefix": false, "plugin": "chat-ui", "row": "chat", "runtime": "py", "node": null, "scope": [{"id":"17","group":"tenant"}], "global": false }
```

`row` 是行的 id（`rows.load` 的 `id`，Rust 行为 `Ctx::mark_row` 标记的行，即加载器的行 id），不是行键，所以过滤函数可以按配置里的名字选择，同一行无论在哪种语言、哪个进程都一样；`runtime` 是运行时的名字（宿主自己的 Rust 插件为 `null`）；`node` 是经节点连接引进的条目所在对端的 id（其余为 `null`）；`scope` 是条目的链，由宿主按行设定（引进的条目：连接的作用域，第九节）。

### 4.3 顺序

- 每个条目有一个**位置**。宿主按接受的先后给追加的条目递增的位置（`events.on` 帧按到达顺序，Rust 登记在登记时，节点引进在收到时）；`prepend` 的条目取比现有全部位置都小的位置。一次分发取匹配的条目（同名，或名字以其前缀开头的前缀条目），按位置排序：本文所说的**登记顺序**。
- 一个进程自己的条目，在本进程里的相对顺序与宿主上相同（它的帧按序处理；它对自己的条目同样应用 `prepend`）。
- `rows.update` 让这一行的监听器重新登记：它们得到新的位置。

### 4.4 登记与分发之间的保证

1. **同一进程**：在同一进程里（按程序顺序）先登记、后发出，这次分发就包含这个监听器，快速路径和经宿主都如此（会话上它的 `events.on` 在 `events.dispatch` 之前，且两者按序处理）。
2. **行**：运行时在这一行于 `apply` 期间发出的每个 `events.on` 都得到回复之后，才回复 `rows.load`。宿主报告为 Running 的行，它的监听器都已在总线上。
3. **跨进程**：宿主在回复 `events.on` 之前，把由此引起的 `events.elsewhere` 排进其他每个会话。凡因果上在这个回复之后的分发（登记的插件随后调用另一个进程的服务，那里再发出）都包含这个条目。
4. **撤下**：`events.off` 得到回复后（或宿主撤下了这一行，§7.1），这些条目不再开始新的调用。已经发出的调用失败一次（§7.1）。
5. 宿主里的 Rust 登记，在 `on` 返回时已进表，相应的 `events.elsewhere` 已排好。

### 4.5 "别处有监听器"（elsewhere）

对每个运行时会话 R，宿主维护 **elsewhere(R)**：至少有一个条目不在 R 里的全部名字和前缀（宿主的 Rust 插件、其他运行时、节点引进），不考虑作用域。宿主用 `events.elsewhere` 把变化发给 R：握手之后、`mount` 之前先把当前全集作为 `add` 发出，之后只发差异，按顺序。R 的快速路径判断（§5.2）用它。R 的会话结束后，R 把 elsewhere(R) 当作空。

内核的进程内诊断事件（名字以 `rutis::` 开头，如 `rutis::ServiceChanged`、`rutis::FiberStatusChanged`）以 Rust 类型命名，在别处无法解码：它们的条目从不计入 elsewhere，也不经节点连接通告。

## 五、分发

### 5.1 算法（每个分发方都相同）

1. **快照**：与名字匹配、按登记顺序、作用域链是发出方链的前缀（第三节）、且存活（未撤下）的条目。
2. **过滤**（第六节），如果这次分发带了过滤：对快照里每个非 `global` 的条目按顺序各求一次，在任何调用开始之前。被它拒绝的条目去掉。
3. **执行**方式（§5.4）。每次开始一个调用之前，分发方检查条目仍然存活；已撤下的条目跳过，就像从未在快照里一样。

### 5.2 谁来分发：快速路径

运行时 R 发出名字 N 时，若 N 不在 elsewhere(R) 里、elsewhere(R) 里也没有是 N 的前缀的前缀，并且（只对 `emit`）R 没有更早经宿主发出、尚未得到回复的 N 的 `emit`（§5.5），R 自己分发（**快速路径**），不给宿主发消息。否则 R 发 `events.dispatch`，由宿主在整张表上分发，R 自己的条目经 `events.call` 回调到 R。Rust 宿主总是自己在整张表上分发。两种路径结果相同，只是代码在哪里执行不同。

### 5.3 消息

**`events.dispatch`**（运行时 → 宿主），`target` 为空的 `invoke`；`bail` 的分发是一次同步调用，帧按现有的 `sync-wait` 规则带 `sync: true`：

```json
[{ "row": "chat", "name": "session/created", "mode": "serial", "args": [ … ], "filter": <函数引用>, "terminal": <函数引用> }]
```

`mode` 是 `emit`、`parallel`、`serial`、`waterfall`、`bail` 之一。`args` 是值的 `list`（第十节）。`filter`（第六节）与 `terminal`（仅 `waterfall`）是可选的函数引用。`rutis/*` 在发送之前就被拒绝（第十一节）。回复按方式而定（§5.4）。

**`events.call`**（宿主 → 运行时，或经连接节点 → 节点），`target` 为空的 `invoke`；`bail` 与 `waterfall-sync` 为同步调用（按 `sync-wait` 规则带 `sync: true`）：

```json
[{ "listener": "7", "dispatch": "d1042", "own": false, "name": "session/created", "mode": "waterfall", "args": [ … ], "next": <函数引用>, "signal": {"type":"signal"} }]
```

| 字段 | 规则 |
| --- | --- |
| `listener` | 接收方的条目 id（它自己 `events.on` 的 id；经连接时为它通告的 id） |
| `dispatch` | 宿主选定的 id，在宿主内唯一；同一次分发的每个调用相同 |
| `own` | 这次分发来自同一会话的 `events.dispatch` 时为 `true` |
| `mode` | `emit`、`parallel`、`serial`、`waterfall`、`bail`、`waterfall-sync`（只有 Rust 发出 `waterfall-sync`） |
| `next` | 仅 `waterfall` / `waterfall-sync`：不带参数的函数引用，返回链上其余部分的结果 |
| `signal` | 现有的 `signal` 值：宿主取消这次调用时它被中止（§5.7） |

接收方的运行时以事件参数调用监听器，`waterfall` / `waterfall-sync` 时把 `next` 追加为最后一个参数。它原样回复监听器的返回值（"有没有结果"由分发方判断），或以 `throw` 回复监听器的错误。`emit` 方式下，监听器结束后无论如何都回复 `undefined`；失败在它这一侧报告（§5.6）。

对未知或已撤下的 id 的 `events.call`，以 `throw` `BindingError` 回复，信息为 `listener <id> of <name> is withdrawn`。

`internal/dispatch`（Node 的 Cordis；Bun 没有 Cordis）：`own: false` 的一次分发第一次调用本进程的监听器时，运行时在调用它之前，对这个 `dispatch` id 发一次 Cordis `internal/dispatch`（方式、名字、参数、`null`）；本进程自己发出的事件经 Cordis 自己的 `dispatch()`，在那里触发它。它抛出时，这次调用失败。

Cordis 的 `events.dispatch(type, args)`（五个方法底下的原语，返回要调用的回调；有的已发布插件直接调用它再自己调用回调）：名字不是 `internal/*`、`type` 为 `emit`、`serial` 或 `bail` 时，它返回一个回调，以被调用时的参数在总线上按 `parallel`、`serial` 或 `bail` 分发（在本进程，或名字在别处时经宿主）。监听器是总线的，不是 Cordis 的，这样的插件因此能到达它们。

### 5.4 方式

语义以 Cordis 4.0.4 `events.ts` 为准（即设计的表）。"开始"指调用本进程的监听器，或发出它的 `events.call`（或调用宿主里的 Rust 监听器）。

| 方式 | 分发方做什么 | 发出方得到（`events.dispatch` 的回复） |
| --- | --- | --- |
| `emit` | 按登记顺序开始每个调用，不等任何一个 | 每个调用都开始之后回复；发出方的 API 早已返回（`void` / `None` / `nil` / `Ok(())`） |
| `parallel` | 按登记顺序开始每个调用，然后等全部完成 | 全部成功时 `undefined`；否则 `throw` `AggregateError`，子错误为各失败，**按登记顺序**（不是完成顺序），只有一个失败时也如此 |
| `serial` | 一次开始一个，逐个等待 | 第一个有结果的返回值；监听器的错误原样抛出并停止其余；都没有结果时 `undefined` |
| `waterfall` | 带 `next` 调用第一个条目；条目 *i* 的 `next` 调用条目 *i+1*（跳过已撤下的）；最后一个的 `next` 以事件参数调用 `terminal`，没有终态时得到 `undefined` | 最外层监听器的返回值，**原样**（不做"有没有结果"的归一，`false` 仍是 `false`）。没有条目时：终态的值，或 `undefined`。不调 `next` 的监听器否决其余 |
| `bail` | 同 `serial`，每个调用都是同步的（`sync: true`） | 同 `serial`，同步 |
| `waterfall-sync` | 同 `waterfall`，同步，`next` 同步调用 | 同 `waterfall`，同步（只有 Rust 发出方） |

`next` 的规则：不带参数；第二次调用、在它的分发结束或被取消之后调用，这次调用以 `WaterfallNext` 失败（第十一节）。`waterfall` 中，接收方的运行时给监听器的 `next` 返回 Promise / 可等待对象 / Go 的阻塞调用；`waterfall-sync` 中同步调用这个引用。

同步方式与监听器种类：`bail` / `waterfall-sync` 中，`async` 种类的条目不被调用，这次分发以 `AsyncListener` 失败。`any` 种类的条目被调用；如果它返回 Promise / thenable / 可等待对象，监听器所在的运行时丢弃它（Python 协程被关闭；Promise 的拒绝被捕获并忽略），这次调用以 `AsyncListener` 失败。Go 的监听器是 `sync`。Rust：`on` 的条目是 `async`，`on_sync` 的是 `sync`。每个同步调用都像同步服务调用一样参加等待环检测（`SyncWaitCycle`）。

### 5.5 emit 的顺序

对每个（发出的进程，名字），每个监听器上 `emit` 分发的调用按发出顺序**开始**：

- 宿主按顺序处理一个会话的 `events.dispatch` 帧，先开始同一进程同一名字前一次 emit 的调用，再开始后一次的；它自己的 Rust 监听器保留内核按键排队的链。
- 发往同一会话的帧按序到达，运行时**必须**按到达顺序开始 `emit` 方式的调用：Node / Bun / Python 在处理下一帧之前调用监听器（同步执行到它的第一个 `await`），进程的一个同步调用正在等待时也是这样（等待期间扣住其他调用的运行时，对 `emit` 方式的 `events.call` 仍就地处理；2026-10-21 修订）；Go 按监听器排队，一个接一个执行（不同的监听器并发执行）。
- 过滤器可能要等待（可以是异步的），`emit` 的调用在过滤器决定之后才开始。快速路径上 N 的 `emit` 在等它的过滤器时，运行时把本进程之后 N 的 emit（无论有没有过滤器、来自哪一行）排在它后面，轮到时再决定走哪条路径，所以仍按发出顺序开始（用例 `emit/order-filtered-then-unfiltered`）。
- 运行时经宿主发出了 N 的一次 `emit`，在它得到回复之前，N 的后续 emit 都继续经宿主（§5.2）。宿主在开始各调用之后才回复，所以这次 emit 的 `events.call` 先于任何后来的 N 的快速路径 emit 到达运行时。
- 回复是普通值（`undefined`），不是 future：宿主在各调用开始之后写出它。运行时像处理 `events.elsewhere` 一样（§4.1）**就地**、按帧的顺序接收它：读到回复的那一帧时就把这次 emit 记为已得到回复，处理完再读下一帧，而不是放到之后的任务、tick 或线程里。所以后面的帧引起的操作（比如调用它的一个服务、在那里发出 N）都看到它已得到回复；宿主在发出那一帧之前已回复的 dispatch，不再让 N 留在经宿主的路径上（2026-10-21 修订）。

### 5.6 `emit` 中的错误

`emit` 的监听器失败不影响发出方、不影响其他监听器，也从不结束进程。它在监听器所在的一侧报告一次：

- 运行时：stderr 一行 `rutis: listener of <name> in row <row> failed: <ErrorName>: <message>`。
- 宿主：它的错误汇集（rutis-host 打印同样的文字）。
- 无法送达或得不到回复的调用（传输、值能力、已撤下）：由分发方报告：`rutis: listener of <name> in <runtime <runtime>|node <peer>> failed: <ErrorName>: <message>`。

`emit` 分发中过滤函数失败时，不送达任何监听器，由分发方同样报告（`listener` 换成 `filter`：`rutis: filter of <name> from row <row> failed: …`）。挂载方式加载的插件（`row: null`）发出时，`from row <row>` 写作 `from the mount`，第十一节的 `emit` 那一行也一样。

### 5.7 取消

- **发出方放弃**（Rust future 被丢弃、Python 任务被取消、Go context 被取消；JavaScript 没有取消分发的办法）：经宿主时，运行时对它的 `events.dispatch` 调用发 `cancel`。分发方不再开始新的调用，对每个进行中的调用发 `cancel`（其 `signal` 被中止），之后的 `next` 调用以 `WaterfallNext` 失败。发出方的调用以本语言的取消结束。`emit` 分发不被取消。
- **接收方**把中止的 `signal` 对应为：Go，监听器的 `context.Context` 被取消；Python，监听器的任务被取消；Rust，监听器的 future 被丢弃。JavaScript 的监听器没有逐次调用的信号：调用继续执行，回复被丢弃（只有卸载（第七节）会中止 `ctx.signal`）。
- 不加调用超时。

## 六、过滤

- **在哪里执行**：总是在发出方的进程里。快速路径上，运行时对每个候选条目直接调用插件的过滤函数。经宿主时，`events.dispatch` 带 `filter`：运行时包在插件过滤函数外面的一个**选择器**的函数引用。宿主每次分发调用它一次，在快照（§5.1）之后，参数只有一个：非 `global` 候选条目的描述列表，按登记顺序。选择器在发出方的进程里按顺序对每个描述调用插件的过滤函数，返回等长的布尔值列表（`true` = 送达），真假按本语言自己的规则 / `bool`。`bail` 分发中宿主同步调用它，其余异步。长度不对或不是布尔值时这次分发失败（`BindingError`，`filter of <name> returned <n> answers for <m> listeners`）。发出方自己会话里的条目，其描述还带 `listener`：该条目的 `events.on` id（2026-10-19 修订）。运行时凭它找到自己的监听器，绝不凭描述的其他字段（同一行在两个实例里的两份副本，或一个 Cordis 插件从两个 Context 监听，描述完全相同），并在调用插件的过滤函数前去掉它，使过滤函数在两条路径上看到的描述相同。
- **错误**：过滤函数抛出时，整次分发以这个错误失败，不开始任何调用（`emit`：§5.6）。Go 的过滤函数返回 `(bool, error)`：它返回的错误（连同名字）就是这次失败，panic 也是。Rust 的 `try_filter` 接受返回 `Result<bool, CordisError>` 的函数，它返回的错误原样就是这次失败；`filter` 接受只返回 `bool` 的函数，只能以 panic 失败。
- **默认**：先应用作用域规则，并且总是应用；过滤只在其上再筛。`global` 条目不经过滤。
- **Cordis**（`Context.filter`）：`ctx.emit(thisArg, name, …)` 以及其余四种，`thisArg` 的 `[Context.filter]` 是函数时，用它作过滤。对本进程的监听器传入条目自己的 Cordis `Context`，对其他每个条目传入 `{ rutis: <描述> }`。rutis 的 Node 运行时里每个 Context 都有只读的 `ctx.rutis`，值为它的描述，所以 `c => c.rutis.row === 'chat'` 对两者都有效。
- **其他语言**：`filter(predicate)` 返回一个有同样五个方法的发出器（第十二节）；predicate 收到描述（Node / Bun 叶子与 Python：带 JSON 字段的普通对象 / `dict`；Go：`rutis.Listener`；Rust：`&ListenerInfo`）。

## 七、生命周期与断开

### 7.1 卸载

一行卸载时（宿主发 `rows.unload`，或运行时报告 `rows.ended`），依次：

1. 宿主在发出 `rows.unload` 或收到 `rows.ended` 时，把这一行的条目撤出监听器表（并发 `events.elsewhere`）；运行时再为它们发 `events.off` 也无妨。
2. 运行时立即不再接受这一行的调用：之后到达的 `events.call` 以已撤下的错误失败（§5.3）。
3. 撤下提供的服务（同 0.12）。
4. 这一行正在运行的监听器调用收到取消信号（Go：context；Python：任务取消；JavaScript：这一行的 `ctx.signal` 被中止；Rust：fiber 的停止，与它的其他任务相同），并被等待，最多 `RUTIS_SHUTDOWN_TIMEOUT`（默认 10 秒；Rust 插件用宿主自己的关闭截止时间）。超过时运行时写 `rutis: listeners of row <row> still running after <s>s; unloading anyway`，放弃它们，继续。
5. 按逆序执行清理，然后回复 `rows.unload`。

条目被撤下时已经发出的调用失败一次，错误为已撤下的错误或传输错误；从不重试。

### 7.2 断开

运行时会话或节点连接结束时，宿主撤下经它来的全部条目（如同它们的行卸载了），并发 `events.elsewhere`。发往它的进行中的调用失败一次，错误与服务调用遇到的传输错误相同（`BindingError`，信息为会话的结束）：`parallel`、`serial`、`waterfall`、`bail` 交给发出方（`parallel` 在 `AggregateError` 里）；`emit` 报告（§5.6）。对它的同步等待同样失败。运行时一侧卸载每一行（同 0.12），并把 elsewhere 当作空。

## 八、框架事件 `rutis/*`

只由宿主以 `emit` 方式发出，每个只有一个参数（一个记录）。插件可以监听；任何插件发出 `rutis/*` 都以 `ReservedEvent` 失败。

| 名字 | 参数 | 何时发出 | 发出方作用域 |
| --- | --- | --- | --- |
| `rutis/service` | `{ "name": <服务名>, "scope": <链>, "available": <bool>, "row": <提供它的行或 null> }` | 一个服务名在一个作用域里有了提供方（`available: true`）；提供方被替换（再次 `true`，新的 `row`）；被撤下且不再有提供方（`false`，`row` 为最后的提供方） | 服务的作用域 |
| `rutis/row` | `{ "row": <行的 id>, "plugin": <名字>, "scope": <链>, "state": "pending" \| "running" \| "failed" \| "stopped", "error": <信息或 null> }` | 配置的行（任何语言，包括 Rust 行）每次状态变化；`failed` 时带 `error`。`row` 是配置给这一行的 id（加载器的行 id，同描述的 `row`，§4.2），不是会话里的行键 | 行的作用域 |
| `rutis/runtime` | `{ "runtime": <名字>, "state": "running" \| "down", "reason": <文字或 null> }` | 运行时挂载成功时（每次重连之后也是）`running`；会话结束或挂载失败时 `down`，`reason` 说明原因（退出状态、传输错误、拒绝信息） | 根 |

`internal/*` 留在每个运行时内部，从不向宿主登记或发给宿主。

## 九、节点连接

- **共享哪些事件**：连接已有的 `export` / `import` 列表，与服务共用。一项是一个名字，或以结尾的 `*` 写出的前缀（`session/*`；单独的 `*` 是所有名字）。前缀项只用于事件；服务仍按确切名字。
- **含义**：节点 A 上的 `export: [x]` 表示 A 发出的 `x` 到达 B 的监听器；B 上的 `import: [x]` 表示 B 的监听器收到 A 的 `x`。所以监听器条目从导入方流向导出方：B 通告它与 `import` 匹配的条目，A 接受与它的 `export` 匹配的。`events: { out }` 改为 `export`，`events: { in }` 改为 `import`。同一个名字可以双向共享。
- **连接上的消息**（`events` 族，在连接的会话上；连接开始时每个节点登记这个族）：

  | 方法 | 参数 | 含义 |
  | --- | --- | --- |
  | `events.on` | `[{ "id", "name", "prefix", "plugin", "row", "runtime", "kind", "prepend", "global" }]` | 发送方的一个条目，提供给接收方。接收方提供 `events` 时，发送方按表中顺序通告整个表，`prepend: false`；此后每个条目在登记时通告，`prepend` 如登记时：接收方的顺序与发送方一致 |
  | `events.off` | `[[id, …]]` | 撤下 |
  | `events.call` | 同 §5.3 | 调用发送方接受的一个条目 |

- **节点通告什么**：只通告它自己的条目（宿主的 Rust 插件和它的运行时的），从不通告经任何连接引进的条目（不转发，所以不会成环）；并且只通告作用域链是连接作用域前缀的条目（连接的作用域是 `rutis-bridge/peer` 行所在的作用域；不在实例里的连接为根），因为只有它们能收到到达那里的事件。前缀条目按它与 `import` 项共同匹配的部分收窄后通告（两个前缀：一个延伸另一个时取较长的；一个前缀与一个名字：取名字）。
- **接受的条目放在哪里**：放在接受方节点上连接的作用域，按到达顺序追加，以 `prepend: true` 通告的放在全部已有条目之前；描述的 `node` = 对端 id，`plugin` / `row` / `runtime` 按通告，`scope` = 连接的作用域。通告来的条目若不匹配 `export` 的任何项，则忽略（回复 `undefined`）。
- **宣告的 id**：`<epoch>.<n>`。`epoch` 是发送方这一次提供 `events` 族的序号（它的 `link.offers` 里这一族的 `since`），`n` 在这次提供内递增。接收方看到对端更新的提供时（经 `link.offers`，或一个 epoch 更大的宣告先到），撤下对端在更早的提供下宣告的全部条目；对端不再提供 `events` 时同样撤下；更早提供的迟到宣告不接受（回复 `undefined`）。不带 epoch 的 id（不说明它的实现）只按 id 替换（2026-10-19 修订）。
- **规则**：与本机条目完全相同：五种方式、作用域、过滤、值类型、撤下与断开（§7.2）。被调用的节点执行这个条目（它的 Rust 监听器，或把 `events.call` 转给它的运行时）。断开的连接的条目被撤下。

## 十、值

事件参数、结果、`next` / 终态的结果、过滤用的描述，在每条路径上都是会话值：十种值标记、函数与对象的引用、信号、64 层的深度上限，编码与能力规则与调用相同（[协议插件设计 §6.1](design-protocol-plugins-2026-09-25.md#61-会话协议的值标记现行规范)）。发送方按接收方的能力检查；发不出的值**只让那次调用失败**（一个监听器：`parallel` 中是 `AggregateError` 的一个子错误，`emit` 中是一行报告），对 `events.dispatch` 则是整次分发失败。节点连接同样承载它们（0.12 的"只能 JSON"规则取消）。作为事件参数传递的引用，与调用参数一样存活到这次调用结束。

## 十一、错误

| `name` | 何时 | 信息（确切；`<…>` 填入） |
| --- | --- | --- |
| `AggregateError` | `parallel` 有失败 | `<n> listeners of <name> failed`；子错误按登记顺序：线上，错误图的根是名为 `AggregateError` 的 `error` 节点，其 `errors` 引用各子错误的节点（每个子错误自己的图并入其中），与 JavaScript 编码 `AggregateError` 的方式相同 |
| `AsyncListener` | `bail` / `waterfall-sync` 遇到异步监听器 | `listener of <name> in <监听器> is asynchronous and cannot run in a synchronous <bail\|waterfall-sync>`；`<监听器>` 是 `row <行>`、`runtime <运行时>`、`node <对端>` 或 `plugin <插件>`，取描述里第一个有值的 |
| `WaterfallNext` | `next` 调用两次 / 结束后 / 取消后 / 不在 waterfall 中（Rust）/ Rust 同步监听器在异步 waterfall 中 | `next of <name> was already called` · `next of <name> was called after its waterfall ended` · `next is available only in a waterfall (this is <mode>)` · `a synchronous listener cannot call next in an asynchronous waterfall; register it with on` · `next of <name> was called off the thread of its waterfall`（Rust：别处监听器的同步 `next` 不在分发它的线程上调用） |
| `ReservedEvent` | 发出 `rutis/*`（任何方式、任何语言，由发出的 API 同步拒绝） | `rutis/* events are emitted by rutis only: <name>` |
| `SyncWaitCycle` | 一次同步调用会使等待成环 | 同服务调用 |
| `BindingError` | 传输错误、已撤下的监听器、被拒绝的帧 | 传输的信息，或第三节、§4.2、§5.3、第六节给出的信息 |

监听器自己的错误带着名字、信息与错误图跨边界，与任何调用的错误相同。

没有人等待的失败在发生的一侧报告，每个运行时都是一行 stderr（§5.6 的监听器与过滤函数那两行之外）：

| 何时 | 行（原文；`<…>` 填入） |
| --- | --- |
| 经宿主发送的 `emit` 被拒绝或没有得到回答 | `rutis: emit of <name> from row <row> failed: <ErrorName>: <message>` |
| 宿主拒绝一个监听器的 `events.on`（行加载期间则改为让加载失败） | `rutis: registration of listener of <name> in row <row> failed: <ErrorName>: <message>` |

## 十二、API

### 12.1 Rust

```rust
// rutis (kernel); everything below is in `rutis::events`, the common types also at the crate root
pub trait Event: Serialize + DeserializeOwned + Send + Sync + 'static {
    const NAME: &'static str;
    type Value: Serialize + DeserializeOwned + Send + 'static;
}   // every event crosses processes: derive Serialize and Deserialize

pub enum Mode { Emit, Parallel, Serial, Waterfall, Bail, WaterfallSync }

pub struct Call<'a, V> { /* .. */ }
impl<'a, V> Call<'a, V> {
    pub fn mode(&self) -> Mode;
    pub fn name(&self) -> &str;
    /// The rest of the chain (`waterfall` only; otherwise `Err(WaterfallNext)`).
    pub fn next(self) -> BoxFuture<'a, Result<Option<V>, CordisError>>;
}
pub struct SyncCall<'a, V> { /* .. */ }
impl<'a, V> SyncCall<'a, V> {
    pub fn mode(&self) -> Mode;
    pub fn name(&self) -> &str;
    /// `waterfall_sync` only; in an async `waterfall`: `Err(WaterfallNext)`.
    pub fn next(self) -> Result<Option<V>, CordisError>;
}

pub trait Listener<E: Event>: Send + Sync + 'static {
    fn call<'a>(&'a self, ctx: &'a Ctx, event: &'a E, call: Call<'a, E::Value>)
        -> BoxFuture<'a, Result<Option<E::Value>, CordisError>>;
}   // implemented for `for<'a> Fn(&'a Ctx, &'a E, Call<'a, E::Value>) -> BoxFuture<'a, …>`
pub trait SyncListener<E: Event>: Send + Sync + 'static {
    fn call(&self, ctx: &Ctx, event: &E, call: SyncCall<'_, E::Value>)
        -> Result<Option<E::Value>, CordisError>;
}   // implemented for `Fn(&Ctx, &E, SyncCall<'_, E::Value>) -> Result<…>`
pub trait PatternListener<E: Event> { /* as Listener, plus `key: EventKey<E>` before `event` */ }
pub trait SyncPatternListener<E: Event> { /* as SyncListener, plus `key` */ }
pub trait Terminal<E: Event>: Send + 'static {
    fn call<'a>(&'a self, ctx: &'a Ctx, event: &'a E) -> BoxFuture<'a, Result<Option<E::Value>, CordisError>>;
}   // implemented for such closures, and for `()`: no terminal (the last `next` gives `None`)

#[non_exhaustive]
pub struct EventOptions { pub prepend: bool, pub once: bool, pub global: bool }
// built with EventOptions::default().prepend(true).once(true).global(true)

impl EventBus {   // ctx.events()
    pub fn on<E: Event>(&self, ctx: &Ctx, key: &EventKey<E>, l: impl Listener<E>) -> Result<Disposer, CordisError>;
    pub fn on_opt<E: Event>(&self, ctx: &Ctx, key: &EventKey<E>, l: impl Listener<E>, o: EventOptions) -> Result<Disposer, CordisError>;
    pub fn on_sync<E: Event>(&self, ctx: &Ctx, key: &EventKey<E>, l: impl SyncListener<E>) -> Result<Disposer, CordisError>;
    pub fn on_sync_opt<E: Event>(&self, ctx: &Ctx, key: &EventKey<E>, l: impl SyncListener<E>, o: EventOptions) -> Result<Disposer, CordisError>;
    pub fn once<E: Event>(&self, ctx: &Ctx, key: &EventKey<E>, l: impl Listener<E>) -> Result<Disposer, CordisError>;
    pub fn on_pattern<E: Event>(&self, ctx: &Ctx, p: &EventPattern<E>, l: impl PatternListener<E>) -> Result<Disposer, CordisError>;
    pub fn on_sync_pattern<E: Event>(&self, ctx: &Ctx, p: &EventPattern<E>, l: impl SyncPatternListener<E>) -> Result<Disposer, CordisError>;
    // and on_pattern_opt / on_sync_pattern_opt with EventOptions

    pub fn emit<E: Event>(&self, ctx: &Ctx, key: &EventKey<E>, e: Arc<E>) -> Result<(), CordisError>;
    pub fn parallel<'a, E: Event>(&self, ctx: &'a Ctx, key: &EventKey<E>, e: Arc<E>) -> BoxFuture<'a, Result<(), CordisError>>;
    pub fn serial<'a, E: Event>(&self, ctx: &'a Ctx, key: &EventKey<E>, e: Arc<E>) -> BoxFuture<'a, Result<Option<E::Value>, CordisError>>;
    pub fn waterfall<'a, E: Event, T: Terminal<E> + 'a>(&self, ctx: &'a Ctx, key: &EventKey<E>, e: &'a E, terminal: T) -> BoxFuture<'a, Result<Option<E::Value>, CordisError>>;
    pub fn bail_sync<E: Event>(&self, ctx: &Ctx, key: &EventKey<E>, e: &E) -> Result<Option<E::Value>, CordisError>;
    pub fn waterfall_sync<E: Event, F>(&self, ctx: &Ctx, key: &EventKey<E>, e: &E, terminal: F) -> Result<Option<E::Value>, CordisError>
        where F: FnOnce(&Ctx, &E) -> Result<Option<E::Value>, CordisError>;

    /// The same six emitting methods, filtered (§6).
    pub fn filter(&self, keep: impl Fn(&ListenerInfo) -> bool + Send + Sync + 'static) -> Filtered<'_>;
    /// 过滤函数可以失败的 `filter`：它的错误让这次分发失败（第六节）。
    pub fn try_filter(&self, keep: impl Fn(&ListenerInfo) -> Result<bool, CordisError> + Send + Sync + 'static) -> Filtered<'_>;
}

pub struct ListenerInfo {
    pub name: String, pub prefix: bool, pub plugin: String, pub row: Option<String>,
    pub runtime: Option<String>, pub node: Option<String>, pub scope: Vec<Scope>, pub global: bool,
}
pub struct Scope { pub id: String, pub group: String }

impl Ctx {
    /// The chain of loader instances around this context; `[]` at the root.
    pub fn scope(&self) -> Vec<Scope>;
    /// Make this context's fiber a scope of `group` (the loader calls it for
    /// each group instance before its rows start).
    pub fn mark_scope(&self, group: &str) -> Result<(), CordisError>;
    /// Make this context's fiber the row `key`: `ListenerInfo::row` of the
    /// listeners registered in it and below it (the loader calls it for each row).
    pub fn mark_row(&self, key: &str) -> Result<(), CordisError>;
    /// The nearest marked row.
    pub fn row(&self) -> Option<String>;
}
```

rutis-bridge 在其上搭建逻辑总线的部分（宿主一侧；插件不用）：

```rust
/// How one Rust type crosses as JSON; carried by every payload and value.
pub struct Codec { /* .. */ }   // Codec::of::<T>(), to_json / from_json, to_args / from_args (the argument mapping below)
pub struct Payload { /* .. */ } // an event: Payload::new(Arc<E>), Payload::from_boxed(box, codec)
pub struct PayloadRef<'a> { /* .. */ }
pub struct Value { /* .. */ }   // a result: Value::new(v), Value::from_boxed(box, codec), into_typed::<T>(transcoder)
/// Converts an event or a value to another type: a listener whose type differs
/// from the dispatch's, a result of another type. Default: JSON. The bridge
/// installs its own (session values, `Args`) with `EventBus::set_transcoder`.
pub trait Transcoder: Send + Sync + 'static {
    fn event(&self, from: PayloadRef<'_>, into: &Codec) -> Result<Box<dyn Any + Send>, CordisError>;
    fn value(&self, from: &Value, into: &Codec) -> Result<Box<dyn Any + Send>, CordisError>;
}

pub enum ListenerKind { Sync, Async, Any }
/// A remote entry: a listener whose calls go to an external dispatcher
/// (a runtime session, a node link). Same table, positions, scope rule,
/// filter (by its `ListenerInfo`) and `global` as any entry.
pub trait RemoteListener: Send + Sync + 'static {
    fn kind(&self) -> ListenerKind;   // `Async`: never called by bail / waterfall-sync (AsyncListener)
    /// Called in registration order when the call starts (for `emit`, before
    /// the dispatch resolves); dropping the future cancels the call (§5.7).
    fn call<'a>(&'a self, call: RemoteCall<'a>) -> BoxFuture<'a, Result<Option<Value>, CordisError>>;
    /// Inline on the dispatching thread, no kernel lock held: a bridge's
    /// sync-wait path (SyncWaitCycle) applies as to any synchronous call.
    fn call_sync(&self, call: RemoteSyncCall<'_>) -> Result<Option<Value>, CordisError>;
}
// RemoteCall / RemoteSyncCall: name(), mode(), dispatch() (one id per dispatch),
// origin() (the raw dispatch's tag), emitter() (&Ctx), event() (PayloadRef),
// next(): Option<RemoteNext> / Option<RemoteSyncNext> — 'static, Clone; the first
// call runs the rest of the chain, a second fails "already called", a call after
// the entry's call ended fails "called after its waterfall ended".
pub struct RemoteEntry { pub name: String, pub prefix: bool, pub info: ListenerInfo, pub options: EventOptions, pub origin: u64 }

impl EventBus {
    pub fn on_remote(&self, ctx: &Ctx, entry: RemoteEntry, l: Arc<dyn RemoteListener>) -> Result<Disposer, CordisError>;
    /// The current entries, then every change; a Rust `on` is reported before it returns (§4.4).
    pub fn watch_table(&self, owner: &Ctx, f: impl Fn(&TableChange) + Send + Sync + 'static) -> Result<(Vec<TableEntry>, Disposer), CordisError>;
    /// A dispatch whose event is erased (a runtime's `events.dispatch`): name, mode, payload,
    /// optional emitter scope, batch filter (§6 selector), terminal, origin, `framework` (`rutis/*`).
    /// For `emit` it resolves once every call started.
    pub fn dispatch<'a>(&self, ctx: &'a Ctx, d: Dispatch) -> BoxFuture<'a, Result<Option<Value>, CordisError>>;
    pub fn dispatch_sync(&self, ctx: &Ctx, d: Dispatch) -> Result<Option<Value>, CordisError>;   // bail, waterfall-sync
    /// Call one entry by id outside a dispatch (a node running the other node's `events.call`, §9).
    pub fn invoke_entry<'a>(&self, ctx: &'a Ctx, id: u64, call: EntryCall) -> BoxFuture<'a, Result<Option<Value>, CordisError>>;
    pub fn invoke_entry_sync(&self, ctx: &Ctx, id: u64, call: EntryCall) -> Result<Option<Value>, CordisError>;
    /// `emit` of a framework event: only rutis emits `rutis/*`.
    pub fn emit_framework<E: Event>(&self, ctx: &Ctx, key: &EventKey<E>, e: Arc<E>) -> Result<(), CordisError>;
}
```

- **名字**：`EventKey::<E>::of()` 的名字是 `E::NAME`；`EventKey::named(n)` / `EventKey::dynamic(n)` 的名字是 `n`。`EventPattern::prefix(p)` 登记前缀条目。监听器只按名字查找：同名的另一个 Rust 类型的监听器也收到这个事件，由转码器（serde）转换。
- **参数 ↔ `E`**（每个事件类型）：`E` 序列化为两个及以上字段的元组或元组结构体 → 每个字段一个参数；unit 或 unit 结构体 → 没有参数；其余（结构体、newtype、map、序列）→ 一个参数。解码：0 个参数 → 从 unit；1 个 → 从这个参数；2 个及以上 → 把列表当作元组（`Codec::to_args` / `from_args`）。bridge 的转码器按服务参数的规则编码（`session::arg` / `decode_value`），所以 `Bytes`、`Instant`、`ValueMap`、`ObjectRef`、`RemoteFunction` 作为值标记与引用跨边界。
- **动态事件**：`rutis_bridge::events::Args(pub Vec<rutis_bridge::session::Value>)`，`Value = rutis_bridge::events::ArgValue`（`ArgValue(pub rutis_bridge::session::Value)`），只以 `EventKey::<Args>::dynamic(name)` 使用（用 `of()` 以 `BindingError` 失败，`Args events need a name: EventKey::dynamic(name)`）；bridge 的转码器原样取用它的值。
- Rust 监听器以登记它的上下文被调用（它的 `ctx.cancelled()` 就是卸载信号，§7.1）。
- `serial` / `bail_sync` 中 `Ok(Some(false))` 是没有结果；`waterfall` 原样返回值。
- 作用域：发出方的作用域是 `ctx.scope()`；插件没有向别的作用域发出的 API（改用按 `ListenerInfo::scope` 过滤）。
- 删除：`on_waterfall*`、`on_waterfall_sync*`、`WaterfallListener`、`SyncWaterfallListener`、`Next`、`SyncNext`、`SyncEvent`、`EventKey::instance`、所有 `*_instance` 与 `*_keyed` 方法、`EventPattern::any_prefix`。

### 12.2 其他语言

`[thisArg,]` 是 Cordis 可选的第一个参数。每种语言里，`waterfall` 的监听器都把 `next` 作为**最后一个**参数收到。

| | Node（Cordis 插件） | Node / Bun（叶子） | Python | Go |
| --- | --- | --- | --- | --- |
| 监听 | `ctx.on(name, fn, options?) → () => boolean`（`options`：`prepend`、`global`；也有 `ctx.once`） | `ctx.on(name, fn, { prepend?, global? }?) → () => void` | `ctx.on(name, fn, *, prepend=False, global_=False) → Callable[[], None]` | `ctx.On(name string, fn func(ctx context.Context, args ...any) (any, error), options ...rutis.ListenOption) func()`（`rutis.Prepend()`、`rutis.Global()`） |
| 监听器 | `(...args[, next]) => value \| Promise` | 同左 | `fn(*args[, next]) -> value \| awaitable` | `fn(ctx, args...)`；waterfall 中最后一个参数是 `rutis.Next`（`func(context.Context) (any, error)`） |
| `emit` | `ctx.emit([thisArg,] name, ...args): void` | `ctx.emit(name, ...args): void` | `ctx.emit(name, *args) -> None` | `ctx.Emit(ctx context.Context, name string, args ...any) error`（只报拒绝） |
| `parallel` | `await ctx.parallel([thisArg,] name, ...args)` | `await ctx.parallel(name, ...args)` | `await ctx.parallel(name, *args)` | `ctx.Parallel(ctx, name, args...) error` |
| `serial` | `await ctx.serial([thisArg,] name, ...args)` | `await ctx.serial(name, ...args)` | `await ctx.serial(name, *args)` | `ctx.Serial(ctx, name, args...) (any, error)` |
| `waterfall` | `ctx.waterfall([thisArg,] name, ...args, terminal)`（Cordis：最后一个参数是终态） | `ctx.waterfall(name, ...args)`；有终态时 `ctx.waterfall.with(terminal)(name, ...args)`（2026-10-19 修订：参数都是事件的） | `await ctx.waterfall(name, *args, terminal=None)` | `ctx.Waterfall(ctx, name string, terminal rutis.Terminal, args ...any) (any, error)`（`rutis.Terminal` = `func(context.Context, ...any) (any, error)`，以事件参数调用；可为 nil） |
| `bail` | `ctx.bail([thisArg,] name, ...args)`（同步） | `ctx.bail(name, ...args)` | `ctx.bail(name, *args)`（同步） | `ctx.Bail(ctx, name, args...) (any, error)` |
| 过滤 | `thisArg[Context.filter] = c => boolean`；`c.rutis` 是描述 | `ctx.filter(d => boolean)` → `{ emit, parallel, serial, waterfall, bail }` | `ctx.filter(lambda d: bool)` → 带这五个方法的对象 | `ctx.Filter(func(rutis.Listener) (bool, error)) *rutis.Emitter`（五个方法；返回的错误或 panic 即过滤失败） |
| 卸载信号 | `ctx.signal`（这一行的；每个 Cordis Context 上也有） | `ctx.signal` | 任务取消 | 监听器的 `ctx` |
| parallel 失败 | `AggregateError` | `AggregateError` | `rutis.AggregateError` | `*rutis.AggregateError` |

- 没有结果到达时是 `undefined`（JS）、`None`、`nil`。Node 的 `waterfall` 只要有监听器在本进程之外就返回 Promise；插件 `await` 它。
- Go：`rutis.Listener` = `struct { Name string; Prefix bool; Plugin, Row, Runtime, Node string; Scope []rutis.Scope; Global bool }`（null 为 `""`）；`rutis.Scope` = `struct { ID, Group string }`；`rutis.Next` = `func(context.Context) (any, error)`。Python 与叶子插件的描述是带 JSON 字段名的普通 `dict` / 对象。
- 保留的名字：叶子插件（Python、Go、Node 叶子）不能监听或发出 `internal/*`（同 0.12）；`rutis/*` 可以监听，不能发出。
- **测试工具**（`@arcships/rutis/testing`、`rutis.testing`、`rutistest`）：同样的五种方法、`filter`、作用域（可以把一行加载到一个命名的作用域路径，如 `t1/s1`）、卸载等待与取消。`@arcships/rutis/testing` 与 `rutis.testing` 都在一个进程里（总是快速路径）；`rutistest` 让插件在真实的 Go 运行时里经真实的会话运行，测试一方就是总线的宿主（`loaded.On` 像宿主一样监听）。它们不发出 `rutis/*`。

## 十三、事件期望文件

跨语言的一致性清单，与 `weather.expected.json` 同类：`crates/rutis-bridge/src/runtime/testing/events.expected.json`，由真实运行时的 Rust 测试（`session_matrix.rs`；两节点与 netem 场景增加 `node` 位置）和各测试工具读取。下面的起始用例就是这个文件的初始内容（#464 第 7 步）；新规则增加用例，不改格式。

### 13.1 一致性插件

每种语言提供插件 `conformance-events`（Rust 宿主提供同名的 Rust 插件）。每个监听器、每个发出方都是它的一行；行键是参与者的 `label`。它的配置是一个参与者（一个监听器，或 `{ "emitter": true }`）。监听器行按配置登记一个监听器；发出方行提供服务 `emitter-<label>`，方法 `dispatch(spec)`（异步；`bail` 用同步方法 `dispatch_sync(spec)`），用本语言原生的 API 发出并返回结果或抛出。每次调用开始、过滤调用与取消，都通过调用宿主的同步服务 `events-log.add(entry)` 记录。`events-log.until(entry)`（同步）记下 `until:<entry>`，等到 `entry` 被记下后返回（spec 的 `then` 用它）。

### 13.2 格式

顶层：`about`（怎样读）、`plugin`、`scopes`（作用域路径 → 组：运行器创建这些实例）、`locations`、`cases`。

| 位置 | 含义 |
| --- | --- |
| `X` | 被测的运行时 |
| `Y` | 另一个运行时进程：运行器对每个用例运行一次 Y 为 X 同语言的第二个进程，再对其他每种语言各运行一次 |
| `host` | 宿主里的 Rust 插件 |
| `node` | 与宿主相连的第二个节点上的 Rust 插件；两边都 `export` 并 `import` `t/*` 与 `rutis/*`，不含 `n/*`；节点上的监听器在节点的根 |

一个**用例**：`case`（id）、`rule`（本文的章节）、`name`（默认事件名）、`listeners`、`steps`，可选 `vary`、`only`、`except`、`tool`。

- **监听器**：`label`、`at`（位置）、`scope`（作用域路径，默认 `root`）、`name`（默认用例的）、`kind`（`sync` / `async` / `any`；用例的每一步都用 `bail` 或 `waterfall-sync` 时默认 `sync`，否则 `async`；Go 总是 `sync`；JavaScript / Python 的 `async` = 异步函数，`any` = 返回 `does` 所返回内容的普通函数）、`options`（`{ "prepend": true }`、`{ "global": true }`；所有语言）、`log`（默认 `"label"`；`"arg"`：记 `label:<args[0]>`；字段名列表：记 `args[0]` 的这些字段 `label:<f1>,<f2>`；`false`：不记）、`does`：

  | `does` | 监听器 |
  | --- | --- |
  | `{ "returns": v }` | 返回 `v`（`null`：JS `null`、Python `None` ……） |
  | `{ "returnsNothing": true }` | 返回 `undefined` / `None` / `nil` / `None` |
  | `{ "echo": true }` | 返回 `args[0]` |
  | `{ "throws": { "name", "message" } }` | 抛出这个名字与信息的错误 |
  | `{ "after": ms, "then": does }` | 等 `ms` 毫秒，再做 `then` |
  | `{ "next": "<text>" }` | 返回 `"<text>" + r`，`r` 为 `next()` 的结果（没有结果时为 `""`） |
  | `{ "veto": v }` | 不调 `next`，返回 `v` |
  | `{ "nextTwice": true }` | 调两次 `next()`，返回第二次的结果 |
  | `{ "waits": true }` | 等待取消，然后记 `label:cancelled` 并返回 `null` |
  | `{ "calls": 0 }` | 以 `"x"` 调用 `args[0]`（一个函数），返回它的结果 |
  | `{ "emits": spec }` | 从自己这一行发出 `spec`（见下），返回结果 |

- **发出说明**（spec）：`from`（位置）、`row`（发出方 label，默认 `e`）、`scope`（默认 `root`）、`mode`、`name`（默认用例的）、`args`（值；值标记写作 `{"$<tag>": …}`，同 `weather.expected.json`；`{"$callback":"upper"}` 是把参数转成大写的函数）、`filter`（`{ "keep": [labels] }`：送达 `row` 在列表中的条目；探针对它被调用时的每个描述记 `filter:<row>`）、`via`（`"thisArg"`：Cordis 的 `Context.filter`，只用于 Node 的 Cordis 发出方）、`terminal`（文字：记 `terminal` 并返回这段文字的终态；`{ "echo": true }`：记 `terminal` 并返回它的第一个参数，即事件的第一个参数）、`cancel`（`{ "when": "<label>" }`：那个监听器记下开始后，发出方取消这次分发）、`then`（`{ "until": "<entry>", "args": [..] }`，只用于 `emit`：分发之后发出方同步调用 `events-log.until(entry)`，返回后用这些参数再发出一次同样的事件；用例关心的是这次等待期间到达它的帧）。
- **发出方的行**：运行器在 spec 的 `from` 位置与 `scope` 创建它写明的发出方（`row`，默认 `e`）；监听器的 `emits` 从监听器自己的行发出。
- **步骤**，按顺序：`{ "emit": spec, "expect": … }`（`"background": true`：这一步不等分发结束；下一个 `unload` / `kill` 步骤的 `expect` 说的是这次分发的结果与日志）、`{ "load": listener }`（新的一行；`provides`：它另外提供一个这个名字的空服务）、`{ "unload": label }`、`{ "kill": "Y" | "node" }`（结束那个进程 / 那条连接；在 `restart` 步骤之前不恢复）、`{ "restart": "Y" }`、`{ "concurrent": [steps], "hold": true }`（`hold`：宿主的测试钩子扣住这些分发直到全部到达；之后 `expect` 适用于每一个，`throws` 表示至少一个）。`unload` 与 `kill` 可带 `"when": "<label>"`：先等那个监听器开始。`unload` 可带 `"then": "<entry>"`：那一行卸载后（带 `answered` 时还要等宿主回复了收到的分发），执行器记下这一条，再去等后台的分发（`then.until` 等的就是它）。
- **期望**：`returns`（值；`null` = 没有结果）、`throws`（`{}` 任意错误，或 `name`、`message`（子串）、`errors`（按顺序的子错误，每个为 `{}` / `name` / `message`））、`log`（确切顺序）、`logUnordered`、`logContains`（按此顺序出现，中间可有其他）、`reported`（`[{ "label", "name" }]`，§5.6）、`filterSees`（过滤函数被调用时的行，按顺序；运行器按每个参与者实际所在核对描述的 `runtime`、`node` 与 `scope`）、`viaHost`（是否有 `events.dispatch` 到达宿主）、`settles`（`true`：这一步在 5 秒内结束，无论成败）、`answered`（`true`：运行器等到宿主对它收到的每个 `events.dispatch` 都写出了回复，最多 5 秒；之后发给运行时的帧都在这些回复之后到达，按 §5.5 发出方已把这些经宿主的 emit 记为已得到回复，用于其后断言 `viaHost: false` 的步骤）、`logLacks`（日志里不得出现的条目）。日志在整个用例中累积；每个日志期望说的都是到此为止的全部日志。运行器最多等 5 秒让日志完整。
- **`vary`**：位置要变化的参与者（label，发出方为 `e`），运行器取所有位置的组合（笛卡尔积），`at` / `from` 是第一种组合。`node` 不被分配到 `root` 以外的作用域。
- **`only` / `except`**：`{ "<label>": [语言] }`（`rust`、`node`、`bun`、`python`、`go`）限定 / 排除组合，带 `why`。
- **`tool`**：测试工具不运行该用例时为 `false`（它需要其他进程、`rutis/*` 或宿主）。测试工具把其余每个用例的全部位置都设为 `X` 运行。

### 13.3 起始用例

```json
{
  "about": [
    "Unified events conformance (#464): one list both the real runtimes and the SDK testing tools are checked against. Format: docs/design-events-protocol-2026-10-18.en.md §13."
  ],
  "plugin": "conformance-events",
  "scopes": { "t1": "tenant", "t1/s1": "session", "t2": "tenant" },
  "locations": ["X", "Y", "host", "node"],
  "cases": [
    { "case": "emit/reaches-all", "rule": "5.4 emit", "name": "t/emit",
      "listeners": [
        { "label": "a", "at": "X", "does": { "after": 100, "then": { "returns": "A" } } },
        { "label": "b", "at": "host", "does": { "returns": null } },
        { "label": "c", "at": "Y", "does": { "returns": "C" } } ],
      "vary": ["a", "b", "c", "e"],
      "steps": [ { "emit": { "from": "X", "mode": "emit", "args": [1] }, "expect": { "returns": null, "logUnordered": ["a", "b", "c"] } } ] },

    { "case": "emit/order-per-listener", "rule": "5.5", "name": "t/order",
      "listeners": [ { "label": "c", "at": "Y", "log": "arg", "does": { "returns": null } } ],
      "vary": ["c", "e"],
      "steps": [
        { "emit": { "from": "X", "mode": "emit", "args": [1] } },
        { "emit": { "from": "X", "mode": "emit", "args": [2] } },
        { "emit": { "from": "X", "mode": "emit", "args": [3] }, "expect": { "log": ["c:1", "c:2", "c:3"] } } ] },

    { "case": "emit/order-across-fast-path", "rule": "5.2, 5.5", "name": "t/switch", "tool": false,
      "listeners": [
        { "label": "a", "at": "X", "log": "arg", "does": { "returns": null } },
        { "label": "b", "at": "Y", "log": false, "does": { "returns": null } } ],
      "steps": [
        { "emit": { "from": "X", "mode": "emit", "args": [1] }, "expect": { "viaHost": true, "answered": true } },
        { "unload": "b" },
        { "emit": { "from": "X", "mode": "emit", "args": [2] }, "expect": { "log": ["a:1", "a:2"], "viaHost": false } } ] },

    { "case": "emit/order-across-fast-path-sync-wait", "rule": "5.2, 5.5", "name": "t/switch-wait", "tool": false,
      "listeners": [
        { "label": "a", "at": "X", "log": "arg", "does": { "returns": null } },
        { "label": "b", "at": "Y", "log": false, "does": { "returns": null } } ],
      "steps": [
        { "emit": { "from": "X", "mode": "emit", "args": [1], "then": { "until": "resume", "args": [2] } }, "background": true,
          "expect": { "logContains": ["until:resume"] } },
        { "unload": "b", "then": "resume", "expect": { "answered": true, "returns": null, "viaHost": false, "logContains": ["a:1", "a:2"] } } ] },

    { "case": "emit/error-isolated", "rule": "5.6", "name": "t/emit-error",
      "listeners": [
        { "label": "a", "at": "Y", "does": { "throws": { "name": "Boom", "message": "a failed" } } },
        { "label": "b", "at": "X", "does": { "returns": null } } ],
      "vary": ["a", "b", "e"],
      "steps": [ { "emit": { "from": "X", "mode": "emit", "args": [] },
        "expect": { "returns": null, "logUnordered": ["a", "b"], "reported": [ { "label": "a", "name": "Boom" } ] } } ] },

    { "case": "parallel/all-ok", "rule": "5.4 parallel", "name": "t/par",
      "listeners": [
        { "label": "a", "at": "X", "does": { "returns": "A" } },
        { "label": "b", "at": "host", "does": { "returns": null } },
        { "label": "c", "at": "node", "does": { "returns": "C" } } ],
      "vary": ["a", "b", "c", "e"],
      "steps": [ { "emit": { "from": "X", "mode": "parallel", "args": [] }, "expect": { "returns": null, "logUnordered": ["a", "b", "c"] } } ] },

    { "case": "parallel/errors-in-registration-order", "rule": "5.4 parallel", "name": "t/par-errors",
      "listeners": [
        { "label": "a", "at": "Y", "does": { "after": 300, "then": { "throws": { "name": "AErr", "message": "a" } } } },
        { "label": "b", "at": "X", "does": { "throws": { "name": "BErr", "message": "b" } } },
        { "label": "c", "at": "host", "does": { "returns": null } } ],
      "vary": ["a", "b", "c", "e"],
      "steps": [ { "emit": { "from": "X", "mode": "parallel", "args": [] },
        "expect": { "throws": { "name": "AggregateError", "errors": [ { "name": "AErr" }, { "name": "BErr" } ] }, "logUnordered": ["a", "b", "c"] } } ] },

    { "case": "parallel/one-error-is-aggregated", "rule": "5.4 parallel", "name": "t/par-one",
      "listeners": [ { "label": "a", "at": "Y", "does": { "throws": { "name": "AErr", "message": "a" } } } ],
      "vary": ["a", "e"],
      "steps": [ { "emit": { "from": "X", "mode": "parallel", "args": [] }, "expect": { "throws": { "name": "AggregateError", "errors": [ { "name": "AErr" } ] } } } ] },

    { "case": "serial/first-result", "rule": "1 no result, 5.4 serial", "name": "t/serial",
      "listeners": [
        { "label": "a", "at": "X", "does": { "returns": null } },
        { "label": "b", "at": "Y", "does": { "returns": false } },
        { "label": "c", "at": "host", "does": { "returns": "C" } },
        { "label": "d", "at": "node", "does": { "returns": "D" } } ],
      "vary": ["a", "b", "c", "d", "e"],
      "steps": [ { "emit": { "from": "X", "mode": "serial", "args": ["x"] }, "expect": { "returns": "C", "log": ["a", "b", "c"] } } ] },

    { "case": "serial/no-result", "rule": "1 no result", "name": "t/serial-none",
      "listeners": [
        { "label": "a", "at": "X", "does": { "returnsNothing": true } },
        { "label": "b", "at": "Y", "does": { "returns": false } },
        { "label": "c", "at": "host", "does": { "returns": null } } ],
      "vary": ["a", "b", "c", "e"],
      "steps": [ { "emit": { "from": "X", "mode": "serial", "args": [] }, "expect": { "returns": null, "log": ["a", "b", "c"] } } ] },

    { "case": "serial/zero-is-a-result", "rule": "1 no result", "name": "t/serial-zero",
      "listeners": [ { "label": "a", "at": "Y", "does": { "returns": 0 } }, { "label": "b", "at": "X", "does": { "returns": 1 } } ],
      "vary": ["a", "b", "e"],
      "steps": [ { "emit": { "from": "X", "mode": "serial", "args": [] }, "expect": { "returns": 0, "log": ["a"] } } ] },

    { "case": "serial/error-stops", "rule": "5.4 serial", "name": "t/serial-error",
      "listeners": [
        { "label": "a", "at": "Y", "does": { "throws": { "name": "AErr", "message": "a" } } },
        { "label": "b", "at": "X", "does": { "returns": "B" } } ],
      "vary": ["a", "b", "e"],
      "steps": [ { "emit": { "from": "X", "mode": "serial", "args": [] }, "expect": { "throws": { "name": "AErr", "message": "a" }, "log": ["a"] } } ] },

    { "case": "waterfall/chain", "rule": "5.4 waterfall", "name": "t/wf",
      "listeners": [
        { "label": "a", "at": "X", "does": { "next": "a" } },
        { "label": "b", "at": "Y", "does": { "next": "b" } },
        { "label": "c", "at": "host", "does": { "veto": "c" } } ],
      "vary": ["a", "b", "c", "e"],
      "steps": [ { "emit": { "from": "X", "mode": "waterfall", "args": [] }, "expect": { "returns": "abc", "log": ["a", "b", "c"] } } ] },

    { "case": "waterfall/veto", "rule": "5.4 waterfall", "name": "t/wf-veto",
      "listeners": [
        { "label": "a", "at": "X", "does": { "next": "a" } },
        { "label": "b", "at": "Y", "does": { "veto": "v" } },
        { "label": "c", "at": "node", "does": { "veto": "c" } } ],
      "vary": ["a", "b", "c", "e"],
      "steps": [ { "emit": { "from": "X", "mode": "waterfall", "args": [] }, "expect": { "returns": "av", "log": ["a", "b"] } } ] },

    { "case": "waterfall/terminal", "rule": "5.4 waterfall", "name": "t/wf-terminal",
      "listeners": [ { "label": "a", "at": "Y", "does": { "next": "a" } }, { "label": "b", "at": "host", "does": { "next": "b" } } ],
      "vary": ["a", "b", "e"],
      "steps": [ { "emit": { "from": "X", "mode": "waterfall", "args": [], "terminal": "t" }, "expect": { "returns": "abt", "log": ["a", "b", "terminal"] } } ] },

    { "case": "waterfall/no-listeners", "rule": "5.4 waterfall", "name": "t/wf-empty", "listeners": [],
      "vary": ["e"],
      "steps": [
        { "emit": { "from": "X", "mode": "waterfall", "args": [], "terminal": "t" }, "expect": { "returns": "t", "log": ["terminal"] } },
        { "emit": { "from": "X", "mode": "waterfall", "args": [] }, "expect": { "returns": null, "log": ["terminal"] } } ] },

    { "case": "waterfall/end-without-terminal", "rule": "5.4 waterfall", "name": "t/wf-end",
      "listeners": [ { "label": "a", "at": "Y", "does": { "next": "a" } } ],
      "vary": ["a", "e"],
      "except": [ { "e": ["node", "bun"], "why": "a Cordis waterfall always has a terminal (its last argument); the probe passes one returning undefined, which is the same" } ],
      "steps": [ { "emit": { "from": "X", "mode": "waterfall", "args": [] }, "expect": { "returns": "a", "log": ["a"] } } ] },

    { "case": "waterfall/next-twice", "rule": "5.4 next", "name": "t/wf-twice",
      "listeners": [ { "label": "a", "at": "Y", "does": { "nextTwice": true } }, { "label": "b", "at": "X", "does": { "veto": "b" } } ],
      "vary": ["a", "b", "e"],
      "steps": [ { "emit": { "from": "X", "mode": "waterfall", "args": [] }, "expect": { "throws": { "name": "WaterfallNext" }, "log": ["a", "b"] } } ] },

    { "case": "waterfall/error", "rule": "5.4 waterfall", "name": "t/wf-error",
      "listeners": [ { "label": "a", "at": "X", "does": { "next": "a" } }, { "label": "b", "at": "Y", "does": { "throws": { "name": "BErr", "message": "b" } } } ],
      "vary": ["a", "b", "e"],
      "steps": [ { "emit": { "from": "X", "mode": "waterfall", "args": [] }, "expect": { "throws": { "name": "BErr" }, "log": ["a", "b"] } } ] },

    { "case": "waterfall-sync/chain", "rule": "5.4 waterfall-sync", "name": "t/wfs", "tool": false,
      "listeners": [ { "label": "a", "at": "X", "does": { "next": "a" } }, { "label": "b", "at": "Y", "does": { "next": "b" } } ],
      "vary": ["a", "b"],
      "steps": [ { "emit": { "from": "host", "mode": "waterfall-sync", "args": [], "terminal": "t" }, "expect": { "returns": "abt", "log": ["a", "b", "terminal"] } } ] },

    { "case": "bail/first-result", "rule": "5.4 bail", "name": "t/bail",
      "listeners": [
        { "label": "a", "at": "Y", "does": { "returns": null } },
        { "label": "b", "at": "host", "does": { "returns": "B" } },
        { "label": "c", "at": "X", "does": { "returns": "C" } } ],
      "vary": ["a", "b", "c", "e"],
      "steps": [ { "emit": { "from": "X", "mode": "bail", "args": [] }, "expect": { "returns": "B", "log": ["a", "b"] } } ] },

    { "case": "bail/async-listener", "rule": "5.4 kinds", "name": "t/bail-async",
      "listeners": [ { "label": "a", "at": "Y", "kind": "async", "does": { "returns": "A" } } ],
      "vary": ["a", "e"],
      "except": [ { "a": ["go"], "why": "Go listeners are synchronous" } ],
      "steps": [ { "emit": { "from": "X", "mode": "bail", "args": [] }, "expect": { "throws": { "name": "AsyncListener" }, "log": [] } } ] },

    { "case": "bail/awaitable-from-plain-function", "rule": "5.4 kinds", "name": "t/bail-any",
      "listeners": [ { "label": "a", "at": "Y", "kind": "any", "does": { "after": 10, "then": { "returns": "A" } } } ],
      "vary": ["a", "e"],
      "only": [ { "a": ["node", "bun", "python"], "why": "only JavaScript and Python functions can return an awaitable unannounced" } ],
      "steps": [ { "emit": { "from": "X", "mode": "bail", "args": [] }, "expect": { "throws": { "name": "AsyncListener" }, "log": ["a"] } } ] },

    { "case": "bail/call-back-into-waiting-emitter", "rule": "5.4 bail, sync-wait", "name": "t/bail-out", "tool": false,
      "listeners": [
        { "label": "p", "at": "Y", "does": { "emits": { "from": "Y", "row": "p", "mode": "bail", "name": "t/bail-back", "args": [] } } },
        { "label": "q", "at": "X", "name": "t/bail-back", "does": { "returns": "Q" } } ],
      "vary": ["p", "q"],
      "steps": [ { "emit": { "from": "X", "mode": "bail", "args": [] }, "expect": { "returns": "Q", "log": ["p", "q"] } } ] },

    { "case": "bail/wait-cycle", "rule": "5.4 bail, SyncWaitCycle", "name": "t/c1", "tool": false,
      "listeners": [ { "label": "p", "at": "Y", "does": { "returns": "P" } }, { "label": "q", "at": "X", "name": "t/c2", "does": { "returns": "Q" } } ],
      "only": [ { "X": ["node"], "Y": ["node"], "why": "a cycle needs two runtimes that do not declare reentrant-sync" } ],
      "steps": [ { "concurrent": [
          { "emit": { "from": "X", "mode": "bail", "args": [] } },
          { "emit": { "from": "Y", "row": "f", "mode": "bail", "name": "t/c2", "args": [] } } ], "hold": true,
        "expect": { "settles": true, "throws": { "name": "SyncWaitCycle" } } } ] },

    { "case": "scope/up-not-down", "rule": "3", "name": "t/scope",
      "listeners": [
        { "label": "r", "at": "host", "scope": "root", "does": { "returns": null } },
        { "label": "u", "at": "X", "scope": "t1", "does": { "returns": null } },
        { "label": "s", "at": "Y", "scope": "t1/s1", "does": { "returns": null } },
        { "label": "v", "at": "X", "scope": "t2", "does": { "returns": null } } ],
      "vary": ["r", "u", "s", "v", "e"],
      "steps": [
        { "emit": { "from": "X", "scope": "t1/s1", "mode": "parallel", "args": [] }, "expect": { "logUnordered": ["r", "u", "s"] } },
        { "emit": { "from": "X", "scope": "t1", "mode": "parallel", "args": [] }, "expect": { "logUnordered": ["r", "u", "s", "r", "u"] } },
        { "emit": { "from": "X", "scope": "t2", "mode": "parallel", "args": [] }, "expect": { "logUnordered": ["r", "u", "s", "r", "u", "r", "v"] } },
        { "emit": { "from": "X", "scope": "root", "mode": "parallel", "args": [] }, "expect": { "logUnordered": ["r", "u", "s", "r", "u", "r", "v", "r"] } } ] },

    { "case": "filter/keeps-and-sees", "rule": "6", "name": "t/filter",
      "listeners": [
        { "label": "a", "at": "X", "does": { "returns": null } },
        { "label": "b", "at": "host", "does": { "returns": null } },
        { "label": "c", "at": "Y", "does": { "returns": null } },
        { "label": "d", "at": "X", "scope": "t2", "does": { "returns": null } } ],
      "vary": ["a", "b", "c", "e"],
      "steps": [ { "emit": { "from": "X", "mode": "serial", "args": [], "filter": { "keep": ["a", "c"] } },
        "expect": { "returns": null, "log": ["filter:a", "filter:b", "filter:c", "a", "c"],
          "filterSees": ["a", "b", "c"] } } ] },

    { "case": "filter/cordis-this-arg", "rule": "6 Cordis", "name": "t/filter-cordis",
      "listeners": [ { "label": "a", "at": "Y", "does": { "returns": null } }, { "label": "b", "at": "X", "does": { "returns": null } } ],
      "vary": ["a", "b"],
      "only": [ { "e": ["node", "bun"], "why": "Context.filter is Cordis's" } ],
      "steps": [ { "emit": { "from": "X", "mode": "parallel", "args": [], "via": "thisArg", "filter": { "keep": ["b"] } }, "expect": { "logUnordered": ["filter:a", "filter:b", "b"] } } ] },

    { "case": "filter/global-skips-filter", "rule": "4.2 global, 6", "name": "t/filter-global",
      "listeners": [ { "label": "g", "at": "Y", "options": { "global": true }, "does": { "returns": null } } ],
      "vary": ["g", "e"],
      "steps": [ { "emit": { "from": "X", "mode": "parallel", "args": [], "filter": { "keep": [] } }, "expect": { "log": ["g"] } } ] },

    { "case": "order/prepend", "rule": "4.3", "name": "t/prepend",
      "listeners": [ { "label": "b", "at": "X", "does": { "returns": null } }, { "label": "a", "at": "Y", "options": { "prepend": true }, "does": { "returns": null } } ],
      "vary": ["a", "b", "e"],
      "steps": [ { "emit": { "from": "X", "mode": "serial", "args": [] }, "expect": { "log": ["a", "b"] } } ] },

    { "case": "values/every-tag", "rule": "10", "name": "t/values",
      "listeners": [ { "label": "a", "at": "Y", "does": { "echo": true } } ],
      "vary": ["a", "e"],
      "steps": [ { "emit": { "from": "X", "mode": "serial", "args": [ [
          { "$instant": "2026-10-15T03:22:15.123Z" }, { "$datetime": "2026-10-15T14:30:05.25" }, { "$date": "2026-10-15" },
          { "$time": "14:30:05.25" }, { "$bytes": "aGVsbG8=" }, { "$map": [[1, "a"]] }, { "$set": ["a"] },
          { "$bigint": "-123456789012345678901" }, { "$decimal": "1.10" }, { "$uuid": "0f8fad5b-d9cb-469f-a165-70867728950e" } ] ] },
        "expect": { "returns": [
          { "$instant": "2026-10-15T03:22:15.123Z" }, { "$datetime": "2026-10-15T14:30:05.25" }, { "$date": "2026-10-15" },
          { "$time": "14:30:05.25" }, { "$bytes": "aGVsbG8=" }, { "$map": [[1, "a"]] }, { "$set": ["a"] },
          { "$bigint": "-123456789012345678901" }, { "$decimal": "1.10" }, { "$uuid": "0f8fad5b-d9cb-469f-a165-70867728950e" } ] } } ] },

    { "case": "values/function-argument", "rule": "10", "name": "t/callback",
      "listeners": [ { "label": "a", "at": "Y", "does": { "calls": 0 } } ],
      "vary": ["a", "e"],
      "steps": [ { "emit": { "from": "X", "mode": "serial", "args": [ { "$callback": "upper" } ] }, "expect": { "returns": "X" } } ] },

    { "case": "unload/waits-and-cancels", "rule": "7.1", "name": "t/unload",
      "listeners": [ { "label": "b", "at": "Y", "does": { "waits": true } } ],
      "vary": ["b", "e"],
      "steps": [
        { "emit": { "from": "X", "mode": "parallel", "args": [] }, "background": true },
        { "unload": "b", "when": "b", "expect": { "settles": true, "log": ["b", "b:cancelled"] } } ] },

    { "case": "unload/not-called-after", "rule": "7.1", "name": "t/unloaded",
      "listeners": [ { "label": "a", "at": "X", "does": { "returns": null } }, { "label": "b", "at": "Y", "does": { "returns": "B" } } ],
      "vary": ["a", "b", "e"],
      "steps": [ { "unload": "b" }, { "emit": { "from": "X", "mode": "serial", "args": [] }, "expect": { "returns": null, "log": ["a"] } } ] },

    { "case": "registration/loaded-row-hears", "rule": "4.4", "name": "t/late", "tool": false,
      "listeners": [],
      "steps": [
        { "load": { "label": "z", "at": "Y", "does": { "returns": "Z" } } },
        { "emit": { "from": "X", "mode": "serial", "args": [] }, "expect": { "returns": "Z", "log": ["z"] } } ] },

    { "case": "cancel/emitter-gives-up", "rule": "5.7", "name": "t/cancel", "tool": false,
      "listeners": [ { "label": "b", "at": "Y", "does": { "waits": true } } ],
      "vary": ["b", "e"],
      "except": [ { "e": ["node", "bun"], "why": "JavaScript cannot cancel a dispatch" }, { "b": ["node", "bun"], "why": "JavaScript listeners have no per-invocation signal" } ],
      "steps": [ { "emit": { "from": "X", "mode": "parallel", "args": [], "cancel": { "when": "b" } }, "expect": { "throws": {}, "log": ["b", "b:cancelled"] } } ] },

    { "case": "disconnect/in-flight-fails-once", "rule": "7.2", "name": "t/gone", "tool": false,
      "listeners": [ { "label": "a", "at": "X", "does": { "returns": null } }, { "label": "b", "at": "Y", "does": { "waits": true } } ],
      "steps": [
        { "emit": { "from": "X", "mode": "serial", "args": [] }, "background": true },
        { "kill": "Y", "when": "b", "expect": { "throws": { "name": "BindingError" } } },
        { "emit": { "from": "X", "mode": "serial", "args": [] }, "expect": { "returns": null, "log": ["a", "b", "a"] } } ] },

    { "case": "disconnect/emit-reported", "rule": "7.2, 5.6", "name": "t/gone-emit", "tool": false,
      "listeners": [ { "label": "b", "at": "Y", "does": { "waits": true } } ],
      "steps": [
        { "emit": { "from": "X", "mode": "emit", "args": [] } },
        { "kill": "Y", "when": "b", "expect": { "reported": [ { "label": "b", "name": "BindingError" } ] } } ] },

    { "case": "disconnect/node-link", "rule": "7.2, 9", "name": "t/gone-node", "tool": false,
      "listeners": [ { "label": "n", "at": "node", "does": { "waits": true } }, { "label": "a", "at": "X", "does": { "returns": "A" } } ],
      "steps": [
        { "emit": { "from": "X", "mode": "serial", "args": [] }, "background": true },
        { "kill": "node", "when": "n", "expect": { "throws": { "name": "BindingError" } } },
        { "emit": { "from": "X", "mode": "serial", "args": [] }, "expect": { "returns": "A", "log": ["n", "a"] } } ] },

    { "case": "node/not-shared", "rule": "9", "name": "n/private", "tool": false,
      "listeners": [ { "label": "n", "at": "node", "does": { "returns": "N" } } ],
      "steps": [ { "emit": { "from": "host", "mode": "serial", "args": [] }, "expect": { "returns": null, "log": [] } } ] },

    { "case": "rutis/row", "rule": "8", "name": "rutis/row", "tool": false,
      "listeners": [
        { "label": "w", "at": "X", "scope": "root", "log": ["row", "state"], "does": { "returns": null } },
        { "label": "o", "at": "X", "scope": "t1", "log": ["row", "state"], "does": { "returns": null } } ],
      "steps": [
        { "load": { "label": "z", "at": "Y", "scope": "t2", "name": "t/unused", "does": { "returns": null } } },
        { "unload": "z", "expect": { "logContains": ["w:z,running", "w:z,stopped"], "logLacks": ["o:z,running", "o:z,stopped"] } } ] },

    { "case": "rutis/service", "rule": "8", "name": "rutis/service", "tool": false,
      "listeners": [ { "label": "w", "at": "X", "log": ["name", "available"], "does": { "returns": null } } ],
      "steps": [
        { "load": { "label": "z", "at": "Y", "provides": "zsvc", "name": "t/unused", "does": { "returns": null } } },
        { "unload": "z", "expect": { "logContains": ["w:zsvc,true", "w:zsvc,false"] } } ] },

    { "case": "rutis/runtime", "rule": "8", "name": "rutis/runtime", "tool": false,
      "listeners": [ { "label": "w", "at": "X", "log": ["state"], "does": { "returns": null } } ],
      "steps": [ { "kill": "Y" }, { "restart": "Y", "expect": { "logContains": ["w:down", "w:running"] } } ] },

    { "case": "reserved/rutis-emit", "rule": "8, 11", "name": "rutis/fake", "listeners": [],
      "vary": ["e"],
      "steps": [
        { "emit": { "from": "X", "mode": "emit", "args": [] }, "expect": { "throws": { "name": "ReservedEvent" } } },
        { "emit": { "from": "X", "mode": "serial", "args": [] }, "expect": { "throws": { "name": "ReservedEvent" } } } ] },

    { "case": "fast-path/local-only", "rule": "5.2", "name": "t/local", "tool": false,
      "listeners": [ { "label": "a", "at": "X", "does": { "returns": "A" } } ],
      "steps": [
        { "emit": { "from": "X", "mode": "serial", "args": [] }, "expect": { "returns": "A", "viaHost": false } },
        { "load": { "label": "b", "at": "Y", "does": { "returns": "B" } } },
        { "emit": { "from": "X", "mode": "serial", "args": [] }, "expect": { "returns": "A", "viaHost": true } } ] },

    { "case": "rust/typed-args", "rule": "12.1", "name": "t/typed", "tool": false,
      "listeners": [ { "label": "h", "at": "host", "typed": "struct", "does": { "echo": true } }, { "label": "k", "at": "host", "name": "t/typed-pair", "typed": "tuple", "does": { "echo": true } } ],
      "steps": [
        { "emit": { "from": "X", "mode": "serial", "args": [ { "city": "Oslo" } ] }, "expect": { "returns": { "city": "Oslo" } } },
        { "emit": { "from": "X", "mode": "serial", "name": "t/typed-pair", "args": ["x", 2] }, "expect": { "returns": ["x", 2] } } ] },

    { "case": "rust/next-outside-waterfall", "rule": "11 WaterfallNext", "name": "t/rust-next", "tool": false,
      "listeners": [ { "label": "h", "at": "host", "does": { "next": "h" } } ],
      "steps": [ { "emit": { "from": "X", "mode": "serial", "args": [] }, "expect": { "throws": { "name": "WaterfallNext" } } } ] }
  ]
}
```

上面用到的 Rust 专用监听器字段：`typed`（`"struct"`：类型化事件 `struct { city: String }`；`"tuple"`：`struct Pair(String, i64)`；两者都派生 `Serialize` / `Deserialize`，每个 Rust 事件都如此）。`does` 为 `next` 的 Rust 监听器，无论以哪种方式被调用都调用 `call.next()`。

文件之外另由 Rust 测试检查：§2.2 对 0.12 运行时与节点的握手拒绝（跨版本端到端测试）；第四至九节的每种帧按所写解码与拒绝（会话单元测试）；卸载超过截止时间（§7.1）。

## 十四、现有协议的变化

| 现有 | 变化 |
| --- | --- |
| `hello.capabilities` | + `events-v2`（必需，§2.1） |
| `mount` | − `events`，− `emits`；+ `scope`、+ `runtime`（第三节）；回复的特性 `events` 删除 |
| `rows.load` | 第七个参数 `{ scope, id }`（第三节） |
| `rows.unload`、`rows.ended`、会话结束 | 撤下这一行 / 这个会话的条目（第七节） |
| 运行时 → 宿主的 `event`、宿主 → 运行时的 `emit` | 删除；由 `events.dispatch`、`events.call` 取代 |
| 节点连接的 `events.forward` | 删除；由连接上的 `events.on` / `events.off` / `events.call` 取代（第九节） |
| 连接配置 `export` / `import` | 也选择事件；`*` 前缀（第九节） |

## 十五、本规格做出的决定

设计没有说到的地方，本规格选择了：

1. **单独成文**而不是附录：协议插件设计除 §6.1 外是历史方案，它的第十一节被整体取代；它与运行时契约都指向本文。
2. **不升会话版本号；用一个必需的能力 `events-v2`**，在每条路径上于握手之后立即检查，信息见 §2.2。帧不变，升协议 2 / 3（以及 `rutis.3`）只会把明确的升级提示换成笼统的版本不符。
3. **插件 API 3**；0.13 运行时仍加载 API 2 的插件。
4. **只要有匹配的条目在发出方进程之外，就由宿主分发**；发出方进程只执行过滤（每次分发一次批量的选择器调用）和终态。此时发出方自己的条目也经 `events.call` 回调。
5. **"别处有监听器"以有序差异推送**名字与前缀，先发全集，就地处理；推送不考虑作用域（只是让快速路径少用一些）。
6. **emit 的顺序指开始的顺序**，按（发出的进程，名字）；宿主在开始各调用后回复 `emit` 分发；一个名字有更早的 emit 未得到回复时，运行时继续让它经宿主；Go 按监听器排队 emit 调用。
7. **登记顺序**是宿主接受的顺序，用位置表示；`prepend` 取最小的位置；`rows.load` 等这一行的 `events.on` 都得到回复；宿主在回复 `events.on` 之前排好推送。
8. **宿主撤下一行的条目**于发出 `rows.unload` 或收到 `rows.ended` 时；运行时立即拒绝这一行的调用。
9. **先快照，每次开始前再检查存活**：已撤下的条目静默跳过；已经发出的调用以 `BindingError`（`listener <id> of <name> is withdrawn`）或传输错误失败一次。
10. **监听器种类** `sync` / `async` / `any`：`bail` / `waterfall-sync` 从不调用已声明为异步的条目；`any` 条目看它的返回值判断。
11. **`parallel` 总是抛出 `AggregateError`**，只有一个失败也如此（同 Cordis）。
12. **`waterfall` 原样返回最外层的值**；没有条目时为终态的值或没有结果；`next` 不带参数；终态收到事件参数；Cordis 插件的终态是 Cordis 必需的最后一个参数；叶子插件用 `waterfall.with(terminal)` 给出（2026-10-19 修订：它的参数可以是函数，最后一个分不清），其他语言可选。
13. **过滤在快照（已按作用域筛过）上、在任何调用之前执行**；`global` 条目不经过滤（仍受作用域规则）；过滤函数抛出时整次分发失败（`emit` 中报告）。
14. **Cordis 过滤收到的上下文**：本进程的监听器为真实的 `Context`，其他条目为 `{ rutis: 描述 }`，每个 Context 上有 `ctx.rutis`；其他语言用 `filter(predicate)` 返回发出器。
15. **作用域是加载器的组实例**，链为 `[{id, group}]`，根为 `[]`，经 `rows.load` 第七个参数与 `mount` 字段下发；宿主按行确定发出方作用域，从不采信自报；Rust 增加 `Ctx::scope` / `Ctx::mark_scope`。
16. **取消按各语言的方式**；JavaScript 监听器没有逐次调用的信号（Cordis 监听器的签名没有它的位置），只在卸载时看到 `ctx.signal`；JavaScript 不能取消一次分发。
17. **卸载最多等 `RUTIS_SHUTDOWN_TIMEOUT`**（默认 10 秒），之后写 §7.1 的一行并继续。
18. **`rutis/*` 的参数**各为一个记录，状态用小写；以 `emit` 方式在对象所在的作用域发出；替换提供方时再发一次 `available: true`。
19. **节点连接**：`export` = 我发出的到达对端，`import` = 我的监听器收到对端的（所以 `events.out` → `export`，`events.in` → `import`）；条目从导入方流向导出方；结尾 `*` 为前缀，只用于事件；只通告节点自己的、位于连接作用域或其上的条目；引进的条目从不转发（事件不多跳）；接受的条目放在连接的作用域；同一名字可以双向共享。
20. **每个 Rust 事件类型都能跨进程**：`Event` 要求 `Serialize + DeserializeOwned`（派生），它的 `Value` 也是，所以没有只留在本进程的类型，也不需要选择（2026-10-18 修订；删除原先的 `const WIRE` / `NoWire`）。监听器按名字查找；同名的另一个 Rust 类型的监听器经总线的转码器（serde）收到转换后的事件。参数按元组 / unit / 单值对应；动态事件为 `rutis_bridge::events::Args`；`SyncEvent`、实例键与已弃用的 keyed / instance 方法随 `on_waterfall*` 一起删除；`Terminal` 返回 `Option`（`()` 表示没有终态）；`on` 保留登记用的 `ctx` 参数，Rust 监听器以这个上下文被调用。内核另加 `Ctx::mark_row`（宿主 Rust 监听器描述的 `row`），以及给 bridge 用的别处监听器条目（`RemoteListener`）、原始的 `dispatch` / `dispatch_sync`、`invoke_entry` 与 `watch_table`（§12.1）。
21. **Rust 的 `on_sync` 监听器在异步 waterfall 中不能调用 `next`**（`WaterfallNext`）；需要时用 `on` 登记。
22. **Go 监听器为 `func(ctx, args ...any) (any, error)`**，waterfall 中 `next` 是最后一个参数；`Emit` 只对拒绝返回错误。
23. **`emit` 方式的 `events.call` 在监听器结束后回复**，总是 `undefined`；失败在监听器所在处以 §5.6 的一行报告。
24. **错误名与信息**见第十一节，包括 `AsyncListener`、`WaterfallNext`、`ReservedEvent`；传输类失败保持 `BindingError`。
25. **Node 对每次分发在每个执行其监听器的进程里触发一次 Cordis `internal/dispatch`**，另加发出方自己的 Cordis 分发中的那一次。
26. **测试工具**提供作用域、过滤、卸载等待与取消（Node、Python 的在一个进程里，Go 的 `rutistest` 以测试一方为宿主运行真实的会话），不提供 `rutis/*`；期望文件以 `tool: false` 标出它们跳过的用例。
27. **`prepend` 与 `global` 在所有语言中都有**（2026-10-18 修订）：Node 的 Cordis 插件用 Cordis 自己的选项，Node / Bun 叶子用 `ctx.on(name, fn, { prepend, global })`，Python 用 `ctx.on(name, fn, prepend=…, global_=…)`，Go 用 `rutis.Prepend()` / `rutis.Global()`，Rust 用 `EventOptions`（第十二节）。

实现宿主侧时（#464，`events/host`）定下的：

28. **`mount` 写明运行时的名字**（`runtime`），快速路径上的描述与宿主的相同。
29. **宿主按顺序处理一个会话的 `emit` 分发，而不阻塞它的读线程**：能马上开始的 `emit`（没有待回的过滤选择器）在会话的有序任务上轮到时开始，并在那里作答（第 30 条）；不能马上开始的（它的选择器调用要从同一个会话回来），连同这个会话之后的每个 `emit`，进入队列，各自在调用都开始后作答。两种情况下回复都是普通值（`undefined`），不是 future，在各调用开始之后写出；运行时就地、按帧的顺序接收它（§5.5）。两种情况下 §5.5 的保证都成立。（2026-10-21 修订：原先排队的 `emit` 答的是一个运行时去等待的 future，运行时何时把这次 emit 记为已得到回复取决于时序。0.13 的宿主仍这样答；运行时仍会等这样的 future，对这种宿主保持 0.13 那种依赖时序的顺序。）
30. **宿主按帧序处理一个会话的事件操作，但不在它的读线程上**：会话层把收到的 `events.*` 依次交给这个会话的一个有序任务；`events.on`、`events.off`、`events.elsewhere` 以及 `emit` 方式的 `events.dispatch` / `events.call` 在轮到时就地执行，其余事件操作在轮到时作为各自的任务开始。读线程从不执行插件代码（监听器、分发观察者）：它们可能同步调用同一个会话，而回复只有这个读线程能读到。
31. **节点按自己的条目每匹配一个 `import` 项就宣告一次**（按 §9 收窄），每次宣告有自己的 id；对端宣告的条目同样按每匹配一个 `export` 项接受一次。宣告在对端提供 `events` 这一族时开始，对端每次重新提供时从头再来。
32. **线上的 `AggregateError`** 在错误图里带着子错误（§11），各语言都按登记顺序还原子错误。
33. **`rutis/row`**：加载器生成该行时发 `pending`，之后按加载器看到的 fiber 状态的每次变化发（`loading` 算作 `pending`；`unloading` 与销毁算作 `stopped`）；组行也是行（`plugin` 为 `group`）。**`rutis/service`** 针对每个宿主服务发出，不论提供方是 Rust 插件、运行时的行还是节点导入，位于提供方的作用域，带提供它的行的 id。
