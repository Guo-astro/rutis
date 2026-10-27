# 宿主自己写插件：宿主里的 agent 怎样操作和管理插件

[English](design-host-writes-plugins-2026-10-27.en.md) · [中文](design-host-writes-plugins-2026-10-27.md)

状态：草案，未实现。日期：2026-10-27。基准：`main` `103c0d22`。排期：0.19（只做设计）；实现在 0.19 之后。
Issue：[#185](https://github.com/arcships/rutis/issues/185)（含并入的 [#693](https://github.com/arcships/rutis/issues/693)）。取代 [#505](https://github.com/arcships/rutis/pull/505) 的"给 agent 框架加两个工具"草案。
与 0.18 的关系：状态与出错全部用 [0.18 的诊断](design-diagnostics-2026-10-26.md)（`Loader::status()`、错误图上的 `site`、错误码），不加结构。
依赖的 0.19 设计：[#683](https://github.com/arcships/rutis/issues/683)（宿主管住插件，[PR #733](https://github.com/arcships/rutis/pull/733) 的 `docs/design-host-plugin-limits-2026-10-27.md`）、[#691](https://github.com/arcships/rutis/issues/691)（插件安装，[PR #734](https://github.com/arcships/rutis/pull/734) 的 `docs/design-plugin-install-2026-10-27.md`）、[#670](https://github.com/arcships/rutis/issues/670)（`rutis-host call`，[PR #731](https://github.com/arcships/rutis/pull/731) 的 `docs/design-try-and-test-plugins-2026-10-27.md`）。需要它们提供的列在 §十。

范围外：给某一种 agent 框架开工具（rutis-agent 的 `ToolRegistry` 要用时，是这个服务上一层几行的适配，不在本稿）；为每个服务生成一个工具；管理服务里的启用 / 停用一行、`upgrade`、单独的 `check` 方法（§十一）；装进单独的试用运行时、由宿主跑插件自己的测试（§五）；从 npm、PyPI 装（只装本地目录）；agent 写 Go、Swift 插件（#683 的命名本机运行时只有 Node、Python、Bun）；agent 写的插件之间互相使用服务；内存、CPU 上限与沙箱；实例内的服务；对象、函数引用作为 `call` 的参数或结果；改了子模块不重载的已知缺口（[rutis-host 指南](guide/rutis-host.md)已写明）；Windows 上的 S1（#670 的 `call` 不在 Windows 上）。

## 一、要做成什么，现在卡在哪

[设计哲学](design-philosophy.md) §1：宿主需要接触什么，就自己写一个插件接上去，常常由 agent 来写；试用、修改、替换，连接从此成为宿主的一部分。

具体场景：一个 `rutis-host run` 的宿主里跑着一个 agent。这个 agent 本身就是宿主里的一行插件（Python 写的，也可以是任何语言）。用户问"奥斯陆今天天气怎样"，它没有能回答的服务。它要：

1. 写一个 Python 插件 `weather`（它自己用 Python 的文件操作写进一个目录）；
2. 让宿主检查并装上；
3. 调用 `weather.today("Oslo")`；
4. 改坏了：知道错在哪个文件哪一行，旧版本继续服务；
5. 修好、换上新版本；
6. 不要了：删掉，它的进程退出；
7. 随时看现在有哪些行、服务、错误。

现在卡在：宿主有这些操作，但只给命令行和终端前的人——`rutis-host check`、`status`（0.18）、`call`（#670）、`add` / `remove`（#691）、`dev` 的重载。一行插件拿不到其中任何一个。[session 持久化与自我控制工具](design-session-persist-and-self-tools-2026-08-23.md) §三把"动态加载新代码"留作后续课题；本稿就是这个课题，做法是把宿主已有的操作交给一行插件，而不是让 agent 把代码加载进自己。

## 二、决策

| # | 决策 | 章节 |
| --- | --- | --- |
| H1 | 宿主按名字提供一个管理服务（默认名 `plugins`），5 个方法：`status`、`call`、`add`、`reload`、`remove`。agent 用各 SDK 都有的 `inject` 用它，不分语言、不分 agent 框架 | 三 |
| H2 | 这些方法和命令行、#670 的 socket 是同一套实现：都调 `Loader::status()`、`call_service`、#691 的 `Installer`、`Loader::check`、`Loader::reload`；组合起来的那一层放在 rutis-loader（`admin`），rutis-host 与嵌入宿主共用 | 三 |
| H3 | 默认不提供。宿主在配置里写明交给哪一行（`admin.for`）；别的行调用时以 `not-allowed` 拒绝 | 四 |
| H4 | agent 默认只能重载、移除它自己加的行（放在 `agent-plugins` 组里）；宿主可以写 `manage: "all"` 放宽 | 四 |
| H5 | `call` 默认能调插件行提供的服务；宿主自己的服务、节点的服务要列进 `allow` | 四 |
| H6 | 每个 agent 加的插件一个命名本机运行时 `agent-<插件名>`，带默认 `limits`：`call` 30 秒、`stop: "process"`、`services` 等于 `allow`（默认空） | 四 |
| H7 | `add`、`reload` 生效前按固定顺序检查：安装 → 运行时 → `Loader::check` → 装上或全有或全无地换上；哪一步失败就停在哪一步、撤掉这一次已做的、旧版本继续服务 | 五 |
| H8 | 结果是普通数据：`{ok, text, error?}`，`error` 是 0.18 错误图的根节点（含 `site`、位置），`text` 是 rutis 打印给人看的那段 | 六 |
| H9 | 加的东西写进宿主配置，重启后仍在：rutis-host 写进 `rutis.json`（行、`runtimes.local`、`installed`）；嵌入宿主经一个保存回调自己存 | 七 |
| H10 | S1 的 agent 是一个脚本化的 Python 插件，由测试用 `rutis-host call agent.step <n>` 一步一步驱动；不用模型 | 九 |

## 三、管理服务

### 3.1 方法

| 方法 | 参数 | 做什么 | 结果 |
| --- | --- | --- | --- |
| `status()` | 无 | 0.18 的 `Loader::status()`：每行的状态与原因、每个服务的提供者与方法（`{方法: "sync" \| "async"}`）、运行时、进行中的调用 | 快照（普通数据），外加 `text`（`rutis-host status` 打印的那段） |
| `call(service, method, args)` | 服务名、方法名、参数列表 | 以"宿主"身份按名字调用，与 `rutis-host call` 相同 | `{ok: true, value}` 或 `{ok: false, text, error}` |
| `add(path)` | 本地目录（相对宿主配置所在目录） | §5.1 的检查与装上 | `{ok, row, text, error?}` |
| `reload(row)` | 行名 | §5.2 的检查与换上 | `{ok, text, error?}` |
| `remove(row)` | 行名 | §5.3 | `{ok, text}` |

- 插件代码出的错（导入失败、`apply` 出错、方法抛错、超时）**不抛出**，放在结果里：agent 要读它、改代码，这是正常流程。只有越过权限才抛出（不是交给它的那一行、不能管的行），错误码是 #683 的 `not-allowed`，见 §八；行名写错也放在结果里。
- 方法集合只放场景用得到的：7 步里每一步对应一个方法。

### 3.2 和已有的操作是同一套

| 操作 | 命令行 | #670 的 socket | 管理服务 | 实现（已有或 0.19 要做） |
| --- | --- | --- | --- | --- |
| 看状态 | `rutis-host status` | `status` | `status()` | `Loader::status()`（0.18） |
| 调用 | `rutis-host call` | `call` | `call()` | `rutis_bridge::session::call_service` |
| 检查 | `rutis-host check` | — | 在 `add`、`reload` 里 | `Loader::check(&layers)`（`crates/rutis-loader/src/loader/check.rs`）；配置 Schema 的校验在其中（0.18） |
| 装上 | `rutis-host add <路径>`（#691） | — | `add()` | #691 的 `Installer::add`；运行中加运行时用 #683 的 `RuntimeSet`；写行用 `Loader::create` |
| 重载 | `dev` 的文件变化、rutis-dev 通道的 `swap` | — | `reload()` | `Loader::reload`（`crates/rutis-loader/src/loader/api.rs`） |
| 移除 | `rutis-host remove`（#691） | — | `remove()` | `Loader::remove`、`RuntimeSet::remove`、`Installer::remove` |

- 新加的只有把这几步按 §五 的顺序串起来的一层：`rutis_loader::admin::Admin`（rutis-loader 的 `admin` feature，带上 #691 的 `install`）。它有这 5 个方法的 Rust 版本，再加一个按名字提供它的 `HostDispatch`。rutis-host 用它；嵌入宿主用同一个。rutis-host 的库不是公开 API，所以不放那里（与 #691 的 `Installer` 同理）。
- socket 不加 `add` / `reload` / `remove`：命令行的 `add` / `remove` 改的是配置文件（#691），没有"对正在运行的宿主装插件"的使用方。
- `rutis-host call` 与 `call()` 的文字相同：两者都由 0.18 打印错误的同一段代码写出。

### 3.3 agent 怎样用

agent 的行写 `inject: ["plugins"]`，代码里照它的语言用这个服务，不写任何 rutis 特有的东西（Python 示意）：

```python
plugins = ctx.use("plugins")
result = await plugins.add("agent-plugins/weather")
if not result["ok"]:
    fix_from(result["error"])        # error 里有 site 与文件、行号
```

agent 写插件文件用它自己语言的文件操作；插件进程本来就以宿主用户的权限运行（#683 范围外：没有沙箱），rutis 不另给写文件的接口。

## 四、权限与限制

### 4.1 谁能用管理服务

- **默认不提供**：配置里没有 `admin` 时，宿主不提供这个服务，任何行 `inject` 它都是 0.18 的"没有提供者"。
- **交给一行**：`admin.for` 写一行的 id。服务收到调用时用已有的 `caller_row()`（`crates/rutis-bridge/src/session/services.rs`）认出调用的行；不是这一行时以 `not-allowed` 拒绝，文字写明交给了谁。不在任何行里的调用（宿主自己的 Rust 代码）照常可用。
- **为什么不用 #683 的 `services`**：它是给一个运行时列"能用哪些"的名单，不写时不限；要让"别的行都用不到 `plugins`"，就得给每个运行时都写名单，做不成默认。按调用的行认人只需要一处检查，用的是已有的 `caller_row()`，不另做权限体系。

### 4.2 agent 能管哪些行

| `admin.manage` | 能 `reload` / `remove` 的行 | 用在 |
| --- | --- | --- |
| `"own"`（默认） | 它用 `add` 加的行：都在 `agent-plugins` 组里（组由 `Admin` 建，不带任何设置，只用来标出这些行） | agent 改错了也碰不到宿主持有者手写、宿主依赖的插件 |
| `"all"` | 所有插件行（不含它自己那一行） | 整个宿主都由 agent 维护时，宿主持有者明确放宽 |

`status`、`call` 不受 `manage` 影响：agent 是替宿主干活的，看得到整个宿主。

### 4.3 `call` 能调哪些服务

默认能调插件行提供的服务（`Loader::status()` 里提供者是行名的），这正是宿主装插件的目的。宿主自己提供的服务、节点（peer）的服务要列进 `admin.allow`；列表外的以 `not-allowed` 拒绝。`call` 以"宿主"身份调用，被调用方的限制（#683）照常生效。

### 4.4 agent 加的插件的运行时

按 #683（限制设在命名运行时上），`add` 给每个插件加一个只属于它的命名本机运行时 `agent-<插件名>`：

| `limits` | 取值 | 理由 |
| --- | --- | --- |
| （一个插件一个运行时） | 是 | 一个插件死循环或崩溃不连累别人；`remove` 连运行时一起移除，进程退出。代价：每个插件多一个进程 |
| `call` | 30 秒 | 进来的调用不只来自 agent；超时以 `timeout` 失败，宿主不被拖住 |
| `stop` | `"process"` | 卡在同步代码里的插件取消不了，只能结束进程；进程只属于它。之后 `reload` 时重新启动这个运行时 |
| `services` | 等于 `admin.allow`，默认空 | 插件代码之后会在没人看着时运行；默认不给它运行时之外的服务。每个运行时只有一行，插件之间也互相用不到 |
| `greeting` | 不设 | agent 不在调试器里启动插件 |

宿主可以用 `admin.limits` 换这些值（`services` 仍以 `allow` 为准）。

## 五、内部检查：改的东西生效之前

### 5.1 `add(path)`

| 步 | 谁做、用什么 | 失败时 |
| --- | --- | --- |
| 1 安装 | `Installer::add`（#691）：按项目文件认语言、准备环境；本地目录用链接或 `-e` 装，之后改代码不用再装。得到语言与语言运行时下的行名（`py:weather`）；`Admin` 把前缀换成这个插件自己的命名运行时，写成 `agent-weather:weather`（不换会按 #691 的规则启用语言运行时 `runtimes.py`） | 返回 #691 的错误（`./agent-plugins/weather has no package.json, pyproject.toml or go.mod`）；什么都没改 |
| 2 运行时 | 按 #683 的顺序加 `agent-weather`：`root.plugin(LocalRuntime + RuntimeRowsPlugin)`、`RuntimeSet::add` | 撤掉第 1 步；返回原因 |
| 3 静态检查 | `Loader::check`（与 `rutis-host check` 同一个）：把新行 `agent-weather:weather` 加进当前配置检查——在新运行时里导入并描述它（语法、导入错误）、试运行（配置 Schema）、依赖都有人提供、没有依赖环、没有同名提供、必需的服务没有被 `services` 挡住 | 撤掉第 2、1 步；返回检查报告里这一行的问题（与 `check` 打印的相同） |
| 4 装上 | `Loader::create` 在 `agent-plugins` 组下写这一行，等它启动或失败（`apply` 出错、启动超时） | 删行、撤掉第 2、1 步；返回 0.18 的错误 |
| 5 保存 | §七 | — |

新插件没有旧版本，所以失败就整个撤掉，不留一个失败的行。

### 5.2 `reload(row)`

| 步 | 谁做、用什么 | 失败时 |
| --- | --- | --- |
| 1 运行时 | 它的进程被 `stop: "process"` 结束过时，先重新启动 `agent-<名>`（与 `dev` 的"进程结束了的本机运行时由下一次改动重新启动"相同） | 返回运行时起不来的原因；旧版本本来就不在运行 |
| 2 静态检查 | `Loader::check`：重新导入并描述这一行的新代码，检查同 §5.1 第 3 步 | 返回问题；旧版本继续服务，什么都没换 |
| 3 换上 | `Loader::reload`：全有或全无。新模块解析不了、试运行失败、或让别的行新失败时，恢复旧模块 | 返回 `cannot reload` 那一句与 0.18 的错误；旧版本继续服务 |

- 第 2 步看起来和 `Loader::reload` 自己的解析、试运行重复，但它多查两件 `reload` 不查的事：新代码新用到的服务有没有人提供、会不会成环。不查的话，新版本会换上后一直等一个没有的服务，旧版本已经没了。
- 已知边界：新代码在 `apply` 里失败时，`Loader::reload` 已经卸掉旧版本，再把旧版本装回来；这中间这个服务短暂不可用。语法与导入错误在第 2 步就拦下，不会走到这里。
- 只重载这一行，不重启运行时进程，与 `dev` 的规则相同。

### 5.3 `remove(row)`

按 #683 的顺序：`Loader::remove` 删行 → 卸载 `LocalRuntime` 与 `RuntimeRowsPlugin`（进程退出，输出 `runtime agent-weather: stopped`，其他运行时的 pid 不变）→ `RuntimeSet::remove` → `Installer::remove` → 保存。agent 写的源码目录不删，那是 agent 自己的文件。

### 5.4 删掉的步骤

- **单独的试用运行时**：先在另一个运行时里导入并 `apply` 新代码再换上。不做：`apply` 会开端口、打开文件、连外部服务，新旧两份同时 `apply` 会互相冲突；导入错误第 2 步已经在插件自己的运行时里拦下；它只多挡住"`apply` 失败"这一种，代价是每次多一个进程和一次真实的 `apply`。
- **宿主跑插件自己的测试**（含 #684 的契约检查）：agent 是一个插件进程，能自己跑它的语言的测试命令（`pytest`）；#684 的契约只描述宿主的 Rust 服务，S1 的插件不用。没有要宿主替它跑的使用方。

## 六、agent 读到的状态与错误

- 0.18 §二 说 `site` 与代码位置给看输出的人与宿主，不自动交给调用方的代码。管理服务是宿主持有者在配置里明确交给一行的宿主读取接口，和打开 #670 的 socket 一样是他的决定；普通调用里插件代码收到的错误仍然没有 `site`，不变。
- `error` 是 0.18 错误图的根节点（`name`、`message`、`code`、`data`、`site`、`stack`），由 `rutis_bridge::thrown()` 取出；`text` 是 0.18 打印的那段，两边的位置都在里面。agent 程序按 `code` 和 `site` 判断，模型读 `text`。

## 七、写进配置

| 宿主 | 写在哪 | 重启后 |
| --- | --- | --- |
| rutis-host | `rutis.json`：`agent-plugins` 组里的行、`runtimes.local` 里的 `agent-<名>`（带 `limits`）、#691 的 `installed` 记录。写的就是 rutis-host 重读配置时会得到的那些条目，所以它下一次重读（`--watch-config`、SIGUSR1）不当作变化 | 照配置启动，与手写的一样 |
| 嵌入宿主 | `Admin` 把 `{行, 运行时条目, 安装记录}` 交给宿主给的保存回调；宿主存在自己想存的地方（与 #691"记录归宿主"相同） | 宿主把记录交回 `Admin`，它按 §5.1 第 2 步的顺序加回运行时 |

宿主持有者打开 `rutis.json` 就能看到 agent 加了什么，也能自己删掉。

## 八、开发者看到什么

宿主持有者写的配置：

```json
{
  "runtimes": { "py": { "project": "." } },
  "admin": { "for": "agent", "allow": [] },
  "rows": [
    { "id": "agent", "name": "py:my_agent", "inject": ["plugins"] },
    { "id": "notes", "name": "py:notes" }
  ]
}
```

`admin` 的字段：`for`（必填）、`service`（默认 `plugins`）、`manage`（`"own"` / `"all"`）、`allow`（默认空）、`limits`（默认 §4.4）。Rust 嵌入：`Admin::new(&root, loader, installer, runtime_set).serve_to("agent").allow([..]).persist(hook)`，再 `root.plugin(admin.service("plugins"))`。

agent 看到的结果（`text` 部分；`ok` 与 `error` 同时给出）：

| 调用 | 结果 |
| --- | --- |
| `add("agent-plugins/weather")` | `ok: true`，`weather: running; provides weather` |
| 同上，语法错误（第 3 步） | `ok: false`，`weather: SyntaxError: invalid syntax (src/weather/__init__.py, line 3)`；`status()` 里没有 `weather` |
| 同上，`apply` 出错（第 4 步） | `ok: false`，`weather: failed in apply: KeyError: 'units'` + `src/weather/__init__.py:12 in apply` |
| 同上，必需的 `llm` 不在 `allow` 里 | `ok: false`，`weather: uses llm, which the host does not let runtime agent-weather use (allowed: )`（#683 的文字） |
| 同上，依赖没人提供 | `ok: false`，`weather: waits for geo, which no row provides` |
| `call("weather", "today", ["Oslo"])` | `ok: true`，`value: "sunny (v1)"` |
| 同上，方法抛错 | `ok: false`，`call to weather.today failed: KeyError: 'region'` + `weather at src/weather/__init__.py:9 in today` |
| 同上，30 秒没返回 | `ok: false`，`call to weather.today did not finish within 30s (the host's limit for runtime agent-weather); the call was cancelled; runtime agent-weather was ended`（`timeout`） |
| 同上，宿主服务不在 `allow` 里 | `ok: false`，`llm is not open to the agent`（`not-allowed`） |
| `reload("weather")`，语法错误 | `ok: false`，`weather: cannot reload: SyntaxError: invalid syntax (src/weather/__init__.py, line 3); the previous version keeps running` |
| 同上，修好 | `ok: true`，`weather: reloaded` |
| `remove("weather")` | `ok: true`，`weather: removed; runtime agent-weather: stopped` |
| `remove("notes")`（`manage: "own"`） | 抛出 `not-allowed`：`notes was not added by agent; the host lets agent manage only the rows it added` |
| `reload("wether")` | `ok: false`，`no row wether (rows agent added: weather)` |
| 别的行调用 `plugins.status()` | 抛出 `not-allowed`：`plugins is given to agent only` |

宿主持有者在终端看到的：rutis-host 照常打印行的变化（`weather: running`、`runtime agent-weather: stopped`），与 agent 收到的 `text` 相同。所有文字不出现 0.18 §4.5 的禁用词。

agent 写的插件就是普通的 Python 插件，不写任何 agent 或 rutis 特有的东西。

## 九、场景 S1（按本设计更新）

取代 #185 正文里的场景与断言（issue 本身不改）。

用一个脚本化的 Python 插件代替 agent：它提供服务 `agent`，方法 `step(n)` 照固定脚本做第 n 步、返回它收到的结果。这比接一个模型简单，走的是同一条路（一行插件 `inject` 管理服务）；agent 的推理不是本场景要测的。测试启动 `rutis-host run --allow-call`（#670），用 `rutis-host call agent.step <n>` 驱动每一步。配置照 §八，另有手写的 `notes` 行与一个 `intruder` 行（它也 `inject` 了 `plugins`）。

| 步 | `agent.step(n)` 做的 | 断言 |
| --- | --- | --- |
| 1 | 用 Python 写出 `agent-plugins/weather/pyproject.toml` 与 `src/weather/__init__.py`（`today` 返回 `"sunny (v1)"`）；`status()` | 文件存在；服务里没有 `weather` |
| 2 | `add("agent-plugins/weather")` | `ok`；`rutis.json` 多了 `agent-plugins` 组里的行、`runtimes.local` 的 `agent-weather`、`installed` 记录；`status` 里 `agent-weather` 只有 `weather` 一行，记下 pid |
| 3 | `call("weather", "today", ["Oslo"])` | `"sunny (v1)"` |
| 4 | 写进语法错误，`reload("weather")`，再 `call` | `reload` 的 `ok` 为假，`text` 以 `weather: cannot reload: SyntaxError` 开头、含 `src/weather/__init__.py`；`call` 仍是 `"sunny (v1)"` |
| 5 | 改成返回 `"rainy (v2)"`，`reload`，再 `call` | `ok`；`"rainy (v2)"` |
| 6 | `remove("weather")`，`status()` | `ok`；没有 `weather` 与 `agent-weather`；记下的进程 5 秒内退出；其他运行时的 pid 不变；`rutis.json` 回到第 1 步时的样子 |
| 7 | `remove("notes")` | 抛出 `not-allowed`；`notes` 照常运行 |

另外：

- `rutis-host call intruder.try_status`（它调 `plugins.status()`）得到 `not-allowed`。
- 服务不中断：第 4 步开始到第 5 步 `reload` 返回，测试每 100 ms 用 `rutis-host call weather.today '"Oslo"'` 调一次，没有一次失败；第 5 步之后全部是 `"rainy (v2)"`。
- 结果里不出现 0.18 §4.5 的禁用词。
- 宿主停下后，`tests/e2e` 的残余检查通过。
- 落点：`tests/e2e/tests/host_writes_plugin.rs`，脚本化的 agent 插件放在场景目录里。E2E 框架 #175 已完成（已关闭）。Linux、macOS（`call` 不在 Windows 上），进 nightly。

## 十、实施

实现在 0.19 之后：

| 步 | 内容 | 能开始的条件 | 完成 |
| --- | --- | --- | --- |
| 1 | `Admin` 的 `status`、`call`，按名字提供、`admin.for` 的检查、`allow` | 0.18 #677、#678；#670 | 单元测试：快照与方法；`call` 的成功与四种失败；别的行被拒 |
| 2 | `add`、`reload`、`remove` 与 §五 的检查顺序；保存回调 | #691 的 `Installer`；#683 的 `RuntimeSet`、`Limits` | §八 每一行有逐字断言的测试；每一步失败后撤得干净（没有多出的运行时、安装、行）；`manage` 两种取值 |
| 3 | rutis-host 读 `admin`、写 `rutis.json`、重启后照常 | 第 2 步；#683 的 `runtimes.local` | 写入后重读不算变化；重启后 agent 加的插件照常运行 |
| 4 | S1（§九）；rutis-host.md 与 rust-host.md 各加一节"让宿主里的 agent 管插件"（中英） | 1–3 | Linux、macOS 通过 |

对应 #185 的「完成」：设计稿两份（本稿）；S1 按 §九 更新。

需要 #683 提供的：

1. rutis-loader 的 `RuntimeSet`：运行中增删命名本机运行时，不重启其他运行时；增删顺序有文档（已在 PR #733）；
2. `LocalRuntime::limits` 的 `call`、`stop`、`services`；`timeout`、`not-allowed` 两个错误码带 0.18 的 `site`；
3. `stop: "process"` 结束进程后，Rust 嵌入能把这个运行时重新启动；
4. `Loader::check` 报告"必需的服务被 `services` 挡住"（PR #733 §4.4 已写 `check` 做同样的检查）；
5. rutis-host 用 `RuntimeSet` 建 loader，`runtimes.local` 的条目在运行中被加进 `rutis.json` 后，重读配置把它当作已有的运行时，不重启它。

需要 #691 提供的：`Installer::add` 接受本地目录并返回语言与行名；本地目录用链接或 `-e` 安装；记录可序列化，rutis-host 的 `installed` 字段与 `Installer` 的记录是同一个形状；`Installer::remove` 按记录删除；API 不读写配置。

需要 #670 提供的：`run --allow-call` 与 `call` 请求（S1 用它驱动）；`call` 的结果与错误文字由 0.18 的打印代码写出，管理服务的 `call()` 用同一段。没有要它新加的东西。

## 十一、自审：是不是太重了

| 项 | 使用方 | 结论 |
| --- | --- | --- |
| 管理服务的 5 个方法 | S1 的 7 步 | 保留 |
| 启用 / 停用一行 | S1 没有；`remove` 已能让它不运行 | 删 |
| `upgrade` | 本地目录改代码后 `reload` 就生效 | 删 |
| 单独的 `check` 方法 | 检查都在 `add`、`reload` 里；agent 没有"只查不装"的步骤 | 删 |
| socket 加 `add` / `reload` / `remove` | 命令行改配置文件，不对运行中的宿主装 | 删 |
| 试用运行时、宿主跑测试 | 见 §5.4 | 删 |
| `reload` 前的 `Loader::check` | 防新版本等一个没有的服务、成环 | 保留 |
| `manage: "all"` | 整个宿主由 agent 维护 | 保留，默认关 |
| 每个插件一个运行时 | 隔离、`remove` 后进程退出 | 保留 |
| 给 rutis-agent 写工具 | 是这个服务的薄适配 | 删，范围外 |
| 用模型驱动 S1 | 测的是管理服务，不是推理 | 删，用脚本化插件 |
