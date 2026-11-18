# 不遍历祖先链的事件监听器关闭接纳

[English](design-event-close-admission-2026-11-08.en.md)

Status: 核心已实现，验证中。日期：2026-11-08；2026-11-09 实现 marker 发布、`Entry::live`、确定性竞态测试和 shuttle 模型。
Issue：[ #1025 ](https://github.com/arcships/rutis/issues/1025)。实施 issue：#1025-core、#1025-tests、#1025-benchmark（将在 0.22 计划中拆出）。

## 1. 现状与要解决的问题

每个被选中的事件监听器现在都要检查 owner 是否仍开放：读取 owner 状态，然后调用 `Ctx::registration_open()`。后者读取全局关闭状态并遍历每个祖先（`crates/rutis/src/ctx.rs:191-207`）；`Entry::live()` 在 `crates/rutis/src/bus/table.rs:77-90` 的热路径执行此操作。在 `close_cpu` 工作负载（1000 个 instance、每个 8 个 row、listener 总数 8000 个）中，2026-11-02 release 样本创建 instance 约 38.2 秒，其中 registration-open 约 9.7 秒（约 25.4%），每次 listener 调用还会读取两三条冷的祖先 cache line。这组 38.2/9.7 秒是 issue 历史测量，不是自动化 wall-clock 断言。#1025 的 issue 目标是该工作负载的创建耗时 ≤30 秒；实现验收还必须满足现有 CREATE_RATIO、CLOSE_RATIO 和 IDLE_SHARE 阈值，且不改变事件/关闭语义。

该 benchmark 测量的是进程 CPU 时间（`CLOCK_PROCESS_CPUTIME_ID`），不是墙上时钟（`crates/rutis-loader/tests/close_cpu.rs:96-106`）。`CREATE_RATIO` 和 `CLOSE_RATIO` 分别是创建、关闭阶段的 `CPU_time(large workload) / CPU_time(small workload)`；`IDLE_SHARE` 是 `idle process CPU time / idle wall-clock window`（`crates/rutis-loader/tests/close_cpu.rs:198-203,273-305`）。release 使用 100 和 1000 个 instance，debug 使用 40 和 400；当前阈值分别为 release `300`、`200`、`0.05`，debug `180`、`160`（idle 阈值共用；常量见 `crates/rutis-loader/tests/close_cpu.rs:67-79,93-94`）。

shutdown 已在标记子树并开始关闭时持有 `Shared.admission`（`crates/rutis/src/fiber.rs:1756-1780`）；root shutdown 在设置 `Shared.closing` 时持有同一锁（`crates/rutis/src/ctx.rs:1318-1338`）。事件 snapshot/selection 不持有该锁，`Entry::start()` 刻意在最终 liveness 检查前开始 call flight。没有发布协议的 marker 可能让 ancestor 已关闭而 descendant 仍被看作 live。下面的发布和脱离要求都是提案、尚未验证；必须等实现和竞态测试建立该证明后才能改为 Current。

## 2. 决定

| 决定 | 选择 | 原因；放弃的方案 |
| --- | --- | --- |
| D1 | 在 `FiberInner` 增加永久的 `lineage_closed: AtomicBool`。永久关闭的 fiber 以及关闭提交时子树中的每个成员都为 true；root 已关闭后新建的 fiber 初始化为 true；generation reload 永不清除。 | 不使用 per-entry marker 或共享继承 marker：entry 是动态的；共享 marker 会在只关闭一个子树时误关 sibling。 |
| D2 | 在 `Shared.admission` 下两阶段发布：收集固定子树；按 postorder（叶到根）为所有成员写入 `lineage_closed=true`；全部写完后再发布现有 per-fiber `closing=true`（root 另发布 `Shared.closing=true`）；最后排入 cleanup。shutdown 线性化点是 marker pass 完成，而不是第一个 ancestor flag。实现必须让 child-link 脱离也遵守该锁，或证明脱离中的 fiber 已不能再接纳工作。 | 放弃简单的 ancestor→child 传播，因为会产生 ancestor closed/child live 窗口；也放弃仅 child-first 的公开关闭变更，因为会改变既有关闭边界。 |
| D3 | `Entry::live()` 读取 `withdrawn` 和 owner 的状态字（`OwnerState`：`alive`、`closing`、`lineage_closed` 和 `live_generation` 共用一个原子，一次读），不再遍历 context parent 链。一次 dispatch 先在 owner 上计数（每个 owner 一次，`DispatchFlight`），再做最终的状态检查（#1125）。 | 放弃移除最终检查或 event flight：已接纳调用必须 drain；marker 提交后不得再接纳调用，但已接纳调用可以在提交后开始并按既有规则完成。按调用计数换成按 dispatch/owner 计数，卸载的等待只会变长，不会变短。 |
| D4 | `Ctx::registration_open()` 及其祖先遍历继续用于 registration/service API。本优化只改变 listener liveness，不改变 public registration error 或 instance-event admission。 | 放弃用 listener marker 取代所有 lifecycle 检查，因为 registration 有独立 API 合约。 |
| D5 | 计数和状态字都用 `SeqCst`。准入/排空的证明要的是“等待没看到的那个计数，必然读到发布后的状态”，这要求计数和状态字在同一个 `SeqCst` 总序里（推导写在 `crates/rutis/src/fiber/state.rs`）。事件 snapshot 热路径不加锁，也不持有会阻塞的 admission lock。 | 放弃每次 dispatch 都拿 `Shared.admission`，因为会增加锁开销并可能无法达到 CPU 目标。放弃只靠 `Acquire`/`Release`：两个不同的原子之间没有总序，ARM 上“计数加在等待读过 0 之后、状态读又读到旧值”是允许的；x86-64 和 aarch64 上 `SeqCst` 读与 `Acquire` 读是同一条指令，代价不在热路径上。 |

## 3. 精确规则

**这些规则的状态。** 2026-11-09 已实现规则 1、2、5 的核心：`mark_subtree_lineage` / `publish_subtree_closing` 的两阶段发布、`release_transient` 持锁摘除、root 子树标记、`Entry::live` 只读 owner marker，以及 `Entry::start` 的 `CallFlight` 顺序。确定性测试见 `crates/rutis-loader/tests/event_close_admission.rs`，shuttle 模型见 `crates/rutis/src/fiber/shuttle_tests.rs`。规则 3、6、7、8、9 的 Loading-owner、`once`、remote-entry、跨进程和重载覆盖尚未全部落地，因此本节仍不把规则写成“当前保证”；等 §6 的全部门槛通过后再改状态。

1. **提案，尚未验证。** 非 root 子树关闭前，shutdown 必须在 `Shared.admission` 下收集当前所有 descendant，并按 postorder（叶到根）为全部成员写 `lineage_closed=true`。然后写现有 `closing=true` 并调用 `pre_cancel`，最后调用 `begin_subtree_shutdown_locked`。fiber 创建和子树收集使用 `Shared.admission`；`release_transient` 已在同一锁下从 `parent.children` 摘除 child（`crates/rutis/src/fiber.rs:1477-1500`），测试见 `crates/rutis-loader/tests/event_close_admission.rs`，模型见 `crates/rutis/src/fiber/shuttle_tests.rs`。实现：`mark_subtree_lineage` / `publish_subtree_closing`（`crates/rutis/src/fiber.rs:2172-2226`）。
2. **提案，尚未验证。** root shutdown 对 root 加全部 descendant 执行相同 marker pass，再写 `Shared.closing=true`，对 root/descendant 执行 pre-cancel 并提交现有关闭意图。即使旧 root shutdown 只写 `Shared.closing`，root descendant 也必须被标记。
3. **提案，尚未验证。** 在 shared root 已关闭时创建的 fiber 以 disposed 和 `lineage_closed=true` 开始。永久关闭 marker 不会在 restart/update 时清除；新 fiber 获取新的 marker。
4. **提案，尚未验证。** marker pass 是关闭接纳边界：最终 `Entry::live` marker load 在完成前的调用可以接纳并正常等待；完成后的检查不得再接纳。必须固定两种竞态： (a) `Entry::start` 在 marker pass 完成前完成最终 `live()`，调用被接纳并必须 drain；(b) marker pass 先完成，最终 `live()` 随后失败，`CallFlight` 被释放且 listener 不运行。snapshot 可能持有 stale entry，但 `Entry::start` 是权威的最终检查。已接纳的 callback 可能在 owner marker 提交后开始，但必须按既有规则 drain；提交后不得有新的 callback 被接纳。由于该提交点晚于当前实现第一次写 `closing` 的时间，实现测试还必须记录 marker pass 是否扩大了接纳窗口。
5. **提案，尚未验证。** `Entry::start` 先开始 `CallFlight` 再调用 `live`；closed 结果立即 drop flight，不调用 listener。已接纳调用保留现有 event-flight 等待和取消行为。`once` 只在 `start` 后 claim，因此被跳过 listener 不会被消耗。
6. **提案，尚未验证。** marker 对子树关闭是永久的。普通 generation unload/reload 只要没有永久 shutdown，fiber 仍可供后续 generation 使用；reload 不会重置 `lineage_closed`。`withdrawn`、`alive`、`closing`、`live_generation` 含义不变。
7. **提案，尚未验证。** registration 仍拿现有 admission 锁，并在 commit 时再次检查生命周期。marker pass 期间 preflight 读到 open 不能通过该锁提交；关闭子树不会接纳新 registration。
8. 既有 instance-event 规则不变。关闭后的 registration/effect/provide 类接纳入口返回 `ErrorKind::Closed`（`root has been shut down`），现有覆盖见 `crates/rutis/tests/instance_subtrees.rs:291-325`。监听器规则中“`Ctx::emit()` 本身不会仅因子树关闭而失败，并且不会触达到该子树的 live listener”仍是提案，而非当前保证：`Filtered::emit()` 是同步的 `Result<(), Error>` API（`crates/rutis/src/events/api.rs:453-480`），现有 service-after-stop 测试明确将停止后的 delivery 标为未承诺（`crates/rutis/tests/service_emits.rs:70-83`）。实现测试必须为 `emit`、`serial`、`parallel`、`bail` 和 waterfall 固定该提案结果。本优化只涉及非 instance listener 的 `Entry::live`，不改变 instance-event 接纳语义。
9. **提案，尚未验证。** listener callback、事件顺序、dispatch mode、跨进程协议、取消 deadline 和 shutdown 输出不变。其他 process/runtime 不会看到新消息。
10. **提案，尚未验证。** 如果实现改用重试 sequence guard，而不是仅用两阶段 marker，也必须保持同一规则：事件路径不能从混合发布中返回 live。可以使用 `SeqCst` 的 odd/even `shutdown_sequence`，但必须测量 spin 成本并证明不会无限等待；默认实现仍为 marker-first 两阶段 pass。

## 4. 开发者看到什么

### 插件作者

仅在提案实现完成并通过验证后，目标行为才是 API 和输出没有变化。运行中的 row 注册的 listener 收到完全相同的事件和错误。ancestor 在 marker 提交后永久关闭时，之后发出的事件不会调用该 listener；关闭后的 `effect()`、`provide()` 和 listener 注册尝试仍返回 `ErrorKind::Closed`，文字为 `root has been shut down`，且没有新的 `code`/`data`。提案中的 `emit()` 行为是正常返回，同时不会触达到该子树的 live listener；该 delivery 结果要等竞态测试落地后才算验证。在途 callback 按现有 shutdown deadline 完成或取消。

新增概念：无。`lineage_closed` 是内部性能 marker。

### 使用 `rutis-host` 组装插件的人

没有配置变化，也没有新的 status 行。`rutis-host status` 仍打印现有 row 状态（`running`、`stopping`、`stopped` 或既有 failure headline）。事件关闭优化没有 host error code 或迁移设置。

新增概念：无。

### 在 Rust 中嵌入 rutis 的宿主作者

提案目标是不改变 public Rust API、事件协议或 shutdown result；在 marker 实现和竞态测试通过前，这不是当前保证。当前基线已有事件测试覆盖接纳与 drain 规则。提案实现后，子树接纳关闭后不再接纳新的 listener callback；已经接纳的调用即使在提交后开始，也按现有等待/取消规则 drain。提案不增加 error code 或 data 字段。

新增概念：无；维护者只增加内部 owner marker 和关闭线性化规则。

### 部署者

没有部署设置、wire version 或 process boundary 变化。release artifact 行为不变，只在 stress benchmark 中降低 instance 创建 CPU。没有可见成功或错误输出变化。

新增概念：无。

## 5. 兼容与迁移

- 提案目标是不改变 public API、事件协议、listener 顺序、shutdown 状态/输出或 error code；这些不是新的当前保证。
- 如果提案实现完成，现有 listener 预计无需迁移。提案兼容要求是关闭后事件仍被排除、已接纳调用仍 drain；目前尚未验证。
- `close_cpu` 仍是 release benchmark。对比现有 `CREATE_RATIO=300`、`CLOSE_RATIO=200`、`IDLE_SHARE=5%` 阈值；不能用牺牲关闭语义换取创建速度。
- 该内部变更不提议新增 package version 或 runtime protocol field。只有在事件关闭保证重新验证后，0.22 release note 才可记录性能修复。

## 6. 实施

1. **Core marker 与发布（#1025-core）。** 在 `FiberInner` 增加 `lineage_closed`，从初始 closed 状态初始化；在 `Shared.admission` 下集中处理 non-root/root shutdown marker 发布，在现有 `closing`/`Shared.closing` 发布前标记固定子树，保持 cleanup 顺序不变。仅从 `Entry::live` 移除 `owner.ctx.registration_open()`。提案完成门槛：源码级锁/顺序审查证明关闭边界前所有子树成员均已标记；generation reload 永不清除 marker。
2. **事件竞态测试（#1025-tests）。** 增加确定性 parent→child listener 测试：完成 parent shutdown 的 marker pass 后，立即从不相关的 open context emit，断言 child 调用次数为零；增加 root shutdown、嵌套 subtree、sibling、Loading-owner、重复 shutdown 和 remote-entry 覆盖。利用测试 selection hook 选中 entry，在 `Entry::start` 前 shutdown，断言最终 marker check 跳过；验证已在途 callback 仍 drain。提案完成门槛：sync、`emit`、`serial`、`parallel`、`bail`、waterfall 均不会在 commit 后接纳新的 callback；已接纳 callback 即使在 commit 后开始，也按既有规则 drain。
3. **Benchmark 与 CI（#1025-benchmark）。** 运行 `cargo test -p rutis-loader --release --test close_cpu -- --nocapture`；对比 creation/close CPU 和 queued-emits memory。验收 job 是 `.github/workflows/stress.yml:381-417` 的 Ubuntu 24.04 release job，依次运行 `close_cpu` 和 `queued_emits_memory`；summary 必须保留两项结果。只用实测值更新 0.22 performance record/CI summary。提案完成门槛：记录 issue 工作负载的创建耗时目标（≤30 秒）；自动测试继续比较既有 `CREATE_RATIO`、`CLOSE_RATIO`、`IDLE_SHARE` 阈值，close CPU 不回归，并保持 queued-emits memory 结果。
4. **文档/release（#1025-docs）。** 保持本文档与英文对应稿同步；加入 0.22 release-note 条目，并链接不变的事件关闭保证。提案完成门槛：双语审查确认没有暗示新的开发者可见行为。

### 2026-11-09 进展

- **core：** `FiberInner::lineage_closed` 在 spawn 时按初始 closed 状态初始化；非 root 和 root shutdown 都在 `Shared.admission` 下先收集固定子树、写完所有 marker，再发布 `closing` / `Shared.closing`；`release_transient` 的 child 摘除已持同一把锁。
- **测试：** `cargo test -p rutis-loader --test event_close_admission` 4 个用例通过：五种 dispatch mode 的 commit-后跳过、root 跳过、嵌套/sibling、已接纳回调 drain。
- **模型：** `shuttle_lineage_marker_precedes_closing` 通过；把 `publish_subtree_closing` 放到 `mark_subtree_lineage` 之前，模型立即报“this node is closing while its lineage marker is clear”。
- **模型：** `specs/event-close/run.sh` 的 7 个配置全部按 `expected.txt` 结束：`close-healthy` 2,263,040 个不同状态、21 秒，`close-tree` 688,580 个、12 秒，`close-drain` 31,488 个、1 秒；4 个变异分别违反 `ClosingImpliesMarked`、`CommittedStaysOut`（两次）和 `CleanupOnce`。
- **benchmark（本机 release，Linux x86-64）：** 100 个 instance 创建 150.60 ms、关闭 119.06 ms；1000 个 instance 创建 17.55 s、关闭 11.15 s。`CREATE_RATIO`、`CLOSE_RATIO`、`IDLE_SHARE` 阈值全部通过；issue 的 ≤30 秒创建目标在本机达到。CI 的 `stress.yml` `cpu` job 仍需记录同一组数字。
- **仍缺：** Loading-owner、`once`、remote-entry 的专项用例；CI stress job；release notes。完成这些之前，规则保持“提案，尚未验证”。

### 2026-11-18 进展（#1125）

- **计数改成按 dispatch/owner。** `FiberInner::event_flights`（互斥锁）和 `event_flights_tx`（watch）换成 `EventFlights`：一个原子计数加一个 `Notify`，只在计数归零且有等待者时唤醒。一次 dispatch 用 `DispatchFlight` 对遇到的 owner 各计一次（`counted_by` 记住本 dispatch 已计过谁），只对真正 `start` 的监听器计数，跳过的当场退回；异步 `emit` 监听器的任务、`parallel` 和远程入口仍按调用计数（`EventBus::start_alone`）。卸载的等待粒度因此从单回调变成 dispatch/owner，等待只会变长。
- **状态字。** `OwnerState` 把 `alive`、`closing`、`lineage_closed` 和 `live_generation` 放进一个 `AtomicU64`；一次调用一次 `SeqCst` 读（原来是四次 marker 读加一次 `withdrawn`）。`FiberInner` 保留四个字段给其它读者，所有写点都同时写状态字，测试 `the_word_matches_the_markers` 在静默点比对两者。
- **顺序。** `Entry::start` 变成一次 `Weak::upgrade`、先计数再读状态字；准入/排空的论证写在 `crates/rutis/src/fiber/state.rs` 的模块注释里。shuttle 模型 6 现在能抓到“先检查再计数”的顺序错误（计数改成 shuttle 可见的原子后才有这个切换点）。
- **数字（本机 release，Linux x86-64，32 核，同一 target 上 base 与分支交替三次取中位数）：** `close_cpu` 1000 instance 创建 16.47 s → 5.69 s，关闭 10.85 s → 1.44 s；`cargo bench -p rutis --bench events` 的 `bail_sync` 64 监听器每次 116.8 ns → 29.8 ns。`CREATE_RATIO`、`CLOSE_RATIO`、`IDLE_SHARE` 仍通过。
- **诊断口径不变：** `EventSubscription::calls` 仍是“已开始的调用数”，仍是一次 `Relaxed` 加（实测约 1.4 ns/次）；`listener_table_model` 用它做精确比对，所以没有改成按 dispatch 汇总的近似值。

## 7. 设计哲学检查

优化只在 Rust core 实现一次，不开放宽 hook；通过测试固定关闭边界和内存序，并且不对跨进程作出超出现有合约的承诺。唯一不能为性能而冒险改变的是精确关闭接纳边界，文档明确写出而不是静默改变。符合设计哲学 §7 的原则 1、3、4、7、10。
