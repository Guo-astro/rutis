# 质量控制第二步（0.10）：跨进程与跨平台的 P0（方案）

[English](design-quality-step2-2026-10-11.en.md)

状态：方案，评审中（Q1、Q2、Q4、Q5、Q6 已决定，2026-10-11）。日期：2026-10-11。基准：`main` `ebf0711`（0.9.1）。
关联：[#183](https://github.com/arcships/rutis/issues/183) 第二步。依据：[质量规范](quality-standard.md)（下称"规范"）；[质量执行现状](quality-status.md)（下称"现状"）§4 风险、§9.2 计划。

本文把第二步剩下的 12 个 issue 放在一起排：每项做什么、不做什么、依赖谁、拆成哪几个 PR、需要维护者先决定什么。每个 PR 的细节（测试名、断言、输出格式）在它自己的 PR 里写；需要单独设计稿的（S4、S5、#178）在本文只定范围和关键做法。

范围外：第一步的收尾 #265（CI 的 `e2e` 任务，本文依赖它）；第三步（0.11）的各项；等待设计的 #185 与"引用、调用数的资源上限"（#178 第 5 项的后半）。

## 一、结论

| # | 结论 | 节 |
| --- | --- | --- |
| C1 | 剩 12 项，拆成约 21 个 PR，分三波：先做被依赖的基础（登记表骨架、E2E 的 Windows 与 macOS 残余、#265），再做协议与内核的单元 / 契约测试，最后做三个黑盒场景 | 九 |
| C2 | **S3、S4 依赖"运行中改配置"，而 `rutis-host run` 不监视 `rutis.json`，被 `kill -9` 的运行时也不会被重新拉起**。已决定：不在 0.10 给 `run` 加配置监视；S3 的恢复改为"重启宿主"，S4 用一个 fixture Rust 宿主，由 stdin 指令触发重新读取配置（Q1）；`run` 是否支持运行中改配置另开 #274 | 三 |
| C3 | **S5 的"吊销 token"没有对应机制**（身份在启动时从环境变量读一次）。已决定：运行中吊销作为对 remote 的需求另开 #273；0.10 的 S5 先测"B 换 token 重启后，A 的重连被拒，类别可读"，#273 完成后补上运行中吊销（Q2） | 3.3 |
| C4 | E2E harness 有一个缺口要先补：`rutis-host` 的运行时进程各自一个进程组，**macOS 上宿主死后留下的运行时查不到**（Linux 有 subreaper，查得到）。与 #232 的 Windows Job Object 放在同一个 PR | 3.4 |
| C5 | #178 拆成 5 个 PR：故障注入与链路；边界规则 3/4/5/7；SDK 对齐；负向 TLS；跨语言值往返属性测试。**边界规则共 8 条**（不是 7 条），1、2、6、8 已有测试 | 四 |
| C6 | 值往返用 `proptest`（新增 dev 依赖），只在 Rust 侧生成值，经现有的 `conformance-session` 回显 fixture 走 Node、Python、Go、Bun；比较规则来自 `guide/plugin-api.md` 的值转换表 | 4.5 |
| C7 | #176 只拆 `cfg(unix)`、补格子，不做生成式矩阵；矩阵报告是一张手写表，放进 `docs/ci.md` | 五 |
| C8 | #206 不在两边之间直接比对，而是让测试工具和真实运行时**对同一份期望文件**比对；Node 的 `conformance-weather` 现在测试工具加载不了，要先补默认导出 | 六 |
| C9 | #207：现有持久化格式都没有格式版本字段。已决定加上（Q4）；同时按版本存样本，并从 0.8.0、0.9.0 的 tag 补样本；`docs/release.md` 加一步 | 七 |
| C10 | #181 先建骨架和编号规则，#177、#178 的测试注释直接用编号；0.10 发布时登记表作为基线，Q9.4.4 的发布门从 0.11 起执行（决定 Q3） | 8.2 |

## 二、剩下的项与覆盖的风险

| issue | 内容 | 风险（现状 §4） | 级 | PR 数 |
| --- | --- | --- | --- | --- |
| #187 | S3 跨语言组合与崩溃 | C3、C4、B10 | P0 | 1 |
| #188 | S4 实现迁移 | G2、G3、G5 | P1 | 1（含 fixture 宿主） |
| #189 | S5 两个宿主进程，loopback | E11、E1、E6 | P1 / P0 | 1 |
| #178 | 协议层缺失场景、故障注入、负向 TLS、SDK 对齐、值往返 | C4、E1、E6、E8、E9、P3、P4、P6、P7、P8、F3 | P0 / P1 | 5 |
| #176 | 一致性套件的 Windows 列与缺失格子 | C10 | P0 | 2 |
| #206 | 测试工具与真实运行时一致 | A1 | P0 | 1 |
| #207 | 持久化格式向前兼容 | J3 | P0 | 1 |
| #177 | 内核保证补齐 | K3–K8 | P0 / P1 | 2 |
| #181 | 保证登记表 | 全部 | — | 1（之后各 PR 顺带更新） |
| #231 | 其他 crate 与其他语言的真实时钟（剩 3/4） | K10 | P0 | 3 |
| #232 | E2E 在 Windows 上的残余检查 | B3、C10 的 E2E 部分 | P0 | 1 |
| #266 | 安装冒烟的后续格子 | B1 | P0 | 2 |

## 三、三个黑盒场景（#187、#188、#189）

### 3.1 先确认的事实（核对过的代码）

| 事实 | 位置 |
| --- | --- |
| `run` 读一次配置、加载、跟随状态，然后只等信号；不监视 `rutis.json` | `crates/rutis-host/src/main.rs` `run` |
| 只有 `dev` 轮询，且看的是项目源文件，不是配置 | `main.rs` `dev` |
| 本地运行时不会被重新拉起；它的 link 不重连；进程死后运行时 `Down`，服务撤回，它的行回到 Pending（状态行 `waiting for its services`），其他运行时的行不受影响 | `rutis-bridge/src/runtime/local.rs`、`runtime/plugin.rs`；`rutis-loader/tests/runtime_rows.rs`、`multilang.rs` |
| 恢复需要 `FiberView::restart`（或 Go 的 `GoRuntimesHandle::restart`），`run` 不调用它们 | `docs/guide/rust-host.md` |
| 需求文档把"自动重启、重连恢复"列为不做 | `requirements-protocol-plugins.md` §8 |
| 身份是启动时由环境变量构造的 `StaticIdentity`，没有任何代码重新检查已建立会话的 token | `rutis-host/src/host.rs` |
| 状态行不打印 link 状态（等待、重试、被拒），也不打印监听实际绑定的地址 | `rutis-host/src/status.rs` |
| 没有现成的 fixture Rust 宿主；`rutis_host::host::Host::start` 作为库可用（不承诺 semver），loader 可以 `Builtins::register` 注册 Rust 行 | `rutis-host/src/lib.rs`、`rutis-loader/src/resolver.rs` |
| `cross_language.rs` 只 kill 宿主，没有 kill 运行时、没有比较其他运行时的 pid；probe 不报告 pid | `tests/e2e/tests/cross_language.rs` |

### 3.2 S3 跨语言组合与崩溃（#187）

一份 `rutis.json`：Python 行 `store`；TS 行 `report` 依赖 `store`；TS probe 依赖 `report`；Bun 行 `audit` 依赖 `report`（#200 评审的 Bun 变体）。

| # | 动作 | 断言 |
| --- | --- | --- |
| 1 | `rutis-host run`；等各行 `running`、probe `started` | probe 调 `report` 得到经过 `store` 的结果 |
| 2 | 在一次调用进行中（`store` 的方法阻塞在一个门文件上）`kill -9` Python 运行时 | 进行中的调用显式失败（C4）；`store`、`report`、`audit` 进入 `waiting for its services`；之后的调用显式失败、不挂起；Node、Bun 运行时 pid 不变（C3） |
| 3 | Ctrl-C；立即用同一份配置重启宿主（B10：socket、端口已释放） | 退出码 0；新宿主各行 `running`，调用恢复 |
| 4 | Ctrl-C | 残余检查通过 |

- 运行时 pid 由 probe 报告不了（probe 在 Node 运行时里），改为 harness 按进程树找出各运行时（3.4 的残余代码顺带提供"列出宿主的后代进程"）。
- 原 issue 的"配置编辑删除 Python 行后 Python 进程退出"移到 S4（那里有能改配置的宿主）。
- 平台：Linux、macOS；Windows 在 #232 之后，Bun 行在 Windows 上跳过并写原因（Bun 设计 §10）。

### 3.3 S5 两个宿主进程，loopback（#189）

节点 B：`python -m rutis listen:ws://127.0.0.1:<port>/rutis …`（按 `guide/nodes.md`，`rutis[network]`），提供 `infer`；每次执行打印一行，供 `Host::count` 计数。主机 A：`rutis-host run`，`runtimes.remote` 声明 B，`gpu:` 行使用 `infer`。

**断网用 harness 里的 TCP 代理**：A 连代理，代理连 B；`cut()` 断开所有连接并拒绝新连接，`restore()` 恢复。这样"断网"（第 2–3 步）和"B 重启"（第 4 步）是两件不同的事，也不需要容器。

| # | 动作 | 断言 |
| --- | --- | --- |
| 1 | 启动 B，再启动 A | `infer` 调用成功 |
| 2 | 调用进行中 `cut()` | 调用显式失败；B 侧该次执行计数 = 1（E1，不自动重试）；A 的 `gpu:` 行进入等待 |
| 3 | `restore()` | 重连后行恢复 `running`，调用成功 |
| 4 | `kill -9` B，再启动 B | A 重新拿到租约，调用成功 |
| 5 | 用另一个 token 重启 B | A 的重连被拒；A 的输出里有可读的拒绝类别；token 值不出现在任何输出里（B7） |
| 6 | 换成 wss：B 的证书主机名不是 `localhost`；另一次用过期证书 | A 连接被拒（E6 在两个进程间的版本） |

- 原 issue 的"吊销 A 的 token"没有对应机制，0.10 按第 5 步改写；运行中吊销是 #273 的需求，完成后在 S5 加一步（Q2）。
- A 的 link 状态不在状态行里，只能从行状态和 A 的错误输出判断。若评审认为需要，另开 issue 给状态行加 link 状态（不在本 PR）。
- 监听地址不打印，所以用 harness 的 `port()` 预留端口，不用 `:0`。
- 容器 + netem 的 nightly 版本（E12）在第三步。

### 3.4 E2E harness 要先补的（随 #232 的 PR）

| 补什么 | 原因 |
| --- | --- |
| macOS 残余：宿主启动时设环境变量 `RUTIS_E2E_SCENARIO=<id>`，残余检查用 `ps -E`（或 `ps eww`）按这个变量找进程，而不是按进程组 | 运行时各自一个进程组；宿主死后运行时被 launchd 接管，按组查不到。Linux 用 subreaper 已能查到，同样改为按变量查，两个平台一致 |
| Windows：每个宿主放进一个 Job Object（`CREATE_SUSPENDED` 启动，分配进 Job 后再恢复，避免分配前已经启动子进程）；Drop 时关闭 Job；残余检查用 `QueryInformationJobObject` 的进程列表 | #232 原文 |
| Windows 的 Ctrl-C：`GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT, pid)`（宿主已经是新进程组，`stop.rs` 处理 Ctrl-Break） | S2、S3、#266 的 Windows 停止检查 |
| `Scenario::descendants(host)`：列出宿主的后代进程及命令行 | S3 第 2 步比较 pid |
| `Scenario::proxy(target) -> Proxy`（`cut()`、`restore()`），代理的连接也登记进残余检查 | S5 |

`cross_language.rs:107` 的注释（"`run` 还没有信号处理"）已经过期，顺手改。

### 3.5 S4 实现迁移（#188）

宿主 A 是一个 fixture Rust 宿主 `tests/e2e/fixture-host`（工作区内的 bin crate，不发布）：用 `rutis_host::host::Host::start` 读 `rutis.json`，另外注册一个 Rust 实现的 `calendar` 内置行；从 stdin 读一行 `reload` 时重新读取 `rutis.json`，按差异做一次 loader 编辑，打印 `applied` 或 `rejected: <原因>`。harness 本身仍不依赖 rutis crate（规范 Q7.6），fixture 宿主是被测的一方。

| 阶段 | A 的配置 | 断言 |
| --- | --- | --- |
| 1 | `planner`（TS）+ `calendar`（Python） | 结果来自 Python |
| 2 | 删掉 Python 行，加上与节点 B（`rutis-host run`，WebSocket + token）的 link，导入 B 的 `calendar` | 结果来自 B；A 上 Python 运行时退出（原 S3 第 4 步） |
| 3 | 删掉 link，换成 Rust 内置的 `calendar` | 结果来自 Rust；到 B 的连接已关闭 |
| 全程 | `planner` 行不变 | A 的 pid 不变（G5）；切换期间 probe 持续调用，每次要么成功要么显式失败（G2）；每次切换后旧实现无残留（G3） |

用 stdin 指令而不是监视文件：切换时刻由场景决定，不依赖轮询间隔（规范 Q7.1）。如果 #274 决定 `run` 支持运行中改配置，阶段 1、2 可以改用 `rutis-host`，但阶段 3 仍需要能注册 Rust 行的宿主。

## 四、协议层（#178）

### 4.1 故障注入与链路（PR 1）

`channel::testing::fault(channel) -> (Channel, Faults)` 已有 `delay`、`half_open`、`drop_and_close`、`heal`，目前只在 `transport/memory/tests.rs` 自测。接到会话和 link 上：

- 进行中的调用遇到 `drop_and_close`：显式失败，对端执行计数 = 1（E1）；
- `delay` 下的调用与取消：取消到达对端，`AbortSignal` 触发；
- 断开重连期间发起的新调用：显式失败或在新会话上成功，不会落在旧会话上；
- 多跳（b → a → c）取消逐跳传递、释放逐跳传递（P6、E9）；
- N 个并发调用交错返回，结果与调用一一对应；
- 会话层大载荷（接近上限）与深嵌套值。

### 4.2 边界规则 3、4、5、7（PR 2）

`requirements-protocol-plugins.md` §5 共 8 条；1、2、6、8 已有对应测试（`projection_lifecycle.rs`、`event_forwarding.rs`、`rpc_callbacks.rs`、`bun_runtime.rs` 等），3、4、5、7 没有。每条一个跨进程测试，同一组操作在原生 Cordis 上跑一遍作为对照（与 `dsh_baseline.rs` 同样的做法）：

| 规则 | 测试 |
| --- | --- |
| 3 事件顺序 | 两侧各注册监听；断言另一侧的监听作为一组出现在固定位置 |
| 4 中间件链 | waterfall / bail 链两侧都有环节；断言对侧作为一个整体环节 |
| 5 `internal/*` | 一侧 emit `internal/*`，另一侧收不到 |
| 7 宿主服务 | rutis 提供的服务在 Cordis 侧 `instanceof` 不成立，按接口调用成立 |

测试注释写 `guarantee: boundary/5.3` 等编号。

### 4.3 SDK 对齐（PR 3）

| 缺口 | 补 |
| --- | --- |
| Python 会话测试没有 release 计数、grant 回滚 | 照 Node `session.test.mjs` 的对应用例补齐 |
| 两个 SDK 都没有"会话协议版本不认识"的测试；Python 协议版本写死（`peer.py` `PROTOCOL = 2`） | 两边各加一个版本 99 的握手测试，断言显式拒绝并说明哪一端要升级（P8）；Python 的常量保留，但加一个与 Rust `session::PROTOCOL` 比对的测试，防止只改一边 |
| `websocket_cross.rs` 只有 Node↔Rust | 加 Python 列（E8） |

### 4.4 负向 TLS（PR 4）

Rust 侧已有"CA 不受信任"。补：证书过期、主机名不匹配（rcgen 生成）；Python 侧这三种都补（`test_websocket.py` 现在只有 `ws://`）。E6 的两进程版本在 S5 第 6 步。

### 4.5 跨语言值往返（PR 5）

- `proptest` 加为 `rutis-bridge` 的 dev 依赖（`cargo deny` 检查）。
- 只在 Rust 侧生成值：字符串（含换行、非 BMP 字符）、整数（含 ±2^53 附近）、浮点（含 -0；NaN、Infinity 是否生成按转换表决定）、null、布尔、数组、对象、嵌套深度。
- 经 `runtime_conformance.rs` 现有的 `conformance-session` 回显 fixture，走 Node、Python、Go、Bun；每个运行时启动一次，跑 256 个用例；失败时打印种子和缩小后的值（规范 Q7.2）。
- 比较规则来自 `guide/plugin-api.md` 的值转换表；表里没写的情况（例如超过 2^53 的整数）测试先失败，再决定是改实现还是在表里写明。
- release 计数不变式（P4）：任意 grant / release 序列后，两边的引用表大小 = 未释放数。

不做：在 SDK 里生成值（与 Rust 驱动重叠，现状附录）；资源上限（等待设计）。

## 五、一致性套件的 Windows 列与缺失格子（#176）

整文件 `cfg(unix)` 的原因：

| 文件 | 真正依赖 Unix 的地方 | 处理 |
| --- | --- | --- |
| `runtime_conformance.rs` | `far_end` 用 `UnixListener` | 加一个 loopback TCP 的 `far_end`；Unix socket 列 `cfg_attr(windows, ignore = "…")` |
| `session_matrix.rs` | `symlink`、`fd:3` 断言 | Loopback、WebSocket 列在 Windows 上运行；Inherit、DialBack 列写明原因跳过 |
| `local_runtime.rs` | `sh -c`、`inherit_fd()` | 逐个测试标注 |
| `bun_runtime.rs`、`bun_channel.rs` | Bun 在 Windows 上未声明支持 | 保持，写明原因 |
| `concurrent_launch.rs`、`local_stdio.rs`、`local_spawn.rs`、`local_plugin.rs`、两个浸泡 | `/proc`、`dup2`、`fd:3`、Unix socket | 保持，写明原因 |

补的格子：会话契约 × WebSocket、loopback；运行时契约 × `Handover::Loopback`（Unix 上也跑，不再只靠 `RUTIS_LOCAL_HANDOVER` 间接覆盖）；节点契约 × Rust↔Rust WebSocket。

拆两个 PR：拆 `cfg` 与 Windows 列；补格子。矩阵报告是 `docs/ci.md` 里的一张手写表（实现 × 通道 × 平台，跳过的格子写原因），每次改格子同步改表。顺手修 `rutis_bridge::testing` 模块文档里过期的 feature 名（写的是 `conformance`，实际是 `testing`）。

## 六、测试工具与真实运行时（#206）

- 新增一份期望文件 `conformance-weather.expected.json`：一组调用（`today`、`later`、`each`，含回调、错误、值转换）和每次的期望结果。
- 真实运行时一侧：`session_matrix.rs` 的 `runtime::testing::runtime` 读同一份文件。
- 测试工具一侧：Node `node/rutis/test`、Python `python/rutis/tests` 各一个测试，用 `load()`（严格模式）加载同一个 fixture，按同一份文件调用、比对。
- Node 的 `conformance-weather.mjs` 只导出 `inject`、`apply`，没有 `provides` 和默认导出，测试工具的 `load()` 拒绝它；先改成 `definePlugin` 的形式（真实运行时两种都能加载）。
- 允许的差异（例如 `crash()` 在测试工具里不结束进程）在期望文件里逐条标为 `allowed`，并写进 `guide/plugin-api.md`（规范 Q6.7.2）。

Go 的 `rutistest`、Bun 也有测试工具，本次不做，写进 issue 作为后续（Bun 按 Q6.8 准入在第三步）。

## 七、持久化格式（#207）

| 格式 | 现状 |
| --- | --- |
| `rutis.json` | 没有版本字段；顶层、runtimes、listen 的未知字段是错误；行的未知字段只警告 |
| dsh 用户层 `<profile>/cordis.patch.yml` | `Patch` 数组，没有版本字段；`Patch` 的未知键进入 `overrides` |
| loader 的 `Persist` | `Version` 是乐观并发的令牌，不是格式版本；loader 自己不写文件 |

做法：

- 样本放 `tests/fixtures/persisted/<版本>/`：一份尽量用到所有字段的 `rutis.json`、一份 `cordis.patch.yml`；同目录放解析结果的规范化 JSON（期望）。
- 测试：当前代码读每个版本的样本，结果等于该版本的期望；不再支持的样本放 `rejected/`，断言错误信息含迁移说明。
- 从 0.8.0、0.9.0、0.9.1 的 tag 补样本（用那时的文档示例与测试配置），之后每次发布加一份。
- `docs/release.md` 步骤 1 加"保存本版本的样本"，PR 模板加一项"改了持久化格式？"。

**格式版本字段（Q4 已决定加）**：

| 格式 | 字段 | 规则 |
| --- | --- | --- |
| `rutis.json` | 顶层 `"format": 1` | 可省略，省略按 1 读（0.10 之前的文件都没有）；`rutis-host new` 生成的文件写上；比当前支持的大时报错："这个文件是格式 N，本 rutis-host 只读到 M，请升级 rutis-host" |
| dsh 用户层 `cordis.patch.yml` | 待定，二选一（在 #207 的 PR 里定） | 一：文件改为 `{format: 1, patches: [...]}`，仍接受顶层数组按 1 读——但 bundle 层也是同样的数组格式，两种写法并存；二：版本写在 profile 的 `package.json`（例如 `"rutis": {"profileFormat": 1}`），覆盖整个 profile 的所有文件，用户层文件本身不变。倾向二 |
| loader 的 `Persist` | 不加 | loader 不写文件，格式由实现 `Persist` 的一方（dsh）负责 |

- 读到旧版本：按旧版本的含义读，样本测试保证；读到新版本：显式拒绝，说明升级哪一个。
- 这是用户可见的格式变化，按规范 §9.3 在 0.10 的发布说明里写明；字段可省略，所以不需要迁移。

## 八、内核与登记表

### 8.1 内核保证（#177）

核对后的状态：

| 项 | 现状 | 做法 |
| --- | --- | --- |
| D8 没有 tokio runtime 时 `Ctx::root()` 报错 | 实现有，无测试 | 单元测试 |
| typed plugin 规则：`InjectUnsatisfied` 不进 ErrorSink；窗口内 `restart()` 返回 Ok；untyped 插件用 `get` 读不算证据；untyped `require` 失败回到 Pending | 都无测试 | `typed_plugin.rs` 加 4 个 |
| 异步 waterfall 回调 panic 传给调用者 | 只测了同步版 | 加异步版 |
| D29 事件不按 isolate 过滤 | 无回归测试 | 加 |
| 实例 ID 耗尽时 panic | 计数器是私有 static | crate 内单元测试，把计数器设到上限附近 |
| instance-subtrees §5：任务数、容量趋势、常驻兄弟 | 只查了 drop 数和条目数 | 任务数用 `Handle::metrics().num_alive_tasks()`；容量在 crate 内单元测试读取 |
| D11 `CordisError` 不实现 Clone | 无断言 | compile_fail 文档测试（不加依赖） |
| D24 | `contract.rs:334` 先排序再断言，掩盖了投递顺序 | 去掉排序，断言到达顺序；多 fiber 下 seq 单调 |
| D31 | 只有一个概率性的探针 | 多发射者各自顺序；panic 后下一次 emit 仍投递；listener 内同类型 emit 排到队尾 |
| D7 五步卸载 | 只有结果断言 | 每步留一个可观察的记录，断言顺序 |
| 丢弃 `restart` / `update` 的 future | 未测 | 首次 poll 前、后各一个 |
| 文档 | config-hot-update §7 写 16、§十一 写 14，实际 15；design-rust-port §5 列了不存在的 `agent_stop` | 改文档 |

拆两个 PR：事件与顺序（D24、D31、D29、异步 waterfall panic）；生命周期与回收（其余）。

### 8.2 保证登记表（#181）

- `docs/guarantees.md`（中英文），每行：编号、出处、内容、级别、状态（已验证 / 部分 / 未验证 / 明确不保证）、对应测试。
- 编号：`core/D1`–`core/D33`（D32、D33 在 config-hot-update 文档）、`core/features.<节>`、`boundary/5.1`–`boundary/5.8`、`loader/<名字>`、`instance/<名字>`、`bridge/<名字>`、`host/<名字>`。约 100 条，人工维护。
- 测试注释写 `guarantee: <编号>`（现在全仓只有 3 处）；PR 模板加一项。
- **先合骨架**（编号规则 + D 编号 + 边界规则），#177、#178 的 PR 在同一个 PR 里更新对应行的状态。
- Q9.4.4（每次发布"未验证"的核心承诺为零）：0.10 发布时登记表作为基线，从 0.11 起执行（决定 Q3）。

### 8.3 真实时钟（#231，剩 3 个 PR）

按已合入的 [设计](design-test-clocks-2026-10-11.md) 继续：loader（约 14 处固定等待，改用 `rutis-test-support`）；其他 crate（dsh、dev、host；不含示例项目 rutis-agent）；其他语言（Node、Python、Bun、Go，最后一个 `Closes #231`）。不需要新的决定。

## 九、顺序与 PR

```
第一波（基础，可并行）
  #265 CI e2e 任务（第一步收尾）
  #181 登记表骨架
  #232 harness：Windows Job Object + macOS 残余 + 代理 + descendants
  #231 2/4 loader
第二波（单元 / 契约，可并行）
  #177 ×2   #178 ×5   #176 ×2   #206   #207   #231 3/4、4/4
第三波（黑盒场景）
  #187 S3 → #189 S5 → #188 S4（含 fixture 宿主）
  #232 之后：S2 的 Windows 场景、#266 Windows 停止检查
  #266 其余格子（Bun 行、websockets 15.0、Go 行；Python 指南示例等 #259）
```

| 依赖 | 原因 |
| --- | --- |
| 场景 → #265 | 新的 E2E 测试要在 `e2e` 任务里跑，而不是挤在 `rust` 任务里 |
| S3、S5 → #232 的 harness 部分 | macOS 残余、`descendants`、代理 |
| S4 → S5 | 复用 S5 起节点 B 的做法 |
| #177、#178 → #181 骨架 | 测试注释用登记表的编号 |
| #206 → #176 不依赖 | 两者都改 `session_matrix.rs`，先合者优先，后者 rebase |

CI 需要的改动（写在各 PR 描述里，由 CI 的 PR 加）：`e2e` 任务装 `rutis[network]`（S5）；Windows 的 E2E 任务（#232 之后）；`proptest` 用例数在 PR 上 256、nightly 上更多（可选）。

## 十、需要维护者决定的（2026-10-11 评审）

| # | 问题 | 建议 |
| --- | --- | --- |
| Q1 | S3、S4 需要运行中改配置，`run` 不支持。给 `run` 加配置监视，还是测试用 fixture 宿主？ | **已决定**：0.10 不改 `run`，S3 重启宿主恢复，S4 用 fixture 宿主；`run` 是否支持运行中改配置见 #274 |
| Q2 | S5 的"吊销 token"没有机制 | **已决定**：作为对 remote 的需求开 #273；0.10 的 S5 先测换 token 重启后被拒 |
| Q3 | 登记表的发布门（Q9.4.4）从哪个版本执行 | **待定**：0.10 建立基线，0.11 起执行 |
| Q4 | 持久化格式要不要加版本字段 | **已决定**：加，见 §七；dsh 用户层放在哪里在 #207 的 PR 里定 |
| Q5 | 值往返超出转换表的情况（大整数等）发现后怎么处理 | **已决定**：先在表里写明当前行为，作为"明确不保证"登记；改实现另开 issue |
| Q6 | 本方案合入后，各项是逐个开 PR，还是在本 PR 里继续加实现 | **已决定**：逐个开 PR；本 PR 只保留方案 |
