# 插件 API

TypeScript / JavaScript 插件（`@arcships/rutis`）、Python 插件（`rutis`）和 Go 插件（`github.com/arcships/rutis/go/rutis`，见 [写一个 Go 插件](go-plugin.md)）的写法与规则相同。当前插件 API 版本：**2**。

## 声明

| 声明 | TypeScript（`definePlugin({...})`） | Python（`define_plugin(apply, ...)` 或模块变量） | Go（`rutis.Define(rutis.Plugin[C]{...})`） | 含义 |
| --- | --- | --- | --- | --- |
| 用到的服务 | `inject: ['llm']` | `inject=["llm"]` | `Inject: []string{"llm"}` | 都就绪才启动；任何一个撤销，插件停下，等它回来再启动 |
| 提供的服务（可选） | `provides: { weather: { today: 'sync' } }` | `provides={"weather": Weather}` 或 `{"weather": {"today": "sync"}}` | `Provides: rutis.Provides{"weather": rutis.MethodsOf[*Weather](rutis.Sync("Unit"))}`（导出方法，除 `Sync` 列出的外都是 `async`） | 不写也行：`ctx.provide` 提供的服务自动对外（见下）。写了就按声明：把方法标成 `sync`，并让 `rutis-host check` 在运行前就知道谁提供什么；声明过的服务只能调用声明过的方法 |
| 配置 | `config: { type: 'object', ... }` | `config={...}`（模块变量写 `Config`） | 类型 `C`（Schema 由它生成），或 `Schema` | JSON Schema；宿主据此校验和展示配置 |
| 启动 | `apply(ctx, config)` | `apply(ctx, config)` | `Apply: func(ctx *rutis.Ctx, config C) error` | 可以是 async；返回清理函数或不返回（Go：返回错误即装载失败） |

## ctx

| 方法 | 作用 |
| --- | --- |
| `ctx.use(name)` | `inject` 里声明的服务。同一个运行时进程里的插件提供的，是对象本身；其他进程、其他语言、其他机器提供的，是代理 |
| `ctx.provide(name, value)` | 提供服务，直到插件卸载，或调用返回的函数。服务自动对外，别的语言、别的进程按名字使用；`provides` 没有声明它时，方法取 `value` 上的公开方法（JavaScript：自身和原型链上的函数，不含 `constructor`；Python：不以 `_` 开头的可调用属性；Go：导出方法），都按 `async` 调用，字段不算方法 |
| `ctx.effect(cleanup)` | 插件卸载时运行 `cleanup`（可以是 async） |
| `ctx.on(name, listener)` | 监听运行时进程共享事件总线上的事件：宿主发进来的（运行时的 `emits` 声明名字，见[在 Rust 应用里嵌入](rust-host.md)），以及进程里插件 `ctx.emit` 的。监听随插件卸载注销；返回的函数立即注销 |
| `ctx.emit(name, ...args)` | 向本运行时进程的事件总线分发事件，供本插件和同进程的其他插件监听；同步调用监听器，不等待它们返回的 Promise / 协程（Go 的接口见下） |

Node、Bun、Python 和 Go 都支持叶子插件事件（插件 API 2），并报告 `events` 运行时能力。Node 和 Bun 用 Cordis 自己的事件服务（Bun 的每个 row 持一个 Cordis fiber），因此和同进程的 Cordis 插件共享一条总线、分发语义与 Cordis 一致；Python 和 Go 的总线按同一套语义实现（对照 `cordis/src/events.ts` 与 `crates/rutis/src/bus.rs`）。Go 的接口是 `ctx.On(name, func(context.Context, ...any) error) func()` 和 `ctx.Emit(callerCtx, name, args...) error`；监听器收到保留当前调用链、随所属插件卸载取消的 context；卸载等正在执行的监听器结束后才运行清理。插件方法里发事件时传收到的 `callerCtx`，在 `Apply` 里可传 `ctx` 本身。

本地发事件按注册顺序调用监听器：Node、Bun 和 Python 的 `ctx.emit` 同步抛错时停止后续监听器；返回的 Promise / 协程不等待，未处理的异步失败结束运行时进程。Go 的 `ctx.Emit` 同步执行，遇到首个错误返回；插件另起的 goroutine 沿用 `ctx.Go` 的约定。宿主 `Process::emit` 则等待全部监听器，并以 `AggregateError` 汇总错误；Go 为这些监听器启动 goroutine，因此不保证宿主事件监听器的执行顺序。与 Rust 的 `Ctx::dispose_self` 同一约定：监听器不得等待自己所属的 row 卸载完成（Go 的监听器经宿主回调同步卸载或更新自身行会等到自己），要卸载就结束监听器之后再做。

一个跨过会话的错误被再次抛出时，Node、Bun、Python 和 Go 都带着它完整的错误图（cause、`AggregateError` 的子错误）再跨一次，不限于聚合的情形：Node 解码出的是真正的 `Error` 对象，重新编码时自然带上整个图，Python 和 Go 的 `encode_error` / `encodeError` 把 `RemoteError` 的图拼进去（节点追加、引用偏移）以保持一致。

`internal/*` 属于运行时，不能监听、发出或列在 `emits` 中。`RuntimePlugin` / `LocalRuntime` 设置了 `emits`，而运行时未报告 `events` 时，挂载失败（`lacks events`）。这项能力检查不用于直接 `Process::mount` / `attach` 的 Cordis 静态挂载：旧版 Node 已支持 Cordis 事件，却没有这项能力声明。

`emits` 只声明宿主 → 运行时。叶子插件的 `ctx.emit` / `ctx.Emit` 在进程内分发；`RuntimePlugin` 没有提供 `Mount.events` / `EventSink` 配置入口，所以运行时 → Rust 宿主的方向暂不可用。

Node 和 Python 的测试工具用运行时自己的那份事件实现：Node 的测试工具与运行时同样走 Cordis，Python 的测试工具用 `rutis.events`。测试工具也在加载时声明事件：Node 的 `load(plugin, { emits })`、Python 的 `load(plugin, emits=[...])`、Go 的 `rutistest.Load(t, plugin, config, services, "event", ...)`。宿主发入的入口分别是 `await t.emit(...)`、`await t.emit(...)` 和 `loaded.Emit(callerCtx, name, args...)`，都校验白名单并等待监听器。Python 测试工具在卸载时取消并收尾未完成的本地异步监听器；已有测试体或清理异常时保留原始异常，否则报告监听失败。真实运行时对未处理的异步失败退出进程。

清理按注册的相反顺序运行：`apply` 返回的清理函数最先，然后是 `ctx.effect` 注册的。

Go 里：`ctx.Use(name, &target)` 把服务绑定到函数字段的结构体（或 `*rutis.Service`）；`ctx.Provide(name, value)`；`ctx.Effect(func(context.Context) error)`；`ctx.Go(task)` 起一个卸载时停下的 goroutine。`*rutis.Ctx` 是插件卸载时取消的 `context.Context`。

## 生命周期

插件不决定自己什么时候运行，宿主决定：

- `inject` 的服务都就绪时启动（`apply`）；
- 任何一个撤销时停下（运行清理），它回来时再启动；
- 配置变了：重启（运行清理，再用新配置 `apply`）；
- 插件提供的服务被撤销时，用到它的插件先停下，提供者后停。

所以 `apply` 里拿到的服务在插件运行期间一直有效，不需要处理"服务中途消失"。

一个服务名在 rutis 里只有一个提供者：别的插件（任何语言、任何节点）已经提供同名服务时，后提供的插件失败（装载时提供的装载失败，装载后才提供的在提供时失败），错误写明双方，例如 `service weather is already provided by row forecast: row weather cannot provide it too`。没有声明的服务在插件运行后才出现，依赖它的插件照常等它出现；`rutis-host check` 对这类依赖报 `may be provided at runtime by …`，不算失败。

宿主停止时卸载所有插件，每个清理执行一次，然后运行时进程退出。宿主没有卸载就没了（被强制结束、崩溃）时，运行时发现和宿主的连接断开，自己卸载所有插件，然后退出进程；插件留下的定时器、套接字、线程、goroutine 不会让它继续运行。

清理有截止时间：宿主停止时是 `rutis-host` 的 `--shutdown-timeout`（默认 10 秒，见 [rutis-host](rutis-host.md#停止与退出码)）；宿主没了时，运行时用同一个时间（`rutis-host` 把它放在环境变量 `RUTIS_SHUTDOWN_TIMEOUT` 里传给运行时，没有时 10 秒）。过了截止时间还没完成的清理，以及排在它后面的，不再等待：运行时在 stderr 写一行说明，以退出码 1 退出。所以必须完成的工作不要只靠清理；清理里不要无限等待。Node 和 Bun 里，同步阻塞的清理（死循环）占住线程，截止时间无法打断。

## 值怎样传递

插件和它用到的服务可能不在同一个进程。跨进程时：

| 值 | 怎样传递 |
| --- | --- |
| `null` / `None`、布尔、数字、字符串 | 复制 |
| 数组 / list、普通对象 / 字符串键的 dict | 递归复制 |
| Python dataclass | 复制为 dict |
| 日期时间、二进制、Map、Set、大整数、Decimal、UUID | 复制，各语言收到本语言的类型，见下面的[十种值](#十种值) |
| `Error` / 异常 | 作为值传递时复制为数据 `{ name, message }`（TypeScript 另带 `stack`）；抛出时另一端得到同名、同消息的错误（TypeScript 中内置错误类保留类，其余是 `Error`；Python 中是 `rutis.peer.RemoteError`，`name` 是原来的类名）；JavaScript 或 Rust 服务抛出的错误上的字符串 `code` 和与它一起的 `data` 也会传过去（各运行时再抛出时继续传），即收到的错误的 `code` 和 `data`（Go 中为 `Code`、`Data`），[导出的 Rust 服务](cordis.md#把-rust-服务导出给行)的 `Err(e)` 就带着它们。其他属性不传 |
| 函数、Promise / 协程 | 按引用：另一端调用或 await 的是原来那个 |
| 带方法的对象、类的实例 | 按引用：另一端拿到代理，调用的是原对象 |
| 数据里的 `undefined` | 变成 `null`（Python 收到 `None`） |
| Go 的结构体、切片、map | 复制：结构体字段、切片和 map 的元素逐个按本节的规则传，字段名遵循 `json` 标签；结构体一律是数据，要按引用传对象写 `rutis.Ref(v)`。`map[string]T` 是对象，键不是字符串的 map 是 Map，`map[K]struct{}` 是 Set |
| 超出 ±(2^53−1) 的整数 | 自动按大整数传，见[十种值](#十种值) |
| symbol、循环引用、`NaN` / `Infinity`、JS 的其他类型化数组（如 `Int16Array`）、带时区的 Python `time`、Go 的 channel | 不能跨进程（调用失败） |
| 嵌套超过 64 层的值 | 不能跨进程（这次调用失败，见下） |

嵌套深度：一个值最多嵌套 **64 层**。数组 / list / tuple、对象 / dict（含 dataclass）、Map、Set、Go 的切片、map、结构体各算一层，里面有没有引用都一样；作为值传递的 `Error` / 异常也算一层。调用的参数列表本身不算一层：每个参数各自最多 64 层；返回值最多 64 层。更深的值由**要发送它的一方**在发出之前拒绝，只让这次调用失败，错误信息说明上限（`value nested deeper than 64 levels`）：

- 作为参数：调用方的这次调用失败（Rust 中是 `Error::Value`），对端不执行它；
- 作为返回值：调用方收到被调方抛出的错误；
- 会话继续，同一会话上的其他调用（包括同时进行的）不受影响。

五个运行时（Rust、Node、Bun、Python、Go）和测试工具的严格模式都按这个上限在发送时检查。上限约束的是发送：接收的 Rust 一侧为不遵守上限的对端（例如更早的版本）留了余量，线上不超过 208 层的帧照常读取，所以从这样的对端**收到**的值可能超过 64 层（最多约 100 层含引用的数组、约 206 层数据）；这部分余量不保证，插件不要依赖。帧更深、Rust 一侧不读时：能看出它属于哪个调用，就只让那个调用失败（对端的调用收到错误；对本侧调用的回复让本侧的调用失败），其中的引用直到会话结束才释放；看不出属于哪个调用时，结束会话并说明原因。见 [#279](https://github.com/arcships/rutis/issues/279)。

数字有下面这些边界。它们是现在的行为，**不保证**以后不变（明确不保证），依赖它们的插件需要自己处理：

- JavaScript（Node、Bun）里数字是 IEEE 754 双精度：JS 自己的 `number` 超出 ±(2^53−1) 时本来就不精确，照原样发出；要精确用 `bigint`。`-0` 变成 `0`；整数值的浮点数（Python 的 `2.0`）到 JS 一端就是 `2`，再传出去是整数。
- Python 保留 `-0.0`。
- NaN、±Infinity 不能跨进程。

同一个进程里没有复制，所以"传过去之后改原对象，另一端也看到"这类写法在单元测试里可能成立、跨进程就不成立。测试工具的严格模式（默认）按上表传值，提前暴露这种问题。

### 十种值

| 值 | JS（Node、Bun） | Python | Go | Rust（`rutis-bridge`） |
| --- | --- | --- | --- | --- |
| 时间点 | `Date` | 带时区的 `datetime`（收到为 UTC） | `time.Time`（收到为 UTC） | `Instant` |
| 本地日期时间 | 字符串 `"2026-10-15T14:30:05"` | 无时区的 `datetime` | `rutis.DateTime` | `DateTime` |
| 日期 | 字符串 `"2026-10-15"` | `date` | `rutis.Date` | `Date` |
| 时刻 | 字符串 `"14:30:05"` | 无时区的 `time` | `rutis.TimeOfDay` | `TimeOfDay` |
| 二进制 | `Uint8Array`（也可发 `Buffer`、`ArrayBuffer`、`DataView`） | `bytes`（也可发 `bytearray`、`memoryview`） | `[]byte` | `Vec<u8>`（生成代码里是 `Bytes`） |
| Map | `Map` | 键不是字符串的 `dict`（收到的是 `dict` 的子类） | 键不是字符串的 map（解码到 `any` 时是 `map[any]any`） | `Vec<(Value, Value)>`（生成代码里是 `ValueMap`） |
| Set | `Set` | `set`（也可发 `frozenset`） | `map[K]struct{}`，或切片（解码到 `any` 时是切片） | `Vec<Value>`（生成代码里是 `ValueSet`） |
| 大整数 | `bigint` | `int`（收到的是 `int` 的子类） | `*big.Int`，或放得下的整数类型 | `BigInt`，或放得下的整数类型 |
| Decimal | 字符串 `"12.30"` | `Decimal` | `rutis.Decimal` | `Decimal` |
| UUID | 字符串（小写） | `UUID` | `rutis.UUID` | `Uuid` |

- 插件只写本语言原生的代码，转换由 rutis 做。收到的大整数、Map 在 Python 里是 `int`、`dict` 的子类，照常使用；原样转发时仍是大整数、Map。
- JS 没有本地日期时间、日期、时刻、Decimal、UUID 的类型，收到的是普通字符串，内容完整，`===` 照常可用。这样的值经 JS 原样转发时类型不保留（Python 发出的 `date` 经 JS 转发回来是 `str`）。要在 JS 里凭空创建这五种值，或以原类型转发，用 `@arcships/rutis` 的构造函数：`rutis.datetime('2026-10-15T14:30')`、`rutis.date('2026-10-15')`、`rutis.time('14:30')`、`rutis.decimal('1.10')`、`rutis.uuid('…')`；普通字符串照旧是字符串。Go 也可以把这五种值解码进 `string`。
- 整数：超出 ±(2^53−1)（即绝对值大于 9007199254740991）的整数（Python `int`、Go `int64` / `uint64`、Rust 整数）自动按大整数传，不需要特殊写法；JS 收到时是 `bigint`，范围内仍是 `number`。收到的大整数放得下时可以解码进声明的整数类型，放不下时这次调用失败。`bool` 不受影响。
- 时间：年份 0001–9999。本地日期时间没有时区，原样传递，不猜时区，也不转成时间点。时间点按接收方的精度向负无穷截断：JS 到毫秒，Python 到微秒，Go、Rust 到纳秒；Python 的本地日期时间和时刻同样截断到微秒。
- Decimal 保留末尾零（`1.10` 与 `1.1` 不同），不用指数；NaN、无穷不能跨进程。大整数、Decimal 至多 4300 位。
- Map 的键、Set 的元素只能是字符串、有限数字、布尔、大整数、本地日期时间、日期、时刻、Decimal、UUID；时间点、二进制、`null`、数组、对象、Map、Set、引用不行，发送方在发出前拒绝。接收方转换后两个键相等时（例如 JS `Map` 的键 `1` 和 `true` 到 Python），这次调用失败：会丢条目。字符串键的 JS `Map` 仍按 Map 传，Python 普通的字符串键 `dict` 按对象传。
- 每种值只发给声明了它的对端。对端不支持时（例如 0.11 之前的运行时，包括超出 ±(2^53−1) 的整数），只让这次调用失败，错误说明哪种值，会话继续，其他调用不受影响。节点之间转发的事件不承载这十种值；同一运行时进程内的叶子事件照常传递。

### 测试工具与真实运行时的差异

测试工具（`@arcships/rutis/testing`、`rutis.testing`、Go 的 `rutistest`）和真实运行时对同一份期望文件比对：对同一个插件做同一组调用（同步、异步、回调、错误、上表的值），结果必须相同（[`weather.expected.json`](../../crates/rutis-bridge/src/runtime/testing/weather.expected.json)）。严格模式下已知的差异只有下面这些：

| 差异 | 原因 |
| --- | --- |
| 结束进程的插件（`process.exit`、`os._exit`）结束的是测试本身 | 测试工具在测试的进程里运行插件；期望文件里这一条标为 `allowed`，测试工具不做这个调用 |
| 不重启：配置变化、注入的服务撤销都不会让插件停下再启动 | `load` 只启动一次，生命周期由宿主决定（见上） |
| 同步调用不阻塞，不会出现 `SyncWaitCycle`；没有取消（`AbortSignal`、协程取消） | 插件与测试在同一个事件循环里 |
| Python：跨过边界的异常是 `RemoteError`，它的 `__cause__` 是原来的异常 | 方便调试；真实运行时的 `RemoteError` 没有 `__cause__` |
| TypeScript：只能加载 `definePlugin` 写的插件，不能加载 Cordis 插件（事件用的是 Cordis 的事件服务，插件模型仍不是 Cordis 的） | 测试工具只挂一个插件，没有服务与依赖门控 |
| TypeScript / Python 事件：被测插件只听到自己 `ctx.emit` 和 `t.emit` 发的事件；真实运行时整个进程共享一条事件总线，同进程其他插件（Node 上还有 Cordis 插件）发的事件也会听到 | 测试工具只加载一个插件。`t.emit` 只接受加载时 `emits` 声明的名字，与 `RuntimePlugin::emits` 相同；两边都拒绝 `internal/*` |
| Python 工具在卸载时取消并收尾本地异步监听，测试体或清理异常优先；真实运行时在未处理的异步失败时退出进程 | 避免结束测试进程，同时让测试明确失败 |

十种值在两边按同样的规则传递，这不是差异。关闭严格模式（`strict: false`）时值不经过上表，以上之外的差异都可能出现。

## 同步调用与可重入

声明为 `sync` 的方法同步返回结果，跨进程时调用方阻塞等待。期间：

- 被调用的一方如果反过来调用调用方（回调），调用能到达正在等待的调用方并执行；
- Python 运行时在同步等待期间也执行其他进来的调用，所以 Python 插件的服务可能在它自己正处于一次同步调用之中时被调用：**调用别的服务时不要持有锁**；
- Go 运行时每个进来的调用都在自己的 goroutine 上执行，所以总是可重入：Go 插件提供的对象要能被并发调用。调用链放在 `context.Context` 里：调用别的服务时**把收到的 ctx 传下去**，否则正在同步等你的调用方可能认不出你的回调。
- Node 运行时在同步等待期间只执行属于这条调用链的调用，其他调用放到它的栈清空之后。
- Python、Node（和 Bun）运行时在同步等待期间执行的调用压在等待的调用之上：先开始的调用要等后开始的结束。

几个运行时互相同步调用时，可能所有参与方都在等别人：两个 Node 运行时互相同步调用；一个 Node 运行时和两个 Python 运行时嵌套调用，也可能如此。宿主在转发每个同步调用之前检查它会不会形成这样的等待环；会的话不转发，调用方得到 `SyncWaitCycle` 错误，其余调用照常完成。收到它时不要立即重试同一个调用（多半会再次形成同一个环），可以改用 `async` 方法、换个时机，或者把它当作失败处理。

检查只涵盖声明了 `sync-wait` 能力的运行时（本仓库的 Node、Python、Bun、Go 运行时；协议见[多语言设计](../design-multilang-runtimes-2026-10-03.md) §九）。没有声明的运行时（旧版 SDK）发出的同步调用不在检查之内，与以前一样可能卡死；不声明 `reentrant-sync` 的运行时按不可重入看待。跨运行时、可能交叉的调用仍然首选 `async` 方法。

需要等待事件循环推进的结果（例如在同步方法里等一个 Promise）不能放在同步方法里，会得到 `SyncWaitCycle` 错误：改成 `async`。

`async` 方法在回复之前就执行到它的第一个 `await`，和 JS 的 async 函数一样；从不挂起的 `async` 方法因此在同步调用链上也能被等待。各个支持的 Python 版本都是这样（3.12 起是 asyncio 自带的 eager start，3.10、3.11 由 rutis 实现）。

## 取消

async 方法的调用方放弃等待（例如超时）时，调用被取消：被调用的 Python 协程被取消；Go 方法的 `context.Context` 被取消；JavaScript 方法在调用方传了 `AbortSignal` 参数时收到 abort（Rust 绑定调用 Cordis 方法时会传）。没有这两种情况时，被调用的方法照常运行完，结果被丢弃。

## 版本

插件上标记着它所用的插件 API 版本（`definePlugin` / `define_plugin` / `rutis.Define` 自动标记）。运行时比插件旧时，加载失败并说明需要升级什么；插件依赖 SDK 的大版本范围即可，不需要与宿主版本对齐。

插件 API 2 增加叶子事件。新版运行时继续接受 API 1；升级 SDK 后生成的 API 2 插件需要一起升级运行时，Go 二进制还需要支持 API 2 的宿主。协议版本和 Go manifest 格式不变。
