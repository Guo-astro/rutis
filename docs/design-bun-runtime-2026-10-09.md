# Bun 作为 JS 插件运行时

[English](design-bun-runtime-2026-10-09.en.md)

设计提案 · 关联 [#194](https://github.com/arcships/rutis/issues/194)

## 1. 背景

TS / JS 插件和 Cordis 插件都在 Node 运行时里跑：`@arcships/rutis-runtime` 的 `src/runner.mjs`，由宿主用 `node --import tsx` 启动（`Launcher::node`，`crates/rutis-bridge/src/runtime/process.rs`）。

Bun 也是 JS 生态里常见的运行时。它原生运行 TS，不需要 tsx；启动更快；有些项目只装了 Bun，或者依赖用 `bun install` 管理。这类项目想用 rutis，目前必须另装一份 Node。

按[设计哲学](design-philosophy.md) §7 的判断标准：

1. Bun 让宿主多接上一类环境（只有 Bun 的项目和机器）。
2. 它可以完全放在核心之外：启动器和运行时包改动，核心不动。
3. 它还是同一个协议，跨进程边界的规则不变。
4. 有具体用户：只用 Bun 的 TS 项目。
5. 它的保证可以用现有的契约测试来钉住。

**Bun 不是一种新语言。** 它和 Node 跑同一个运行时包（`runner.mjs`）、同一份协议、同一个 SDK（`@arcships/rutis`）。本设计不新增运行时种类，只给 Node 运行时加一个**引擎**选项。

## 2. 调研结论

2026-10-09 在 macOS 上用 Bun 1.3.14 做了验证。方法是用一个 `node` 垫片把运行时换成 `bun --no-install`，并在运行时包的临时副本里做了下表中的修补，仓库本身未改动。

**结果：**

- `cargo test -p rutis-bridge --all-features` 只有 1 个测试失败：`websocket_cross::node_dialing_rust_is_refused_by_category`，原因见下表最后一行。通过的包括 runtime_conformance、session_matrix、node_conformance、process_exit、live_objects、cancellation、group_mount，以及 websocket_conformance、link、multihop。
- `rutis-loader` 的 runtime_rows、multilang、cordis_node 和 instance_* 都通过。
- `rutis-host` 的 8 个测试都通过。

以下两项在 Bun 下直接可用：

- **同步调用机制**：主线程用 `worker_threads`、SharedArrayBuffer、`Atomics.wait`、`receiveMessageOnPort`，worker 崩溃时也能在阻塞等待中报出。
- **Cordis 4.0.4**。

**启动耗时**（10 次取中位数）：`node --import tsx` 85ms，`node` 36ms，`bun` 29ms。吞吐和内存没有测。

下表是不兼容的地方：

| 依赖 | 位置 | Bun 下的表现 | 处理 |
| --- | --- | --- | --- |
| `new net.Socket({ fd: 3 })` | `channel/fd.mjs:11` | **静默失效**：不报错，读不到数据，进程自行退出 | Bun 下改用 `net.connect({ fd })`（已验证）。Node 不接受这种写法，所以按 `process.versions.bun` 分支 |
| 热重载 `import(href + '?rutis-reload=…')` | `runner.mjs:205-214` | **查询串被忽略**，一直返回旧模块（.mjs 和 .ts 都是） | 先删 `require.cache` 中入口文件的 realpath 一项，再导入（已验证）。必须用 realpath，否则 macOS 上 `/var` 与 `/private/var` 对不上 |
| `createRequire(anchor).resolve('@deepseek-ai/cordis')` | `runner.mjs:33,38,174`、`bridge/features.mjs:123` | **自动安装**：目录里没有 node_modules 时，Bun 从全局缓存解析出另一份 Cordis，runtime_rows 有 10 个测试失败 | **始终带 `--no-install` 启动**（已验证）。这同时是安全要求：不加的话，插件在运行时可以下载包 |
| `--import tsx` | `process.rs:167-170` | 不需要。enum、参数属性、装饰器、`./a.js` 指向 `a.ts` 都能直接运行 | Bun 启动器不加 tsx |
| `ws` 客户端的 `upgrade` / `unexpected-response` 事件、`maxPayload`、`ca` | `channel/websocket.mjs:101-128` | Bun 用内置实现替换 `ws`，不触发这些事件。遇到 401 时既没有事件也没有错误，连接一直挂起 | 见 §5 |
| 服务端 `WebSocketServer`、listen:ws、`serve.mjs` 用 `spawn(process.execPath, execArgv)` 拉起子进程 | 多处 | 正常；`execArgv` 会保留 `--no-install` | 无需处理 |
| Windows：loopback TCP 加一次性 token | `channel/tcp.mjs` | 未验证 | 见 §7 |

## 3. 引擎

### 3.1 Rust 侧

```rust
Launcher::node(package)          // 不变：node --import tsx <package>/src/runner.mjs
Launcher::bun(package)           // 新增：bun --no-install <package>/src/runner.mjs
Launcher::js(package, Engine)    // 新增：按 Engine::{Node, Bun} 选择以上两者之一
LocalRuntime::bun(package, anchor)  // 新增：运行时名仍为 "node"
```

- **运行时名和行名不变。** `LocalRuntime::bun` 提供的仍是 `Runtime#node`，未加前缀的 npm 行名照旧由 `RuntimeResolver::node` 解析。插件和配置不感知自己跑在 Node 还是 Bun 上，这符合原则 8：依赖声明只含服务名。一个宿主同一时间只有一个 `node` 运行时，引擎二选一；如果确实需要两个引擎并存，用远程运行时（`remote`）挂第二个。
- **`program`**：默认在 `PATH` 上找 `bun`（Windows 上是 `bun.exe`）；也可以指定路径，用于固定版本或使用项目内的 Bun。
- **fd:3**：引擎为 Bun 时，必须同时满足下面两点才走 fd:3 继承：
  - 运行时包的 `rutisChannels` 含 `"fd"`；
  - 运行时包声明支持 Bun（§3.3）。

  不满足就退回 dial-back（Unix）或 loopback（Windows），旧包因此不会在 Bun 下静默失效。
- **`RUTIS_JS_ENGINE`**：设为 `bun` 时，`Launcher::node` 改用 Bun 启动。这是给测试矩阵和临时试用准备的开关，做法与现有的 `RUTIS_LOCAL_HANDOVER=loopback`（`transport/local/spawn.rs:101`）一致。配置里写明的引擎优先于这个变量。
- **不新增 cargo feature。** 改动都在 `node` feature 下。

### 3.2 配置

`rutis.json` 的 `runtimes.node` 增加两个字段，见 `crates/rutis-host/src/config.rs:69` 的 `NodeRuntime`：

```json
{
  "runtimes": {
    "node": { "project": ".", "engine": "bun", "program": "/opt/bun/bin/bun" }
  }
}
```

| 字段 | 默认 | 说明 |
| --- | --- | --- |
| `engine` | `"node"` | `"node"` 或 `"bun"` |
| `program` | `node` 或 `bun`，在 `PATH` 上查找 | 引擎的可执行文件 |

**远程运行时**：在另一台机器上用 Bun 跑 `@arcships/rutis-runtime` 的 serve，`remote` 的 `language` 仍写 `"node"`。语言和协议都相同，引擎是对端自己的选择。

### 3.3 运行时包的声明

`@arcships/rutis-runtime` 的 package.json 增加：

```json
"rutisEngines": ["node", "bun"]
```

- 宿主用 Bun 启动之前先读这个字段。没有声明 `bun` 时（比如 0.8 及更早的包）直接报错：

  ```
  @arcships/rutis-runtime <版本> does not support Bun; install 0.9 or later
  ```

  不会尝试启动。
- 运行时启动时检查 Bun 的版本。低于支持的最低版本（暂定 1.3）时，在问候之前以明确的错误退出，宿主报 `exited before connecting`，附带这条错误。

### 3.4 JS 运行时包

1. **`channel/fd.mjs`**：在 `process.versions.bun` 下用 `net.connect({ fd })`。
2. **`runner.mjs` 的 `fresh()`**：在 Bun 下，入口文件变化时删除 `require.cache` 里 `realpathSync(entry)` 这一项，再导入。重载只换入口模块，入口导入的其他模块继续用缓存，这与 Node 下用查询串的做法一致，语义不变。
3. **诊断**：运行时在描述自己时带上引擎和版本（如 `bun 1.3.14`）。`rutis-host check` 的输出和 `diagnostics()` 能看出某个运行时跑在什么引擎上，对应设计哲学 §1 的"认识连接"。
4. **WebSocket 客户端**：见 §5。

## 4. 插件项目与 rutis-host

- **`rutis-host new --lang node`**：生成的项目不变。完成提示里补一句"用 Bun 时：`bun install`，并在 rutis.json 里设 `engine: "bun"`"。不新增 `--lang bun`，因为插件代码完全相同。
- **`tsx` 依赖**：在 Bun 下不会用到，但保留在模板里，让同一个项目两种引擎都能跑。
- **`bunx @arcships/rutis-host`**：平台二进制包通过 `optionalDependencies` 加上 `os` / `cpu` 选择，Bun 支持这种写法，但还没验证。归入 S9（[#193](https://github.com/arcships/rutis/issues/193)）的安装冒烟测试。
- **`bun install` 的隔离链接布局**：在这种布局下，插件解析到的 Cordis 和运行时解析到的 Cordis 是否是同一份，还没验证。Bun 按 realpath 解析，预计一致。由 §6 的 loader 行测试覆盖。

## 5. WebSocket 客户端

只有 JS 侧主动拨出时才会用到 `ws` 客户端：

- 运行时以 `ws://` 或 `wss://` 作为通道去拨号（`io-worker.mjs:47`）；
- Cordis 应用作为节点拨向 rutis 节点（`bridge/worker.mjs:61`）。

宿主拉起的本地运行时走 fd、unix 或 tcp，不经过这里。运行时作为远程运行时监听（`serve`）时用的是服务端，在 Bun 下是好的。

**本设计的处理分两步：**

1. **第一阶段：明确报错。** 在 Bun 下拨 `ws://` 或 `wss://` 时，直接以 `ConnectError('incompatible', 'dialing a WebSocket is not supported on Bun yet')` 失败，不进入挂起状态。文档写明 Bun 支持本地通道和监听，暂不支持拨出。这符合原则 7：跨不过边界的东西写成规则，而不是静默降级。
2. **第二阶段：实现拨出。** 拒绝分类依赖握手响应的状态码，有两种做法：
   - 用 `node:https` 或 `node:http` 自己发 Upgrade 请求，拿到 101 之后把 socket 交给 `ws` 的 `WebSocket.setSocket` 风格的接口。需要验证 Bun 是否支持。
   - 先用 `fetch` 带同样的头做一次预检，读出 401、403、426 等状态码，再用原生 `WebSocket` 连接。代价是多一次往返，而且有竞态（预检成功不代表后续连接成功）。

   `maxPayload` 由我们自己的分帧层在收到消息时检查。`ca` 走 Bun 的 `tls` 选项。

   第二阶段单独开 PR。它的验收标准是 `websocket_cross` 在 Bun 下通过。

## 6. 测试

用测试把上面的承诺钉住。都是在现有测试上加一列 Bun，不另写一套。

| 层 | 测试 | 改动 |
| --- | --- | --- |
| 运行时契约 | `rutis-bridge/tests/session_matrix.rs` | Node 列按引擎参数化，增加 {Bun} × {fd:3、dial-back、WebSocket listen} |
| 会话契约 | `runtime_conformance.rs` | 增加 Bun 端点 |
| loader 行 | `rutis-loader/tests/runtime_rows.rs`、`multilang.rs`、`instance_runtimes.rs`、`cordis_node.rs` | 在 CI 里设 `RUTIS_JS_ENGINE=bun` 整体再跑一遍（做法同 `RUTIS_LOCAL_HANDOVER`） |
| 宿主 | `rutis-host` 的 `a_reloaded_row_runs_the_edited_plugin` | 增加 Bun 版本，这是热重载修复的回归测试 |
| 启动器 | 新增 | 声明不支持 Bun 的运行时包会被拒绝，并给出提示；Bun 版本过低时报错；`--no-install` 始终存在 |
| 安全 | 新增 | 在没有 node_modules 的目录里引用一个未安装的包时，加载失败，不会自动下载 |
| WebSocket | `websocket_cross.rs` | 第一阶段断言 Bun 拨出以 `incompatible` 失败而不是挂起；第二阶段改为与 Node 一样的分类断言 |
| JS 单元测试 | `node/rutis-runtime/test` | 同时用 `bun test` 运行。现在 channel 和 websocket 各有 2 个失败、handshake 超时，原因未全部确认，需要逐个处理或标注 |
| E2E | S2 [#186](https://github.com/arcships/rutis/issues/186)、S3 [#187](https://github.com/arcships/rutis/issues/187) | 各加一个 Bun 变体 |

**CI**：新增 `runtimes-bun` job，在 Linux 和 macOS 上用 `oven-sh/setup-bun` 固定版本安装 Bun，然后：

- `cargo test -p rutis-bridge --all-features`，Bun 列；
- `RUTIS_JS_ENGINE=bun cargo test -p rutis-loader --features node,python,peer`；
- `cargo test -p rutis-host`；
- `bun test`。

Windows 等 §7 第 2 条验证之后再加入。

## 7. 待定

1. **支持的 Bun 版本范围。** 最低暂定 1.3；Bun 发版快，Node 兼容性会随版本变化。CI 固定一个版本，加上一个跟踪最新版的 nightly。
2. **Windows。** `kill_on_drop`、job object 收养、loopback TCP 交接在 Bun 下都还没验证。
3. **性能。** 目前只有启动耗时的数据。同步调用的往返（Node 侧约 30µs）和吞吐要在 Bun 下测，结果写进 [performance-event-dispatch](performance-event-dispatch-2026-09-27.md) 一类的文档。
4. **`bun test` 与 `node:test` 的差异。** JS 单元测试是否要改成两边都能跑的写法。

## 8. 不做

- **Deno。** Deno 的 Node 兼容层和权限模型差别更大，有需求时另行评估。
- **Bun 专有 API**（`Bun.spawn`、`Bun.serve` 等）。运行时包只用 Node 兼容 API 加少量分支，两种引擎共用一份代码。
- **同一个宿主里同时有 Node 和 Bun 两个本地 `node` 运行时。** 需要时用远程运行时。

## 9. 分阶段

| 阶段 | 内容 |
| --- | --- |
| B1 | §3、§3.4 的 1–3 条、§5 第一阶段，以及 §6 中除 WebSocket 第二阶段以外的测试，加 CI `runtimes-bun`（Linux、macOS） |
| B2 | §5 第二阶段：Bun 下拨出 WebSocket |
| B3 | Windows；性能数据；`bunx` 安装冒烟（并入 S9） |
