# specs：设计模型

这里放设计阶段的模型检查（质量规范 §11.2）：多个参与者、消息有延迟和交错、错误只在特定时序下出现的协议，在定稿或修改之前用 TLA+ 穷举验证。

每个子目录一份模型，里面有：

- `.tla`：模型本身；
- `.cfg`：TLC 配置，每份对应一种行为或候选设计；
- `expected.txt`：每份配置的预期结果（通过、死锁、某个不变式被违反）；
- `run.sh`：默认只跑正面不变式配置（跳过 `*-mut-*.cfg` 变异配置；改模型需要验证不变式非空时传 `--mutations` 或指定配置名），并和预期比较；
- `README.md` / `README.en.md`：对应哪份设计、做了哪些简化、检查哪些不变式、结果和结论（规范 Q11.2.3）。

设计改了，模型一起改。`tla2tools.jar` 不提交，按各模型 README 里固定的版本下载。CI 的 `specs` 任务只在改了 `specs/**` 的 PR / 每晚范围和手动运行时跑每个模型的 `run.sh`（[CI](../docs/ci.md) §4）；新加一个模型时把目录名加进这个任务的矩阵。

| 目录 | 内容 |
| --- | --- |
| [`cross-runtime-sync/`](cross-runtime-sync/README.md) | 跨运行时同步调用与死锁策略：等待图、宿主 worker、serial/parallel/async waterfall、取消与迟到回复、Cordis 节点与跨主机环（多语言设计 §五、§九；#614） |
| [`remote-lease/`](remote-lease/README.md) | 远端租约接管、清理截止、应用托管起停、link 重连与注册撤销，含接入点与 Swift/Kotlin 对应表（远程插件 §3.2、§4.4；拨出稿 §五、§六；#615） |
| [`row-lifecycle/`](row-lifecycle/README.md) | 行生命周期：加载/卸载、代次与两阶段、撞名拒绝、服务公告/版本/撤下、宿主服务租约、依赖、崩溃重启、两运行时、`rows.update`、投影关闭（#625；会话追踪契约 §3.0；远程会话 §4.2、§4.3） |
| [`event-close/`](event-close/README.md) | 事件关闭接纳：子树 marker 先于 closing 发布、已接受调用 drain、cleanup 恰好一次（[事件关闭接纳设计](../docs/design-event-close-admission-2026-11-08.md)；#1025） |
| [`events/`](events/README.md) | 统一事件协议：登记表、分发路径、emit 队列、同步等待、link offers 与公告 epoch（[事件协议](../docs/design-events-protocol-2026-10-18.md) §4–§9；#613） |
| [`peer-rows/`](peer-rows/README.md) | 节点托管行：行键/实例、`Host.loaded`、旧会话清理与新会话装载、迟到 `rows.ended`、`describe/schema/load/update/unload`、`link.offers` epoch、两节点代装与服务公告（远程会话 §4.2、§4.3；#640） |
| [`call-lifecycle/`](call-lifecycle/README.md) | 调用生命周期：调用号、答复、取消与迟到答复、引用计数/参数引用/钉住目标、会话关闭与关闭后帧、出站积压、取消表规则、跨会话中继（会话协议 §5–§7；#626） |
| [`config-schema/`](config-schema/README.md) | 不是模型：配置 Schema 的共用用例表，各语言的实现逐字比较（配置 Schema 设计 §8），由各实现的测试跑 |

---

# specs: design models

Design-stage model checking (quality standard §11.2): protocols with several participants, delayed and interleaved messages, and errors that show up only under particular timings are checked exhaustively with TLA+ before they are settled or changed.

One model per directory, each with the `.tla` model, `.cfg` configurations (one per behaviour or candidate design), `expected.txt` (expected outcome per configuration), `run.sh` (runs the non-mutation configurations by default; pass `--mutations` or config names when changing a model), and a README stating the design it corresponds to, the simplifications, the invariants, the results and the conclusion (standard Q11.2.3). Change the model with the design. Do not commit `tla2tools.jar`; download the version pinned in each README. CI's `specs` job runs each model's `run.sh` only when `specs/**` changes or when invoked by hand ([CI](../docs/ci.en.md) §4); a new model's directory goes into that job's matrix.

| Directory | Subject |
| --- | --- |
| [`cross-runtime-sync/`](cross-runtime-sync/README.en.md) | Cross-runtime synchronous calls and the deadlock policy: the wait graph, host workers, serial/parallel/async waterfall, cancel and late replies, Cordis nodes and cross-host cycles (multi-language design §5, §9; #614) |
| [`remote-lease/`](remote-lease/README.en.md) | Remote lease takeover, cleanup deadline, app-hosting start/stop, link reconnection and registration revocation, the access point and the Swift/Kotlin correspondence included (remote plugins §3.2, §4.4; dial-out §5, §6; #615) |
| [`row-lifecycle/`](row-lifecycle/README.en.md) | Row lifecycle: load/unload, generations and two stages, name collisions, service announcements/versions/withdrawal, host service leases, dependencies, crash/restart, two runtimes, `rows.update`, projection close (#625; session trace contract §3.0; remote sessions §4.2, §4.3) |
| [`event-close/`](event-close/README.en.md) | Event close admission: a subtree marker is published before closing, accepted calls drain, cleanup runs once (event close admission design; #1025) |
| [`events/`](events/README.en.md) | Unified event protocol: the registration table, dispatch paths, the emit queue, synchronous waiting, link offers and announcement epochs (events protocol §4–§9; #613) |
| [`peer-rows/`](peer-rows/README.en.md) | Peer-hosted rows: the row key/instance, `Host.loaded`, old-session cleanup before a new session loads, late `rows.ended`, `describe/schema/load/update/unload`, `link.offers` epochs, two-node loading and service announcements (remote sessions §4.2, §4.3; #640) |
| [`call-lifecycle/`](call-lifecycle/README.en.md) | Call lifecycle: call ids, replies, cancel and late replies, reference counts/argument references/pinned targets, session close and frames after it, outbound backlog, cancel-table rules, cross-session relay (session protocol §5–§7; #626) |
| [`config-schema/`](config-schema/README.en.md) | Not a model: the shared case list for config schemas, compared word for word by every implementation (config schema design §8), run by their own tests |
