# 0.18 带方法的对象跨语言按引用传

[English](design-object-refs-2026-10-27.en.md)

状态：定稿，未实现。日期：2026-10-27。基准：`main` `85678323`（0.17.2 之后）。排期：0.18。
Issue：[#687](https://github.com/arcships/rutis/issues/687)（从 [#675](https://github.com/arcships/rutis/issues/675) 拆出，属于 [#663](https://github.com/arcships/rutis/issues/663)）。
依据：[插件 API](guide/plugin-api.md)「值怎样传递」；[设计哲学](design-philosophy.md) §七；[值类型](design-value-types-2026-10-15.md)（V0 开发者无感、V4 能力协商、V10 `undefined`）；[插件定义](design-plugin-values-2026-10-25.md)（加一种引用的先例）；[诊断](design-diagnostics-2026-10-26.md)（错误码 `bad-value`、`no-method`、`peer-gone`，§十）。

范围外：Rust 插件（`#[rutis::service]`）收发自己的对象（§九）；对象的数据字段跨进程实时读写；节点之间事件转发（`NodeEvent` 的参数是 JSON，照旧不带引用）。

## 一、选哪种做法

**选 1：按文档实现。** 理由：

- **开发者无感**（V0）：同一个进程里，Cordis 把对象本身交给对方；开发者照这个写法把对象交给别的语言的服务，是最自然的写法。改文档就是让开发者为跨语言改写代码。
- **大部分已经有了**：线上的 `object` 引用、带方法名的 `call` 帧、`Release` 和计数、宿主转发对象、能力 `objects`，都已存在。Node、Bun、Python、Go、Swift、Kotlin 都已经能**发出**对象并执行对它的方法调用；只是除了 Rust，谁都不**收**（不声明 `objects`，收到就让这次调用失败）。要补的主要是收的一方。
- **入口已经有了**：收到的对象和 `ctx.use` 得到的服务是同一种东西：一组可以调用的方法。各语言就用它们已有的服务代理来收，不加新的写法（§三）。

## 二、决策

| # | 决策 | 章节 |
| --- | --- | --- |
| O1 | 由**发出方的值**决定按值还是按引用：数据类型复制，带方法的类型按引用；不需要任何 rutis 专用写法（Go 的 `any`、Swift / Kotlin 的方法表是语言做不到的部分，用已有的写法） | 三 |
| O2 | JS 的普通对象（字面量）一律是数据，里面的函数按函数引用传，调用时 `this` 是这个对象；只有类的实例按引用，没有方法的类的实例是数据。Python 的 dict、dataclass 同理 | 三 |
| O3 | 收到的对象和 `ctx.use` 得到的服务代理一样用：方法都是异步调用，没有数据字段；Go、Swift、Kotlin 用已有的服务客户端类型（函数字段的结构体、`@RutisClient`）收 | 四 |
| O4 | 同一个对象交出几次，对方拿到同一个代理；交回原来的进程，拿到的是原对象 | 四 |
| O5 | 生命周期和函数引用相同：接收方不再持有时释放；不跟行走；对方进程不在后调用以 `peer-gone` 失败 | 五 |
| O6 | 线上格式不变；每个 SDK 声明已有的能力 `objects`（两种握手都声明）。0.17 的一端不声明，给它的那次调用以 `bad-value` 失败 | 六 |
| O7 | `undefined`：只有作为整个参数或整个返回值时保留；在数组、对象、Map 里任何深度都是 `null` / `None`，发的一方这样写，收的一方也这样读 | 七 |

## 三、什么按引用传

规则：**数据复制，带方法的东西按引用。** 由发出的值本身决定，和 Cordis 一样不看对方要什么。

| 语言 | 复制（数据） | 按引用（对方拿到代理） |
| --- | --- | --- |
| JS（Node、Bun） | 普通对象（原型是 `Object.prototype` 或 `null`）；数组；没有方法的类的实例（复制自己的可枚举属性） | 有方法的类的实例：原型链上（`Object.prototype` 之前）有除 `constructor` 以外的函数，或者自己有函数值的属性 |
| Python | `dict`、`list`、`tuple`、dataclass（复制为 dict）；没有公开方法的类的实例（如 `SimpleNamespace`，复制公开属性） | 有公开方法（不以 `_` 开头）的类的实例 |
| Go | 结构体、切片、map | 发往声明为接口类型（`any`、`error` 除外）的参数或返回值；`any` 处写 `rutis.Ref(v)`（已有） |
| Swift | `Codable` 的结构体与类 | `@RutisService` 类型的实例（已有） |
| Kotlin | `@Serializable` 的类 | `@RutisService` 类的实例（已有） |
| Rust | serde 的值 | 不发出自己的对象（§九）；转发别的进程的对象 |

几点说明：

- **数据里的函数**：普通对象 / dict / 结构体里的函数照旧按函数引用传，其余字段复制。所以 `{ total: 3, onProgress }` 到 Python 是 `{"total": 3, "onProgress": <函数>}`，两样都在。JS 普通对象里的函数被调用时，`this` 是那个对象，所以 `{ count: 0, inc() { this.count++ } }` 跨进程也能用。
- **数据里的对象**：对象可以出现在任何深度的数据里（数组的元素、dict 的值、dataclass 的字段），各自按引用传。dataclass 的字段过去不许带引用，现在和 dict 相同。
- **Cordis 插件**：Node 里带 `apply` 函数的普通对象是 Cordis 的插件，照旧按对象引用发出，交给 `rutis::Definition` 时按插件定义读（[插件定义](design-plugin-values-2026-10-25.md) R1、R2）。这是 Cordis 自己对插件的定义，不是新规则。
- **可调用的对象**：Python 带 `__call__` 的实例照旧按函数引用。
- **Go 为什么要写**：Go 的结构体既是数据也可以有方法，值本身分不出来；声明为接口时按引用不用写，只有 `any` 要写 `rutis.Ref(v)`。
- **Swift、Kotlin 为什么要标注**：这两种语言在运行时列不出一个类的方法，方法表由 `@RutisService` 在编译时生成。它就是提供服务时已经在用的标注。

## 四、收到的是什么

收到的对象和 `ctx.use` 得到的服务代理是同一种：**每个方法都是一次异步的跨进程调用，调用的是原对象；没有数据字段**。要拿数据，让方法返回它。

| 语言 | 写法 | 收到的 |
| --- | --- | --- |
| JS（Node、Bun） | 不用声明 | 代理：`await cache.get('a')`；读到的每个名字都是方法（symbol、`then`、`toJSON`、`toString`、`valueOf` 除外，所以 `await` 一个代理得到它自己）；`console.log` 显示 `[object from py]` |
| Python | 不用声明 | 代理：`await cache.get("a")`，任何公开名字都是方法（和 `ctx.use` 一样）；`repr` 是 `<object from node>` |
| Go | 参数类型写成函数字段的结构体（和 `ctx.Use` 的目标一样，字段名按同样的规则对应方法名）；`any` 收到 `*rutis.Object` | 结构体的每个函数字段调用对应的方法（阻塞，第一个参数是 `context.Context`）；`*rutis.Object` 有 `Call(ctx, method, args...) (any, error)`，可以原样再交出 |
| Swift | 参数类型写成 `@RutisClient` 协议（`any Cache`）；`Any` 收到 `Rutis.RemoteObject` | 宏生成的同一个代理（`ctx.use(name, Cache.self)` 用的那个） |
| Kotlin | 参数类型写成 `@RutisClient` 接口；`Any?` 收到 `RemoteObject` | KSP 生成的同一个代理；`suspend` 方法挂起，普通方法阻塞（和服务客户端一样） |
| Rust | 宿主手写分发、Cordis 绑定用 `rutis_bridge::session::ObjectRef`（已有） | `call` / `call_async` |

- `*rutis.Object`、`Rutis.RemoteObject`、`RemoteObject` 和已有的 `*rutis.Function`、`Rutis.RemoteFunction`、`RemoteFunction` 对应：只在参数类型是 `any` / `Any` 时出现，用于原样转交或按名字调用。
- **同一个对象**：同一个对象交出几次，接收方拿到同一个代理（JS `===`、Python `is`）；代理交回原来的进程，那边拿到的就是原对象。交给第三个进程时，宿主照常转发。
- **同一个进程里**拿到的是对象本身（Cordis 的语义），方法照常同步、字段照常可读。跨进程时只有方法、方法都是异步的：这条写进边界规则（原则 7），测试工具的严格模式（默认）按跨进程的样子给代理，提前暴露差别。
- **调用的语义**和函数引用相同：参数、返回值按「值怎样传递」跨边界；对方抛出的错误照常传回；取消照常传到对面（[诊断](design-diagnostics-2026-10-26.md) §6.1）。Go 对象的方法可能被并发调用（已有的规则）。

## 五、生命周期

- **和函数引用相同**：每交出一次计一次；接收方的代理不再被引用时（JS `FinalizationRegistry`、Python `weakref.finalize`、Go `runtime.AddCleanup`、Swift `deinit`、Kotlin 的清理器，和各自的函数引用同一套）发 `Release`；计数到 0，发出方放掉这个对象。作为事件参数传过去的对象，照旧存活到那次调用结束。
- **不跟行走**：交出对象的行卸载后，对象照常存在、照常可以调用，和同一个进程里 Cordis 的对象一样。要在卸载时停用，由作者在清理里自己处理（和函数相同）。插件定义跟行走，是因为它会被挂出副本；对象没有这回事。
- **进程不在**：发出方的运行时退出或断开后，对代理的调用失败，错误码 `peer-gone`；进行中的调用也是。只有这次调用失败，代理之后的调用都同样失败。不会自动换到新进程（新进程里没有这个对象）。

## 六、线上格式与能力

- **线上格式不变**：`{"type":"reference","value":{"id","home","kind":"object","origin"}}`；方法调用是带 `method` 的 `call` 帧；`get` 帧（实时读属性）保留给 Rust 的 `ObjectRef::get` 和 Cordis，新代理不用。
- **能力 `objects`**（已有，现在只有 Rust 声明）：Node、Bun、Python、Go、Swift、Kotlin 在 endpoint 和 compat 两种握手里都声明；Rust 的 compat 声明也加上它。发出方和宿主转发时都按**收的一方**是否声明检查，两种握手都检查（现在只查 endpoint）。
- **与 0.17 混用**：
  - 0.18 的对象发给 0.17 的运行时：对方没有声明 `objects`，这次调用以 `bad-value` 失败，信息写明对方版本太旧（§八），会话继续。0.17 的一端从不会收到对象。
  - 0.18 的运行时连 0.17 的宿主：0.17 的宿主已经声明 `objects`，转发时也按下游的能力检查，线上格式没有变，所以对象照常在 0.18 的运行时之间传。
  - 不升协议版本，不加新能力名。

## 七、`undefined`

现在同一个值在 Python 里有两种样子：`[1, undefined, 3]` 作为参数是 `[1, UNDEFINED, 3]`，放在对象里是 `[1, None, 3]`。原因是 JS 把数组逐个元素编码（保留 `undefined` 标记），把纯数据的对象整个编成 JSON（`undefined` 变 `null`），Python 解码时又不归一。

规则（落实 V10）：

- **作为整个参数**：保留"没有给"的意思，和现在相同：JS 的 `f(1, undefined)`，Python 收到的是少一个参数或 `None`（现在的 `_apply`），Go 是零值。
- **作为整个返回值**：JS 收到 `undefined`，其他语言是各自的空值。
- **其余位置**（数组、对象、Map、Set 里任何深度，稀疏数组的空位）：一律是 `null` / `None` / `nil`。发的一方写 `null`；收的一方把收到的 `undefined` 标记也读成 `null`，所以旧的一端发来的也一致。
- 结果：`[1, undefined, 3]` 不论在哪一层，到 Python 都是 `[1, None, 3]`，到 JS 都是 `[1, null, 3]`。Python 插件的代码里不再出现 `UNDEFINED`。

## 八、开发者看到什么

错误码按[诊断](design-diagnostics-2026-10-26.md) §4.3；码不出现在给人看的文字里。前缀（行名、方法、第几个参数）按诊断设计的写法。

```ts
// TypeScript：把一个有方法的类的实例交给 Python 的服务
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

| 情况 | 看到的 | 码 |
| --- | --- | --- |
| 调用对象没有的方法 | `chat: warmer.warm failed: the Cache from node has no method gett (methods: get, set)` | `no-method` |
| JS 代理上把数据字段当方法读 | `… the Cache from node has no method size (size is a field; objects passed by reference expose only their methods)` | `no-method` |
| 对方进程已经退出 | `chat: a method of the Cache from py failed: runtime py exited` | `peer-gone` |
| 交给 0.17 的运行时 | `chat: argument 0 of warmer.warm: an object with methods (Cache) cannot cross to py 0.17.2, which does not take objects; upgrade it to rutis 0.18` | `bad-value` |
| Swift 没标注的类 | `argument 0 of warmer.warm: a Cache cannot cross: mark it @RutisService to pass it by reference, or make it Codable to copy it` | `bad-value` |
| Kotlin 没标注的类 | `argument 0 of warmer.warm: a Cache cannot cross: annotate it @RutisService to pass it by reference, or @Serializable to copy it` | `bad-value` |
| Go 参数类型收不了对象 | `chat: warmer.warm: argument 0 should be a string, got an object with methods (Cache from node)` | `bad-argument` |
| Swift / Kotlin 参数是没标 `@RutisClient` 的协议 / 接口 | `warmer.warm: argument 0 should be a Cache, but Cache is not @RutisClient, so an object cannot be received as it` | `bad-argument` |

[诊断](design-diagnostics-2026-10-26.md) §十"值是复制的"那条提示改为："按值复制；带方法的对象按引用，对方只能调用它的方法"。

## 九、Rust

- **宿主一侧**：所有跨运行时的对象都经宿主转发（已有），这一列测试的就是转发：任意两种语言之间、交回原进程、`Release` 传到源头、源头退出后 `peer-gone`。
- **宿主手写分发、Cordis 绑定**照旧用 `rutis_bridge::session::ObjectRef` 收对象、调用方法。
- **Rust 插件不在 0.18 里收发自己的对象**：Rust 不能在运行时为一个 trait 造出实现，要另做一个标在 trait 上的宏，现在没有使用方（判断标准 4）。Rust 插件收行为照旧用函数参数（`impl Fn`）；`#[rutis::service]` 的参数写成别的 rutis-bridge 类型时，照旧是编译错误，信息改为提示用函数参数。

## 十、一致性用例

一份共用的期望文件（和事件、值类型的一样），每种语言一列，测试工具的严格模式对同一份比对。

| 用例 | Node | Bun | Python | Go | Swift | Kotlin | Rust |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 发出带方法的对象，对方调用方法，原对象的状态改变 | 发 / 收 | 发 / 收 | 发 / 收 | 发（接口、`Ref`）/ 收（结构体、`*Object`） | 发 / 收（协议、`RemoteObject`） | 发 / 收（接口、`RemoteObject`） | 转发；`ObjectRef` 收 |
| 交出两次是同一个代理；交回原进程是原对象 | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | 转发 |
| 交给第三种语言 | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | 转发 |
| 作为返回值、作为数据里的元素、作为事件参数 | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | 转发 |
| 数据仍是数据：普通对象 / dict / 结构体 / `Codable` / `@Serializable` 复制；里面的函数按引用、`this` 是该对象（JS） | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | — |
| 没有方法的类的实例复制 | ✓ | ✓ | ✓ | — | — | — | — |
| 没有的方法：`no-method`，只有这次调用失败 | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |
| 接收方放掉代理：发出方的对象被释放 | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | 转发 |
| 发出方进程被杀：调用 `peer-gone`，会话和其他行不受影响 | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |
| 对方没有声明 `objects`：`bad-value`，会话继续 | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |
| 声明的类型收不了对象：`bad-argument` | — | — | — | ✓ | ✓ | ✓ | — |
| `undefined` 在任何深度是 `null`，作为整个参数保留 | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |

跨版本：发布门的跨版本相遇加两条：0.18 的对象发给 0.17 的运行时（`bad-value`，会话继续）；0.18 的运行时之间经 0.17 的宿主传对象（照常）。[保证表](guarantees.md)新增 `bridge/object-refs`（§三到§六）并改写值的 `undefined` 条目，各写明验证它的测试。

## 十一、兼容与迁移

0.18 已有迁移指南，写进《迁移到 0.18》（两份）。这是不兼容的变化，但只影响过去只能发给 Rust 的值：

- **JS**：带函数属性的普通对象过去整个按对象引用（只有 Rust 收），现在是数据加函数引用；没有方法的类的实例过去按对象引用，现在复制。用 `ObjectRef` 读这类值的 Rust 宿主改为按数据读。带 `apply` 的 Cordis 插件对象不变。
- **所有语言**：数组里的 `undefined` 现在是 `null`（过去 JS 发给 JS、Python 时保留 `undefined` / `UNDEFINED`）。
- **Python**：dataclass 的字段可以带函数和对象（过去报错）；没有公开方法的类的实例复制（过去按引用，只有 Rust 收）。
- **过去失败、现在成功**：带方法的对象发给 Node、Bun、Python、Go、Swift、Kotlin。
- 出错文字会变（§八）。线上格式、协议版本不变。

## 十二、要改的文档

- [插件 API](guide/plugin-api.md)（两份）：「值怎样传递」的"带方法的对象、类的实例"一行改为本稿 §三、§四；"数据里的 `undefined`"、"数组的空位"两行改为 §七；加一段"同一个进程与跨进程的差别"；测试工具差异表里"带方法的对象总是按引用传"那行改写。
- 各语言指南（两份）：[TypeScript](guide/typescript-plugin.md)、[Bun](guide/bun-plugin.md)、[Python](guide/python-plugin.md)、[Go](guide/go-plugin.md)（接口参数、函数字段结构体、`*rutis.Object`）、[Swift](guide/swift-plugin.md)、[Swift App](guide/swift-app.md)、[Kotlin App](guide/kotlin-app.md)（`@RutisClient` 参数、`RemoteObject`）、[Rust 宿主](guide/rust-host.md)（`ObjectRef` 收到的值的变化）、[Cordis](guide/cordis.md)。
- [协议设计](design-protocol-plugins-2026-09-25.md)：`objects` 能力的含义、两种握手都检查、`undefined` 的位置规则。
- 《迁移到 0.18》、[保证表](guarantees.md)。

## 十三、实施

| 阶段 | 内容 |
| --- | --- |
| O-1 | 规范与宿主：协议设计里的 `objects`、`undefined` 规则；Rust compat 握手声明 `objects`，转发在两种握手都检查；Rust 编解码里 `undefined` 在容器中写读为 `null`；对象调用的错误文字与码（码表随 #677）；共用的一致性期望文件与 Rust 列；保证条目 |
| O-2 | Node、Bun：§三 的发送规则（普通对象是数据、函数属性的 `this`、没有方法的类复制）；收对象的代理；声明 `objects`；测试工具严格模式；`undefined`；指南 |
| O-3 | Python、Go：发送规则（Python 没有公开方法的实例、dataclass 带引用；Go 接口参数按引用）；代理（Python）、函数字段结构体与 `*rutis.Object`（Go）；声明 `objects`；测试工具；`undefined`；指南 |
| O-4 | Swift、Kotlin：`@RutisClient` 类型的参数收对象、`RemoteObject`；声明 `objects`；测试工具；指南；《迁移到 0.18》与插件 API 指南的最终改写 |

每个阶段一个 PR，按顺序合并；后面的阶段复用 O-1 的期望文件。#687 在 O-4 合并、七列都通过后关闭。
