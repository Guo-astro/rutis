# rutis 运行时契约（摘要）

> 整理自 rutis main（2026-10-14）的实现与设计稿，随 skill 一起分发以便独立使用。以代码与原文为准：
> 多语言总体设计 https://github.com/arcships/rutis/blob/main/docs/design-multilang-runtimes-2026-10-03.md ·
> 远程插件 https://github.com/arcships/rutis/blob/main/docs/design-remote-plugins-2026-10-03.md ·
> 参考实现 Python https://github.com/arcships/rutis/tree/main/python/rutis/rutis 、Go https://github.com/arcships/rutis/tree/main/go/rutis

运行时是一个进程（或一条远程连接），它和 rutis 之间是一个**会话**：先握手，然后双方互发调用帧。rutis 用控制操作驱动运行时装载、卸载插件；插件提供的服务由 rutis 按名字投到宿主，别的插件（任何语言）按名字使用。

## 1. 帧与通道

- 帧：换行分隔的紧凑 JSON，一条消息上限 16 MiB，超限关闭通道。
- 本地通道（rutis 拉起进程时在参数里给出）：

  | channel | 方式 |
  | --- | --- |
  | `fd:3` | 继承的 socket pair 一端 |
  | `unix:<path>` 或裸路径 | 拨回 rutis 建在私有目录里的 socket |
  | `tcp:127.0.0.1:<port>` | 拨回本机端口，先发环境变量 `RUTIS_CHANNEL_TOKEN` 的值加换行，随即从环境删除（Windows 用这个） |

- 远程通道：`listen:wss://…`（等 rutis 拨入）与 `dial:wss://…`（连到 rutis 的接入点，设计已定、实现中）；WebSocket 子协议 `rutis.3`；非回环必须 TLS（`RUTIS_CERT` / `RUTIS_KEY`），凭据 `RUTIS_TOKEN` 常数时间比较；30 秒心跳（`RUTIS_HEARTBEAT` 可调）；一次只服务一个控制方，新连接接管旧连接。
- 进程参数约定：`<channel> [--id <endpoint>] [--peer <endpoint>] <project>`。

## 2. 握手

| 格式 | 用在 | 握手 |
| --- | --- | --- |
| compat，协议 2 | 本地通道 | `{"op":"hello","version":2,"capabilities":[…]}` |
| endpoint，协议 3 | 远程 | 带本端 / 对端端点 id、实现名与版本、能力列表；作为远程运行时还要声明能力 `runtime`（rutis 侧 link 带 `runtime` 时会在握手后校验，缺少即报 `incompatible` 并停止） |

### 能力（如实声明，不推断）

| 能力 | 声明它的条件 | 不声明的后果 |
| --- | --- | --- |
| `sync-wait` | 发同步调用时在帧上带 `sync: true`，也接受对端的这个字段 | 你发出的同步调用不在宿主的等待环检测范围内，可能卡死 |
| `reentrant-sync` | 同步等待期间仍执行**任何**进来的调用 | 按不可重入看待（只执行本调用链上的调用，其余延后） |
| `sync-stack` | 单线程单栈：等待期间执行的调用压在等待的调用之上（后进先出） | 只参与调用链边（每个调用一个执行单元的运行时不声明） |
| `signals` | endpoint 格式的信号帧；现有所有实现（Rust、Node、Python、Bun、Go）都声明 | — |
| `objects` | 能接收对端发来的对象引用 | 只能发引用，不能收 |

选择依据：每个进来的调用一个执行单元（线程 / goroutine / Task / 虚拟线程）→ `sync-wait` + `reentrant-sync`，不声明 `sync-stack`；Python、Bun 这类单线程事件循环且等待时执行进来的调用 → 三者都声明；Node 不可重入 → 不声明 `reentrant-sync`。

## 3. `mount`

rutis 第一个控制操作。回复：

```json
{
  "services": {},
  "features": ["rows.v2", "hosts", "leaf", "scopes", "events"],
  "implementation": { "name": "<包名>", "version": "<SDK 版本>" },
  "engine": { "name": "<语言>", "version": "<语言 / VM 版本>" }
}
```

| 特性 | 含义 |
| --- | --- |
| `rows.v2` | 支持下面的行操作 |
| `hosts` | 支持 `hosts.provide` / `hosts.withdraw` |
| `leaf` | 叶子运行时：插件的全部 `inject` 由 rutis 门控 |
| `scopes` | 服务 id 可带实例标签：`name` 或 `name` + NUL + `label` |
| `events` | 叶子事件（插件 API 2）：`ctx.on` / `ctx.emit`，以及宿主向运行时发事件；宿主配置了 `emits` 而运行时没有 `events` 时挂载失败 |

`mount` 回复里增加字段（如 `engine`、`plugins`）不算协议变化。

## 4. 控制操作

| 操作 | 行为 |
| --- | --- |
| `rows.schema(entry)` | `{ config, inject, provides, version }`：配置 JSON Schema、用到的服务名、提供的服务名与每个方法的 `sync` / `async`；未知 entry 报错并列出可用的名字 |
| `rows.load(key, entry, config, isolate, inject, exports)` | 解析 entry → 检查插件 API 版本（插件比运行时新则拒载并报两边版本）→ 解码配置 → `apply`；`ctx.provide` 的服务在回复前以 `service(id, handle, version)` 通知；失败先卸载该行再报错 |
| `rows.update(key, config)` | 叶子插件没有 volatile 字段：卸载后重装 |
| `rows.unload(key)` | 先撤 provides（`service(id, None, version)`）→ 取消该行的任务 → 逆序清理 |
| `hosts.provide(name, methods, label?)` | 登记一个 rutis 服务代理，供插件 `ctx.use`；调用目标 `host:<id>` |
| `hosts.withdraw(id)` | 撤销代理 |
| `release` / `get` | 归还导出对象 / 读导出对象的字段 |
| `dispose` | 卸载全部行，等在途调用排空 |

- 会话内本端发起的调用号前缀 `node:`（所有叶子运行时相同）；调用链 `path` 跨会话由 Rust 侧改写。
- 导出槽：`ctx.provide` 每次给新 handle（`name` 第 1 代，之后 `name#N`）。
- 会话结束（rutis 没了）：卸载全部行，清理受 `RUTIS_SHUTDOWN_TIMEOUT`（默认 10 秒）截止，超时在 stderr 说明并以退出码 1 退出。

## 5. 值

与插件 API 的"值怎样传递"一致（见 `plugin-api.md`）：JSON 兼容数据复制；函数、Promise / 协程、带方法的对象按引用；整数 ±(2^53−1)；嵌套 ≤ 64 层；NaN、±Infinity、二进制等不可传。超限由**发送方**在发出前拒绝，只让这次调用失败，会话继续。错误跨边界带名字、消息与完整错误图（cause、聚合错误的子错误）。

## 6. 编译成程序的运行时（Go 模型）额外约定

- 可执行文件响应 `--rutis-manifest`：stdout 打印清单后退出（不连通道、不执行 `apply`）。

  ```json
  { "manifest": 1, "sdk": "<版本>", "pluginApi": 2, "runtime": "rutis-<语言>-runtime:1",
    "platform": "<os>/<arch>", "plugins": { "<名字>": { "config": {}, "inject": [], "provides": {}, "version": null } } }
  ```

- 文件内容里含标记串 `rutis-<语言>-runtime:1`，rutis 扫描目录时只执行含它的文件；插件条目与 `rows.schema` 回复同一段代码生成。
- 一个文件里插件同名：启动即退出并指明两个定义；插件只在 `serve(...)` 参数里登记，不用静态注册表。

## 7. 一致性测试（契约本身）

rutis 仓库的 Rust 测试拉起你的运行时，逐条比对：

| 测试 | 验证 |
| --- | --- |
| `rutis-bridge/tests/runtime_conformance.rs` | `session::testing::session`：引用、回调、错误、malformed 帧终结会话 |
| `rutis-bridge/tests/session_matrix.rs` | 运行时契约 × 每种通道 |
| `rutis-loader/tests/multilang*.rs` | 多语言冷启动互调、撤提供者只停使用者、`inject` 门控、交叉同步调用不卡死 |
| `rutis-bridge/tests/websocket_conformance.rs` | 远程监听 |
| 测试工具期望文件 `crates/rutis-bridge/src/runtime/testing/weather.expected.json` | SDK 的无宿主测试工具与真实运行时结果一致 |
