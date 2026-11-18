# 宿主边界的服务公告策略

[English](design-service-announcement-policy-2026-11-08.en.md)

Status：已被取代，作为宿主选项实现。日期：2026-11-08（2026-11-18 被取代）。
Issue：[ #1040 ](https://github.com/arcships/rutis/issues/1040)。

决定说明：2026-11-18 被取代。#1040 作为宿主选项实现：`SpawnPlugin::undeclared` / `Session::undeclared` / `Mount::undeclared` 接受 `UndeclaredPolicy::{Publish, Refuse, Allow}`；默认 `Publish`，与 Cordis 一致。被拒绝的名字不发布，它所在的行失败并写明名字和行；没有 `HostStatus.announcements` 视图，对端内核的导入也不覆盖（这里不知道对端插件的声明）。[做一个开放插件的宿主](guide/open-host.md#3-管住插件) 和 [保证清单](guarantees.md) 的 `loader/announced-services` 一行是契约。本文其余部分保留原延期提案的原样。

原说明：现有 Cordis 兼容动态公告保持不变；本提案的宿主侧拒绝策略在没有明确需求时暂缓。

## 1. 现状与要解决的问题

runtime row 可以提供不在其声明 `provides` 集合中的服务名。现在 `rows.announce` 会跟随每个此类名称，并用 host-wide `host_key(name)` 发布。没有 `rutis.provides` 的包因此可以公告 `llm`、`storage` 或 `http`；先到的 provider 获胜，低信任 row 可能在真正 provider 启动前占用名称。独立进程可以隔离崩溃，但不会创建服务命名空间。

相关路径是 `row_projection_of`（`crates/rutis-bridge/src/runtime/rows.rs`）、runtime row 加载（`crates/rutis-loader/src/runtime.rs`）、`Process::announce_row_service`（`crates/rutis-bridge/src/runtime/process.rs`）和 peer import（`crates/rutis-bridge/src/session/plugin/services.rs`）。`rutis-host check` 无法知道未来的动态名称。现有 `announces` 能力只表示 runtime 能发送动态公告，不是宿主授权。现有 loader `Status` 与 connection-row 状态彼此分离，因此本设计提议新增宿主级 `AnnouncementDiagnostics` store 和 `HostStatus` 聚合投影，不假装把 peer 拒绝塞进 loader-only status。`HostStatus` 是提案中的新聚合类型，不是当前代码的 `Shown` status 类型。

## 2. 决定

| 决定 | 选择 | 原因；放弃的方案 |
| --- | --- | --- |
| D1 | 增加宿主侧 `undeclared` 策略，仅有 `"publish"` 和 `"refuse"`，默认 `"publish"`。 | `publish` 保持 0.21 行为。`isolate`、`isolate: "*"`、allowlist 和隐式 runtime namespace 不属于这个延期提案：其作用域与可见性尚未定义。 |
| D2 | 通过嵌入 API 配置接收 process/connection 边界：使用 `SpawnPlugin::undeclared(UndeclaredPolicy::...)`。一个边界策略覆盖其所有子 runtime row；逐 row 覆盖延期。`SessionOptions` 继续只表示传输限制。`rutis.json` row 字段及语言 runtime 默认值延期。 | 将 row 字段贯穿 Desired/RowInfo/reconcile 以及所有禁止未知字段的 runtime schema 是独立配置变更；文档声称支持但实际不接受 JSON 字段的做法被放弃。 |
| D3 | 接收侧宿主对本地 runtime 公告、connection boundary 公告和 peer `services.announce` 执行策略。`publish` 保持当前 host-wide key 和先到先得；`refuse` 永不创建 host slot。 | 发送端标志不能决定接收方信任边界。只在发送方配置会遗漏 peer 和 connection boundary import。 |
| D4 | 在延期提案中，`refuse` 是“释放并报告”决策。接收方会在 `provide_at` 前释放公告对象，不创建或替换 slot，并记录结构化宿主诊断。 | 此处不定义 typed wire rejection：Node、Bun、Python、Go 都会在加载成功路径上等待公告完成（Node：`node/rutis-runtime/src/runner.mjs:706-715`；Bun：`bun/rutis-bun/src/runner.ts:424-429`；Python：`python/rutis/rutis/runner.py:676-689`；Go：`go/rutis/internal/runtime/runtime.go:559-570`）。Node/Bun 在公告调用本身附加 `.catch(() => {})`（Node：`node/rutis-runtime/src/runner.mjs:268-274`；Bun：`bun/rutis-bun/src/runner.ts:289-298`），因此公告拒绝会在 row 等待完成前被吞掉。Python/Go 会等待 pending announcement，并只传播显式标记为 registration refusal 的失败。统一的终态/不重启 row 状态需要独立 SDK 与 loader 协议设计。 |
| D5 | 增加错误码 `undeclared-service`、data `{service, row, policy: "refuse"}` 以及有界的宿主级 `HostStatus.announcements` 诊断视图。这里的 `row` 是接收侧 SpawnPlugin/connection boundary 标识（例如 `thirdparty`），不是其中某个子 runtime row；每条记录另带用于去重和清理的不透明 boundary generation。loader status 提供本地记录，connection/peer session 提供各自记录，`rutis-host status` 展示合并视图。精确行是 `thirdparty: refused undeclared service llm (policy: refuse)`。 | 复用 `already-provided` 会把策略拒绝误报成 provider 冲突。无界日志或只存在内存中、无法从状态读取的诊断均放弃。 |

## 3. 精确规则

1. `undeclared` 是宿主授权，不是 manifest 字段。`announces: true` 仍只表示 runtime 可以发送动态公告，不授权发布。
2. 在这个延期提案中，策略原计划设置在接收侧 `SpawnPlugin`/connection boundary；一个边界策略覆盖其所有子 runtime row，提案 API 不支持逐 row 覆盖。未设置时原计划为 `publish`。提案不增加 `rutis.json` 字段、`SessionOptions` 字段或全局 allowlist。非法 API 输入原计划在边界启动前失败，精确输出：`config: undeclared must be "publish" or "refuse"`（错误码 `bad-config`）；此时尚无 boundary identifier。
3. 已声明服务不受影响，其 catalog/instance/`isolate` key 及声明/use 规则保持不变。只有首次公告一个不在发起公告的子 runtime row 声明集合中的名称才判断：
   - `publish`：接受并使用现有 key（通常是 `host_key(name)`）发布；
   - `refuse`：在 `provide_at` 前恰好释放一次对象，记录一条诊断，不创建或替换 host slot。
4. 诊断精确行是 `thirdparty: refused undeclared service llm (policy: refuse)`；code 为 `undeclared-service`；data 为 `{ "service": "llm", "row": "thirdparty", "policy": "refuse" }`，其中 `row` 是接收边界标识。
5. `HostStatus.announcements` 对 `(row, boundary-generation, service, policy)` 元组最多保存一条记录；这里的 `row` 是接收边界标识。同一 boundary generation 的重复公告去重。边界 unload 或 disconnect 时删除记录，新 generation 从空集合开始。提案中的 `rutis-host status` 与 status socket 将在 row/service 状态之后序列化这些记录。没有记录时，提案文本精确为 `announcements: none`；有记录时，文本块是 `announcements:` 加上两空格缩进的 payload 行。提案中的 status-socket JSON 有 `announcements` 数组。每条记录将公开 `row: String`、`service: String`、`policy: UndeclaredPolicy`（序列化为 `publish` 或 `refuse`）、`code: String`、`data: serde_json::Value` 和不透明 `generation: u64`；`generation` 只用于去重/清理。本地 loader 记录与 connection/peer 记录将由 `AnnouncementDiagnostics` 合并到这个宿主级视图。人类可见行是规则 4 的精确文字，embedding caller 将可读取 code/data。这是提议的有界诊断视图，不是当前 status 字段，也不是无界审计日志。
6. 被拒对象恰好释放一次；之后 withdrawal 幂等且不能创建 slot。没有 observer 或 observer 主动过滤时，继续使用现有 ignore-and-release，不写策略诊断；策略拒绝是独立的内部决策。
7. connection boundary 对 runtime 公告使用相同接收策略。peer session 在 `ctx.provide_at(host_key(name), ...)` 前检查接收方策略；`refuse` 时不插入 import 并释放 wrapper。peer 公告调用仍按现有方式成功完成，因此旧 sender 或吞错 sender 可以继续运行；本地 provider 不会因该策略失败。
8. 普通重复提供不变。现有测试仍逐字通过，例如 `node-late: weather is already provided by py-early: rename one of them` 和 `store is already provided by the host: rename it`，code 为 `already-provided`，data 为 `{service, provider}`。peer import 冲突继续使用现有 session 文字 `far:plugin: name is provided here by holder: not registered from far` 及冲突/重启处理；不得改写为 `undeclared-service`。后 provider 失败，先 provider 及其用户继续运行。
9. `rutis-host check` 无法发现未来动态名称，只验证已知配置并继续警告运行时可能出现动态服务；不承诺预检未来拒绝。
10. `isolate: "*"`、隐式 runtime namespace、动态名称的组/实例隔离、全局 allowlist 和 `rutis.json` row 字段均不在范围内。本设计不改变 `inject` 或已声明服务隔离。
11. 这是接纳边界，不是进程沙箱，也不保证代码无法使用已经租借给其 row 的服务。需要更强隔离的宿主必须使用独立 runtime 和 `refuse`；不得把 `publish` 记录为隔离。
12. 将来的协议设计可以加入结构化拒绝及终态 row。必须同时更新 Node/Bun（`node/rutis-runtime/src/runner.mjs`、`bun/rutis-bun/src/runner.ts`）、Python、Go、peer target 及 loader 重启处理，之后才能承诺 row 失败。

## 4. 开发者看到什么

### 插件作者

默认值下现有代码不变。插件仍可在没有 `rutis.provides` 时调用 `ctx.provide('metrics', service)`；首次公告以 host-wide `metrics` 发布。

如果未来实现该提案并通过 `SpawnPlugin::undeclared(UndeclaredPolicy::Refuse)` 配置宿主，对象将被释放，提议的 `rutis-host status` 的 `announcements` 部分将显示：

```
announcements:
  thirdparty: refused undeclared service llm (policy: refuse)
```

记录 payload（不含标题和两个空格缩进）是 `thirdparty: refused undeclared service llm (policy: refuse)`。

如果实现该提案，状态记录的 code 将为 `undeclared-service`，data 为 `{ "service": "llm", "row": "thirdparty", "policy": "refuse" }`，关联当时的 `thirdparty` boundary generation。该策略不会使 boundary 或其子 row 失败或重启。修复：在 `rutis.provides` 声明 `llm`，或为该接收 boundary 选择 `publish`。这是提议的宿主诊断，不是插件异常。

真实冲突仍是另一种错误：`store is already provided by the host: rename it`（code `already-provided`）；修改服务名或已声明服务的 `isolate` 配置。

提议新增概念：一个接收侧宿主策略、一个诊断 code 和一个有界 status 诊断视图。`announces` 的能力含义不变。

### 使用 `rutis-host` 组装插件的人

这个延期提案不向 `rutis.json` 增加 `undeclared` 字段；如果未来重新讨论，配置位置必须由新的设计决定。现有配置和 Cordis 兼容动态公告输出保持不变。

错误与记录：

| 原因 | 逐字输出和 code | 记录 / 修复 |
| --- | --- | --- |
| 提议 API 策略值非法 | `config: undeclared must be "publish" or "refuse"` / `bad-config` | 提议的 boundary builder，在生成标识之前；改为 `publish` 或 `refuse`。 |
| 提议 `refuse` 下公告 `llm` | `thirdparty: refused undeclared service llm (policy: refuse)` / `undeclared-service`，data `{ "service": "llm", "row": "thirdparty", "policy": "refuse" }` | 当前 boundary generation 的提议 `HostStatus.announcements`；声明 `llm` 或选择 `publish`。 |
| 发布名重复 | `store is already provided by the host: rename it` / `already-provided` | 后 provider 的现有失败；改名或使用已声明 `isolate`。 |
| Peer import 冲突 | `far:plugin: name is provided here by holder: not registered from far` / 现有 session conflict | 现有 peer 冲突/重启记录；改名或处理现有 provider。不得改为 `undeclared-service`。 |

### 在 Rust 中嵌入 rutis 的宿主作者

如果实现该提案，`SpawnPlugin::undeclared(UndeclaredPolicy::...)` 将为本地或 connection boundary 提供接收策略，并覆盖其所有子 runtime row。`SessionOptions` 仍只表示传输限制。接收方将对本地 runtime row 和 peer import 应用策略，发送方不能覆盖。`publish` 将保持当前 `host_key(name)` 和先到先得；`refuse` 将释放对象/wrapper，将诊断存入提议的 `HostStatus.announcements`，绝不替换本地 provider。

如果未来实现本提案，宿主将通过 status 接收上述精确行及结构化 code/data，不得将其改名为 `unavailable` 或 `already-provided`，也不得声称远端 row 失败或重启。这些是延期语义，不是 0.22 承诺。

提议新增概念：一个宿主接纳策略、一个诊断 code 和一个有界状态字段；不新增进程或协议 namespace 概念。

### 部署者

独立进程是崩溃/挂起隔离，不是服务名隔离。默认 `publish` 下，未声明服务是 host-wide 且先到先得。低信任 process/connection boundary 应配置 `SpawnPlugin::undeclared(UndeclaredPolicy::Refuse)`，再声明预期公开的服务。如果未来实现，部署者会在提议的 status 视图中看到上述拒绝行；它不是静默私有 fallback，也不会重启 row。

概念预算：这个延期提案原计划增加一条部署规则——在接收 process/connection boundary 选择 `publish` 或 `refuse`。`isolate` 不属于该提案。

## 5. 兼容与迁移

- 默认值是 `publish`；现有 row、Cordis 包和 peer 保持 host-wide 行为及重复 provider 输出。
- `refuse` 仅通过嵌入 API opt-in。现有 `rutis.json` 不会因此失效，也不会被静默重新解释。
- 如果未来重新讨论本提案，wire 兼容性和公告完成语义需要重新审查。0.22 不增加接收侧拒绝行为；现有 Cordis 兼容动态公告保持不变。
- 如果实现该提案，提议的 `HostStatus.announcements` 及 status socket 序列化将是 additive。记录将仅限当前 boundary generation，不是持久审计日志。
- `already-provided` 的文字/code/data、`announces`、`inject`、已声明服务 key 和 `isolate` 语义不变；不恢复 `allowed_services` API。
- 将来的 typed rejection/终态 row 设计需要新文档和迁移审查；本策略不暗示该行为。

## 6. 如果重新启动本提案

目前不计划实现。如果有明确需求重新讨论 #1040，应先确定责任层次，再拆分策略、诊断、bridge、测试和文档工作。任何实现都必须重新审查精确输出、wire 行为、status 生命周期和 SDK 处理；本提案任何部分都不是 0.22 必需项。

## 7. 未来提案的设计哲学检查

如果未来重新讨论本主题，必须将策略归属于正确层次，并保持扩展点窄。动态服务继续保持兼容和按名称工作；没有明确需求，不应引入语言特定权限 hook 或宽 allowlist。JSON/API 边界必须明确，不能假装支持未实现的字段。本检查是未来工作的前置条件，不是 0.22 的实现决定。
