# 插件值跨语言：另一种语言定义的插件，Rust 宿主也能挂上

[English](design-plugin-values-2026-10-25.en.md)

状态：设计（2026-10-25），#642。

## 一、目标

在 Cordis 里，插件是一个值：一个函数，或者一个带 `apply` 的对象。它可以带着外层 `apply` 里准备好的东西交给别的插件；谁调用 `ctx.plugin(插件, 配置)`，这一份就挂在谁下面。同一个插件挂出的所有副本记在一起，插件重载时，`ctx.registry.delete(插件)` 一次结束全部副本（见 `@deepseek-ai/cordis` 4 的 `registry.ts`）。

本设计让这条规则跨语言也成立：

> 一种语言定义的插件，照常作为服务调用的参数或返回值交给 Rust。Rust 收到的是一个可以挂的插件，用 `ctx.plugin` 挂上。挂上的每一份都在定义它的进程里运行，并随交出它的那一行的这一代结束。插件作者不需要写任何 rutis 专用的东西。

这个需求只在跨语言时才有。在同一个进程里，今天已经能这样做：Rust 插件之间可以，同一个 Node 运行时里的 Cordis 插件之间也可以；Cordis 本身也没有对应的问题。所以本设计只补跨语言这一步，不加新的组合方式。

不在本设计里：

- 生成器、登记处、登记事件。怎么登记、什么时候挂、挂到哪个实例里，都由用的一方决定。例如 Dim 用自己的部件登记处，写法和 Cordis 一样。
- 新的 SDK 方法。
- 由其他语言的服务收插件并挂上。叶子插件的 ctx 没有 `ctx.plugin()`，现在只有 Rust 一侧需要挂。
- Rust 调用方把 Rust 插件交给 Rust 服务。这仍然用 Rust 自己的类型；如果一个服务两种都要收，由服务自己处理。
- loader 的实例组和配置行。这部分机制不变。

## 二、现状

| 情况 | 能否做到 |
| --- | --- |
| Rust 插件把一个 Rust 插件交给 Rust 服务，服务用 `ctx.plugin` 挂它 | 能 |
| 同一个 Node 运行时里，一个 Cordis 插件把插件对象交给另一个 Cordis 插件，后者用 `ctx.plugin` 挂它 | 能，Cordis 原生支持 |
| Node、Python 等语言的插件把插件交给 Rust 服务 | 不能，见下 |

各 SDK 的插件定义，今天交给另一种语言时是这样：

| SDK | 插件定义 | 今天交给另一种语言时 |
| --- | --- | --- |
| Node、Bun | `definePlugin(...)` 的结果 | 按对象引用传过去，插件标记丢失 |
| Node 里的 Cordis 插件 | 函数、类、带 `apply` 的对象 | 函数和类按函数引用传，带 `apply` 的对象按对象引用传。类经引用调用时没有 `new`，会失败 |
| Python | `define_plugin(...)` 的结果（dataclass） | 报 `TypeError` |
| Go | `rutis.Define(...)` 的结果（`*rutis.Definition`） | 按数据传，变成空对象 `{}` |
| Swift | `Rutis.define(...)` 的结果（`Rutis.Definition`） | 报错 |
| Kotlin | `Rutis.define(...)` 的结果（`Plugin`） | 报错 |

Rust 一侧和运行时还有三处缺口：

- 生成的服务（`#[rutis::service]`、`build.rs` 的 `Exports`）的参数只认数据和函数，收到对象引用就报错。
- 引用的生命周期只按会话计数，没有记录它是哪一行交出来的。行卸载时，运行时既不释放也不作废它交出的引用。
- `rows.load` 和 `rows.schema` 只收模块名或二进制名。

第一个要用这个能力的是 Dim（arcships/dim ADR 006 D3）。在 Dim 里，每个插件只有一个进程级的行，它在 `apply` 里把 Session 级、分支级的部件交给 Dim 的部件登记处；Session 打开时，把登记的部件挂到自己下面。Session 在 Rust 里，部件可能是别的语言写的。现在非 Rust 的部件只能交"模块名 + 配置"，按普通的行加载，于是：

- 部件拿不到插件 `apply` 里准备好的东西，只能经共享模块或服务转交；
- 部件不随插件的这一代结束。插件卸载、重载时，Dim 要自己找出并卸载这些部件，还要保证先停部件、再停插件；
- 一个功能要拆成两个模块。

本设计落地之前，Dim 继续用"模块名 + 配置"。

## 三、用法

插件作者照常写插件，照常调用服务。下面例子里的 `parts` 是 Dim 的部件登记处，它就是一个普通的 Rust 服务。

```js
// Node / Bun 叶子插件
import { definePlugin } from '@arcships/rutis'

export default definePlugin({
  inject: ['parts'],
  async apply(ctx) {
    const pool = await openPool()
    await ctx.use('parts').register('session', definePlugin({
      inject: ['history'],
      config: { type: 'object', properties: { depth: { type: 'number' } } },
      apply(ctx, config) {
        ctx.provide('gitStatus', new GitStatus(pool, config.depth))
      },
    }))
  },
})
```

```js
// Cordis 插件：Cordis 自己的写法，不用改
export const inject = ['parts']
export async function apply(ctx) {
  const pool = await openPool()
  await ctx.parts.register('session', {
    inject: ['history'],
    apply(ctx, config) { /* 直接用 pool */ },
  })
}
```

```python
# Python
from rutis import define_plugin

async def apply(ctx, config):
    pool = await open_pool()

    def session(ctx, config):
        ctx.provide("gitStatus", GitStatus(pool, config["depth"]))

    await ctx.use("parts").register("session", define_plugin(session, inject=["history"]))
```

```go
// Go：函数字段的参数照常按值转换，不用改声明
var parts struct {
	Register func(ctx context.Context, scope string, part *rutis.Definition) error
}
if err := ctx.Use("parts", &parts); err != nil {
	return err
}
return parts.Register(ctx, "session", rutis.Define(rutis.Plugin[SessionConfig]{
	Inject: []string{"history"},
	Apply:  func(ctx *rutis.Ctx, config SessionConfig) error { /* 直接用 pool */ return nil },
}))
```

收的一方是 Rust。参数类型写成 `ForeignPlugin`；挂的时候，先用 `at(&ctx, config)` 造出挂在这个位置、带着这份配置的插件，再交给 `ctx.plugin`：

```rust
use rutis_bridge::ForeignPlugin;

#[rutis::service]
impl Parts {
    pub fn register(&self, scope: String, part: ForeignPlugin) {
        self.add(scope, part); // 存起来；part.ended() 完成时撤掉
    }
}

// Session 打开时，在 Session 自己的 ctx 上挂
for part in parts.of("session") {
    ctx.plugin(part.at(&ctx, config.clone())?);
}
```

## 四、规则

| # | 决定 |
| --- | --- |
| R1 | **什么能当插件交出：** 各 SDK 的插件定义，包括 Node 和 Bun 的 `definePlugin` 结果、Python 的 `define_plugin` 结果、Go 的 `rutis.Define` 结果、Swift 和 Kotlin 的 `Rutis.define` 结果。它们按新的引用种类 `plugin` 发出（§五）。今天它们大多传不过去（§二），所以这不会改变任何能用的写法。Node 运行时里的 Cordis 插件（函数、类、带 `apply` 的对象）照旧按函数或对象引用发出，由收的一方认（R2）。 |
| R2 | **由收的一方决定：** Rust 的参数或返回值类型是 `ForeignPlugin` 时，收到的 `plugin`、函数或对象引用都当作插件。方法运行前，宿主向定义它的运行时读一次声明（名字、`inject`、`provides`、配置 schema）。如果那个值不是插件，这次调用失败，错误写明是第几个参数或返回值。其他类型照旧：要数据的参数收到 `plugin` 引用会报错，要函数的参数照旧按函数调用。一个 Cordis 对象交给要对象的参数，仍然是对象引用。 |
| R3 | **Rust 一侧的类型：** `rutis_bridge::ForeignPlugin`，可以 `Clone`。它有四个方法：`name()`；`at(&ctx, config)` 造出挂在 `ctx` 这个位置、带着这份配置的插件，交给 `ctx.plugin`；`ended()` 等到它失效的那一刻；`is_ended()`。同一个值交出几次都是同一个插件：两个 `ForeignPlugin` 相等，它们挂出的副本记在一起。它不能再交给别的运行时，编码时会报错；交回定义它的运行时时，对方拿到的是原来的值。 |
| R4 | **挂在哪里：** `part.at(&ctx, config)` 按 `ctx` 所在的位置算出这一份的键。在实例里时，共享名字按实例隔离，门控和提供服务都用这个实例的键，与行在实例里的副本完全相同。得到的插件只能在这个 `ctx` 上挂，挂到别处会启动失败。只能挂在来源行所在的实例里，或者更里面的实例；来源行在根上时不限。属于进程的值（R6）：本地运行时的不限；连进来的运行时的，只能挂在它的连接所在的实例里，或者更里面。位置不对时 `at` 失败并说明原因。挂载要用 rutis-loader，树上没有 loader 时 `at` 失败。 |
| R5 | **挂上的每一份：** <br>· 在定义它的进程里运行；<br>· 按它声明的 `inject` 在 rutis 里门控；<br>· 它提供的服务按 catalog 发布，和行提供的服务一样；<br>· 归挂它的 fiber 所有，那个 fiber 关闭，这一份就结束；<br>· 配置在挂上时定下，按它的 schema 校验，每一份可以不同；<br>· 要改配置，挂的一方卸掉这一份再用新配置重挂，和 0.17 里其他 Rust 子插件一样；<br>· 插件自己结束时，只有这一份结束。<br>在监听器描述、`CallerRow` 和错误里，它的 `row` 是来源行的 id（属于进程的值为空），`scope` 是挂载位置的实例链。这和行在实例里的副本一样：它们共用行的 id。 |
| R6 | **跟着来源结束：** 插件值属于交出它的那一行的这一代：作为调用参数交出的，属于调用方的行；作为返回值交出的，属于提供这个服务的行；不在任何行的调用里交出的，属于那个运行时进程。同一个值属于第一次交出它的那一行。这一行卸载、重载、自己结束时：<br>1. 先结束从这个值挂出的每一份，等它们清理完；<br>2. 再卸载这一行；<br>3. 然后 `ended()` 完成，之后再调 `at` 会失败。<br>进程退出时，副本直接结束（连接已经断了），`ended()` 完成。这和 Cordis 重载插件时 `registry.delete` 结束全部副本一样。 |
| R7 | **重载之后：** 新一代的 `apply` 交出的是新的值，用的一方重新登记、重新挂；旧值的副本这时已经结束（R6）。 |
| R8 | **检查：** `rutis-host check` 不知道运行时才交出来的插件。挂上以后，如果门控条件不满足，只有那一份等待，和行一样。 |
| R9 | **各语言的客户端：** <br>· Go：函数字段直接传 `*rutis.Definition`，参数照常按值转换；<br>· Swift：`@RutisClient` 的参数类型写 `Rutis.Definition`；<br>· Kotlin：KSP 客户端接口的参数类型写 `Plugin`；<br>· TS：`rutis-host types` 把 `ForeignPlugin` 写成 TS 的插件类型（`definePlugin` 的结果或 Cordis 插件）；<br>· Python、Node、Bun：不用声明。 |
| R10 | **测试工具：** 插件值交给测试替身时，替身收到的值可以直接交给测试工具的 `load` 装上，和真实运行时一致。Node 严格模式下，冻结的 `definePlugin` 结果也能这样交出（今天会在 Proxy 里报错）。Python 测试工具今天能交出 `Plugin`，真实运行时却会报错，这次两者一致。 |

## 五、线上格式与运行时契约

1. **新的引用种类 `plugin`：** 格式是 `{"type":"reference","value":{"id","home","kind":"plugin","origin"}}`，其余字段和函数、对象引用相同，生命周期也一样：按会话计数，用 `Release` 释放，会话结束时作废。各 SDK 的插件定义按这个种类发出（R1）。
2. **能力 `plugins`：** 宿主在握手里声明 `plugins`，endpoint 和 compat 两种握手都声明。对端没有声明时，运行时发 `plugin` 引用的那次调用失败，错误写明"对端不接收插件值"，会话继续。规则和值类型的能力一样（[值类型](design-value-types-2026-10-15.md) §四）。运行时在 `mount` 回复的 `features` 里加上 `plugin-values`；没有这个特性的运行时交出的引用，Rust 当插件收时报错，说明这个运行时的 SDK 太旧。
3. **控制调用 `plugins.read [value]`：** 参数就是那个值，作为引用传回定义它的运行时，在那里解码成原来的值。运行时用描述行的同一段代码（`rows.schema` 的 `describe`）回答 `{ name, config, inject, provides, announces }`。不是插件时报错。在 Node 里，Cordis 插件按 Cordis 的规则认：函数、类，或带 `apply` 的对象。
4. **`rows.load` 的 `entry` 可以是插件值本身：** 和 `plugins.read` 一样，它作为引用传回定义它的运行时。其余参数（`key`、配置、`isolate`、`inject`、`exports`、作用域）和行完全相同。这一份在运行时里就是一行：`rows.unload`、`rows.ended` 都照旧；它的配置挂上后不变，所以不会收到 `rows.update`。Node 运行时把叶子插件包成 Cordis 插件，Cordis 插件值直接 `ctx.plugin`，和行一样。
5. **卸载顺序由宿主决定：** 宿主总是先卸载副本，再卸载来源行，运行时不需要记录副本属于哪一行。

## 六、宿主一侧怎么实现

1. **类型和解码（rutis-bridge）：** `ForeignPlugin` 持有引用、读到的声明，以及来源（来源行这一代的 key，或者进程）。
   - 生成的服务里，参数种类在"函数""数据"之外加一种"插件"（宏的 `Param` 和 `rutis::service` 的 `Wire`），`build.rs` 的 `Exports` 也认这个类型。
   - 解码出 `ForeignPlugin` 时发 `plugins.read`，生成的分发代码等它完成再调用方法。
   - 来源在解码时确定：参数取 `caller_row()`，返回值取提供这个服务的行。
2. **挂载（rutis-loader）：** `at(&ctx, config)` 经 rutis-bridge 定义、rutis-loader 提供的一个接口，找到树上的 loader。
   - loader 沿父 fiber 找到 `ctx` 所在的行和实例（和 `Loader::locate` 的找法一样），算出实例链和键，造出这一份插件（一个 `DynPlugin`，依赖在这时定下）。
   - 这一份启动时，走和行在实例里的副本同一段代码（`JsRow::start`）：租用宿主服务、发布服务、`rows.load`、处理 `rows.ended`。区别只有两点：运行时是来源的进程（`RowRuntime::Connection`），`entry` 是插件值。
3. **结束（rutis-loader）：** loader 为每个来源记下从它的插件值挂出的副本。
   - 来源行卸载时（`JsRow` 的清理），先让这些副本自己结束（`dispose_self`），等它们清理完，再撤下这一行发布的服务、发 `rows.unload`。
   - 进程退出时，发布的服务随连接断开撤下，副本按 `rows.ended` 的同一条路径结束。
   - 之后 `ended()` 完成；宿主丢掉引用，运行时随之释放这个值。
4. **各运行时：**
   - Node、Bun：实现 `plugins.read` 和以插件值为 `entry` 的 `rows.load`；`definePlugin` 的结果按 `plugin` 种类发出。
   - Python：`Plugin` 按 `plugin` 种类发出。
   - Go：`*rutis.Definition` 按 `plugin` 种类发出。
   - Swift、Kotlin：插件定义按 `plugin` 种类发出，客户端宏和 KSP 认插件定义类型。
   - 都复用各自的 `describe` 和装行的代码。

## 七、性能

| 方面 | 影响 |
| --- | --- |
| 交出一个插件 | 每收到一个值做一次 `plugins.read` 往返。登记很少发生，不在常用路径上 |
| 挂一份 | 和在实例里装一行的副本相同 |
| 已有的调用 | 编码时多判断一次是不是插件定义，没有其他变化 |
| 内存 | 每个来源记一张副本表；每个 `ForeignPlugin` 一个引用 |

## 八、考虑过的其他方案

| 方案 | 为什么不选 |
| --- | --- |
| rutis 加 `ctx.generator(name, plugin)`，自己维护生成器的登记、列表和 `rutis/generator` 事件 | Cordis 没有这个方法，Cordis 插件用不了。登记处是用的一方的概念（Dim 的 ADR 006 D2 写明归 Dim），搬进 rutis 就多了一套 Cordis 没有的组合方式 |
| 作者用 `ctx.provide('sessionPart:git', plugin)` 交出插件，Session 按名字前缀找提供方 | 只用到已有的入口，但"把插件当服务提供、按前缀找"是 Cordis 里没有的约定，Cordis 也不能按前缀找提供方 |
| 保持"模块名 + 配置" | 部件拿不到 `apply` 里准备的东西，也不随插件的这一代结束（§二） |
| 只看发出方的种类，`plugin` 引用才是插件 | Cordis 插件就得包一层才能交出；带 `apply` 的普通对象也分不清。由收的一方的类型决定，Cordis 插件不用改 |
| 只看收的一方，不加 `plugin` 种类 | Python、Go、Swift、Kotlin 的插件定义今天根本传不过去（§二），总要有一种发法；新种类也让"不能调用、只能挂"这一点在线上就清楚 |
| 声明跟着引用一起发出，省掉 `plugins.read` | 只对 SDK 的定义有效，Cordis 插件仍然要读一次；两条路不如一条。登记很少，一次往返可以忽略 |
| 不要 `at`，像 Cordis 一样写 `ctx.plugin(part, config)` | Rust 没有重载，`ctx.plugin` 只收一个参数。而且依赖在 spawn 时就要确定（`DynPlugin::injects` 不带 ctx，[core/D32](design-config-hot-update-and-dynamic-events-2026-09-21.md)），插件值自己不知道会挂在哪个实例里；实例里的名字各有各的键，必须先知道位置 |
| 用宿主层的 `plugin_with(工厂, 配置)`，之后可以原地改配置 | 那是宿主层（`host::CtxHost`）的方法；插件代码挂子插件用 `ctx.plugin`。0.17 的规则是配置在挂上时定下，改配置由挂的一方重建（[Rust API 收敛](design-rust-api-convergence-2026-10-24.md)）；Cordis 也是在 `ctx.plugin(插件, 配置)` 这一步交配置 |
| 运行时自己记录引用属于哪一行 | 六个运行时都要加这份记录。宿主在收到的那一刻就知道是哪一行交出的（调用方的行或服务的行），记在宿主一侧就够了 |
| 由其他语言的服务收插件并挂上 | 叶子插件的 ctx 没有 `ctx.plugin()`，现在只有 Rust 一侧要挂。以后需要时，按同样的线上格式补上 |

## 九、验收

- 一个 Node 行在 `apply` 里把一个插件交给一个 Rust 服务，这个插件的闭包用到了 `apply` 里准备的值。两个实例里的 Rust 宿主部件各用 `ctx.plugin(part.at(&ctx, 配置)?)` 挂一份，检查：
  - 两份都用到这个值；
  - 每一份只看得到自己实例的服务；
  - 声明的 `inject` 门控生效；
  - 两份的配置可以不同；
  - 卸掉一份再用新配置挂上，新的一份用新配置，旧的一份已经清理完。
- Cordis 插件的对象和函数同样能交出并挂上；类插件也可以。
- 那个 Node 行卸载时，两份先结束，这一行才卸载完；`ended()` 完成，之后 `at` 失败。行重载后交来新的值，能重新挂上。运行时进程被杀时，两份结束，`ended()` 完成。
- 挂到来源行所在实例之外时，`at` 失败并说明原因。
- 普通参数照旧：要数据的参数收到插件定义时报错；Cordis 对象交给要对象的参数，仍然是对象引用。
- 宿主没有声明 `plugins` 能力时，运行时交出插件的那次调用失败，会话继续。
- 测试工具：Node 严格模式下，插件值交给测试替身，替身用 `load` 装上它；Python 测试工具和运行时的结果一致。
- 这些用例写成各运行时共用的一致性测试（和事件的一致性测试一样），后面跟进的运行时直接复用。
- [保证表](guarantees.md)新增两条：`bridge/plugin-values`（R1–R3、§五）和 `loader/plugin-value-copies`（R4–R7）。

## 十、实施

| 阶段 | 内容 |
| --- | --- |
| P1 | 线上格式与能力；`ForeignPlugin` 和宏、`Exports` 的参数种类；`plugins.read`；loader 的挂载与结束；Node 和 Bun（含测试工具）；TS 声明；保证条目；文档（[插件 API](guide/plugin-api.md)「值怎样传递」加"插件"一行，[Rust 宿主](guide/rust-host.md)、TypeScript 和 Bun 插件指南） |
| P2 | Python 和 Go：SDK 发送、运行时、测试工具、指南 |
| P3 | Swift 和 Kotlin：SDK 发送、客户端宏和 KSP、测试工具、指南。连进来的 App 运行时只能挂在它的连接所在的实例里（R4） |

每个阶段一个 PR，按顺序合并；P2、P3 复用 P1 的一致性测试。
