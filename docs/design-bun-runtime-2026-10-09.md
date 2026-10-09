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

**Bun 不是一种新语言。** 它和 Node 跑同一个运行时包（`runner.mjs`）、同一份协议、同一个 SDK（`@arcships/rutis`）。本设计不新增运行时种类，而是给 JS 运行时加一个**引擎**选项，并让一个宿主可以有多个具名的本地 JS 运行时，Node 与 Bun 可以并存（§4）。

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
pub enum Engine { Node, Bun }

Launcher::node(package)            // 不变：node --import tsx <package>/src/runner.mjs
Launcher::bun(package)             // 新增：bun --no-install <package>/src/runner.mjs
Launcher::js(package, engine)      // 新增：按 engine 选择以上两者之一

LocalRuntime::node(package, anchor)              // 不变：名为 "node"，引擎 Node
LocalRuntime::js(name, engine, package, anchor)  // 新增：任意名字、任意引擎
```

- **`program`**：默认在 `PATH` 上找 `node` / `bun`（Windows 上带 `.exe`）；可以指定路径，用于固定版本或使用项目内的 Bun。
- **fd:3**：引擎为 Bun 时，只有运行时包的 `rutisChannels` 含 `"fd"` **并且**声明支持 Bun（§3.2）才走 fd:3 继承，否则退回 dial-back（Unix）或 loopback（Windows），旧包不会在 Bun 下静默失效。
- **`RUTIS_JS_ENGINE`**：设为 `bun` 时，没有明确写引擎的 JS 运行时都用 Bun 启动。这是给测试矩阵和临时试用的开关，与 `RUTIS_LOCAL_HANDOVER=loopback`（`transport/local/spawn.rs:101`）同一做法；配置里写明的引擎优先。
- **不新增 cargo feature**，都在 `node` feature 下。

### 3.2 运行时包的声明

`@arcships/rutis-runtime` 的 package.json 增加：

```json
"rutisEngines": ["node", "bun"]
```

宿主用 Bun 启动前读取它；没有声明 `bun` 的包（0.8 及更早）直接报错"@arcships/rutis-runtime <版本> does not support Bun; install 0.9 or later"，不尝试启动。运行时启动时检查 Bun 版本，低于支持的最低版本（暂定 1.3）时在问候前以明确的错误退出，宿主报 `exited before connecting` 并带上这条错误。

### 3.3 JS 运行时包

1. **`channel/fd.mjs`**：`process.versions.bun` 下用 `net.connect({ fd })`。
2. **`runner.mjs` 的 `fresh()`**：Bun 下入口文件变化时，删除 `require.cache` 中 `realpathSync(entry)` 一项再导入。重载只换入口模块、它导入的模块仍用缓存，与 Node 下用查询串的语义相同。
3. **诊断**：运行时在描述自己时带上引擎和版本（如 `bun 1.3.14`），`rutis-host check` 和 `diagnostics()` 列出每个运行时的名字、引擎与版本——设计哲学 §1 的"认识连接"。
4. **WebSocket 客户端**：见 §5。

## 4. 多个 JS 运行时并存

一个宿主可以有**任意多个具名的本地 JS 运行时**，各自选择引擎和项目。Node 与 Bun 并存只是其中一种情况；同一引擎开多个实例用于隔离（设计哲学 §6"隔离意味着更多运行时实例"）也是。

### 4.1 行属于哪个运行时

沿用 Python 行与远程运行时已有的前缀规则（`<运行时名>:<模块>`）：

| 行的 `name` | 运行在 |
| --- | --- |
| `@foo/weather`、`./plugin.ts`（不带前缀） | **默认 JS 运行时** |
| `bun:@foo/weather`、`bun:./plugin.ts` | 名为 `bun` 的运行时 |
| `sandbox:@foo/weather` | 名为 `sandbox` 的运行时 |

- **默认 JS 运行时**：配置里标了 `"default": true` 的那个；没有标时，名为 `node` 的那个；只有一个本地 JS 运行时时，就是它。都不满足时，不带前缀的行无效（`Unresolved`），错误列出可用的前缀。
- npm 包名不含 `:`，Windows 盘符只有一个字母，运行时名要求至少两个字符，不会混淆。
- 远程 JS 运行时（`remote`，`language: "node"`）的解析方式不变。
- `RuntimeResolver::node(handle)` 解析不带前缀的名字和 `<名字>:` 前缀；新增 `RuntimeResolver::node_prefixed(handle)`，只解析 `<名字>:` 前缀。实现上是给 `Naming::Npm` 加一个可选前缀（`crates/rutis-loader/src/runtime.rs:69`）。

### 4.2 配置

`runtimes.node` 保持不变（名为 `node`、引擎 Node）；新增 `runtimes.js` 列表：

```json
{
  "runtimes": {
    "node": { "project": "." },
    "js": [
      { "name": "bun", "engine": "bun", "project": "." },
      { "name": "sandbox", "engine": "node", "project": "./sandbox" }
    ]
  },
  "rows": [
    { "id": "weather", "name": "@foo/weather" },
    { "id": "report", "name": "bun:./report.ts", "inject": ["weather"] }
  ]
}
```

| 字段 | 默认 | 说明 |
| --- | --- | --- |
| `name` | 必填 | 运行时名，也是行名前缀；至少两个字符，不能与 `py`、远程运行时重名 |
| `engine` | `"node"` | `"node"` 或 `"bun"` |
| `project` | `.` | 插件包从这里的 `package.json` 解析；几个运行时可以共用一个项目 |
| `runtime` | 项目里的 `@arcships/rutis-runtime` | 同 `runtimes.node` |
| `program` | 在 `PATH` 上找 | 引擎的可执行文件 |
| `default` | `false` | 不带前缀的行跑在这里 |

只用 Bun 的项目写一个 `{ "name": "bun", "engine": "bun" }` 即可，它是唯一的 JS 运行时，自然是默认。

### 4.3 并存时的语义

- **服务照常跨运行时共享**：`bun` 里提供的 `weather`，`node` 里的插件 `inject` 即可，与跨语言服务走同一条路（`host_key`），依赖声明不变（原则 8）。
- **Cordis 的原生共享只在一个运行时内**：每个运行时有自己的 Cordis Context。依赖 Cordis 进程内特性互相配合的插件（同组挂载、`ctx.set` 直接替换等）要放在同一个运行时；跨运行时就按需求文档 §5 的边界规则，与跨进程相同。
- **故障范围按运行时划分**：一个运行时崩溃只撤回它的服务，其他运行时的插件按依赖规则等待，不受牵连。
- **加载与资源**：每个运行时一个进程；运行时没有行时是否退出、按需启动，沿用现有运行时的行为，不在本设计改变。

### 4.4 插件项目与 rutis-host

- `rutis-host new --lang node` 生成的项目不变；完成提示补一句用 Bun 时的写法（`bun install`，`runtimes.js` 加 `{ "name": "bun", "engine": "bun" }`）。不新增 `--lang bun`：插件代码相同。
- 模板里的 `tsx` 依赖在 Bun 下用不到，但保留，同一个项目两种引擎都能跑。
- `rutis-host dev`：开发中的插件跑在默认 JS 运行时；`rutis.dev.json` 可以用同样的 `runtimes` 指定引擎。
- `bunx @arcships/rutis-host`：平台二进制包靠 `optionalDependencies` + `os` / `cpu`，Bun 支持但未验证，归入 S9（[#193](https://github.com/arcships/rutis/issues/193)）。
- `bun install` 的隔离链接布局下，插件与运行时解析到的 Cordis 是否同一份未验证（Bun 按 realpath 解析，预计一致），由 §6 的 loader 行测试覆盖。

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

用测试把上面的承诺钉住。引擎相关的都是在现有测试上加一列 Bun，不另写一套；并存相关的是新增测试。

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
| 并存 | `rutis-loader/tests/runtime_rows.rs` 新增 | 同一宿主里 `node`、`bun` 两个运行时：不带前缀与 `bun:` 前缀的行各自落在对应运行时；`bun` 提供的服务被 `node` 里的插件使用，反之亦然；杀掉 `bun` 进程只撤回它的服务，`node` 的行按依赖等待；两个同引擎实例互相隔离 |
| 默认运行时 | `rutis-host` 的 config / host 测试新增 | 默认规则（`default`、名为 `node`、唯一一个）；多个候选且未指定时不带前缀的行无效，错误列出可用前缀；运行时名重名、短于两个字符时配置报错 |
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

## 9. 分阶段

| 阶段 | 内容 |
| --- | --- |
| B1 | §4：多个具名本地 JS 运行时并存（与引擎无关，只用 Node 也有用：隔离）及其测试 |
| B2 | §3：Bun 引擎、运行时包声明与修补；§5 第一阶段；§6 中引擎相关的测试；CI `runtimes-bun`（Linux、macOS） |
| B3 | §5 第二阶段：Bun 下拨出 WebSocket |
| B4 | Windows；性能数据；`bunx` 安装冒烟（并入 S9） |
