# 宿主自己写插件：宿主里的 agent 怎样操作和管理插件

[English](design-host-writes-plugins-2026-10-27.en.md) · [中文](design-host-writes-plugins-2026-10-27.md)

状态：草案，未实现。日期：2026-10-27。基准：`main` `103c0d22`。排期：0.19（只做设计）；实现在 0.19 之后。
Issue：[#185](https://github.com/arcships/rutis/issues/185)（含并入的 [#693](https://github.com/arcships/rutis/issues/693)）。取代 [#505](https://github.com/arcships/rutis/pull/505) 的"给 agent 框架加两个工具"草案。
与 0.18 的关系：状态与出错全部用 [0.18 的诊断](design-diagnostics-2026-10-26.md)（`Loader::status()`、错误图上的 `site`、错误码）与 [0.18 的配置 Schema](design-config-schema-2026-10-27.md)，不加结构。
依赖的 0.19 设计：[#683](https://github.com/arcships/rutis/issues/683)（[PR #733](https://github.com/arcships/rutis/pull/733) 的 `docs/design-host-plugin-limits-2026-10-27.md`）、[#691](https://github.com/arcships/rutis/issues/691)（[PR #734](https://github.com/arcships/rutis/pull/734) 的 `docs/design-plugin-install-2026-10-27.md`）、[#684](https://github.com/arcships/rutis/issues/684)（契约描述，[PR #730](https://github.com/arcships/rutis/pull/730)）；S1 用 [#670](https://github.com/arcships/rutis/issues/670) 的 `rutis-host call` 驱动（[PR #731](https://github.com/arcships/rutis/pull/731)）。需要它们提供的列在 §十一。

范围外：管理服务里的通用调用（插件的能力按 Cordis 的方式注册给 agent，§4.4；人要试调用用 #670 的 `rutis-host call`）；给某一种 agent 框架开工具（rutis-agent 的 `ToolRegistry` 要用时，是这个服务上一层几行的适配）；启用 / 停用一行（dsh 的 `set_plugin`）、`upgrade`、版本豁免（§十二）；API 文档生成；agent 专用的 skill（§4.5）；每次调用的用户批准（属于 agent 框架，§5.3）；单独的试用运行时、由宿主跑插件自己的测试（§6.4）；从 npm、PyPI 装（只装本地目录）；agent 写 Go、Swift 插件（#683 的命名本机运行时只有 Node、Python、Bun）；agent 写的插件之间互相使用服务；内存、CPU 上限与沙箱；改了子模块不重载的已知缺口（[rutis-host 指南](guide/rutis-host.md)已写明）；Windows 上的 S1（#670 的 `call` 不在 Windows 上）。

## 一、要做成什么，现在卡在哪

[设计哲学](design-philosophy.md) §1：宿主需要接触什么，就自己写一个插件接上去，常常由 agent 来写；试用、修改、替换，连接从此成为宿主的一部分。

场景：一个 `rutis-host run` 的宿主里跑着一个 agent，它本身就是宿主里的一行插件（这里用 Python，任何语言都行），提供一个工具服务 `tools`。用户问"奥斯陆今天天气怎样"，它没有能回答的工具。它要：

1. 查宿主里有什么（服务、方法、配置 Schema、插件目录），写一个 Python 插件 `weather`，让它把一个 `weather` 工具注册进 agent 的 `tools`；
2. 让宿主检查并装上；
3. 调用这个工具；
4. 改坏了：知道错在哪个文件哪一行，旧版本继续服务；
5. 修好、换上新版本；
6. 不要了：删掉，它的进程退出；
7. 随时看现在有哪些行、服务、错误。

现在卡在：宿主有这些操作，但只给命令行和终端前的人——`rutis-host check`、`status`（0.18）、`add` / `remove`（#691）、`dev` 的重载。一行插件拿不到其中任何一个。[session 持久化与自我控制工具](design-session-persist-and-self-tools-2026-08-23.md) §三把"动态加载新代码"留作后续课题；本稿就是它。

## 二、Cordis 的做法

dsh（Cordis 4 上的 agent 产品）已经做了这件事。对照它的各个包：

| dsh | 做法 | 本设计 | 不同的理由 |
| --- | --- | --- | --- |
| `dsh-agent-preset`（创造模式）、`dsh-tool-cordis` 的 skills 与模板 | agent 按随包的 skill 和模板写出一个组合包，从本地路径装进 profile；没有模型工具创建临时的动态定义（`dsh-cordis-host-runner`） | agent 写本地目录，`add` 装进宿主配置；不做临时定义。写法靠 `rutis-host new` 的模板和指南（§4.5） | 相同 |
| `dsh-tool-cordis` | 两个只读查询：服务与事件的 API 声明、当前工具的 schema、运行中条目的配置 JSON Schema、包目录 `packageDir`；查询不调用服务方法 | `describe(name)`：方法与同步异步、#684 的契约类型、0.18 的配置 Schema、项目目录（§4.1） | 不生成 API 文档：rutis 没有通用的类型层（设计哲学 §七 原则 6），能给的就是这几样 |
| `tools` 服务 | 插件 `inject` 宿主的 `tools` 并注册工具；没有通用的调用工具 | agent 那一行 `provide` 一个工具服务，插件 `inject` 它、注册回调（按引用传）（§4.4） | 相同；rutis 不加东西 |
| `dsh-plugin-manager`（服务 `pluginManager`） | Web 页面、agent 工具 `plugin_manager`、命令行 `dsh plugin` 共用一个服务；`list`、`set_plugin`、`set_bundle`、`install_bundle`、`remove_bundle`、版本豁免；管整个 profile；写进 profile 的补丁文件，HMR 立即生效 | 管理服务 `plugins`：`status`、`describe`、`add`、`reload`、`remove`；与命令行同一套实现；写进 `rutis.json`，立即生效（§四、§八） | 默认只管 agent 自己加的行（§5.2）：agent 改错了碰不到宿主依赖的手写插件。没有 `set_plugin`、豁免：S1 用不到 |
| `plugin_manager` 工具 | 只在创造模式的预设里启用；每次调用要 `danger-full-access` 或这一次的用户批准；服务本身不认身份 | 服务只交给配置里写明的一行（`admin.for`）；批准由 agent 框架做，结果里写明会持久化、影响整个宿主（§5.3） | rutis 没有预设和会话权限；"交给哪一行"就是它的"在哪个预设里启用" |
| `dsh-plugin-manager` 的 `inspect` 与版本检查 | 装之前读出引用指向什么、给出 `problem` 码；插件声明的 dsh peer 不兼容时在 pnpm 运行前拒绝 | `add` 第 1 步：#691 认出目录、按宿主版本装 SDK，不兼容就在装上前失败（§6.1） | 相同 |
| `dsh-plugin-manager` 的恢复 | 失败、取消时恢复 `package.json` 与锁文件；依赖的构建脚本单独批准 | 每一步失败都撤掉这次已做的（§6.1）；构建脚本照 #691 的规则 | 相同 |
| `dsh-config-editor` | 先校验完整的候选值再写盘；应用失败就恢复原文档、重新加载原补丁 | `Loader::check` 在候选配置上检查后才改；`Loader::reload` 全有或全无（§6.2） | 相同 |
| `dsh-hmr` | 模块替换、配置变更共用一个串行队列；包安装在队列之外 | 管理服务的操作与 rutis-host 的配置重读共用一个队列，一次一件（§6.5） | 安装放在队列里：一次操作的几步要连着做，宿主不会同时装很多插件 |
| `dsh-host-plugin-inventory` | `pluginInventory` 每次直接读 `loader.entries()`，只读快照，不缓存 | `status()` 每次读 0.18 的 `Loader::status()` | 相同 |
| （单进程） | 插件都在宿主进程里 | 每个 agent 加的插件一个命名运行时、一个进程，带 #683 的限制（§5.4） | rutis 跨语言、多进程才有这一层；它让一个坏插件不连累别人，`remove` 后进程退出 |

## 三、决策

| # | 决策 | 章节 |
| --- | --- | --- |
| H1 | 宿主按名字提供管理服务（默认名 `plugins`），5 个方法：`status`、`describe`、`add`、`reload`、`remove`。agent 用各 SDK 都有的 `inject` 使用它 | 四 |
| H2 | 与命令行同一套实现：都调 `Loader::status()`、`Loader::check`、`Loader::reload`、#691 的 `Installer`；串起来的一层是 rutis-loader 的 `admin` | 四 |
| H3 | 插件的能力按 Cordis 的方式交给 agent：agent 那一行提供工具服务，插件 `inject` 它并注册回调；管理服务没有通用调用 | 4.4 |
| H4 | 默认不提供；宿主在配置里写明交给哪一行（`admin.for`），别的行以 `not-allowed` 拒绝；每次调用要不要用户批准由 agent 框架决定 | 五 |
| H5 | agent 默认只能重载、移除它自己加的行；`manage: "all"` 放宽 | 五 |
| H6 | 每个 agent 加的插件一个命名本机运行时 `agent-<名>`，`limits`：`call` 30 秒、`stop: "process"`、`services` 等于 `admin.allow` | 五 |
| H7 | 生效前按固定顺序检查：安装（含版本兼容）→ 运行时 → `Loader::check` → 装上或全有或全无地换上；失败停在那一步、撤掉、旧版本继续服务；所有操作排一个队 | 六 |
| H8 | 结果是普通数据 `{ok, text, error?, persisted}`；`error` 是 0.18 错误图的根（含 `site`） | 七 |
| H9 | 写进宿主配置，重启后仍在 | 八 |
| H10 | S1 的 agent 是一个脚本化的 Python 插件，测试用 `rutis-host call agent.step <n>` 驱动 | 十 |

## 四、管理服务

### 4.1 方法

| 方法 | 参数 | 做什么 | 结果 |
| --- | --- | --- | --- |
| `status()` | 无 | 0.18 的 `Loader::status()`：每行的状态与原因、服务与提供者、运行时、进行中的调用 | 快照（普通数据）与 `text`（`rutis-host status` 打印的那段） |
| `describe(name)` | 服务名或行名 | 只读：服务的方法与 `sync` / `async`；宿主给了 #684 的契约描述时，参数与结果的类型；行的配置 Schema（0.18）与项目目录 | 普通数据 |
| `add(path)` | 本地目录（相对宿主配置所在目录） | §6.1 的检查与装上 | `{ok, row, text, error?, persisted}` |
| `reload(row)` | 行名 | §6.2 的检查与换上 | `{ok, text, error?, persisted}` |
| `remove(row)` | 行名 | §6.3 | `{ok, text, persisted}` |

- 插件代码出的错放在结果里，不抛出：agent 要读它、改代码，这是正常流程。只有越过权限才抛出 `not-allowed`（§九）。
- 7 步里每一步对应一个方法，没有别的。

### 4.2 和已有的操作是同一套

| 操作 | 命令行 | 管理服务 | 实现（已有或 0.19 要做） |
| --- | --- | --- | --- |
| 看状态 | `rutis-host status` | `status()` | `Loader::status()`（0.18） |
| 查一个服务或一行 | `rutis-host check` 打印的 Schema、`rutis-host types --contract`（#684） | `describe()` | `HostDispatch::methods()`、`Loader::schema_of`、#684 的契约描述、#691 的安装记录里的目录 |
| 检查 | `rutis-host check` | 在 `add`、`reload` 里 | `Loader::check(&layers)`（`crates/rutis-loader/src/loader/check.rs`），含 0.18 的配置校验 |
| 装上 | `rutis-host add <路径>`（#691） | `add()` | `Installer::add`；运行中加运行时用 #683 的 `RuntimeSet`；写行用 `Loader::create` |
| 重载 | `dev` 的文件变化、rutis-dev 通道的 `swap` | `reload()` | `Loader::reload`（`crates/rutis-loader/src/loader/api.rs`） |
| 移除 | `rutis-host remove`（#691） | `remove()` | `Loader::remove`、`RuntimeSet::remove`、`Installer::remove` |

新加的只有按 §六 的顺序把这几步串起来的一层：`rutis_loader::admin::Admin`（rutis-loader 的 `admin` feature，带上 #691 的 `install`），有这 5 个方法的 Rust 版本和一个按名字提供它的 `HostDispatch`。rutis-host 与嵌入宿主用同一个。rutis-host 的库不是公开 API，不放那里（与 #691 的 `Installer` 同理）。#670 的 socket 不加这些操作：命令行的 `add` / `remove` 改配置文件，没有"从命令行往运行中的宿主装"的使用方。

### 4.3 agent 怎样用

agent 的行写 `inject: ["plugins"]`，照它的语言用（Python 示意）：

```python
plugins = ctx.use("plugins")
info = await plugins.describe("notes")            # 方法、类型、Schema、目录
result = await plugins.add("agent-plugins/weather")
if not result["ok"]:
    fix_from(result["error"])                     # site、文件与行号
```

agent 用它自己语言的文件操作写插件文件；插件进程本来就以宿主用户的权限运行（没有沙箱），rutis 不另给写文件的接口。

### 4.4 插件的能力怎样到 agent 手里

和 dsh 一样：agent 那一行 `provide` 一个工具服务（名字由 agent 定，例如 `tools`），它写的插件 `inject` 它、在 `apply` 里注册自己的工具，处理函数按引用传过去（[插件 API](guide/plugin-api.md)"值怎样传递"：函数按引用，另一端调用的是原来那个）；插件卸载时用 `apply` 返回的清理函数注销。这些是各 SDK 都有的 `provide` / `inject` / 清理，rutis 不加东西，管理服务也不需要通用调用。工具服务的样子是 agent 作者的事。

`weather` 插件（Python 示意）：

```python
async def apply(ctx, config):
    async def today(city: str) -> str:
        return f"sunny in {city} (v1)"
    return await ctx.use("tools").register("weather", "Today's weather for a city", today)  # 返回注销函数
```

调用这个处理函数出错时，agent 的代码收到的是普通的错误（名字、消息、错误码、`data`），与任何调用方相同（0.18 §4.4），里面没有位置。S1 里 agent 要修的错误（语法错误、`apply` 出错）都从 `add` / `reload` 的结果读到，带位置。"把调用时收到的错误交给宿主读出位置"要多一个方法，现在没有使用方，写进 §十二。

### 4.5 agent 写插件靠什么

对应 dsh 的 skills 与模板：rutis 已有 `rutis-host new` 的模板和插件作者指南（#671 的短路径）。宿主把它们交给 agent（放进提示、让 agent 去读），是宿主产品的事；rutis 不提供 agent 专用的 skill。

## 五、权限与限制

### 5.1 谁能用管理服务

- **默认不提供**：配置里没有 `admin` 时，宿主不提供这个服务，`inject` 它的行一直等（0.18 的"没有提供者"）。
- **交给一行**：`admin.for` 写一行的 id。服务用已有的 `caller_row()`（`crates/rutis-bridge/src/session/services.rs`）认出调用的行，不是这一行时以 `not-allowed` 拒绝。这相当于 dsh 的"只在创造模式的预设里启用"。不在任何行里的调用（宿主自己的 Rust 代码）照常可用。
- **为什么不用 #683 的 `services`**：它给一个运行时列"能用哪些"，不写时不限；要让别的行都用不到 `plugins`，就得给每个运行时都写名单，做不成默认。

### 5.2 agent 能管哪些行

| `admin.manage` | 能 `reload` / `remove` 的行 | 用在 |
| --- | --- | --- |
| `"own"`（默认） | 它用 `add` 加的行：都在 `agent-plugins` 组里（组由 `Admin` 建，不带设置，只用来标出这些行） | agent 改错了碰不到宿主持有者手写、宿主依赖的插件。比 dsh（管整个 profile）窄 |
| `"all"` | 所有插件行（不含它自己那一行） | 整个宿主都由 agent 维护时，宿主持有者明确放宽 |

`status`、`describe` 不受 `manage` 影响：agent 替宿主干活，看得到整个宿主。

### 5.3 每次调用的批准

dsh 在工具层要求每次调用 `danger-full-access` 或这一次的用户批准，服务本身不认身份。rutis 同样不做：批准属于宿主产品或 agent 框架。为此，`add`、`reload`、`remove` 的结果带 `persisted: true`，管理服务的说明（指南）写明这些操作写进宿主配置、影响整个宿主、装上的代码以宿主用户的权限运行，agent 那一层据此决定要不要先请用户批准。

### 5.4 agent 加的插件的运行时

按 #683（限制设在命名运行时上），`add` 给每个插件加一个只属于它的命名本机运行时 `agent-<插件名>`：

| `limits` | 取值 | 理由 |
| --- | --- | --- |
| （一个插件一个运行时） | 是 | 一个插件死循环或崩溃不连累别人；`remove` 连运行时一起移除，进程退出。代价：每个插件多一个进程 |
| `call` | 30 秒 | agent 调它的工具、别的插件用它的服务，超时都以 `timeout` 失败，宿主不被拖住 |
| `stop` | `"process"` | 卡在同步代码里的插件取消不了，只能结束进程；进程只属于它。之后 `reload` 重新启动这个运行时 |
| `services` | 等于 `admin.allow`，默认空 | 插件代码之后会在没人看着时运行；只给宿主列出的服务。agent 的工具服务要列进去（S1：`["tools"]`）。每个运行时一行，插件之间互相用不到 |
| `greeting` | 不设 | agent 不在调试器里启动插件 |

宿主可以用 `admin.limits` 换这些值（`services` 仍以 `allow` 为准）。

## 六、内部检查：改的东西生效之前

### 6.1 `add(path)`

| 步 | 谁做、用什么 | 失败时 |
| --- | --- | --- |
| 1 安装 | `Installer::add`（#691）：按项目文件认语言、准备环境，本地目录用链接或 `-e` 装（之后改代码不用再装）。**版本兼容**在这里：#691 把运行时的 SDK 钉在宿主的版本上一起装，插件声明的 SDK 下限不满足时包管理器解析失败，什么都不装（对应 dsh 在 pnpm 运行前按 peer 拒绝）；Go、Swift 二进制的 `PLUGIN_API` 由已有的清单检查（`crates/rutis-loader/src/runtime/binary.rs`），agent 不写这两种 | 返回 #691 的错误；什么都没改 |
| 2 运行时 | 按 #683 的顺序加 `agent-weather`：`root.plugin(LocalRuntime + RuntimeRowsPlugin)`、`RuntimeSet::add` | 撤掉第 1 步 |
| 3 静态检查 | `Loader::check`（与 `rutis-host check` 同一个），在加了新行 `agent-weather:weather` 的候选配置上：在新运行时里导入并描述它（语法、导入错误；运行时握手拒绝不兼容的 SDK）、试运行（配置 Schema）、依赖都有人提供、没有依赖环、没有同名提供、必需的服务没被 `services` 挡住 | 撤掉第 2、1 步；返回这一行的问题（与 `check` 打印的相同） |
| 4 装上 | `Loader::create` 在 `agent-plugins` 组下写这一行，等它启动或失败（`apply` 出错、启动超时） | 删行、撤掉第 2、1 步；返回 0.18 的错误 |
| 5 保存 | §八 | — |

新插件没有旧版本，失败就整个撤掉，不留失败的行（对应 dsh 失败时恢复 `package.json` 与锁文件）。

### 6.2 `reload(row)`

| 步 | 谁做、用什么 | 失败时 |
| --- | --- | --- |
| 1 运行时 | 进程被 `stop: "process"` 结束过时，先重新启动 `agent-<名>`（与 `dev` 的"进程结束了的本机运行时由下一次改动重新启动"相同） | 返回原因；旧版本本来就不在运行 |
| 2 静态检查 | `Loader::check`，检查同 §6.1 第 3 步，用这一行的新代码 | 返回问题；旧版本继续服务，什么都没换 |
| 3 换上 | `Loader::reload`：全有或全无，新模块解析不了、试运行失败或让别的行新失败时恢复旧模块（对应 dsh 应用失败就恢复原文档、重新加载原补丁） | 返回 `cannot reload` 那一句与 0.18 的错误；旧版本继续服务 |

- 第 2 步多查 `Loader::reload` 不查的两件事：新代码新用到的服务有没有人提供、会不会成环。不查的话，新版本换上后会一直等一个没有的服务，旧版本已经没了。
- 已知边界：新代码在 `apply` 里失败时，`Loader::reload` 已卸掉旧版本，再把它装回来，中间短暂不可用。语法与导入错误在第 2 步就拦下。
- 只重载这一行，不重启运行时进程，与 `dev` 相同。

### 6.3 `remove(row)`

按 #683 的顺序：`Loader::remove` 删行 → 卸载 `LocalRuntime` 与 `RuntimeRowsPlugin`（进程退出，输出 `runtime agent-weather: stopped`，其他运行时的 pid 不变）→ `RuntimeSet::remove` → `Installer::remove` → 保存。agent 的源码目录不删。

### 6.4 删掉的步骤

- **单独的试用运行时**（先在另一个运行时里导入并 `apply`）：`apply` 会开端口、打开文件、连外部服务，新旧两份同时 `apply` 会冲突；导入错误第 2 步已在插件自己的运行时里拦下。dsh 也没有这一步。
- **宿主跑插件自己的测试**（含 #684 的契约检查）：agent 是一个插件进程，能自己跑 `pytest`；#684 的契约描述宿主的 Rust 服务，agent 可以从 `describe` 读到类型。dsh 也不替插件跑测试。

### 6.5 一次一件

`Admin` 的每个操作从第 1 步到保存拿着同一把锁；rutis-host 的配置重读（`--watch-config`、SIGUSR1）也拿这把锁。所以两次操作、操作与重读不会交错（对应 dsh-hmr 的串行队列）。loader 自己的编辑本来就串行（`Loader` 的操作锁），这把锁管的是"几步连着做"。

## 七、agent 读到的状态与错误

- 0.18 §二 说 `site` 与代码位置给看输出的人与宿主，不自动交给调用方的代码。管理服务是宿主持有者在配置里明确交给一行的宿主读取接口；普通调用里插件代码收到的错误仍然没有 `site`，不变。
- `error` 是 0.18 错误图的根节点（`name`、`message`、`code`、`data`、`site`、`stack`），由 `rutis_bridge::thrown()` 取出；`text` 是 0.18 打印的那段，两边的位置都在里面。agent 的代码按 `code` 与 `site` 判断，模型读 `text`。

## 八、写进配置

| 宿主 | 写在哪 | 重启后 |
| --- | --- | --- |
| rutis-host | `rutis.json`：`agent-plugins` 组里的行、`runtimes.local` 里的 `agent-<名>`（带 `limits`）、#691 的 `installed` 记录。写的就是重读配置会得到的那些条目，下一次重读不当作变化 | 照配置启动，与手写的一样 |
| 嵌入宿主 | `Admin` 把 `{行, 运行时条目, 安装记录}` 交给宿主给的保存回调，宿主自己存（与 #691"记录归宿主"相同） | 宿主把记录交回 `Admin`，它按 §6.1 第 2 步加回运行时 |

宿主持有者打开 `rutis.json` 就能看到 agent 加了什么，也能自己删掉。

## 九、开发者看到什么

宿主持有者的配置：

```json
{
  "runtimes": { "py": { "project": "." } },
  "admin": { "for": "agent", "allow": ["tools"] },
  "rows": [
    { "id": "agent", "name": "py:my_agent", "inject": ["plugins"] },
    { "id": "notes", "name": "py:notes" }
  ]
}
```

`admin` 的字段：`for`（必填）、`service`（默认 `plugins`）、`manage`（`"own"` / `"all"`）、`allow`（默认空）、`limits`（默认 §5.4）、`contract`（#684 契约描述文件，可选）。Rust 嵌入：`Admin::new(&root, loader, installer, runtime_set).serve_to("agent").allow(["tools"]).persist(hook)`，再 `root.plugin(admin.service("plugins"))`。

agent 收到的（`text` 部分；`ok`、`error`、`persisted` 同时给出）：

| 调用 | 结果 |
| --- | --- |
| `describe("notes")` | `{service: "notes", provider: "notes", methods: {put: "async", get: "async"}, row: {config_schema: {...}, dir: "plugins/notes"}}`；有契约描述时多 `types` |
| `add("agent-plugins/weather")` | `ok: true`，`weather: running` |
| 同上，目录不是插件项目 | `ok: false`，`./agent-plugins/weather has no package.json, pyproject.toml or go.mod`（#691 的文字） |
| 同上，SDK 版本不满足 | `ok: false`，#691 转述的包管理器解析失败 |
| 同上，语法错误（第 3 步） | `ok: false`，`weather: SyntaxError: invalid syntax (src/weather/__init__.py, line 3)`；`status()` 里没有 `weather` |
| 同上，`apply` 出错（第 4 步） | `ok: false`，`weather: failed in apply: KeyError: 'units'` + `src/weather/__init__.py:12 in apply` |
| 同上，必需的 `llm` 不在 `allow` 里 | `ok: false`，`weather: uses llm, which the host does not let runtime agent-weather use (allowed: tools)`（#683 的文字） |
| 同上，依赖没人提供 | `ok: false`，`weather: waits for geo, which no row provides` |
| `reload("weather")`，语法错误 | `ok: false`，`weather: cannot reload: SyntaxError: invalid syntax (src/weather/__init__.py, line 3); the previous version keeps running` |
| 同上，修好 | `ok: true`，`weather: reloaded` |
| `remove("weather")` | `ok: true`，`weather: removed; runtime agent-weather: stopped` |
| `remove("notes")`（`manage: "own"`） | 抛出 `not-allowed`：`notes was not added by agent; the host lets agent manage only the rows it added` |
| `reload("wether")` | `ok: false`，`no row wether (rows agent added: weather)` |
| 别的行调用 `plugins.status()` | 抛出 `not-allowed`：`plugins is given to agent only` |
| agent 调 `weather` 工具，30 秒没返回 | agent 的代码收到 `timeout`：`a function of weather did not finish within 30s (the host's limit for runtime agent-weather); the call was cancelled; runtime agent-weather was ended`（#683 的文字） |

宿主持有者在终端看到的：rutis-host 照常打印行的变化（`weather: running`、`runtime agent-weather: stopped`），与 agent 收到的 `text` 相同。所有文字不出现 0.18 §4.5 的禁用词。agent 写的插件是普通的 Python 插件，不写任何 agent 或 rutis 特有的东西。

## 十、场景 S1（按本设计更新）

取代 #185 正文里的场景与断言（issue 本身不改）。

用一个脚本化的 Python 插件代替 agent：它提供 `tools`（一个记下注册的工具、按名字调用它们的小服务）和 `agent`，`agent.step(n)` 照固定脚本做第 n 步、返回结果。这比接一个模型简单，走的是同一条路；agent 的推理不是本场景要测的。测试启动 `rutis-host run --allow-call`（#670），用 `rutis-host call agent.step <n>` 驱动每一步。配置照 §九，另有一个也 `inject` 了 `plugins` 的 `intruder` 行。

| 步 | `agent.step(n)` 做的 | 断言 |
| --- | --- | --- |
| 1 | `describe("notes")`；用 Python 写出 `agent-plugins/weather/pyproject.toml` 与 `src/weather/__init__.py`（注册工具 `weather`，返回 `"sunny (v1)"`） | `describe` 有 `notes` 的方法与目录；文件存在；工具列表里没有 `weather` |
| 2 | `add("agent-plugins/weather")` | `ok`；`rutis.json` 多了 `agent-plugins` 组里的行、`runtimes.local` 的 `agent-weather`、`installed` 记录；`status` 里 `agent-weather` 只有 `weather` 一行，记下 pid；工具列表里有 `weather` |
| 3 | 调 `weather` 工具，参数 `"Oslo"` | `"sunny in Oslo (v1)"` |
| 4 | 写进语法错误，`reload("weather")`，再调工具 | `ok` 为假，`text` 以 `weather: cannot reload: SyntaxError` 开头、含 `src/weather/__init__.py`；工具仍返回 v1 |
| 5 | 改成返回 `"rainy …(v2)"`，`reload`，再调工具 | `ok`；工具列表里仍是一个 `weather`；返回 v2 |
| 6 | `remove("weather")`，`status()` | `ok`；工具列表里没有 `weather`；没有 `agent-weather`；记下的进程 5 秒内退出；其他运行时的 pid 不变；`rutis.json` 回到第 1 步时的样子 |
| 7 | `remove("notes")` | 抛出 `not-allowed`；`notes` 照常运行 |

另外：

- `rutis-host call intruder.try_status`（它调 `plugins.status()`）得到 `not-allowed`。
- 服务不中断：第 4 步期间，测试每 100 ms 用 `rutis-host call agent.use_tool '"weather"' '"Oslo"'` 调一次，没有一次失败；第 5 步 `reload` 返回之后的调用全部是 v2。
- 结果里不出现 0.18 §4.5 的禁用词；宿主停下后 `tests/e2e` 的残余检查通过。
- 落点：`tests/e2e/tests/host_writes_plugin.rs`，脚本化的 agent 插件放在场景目录里。E2E 框架 #175 已完成（已关闭）。Linux、macOS，进 nightly。

## 十一、实施

实现在 0.19 之后：

| 步 | 内容 | 能开始的条件 | 完成 |
| --- | --- | --- | --- |
| 1 | `Admin` 的 `status`、`describe`，按名字提供、`admin.for` 的检查 | 0.18 #677、#678；#684 的契约描述格式 | 单元测试：快照、`describe` 的四项、别的行被拒 |
| 2 | `add`、`reload`、`remove` 与 §六 的顺序、队列、保存回调 | #691 的 `Installer`；#683 的 `RuntimeSet`、`Limits` | §九 每一行有逐字断言的测试；每一步失败后没有多出的运行时、安装、行；`manage` 两种取值；并发的两次 `add` 依次完成 |
| 3 | rutis-host 读 `admin`、写 `rutis.json`、配置重读拿同一把锁、重启后照常 | 第 2 步；#683 的 `runtimes.local` | 写入后重读不算变化；重启后 agent 加的插件照常运行 |
| 4 | S1（§十）；rutis-host.md 与 rust-host.md 各加一节"让宿主里的 agent 管插件"（中英），写明 §5.3 的批准建议 | 1–3；#670 的 `call` | Linux、macOS 通过 |

对应 #185 的「完成」：设计稿两份（本稿）；S1 按 §十 更新。

需要 #683 提供的：

1. rutis-loader 的 `RuntimeSet`：运行中增删命名本机运行时，不重启其他运行时，增删顺序有文档（已在 PR #733）；
2. `LocalRuntime::limits` 的 `call`、`stop`、`services`；`timeout`、`not-allowed` 带 0.18 的 `site`；
3. `stop: "process"` 结束进程后，Rust 嵌入能把这个运行时重新启动；
4. `Loader::check` 报告"必需的服务被 `services` 挡住"（PR #733 §4.4 已写）；
5. rutis-host 用 `RuntimeSet` 建 loader，运行中写进 `rutis.json` 的 `runtimes.local` 条目在重读时当作已有的运行时，不重启它。

需要 #691 提供的：`Installer::add` 接受本地目录、返回语言与行名；本地目录用链接或 `-e` 安装；安装时把 SDK 钉在宿主版本，插件的 SDK 下限不满足时在装上之前失败；记录可序列化，与 rutis-host 的 `installed` 字段同一个形状，记录里有项目目录；`Installer::remove` 按记录删除；API 不读写配置。

需要 #684 提供的：契约描述可以按服务名读出一个服务的方法、参数与结果的类型（`describe` 用）。

需要 #670 提供的：`run --allow-call` 与 `call`，只给 S1 驱动用；管理服务不再依赖它。

## 十二、自审：是不是太重了

| 项 | 使用方 | 结论 |
| --- | --- | --- |
| 5 个方法 | S1 的 7 步 | 保留 |
| `describe` | 第 1 步：agent 要知道宿主里有什么、去哪读源码（dsh-tool-cordis） | 保留，只读 |
| 通用调用 `call` | 插件的能力按 Cordis 的方式注册给 agent；人用 `rutis-host call` | 删 |
| 把错误交给宿主读出位置的方法 | agent 调工具出错时找位置；S1 的错误都从 `add` / `reload` 的结果里读到 | 删，等有使用方 |
| 启用 / 停用一行（dsh 的 `set_plugin`） | S1 没有；`remove` 已能让它不运行 | 删 |
| `upgrade`、版本豁免 | 本地目录改了代码 `reload` 就生效；agent 不装别人的包 | 删 |
| 单独的 `check` 方法 | 检查都在 `add`、`reload` 里 | 删 |
| 试用运行时、宿主跑测试 | 见 §6.4；dsh 也没有 | 删 |
| `reload` 前的 `Loader::check` | 防新版本等一个没有的服务、成环 | 保留 |
| 一把锁 | 两次操作、操作与重读不交错（dsh-hmr） | 保留 |
| `manage: "all"` | 整个宿主由 agent 维护 | 保留，默认关 |
| 每个插件一个运行时 | 隔离、`remove` 后进程退出 | 保留 |
| rutis-agent 的工具适配、agent 专用 skill、每次调用的批准 | 属于 agent 框架与宿主产品 | 删，范围外 |
