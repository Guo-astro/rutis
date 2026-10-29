# 0.19 平台清理：按职责砍掉不属于平台的东西

[English](design-platform-cleanup-2026-10-28.en.md)

状态：定稿，未实现。日期：2026-10-28。基准：`main` `8071a794`。排期：0.19。
Issue：不另开，Refs [#694](https://github.com/arcships/rutis/issues/694)（版本规划）、[#771](https://github.com/arcships/rutis/issues/771)（0.19 实现跟踪）。依据：用户 2026-10-28 批准的五层盘点清单。
与 [远程连接：只留一个会话插件](https://github.com/arcships/rutis/pull/811)（PR #811，文件 `docs/design-remote-session-2026-10-28.md`）的关系：远程连接的全部内容——link、接入点、准入、撤销、登记、`Transport`、`PeerPlugin`、`RuntimeSession`、`RuntimeInstances`、`runtimes.remote`、compat 会话格式、各语言本机模式改 endpoint、`platform()` 的 link 系实现——都在 #811 定，本稿不重复，只引用它的节号与 PR 编号（S1–S6、R1–R6、A1、A2）。两份稿子碰到同一处代码的地方，本稿 §十三 写明放进哪个闭环、谁先谁后。

范围外：会话协议的帧；Cordis 生命周期语义；loader 的行模型、层叠与 `instanced` 分组；dylib 的扩展。

本稿每一条都在 `8071a794` 上核实过定义位置与调用方。核实结果与批准清单不一致的，在对应一节的"核实后与清单不同的地方"里写明事实与处理，不静默改。

## 一、职责与边界

平台的职责只有五项：

1. **生命周期内核**：插件、依赖、服务、事件、清理，包括实例与作用域。
2. **跨边界的统一**：会话协议、值、错误、对象引用、一个交接点（#811 的 session）。
3. **各语言的叶子 SDK**：很小，只让插件作者写本语言的代码。
4. **期望状态**：loader 对账。
5. **看得见**：错误结构、状态快照。平台只负责让人看见，不替人做决定。

平台不做：业务的隔离与分配；连接方式（#811 交给连接插件）；业务契约；agent 功能；长期跨版本兼容。

边界：

| 层 | 是什么 | 规则 |
| --- | --- | --- |
| 平台 | `rutis` 内核（含 `rutis-macros`）、`rutis-bridge`、`rutis-loader`、各语言 SDK 与运行时 | 只放五项职责要的东西 |
| `rutis-host` 与工具 crate `rutis-types` | 参考宿主与工具 | 只能用平台的公开 API，不能把功能推进平台 |
| 其余 | 业务 | 平台不为它兜底 |

原则：

- 一项职责成立的理由，是**我们自己的设计要它**。例：实例插件成立，是因为我们希望复用的实例也有生命周期。
- 业务能解决的，平台不做兜底。
- 不做兼容。旧写法直接删，不加转发、旧名、兜底分支、开关。
- Cordis 兼容与 dsh 兼容是我们要的：dsh 要能装 Cordis 的 dsh 插件。为它留的东西写明理由。

## 二、判断方法

清单里每一项归入下面一类：

| 类 | 判断 | 处理 |
| --- | --- | --- |
| A 职责 | 属于 §一 的五项之一，并且是我们自己的设计要它 | 留在平台；有重复的合成一份 |
| B 工具 | rutis-host 或开发者需要，但不是平台的职责（语言启动方式、终端输出排版、重启策略、开发工具） | 搬到 rutis-host |
| C 砍 | 没有生产调用方；或为旧版本、旧宿主保留的兼容；或与另一项重复；或业务自己能做 | 删，连同测试、文档、示例 |
| 用户定 | 用户 2026-10-28 明确留下的 | 留，理由写在对应一节：Cordis/dsh 兼容、dylib、同步 `bail`、`FiberView::set_config`、`greeting` |

"没有生产调用方"指在 `crates/`、`examples/`、`tests/e2e/`、各语言源码里除定义文件和测试外没有调用；只被测试用的接口算 C。

## 三、内核（`crates/rutis`、`crates/rutis-macros`）

### 3.1 删

| 名字 | 位置 | 调用方（核实） | 类 | 一起改的 |
| --- | --- | --- | --- | --- |
| 服务拦截器：`intercept_require_as`、`intercept_set_as`、`ServiceIntercept`、钩子表 `ServiceInterceptors`、`ServiceReadFailure::Intercept*`、`ServiceWriteFailure::Intercept*` | `intercept.rs:21`、`:48–326`；`internal.rs:179,187`；`ctx.rs:1602–1616`；`error.rs:14–16,52–54` | 生产无。测试：`tests/service_intercepts.rs`（拦截部分）、bridge `marked.rs:1055`、`tests/marked_services.rs:739`、`tests/projection_lifecycle.rs:132,336` | C | `require_provided_as` 的 `actor` 参数只给拦截器用（`ctx.rs:465–468`），一起删；宏生成的 `__rutis_resolve`（`rutis-macros/src/lib.rs:460–464`）、`service::Resolve` 与 `Entry::resolve`（`service.rs:112–120,190–196`）、bridge `marked.rs:72–78` 的签名跟着改。`ServiceWriter::set` 里的写拦截（`intercept.rs:387–402`）删掉，其余留 |
| `check()` 谓词门控：`provide_checked`、`refresh`、`CheckFn`、`service::Check`、`Exposure::check`、`DependencyStatus::CheckPending`、`CheckRejected`、`CheckPanicked` | `registry.rs:10,36–37,355,385–393`；`internal.rs:133,162`；`ctx.rs:1552,1587`；`diagnostics.rs:191–193`；`service.rs:460–462,493–495` | 生产：只有 bridge `marked.rs:219–223` 把 check 转给按名字的入口，它的源头只能是 `provide_checked`，而生产里没有源头。协议与六个 SDK 里没有 check 概念 | C | 清单只写了 `CheckPending`；另两个变体同属这个机制，一起删 |
| 派发观察器：`EventBus::observe_dispatch`、`DispatchAttempt`、`DispatchObserver` | `bus.rs:37,45–50,88,504,531,571–632` | 生产无。测试：`tests/dispatch_observation.rs`（整份）、`typed_dispatch.rs` 5 处、`bus/transient_tests.rs:128`、bridge `tests/event_hub.rs:984`（当探针用） | C | `testing` feature 的 `on_selected_for_testing`（`bus.rs:668`）不是它，留 |
| `take_cleanup_errors`（`CtxHost`、`Ctx`、`FiberView` 三处） | `host.rs:70`、`ctx.rs:1076`、`fiber.rs:1684` | 只有 `tests/cleanup_errors.rs:20,23,66` | C | 底层的 `drained_errors`（`fiber.rs:378`）在卸载时并入结果，留 |
| 清理树内省：`FiberView::effects`、`EffectMeta`、`EffectPhase` | `fiber.rs:1667`、`effect.rs:26,35–48,147` | 只有 `tests/effect_tree.rs` | C | Stuck 提示读的是清理的标签（`fiber.rs:157` → `effect.rs:141–145`，输出在 `fiber.rs:174`）。标签现在放在 `EffectMeta.label`，删 `EffectMeta` 时改为 `EffectRecord` 自己的 `label` 字段；`effect_named` 留 |
| `CtxHost::plugin_from` | `host.rs:239`、`ctx.rs:1421` | 生产无。示例 `examples/development_workflow.rs:134`；测试 `instance_subtrees.rs:411`、`lifecycle_order.rs:276`、`typed_plugin.rs:375`；指南 `rust-host.md:39,89`、`rust-plugin.md:615`（doctest） | C | 见 3.4 第 1 条 |
| `EventPattern` | `key.rs:318–353`；`internal.rs:21` | 全仓无构造与使用（`on_pattern` 0.13 已删） | C | — |
| `waterfall_sync` 与协议方式 `waterfall-sync` | `host.rs:248`、`ctx.rs:1430`、`events/api.rs:400,494–507,691–700`、`events.rs:123–146`（`Mode::WaterfallSync`）、`bus.rs:933,1024,1038–1046,1122,1168`、`bus/run.rs:477–610`、`events/remote.rs:113,151–200` | Rust 生产无发出方。示例 `examples/sync_decision.rs`（整份只演示它）、`benches/events.rs:79–83`、loader 测试 `rust_participant.rs:407–409` | C | 接收分支：bridge `events/hub.rs:1160,1254`（清单漏列）、`events/link.rs:495,578`（#811 改写的文件）、测试工具 `runtime/testing/events.rs` 与 `events.expected.json` 的 `waterfall-sync/*` 两个用例；六个 SDK 的接收分支见 §七；`lib.rs:38` 的"six dispatch modes"改成五种。内核现有的 3 处 `unsafe` 全部只为同步 next 存在（`bus/run.rs:589`、`events/remote.rs:163,197`），随之消失 |
| `internal` 里的 `get_key`、`require_key`、`provide_key` | `internal.rs:99,104,122`；`ctx.rs:1488,1521,1543` | 生产：bridge 留下的 `marked.rs:172,223`、`runtime/rows.rs:317`、`services.rs` 余下部分；loader `catalog.rs:120,125`、`runtime.rs:878,1008`、`runtime/rows.rs:122`；内核 `deps.rs:63,97`；其余在 #811 删除的文件里 | C（与 `*_at` 重复） | 改用 `Key::of/named/dynamic(..).instance(..)` 与 `get_at`/`require_at`/`provide_at`。调用方要改为持有 `Key<T>`：`RuntimeRows::key() -> TypeKey`（`rows.rs:51`，公开）与 `catalog.rs` 的闭包签名跟着改。内核 `deps.rs` 改用 crate 内部方法 |
| 线上错误名 `"CordisError"` | `service.rs:1024`（`FailCordis`） | 生成器 `build/exports.rs:479–491,1856–1858,1876`；生成文件 `examples/rust-services/rows/services.d.ts`、`examples/marked-services/rows/services.{d.ts,py}`；测试 `marked_services.rs:274`；指南 `rust-host.md:145,718` | C | 改名为 `"Error"`，`code` 仍是 `ErrorKind` 的变体名；生成的类型 `CordisErrorThrown` 改为 `ErrorThrown`。理由：0.18 [诊断设计](design-diagnostics-2026-10-26.md) §4.5 规定面向开发者的文字只用开发者写过的东西，开发者写的是 `rutis::Error`。生成器对旧路径 `rutis::CordisError` 的识别一起删。六个 SDK 与 specs 里没有这个名字 |
| `anyhow` feature、`from_anyhow`、`From<Error> for anyhow::Error` | `Cargo.toml:22–23,26`；`error.rs:374–379,439–444` | 只有 `tests/author_api.rs:227–245`；CI `ci.yml:493–495` 专门跑它；没有 Cargo.toml 打开它 | C | 指南给替代写法 `.map_err(\|e\| Error::msg(format!("{e:#}")))`。生成器里识别 `anyhow::Result` 的代码与这个 feature 无关，留（随 §八 搬到 `rutis-types`） |
| `try_filter`、`try_filter_named` | `events/api.rs` | 只有 loader 测试 `rust_participant.rs:354` | C | `Ctx::filter` 留，见 3.3 |

### 3.2 搬

| 名字 | 清单 | 核实后的处理 |
| --- | --- | --- |
| loader 的 Schema 校验 `rutis_loader::config::check`（594 行） | 未列（用户 2026-10-28 确认） | **搬进内核** `rutis::config`，内核是唯一一份配置校验，见 3.4 第 2 条 |
| `internal::config` 的提示工具 `closest`、`distance`、`value_text`、`path_text`、`number_text`、`float_text` | 移到 loader | **留在内核**，从 `internal` 移出，公开在 `rutis::config`。见 3.4 第 2 条 |
| 过滤与作用域描述 `row`、`group`、`mark_row`、`mark_scope`、`CtxHost::scope`/`row`、`events::Scope`、`ListenerInfo.row/scope` | 移到 loader | **不搬**，见 3.4 第 3 条 |

### 3.3 留

| 名字 | 位置 | 理由 |
| --- | --- | --- |
| 同步 `bail`（`Ctx::bail`、`Filtered::bail`） | `events/api.rs:487,668` | 用户定：Cordis 的 `bail`，是范式的一部分；六个 SDK 都提供 |
| `FiberView::set_config` | `fiber.rs:1964` | 用户定：Cordis 的 volatile 字段原地改配置。使用链路（核实）：loader `loader/reconcile.rs:968` 与 `volatile.rs` 调它并发 `loader/volatile-update` 事件，dsh 的插件 `crates/rutis-dsh/dsh/aimux/src/index.ts:19,53` 用 `.volatile()` 配置并监听这个事件 |
| dylib 的库代次：`Generation`、`loading`、`current_loading`、`set_generation`，`Ctx::set_code_generation`/`code_generation`，`code_generation` 字段 | `service.rs:211–285,304,510,523`；`ctx.rs:241–259`；`fiber.rs:408–410` | 用户定：dylib 是我们支持 Rust 动态插件的方式（设计哲学"dylib 也都是插件"）。使用方 `rutis-dylib/src/loader.rs:14,127,596` |
| `Ctx::filter` | `events/api.rs:676` | Cordis 的 `Context.filter`，六个 SDK 都提供（Node `index.d.ts:24`、Python `events.py:544`、Swift `Ctx.swift:161`、Kotlin `Ctx.kt:88`，Go、Bun 同） |
| `effect_named` | — | Stuck 提示用它的标签；生产使用方 bridge `marked.rs:233` |
| `provide_mut_at`、`ServiceWriter` | `intercept.rs:331–447` | bridge projection（`runtime/projection.rs:14,219,241`）在用，不依赖拦截器。`intercept.rs` 只剩它后改名 `writer.rs` |
| `CtxInternal::named` | — | 有生产调用方（loader `reconcile.rs:998`、`runtime.rs:1243`、`binary.rs:2302`；bridge `runtime/rows.rs:312`） |

`DynPlugin::platform()`（`plugin.rs:113`，隐藏标志，默认 false）：本稿不定。核实：返回 true 的 23 个实现里，随 #811 删掉的有 14 个；#811 之后仍存在 6 个真实实现（bridge `RuntimePlugin`、loader `LoaderPlugin`、`RuntimeRowsPlugin`、`BinaryRuntimes`、`BinaryRuntime`，rutis-host `Released`），内核在 7 处读它（`fiber.rs:135,1048,1492`、`fiber/waiting.rs:91`、`diagnostics.rs:68,107,167`）。处理时点：#811 的 S5 合并后，与 §十三 CL6 同一个 PR 复查剩余实现，按那时的实现决定留或删。

`docs/design-rust-port.md`（v5，285 行，头部仍写"草案"）：全文 29 处 `CordisError`，写着"bail 删除、四分发"，与现状不符；但 `guarantees.md:31,117` 把它的 §3 登记为 `core/D1–D31` 的出处，README 与设计哲学也链接它。处理：不删，在头部加一段"本稿是 2026-08 的历史设计，现行说明以 `crates/rutis/src/lib.rs` 的模块文档与 `docs/guarantees.md` 为准"。

### 3.4 核实后与清单不同的地方

1. **`plugin_from` 与 `plugin_with` + `PluginFactory`、`Definition::new` 并不完全重复**。`Definition::new` 加 `ctx.plugin(def.at(..))` 得到的 `FiberView` 不能 `update(config)`（`fiber.rs:500`，没有 factory）；`plugin_with` 要手写 `PluginFactory`，把闭包变成 factory 的 `build_fn`（`plugin.rs:241`）是隐藏的。处理：照清单删 `plugin_from`；需要 `update` 的宿主实现公开的 `host::PluginFactory`（`host.rs:26`），不公开 `build_fn`，不另开入口。指南两处与 doctest 改为 `PluginFactory` 的写法。
2. **提示工具不能整体搬到 loader**：内核的 `decode` 自己就用 `closest`、`value_text`（`config/decode.rs:11,105,111,126,214`），`ConfigIssue::at` 用 `path_text`（`config/mod.rs:108`），而 loader 依赖内核，内核不能反过来调 loader。另一个事实：§七 要把 Schema 校验只留 Rust 一份，但插件值（`ctx.plugin(def.at(&ctx, config))`）的配置由 bridge 直接 `rows.load`（`runtime/process.rs:1220–1299`），不经过 loader；校验只放 loader，这条路径就没人校验。内核是 loader 与 bridge 共同的下层，`check` 只依赖 `serde_json` 与内核自己的 `ConfigError`。决定（用户 2026-10-28 确认）：`rutis_loader::config::check` 搬进内核 `rutis::config`，与 `ConfigError`、`decode` 放在一起；内核是唯一一份配置校验，配置的校验、补默认值、解码、错误文字只在这里；loader 的行与 bridge 的插件值都调它。提示工具留在内核，从 `internal` 移出、在 `rutis::config` 公开（`closest`、`distance`、`value_text`、`path_text`、`number_text`、`float_text`；loader `loader/waits.rs:439,441`、rutis-host `call.rs:107` 用 `closest`、`distance` 说"是不是指"）。这样 rutis-host 与 loader 都不再用 `rutis::internal::config`。
3. **`row`、`group`、`mark_row`、`mark_scope` 不是只服务过滤描述**：内核的监听器描述（`bus.rs:333,336`）、错误文字（`bus.rs:1195` 的 `FilterFailed`、`fiber.rs:604`）、Stuck 与诊断的显示名（`fiber.rs:476`、`diagnostics.rs:60,164,262,284`）、`ServiceChanged` 的 `row`（`ctx.rs:815–816`）、`Definition` 的挂载范围检查（`definition.rs:117,151`）都用它们；它们也是事件协议的一部分（[事件协议](design-events-protocol-2026-10-18.md) §3 的 `rows.load` 第七个参数 `{scope, id}`、§4.2 描述里的 `row`/`scope`），各语言 SDK 的过滤函数读的就是这些字段。处理：不搬，内核保留；写入它们的仍是 loader（`loader/plugins.rs:97,132`）。理由：行与作用域是协议里的值、是诊断里的名字，属于职责 1 与 5。`Ctx::filter` 留（3.3），只删 `try_filter*`。
4. **`check()` 被测试当作卡住门控的钩子**：`tests/load_window.rs:383–410` 的 `CheckedProvider` 用阻塞的 check 停住门控，4 个测试（`:456,:491,:631,:837`）是保证 `core/D6`、`core/D7`、`core/D14`、`loader/rows-gate-on-services` 的证据。处理：这 4 个测试改为用一个 `apply` 里等待信号、之后才 `provide` 的提供方插件卡住门控；不加测试钩子。`contract.rs` 的 check 测试与 `lifecycle_diagnostics.rs::cached_check_diagnostics_do_not_call_user_code` 删；`parity.rs:345` 把 `refresh` 当作 Cordis `reflect.notify` 的对应，改写对拍说明。
5. **拦截器的测试文件里约一半测的是 `ServiceWriter`**（`service_intercepts.rs:219–700`）：这部分移到新文件 `service_writer.rs`，用例不变。projection 的两个测试（`a_proxy_held_during_reentrant_replacement_stays_callable`、`a_retried_publication_keeps_its_handle`）用写拦截制造重入与拒绝：前者改为用真实的重新提供（另一个提供方在 Drop 里重入）制造；后者测的拒绝只有拦截器能制造，随拦截器删掉，`projection.rs` 的重试路径只剩 `provide_mut_at` 失败一个入口，由现有的属主检查测试覆盖。
6. **`set_config`**：dsh 的 Rust 代码不直接调用，是 dsh 的插件经 loader 的 volatile 路径用到（3.3 已按事实写）。

## 四、rutis-bridge（非远程部分）

### 4.1 删

| 名字 | 位置 | 调用方（核实） | 类 | 一起改的 |
| --- | --- | --- | --- | --- |
| 反方向的 Cordis 挂载（Cordis 宿主挂 Rust 插件）：`rutis_plugin` | `cordis/build/rust.rs:668`（整文件 855 行）；再导出 `cordis/build.rs:5–6` | 只有 `examples/native-mount/build.rs:9` | C | 与"rutis 是宿主"相反；与 dsh 无关（dsh 只用 `from_manifest`、`include_mounts!`、`npm_root`、`ROOT_VARIABLE`、`session::Error`）；`examples/native-mount/README.md:12` 已写"反方向（已冻结，不再增加能力）" |
| `cordis::server::{serve, Dispatch}` | `cordis/server.rs`（106 行，`#[cfg(unix)]`） | rust.rs 生成的代码；`examples/native-mount/tests/export_identity.rs` | C | — |
| `runtime/unix.rs`（`Connection::connect`、`connect_with`，75 行） | — | `cordis/server.rs:94`；测试 `tests/rpc_callbacks.rs:305`（保证 `boundary/5.6` 的证据） | C | `rpc_callbacks.rs:305` 的测试改为经本机管道起 Node 运行时（测试名不变） |
| Node 一侧的反方向部分：`client.mjs` 的 `Process.launch`（`:53–63`）、`dispose()`（`:87–95`）、构造函数的 `executable` 参数；`io-worker.mjs:55–79`（注释写明 "The frozen reverse direction"） | `node/rutis-runtime/src/` | `test/fixtures/rust-mount.test.mjs`（146 行）、`bench/sync-call.mjs`（26 行） | C | **`client.mjs` 本身不删**，见 4.4 第 1 条。两份测试与基准删；`bench/sync-call.mjs` 由 §七 的事件基准取代 |
| `examples/native-mount` 中只为反方向的部分 | `src/lib.rs`（84）、`src/bin/rutis-counter.rs`（14）、`tests/cordis_mount.rs`（22）、`tests/export_identity.rs`（31）、`build.rs:9–14` | — | C | 留主方向 `src/main.rs`、`tests/edge.rs`、`build.rs` 的 `from_manifest`；README 删反方向一行 |
| 为旧运行时保留的协商：`events-v2`（`EVENTS_CAPABILITY` `rpc.rs:922`、`without_events` `:1294`、拒绝 `:2461,:2500`、`session/testing.rs:201`）；`objects`（`relay.rs:41`、`crossing.rs:171–175` 的 `object_refused`）；`plugins` 自查自（`rpc.rs:927,2421`，`Endpoint::rust` 总是声明它，死分支）；`sync-wait` 的兼容条件（`rpc.rs:1579,1838`）；空声明的 `signals`（`rpc.rs:1245`）与 `forwarding`（`rpc.rs:1247`，无人检查） | `session/` | 声明与检查遍布六个 SDK（§七 7.2） | C | 保证 `events/v2-required` 删；`events/old-peer-refused` 改为"按协议版本拒绝"，依赖 4.4 第 9 条 |
| 运行时 mount 层的旧特性检查："A runner older than 0.3.0 reports no features"（`runtime/process.rs:1087`）、`require_plugin_values`（`:1265`，调用 `:1231,:1251`）、写死的"@arcships/rutis-runtime 0.3.0 or later"（`:1517–1524`）、没有 `scopes` 时的旧路（`:1577`，`require("scopes")` `:1329,:1607`） | `runtime/process.rs` | — | C | `check_rows`（`:1494`）的文字由 #811 改 |
| `Connection::independent_future` | `session/rpc.rs:1650` | 只有测试 `tests/rpc_callbacks.rs:445,499`，均不在 guarantees 里 | C | `background()` 与 `Background` 执行器留（`relay.rs:55` 在用） |
| 旧的起进程入口：`Process::launch`、`launch_observed`、`launch_group`、`launch_mount`、`attach` | `runtime/process.rs:832,842,855,867,932` | 只有测试（cancellation、concurrent_launch、dsh_baseline、group_mount、rpc_callbacks、projection_lifecycle、error_shape、host_services、live_objects、service_projection、process_exit、session_matrix、event_hub） | C | 这些测试改走 `rutis-test-support` 的起进程辅助（§十三 CL12a）；被 guarantees 引用的测试名不变 |
| `runtime/spawn.rs`（767 行） | — | `Process::mount`（`process.rs:912–913`） | C（与 `transport/local/spawn.rs` 重复） | 逐函数重复：`Connect`/`Handover`、`CHANNEL_FD`、`inherit`、`dial_back`、`loopback`、`Child::watch`、`describe`、`peek_exit`、`HANG_GUARD`、`ended_with`/`Exit::wait`、`command`/`Spawn::command`、`traced`/`LocalTransport::trace`。`Process::mount` 改走 #811 的 `pipe::spawn` 后整份删；dsh 起的进程从此也进进程组与 `RUNNING` 登记 |

`Process::mount`（`process.rs:891`）**留**：生产唯一调用方是 dsh 的 Cordis 类型绑定生成的代码（`node/rutis-runtime/src/generate.mjs:996`），用 `Mount { plugins, services, observer, hosts, ctx, ..Default }`，依赖 `launcher: None` 的默认值（Node）。与 #811 S2b 删 compat（`process.rs:919` 的 `Format::Compat`）对齐，见 §十三。

### 4.2 搬到 rutis-host

| 名字 | 位置 | 调用方 | 一起改的 |
| --- | --- | --- | --- |
| 语言知识：`Launcher::node/python/go/swift/bun` | `runtime/process.rs:333,350,369,377,388` | rutis-host `languages/node.rs:168`、`bun.rs:176`；测试 | 与 #811 的 `rutis-host/spawn` 放在一起。dsh 的 Node 启动见 4.4 第 2 条 |
| `spawn::node_inherits`（旧 Node 包没有 `rutisChannels: ["fd"]` 时改用回拨的兼容分支） | `runtime/spawn.rs:371` | `process.rs:339` | 回拨分支是兼容，删；只留 fd 交接 |
| `go_runtime_name`、`swift_runtime_name` | `runtime/mod.rs:70,78` | loader `runtime/go.rs:46`、`swift.rs:45`（行名）；rutis-host `go.rs:234,338`、`swift.rs:177` | 见 4.4 第 3 条：搬到 **loader**（行名是 loader 的概念），不搬 rutis-host |
| Cargo feature `node`、`python`、`go`、`bun`、`swift`、`kotlin`（`default = ["node"]`） | `Cargo.toml` | src 里只门控上面这些语言项；测试 17 个文件、CI 用它们门控；`cordis = ["node", …]` | bridge 不再有语言 feature；`cordis` 不再依赖 `node`；测试门控见 4.4 第 5 条。清单未提 `kotlin`（注释"Tests only"），同类，一起删 |
| 按行合并输出与行前缀：`transport/local/output.rs`（`OutputStream`、`MAX_LINE`、`write_line`、`output_ended`、`prefixed`、`pipe`）、`Stdio::Lines`（`transport/local/spawn.rs:59`）、`Spawn.output_prefix`（`:103`）、`Launcher.output_prefix`（`process.rs:226,323`） | — | rutis-host `outln!`/`errln!`（`lib.rs:19–38`，93 处）、`host.rs:119–121`、`languages/mod.rs:345`、`binary.rs:184`；loader `binary.rs:1554,2306` | bridge 只交出原始管道；rutis-host 自己分行、给 Go/Swift/Kotlin 二进制加运行时名前缀。保证 `host/output-lines-whole`、`host/output-prefix` 的测试路径跟着改。Node/Bun/Python 的按行前缀不搬，见 4.4 第 8 条 |

### 4.3 留

| 名字 | 位置 | 理由 |
| --- | --- | --- |
| dsh 用的 Cordis 类型绑定与静态挂载：`cordis::build::from_manifest`（`cordis/build.rs:207`）、`Bindings`（`:56`）、`include_mounts!`（`cordis/mod.rs:44`）、`npm_root`（`:35`）、`ROOT_VARIABLE`、`generate.mjs`（1042 行，`cordis/build.rs:151` 调用）、`session::{ObjectRef, RemoteFunction, JsError}`（`session/objects.rs:619,662,699`） | `cordis/`、`session/` | Cordis/dsh 兼容：dsh 要装 Cordis 的 dsh 插件。`ObjectRef`、`RemoteFunction`、`JsError` 在 bridge 内只被生成的绑定代码使用。本稿不把 `cordis` 挪成独立 crate：它依赖 `Process::mount` 与 `Projection`，挪出去要把这两样公开给另一个 crate，不减少代码 |
| `sync-stack`、`reentrant-sync`、`leaf`、`check_rows`、`rows.v2`、`hosts`、`rows.announce` | `rpc.rs:1579–1581`；loader `runtime.rs:423,1202`；`plugin.rs:510`；`process.rs:1319,1451,1605` | 在用：同步等待的环检测、纯叶子运行时、行约定（#811 保留 `check_rows`） |
| `events::report`、`headline` | `report.rs:109,161` | loader 生产在用（`loader/status.rs:389–391`），rutis-host `status.rs:38–41`、`call.rs:187` 也用 |
| `events::report_line` | `events/errors.rs:122` | 见 4.4 第 6 条 |
| `channel/trace.rs` 与 `RUTIS_TRACE` | — | 见 4.4 第 7 条 |
| `greeting`（问候时限） | `runtime/local.rs:293` | 用户定（调试器 `--inspect-brk` 时要等）。载体 `LocalRuntime` 由 #811 删，新位置见 4.4 第 10 条 |

### 4.4 核实后与清单不同的地方

1. **`client.mjs` 不能整个删**：Node 主路径在用它的 `Process.connect`（`runner.mjs:6,1073`），测试夹具也用（`conformance-session.mjs:37`、`rpc-client.mjs:9`——保证 `boundary/5.6` 的夹具、`sync-heartbeat.mjs:6`、`handshake.test.mjs:19`、`texts.test.mjs:95`）。处理：只删反方向的 `launch`、`dispose`、`executable` 参数与 `io-worker.mjs:55–79`，文件保留。
2. **dsh 的静态挂载要 Node 启动方式**：生成的代码调 `Process::mount(.., Mount{..Default})`，默认走 `Launcher::node`（`runtime/spawn.rs:304–318`）；dsh 不依赖 rutis-host。处理：Cordis 静态挂载的 Node 命令放进 bridge 的 `cordis` 模块（Cordis 插件只能跑在 Node 上，这是 Cordis 兼容的一部分），通用的 `Launcher::*` 搬 rutis-host。
3. **`go_runtime_name`、`swift_runtime_name` 被 loader 生产代码用来定行名与运行时名**（`runtime/go.rs:46`、`swift.rs:45`）。处理：搬到 loader（行名规则属于期望状态），rutis-host 从 loader 的公开 API 拿。
4. **不依赖 rutis-host 的 Rust 嵌入方会失去语言启动器**：`rust-host.md:426–560` 用 `LocalRuntime::node/python/go`；e2e `fixture-tenants` 注明"按 rust-host 指南组装，不用 rutis-host"，`fixture-toolhost` 同；examples 的测试与约 20 个 loader 测试也用。`LocalRuntime` 由 #811 删。处理：Rust 嵌入方要起本机语言运行时时依赖 rutis-host 的库（#811 的 `rutis-host/spawn` 与本稿搬过去的语言启动），rust-host 指南与两个 e2e fixture 改为这样；loader、bridge、examples 的测试改用 `rutis-test-support`（不发布）里的起进程辅助，不依赖 rutis-host。
5. **删语言 feature 后测试失去工具链门控**（bridge 17 个测试文件、loader 约 20 个、`ci.yml:534–1756` 十几条命令、`stress.yml:214,347,362`）。处理：需要某种语言的测试改由 `rutis-test-support` 按环境变量 `RUTIS_TEST_RUNTIMES`（逗号分隔，例如 `node,python`）决定：列出的语言找不到工具链时测试失败并写明缺什么；没列出的语言的测试打印一行"skipped: <语言> not in RUTIS_TEST_RUNTIMES"后返回。CI 每个任务设置它，代替现在的 `--features`。这是测试约定，不是产品开关。
6. **`report_line` 不搬**：bridge 自己的 `events::testing::Tap`（`events/testing.rs:64`，loader 测试在用）用它；它依赖私有的 `named`；rust-host 指南（`:658`）把它写给嵌入方，嵌入方的事件监听器出错时用它打印一行。它与 `report`、`headline` 是同一组错误文字，属于职责 5。处理：留在 bridge 的 `events`，rutis-host 照旧调用。
7. **`RUTIS_TRACE` 不只服务本机调试**：rust-host 指南（`:496,:574`）把它写给 Rust 嵌入方，dsh 的挂载也经它（`runtime/spawn.rs:289–300`）。清单的条件是"若只服务本机调试"，条件不成立。处理：留在 bridge，随 #811 的 `pipe::spawn` 走（`pipe::spawn` 在 bridge）。
8. **Node/Bun/Python 运行时一侧的行前缀不能删**：这三种运行时一个进程里有多个插件，前缀 `[行名] ` 按"当前在跑的行"加（Node `output.mjs:32–81`、Bun `runner.ts:143–147`、Python `_output.py:37–99`），宿主读管道时分不出是哪一行写的。bridge 的 `output_prefix` 只给一进程一插件的 Go/Swift/Kotlin 用。处理：运行时一侧的前缀保留；rutis-host 只给二进制运行时加运行时名前缀，并设 `RUTIS_PREFIX_OUTPUT` 让脚本运行时自己加。
9. **会话协议版本升到 4**：删 `events-v2` 拒绝后，旧对端只能靠协议版本拒绝。endpoint 格式的版本 `ENDPOINT_PROTOCOL = 3`（`session/mod.rs:18`）从 2026-10-06（`ee5b605c`）起没变，0.11–0.18 的远程运行时都说 3；#811 §七 写"0.18 的宿主或运行时：握手时按协议版本拒绝"，这句本身也要求 0.19 的版本与 0.18 不同。决定（用户 2026-10-28 确认）：`ENDPOINT_PROTOCOL` 升到 4（WebSocket 子协议 `rutis.4`），落在 #811 的 S2b（与删 compat 同一个 PR）。0.18 及更早的宿主或运行时连上时在握手被拒，不重试，文字说明升级哪一端（真实相遇的文字，与 #811 §七 同）：本机 `incompatible session: the far end speaks protocol 2, this side 4: upgrade the far end`；WebSocket 在子协议那一步，0.18 拨号方写出 `this endpoint speaks rutis.4, the dialer offered rutis.3: upgrade the dialer`，0.18 监听方答 `this endpoint speaks rutis.3, the dialer offered rutis.4: upgrade this endpoint (<它的包>)`。本稿的协商删除（CL11a）排在 S2b 之后。
10. **`greeting` 的载体被 #811 删**：它挂在 `LocalRuntime`（`local.rs:50,152,293`），经 `LinkConfig::local_runtime()`（`link.rs:140,154`，默认 30 秒）与 `Retry::handshake` 生效，三者都在 #811 删除清单里，#811 没给新位置。处理：`greeting` 成为 `rutis-host/spawn` 的 Rust 构造选项（默认 30 秒），与现在一样只在 Rust 嵌入上有；rutis.json 不加字段。与 #811 S4 同一个 PR。
11. **`plugins` 能力**：Rust 的自查（`rpc.rs:2421`）是死分支，删。六个 SDK 的"只向声明了 `plugins` 的对端发插件值"不是旧版兼容，是保证 `bridge/plugin-values` 的内容，并且 #811 的 `sessions.open` 会出现运行时对运行时的会话；不删。
12. **`objects` 的删除连带一致性夹具**：Go 夹具 `go/rutis/internal/fixtures/session/main.go:193` 故意不声明 `objects`，`rpc/tests.rs:2676–2706` 测"不能发到 py"，跨语言用例 `objects.expected.json:180` 的 `no-objects-declared`。这些随删除一起删。

## 五、rutis-loader

### 5.1 删

| 名字 | 位置 | 调用方（核实） | 类 | 一起改的 |
| --- | --- | --- | --- | --- |
| 为 0.18 之前 Go 二进制保留的分支：`config::Options`、`Options::required`、`without_required`；`config_step.rs` 的 `options`、`older_than_0_18` | `config.rs:19–41,66,405`；`config_step.rs:98–166,223–255` | 生产只有 `config_step.rs:108`；测试 `tests/config_cases.rs:6,45,80,97–100` | C | `config::check` 从 3 个参数变 2 个（随 3.4 第 2 条搬进内核时一起做）；`rutis-host.md:238` 删；《迁移到 0.19》写"用 0.18 及以后的 SDK 重新构建 Go 插件" |
| `ServiceCatalog::share_by_name`、`share_registered_only`、`by_name` 字段 | `catalog.rs:116,134,289,298,426` | `share_by_name`：生产 rutis-host `host.rs:216`、e2e `fixture-toolhost/src/main.rs:171`，测试 `runtime_rows.rs:1036`、`fault_containment.rs:170`、`remote_modes.rs:199`、`runtime_instances.rs:336`，均为空操作（默认已是 true）。`share_registered_only`：测试 `check.rs:106`、`scope.rs:52`、`marked_services.rs:253`，指南 `rust-host.md:434`、`open-host.md:129` | C | 与 #811 §八 的 `share_registered_only` 一行重叠：由本稿 CL17 一次做完，#811 S5 不再做这一行（#811 没列 `share_by_name` 的调用方）。`shared_key` 恒为真后，`catalog.rs:426` 对 `is_exported` 的用法删 |
| `Loader::waiting_for` | `loader/api.rs:178–202` | 只有测试 `marked_services.rs:458,466` | C（与 `waits`/`waits_of` 重复，`WaitReason::Explained` 已覆盖） | `guarantees.md:279`（`bridge/marked-services` 的文字）、`rust-host.md:243,487`、`.agents/skills/rutis-remote-plugin/SKILL.md:48` 改为 `waits` |
| `Loader::locate` | `api.rs:237–264` | 零调用 | C | — |
| `Loader::schema_of` | `api.rs:267–269` | 测试 `loader.rs:458,460`、`peer_rows.rs:230` | C（等于 `resolve(name).await?.schema`） | 保证 `loader/schema`（`guarantees.md:358`）的写法与测试改为 `resolve(..).schema` |
| `register_instance_keyed`、`readable_instance_keyed`、`readable_instance` | `catalog.rs:183,210,200` | 前两个只被同文件的方法调用；`readable_instance` 只有测试 `instance_services.rs:170` | C | `register_instance_keyed` 的函数体内联为私有函数；dsh 用的是 `readable`、`readable_keyed`，不受影响 |
| Go/Swift 旧名别名（每种 **8 个**，清单写 7 个）：`GoBinaries`、`GoManifest`、`GoBinary`、`GoResolver`、`GoRuntimes`、`GoRuntimesHandle`、`GoRuntimeState`、`GoRuntimeInfo` 与 Swift 同名 8 个 | `runtime/go.rs:56–78`、`swift.rs:55–76` | rutis-host 不用；测试十几个文件；指南 `rust-host.md:539–547`（doctest）、`go-plugin.md:414` | C | `struct Go`、`struct Swift` 不是别名，rutis-host 在用，留。错误文字 `"{}Runtimes restart"`（`binary.rs:1044,1080`）展开成 "GoRuntimes restart"，改为写运行时名 |
| `read_go_manifest`、`read_swift_manifest` | `go.rs:83`、`swift.rs:81` | 前者零调用；后者只有测试 `swift_rows.rs:18,130` | C | — |
| `internal` 的 `GO_MARKER`、`SWIFT_MARKER`、`resolve_entry` | `lib.rs:89–105` | 无外部调用 | C | 常量内联进 `Go`/`Swift` 的 impl；`resolve_entry` 随 npm 解析删 |
| 只转 bridge 的 feature `python`、`bun`、`kotlin` | `Cargo.toml` | src 里无代码门控（`python` 只门控 `lib.rs:36,47` 的 doctest） | C | 测试门控按 4.4 第 5 条；rutis-host `Cargo.toml:33`、`examples/marked-services`、`examples/rust-services`、`tests/e2e/fixture-tenants` 的 `features` 同步删。`node`、`go`、`swift` 门控 loader 自己的代码，不在清单内，留 |
| `allowed_services` 的检查 | `runtime.rs:1014–1019` | 见 §四 #683 一节（4.5） | C | — |
| Rust 里重写的 npm 解析：`npm` 模块（`runtime.rs:1449–1618`，170 行）、`package_version`（`:1291–1306`）、`Naming::Npm` 的本机分支、`pub use npm::resolve_entry`；`url` 可选依赖 | `runtime.rs` | `RuntimeResolver::node`（rutis-host 默认的 Node 运行时 `languages/node.rs:84`） | C（与运行时自己的解析重复） | 见 5.4 第 1 条 |
| Go/Swift 的限定行名写法 `go-<文件>:<插件>`、`swift-<文件>:<插件>` | `binary.rs:990–1012,1084–1094` | 见 5.4 第 2 条 | C | — |
| 一行状态的第二套说法 `status::RowState` | `loader/status.rs:61` | rutis-host `call.rs:10,137`；测试 | C | 见 5.4 第 3 条 |

### 5.2 搬到 rutis-host

| 名字 | 位置 | 处理 |
| --- | --- | --- |
| Go/Swift 二进制运行时的重启策略：`RestartPolicy`、`RuntimeEnd`、`RuntimeEnd::line`、`Supervision`、`supervise`/`unsupervise`、结束处理与退避（`restart_after_end`）、字段 `backoffs`/`pending` | `runtime/binary.rs:1231–1289,1398–1411,1600–1650,1960–2075` | 与 rutis-host 的 `restart.rs`（它已 `pub use rutis_loader::backoff::*`，`restart.rs:17`）合成一份；两边各写一遍的 `"runtime {name}: restarting"`、`"gave up after {LIMIT} restarts within {} minutes"`（`restart.rs:55–62`、`binary.rs:1271–1275`）只留一份。起进程归 #811 的 `rutis-host/spawn`，重启随它 |
| `backoff` 模块（68 行，7 个公开项） | `backoff.rs` | 搬 rutis-host |
| `BinaryRuntimes::prefix_output` | `binary.rs:1550–1557,2293–2310` | 随 4.2 的输出前缀搬 |

loader 只留 `restart(name)` 与状态。**按需启动与闲置停止留在 loader**：两者在同一个定时循环 `sweep`（`binary.rs:2120–2200`，`idle` 默认 60 秒 `:1528`、`eager` `:1534`）里，读 `loader.entries()` 判断有没有行在用，是对账的一部分。#811 之后二进制运行时是 `rutis-host/spawn` 的行，`sweep` 按需启停这些行。

### 5.3 接收与留

接收：内核移来的东西按 3.4 第 2、3 条不再移过来；loader 改为调内核的 `rutis::config::check`。

| 名字 | 位置 | 理由 |
| --- | --- | --- |
| 配置表达式：`Expressions`（`catalog.rs:526`）、`ExprScope`（`:459`）、`{"__jsExpr":…}`（`loader/desired.rs:111,161`、`loader/mod.rs:90`）、`readable`、`readable_keyed`（`catalog.rs:226,233`）、`NotReadable`（`error.rs:103`） | — | Cordis/dsh 兼容：dsh 的 profile 用它们（`profile/mod.rs:23,73`、`profile/expr.rs:19,782`、`profile/yaml.rs:22`）。见 5.4 第 4 条 |
| `patch.rs`（367 行） | — | Cordis/dsh 兼容：照搬 cordis-plugin-include 的格式与行为 |
| 层叠（`Layer`、`apply_patches`） | — | dsh 在用（它的 `--patch` 就是一层） |

`internal` 模块（给外部的后门，`lib.rs:89–105`）按下表处理，处理完删掉模块：

| 项 | 外部调用方（核实） | 处理 |
| --- | --- | --- |
| `apply_patches` 及 `Composed`、`ComposedRow`、`Owner`、`PatchWarning` | dsh 生产代码 `profile/mod.rs:98`、`profile/layers.rs:19,562`、`profile/include.rs:16,46,141,250`（**不是 rutis-host**）；`Owner`、`PatchWarning` 已出现在公开字段 `EntryInfo.owner`、`ReconcileReport.warnings` 里 | 转成公开 API（`rutis_loader::patch`），理由是 dsh 兼容 |
| `is_runtime_name` | rutis-host `project.rs:133`、`dev.rs:611`、`config.rs:608`（后者 #811 删） | 公开（运行时名规则属于行名规则，与 3 条同） |
| `read_binary_manifest` | rutis-host `languages/go.rs:170` | 公开 |
| `PLUGIN_API`、`SWIFT_PLUGIN_API` | rutis-host `main.rs:32,33` | 删；rutis-host 改用 `Go::PLUGIN_API`、`Swift::PLUGIN_API`（`BinaryLanguage` 的公开关联常量，`go.rs:335` 已这样用） |
| `node_schema`、`REMOVED_EVENTS` | rutis-host `config.rs:861` | #811 处理（peer 整体删） |
| `GO_MARKER`、`SWIFT_MARKER`、`resolve_entry` | 无 | 删（5.1） |

`install` feature（#691）不进 loader：核实 loader 没有这个 feature，#805 正文写明"rutis-loader 不变"。

### 5.4 核实后与清单不同的地方

1. **不带前缀的 npm 包名是现在的主写法，也是 dsh 兼容要的写法**。清单写"行名不能再写不带前缀的 npm 包名"。核实：rutis-host 默认 Node 运行时走 `RuntimeResolver::node`；指南 `rutis-host.md:75,596`、`typescript-plugin.md:283,293`、`rust-host.md:440,460` 都写包名；#805 的 `add` 给 Node 写的就是裸包名，`Installed::of_row` 明确拒绝 `node:` 前缀；持久化样本 0.8–0.18 有 `weather-plugin` 等；dsh 的 Cordis 行（`@deepseek-ai/dsh-storage`、`@rutis/dsh-aimux`）是 Cordis 的写法——Cordis 插件名就是 npm 包名。决定（用户 2026-10-28 确认）：**删 Rust 里的 npm 解析，保留不带前缀的行名**，名字原样交给 Node 运行时解析（现有的远程分支 `runtime.rs:130–133` 已经这样做，#811 统一本机与远程后本来也只剩这一条路）。`RuntimeResolver` 只有一个，Node 行名两种：不带前缀（默认 Node 运行时，Cordis/dsh 与 #805 的写法）、`<运行时名>:<包或文件>`（每个插件一个进程，#683 H1 的 `modules`）。代价：失去 Rust 一侧按 `package.json` 的 `version` 判断过期；运行时重启后的重新解析由已有的按会话判断（`take_stale`，#701）覆盖，运行中的 Node 不会因为文件变了换代码，所以不需要版本判断。删掉约 215 行与 `url` 依赖，不需要迁移。
2. **Go/Swift 两种行名是两种用途**：`go:<插件>` 在所有二进制里找，`go-<文件>:<插件>` 在同名冲突时指定二进制；`rutis-host dev` 的 `dev_rows`（`languages/go.rs:107–109,164–178`）用限定写法把行钉在开发构建上。#805 的 `add`/`installed`、指南主写法都用 `go:<插件>`。处理：**只留语言前缀写法 `go:<插件>`、`swift:<插件>`**。理由：它不把行名和文件名绑在一起（二进制改名行不失效），#805 与指南已经用它。两个二进制提供同名插件时启动报错，写明两个文件名，由插件作者改名——与内核"同名后来的被拒"一致，不另给消歧写法。`dev` 下 Go/Swift 运行时只看开发构建的目录（`.rutis/go/`），不看已装目录，所以 `go:<插件>` 只命中开发构建。持久化样本里的 `go-dns:resolver`、`swift-MacKit:clipboard` 按 #811 §十一 第 8 条的同一做法：样本删掉这些写法，加 `rejected/` 用例。
3. **`report::RowState` 是协议值**（线上 `rutis/row` 事件的取值，[事件协议](design-events-protocol-2026-10-18.md) §8、保证 `events/framework`），不能并进带载荷的 Rust 枚举。另有第四套 `check::RowStatus`（`loader/check.rs:79`，`check` 的结果，不是运行状态，本稿不动）。处理：`status::RowState` 删，状态快照直接用 `EntryStatus`（`Starting` 由 `Running` 的快照阶段给出）；`report::RowState` 作为线上取值保留，改名 `RowEventState`，避免两个同名类型。完成条件：`rutis-host status`、`call` 的输出逐字不变。
4. **配置表达式的公开名字里本来没有 JS**：只有线上键 `__jsExpr` 与文档注释带 JS。`__jsExpr` 是 Cordis/dsh 的格式键，dsh 的 YAML（`!!js` → 这个节点）、持久化样本 `0.15.1`、`0.17.2` 的 `cordis.patch.expected.json` 都在用。处理：键名不改，只改文档注释的措辞。
5. **`Loader::set_overlay` 没有 dsh 使用**：dsh 的 `--patch` 是普通的应用层（`profile/layers.rs:550–557` 做成 `Layer::new("patch:…")`，经 `watch.rs:95` 的 `reconcile`），不走 `set_overlay`。它唯一的生产调用方是要删的 rutis-dev（`server.rs:285,309`）。清单的保留理由不成立。决定（用户 2026-10-28 确认）：overlay 层删除——`Loader::set_overlay`（`api.rs:115`）、`overlays()`（`api.rs:97`）、overlay 状态（`loader/mod.rs:412,528–536`、`loader/commit.rs:312–332`）、测试 `tests/overlay.rs`（187 行）、保证 `loader/overlay`（`guarantees.md:356`）；与删 rutis-dev 放在同一个闭环（§十三 CL23）。
6. **别名是每种语言 8 个**，已在 5.1 按实数写。

### 5.5 #683 宿主限制（rutis-bridge 与 loader 两层一起写）

| 名字 | 位置 | 调用方（核实） |
| --- | --- | --- |
| `call_limit`、`kill_on_timeout` | bridge `runtime/local.rs:51–54,153–154,251,263`；`runtime/plugin.rs:247,341,356–380,515`；`runtime/rows.rs:15,96–159`；`session/rpc/limits.rs`（188 行）；`session/rpc.rs:307–398,980,1382,1960–1990,2413,3260`；`session/rpc/relay.rs:122–182`；`report.rs:133–135` | 生产无；测试 bridge `tests/plugin_limits.rs` 6 个 |
| 错误码 `timeout` | `session/mod.rs:123–126` | 六个 SDK 与 specs 都没有这个码 |
| `allowed_services`、`may_use` | `local.rs:278`；`plugin.rs:30,40,348`；`report.rs:117–125`（#793 只为它加的 `unavailable` 分支）；loader `runtime.rs:1015–1019` | 生产无；测试 loader `tests/plugin_limits.rs` 3 个 |

处理：全部删（类 C）。理由：唯一的使用方是 [#663 评估](https://github.com/arcships/rutis/issues/663) 里假想的宿主 deskmate（代码里没有，只出现在设计稿与指南的示例名里）；调用方自己限时（0.18 [诊断设计](design-diagnostics-2026-10-26.md) §6.1，`:211–224` 附各语言写法），超时后宿主 `processes().kill()`（`transport/local/spawn.rs:988`）加 `FiberView::restart()`（`crates/rutis/src/fiber.rs:1828`）；能用哪些服务是业务分配。`#796` 为 call_limit 给 `request_sync` 加的 `deadline` 参数退回原样。`processes()` 现在挂在 `LocalRuntime`（`local.rs:306`），#811 删 `LocalRuntime` 后它在 `rutis-host/spawn` 的句柄上，指南写那时的位置。

留：`greeting`（4.4 第 10 条）；#683 H1"每个插件一个进程"（`RuntimeResolver::modules`，测试 `each_plugin_runs_in_its_own_process`、`a_runtime_waiting_for_a_debugger_stays_starting`）。所以保证 `host/plugin-limits` 是收窄（只留 greeting 与每插件一进程，验证列去掉 8 个测试），不是删整行；open-host 第 3 节删"调用时限""能用的服务"两小节，示例代码按 #811 改写。

## 六、rutis-host 与工具 crate

### 6.1 删

| 名字 | 现状（核实） | 一起改的 |
| --- | --- | --- |
| `crates/rutis-dev` 整个 crate | src 588 行、tests 389 行、公开项 13；版本 0.1.0，`publish = false`；无任何 crate 依赖；不在发布列车；功能与 rutis-host 的 `control.rs`（`status`、`call`）、`dev.rs:376`（`reload`）、`status.rs`（状态变化）重复 | 同一个闭环删 loader 的 overlay 层（5.4 第 5 条）；根 `Cargo.toml:3`、`Cargo.lock`、`ci.yml:175` 与 `nightly.yml:112` 的路径过滤、`docs/ci.md:80`、`docs/quality-status.md` 第 59、71、105、142、411、413 行、`design-host-dev-mode-2026-09-25.md:4` 的状态行 |
| `crates/rutis-migrate` | src 3354 行（`lib.rs` 3226、`main.rs` 128）、tests 286 行；只做 0.16→0.17；无 crate 依赖 | 退出发布列车：`scripts/train.mjs:26–27`、`release.yml:8` 的注释、`docs/release.md:9` 的表、根 `Cargo.toml`、`Cargo.lock`。crates.io 上的旧版仍可 `cargo install`，所以 `crates/rutis/README.md:53`、`guide/README.md:32`、`migration-0.17.md` 的指向保留。#811 §八 把"rutis-migrate 的名字表"（`lib.rs:2878`）列为影响方，crate 删后那一项不用做 |
| `rutis-host go add` | `main.rs:383–416`（约 35 行）、分发 `:100`、说明 `:11`、用法 `:76–78`；无专门测试 | 见 6.3 第 1 条 |

### 6.2 留

dylib 整族：`rutis-dylib`（src 1231 行）、`rutis-dylib-meta`（974）、`rutis-dylib-launcher`（292）、`rutis-sdk`（243）、`rutis-xtask`（1347），以及内核的库代次（3.3）。理由（用户定）：它是我们支持 Rust 动态插件的方式（设计哲学"dylib 也都是插件"）。现状（核实）：依赖它的只有示例 `crates/rutis-cli`（feature `dylib-plugins`）与 `tests/dylib-fixtures`；rutis-host、loader、bridge、dsh 都不加载 dylib。本稿不扩展。

### 6.3 rutis-host 接收与不再用的东西

接收（§四、§五）：语言启动（`Launcher::*` 的语言部分）、二进制运行时的输出分行与前缀、重启策略（与 `restart.rs` 合成一份）。不接收：`report_line`（4.4 第 6 条）、trace（4.4 第 7 条）。

rutis-host 不再用 `rutis::internal` 与 `rutis_loader::internal`。核实到的全部使用：

| 位置 | 用到的 | 改为 |
| --- | --- | --- |
| `call.rs:107`（生产） | `rutis::internal::config::closest` | `rutis::config::closest`（3.4 第 2 条） |
| `status.rs:580`（测试） | `rutis::internal::config::decode` | 经公开的 `Definition` 与 `ConfigError` 构造错误 |
| `host.rs:1366`（测试） | `CtxInternal` | 测试改用公开的 `CtxHost` |
| `config.rs:608`、`dev.rs:611`、`project.rs:133` | `is_runtime_name` | loader 公开 API（5.3） |
| `languages/go.rs:170` | `read_binary_manifest` | loader 公开 API |
| `main.rs:32,33` | `PLUGIN_API`、`SWIFT_PLUGIN_API` | `Go::PLUGIN_API`、`Swift::PLUGIN_API` |
| `config.rs:861` | `REMOVED_EVENTS` | #811 处理 |

### 6.4 核实后与清单不同的地方

1. **#805 保留了 `go add`**：#805（未合并）让 `go add` 改调 `install_go_module`，它的设计稿兼容一节与 I1 写"`go add` 保留，行为不变"。行为差异：`add <Go 模块>` 用同样的 `GOBIN=runtimes.go.dir go install` 安装，另外写 `installed` 记录与行；`go add` 只装二进制、不改配置。处理：照清单在 #805 合并后删 `go add`（§十三 CL25），同一个 PR 改 `docs/design-plugin-install-2026-10-27.md` 的兼容一节与 I1、`rutis-host.md:78,96`、`go-plugin.md:411`、`templates/go/README.md:11`、保证 `host/go-add`。"只装不加行"的用法不再有：插件要被加载本来就要一行。
2. **`Loader::set_overlay` 不是 dsh 在用**，随 rutis-dev 一起删（5.4 第 5 条）。
3. **`apply_patches` 的使用方是 dsh，不是 rutis-host**（5.3 的表已按事实写）。

## 七、各语言 SDK

### 7.1 删：#692 的旧名转发

不做兼容：旧位置直接删，不留转发与弃用声明；仓库自己的示例与测试改用新位置；[SDK 入口收敛](design-sdk-entry-convergence-2026-10-27.md) 与《迁移到 0.19》改成"直接移动，无旧名"。

| 语言 | 位置与数量（核实） | 一起改的 |
| --- | --- | --- |
| TypeScript / Bun | `node/rutis/src/index.mjs:49–51` 转发 9 个值；`index.d.ts:48–82` 16 个 `@deprecated`（7 个类型、9 个常量） | `node/rutis/src/testing.mjs:37` 与 `testing.d.ts:1` 从旧位置导入，同一个 PR 改为 `./advanced` |
| Python | `__init__.py:15–17,21–42`（`_MOVED`、`__getattr__`、`__dir__`）、`__all__` 里 7 个旧名 | `public-api.txt:25–32` 的 deprecated 一节；`tests/test_advanced.py` 测弃用的用例；`test_public_api.py:23–25,119` |
| Go | 根包 33 处 `Deprecated:`（`values.go` 13、`events.go` 7、`runtime.go` 4、`serve.go` 4、`version.go` 3、`plugin.go` 1、`schema.go` 1）；`advanced` 现在是反向别名（`= rutis.X`） | 见 7.6 第 2 条；`public-api.txt:52` 的 deprecated 一节、`public_api_test.go:85–130`；`scripts/train.mjs:43` 与 `release.md:18` 读 `version.go` 的 `const Version` |
| Kotlin | `Moved.kt` 19 个 `@Deprecated typealias`；另有 `jvmMain/Serve.kt:35,65`（`Rutis.project`、`Rutis.manifest`）、`Rutis.kt:34,38,42`（`VERSION`、`PLUGIN_API`、`IMPLEMENTATION`），共 24 处 | 三个版本常量的定义在被弃用的 `Rutis` 对象里，`advanced/Versions.kt:8–14` 反向引用它：字面值挪进 `Versions.kt`；`scripts/train.mjs:48`、`release.md:18`；`api/*.api` |
| Swift | `API/Moved.swift` 25 个 `@available(*, deprecated)`；定义都已在 `Rutis.Advanced` 与 `_Generated` | `public-api.txt:190` 的 deprecated 一节 |

仓库内（templates、examples、tests、docs、.agents）没有用旧名的地方，核实为零，只有上表列的 SDK 自身文件。

### 7.2 删：为旧宿主保留的分支

| 项 | Node（rutis-runtime） | Bun | Python | Go | Swift | Kotlin |
| --- | --- | --- | --- | --- | --- | --- |
| `announce` 缺失的提示（早于 0.12 的宿主；bridge 总发 `announce: true`，`process.rs:1079`，死分支） | `runner.mjs:65–68,438–444,862`；`leaf.mjs:70–90` | `runner.ts:94–97,168–171,226–235,477` | `runner.py:139–146,347–352,396–405` | `runtime.go:187–192,238–250,264,428–437,717–719`；测试 `oldhost_test.go`（整份） | `Runtime.swift:340–342,477–487,685` | `internal/Runtime.kt:41–47,182–187,264–272,578–581` |
| `events-v2` 握手检查与报错 | `events.mjs:63–65`（三份副本）、`runner.mjs:1083,1090–1095`、`session.mjs:72–82`、`dial.mjs:53–54` | `main.ts:139–148`、`session.ts:73–82,262`、`remote.ts:223–224` | `peer.py:49–58,230,879`、`__main__.py:91–95`、`runner.py:38` | `serve.go:228–231,284–295`、`internal/peer/peer.go:56–58,1584–1590`；测试 `events_session_test.go:455` | `Serve.swift:3–4,134,148–149`、`Peer.swift:20,97–98,1259–1263` | `Sessions.kt:16–18,68–70`、`Peer.kt:49,101–102,1210–1212` |
| `refuseRemoved`（`events`/`emits` 字段的迁移报错） | `events.mjs:55–61`、`runner.mjs:851`、`generate.mjs:43–49,1032–1034`、`testing.mjs:694–702` | `runner.ts:32,474` | `runner.py:102–107,1126–1128`、`testing.py:788–792`（`load(emits=…)`） | 无 | 无 | 无 |
| 插件 API 1、2 的装载 | `leaf.mjs:15–35` 的 `api ?? 1` 与注释 | `plugin.ts:86–89` | `plugin.py:108–111`、`runner.py:1374–1379` | 无 | 无 | 无 |
| `rows.failed` 兜底 | `runner.mjs:1043–1046` | 无 | `runner.py:649–652` | 无 | 无 | 无 |
| `objects` 兜底 | `session.mjs:348–349` | `session.ts:337–338` | `peer.py:1150–1156` | `peer.go:749–750`、`crossing.go:153–166` | `Peer.swift:733–742` | `Peer.kt:500`、`Crossing.kt:46–50` |
| `sync` 字段（只在对端声明 `sync-wait` 时发） | `session.mjs:573–575` | `session.ts:563–565` | `peer.py:1436–1439` | `peer.go:1113–1116` | `Peer.swift:930–932` | `Peer.kt:738–740` |
| `waterfall-sync` 接收分支（§三） | `events.mjs:524,540`（两份）、`bridge/features.mjs:341,345` | `events.mjs:524,540` | `runner.py:1037–1038` | `internal/events/events.go:105,111,118,349`、`events.go:710` | `EventsCore.swift:111,113`、`RuntimeEvents.swift:478` | `Bus.kt:34,38`、`EventsRuntime.kt:458`、`api/*.api` |

另删两处（清单外，用户 2026-10-28 确认），放进各自语言的闭环：

- Go 只在 dev 下生效的 `warnChainless` 提示（`go/rutis/runtime.go:1165–1192`，调用 `events.go:537`、`runtime.go:1075`；`devMode` 只为它存在）。`RUTIS_DEV` 仍由 Node、Bun 的行跟踪使用（`runner.mjs:106`、`plugin.ts:125`），rutis-host `dev.rs:54` 照旧设置。`design-go-runtime-2026-10-10.md:257` 的一行删。
- Kotlin、Swift 的"旧宿主的 emit 回复是 future"分支（`EventsRuntime.kt:305–311`、`RuntimeEvents.swift:350–359`）：核实发现它不是旧宿主的兼容，见 7.6 第 11 条。

"插件 API 1、2 的装载"没有专门的分支，三种脚本运行时都只查 `api > PLUGIN_API`。处理：删 `api ?? 1` 的默认与注释；插件对象由 `definePlugin` / `plugin(...)` 生成，总带 `api`。不新加 `api < 3` 的拒绝。

`events-v2` 的 `Require`/`Refuse` 机制在 Go（`serve.go:290`）与 Kotlin（`Listen.kt:339`）还被远程拨号的 `incompatible` 停止路径使用：只删 `events-v2` 这一项要求，`Require`/`Refuse` 本身留。

### 7.3 删：Python 3.10、3.11

最低版本提到 3.12。删 `python/rutis/rutis/_eager.py`（161 行，调用方 `events.py:33,126–128`、`peer.py:36,1819–1826`）、`_unhandled.py:56–79,94–99` 的回退、`peer.py:277` 的注释、`tests/test_eager.py`。`requires-python` 三处（`python/rutis/pyproject.toml:7`、`crates/rutis-host/pyproject.toml:13`、`crates/rutis-host/templates/python/pyproject.toml:6`）；CI 写死 3.10 的任务（`ci.yml:448,606,690–702,940–952,1042,1142–1151`、`install.yml:19,108,109`，按 `docs/ci.md:288` 的规则同一个 PR 改）；文档见 §十二。生成的 Python 类型仍按 3.10 的写法（`build/exports.rs:82,366,386,1233`），本稿不改输出（§八）。

### 7.4 搬：Kotlin 测试工具

`RutisTest.kt`（691 行）在 `kotlin/rutis/rutis/src/shared/kotlin/arcships/rutis/testing/`，`build.gradle.kts` 让 `jvmMain` 与 `androidMain` 都收这个目录，所以打进每个 Android 应用；它 import 17 个 `internal.*` 名字，并直接调 `internal.ConfigSchema`。处理：拆成单独的 artifact `rutis-testing`，在 7.5 的改写之后它只用公开 API，不再 import `internal.*`。`kotlin-app.md:67,102,209` 与 `kotlin/README` 的依赖坐标跟着改。

### 7.5 改：测试工具、配置校验、事件分发——叶子只剩叶子

三件事在同一份代码里互相依赖（测试工具用本地事件总线与 SDK 的配置校验模拟宿主），按语言一起改。

**测试工具改为"真实运行时 + 真实 rutis 内核"。**

- rutis-host 加 `rutis-host test`：`rutis-host test <驱动测试的插件> -- <运行时命令…> <项目>` 起一个没有行的宿主，像本机运行时那样启动运行时（`pipe::spawn`：fd 3；Windows 上是本机回环地址加一次性 token），与它建一条 #811 的 session。宿主里有一个普通的 Rust 插件 `rutis-host/test`，只用 loader 的公开 API，提供服务 `rutis-host/test`：`load(name, config)`、`update(id, config)`、`unload(id)`、`reported(id)`。它属于 rutis-host（参考宿主与工具），平台不变。`rutis-host test <驱动测试的插件> --channel <通道>`（CL27b）接测试工具在自己进程里跑的运行时：测试工具按本机运行时的交接方式把通道交给它（Unix 继承 fd，`fd:<n>`；Windows 与 `RUTIS_LOCAL_HANDOVER=loopback` 下是本机回环地址加一次性 token，`tcp:<主机>:<端口>`），给要在测试进程里调用被测插件的 Go、Swift、Kotlin 用。
- 各语言的测试工具以本语言真实运行时的命令运行 `rutis-host test`；被测插件经 `load` 成为一行，由宿主按 #811 的 `run` 跑在这个运行时里；测试方的假服务、`t.on`、`emit` 是同一运行时里的一个测试插件，用公开的 `ctx.provide`、`ctx.on`、`ctx.emit`。
- 删掉各语言模拟宿主规则的部分：Node `testing.mjs` 的值跨进程语义（`:46–360`）、依赖检查与它用的 `config.mjs`/`events.mjs` 部分；Python `testing.py` 的 `_cross` 等（`:93–360`）；Go `rutistest.go` 的宿主分发（`:800–931,955–1213,1223–1290,1359–1403,350–391`，约 550 行）；Swift `RutisTest.swift` 的 `Harness`（`:61–508`）；Kotlin `RutisTest.kt` 的 `Harness`（`:101–547`）。
- 公开写法不变：Node `load`、`host`、`Loaded` 的 `service`/`methods`/`provided`/`reported`/`on`/`update`/`unload`/五种发出方式/`filter`；Python `load`、`Bus`、`Loaded`、`Reported`；Go `Load`、`TryLoad`、`Loaded`；Swift、Kotlin `RutisTest.load`、`scope`、`Loaded`。Python `load(emits=…)` 参数随 `refuseRemoved` 删。
- 找 rutis-host：`PATH` 上的 `rutis-host`，或 `RUTIS_HOST` 指定的路径。Node 测试工具 `@arcships/rutis/testing` 动态导入 `@arcships/rutis-runtime`（SDK 包不静态依赖运行时包，`leaf.mjs` 的约定不变）；模板把 `@arcships/rutis-runtime` 与 `@arcships/rutis-host` 加为开发依赖。Bun 用同一份测试工具，按 `typeof Bun` 起 Bun 运行时。
- 0.19 里测试工具只用本机运行时（#875 定）：设备上的测试（Android instrumented 测试、iOS 模拟器）不用 `rutis-host test`，那里由应用自己装插件。
- H4（测试工具拒绝插件提供宿主已有的服务名，#810）由真实内核报，`testing.mjs`、`testing.py` 里的两处手写检查删；报错文字以内核为准。
- 测试速度：现在单个测试在 1 毫秒量级（`node/rutis` 全部 169 个测试 1.4 秒，实测）；起一次宿主估计 150–500 毫秒。做法：每个测试文件起一个宿主，用例之间卸载行，不重启宿主。可接受：模板项目的测试（一个文件）总时间不超过 2 秒；`node/rutis` 与 `python/rutis` 全部测试不超过现在的 3 倍。超出时停下报告，不恢复模拟宿主。

**配置 Schema 校验只留 Rust 一份。** 校验与补默认值在内核 `rutis::config::check`（3.4 第 2 条），loader 的行与 bridge 的插件值都调它；loader 把补过默认值的配置发给运行时（现在只存原值、不补别的运行时的行，[配置 Schema](design-config-schema-2026-10-27.md) §2.1 第 47 行）。SDK 只解码：

| 语言 | 删 | 留 |
| --- | --- | --- |
| Node / Bun | `config.mjs` 三份副本（各 476 行，md5 相同，`config.test.mjs:43–45` 查一致）的校验 | `schemaOf`（`definePlugin` 拒绝 Standard Schema，`index.mjs:28,36`）、`configError`、`pathText`（`cordis-config.mjs:1` 格式化 Cordis 的错误），约 20 行挪进 `cordis-config.mjs` 与 `index.mjs` |
| Python | `config.py`（626 行）的校验 | 解码 |
| Go | `internal/config`（`config.go` 785、`text.go` 237）的校验与补默认值 | 解码与"is not a value the plugin accepts"的改写（`config.go:22–43`） |
| Swift | `ConfigSchema.swift`（670 行） | `ConfigSchema.path(of:)` 的解码路径 |
| Kotlin | `internal/ConfigSchema.kt`（566 行） | `DecodePaths` |

应用自己装的插件（S4a 的 own plugins）不经 loader，没有宿主替它校验：它们的配置只解码，缺的键取作者在类型里写的默认值（Swift 由 `@RutisConfig` 宏生成带默认值的解码，不靠合成的 `Decodable`），不做 Schema 校验；缺必填的键或类型不对时报解码错误，文字点名键，与内核 `rutis::config::check` 的措辞一致（`config: count is required`、`config: count should be a number, got "x"`）。宿主装的行仍由内核校验后再交给运行时。

`specs/config-schema/cases.json` 只由内核（从 loader `tests/config_cases.rs` 搬来）与 `rutis-host check` 跑；各 SDK 的用例运行器删，`specs/config-schema/README.md:5` 改写。Cordis 行不按转换出来的 Schema 校验（§2.3）的例外保留在 loader。测试工具的多余键告警由真实 loader 给出。

0.18 [配置 Schema](design-config-schema-2026-10-27.md) §2.1 的五条理由逐条对照：第 1 条"测试工具里没有 loader，只能放 SDK"——测试工具改走真实内核后不成立；第 2 条"远程运行时由运行它的一端检查"——loader 对每一行（含连进来的运行时的行）已经拿得到 Schema（`runtime.rs:546` 的 `schema: described.config`），不成立；第 3 条"类型化语言本来要解码"——仍成立，只留解码；第 4 条"一份规则靠用例表保持一致"——只剩一份实现后不再需要；第 5 条"0.18 以前的 SDK"——不做兼容，作废。§2.1 的结论改为"校验在内核一份，SDK 只解码"。

**事件一律交给宿主分发。** 删各语言本地分发：五种方式的本地实现、本地过滤、本地顺序、`events.elsewhere`。

| 语言 | 删的位置（核实） |
| --- | --- |
| Node / Bun | `events.mjs`（三份副本，558 行）的 elsewhere（`:273–301`）、本地五种方式（`:310–455`）、`#keeps`、`#unanswered`；`#viaHost`（`:462–509`）与宿主回调 `call()`（`:515–558`）留 |
| Python | `events.py:355–494`；`runner.py:360–362,895–1020,1122–1123` |
| Go | `internal/events/events.go:231–422` 的 `Dispatch` 与 `Queue`；`events.go:403–439,590–617` |
| Swift | `EventsCore.swift:224–391`；`RuntimeEvents.swift:212–310,417–436` |
| Kotlin | `Bus.kt:120–217`；`EventsRuntime.kt:221–266,377–406` |

基准（先于删除，单独一个 PR）：新写 `tools/bench/events`，在 Node 与 Python 运行时里各测一种情形——发出方与监听器在同一个运行时，分别用 `emit`、`serial`、同步 `bail`，每种 10 000 次，报中位数与 p99；分别在删前（本地分发）与删后（经宿主）跑。可接受：经宿主的 `emit` 中位数按实测值接受——Node 71 微秒、Python 139 微秒（`docs/performance-events-2026-10-28.md`；原定 100 微秒，Python 超出，用户看过数字后决定照删），经宿主的路径以后可优化；并且 dsh 的基线（`node/baseline/`）启动与跑完一遍的时间不超过现在的 1.1 倍。dsh 基线超出时停下，把数字报给用户，不保留快速路径。

删除带来的行为变化（写进《迁移到 0.19》）：Kotlin 在 Android 主线程上的同步 `bail` 一律被拒（`EventsRuntime.kt:321–327`，现在本地有监听器时可以）；Go 会话结束后的 `emit` 不再走本地（`events.go:446–449`），清理阶段的 `emit` 报会话已结束。

### 7.6 核实后与清单不同的地方

1. **Python 删旧名会让已发布的插件导入失败**：插件与运行时共用宿主环境里同一个 `rutis` 包（SDK 入口设计 §五）。现行规则 `release.md:36`、`plugin-api.md:478`、`python-plugin.md:389` 写"`PLUGIN_API` 不变时不删名字、`PLUGIN_API` 增加时运行时继续加载旧 API"。处理：按"不做兼容"改写这三处规则为"删名字的版本在迁移指南写明，插件随宿主一起升级"；不升 `PLUGIN_API`（插件 API 本身没变，变的是导入位置）。《迁移到 0.19》写明。
2. **Go 不能直接"把定义挪进 `advanced`"**：根包不能 import `advanced`（成环）；`Service`、`ServeArgs`、`ServeConn`、`Manifest`、`Project`、`Rename`、`Schemer`、`Version`、`PluginAPI`、`Implementation` 的实现依赖根包的未导出状态。处理：把这些实现移进 `go/rutis/internal/runtime`，根包（只留插件作者用的 19 个顶层名字）与 `advanced` 都调它；`advanced` 里的名字是 `internal/runtime` 的别名。
3. **Kotlin 删 generated 方向的旧别名后，0.17.2 的 `rutis-ksp` 生成的代码不能再编译**，与 SDK 入口设计 :63、迁移 :102 的说法相反。处理：迁移写明 `rutis-ksp` 与 SDK 同版本升级。
4. **"占住事件循环 5 秒"的提示不是只为 dev**：三种脚本运行时默认都开（Node/Bun `output.mjs:83–159`、Python `_output.py:100–211`），只有 `RUTIS_DEBUG=1`（`dev --debug`）才关，`run` 下也打印；它来自 0.18 诊断设计 §6.4，有保证 `host/blocking-hint`。清单的前提是"只为 dev"，不成立。处理：不删。Go、Swift、Kotlin 没有这个提示。
5. **运行时一侧的输出前缀保留**（4.4 第 8 条）。Go、Swift、Kotlin 没有运行时一侧的前缀。
6. **`refuseRemoved`、插件 API 1/2、`rows.failed` 在 Go、Swift、Kotlin 里没有**，7.2 表里按实况写"无"。
7. **配置校验"只留 loader"有缺口**（插件值不经 loader），按 3.4 第 2 条放内核；`config.mjs` 不能整份删（上表"留"一列）。
8. **事件本地分发在 Node 上删不干净**：Cordis 节点 `@arcships/rutis-runtime/bridge`（`src/bridge/*.mjs`，约 810 行）自己当宿主，"its bus has no host above it"（`events.mjs:19–22`），必须本地分发；它是 Cordis 兼容的一部分，#811 只改它的写法。处理：本地分发只留在 Cordis 节点里（从 `events.mjs` 拆出它自己用的一份），叶子路径全删；三份副本一致的检查改为两份（`node/rutis`、Bun）。
9. **测试工具与快速路径的先后**：测试工具完全依赖本地分发，所以每种语言的快速路径删除与测试工具改写放在同一个 PR（7.5 开头）。
10. **SDK 与 bridge 的协商删除有先后**：bridge 先停止检查对端的 `events-v2`、`objects`、`sync-wait`，SDK 再停止检查宿主的与停止自己声明，最后 bridge 停止声明。反过来做，带方法的对象会被拒、等待环检测会失效。§十三 CL11 按这个顺序拆。

11. **Kotlin、Swift 的"旧宿主的 emit 回复是 future"分支在当前宿主上仍会走到**：bridge 的 `events.dispatch` 在同一行已有 emit 在跑时把这次 emit 排队，回一个 future（`crates/rutis-bridge/src/events/hub.rs:809–814` 的 `run_emit` 返回 false 一支）；Go 的同一处 `peer.Settle`（`events.go:541–545`）注释写的正是这个现行行为（#677）。这个分支的注释（c2c1b7e8，2026-10-21）写成"older host"，与事实不符。删掉它，同一行连续 emit 时后一次的完成与错误会丢失，Kotlin、Swift 的测试会失败。处理：按 §13.1 第 2 条不绕过——分支不删，只把注释改为"宿主排队时回一个 future"，在 Kotlin、Swift 的 CL26 里做；这一条报回用户。

## 八、生成类型：做法不变，搬出平台

### 8.1 现在

`build::Exports`（`crates/rutis-bridge/src/build/exports.rs` 3612 行，其中约 830 行测试；`build/crates.rs` 895 行；`build/mod.rs` 8 行）用 syn 读源码：`crates.rs` 跑 `cargo metadata` 与 `rustc --print cfg`，从入口读整棵模块树、解析 `use` 与再导出、按 `#[cfg(feature)]` 判断，不展开宏；`exports.rs` 找 `#[rutis::service]` 的 `impl`，把方法签名、被引用的结构体与枚举的字段、doc 注释、serde 属性（rename、rename_all、tag、content、untagged、flatten、default、skip、transparent）、别名、泛型、错误码转成 TypeScript 与 Python。依赖 `exports = ["dep:proc-macro2", "dep:quote", "dep:syn"]`（bridge `Cargo.toml:33–35,77`）。

调用方：`rutis-host types <crate 目录>`（`crates/rutis-host/src/main.rs:150–184`，只读源码、不编译）；`examples/rust-services/build.rs`；`examples/marked-services` 的快照测试（`tests/rows.rs:175–260,340` 比对 `rows/services.d.ts`、`services.py`，并跑 `tsc --strict`、`pyright --strict`）。`cordis` feature 不调用 Exports，它依赖 `exports` 只为拿到 syn 与 quote（给 4.1 要删的 `rust.rs`，与 `cordis/build.rs:291` 一处标识符检查）。

`src/exports.rs`（228 行）与 `src/marked.rs`（1096 行）是运行时代码（错误转换、按名字对外），不属于生成，留。

### 8.2 决定

生成的做法不变（用户 2026-10-28 定）：保留 `Exports` 读源码生成 TypeScript 与 Python 的实现。作者不用为数据类型多写任何东西，只有手写 serde 实现的类型用 `Exports::declared` / `declared_python` 写出它的类型。只把它从 rutis-bridge 搬出平台：它是给开发者的工具（§二 的 B 类），不是平台的职责。

搬到一个独立的工具 crate `rutis-types`（`crates/rutis-types`，随发布列车发布），不搬进 rutis-host。理由（按依赖关系）：

- 生成器只依赖 syn、quote、proc-macro2 与标准库（核实：`build/exports.rs`、`build/crates.rs` 的 `use` 里没有 rutis 或 bridge 的任何东西），独立成 crate 不需要拆任何依赖。
- 它的使用方里有 build.rs（`examples/rust-services/build.rs`，以及照 rust-host 指南写的嵌入方）。搬进 rutis-host，build.rs 就要把 rutis-host 整棵依赖（loader、bridge、tokio、WebSocket、TLS）拉进 build-dependencies；rutis-host 的库还声明不做 semver 承诺（`lib.rs:4–5`）。独立 crate 只带 syn。
- rutis-host 依赖 `rutis-types` 实现 `rutis-host types`。

写法不变：`rutis_types::Exports::new().typescript(..).python(..).declared(..).declared_python(..)`，`.generate()`、`.declarations()`、`.write(dir)` 都照旧；`rutis-host types <crate 目录>`、`--python` 照旧；build.rs 只把依赖从 `rutis-bridge = { features = ["exports"] }` 改成 `rutis-types`，`use` 路径从 `rutis_bridge::build::Exports` 改成 `rutis_types::Exports`。生成的内容逐字不变。

### 8.3 为什么不改做法

清单原写"由 `#[rutis::service]` 宏在编译时把服务描述带进二进制"。核实后三种改法都不用：

- **宏带描述**：属性宏只拿到被标注的 `impl` 块——方法名、`async`、参数名、参数与返回值**按书写的路径**、方法的 doc 注释；被引用的结构体与枚举的字段、字段 doc、serde 属性、枚举变体（错误码从这里来）、类型别名指向什么、路径属于哪个 crate，宏都看不到。现有例子正是跨 crate 的：`examples/rust-services` 的数据类型在依赖 crate `rust-services-calendar` 里，`build/crates.rs` 就是为这个存在的。另外 build.rs 在本 crate 编译之前运行，宏带进二进制的描述在 build.rs 里拿不到。
- **serde-reflection 一类（运行时追踪 serde）**：已核实不可行。rutis 的值类型、`#[serde(tag)]`、`untagged`、`flatten` 都走 `deserialize_any`，追踪追不出结构；不同模块里的同名类型会被当成一个，给出错误的结果；返回值只有 `Serialize` 时也走不通。现在的生成器这些都支持，有测试。
- **schemars（`JsonSchema` 派生）**：要作者给每个参数、返回值、错误里出现的类型都加派生，依赖 crate 里的类型也要加；这违背"作者不用多写"。

### 8.4 影响

- bridge：`build/` 三个文件（4515 行）整体移到 `crates/rutis-types/src/`；`pub mod build`（`lib.rs:32`）、`exports` feature 删；`cordis/build.rs:291` 的 `syn::parse_str::<syn::Ident>` 改为手写检查（字母或下划线开头，后接字母、数字、下划线，排除关键字），`cordis` 不再依赖 `exports`（syn 的另一个用户 `cordis/build/rust.rs` 由 CL10 删）。之后 bridge 不再依赖 syn、quote、proc-macro2。
- `src/exports.rs`（228 行）与 `src/marked.rs`（1096 行）是运行时代码（错误转换、按名字对外，loader 也用），不是生成，留在 bridge。
- rutis-host：`Cargo.toml:32` 的 `rutis-bridge` `exports` feature 改为依赖 `rutis-types`；`main.rs:150–184` 改 `use`。
- `examples/rust-services`：build-dependency 改 `rutis-types`；`examples/marked-services`：dev-dependency 改 `rutis-types`，快照测试不变。
- 生成器的单元测试（`exports.rs` 内约 830 行）随文件搬，测试夹具字符串里的 `crate::types` 等不变。
- 发布列车：`scripts/train.mjs` 在 `rutis-host` 之前加 `rutis-types`；`docs/release.md:9` 的表加一行。
- 与已合并 PR 的关系：#798（Python 类型）、#810（文档与 H4）不受影响，只是代码所在的 crate 变了；CL5 改的 `CordisError` 识别在搬之前做。
- 保证 `bridge/marked-services` 的验证列里 `exports.rs` 单元测试的路径改为新 crate。

## 九、改前改后

现在的数字在 `8071a794` 上用 `wc -l` 实测（含文件内的单元测试）；"公开项"按 #811 §九 的口径：`grep -cE '^\s*pub (fn|struct|enum|trait|type|const|use|mod) '`。改后的数字都是**估计**；"#811 后"一列是 #811 §九 的估计，本稿的减少在它之后另算，不重复。

### 9.1 Rust crate

| crate | 现在行数（src） | 现在公开项 | #811 后（估计） | 本稿后（估计） | 本稿的主要变化 |
| --- | ---: | ---: | ---: | ---: | --- |
| `rutis` | 16180 | 408 | 16180 | 约 15500 | 删拦截器 −390、bus −170、run.rs −150、ctx −90、internal −80 等约 −1230；加 `config::check` +约 560 |
| `rutis-macros` | 1284 | 4 | 1284 | 约 1280 | 删 `actor` 参数 |
| `rutis-bridge` | 41378 | 750 | 约 36100 | 约 29000 | 反方向挂载 −1040、`runtime/spawn.rs` −767、`process.rs` −213、`output.rs` −259、`runtime/mod.rs` −71、协商 −60、#683 −约 400；`build/` −4515（搬到 `rutis-types`） |
| `rutis-loader` | 15404 | 297 | 约 13610 | 约 11850 | `config.rs` 搬进内核 −594、重启策略搬走 −250、npm 解析 −215、别名与死方法 −约 400、overlay 层 −约 60 |
| `rutis-host` | 11128 | 176 | 约 10760 | 约 12000 | 接收语言启动 +200、输出 +260、重启 +330（合并去重 −100）、`rutis-host test` +约 300；`go add` −35 |
| `rutis-types`（新） | — | — | — | 4515 | 从 bridge 的 `build/` 原样搬来 |
| `rutis-dev` | 588 | 13 | 588 | 0 | 删 |
| `rutis-migrate` | 3354 | 5 | 3354 | 0 | 删 |
| dylib 族（`rutis-dylib`、`-meta`、`-launcher`、`rutis-sdk`、`rutis-xtask`） | 4087 | 57 | 4087 | 4087 | 不动 |

逐文件的现在数字（本稿涉及的）：

| 文件 | 行数 | 公开项 | 改后（估计） |
| --- | ---: | ---: | ---: |
| `rutis/src/intercept.rs` | 492 | 3 | 约 100（改名 `writer.rs`） |
| `rutis/src/bus.rs` | 1209 | 15 | 约 1040 |
| `rutis/src/bus/run.rs` | 719 | 7 | 约 570 |
| `rutis/src/events/remote.rs` | 317 | 31 | 约 255 |
| `rutis/src/events/api.rs` | 702 | 36 | 约 645 |
| `rutis/src/effect.rs` | 318 | 5 | 约 275 |
| `rutis/src/fiber.rs` | 2140 | 22 | 约 2120 |
| `rutis/src/key.rs` | 531 | 27 | 约 495 |
| `rutis/src/internal.rs` | 202 | 21 | 约 120 |
| `rutis/src/config/mod.rs` + `decode.rs` | 1136 | 18 | 约 1700（收下 `check`） |
| `rutis-bridge/src/build/`（三个文件） | 4515 | 73 | 0（搬到 `crates/rutis-types`） |
| `rutis-bridge/src/cordis/build/rust.rs` | 855 | 1 | 0 |
| `rutis-bridge/src/cordis/server.rs` | 106 | 2 | 0 |
| `rutis-bridge/src/runtime/spawn.rs` | 767 | 0 | 0 |
| `rutis-bridge/src/runtime/unix.rs` | 75 | 2 | 0 |
| `rutis-bridge/src/runtime/process.rs` | 1933 | 39 | 约 1720 |
| `rutis-bridge/src/transport/local/output.rs` | 259 | 4 | 0 |
| `rutis-bridge/src/session/rpc/limits.rs` | 188 | 0 | 0 |
| `rutis-bridge/src/events/errors.rs` | 340 | 2 | 340（`report_line` 留） |
| `rutis-loader/src/config.rs` | 594 | 6 | 0（搬进内核） |
| `rutis-loader/src/config_step.rs` | 255 | 1 | 约 195 |
| `rutis-loader/src/loader/api.rs` | 650 | 11 | 约 588 |
| `rutis-loader/src/runtime/go.rs`、`swift.rs` | 85、83 | 12、12 | 约 45、45 |
| `rutis-loader/src/runtime/binary.rs` | 2667 | 35 | 约 2400 |
| `rutis-loader/src/backoff.rs` | 68 | 7 | 0（搬 rutis-host） |

- crate 个数 17 → 16（删 `rutis-dev`、`rutis-migrate`，加 `rutis-types`）；workspace 成员 29 → 28；crates.io 发布列表 10 → 10（去 `rutis-migrate`，加 `rutis-types`）。
- 内核 `CtxInternal` 的方法 18 → 9（删 `get_key`、`require_key`、`provide_key`、`provide_checked`、`refresh`、`intercept_require_as`、`intercept_set_as`、`try_filter`、`try_filter_named`）；`CtxHost` 的方法 30 → 27。内核根导出 14 个名字不变。
- rutis-loader 根导出 20 项（12 个名字、8 个公开模块）→ 约 18：去掉 `backoff`、`internal`（`peer` 由 #811 去掉），加 `patch`。
- bridge 的 Cargo feature 11 个（`default`、`node`、`python`、`go`、`bun`、`kotlin`、`swift`、`websocket`、`exports`、`cordis`、`testing`）→ 4 个（`websocket`、`cordis`、`testing`、`default` 为空）；依赖少 syn、quote、proc-macro2。
- 内核的 `unsafe` 4 处 → 1 处（只剩 `service.rs:1482–1488` 的 link_section）。

### 9.2 各语言 SDK（源文件，不含测试与 node_modules）

| 包 | 现在行数 | 改后（估计） | 主要变化 |
| --- | ---: | ---: | --- |
| `node/rutis/src` | 2375 | 约 800 | `config.mjs` 476 → 约 20；`events.mjs` 558 → 0；`testing.mjs` 720 → 约 350；`values.mjs` 261 → 约 80；`index.d.ts` 82 → 47 |
| `node/rutis-runtime/src` | 7348 | 约 6600 | `config.mjs` −476；`events.mjs` 558 → 约 480（Cordis 节点留本地分发）；`client.mjs`、`io-worker.mjs` −约 50；旧宿主分支 −约 80 |
| `bun/rutis-bun/src` | 4542 | 约 3800 | `config.mjs` −476；`events.mjs` 558 → 约 330；旧宿主分支 −约 50 |
| `python/rutis/rutis` | 7725 | 约 6300 | `_eager` −161；`config` −626；`events` 553 → 约 400；`testing` 826 → 约 380；`__init__` 59 → 25 |
| `go/rutis` | 12991 | 约 10000 | `internal/config` 1022 → 约 120；`rutistest` 1648 → 约 700；快速路径 −约 250；旧宿主 −约 80 |
| `swift` | 11488 | 约 10200 | `ConfigSchema` −670；`Moved` −74；快速路径 −约 250；`RutisTest` 673 → 约 300 |
| `kotlin`（主 artifact） | 9661 | 约 8300 | `ConfigSchema` −566；`Moved` 等 −约 75；快速路径 −约 200；`RutisTest` 691 移出（新 artifact 约 300） |

公开面：

| 入口 | 现在 | 改后 |
| --- | ---: | ---: |
| Node `index.mjs` 运行时导出 | 10 | 1 |
| Node `index.d.ts` 的 `export` | 20 | 4 |
| Python `__all__` | 12 | 5 |
| Go 根包顶层导出 | 52 | 19 |
| Swift `public-api.txt`（含 deprecated） | 213 | 188 |
| Kotlin 旧名（typealias 与旧常量） | 24 | 0 |

## 十、对已合并（与未合并）0.19 PR 的影响

| PR | 内容 | 本稿删或改的 | 不受影响的 |
| --- | --- | --- | --- |
| #767 L1 | `call_limit`、`kill_on_timeout`、`greeting`、`timeout` 码 | 删 `call_limit`、`kill_on_timeout`、`limits.rs`、`code::TIMEOUT`、`report.rs` 的 timeout 分支、bridge `tests/plugin_limits.rs` 的 6 个限时测试 | `greeting` 与它的两个测试（载体随 #811 S4 换到 `rutis-host/spawn`） |
| #793 L2 | `allowed_services`；`modules` 每插件一进程 | 删 `allowed_services`、`may_use`、loader 的检查、`report.rs` 的 `unavailable` 分支、3 个 loader 测试 | `RuntimeResolver::modules`、`each_plugin_runs_in_its_own_process`、`a_runtime_waiting_for_a_debugger_stays_starting` |
| #796 L3 | 文档与保证、`request_sync` 合并截止时间 | `request_sync` 的 `deadline` 参数退回；open-host 第 3 节删两小节、loader doctest `OpenHostGuide` 改；rust-host 参考表删 3 行；`host/plugin-limits` 收窄 | open-host 第 3 节的"每个插件一个进程""问候时限" |
| #762 E1 TS/Bun | `./advanced` 入口与转发 | 删 `index.mjs:49–51` 的转发与 `index.d.ts` 的 16 个 `@deprecated` | `./advanced` 与公开面测试 |
| #763 E2 Python | `rutis.advanced` 与 `__getattr__` | 删 `_MOVED`、`__getattr__`、`__all__` 旧名、deprecated 一节与测弃用的用例 | `rutis.advanced` |
| #769 E3 Go | `advanced` 别名、根包 33 处 `Deprecated:` | 删 33 处；实现移进 `internal/runtime`，`advanced` 改为指向它（7.6 第 2 条） | 子包划分 |
| #775 E4 Swift | `Moved.swift` | 删 | `Rutis.Advanced`、`_Generated`、快照脚本 |
| #777 E5 Kotlin | `Moved.kt` | 删；版本常量挪进 `Versions.kt`；"旧 `rutis-ksp` 生成的代码照样编译"不再成立 | advanced、generated 包 |
| #778 E6 | 《迁移到 0.19》一节、速查表、ci.md、release.md | 迁移一节改为"直接移动，无旧名"；`release.md:36` 的规则改写（7.6 第 1 条） | 速查表的新位置写法 |
| #765 K1 / #803 K1d | 契约描述（已删） | 无 | — |
| #798 K2 | Python 类型 | 代码随 `build/` 搬到 `rutis-types`（§八） | 生成内容、`Exports` 的写法、`--python` |
| #810 K5 / H4 | 文档、测试工具拒绝宿主已有的服务名 | H4 改由真实内核报，两处手写检查删；rust-host.md 里 build.rs 的依赖改为 `rutis-types` | python-plugin.md 的用法 |
| #770 T4 | 测试工具 `t.on` | 实现改为真实内核上的测试插件 `ctx.on`；两个测试改为跑在真实内核上 | `t.on(name, listener, options)` 写法与指南 |
| #772 C1、#794 C2 | `rutis-host call`、`dev` 的服务行 | `call.rs:107`、`dev.rs:611` 改用公开 API（6.3） | 功能 |
| #801、#804 | 指南、去掉 `--allow-call` | 无 | — |
| #805 P2（未合并） | 安装与 `add`/`remove`/`upgrade` | 合并后删 `go add`（6.4 第 1 条）；它用的 `rutis_loader::internal::resolve_entry`（diff 第 1611 行）与 `LocalRuntime::node/bun/python`（第 1616、1621、1628 行）由本稿与 #811 删除，见 §十三 的先后；Node 裸包名、`go:`/`swift:` 行名与本稿一致 | `add`/`remove`/`upgrade`/`installed` 的设计 |
| #768 R1、#791 R2 | 远程 | #811 §十 处理 | — |

要改的设计稿（头部加一行"部分被 [0.19 平台清理](design-platform-cleanup-2026-10-28.md) 改写"，并改对应节）：

| 设计稿 | 节 | 改什么 |
| --- | --- | --- |
| [SDK 入口收敛](design-sdk-entry-convergence-2026-10-27.md) | §二 决策表、§五 整节、§六、§七、§八、§九 第 2、3、5 步 | 旧位置不再可用；删"为什么不在 0.19 直接删"与 Python 不能删的理由；提示文字表改为编译错误或导入错误；完成条件里的 `DeprecationWarning`、SA1019、"0.17.2 的 `rutis-ksp` 能编译"删 |
| [宿主管住插件](design-host-plugin-limits-2026-10-27.md) | 标题、头部、§二、§三、§四 H2/H3/H5、§4.2、§4.3、§五 第 1–4 条、§六、§七 第 2、3、5 条、§八 第 1–3 步 | 只剩 §4.1（H1 每插件一进程）与 §4.4（greeting）；头部写明调用时限与服务名单 0.19 内加了又删、未发布 |
| [宿主契约类型](design-host-contract-types-2026-10-27.md) | §三 H1、§4.1、§五、§七 | `Exports` 所在 crate 改为 `rutis-types`（写法不变）；H4 改为真实内核拒绝；"协议、内核、loader、各语言运行时不变"改写 |
| [配置 Schema](design-config-schema-2026-10-27.md) | §2.1、§2.2、§2.3、§六 开头、§6.1、§七 第 3 条、§八、§十一 第 2–5 步 | 校验在内核一份，SDK 只解码；Cordis 行例外保留；0.18 前 Go 的 `required` 特例删；用例表只测内核与 `check` |
| [宿主自己写插件](design-host-writes-plugins-2026-10-27.md) | L3（第 41、148 行） | "宿主要限制，用 #683 的 `allowed_services`"改为"限制哪些插件能用 `loader` 是宿主的业务，由宿主在自己的行上处理（例如 `isolate`）" |
| [0.18 诊断](design-diagnostics-2026-10-26.md) | §6.1 末句 | 删"宿主想限制第三方插件的资源，属于 0.19 的 #683" |
| [插件安装](design-plugin-install-2026-10-27.md) | 兼容一节、I1 | `go add` 删（随 CL25） |
| [事件](design-events-2026-10-18.md)、[事件协议](design-events-protocol-2026-10-18.md) | 事件协议 §5.1、§5.2、§十五 第 5 条；`waterfall-sync` 出现的各节（:154–192,285,411,434,665–668,883） | 删本地快速路径与 `events.elsewhere`；方式表删 `waterfall-sync` |
| [内核设计 v5](design-rust-port.md) | 头部 | 3.3 末段的说明 |

## 十一、兼容与《迁移到 0.19》

不做兼容。旧写法在 0.19 直接删，不标弃用，不做专门的转换或提示：删掉的 Rust API 是编译错误，删掉的 SDK 名字是导入错误或编译错误，删掉的行名写法按 0.18 的规则报"不认识"。

《迁移到 0.19》加"平台清理"一节（与 #811 的"远程连接"一节并列），只写新写法与对照：

1. **Rust API**：

   | 0.18 | 0.19 |
   | --- | --- |
   | `ctx.plugin_from(\|cfg\| …)` | 实现 `rutis::host::PluginFactory`，`ctx.plugin_with(factory, config)` |
   | `waterfall_sync` | 改用 `waterfall`（异步）或同步 `bail` |
   | `take_cleanup_errors()` | 清理的错误在卸载结果里（`dispose` 返回的错误） |
   | `FiberView::effects()`、`EffectMeta`、`EffectPhase` | 无替代；诊断用 `diagnostics()` |
   | `EventBus::observe_dispatch` | 无替代；要观察就注册一个普通监听器 |
   | `internal::intercept_*`、`internal::provide_checked`、`refresh` | 无替代 |
   | `internal::{get_key, require_key, provide_key}` | `Key::of/named/dynamic(..).instance(..)` 加 `get_at`/`require_at`/`provide_at` |
   | `internal::config::{closest, decode, …}` | `rutis::config::closest`；校验 `rutis::config::check` |
   | `rutis_loader::config::check(schema, config, options)` | `rutis::config::check(schema, config)` |
   | feature `anyhow`、`Error::from_anyhow` | `.map_err(\|e\| Error::msg(format!("{e:#}")))` |
   | `ServiceCatalog::share_by_name()`、`share_registered_only()` | 删掉这一行（默认按名字共享） |
   | `Loader::waiting_for`、`locate`、`schema_of` | `waits`/`waits_of`、`row`、`resolve(name).await?.schema` |
   | `GoBinaries`、`GoResolver`、`GoRuntimes` 等 16 个别名 | `binary::*` 加 `Go`、`Swift` |
   | `LocalRuntime::call_limit`、`kill_on_timeout`、`allowed_services`，错误码 `timeout` | 调用方自己限时；超时后 `processes().kill()` 加 `FiberView::restart()`；能用哪些服务由宿主自己决定 |
   | `rutis_bridge::build::Exports`，build-dependency `rutis-bridge = { features = ["exports"] }` | `rutis_types::Exports`，build-dependency `rutis-types`；写法不变 |
   | `Loader::set_overlay`、`overlays()` | 删；要覆盖一行就在自己的层里写 |
   | `rutis_bridge::cordis::build::rutis_plugin`、`cordis::server` | 删（Cordis 宿主挂 Rust 插件的方向不再支持） |
   | `rutis_bridge::Launcher::node` 等、bridge 的语言 feature | rutis-host 的库（`rutis-host/spawn`）；测试用 `rutis-test-support` |
   | `rutis_loader::internal::*` | `rutis_loader::patch::apply_patches`、公开的 `is_runtime_name`、`read_binary_manifest`、`Go::PLUGIN_API` |
   | 线上错误名 `CordisError`、生成类型 `CordisErrorThrown` | `Error`、`ErrorThrown` |

2. **rutis.json 与行名**：Go/Swift 的 `go-<文件>:<插件>`、`swift-<文件>:<插件>` 改为 `go:<插件>`、`swift:<插件>`；两个二进制提供同名插件时改名其中一个。Node 行名不变。
3. **Go 插件**：用 0.18 及以后的 SDK 重新构建（0.18 前构建的二进制的 `required` 例外删除）。
4. **SDK 的旧位置**：#692 那一节的对照表保留"新位置"一列，删"看到什么提示"一列与"不改也能跑"一段；写明 Python 插件要与宿主一起升级，Kotlin 的 `rutis-ksp` 与 SDK 同版本。
5. **Python**：最低 3.12。
6. **测试工具**：写法不变；运行时需要 `rutis-host`（`PATH` 上或 `RUTIS_HOST`），Node 项目把 `@arcships/rutis-runtime`、`@arcships/rutis-host` 加为开发依赖；Kotlin 测试工具的依赖坐标改为 `rutis-testing`；设备上的测试由应用自己装插件，不用 `rutis-host test`。
7. **事件**：Kotlin 在 Android 主线程上的同步 `bail` 一律被拒；Go 会话结束后的 `emit` 报会话已结束。
8. **`rutis-host go add`**：改用 `rutis-host add <模块路径>`（它会同时写一行）。
9. **工具**：`rutis-dev` 删；生成类型的 `Exports` 搬到新 crate `rutis-types`，build.rs 改依赖它，写法不变；`rutis-migrate` 不再随 0.19 发布（0.17 的迁移仍可 `cargo install rutis-migrate --version 0.18.0`）。
10. **一起升级**：同 #811 §十一 第 5 条——宿主与各语言运行时、SDK 一起升到 0.19；协议版本 4 与 0.18 不相遇。

## 十二、要同步改的文档

| 文档（两种语言） | 改什么 |
| --- | --- |
| `docs/guarantees.md` | 删：`events/v2-required`、`events/presence`、`host/go-add`、`loader/overlay`。改：`core/D4`、`core/D16`、`core/D17`、`events/waterfall`（去掉 `waterfall_sync` 与它的测试）；`core/D11`（去掉 `take_cleanup_errors`）；`core/D14`（证据改为 3.4 第 4 条的新测试）；`core/D18`、`events/reserved`、`events/serial-bail`（换测试）；`events/old-peer-refused`、`bridge/cross-version-meetings`（按协议版本 4 拒绝，与 #811 一起）；`events/hub-filter`（过滤一律在宿主）；`events/one-bus`、`sdk/testing-matches-runtime`（测试工具就是真实内核，差异一节删）；`sdk/undeclared-use`（由内核报）；`sdk/config-schema`（只有内核与 `check`）；`sdk/leaf-events`（删 `refuseRemoved` 一句）；`host/output-prefix`、`host/output-lines-whole`（测试路径）；`loader/go-runtimes`、`loader/swift-runtimes`（只留语言前缀写法，重启策略在 rutis-host）；`host/runtime-restart`（测试路径）；`loader/schema`、`bridge/marked-services`（`waiting_for`、`schema_of`、生成类型的写法）；`host/plugin-limits`（收窄）；`host/check`（去掉"0.12 SDK"）；`boundary/5.6`（夹具与测试改写后测试名不变）；`tests/events.rs:602` 的悬空标签 `events/waterfall-sync` 删。每行的测试名随测试改，§15 计数重算 |
| `docs/guide/rust-host.md` | `plugin_from`（:39,89）、`take_cleanup_errors`（:146）、`CordisError`（:145,718）、拦截（:238）、`share_registered_only`（:434）、`LocalRuntime::*`（:426–560，改为 rutis-host 库）、限制选项表（:512–523）、`waterfall_sync`（:641,654）、`waiting_for`（:243,487）、Go 别名（:539–547 doctest）、`Exports` 的依赖与路径改为 `rutis-types`（:246,280,667–675,731–748，写法不变）、`report_line`（:658，留）、`RUTIS_TRACE`（:496,574，留） |
| `docs/guide/rust-plugin.md` | `anyhow`（:495）、`plugin_from`（:615，doctest） |
| `docs/guide/plugin-api.md` | `waterfall_sync`（:68,72,86,212）、events-v2 报错（:227,230）、:253 的 open-host 一句、测试工具差异（:421–432）、Python 弃用规则（:478）、Python 3.10（:265,452） |
| `docs/guide/open-host.md` | 第 3 节删"调用时限""能用的服务"；`share_registered_only`（:129）；示例代码按 #811 改 |
| `docs/guide/rutis-host.md` | `go add`（:78,96）、events-v2（:179）、0.18 前 Go 的 `required`（:238）、Go/Swift 行名（:513,596）、Python 3.10（:7）、`rutis-host types`、`rutis-host test` |
| `docs/guide/go-plugin.md` | `go add`（:411）、限定行名（:414）、`GoResolver`（:414）、`rutistest` 的说明（:233–296） |
| `docs/guide/typescript-plugin.md`、`bun-plugin.md`、`python-plugin.md`、`swift-plugin.md`、`kotlin-app.md`、`swift-app.md` | 测试工具一节（真实宿主、开发依赖、设备上的做法）；Python 3.12（`python-plugin.md:3`）；Kotlin 测试 artifact 坐标；`python-plugin.md:389` 的弃用规则 |
| `docs/guide/README.md` | Python 3.10（:38） |
| `docs/migration-0.19.md` | §十一 |
| `docs/core-features.md` | :7,23,45,46,48,55,59,63,81 |
| `docs/development-handbook.md`、`development-guide.md` | `plugin_from`、`provide_checked`、`waterfall_sync`、`take_cleanup_errors` |
| `docs/quality-status.md` | `rutis-dev`、Python 3.10 |
| `docs/design-philosophy.md` | §六 Python 3.10+ 改 3.12+ |
| `docs/release.md` | 发布列表删 `rutis-migrate`、加 `rutis-types`（:9）；`PLUGIN_API` 与弃用规则（:36）；Go/Kotlin 版本常量的路径（:18） |
| `docs/ci.md` | `rutis-dev` 路径过滤（:80）；公开面检查的 deprecated 一节（:99,107）；bridge/loader feature 组合（:103）改为 `RUTIS_TEST_RUNTIMES`；`requires-python`（:276,288,294,342,344）；Swift/Kotlin 测试工具（:108,109）；`anyhow` 任务 |
| `.agents/skills/rutis-new-runtime/SKILL.md` | :72（测试工具）、:54,117（能力）、:74–77（新语言不再在 bridge 加 `Launcher`/`LocalRuntime`/feature，改为 rutis-host）、:120（本地转发） |
| `.agents/skills/rutis-new-runtime/references/runtime-contract.md` | 能力表 :34–42（删 `sync-wait`、`signals`、`objects`、`events-v2`；`plugins` 留）、:65（`events`/`emits`）、:113–123（`events.elsewhere`、快速路径、`waterfall-sync`）、:147（旧宿主）、:174（测试工具期望文件） |
| `.agents/skills/*/references/plugin-api.md`（两份） | `waterfall_sync`（:58,62,76,200）、events-v2（:215,218）、测试工具（:299–309）、Python 3.10（:329） |
| `.agents/skills/rutis-remote-plugin/SKILL.md` | `waiting_for`（:48） |
| `specs/config-schema/README.md` | :5、:16 |
| `specs/cross-runtime-sync/README.md` | `sync-wait` 能力（:15） |
| templates（`crates/rutis-host/templates/`） | `python/pyproject.toml:6`（3.12）；`go/README.md:11`（`add`）；`node`、`bun` 的 `package.json` 加开发依赖；各模板测试按新测试工具重跑 |
| `README.md`、`CONTRIBUTING.md`、`python/rutis/README.md`、`go/rutis/README.md`、`swift/README.md`、`kotlin/README.md`、`bun/rutis-bun/README.md`、`crates/rutis/README.md`、`crates/rutis-loader/README.md`、`crates/rutis-bridge/README.md`、`examples/native-mount/README.md`、`examples/marked-services/README.md` | Python 3.12；测试工具段落；`LocalRuntime` 语言构造；反方向挂载一行；`EventPattern`/`waterfall_sync`；`Exports` 所在的 crate |

历史文档（旧设计稿、`review-*`、`research-*`、0.18 及以前的迁移指南与发布说明）不改。

## 十三、实施

### 13.1 规则

1. **每个 PR 自闭环**：删一样东西，就在同一个 PR 里把它在所有 crate、各语言 SDK、测试、文档、示例、templates 里的用处改完。每个 PR 下面列出它的闭环清单。
2. **不为通过测试或 CI 加兼容**：不加转发、兜底分支、旧名别名、feature 开关、"暂时保留"。因删掉的行为而失败的测试，在同一个 PR 里改写或删除；改不了时停下来报告，不绕过。
3. **闭环太大时按依赖拆**：先加新的，所有使用方切过去，再删旧的；每一步合并后 main 可用、CI 通过。必须同时改的写明"同一个 PR"。
4. **与 #811 对齐**：碰同一处代码的放进同一个闭环，或写明先后（下表"依赖"一列）。#811 的 PR：S1 Channel、S2 session、R1–R6 各语言本机 endpoint、A1/A2 应用自己装插件、S2b 删 compat、S3 loader 统一解析器、S4 rutis-host 三个连接插件、S5 删除、S6 文档。
5. SDK 的改动每种语言一个 PR（Node 与 Bun 共用 `@arcships/rutis` 与 `events.mjs` 副本，算一个闭环）。
6. 每个 PR 在 #771 加一行再开（#771 的规则 2）。

### 13.2 PR

编号 CL（cleanup）。"闭环清单"列出要改的文件或目录、要删或改写的测试。

**内核**

| PR | 内容 | 闭环清单 | 完成条件 | 依赖 |
| --- | --- | --- | --- | --- |
| CL1 内核：没有生产调用方的接口 | 删 `EventPattern`、`observe_dispatch`/`DispatchAttempt`/`DispatchObserver`、`take_cleanup_errors`（三处）、`FiberView::effects`/`EffectMeta`/`EffectPhase`（标签移到 `EffectRecord`）、`anyhow` feature 与 `from_anyhow`、`try_filter`/`try_filter_named`；bridge 的 `Connection::independent_future` | `crates/rutis/src/{key,bus,host,ctx,fiber,effect,error,internal,events/api}.rs`、`Cargo.toml`、`public-api.txt`；测试：删 `dispatch_observation.rs`、`effect_tree.rs`，改 `cleanup_errors.rs`、`author_api.rs`、`typed_dispatch.rs`、`bus/transient_tests.rs`、bridge `tests/event_hub.rs:984`（探针改普通监听器）、bridge `tests/rpc_callbacks.rs:445,499`（删）、loader `events_conformance/rust_participant.rs:354`；`ci.yml:493–495`；文档 `core-features`、`development-handbook`、`rust-host`、`rust-plugin`（anyhow）、guarantees `core/D11`、`core/D18`、`events/serial-bail` | `git grep` 这些名字在 `crates/ docs/guide .agents` 下无结果；Stuck 提示的测试照旧通过 | — |
| CL2 内核：服务拦截器 | 删拦截器全部与 `require_provided_as` 的 `actor`；`intercept.rs` → `writer.rs` | 内核 `intercept.rs`、`internal.rs`、`ctx.rs`、`error.rs`、`service.rs`；`rutis-macros/src/lib.rs:460–464`；bridge `marked.rs:72–78,1055`；测试：`service_intercepts.rs` 拆出 `service_writer.rs`，删拦截部分；bridge `marked_services.rs:739`、`projection_lifecycle.rs` 两个测试按 3.4 第 5 条；guarantees `bridge/marked-services`；`rust-host.md:238`、`core-features.md:63` | 宏生成代码编译通过；`ServiceWriter` 的用例全部保留并通过 | CL1 |
| CL3 内核：`check()` 门控 | 删 `provide_checked`、`refresh`、`CheckFn`、`service::Check`、`Exposure::check`、三个 `DependencyStatus` 变体 | 内核 `registry.rs`、`internal.rs`、`ctx.rs`、`diagnostics.rs`、`service.rs`、`host.rs:133` 文档、`public-api.txt`；bridge `marked.rs:219–223`、`tests/marked_services.rs:391,431,554,564,569`；测试 `load_window.rs` 按 3.4 第 4 条改写，`contract.rs` 的 check 测试与 `lifecycle_diagnostics.rs` 那一个删，`parity.rs:345` 改；guarantees `core/D6`、`D7`、`D14`、`loader/rows-gate-on-services`（证据换成新测试）、`bridge/marked-services`；`core-features.md:71`、`development-handbook.md:137` | 四条保证的新证据测试存在并通过 | CL2（同改 `marked.rs`） |
| CL4 `waterfall_sync`（Rust 一侧） | 删内核 `waterfall_sync` 与 `Mode::WaterfallSync`、同步 next 与 3 处 `unsafe`；bridge `events/hub.rs:1160,1254`、`events/link.rs:495,578` 的分支；测试工具 `runtime/testing/events.rs` 与 `events.expected.json` 的 `waterfall-sync/*` 用例 | 内核 `host.rs`、`ctx.rs`、`events.rs`、`events/api.rs`、`events/remote.rs`、`bus.rs`、`bus/run.rs`、`lib.rs:38`；`examples/sync_decision.rs` 删（及 `migration-0.3-to-0.5` 外的链接）；`benches/events.rs:79–83`；loader `rust_participant.rs:407–409`；bridge `tests/events_expected.rs:39`；guarantees `core/D4`、`D16`、`D17`、`events/waterfall`、`tests/events.rs:602` 的悬空标签；文档 `plugin-api.md`、`rust-host.md:641,654`、`core-features.md`、`development-*`、`crates/rutis/README.md`、事件协议的方式表、两个 skills 的 `plugin-api.md` | `grep -ri waterfall.sync crates/ docs/guide .agents` 无结果；SDK 的接收分支此后已无发送方，由 CL26-* 删 | — |
| CL5 线上错误名 `CordisError` → `Error` | 改 `service.rs:1024`；生成器的旧路径识别删；生成文件与测试 | `crates/rutis/src/service.rs`；bridge `build/exports.rs:479–491,1856–1858,1876`；`examples/rust-services/rows/services.d.ts`、`examples/marked-services/rows/services.{d.ts,py}`；bridge `tests/marked_services.rs:274`；`rust-host.md:145,718` | 两个示例的快照测试与 `tsc`/`pyright` 通过 | — |
| CL6 `*_key` → `Key` + `*_at`；`platform()` 复查 | 删 `internal::{get_key, require_key, provide_key}`；调用方改持有 `Key<T>`；按 3.3 复查 `platform()` | 内核 `internal.rs`、`ctx.rs`、`deps.rs`；bridge `marked.rs:172,223`、`runtime/rows.rs:51,317`、`services.rs` 余下部分；loader `catalog.rs:120,125`、`runtime.rs:878,1008`、`runtime/rows.rs:122`；rutis-host `host.rs:1523`（测试） | `git grep -E '(get|require|provide)_key' crates/` 无结果；`platform()` 的处理写进 PR 说明 | #811 S5（被删的文件里还有调用点） |
| CL7 `plugin_from` | 删 | 内核 `host.rs:239`、`ctx.rs:1421`；`examples/development_workflow.rs:134`；测试 `instance_subtrees.rs:411`、`lifecycle_order.rs:276`、`typed_plugin.rs:375`；`rust-host.md:39,89`、`rust-plugin.md:615`（doctest 改 `PluginFactory`）、`core-features.md:7,23`、`development-handbook.md:98,101,336` | doctest 通过 | — |
| CL8 配置：校验进内核 | `rutis_loader::config::check` 搬进 `rutis::config`，删 `Options`/`required`/`older_than_0_18`；提示工具出 `internal`（`closest` 公开）；loader 把补过默认值的配置发给运行时；bridge 加载插件值前调 `check` | 内核 `config/`、`internal.rs:13–17`；loader `config.rs`（删）、`config_step.rs`、`loader/waits.rs:439,441`、`tests/config_cases.rs` → 内核测试；bridge `runtime/process.rs` 的插件值加载、`events/errors.rs:240`（测试）；rutis-host `call.rs:107`、`status.rs:580`（测试）；`rutis-host.md:238`；配置 Schema 设计稿 §2.1、§七 第 3 条 | 105 条用例在内核通过；插件值的错误配置在 bridge 一侧被拒（新测试）；各 SDK 的现有配置测试不改仍通过（SDK 校验此时还在，结果相同） | — |

**rutis-bridge**

| PR | 内容 | 闭环清单 | 完成条件 | 依赖 |
| --- | --- | --- | --- | --- |
| CL10 反方向 Cordis 挂载 | 删 `rutis_plugin`、`cordis::server`、`runtime/unix.rs`；Node 一侧的 `launch`/`dispose`/`executable`/`io-worker.mjs:55–79`；`cordis/build.rs:291` 的 syn 检查改手写；`cordis` 不再依赖 `exports` | bridge `cordis/build/rust.rs`、`cordis/build.rs:5–6,291`、`cordis/mod.rs:16`、`cordis/server.rs`、`runtime/unix.rs`、`Cargo.toml`；`node/rutis-runtime/src/client.mjs`、`io-worker.mjs`、`test/fixtures/rust-mount.test.mjs`（删）、`bench/sync-call.mjs`（删）；`examples/native-mount` 的 `src/lib.rs`、`src/bin/rutis-counter.rs`、`tests/cordis_mount.rs`、`tests/export_identity.rs`、`build.rs:9–14`、README；bridge `tests/rpc_callbacks.rs:305` 改走本机管道（测试名不变） | `boundary/5.6` 的测试通过；dsh 与 `examples/dsh-baseline`、`interop-experiments` 构建通过 | — |
| CL11a bridge：不再检查对端的旧能力 | bridge 不再要求对端声明 `events-v2`、不再按对端的 `objects`、`sync-wait` 改变行为；删 mount 层旧特性检查（0.3.0、`require_plugin_values`、无 `scopes` 的旧路）；删 `plugins` 自查自；bridge 自己仍声明这些能力 | `session/rpc.rs:1294,1579,1838,2421,2461,2500`、`relay.rs:41`、`crossing.rs:171–175`、`runtime/process.rs:1087,1231,1251,1265,1329,1517–1524,1577,1607`；测试 `rpc/tests.rs::a_far_end_without_events_v2_is_refused`、`event_hub.rs::a_runtime_without_events_v2_is_refused`、`rpc/tests.rs:2676–2706` 删；guarantees `events/v2-required` 删、`events/old-peer-refused` 改为按协议版本 | 0.18 及更早的对端在握手时被拒（#811 S2b 已升到版本 4） | #811 S2b |
| CL26-node / bun / py / go / swift / kotlin 各语言：旧宿主分支与旧能力 | 每种语言一个 PR：删 7.2 表里本语言的全部分支（`announce`、`events-v2` 要求、`refuseRemoved`、插件 API 默认、`rows.failed`、`objects` 与 `sync` 的对端检查、`waterfall-sync` 接收）；不再声明 `events-v2`、`objects`、`sync-wait`、`signals`；Python 的 PR 同时把最低版本提到 3.12（7.3）；Go 的 PR 同时删 `warnChainless`；Kotlin、Swift 的 PR 把"older host answers with a future"的注释改为现行行为（7.6 第 11 条） | 本语言 7.2 表列的文件；测试：Go `oldhost_test.go`、`events_session_test.go:455` 删，Node `runner-events.test.mjs`、Bun `test/old-host.test.ts` 的旧宿主用例删，Go 夹具 `internal/fixtures/session/main.go:193` 与 `objects.expected.json:180` 的 `no-objects-declared`（在本语言最后一个 PR 删用例）；本语言的 `public-api` 快照；Python：`_eager.py`、`_unhandled.py`、`tests/test_eager.py`、三处 `requires-python`、ci.yml 与 install.yml 的 3.10、文档里的 3.10；Go：`runtime.go:1165–1192`、`events.go:537`、`runtime.go:1075`、`design-go-runtime-2026-10-10.md:257`（两种语言） | 本语言的全部测试与 `runtime_conformance` 通过；`git grep` 本语言源码无 `events-v2`、`waterfall-sync` | CL11a；本语言的 #811 R-x 与 S2b（改同一个 `session` 文件）；Kotlin、Swift 另在 A1、A2 之后 |
| CL11b bridge：不再声明旧能力 | bridge 删 `EVENTS_CAPABILITY`、`signals`、`forwarding`、`objects`、`sync-wait` 的声明；`.agents` 的能力表 | `session/rpc.rs:922,1243–1251`、`session/testing.rs:201`、`rpc/tests.rs:1033`；`runtime-contract.md:34–42` | `grep` 这些能力名在 `crates/rutis-bridge/src` 与六个 SDK 的源码下无结果 | CL26-* 全部 |
| CL12a 测试起进程辅助 | `rutis-test-support` 加各语言起本机运行时的辅助与 `RUTIS_TEST_RUNTIMES`；bridge、loader、examples 的测试全部切过去；CI 各任务设置 `RUTIS_TEST_RUNTIMES` 代替 `--features` | `crates/rutis-test-support`；bridge 17 个测试文件、loader 约 20 个、`tests/common/runtimes.rs:218,243`、`examples/rust-services`、`marked-services` 的测试；`ci.yml`、`stress.yml`；`docs/ci.md:103` | CI 每个语言任务实际跑了它那门语言的测试（日志里没有 skipped） | #811 S5（`LocalRuntime` 已删，测试已改走 `pipe::spawn`） |
| CL12b 语言知识移出 bridge | 删 bridge 的 `Launcher::node/python/go/swift/bun`、`node_inherits`、`Process::launch*`/`attach`、`runtime/spawn.rs`、语言 feature；`Process::mount` 改走 `pipe::spawn`，Node 命令放进 `cordis` 模块；`go_runtime_name`/`swift_runtime_name` 搬 loader；输出分行与前缀（`output.rs`、`Stdio::Lines`、`output_prefix`、`prefix_output`）搬 rutis-host；loader 的 `python`、`bun`、`kotlin` feature 删 | bridge `runtime/process.rs`、`runtime/spawn.rs`、`runtime/mod.rs`、`transport/local/output.rs`、`transport/local/spawn.rs`、`Cargo.toml`；loader `Cargo.toml`、`runtime/go.rs`、`swift.rs`、`binary.rs:1550–1557,2293–2310`；rutis-host `lib.rs`、`host.rs:119–121`、`languages/*.rs`、`Cargo.toml:33`；`examples/*/Cargo.toml`、`tests/e2e/fixture-tenants`、`fixture-toolhost`（改用 rutis-host 库）；guarantees `host/output-lines-whole`、`host/output-prefix`；`rust-host.md:426–560`、`crates/rutis-bridge/README.md:5`、`crates/rutis-loader/README.md:17`、`bun/rutis-bun/README.md:28`、`rutis-new-runtime/SKILL.md:74–77` | `cargo tree -p rutis-bridge` 无语言 feature；dsh 构建与 dsh 测试通过；`rutis-host run` 的前缀输出逐字不变 | CL12a、#811 S4、S5 |
| CL13 #683 调用时限与服务名单 | 按 5.5 删；`request_sync` 的 `deadline` 退回 | bridge `runtime/local.rs`、`plugin.rs`、`rows.rs`、`session/mod.rs`、`session/rpc.rs`、`limits.rs`、`relay.rs`、`report.rs`；loader `runtime.rs:1015–1019`、`lib.rs`（doctest `OpenHostGuide`）；测试 bridge `tests/plugin_limits.rs`（留 greeting 两个）、loader `tests/plugin_limits.rs`（留两个）；guarantees `host/plugin-limits`；`open-host.md` 第 3 节、`rust-host.md:512–523`、`plugin-api.md:253`；设计稿 `design-host-plugin-limits`、`design-host-writes-plugins` L3、`design-diagnostics` §6.1 末句 | `git grep -E 'call_limit|kill_on_timeout|allowed_services|may_use' crates/ docs/guide` 无结果 | 在 #811 S4 之前（S4 搬 `LocalRuntime` 时少搬这些） |

**rutis-loader**

| PR | 内容 | 闭环清单 | 完成条件 | 依赖 |
| --- | --- | --- | --- | --- |
| CL17 loader：重复与没人用的接口 | 删 `share_by_name`、`share_registered_only`、`by_name`；`waiting_for`、`locate`、`schema_of`；`register_instance_keyed`、`readable_instance_keyed`、`readable_instance`；16 个 Go/Swift 别名、`read_go_manifest`、`read_swift_manifest`；`internal` 的 `GO_MARKER`、`SWIFT_MARKER` | loader `catalog.rs`、`loader/api.rs`、`runtime/go.rs`、`swift.rs`、`binary.rs:1044,1080`、`runtime.rs:27,33,49`、`lib.rs`；rutis-host `host.rs:216`；`tests/e2e/fixture-toolhost/src/main.rs:171`；测试 `runtime_rows.rs:1036`、`fault_containment.rs:170`、`remote_modes.rs:199`、`runtime_instances.rs:336`、`check.rs:106`、`scope.rs:52`、`marked_services.rs:253,458,466`、`loader.rs:458,460`、`peer_rows.rs:230`、`instance_services.rs:170`、`swift_rows.rs:18,130` 与使用别名的十几个测试；guarantees `bridge/marked-services`、`loader/schema`；`rust-host.md:243,434,487,539–547`、`open-host.md:129`、`go-plugin.md:414`、`rutis-remote-plugin/SKILL.md:48`、`design-host-writes-plugins` §九 | `git grep` 这些名字在 `crates/ docs/guide .agents tests/e2e` 下无结果 | 在 #811 S5 之前；#811 S5 不再做 `share_registered_only` 一行 |
| CL18 loader：`internal` 收掉 | `apply_patches` 与 `Composed`、`ComposedRow`、`Owner`、`PatchWarning` 公开为 `rutis_loader::patch`；`is_runtime_name`、`read_binary_manifest` 公开；rutis-host 改用 `Go::PLUGIN_API`、`Swift::PLUGIN_API`；删 `internal` 模块 | loader `lib.rs:89–105`、`patch.rs`；dsh `profile/mod.rs`、`layers.rs`、`include.rs` 与 dsh 测试；loader 测试 `loader.rs`、`patch_parity.rs`、`regressions.rs`；rutis-host `config.rs:608`、`dev.rs:611`、`project.rs:133`、`languages/go.rs:170`、`main.rs:32,33` | `git grep 'rutis_loader::internal' ` 无结果；dsh 测试通过 | #811 S5（`REMOVED_EVENTS`、`node_schema`） |
| CL19 loader：删 Rust 的 npm 解析 | 按 5.4 第 1 条：删 `npm` 模块、`package_version`、`resolve_entry`、`Naming::Npm` 的本机分支、`url` 依赖；不带前缀的名字交给 Node 运行时解析 | loader `runtime.rs`、`Cargo.toml`；rutis-host `languages/node.rs:84`；#805 合并后它的 `installer` 里对 `resolve_entry` 的调用改为交给运行时（同一个 PR）；测试 `runtime.rs` 的单元测试、用到本机 npm 解析的 loader 测试 | `typescript-plugin` 指南的例子、模板 `node` 项目、dsh 的 Cordis 行端到端通过 | #811 S3（改同一个 `runtime.rs`）；#805 先合并时在这里改它 |
| CL20 Go/Swift 只留语言前缀行名 | 按 5.4 第 2 条 | loader `binary.rs:990–1012,1084–1094`；rutis-host `languages/go.rs:107–109,164–178`、`swift.rs`、`dev.rs:922–926`、`project.rs:345,428`；持久化样本（`tests/fixtures/persisted/*`）删限定写法并加 `rejected/` 用例；测试中 33 处 `"go-…:…"`、14 处 `"swift-…:…"`；guarantees `loader/go-runtimes`、`loader/swift-runtimes`；`rutis-host.md:513,596`、`go-plugin.md:414`、`.agents/.../rutis-json.md:212` | `dev` 下改一个 Go 插件只重载它；同名冲突的报错写出两个文件名 | CL19 |
| CL21 一行的状态合成一套 | 删 `status::RowState`，快照用 `EntryStatus`；`report::RowState` 改名 `RowEventState` | loader `loader/status.rs`、`framework.rs:18`、`lib.rs:75`；rutis-host `call.rs:10,137`、`status.rs`；测试 `status.rs`、`row_events.rs`、`events_conformance.rs` | `rutis-host status`、`call` 的输出逐字不变；`rutis/row` 事件的取值不变 | CL13（`plugin_limits.rs` 用到 `status::RowState`） |
| CL22 重启策略搬 rutis-host | 按 5.2；loader 只留 `restart(name)`、状态、按需启动与闲置停止 | loader `runtime/binary.rs`、`backoff.rs`、`lib.rs`；rutis-host `restart.rs`、`host.rs:22,450–535,831`、`languages/binary.rs`、`config.rs:350`、`main.rs:329,340`、`dev.rs`；guarantees `loader/go-runtimes`、`host/runtime-restart`；`rust-host.md:557` | Go 与 Node 运行时崩溃后的重启输出逐字相同、只有一份实现 | #811 S4（二进制运行时成为 spawn 行）、CL12b |

**rutis-host 与工具**

| PR | 内容 | 闭环清单 | 完成条件 | 依赖 |
| --- | --- | --- | --- | --- |
| CL23 删 rutis-dev 与 overlay 层 | 删 crate；删 loader 的 `set_overlay`、`overlays()` 与 overlay 状态（5.4 第 5 条） | loader `loader/api.rs:97,115`、`loader/mod.rs:412,528–536`、`loader/commit.rs:312–332`、`tests/overlay.rs`；guarantees `loader/overlay`；根 `Cargo.toml`、`Cargo.lock`、`ci.yml:175`、`nightly.yml:112`、`docs/ci.md:80`、`docs/quality-status.md`、`design-host-dev-mode` 状态行 | workspace 构建通过；`set_overlay`、`overlays` 在 `crates/` 下搜不到 | — |
| CL24 删 rutis-migrate | 删 crate，退出发布列车 | 根 `Cargo.toml`、`Cargo.lock`、`scripts/train.mjs:26–27`、`release.yml:8`、`docs/release.md:9` | `node scripts/train.mjs --crates` 不再列它；ci 的打包检查通过 | — |
| CL25 删 `go add` | 按 6.4 第 1 条 | rutis-host `main.rs:11,76–78,100,383–416`；guarantees `host/go-add`；`rutis-host.md:78,96`、`go-plugin.md:411`、`templates/go/README.md:11`；`design-plugin-install` 兼容一节与 I1 | — | #805 合并 |
| CL27 `rutis-host test` | 加 `rutis-host test`（标准输入输出上的 session、`--listen`）与插件 `rutis-host/test` | rutis-host `main.rs`、新 `test.rs`；`tests/` 端到端测试；`rutis-host.md` | 用 Node 与 Python 的最小测试插件经它跑通 load、on、emit、update、unload | #811 S4（session 与 listen） |

**生成类型**

| PR | 内容 | 闭环清单 | 完成条件 | 依赖 |
| --- | --- | --- | --- | --- |
| CL15 生成器搬出平台 | 按 §八：`build/` 三个文件原样搬到新 crate `crates/rutis-types`；删 bridge 的 `pub mod build`、`exports` feature 与 syn、quote、proc-macro2 依赖；rutis-host 与两个示例改依赖 | bridge `lib.rs:32`、`build/`、`Cargo.toml:33–35,77`；新 `crates/rutis-types`（`Cargo.toml`、README 两种语言）；根 `Cargo.toml` 的 members、`Cargo.lock`；rutis-host `Cargo.toml:32`、`main.rs:150–184`；`examples/rust-services`（build-dependency、`build.rs` 的 `use`）、`examples/marked-services`（dev-dependency、`tests/rows.rs`）；`scripts/train.mjs`、`docs/release.md:9`；guarantees `bridge/marked-services` 的测试路径；`rust-host.md:246,280,667–675,731–748` 的依赖与路径；`design-host-contract-types` §三、§4.1 | 两个示例的生成结果与提交的文件逐字相同；`rutis-host types` 输出不变；`cargo tree -p rutis-bridge` 里没有 syn | CL5、CL10 |

**各语言 SDK**（每种语言一组，组内按序）

| PR | 内容 | 闭环清单 | 完成条件 | 依赖 |
| --- | --- | --- | --- | --- |
| CL28-node、-py、-go、-swift、-kotlin #692 旧名 | 按 7.1 删本语言的转发；Go 先把实现移进 `internal/runtime`（同一个 PR）；Kotlin 版本常量挪进 `Versions.kt` | 本语言 7.1 表列的文件与公开面快照；Node `testing.mjs:37`、`testing.d.ts:1`；`scripts/train.mjs:43,48`、`release.md:18`（Go、Kotlin）；第一个合并的 PR 同时改 `release.md:36`、`plugin-api.md:478`、`python-plugin.md:389` 的规则与 SDK 入口设计稿 §五；每个 PR 改《迁移到 0.19》#692 一节里本语言的一行 | 本语言公开面快照里没有 deprecated 一节；仓库内无旧名 | — |
| CL29-bench 事件基准 | 按 7.5 加 `tools/bench/events`，记录删前数字 | `tools/bench/events`、`docs/performance-*`（新的一份，两种语言） | 删前的数字提交在文档里 | — |
| CL30-node、-py、-go、-swift、-kotlin 叶子只剩叶子 | 本语言的测试工具改为真实运行时 + `rutis-host test`；删模拟宿主规则的部分、本地事件分发（Node 留 Cordis 节点一份）、SDK 的配置校验（留解码）；Kotlin 测试工具拆成 `rutis-testing` artifact | 本语言 7.5 各表列的文件；本语言跑 `cases.json` 的测试删；本语言的测试工具测试（含 #770 的 `testing-on`、#810 的 H4 测试）改为跑在真实内核上；模板 `crates/rutis-host/templates/<语言>/` 的依赖与测试；本语言指南的测试工具一节与 README；Kotlin `build.gradle.kts`、`api/*.api`、`MainThreadTest.kt`；最后一个合并的 PR 改 guarantees `sdk/testing-matches-runtime`、`sdk/config-schema`、`sdk/undeclared-use`、`events/presence`、`events/hub-filter`、`events/one-bus`，`specs/config-schema/README.md`，配置 Schema 设计稿 §2.1 | 本语言模板项目 `test` 通过且在 7.5 的时间内；基准在 7.5 的可接受范围内（不在范围内时停下报告） | CL8、CL27、CL29-bench、本语言的 CL26；Kotlin、Swift 另在 A1、A2 之后 |

### 13.3 先后

```text
可以马上开始、互不依赖：CL1 → CL2 → CL3；CL4；CL5 → CL15（另需 CL10）；CL7；CL8；CL10；CL13；CL17；CL23；CL24；CL28-*；CL29-bench
#811 S2b 之后：CL11a → CL26-*（各语言，另需本语言 R-x；Kotlin、Swift 另需 A1、A2）→ CL11b
#811 S3 之后：CL19 → CL20
#811 S4 之后：CL27 → CL30-*（另需 CL8、CL29-bench、本语言 CL26）
#811 S5 之后：CL6；CL18；CL12a → CL12b → CL22
#805 合并之后：CL25（CL19 若在 #805 之后，在 CL19 里改 #805 的调用）
CL13 在 #811 S4 之前；CL17 在 #811 S5 之前；CL21 在 CL13 之后
```

全部合并后，§十二 的文档已随各 PR 改完；发布前按 #690 的发布门跑一遍开发者体验检查，《迁移到 0.19》的"平台清理"一节由各 PR 逐条写入，最后一个 PR 核对条目齐全。

