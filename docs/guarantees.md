# rutis 保证登记表

[English](guarantees.en.md) · [质量规范](quality-standard.md) · [质量执行现状](quality-status.md)

状态：骨架（#181）。本表登记 rutis 的核心承诺与契约行为，每条一行：编号、出处、内容、级别、状态、对应的验证。规则依据是[质量规范](quality-standard.md) §3（承诺的分级）与 §9.4（Q9.4.1–Q9.4.4）。

本表**人工维护**：不从设计文档自动抽取，CI 也不核对表与测试是否一致（误报多，维护成本高于收益；条目超过 300 条时再考虑）。表的正确性靠 PR 作者和评审在改动时同步更新（§5）。

本 PR 登记了 `core/D1`–`core/D33` 与 `boundary/5.1`–`boundary/5.8`。其余编号族（§2）已预留，由后续 PR 补齐；全部补齐后关闭 #181。

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

| 族 | 编号形式 | 出处 | 本 PR |
| --- | --- | --- | --- |
| `core/` | `core/D1`–`core/D31` | [design-rust-port](design-rust-port.md) §三 决策表 | 已登记 |
| `core/` | `core/D32`、`core/D33` | [design-config-hot-update-and-dynamic-events-2026-09-21](design-config-hot-update-and-dynamic-events-2026-09-21.md) §一、§二 | 已登记 |
| `core/` | `core/features.<节>` | [core-features](core-features.md) 各节 | 预留 |
| `boundary/` | `boundary/5.1`–`boundary/5.8` | [requirements-protocol-plugins](requirements-protocol-plugins.md) §5 的 8 条边界规则 | 已登记 |
| `loader/` | `loader/<名字>` | rutis-loader 设计 | 预留 |
| `instance/` | `instance/<名字>` | 实例子树与实例服务设计 | 预留 |
| `bridge/` | `bridge/<名字>` | 会话协议、运行时、链路相关设计 | 预留 |
| `host/` | `host/<名字>` | rutis-host 设计与用户文档 | 预留 |
| `sdk/` | `sdk/<名字>` | 插件 SDK 与测试工具：[插件 API](guide/plugin-api.md)、质量规范 Q6.7 | `sdk/testing-matches-runtime` 已登记（#206） |

`D` 编号沿用设计文档里的决策编号（包括复述其他编号的 D20、D27 和已并入 D27 的 D26），方便与设计文档、测试注释对照。新族的 `<名字>` 用短横线连接的英文小写短语（如 `loader/rollback`）。

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

### 现有标记

本表建立之前，仓库里已有 3 处 `guarantee:` 标记，它们写的是 issue 和风险编号，还不是本表的编号：

| 位置 | 现在写的 | 将来的编号 |
| --- | --- | --- |
| `rutis-bridge/tests/concurrent_launch.rs::spawned_processes_started_at_once_hold_only_their_own_channel` | `issue #184 (risk C11, Q6.5.2)` | `bridge/` 族登记时分配 |
| `rutis-bridge/tests/concurrent_launch.rs::node_runtimes_launched_at_once_hold_only_their_own_channel` | 同上 | 同上 |
| `rutis-host/tests/signals.rs`（文件级） | 清理恰好一次、信号结束宿主时运行时进程已退出（Q5.4.4、Q6.6.3） | `host/` 族登记时分配；清理恰好一次一项同时对应 `core/D6` |

本 PR 不改测试代码；登记 `bridge/`、`host/` 族的 PR 同时把这些标记改成本表的编号。

---

## 5. 什么时候更新本表

- 新增或修改设计文档、用户文档中的保证：同一个 PR 登记或修改对应行（级别必须写明，Q3 的要求）。
- 新增验证某条保证的测试：在测试里写 `guarantee: <编号>`，同一个 PR 把测试加到"验证"列，必要时修改状态。
- 删除、跳过（`#[ignore]`、平台条件跳过）或削弱某个列出的测试：同一个 PR 修改"验证"列与状态（Q9.4.3）。
- 测试改名或移动文件：同步修改"验证"列。

PR 模板中有对应的检查项。

---

## 6. 核心：`core/D1`–`core/D33`

出处：D1–D31 见 [design-rust-port](design-rust-port.md) §三；D32、D33 见 [design-config-hot-update-and-dynamic-events-2026-09-21](design-config-hot-update-and-dynamic-events-2026-09-21.md) §一、§二。基准 `main` `2e77fdc`。

核心承诺的内容写可观察的行为；实现机制（锁、任务、三元组等）留在设计文档（Q3.3）。D20、D26、D27 是其他编号的复述或已并入，编号保留、不单独计数。

| 编号 | 内容 | 级别 | 状态 | 验证 | 说明 |
| --- | --- | --- | --- | --- | --- |
| `core/D1` | 需要 dyn 的 trait 统一用手写别名 `BoxFuture<'a, T>`，不用 async-trait 宏 | 契约行为 | 已验证 | 编译期：所有实现 `Plugin` / `Listener` 的测试（如 `rutis/tests/contract.rs`） | 类型层面的约定，无行为测试 |
| `core/D2` | 事件载荷类型化（`Event` trait），内部以 `Arc<dyn Any + Send + Sync>` 擦除；`serde_json::Value` 只用在序列化边界，不进核心 | 契约行为 | 已验证 | `rutis/tests/contract.rs::emit_async_safe`、`rutis/tests/typed_dispatch.rs::typed_identity_and_prefix_scope_are_distinct`；`crates/rutis/Cargo.toml` 无 serde 依赖 | "不进核心"只由依赖清单体现，没有自动检查 |
| `core/D3` | 回调注册表式总线（钩子语义），监听器为 owned `Arc<dyn Listener>`，不引入 channel | 契约行为 | 已验证 | `rutis/tests/contract.rs::plugin_registers_n_listeners`、`::prepend_order`、`::once_once` | |
| `core/D4` | 四种分发：`emit` fire-and-forget（Err / panic 进 ErrorSink）；`parallel` 等全部并聚合错误；`serial` 按序至短路；`waterfall` 续延式，不调 `next` 即否决 | 契约行为 | 已验证 | `rutis/tests/contract.rs::emit_async_safe`、`::emit_error_sink`、`::parallel_aggregates`、`::parallel_waits_all`、`::serial_bails`、`::waterfall_veto_around` | |
| `core/D5` | fiber 状态在单锁域内修改；锁内只做 FIFO 入队，绝不持锁跨 await，回调在锁外执行 | 契约行为 | 部分验证 | `rutis/tests/contract.rs::mixed_concurrent_ops_complete`、`rutis/tests/interleave.rs::restart_settle_and_dispose_interleave`、`::dependency_churn_interleaves_with_restart`；CI clippy `-D warnings` 含 `await_holding_lock` | "持锁跨 await"由 clippy 静态检查（只覆盖 std 锁）；锁内 FIFO 入队的顺序没有直接断言（见 D24） |
| `core/D6` | 卸载时插件注册的清理恰好执行一次：并发的 dispose 等待同一次卸载并得到同一个错误；同一代不重复执行，重启后的新一代重新开始 | 核心承诺 | 已验证 | `rutis/tests/contract.rs::concurrent_dispose_join`、`::exactly_once_same_error`、`::dispose_idempotent`、`::cross_generation_isolation`、`rutis/tests/parity.rs::returns_one_disposal_promise_and_joins_cleanup_already_in_progress` | 机制（锁内单个 `Arc<TransitionTask>`）见设计文档；D20 是同一条 |
| `core/D7` | 协作取消：每代 fiber 独立的子 `CancellationToken`，`ctx.cancelled()` 暴露；卸载五步（标记 unloading → 取消当前代 → 等 apply 退出 → 清理 EffectRecord → 发布终态）；不观察 token 的插件会让 dispose 一直等（写明的限制） | 核心承诺 | 部分验证 | `rutis/tests/contract.rs::cancel_wakes_awaiters`、`::cancel_during_loading`、`::cancel_idempotent`、`::cancel_cascades`、`::disposal_deadline_reports_loading_and_later_joins` | 只断言了结果，五步的先后顺序没有断言（#177）；D27（及并入它的 D26）是同一条 |
| `core/D8` | runtime 接入：优先构造时注入 `Handle`（`Ctx::root_with`）；自动路径 `Handle::try_current()` 失败返回明确错误，绝不隐式创建 runtime | 契约行为 | 未验证 | — | 实现在 `rutis/src/ctx.rs`（`Ctx::root`）；没有测试在 runtime 之外调用 `Ctx::root()`（#177） |
| `core/D9` | 过期的上下文（旧一代）发起的操作被拒绝，判定不依赖调度顺序；**监听器完成顺序不作契约**（明确不保证） | 契约行为 | 已验证 | `rutis/tests/contract.rs::cross_generation_isolation`、`rutis/tests/generation_registration.rs::rejects_late_registration_current_thread`、`::rejects_late_registration_multi_thread`、`::rejects_registration_after_failed_apply`、`rutis/tests/host_control.rs::a_stale_context_cannot_dispose_the_next_generation` | 按保证部分定级。`yield_now` 只是公平性提示，属实现。"完成顺序不固定"没有测试锁定 |
| `core/D10` | 插件身份 = `FiberView.id: PluginId`，显示名为 `Plugin::name()`；`is_unique` 式去重可选 | 契约行为 | 已验证 | `rutis/tests/host_control.rs::view_finds_a_fiber_by_id` | `is_unique` 未实现（设计写明可选） |
| `core/D11` | 错误类型 `CordisError`：thiserror 派生（thiserror 是普通依赖），`Error + Send + Sync + 'static`，**不实现 Clone**，以 `Arc` 共享，聚合为 `Vec` | 契约行为 | 部分验证 | 编译期：`Send + Sync + 'static` 由跨任务传递 `Arc<CordisError>` 隐含 | 只有编译期的间接保证；"不实现 Clone"没有断言（#177 的 PR 加 compile_fail 文档测试） |
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
| `core/D24` | `FiberStatusChanged`：锁内 FIFO 入队、锁外分发；保证提交顺序（generation 作 sequence），不保证监听器完成顺序 | 契约行为 | 部分验证 | `rutis/tests/contract.rs::state_transitions` | 测试先按 `seq` 排序再断言，掩盖了投递顺序（#177） |
| `core/D25` | 错误分层：身份归 `TransitionTask`（缓存 `Arc<CordisError>`），包装归 `PluginFailed`（`#[source]`），不递归 | 契约行为 | 已验证 | `rutis/tests/contract.rs::exactly_once_same_error`、`rutis/src/error.rs::plugin_failure_display_and_sink_preserve_each_cause_once`、`::sink_recurses_into_aggregate_used_as_plugin_failure_source`、`rutis/tests/parity.rs::keeps_plugin_execution_failure_separate_from_rollback_cleanup_failure` | |
| `core/D26` | 并入 D27（即 D7） | — | — | — | 编号保留，不单独计数 |
| `core/D27` | 同 D7 | — | — | — | 设计文档写"见 D7"；编号保留，不单独计数 |
| `core/D28` | 资源所有权：经 `Ctx` 注册的资源自动归 fiber；`Disposer` 只用于提前释放；isolate 出的 `Ctx` 保留原 fiber 的所有权 | 核心承诺 | 部分验证 | `rutis/tests/contract.rs::auto_ownership`、`::effect_yields_disposer`、`rutis/tests/event_keys.rs::keyed_listener_removed_with_owner_fiber` | "isolate 出的 `Ctx` 保留原 fiber 所有权"没有测试（插件经 isolate 注册、卸载后资源被回收） |
| `core/D29` | 事件跨 isolate 不过滤；isolate 只隔离服务注册表；需要作用域事件时用户自建子总线 | 契约行为 | 未验证 | — | 没有回归测试锁定"不过滤"（#177） |
| `core/D30` | 插件代码失败或 panic 不挂死、不越界：apply 失败或 panic 时该插件进入 Failed 并回滚已注册的资源，等待者收到显式错误；监听器 panic 进 ErrorSink；清理 panic 被捕获并包装为 `PluginFailed` | 核心承诺 | 部分验证 | `rutis/tests/contract.rs::apply_panics_complete_and_roll_back_current_thread`、`::apply_panics_complete_and_roll_back_multi_thread`、`::driver_panic_completes_waiters_with_error`、`::apply_failure_rolls_back`、`::emit_error_sink`、`::cleanup_panics_contained`、`::serial_panic_contained`、`rutis/tests/parity.rs::keeps_plugin_execution_failure_separate_from_rollback_cleanup_failure` | 对应 Q3.1 的"失败时回滚"与"中断时显式失败"。异步 waterfall 回调 panic 传给调用者只测了同步版（#177） |
| `core/D31` | `emit` 派发序：同一事件类型按发射顺序串行派发；panic 不断链；监听器内重入 emit 同类型排到链尾不死锁；**跨事件类型不保证顺序**；监听器完成顺序仍不作契约 | 契约行为 | 部分验证 | `rutis/tests/dispatch_chain_probe.rs::dispatch_chain_does_not_fork_under_concurrent_emit`、`rutis/tests/event_keys.rs::keyed_emit_preserves_emission_order_per_name`、`rutis-agent/tests/order_probe.rs::emit_delivers_in_order_back_to_back_multi_thread` | 只有概率性探针与单发射者顺序；多发射者各自顺序、panic 后下一次仍投递、重入 emit 排队尾没有测试。`rutis-agent` 是示例项目，不在测试范围内；#177 的第 1 个 PR 在 `crates/rutis` 内加确定性的 D31 测试 |
| `core/D32` | 配置热更新：运行中 `update(config)`；先 dry-run（`validate_config` → `build` → 实例 `validate`），失败返回错误、不存不重启，旧状态继续工作；成功后按状态矩阵重启生效；config 不进依赖身份；**不提供 `internal/update` waterfall，持久化归宿主层**（D32d，明确不保证） | 核心承诺 | 已验证 | `rutis/tests/config_update.rs` 全部 15 个测试，如 `::update_active_applies_new_config_and_cleans_once`、`::update_dry_run_failure_leaves_state_untouched`、`::update_during_loading_converges_to_new_config`、`::concurrent_update_and_dispose_settle_correctly` | 丢弃 `update` 的 future 未测（#177） |
| `core/D33` | 动态事件键：`TypeKey` 限定名接受运行时字符串；事件总线 keyed 面，四种分发、once / prepend、随 fiber 清理全部继承；同名不同类型、不同名之间互不串；**事件仍不按 isolate 过滤**（D33d，同 D29） | 契约行为 | 已验证 | `rutis/tests/event_keys.rs` 全部 11 个测试，如 `::keyed_channels_are_isolated_by_name`、`::runtime_constructed_name_dispatches`、`::same_name_different_event_types_do_not_cross` | D33d 的验证状态见 D29 |

---

## 7. Cordis 插件的边界规则：`boundary/5.1`–`boundary/5.8`

出处：[requirements-protocol-plugins](requirements-protocol-plugins.md) §5。

| 编号 | 内容 | 级别 | 状态 | 验证 | 说明 |
| --- | --- | --- | --- | --- | --- |
| `boundary/5.1` | 服务：`ctx.provide` / 属性赋值发布和换值；直接 `ctx.set` 换值不产生通知，rutis 侧在下一次调用该服务后切换到新对象 | 契约行为 | 已验证 | `rutis-bridge/tests/service_projection.rs::projected_service_follows_replacement_and_withdrawal`、`rutis-bridge/tests/projection_lifecycle.rs::a_throwing_method_still_refreshes_the_slot`、`::a_call_on_an_exported_object_refreshes_the_slots`、`::withdrawal_and_immediate_reregistration_keep_the_service` | |
| `boundary/5.2` | 跨边界事件：`emit` 只是通知，返回时不保证 rutis 侧已执行；需要等待或结果时用 `parallel` / `serial` / `bail` | 契约行为 | 部分验证 | `rutis-bridge/tests/event_forwarding.rs::cordis_events_reach_rutis_listeners`、`::rutis_events_reach_cordis_listeners` | 只验证了 `emit` 与 `parallel`；`serial` / `bail` 跨边界没有测试 |
| `boundary/5.3` | 事件顺序：只在同一侧内保证原生顺序；另一侧的监听作为一组在固定位置执行；不支持依赖跨侧交错顺序 | 契约行为 | 未验证 | — | #178 |
| `boundary/5.4` | 中间件链：waterfall / bail 链不跨侧交错；对方一组作为一个整体环节参与 | 契约行为 | 未验证 | — | #178 |
| `boundary/5.5` | 框架内部事件 `internal/*` 不跨边界 | 契约行为 | 未验证 | — | #178 |
| `boundary/5.6` | 同步等待：同步方法在执行中等待需要 Node 事件循环推进的结果（定时器、Promise）时返回 `SyncWaitCycle` 错误，不卡死 | 核心承诺 | 已验证 | `rutis-bridge/tests/rpc_callbacks.rs::node_sync_wait_pumps_callbacks_and_reports_its_executor_cycle`（夹具 `node/rutis-runtime/test/fixtures/rpc-client.mjs` 断言 `SyncWaitCycle`）、`::sync_await_reports_known_executor_cycle_but_allows_ready_future`、`::sync_wait_detects_an_async_callback_already_running_on_its_runtime` | 跨边界做不到的事显式报错（Q3.1） |
| `boundary/5.7` | 宿主提供的服务在 Cordis 侧是按接口生成的代理：按接口调用与原生一致，`instanceof` 不成立 | 契约行为 | 未验证 | — | #178 |
| `boundary/5.8` | 事件循环：插件不得长期阻塞；兼容层不为调用加超时，rutis 侧需要超时时用异步方法并自行设置（超时即取消），卸载的等待时间由应用决定；未捕获的异常 / 拒绝结束整个 Node 进程，同一挂载的服务随之撤销 | 核心承诺 | 部分验证 | `rutis-bridge/tests/projection_lifecycle.rs::a_crashed_process_withdraws_its_services`、`rutis-bridge/tests/bun_runtime.rs::an_uncaught_error_ends_the_runtime` | 进程退出后撤销服务已验证（Node 用 `process.exit`）；Node 上未捕获异常结束进程只在 Bun 运行时验证；"不加超时"没有断言 |

---

## 8. 插件 SDK：`sdk/*`

出处：[插件 API](guide/plugin-api.md)、质量规范 Q6.7。

| 编号 | 内容 | 级别 | 状态 | 验证 | 说明 |
| --- | --- | --- | --- | --- | --- |
| `sdk/testing-matches-runtime` | 测试工具（`@arcships/rutis/testing`、`rutis.testing`，严格模式）对插件的调用结果与真实运行时相同：同步、异步、回调、错误跨边界（只带名字和消息）、值的复制与拒绝；已知差异有界并写明（[插件 API](guide/plugin-api.md)「测试工具与真实运行时的差异」，期望文件中标为 `allowed`） | 契约行为 | 部分验证 | `rutis-bridge/src/runtime/testing.rs::runtime`（经 `rutis-bridge/tests/session_matrix.rs` 在 Node、Python、Go、Bun 各通道上运行）、`node/rutis/test/conformance.test.mjs`、`python/rutis/tests/test_conformance.py::Conformance::test_the_testing_tool_returns_what_the_real_runtime_does` | 两侧对同一份期望文件 `rutis-bridge/src/runtime/testing/weather.expected.json` 比对（Q6.7.2）。Go 的 `rutistest` 与 Bun 的测试工具尚未比对；Map、Set、Date、二进制、bigint 按现行规则拒绝，待 #308 |

---

## 9. 统计（基准 `2e77fdc`，另加 `sdk/*`）

| 范围 | 已验证 | 部分验证 | 未验证 | 明确不保证 | 合计 |
| --- | --- | --- | --- | --- | --- |
| `core/D*`（不含 D20、D26、D27） | 19 | 8 | 2 | 1 | 30 |
| `boundary/5.*` | 2 | 2 | 4 | 0 | 8 |
| `sdk/*`（#206） | 0 | 1 | 0 | 0 | 1 |
| 合计 | 21 | 11 | 6 | 1 | 39 |

其中核心承诺 10 条：已验证 5（D6、D14、D21、D32、5.6），部分验证 5（D7、D22、D28、D30、5.8），未验证 0。未验证的契约行为 6 条（D8、D29、5.3、5.4、5.5、5.7），作为 0.10 的基线。核心承诺的划分由维护者确认，可以调整。
