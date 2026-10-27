# 按层重写指南：插件作者的短路径，宿主持有者的「做一个开放插件的宿主」

[English](design-guides-by-layer-2026-10-27.en.md)

状态：草案，未实现。日期：2026-10-27。基准：main `103c0d22`。排期：0.19。
Issue：[#671](https://github.com/arcships/rutis/issues/671)（含并入的 [#685](https://github.com/arcships/rutis/issues/685)）。属于 [#663](https://github.com/arcships/rutis/issues/663)（体验要求 2、3；A4、H2、O1、O5）。
与 0.18 的关系：指南写的是 0.18 之后的行为——配置按 Schema 补默认值（[配置 Schema](design-config-schema-2026-10-27.md)）、出错文字与 `Loader::status()` / `rutis-host status`（[诊断](design-diagnostics-2026-10-26.md) §4.5、§5）、各指南已有的「调试」一节（#679）。本稿不改这些行为，只决定指南怎样写、写在哪里。

这份设计稿是**重写指南的计划**：定下每份指南的新结构、每一步讲什么和用哪个例子、哪些内容搬到哪里、宿主持有者指南的目录和例子、要做的少量代码改动，以及怎样验收。指南本身在实施步骤里重写。

范围外：
- 别的 0.19 issue 的 API 与输出文字：`rutis-host call` 与服务列表（#670）、严格假服务与 Python 类型（#684）、`管住插件` 的选项（#683）、`rutis-host add` 与安装 API（#691）、各 SDK 根上的名字（#692）。本稿只引用，不替它们设计。
- Kotlin 插件指南：Kotlin 插件只在 Android 应用里由应用托管，没有 `rutis-host new` 与 `dev`，短路径的第 1、7 步不存在；它的读者是应用作者。`kotlin-app.md`、`swift-app.md` 不改。
- 以 `rutis-host` 为运行时的产品、Swift / Kotlin 应用作为宿主：#663 还没有评估过，宿主持有者指南这一版只写 Rust 嵌入。
- 给 Rust 插件加 `rutis-host new --lang rust` 或 `dev`：Rust 插件编进 Rust 宿主，没有对应的运行时；Rust 的短路径在第 7 步用一个最小的 Rust 宿主（§3.3）。
- 调试指南的内容（#679，0.18 已在各指南加「调试」一节，本稿只定它的位置）。
- `rutis-agent`、`rutis-cli`、`aimux-llm` 的文档注释：示例项目，不在发布列车里（[发布](release.md)）。

## 一、现状与问题

**插件作者**。0.17.0 评估里，插件作者完成常见的事只用到 7 个概念：插件、服务、`inject`、配置、事件（`on` / `emit`）、测试工具、`dev` 与行。现在的指南却是这样：

- TypeScript 指南第 2 节讲完 `weather` 就讲「插件本身也是一个值」；第 3 节「测试」里讲事件的五种方式、`filter`、`prepend`、作用域、取消，例子是两个插件在 `host()` 上用 `serial`、`waterfall` 互相改写问候语。Python、Go、Swift 指南同样。
- 生成的项目是 `greeter.hello`，指南示例是 `weather`：把指南代码贴进生成的项目，模板的测试就坏了（#663 A1）。
- 「行」「和插件同 id 的那一行就是插件自己的配置」只在 TypeScript、Python 指南第 4 节末尾提一句，Go 指南没讲。
- 每份指南末尾都有「被宿主使用」，写各语言的安装命令；这是宿主作者的事，rutis-host.md 里已经有一张完整的表（0.17.2 #666）。
- 只有 TypeScript、Python 指南的例子由文档示例检查跑（`tools/doc-examples.mjs`，发布后在 Linux 上照着跑）；Rust 指南的例子是 crate 的 doctest；Bun、Go、Swift 的例子没人检查。

**宿主持有者**。0.17.0 的宿主持有者端到端评估（deskmate）里，下面这些只能读 crate 源码才找到：行的状态（`Loader::entries()`、`EntryStatus`、`waiting_for`、`settled`）、`reconcile` 会等所有插件的 `apply`、`LocalRuntime::python_path`、`Launcher::env`、`processes().kill()`、`RuntimeState`、标注服务里的 `caller_row()`、自定义 `Resolver`，以及 loader 那几样零件为什么这样组合（「照抄了，没弄懂」）。rust-host.md 是按 API 组织的参考，没有一份按「做一个开放插件的宿主」这件事来写的指南。另外：

- `use rutis_loader::EntryStatus` 报 `E0603 enum import EntryStatus is private`：crate 根上有 `pub(crate) use crate::report::*`（以及 `edit`、`resolve`、`internal`、`runtime`、`peer` 的同样写法），根路径被这条内部导入占住，公开路径 `rutis_loader::report::EntryStatus` 反而要猜。
- 发布的 crate 里有约 280 行中文文档注释（`FiberState`、`Snapshot`、`shutdown_with_timeout` 等），docs.rs 上不好读。

## 二、决策

| # | 决策 |
| --- | --- |
| D1 | TypeScript、Bun、Python、Go、Rust、Swift 六份插件指南改成同一条 8 步的短路径（§三），每步一个能原样运行的例子；Kotlin 不在其内（见范围外） |
| D2 | 例子统一用模板的 `greeter`：`rutis-host new greeter` 生成的项目就是第 2 步的样子；第 3–7 步依次加上使用 `translate` 服务、配置 `greeting`、发出 `greeter/greeted`、测试、`dev` 与假的 `translate`。模板因此改成第 2 步的样子（去掉模板里的配置，挪到第 4 步） |
| D3 | 第 7 步之前，指南只出现 7 个概念；其余内容搬进 plugin-api.md、cordis.md、nodes.md、rutis-host.md（§四的对照表），原处只留一行链接 |
| D4 | 每份指南自己讲清「行」：在第 7 步，用 `rutis.dev.json` 的两行讲 |
| D5 | rutis-host.md 的开头面向组装插件的人：快速上手改用 `rutis-host add`（#691）、启动后看到服务列表并用 `rutis-host call` 调一次（#670）、`rutis-host status`（0.18）；各语言插件的手动装法留作参考表 |
| D6 | 宿主持有者指南放在**新文件** `docs/guide/open-host.md`（两份），按宿主要完成的 6 件事写（§五）；rust-host.md 保持 API 参考，开头加一行指向它。#683 说的「rust-host.md 一节『管住插件』」写进新指南的第 3 节，选项的参考行由 #683 加在 rust-host.md 的表里 |
| D7 | 指南里的例子都由检查跑：插件指南与宿主持有者指南的命令和文件用 `tools/doc-examples.mjs` 的标记；Rust 代码另外作为 doctest 编译（已有的做法）。doc-examples 加两样：`matches=<路径>`（这一块必须和项目里的文件一字不差，锁住指南与模板一致）、带属性的代码块（` ```rust,no_run `）；Swift 的例子在 macOS 上跑 |
| D8 | 公开路径：删掉 rutis-loader 根上的 crate 内 glob 导入，让根路径不再被占；其余 #685 列的名字已经在公开路径下，指南按那些路径写，由 doctest 锁住 |
| D9 | 发布列车里 crate 的文档注释改成英文（§六列出文件），CI 加一条检查 |
| D10 | 实施排在依赖的功能之后：各节对应的 issue 合并后再写那一节（§九）；验收以 0.19 发布后的 #663 评估为准：插件作者在「跑起来」之前遇到的概念不超过 7 个 |

## 三、插件指南的短路径

### 3.1 八步

以 TypeScript 为例；每步的代码块是**整个文件**（读者照着替换），带 doc-examples 的 `file=` 标记，命令带 `run` 标记。

| 步 | 讲什么 | 例子 | 概念 | 等哪个 issue |
| --- | --- | --- | --- | --- |
| 1 起项目 | `npx @arcships/rutis-host new greeter --lang node`、`npm install`、`npm test` 通过；生成的文件只列 4 个（插件、测试、`rutis.dev.json`、`package.json`） | 命令 | 插件 | — |
| 2 提供服务 | 生成的 `src/index.ts`：`definePlugin`、`apply`、`ctx.provide('greeter', …)`、`provides` 一行说明（`check` 不运行就知道谁提供什么）。代码块带 `matches=greeter/src/index.ts` | `greeter.hello(name)` | 插件、服务 | #692 |
| 3 使用服务 | `inject: ['translate']`、`ctx.use('translate')`；「都就绪才启动」一句 | `hello(name, lang)` 调 `translate.text(greeting, lang)` | `inject` | #692 |
| 4 配置 | Schema 里写 `default`，`apply` 收到补好默认值、校验过的配置；配置从哪里来：测试里的 `config`，运行时来自宿主的行（第 7 步） | `greeting`，默认 `"Hello"` | 配置 | 0.18 #669 |
| 5 发出和监听事件 | `ctx.emit('greeter/greeted', name)`；`ctx.on(name, listener)` 一个例子；「其余四种方式、过滤、作用域见插件 API」一行 | 每次问候发出一次 | 事件 | — |
| 6 测试 | `load(plugin, { config, services })`、`t.service`、`t.on('greeter/greeted')`、`t.unload`；一句：给某个宿主写插件、宿主给了严格的假服务时，把它们传给 `services` | 假的 `translate` | 测试工具 | #670（`t.on`）、#684（那一句） |
| 7 本地运行并试一下 | `rutis-host dev`；`rutis.dev.json` 的两行：和插件同 id 的那一行就是插件自己（`config` 就是它收到的配置），另一行是一起运行的假 `translate`（本语言写的）；`dev` 打印的服务列表；`rutis-host call greeter.hello '"Ada"' '"sv"'`；改一次代码看到 `reloaded` | `dev/fake-translate.ts` | `dev` 与行 | #670 |
| 8 发布 | 起自己的包名、打 tag 的工作流、依赖范围一行（链接插件 API「版本」）、「宿主怎样装你的插件」一行（链接 rutis-host.md） | 命令 | — | #691（链接写 `add`） |

路径之后，每份指南只有：「调试」（#679 已加）、该语言特有的事（Swift 的「系统权限」、Bun 的「运行时的约定」），以及一行「进阶：插件 API」。

### 3.2 第 7 步之前遇到的名字（TypeScript）

| 名字 | 归入的概念 |
| --- | --- |
| `definePlugin`、`apply` | 插件 |
| `ctx.provide`、`provides` | 服务 |
| `inject`、`ctx.use` | `inject` |
| `config`（JSON Schema）、`default` | 配置 |
| `ctx.emit`、`ctx.on` | 事件 |
| `load`、`t.service`、`t.on`、`t.unload` | 测试工具 |
| `rutis-host dev`、`rutis.dev.json` 的行、`rutis-host call` | `dev` 与行 |

不出现：清理函数与 `ctx.effect`（模板不需要；在插件 API「生命周期」）、`'sync'`、`ctx.signal`、插件定义、`host()`、`serial` / `waterfall` / `parallel` / `bail` / `filter`、作用域、严格模式的细节、插件 API 版本、Cordis。每种语言的实施 PR 附一张同样的表，审阅时核对。

### 3.3 各语言的差别

各语言走同样的 8 步、同样的例子，只在下面这些地方不同：

| 语言 | 不同的地方 |
| --- | --- |
| Bun | 命令换成 `bunx --bun`、`bun install`、`bun test`；其余与 TypeScript 相同（指南仍写全 8 步，不让读者在两份之间跳） |
| Python | 第 2 步一句：`async def` 是异步方法；第 8 步：入口点名就是宿主的行名 |
| Go | 第 1 步一句：一个二进制就是宿主里的一个运行时进程；第 2 步：方法名小写对外、方法要能并发调用；第 3 步：函数字段的结构体、**把 ctx 传下去**；第 4 步：没写的字段是零值，默认值在 `Apply` 里给（0.18 的 Go 规则）；第 7 步：假的 `translate` 用 Go 写，形式按 #670 第 3 条 |
| Rust | 第 1 步：`cargo new --lib greeter` 加依赖（没有模板）；第 2 步：`#[rutis::service]`；第 3 步：`type Deps = Arc<…>`；第 4 步：`fn new(config: &Config)`，默认值用 serde 的 `default`；第 5 步：事件类型与在服务方法里发出（保存插件的 `Ctx`）；第 6 步：`#[tokio::test]` 里建根；第 7 步：`examples/try.rs` 是一个最小的 Rust 宿主，建根、装插件与假的 `translate`、用 `call_service` 调一次，`cargo run --example try`；第 8 步：`cargo publish`，宿主用 `Builtins::register` 装（dylib 链接外部插件指南）。Rust 的第 7 个概念因此是「宿主装插件」，不是「行」 |
| Swift | 第 1 步：`rutis-host new greeter --lang swift`、`swift test`，第一次要信任 SDK 的宏；第 7 步：`dev` 会先构建；路径之后保留「系统权限」 |

### 3.4 模板

`rutis-host new` 的 node、bun、python、go、swift 模板改成第 2 步的样子：只提供 `greeter.hello(name)`，带它的测试，`rutis.dev.json` 只有 `{ "id": "<名字>" }` 一行；代码里的注释指向指南的第 3、4、5 步。服务名仍是 `greeter`，与项目名无关（现状）。改模板与改指南在同一个 PR 里，`matches=` 保证二者一致。

## 四、搬什么到哪里

| 现在在哪 | 内容 | 搬到哪 |
| --- | --- | --- |
| TS §2、Python §2、Go §2、Swift §2 | `provides` 的细节（`'sync'`、不写时怎样推断、`--strict`） | 第 2 步一行；其余进插件 API「声明」 |
| TS §2、Python §2、Go §2、Bun §2、Swift「把插件交给服务」、Rust「插件定义」 | 插件本身也是一个值 | 插件 API「插件定义」（已有通用规则；各语言的写法放进这一节） |
| TS §3、Python §3、Go §2、Bun §2、Swift §2、Rust §2 | 事件的五种方式、`filter`、`prepend` / `global`、`once`、前缀监听、作用域、卸载与取消、`on_sync`；各语言的 greeter / shout 例子；Python 的跨语言例子（`sessions` 与 `audit`） | 插件 API「事件」，各语言一小节；第 5 步只留 `emit` / `on` |
| TS §3、Python §3、Go §3、Swift §3 | 严格模式、`t.methods`、Go 的 `InScope` / `Update` / 加载插件定义、Swift 的 `load(config:row:scope:)` | 插件 API「测试工具」，加各语言测试工具的接口表（含 Python 测试工具的接口，#670 第 4 条要的文档） |
| Go §2「要点」、Swift §2「要点」 | 方法签名、同步还是异步、值、错误、并发、取消、调用链 | 第 2、3 步各留一行必要的（见 §3.3）；其余进插件 API「服务」「值怎样传递」「错误」（Go、Swift 一列） |
| Swift §2 的值表 | Swift 的值 | 删去：插件 API「十种值」已有 Swift 一列 |
| Rust 开头的名字表与 `Ctx` 方法表、§2 的「三件事」与「事项」表、`provide_as` / `get` / `Option` 依赖 / `Deps` 表、「清理和后台任务」、「错误」 | Rust 的进阶写法 | 插件 API 新的「Rust」一节；插件 API 因此也作为 `rutis` crate 的 doctest 编译（与 rust-plugin.md 相同） |
| TS、Python、Go、Swift §5、§6 | 依赖写 `^` 或下限的理由；宿主怎样装（npm、uv、`go add`、目录、按需启动） | 第 8 步各一行，链接插件 API「版本」与 rutis-host.md「宿主怎样装插件」 |
| Go §7、Bun §6、Swift §8 | 在另一台机器上运行 | nodes.md「别的机器上的语言运行时」（在 #698 改写远程配置之后搬） |
| Bun §3 | 宿主的 `rutis.json` 与 `runtimes.bun` 字段 | rutis-host.md `runtimes`（已有）；指南删去 |
| TS「已有的 Cordis 插件」 | Cordis 包作为行 | cordis.md |
| rutis-host.md 开头 | 「插件作者用它在本地开发」 | 改为面向组装插件的人；`dev` 一节原样保留作参考，插件指南链接它 |

不搬的：rust-host.md 全部留在原处（它是嵌入的 API 参考）；Swift「系统权限」、Bun「运行时的约定」留在各自指南的路径之后。

docs/guide/README.md 与根 README 的指南索引按层分组：给宿主写插件（六种语言）、用 rutis-host 组装插件、做一个开放插件的宿主（新）、参考（插件 API、连接节点、Cordis、Rust 嵌入、应用托管）。

## 五、宿主持有者指南「做一个开放插件的宿主」

`docs/guide/open-host.md` / `open-host.en.md`。读者：在自己的 Rust 产品里接入 rutis，并把插件能力开放给别的开发者。全篇用一个例子：命令行记事工具 `jot`，宿主提供 `notes` 服务，开放一个扩展点 `commands.register(name, handler)`，第三方用 TypeScript 或 Python 写命令。

| 节 | 讲什么 | 例子 | 用到的名字 | 等哪个 issue |
| --- | --- | --- | --- | --- |
| 1 接入 | 依赖；建根；提供自己的服务；用 loader 装插件（Node、Python 运行时）；等就绪（`reconcile` 等每一行的 `apply` 跑完一遍，卡住的插件用 `tokio::time::timeout` 包住，再看状态）；关闭（`shutdown_with_timeout`，剩下的进程 `kill_processes()`）。一张表说清 loader 每个零件做什么：`Chain` 按顺序问谁认识这个行名、`with_shared` 让运行时的解析器也给 `RuntimeRowsPlugin` 用、`RuntimeResolver` 是一个运行时认识的行名、`RuntimeRowsPlugin` 让运行时重启后重新解析它的行、`ServiceCatalog` 是服务名表、`Layer` / `Patch` 是行的来源。路径相对于哪里、没安装的 Python 插件用 `python_path`、给一个运行时设环境变量 | `jot` 的 `main.rs` 与一个 TS 命令 `wordcount` | `Ctx::root`、`rutis::host::*`、`LoaderPlugin`、`Chain`、`Builtins`、`ServiceCatalog`、`Layer`、`Patch`、`rutis_loader::runtime::{RuntimeResolver, RuntimeRowsPlugin}`、`rutis_bridge::runtime::{LocalRuntime, Launcher}`、`rutis_bridge::transport::local::kill_processes` | 0.18（默认输出、`status()`） |
| 2 定义插件面 | `#[rutis::service]` 的 `Notes`；回调注册 `Commands::register`：处理函数按引用传进来，`&Ctx` 参数是调用方的行，登记挂在它上面，插件卸载时自动去掉；`caller_row()` 知道是哪个插件；`rutis-host types` 给插件作者生成 TypeScript 与 Python 类型，文件头与错误类型用宿主的名字 | `Commands`、`Notes`、生成的 `jot.d.ts` / `jot.py` | `rutis::service`、`rutis::host::BoxFuture`、`rutis_bridge::session::caller_row` | #684 |
| 3 管住插件 | 每个插件一个运行时进程；调用时限与取消；问候时限（调试时关掉）；限定一个插件能用哪些服务；超时或被拒时宿主和插件作者看到什么 | `jot` 给第三方命令每个一个进程、5 秒时限、只许用 `notes` 与 `commands` | #683 定的选项 | #683 |
| 4 管理插件 | 装、升级、卸载插件（不需要知道语言）；增删行、改配置（loader 的命令式修改与持久化）；看一行现在怎样 | `jot install <插件引用>`、`jot remove` | #691 的嵌入 API、`rutis_loader::edit`、`Loader::status()` | #691 |
| 5 服务好插件作者 | 交给插件作者的东西：模板（宿主自己的，带生成的类型与严格假服务）、开发循环（`--plugin <目录>` 用 `RuntimeResolver::invalidate_all` 与 `Loader::reload`）、出错时按错误码给提示（`rutis_bridge::thrown()` 读出 name、code、site） | `jot` 的 TS 模板与测试 | #684 的严格假服务、`thrown()` | #684、#670、0.18 #677 |
| 6 看得到状态 | 一处看到哪些行在跑、在等什么、运行时进程、进行中的调用与调用方；要随变化做事时监听 `rutis/row`、`rutis/runtime` | `jot status` 打印 `Loader::status()` | `Loader::status()`、`RuntimeHandle::state()`、`RowEvent` | 0.18 #678 |

- 每节的 Rust 代码既是 doc-examples 的 `file=` 块（第 1 节写出 `Cargo.toml`、`src/main.rs`，再 `cargo run` 一次，看到 `wordcount` 跑起来），也作为 doctest 编译：open-host.md 两份像 rust-host.md 一样由 `rutis-loader` 的 `lib.rs` 引入。
- 第 5 节的 TS 模板与测试同样由 doc-examples 跑：第三方照模板写 `wordcount`、`npm test`，用的是第 2 节生成的严格假服务。
- 只讲做成这件事要用的；每个名字第一次出现时链接 rust-host.md 里它的参考条目，不在两处重复参数细节。

## 六、小的代码改动

**公开路径**（D8）。#685 列的名字现在的位置：

| 名字 | 现在 | 指南写 |
| --- | --- | --- |
| `EntryStatus`、`EntryInfo`、`RowStatus` 等 `report` 下的名字 | 公开路径在 `rutis_loader::report::`；从根导入报 E0603（根上的 `pub(crate) use crate::report::*` 占住了这个名字） | **改**：删掉 crate 根上 `report`、`edit`、`resolve`、`internal`、`runtime`、`peer` 等模块的 `pub(crate) use …::*`，crate 内部改用模块路径。公开 API 不变（`public-api.txt` 不变） |
| `Loader::entries`、`waiting_for`、`settled`、`status`（0.18） | `rutis_loader::Loader` 的方法 | 原样 |
| `Snapshot`、`FiberState`、`FiberView` | `rutis::host` | 原样 |
| `LocalRuntime::python_path`、`Launcher::env`、`RuntimeState`、`RuntimeHandle` | `rutis_bridge::runtime` | 原样 |
| `caller_row`、`CallerRow`、`call_service` | `rutis_bridge::session` | 原样 |
| `Resolver` 与写解析器的类型 | `rutis_loader::Resolver`、`rutis_loader::resolve` | 原样 |
| `kill_processes` | `rutis_bridge::transport::local` | 原样 |
| `RuntimeResolver`、`RuntimeRowsPlugin` | `rutis_loader::runtime` | 原样 |

只有第一行要改代码。之后由 doctest 保证：指南里每一行 `use` 都要能编译。

**英文文档注释**（D9）。发布列车里的 crate（`rutis`、`rutis-macros`、`rutis-bridge`、`rutis-loader`、`rutis-host`、`rutis-sdk`、`rutis-dylib`、`rutis-dylib-meta`、`rutis-dylib-launcher`、`rutis-migrate`）里，`///`、`//!` 有中文的文件（不含测试文件）：

- `rutis`：`src/fiber.rs`（112 行）、`registry.rs`（32）、`host.rs`（21）、`ctx.rs`（20）、`effect.rs`（18）、`key.rs`（17）、`plugin.rs`（16）、`error.rs`（12）、`lib.rs`（7）、`internal.rs`（7）、`events.rs`（1）、`future.rs`（1）
- `rutis-bridge`：`src/identity.rs`（2）、`access_point.rs`（1）、`runtime/testing/events.rs`（1）
- `rutis-loader`：`src/runtime/connections.rs`（1）
- `rutis-host`：`src/languages/mod.rs`（1）
- `rutis-dylib`：`src/windows.rs`（1）
- `rutis-dylib-launcher`：`src/main.rs`（1）

改成英文，内容按代码现在的行为写（旧注释里的设计编号如「§四」「D24」删掉）。CI 的 lint 任务加一条检查：这些 crate 的 `src/` 里 `///` / `//!` 不含中文（普通 `//` 注释不管）。实施时按当时的 main 重新数一遍。

**文档示例检查**（D7）。`tools/doc-examples.mjs`：标记加 `matches=<路径>`（与 `file=` 一样相对于例子的根，比对而不写入，不同时打印差异并失败）；代码块的语言可以带属性（` ```rust,no_run `，按 `rust` 处理）。`tests/e2e/tests/doc_examples.rs`：执行 `matches=`；名字以 `swift` 开头的例子只在 macOS 上跑。`install.yml` 的「the guides' examples」一步在 Linux 之外也在 `darwin-arm64` 上跑（只跑 Swift 的例子）。`plugin-api.md` 两份作为 `rutis` 的 doctest 引入。

## 七、开发者看到什么

**TypeScript 插件作者，照指南走一遍**（输出里 `dev` 的服务列表与 `call` 的格式由 #670 定，下面按它的意思写）：

```text
$ npx @arcships/rutis-host new greeter --lang node && cd greeter && npm install && npm test
# tests 1  pass 1
（第 3 步若只写了 ctx.use、忘了写进 inject，测试失败，文字是现在的：）
service translate is not available to this plugin, which does not declare it in inject: add 'translate' to its inject
（写进 inject 后通过；第 6 步用 t.on 断言收到 greeter/greeted）
$ npx rutis-host dev
greeter: running
translate: running
（服务列表：greeter 由 greeter 提供，translate 由 translate 提供）
$ npx rutis-host call greeter.hello '"Ada"' '"sv"'
"Hej, Ada!"
（改 src/index.ts 保存）
greeter: reloaded (src/index.ts changed)
```

**指南与模板不一致时**（维护者看到的，doc-examples 发布后检查）：

```text
docs/guide/typescript-plugin.md:41: the block does not match greeter/src/index.ts
-  provides: { greeter: { hello: 'async' } },
+  provides: { greeter: { hello: 'sync' } },
```

**Go 插件作者，第 7 步**：`rutis.dev.json` 里假的 `translate` 是项目里的 Go 代码，`rutis-host dev` 构建并运行它，不需要 Node 或 Python（形式按 #670）。

**宿主持有者**：

```text
$ cargo build      # 0.17：use rutis_loader::EntryStatus;
error[E0603]: enum import `EntryStatus` is private
$ cargo build      # 0.19：同一行
error[E0432]: unresolved import `rutis_loader::EntryStatus`
help: consider importing this enum instead: rutis_loader::report::EntryStatus
```

（第二段的 `help` 是 rustc 自己给的；实施第 1 步核对一次。）

指南第 1 节照着做完，`cargo run -- wordcount one two three` 打印 `3 words`；`jot status` 打印 0.18 `Status` 的 `Display`（行、服务、运行时、进行中的调用，[诊断](design-diagnostics-2026-10-26.md) §5.1）。第三方命令调用没被允许的服务、调用超时时的文字由 #683 定，指南原样引用；文字遵守诊断 §4.5（不出现 rutis、binding、内部 id）。

## 八、兼容与迁移

- 不改插件 API、配置格式与行为，不进《迁移到 0.19》。
- 模板改成第 2 步的样子只影响新建的项目。
- 删掉 rutis-loader 根上的 `pub(crate)` 导入不改变公开 API；从根导入这些名字本来就不能编译。
- 指南里的名字随 #692 改到各 SDK 根上的名字；旧导入的提示属于 #692 与《迁移到 0.19》。

## 九、实施

| 步 | 内容 | 前提 | 完成条件 |
| --- | --- | --- | --- |
| 1 | §六的公开路径改动与英文文档注释，CI 检查 | 无，可以先做 | `use rutis_loader::EntryStatus` 不再报 E0603；列出的文件没有中文文档注释，CI 检查在 lint 任务里运行 |
| 2 | doc-examples 的 `matches=` 与带属性的代码块；`doc_examples.rs` 支持它们并在 macOS 上跑 Swift；plugin-api.md 作为 doctest 引入 | 无 | `node tools/doc-examples.mjs --check` 通过；有一个故意不一致的例子能被 `matches=` 查出（工具的测试） |
| 3 | plugin-api.md 收下 §四的进阶内容（各语言小节、Rust 一节、测试工具接口表） | #692（进阶名字的新位置）、0.18 合并 | §四表里标为「插件 API」的内容都在 plugin-api.md，两份一致，Rust 例子编译 |
| 4 | 六份插件指南按 §三重写，模板同步改，路径之后的内容按 §四删减；README 与 guide/README 的索引按层分组 | 第 2、3 步；#670（`call`、服务列表、`t.on`、本语言的假依赖）；#698（再搬「另一台机器」） | 每份指南 8 步，两种语言；每个代码块都有 doc-examples 标记（Rust 为 doctest），第 2 步有 `matches=`；每种语言附 §3.2 那样的名字表 |
| 5 | rutis-host.md 开头按 D5 重写 | #691、#670 | 快速上手用 `add`、看到服务列表、`call` 一次、`status` 一次，例子带标记 |
| 6 | open-host.md 两份，按 §五：第 1、6 节在 0.18 合并后写；第 2、5 节在 #684 后；第 3 节在 #683 后；第 4 节在 #691 后 | 第 1、2 步与各节的 issue | 6 节齐全，Rust 代码是 doctest 并有一条 doc-examples 跑通 `cargo run`；用到的每个名字从指南写的路径能直接用；#691 的「宿主持有者指南两份同步」一条由第 4 节满足 |
| 7 | 验收 | 0.19 发布 | #663 的 0.19 评估记录里：每种语言的插件作者在「跑起来」（第 7 步）之前遇到的概念不超过 7 个（插件、服务、`inject`、配置、事件、测试工具、`dev` 与行）；宿主持有者不读 crate 源码完成接入与开放。超出时在 0.19.x 里改指南 |

#671「完成」各条：1、2 → 第 4、3 步；3「例子由文档示例检查跑」→ 第 2、4、6 步；4「概念不超过 7 个」→ 第 7 步。并入的 #685：「指南两份，例子能编译运行」→ 第 6 步；「名字能从公开路径直接用」→ 第 1、6 步；「文档注释用英文」→ 第 1 步。

## 十、自审：是不是太重了

- 新的宿主持有者指南文件：#685 要一份按任务写的指南；塞进 658 行的 rust-host.md 会把参考与教程混在一起。保留。
- `matches=`：使用方是第 2 步与模板的一致性（0.17 评估 A1 的问题、#668 修过一次又会漂）。保留；不做更一般的「片段比对」。
- Swift 在 macOS 上跑：否则 #671「例子都由检查跑」对 Swift 不成立。只跑 Swift 的例子，不在 macOS 上重跑其他语言。
- plugin-api.md 作为 doctest：Rust 的进阶例子搬过去后要继续编译。保留。
- 中文注释的 CI 检查：一条 grep 级别的检查，防止新注释再写成中文。保留。
- 删掉的：`rutis_bridge::runtime` 再导出 `Processes`（指南用 `kill_processes()`，不需要这个类型名）；给 Rust 做模板或 `dev`；宿主持有者指南覆盖 Swift / Kotlin 应用与以 rutis-host 为运行时的产品；为每一节另建示例 crate（doc-examples 写出文件再 `cargo run` 已经够）。
