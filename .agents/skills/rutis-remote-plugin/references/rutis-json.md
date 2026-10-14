> 摘自 rutis 仓库 `docs/guide/rutis-host.md`（main，2026-10-14），随 skill 一起分发以便独立使用；以原文为准：https://github.com/arcships/rutis/blob/main/docs/guide/rutis-host.md

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
| `bun` | `project`（默认 `.`）、`runtime`、`program` | 启动 Bun 运行时，行名 `bun:<模块>`。模块从 `project` 解析（`node_modules` 里的包、`./相对路径`）；`@arcships/rutis-bun` 装在这里（或由 `runtime` 指定）；`program` 是 Bun 可执行文件，默认 `PATH` 上的 `bun`。见 [写一个 Bun 插件](https://github.com/arcships/rutis/blob/main/docs/guide/bun-plugin.md) |
| `py` | `project`（默认 `.`）、`python` | 启动 Python 运行时。解释器默认依次为 `$VIRTUAL_ENV/bin/python`、`<project>/.venv/bin/python`、`python3`（Windows 上为 `Scripts\python.exe` 和 `python`），它的环境里要装 `rutis` 和插件 |
| `go` | `dir`、`binaries`、`start`（`on-demand` 默认 / `eager`）、`idle`（秒，默认 60）、`project`（默认 `.`） | Go 插件：一个二进制一个运行时进程。`dir` 里含 Go SDK 标记的可执行文件都算（整个目录被视为受信任，应是专用目录），`binaries` 列出单个文件。运行时名由文件名得出（`netkit` → `go-netkit`）。按需启动的运行时在第一次有行用到时启动、空闲 `idle` 秒后停下；`eager` 全部启动并一直运行（不能同时写 `idle`）。换了二进制，重启宿主后生效 |
| `remote` | `name`、`language` | 别的机器上的运行时，经 `rows` 里 `"runtime": "<name>"` 的节点行连接，见 [连接节点](https://github.com/arcships/rutis/blob/main/docs/guide/nodes.md) |

缺少运行时包时，启动失败并给出安装命令。

### listen

别的节点连进来用的 WebSocket 监听器。`cert` / `key` 是 TLS 证书和私钥（PEM），也可以由 `RUTIS_CERT` / `RUTIS_KEY` 给出（文件里写的优先）。两者要成对：只有其中一个时启动失败，而不是退回不带 TLS。不带 TLS 时只能监听回环地址。

### rows

要运行的插件，每一行：

| 字段 | 含义 |
| --- | --- |
| `id` | 行的名字，在本文件内唯一 |
| `name` | 插件：npm 包名（或包的子路径）、`py:<名字>`（Python 入口点或模块）、`bun:<模块>`（Bun 运行时里的模块）、`go:<插件>`（含这个插件的 Go 二进制；两个二进制都有时写 `go-<二进制名>:<插件>`）、`./相对路径`（插件文件）、`<远程运行时>:<模块或插件>`、`rutis-bridge/peer`（节点，见 [连接节点](https://github.com/arcships/rutis/blob/main/docs/guide/nodes.md)）、`peer:<节点>/<插件>`（在别的节点上运行的插件） |
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
