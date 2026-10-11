# `crates/rutis` 之外依赖真实时钟的测试（设计稿）

[English](design-test-clocks-2026-10-11.en.md)

状态：设计，待评审。日期：2026-10-11。基准：`main` `0941ef4`。Issue：#231（属于 #183）。
依据：[质量规范](quality-standard.md) Q7.1–Q7.5；[质量现状](quality-status.md) K10、P11；#220（已合并，只改 `crates/rutis`）。

范围：`crates/rutis-bridge`、`crates/rutis-loader`、`crates/rutis-host`、`crates/rutis-dsh`、`crates/rutis-dev`、`tests/e2e`，以及 Node（`node/`）、Bun（`bun/`）、Python（`python/`）、Go（`go/`）的测试。`crates/rutis` 已由 #220 处理，不在范围内。
范围外：示例项目 `crates/rutis-agent`、`crates/rutis-cli`（附录 A.3 只列出供参考）；`stress.yml`、`ci.yml` 的改动（本系列预计不需要；只有 Bun 1.4.0 不支持 `bunfig.toml` 的超时设置时，由 #256 在 `bun test` 上加 `--timeout`）；#249、#250 两个偶发失败的根因修复（它们是产品代码的竞态，不是测试时钟问题，见 §二）。

## 一、结论

| # | 结论 | 章节 |
| --- | --- | --- |
| C1 | 按"测试能不能控制时钟"把每处等待分成三类：进程内 tokio（可用暂停时钟）、跨线程或跨进程（只能等可观察的事件）、时间本身就是被测行为（可设置的时长或写明容差） | 三 |
| C2 | bridge 的会话在自己的 OS 线程上读帧（`session/rpc.rs` `Connection::open`），并有自己的后台运行时线程；所以即使用内存通道，bridge 和 loader 里经过会话的测试也**不能**用暂停时钟，只能按第二类处理 | 三 |
| C3 | 新增一个不发布的工作区 crate `rutis-test-support`，放共用的等待辅助函数；#220 的 `still_pending`、`on_thread` 移进去 | 四 |
| C4 | 产品代码只加 2 个小接口（S1、S4），不引入通用的 `Clock` trait | 五 |
| C5 | 小于 10 s 的防挂死上限（C 类）统一换成 `HANG_GUARD`，是机械替换；逐处分析的是 A、B、E、F、G、U 类 | 三 |
| C6 | 分 4 个 PR：bridge、loader、其他 crate、其他语言 | 七 |

## 二、清单汇总

统计口径：只算测试代码（`tests/`、`src/**/tests.rs`、`src/**/testing.rs`、`#[cfg(test)]` 模块、JS/Python/Go 的测试文件和测试夹具），以及测试依赖的产品时长常量。行号以 `0941ef4` 为准。完整清单见附录 A。

分类：

- **A** 用固定时长的 sleep 做同步（等某事发生）
- **B** 固定等待后断言"某事没有发生"
- **C** 防挂死上限或轮询截止时间小于 10 s
- **D** 防挂死上限 ≥ 10 s（符合规范，只计数）
- **E** 时间本身是被测行为（心跳、退避、空闲停止、清理时限）
- **F** 断言耗时上限（或下限）
- **G** 用了随机数但不打印种子
- **H** 夹具里的 sleep 是场景内容（模拟慢操作），不是同步
- **W** sleep 只用来"放大"竞态窗口，断言无论是否命中都成立
- **U** 没有任何上限的等待（挂住时测试永不结束）

| 位置 | A | B | C | D | E | F | G | H | W | U |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| rutis-bridge | 7 | 7 | 21 | 27 | 4 | 4 | 1 | 9 | 5 | 0 |
| rutis-loader | 0 | 14 | 4 | 26 | 5 | 0 | 0 | 5 | 0 | 1 |
| rutis-dsh | 0 | 0 | 5 | 3 | 2 | 0 | 0 | 1 | 0 | 0 |
| rutis-host | 0 | 0 | 0 | 9 | 4 | 0 | 0 | 0 | 0 | 0 |
| rutis-dev | 0 | 0 | 1 | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| tests/e2e | 0 | 0 | 0 | 3 | 0 | 1 | 0 | 5 | 0 | 0 |
| Node（含夹具、baseline） | 1 | 2 | 2 | 1 | 5 | 1 | 0 | 11 | 0 | 3 |
| Bun | 3 | 0 | 1 | 0 | 0 | 0 | 0 | 2 | 0 | 0 |
| Python | 1 | 0 | 11 | 7 | 1 | 1 | 0 | 1 | 0 | 6 |
| Go | 1 | 1 | 7 | 3 | 2 | 0 | 0 | 0 | 0 | 7 |
| **合计** | **13** | **24** | **52** | **79** | **23** | **7** | **1** | **34** | **5** | **17** |

要改的是 A、B、C、E、F、G、U 和部分 W：约 137 处（不含示例项目 `rutis-agent`），其中 52 处 C 类是机械替换。D、H 不改（H 里有 4 处的结果取决于夹具的时长，按 A 处理，见附录 A.9）。

其他发现：

- 没有任何一处用到暂停时钟、虚拟时钟或种子（Rust 的 `start_paused`、Node `mock.timers`、Bun 假定时器、Go `testing/synctest`、Python 的任何假时钟都没有）。
- 等待辅助函数在各文件里各写一份：bridge 的 `eventually` 11 份，loader 的 `eventually` 7 份、`until` 4 份、`Probe::wait_for` 约 9 份，dsh、host 各自一份（示例项目 `rutis-agent` 另有 `soon` 5 份，不改）。上限从 5 s 到 30 s 不等。
- 风险最高、最可能已经在 CI 上偶发的几处：`python/rutis/tests/test_peer.py:146`（在事件循环线程上做阻塞读）、`go/rutis/internal/peer/peer_test.go:116`、`:190`、`node/rutis-runtime/test/serve.test.mjs:26`、`bun/rutis-bun/test/fixtures/crashing-worker.ts:12`、`crates/rutis-loader/tests/runtime_rows.rs:1101`/`:1119`（夹具 300 ms 后退出，测试也等 300 ms）、三套 WebSocket 心跳测试的耗时上限。
- 与已登记的偶发失败的关系：#238（multilang_go 20 s 超时）、#249（go_rows 等不到停止）失败时只说"超时"，看不出卡在哪；#233、#250 是"退出状态"与"通道结束"抢先的竞态。它们都不是测试用了太短的时间，而是产品代码的竞态；但它们都说明失败信息要带出当时的状态（§4.2 的 `eventually_with`）。

## 三、规则

### 3.1 先判断属于哪一类

| 类 | 判断 | 正向等待（等某事发生） | 负向断言（某事没发生） |
| --- | --- | --- | --- |
| 一：进程内 tokio | 被测代码只在测试的运行时里跑：没有子进程、没有 socket、没有 `std::thread`、没有 `spawn_blocking`（或用 `on_thread` 代替）、计时用 `tokio::time` | 同步点（channel、`watch`、`Notify`） | `start_paused` + `still_pending`（#220 的做法） |
| 二：跨线程或跨进程 | 其他情况。bridge 的会话属于这一类（C2） | 等一个可观察的事件；只有在拿不到事件时才轮询，轮询用 `eventually`，上限 10 s | 见 §3.2 |
| 三：时间是被测行为 | 断言的就是"多久之后发生" | 见 §3.3 | 见 §3.3 |

### 3.2 第二类的负向断言

按优先顺序：

1. **标记**：在同一条有序路径上做一件后续的事，等它的结果出现，再断言前一件事没发生。例如 `node.rs:674`：先 `announce(2)`，再宣告另一个服务 `marker`；等 `marker` 出现后断言 `clock` 不在。前提是这条路径确实保证顺序（同一会话的帧按序处理、同一个 loader 的 reconcile 按序执行）。每处改写在 PR 里写明依靠的是哪条顺序保证；如果找不到成文的保证，就不能用标记，改用第 2 种。
2. **可观察的状态**：让被测代码在"决定不做"时留下记录，测试等这条记录。例如 loader 的 `LoaderChanged`、`SelfDisposed` 事件；内存通道的单元测试（白盒）加一个 `#[cfg(test)]` 的"阻塞中的发送者数量"。
3. **弱负向检查**：上面两种都做不到时，保留一段固定等待，但要满足：它只可能漏报（机器慢时什么都没测到），不可能误报；同一行为在别处另有确定性的测试或标记；代码上标 `// clock: weak-negative — <原因>`。附录 A 里标了"弱"的就是预计会留下来的，目前 2 处（`channel/testing.rs:150`、`runtime/spawn.rs:459`），都跨进程；实现中再发现的要在 PR 里说明理由，总数不超过 6。

### 3.3 第三类：时间是被测行为

1. 时长能从测试设置的，测试设置它；不能设置的，加一个小接口（§五）。
2. 被测代码的计时在测试的运行时里（`tokio::time`）的，用暂停时钟。把产品代码里的 `std::time::Instant` 换成 `tokio::time::Instant` 不改变行为（没暂停时两者一样），可以直接做。
3. 计时在别的线程、别的运行时、别的进程里的（WebSocket 心跳、运行时进程、rutis-host 的清理时限），用**容差**：
   - "应当按时发生"的一方，期望时长和防挂死上限之间至少差 10 倍；
   - "不应发生"的一方，等到可观察的次数（例如经过中继的 ping 至少 N 个），而不是等固定时长；
   - **把坏的结果变成挂死**：让"本不该等"的那段等待长到 1 小时，测试只要正常结束就说明没等；挂住时由 10 s 防挂死上限报告。例如 `local_loopback.rs:100`：把 token 等待设成 1 小时（S4），拨号成功就证明没被沉默的连接挡住。
   - "必须超时"的一方让对方永远不结束（夹具不完成清理），而不是"比时限慢一点"。
4. 耗时上限断言（F）一律删除，换成上面的做法；耗时下限（`>=`）只会漏报，可以保留，注明理由（`memory/tests.rs:28`）。

### 3.4 其他规则

- **防挂死上限**：统一用 `HANG_GUARD`（10 s）；需要更长的（冷启动多个运行时）用 `HANG_GUARD * k` 并写原因。没有上限的等待（U）都加上。Bun 的测试默认每个 5 s，改为 ≥ 10 s（`bunfig.toml` `[test] timeout`，1.4.0 不支持时在 CI 命令上加 `--timeout`）。
- **轮询**：只用于拿不到事件的情况；间隔 10 ms；超时时 panic 信息要带出当前状态（`eventually_with`），满足 #238、#249 的要求。
- **随机**：用到随机数的测试从 `RUTIS_SEED` 读种子，没有就生成，打印 `seed: N (replay with RUTIS_SEED=N)`，和 `interleave.rs` 一致。产品代码里的随机（`link.rs` 的退避抖动）在测试里设 `jitter: 0.0`，不改产品代码。
- **夹具的 sleep（H）**：可以保留。但如果测试结果取决于它的时长（夹具 20 ms 后崩溃、300 ms 后退出），按 A 处理：夹具改为收到信号或某事发生之后再行动。
- **放大窗口的 sleep（W）**：只在断言无论是否命中都成立时允许，注明 `// clock: widen — <原因>`；能低成本地让状态可观察的（内存通道），改成等状态。

## 四、共用辅助函数

### 4.1 位置

新增 `crates/rutis-test-support`（`publish = false`，只依赖 `tokio`），各 crate 在 `[dev-dependencies]` 里引用。它不依赖 `rutis`，所以 `crates/rutis` 也能用，不成环。`crates/rutis/tests/common/mod.rs` 改为转出这里的函数，#220 的测试不用改调用。

不放进 `rutis-bridge` 的 `testing` feature：那个 feature 是节点一致性测试（给外部实现用），性质不同。

### 4.2 API

```rust
/// 只防挂死，不测速度（Q7.1.2）。
pub const HANG_GUARD: Duration = Duration::from_secs(10);

/// `f` 在 HANG_GUARD 内完成，否则 panic："timed out: {what}"。
pub async fn within<F: Future>(what: &str, f: F) -> F::Output;

/// 每 10 ms 检查一次，直到返回 Some；超时 panic。
pub async fn eventually<T>(what: &str, check: impl FnMut() -> Option<T>) -> T;
/// 同上，超时时把 `describe()` 的结果写进 panic 信息（#238、#249）。
pub async fn eventually_with<T>(what: &str, check: impl FnMut() -> Option<T>,
                                describe: impl FnOnce() -> String) -> T;
/// std 线程上的版本。
pub fn eventually_blocking<T>(what: &str, check: impl FnMut() -> Option<T>) -> T;
pub fn recv_within<T>(what: &str, rx: &std::sync::mpsc::Receiver<T>) -> T;


/// #220 移入：暂停时钟下判断"仍在等待"；在单独线程上运行阻塞函数。
pub async fn still_pending<F: Future + Unpin>(f: &mut F) -> bool;
pub fn on_thread<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> oneshot::Receiver<T>;
```

各文件里的 `eventually`、`until`、`soon`、`Probe::wait_for` 删除或改为调用这里。`Probe::wait_for` 这类等"某条记录出现"的，保留各自的类型，内部换成 `eventually_with`。

### 4.3 其他语言

每种语言一个小文件，不做包：

| 语言 | 位置 | 内容 |
| --- | --- | --- |
| Node | `node/rutis-runtime/test/support.mjs`（`node/rutis` 复用） | `HANG_GUARD_MS = 10000`、`within(what, promise)`、`eventually(what, check)` |
| Bun | `bun/rutis-bun/test/support.ts` | 同上；`bunfig.toml` 把默认超时设为 ≥ 10 s |
| Python | `python/rutis/tests/support.py` | `HANG_GUARD = 10`、`within`、`eventually`（asyncio 版和线程版） |
| Go | `go/rutis/internal/testwait`（`rutistest` 也用） | `HangGuard = 10 * time.Second`、`Within(t, what, ch)`、`Eventually(t, what, check)` |

Node 22+ 有 `mock.timers`，Bun 有假定时器，但被测的心跳都跨 socket 或进程，假定时器管不到对方，所以本设计不用它们（§十 第 2 项）。

## 五、产品代码要加的接口

| # | 位置 | 改动 | 解决 | 成本 |
| --- | --- | --- | --- | --- |
| S1 | `crates/rutis-bridge/src/link.rs:11,256,361` | `Live::since` 从 `std::time::Instant` 改为 `tokio::time::Instant` | `stable` 之后退避重置可以在暂停时钟下测（只对不经过会话线程的拨号失败路径） | 无：未暂停时两者相同 |
| S4 | `crates/rutis-bridge/src/transport/local/spawn.rs:337` | `TOKEN_WAIT` 改为 `LocalTransport` 的设置项，默认 10 s | `local_loopback.rs:100` 用"把坏结果变成挂死"，删掉 `elapsed < 5s` | 一个字段 |

不改产品代码、在测试里解决的：

- `link.rs` 的退避抖动（G）：集成测试的 `quick()` 设 `jitter: 0.0`。
- `go_rows.rs:358-371` 的空闲停止：`idle` 已可设置（`GoRuntimes::idle`）；测试等运行时的"已停止"状态，上限 `HANG_GUARD`，不再用 800 ms 的固定等待来断言。清扫间隔（100 ms）不变。

不做的：

- **通用 `Clock` trait**：要穿过 bridge 的会话线程、WebSocket 传输自己的运行时和子进程，改动大，而这些地方用容差（§3.3）就够了。
- **WebSocket 心跳注入时钟**（`transport/websocket/connection.rs:196-255`）：心跳在传输自己的多线程运行时里跑（`websocket/mod.rs:129`），测试的暂停时钟管不到。用容差（§十 第 2 项）。
- **rutis-host 的 `PROCESS_EXIT`（3 s）、`KILLED_EXIT`（5 s）**：`signals.rs` 的三处 `--shutdown-timeout 1` 按 §3.3 第 3 条改夹具（必须超时的一方永不结束），不需要改常量。
- **`MANIFEST_TIMEOUT`（30 s）**、loader 的 `generate_id`：没有测试依赖它们的时长或取值。

## 六、怎么确认没有退步

1. **改写后的测试仍能发现问题**（和 #220 相同）：每处 B、E 的改写，在 PR 里写一次"临时把被测代码改坏，测试在新断言处失败"的记录。例如 `node.rs:200`：让 `ExportPlugin` 不等对方提供服务就导出，测试应在标记之后的断言处失败。
2. **重复运行**：每个 PR 在本地把受影响的测试文件跑 10 轮，`--test-threads` 轮换 1、2、核数，结果写在 PR 描述里。
3. **夜间**：bridge、loader 的多进程测试已经由 `stress.yml` 的 `multiprocess` 任务（#245）每晚重复运行；本系列合并后看它有没有新的失败。

## 七、分步

| PR | 内容 | 依赖 |
| --- | --- | --- |
| 1（本 PR） | `rutis-test-support`；`crates/rutis/tests/common` 改为转出；bridge：11 份 `eventually` 换成共用函数，C 类上限改为 `HANG_GUARD`；A、B、W、F（`node.rs:185` 优先，`:200`、`:556`、`:674`，`cancellation.rs`，`projection_lifecycle.rs` / `service_projection.rs` 的 `settle()`，`bun_runtime.rs:311`，内存通道）；E、G（心跳容差，S1、S4，`jitter: 0.0`） | — |
| 2 | loader：14 处 B、`bun_multilang.rs:313` 无上限循环、`runtime_rows.rs:994` 夹具竞态、`go_rows.rs` 空闲停止；共用函数 | 1 |
| 3 | dsh、dev、host、e2e：`signals.rs` 夹具；共用函数 | 1 |
| 4 | Node、Bun、Python、Go 套件和夹具；Bun 每测试超时 ≥ 10 s（`bunfig.toml`） | — |

## 八、风险

| 风险 | 应对 |
| --- | --- |
| 改写后的负向断言变弱，什么都没测到 | §六 第 1 条：每处都做"改坏后失败"的记录 |
| "标记"依赖的顺序其实没有保证，改写引入新的偶发失败 | §3.2 第 1 条：写明依靠的顺序保证，找不到就用第 2、3 种 |
| 暂停时钟遇到 `spawn_blocking` 或 OS 线程不推进或提前推进（#220 遇到过） | 只用于第一类；bridge、loader 经过会话的测试不用（C2）；用 `on_thread` |
| 上限放宽到 10 s，真挂住时测试更晚失败 | 只影响失败时的时长，不影响通过时的时长 |
| 测试用接口进入公开 API | 只有 S4 是新的配置项，有默认值；S1 不改接口 |
| 与并行的工作冲突：#245（`stress.yml`）、#249 / #250（同一批 loader、bridge 测试）、#177（类型分发测试） | 本系列不改 `stress.yml`；#249、#250 的修复先合，本系列 rebase；改到同一文件时在 PR 里互相注明 |
| Go 的 `testing/synctest` 要 Go 1.25（`go.mod` 是 1.24） | 本设计不用它（§十 第 3 项） |

## 九、验收标准（可自动检查）

1. 范围内的测试文件里，没有字面时长小于 10 s 的 `timeout(` / `recv_timeout(` / `settimeout(` / `WithTimeout(`（`rg` 检查，实现时写进 PR 描述）。
2. 范围内没有 `elapsed() <`、`Date.now() - … <`、`time.monotonic() - … <` 形式的耗时上限断言。
3. `rg 'fn eventually|fn until' crates tests` 只在 `crates/rutis-test-support` 里找到定义（示例项目除外）。
4. 附录 A 里的 A、B、F、U 类，每处都已改写，或标为"弱"并写明原因（不超过 6 处）。
5. Bun 套件的每测试超时 ≥ 10 s。
6. 每个 PR 描述里有"改坏后失败"的记录和 10 轮重复运行的结果。

## 十、维护者的决定（2026-10-11）

- 允许"弱负向检查"（§3.2 第 3 种）：只限跨进程、没有顺序保证的地方，标注 `// clock: weak-negative — <原因>`，不超过 6 处。
- 四套 WebSocket 心跳测试用容差：ping 与 timeout 至少差 10 倍，"保持连接"一方等经过中继的 ping 次数，删掉耗时上限；不给传输注入时钟。
- Go 暂不升到 1.25，不用 `testing/synctest`。
- 共用函数放新 crate `rutis-test-support`（不发布，只做 dev-dependency；实现时用 `cargo publish --dry-run` 确认）；分成 4 个 PR，本 PR 是第 1 个，写 `Refs #231`，最后一个写 `Closes #231`；示例项目 `rutis-agent` 不在范围内；不做检查脚本。

## 附录 A：完整清单

"类"一列用 §二 的字母；"进程"一列：内 = 进程内 tokio，线 = 进程内但跨 OS 线程（含 bridge 会话），外 = 跨进程或真实 socket。"弱"表示预计按 §3.2 第 3 种保留。

### A.1 rutis-bridge

| 位置 | 做什么 | 类 | 进程 | 改法 |
| --- | --- | --- | --- | --- |
| `tests/node.rs:185` | sleep 200 ms 等导入完成，再取本地服务 | A | 线 | 等对方 `ImportPlugin` 的状态（导入完成的事件）或宣告标记，再断言本地 clock 仍是 7 |
| `tests/node.rs:200` | 100 ms 后断言没有导入 | B | 线 | 标记：对方再导出一个已提供的服务，等它出现后断言 `clock` 不在 |
| `tests/node.rs:556` | 200 ms 后断言行没有启动（等注入） | B | 线 | 等 loader 报告该行为 Pending（行状态），再断言 `starts == 1` |
| `tests/node.rs:570` | `timeout(5s, pending)` | C | 线 | `within` |
| `tests/node.rs:674` | 100 ms 后断言旧宣告被丢弃 | B | 线 | 标记：在 `announce(2)` 后宣告另一个服务，等它出现 |
| `tests/cancellation.rs:42` | 100 ms 超时当作"调用不会自己结束"并借此丢弃调用 | B | 外 | 等夹具报告 `wait` 已开始，再丢弃；"不会自己结束"由后面的 `aborted` 断言覆盖 |
| `tests/cancellation.rs:51-57`、`:97-101` | 50 × 10 ms 轮询 | C | 外 | `eventually` |
| `tests/cancellation.rs:93` | 一次 poll 后 sleep 100 ms，让回复先到 | W | 外 | 保留并标注；或等连接上待处理回复数 > 0 |
| `tests/projection_lifecycle.rs:94` `settle()`（9 处调用） | yield + 共 100 ms sleep 后断言 | A（:335 为 B） | 外 | 正向的换成 `eventually`；`:335`（发布被拒）等拒绝事件或用 `:339` 之后的正向断言覆盖 |
| `tests/projection_lifecycle.rs:217` | `timeout(5s, dispose)` | C | 外 | `within` |
| `tests/service_projection.rs:94` `settle()`（3 处） | 同上 | A | 外 | `eventually` |
| `tests/bun_runtime.rs:311` | sleep 100 ms 让 `pass` 进入 Bun 的闸门 | A | 外 | `Gate` 被调用时先发 `Signal`，测试等它 |
| `tests/bun_runtime.rs:229` | 断言"不下载"耗时 < 5 s | F | 外 | 删除上限；用不可达的 registry 环境变量，保留"没有 node_modules/left-pad" |
| `tests/bun_runtime.rs:314` | 5 s | C | 外 | `within` |
| `tests/go_runtime.rs:215` | 100 ms 超时用来丢弃调用 | A | 外 | 等夹具报告 `wait` 已开始，再丢弃 |
| `tests/process_exit.rs:45`、`:146` | 2 s | C | 外 | `within` |
| `tests/rpc_callbacks.rs:514` | 2 s | C | 外 | `within` |
| `tests/event_forwarding.rs:91-96`、`:182-187` | 100 × 10 ms 轮询 | C | 外 | `eventually` |
| `tests/websocket.rs:103,276,330,381,518,560` | `recv_timeout(5s)` | C | 外 | `recv_within` |
| `tests/websocket.rs:505-533` | 心跳：ping 100 / timeout 400；sleep 800 ms 断言仍连着；断言断开耗时 < 3 s | E、F | 外 | §3.3 容差：timeout 改 1 s；中继计数，等 ≥ 15 个 ping 通过后断言仍连着；删除耗时上限 |
| `tests/websocket_multihop.rs:148-170` | 主线程同步等待 1.5 s 时心跳仍被应答 | E | 外 | 同上：ping 与 timeout 差 10 倍，同步调用时长 ≥ 2 × timeout |
| `tests/local_loopback.rs:100-121` | 沉默连接不挡住进程：耗时 < 5 s | E、F | 外 | S4：token 等待设 1 小时，删除耗时断言 |
| `tests/link.rs:29` | `quick()` 握手 2 s | C | 线 | 握手 10 s；`quick()` 加 `jitter: 0.0` |
| `tests/memory_mux.rs:511`、`:591` | 5 s | C | 线 | `recv_within` |
| `src/transport/memory/tests.rs:28` | 延迟故障：耗时 ≥ 50 ms | F（下限） | 线 | 保留，注明只是下限 |
| `src/transport/memory/tests.rs:41` | 半开时 100 ms 内没收到 | B | 线 | 删除：后面"收到的第一条是 `after`"已经证明 `lost` 没送达 |
| `src/transport/memory/tests.rs:78` | 缓冲满时 100 ms 内发送未返回 | B | 线 | `#[cfg(test)]` 的阻塞发送者计数，等它为 1 |
| `src/transport/memory/tests.rs:95` | sleep 50 ms 等两个线程进入阻塞 | W | 线 | 同上，等阻塞计数 |
| `src/channel/testing.rs:22` `PATIENCE` | 5 s | C | 外 | `HANG_GUARD` |
| `src/channel/testing.rs:88`、`:110`、`:137` | sleep 让对方先阻塞或先写满 | W | 外 | 保留并标注（一致性测试跨所有通道，无法统一观察阻塞） |
| `src/channel/testing.rs:150` | sleep 20 ms "让消息先离开再关闭" | A | 外 | 弱：各通道的 `close` 语义不保证已发送的消息送达时需要在规范里写明；先查各通道实现，能保证的删除 sleep，不能保证的标注 |
| `src/session/testing.rs:299` | 50 ms 超时用来丢弃异步调用 | A | 线 | 等对方报告调用已开始 |
| `src/session/testing.rs:302-307` | 100 × 20 ms 轮询 | C | 线 | `eventually_blocking` |
| `src/runtime/spawn.rs:459` | 2 s 内通道不结束（进程还在） | B | 外 | 弱：没有可观察的"正在等退出"事件；保留并标注，正向部分已断言退出状态 |
| `src/link.rs:540-550` | 退避单元测试，种子来自系统时间 | G | 内 | 断言对任意种子都成立的性质（每次延迟在范围内、不超过上限），不依赖具体序列；不改产品代码 |
| 夹具 sleep（`rpc_callbacks.rs:160,192,446`、`event_forwarding.rs:49`、`node.rs:359`、`session/testing.rs:56`、`python_runtime.rs:32`、`bun_runtime.rs:30`、`websocket_multihop.rs:158`） | 模拟慢操作 | H | — | 不改 |
| `eventually` 副本 11 份（`node.rs:24`、`link.rs:34`、`multihop.rs:26`、`websocket_link.rs:22`、`websocket_soak.rs:29`、`go_runtime.rs:72`、`bun_runtime.rs:75`、`python_runtime.rs:85`、`row_services.rs:77`、`src/runtime/testing.rs:48`、`src/testing.rs:61`）等 | ≥ 10 s | D | — | 第 1 个 PR 换成共用函数 |
| `local_soak.rs`、`websocket_soak.rs` 的运行时长 | 时长是长时间测试的输入 | — | 外 | 不改 |

### A.2 rutis-loader

| 位置 | 做什么 | 类 | 进程 | 改法 |
| --- | --- | --- | --- | --- |
| `tests/peer_rows.rs:178` | 100 ms 后断言只启动一次 | B | 线 | 标记：再加一行到同一主机，等它启动后断言 `starts == 1` |
| `tests/runtime_rows.rs:145`、`:271` | 100 ms 后断言 | B | 外 | 标记行或 `LoaderChanged` 事件 |
| `tests/runtime_rows.rs:618` | 200 ms | B | 外 | 同上 |
| `tests/runtime_rows.rs:1101`、`:1119` | 300 ms，与夹具 `:994` 的 `setTimeout(quit, 300)` 赛跑 | B（结果取决于夹具时长） | 外 | 夹具改为收到消息后退出；测试等进程退出事件 |
| `tests/lifecycle.rs:227` | 50 ms 后断言没有 `SelfDisposed` | B | 内 | 暂停时钟 + `still_pending`（最适合的一处） |
| `tests/multilang.rs:430` | 200 ms | B | 外 | 等 `meta["inject"]`（文件里其他用例的做法） |
| `tests/multilang_go.rs:402` | 500 ms | B | 外 | 同上 |
| `tests/cordis_node.rs:193`、`:273`、`:298` | 各 300 ms | B | 外 | 标记：同一会话上的后续调用返回后断言 |
| `tests/go_rows.rs:336` | 300 ms | B | 外 | 标记或运行时状态 |
| `tests/go_rows.rs:358-371` | 空闲 300 ms、sleep 800 ms 后断言仍在运行 | E、B | 外 | "有行在用时不停"改为在一次调用成功之后检查状态仍是运行；移除行后等运行时"已停止"，上限 `HANG_GUARD`；不改产品代码 |
| `tests/go_rows.rs:516` | 故意 1 ms 超时 | E | 外 | 保留（断言超时这件事本身，对方永不回复），注明 |
| `tests/bun_multilang.rs:313-320` | `loop { sleep(10ms) }` 无上限 | U | 外 | `eventually` |
| `tests/instances.rs:390`、`tests/lifecycle.rs:154` | `eventually` 5 s | C | — | 共用函数 |
| `tests/loader.rs:568`、`:592` | 5 s | C | — | `within` |
| `tests/leases.rs:138`、`:377`、`:400` | 600 / 400 ms 停止延迟（夹具） | E | 外 | 夹具改为由测试放行；`:374` 提到的抖动：`quick()` 设 `jitter: 0.0` |
| `tests/fixtures/go/cmd/logger/main.go:34` | 夹具计时 | E | 外 | 同上，由测试放行 |
| 夹具 sleep 其余 4 处 | 模拟慢操作 | H | — | 不改 |
| 等待辅助函数：`eventually` 7 份、`until` 4 份、`Probe::wait_for` 约 9 份、`multilang_go` 的 `Fixture::until`（超时时输出状态，保留为 `eventually_with` 的用法范例） | 10–30 s | D | — | 第 2 个 PR 换成共用函数 |

### A.3 rutis-agent（示例项目，不在范围内，只列出供参考）

| 位置 | 做什么 | 类 | 进程 | 改法 |
| --- | --- | --- | --- | --- |
| `tests/integration.rs:160`、`:171` | 30 ms 后断言仍是 Pending | B | 内 | 暂停时钟 + `still_pending`（先确认没有 OS 线程）；或用文件里已有的 `wait_state`（`:38`，`watch` 通道）等状态变化 |
| `tests/integration.rs:459` | 50 ms 后断言没有残留监听器 | B | 内 | 同上 |
| `src/tools/mod.rs:377` | 400 ms 后断言后台子进程没留下文件 | B | 外 | 先确认进程组已不存在（`kill(-pgid, 0)` 返回 `ESRCH`），再断言没有文件 |
| `src/tools/bash.rs:334-339`、`:356-363` | 同上 | B | 外 | 同上 |
| `soon`（5 s）：`tests/integration.rs:33`、`tests/unit_loop.rs:168`、`src/driver.rs:755` 等 5 份 | 5 s | C | — | 共用函数 |
| `src/tools/mod.rs:264`（2 s）、`:315`（1 s）、`:319`（3 s，对比 `CANCEL_JOIN_GRACE` 2 s）、`:368`（2 s 轮询） | 短上限，且与产品常量相比 | C、E | 外 | S5：测试设置时限；上限改 `HANG_GUARD` |
| `tests/minimal_tools.rs:114`、`:123` | 耗时 < 5 s | F | 外 | 删除，换成"取消后进程组不存在" |
| 夹具 sleep 8 处 | 模拟慢命令 | H | — | 不改 |

### A.4 rutis-dsh、rutis-host、rutis-dev、tests/e2e

| 位置 | 做什么 | 类 | 改法 |
| --- | --- | --- | --- |
| `crates/rutis-dsh/tests/profile_loader.rs:257`、`:267`、`:283` | 5 s 轮询 | C | `eventually` |
| `crates/rutis-dsh/tests/stream.rs:86`、`tests/web.rs:127` | 5 s | C | `within` |
| `crates/rutis-dsh/src/profile/lock.rs:149`、`:154` | 锁等待、监视间隔（已可配置） | E | 测试设置时长，按 §3.3 第 3 条 |
| `crates/rutis-host/tests/signals.rs:384`、`:413`、`:456` | `--shutdown-timeout 1` | E | 必须超时的用例夹具永不完成清理；必须按时完成的用例用默认 10 s |
| `crates/rutis-host/src/main.rs:273`（400 ms）、`src/status.rs:53`（200 ms） | 产品的轮询间隔 | — | 不改（测试只等结果，不依赖间隔） |
| `crates/rutis-dev/tests/channel.rs:97` | 每行 5 s（真实 unix socket） | C | `HANG_GUARD` |
| `tests/e2e/src/residue.rs:356`、`:359` | 宽松的耗时检查 | F | 改为只防挂死 |
| `tests/e2e/src/lib.rs:55` `hang_guard()` 等 | 30 s 或 `RUTIS_E2E_TIMEOUT` | D | 不改；e2e 继续用自己的（黑盒，独立 crate） |

### A.5 Node（`node/`）

| 位置 | 做什么 | 类 | 改法 |
| --- | --- | --- | --- |
| `node/rutis-runtime/test/websocket.test.mjs:7` | `quick`：ping 50 / timeout 200 / handshake 2000 | E、C | ping 与 timeout 差 10 倍；handshake 10 s |
| `websocket.test.mjs:63` | 1009 用例在 > 200 ms 卡顿时会误报 | E | 同上 |
| `websocket.test.mjs:95` | `setTimeout(500)` 断言超过 timeout 仍连着 | E | 计数 pong |
| `websocket.test.mjs:99-102` | 耗时 < 2000 | F | 删除 |
| `websocket.test.mjs` `closedWith` / `next()` | 无上限 | U | `within` |
| `node/rutis-runtime/test/serve.test.mjs:26` | `setTimeout(100)` 后断言 | B | 等服务端 socket 关闭事件 |
| `serve.test.mjs:56-62`、`handshake.test.mjs:20` | 无上限 | U | `within` |
| `serve.test.mjs:66` | 10 s 竞赛 | D | 不改 |
| `node/rutis-runtime/test/fixtures/rust-mount.test.mjs:21`、`:26` | 1 ms 轮询、2 s 上限；仓库里没有使用者 | A、C | 确认无人使用后删除，否则改 `eventually` |
| `node/baseline/native.mjs:24` | `Promise.race(fibers, 500ms)` 报告 "unavailable" | B | 等 fiber 的状态事件；或标注为基准脚本（不是测试）后移出范围 |
| `fixtures/sync-heartbeat.mjs:7`（对应 `websocket_multihop.rs`）、`rpc-client.mjs:12`、`cordis-node.mjs:14`（重试 50 / 500） | 夹具计时 | E | 随对应 Rust 测试一起按 §3.3 调整 |
| 其余夹具 sleep 11 处 | 模拟慢操作 | H | 不改 |

### A.6 Bun（`bun/rutis-bun`）

| 位置 | 做什么 | 类 | 改法 |
| --- | --- | --- | --- |
| `test/channel.test.ts:34` | `Bun.sleep(10)` 把读拆成两次 | A | 等收到 `one` 后再写第二段 |
| `test/client.test.ts:26` | `setTimeout(20)` 后再调用 | A | 收到 `hello` 后再调用 |
| `test/fixtures/crashing-worker.ts:12` | 20 ms 后崩溃，与连接赛跑 | A（结果取决于夹具时长） | 连接完成（或收到指令）后崩溃 |
| `test/fixtures/channel-relay.ts:15`（`crates/rutis-bridge/tests/bun_channel.rs:41` 使用） | 50 ms 后退出 | A | 通道关闭或数据发完后退出 |
| `bun test` 默认每测试 5 s | 唯一的防挂死上限 | C | ≥ 10 s（§3.4） |
| 夹具 sleep 2 处 | 模拟慢操作 | H | 不改 |

### A.7 Python（`python/rutis/tests`）

| 位置 | 做什么 | 类 | 改法 |
| --- | --- | --- | --- |
| `test_peer.py:146` | `asyncio.sleep(0.05)` 后在事件循环线程上**阻塞**读 `lines.readline()` | A | `await asyncio.to_thread(lines.readline)`（`:119` 已是这样） |
| `test_peer.py:17`、`:134` | `settimeout(5)` | C | 10 s |
| `test_websocket.py:42,55,59,76,151,153,159,167,178` | 5 s 的 get / recv；`:153`、`:167` 超时抛 `TimeoutError` 而不是期望的 `ConnectionClosed` | C | `HANG_GUARD`；`:153`、`:167` 超时时给出明确失败信息 |
| `test_websocket.py:171` | `RUTIS_HEARTBEAT=100,400` | E | §3.3 容差 |
| `test_websocket.py:182-185` | 耗时 < 3 | F | 删除 |
| `test_websocket.py:57,61,74,78,180,184` | 无上限的 `channel.recv()` | U | 加上限 |
| 其余（7 处 ≥ 10 s、1 处夹具） | — | D、H | 不改 |

### A.8 Go（`go/rutis`）

| 位置 | 做什么 | 类 | 改法 |
| --- | --- | --- | --- |
| `internal/peer/peer_test.go:42`、`:112`、`:271` | 5 s | C | `testwait.HangGuard` |
| `peer_test.go:105` | 50 ms 的 context 超时，期望 `DeadlineExceeded` | E | 对方永不回复，保留短超时并注明（被测的就是超时） |
| `peer_test.go:116` | `Sleep(50ms)` 后断言 `host.Err() == nil` | B | 标记：再做一次往返调用，成功后断言 |
| `peer_test.go:155-168` | GC 轮询 10 ms、上限 5 s | C | 10 s |
| `peer_test.go:190` | `Sleep(50ms)` 后检查通知顺序（这样检查不出顺序） | A | 等第二条通知到达后再比较顺序 |
| `peer_test.go:35` 等 | 无上限 | U | 加上限 |
| `listen_test.go:146`、`:184` | 5 s | C | 10 s |
| `listen_test.go:247` | `RUTIS_HEARTBEAT=20,150`（ticker 在 `listen.go:36-46,208-222`） | E | §3.3 容差 |
| `listen_test.go:197,210,237,276`、`rutistest/rutistest.go:86` | 无上限 | U | 加上限 |
| `rutistest/rutistest.go:207-221` `Service()` | 等 2 s，并在 `l.changed` 之外每 10 ms 多余地轮询 | C | 只等 `l.changed`，上限 10 s |
| `rutistest.go:87`、`:231`、`:240` | ≥ 10 s | D | 不改 |

### A.9 结果取决于夹具时长的 H（按 A 处理）

`crates/rutis-loader/tests/runtime_rows.rs:994`、`bun/rutis-bun/test/fixtures/crashing-worker.ts:12`、`bun/rutis-bun/test/fixtures/channel-relay.ts:15`、`crates/rutis-loader/tests/leases.rs:138`。
