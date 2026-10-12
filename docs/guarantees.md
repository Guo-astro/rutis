# rutis 保证登记表

[English](guarantees.en.md) · [质量规范](quality-standard.md) · [质量执行现状](quality-status.md)

状态：登记中（#181）。本表登记 rutis 的核心承诺与契约行为，每条一行：编号、出处、内容、级别、状态、对应的验证。规则依据是[质量规范](quality-standard.md) §3（承诺的分级）与 §9.4（Q9.4.1–Q9.4.4）。

本表**人工维护**：不从设计文档自动抽取，CI 也不核对表与测试是否一致（误报多，维护成本高于收益；条目超过 300 条时再考虑）。表的正确性靠 PR 作者和评审在改动时同步更新（§5）。

已登记：`core/`（D 编号、`core/features.*`、`core/typed.*` 中已有测试的条目）、`boundary/`，以及 `instance/`、`bridge/`、`host/`、`dsh/`、`sdk/` 中已有测试的条目。`loader/` 族与 `host/` 的其余条目由后续 PR 补齐；全部补齐后关闭 #181。

---

## 1. 发布检查

- **0.10**：本表作为基线发布，记录当时各条的状态。0.10 发布不以本表的状态为门槛。
- **从 0.11 起**执行 Q9.4.4：每次发布前，人工核对本表，
  1. 级别为"核心承诺"、状态为"未验证"的条目必须为零；
  2. 级别为"契约行为"、状态为"未验证"的条目数不得多于上一次发布。
- 核对结果写在发布 PR 的描述里（两个数字：未验证的核心承诺数、未验证的契约行为数，以及与上一次发布的比较）。

"部分验证"不计入上面两个数字，但核心承诺处于"部分验证"的条目应在质量执行现状中有对应的计划。

---

## 2. 编号

编号形如 `<族>/<名字>`，一经登记不复用；某条保证被撤销时，行保留，状态写明撤销及原因。

| 族 | 编号形式 | 出处 | 状态 |
| --- | --- | --- | --- |
| `core/` | `core/D1`–`core/D31` | [design-rust-port](design-rust-port.md) §三 决策表 | 已登记 |
| `core/` | `core/D32`、`core/D33` | [design-config-hot-update-and-dynamic-events-2026-09-21](design-config-hot-update-and-dynamic-events-2026-09-21.md) §一、§二 | 已登记 |
| `core/` | `core/features.<节>` | [core-features](core-features.md) 的各节 | 部分登记 |
| `core/` | `core/typed.<名字>` | [design-typed-plugin-2026-10-02](design-typed-plugin-2026-10-02.md) | 已登记 |
| `boundary/` | `boundary/5.1`–`boundary/5.8` | [requirements-protocol-plugins](requirements-protocol-plugins.md) §5 的 8 条边界规则 | 已登记 |
| `instance/` | `instance/<设计>.<节号>` | 实例相关设计：`subtrees` = [design-instance-subtrees](design-instance-subtrees.md)，`services` = [design-instance-services-2026-10-08](design-instance-services-2026-10-08.md) | 部分登记 |
| `bridge/` | `bridge/<名字>` | 会话、链路、承载、跨语言值：[design-remote-plugins-2026-10-03](design-remote-plugins-2026-10-03.md)、[requirements-protocol-plugins](requirements-protocol-plugins.md) §6、[插件 API](guide/plugin-api.md)、质量规范 Q6.2、Q6.5 | 部分登记 |
| `loader/` | `loader/<名字>` | rutis-loader 设计 | 预留 |
| `host/` | `host/<名字>` | rutis-host 设计与用户文档 | 预留；`host/rutis-json-format` 已登记（#207），`host/link-state-output` 已登记（#321），`host/output-lines-whole` 已登记（#332） |
| `dsh/` | `dsh/<名字>` | rutis-dsh 设计与文档 | 预留；`dsh/user-layer-format` 已登记（#207） |
| `sdk/` | `sdk/<名字>` | 插件 SDK 与测试工具：[插件 API](guide/plugin-api.md)、质量规范 Q6.7 | `sdk/testing-matches-runtime` 已登记（#206） |

各族的 `<名字>` 规则：

- `core/D<n>`：沿用设计文档里的决策编号（包括复述其他编号的 D20、D27 和已并入 D27 的 D26），方便与设计文档、测试注释对照。
- `core/features.<节>`：core-features 一节一个英文小写短名，如 `errors` =「错误与 panic」。一节里有多条保证时，在短名后加 `.<名字>`。
- `core/typed.<名字>`、`bridge/<名字>`、`loader/<名字>`、`host/<名字>`、`dsh/<名字>`、`sdk/<名字>`：短横线连接的英文小写短语，说出保证本身（如 `bridge/in-flight-call-fails-once`）。
- `instance/<设计>.<节号>`：设计文档的短名加节号，如 `instance/subtrees.2.2` 是 design-instance-subtrees §2.2。一节里有多条保证时，在节号后加 `.<名字>`。

---

## 3. 级别与状态

**级别**按质量规范 §3：

| 级别 | 含义 |
| --- | --- |
| 核心承诺 | Q3.1：使用者依赖它构建系统，失效时无法自行绕过（依赖门控；清理恰好一次；失败时回滚、旧状态继续工作；使用方只依赖服务名而不依赖实现；跨边界做不到的事显式报错而不静默降级；中断时调用显式失败、不报假成功、不自动重试副作用；结束后不留残余） |
| 契约行为 | Q3.2：写在设计文档或用户文档中的具体行为（顺序、参数、默认值、错误类别、超时） |
| 明确不保证 | Q3.4：文档写明不提供的行为 |

实现细节（Q3.3）不登记。一行同时含有保证部分和"不保证"部分时，按保证部分定级，"不保证"部分写在内容里。

**状态**：

| 状态 | 含义 |
| --- | --- |
| 已验证 | 至少有一个自动化验证断言了该行为的全部要点 |
| 部分验证 | 有验证，但只覆盖其中一部分要点，或断言方式掩盖了要验证的性质；"说明"写明缺什么 |
| 未验证 | 没有任何自动化验证断言它 |
| 明确不保证 | 级别为"明确不保证"的条目；如果有验证锁定"确实不提供"，在"验证"列列出 |

状态只说明"有没有验证断言它"。核心承诺在 Q3.1 下还有更高的要求（每个支持的平台、故障注入下、两种独立方式），这些要求是否满足记录在[质量执行现状](quality-status.md)，不在本表。

---

## 4. 验证的写法

**本表中**："验证"列列出测试函数，写法是 `<路径>::<函数>`，路径相对 `crates/`，例如 `rutis/tests/contract.rs::state_transitions`；`crates/` 之外的文件写仓库根起的路径（如 `node/rutis-runtime/test/...`）。编译期即可保证的条目写"编译期"并说明依据。

**测试中**：在测试函数（或整个测试文件）的文档注释里写编号，便于从测试反查本表：

```rust
/// guarantee: core/D24
#[tokio::test]
async fn status_events_arrive_in_commit_order() { /* ... */ }
```

一个测试验证多条保证时写多个编号，用逗号分隔：`guarantee: core/D6, core/D14`。其他语言写在对应的注释里（`//`、`#`）。

### 早期标记

本表建立之前写下的 3 处 `guarantee:` 标记用的是 issue 和风险编号：

| 位置 | 现在的编号 |
| --- | --- |
| `rutis-bridge/tests/concurrent_launch.rs::spawned_processes_started_at_once_hold_only_their_own_channel`、`::node_runtimes_launched_at_once_hold_only_their_own_channel` | 已改为 `bridge/spawned-processes-own-channel` |
| `rutis-host/tests/signals.rs`（文件级：清理恰好一次、信号结束宿主时运行时进程已退出，Q5.4.4、Q6.6.3） | 未改：`host/` 族尚未登记，登记时一并改；其中"清理恰好一次"在内核层面是 `core/D6` |

---

## 5. 什么时候更新本表

- 新增或修改设计文档、用户文档中的保证：同一个 PR 登记或修改对应行（级别必须写明，Q3 的要求）。
- 新增验证某条保证的测试：在测试里写 `guarantee: <编号>`，同一个 PR 把测试加到"验证"列，必要时修改状态。
- 删除、跳过（`#[ignore]`、平台条件跳过）或削弱某个列出的测试：同一个 PR 修改"验证"列与状态（Q9.4.3）。
- 测试改名或移动文件：同步修改"验证"列。

PR 模板中有对应的检查项。

---

## 6. 核心：`core/D1`–`core/D33`

出处：D1–D31 见 [design-rust-port](design-rust-port.md) §三；D32、D33 见 [design-config-hot-update-and-dynamic-events-2026-09-21](design-config-hot-update-and-dynamic-events-2026-09-21.md) §一、§二。基准 `main` `121eded`。

核心承诺的内容写可观察的行为；实现机制（锁、任务、三元组等）留在设计文档（Q3.3）。D20、D26、D27 是其他编号的复述或已并入，编号保留、不单独计数。

| 编号 | 内容 | 级别 | 状态 | 验证 | 说明 |
| --- | --- | --- | --- | --- | --- |
| `core/D1` | 需要 dyn 的 trait 统一用手写别名 `BoxFuture<'a, T>`，不用 async-trait 宏 | 契约行为 | 已验证 | 编译期：所有实现 `Plugin` / `Listener` 的测试（如 `rutis/tests/contract.rs`） | 类型层面的约定，无行为测试 |
| `core/D2` | 事件载荷类型化（`Event` trait），内部以 `Arc<dyn Any + Send + Sync>` 擦除；`serde_json::Value` 只用在序列化边界，不进核心 | 契约行为 | 已验证 | `rutis/tests/contract.rs::emit_async_safe`、`rutis/tests/typed_dispatch.rs::typed_identity_and_prefix_scope_are_distinct`；`crates/rutis/Cargo.toml` 无 serde 依赖 | "不进核心"只由依赖清单体现，没有自动检查 |
| `core/D3` | 回调注册表式总线（钩子语义），监听器为 owned `Arc<dyn Listener>`，不引入 channel | 契约行为 | 已验证 | `rutis/tests/contract.rs::plugin_registers_n_listeners`、`::prepend_order`、`::once_once` | |
| `core/D4` | 四种分发：`emit` fire-and-forget（Err / panic 进 ErrorSink）；`parallel` 等全部并聚合错误；`serial` 按序至短路；`waterfall` 续延式，不调 `next` 即否决 | 契约行为 | 已验证 | `rutis/tests/contract.rs::emit_async_safe`、`::emit_error_sink`、`::parallel_aggregates`、`::parallel_waits_all`、`::serial_bails`、`::waterfall_veto_around` | |
| `core/D5` | fiber 状态在单锁域内修改；锁内只做 FIFO 入队，绝不持锁跨 await，回调在锁外执行 | 契约行为 | 部分验证 | `rutis/tests/contract.rs::mixed_concurrent_ops_complete`、`rutis/tests/interleave.rs::restart_settle_and_dispose_interleave`、`::dependency_churn_interleaves_with_restart`；CI clippy `-D warnings` 含 `await_holding_lock` | "持锁跨 await"由 clippy 静态检查（只覆盖 std 锁）；锁内 FIFO 入队的顺序没有直接断言（见 D24） |
| `core/D6` | 卸载时插件注册的清理恰好执行一次：并发的 dispose 等待同一次卸载并得到同一个错误；同一代不重复执行，重启后的新一代重新开始 | 核心承诺 | 已验证 | `rutis/tests/contract.rs::concurrent_dispose_join`、`::exactly_once_same_error`、`::dispose_idempotent`、`::cross_generation_isolation`、`rutis/tests/parity.rs::returns_one_disposal_promise_and_joins_cleanup_already_in_progress` | 机制（锁内单个 `Arc<TransitionTask>`）见设计文档；D20 是同一条 |
| `core/D7` | 协作取消与卸载顺序：每代插件有自己的取消令牌（`ctx.cancelled()`）；卸载时先发布 Unloading 并取消当前代，等 apply 退出，再按 LIFO 清理，全部清理结束后才发布 Disposed；不观察令牌的插件会让 dispose 一直等（写明的限制） | 核心承诺 | 已验证 | `rutis/tests/lifecycle_order.rs::unload_publishes_unloading_and_cancels_before_cleanup_and_disposed_after`、`::unload_waits_for_apply_to_exit_before_cleanup`、`rutis/tests/contract.rs::cancel_wakes_awaiters`、`::cancel_during_loading`、`::cancel_idempotent`、`::cancel_cascades`、`::disposal_deadline_reports_loading_and_later_joins` | 断言了"发布 Unloading""取消""等 apply 退出"都先于清理、清理按 LIFO、Disposed 在清理全部结束之后。dispose 等在调用时就预取消，所以"发布 Unloading"与"取消"之间的先后从外部观察不到，不断言。D27（及并入它的 D26）是同一条 |
| `core/D8` | runtime 接入：优先构造时注入 `Handle`（`Ctx::root_with`）；自动路径 `Handle::try_current()` 失败返回明确错误，绝不隐式创建 runtime | 契约行为 | 已验证 | `rutis/src/ctx.rs::root_without_a_runtime_is_an_explicit_error` | |
| `core/D9` | 过期的上下文（旧一代）发起的操作被拒绝，判定不依赖调度顺序；**监听器完成顺序不作契约**（明确不保证） | 契约行为 | 已验证 | `rutis/tests/contract.rs::cross_generation_isolation`、`rutis/tests/generation_registration.rs::rejects_late_registration_current_thread`、`::rejects_late_registration_multi_thread`、`::rejects_registration_after_failed_apply`、`rutis/tests/host_control.rs::a_stale_context_cannot_dispose_the_next_generation` | 按保证部分定级。`yield_now` 只是公平性提示，属实现。"完成顺序不固定"没有测试锁定 |
| `core/D10` | 插件身份 = `FiberView.id: PluginId`，显示名为 `Plugin::name()`；`is_unique` 式去重可选 | 契约行为 | 已验证 | `rutis/tests/host_control.rs::view_finds_a_fiber_by_id` | `is_unique` 未实现（设计写明可选） |
| `core/D11` | 错误类型 `CordisError`：thiserror 派生（thiserror 是普通依赖），`Error + Send + Sync + 'static`，**不实现 Clone**，以 `Arc` 共享，聚合为 `Vec` | 契约行为 | 已验证 | `rutis/src/error.rs` 中 `CordisError` 的两个文档测试（`compile_fail,E0277` 断言不实现 Clone；正例断言 `Error + Send + Sync + 'static` 与 `Arc` 共享） | |
| `core/D12` | 配置校验：`Plugin::validate` 校验实例自持的配置，校验通过才存储；不引入 schema 库 | 契约行为 | 已验证 | `rutis/tests/contract.rs::validate_panic_marks_failed`、`rutis/tests/host_control.rs::set_config_stores_without_restarting`、`rutis/tests/config_update.rs::update_dry_run_failure_leaves_state_untouched` | |
| `core/D13` | 服务定位器 `get::<T>() -> Option<Arc<T>>`：显式、类型键、以 `None` 表示失败；沿父链回溯 | 契约行为 | 已验证 | `rutis/tests/contract.rs::typed_roundtrip`、`::parent_chain_lookup`、`::keyed_multi_instance` | |
| `core/D14` | 依赖门控重载：提供方撤销服务时，依赖它的消费者先停下（回到 Pending），提供方最后清理，清理期间仍能访问自身服务；只驱逐依赖被撤销服务的消费者；服务恢复后消费者重新启动 | 核心承诺 | 已验证 | `rutis/tests/contract.rs::eviction_order`、`::self_access_during_cleanup`、`::single_service_evict`、`::provider_reload_reactivates`、`::check_evicts`、`rutis/tests/instance_subtrees.rs::closing_subtree_disposes_consumer_before_provider` | 设计中的"消费者并发排干"是机制，没有断言：`eviction_order` 断言的是链式依赖的先后（C → M → P），不是并发 |
| `core/D15` | tower 兼容只作为可选适配层，**不进核心**；生命周期钩子不套用 `Service` / `Layer` | 明确不保证 | 明确不保证 | — | 核心不提供 tower 兼容；目前也没有适配层 |
| `core/D16` | 事件系统全异步，四种分发；监听器 helper trait `call<'a>`；`Event::Value` 关联类型；`emit` / `parallel` 载荷为 `Arc<E>` | 契约行为 | 已验证 | `rutis/tests/contract.rs::emit_async_safe`、`::parallel_aggregates`、`::serial_bails`、`::waterfall_veto_around` | "删 bail"之后在 0.5 增加了同步面 `bail_sync` / `waterfall_sync`（见 [core-features](core-features.md)），属于 `core/features.*`，待登记 |
| `core/D17` | waterfall 有独立注册面（`on_waterfall` + `WaterfallListener`）；`next` 是调用方提供的兜底续延 | 契约行为 | 已验证 | `rutis/tests/contract.rs::waterfall_veto_around`、`rutis/tests/event_keys.rs::keyed_waterfall_veto_and_chain` | |
| `core/D18` | `Effect` 的两个 Disposer 变体返回 `Result<(), CordisError>`；闭包捕获 owned `Ctx` | 契约行为 | 已验证 | `rutis/tests/contract.rs::effect_yields_disposer`、`::aggregate_no_flatten`、`::restart_cleanup_errors_to_sink`、`rutis/tests/cleanup_errors.rs::unconsumed_early_cleanup_error_reaches_unload_with_same_identity` | |
| `core/D19` | 配置烘进实例：`Plugin` 无关联类型，具体插件 `new(config)` 自持；注册表只存 `Arc<dyn Plugin>` | 契约行为 | 已验证 | `rutis/tests/parity.rs::plugin_apply_functional_plugin`、`rutis/tests/contract.rs::plugin_provides_n_services` | 设计中的"`update` 推迟到 M4"已由 D32 实现 |
| `core/D20` | 同 D6 | — | — | — | 设计文档写"见 D6"；编号保留，不单独计数 |
| `core/D21` | 驱逐精确到提供方的那一次注册：只有依赖被撤销的那个提供方（同一插件、同一代、同一服务键）的消费者被驱逐；`isolate(key, label)` 按服务键隔离作用域，同 label 共享，子作用域找不到时回退父作用域；使用方只依赖服务键，不依赖由哪个插件提供 | 核心承诺 | 已验证 | `rutis/tests/contract.rs::isolate_scoping`、`::isolate_no_cross_evict`、`::mismatched_service_key_never_satisfies_injection`、`rutis/tests/parity.rs::isolate_isolated_context`、`::isolate_shared_label`、`::distinguishes_provider_incarnations_without_a_global_counter` | 机制（`(PluginId, generation, ServiceKey)` 三元组）见设计文档 |
| `core/D22` | 依赖未就绪的插件保持 Pending，不启动、不报错，依赖到位后启动；**不检测依赖环**（服务动态注册，环无法证明；明确不保证） | 核心承诺 | 部分验证 | `rutis/tests/contract.rs::pending_not_failed`、`::waits_for_dependency`、`::late_provider_activates` | 核心承诺只指门控部分，已验证；"不检测依赖环"没有回归测试锁定（#177） |
| `core/D23` | `Ctx::effect()` 注册清理并交回 Disposer；增量式（async-iterable）effect 刻意不提供 | 契约行为 | 已验证 | `rutis/tests/contract.rs::effect_yields_disposer`、`::effect_during_cleanup_fails`、`rutis/tests/parity.rs::rejects_effect_registration_during_unload` | 增量式 effect 不提供，无需测试 |
| `core/D24` | `FiberStatusChanged`：按提交顺序投递（每个插件的 seq 连续，每条的 from 是上一条的 to）；不保证监听器完成顺序 | 契约行为 | 已验证 | `rutis/tests/contract.rs::state_transitions`、`rutis/tests/event_order.rs::status_events_arrive_in_commit_order_across_concurrent_fibers` | |
| `core/D25` | 错误分层：身份归 `TransitionTask`（缓存 `Arc<CordisError>`），包装归 `PluginFailed`（`#[source]`），不递归 | 契约行为 | 已验证 | `rutis/tests/contract.rs::exactly_once_same_error`、`rutis/src/error.rs::plugin_failure_display_and_sink_preserve_each_cause_once`、`::sink_recurses_into_aggregate_used_as_plugin_failure_source`、`rutis/tests/parity.rs::keeps_plugin_execution_failure_separate_from_rollback_cleanup_failure` | |
| `core/D26` | 并入 D27（即 D7） | — | — | — | 编号保留，不单独计数 |
| `core/D27` | 同 D7 | — | — | — | 设计文档写"见 D7"；编号保留，不单独计数 |
| `core/D28` | 资源所有权：经 `Ctx` 注册的资源自动归 fiber；`Disposer` 只用于提前释放；isolate 出的 `Ctx` 保留原 fiber 的所有权 | 核心承诺 | 部分验证 | `rutis/tests/contract.rs::auto_ownership`、`::effect_yields_disposer`、`rutis/tests/event_keys.rs::keyed_listener_removed_with_owner_fiber` | "isolate 出的 `Ctx` 保留原 fiber 所有权"没有测试（插件经 isolate 注册、卸载后资源被回收） |
| `core/D29` | 事件跨 isolate 不过滤；isolate 只隔离服务注册表；需要作用域事件时用户自建子总线 | 契约行为 | 已验证 | `rutis/tests/event_order.rs::events_are_not_filtered_by_isolate` | |
| `core/D30` | 插件代码失败或 panic 不挂死、不越界：apply 失败或 panic 时该插件进入 Failed 并回滚已注册的资源，等待者收到显式错误；监听器 panic 进 ErrorSink；清理 panic 被捕获并包装为 `PluginFailed`；waterfall 回调的 panic 原样传给调用方 | 核心承诺 | 已验证 | `rutis/tests/contract.rs::apply_panics_complete_and_roll_back_current_thread`、`::apply_panics_complete_and_roll_back_multi_thread`、`::driver_panic_completes_waiters_with_error`、`::apply_failure_rolls_back`、`::emit_error_sink`、`::cleanup_panics_contained`、`::serial_panic_contained`、`rutis/tests/parity.rs::keeps_plugin_execution_failure_separate_from_rollback_cleanup_failure`、`rutis/tests/event_order.rs::async_waterfall_panic_reaches_the_caller`、`tests/e2e/tests/runtime_crash.rs::a_killed_runtime_fails_its_calls_and_a_restart_recovers` | 对应 Q3.1 的"失败时回滚"与"中断时显式失败" |
| `core/D31` | `emit` 派发序：同一事件类型按发射顺序串行派发（多个发射者各自的顺序都保持）；panic 不断链；监听器内重入 emit 同类型排到链尾不死锁；**跨事件类型不保证顺序**；监听器完成顺序仍不作契约 | 契约行为 | 已验证 | `rutis/tests/event_order.rs::emit_order_is_kept_per_emitter`、`::emit_after_listener_panic_is_still_delivered`、`::emit_inside_listener_goes_to_the_back`、`rutis/tests/dispatch_chain_probe.rs::dispatch_chain_does_not_fork_under_concurrent_emit`、`rutis/tests/event_keys.rs::keyed_emit_preserves_emission_order_per_name`、`rutis-agent/tests/order_probe.rs::emit_delivers_in_order_back_to_back_multi_thread` | `rutis-agent` 是示例项目，不在测试范围内，只作参考 |
| `core/D32` | 配置热更新：运行中 `update(config)`；先 dry-run（`validate_config` → `build` → 实例 `validate`），失败返回错误、不存不重启，旧状态继续工作；成功后按状态矩阵重启生效；config 不进依赖身份；**不提供 `internal/update` waterfall，持久化归宿主层**（D32d，明确不保证） | 核心承诺 | 已验证 | `rutis/tests/config_update.rs` 全部 15 个测试，如 `::update_active_applies_new_config_and_cleans_once`、`::update_dry_run_failure_leaves_state_untouched`、`::update_during_loading_converges_to_new_config`、`::concurrent_update_and_dispose_settle_correctly`；`rutis/tests/lifecycle_order.rs::an_update_dropped_after_its_cleanup_started_still_completes`、`::an_update_dropped_before_its_first_poll_changes_nothing` | |
| `core/D33` | 动态事件键：`TypeKey` 限定名接受运行时字符串；事件总线 keyed 面，四种分发、once / prepend、随 fiber 清理全部继承；同名不同类型、不同名之间互不串；**事件仍不按 isolate 过滤**（D33d，同 D29） | 契约行为 | 已验证 | `rutis/tests/event_keys.rs` 全部 11 个测试，如 `::keyed_channels_are_isolated_by_name`、`::runtime_constructed_name_dispatches`、`::same_name_different_event_types_do_not_cross` | D33d 由 D29 的测试覆盖 |

---

## 7. 核心：`core/features.*`、`core/typed.*`

出处：`core/features.errors` 见 [core-features](core-features.md)「错误与 panic」；`core/typed.*` 见 [design-typed-plugin-2026-10-02](design-typed-plugin-2026-10-02.md)「依赖在门控后丢失」。

| 编号 | 内容 | 级别 | 状态 | 验证 | 说明 |
| --- | --- | --- | --- | --- | --- |
| `core/features.errors` | 丢弃等待 `restart` / `update` 的 future 不会停止已开始的清理：清理照常结束，插件进入下一代 | 核心承诺 | 已验证 | `rutis/tests/lifecycle_order.rs::a_restart_dropped_after_its_cleanup_started_still_completes`、`::an_update_dropped_after_its_cleanup_started_still_completes`、`::a_restart_dropped_before_its_first_poll_starts_nothing`、`::an_update_dropped_before_its_first_poll_changes_nothing` | 后两个测试还锁定了"第一次 poll 之前丢弃什么都不做"（`restart` / `update` 的 future 是惰性的）。这一点没有写进文档，属于实现细节（Q3.3）；以后改成调用时登记的话，这两个测试随之改 |
| `core/typed.lost-dependency-pending` | 门控通过后、读取前必需依赖失效（被摘除、提供者失活或 `check` 拒绝）时，插件回到 Pending 而不是 Failed，依赖恢复后重新装载；这一代没有失败的严格读取时按普通失败处理（防循环） | 核心承诺 | 已验证 | `rutis/tests/typed_plugin.rs::a_dependency_lost_before_it_is_read_returns_the_plugin_to_pending`、`::a_dependency_back_before_the_failure_is_handled_still_reloads`、`::a_dependency_withdrawn_before_apply_starts_returns_the_plugin_to_pending`、`::a_check_rejected_after_the_gate_returns_the_plugin_to_pending_even_if_it_recovers`、`::the_claim_without_a_failed_read_is_a_plain_failure` | 依赖门控（Q3.1） |
| `core/typed.lost-dependency-silent` | 这种情况下这一代的 `InjectUnsatisfied` 不进 ErrorSink | 契约行为 | 已验证 | `rutis/tests/typed_plugin.rs::a_lost_dependency_error_does_not_reach_the_error_sink` | |
| `core/typed.lost-dependency-restart` | 落在这个窗口里的 `restart()` 返回 Ok，插件回到 Pending 或重新装载 | 契约行为 | 已验证 | `rutis/tests/typed_plugin.rs::a_restart_in_the_lost_dependency_window_returns_ok` | |
| `core/typed.get-not-evidence` | 非 typed 插件用 `get` 读不到后返回 `InjectUnsatisfied`：不算证据，按普通失败进 Failed | 契约行为 | 已验证 | `rutis/tests/typed_plugin.rs::a_get_read_is_not_evidence_of_a_lost_dependency` | |
| `core/typed.untyped-require` | 非 typed 插件用 `require` 读取失败后返回同一错误：回到 Pending，新提供者出现后装载 | 契约行为 | 已验证 | `rutis/tests/typed_plugin.rs::an_untyped_plugin_whose_require_fails_returns_to_pending` | |

---

## 8. Cordis 插件的边界规则：`boundary/5.1`–`boundary/5.8`

出处：[requirements-protocol-plugins](requirements-protocol-plugins.md) §5。

| 编号 | 内容 | 级别 | 状态 | 验证 | 说明 |
| --- | --- | --- | --- | --- | --- |
| `boundary/5.1` | 服务：`ctx.provide` / 属性赋值发布和换值；直接 `ctx.set` 换值不产生通知，rutis 侧在下一次调用该服务后切换到新对象 | 契约行为 | 已验证 | `rutis-bridge/tests/service_projection.rs::projected_service_follows_replacement_and_withdrawal`、`rutis-bridge/tests/projection_lifecycle.rs::a_throwing_method_still_refreshes_the_slot`、`::a_call_on_an_exported_object_refreshes_the_slots`、`::withdrawal_and_immediate_reregistration_keep_the_service` | |
| `boundary/5.2` | 跨边界事件：`emit` 只是通知，返回时不保证 rutis 侧已执行；需要等待或结果时用 `parallel` / `serial`（同步 `bail` 见 5.4） | 契约行为 | 已验证 | `rutis-bridge/tests/event_forwarding.rs::cordis_events_reach_rutis_listeners`、`::rutis_events_reach_cordis_listeners`、`rutis-bridge/tests/boundary_rules.rs::a_cordis_serial_chain_takes_the_rutis_side_as_one_link`、`::a_cordis_bail_across_the_boundary_fails` | `emit`、`parallel` 与 Cordis 侧 `serial` 已验证；同步 `bail` 明确报错（#281） |
| `boundary/5.3` | 事件顺序：只在同一侧内保证原生顺序；另一侧的监听作为一组在固定位置执行，组内总是按该侧的 `parallel` 调度；不支持依赖跨侧交错顺序 | 契约行为 | 已验证 | `rutis-bridge/tests/boundary_rules.rs::cordis_events_reach_rutis_listeners_as_one_group`、`::rutis_events_reach_cordis_listeners_as_one_group`、`::a_rutis_serial_runs_the_cordis_group_as_a_cordis_parallel` | rutis `serial` 到达 Cordis 时 Cordis 一组按 `parallel` 执行，写进规则 3（#281）；按发起方的调度方式执行需要在协议里带上调度方式，见 #340 |
| `boundary/5.4` | 中间件链：waterfall / bail 链不跨侧交错；对方一组作为一个整体环节参与；Cordis 同步 `bail` 到达转发的事件时报错；waterfall 中 rutis 一组是调用一次 `next` 的异步环节 | 契约行为 | 已验证 | `rutis-bridge/tests/boundary_rules.rs::a_cordis_serial_chain_takes_the_rutis_side_as_one_link`、`::a_rutis_serial_chain_takes_the_cordis_side_as_one_link`、`::a_cordis_bail_across_the_boundary_fails`、`::a_cordis_waterfall_takes_the_rutis_side_as_one_async_link`、`::an_async_cordis_waterfall_across_the_boundary_behaves_natively` | 全同步的 waterfall 跨边界后返回 Promise（值与原生相同），写进规则 4（#281）；同步 `bail` 之前的监听器以相同参数再次分发同一事件时，这次 `bail` 仍拿到 Promise 而不报错，写进规则 2，见 #340 |
| `boundary/5.5` | 框架内部事件 `internal/*` 不跨边界 | 契约行为 | 已验证 | `rutis-bridge/tests/boundary_rules.rs::internal_events_stay_on_the_cordis_side`、`::a_mount_listing_an_internal_event_fails`、`node/rutis-runtime/test/generate.test.mjs`（`internal events are not forwarded in either direction`） | 绑定生成器、`Process::mount` 与 Node 运行时都拒绝列出 `internal/*` 的挂载（#282） |
| `boundary/5.6` | 同步等待：同步方法在执行中等待需要 Node 事件循环推进的结果（定时器、Promise）时返回 `SyncWaitCycle` 错误，不卡死 | 核心承诺 | 已验证 | `rutis-bridge/tests/rpc_callbacks.rs::node_sync_wait_pumps_callbacks_and_reports_its_executor_cycle`（夹具 `node/rutis-runtime/test/fixtures/rpc-client.mjs` 断言 `SyncWaitCycle`）、`::sync_await_reports_known_executor_cycle_but_allows_ready_future`、`::sync_wait_detects_an_async_callback_already_running_on_its_runtime` | 跨边界做不到的事显式报错（Q3.1） |
| `boundary/5.7` | 宿主提供的服务在 Cordis 侧是按接口生成的代理：按接口调用与原生一致，`instanceof` 不成立 | 契约行为 | 已验证 | `rutis-bridge/tests/boundary_rules.rs::a_host_service_is_a_proxy_not_an_instance_of_the_declared_class` | |
| `boundary/5.8` | 事件循环：插件不得长期阻塞；兼容层不为调用加超时，rutis 侧需要超时时用异步方法并自行设置（超时即取消），卸载的等待时间由应用决定；未捕获的异常 / 拒绝结束整个 Node 进程，同一挂载的服务随之撤销 | 核心承诺 | 部分验证 | `rutis-bridge/tests/projection_lifecycle.rs::a_crashed_process_withdraws_its_services`、`rutis-bridge/tests/bun_runtime.rs::an_uncaught_error_ends_the_runtime`、`tests/e2e/tests/runtime_crash.rs::a_killed_runtime_fails_its_calls_and_a_restart_recovers` | 进程退出后撤销服务已验证（Node 用 `process.exit`；Python 运行时被 `kill -9` 时经 rutis-host 二进制验证，S3）；Node 上未捕获异常结束进程只在 Bun 运行时验证；"不加超时"没有断言 |

---

## 9. 实例：`instance/*`

出处：[design-instance-subtrees](design-instance-subtrees.md)。

| 编号 | 内容 | 级别 | 状态 | 验证 | 说明 |
| --- | --- | --- | --- | --- | --- |
| `instance/subtrees.2.2` | 实例 ID 在创建插件时分配，进程内唯一；同一插件 restart / update / 依赖重载不改变它，重新建立的子插件得到新 ID；耗尽时以明确原因 panic，不回绕；保留的旧 `Ctx` 仍报告旧 ID，旧 ID 读不到新插件的服务 | 契约行为 | 已验证 | `rutis/src/key.rs::instance_id_exhaustion_panics_instead_of_wrapping`、`rutis/tests/instance_subtrees.rs::a_recreated_child_gets_a_new_instance_id`、`::fiber_instance_stays_stable_across_restart_update_and_dependency_reload` | 耗尽的 panic 发生在持有 root 的准入锁时，会让这把锁中毒（u64 实际不可达，只记录；见 #286） |
| `instance/subtrees.5` | 长寿 root 上反复创建和关闭子树（包括旁边有常驻兄弟）之后不留残余：插件被释放，内部表项、任务数回到基线，父级表的容量不随轮数增长，常驻兄弟不重载 | 核心承诺 | 已验证 | `rutis/tests/instance_subtrees.rs::thousand_subtrees_next_to_a_resident_leave_no_tasks_behind`、`::thousand_subtrees_leave_no_live_fibers`、`rutis/src/fiber/transient_tests.rs::parent_tables_shrink_back_after_every_batch_of_subtrees`、`::thousand_shutdowns_reclaim_all_root_side_records`、`rutis/tests/transient_release.rs::churn_on_long_lived_root_releases_instances` | 结束后不留残余（Q3.1）。单个键下的消费者 / 监听器列表保留峰值容量（不随轮数增长，不违反 §4.6；见 #286） |

---

## 10. 会话与链路：`bridge/*`

出处见各行。"运行时"指各语言的会话实现：Rust（`rutis-bridge`）、Node、Python、Bun、Go。各运行时覆盖到哪里写在"说明"里；运行时矩阵是否完整由[质量执行现状](quality-status.md)跟踪（§3）。

| 编号 | 出处 | 内容 | 级别 | 状态 | 验证 | 说明 |
| --- | --- | --- | --- | --- | --- | --- |
| `bridge/in-flight-call-fails-once` | [design-remote-plugins](design-remote-plugins-2026-10-03.md) §7；[requirements-protocol-plugins](requirements-protocol-plugins.md) §6 | 链路断开时，进行中的调用以传输错误显式失败（不报成功、不挂死）；对端只执行一次，重连后不重发 | 核心承诺 | 已验证 | `rutis-bridge/tests/fault_links.rs::a_call_in_flight_when_the_link_drops_fails_and_ran_once`、`tests/e2e/tests/runtime_crash.rs::a_killed_runtime_fails_its_calls_and_a_restart_recovers` | 中断时调用显式失败、不报假成功、不自动重试副作用（Q3.1）；本机运行时进程被 `kill -9`、会话随之结束时，经 rutis-host 二进制验证（S3） |
| `bridge/no-call-on-a-dead-session` | [design-remote-plugins](design-remote-plugins-2026-10-03.md) §3.2、§7 | 链路断开和重连期间发起的调用要么显式失败，要么落在新会话上，不会落在旧会话；断开期间不提供 `Peer` 服务，旧会话的引用一直失败 | 核心承诺 | 已验证 | `rutis-bridge/tests/fault_links.rs::calls_while_a_link_reconnects_fail_or_reach_the_new_session` | |
| `bridge/cancel-reaches-callee` | [design-remote-plugins](design-remote-plugins-2026-10-03.md) §6.2；[插件 API](guide/plugin-api.md)「取消」 | 调用方放弃等待时，取消到达被调方；结果被释放时释放也到达，被调方丢弃 future | 契约行为 | 已验证 | `rutis-bridge/tests/fault_links.rs::under_delay_a_cancellation_and_a_release_reach_the_callee`、`rutis-bridge/tests/cancellation.rs` | Node 被调方收到 `AbortSignal` 由 `cancellation.rs` 覆盖 |
| `bridge/multihop-cancel-and-release` | [design-remote-plugins](design-remote-plugins-2026-10-03.md) §6.2；质量规范 Q6.2.7 | 多跳转发（b → a → c）时取消和释放逐跳传递 | 契约行为 | 已验证 | `rutis-bridge/tests/fault_links.rs::cancellation_and_release_pass_hop_by_hop` | |
| `bridge/call-chains-across-processes` | [会话标签设计](design-session-tags-2026-10-12.md) §五；[多语言 M1 设计](design-multilang-m1-2026-10-04.md) §7.2；质量规范 Q6.13.2 | 调用链经 link 跨任意多个进程、往返任意次时，每个进程只把自己会话的条目还原为原生调用号；另一个进程的调用不会被当成本进程的调用（不因此结束会话，不投错等待线程，不进入本进程的等待图） | 契约行为 | 已验证 | `rutis-bridge/tests/session_tags.rs::a_call_chain_crossing_processes_keeps_each_process_s_calls_apart`、`tests/e2e/tests/migration.rs::an_implementation_moves_from_python_to_a_node_to_rust_while_the_host_runs` | #325。前者是两个子进程、确定性；后者经 rutis-host 与 fixture 宿主（S4）。标签唯一性是概率性的（进程随机数相等的概率约 10⁻¹⁵）；两个 0.9 及更早的进程之间仍有 #325 |
| `bridge/replies-match-calls` | [design-remote-plugins](design-remote-plugins-2026-10-03.md) §6.2 | 多个调用同时进行、以任意顺序返回时，每个回复交给它自己的调用 | 契约行为 | 已验证 | `rutis-bridge/tests/fault_links.rs::interleaved_replies_reach_their_own_calls` | 放行顺序按种子打乱（`RUTIS_SEED`） |
| `bridge/large-and-deep-values` | [design-remote-plugins](design-remote-plugins-2026-10-03.md) §7（消息大小上限）；[插件 API](guide/plugin-api.md)「值怎样传递」 | 接近消息上限的值和嵌套到上限（64 层）的值完整往返；超过消息上限的调用显式失败，对端不执行它；嵌套超过 64 层的值由要发送它的一方在发出前拒绝，只让这次调用失败（错误说明上限），会话和同一会话上的其他调用继续；不遵守上限的对端发来过深（线上超过 144 层）的帧时，只让它所属的调用失败，看不出所属调用时结束会话并说明原因。上限约束的是发送：从不遵守上限的对端收到的值可能超过 64 层（Rust 一侧读到线上 144 层，为余量，不保证），不承诺收到的值都在 64 层以内 | 契约行为 | 已验证 | `rutis-bridge/tests/fault_links.rs::large_and_deep_values_cross_a_session_whole`、`::over_websocket_a_value_near_the_limit_passes_and_one_over_it_fails`、`::a_value_too_deep_fails_its_call_and_the_session_goes_on`；`rutis-bridge/tests/value_roundtrip.rs::rust_depth_limit`、`::node_depth_limit`、`::python_depth_limit`、`::go_depth_limit`、`::bun_depth_limit`；`rutis-bridge/src/session/rpc/tests.rs::a_value_deeper_than_the_limit_fails_its_call_before_it_is_sent`、`::a_frame_too_deep_fails_its_call_and_the_session_goes_on`、`::the_deepest_frames_allowed_go_the_receive_path_on_the_readers_stack` | #279。5 个运行时都作为过深值的发送方验证过（参数和返回值；Node、Bun、Python、Go 经各自的 conformance 夹具 `wrap`、`applyWrapped`）；接收过深的帧只有 Rust 一侧实现（各语言运行时只与 Rust 宿主建立会话） |
| `bridge/value-roundtrip` | [插件 API](guide/plugin-api.md)「值怎样传递」 | null、布尔、数字、字符串、数组、对象跨会话按值转换表往返，`bridge/value-limits` 列出的除外 | 契约行为 | 已验证 | `rutis-bridge/tests/value_roundtrip.rs::rust_values_round_trip`、`::node_values_round_trip`、`::python_values_round_trip`、`::go_values_round_trip`、`::bun_values_round_trip` | 属性测试，失败时打印 `RUTIS_PROPTEST_SEED`。5 个运行时都覆盖；Go 列测的是会话层（回显 `json.RawMessage`） |
| `bridge/value-limits` | [插件 API](guide/plugin-api.md)「值怎样传递」 | **明确不保证**：JS 一侧超出 ±(2^53−1) 的整数、`-0`、整数值浮点数；NaN、±Infinity | 明确不保证 | 明确不保证 | `rutis-bridge/tests/value_roundtrip.rs` 的比较规则按这些边界放宽 | 测试只放宽、不锁定"确实不提供"。嵌套深度原先在本行（#279），已修复并移到 `bridge/large-and-deep-values`：上限 64 层，过深只让这次调用失败 |
| `bridge/release-count` | [design-remote-plugins](design-remote-plugins-2026-10-03.md) §6.2；质量规范 Q6.2.6 | 任意授予、释放、转回序列之后，两端引用表只含未释放的引用，全部释放后回到起点 | 契约行为 | 已验证 | `rutis-bridge/tests/value_roundtrip.rs::rust_releases_balance`、`::node_releases_balance`、`::python_releases_balance` | Rust、Node、Python；Go、Bun 的夹具没有观察引用表的入口，未覆盖 |
| `bridge/reference-release-count` | 质量规范 Q6.2.6 | 引用的释放按计数：已收下的调用不受之后的释放影响；旧的释放不扣新授予；全部释放后再导出得到新 id；重复导入的引用显式释放时按次数计 | 契约行为 | 已验证 | `python/rutis/tests/test_peer.py::ReferenceCountTests`（`test_an_admitted_callback_survives_a_following_counted_release`、`test_an_old_release_preserves_a_new_grant_and_identifiers_are_not_reused`、`test_an_explicit_release_accounts_repeated_live_imports`）；`node/rutis-runtime/test/session.test.mjs`（`admitted callback survives a following counted release before dispatch`、`old release preserves a new grant and identifiers are not reused`、`explicit release accounts repeated live imports without relying on GC`） | Node、Python；Rust 侧由 `bridge/release-count` 覆盖；Go、Bun 没有对应测试 |
| `bridge/grant-rollback` | [插件 API](guide/plugin-api.md)「值怎样传递」（不能跨进程的值） | 参数里有不能跨进程的值时，调用显式失败，已做的引用授予回滚，什么都不发出 | 核心承诺 | 已验证 | `python/rutis/tests/test_peer.py::ReferenceCountTests::test_an_unsupported_nested_value_fails_explicitly_and_rolls_back_partial_grants`；`node/rutis-runtime/test/session.test.mjs`（`unsupported nested references fail explicitly and roll back partial grants`） | 跨边界做不到的事显式报错、不静默降级（Q3.1）。Node、Python；Rust、Go、Bun 没有对应测试 |
| `bridge/version-refusal` | [design-remote-plugins](design-remote-plugins-2026-10-03.md) §6.1；质量规范 Q6.2.4 | 会话协议版本不同时，握手阶段显式拒绝，说明哪一端需要升级；各运行时的协议常量与 Rust 一致 | 契约行为 | 已验证 | `rutis-bridge/src/session/rpc/tests.rs::endpoint_format::a_far_end_of_another_protocol_version_is_told_which_end_to_upgrade`；`python/rutis/tests/test_peer.py::VersionMismatchTests`；`node/rutis-runtime/test/session.test.mjs`（`a far end of another protocol version is refused, saying which end to upgrade`）；`bun/rutis-bun/test/session.test.ts`（同名）；`go/rutis/internal/peer/peer_test.go::TestAFarEndOfAnotherProtocolVersionIsRefusedSayingWhichEndToUpgrade`；WebSocket 子协议拒绝：`rutis-bridge/tests/websocket.rs::authentication_routing_and_compatibility_failures_carry_their_category`、`node/rutis-runtime/test/websocket.test.mjs`、`python/rutis/tests/test_websocket.py::WebSocketTest::test_a_protocol_refusal_says_which_end_to_upgrade`、`go/rutis/listen_test.go::TestAProtocolRefusalSaysWhichEndToUpgrade`；`rutis-bridge/tests/sdk_protocol.rs`（4 个测试，Python、Node、Bun、Go 各一） | 五种实现都按两个方向断言：对端更新时提示升级本端并写出本端的包（crate、npm 包、PyPI 包或 Go 模块），对端更旧时提示升级对端，握手里带了 `implementation` 时写出它的名字和版本 |
| `bridge/websocket-refusal-categories` | [design-remote-plugins](design-remote-plugins-2026-10-03.md) §3.2 | WebSocket 建立连接时的拒绝按结构化类别报告：错 token、无 token、CA 不受信任为 `AuthRejected`，子协议版本不符为 `Incompatible` | 契约行为 | 已验证 | `rutis-bridge/tests/websocket_cross.rs::rust_dialing_python_is_refused_by_category`、`::rust_dialing_node_is_refused_by_category`、`::node_dialing_rust_is_refused_by_category` | Python 没有拨号端（只监听），所以没有"Python 拨 Rust"一列 |
| `bridge/tls-server-verified` | [design-remote-plugins](design-remote-plugins-2026-10-03.md) §7 | 拨号方验证服务端证书（受信任的 CA、有效期、名字）；任一不符则拒绝连接、归为 `AuthRejected`，token 不发送、不出现在原因里 | 契约行为 | 已验证 | `rutis-bridge/tests/websocket.rs::wss_verifies_the_server_and_accepts_client_certificates`、`::wss_refuses_an_expired_or_not_yet_valid_server_certificate`、`::wss_refuses_a_server_certificate_for_another_name`；`python/rutis/tests/test_websocket.py::WssTest` | |
| `bridge/tls-client-certificate-verified` | [design-remote-plugins](design-remote-plugins-2026-10-03.md) §7 | 监听方验证客户端证书（CA、有效期）；无效证书不认证，即使指纹在接受列表里，拨号方得到 `AuthRejected` | 契约行为 | 已验证 | `rutis-bridge/tests/websocket.rs::wss_refuses_an_expired_client_certificate` | |
| `bridge/spawned-processes-own-channel` | 质量规范 Q6.5.2；#184 | 同一宿主同时启动的多个运行时进程各自只持有自己的通道；一个进程结束时它的通道随之结束，不被其他进程继承的描述符拖住 | 核心承诺 | 已验证 | `rutis-bridge/tests/concurrent_launch.rs::spawned_processes_started_at_once_hold_only_their_own_channel`、`::node_runtimes_launched_at_once_hold_only_their_own_channel` | 结束后不留残余（Q3.1）。只在 Unix 上测：Windows 不交接描述符 |

---

## 11. 持久化格式：`host/rutis-json-format`、`dsh/user-layer-format`

出处：[design-quality-step2-2026-10-11](design-quality-step2-2026-10-11.md) §七；用户文档 [rutis-host](guide/rutis-host.md) 的 `format` 一节；发布流程见 [release](release.md) 的"持久化样本"（#207，风险 J3）。

| 编号 | 内容 | 级别 | 状态 | 验证 | 说明 |
| --- | --- | --- | --- | --- | --- |
| `host/rutis-json-format` | `rutis.json` 顶层 `format` 是格式版本，可省略、省略按 1 读；比本 rutis-host 支持的新时，在读其他字段之前拒绝并说明升级 rutis-host；`rutis-host new` 生成的文件写上它；每个已发布版本的 `rutis.json`（0.8.0 起）仍按当时的含义读出 | 契约行为 | 已验证 | `rutis-host/tests/config.rs::the_format_is_checked`、`rutis-host/tests/errors.rs::a_configuration_that_is_not_there_or_not_valid`、`rutis-host/tests/persisted.rs::every_release_s_rutis_json_reads_as_it_did`、`::formats_no_longer_read_are_refused`、`rutis-host/src/new.rs::projects_are_created_with_their_names_filled_in` | 样本与期望在 `tests/fixtures/persisted/<版本>/`，0.8.0、0.9.0、0.9.1 的期望由各自 tag 的解析器生成 |
| `dsh/user-layer-format` | dsh 用户层 `<profile>/cordis.patch.yml`（Cordis 的配置格式）没有格式版本；每个已发布版本写下的用户层（0.8.0 起）仍读出同样的 patch | 契约行为 | 已验证 | `rutis-dsh/tests/persisted.rs::every_release_s_user_layer_reads_as_it_did` | 不加版本字段（Cordis 的配置没有版本管理），兼容只靠样本；读不出的旧格式没有显式拒绝 |

---

## 12. 插件 SDK：`sdk/*`

出处：[插件 API](guide/plugin-api.md)、质量规范 Q6.7。

| 编号 | 内容 | 级别 | 状态 | 验证 | 说明 |
| --- | --- | --- | --- | --- | --- |
| `sdk/testing-matches-runtime` | 测试工具（`@arcships/rutis/testing`、`rutis.testing`，严格模式）对插件的调用结果与真实运行时相同：同步、异步、回调、错误跨边界（只带名字和消息）、值的复制与拒绝；已知差异有界并写明（[插件 API](guide/plugin-api.md)「测试工具与真实运行时的差异」，期望文件中标为 `allowed`） | 契约行为 | 部分验证 | `rutis-bridge/src/runtime/testing.rs::runtime`（经 `rutis-bridge/tests/session_matrix.rs` 在 Node、Python、Go、Bun 各通道上运行）、`node/rutis/test/conformance.test.mjs`、`python/rutis/tests/test_conformance.py::Conformance::test_the_testing_tool_returns_what_the_real_runtime_does` | 两侧对同一份期望文件 `rutis-bridge/src/runtime/testing/weather.expected.json` 比对（Q6.7.2）。Go 的 `rutistest` 与 Bun 的测试工具尚未比对；Map、Set、Date、二进制、bigint 按现行规则拒绝，待 #308 |

---

## 13. 宿主的输出：`host/link-state-output`

出处：用户文档 [rutis-host](guide/rutis-host.md) 的「run 打印什么」、[连接节点](guide/nodes.md) 的重连一段（#321）；[rutis-host](guide/rutis-host.md) 的“输出”一节（#332）。

| 编号 | 内容 | 级别 | 状态 | 验证 | 说明 |
| --- | --- | --- | --- | --- | --- |
| `host/link-state-output` | `rutis-host run` 启动时为每个监听器打印一次实际绑定的地址（`rutis-host: listening on <名字> <地址>`）；节点行的 link 状态变化时各打印一行，与行状态行分开：`<行 id>: link connected`、`link down (<原因>); retrying in <时间>`、`link refused: auth-rejected (<原因>); retrying in 30s`、`link stopped: incompatible (<原因>)`；类别来自 link 对失败的分类（`Failure`），不靠错误文字；token 被拒、服务端证书名字不符或过期都报为 `auth-rejected`；输出里不出现 token | 契约行为 | 已验证 | `rutis-host/src/status.rs::tests::each_link_state_has_its_line`、`rutis-host/tests/links.rs::a_link_reports_connected_and_down`、`::an_unreachable_far_end_is_reported_as_down`、`::a_refused_token_is_reported_by_its_category_without_it`、`::an_incompatible_far_end_is_reported_as_stopped`；`tests/e2e/tests/nodes.rs::a_refused_reconnect_is_reported_by_its_category`、`::a_refused_certificate_is_reported_by_its_category` | 括号里的原因只给人看，措辞不保证；状态每 200 毫秒检查一次，只打印变化，持续更短的状态可能不打印 |
| `host/output-lines-whole` | `rutis-host` 运行的运行时进程（Node、Bun、Python、Go）写到 stdout、stderr 的每一行，原样、整行地出现在 `rutis-host` 的同一个流上：`rutis-host` 自己的行和其他运行时的行不会落进它中间，插件把一行分几次写也一样；运行时结束时没有换行的最后一行作为单独一行写出，在 `rutis-host` 退出之前 | 契约行为 | 已验证 | `rutis-host/tests/output.rs::plugin_lines_written_in_pieces_come_out_whole`、`rutis-bridge/src/transport/local/output.rs::tests::a_line_written_in_pieces_is_passed_on_whole`；`tests/e2e` 的每个场景（harness 遇到被拆开的探针行即失败） | 同一个运行时进程里的两个插件各写半行，仍会拼在一起（一个进程只有一份输出）。超过 64 KiB 没有换行的行分段写出。黑盒测试只在 Unix 上跑（用 SIGTERM 结束宿主） |

---

## 14. 统计（基准：`121eded` 之后合入 #284、#298、#334、#335 的 `main`，加上 #281、#282 的修复和 #321）

| 范围 | 已验证 | 部分验证 | 未验证 | 明确不保证 | 合计 |
| --- | --- | --- | --- | --- | --- |
| `core/D*`（不含 D20、D26、D27） | 26 | 3 | 0 | 1 | 30 |
| `core/features.*`、`core/typed.*` | 6 | 0 | 0 | 0 | 6 |
| `boundary/5.*` | 7 | 1 | 0 | 0 | 8 |
| `instance/*` | 2 | 0 | 0 | 0 | 2 |
| `bridge/*` | 16 | 0 | 0 | 1 | 17 |
| `host/*`、`dsh/*`（§11） | 2 | 0 | 0 | 0 | 2 |
| `host/*`（§13） | 2 | 0 | 0 | 0 | 2 |
| `sdk/*`（§12） | 0 | 1 | 0 | 0 | 1 |
| 合计 | 61 | 5 | 0 | 2 | 68 |

核心承诺 17 条：已验证 14，部分验证 3（D22、D28、5.8），未验证 0。契约行为 49 条：已验证 47，部分验证 2（D5、`sdk/testing-matches-runtime`），未验证 0。

**0.10 的基线**：未验证的核心承诺 0 条，未验证的契约行为 0 条。从 0.11 起，新登记的条目（`loader/`、`host/` 等）如果是"未验证"，会让契约行为的数字超过基线（Q9.4.4），所以新登记的行应当带着测试一起登记。核心承诺的划分由维护者确认，可以调整。
