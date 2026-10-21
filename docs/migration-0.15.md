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
