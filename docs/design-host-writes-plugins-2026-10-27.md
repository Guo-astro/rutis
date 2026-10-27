# 宿主自己写插件：agent 的工具、隔离、出错与迭代

[English](design-host-writes-plugins-2026-10-27.en.md) · [中文](design-host-writes-plugins-2026-10-27.md)

状态：草案，未实现。日期：2026-10-27。基准：`main` `103c0d22`。排期：0.19（只做设计）；实现在 0.19 之后。
Issue：[#185](https://github.com/arcships/rutis/issues/185)（含并入的 [#693](https://github.com/arcships/rutis/issues/693)）。沿用并更新 [#505](https://github.com/arcships/rutis/pull/505) 的工具草案（0.15 之前，已关闭）。
与 0.18 的关系：出错与状态全部用 [0.18 的诊断](design-diagnostics-2026-10-26.md)（错误图上的 `site`、错误码、`Loader::status()`），不另加结构。agent 是宿主的一部分，按该稿 §二 属于"看输出的人与宿主"一侧。
依赖的 0.19 工作：[#683](https://github.com/arcships/rutis/issues/683)（宿主管住插件，设计见 [PR #733](https://github.com/arcships/rutis/pull/733) 的 `docs/design-host-plugin-limits-2026-10-27.md`）、[#691](https://github.com/arcships/rutis/issues/691)（插件安装与嵌入 API，设计见 [PR #734](https://github.com/arcships/rutis/pull/734) 的 `docs/design-plugin-install-2026-10-27.md`）。需要它们提供的每一项列在 §十。

范围外：rutis-dsh 里的 agent 接入（§3.4）；为每个服务方法生成一个工具（§3.2）；agent 从 npm、PyPI 等包仓库装插件；agent 写的插件之间默认互相使用服务；内存、CPU 上限与文件、网络沙箱（§五）；agent 写 Go、Swift 插件（它们一个二进制一个运行时，#683 的命名本机运行时只有 Node、Python、Bun）；实例内的服务、对象引用作为 `rutis_call` 的参数或结果；改了插件导入的子模块时不重载的已知缺口（[rutis-host 指南](guide/rutis-host.md)已写明）；嵌入宿主里按文件变化自动重载；"试用后丢弃、不写进配置"的模式；#505 的 `Loader::services()`、`ToolRegistry` 按作用域注册与注销、引用数与在途调用数上限。

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
| H4 | agent 写的插件是本地目录，用 #691 的嵌入 API（`Installer`）装上；`RutisTools` 为它加一个只属于它的命名本机运行时（`agent-<插件名>`），把行写进宿主可编辑配置里的一个组（"agent 组"），宿主重启后仍在 | 四 |
| H5 | agent 组只标出哪些行是 agent 装的：`rutis_plugin` 只能重载、移除这些行；重载用已有的 `Loader::reload`（全有或全无，旧版本继续服务），只重载这一行；移除时连同它的运行时一起移除，进程结束 | 四 |
| H6 | 隔离只用 #683 的选项，设在每个 agent 插件自己的运行时上：一个插件一个进程；`limits` 为调用时限 30 秒、超时结束进程（`stop: "process"`）、`services` 等于 `allow`（默认为空）；问候时限保持默认。不提供沙箱 | 五 |
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
- 取消：agent 这一轮被取消时，`ToolRegistry::execute` 已经会丢弃工具的 future，`call_service` 的 future 被丢弃即取消对面的调用（0.18 §6.1 已有）。`rutis_call` 自己不设时限：按 0.18 §二，等多久是调用方的事；agent 写的插件的时限由 §五 它自己运行时上的 `limits.call` 管。

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

`allow` 同时是每个 agent 插件的运行时上 `limits.services` 的值（§五）。一份名单，含义是"宿主向 agent 这一侧开放的服务"。

agent 的文件、命令权限来自它已有的 `bash`、`replace_text` 工具，本稿不增加也不削弱。

### 3.4 放在哪

- 新模块 `rutis_agent::rutis_tools`（rutis-agent 的可选 feature `plugins`，加上对 rutis-loader、rutis-bridge 的可选依赖）。它提供一个 `RutisTools`，从根 `Ctx`、`Loader`、agent 组的行名与 `allow` 构造，`defs()` 返回三个 `ToolDef`。宿主装配时把它们和其他工具一起交给 `ToolsPlugin::new(defs)`。工具集固定，`ToolRegistry` 不改。
- rutis-agent 是示例项目（不在 MSRV、测试加固与审计范围内），但 #185 的场景用它做端到端，所以放在这里。工具的逻辑只依赖 `Ctx`、`Loader`、`call_service` 与 #691 的嵌入 API，不依赖 rutis-agent 的类型；以后要作为产品接口时整体移出。
- **rutis-dsh 本轮不接**：dsh 的 agent 循环与工具服务在 Node 里（TS），rutis-dsh 只在 Unix、装了 npm 项目时构建，S1 要三个平台都跑。以后接入时，宿主把同样的三个操作作为一个按名字提供的服务交给 dsh，由一个 dsh 工具插件转发；等有使用方再做。

## 四、装上、重载、移除

### 4.1 agent 组与每个插件的运行时

- **agent 组**：宿主可编辑配置里的一个组行（名字由宿主定，例如 `agent-plugins`），agent 装的插件都是它的子行。它只用来标出哪些行是 agent 装的：`rutis_plugin` 的 `reload`、`remove` 只接受这个组里的行，agent 动不了手写插件的行。它不承担任何限制。
- **每个插件一个运行时**：#683 的限制设在命名运行时上（同一运行时里的行互相提供的服务不受 `services` 限制）。所以 `RutisTools` 给每个 agent 插件加一个只属于它的命名本机运行时 `agent-<插件名>`（Rust 嵌入的 `LocalRuntime::named(..).limits(..)`），行写成 `agent-<插件名>:<插件>`。这样每个插件一个进程，插件之间也互相用不到服务（范围外）。
- **记录**：`RutisTools` 把每个 agent 插件的 `{插件名, 运行时名, #691 的安装记录}` 存进宿主给它的一个文件（例如 `.rutis/agent-plugins.json`）；#691 的安装记录本来就由宿主保存。宿主建 loader 时把 #683 的 `RuntimeSet` 放进 `Chain`（`RutisTools` 用它在运行中增删运行时）；启动时 `RutisTools` 按这个文件、照 §4.2 `add` 的顺序重新加上这些运行时，配置里的行等到自己的运行时后启动。

### 4.2 四个动作

| 动作 | 做什么 | 用到的现有能力 |
| --- | --- | --- |
| `add {path}` | 用 #691 的 `Installer::add` 装本地目录（按项目文件判断语言、准备环境），得到 loader 认得的行名（如 `py:weather`）；按 #683 的顺序加运行时 `agent-weather`：先 `root.plugin` 装上 `LocalRuntime`（语言取自安装结果，带 §五 的 `limits`）与它的 `RuntimeRowsPlugin`，再 `RuntimeSet::add` 加入它的解析器，最后用 `Loader::create` 在 agent 组下写行 `{"id": "weather", "name": "agent-weather:weather"}`；存记录；等这一行启动或失败后返回 | #691 的嵌入 API；#683 的命名运行时、`Limits` 与 `RuntimeSet`；已有的 `Loader::create(entry, parent, ..)` |
| `reload {name}` | 重新读这一行的代码并换上；新代码导入不了、试运行失败或让别的行失败时，旧版本继续服务，返回原因。它的运行时进程因超时被结束过时，先重新启动这个运行时（与 `dev` 的"进程结束了的本机运行时由下一次改动重新启动"相同） | 已有的 `Loader::reload(id)`（`crates/rutis-loader/src/loader/api.rs`），与 `rutis-host dev` 和 rutis-dev 通道的 `swap` 是同一个 |
| `remove {name}` | 按 #683 的顺序：先 `Loader::remove` 删掉这一行，再卸载 `LocalRuntime` 与 `RuntimeRowsPlugin` 两个 fiber（进程随之结束，输出 `runtime agent-weather: stopped`，其他运行时的 pid 不变），最后 `RuntimeSet::remove`；然后 `Installer::remove` 删掉安装，删掉记录 | 已有的 `Loader::remove`；#683 的 `RuntimeSet`；#691 的 `Installer::remove` |
| `status` | agent 组里每一行的状态与原因、它们提供的服务、它们的运行时、进行中的调用 | 0.18 的 `Loader::status()`，只取 agent 组的部分 |

- **写进配置**：#691 的嵌入 API 不读写配置；行由 `RutisTools` 用 `Loader::create` 写进宿主的可编辑配置，和宿主持有者自己装的插件一样，宿主重启后仍在。这对应设计哲学 §1"连接从此成为宿主的一部分"。不想保留的，agent 调 `remove`。
- **只重载这一行**：`reload` 不重启运行时进程。agent 插件一个插件一个运行时，但仍按行重载，与 `dev` 的规则一致（共用运行时时不能为一个插件重启整个进程）；而且只有按行重载才能做到"新代码失败时旧版本继续服务"。已知缺口：只重新导入这一行自己的模块，不重新导入它导入的子模块（范围外，指南已写明）。
- **改了代码不用再 `add`**：#691 对本地目录用链接（npm）或 `-e`（Python）安装，agent 改的就是装上的代码。
- **不监视文件**：嵌入的宿主没有文件监视，agent 写完代码后显式调 `reload`。这让每一步什么时候生效是确定的。宿主跑在 `rutis-host dev` 下时，文件变化本来就会触发重载，`reload` 再调一次也只是一次没有变化的重载。
- `cannot reload` 那一句现在由 `rutis-host` 的 `dev.rs` 写出；实现时把这一句的写法移到 rutis-loader 的 `report` 模块，`dev` 与 `rutis_plugin` 共用，文字不变。

## 五、隔离：用 #683 的选项

agent 写的插件是没有人审过的代码，按宿主开放给第三方的插件对待。本稿不设计机制，只定每个 agent 插件的运行时上用哪些 #683 的选项、取什么值：

| #683 的选项 | agent 插件的运行时上 | 理由 |
| --- | --- | --- |
| 命名本机运行时（一个进程） | 每个插件一个：`agent-<插件名>` | 一个插件死循环或崩溃不拖住别的插件；`remove` 时连运行时一起移除，进程结束，S1 第 6 步可以断言。代价：每个插件多一个进程（Python 几十 MB） |
| `limits.call` | 30 秒 | 进来的调用不只来自 agent，手写插件也可能用 agent 写的服务；超时的调用以 `timeout` 失败并写明运行时与调用，宿主不被拖住。30 秒与本机问候时限相同，足够天气、检索这类调用 |
| `limits.stop` | `"process"` | 卡在同步代码里的插件取消不了（Python 的 `def`、JS 的忙循环），只有结束进程才停得下；进程只属于这个插件，结束它不影响别人。之后 agent 修好代码再 `reload` 时重新启动这个运行时（§4.2） |
| `limits.services` | 等于 `allow`，默认为空 | 插件代码之后会在没有人看着时运行；默认不给它任何运行时之外的服务，用了就是 `not-allowed`。S1 的 `weather` 不用任何服务。要让它用某个服务（包括手写插件的服务），宿主把名字加进 `allow` |
| `greeting` | 不设（本机 30 秒） | agent 不在调试器里启动插件 |

没有的东西写清楚：rutis 不提供沙箱。agent 写的插件以宿主进程的用户运行，能读写的文件、能连的网络与宿主相同；也没有内存、CPU 上限（#683 的指南写明按进程用操作系统工具限的做法，每个 agent 插件有自己的进程，可以照做）。需要更强隔离的宿主，把 agent 插件放到另一个用户或容器里的远程运行时上（0.19 #695），本稿不另做。

## 六、出了什么错、怎样修

agent 是宿主的一部分，不是调用方的插件代码。按 0.18 §二，`site` 与代码位置给看输出的人与宿主；agent 通过宿主的读取接口读它们，和宿主持有者在终端里看到的是同一段文字。agent 写的插件代码在运行时收到的错误照常只有名字、消息、错误码、`data`，不变。

| 出错的地方 | agent 从哪里看到 | 读的是什么 |
| --- | --- | --- |
| 调用失败（方法抛错、方法不存在、参数不对、`timeout`、`not-allowed`） | `rutis_call` 的结果 | `rutis_bridge::thrown()` 取出错误图，用 0.18 打印错误的同一段代码（#677）写成文字：哪一行、哪个方法、为什么，下面是插件代码的文件与行号 |
| 装上后启动失败（导入错误、`apply` 出错、配置不符合 schema、服务名冲突、必需的服务不在 `allow` 里） | `add` 的结果；之后 `status` | `Loader::status()` 里这一行的状态与原因 |
| 改坏了 | `reload` 的结果 | `cannot reload` 那一句，含原因与位置；旧版本继续服务 |
| 后来出的事（运行时退出、超时后进程被结束、调用卡住） | `status` | 0.18 快照：运行时 `down` 与原因；进行中的调用与已等多久 |

错误码（`no-method`、`bad-argument` 等）给程序判断，0.18 规定它们不出现在给人看的文字里；agent 读的是文字，不另加码，文字本身已经写明怎么改（`methods: today, tomorrow`、`argument 1 should be a number`）。

## 七、开发者看到什么

### 7.1 宿主持有者

装配 agent 时：

```rust
let tools = rutis_agent::rutis_tools::RutisTools::new(&root, loader.clone(), "agent-plugins")
    .allow(["calendar"])            // 默认为空
    .records(home.join(".rutis/agent-plugins.json"))
    .installer(installer)           // #691 的 Installer，宿主照自己的目录建
    .runtimes(runtime_set.clone())  // #683 的 RuntimeSet，建 loader 时已放进 Chain
    .await?;
let mut defs = vec![bash_tool(), replace_text_tool()];
defs.extend(tools.defs());
root.plugin(ToolsPlugin::new(defs))?;
```

- 配置里还没有 `agent-plugins` 时，`RutisTools` 在可编辑的配置里建这个组（`{"id": "agent-plugins", "group": true, "config": []}`）；已经有时原样使用。组上没有任何设置。
- 启动时 `RutisTools` 按记录文件给每个 agent 插件加回它的运行时，带 §五 的 `Limits`（`Limits::new().call(Duration::from_secs(30)).stop(Stop::Process).services(allow)`）。要换值，宿主用 `.limits(..)` 给出自己的 `Limits`，`services` 仍以 `allow` 为准。
- 宿主持有者在终端（`rutis-host status` 或宿主自己的输出）看到的行、错误，与 agent 在工具结果里看到的相同。

### 7.2 agent（模型看到的工具结果）

| 调用 | 结果 |
| --- | --- |
| `rutis_services` | `[{"name":"weather","provider":"weather","methods":{"today":"async"}}]`；没有时 `[]` |
| `rutis_call {service:"weather", method:"today", arguments:["Oslo"]}` | `"sunny (v1)"` |
| `rutis_plugin {action:"add", path:"weather"}` | `weather: running; provides weather` |
| 同上，目录不是插件项目 | `error: ./weather has no package.json, pyproject.toml or go.mod`（#691 的文字） |
| 同上，`weather` 必需的服务 `llm` 不在 `allow` 里 | `weather: uses llm, which the host does not let runtime agent-weather use (allowed: )`（#683 的文字） |
| 同上，`apply` 出错 | `weather: failed in apply: KeyError: 'units'` + `src/weather/__init__.py:12 in apply` |
| `rutis_plugin {action:"reload", name:"weather"}`（语法错误） | `weather: cannot reload: SyntaxError: invalid syntax (src/weather/__init__.py, line 3); the previous version keeps running` |
| 同上，修好 | `weather: reloaded` |
| 同上，不是 agent 组里的行 | `error: calendar was not added by the agent; only rows under agent-plugins can be reloaded or removed` |
| `rutis_plugin {action:"remove", name:"weather"}` | `weather: removed` |
| `rutis_plugin {action:"status"}` | 0.18 快照的文字，只含 agent 组：`weather  running`、`  provides weather`、`runtime agent-weather  running  pid 4123` |
| `rutis_call`，方法抛错 | `error: call to weather.today failed: KeyError: 'region'` + `weather at src/weather/__init__.py:9 in today` |
| `rutis_call`，方法名写错 | `error: weather has no method tody (methods: today)` |
| `rutis_call`，30 秒没返回 | `error: call to weather.today did not finish within 30s (the host's limit for runtime agent-weather); the call was cancelled; runtime agent-weather was ended`（#683 的文字，错误码 `timeout`） |
| `weather` 的代码用了不在 `allow` 里的可选服务 | 插件代码收到 `not-allowed`：`llm is not available to weather: the host does not let runtime agent-weather use it`；它没处理时，`rutis_call` 的结果是这一句前面加上 `error: call to weather.today failed: ` |
| `rutis_call`，服务不存在或已移除 | `error: no service named weather is provided` |
| `rutis_call`，宿主服务不在 `allow` 里 | `error: llm is not open to the agent` |

所有文字遵守 0.18 §4.5 的禁用词（工具名是宿主交给模型的名字，不是出错文字）。

### 7.3 插件作者（agent 或人）

agent 写的插件就是一个普通的 Python 插件：`pyproject.toml` 加一个用 SDK 定义的插件，`provide` 一个 `weather` 服务。不写任何 agent 或 rutis 特有的东西；人写的同一个插件放进 agent 组以外的地方，行为相同。

## 八、场景 S1（按本设计更新）

取代 #185 正文里的场景与断言（issue 本身不改）。

角色：E2E 的一个新 fixture 宿主 `tests/e2e/fixture-agent`（Rust，嵌入 rutis-loader、rutis-bridge 与 rutis-agent），模型用 `ScriptedLlm`，脚本由场景写成 JSON 交给它；工具只有 `replace_text`（三个平台都能用，不用 `bash`）与三个 `rutis_*` 工具。场景每发一句用户消息开始一轮，fixture 把每次工具调用与结果写成一行 JSON。配置里有空的 `agent-plugins` 组；机器上有 Python（#691 按需建环境）。

| 步 | agent 做的 | 断言 |
| --- | --- | --- |
| 1 | 第 1 轮：`rutis_services` 得到 `[]`；用 `replace_text create` 写出 `weather/pyproject.toml` 与 `weather/src/weather/__init__.py`（`today(city)` 返回 `"sunny (v1)"`） | 文件存在；`rutis_services` 结果为 `[]` |
| 2 | 同一轮：`rutis_plugin add {path:"weather"}` | 结果 `weather: running; provides weather`；`status` 里有运行时 `agent-weather`，只有 `weather` 一行，记下它的 pid |
| 3 | 第 2 轮：`rutis_services`，再 `rutis_call weather.today ["Oslo"]` | 列表只有 `weather`（`today: async`）；结果 `"sunny (v1)"` |
| 4 | 第 3 轮：`replace_text str_replace` 写进语法错误，`rutis_plugin reload weather`，再 `rutis_call` | `reload` 结果以 `weather: cannot reload: SyntaxError` 开头，含 `src/weather/__init__.py`，以 `the previous version keeps running` 结尾；`rutis_call` 仍是 `"sunny (v1)"` |
| 5 | 第 4 轮：修好并改成返回 `"rainy (v2)"`，`reload`，再 `rutis_call` | `reload` 结果 `weather: reloaded`；`rutis_call` 结果 `"rainy (v2)"` |
| 6 | 第 5 轮：`rutis_plugin remove weather`，`rutis_services`，`rutis_call` | `remove` 结果 `weather: removed`；列表为 `[]`；`rutis_call` 结果 `error: no service named weather is provided`；输出 `runtime agent-weather: stopped`；`status` 里没有运行时 `agent-weather`，其他运行时的 pid 不变；第 2 步记下的进程 5 秒内退出；记录文件里没有 `weather` |

另外的断言：

- 每一轮交给模型的工具定义相同（同样四个工具）；每一轮的 `rutis_services` 结果依次是 `[]`、`[weather]`、`[weather]`、`[weather]`、`[]`。
- 服务不中断：从第 4 步开始到第 5 步 `reload` 返回，fixture 在后台每 100 ms 调一次 `weather.today`，没有一次失败；第 5 步 `reload` 返回之后的调用全部是 `"rainy (v2)"`，没有新旧交替。
- 工具结果里不出现 0.18 §4.5 的禁用词。
- fixture 停下后，`tests/e2e` 的残余检查通过（进程、socket、临时目录）。

## 九、兼容与迁移

全部是新加的、默认不装的东西：不装 `RutisTools` 的宿主行为不变；agent 组和 `agent-*` 运行时只在宿主装了它时存在。`cannot reload` 那一句换了实现位置，文字不变。没有不兼容的改动，不进《迁移到 0.19》。

## 十、实施

实现在 0.19 之后。按依赖拆成四步：

| 步 | 内容 | 能开始的条件 | 完成 |
| --- | --- | --- | --- |
| 1 | `rutis_services`、`rutis_call`：`RutisTools` 的骨架、`allow` 过滤、错误文字 | 0.18 的 #677（错误结构、`thrown()`、打印）与 #678（`Loader::status()`）已发布 | rutis-agent 里的测试：服务出现与消失时列表跟着变；未知服务、未知方法、参数不对、不在 `allow` 里各一条，文字逐字断言 |
| 2 | `rutis_plugin` 的四个动作（先不带 `limits`）、记录文件与启动时加回运行时；`cannot reload` 的句子移到 rutis-loader | #691 的 `Installer` 完成；#683 的第 1 步（`Limits`、命名运行时）完成 | 测试：`add` 成功与三种失败；`reload` 失败时旧版本继续；不是 agent 组的行被拒绝；`remove` 后行、运行时、安装与记录都没了；重启宿主后 agent 插件照常运行 |
| 3 | 每个 agent 插件的运行时带上 §五 的 `Limits` | #683 完成 | 测试：两个 agent 插件在两个进程里，互相用不到对方的服务；一个插件的调用超过 30 秒时以 `timeout` 失败、它的进程被结束、另一个照常，`reload` 后重新启动；用 `allow` 之外的服务以 `not-allowed` 失败 |
| 4 | `tests/e2e/fixture-agent` 与 `tests/e2e/tests/host_writes_plugin.rs`（§八 S1）；rutis-agent 的 README 加一节"让 agent 写插件" | 1–3 完成；E2E 框架 #175 已完成（已关闭，`tests/e2e` 现有残余检查与 fixture 宿主可直接用） | Linux、macOS、Windows 通过；按 #175 的分法进 nightly（PR CI 只跑 S2、S3、S4、S9） |

对应 #185 的「完成」：设计稿两份（本稿）；S1 按本稿 §八 更新。

需要 #683 提供的（按它在 PR #733 的设计：限制设在命名运行时上）：

1. rutis-loader 的 `RuntimeSet`：建 loader 时放进 `Chain` 一次，之后用 `add(rows)` / `remove(name)` 在运行中增删命名本机运行时（`LocalRuntime::named(..)`，Node、Python、Bun）的解析器，把 `<运行时名>:<插件>` 的行送到对应运行时，不重启其他运行时；
2. 增删的顺序有文档：加入时先 `root.plugin(LocalRuntime + RuntimeRowsPlugin)`、再 `RuntimeSet::add`、再插入行；移除时先删行、再卸载这两个 fiber（进程随之结束，输出 `runtime <名字>: stopped`，其他运行时的 pid 不变）、最后 `RuntimeSet::remove`；
3. `Limits` 能设在这样加的运行时上（`LocalRuntime::limits`）：`call`、`stop`、`services`；
4. `stop: "process"` 结束进程后，Rust 嵌入能把这个运行时重新启动（`reload` 用）；
5. `timeout`、`not-allowed` 的错误照 0.18 的错误结构带 `site`，宿主用 `rutis_bridge::thrown()` 读得到；
6. 同一运行时里的行互相提供的服务不受 `services` 限制（本稿每个运行时只有一行，用不到这一条，只是不与它冲突）；
7. 问候时限保持现有默认（不需要新东西）。

需要 #691 提供的（按它在 PR #734 的设计）：

1. `Installer::add` 接受本地目录，按项目文件判断语言并准备环境，不需要调用方写语言前缀；
2. 返回的安装结果里有语言与 loader 认得的行名（`py:weather`），`RutisTools` 从中取插件名，把前缀换成自己的运行时名；
3. 本地目录用链接或 `-e` 安装，改了代码不用重装；
4. 安装记录可以序列化，由宿主保存；`Installer::remove` 按记录删掉安装；
5. API 不读写配置（行放进哪个组、写进哪份配置由 `RutisTools` 决定）；
6. 失败时返回能直接打印的错误（遵守 0.18 的文字规则）。

## 十一、自审：删掉了什么

- #505 的 `Loader::services()`：0.18 的 `Loader::status()` 已有服务与提供者，删。
- #505 的 `callable` 字段与"列出但不能调用"：Rust 服务 0.17 起按名字自动对外，删。
- #505 的 `ToolRegistry::register_scoped` / `unregister`：工具集固定，装配时一次给齐，删。
- #505 的资源上限轨道（引用数、在途调用数）：属于 #683 或以后，不在这里。
- agent 组上的限制设置：#683 的限制设在运行时上，组只用来标出 agent 装的行。
- 文件监视：agent 显式 `reload` 更确定，嵌入宿主不加监视。
- agent 写的插件之间默认互相使用服务、从包仓库装：没有具体使用方，宿主可以用 `allow` 打开前者。
- rutis-dsh 接入：S1 要三个平台，dsh 只在 Unix；等有使用方。
