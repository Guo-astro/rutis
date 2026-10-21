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

## rutis-host

`runtimes.go` 新增 `restart`（`"no"` 默认、`"on-failure"`、`"exit"`），与 `node`、`py`、`bun` 的相同，见 [rutis-host](guide/rutis-host.md)「运行时结束时」。只是新增字段：已有的 `rutis.json` 照旧读取，不用改。

新增（不影响已有配置）：

- `runtimes.swift`：Swift 插件二进制（macOS），设置与 `runtimes.go` 相同（`start`、`idle`、`restart`），见 [Swift 插件](guide/swift-plugin.md)。
- 远程运行时的 `language` 接受 `"swift"`、`"kotlin"` / `"kt"`：应用托管（[Swift 应用](guide/swift-app.md)、[Kotlin 应用](guide/kotlin-app.md)）。

`rutis-host check` 对 Go / Swift 插件二进制检查它们的插件 API：低于 3（rutis 0.13 之前的 SDK 构建的）报错，退出码非 0。这样的二进制在 0.13 起本来就连不上（统一事件），现在在 `check` 时就指出来，重新用新 SDK 构建即可。

## rutis-bridge

`session::Dispatch` 与 `peer::Handler` 各加了带默认实现的方法（事件测试钩子用）：已有的实现照旧编译，不用改。

## 事件

运行时按帧的顺序接收 emit 的回复，rutis 对 emit 的转发总是回一个普通值（[事件协议](design-events-protocol-2026-10-18.md) §5.5）。插件代码不用改；与 0.13 的宿主、运行时照常对接。

写运行时与测试工具的人需要知道（协议 §5.5，[运行时约定](../.agents/skills/rutis-new-runtime/references/runtime-contract.md) §6，英文）：

- **宿主对 `emit` 总是回普通值**：`emit` 方式的 `events.dispatch` 在各监听器开始之后回 `undefined`，在会话队列里等过（过滤选择器尚未回复）的也一样。0.13 对排队的 emit 回的是 future。
- **运行时按帧的顺序接收这个回复**：读到回复帧时就生效，处理完再读下一帧，而不是放到之后的任务、tick 或线程里。`emit` 方式的 `events.call` 也按帧的顺序开始，自己的同步调用正在等待时也一样。Node 运行时现在就是这样；在此之前它把这类调用扣到等待结束之后，后面的 emit 可能走快速路径抢到前面。
- **`events.expected.json`**：步骤期望里有 `answered`，表示等宿主回复了收到的每个 `events.dispatch`。另外新增 spec 字段 `then`（`{until, args}`）、`unload` 步骤的 `then`，以及同步方法 `events-log.until(entry)`。读这个文件的测试工具要接受这些键。用 `then` 的用例都是 `tool: false`。
- **`Tap::answered`**（`rutis_bridge::events::testing`，feature `testing`）：等这些回复，供你自己的宿主侧测试使用。
- **兼容**：0.13 的运行时仍可接 0.15 的宿主，但保持依赖时序的顺序，因为它在之后的 tick 才记下回复。0.15 的运行时仍会结算 0.13 宿主回的 future，与这种宿主对接时保持 0.13 的顺序。
