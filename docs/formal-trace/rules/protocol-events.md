# 规则：协议清单 events（#634 评论）

[中文索引](../README.md) · [English](../README.en.md) · 来源：[评论 6384932886](https://github.com/arcships/rutis/issues/634#issuecomment-6384932886) · [保证登记表](../../guarantees.md)

本文件把 #634 的 `events` 分区评论（[评论 6384932886](https://github.com/arcships/rutis/issues/634#issuecomment-6384932886)）里的 Markdown 表格导出到仓库。表格内容原样保留；单元格里的 `|` 按 Markdown 表格规则转义为 `\|`，渲染后文字不变。列的含义见 #634 正文：编号、来源、规则、参与方、手段、对应、现有形式化、状态、归属 issue、备注。

抽取基线：`main` `3496669b`（issue 正文的统计口径）。**这批行没有按 0.22 的现行规格复核**：状态列仍是 issue 的词汇，`node tools/formal-trace.mjs` 在统计时按 `README.md` §3 的映射换成追踪表状态；映射只降不升，不改写本文件。

2026-11-11：EVP-5.7-4 按 #1121 的修复更新「现有形式化」「状态」「备注」（宿主侧已有用例，状态从「未覆盖」改为「部分」）；其余行仍是 issue 快照。

2026-11-16：EVP-15.33-2 按 #1171 的修复更新「现有形式化」「状态」「备注」：节点导入的服务带提供方的行 id（无行时不带），两个会话用例覆盖有行与无行，状态从「部分」改为「已覆盖」。

分区合计：现行 253 条，已取代 9 条，共 262 行（与 issue 正文的统计表一致）。

## 分区 `events`

> 分区 `events`：统一事件协议（EVP、EVD）
>
> 现行 253 条，已取代 9 条（列出、不计入）。"归属 issue"一列带 ※ 的是这次新分配或调整的。列的含义见 #634 正文。

| 编号 | 来源 | 规则 | 参与方 | 手段 | 对应 | 现有形式化 | 状态 | 归属 issue | 备注 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| EVP-1-1 | §一 没有结果 | `undefined`/`null`/`None`/`nil` 与 `false`（含 Rust `Some(false)`）是没有结果，其余（含 `0`、`""`）都是结果 | 纯逻辑 | K + PT | events/serial-bail | K `serial/first-result`、`serial/no-result`、`serial/zero-is-a-result`、`bail/no-result`、`rust/some-false-is-no-result` | 已覆盖 | — |  |
| EVP-2.1-1 | §2.1 | 每个 0.13+ 端（宿主、运行时、节点）在 compat 与 endpoint 两种格式的 `hello.capabilities` 里都声明 `events-v2` | 纯逻辑 | K | events/v2-required | K（隐含：`events_conformance.rs` 的每种运行时语言与 Rust 第二节点都要先过握手） | 部分 | #639 ※ | Cordis 节点（Node 作节点，`features.mjs`）不在套件里，只有 `cordis-events.test.mjs` 普通测试；`COMPAT_CAPABILITIES` 等是实现细节 |
| EVP-2.1-2 | §2.1 | 对端没有声明 `events-v2` 时，握手之后立即结束会话，不再发任何东西（没有 `mount`、没有 `link.offers`） | 跨进程 | PT + 测试 | events/v2-required、events/old-peer-refused | — | 未覆盖 | #631 | #631 的握手判定表 PT（含能力）覆盖判定；"什么都不发"现在只有 `rpc/tests.rs::a_far_end_without_events_v2_is_refused`、`event_hub.rs::a_runtime_without_events_v2_is_refused` |
| EVP-2.1-3 | §2.1 | 0.12 及更早的对端与 0.13+ 混用时在加载任何东西之前失败，本地/远程/接入的运行时与节点连接各路径都如此 | 跨进程 | 测试 | events/old-peer-refused | — | 不适用 | — | 要下载已发布的旧版本（`tests/e2e/tests/cross_version.rs`，`#[ignore]`，只在完整 CI） |
| EVP-2.2-1 | §2.2 | Rust（宿主或节点）拒绝报了实现的对端（endpoint 格式）时信息确切为 `incompatible session: the far end (<name> <version>) does not declare events-v2 …` | 外部 | 测试 | events/v2-required | — | 不适用 | — | 输出文字；`rpc.rs:1055` 与规格一致 |
| EVP-2.2-2 | §2.2 | compat 格式（对端不报实现）时信息确切为 `… the far end (a runtime older than rutis 0.13) does not declare events-v2 …` | 外部 | 测试 | events/v2-required | — | 不适用 | — | 输出文字 |
| EVP-2.2-3 | §2.2 | Rust 拒绝之后：`Handshake::Incompatible`；运行时以此错误启动失败；节点连接停止（`link stopped: incompatible (…)`），不重试 | 跨进程 | 测试 | events/old-peer-refused | — | 不适用 | — | 要跨版本对端（e2e）；判定部分见 EVP-2.1-2 |
| EVP-2.2-4 | §2.2 | 0.13+ 运行时遇到不声明 `events-v2` 的宿主：stderr 写 `rutis: refused: incompatible (this host does not declare events-v2 …); stopping`，退出状态 1；拨出的运行时不再重试 | 外部 | 测试 | events/old-peer-refused | — | 不适用 | — | 退出状态与输出文字；各语言有测试（`runner-events.test.mjs`、`old-host.test.ts`、`test_events_session.py`、`events_session_test.go`） |
| EVP-2.2-5 | §2.2 | 0.12 运行时遇到 0.13+ 宿主：hello 之后看到会话关闭，按会话丢失退出；用户读到宿主一侧的信息 | 跨进程 | 测试 | events/old-peer-refused | — | 不适用 | — | 旧版本的行为，只能跨版本 e2e |
| EVP-2.3-1 | §2.3 | 插件 API 版本为 3（Node / Python `PLUGIN_API`、Go 清单 `pluginApi`） | 纯逻辑 | 测试 | sdk/leaf-events | — | 不适用 | — | 静态常量，没有输入空间；代码均为 3（`node/rutis/src/index.mjs:35`、`python/rutis/rutis/plugin.py:111`、`bun/rutis-bun/src/plugin.ts:20`） |
| EVP-2.3-2 | §2.3 | 0.13+ 运行时加载声明 API 2 或 3 的插件 | 纯逻辑 | 测试 | sdk/leaf-events | — | 不适用 | — | 两个取值的判定，单元测试穷尽（`binary.rs` 的清单测试） |
| EVP-2.3-3 | §2.3 | 声明 API 3 的插件被旧运行时以现有信息拒绝 | 外部 | 测试 | — | — | 不适用 | — | 旧版本的行为，只能跨版本 e2e |
| EVP-2.4-1 | §2.4 | 协议删除 `mount` 参数 `events`/`emits`、`mount` 特性 `events`、运行时→宿主的 `event`、宿主→运行时的 `emit`（§2.1 先拒绝对端，到不了） | 纯逻辑 | 测试 | sdk/leaf-events | — | 不适用 | — | 删除项；Python 运行时对 mount 参数 `events`/`emits` 仍以另一条信息拒绝（`runner.py:83`，"the mount argument … was removed"），规格写"到不了" |
| EVP-2.4-2 | §2.4 | Cordis 挂载 / 生成器配置的 `events`、`emits` 以确切信息拒绝（`the config field <events\|emits> was removed in rutis 0.13: …`） | 外部 | 测试 | sdk/leaf-events | — | 不适用 | — | 输出文字；`cordis/build.rs:364`、`events.mjs:56` 一致 |
| EVP-2.4-3 | §2.4 | 节点连接配置 `events: { out, in }` 以确切信息拒绝，说明改为 `export` / `import` | 外部 | 测试 | events/node-config | — | 不适用 | — | 输出文字；`rutis-loader/src/peer.rs:455`、`features.mjs:251` |
| EVP-2.4-4 | §2.4 | Rust API 删除项（`RuntimePlugin::emits`、`Process::emit`、`on_waterfall*`、`*_instance`、`*_keyed`、`SyncEvent` 等）为编译错误 | 纯逻辑 | 测试 | core/D17 | — | 不适用 | — | 编译期 |
| EVP-2.4-5 | §2.4 | rutis-bridge 删除节点的 `events.forward`、`NodeEvent`、`EventsPlugin` | 纯逻辑 | 测试 | — | — | 不适用 | — | 编译期 |
| EVP-3-1 | §三 | 作用域是加载器的一个组实例或根；每个插件、行和每次发出恰在一个作用域（最内层实例或根） | 纯逻辑 | K | events/flat | K `filter/own-listeners-in-scopes`、`filter/every-listener`（`filterSees` 核对描述的 `scope`） | 已覆盖 | — |  |
| EVP-3-2 | §三 | 线上作用域链为 `[{id, group}]`，外层在前，根为 `[]`；`id` 不透明、在宿主现在与过去的全部实例中唯一，`group` 为组名 | 纯逻辑 | K + PT | events/flat | K `filterSees`（核对 `scope`） | 部分 | #639 ※ | "过去的全部实例中唯一"（不复用）无检查：套件每个作用域只建一次 |
| EVP-3-3 | §三 | 事件按名字送达：root 总线上这个名字的每个监听器都收到每次分发，不论作用域、运行时；作用域只是供过滤的描述 | 跨进程 | K | events/flat、events/hub-flat | K `scope/flat-bus`、`scope/siblings-share-the-bus`、`scope/bail-and-waterfall`、`rutis/row`、`rutis/service-scoped` | 已覆盖 | — | 0.15.1 改写（1f0a7b3b、4549f495）；#613 不建作用域 |
| EVP-3-4 | §三 | 相连的节点只增加其连接 `export` / `import` 承载的名字 | 跨进程 | K | events/flat、events/link-export-import | K `node/not-shared`、`node/root-hears-instances` | 已覆盖 | — |  |
| EVP-3-5 | §三 | 宿主经 `rows.load` 第七个参数 `{scope, id}` 告诉每一行它的链与 id；0.13+ 宿主总是发七个参数（无导出时 `exports` 为 `{}`） | 跨进程 | K | events/hub-flat | K `filterSees`（描述的 `scope`、`row` 来自它） | 部分 | #639 ※ | "总是七个参数"只有普通测试 |
| EVP-3-6 | §三 | 描述与报告信息用配置的行 id 称呼行（同组各实例的副本 id 相同），协议消息仍用行键；没有 id 时用行键 | 纯逻辑 | K | events/one-bus | K `filter/keeps-and-sees`（`filter:<row>`、`filterSees`）、各用例的 `reported`（`in row <label>`） | 已覆盖 | — |  |
| EVP-3-7 | §三 | 经 `mount` 的 `plugins` 列表加载的插件得到 `mount` 参数 `scope`（挂载它的插件的链） | 纯逻辑 | K | — | — | 未覆盖 | #639 ※ | 套件不经 `mount` 的 `plugins` 列表加载参与者 |
| EVP-3-8 | §三、§15.28 | `mount` 带 `runtime`（描述里的运行时名）；运行时在快速路径上给自己的条目写的描述与经宿主时相同 | 纯逻辑 | K | events/one-bus | K `filterSees`（按放置核对 `runtime`，快速路径与经宿主两种放置都有） | 已覆盖 | — |  |
| EVP-3-9a | §三 | 运行时的 `events.dispatch` 写明 `row`，宿主用这一行的链作发出方作用域，从不采信自报 | — | — | events/hub-scope（已撤回） | — | 已取代 | — | 0.15.1 平总线（#604、#605）后发出方作用域不参与送达，也不进过滤的输入，无可观察效果；0.16 删 `Dispatch::scope`（`docs/migration-0.16.md`）；规格这半句仍在 |
| EVP-3-9b | §三 | 发出的行没有加载在这个会话上时宿主拒绝（`BindingError`，`row <key> is not loaded in this runtime`）；挂载方式的插件写 `row: null` | 跨进程 | TV + 测试 | events/hub-flat | — | 未覆盖 | #616 | `hub.rs:639`；各运行时也在本地先拒绝（`events.mjs:137`、`runner.py:608`、`events.go:401`）；套件无用例 |
| EVP-3-10 | §三 | 叶子插件的 `isolate` 标签（服务作用域、U+0001 租约）与事件作用域无关 | 纯逻辑 | K | — | — | 未覆盖 | #639 ※ | 写法接近说明；可按 Q5.1.1 降为说明，或补一个 isolate 行的 `filterSees` 用例 |
| EVP-3-S1 | §三（2026-10-18 原文） | 作用域链为 L 的条目收到在链 E 发出的分发，当且仅当 L 是 E 的前缀；根收到全部，租户 t1 收不到 t2 的 | — | — | events/scope、events/hub-scope（已撤回） | — | 已取代 | — | 0.15.1 平总线（#604、#605；1f0a7b3b） |
| EVP-4.1-1 | §4.1 | 事件消息都是 `target` 为空的 `invoke` 帧（控制操作），属于 `events` 族 | 纯逻辑 | K | events/hub-dispatch | K（全部经宿主的用例都经这些帧） | 已覆盖 | — | FZ `session_frame` 只到帧解析与值，不解释 `events.*` |
| EVP-4.1-2 | §4.1 | 运行时一侧就地、按帧的顺序处理 `events.on` / `events.off` / `events.elsewhere`，处理完再读下一帧 | 跨进程 | TLA + SH | events/presence | — | 未覆盖 | #613、#630 | Python 把 invoke 帧（含 `events.elsewhere`、`events.call`）经 `loop.call_soon` 推迟（`peer.py:1375`），与本条不符：#613 配置 `python-deferred`、#630 的 Python 可控事件循环 |
| EVP-4.1-3 | §4.1、§15.30 | 宿主一侧按帧序在会话的有序任务上处理事件操作（表操作与 emit 方式的 `events.dispatch` / `events.call` 轮到时就地执行，其余轮到时作为任务开始），不在读线程上；读线程从不执行插件代码 | 进程内并发 | SH + TV | events/hub-dispatch、events/hub-lifecycle | — | 未覆盖 | #616、#628 | `rpc.rs:2382–2500`（`run_ordered` / `drain_ordered`）；与 §4.1 "处理完再读下一帧"字面不符（宿主读线程先读下一帧） |
| EVP-4.1-4 | §4.1 | 每条事件消息以 `return undefined` 或 `throw` 回复 | 纯逻辑 | K | events/hub-dispatch | K（全部用例） | 已覆盖 | — |  |
| EVP-4.1-5 | §4.1 | `events.on` 在条目已进表、且它引起的 `events.elsewhere` 已排进其他每个会话之后才回复 | 跨进程 | TLA + TV | events/presence | — | 未覆盖 | #613、#616 | `RegisteredThenSeen` 依赖它（§4.4 第 3 条） |
| EVP-4.1-6 | §4.1 | `events.off` 在条目已出表之后回复；未知的 id 忽略 | 跨进程 | TLA + TV | events/hub-lifecycle | — | 未覆盖 | #613、#616 | `WithdrawnStaysOut` |
| EVP-4.1-7 | §4.1 | `events.elsewhere` 参数 `{add, remove, addPrefixes, removePrefixes}`（缺少的即 `[]`），应用之后回复 | 纯逻辑 | K | events/presence | K `fast-path/local-only`、`fast-path/elsewhere-follows`（间接） | 部分 | #639 ※ | 字段缺省、前缀字段（只有 Rust 前缀条目产生）无用例 |
| EVP-4.2-1 | §4.2 | 登记的 `id` 由运行时选定，会话存续期间唯一、从不复用 | 纯逻辑 | SH + 测试 | — | — | 未覆盖 | #628 | 宿主拒绝重复 id（`listener <id> is already registered`，`hub.rs`）不在规格错误表里；#628 的 `SessionEvents`"重复 id 被拒" |
| EVP-4.2-2 | §4.2 | 名为 `internal/*` 的登记被拒（`BindingError`，`event <name> is internal to its runtime`） | 纯逻辑 | K | events/hub-reserved | — | 未覆盖 | #639 ※ | 只有 `event_hub.rs::reserved_and_internal_names_are_refused`（普通测试） |
| EVP-4.2-3 | §4.2 | `rutis/*` 可以监听 | 跨进程 | K | events/reserved | K `rutis/row`、`rutis/service`、`rutis/runtime` | 已覆盖 | — |  |
| EVP-4.2-4 | §4.2 | `row` 为登记它的行的键（挂载方式为 `null`）；不是这个会话上加载的行时宿主拒绝 | 跨进程 | TV | events/hub-flat | — | 未覆盖 | #616 | `hub.rs:657`；`hub.rs` 持 `rows` 锁直到条目记下，使并发的卸载也撤下它（`a_row_unloading_while_a_listener_registers_withdraws_it`） |
| EVP-4.2-5 | §4.2 | `plugin` 为登记它的插件名（可为这一行的子插件；叶子插件的名字） | 纯逻辑 | K | events/one-bus | — | 未覆盖 | #639 ※ | `filterSees` 只核对 `row`、`runtime`、`node`、`scope`，不核对 `plugin` |
| EVP-4.2-6 | §4.2 | `kind`：`sync`（已知同步，Go 的每个监听器）/ `async`（`AsyncFunction`、`async def`）/ `any`（其余 JS 与 Python 函数） | 纯逻辑 | K | events/hub-bail | K `bail/async-listener`、`bail/awaitable-from-plain-function`、`bail/plain-function-plain-value`、`kinds/sync-listener-every-mode` | 已覆盖 | — | Swift、Kotlin 的监听器也是 `sync`（套件 `except` 写明），规格只写了 Go |
| EVP-4.2-7 | §4.2 | `prepend` 为监听器选项（所有语言；Cordis 用自己的） | 跨进程 | K | events/order | K `order/prepend`、`registration/unloaded-row-silent` | 已覆盖 | — |  |
| EVP-4.2-8 | §4.2 | `global` 为监听器选项：条目不受自定义过滤，作用域只是描述 | 跨进程 | K | events/filter | K `filter/global-skips-filter`、`filter/global-skips-filter-in-scopes` | 已覆盖 | — |  |
| EVP-4.2-9 | §4.2 | 运行时从不登记前缀；Rust 的模式监听器是宿主上的前缀条目（`prefix: true`，`name` 为前缀） | 纯逻辑 | PT + TV | events/order | — | 未覆盖 | #616、#631 | 套件无前缀用例；#613 的配置不含前缀（只在 `EventsHost` 定义） |
| EVP-4.2-10 | §4.2 | 描述字段：`name`、`prefix`、`plugin`、`row`（行 id）、`runtime`（宿主 Rust 插件为 `null`）、`node`（引进条目的对端 id）、`scope`（宿主按行设定；引进的为连接的作用域）、`global` | 纯逻辑 | K | events/one-bus | K `filterSees`（`row`、`runtime`、`node`、`scope`） | 部分 | #639 ※ | `name`、`prefix`、`plugin`、`global` 字段未核对 |
| EVP-4.2-S1 | §4.2（2026-10-18 原文） | `global` 条目不受过滤，但仍受作用域规则 | — | — | — | — | 已取代 | — | 0.15.1 平总线（1f0a7b3b） |
| EVP-4.3-1 | §4.3 | 宿主按接受先后给追加的条目递增位置（`events.on` 按到达、Rust 在登记时、节点引进在收到时） | 跨进程 | TLA + TV | events/order、events/one-bus | K `order/registration` | 部分 | #613 | `OneOrder`（Table） |
| EVP-4.3-2 | §4.3 | `prepend` 的条目取比现有全部位置都小的位置 | 纯逻辑 | PT + TV | events/order | K `order/prepend`、`registration/unloaded-row-silent` | 部分 | #616、#631 | #613 未写明配置含 `prepend`；#631 监听器表 PT 含 prepend |
| EVP-4.3-3 | §4.3 | 一次分发取匹配的条目（同名，或名字以其前缀开头的前缀条目），按位置排序（登记顺序） | 纯逻辑 | PT + TV | events/order | K `order/registration`（只有确切名字） | 部分 | #616、#631 | 前缀匹配无一致性用例 |
| EVP-4.3-4 | §4.3 | 一个进程自己的条目在本进程里的相对顺序与宿主上相同（帧按序处理；对自己的条目同样应用 `prepend`） | 跨进程 | TLA + K | events/one-bus | K `order/prepend`、`order/registration`（快速路径放置） | 部分 | #613 | `OneOrder` 要求两条路径的先后一致 |
| EVP-4.3-5 | §4.3 | `rows.update` 让这一行的监听器重新登记，得到新位置 | 跨进程 | K | events/order | K `order/update-registers-again` | 已覆盖 | — | #613 不建 `rows.update`（等价于撤下再登记） |
| EVP-4.4-1 | §4.4 第 1 条 | 同一进程按程序顺序先登记、后发出，这次分发包含这个监听器，快速路径和经宿主都如此 | 跨进程 | TLA + K | events/presence、events/hub-dispatch | K `fast-path/local-only`、`registration/loaded-row-hears`（构造的时序） | 部分 | #613 | `RegisteredThenSeen` |
| EVP-4.4-2 | §4.4 第 2 条 | 运行时在一行 `apply` 期间发出的每个 `events.on` 都得到回复之后才回复 `rows.load`；宿主报告为 Running 的行，监听器都已在总线上 | 跨进程 | TLA + K | （登记表无对应条目） | K `registration/loaded-row-hears` | 部分 | #613 | `RunningRowHeard`；#613 计划补登记 |
| EVP-4.4-3 | §4.4 第 3 条 | 宿主在回复 `events.on` 之前把由此引起的 `events.elsewhere` 排进其他每个会话；因果上在这个回复之后的分发都包含这个条目 | 跨进程 | TLA + TV | events/presence | — | 未覆盖 | #613、#616 | `RegisteredThenSeen`（含跨进程调用） |
| EVP-4.4-4 | §4.4 第 4 条 | `events.off` 得到回复后（或宿主撤下这一行）这些条目不再开始新的调用；已经发出的调用失败一次 | 跨进程 | TLA + K | events/hub-lifecycle、events/lifecycle | K `unload/not-called-after`、`registration/unloaded-row-silent` | 部分 | #613、#630 | `WithdrawnStaysOut`、`SettlesOnce`；#630 的 Go、Kotlin 快速路径"撤下之后不再开始" |
| EVP-4.4-5 | §4.4 第 5 条 | 宿主里的 Rust 登记在 `on` 返回时已进表，相应的 `events.elsewhere` 已排好 | 进程内并发 | SH + TV | events/presence、events/remote | — | 未覆盖 | #616、#622 | #622"监听器表的观察者"：`deliver(seq)` 在 seq 被听到后才返回；#613 未写明配置含宿主 Rust 登记 |
| EVP-4.5-1 | §4.5 | 宿主为每个运行时会话 R 维护 elsewhere(R)：至少有一个条目不在 R 里的全部名字和前缀（Rust 插件、其他运行时、节点引进），不考虑作用域 | 进程内并发 | TLA + SH + TV | events/presence | K `fast-path/local-only`、`fast-path/elsewhere-follows` | 部分 | #613、#616、#628 | #628 presence 计数等于表；疑点：`Hub::of` 重放时饱和丢 Removed，留下幽灵的"别处有"条目 |
| EVP-4.5-2 | §4.5 | 握手之后、`mount` 之前先把当前全集作为 `add` 发出，之后只发差异，按顺序 | 跨进程 | TLA + TV | events/presence | — | 未覆盖 | #613、#616 | EventsTable 的"会话重新开始"配置 |
| EVP-4.5-3 | §4.5 | R 的快速路径判断用 elsewhere(R)；R 的会话结束后 R 把 elsewhere(R) 当作空 | 跨进程 | TLA | events/presence | — | 未覆盖 | #613 |  |
| EVP-4.5-4 | §4.5 | 名字以 `rutis::` 开头的内核诊断事件的条目从不计入 elsewhere，也不经节点连接通告 | 纯逻辑 | PT + TV | — | — | 未覆盖 | #631 ※ | `hub.rs:186` `process_local`；无用例；#613/#616 未提这类名字 |
| EVP-5.1-1 | §5.1 第 1 步 | 快照：与名字匹配、按登记顺序、存活（未撤下）的条目 | 跨进程 | TLA + K | events/one-table、events/order | K `order/registration`、`unload/not-called-after` | 部分 | #613 | `RegisteredThenSeen`、`WithdrawnStaysOut` |
| EVP-5.1-2 | §5.1 第 2 步 | 带过滤时，对快照里每个非 `global` 条目按顺序各求一次，在任何调用开始之前；被拒的去掉 | 纯逻辑 | K + PT | events/filter、events/hub-filter | K `filter/keeps-and-sees`、`filter/every-listener`、`filter/bail`、`filter/global-skips-filter` | 已覆盖 | — | `log` 为确切顺序，过滤被调两次或晚于调用都会失败 |
| EVP-5.1-3 | §5.1 第 3 步 | 每次开始一个调用之前检查条目仍存活；已撤下的跳过，如同从未在快照里 | 跨进程 | TLA + TV | events/hub-lifecycle | — | 未覆盖 | #613、#616 | `WithdrawnStaysOut`；套件无"快照之后、开始之前撤下"的用例 |
| EVP-5.1-S1 | §5.1（2026-10-18 原文） | 快照只取作用域链是发出方链前缀的条目 | — | — | — | — | 已取代 | — | 0.15.1 平总线（1f0a7b3b） |
| EVP-5.2-1 | §5.2 | R 发出 N：N 不在 elsewhere(R)、也没有是 N 前缀的前缀，且（只对 `emit`）没有更早经宿主、未得回复的 N 的 `emit` 时，R 自己分发，不给宿主发消息 | 跨进程 | TLA + K | events/one-bus | K `fast-path/local-only`、`fast-path/elsewhere-follows`、`emit/order-across-fast-path` | 部分 | #613 | 前缀分支无用例 |
| EVP-5.2-2 | §5.2 | 否则 R 发 `events.dispatch`，宿主在整张表上分发，R 自己的条目经 `events.call` 回调到 R | 跨进程 | TLA + K | events/hub-dispatch | K（`viaHost: true` 的用例；X、Y 都有监听器的放置） | 部分 | #613 |  |
| EVP-5.2-3 | §5.2 | Rust 宿主总是自己在整张表上分发 | 进程内并发 | K | events/hub-dispatch | K（发出方 `e` 在 `host` 的放置） | 已覆盖 | — |  |
| EVP-5.2-4 | §5.2 | 两种路径结果相同，只是代码在哪里执行不同 | 跨进程 | TLA + K | events/one-bus | K（`vary` 的全部放置） | 部分 | #613 | 并发时不成立（#613 `SeesAll` 预期反例），需写明不保证 |
| EVP-5.3-1 | §5.3 | `events.dispatch` 为 `target` 为空的 `invoke`；`bail` 的分发是同步调用，按 `sync-wait` 规则带 `sync: true` | 跨进程 | K + TLA | events/hub-bail | K `bail/first-result`、`bail/wait-cycle` | 部分 | #614 |  |
| EVP-5.3-2 | §5.3 | `events.dispatch` 参数 `{row, name, mode, args, filter?, terminal?}`：`mode` 为五种之一，`args` 为值的 list，`filter`、`terminal`（仅 waterfall）为可选函数引用 | 纯逻辑 | K + FZ | events/hub-dispatch | K（全部经宿主的用例） | 部分 | #639 ※ | 未知方式的拒绝（`hub.rs` `events.dispatch: unknown mode`）不在规格里、无形式化；FZ `session_frame` 不解释事件参数 |
| EVP-5.3-3 | §5.3 | `rutis/*` 在发送之前就被拒绝 | 纯逻辑 | K | events/reserved、events/hub-reserved | K `reserved/rutis-emit`、`reserved/every-mode` | 已覆盖 | — |  |
| EVP-5.3-4 | §5.3 | `events.call`（宿主→运行时，或经连接节点→节点）为 `target` 为空的 `invoke`；`bail` 与 `waterfall-sync` 为同步调用（`sync: true`） | 跨进程 | K + TLA | events/hub-bail | K `bail/first-result`、`waterfall-sync/chain` | 部分 | #614 |  |
| EVP-5.3-5 | §5.3 | `listener` 为接收方的条目 id（它自己 `events.on` 的 id；经连接为它通告的 id） | 纯逻辑 | K | events/hub-dispatch | K（全部经宿主与 node 位置的用例） | 已覆盖 | — |  |
| EVP-5.3-6 | §5.3 | `dispatch` 为宿主选定的 id，在宿主内唯一；同一次分发的每个调用相同 | 纯逻辑 | K + TV | — | K `cordis/internal-dispatch`（只一个监听器） | 部分 | #639 ※ | "每个调用相同"无多监听器用例；#616 的日志是否记 dispatch id 未写明 |
| EVP-5.3-7 | §5.3 | `own`：这次分发来自同一会话的 `events.dispatch` 时为 `true` | 纯逻辑 | K | events/hub-dispatch | K `cordis/internal-dispatch`（`own: false` 一侧） | 部分 | #639 ※ | `own: true` 时不再触发 `internal/dispatch` 无用例 |
| EVP-5.3-8 | §5.3 | `mode` 为六种之一；只有 Rust 发出 `waterfall-sync` | 纯逻辑 | K | events/hub-dispatch | K `waterfall-sync/chain`、`waterfall-sync/async-listener` | 已覆盖 | — |  |
| EVP-5.3-9 | §5.3 | `next` 仅在 waterfall / waterfall-sync：不带参数的函数引用，返回链上其余部分的结果 | 跨进程 | K | events/waterfall | K `waterfall/chain`、`waterfall/veto`、`waterfall-sync/chain` | 已覆盖 | — |  |
| EVP-5.3-10 | §5.3 | `signal` 为现有的 `signal` 值：宿主取消这次调用时它被中止 | 跨进程 | K | — | K `cancel/emitter-gives-up` | 已覆盖 | — | JS 监听器按 §5.7 排除 |
| EVP-5.3-11 | §5.3 | 接收方以事件参数调用监听器，waterfall / waterfall-sync 时把 `next` 追加为最后一个参数 | 跨进程 | K | events/waterfall | K `waterfall/*`、`serial/argument-reaches` | 已覆盖 | — |  |
| EVP-5.3-12 | §5.3 | 接收方原样回复监听器的返回值（有没有结果由分发方判断），或以 `throw` 回复监听器的错误 | 跨进程 | K | events/serial-bail | K `serial/first-result`、`serial/zero-is-a-result`、`waterfall/false-stays-false`、`serial/error-stops` | 已覆盖 | — |  |
| EVP-5.3-13 | §5.3 | `emit` 方式下监听器结束后无论如何回复 `undefined`；失败在它这一侧报告 | 跨进程 | K | events/emit | K `emit/error-isolated`、`emit/error-keeps-process` | 已覆盖 | — | 疑点：Kotlin 的 emit 方式 `events.call` 不理 `cancel` / `signal`（#630） |
| EVP-5.3-14 | §5.3 | 对未知或已撤下的 id 的 `events.call` 以 `throw` `BindingError`（`listener <id> of <name> is withdrawn`）回复 | 跨进程 | TLA + K | events/hub-lifecycle | — | 未覆盖 | #613 | `WithdrawnStaysOut`（"`events.call` 以已撤下回复"）；信息文字只有普通测试，套件无竞态用例 |
| EVP-5.3-15 | §5.3 | Node 的 Cordis：`own: false` 的分发第一次调用本进程监听器之前，对这个 `dispatch` id 发一次 `internal/dispatch`（方式、名字、参数、`null`） | 进程内并发 | K | — | K `cordis/internal-dispatch` | 部分 | #639 ※ | 只测一个监听器；Node 只记最近 4096 个 dispatch id（`events.mjs:68` `SEEN`），更早且仍在进行的分发会再触发；`waterfall-sync` 以 `waterfall` 报（`events.mjs:521`） |
| EVP-5.3-16 | §5.3 | 本进程自己发出的事件经 Cordis 自己的 `dispatch()` 触发 `internal/dispatch`；它抛出时这次调用失败 | 进程内并发 | K | — | — | 未覆盖 | #639 ※ |  |
| EVP-5.3-17 | §5.3 | Cordis `events.dispatch(type, args)` 原语：名字非 `internal/*`、`type` 为 `emit`/`serial`/`bail` 时返回回调，以调用时的参数在总线上按 `parallel`/`serial`/`bail` 分发（本进程或经宿主） | 跨进程 | K | — | — | 未覆盖 | #639 ※ | 只有 Node 的普通测试 |
| EVP-5.4-1 | §5.4 emit | `emit`：按登记顺序开始每个调用，不等任何一个 | 跨进程 | K + TLA | events/emit | K `emit/reaches-all`、`emit/does-not-wait` | 部分 | #613 | 跨进程的开始先后由 `OneOrder` 管 |
| EVP-5.4-2 | §5.4 emit | `emit`：每个调用都开始之后才回复 `events.dispatch`；发出方的 API 早已返回（`void`/`None`/`nil`/`Ok(())`） | 跨进程 | TLA + TV + K | events/emit、events/remote | K `emit/does-not-wait`、`modes/no-listeners` | 部分 | #613、#616 | `before-future-reply` 配置 |
| EVP-5.4-3 | §5.4 parallel | `parallel`：按登记顺序开始每个调用，再等全部完成；全部成功时 `undefined` | 跨进程 | K + PT | events/parallel | K `parallel/all-ok`、`kinds/sync-listener-every-mode`、`modes/no-listeners` | 已覆盖 | — |  |
| EVP-5.4-4 | §5.4 parallel | `parallel` 有失败时 `throw` `AggregateError`，子错误为各失败、按登记顺序（非完成顺序），只有一个失败也如此 | 跨进程 | K + PT | events/parallel、events/hub-errors | K `parallel/errors-in-registration-order`、`parallel/one-error-is-aggregated`、`disconnect/parallel-aggregates` | 已覆盖 | — |  |
| EVP-5.4-5 | §5.4 serial | `serial`：一次开始一个、逐个等待；返回第一个有结果的值；都没有结果时 `undefined` | 跨进程 | K + PT | events/serial-bail | K `serial/first-result`、`serial/no-result`、`serial/zero-is-a-result` | 已覆盖 | — |  |
| EVP-5.4-6 | §5.4 serial | `serial`：监听器的错误原样抛出并停止其余 | 跨进程 | K + PT | events/serial-bail | K `serial/error-stops` | 已覆盖 | — |  |
| EVP-5.4-7 | §5.4 waterfall | `waterfall`：带 `next` 调第一个条目；条目 i 的 `next` 调 i+1（跳过已撤下的）；最后一个的 `next` 以事件参数调终态，没有终态得到 `undefined` | 跨进程 | K + PT | events/waterfall | K `waterfall/chain`、`waterfall/terminal`、`waterfall/terminal-gets-args`、`waterfall/end-without-terminal` | 部分 | #616 | "跳过已撤下的"（`next` 时下一个条目已撤下）无用例；`EventsHost` 定义 waterfall 等 `next`，由 #616 校验宿主 |
| EVP-5.4-8 | §5.4 waterfall | `waterfall` 的结果是最外层监听器的返回值，原样（不归一，`false` 仍是 `false`） | 跨进程 | K | events/waterfall | K `waterfall/false-stays-false`、`waterfall/chain` | 已覆盖 | — |  |
| EVP-5.4-9 | §5.4 waterfall | `waterfall` 没有条目时得到终态的值，或 `undefined` | 纯逻辑 | K | events/waterfall | K `waterfall/no-listeners`、`waterfall/terminal-gets-args-no-listeners` | 已覆盖 | — |  |
| EVP-5.4-10 | §5.4 waterfall | 不调 `next` 的监听器否决其余 | 跨进程 | K | events/waterfall | K `waterfall/veto` | 已覆盖 | — |  |
| EVP-5.4-11 | §5.4 waterfall | `waterfall` 中监听器的错误抛到发出方 | 跨进程 | K | events/waterfall | K `waterfall/error` | 已覆盖 | — | 表里没写明，设计 §三.3 写了 |
| EVP-5.4-12 | §5.4 bail | `bail`：同 `serial`，每个调用都是同步的（`sync: true`），结果同步 | 跨进程 | K + TLA | events/hub-bail、events/serial-bail | K `bail/first-result`、`bail/no-result`、`bail/error-stops`、`filter/bail` | 部分 | #614 | 同步语义与等待图在 #614 |
| EVP-5.4-13 | §5.4 waterfall-sync | `waterfall-sync`：同 `waterfall`，同步，`next` 同步调用；只有 Rust 发出方 | 跨进程 | K + TLA | events/waterfall | K `waterfall-sync/chain`、`waterfall/next-twice-in-sync` | 部分 | #614 |  |
| EVP-5.4-14 | §5.4 next | `next` 不带参数；第二次调用、在它的分发结束或被取消之后调用，以 `WaterfallNext` 失败 | 跨进程 | K | events/one-table、events/remote | K `waterfall/next-twice`、`waterfall/next-twice-in-sync` | 部分 | #639 ※ | "结束之后""取消之后"无用例；#613 不建 next 单次规则 |
| EVP-5.4-15 | §5.4 next | `waterfall` 中接收方给监听器的 `next` 返回 Promise / 可等待对象 / Go 的阻塞调用；`waterfall-sync` 中同步调用这个引用 | 跨进程 | K | events/waterfall | K `waterfall/chain`（各语言 Y）、`waterfall-sync/chain` | 已覆盖 | — |  |
| EVP-5.4-16 | §5.4 种类 | `bail` / `waterfall-sync` 中 `async` 种类的条目不被调用，分发以 `AsyncListener` 失败 | 跨进程 | K | events/serial-bail、events/hub-bail | K `bail/async-listener`、`waterfall-sync/async-listener`、`rust/bail-async-listener` | 已覆盖 | — |  |
| EVP-5.4-17 | §5.4 种类 | `any` 种类的条目被调用；返回 Promise / thenable / 可等待对象时运行时丢弃它（Python 协程关闭；Promise 的拒绝被捕获忽略），这次调用以 `AsyncListener` 失败 | 跨进程 | K | events/hub-bail | K `bail/awaitable-from-plain-function`、`bail/plain-function-plain-value` | 部分 | #639 ※ | 只查 `AsyncListener`；"协程被关闭、拒绝被忽略"无检查 |
| EVP-5.4-18 | §5.4 种类 | Go 的监听器是 `sync`；Rust 的 `on` 条目是 `async`，`on_sync` 的是 `sync` | 纯逻辑 | K | events/one-table | K `rust/bail-async-listener`、`kinds/sync-listener-every-mode` | 已覆盖 | — | Swift / Kotlin 规格未写 |
| EVP-5.4-19 | §5.4 种类 | 每个同步调用都像同步服务调用一样参加等待环检测（`SyncWaitCycle`） | 跨进程 | TLA + K | events/serial-bail、bridge/sync-wait-cycle-refused | K `bail/wait-cycle`（只 Node×Node）、`bail/call-back-into-waiting-emitter` | 部分 | #614 | `cross-runtime-sync` 现在没有事件的同步方式（只有 emit 投递 `MaxPosts`） |
| EVP-5.5-1 | §5.5 | 对每个（发出的进程，名字），每个监听器上 `emit` 的调用按发出顺序开始 | 跨进程 | TLA + K + TV | events/emit、events/one-bus | K `emit/order-per-listener`、`emit/order-filtered-then-unfiltered`、`emit/order-across-fast-path`、`emit/order-across-fast-path-sync-wait` | 部分 | #613、#616 | `EmitOrder`；"同一进程并发的发出没有先后"规格未写（#613 要求补） |
| EVP-5.5-2 | §5.5 第 1 条 | 宿主按顺序处理一个会话的 `events.dispatch` 帧，先开始同一进程同名前一次 emit 的调用，再开始后一次的 | 跨进程 | TLA + TV + SH | events/emit | K `emit/order-per-listener` | 部分 | #613、#616、#628 |  |
| EVP-5.5-3 | §5.5 第 1 条 | 宿主自己的 Rust 监听器保留内核按键排队的 emit 链 | 进程内并发 | SH | events/emit | — | 未覆盖 | #622 | #622 的 emit 链模型（每个发出方的开始顺序） |
| EVP-5.5-4 | §5.5 第 2 条 | 宿主上一个在等过滤器的 `emit` 把这个会话之后的 `emit` 分发排在队列里，每项记着发出它的行 | 进程内并发 | TLA + SH + TV | events/emit | K `emit/order-filtered-then-unfiltered` | 部分 | #613、#616、#628 | `hub.rs` `run_emit` |
| EVP-5.5-5 | §5.5 第 2 条 | 行卸载时宿主丢弃它还在排队的 emit；正在跑且属于它的被放开：过滤器算作一个都不保留（运行时之后回答也不开始、不报告），调用被丢弃 | 跨进程 | TLA + TV + SH | （登记表无对应条目） | — | 未覆盖 | #613、#616、#628 | `UnloadedRowEmitsNothing`、`before-unload-queue`；`hub.rs:531` 与规格一致 |
| EVP-5.5-6 | §5.5 第 2 条 | 被丢弃或放开的 emit 的 `events.dispatch` 照样得到回复；会话的其他 emit（别的行或挂载本身的）按顺序继续 | 跨进程 | TLA + TV | — | — | 未覆盖 | #613、#616 | "不卡住" |
| EVP-5.5-7 | §5.5 第 2 条 | 会话结束时它的所有 emit 都这样处理 | 跨进程 | TLA + SH | events/hub-lifecycle | — | 未覆盖 | #613、#628 | `hub.rs` `leave()` |
| EVP-5.5-8 | §5.5 第 2 条 | 运行中 panic 的 emit 像正常结束一样收尾：下一个照常开始 | 进程内并发 | SH | — | — | 未覆盖 | #628 | `hub.rs` `EmitRun`；#628 宿主 emit 队列"panic 后下一个照常" |
| EVP-5.5-9 | §5.5 第 3 条 | 运行时按到达顺序开始 `emit` 方式的调用：Node / Bun / Python 在处理下一帧之前调用监听器（同步执行到第一个 `await`） | 跨进程 | TLA + SH | events/emit | K `emit/order-per-listener`、`emit/order-across-fast-path` | 部分 | #613、#630 | Python 现在经 `loop.call_soon` 推迟（`peer.py:1375`），与本条不符；`runner.py:754` 的注释称已做到；#613 `python-deferred` |
| EVP-5.5-10 | §5.5 第 3 条 | 不可重入的 Node 在自己的同步调用等待期间，扣住 `emit` 方式的 `events.call`（同其他不在等待链上的调用），等待结束后按帧序执行 | 跨进程 | TLA + SH | events/emit | K `emit/order-across-fast-path-sync-wait`；TLA `cross-runtime-sync` `policy-b-posts-node`（只查放到一边不卡死，不查顺序） | 部分 | #613、#630 | R1 |
| EVP-5.5-11 | §5.5 第 3 条 | 扣住期间本进程快速路径上发出的同名 emit 排在被扣住的调用之后，不抢先（2026-10-21 修订） | 跨进程 | TLA + SH | events/emit | K `emit/order-across-fast-path-sync-wait` | 部分 | #613、#630 | `before-held-emit`、`before-later-overtakes`；疑点：Node 的 behind 任务在发出行卸载时不丢弃（#613） |
| EVP-5.5-12 | §5.5 第 3 条 | Go 按监听器排队 emit 调用，一个接一个执行（不同监听器并发） | 进程内并发 | TLA + SH | events/emit | K `emit/order-per-listener`（Go 列） | 部分 | #613 | R2b（Go、Swift、Kotlin）的 EventsEmit 配置；#630 的 Go 闸门只列了撤下之后不再开始，不含按监听器排队 |
| EVP-5.5-13 | §5.5 第 4 条 | `emit` 的调用在过滤器决定之后才开始（过滤器可能要等待，可以是异步的） | 跨进程 | TLA + K | events/filter | K `emit/order-filtered-then-unfiltered` | 部分 | #613 | 过滤器"迟到 / 永不回答"；Node / Bun / Python 快速路径上返回 Promise / 协程的谓词按真值保留（`events.mjs:351`、`events.py:280`），并不等待 |
| EVP-5.5-14 | §5.5 第 4 条 | 快速路径上 N 的 emit 等过滤器时，本进程之后 N 的 emit（无论有无过滤器、来自哪一行）排在它后面，轮到时再决定走哪条路径 | 进程内并发 | TLA + SH | events/emit | K `emit/order-filtered-then-unfiltered` | 部分 | #613、#630 | EventsEmit 的运行时本地队列；疑点：Bun 的过滤器在同步调用里泵帧，嵌套的较晚 emit 先开始 |
| EVP-5.5-15 | §5.5 第 4 条 | 运行时排队的 emit 属于发出它的行：行卸载时丢弃它排队的 emit，放开正在跑过滤器的那一个（之后过滤器回答也什么都不开始、不报告） | 进程内并发 | TLA + SH | （登记表无对应条目） | — | 未覆盖 | #613 | `UnloadedRowEmitsNothing`（两侧）；81e30688 修了 Swift、Go |
| EVP-5.5-16 | §5.5 第 5 条 | 运行时经宿主发出 N 的一次 emit 后，在它得到回复之前，N 的后续 emit 都继续经宿主 | 跨进程 | TLA + K | events/one-bus | K `emit/order-across-fast-path` | 部分 | #613 | `EmitOrder` |
| EVP-5.5-17 | §5.5 第 5 条 | 宿主在开始各调用之后才回复，所以这次 emit 的 `events.call` 先于任何后来的 N 的快速路径 emit 到达运行时 | 跨进程 | TLA + TV | events/emit | K `emit/order-across-fast-path` | 部分 | #613、#616 | `before-future-reply` |
| EVP-5.5-18 | §5.5 第 6 条、§15.29 | emit 的回复是普通值 `undefined`，不是 future，在各调用开始之后写出 | 跨进程 | TV + TLA | events/emit | — | 未覆盖 | #613、#616 | `rpc.rs` `execute`：emit 的 future 先 `settle` 再回复，与规格一致；c2c1b7e8 之前宿主答 future |
| EVP-5.5-19 | §5.5 第 6 条 | 运行时像 `events.elsewhere` 一样就地、按帧序接收 emit 的回复：读到回复帧时即记为已得到回复，处理完再读下一帧 | 跨进程 | TLA + SH | events/one-bus | K `emit/order-across-fast-path`（`answered`） | 部分 | #613、#630 | `FastPathByFrame`、`before-reply-async` |
| EVP-5.5-20 | §5.5 第 6 条 | 宿主在发出某一帧之前已回复的 dispatch，不再让 N 留在经宿主的路径上：之后帧引起的操作都看到它已得到回复 | 跨进程 | TLA + K | events/one-bus | K `emit/order-across-fast-path`、`emit/order-across-fast-path-sync-wait` | 部分 | #613 | `FastPathByFrame` |
| EVP-5.6-1 | §5.6 | `emit` 的监听器失败不影响发出方、不影响其他监听器，也从不结束进程 | 跨进程 | K | events/emit | K `emit/error-isolated`、`emit/error-keeps-process` | 已覆盖 | — |  |
| EVP-5.6-2 | §5.6 | 失败在监听器所在的一侧报告恰好一次，报告指明事件名、行与错误名 | 跨进程 | K | events/emit | K 各用例的 `reported`（恰好一行，`in row <label>`、错误名） | 已覆盖 | — |  |
| EVP-5.6-3 | §5.6 | 运行时报告的文字确切为 `rutis: listener of <name> in row <row> failed: <ErrorName>: <message>` | 外部 | 测试 | — | — | 不适用 | — | 输出文字；`reported` 只核对行与错误名 |
| EVP-5.6-4 | §5.6 | 宿主：失败进它的错误汇集（rutis-host 打印同样的文字） | 跨进程 | K + 测试 | events/emit | K `reported`（host 位置，经 `Tap` 钩子） | 已覆盖 | — | rutis-host 打印的文字由普通测试 |
| EVP-5.6-5 | §5.6 | 无法送达或得不到回复的调用（传输、值能力、已撤下）由分发方报告：`rutis: listener of <name> in <runtime <r>\|node <p>> failed: …` | 跨进程 | K | events/hub-lifecycle | K `disconnect/emit-reported` | 部分 | #639 ※ | 值能力、已撤下两种原因无用例 |
| EVP-5.6-6 | §5.6 | `emit` 中过滤函数失败时不送达任何监听器，由分发方报告（`rutis: filter of <name> from row <row> failed: …`） | 跨进程 | K | events/filter | K `filter/throws-in-emit` | 已覆盖 | — |  |
| EVP-5.6-7 | §5.6 | 挂载方式加载的插件（`row: null`）发出时，`from row <row>` 写作 `from the mount`（§11 的 emit 行同） | 外部 | 测试 | — | — | 不适用 | — | 输出文字；Go、Swift、Kotlin 没有挂载方式的插件，总写 `from row` |
| EVP-5.7-1 | §5.7 | 发出方放弃（Rust future 丢弃、Python 任务取消、Go context 取消）时，经宿主的运行时对它的 `events.dispatch` 发 `cancel` | 跨进程 | K + TLA | — | K `cancel/emitter-gives-up` | 部分 | #626 | #626 `CancelReaches`（通用调用的取消）；Swift / Kotlin 规格未写 |
| EVP-5.7-2 | §5.7 | 分发方不再开始新的调用，对每个进行中的调用发 `cancel`（其 `signal` 中止），之后的 `next` 调用以 `WaterfallNext` 失败 | 跨进程 | TV + K | — | K `cancel/emitter-gives-up` | 部分 | #616 | 取消之后调 `next` 无用例 |
| EVP-5.7-3 | §5.7 | 发出方的调用以本语言的取消结束 | 跨进程 | K | — | K `cancel/emitter-gives-up` | 已覆盖 | — | 期望 `cancelled` 按语言核对 |
| EVP-5.7-4 | §5.7 | `emit` 分发不被取消 | 跨进程 | K | events/emit | `crates/rutis-bridge/tests/conformance_events_tracker.rs::an_emit_is_not_cancelled`、`crates/rutis-bridge/src/session/rpc/tests.rs::a_cancelled_emit_dispatch_stays_queued` | 部分 | #639 ※、#1121 | 宿主侧已修（#1121）：`cancel` 先于有序任务到达时，已接纳的 emit 仍开始；Go `Emit` 的 ctx 取消、Python 任务取消对 emit 无效的运行时用例仍无 |
| EVP-5.7-5 | §5.7 | JavaScript 没有取消分发的办法（明确不保证） | 纯逻辑 | 测试 | — | — | 不适用 | — | 明确不保证，没有要验证的行为 |
| EVP-5.7-6 | §5.7 | 接收方把中止的 `signal` 对应为：Go 监听器的 context 取消；Python 监听器的任务取消；Rust 监听器的 future 丢弃 | 跨进程 | K | — | K `cancel/emitter-gives-up`（b 在 Go、Python、Rust、Swift、Kotlin） | 已覆盖 | — | 规格未写 Swift / Kotlin 的对应 |
| EVP-5.7-7 | §5.7 | JavaScript 监听器没有逐次调用的信号：调用继续执行，回复被丢弃（只有卸载中止 `ctx.signal`） | 跨进程 | K | — | — | 未覆盖 | #639 ※ | 套件以 `except` 排除 JS 监听器；"回复被丢弃"无检查 |
| EVP-5.7-8 | §5.7 | 事件调用不加超时 | 外部 | 测试 | — | — | 不适用 | — | 计时；模型以"之后某时"抽象，#633 未列 |
| EVP-6-1 | §六 | 过滤总是在发出方的进程里执行 | 跨进程 | K | events/hub-filter | K `filter/keeps-and-sees`、`filter/every-listener`（`filter:<row>` 由发出方记） | 已覆盖 | — |  |
| EVP-6-2 | §六 | 快速路径上运行时对每个候选条目直接调用插件的过滤函数 | 进程内并发 | K | events/filter | K `filter/*`（全在 X 的放置） | 已覆盖 | — |  |
| EVP-6-3 | §六 | 经宿主时 `events.dispatch` 带选择器的函数引用；宿主每次分发调用它一次，在快照之后，参数为非 `global` 候选描述的列表，按登记顺序 | 跨进程 | K + TV | events/hub-filter | K `filter/keeps-and-sees`、`filter/every-listener`、`filter/global-skips-filter`（`log` 确切，调两次即失败） | 已覆盖 | — |  |
| EVP-6-4 | §六 | 选择器在发出方进程里按顺序对每个描述调用插件的过滤函数，返回等长的布尔列表（`true` = 送达），真假按本语言的规则 | 跨进程 | K | events/hub-filter | K `filter/keeps-and-sees`、`filter/cordis-this-arg`、`filter/bail` | 已覆盖 | — | 谓词返回 Promise / 协程时按真值保留（`events.mjs:351`、`events.py:280`），与 §5.5"过滤器可以是异步的"冲突 |
| EVP-6-5 | §六 | `bail` 分发中宿主同步调用选择器，其余异步 | 跨进程 | K + TLA | events/hub-filter | K `filter/bail` | 部分 | #614 | 同步的选择器调用参加等待环检测 |
| EVP-6-6 | §六 | 答案长度不对或不是布尔值时分发失败（`BindingError`，`filter of <name> returned <n> answers for <m> listeners`） | 纯逻辑 | PT | events/hub-filter | — | 未覆盖 | #631 ※ | 只有 `event_hub.rs` 普通测试；`hub.rs:1003` 非布尔时 `<n>` 报的是列表长度 |
| EVP-6-7 | §六（2026-10-19 修订） | 发出方自己会话里的条目，描述带 `listener`（它的 `events.on` id）；运行时只凭它找自己的监听器，并在调插件过滤函数前去掉，两条路径上描述相同 | 跨进程 | K | events/hub-filter | K `filter/own-listeners-in-scopes`、`filter/cordis-this-arg`、`filterSees` | 已覆盖 | — |  |
| EVP-6-8 | §六 | 过滤函数抛出时整次分发以这个错误失败，不开始任何调用（`emit`：§5.6） | 跨进程 | K | events/filter | K `filter/throws`、`filter/throws-in-emit` | 已覆盖 | — |  |
| EVP-6-9 | §六 | Go 的过滤函数返回 `(bool, error)`：返回的错误（连同名字）或 panic 就是这次失败 | 纯逻辑 | K | events/filter | K `filter/throws`（Go 发出方） | 部分 | #639 ※ | panic 分支无用例 |
| EVP-6-10 | §六 | Rust `try_filter` 的 `Err` 原样是这次失败；`filter` 只能以 panic 失败 | 纯逻辑 | K | events/filter | K `filter/throws`（rust 发出方） | 部分 | #639 ※ | panic 分支无用例 |
| EVP-6-11 | §六 | 默认：快照按名字取出全部候选；过滤在每个非 `global` 候选上再筛；`global` 条目不经过滤 | 纯逻辑 | K | events/filter、events/flat | K `filter/global-skips-filter`、`filter/global-skips-filter-in-scopes`、`filter/every-listener` | 已覆盖 | — |  |
| EVP-6-12 | §六 Cordis | `thisArg` 的 `[Context.filter]` 为函数时用作 `emit` 及其余四种方法的过滤；本进程监听器传它的 Cordis `Context`，其他条目传 `{ rutis: 描述 }` | 跨进程 | K | events/filter | K `filter/cordis-this-arg` | 部分 | #639 ※ | 只用 `parallel` |
| EVP-6-13 | §六 Cordis | rutis 的 Node 运行时里每个 Context 都有只读的 `ctx.rutis`（它的描述），`c => c.rutis.row === …` 对两种上下文都有效 | 纯逻辑 | K | events/filter | K `filter/cordis-this-arg`（夹具按 `c.rutis` 记录） | 部分 | #639 ※ | "只读"未检查 |
| EVP-6-14 | §六 | 其他语言：`filter(predicate)` 返回有同样五个方法的发出器；predicate 收到描述（对象 / `dict`、Go `rutis.Listener`、Rust `&ListenerInfo`） | 纯逻辑 | K | events/filter | K `filter/keeps-and-sees`、`filter/bail`（各语言发出方） | 已覆盖 | — |  |
| EVP-6-S1 | §六（2026-10-18 原文） | 先应用作用域规则，并且总是应用；过滤只在其上再筛 | — | — | — | — | 已取代 | — | 0.15.1 平总线（1f0a7b3b） |
| EVP-7.1-1 | §7.1 第 1 步 | 宿主在发出 `rows.unload` 或收到 `rows.ended` 时把这一行的条目撤出表（并发 `events.elsewhere`）；运行时再发 `events.off` 也无妨 | 跨进程 | TLA + TV | events/hub-lifecycle | K `unload/not-called-after`、`registration/unloaded-row-silent` | 部分 | #613、#616 | `WithdrawnStaysOut` |
| EVP-7.1-2 | §7.1 第 1 步 | `rows.ended` 与同一会话的 `events.*` 按帧序处理：之前的 `events.on` 先进表、再随行撤下，之后的 `events.dispatch` 不再送达这一行 | 跨进程 | TLA + TV + SH | events/hub-lifecycle | — | 未覆盖 | #613、#616、#628 | `before-ended-order`（#587） |
| EVP-7.1-3 | §7.1 第 2 步 | 运行时立即不再接受这一行的调用：之后到达的 `events.call` 以已撤下的错误失败 | 跨进程 | TLA + K | events/hub-lifecycle | K `unload/not-called-after` | 部分 | #613 | `WithdrawnStaysOut` |
| EVP-7.1-4 | §7.1 第 3 步 | 撤下这一行提供的服务（同 0.12） | 跨进程 | TLA | — | K `rutis/service`（撤下时 `available: false`） | 部分 | #625 | 行生命周期 `WithdrawBeforeUnload` |
| EVP-7.1-5 | §7.1 第 4 步 | 这一行正在运行的监听器调用收到取消信号（Go context；Python 任务取消；JS 这一行的 `ctx.signal` 中止；Rust fiber 的停止）并被等待 | 跨进程 | K + SH | events/lifecycle | K `unload/waits-and-cancels`、`unload/in-flight-emit`、`emit/does-not-wait` | 部分 | #622、#630 ※ | #622 内核在途计数（`wait_events` 看到 0 之后没有调用体再运行）；运行时一侧的取消与等待没有计划（#630 只列了 Python 线程里的卸载先查后做） |
| EVP-7.1-6 | §7.1 第 4 步 | 等待最多 `RUTIS_SHUTDOWN_TIMEOUT`（默认 10 秒；Rust 插件用宿主的关闭截止时间），超过时写 `rutis: listeners of row <row> still running after <s>s; unloading anyway`，放弃并继续 | 外部 | 测试 | events/lifecycle | — | 不适用 | — | 计时与输出文字；Rust 有 `unload_deadline.rs`；#633 未列这一条 |
| EVP-7.1-7 | §7.1 第 5 步 | 按逆序执行清理，然后回复 `rows.unload`（五步依次进行） | 跨进程 | TLA | — | — | 未覆盖 | #625 | 行生命周期模型 |
| EVP-7.1-8 | §7.1 | 条目被撤下时已经发出的调用失败一次（已撤下的错误或传输错误），从不重试 | 跨进程 | TLA + K | events/hub-lifecycle | K `disconnect/in-flight-fails-once` | 部分 | #613 | `SettlesOnce` |
| EVP-7.2-1 | §7.2 | 运行时会话或节点连接结束时，宿主撤下经它来的全部条目（如同它们的行卸载了），并发 `events.elsewhere` | 跨进程 | TLA + TV + SH | events/hub-lifecycle、events/node-links | K `disconnect/restart-hears-again`、`disconnect/node-link` | 部分 | #613、#616、#628 | #628 `SessionEvents`：会话结束后没有远程条目留下 |
| EVP-7.2-2 | §7.2 | 发往它的进行中的调用失败一次，错误同服务调用的传输错误（`BindingError`，信息为会话的结束）：`parallel`/`serial`/`waterfall`/`bail` 交给发出方（`parallel` 在 `AggregateError` 里），`emit` 报告 | 跨进程 | TLA + K | events/hub-lifecycle | K `disconnect/in-flight-fails-once`、`disconnect/parallel-aggregates`、`disconnect/bail-in-flight`、`disconnect/emit-reported`、`disconnect/node-link` | 部分 | #613 | `SettlesOnce`；`waterfall` 无断开用例 |
| EVP-7.2-3 | §7.2 | 对断开一侧的同步等待同样失败 | 跨进程 | TLA + K | events/hub-bail | K `disconnect/bail-in-flight` | 部分 | #614 |  |
| EVP-7.2-4 | §7.2 | 运行时一侧卸载每一行（同 0.12），并把 elsewhere 当作空 | 跨进程 | TLA | — | — | 未覆盖 | #613 | EventsTable 的"会话结束后以新会话重新开始" |
| EVP-8-1 | §八 | `rutis/*` 只由宿主以 `emit` 方式发出，每个只有一个参数（一个记录，状态小写，§15.18） | 跨进程 | K | events/framework | K `rutis/row`、`rutis/service`、`rutis/runtime` | 已覆盖 | — |  |
| EVP-8-2 | §八 | 插件可以监听 `rutis/*`；任何插件以任何方式发出都以 `ReservedEvent` 失败 | 纯逻辑 | K | events/reserved | K `reserved/rutis-emit`、`reserved/every-mode` | 已覆盖 | — |  |
| EVP-8-3 | §八 | `rutis/service` 记录 `{name, scope, available, row}`：一个服务名在一个作用域有了提供方时 `true`；提供方被替换时再 `true`（新 `row`）；撤下且不再有提供方时 `false`（`row` 为最后的提供方） | 跨进程 | K | events/framework | K `rutis/service`、`rutis/service-scoped`、`rutis/service-of-rust` | 部分 | #639 ※ | "替换时再发 `true`"无用例 |
| EVP-8-4 | §八 | `rutis/row` 记录 `{row, plugin, scope, state, error}`：配置的行（任何语言，含 Rust 行）每次状态变化发；`failed` 带 `error`；`row` 是配置的行 id 不是键 | 跨进程 | K | events/framework | K `rutis/row`、`rutis/row-failed`、`rutis/row-failed-gated`、`rutis/row-host-row`、`node/rutis-events` | 已覆盖 | — |  |
| EVP-8-5 | §八 | `rutis/runtime` 记录 `{runtime, state, reason}`：挂载成功时（每次重连之后也是）`running`；会话结束或挂载失败时 `down`，`reason` 说明原因 | 跨进程 | K + SH | events/framework | K `rutis/runtime`、`rutis/runtime-down-reason` | 部分 | #628 | #628 疑点：跟随 `watch`，快速重启时 `down` 与第二次 `running` 被合并（确定性测试）；挂载失败的 `down` 无用例 |
| EVP-8-6 | §八 表"发出方作用域"列、§15.18、§15.33 | `rutis/service` 在服务的作用域、`rutis/row` 在行的作用域、`rutis/runtime` 在根发出 | — | — | events/framework | — | 已取代 | — | 平总线后发出方作用域不可观察（过滤只看监听器描述，0.16 删 `Dispatch::scope`）；规格表、§15.18、§15.33 与登记表 `events/framework`（"于该行的作用域发出"）仍这样写 |
| EVP-8-7 | §八 | `internal/*` 留在每个运行时内部，从不向宿主登记或发给宿主 | 跨进程 | K | events/hub-reserved | — | 未覆盖 | #639 ※ | 宿主拒绝只有普通测试；套件无 `internal/*` 用例 |
| EVP-9-1 | §九 | 连接用已有的 `export` / `import`（与服务共用）共享事件；一项是名字或以 `*` 结尾的前缀（单独 `*` 为所有名字）；前缀项只用于事件，服务仍按确切名字 | 纯逻辑 | PT + K | events/link-export-import | K `node/not-shared`（`n/*` 不共享） | 部分 | #631 | #631 export / import 收窄 PT |
| EVP-9-2 | §九 | A 的 `export: [x]` = A 发出的 x 到达 B 的监听器；B 的 `import: [x]` = B 的监听器收到 A 的 x；条目从导入方流向导出方：B 通告与 `import` 匹配的，A 接受与 `export` 匹配的 | 跨进程 | K + TV | events/link-export-import | K `parallel/all-ok`、`serial/first-result`、`waterfall/veto`（node 位置）、`node/not-shared` | 部分 | #616、#631 |  |
| EVP-9-3 | §九 | 同一个名字可以双向共享 | 跨进程 | K | events/link-export-import | K `node/shared-both-ways` | 已覆盖 | — |  |
| EVP-9-4 | §九 | 连接开始时每个节点登记 `events` 族；连接上的消息为 `events.on` / `events.off` / `events.call` | 跨进程 | TLA + K | events/node-links | K（node 位置的用例） | 部分 | #613 | EventsLink 的提供（`link.offers`） |
| EVP-9-5 | §九 | 连接上 `events.on` 的参数为 `{id, name, prefix, plugin, row, runtime, kind, prepend, global}` | 纯逻辑 | K | events/node-links | K（node 位置；`filterSees` 核对 `node`） | 部分 | #639 ※ | `plugin`、`kind` 等字段未核对 |
| EVP-9-6 | §九 | 接收方提供 `events` 时，发送方按表中顺序通告整个表（`prepend: false`），此后每个条目在登记时通告（`prepend` 如登记时）：接收方的顺序与发送方一致 | 跨进程 | TLA + TV + K | events/node-links | K `order/registration`、`order/prepend`（node 位置的放置） | 部分 | #616 | EventsLink 不建顺序；`OneOrder` 只在 Table 模块 |
| EVP-9-7 | §九 | `events.off` 撤下；`events.call` 同 §5.3，调用发送方接受的条目；被调用的节点执行它（Rust 监听器，或转给它的运行时） | 跨进程 | K | events/node-links | K（node 位置的用例） | 已覆盖 | — |  |
| EVP-9-8 | §九 | 节点只通告它自己的条目（宿主 Rust 插件和它的运行时的），从不通告经任何连接引进的条目（不转发，不成环） | 跨进程 | TLA + K | events/no-multi-hop | — | 未覆盖 | #613 | `NoEcho`（一跳）；a—b—c 只有 `events_nodes.rs` 普通测试 |
| EVP-9-9 | §九 | `export` / `import` 名单是唯一的边界 | 跨进程 | K | events/flat、events/link-export-import | K `node/root-hears-instances` | 已覆盖 | — | 0.15.1 去掉作用域边界 |
| EVP-9-10 | §九 | 前缀条目按它与 `import` 项共同匹配的部分收窄后通告（两个前缀：一个延伸另一个时取较长的；前缀与名字：取名字） | 纯逻辑 | PT | events/node-links | — | 未覆盖 | #631 |  |
| EVP-9-11 | §九 | 接受的条目放在接受方连接的作用域（描述的 `scope`），按到达顺序追加，`prepend: true` 的放在全部已有条目之前；描述 `node` = 对端 id，`plugin`/`row`/`runtime` 按通告 | 跨进程 | K + TV | events/node-links | K `filterSees`（node 放置核对 `node`、`scope`） | 部分 | #616 |  |
| EVP-9-12 | §九 | 通告来的条目不匹配 `export` 的任何项则忽略（回复 `undefined`） | 纯逻辑 | PT + TV | events/link-export-import | — | 未覆盖 | #616、#631 |  |
| EVP-9-13 | §九 | 宣告 id 为 `<epoch>.<n>`：`epoch` 为发送方这次提供 `events` 族的序号（`link.offers` 中的 `since`），`n` 在这次提供内递增 | 跨进程 | TLA | events/node-links | — | 未覆盖 | #613 | EventsLink |
| EVP-9-14 | §九 | 接收方看到对端更新的提供（经 `link.offers`，或 epoch 更大的宣告先到）时，撤下对端在更早提供下宣告的全部条目 | 跨进程 | TLA | events/node-links | — | 未覆盖 | #613 | `CurrentOffer`；`accept-unlocked` 疑点 |
| EVP-9-15 | §九（2026-10-23 修订） | 对端的 `link.offers` 不再列出它列出过的 `events` 提供时，撤下那次及更早提供下的条目；`link.offers` 还没列出的那次提供的条目留着 | 跨进程 | TLA | events/node-links | — | 未覆盖 | #613 | `CurrentOffer`、`before-offer-race`（#593） |
| EVP-9-16 | §九 | 更早提供的迟到宣告不接受（回复 `undefined`） | 跨进程 | TLA | events/node-links | — | 未覆盖 | #613 | `CurrentOffer` |
| EVP-9-17 | §九（2026-10-19 修订） | 不带 epoch 的 id（不说明它的实现）只按 id 替换 | 跨进程 | TLA | — | — | 未覆盖 | #613 ※ | EventsLink 只写了 `<epoch>.<n>` |
| EVP-9-18 | §九 | 连接上的条目规则与本机完全相同：五种方式、按名字送达、过滤、值类型、撤下与断开 | 跨进程 | K | events/node-links | K（node 位置：`parallel/all-ok`、`serial/first-result`、`waterfall/veto`、`node/values`、`node/function-argument`、`disconnect/node-link`） | 已覆盖 | — | 套件的 node 只是 Rust 节点；Cordis 节点（`features.mjs`）只有 `cordis-events.test.mjs` 普通测试 |
| EVP-9-19 | §九 | 断开的连接的条目被撤下 | 跨进程 | TV + K | events/node-links | K `disconnect/node-link` | 部分 | #616 |  |
| EVP-9-20 | §15.31 | 节点按自己的条目每匹配一个 `import` 项就宣告一次（按 §9 收窄），每次宣告有自己的 id；对端宣告的条目同样按每匹配一个 `export` 项接受一次 | 纯逻辑 | PT + TV | events/node-links | — | 未覆盖 | #616、#631 | 与实现不符：`link.rs` `selected()` 去掉被另一前缀覆盖的前缀与被前缀覆盖的名字（嵌套前缀只送达一次），规格没写这一去重 |
| EVP-9-21 | §15.31 | 宣告在对端提供 `events` 族时开始，对端每次重新提供时从头再来 | 跨进程 | TLA | events/node-links | — | 未覆盖 | #613 | EventsLink |
| EVP-9-S1 | §九（2026-10-18 原文） | 节点只通告作用域链是连接作用域前缀的条目 | — | — | — | — | 已取代 | — | 0.15.1 平总线（1f0a7b3b） |
| EVP-10-1 | §十 | 事件参数、结果、`next` / 终态的结果、过滤的描述在每条路径上都是会话值：十种值标记、函数与对象引用、信号、64 层深度上限，编码与能力规则同调用 | 跨进程 | K + PT | events/one-bus、bridge/value-tags | K `values/every-tag`、`values/set`、`values/through-javascript`、`values/function-argument`、`serial/argument-reaches`；PT `value_roundtrip.rs`（同一编码层，经服务调用） | 部分 | #639 ※ | 深度上限、信号作参数、对象引用作结果在事件路径上无用例 |
| EVP-10-2 | §十 | 发送方按接收方能力检查；发不出的值只让那次调用失败（`parallel` 中为 `AggregateError` 的一个子错误，`emit` 中为一行报告），对 `events.dispatch` 则整次分发失败 | 跨进程 | K + TV | events/hub-errors | — | 未覆盖 | #616、#639 ※ | TV 只查宿主之后的反应（值发不出去是"可以发生"的选择）；错误落点无用例 |
| EVP-10-3 | §十 | 节点连接同样承载会话值（0.12 的"只能 JSON"取消） | 跨进程 | K | events/node-links | K `node/values`、`node/function-argument` | 已覆盖 | — |  |
| EVP-10-4 | §十 | 作为事件参数传递的引用与调用参数一样存活到这次调用结束 | 跨进程 | TLA + K | — | K `values/function-argument`（调用期间可用） | 部分 | #626 | "结束后释放"未检查；#626 `PinnedTarget`、计数释放 |
| EVP-11-1 | §十一 | `AggregateError` 信息 `<n> listeners of <name> failed`；线上错误图的根是名为 `AggregateError` 的 `error` 节点，其 `errors` 引用各子错误节点（子图并入），与 JS 编码一致 | 纯逻辑 | PT + K | events/hub-errors | K `parallel/errors-in-registration-order`（子错误名与顺序） | 部分 | #631 | #631"事件错误的移植（graft、moved）"；信息文字无检查 |
| EVP-11-2 | §十一、§15.32 | 各语言都按登记顺序还原 `AggregateError` 的子错误 | 跨进程 | K | events/hub-errors | K `parallel/errors-in-registration-order`、`parallel/one-error-is-aggregated` | 已覆盖 | — |  |
| EVP-11-3 | §十一 | `AsyncListener` 信息 `listener of <name> in <监听器> is asynchronous and cannot run in a synchronous <bail\|waterfall-sync>`，`<监听器>` 取描述里第一个有值的 row / runtime / node / plugin | 纯逻辑 | K | events/serial-bail | K `waterfall-sync/async-listener`（信息子串） | 部分 | #639 ※ | `<监听器>` 的取法无检查 |
| EVP-11-4 | §十一 | `WaterfallNext` 各情形的确切信息（调两次、结束后、不在 waterfall 中、Rust 同步监听器在异步 waterfall 中、别处同步 `next` 不在分发线程上） | 纯逻辑 | K | events/one-table | K `waterfall/next-twice-in-sync`、`rust/sync-next-in-async-waterfall`、`rust/next-outside-waterfall` | 部分 | #639 ※ | "结束之后""不在分发线程上"无用例 |
| EVP-11-5 | §十一 | `ReservedEvent`：发出 `rutis/*`（任何方式、任何语言）由发出的 API 同步拒绝，信息 `rutis/* events are emitted by rutis only: <name>` | 纯逻辑 | K | events/reserved | K `reserved/every-mode`（信息）、`reserved/rutis-emit`（emit 同步抛出） | 已覆盖 | — |  |
| EVP-11-6 | §十一 | `SyncWaitCycle` 同服务调用 | 跨进程 | TLA + K | bridge/sync-wait-cycle-refused | K `bail/wait-cycle` | 部分 | #614 |  |
| EVP-11-7 | §十一 | `BindingError`：传输错误、已撤下的监听器、被拒绝的帧；信息为传输的信息或 §3、§4.2、§5.3、§6 给出的 | 纯逻辑 | K | events/hub-lifecycle | K `disconnect/*`（`BindingError`） | 部分 | #639 ※ | 被拒绝的帧的信息无用例 |
| EVP-11-8 | §十一 | 监听器自己的错误带着名字、信息与错误图跨边界，与任何调用的错误相同 | 跨进程 | K | events/one-bus | K `serial/error-stops`、`waterfall/error`、`bail/error-stops`（名字与信息） | 部分 | #639 ※ | 错误图（cause、data）在事件路径上无用例 |
| EVP-11-9 | §十一 | 没有人等待的失败在发生的一侧报告，每个运行时一行 stderr | 跨进程 | K | — | K 各用例的 `reported` | 已覆盖 | — | Go 另有一行未列：`rutis: row <key> is not active: listener of <name> is not registered`（`events.go:193`） |
| EVP-11-10 | §十一 | 经宿主发送的 `emit` 被拒绝或没有得到回答：`rutis: emit of <name> from row <row> failed: <ErrorName>: <message>` | 外部 | 测试 | — | — | 不适用 | — | 输出文字；`reported` 只覆盖监听器与过滤两种行 |
| EVP-11-11 | §十一 | 宿主拒绝一个监听器的 `events.on` 时报告；行加载期间则改为让加载失败 | 跨进程 | K | — | — | 未覆盖 | #639 ※ | "加载期间让加载失败"是行为，无用例 |
| EVP-11-12 | §十一 | 上一条的报告文字为 `rutis: registration of listener of <name> in row <row> failed: …` | 外部 | 测试 | — | — | 不适用 | — | 输出文字 |
| EVP-12.1-1 | §12.1、§15.20 | 每个 Rust 事件类型都能跨进程：`Event` 要求 `Serialize + DeserializeOwned`（`Value` 也是） | 纯逻辑 | K | core/D2 | K `rust/typed-args`、`rust/unit-event` | 已覆盖 | — | 编译期约束；跨进程由用例 |
| EVP-12.1-2 | §12.1 | `Call::next` 只在 waterfall，其他方式 `Err(WaterfallNext)`；`SyncCall::next` 只在 `waterfall_sync`，在异步 waterfall 中 `Err(WaterfallNext)` | 纯逻辑 | K | events/one-table、core/D17 | K `rust/next-outside-waterfall`、`rust/sync-next-in-async-waterfall` | 已覆盖 | — |  |
| EVP-12.1-3 | §12.1 | `Terminal` 为 `()` 时没有终态：最后的 `next` 得到 `None` | 纯逻辑 | K | events/waterfall | K `waterfall/end-without-terminal`（rust 发出方） | 已覆盖 | — |  |
| EVP-12.1-4 | §12.1 | `EventOptions` 的 `prepend`、`once`（只被调用一次，调用开始时才认领）、`global` | 进程内并发 | PT + SH | events/order | — | 未覆盖 | #631 | #631 五种方式 PT 含 once / prepend / global；`once` 无一致性用例 |
| EVP-12.1-5 | §12.1 | `RemoteListener` 的 `kind() == Async` 从不被 `bail` / `waterfall-sync` 调用（`AsyncListener`） | 纯逻辑 | K | events/remote | K `bail/async-listener`（运行时的条目就是 RemoteListener） | 已覆盖 | — |  |
| EVP-12.1-6 | §12.1 | `RemoteListener::call` 在调用开始时按登记顺序被调用（`emit`：在分发完成之前）；丢弃 future 即取消这次调用 | 进程内并发 | SH + TV | events/remote | — | 未覆盖 | #616、#622 ※ |  |
| EVP-12.1-7 | §12.1 | `call_sync` 在分发线程上就地调用，不持内核锁；bridge 的 sync-wait 路径照常适用 | 进程内并发 | SH + TLA | events/remote | — | 未覆盖 | #614 | #614 宿主参与者（工作线程） |
| EVP-12.1-8 | §12.1 | `RemoteNext` / `RemoteSyncNext`：第一次调用执行其余的链；第二次以 already called 失败；条目的调用结束后再调用以 called after its waterfall ended 失败 | 纯逻辑 | PT | events/remote | — | 未覆盖 | #631 | #631 五种方式参考解释器（含远程监听器、调用次数）；现有 `remote_waterfall_next_runs_once` 为普通测试 |
| EVP-12.1-9 | §12.1 | `watch_table`：先给当前条目，再按发生顺序给每次变化；Rust 的 `on` 在返回之前已报告 | 进程内并发 | SH | events/remote | — | 未覆盖 | #622 | #622"监听器表的观察者"（FIFO、恰好一次） |
| EVP-12.1-10 | §12.1 | 原始 `dispatch` 对 `emit` 在每个调用都开始后完成；`dispatch_sync` 用于 `bail`、`waterfall-sync` | 进程内并发 | SH + TV | events/remote | — | 未覆盖 | #616、#622 | #622 emit 链模型 |
| EVP-12.1-11 | §12.1 | `invoke_entry` 在分发之外按 id 调用一个条目（节点执行对端的 `events.call`）；条目必须是这个名字的 | 纯逻辑 | PT | events/remote | — | 未覆盖 | #631 ※ | `an_entry_called_by_id_must_have_the_name` 为普通测试 |
| EVP-12.1-12 | §12.1 | `emit_framework`：只有 rutis 发出 `rutis/*` | 纯逻辑 | K | events/reserved | K `reserved/*`、`rutis/*` | 已覆盖 | — |  |
| EVP-12.1-13 | §12.1 | `EventKey::of` 的名字为 `E::NAME`，`named` / `dynamic` 为给的名字；`EventPattern::prefix` 登记前缀条目 | 纯逻辑 | PT | events/by-name | K `rust/typed-args` | 部分 | #631 | 前缀条目无用例 |
| EVP-12.1-14 | §12.1 | 监听器只按名字查找：同名的另一个 Rust 类型的监听器也收到事件，经转码器（serde）转换，结果以发出方的类型返回 | 纯逻辑 | PT + K | events/by-name、core/D2 | K `rust/typed-args`（运行时发出、宿主有类型的监听器） | 部分 | #631 | #631 事件参数编解码 PT |
| EVP-12.1-15 | §12.1 | 参数 ↔ `E`：两个及以上字段的元组 → 每字段一个参数；unit → 无；其余 → 一个；解码 0 / 1 / ≥2 个参数对应 | 纯逻辑 | PT | events/by-name | K `rust/typed-args`、`rust/unit-event` | 部分 | #631 |  |
| EVP-12.1-16 | §12.1 | bridge 的转码器按服务参数的规则编码：`Bytes`、`Instant`、`ValueMap`、`ObjectRef`、`RemoteFunction` 作为值标记与引用跨边界 | 纯逻辑 | PT + K | events/by-name、bridge/value-tags | K `values/*`（host 位置）；PT `value_roundtrip.rs`（服务调用路径） | 部分 | #631 |  |
| EVP-12.1-17 | §12.1 | 动态事件 `Args` 只以 `EventKey::<Args>::dynamic(name)` 使用；用 `of()` 以 `BindingError`（`Args events need a name: EventKey::dynamic(name)`）失败 | 纯逻辑 | 测试 | events/by-name | — | 不适用 | — | 单一判定与输出文字；`event_hub.rs::args_events_need_a_name` |
| EVP-12.1-18 | §12.1 | Rust 监听器以登记它的上下文被调用（它的 `ctx.cancelled()` 就是卸载信号） | 进程内并发 | SH | events/lifecycle | — | 未覆盖 | #622 | #622 事件调用的在途计数 |
| EVP-12.1-19 | §12.1 | `serial` / `bail_sync` 中 `Ok(Some(false))` 是没有结果；`waterfall` 原样返回 | 纯逻辑 | K | events/serial-bail | K `rust/some-false-is-no-result`、`waterfall/false-stays-false` | 已覆盖 | — |  |
| EVP-12.1-20 | §12.1 | 发出方的作用域 `ctx.scope()` 只是描述，事件按名字送达；进一步挑选按 `ListenerInfo::scope` 过滤 | 纯逻辑 | K | events/flat | K `scope/*`（e 在 host 的放置） | 已覆盖 | — |  |
| EVP-12.1-21 | §12.1 | `Ctx::mark_row` 使这个 fiber 成为行（其中及其下登记的监听器的 `ListenerInfo::row`），`Ctx::row` 为最近的标记行；`mark_scope` 在组实例的行开始前调用 | 纯逻辑 | K | events/one-bus | K `rutis/row-host-row`、`filterSees`（host 位置的 `row`） | 已覆盖 | — |  |
| EVP-12.1-S1 | §12.1（2026-10-18 原文） | 插件没有向别的作用域发出的 API；原始 `Dispatch` 带可选的发出方作用域 | — | — | — | — | 已取代 | — | 4549f495；0.16 删 `Dispatch::scope` |
| EVP-12.2-1 | §12.2 | 每种语言里 waterfall 的监听器都把 `next` 作为最后一个参数收到 | 跨进程 | K | events/waterfall | K `waterfall/*`（各语言 Y） | 已覆盖 | — |  |
| EVP-12.2-2 | §12.2、§15.22 | Go 的 `Emit` 只对拒绝返回错误，不等监听器 | 纯逻辑 | K | sdk/leaf-events | K `emit/does-not-wait`、`reserved/rutis-emit`（Go 发出方） | 已覆盖 | — |  |
| EVP-12.2-3 | §12.2（2026-10-19 修订） | 终态：Cordis 为最后一个参数；叶子用 `ctx.waterfall.with(terminal)(name, ...args)`（参数都是事件的）；Python `terminal=`；Go `terminal` 可为 nil；都以事件参数调用 | 纯逻辑 | K | events/waterfall | K `waterfall/terminal`、`waterfall/terminal-gets-args`、`waterfall/end-without-terminal` | 已覆盖 | — |  |
| EVP-12.2-4 | §12.2 | Node 与 Python 的 `bail` 是同步的 | 纯逻辑 | K | events/hub-bail | K `bail/*` | 已覆盖 | — |  |
| EVP-12.2-5 | §12.2 | `parallel` 失败的类型：JS `AggregateError`、Python `rutis.AggregateError`、Go `*rutis.AggregateError` | 纯逻辑 | K | events/parallel | K `parallel/errors-in-registration-order`（按错误名） | 部分 | #639 ※ | 只按名字比对，不查各语言的类型（`isinstance`、`errors.As`） |
| EVP-12.2-6 | §12.2 | 没有结果到达时是 `undefined` / `None` / `nil` | 纯逻辑 | K | events/serial-bail | K `serial/no-result`、`bail/no-result`、`modes/no-listeners` | 已覆盖 | — |  |
| EVP-12.2-7 | §12.2 | Node 的 `waterfall` 只要有监听器在本进程之外就返回 Promise | 纯逻辑 | 测试 | — | — | 不适用 | — | API 的返回类型，单元测试即可 |
| EVP-12.2-8 | §12.2 | Go `rutis.Listener` / `rutis.Scope` / `rutis.Next` 的形状（null 为 `""`）；Python 与叶子插件的描述是带 JSON 字段名的普通 `dict` / 对象 | 纯逻辑 | K | events/filter | K `filterSees`（Go、Python、叶子发出方） | 已覆盖 | — |  |
| EVP-12.2-9 | §12.2 | 叶子插件（Python、Go、Node 叶子）不能监听或发出 `internal/*`；`rutis/*` 可以监听、不能发出 | 纯逻辑 | K | events/hub-reserved、sdk/leaf-events | K `reserved/*`、`rutis/*`（`rutis/*` 一半） | 部分 | #639 ※ | `internal/*` 一半无用例 |
| EVP-12.2-10 | §12.2 | 测试工具提供同样的五种方法、`filter`、作用域（把行加载到命名的作用域路径）、卸载等待与取消，结果与真实运行时相同 | 跨进程 | K | events/one-bus、sdk/testing-matches-runtime | K 各测试工具跑全部非 `tool: false` 用例（`toolCases` 计数） | 已覆盖 | — |  |
| EVP-12.2-11 | §12.2 | Node、Python 的测试工具在一个进程里（总是快速路径）；`rutistest` 经真实会话在真实 Go 运行时里运行插件，测试一方是总线的宿主 | 跨进程 | K | events/one-bus | K（`rutistest` 跑套件） | 已覆盖 | — |  |
| EVP-12.2-12 | §12.2 | 测试工具不发出 `rutis/*` | 纯逻辑 | 测试 | — | — | 不适用 | — | 不提供的功能，没有要验证的行为（用例以 `tool: false` 标出） |
| EVP-12.2-13 | §12.2、§15.27 | `prepend` 与 `global` 在所有语言中都有（Cordis 自己的；叶子 `{prepend, global}`；Python `prepend=`、`global_=`；Go `rutis.Prepend()` / `rutis.Global()`；Rust `EventOptions`） | 纯逻辑 | K | events/order、events/filter | K `order/prepend`、`filter/global-skips-filter`（vary 各语言） | 已覆盖 | — |  |
| EVP-13.1-1 | §13.1 | 每种语言提供插件 `conformance-events`（Rust 宿主提供同名 Rust 插件）：每个参与者一行；监听器行按配置登记一个监听器；发出方行提供 `emitter-<label>` 的 `dispatch(spec)` / `dispatch_sync(spec)`，用原生 API 发出 | 纯逻辑 | K | events/one-bus | K `events_expected.rs`（文件形状）、`events_conformance.rs` | 已覆盖 | — |  |
| EVP-13.1-2 | §13.1 | 每次调用开始、过滤调用与取消都经宿主的同步服务 `events-log.add(entry)` 记录；`events-log.until(entry)` 同步等到该条被记下 | 跨进程 | K | events/one-bus | K（全部用例依赖它） | 已覆盖 | — |  |
| EVP-13.2-1 | §13.2 | 监听器 `kind` 默认：每一步都是 `bail` / `waterfall-sync` 时 `sync`，否则 `async`；Go 总是 `sync`；JS / Python 的 `async` = 异步函数，`any` = 普通函数 | 纯逻辑 | K | — | K（夹具） | 已覆盖 | — |  |
| EVP-13.2-2 | §13.2 | `waits` 的 `sync` 监听器在不能被唤醒的语言（JS、Python）里阻塞所在线程直到进程结束 | 进程内并发 | K | — | K `disconnect/bail-in-flight` | 已覆盖 | — |  |
| EVP-13.2-3 | §13.2 | 测试工具把每个不带 `tool: false` 的用例的全部位置设为 X 运行，并核对跑的用例数等于 `toolCases` | 纯逻辑 | K | events/one-bus | K `events_expected.rs`（`toolCases`） | 已覆盖 | — |  |
| EVP-13.3-S1 | §13.3 起始用例 | `filter/keeps-and-sees` 的 `filterSees` 不含 t2 的 d、`rutis/row` 的 `logLacks: o:z,…`（作用域送达的期望） | — | — | — | — | 已取代 | — | 期望文件里的同名用例已改为平总线；规格 §13.3 未同步 |
| EVP-15.29-1 | §15.29（2026-10-21 修订） | 宿主按顺序处理一个会话的 emit 而不阻塞读线程：能马上开始的在会话的有序任务上轮到时开始并作答；选择器调用要从同一会话回来的，连同之后每个 emit 进入队列，各自在调用都开始后作答 | 进程内并发 | SH + TV + TLA | events/hub-dispatch | — | 未覆盖 | #613、#616、#628 | `hub.rs` `run_emit` / `next_emit` |
| EVP-15.29-2 | §15.29 | 运行时仍会等以 future 作答的 emit 回复，对 0.13 宿主保持 0.13 那种依赖时序的顺序 | 跨进程 | 测试 | bridge/cross-version-meetings | — | 不适用 | — | 要 0.13 的宿主（跨版本） |
| EVP-15.33-1 | §15.33 | `rutis/row`：加载器生成该行时发 `pending`，之后按 fiber 状态的每次变化发（`loading` 算 `pending`；`unloading` 与销毁算 `stopped`）；组行也是行（`plugin` 为 `group`） | 跨进程 | K | events/framework | K `rutis/row`（`w:o,pending`…）、`rutis/row-failed` | 部分 | #639 ※ | 组行无用例 |
| EVP-15.33-2 | §15.33 | `rutis/service` 针对每个宿主服务发出，不论提供方是 Rust 插件、运行时的行还是节点导入，带提供它的行的 id | 跨进程 | K | events/framework | K `rutis/service`、`rutis/service-of-rust`；`crates/rutis-bridge/tests/session_plugin.rs::a_node_imported_service_is_announced_with_its_row`、`crates/rutis-bridge/tests/session_plugin.rs::a_node_imported_service_without_a_row_announces_none` | 已覆盖 | #639 ※、#1171 |  |
| EVD-1-1 | 设计 §一 | 一个事件在哪里发出、监听器在哪里，结果都和所有监听器在同一个进程里一样 | 跨进程 | TLA + K | events/one-bus | K（全部用例在全部放置下结果相同） | 部分 | #613 | 字面上不成立：两个运行时在 elsewhere 更新前各走快速路径时排不出一致的全序（#613 `SeesAll` 预期反例），同一进程里并发的发出也没有先后；按 Q5.1.1 应限定，并在协议 §4.4 写明不保证（#613 的产出） |
| EVD-1-2 | 设计 §一、§四 | 做不到的只有物理上不可能的情形（同步等待成环、同步等待已断开的对端、同步方式遇到异步监听器），这时明确报错，从不悄悄换成别的行为 | 跨进程 | K + TLA | events/serial-bail、events/hub-bail | K `bail/wait-cycle`、`disconnect/bail-in-flight`、`bail/async-listener` | 部分 | #613、#614 | §一 只列两种，§三.3 / §四 又加了异步监听器；协议另有值发不出、过滤答案数不符等失败；Node / Bun / Python 把返回 Promise / 协程的过滤谓词悄悄当作保留（#613 待复现疑点） |
| EVD-3.1-1 | 设计 §三.1 | 登记是自动的：插件照常写 `ctx.on(name, listener)`，不需要任何声明 | 跨进程 | K | sdk/leaf-events、events/one-bus | K（各语言夹具都不声明事件） | 已覆盖 | — |  |
| EVD-3.3-1 | 设计 §三.3 | 一张监听器表：一个监听器登记一次就可以被任何方式分发，方式由发出方决定；`on_sync` 的同步监听器可被所有方式调用 | 跨进程 | K | events/one-table | K `kinds/sync-listener-every-mode`；各方式的用例共用同样配置的监听器 | 已覆盖 | — |  |
| EVD-3.7-1 | 设计 §三.7 | Cordis 插件在本进程内用的 `internal/dispatch`、`internal/service` 不变 | 进程内并发 | K | — | K `cordis/internal-dispatch`（只到别处来的分发） | 部分 | #639 ※ | `internal/service` 与本进程发出的 `internal/dispatch` 无检查 |
