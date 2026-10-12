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
| `rutis-host run [rutis.json]` | 按配置运行，打印每一行状态的变化；Ctrl-C 结束（见 [停止与退出码](#停止与退出码)） |
| `rutis-host dev [目录]` | 在插件项目里运行这个插件（加上 `rutis.dev.json`），文件变化时重新加载；Go 项目重新构建，只重启它的运行时 |
| `rutis-host check [rutis.json]` | 不运行任何插件，检查每一行能不能运行（见 [检查配置](#检查配置)），打印版本、依赖、提供的服务、配置 Schema，并列出每个 Go 二进制（运行时名、SDK、插件 API、插件）；有不能运行的行或二进制时以非零状态退出。在没有 rutis.json 的插件项目里检查这个项目 |
| `rutis-host new <名字> --lang node\|bun\|python\|go` | 创建插件项目 |
| `rutis-host go add <模块>@<版本> [rutis.json]` | 用本机的 Go 工具链把插件二进制 `go install` 到 `runtimes.go.dir` |

## 检查配置

`check` 对每一行（被禁用的行除外）检查：

- 插件能找到并解析；
- 行的设置能通过加载前的检查（作用域、内置插件如节点行的配置）；
- 它等待的每个服务（插件的 `inject`，加上行自己的 `inject`）有别的行提供：按插件声明的 `provides`，节点行提供它 `import` 的服务。

一个服务没有任何行提供时，这一行永远停在等待依赖：`check` 写明缺哪个服务、怎么处理，并以 1 退出；只有被禁用的行提供它时会指出是哪些行。有的行提供什么在检查时还不知道——别的机器上的运行时（检查时不连接）、`peer:` 行——这时缺的服务只提示可能来自它们，不算失败。

插件自己的配置由它的运行时在加载时校验，`check` 只打印它的 Schema。

## 停止与退出码

`run` 和 `dev` 收到终止信号时——Unix 上的 SIGINT（Ctrl-C）、SIGTERM、SIGHUP，Windows 上的 Ctrl-C、Ctrl-Break、关闭控制台——卸载所有行和运行时：每个插件的清理执行一次，运行时进程随后退出，`rutis-host` 再退出。

清理的总时长有截止时间，默认 10 秒，用 `--shutdown-timeout <秒>` 或环境变量 `RUTIS_SHUTDOWN_TIMEOUT` 修改（命令行优先，可以是小数，必须大于 0）。超过截止时间，或在停止过程中再收到一次信号（再按一次 Ctrl-C），`rutis-host` 打印还在停止的插件，结束运行时进程和它们启动的进程（不再等它们的清理），立即退出。启动失败时，已经起来的部分也按同样的规则关闭。

| 退出码 | 含义 |
| --- | --- |
| 0 | 正常结束，包括收到信号后所有清理都完成 |
| 1 | 不能启动或运行：参数或配置错误、运行时启动失败、行加载失败；`check` 有不能运行的行 |
| 2 | 停止没有完成：超过截止时间，或停止时再次收到信号；或者清理都完成了，但运行时进程没有按时退出（截止时间剩余的时间，至少 3 秒）。这些情况下 `rutis-host` 会结束运行时进程及它们启动的进程 |

2 和 1 分开，是为了让进程管理器和脚本区分“配置有问题”和“停止时有插件的清理没完成”；它也不在 126 以上，那一段是 shell 用来表示“程序不能执行”和“被信号 n 结束”（128 + n）的。

Unix 上，`rutis-host` 启动时就被忽略的信号（比如用 `nohup` 启动时的 SIGHUP）继续被忽略。运行时进程各在自己的进程组里，终端的 Ctrl-C 只发给 `rutis-host`，由它卸载各行。Windows 上运行时进程各在自己的控制台进程组里，不响应 Ctrl-C；但 Ctrl-Break 和关闭控制台仍会发给它们，这时它们的清理可能来不及执行，所以停止时用 Ctrl-C。关闭控制台时，Windows 还会在几秒后结束 `rutis-host`，不管截止时间多长。

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
| `node` | `project`（默认 `.`）、`runtime` | 启动 Node 运行时。插件包从 `project` 的 `package.json` 解析；`@arcships/rutis-runtime` 装在这里（或由 `runtime` 指定，或用 npm 版 rutis-host 自带的） |
| `bun` | `project`（默认 `.`）、`runtime`、`program` | 启动 Bun 运行时，行名 `bun:<模块>`。模块从 `project` 解析（`node_modules` 里的包、`./相对路径`）；`@arcships/rutis-bun` 装在这里（或由 `runtime` 指定）；`program` 是 Bun 可执行文件，默认 `PATH` 上的 `bun`。见 [写一个 Bun 插件](bun-plugin.md) |
| `py` | `project`（默认 `.`）、`python` | 启动 Python 运行时。解释器默认依次为 `$VIRTUAL_ENV/bin/python`、`<project>/.venv/bin/python`、`python3`（Windows 上为 `Scripts\python.exe` 和 `python`），它的环境里要装 `rutis` 和插件 |
| `go` | `dir`、`binaries`、`start`（`on-demand` 默认 / `eager`）、`idle`（秒，默认 60）、`project`（默认 `.`） | Go 插件：一个二进制一个运行时进程。`dir` 里含 Go SDK 标记的可执行文件都算（整个目录被视为受信任，应是专用目录），`binaries` 列出单个文件。运行时名由文件名得出（`netkit` → `go-netkit`）。按需启动的运行时在第一次有行用到时启动、空闲 `idle` 秒后停下；`eager` 全部启动并一直运行（不能同时写 `idle`）。换了二进制，重启宿主后生效 |
| `remote` | `name`、`language` | 别的机器上的运行时，经 `rows` 里 `"runtime": "<name>"` 的节点行连接，见 [连接节点](nodes.md) |

缺少运行时包时，启动失败并给出安装命令。

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

凭据不写在文件里：

| 环境变量 | 作用 |
| --- | --- |
| `RUTIS_TOKEN` | 连接其他节点时出示的、以及接受其他节点连入时核对的 token |
| `RUTIS_TOKEN_<节点>` | 只对某个节点（大写，`-` 写作 `_`），优先于 `RUTIS_TOKEN` |
| `RUTIS_CA` | 额外信任的 CA 证书（PEM），用于 `wss://` 连接 |
| `RUTIS_CERT` / `RUTIS_KEY` | 监听器的证书和私钥（`listen` 里没有写时） |

## 部署

把 `rutis.json`、Node 项目（装好插件和 `@arcships/rutis-runtime`）、Python 环境（装好 `rutis` 和插件）、Go 插件目录（对应平台的二进制）放在一起，用进程管理器（systemd 等）运行 `rutis-host run /srv/app/rutis.json`。`rutis-host` 结束时，它启动的运行时进程随之结束。

停止时让进程管理器只给 `rutis-host` 发 SIGTERM，由它卸载各行：systemd 默认（`KillMode=control-group`）会同时给运行时进程发 SIGTERM，它们来不及执行清理，应设 `KillMode=mixed`，并让 `TimeoutStopSec` 大于清理截止时间。`rutis-host` 被强制结束（SIGKILL、Windows 上结束进程）时没有机会卸载各行：Windows 上运行时进程随之被结束；Unix 上运行时进程发现通道断开，自己卸载各行后退出。
