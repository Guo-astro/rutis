# Rust 服务自动对外：类型上的 `#[rutis::service]`

[English](design-rust-service-exposure-2026-10-21.en.md)

状态：方案（2026-10-21），#510，0.15。取代 `build.rs` 清单作为 Rust 服务对外的主要方式；`rutis_bridge::build::Exports` 保留，只用于改不了源码的类型。

## 一、目标

服务对外应当全自动、开发者无感。Node / Bun / Python / Go 的叶子插件已经做到：`ctx.provide(name, impl)` 之后，其他语言就能按名字调用。Rust 这一侧没有做到。

本方案要做到：

> 写一个 Rust 服务的人，在它的 `impl` 块上加一行 `#[rutis::service]`，照常 `ctx.provide(...)`，所有语言的行就能按名字调用它。不写清单、不写 `build.rs`、不写 `HostDispatch`。内置插件和 dylib 插件一样。

## 二、现状

| 提供方 | 其他语言能否调用 |
| --- | --- |
| 叶子插件 `ctx.provide(name, impl)` | 能，全自动 |
| Rust 插件 `ctx.provide(value)` | 不能。要在宿主的 `build.rs` 用 `Exports` 列出类型并 `export_all`，或者手写 `HostDispatch` |

问题：
- 服务写在一个 crate，清单在宿主的另一个 crate 里。
- 没登记时，依赖它的行只会一直等待，没有任何提示。
- dylib 插件里的类型，宿主根本不应该知道。

结果下游仍在逐个手写绑定。

Rust 不能完全零标注：程序运行时看不到一个类型有哪些方法，翻译代码只能在编译时生成，所以总得有东西告诉编译器给哪个类型生成。本方案把这个"东西"从宿主的清单挪到类型自己身上，缩成一行。Swift 的 `@RutisService`、Kotlin 的 `@RutisService` 也是这样，Rust 与它们一致。Rust 生态里这也是常见写法，例如 `#[pymethods]`、`#[wasm_bindgen]`、`#[zbus::interface]`。

## 三、用法

```rust
use rutis::service;

pub struct Calendar { /* … */ }

#[service]                       // 对外名字默认 "calendar"（类型名转 camelCase）
impl Calendar {
    pub async fn add(&self, entry: NewEntry) -> Result<Entry, CalendarError> { /* … */ }
    pub fn list(&self, day: Date) -> Vec<Entry> { /* … */ }
    pub async fn watch(&self, ctx: &rutis::Ctx, handler: impl Fn(Entry) -> BoxFuture<'static, Result<(), Error>>) { /* … */ }
}

// 插件里照常提供；什么都不用多写
ctx.provide(Calendar::new())?;
```

```ts
// 任何语言的行，和调用叶子插件的服务一样
const calendar = ctx.use('calendar')
await calendar.add({ title: 'Standup', startsAt: new Date() })
```

改名用 `#[service(name = "cal")]`。这是唯一的参数。

## 四、规则

| # | 决定 |
| --- | --- |
| R1 | **对外的范围：** 带 `#[service]` 的 `impl` 块里的公开 `&self` 方法。一个类型可以有多个带标注的 `impl` 块，合起来就是它的方法。没有标注的类型不对外。宿主内部服务（存储、锁、数据库句柄等）默认不暴露，这是有意保留的边界。 |
| R2 | **提供即对外：** `ctx.provide(value)`（以及 `provide_as`，键是这个类型本身时）在同一个作用域里同时提供它的对外入口，两者由同一个撤销函数撤销，一起随插件卸载。不需要 `export_all`，也没有单独的"导出插件"。 |
| R3 | **作用域跟着提供的位置：** 在实例里提供，就只在那个实例里对外（等同今天的 `.service_in(…, group)`）；在实例外提供就是全局的。不再需要写实例组。 |
| R4 | **能传什么：** 与今天的 `Exports` 完全相同：serde 参数与返回值、`async` 与同步方法（对调用方都是 `Promise`）、按引用的回调、`&Ctx` 调用方行、`Result` 错误抛出（`name` / `code` / `data`）、带标签的值。规则写在 [Rust 宿主指南](guide/rust-host.md#导出服务参考)，不变。 |
| R5 | **出错在编译期：** 带标注的 `impl` 里有一个公开方法绑定不了（`&mut self`、借用返回值、不支持的参数），是**编译错误**，指到那个方法，提示改法或在方法上加 `#[service(skip)]`。今天是构建警告并悄悄跳过。 |
| R6 | **重名：** 同一作用域里两个类型对外名字相同，后提供的那个 `provide` 失败，错误写明双方的类型和插件。与叶子插件的重名规则一致。 |
| R7 | **没标注的诊断：** 行按名字依赖 `x`，运行时里没有叫 `x` 的服务，但有一个类型名转 camelCase 等于 `x` 的未标注 Rust 服务时，等待信息写明："`x` 是未对外的 Rust 服务 `crate::X`，在它的 `impl` 上加 `#[rutis::service]`"。 |
| R8 | **dylib 插件：** dylib 里带标注的类型在加载后同样对外，宿主不需要知道它。换代时旧代的对外入口随旧代的服务一起撤销，新代提供时重新对外，消费者按现有规则重载。 |
| R9 | **TypeScript 声明：** `rutis-host types <crate 目录> -o services.d.ts` 从源码里找出全部带标注的类型，写出与今天相同的声明（同一个生成器）。想每次构建自动更新的，`build.rs` 里写一行 `rutis_bridge::build::Exports::new().typescript("services.d.ts").generate()`，不用列类型。调用本身不需要声明。 |
| R10 | **兼容：** `build.rs` 的 `.service(…)` / `.service_in(…)` 继续可用，用于改不了源码的第三方类型。一个类型既有标注又在清单里，构建报错，提示删掉清单里那一行。手写 `HostDispatch` 继续可用。 |

## 五、怎么实现

三部分，都在 rutis 里，用户代码只看到 R1 的一行标注。

1. **编译期：`#[rutis::service]` 过程宏**（新 crate `rutis-macros`，由 `rutis` 再导出）。它读 `impl` 块里的方法签名，生成与今天 `Exports` 相同的 dispatcher：解码参数、调用、编码结果、映射错误。需要按类型做的判断（是不是 `Result`、错误怎么取 `code`、是不是回调）在展开后的具体代码里交给 trait 去选，所以类型别名（`io::Result`、自定义 `Result<T>`）照样能识别。生成器读源码的那部分只留给 R9 的声明。
2. **登记：** 宏为类型生成一个静态登记项（`TypeId`、对外名字、构造 dispatcher 的函数）。登记表在 `rutis` 里，进程启动或 dylib 加载时自动收集，不需要任何人列出来。dylib 的登记项记下它属于哪个模块和哪一代，查找时先用调用方自己那一代的。
3. **提供时挂上：** `Ctx::provide_as` 在提供一个具体类型时，用它的 `TypeId` 查一次登记表；有登记就在同一作用域、同一实例里多提供一个 `host_key(name)` 的 dispatcher，并把它并入同一个撤销函数（R2、R3）。`rutis` 核心只保存这张表和这个钩子，不依赖 `rutis-bridge`；dispatcher 的代码在宏展开的地方，依赖 `rutis-bridge`。

今天 `Exports` 的 dispatcher 代码、值规则、错误规则、`caller_row` 都原样复用，变的只有"谁来决定导出哪个类型"和"在哪里挂上去"。

## 六、性能

结论：调用没有额外开销；启动和提供的开销可以忽略；编译比今天的 `build.rs` 方式更快。

| 方面 | 影响 |
| --- | --- |
| 跨语言调用 | 与今天生成的 dispatcher 是同一份代码，开销不变。开销本来就在跨语言的那一步（编解码、进程间传输），与怎么登记无关。 |
| Rust 调 Rust | 完全不经过 dispatcher，标注不影响原生调用。 |
| `provide` | 多一次按 `TypeId` 的哈希查找（纳秒级）；有标注时多提供一个 `Arc`。没标注的类型只有那一次查找。 |
| 启动 | 收集登记项与带标注的类型数量成正比，每项是一个静态记录，微秒级。今天每个导出要挂一个等待原生服务的插件、每个实例组各一个，改成提供时挂上之后这些插件都没了。 |
| 编译 | 过程宏只处理带标注的 `impl` 块，开销与 serde 的 derive 相当。今天的 `build.rs` 每次要跑 `cargo metadata` 并解析依赖的源码树，改用标注后这一步只在生成声明时才发生（R9）。 |
| 二进制大小 | 每个对外方法一段 dispatcher，与今天相同；没标注的类型不生成任何代码。 |
| 内存 | 每个已提供的带标注服务多一个 dispatcher 对象。 |

## 七、考虑过的其他方案

| 方案 | 为什么不选 |
| --- | --- |
| 保持 `build.rs` 清单 | 清单离类型很远，必须写，漏写没有提示；dylib 插件的类型宿主不该知道。 |
| 不加标注，`provide` 的每个类型都自动对外 | 做不到：没有标注就没有编译期生成的代码，Rust 运行时又看不到方法。就算能做到，也会把宿主内部服务全部暴露给插件。 |
| 扫描整个依赖图，导出所有公开且有 `&self` 方法的类型 | 编译慢；暴露内部类型；无关类型之间名字冲突。 |
| 运行时用 serde 反射推结构 | 只能推出数据的形状，推不出方法，仍然需要标注；而且不支持 `untagged`、`flatten` 等常用写法。 |
| 在 trait 上标注，用 `provide_as::<dyn Trait>` 提供 | 多一层 trait 和键，比在 `impl` 上标注更啰嗦；直接在类型上标注已经覆盖这个需求。 |
| 提供时传名字，`ctx.provide_named("calendar", v)` | 改了 Rust 的提供方式，而且名字仍然只在提供的地方；默认用类型名、需要时在标注里改名，更自然。 |

## 八、验收

- 一个内置 Rust 插件只加 `#[rutis::service]` 并 `provide`，宿主不写 `build.rs`、不调用 `export_all`、不写 `HostDispatch`，Node、Bun、Python、Go 的行都能按名字调用它的方法，覆盖 serde 参数与返回值、同步与异步方法、`Result` 错误、回调、`&Ctx`。
- 同一个服务在两个实例里分别提供，两个实例的行各自拿到自己的那一份；在实例外提供时全局可用。
- 同样的服务放进 dylib 插件：加载后可调用；换代后旧入口撤销、新入口出现、消费者重载。
- 撤销 `provide` 返回的撤销函数后，其他语言立刻调不到它。
- 一个不能绑定的公开方法得到指向它的编译错误；`#[service(skip)]` 之后编译通过。
- 两个同名服务：第二个 `provide` 失败，错误写明双方。
- 未标注的 Rust 服务：依赖它的行得到 R7 的诊断。
- `rutis-host types` 写出的声明与今天 `Exports` 对同一类型写出的完全相同，并通过 `tsc --strict`。
- 现有 `build.rs` 导出的示例与测试不改也能通过；`examples/rust-services` 改成标注写法，另保留一个第三方类型的清单写法。

## 九、实施

一次做完，一个 PR：

1. `rutis-macros` 过程宏与 `rutis` 里的登记表、`provide_as` 钩子。
2. dispatcher 生成从 `build::Exports` 抽成宏与生成器共用的部分。
3. dylib 加载时的登记收集与按代查找。
4. `rutis-host types`；R7 的诊断；R10 的重复检查。
5. 文档：Rust 宿主指南"给插件提供 Rust 服务"改成以标注为主，清单写法挪到"第三方类型"；插件 API、agent skills；0.15 发布说明。中英两份。
