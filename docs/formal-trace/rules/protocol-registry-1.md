# 规则：协议清单 registry-1（#634 评论）

[中文索引](../README.md) · [English](../README.en.md) · 来源：[评论 6384932214](https://github.com/arcships/rutis/issues/634#issuecomment-6384932214) · [保证登记表](../../guarantees.md)

本文件把 #634 的 `registry-1` 分区评论（[评论 6384932214](https://github.com/arcships/rutis/issues/634#issuecomment-6384932214)）里的 Markdown 表格导出到仓库。表格内容原样保留；单元格里的 `|` 按 Markdown 表格规则转义为 `\|`，渲染后文字不变。列的含义见 #634 正文：编号、来源、规则、参与方、手段、对应、现有形式化、状态、归属 issue、备注。

抽取基线：`main` `3496669b`（issue 正文的统计口径）。**这批行没有按 0.22 的现行规格复核**：状态列仍是 issue 的词汇，`node tools/formal-trace.mjs` 在统计时按 `README.md` §3 的映射换成追踪表状态；映射只降不升，不改写本文件。

2026-11-16（#1212）：`events/flat b`、`events/link-export-import b` 的备注按 0.19 删除的机制改写。节点的 `export` / `import` 名单在 0.19 删除（`design-remote-session-2026-10-28.md` §八、§十二；`migration-0.19.md`），节点之间是一条总线、每个名字都在；备注里「#631 export / import 收窄 PT」没有对象，按[复核表](../recheck-0.22.md)记「已撤回」。状态列不改。

分区合计：现行 156 条，已取代 9 条，共 165 行（与 issue 正文的统计表一致）。

## 分区 `registry-1`

> 分区 `registry-1`：保证登记表 §6–§8a（core、boundary、events 宿主侧）
>
> 现行 156 条，已取代 9 条（列出、不计入）。"归属 issue"一列带 ※ 的是这次新分配或调整的。列的含义见 #634 正文。

| 编号 | 来源 | 规则 | 参与方 | 手段 | 对应 | 现有形式化 | 状态 | 归属 issue | 备注 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `core/D1` | §6 | 需要 dyn 的 trait 统一用手写 `BoxFuture<'a, T>`，不用 async-trait | 纯逻辑 | 测试 | — | — | 不适用 | — | 编译期约定，类型系统即检查；形式化没有对象 |
| `core/D2a` | §6 | 事件载荷类型化（`Event` trait，serde），内部以 `Arc<dyn Any + Send + Sync>` 擦除 | 纯逻辑 | 测试 | EVP-12.1-1 | — | 不适用 | — | 编译期（trait 约束） |
| `core/D2b` | §6 | 事件能到达另一个类型的监听器与别的进程，只在那时经总线的转码器（serde）转换 | 纯逻辑 | PT + K | EVP-12.1-1、EVP-12.1-2 | K：`rust/typed-args`、`rust/unit-event` | 部分 | #631 | 缺 PT（任意嵌套 serde 类型往返、参数个数）；K 只有用例里的两种形状 |
| `core/D3a` | §6 | 回调注册表式总线，监听器为 owned `Arc<dyn Listener>`，不引入 channel | 纯逻辑 | 测试 | — | — | 不适用 | — | 实现形状（类型层面） |
| `core/D3b` | §6 | 插件可登记 n 个监听器；`prepend` 排在前；`once` 只调用一次（钩子语义） | 纯逻辑 | PT | EVP-4.3-1 | — | 未覆盖 | #631 | 监听器表 PT：插入 / 移除 / prepend / 前缀序列对照有序表 |
| `core/D4a` | §6 | `emit` 立即返回，失败进 ErrorSink | 进程内并发 | PT + K | EVP-5.4-1、EVP-5.6-1 | K：`emit/does-not-wait`、`emit/error-isolated` | 部分 | #631 | 细则同 `events/emit` |
| `core/D4b` | §6 | `parallel` 等全部完成，失败汇总为 `ListenersFailed`，按登记顺序 | 纯逻辑 | PT + K | EVP-5.4-2 | K：`parallel/errors-in-registration-order`、`parallel/one-error-is-aggregated` | 部分 | #631 | 同 `events/parallel` |
| `core/D4c` | §6 | `serial` 按序调用到第一个结果为止 | 纯逻辑 | PT + K | EVP-5.4-3 | K：`serial/first-result`、`serial/no-result` | 部分 | #631 | 同 `events/serial-bail` a |
| `core/D4d` | §6 | `waterfall` 续延式，不调 `next` 即否决 | 纯逻辑 | PT + K | EVP-5.4-4 | K：`waterfall/chain`、`waterfall/veto` | 部分 | #631 | 同 `events/waterfall` |
| `core/D4e` | §6 | 同步方式 `bail_sync` / `waterfall_sync` | 纯逻辑 | PT + K | EVP-5.4-5、EVP-5.4-6 | K：`bail/first-result`、`waterfall-sync/chain` | 部分 | #631 |  |
| `core/D5a` | §6 | fiber 状态在单锁域内修改；锁内只做 FIFO 入队，回调在锁外执行 | 进程内并发 | SH | — | SH：`shuttle_post_join_completes_across_driver_exit`（投递跨驱动退出时 join 一定返回；只查会结束） | 部分 | #622 | 入队的 FIFO 顺序、"回调在锁外"都没有模型 |
| `core/D5b` | §6 | 绝不持锁跨 await | 进程内并发 | 测试 | — | — | 不适用 | — | 编码规则，CI clippy `await_holding_lock`（`-D warnings`）静态检查；内核的锁都是 std 的（`crate::sync` 普通构建即 std） |
| `core/D6a` | §6 | 卸载时清理恰好执行一次；并发的 dispose 等同一次卸载并得到同一个错误 | 进程内并发 | SH | — | SH：`shuttle_concurrent_dispose_cleans_up_exactly_once`、`shuttle_transition_task_reaches_every_join` | 部分 | #619、#622 | 两个模型各只覆盖一个部件（EffectRecord 的并发排干；单完成方的 TransitionTask）。fiber 层的 dispose 登记（`register_dispose` / `terminal_task` 与 `load()`）不在模型里，Bug A（#619）让 dispose 卡住、清理一次也不执行；登记表"已验证"与 #619 矛盾。2026-10-26：#619 的修复在 #652（0.17.1），回归测试 `load_window.rs::a_dispose_right_after_mounting_ends_on_*`、`a_dispose_during_a_restart_ends_on_*`、`a_restart_queued_before_a_dispose_leaves_the_cleanup_to_it_on_*`、`a_refresh_queued_before_a_dispose_leaves_the_cleanup_to_it_on_*`、`a_dispose_registered_in_the_gate_starts_no_generation`、`two_restarts_queued_before_a_dispose_both_report_it_on_*`、`a_restart_behind_a_refresh_that_disposes_reports_it_on_*`（测试，不算形式化）；SH 仍归 #622 |
| `core/D6b` | §6 | 同一代的清理不重复执行；重启后的新一代重新开始 | 进程内并发 | PT + SH | — | — | 未覆盖 | #622 | fiber 生命周期参考模型 PT（代次、清理次数） |
| `core/D7a` | §6 | 每代有自己的取消令牌；卸载时发布 Unloading、取消当前代、等 apply 退出，三步都先于清理；清理 LIFO；全部清理结束后才发布 Disposed | 进程内并发 | PT + SH | — | — | 未覆盖 | #622 | #622 的参考模型只比较最终状态、代次、apply 与清理次数，不比较这几步的先后，需在模型里记事件序列 |
| `core/D7b` | §6 | dispose、restart、update、驱逐在调用时就取消 | 进程内并发 | SH | — | — | 未覆盖 | #619、#622 | Bug A：dispose 只取消当前令牌，`load()`（`fiber.rs` 约 642–658 行）之后新建的令牌没被取消；登记表"已验证"与之矛盾。2026-10-26：Bug A 由 #652（0.17.1）修复；同类窗口 restart / update 在锁外预取消也一并修复（挂上后立刻调用时取消落在初始令牌上）。回归测试 `load_window.rs::a_dispose_right_after_mounting_ends_on_*`、`a_dispose_during_a_restart_ends_on_*`、`a_restart_right_after_mounting_is_taken_up_on_*`、`an_update_right_after_mounting_is_taken_up_on_*`、`a_restart_queued_before_a_dispose_leaves_the_cleanup_to_it_on_*`、`a_dispose_registered_in_the_gate_starts_no_generation`、`two_restarts_queued_before_a_dispose_both_report_it_on_*`、`a_restart_behind_a_refresh_that_disposes_reports_it_on_*`（测试，不算形式化）；SH 仍归 #622 |
| `core/D7c` | §6 | 不观察令牌的插件让 dispose 一直等（写明的限制） | 进程内并发 | 测试 | — | — | 不适用 | — | 限制，不是保证 |
| `core/D8` | §6 | 优先构造时注入 `Handle`（`Ctx::root_with`）；`Handle::try_current()` 失败返回明确错误，绝不隐式创建 runtime | 外部 | 测试 | — | — | 不适用 | — | 依赖 tokio 运行时环境；`ctx.rs` 单元测试已有 |
| `core/D9a` | §6 | 过期的上下文（旧一代）发起的操作被拒绝，判定不依赖调度顺序 | 进程内并发 | SH | — | — | 未覆盖 | #622 | "登记准入与卸载"模型（已提交的登记恰好清理一次，被拒的不留记录） |
| `core/D9b` | §6 | 监听器完成顺序不作契约（明确不保证） | 进程内并发 | 测试 | — | — | 不适用 | #641 ※ | 明确不保证；"完成顺序不固定"没有测试锁定（登记表） |
| `core/D10` | §6 | 插件身份 = `FiberView.id: PluginId`，显示名 `Plugin::name()`；`is_unique` 去重可选（未实现） | 纯逻辑 | 测试 | — | — | 不适用 | — | API 形状 |
| `core/D11` | §6 | `CordisError`：thiserror 派生，`Error + Send + Sync + 'static`，不实现 Clone，以 Arc 共享，聚合为 Vec | 纯逻辑 | 测试 | — | — | 不适用 | — | 编译期（doctest `compile_fail,E0277`） |
| `core/D12` | §6 | `Plugin::validate` 校验实例自持的配置，通过才存储；不引入 schema 库 | 纯逻辑 | PT | — | — | 未覆盖 | #622 | 参考模型 PT 的更新与失败注入 |
| `core/D13` | §6 | 服务定位器 `get::<T>() -> Option<Arc<T>>`：类型键，以 None 表示失败，沿父链回溯 | 纯逻辑 | PT | — | — | 未覆盖 | #631 | "查找、作用域与严格读取的判定表" |
| `core/D14a` | §6 | 提供方撤销服务时，依赖它的消费者先停下（回到 Pending），提供方最后清理 | 进程内并发 | SH | — | — | 未覆盖 | #620、#622 | Bug B（#620）：消费者过了门、还没写 `last_deps` 时服务被撤下，消费者不被停；登记表"已验证"与之矛盾。"消费者并发排干"也没有断言。2026-10-26：Bug B 由 #652（0.17.1）修复，绑定在过门与记下依赖之间被替换的情况一并修复。回归测试 `load_window.rs::a_dependency_withdrawn_at_the_gate_of_a_first_load_stops_the_consumer`、`…_of_a_retry_stops_the_consumer`、`a_dependency_replaced_at_the_gate_is_not_used_on_the_old_record`、`a_dependency_withdrawn_before_it_is_recorded_does_not_run_the_generation`、`a_refresh_queued_before_a_dispose_leaves_the_cleanup_to_it_on_*`（测试，不算形式化）；SH 仍归 #622 |
| `core/D14b` | §6 | 清理期间提供方仍能访问自身服务 | 进程内并发 | PT | — | — | 未覆盖 | #622 |  |
| `core/D14c` | §6 | 只驱逐依赖被撤销服务的消费者 | 纯逻辑 | PT | — | — | 未覆盖 | #622、#631 | 参考模型 PT；服务注册槽 PT |
| `core/D14d` | §6 | 服务恢复后消费者重新启动 | 进程内并发 | PT + SH | — | — | 未覆盖 | #622 |  |
| `core/D15` | §6 | tower 兼容只作可选适配层，不进核心；生命周期钩子不套 `Service` / `Layer`（明确不保证） | 纯逻辑 | 测试 | — | — | 不适用 | — | 明确不保证；现在也没有适配层 |
| `core/D16a` | §6 | 事件系统异步，另有同步 `bail_sync` / `waterfall_sync`；监听器 trait `call<'a>`、`Call` / `SyncCall`、`Event::Value`；`emit` / `parallel` / `serial` 载荷为 `Arc<E>` | 纯逻辑 | 测试 | EVP-12.1 | — | 不适用 | — | API 形状，编译期 |
| `core/D16b` | §6 | 一个监听器被每种方式调用 | 纯逻辑 | PT + K | EVP-12.1、EVP-5.4-8 | K：`kinds/sync-listener-every-mode` | 部分 | #631 | 同 `events/one-table` a |
| `core/D17` | §6 | waterfall 没有单独注册面；监听器经 `Call` / `SyncCall` 拿 `next`，只在 waterfall 中可用；终态由调用方提供，可省略 | 纯逻辑 | PT + K | EVP-5.4-4、EVP-5.4-7、EVP-11-3 | K：`rust/next-outside-waterfall`、`rust/sync-next-in-async-waterfall`、`waterfall/terminal`、`waterfall/end-without-terminal` | 部分 | #631、#632 | #632 用 Miri 查同步 `next` 的生命周期 transmute（`bus/run.rs` 约 494 行、`events/remote.rs`） |
| `core/D18a` | §6 | `Effect` 的两个 Disposer 变体返回 `Result<(), CordisError>`；闭包捕获 owned Ctx | 纯逻辑 | 测试 | — | — | 不适用 | — | 编译期 |
| `core/D18b` | §6 | 清理错误聚合不展平；重启的清理错误进 sink；未取用的提前清理错误以同一身份到达卸载 | 进程内并发 | SH + PT | — | SH：`shuttle_concurrent_dispose_cleans_up_exactly_once`（并发排干的每个调用方拿到同一个 Arc） | 部分 | #622 | 模型不含所属 fiber（`finish()` 自移除、错误存放与 `drain_effects` 的 `mem::take` 竞争，#622 第 3 项）；"聚合不展平"没有 PT |
| `core/D19` | §6 | 配置烘进实例：`Plugin` 无关联类型，`new(config)` 自持，注册表只存 `Arc<dyn Plugin>` | 纯逻辑 | 测试 | — | — | 不适用 | — | 编译期 |
| `core/D20` | §6 | 同 D6 | — | — | — | — | 已取代 | — | 登记表：复述 D6，编号保留、不单独计数 |
| `core/D21a` | §6 | 驱逐精确到提供方的那一次注册：只驱逐依赖被撤销的那个提供方（同插件、同代、同服务键）的消费者 | 进程内并发 | SH + PT | — | — | 未覆盖 | #622、#631 | "依赖驱逐"SH 模型；服务注册槽 PT |
| `core/D21b` | §6 | `isolate(key, label)` 按服务键隔离作用域；同 label 共享；子作用域找不到时回退父作用域 | 纯逻辑 | PT | — | — | 未覆盖 | #631 | 查找与作用域判定表 |
| `core/D21c` | §6 | 使用方只依赖服务键，不依赖由哪个插件提供（不匹配的键从不满足注入） | 纯逻辑 | PT | — | — | 未覆盖 | #631 |  |
| `core/D22a` | §6 | 依赖未就绪的插件保持 Pending，不启动、不报错，依赖到位后启动 | 进程内并发 | PT + SH | — | — | 未覆盖 | #622 |  |
| `core/D22b` | §6 | 不检测依赖环（明确不保证） | 纯逻辑 | 测试 | — | — | 不适用 | #641 ※ | 登记表说没有回归测试锁定（#177），但 #177 已关闭，没有已开 issue 计划补 |
| `core/D23a` | §6 | `Ctx::effect()` 注册清理并交回 Disposer；清理期间、卸载中登记 effect 失败 | 进程内并发 | SH | — | — | 未覆盖 | #622 | "登记准入与卸载"模型 |
| `core/D23b` | §6 | 增量式（async-iterable）effect 刻意不提供 | 纯逻辑 | 测试 | — | — | 不适用 | — | 明确不提供 |
| `core/D24a` | §6 | `FiberStatusChanged` 按提交顺序投递：每个插件的 seq 连续，每条的 from 是上一条的 to | 进程内并发 | SH | — | — | 未覆盖 | #622 | #622 的 emit 链模型只查发出方的开始顺序；状态入队与 `flush_status`（`fiber.rs:533`：锁内取走队列、锁外逐条 emit，顺序依赖"只有驱动调用它"）没有列入 |
| `core/D24b` | §6 | 不保证监听器完成顺序 | 进程内并发 | 测试 | — | — | 不适用 | — | 明确不保证 |
| `core/D25a` | §6 | 错误身份归 TransitionTask（缓存 `Arc<CordisError>`）：每个等待者拿到同一个 Arc | 进程内并发 | SH | — | SH：`shuttle_transition_task_reaches_every_join` | 部分 | #622 | 只有一个完成方；#622 的疑点"`complete` 后写覆盖先写"要加多个完成方 |
| `core/D25b` | §6 | 包装归 `PluginFailed`（`#[source]`），不递归：显示与 sink 中每个原因只出现一次 | 纯逻辑 | PT | — | — | 未覆盖 | #631 ※ | 随机嵌套错误树对照参考实现；#631 的内核清单没有这一项 |
| `core/D26` | §6 | 并入 D27 | — | — | — | — | 已取代 | — | 登记表：编号保留、不单独计数 |
| `core/D27` | §6 | 同 D7 | — | — | — | — | 已取代 | — | 登记表：复述 D7，编号保留、不单独计数 |
| `core/D28a` | §6 | 经 Ctx 注册的资源自动归 fiber；Disposer 只用于提前释放 | 进程内并发 | SH | — | SH：`shuttle_concurrent_dispose_cleans_up_exactly_once`（提前释放与排干并发时恰好一次） | 部分 | #622 | 模型不含所属 fiber（#622 第 3 项） |
| `core/D28b` | §6 | isolate 出的 Ctx 保留原 fiber 的所有权（经它注册的资源随原 fiber 卸载回收） | 进程内并发 | PT | — | — | 未覆盖 | #631 ※ | 登记表：没有测试；#622 参考模型的操作里没有 isolate |
| `core/D29a` | §6 | 事件跨 isolate 不过滤；isolate 只隔离服务注册表 | 纯逻辑 | PT | EVP-3-2 | — | 未覆盖 | #631 ※ | K 的作用域是加载器的组实例（`mark_scope`），不是 `isolate`；#631 的分发参考解释器没有列 isolate |
| `core/D29b` | §6 | 事件按名字送达（平总线，`events/flat`） | 纯逻辑 | PT + K | EVP-3-2 | K：`scope/flat-bus`、`scope/siblings-share-the-bus`、`scope/bail-and-waterfall` | 部分 | #631 |  |
| `core/D30a` | §6 | apply 失败或 panic：插件进入 Failed，回滚已注册的资源，等待者收到显式错误 | 进程内并发 | PT + SH | — | — | 未覆盖 | #622 | 参考模型 PT 含失败与 panic 注入 |
| `core/D30b` | §6 | 监听器 panic 进 ErrorSink（`emit`）或作为错误交给发出方（其他方式） | 纯逻辑 | PT + K | EVP-5.6-1 | K：`emit/error-isolated`、`emit/error-keeps-process`（监听器抛错；Rust 参与者不 panic） | 部分 | #631 | 五种方式参考解释器含 panic |
| `core/D30c` | §6 | 清理 panic 被捕获并包装为 `PluginFailed` | 纯逻辑 | PT | — | — | 未覆盖 | #622 |  |
| `core/D30d` | §6 | waterfall 回调的 panic 以 `PluginFailed` 错误交给调用方 | 纯逻辑 | PT | EVP-5.4-4 | — | 未覆盖 | #631 |  |
| `core/D30e` | §6 | 运行时进程被杀时它的调用显式失败，重启后恢复 | 跨进程 | TLA + 测试 | — | — | 未覆盖 | #626 | 登记表把 e2e `runtime_crash` 列在 D30 下；调用生命周期模型的 `SettlesOnce`；进程被杀本身是外部行为 |
| `core/D31a` | §6 | 同一名字的多次 emit 在每个监听器上按发出顺序开始（多个发射者各自的顺序都保持） | 进程内并发 | SH | EVP-5.5-1、EVP-5.5-2 | — | 未覆盖 | #622 | emit 链模型（2 个发出方 × 2 次 emit × 2 个监听器）；与 EVP-5.5-1 的"每个（发出的进程，名字）"口径不同（问题 3） |
| `core/D31b` | §6 | 监听器 panic 不断链 | 进程内并发 | SH | — | — | 未覆盖 | #622 |  |
| `core/D31c` | §6 | 监听器内再 emit 同名排到后面，不死锁 | 进程内并发 | SH | — | — | 未覆盖 | #622 | emit 链模型的"外加监听器里再 emit" |
| `core/D31d` | §6 | 跨名字不保证顺序；调用并发执行，完成顺序不作契约 | 进程内并发 | 测试 | — | — | 不适用 | — | 明确不保证 |
| `core/D32a` | §6 | 运行中 `update(config)` 先 dry-run（`validate_config` → `build` → 实例 `validate`），失败返回错误、不存不重启、旧状态继续工作 | 纯逻辑 | PT | — | — | 未覆盖 | #622 | 参考模型 PT 的更新与失败注入 |
| `core/D32b` | §6 | dry-run 成功后按状态矩阵重启生效（含 Loading 中、与 dispose 并发） | 进程内并发 | SH + PT | — | SH：`shuttle_post_join_completes_across_driver_exit`（重启意图跨驱动退出时 join 返回；只查会结束） | 部分 | #622 | #622 疑点：restart / update 在检查与投递之间输给终止的 Dispose 时被排干成 Ok |
| `core/D32c` | §6 | config 不进依赖身份 | 纯逻辑 | PT | — | — | 未覆盖 | #622 |  |
| `core/D32d` | §6 | 不提供 `internal/update` waterfall，持久化归宿主层（明确不保证） | 纯逻辑 | 测试 | — | — | 不适用 | — | 明确不保证 |
| `core/D33a` | §6 | `EventKey::named` / `dynamic` 接受运行时字符串；一张表按名字查找；五种方式、once / prepend、随 fiber 清理全部继承；不同名字之间不串 | 纯逻辑 | PT + K | EVP-12.1-1 | K：全部用例（宿主参与者以 `EventKey::<Args>::dynamic` 登记，`events_conformance/rust_participant.rs:341`） | 部分 | #631 |  |
| `core/D33b` | §6 | 同名不同类型的监听器会串，经 serde 转换 | 纯逻辑 | PT | EVP-12.1-1 | — | 未覆盖 | #631 | 同 `events/by-name` a |
| `core/D33c` | §6 | 事件仍不按 isolate 过滤（D33d） | 纯逻辑 | PT | EVP-3-2 | — | 未覆盖 | #631 ※ | 同 `core/D29a` |
| `core/features.shutdown-state a` | §7 | `Ctx::is_shutting_down()` 从调用 `shutdown()` 起、清理期间及关闭完成后都为真，与 `diagnostics().shutting_down` 一致 | 进程内并发 | SH | — | — | 未覆盖 | #622 ※ | #622 的根关闭模型只查 join 的结果，不查这个标志 |
| `core/features.shutdown-state b` | §7 | 子上下文和隔离上下文共享该状态；仅卸载或关闭子插件不将它置为真 | 纯逻辑 | PT | — | — | 未覆盖 | #631 ※ |  |
| `core/features.errors` | §7 | 丢弃等待 `restart` / `update` 的 future 不停止已开始的清理：清理照常结束，插件进入下一代 | 进程内并发 | SH | — | — | 未覆盖 | #622 ※ | 取消安全；#622 的模型没有"在 await 处丢弃 future"的动作（#629 的做法只用于加载器）。"第一次 poll 前丢弃什么都不做"是实现细节（Q3.3），不列 |
| `core/features.errors.failed-retry` | §7 | 装载失败的插件停在 Failed，只在依赖变化时重新装载；同组依赖的重复通知不让它重试 | 进程内并发 | PT + SH | — | — | 未覆盖 | #622 | 参考模型 PT（失败注入 + 提供 / 撤下）；逐键通知的交错要 SH |
| `core/typed.lost-dependency-pending a` | §7 | 门控通过后、读取前必需依赖失效（摘除、失活、`check` 拒绝）时插件回到 Pending 而非 Failed，依赖恢复后重新装载 | 进程内并发 | SH + PT | — | — | 未覆盖 | #620、#622 | 现有测试 `a_dependency_withdrawn_before_apply_starts_returns_the_plugin_to_pending` 依赖 Bug B 的窗口，#620 修复后要改写。2026-10-26：已在 #652（0.17.1）按 #620 改写（validate 只等到撤下已标记）；SH + PT 仍归 #622 |
| `core/typed.lost-dependency-pending b` | §7 | 这一代没有失败的严格读取时按普通失败处理（防循环） | 纯逻辑 | PT | — | — | 未覆盖 | #631 | 严格读取判定表 |
| `core/typed.lost-dependency-silent` | §7 | 上述情况下这一代的 `InjectUnsatisfied` 不进 ErrorSink | 纯逻辑 | PT | — | — | 未覆盖 | #631 |  |
| `core/typed.lost-dependency-restart` | §7 | 落在这个窗口里的 `restart()` 返回 Ok，插件回到 Pending 或重新装载 | 进程内并发 | SH | — | — | 未覆盖 | #622 ※ | #622 的"依赖驱逐"模型不含 restart |
| `core/typed.get-not-evidence` | §7 | 非 typed 插件用 `get` 读不到后返回 `InjectUnsatisfied`：不算证据，按普通失败进 Failed | 纯逻辑 | PT | — | — | 未覆盖 | #631 |  |
| `core/typed.untyped-require` | §7 | 非 typed 插件 `require` 失败后返回同一错误或原样返回（`?`）：回到 Pending，新提供者出现后装载 | 纯逻辑 | PT | — | — | 未覆盖 | #631 |  |
| `events/one-table a` | §7 | 一个监听器登记一次（`on`、`on_sync`、前缀条目、别处条目），发出方选择的任何方式都调用它；`call.mode()` 说明方式 | 纯逻辑 | PT + K | EVP-12.1、EVP-5.4-8 | K：`kinds/sync-listener-every-mode` | 部分 | #631 |  |
| `events/one-table b` | §7 | `next` 只在 waterfall 中可用，否则 `WaterfallNext`；同步监听器在异步 waterfall 中也是 | 纯逻辑 | PT + K | EVP-5.4-7、EVP-11-3 | K：`rust/next-outside-waterfall`、`rust/sync-next-in-async-waterfall` | 部分 | #631 |  |
| `events/emit a` | §7 | `emit` 立即返回；每个监听器的调用按登记顺序开始，不等任何一个 | 进程内并发 | PT + K | EVP-5.4-1 | K：`emit/does-not-wait`、`emit/reaches-all` | 部分 | #631 |  |
| `events/emit b` | §7 | 同一名字的多次 emit 按发出顺序开始 | 进程内并发 | SH + K | EVP-5.5-1、EVP-5.5-2 | K：`emit/order-per-listener`、`emit/order-filtered-then-unfiltered` | 部分 | #622 | 内核一侧由 emit 链 SH 检查；K 走经会话的路径；跨进程的 `EmitOrder` 在 #613（问题 3） |
| `events/emit c` | §7 | 监听器失败报给 ErrorSink（`ListenerFailed`），从不到达发出方，也不中断其他监听器 | 纯逻辑 | PT + K | EVP-5.6-1 | K：`emit/error-isolated`、`emit/error-keeps-process` | 部分 | #631 |  |
| `events/parallel` | §7 | `parallel` 按登记顺序开始每个调用并等全部完成；有失败得 `ListenersFailed`（AggregateError），子错误按登记顺序，单个失败也如此 | 纯逻辑 | PT + K | EVP-5.4-2、EVP-11-1 | K：`parallel/all-ok`、`parallel/errors-in-registration-order`、`parallel/one-error-is-aggregated` | 部分 | #631 |  |
| `events/serial-bail a` | §7 | `serial` / `bail_sync` 逐个调用，返回第一个结果；`None` 与 `Some(false)`（`null` / `false`）是没有结果；错误停止 | 纯逻辑 | PT + K | EVP-1（没有结果）、EVP-5.4-3、EVP-5.4-5、EVP-12.1-5 | K：`serial/first-result`、`serial/no-result`、`serial/zero-is-a-result`、`serial/error-stops`、`bail/first-result`、`bail/no-result`、`bail/error-stops`、`rust/some-false-is-no-result` | 部分 | #631 |  |
| `events/serial-bail b` | §7 | 同步分发遇到异步监听器以 `AsyncListener` 失败 | 纯逻辑 | PT + K | EVP-5.4-8、EVP-11-2 | K：`bail/async-listener`、`bail/awaitable-from-plain-function`、`rust/bail-async-listener`、`waterfall-sync/async-listener` | 部分 | #631 |  |
| `events/serial-bail c` | §7 | 同名的同步分发可以重入；只有真正的等待环以 `SyncWaitCycle` 失败 | 跨进程 | TLA + K | EVP-5.4-9、EVP-11-5 | TLA：CrossRuntimeSync 的不死锁与 `NoFalseCycle`（policy-b-* 配置；只含同步服务调用与 emit 投递，没有 `bail` 分发）；K：`bail/wait-cycle`、`bail/call-back-into-waiting-emitter` | 部分 | #614 | #614 把 `bail` / `waterfall-sync` 建成宿主上的一次同步执行；同进程的重入属纯逻辑，由 `typed_dispatch.rs` 的测试覆盖 |
| `events/waterfall` | §7 | 经 `next` 由外向内；不调即否决其余；最后一个 `next` 调用终态（可省略，没有时 None）；没有监听器时为终态的值；原样返回最外层的值 | 纯逻辑 | PT + K | EVP-5.4-4、EVP-5.4-6 | K：`waterfall/chain`、`waterfall/veto`、`waterfall/terminal`、`waterfall/no-listeners`、`waterfall/end-without-terminal`、`waterfall/false-stays-false`、`waterfall/terminal-gets-args`、`waterfall/terminal-gets-args-no-listeners`、`waterfall-sync/chain` | 部分 | #631 |  |
| `events/by-name a` | §7 | 监听器按名字查找（`EventKey::of` 为 `E::NAME`）；同名的另一个 Rust 类型的监听器收到事件，结果经 serde 以发出方的类型返回 | 纯逻辑 | PT | EVP-12.1-1 | — | 未覆盖 | #631 | K 的宿主参与者只用 `Args`，碰不到两种 Rust 类型 |
| `events/by-name b` | §7 | 参数按事件的顶层形状对应（2 个及以上字段的元组 = 每字段一个，unit = 没有，其余 = 一个） | 纯逻辑 | PT + K | EVP-12.1-2 | K：`rust/typed-args`、`rust/unit-event` | 部分 | #631 | 事件参数编解码 PT |
| `events/by-name c` | §7 | `EventKey::<Args>::of()` 以 `BindingError` 失败（确切信息） | 纯逻辑 | 测试 | EVP-12.1-3 | — | 不适用 | — | 确切文字 |
| `events/order a` | §7 | 精确条目与前缀条目按登记先后；`prepend` 排在全部现有条目之前 | 纯逻辑 | PT + K + TLA | EVP-4.3-1、EVP-4.3-2 | K：`order/prepend`、`order/registration`、`order/update-registers-again` | 部分 | #613、#631 | #613 的 `OneOrder` 管跨进程的先后一致 |
| `events/order b` | §7 | `once` 只被调用一次，在调用开始时认领：被过滤掉或被 `serial` / `bail` 提前结束而没有调用的不算 | 进程内并发 | PT + SH | — | — | 未覆盖 | #631 | 并发分发同时认领 once 的交错，#622 没有列 |
| `events/reserved a` | §7 | 插件以任何方式发出 `rutis/*` 都以 `ReservedEvent` 失败 | 纯逻辑 | K | EVP-8-1、EVP-11-4 | K：`reserved/rutis-emit`、`reserved/every-mode`（发出方 vary，含 host） | 已覆盖 | — |  |
| `events/reserved b` | §7 | `rutis/*` 可以监听 | 纯逻辑 | K | EVP-8-1、EVP-4.2-2 | K：`rutis/row`、`rutis/service`、`rutis/runtime-down-reason`（host 上的监听器） | 已覆盖 | — |  |
| `events/flat a` | §7 | 事件在一个 root 的总线上按名字送达所有监听器；作用域或运行时描述不挑选监听器；`ListenerInfo::scope` 只是过滤用的描述 | 纯逻辑 | PT + K | EVP-3-2、EVP-12.1-6 | K：`scope/flat-bus`、`scope/siblings-share-the-bus`、`scope/bail-and-waterfall`、`filter/own-listeners-in-scopes` | 部分 | #631 |  |
| `events/flat b` | §7 | 相连的节点只见到它的连接 `export` / `import` 承载的名字 | 跨进程 | PT + K | EVP-3-2、EVP-9-2 | K：`node/not-shared`、`node/shared-both-ways` | 部分 | #631 | 已撤回（#1212）：0.19 删除节点的 `export` / `import` 名单（`design-remote-session-2026-10-28.md` §八、§十二；`migration-0.19.md`），节点之间是一条总线、每个名字都在；#631 的 export / import 收窄 PT 没有对象，`node/not-shared` 已删；按[复核表](../recheck-0.22.md)记「已撤回」 |
| `events/flat c` | §7 | 没有总线的挂载（`Mount.ctx: None`）则没有跨进程事件 | 纯逻辑 | 测试 | — | — | 不适用 | #641 ※ | 登记表"验证"列的 4 个测试都不碰这一支 |
| `events/scope` | §7 | 监听器收到自己作用域及其下各作用域的事件，收不到兄弟的；根收到全部 | — | — | — | — | 已取代 | — | 登记表：已撤回（0.15.1），由 `events/flat` 取代（#604、#605） |
| `events/filter a` | §7 | 过滤在按名字取出的快照上、任何调用之前，按登记顺序看每个非 `global` 候选的 `ListenerInfo` | 纯逻辑 | PT + K | EVP-5.1-2、EVP-6-1、EVP-6-4 | K：`filter/keeps-and-sees`、`filter/every-listener`、`filter/global-skips-filter`、`filter/global-skips-filter-in-scopes`、`filter/bail` | 部分 | #631 |  |
| `events/filter b` | §7 | 过滤失败时整次分发在任何调用前失败（`emit` 中报告为 `FilterFailed`，不送达任何监听器） | 纯逻辑 | PT + K | EVP-6-3、EVP-5.6-3 | K：`filter/throws`、`filter/throws-in-emit` | 部分 | #631 |  |
| `events/lifecycle a` | §7 | fiber 开始卸载后不再开始它的监听器的新调用 | 进程内并发 | SH + K | EVP-4.4-4、EVP-7.1-2 | K：`unload/not-called-after`（经会话） | 部分 | #622 | 事件调用的在途计数模型（`wait_events` 看到 0 之后没有调用体再运行） |
| `events/lifecycle b` | §7 | 卸载等它正在运行的调用结束，它们看到 `ctx.cancelled()` | 进程内并发 | SH + K | EVP-7.1-4 | K：`unload/waits-and-cancels`、`unload/in-flight-emit` | 部分 | #622 |  |
| `events/lifecycle c` | §7 | 这个等待以宿主的关闭截止时间为限 | 外部 | 测试 | EVP-7.1-4 | — | 不适用 | — | 计时；`unload_deadline.rs` 已有 |
| `events/lifecycle d` | §7 | 监听器以登记它的上下文被调用 | 纯逻辑 | 测试 | EVP-12.1-4 | — | 不适用 | — | 确定性 |
| `events/remote a` | §7 | 别处条目（`RemoteListener`）是同一张表里的条目：登记顺序、按其描述过滤、`global`、由其 disposer 撤下 | 纯逻辑 | PT | EVP-12.1 | — | 未覆盖 | #631 | 五种方式参考解释器含远程监听器 |
| `events/remote b` | §7 | 交给它的 waterfall `next` 只执行其余一次（第二次调用、调用结束后的调用以 `WaterfallNext` 失败） | 纯逻辑 | PT | EVP-5.4-7、EVP-12.1 | — | 未覆盖 | #631、#632 | #632：同步 `next` 的 transmute 交给 Miri |
| `events/remote c` | §7 | 同步方式就地调用它 | 纯逻辑 | PT | EVP-12.1 | — | 未覆盖 | #631 |  |
| `events/remote d` | §7 | 表的观察者收到每次变化，按发生的顺序（`watch_table`）；Rust 的 `on` 在返回之前已通知 | 进程内并发 | SH | EVP-4.4-5、EVP-12.1 | — | 未覆盖 | #622 | 监听器表观察者模型（FIFO、恰好一次、`deliver(seq)` 在 seq 被听到后才返回） |
| `events/remote e` | §7 | 原始分发：`emit` 在全部调用开始后才完成；`invoke_entry` 按 id 调用一个条目 | 进程内并发 | SH + PT | EVP-5.4-1、EVP-12.1 | — | 未覆盖 | #622、#631 | 宿主据此在调用开始之后才回复 emit（c2c1b7e8 的宿主一半）；协议层的 `before-future-reply` 在 #613 |
| `events/remote f` | §7 | 按 id 调用的条目必须是这个名字的 | 纯逻辑 | 测试 | EVP-12.1 | — | 不适用 | — | 确定性判定 |
| `boundary/5.1a` | §8 | 服务经 `ctx.provide` / 属性赋值发布和换值，rutis 侧随之换到新对象 | 跨进程 | SH + TLA | REQ-5-1 | — | 未覆盖 | #625、#628 | #628 的服务投影 `Projection`（静止时每个槽应用最新句柄、恰好释放一次）；#625 的 `NewerWins` |
| `boundary/5.1b` | §8 | 直接 `ctx.set` 换值不产生通知，rutis 侧在下一次调用该服务后切换到新对象 | 跨进程 | SH | REQ-5-1 | — | 未覆盖 | #628 | 调用后刷新槽属投影（`projection_lifecycle.rs` 的两个测试） |
| `boundary/5.2` | §8 | 跨边界 `emit` 只是通知；需要等待或结果时用 `parallel` / `serial` | — | — | — | — | 已取代 | — | 登记表：已撤回（0.13），由 `events/*` 取代（#464） |
| `boundary/5.3` | §8 | 事件顺序只在同一侧内保证；另一侧的监听作为一组在固定位置执行 | — | — | — | — | 已取代 | — | 同上 |
| `boundary/5.4` | §8 | waterfall / bail 链不跨侧交错；对方一组作为一个整体环节 | — | — | — | — | 已取代 | — | 同上 |
| `boundary/5.5` | §8 | 框架内部事件 `internal/*` 不跨边界 | — | — | — | — | 已取代 | — | 已撤回（0.13）；"`internal/*` 不跨边界"仍成立，现由 `events/hub-reserved` b 承担 |
| `boundary/5.6` | §8 | 同步方法在执行中等待需要 Node 事件循环推进的结果（定时器、Promise）时返回 `SyncWaitCycle`，不卡死 | 跨进程 | TLA + SH | REQ-5-6、EVP-11-5 | — | 未覆盖 | #614、#630 | CrossRuntimeSync 不建"等待链上 await 未完成的 promise"（#614"Node 的细节"计划加）；`sync_cycles.rs` 不含这一形状；Node 帧泵的可控调度在 #630 |
| `boundary/5.7` | §8 | 宿主提供的服务在 Cordis 侧是按接口生成的代理：按接口调用与原生一致，`instanceof` 不成立 | 纯逻辑 | 测试 | REQ-5-7 | — | 不适用 | — | 代理的形状；确定性测试 |
| `boundary/5.8a` | §8 | 插件不得长期阻塞事件循环；兼容层不为调用加超时 | 外部 | 测试 | REQ-5-8、EVP-5.7-3 | — | 不适用 | #637、#641 ※ | "不加超时"没有断言（登记表）；#633 的计时清单不含；"插件不得阻塞"是对插件作者的要求，无法验证（问题 8） |
| `boundary/5.8b` | §8 | rutis 侧需要超时时用异步方法自行设置，超时即取消 | 跨进程 | TLA | REQ-5-8 | — | 未覆盖 | #626 | `CancelReaches` |
| `boundary/5.8c` | §8 | 卸载的等待时间由应用决定 | 外部 | 测试 | REQ-5-8 | — | 不适用 | #637、#641 ※ | 没有测试；与 EVP-7.1-4（运行时等这一行的监听器最多 `RUTIS_SHUTDOWN_TIMEOUT`，默认 10 秒）的关系没写明（问题 7） |
| `boundary/5.8d` | §8 | 未捕获的异常 / 拒绝结束整个 Node 进程 | 外部 | 测试 | REQ-5-8 | — | 不适用 | — | 进程行为；Bun 测试与 rutis-host e2e（S6）已有 |
| `boundary/5.8e` | §8 | 进程结束时同一挂载的服务随之撤销 | 跨进程 | TLA + K | REQ-5-8 | — | 未覆盖 | #625 | 行生命周期模型的运行时崩溃动作与 `WithdrawBeforeUnload` |
| `events/v2-required a` | §8a | 没有声明 `events-v2` 的对端（运行时、节点）在握手之后立即被拒绝；什么都不挂载 | 跨进程 | PT | EVP-2.1-2、EVP-2.1-3 | — | 未覆盖 | #631 | 握手判定表 PT；两方两条消息，不需要 TLA（#612 §2.1） |
| `events/v2-required b` | §8a | 拒绝信息即 §2.2 的原文，两种格式都是 | 外部 | 测试 | EVP-2.2-1、EVP-2.2-2 | — | 不适用 | — | 确切文字 |
| `events/presence a` | §8a | 运行时在 `mount` 之前得知在它之外有监听器的全部名字和前缀，之后得知每次变化，按顺序；它自己的条目不算 | 跨进程 | TLA + SH + K | EVP-4.5-1、EVP-4.5-2 | K：`fast-path/elsewhere-follows`、`fast-path/local-only` | 部分 | #613、#628 | #628 presence（计数等于表；疑点：`Hub::of` 重放时丢掉 Removed，留下幽灵条目） |
| `events/presence b` | §8a | 变化在引起它的登记返回之前送到 | 跨进程 | TLA + TV | EVP-4.1-2、EVP-4.4-3、EVP-4.4-5 | — | 未覆盖 | #613、#616 | `RegisteredThenSeen` |
| `events/hub-dispatch a` | §8a | 宿主发出的事件以一个 `events.call` 到达运行时的监听器（有类型的事件按参数） | 跨进程 | TLA + K + TV | EVP-5.3-3、EVP-5.3-5、EVP-12.1-2 | K：`emit/reaches-all`、`order/registration`（发出方 vary 到 host）、`rust/typed-args` | 部分 | #613、#616 | `SettlesOnce`（每个（分发，条目）最多一个 `events.call`） |
| `events/hub-dispatch b` | §8a | 运行时经宿主的分发按登记顺序到达宿主的与它自己的（`own`）监听器，得到该方式的结果 | 跨进程 | TLA + K + TV | EVP-5.2-2、EVP-4.3-1、EVP-5.3-4 | K：`order/registration`、`fast-path/elsewhere-follows` | 部分 | #613、#616 | `OneOrder`、`RegisteredThenSeen` |
| `events/hub-dispatch c` | §8a | 读线程不执行插件代码：看到运行时 emit 的插件代码可以同步调用这个运行时 | 进程内并发 | SH + TLA | — | — | 未覆盖 | #614 | #614 的宿主参与者（读线程不算 `Workers`）；EVP 没有对应规则（问题 9） |
| `events/hub-flat a` | §8a | 运行时的分发按名字送达所有监听器，与监听器的作用域无关 | 跨进程 | PT + K | EVP-3-2 | K：`scope/flat-bus`、`scope/siblings-share-the-bus`、`node/root-hears-instances` | 部分 | #631 |  |
| `events/hub-flat b` | §8a | 运行时的分发指明会话上已加载的一行；会话上未加载的行被拒绝 | 跨进程 | TLA + TV | EVP-3-4、EVP-4.2-3 | — | 未覆盖 | #613、#616 | 与 `rows.unload` / `rows.ended` 的先后在 EventsTable 的行卸载里 |
| `events/hub-scope` | §8a | 运行时的分发在它那一行的作用域发出；监听器听到自己作用域及其下的 | — | — | — | — | 已取代 | — | 登记表：已撤回（0.15.1），见 `events/hub-flat`（#604、#605） |
| `events/hub-filter a` | §8a | 分发的过滤函数在发出方进程里运行，按名字取出候选后带着全部（非 `global`）描述调用一次 | 跨进程 | K + TV | EVP-6-1、EVP-6-2 | K：`filter/keeps-and-sees`、`filter/every-listener`、`filter/own-listeners-in-scopes`、`filter/cordis-this-arg` | 部分 | #616 | 选择器的调用次数与时机是宿主的动作，由 TV 校验 |
| `events/hub-filter b` | §8a | 答案个数不对则分发失败 | 纯逻辑 | 测试 | EVP-6-1 | — | 不适用 | — | 确定性判定与确切文字 |
| `events/hub-lifecycle a` | §8a | 行卸载时它的条目离开总线；会话结束时会话的全部条目离开 | 跨进程 | TLA + SH + K + TV | EVP-7.1-1、EVP-7.2-1 | K：`unload/not-called-after`、`registration/unloaded-row-silent`、`disconnect/restart-hears-again`、`disconnect/emitter-gone-listener-stays` | 部分 | #613、#616、#628 | `WithdrawnStaysOut`；#628 `SessionEvents` |
| `events/hub-lifecycle b` | §8a | 卸载不等运行时永远不回答的调用；它以会话结束的传输错误失败一次 | 跨进程 | TLA + K | EVP-7.1-6、EVP-7.2-2 | K：`disconnect/in-flight-fails-once`、`disconnect/parallel-aggregates`、`disconnect/bail-in-flight` | 部分 | #613 | `SettlesOnce`；K 查的是会话结束时失败一次，不查"卸载不等" |
| `events/hub-lifecycle c` | §8a | 行在登记中途卸载时，登记的条目也被撤下 | 进程内并发 | SH | EVP-7.1-1 | — | 未覆盖 | #628 | `SessionEvents`（join、on、off、row_ended、leave） |
| `events/hub-lifecycle d` | §8a | `rows.ended` 与同一会话的 `events.*` 按帧的顺序处理 | 跨进程 | TLA + SH + TV | EVP-7.1-1、EVP-4.1-1 | — | 未覆盖 | #613、#616、#628 | `before-ended-order`；`run_ordered` / `drain_ordered` |
| `events/hub-bail a` | §8a | `bail` 处处同步：已知异步的运行时监听器使其失败（`AsyncListener`） | 跨进程 | K | EVP-5.4-5、EVP-5.4-8 | K：`bail/async-listener`、`rust/bail-async-listener` | 已覆盖 | — |  |
| `events/hub-bail b` | §8a | 同步的监听器给出答案；运行时经宿主的 `bail` 得到宿主监听器的答案 | 跨进程 | K + TLA | EVP-5.4-5、EVP-5.3-1 | K：`bail/first-result`（含 host）、`bail/no-result`、`bail/call-back-into-waiting-emitter` | 部分 | #614 | bail 作为宿主上的同步执行（不死锁）；等待环归 `bridge/sync-wait-cycle-refused` |
| `events/hub-errors a` | §8a | `parallel` 按登记顺序汇总失败（`AggregateError`） | 跨进程 | K + PT | EVP-5.4-2、EVP-11-1 | K：`parallel/errors-in-registration-order`、`parallel/one-error-is-aggregated`、`disconnect/parallel-aggregates` | 部分 | #631 | PT：事件错误的移植（`graft`、`moved`） |
| `events/hub-errors b` | §8a | 对端不接受的值只让那一次调用失败 | 跨进程 | K + PT | EVP-10-1 | — | 未覆盖 | #639 ※ | 期望文件没有"值能力不足"的用例；值往返 PT 的 `rust_tagged_values_refused_without_capabilities` 只经服务调用；#613 不建值，#624 的 PT 补列不含事件 |
| `events/hub-reserved a` | §8a | 运行时不能发出 `rutis/*`（`ReservedEvent`） | 跨进程 | K | EVP-5.3-2、EVP-8-1、EVP-11-4 | K：`reserved/rutis-emit`、`reserved/every-mode` | 已覆盖 | — | K 查各语言发出 API 的拒绝 |
| `events/hub-reserved b` | §8a | 宿主拒绝运行时帧里 `rutis/*` 的分发与 `internal/*` 的登记 | 纯逻辑 | 测试 | EVP-4.2-2、EVP-8-5 | — | 不适用 | — | 确定性名字判定；`event_hub.rs` 已测 |
| `events/node-links a` | §8a | 按 `export` / `import`，一个节点的监听器是另一个节点总线上的条目：方式、结果、带标签的值 | 跨进程 | K | EVP-9-2、EVP-9-7 | K：`node/shared-both-ways`、`node/values`、`node/function-argument`、`node/rutis-events`，以及 vary 放到 node 的各用例 | 已覆盖 | — | node 位置只有 Rust 节点 |
| `events/node-links b` | §8a | 不共享的名字留在原节点 | 跨进程 | K | EVP-9-4 | K：`node/not-shared` | 已覆盖 | — |  |
| `events/node-links c` | §8a | 连接断开时条目离开 | 跨进程 | TLA + K | EVP-9-7、EVP-7.2-1 | K：`disconnect/node-link` | 部分 | #613 | #613 的 EventsLink 没有列断开（EventsTable 的会话结束只对运行时），只是部分计划 |
| `events/node-links d` | §8a | 对端重新提供 `events` 时，旧提供下宣告的条目被替换；迟到的旧宣告不被接受；先于提供到达的宣告不因尚未列出它的 `link.offers` 被撤下 | 跨进程 | TLA + K | EVP-9-6 | — | 未覆盖 | #613 | `CurrentOffer`、`before-offer-race`、`accept-unlocked`（疑点：`link.rs` 的 `accept()` 在查代次与插入之间解锁） |
| `events/node-links e` | §8a | 宣告的 id 为 `<epoch>.<n>`，Rust 与 Node 的 Cordis 节点相同 | 跨进程 | K | EVP-9-6 | — | 未覆盖 | #639 ※ | 两种实现各自测（`node.rs`、`cordis-events.test.mjs`），没有共用的一致性用例；期望文件的 node 位置只有 Rust 节点；#615 补的是 Cordis 节点的退避与接管 |
| `events/node-links f` | §8a | 嵌套的前缀只送达一次 | 纯逻辑 | PT | EVP-9-4 | — | 未覆盖 | #631 | export / import 收窄（两者都匹配的具体名字恰好被覆盖一次） |
| `events/node-config a` | §8a | 节点行里的 `events: { out, in }` 被拒绝并说明替代写法 | 纯逻辑 | 测试 | EVP-2.4 | — | 不适用 | — | 确切文字 |
| `events/node-config b` | §8a | `export` / `import` 支持 `*` 前缀 | 纯逻辑 | PT | EVP-9-1 | — | 未覆盖 | #631 |  |
| `events/framework a` | §8a | `rutis/row` 在行的每次状态变化时（运行时的行与宿主的行都一样）于该行的作用域发出 | 跨进程 | K + SH | EVP-8-3 | K：`rutis/row`、`rutis/row-failed`、`rutis/row-failed-gated`、`rutis/row-host-row` | 部分 | #639 ※ | 登记表只写 running、stopped、failed，规格 §8 与用例 `rutis/row` 还有 `pending`（问题 4）；#612 §2.2 把"`rutis/row` 跟随者"列为加载器 SH，但 #627 没有列它 |
| `events/framework b` | §8a | `rutis/runtime` 在运行时挂载成功（每次重连后也是）时 `running`，会话结束或挂载失败时 `down` | 跨进程 | K + SH | EVP-8-4 | K：`rutis/runtime`、`rutis/runtime-down-reason` | 部分 | #628 | #628 疑点：快速重启时 down 与第二次 running 被 `watch` 合并掉 |
| `events/framework c` | §8a | `rutis/service` 在宿主总线上的服务可用或离开时发出（带提供它的行），每个宿主服务都有，Rust 提供的也一样，于提供方的作用域发出 | 跨进程 | K + SH | EVP-8-2 | K：`rutis/service`、`rutis/service-scoped`、`rutis/service-of-rust` | 部分 | #622 | `rutis/service` 由 `ServiceChanged` 转出（`events/hub.rs:37`），#622 疑点"同一绑定的 `ServiceChanged` 可能先 Removed 后 Provided"会直接反映到这里；规格的"提供方被替换（再次 true）"不在登记内容里 |
| `events/one-bus a` | §8a | 期望文件的每个用例在 X 为每种运行时语言、Y 为每种可用语言、Rust 参与者在宿主和第二个节点上时都成立（按写定的与抽样的放置） | 跨进程 | K + TLA | EVP-13、EVP-5.2-4 | K：`events_conformance.rs` 的 `events_with_x_a_{cordis,node_leaf,python,go,kotlin,swift,bun}_plugin`（97 个用例；每晚 `events-all` 跑全部放置） | 部分 | #613 | 登记表写 91 个用例、5 种 X 语言，现在 97 个、7 种（问题 4）；"一条逻辑总线"在 #613 里是 `OneOrder`、`RegisteredThenSeen`、`EmitOrder`、`FastPathByFrame` 等的合取，协议不承诺顺序一致（`SeesAll` 预期违反，问题 2） |
| `events/one-bus b` | §8a | 测试工具对它运行的每个用例（没有 `tool: false` 的）给出相同的结果 | 纯逻辑 | K | EVP-12.2、EVP-13.2 | K：Node `events-conformance.test.mjs`、Python `test_events_conformance.py`、Go `events_conformance_test.go`、Bun `events-conformance.test.ts`、Kotlin `EventsConformanceTest.kt` | 已覆盖 | — | 与运行时共用代码的工具可能同错（#612 §2.4）；登记表没列 Kotlin 的工具 |
| `events/old-peer-refused a` | §8a | 0.13 起每个早于 0.13 的已发布版本与当前代码相遇时，在握手之后、任何东西加载之前被拒绝，消息即 §2.2 原文、说明升级哪一端（各方向、各语言、节点） | 外部 | 测试 | EVP-2.1-3、EVP-2.2-1、EVP-2.2-3、EVP-2.2-4 | — | 不适用 | — | 要下载已发布版本（e2e `#[ignore]`，完整 CI 运行）；判定与 `events/v2-required` a 共用握手判定表 PT（#631） |
| `events/old-peer-refused b` | §8a | 拒绝不重试、不挂起：Go 二进制记为被拒、不再启动，它的行以同一消息失败；远程运行时的连接以 `incompatible` 停止、不再拨号；节点任一方都不重试 | 进程内并发 | SH + PT | EVP-2.2-1、EVP-2.2-3 | — | 未覆盖 | #627、#631 | #627 二进制监督（失败的启动只在文件变化或请求重启时重试）；link 循环"被拒不重试"的判定属握手判定表 / 退避 PT |
| `events/no-multi-hop` | §8a | 只走一跳：节点经连接够到的监听器从不再传给它的其他连接 | 跨进程 | TLA | EVP-9-4 | — | 未覆盖 | #613 | `NoEcho`（单连接；a—b—c 由 `events_nodes.rs` 的普通测试覆盖） |
| `events/link-export-import a` | §8a | 节点连接的 `export` / `import` 接受事件名和 `*` 前缀（服务仍是确切名字） | 纯逻辑 | PT | EVP-9-1 | — | 未覆盖 | #631 |  |
| `events/link-export-import b` | §8a | 事件只有在名字同时在发出节点的 `export` 和监听节点的 `import` 里时才到达另一个节点上的监听器；不在其中的收不到 | 跨进程 | PT + K | EVP-9-2 | K：`node/not-shared`（`n/*` 不在两边的列表里）、`node/shared-both-ways` | 部分 | #631 | 已撤回（#1212）：0.19 删除节点的 `export` / `import` 名单（`design-remote-session-2026-10-28.md` §八、§十二；`migration-0.19.md`），事件按名字到达另一节点的监听器，不再需要两边名单；#631 的 export / import 收窄 PT 没有对象，K 的 `node/not-shared` 已删，`node/shared-both-ways` 只剩 `tool: false` 的期望；按[复核表](../recheck-0.22.md)记「已撤回」 |
