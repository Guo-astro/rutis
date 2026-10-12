# 插件 API

TypeScript / JavaScript 插件（`@arcships/rutis`）、Python 插件（`rutis`）和 Go 插件（`github.com/arcships/rutis/go/rutis`，见 [写一个 Go 插件](go-plugin.md)）的写法与规则相同。当前插件 API 版本：**1**。

## 声明

| 声明 | TypeScript（`definePlugin({...})`） | Python（`define_plugin(apply, ...)` 或模块变量） | Go（`rutis.Define(rutis.Plugin[C]{...})`） | 含义 |
| --- | --- | --- | --- | --- |
| 用到的服务 | `inject: ['llm']` | `inject=["llm"]` | `Inject: []string{"llm"}` | 都就绪才启动；任何一个撤销，插件停下，等它回来再启动 |
| 提供的服务 | `provides: { weather: { today: 'async' } }` | `provides={"weather": Weather}` 或 `{"weather": {"today": "async"}}` | `Provides: rutis.Provides{"weather": rutis.MethodsOf[*Weather](rutis.Sync("Unit"))}`（导出方法，除 `Sync` 列出的外都是 `async`） | 名字和每个方法的种类（`sync` / `async`）。只有声明过的方法能被别的插件调用 |
| 配置 | `config: { type: 'object', ... }` | `config={...}`（模块变量写 `Config`） | 类型 `C`（Schema 由它生成），或 `Schema` | JSON Schema；宿主据此校验和展示配置 |
| 启动 | `apply(ctx, config)` | `apply(ctx, config)` | `Apply: func(ctx *rutis.Ctx, config C) error` | 可以是 async；返回清理函数或不返回（Go：返回错误即装载失败） |

## ctx

| 方法 | 作用 |
| --- | --- |
| `ctx.use(name)` | `inject` 里声明的服务。同一个运行时进程里的插件提供的，是对象本身；其他进程、其他语言、其他机器提供的，是代理 |
| `ctx.provide(name, value)` | 提供服务，直到插件卸载，或调用返回的函数 |
| `ctx.effect(cleanup)` | 插件卸载时运行 `cleanup`（可以是 async） |

清理按注册的相反顺序运行：`apply` 返回的清理函数最先，然后是 `ctx.effect` 注册的。

Go 里：`ctx.Use(name, &target)` 把服务绑定到函数字段的结构体（或 `*rutis.Service`）；`ctx.Provide(name, value)`；`ctx.Effect(func(context.Context) error)`；`ctx.Go(task)` 起一个卸载时停下的 goroutine。`*rutis.Ctx` 是插件卸载时取消的 `context.Context`。

## 生命周期

插件不决定自己什么时候运行，宿主决定：

- `inject` 的服务都就绪时启动（`apply`）；
- 任何一个撤销时停下（运行清理），它回来时再启动；
- 配置变了：重启（运行清理，再用新配置 `apply`）；
- 插件提供的服务被撤销时，用到它的插件先停下，提供者后停。

所以 `apply` 里拿到的服务在插件运行期间一直有效，不需要处理"服务中途消失"。

## 值怎样传递

插件和它用到的服务可能不在同一个进程。跨进程时：

| 值 | 怎样传递 |
| --- | --- |
| `null` / `None`、布尔、数字、字符串 | 复制 |
| 数组 / list、普通对象 / 字符串键的 dict | 递归复制 |
| Python dataclass | 复制为 dict |
| `Error` / 异常 | 作为值传递时复制为数据 `{ name, message }`（TypeScript 另带 `stack`）；抛出时另一端得到同名、同消息的错误（TypeScript 中内置错误类保留类，其余是 `Error`；Python 中是 `rutis.peer.RemoteError`，`name` 是原来的类名），其他属性不传 |
| 函数、Promise / 协程 | 按引用：另一端调用或 await 的是原来那个 |
| 带方法的对象、类的实例 | 按引用：另一端拿到代理，调用的是原对象 |
| symbol、bigint、Map、Set、TypeScript 的 Date、二进制、循环引用、`NaN` / `Infinity` | 不能跨进程（调用失败）。Map、Set、Date、二进制、bigint 是否改为可传递由 [#308](https://github.com/arcships/rutis/issues/308) 决定，在此之前真实运行时和测试工具都拒绝它们 |
| TypeScript 数据（不含函数、带方法的对象）里的 `undefined` | 变成 `null` |
| Go 的结构体、切片、`map[string]T` | 按 JSON 复制（遵循 `json` 标签）；结构体一律是数据，要按引用传对象写 `rutis.Ref(v)`。超出 ±2^53−1 的整数、`[]byte`、channel、非字符串键的 map 不能跨进程 |
| 嵌套超过 64 层的值 | 不能跨进程（这次调用失败，见下） |

嵌套深度：一个值最多嵌套 **64 层**。数组 / list / tuple、对象 / dict（含 dataclass）、Go 的切片、map、结构体各算一层，里面有没有引用都一样；作为值传递的 `Error` / 异常也算一层。调用的参数列表本身不算一层：每个参数各自最多 64 层；返回值最多 64 层。更深的值由**要发送它的一方**在发出之前拒绝，只让这次调用失败，错误信息说明上限（`value nested deeper than 64 levels`）：

- 作为参数：调用方的这次调用失败（Rust 中是 `Error::Value`），对端不执行它；
- 作为返回值：调用方收到被调方抛出的错误；
- 会话继续，同一会话上的其他调用（包括同时进行的）不受影响。

五个运行时（Rust、Node、Bun、Python、Go）和测试工具的严格模式都按这个上限在发送时检查。上限约束的是发送：接收的 Rust 一侧为不遵守上限的对端（例如更早的版本）留了余量，线上不超过 144 层的帧照常读取，所以从这样的对端**收到**的值可能超过 64 层（最多约 70 层含引用的数组、约 142 层数据）；这部分余量不保证，插件不要依赖。帧更深、Rust 一侧不读时：能看出它属于哪个调用，就只让那个调用失败（对端的调用收到错误；对本侧调用的回复让本侧的调用失败），其中的引用直到会话结束才释放；看不出属于哪个调用时，结束会话并说明原因。见 [#279](https://github.com/arcships/rutis/issues/279)。

数字有下面这些边界。它们是现在的行为，**不保证**以后不变（明确不保证），依赖它们的插件需要自己处理：

- JavaScript（Node、Bun）里数字是 IEEE 754 双精度：超出 ±(2^53−1) 的整数变成最接近的双精度值（2^53+1 到 JS 一端是 2^53）；`-0` 变成 `0`；整数值的浮点数（Python 的 `2.0`）到 JS 一端就是 `2`，再传出去是整数。需要精确的大整数时用字符串传。
- Python 保留 64 位范围内的整数和 `-0.0`。
- NaN、±Infinity 不能跨进程。

同一个进程里没有复制，所以"传过去之后改原对象，另一端也看到"这类写法在单元测试里可能成立、跨进程就不成立。测试工具的严格模式（默认）按上表传值，提前暴露这种问题。

### 测试工具与真实运行时的差异

测试工具（`@arcships/rutis/testing`、`rutis.testing`）和真实运行时对同一份期望文件比对：对同一个插件做同一组调用（同步、异步、回调、错误、上表的值），结果必须相同（[`weather.expected.json`](../../crates/rutis-bridge/src/runtime/testing/weather.expected.json)）。严格模式下已知的差异只有下面这些：

| 差异 | 原因 |
| --- | --- |
| 结束进程的插件（`process.exit`、`os._exit`）结束的是测试本身 | 测试工具在测试的进程里运行插件；期望文件里这一条标为 `allowed`，测试工具不做这个调用 |
| 不重启：配置变化、注入的服务撤销都不会让插件停下再启动 | `load` 只启动一次，生命周期由宿主决定（见上） |
| 同步调用不阻塞，不会出现 `SyncWaitCycle`；没有取消（`AbortSignal`、协程取消） | 插件与测试在同一个事件循环里 |
| Python：跨过边界的异常是 `RemoteError`，它的 `__cause__` 是原来的异常 | 方便调试；真实运行时的 `RemoteError` 没有 `__cause__` |
| TypeScript：只能加载 `definePlugin` 写的插件，不能加载 Cordis 插件 | 测试工具不包含 Cordis |

Map、Set、Date、二进制、bigint 两边都拒绝，这不是差异；它们以后是否可以传递见 [#308](https://github.com/arcships/rutis/issues/308)。关闭严格模式（`strict: false`）时值不经过上表，以上之外的差异都可能出现。Go 的 `rutistest` 和 Bun 尚未对这份期望文件比对。

## 同步调用与可重入

声明为 `sync` 的方法同步返回结果，跨进程时调用方阻塞等待。期间：

- 被调用的一方如果反过来调用调用方（回调），调用能到达正在等待的调用方并执行；
- Python 运行时在同步等待期间也执行其他进来的调用，所以 Python 插件的服务可能在它自己正处于一次同步调用之中时被调用：**调用别的服务时不要持有锁**；
- Go 运行时每个进来的调用都在自己的 goroutine 上执行，所以总是可重入：Go 插件提供的对象要能被并发调用。调用链放在 `context.Context` 里：调用别的服务时**把收到的 ctx 传下去**，否则正在同步等你的调用方可能认不出你的回调。
- Node 运行时在同步等待期间只执行属于这条调用链的调用，其他调用放到它的栈清空之后。
- Python、Node（和 Bun）运行时在同步等待期间执行的调用压在等待的调用之上：先开始的调用要等后开始的结束。

几个运行时互相同步调用时，可能所有参与方都在等别人：两个 Node 运行时互相同步调用；一个 Node 运行时和两个 Python 运行时嵌套调用，也可能如此。宿主在转发每个同步调用之前检查它会不会形成这样的等待环；会的话不转发，调用方得到 `SyncWaitCycle` 错误，其余调用照常完成。收到它时不要立即重试同一个调用（多半会再次形成同一个环），可以改用 `async` 方法、换个时机，或者把它当作失败处理。

检查只涵盖声明了 `sync-wait` 能力的运行时（本仓库的 Node、Python、Bun 运行时；协议见[多语言设计](../design-multilang-runtimes-2026-10-03.md) §九）。没有声明的运行时（旧版 SDK、Go 运行时）发出的同步调用不在检查之内，与以前一样可能卡死；不声明 `reentrant-sync` 的运行时按不可重入看待。跨运行时、可能交叉的调用仍然首选 `async` 方法。

需要等待事件循环推进的结果（例如在同步方法里等一个 Promise）不能放在同步方法里，会得到 `SyncWaitCycle` 错误：改成 `async`。

`async` 方法在回复之前就执行到它的第一个 `await`，和 JS 的 async 函数一样；从不挂起的 `async` 方法因此在同步调用链上也能被等待。各个支持的 Python 版本都是这样（3.12 起是 asyncio 自带的 eager start，3.10、3.11 由 rutis 实现）。

## 取消

async 方法的调用方放弃等待（例如超时）时，调用被取消：被调用的 Python 协程被取消；Go 方法的 `context.Context` 被取消；JavaScript 方法在调用方传了 `AbortSignal` 参数时收到 abort（Rust 绑定调用 Cordis 方法时会传）。没有这两种情况时，被调用的方法照常运行完，结果被丢弃。

## 版本

插件上标记着它所用的插件 API 版本（`definePlugin` / `define_plugin` / `rutis.Define` 自动标记）。运行时比插件旧时，加载失败并说明需要升级什么；插件依赖 SDK 的大版本范围即可，不需要与宿主版本对齐。
