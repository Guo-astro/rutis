# rutis 测试"按时间等待"审计报告

- 审计对象：`/Users/eric8810/Code/rutis`，分支 `test/1373-test-reduction`，提交 `2d245d3a`
- 审计日期：2026-11-21
- 审计性质：只读审计。未运行任何 cargo / swift / go / gradle / bun / node / python 测试，未修改任何被跟踪文件。

> 说明：本报告基于提交 `2d245d3a`（PR #1386 的测试合并之后）的树；文中的 `file:line` 都对应这个提交。

---

## 一、结论

**1. 仓库已经有一套正确的等待规范，问题不在"没有规范"，而在"没有全部用上"。**

`crates/rutis-test-support` 明确定义了 `HANG_GUARD`、`scaled`、`within`、`eventually`、`until`、`still_pending`，并在注释里写死"超时只防挂死，永不代替事件"。六种语言各有一份等价实现：

| 语言 | 规范实现 |
| --- | --- |
| Rust | `crates/rutis-test-support/src/lib.rs:28`（`HANG_GUARD`）、`:61`（`within`）、`:67`（`eventually`）、`:72`（`until`）、`:52`（`scaled`） |
| Go | `go/rutis/internal/testwait/testwait.go:18`（`HangGuard`）、`:24`（`Within`）、`:51`（`Eventually`） |
| Node | `node/rutis-runtime/test/support.mjs:8`（`HANG_GUARD_MS`）、`:15`（`within`）、`:32`（`eventually`） |
| Bun | `bun/rutis-bun/test/support.ts:13`、`:23`、`:39` |
| Python | `python/rutis/tests/support.py:29`（`within`）、`:38`（`eventually`）、`:51`（`eventually_blocking`） |
| Swift | `swift/rutis/Sources/RutisTest/RutisTest.swift:147`（`settle`）、`:200`（`started`） |

**2. 审计共发现 336 处按时间等待的调用点**（Rust 144、Swift 61、Kotlin 37、Go 16、Node/Bun 45、Python 33），另有 13 处 shell/工作流 sleep。分类结果：**A 74、B 116、C 5、D 21、E 3**，其余 117 处为夹具场景内容（H）、产品代码（P）或等待辅助函数自身（S），不属于测试等待。

**3. 真正会导致偶发失败的是 E 类，共 9 项**（3 项来自 sleep 调用点，6 项来自进程级全局状态），其中 5 项是本轮合并测试二进制后被放大的：

- **进程环境变量被并发修改**：`RUTIS_SHUTDOWN_TIMEOUT` 在 `core` 与 `rows` 套件里被 `set_var`，而 `shutdown_deadline()` 每次调用都重新读取 env（`crates/rutis/src/fiber.rs:1344-1351`），因此同二进制内任何其它测试的卸载时限都会被改成 0.3 s。
- **`RUTIS_TOKEN_*` 的加锁不一致**：`crates/rutis-host/tests/dial/dial_conformance.rs:245` 不加锁写入，而同套件的 `lease_e2e` / `swift_app` / `kotlin` 会通过 `common::environment()`（`crates/rutis-host/tests/common/mod.rs:33-44`）**删除**所有 `RUTIS_TOKEN_*`。两者交错即失败。
- **两处 sleep 的断言取决于竞态窗口是否输/赢**：`still_waiting.rs:124`（50 ms 抢在 300 ms 定时器前）与 `unload_deadline.rs:49`（50 ms 内必须已有 listener 在跑）。

**4. B 类 116 处数量最大，但不是不可用**：其中绝大多数是"有截止时间的轮询"，语义正确，只是各自手写了一份循环（测试目录里仍有 27 个本地 `eventually`/`until`/`wait_for`/`soon` 定义）。这些属于**机械替换**，风险低、收益是可读性与失败信息（`eventually_with` 会带出当时的 state，正好对应 #238/#249 的抱怨）。

**5. 按语言看，Rust 的风险集中度最高**（并行线程模型 + 合并后的 18 个套件 + 进程级 env），Kotlin/Swift 的风险主要是**耗时上界断言**（`< 100 ms`、`< 1000 ms`）与**固定等待正事件**；Go、Python、Node/Bun 整体健康，问题零散。

**6. `RUTIS_TEST_TIME_SCALE` 只放长"防挂死上限"，不放长裸 sleep**（`crates/rutis-test-support/src/lib.rs:33-54`，CI 在自托管 runner 上默认设 3，见 `.github/workflows/ci.yml:339` 等）。因此凡是"裸 sleep 去追一个未缩放的固定时长"的写法，在负载下只会更糟——这是第 3 条两个 E 项在 CI 上必然复现的机制。

---

## 二、范围与方法

### 2.1 范围

纳入：

- 所有集成测试与单元测试中的等待：`crates/*/tests/**`、`crates/*/src/**/tests*.rs`、`crates/*/src/**/testing*.rs`、`tests/e2e/**`。
- 测试夹具（fixture）中与测试结果相关的 sleep。
- 六种语言 SDK 的测试与测试工具：Rust、Swift、Kotlin、Go、Node、Bun、Python。
- `tools/**` 与 `.github/workflows/*.yml` 中基于 sleep 的等待。

不纳入：

- 产品代码里的 sleep（如 `swift/rutis/Sources/Rutis/Remote/App.swift` 的重试、`go/rutis/internal/runtime/listen.go:573` 的宽限关闭、`kotlin/.../Peer.kt:630`）。
- 文档里的代码片段。

### 2.2 检索式

按要求逐一执行，并补充了 #1373 相关的全局状态检索：

```
tokio::time::sleep   std::thread::sleep   thread::sleep   Task.sleep   asyncAfter
time.Sleep           time.After           Thread.sleep    delay(       setTimeout
asyncio.sleep        time.sleep           eventually      until(       within(
within_blocking      HANG_GUARD           scaled(         Duration::from_millis
Duration::from_secs  start_paused         RUTIS_TEST_TIME_SCALE
running_processes    processes_ended      DialedSocket.live   set_var    set_current_dir
fixed ports (127.0.0.1:<port>, :<port>)   temp dirs (/tmp, temp_dir)
```

命中量（全树、排除 `node_modules`/`build`/`.build`/`.gradle`）：

| 检索式 | 命中 | 说明 |
| --- | --- | --- |
| `tokio::time::sleep` | 173 | Rust 测试与产品代码 |
| `std::thread::sleep` | 61 | 同上 |
| `Task\.sleep` | 51 | Swift |
| `asyncAfter` | 10 | Swift |
| `Thread\.sleep` | 34 | Kotlin |
| `delay(` | 23 | Kotlin |
| `time\.Sleep` | 14 | Go |
| `time\.After` | 8 | Go |
| `setTimeout` | 114 | Node/Bun（多数在产品代码与夹具里） |
| `asyncio\.sleep` | 54 | Python |
| `time\.sleep` | 17 | Python |
| `HANG_GUARD` | 283 | 规范设施，正面信号 |
| `eventually` | 378 | 规范设施，正面信号 |
| `within(` | 557 | 规范设施，正面信号 |
| `until(` | 358 | 规范设施，正面信号 |
| `scaled(` | 44 | 规范设施，正面信号 |
| `start_paused` | 38 | 暂停时钟，正面信号 |

规范设施的采用面已经很大：**113 个测试文件**使用了 `within`/`eventually`/`until`，即"事件驱动"已经是主流写法。

### 2.3 #1373 的并行性事实

- 合并后共有 **18 个 `tests/<suite>/main.rs`**，例如 `crates/rutis/tests/core/main.rs` 把 `unload_deadline`、`contract`、`events`、`interleave` 等 34 个模块装进同一个二进制。
- Rust libtest **默认在同一二进制内用多线程并行**；CI 在自托管 runner 上显式设 `RUST_TEST_THREADS=$jobs`（`jobs = CPU/4`，最小 2，`.github/workflows/ci.yml:2016-2023`）。
- `stress.yml` 每 3 轮在 `--test-threads` 的 1 / 2 / 物理核数之间切换（`.github/workflows/stress.yml:136-143`），**核数那一轮正是 E 类全局状态问题最容易触发的配置**。
- Swift `RutisSelfTest` 是自研顺序 runner（`swift/rutis/Tests/SelfTest/main.swift:1`：`// Runs every test of the registry, one after another`），因此 Swift 的全局计数器风险低于 Rust。
- Go 包内串行、包间并行；Kotlin 未设 `maxParallelForks`；Python unittest 串行；Node `node --test` 按文件并行（进程级隔离）。
- 因此：**"测试从独占进程变成共享进程"这件事只发生在 Rust 的 18 个套件上**，E 类里最严重的 5 项全部落在这里。

### 2.4 分类判定方法

对每个命中读其上下文（不只看调用点），按"这个等待在断言什么"归类，而不是按写法归类。例如同为 `sleep(100ms)`：

- 若随后断言"某事没有发生"，且该事**永远不会**正确发生 → A（只可能漏报）；
- 若随后断言"某事没有发生"，但该事**稍后应当正确发生** → B/E（慢机器上会误报）；
- 若 sleep 只是让某个后续动作落在窗口内，断言两边都成立 → D；
- 若 sleep 是在测量稳态或耗时 → C。

---

## 三、分类 A–E

| 类 | 定义 | 本仓库的典型形态 | 处置 |
| --- | --- | --- | --- |
| **A** | **合法的负向断言**：用固定时长证明"某事没有发生"，且该事永远不该发生，因此只可能漏报、不可能误报 | `sleep(500ms)` 后断言行仍是 `Down` | 保留；按规范加 `// clock: weak-negative — <原因>` |
| **B** | **用固定轮询（或固定时长）等一个事件/状态** | `while !cond { sleep(10ms) }`、`sleep(50ms)` 后断言异步结果已到 | 改为等待事件；拿不到事件就用共享 `eventually`/`until`，超时要报出当时状态 |
| **C** | **等性能稳态**：等排空、等基线稳定、测耗时 | `drained()` 连续 5 个静默窗口、`settle()` 等 fd/字节不动、`< 100 ms` 上界 | 保留，但必须在独立二进制或串行执行；耗时上界要放宽或删除 |
| **D** | **用时间人为制造竞态窗口**：断言在窗口内/外都成立，只是让某条路径有机会被走到 | `// clock: widen — ...`、`sleep(50µs)` "放大重叠窗口" | 保留；必须满足"断言与是否命中无关"，并加 `// clock: widen — <原因>` |
| **E** | **真实缺陷**：全局状态、共享计数器、固定端口/临时目录、或被竞态窗口决定结果的断言 | 进程 env 的 `set_var`、`running_processes()` 全量相等、`DialedSocket.live` 计数、50 ms 抢 300 ms | 必须修 |

**关于 A 的一个补充**：A 类"合法"指的是**不会导致偶发失败**，不表示"没有代价"。本报告里 A 类有 3 处代价偏高（`plugin_limits.rs:192` 的 12 s、`config/dev.rs` 的 3×1.2 s、`cross_version.rs` 的 3×3 s），它们的代价是 CI 墙钟时间，不是稳定性。

---

## 四、按语言统计

### 4.1 原始命中与分类

| 语言 | 原始调用点 | A | B | C | D | E | 排除：H 夹具 | 排除：P 产品 | 排除：S 辅助 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Rust | 144 | 46 | 66 | 5 | 8 | 2 | 17 | 0 | 0 |
| Swift | 61 | 9 | 15 | 0 | 7 | 1 | 17 | 8 | 4 |
| Kotlin | 37 | 6 | 12 | 0 | 4 | 0 | 12 | 3 | 0 |
| Go | 16 | 5 | 2 | 0 | 0 | 0 | 7 | 1 | 1 |
| Node/Bun | 45 | 3 | 10 | 0 | 1 | 0 | 26 | 0 | 5 |
| Python | 33 | 5 | 11 | 0 | 1 | 0 | 13 | 1 | 2 |
| **合计** | **336** | **74** | **116** | **5** | **21** | **3** | **92** | **13** | **12** |

校验：74 + 116 + 5 + 21 + 3 + 92 + 13 + 12 = 336。

### 4.2 处置口径

| 语言 | 必须修（E） | 建议改（B + D） | 只需标注（A） | 保留（C + H + S） |
| --- | ---: | ---: | ---: | ---: |
| Rust | 2 (+6 非 sleep) | 74 | 46 | 22 |
| Swift | 1 | 22 | 9 | 21 |
| Kotlin | 0 | 16 | 6 | 12 |
| Go | 0 | 2 | 5 | 8 |
| Node/Bun | 0 | 11 | 3 | 31 |
| Python | 0 | 12 | 5 | 15 |
| **合计** | **3 (+6)** | **137** | **74** | **109** |

本表不计入产品代码（P）的 13 处，因此每行合计为"原始调用点 − P"。

### 4.3 tools / workflows

| 位置 | 数量 | 判定 |
| --- | ---: | --- |
| `.github/workflows/ci.yml` | 6 | 重试退避 / 重试删除，保留 |
| `.github/workflows/release.yml` | 3 | 重试退避 + 有界轮询，保留 |
| `tools/*.sh`、`tools/bench/**` | 5 | 2 处信号升级宽限（保留）、3 处有界轮询（B，可保留） |

---

## 五、逐项清单

记号：**风险** 指"负载下偶发失败"的风险；**工作量** S < 15 分钟、M 15–60 分钟、L > 1 小时。

### 5.1 Rust

#### A 类（合法负向断言，保留 + 标注）

| 位置 | 片段 | 说明 | 风险 | 替代 / 处置 | 量 |
| --- | --- | --- | --- | --- | --- |
| `crates/rutis/tests/core/load_window.rs:269` | `sleep(100ms); assert!(!returned…)` | 消费者被 `release` 通道闸住，断言确定性 | 低 | 加 `// clock: weak-negative — consumer is gated` | S |
| `crates/rutis/tests/core/evicted_apply.rs:481` | 同上 | 同上 | 低 | 同上 | S |
| `crates/rutis/tests/core/evicted_apply.rs:709,739` | `sleep(100ms); assert_eq!(state, Failed)` | 断言"不重试"，只可能漏报 | 低 | 标注 | S |
| `crates/rutis/tests/core/still_waiting.rs:66,113,132,139` | `sleep(300–500ms); assert_eq!(len, n)` | 断言"不重复上报" | 低 | 标注 | S |
| `crates/rutis/tests/core/events.rs:944` | `sleep(20ms); assert!(entries.is_empty())` | 失败过滤器不入表 | 低 | 标注 | S |
| `crates/rutis/tests/core/embedding_output.rs:124` | `sleep(200ms); assert_eq!(said.len(), 1)` | 断言只说一次 | 低 | 标注 | S |
| `crates/rutis/tests/core/transient_release.rs:66` | `select! { sleep(deadline), last.changed() }` | 有界等待 + 事件二选一 | 低 | 已是事件优先，保留 | — |
| `crates/rutis-bridge/tests/lang/event_hub.rs:538,611,623` | `eventually(...); sleep(50ms); assert_eq!` | 先等正事件再等 50 ms 确认没多 | 低 | 标注 | S |
| `crates/rutis-bridge/tests/session/session_plugin.rs:249,424,1047` | `sleep(100–200ms); assert!(none)` | 断言"未共享/未多发" | 低 | 标注 | S |
| `crates/rutis-bridge/tests/native/bun_runtime.rs:387` | `sleep(300ms); assert!(exit_status().is_none())` | 断言进程没死 | 低 | 标注 | S |
| `crates/rutis-loader/tests/rows/event_close_admission.rs:195,245,417,446` | `sleep(100ms); assert_eq!(calls, 0)` | 关停后监听器不跑；顺序由协议保证 | 低 | 标注 | S |
| `crates/rutis-loader/tests/go/go_rows.rs:395,517,526,534,1253` | `sleep(300–500ms); assert_eq!(runs, n)` | 断言"不再重启" | 低 | 标注 | S |
| `crates/rutis-loader/tests/lang/swift_rows.rs:315` | `sleep(200ms); assert_eq!(row_state(probe), Active)` | 断言另一行未受影响 | 低 | 标注 | S |
| `crates/rutis-loader/tests/rows/plugin_limits.rs:192` | `sleep(12s); assert_eq!(line, "wordcount: starting …")` | 断言未超时；**代价 12 s** | 低 | 保留但缩短：先断言 `starting`，再等一个"随后必然发生"的事件 | M |
| `crates/rutis-loader/tests/go_stop.rs:139` | `sleep(DEADLINE*2); assert_eq!(generation, …)` | 断言未重启 | 低 | 标注 | S |
| `crates/rutis-loader/tests/runtimes/multilang.rs:720` | `sleep(500ms); assert!(no "in tromso")` | 断言隔离 | 低 | 标注 | S |
| `crates/rutis-host/tests/process/runtime_exit.rs:293,373` | `sleep(1500ms); assert!(Down / empty)` | 断言不重启 | 低 | 标注 | S |
| `crates/rutis-host/tests/config/unhandled.rs:220,262` | `sleep(600ms); assert_eq!(stderr, …)` | 断言没有多余输出 | 低 | 标注 | S |
| `crates/rutis-host/tests/config/dev.rs:442,812,854` | `sleep(1200ms)` ×3 | 断言没有多余行 | 低 | 标注；可改为"等到下一行已知输出" | S |
| `crates/rutis-host/tests/config/rows_file.rs:107` | `sleep(1500ms)` | 防抖窗口 | 低 | 标注，或等"第一次 reload"事件 | S |
| `crates/rutis-agent/tests/integration.rs:153,164,423` | `sleep(30–50ms); assert_eq!(Pending / len)` | 断言未启动/无残留 | 低 | 标注 | S |
| `tests/e2e/tests/install/cross_version.rs:665,724,933` | `sleep(3s)` ×3 | 断言"没有第二次" | 低 | 标注；e2e 独占进程，风险确实低 | S |
| `tests/e2e/tests/install/ios_app.rs:380` | `sleep(2s)` | 系统 UI 前置条件（本地跑，不进 CI） | 低 | 保留 | — |
| `node/.../websocket.test.mjs:168` | `sleep(500ms); assert.equal(closed,false)` | "十个超时之内不关" | 低 | 标注 | S |
| `node/.../cordis-events.test.mjs:330` | `sleep(10ms); assert.deepEqual(reported, [])` | 断言不重复上报 | 低 | 标注 | S |
| `node/rutis/test/testing-on.test.mjs:59` | `sleep(50ms); assert.deepEqual(asked, [])` | 卸载后不投递 | 低 | 标注 | S |
| `python/.../test_events_session.py:233,762` | `sleep(0.05)` | 断言仍挂起 | 低 | 标注 | S |
| `python/.../test_peer.py:85,97` | `sleep(0.01)` | 断言被丢弃的调用没跑 | 低 | 标注 | S |
| `python/.../test_testing.py:387` | `sleep(0.05)` | 断言已取消 | 低 | 标注 | S |
| `kotlin/.../AppTest.kt:175` | `Thread.sleep(100)` | "events 未提供前不安装" | 低 | 标注 | S |
| `kotlin/.../ConnectionTest.kt:199,234,245,482` | `Thread.sleep(200–1500)` | 断言未阻塞/未关闭 | 低 | 标注 | S |
| `kotlin/.../EventsConformanceTest.kt:255` | `Thread.sleep(LACKS)` | 一致性的 settle 窗口 | 低 | 保留（有 `RUTIS_EVENTS_SETTLE_SCALE`） | — |
| `go/.../events_conformance_test.go:601,669` | `time.Sleep(eventLacks)` / `time.After(eventSettle)` | 同上 | 低 | 保留 | — |
| `go/.../dial_test.go:379`、`events_host_test.go:56`、`plugin_values_test.go:214` | `time.After(3–10s)` | 防挂死上限 | 低 | 保留 | — |

#### B 类（有界轮询等事件，建议改用共享 helper）

| 位置 | 片段 | 风险 | 替代 | 量 |
| --- | --- | --- | --- | --- |
| `crates/rutis/tests/core/load_window.rs:263,409,558,637,747` | `while cond { sleep(1ms) }` inside `timeout(LIMIT)` | 低 | `rutis_test_support::until` | S |
| `crates/rutis/tests/core/evicted_apply.rs:278,368,475` | 同上 | 低 | `until` | S |
| `crates/rutis/tests/core/still_waiting.rs:61,79,98,106,128` | `within(loop { sleep(10ms) })` | 低 | `eventually` | S |
| `crates/rutis/tests/core/embedding_output.rs:120` | 同上 | 低 | `eventually` | S |
| `crates/rutis/tests/core/service_emits.rs:66` | `while len<2 && now<deadline { sleep(10ms) }`，deadline 硬编码 10 s | 低 | `Guard(HANG_GUARD*2).until`（受 `RUTIS_TEST_TIME_SCALE` 缩放） | S |
| `crates/rutis/tests/core/terminal_cleanup.rs:112` | `sleep(1ms)` 等 backlog 排空 | 低 | `until` | S |
| `crates/rutis/tests/core/instance_subtrees.rs:589` | `timeout(10s, loop sleep(1ms))` | 低 | `until` | S |
| `crates/rutis/tests/core/event_backlog.rs:57` | `sleep(20ms)` 只为让最老的变老 | 低 | 已是单调下界，保留 | — |
| `crates/rutis/tests/core/dispatch_chain_probe.rs:94` | `timeout(60s, loop sleep(5ms))`，60 s 未缩放 | 低 | `Guard(HANG_GUARD*2).until` | S |
| `crates/rutis/tests/core/transient_release.rs:100` | `loop { … sleep(5ms) }` | 低 | `until` | S |
| `crates/rutis-bridge/tests/lang/event_hub.rs:223,298,676` | 本地 `eventually()` 定义 + select 轮询 | 低 | 删本地定义，用 `rutis_test_support::eventually` | S |
| `crates/rutis-bridge/tests/session/session_plugin.rs:1002` | 本地 `eventually_async()` | 低 | 保留（异步检查），或扩展共享 helper | S |
| `crates/rutis-bridge/tests/session/mount_host_service.rs:127,151,161,181` | `within(loop sleep(10ms))` | 低 | `until` | S |
| `crates/rutis-bridge/tests/session/session_future_release.rs:122` | `while weak.upgrade() { assert deadline; sleep(10ms) }` | 低 | `until` | S |
| `crates/rutis-bridge/tests/session/session_tags.rs:58,99` | 等 `port` 文件 / 等进程退出 | 低 | 保留（跨进程，无事件） | — |
| `crates/rutis-bridge/tests/native/local_spawn.rs:134` | `loop { send; sleep(10ms) }` | 低 | 保留（等管道报告退出，无事件） | — |
| `crates/rutis-bridge/tests/lang/local_soak.rs:80` | `while children>0 && now<deadline { sleep(50ms) }` | 低 | 保留（`#[ignore]` soak） | — |
| `crates/rutis-loader/tests/rows/optional_deps.rs:503`、`optional_deps_go.rs:260` | `while … { sleep(20ms) }` | 低 | `until` | S |
| `crates/rutis-loader/tests/lang/objects_cross.rs:667,804` | `loop { … sleep(50ms) }` | 低 | `until` | S |
| `crates/rutis-loader/tests/events_conformance.rs:888,1030,1376` | 本地 `until()` 与轮询 | 低 | 用共享 helper 或保留（该套件自带串行锁） | M |
| `crates/rutis-loader/tests/events_conformance/rust_participant.rs:735` | `loop { … sleep(10ms) }` | 低 | `until` | S |
| `crates/rutis-loader/tests/go_stop.rs:106` | `while watching { sleep(2ms) }` | 低 | 保留（采样全局进程数） | — |
| `crates/rutis-host/tests/config/unhandled.rs:167` | `while !files { sleep(50ms) }` | 低 | 保留（等文件，跨进程） | — |
| `crates/rutis-host/tests/config/call.rs:59,179` | `while … { sleep(200ms) }` | 低 | 保留（等 CLI 输出） | — |
| `crates/rutis-host/tests/config/status.rs:119` | 同上 | 低 | 保留 | — |
| `crates/rutis-host/tests/process/nodes.rs:122` | `while … { sleep(20ms) }` | 低 | 保留 | — |
| `crates/rutis-host/tests/process/reload.rs:506` | `while !cleaned.exists() { sleep(10ms) }` | 低 | 保留（等文件） | — |
| `crates/rutis-agent/tests/integration.rs:260,396`、`unit_loop.rs:199`、`minimal_tools.rs:492`、`order_probe.rs:53` | `sleep(2–5ms)` 轮询 | 低 | `eventually` | S |
| `tests/e2e/src/residue.rs:205`、`host.rs:419` | 进程表轮询 | 低 | 保留（无事件） | — |
| `tests/e2e/tests/install/embedding.rs:295,324`、`crashes/stray_runtime.rs:92,180` | 轮询 | 低 | 保留 | — |
| `tests/e2e/tests/soak/soak.rs:1442`、`soak/netem.rs:392`、`develop/dev_loop.rs:625`、`develop/devx.rs:1003`、`develop/host_writes_plugin.rs:187`、`install/ios_app.rs:257` | 轮询 | 低 | 保留 | — |
| `swift/.../AppTests.swift:216,420` | `for _ in 0..<500 where !cond { sleep(10ms) }` | 低 | 抽成 `waitFor`（已在用）或 `RutisTest.settle` | S |
| `swift/.../PeerTests.swift:217,253,284,319,350`、`ObjectTests.swift:221,229`、`UnloadOrderTests.swift:163,170`、`EventsConformanceTests.swift:236,248` | 有界轮询 | 低 | 抽公共 `eventually` | M |
| `kotlin/.../AppTest.kt:71` | `until()`，10 s 死线 | 低 | 保留 | — |
| `kotlin/.../ConnectionTest.kt:387,404`、`ObjectsTest.kt:294`、`PeerTest.kt:147,160`、`UnknownFieldsTest.kt:184`、`RutisTestTest.kt:59` | 有界轮询 | 低 | 保留 | — |
| `kotlin/.../EventsConformanceTest.kt:252,264` | 轮询 | 低 | 保留 | — |
| `go/.../events_conformance_test.go:598,614` | 轮询 | 低 | 保留（用 `testwait`） | — |
| `python/.../test_peer.py:122,126,149,152,159,183,199,363` | `await asyncio.sleep(0)` 让出事件循环 | 低 | 保留（单线程事件循环，确定性） | — |
| `python/.../test_testing.py:298`、`test_chunked.py:114` | 轮询 / 让出一拍 | 低 | 保留 | — |

#### C 类（性能稳态，保留但需隔离）

| 位置 | 片段 | 风险 | 处置 | 量 |
| --- | --- | --- | --- | --- |
| `crates/rutis-loader/tests/close_cpu.rs:183` | `drained()`：连续 5 个 10 ms 静默窗口 | 低 | 已是独立二进制；保持（该文件头注释已声明"no other test may run beside the measured one"） | — |
| `crates/rutis-loader/tests/close_cpu.rs:202` | `sleep(IDLE=1s)` 测 CPU | 低 | 同上 | — |
| `crates/rutis-loader/tests/close_memory.rs:247` | `settle()`：等任务数/字节静止，按 `time_scale()` 放大 | 低 | 已是独立二进制；保持 | — |
| `crates/rutis-bridge/tests/lang/local_soak.rs:73` | `sleep(500ms)` 取 fd 基线 | 中 | 见 E-07（全局量测） | S |
| `tests/e2e/tests/soak/soak.rs:1589`、`soak/netem.rs:616` | soak 空闲采样 / 35 s 相位 | 低 | e2e 独占进程，保留 | — |

#### D 类（人为制造窗口，断言应两边成立）

| 位置 | 片段 | 风险 | 处置 | 量 |
| --- | --- | --- | --- | --- |
| `crates/rutis/tests/core/load_window.rs:643,753` | `sleep(50ms)` 让 withdrawal 排入 refresh | 低 | 等一个"已排队"的可观察记录；否则标 `// clock: widen` | S |
| `crates/rutis/tests/core/events.rs:1716` | `sleep(50ms); // Give a delivery out of order its chance` | 低 | 补 `// clock: widen` | S |
| `crates/rutis/tests/core/terminal_cleanup.rs:210` | 采样协程 `sleep(200µs)` | 低 | 保留（诊断用） | — |
| `crates/rutis/tests/core/dispatch_chain_probe.rs:49` | `thread::sleep(50µs); // 放大重叠窗口` | 低 | 补 `// clock: widen` | S |
| `crates/rutis-bridge/tests/session/session_plugin.rs:1022` | `sleep(50ms)` 后再 dispose，让调用已在途 | 低 | 等"调用已开始"信号 | S |
| `crates/rutis-loader/tests/rows/event_close_admission.rs:117` | `sleep(100ms); // Give the dispatch a bounded window` | 低 | 补 `// clock: widen` | S |
| `crates/rutis-bridge/tests/session/cancellation.rs:129` | `sleep(100ms)`，已标 `// clock: widen` | 低 | 保持现状（范例） | — |
| `swift/.../AppTests.swift:318,350,355` | 50 ms / 0.3 ms 抖动，让 stop 落在拨号各阶段 | 低 | 保留（断言与命中无关），补注释 | S |
| `swift/.../ConcurrencyTests.swift:21` | `Task.sleep(20ms)` 让 64 个任务都进入 waiters | 低 | 改为等计数达到 N | S |
| `swift/.../ObjectTests.swift:218` | `sleep(100ms)` 后 cancel | 低 | 等"对象已开始运行"信号 | S |
| `swift/.../PeerTests.swift:106,133` | 50/200 ms 让 attach 完成 | 低 | 等 attach 事件 | S |
| `kotlin/.../ConnectionTest.kt:399` | `Thread.sleep(100)` 后再 `stop()` | 低 | 等 `listening` 已开始 | S |
| `kotlin/.../ObjectsTest.kt:257`、`PeerTest.kt:158`、`UnloadOrderTest.kt:173` | `sleep(100)` 后再 cancel | 低 | 等"调用已开始"信号 | S |
| `node/rutis/test/events.test.mjs:69` | `sleep(20)` 后再 unload | 低 | 等 listener 已开始 | S |
| `python/.../test_objects.py:147` | `sleep(0.05)` 后 cancel | 低 | 等 `run` 已进入 | S |

#### E 类（真实缺陷，sleep 相关）

| 位置 | 片段 | 问题 | 风险 | 替代 | 量 |
| --- | --- | --- | --- | --- | --- |
| `crates/rutis/tests/core/still_waiting.rs:124` | `sleep(50ms)` 后才 `provide(Greeting)`，而上报阈值是 300 ms | 50 ms 被拖到 >300 ms 就提前上报，`assert!(reported.is_empty())` 直接失败。裸 sleep 不受 `RUTIS_TEST_TIME_SCALE` 放大，负载下必然发生 | **高** | 不要抢时间窗：把 `report_waiting_after` 调到"足够长"（如 10 s）再断言不上报；或用暂停时钟 | S |
| `crates/rutis/tests/core/unload_deadline.rs:49` | `spawn(emit Hang); sleep(50ms); dispose(); assert(elapsed >= 250ms)` | 若 50 ms 内 listener 尚未被派发，dispose 没有可等的清理，`elapsed` < 250 ms 直接失败 | **高** | 用 listener 已进入的信号（`Notify`/`oneshot`）代替 sleep | S |
| `swift/rutis/Tests/SelfTest/AppTests.swift:362` | `for _ in 0..<300 where DialedSocket.live.sessions != sessions { sleep(10ms) }` 后断言相等 | `DialedSocket.live` 是进程级计数器；任何前序测试残留都会让断言失败 | 中 | 计数器改为测试内注入的实例计数，或断言"不增长"而非"等于基线" | M |

#### E 类（真实缺陷，非 sleep：全局状态 / 共享计数器 / 固定端口）

| 位置 | 片段 | 问题 | 风险 | 替代 | 量 |
| --- | --- | --- | --- | --- | --- |
| `crates/rutis/tests/core/unload_deadline.rs:37` | `std::env::set_var("RUTIS_SHUTDOWN_TIMEOUT", "0.3")`，**从不恢复** | `shutdown_deadline()`（`crates/rutis/src/fiber.rs:1344`）每次调用都读 env，因此该值会泄漏给 `core` 二进制里所有并发/后续测试的卸载路径 | **高** | 加进程级 env 互斥（照 `rutis-host/tests/common/mod.rs:33` 的 `environment()`），或改为把时限作为参数传入被测对象 | S |
| `crates/rutis-loader/tests/rows/virtual_clock_contract.rs:184-200`（185/187/189/192/199 行各一次 env 写） | `#[test]` 内 5 条 `set_var`/`remove_var`，注释写"the variable belongs to the process" | 同一 `rows` 套件内其它测试同时卸载插件时，会读到 0.25 s / 3 s / 非法值 | **高** | 同上：加互斥；或把 `shutdown_deadline()` 改成可注入 | M |
| `crates/rutis-host/tests/dial/dial_conformance.rs:245` | `std::env::set_var("RUTIS_TOKEN_PHONE", TOKEN)`，**未取 `environment()`** | 同套件的 `lease_e2e.rs:433`、`swift_app.rs:206`、`kotlin.rs:91` 会在 `environment()` 里删除所有 `RUTIS_TOKEN_*`（`common/mod.rs:39-41`）。交错即让 `dial_conformance` 的 host 找不到 token | **高** | 在 `listen()` 前取 `crate::common::environment()` 并持有到测试结束 | S |
| `crates/rutis-host/tests/config/android.rs:156` | 同样的未加锁 `set_var` | **假阳性**：`Rutis::start` / `Rutis::with` 的三个调用点（280、344、503 行）都已在调用前持有 `environment()`；在 `with` 里再加锁会自锁 | — | 不改；如需更强约束，在 `with` 的文件头写清“调用方必须持有 environment()” | — |
| `crates/rutis-loader/tests/go_stop.rs:96,133` | `assert_eq!(running_processes(), vec![pid])` | 进程级全局注册表全量相等；今天安全仅因为该文件只有 1 个测试、且是独立二进制 | 低（潜在） | 改成"只检查包含关系"，或在注释里写明"必须独立二进制" | S |
| `crates/rutis-bridge/tests/lang/local_soak.rs:14-34`、`crates/rutis-bridge/tests/lang/websocket_soak.rs:15-20` | 读 `/proc/self/fd`、`/proc/self/task/*/children`、线程数 | 进程级量测，却被放进 `lang` 套件。今天安全仅因为 `stress.yml:506-507` 用 `local_soak::` / `websocket_soak::` 名字过滤，只跑一个测试 | 中（潜在） | 移回独立二进制，或在文件头写明"不得与其它模块同跑" | S |

**已正确处理、可作为范例的 E 类**：

- 固定端口 9230 与 2345：`crates/rutis-host/tests/config/debug.rs:35-45` 用 `flock(/tmp/rutis-host-debug-ports.lock)` 跨进程串行化，7 处调用。
- 进程列表断言不再看全局：`crates/rutis-host/tests/process/reload.rs:468-472` 只断言 `!running.contains(&restarted)`，并在注释里承认"其它测试可能同时跑自己的 runtime"。
- 诊断输出里带全局进程列表是安全的：`crates/rutis-loader/tests/go/go_rows.rs:156`、`crates/rutis-bridge/tests/platform/host_gone.rs:49` 只用于失败信息。
- 需要独占的 CPU/内存测量已经拆成独立二进制：`crates/rutis-loader/tests/close_cpu.rs:6-7`、`crates/rutis-loader/tests/close_memory.rs`。

### 5.2 Swift

Swift 的 `RutisSelfTest` 是顺序 runner，因此全局状态风险低于 Rust；主要问题集中在耗时上界与固定等待正事件。

| 位置 | 片段 | 类 | 风险 | 替代 / 处置 | 量 |
| --- | --- | --- | --- | --- | --- |
| `swift/rutis/Tests/SelfTest/AppTests.swift:275` | `sleep(500ms); XCTAssertEqual(app.state, .connecting)` | A | 低 | 保留，标注 | S |
| `AppTests.swift:326` | `sleep(50ms); XCTAssertEqual(discarded.all, ["socket"])` | A | 低 | 保留 | — |
| `AppTests.swift:438` | `sleep(100ms); XCTAssertEqual(lines, [])` | A | 低 | 保留 | — |
| `AppTests.swift:216,420` | `for _ in 0..<500 where !cond { sleep(10ms) }` | B | 低 | 已封装为 `waitFor`，可提到共用处 | S |
| `AppTests.swift:351,354` | 有界轮询等 accept | B | 低 | 保留 | — |
| `AppTests.swift:318,350,355` | 抖动 sleep 让 stop 落在拨号各阶段 | D | 低 | 保留（断言两边成立），补注释 | — |
| `AppTests.swift:362` | 轮询 `DialedSocket.live` 全局计数后断言相等 | **E** | 中 | 改为测试内注入的计数，或断言"不增长" | M |
| `AppTests.swift:75` | `uncancellableSleep` 辅助 | H | — | 保留 | — |
| `ChunkedTests.swift:167,229` | `sleep(100ms); XCTAssertEqual(tally, 0)` | A | 低 | 保留 | — |
| `ConcurrencyTests.swift:21` | `sleep(20ms)` 让 64 个任务进入 waiters | D | 低 | 等 `waiters.count == N` | S |
| `EventsConformanceTests.swift:236,248` | 有界轮询 | B | 低 | 保留 | — |
| `EventsConformanceTests.swift:239,275` | settle / 取消定时器 | A | 低 | 保留 | — |
| `LifetimeTests.swift:20,27` | 夹具里 100 ms 慢 handler | H | — | 保留 | — |
| `LifetimeTests.swift:35` | 结尾 `sleep(1s)`，无断言 | A | 低 | 保留（让排队的调用跑完） | — |
| `ObjectTests.swift:218` | `sleep(100ms)` 后 cancel | D | 低 | 等"对象已开始运行"信号 | S |
| `ObjectTests.swift:221,229` | 有界轮询导出表 | B | 低 | 保留 | — |
| `ObjectTests.swift:117` | 夹具 60 s | H | — | 保留 | — |
| `PeerTests.swift:106,133` | 50/200 ms 让 attach 完成 | D | 低 | 等 attach 事件 | S |
| `PeerTests.swift:217,253,284,319,350` | 有界轮询 | B | 低 | 抽公共 `eventually` | M |
| `PeerTests.swift:67,94`、`UnknownFieldTests.swift:161` | 夹具 30 s | H | — | 保留 | — |
| `UnloadOrderTests.swift:163,170` | 有界轮询 50 ms × 100 | B | 低 | 抽公共 helper | S |
| `Sources/RutisTest/RutisTest.swift:151,206,227`、`RutisTestDevice.swift:98` | 1–2 ms 轮询 | S | — | 规范实现，保留 | — |
| `Sources/RutisFixturePlugins/*`（6 文件 8 处） | 夹具延迟（`Lease.swift:39`、`Weather.swift:45`、`PluginValues.swift:68`、`Objects.swift:86` 等） | H | — | 保留；其中 `Objects.swift:86`（20 ms 后 `exit(1)`）与 `PluginValues.swift:68`（200 ms 后 `exit(17)`）让结果取决于夹具时长，建议改成收到信号再退出 | M |

### 5.3 Kotlin

| 位置 | 片段 | 类 | 风险 | 替代 | 量 |
| --- | --- | --- | --- | --- | --- |
| `kotlin/rutis/rutis/src/jvmTest/kotlin/arcships/rutis/ConnectionTest.kt:370,376` | `Thread.sleep(200)` 等服务器接纳 2 条空闲连接，再断言第 3 条被拒 | **B（最危险的一类：固定时长等正事件）** | **中高** | 等 `server.accepted.size == 2`（服务器已有可观察计数） | S |
| `ConnectionTest.kt:286` | `assertTrue((nanoTime-start)/1e6 < 100, "closeWith blocked")` | **C（耗时上界）** | **中高** | 删除上界，改为"不阻塞"的因果断言，或把上界放到秒级 | S |
| `ConnectionTest.kt:300` | `assertTrue(... < 1000, "the other connection was not pinged")` | C | 中 | 等 ping 帧到达，而不是等时长 | S |
| `ConnectionTest.kt:199,234,245,482` | 200–1500 ms 负向断言 | A | 低 | 保留 | — |
| `ConnectionTest.kt:387,404` | 有界轮询 | B | 低 | 保留 | — |
| `ConnectionTest.kt:399` | `sleep(100)` 后 `stop()` | D | 低 | 等 listening 已开始 | S |
| `ConnectionTest.kt:208,312,353,439` | 夹具线程 | H | — | 保留 | — |
| `ObjectsTest.kt:257` | `sleep(100)` 后 cancel | D | 低 | 等调用已开始 | S |
| `ObjectsTest.kt:294`、`PeerTest.kt:147,160`、`UnknownFieldsTest.kt:184` | 有界轮询 | B | 低 | 保留 | — |
| `PeerTest.kt:89` | 夹具 `delay(20)` | H | — | 保留 | — |
| `PeerTest.kt:158`、`UnloadOrderTest.kt:173` | `delay(100)` 后 cancel | D | 低 | 等"已开始"信号 | S |
| `AppTest.kt:175` | `Thread.sleep(100)` 负向断言 | A | 低 | 保留 | — |
| `AppTest.kt:71` | `until()` 10 s 死线 | B | 低 | 保留 | — |
| `EventsConformanceTest.kt:252,264` | 轮询 | B | 低 | 保留 | — |
| `EventsConformanceTest.kt:255` | settle | A | 低 | 保留（受 `RUTIS_EVENTS_SETTLE_SCALE`） | — |
| `RutisTestTest.kt:59` | 有界轮询 | B | 低 | 保留 | — |
| `rutis-conformance/.../plugins/*`（5 文件 6 处） | 夹具延迟 | H | — | 保留 | — |

### 5.4 Go

| 位置 | 片段 | 类 | 风险 | 替代 | 量 |
| --- | --- | --- | --- | --- | --- |
| `go/rutis/rutistest/events_conformance_test.go:598,614` | `time.Sleep(10ms)` / `5ms` 轮询 | B | 低 | 改用 `testwait.Eventually` | S |
| `events_conformance_test.go:601` | `time.Sleep(eventLacks)` | A | 低 | 保留 | — |
| `events_conformance_test.go:669`、`plugin_values_test.go:214`、`internal/runtime/dial_test.go:379`、`events_host_test.go:56` | `time.After(...)` 防挂死 | A | 低 | 保留（相当于 `testwait.HangGuard`） | — |
| `internal/runtime/events_test.go:216`、`listen_test.go:427` | 夹具慢操作 | H | — | 保留 | — |
| `internal/fixtures/**`（5 处） | 夹具场景 | H | — | 保留 | — |
| `internal/testwait/testwait.go:62` | 规范 `Eventually` 的 10 ms 轮询 | S | — | 规范实现，保留 | — |

Go 没有发现 E 类问题：`testleak`（`go/rutis/internal/testleak`）用 goleak 在 `TestMain` 检查泄漏，`SharedHost()` 显式忽略共享 host 的 goroutine，是有意的全局状态处理。

### 5.5 Node / Bun

| 位置 | 片段 | 类 | 风险 | 替代 | 量 |
| --- | --- | --- | --- | --- | --- |
| `node/rutis-runtime/test/cordis-events.test.mjs:322` | `sleep(10ms); assert.deepEqual(reported, [完整列表])` | **B（固定时长等正事件）** | 中 | 用 `eventually` 等 `reported.length === 1` | S |
| `cordis-events.test.mjs:330` | `sleep(10ms); assert.deepEqual(reported, [])` | A | 低 | 保留 | — |
| `node/rutis-runtime/test/websocket.test.mjs:168` | `sleep(500ms); assert.equal(closed,false)` | A | 低 | 保留 | — |
| `node/rutis-runtime/test/leaf.test.mjs:217,249` | 夹具内 `setTimeout` | H | — | 保留 | — |
| `node/rutis-runtime/test/fixtures/**`（10 处） | 夹具 | H | — | 保留 | — |
| `node/rutis-runtime/test/serve.test.mjs:124,138` | `took < 4500` / `took < 1000` | **C** | 中 | 上界放大到 ≥ 10× 预期，或删除上界 | S |
| `node/rutis/test/events.test.mjs:35` | 有界轮询 100×10 ms | B | 低 | 用 `eventually` | S |
| `node/rutis/test/events.test.mjs:69` | `sleep(20)` 后 unload | D | 低 | 等 listener 已开始 | S |
| `node/rutis/test/testing-on.test.mjs:10,59` | 轮询 / 负向断言 | B / A | 低 | 保留 | — |
| `node/rutis/test/testing-*.test.mjs`（4 处 deadline 轮询） | 有界轮询 | B | 低 | 保留 | — |
| `node/rutis/test/fixtures/**`（3 处） | 夹具 | H | — | 保留 | — |
| `node/rutis-runtime/test/support.mjs:18,40`、`bun/rutis-bun/test/support.ts:24,26` | 规范 helper | S | — | 规范实现，保留 | — |
| `bun/rutis-bun/test/remote.test.ts:61,73` | `toBeLessThan(4_500)` / `toBeLessThan(1_000)` | C | 中 | 同 serve.test.mjs | S |
| `bun/rutis-bun/test/sdk-definitions.test.ts:22` | 有界轮询 | B | 低 | 用 `eventually` | S |
| `bun/rutis-bun/test/bad-values.test.ts:342`、`node/rutis-runtime/test/session.test.mjs:836` | GC 后等 5×10 ms | C（稳态） | 低 | 保留 | — |
| `bun/rutis-bun/test/fixtures/**`、`node/rutis-runtime/test/fixtures/**` | 夹具 | H | — | 保留 | — |

### 5.6 Python

| 位置 | 片段 | 类 | 风险 | 替代 | 量 |
| --- | --- | --- | --- | --- | --- |
| `python/rutis/tests/test_output.py:140` | `sleep(0.05)` 等监听线程把一行写进重定向的 stderr | **B（固定时长等正事件）** | 中 | 等"行已写入"的事件（`threading.Event`），或轮询输出缓冲直到出现 | S |
| `test_peer.py:122,126,149,152,159,183,199,363` | `await asyncio.sleep(0)` 让出一拍 | B | 低 | 保留（单事件循环，确定性） | — |
| `test_peer.py:85,97` | `sleep(0.01)` 负向断言 | A | 低 | 保留 | — |
| `test_events_session.py:233,762` | `sleep(0.05)` 负向断言 | A | 低 | 保留 | — |
| `test_testing.py:298` | 轮询 100×10 ms | B | 低 | 用 `eventually` | S |
| `test_testing.py:387` | `sleep(0.05)` 负向断言 | A | 低 | 保留 | — |
| `test_objects.py:147` | `sleep(0.05)` 后 cancel | D | 低 | 等 `run` 已进入 | S |
| `test_chunked.py:114` | `settle()` = `sleep(0.01)` | B | 低 | 保留（同一事件循环） | — |
| `tests/support.py:48,60` | 规范 `eventually` 轮询 | S | — | 规范实现，保留 | — |
| `tests/fixtures/**`、`conformance_*.py` | 夹具 `after` 延迟 | H | — | 保留 | — |

Python 全局状态：`test_testing.py:510` 修改 `os.environ["RUTIS_HOST"]`/`PATH`、`test_websocket.py:475-479` 设置并删除 `RUTIS_HEARTBEAT`。unittest 串行执行，因此当前无并发风险；但如果将来引入 `pytest-xdist` 或线程池，会变成 E 类。

### 5.7 tools / workflows

| 位置 | 片段 | 类 | 风险 | 处置 | 量 |
| --- | --- | --- | --- | --- | --- |
| `.github/workflows/ci.yml:463,589,1126,1584,2122` | `sleep 20` 在 `for attempt in 1 2 3` 里重试 `dlv` / Gradle fatJar 下载 | 重试退避 | 低 | **保留**（不是等事件，是等网络） | — |
| `.github/workflows/ci.yml:2191` | `sleep 2` 在重试 `rm -rf "$GOCACHE"` | 重试退避 | 低 | **保留** | — |
| `.github/workflows/release.yml:497` | `sleep $((attempt * 3))` 指数退避上传重试 | 重试退避 | 低 | **保留** | — |
| `.github/workflows/release.yml:381` | `for _ in $(seq 90); do published && break; sleep 30` | B（有界轮询） | 低 | 保留；已是"最多 45 分钟"的显式上限 | — |
| `.github/workflows/release.yml:568` | `while :; sleep 15`，15 分钟上限，注释写"a hang guard, not a synchronization" | A | 低 | **保留**（写法完全正确，可作为范例） | — |
| `tools/ci-self-hosted.sh:123` | SIGTERM 后 `sleep 2` 再 SIGKILL | 信号升级宽限 | 低 | 保留；可改为轮询 `kill -0` | S |
| `tools/android-emulator.sh:127` | 每 2 s 查一次 PID，5 分钟上限 | B（有界轮询） | 低 | 保留 | — |
| `tools/test-dylib-launcher.sh:163,183` | 每 0.2 s 查 `tasklist`，50 次上限 | B（有界轮询） | 低 | 保留 | — |
| `tools/bench/events/run.sh:65` | 每 0.5 s 查报告文件，600 次上限 | B（有界轮询） | 低 | 保留 | — |

工作流层面没有发现"用 sleep 冒充同步"的写法；所有 sleep 都是网络重试或显式的有界轮询。

### 5.8 非 sleep 的 E 类汇总

| 编号 | 位置 | 类型 |
| --- | --- | --- |
| E-01 | `crates/rutis/tests/core/unload_deadline.rs:37` | 进程 env 泄漏 |
| E-02 | `crates/rutis-loader/tests/rows/virtual_clock_contract.rs:184-200`（185/187/189/192/199 行各一次 env 写） | 进程 env 并发写 |
| E-03 | `crates/rutis-host/tests/dial/dial_conformance.rs:245` | 进程 env 无锁写，与有锁删除冲突 |
| E-04 | `crates/rutis-host/tests/config/android.rs:156` | **假阳性**：调用方（280/344/503 行）已持锁 |
| E-05 | `crates/rutis-loader/tests/go_stop.rs:96,133` | 全局进程表全量相等（潜在） |
| E-06 | `crates/rutis-bridge/tests/lang/local_soak.rs:14-34`、`websocket_soak.rs` | 进程级 fd/线程量测被放进共享套件（潜在） |

加上 sleep 相关的 3 项（`still_waiting.rs:124`、`unload_deadline.rs:49`、`AppTests.swift:362`），**E 类共 9 项，其中 5 项高风险**。

---

## 六、优先级与建议的修复顺序

### P0（本周，必须先做；不修则 CI 上必然偶发）

1. **E-01 `unload_deadline.rs:37`**：把 `RUTIS_SHUTDOWN_TIMEOUT` 放进 `core` 套件的进程级互斥，并在测试结束时恢复。参照 `rutis-host/tests/common/mod.rs:33-44` 的 `environment()`（进入时清理、持锁到测试结束）。
2. **E-02 `virtual_clock_contract.rs:184-200`**：同上，`rows` 套件需要一份等价的 `environment()`。更彻底的做法是把 `shutdown_deadline()` 改成"可注入"，但那是产品改动，建议先加锁。
3. **E-03 `dial_conformance.rs:245`**：`listen()` 前取 `crate::common::environment()` 并持有到测试结束。
4. **两个"抢时间窗"的 sleep**：
   - `still_waiting.rs:124`：把 `report_waiting_after` 调大（例如 10 s）只验证"短等待不上报"，或改用暂停时钟；
   - `unload_deadline.rs:49`：用 listener 已进入的信号替代 50 ms。
5. **E-04 `android.rs:156`：不改（假阳性）。** `Rutis::start` / `Rutis::with` 的三个调用点（280、344、503 行）都已在调用前持有 `common::environment()`；在 `with` 里再加一次锁会死锁。报告这里只看了 `set_var` 本身，没有看调用点的锁。

### P1（下一批；消除负载下的中高风险）

6. **耗时上界断言（C 类）**：`ConnectionTest.kt:286`（<100 ms）最紧，`ConnectionTest.kt:300`、`serve.test.mjs:124,138`、`remote.test.ts:61,73` 次之。按仓库规范（`docs/design-test-clocks-2026-10-11.md` §3.3）"应当按时发生"的一方要求期望时长与防挂死上限差 10 倍；`<100 ms` 不满足，应删除上界改为因果断言。
7. **固定时长等正事件（B 类最危险的 3 处）**：`ConnectionTest.kt:370,376`（服务器已有 `accepted` 计数）、`cordis-events.test.mjs:322`、`test_output.py:140`。
8. **E-05 / E-06（潜在）**：给 `go_stop.rs` 与 `local_soak.rs` / `websocket_soak.rs` 的文件头写明"必须独立二进制 / 必须按模块名过滤运行"，或直接移回独立二进制。这一步成本极低，但能在下一次有人重排套件时避免重演 #1373。

### P2（机械替换；一次性清理）

9. **B 类 116 处收敛到共享 helper**：删掉 27 个本地 `eventually`/`until`/`wait_for`/`soon` 定义，统一用 `rutis_test_support::eventually(_with)` / `until` / `Guard`。特别值得做的：
   - `service_emits.rs:66`（10 s 硬编码，不受 `RUTIS_TEST_TIME_SCALE` 放大）→ `Guard(HANG_GUARD * 2).until`；
   - `dispatch_chain_probe.rs:94`（60 s 未缩放）→ 同上；
   - `events_conformance.rs:888,1030,1376`（本地 `until` 返回 `Result`）→ 统一。
   
   这一步会让失败信息自动带出当时状态，正好对上 #238 / #249 / #1003 的抱怨。
10. **给 A / D 类补 `clock:` 注释**：仓库目前只有 5 处 `// clock: widen` / `// clock: weak-negative`（`crates/rutis-bridge/src/channel/testing.rs:89,113,142,156`、`crates/rutis-bridge/tests/session/cancellation.rs:127`）。A 类 74 处、D 类 21 处应补齐；否则下一个人无法区分"故意的"与"漏改的"。
11. **成本优化**：`plugin_limits.rs:192` 的 12 s 是 `rows` 套件的关键路径之一，建议改成"先断言 starting，再等一个随后必然发生的事件"。

### P3（可选）

12. Swift / Kotlin / Node 的本地轮询 helper 抽公共实现（各 1–2 小时）。
13. Python 若将来引入并行执行器，需先处理 `test_testing.py:510` 与 `test_websocket.py:475` 的 `os.environ` 修改。

### 不做的事

- 不把 A 类改成"等事件"：负向断言没有事件可等，改成等事件反而会引入新的假象。
- 不动 C 类中已经隔离好的 `close_cpu` / `close_memory`：它们的设计（独立二进制 + 按 `time_scale()` 放大 + 稳定性判定）本来就正确。
- 不动 `#[ignore]` 的 soak 与 `netem`：它们本来就只在 stress 里按名字过滤运行。

---

## 七、不需要改的合法 sleep

以下逐项列出，**明确建议保持原样**（部分只需补注释）。

### 7.1 防挂死上限（只防挂死，不作为同步）

| 位置 | 说明 |
| --- | --- |
| `crates/rutis-test-support/src/lib.rs:28` `HANG_GUARD = 10s` | 规范设施本身 |
| `crates/rutis/tests/core/transient_release.rs:66` `select!{ sleep(deadline), changed() }` | 事件优先，sleep 只是上限 |
| `crates/rutis-host/tests/config/dev.rs` / `unhandled.rs` 的 `COLD`/`deadline` 用法 | 有界等待 |
| `go/rutis/internal/testwait/testwait.go:18` `HangGuard` | 规范设施 |
| `go/.../dial_test.go:379`、`events_host_test.go:56`、`plugin_values_test.go:214`、`events_conformance_test.go:669` | `time.After` 作为上限 |
| `node/rutis-runtime/test/support.mjs:18`、`bun/.../support.ts:26`、`node/baseline/native.mjs:30`、`python/.../support.py:29-60`、`swift/.../RutisTest.swift:147-229` | 各语言规范实现 |
| `.github/workflows/release.yml:568` | 15 分钟上限，注释已写明"a hang guard, not a synchronization" |

### 7.2 夹具场景内容（模拟慢操作，本身就是被测场景）

| 位置 | 说明 |
| --- | --- |
| `crates/rutis/tests/core/events.rs:334,404,969` | 测试插件里的延迟，用于构造事件顺序 |
| `crates/rutis-loader/tests/rows/settled.rs:38,43` | 慢 `apply` 建模 |
| `crates/rutis-loader/tests/events_conformance.rs:1549`、`events_conformance/rust_participant.rs:128,209` | 一致性夹具按 `after` 延迟 |
| `crates/rutis-bridge/tests/session/rpc_callbacks.rs:247,282` | 回调里延迟返回 |
| `crates/rutis-agent/tests/unit_loop.rs:507`、`integration.rs:283`、`order_probe.rs:48` | agent 工具/顺序夹具 |
| `crates/rutis-dsh/tests/common/mod.rs:72` | 无限流夹具 |
| `tests/e2e/fixture-toolhost/src/main.rs:238`、`fixture-tenants/src/main.rs:284` | e2e 夹具 |
| `crates/rutis-bridge/tests/session/session_tags.rs:227` | 子进程角色"永远服务" |
| `swift/.../RutisFixturePlugins/*`（8 处）、`RutisSessionFixture/main.swift:45`、`RutisAppFixture/main.swift:74` | Swift 夹具 |
| `kotlin/.../rutis-conformance/.../plugins/*`（6 处） | Kotlin 夹具 |
| `go/rutis/internal/fixtures/**`（5 处）+ `runtime/events_test.go:216`、`listen_test.go:427` | Go 夹具 |
| `node/rutis-runtime/test/fixtures/**`、`node/rutis/test/fixtures/**`、`bun/.../fixtures/**` | Node/Bun 夹具 |
| `python/rutis/tests/fixtures/**`、`conformance_*.py`、`test_output.py:35-55`、`test_shutdown.py:43`、`test_objects.py:138` | Python 夹具 |

### 7.3 产品代码（不是测试等待）

| 位置 | 说明 |
| --- | --- |
| `swift/rutis/Sources/Rutis/Remote/App.swift:406,447,517,549`、`Listen.swift:252,335`、`WebSocket.swift:260`、`Session/Value.swift:268` | 拨号重试、升级超时、宽限关闭 |
| `kotlin/.../internal/peer/Peer.kt:630`、`remote/Dial.kt:300`、`remote/Frames.kt:126` | 关闭等待、重试间隔 |
| `go/rutis/internal/runtime/listen.go:573` | WebSocket 关闭宽限 |

### 7.4 已经正确处理的全局状态（保持，可作为范例）

| 位置 | 说明 |
| --- | --- |
| `crates/rutis-host/tests/config/debug.rs:35-45` | `flock` 串行化固定端口 9230 / 2345 |
| `crates/rutis-host/tests/process/reload.rs:468-472` | 只断言"不包含"，不看全局全量 |
| `crates/rutis-loader/tests/go/go_rows.rs:156`、`crates/rutis-bridge/tests/platform/host_gone.rs:49` | 全局进程列表只用于失败诊断 |
| `crates/rutis-loader/tests/close_cpu.rs`、`close_memory.rs` | 进程级测量留在独立二进制 |
| `crates/rutis-loader/tests/rows/event_close_admission.rs:195,245,417,446` | `fired.notified()` 先建立因果，再 100 ms 观察 |
| `crates/rutis/tests/core/load_window.rs:269`、`evicted_apply.rs:481` | 消费者被通道闸住，断言确定性 |
| `go/rutis/internal/testleak/testleak.go` | 用 goleak 检查泄漏，并显式忽略共享 host 的 goroutine |

### 7.5 CI / 工具里的重试退避（不是等待同步）

| 位置 | 说明 |
| --- | --- |
| `.github/workflows/ci.yml:463,589,1126,1584,2122` | 下载/构建失败重试 |
| `.github/workflows/ci.yml:2191` | `rm -rf` 失败重试 |
| `.github/workflows/release.yml:497` | 上传失败指数退避 |
| `.github/workflows/release.yml:381` | Maven Central 发布轮询（有 45 分钟上限） |
| `tools/ci-self-hosted.sh:123` | SIGTERM → SIGKILL 的升级宽限 |
| `tools/android-emulator.sh:127`、`tools/test-dylib-launcher.sh:163,183`、`tools/bench/events/run.sh:65` | 有界轮询 |

---

## 八、附：口径与未覆盖项

- 本报告的分类计数来自逐点人工判定，并用脚本核对（分类表与 336 条命中一一对应，无遗漏、无重复）。同一文件同一形态的多行合并为一行列出，行内给出全部行号。
- 未运行测试，因此**没有**观测到真实的偶发失败记录；E 类的高风险判定基于"竞态窗口是否由断言结果决定"的静态推理，以及 CI 的并行度事实（`RUST_TEST_THREADS`、`--test-threads` 轮换）。
- 未覆盖：`docs/**` 中的示例代码；`examples/**`（示例项目，按设计文档 §范围外）；`fuzz/**`（无时间断言）；`kotlin/rutis/android-sample`（需设备）。
- `fuzz/` 下没有 sleep 型等待；`tools/bench/events/bench.mjs:48` 的 `setTimeout(..., 0)` 只是让出事件循环，不是等待。
- 建议在修复后用 `stress.yml` 的 `--test-threads=$cores` 轮次验证 P0 的 5 项：那正是 #1373 之后最容易复现的配置。
