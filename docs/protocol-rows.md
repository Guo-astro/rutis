# 行协议规格：运行时行（协议 5）

[English](protocol-rows.en.md)

状态：现行规格。日期：2026-11-16。基准：`main` `b8fb89ba`。
议题：[#635](https://github.com/arcships/rutis/issues/635)（现行规格）、[#613](https://github.com/arcships/rutis/issues/613)（行生命周期模型），属于 [#183](https://github.com/arcships/rutis/issues/183) 第四步；模型见[行生命周期](../specs/row-lifecycle/README.md)，追踪词汇见[会话追踪契约](formal-trace.md)。

本文是宿主与运行时之间的行协议现行规格：一条会话上装载、更新、卸载一行，行提供的服务与宿主服务的租约，行自行结束与失败，以及宿主（loader）一侧的行生命周期。只写现行实现有证据的行为。

不在本文范围：会话帧本身（[会话协议规格](protocol-session.md)）；事件（[事件协议](design-events-protocol-2026-10-18.md)）；远端租约、重连与身份（[远端租约模型](../specs/remote-lease/README.md)）；节点托管行（[peer-rows](../specs/peer-rows/README.md)）；运行时进程的监督、重启与空闲停止（[质量执行现状](quality-status.md) §7.2 的 RS1）。

每条规则一个编号（`ROWS-*`）。状态四种：

| 状态 | 含义 |
| --- | --- |
| **当前** | 本文指定的实现和测试所执行的规则；给出代码位置或测试锚点。 |
| **已取代** | 旧设计、旧 issue 或旧协议版本的规则，不能再作为证据。 |
| **明确不保证** | 实现有意不承诺；验证器不得依赖。 |
| **未确认** | 只有一处实现或只有模型、没有跨实现用例；不能当成七种运行时共同的承诺，证据补齐前不得依赖。 |

错误**文字**只有在本文逐字给出时才是契约；同一类错误在不同实现里的句子可以不同（§10 记下已知的差异）。

## 1. 范围与承载

`ROWS-1` **当前。** 行协议是会话上的控制操作：`invoke` 帧的 `target` 为空，`method` 是被调用的控制操作，`args` 是参数列表。宿主发给运行时的有 `mount`、`rows.load`、`rows.update`、`rows.unload`、`rows.schema`、`hosts.provide`、`hosts.withdraw`、`plugins.read`（`plugin-values`）；运行时发给宿主的有 `rows.ended`、`rows.failed` 和 `service`（服务槽位变化）。`target` 非空的 `invoke` 是应用服务调用，与行协议无关。`crates/rutis-bridge/src/runtime/process.rs:480-608`；`node/rutis-runtime/src/runner.mjs:1008-1041`

`ROWS-2` **当前。** 一个运行时的一条会话服务它的全部行，不为一行另开会话或通道；控制操作按会话的帧序处理，和前后的会话帧没有别的顺序。`rows.ended` 与同一会话的 `events.*` 按帧序处理（会话协议 `SESS-19`）。`crates/rutis-bridge/src/runtime/process.rs:135,589`；`crates/rutis-bridge/src/session/rpc.rs:1433-1444`

`ROWS-3` **当前。** 跑行的运行时必须声明能力 `rows.v2` 与 `hosts`。宿主在让运行时跑行之前检查，缺一项就拒绝，文字逐字为：

```text
the <运行时名> runtime lacks <缺少的能力，用 and 连接>: a runtime speaking the row contract is required (for Node, @arcships/rutis-runtime 0.3.0 or later)
```

静态挂载（`Mount` 不跑行）不检查。`crates/rutis-bridge/src/runtime/process.rs:13,1452-1472`；`crates/rutis-host/src/status.rs:474-476`

`ROWS-4` **当前。** 六种运行时在 `mount` 回复里报出同一组能力：`rows.v2`、`hosts`、`leaf`、`scopes`、`rows.announce`、`plugin-values`；Node 报 `leaf.js` 而不报 `leaf`。宿主只按用途读它需要的能力，不按“能力全部一致”判断。`node/rutis-runtime/src/runner.mjs:89`；`python/rutis/rutis/runner.py:75`；`bun/rutis-bun/src/runner.ts:60`；`go/rutis/internal/runtime/runtime.go:28`；`swift/rutis/Sources/Rutis/API/Runtime.swift:13`；`kotlin/rutis/rutis/src/shared/kotlin/arcships/rutis/internal/Runtime.kt:41`；`crates/rutis-bridge/src/session/plugin/rows.rs:47`

## 2. 键、代次与行 id

`ROWS-5` **当前。** 行在一条会话里由 `key` 标识，同一个 `key` 最多一行。重复装载同一个 `key` 让这次调用失败，文字为 `row <key> is already loaded`。`node/rutis-runtime/src/runner.mjs:647`；`bun/rutis-bun/src/runner.ts:358`；`python/rutis/rutis/runner.py:616`；`go/rutis/internal/runtime/runtime.go:520`；`swift/rutis/Sources/Rutis/API/Runtime.swift:792`；`kotlin/rutis/rutis/src/shared/kotlin/arcships/rutis/internal/Runtime.kt:615`

`ROWS-6` **当前。** 每次装载取一个新 `key`，行的旧代与新代因此不会共用一个键：loader 的行用 `<实例>.<代次>`（`crates/rutis-loader/src/runtime.rs:719-727`），`run` API 的行用 `<插件>#<n>`（`crates/rutis-loader/src/run.rs:168`）。旧代的迟到 `rows.ended` / `rows.failed` 只作用于收到它的那个键，不结束、不重启新代。`crates/rutis-loader/tests/rows/session_rows.rs::a_late_notice_of_an_old_key_reaches_no_row`、`crates/rutis-loader/tests/go/runtime_rows.rs::a_row_restarted_while_loading_is_unloaded_in_the_runtime`；模型 `OwnFramesOnly`

`ROWS-7` **当前。** `rows.load` 的第七个参数是 `{scope, id}`：`scope` 是行的作用域链（`[{id, group}]`，根为 `[]`），`id` 是配置给这一行的 id；不给 `id` 时用 `key`。`id` 不要求唯一（实例里同组的两行可以同名），`key` 必须唯一。`crates/rutis-bridge/src/runtime/process.rs:1094-1111`

## 3. 装载

`ROWS-8` **当前。** `rows.load` 的参数固定七个、顺序固定：

```text
[key, entry, config, isolate, inject, exports, {scope, id}]
```

`entry` 由运行时自己解释（模块名、bundle、二进制里的插件名，或插件定义引用）；`config` 是已补默认值、已校验的配置；`isolate` 是 `[服务名, 标签]` 对；`inject` 是这一行自己等的服务名；`exports` 是 `{服务名: {方法: "sync" | "async"}}`，不导出时为 `{}`。宿主总是发七个参数。`crates/rutis-bridge/src/runtime/process.rs:1068-1079,1284-1294`；`node/rutis-runtime/src/runner.mjs:647`；`python/rutis/rutis/runner.py:1277-1287`。

`ROWS-9` **当前。** `rows.load` 的回复在下面这些之后才发：插件的 `apply` 返回；它声明的服务已投到 rutis；没有声明就提供的服务已公告并被接受；它的监听器已在总线上登记。插件在装载期间抛出的错误（包括 `apply` 返回的拒绝）让这次装载失败，不产生“已装载再失败”。`node/rutis-runtime/src/runner.mjs:673-742`；`python/rutis/rutis/runner.py:671-700`；事件协议 §4.4 第 2 条；`crates/rutis-bridge/src/runtime/process.rs:531-541`

`ROWS-10` **当前。** 装载失败只让这一次调用失败：运行时卸载已经建立的半行（清理、撤服务、撤监听器），宿主忘掉这一行并释放它已租用的宿主服务，这一行不留残余。失败是这次 `rows.load` 的 `throw`，不是单独的帧。`node/rutis-runtime/src/runner.mjs:745-756`；`crates/rutis-loader/src/runtime.rs:613-640`；`crates/rutis-bridge/src/runtime/process.rs:1068-1079,1305-1315`；测试 `crates/rutis-loader/tests/go/runtime_rows.rs::a_row_that_fails_to_load_releases_its_leases`

`ROWS-11` **当前。** `rows.schema` 的参数是 `entry`，回复是这一行的描述：`config`（配置的 JSON Schema，没有时为 `null`）、`inject`、`optional`、`provides`（`{名字: {方法: "sync" | "async"}}`）、`announces`（运行时会公告没有声明的服务）、`version`（包或模块的版本，没有时为 `null`）；Node 另有 `cordis: true`。描述里缺的字段按空处理。`crates/rutis-bridge/src/runtime/process.rs:730-757,1417-1421`；`node/rutis-runtime/src/runner.mjs:785-817`；`python/rutis/rutis/runner.py:970-980`；`kotlin/rutis/rutis/src/shared/kotlin/arcships/rutis/internal/Runtime.kt:978-990`

`ROWS-12` **当前。** `mount` 的参数是 `{plugins, services, provided, announce, scope, runtime}`；回复至少带 `services` 与 `features`，还可以带 `plugins`（`{名字: 描述}`）、`implementation`、`engine`。宿主先读 `features` 并检查 `ROWS-3`，再按 `plugins` 或逐个 `rows.schema` 得知它有哪些插件。Go、Swift、Kotlin 在回复里列出 `plugins`；Node、Bun、Python 不列，宿主逐个问 `rows.schema`。`crates/rutis-bridge/src/runtime/process.rs:1014-1045`；`node/rutis-runtime/src/runner.mjs:938-977`；`go/rutis/internal/runtime/serve.go:136`；`swift/rutis/Sources/Rutis/API/Runtime.swift:964-971`；`kotlin/rutis/rutis/src/shared/kotlin/arcships/rutis/internal/Control.kt:34`

## 4. 更新

`ROWS-13` **当前。** `rows.update` 的参数是 `[key, config]`，键与代次不变。只改了易变字段（schema 里标 `x-volatile` 的字段）时，Node 的 Cordis 行就地提交、不卸载也不换代；其余更新按“卸载旧行、用同一 `key` 再装载”完成。全部叶运行时（Python、Bun、Go、Swift、Kotlin）没有易变字段，每次更新都重启这一行。`crates/rutis-bridge/src/runtime/process.rs:1347-1349`；`node/rutis-runtime/src/runner.mjs:837-886`；`python/rutis/rutis/runner.py:863-875`；`bun/rutis-bun/src/runner.ts:504-511`；`go/rutis/internal/runtime/runtime.go:644-666`；`swift/rutis/Sources/Rutis/API/Runtime.swift:892-905`；`kotlin/rutis/rutis/src/shared/kotlin/arcships/rutis/internal/Runtime.kt:875-887`；测试 `crates/rutis-loader/tests/go/runtime_rows.rs::volatile_changes_reach_cordis_in_place`

`ROWS-14` **当前。** 更新一个不在装载中的行（没装载过，或正在卸载）让这次调用失败，文字为 `row <key> is not loaded`。六种运行时用同一句。`node/rutis-runtime/src/runner.mjs:852`；`bun/rutis-bun/src/runner.ts:506`；`python/rutis/rutis/runner.py:870`；`go/rutis/internal/runtime/runtime.go:646-650`；`swift/rutis/Sources/Rutis/API/Runtime.swift:893-896`；`kotlin/rutis/rutis/src/shared/kotlin/arcships/rutis/internal/Runtime.kt:876-879`

`ROWS-15` **当前。** 重启式的更新按卸载再装载的顺序进行：先撤下这一行的服务与监听器、取消并等正在运行的监听器结束，再运行新的 `apply`；回复在新的监听器登记之后发（`ROWS-9`）。`node/rutis-runtime/src/runner.mjs:860-884`；`bun/rutis-bun/src/runner.ts:504-511`；`python/rutis/rutis/runner.py:863-875`；`crates/rutis-loader/tests/go/runtime_rows.rs::volatile_changes_reach_cordis_in_place`（其余字段的更新走重启）

## 5. 卸载

`ROWS-16` **当前。** `rows.unload` 的参数是 `[key]`，幂等：行不在就只回复，不报错。正在卸载的行再次卸载同样只回复。`node/rutis-runtime/src/runner.mjs:747-750`；`python/rutis/rutis/runner.py:815-819`；`bun/rutis-bun/src/runner.ts:513-517`；`go/rutis/internal/runtime/runtime.go:859-863`；`swift/rutis/Sources/Rutis/API/Runtime.swift:837-841`；`kotlin/rutis/rutis/src/shared/kotlin/arcships/rutis/internal/Runtime.kt:716-720`

`ROWS-17` **当前。** 卸载一开始，这一行的监听器离开总线，别的行不再看到它提供的服务（调用以 `unavailable` 失败，不看撤回排在卸载栈的哪一步）；这一行自己的清理仍能发出，别的行听得到，直到它回复 `rows.unload`。回复之后它的上下文再发不出，错误为 `BindingError`，文字 `row <行> is not loaded in this runtime`。宿主在发出 `rows.unload` 时就撤下这一行的条目。`crates/rutis-bridge/src/runtime/process.rs:1121-1131,1352-1365`；`node/rutis-runtime/src/runner.mjs:747-783`；`docs/guarantees.md` 的 `sdk/unload-order`；事件协议 §7.1

`ROWS-18` **当前。** 运行时在回复 `rows.unload` 之前撤下这一行提供的服务，回复在全部清理跑完之后；宿主收到回复后忘掉这一行的服务与注销。会话中途结束（断线、顶替）时同样先撤服务。`crates/rutis-bridge/src/runtime/process.rs:1298-1315`；`node/rutis-runtime/src/runner.mjs:747-783`；`docs/guarantees.md` 的 `sdk/unload-order`；`crates/rutis-loader/src/runtime.rs:649-665`

`ROWS-19` **当前。** 会话结束面：一条会话结束时（断线或被新连接顶替），运行时清理它的全部行，每行一次；新会话的 `hello` 在旧会话的行清理完之后才回答；旧会话不再读，旧代的迟到帧不作用于新会话。`specs/row-lifecycle/README.md` 的 `NewSessionOnlyAfterSweep`、`NoRowsAfterSweep`、`CleanupOnce`；`node/rutis-runtime/src/serve.mjs:1-18`（顶替顺序）；`python/rutis/rutis/__main__.py:212`（`serve` 同一顺序）；`go/rutis/internal/runtime/listen.go`（同一顺序）

`ROWS-20` **当前。** 承载卸载等待的时限：卸载等正在运行的监听器，最多等到关闭截止（`RUTIS_SHUTDOWN_TIMEOUT`，默认 10 秒），到点继续卸载，清理可能没跑完；宿主的停止时限另见 `docs/guarantees.md` 的 `host/stop-deadline`。`go/rutis/internal/runtime/runtime.go:669-686`；`node/rutis-runtime/src/runner.mjs:149,762`；`docs/guarantees.md` 的 `sdk/unload-order`、`host/stop-deadline`

## 6. 服务与宿主服务租约

`ROWS-21` **当前。** 一行提供的服务按“名字 + `isolate` 标签”组成一个 id：没有标签时就是名字，有标签时是名字与标签（`name\0label`）。一条会话里同一个 id 最多一个提供者；冲突让后装载的那一行失败，文字形如 `service <id> is already exported by row <key>`（Go、Swift、Kotlin 是 `<名字> is already provided by <行>: rename one of them`）。`crates/rutis-bridge/src/runtime/process.rs:43-54`；`node/rutis-runtime/src/runner.mjs:650-657`；`go/rutis/internal/runtime/runtime.go:530-538`；`swift/rutis/Sources/Rutis/API/Runtime.swift:53-59`；`kotlin/rutis/rutis/src/shared/kotlin/arcships/rutis/Refusals.kt:81-87`

`ROWS-22` **当前。** 运行时用 `service` 控制操作报告服务槽位的变化：参数是 `[id, handle, version]`；没有声明就提供的服务在第四位多一个 `{row, methods}`，`row` 是它的行键，`methods` 是 `{方法: "sync" | "async"}`。`handle` 为 `null` 是撤下。声明的服务在 `rows.load` 回复之前发出（`ROWS-9`），没有声明的也在回复之前公告并被接受。`crates/rutis-bridge/src/runtime/process.rs:570-608`；`node/rutis-runtime/src/runner.mjs:266-274,706-714`；`crates/rutis-bridge/tests/session/row_services.rs::services_a_row_does_not_declare_are_announced_with_their_methods`

`ROWS-23` **当前。** 服务槽位带版本，同一个 id 每次变化取一个递增的版本；宿主只应用比已知版本新的公告，迟到的旧公告被丢弃。`crates/rutis-bridge/src/runtime/process.rs:453-472`；`node/rutis-runtime/src/runner.mjs:266`；模型 `NewerWins`

`ROWS-24` **当前。** 宿主为行租用的服务走 `hosts.provide`（参数 `[名字, 方法, 标签?]`）与 `hosts.withdraw`（参数 `[id]`）：同一个 id 的第一个租约 `provide`，最后一个释放时 `withdraw`；id 的组成同 `ROWS-21`。重复 `provide` 让这次调用失败，文字为 `host service <id> is already provided`。`crates/rutis-bridge/src/runtime/process.rs:1546-1625`；`node/rutis-runtime/src/runner.mjs:1021-1039`；`python/rutis/rutis/runner.py:1294-1306`；`go/rutis/internal/runtime/runtime.go:890-920`

`ROWS-25` **当前。** 租约不长于行：行装载时租用它注入的宿主服务，行清理时在 `rows.unload` 之后释放；装载失败也释放（`ROWS-10`）。`crates/rutis-loader/src/runtime.rs:623-665,1027-1031`；`crates/rutis-bridge/src/runtime/process.rs:1546-1625`；模型 `LeaseWithinRow`、`HostLeases`

## 7. 行自行结束与失败

`ROWS-26` **当前。** 插件自行结束（Cordis 插件 dispose 自己的 fiber）时，运行时把这一行剩下的部分卸掉，然后发 `rows.ended`（参数 `[key]`）；宿主结束这一行并卸掉它，不再装载。`rows.ended` 是会话帧序里的表变更，和 `events.*` 同序（`SESS-19`）。`crates/rutis-bridge/src/runtime/process.rs:531-541`；`node/rutis-runtime/src/runner.mjs:372-382`；`crates/rutis-loader/tests/go/runtime_rows.rs::a_plugin_that_disposes_itself_ends_its_row`

`ROWS-27` **未确认。** 装载期间插件自行结束：按 Node 的规则，装载照样成功，`rows.ended` 在装载回复之后才发（否则装载失败只报装载的错误）。这是模型 `EndingAfterLoad` 建的规则，只有 Node 的实现和用例；其余运行时的同一时序未确认。`node/rutis-runtime/src/runner.mjs:737,372-377`；`specs/row-lifecycle/README.md` 的 `EndingAfterLoad`

`ROWS-28` **当前。**（帧与宿主处理；发送方见下）行的代码在运行中留下没处理的错误时，运行时发 `rows.failed`：参数 `[key, 原因, 错误?]`，`原因` 写明是什么（如 `unhandled rejection: Error: boom`），`错误` 是运行时编码的抛出值。宿主把它作为 `UnhandledError` 让这一行失败，运行时继续运行、这一行仍保持装载（由 loader 决定重启，重启以这个错误失败）。每代最多一次（模型 `RFail`；Node 的 `failing` 保证一次）。`crates/rutis-bridge/src/runtime/process.rs:543-568`；`node/rutis-runtime/src/runner.mjs:1130-1132`；`python/rutis/rutis/runner.py:785-796`；`docs/guarantees.md` 的 `loader/unhandled-errors`

`ROWS-29` **未确认。**（跨实现）`rows.failed` 只有 Node（`dev` 下追踪到行）与 Python 发出；Bun 追踪不到行，只写 `runtime <名字>: an unhandled error from an unknown row: …`，不让任何行失败；Go、Swift、Kotlin 没有这条路径。因此“行代码未处理的错误让这一行失败”现在只是 Node（`dev`）与 Python 的行为，不能写成七种运行时的承诺。`bun/rutis-bun/src/main.ts:50-65`；`docs/guarantees.md` 的 `loader/unhandled-errors`

`ROWS-30` **当前。** 宿主收到一个没有登记处理函数的行键的 `rows.failed` 时，回错误 `rows.failed: nothing fails row <key> here`；收到没有登记的 `rows.ended` 时忽略。`crates/rutis-bridge/src/runtime/process.rs:531-568`

## 8. 宿主一侧的行生命周期

`ROWS-31` **当前。** 两阶段放行：运行时进程起来、握手完成后先提供 `<语言>Runtime`；宿主用它刷新绑在这个运行时上的行的解析结果，完成后再提供 `<语言>RuntimeRows`；行等后者齐备才装载。刷新是独立任务，运行时重启打断它，重启之后重做。`crates/rutis-loader/src/runtime/sessions.rs:450-520`；模型 `NoProvisional`（修复前配置 `before-stale`，6f8fb5c7）；`docs/guarantees.md` §14 的 `loader/rows-gate-on-services`

`ROWS-32` **当前。** 宿主一行是一个 fiber，一行最多一个代次处于运行；迟到的 `rows.ended` / `rows.failed` 只结束自己的代次；服务在提供方卸载之前撤下；租约不长于行。`specs/row-lifecycle/README.md` 的 `OneRowPerName`、`OwnFramesOnly`、`WithdrawBeforeUnload`、`LeaseWithinRow`；36 个配置（`specs/row-lifecycle/expected.txt`）

`ROWS-33` **当前。** 撞名拒绝记在引起它的那次装载（fiber 实例）上：只让紧接的那次 `apply` 失败；这次重启在等依赖、没有 `apply` 时拒绝被忘掉，不失败后面的代。`crates/rutis-loader/src/runtime.rs:371-405`；`crates/rutis-loader/tests/rows/fault_containment.rs::node_a_clashing_provide_fails_only_its_row`；模型 `RefusalScope`

`ROWS-34` **当前。** 运行时进程结束时 `Runtime` 与 `RuntimeRows` 都撤销，它的行停下等待，正在进行的装载也随之等待；不自动重启，是否重启由应用决定；其他运行时、不用它的服务的行照常运行。重启之后行重新解析、重新装载。`crates/rutis-loader/src/runtime.rs:1013-1035` 的 `wait_for_the_runtime`；`docs/guarantees.md` 的 `loader/runtime-down-stops-its-rows`、`loader/runtime-start-failure-isolated`

## 9. 未确认与缺口

以下事项**不**是本规格的承诺，验证器与实现不得依赖；括号里是已知证据：

- **七种运行时的行控制没有共享一致性矩阵**（`SYN-2-13` 的备注、#625）：`rows.load` / `update` / `unload` / `schema` 的用例分散在 loader 的 `*_rows.rs` 与本语言测试里，Swift、Kotlin 没有跨语言的行控制用例。
- **装载中自行结束的时序**只有 Node 与模型（`ROWS-27`）。
- **运行中未处理错误的行失败**只有 Node（`dev`）与 Python（`ROWS-29`）。
- **各语言卸载栈的差异、`rows.schema` 与 `mount` 的形状协商、调用链**不在 `specs/row-lifecycle` 的模型里（`specs/row-lifecycle/README.md` 「未覆盖」）；本文只写实现与用例能核对的部分。
- **Swift、Kotlin 的会话结束顶替顺序**没有像 Node、Python、Go 那样在模型 README 的对应表里逐条核对（`ROWS-19`）。
- **`rows.update` 的易变字段**只有 Node 的 Cordis 行有；叶运行时一律重启（`ROWS-13`），这一半是“不提供”，不是未确认。

## 10. 开发者看到什么

### 插件作者

正常时：`apply(ctx, config)` 返回清理函数；`ctx.provide(name, object)` 之后这个服务按 `ROWS-22` 投到 rutis，别的语言的行按名字调用它；`ctx.inject` 声明的服务由 loader 等齐备才让这一行启动（`ROWS-31`）。不需要写行协议。

出错时：

- 两个行提供同一个名字（同一个 `isolate` 标签下）：后装载的行失败，宿主打印 `service <id> is already exported by row <key>`（Go、Swift、Kotlin：`<名字> is already provided by <行>: rename one of them`），`rutis-host run` 在下面缩进写出这一行（`in <行>:`）与插件语言给出的位置，改名字或加 `isolate`。
- 行代码里 promise 拒绝没人处理：Node（`dev`）与 Python 让这一行失败，`rutis-host` 打印 `failed in apply: <插件自己的错误>`（或从调用处来的 `call to <服务>.<方法> failed: …`），下面缩进写出 `in <行>:`；`run` 下的 Node 与 Bun 只写 `runtime <名字>: an unhandled error from an unknown row: …`，哪一行都不失败（`ROWS-29`）。
- 行卸载之后它的上下文再发事件：`BindingError`，文字 `row <行> is not loaded in this runtime`；清理里发出的仍能到达别的行（`ROWS-17`）。

### 宿主作者与部署者

正常时：用 loader 的行名写一行（`py:weather.plugin`、`go:tools/ping`、`<连接行>:<插件>`），loader 按 `ROWS-6` 的键装载、更新、卸载；运行时进程意外结束时它的行停下等待，不自动重启（`ROWS-34`）。

出错时：

- 运行时不声明 `rows.v2` 或 `hosts`：`the <运行时名> runtime lacks rows.v2 and hosts: a runtime speaking the row contract is required (for Node, @arcships/rutis-runtime 0.3.0 or later)`，运行时状态显示为 `down (<原因>)`，升级运行时或换一个。
- 服务名在同一个 `isolate` 标签下撞车：后装载的行失败，打印见上；先提供者的行继续运行。
- 一个运行时的行不占用别的运行时：另一运行时崩溃只让它的行等待（`ROWS-34`）。

### 运行时作者与贡献者

实现一行运行时，要提供 `ROWS-1` 的控制操作、在 `mount` 里报 `features`（至少 `rows.v2` 与 `hosts`），并对同一个 `key` 提供 `ROWS-5`、`ROWS-14`、`ROWS-16` 的错误与幂等行为。逐条核对 [`specs/row-lifecycle`](../specs/row-lifecycle/README.md) 的 36 个配置与本文的 `未确认` 清单；新增一条跨实现规则时，先补共享用例再改本文。

## 11. 兼容与迁移

- 本文描述协议 5 的行控制操作；不为此升协议版本。新增控制操作、或改变现有参数的含意，按会话协议 `SESS-80` 处理：升协议版本，并同步本文、`Frame`、畸形矩阵与七种实现。
- 新增能力按 `SESS-80`：发送方按能力决定是否使用，不需要版本协商。
- 旧版 SDK 的运行时缺少 `rows.v2` / `hosts` 时被 `ROWS-3` 拒绝，消息说明升级运行时；0.18 及更早的对端在会话握手时已被拒绝（`SESS-79`）。

## 相关文件

- [会话协议规格](protocol-session.md)
- [事件协议](design-events-protocol-2026-10-18.md)
- [行生命周期模型](../specs/row-lifecycle/README.md)
- [节点托管行](../specs/peer-rows/README.md)
- [保证登记表](guarantees.md) §14「加载器」
