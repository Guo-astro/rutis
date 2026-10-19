# rutis-host 与 rutis.json

`rutis-host` 是不需要写 Rust 的宿主：它把内核、插件加载、本机的 Node / Python / Go 运行时和网络连接组装好，由一份 `rutis.json` 驱动。插件作者用它在本地开发，也可以直接用它部署。需要自定义 Rust 服务的应用见 [在 Rust 应用里嵌入](rust-host.md)。

## 安装

| 方式 | 命令 |
| --- | --- |
| npm | `npx @arcships/rutis-host …`，或在项目里 `npm install -D @arcships/rutis-host` 后 `npx rutis-host …` |
| PyPI | `uvx rutis-host …`，或 `uv add --dev rutis-host` 后 `uv run rutis-host …` |
| 二进制 | GitHub Release 的 `rutis-host-<版本>-<平台>.tar.gz`（Linux、macOS 的 x64 / arm64），Windows x64 为 `.zip` |
| crates.io | `cargo install rutis-host` |

npm 分发自带 Node 运行时（`@arcships/rutis-runtime`），PyPI 分发自带 Python 运行时（`rutis`）：项目里没有自己的运行时时用它们。Go 插件是编译好的二进制，运行它们不需要别的包，也不需要 Go 工具链。

## 命令

| 命令 | 作用 |
| --- | --- |
| `rutis-host run [rutis.json]` | 按配置运行（Unix 上 SIGUSR1 或加 `--watch-config` 时运行中重新读配置，见 [运行中改配置](#运行中改配置)），打印每一行状态的变化、节点行 link 的状态和本机运行时的结束（见 [run 打印什么](#run-打印什么)）；Ctrl-C 结束（见 [停止与退出码](#停止与退出码)） |
| `rutis-host dev [目录]` | 在插件项目里运行这个插件（加上 `rutis.dev.json`），文件变化时重新加载；Go 项目重新构建，只重启它的运行时 |
| `rutis-host check [rutis.json] [--strict]` | 不运行任何插件，检查每一行能不能运行（见 [检查配置](#检查配置)），打印版本、依赖、提供的服务、配置 Schema，并列出每个 Go 二进制（运行时名、SDK、插件 API、插件）；有不能运行的行或二进制时以非零状态退出。没有行声明、只可能在运行时由别的行提供的依赖是告警，加 `--strict` 时算错误。在没有 rutis.json 的插件项目里检查这个项目 |
| `rutis-host new <名字> --lang node\|bun\|python\|go` | 创建插件项目 |
| `rutis-host go add <模块>@<版本> [rutis.json]` | 用本机的 Go 工具链把插件二进制 `go install` 到 `runtimes.go.dir` |

## run 打印什么

`run` 和 `dev` 的输出是给人看、也给脚本和测试读的，格式如下，改动按不兼容的变化处理。

启动时，每个 [`listen`](#listen) 监听器打印一次它实际绑定的地址（端口写 0 时是系统挑的端口）：

```
rutis-host: listening on <监听器名> <地址:端口>
```

之后每一行状态变化时打印一行 `<行 id>: <状态>`（`running`、`waiting for its services`、`starting`、`failed: …`、`disabled`、`removed` 等；实例里的副本写成 `<行 id> in PluginId(n)`）。

本机的运行时（`node`、`py`、`bun`、Go 的 `go-<二进制>`）进程自己结束或起不来时打印一行，恢复运行时再打印一行；`rutis-host` 停止它们（停止宿主、Go 运行时空闲停下）时不打印：

| 输出 | 含义 |
| --- | --- |
| `runtime <名字>: down (<原因>)` | 运行时进程结束了，原因写它怎样结束，例如 `exited with signal: 9 (SIGKILL)`、`exited with exit status: 1`；或者它没能启动、原因是启动失败的错误。它的行回到 `waiting for its services`，其他运行时和它们的行不受影响 |
| `runtime <名字>: running` | 打印过 `down` 的运行时重新运行了（`dev` 里重新构建的 Go 运行时） |

一个运行时停在 `down` 时只打印一次；括号里的原因是给人看的，不保证措辞。

节点行（`rutis-bridge/peer`）的状态只说明这一行在运行；它的 link 连没连上另打一行，在 link 的状态变化时打印：

| 输出 | 含义 |
| --- | --- |
| `<行 id>: link connected` | 会话就绪，对端可用 |
| `<行 id>: link down (<原因>); retrying in 2.1s` | 连不上或连接断了（对端没起来、网络断开、会话结束），按退避在给出的时间后重试 |
| `<行 id>: link refused: auth-rejected (<原因>); retrying in 30s` | 被拒绝：token 不对、证书不受信任（名字不符、过期、CA 不认）、身份对不上；30 秒后再试 |
| `<行 id>: link stopped: incompatible (<原因>)` | 不兼容：协议、地址写法、对端缺少要求的能力（如 `runtime`）；不再重试，改配置后重新加载 |
| `<行 id>: link down (the session ended)` | 监听方：会话结束，等对端重新连进来（监听方不重试，不带 `retrying in`） |

- 类别（`auth-rejected`、`incompatible`）来自 link 对失败的分类，不靠错误文字；括号里的原因是给人看的细节，例如证书被拒时写明证书的问题，不保证措辞。
- 输出里不会出现 token 或其他凭据。
- 等待时间整秒写成 `30s`，否则保留一位小数。
- 状态每 200 毫秒检查一次，只打印变化：和上一行一样的不再打印（例如每 30 秒一次的同一个拒绝只打印一次）；持续不到 200 毫秒的状态可能不打印。拨号方还没有结果（正在连接）、监听方还没有对端连进来时不打印。

在 `emit` 里失败的监听器（发出方不等待，所以没有别人知道）在它运行的地方往 stderr 打一行：宿主自己的是 `rutis: listener of <name> in row <row> failed: <ErrorName>: <message>`，运行时的由运行时打同样的一行；宿主没能送达的调用写明去了哪里（`in runtime <name>`、`in node <peer>`）。`emit` 的过滤函数失败打 `rutis: filter of <name> from row <row> failed: …`（挂载方式加载的插件写作 `from the mount`）。

### 事件

所有行的监听器，不论什么语言、是否在相连的节点上，都在一个事件总线上，规则只有一套（[统一事件](../design-events-2026-10-18.md)）：`rutis.json` 里什么都不用声明。`instanced` 组的某个实例里的行，听得到这个实例里和它之上发出的事件，听不到别的实例的。`rutis-host` 还会发出下面这些事件，各语言的插件都可以监听：

| 事件 | 何时 |
| --- | --- |
| `rutis/row` | 行的每次状态变化：`{ row, plugin, scope, state: "pending" \| "running" \| "failed" \| "stopped", error }` |
| `rutis/runtime` | 运行时运行起来（`running`）或结束（`down`，带 `reason`） |
| `rutis/service` | 运行时的行提供的服务出现、被替换或撤走（`available`） |

rutis 0.12 及更早的运行时（或节点）连进来时被拒绝：`incompatible session: the far end (…) does not declare events-v2 (the event protocol of rutis 0.13): upgrade it to rutis 0.13 or later`。

## 检查配置

`check` 对每一行（被禁用的行除外）检查：

- 插件能找到并解析；
- 行的设置能通过加载前的检查（作用域、内置插件如节点行的配置）；
- 它等待的每个服务（插件的 `inject`，加上行自己的 `inject`）有别的行提供：按插件声明的 `provides`，节点行提供它 `import` 的服务。

一个服务没有任何行提供、也没有行可能在运行时提供时，这一行永远停在等待依赖：`check` 写明缺哪个服务、怎么处理，并以 1 退出；只有被禁用的行提供它时会指出是哪些行。有的行提供什么在检查时还不知道——别的机器上的运行时（检查时不连接）、`peer:` 行——这时缺的服务只提示可能来自它们，不算失败。

插件 `ctx.provide` 的服务不用声明就投到 rutis（0.12 起四种 SDK 写的插件都是，声明了 `provides` 的也可以多提供；没有声明 `rutis.provides` 的 Cordis 行也是），这些服务运行时才知道。一个服务没有行声明、但有这样的行时，`check` 在 stderr 写一条告警，仍以 0 退出：

```
rutis-host: warning: <行>: nothing declares <服务>; it may be provided at runtime by <行>, …
```

最多列三行，多的写成 `and N more`；这一行和可能提供它的行里有 Cordis 行时，后面再加 `; between Cordis rows of one runtime it resolves in Cordis`——同一个 Node 运行时里的 Cordis 行之间的依赖在 Cordis 里解析。声明了的名字照常算作提供者。

`check --strict` 把这些告警当作错误：stderr 写 `rutis-host: error: …`，这一行写 `cannot run with --strict: nothing declares <服务>`，以 1 退出。想在部署前确认每个依赖都有声明的提供者时用它（CI 里，比如）。

插件自己的配置由它的运行时在加载时校验，`check` 只打印它的 Schema。

## 停止与退出码

`run` 和 `dev` 收到终止信号时——Unix 上的 SIGINT（Ctrl-C）、SIGTERM、SIGHUP，Windows 上的 Ctrl-C、Ctrl-Break、关闭控制台——卸载所有行和运行时：每个插件的清理执行一次，运行时进程随后退出，`rutis-host` 再退出。

清理的总时长有截止时间，默认 10 秒，用 `--shutdown-timeout <秒>` 或环境变量 `RUTIS_SHUTDOWN_TIMEOUT` 修改（命令行优先，可以是小数，必须大于 0）。超过截止时间，或在停止过程中再收到一次信号（再按一次 Ctrl-C），`rutis-host` 打印还在停止的插件，结束运行时进程和它们启动的进程（不再等它们的清理），立即退出。启动失败时，已经起来的部分也按同样的规则关闭。

`rutis-host` 把截止时间传给它启动的运行时进程（环境变量 `RUTIS_SHUTDOWN_TIMEOUT`）。`rutis-host` 自己没来得及停止它们就结束了（`kill -9`、崩溃）时，每个运行时发现连接断开，在这个截止时间内自己卸载各行，然后退出，不管插件留下了什么；Windows 上它们随 `rutis-host` 一起结束（Job Object），清理不执行。

| 退出码 | 含义 |
| --- | --- |
| 0 | 正常结束，包括收到信号后所有清理都完成 |
| 1 | 不能启动或运行：参数或配置错误、运行时启动失败、行加载失败；`check` 有不能运行的行 |
| 2 | 停止没有完成：超过截止时间，或停止时再次收到信号；或者清理都完成了，但运行时进程没有按时退出（截止时间剩余的时间，至少 3 秒）。这些情况下 `rutis-host` 会结束运行时进程及它们启动的进程 |
| 3 | `restart` 为 `"exit"` 的本机运行时自己结束了，宿主已经停止（清理都完成）；见 [运行时结束时](#运行时结束时)。停止没有完成时是 2 |

2 和 1 分开，是为了让进程管理器和脚本区分“配置有问题”和“停止时有插件的清理没完成”；3 让它们知道是一个运行时崩溃了、应当重启宿主；它也不在 126 以上，那一段是 shell 用来表示“程序不能执行”和“被信号 n 结束”（128 + n）的。

Unix 上，`rutis-host` 启动时就被忽略的信号（比如用 `nohup` 启动时的 SIGHUP）继续被忽略。运行时进程各在自己的进程组里，终端的 Ctrl-C 只发给 `rutis-host`，由它卸载各行。Windows 上运行时进程各在自己的控制台进程组里，不响应 Ctrl-C；但 Ctrl-Break 和关闭控制台仍会发给它们，这时它们的清理可能来不及执行，所以停止时用 Ctrl-C。关闭控制台时，Windows 还会在几秒后结束 `rutis-host`，不管截止时间多长。

## 运行中改配置

`run` 运行时可以改 `rutis.json`，不用重启宿主：

- Unix 上给 `rutis-host` 发 SIGUSR1，它重新读一次文件。systemd 里写 `ExecReload=/bin/kill -USR1 $MAINPID`，`systemctl reload` 就会触发。SIGHUP 仍是终止信号（终端关闭时宿主随之停止），不用来重新读配置。
- 加上 `--watch-config`（默认关闭），文件内容变了就重新读：在内容变化后、连续 500 毫秒不再变化时读一次，内容和上次读到的相同时不读，文件暂时不存在（编辑器先删后建）不算变化。Windows 上没有 SIGUSR1，这是唯一的方式；Kubernetes 更新 ConfigMap 时改的是挂载的文件，也用它。

两种方式走同一个重新读取，全有或全无：

1. 文件读不出来、不是有效的配置：拒绝。
2. 运行中改不了的部分有变化：拒绝，提示重启宿主。它们是顶层的 `format`、`id`、`listen`、`runtimes.go`（任何变化）、`runtimes.remote`（增删条目，或改了条目的任何字段，包括 `listen`、`instances`），以及新增的或改了设置的本地运行时（`node`、`py`、`bun`）。
3. 先检查新的行（同 `check`）：有行起不来（插件找不到、行的设置通不过加载前的检查），而它现在并没有在失败，就拒绝。等待服务的行不算起不来，和启动时一样。
4. 再用新的行替换旧的：新增的行启动，删掉的行卸载，改了的行重启；没改的行不动（用到被重启的行所提供服务的行随它等待、恢复，和平时一样）。替换后有行失败（例如插件的 `apply` 抛错），而它之前没有失败，就换回旧的行（这时被换掉的行会重启一次），然后拒绝。
5. 删掉的本地运行时：它的行先卸载，再停止运行时，它的进程退出后才算完成。

`instanced` 分组里的行变化时，对已经存在的每个实例同样生效：新增的行在每个实例里启动，删掉的在每个实例里卸载，改了的在每个实例里重启；连接（实例）本身保留，实例里没改的行不动。之后新建的实例按新的行运行。

结果打印一行：

```
rutis-host: reloaded <文件> (<n> rows changed)
rutis-host: reloaded <文件> (1 row changed, runtime py stopped)
rutis-host: reload rejected: <原因>; nothing changed
```

被拒绝时旧的配置继续运行，原因写明是哪一行、什么问题，或者哪一项要重启宿主。行的状态变化照常由状态行打印。重新读取进行中又收到 SIGUSR1、文件又变了，等这一次结束后再读一次；进行中收到终止信号，宿主照常停止。

凭据在拨号和对端连进来时才从环境变量读（见 [凭据](#凭据)），所以重新读取新增的节点行也能连上。环境变量在进程运行期间不变，换 token 仍要重启宿主。

## 运行时结束时

本机运行时（`node`、`py`、`bun`、每个 Go 运行时）的进程自己结束（崩溃、被 `kill -9`、插件让进程退出）时，`rutis-host` 按这个运行时的 `restart` 处理：

| `restart` | 做法 |
| --- | --- |
| `"no"`（默认） | 不处理：它的行回到 `waiting for its services`，其他运行时和它们的行照常运行；要让它回来，重启宿主 |
| `"on-failure"` | 等一会儿后重新启动它，它的行随之恢复运行（重新加载，插件的 `apply` 再执行一次）；打印 `runtime <名字>: restarting`，起来后打印 `runtime <名字>: running` |
| `"exit"` | 卸载其他所有行（每个清理执行一次）后以 3 退出，由 systemd、Kubernetes 等重启整个宿主；stderr 打印 `rutis-host: runtime <名字> ended (<原因>), and its restart is "exit": stopping; …` |

`on-failure` 的等待和上限是固定的，不能配置：

- 第一次重启前等 1 秒，之后每次翻倍，最多 30 秒；
- 运行满 60 秒后才结束的，下一次重新从 1 秒开始；
- 5 分钟内已经重启过 5 次又结束时不再重启，打印 `runtime <名字>: gave up after 5 restarts within 5 minutes`，它留在 `down`，和 `"no"` 一样（启动即崩溃的插件因此最多重启 5 次）。

其他说明：

- 进程内存里的状态随崩溃丢失，重启不会多丢；正在进行的调用已经显式失败，不会自动重试。会再执行一次的是插件启动时做的事（`apply`），和手动重启宿主一样。
- 宿主停止时（收到终止信号、`exit` 策略）不再重启任何运行时；[运行中改配置](#运行中改配置)删掉的运行时也不会重启。
- `restart` 是运行时的设置，运行中改它要重启宿主。
- `dev` 用同样的规则和默认值；在 `rutis.dev.json` 里写 `"runtimes": { "py": { "restart": "on-failure" } }`（或 `"go"` 等）（项目自己的那个运行时只取 `restart`）。

- Go：`runtimes.go.restart` 对每个 Go 运行时生效，各算各的（各自的等待和上限；打印 `runtime go-<名字>: restarting`）。空闲停下的运行时不算自己结束；握手被拒（旧 SDK）的二进制也不算，文件改变前不再启动。不论 `restart` 怎么写，崩溃的 Go 运行时换了二进制后有行用到时也会再启动。运行中改 `runtimes.go` 仍要重启宿主。
- 远程运行时和节点行的 link 自己重连，不受它影响。

## 输出

运行时进程（Node、Bun、Python、Go）的标准输出和标准错误不直接写到 `rutis-host` 的输出上：`rutis-host` 经管道读出它们，一次一整行地写出——插件写到 stdout 的行出现在 `rutis-host` 的 stdout，写到 stderr 的出现在 stderr，内容原样，不加前缀。`rutis-host` 自己的状态行同样一次写一整行。所以插件的行和 `rutis-host` 的行、两个运行时的行不会拼到一起，即使插件把一行分几次写（Python 的 `print` 先写内容、再写换行），也即使 stdout 和 stderr 是同一个文件（`2>&1`）。

- 一行在它的换行到来时写出，没有按时间的刷新：不带换行的提示、用 `\r` 重画的进度条，要到换行时才出现。运行时结束时，没有换行的最后一行作为单独的一行写出。
- 超过 64 KiB 还没有换行的行分段写出。
- 同一个运行时进程里的插件共用这个进程的输出：两个插件在同一个进程里各写半行，仍会拼在一起。
- 运行时进程的 stdout、stderr 是管道而不是终端：按是否终端决定颜色等的库，按非终端输出。
- 管道一直有人读：输出很多的插件不会因为管道满了而停住；只有 `rutis-host` 自己的输出写不出去时（终端暂停、下游不读），它们才像以前一样等。
- `rutis-host` 退出前写完运行时剩下的输出。运行时启动的进程在运行时退出后还在运行时，它也拿着这份输出，`rutis-host` 最多再等它 2 秒，之后它写的输出不再出现。

## rutis.json

```json
{
  "format": 1,
  "id": "main",
  "runtimes": {
    "node": { "project": "." },
    "py": { "project": "plugins", "python": ".venv/bin/python" },
    "go": { "dir": "plugins/go" },
    "remote": [{ "name": "gpu", "language": "python" }]
  },
  "listen": [
    { "name": "public", "address": "0.0.0.0:7443", "cert": "tls/server.pem", "key": "tls/server.key" }
  ],
  "rows": [
    { "id": "weather", "name": "weather-plugin", "config": { "city": "Oslo" } },
    { "id": "llm", "name": "py:llm_gateway" },
    { "id": "dns", "name": "go:dns" },
    { "id": "local-tool", "name": "./tools/tool.ts" },
    { "id": "office", "name": "rutis-bridge/peer", "config": { "peer": "office", "dial": "wss://office.example.com/rutis", "export": ["weather"] } },
    { "id": "gpu", "name": "rutis-bridge/peer", "config": { "peer": "gpu", "dial": "wss://gpu.example.com/rutis", "runtime": "gpu" } },
    { "id": "embedder", "name": "gpu:embedder" }
  ]
}
```

文件里的相对路径都相对于文件所在的目录。

不认识的字段是错误：启动失败，指出文件、行列号和这里可用的字段。`rows` 里不认识的字段是警告：这一行照常运行（不带这个字段），启动前打印警告，指出文件、第几行和行可用的字段。拼错的字段因此不会被悄悄忽略。

### format

文件的格式版本，目前是 `1`。可以省略，省略按 `1` 读（0.10 之前的文件都没有这个字段，照常能读）；`rutis-host new` 生成的 `rutis.dev.json` 写上了它。比这个 `rutis-host` 支持的版本新的文件不会被读，启动前报错，例如：

```text
rutis.json: this file is format 2, this rutis-host reads up to 1: upgrade rutis-host
```

这时升级 `rutis-host`。格式版本只在 `rutis.json` 的含义变化时增加，并写在发布说明里。

### id

这个节点的端点 id，连接到它的节点看到的名字。默认 `host`。

### runtimes

| 键 | 字段 | 作用 |
| --- | --- | --- |
| `node` | `project`（默认 `.`）、`runtime`、`restart` | 启动 Node 运行时。插件包从 `project` 的 `package.json` 解析；`@arcships/rutis-runtime` 装在这里（或由 `runtime` 指定，或用 npm 版 rutis-host 自带的） |
| `bun` | `project`（默认 `.`）、`runtime`、`program`、`restart` | 启动 Bun 运行时，行名 `bun:<模块>`。模块从 `project` 解析（`node_modules` 里的包、`./相对路径`）；`@arcships/rutis-bun` 装在这里（或由 `runtime` 指定）；`program` 是 Bun 可执行文件，默认 `PATH` 上的 `bun`。见 [写一个 Bun 插件](bun-plugin.md) |
| `py` | `project`（默认 `.`）、`python`、`restart` | 启动 Python 运行时。解释器默认依次为 `$VIRTUAL_ENV/bin/python`、`<project>/.venv/bin/python`、`python3`（Windows 上为 `Scripts\python.exe` 和 `python`），它的环境里要装 `rutis` 和插件 |
| `go` | `dir`、`binaries`、`start`（`on-demand` 默认 / `eager`）、`idle`（秒，默认 60）、`project`（默认 `.`）、`restart` | Go 插件：一个二进制一个运行时进程。`dir` 里含 Go SDK 标记的可执行文件都算（整个目录被视为受信任，应是专用目录），`binaries` 列出单个文件。运行时名由文件名得出（`netkit` → `go-netkit`）。按需启动的运行时在第一次有行用到时启动、空闲 `idle` 秒后停下；`eager` 全部启动并一直运行（不能同时写 `idle`）。换了二进制，重启宿主后生效 |
| `remote` | `name`、`language`、`listen`、`instances` | 别的机器上的运行时：经 `rows` 里 `"runtime": "<name>"` 的节点行连接，见 [连接节点](nodes.md)；或写 `listen` 与 `instances`，由运行时连进来，见下 |

缺少运行时包时，启动失败并给出安装命令。

`node`、`py`、`bun`、`go` 的 `restart`（`"no"` 默认、`"on-failure"`、`"exit"`）决定运行时进程自己结束时怎么办（`go` 是每个 Go 运行时各自的），见 [运行时结束时](#运行时结束时)。

#### 连进来的运行时

`remote` 的条目写上 `listen`（`listen` 里一个监听器的名字）和 `instances`（一个顶层的 `instanced` 分组的 id），这个运行时就不由 rutis 去连，而是由运行时连到这个监听器；可以有任意多条连接，每条连接是分组的一个实例：

```json
{
  "listen": [{ "name": "public", "address": "0.0.0.0:7443", "cert": "server.pem", "key": "server.key" }],
  "runtimes": {
    "py": { "project": "." },
    "remote": [{ "name": "phone", "language": "python", "listen": "public", "instances": "phone" }]
  },
  "rows": [
    { "id": "phone", "group": true, "instanced": true, "config": [
      { "id": "contacts", "name": "phone:contacts" },
      { "id": "digest", "name": "py:digest" }
    ] }
  ]
}
```

- 运行时连接时出示 token `RUTIS_TOKEN_<NAME>`（运行时名大写，`-` 写作 `_`；没有时用 `RUTIS_TOKEN`），并自报一个 id（`[a-z0-9-]`）。token 不对时拒绝连接；id 只用来区分连接，不作凭据。
- 每条连接是分组的一个实例：连进来时创建，断开时关闭。同一个 id 的新连接接管旧连接：旧实例关闭后再创建新实例。
- 分组里的 `<name>:<插件>` 行在每个实例里运行在该实例的连接上。连接里没有这个插件时，这一行只在这个实例里失败（`this connection does not contain plugin <插件>`），不影响其他实例；新增这样的行也不会因此被拒绝。
- 连接上的插件提供的服务是它所在实例自己的：多条连接带同名插件互不冲突，同一实例里的行（任何语言）按名字用到的是本连接的服务。实例外的行用不到它们，需要时把使用者放进这个分组。
- 不需要另写节点行。`listen` 与 `instances` 要同时写；监听器、分组不存在，分组不是 `instanced`，或者没有 token，启动时报错。
- `<name>:` 行只能放在这个分组里；放在别处时这一行失败。

Python 运行时用 `python -m rutis dial:wss://… --id <id> --peer <本宿主的 id> <项目>` 连进来，Go 插件二进制用 `<二进制> dial:wss://… --id <id> --peer <本宿主的 id> <项目>`，Node、Bun 运行时同样以 `dial:` 连进来；命令见 [连接节点](nodes.md#连进来的运行时)。

### listen

别的节点连进来用的 WebSocket 监听器。`cert` / `key` 是 TLS 证书和私钥（PEM），也可以由 `RUTIS_CERT` / `RUTIS_KEY` 给出（文件里写的优先）。两者要成对：只有其中一个时启动失败，而不是退回不带 TLS。不带 TLS 时只能监听回环地址。

### rows

要运行的插件，每一行：

| 字段 | 含义 |
| --- | --- |
| `id` | 行的名字，在本文件内唯一 |
| `name` | 插件：npm 包名（或包的子路径）、`py:<名字>`（Python 入口点或模块）、`bun:<模块>`（Bun 运行时里的模块）、`go:<插件>`（含这个插件的 Go 二进制；两个二进制都有时写 `go-<二进制名>:<插件>`）、`./相对路径`（插件文件）、`<远程运行时>:<模块或插件>`、`rutis-bridge/peer`（节点，见 [连接节点](nodes.md)）、`peer:<节点>/<插件>`（在别的节点上运行的插件） |
| `config` | 插件的配置 |
| `inject` | 额外要等的服务名 |
| `isolate` | `{ "服务名": true }` 让这一行用自己的那份服务；`{ "服务名": "标签" }` 让同标签的行共用一份 |
| `disabled` | `true` 时不运行 |

服务按名字在所有插件之间共享：任何语言、任何节点上提供的 `weather`，所有声明了 `inject: ["weather"]` 的插件都能用。

## 凭据

凭据不写在文件里，在拨号和对端连进来时从环境变量读：

| 环境变量 | 作用 |
| --- | --- |
| `RUTIS_TOKEN` | 连接其他节点时出示的、以及接受其他节点连入时核对的 token |
| `RUTIS_TOKEN_<节点>` | 只对某个节点（大写，`-` 写作 `_`），优先于 `RUTIS_TOKEN`；连进来的运行时（`listen` / `instances`）按运行时名，所有连接共用这一个 token |
| `RUTIS_CA` | 额外信任的 CA 证书（PEM），用于 `wss://` 连接 |
| `RUTIS_CERT` / `RUTIS_KEY` | 监听器的证书和私钥（`listen` 里没有写时） |

环境变量在进程外改不了，所以这些 token 运行中不能吊销：要切断一个对端或连进来的运行时，换掉 token 后重启 `rutis-host`，重启会断开所有连接。运行中只切断一个身份要用身份层的吊销钩子，见 [连接节点](nodes.md#安全)。

## 部署

把 `rutis.json`、Node 项目（装好插件和 `@arcships/rutis-runtime`）、Python 环境（装好 `rutis` 和插件）、Go 插件目录（对应平台的二进制）放在一起，用进程管理器（systemd 等）运行 `rutis-host run /srv/app/rutis.json`。`rutis-host` 结束时，它启动的运行时进程随之结束。

改了 `rutis.json` 要生效时用 `systemctl reload`（`ExecReload=/bin/kill -USR1 $MAINPID`），或者用 `--watch-config` 运行，见 [运行中改配置](#运行中改配置)。

停止时让进程管理器只给 `rutis-host` 发 SIGTERM，由它卸载各行：systemd 默认（`KillMode=control-group`）会同时给运行时进程发 SIGTERM，它们来不及执行清理，应设 `KillMode=mixed`，并让 `TimeoutStopSec` 大于清理截止时间。`rutis-host` 被强制结束（SIGKILL、Windows 上结束进程）时没有机会卸载各行：Windows 上运行时进程随之被结束；Unix 上运行时进程发现通道断开，自己卸载各行后退出。
