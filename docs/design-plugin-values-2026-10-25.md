# 插件是一个值：跨语言的插件定义 `rutis::Definition`

[English](design-plugin-values-2026-10-25.en.md)

状态：P1 已实现（2026-10-25，#642）：内核、rutis-bridge、rutis-loader、Node、Bun。P3 已实现（2026-10-26）：Swift、Kotlin，宿主和协议不变。P2（Python、Go）未做。

## 一、目标

在 Cordis 里，插件是一个值：一个函数，或者一个带 `apply` 的对象。它可以带着外层 `apply` 里准备好的东西交给别的插件；谁调用 `ctx.plugin(插件, 配置)`，这一份就挂在谁下面。同一个插件挂出的所有副本记在一起，插件重载时，`ctx.registry.delete(插件)` 一次结束全部副本（见 `@deepseek-ai/cordis` 4 的 `registry.ts`）。

本设计让这条规则在每种语言、跨语言都成立：

> 任何语言的插件定义，Rust 的也一样，都可以照常作为服务调用的参数或返回值交出。收的一方拿到的都是同一种类型 `rutis::Definition`，挂法也都一样：`ctx.plugin(def.at(&ctx, config)?)`。挂上的每一份在定义它的进程里运行，随它的来源结束。其他语言的插件作者不写任何 rutis 专用的东西；Rust 插件作者用 `Definition::new` 造一个定义。

收的一方不关心插件是哪种语言写的，Cordis 里也只有一种插件。所以 Rust 一侧只有一个类型，它的名字说的是"插件定义"这个东西本身，不分来源，和 Go 的 `rutis.Definition`、Swift 的 `Rutis.Definition` 同名。

不在本设计里：

- 生成器、登记处、登记事件。怎么登记、什么时候挂、挂到哪个实例里，都由用的一方决定。例如 Dim 用自己的部件登记处，写法和 Cordis 一样。
- 其他语言的新 SDK 方法。
- 由其他语言的服务收插件定义并挂上。叶子插件的 ctx 没有 `ctx.plugin()`，现在只有 Rust 一侧需要挂。
- loader 的实例组和配置行。这部分机制不变。

## 二、现状

| 情况 | 能否做到 |
| --- | --- |
| 同一个 Node 运行时里，一个 Cordis 插件把插件对象交给另一个 Cordis 插件，后者用 `ctx.plugin` 挂它 | 能，Cordis 原生支持 |
| Rust 插件把部件交给 Rust 服务 | 能，但没有共同的类型。Rust 0.17 的 `Plugin` 是一个**实例**：配置在结构体字段里，交给 `ctx.plugin(child)` 就用掉了，只能挂一次。能挂很多次、每次给不同配置的**定义**，今天只有构造闭包 `Fn(&C) -> Result<P>`（`plugin_from`、`Builtins::register` 用它），它不是一个能存、能跨语言传的值，每个服务只能自己定义部件的类型 |
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

第一个要用这个能力的是 Dim（arcships/dim ADR 006 D3）。在 Dim 里，每个插件只有一个进程级的行，它在 `apply` 里把 Session 级、分支级的部件交给 Dim 的部件登记处；Session 打开时，把登记的部件挂到自己下面。Session 在 Rust 里，部件可能是 Rust 写的，也可能是别的语言写的。现在：

- Rust 部件和其他语言的部件走两套：Rust 的交 Dim 自己定义的生成器类型，非 Rust 的只能交"模块名 + 配置"，按普通的行加载；
- 非 Rust 的部件拿不到插件 `apply` 里准备好的东西，只能经共享模块或服务转交；
- 非 Rust 的部件不随插件的这一代结束。插件卸载、重载时，Dim 要自己找出并卸载这些部件，还要保证先停部件、再停插件；
- 一个功能要拆成两个模块。

本设计落地之前，Dim 继续用现在的写法。

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
	Register func(ctx context.Context, scope string, def *rutis.Definition) error
}
if err := ctx.Use("parts", &parts); err != nil {
	return err
}
return parts.Register(ctx, "session", rutis.Define(rutis.Plugin[SessionConfig]{
	Inject: []string{"history"},
	Apply:  func(ctx *rutis.Ctx, config SessionConfig) error { /* 直接用 pool */ return nil },
}))
```

```rust
// Rust 插件：用 Definition::new 造一个定义，交给同一个服务
use rutis::{Ctx, Definition, Plugin};

impl Plugin for Git {
    type Deps = Arc<Parts>;
    async fn apply(&self, ctx: &Ctx, parts: Arc<Parts>) -> rutis::Result {
        let pool = open_pool().await?;
        parts.register("session".into(), Definition::new(ctx, move |_ctx: &Ctx, config: SessionConfig| {
            Ok(GitStatus::new(pool.clone(), config.depth))
        }));
        Ok(())
    }
}
```

收的一方只有一种参数类型 `Definition`，不管部件是哪种语言写的。挂的时候，先用 `at(&ctx, config)` 造出挂在这个位置、带着这份配置的插件，再交给 `ctx.plugin`：

```rust
use rutis::Definition;

#[rutis::service]
impl Parts {
    pub fn register(&self, scope: String, def: Definition) {
        self.add(scope, def); // 存起来；def.ended() 完成时撤掉
    }
}

// Session 打开时，在 Session 自己的 ctx 上挂
for def in parts.of("session") {
    ctx.plugin(def.at(&ctx, config.clone())?);
}
```

## 四、规则

| # | 决定 |
| --- | --- |
| R1 | **什么能当插件定义交出：** <br>· Rust：`Definition::new(ctx, build)` 造的定义。`build` 是 `Fn(&Ctx, C) -> Result<P>`，`C` 是配置类型（serde），`P` 是一个 Rust `Plugin`；`ctx` 是造它的插件自己的 ctx。<br>· 其他语言：各 SDK 的插件定义，包括 Node 和 Bun 的 `definePlugin` 结果、Python 的 `define_plugin` 结果、Go 的 `rutis.Define` 结果、Swift 和 Kotlin 的 `Rutis.define` 结果。它们按新的引用种类 `plugin` 发出（§五）。今天它们大多传不过去（§二），所以这不会改变任何能用的写法。<br>· Node 运行时里的 Cordis 插件（函数、类、带 `apply` 的对象）照旧按函数或对象引用发出，由收的一方认（R2）。 |
| R2 | **由收的一方决定：** Rust 的参数或返回值类型是 `Definition` 时，收到的 `plugin`、函数或对象引用都当作插件定义，宿主向定义它的运行时读一次声明（名字、`inject`、`provides`、配置 schema）。读的时机分三种：<br>· 参数：收到引用 → 读声明 → 调用 Rust 方法；<br>· Rust 调用其他语言的服务，对方返回插件：对方的方法完成 → 读声明 → Rust 调用方拿到 `Definition`；<br>· Rust 方法把 `Definition` 作为返回值交出：只能交回定义它的运行时（R3），不再读声明。<br>读不到声明（那个值不是插件）时，这次调用失败，错误写明是第几个参数或返回值。Rust 调用方直接传 `Definition`，不用读。其他类型照旧：要数据的参数收到 `plugin` 引用会报错，要函数的参数照旧按函数调用。一个 Cordis 对象交给要对象的参数，仍然是对象引用。 |
| R3 | **类型：** `rutis::Definition`，在 rutis 内核里，可以 `Clone`。方法：`new(ctx, build)`；`name()`；`at(&ctx, config)` 造出挂在 `ctx` 这个位置、带着这份配置的插件，交给 `ctx.plugin`；`ended()` 等到它失效的那一刻；`is_ended()`。同一个定义交出几次都是同一个：两个 `Definition` 相等，它们挂出的副本记在一起。它不参与普通的引用转发（§五）：其他语言的定义只能交回定义它的运行时，交回时用的是原来的引用，对方拿到的是原来的值；发给第三个运行时时编码失败。Rust 造的定义不能交给其他语言，编码失败。 |
| R4 | **挂在哪里：** `def.at(&ctx, config)` 按 `ctx` 所在的位置造出这一份。只能挂在来源所在的实例里，或者更里面的实例（挂载位置的作用域链以来源的作用域链开头）；来源在根上时不限；位置不对时 `at` 失败并说明原因。得到的插件只能在这个 `ctx` 上挂，挂到别处会启动失败。<br>· Rust 造的：`at` 用 `ctx` 和解码后的配置调用 `build`，`build` 从 `ctx` 读到所在的实例，和今天 Rust 生成器一样。<br>· 其他语言的：在实例里时，共享名字按实例隔离，门控和提供服务都用这个实例的键，与行在实例里的副本完全相同。属于进程的定义（R6）：本地运行时的不限；连进来的运行时的，只能挂在它的连接所在的实例里，或者更里面。这需要 rutis-loader，树上没有 loader 时 `at` 失败。 |
| R5 | **挂上的每一份：** <br>· 在定义它的进程里运行：Rust 造的在宿主进程里，其他语言的在定义它的运行时里；<br>· 门控：Rust 造的按 `P::Deps`，其他语言的按声明的 `inject`；<br>· 它提供的服务和同语言的插件一样发布；<br>· 归挂它的 fiber 所有，那个 fiber 关闭，这一份就结束；<br>· 配置在挂上时定下，每一份可以不同。Rust 造的解码成 `C`，其他语言的按它的 schema 校验；<br>· 要改配置，挂的一方卸掉这一份再用新配置重挂，和 0.17 里其他 Rust 子插件一样；<br>· 插件自己结束时，只有这一份结束。<br>其他语言的副本，在监听器描述、`CallerRow` 和错误里，`row` 是来源行的 id（属于进程的定义为空），`scope` 是挂载位置的实例链。这和行在实例里的副本一样：它们共用行的 id。 |
| R6 | **跟着来源结束：** <br>· Rust 造的：属于造它的那个插件的这一代（`new` 的 `ctx`）。<br>· 其他语言的：属于交出它的那一行的这一代。作为调用参数交出的，属于调用方的行；作为返回值交出的，属于提供这个服务的行；不在任何行的调用里交出的，属于那个运行时进程。同一个定义属于第一次交出它的那一行。宿主只在定义被交出时才知道它，而交出通常就发生在造它的 `apply` 里，所以和 Rust 的规则效果相同。<br>来源卸载、重启、自己结束时：<br>1. 先结束从这个定义挂出的每一份，等它们清理完；<br>2. 再运行来源自己的清理、卸载来源；<br>3. 然后 `ended()` 完成，之后再调 `at` 会失败。<br>其他语言的运行时进程退出时，副本直接结束（连接已经断了），`ended()` 完成。这和 Cordis 重载插件时 `registry.delete` 结束全部副本一样。 |
| R7 | **重载之后：** 新一代的 `apply` 交出的是新的定义，用的一方重新登记、重新挂；旧定义的副本这时已经结束（R6）。 |
| R8 | **检查：** `rutis-host check` 不知道运行时才交出来的插件定义。挂上以后，如果门控条件不满足，只有那一份等待，和行一样。 |
| R9 | **各语言的客户端：** <br>· Rust：参数、返回值直接写 `Definition`，`#[rutis::service]` 认它；dylib 插件经 `rutis-sdk` 拿到它；<br>· Go：函数字段直接传 `*rutis.Definition`，参数照常按值转换；<br>· Swift：`@RutisClient` 的参数类型写 `Rutis.Definition`；<br>· Kotlin：KSP 客户端接口的参数类型写 `Plugin`；<br>· TS：`rutis-host types` 把 `Definition` 写成 TS 的插件类型（`definePlugin` 的结果或 Cordis 插件）；<br>· Python、Node、Bun：不用声明。 |
| R10 | **测试工具：** 插件定义交给测试替身时，替身收到的值可以直接交给测试工具的 `load` 装上，和真实运行时一致。Node 严格模式下，冻结的 `definePlugin` 结果也能这样交出（今天会在 Proxy 里报错）。Python 测试工具今天能交出 `Plugin`，真实运行时却会报错，这次两者一致。 |

## 五、线上格式与运行时契约

本节只涉及其他语言的定义；Rust 造的定义不上线。

1. **新的引用种类 `plugin`：** 格式是 `{"type":"reference","value":{"id","home","kind":"plugin","origin"}}`，其余字段和函数、对象引用相同，生命周期也一样：按会话计数，用 `Release` 释放，会话结束时作废。各 SDK 的插件定义按这个种类发出（R1）。
2. **不参与普通转发：** 今天宿主收到的引用可以作为中继对象转发给别的会话。`plugin` 引用和其他语言的 `Definition`（不论底下是 `plugin`、函数还是对象引用）不走这条路：宿主要把它发给第三个运行时时，这次调用失败，错误写明插件定义只能交回定义它的运行时；交回定义它的运行时时，宿主发出原来的引用（`home: true`），不另建中继。
3. **能力 `plugins`：** 宿主在握手里声明 `plugins`，endpoint 和 compat 两种握手都声明。对端没有声明时，运行时发 `plugin` 引用的那次调用失败，错误写明"对端不接收插件定义"，会话继续。规则和值类型的能力一样（[值类型](design-value-types-2026-10-15.md) §四）。运行时在 `mount` 回复的 `features` 里加上 `plugin-values`；没有这个特性的运行时交出的引用，Rust 当插件定义收时报错，说明这个运行时的 SDK 太旧。
4. **控制调用 `plugins.read [value]`：** 参数就是那个值，作为引用传回定义它的运行时，在那里解码成原来的值。运行时用描述行的同一段代码（`rows.schema` 的 `describe`）回答 `{ name, config, inject, provides, announces }`。不是插件时报错。在 Node 里，Cordis 插件按 Cordis 的规则认：函数、类，或带 `apply` 的对象。
5. **`rows.load` 的 `entry` 可以是插件定义本身：** 和 `plugins.read` 一样，它作为引用传回定义它的运行时。其余参数（`key`、配置、`isolate`、`inject`、`exports`、作用域）和行完全相同。这一份在运行时里就是一行：`rows.unload`、`rows.ended` 都照旧；它的配置挂上后不变，所以不会收到 `rows.update`。Node 运行时把叶子插件包成 Cordis 插件，Cordis 插件值直接 `ctx.plugin`，和行一样。
6. **卸载顺序由宿主决定：** 宿主总是先卸载副本，再卸载来源行，运行时不需要记录副本属于哪一行。

## 六、怎么实现

1. **类型（rutis 内核）：** `Definition` 放在 rutis 内核，是根上的第 10 个名字。它不能放在 rutis-bridge：dylib 插件只依赖 `rutis-sdk`，而 `rutis-sdk` 只再导出 rutis；`#[rutis::service]` 生成的代码也只依赖 rutis。内核只放类型和一个对象安全的接口：
   - Rust 造的定义由内核自己实现。`at` 解码配置、调用 `build`，把得到的插件包一层，记进这个定义的副本表。
   - 其他语言的定义由 rutis-bridge 在加载时装进内核的钩子实现，做法和 `#[rutis::service]` 的 `Exposer` 一样。
   - 来源结束：内核在来源 fiber 停下时，先结束这个定义的副本并等它们清理完，再运行来源的清理（R6）。Rust 造的定义，来源就是 `new` 的 `ctx` 所在的 fiber；其他语言的定义，来源是交出它的那一行的 fiber（下面第 4 条）。
   - 位置检查（R4）：比较挂载位置和来源的作用域链（`ctx.scope()`），内核就能做。
2. **解码（rutis-bridge）：** 其他语言的 `Definition` 持有引用、读到的声明，以及来源（来源行这一代的 key，或者进程）。
   - 生成的服务里，参数种类在"函数""数据"之外加一种"插件定义"（宏的 `Param` 和 `rutis::service` 的 `Wire`）。`build.rs` 的服务清单（`Exports::service`）P1 时也认这个类型，0.17 随清单一起删除（#647）。
   - 解码出 `Definition` 时发 `plugins.read`。参数：生成的分发代码等它完成再调用方法。返回值：Rust 调用其他语言的服务，拿到回复后用 `rutis_bridge::runtime::returned(&*service, value).await` 读出定义，这一步读声明，读完才得到 `Definition`。
   - 来源在解码时确定：参数取 `caller_row()`，返回值取提供这个服务的行。
   - 编码 `Definition` 时检查要发往的会话：其他语言的定义发回定义它的运行时，就发原来的引用；其他情况都让这次调用失败（§五 第 2 条，R3）。
3. **挂载其他语言的定义（rutis-loader）：** `at(&ctx, config)` 经 rutis-bridge 定义、rutis-loader 提供的一个接口，找到树上的 loader。
   - loader 沿父 fiber 找到 `ctx` 所在的行和实例（和 `Loader::locate` 的找法一样），算出实例链和键，造出这一份插件（一个 `DynPlugin`，依赖在这时定下）。
   - 这一份启动时，走和行在实例里的副本同一段代码（`JsRow::start`）：租用宿主服务、发布服务、`rows.load`、处理 `rows.ended`。区别只有两点：运行时是来源的进程（`RowRuntime::Connection`），`entry` 是插件定义。
4. **结束其他语言的定义：** 其他语言的定义的来源就是来源行的 fiber（调用方 `caller_row()` 的 `ctx`），所以和 Rust 造的定义一样由内核结束（第 1 条）：来源行停下时，先结束副本并等它们清理完，再运行 `JsRow` 的清理（撤下这一行发布的服务、发 `rows.unload`）。loader 不另记副本。
   - 不属于任何行的定义，由 rutis-bridge 在进程的会话结束时结束。
   - 进程退出时，来源行跟着停下（运行时不在了，行回到等待），副本因此结束。
   - 之后 `ended()` 完成；宿主丢掉引用，运行时随之释放这个值。
5. **各运行时：**
   - Node、Bun：实现 `plugins.read` 和以插件定义为 `entry` 的 `rows.load`；`definePlugin` 的结果按 `plugin` 种类发出。
   - Python：`Plugin` 按 `plugin` 种类发出。
   - Go：`*rutis.Definition` 按 `plugin` 种类发出。
   - Swift、Kotlin：插件定义按 `plugin` 种类发出，客户端宏和 KSP 认插件定义类型。
   - 都复用各自的 `describe` 和装行的代码。

## 七、性能

| 方面 | 影响 |
| --- | --- |
| 交出一个定义 | Rust 造的没有额外开销。其他语言的，每收到一个做一次 `plugins.read` 往返；登记很少发生，不在常用路径上 |
| 挂一份 | Rust 造的和挂一个 Rust 子插件相同；其他语言的和在实例里装一行的副本相同 |
| 已有的调用 | 编码时多判断一次是不是插件定义，没有其他变化 |
| 内存 | 每个定义一张副本表；其他语言的定义多持有一个引用 |

## 八、考虑过的其他方案

| 方案 | 为什么不选 |
| --- | --- |
| 只给其他语言的插件一个类型（`rutis_bridge::ForeignPlugin`），Rust 部件照旧用各服务自己的类型 | 名字说的是来源，不是它是什么；收的一方要为 Rust 和其他语言写两套。而且 rutis-bridge 里的类型，只依赖 `rutis-sdk` 的 dylib 插件和 `#[rutis::service]` 生成的代码都看不到 |
| 不加类型，Rust 部件用构造闭包 `Fn(&C) -> Result<P>` | 闭包不是一个能交出、能跨语言传的值，也不知道自己属于哪个插件，副本没法随来源结束；其他语言的定义也装不进这个形状（依赖要到运行时才知道） |
| 类型叫 `PluginDef`，或者放进 `rutis::host` | 插件作者（比如 Dim 的登记处）要直接用它，所以放在根上；名字和 Go 的 `rutis.Definition`、Swift 的 `Rutis.Definition` 一致 |
| rutis 加 `ctx.generator(name, plugin)`，自己维护生成器的登记、列表和 `rutis/generator` 事件 | Cordis 没有这个方法，Cordis 插件用不了。登记处是用的一方的概念（Dim 的 ADR 006 D2 写明归 Dim），搬进 rutis 就多了一套 Cordis 没有的组合方式 |
| 作者用 `ctx.provide('sessionPart:git', plugin)` 交出插件，Session 按名字前缀找提供方 | 只用到已有的入口，但"把插件当服务提供、按前缀找"是 Cordis 里没有的约定，Cordis 也不能按前缀找提供方 |
| 保持"模块名 + 配置" | 部件拿不到 `apply` 里准备的东西，也不随插件的这一代结束（§二） |
| 只看发出方的种类，`plugin` 引用才是插件 | Cordis 插件就得包一层才能交出；带 `apply` 的普通对象也分不清。由收的一方的类型决定，Cordis 插件不用改 |
| 只看收的一方，不加 `plugin` 种类 | Python、Go、Swift、Kotlin 的插件定义今天根本传不过去（§二），总要有一种发法；新种类也让"不能调用、只能挂"这一点在线上就清楚 |
| 声明跟着引用一起发出，省掉 `plugins.read` | 只对 SDK 的定义有效，Cordis 插件仍然要读一次；两条路不如一条。登记很少，一次往返可以忽略 |
| 不要 `at`，像 Cordis 一样写 `ctx.plugin(def, config)` | Rust 没有重载，`ctx.plugin` 只收一个参数。而且依赖在 spawn 时就要确定（`DynPlugin::injects` 不带 ctx，[core/D32](design-config-hot-update-and-dynamic-events-2026-09-21.md)），定义自己不知道会挂在哪个实例里；实例里的名字各有各的键，必须先知道位置 |
| 用宿主层的 `plugin_with(工厂, 配置)`，之后可以原地改配置 | 那是宿主层（`host::CtxHost`）的方法；插件代码挂子插件用 `ctx.plugin`。0.17 的规则是配置在挂上时定下，改配置由挂的一方重建（[Rust API 收敛](design-rust-api-convergence-2026-10-24.md)）；Cordis 也是在 `ctx.plugin(插件, 配置)` 这一步交配置 |
| 运行时自己记录引用属于哪一行 | 六个运行时都要加这份记录。宿主在收到的那一刻就知道是哪一行交出的（调用方的行或服务的行），记在宿主一侧就够了 |
| 由其他语言的服务收插件定义并挂上 | 叶子插件的 ctx 没有 `ctx.plugin()`，现在只有 Rust 一侧要挂。以后需要时，按同样的线上格式补上 |

## 九、验收

- 一个 Node 行在 `apply` 里把一个插件交给一个 Rust 服务，这个插件的闭包用到了 `apply` 里准备的值。两个实例里的 Rust 宿主部件各用 `ctx.plugin(def.at(&ctx, 配置)?)` 挂一份，检查：
  - 两份都用到这个值；
  - 每一份只看得到自己实例的服务；
  - 声明的 `inject` 门控生效；
  - 两份的配置可以不同；
  - 卸掉一份再用新配置挂上，新的一份用新配置，旧的一份已经清理完。
- 一个 Rust 插件用 `Definition::new` 造一个定义，交给同一个服务的同一个方法；挂法相同，`build` 从 `ctx` 读到所在的实例。造它的插件卸载时，副本先结束，它自己的清理后运行；`ended()` 完成。
- 一个只依赖 `rutis-sdk` 的 dylib 插件里，带 `#[rutis::service]` 的服务能收 `Definition` 参数。
- Cordis 插件的对象和函数同样能交出并挂上；类插件也可以。
- 那个 Node 行卸载时，两份先结束，这一行才卸载完；`ended()` 完成，之后 `at` 失败。行重载后交来新的定义，能重新挂上。运行时进程被杀时，两份结束，`ended()` 完成。
- 挂到来源所在实例之外时，`at` 失败并说明原因。
- 普通参数照旧：要数据的参数收到插件定义时报错；Cordis 对象交给要对象的参数，仍然是对象引用。
- Rust 调用 Node 的服务，对方返回一个插件时，Rust 拿到的 `Definition` 已经读好声明；返回的不是插件时，这次调用失败。
- 把其他语言的 `Definition` 作为参数或返回值交给第三个运行时时，那次调用失败；交回定义它的运行时时，对方拿到原来的值。Rust 造的 `Definition` 交给其他语言时失败。
- 宿主没有声明 `plugins` 能力时，运行时交出插件的那次调用失败，会话继续。
- 测试工具：Node 严格模式下，插件定义交给测试替身，替身用 `load` 装上它；Python 测试工具和运行时的结果一致。
- 这些用例写成各运行时共用的一致性测试（和事件的一致性测试一样），后面跟进的运行时直接复用。
- [保证表](guarantees.md)新增三条：`core/definitions`（Rust 造的定义：R3、R4、R6）、`bridge/plugin-values`（其他语言的定义：R1–R3、§五）和 `loader/plugin-value-copies`（R4–R7）。

## 十、实施

| 阶段 | 内容 |
| --- | --- |
| P1 | 内核的 `Definition` 和 Rust 造的定义；线上格式与能力；宏、`Exports` 的参数种类；`plugins.read`；loader 的挂载与结束；Node 和 Bun（含测试工具）；TS 声明；公开面快照；保证条目；文档（[插件 API](guide/plugin-api.md)「值怎样传递」加"插件定义"一行，[写一个 Rust 插件](guide/rust-plugin.md)、[Rust 宿主](guide/rust-host.md)、TypeScript 和 Bun 插件指南） |
| P2 | Python 和 Go：SDK 发送、运行时、测试工具、指南 |
| P3 | Swift 和 Kotlin：SDK 发送、客户端宏和 KSP、测试工具、指南。连进来的 App 运行时只能挂在它的连接所在的实例里（R4） |

每个阶段一个 PR，按顺序合并；P2、P3 复用 P1 的一致性测试。
