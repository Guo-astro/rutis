# 值传不过去时的一个样子：NaN、循环、Go 的编码错误、返回值解不开

[English](design-value-errors-2026-11-03.en.md)

状态：定稿，未实现。日期：2026-11-03。基线：`main` `d2a8d069`（0.20.1）。计划：0.21。

Issue：[#936](https://github.com/arcships/rutis/issues/936)（属于 [#663](https://github.com/arcships/rutis/issues/663) 的 D1、D5）。本设计实施的 issue 见 §7。
依据：[插件 API](guide/plugin-api.md)「值怎样传递」；[诊断](design-diagnostics-2026-10-26.md) §4.3、§4.5；0.19.0 开发者体验评估（#663）。

## 1. 现在怎样

0.19 的评估说：值传不过去时，大多数出口都按插件 API 指南报 `<服务>.<方法>: argument N cannot be sent: …` 加 `bad-value`，下面这些不是。每一行都对着源码核对过，下表第三列写的是**今天开发者逐字看到的**。

| 出口 | 今天开发者看到的 | 为什么（已对照源码） |
| --- | --- | --- |
| Node、Bun 发送 NaN / Infinity | `TypeError: non-finite number`，`code`、`data` 都空 | 逐参数包装之外才编码整帧：`checkData` 不查数字（`node/rutis-runtime/src/session.mjs:188`），到 `encode` 才在 `JSON.stringify` 的钩子里抛（`codec.mjs:6`），而 `#request` 在 `session.mjs:551` 调它 |
| Node、Bun 发送装着自己的 Map | `test.take: argument 1 cannot be sent: a value nested deeper than 64 levels cannot cross between processes` | Map / Set 走 `#encodeTagged`，那里只查深度（`session.mjs:360-363`） |
| Python 发送循环引用 | `weather.tomorrow: argument 1 cannot be sent: value nested deeper than 64 levels, the most a value crossing processes may nest` | `_data` 只查深度，不查循环（`python/rutis/rutis/peer.py:648-675`） |
| Go 发送循环引用（指向自己的指针、装着自己的切片） | `… cannot be sent: value nested deeper than 64 levels, the most a value crossing processes may nest` | `go/rutis/internal/convert/encode.go:142-147`：结构体字段也让 `levels` 加一，先撞 64 层，`maxHops` 那句到不了 |
| Swift 发送循环引用 | `weather.tomorrow: argument 1 cannot be sent: value nested deeper than 64 levels, the most a value crossing processes may nest (a cyclic reference?)` | 只查深度，括号里猜「是循环吧」：`swift/rutis/Sources/Rutis/Codec/ValueCodec.swift:147-148`、`Codec/Encoder.swift:55-57`；收值的一侧同样（`Session/Peer.swift:662,741-742`） |
| Go 返回 channel / NaN | `chan int cannot cross between processes`、`the number NaN is not finite: only finite numbers cross`：没有 `bad-value`、没有方法名、没有「返回值」 | 方法返回时 `convert.Results` 就把值换成对端的样子（`convert.go:244-258`），失败于是成了这个方法的 error，走 `respondTo` 的 throw 分支，`ofResult`（`peer/crossing.go:68-75`）碰不到它 |
| Go 调用方发送 NaN | `argument 1: the number NaN is not finite: only finite numbers cross`：没有方法名、没有 `bad-value` | `Service.Call` 直接返回 `convert.ToPeerArgs` 的错误（`go/rutis/internal/runtime/runtime.go:1153`） |
| 返回值解不开（Go 收到对象却要 string；Rust 宿主收到 42 却要 string） | `cannot decode {"text":"cloudy"} as string: json: cannot unmarshal object into Go value of type string`；``invalid type: integer `42`, expected a string`` | Go：`decode.go:70`，`Service.Call` 原样返回；Rust：`decode_value` 把 serde 的原文塞进 `Error::Value`（`crates/rutis-bridge/src/session/objects.rs:784`、`:809`）。原文本身是对的（§4 的原则），缺的是「这是哪个方法的返回值」这个前缀 |
| Python 收到 TS 的函数 | `<function from the host>` | `peer.py:694-695` |
| Python 返回函数给 Rust 要的 `serde_json::Value` | `the value holds a live Cordis object or function that its Rust type cannot represent` | `objects.rs:809`：这句跟 Cordis 无关 |
| Node 收到另一行的函数 | `console.log` 印 `[Function (anonymous)]`；`String(fn)` 印 `() => { check(); return this.#requestSync('call', { reference: id }, args) }` | 收到的函数就是个普通箭头函数，没有自己的显示（`session.mjs:474`） |
| Kotlin 收到函数、对象 | `RemoteFunction(7)`、`RemoteObject(7)` | `advanced/RemoteFunction.kt:25`、`advanced/RemoteObject.kt:29` |

核对源码时另有两处评估的表里没有：

- **Rust 发 NaN 不报错，发出去的是 `null`。** `Exact` 把标量交给 `serde_json::value::Serializer`（`objects.rs:380-396`），它对 f64 用 `Value::from`，非有限数变成 `Json::Null`（serde_json 的 `From<f64> for Value` 文档写明）。别的语言都拒绝，Rust 悄悄换成了另一个值。
- **Node 的 `String(fn)` 把运行时的源码印给开发者看。** 上面那一行就是它印的。
- **「嵌套过深」今天有四种说法**，循环只能靠它表达，Swift 甚至在括号里猜：
  - Node、Bun：`a value nested deeper than 64 levels cannot cross between processes`（`session.mjs:102`）；
  - Python、Go、Kotlin：`value nested deeper than 64 levels, the most a value crossing processes may nest`（`peer.py:520-523`、`peer.go:607`、`internal/peer/Peer.kt:63-65`）；
  - Swift：后一种加 ` (a cyclic reference?)`（`Codec/ValueCodec.swift:148`）；
  - Rust：`value nested deeper than 64 levels, the most a value crossing a session may nest`（`rpc.rs:3136`）。

另外，`bad-value` 的 `data` 今天只有 `{part, argument?, limit?}`（诊断 §4.3）：程序分不出「NaN」和「循环」，只能读文字。

## 2. 要解决什么

1. 值传不过去时只有一个样子：前缀（是哪个调用、第几个参数或返回值）、原因的一句话、错误码 `bad-value`、`data`。
2. 循环引用报成循环，不报成「嵌套过深」。
3. 收到的值解不开时，说这是哪个方法的参数或返回值，然后照搬解码器的原文（serde 说 ``invalid type: integer `42`, expected a string``，Go 说 `json: cannot unmarshal object into Go value of type string`），前缀只说位置。

## 3. 决策

**D1 一个框架，原因一句话。** 每个「值传不过去」的出口都写成 `<调用>: argument N cannot be sent: <原因>`、`<调用>: the result cannot be sent: <原因>`、`<调用>: the error <Name> cannot be sent: <原因>`、`the payload of <事件> cannot be sent: <原因>`；码一律 `bad-value`，`data` 一律 `{part, argument?, reason, limit?}`。原因是一句话，只说开发者写的名字（服务、方法、参数、类型名），说 `cannot cross between processes`（Python 今天的「cannot cross the boundary」改掉）。这里能改写，是因为这句话是 rutis 自己造的，没有底层错误可言（§4 的原则、D6）。**不采用**：给每个出口一套自己的文字——程序还是只能读文字，评估里的每一行也正是这么来的。

**D2 检查在编码之前，逐种语言统一。** 值的检查放在编码之前的一次遍历里，遍历的就是运行时本来就要走的那条路（数据、列表、记录、带标签的值、Map / Set），不是等 `JSON.stringify`、`json.Marshal`、serde 报错。Node / Bun 的 `checkData` 因此要走进 Map / Set 与记录（今天只走纯数据），数字检查从 `codec.mjs` 挪出来；Python 的 `_data` / `_encode` / `_encode_tagged` 都要带 `seen`；Go 的 `toPeerValue` 把 `seen` 与 `levels`、`hops` 一起传。**不采用**：在 codec 里兜底——那里已经不知道是哪个调用的第几个参数了。

**D3 循环先于深度。** 每个节点先看「它是不是这条路上已经出现过」，再看深度；`seen` 是当前这条路上的容器，往回走时删掉，所以同一个对象出现在兄弟位置（`[shared, shared]`）不是循环。循环长过 64 层的（走完 64 层才重复）仍报「嵌套过深」——那是它唯一能说清的。**不采用**：只靠深度——#936 的第二行就是这样，开发者看到「嵌套过深」去改结构，改不好。

**D4 深度限制留 64 层，一句话。** 今天有四种说法（§1），Swift 还在括号里猜是不是循环，统一成 `the value nests deeper than 64 levels`。限制留下：它保证一次发送不会把进程栈走穿，也让对端收到的东西有界；去掉它，值的大小只受 16 MiB 消息限制约束。**不采用**：去掉限制。

**D5 非有限数：`the number NaN is not finite`。** 「NaN」「Infinity」「-Infinity」是开发者写的那个值本身；Python 今天的 `NaN cannot cross processes`、Go 的 `… : only finite numbers cross` 都改成这一句。Rust 的 `Exact` 在 `serialize_f32` / `serialize_f64` 里拒绝（今天变成 `null`，D9 的兼容一节）。**不采用**：让 NaN 变成 `null`——那是 Rust 今天的行为，两次开发者的调用因此得到不同的值而不是一个错误。今天那句 `non-finite number` 是 Node 的编码器自己说的，没有底层错误，所以这句改写不算转述（§4 的原则）。

**D6 解不开的值：原文照搬，只加前缀，各给一个码。** 值到了却解不开，是 serde、`encoding/json`、kotlinx.serialization、Swift 的 `ValueCodec` 说的话，它们比任何改写都准：``invalid type: integer `42`, expected a string`` 一次说出了期望什么、收到什么，`json: cannot unmarshal object into Go value of type string` 说出了 Go 看到了什么。所以消息体是那句原文，rutis 只加一个前缀回答「是我的哪次调用、哪个参数」：`<服务>.<方法>: argument N: <原文>`（码 `bad-argument`）、`<服务>.<方法>: the result: <原文>`（码 `bad-result`）。`data` 照旧是 `{service|object, method, argument?, expected?}`，`expected` 来自声明里的类型，不是消息的替代品。**不采用**：转述成 `<调用>: argument 1 should be a string, got an object`（Rust 的 `marked.rs`、Go 的 `refusals.go`、Kotlin 的 `Refusals.kt`、Swift 今天就是这么做的）——`got an object` 把解码器已经说清的事又说了一遍，还常常说得更少；这是这份设计要改回来的一处。**不采用**：复用 `bad-value`——那句框架是「cannot be sent」，可这个值已经到了，只是这个类型收不下。**不采用**：只留原文、不加前缀——开发者就不知道是哪次调用；去掉码——程序还是要有码。

**D7 事件的负载说自己的名字。** 事件经过会话时是一个控制调用（`events.dispatch`），今天负载坏了报 `events.dispatch: argument 1 cannot be sent: …`：那是内部名字，也说不清是哪个参数。发出事件的一方把这次调用**描述成事件名**（会话的调用方法多收一个描述），于是报 `the payload of weather/asked cannot be sent: …`，`part` 是 `payload`。事件在本进程分发时不过会话，也就不会有这个错误。

**D8 引用值的样子：`<function of another process>`（有行名时 `<function of row weather in node>`）。** 三种引用一个形状：`<function of …>`、`<object of …>`、`<asynchronous result of …>`。今天 Python 说「from the host」（它其实来自另一行）、Node 印 `[Function (anonymous)]` 和运行时的源码、Kotlin 印 `RemoteFunction(7)`（内部 id）。行名今天不在线上：引用只带 `origin`（调用 id 的链），带的是调用方的端点，不是发出值的行。**不采用**：现在就把行名放进引用——协议里 `WireValue::Reference` 是 `deny_unknown_fields`（`crates/rutis-bridge/src/session/protocol.rs:8,31,42-47`），多一个键会让旧宿主直接结束会话；要做就得先抬协议版本。这一轮的话术先不承诺行名，行名留给需要它的那个协议改动（§7 的 S7 记下这一点）。

**D9 谁失败，只有那一次调用。** 上面每一行都只让这次调用失败（发出的一方拿到错误，收到的一方拿到一个 throw），会话继续；只有「消息超过 16 MiB」照旧在整帧编码后判（`part` + `limit`）。**不采用**：让坏值结束会话——那是 0.18 之前的样子。

## 4. 规则

### 原则：原文优先

消息体是底层错误自己的话，逐字照搬；rutis 只加一个短前缀，回答「这是我的哪次调用、哪个参数」。

- 底层错误（serde、`encoding/json`、kotlinx.serialization、Swift 的 `ValueCodec`、Go 的 `reflect`）说什么，就说什么：``invalid type: integer `42`, expected a string``、`json: cannot unmarshal object into Go value of type string`、`KeyError: 'town'`。
- 前缀只说位置：`weather.ask: `、`argument 2: `、`the result: `、`the payload of weather/asked: `。不转述，不概括，不把底层错误已经说过的话再说一遍。
- 只有 rutis 自己造的错误才改写成开发者读得懂的话——它没有底层错误可言。这类句子又分两种：这份设计重定的，和照今天不动的。
- 拿不准时，保留原文。

每一句归谁，一张表说清：

| 句子 | 谁说的 | 这份设计 |
| --- | --- | --- |
| ``invalid type: integer `42`, expected a string``、`json: cannot unmarshal object into Go value of type string`、kotlinx.serialization 与 Swift `ValueCodec` 解码时的那句 | 底层解码器 | 逐字保留，只加位置前缀（§4.5） |
| `the number NaN is not finite`（Node 今天只说 `non-finite number`） | rutis | **重定**（§4.2） |
| `the value holds itself (a cycle)` | rutis | **重定**：今天 Python、Go、Kotlin、Swift 报成「嵌套过深」 |
| `the value nests deeper than 64 levels` | rutis | **重定**：今天四种说法，Swift 还带 `(a cyclic reference?)` |
| `a <类型名> cannot cross between processes`（`a symbol`、`a chan int`、`a module`，Node 的 `: pass plain data instead`） | rutis | **重定句子**：统一成这一句；哪些值不跨仍由各语言的规则定（§4.2 的 `type` 行） |
| `the far end does not take <标签> values (it did not declare the capability value-<标签>)` | rutis | 照今天的句子不动 |
| `the message would be <n> bytes, over the limit of <limit> bytes (<n> MiB)` | rutis | 照今天的句子不动 |
| `argument 1 is missing`、`<服务>.<方法> takes 1 argument, got 2` | rutis（没有底层错误） | 照今天的句子不动 |
| `the value holds a function that its Rust type cannot represent`、`the value holds a tagged value (…) that its Rust type cannot represent`、Swift 的 `cannot decode … into …` | rutis（没有底层错误） | 照今天的句子不动，只把 `live Cordis` 去掉 |
| 同步等待成环的 `<那次调用> would wait on itself: it is awaited inside the synchronous call that produces it (synchronous calls)`、`row <x> is not loaded in this runtime`、`non-finite number` 这类运行时自己的行 | rutis | 不归这份设计（诊断设计与它自己的设计管） |

### 4.1 框架

值传不过去时，发出的一方抛出的错误的 `message` 是下列之一，逐字：

| 传的是什么 | 文字 | `part` |
| --- | --- | --- |
| 调用的第 N 个参数（从 1） | `<服务>.<方法>: argument N cannot be sent: <原因>` | `argument`，`argument: N` |
| 一个控制调用（没有服务，例如 `events.on`） | `<方法>: argument N cannot be sent: <原因>` | 同上 |
| 函数引用的调用 | `a function: argument N cannot be sent: <原因>` | 同上 |
| 返回值 | `<服务>.<方法>: the result cannot be sent: <原因>` | `result` |
| 返回的 error（超过消息限制） | `<服务>.<方法>: the error <Name> cannot be sent: <原因>` | `error` |
| 事件的参数（D7） | `the payload of <事件名> cannot be sent: <原因>` | `payload` |
| 整个参数表传不过去（不是一串参数） | `<服务>.<方法>: the arguments cannot be sent: <原因>` | `arguments` |

每种语言相同，字的顺序也相同。Node、Bun 保留今天抛的类（`TypeError` / `RangeError`），Python `ValueError`，Go、Swift、Kotlin 是 `Error`，Rust `Error::Value`：**类由语言的习惯决定，程序只看 `code`**。**不采用**：把它们都改成一种类——`instanceof` / `except` 的代码会静默改变行为。

### 4.2 原因（封闭的一组）

原因是一句话，不要说运行时自己的事。`data.reason` 是程序用的名字：

| `reason` | 一句话 | 谁来报 |
| --- | --- | --- |
| `non-finite` | `the number NaN is not finite`、`the number Infinity is not finite`、`the number -Infinity is not finite` | 所有语言（Rust 今天不报，D5） |
| `cycle` | `the value holds itself (a cycle)` | 所有语言（Python、Go、Kotlin、Swift 今天报成深度） |
| `too-deep` | `the value nests deeper than 64 levels` | 所有语言（今天有四种说法，Swift 那句后面还带 `(a cyclic reference?)`，见 §1） |
| `type` | `a <类型名> cannot cross between processes`：`a symbol`、`a chan int`、`a module`；Node 今天在纯数据的位置还会补一句怎么办（`a WeakMap cannot cross between processes: pass plain data instead`），那一句照留 | 各语言今天已有的那一句，加上冠词与 `between processes`；句子里的类型名用它自己的语言写（Go 写 `chan int`，Python 写 `module`） |
| `tag` | `the far end does not take <标签> values (it did not declare the capability value-<标签>)` | 今天已有的句子，不改 |
| `limit` | `the message would be <n> bytes, over the limit of <limit> bytes (<n> MiB)` | 今天已有的句子，不改 |

这张表只收 rutis 自己造的原因句子，逐句归在哪里见 §4 开头的分类表：**重定**的是 `non-finite`、`cycle`、`too-deep` 三句，以及 `type` 那类句子的写法；**照今天不动**的是 `tag` 与 `limit` 两句。`type` 那一行的另一半是各语言自己的规则（哪些值不跨），那部分这份设计不管，只统一句子：`a <类型名> cannot cross between processes`。

### 4.3 错误码与 `data`

- 传不出去：`bad-value`，`data` = `{part, argument?, reason, limit?}`。`part` 取 `argument` / `arguments` / `result` / `error` / `payload`；`argument` 从 1；`reason` 取 §4.2 的六个之一；`limit` 只在 `reason` 是 `limit` 时有，单位字节。
- 收到的参数解不开：`bad-argument`（不变），`data` = `{service|object, method, argument, expected}`。
- 收到的返回值解不开：`bad-result`（新），`data` = `{service|object, method, expected}`。

诊断设计的码表（§4.3）里，这份设计**只动两行**：`bad-value` 一行（`data` 加 `reason`，`part` 多一个 `payload`）和新增的 `bad-result` 一行。表里别的行不归这份设计——`already-provided`、`no-method` 的 `provider` 等属于 [#937](https://github.com/arcships/rutis/issues/937) 的设计稿（`docs/design-error-messages-2026-11-03.md`），两份稿子落进同一张表，后落地的那一份负责 rebase。这份设计不复制整张表。

### 4.4 检查在哪一步

每种语言在同一步、同一处检查：把值换成线上的样子的时候，走完这个值之前。

- **Node / Bun**：`checkData` 走进 Map、Set、记录和带标签的值（今天只走纯数据），先查 `seen` 再查深度，数字在那里查；`codec.mjs` 的钩子留着兜底（`#write` 的握手等不含调用名的帧），调用路径上再也到不了它。
- **Python**：`_data`、`_encode`、`_encode_tagged` 都带 `seen` 与 `depth`，顺序同 Node。
- **Go**：`convert.toPeerValue` 带 `seen`（`map[uintptr]bool`，往回收时删），`levels` 与 `hops` 照旧；数字在 `float32` / `float64` 那一支查。
- **Rust**：`Exact` 的 `serialize_f32` / `serialize_f64` 先查有限，再交给 `serde_json`（今天不查）；层级照今天的位置（发出去之前 `fits` 查 `MAX_DEPTH`，`rpc.rs:3139`，收进来之前 `MAX_FRAME_DEPTH`），只换那句说法；`Exact` 自己也要数层级，否则一个用 `Rc` 指回自己的值在 `arg` 里就递归下去，到不了 `fits`。
- **Swift / Kotlin**：编码器按同一顺序做同样三步。它们今天都查深度（`Codec/ValueCodec.swift:115`、`Codec/Encoder.swift:55`、`internal/peer/Peer.kt:420,508`）并拒绝非有限数（用例表的 `swift/nan`、`kotlin/nan`），但都不查循环、都把循环报成深度（Swift 还在括号里猜）；`seen` 与三句话按这条规则补上，由 S8 的用例核对。

层级照旧：数组、对象、Map、Set、记录各算一层，一次调用的参数表不算一层。

### 4.5 收到的值解不开：原文照搬，只加前缀

- 只有把值解成声明过类型的运行时才会有这个错误：Go、Rust、Swift、Kotlin。Node、Bun、Python 没有声明过类型，收到的值是什么就是什么，不会有这一行。
- 消息体是解码器的原文，rutis 只加前缀：`<服务>.<方法>: argument N: <原文>`（码 `bad-argument`）、`<服务>.<方法>: the result: <原文>`（码 `bad-result`）；对象的方法写 `<类型>.<方法>: …`。前缀只说位置，原文一个字不改，也不补一句自己的概括。
- 各语言的「原文」是哪一句：
  - **Rust**：serde 逐字的那句——``invalid type: integer `42`, expected a string``、``invalid type: null, expected a string``。`marked.rs` 的 `of_decoding` 不再把它转述成 `should be a string, got an integer`。
  - **Go**：`encoding/json` 逐字的那句——`json: cannot unmarshal object into Go value of type string`、`json: cannot unmarshal array into Go value of type int`；`convert.FromPeer` 自己的判断（`a list cannot be decoded as string`、`a function cannot be decoded as int` 之类，`convert/decode.go:70`）是 rutis 造的，照它今天的说法留。
  - **Swift**：`ValueCodec` 自己的那句 `cannot decode an object into String`（`Codec/ValueCodec.swift:232`）——rutis 造的，本来就清楚，只加前缀。
  - **Kotlin**：生成代码保留解码器（kotlinx.serialization 的 `SerializationException`）的原文；`Refusals.kt:32-45` 不再拼成 `should be … got …`。只有「参数没给」这类没有底层错误的，仍用平白的话。
- rutis 自己造的、没有底层错误的，仍用平白的话，逐字保留今天的句子：`argument 1 is missing`、`<服务>.<方法> takes 1 argument, got 2`、`the value holds a function that its Rust type cannot represent`（今天那句里的 `live Cordis` 去掉）、`the value holds a tagged value (an instant, bytes, a map, ...) that its Rust type cannot represent`。
- `data` 的作用不变：`{service|object, method, argument?, expected?}`。`expected` 是声明里那个类型（Go 的 `Expected(t)`，Rust 从 serde 那句里取的 `expected`），它是给程序看的，不是消息的替代品。

### 4.6 事件的负载

发出事件的那一侧把这次分发描述成事件名（D7）；于是负载里任何一个传不过去的值都报 `the payload of <事件名> cannot be sent: <原因>`，`part` 是 `payload`。`emit` 不等结果，所以这个错误在发出方报告成一行（`rutis: emit of <事件名> from row <行> failed: <错误>`）；`parallel`、`serial`、`bail`、`waterfall` 把它抛给调用方。

### 4.7 引用值的样子

收到的函数、对象、异步结果，在语言印值的地方（Node 的 `util.inspect` 与 `String`、Python 的 `repr`、Kotlin / Swift 的 `toString` / `description`、Go 的 `%v`）印成一行：

- `<function of another process>`、`<object of another process>`、`<asynchronous result of another process>`：今天的样子（D8）。
- 线上带行名之后（另一个协议改动）：`<function of row weather in node>`，`in` 后面是发出值的那个端点在握手里说的 `implementation.name`。

不管哪一种，都不印运行时的源码、内部 id、`Cordis`、`host` 这类开发者没写的词。

### 4.8 各语言必然不同的地方

| 哪里 | 为什么 | 例子 |
| --- | --- | --- |
| 类型名 | 用开发者写的那种类型 | Go `a chan int cannot cross between processes`；Python `a module cannot cross between processes`；Node `a symbol cannot cross between processes` |
| 有没有「解不开」 | 只有声明类型的运行时才有 | Node、Bun、Python 没有；Go、Rust、Swift、Kotlin 有 |
| 错误类 | 语言的习惯（D1） | `TypeError` / `RangeError`（Node、Bun）、`ValueError`（Python）、`Error`（Go、Swift、Kotlin）、`Error::Value`（Rust） |
| 期望的词 | 各语言 `Expected` 已有的那套 | Go `[]int` 的期望是 `a list of integers`；Rust `Vec<i64>` 是 `a list` |

其余每一处——框架、原因的三句话、码、`data`、`part`、参数的序号从 1 起——每种语言相同，逐字相同。

## 5. 开发者看到什么

### 5.1 写插件的人（TypeScript）

写 `weather` 这一行的插件，它调用另一行的 `forecast`：

```ts
// rows/weather/src/index.ts
import { definePlugin } from '@arcships/rutis'

export default definePlugin({
  apply(ctx) {
    const forecast = ctx.use('forecast')

    ctx.provide('weather', {
      // 正常：返回值作为数据回来（{city, slug}）
      today(city: string) {
        return { city, slug: forecast.tomorrow(city) }
      },
      // 正常：事件的负载跨进程，监听器在别的行
      asked(city: string) {
        ctx.emit('weather/asked', { city, degrees: 18 })
      },
    })

    ctx.on('weather/answered', (answer: { city: string; degrees: number }) => {
      console.log(answer.city, answer.degrees)   // Oslo 18
    })
  },
})
```

正常时，`ctx.use('forecast').tomorrow('Oslo')` 得到 `"Oslo tomorrow"`；`ctx.emit('weather/asked', …)` 立即返回；`await ctx.parallel('weather/asked', { city: 'Oslo', degrees: 18 })` 等所有监听器。

失败时，一栏一行，逐字：

| 怎么造成的 | 输出 | 码与 `data` | 怎么改 |
| --- | --- | --- | --- |
| `forecast.week('Oslo', Number.NaN)` | `forecast.week: argument 2 cannot be sent: the number NaN is not finite`（`TypeError`） | `bad-value`，`{part: 'argument', argument: 2, reason: 'non-finite'}` | 别发 NaN：`null`，或者不带这个参数 |
| 值里某处有 `-Infinity` | `forecast.week: argument 1 cannot be sent: the number -Infinity is not finite` | `bad-value`，`{part: 'argument', argument: 1, reason: 'non-finite'}` | 同上 |
| 参数是装着自己的对象：`const a = {}; a.self = a` | `forecast.week: argument 1 cannot be sent: the value holds itself (a cycle)`（`TypeError`） | `bad-value`，`{part: 'argument', argument: 1, reason: 'cycle'}` | 去掉指回自己的那一个字段；`structuredClone` 也会保留环，用新的对象重建 |
| 参数是装着自己的 Map（对端声明了 `value-map`） | 同上那一句 | 同上，`reason: 'cycle'` | 同上 |
| 参数嵌套 70 层 | `forecast.week: argument 1 cannot be sent: the value nests deeper than 64 levels`（`RangeError`） | `bad-value`，`{part: 'argument', argument: 1, reason: 'too-deep'}` | 少一层：把中间那层摊平成对象 |
| 参数里有 `Symbol('x')` | `forecast.week: argument 1 cannot be sent: a symbol cannot cross between processes`（`TypeError`） | `bad-value`，`{part: 'argument', argument: 1, reason: 'type'}` | 发字符串或数字 |
| 返回值里有 NaN：`today() { return { degrees: NaN } }` | 调用方收到 `weather.today: the result cannot be sent: the number NaN is not finite` | `bad-value`，`{part: 'result', reason: 'non-finite'}` | 同上；这一行自己不受影响，会话继续 |
| `await ctx.parallel('weather/asked', { city: 'Oslo', degrees: Number.NaN })` | `the payload of weather/asked cannot be sent: the number NaN is not finite` | `bad-value`，`{part: 'payload', reason: 'non-finite'}` | 同上 |
| `ctx.emit('weather/asked', { city: 'Oslo', degrees: Number.NaN })` | 发出方报告一行：`rutis: emit of weather/asked from row weather failed: TypeError: the payload of weather/asked cannot be sent: the number NaN is not finite` | 同上 | 同上 |
| `console.log(forecast.tomorrow)` | `<function of another process>` | — | — |
| `console.log(forecast)` | `<object of another process>` | — | — |

`String(forecast.tomorrow)` 印同一句 `<function of another process>`，不再印运行时的源码。

### 5.2 Python：只有不同的地方

```python
# rows/forecast/src/forecast.py
from rutis import define_plugin


class Forecast:
    async def tomorrow(self, city):
        return f"{city} tomorrow"


plugin = define_plugin(name="forecast", provides={"forecast": Forecast})
```

| 怎么造成的 | 输出 | 码与 `data` |
| --- | --- | --- |
| `ctx.use('forecast').week('Oslo', float('nan'))` | `forecast.week: argument 2 cannot be sent: the number NaN is not finite` | `bad-value`，`{part: 'argument', argument: 2, reason: 'non-finite'}` |
| 循环引用（列表、字典、dataclass 里的都算） | `forecast.week: argument 1 cannot be sent: the value holds itself (a cycle)` | `bad-value`，`{part: 'argument', argument: 1, reason: 'cycle'}` |
| 65 层 | `forecast.week: argument 1 cannot be sent: the value nests deeper than 64 levels` | `bad-value`，`{part: 'argument', argument: 1, reason: 'too-deep'}` |
| 模块对象、生成器 | `forecast.week: argument 1 cannot be sent: a module cannot cross between processes` | `bad-value`，`reason: 'type'` |
| `repr(forecast.tomorrow)`（收到的是另一行的函数） | `<function of another process>` | — |

Python 没有「解不开」这一类：收到的值不按声明的类型解。

### 5.3 Go：只有不同的地方

```go
// 要 string 的返回值
var text string
if err := forecast.Call(ctx, "tomorrow", &text, "Oslo"); err != nil {
	return err
}
```

| 怎么造成的 | 输出 | 码与 `data` |
| --- | --- | --- |
| `forecast.Call(ctx, "week", nil, "Oslo", math.NaN())` | `forecast.week: argument 2 cannot be sent: the number NaN is not finite` | `Code()` 是 `bad-value`，`Data()` 是 `{part: "argument", argument: 2, reason: "non-finite"}` |
| 方法返回 `make(chan int)` | `weather.today: the result cannot be sent: a chan int cannot cross between processes` | `Code()` 是 `bad-value`，`Data()` 是 `{part: "result", reason: "type"}` |
| 方法返回指回自己的 `*Node`、装着自己的 `[]any` | `weather.today: the result cannot be sent: the value holds itself (a cycle)` | `Code()` 是 `bad-value`，`Data()` 是 `{part: "result", reason: "cycle"}` |
| 方法返回 65 层的值 | `weather.today: the result cannot be sent: the value nests deeper than 64 levels` | `Code()` 是 `bad-value`，`Data()` 是 `{part: "result", reason: "too-deep"}` |
| 调用一个要 `string` 的方法，返回值是对象 | `forecast.tomorrow: the result: json: cannot unmarshal object into Go value of type string` | `Code()` 是 `bad-result`，`Data()` 是 `{service: "forecast", method: "tomorrow", expected: "a string"}` |
| 收到的参数不是声明的类型 | `weather.today: argument 1: json: cannot unmarshal object into Go value of type string` | `Code()` 是 `bad-argument`，`Data()` 是 `{service, method, argument: 1, expected: "a string"}` |

### 5.4 Rust：只有不同的地方

```rust
// 调用另一行的服务，返回值解成声明的类型
let text: String = call_service(&ctx, "forecast", "tomorrow", ("Oslo",)).await?;
```

| 怎么造成的 | 输出 | 码与 `data` |
| --- | --- | --- |
| `call_service(&ctx, "forecast", "week", ("Oslo", f64::NAN)).await` | `forecast.week: argument 2 cannot be sent: the number NaN is not finite` | `bad-value`，`{part: "argument", argument: 2, reason: "non-finite"}` |
| 返回值是 42，要 `String` | ``forecast.tomorrow: the result: invalid type: integer `42`, expected a string`` | `bad-result`，`{service: "forecast", method: "tomorrow", expected: "a string"}` |
| 返回值里有函数，要 `serde_json::Value` | `forecast.tomorrow: the result: the value holds a function that its Rust type cannot represent` | `bad-result`，`{service: "forecast", method: "tomorrow"}` |
| 返回值里有 instant，要 `serde_json::Value` | `forecast.tomorrow: the result: the value holds a tagged value (an instant, bytes, a map, ...) that its Rust type cannot represent` | `bad-result`，`{service: "forecast", method: "tomorrow"}` |

Rust 今天发 NaN 得到 `null`（§1）；改完之后它和别的语言一样报错。serde 的原文没有消失，而是只留在它该在的地方：解不开的时候逐字照搬（§4.5，宿主和开发者在 `bad-result`、`bad-argument` 里读到的就是它）；值传不出去的那几种（非有限数、循环、深度、种类）是 rutis 自己的判断，说的是 rutis 自己的话（§4.2）。

### 5.5 宿主、应用（嵌入 rutis 的程序，或把 `rutis-host` 当运行时的产品）

- 同一个错误跨进程：`code` 与 `data` 原样到达调用方（诊断 §4.4）。宿主按 `code` 分支的程序多一个分支 `bad-result`；`bad-value` 的 `data` 多一个键 `reason`（按名读的代码不用改）。
- `rutis-host call weather.today Oslo` 印的错误行就是 §5.1 那一句，不另外包装。
- 宿主会看到 serde、`encoding/json` 的原文（例如 ``invalid type: integer `42`, expected a string``）。这是有意的：原文最准，rutis 只加前缀（§4 的原则）。
- 发出事件时报告的一行（§4.6）与今天同一位置、同一前缀，只有里面那句话变了。

### 5.6 部署的人

看不到变化：没有新配置键、没有新文件、没有新端口、没有协议版本，`rutis-host` 和运行时照旧升级即可。

### 5.7 要学的概念

这一轮加：一个错误码（`bad-result`）、一个 `data` 字段（`reason`）、一种显示形式（`<… of another process>`）。按 #663 的分层预算，「值」这一层从 2 个概念（`bad-value`、`bad-argument`）到 3 个，不超；同时去掉一个误解：「嵌套过深」不再兼指循环。没有新配置项、没有新 API。

## 6. 兼容与迁移

**文字变了**（不兼容的比较，测试与文档逐字对齐）：

| 今天 | 改成 |
| --- | --- |
| `TypeError: non-finite number`（Node、Bun，发参数时） | `<调用>: argument N cannot be sent: the number NaN is not finite` 等 |
| `NaN cannot cross processes`（Python） | `the number NaN is not finite` |
| `a value that holds itself (a cycle) cannot cross between processes`（Node、Bun） | `the value holds itself (a cycle)` |
| `a value nested deeper than 64 levels cannot cross between processes`（Node、Bun） | `the value nests deeper than 64 levels` |
| `value nested deeper than 64 levels, the most a value crossing processes may nest`（Python、Go、Kotlin） | 同上 |
| `value nested deeper than 64 levels, the most a value crossing processes may nest (a cyclic reference?)`（Swift） | `the value nests deeper than 64 levels`（括号里那句「是循环吧」没有了：循环报循环） |
| `value nested deeper than 64 levels, the most a value crossing a session may nest`（Rust） | 同上 |
| `module cannot cross the boundary`（Python） | `a module cannot cross between processes` |
| `argument 1: the number NaN is not finite: only finite numbers cross`（Go 调用方） | `forecast.week: argument 2 cannot be sent: the number NaN is not finite` |
| `chan int cannot cross between processes`（Go 的返回值） | `weather.today: the result cannot be sent: a chan int cannot cross between processes` |
| `cannot decode {…} as string: json: …`（Go 的返回值） | ``forecast.tomorrow: the result: json: cannot unmarshal object into Go value of type string``（原文不变，只加前缀） |
| ``invalid type: integer `42`, expected a string``（Rust 的返回值） | ``forecast.tomorrow: the result: invalid type: integer `42`, expected a string``（原文不变，只加前缀） |
| `forecast.tomorrow: argument 1 should be a string, got an object`（Rust、Go、Kotlin、Swift 今天的参数） | `forecast.tomorrow: argument 1: <解码器的原文>`——转述没了 |
| `the value holds a live Cordis object or function that its Rust type cannot represent` | `forecast.tomorrow: the result: the value holds a function that its Rust type cannot represent`（`live Cordis` 去掉，其余照旧） |
| `<function from the host>`（Python） | `<function of another process>` |
| `[remote object]`（Node 的对象）、`[Function (anonymous)]`（Node 的函数） | `<object of another process>`、`<function of another process>` |
| `RemoteFunction(7)`、`RemoteObject(7)`（Kotlin） | `<function of another process>`、`<object of another process>` |

**行为变了**：

- Rust 发 NaN / Infinity 从「变成 `null`」改成报错。依赖 `null` 的代码要自己换。
- Go 的返回值在方法里不再换成对端的样子：`Results` 交回原来的值，编码在 `respondTo` 里做（那是唯一知道「这是谁的返回值」的地方）。返回 channel 之类的方法，错误从「方法的 error」变成「这次调用的 `bad-value`」。
- 循环引用在 Python、Go、Kotlin、Swift 里从「嵌套过深」改成「循环」（Swift 括号里那句「是循环吧」也去掉）；`reason` 让程序能分辨。

**码不变**：`bad-value`、`bad-argument`、`bad-result` 三个码，以及 `data` 的字段都不变；凡是底层错误，变的是句话——它现在就是原文，前面只有位置前缀（`weather.ask: `、`argument 2: `、`the result: `）。按码判断的程序一行都不用改。

**文档**：插件 API 指南「值怎样传递」（`docs/guide/plugin-api.md:938`）那一句把 `data` 写成 `{part, argument?, limit?}`；它加 `reason`、加 `payload` 这个 `part`、并把「名字（`BindingError`、`TypeError`……）不作保证，看错误码」说清 `bad-result`，两种语言都改。

**协议不变**：没有新帧、没有新字段、没有抬版本。`bad-value` 的 `data` 多一个 `reason` 键，旧的一端照旧忽略它，因为错误的 `data` 不是按结构解的，它作为普通 JSON 随错误图一起传：

- Rust 收错误时，`Failure` 把整个 `graph` 当作 `Option<serde_json::Value>`（`crates/rutis-bridge/src/session/protocol.rs:87-95`），`Error::data()` 再按键从图上取（`session/mod.rs:199-213` 的 `graph_root` / `graph_value`）；
- 其余每一种语言都从图上按名读：Node、Bun（`node/rutis-runtime/src/errors.mjs:170`、`bun/rutis-bun/src/errors.ts:170`）、Python（`peer.py:114-135` 的 `_code_and_data`）、Go（`peer/errors.go:350` 的 `codeAndData`）、Kotlin（`internal/peer/Errors.kt:408`）、Swift（`Errors.swift:385`）。

`deny_unknown_fields` 在哪里、不在哪里，说清：它在**帧的形状**上——`Frame` 的键、`Failure` 顶层的键（`name`、`message`、`graph`）、`WireValue` 的键（`protocol.rs:31,88,155`）；它不在 `graph` 里面，也不在一个节点的 `data` 的值里，那里是普通 JSON（`{type: "data", value: …}` 的 `value` 原样带过去）。所以往 `data` 里加一个键可以，与 0.18 加 `limit` 一样；往帧或 `Failure` 顶上加一个字段不行。

一个例外要说清：Rust 自己造出来的 `bad-value`，`data` 不是从线上读的，是从文字反推的（`rpc/crossing.rs:61-95` 的 `value_data`）。`reason` 在那里推不出来，所以 `Error::Value` 要显式带上 `reason`，不从文字猜（S4）。

**每条线上的版本**：

| 线 | 版本 | 这份设计 | 旧的一端对新的一端 |
| --- | --- | --- | --- |
| 会话协议 | 4（`node/rutis-runtime/package.json:18`、`bun/rutis-bun/package.json:18`、`node/rutis-runtime/src/session.mjs:58` 的 `ENDPOINT_PROTOCOL`） | 不变：没有新帧、没有新字段、没有新能力 | 旧运行时对新宿主：它照旧说旧话，宿主拿到的码、`data` 与今天一样；新运行时对旧宿主：新说法和 `reason` 照常跨过去，旧宿主从错误图上按名读 `data`（上面那段），`data` 里多一个键它不看也不拒 |
| `rutis.json` | 1（`crates/rutis-host/src/config.rs:78` 的 `FORMAT`） | 不变：没有新键 | 旧文件照读，新文件旧宿主照读 |
| 插件 API | 3（`crates/rutis-loader/src/runtime/go.rs:34`、`swift.rs:33` 的 `PLUGIN_API`，清单里的 `pluginApi`） | 不变：插件要声明的东西没有变化，只有错误的文字与码 | 旧的清单与新宿主、新的清单与旧宿主都照装 |
| 清单格式 | 1（`crates/rutis-loader/src/runtime/binary.rs:186` 的 `MANIFEST_FORMAT`） | 不变 | 同上 |

**新旧混跑**：检查都在本地，一端新一端旧没有任何危险；旧的一端仍用旧的说法（它不知道自己手里的值要怎么说新话），新的一端照新说法。所以 §5 的逐字用例要求两端都是这一版。

**迁移指南**（0.21，两种语言）加五条：Node、Bun 发 NaN 的错误；Python 的循环与 NaN 说法；Go 的返回值与调用方的参数说法；Rust 的 NaN；Swift、Kotlin 的深度那句和引用值的显示。**不采用**：为了文字对齐抬协议版本——文字不跨版本协商，抬了也没用。

## 7. 实施

| Issue | 内容 | 完成的标志 |
| --- | --- | --- |
| S1（#936） | Node、Bun：`checkData` 走进 Map / Set / 记录 / 带标签的值；数字、循环（先于深度）、深度一句话（今天四种说法，§1）；事件的调用描述成事件名（D7）；显示（D8） | `node/rutis-runtime/test/session.test.mjs`、`node/rutis/test/values.test.mjs`、`bun/rutis-bun/test/session.test.ts` 逐字锁住 §5.1 的每一行与 `code`、`data`；事件那条在 `bad-values` 用例里 |
| S2（#936） | Python：`_data` / `_encode` / `_encode_tagged` 带 `seen`；三句统一；`repr` 改 `<… of another process>` | `python/rutis/tests/test_plain_texts.py` 逐字锁住 §5.2 |
| S3（#936） | Go：`toPeerValue` 带 `seen`；`ToPeerArgs` 带上调用名（`Service.Call` 与运行时里已知服务、方法的地方）；`Results` 不再提前换值，返回值的编码与 `ofResult` 在 `respondTo` 里做；`badResult`（`refusals.go` 旁边）；`badArgument` 不再转述 `encoding/json`，只加 `argument N: ` 前缀（`Expected`／`actual` 只留给 `data`） | `go/rutis/internal/convert`、`internal/peer` 的用例逐字锁住 §5.3；`go/rutis/internal/fixtures/session` 里加一条 channel 返回值的用例 |
| S4（#936） | Rust：`Exact` 的 `serialize_f32` / `serialize_f64` 查有限；`Exact` 自己数层级（`Rc` 指回自己时以 `cycle` 失败），`fits` 那句说法换掉；`call_service` 把解不开的返回值包成 `bad-result`：serde 原文逐字保留，只加 `the result: ` 前缀；`of_decoding` 不再转述成 `should be … got …`；`objects.rs:809` 那句去掉 `live Cordis` | `crates/rutis-bridge/src/session/rpc/tests.rs` 与 `objects.rs` 的用例逐字锁住 §5.4 |
| S5（#936） | Swift：`Codec/ValueCodec.swift`（编码、`:147-148`）与 `Session/Peer.swift`（收值、`:662,741-742`）加 `seen`，循环先于深度，三句话统一；`ValueCodec` 的 `cannot decode … into …` 是 Swift 自己造的句子，照旧，只加前缀；`Errors.swift:385` 的 `data` 照旧按数据读 | `swift/rutis/Tests/SelfTest/PeerTests.swift`（今天 `:96` 只查「包含 nested deeper」）逐字锁住属于 Swift 的行 |
| S6（#936） | Kotlin：`internal/peer/Peer.kt:420,508` 两处编码加 `seen`，`tooDeep()`（`:63-65`）换句；生成的参数解码保留 kotlinx.serialization 的原文（`Refusals.kt:32-45` 不再拼 `should be … got …`）；`advanced/RemoteFunction.kt:25`、`RemoteObject.kt:29` 的 `toString` 改 `<… of another process>` | `kotlin/rutis/rutis/src/jvmTest/kotlin/arcships/rutis/PeerTest.kt`、`ErrorTextTest.kt` 逐字锁住属于 Kotlin 的行 |
| S7（`bad-result` 与行名） | 诊断设计 §4.3 的码表只动这份设计的两行（§4.3；其余属于 #937，后落地的一份 rebase）；引用值带行名（新的协议改动，`deny_unknown_fields` 决定它必须先抬版本或先加一条能力，另写一个 issue） | 码表更新；行名那一条在它自己的设计里定 |
| S8（#936 的完成条件） | 不加新机制：§1 的每一行由 S1–S6 各自的用例逐字锁住，六个运行时各写自己那一行（下段说明为什么不走共用用例表） | 六个阶段的用例合起来覆盖 §1 的每一行，连同 `code` 与 `data`；一行改了而用例没改就红 |

#### 共用用例表为什么不动

`crates/rutis-bridge/src/runtime/testing/weather.expected.json` 的 `throws` 只写 `{}` 或 `{name, message}`——文件自己的 `about` 这么写，读它的代码也只比这两项（`crates/rutis-bridge/src/runtime/testing.rs:178-195` 的 `check`）。把 `code` 与 `data` 加进去要同时改三处：`check`、那份文件的 `about`，以及三个测试工具里各自的 `throws` 结构（Go `go/rutis/rutistest/conformance_test.go:132-196`、Kotlin `kotlin/rutis/rutis-conformance/src/test/kotlin/arcships/rutis/conformance/WeatherConformanceTest.kt:53-67`、Swift `swift/rutis/Tests/SelfTest/WeatherConformanceTests.swift:64-72`），而三个工具又不模拟跨进程、靠 `crossing: true` 跳过这类 sample。那是共用用例表自己的一件事，跟本设计的六个阶段无关，所以这份设计不改它：§1 的每一行落在各语言自己的用例里（S1–S6 的「完成」一列写着文件），要一张跨语言的值错误表时，它自己另写一份设计。

每个 issue 的「完成」都包含：§5 里属于它的每一行都有用例逐字比较；旧的说法（§6 的表）在仓库里不再出现。
