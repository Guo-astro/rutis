# Rust 服务自动对外：类型上的 `#[rutis::service]`

[English](design-rust-service-exposure-2026-10-21.en.md)

状态：已实现（2026-10-21），#510，0.15。取代 `build.rs` 清单作为 Rust 服务对外的主要方式；`rutis_bridge::build::Exports` 保留，只用于改不了源码的类型。

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
| R2 | **提供即对外：** `ctx.provide(value)`（以及 `provide_as`，键是这个类型本身时）在同一个作用域里同时提供它的对外入口，两者由同一个撤销函数撤销，一起随插件卸载。不需要 `export_all`，也没有单独的"导出插件"。`provide_mut_as` 的可变绑定不对外（入口持有的是提供时的值），带限定名的（`TypeKey::keyed`）也不对外。带 `check()` 谓词（`provide_as_with_check`）时，入口与原生绑定一样受它门控；每次调用都经提供方上下文的严格读取路径（`require_provided_as`）取服务：读取拦截器以调用方的行为读取方生效，与处在它位置的 Rust 读取方相同（没有行调用时为提供方）；读到的只能是入口提供时的那个绑定，之后在同一槽位提供的不算。因原生槽位被占而被拒的 provide 什么都不提供。 |
| R3 | **作用域跟着提供的位置：** 在实例里提供，就只在那个实例里对外（等同今天的 `.service_in(…, group)`）；在实例外提供就是全局的。在隔离作用域里提供（对它的键 `Ctx::isolate`）时，名字在同一个作用域里对外，两个隔离作用域各自对外。不再需要写实例组。提供了它的实例里的行用这个实例自己的，不退回同名的全局服务。 |
| R4 | **能传什么：** 与今天的 `Exports` 完全相同：serde 参数与返回值、`async` 与同步方法（对调用方都是 `Promise`）、按引用的回调、`&Ctx` 调用方行、`Result` 错误抛出（`name` / `code` / `data`）、带标签的值。规则写在 [Rust 宿主指南](guide/rust-host.md#导出服务参考)，不变。 |
| R5 | **出错在编译期：** 带标注的 `impl` 里有一个公开方法绑定不了（`&mut self`、借用返回值、不支持的参数），是**编译错误**，指到那个方法，提示改法或在方法上加 `#[service(skip)]`。带 `#[cfg]` 的方法只在包含它的构建里检查：宏求不了 cfg，所以它的错误放在同样的 cfg 下生成。今天是构建警告并悄悄跳过。 |
| R6 | **重名：** 同一作用域里两个类型对外名字相同，后提供的那个 `provide` 失败，错误写明双方的类型。与叶子插件的重名规则一致。 |
| R7 | **没标注的诊断：** 行按名字依赖 `x`，运行时里没有叫 `x` 的服务，但有一个按默认规则命名为 `x` 的未标注 Rust 服务时，`Loader::waiting_for(&entry)` 列出这一行等的名字，并写明："`x` 是未对外的 Rust 服务 `crate::X`，在它的 `impl` 上加 `#[rutis::service]`"。 |
| R8 | **dylib 插件：** dylib 里带标注的类型在加载后同样对外，宿主不需要知道它（插件 crate 只依赖 `rutis-sdk`，对着 sdk-bundle 构建时连依赖都不写；它经 `rutis_sdk::rutis::service` 拿到标注，那样生成的代码经 `rutis_sdk::rutis`）。换代时旧代的对外入口随旧代的服务一起撤销，新代提供时重新对外，消费者按现有规则重载。宿主要链接 `rutis-bridge`。 |
| R9 | **TypeScript 声明：** `rutis-host types <crate 目录> -o services.d.ts` 从源码里找出全部带标注的类型，写出与今天相同的声明（同一个生成器）。想每次构建自动更新的，`build.rs` 里写一行 `rutis_bridge::build::Exports::new().typescript("services.d.ts").generate()`，不用列类型。调用本身不需要声明。 |
| R10 | **兼容：** `build.rs` 的 `.service(…)` / `.service_in(…)` 继续可用，用于改不了源码的第三方类型。一个类型既有标注又在清单里，构建报错，提示删掉清单里那一行。手写 `HostDispatch` 继续可用。 |

## 五、怎么实现

三部分，都在 rutis 里，用户代码只看到 R1 的一行标注。

1. **编译期：`#[rutis::service]` 过程宏**（新 crate `rutis-macros`，由 `rutis` 再导出）。它读 `impl` 块里的方法签名，在块旁边生成一个分发函数：解码参数、调用、编码结果、映射错误。需要按类型做的判断（是不是 `Result`、错误怎么取 `code`、参数是不是函数）在展开后的具体代码里交给 trait 去选，所以类型别名（`io::Result`、自定义 `Result<T>`）照样能识别。生成的代码只依赖 `rutis`：值经 `rutis::service` 里的对象安全接口（`Wire`，以 erased-serde 传值）交给宿主的 `rutis-bridge` 编解码，所以只依赖 `rutis-sdk` 的 dylib 插件也能用。
2. **登记：** 宏为每个带标注的块生成一个静态登记项（`TypeId`、对外名字、方法名、分发函数），放进程序或动态库的初始化段（Linux `.init_array`、macOS `__mod_init_func`、Windows `.CRT$XCU`），在加载时加进 `rutis` 的登记表，不需要任何人列出来。dylib 类型的 `TypeId` 取决于 crate 的名字和元数据（含版本），不取决于源码：同一个版本重新构建的插件 `TypeId` 不变。所以登记表按代区分各个映像的登记项：dylib 加载器在 `rutis::service::loading` 里映射每个库，库的初始化段登记的项属于它自己的一代；库构建出的每个插件的 fiber 都标上这一代（`rutis::service::set_generation`）。带标注的值按提供它的代码所属的那一代的登记项对外，在提供时取定。那一代没有为这个类型登记（它的构建去掉了标注），或者代码不属于任何库时，只用程序自己的登记项（第 0 代，宿主与 SDK），否则就没有：绝不用另一个库的，那份代码与值的布局对不上。名字只在每个插件最新的构建里查（加载器每加载一次新构建就调用 `rutis::service::supersede`），新构建改名或去掉的名字就不再对外。登记表为每个映射过的映像的每个标注块保留一项，加载器为每个库保留一个代，映像永不卸载：两者都受加载器对每个插件的版本数上限约束。所以一次构建的值总是运行这次构建的分发代码，同一版本的两次构建可以同时加载，重新加载不需要改版本号。旧库永不卸载，取定的登记项一直有效。登记表的其他用途只读名字（`rutis-host check`、R7 诊断、目录里共享的名字），声明则从源码写出。`rutis-bridge` 用同样的方式在加载时把它的 `Exposer` 装进 `rutis`。
3. **提供时挂上：** `Ctx::provide_as` 提供一个类型时，按它的 `TypeId` 查登记表；有登记就交给 `Exposer`，在同一作用域、同一隔离作用域、同一实例里多提供一个 `host_key(name)`（实例里为 `host_key_in(name, id)`）的 dispatcher，并把它并入同一个撤销函数（R2、R3）；先检查原生槽位，被拒的 provide 什么都不提供。在实例里提供时，这个实例**认领**这个名字：rutis-loader 解析实例里的行的名字时先看认领（与运行时连接通告的名字同一套规则），新的认领会让已按全局键建好的副本重建。`rutis` 核心只保存登记表、钩子和接口，不依赖 `rutis-bridge`。

值规则、错误规则、`caller_row` 与 `build.rs` 生成的完全相同；TypeScript 声明仍由读源码的生成器写出，它在依赖 rutis 的 crate 里找带标注的块（R9）。

## 六、性能

结论：调用的开销与今天相当；启动和提供的开销可以忽略；编译比今天的 `build.rs` 方式更快。

| 方面 | 影响 |
| --- | --- |
| 跨语言调用 | 编解码规则与今天相同；值经 erased-serde 多一层动态分发（每个值几次虚调用），相对跨语言那一步（编解码、进程间传输）可以忽略。 |
| Rust 调 Rust | 完全不经过 dispatcher，标注不影响原生调用。 |
| `provide` | 遍历一次登记表比较 `TypeId`（登记项与带标注的块一样多，几十个以内是纳秒到微秒级）；有标注时多提供一个 `Arc`。 |
| 启动 | 每个带标注的块在加载时做一次原子插入。今天每个导出要挂一个等待原生服务的插件、每个实例组各一个，改成提供时挂上之后这些插件都没了。 |
| 编译 | 过程宏只处理带标注的 `impl` 块，开销与 serde 的 derive 相当。今天的 `build.rs` 每次要跑 `cargo metadata` 并解析依赖的源码树，改用标注后这一步只在生成声明时才发生（R9）。 |
| 二进制大小 | 每个对外方法一段分发代码，与今天相当；没标注的类型不生成任何代码。 |
| 内存 | 每个已提供的带标注服务多一个 dispatcher 对象；每个实例的认领是一个名字。 |

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
- 现有 `build.rs` 导出的示例与测试不改也能通过（`examples/rust-services`）；新示例 `examples/marked-services` 用标注写法。

## 九、实施

一个 PR：`rutis-macros` 与 `rutis::service`（登记表、钩子、`Wire`）、`rutis-bridge` 的 `Exposer` 与认领、rutis-loader 按认领解析与 `Loader::waiting_for`、生成器找带标注的块与重复检查、`rutis-host types`、测试（Node、Bun、Python、Go 的行，实例，dylib 换代）、文档。
