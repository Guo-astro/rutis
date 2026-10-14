---
name: rutis-new-runtime
description: Use when adding support for a new plugin language or platform to rutis — "add a runtime", "新增运行时", "support Kotlin/Swift/Ruby/… plugins", a new leaf SDK, or making an existing runtime work remotely. Covers whether to do it at all, the design doc, the leaf SDK and runtime contract every runtime must implement, the decisions each runtime must state (reentrancy, capabilities, channels, artifact model), and the full checklist of places in the repo a runtime touches, based on how Bun (#200) and Go (#227) were added.
---

# 为 rutis 添加一个语言运行时

运行时是"让某种语言写的插件接进 rutis"的那一层。rutis 的范式（依赖门控、生命周期、配置对账）**只在 Rust 内核里实现一份**；运行时与它的 SDK 只是很薄的叶子层。凡是想在新语言里再实现一遍范式的冲动，都是走错了方向。

## 0. 先判断该不该做

按 `docs/design-philosophy.md` §七：

- 它带来什么生态能力？（接一种运行时是为了生态，不是为了多一种语言。）
- 有具体的使用方吗？没有就先不做（Go、Kotlin 桌面 JVM 都是"有需求时再做"）。
- 能用已有运行时覆盖吗？（TS 插件已经能在 Node 与 Bun 里跑。）

## 1. 先写设计稿，再写代码

- 位置：`docs/design-<语言>-runtime-<日期>.md` 与 `.en.md`，中文为准。对照已有稿的结构：`docs/design-go-runtime-2026-10-10.md`（编译成程序的语言）、`docs/design-bun-runtime-2026-10-09.md`（按名字装载的语言）、`docs/design-swift-sdk-2026-10-12.md`（含应用托管）。
- 决策表写定，不留"待定"；有明显默认答案的直接定下并写理由。
- 写"与总体稿的出入"一节；合并时更新 `docs/design-multilang-runtimes-2026-10-03.md` 的里程碑与 §九。

## 2. 必须遵守的框架约束

| 约束 | 含义 |
| --- | --- |
| 叶子插件 | SDK 只提供 `define`（`inject` / `provides` + 方法形状 / 配置 schema / `apply`）、`ctx.use` / `ctx.provide` / `ctx.effect`、取消、错误。没有子插件、事件树、本地依赖图 |
| rutis 驱动 | 运行时不决定依赖满足、启动顺序、重启策略；全部 `inject` 由 rutis 门控（`leaf` 特性） |
| 同一个插件 API | 与 `docs/guide/plugin-api.md` 的声明、ctx、生命周期、清理顺序、截止时间一致；插件 API 版本 1 |
| 值规则统一 | JSON 兼容数据复制；函数与对象按引用；整数 ±2^53−1；嵌套 ≤ 64 层；超限由发送方拒绝本次调用，会话继续 |
| 平台语言实现 | 运行时与 SDK 全部用该语言实现，**不依赖 Rust**（不用 FFI / 原生扩展桥接 rutis） |
| 两个方向 | 支持本地通道；要远程时 `listen:` 与 `dial:` 两个方向都实现（`docs/design-remote-runtime-dial-2026-10-14.md` R1） |
| 不碰业务层 | 平台前后台、设备、身份、存储都不进运行时 |
| 测试即契约 | 必须通过 Rust 侧的一致性测试，而不是只有自己的单测 |

## 3. 每个运行时都要回答的决策

| 决策 | 选项与参考 |
| --- | --- |
| 产物模型 | **按名字装载**：一个运行时进程装载多个已安装的包（Node / Bun / Python，行名 `<前缀>:<模块>`，`RuntimeResolver::modules`）；**编译成程序**：插件编进可执行文件，`--rutis-manifest` 自描述，按需启动（Go，`GoResolver` / `GoRuntimes`；Swift 复用其共用代码）；**应用托管**：插件编进应用，应用连 rutis（Swift / Kotlin 应用） |
| 行名前缀与运行时名 | 不与已有前缀冲突（`py`、`bun`、`go`、`swift`、`kt`…）；远程 npm 命名已拒绝任何 `<名>:` 前缀 |
| 执行模型与可重入 | 每个进来的调用一个执行单元（线程 / goroutine / Task / 虚拟线程）→ 可重入，声明 `reentrant-sync`；单线程单栈且等待时把进来的调用压在上面 → 另声明 `sync-stack`；单线程不重入（如 Node）→ 都不声明 |
| 能力声明 | 发同步调用时带 `sync: true` → 声明 `sync-wait`（不声明则不在等待环检测范围内）；能接收对象引用 → `objects`；如实声明，不推断（见总体稿 §九、`crates/rutis-bridge/src/session/rpc/waits.rs`） |
| 通道 | 本地：`fd:3`（继承）、socket 路径拨回（`unix:` 或裸路径）、`tcp:127.0.0.1:<port>`（先发 `RUTIS_CHANNEL_TOKEN` + 换行，随即从环境删除；Windows 用这个）；语言拿不到继承 fd 时用拨回。远程：`listen:wss://…`、`dial:wss://…`（endpoint 协议 3，子协议 `rutis.3`） |
| 方法枚举与 schema | 有反射就派生（Go、Python），没有就显式方法表与手写 schema（Swift、Kotlin） |
| 分发 | 该语言的标准包管理（npm、PyPI、Go 模块、SwiftPM、Maven Central）；版本跟发布列车（`scripts/train.mjs`） |

## 4. 运行时契约（运行时进程要实现的）

握手：compat 协议 2（本地）或 endpoint 协议 3（远程），带能力列表。`mount` 回复：`{ services, features: ["rows.v2", "hosts", "leaf", "scopes"], implementation, engine }`。会话内调用号前缀 `node:`。

| 控制操作 | 行为 |
| --- | --- |
| `rows.schema(entry)` | `{ config, inject, provides, version }`；未知 entry 报错并列出可用名字 |
| `rows.load(key, entry, config, isolate, inject, exports)` | 解析 entry → 解码配置 → `apply`；失败先卸载该行再报错；`ctx.provide` 的服务在回复前以 `service(id, handle, version)` 通知 |
| `rows.update(key, config)` | 叶子无 volatile：卸载后重装 |
| `rows.unload(key)` | 先撤 provides → 取消行的任务 → 逆序清理 |
| `hosts.provide(name, methods, label?)` / `hosts.withdraw(id)` | 登记 / 撤销 rutis 服务代理；`scopes`：服务 id = `name` 或 `name` + NUL + `label` |
| `release` / `get` / `dispose` | 归还导出对象 / 读字段 / 卸载全部行 |
| 会话结束 | 卸载全部行，清理受 `RUTIS_SHUTDOWN_TIMEOUT` 截止，然后退出 |

照抄行为最快的办法是读一个现有实现：Python `python/rutis/rutis/{peer,runner,__main__,websocket}.py`；Go `go/rutis/{serve,listen,runtime,plugin}.go`；Bun `bun/rutis-bun/src/`。

## 5. 仓库里要改的地方（Bun #200、Go #227 的实际落点）

**语言侧**
- [ ] `<语言>/rutis…`：会话层、SDK、运行时入口、无宿主测试助手（如 Go `rutistest`、Python `rutis.testing`）
- [ ] 一致性测试入口（供 Rust 测试拉起）

**rutis-bridge**
- [ ] `src/runtime/process.rs`：`Launcher::<语言>`（命令、工作目录、通道交接方式）
- [ ] `src/runtime/local.rs`：`LocalRuntime::<语言>`、运行时名
- [ ] `src/runtime/mod.rs`、`Cargo.toml` feature
- [ ] 测试：`tests/runtime_conformance.rs`（`session::testing::session`）、`tests/session_matrix.rs`（契约 × 通道）、`tests/<语言>_runtime.rs`；远程加 `tests/websocket_conformance.rs`（与将来的 `dial_conformance.rs`）

**rutis-loader**
- [ ] `src/runtime.rs`（按名字装载：`RuntimeResolver`）或 `src/runtime/<语言>.rs`（编译成程序：复用 Go 的清单机器）、`src/lib.rs`、`Cargo.toml` feature
- [ ] 测试：`tests/<语言>_rows.rs`、`tests/multilang_<语言>.rs`（多语言冷启动互调、撤提供者只停使用者、`inject` 门控、交叉同步调用）、`tests/instance_<语言>.rs`、`tests/leases.rs`

**rutis-host**
- [ ] `src/config.rs`：`runtimes.<语言>` 字段
- [ ] `src/host.rs`：挂载分支、预检（解释器 / 工具链版本）、`runtimes.remote[].language` 映射
- [ ] `src/main.rs`、`src/project.rs`、`src/new.rs`：`check`、`dev`（宿主不编译，只调项目自己的构建）、`new --lang <语言>`
- [ ] `templates/<语言>/`：插件、测试、`rutis.dev.json`、发布 workflow

**发布与 CI**
- [ ] `.github/workflows/ci.yml`：`runtimes-<语言>` job；`release.yml`
- [ ] `scripts/train.mjs`：版本文件与包坐标

**文档（中英都要）**
- [ ] `docs/guide/<语言>-plugin.md`、`docs/guide/plugin-api.md` 加一列、`docs/guide/rutis-host.md`、`docs/guide/nodes.md`（远程）
- [ ] `README.md` / `README.zh-CN.md` 包表、`CONTRIBUTING.md` 工具链
- [ ] 总体稿里程碑与 §九

## 6. 分阶段（照 Go / Swift 的节奏）

1. 设计稿合并。
2. 会话层 + 一致性测试入口：过 `runtime_conformance`。
3. 运行时契约 + SDK + Rust 接入：过 `<语言>_runtime`、`<语言>_rows`、`multilang` 列；现有测试全绿。
4. 开发流程：`new`、`dev`、`check`、模板、发布。
5. 远程：`listen:` 与 `dial:`，过 `websocket_conformance` 与 `dial_conformance`。

## 7. 常见错误

| 错误 | 正确做法 |
| --- | --- |
| 在 SDK 里实现依赖等待、重试、启动顺序 | rutis 门控；SDK 只执行 `rows.load` / `unload` |
| 用 FFI 嵌入 Rust 来省掉协议实现 | 平台语言实现；一致性测试保证与其他语言一致 |
| 不声明或乱声明能力 | 按执行模型如实声明 `reentrant-sync` / `sync-stack` / `sync-wait` |
| 读线程上跑插件代码或做同步桥接 | 每个进来的调用一个执行单元；读线程只解析帧 |
| 宿主替插件编译代码 | `dev` 只调用项目自己的构建；`run` 只运行现成产物 |
| 只写中文或只写英文文档 | `.md` 与 `.en.md` 同步 |
| 只跑自己的单测 | Rust 侧一致性测试是契约 |
