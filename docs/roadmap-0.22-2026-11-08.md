# rutis 0.22 设计与交付路线图

[English](roadmap-0.22-2026-11-08.en.md)

Status：设计基线，尚未实现。日期：2026-11-08；2026-11-09 修订：0.23 的形式化覆盖并入 0.22。
跟踪：[#694](https://github.com/arcships/rutis/issues/694)（版本规划）、[#1025](https://github.com/arcships/rutis/issues/1025)（事件关闭接纳 CPU）和 [#183](https://github.com/arcships/rutis/issues/183)（质量控制）。0.23 里程碑的 #613、#622、#624、#630、#631、#634、#635、#639、#776 与 #1025 一起作为 0.22 的发布范围；执行计划见[形式化覆盖执行计划](plan/formal-verification-0.22.md)。形式化验证设计见[形式化验证重新设计](design-formal-verification-2026-11-08.md)；#1040 仍明确延期。

## 1. 目的与现状

0.22 从 `v0.21.0` 标签开始，目前没有 release branch 或实现代码。本文件在开始编码前冻结设计范围。0.23 已并入 0.22，计划范围不再分两个版本：

1. 从事件 listener liveness 检查中移除祖先遍历，同时保持现有关闭/接纳边界（[事件关闭设计](design-event-close-admission-2026-11-08.md)）；
2. 完成四层形式化覆盖：现行会话协议规格与规则追踪表、事件与生命周期的设计模型、Rust 内核可控调度、真实执行 trace 与跨语言一致性、非 Rust 运行时可控调度、属性/模糊/Kani/Miri、虚拟时钟与普通测试（[形式化覆盖执行计划](plan/formal-verification-0.22.md)）。

动态服务公告继续保持现有的 Cordis 兼容行为。宿主侧拒绝策略继续延期：在没有明确需求时，不应由这一层负责不可信插件的防御。

当前 workspace package 声明仍是 `Cargo.toml` 中的 `version = "0.6.0"`；release 工作必须先确认这是有意的 workspace 约定还是遗留声明。在决定 release 流程前，不要把它作为任一 feature 的一部分修改。

## 2. 版本承诺

0.22 承诺：

- 在不改变公开事件、shutdown、取消或协议行为的情况下，降低 `close_cpu` 工作负载中的事件 listener 接纳 CPU；
- 把 0.23 的形式化覆盖并入本版本：会话协议有现行规格，规则到证据有追踪表，四层验证按[执行计划](plan/formal-verification-0.22.md)完成并留下可复查的结果；
- 中英文文档、迁移指南和 release notes 都写明精确可见输出。

0.22 不新增：每进程一个服务 namespace、全局 allowlist、未声明公告的 `rutis.json` 配置、动态名称 `isolate`，或跨语言终态的“策略拒绝” row 状态。现有 Cordis 兼容动态公告行为保持不变；#1040 仍不在 0.22 内。

## 3. 工作流与 gate

### W0 — 验证基线与执行计划（#183、#776、#634、#635）

0.22 不再把形式化覆盖留到后续版本：先建立现行会话协议规格和规则清单，把 `docs/guarantees.md` 的条目接进追踪表；同时对账现有 `specs/cross-runtime-sync`、`specs/remote-lease` 和三个 shuttle 模型，旧架构模型不能当新实现的证明。执行计划和缺口见[形式化覆盖执行计划](plan/formal-verification-0.22.md)。

Gate：协议规格覆盖帧、握手、调用、引用、取消、畸形帧、未知字段和大小上限；追踪表能按规则列出证据和缺口；旧模型逐条标为当前、已取代或明确不保证；#1025 的关闭/接纳模型、反例和确定性 race test 先建立。

### W1 — 事件关闭接纳（#1025）

设计和实现遵循关联设计文档：

- 增加永久 `FiberInner::lineage_closed` marker；
- 在 `Shared.admission` 下先标记完整固定子树，再发布既有 `closing` flags，之后执行 cleanup/shutdown；
- 让 `Entry::live()` 读取 owner marker，不再遍历祖先；`Entry::start()` 保留 `CallFlight::begin(owner)` 和最终 `live()` 检查；
- 保持 registration 检查、instance-event permit、callback 顺序、取消、shutdown 输出和跨进程行为不变。

Gate：确定性的 ancestor-close 竞态及现有 sync/async dispatch mode 证明 marker commit 后不再接纳新的 callback；已接纳 callback 即使在 commit 后开始仍 drain。issue 的 `creation <= 30s` 是待记录的性能目标，不是当前 `close_cpu` 的绝对耗时断言；自动 gate 继续比较 `CREATE_RATIO`、`CLOSE_RATIO` 和 `IDLE_SHARE`，且不回归 queued-emits memory。

CI 验收 job 是 `.github/workflows/stress.yml:381-417` 的 Ubuntu 24.04 release job；它运行 `close_cpu` 和 `queued_emits_memory`，两项结果都必须保留在 summary 中。

### W2 — 四层形式化覆盖（#613、#622、#624、#630、#631、#635、#639）

0.23 并入后，W2 是 0.22 的完整覆盖工作，按四层推进：

- **A 协议基础：** 现行会话协议规格、帧与错误/引用/取消/上限规则、跨实现一致性用例；
- **B 并发与生命周期：** 事件关闭接纳的 shuttle 模型和确定性测试；事件、跨运行时调用、行生命周期、调用生命周期和节点托管行的 TLA+ 模型；
- **C 跨进程与部署：** 真实执行 trace 对照模型；leases、host-gone、sync-cycles、AccessEvent 和跨语言适用列；
- **D 输入与长期可靠性：** 属性测试、模糊测试、Kani/Miri、虚拟时钟、缺失的一致性用例和普通测试，以及反复创建/关闭/重载/重连后的资源趋势。

Gate：按[形式化验证重新设计](design-formal-verification-2026-11-08.md) §10 和[执行计划](plan/formal-verification-0.22.md) §2，每条适用规则有现行来源、实现证据、适用矩阵和状态；模型、trace、一致性、属性/模糊、虚拟时钟、平台和长期测试均有结果；故意注入的错误能被对应手段发现。

### W3 — Release、迁移与文档（#694）

在宣布实现完成前：

1. 决定并记录 package/version 更新流程，包括 `Cargo.toml` workspace 声明和 `v0.21.0` 标签；
2. 增加 `migration-0.22.md` 与 `migration-0.22.en.md`；说明动态公告行为不变，事件优化不需要 API 迁移；
3. 增加 `docs/releases/0.22.0.md` 和 `.en.md`，只在 CI 运行后写入实测 benchmark 数值；
4. 同步更新事件、host、plugin 指南以及 `release.md` 和 `release.en.md` 的中英文版本；
5. 保持本路线图与事件关闭设计稿同步；#1040 已延期，不得为 0.22 增加 JSON 字段或 wire-visible 行为；
6. 如果将来重新讨论信任边界，应先基于明确需求写新的设计文档，确定责任层次后再实现。

Gate：中英文审查确认决定、精确输出/error/code/data、按角色的 “What developers see”、迁移指引和验收标准一致。

## 4. 工作顺序

1. **基线：**核对 tag、package-version 约定、当前 status 序列化和 benchmark 环境；改代码前记录测量值；
2. **验证基线：**写现行会话协议规格和追踪表骨架；对账现有 TLA+/shuttle 模型；先写 #1025 的关闭/接纳模型、反例和确定性 race test；
3. **合约：**提交事件关闭设计、本路线图和[执行计划](plan/formal-verification-0.22.md)；在 #1025 下拆出 core、race-test、benchmark 和文档任务；#1040 不进入 0.22 实施计划；
4. **核心变更：**独立实现 marker 发布，并保持可单独回滚；
5. **四层覆盖：**按执行计划并行推进协议规格、TLA+/shuttle 模型、trace validation、非 Rust 可控调度、属性/模糊/Kani/Miri、虚拟时钟和一致性用例；
6. **验收：**运行事件 race tests、`cargo test -p rutis-loader --release --test close_cpu -- --nocapture` 以及 CI stress pair；确认模型、实现测试和 benchmark 的结果分别记录；
7. **发布准备：**依据追踪表和实测结果更新 migration/release 文档，解决 package-version 策略，再进行正常 release 审查。

本设计阶段不包含 release commit、tag、package-version 变更或公开 API 实现。

## 5. 0.22 中开发者看到什么

### 插件作者

现有 Cordis 兼容动态公告行为保持不变。事件关闭优化没有新的插件 API、输出或错误。

### 宿主作者与部署者

0.22 没有新的信任边界设置。事件关闭优化没有新的输出、status 字段、协议版本或迁移设置；现有状态行和既有失败文字保持不变。

### Rust 宿主作者

public Rust API、事件协议和 shutdown result 不变。关闭后的 registration/effect/provide 类操作仍返回 `ErrorKind::Closed`，文字为 `root has been shut down`，没有新增 `code` 或 `data`（`crates/rutis/tests/core/instance_subtrees.rs:291-325`）。事件关闭设计提议 `emit()` 本身不会仅因子树关闭而失败，但该 delivery 结果不是 0.22 保证：现有 service-after-stop 测试将 delivery 明确留作未承诺（`crates/rutis/src/events/api.rs:453-480`；`crates/rutis/tests/core/service_emits.rs:70-83`）。

### 运行时作者与贡献者

本路线图不新增插件用户可见的运行时命令、退出码、成功输出或失败输出；既有模型、shuttle 和 CI 输出保持原样。W2 新增的是贡献者使用的规格、模型、trace 和测试，它们不是运行时输出。

## 6. 明确不在 0.22 内的后续（0.23 已并入，不再另列）

- 未声明动态公告的宿主拒绝策略或信任边界防御；#1040 已延期，因为在没有明确需求时，这一层不是合适的责任方；
- 服务 namespace、全局 allowlist、`rutis.json` row-level policy 或动态名称 isolation 模型；
- 所有 SDK 和 loader restart handling 的 typed wire rejection 及终态 policy-refused row；
- 初版 `SeqCst` marker 实现之外的新 memory-order 方案；
- 在未先核对仓库 release 约定前进行任何 package-version 修改。

这些都是独立设计，不由 0.22 承诺推导出来。
