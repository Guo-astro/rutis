# 宿主自己写插件：agent 的工具、隔离、出错与迭代

[English](design-host-writes-plugins-2026-10-27.en.md) · [中文](design-host-writes-plugins-2026-10-27.md)

状态：草案，未实现。日期：2026-10-27。基准：`main` `103c0d22`。排期：0.19（只做设计）；实现在 0.19 之后。
Issue：[#185](https://github.com/arcships/rutis/issues/185)（含并入的 [#693](https://github.com/arcships/rutis/issues/693)）。沿用并更新 [#505](https://github.com/arcships/rutis/pull/505) 的工具草案（0.15 之前，已关闭）。
与 0.18 的关系：出错与状态全部用 [0.18 的诊断](design-diagnostics-2026-10-26.md)（错误图上的 `site`、错误码、`Loader::status()`），不另加结构。agent 是宿主的一部分，按该稿 §二 属于"看输出的人与宿主"一侧。
依赖的 0.19 工作：[#683](https://github.com/arcships/rutis/issues/683)（宿主管住插件的选项）、[#691](https://github.com/arcships/rutis/issues/691)（插件安装与嵌入 API）。本稿只按这两个 issue 写的能力引用它们，不假定字段名；需要它们提供的每一项列在 §十。

范围外：rutis-dsh 里的 agent 接入（§3.4）；为每个服务方法生成一个工具（§3.2）；agent 从 npm、PyPI 等包仓库装插件；agent 写的插件之间默认互相使用服务；内存、CPU 上限与文件、网络沙箱（§五）；实例内的服务、对象引用作为 `rutis_call` 的参数或结果；改了插件导入的子模块时不重载的已知缺口（[rutis-host 指南](guide/rutis-host.md)已写明）；嵌入宿主里按文件变化自动重载；"试用后丢弃、不写进配置"的模式；#505 的 `Loader::services()`、`ToolRegistry` 按作用域注册与注销、引用数与在途调用数上限。

## 一、要做成什么，现在卡在哪

[设计哲学](design-philosophy.md) §1 的主线：宿主需要接触什么，就自己写一个插件接上去，常常由 agent 来写；试用、修改、替换，这个连接从此成为宿主的一部分。

具体场景：一个宿主里跑着 agent。用户问"奥斯陆今天天气怎样"，agent 没有能回答的工具。它应该能：

1. 用自己的文件工具写一个 Python 插件，提供服务 `weather`；
2. 把它装进宿主；
3. 调用 `weather.today("Oslo")` 回答问题；
4. 改坏了：知道错在哪个文件哪一行，旧版本继续服务；
5. 修好、改了行为：换上新版本；
6. 不要了：删掉，进程干净退出。

现在卡在：

- **agent 碰不到插件的服务。** rutis-agent 的工具来自 `ToolRegistry`（`crates/rutis-agent/src/tools/mod.rs`），里面只有 `bash`、`replace_text` 等手写工具，与 rutis 的服务没有打通。[session 持久化与自我控制工具](design-session-persist-and-self-tools-2026-08-23.md) §三写明"不做动态加载新代码"，留作后续课题；本稿就是这个后续课题，做法不是让 agent 把代码加载进自己，而是让它往宿主里加插件行，生命周期由 rutis 已有的机制管。
- **agent 装不上、换不了、删不掉插件。** 嵌入的宿主没有"装一个本地目录"的接口（#691 要做）；`rutis-host dev` 能重载，但只看一个项目目录。
- **agent 写的插件没有隔离。** 和其他插件共用一个 Python 进程；死循环会让同进程的插件一起等。
- **agent 读不懂错误。** 0.17 的错误到宿主时已经是字符串；0.18 修好了结构，但还没有把它交给 agent 的入口。

## 二、决策

| # | 决策 | 章节 |
| --- | --- | --- |
| H1 | 宿主给 agent 三个固定的工具：`rutis_services`（列出能调用的服务）、`rutis_call`（按名字调用），沿用 #505；加 `rutis_plugin`（装上、重载、移除、看状态） | 三、四 |
| H2 | 不为每个服务生成一个工具：工具的 schema 固定，服务列表是数据 | 3.2 |
| H3 | agent 默认能调用插件行提供的服务；宿主自己的服务、节点的服务要宿主列进 `allow` 才能调用 | 3.3 |
| H4 | agent 写的插件是本地目录，用 #691 的嵌入 API 装进宿主配置里的一个组（"agent 组"），写进配置，宿主重启后仍在 | 四 |
| H5 | `rutis_plugin` 只能重载、移除 agent 组里的行；重载用已有的 `Loader::reload`（全有或全无，旧版本继续服务），只重载这一行 | 四 |
| H6 | 隔离只用 #683 的选项，设在 agent 组上：每个插件一个进程（开）、调用时限 30 秒（开）、能用的服务只有 `allow` 里的（开，默认为空）、问候时限保持默认。不提供沙箱 | 五 |
| H7 | 工具结果里的错误，就是 rutis 打印给人看的那段文字（带 `site` 与两边的位置）；agent 通过宿主的读取接口拿到，插件代码拿不到 | 六 |
| H8 | 实现放在 rutis-agent 的一个可选模块里，宿主装配时把三个工具交给 `ToolsPlugin::new`；`ToolRegistry` 不改 | 3.4 |
| H9 | 场景 S1 用 rutis-agent 与脚本化模型，E2E 放 `tests/e2e`，三个平台跑，进 nightly | 八 |

## 三、agent 怎样看到和调用服务

### 3.1 三个工具

| 工具 | 参数 | 结果 |
| --- | --- | --- |
| `rutis_services` | 无 | 当前能调用的服务：`[{name, provider, methods}]`；`provider` 是行名或"宿主"，`methods` 是 `{方法名: "sync" \| "async"}`，服务没说时为 `null` |
| `rutis_call` | `{service, method, arguments}`（`arguments` 是列表） | 方法的返回值（JSON）；失败时是 `error: ` 加 rutis 打印的错误文字（§六） |
| `rutis_plugin` | `{action: "add", path}` / `{action: "reload" \| "remove", name}` / `{action: "status"}` | 这次操作的结果，一项一行（§四、§七） |

数据从现有的接缝取，不加新的核心接口：

- `rutis_services`：服务名与提供者取自 0.18 的 `Loader::status().services`；`methods` 取自 `HostDispatch::methods()`（`crates/rutis-bridge/src/session/services.rs`）。#505 要新增的 `Loader::services()` 由 0.18 的快照取代，不再做。#505 的 `callable` 字段也去掉：0.17 起 Rust 服务按名字自动对外（`#[rutis::service]`），列表里只放能调用的。
- `rutis_call`：直接用已有的 `rutis_bridge::session::call_service`。它已经做到 #505 第一版要定死的几条：同步方法放到阻塞线程，异步方法等待结果；调用不带"调用方的行"，需要调用方 `&Ctx` 的方法明确失败；每次调用重新查服务，换了实现或撤下后下一次就看到。结果按普通数据解码；对象引用、函数引用作为结果时这次调用失败并说明（范围外）。
- 取消：agent 这一轮被取消时，`ToolRegistry::execute` 已经会丢弃工具的 future，`call_service` 的 future 被丢弃即取消对面的调用（0.18 §6.1 已有）。`rutis_call` 自己不设时限：按 0.18 §二，等多久是调用方的事；agent 写的插件的时限由 §五 的组设置管。

### 3.2 为什么不为每个服务生成工具

`ToolRegistry::register` 已经能在运行中加工具，看上去可以"装一个插件，多一个工具"。不这样做：

- 服务方法没有跨语言的参数 schema（0.18 的配置 schema 只管配置；#684 生成的是 Python 类型，不是工具参数），每个工具的参数只能猜；
- 工具列表随加载、重载失败、换实现不停变化，模型看到的工具定义每轮不同，脚本化测试难以复现；`ToolRegistry` 还要补注销（#505 的 `register_scoped`）；
- 两个通用工具在插件换语言、换位置（远程运行时）时都不用改，agent 每轮从 `rutis_services` 读当前状态。

代价：模型要多一步（先列再调）。方法的参数靠插件自己的说明和出错文字（`bad-argument` 写明第几个参数应是什么）。

### 3.3 默认能调用哪些服务

沿用 #505：

- **默认**：插件行提供的服务（`Loader::status()` 里提供者是行名的），包括手写插件的行、远程运行时上的行与 agent 组里的行。这正是宿主装插件的目的。
- **要宿主列出**：宿主自己提供的服务（提供者是"宿主"）、节点（peer）提供的服务。宿主装配时给出 `allow` 名单。名单外的不出现在 `rutis_services` 里，`rutis_call` 返回 `error: <服务名> is not open to the agent`。
- **不列出**：实例内的服务（agent 从宿主的根调用，看不到实例里的服务）。

`allow` 同时是 agent 组能用的服务名单（§五）。一份名单，含义是"宿主向 agent 这一侧开放的宿主能力"。

agent 的文件、命令权限来自它已有的 `bash`、`replace_text` 工具，本稿不增加也不削弱。

### 3.4 放在哪

- 新模块 `rutis_agent::rutis_tools`（rutis-agent 的可选 feature `plugins`，加上对 rutis-loader、rutis-bridge 的可选依赖）。它提供一个 `RutisTools`，从根 `Ctx`、`Loader`、agent 组的行名与 `allow` 构造，`defs()` 返回三个 `ToolDef`。宿主装配时把它们和其他工具一起交给 `ToolsPlugin::new(defs)`。工具集固定，`ToolRegistry` 不改。
- rutis-agent 是示例项目（不在 MSRV、测试加固与审计范围内），但 #185 的场景用它做端到端，所以放在这里。工具的逻辑只依赖 `Ctx`、`Loader`、`call_service` 与 #691 的嵌入 API，不依赖 rutis-agent 的类型；以后要作为产品接口时整体移出。
- **rutis-dsh 本轮不接**：dsh 的 agent 循环与工具服务在 Node 里（TS），rutis-dsh 只在 Unix、装了 npm 项目时构建，S1 要三个平台都跑。以后接入时，宿主把同样的三个操作作为一个按名字提供的服务交给 dsh，由一个 dsh 工具插件转发；等有使用方再做。

## 四、装上、重载、移除

### 4.1 agent 组

宿主在自己的配置里放一个组行（名字由宿主定，例如 `agent-plugins`），agent 装的插件都是它的子行。这个组同时决定两件事：

- §五 的隔离设置设在这个组上，组里的每一行都按它运行；
- `rutis_plugin` 的 `reload`、`remove` 只接受这个组里的行，agent 动不了手写插件的行。

### 4.2 四个动作

| 动作 | 做什么 | 用到的现有能力 |
| --- | --- | --- |
| `add {path}` | 把本地目录装进宿主：按目录内容判断语言、准备好运行时环境、在 agent 组下写一行；等这一行启动或失败后返回 | #691 的嵌入 API（`add <本地路径>`），装到指定的组下 |
| `reload {name}` | 重新读这一行的代码并换上；新代码导入不了、试运行失败或让别的行失败时，旧版本继续服务，返回原因 | 已有的 `Loader::reload(id)`（`crates/rutis-loader/src/loader/api.rs`），与 `rutis-host dev` 和 rutis-dev 通道的 `swap` 是同一个 |
| `remove {name}` | 卸载并移除这一行及它的安装记录；它的进程随之结束 | #691 的 `remove` 嵌入 API；进程结束靠 #683 的"每个插件一个进程" |
| `status` | agent 组里每一行的状态与原因、它们提供的服务、它们的运行时、进行中的调用 | 0.18 的 `Loader::status()`，只取 agent 组的部分 |

- **写进配置**：`add` 写的行和宿主持有者自己装的插件一样存进宿主的配置，宿主重启后仍在。这对应设计哲学 §1"连接从此成为宿主的一部分"。不想保留的，agent 调 `remove`。
- **只重载这一行**：`reload` 不重启运行时进程。agent 组里一个插件一个进程，但仍按行重载，与 `dev` 的规则一致（共用运行时时不能为一个插件重启整个进程）；而且只有按行重载才能做到"新代码失败时旧版本继续服务"。已知缺口：只重新导入这一行自己的模块，不重新导入它导入的子模块（范围外，指南已写明）。
- **不监视文件**：嵌入的宿主没有文件监视，agent 写完代码后显式调 `reload`。这让每一步什么时候生效是确定的。宿主跑在 `rutis-host dev` 下时，文件变化本来就会触发重载，`reload` 再调一次也只是一次没有变化的重载。
- `cannot reload` 那一句现在由 `rutis-host` 的 `dev.rs` 写出；实现时把这一句的写法移到 rutis-loader 的 `report` 模块，`dev` 与 `rutis_plugin` 共用，文字不变。

## 五、隔离：用 #683 的选项

agent 写的插件是没有人审过的代码，按宿主开放给第三方的插件对待。本稿不设计机制，只定 agent 组上默认打开哪些、取什么值：

| #683 的选项 | agent 组的默认 | 理由 |
| --- | --- | --- |
| 每个插件（每组插件）一个运行时进程 | 开：每个插件一个进程 | 一个插件死循环或崩溃不拖住别的插件；`remove` 后它的进程结束，S1 第 6 步可以断言。代价：每个 Python 插件多一个进程（几十 MB） |
| 调用的时限与取消 | 开：30 秒 | 进来的调用不只来自 agent，手写插件也可能用 agent 写的服务；时限到了调用失败并写明是哪个插件的哪个调用，宿主不被拖住。30 秒与本机问候时限相同，足够天气、检索这类调用 |
| 问候时限 | 不改（本机 30 秒） | agent 不在调试器里启动插件，不需要关掉 |
| 能用哪些服务 | 开：只有 `allow` 里的，默认为空 | 插件代码之后会在没有人看着时运行；默认不给它宿主能力。S1 的 `weather` 不用任何服务。要让它用某个服务（包括手写插件的服务），宿主把名字加进 `allow` |

没有的东西写清楚：rutis 不提供沙箱。agent 写的插件以宿主进程的用户运行，能读写的文件、能连的网络与宿主相同；也没有内存、CPU 上限。需要更强隔离的宿主，把 agent 组放到另一个用户或容器里的远程运行时上（0.19 #695），本稿不另做。

## 六、出了什么错、怎样修

agent 是宿主的一部分，不是调用方的插件代码。按 0.18 §二，`site` 与代码位置给看输出的人与宿主；agent 通过宿主的读取接口读它们，和宿主持有者在终端里看到的是同一段文字。agent 写的插件代码在运行时收到的错误照常只有名字、消息、错误码、`data`，不变。

| 出错的地方 | agent 从哪里看到 | 读的是什么 |
| --- | --- | --- |
| 调用失败（方法抛错、方法不存在、参数不对、时限到了） | `rutis_call` 的结果 | `rutis_bridge::thrown()` 取出错误图，用 0.18 打印错误的同一段代码（#677）写成文字：哪一行、哪个方法、为什么，下面是插件代码的文件与行号 |
| 装上后启动失败（导入错误、`apply` 出错、配置不符合 schema、服务名冲突） | `add` 的结果；之后 `status` | `Loader::status()` 里这一行的状态与原因 |
| 改坏了 | `reload` 的结果 | `cannot reload` 那一句，含原因与位置；旧版本继续服务 |
| 后来出的事（运行时退出、调用卡住） | `status` | 0.18 快照：运行时 `down` 与原因；进行中的调用与已等多久 |

错误码（`no-method`、`bad-argument` 等）给程序判断，0.18 规定它们不出现在给人看的文字里；agent 读的是文字，不另加码，文字本身已经写明怎么改（`methods: today, tomorrow`、`argument 1 should be a number`）。

## 七、开发者看到什么

### 7.1 宿主持有者

装配 agent 时：

```rust
let tools = rutis_agent::rutis_tools::RutisTools::new(&root, loader.clone(), "agent-plugins")
    .allow(["calendar"])            // 默认为空
    .await?;
let mut defs = vec![bash_tool(), replace_text_tool()];
defs.extend(tools.defs());
root.plugin(ToolsPlugin::new(defs))?;
```

- 配置里还没有 `agent-plugins` 时，`RutisTools` 在可编辑的配置里建这个组（`{"id": "agent-plugins", "group": true, "config": []}`），并按 §五 的表写上 #683 的组设置（写法以 #683 为准）。
- 已经有这个组时（宿主持有者自己写了，或上次运行建的），原样使用，不改它的设置：宿主持有者改过的值以配置为准。
- 宿主持有者在终端（`rutis-host status` 或宿主自己的输出）看到的行、错误，与 agent 在工具结果里看到的相同。

### 7.2 agent（模型看到的工具结果）

| 调用 | 结果 |
| --- | --- |
| `rutis_services` | `[{"name":"weather","provider":"weather","methods":{"today":"async"}}]`；没有时 `[]` |
| `rutis_call {service:"weather", method:"today", arguments:["Oslo"]}` | `"sunny (v1)"` |
| `rutis_plugin {action:"add", path:"weather"}` | `weather: running; provides weather` |
| 同上，目录不是插件项目 | `error: weather is not a plugin project: no pyproject.toml, package.json or go.mod`（文字以 #691 为准） |
| 同上，`apply` 出错 | `weather: failed in apply: KeyError: 'units'` + `src/weather/__init__.py:12 in apply` |
| `rutis_plugin {action:"reload", name:"weather"}`（语法错误） | `weather: cannot reload: SyntaxError: invalid syntax (src/weather/__init__.py, line 3); the previous version keeps running` |
| 同上，修好 | `weather: reloaded` |
| 同上，不是 agent 组里的行 | `error: calendar was not added by the agent; only rows under agent-plugins can be reloaded or removed` |
| `rutis_plugin {action:"remove", name:"weather"}` | `weather: removed` |
| `rutis_plugin {action:"status"}` | 0.18 快照的文字，只含 agent 组：`weather  running`、`  provides weather`、`runtime py-weather  running  pid 4123`（独占进程的运行时名以 #683 为准） |
| `rutis_call`，方法抛错 | `error: call to weather.today failed: KeyError: 'region'` + `weather at src/weather/__init__.py:9 in today` |
| `rutis_call`，方法名写错 | `error: weather has no method tody (methods: today)` |
| `rutis_call`，30 秒没返回 | #683 的超时文字，写明 `weather.today` |
| `rutis_call`，服务不存在或已移除 | `error: no service named weather is provided` |
| `rutis_call`，宿主服务不在 `allow` 里 | `error: llm is not open to the agent` |

所有文字遵守 0.18 §4.5 的禁用词（工具名是宿主交给模型的名字，不是出错文字）。

### 7.3 插件作者（agent 或人）

agent 写的插件就是一个普通的 Python 插件：`pyproject.toml` 加一个用 SDK 定义的插件，`provide` 一个 `weather` 服务。不写任何 agent 或 rutis 特有的东西；人写的同一个插件放进 agent 组以外的地方，行为相同。

## 八、场景 S1（按本设计更新）

取代 #185 正文里的场景与断言（issue 本身不改）。

角色：E2E 的一个新 fixture 宿主 `tests/e2e/fixture-agent`（Rust，嵌入 rutis-loader、rutis-bridge 与 rutis-agent），模型用 `ScriptedLlm`，脚本由场景写成 JSON 交给它；工具只有 `replace_text`（三个平台都能用，不用 `bash`）与三个 `rutis_*` 工具。场景每发一句用户消息开始一轮，fixture 把每次工具调用与结果写成一行 JSON。配置里有空的 `agent-plugins` 组，Python 运行时可用。

| 步 | agent 做的 | 断言 |
| --- | --- | --- |
| 1 | 第 1 轮：`rutis_services` 得到 `[]`；用 `replace_text create` 写出 `weather/pyproject.toml` 与 `weather/src/weather/__init__.py`（`today(city)` 返回 `"sunny (v1)"`） | 文件存在；`rutis_services` 结果为 `[]` |
| 2 | 同一轮：`rutis_plugin add {path:"weather"}` | 结果 `weather: running; provides weather`；`status` 里 `weather` 在一个只属于它的进程里运行 |
| 3 | 第 2 轮：`rutis_services`，再 `rutis_call weather.today ["Oslo"]` | 列表只有 `weather`（`today: async`）；结果 `"sunny (v1)"` |
| 4 | 第 3 轮：`replace_text str_replace` 写进语法错误，`rutis_plugin reload weather`，再 `rutis_call` | `reload` 结果以 `weather: cannot reload: SyntaxError` 开头，含 `src/weather/__init__.py`，以 `the previous version keeps running` 结尾；`rutis_call` 仍是 `"sunny (v1)"` |
| 5 | 第 4 轮：修好并改成返回 `"rainy (v2)"`，`reload`，再 `rutis_call` | `reload` 结果 `weather: reloaded`；`rutis_call` 结果 `"rainy (v2)"` |
| 6 | 第 5 轮：`rutis_plugin remove weather`，`rutis_services`，`rutis_call` | `remove` 结果 `weather: removed`；列表为 `[]`；`rutis_call` 结果 `error: no service named weather is provided`；第 2 步记下的进程 5 秒内退出 |

另外的断言：

- 每一轮交给模型的工具定义相同（同样四个工具）；每一轮的 `rutis_services` 结果依次是 `[]`、`[weather]`、`[weather]`、`[weather]`、`[]`。
- 服务不中断：从第 4 步开始到第 5 步 `reload` 返回，fixture 在后台每 100 ms 调一次 `weather.today`，没有一次失败；第 5 步 `reload` 返回之后的调用全部是 `"rainy (v2)"`，没有新旧交替。
- 工具结果里不出现 0.18 §4.5 的禁用词。
- fixture 停下后，`tests/e2e` 的残余检查通过（进程、socket、临时目录）。

## 九、兼容与迁移

全部是新加的、默认不装的东西：不装 `RutisTools` 的宿主行为不变；agent 组只在宿主放了它时存在。`cannot reload` 那一句换了实现位置，文字不变。没有不兼容的改动，不进《迁移到 0.19》。

## 十、实施

实现在 0.19 之后。按依赖拆成四步：

| 步 | 内容 | 能开始的条件 | 完成 |
| --- | --- | --- | --- |
| 1 | `rutis_services`、`rutis_call`：`RutisTools` 的骨架、`allow` 过滤、错误文字 | 0.18 的 #677（错误结构、`thrown()`、打印）与 #678（`Loader::status()`）已发布 | rutis-agent 里的测试：服务出现与消失时列表跟着变；未知服务、未知方法、参数不对、不在 `allow` 里各一条，文字逐字断言 |
| 2 | `rutis_plugin` 的四个动作；`cannot reload` 的句子移到 rutis-loader | #691 的嵌入 API（本地目录 `add`、`remove`、指定组）完成 | 测试：`add` 成功与三种失败；`reload` 失败时旧版本继续；不是 agent 组的行被拒绝；`remove` 后行与安装记录都没了 |
| 3 | agent 组上默认打开 #683 的选项（§五的表） | #683 完成 | 测试：组里两个插件在两个进程里；一个插件的调用超过 30 秒时失败并写明是谁；组里插件用 `allow` 之外的服务被拒绝；`remove` 后进程结束 |
| 4 | `tests/e2e/fixture-agent` 与 `tests/e2e/tests/host_writes_plugin.rs`（§八 S1）；rutis-agent 的 README 加一节"让 agent 写插件" | 1–3 完成；E2E 框架 #175 已完成（已关闭，`tests/e2e` 现有残余检查与 fixture 宿主可直接用） | Linux、macOS、Windows 通过；按 #175 的分法进 nightly（PR CI 只跑 S2、S3、S4、S9） |

对应 #185 的「完成」：设计稿两份（本稿）；S1 按本稿 §八 更新。

需要 #683 提供的：

1. 每个插件一个运行时进程，可以设在一组行上，对 Python 行有效；
2. 组里的一行被移除时，它独占的进程随之结束，不等宿主退出；
3. 调用时限可以设在一组行上，时限到了取消传到插件，失败文字写明插件与调用；
4. 能用的服务名单可以设在一组行上，名单外的使用被拒绝并说明；
5. 组上的这些设置对之后新加进组的行同样有效，加一行不重启组里其他行；
6. 组设置写在配置的组行上，宿主能用 loader 的编辑接口写（`RutisTools` 建组时写默认值）；
7. 问候时限保持现有默认（不需要新东西）。

需要 #691 提供的：

1. 嵌入 API 的 `add` 接受本地目录，按目录内容判断语言并准备运行时环境，不需要调用方写语言前缀；
2. `add` 能把新行放在指定的组下（第一次启动就带着组的设置）；
3. `add` 写的行存进宿主的可编辑配置，宿主重启后仍在；
4. 嵌入 API 的 `remove` 移除行与它的安装记录；
5. `add`、`remove` 失败时返回能直接打印的错误（遵守 0.18 的文字规则）。

## 十一、自审：删掉了什么

- #505 的 `Loader::services()`：0.18 的 `Loader::status()` 已有服务与提供者，删。
- #505 的 `callable` 字段与"列出但不能调用"：Rust 服务 0.17 起按名字自动对外，删。
- #505 的 `ToolRegistry::register_scoped` / `unregister`：工具集固定，装配时一次给齐，删。
- #505 的资源上限轨道（引用数、在途调用数）：属于 #683 或以后，不在这里。
- 文件监视：agent 显式 `reload` 更确定，嵌入宿主不加监视。
- agent 写的插件之间默认互相使用服务、从包仓库装：没有具体使用方，宿主可以用 `allow` 打开前者。
- rutis-dsh 接入：S1 要三个平台，dsh 只在 Unix；等有使用方。
