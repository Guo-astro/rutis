# Bun 运行时

[English](design-bun-runtime-2026-10-09.en.md)

设计提案 · 关联 [#194](https://github.com/arcships/rutis/issues/194)

## 1. 背景

[多语言插件设计](design-multilang-runtimes-2026-10-03.md)给出了接入一种语言的方式：一个运行时进程实现运行时契约（§五），加一个叶子 SDK（§六）；rutis 核心不变，插件作为 loader 的行运行。Python 运行时就是按这个方式接入的。

Bun 是一个独立的 JS / TS 运行时，原生运行 TS，有自己的模块解析、包管理和网络 API。本设计按同样的方式接入 **Bun 运行时**：

- 一个独立的运行时包 `@arcships/rutis-bun`（`bun/rutis-bun`），在 Bun 进程里实现运行时契约；
- 行名 `bun:<模块>`，与 Python 行的 `py:<模块>` 同一种写法；
- Rust 侧有自己的启动器、cargo feature 和配置项；
- 用同一套契约测试验证。

按[设计哲学](design-philosophy.md) §7 的判断：它让宿主能接上只有 Bun 的项目和机器；放在核心之外；协议和边界规则不变；有具体用户；保证可以由现有的契约测试钉住。

## 2. Bun 平台调研

2026-10-09，Bun 1.3.14，macOS，在 Bun 上运行一个实现了运行时契约的 JS 原型，结果：

| 能力 | Bun 的行为 | 对本设计的影响 |
| --- | --- | --- |
| 同步等待期间的重入：`worker_threads`、SharedArrayBuffer、主线程 `Atomics.wait`、`receiveMessageOnPort` | 可用；worker 崩溃也能在阻塞等待中报出 | I/O 放在 worker，主线程阻塞等待并按调用链执行反向调用（§3.3） |
| 继承的 socket（fd:3） | `net.connect({ fd })` 可用；`new net.Socket({ fd })` **静默失效**（不报错、读不到数据、进程自行退出） | 只用 `net.connect({ fd })` |
| Unix socket、回环 TCP | `net.createConnection` / `createServer` 可用 | dial-back 与 Windows 的 loopback 交接可用（Windows 未验证） |
| WebSocket 服务端 | 可用 | 远程运行时监听可用 |
| WebSocket 客户端 | 内置的 `ws` 替代实现不触发 `upgrade` / `unexpected-response`；握手被拒（如 401）时既无事件也无错误，挂起 | 拨出需要自行实现，见 §5 |
| 模块重新导入 | `import(url + '?v=…')` **忽略查询串**，返回旧模块；删除 `require.cache[realpath]` 后再导入可得到新模块 | 热重载用 `require.cache`，必须用 realpath（macOS `/var` → `/private/var`） |
| 自动安装 | 目录里没有 node_modules 时，从全局缓存解析甚至下载包 | **始终 `bun --no-install` 启动**：既保证解析结果来自项目，也是安全要求 |
| TS | 原生运行：enum、参数属性、装饰器、`./a.js` 指向 `a.ts` | 不需要额外的 TS 加载器 |
| 启动耗时 | 29ms（10 次中位数） | — |

吞吐、内存、Windows 都没有数据。

## 3. 运行时包 `@arcships/rutis-bun`

### 3.1 进程

```
bun --no-install <包>/src/main.ts <通道> <锚点>
```

- `<通道>`：`fd:3`（Unix 继承）、socket 路径（dial-back）、`tcp:<地址>`（Windows loopback，带一次性 token），与其他运行时相同的交接方式。
- `<锚点>`：项目的 `package.json`，模块从这里解析。
- 启动时检查 Bun 版本，低于支持的最低版本（暂定 1.3）时在问候前以明确的错误退出。
- 问候里报告实现名与版本（`rutis-bun 0.9.0`、`bun 1.3.14`），`rutis-host check` 和 `diagnostics()` 能看到。

### 3.2 契约

实现[多语言插件设计](design-multilang-runtimes-2026-10-03.md) §五 的运行时契约：

- 会话层（值、引用、回调、取消、release 计数），以 `rutis-bridge` 的会话契约测试为准；
- `rows.load` / `rows.schema`：`entry` 是模块名，由 Bun 从锚点解析（npm 包名、包的子路径、`./相对路径`）；`rows.schema` 返回配置 Schema、依赖、提供的服务及每个方法是同步还是异步；
- 行提供的服务通过服务槽位通知报给 rutis；
- `ctx.use(name)` 返回按名字调用的代理，走 `host:<名字>`；
- 同一运行时内的插件互相使用时直接给对象。

### 3.3 同步等待

I/O 在一个 worker 里，主线程发起同步调用后用 `Atomics.wait` 阻塞；等待期间到达的、属于这条调用链的反向调用由主线程执行（按 `path` 判断），其余排队。与 Python 运行时的 I/O 线程加主线程是同一个模型。不属于调用链、又需要事件循环推进才能完成的等待，返回 `SyncWaitCycle`（需求文档 §5 规则 6）。

### 3.4 热重载

入口文件变化（mtime + 大小）时，删除 `require.cache` 中 `realpathSync(entry)` 一项再导入。只重新加载入口模块，它导入的模块继续用缓存；需要连同依赖一起换时，重启这一行所在的运行时。

### 3.5 插件 SDK

插件用 `@arcships/rutis` 的 `definePlugin` 声明（`inject`、`apply`、配置 Schema）。这个包只定义插件的声明形状，不依赖任何运行时，Bun 运行时按它的形状装载插件，插件作者不需要另一套 SDK。测试工具（`@arcships/rutis/testing`）在 `bun test` 下可用要作为验收项。

### 3.6 范围

第一版只运行**叶子插件**（`definePlugin` 声明的插件）。Cordis 插件的挂载、Cordis 应用作为节点接入不在本设计内，有需要时另行设计。

## 4. Rust 侧与配置

### 4.1 rutis-bridge 与 rutis-loader

```rust
Launcher::bun(package)               // bun --no-install <package>/src/main.ts
LocalRuntime::bun(package, project)  // 名为 "bun"
RuntimeResolver::modules(handle)     // 现有：行名 "bun:<模块>"，由运行时解析
```

- 新增 cargo feature `bun`（rutis-bridge、rutis-loader），与 `node`、`python` 并列。
- 行名解析沿用 `Naming::Modules`（`crates/rutis-loader/src/runtime.rs:78`）：前缀是运行时名 `bun:`，模块由运行时自己解析，每次都问运行时（不缓存）。
- fd:3 继承由运行时包的 `rutisChannels` 声明决定，与其他运行时一致。
- `program` 默认在 `PATH` 上找 `bun`（Windows 为 `bun.exe`），可以指定路径。
- 始终带 `--no-install`，不可配置去掉。

### 4.2 rutis.json

```json
{
  "runtimes": {
    "bun": { "project": ".", "program": "/opt/bun/bin/bun" }
  },
  "rows": [
    { "id": "weather", "name": "bun:@foo/weather" },
    { "id": "report", "name": "bun:./report.ts", "inject": ["weather"] }
  ]
}
```

| 字段 | 默认 | 说明 |
| --- | --- | --- |
| `project` | `.` | 插件包从这里的 `package.json` 解析 |
| `runtime` | 项目里的 `@arcships/rutis-bun` | 运行时包位置 |
| `program` | 在 `PATH` 上找 `bun` | Bun 可执行文件 |

- 缺少运行时包时启动失败，提示 `bun add -d @arcships/rutis-bun`。
- 远程运行时：`remote` 的 `language` 新增 `"bun"`，行名 `<远程运行时名>:<模块>`。
- 与 `runtimes.node`、`runtimes.py` 互不影响，可以同时存在；服务按名字跨运行时共享，与跨语言服务走同一条路（`host_key`）。

### 4.3 rutis-host

- `rutis-host new <名字> --lang bun`：生成 Bun 插件项目（`package.json`、`src/index.ts`、`bun test` 的测试、`rutis.dev.json`）。
- `rutis-host dev`：识别 Bun 项目（`rutis.dev.json` 的 `runtimes.bun`），文件变化时重新加载。
- `rutis-host check`：列出 `bun:` 行的版本、依赖、服务、配置 Schema。
- npm 分发的 `@arcships/rutis-host` 不自带 Bun 运行时；Bun 项目把 `@arcships/rutis-bun` 装在自己的项目里。

## 5. WebSocket

- **监听**（远程运行时，`serve listen:ws://…` / `wss://…`）：用 Bun 的服务端 API 实现，第一版包含。
- **拨出**（运行时以 `ws://` / `wss://` 为通道去连 rutis 节点）：Bun 内置的客户端在握手被拒时不给出状态码，无法按 `auth-rejected` / `incompatible` / `retryable` 分类。第一版在拨出时直接以 `incompatible: dialing a WebSocket is not supported by the Bun runtime yet` 失败，不挂起（原则 7：跨不过的写成规则）。第二阶段自行实现握手：`Bun.connect` / `tls.connect` 建连，自己发 Upgrade 请求、读状态码，101 之后交给帧层。验收是 `websocket_cross` 的拒绝分类在 Bun 列通过。

## 6. 测试

| 层 | 测试 | 内容 |
| --- | --- | --- |
| 会话契约 | `rutis-bridge/tests/runtime_conformance.rs` | 增加 Bun 端点，跑 `session::testing` 全部检查 |
| 运行时契约 | `rutis-bridge/tests/session_matrix.rs` | 增加 {Bun} × {fd:3、dial-back、WebSocket 监听} |
| 通道契约 | 新增 | Bun 的各通道实现跑通道契约（顺序、背压、大消息、close 语义、长度上限） |
| loader 行 | 新增 `rutis-loader/tests/bun_rows.rs` | 装载、热重载、改坏后旧版本继续服务、进程退出与重启、启动失败不阻塞解析（对应 `python_rows.rs`、`runtime_rows.rs` 的检查） |
| 跨语言 | `rutis-loader/tests/multilang.rs` | Bun、Python 与 Node 的插件互相使用服务；提供者离开时只停它的使用者 |
| 实例 | `instance_runtimes.rs` | Bun 行在实例内的服务名 |
| 宿主 | `rutis-host` | `bun:` 行的热重载；`new --lang bun` 生成的项目 `check` 通过 |
| 启动器 | 新增 | `--no-install` 始终存在；没有 node_modules 时引用未安装的包加载失败，不下载；Bun 版本过低时报错；缺少运行时包时的提示 |
| WebSocket | `websocket_cross.rs` | 第一阶段：Bun 拨出以 `incompatible` 失败而不是挂起；第二阶段：与其他实现相同的拒绝分类 |
| 运行时自身 | `bun/rutis-bun/test` | `bun test`：通道、会话、同步等待、热重载、Schema |
| E2E | S2 [#186](https://github.com/arcships/rutis/issues/186)、S3 [#187](https://github.com/arcships/rutis/issues/187) | `new --lang bun` 的开发循环；Bun 行参与跨语言组合与崩溃恢复 |

**CI**：新增 `runtimes-bun` job，Linux 与 macOS，用 `oven-sh/setup-bun` 安装固定版本的 Bun，运行 `bun test`、`cargo test -p rutis-bridge --features bun,…`、`cargo test -p rutis-loader --features bun,…`、`cargo test -p rutis-host`；另有 nightly 跟踪最新版 Bun。`release-dry-run` 增加 `@arcships/rutis-bun` 的打包，发布列车（`scripts/train.mjs`）加入这个包。

## 7. 待定

1. 支持的 Bun 版本范围（最低暂定 1.3）。
2. Windows：loopback 交接、job object 收养、`kill_on_drop` 在 Bun 下的行为。
3. 性能：同步调用往返和吞吐的数据。
4. Cordis 插件在 Bun 运行时里的挂载是否需要（§3.6）。

## 8. 分阶段

| 阶段 | 内容 |
| --- | --- |
| B1 | 运行时包：本地通道（fd、dial-back、loopback）、会话、行、同步等待、热重载；Rust 的 `bun` feature、启动器、`LocalRuntime::bun`；`rutis.json` 的 `runtimes.bun`；§6 中除 WebSocket 第二阶段以外的测试；CI `runtimes-bun` |
| B2 | WebSocket 监听（远程运行时）与拨出（§5 第二阶段）；`remote` 的 `language: "bun"` |
| B3 | `rutis-host new --lang bun` 与 dev；npm 发布；Windows；性能数据 |
