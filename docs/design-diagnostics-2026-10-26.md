# 0.18 的诊断：错误、当前状态、时限

[English](design-diagnostics-2026-10-26.en.md)

状态：定稿，未实现。日期：2026-10-26（同日修订：删去调用时限与 Python 日志设置，关闭时限归宿主，错误位置只给看输出的人与宿主，补"格式化不能出错"）。基准：`main` `ce131824`（0.17.1 之后），并假定 0.17.2 的 #704（等待原因 `Loader::waits`）已合并。排期：0.18。
Issue：[#689](https://github.com/arcships/rutis/issues/689)。按本稿实施：[#677](https://github.com/arcships/rutis/issues/677)（错误，含并入的 #680）、[#678](https://github.com/arcships/rutis/issues/678)（状态、时限、输出，含并入的 #681）、[#688](https://github.com/arcships/rutis/issues/688)（没处理的错误归属）、[#676](https://github.com/arcships/rutis/issues/676) 后半（`check` 报错）。
依据：[#663](https://github.com/arcships/rutis/issues/663) 的 0.17.0 调试评估（4 种语言 × 14 种故障：清楚 5，要猜 25，误导或不说 22）与宿主持有者端到端评估。

范围外：调用的时限（归调用方，见 §五）；调试器怎样接（#679，只引用本稿的输出）；值在语言之间怎样转换（#675、#687）；跟踪系统（OpenTelemetry 等）；宿主改写平台名字的接口（出错信息不出现平台名字，就不需要它）；生成的类型声明（#684）；测试工具读状态快照（测试工具里没有 loader，各语言已有 `provided()` / `reported`；#689 原文列了它，本稿不做）。

## 一、开发者要的是什么

出错的时候，从输出里直接知道：

| 问题 | 例子 |
| --- | --- |
| 是谁 | 哪一行 |
| 在做什么 | 哪个服务的哪个方法、第几个参数；或者 `apply`、清理 |
| 为什么 | 一句人话；rutis 自己的错误另有错误码给程序判断 |
| 在哪里 | 自己代码的文件和行号；跨语言时两边都有 |
| 怎么改 | 能说出来的时候说出来 |

卡住的时候，看得到现在在发生什么：哪些行在跑、在等什么、谁在调用谁、调了多久。

现在做不到的原因：

1. **结构有，但半路丢了。** 跨语言的错误本来就有结构（错误图：`name`、`message`、`stack`、`code`、`data`、`cause`、`errors`）。loader 的 `failed()`（`crates/rutis-loader/src/runtime.rs` 与 `peer.rs`）把它转成字符串再交给内核；宿主拿到的 `CallError` 里虽然留着原错误，却没有读取入口。Go、Swift、Kotlin 发出的堆栈只有"名字: 消息"；Node 收到时用对面的堆栈覆盖自己的。
2. **没人记下"是谁、在做什么"。** 错误里没有行和方法；rutis 自己的错误没有错误码。
3. **状态散在各处。** 内核 `diagnostics()`、loader `entries()` 与 0.17.2 的等待原因、运行时状态各管一块，没有"正在进行的调用"，`rutis-host` 没有 `status` 命令。
4. **卡住时不说是谁。** 看不到哪个调用等了多久；关闭卡住时报的是根，不是卡住的插件。

## 二、原则：平台只看，不替业务做决定

诊断要的是"看得到"，不是"替别人决定"。下面几件事各有归属，本稿不越过：

| 事 | 归谁 | 本稿怎么做 |
| --- | --- | --- |
| 一个调用等多久 | 调用方的业务代码 | 不设调用时限；只在状态里显示进行中的调用与已经等了多久 |
| 关闭等多久 | 宿主（嵌入 rutis 的程序） | 内核不定默认值；rutis-host 作为一个宿主选 10 秒（已有） |
| 插件进程里的语言全局设置（日志级别等） | 插件 | 不改；指南写明各语言的默认规则 |
| 被调用的是哪一行、它的代码位置 | 看输出的人与宿主持有者 | 只出现在 rutis 打印的输出和宿主的读取接口里，不交给调用方的代码（原则 8：依赖里只有服务名，不出现位置） |
| 运行时进程、终端、平台自己的生命周期 | 平台 | 平台决定：没处理的错误怎样归属、输出前缀、握手时限 |

## 三、决策

| # | 决策 | 章节 |
| --- | --- | --- |
| G1 | 错误沿用现有的错误图；loader 不再把它转成字符串；rutis 在边界上给错误补上"是谁、在做什么"（`site`），只用于打印与宿主读取 | 四 |
| G2 | rutis 自己产生的 `BindingError` 有 6 个错误码；插件自己的 `code` 原样保留 | 四 |
| G3 | Go、Swift、Kotlin 发出真实位置；各语言收到对面的错误时，交给代码的错误对象只有名字、消息、错误码、data，对面的错误图在内部保留，再抛出时原样带上；rutis 打印时两边都显示 | 四 |
| G4 | 出错信息只用开发者写过的东西说话，按 #677（含 #680）的清单逐条改写 | 四 |
| G5 | 格式化错误不能失败：任何坏数据都退回"名字: 消息"，不丢原错误 | 四 |
| G6 | 当前状态是 `Loader::status()` 一份快照：行、服务、运行时、进行中的调用 | 五 |
| G7 | 调用不设时限；关闭的时限由宿主决定，超时报告写明卡住的插件 | 六 |
| G8 | Node、Bun、Python 运行时被同步代码占住 5 秒，打印是哪一行的哪个方法，不打断 | 六 |
| G9 | 插件输出带名字前缀：`dev` 默认开，`run` 默认关 | 七 |
| G10 | Rust 嵌入默认把加载失败、运行时退出写到标准错误，输出里不出现平台名字 | 七 |
| G11 | 各语言的错误类型名与字段不变，`Failure` 等 loader 类型不变；变的是出错文字、`plugin failed: ` 前缀、Node 的 `error.stack`，以及 Rust 的 `session::Error` 新增一个带码的分支（§4.3） | 十一 |

## 四、错误

### 4.1 结构：沿用错误图，补上 `site`

错误图不变（[插件挂载设计](design-protocol-plugin-mount.md)，`{root, nodes}`，节点 `{type: "error", name, message, stack, cause?, errors?, code?, data?}`）。错误节点加一个可选字段：

```text
site: {
  row:      "weather",                          // 行名；实例里的行写 "weather in phone-1"
  doing:    "call" | "apply" | "cleanup" | "listener",
  service?: "forecast",                         // doing = call
  method?:  "tomorrow",
  event?:   "weather/asked"                     // doing = listener
}
```

- **由 rutis 在边界上填**，不由各 SDK 填：
  - 调用：被调用的行返回错误时，rutis 在 `RowService::invoke`（`crates/rutis-loader/src/runtime/rows.rs`）知道是哪一行的哪个服务、方法，给错误的根节点补上 `site`；
  - `apply`、清理：loader 知道是哪一行；
  - 监听器：事件的转发处已经知道监听器在哪一行（现在的 `listener of <name> in row <row> failed`）。
- 已经有 `site` 的节点不覆盖（多跳时保留最早出错的那一跳）。
- **谁看得到**：rutis 打印的输出（`dev`、`run`、Rust 嵌入的默认输出）与宿主的读取接口（§4.2）。各语言 SDK 不把 `site` 交给调用方的代码。
- 补不上的写进边界：按引用传过来的函数（只写行）；不在任何调用里的错误（定时器、没 await 的 promise，见 §4.7）；同一运行时里的调用（本来就是语言自己的错误和堆栈）。
- 各语言解码时都容忍多出来的字段（已核实：Node、Python、Go、Swift、Kotlin 都按键读取），不升协议版本，旧的一端照常工作。

### 4.2 结构一路保持

- loader 的 `failed()`（两处）改为把原错误包进 `ErrorKind::PluginFailed`，不再转字符串。内核的行状态本来就保存 `Arc<ErrorKind>`，结构由此保住。
- `ErrorKind::PluginFailed` 显示时去掉 `plugin failed: ` 前缀。
- 宿主读结构：rutis-bridge 提供 `rutis_bridge::thrown(&rutis::Error) -> Option<&session::Error>`（找到包着的跨语言错误），`session::Error` 已有 `code()`、`data()`，补 `stack()` 与 `site()`。不新增内核类型。
- loader 的 `Failure { id, error: String }`、`ReconcileReport` 不变（它们要比较相等）；要结构的从行的状态（`EntryInfo` 的快照错误）读。

### 4.3 错误码

插件自己抛出的错误带什么 `code` 就是什么。rutis 自己产生的 `BindingError` 用下面 6 个码，细节放进已有的 `data`：

| code | 什么时候 | `data` |
| --- | --- | --- |
| `not-declared` | 用了没写进 `inject` 的服务 | `{service}` |
| `unavailable` | 服务现在没有提供者 | `{service, reason}`（`Loader::waits` 的原因） |
| `no-method` | 方法不存在 | `{service, method, methods}` |
| `bad-argument` | 参数个数或类型不对 | `{service, method, argument, expected}` |
| `bad-value` | 值传不过去，或消息超过上限 | `{argument?, part, limit?}` |
| `peer-gone` | 调用进行中对面的运行时退出或断开 | `{runtime, reason}` |

`SyncWaitCycle`、`AggregateError` 已有自己的类型名，不另设码。码给程序判断，不出现在给人看的那一行文字里。表在实施时可以加行，不改已有的行。

Rust 一侧（实施时定，2026-10-26）：`rutis_bridge::session::Error` 新增分支 `Binding { code: Option<&'static str>, message, data }`，rutis 拒绝一次调用时用它：方法不存在、参数不对、服务不在带码，协议层面的拒绝（一端没按约定办事）不带码。`Error::Value` 只表示值传不过去。发到线上时 `Binding` 与 `Value` 都是带码的 `BindingError`（`Value` 为 `bad-value`），会话中途结束的 `Transport` 为 `peer-gone`；`Error::code()` 在本端给出同样的码。对 `session::Error` 逐个分支匹配的 Rust 宿主要加上这一支（不兼容，写进迁移指南）。

### 4.4 位置与跨语言

发出的位置：

| 语言 | 发出的 `stack` |
| --- | --- |
| TypeScript（Node、Bun） | 真实堆栈（已有） |
| Python | `traceback`（已有） |
| Go | 普通错误：返回它的方法声明的文件:行号（`Type.Method(i).Func.Pointer()` 取，不是反射的桩函数）；panic：完整堆栈（已有） |
| Swift | 抛出它的方法声明的文件:行号（`RutisService` 宏展开时记下）；Swift 的错误没有堆栈，只能到方法，这是边界 |
| Kotlin | JVM 真实堆栈（现在只有"名字: 消息"） |
| Rust | 返回错误的方法（`#[rutis::service]` 生成的代码记下）；panic 在 `RUST_BACKTRACE` 打开时带 `std::backtrace` |

收到对面的错误时：

- 交给调用方代码的错误对象：类型名、消息、错误码、`data`，与现在相同，不新加字段。
- 对面的错误图在 SDK 内部保留（Python、Go、Kotlin 现在已经这样），调用方再把它抛出时原样带上。
- Node、Bun 不再用对面的堆栈覆盖 `error.stack`：`error.stack` 是调用方自己的堆栈。
- rutis 打印时把两边都显示：对面出错的那一行与位置，下面接着调用方的位置。

另外修 #677 点名的一处：Go 的 `Parallel` 在真实宿主里返回 `*rutis.AggregateError`（`errors.As` 成立，`Data` 不为空），现在只在测试工具里对。

### 4.5 文字

规则：只用开发者写过的东西说话。

- 不出现：rutis、Cordis、Rust（插件是 Rust 写的除外）、binding、session、WireValue 与 serde 的报错原文、内部 id（`row:23.2`、`PluginId(3)`、带 NUL 的标签）、内部插件（`runtime-rows`、`rutis-bridge/link#py`）、`<root>` / `<leaf>`、宿主私有的 Rust 类型路径、SDK 的内部类型（`HostProxy`、`RemoteFuture`、`e.graph`、`UNDEFINED`）、`rollback failed`、`cannot resolve row … again`、`ThrownValue`。
- 行写行名，运行时写它的名字（`py`），宿主写"宿主"。
- 按 #677 正文与"并入 #680"一节列出的每一条改写，各有测试锁住新文字；其中几条要说出原因：默认导出不是 `definePlugin` 的结果时直说；`entry file is gone` 写出是哪个文件；抛出字符串或对象时显示它本身（`"just a string"`、`{code: 3}`），不显示 `ThrownValue`；没处理的 rejection 只打印插件自己的帧。
- Rust 宏与 trait 的编译错误用 `#[diagnostic::on_unimplemented]` 说人话。

### 4.6 格式化不能出错

打印错误、读 `site`、接两边位置的代码，本身不能变成新的故障：

1. **不失败**：字段缺失、类型不对、错误图里有循环引用或断掉的引用，都退回"名字: 消息"；连名字和消息都没有时写"一个无法读取的错误"。不 panic，不抛出，原错误照常传给调用方。
2. **有上限**：单个错误的打印最多 64 行堆栈、消息最多 4 KiB，超出部分截掉并注明"已截断"；嵌套的 `cause` / `errors` 最多 8 层。
3. **去掉控制字符**：打印前把 NUL 等控制字符换成可见的转义（0.17.0 的内部标签带 NUL，`grep` 会把整个日志当成二进制）。
4. **新旧两端都能读**：没有 `site` 的错误照常打印，只是不写"是谁、在做什么"；旧的一端收到 `site` 时忽略。

这几条由测试锁住：

| 测试 | 锁住什么 |
| --- | --- |
| 逐字断言 | §八 每种故障、每种语言的输出（位置只比文件名与方法名，不比行号）；先例是 `rutis-host/tests/errors.rs` |
| 禁用词 | 跑完故障清单，输出里不出现 §4.5 的词 |
| 模糊测试 | 新增 fuzz 目标：任意字节作为错误图，解码与打印不 panic、一定有输出、遵守上限（现有的 fuzz 只覆盖消息帧） |
| 跨版本 | 发布门的跨版本相遇加两条：新的一端发 `site` 给旧的一端，旧的一端发没有 `site` 的错误给新的一端 |

以上登记进 `docs/guarantees.md`（新条目：错误的格式化、错误的去向、禁用词），每条写明验证它的测试。

### 4.7 错误记在出错的那一行

- 被调用的方法出错：调用方看到"调用 X 失败"，`site` 是被调用的那一行。
- 配置不符合 schema：记在配置写错的那一行，不记到调用方（#669 让 schema 生效，它的错误按这里的规则归属）。
- 没处理的 rejection / 异常（#688）。运行时进程由几个插件共用，进程怎样处理这类错误是平台的事：
  - Node：在 `dev` 下用 `async_hooks` 记下 promise 属于哪一行，这一行失败；`run` 下不开（它给每个 promise 加开销），写"运行时 node 里有一个没处理的错误，来源不明"。
  - Python：用任务的上下文追到行（3.12 起 `Task.get_context()`，3.10、3.11 读 `_context`）。
  - Bun：追不到，一律"来源不明"。
  - 追不到时进程都不退出。插件自己调用 `process.exit` / `os._exit` 时照常退出，报告写明是哪一行调用的。
  - 这改了"边界规则 8"（未捕获的错误结束运行时进程）与 Bun `main.ts` 的处理；`docs/guarantees.md` 与插件 API 指南的"同一运行时里的插件共用一个进程"一节同步改写。

## 五、当前状态

### 5.1 一份快照

`Loader::status() -> Status`（各部分在一串短锁里依次读，不保证原子，和 `diagnostics()` 一样）：

```text
Status {
  rows:     [{ id, instance?, state, reason? }]   // pending | starting | running | failed | stopped | disabled
                                                  // reason: 失败的错误，或等待原因（Loader::waits）
  services: [{ name, provider }]                  // provider: 行名 | "宿主"
  runtimes: [{ name, state, reason?, pid? }]      // running | starting | down；pid 只有本机运行时有
  calls:    [{ from, to, service, method, elapsed }]
}
```

- 只有名字，没有内部 id，不列内部插件。`Status` 实现 `Display`（给人看，一项一行）。
- 行、等待原因来自 loader（已有）；服务来自 `diagnostics()`（已有）；运行时来自 `RuntimeHandle`（已有）。
- 运行时退出时，它的行立即显示为等运行时（读 `RuntimeHandle` 的状态，不等内核几秒后才变的 `Pending`；#681）。
- 快照给看状态的人与宿主，插件拿不到。

### 5.2 进行中的调用

- 在 `RowService::invoke` 登记：被调用的行与服务是它自己的，方法是参数，调用方用 `caller_row()`（不在行里调用的记为"宿主"）。异步调用登记包住返回的 future，结束时去掉。`RowService` 现在没存服务名，要补上。只记录，不改变调用。
- 看不到的写进边界：同一运行时里的调用；按引用传过来的函数（只能看到"宿主 → 行"，没有方法名）；进程内的 Rust 插件调用会记为"宿主"。

### 5.3 谁读它

| 地方 | 怎样读 |
| --- | --- |
| `rutis-host run` / `dev` | 状态行照旧打印变化；等待行用 `Loader::waits`；关闭时卡住的那一句改用快照（只数行，写明在等谁） |
| `rutis-host status` | 新命令，打印一次快照。`run` / `dev` 开一个只读的本地 socket（权限 0600），只答 `status` 一个请求；socket 放在系统的运行时目录（`$XDG_RUNTIME_DIR`，没有时用临时目录），文件名带配置文件路径的散列，不往项目目录里写文件；`status` 用同一个配置路径（默认当前目录的 `rutis.json`，或 `--config`）找到它。Windows 上 0.18 不提供，写进边界 |
| Rust 嵌入 | `Loader::status()`；`diagnostics()` 加可读的 `Display`（用名字，默认不列内部插件）；`RuntimeState` 实现 `Debug` |

`rutis/*` 事件不变（只报变化）。rutis-dev 的通道不复用（它不发布，且带改动命令）。

## 六、时限

### 6.1 调用：不设时限

一个调用该等多久由调用方的业务决定，rutis 不设调用时限，也不替双方取消调用。调用方要限时，用自己语言的写法，取消会传到对面（已有）：

| 语言 | 写法 |
| --- | --- |
| Python | `asyncio.wait_for(...)` |
| Go | `context.WithTimeout` |
| Swift | 在 `Task` 里限时后取消 |
| Kotlin | `withTimeout { ... }` |
| Rust | `tokio::time::timeout(...)`（丢弃 future 即取消） |
| TypeScript | `Promise.race` 加定时器；对面不会被取消，与原生 JS 相同（promise 本身不能取消） |

插件 API 指南写明这张表。rutis 做的只是让人看得到：状态快照里列出进行中的调用与已经等了多久（§5）。宿主想限制第三方插件的资源，属于 0.19 的 #683（宿主的选项，默认关）。

### 6.2 关闭：时限归宿主

| | 现在 | 0.18 |
| --- | --- | --- |
| 内核 `shutdown()` | 不限时 | 不变：等多久由嵌入 rutis 的程序决定 |
| 内核 `shutdown_with_timeout(limit)` | 到时报根（`fiber PluginId(1) … still Unloading`） | 到时写明卡住的插件与清理：`ctx.effect` 登记的清理写出登记处的文件:行号（`#[track_caller]`），其他清理（`provide` 的撤销、loader 行的清理）写插件或行名 |
| rutis-host | 10 秒（`RUTIS_SHUTDOWN_TIMEOUT` / `--shutdown-timeout`） | 不变；卡住的那一句改用快照 |

rust-plugin.md 现在写"清理有时限"，改为如实写：时限由宿主决定，rutis-host 默认 10 秒，Rust 嵌入用 `shutdown_with_timeout`。

### 6.3 握手

本机 30 秒、网络 10 秒，不变，只改文字。这是平台自己的连接过程。

### 6.4 同步阻塞提示

- Node、Bun、Python 运行时被一个同步方法占住 5 秒（固定）时，运行时打印一次："运行时 py 被 weather.tomorrow 占住 5 秒，这期间其他行都在等；它是同步方法（`def`），改成 `async def` 就不会占住"。
- 只打印，不打断方法。Go、Swift、Kotlin 的同步方法不占住整个运行时，不需要。

## 七、输出

- **插件的输出带前缀**（终端是宿主的）：
  - Node、Bun、Python（多行共用一个进程）：运行时按当前在跑的行加 `[weather] `。Node、Bun 在 `apply`、服务方法、监听器、按引用导出的函数这几个入口套上异步上下文；Python 用 contextvars。追不到的（Python 的线程、Node 的子进程与直接写 fd 的原生模块）加运行时名。
  - Go、Swift、Kotlin（一个进程一个插件）：宿主读输出管道时加运行时名，不改 SDK。
  - `dev` 默认开，`run` 默认关（生产环境里插件的输出原样不动）；`--prefix-output` / `--no-prefix-output` 切换。
- **Python 的 `logging` 不改**：默认不显示 INFO 是 Python 自己的规则。指南写明：要看 INFO，在插件里配置日志（`logging.basicConfig(level=logging.INFO)`）。
- **Rust 嵌入默认看得到**：默认的错误汇集除了监听器的错误，再写出插件或行加载失败、运行时退出（等待超过 10 秒的提示 0.17.2 已有）；都写明插件或行的名字。输出不带 `[rutis]` 之类的平台名字，以插件或行名开头。`CtxHost` 已有的错误汇集接口可以换掉或关掉它。
- **按名字提供不取决于是否链接了 rutis-bridge**（#681）：标了 `#[rutis::service]` 的类型被提供、而名字登记没有链接进来时，启动时写一次"服务 calendar 按类型提供了，但没有按名字登记：依赖 rutis-bridge 才能按名字使用和检查冲突"。
- **状态行写到标准错误**，插件的输出按它原来写的流。

## 八、开发者会看到什么

### 8.1 14 种故障（#663 调试评估）

TypeScript 为例；其他语言同样的内容，位置写法各自的。这些是 rutis 打印的输出。

| # | 故障 | 输出 |
| --- | --- | --- |
| 1 | 依赖的服务名写错 | `weather: waiting for lmm (no row provides it; did you mean llm?)`（0.17.2） |
| 2 | 服务方法抛错，跨语言 | `chat: call to weather.tomorrow failed: KeyError: 'region'`，下面两段位置：`weather at weather.py:31 in tomorrow`、`called from chat at src/chat.ts:14:36` |
| 3 | 方法不存在 / 参数错 | `chat: weather has no method tomorow (methods: today, tomorrow)`；`chat: weather.tomorrow: argument 1 should be a number, got "x"` |
| 4 | `apply` 里出错 | `weather: failed in apply: KeyError: 'region'` + `weather.py:12 in apply` |
| 5 | 导入错误 | `weather: cannot load src/util.ts: SyntaxError … (src/util.ts:3:7)`（写出错的子模块） |
| 6 | 值传不过去 | `chat: weather.tomorrow: argument 0 cannot be sent: a sparse array (index 1 is empty)`；运行时不退出（#675） |
| 7 | 监听器出错 | `weather: listener of weather/asked failed: …` + 位置；`parallel` 的调用方看到 `2 listeners of weather/asked failed`，下面逐个列出行与位置 |
| 8 | 调用不返回 | `rutis-host status`：`chat → weather.tomorrow  42s` |
| 9 | 互相调用 / 互相依赖 | `chat: weather.today would wait on itself: chat → weather → chat (synchronous calls)`；互相 `inject`：`check` 报错（§九） |
| 10 | 运行时中途退出 | `runtime py: down (exited with status 1)`；`weather: waiting for runtime py (down: exited with status 1)`；进行中的调用 `chat: weather.tomorrow failed: runtime py exited during the call` |
| 11 | 下断点、看日志 | 输出带 `[weather]` 前缀；调试器的接法见 #679 |
| 12 | 配置写错 | `weather: config: units should be "metric" or "imperial", got "kelvin"`（记在 weather） |
| 13 | 两个插件提供同名服务 | `check`：`rows a and b both provide forecast in the same scope` 并以 1 退出；运行时：`b: forecast is already provided by a` |
| 14 | 查看当前状态 | `rutis-host status`：每行状态与原因、每个服务的提供者、进行中的调用 |

### 8.2 宿主持有者评估里的（#663）

| 故障 | 输出 |
| --- | --- |
| 宿主拿到插件的错误 | `rutis_bridge::thrown(&err)` 给出 name、code、message、stack、site，宿主按 code 改写 |
| 插件抛出字符串 / 对象 | `wordcount: call to tools.run failed: "need 5 words"` |
| 一个插件没处理的 rejection | `dev`：只有那一行失败；`run`：`runtime node: an unhandled error from an unknown row: …`，其他行继续 |
| 默认导出不是 `definePlugin` 的结果 | `wordcount: the default export of src/index.ts is not a plugin made with definePlugin` |
| Python 的 `def` 方法阻塞 | 阻塞提示（§6.4） |

## 九、`check`

#676 的后半：互相 `inject` 的环、同一作用域里几行提供同一个服务，0.17.2 是警告，0.18 改为报错（以 1 退出），信息写明是哪几行、怎么改。

## 十、跨语言要知道的四件事

| 事 | 在哪里提示 |
| --- | --- |
| 值是复制的 | 插件 API 指南；`bad-value` 的信息说"按值复制，对象的方法不会过去"（#687 之后按引用的那部分除外） |
| 对面可能不在 | `unavailable`、`peer-gone` 的信息写明为什么不在 |
| 调试按进程各自进行 | #679 的调试指南；状态快照里本机运行时的 pid |
| 跨进程的事件不保证先后 | 事件指南 |

## 十一、兼容与迁移

写进《迁移到 0.18》（两份）：

- **Rust 宿主**：`ErrorKind::PluginFailed` 显示时没有 `plugin failed: ` 前缀；插件的错误不再是字符串，用 `rutis_bridge::thrown()` 读结构。按文字匹配的代码要改为按错误码。loader 的类型（`Failure`、`ReconcileReport`）不变。`session::Error` 新增 `Binding` 分支（§4.3）。
- **Rust 嵌入（行为变化）**：加载失败、运行时退出默认写到标准错误（可以关掉）；默认输出不再以 `[rutis]` 开头。`shutdown()` 不变。
- **Node、Bun**：收到的跨语言错误，`error.stack` 改为调用方自己的堆栈（以前是对面的）。
- **所有语言**：错误类型名与字段不变。出错文字会变。
- **Node、Bun、Python 运行时**：没处理的错误不再让进程退出（边界规则 8 改写）。
- **`check`**：环和同名提供以 1 退出。
- **协议**：错误节点多一个可选字段 `site`，旧的一端忽略；不升协议版本，跨版本相遇照常。

## 十二、实施

| Issue | 内容 |
| --- | --- |
| #677 | §四（4.1–4.6）：`site`、错误码、各语言的位置与收到时的处理、loader 不转字符串、`thrown()`、Go `Parallel`、文字改写、格式化不能出错及其测试与保证登记、宏的提示 |
| #678 | §五、§六、§七：`Loader::status()`、进行中的调用、`rutis-host status`、调用方限时的指南、`shutdown_with_timeout` 写明卡住的插件、阻塞提示、输出前缀、Rust 嵌入默认输出、按名字提供的启动提示 |
| #688 | §4.7 没处理的错误归属、边界规则 8 |
| #676 后半 | §九 |

各 issue 的完成条件以 §八 为准：每种语言跑一遍 14 种故障与宿主评估的故障，输出与表一致（位置写法按语言）。
