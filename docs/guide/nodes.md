# 连接节点

一个 rutis 宿主是一个**节点**。节点之间用 **link** 连接：一端拨号（`dial`），一端监听（`listen`），连上后在这条连接上共享服务、代为运行插件、共享事件。断开后自动重连。

下面用 `rutis.json` 的行来写；在 Rust 里用 `rutis_bridge::PeerPlugin` 做同样的事（见 [在 Rust 应用里嵌入](rust-host.md)）。

## 一条 link

```json
{ "id": "office", "name": "rutis-bridge/peer", "config": { "peer": "office", "dial": "wss://office.example.com/rutis" } }
```

另一端（`office`）监听：

```json
{
  "id": "office",
  "listen": [{ "name": "public", "address": "0.0.0.0:7443", "cert": "server.pem", "key": "server.key" }],
  "rows": [
    { "id": "main", "name": "rutis-bridge/peer", "config": { "peer": "main", "listen": "public" } }
  ]
}
```

两端用同一个 token：拨号方（`main`）出示 `RUTIS_TOKEN_OFFICE`，监听方（`office`）用 `RUTIS_TOKEN_MAIN` 核对来者是 `main`，两个变量设成同一个值。

- `dial` 是 `wss://…`（跨机器，必须 TLS），或 `ws://127.0.0.1…`（本机回环）。
- 自签证书：把签发它的 CA 放进拨号方的 `RUTIS_CA`。
- 断开后重连：从 0.5 秒开始退避，最长 30 秒；凭据被拒（403、证书不受信任）时每 30 秒重试一次；协议不兼容时停止并报告。
- `rutis-host run` 在 link 的状态变化时打印一行，带可读的类别，不带 token；节点行自己的 `running` 只说明这一行在运行，不说明连上了：

  ```
  office: link connected
  office: link down (127.0.0.1:7443: Connection refused (os error 61)); retrying in 2.1s
  office: link refused: auth-rejected (office.example.com: upgrade refused with 403 Forbidden); retrying in 30s
  office: link stopped: incompatible (office does not declare runtime)
  ```

  `auth-rejected` 是凭据被拒（token 不对、证书名字不符或过期、CA 不受信任），`incompatible` 是不兼容（不再重试），`link down` 是暂时连不上。完整格式见 [rutis-host 打印什么](rutis-host.md#run-打印什么)。
- 监听方的 link 还没就绪时连进来的拨号方会被要求重试，所以两端可以按任意顺序启动。

## 在 link 上做什么

节点行的 `config` 决定这条连接上做什么。`export`、`import` 改了不会断开连接，只重启对应的那部分。

| 字段 | 作用 |
| --- | --- |
| `export: ["weather", "session/*"]` | 把本节点的服务 `weather` 给对端用；本节点发出的 `weather` 和所有 `session/…` 事件到达对端的监听器 |
| `import: ["calendar", "tock"]` | 把对端的服务 `calendar` 作为本节点的服务（本节点已有同名服务时拒绝）；本节点 `calendar`、`tock` 的监听器听到对端发出的这两个事件 |
| `host: true` | 允许对端在本节点上运行这里装着的插件（只对可信的对端打开） |
| `rows: true` | 本文件里 `peer:<对端>/<插件>` 的行在对端运行 |
| `runtime: "<名字>"` | 对端是一个语言运行时，见下文 |

服务跨节点时调用链保持：同步调用期间的回调、回调里再调回去，都能穿过多个节点。

### 节点之间的事件

`export` / `import` 的一项是名字，或以 `*` 结尾的前缀（`session/*`；单独的 `*` 是所有名字）；前缀只选事件，服务仍按确切名字。同一个名字可以双向共享。对端的 `import` 和本节点的 `export` 对上时，对端的监听器就是本节点事件总线上的条目，规则和本机一样：五种方式（`emit`、`parallel`、`serial`、`waterfall`、`bail`）、过滤、所有值类型；连接断开时它们离开总线（进行中的调用失败一次）。事件只走一跳：一个节点导入的东西不再往外共享。对端的监听器是这条总线上的条目；它们的描述里带这个节点行所在的作用域（实例之外的连接就是根），供过滤使用。完整模型见 [统一事件](../design-events-2026-10-18.md)。

例：`main` 上发出的 `session/…` 事件到达 `office` 上任何语言的监听器，`office` 上发出的 `audit/flagged` 到达 `main` 的监听器：

```json
{ "id": "office", "name": "rutis-bridge/peer", "config": { "peer": "office", "dial": "wss://office.example.com/rutis", "export": ["session/*"], "import": ["audit/flagged"] } }
```

```json
{ "id": "main", "name": "rutis-bridge/peer", "config": { "peer": "main", "listen": "public", "import": ["session/*"], "export": ["audit/flagged"] } }
```

一个名字要在发出的一端 `export`、在监听的一端 `import`，两边都写了才共享。`main` 上的 `await ctx.serial("session/quota", user)` 会按登记顺序逐个问 `main` 和 `office` 上的监听器，返回第一个结果；`office` 断开时，它的监听器离开总线，之后的分发只问 `main` 上的。

运行时（本机的、[别的机器上的](#别的机器上的语言运行时)、[连进来的](#连进来的运行时)）不是节点连接：它们的监听器直接在宿主的总线上，不需要 `export` / `import`。

`events: { "out": […], "in": […] }` 在 rutis 0.13 删除，写了会报错：`out` 写进 `export`，`in` 写进 `import`。0.12 及更早的节点连进来时被拒绝（`link stopped: incompatible (…)`），见 [迁移到 0.13](../migration-0.13.md)。

## 在别的节点上运行插件

对端开了 `host: true`，本端开了 `rows: true` 时，`peer:<对端>/<插件>` 的行在对端加载那个插件：

```json
{ "id": "office", "name": "rutis-bridge/peer", "config": { "peer": "office", "dial": "wss://office.example.com/rutis", "rows": true } },
{ "id": "scanner", "name": "peer:office/scanner-plugin", "config": { "dpi": 300 } }
```

插件在对端运行，用对端的服务；它的配置、启停仍由本端决定。对端离线时这一行等待，回来后重新加载。行的 `isolate` / `inject` 交给对端执行。

## 别的机器上的语言运行时

在 GPU 机器上只运行一个 Python 运行时，由主节点管理它的插件：

```bash
# GPU 机器：装好 rutis[network] 和插件，监听
RUTIS_TOKEN=secret RUTIS_CERT=server.pem RUTIS_KEY=server.key \
  python -m rutis listen:wss://0.0.0.0:7443/rutis --id gpu --peer main /srv/plugins
```

```json
{
  "id": "main",
  "runtimes": { "remote": [{ "name": "gpu", "language": "python", "dial": "wss://gpu.example.com:7443/rutis" }] },
  "rows": [
    { "id": "embedder", "name": "gpu:embedder" }
  ]
}
```

`runtimes.remote` 的一条写全一个远程运行时，谁主动连、一台还是多台各由一个字段决定：

| 写法 | 含义 |
| --- | --- |
| `dial: <地址>` | 一台，rutis 去连；对方的 `--id` 是运行时名 |
| `listen: <监听器>` | 一台，等它连进来；对方的 `--id` 是运行时名 |
| `listen` + `instances: <分组>` | 多台，每条连接一个实例（见[连进来的运行时](#连进来的运行时)） |
| 都不写 | 不起连接，由一条带 `runtime` 的节点行或嵌入的程序提供会话 |

`dial` 与 `listen` 不能同时写，`dial` 与 `instances` 也不能（rutis 去连的只能是一台）。token 是 `RUTIS_TOKEN_<名字>`。连接走 WebSocket。

0.19 之前的写法——`runtimes.remote` 只写 `name` 与 `language`，另写一条带 `"runtime": "gpu"` 的 `rutis-bridge/peer` 行——照常可用（见[迁移到 0.19](../migration-0.19.md)）。

`run` 打印连接的变化，前缀是 `runtime <名>`：

```text
runtime gpu: link connected
runtime gpu: link down (gpu.example.com:7443: Connection refused); retrying in 2.1s
runtime worker: connected worker-1
runtime worker: disconnected worker-1 (connection lost)
runtime worker: refused (the worker runtime lacks rows.v2 and hosts: …)
```

`rutis-host status` 的 `connections:` 一节给出当前状态。远端不含某行的插件时，只有这一行失败（`runtime gpu: this connection does not contain plugin embedder`），其他行不受影响。

远程 Python 运行时的行名是 `<运行时名>:<模块>`，模块在远程机器上解析。Node 运行时同样可以远程运行：在那台机器的 Node 项目里执行 `node node_modules/@arcships/rutis-runtime/src/runner.mjs listen:wss://… --id <名字> --peer <控制方> ./package.json`（插件从这个 package.json 解析），`language` 写 `node`；Bun 运行时也一样：在那台机器的 Bun 项目里执行 `bun --no-install --no-env-file node_modules/@arcships/rutis-bun/src/main.ts listen:wss://… --id <名字> --peer <控制方> .`（插件从这个目录解析，行名 `<运行时名>:<模块>`），`language` 写 `bun`。远程运行时一次服务一个控制方；控制方断开时，它为这个控制方运行的插件全部卸载。每种远程运行时（Python、Node、Bun、Go 或 Swift 二进制）开始监听后都在 stderr 上打印一行实际绑定的地址：`rutis: listening on <地址>`。端口写 0（`listen:ws://127.0.0.1:0/rutis`）时由系统挑一个空闲端口，脚本和测试从这一行读出来；不要先找一个空闲端口再传给它，运行时绑定之前别的进程可能已经占了这个端口。Python 运行时会话结束时还打印一行 `rutis: session ended (<原因>); waiting for a controller`（原因如 `heartbeat timeout`、`connection lost`、`closed (1001): …`），被新连接接替时打印 `rutis: session ended (replaced by a new connection)`。

### 连进来的运行时

运行时也可以反过来连到 rutis：远程机器在 NAT 后、地址会变，或者数量会变（多台设备、扩缩容的 worker）时用这种方式。只有一台时在 `runtimes.remote` 的条目里只写 `listen`（接入的监听器），运行时的 `--id` 写运行时名；数量会变时再加 `instances`（一个 `instanced` 分组）。都不写节点行：

```json
{
  "id": "main",
  "listen": [{ "name": "public", "address": "0.0.0.0:7443", "cert": "server.pem", "key": "server.key" }],
  "runtimes": { "remote": [{ "name": "worker", "language": "python", "listen": "public", "instances": "worker" }] },
  "rows": [
    { "id": "worker", "group": true, "instanced": true, "config": [
      { "id": "embedder", "name": "worker:embedder" }
    ] }
  ]
}
```

每条连接出示同一个 token（rutis 一侧是 `RUTIS_TOKEN_WORKER`），自报一个 id，成为分组的一个实例：连进来时创建，断开时关闭，同一 id 的新连接接管旧连接。分组里的行在每个实例中运行在该实例的连接上，它们提供的服务只在本实例里可见。没有声明就提供的服务（自动投到 rutis 的）也一样：只有本实例里的行看得到，实例外的行和别的实例都看不到。字段与规则见 [rutis-host](rutis-host.md#连进来的运行时)。

Python 运行时用 `dial:` 连进来：

```bash
# worker 机器：装好 rutis[network] 和插件，连到 rutis
RUTIS_TOKEN=secret RUTIS_CA=ca.pem \
  python -m rutis dial:wss://main.example.com:7443/rutis --id worker-1 --peer main /srv/plugins
```

Go 或 Swift 插件二进制接受同样的 `dial:` 通道（以及 `listen:`），参数、环境变量、重连规则和状态行都与 Python 相同（`language` 写 `go` 或 `swift`）：

```bash
# worker 机器：插件二进制在这里，连到 rutis
RUTIS_TOKEN=secret RUTIS_CA=ca.pem \
  ./weather dial:wss://main.example.com:7443/rutis --id worker-1 --peer main .
```

- `--id` 是这条连接自报的 id（小写字母、数字、`-`），即它的实例名；`--peer` 是 rutis 的 `id`。
- `RUTIS_TOKEN` 与 rutis 一侧的 `RUTIS_TOKEN_WORKER` 相同；`RUTIS_CA` 是额外信任的 CA（自签证书时用），系统根证书照常信任。非回环地址只能用 `wss://`。
- 运行时自己保持连接，按固定规则重连：连不上、连接断开时从 0.5 秒起翻倍等待，最长 30 秒，连接持续 60 秒后重新从 0.5 秒算；token 或 id 被拒绝（升级时 401 / 403，或 rutis 以关闭码 4003 拒绝这个 id）每 30 秒再试；协议不兼容（升级时 400 等）不再重试，进程以状态 1 退出。每次断开都先卸载全部行（清理逆序执行），重连是新的会话，行重新装载。
- 状态写在 stderr：`rutis: connected to …`、`rutis: connection lost (…); retrying in 1s`、`rutis: refused: auth-rejected (…); retrying in 30s`、`rutis: refused: incompatible (…); stopping`。
- SIGTERM 或 Ctrl-C 停下它：卸载全部行，关闭连接，不再重连。
- 两个进程用同一个 id 会互相接管，按各自的退避轮流把对方顶掉；每台机器、每个 worker 用不同的 id。

Node、Bun 运行时用同样的参数、环境变量和规则拨出，`runtimes.remote` 的 `language` 分别写 `node`、`bun`：

```bash
# Node 项目里（插件从 package.json 解析）
RUTIS_TOKEN=secret RUTIS_CA=ca.pem \
  node node_modules/@arcships/rutis-runtime/src/runner.mjs dial:wss://main.example.com:7443/rutis --id worker-1 --peer main ./package.json
# Bun 项目里（插件从这个目录解析）
RUTIS_TOKEN=secret RUTIS_CA=ca.pem \
  bun --no-install --no-env-file node_modules/@arcships/rutis-bun/src/main.ts dial:wss://main.example.com:7443/rutis --id worker-1 --peer main .
```

Node、Bun 的每个会话由一个子进程运行：断开时子进程卸载全部行后退出，重连起新的子进程。`RUTIS_CA` 与 Node、Bun 默认信任的根证书一起信任。

Kotlin 插件一般编进 Android 应用，由应用拨出到 rutis（[Android 应用托管](../design-android-runtime-2026-10-12.md) §五）。SDK 的 jvm target 的 `Rutis.serve` 接受同样的 `dial:`、`listen:` 通道、环境变量和规则（监听 `wss://` 用 `RUTIS_CERT`、`RUTIS_KEY`），`language` 写 `kotlin`（或 `kt`）。

## 把 Cordis 应用接成节点

已有的 Cordis 应用用 `@arcships/rutis-runtime/bridge` 成为一个节点，见 [Cordis](cordis.md)。

## 自建连接

自己的连接方式（中继、蓝牙、别的传输）写成一个连接插件：拿到一条能收发二进制消息的连接，交出去成为一个会话。Rust 里交出 `Channel`：`ctx.plugin(rutis_bridge::Session::new(channel))`（见 [写 Rust 宿主：节点](rust-host.md#节点)）；其他语言调服务 `sessions` 的 `open`（见 [插件 API：自建连接](plugin-api.md#自建连接sessionsopen)）。对面的插件在这里叫 `<对面 id>:<插件>`，要让对面运行一个插件就写行 `<连接行>:<插件>`。

## 安全

- 跨机器只用 `wss://`。不带 TLS 的监听器只能绑定回环地址。
- Python 运行时监听 `wss://` 不能用 CPython 3.13.6：它的 TLS socket 在一个线程阻塞读、另一个线程写时死锁（[python/cpython#137583](https://github.com/python/cpython/issues/137583)），每个握手都会卡住，所以 rutis 在这个版本上拒绝启动 `listen:wss://`。用 3.13.7 或更新的版本（3.13.5 及更早、3.14 不受影响）；`ws://` 不受影响。拨出（`dial:wss://`）由一个线程完成连接的全部读写，不受影响。
- 每个对端一个 token（`RUTIS_TOKEN_<节点>`），只在需要的节点上设置。
- `rutis-host` 的 token 来自环境变量，运行中不能换：换 token 要重启 `rutis-host`（重启会断开所有连接）。断开现有连接靠停掉那一行：连接这个对端或接受这些运行时的那一行停掉，它的连接随之关闭，进行中的调用以传输错误失败。
- `host: true` 让对端能在本节点运行这里装着的任何插件，只对可信的对端打开；它只能加载已安装的插件，不能加载文件路径。
- 节点只按名字共享 `export` 里列出的服务和事件；事件还要对端在 `import` 里列出。
