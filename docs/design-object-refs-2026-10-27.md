# 0.18 带方法的对象跨语言按引用传

[English](design-object-refs-2026-10-27.en.md)

状态：定稿，未实现。日期：2026-10-27（同日修订：发送规则保持现状；改正与 0.17 混用的事实；Go 只学会收；出错文字只用线上有的信息；补空闲停止、非本端的插件引用、Rust 服务收到对象）。基准：`main` `85678323`（0.17.2 之后）。排期：0.18。
Issue：[#687](https://github.com/arcships/rutis/issues/687)（从 [#675](https://github.com/arcships/rutis/issues/675) 拆出，属于 [#663](https://github.com/arcships/rutis/issues/663)）。
依据：[插件 API](guide/plugin-api.md)「值怎样传递」；[设计哲学](design-philosophy.md) §七；[值类型](design-value-types-2026-10-15.md)（V0 开发者无感、V4 能力协商、V10 `undefined`）；[插件定义](design-plugin-values-2026-10-25.md)（加一种引用的先例）；[诊断](design-diagnostics-2026-10-26.md)（错误码 `bad-value`、`bad-argument`、`no-method`、`peer-gone`，§十）。

范围外：Rust 插件（`#[rutis::service]`）收发自己的对象（§九）；对象的数据字段跨进程实时读写；节点之间事件转发（`NodeEvent` 的参数是 JSON，照旧不带引用）。

## 一、选哪种做法

**选 1：按文档实现。** 理由：

- **开发者无感**（V0）：同一个进程里，Cordis 把对象本身交给对方；开发者照这个写法把对象交给别的语言的服务，是最自然的写法。改文档就是让开发者为跨语言改写代码。
- **大部分已经有了**：线上的 `object` 引用、带方法名的 `call` 帧、`Release` 和计数、宿主转发对象、能力 `objects`，都已存在。Node、Bun、Python、Go、Swift、Kotlin 都已经能**发出**对象并执行对它的方法调用；只是除了 Rust，谁都不**收**（不声明 `objects`，收到就让这次调用失败）。要补的是收的一方。
- **入口已经有了**：收到的对象和 `ctx.use` 得到的服务是同一种东西：一组可以调用的方法。各语言就用它们已有的服务代理来收，不加新的写法（§四）。

## 二、决策

| # | 决策 | 章节 |
| --- | --- | --- |
| O1 | 发送规则不变：数据复制；类的实例、带函数值属性的普通对象按引用（JS）；数据类以外的实例按引用（Python）；`rutis.Ref(v)`（Go）；`@RutisService`（Swift、Kotlin）。对象在数据里任何深度都按引用，所以 dataclass 的字段可以带引用 | 三 |
| O2 | 收到的对象和 `ctx.use` 得到的服务代理一样用：方法都是异步调用，没有数据字段；Go 用函数字段的结构体，Swift、Kotlin 用 `@RutisClient` 类型；`any` / `Any` 收到与已有函数类型对应的对象类型 | 四 |
| O3 | 同一个对象交出几次，对方拿到同一个代理；交回原来的进程，拿到的是原对象 | 四 |
| O4 | 生命周期和函数引用相同：接收方不再持有时释放；不跟行走；没有释放的引用不让运行时保持运行；对方进程不在后调用以 `peer-gone` 失败 | 五 |
| O5 | 线上格式不变；各 SDK 声明已有的能力 `objects`；0.18 宿主在 compat 握手里也认它。给不收对象的一端的那次调用以 `bad-value` 失败，会话继续 | 六 |
| O6 | `undefined`：只有作为整个参数或整个返回值时保留；在数组、对象、Map 里任何深度都是 `null` / `None`。这是按已写的规则（V10）修正 | 七 |
| O7 | Node、Python 收到不是交回本端的插件引用时，只让这次调用失败，不再结束会话（和收到对象的旧处理一样） | 八 |

## 三、什么按引用传

规则不变，就是插件 API 指南现在写的：**数据复制，带方法的对象、类的实例按引用。** 由发出的值本身决定，和 Cordis 一样不看对方要什么。

| 语言 | 复制（数据） | 按引用（对方拿到代理） |
| --- | --- | --- |
| JS（Node、Bun） | 普通对象（原型是 `Object.prototype` 或 `null`）且没有函数值的属性；数组 | 类的实例；有函数值属性的普通对象（`{ get() {}, set() {} }`） |
| Python | `dict`、`list`、`tuple`、dataclass（复制为 dict）；dict 里的函数按函数引用 | 其他类的实例；带 `__call__` 的实例按函数引用 |
| Go | 结构体、切片、map | `rutis.Ref(v)`（已有，唯一的写法） |
| Swift | `Codable` 的结构体与类 | `@RutisService` 类型的实例（已有） |
| Kotlin | `@Serializable` 的类 | `@RutisService` 类的实例（已有） |
| Rust | serde 的值 | 不发出自己的对象（§九）；转发别的进程的对象 |

- **数据里的对象**：对象可以出现在数据的任何深度（数组的元素、dict 的值、dataclass 的字段），各自按引用传。dataclass 的字段过去不许带引用，为满足这一条，现在和 dict 相同。
- **Go 为什么要写**：Go 的结构体既是数据也可以有方法，值本身分不出来，所以按引用要写 `rutis.Ref(v)`（已有规则）。Go 要做的只是学会收。
- **Swift、Kotlin 为什么要标注**：这两种语言在运行时列不出一个类的方法，方法表由 `@RutisService` 在编译时生成。它就是提供服务时已经在用的标注。

## 四、收到的是什么

收到的对象和 `ctx.use` 得到的服务代理是同一种：**每个方法都是一次异步的跨进程调用，调用的是原对象；没有数据字段**。要拿数据，让方法返回它。

| 语言 | 写法 | 收到的 |
| --- | --- | --- |
| JS（Node、Bun） | 不用声明 | 代理：`await cache.get('a')`；读到的每个名字都是方法（symbol、`then`、`toJSON`、`toString`、`valueOf` 除外，所以 `await` 一个代理得到它自己）；`console.log` 显示 `[object from py]` |
| Python | 不用声明 | 代理：`await cache.get("a")`，任何公开名字都是方法（和 `ctx.use` 一样）；`repr` 是 `<object from node>` |
| Go | 参数类型写成函数字段的结构体（和 `ctx.Use` 的目标一样，字段名按同样的规则对应方法名）；`any` 收到 `*rutis.Object` | 结构体的每个函数字段调用对应的方法（阻塞，第一个参数是 `context.Context`）。`*rutis.Object` 和 `*rutis.Function` 对应：`Call(ctx, method string, result any, args ...any) error`，可以原样再交出 |
| Swift | 参数类型写成 `@RutisClient` 协议（`any Cache`）；`Any` 收到 `Rutis.RemoteObject` | 宏生成的同一个代理（`ctx.use(name, Cache.self)` 用的那个） |
| Kotlin | 参数类型写成 `@RutisClient` 接口；`Any?` 收到 `RemoteObject` | KSP 生成的同一个代理；`suspend` 方法挂起，普通方法阻塞（和服务客户端一样） |
| Rust | 宿主手写分发、Cordis 绑定用 `rutis_bridge::session::ObjectRef`（已有） | `call` / `call_async` |

- `*rutis.Object`、`Rutis.RemoteObject`、`RemoteObject` 和已有的 `*rutis.Function`、`Rutis.RemoteFunction`、`RemoteFunction` 对应：只在参数类型是 `any` / `Any` 时出现，用于原样转交或按名字调用。
- **同一个对象**：同一个对象交出几次，接收方拿到同一个代理（JS `===`、Python `is`）；代理交回原来的进程，那边拿到的就是原对象。交给第三个进程时，宿主照常转发。
- **同一个进程里**拿到的是对象本身（Cordis 的语义），方法照常同步、字段照常可读。跨进程时只有方法、方法都是异步的：这条写进边界规则（原则 7）。测试工具的严格模式（默认）按跨进程的样子给代理，提前暴露差别。
- **调用的语义**和函数引用相同：参数、返回值按「值怎样传递」跨边界；对方抛出的错误照常传回；取消照常传到对面（[诊断](design-diagnostics-2026-10-26.md) §6.1）。Go 对象的方法可能被并发调用（已有的规则）。

## 五、生命周期

- **和函数引用相同**：每交出一次计一次；接收方的代理不再被引用时（各语言用它们给函数引用的同一套：JS `FinalizationRegistry`、Python `weakref.finalize`、Go `runtime.AddCleanup` 等）发 `Release`；计数到 0，发出方放掉这个对象。作为事件参数传过去的对象，照旧存活到那次调用结束。
- **不跟行走**：交出对象的行卸载后，对象照常存在、照常可以调用，和同一个进程里 Cordis 的对象一样。要在卸载时停用，由作者在清理里自己处理（和函数相同）。插件定义跟行走，是因为它会被挂出副本；对象没有这回事。
- **空闲停止**：按需启动的运行时（Go、Swift）在最后一行卸载 `idle` 秒后停下（已有）。别处还持有它的对象，**不**让它保持运行：持有引用的一方不是行，宿主不按引用计算进程的生死，和函数引用一样。停下之后对这些对象的调用以 `peer-gone` 失败。要一直可用，让交出它的行保持加载，或者把运行时设成 `eager`。
- **进程不在**：发出方的运行时退出、断开或空闲停止后，对代理的调用失败，错误码 `peer-gone`；进行中的调用也是。只有这次调用失败，代理之后的调用都同样失败。不会自动换到新进程（新进程里没有这个对象）。

## 六、线上格式与能力

- **线上格式不变**：`{"type":"reference","value":{"id","home","kind":"object","origin"}}`；方法调用是带 `method` 的 `call` 帧；`get` 帧（实时读属性）保留给 Rust 的 `ObjectRef::get` 和 Cordis，新代理不用。引用里没有类名和方法表，所以收的一方和宿主说不出对象的类（§八）。
- **能力 `objects`**（已有，现在只有 Rust 声明）：
  - Node、Bun、Python、Go、Swift、Kotlin 在 endpoint 和 compat 两种握手里都声明。
  - 0.18 宿主把 `objects` 加进 `COMPAT_DECLARED`。本机运行时都以 compat 格式启动（`runtime/process.rs`），0.17 的宿主只认 `COMPAT_DECLARED` 里的能力，其中没有 `objects`，所以在 0.17 宿主下它们总是"不收对象"。
  - 运行时照旧只在 endpoint 会话上检查对方的 `objects`：compat 会话的对方总是 Rust 宿主，它在每个版本都收对象。宿主转发时按**下游**是否声明检查（已有）。
- **与 0.17 混用**：
  - 0.18 宿主下，对象要转发给 0.17 的运行时：它没有声明 `objects`，这次调用以 `bad-value` 失败（§八 的文字），会话继续。0.17 的一端从不会收到对象。
  - 0.18 的运行时在 0.17.2 宿主下：宿主认不出它们的 `objects`，在两个运行时之间转发对象时按 0.17.2 的文字拒绝（`an object with methods cannot cross to py; pass plain data or a function instead`），只有这次调用失败，会话继续。0.18 的运行时把对象交给 0.17 宿主自己的 Rust 代码（`ObjectRef`，`live_objects.rs`）照常可用。
  - 不升协议版本，不加新能力名。

## 七、`undefined`

现在同一个值在 Python 里有两种样子：`[1, undefined, 3]` 作为参数时，中间那个是一个特殊的占位值；放在对象里时是 `None`。原因是 JS 把数组逐个元素编码（保留 `undefined` 标记），把纯数据的对象整个编成 JSON（`undefined` 变 `null`），Python 解码时又不归一。插件 API 指南和 V10 写的是 `None`。

规则（按 V10 修正）：

- **作为整个参数**：保留"没有给"的意思，和现在相同：JS 的 `f(1, undefined)`，Python 收到的是少一个参数或 `None`，Go 是零值。
- **作为整个返回值**：JS 收到 `undefined`，其他语言是各自的空值。
- **其余位置**（数组、对象、Map、Set 里任何深度，稀疏数组的空位）：一律是 `null` / `None` / `nil`。发的一方写 `null`；收的一方把收到的 `undefined` 标记也读成 `null`，所以旧的一端发来的也一致。
- 结果：`[1, undefined, 3]` 不论在哪一层，到 Python 都是 `[1, None, 3]`，到 JS 都是 `[1, null, 3]`。

## 八、开发者看到什么

错误码按[诊断](design-diagnostics-2026-10-26.md) §4.3；码不出现在给人看的文字里。前缀（行名、方法、第几个参数）按诊断设计的写法。

```ts
// TypeScript：把一个类的实例交给 Python 的服务
class Cache { #m = new Map(); get(k) { return this.#m.get(k) } set(k, v) { this.#m.set(k, v) } }
await ctx.use('warmer').warm(new Cache())
```

```python
# Python：照常用，方法是 async 的
class Warmer:
    async def warm(self, cache):
        await cache.set("a", 1)
        return await cache.get("a")    # 1，调用的是 TS 里那个 Cache
```

```go
// Go：参数类型写成函数字段的结构体，和 ctx.Use 的目标一样
type Cache struct {
	Get func(ctx context.Context, key string) (any, error)
	Set func(ctx context.Context, key string, value any) error
}
func (w *Warmer) Warm(ctx context.Context, cache Cache) (any, error) {
	if err := cache.Set(ctx, "a", 1); err != nil { return nil, err }
	return cache.Get(ctx, "a")
}
```

```swift
// Swift：参数写成 @RutisClient 协议
@RutisClient protocol Cache { func get(_ key: String) async throws -> Int?; func set(_ key: String, _ value: Int) async throws }
@RutisService final class Warmer { func warm(_ cache: any Cache) async throws -> Int? { try await cache.set("a", 1); return try await cache.get("a") } }
```

```kotlin
// Kotlin：参数写成 @RutisClient 接口
@RutisClient interface Cache { suspend fun get(key: String): Int?; suspend fun set(key: String, value: Int) }
@RutisService class Warmer { suspend fun warm(cache: Cache): Int? { cache.set("a", 1); return cache.get("a") } }
```

谁写文字决定能说什么：对象所在的一端知道它的类和方法；收的一方、宿主只知道它来自哪个运行时，只写"an object from py"。

| 情况 | 看到的 | 写的一方 | 码 |
| --- | --- | --- | --- |
| 调用对象没有的方法 | `chat: warmer.warm failed: an object from node: Cache has no method gett (methods: get, set)`；普通对象写 `the object has no method gett (methods: get, set)` | 对象所在的一端（"an object from node"由宿主加） | `no-method`，`data`：`{object, method, methods}`（`object` 是类名，普通对象为 `null`） |
| JS 代理上把数据字段当方法读 | `… an object from node: Cache has no method size (size is a field; objects passed by reference expose only their methods)` | 同上 | `no-method` |
| 对方进程已经退出或空闲停止 | `chat: a call on an object from py failed: runtime py exited` | 收的一方 / 宿主 | `peer-gone`，`data`：`{runtime, reason}` |
| 0.18 宿主转发给 0.17 的运行时 | `chat: argument 0 of warmer.warm: an object from node cannot cross to py: py does not take objects (older than rutis 0.18); upgrade it` | 宿主 | `bad-value` |
| 0.17.2 宿主下的 0.18 运行时之间 | 0.17.2 的文字：`argument 0 of warmer.warm: an object with methods cannot cross to py; pass plain data or a function instead` | 0.17.2 宿主 | 无（0.17 没有码） |
| Swift 没标注的类 | `argument 0 of warmer.warm: a Cache cannot cross: mark it @RutisService to pass it by reference, or make it Codable to copy it` | 发出方 | `bad-value` |
| Kotlin 没标注的类 | `argument 0 of warmer.warm: a Cache cannot cross: annotate it @RutisService to pass it by reference, or @Serializable to copy it` | 发出方 | `bad-value` |
| Go 参数类型收不了对象 | `chat: warmer.warm: argument 0 should be a string, got an object from node` | 收的一方 | `bad-argument` |
| Swift / Kotlin 参数类型收不了对象 | `chat: warmer.warm: argument 0 should be a Cache, got an object from node; an object is received as a @RutisClient protocol (interface) or Any` | 收的一方 | `bad-argument` |
| 对象到了 Rust 服务（`#[rutis::service]`）的参数 | `chat: store.put: argument 0 should be a Config, got an object from py; store takes data and functions, so pass a function for each method` | 宿主 | `bad-argument` |
| Node、Python 收到不是交回本端的插件定义 | `chat: argument 0 of parts.register: a plugin definition from py can only be handed to a Rust service` | 收的一方 | `bad-value` |

[诊断](design-diagnostics-2026-10-26.md) §十"值是复制的"那条提示改为："按值复制；带方法的对象按引用，对方只能调用它的方法"。

## 九、Rust

- **宿主一侧**：所有跨运行时的对象都经宿主转发（已有），这一列测试的就是转发：任意两种语言之间、交回原进程、`Release` 传到源头、源头退出后 `peer-gone`。
- **宿主手写分发、Cordis 绑定**照旧用 `rutis_bridge::session::ObjectRef` 收对象、调用方法（[Cordis](guide/cordis.md) 里带方法的 TS 接口生成的就是包着 `ObjectRef` 的代理结构体）。发送规则不变，所以它们收到的值也不变。
- **Rust 插件不在 0.18 里收发自己的对象**：Rust 不能在运行时为一个 trait 造出实现，要另做一个标在 trait 上的宏，现在没有使用方（判断标准 4）。Rust 插件收行为照旧用函数参数（`impl Fn`）。对象到了 `#[rutis::service]` 的参数时，这次调用以 `bad-argument` 失败，信息提示改传函数（§八）。

## 十、一致性用例

一份共用的期望文件（和事件、值类型的一样），每种语言一列，测试工具的严格模式对同一份比对。

| 用例 | Node | Bun | Python | Go | Swift | Kotlin | Rust |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 发出带方法的对象，对方调用方法，原对象的状态改变 | 发 / 收 | 发 / 收 | 发 / 收 | 发（`Ref`）/ 收（结构体、`*Object`） | 发 / 收（协议、`RemoteObject`） | 发 / 收（接口、`RemoteObject`） | 转发；`ObjectRef` 收 |
| 有函数值属性的普通对象按引用 | ✓ | ✓ | — | — | — | — | 转发 |
| 数据仍是数据：普通对象 / dict / 结构体 / `Codable` / `@Serializable` 复制，里面的函数按引用 | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | — |
| dataclass 的字段带对象 | — | — | ✓ | — | — | — | — |
| 交出两次是同一个代理；交回原进程是原对象 | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | 转发 |
| 交给第三种语言 | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | 转发 |
| 作为返回值、作为数据里的元素、作为事件参数 | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | 转发 |
| 交出它的行卸载后，对象仍可调用 | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | 转发 |
| 通过方法调用取消（调用方放弃，对面收到取消） | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | 转发 |
| 没有的方法：`no-method`，只有这次调用失败 | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |
| 接收方放掉代理：发出方的对象被释放 | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | 转发 |
| 发出方进程被杀：调用 `peer-gone`，会话和其他行不受影响 | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |
| 空闲停止：按需运行时停下后，它的对象以 `peer-gone` 失败 | — | — | — | ✓ | ✓ | — | ✓ |
| 对方没有声明 `objects`：`bad-value`，会话继续 | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |
| 声明的类型收不了对象：`bad-argument` | — | — | — | ✓ | ✓ | ✓ | ✓（`#[rutis::service]` 参数） |
| 测试工具的严格模式给出跨进程样子的代理 | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | — |
| 收到不是交回本端的插件引用：只有这次调用失败 | ✓ | ✓ | ✓ | — | — | — | — |
| `undefined` 在任何深度是 `null`，作为整个参数保留 | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |

跨版本：发布门的跨版本相遇加两条：0.18 宿主把对象转发给 0.17 的运行时（`bad-value`，会话继续）；0.17.2 宿主下两个 0.18 运行时之间传对象（按 0.17.2 的文字只让这次调用失败，会话继续），以及 0.18 运行时把对象交给 0.17.2 宿主的 `ObjectRef`（照常）。[保证表](guarantees.md)新增 `bridge/object-refs`（§三到§六），各写明验证它的测试。

## 十一、兼容与迁移

写进《迁移到 0.18》（两份）：

- **过去失败、现在成功**：带方法的对象发给 Node、Bun、Python、Go、Swift、Kotlin。发送规则不变，Rust 宿主、Cordis 绑定收到的值不变。
- **Python**：dataclass 的字段可以带函数和对象（过去报错）。
- **`undefined`**：按已写的规则（插件 API 指南、V10）修正：数组里的 `undefined` 到 Python 是 `None`、到 JS 是 `null`，不论在哪一层。过去作为参数传的数组里会保留一个特殊值，这是与文档不符的错误。
- **Node、Python**：收到不是交回本端的插件定义只让这次调用失败，不再结束会话。
- 出错文字会变（§八）。线上格式、协议版本不变。

## 十二、要改的文档

- [插件 API](guide/plugin-api.md)（两份）：「值怎样传递」的"带方法的对象、类的实例"一行去掉"只传给 Rust"，写成本稿 §三、§四；加一段"同一个进程与跨进程的差别"与空闲停止（§五）；测试工具差异表里"带方法的对象总是按引用传"那行改写。
- 各语言指南（两份）：[TypeScript](guide/typescript-plugin.md)、[Bun](guide/bun-plugin.md)、[Python](guide/python-plugin.md)（dataclass 字段）、[Go](guide/go-plugin.md)（函数字段结构体参数、`*rutis.Object`）、[Swift](guide/swift-plugin.md)、[Swift App](guide/swift-app.md)、[Kotlin App](guide/kotlin-app.md)（`@RutisClient` 参数、`RemoteObject`）、[Rust 插件](guide/rust-plugin.md)（对象到了服务参数）、[rutis-host](guide/rutis-host.md)（`idle` 不看引用）。
- [协议设计](design-protocol-plugins-2026-09-25.md)：`objects` 能力在 compat 握手里的含义、`undefined` 的位置规则。
- 《迁移到 0.18》、[保证表](guarantees.md)。

## 十三、实施

| 阶段 | 内容 |
| --- | --- |
| O-1 | 规范与宿主：协议设计里的 `objects`、`undefined` 规则；`COMPAT_DECLARED` 加 `objects`；转发拒绝、`peer-gone`、`#[rutis::service]` 参数收到对象的文字与码（码表随 #677）；Rust 编解码里 `undefined` 在容器中写读为 `null`；共用的一致性期望文件与 Rust 列；保证条目 |
| O-2 | Node、Bun：声明 `objects`；收对象的代理；`no-method` 的文字与 `data`；非本端的插件引用只让这次调用失败（同一段解码代码）；测试工具严格模式；`undefined`；指南 |
| O-3 | Python、Go：声明 `objects`；Python 的代理、dataclass 字段带引用、非本端的插件引用只让这次调用失败；Go 的函数字段结构体参数与 `*rutis.Object`；测试工具；`undefined`；指南 |
| O-4 | Swift、Kotlin：声明 `objects`；`@RutisClient` 类型的参数收对象、`RemoteObject`；测试工具；指南；《迁移到 0.18》与插件 API 指南的最终改写 |

每个阶段一个 PR，按顺序合并；后面的阶段复用 O-1 的期望文件。

完成条件（#687 在全部满足后关闭）：

- [ ] §十 的每一行在七列都通过，跨版本两条加入发布门；
- [ ] Node、Python 收到非本端的插件引用只让这次调用失败；
- [ ] Python 的 `undefined` 顶层与嵌套一致；
- [ ] §十二 的文档两份同步。
