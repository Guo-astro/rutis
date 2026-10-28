# 插件 API

TypeScript / JavaScript 插件（`@arcships/rutis`）、Python 插件（`rutis`）和 Go 插件（`github.com/arcships/rutis/go/rutis`，见 [写一个 Go 插件](go-plugin.md)）的写法与规则相同；Swift 插件（本仓库的 Swift 包 `Rutis`，见 [写一个 Swift 插件](swift-plugin.md)）也一样，Kotlin 插件（`io.github.arcships:rutis`，K-P1：桌面 jvm target 与测试工具，应用接入在 K-P2，见 [kotlin/README](../../kotlin/README.zh-CN.md)）也一样。当前插件 API 版本：**3**。

## 声明

| 声明 | TypeScript（`definePlugin({...})`） | Python（`define_plugin(apply, ...)` 或模块变量） | Go（`rutis.Define(rutis.Plugin[C]{...})`） | Swift（`Rutis.define(Plugin(...))`） | Kotlin（`Rutis.define<C>(...) { ctx, config -> }`） | 含义 |
| --- | --- | --- | --- | --- | --- | --- |
| 用到的服务 | `inject: ['llm']` | `inject=["llm"]` | `Inject: []string{"llm"}` | `inject: ["llm"]` | `inject = listOf("llm")` | 都就绪才启动；任何一个撤销，插件停下，等它回来再启动 |
| 提供的服务（可选） | `provides: { weather: { today: 'sync' } }` | `provides={"weather": Weather}` 或 `{"weather": {"today": "sync"}}` | `Provides: rutis.Provides{"weather": rutis.MethodsOf[*Weather](rutis.Sync("Unit"))}`（导出方法，除 `Sync` 列出的外都是 `async`） | `provides: ["weather": .of(Weather.self, sync: ["unit"])]`（`@RutisService` 类型的方法，除 `sync` 列出的外都是 `async`） | `provides = mapOf("weather" to service<Weather>(sync = setOf("unit")))`（`@RutisService` 类的 public 成员函数，除 `sync` 列出的外都是 `async`） | 不写也行：`ctx.provide` 提供的服务自动对外（见下）。写了就按声明：把方法标成 `sync`，并让 `rutis-host check` 在运行前就知道谁提供什么；声明过的服务只能调用声明过的方法 |
| 配置 | `config: { type: 'object', ... }` | `config={...}`（模块变量写 `Config`） | 类型 `C`（Schema 由它生成），或 `Schema` | `config:` 的 `Decodable` 类型（Schema 由 `@RutisConfig` 生成，或 `static let rutisSchema`） | `@Serializable` 类型 `C`（Schema 由它的 `SerialDescriptor` 推出），或 `schema` | JSON Schema：`apply` 之前按它补默认值并校验配置（见[配置](#配置)）；`rutis-host check` 也一样 |
| 启动 | `apply(ctx, config)` | `apply(ctx, config)` | `Apply: func(ctx *rutis.Ctx, config C) error` | `apply: { ctx, config in ... }`（`async throws`） | `{ ctx, config -> }`，可以挂起；抛异常即装载失败 | 可以是 async；返回清理函数或不返回（Go：返回错误即装载失败） |

## 配置

`apply` 之前，各语言的 SDK 给这一行的配置补上 Schema 里的默认值并校验，`rutis-host check` 按同样的规则做同样的事（各语言逐字一致，由一份共用用例表 `specs/config-schema/cases.json` 保证）：

- **默认值**：没有配置（不写或 `null`）时取根的 `default`，没有时根的 `type` 是或含 `object` 就是 `{}`。再一层层补：在已有的对象里，缺的属性有 `default` 就补上，再进到它的子对象和数组的每一项。顺着 `$ref` 和 `allOf`，不进 `anyOf` / `oneOf`；缺的子对象自己没有 `default` 时不会凭空造出来（要它总在，就给它 `default: {}`）。`apply` 拿到的是副本。
- **检查的关键字**：`type`、`properties`、`required`、`additionalProperties`、`items`、`enum`、`const`、`minimum`、`maximum`、`default`、`anyOf`、`oneOf`（符合一个分支就行）、`allOf`、`$ref`（指向 `#/$defs/…` 和 `#/definitions/…`）。其他关键字（`format`、`pattern`、`minLength` 等）既不检查也不警告。不做类型转换：`"5"` 不是数字。
- **多余的键**：Schema 有 `properties` 而没写 `additionalProperties` 时，它没列的键是警告（由宿主给出，每行每个值一次），拼写接近时给出提示；值原样交给 `apply`。`additionalProperties: false` 时是错误。
- **配置不合格**：这一行失败，不调用 `apply`。所有问题一次列出，按路径排序，每个一行（最多 20 行，之后是 `config: and N more problems`）：`weather: config: units should be "metric" or "imperial", got "kelvin"`。依赖它的行写 `waiting for weather (row weather failed: config: …)`。跨语言时这个错误是 `BindingError`，错误码 `bad-config`，`data: { issues: [{ path, message }] }`；测试工具抛同样的错误。
- **类型化的配置**（Go、Swift、Kotlin、Rust）：Schema 由类型生成，只写出解码器本来就有的规则；先校验，再解码。哪些字段必填、默认值从哪来，见各语言的指南。

## ctx

| 方法 | 作用 |
| --- | --- |
| `ctx.use(name)` | `inject` 里声明的服务。同一个运行时进程里的插件提供的，是对象本身；其他进程、其他语言、其他机器提供的，是代理。插件和它的行都没在 `inject` 里声明的服务拿不到时，报 `service <name> is not available to this plugin, which does not declare it in inject: add '<name>' to its inject`（只有声明了的服务才一定给它）；调用需要调用方的行的宿主服务方法（例如带 `&Ctx` 参数的 Rust 方法）时，也提示把服务写进 `inject` |
| `ctx.provide(name, value)` | 提供服务，直到插件卸载，或调用返回的函数。服务自动对外，别的语言、别的进程按名字使用；`provides` 没有声明它时，方法取 `value` 上的公开方法（JavaScript：自身和原型链上的函数，不含 `constructor`；Python：不以 `_` 开头的可调用属性；Go：导出方法），都按 `async` 调用，字段不算方法。推断按各语言自己的规则，同一个服务用两种语言写时方法可能不同：继承 `EventEmitter` 的 JavaScript 类也会带上 `on`、`emit` 等方法；只想对外其中几个时在 `provides` 里列出它们 |
| `ctx.effect(cleanup)` | 插件卸载时运行 `cleanup`（可以是 async） |
| `ctx.on(name, listener, options?)` | 监听事件：不论在哪里发出（另一个插件、另一个运行时、宿主、相连的节点），只要是这个 root 总线上的同名事件，就送到 `listener`。随插件卸载注销；返回的函数立即注销。见下面的[事件](#事件) |
| `ctx.emit` / `parallel` / `serial` / `waterfall` / `bail` | 发出事件的五种方式；`ctx.filter(keep)` 给出只送达部分监听器的同样五个方法。见[事件](#事件) |
| `ctx.signal` | （JavaScript）插件卸载时中止；Python 是任务取消，Go 是 `ctx` 本身 |

一个跨过会话的错误被再次抛出时，Node、Bun、Python 和 Go 都带着它完整的错误图（cause、`AggregateError` 的子错误）再跨一次，不限于聚合的情形：Node 解码出的是真正的 `Error` 对象，重新编码时自然带上整个图，Python 和 Go 的 `encode_error` / `encodeError` 把 `RemoteError` 的图拼进去（节点追加、引用偏移）以保持一致。

清理按注册的相反顺序运行：`apply` 返回的清理函数最先，然后是 `ctx.effect` 注册的。

Go 里：`ctx.Use(name, &target)` 把服务绑定到函数字段的结构体（或 `*advanced.Service`）；`ctx.Provide(name, value)`；`ctx.Effect(func(context.Context) error)`；`ctx.Go(task)` 起一个卸载时停下的 goroutine。`*rutis.Ctx` 是插件卸载时取消的 `context.Context`。

Kotlin 里：`ctx.use<I>(name)` 经 `@RutisClient` 接口 `I` 使用服务（同进程提供者实现了 `I` 时就是对象本身）；`ctx.provide(name, obj)` 的 `obj` 是 `@RutisService` 类的实例，方法表由 KSP 处理器在编译期生成；`ctx.effect { }`；`ctx` 是插件卸载时取消的协程作用域，`ctx.launch { }` 起的协程卸载时取消并等待。收到的错误是 `RutisException`，按 `name` 匹配。

宿主里的 Rust 插件（编进宿主的和 dylib 插件）：服务类型的 `impl` 块上加 `#[rutis::service]`，`ctx.provide(value)` 之后它就按名字自动对外（名字默认是类型名、第一个词小写），各语言的行用 `ctx.use(name)` 调用它；没标注的 Rust 类型不对外。见 [Rust 宿主](rust-host.md#在类型上标注-rutisservice)。插件本身的写法见 [写一个 Rust 插件](rust-plugin.md)。

## 事件

一个宿主上的全部监听器——宿主里的 Rust 插件的、每个运行时进程里每个插件的（任何语言）、经节点连接引进来的——组成**一个逻辑总线**。规则只有一条：

> 事件在哪里发出、监听器在哪里，结果都和所有监听器在同一个进程里一样。

做不到的只有物理上不可能的情况（同步等待成环、同步等待一个已经断开的对端），这时明确报错，从不悄悄换成别的行为。设计见 [统一事件](../design-events-2026-10-18.md)，线上协议见 [协议规格](../design-events-protocol-2026-10-18.md)。

- **登记是自动的**：插件照常写 `ctx.on(name, listener)`，什么都不用声明，`rutis.json` 和宿主的 Rust 代码里也不用写。运行时把监听器登记给宿主，就像服务自动对外一样。
- **事件 = 名字 + 参数 + 发出方的作用域描述**。名字是字符串，所有语言、所有路径通用，惯例是 `area/verb`（`session/created`）。参数按[值怎样传递](#值怎样传递)跨边界。作用域描述发出方所在的加载器实例（见下）；送达按名字。
- **一张监听器表**：一个监听器登记一次，可以被下面任何一种方式调用；由发出方选择方式。
- **只在需要时经过宿主**：一个名字的监听器都在发出方的进程里时，就在本进程分发，不经过宿主；结果相同，只是代码在哪里执行不同。

### 五种方式

| 方式 | 发出方得到 | 执行 | 返回值 | 监听器出错 |
| --- | --- | --- | --- | --- |
| `emit` | 立即返回，不等 | 按登记顺序开始每个监听器；同一发出方、同一名字的事件按发出顺序到达每个监听器 | 忽略 | 不影响发出方，也不影响其他监听器；在监听器所在的一侧报告一行（见[失败的报告](#失败的报告)），从不结束进程 |
| `parallel` | 等全部完成 | 按登记顺序开始，异步部分重叠 | 忽略 | 全部跑完；有失败时抛出 `AggregateError`，子错误按**登记顺序**（不是完成顺序），只有一个失败也如此 |
| `serial` | 第一个"有结果"的返回值，或没有结果 | 按登记顺序逐个等待 | 第一个有结果的返回值让分发停下并返回它 | 抛出并停止其余 |
| `waterfall` | 最外层监听器的返回值，原样 | 由外向内，每个监听器的最后一个参数是 `next`；`next()` 执行链上其余部分，最后执行终态（terminal） | 链式传递；不调 `next` 就否决其余 | 抛到发出方；`next` 调两次、在分发结束后调用，以 `WaterfallNext` 失败 |
| `bail`（同步） | 第一个有结果的返回值，同步 | 按登记顺序同步调用；别处的监听器作为同步调用执行，与同步服务调用参加同一套等待环检测 | 同 `serial` | 抛出并停止；等待成环（`SyncWaitCycle`）或对端断开时报错；遇到异步监听器以 `AsyncListener` 失败 |

- **没有结果**：返回 `undefined` / `null`（JavaScript）、`None`（Python）、`nil`（Go）、`None`（Rust），或任何语言的 `false`（包括 Rust 的 `Some(false)`）。其余都是结果，`0` 和 `""` 也是。
- **`waterfall` 原样返回**：不做"有没有结果"的归一，`false` 仍是 `false`。`next` 不带参数，返回链上其余部分的结果（JavaScript：Promise；Python：可 await 的对象；Go：阻塞调用；Rust：future）。终态收到事件参数；没有终态时最后一个 `next()` 得到没有结果。没有监听器时，`waterfall` 返回终态的值。
- **异步监听器**：JavaScript 的 `async` 函数、Python 的 `async def`、Rust 用 `on` 登记的监听器不能被 `bail` 调用（`AsyncListener`）；普通函数返回了 Promise / 协程时也以 `AsyncListener` 失败（返回的 Promise / 协程被丢弃）。Go 的监听器都是同步的。

### 各语言的写法

每种语言都有这五种方式，写法是这门语言自己的：

| | Rust（`rutis`） | Node / Bun 叶子插件（`@arcships/rutis`） | Cordis 插件（Node 运行时里） | Python（`rutis`） | Go（`rutis`） | Swift（`Rutis`） | Kotlin（`arcships.rutis`） |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 监听 | `ctx.on::<E>(listener)`；同步监听器 `ctx.on_sync::<E>(..)`；选项与前缀 `ctx.listen::<E>().prefix(p)` / `.name(n)` / `.once()` / `.prepend()` / `.global()`，再 `.on(..)` → `Disposer` | `ctx.on(name, fn, { prepend, global }?)` → `() => void` | Cordis 自己的 `ctx.on(name, fn, options?)` → `() => boolean`；`ctx.once` | `ctx.on(name, fn, *, prepend=False, global_=False)` → `Callable[[], None]` | `ctx.On(name, func(ctx context.Context, args ...any) (any, error), advanced.Prepend(), advanced.Global())` → `func()` | `ctx.on(name, prepend:, global:) { (参数...) in ... }` → `() -> Void` | `ctx.on(name, prepend = false, global = false) { args -> }` → `() -> Unit` |
| 监听器 | 闭包 `\|event: Arc<E>, call: Call<E>\|`，返回 `Result<Option<E::Value>>`；`call.mode()`、`call.next()`；`on_sync` 的闭包收到 `SyncCall` | `(...args[, next]) => 值 \| Promise` | 同左 | `fn(*args[, next])`，可以是 `async def` | `fn(ctx, args...)`；waterfall 中最后一个参数是 `advanced.Next`（`func(context.Context) (any, error)`） | 任意参数的闭包，按声明类型解码；waterfall 中最后一个参数是 `Rutis.Advanced.Next<R>`（`async throws -> R?`） | `suspend (EventArgs) -> Any?`；waterfall 中最后一个参数是 `args.next`（`suspend () -> Any?`） |
| `emit` | `ctx.emit(e)`，`e` 是 `E` 或 `Arc<E>` | `ctx.emit(name, ...args): void` | `ctx.emit([thisArg,] name, ...args)` | `ctx.emit(name, *args) -> None` | `ctx.Emit(ctx, name, args...) error`（只报拒绝） | `try ctx.emit(name, args...)`（只报拒绝） | `ctx.emit(name, *args)`（只报拒绝） |
| `parallel` | `ctx.parallel(e).await` | `await ctx.parallel(name, ...args)` | `await ctx.parallel([thisArg,] name, ...args)` | `await ctx.parallel(name, *args)` | `ctx.Parallel(ctx, name, args...) error` | `try await ctx.parallel(name, args...)` | `ctx.parallel(name, *args)`（挂起） |
| `serial` | `ctx.serial(e).await -> Option<E::Value>` | `await ctx.serial(name, ...args)` | `await ctx.serial([thisArg,] name, ...args)` | `await ctx.serial(name, *args)` | `ctx.Serial(ctx, name, args...) (any, error)` | `try await ctx.serial(name, args..., as: R.self)` | `ctx.serial(name, *args): Any?`（挂起） |
| `waterfall` | `ctx.waterfall(e).terminal(t).await`（`.terminal` 可省略） | `ctx.waterfall(name, ...args)`；有终态时 `ctx.waterfall.with(terminal)(name, ...args)`（参数都是事件的，函数也是） | `ctx.waterfall([thisArg,] name, ...args, terminal)` | `await ctx.waterfall(name, *args, terminal=None)` | `ctx.Waterfall(ctx, name, terminal, args...) (any, error)`（`terminal` 是 `rutis.Terminal`，即 `func(ctx, args ...any) (any, error)`，或 `nil`） | `try await ctx.waterfall(name, args..., as: R.self) { 终态 }` | `ctx.waterfall(name, *args, terminal = { args -> })`（挂起） |
| `bail` | `ctx.bail(e)` | `ctx.bail(name, ...args)` | `ctx.bail([thisArg,] name, ...args)` | `ctx.bail(name, *args)` | `ctx.Bail(ctx, name, args...) (any, error)` | `try await ctx.bail(name, args..., as: R.self)`（线上同步） | `ctx.bail(name, *args): Any?`（阻塞本线程） |
| 过滤 | `ctx.filter(\|info\| ..)`，之后同样的五个方法 | `ctx.filter(d => boolean)` → `{ emit, parallel, serial, waterfall, bail }` | `thisArg[Context.filter] = c => boolean`，`c.rutis` 是描述 | `ctx.filter(lambda d: ...)` → 带这五个方法的对象 | `ctx.Filter(func(rutis.Listener) (bool, error)) *rutis.Emitter` | `ctx.filter { (l: Rutis.Advanced.Listener) in ... }` → `Rutis.Advanced.Emitter` | `ctx.filter { d -> Boolean }` → `Emitter` |
| 作用域 | `ctx.scope()` | 描述的 `scope` | `ctx.rutis.scope` | 描述的 `scope` | `ctx.Scope()` | `ctx.scope` | `ctx.scope` |
| 卸载信号 | 监听器以登记它的 `ctx` 被调用，`ctx.cancelled()` | `ctx.signal` | `ctx.signal` | 监听器的任务被取消 | 监听器的 `ctx` 被取消 | 监听器的 Task 被取消 | 监听器的协程被取消 |
| `parallel` 的失败 | `ErrorKind::ListenersFailed` | `AggregateError` | `AggregateError` | `rutis.advanced.AggregateError`（`errors`） | `*advanced.AggregateError`（`Errors`） | 名为 `AggregateError` 的 `RutisError`（`errors`） | `RutisException`（`name == "AggregateError"`，`errors`） |

表里带 `advanced` 的名字在各语言的 advanced 子模块里（0.19 起，#692）：Go `github.com/arcships/rutis/go/rutis/advanced`，Python `rutis.advanced`，Swift `Rutis.Advanced`，Kotlin `arcships.rutis.advanced`，JavaScript 与 TypeScript `@arcships/rutis/advanced`。

Node 的 `waterfall` 只要有监听器在本进程之外就返回 Promise，所以总是 `await` 它。Go 在插件方法里发出时传收到的 `ctx`（带着调用链），在 `Apply` 里传 `ctx` 本身。Rust 的发出方法在 `Ctx` 上，与其他语言同名。

同一个 waterfall 监听器在五种语言里：

```rust
// Format: impl Event, Value = String
ctx.on::<Format>(|_, call| async move { Ok(call.next().await?.map(|text| format!("{text}!"))) })?;
```

```ts
// Node / Bun 叶子插件；Cordis 插件写法相同
ctx.on('greeting/format', async (text: string, next: () => Promise<string>) => `${await next()}!`)
```

```python
async def exclaim(text, next):
    return f"{await next()}!"

ctx.on("greeting/format", exclaim)
```

```go
ctx.On("greeting/format", func(ctx context.Context, args ...any) (any, error) {
	inner, err := args[len(args)-1].(advanced.Next)(ctx)
	if err != nil {
		return nil, err
	}
	return fmt.Sprint(inner, "!"), nil
})
```

每种语言的完整例子在它的指南里：[TypeScript](typescript-plugin.md#事件)、[Bun](bun-plugin.md#事件)、[Python](python-plugin.md#事件)、[Go](go-plugin.md#事件)、[Cordis](cordis.md#事件)、[Rust](rust-plugin.md#事件)。

### 作用域

作用域是加载器的一个组实例（`instanced` 组的一份副本，如一个租户、一个会话），或者根。每个插件、每次发出都在一个作用域里：包含它的最内层实例，或者根。

> 事件是平总线：一个 root 里的所有同名监听器都收到，与发出方或监听器的作用域无关。

作用域只是 `ListenerInfo` 里的描述，供过滤使用；默认不做实例隔离。要把事件限制在一个实例里，用 `filter` 按描述里的 `scope` 判断，或使用不同的事件名。发出方的作用域由宿主按发出的行确定。叶子插件的 `isolate` 标签（服务的作用域）与事件作用域无关。

### 过滤

发出时可以带一个过滤函数：它对每个候选监听器调用一次，收到这个监听器的**描述**，返回是否送达。

- 在**发出方的进程里**执行，按登记顺序，在任何监听器开始之前；它看到的是按名字取出的候选监听器，对任何地方的监听器都一样有效。
- 过滤函数抛出时，整次分发以这个错误失败，不开始任何监听器（`emit` 中改为报告）。
- 用 `global` 选项登记的监听器不经过滤。

描述（JSON 字段，没有值时为 `null`；Rust 为 `ListenerInfo`，没有值时为 `None`；Go 为 `rutis.Listener`，没有值时为 `""`）：

| 字段 | 含义 |
| --- | --- |
| `name` | 事件名（Rust 前缀监听器为前缀） |
| `prefix` | 是不是 Rust 的前缀监听器 |
| `plugin` | 登记它的插件：插件声明的名字（`definePlugin({ name })`、`define_plugin(name=)`、Go 插件的名字、Cordis 插件的名字），没有声明时是它所在行的 id |
| `row` | 它所在行的 id，即配置给这一行起的名字（组的各个实例复制出的副本共用它）；挂载方式加载的插件为 `null` |
| `runtime` | 它所在的运行时；宿主自己的 Rust 插件为 `null` |
| `node` | 经节点连接引进来的，是对端节点的 id；其余为 `null` |
| `scope` | 它的作用域链，`[{ id, group }]`，最外层在前，根是 `[]` |
| `global` | 是否用 `global` 登记 |

Cordis 插件用 Cordis 自己的写法：`ctx.emit(thisArg, name, ...)`（以及其余四种）的 `thisArg[Context.filter]` 是函数时用它过滤。本进程的监听器传入它自己的 Cordis `Context`，其他监听器传入 `{ rutis: 描述 }`；Node 运行时里每个 Context 都有只读的 `ctx.rutis`，所以 `c => c.rutis.row === 'chat'` 对两者都有效。

### `prepend` 与 `global`

所有语言都有这两个监听器选项：`prepend` 让监听器排在这个名字已有的全部监听器之前（各处一致的"登记顺序"由宿主决定）；`global` 让它不受过滤。Rust：`ctx.listen::<E>().prepend()`（还有 `.global()`、`.once()`），再 `.on(..)` / `.on_sync(..)`；Node / Bun：`ctx.on(name, fn, { prepend: true })`（Cordis 插件就是 Cordis 自己的选项）；Python：`ctx.on(name, fn, prepend=True, global_=True)`；Go：`ctx.On(name, fn, advanced.Prepend(), advanced.Global())`。

### 生命周期

- 监听器属于登记它的插件（行），插件卸载时注销；返回的函数立即注销。
- **卸载时**：这一行的监听器先从总线上撤下（之后的事件不再到达），正在运行的监听器收到取消信号——Go 是监听器的 `context`，Python 是任务取消，JavaScript 是这一行的 `ctx.signal` 被中止，Rust 是 `ctx.cancelled()`——并被等待，最多 `RUTIS_SHUTDOWN_TIMEOUT`（默认 10 秒；宿主里的 Rust 插件用宿主的关闭截止时间），然后才运行清理。超过时运行时在 stderr 写 `rutis: listeners of row <row> still running after <s>s; unloading anyway` 并继续。
- 撤下时已经发往它的调用失败一次（`BindingError`，`listener <id> of <name> is withdrawn`，或传输错误），从不重试。
- **发出方放弃**（Rust 丢弃 future、Python 取消任务、Go 取消 context）：不再开始新的监听器，正在运行的收到取消；发出方得到本语言的取消。`emit` 不被取消。JavaScript 没有取消一次分发的办法，JavaScript 的监听器也没有逐次调用的信号（只有卸载时的 `ctx.signal`）。
- 监听器在哪个上下文里运行：Rust 监听器以**登记它的** `ctx` 被调用，它在里面登记的东西属于监听器的插件；Go 监听器的 `ctx` 带着发出方的调用链，发出方放弃或这一行卸载时取消，监听器里调用服务时把它传下去。
- 与 Rust 的 `Ctx::dispose_self` 同一约定：监听器不得等待自己所属的行卸载完成，要卸载就结束监听器之后再做。

### 断开

运行时进程或节点连接断开时，它的监听器从总线上撤下，就像它们的插件卸载了：之后的事件不再发给它们（这是"监听器不在了"，不是静默丢失）。正在进行的分发里，发往断开一侧的调用失败一次，错误与服务调用遇到的传输错误相同（`BindingError`）：`parallel` 把它放进 `AggregateError`，`serial` / `waterfall` / `bail` 抛给发出方，`emit` 报告一行。对它的同步等待同样失败。不加调用超时，与服务调用一致。

### 框架事件 `rutis/*`

宿主以 `emit` 方式发布框架信息，每个事件一个参数（一个记录）。各语言的插件都可以监听，任何插件发出 `rutis/*`（任何方式）都以 `ReservedEvent` 失败。

| 事件 | 参数 | 何时 | 作用域 |
| --- | --- | --- | --- |
| `rutis/service` | `{ name, scope, available, row }` | 一个服务名在一个作用域里有了提供方（`available: true`）、提供方被替换（再次 `true`，新的 `row`）、被撤下且不再有提供方（`false`） | 服务的作用域 |
| `rutis/row` | `{ row, plugin, scope, state, error }`，`state` 为 `pending` / `running` / `failed` / `stopped` | 配置的行（任何语言，包括 Rust 行和组行）每次状态变化；`failed` 时带 `error` | 行的作用域 |
| `rutis/runtime` | `{ runtime, state, reason }`，`state` 为 `running` / `down` | 运行时挂载成功（每次重连之后也是）；会话结束或挂载失败（`reason` 说明原因） | 根 |

`rutis/row` 也是这条总线上的事件，所有监听器都收到。Rust 里对应的类型是 `rutis_loader::RowEvent`、`rutis_bridge::events::{ServiceEvent, RuntimeEvent}`。测试工具不发出 `rutis/*`。

`internal/*` 是每个运行时自己的框架内部机制（Cordis 的 `internal/dispatch`、`internal/service` 等），从不跨进程：叶子插件不能监听或发出它，宿主拒绝登记它（`event <name> is internal to its runtime`）。Cordis 插件在本进程里照常使用；从别处到达的事件也经过 Cordis 自己的分发，所以 `internal/dispatch` 的校验插件照常看得到它们。

### 值

事件的参数、结果、`next` 与终态的结果，在每条路径上都按[值怎样传递](#值怎样传递)跨边界：十种值、函数与对象的引用、64 层的深度上限，节点之间也一样。发不出的值只让那一次调用失败（`parallel` 中是 `AggregateError` 的一个子错误，`emit` 中是一行报告）；发出方自己的参数发不出时，整次分发失败。作为参数传过去的引用，存活到那次调用结束。

Rust 的类型化事件以 `Event::NAME` 作为跨语言的名字，参数按 serde 编解码，所以 `Event` 和它的 `Value` 都要派生 `Serialize` / `Deserialize`。参数个数：两个及以上字段的元组结构体（`struct Created(String, u32)`）对应同样多个参数；unit 结构体没有参数；其余（普通结构体、newtype、map、序列）是一个参数。监听器按名字查找：同名的另一个 Rust 类型的监听器也收到这个事件，经 serde 转换。按名字收任意参数用 `rutis_bridge::events::Args` 监听（`ctx.listen::<Args>().name(name)`）；名字由值决定的事件类型重写 `Event::name`。

### 失败的报告

没有人等待的失败在发生的一侧写一行（运行时写 stderr，宿主进入它的错误汇集，rutis-host 打印同样的文字）：

| 何时 | 一行 |
| --- | --- |
| `emit` 的监听器失败 | `rutis: listener of <name> in row <row> failed: <ErrorName>: <message>` |
| 无法送达或得不到回复的调用（传输、值、已撤下），由分发方报告 | `rutis: listener of <name> in runtime <runtime> failed: …` 或 `… in node <peer> failed: …` |
| `emit` 的过滤函数失败 | `rutis: filter of <name> from row <row> failed: …` |
| 经宿主的 `emit` 被拒绝或没有回答 | `rutis: emit of <name> from row <row> failed: …`（挂载方式加载的插件：`from the mount`，过滤那一行也是） |
| 宿主拒绝登记一个监听器（行加载期间则让加载失败） | `rutis: registration of listener of <name> in row <row> failed: …` |

### 错误

| 名字 | 何时 | 各语言 |
| --- | --- | --- |
| `AggregateError` | `parallel` 有失败；子错误按登记顺序，信息 `<n> listeners of <name> failed` | JS `AggregateError`（`errors`）；Python `rutis.advanced.AggregateError`（`errors`）；Go `*advanced.AggregateError`（`Errors`）；Rust `ErrorKind::ListenersFailed` |
| `AsyncListener` | `bail` 遇到异步监听器 | JS 名为 `AsyncListener` 的 `Error`；Python `rutis.advanced.AsyncListener`；Go `advanced.ErrAsyncListener`；Rust `ErrorKind::AsyncListener` |
| `WaterfallNext` | `next` 调两次、在分发结束或取消后调用；Rust 在 waterfall 之外调用 `call.next()`，或同步监听器在异步 waterfall 里调用它 | JS `WaterfallNext`；Python `rutis.advanced.WaterfallNext`；Go `advanced.ErrWaterfallNext`；Rust `ErrorKind::WaterfallNext` |
| `ReservedEvent` | 发出 `rutis/*`，发出的 API 同步拒绝 | JS `ReservedEvent`；Python `rutis.advanced.ReservedEvent`；Go `advanced.ErrReservedEvent`；Rust `ErrorKind::ReservedEvent` |
| `SyncWaitCycle` | 一次同步调用（`bail`）会使等待成环 | 同服务调用：Python `rutis.advanced.SyncWaitCycle`；Go `advanced.ErrSyncWaitCycle` |
| `BindingError` | 传输错误、已撤下的监听器、被拒绝的帧 | Python `rutis.BindingError`；Go `rutis.ErrBinding`；Rust `ErrorKind::ListenerWithdrawn` 或传输错误 |

Go 用 `errors.Is(err, advanced.ErrWaterfallNext)` 等判断，任何进程里同名的错误都匹配。监听器自己抛出的错误带着名字、信息与错误图跨边界，与任何调用的错误相同。

**按名字匹配监听器的错误，不按类。** 收到的是什么取决于监听器在哪里运行：来自别的进程时，它是本语言的远端错误（TypeScript：同一内置类的 `Error`，或带那个 `name` 的普通 `Error`；Python：`rutis.RemoteError`，rutis 自己的名字是对应的类；Go：`*rutis.RemoteError`，先 `errors.As` 再看 `Name`），带着它的 `name`、`message` 和 `code`；所有监听器都在发出方进程里时（快速路径），可能收到监听器自己的错误对象。测试工具让插件自己的发出方法收到跨进程后的形式（严格模式下），所以按类匹配会在测试里失败；快速路径保留原对象这一点不作保证。

### 与 0.12 及更早的端混用

事件协议是 rutis 0.13 的，没有旧路径。0.12 及更早的运行时或节点连进当前的宿主时，在加载任何东西之前按协议版本被拒绝：

```text
incompatible session: the far end speaks protocol 2, this side 4: upgrade the far end
```

0.13 的运行时遇到旧宿主时退出（状态 1）：`rutis: refused: incompatible (this host does not declare events-v2 (the event protocol of rutis 0.13): upgrade rutis-host to 0.13 or later); stopping`。从 0.12 升级见 [迁移到 0.13](../migration-0.13.md)。

## 生命周期

插件不决定自己什么时候运行，宿主决定：

- `inject` 的服务都就绪时启动（`apply`）；
- 任何一个撤销时停下（运行清理），它回来时再启动；
- 配置变了：重启（运行清理，再用新配置 `apply`）；
- 插件提供的服务被撤销时，用到它的插件先停下，提供者后停。

所以 `apply` 里拿到的服务在插件运行期间一直有效，不需要处理"服务中途消失"。

用 `rutis-host` 或 `LoaderPlugin` 的宿主提供服务 `loader`（`create`、`reload`、`remove`、`status`，见 [宿主自己写插件](../design-host-writes-plugins-2026-10-27.md)）。它的方法在 `apply` 返回后调用（服务方法、事件里）：在一行自己的 `apply` 里调用会等到这个 `apply` 结束，所以会卡住。在 `apply` 里挂子插件用 `ctx.plugin`（Rust；Node 里的 Cordis 插件）。

一个服务名在 rutis 里只有一个提供者：别的插件（任何语言、任何节点）已经提供同名服务时，后提供的插件失败（装载时提供的装载失败，装载后才提供的在提供时失败），错误写明双方，例如 `service weather is already provided by row forecast: row weather cannot provide it too`。只有这个插件失败：其他插件（同一运行时里的也一样）继续用原来的提供者。没有声明的服务在插件运行后才出现，依赖它的插件照常等它出现；`rutis-host check` 对这类依赖写一条告警 `nothing declares <服务>; it may be provided at runtime by …`，不算失败（`check --strict` 时算，见 [检查配置](rutis-host.md#检查配置)）。从线程或 goroutine 里在 `apply` 返回前提供的也一样：插件装载完成前宿主已经处理完它，重名时装载就失败。

0.12 的运行时被比 0.12 旧的宿主挂载时，宿主不接收没有声明的服务，它们只在本进程里可见；插件提供这样的服务时，运行时按名字只在 stderr 提示一次：`rutis: service <名字> is not declared in provides, and this host (older than 0.12) does not take undeclared services; declare it or upgrade rutis-host`。在 `provides` 里声明它，或者升级 rutis-host。

宿主停止时卸载所有插件，每个清理执行一次，然后运行时进程退出。宿主没有卸载就没了（被强制结束、崩溃）时，运行时发现和宿主的连接断开，自己卸载所有插件，然后退出进程；插件留下的定时器、套接字、线程、goroutine 不会让它继续运行。

清理有截止时间：宿主停止时是 `rutis-host` 的 `--shutdown-timeout`（默认 10 秒，见 [rutis-host](rutis-host.md#停止与退出码)）；宿主没了时，运行时用同一个时间（`rutis-host` 把它放在环境变量 `RUTIS_SHUTDOWN_TIMEOUT` 里传给运行时，没有时 10 秒）。过了截止时间还没完成的清理，以及排在它后面的，不再等待：运行时在 stderr 写一行说明，以退出码 1 退出。所以必须完成的工作不要只靠清理；清理里不要无限等待。Node 和 Bun 里，同步阻塞的清理（死循环）占住线程，截止时间无法打断。

## 同一运行时里的插件共用一个进程

同一个运行时（`node`、`py`、`bun`）里的行在同一个进程里运行：它们共用内存、全局状态和事件循环，一个插件阻塞事件循环，其他插件也停下。要隔离，把插件放进另一个运行时实例。宿主可以把插件放进单独的进程（见[做一个开放插件的宿主](open-host.md#3-管住插件)）。

一个服务方法一口气占住运行时 5 秒（固定，0.18 起，#678）时，运行时在 stderr 写一次是哪一行的哪个方法、这期间其他行都在等、怎么改，例如：

```text
runtime py: forecast.tomorrow of row weather has held the runtime for 5 s, and the other rows wait meanwhile: it blocks the event loop (a def method, or a blocking call in an async def one); make it async def and await what it waits for (asyncio.sleep, an async client, asyncio.to_thread for a blocking call), and it will not hold the runtime
```

Python 里看方法的每一段：`def` 方法整个，`async def` 方法两次 `await` 之间的每一段（例如里面的 `time.sleep`、`requests.get`）；改成 `async def`，等待的东西用 `await`（`asyncio.sleep`、异步的客户端，阻塞的调用放进 `asyncio.to_thread`）。Node、Bun 里看方法开始到它第一次 `await` 之前同步运行的那一段（`readFileSync`、`execSync`、忙等），`await` 之后的阻塞不提示；改用异步调用并 `await`，或者把计算放进 worker 线程。`apply` 和监听器里的阻塞不提示。只写一次、只写不打断：方法照常运行完。`rutis-host dev --debug` 下不写（停在断点上的运行时可以停多久都行）。Go、Swift、Kotlin 的同步方法不占住整个运行时，没有这个提示。

插件没处理的错误（JavaScript 里没处理的 promise 拒绝、定时器里抛出的异常，Python 任务里没处理的异常）不结束进程（0.18 起，#688）：

- **追得到是哪一行时，这一行失败**，写明原因和位置，例如 `weather: failed: unhandled rejection: Error: no region`，下面是插件自己的堆栈（`at … (file:///…/weather.mjs:12:9)`）；错误自己的 `code`、`data` 照常带着。正在加载的行加载失败（`failed in apply: unhandled rejection: …`）。同一运行时里的其他行继续运行。行的 `apply`、服务方法、监听器、交出去的函数带着这一行的上下文运行，它们启动的任务、promise 也带着。一行直接调用同一运行时里另一行的服务、错误的堆栈里最内层的插件文件是另一行的时，分不清是哪一行没处理它（堆栈只说明它在哪里抛出），按来源不明报告，不记到任何一行。Python 总是这样追（3.12 起读任务的上下文，3.10、3.11 在任务创建时记下）；插件还拿着的任务，异常不算没处理，不报告。Node 只在 `rutis-host dev` 下追：它给每个 promise 记下所属的行（`async_hooks`），这对每个 promise 都有开销。
- **追不到时，写明来源不明**：`runtime node: an unhandled error from an unknown row: <堆栈>`（只列插件自己的帧，一帧都不是插件的时候列出完整堆栈），哪一行都不失败。Node 在 `rutis-host run` 下、Bun 总是这样；Python 里不在任何一行里运行的任务和回调也是这样。Python 线程里没处理的异常照 Python 自己的规则打印，线程结束，进程继续。要知道是哪一行，用 `rutis-host dev` 再跑一次。
- **插件自己结束进程**（`process.exit`、`os._exit`）：进程照常结束，同一运行时里的行都停下等待运行时（见[生命周期](#生命周期)），运行时先写一行 `runtime node: row weather called process.exit(1)`。是哪一行，`dev` 下的 Node 与 Python 按当前在跑的行，其余按栈上插件的文件认（只有一行对得上时才写）；Bun 的尾调用（`() => process.exit(1)`）不留插件的栈帧，那时不写行名。
- Go、Swift、Kotlin 运行时不变：panic、未捕获的异常照各语言自己的规则处理。

## 输出

插件写到标准输出、标准错误的内容（`console.log`、`print`、`fmt.Println`）进宿主的终端，各写各的流。`rutis-host dev` 下每一行前面加上写它的行的名字（0.18 起，#678）：

```text
[weather] fetching forecast for Berlin
```

- Node、Bun、Python 的几行共用一个进程，运行时按当前在跑的行加前缀：行的 `apply` 与清理、服务方法、监听器、交出去的函数，以及它们启动的异步工作（Node、Bun 的异步上下文，Python 的 contextvars）。追不到是哪一行的输出加运行时的名字（`[py] `）：Python 插件自己开的线程，以及不在任何一行里运行的代码。直接写文件描述符的输出（子进程、`os.write`、原生模块）运行时看不到，原样传过去。
- Go、Swift 一个进程只有一个插件：宿主读它的输出时加运行时的名字（`[go-weather] `），SDK 不变。
- Bun 的 `console` 自己直接写文件描述符：加前缀时运行时换上一个经过 `process.stdout` / `process.stderr` 写的 `console`，对象按 Node 的格式打印在一行里（`{ a: 1 }`），不是 Bun 平时的多行格式。
- `rutis-host run` 默认不加（生产环境里插件的输出原样不动）；`--prefix-output` / `--no-prefix-output` 打开或关掉，见 [rutis-host](rutis-host.md#输出)。

Python 的 `logging` 不改：默认只显示 WARNING 及以上，这是 Python 自己的规则。要看 INFO，在插件里配置日志：

```python
import logging

logging.basicConfig(level=logging.INFO)
```

## 值怎样传递

插件和它用到的服务可能不在同一个进程。跨进程时：

| 值 | 怎样传递 |
| --- | --- |
| `null` / `None`、布尔、数字、字符串 | 复制 |
| 数组 / list、普通对象 / 字符串键的 dict | 递归复制 |
| Python dataclass | 复制为 dict |
| 日期时间、二进制、Map、Set、大整数、Decimal、UUID | 复制，各语言收到本语言的类型，见下面的[十种值](#十种值) |
| `Error` / 异常 | 作为值传递时复制为数据 `{ name, message }`（TypeScript 另带 `stack`）；抛出时另一端得到同名、同消息的错误（TypeScript 中内置错误类保留类，其余是 `Error`；Python 中是 `rutis.RemoteError`，`name` 是原来的类名；Go 中是 `*rutis.RemoteError`）。任何语言抛出的错误都可以带 code 和 data：TypeScript 中是字符串属性 `code` 和属性 `data`，Python 中是字符串属性 `code` 和属性 `data`，Go 中是方法 `Code() string` 和 `Data() any`，Rust 中是 [`rutis_bridge::exports::failure`](rust-host.md#手写-hostdispatch)（[导出的 Rust 服务](rust-host.md#导出服务参考)的 `Err(e)` 就变成它）。收到的一方从 `code`、`data` 读到它们（Go：`Code`、`Data`；Rust：`Error::code()`、`Error::data()`），再次抛出时继续传。`data` 按数据传（大整数为 bigint）；传不过去的 data 被略去，错误的其余部分照常传。其他属性不传 |
| 函数、Promise / 协程 | 按引用：另一端调用或 await 的是原来那个 |
| 带方法的对象、类的实例 | 按引用，传给任何语言：另一端拿到代理，方法调用的是原对象。哪些值算对象、各语言怎样收，见下面的[带方法的对象](#带方法的对象) |
| 插件定义（`definePlugin` 的结果；Node 里的 Cordis 插件；Python 的 `define_plugin` 结果；Go 的 `*rutis.Definition`；Swift、Kotlin 的 `Rutis.define` 结果） | 按引用，只交回定义它的运行时：Rust 服务把参数或返回值类型写成 `rutis::Definition`，收到的就是一个可以挂的插件定义，见下面的[插件定义](#插件定义)。Node、Bun、Python、Go、Swift、Kotlin 都能交出 |
| `undefined` | 只在作为整个参数或整个返回值时保留：作为参数是"没有给"（Python 少收一个参数或收到 `None`，Go 是零值）；作为返回值 JS 收到 `undefined`，其他语言收到各自的空值。在数组、对象、Map、Set 里任何一层都变成 `null`（Python 收到 `None`） |
| 数组的空位（`[1, , 3]`） | 与数组里的 `undefined` 相同：`null` |
| Go 的结构体、切片、map | 复制：结构体字段、切片和 map 的元素逐个按本节的规则传，字段名遵循 `json` 标签；结构体一律是数据，要按引用传对象写 `rutis.Ref(v)`。`map[string]T` 是对象，键不是字符串的 map 是 Map，`map[K]struct{}` 是 Set |
| 超出 ±(2^53−1) 的整数 | 自动按大整数传，见[十种值](#十种值) |
| symbol、循环引用、`NaN` / `Infinity`、JS 的其他类型化数组（如 `Int16Array`）、带时区的 Python `time`、Go 的 channel | 不能跨进程（调用失败） |
| 嵌套超过 64 层的值 | 不能跨进程（这次调用失败，见下） |
| 编码后超过 16 MiB 的调用或返回值 | 不能跨进程（这次调用失败，见下） |

不能跨进程的值只让**这一次调用**失败：错误写明方法、第几个参数（或返回值）和原因，例如参数编码后过大时，这次调用得到 `store.put: argument 1 cannot be sent: the message would be 30889096 bytes, over the limit of 16777216 bytes (16 MiB)`。会话、运行时、同一会话上的其他调用和其他行都不受影响。错误的名字取决于是哪一方拒绝的、用什么语言写的（宿主是 `BindingError`，Node、Bun 是 `RangeError` 或 `TypeError`，Python 是 `ValueError`，Go、Swift、Kotlin 是 `Error`），不作保证；要判断是哪种失败，看错误码：值传不过去都是 `bad-value`，`data` 写明是参数还是返回值（`{part, argument?, limit?}`），不要看名字或文字。

嵌套深度：一个值最多嵌套 **64 层**。数组 / list / tuple、对象 / dict（含 dataclass）、Map、Set、Go 的切片、map、结构体各算一层，里面有没有引用都一样；作为值传递的 `Error` / 异常也算一层。调用的参数列表本身不算一层：每个参数各自最多 64 层；返回值最多 64 层。更深的值由**要发送它的一方**在发出之前拒绝，只让这次调用失败，错误信息说明上限（`value nested deeper than 64 levels`）：

- 作为参数：调用方的这次调用失败（Rust 中是 `Error::Value`），对端不执行它；
- 作为返回值：调用方收到被调方抛出的错误；
- 会话继续，同一会话上的其他调用（包括同时进行的）不受影响。

五个运行时（Rust、Node、Bun、Python、Go）和测试工具的严格模式都按这个上限在发送时检查。上限约束的是发送：接收的 Rust 一侧为不遵守上限的对端（例如更早的版本）留了余量，线上不超过 208 层的帧照常读取，所以从这样的对端**收到**的值可能超过 64 层（最多约 100 层含引用的数组、约 206 层数据）；这部分余量不保证，插件不要依赖。帧更深、Rust 一侧不读时：能看出它属于哪个调用，就只让那个调用失败（对端的调用收到错误；对本侧调用的回复让本侧的调用失败），其中的引用直到会话结束才释放；看不出属于哪个调用时，结束会话并说明原因。见 [#279](https://github.com/arcships/rutis/issues/279)。

消息大小：一次调用（连同它的参数）或一个返回值编码后最多 **16 MiB**（16777216 字节，本地通道和 WebSocket 的默认上限）。**要发送它的一方**在发出之前拒绝更大的消息，只让这次调用失败，错误写明方法、第几个参数（最大的那个）或返回值、大小和上限（`store.put: argument 1 cannot be sent: the message would be 30889096 bytes, over the limit of 16777216 bytes (16 MiB)`）：作为参数时调用方的这次调用失败，对端不执行它；作为返回值时调用方收到这个错误；会话继续，其他调用不受影响。Rust、Node、Bun、Python、Go、Swift、Kotlin 都在发送前检查。更早版本的对端仍可能发来超过上限的消息：接收方像以前一样关闭这条连接，这个会话结束（本地运行时随之退出），其他会话不受影响。大的数据分块传，或者只传它的位置（路径、地址）。

数字有下面这些边界。它们是现在的行为，**不保证**以后不变（明确不保证），依赖它们的插件需要自己处理：

- JavaScript（Node、Bun）里数字是 IEEE 754 双精度：JS 自己的 `number` 超出 ±(2^53−1) 时本来就不精确，照原样发出；要精确用 `bigint`。`-0` 变成 `0`；整数值的浮点数（Python 的 `2.0`）到 JS 一端就是 `2`，再传出去是整数。
- Python 保留 `-0.0`。
- NaN、±Infinity 不能跨进程。

同一个进程里没有复制，所以"传过去之后改原对象，另一端也看到"这类写法在单元测试里可能成立、跨进程就不成立。测试工具的严格模式（默认）按上表传值，提前暴露这种问题。

### 带方法的对象

复制还是按引用，由发出的值决定，与另一端期望什么无关：

| 语言 | 复制（数据） | 按引用（另一端拿到代理） |
| --- | --- | --- |
| JS（Node、Bun） | 原型是 `Object.prototype` 或 `null`、没有函数值属性的普通对象；数组 | 类的实例；带函数值属性的普通对象（`{ get() {}, set() {} }`） |
| Python | `dict`、`list`、`tuple`、dataclass（复制为 dict） | 其他类的实例（带 `__call__` 的实例按函数传） |
| Go | 结构体、切片、map | `rutis.Ref(v)` |
| Swift | `Codable` 结构体和类 | `@RutisService` 类型的实例 |
| Kotlin | `@Serializable` 类 | `@RutisService` 类的实例 |
| Rust | serde 值 | 不发出自己的对象；转发别的进程的对象 |

数组、字典里的对象也各自按引用传。收到的对象和 `ctx.use` 得到的服务代理是同一种东西：每个方法都是对原对象的异步调用，没有数据字段；要数据，让方法返回它。

| 语言 | 怎样收 | 拿到什么 |
| --- | --- | --- |
| JS（Node、Bun） | 不用声明 | 代理：`await cache.get('a')` |
| Python | 不用声明 | 代理：`await cache.get("a")` |
| Go | 参数类型写成函数字段的结构体（和 `ctx.Use` 的目标一样）；或写 `any` | 调用各个方法的函数字段（阻塞）；`any` 收到 `*advanced.Service` |
| Swift | 参数类型写 `@RutisClient` 协议（`any Cache`）；或写 `Any` | 这个协议的代理；`Any` 收到 `Rutis.Advanced.Client` |
| Kotlin | 参数类型写 `@RutisClient` 接口；或写 `Any?` | 这个接口的代理；`Any?` 收到 `RemoteObject`（`arcships.rutis.advanced`） |
| Rust | 手写的 `HostDispatch` | `rutis_bridge::session::Reference`（`call_method`、`call_method_async`），Cordis 绑定里是 `ObjectRef`；`#[rutis::service]` 的参数不收对象（`bad-argument`） |

- 同一个对象交出几次，对方拿到同一个代理；代理交回原来的进程，那边拿到的是原对象；交给第三个进程，仍是同一个对象。
- 调用对象没有的方法，这次调用以 `no-method` 失败：`Cache has no method gett (methods: get, set, size)`，`data` 是 `{object, method, methods}`。
- 声明的参数类型收不了对象时（Go、Swift、Kotlin 的数据类型），这次调用以 `bad-argument` 失败，错误写明对象要怎样收。
- 生命周期和函数引用相同：收的一方不再持有代理，或它的进程退出后，发出方放掉对象。对象不跟行走：交出它的行卸载后，对象照常可以调用。
- 发出方的运行时退出或断开后，对它的对象的调用以 `peer-gone` 失败（`a call on an object from py failed: runtime py exited`），只有这些调用失败。
- 各语言的细节见各自的指南：[TypeScript](typescript-plugin.md#带方法的对象)、[Bun](bun-plugin.md#带方法的对象)、[Python](python-plugin.md#带方法的对象)、[Go](go-plugin.md#带方法的对象)、[Swift](swift-plugin.md#带方法的对象)、[Swift 应用](swift-app.md#带方法的对象)、[Kotlin 应用](kotlin-app.md#带方法的对象)、[Rust](rust-plugin.md#对其他语言提供服务)。

#### 同一个进程与跨进程的差别

- **同一个进程**（同一个运行时里的行）拿到的是对象本身：方法照旧是同步的，字段可以读。
- **跨进程**只有方法，每次调用都是异步的。读字段、依赖同步结果的写法在同一个进程里成立、跨进程就失败；测试工具的严格模式按跨进程的样子交出代理，在测试里先失败。
- **空闲停止**：按需启动的运行时（Go、Swift）在最后一行卸载 `idle` 秒后停下，别处持有它的对象不让它保持运行。停下后对这些对象的调用以 `peer-gone` 失败：`a call on an object from go-netkit failed: runtime go-netkit stopped because it was idle`。要一直可用，让交出它们的行保持加载，或者把运行时设成 `eager`。
- **身份**：同一个对象收到几次是同一个代理，交回原来的进程是原对象，各语言都一样（Go 只有 `*advanced.Service` 如此）。

### 十种值

| 值 | JS（Node、Bun） | Python | Go | Kotlin | Rust（`rutis-bridge`） | Swift |
| --- | --- | --- | --- | --- | --- | --- |
| 时间点 | `Date` | 带时区的 `datetime`（收到为 UTC） | `time.Time`（收到为 UTC） | `java.time.Instant`、`kotlin.time.Instant`（收到为 `java.time.Instant`） | `Instant` | `Date`（到微秒） |
| 本地日期时间 | 字符串 `"2026-10-15T14:30:05"` | 无时区的 `datetime` | `advanced.DateTime` | `LocalDateTime` | `DateTime` | `Rutis.Advanced.DateTime` |
| 日期 | 字符串 `"2026-10-15"` | `date` | `advanced.Date` | `LocalDate` | `Date` | `Rutis.Advanced.CalendarDate` |
| 时刻 | 字符串 `"14:30:05"` | 无时区的 `time` | `advanced.TimeOfDay` | `LocalTime` | `TimeOfDay` | `Rutis.Advanced.TimeOfDay` |
| 二进制 | `Uint8Array`（也可发 `Buffer`、`ArrayBuffer`、`DataView`） | `bytes`（也可发 `bytearray`、`memoryview`） | `[]byte` | `ByteArray` | `Vec<u8>`（生成代码里是 `Bytes`） | `Data` |
| Map | `Map` | 键不是字符串的 `dict`（收到的是 `dict` 的子类） | 键不是字符串的 map（解码到 `any` 时是 `map[any]any`） | 键不是字符串的 `Map` | `Vec<(Value, Value)>`（生成代码里是 `ValueMap`） | 键不是 `String` 的 `Dictionary`（解码进 `Any` 时为 `[AnyHashable: Any?]`） |
| Set | `Set` | `set`（也可发 `frozenset`） | `map[K]struct{}`，或切片（解码到 `any` 时是切片） | `Set` | `Vec<Value>`（生成代码里是 `ValueSet`） | `Set` |
| 大整数 | `bigint` | `int`（收到的是 `int` 的子类） | `*big.Int`，或放得下的整数类型 | `BigInteger`，或放得下的整数类型 | `BigInt`，或放得下的整数类型 | `Rutis.Advanced.BigInt`，或放得下它的整数类型 |
| Decimal | 字符串 `"12.30"` | `Decimal` | `advanced.Decimal` | `BigDecimal`（保留标度） | `Decimal` | `Decimal`（38 位，不保留末尾零；要文本时声明 `String`） |
| UUID | 字符串（小写） | `UUID` | `advanced.UUID` | `java.util.UUID`、`kotlin.uuid.Uuid` | `Uuid` | `UUID` |

- 插件只写本语言原生的代码，转换由 rutis 做。收到的大整数、Map 在 Python 里是 `int`、`dict` 的子类，照常使用；原样转发时仍是大整数、Map。
- JS 没有本地日期时间、日期、时刻、Decimal、UUID 的类型，收到的是普通字符串，内容完整，`===` 照常可用。这样的值经 JS 原样转发时类型不保留（Python 发出的 `date` 经 JS 转发回来是 `str`）。要在 JS 里凭空创建这五种值，或以原类型转发，用 `@arcships/rutis/advanced` 的构造函数：`rutis.datetime('2026-10-15T14:30')`、`rutis.date('2026-10-15')`、`rutis.time('14:30')`、`rutis.decimal('1.10')`、`rutis.uuid('…')`；普通字符串照旧是字符串。Go 也可以把这五种值解码进 `string`。
- 整数：超出 ±(2^53−1)（即绝对值大于 9007199254740991）的整数（Python `int`、Go `int64` / `uint64`、Rust 整数）自动按大整数传，不需要特殊写法；JS 收到时是 `bigint`，范围内仍是 `number`。收到的大整数放得下时可以解码进声明的整数类型，放不下时这次调用失败。`bool` 不受影响。
- 时间：年份 0001–9999。本地日期时间没有时区，原样传递，不猜时区，也不转成时间点。时间点按接收方的精度向负无穷截断：JS 到毫秒，Python 到微秒，Go、Rust 到纳秒；Python 的本地日期时间和时刻同样截断到微秒。
- Decimal 保留末尾零（`1.10` 与 `1.1` 不同），不用指数；NaN、无穷不能跨进程。大整数、Decimal 至多 4300 位。
- Map 的键、Set 的元素只能是字符串、有限数字、布尔、大整数、本地日期时间、日期、时刻、Decimal、UUID；时间点、二进制、`null`、数组、对象、Map、Set、引用不行，发送方在发出前拒绝。接收方转换后两个键相等时（例如 JS `Map` 的键 `1` 和 `true` 到 Python），这次调用失败：会丢条目。字符串键的 JS `Map` 仍按 Map 传，Python 普通的字符串键 `dict` 按对象传。
- 每种值只发给声明了它的对端。对端不支持时（例如 0.11 之前的运行时，包括超出 ±(2^53−1) 的整数），只让这次调用失败，错误说明哪种值，会话继续，其他调用不受影响。事件的参数与结果和调用一样承载它们，节点之间也一样。

### 插件定义

插件是一个值：插件定义可以照常作为服务调用的参数或返回值交出，收的一方挂上它，每挂一次是一份，每一份可以有自己的配置。这样一个插件能把它在 `apply` 里准备好的东西交给别处挂载的部件，不用写任何 rutis 专用的东西（[设计](../design-plugin-values-2026-10-25.md)）。

```js
export default definePlugin({
  inject: ['parts'],
  async apply(ctx) {
    const pool = await openPool()
    await ctx.use('parts').register('session', definePlugin({
      inject: ['history'],
      apply(ctx, config) { /* 直接用 pool */ },
    }))
  },
})
```

- 能交出的：Node、Bun 的 `definePlugin` 结果；Node 里的 Cordis 插件（函数、类、带 `apply` 的对象）；Python 的 `define_plugin` 结果；Go 的 `rutis.Define` 结果（`*rutis.Definition`，客户端的函数字段直接写这个类型）；Swift、Kotlin 的 `Rutis.define` 结果（客户端的参数类型写 `Rutis.Definition`、`Plugin`，见 [Swift 插件](swift-plugin.md#把插件交给服务)、[Kotlin 应用](kotlin-app.md#2-写插件)）。
- 连进来的应用（Swift、Kotlin 的 `RutisApp`）交出的插件定义只能挂在这个连接所在的实例里，或更里面。
- 收的一方是 Rust：参数或返回值的类型写 `rutis::Definition`，用 `ctx.plugin(def.at(&ctx, config)?)` 挂上，见 [Rust 宿主](rust-host.md#收插件定义)。其他类型的参数照旧：要数据的参数收到插件定义时这次调用失败。
- 每一份在定义它的运行时里运行，按挂上它的实例隔离，按它的 `inject` 门控。
- 插件定义属于交出它的那一行的这一代：这一行卸载、重载或结束时，先结束从它挂出的每一份，等它们清理完，再卸载这一行。之后再挂会失败。运行时进程退出时，这些副本也结束。
- 插件定义只交回定义它的运行时，不能再交给第三个运行时（那次调用失败）。

### 测试工具与真实运行时的差异

测试工具（`@arcships/rutis/testing`、`rutis.testing`、Go 的 `rutistest`）和真实运行时对同一份期望文件比对：对同一个插件做同一组调用（同步、异步、回调、错误、上表的值），结果必须相同（[`weather.expected.json`](../../crates/rutis-bridge/src/runtime/testing/weather.expected.json)）。严格模式下已知的差异只有下面这些：

| 差异 | 原因 |
| --- | --- |
| 结束进程的插件（`process.exit`、`os._exit`）结束的是测试本身 | 测试工具在测试的进程里运行插件；期望文件里这一条标为 `allowed`，测试工具不做这个调用 |
| 不重启：配置变化、注入的服务撤销都不会让插件停下再启动 | `load` 只启动一次，生命周期由宿主决定（见上） |
| 同步调用不阻塞，不会出现 `SyncWaitCycle`；没有取消（`AbortSignal`、协程取消） | 插件与测试在同一个事件循环里 |
| Python：跨过边界的异常是 `RemoteError`，它的 `__cause__` 是原来的异常 | 方便调试；真实运行时的 `RemoteError` 没有 `__cause__` |
| 描述里的 `runtime` 是语言名（`node`、`bun`、`python`、`go`；宿主用配置里给的名字）；卸载最多等 `RUTIS_SHUTDOWN_TIMEOUT`，与运行时相同，或用工具自己的选项：TypeScript `host({ deadline })` / `load(…, { deadline, runtime })`（毫秒），Python `load(…, deadline=, runtime=)`（秒），Go `rutistest.RuntimeName` 等选项 | 各工具的选项按各自语言的习惯 |
| TypeScript：只能加载 `definePlugin` 写的插件，不能加载 Cordis 插件 | 测试工具没有 Cordis 的插件模型、服务与依赖门控 |
| 不检查 16 MiB 的消息上限 | 测试工具不把值编码成消息；真实运行时在消息超过上限时让这次调用失败（见[值怎样传递](#值怎样传递)） |
| 事件：全部监听器都在测试的进程里（总是本地分发），没有别的进程、节点，也不发出 `rutis/*` | 测试工具在一个进程里运行插件。五种方式、过滤、作用域（TypeScript `host()` 的 `scope`、Python `Bus` 与 `scope=`、Go `rutistest.InScope`）、卸载等待与取消、`emit` 失败的报告（`reported`）与真实运行时相同，对同一份[事件期望文件](../../crates/rutis-bridge/src/runtime/testing/events.expected.json)比对 |

十种值在两边按同样的规则传递，这不是差异。带方法的对象也不是：严格模式下，每种语言的测试工具都按跨进程的样子交出代理（只有方法，每次调用都是异步的），与真实运行时相同。关闭严格模式（`strict: false`）时值不经过上表，以上之外的差异都可能出现。

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

## 调用等多久

rutis 不给调用设时限，也不替双方取消调用：一个调用该等多久是调用方的业务。要限时，用调用方自己语言的写法，取消会传到对面（见[取消](#取消)）：

| 语言 | 写法 |
| --- | --- |
| Python | `await asyncio.wait_for(weather.tomorrow(), timeout=5)` |
| Go | `ctx, cancel := context.WithTimeout(ctx, 5*time.Second)`，把 `ctx` 传给调用 |
| Swift | 在一个 `Task` 里调用，到时取消这个 `Task` |
| Kotlin | `withTimeout(5_000) { weather.tomorrow() }` |
| Rust | `tokio::time::timeout(Duration::from_secs(5), weather.tomorrow())`（丢弃 future 即取消） |
| TypeScript | `Promise.race` 加一个定时器；对面不会被取消，与原生 JS 一样（promise 本身不能取消） |

看一个调用已经等了多久，用 `rutis-host status`（它列出进行中的调用）。

## 取消

async 方法的调用方放弃等待（例如超时）时，调用被取消：被调用的 Python 协程被取消；Go 方法的 `context.Context` 被取消；JavaScript 方法在调用方传了 `AbortSignal` 参数时收到 abort（Rust 绑定调用 Cordis 方法时会传）。没有这两种情况时，被调用的方法照常运行完，结果被丢弃。

## 版本

插件上标记着它所用的插件 API 版本（`definePlugin` / `define_plugin` / `rutis.Define` 自动标记）。运行时比插件旧时，加载失败并说明需要升级什么；比插件新时照常加载。插件对 SDK 的依赖：

- JavaScript、TypeScript：`^` 范围（`"@arcships/rutis": "^0.17.2"`）。插件用它写时那个次版本的 SDK，宿主里装着别的版本时 npm 给它装自己的一份，几个版本可以并存。
- Python：只写下限（`rutis>=0.17`）。插件和 Python 运行时是宿主环境里的同一个 `rutis` 包，写了上限（`<0.18`），装插件会把运行时降级，或者让宿主升级不了。所以一个版本删掉或移动了插件导入的名字时，那一版的迁移指南写明，插件随宿主一起升级（例如 0.19 把进阶的名字移进 `rutis.advanced`，见[迁移到 0.19](../migration-0.19.md)）。
- Go、Swift：SDK 编进二进制（`require`、`from:`），与宿主装的版本无关。

插件 API 3（rutis 0.13）增加 `parallel`、`serial`、`waterfall`、`bail` 与 `filter`，事件成为一个逻辑总线；0.13 的运行时继续加载 API 2 的插件，API 3 的插件被旧运行时以现有的信息拒绝。0.13 的事件协议不与 0.12 及更早的运行时、节点混用（见上面的[与 0.12 及更早的端混用](#与-012-及更早的端混用)）。插件 API 2 增加了叶子事件，API 1 的插件仍可加载。
