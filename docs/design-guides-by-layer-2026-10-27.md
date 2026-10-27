# 按层重写指南：插件作者的短路径，宿主持有者的「做一个开放插件的宿主」

[English](design-guides-by-layer-2026-10-27.en.md)

状态：定稿，未实现。日期：2026-10-27。基准：main `103c0d22`。排期：0.19。
Issue：[#671](https://github.com/arcships/rutis/issues/671)（含并入的 [#685](https://github.com/arcships/rutis/issues/685)）。属于 [#663](https://github.com/arcships/rutis/issues/663)（体验要求 2、3；A4、H2、O1、O5）。
与 0.18 的关系：指南写的是 0.18 之后的行为——配置按 Schema 补默认值（[配置 Schema](design-config-schema-2026-10-27.md)）、出错文字与 `Loader::status()` / `rutis-host status`（[诊断](design-diagnostics-2026-10-26.md) §4.5、§5）、各指南已有的「调试」一节（#679）。本稿不改这些行为。

本稿只定**指南的结构与几处必要的代码改动**。每节的具体例子、名字清单、要改哪些文件，在写那份指南的 PR 里定，审阅时按本稿的规则核对。

范围外：
- 别的 0.19 issue 的 API 与输出文字：`rutis-host call`、服务列表、测试工具的 `t.on`、本语言的假依赖（#670），契约描述与 Python 类型（#684），管住插件的选项（#683），安装命令与 API（#691），各 SDK 根上的名字（#692），远程配置（#698）。本稿只引用，不复述。
- Kotlin 插件指南：Kotlin 插件只在 Android 应用里由应用托管，没有 `rutis-host new` 与 `dev`。`kotlin-app.md`、`swift-app.md` 不改。
- 宿主持有者指南这一版只写 Rust 嵌入；以 `rutis-host` 为运行时的产品、Swift / Kotlin 应用作为宿主，#663 还没有评估过。
- 给 Rust 插件做模板或 `dev`；为指南另建示例 crate；文档注释的 CI 检查；`matches=` 比对标记（见 D7）。
- `rutis-agent`、`rutis-cli`、`aimux-llm` 的文档注释：示例项目，不在发布列车里。

## 一、现状与问题

**插件作者**。0.17.0 评估里，插件作者完成常见的事只用到 7 个概念：插件、服务、`inject`、配置、事件（`on` / `emit`）、测试工具、`dev` 与行。现在的指南在「跑起来」之前就讲了插件作为值、事件的五种方式、过滤、作用域、取消；示例是 `weather`，生成的项目是 `greeter`，把指南代码贴进生成的项目，模板的测试就坏了（#663 A1）；「行」只在 TypeScript、Python 指南里提一句，Go 指南没讲；只有 TypeScript、Python 指南的例子由文档示例检查跑。

**宿主持有者**。0.17.0 的宿主持有者评估（deskmate）里，#685 列的那些名字只能读 crate 源码才找到，loader 的几样零件「照抄了，没弄懂」。rust-host.md 是按 API 组织的参考，没有按「做一个开放插件的宿主」这件事写的指南。`use rutis_loader::EntryStatus` 报 `E0603`：crate 根上的 `pub(crate) use crate::report::*` 等内部导入占住了根路径。发布的 crate 里有约 280 行中文文档注释，主要在 `rutis/src/fiber.rs`。

## 二、决策

| # | 决策 |
| --- | --- |
| D1 | TypeScript、Bun、Python、Go、Rust、Swift 六份插件指南走同一条 8 步的短路径（§三），每步一个能原样运行的例子 |
| D2 | 例子统一用模板的 `greeter`：`rutis-host new greeter` 生成的项目就是第 2 步；之后加上使用 `translate` 服务、事件、测试、`dev` 与假的 `translate` |
| D3 | 第 7 步之前只出现 7 个概念，名字只用 #692 定下的根上名字；其余内容搬进 plugin-api.md 等参考文档（§四），原处留一行链接 |
| D4 | 每份指南在第 7 步用 `rutis.dev.json` 的两行讲清「行」 |
| D5 | rutis-host.md 开头面向组装插件的人：TypeScript + Python 的快速上手（用 #691 的安装命令、#670 的服务列表与 `call`）、各语言插件怎样装（从插件指南移过来）、最小的 `rutis.json` |
| D6 | 宿主持有者指南放新文件 `docs/guide/open-host.md`（两份），按 #685 的 6 件事写（§五），例子是命令行记事工具 `jot`；rust-host.md 保持 API 参考，开头加一行指向它 |
| D7 | 例子由已有的检查跑：TypeScript、Bun、Python、Go、Swift 的命令和文件用 `tools/doc-examples.mjs` 的 `run` / `file=` 标记；Rust 代码照现有做法作为 doctest 编译。只加两处小改动：doc-examples 认得 ` ```rust,no_run ` 这样带属性的代码块；Swift 的例子在 macOS 上跑 |
| D8 | 公开路径：删掉 rutis-loader 根上的 crate 内 glob 导入；#685 列的其余名字已有公开路径，指南按那些路径写 |
| D9 | 发布列车里 crate 的中文文档注释改成英文 |
| D10 | 各节在它依赖的 issue 合并后再写；验收以 0.19 发布后的 #663 评估为准 |

## 三、插件指南的短路径

### 3.1 八步

| 步 | 讲什么 | 依赖 |
| --- | --- | --- |
| 1 起项目 | `rutis-host new greeter`、安装依赖、跑测试 | — |
| 2 提供服务 | 生成的插件文件：定义插件、`provide`、`provides` 一句 | #692 |
| 3 使用服务 | `inject: ['translate']`、`use`；「都就绪才启动」一句 | #692 |
| 4 配置 | 模板里已有的 `greeting`：Schema 的 `default`，`apply` 收到补好默认值的配置；配置从哪里来（测试里传入，运行时来自宿主的行） | 0.18 #669 |
| 5 发出和监听事件 | `emit('greeter/greeted', …)`、`on` 各一个例子；其余方式一行链接插件 API | — |
| 6 测试 | `load`、取服务、监听事件、卸载；假的 `translate`；宿主给了契约描述时怎样用（一句，按 #684） | #670、#684 |
| 7 本地运行并试一下 | `rutis-host dev`；`rutis.dev.json` 的两行：和插件同 id 的那一行就是插件自己，另一行是本语言写的假 `translate`；用 #670 的服务列表与 `rutis-host call` 试一次；改代码看到重新加载 | #670 |
| 8 发布 | 包名、打 tag 的工作流、依赖范围一行；「宿主怎样装你的插件」一行链接 rutis-host.md | #691 |

路径之后，每份指南只留：「调试」（#679）、该语言特有的事（Swift 的「系统权限」、Bun 的「运行时的约定」），以及一行「进阶：插件 API」。

每步的代码块是整个文件，带 doc-examples 标记，所以 #663 A1 那种「指南代码配上模板的测试就坏」会在检查里失败。

### 3.2 各语言的差别

- **Bun**：命令换成 `bun` 的；仍写全 8 步，不让读者在两份指南之间跳。
- **Go、Swift**：编译成二进制，一个二进制是宿主里的一个运行时进程（第 1 步一句）；第 7 步的假 `translate`：Go 按 #670 定的写法；Swift 没有本语言的假依赖（#670 不做），用 TypeScript 或 Python 写。
- **Rust**：没有模板与 `dev`（Rust 插件编进 Rust 宿主）。第 1 步是 `cargo new --lib`；第 7 步是 `examples/` 里一个最小的 Rust 宿主，建根、装插件与假的 `translate`、调一次；第 8 步 `cargo publish`。Rust 的第 7 个概念因此是「宿主装插件」，不是「行」。

### 3.3 模板

模板保留现有的 `greeter.hello` 与 `greeting` 配置，只改注释：注释里只出现 7 个概念的事（去掉 `'sync'`、`ctx.effect` 这类），并指向指南的对应步骤。是否带假的 `translate` 由写指南的 PR 定（#731 把这件事留给本稿，用的是 #670 的机制）。

## 四、进阶内容去哪里

- 插件作为值、其余事件方式、过滤、作用域、取消、同步方法、严格模式、各语言测试工具的其余接口、Rust 的进阶写法 → plugin-api.md。`ctx` 上的方法照原样写（`ctx.parallel` 等，#692 不移动它们）；从根上移走的导出名按 #692 的新位置写。
- 在另一台机器上运行 → nodes.md（#698 改完远程配置之后搬）。
- 宿主怎样装各语言插件、Bun 的 `runtimes.bun` → rutis-host.md（已有）。
- 已有的 Cordis 插件 → cordis.md。

Rust 的进阶例子搬进 plugin-api.md 后，plugin-api.md 两份像 rust-plugin.md 一样由 `rutis` 的 `lib.rs` 以 `#[cfg(doctest)]` 引入，继续编译。

docs/guide/README.md 与根 README 的索引按层分组：给宿主写插件、用 rutis-host 组装插件、做一个开放插件的宿主、参考。

## 五、宿主持有者指南「做一个开放插件的宿主」

`docs/guide/open-host.md` / `open-host.en.md`。读者：在自己的 Rust 产品里接入 rutis，并把插件能力开放给别的开发者。全篇用 `jot`：宿主提供 `notes` 服务，开放一个扩展点 `commands.register(name, handler)`，第三方用 TypeScript 或 Python 写命令。

| 节 | 讲什么 | 依赖 |
| --- | --- | --- |
| 1 接入 | 依赖、建根、提供自己的服务、用 loader 装插件、等就绪（`reconcile` 会等每一行的 `apply`，卡住时怎么办）、关闭；loader 的每个零件（`Chain`、`with_shared`、`ServiceCatalog`、`RuntimeResolver`、`RuntimeRowsPlugin`、`Layer`、`Patch`）做什么、为什么这样组合；路径相对于哪里、`python_path`、`Launcher::env`、自定义 `Resolver` | 0.18 |
| 2 定义插件面 | 用 `#[rutis::service]` 写服务；回调注册（按引用传入的处理函数，`&Ctx` 是调用方的行，插件卸载时登记自动去掉）；`caller_row()`；给插件作者生成类型与契约描述——讲 #684 定下的做法 | #684 |
| 3 管住插件 | 讲 #683 定下的选项：进程划分、时限、能用哪些服务，以及超时或被拒时宿主和插件作者看到什么 | #683 |
| 4 管理插件 | 装、升级、卸载插件——讲 #691 定下的安装接口；增删行、改配置（`rutis_loader::edit`）；看一行现在怎样（`Loader::status()`）。插件也能管理插件：`inject: ['loader']`（#185），与 Cordis 的 `ctx.loader` 相同 | #691、#185 |
| 5 服务好插件作者 | 交给插件作者的模板与假服务、契约描述怎样用在测试里（按 #684）；开发循环（`RuntimeResolver::invalidate_all`、`Loader::reload`）；出错信息怎样改成宿主的话（按 0.18 #677） | #684、#670、0.18 #677 |
| 6 看得到状态 | 哪些行在跑、在等什么、运行时进程、进行中的调用与调用方：`Loader::status()`；要随变化做事时监听 `rutis/row`、`rutis/runtime` | 0.18 #678 |

- Rust 代码作为 doctest 编译：open-host.md 两份像 rust-host.md 一样由 `rutis-loader` 的 `lib.rs` 引入。第 1 节另用 doc-examples 写出 `Cargo.toml`、`src/main.rs` 再 `cargo run` 一次，所以这些代码块写成 ` ```rust,no_run `（D7）。
- 第 3、4、5 节只讲怎样用那几份设计定下的东西，字段与参数链接到 rust-host.md / rutis-host.md 的参考条目，不在本指南重复。

## 六、小的代码改动

- **公开路径**（D8）：删掉 rutis-loader crate 根上 `report`、`edit`、`resolve`、`internal`、`runtime`、`peer` 等模块的 `pub(crate) use …::*`，crate 内部改用模块路径。公开 API 不变（`public-api.txt` 不变）。之后指南里的每一行 `use` 由 doctest 保证能编译。
- **英文文档注释**（D9）：发布列车里各 crate 的 `src/` 中，`///`、`//!` 含中文的改成英文，按代码现在的行为写，旧注释里的设计编号删掉。实施时按当时的 main 列出文件。
- **文档示例检查**（D7）：`tools/doc-examples.mjs` 认代码块语言后面的属性（` ```rust,no_run ` 按 `rust` 处理）；`install.yml` 的「the guides' examples」一步在 `darwin-arm64` 上也跑，只跑 Swift 的例子（Swift 插件只在 macOS 上）。

## 七、兼容与迁移

- 不改插件 API、配置格式与行为，不进《迁移到 0.19》。插件作者不需要多写任何东西。
- 模板只改注释，只影响新建的项目。
- 删掉 rutis-loader 根上的 `pub(crate)` 导入不改变公开 API。

## 八、实施

| 步 | 内容 | 前提 |
| --- | --- | --- |
| 1 | §六的公开路径与英文文档注释 | 无 |
| 2 | doc-examples 认带属性的代码块；Swift 例子在 macOS 上跑 | 无 |
| 3 | plugin-api.md 收下进阶内容，作为 doctest 引入 | #692、0.18 |
| 4 | 六份插件指南按 §三重写，模板注释同步改；索引按层分组 | 第 2、3 步；#670；#698（搬「另一台机器」） |
| 5 | rutis-host.md 开头按 D5 重写 | #691、#670 |
| 6 | open-host.md 两份：第 1、6 节在 0.18 后；第 2、5 节在 #684 后；第 3 节在 #683 后；第 4 节在 #691、#185 后 | 第 1、2 步与各节的 issue |
| 7 | 验收：#663 的 0.19 评估里，每种语言的插件作者在第 7 步之前遇到的概念不超过 7 个；宿主持有者不读 crate 源码完成接入与开放。超出时在 0.19.x 里改指南 | 0.19 发布 |

#671「完成」各条：1 → 第 4 步；2 → 第 3 步；3「例子由文档示例检查跑」→ 第 2、4、6 步；4 → 第 7 步。#685：「指南两份，例子能编译运行」→ 第 6 步；「名字能从公开路径直接用」→ 第 1、6 步；「文档注释用英文」→ 第 1 步。
