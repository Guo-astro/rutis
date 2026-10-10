# Bun 运行时

[English](design-bun-runtime-2026-10-09.en.md)

设计提案 · 关联 [#194](https://github.com/arcships/rutis/issues/194)

## 1. 背景

[多语言插件设计](design-multilang-runtimes-2026-10-03.md)规定了接入一种语言的方式：一个运行时进程实现运行时契约（§五），加上叶子 SDK（§六）。rutis 核心不变，插件作为 loader 的行运行。Python 运行时就是这样接入的，它也是本设计对照的参考实现。

Bun 是一个独立的 JS / TS 运行时，有自己的模块解析、包管理、网络 API，原生运行 TS，还能把程序编译成单文件可执行程序。本设计按同样的方式接入 **Bun 运行时**：

- 运行时：`rutis-bun`，在 Bun 进程里实现运行时契约，有自己的一份会话层实现；
- 行名：`bun:<模块>`；
- Rust 侧：自己的启动器、cargo feature 和配置项；
- 分发：npm 包（需要本机有 Bun）和单文件可执行程序（不需要 Bun）两种；
- 验证：用同一套契约测试。

[M1 实施稿](design-multilang-m1-2026-10-04.md) §11.4 当时没有选择"另开一个纯 JS 运行时"。本设计是 [#194](https://github.com/arcships/rutis/issues/194) 提出的新决定：Bun 作为一种独立的运行时加入。§11.4 那段话只针对当时 M1 的叶子插件方案，对本设计不构成约束。

## 2. Bun 平台调研

调研用 Bun 1.3.14 在 macOS 上实测，并对照 Bun 官方文档（bun.com/docs）。"文档"表示来自官方文档，"实测"表示本机验证过。

| 能力 | Bun 的行为 | 对设计的影响 |
| --- | --- | --- |
| `worker_threads`、SharedArrayBuffer、主线程 `Atomics.wait`、`receiveMessageOnPort` | 实测可用。文档：`worker_threads` 部分实现（缺 `moveMessagePortToContext` 等），Web Worker 的终止标为 experimental | I/O 放在 worker，主线程阻塞等待（§3.4）。worker 的崩溃和退出路径要有测试 |
| 继承的 socket（fd:3） | 文档写明 `new net.Socket({ fd })` 不能读已有的 fd；实测它静默失效。`net.connect({ fd })` 和 `Bun.connect({ fd, socket })` 实测可用 | 用 `Bun.connect({ fd })`；原生 socket 不带缓冲，背压由运行时自己处理（文档） |
| Unix socket、回环 TCP | 可用 | dial-back、loopback 交接 |
| WebSocket 服务端（`Bun.serve`） | 可用。文档默认值：`maxPayloadLength` 16MB，`idleTimeout` 120s，`backpressureLimit` 16MB | 远程运行时监听用它；`idleTimeout` 必须关掉或大于心跳间隔，上限与 rutis 对齐（§4） |
| 模块重新导入 | 实测：`file://` URL 带查询串返回旧模块；绝对路径带查询串得到新实例，但旧实例会一直留在缓存里。删除 `require.cache[realpath]` 后再导入可以得到新模块，并且只重载入口。文档：官方没有提供以编程方式让模块失效的接口（`--hot` 内部用的 `Loader.registry` 不对外暴露） | 用 `require.cache`（§3.8）。这是文档没有承诺的行为，nightly 跟踪最新版 Bun 时必须覆盖到 |
| 自动安装 | 文档：`install.auto` 默认是 `auto`，即没有 node_modules 时自动安装。实测：`--no-install` 能压过项目 bunfig 里的 `auto = "force"` | 始终加 `--no-install` |
| bunfig 与 `.env` | 文档 + 实测：`bunfig.toml` 从**工作目录**读取，其中的 `preload` 会在入口之前执行；`.env` 会自动加载 | 工作目录固定为项目目录，项目的 bunfig 视为项目的可信配置；始终加 `--no-env-file`，环境变量只由宿主给出（§3.1） |
| `bun install` 的生命周期脚本 | 文档：默认只运行内置受信名单里的依赖的脚本；项目里写 `trustedDependencies` 会**替换**这个名单 | 写进插件项目指南 |
| TS | 文档 + 实测：原生运行；支持 tsconfig 的 `paths`；装饰器语义随 tsconfig 而定（1.3.10 起未开 `experimentalDecorators` 时用 TC39 标准装饰器）；**不做类型检查** | 模板带 `tsc --noEmit` |
| WebSocket 客户端 | 实测：握手被拒时触发 `error`，然后以 1002 关闭，**拿不到状态码** | 运行时不主动拨出（§4），不受影响 |
| `bun build --compile` | 实测：单文件约 61MB，能从磁盘动态导入外部 `.ts` 插件，删除 `require.cache` 后重载正常。文档：1.4 起编译产物默认不读 tsconfig 和 package.json，需要 `--compile-autoload-tsconfig` / `--compile-autoload-package-json`；`BUN_BE_BUN=1` 会让它退回成完整的 bun 命令 | 作为第二种分发形式（§6） |
| `--no-orphans` | 文档：父进程退出时，自己退出并 SIGKILL 所有子孙进程（Linux、macOS） | 启动参数加上，作为 `kill_on_drop` 的补充 |
| `node:test` / `async_hooks` | 文档：`node:test` 部分支持；`async_hooks` 只实现了 `AsyncLocalStorage` | 运行时自身的测试用 `bun:test`；异步上下文只用 ALS |
| 版本 | 文档：没有 LTS，也没有 semver 承诺；当前最新是 1.4.2 | 见 §7 |

## 3. 运行时 `rutis-bun`

### 3.1 进程

```
bun --no-install --no-env-file --no-orphans <包>/src/main.ts <通道> [--id <端点> --peer <端点>] <项目>
rutis-bun <通道> [--id <端点> --peer <端点>] <项目>          # 单文件可执行程序
```

- **参数**：与 Python 运行时一致（`python3 -m rutis <通道> [--id] [--peer] <项目>`）。
- **工作目录**：项目目录。
- **`--no-install`、`--no-env-file`**：始终加上，不能通过配置去掉。可执行程序在编译时就关掉自动安装和 `.env`，并且不响应 `BUN_BE_BUN`。
- **插件是可信代码**：运行时不是沙箱，这与[需求文档](requirements-protocol-plugins.md) §7 一致。项目的 `bunfig.toml` 属于项目配置，其中的 `preload` 和插件代码一样被信任。

### 3.2 通道与交接

| 通道 | 做法 |
| --- | --- |
| `fd:3`（Unix，宿主创建 socket 后交给子进程继承） | `Bun.connect({ fd: 3 })`。Rust 侧的启动器固定使用继承，和 Python 一样，不读包里的声明 |
| socket 路径（dial-back，即运行时反过来去连宿主给的 socket） | `Bun.connect({ unix })` |
| `tcp:<地址>`（loopback，Windows 默认使用；Unix 上设 `RUTIS_LOCAL_HANDOVER=loopback` 时使用） | 先发送 `RUTIS_CHANNEL_TOKEN`，**用完立即从 `process.env` 删除**，与 Python `__main__.py` 的做法一致 |
| `listen:ws://…` / `listen:wss://…` | 远程运行时，见 §4 |
| `ws://…`（主动拨出） | 启动时直接报错："the Bun runtime only listens"（远程插件设计 §4.4） |

- **分帧**：按行分帧。单条消息有长度上限，与 [#173](https://github.com/arcships/rutis/issues/173) 一致；超过上限时关闭通道。
- **half-close**：要单独测试。Node 在 macOS 上有一个已知问题，`node/rutis-runtime/src/channel/unix.mjs:20` 记录过。

### 3.3 契约

**协议格式**：本机用 compat（协议 2）；网络用 endpoint 格式（协议 3，WebSocket 子协议 `rutis.3`）。

**能力**（问候时的 `capabilities`）：`signals`、`reentrant-sync`。

**`mount` 的回复**：`{ services: {}, features: ["rows.v2", "hosts", "leaf", "scopes"], implementation: { name: "rutis-bun", version }, engine: { name: "bun", version: Bun.version } }`。Rust 侧对 `mount` 回复按松散 JSON 解析，所以 `implementation` 和 `engine` 是新增字段，不改协议；`rutis-host check` 会打印它们。

**控制操作**：与 Python 运行时（`python/rutis/rutis/runner.py`）逐项对齐。

| 操作 | 要点 |
| --- | --- |
| `mount` | 见上 |
| `dispose` | 卸载全部行，并等待进行中的调用排空 |
| `rows.load` | `(key, entry, config, isolate, inject, exports)`：`entry` 是模块名，由 Bun 从项目解析（npm 包名、子路径、`./相对路径`）；`exports` 中列出的服务登记为导出槽 |
| `rows.update` | 叶子插件改配置等于重启这一行 |
| `rows.unload` | 执行清理，撤回这一行的服务 |
| `rows.schema` | 返回配置 Schema、`inject`、`provides`、每个方法是同步还是异步、`version`（插件包的 package.json） |
| `hosts.provide` / `hosts.withdraw` | 宿主的服务出现或撤回时，`[name, methods, label?]` / `[id]` |
| `release` / `get` | 引用计数与属性读取 |

**插件 API 版本**：插件声明的 `api` 高于运行时支持的版本时拒绝加载，错误提示升级 `rutis-bun`（[开发者包设计](design-developer-packages-2026-10-06.md) §5.1）。

### 3.4 同步等待与重入

I/O 在一个 worker 里。插件发起同步调用后，主线程用 `Atomics.wait` 阻塞。**这期间进来的所有调用都在主线程上执行**，不管是否属于本调用链，与 Python 运行时一样（`peer.py:9-14`）。所以 Bun 与任何运行时互相同步调用都不会卡死。代价是插件的服务可能在它自己发起的同步调用中途被调用，指南里写明"调用 rutis 服务时不要持有锁"。

只有一种情况返回 `SyncWaitCycle`：被等待的结果要靠被父同步调用占住的事件循环才能完成，对照 `peer.py:843-850`。

### 3.5 实例内服务

按[实例内服务设计](design-instance-services-2026-10-08.md) §4.2 实现：

- 服务身份是 `(名字, 标签)`，id 为 `name\0label`；
- 导出槽、句柄、宿主代理、`host:<id>` 都按 id 登记；
- 句柄按代递增：`id#N`，带作用域时为 `\0N`。

同一运行时内的插件互相使用时直接给对象，但要先按本行的 isolate 表查到对应的 id。

### 3.6 取消、错误与进程退出

- **取消**：参数里的 `signal` 解码成真正的 `AbortSignal`，有效期到结果确定为止；收到 `cancel` 时中止。
- **错误**：错误图原样往返，包括 cause、AggregateError、自定义名字和字段。Bun 的 Error 带额外字段（如 `line`、`column`、`sourceURL`），**不能**泄漏到错误形状里。
- **未捕获的异常或 rejection**：结束整个进程，这个运行时的全部服务随之撤回（需求文档 §5 规则 8）。

### 3.7 远程租约

见 §4。

### 3.8 热重载

入口文件变化时（比较 mtime 和大小），从 `require.cache` 里删除 `realpathSync(entry)` 这一项，然后重新导入。

- 只换入口模块，它导入的模块继续用缓存。要连依赖一起换，就重启这个运行时。
- 不用查询串：带查询串的旧实例会一直留在缓存里。
- 不用 `--hot`：它会让整个进程重新执行，与"按行重载"不符。

### 3.9 插件 SDK 与运行时代码

- **插件声明**：插件用 `@arcships/rutis` 的 `definePlugin` 声明。这个包只有声明形状和测试工具（`index.mjs`、`testing.mjs`），不依赖任何运行时。它的 `engines` 目前只写了 `node`，要补上 `bun`。
- **测试工具**：`@arcships/rutis/testing` 在 `bun test` 下能用，这一条作为验收项。
- **会话层、通道、叶子装载**：`rutis-bun` **自己维护一份**，不依赖 `@arcships/rutis-runtime`。那个包的叶子装载依赖 Cordis Context，内部模块也没有导出。实现时可以参考现有代码，但代码归 `rutis-bun` 所有，正确性以契约测试为准。

### 3.10 范围

第一版只运行叶子插件（`definePlugin`）。Cordis 插件的挂载、Cordis 应用作为节点接入不在本设计内。

## 4. 远程运行时（监听与租约）

`rutis-bun listen:ws://… --id <端点> [--peer <控制方>] <项目>`，按[远程插件设计](design-remote-plugins-2026-10-03.md) §4.4 实现，与 Python 的 `rutis[network]` 对齐：

- **监听**：只监听，不主动拨号；不带 TLS 时只允许回环地址。
- **鉴权**：凭据来自 `RUTIS_TOKEN`、`RUTIS_CERT`、`RUTIS_KEY`。token 用防时序攻击的方式比较。拒绝原因要区分：404 是路径不对，401 是没带凭据，403 是 token 不对，400 是子协议不对（与 `python/rutis/rutis/websocket.py:147-161` 一致）。
- **消息与心跳**：单条消息上限 16 MiB，超过时以 1009 关闭；心跳默认值与 Python 相同（30 s 无响应断开），可用 `RUTIS_HEARTBEAT` 调整；`Bun.serve` 的 `idleTimeout` 关掉，由心跳负责发现断线。
- **接管**：新连接的处理顺序是：鉴权 → 关闭旧会话通道，不再读它的后续帧 → 清理旧租约里的全部行和代理 → 回应新会话的 `hello`。被接管的旧连接以 4002 关闭。租约在进程内清理（与 Python 相同），模块缓存跨租约保留。
- **启动输出**：启动后在 stderr 打印 `rutis: listening on …`。

## 5. Rust 侧与配置

### 5.1 rutis-bridge 与 rutis-loader

```rust
Launcher::bun(program, package)        // bun --no-install --no-env-file --no-orphans <package>/src/main.ts，固定继承 fd:3
Launcher::bun_executable(program)      // rutis-bun 单文件可执行程序
LocalRuntime::bun(launcher, project)   // 名为 "bun"
RuntimeResolver::modules(handle)       // 现有：行名 "bun:<模块>"
```

- **cargo feature**：新增 `bun`，加在 rutis-bridge 和 rutis-loader 上，与 `node`、`python` 并列。
- **行名解析**：沿用 `Naming::Modules`，前缀是运行时名 `bun:`。版本从 `rows.schema` 的 `version` 获得：`RowSchema` 加上 `version` 字段，写进行的 meta。Python 入口点的版本现在会被丢掉，这一处一起修。
- **名字冲突**：
  - 行名 `bun:sqlite` 指项目里的模块 `sqlite`，**不是** Bun 的内置模块 `bun:sqlite`。插件要用内置模块，就在插件代码里 import。指南里写明这一点。
  - 远程 node 运行时的 `Naming::Npm` 目前会接收任何不是文件的名字（`rutis-loader/src/runtime.rs:90-94`），包括 `bun:x`。改成不接收带有已知运行时前缀的名字。
  - 运行时名（本机的 `node`、`py`、`bun` 和远程运行时的名字）重复时，配置报错。

### 5.2 rutis.json

```json
{
  "runtimes": {
    "bun": { "project": "." }
  },
  "rows": [
    { "id": "weather", "name": "bun:@foo/weather" },
    { "id": "report", "name": "bun:./report.ts", "inject": ["weather"] }
  ]
}
```

| 字段 | 默认 | 说明 |
| --- | --- | --- |
| `project` | `.` | 插件从这里的 `package.json` 解析，也是工作目录 |
| `runtime` | 依次找：项目里的 `@arcships/rutis-bun`、`PATH` 上的 `rutis-bun` 可执行程序 | 运行时的位置：一个 npm 包目录，或一个可执行文件 |
| `program` | `PATH` 上的 `bun` | Bun 可执行文件；`runtime` 是单文件可执行程序时不用 |

- **运行时缺失**：启动失败，并给出两种安装方式。
- **远程运行时**：`remote` 的 `language` 新增 `"bun"`（`rutis-host/src/host.rs:78-83`），行名写作 `<远程运行时名>:<模块>`。
- **与其他运行时的关系**：`runtimes.bun` 与 `runtimes.node`、`runtimes.py` 互相独立，可以同时配置。服务按名字共享，走 `host_key`。

### 5.3 rutis-host

- **`new <名字> --lang bun`**：生成 `package.json`、`src/index.ts`、`bun:test` 测试、`tsconfig.json`（检查脚本 `tsc --noEmit`）和 `rutis.dev.json`。
- **`dev`**：先读 `rutis.dev.json` 再决定运行时；没有这个文件时，看到 `bun.lock` 就按 Bun 项目处理。现在 `project.rs:21-22` 一看到 `package.json` 就选 Node，要改。
- **`check`**：列出 `bun:` 行的版本、依赖、服务和配置 Schema，以及运行时的实现名和 Bun 版本。

## 6. 分发

分发的是运行时（协议实现加插件装载），插件 SDK 仍然是 `@arcships/rutis`。提供两种形式：

| 形式 | 内容 | 需要 |
| --- | --- | --- |
| npm 包 `@arcships/rutis-bun` | `src/main.ts` 等源码 | 本机装有 Bun |
| 单文件可执行程序 `rutis-bun-<版本>-<平台>` | `bun build --compile` 的产物，内嵌 Bun | 什么都不需要 |

- **可执行程序的发布**：随 GitHub Release 发布（Linux、macOS 的 x64 / arm64，Windows x64），也可以作为 npm 平台包分发（和 `@arcships/rutis-host` 的平台包方式相同）。编译参数固定为：关闭自动安装、`--compile-autoload-tsconfig`、`--compile-autoload-package-json`。插件从项目目录动态导入，项目的 tsconfig `paths` 照常生效。
- **只有 Bun 的机器**：npm 版 `rutis-host` 的入口是 `#!/usr/bin/env node`，在这种机器上跑不起来。所以 Bun 用户用 `rutis-host` 的二进制分发（GitHub Release、`cargo install`），或者验证 `bunx --bun @arcships/rutis-host` 是否可用（B3）。
- **发布列车**：加入 `@arcships/rutis-bun`（`scripts/train.mjs`），并校验运行时代码里的实现版本常量，做法与 Python 的 `IMPLEMENTATION` 相同。

## 7. Bun 版本

本设计用到的能力（`Bun.connect({ fd })`、`worker_threads` 与主线程 `Atomics.wait`、`require.cache`、`--no-install`、`Bun.serve` 的 WebSocket）没有哪一项依赖很新的版本，所以**不人为设定下限**：

- **npm 形式**：支持的最低版本是 CI 矩阵里实测通过的最老版本，写进 `engines.bun`，运行时启动时也检查一遍。CI 矩阵包括：一个较老的版本（从 1.1.x / 1.2.x 里选出能通过的最老版本），当前最新的稳定版，以及 nightly 跟踪最新版。
- **可执行程序形式**：内嵌的 Bun 版本在构建时固定，不受用户机器影响。
- **`--no-orphans` 等较新的参数**：运行时按 `Bun.version` 判断，旧版本上不加（Rust 侧先问一次 `bun --version`，或者由运行时自己处理），不因此抬高下限。

## 8. 测试

**契约测试**

| 测试 | 内容 |
| --- | --- |
| 会话契约 `runtime_conformance.rs` | 加一个 Bun 端点，跑 `session::testing` 的全部检查 |
| 运行时契约 `session_matrix.rs` | {Bun} × {fd:3、dial-back、WebSocket 监听}；另加 loopback 列（Unix 上设 `RUTIS_LOCAL_HANDOVER=loopback` 跑） |
| 通道契约 | `channel::testing::contract` 现在只接受 Rust 的通道对，需要先写一个 harness，让 Bun 一侧作为对端回显 |
| 一致性夹具 | `conformance-session`、`conformance-weather`、`conformance-greeter` 写 Bun 版本；`crash()` 以状态码 17 退出 |

**会话层专项**

| 测试 | 内容 |
| --- | --- |
| `cancellation.rs` | `AbortSignal` |
| `error_shape.rs` | 错误图原样往返，Bun 的额外字段不泄漏 |
| `rpc_callbacks.rs` | 回调、同步等待 |
| `process_exit.rs` | 未捕获的异常结束进程，服务撤回 |
| `live_objects.rs` | 引用与 release |

**python_runtime.rs 的 Bun 版**

- features 包含 `rows.v2`、`hosts`、`leaf`、`scopes`；
- 不支持行契约的运行时启动失败；
- 卸载时不等运行时，先撤回服务。

**loader**

- **`bun_rows.rs`**：装载；热重载（含通过 `bun link` 建立的符号链接下的 realpath）；改坏后旧版本继续服务；进程退出与重启；启动失败不阻塞解析。
- **`leases.rs`、`remote_rows.rs`**：加 Bun 列。依次接入的控制方各自拿到干净的租约；旧租约清理完才接管；慢启动加慢清理；重连后拿到新租约。
- **`multilang.rs`**：Bun、Python、Node 互相使用服务；**冷启动时互相同步调用不卡死**（对应 §3.4）；M2 那组四行同时冷启动的场景作为验收。
- **`instance_runtimes.rs`**：实例内的服务名。

**宿主与启动器**

| 测试 | 内容 |
| --- | --- |
| rutis-host | `bun:` 行热重载；`a_remote_runtime_runs_rows_named_after_it` 的 Bun 版；`new --lang bun` 生成的项目能通过 `check`；`dev` 能识别 Bun 项目；名字冲突时配置报错 |
| 启动器 | 三个必加参数始终存在；没有 node_modules 时引用未安装的包，加载失败，不会下载；项目 `.env` 不被加载；缺少运行时时的提示；`ws://` 拨出被拒绝；`RUTIS_CHANNEL_TOKEN` 用完被删除 |

**运行时自身与 SDK**（`bun test`，用 `bun:test`）

- 通道、会话、同步重入、热重载、Schema、实例作用域（对照 `test_scopes.py`）；
- WebSocket 监听：鉴权、回环限制、大小上限、4002、心跳（对照 `test_websocket.py`）；
- 版本（对照 `test_entry_points.py`）；
- `@arcships/rutis/testing` 在 `bun test` 下可用。

**E2E**

- S2（[#186](https://github.com/arcships/rutis/issues/186)）：`new --lang bun` 的开发循环；
- S3（[#187](https://github.com/arcships/rutis/issues/187)）：Bun 行参与跨语言组合与崩溃恢复；
- S9（[#193](https://github.com/arcships/rutis/issues/193)）：在没有 Bun 的干净环境里运行单文件可执行程序。

**CI**

- **`runtimes-bun` job**：在 Linux 和 macOS 上，用 `oven-sh/setup-bun` 装上 §7 矩阵里的各个版本，然后运行 `bun test`、`cargo test -p rutis-bridge --features bun,…`、`cargo test -p rutis-loader --features bun,…`、`cargo test -p rutis-host`。
- **单一 feature 编译检查**：`cargo check --no-default-features --features bun`。
- **可执行程序冒烟**：打包可执行程序，在没有 Bun 的环境里跑契约测试。
- **nightly**：跟踪最新版 Bun，重点盯 `require.cache` 重载、`Bun.connect({ fd })`、WebSocket 服务端的默认值。
- **Windows**：现有的 `runtimes-windows` 已经跑 Python 运行时。Bun 在 B3 加入，第一版先跑 loopback 列。

## 9. 待定

1. **Windows**：loopback 交接、job object 收养、`kill_on_drop` 在 Bun 下的表现。
2. **性能**：同步调用往返和吞吐的数据。
3. **Cordis 插件**：是否需要在 Bun 运行时里挂载（§3.10）。
4. **`bunx --bun @arcships/rutis-host`**：是否可用，决定 npm 版 `rutis-host` 能不能给只有 Bun 的用户使用。

## 10. 分阶段

| 阶段 | 内容 |
| --- | --- |
| B1 | `rutis-bun` npm 形式，包括：本机通道（fd、dial-back、loopback）、会话、全部控制操作、同步重入、实例作用域、取消与错误、热重载。Rust 侧：`bun` feature、启动器、`LocalRuntime::bun`、`RowSchema.version`、名字冲突检查；`runtimes.bun`。§8 中本机部分的测试；CI `runtimes-bun` |
| B2 | 远程运行时：监听与租约（§4）；`remote` 的 `language: "bun"` 及租约测试 |
| B3 | 单文件可执行程序与发布；`rutis-host new --lang bun` / `dev`；Windows；性能数据 |
