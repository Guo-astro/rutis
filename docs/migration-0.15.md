# 迁移到 0.15

[English](migration-0.15.en.md)

## rutis-loader：Go 运行时的类型成了别名

编译型语言（Go，之后的 Swift）共用的部分抽到了 `rutis_loader::BinaryResolver<L>`、`BinaryRuntimes<L>` 等，`L` 是实现 `BinaryLanguage` 的语言（`rutis_loader::Go`）。Go 的名字都还在，成了类型别名：

| 名字 | 现在是 |
| --- | --- |
| `GoResolver` | `BinaryResolver<Go>` |
| `GoRuntimes` | `BinaryRuntimes<Go>` |
| `GoRuntimesHandle` | `BinaryRuntimesHandle<Go>` |
| `GoBinaries` | `Binaries` |
| `GoManifest` | `BinaryManifest` |
| `GoBinary` | `BinaryFile` |
| `GoRuntimeState` | `BinaryRuntimeState` |
| `GoRuntimeInfo` | `BinaryRuntimeInfo` |

**不用改代码**：`GoResolver::new(..)`、`GoRuntimes::new(..)`、`GoRuntimeState::Stopped(_)` 这样的写法照旧编译，行为不变。`cargo semver-checks` 会把 `GoResolver`、`GoRuntimes`、`GoRuntimesHandle` 报成"删除了的结构体"（它不跟进泛型的别名），这是误报。

新增（不影响已有代码）：

- `GoRuntimesHandle::supervise(RestartPolicy, 回调)` / `unsupervise()`：自己结束的 Go 运行时按 `no` / `on-failure` / `exit` 处理，默认仍不处理。
- `rutis_loader::backoff`：重启的退避（rutis-host 也用它）。
- feature `binary`：编译型语言共用的部分，`go` 会开启它。

行为变化（不用改代码）：

- **启动失败不再每 100 毫秒重试**：二进制不见了或清单读不出来时，运行时显示为停止并带原因（`states()`、`runtimes()` 里的 `Stopped(why)`），只在那个文件变化或调用 `restart()` 时再试。
- **重启先读新二进制**：读不出来时旧运行时继续运行、继续服务它的行。
- **停不下来的运行时被结束**：重启或空闲停止时，运行时的清理最多给 `RUTIS_SHUTDOWN_TIMEOUT`（默认 10 秒）再加 2 秒；到时结束它的进程，等进程没了才启动新的。同一个运行时名下不会同时有两个进程。
- **清单输出有上限**：二进制打印清单时 stdout 超过 16 MiB 就拒绝；stderr 只留前 64 KiB，写得再多也不会让它因 SIGPIPE 退出。
- **监督开始前的结束**：第一次 `supervise` 之前结束的运行时，每个运行时只留最近一次；不打算监督的应用调用一次 `unsupervise()`，之后就不再留。

## rutis-host

`runtimes.go` 新增 `restart`（`"no"` 默认、`"on-failure"`、`"exit"`），与 `node`、`py`、`bun` 的相同，见 [rutis-host](guide/rutis-host.md)「运行时结束时」。只是新增字段：已有的 `rutis.json` 照旧读取，不用改。

新增（不影响已有配置）：

- `runtimes.swift`：Swift 插件二进制（macOS），设置与 `runtimes.go` 相同（`start`、`idle`、`restart`），见 [Swift 插件](guide/swift-plugin.md)。
- 远程运行时的 `language` 接受 `"swift"`、`"kotlin"` / `"kt"`：应用托管（[Swift 应用](guide/swift-app.md)、[Kotlin 应用](guide/kotlin-app.md)）。

`rutis-host check` 对 Go / Swift 插件二进制检查它们的插件 API：低于 3（rutis 0.13 之前的 SDK 构建的）报错，退出码非 0。这样的二进制在 0.13 起本来就连不上（统一事件），现在在 `check` 时就指出来，重新用新 SDK 构建即可。

## rutis-bridge

`session::Dispatch` 与 `peer::Handler` 各加了带默认实现的方法（事件测试钩子用）：已有的实现照旧编译，不用改。

行为变化（不用改代码）：

- **本地运行时的问候时限 10 秒 → 30 秒**（`LinkConfig::local_runtime()`）：从启动进程算起，包括解释器、JVM 的启动，慢但健康的机器上不再误判。进程退出时照旧立即失败；网络连接（远程运行时、节点）仍是 10 秒。
- **`build.rs` 导出的错误码**：只有一个字段的结构体错误，`code` 现在是它的类型名（透明的 newtype 是字符串），不再是字段名；枚举变体照旧是变体名。按旧的 `code` 判断的插件要改。
- **等待环**：同步等待期间被压在等待之上执行的监听器（Python、Bun 这类可重入运行时）再同步调用回等待链时，以前会卡住，现在以 `SyncWaitCycle` 失败，其余照常完成。
- **macOS 上关闭会话**：宿主只关会话（不退出）时，本地运行时总能读到结束，按截止时间清理后退出；以前偶尔读不到、一直不退出。

## 事件

运行时按帧的顺序接收 emit 的回复，rutis 对 emit 的转发总是回一个普通值（[事件协议](design-events-protocol-2026-10-18.md) §5.5）。插件代码不用改；与 0.13 的宿主、运行时照常对接。

写运行时与测试工具的人需要知道（协议 §5.5，[运行时约定](../.agents/skills/rutis-new-runtime/references/runtime-contract.md) §6，英文）：

- **宿主对 `emit` 总是回普通值**：`emit` 方式的 `events.dispatch` 在各监听器开始之后回 `undefined`，在会话队列里等过（过滤选择器尚未回复）的也一样。0.13 对排队的 emit 回的是 future。
- **运行时按帧的顺序接收这个回复**：读到回复帧时就生效，处理完再读下一帧，而不是放到之后的任务、tick 或线程里。`emit` 方式的 `events.call` 也按帧的顺序开始。不可重入的运行时在自己的同步调用等待期间仍扣住这类调用，这时本进程在快速路径上发出的同名 emit 必须排在被扣住的调用之后，不能抢先。Node 运行时现在就是这样；在此之前后面的 emit 可能走快速路径抢到前面。
- **`events.expected.json`**：步骤期望里有 `answered`，表示等宿主回复了收到的每个 `events.dispatch`。另外新增 spec 字段 `then`（`{until, args}`）、`unload` 步骤的 `then`，以及同步方法 `events-log.until(entry)`。读这个文件的测试工具要接受这些键。用 `then` 的用例都是 `tool: false`。
- **`Tap::answered`**（`rutis_bridge::events::testing`，feature `testing`）：等这些回复，供你自己的宿主侧测试使用。
- **Node 保持不可重入**：同步等待期间，Node 运行时仍扣住 emit 方式的监听器调用，等待结束后按帧的顺序执行，插件不会在同步调用中途看到改了一半的状态。
- **行结束时丢掉它排队的 emit**：一个过滤器一直不回答、行又卸载了时，这一行排队的 emit 被丢掉、不报告，会话里之后的 emit 照常继续。
- **兼容**：0.13 的运行时仍可接 0.15 的宿主，但保持依赖时序的顺序，因为它在之后的 tick 才记下回复。0.15 的运行时仍会结算 0.13 宿主回的 future，与这种宿主对接时保持 0.13 的顺序。

## Go SDK

在已经卸载的行上调用 `ctx.Provide` 或 `ctx.On`（例如从一个普通 goroutine、`time.AfterFunc` 里）不再 panic：写一行 `rutis: row <key> is not active: …`，返回什么都不做的函数。保留名（`rutis/*`）仍然 panic。

## Bun 运行时

拨出在 Bun 1.4.4 上恢复正常：接入点拒绝升级时立即失败（以前等到 10 秒的握手时限），证书被拒归为 `auth-rejected`（以前当作可重试）。旧版 Bun 照旧。

## Rust 服务：在类型上标注 `#[rutis::service]`

Rust 服务在 `impl` 块上加 `#[rutis::service]`，照常 `provide`，各语言的行就能按名字调用（[#510](https://github.com/arcships/rutis/issues/510)，[Rust 宿主指南](guide/rust-host.md#在类型上标注-rutisservice)）。不用 `build.rs` 清单、`export_all` 或手写 `HostDispatch`；内置插件和 dylib 插件一样。

**不用改代码**：`build.rs` 的 `Exports::service(..)` / `service_in(..)` 和手写的 `HostDispatch` 照旧可用。想换成标注的：在类型的 `impl` 上加标注，删掉 `build.rs` 里那一行（两者都有时构建报错）；在实例里提供的服务不用再写实例组。`build.rs` 只剩声明时，写 `Exports::new().typescript("…").generate()`，或者改用 `rutis-host types <crate 目录> -o services.d.ts`。

新增（不影响已有代码）：

- `rutis::service`（宏）与 `rutis::CallError`：标注的方法收到的函数参数调用失败时的错误；函数参数的错误类型要能从它转换（`CordisError`、`anyhow::Error`、装箱的错误都可以）。
- `rutis::TypeKey::type_name()` 公开。
- `rutis_loader::Loader::waiting_for(&entry)`：一行在等哪些服务；等的是一个没标注的 Rust 服务时说明怎么改。
- `rutis-host types`。
- `rutis-host` 的 `rutis-bridge` 依赖开了 `exports` feature。
- `rutis_loader::Provider::Marked(类型)`：`Loader::check` 用它报告标注类型的名字（提供后才对外）。`Provider` 是 `#[non_exhaustive]`，已有的 match 本来就有兜底分支。
- dylib 插件里标注的类型按提供它的那次构建对外：同一版本重新构建、重新加载不用改版本号。
- 按名字调用标注服务时，读取拦截器（`intercept_require_as`）按**调用方**的行选择，与原生的 `require` 一致。
- 新 crate `rutis-macros` 放这个宏；`rutis` 依赖并重新导出它，`Cargo.toml` 不用加。dylib 插件经 `rutis_sdk::rutis::service` 用它。
