# 连接节点

一个 rutis 宿主是一个**节点**。两个节点之间是一条连接：一端写一行 `rutis-host/dial` 去连，另一端写一行 `rutis-host/listen` 等它连进来。连上后，两边的服务互相可见，事件是一条总线，每一端都可以让对面运行插件。断开后拨号的一端自动重连。节点与[远程运行时](#别的机器上的语言运行时)是同一种写法，没有节点专用的配置。

## 两个节点

`main` 连到 `office`：

```json
{
  "id": "main",
  "rows": [
    { "id": "office", "name": "rutis-host/dial", "config": { "address": "wss://office.example.com/rutis" } }
  ]
}
```

`office` 等 `main` 连进来：

```json
{
  "id": "office",
  "listen": [{ "name": "public", "address": "0.0.0.0:7443", "cert": "server.pem", "key": "server.key" }],
  "rows": [
    { "id": "main", "name": "rutis-host/listen", "config": { "listener": "public" } }
  ]
}
```

- 两端用同一个 token：`main` 的 `office` 行出示 `RUTIS_TOKEN_OFFICE`，`office` 的 `main` 行用 `RUTIS_TOKEN_MAIN` 核对，两个变量设成同一个值（行 id 大写，`-` 写作 `_`）。
- 两端在握手里报各自 rutis.json 的 `id`：`office` 上的插件在 `main` 这边叫 `office:<插件>`，反过来是 `main:<插件>`。
- `address` 是 `wss://…`（跨机器，必须 TLS），或 `ws://127.0.0.1…`（本机回环）。自签证书：把签发它的 CA 放进拨号方的 `RUTIS_CA`。
- 断开后重连：从 0.5 秒开始退避，最长 30 秒，连接持续 60 秒后重新从 0.5 秒算；凭据被拒（403、证书不受信任）时每 30 秒重试一次；协议不兼容时停止。监听的一端还没有这一行时，连进来的拨号方被拒（403），30 秒后再试，所以先启动监听的一端。
- `rutis-host run` 打印连接的变化，前缀是行 id，不带 token；这一行自己的 `running` 只说明它在运行，不说明连上了：

  ```text
  office: connected
  office: disconnected (office.example.com:7443: Connection refused); retrying in 2.1s
  office: disconnected (auth-rejected: office.example.com:7443: upgrade refused with 403 Forbidden); retrying in 30s
  main: connected main
  main: disconnected main (connection lost)
  ```

  前三行在 `main` 上，后两行在 `office` 上（`rutis-host/listen` 的行在 `connected` / `disconnected` 后面写连进来的 id）。完整格式见 [rutis-host 打印什么](rutis-host.md#run-打印什么)。

## 连上之后

- **服务两边都可见**：`office` 上的行提供的服务在 `main` 上可以直接 `inject`、调用，反过来也一样，不用列名单。在本端，它由远程插件 `office:<提供它的行>` 提供。调用链保持：同步调用期间的回调、回调里再调回去，都能穿过多个节点。
- **同名按内核规则**：两边都有的服务名，各自用自己的；后注册的那一个被拒，那个远程插件失败并说明原因：

  ```text
  office:weather: weather is provided here by row weather: not registered from office
  ```

- 经一条连接来的服务不再发回这条连接；经过中间的节点，它可以到达第三个节点（`b` 的服务经 `a` 到 `c`）。
- 不想让对面看到某个服务时，在提供它的行上用 `isolate`：只在一个作用域里的服务不跨连接。
- **事件是一条总线**：对面的监听器是本端总线上的条目，规则和本机一样：五种方式（`emit`、`parallel`、`serial`、`waterfall`、`bail`）、过滤、所有值类型，不用列名单。连接断开时它们离开总线（进行中的调用失败一次）。对面的监听器的描述里带这条连接的作用域，供过滤使用。完整模型见 [统一事件](../design-events-2026-10-18.md)。
- 连接断开时，对面的插件在本端全部卸载；依赖它们的服务的行按内核规则停下等待，重连后恢复。

## 在别的节点上运行插件

行 `<连接行>:<插件>` 让对面运行一个插件，配置写在这一行：

```json
{ "id": "office", "name": "rutis-host/dial", "config": { "address": "wss://office.example.com/rutis" } },
{ "id": "scanner", "name": "office:scanner", "config": { "dpi": 300 } }
```

- 对面用它自己的加载器找 `scanner`，和它自己的行一样解析（npm 包、`py:` 模块、Go / Swift 二进制里的插件等）；插件在对面运行，用对面的服务，它提供的服务在本端可见。配置和启停由本端决定。
- 对面没连上时这一行等待，连上后加载；对面找不到这个插件时只有这一行失败：`scanner: unresolved: entry "scanner" rejected: runtime office: this connection does not contain plugin scanner`。

## 别的机器上的语言运行时

在 GPU 机器上只运行一个 Python 运行时，由主节点决定它运行哪些插件：

```bash
# GPU 机器：装好 rutis[network] 和插件，监听
RUTIS_TOKEN=secret RUTIS_CERT=server.pem RUTIS_KEY=server.key \
  python -m rutis listen:wss://0.0.0.0:7443/rutis --id gpu --peer main /srv/plugins
```

```json
{
  "id": "main",
  "rows": [
    { "id": "gpu", "name": "rutis-host/dial", "config": { "address": "wss://gpu.example.com:7443/rutis" } },
    { "id": "embedder", "name": "gpu:embedder" }
  ]
}
```

- 一行 `rutis-host/dial` 是一条连接，行 id（`gpu`）是它的名字：对面运行时的 `--id` 写这个名字，`--peer` 写 rutis 的 `id`。
- 在对面运行的插件写成 `<连接行>:<插件>`（`gpu:embedder`），配置写在这一行的 `config` 里。插件名在对面解析，不用写语言：Python 运行时里是模块，Node、Bun 运行时里是包或模块，Go、Swift 二进制里是插件名。对面没连上时这一行等待，连上后 rutis 让对面运行它。
- token：rutis 一侧是 `RUTIS_TOKEN_GPU`（行 id 大写，`-` 写作 `_`），运行时一侧是 `RUTIS_TOKEN`，两者设成同一个值。自签证书时把 CA 放进 rutis 一侧的 `RUTIS_CA`。连接走 WebSocket。
- 断开后 rutis 退避重连：从 0.5 秒起翻倍，最长 30 秒，连接持续 60 秒后重新从 0.5 秒算；被拒绝时每 30 秒再试；协议不兼容时停止。
- `run` 打印连接的变化，前缀是行 id：

  ```text
  gpu: connected
  gpu: disconnected (gpu.example.com:7443: Connection refused); retrying in 2.1s
  gpu: disconnected (auth-rejected: gpu.example.com:7443: upgrade refused with 403 Forbidden); retrying in 30s
  ```

  `rutis-host status` 的 `connections:` 一节给出当前状态（`gpu: connected`）。对面不含某行的插件时，只有这一行失败（`embedder: unresolved: entry "embedder" rejected: runtime gpu: this connection does not contain plugin embedder`），其他行不受影响。字段与规则见 [rutis-host](rutis-host.md#远程运行时与设备)。
- 0.18 的 `runtimes.remote` 在 0.19 删除，改法见 [迁移到 0.19](../migration-0.19.md)。

Node 运行时同样可以远程运行：在那台机器的 Node 项目里执行 `node node_modules/@arcships/rutis-runtime/src/runner.mjs listen:wss://… --id <名字> --peer <控制方> ./package.json`（插件从这个 package.json 解析）；Bun 运行时也一样：在那台机器的 Bun 项目里执行 `bun --no-install --no-env-file node_modules/@arcships/rutis-bun/src/main.ts listen:wss://… --id <名字> --peer <控制方> .`（插件从这个目录解析）。Go 或 Swift 插件二进制用 `<二进制> listen:wss://… --id <名字> --peer <控制方> .`。rutis 一侧都是同样的一行 `rutis-host/dial`。远程运行时一次服务一个控制方；控制方断开时，它为这个控制方运行的插件全部卸载。每种远程运行时（Python、Node、Bun、Go 或 Swift 二进制）开始监听后都在 stderr 上打印一行实际绑定的地址：`rutis: listening on <地址>`。端口写 0（`listen:ws://127.0.0.1:0/rutis`）时由系统挑一个空闲端口，脚本和测试从这一行读出来；不要先找一个空闲端口再传给它，运行时绑定之前别的进程可能已经占了这个端口。Python 运行时会话结束时还打印一行 `rutis: session ended (<原因>); waiting for a controller`（原因如 `heartbeat timeout`、`connection lost`、`closed (1000): …`），被新连接接替时打印 `rutis: session ended (replaced by a new connection)`。

### 连进来的运行时

运行时也可以反过来连到 rutis：远程机器在 NAT 后、地址会变时用这种方式。rutis 一侧写一行 `rutis-host/listen`，指向 `listen` 里的一个监听器：

```json
{
  "id": "main",
  "listen": [{ "name": "public", "address": "0.0.0.0:7443", "cert": "server.pem", "key": "server.key" }],
  "rows": [
    { "id": "worker", "name": "rutis-host/listen", "config": { "listener": "public" } }
  ]
}
```

每条连接出示同一个 token（rutis 一侧是 `RUTIS_TOKEN_WORKER`），在握手里报一个 id，各自独立：它带来的插件注册为 `<id>:<插件>`，服务全局可见；同一 id 的新连接顶替旧的（旧连接的插件先全部卸载，旧连接再以 4002 关闭）。rutis 不为每条连接建实例。

 0.19 里 `rutis-host/listen` 的对面只带来它自己装着、提供着服务的插件（例如自己装插件的 [Swift](swift-app.md)、[Kotlin](kotlin-app.md) 应用）；`<连接行>:<插件>` 的行不在连进来的运行时上运行。要由 rutis 决定运行时跑什么插件，让运行时监听、rutis 用 `rutis-host/dial`（上一节）。字段与规则见 [rutis-host](rutis-host.md#远程运行时与设备)。

Python 运行时用 `dial:` 连进来：

```bash
# worker 机器：装好 rutis[network] 和插件，连到 rutis
RUTIS_TOKEN=secret RUTIS_CA=ca.pem \
  python -m rutis dial:wss://main.example.com:7443/rutis --id worker-1 --peer main /srv/plugins
```

Go 或 Swift 插件二进制接受同样的 `dial:` 通道（以及 `listen:`），参数、环境变量、重连规则和状态行都与 Python 相同：

```bash
# worker 机器：插件二进制在这里，连到 rutis
RUTIS_TOKEN=secret RUTIS_CA=ca.pem \
  ./weather dial:wss://main.example.com:7443/rutis --id worker-1 --peer main .
```

- `--id` 是这条连接在握手里报的 id（小写字母、数字、`-`），它的插件注册为 `<id>:<插件>`；`--peer` 是 rutis 的 `id`。
- `RUTIS_TOKEN` 与 rutis 一侧的 `RUTIS_TOKEN_WORKER` 相同；`RUTIS_CA` 是额外信任的 CA（自签证书时用），系统根证书照常信任。非回环地址只能用 `wss://`。
- 运行时自己保持连接，按固定规则重连：连不上、连接断开时从 0.5 秒起翻倍等待，最长 30 秒，连接持续 60 秒后重新从 0.5 秒算；token 被拒绝（升级时 401 / 403，或握手里的检查以关闭码 4003 拒绝）每 30 秒再试；协议不兼容（升级时 400 等）不再重试，进程以状态 1 退出。每次断开都先卸载全部行（清理逆序执行），重连是新的会话，行重新装载。
- 状态写在 stderr：`rutis: connected to …`、`rutis: connection lost (…); retrying in 1s`、`rutis: refused: auth-rejected (…); retrying in 30s`、`rutis: refused: incompatible (…); stopping`。
- SIGTERM 或 Ctrl-C 停下它：卸载全部行，关闭连接，不再重连。
- 两个进程用同一个 id 会互相接管，按各自的退避轮流把对方顶掉；每台机器、每个 worker 用不同的 id。

Node、Bun 运行时用同样的参数、环境变量和规则拨出：

```bash
# Node 项目里（插件从 package.json 解析）
RUTIS_TOKEN=secret RUTIS_CA=ca.pem \
  node node_modules/@arcships/rutis-runtime/src/runner.mjs dial:wss://main.example.com:7443/rutis --id worker-1 --peer main ./package.json
# Bun 项目里（插件从这个目录解析）
RUTIS_TOKEN=secret RUTIS_CA=ca.pem \
  bun --no-install --no-env-file node_modules/@arcships/rutis-bun/src/main.ts dial:wss://main.example.com:7443/rutis --id worker-1 --peer main .
```

Node、Bun 的每个会话由一个子进程运行：断开时子进程卸载全部行后退出，重连起新的子进程。`RUTIS_CA` 与 Node、Bun 默认信任的根证书一起信任。

Kotlin 插件一般编进 Android 应用，由应用拨出到 rutis（[Android 应用托管](../design-android-runtime-2026-10-12.md) §五）。SDK 的 jvm target 的 `Rutis.serve` 接受同样的 `dial:`、`listen:` 通道、环境变量和规则（监听 `wss://` 用 `RUTIS_CERT`、`RUTIS_KEY`）。

## Cordis 应用

Cordis 应用接 rutis：用 Node 运行时跑它的插件（`<包名>` 行），见 [Cordis](cordis.md)。

## 自建连接

自己的连接方式（中继、蓝牙、别的传输）写成一个连接插件：拿到一条能收发二进制消息的连接，交出去成为一个会话。Rust 里交出 `Channel`：`ctx.plugin(rutis_bridge::Session::new(channel))`（见 [写 Rust 宿主：节点](rust-host.md#节点)）；其他语言调服务 `sessions` 的 `open`（见 [插件 API：自建连接](plugin-api.md#自建连接sessionsopen)）。对面的插件在这里叫 `<对面 id>:<插件>`，要让对面运行一个插件就写行 `<连接行>:<插件>`。

## 安全

- 跨机器只用 `wss://`。不带 TLS 的监听器只能绑定回环地址。
- Python 运行时监听 `wss://` 不能用 CPython 3.13.6：它的 TLS socket 在一个线程阻塞读、另一个线程写时死锁（[python/cpython#137583](https://github.com/python/cpython/issues/137583)），每个握手都会卡住，所以 rutis 在这个版本上拒绝启动 `listen:wss://`。用 3.13.7 或更新的版本（3.13.5 及更早、3.14 不受影响）；`ws://` 不受影响。拨出（`dial:wss://`）由一个线程完成连接的全部读写，不受影响。
- 每条连接行一个 token（`RUTIS_TOKEN_<行 id>`），只在需要的节点上设置。
- `rutis-host` 的 token 来自环境变量，运行中不能换：换 token 要重启 `rutis-host`（重启会断开所有连接）。断开现有连接靠停掉那一行：那一行停掉，它的连接随之关闭，进行中的调用以传输错误失败。
- 连上的节点互相看到对方全部可共享的服务与事件，并能让本节点运行这里的加载器能解析的任何插件（`<连接行>:<插件>`）：只连可信的节点。不给对面看的服务在自己的行上用 `isolate`。
