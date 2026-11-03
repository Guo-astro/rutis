# 出错信息：谁出的错、同名服务、互相等待、在途调用、内部帧

[English](design-error-messages-2026-11-03.en.md)

状态：草稿。日期：2026-11-03。基准：`main` `d2a8d069`（0.20.1）。
Issue：[#937](https://github.com/arcships/rutis/issues/937)，属于 [#663](https://github.com/arcships/rutis/issues/663)（D2、D3、D5、O6）。
依据：[#663](https://github.com/arcships/rutis/issues/663) 的 0.19.0 评估（14 种故障、四种语言全写清楚的有 34 / 56）；[诊断](design-diagnostics-2026-10-26.md) §4.1（`site`）、§4.4（`rethrown`）、§4.5（用词）、§4.6（打印不失败）、§5.2（在途调用与写明的边界）；[声明集合](design-declared-services-2026-10-29.md)（`not-declared` 的文字）；[对象引用](design-object-refs-2026-10-27.md) §四（同一进程里拿到的是对象本身）。

范围外：

- 平台怎样归属**没人处理**的错误（#688，诊断设计 §4.7）：本稿只改那些行失败时打印出来的字。
- 调用的时限、关闭的时限（诊断设计 §6）：本稿不改时限，只改到点之后写的那几行。
- 远程（跨机器）节点的错误文字：与本地运行时同一套规则，随各运行时一起改，本稿不单独写。
- 平台改写宿主名字的接口：输出里不出现平台名字，就不需要它。
- 事件订阅、配置校验、值类型转换的文字：各自的设计里改。

## 一、现在怎样 / 要解决什么

按 issue 的五组现象。每条给出今天的输出和产生它的代码。

### 1.1 同一个运行时里，错误标成调用方

> 同一个运行时里，被调用的服务出错，错误标成调用方：llm 抛错，看到 `in weather:`，没有 `in llm:` / `called from:`（Go、TS）。跨进程时两段都有。

三段代码凑成这个结果：

1. **`site` 由宿主填，只在跨出边界时填。** 宿主把一次调用交给运行时行的 `RowService` 时，按被调用的那一行与 `service.method` 给错误补 `site`（`crates/rutis-bridge/src/runtime/rows.rs:52`，`:119`、`:144` 用它）。一次调用里，宿主只知道**它交给的那一行**（weather）。
2. **同一进程里的调用不经过 rutis。** 一个行 `ctx.use` 拿到的服务，提供者在同一个进程时就是对象本身：Python 的 `Runtime.lookup` 直接返回 `provided.value`（`python/rutis/rutis/runner.py:412`），Node 的叶适配层返回 Cordis 的服务对象（`node/rutis-runtime/src/leaf.mjs:98`），Go 在目标放得下时把对象本身给出去（`go/rutis/internal/runtime/runtime.go:1287`）。weather 调 llm 是直接的方法调用，所以 llm 这一跳既没有 `site`，也没有进 `rethrown`。
3. **已有的 `site` 不被覆盖**（`crates/rutis-bridge/src/session/mod.rs:253`）。这条本身是对的（最早出错的那一跳最要紧），但现在最早的那一跳没人填，于是只剩宿主填的 weather。

### 1.2 同名服务：不说被谁占了

> Python 行提供一个已被 Rust 行提供的名字，报 `weather is already provided by the host`（其实是一行）；两个 Rust 行同名，报 `service "weather::Weather in scope <default>" already registered in scope`，不说被谁占了；同名而另一个只有别的方法时，`rutis-host call weather.today` 说 `weather has no method today (methods: ask)`，把人引向方法名。

三个来源：

- **运行时的行**：宿主回答"这个键已经被占了"时，用 `HostDispatch::provider()` 说持有者，只认 `row …` 与 `peer …` 两种写法，其他一律写成 `the host`（`crates/rutis-bridge/src/runtime/rows.rs:296`）。同一个仓库里早就有做对了的那一半：宿主自己的服务被占时按 `diagnostics().bindings` 找持有者的行（`crates/rutis-bridge/src/session/plugin/services.rs:216`）。`#[rutis::service]` 类型的 `provider()` 给的是类型名（`crates/rutis-bridge/src/marked.rs:110`），于是"一行提供的名字"被说成"宿主提供的"。而那一行是有的：`crates/rutis-bridge/src/services.rs:154` 已经算出了 `plugin.row`。
- **两个 Rust 行**：内核的同名错误只带键（`crates/rutis/src/registry.rs:134`、`crates/rutis/src/ctx.rs:694`），显示成 `service {0:?} already registered in scope`（`crates/rutis/src/error.rs:144`），里面是 `TypeKey::describe()` 与作用域名，没有谁。
- **名字被别人占着、方法不一样**：`no-method` 只说这个方法不在（`crates/rutis-bridge/src/runtime/rows.rs:149`），不说这个**名字**现在是谁的。开发者看到的是"方法名写错了"。

### 1.3 只把 `ctx.use("lmm")` 写错时，让人把错字加进去

> 只把 `ctx.use("lmm")` 写错（inject 写对）时，提示 `add 'lmm' to its inject`，让人把错字加进去，没有 did you mean。

六种语言同一句（`python/rutis/rutis/runner.py:98`、`node/rutis-runtime/src/errors.mjs:220`、`bun/rutis-bun/src/errors.ts:220`、`go/rutis/internal/runtime/runtime.go:988` 的 `Inject`、`swift/rutis/Sources/Rutis/API/Runtime.swift:1075`、`kotlin/rutis/rutis/src/shared/kotlin/arcships/rutis/Refusals.kt:144`）。`check` 一侧早就会说 did you mean（`crates/rutis-host/src/main.rs:976`），运行时一侧不会。

### 1.4 `check` 的告警列出无关的行

> `check` 的告警 `nothing declares lmm; it may be provided at runtime by llm` 列出所有可能在运行时提供服务的行，和这个名字无关。

`undeclared_line`（`crates/rutis-host/src/main.rs:953`）拿到的是 `Supply::Announced(rows)`，也就是**所有不声明 `provides` 的行**（`:831`），不是"可能提供这个名字"的行。名字对得上时它照样把清单列在前面（`:907` 的 did you mean 接在后面）。

### 1.5 两个 Rust 行互相依赖

> 两个 Rust 行互相依赖：两行都写 `waiting for … (no row provides it)`，没有「互相依赖」，而且这句不对（另一行会提供）。Rust 内置行不声明会提供什么。

等待原因按**声明**找提供者（`crates/rutis-loader/src/loader/waits.rs:344`：`fact.provides`）。`fact.provides` 来自行的 `provides` 声明：运行时行来自插件清单，内置行没有（`crates/rutis-loader/src/resolver.rs:310` 的 `declares: None`）。于是互相等待的两行走不到 `Cycle`（`waits.rs:365`），落进 `NoProvider`，说的是 `no row provides it`——而另一行确实会提供，只是没有声明。`Builtins::declare`（`crates/rutis-loader/src/resolver.rs:319`）本来就能让内置行声明，只是没人用。

### 1.6 运行时退出后，`status` 还要过约半秒才说

> 运行时 down 时，`diagnostics` 写 `pyhello: waiting for py (service missing)`；运行时退出后立刻读 `status` 仍是 `running`，约 0.5 秒后才对。

- **用词**：内核 `RuntimeDiagnostics` 的 `Display` 把依赖状态原样打出来（`crates/rutis/src/diagnostics.rs:22`、`:80`、`:198`、`:200`），`(service missing)`、`provider inactive (Pending)` 都是内部说法；`rutis-host` 的 `status` 早就会说 `waiting for runtime py (down: exited with status 1)`（`crates/rutis-loader/src/loader/status.rs:432`）。同一个宿主，两处说法不同。
- **滞后**：`status` 的运行时部分只看 `RuntimeHandle::state()`（`crates/rutis-loader/src/loader/status.rs:278`），而那个 watch 要等管道那边的退出被发现才变（`crates/rutis-bridge/src/runtime/state.rs:235`）。判定"已经结束"的材料在 `ended_status` 里（`state.rs:180`），只是没被 `status` 用来即时判断。

### 1.7 等运行时的提示把运行时说成服务

> 等运行时的提示把运行时说成服务：`hang: still waiting after 10s for hang: it starts once that is provided`。

`StillWaiting` 只拿到键的名字（`crates/rutis/src/error.rs:214`），名字由宿主登记的描述者给（`crates/rutis/src/host.rs:116` 的 `name_keys_with`）。运行时那一行的会话键名是运行时行的 id，于是它和普通服务一样被说成"等这个服务"，尾巴还会补一句"它被提供之后就会启动"。同一个键在 `status` 里说得对，在这里说不出来。

### 1.8 在途调用看不到对 Rust 行的调用

> `status` 的 `calls` 看不到同一运行时里的调用（文档写了），也看不到对宿主进程里 Rust 行的调用（文档没写）：宿主 → Python → Rust weather → Rust llm（卡住）时只有 `host → pyhello.forecast 4s`。

在途调用登记在运行时进程上（`crates/rutis-bridge/src/runtime/process.rs:1414`），只在宿主把一次调用交给**运行时行**时登记（`crates/rutis-bridge/src/runtime/rows.rs:87`）。一次调用交给宿主自己的服务（`#[rutis::service]` 的行、宿主自己注册的服务）时，路由在 `crates/rutis-bridge/src/session/services.rs`，那里没有登记。同一运行时里两行之间的调用更不经过宿主（§1.1 第 2 点）。

### 1.9 内部帧出现在给开发者看的堆栈里

> Python 的错误堆栈前面有 5～15 帧 rutis 内部帧（`_unhandled.py`、`_output.py`、`runner.py`、`peer.py`），插件自己那一行在最后；Go 的 panic 约 40 行，大半是 SDK 与 reflect；Rust 宿主拿到的跨语言错误 `Debug` 有几千字符，大半是这些帧。

- Python：错误离开进程时用 `traceback.format_exception` 整段（`python/rutis/rutis/peer.py:292`），而"只留插件自己的帧"的过滤函数已经在（`python/rutis/rutis/runner.py:1341` 的 `_plugin_frame`，`_unhandled.py:109` 的 `report_text` 用了它）。
- Go：panic 用 `debug.Stack()` 整段（`go/rutis/internal/peer/peer.go:1712`）。
- Rust：`session::Error` 的 `Debug` 是派生的（`crates/rutis-bridge/src/session/mod.rs:35`），`Remote` 带着整个错误图的 JSON。
- Node、Bun 已经过滤（`node/rutis-runtime/src/errors.mjs:22`），这一条只是让其他三家与它们一致。

### 1.10 停止超时与状态词

> 停止超时的输出里有 `still stopping: rutis-loader`；状态词 `unloading`、`inactive` 文档里没有。

`still_running` 把不是行的插件也列出来，用它们在 rutis 里的名字（`crates/rutis-host/src/host.rs:853`），于是出现 `rutis-loader`。状态词：`unloading` 是 `FiberState` 兜底打印出来的（`crates/rutis-host/src/status.rs:41`），`inactive` 是 `EntryStatus::Inactive` 的直译（`crates/rutis-host/src/status.rs:21`），指南里都没有这两行。

### 1.11 关闭时还在跑的监听器，原来的错误丢了

> 关闭时还在跑的 Python 监听器出错，报成 `the connection closed`，原来的错误丢了。

会话关闭后，还在等的回复拿到的是 `Error::Transport("the connection closed")`（`crates/rutis-bridge/src/session/rpc.rs:941`，事件侧 `crates/rutis-bridge/src/events/hub.rs:1189`）。监听器真正的错误在对面，已经没有人接了。

## 二、决策

| # | 决策 | 为什么；放弃的做法 |
| --- | --- | --- |
| D1 | 错误在同一个运行时里跨行时，**运行时**按栈填最早出错的那一跳：`site` 写它（行名，能说就说 `service.method`），往外的每一跳各写一条 `rethrown`；`stack` 只留最早那一行的帧 | 最早出错的一跳是开发者要找的那一行（诊断设计 §4.1 的规则），只是现在没人填。栈本来就在运行时手里，且已经有一份"哪个文件属于哪一行"的映射（Python 的 `charged_row`，`runner.py:649`）。放弃"让 `ctx.use` 返回代理"：那会把同一进程的语义改成跨进程，插件失去对象本身（数据字段、`===`、类型断言），与[对象引用](design-object-refs-2026-10-27.md) §四 相反 |
| D2 | 服务名与方法名只在**rutis 递过去的那一跳**写；只能从栈看出来的那一跳只写行名 | 递过去时 rutis 手里就是 `service.method`（Go 的 `*Service`、函数字段结构体，`runtime.go:1216`；宿主转进来的调用，`rows.rs:52`）。从栈里推方法名是猜，猜错比不说更糟。只写行名时，打印出来的头一行是错误自己的文字，`in <行>:` 下面就是那一行的帧，位置照样清楚 |
| D3 | 同名服务一句话：`<名字> is already provided by <持有者>`，持有者是行名，或 `the host`（宿主自己提供的）；后面接一句怎么改（`rename one of them` / `rename it`）。运行时的行、两个 Rust 行、测试工具都用这一句 | 一个服务名只有一个提供者，出错的一定是后提供的那个，它需要知道"是谁、怎么办"。三方各写一句话，开发者得记三句。放弃"由宿主补一句"：宿主补的话到不了行自己的状态行 |
| D4 | `not-declared` 找到近名时改说 `did you mean llm?`，找不到近名才说加到 `inject` | 让人把错字加进去是错的建议。#676 的原意是"说该声明什么"，近名时说错字才是该说的。候选名字用这一行已经有的（本进程的、租来的、插件与行声明的名字），规则沿用 `rutis::config::closest`（`crates/rutis/src/config/mod.rs:267`：3～6 个字符容 1 处编辑，更长的容 2 处） |
| D5 | `check` 的同一条告警不再列行：`nothing declares lmm; did you mean llm?`，没有近名时说"有一行跑起来之后可能提供它" | `check` 不跑插件，它不知道谁会提供；列出来的行与这个名字无关，是噪音。Cordis 行之间的解析仍然写一句（那是真的） |
| D6 | 内置 Rust 行声明它提供什么（`Builtins::declare`），行的提供者表因此与运行时行一样完整；仍然没人声明的名字，等待原因写 `nothing provides it yet`，不写 `no row provides it` | "没有行提供它"是一句关于**提供**的断言，而 rutis 那时只知道**声明**。声明之后，两个内置行互相等待能走到 `Cycle`，`check` 也能看见它们。放弃"从栈或运行时猜谁提供"：猜错就是另一句错话 |
| D7 | `diagnostics()` 的 `Display` 与 `status` 用同一套等待说法：宿主可以给根登记一个**描述者**（`describe_keys_with`），把键写成 `runtime py (down: exited with status 1)` 或服务名加原因；`StillWaiting` 也用它，去掉尾巴 `: it starts once that is provided` | 同一个键在两处说法不同，是因为一处只有名字、另一处有 loader 的 `waits`。让内核问宿主（已经有 `name_keys_with` 的先例）比让内核自己懂运行时简单。尾巴那句话在等运行时的时候是错的 |
| D8 | `status` 的运行时状态以进程为准：`Ready` 的运行时如果连接已经关了、或退出码已经有了，立刻按 `ended_status` 显示 down | 状态快照是用来"现在看"的（O6）。等管道那边的通知（约 0.5 秒）会让紧跟着的 `status` 说假话。放弃"加快通知"：那要改进程回收的时机，且仍然有窗口 |
| D9 | 在途调用：**凡是经过宿主的**都登记，包括交给宿主自己的服务与 `#[rutis::service]` 行的那一次；同一运行时里两行之间的调用仍不登记，写进指南与 `status` 一节 | 那句调用真的不经过 rutis（§1.1 第 2 点），要登记就得先改 `ctx.use` 的返回值（D1 已放弃）。跨宿主的那一半是 issue 点名的（`host → pyhello.forecast` 有、`pyhello → weather.forecast` 没有），并且能说清"谁在等谁" |
| D10 | 发出去的 `stack` 只含出错那一行自己的帧；一帧都没有时只留最里面那一帧。Rust 的 `session::Error` 不再派生 `Debug`，`Remote` 只打印名字、消息、`site`、帧数与错误码 | 帧是给开发者找自己的代码用的，SDK 与 `reflect`、`runtime` 的帧只是长度（D5）。错误图留给读接口（`thrown()`） |
| D11 | 停止超时只写开发者写过的名字：行写行名，运行时写 `runtime <名字>`，只有 rutis 自己的插件卡住时写一次 `the host`；状态词 `unloading` 改说 `stopping`，`inactive` 改说 `not running (its group is not running)`，两个词都写进指南 | `rutis-loader` 是 rutis 的内部插件名（诊断设计 §4.5 的禁用词）。状态词是开发者每天看的字，要么写进指南，要么换成一眼能懂的话 |
| D12 | 关闭时还在跑的监听器出错：运行时按行把它写出来（`runtime py: row weather: listener of weather/asked failed: …`），宿主不再用 `the connection closed` 顶替它 | 原来的错误只有对面有，所以只能由对面说；会话关了以后，对面写到自己的标准错误（`run` 下宿主照旧转发运行时的输出） |
| D13 | 新增一个错误码 `already-provided`（`data {service, provider}`）；`no-method` 的 `data` 加 `provider`（提供者是行时是行名，宿主自己提供时是 `null`）。其余文字不改码 | 宿主按码判断，不按文字（诊断设计 §4.3）。同名冲突是开发者要处理的配置错误，值得一个码 |

## 三、规则

### 3.1 归属：一个运行时里谁出的错

1. **什么时候标**：错误离开一行时标一次——`dispatch` 的回复、监听器的回复、`apply` 失败、清理失败、没人处理的错误。插件自己接住、处理的错误不标。
2. **怎么标**（每个运行时一样）：
   1. 从栈的最里面往外走，第一个属于**另一个行**的帧是"最早出错的那一行"（`R`）。判断用已有的映射：行的入口模块文件 → 行的 id（Python `runner.py:649` 的 `charged_row` 就是这么做的；Node、Bun、Go 用它们 `rows.load` 时拿到的入口）。
   2. 找到了：错误的根节点写 `site = {row: R, doing: "call"}`；rutis 递过来的那一跳另外写 `service`、`method`（D2）。
   3. `stack` 只保留 `R` 的帧（从最里面数到 `R` 的第一帧为止）；往外的每一段，按行的变化切一刀，各写一条 `rethrown`（最多 8 条，`crates/rutis-bridge/src/exports.rs:79`）。
   4. 没找到（错误就在这一行里）：什么都不改。
3. **不覆盖**：宿主填 `site` 时不覆盖已有的（`crates/rutis-bridge/src/session/mod.rs:253`），所以运行时先标的那一跳保留。
4. **看得见的只有打印与宿主**：`site`、`rethrown` 与拆分后的 `stack` 一样不交给插件代码（与诊断设计 §4.1 相同）。运行时在进程里记它们的方式不进入各语言 SDK 的公开类型。
5. **语言差异（写进指南）**：
   - Go 用 `*Service`、函数字段结构体的目标调用时，rutis 自己递过去，因此 `service.method` 一定有；把服务绑成接口（`runtime.go:1287` 把对象本身给出去）时没有栈可读，这一跳归不到提供者，与今天相同。
   - Python、Node、Bun 的对象调用都没有服务名与方法名（D2）。
   - Rust 行之间已经在内核里，`site` 由宿主填，不变。

### 3.2 同名服务

1. 一句话，六种语言与 Rust 一致：`<名字> is already provided by <持有者>`。
   - 持有者是一行：行名（`a`）。
   - 持有者是宿主自己：`the host`。
   - 后面接怎么改：持有者是一行时 `rename one of them`；是宿主时 `rename it`。
2. 打印在**后提供的那一行**上，不写在终端自己的话里：`b: weather is already provided by a: rename one of them`。
3. `#[rutis::service]` 类型的服务，`provider()` 在这一行提供时返回 `row <行名>`，不再返回类型名（`crates/rutis-bridge/src/marked.rs:110`）。宿主自己注册的服务仍然没有行，仍是 `the host`。
4. 内核的同名错误带上持有者：`ServiceExists` 里放完整的句子（`<名字> is already provided by <持有者>: rename …`），显示时不再套 `service "…" already registered in scope`（`crates/rutis/src/error.rs:144`）。名字用 `ServiceCatalog` 给的写名（插件作者写的那个），没有写名时用 `TypeKey::describe()`（一行 Rust 自己的类型，作者写的也是它）。
5. `check` 的同名错误：`rows a and b provide weather in the same scope: only the first to start provides it, the others fail; rename one of them, or isolate the name in each row`（`crates/rutis-host/src/main.rs:1001` 改写，与运行时同一句开头）。
6. 名字被别人占着、方法不一样：`no-method` 的句子后面接 `; it is provided by row <行>`（提供者不是调用方的行时）。调用方没有行（`rutis-host call`、Rust 嵌入）时也接。
7. 测试工具里"插件提供的名字测试已经给了"仍然写 `the host`：那里确实不是行（`docs/design-host-contract-types-2026-10-27.md` §六 的输出不变）。

### 3.3 等待的原因

1. 提到一个**服务**：`waiting for <名字> (<原因>)`，原因用下面这些：
   - 没有任何声明、也没有绑定：`nothing provides it yet`，有近名时 `nothing provides it yet; did you mean llm?`。
   - 提供者行在等这一行提供的服务：`row b waits for a too: they inject each other`。
   - 提供者那一行失败、被禁用、没在跑、在启动、在等它自己的运行时：沿用现有的说法（`crates/rutis-loader/src/loader/status.rs:446` 起），不新增词。
2. 提到一个**运行时**：`waiting for runtime py (down: exited with status 1)`、`(starting)`、`(stopped)`、`(running)`，与 `own_runtime_line` 现在一样（`status.rs:432`）。
3. 内置行的 `provides`：`Builtins::declare` 由 `rutis-host` 对它自己的每一行调用；没有声明的行照旧不提供名字，于是落进第 1 条的第一种。
4. `StillWaiting`（10 秒的提示）用同一套：`hang: still waiting after 10s for runtime py (down: exited with status 1)`、`chat: still waiting after 10s for llm (nothing provides it yet)`。尾巴 `: it starts once that is provided` 去掉：运行时被提供之后不会"启动"，这句话只有对服务成立，而服务的说法已经在括号里。
5. `check` 一侧不动这套词：`check` 说的是声明（`nothing declares …`），`status` 说的是此刻（`nothing provides it yet`）。两个词都不能换成对方。

### 3.4 在途调用

1. 登记点两处：宿主把调用交给运行时行（已有，`rows.rs:87`），宿主把调用交给宿主自己的服务（新，`crates/rutis-bridge/src/session/services.rs` 的分发处）。
2. 一条调用记：调用方（`caller_row()`，`services.rs:242`；没有行时是 `host`）、被调用的行（`diagnostics().bindings` 里那个提供者的行，`services.rs:154`）、服务名、方法名、开始时间。
3. `status` 的 `calls:` 合并两处，按时间从长到短：`pyhello → weather.forecast (row weather)  4s`。
4. 同一运行时里两行之间的调用不登记（D9）。指南的 `status` 一节写明：`calls:` 列的是经过宿主的调用。

### 3.5 帧与状态词

1. 发出去的 `stack`：只留出错那一行自己的帧（Python `_plugin_frame`、Go 的模块过滤、Node 的 `pluginStack` 同一件事）；一帧都留不下时，只留最里面那一帧（那时它是唯一能说明来源的东西）。
2. `rethrown` 里的帧同样过滤（对面发来的旧错误不过滤也算：打印时按 `MAX_FRAMES` 截断，诊断设计 §4.6）。
3. Rust 的 `session::Error`：`Debug` 手写，`Remote` 打印成 `Remote { name, message, code, site, frames }`，`frames` 是帧数，`graph` 不打印。读结构用 `thrown()`。
4. 状态词：`unloading` → `stopping`；`inactive` → `not running (its group is not running)`。`running`、`waiting`、`starting`、`failed`、`disabled`、`stopped in its instance`、`stopped` 不变，全部写进 [rutis-host 指南](guide/rutis-host.md) 的 `status` 一节。
5. 停止超时：`still stopping: weather`；卡住的是 rutis 自己的插件时 `still stopping: the host`；一个运行时进程还在时 `still stopping: runtime py`。

### 3.6 错误码

| code | 什么时候 | `data` |
| --- | --- | --- |
| `already-provided` | 一个行提供一个已被别人提供的名字 | `{service, provider}`（`provider` 是行名，宿主提供时为 `null`） |
| `no-method`（加一项） | 同前 | 加 `provider`（同上） |

其余码不变（诊断设计 §4.3 的表按上面两行补）。码不出现在给人看的那一行文字里。

## 四、开发者看到什么

### 4.1 插件作者

正常时什么都不用写：错误照常抛，`ctx.use` 照常拿到服务。改的是出错时的字。

下面的例子里有 Python 与 TypeScript 的行；其他语言同样的内容，位置写法各自的（`at f (x.ts:1:2)` / `File "x.py", line 1, in f` / `    at T.M (x.go:1)`）。

**另一个行抛了错，被这一行调用，而两个行在同一个运行时里**（例子里 llm 与 weather 在同一个 Python 运行时，chat 在别处；D1）：

```text
chat: KeyError: 'region'
    in llm:
    File "llm.py", line 31, in ask
    called from:
    File "weather.py", line 12, in today
    called from:
    at reply (src/chat.ts:14:36)
```

头一行是错误自己的文字（这一跳只知道行名，不知道服务名与方法名，D2），下面按"最早的在前"列出每一跳。

**同一个运行时，Go 用 `*Service` 或函数字段结构体调用**（D2 写明服务名）：

```text
chat: call to llm.ask failed: no API key
    in llm:
        at Llm.Ask (llm.go:31)
    called from:
        at Weather.Today (weather.go:12)
```

**出错时的每一种情况**：

| # | 怎样引起 | 输出（逐字） | 码 | 记在哪 | 怎样改 |
| --- | --- | --- | --- | --- | --- |
| 1 | 同一运行时里另一个行抛错（Python、Node、Bun、Go 的接口目标） | `chat: KeyError: 'region'` 下面是 `in llm:`、llm 的帧、`called from:`、weather 的帧 | 照旧 | 调用方这一行（它没接住时） | 看 `in llm:` 下面那一帧 |
| 2 | 同一运行时里另一个行抛错，Go 用 `*Service` | `chat: call to llm.ask failed: …` 同上 | 照旧 | 同上 | 同上 |
| 3 | 名字已经被一行提供 | `b: weather is already provided by a: rename one of them` | `already-provided`，`data {service: "weather", provider: "a"}` | 后提供的这一行 | 给其中一个换名字 |
| 4 | 名字已经被宿主自己提供 | `b: weather is already provided by the host: rename it` | `already-provided`，`provider: null` | 同上 | 给这一行的服务换名字 |
| 5 | 两个 Rust 行（`#[rutis::service]`）提供同一个名字 | `b: weather is already provided by a: rename one of them` | 同上 | 后提供的那一行 | 同上 |
| 6 | 名字被别人占着，方法不一样，`rutis-host call weather.today` | `weather has no method today (methods: ask); it is provided by row a` | `no-method`，`data {service, method, methods, provider: "a"}` | 记在这次调用上 | 看清楚 `weather` 现在是谁的：换名字，或找你那一行 |
| 7 | `ctx.use("lmm")`，`inject` 里写的是 `llm` | `service lmm is not available to this plugin, which does not declare it in inject: did you mean llm?`（Go：`… in Inject: did you mean llm?`） | `not-declared`，`data {service: "lmm"}` | 这一次调用 | 改成 `llm` |
| 8 | 同上，但整份配置里没有近名（`ctx.use("zzz")`） | `service zzz is not available to this plugin, which does not declare it in inject: add 'zzz' to its inject`（不变） | `not-declared` | 同上 | 要么声明它，要么等提供它的行 |
| 9 | `ctx.get("lmm")`（可选） | `service lmm is not available to this plugin, which does not declare it: did you mean llm?` | `not-declared` | 同上 | 改成 `llm` |
| 10 | 两个 Rust 行互相 inject | 行上说 `a: waiting for b (row b waits for a too: they inject each other)` | 无（状态） | `status`、`dev` 的状态行 | 打破其中一个的 `inject` |
| 11 | 等一个没人声明、也没人提供的名字 | `weather: waiting for llm (nothing provides it yet)` | 无 | 同上 | 等提供者，或改名字 |
| 12 | 名字写错，没人声明 | `weather: waiting for lmm (nothing provides it yet; did you mean llm?)` | 无 | 同上 | 改成 `llm` |
| 13 | 等自己的运行时 10 秒 | `hang: still waiting after 10s for runtime py (down: exited with status 1)` | 无 | 标准错误 | 看运行时的退出原因 |
| 14 | 关闭时监听器还在跑、抛了错 | `runtime py: row weather: listener of weather/asked failed: KeyError: 'region'` | 无 | 运行时的标准错误（`run` 下照旧转发） | 看这一行 |

第 1 行的头一行是错误自己的文字；跨进程时这一段仍然是 `call to weather.tomorrow failed: …`（诊断设计 §8.1 第 2 行不变）。

### 4.2 用 `rutis-host` 组装插件的人

配置不变。变的是 `check`、`status`、停止时的输出，以及同名时的行状态。

```json
{ "rows": [ { "id": "a", "name": "weather" }, { "id": "b", "name": "weather" } ] }
```

`rutis-host check`：

```text
rutis-host: error: rows a and b provide weather in the same scope: only the first to start provides it, the others fail; rename one of them, or isolate the name in each row
```

`rutis-host check`，一个谁都没声明的名字：

```text
rutis-host: warning: weather: nothing declares lmm; did you mean llm?
rutis-host: warning: weather: nothing declares zzz; a row that does not declare its services may provide it once it runs
```

（第二个名字只有在一行是 Cordis 行、另一行也是时才多一句 `; between Cordis rows of one runtime it resolves in Cordis`。）

`rutis-host status`：

```text
rows:
  weather: running
  pyhello: waiting for runtime py (down: exited with status 1)
services:
  weather: a
  llm: llm
runtimes:
  py: down (exited with status 1)
calls:
  host → pyhello.forecast  4s
```

`rutis-host call`，名字被别人占着：

```text
weather has no method today (methods: ask); it is provided by row a
```

停止超时：

```text
rutis-host: 10s passed: cleanups did not finish
  still stopping: weather
  still stopping: the host
```

状态词（`run`、`dev`、`status` 都用这几个）：

| 输出 | 什么时候 | 以前 |
| --- | --- | --- |
| `running` | 在跑 | 不变 |
| `waiting for …` | 等依赖或运行时 | 不变 |
| `starting` | 正在装载 | 不变 |
| `stopping` | 正在卸载 | `unloading` |
| `not running (its group is not running)` | 外面那一组没在跑 | `inactive` |
| `failed: …` | 失败，带原因 | 不变 |
| `disabled` | 配置里禁用了 | 不变 |
| `stopped` / `stopped in its instance` | 停了 | 不变 |

### 4.3 宿主持有者（嵌入 rutis 的应用）

```rust
let diagnostics = rutis::host::CtxHost::diagnostics(&root);
eprintln!("{diagnostics}");
```

```text
plugins:
  weather: running
  pyhello: waiting for runtime py (down: exited with status 1)
services:
  weather: provided by a
  llm: provided by llm
```

错误的结构不变，读法不变（`rutis_bridge::thrown`）：

| 情况 | 看到的 |
| --- | --- |
| 打印一个跨语言的错误 | `rutis_bridge::report::report(&error)`：`call to llm.ask failed: …` 或错误自己的头一行，下面是 `in llm:`、帧、`called from:` |
| `Debug` 打印跨语言错误 | `Remote { name: "KeyError", message: "'region'", code: None, site: Some(Site { row: "llm", .. }), frames: 3 }`，不再有错误图的 JSON |
| 同名冲突 | `thrown(&error)` 没有（它是内核的错误），`ErrorKind::ServiceExists` 的 `to_string()` 是 `weather is already provided by a: rename one of them`；`diagnostics()` 的 `services` 里这一行的提供者仍是原来的那个 |
| 在途调用 | `Loader::status().calls` 里有交给宿主自己的服务的那一次：`pyhello → weather.forecast (row weather)  4s` |

### 4.4 要懂的概念

没有新增。行的名字、服务的名字、`inject`、`status`、`check` 都是各角色已经要懂的（[#663](https://github.com/arcships/rutis/issues/663) 的概念预算：插件作者 7、`rutis-host` 用户 6、宿主持有者 10）。本稿做的是把已经有的概念用对：说错的行名改成对的，说"没有行提供"改成"还没有人提供"，把 `rutis-loader` 这类内部名字换掉。

宿主持有者多一样要看的东西：`describe_keys_with`（D7）。它是可选的，不调用时诊断输出仍能看，只是运行时那一行不说 `down`。这一样记在概念预算的"调试"一格里（每人最多多懂 3 个），不单独加一层。

## 五、兼容与迁移

写进《迁移到 0.21》与发布说明（两份）：

- **文字变了，结构不变**：诊断设计列出的错误类型、字段、`site`、`rethrown` 都不变；本稿改的是这些字段填什么、以及打印出来的字。
- **同一运行时里的错误换了归属**：以前 `in weather:`，现在 `in llm:`，并且多了 `called from:`。按行名匹配输出的人要跟着改。错误对象本身在插件里看不到这些，插件代码不受影响。
- **同名服务的文字**：`service "weather::Weather in scope <default>" already registered in scope` → `weather is already provided by a: rename one of them`；`weather is already provided by the host` 在提供者是一行时改成行名。按文字匹配的代码改为按 `already-provided` 码（D13）。
- **`no-method` 的文字**：后面多了 `; it is provided by row <行>`（提供者是一行、且不是调用方的行时）。`data.provider` 是新的键，旧的读法不受影响。
- **`not-declared` 的文字**：名字有近名时不再说 `add '…'`，改说 `did you mean …?`。`data` 不变，码不变。
- **`check` 的告警**：不再列行；`--strict` 的行为不变（还是算错误、还是以 1 退出）。
- **状态与提示的文字**：`no row provides it` → `nothing provides it yet`（只在确实没人提供时）；`still waiting … for hang: it starts once that is provided` → `… for runtime py (down: …)`；`still stopping: rutis-loader` → `still stopping: the host`；`unloading` → `stopping`，`inactive` → `not running (its group is not running)`。按这些字匹配的脚本要改；`status` 的 `--json` 形状不变（状态词跟着变）。
- **Rust**：`session::Error` 的 `Debug` 不再打印错误图（读结构用 `thrown()`）；`ErrorKind::ServiceExists` 的 `Display` 文本变了（变体与字段不变）。
- **内置行要声明 `provides`**：不声明时，`check` 与 `status` 对它们提供的名字仍说"没人声明/还没人提供"。这是行为上的放宽（不再说错），不是收紧。
- **协议**：不加字段、不升版本。`site`、`rethrown` 已在线上；本稿只是让运行时也开始填。旧的一端收到 `site` 照旧忽略（诊断设计 §4.1）。
- **同一运行时里两行之间的调用仍然不出现在 `calls:`**：这一条写进指南（以前只在设计稿里）。

## 六、实施

先做宿主与内核（错误码、同名、等待原因、在途调用），再按语言做归属与帧，最后是文档。每个 PR 自带逐字测试。

| # | 内容 | 位置 | 完成条件 |
| --- | --- | --- | --- |
| E-1 内核与宿主 | `ServiceExists` 带完整句子（`crates/rutis/src/registry.rs`、`ctx.rs`、`crates/rutis/src/error.rs:144`）；`ErrorKind::ServiceExists` 的码与 `data`；`no-method` 的 `provider`（`crates/rutis-bridge/src/runtime/rows.rs:149`）；`provider()` 给行名（`marked.rs:110`，用 `crates/rutis-bridge/src/services.rs:154`）；`already-provided` 码进诊断设计 §4.3 的表 | 三个来源（运行时行、`#[rutis::service]` 行、宿主自己）各一条逐字用例；`rutis-bridge/tests/report.rs` 加两行 |
| E-2 等待与诊断 | 内置行 `Builtins::declare`（`rutis-host` 对它自己的行调用）；`describe_keys_with`（`crates/rutis/src/host.rs`）；`RuntimeDiagnostics` 的 `Display` 用描述者；`StillWaiting` 去掉尾巴；`status` 的运行时状态按进程判断（`crates/rutis-loader/src/loader/status.rs:278`，用 `crates/rutis-bridge/src/runtime/state.rs:180`）；`Cycle` 的说法（`status.rs:474`）；`NoProvider` 改说 `nothing provides it yet`（`status.rs:453`） | 互相 inject 的两个内置行在 `status` 与 `dev` 上都写 `they inject each other`；`check` 对同样两行报环；运行时退出后立刻 `status` 就是 down（一条不 sleep 的用例，靠关闭进程与读状态之间不等待）；`diagnostics()` 与 `status` 对同一个键说法一致（一条比对用例） |
| E-3 `check` 与命令行 | `undeclared_line` 不再列行（`crates/rutis-host/src/main.rs:953`）；`clash_line` 改写（`:1001`）；`call.rs` 的 `failed` 不再需要自己补提供者（宿主已经写在错误里）；`still_running` 与 `describe`（`crates/rutis-host/src/host.rs:828`、`crates/rutis-host/src/status.rs:18`） | `rutis-host/tests/check.rs` 的两条改成新文字；`call` 的一条；停止超时的一条 |
| E-4 在途调用 | 宿主把调用交给宿主自己的服务时登记（`crates/rutis-bridge/src/session/services.rs`）；`Loader::status` 合并两处（`crates/rutis-loader/src/loader/status.rs:313`） | 一条用例：宿主 → 运行时行 → 宿主服务（卡住），`status` 里两条都在，`from` 是行名 |
| E-5 Python | 归属与拆栈（`runner.py` 的 `charged_row` 复用、`peer.py` 的 `encode_error`）；帧过滤用 `_plugin_frame`（`runner.py:1341`）；`not-declared` 的 did you mean（`runner.py:98`）；关闭时的监听器错误（`runner.py` 的 `charge`）；`no-method` 的 `provider` | 四条逐字用例（归属、did you mean、关闭时的监听器、`no-method`），外加一条"内部帧不出现"的禁用词用例 |
| E-6 Node、Bun | 归属与拆栈（`node/rutis-runtime/src/unhandled.mjs`、`errors.mjs`；`bun/rutis-bun/src/errors.ts`）；`not-declared` 的 did you mean；`no-method` 的 `provider`；同名文字统一（`leaf.mjs:63`、`runner.ts:239`） | 同上 |
| E-7 Go、Swift、Kotlin | 归属：Go 在 `localService`（`runtime.go:1201`）里写 `site`（行、服务、方法），接口目标写明边界；Swift、Kotlin 按栈判断（各自 `rows.load` 时拿到的入口）；panic 的帧过滤（`peer/peer.go:1712`）；`not-declared` 的 did you mean（`runtime.go:988`、`Runtime.swift:1075`、`Refusals.kt:144`）；同名文字统一（`runtime.go:435`、`:528`、`Runtime.kt:379-380`、`:497`、`:606`） | 每种语言同上四条；Go 另外一条"接口目标归不到提供者"的用例（输出与今天相同，写明在指南里） |
| E-8 文档 | [rutis-host](guide/rutis-host.md) 的 `status` 一节（状态词、`calls:` 列什么）、[插件 API](guide/plugin-api.md)（出错时的字、同一运行时里的归属与它的边界）、各语言指南（Go 的接口目标边界）、《迁移到 0.21》、[保证表](guarantees.md)（新条目 `bridge/error-ownership`、`loader/wait-words`） | 链接检查、两份同步 |

完成条件（#937 在全部满足后关闭）：

- [ ] §四 的每一条都有逐字测试；四种语言（Node、Python、Go、Rust 宿主）各一列；
- [ ] §3.6 的两个码在测试里按码断言，不按文字；
- [ ] 逐条重跑 0.19.0 评估里 #937 点到的故障，输出与 §四 一致；
- [ ] 《迁移到 0.21》与发布说明两份写好。

## 七、后续

- **同一运行时里两行之间的调用**在 `calls:` 里仍旧看不到（D9）。要看到，就得让 `ctx.use` 交出代理而不是对象，那是对象引用的语义变更（[对象引用](design-object-refs-2026-10-27.md) §四），单独一份设计稿。
- **Go 的接口目标**归不到提供者（§3.1 第 5 条）：Go 的错误没有栈，唯一能补的是让 `ctx.Use` 连接口目标也走一层包装，那同样会改变 `ctx.use` 交出来的东西。
- **`check` 看不见内置行的 `provides`**，除非宿主声明（§3.3 第 3 条）。`#[rutis::plugin]` 与 `#[rutis::service]` 自动填 `provides` 是另一件事：它给的是"这个插件会提供哪些名字"，值得单独做。
