# specs：设计模型

这里放设计阶段的模型检查（质量规范 §11.2）：多个参与者、消息有延迟和交错、错误只在特定时序下出现的协议，在定稿或修改之前用 TLA+ 穷举验证。

每个子目录一份模型，里面有：

- `.tla`：模型本身；
- `.cfg`：TLC 配置，每份对应一种行为或候选设计；
- `expected.txt`：每份配置的预期结果（通过、死锁、某个不变式被违反）；
- `run.sh`：跑全部或部分配置，并和预期比较；
- `README.md` / `README.en.md`：对应哪份设计、做了哪些简化、检查哪些不变式、结果和结论（规范 Q11.2.3）。

设计改了，模型一起改。`tla2tools.jar` 不提交，按各模型 README 里固定的版本下载。CI 的 `specs` 任务在改了 `specs/**` 的 PR 和每晚的完整运行里跑每个模型的 `run.sh`（[CI](../docs/ci.md) §4）；新加一个模型时把目录名加进这个任务的矩阵。

| 目录 | 内容 |
| --- | --- |
| [`cross-runtime-sync/`](cross-runtime-sync/README.md) | 跨运行时同步调用与死锁策略（多语言设计 §五、§九） |
| [`remote-lease/`](remote-lease/README.md) | 远端租约接管、link 重连与注册撤销，含接入点（远程插件 §3.2、§4.4；拨出稿 §五、§六） |
| [`config-schema/`](config-schema/README.md) | 不是模型：配置 Schema 的共用用例表，各语言的实现逐字比较（配置 Schema 设计 §8），由各实现的测试跑 |

---

# specs: design models

Design-stage model checking (quality standard §11.2): protocols with several participants, delayed and interleaved messages, and errors that show up only under particular timings are checked exhaustively with TLA+ before they are settled or changed.

One model per directory, each with the `.tla` model, `.cfg` configurations (one per behaviour or candidate design), `expected.txt` (expected outcome per configuration), `run.sh` (runs them and compares), and a README stating the design it corresponds to, the simplifications, the invariants, the results and the conclusion (standard Q11.2.3). Change the model with the design. Do not commit `tla2tools.jar`; download the version pinned in each README. CI's `specs` job runs each model's `run.sh` on pull requests that change `specs/**` and in the nightly full run ([CI](../docs/ci.en.md) §4); a new model's directory goes into that job's matrix.

| Directory | Subject |
| --- | --- |
| [`cross-runtime-sync/`](cross-runtime-sync/README.en.md) | Cross-runtime synchronous calls and the deadlock policy (multi-language design §5, §9) |
| [`remote-lease/`](remote-lease/README.en.md) | Remote lease takeover, link reconnection and registration revocation, the access point included (remote plugins §3.2, §4.4; dial-out §5, §6) |
| [`config-schema/`](config-schema/README.en.md) | Not a model: the shared case list for config schemas, compared word for word by every implementation (config schema design §8), run by their own tests |
