---
name: rutis-new-runtime
description: Use when adding support for a new plugin language or platform to rutis — "add a runtime", "新增运行时", "support Kotlin/Swift/Ruby/… plugins", writing a new leaf SDK, or making an existing runtime work remotely. Covers whether to do it at all, the design doc, the framework constraints, the decisions every runtime must state (artifact model, reentrancy and capabilities, channels, method enumeration, distribution), the runtime contract, and the checklist of places in the rutis repository a runtime touches, based on how Bun and Go were added. Self-contained; reference material is in references/.
---

# 为 rutis 添加一个语言运行时

rutis（https://github.com/arcships/rutis）用 Rust 写核心，其他语言的插件通过**运行时**接入。本 skill 不依赖 rutis 仓库的文档，需要的参考都在 `references/`：

| 文件 | 内容 |
| --- | --- |
| `references/principles.md` | rutis 的设计原则与"该不该做"的判断标准 |
| `references/plugin-api.md` | 所有语言共用的插件 API（插件作者看到的那一面） |
| `references/runtime-contract.md` | 运行时要实现的契约：帧与通道、握手与能力、`mount`、控制操作、值、清单、一致性测试 |

核心认识：rutis 的范式（依赖门控、生命周期、配置对账）**只在 Rust 内核里实现一份**。运行时与它的 SDK 是很薄的叶子层，只负责把一种语言的插件接上。想在新语言里再实现一遍范式，就是走错了方向。

## 0. 先判断该不该做

按 `references/principles.md` 的判断标准：

- 它带来什么生态能力？接一种运行时是为了生态，不是为了多一种语言。
- 有具体的使用方吗？没有就先不做。Go 和 Kotlin 桌面 JVM 都是"有需求时再做"。
- 已有运行时能覆盖吗？比如 TS 插件已经能在 Node 和 Bun 里跑。

## 1. 先写设计稿，再写代码

- 在 rutis 仓库写 `docs/design-<语言>-runtime-<日期>.md` 与 `.en.md`，以中文为准。结构可参考：
  - 编译成程序的语言：https://github.com/arcships/rutis/blob/main/docs/design-go-runtime-2026-10-10.md
  - 按名字装载的语言：https://github.com/arcships/rutis/blob/main/docs/design-bun-runtime-2026-10-09.md
  - 带应用托管的语言：https://github.com/arcships/rutis/blob/main/docs/design-swift-sdk-2026-10-12.md
- 决策表要写定，不留"待定"；有明显默认答案的直接定下来，并写明理由。
- 写一节"与总体稿的出入"。合并时同步更新多语言总体稿的里程碑和 §九（可重入的进展）。

## 2. 框架约束

| 约束 | 含义 |
| --- | --- |
| 叶子插件 | SDK 只提供 `define`（`inject`、`provides` 及方法形状、配置 schema、`apply`）、`ctx.use` / `ctx.provide` / `ctx.effect` / `ctx.on` / `ctx.emit`、取消、错误。不提供子插件、事件树、本地依赖图 |
| rutis 驱动 | 运行时不决定依赖是否满足、启动顺序、重启策略；全部 `inject` 由 rutis 门控（特性 `leaf`） |
| 同一个插件 API | 与 `references/plugin-api.md` 一致：声明、ctx、生命周期、清理顺序、截止时间、值规则、插件 API 版本（当前为 2） |
| 平台语言实现 | 运行时和 SDK 全部用该语言实现，**不依赖 Rust**（不用 FFI 或原生扩展去桥接 rutis） |
| 两个方向 | 本地通道必须有；要支持远程时，`listen:` 和 `dial:` 两个方向都实现 |
| 不碰业务层 | 平台前后台、设备、身份、存储都不进运行时 |
| 测试即契约 | 必须通过 rutis 仓库 Rust 侧的一致性测试，只有自己的单测不够 |

## 3. 每个运行时都要回答的决策

| 决策 | 选项与依据 |
| --- | --- |
| 产物模型 | **按名字装载**：一个运行时进程装载多个已安装的包（Node、Bun、Python），行名 `<前缀>:<模块>`。**编译成程序**：插件编进可执行文件，用 `--rutis-manifest` 自描述，按需启动（Go；Swift 在 macOS 上复用这套机制）。**应用托管**：插件编进应用，由应用连到 rutis（Swift / Kotlin 应用） |
| 行名前缀 | 不得与已有前缀冲突：`py`、`bun`、`go`、`swift`、`kt` 等 |
| 执行模型 | 每个进来的调用一个执行单元（线程、goroutine、Task、虚拟线程）就是可重入的；读线程只解析帧，不跑插件代码 |
| 能力声明 | 按 `references/runtime-contract.md` §2 如实声明 `sync-wait`、`reentrant-sync`、`sync-stack`、`objects`。新运行时应当声明 `sync-wait`，否则不在等待环检测范围内 |
| 通道 | 本地支持 `fd:3`、socket 路径拨回、`tcp:127.0.0.1:<port>`（Windows 用这个）；语言拿不到继承的 fd 时用拨回。远程支持 `listen:wss://` 和 `dial:wss://` |
| 方法枚举与 schema | 语言有反射就自动派生（Go、Python），没有就用显式方法表加手写 schema（Swift、Kotlin） |
| 分发 | 用该语言的标准包管理发布（npm、PyPI、Go 模块、SwiftPM、Maven Central），版本跟随 rutis 的发布列车 |

## 4. 运行时契约

完整内容见 `references/runtime-contract.md`。实现时最快的办法是对照一个现有实现读：

- Python：https://github.com/arcships/rutis/tree/main/python/rutis/rutis（`peer.py`、`runner.py`、`__main__.py`、`websocket.py`）
- Go：https://github.com/arcships/rutis/tree/main/go/rutis（`serve.go`、`listen.go`、`runtime.go`、`plugin.go`）
- Bun：https://github.com/arcships/rutis/tree/main/bun/rutis-bun/src

## 5. rutis 仓库里要改的地方

下面的清单按 Bun（arcships/rutis#200）和 Go（arcships/rutis#227）实际改动的文件整理。

**语言侧**
- [ ] `<语言>/…`：会话层、SDK、运行时入口、无宿主测试助手（如 Go 的 `rutistest`、Python 的 `rutis.testing`）
- [ ] 一致性测试入口（供 Rust 测试拉起）

**rutis-bridge**
- [ ] `src/runtime/process.rs`：`Launcher::<语言>`（命令、工作目录、通道交接方式）
- [ ] `src/runtime/local.rs`：`LocalRuntime::<语言>` 与运行时名
- [ ] `src/runtime/mod.rs`；`Cargo.toml` 加 feature
- [ ] 测试：`tests/runtime_conformance.rs`、`tests/session_matrix.rs`、`tests/<语言>_runtime.rs`；支持远程时加 `tests/websocket_conformance.rs`（拨出另有 `dial_conformance.rs`）

**rutis-loader**
- [ ] 按名字装载：`src/runtime.rs`（`RuntimeResolver`）。编译成程序：`src/runtime/<语言>.rs`，复用 Go 的清单机制
- [ ] `src/lib.rs`；`Cargo.toml` 加 feature
- [ ] 测试：`tests/<语言>_rows.rs`、`tests/multilang_<语言>.rs`、`tests/instance_<语言>.rs`、`tests/leases.rs`

**rutis-host**
- [ ] `src/config.rs`：新增 `runtimes.<语言>` 字段
- [ ] `src/host.rs`：挂载分支、预检解释器或工具链版本、`runtimes.remote[].language` 的映射
- [ ] `src/main.rs`、`src/project.rs`、`src/new.rs`：`check`、`dev`（宿主不编译，只调用项目自己的构建）、`new --lang <语言>`
- [ ] `templates/<语言>/`：插件、测试、`rutis.dev.json`、发布 workflow

**发布与 CI**
- [ ] `.github/workflows/ci.yml` 加 `runtimes-<语言>` job；`release.yml`
- [ ] `scripts/train.mjs`：版本文件与包坐标

**文档（`.md` 与 `.en.md` 都要改）**
- [ ] `docs/guide/<语言>-plugin.md`；`docs/guide/plugin-api.md` 加一列；`docs/guide/rutis-host.md`；涉及远程时改 `docs/guide/nodes.md`
- [ ] `README` 的包表；`CONTRIBUTING` 的工具链说明
- [ ] 多语言总体稿：里程碑与 §九

## 6. 分阶段（照 Go / Swift 的节奏）

1. 设计稿合并。
2. 会话层与一致性测试入口：通过 `runtime_conformance`。
3. 运行时契约、SDK 与 Rust 接入：通过 `<语言>_runtime`、`<语言>_rows` 以及 `multilang` 里的新列；现有测试全部保持通过。
4. 开发流程：`new`、`dev`、`check`、模板、发布。
5. 远程：`listen:` 与 `dial:`，通过 `websocket_conformance` 和 `dial_conformance`。

## 7. 常见错误

| 错误 | 正确做法 |
| --- | --- |
| 在 SDK 里实现依赖等待、重试、启动顺序 | 交给 rutis 门控；SDK 只执行 `rows.load` / `unload` |
| 用 FFI 嵌入 Rust 来省掉协议实现 | 用平台语言实现；靠一致性测试保证和其他语言行为一致 |
| 能力不声明或乱声明 | 按执行模型如实声明 `reentrant-sync`、`sync-stack`、`sync-wait` |
| 在读线程上跑插件代码，或在读线程上做同步桥接 | 每个进来的调用一个执行单元 |
| 宿主替插件编译代码 | `dev` 只调用项目自己的构建；`run` 只运行现成产物 |
| 叶子事件跨进程转发 | 事件只在本运行时进程内分发；宿主 → 运行时由 `emits` 声明，运行时 → 宿主暂不可用 |
| 文档只写中文或只写英文 | `.md` 与 `.en.md` 同步 |
| 只跑自己的单测 | Rust 侧的一致性测试才是契约 |
