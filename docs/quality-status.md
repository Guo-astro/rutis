# rutis 质量执行现状

[English](quality-status.en.md) · [质量规范](quality-standard.md)

状态：随代码持续更新。最近更新：2026-11-18（#1270 手段复核第二半：15 条内核／事件规则逐条定手段，新增内核模型 28（卸载四步的先后，`core/D7a`）与两消费者驱逐用例（`core/D14a`），内核 shuttle 模型 27 → 28、四个 crate 63 → 64；其余 13 条按已有模型、PT、K 或用例写清理由，见 §7.6 与[手段复核](formal-trace/means-0.23.md)）。上一次：2026-11-18（#1270 手段复核：21 条只有具名用例的规则逐条定手段，第一批 7 条 loader／host 集成层改为测试并补了 `MNT-11-4` 的替换与实例两处断言，见 §7.6 与[手段复核](formal-trace/means-0.23.md)；登记表 219 行、`guarantee:` 标记 1233 个，追踪表与协议复核的数字不变）。上一次：2026-11-17（按 `node tools/formal-trace.mjs` 与 `tools/check-guarantees.mjs` 的当前输出同步 §1、§2.3、§4.1、§6、§7.3、§7.6、§9.4 与附录：内核 shuttle 模型 24 → 27、`rutis-bridge` 26 → 30，四个 crate 共 63；`guarantee:` 标记 1226 → 1232；追踪表 359 已验证、423 部分验证、482 未验证、61 明确不保证、296 已撤回、204 不单独计数，协议复核 179 / 364 / 479 / 42 / 103 / 201，其中 67 条按证据与变异升级（35 + 32）；#613、#622、#624、#630、#631、#634、#635、#639 已关闭，残余 #1270–#1274 与 #776 仍开），基准 `origin/main` `3d8acdcf`（#1299）。上一次：2026-11-17（按当前树核准 §1.1、§3、§4、§7.1、§7.3、§7.5、§7.8、§9.4 的过期句：内核 shuttle 模型 16 → 24，`rutis-bridge` 26 个、`rutis-host` 3 个、`rutis-loader` 3 个各成一行；#1190、#1200、#1194、#1215 已合入；`model-pr` 检查已落地；Kotlin 的 Fray 与 Swift 的 TSan 按现状改写），基准 `origin/main` `acd4f165`（#1260）；追踪表与保证登记表的数字在同一提交上重跑，未变（`node tools/formal-trace.mjs` 0 problem，`tools/check-guarantees.mjs` 219 行、1226 个标记）。再上一次：2026-11-17（`guarantee:` 标记按 `tools/check-guarantees.mjs` 的当前输出从 1162 改到 1226），基准 `origin/main` `4e99f502`（#1256，0.22.0 的版本号）。再上一次：2026-11-16（按追踪表脚本的当前输出同步 Q9.4 与 §7.6：#1215 把 NOD-3-1 改记已撤回后，未验证 540 → 539、已撤回 294 → 295，协议复核 537 → 536、101 → 102），基准 `origin/main` `047c2386`（#1205）叠加 #1215（`47b62c85`；#1215 后来按 `c954dec9` 合入）。再上一次：2026-11-16，基准 `abfa7495`（#1202；按 #639、#1100、#1101、#1194、#1200 同步 Q9.4、§7.3、§7.6、§7.7 与 §9.4）；再上一次：2026-11-16，基准 `f70156be`（按 #1190、#624、#1181/#1183、#1185 更新 §1.2、§4、§7.2、§7.5、§9.4 与附录）；更早：2026-11-16，基准 `328c228a`（#613）；2026-11-15，基准 `1dd73888`。
对应：[质量规范](quality-standard.md)。本文把规范逐条对应到当前的实现：哪些条款已满足、哪些部分满足、缺什么、计划怎么补。规范描述长期不变的规则；本文中的文件名、测试名、issue 号、版本号都会随实现变化。每条保证由哪些测试验证，见[保证登记表](guarantees.md)。

---

## 1. 按条款的满足情况

状态：**满足**、**部分**、**未满足**、**不适用**。缺口一栏引用 §4 的风险编号和 issue。

### 1.1 横向要求

| 条款 | 内容 | 状态 | 现状与缺口 |
| --- | --- | --- | --- |
| Q3.1 | 核心承诺按最高级验证 | 部分 | "结束后不留残余""中断时显式失败"已在黑盒层和三个平台上验证：E2E 场景结束时做残余检查（#221、#277）；S3、S5 和 `fault_links.rs` 验证中断时调用显式失败、不重试（#317、#322、#285）。缺：登记表中 `core/D22`、`core/D28`、`boundary/5.8` 为"部分验证"；Unix 上事件循环不空的运行时在宿主被 `kill -9` 后不退出（B3）；长时间运行由 S8 每晚在三个平台上验证（#192） |
| Q3.1、Q8.3 | 核心承诺由两种独立方式验证 | 部分 | Python SDK 的会话测试已与 Node 对齐（release 计数、grant 回滚、协议版本，#288）；跨运行时同步调用、远端租约、统一事件、行与调用生命周期、节点托管行有 TLA+ 设计模型（7 个目录、17 个模块，§7.2），会话通道有 trace validation（§7.5）；内核 28 个、`rutis-bridge` 30 个、`rutis-host` 3 个、`rutis-loader` 3 个 shuttle 模型（§7.3）。没有逐条核对每个核心承诺是否有两种独立验证（登记表只记一列"验证"） |
| Q3.4 | 明确不保证的行为有锁定 | 部分 | D29（事件不按 isolate 过滤）有回归测试（`event_order.rs`，#280）；值的范围限制登记为明确不保证并有测试（`bridge/value-limits`，#296）。D22（不检测依赖环）仍没有测试锁定 |
| Q4.5 | 风险评估结论有记录 | 部分 | 本文 §4 每一步结束时更新（本次 2026-11-17）；设计文档没有标风险（Q9.1.1） |
| Q5.1.2 | 验证检查结果 | 部分 | 已知的两处已改：`typed_plugin.rs` 断言 ErrorSink 收到的内容（#286）；`contract.rs` 的 D24 断言到达顺序，不再先排序（#280）。没有系统地审查其他测试 |
| Q5.2 | 故障清单逐项注入验证 | 部分 | 已注入：链路在调用中途断开、延迟下的取消与释放、重连期间的新调用、多跳（`fault_links.rs`，#285）；运行时 `kill -9`（S3，#317）；两个宿主之间断网、对端重启、换 token（S5，#322）；两个容器之间的真实网络：200 ms 延迟 + 5% 丢包、断网与恢复、换 token（S5 容器版，#433，nightly）；并发启动（`concurrent_launch.rs`，#215）；宿主收到信号（`signals.rs` #216；E2E 的 Ctrl-C、Windows Ctrl-Break #277）；协议版本不匹配（#288、#335）；畸形帧（`malformed`，25 个用例 × 七种实现）。运行中配置被改坏（`host/reload-config`，#274）；长时间运行中的反复故障：随机的增删改行与重新加载、被拒绝的配置、运行时 `kill -9` 后重启、断链重连（S8，#192）。缺：插件阻塞事件循环（`boundary/5.8` 部分验证） |
| Q5.3.2 | 跨机加密、非回环必须 TLS | 部分 | 负向 TLS：证书过期、主机名不匹配、CA 不受信任，Rust（`websocket.rs`）与 Python（`test_websocket.py`）各自验证（#297）；两个宿主进程之间也验证（S5 第 6 步）；两个容器之间在非 loopback 地址上用测试时生成的 CA 走 `wss://`（S5 容器版，#433）。Windows 上客户端证书无效的类别已修（#300 → #334）。缺：无 TLS 的监听绑定到非回环地址是否被拒绝，未核对（B8） |
| Q5.3.3 | 凭据不进输出 | 部分 | E2E 每个场景结束时检查全部捕获输出中没有本场景的凭据（#221、#277）；S5 被拒时 token 不出现在输出里（#322）。库级的日志、trace、诊断输出没有统一检查 |
| Q5.3.4 | 外部输入有上限、畸形输入不崩溃 | 部分 | WebSocket 16 MiB 上限；本地行分帧有上限（#173 → #214）；值的嵌套深度上限 64 层，超限只让这次调用失败（#279 → #338）；畸形帧检查覆盖七种实现（含 #294 → #305 之后加入的 Go）；模糊测试：会话帧、本地行分帧、错误图、WebSocket 升级请求、dylib 元数据与二进制清单、dsh profile 的 YAML 与表达式，以及 Go 的 RFC 6455 帧解析（Go 原生 fuzz），`stress.yml` 每晚各跑一段时间（#179 → #425、#624、#631） |
| Q5.3.6 | 不安全代码集中、加载前校验 | 部分 | `unsafe` 集中在 dylib 加载与进程启动；dylib 校验由脚本验证，`design-dylib-sdk` §十三 5 项未勾选，需要对账 |
| Q5.3.7 | 依赖漏洞与许可证检查 | 部分 | 每周 `deps.yml`：`cargo deny check advisories`、`npm audit`、`pip-audit`（#256），不阻止合并。许可证没有检查；新增依赖的理由没有固定的检查项 |
| Q5.4.2 | 每个场景结束核对残余 | 部分 | E2E harness 每个场景结束时核对：宿主拉起的进程（Linux、macOS 按场景标记找，Windows 按 Job Object）、socket 文件、端口、临时目录、凭据（#221、#277）。测试进程的 fd、线程只在两个浸泡测试里查；tokio 任务数与 Linux 上的 OS 线程数在 `instance_subtrees.rs`、`instance_runtimes.rs`、`close_memory.rs` 查（#286、#191、#979）；运行时内部的注册由 Node、Python 运行时的测试控制方法 `inspect` 查询，只有 S7 用到（§5，#191） |
| Q5.4.3 | 反复创建销毁后资源不增长 | 部分 | bridge 层两个浸泡（反复建链断链、起停进程，查测试进程的 fd、线程）；宿主进程级的 S8（`tests/e2e/tests/soak.rs`，#192）：一个 `rutis-host run` 每晚运行一小时，反复增删改行并重新加载、杀掉运行时由宿主重启、断开对端的 link，采样宿主、它的运行时和对端的进程数、fd（Windows 上是句柄）、线程、常驻内存，后 25% 时间的中位数与前段（跳过前 10% 预热）的中位数比较，不超过阈值（进程数最多多一个：Node 运行时的 TypeScript 加载器按需起 esbuild 进程；其余阈值及理由见 `soak.rs` 的 `Bounds`），Linux、macOS、Windows。缺：内核层 root 上反复创建销毁实例（K5，#191）；tokio 任务数只在 `instance_subtrees.rs`、`instance_runtimes.rs`、`close_memory.rs` 查 |
| Q5.4.4 | 宿主被强制终止时不留孤儿进程 | 满足 | `cross_language.rs` 在三个平台上 `kill -9` 宿主，残余检查确认运行时退出（Unix 靠通道关闭，Windows 靠 rutis-host 的 Job Object）。Unix 上插件留着定时器、或清理一直不返回时，运行时也在截止时间内卸载各行后退出（#353，由 #354 修复；`stray_runtime.rs`、`rutis-bridge/tests/host_gone.rs`，Node、Python、Go、Bun）。S8 每晚在三个平台上轮换 Ctrl-C（Windows 上 Ctrl-Break）、SIGTERM、`kill -9` 结束宿主，`kill -9` 前在 Node、Python、Bun 运行时里各加一个清理永不返回的行，检查宿主的进程在限定时间内全部退出、运行时自己卸载各行并守住截止时间（#192）；它发现宿主被 `kill -9` 后 Python 运行时到截止时间写不出放弃清理那一行（stderr 管道的读端随宿主没了），一直不退出，已修复（`stray_runtime.rs` 的 Python 例） |
| Q5.5.2 | 多写者冲突检测 | 部分 | loader 实现了 CAS 与重放；设计 §十七 列出的多写者测试需逐条核对 |
| Q5.5.3 | 持久化格式向前兼容 | 部分 | `rutis.json` 有顶层 `format`（省略按 1 读，比当前新时拒绝并提示升级）；`tests/fixtures/persisted/` 存 0.8.0、0.9.0、0.9.1 的样本，`rejected/` 断言错误信息；`docs/release.md` 每次发布加样本（#207 → #284）。dsh 的 `cordis.patch.yml` 按决定不加版本字段（跟随 Cordis，方案 Q4），只靠样本测试，与本条"每种格式都有版本"不完全一致 |
| Q5.5.4 | 跨语言值往返验证 | 满足 | 转换规则在 `guide/plugin-api.md`；`value_roundtrip.rs` 用 proptest 在 Rust 侧生成值，经 Node、Python、Go、Bun 往返比较，每个 256 例，失败打印种子，另有 release 计数不变式（#296）；发现的问题已修（#290 → #312、#302 → #311、#279 → #338）。表外的值（Date、Map、Set、二进制、大整数）显式拒绝，登记为明确不保证（方案 Q5；是否支持见 #308）。只在 Unix 上运行 |
| Q5.6.1、Q5.6.3 | 错误信息内容与锁定 | 部分 | `rutis-host` 的常见错误逐字锁定（`rutis-host/tests/errors.rs`，#205 → #217）；协议版本不匹配时说明升级哪一端（#293 → #335）；link 被拒的类别出现在 `run` 的输出里（#321 → #336）。SDK 与 bridge 库级的错误没有锁定 |
| Q5.6.2 | 不依据错误文字分支 | 满足 | 建立连接的错误按结构化类别分类，有跨实现测试 |
| Q5.7.1 | 性能数字标明承诺或参考值 | 部分 | "同步调用约 30 µs"在设计哲学中未标明性质；基准测试无固定环境 |
| Q5.7.2 | 声明规模范围 | 未满足 | 没有对外声明；本文 §2.2 的规模为待确认的假设。连续运行时间：S8 每晚验证一个宿主一小时内资源没有增长趋势（#192），更长时间没有验证 |

### 1.2 组件类型

| 条款 | 组件 | 状态 | 现状与缺口 |
| --- | --- | --- | --- |
| Q6.1 | 内核 `rutis` | 部分 | 契约、Cordis 对拍、交错测试；#177 补齐事件顺序（D24、D31、D29）与生命周期、回收（D7、D8、丢弃 future、instance-subtrees §5 的任务数）（#280、#286，后续 #345）。可控调度的模型检查：内核 28 个 shuttle 模型（K2、#622，§7.3），`rutis-bridge` 30 个、`rutis-host` 3 个、`rutis-loader` 3 个（#622、#627、#628），`stress.yml` 每晚把 `rutis` 与 `rutis-bridge` 的每个跑到 100 万次迭代。缺：`rutis-host`、`rutis-loader` 的模型不在任何 workflow 里跑；dylib 没有模型；契约测试大多只用单线程 runtime；D22 的环、D28 的 isolate 所有权没有测试 |
| Q6.2 | 会话协议、运行时控制操作、节点操作 | 部分 | 一致性套件；畸形帧、未知 op、引用不存在（`malformed`，25 个用例 × 七种实现）；协议版本不匹配（三种 SDK 单元测试 #288，信息 #335）；多跳取消与释放、故障注入（#285）；跨运行时同步调用策略经 TLA+ 选定并实现（#222、#261；模型见 §7.2 的 T1、T5）；上一个发布版本与当前代码相遇（#209 → #426）。缺：引用与调用数的资源上限（等待设计）；取消表清理与取消确认未定（取消之后可能回也可能不回，调用方必须接受两种，`SESS-61`；#1099）。会话帧解码有模糊测试（#179 → #425） |
| Q6.3 | 本机 socket、继承 fd、loopback、WebSocket、内存通道 | 部分 | Rust 通道全部跑通道契约；本地通道有大小上限（#214）。Node、Python 的通道实现没跑通道契约（P1） |
| Q6.4 | 帧解码、分帧、握手、配置、清单、patch 合并 | 部分 | patch 合并有差分验证（以 cordis-plugin-include 为参照）；行分帧上限与嵌套深度上限有边界值测试（#214、#338）；`rutis.json` 每个字段有单元测试（#217）；帧解码（含 0.11 的十种带标签值，解码→编码→解码稳定）、本地行分帧、WebSocket 升级请求有模糊测试，语料在仓库里（#179 → #425）。缺：配置文件的模糊测试（低优先级） |
| Q6.5 | 拉起进程、持有连接的部件 | 部分 | 并发启动的竞态已修（#184 → #215）；E2E 场景结束时在三个平台做进程残余检查；宿主被强制终止后运行时退出（Q5.4.4）；长时间运行与每种退出方式（S8，#192）。缺：Q5.4.2 中运行时内部注册表的基线（没有查询接口，§5） |
| Q6.6 | `rutis-host`、`rutis.json`、项目模板 | 部分 | 有黑盒测试：E2E harness 启动真实的 `rutis-host`（#221），S2 开发循环、S3、S5、S9 安装（#253、#317、#322、#254），三个平台；信号处理（#174 → #216）；配置、装配与常见错误输出（#217）；模板生成的项目在 S2 中安装、测试、运行。运行中改配置（#274）。缺：Windows 上只验证了 Ctrl-Break，Ctrl-C 与关闭控制台没有 |
| Q6.7 | Node / Python SDK 与测试工具 | 部分 | 两个 SDK 都有测试工具和严格模式；测试工具与真实运行时对同一份期望文件比对（#206 → #298，`sdk/testing-matches-runtime`）。Go 的 `rutistest` 与 Bun 的测试工具没有比对 |
| Q6.8 | Node、Python、Go 运行时；Bun | 部分 | Node、Python、Go 通过一致性检查；Windows 上会话一致性与运行时一致性矩阵的 loopback、WebSocket 列运行（#176 → #295、#309；矩阵见 [ci.md](ci.md) §4）。跨运行时同步调用：策略 B 已实现（#228 → #261）。Bun：B1 已合入（#200），按 Q6.8 准入的 B2、B3 未做（#194）；Windows 不是 Bun 声明支持的平台 |
| Q6.9 | 跨实现服务契约 | 部分 | 方法形状可声明；各语言都能给错误命名；值往返有属性测试（#296）；Node、Python 测试工具与真实运行时一致（#298）；替换有端到端验证（S4，#326）。缺：Go、Bun 测试工具的比对；Date、Map、Set、二进制、大整数一律拒绝（#308 等设计）。不同实现的数据是否一致不在范围（第 4 条，2026-10-11 决定） |
| Q6.10 | loader 可编辑层、dsh profile 文件 | 部分 | 原子写、冲突重放已实现；dsh 用户层有各版本样本（#284）。S7：嵌入应用自己的 `Persist`（JSON 文件，写到旁边再改名，内容哈希作版本）在真实进程重启后恢复可编辑层，状态与重启前一致；被拒绝、回滚的编辑不写存储（#191）。多写者测试未逐条核对（Q5.5.2） |
| Q6.11 | Cordis 绑定生成 | 满足 | 生成器有单元测试，真实插件基线编译调用；外部新版本每晚用最新已发布的插件重新生成、编译、调用，只报告（F4，`nightly.yml` 的 `cordis-latest`，#190 → #429） |
| Q6.12 | dylib 加载 | 满足 | Linux、macOS 在 PR 改 dylib 时和每晚运行；Windows（`dylib-windows.yml`）在每晚和发布门里运行（PR 上不跑，#569；ci.md §2）。加载前校验的对账见 Q5.3.6 |
| Q6.13 | 节点互联、远程运行时、租约 | 部分 | 库级测试完整；两个独立宿主进程之间经 loopback 验证（S5，`nodes.rs`，#322、#336）：断网时进行中的调用失败一次、重连恢复、对端重启后拿到新租约、换 token 被拒、证书主机名不匹配与过期被拒；两个容器之间经真实网络（E12，#341 → #433，nightly 只报告）：非 loopback + TLS，200 ms 延迟 + 5% 丢包下调用成功、只执行一次、link 不断，断网时进行中的调用失败一次、心跳超时报告、退避上限内重连。缺：运行中吊销凭据（#273）；link 实际重试时序与稳定 60 s 后重置（E3）。租约、接管、重连与吊销有 TLA+ 模型（E4、E5，#210） |
| Q6.14 | Cordis 挂载与 Cordis 节点 | 部分 | 以原生 Cordis 为参照的差分验证；边界规则 3、4、5、7 有跨进程测试（`boundary_rules.rs` 等，#283），测试发现的静默降级改为明确报错（#281、#282 → #337）。用 `rutis.json` 挂载已发布插件的黑盒场景 S6（`tests/e2e/tests/cordis.rs`，#190 → #429）：依赖 rutis 一侧服务的插件所做的调用与原生一致，服务撤回、恢复时按原生规则停止、重启，未捕获的异常明确报错；`rutis.json` 挂载的已发布插件之间按原生规则互用服务，提供的服务自动投到 rutis，probe 经这些服务的调用与原生一致（#428）；新发布的版本每晚跑一遍，只报告（F4）。缺：跨边界的 `serial` 在对侧也按 serial 执行，需要协议改动（#340）；`boundary/5.8` 部分验证 |
| Q6.15 | npm、PyPI、二进制、crates.io | 部分 | `install.yml`：npm、PyPI、release 二进制在干净目录安装运行，每晚五个平台，PR 上 linux-x64，发布前五个平台；发布后用注册表上的包跑文档示例（#193 → #254，#266 → #324）。缺：crates.io 与 Go 模块没有安装验证；Python 指南示例的标记（#266 (d)，等 #259） |
| Q6.16 | `rutis-dev` dev 通道 | 不适用 | 0.19 删除 `rutis-dev`（#694） |

### 1.3 测试规则、流程、矩阵、发布

| 条款 | 内容 | 状态 | 现状与缺口 |
| --- | --- | --- | --- |
| Q7.1 | 确定性 | 满足 | 依赖真实时钟的断言已去掉：内核 #182 → #220；其他 crate 与 Node、Python、Bun、Go #231 → #257、#278、#299、#301；共用的等待工具在 `rutis-test-support`。之后出现的偶发失败按 Q7.3 登记修复（§9.2） |
| Q7.2 | 可复现 | 满足 | `interleave.rs`、`stress.yml` 每轮打印种子，作业摘要给出重放命令（#220、#245）；属性测试与 `fault_links.rs` 打印种子（#296、#285） |
| Q7.3 | 不掩盖失败 | 满足 | CI 不自动重试；偶发失败先登记 issue 再修（ci.md §7）。0.10 期间登记并修复了 #289、#303、#306、#307、#314、#316、#342 等 |
| Q7.4 | 后台异常导致失败 | 部分 | Python 测试中后台线程的异常让测试失败（#197 → #218）；E2E harness 遇到被拆开的探针行即失败；PR 工作约定有这一项。Rust、Node 测试没有统一的机制 |
| Q7.6 | 黑盒与白盒分开 | 满足 | 黑盒层 `tests/e2e`：harness 不依赖任何 rutis crate，只通过二进制、配置文件和探针插件操作（#175 → #221）；S4 的 fixture 宿主是被测的一方 |
| Q7.7 | 参照可再生 | 部分 | patch 参照文件有生成说明，CI 不核对 |
| Q7.8 | 跳过必须说明 | 部分 | 只在部分平台运行的测试文件在 `cfg` 旁写原因，跳过的格子用 `ignore = "原因"`；一致性矩阵写在 ci.md §4（#176）；残余检查报告本平台未运行的项。`rutis-dev/tests/channel.rs` 的 `cfg(unix)` 没有写原因 |
| Q8.1 | 三个层次 | 部分 | 规则层充分；组合层有黑盒 E2E 与故障注入（S2–S5、`fault_links.rs`）；环境层有支持矩阵与干净环境安装（S9）、真实网络（S5 容器 + netem，#433）；长时间运行（S8，#192） |
| Q9.1.1 | 设计文档包含级别、风险、验收 | 部分 | 多数设计有验收节；PR 模板加了持久化格式与保证登记的检查项（#284、#276）；规范本节加了第 5 项（改了多方交互规则时写"模型检查"一节，或写明为什么不需要）。缺：设计文档没有标级别与风险；第 5 项的检查在 `model-pr.yml` 里跑（`tools/check-model-pr.mjs`），不在 `ci-ok` 里（§7.8） |
| Q9.3.2 | 不兼容变化检测与跨版本相遇 | 部分 | `cargo semver-checks` 只提示；协议版本不匹配时说明升级哪一端（#335）；上一个发布版本（运行时、Go SDK、`rutis-host`、节点）与当前代码相遇，断言正常工作或在加载、连接时说明升级哪一端（`tests/e2e/tests/cross_version.rs`，完整运行，#209 → #426） |
| Q9.4 | 保证登记 | 部分 | [登记表](guarantees.md)已建立（#276、#310，之后各 PR 顺带登记），测试注释写 `guarantee: <编号>`；0.10 作为基线，Q9.4.4 从 0.11 起执行（方案 Q3）。规则按条登记在[追踪表](formal-trace/README.md)：1825 行，359 已验证、423 部分验证、482 未验证（§7.6）。登记表本身覆盖 §6–§15 共 219 行（#181 已关闭） |
| Q10.2 | 支持矩阵最低与最新版本 | 满足 | Linux 任务用最低版本（Node 22、Python 3.12、websockets 15、Bun 1.4.0、Go oldstable），macOS、Windows 用最新版本；MSRV 1.88 由 `msrv` 任务验证（#204 → #256；ci.md §9）。运行时 × 通道 × 平台矩阵写在 ci.md §4，不运行的格子写原因（#176） |
| Q11.3.2 | 不以覆盖率为门槛 | 满足 | — |
| Q12.1 | 分级运行 | 部分 | PR 按改动选择；每晚（`nightly.yml`，main 有新提交时）运行全部任务、五平台安装（`install.yml`）、Windows dylib、MSRV、feature 组合、两机可复现，以及 `stress.yml`（内核多轮、多进程测试多轮 #245、两个浸泡、`rutis` 与 `rutis-bridge` 的 57 个 shuttle 模型每晚每个 100 万次迭代（`rutis-host`、`rutis-loader` 的 6 个不在 workflow 里）、九个模糊测试目标 #425、事件的全部放置 #464）；跨版本相遇（`ci.yml` 完整运行的 `e2e`，#209）、最新已发布 Cordis 插件的基线（只报告，#190）、S5 容器 + netem（只报告，#341 → #433）；每周依赖检查与 CI 度量。缺：每周的长时间宿主浸泡与基准测试（#192） |
| Q12.3 | 按改动选择 | 满足 | `changes` 任务按改动打开开关；每晚 `nightly.yml` 在 main 有新提交时运行全部任务（`ci.yml` 的 `all`），也可以手动运行（#203 → #213、#369；ci.md §3）。发布前：release.md 第 2 步确认发布的提交上有通过的每晚运行；`release.yml` 自己调用完整 CI 见 CI 重新设计的第 3 步 |
| Q12.4 | 取消被取代的运行 | 满足 | PR 上新推送取消旧运行（#224）。合并到 main 不再运行；每晚的运行检查上一次成功的每晚运行以来合并的所有提交，不被取消；失败时开问题单（标签 `ci`），写明失败的任务和提交范围（#369；ci.md §7） |
| Q12.5 | 统一的合入条件 | 满足 | `ci-ok` 是唯一必需检查（#213；ci.md §5） |
| Q12.6 | 时间以等待时间计 | 部分 | 目标：普通代码 PR 从推送到结果不超过 10 分钟，含排队（ci.md §1）；时间要求只针对 PR 和发布，每晚的运行不计。2026-10-12 的统计：中位 15.6 分钟、p90 42.9（9 次，多数在切换自托管 runner 之前或当中，§8.3）。切换后的数据还不够 |
| Q12.7 | 不重复 | 部分 | 重复的发布构建已去掉（#265 → #275）；PR 上可复现检查只在 `repro` 打开时在 dylib 任务里做单机检查，两机比对的 `sdk-repro*` 只在完整运行（#213、#275）。完整运行中单机与两机两种可复现检查并存 |
| Q12.8 | 主线失败优先处理 | 满足 | ci.md §7 写明流程 |
| Q12.9 | 度量 CI | 满足 | `tools/ci-stats.mjs` 与每周的 `ci-stats.yml`（#213）；§8.1、§8.3 为两次统计 |
| Q12.10 | CI 配置是代码 | 满足 | CI 改动单独开 PR，运行全部任务（ci.md §6） |
| Q13 | 发布门 | 部分 | 有版本一致检查、打包、两机冒烟、发布前五平台安装、发布后用注册表上的包验证（#254）、保存持久化样本（#284）、跨版本相遇（发布门调用的 `ci.yml` 完整运行，#209）。缺：登记表核对从 0.11 起（方案 Q3） |

---

## 2. 项目现状

### 2.1 组成

| 部分 | 内容 | 谁直接使用 |
| --- | --- | --- |
| 内核 `rutis` | 插件、fiber 生命周期、依赖门控、服务、事件 | Rust 嵌入方；其他所有部分 |
| `rutis-loader` | 按数据（行）管理一组插件：分层配置、命令式修改、持久化、实例 | Rust 嵌入方；rutis-host |
| `rutis-bridge` | 通道、会话协议、语言运行时、节点互联、Cordis 挂载 | Rust 嵌入方；rutis-host |
| `rutis-host` | 不写 Rust 的宿主：`run` / `dev` / `check` / `new` + `rutis.json` | 插件作者；运维 |
| Node 包 | `@arcships/rutis`（插件 SDK 与测试工具）、`@arcships/rutis-runtime`（运行时、Cordis 节点桥）、`@arcships/rutis-host`（分发） | 插件作者；Cordis 应用 |
| Bun 包 | `@arcships/rutis-bun`（Bun 运行时，#194 B1） | 插件作者 |
| Python 包 | `rutis`（插件 SDK、测试工具、运行时）、`rutis-host`（分发） | 插件作者 |
| Go 模块 | `github.com/arcships/rutis/go/rutis`（插件 SDK、`rutistest`、运行时） | 插件作者 |
| dylib 工具链 | `rutis-sdk`、`rutis-dylib*` | Rust 插件作者 |
| 应用 | `rutis-dsh`、`rutis-agent` | 本仓库自用，也是框架的第一批使用方 |

### 2.2 运行条件与规模假设

| 条件 | 取值 | 来源 |
| --- | --- | --- |
| 操作系统 | Linux、macOS、Windows x64（MSVC）；Bun 不含 Windows | guide/README |
| 语言环境 | Node ≥ 22、Python ≥ 3.12、Rust ≥ 1.88、Bun ≥ 1.4、Go ≥ 1.24 | guide、0.10 发布说明 |
| 可选依赖 | `websockets` ≥ 15（Python 网络） | pyproject |
| 进程结构 | 宿主一个进程；每种语言运行时至少一个进程；插件可能自己再启动子进程 | 多语言设计 |
| 网络 | 本机：Unix socket / 继承 fd / loopback TCP；跨机：WebSocket + TLS，可能经过反向代理，可能断网、半开、延迟 | 网络栈设计 |
| 信任 | 插件可信；网络和远端机器不完全可信；进程外运行不是沙箱 | 远程设计 §7 |
| 版本 | 0.x，几天一个版本，minor 允许破坏性变更；宿主、运行时、SDK 各自独立升级 | release.md |

**规模假设**（待确认，影响 §4 中性能与浸泡相关风险的评级）：

| 项 | 假设的典型值 | 假设的上限 |
| --- | --- | --- |
| 一个宿主的行数 | 10–50 | 500 |
| 语言运行时实例 | 1–3 | 10 |
| 节点数 | 1–3 | 20 |
| 一个嵌入应用的实例（租户、会话） | 数十 | 数千，反复创建关闭 |
| 连续运行时间 | 数周 | 数月 |
| 开发期热重载次数 | 一次会话数百次 | — |
| 跨进程调用频率 | 每秒数十 | 每秒数千（同步调用约 30 µs） |

### 2.3 现有测试

计数方法：Rust 数 `#[test]` 与 `#[tokio::test]` 属性，Node / Bun 数 `test(` / `it(`，Python 数 `def test_`，Go 数 `func Test`，Kotlin 数 `@Test`，Swift 数 `func test`。**2026-11-15 重新统计**（基准 `origin/main` `1dd73888`）。行覆盖率一列仍是 2026-10-10 在两个平台上实测的值（#183，含 Rust 测试拉起的 Node / Python 子进程），之后没有重新测，只作量级参考。

| 部分 | 测试 | 测试函数数 | 行覆盖率（2026-10-10） |
| --- | --- | ---: | ---: |
| 内核 `rutis` | 46 个文件：契约 `contract.rs`、Cordis 对拍 `parity.rs`、交错 `interleave.rs`、监听器表模型 `listener_table_model.rs` 等；8 个 doctest、4 个 `compile_fail` | 413 | 92.8% |
| `rutis-bridge` | 83 个文件；channel / session / runtime / node 四套一致性套件；`fault` 装饰器；`trace_validation.rs`；两个浸泡测试 | 593 | 87.8% |
| `rutis-loader` | 61 个文件；patch 合并以 cordis-plugin-include 为参照；各语言的行契约（`*_rows.rs`） | 335 | 89.8% |
| `rutis-host` | 56 个文件，含 `tests/` 下的黑盒测试 | 291 | 69.8%（`main.rs` 0%） |
| `tests/e2e` | 黑盒层，不依赖任何 rutis crate（Q7.6） | 91 | — |
| dylib 工具链 | `tools/test-dylib*.sh`、`tools/test-sdk-bundle.sh`；两个 runner 比较 SDK 哈希 | — | — |
| Node 包 | `node/*/test/`，48 个文件 | 268 | 86% |
| Bun 包 | `bun/rutis-bun/test/`，23 个文件 | 159 | — |
| Python 包 | `python/rutis/tests/`，27 个文件 | 284 | 81% |
| Go 模块 | `go/`，39 个文件 | 176 | — |
| Kotlin | `kotlin/`，23 个文件 | 124 | — |
| Swift | `swift/`，4 个文件 | 32 | — |
| 设计模型 | `specs/` 7 个目录、17 个 `.tla`、220 个配置（§7.2） | — | — |
| 可控调度 | 四个 crate 共 64 个 shuttle 模型（内核 28、`rutis-bridge` 30、`rutis-host` 3、`rutis-loader` 3），`--cfg rutis_shuttle` 才编译（§7.3） | 64 | — |
| 模糊测试 | `fuzz/fuzz_targets/`，9 个目标（§7.4）；另有 Go 原生 fuzz | 9 | — |

0.11 以后新增、现在仍有效的几块（细节在 §4、§5、§7）：

- 跨语言值类型（日期时间、二进制、Map、Set、大整数、Decimal、UUID，0.11）与七种语言的值往返属性测试；
- 远程运行时拨出、接入点与 Swift / Kotlin、iOS / Android 应用托管（0.13–0.18）；
- 四个 crate 的 64 个 shuttle 模型（内核 28），`stress.yml` 每晚把 `rutis` 与 `rutis-bridge` 的 57 个大规模运行；`fuzz/` 的第四个目标（错误图）；
- `specs/` 从 2 个目录扩到 7 个目录、17 个模块、220 个配置；
- trace validation 的第一块（会话通道，`trace_validation.rs`）；
- 测试时钟（`virtual_clock_contract.rs`、`design-test-clocks-2026-10-11.md`）与上一个发布版本与当前代码相遇（`cross_version.rs`）。

0.9、0.10 新增的部分（历史记录，截至 `f8ce69b`）：

| 部分 | 内容 | PR |
| --- | --- | --- |
| E2E harness `tests/e2e` | 启动真实的 `rutis-host` 进程与各语言的探针插件；不依赖 rutis crate（Q7.6）。每个场景结束时做残余检查：进程（Linux、macOS 按场景标记 `RUTIS_E2E_SCENARIO`，Windows 按 Job Object）、socket 文件、端口、临时目录、输出中的凭据。Windows 上用 Ctrl-Break 停止宿主；`descendants` 列出宿主的后代进程；TCP 代理（`cut`、`restore`）模拟断网 | #221、#277 |
| 场景 S2 开发循环 | `dev_loop.rs`：`new` → 模板测试 → `dev` → 热重载 → 写坏文件 → `check` → `run` → Ctrl-C；Node、Python、Bun、Go；三个平台（Windows 不含 Bun） | #253、#323 |
| 场景 S3 跨语言与崩溃 | `runtime_crash.rs`：Python、TS、Bun 行，调用进行中 `kill -9` Python 运行时，调用显式失败、其他运行时 pid 不变，重启宿主后恢复；`cross_language.rs`：TS ↔ Python，`kill -9` 宿主后运行时退出 | #317、#221 |
| 场景 S4 实现迁移 | `migration.rs` + fixture Rust 宿主 `tests/e2e/fixture-host`：`calendar` 从 Python 换到节点 B 再换到 Rust，`planner` 不变、宿主 pid 不变、切换期间每次调用要么成功要么显式失败、旧实现无残留 | #326、#330 |
| 场景 S5 两个宿主进程 | `nodes.rs`：loopback 上的两个宿主，断网、对端 `kill -9` 后重启、换 token、证书主机名不匹配与过期；B 侧执行计数确认不重试 | #322、#336 |
| 场景 S5 两个容器（0.12，nightly） | `netem.rs`：两个 Docker 容器，B 在非 loopback 地址上监听 `wss://`（证书由测试时生成的 CA 签发），A 拨号；netem 200 ms 延迟 + 5% 丢包、双向断网与恢复、换 token；进行中的调用失败一次、B 侧执行计数、心跳超时报告、退避上限内重连、两边 Ctrl-C 后容器内无残留。`netem.yml`，每晚只报告 | #341 → #433 |
| 场景 S7 Rust 嵌入与多租户（0.12） | `embedding.rs` + fixture Rust 应用 `tests/e2e/fixture-tenants`（按 rust-host 指南用 crate 组装，每个租户一个实例子树，含 TS、Python 行）：租户之间同名服务互不可见；管理编辑只影响本租户，校验拒绝和启动失败回滚的编辑都不生效、不写存储；关闭租户后内核、loader、两个运行时进程内都不剩它的东西；重启后从可编辑层恢复，状态一致；反复创建关闭（PR 20 次、nightly 3000 次）进程、数目、描述符、内存平稳 | #191 → #430 |
| 场景 S9 安装 | `install.rs`：npm、PyPI、release 二进制在干净目录安装运行（`install.yml`）；`doc_examples.rs`：发布后用注册表上的包跑指南示例 | #254、#324 |
| 一致性套件的 Windows 与 loopback 列 | 会话一致性、运行时一致性矩阵拆掉整文件 `cfg(unix)`；loopback、WebSocket 列在 Windows 上运行；补了会话契约 × WebSocket、loopback，运行时契约 × `Handover::Loopback`，节点契约 × Rust↔Rust WebSocket。矩阵见 ci.md §4 | #295、#309、#313、#328 |
| 故障注入 | `fault_links.rs`：`fault` 装饰器接到会话与 link（调用中途断开、延迟下取消与释放、重连期间的调用、多跳、交错返回、大载荷与深嵌套）；`concurrent_launch.rs`（并发启动） | #285、#215 |
| 属性测试 | `value_roundtrip.rs`（proptest）：Rust 侧生成值，经 Node、Python、Go、Bun 往返比较；嵌套深度上限；release 计数不变式 | #296 |
| 协议层其他 | `boundary_rules.rs`（边界规则 3、4、5、7）；`sdk_protocol.rs` 与两个 SDK 的协议版本 99 测试；Python 的 release 计数、grant 回滚；`websocket_cross.rs` 的 Python 列；Rust、Python 的负向 TLS | #283、#288、#297 |
| 内核 | `event_order.rs`（D24、D31、D29、异步 waterfall panic）；`lifecycle_order.rs`（D7、丢弃 restart / update 的 future）；`typed_plugin.rs`、`instance_subtrees.rs` 补充 | #280、#286、#345 |
| `rutis-host` | `tests/` 下 8 个文件：信号、配置、`check`、常见错误输出、项目模板、持久化样本、link 状态、输出整行 | #216、#217、#284、#336、#339 |
| 测试工具与真实运行时 | 同一份期望文件 `weather.expected.json`：真实运行时经 `session_matrix.rs`，Node、Python 测试工具各一个测试 | #298 |
| 持久化样本 | `tests/fixtures/persisted/`：0.8.0、0.9.0、0.9.1、`rejected/` | #284 |
| TLA+ | `specs/` 的 7 个模型目录、17 个模块、220 个配置（§7.2）。CI 的 `specs` 任务在改了 `specs/**` 的 PR 和每晚运行 | #222、#210，之后 #613 |
| 保证登记表 | [guarantees.md](guarantees.md)：`core/`、`boundary/`、`instance/`、`bridge/`、`host/`、`dsh/`、`sdk/` 族；代码里 1232 处 `guarantee: <编号>` 标记（`tools/check-guarantees.mjs` 的计数）；规则追踪表 1825 行（§7.6） | #276、#310 → #634 |

CI：`ci.yml`（PR 按改动选择任务，每晚全部运行；Linux 全量与最低版本、E2E、lint、macOS 与 Windows 的平台相关测试、Bun 与 Go 运行时、dylib、打包、semver、MSRV、feature 组合、可复现、7 个模型的 `specs` 任务）；`install.yml`（安装冒烟）；`nightly.yml`（每晚调用前面三个和 `stress.yml`，失败时开问题单）；`stress.yml`（每晚：内核多轮、多进程测试多轮、两个浸泡、`rutis` 与 `rutis-bridge` 的 57 个 shuttle 模型、9 个模糊测试目标、事件的全部放置）；`dylib-windows.yml`；`deps.yml`、`ci-stats.yml`（每周）。详见 [ci.md](ci.md)。CI 中仍然没有：actionlint、许可证检查。

## 3. 控制手段代号

§4 风险表中的控制手段用以下代号，选用原则见规范 §11。

| 代号 | 手段 | 能发现 | 发现不了 | 代价 |
| --- | --- | --- | --- | --- |
| **U** | 单元测试 | 单个函数或模块的逻辑错误 | 组件之间、进程之间的问题 | 低 |
| **K** | 契约与一致性套件（现有的 channel / session / runtime / node 四套，内核 contract / parity） | 同一份规范在不同实现、不同通道上的偏差 | 规范本身的错误；套件没写到的行为 | 中（套件写一次，各实现复用） |
| **X** | 差分测试：以参照实现为准对比输出（原生 Cordis、cordis-plugin-include、dsh） | 与参照行为的偏差，包括没人想到要测的 | 参照实现本身的错误；没有参照的部分 | 中 |
| **E** | 黑盒端到端：启动真实的 `rutis-host` 或安装好的包，用配置文件和 probe 插件驱动 | 装配、二进制行为、信号、退出码、多进程交互 | 难以构造的时序；内部状态 | 高（需要框架） |
| **R** | 残余检查：场景结束时核对进程、socket、端口、fd、线程、任务、注册表 | 资源泄漏和孤儿进程 | 泄漏的原因 | 低（框架写一次，处处复用） |
| **FI** | 故障注入：`fault` 通道装饰器、kill 进程、写坏配置、占用端口 | 故障下的行为是否符合承诺 | 没有列入注入清单的故障 | 中 |
| **FZ** | 模糊测试（cargo-fuzz） | 解析外部输入时的崩溃、无限内存、死循环 | 逻辑错误（结果不对但不崩溃） | 中（目标写一次，nightly 运行） |
| **SH** | 可控调度：接管调度，系统地尝试不同执行顺序，失败时给出具体顺序。Rust 用 shuttle、Kotlin 用 Fray；Python 用可步进的 `SelectorEventLoop`；Node / Bun 用可注入的帧泵。Go、Swift 没有可控调度 | 一个进程内多个任务 / 线程的竞态、丢失唤醒、死锁 | 大范围交互；没被纳入探索的外部事件（真实进程、操作系统、运行时自身）；需要把代码抽成小模型 | 中 |
| **TLA** | TLA+ 设计模型检查 | 协议设计本身的死锁、违反不变式 | 实现与设计不符 | 高（需要学习，spec 需要随设计维护） |
| **TV** | trace validation：把真实运行的日志当作 trace，逐事件检查它是模型允许的一个行为 | 实现是否照模型的规则做 | 没跑到的交错；模型本身写错 | 中（要先在实现侧有记录点） |
| **PT** | 属性测试（随机生成输入，检查性质） | 值往返、计数不变式、匹配规则这类有明确性质的逻辑 | 没有明确性质的行为 | 低到中 |
| **KANI** | 有界模型检查（Kani）：在给定的输入范围内穷举极小的纯函数或 `unsafe` 函数 | 输入空间小到能穷举的函数 | 有分配、格式化或依赖运行时的代码（这台机器上 Kani 0.68.0 不收敛）；并发 | 中（要装工具链，harness 必须很小） |
| **SK** | 浸泡：长时间重复操作，采样资源曲线 | 缓慢增长的泄漏 | 快速出现的错误 | 中（占用 CI 时间） |
| **MX** | 版本与平台矩阵：在声明支持的每个 OS、语言版本、依赖下限上运行 | 只在某个版本或平台出现的问题 | — | 中（CI 时间） |
| **IN** | 安装冒烟：在干净目录里安装本次构建的包，走最小使用路径 | 打包、分发、平台包选择的错误 | 功能逻辑 | 中 |
| **G** | 输出快照：错误信息、状态输出、`check` 输出与保存的期望文本对比 | 错误信息变差、输出格式变化 | 信息是否真的有用（需人工评审） | 低 |
| **ST** | 静态检查：fmt、clippy、`cargo deny`、feature 组合编译 | 常见编码错误、已知漏洞依赖、条件编译遗漏 | 逻辑错误 | 低 |
| **SV** | API 兼容检查：`cargo semver-checks`；迁移指南的代码编译运行 | 未声明的破坏性变更；迁移指南过期 | 行为上的破坏 | 低 |
| **DOC** | 文档示例运行 | 文档和实际不一致 | — | 中 |
| **DR** | 设计评审与清单：设计稿的验收节、PR 模板检查项 | 设计遗漏；改动没有配套的测试或文档 | 依赖人 | 低 |
| **BM** | 基准测试 | 性能变化 | CI 机器噪声下的小幅变化 | 中 |

---

## 4. 风险分析

评级口径见规范 §4（影响、可能性、P0 / P1 / P2）。"现状"一列：§4.2 的场景表与 §4.1 前两张表是 2026-10-12 的状态；§4.1 的第三张表（事件、行与调用的生命周期）按 2026-11-15 的当前树核对。

### 4.1 基础层：内核与协议

所有场景都依赖这一层，单独分析。

#### 内核 `rutis`

| # | 可能出的问题 | 维度 | 影响 | 可能性 | 级 | 控制 | 阶段 | 现状 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| K1 | 生命周期规则不成立：清理执行零次或两次、依赖未齐就启动、驱逐顺序错 | 语义 | 高 | 中 | P0 | K（contract 65 条、parity 57 条）、U | PR | 有 |
| K2 | 并发交错下的竞态：丢失唤醒、驱动退出竞态、尾链分叉 | 语义 | 高 | 中（历史上发生过 3 次） | P0 | SH（28 个模型跑内核自己的代码，不是副本）；多线程重复运行带种子 | PR（1000 次迭代）、nightly（每个模型 100 万次） | 有：`interleave.rs` 带种子、`stress.yml` 每轮换线程数与种子（#220）；`fiber/shuttle_tests.rs` 28 个模型覆盖 TransitionTask 的每次 join、`post_join` 退出竞态、并发 dispose、代次 marker 与 closing 的发布顺序、effect 登记与清理、在途监听器调用期间的卸载、并发子树关闭、root dispose 与 shutdown、依赖撤下、emit 链顺序、监听器表观察者、卸载取消当前代、重启发布新代次、root 关闭时它挂载的子节点、dispose 结果跨驱动退出（#1194）、依赖窗口里的重启、spawn 的任务、root 关闭与子节点挂载的准入、两个 fiber 调用的截止时间、重启输给 dispose、`Provided` 先于 `Removed`（#179 → #425、#622、#628）、三个 fiber 同时重启时 `FiberStatusChanged` 的投递顺序与锁外投递（#622）、挂载插件的清理 effect 与服务经真实的逆序 pass（#622）、方法登记过的 `Ctx` 所在行被卸载时另一线程同时登记（#622）。模型照代码建之后确认了 #619、#620 两个内核缺陷（已修），#1194 由 #1196 修好，模型 18 钉住它。缺：dylib 没有模型；会话层、事件宿主、运行时监督与宿主重载的模型在 §7.3 的另外三行，其中 `rutis-host`、`rutis-loader` 的还不在任何 workflow 里跑；尾链分叉（D31）仍只有 `dispatch_chain_probe.rs` |
| K3 | 事件顺序保证只成立一半：多个发射者各自的顺序、panic 后下一次仍投递 | 语义 | 中 | 中 | P1 | K | PR | 有（`event_order.rs`，#280） |
| K4 | 写明"不保证"的行为被悄悄改变（D29 事件不按 isolate 过滤、D22 不检测依赖环） | 语义 | 中 | 低 | P2 | U（回归测试锁定当前行为） | PR | 部分：D29 有（#280）；D22 没有 |
| K5 | 长寿 root 上反复创建销毁，内部表、任务数增长 | 残留 | 高 | 中 | P0 | U + R（断言任务数、表大小回到基线）；SK | PR、nightly | 部分：U + R 有（`instance_subtrees.rs` 任务数与容量，#286）；嵌入应用里反复创建关闭租户有 E（S7，nightly 3000 次，#191）；宿主级 SK 没有（#192） |
| K6 | 用户回调 panic 越过边界，击穿驱动任务 | 故障 | 高 | 低 | P1 | U | PR | 有（异步 waterfall 的 panic 传播，#280） |
| K7 | 没有 tokio runtime 或用 current_thread runtime 时行为不对 | 兼容 | 中 | 中 | P1 | U（D8）；契约测试单线程和多线程两种 flavor 都跑 | PR | 部分：D8 有测试（#286）；契约测试大多只用单线程 runtime |
| K8 | 丢弃等待中的 future 后，已开始的清理被中断 | 语义 | 高 | 低 | P1 | U | PR | 有（`lifecycle_order.rs`，#286） |
| K9 | `diagnostics()` 报告与实际状态不符 | 诊断 | 中 | 中 | P1 | U | PR | 有（`lifecycle_diagnostics.rs`） |
| K10 | 测试依赖机器速度，CI 负载高时误报，或机器快时什么都没测到 | — | 中 | 高 | P0 | 确定性规则（规范 Q7.1） | PR | 有（#220、#231） |

#### 通道与会话协议

| # | 可能出的问题 | 维度 | 影响 | 可能性 | 级 | 控制 | 阶段 | 现状 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| P1 | 通道违反契约：丢消息、乱序、背压失效、关闭不唤醒 | 语义 | 高 | 低 | P1 | K（通道契约 6 项） | PR | Rust 通道有；Node、Python 通道没跑契约 |
| P2 | 畸形帧、超长行、无换行持续写入导致崩溃或内存无限增长 | 故障、安全 | 高 | 中 | P0 | U（边界值）；FZ（帧解码、行分帧） | PR、nightly | 有：U（WebSocket 与本地通道都有上限，#214）；FZ 的 `session_frame`、`local_lines`、`error_graph` 在 `stress.yml` 每晚跑（#179 → #425、#624） |
| P3 | 未知 op、字段类型错、引用 id 不存在时，会话挂起或静默忽略 | 故障 | 中 | 中 | P1 | U（七种实现各自） | PR | 有：一致性套件的 `malformed` 检查，25 个用例 × 七种实现（#214；Go #305） |
| P4 | 引用计数错误：释放过早（对端调用失败）或永不释放（泄漏） | 语义、残留 | 高 | 中 | P0 | K；PT（任意 grant / release 序列后表大小 = 未释放数） | PR | 有（Python #288；PT #296；`grant_rollback.rs` 在值不可跨、能力缺失、超限、结果不可跨时都不授予）。#621 修的是这条路径上的死锁：解码遇到引用类型不匹配时持着 `imports` 锁丢掉 `Import`，析构在同一线程再锁一次 |
| P5 | 跨会话转发时调用链改写错误，反向调用送错运行时或死锁 | 语义 | 高 | 中 | P0 | K（`rpc_callbacks.rs` 两边同号调用） | PR | 有 |
| P6 | 取消没有传到对端，或多跳时中途丢失 | 语义 | 中 | 中 | P1 | K（`abortable`）；多跳取消新增一个测试 | PR | 有（`fault_links.rs`，#285） |
| P7 | 值在三种语言间转换不一致：`undefined` / `null` / 缺省、大整数、dataclass、嵌套深度 | 语义 | 高 | 高 | P0 | PT（同一批随机值在 Rust↔Node、Rust↔Python 间往返，比较结果）；K | PR | 有（`value_roundtrip.rs`，含 Go、Bun，#296；修复 #312、#311、#338）；只在 Unix 上运行 |
| P8 | 协议版本或能力不匹配时挂起，或加载后才出错 | 兼容、诊断 | 中 | 高（运行时和宿主独立升级） | P1 | U（三种语言各自）；MX（上一版运行时 vs 当前宿主） | PR（U）；每晚、发布门（MX） | 有：U（#288，信息说明升级哪一端 #335）；MX（#209 → #426：上一版运行时、Go 二进制、宿主、节点与当前代码，能力不足时在加载时拒绝并说明升级哪一端，新值只让这次调用失败） |
| P9 | 同步调用期间对端反向调用：Node 应返回 `SyncWaitCycle`，Python 应执行；行为与文档不符 | 语义 | 中 | 中 | P1 | K（`reenter`） | PR | 有 |
| P10 | 跨运行时同步调用的死锁策略本身有漏洞（两个不可重入运行时交叉调用） | 语义 | 高 | 中 | P0 | TLA（设计阶段）；之后 K 加反例测试 | 设计 | 有：TLA+ 选定策略 B（#222），已实现并有测试（`rpc_callbacks.rs`、`sync_cycles.rs`，#261）。两个已知限制：未声明可重入的运行时按不可重入处理；未声明 `sync-wait` 的运行时不在检测范围内 |
| P11 | 后台线程或 worker 里的异常被吞掉，测试照样通过 | — | 高 | 高（#197 已发生） | P0 | 测试规则：后台异常必须让测试失败（规范 Q7.4） | PR | 部分：Python 有（#218）；Rust、Node 没有统一的机制 |

#### 事件、行与调用的生命周期

这一组是 0.13–0.18 加进来的部件与协议（统一事件、节点连接、应用托管、二进制运行时监督、标记的服务），0.22 之前 §4 没有它们；编号与控制方式来自 #612，现状按当前树核对。

| # | 可能出的问题 | 维度 | 影响 | 可能性 | 级 | 控制 | 阶段 | 现状 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| EV1 | 登记与分发之间的保证（事件协议规格 §4.4）在快速路径和经宿主两条路径上不一致 | 语义 | 高 | 中 | P0 | TLA（`specs/events/`）；K（`events.expected.json`） | PR、设计 | 部分：`EventsTable.tla` / `EventsHost.tla` 检查两条路径上"先登记后发出"，`events.expected.json` 由 `events_conformance` 在七种语言逐条跑（`stress.yml` 的 `events-all` 每晚按采样放置，周一全跑）。缺：快速路径上的过滤器、五种分发的完整语义只有 `emit` 建模 |
| EV2 | `emit` 的顺序在路径切换、过滤器等待、不可重入运行时扣住调用时被打破（已发生：#513、#527、#557、#519、#554） | 语义 | 高 | 中 | P0 | TLA；K；nightly `events-all`；TV（#624） | PR、nightly | 部分：`EventsEmit.tla` 的两个顺序性质通过，卸载丢弃排队 emit 有配置；`emit-python-deferred` 通过（`Defer = "nextRound"` 只把开始推到下一轮，模型本来就允许任意晚）；`before-future-reply` 以变异 `emit-mut-future-reply` 建（宿主对还在排队、调用尚未开始的 emit 先回复），违反 `EmitOrder`；`events.expected.json` 与 nightly 的 `events-all` 有。缺：Node 按 key 扣住的配置；事件宿主的 trace validation 未做 |
| EV3 | 卸载或断开之后，已撤下的条目仍开始调用，或进行中的调用失败零次、两次；已卸载的行扣住队列（已发生：#536（#564 修复）、#587） | 语义、残留 | 高 | 中 | P0 | TLA；K；TV（#624） | PR | 部分：`EventsTable.tla` 的"只接受这一行当前登记"与 `table-reload`/`table-end-session` 配置；实现侧 `event_hub.rs` 的 `rows.ended` 按帧序（#587）、卸载丢弃排队 emit（#536）。缺：trace validation |
| EV4 | 节点连接的提供与宣告：旧提供的条目残留，先于提供到达的宣告被撤下（已发生：#593；#435 的根因在宿主内的登记，由 `peer.rs` 的 `a_family_is_announced_only_with_its_offer` 覆盖） | 语义 | 中 | 中 | P1 | TLA（`EventsLink.tla`）；K（`crates/rutis-bridge/src/peer.rs`、`events.expected.json` 的 `node/` 用例、`node/rutis-runtime/test/cordis-events.test.mjs`） | PR | 部分：`EventsLink.tla` 检查只保留最高版本、旧提供按 epoch 撤下、先到的宣告保留；`events.expected.json` 有 5 个 `node/` 用例。缺：多对端与多跳不在模型里 |
| EV5 | 同步分发（`bail`、`waterfall-sync`）、宿主自己的同步调用、每个调用一个线程的运行时（Go、Swift、Kotlin）让等待环漏检或误报（已发生：#537） | 语义 | 高 | 中 | P0 | TLA（`specs/cross-runtime-sync/`）；K（`crates/rutis-loader/tests/sync_cycles.rs`）；TV（#624） | PR | 部分：`cross-runtime-sync` 44 个配置含 `Threaded`（每调用一线程）与宿主发起的 `host-call`，`sync_cycles.rs` 有 Node / Python / Go / Swift 列；`cycle_among` 的差分属性测试（`waits.rs` 的 `cycle_among_agrees_with_the_models_graph` 与 `the_model_check_agrees_on_the_small_graphs`）与等待图 trace validation（`waits/trace.rs` 记录、`waits/trace/check.rs` 按 `RustSucc` 重放，在 `sync_cycles.rs`、`rpc_callbacks.rs` 上跑）已做。缺：`sync_cycles` 没有 Kotlin 列（Kotlin 要 JDK 17 与 `:rutis-conformance:fatJar`，#624） |
| RL1 | 宿主与运行时之间的行生命周期：迟到的 `rows.ended`、拒绝、服务公告作用到别的代次；租约比行活得长（已发生：a88fb65b、54a7e824、d94aae9a、#247、#314、#372、#584、#191） | 语义、残留 | 高 | 中 | P1 | TLA（`specs/row-lifecycle/`）；K（各语言的行契约，`crates/rutis-loader/tests/*_rows.rs`） | PR | 部分：`RowLifecycle.tla` 36 个配置检查"一行最多一个代次在运行、迟到结束只结束自己的代次、租约不长于行、服务在提供方卸载前撤下"；各语言行契约有 Node / Python / Bun / Go / Swift。缺：`rows.schema` / `mount`、调用链、各语言卸载栈差异、shutdown 截止不在模型里；#1100 已定案（键每次装载新取）、#1101（卸载先于装载）已修（#1185），两个 issue 都已关闭 |
| CL1 | 调用生命周期：取消之后被调用方可能回也可能不回（已成文，`SESS-61`），调用方的取消表只增不减；读线程写回在两端背压时可能互相卡死 | 语义、残留 | 高 | 中 | P1 | TLA（`specs/call-lifecycle/`）；K（`rpc_callbacks.rs`、`multihop.rs`） | PR | 部分：`CallLifecycle.tla` / `CallRelay.tla` 35 个配置把两种取消答复实现都验证为"不重复结算、不漏引用"，并证明读线程写回不死锁的前提（读线程不直接写）、中继逐跳传取消。缺：调用方的取消表可能只增不减，清理与取消确认未定（#1099）；中继的关闭、关闭后对侧继续读帧不在模型里 |
| AH1 | 应用托管：`RutisApp` 的开始、停止不按调用顺序生效，停止后端口没有释放，再开始时拿到上一轮的状态（已发生：#519、#521、#532、#554） | 语义、残留 | 高 | 中 | P1 | SH：Kotlin 用 Fray（#630）；Swift 没有可控调度，用严格并发检查加 TSan（#630）；连接层用 TLA（`remote-lease`）；模拟器与设备上的测试在本地运行（[CI](ci.md) §12） | PR、设计 | 部分：`remote-lease` 的 `lease-app-restart` / `access-app-restart` 配置按"停止后再开始"建，`RutisApp` 的线程交错已在 f57b4138 修复；Kotlin 的 Fray 用例（§7.3）没有覆盖 `RutisApp` 的起停；Swift 的 TSan 任务跑 AppTests（§7.3）；iOS / Android 的应用托管测试只在本地跑 |
| AH2 | Swift / Kotlin 的接管顺序、再拨规则与其他语言不一致 | 语义 | 中 | 中 | P1 | TLA 的对应表核对；K（#624） | PR | 部分：`specs/remote-lease/README.md` 的对应表已补 Swift / Kotlin；Kotlin 心跳起算点已修（#1181），模型按正确规则建，修复前的实现留作反例 `lease-mut-heartbeat-from-accept`。`sync_cycles` 没有 Kotlin 列（#624） |
| RS1 | 二进制运行时（Go、Swift）的监督与宿主的重载、本地运行时监督：监督开始前的结束无界地留着，重启时旧进程没结束就启动新的，缺失的二进制每轮都重试，`dev` 重建失败时丢掉旧的构建，被移除的运行时等错了进程（已发生：#524、#563、#565、#566、57f536a0） | 语义、残留 | 中 | 中 | P1 | SH（抽出 Launcher 接缝之后用 shuttle，#622）；U（测试时钟） | PR | 部分：`crates/rutis-loader/tests/virtual_clock_contract.rs` 与 [测试时钟设计](design-test-clocks-2026-10-11.md) 覆盖计时规则；二进制运行时的重启、停止与安装由 `rutis-loader/src/runtime/binary/shuttle_tests.rs` 的 3 个模型检查，宿主等运行时自己的进程由 `rutis-host/src/restart/shuttle_tests.rs` 与 `rutis-host/src/host_shuttle_tests.rs` 的 3 个模型检查（#627，§7.3），这两个 crate 的模型不在任何 workflow 里跑。缺：`dev` 的重建、巡检与 dylib 仍只靠真实 Go / Swift 进程测 |
| SV1 | 标记的服务：同一版本重建的 dylib 仍由旧构建提供，按名字调用时按错误的行选拦截器（已发生：#522、#531、#534） | 语义 | 中 | 低 | P1 | U；E | PR | 有：`crates/rutis-bridge/tests/marked_services.rs`、`crates/rutis-loader/tests/marked_services.rs` 覆盖标记服务的代次与拦截器选择；三个缺陷已修。缺：E2E 里没有专门的标记服务场景 |

---

### 4.2 场景

每个场景先说明它是什么、经过哪些组件、在什么条件下运行，再列风险。

#### 场景 A：插件作者的开发循环

**内容**：`rutis-host new` 创建项目 → 用 SDK 测试工具写单元测试 → `rutis-host dev` 在本地宿主里运行，改代码自动重新加载 → `check` → 发布到 npm / PyPI → 被宿主使用。TS 和 Python 两条线。

**经过**：`rutis-host`（new / dev / check）、模板、SDK 和测试工具、运行时、loader 的重新加载。

**条件**：三个平台；Node 与 Python 的最低和最新版本；一次开发会话里重新加载数百次；代码经常处在写错的状态。

| # | 可能出的问题 | 维度 | 影响 | 可能性 | 级 | 控制 | 阶段 | 现状 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| A1 | 测试工具和真实运行时行为不一致：在 `load()` 里通过的插件，到宿主里失败（或反过来） | 语义 | 高 | 中 | P0 | X：把运行时一致性 fixture（`conformance-weather`）同时用测试工具和真实运行时运行，结果比对 | PR | 部分：Node、Python 有（同一份期望文件，#298）；Go `rutistest`、Bun 没有 |
| A2 | 模板生成的项目不能直接安装、测试、运行 | 兼容、文档 | 中 | 高（依赖和工具链变化快） | P1 | E（S2 第 1–2 步）；MX | 合并 | 有（S2，#253） |
| A3 | 重新加载后旧代码仍在运行，或旧实例没有卸载 | 语义、残留 | 高 | 中 | P0 | E + R | PR | 有（S2，#253） |
| A4 | 写入语法错误后 `dev` 崩溃，或旧版本停止服务 | 故障 | 中 | 高 | P1 | E（写坏文件，断言旧版本继续服务、输出 `cannot reload`） | PR | 有（S2，#253） |
| A5 | 反复重新加载数百次后资源增长 | 残留 | 中 | 中 | P1 | E + R，循环 N 次 | nightly | 没有 |
| A6 | `check` 误判：能运行的报错，或不能运行的通过 | 诊断 | 中 | 中 | P1 | G（一组正例和负例项目，对比输出和退出码） | PR | 有（`rutis-host/tests/check.rs`，#217；#258） |
| A7 | 错误信息看不出是哪个文件、哪一行、哪个服务 | 诊断 | 中 | 高 | P1 | G（常见错误的输出快照）；DR | PR | 有（`rutis-host/tests/errors.rs`，#217） |
| A8 | Windows 上路径、venv（`Scripts\python.exe`）、换行解析错 | 兼容 | 中 | 中 | P1 | E 在 Windows 上运行 | PR | 有（S2 在 Windows，#323） |
| A9 | 插件 API 版本不匹配时，错误信息不说明要升级什么 | 兼容、诊断 | 中 | 低 | P2 | U（两个 SDK） | PR | 有 |
| A10 | 方法形状声明和实现不一致（声明 sync 却返回 Promise），测试工具没发现 | 语义 | 中 | 中 | P1 | U（测试工具严格模式） | PR | 有 |
| A11 | Python 只重新导入入口模块（已知限制），用户以为改了其他模块也会生效 | 文档 | 低 | 高 | P2 | 文档说明；`dev` 在检测到其他模块变化时给出提示 | — | 文档有 |
| A12 | 模板里的发布工作流不能用 | 文档 | 低 | 中 | P2 | ST（actionlint 检查 workflow 语法） | PR | 没有 |

#### 场景 B：运维用 rutis-host 部署运行

**内容**：从 npm / PyPI / 二进制 / crates.io 安装 → 写 `rutis.json` → `rutis-host run` 由 systemd 等进程管理器管理 → 运行中修改配置 → 停止、重启、升级。

**经过**：分发包、`rutis-host` 的配置解析和装配、loader、运行时、节点。

**条件**：三个平台；可能被 SIGTERM、SIGKILL 结束；配置可能写错；运行数周。

| # | 可能出的问题 | 维度 | 影响 | 可能性 | 级 | 控制 | 阶段 | 现状 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| B1 | 某个安装渠道的包坏了：平台包没被选中、wheel 缺文件、二进制在干净机器上跑不起来 | 兼容 | 高 | 中 | P0 | IN（三个平台 × npm / PyPI / 二进制，本次构建的包）；发布后用注册表上的包再跑一次 | 合并、发布后 | 有（S9，#254、#324）；Python 指南示例的标记等 #259（#266） |
| B2 | 收到 SIGINT / SIGTERM（Windows：Ctrl-C、Ctrl-Break、关闭控制台）时不执行清理；退出码不符合文档 | 残留 | 高 | 高（#174 已发生） | P0 | E + R（每种信号轮换） | PR | 部分：Unix 的 SIGINT、SIGTERM 有（#216）；Windows 的 Ctrl-Break 有（#277、#324）；Windows 的 Ctrl-C、关闭控制台没有 |
| B3 | 宿主被 SIGKILL 后，运行时子进程成为孤儿 | 残留 | 高 | 中 | P0 | E + R（Unix 靠通道断开，Windows 靠 Job Object） | PR | 有：三个平台（`cross_language.rs`）；Unix 上事件循环不空、清理不返回的运行时也在截止时间内退出（#353、#354，`stray_runtime.rs`、`host_gone.rs`） |
| B4 | 配置改坏后，旧的行被卸载、新的起不来 | 故障 | 高 | 中 | P0 | E（写入错误配置，断言旧版本继续服务）；loader 库级 K 已有 | PR | loader 有；`run` 运行中重新读配置（SIGUSR1、`--watch-config`，#274）全有或全无，E 见 `host/reload-config` |
| B5 | `rutis.json` 解析问题：未知字段被静默忽略、相对路径基准错误、凭据环境变量优先级错误 | 语义、诊断 | 中 | 中 | P1 | U（每个字段）；FZ（配置文件，低优先级） | PR | 有（U，`rutis-host/tests/config.rs`，#217；#226）；FZ 没有 |
| B6 | 运行时包缺失、解释器找不到、端口被占、证书文件错误时，错误信息不足 | 诊断 | 中 | 高 | P1 | G（每种错误一个快照） | PR | 有（`errors.rs`，#217） |
| B7 | 凭据出现在日志、错误信息、状态输出、trace 里 | 安全 | 高 | 低 | P1 | E 的公共断言：所有捕获输出中查不到 token 值 | PR | 有（E2E 残余检查，#221、#277；S5 #322） |
| B8 | 无 TLS 的监听绑定到非回环地址 | 安全 | 高 | 低 | P1 | U | PR | 需核对 |
| B9 | 状态输出格式变化，破坏运维脚本 | 兼容 | 低 | 中 | P2 | G | PR | 部分：连接行的状态与监听地址的行有测试（`rutis-host/src/connect/dial.rs`、`remote_runtimes.rs`，#336） |
| B10 | 进程管理器重启宿主时，上一次的 socket 文件或端口没有释放，新进程起不来 | 残留 | 中 | 中 | P1 | E（kill -9 后立即重启） | PR | 部分：Ctrl-C 停止后用同一份配置立即重启、监听端口能再次绑定（S3，#317）；宿主被 `kill -9` 后立即重启没有验证（残余检查只确认端口可以再绑定） |

#### 场景 C：跨语言组合

**内容**：同一个宿主里，Rust、TS、Python 插件互相提供和使用服务。设计哲学 §二："多语言插件不是附加功能，而是它存在的理由"。

**经过**：内核门控、loader 行、两段放行（`SessionRows`）、运行时进程、会话转发、服务投影。

**条件**：多个运行时同时冷启动；运行时进程可能崩溃；同步和异步调用交织；三个平台（Windows 走 loopback TCP）。

| # | 可能出的问题 | 维度 | 影响 | 可能性 | 级 | 控制 | 阶段 | 现状 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| C1 | 多个运行时冷启动时互相等待 | 语义 | 高 | 低 | P1 | K（M1/M2 交界一致性测试） | PR | 有 |
| C2 | 行在依赖声明完整之前启动（两段放行失效） | 语义 | 高 | 中 | P0 | K（`runtime_rows.rs` 先解析后启动） | PR | 有 |
| C3 | 一个运行时崩溃影响其他运行时，或不相关的行也停下 | 故障 | 高 | 中 | P0 | K（M2 §三.4）；E（kill -9，断言其他运行时 pid 不变） | PR | 有（S3，#317；Windows 不含 Bun 行） |
| C4 | 运行时崩溃时，进行中的调用挂起而不是显式失败 | 故障 | 高 | 中 | P0 | FI（一致性 fixture 的 `crash()` 在调用进行中触发） | PR | 有（S3 #317；`fault_links.rs` #285） |
| C5 | 跨语言值转换不一致（见 P7） | 语义 | 高 | 高 | P0 | PT | PR | 有（#296） |
| C6 | 两个 Node 运行时交叉同步调用卡死（已知限制）；接入第三种语言后限制扩大 | 语义 | 高 | 中 | P0 | TLA（选定策略）；文档；选定策略后 K 增加反例 | 设计 | 有：策略 B，宿主检测等待环并返回 `SyncWaitCycle`（#222、#261）；限制见 P10 |
| C7 | Python 运行时可重入，插件的服务在自己的同步调用中途被调用，状态被并发修改 | 语义 | 中 | 中 | P1 | 文档；测试工具在严格模式下模拟重入（可选） | — | 文档有 |
| C8 | 撤销顺序错误：提供者先于使用者停下 | 语义 | 高 | 低 | P1 | K | PR | 有 |
| C9 | 同一运行时内直接拿对象、跨运行时拿代理，两者行为差异超出文档说明 | 语义 | 中 | 中 | P1 | K（`multilang.rs`） | PR | 有 |
| C10 | Windows（loopback TCP + token 交接）下以上任何一条不成立 | 兼容 | 高 | 中 | P0 | MX：一致性套件的 loopback、WebSocket 两列在 Windows 运行 | PR | 有（#295、#309；矩阵见 ci.md §4；Bun 不支持 Windows） |
| C11 | 同一进程内并发启动多个运行时时偶发失败 | 故障 | 中 | 高（#184 本机约 50%） | P0 | FI：N 个线程同时启动，重复 100 轮 | PR | 有（`concurrent_launch.rs`，#215） |
| C12 | 运行时进程大量写 stdout / stderr，管道填满导致阻塞 | 故障 | 中 | 低 | P2 | FI（fixture 持续输出） | nightly | 没有 |

#### 场景 D：Rust 应用嵌入与多租户实例

**内容**：Rust 应用用 crate 组装宿主，提供自己的 Rust 服务；按业务对象（租户、会话、文档）创建实例子树，由一个 loader 管理全局行和实例行；配置持久化，重启后恢复。

**经过**：内核实例子树、loader 实例、持久化、`Persist` 实现、运行时内的实例作用域、节点里的实例。

**条件**：长寿进程；实例数十到数千，反复创建关闭；嵌入方的 tokio 配置各异；嵌入方会升级 crate 版本。

| # | 可能出的问题 | 维度 | 影响 | 可能性 | 级 | 控制 | 阶段 | 现状 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| D1 | 实例之间服务可见性泄漏：租户 A 读到租户 B 的服务 | 安全、语义 | 高 | 低 | P1 | K（`instances.rs`、`instance_services.rs`、`instance_runtimes.rs`） | PR | 有 |
| D2 | 关闭实例后有残留：内核注册、运行时内的注册、导出槽、代理 | 残留 | 高 | 中 | P0 | U + R（运行时内注册也要检查，不只看 diagnostics） | PR | 有：`instance_runtimes.rs`；S7 关闭租户后，实例下的插件、带实例键的绑定、loader 条目，以及 Node、Python 运行时进程内的行、服务、宿主代理、导出槽、对象句柄、Cordis 插件与 fiber 都回到没有它时的数目，它的 id 一个不剩（运行时的测试控制方法 `inspect`，#191） |
| D3 | 反复创建、关闭实例数千次，任务数、内存、内部表容量增长 | 残留 | 高 | 中 | P0 | SK（1000 轮，断言任务数和表容量回到基线） | nightly | 有：instance-subtrees §5 的任务数与容量有 U（#286）；S7 在常驻租户旁反复创建关闭租户（PR 20 次、nightly 3000 次）：进程不变，内核、loader、运行时内的数目回到基线，描述符最多 +8，内存有阈值（应用 1 MiB + 每次 256 B）。由此发现并修复：每加载一个语言行，应用里留下一个等运行时会话结束的任务，宿主侧导出槽表也只增不减（每次约 1.4 KiB，#191）；`instance_runtimes.rs` 断言任务数回到基线。实例资源基线（`close_memory`）在 PR 上以 debug、每晚 `stress.yml` 的 `memory` 以 release 跑：一个 8 行实例从 62.0–72.7 KiB 降到 49.6–60.3 KiB（每行监视任务的 future 从 1,872 B 降到 344 B），关闭 40–1000 个后残留 18–58 KiB（阈值每实例 Linux 61.5 KiB、macOS 78 KiB，关完后 128 KiB + 128 B/实例；另有 `the_monitor_future_stays_small` 断言监视 future ≤ 512 B），fiber、服务登记、监听者、任务数、Linux 上的 OS 线程数回到基线；组成、可省项、前后对比与不覆盖范围见[实例常驻占用](performance-instance-footprint-2026-11-02.md)。由此发现并修复：loader 的 `running`、`groups`、`instances` 等表和跟随表在关闭实例后保留峰值容量（1000 个实例约 6 MB，现约 22 KB，#979） |
| D4 | 一次编辑影响多个实例，部分成功部分失败 | 语义 | 高 | 低 | P1 | K | PR | 有 |
| D5 | 持久化后重启，状态和重启前不一致 | 语义 | 高 | 中 | P0 | K（loader §十七 的重启一致性）；E（S7） | PR | 部分：E 有，S7 重启应用后只凭可编辑层重新打开租户，配置、行状态、各语言服务的回答与重启前一致，启动不改写存储（#191）；§十七 仍需逐条核对 |
| D6 | 多个进程写同一份存储，修改丢失 | 语义 | 高 | 中（dsh 的 CLI 和 Web 同时开着） | P0 | K（loader 多写者 CAS、重放丢弃）；PT（随机修改序列 + 模拟冲突，最终存储 = 全部修改） | PR | 需核对 |
| D7 | 公开 API 发生未声明的破坏性变更 | 兼容 | 中 | 高（0.x） | P1 | SV（semver-checks 已有）；迁移指南代码编译 | PR | 部分 |
| D8 | 嵌入方在 current_thread runtime 或阻塞线程里调用同步方法时卡死 | 故障 | 中 | 中 | P1 | K（网络栈设计要求的"调用方 current_thread 被阻塞时通道仍推进"） | PR | 需核对 |
| D9 | 只开部分 feature 时编译失败 | 兼容 | 中 | 中 | P1 | ST（feature 组合，每晚的 `checks` 任务） | PR | 有 |
| D10 | 关闭（`shutdown`）在截止时间内没完成，嵌入方无法判断状态 | 诊断 | 中 | 低 | P2 | U | PR | 有 |
| D11 | 实例数到上限时延迟异常增长 | 性能 | 中 | 低 | P2 | BM（1000 实例下创建、关闭、读服务） | 每周 | 没有 |

#### 场景 E：多机节点与远程运行时

**内容**：节点之间用 WebSocket + TLS 连接，导入导出服务、代为运行插件、转发事件；GPU 机器上只运行一个 Python 运行时，由主节点管理。

**经过**：WebSocket 承载、身份、link、会话、节点功能插件、远程运行时租约、loader 的 `peer:` 行。

**条件**：真实网络（延迟、丢包、断开、半开、NAT、反向代理）；两端版本可能不同；凭据可能被吊销；远端机器不完全可信。

| # | 可能出的问题 | 维度 | 影响 | 可能性 | 级 | 控制 | 阶段 | 现状 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| E1 | 断网时进行中的调用报告成功，或被自动重试导致副作用执行两次 | 故障 | 高 | 中 | P0 | FI（`fault::drop_and_close` 在第 N 帧后断开，对端计数副作用） | PR | 有（`fault_links.rs` #285；S5 两个进程之间 #322） |
| E2 | 半开连接没有在 30 s 内发现 | 故障 | 中 | 中 | P1 | FI（`half_open` + 暂停时钟） | PR | 有 |
| E3 | 重连退避参数不符合文档（0.5 s 起、30 s 封顶、±20%、稳定 60 s 后重置；凭据被拒 30 s；不兼容停止） | 语义 | 中 | 中 | P1 | U（暂停时钟，精确断言每次间隔） | PR | 部分：`rutis-host/src/connect/dial.rs` 的测试覆盖退避序列（0.5 s 起、30 s 封顶、±20%、reset）、稳定 60 s 后重置、凭据被拒后 30 s（暂停时钟） |
| E4 | 远程运行时接管时新旧租约同时存在；旧会话的迟到指令作用于新租约 | 语义 | 高 | 低 | P1 | TLA（设计）；K（`crates/rutis-host/tests/lease_e2e.rs`，Python、Go、Bun 三列） | 设计、PR | 有：TLA+（`specs/remote-lease/`，#210）检查接管时最多一个租约、接入点每个 id 最多一个实例、旧会话的指令不作用于新租约；K（`crates/rutis-host/tests/lease_e2e.rs`，Python、Go、Bun 三列） |
| E5 | 吊销凭据后仍能连接；握手进行中撤销注册，旧握手仍完成移交 | 安全 | 高 | 低 | P1 | K（`link.rs`）；TLA | PR | 部分：TLA+（#210）检查注册撤销后旧握手不移交；运行中吊销的身份层钩子（#273）在 0.19 删除（没有生产调用方），断开现有连接靠停掉那一行，换 token 要重启 rutis-host，S5 没有这一步 |
| E6 | TLS 校验漏洞：证书过期、CA 不符、主机名不匹配时仍然连接 | 安全 | 高 | 中 | P0 | U（三种负向，Rust 和 Python 各自） | PR | 有（#297；S5 第 6 步 #322） |
| E7 | WebSocket 升级请求畸形时监听方崩溃 | 安全、故障 | 高 | 中 | P0 | FZ | nightly | 有：FZ `websocket_upgrade`（tungstenite 握手与监听方对路径、bearer 令牌、子协议的读取），nightly 20 分钟（#179 → #425） |
| E8 | 拒绝的类别在不同实现间不一致（Python↔Rust） | 兼容 | 中 | 中 | P1 | K（`websocket_cross.rs` 加 Python 列） | PR | 有（#288） |
| E9 | 多跳时取消、释放没有逐跳传递 | 语义、残留 | 中 | 中 | P1 | K（新增多跳取消、多跳释放测试） | PR | 有（`fault_links.rs`，#285） |
| E10 | 服务公告乱序到达，已撤销的服务被旧公告恢复 | 语义 | 中 | 低 | P2 | U（构造乱序） | PR | 需核对 |
| E11 | 两个独立宿主进程之间（不是同一测试进程内）的行为与库级测试不同 | 语义 | 中 | 中 | P1 | E（S5：两个 `rutis-host` 进程，loopback） | PR | 有（S5，#322、#336） |
| E12 | 真实网络条件（延迟 200 ms、丢包 5%）下超时、重连频繁 | 故障、性能 | 中 | 中 | P1 | E + 容器 + netem | nightly | 有（S5 容器版 `netem.rs`，#341 → #433，nightly 只报告）：35 s 的 200 ms 延迟 + 5% 丢包下 link 不断、调用不重复；断网后心跳超时报告、退避上限内重连。发现并修复 Python `listen:` 连着控制方时 Ctrl-C 不退出、SIGTERM 不清理；断网时 B 打印 websockets 的 traceback 另开 #434 |
| E13 | 两端版本不同时，握手报不兼容的信息不足以判断该升级哪一端 | 兼容、诊断 | 中 | 高 | P1 | MX（上一版本节点 vs 当前）；G | 每晚、发布门 | 有：信息说明升级哪一端（#335）；MX（#209 → #426：上一版 `rutis-host` 节点与当前节点两个方向连上，服务双向可用，新值只让这次调用失败） |
| E14 | 远程运行时接受了 `file:` 或绝对路径的插件名 | 安全 | 高 | 低 | P1 | U | PR | 需核对 |
| E15 | 一个 link 反复断开重连后资源增长 | 残留 | 中 | 中 | P1 | SK（`websocket_soak.rs` 已有） | nightly | 有 |

#### 场景 F：接入已有的 Cordis 插件与 Cordis 应用

**内容**：两种方式——Cordis 插件作为 rutis-loader 的行运行；在 Rust 应用里挂载 Cordis 插件，构建时生成 Rust 类型。已有的 Cordis 应用作为节点（`@arcships/rutis-runtime/bridge`）在 0.19 删除：Cordis 应用用 Node 运行时跑它的插件。

**经过**：Node 运行时、Cordis 4.0.4、生成器（`generate.mjs`）、服务投影、事件转发、边界规则。

**条件**：插件是第三方已发布的 npm 包，版本由用户锁定，会持续更新；Cordis 本身也会更新。

| # | 可能出的问题 | 维度 | 影响 | 可能性 | 级 | 控制 | 阶段 | 现状 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| F1 | 挂载后的行为与原生 Cordis 不同 | 语义 | 高 | 中 | P0 | X（`dsh_baseline.rs` 与原生 Cordis 对比同一组场景） | PR | 有 |
| F2 | 生成器的类型映射错误（可选与可空的区分、newtype、活对象联合） | 语义 | 高 | 中 | P0 | U（`generate.test.mjs` 15 条）；用基线插件生成后编译并调用 | PR | 有 |
| F3 | 边界规则 3、4、5、7 的违规没有被检测，静默降级 | 语义 | 中 | 中 | P1 | K（每条规则一个跨进程测试，与原生对照） | PR | 有（`boundary_rules.rs` #283；静默降级改为报错 #337）；跨边界 serial 的协议改动等设计（#340） |
| F4 | 新发布的 Cordis 或插件版本破坏挂载 | 兼容 | 中 | 高 | P1 | X：nightly 用最新已发布版本重新跑基线（不改锁文件），失败只报告 | nightly | 有（`nightly.yml` 的 `cordis-latest`：库级挂载与原生对照、生成的绑定、S6，#190 → #429） |
| F5 | Node 进程因插件未捕获异常退出后，服务没有撤销，或依赖方没停下 | 故障 | 高 | 中 | P0 | K | PR | 有 |
| F6 | 部署到别的目录后（`RUTIS_CORDIS_ROOT`）找不到运行时或插件 | 兼容 | 中 | 中 | P1 | E（构建后复制到另一目录运行） | 合并 | 没有 |
| F7 | 无法绑定的成员没有在构建时报告 | 诊断 | 低 | 中 | P2 | G（构建警告快照） | PR | 需核对 |
| F8 | 用 `rutis.json` 挂载（而不是库级）时行为不同 | 语义 | 中 | 中 | P1 | E（S6） | PR | 部分（S6，`tests/e2e/tests/cordis.rs`，#190 → #429）；发现的已发布插件之间不能按原生规则互用服务、服务到不了 rutis 已修复（#428） |

#### 场景 G：实现迁移（尝试 → 验证 → 固化）

**内容**：设计哲学 §四：`planner` 依赖服务名 `calendar`，实现从 Python 插件换到内网节点上的插件，再换成 Rust 实现；三个阶段 `planner` 一行不改，宿主不重启。

**经过**：loader 编辑、服务名目录、内核驱逐与重载、节点导入、运行时。

**条件**：同名服务的不同实现由不同的人、用不同语言写成；切换发生在运行中。

| # | 可能出的问题 | 维度 | 影响 | 可能性 | 级 | 控制 | 阶段 | 现状 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| G1 | 同名服务的不同语言实现，返回的数据形状不一致（字段名、可选字段、错误类型），使用方在切换后出错 | 语义 | — | — | 不在范围 | 由服务作者负责（规范 Q6.9.4，2026-10-11 决定）。rutis 负责的部分是同一个值在各语言之间的转换一致（P7）、测试工具与真实运行时一致（A1） | — | — |
| G2 | 切换窗口内的调用挂起 | 故障 | 中 | 中 | P1 | E（S4：切换期间持续调用，每次要么成功要么显式失败） | PR | 有（S4，#326；阶段 1、2 也用 `rutis-host run` + SIGUSR1，#274） |
| G3 | 切换后旧实现残留（进程、link） | 残留 | 中 | 中 | P1 | E + R | PR | 有（S4，#326） |
| G4 | 使用方没有跟随重启，仍持有旧实现 | 语义 | 高 | 低 | P1 | K（内核驱逐） | PR | 有 |
| G5 | 宿主在切换过程中需要重启（违反承诺） | 语义 | 高 | 低 | P1 | E（断言宿主 pid 不变） | PR | 有（S4，#326） |

#### 场景 H：agent 写插件、接上、迭代

**内容**：设计哲学 §一的主线：宿主需要接触什么，就由 agent 写一个插件接上，试用、修改、替换。

**现状**：缺一块产品能力。`design-session-persist-and-self-tools` §三 明确"不做动态加载新代码"，agent 的工具来自 `ToolRegistry`，没有和 rutis 服务打通（#185）。所以这个场景的大部分控制要等设计。现在能确定的风险：

| # | 可能出的问题 | 维度 | 影响 | 可能性 | 级 | 控制 | 阶段 | 现状 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| H1 | agent 写出的插件死循环、阻塞事件循环或让运行时崩溃，拖垮同一运行时里的其他插件 | 故障 | 高 | 高 | P0（能力上线时） | 设计：agent 写的插件默认放进单独的运行时实例；E | 设计 | 没有设计 |
| H2 | agent 写的代码以宿主的权限运行（不是沙箱） | 安全 | 高 | 高 | P0（能力上线时） | 设计决策（设计哲学 §八"权限"）；DR | 设计 | 开放问题 |
| H3 | 加载失败的原因不能以 agent 能理解的形式返回 | 诊断 | 中 | 高 | P1 | G（错误结构化输出的快照） | PR | 没有 |
| H4 | agent 反复生成、加载、卸载数百次后资源增长 | 残留 | 中 | 中 | P1 | SK + R | nightly | 没有 |
| H5 | 用真实模型的测试不可重复 | — | 中 | 高 | P1 | 用脚本化模型后端（`ScriptedLlm`）驱动；真实模型只做 `#[ignore]` 的手动测试 | — | 有脚本化后端 |

#### 场景 I：dylib 插件

**内容**：Rust 插件编译为动态库，宿主校验身份后加载。

**经过**：`rutis-sdk`、`rutis-dylib`、launcher、`xtask`。

**条件**：工具链版本、构建路径、平台各不相同；加载错误的库可能导致内存错误。

| # | 可能出的问题 | 维度 | 影响 | 可能性 | 级 | 控制 | 阶段 | 现状 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| I1 | 身份不匹配（SDK、工具链、产物哈希）的库被加载，执行了初始化代码 | 安全、故障 | 高 | 中 | P0 | E（`tools/test-dylib*.sh`、`test-sdk-bundle.sh`） | PR（Linux、macOS） | 有；`design-dylib-sdk` §十三 有 5 项未勾，需要对账 |
| I2 | 替换失败后旧版本停止服务 | 故障 | 高 | 低 | P1 | E | PR | 有 |
| I3 | SDK 构建不可复现 | 兼容 | 中 | 中 | P1 | 两个 runner 独立构建比较哈希 | 每晚（PR 上只在 `repro` 打开时做单机检查） | 有 |
| I4 | Windows 上的 dylib 问题 | 兼容 | 中 | 中 | P1 | E（dylib-windows） | 每晚、发布门 | 有 |
| I5 | 工具链不匹配没有在编译前被 `xtask dev` 拦下 | 诊断 | 中 | 中 | P1 | E | PR | 没有（#260，不在本计划内） |
| I6 | dev 通道在生产构建中仍然存在 | 安全 | 高 | 低 | P1 | ST（检查生产构建不包含 dev 监听的符号或 feature） | 合并 | 没有 |
| I7 | dev 通道的命令在参数缺失、目标不存在、宿主关闭中时行为不明确 | 诊断 | 低 | 中 | P2 | U（命令 × 错误矩阵） | PR | 部分（5 条） |

#### 场景 J：升级 rutis

**内容**：用户把 rutis 从 0.x 升到 0.(x+1)：Rust crate、npm 包、PyPI 包、`rutis-host`。宿主、运行时、插件 SDK 可能不同时升级。

**经过**：发布列车、协议版本、插件 API 版本、持久化格式、迁移指南。

**条件**：0.x 允许破坏性变更；插件作者依赖 SDK 的版本范围，不和宿主同步。

| # | 可能出的问题 | 维度 | 影响 | 可能性 | 级 | 控制 | 阶段 | 现状 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| J1 | 迁移指南里的代码过期，照着改编译不过 | 文档 | 中 | 中 | P1 | SV（迁移指南的"改后"代码编译运行；`migration_example.rs` 已有一例） | PR | 部分 |
| J2 | 宿主升级而运行时没升级（或反过来）时，行为不明确：挂起、加载后才出错 | 兼容 | 中 | 高 | P1 | MX：上一版发布的运行时包 vs 当前宿主，断言要么正常工作，要么启动时明确报错 | 每晚、发布门 | 有（#209 → #426）：Node、Bun、PyPI（本机与远程）运行时、Go SDK 二进制与当前宿主，上一版宿主与当前运行时；对上一个发布版本（以及本仓库的版本已经发布时的这一版）全部正常或在加载时说明升级哪一端 |
| J3 | 旧版本写入的持久化配置（loader 层、dsh profile）在新版本中读不了，或含义改变 | 兼容 | 高 | 中 | P0 | U：每个版本保存一份持久化样本到 fixture，新版本必须能读 | PR | 有（0.8.0、0.9.0、0.9.1 样本，`rutis.json` 加 `format`，#284） |
| J4 | 发布列车的版本号不一致，用户装到两份内核 | 兼容 | 高 | 低 | P1 | ST（`scripts/train.mjs`） | PR | 有 |
| J5 | 插件 SDK 新版本标记了更高的插件 API，旧宿主报错不清楚 | 兼容、诊断 | 中 | 中 | P1 | U | PR | 有 |

#### 场景 K：长时间运行

**内容**：宿主连续运行数周到数月，期间有配置编辑、重新加载、运行时崩溃、断网重连。

| # | 可能出的问题 | 维度 | 影响 | 可能性 | 级 | 控制 | 阶段 | 现状 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| L1 | 宿主整体的 fd、线程、任务、内存随时间增长 | 残留、性能 | 高 | 中 | P0 | SK：宿主进程级浸泡，随机操作（编辑、重新加载、kill 运行时、断链），采样资源曲线，断言无持续增长 | nightly（1 h）、每周（更长） | 部分：S8（#192）每晚在三个平台上浸泡宿主一小时，采样宿主、运行时、对端的进程数、fd、线程、内存，断言后段不超过前段加阈值；bridge 层另有两个浸泡。缺：每周更长时间的运行 |
| L2 | 慢监听器导致事件积压无限增长 | 性能 | 中 | 中 | P1 | U（积压诊断）；文档 | PR | 有诊断，无上限（设计如此） |
| L3 | 运行时进程被反复杀掉后，宿主状态或计数错乱 | 故障 | 中 | 中 | P1 | SK（随机操作中包含 kill） | nightly | 有：S8（#192）的随机操作反复 `kill -9` Node、Python 运行时（`on-failure` 重启），每个宿主结束时核对输出与做过的事一致：`runtime <名字>: down`、`restarting`、`running` 的次数，重新加载接受与拒绝的次数，link 重连次数，没有行失败、没有放弃重启 |
| L4 | 随机操作序列中的失败无法复现 | — | 中 | 高 | P1 | 随机序列由种子生成，失败时保存种子和序列 | nightly | 有：S8（#192）的操作只由种子决定（与时间无关，有单元测试），失败时在场景的 `logs/` 写种子、已做的操作和资源采样，并打印重放命令（`RUTIS_S8_SEED`、`RUTIS_S8_OPS`）；CI 上作为 artifact 上传 |

---

## 5. 残余检查：做法与现状

规范 Q5.4.2 在当前实现上的检查方式。E2E harness 已实现进程、socket、端口、临时目录、凭据五项（`tests/e2e/src/residue.rs`，#221、#277），每个 E2E 场景结束时运行；报告残余后结束这些进程，场景失败也不留进程；本平台不运行的检查写在报告里（Q7.8）。

| 检查项 | 做法 | 现状 |
| --- | --- | --- |
| 宿主启动的进程全部退出 | Linux、macOS：harness 启动的进程带环境变量 `RUTIS_E2E_SCENARIO`，按它找进程（Linux 读 `/proc/<pid>/environ`，macOS 用 `KERN_PROCARGS2`），另加宿主进程组和已找到进程的子进程；Linux 测试进程设 `PR_SET_CHILD_SUBREAPER` 回收孤儿。Windows：宿主放进 Job Object，检查进程列表为空 | 已实现（E2E）。macOS 上读不到 Apple 自带程序的环境，离开宿主进程组又成为孤儿的这类程序会漏掉（模块注释写明） |
| socket 文件已删除 | 场景临时目录下没有 socket 文件（Windows 按 `*.sock` 文件名） | 已实现（E2E） |
| 端口已释放 | 对记录的端口（含代理的端口）重新 `bind` 成功 | 已实现（E2E） |
| 宿主的临时目录为空 | — | 已实现（E2E） |
| 凭据不在输出里 | 捕获的所有输出中查不到本场景的 token 值 | 已实现（E2E） |
| 测试进程的 fd、线程 | 回到基线，容差待定（现有浸泡为 +16）；拟把 `local_soak.rs` 的采样代码抽到共用的测试模块 | 只有两个浸泡测试 |
| tokio 存活任务数 | `Handle::metrics().num_alive_tasks()` 回到基线 | `instance_subtrees.rs`（#286）、`instance_runtimes.rs`（#191） |
| 内核注册表 | `diagnostics()` 的 fiber、绑定、监听器、事件积压回到基线 | 部分测试各自断言，没有共用检查 |
| 运行时内的注册 | 运行时提供一个测试用的查询，返回它持有的行、代理、导出槽数量 | Node、Python 运行时：设置 `RUTIS_TEST_INSPECT` 时回答控制方法 `inspect`（行、服务、宿主代理、导出槽、对象句柄；Node 另有 Cordis 插件、fiber、服务数，Python 另有监听器数），不属于协议；S7 使用（#191）。Bun、Go 运行时没有 |

## 6. 要建设的控制手段

把 §4 中 P0 和 P1 风险用到的控制手段合并，得到需要新建或扩展的部分。

| 手段 | 服务的风险 | 现状 | 还要做的事 |
| --- | --- | --- | --- |
| **确定性规则** | K10、P11，以及所有测试的可信度 | 有（#220、#231；Python 后台异常 #218） | Rust、Node 测试的后台异常没有统一机制（P11） |
| **E + R：黑盒端到端框架与残余检查** | A3–A5、A8、B2–B4、B10、C3、E11、F6、G2、G3、G5、H4 | 有（`tests/e2e`，#221、#277；三个平台） | 测试进程 fd、线程、运行时内注册的检查（§5）；Unix 上宿主被杀后不退出的运行时（B3） |
| **场景** | 同上 | S2、S3、S4、S5（loopback；容器 + netem，nightly）、S6、S7、S8（nightly）、S9 已完成 | 等待设计：S1（#185） |
| **FI：故障注入接到会话与链路** | C4、C11、E1、E2 | 有（`fault_links.rs` #285；`concurrent_launch.rs` #215；S3、S5；长时间运行中的反复故障 S8 #192） | — |
| **MX：版本与平台矩阵** | C10、A8、E13、J2、P8 | 最低与最新版本、MSRV（#256）；Windows 的 loopback、WebSocket 列（#295、#309）；上一个发布版本与当前代码（#209 → #426，Linux 完整运行） | 跨版本相遇在 macOS、Windows 上不跑 |
| **IN：安装冒烟** | B1 | 有（#254、#324） | crates.io、Go 模块；Python 指南示例的标记（#266 (d)，等 #259） |
| **PT：跨语言值往返** | P7、C5、P4 | 有（#296） | Windows 上不运行 |
| **X：测试工具与真实运行时比对** | A1 | Node、Python 有（#298） | Go `rutistest`、Bun 测试工具 |
| **X：最新 Cordis 插件的 nightly 基线** | F4 | 有（`nightly.yml` 的 `cordis-latest`，只报告，#190 → #429） | — |
| **FZ** | P2、E7、B5 | 有：会话帧、本地行分帧、错误图、WebSocket 升级请求（`fuzz/`，nightly，#179 → #425、#624） | 配置文件（B5，低优先级）、dylib 的目标文件解析（#631） |
| **SH** | K2、RS1 | 有：内核 27 个、`rutis-bridge` 30 个、`rutis-host` 3 个、`rutis-loader` 3 个模型（§7.3），`stress.yml` 每晚把前两个 crate 的每个跑到 100 万次迭代；Python 的可步进事件循环；Node 的帧泵；Kotlin 的 Fray 用例；Swift 的 TSan | dylib 的模型（#622）；Go 的显式闸门（#630）；`rutis-host`、`rutis-loader` 的模型与 Kotlin 的 `frayTest` 不在 workflow 里跑 |
| **TLA** | EV1–EV5、RL1、CL1、AH1、AH2、P10、C6、E4、E5 | 7 个目录、17 个模块、220 个配置（§7.2）；CI 的 `specs` 任务 | 逐条见 §7.2 的缺口一列 |
| **TV** | EV2、EV3、EV5、AH2 | 会话通道的 trace validation（`trace_validation.rs`，#624）；等待图（`waits/trace.rs`、`waits/trace/check.rs`，#613） | 事件宿主的动作日志（#624）；接入点随 0.19 删除（#695）撤回，不适用 |
| **KANI** | 值的范围与纯函数的边界 | 11 个 harness 在 `fuzz/kani/`：日期与时间的谓词与往返、boot blob 的 const ABI；2026-11-18 全部 `VERIFICATION:- SUCCESSFUL`（`fuzz/KANI-MIRI.md`） | 构造器本身连同 `format!` 的错误路径、`parse_boot`（`str::from_utf8`）、`boot_meta`/`is_sha256` 的 `&str` 包装；`unsafe` 的其余部分（#631） |
| **SK：宿主级浸泡** | L1、L3、D3、A5、H4 | 有：bridge 层两个；宿主进程级 S8（#192），带种子，每晚三个平台 | 每周更长时间的运行 |
| **G：输出快照** | A6、A7、B6、B9、E13、F7、H3 | `rutis-host` 的常见错误、`check`、link 状态（#217、#336） | 状态输出整体；SDK 的错误；H3 等设计 |
| **J3：持久化格式样本** | J3 | 有（#284） | 每次发布加样本（`docs/release.md`） |
| **ST 补齐** | 所有 | fmt、clippy（`lint`）、MSRV、feature 组合、每周漏洞检查（#256） | actionlint、许可证检查 |
| **补单元与契约测试** | K3–K8、P3、P6、P8、E3、E6、E8、E9、F3 等 | 0.10 补齐 K3、K6、K8、P3、P6、E6、E8、E9、F3（#280、#286、#285、#283、#288、#297） | K4 的 D22、K7 的两种 flavor、E3 的实际重试时序 |

---

## 7. 形式化验证的覆盖图

对应规范 Q9.1.2（多方交互的协议在设计阶段完成模型检查）、Q11.2（模型检查的适用条件）、Q11.2.3（设计改时模型同步改），以及 Q9.1.1 第 5 项。范围：除示例 `rutis-agent`、`rutis-cli` 之外的全部 crate（含 `rutis-dsh`）与七种语言运行时。

现状一列：**有** = 已有对应的形式化手段、结论与当前的代码一致；**部分** = 有，但落后于代码或缺几种情形；**无**。

### 7.1 五种手段各管什么

| 手段 | 用在哪里 | 证明的是什么 | 证明不了什么 |
| --- | --- | --- | --- |
| **TLA**：TLA+ 设计模型（TLC 穷举） | 多个进程 / 主机之间的协议，消息可以延迟、乱序、丢失（规范 Q11.2.1） | 在有限规模内，协议规则本身没有死锁、不违反不变式 | 实现是否照规则做；规模之外的情形 |
| **SH**：可控调度检查真实代码 | 一个进程内多个任务 / 线程的竞态（§7.3）。Rust 用 shuttle、Kotlin 用 Fray；Python 用可步进的 `SelectorEventLoop`；Node 用可注入的帧泵。Go、Swift 没有可控调度 | 在探索到的调度下，真实代码不丢唤醒、不重复执行、不死锁 | 没被纳入探索的外部事件（真实进程、操作系统、运行时自身） |
| **PT / KANI / FZ**：属性测试、有界模型检查、模糊测试（§7.4） | 有明确性质的纯逻辑：状态机、匹配规则、编解码、退避、配置求值。KANI 只用于极小的纯函数或 `unsafe` 函数；FZ 用于解析外部输入 | 随机（或穷举到界内的）输入下，结果与参考模型一致、不 panic | 参考模型和实现犯同一个错；并发 |
| **TV + K**：trace validation 与一致性套件（§7.5） | 实现与模型之间：用真实运行的日志检查"这次运行是模型的一个行为"；同一套用例在每个实现上跑 | 实现确实照模型的规则做（只覆盖跑到的运行） | 没跑到的交错 |
| **测试**：端到端、故障注入、虚拟时钟、矩阵、浸泡 | 操作系统与平台行为、计时与截止时间、资源上限、打包与发布、CLI 输出（§7.7） | — | — |

### 7.2 跨进程协议：TLA+

`specs/` 有 7 个模型目录、17 个 `.tla` 模块、220 个配置（另有非模型的 `specs/config-schema/`）。CI 的 `specs` 任务（[CI](ci.md) §4）在改了 `specs/**` 的 PR 和完整运行（每晚、发布门）里把每个模型的全部配置跑一遍，与各目录的 `expected.txt` 比较。

| # | 协议 | 目录 | 模块 | 配置 | 在穷举范围内证明的 | 现状与缺口 |
| --- | --- | --- | --- | ---: | --- | --- |
| T1 | 跨运行时同步调用与等待图 | [cross-runtime-sync](../specs/cross-runtime-sync/README.md) | `CrossRuntimeSync.tla` | 44 | 策略 B 不成环、反向调用回到发起方、`SyncWaitCycle` 只在确实成环时返回 | **部分**：`cycle_among` 的差分属性测试与等待图的 trace validation 已做（#613）；Node 按 key 扣住、跨主机的环（写成明确不保证）、运行时线程池未建 |
| T2 | 远端租约、link 重连、接入点 | [remote-lease](../specs/remote-lease/README.md) | `RemoteLease.tla`、`AccessPoint.tla` | 32 | 一个运行时最多一个活动租约、接入点每个 id 最多一个实例、旧会话的指令不作用于新租约、停止后不再重连 | **部分**：Kotlin 心跳起算点已修（#1181），Go、Swift、Node、Bun 的心跳也改为在 hello 之后起算（#1183），模型的反例配置 `lease-mut-heartbeat-from-accept` 保留；Swift 停止后到达的 `.accepted` 没有逐行核对；Cordis 节点的 `link.mjs` 不在本仓库；计时不建 |
| T3 | 统一事件：登记与分发、emit 顺序、节点提供、关闭接纳 | [events](../specs/events/README.md)、[event-close](../specs/event-close/README.md) | `EventsHost`、`EventsTable`、`EventsEmit`、`EventsLink`、`EventsHostCheck`、`CloseAdmission` | 37 + 7 | 先登记后发出（两条路径）、撤下后不再开始调用、同一对条目处处同一先后、emit 的两个顺序性质、节点只保留最高版本、关闭的 marker 与恰好一次清理 | **部分**：`RegisteredThenSeen`、`OneCallPerEntry` 各有变异反例（`table-mut-on-late`、`table-mut-dup-local`）；`emit-python-deferred` 通过，`emit-mut-future-reply` 违反 `EmitOrder`；作用域、值、错误、取消、五种分发的完整语义、多跳、多对端未建 |
| T4 | 宿主与运行时之间的行生命周期 | [row-lifecycle](../specs/row-lifecycle/README.md) | `RowLifecycle.tla` | 36 | 一行最多一个代次在运行、迟到结束只结束自己的代次、租约不长于行、服务在提供方卸载前撤下 | **部分**：`rows.schema` / `mount`、调用链、各语言卸载栈差异、shutdown 截止不在模型里；#1100 已定案、#1101 已修（#1185） |
| T5 | 调用生命周期（两端加一跳中继） | [call-lifecycle](../specs/call-lifecycle/README.md) | `CallLifecycle.tla`、`CallRelay.tla` | 35 | 每个调用最多结算一次、迟到答复丢弃、引用计数守恒、关闭后帧不改变状态、中继逐跳传取消 | **部分**：两种取消答复都已验证，回复规则已写进规格（`SESS-61`）；取消表清理与取消确认未定（#1099）；中继的关闭与关闭后对侧继续读帧不在模型里 |
| T6 | 节点托管的行 | [peer-rows](../specs/peer-rows/README.md) | `PeerRows`、`PeerOffers`、`PeerHosted`、`PeerPair`、`PeerServices` | 29 | 同一实例不同时在两个会话里、旧代迟到的结束不结束新代、门控与离线等待、两个节点互相代装不互相等待、服务版本与墓碑 | **部分**：当前代码已没有 `peer:` 行（0.19 删除），#438 无法对当前代码复现；#1100 已定案（键每次装载新取，`peer-fresh-keys`）；#1101（卸载先于装载）已修（#1185），issue 已关闭 |

不是所有跨进程的部分都需要 TLA：握手与能力协商是两方两条消息（PT 枚举判定表即可）；节点服务的导出 / 导入是"版本最高者胜"（PT）；监听 link 是接入点去掉自报 id 的特例；跨会话引用计数每个会话各自是两方协议，中继只靠进程内持有（SH 与 PT）。

### 7.3 进程内并发：可控调度（SH）

| 区域 | 部件 | 现状 |
| --- | --- | --- |
| 内核 | [`crates/rutis/src/fiber/shuttle_tests.rs`](../crates/rutis/src/fiber/shuttle_tests.rs) 的 28 个模型：TransitionTask 的每次 join、`post_join` 跨驱动退出、并发 dispose 恰好一次、代次 marker 先于 closing 发布、effect 登记与清理、在途监听器调用期间的卸载、并发子树关闭、root dispose 与 shutdown 共用一次关闭、依赖撤下、emit 链顺序、监听器表观察者、卸载取消当前代、effect 清理错误只报一次、多次完成、重启发布新代次、真实 fiber 树关闭、root 关闭时它挂载的子节点、dispose 结果跨驱动退出、依赖窗口里的重启、spawn 的任务在 shuttle 执行器上交错、root 关闭与子节点挂载的准入、两个 fiber 调用的截止时间、重启输给 dispose、`Provided` 先于 `Removed`、三个 fiber 同时重启时 `FiberStatusChanged` 的投递顺序（`core/D24a`）与锁外投递（`core/D5a`）、挂载插件的清理 effect 与服务经真实的逆序 pass（`MNT-6-4`）、方法登记过的 `Ctx` 所在行被卸载时另一线程同时登记（`MNT-11-9b`） | **有**：`stress.yml` 每晚每个模型 100 万次迭代，失败打印可重放的调度；`RUSTFLAGS="--cfg rutis_shuttle"` 才编译，普通构建不跑。#1194（干净的子 dispose 与根关闭竞争时，同一 transition 的 join 拿到 `Err(Closed)`）已由 #1196 修好，模型 18 钉住它 |
| 会话与事件宿主（`rutis-bridge`，30 个模型） | [`pipe/spawn/shuttle_tests.rs`](../crates/rutis-bridge/src/pipe/spawn/shuttle_tests.rs) 的 8 个模型（子进程结束状态，#623）、[`session/rpc/shuttle_tests.rs`](../crates/rutis-bridge/src/session/rpc/shuttle_tests.rs) 的 6 个（会话的调用与引用表、回派调用的认领、事件操作的次序、future 的最后一个等待者，#628）、[`session/rpc/waits/shuttle_tests.rs`](../crates/rutis-bridge/src/session/rpc/waits/shuttle_tests.rs) 的 1 个（等待表的快速路径）、[`memory/shuttle_tests.rs`](../crates/rutis-bridge/src/memory/shuttle_tests.rs) 的 3 个（内存通道两个方向的关闭、发送、接收与丢弃）、[`events/hub/shuttle_tests.rs`](../crates/rutis-bridge/src/events/hub/shuttle_tests.rs) 的 3 个（事件宿主：监听者随行登记与卸载、emit 队列）、[`websocket/pipe/shuttle_tests.rs`](../crates/rutis-bridge/src/websocket/pipe/shuttle_tests.rs) 的 2 个（WebSocket 管道的阻塞侧）、[`runtime/projection/shuttle_tests.rs`](../crates/rutis-bridge/src/runtime/projection/shuttle_tests.rs) 的 2 个（服务投影）、[`pipe/lines/shuttle_tests.rs`](../crates/rutis-bridge/src/pipe/lines/shuttle_tests.rs) 的 1 个（行分帧的写端）、[`marked/shuttle_tests.rs`](../crates/rutis-bridge/src/marked/shuttle_tests.rs) 的 4 个（标记服务：先注册原生服务再注册名字、撤回反序且不留残余；被拒的 provide 不留东西；同名两次 provide 只剩一个服务；实例只认领一次名字、只释放一次，#622） | **有**（#622、#623、#628）：PR 的 `ci.yml` 每个模型每种调度器 1000 次迭代，`stress.yml` 每晚 100 万次 |
| 运行时监督与宿主重载（`rutis-host`） | [`crates/rutis-host/src/restart/shuttle_tests.rs`](../crates/rutis-host/src/restart/shuttle_tests.rs) 的 2 个模型（退避的上限、被下一次启动接管的结束）、[`crates/rutis-host/src/host_shuttle_tests.rs`](../crates/rutis-host/src/host_shuttle_tests.rs) 的 1 个（重载移除运行时后等它自己的进程） | **有**（#627）：只在本地按 [CONTRIBUTING](../CONTRIBUTING.md) 跑；任何 workflow 都没有跑这 3 个模型 |
| 二进制运行时（`rutis-loader`） | [`crates/rutis-loader/src/runtime/binary/shuttle_tests.rs`](../crates/rutis-loader/src/runtime/binary/shuttle_tests.rs) 的 3 个模型：重启等旧进程结束后才装新运行时、替换掉的二进制在旧运行时停下后才装、重启期间被要的行等新运行时 | **有**（#627）：只在本地按 [CONTRIBUTING](../CONTRIBUTING.md) 跑；任何 workflow 都没有跑这 3 个模型 |
| dylib | `rutis-dylib` 的加载与符号解析 | **无**（#622 的遗留项） |
| Python | [`python/rutis/tests/scheduling.py`](../python/rutis/tests/scheduling.py) 的 `SteppedLoop`、`ScriptedChannel`：就绪回调只在测试推进时跑，时钟只随测试前进 | **部分**：用于调度与关闭相关的用例；控制的是抽出来的事件循环，不是整个运行时 |
| Node / Bun | 可注入的帧泵（`node/rutis-runtime/test/frame-pump.test.mjs` 等） | **部分**：可以按测试的顺序推帧；没有用 `fc.scheduler` 生成调度 |
| Kotlin | Fray、Lincheck | **有**（#630）：Fray 0.9.2 的 `frayTest` 任务跑 5 个测试类的 9 个用例（Counter、Queue、RuntimeState、MemoryChannel、Peer），`jvmTest` 按 `FrayTest` tag 排除它们；Lincheck 没有用。缺：`frayTest` 不在任何 workflow 里跑 |
| Swift | 没有可控调度；数据竞争靠 Swift 6 严格并发检查、顺序竞态靠 TSan 加压测 | **是**（#630、#1273）：`Package.swift` 是 `swift-tools-version:6.0`，`swiftLanguageModes: [.v6]`，四个 SDK 目标在 Swift 6 语言模式下编译，严格并发诊断是错误、会让 `swift build` 失败；编译器证不了的共享状态用 `@unchecked Sendable`，每一处就地写明不变量（`tools/swift-strict-concurrency.sh` 按 target 列出剩下的数量和诊断）；CI 的 `runtimes-swift` 任务对 Peer、Writer、EventsConformance、App、Concurrency 五个套件跑 `swift run --sanitize=thread`（`TSAN_OPTIONS=halt_on_error=1`） |
| Go | 没有可控调度；`testing/synctest` 未用，要构造特定的交错只能在测试构建里加显式闸门 | **无**（#630） |

### 7.4 纯逻辑与解析：PT / KANI / FZ

| 区域 | 部件 | 手段 | 现状 |
| --- | --- | --- | --- |
| 内核 | 监听器表的登记、撤下、`prepend`、前缀、`once` 与四种分发 | PT | **有**：[`crates/rutis/tests/listener_table_model.rs`](../crates/rutis/tests/listener_table_model.rs) 用有序列表模型逐次对照（256 例，`PROPTEST_CASES`），失败打印种子与收缩后的操作序列 |
| 会话 | 值往返（七种语言）、引用的授予与回滚、帧与值的编解码 | PT | **有**：`value_roundtrip.rs`、`grant_rollback.rs`、`session/protocol.rs`（后者也能在 Miri 下跑） |
| 会话 | `cycle_among` 的环检测，与按 TLA 定义建全部边的朴素实现做差分 | PT | **有**（#613）：[`crates/rutis-bridge/src/session/rpc/waits.rs`](../crates/rutis-bridge/src/session/rpc/waits.rs) 的 `cycle_among_agrees_with_the_models_graph`（最多 20 跳的随机图、随机 profile 与入口，默认 256 例，`PROPTEST_CASES`）与 `the_model_check_agrees_on_the_small_graphs` |
| 加载器、宿主、dsh | 编辑流水线、待保存队列与多写者 CAS、配置求值、profile 层合成 | PT | **无**（#631 并入的 #629） |
| 值的纯函数 | 日期、时间的构造与往返 | KANI | **有**（2026-11-18）：11 个 harness 在 `fuzz/kani/`，日期与时间的谓词、往返和 boot blob 的 const ABI 全部 `VERIFICATION:- SUCCESSFUL`；构造器本身经 `format!` 的错误路径、`parse_boot` 仍无结论，见 `fuzz/KANI-MIRI.md` |
| 传输 | 会话帧、本地行分帧、错误图、WebSocket 升级请求 | FZ | **有**：`session_frame`、`session_frames`、`local_lines`、`websocket_upgrade`、`error_graph` 五个目标在 `fuzz/fuzz_targets/`，`stress.yml` 每晚各跑 `fuzz_secs`；其中三个另在 Miri 下按固定输入跑（`fuzz/miri/`）。Go 的 RFC 6455 帧解析有 `FuzzFrameParsing`（Go 原生 fuzz，种子随 `go test` 跑） |
| dylib | 目标文件解析（ELF / Mach-O / PE）、清单格式 | FZ | **有**：`dylib_meta`、`binary_manifest`（#631） |

### 7.5 实现是否照模型做：TV 与一致性套件

| 对象 | 做法 | 现状 |
| --- | --- | --- |
| 会话通道 | `crates/rutis-bridge/tests/trace_validation.rs`：通道、帧与租约的步骤写成 trace，逐事件检查它是否被前面的事件允许（[会话追踪契约](formal-trace.md) §2–§4） | **有**（#624 第一块）：通道方向与结束、hello 只一次且最先、调用号与 `path`、答复至多一次、允许的悬空取消、租约的接管步骤 |
| 接入点（已撤回） | 对象在 0.19 删除（#695）：`AccessEvent` 日志、接入点与身份层吊销钩子一起删除；`AccessPoint.tla` 改为按 `rutis-host/listen` 建的最小模型，本侧只看到准入的结局 | **不适用**（#624 的接入点 TV 已撤回）：`trace_validation.rs` §3.8 的用例检查这一侧看到的 4003 吊销与 403 凭据拒绝，没有接入点自身的动作日志可对照 |
| 等待图 | 记录 `waits.rs` 的六个操作与每次判定，对照 `CrossRuntimeSync.tla` | **有**（#613）：`waits/trace.rs` 记录、`waits/trace/check.rs` 按 `RustSucc` 重放；`sync_cycles.rs` 与 `rpc_callbacks.rs` 的真实运行上逐事件检查，改坏同一份日志必须报出 |
| 事件宿主 | 用 `events-all` 运行中宿主的动作日志对照宿主模型 | **无**（#624 并入的 #616） |
| 各语言运行时 | 一致性套件：`runtime_conformance`、`events_conformance`、`tracker_conformance`、`sync_cycles`、`host_gone` | **部分**：`sync_cycles` 有 Node / Python / Go / Swift 列，没有 Kotlin；`host_gone` 有 Node / Python / Go / Bun，没有 Swift / Kotlin（#624 未做：Swift 插件只在 macOS 上构建，Kotlin 要 JDK 17 与 `:rutis-conformance:fatJar`）；租约套件 `crates/rutis-host/tests/lease_e2e.rs` 有 Python、Go、Bun 列，同样缺 Swift、Kotlin；引用计数 PT 现在七种语言都有（`value_roundtrip.rs` 的 `*_releases_balance` / `*_reference_counts`，Go、Bun、Kotlin 只查本侧，#624）；授予与回滚的 PT 在 `grant_rollback.rs`（Rust 侧） |
| 测试工具与真实运行时 | `weather.expected.json`、`events.expected.json` 中不带 `tool: false` 的用例 | **部分**：Node、Python 测试工具与真实运行时对同一份期望文件；Go `rutistest` 已对照整份 weather；与运行时共用代码的工具（JS `events.mjs`、Python `Bus`、Go `internal/events`）可能与运行时同错 |
| 模型对应表 | 每个模型 README 的"对应的设计和代码" | **有**：七个模型的 README 都用仓库全路径，`node tools/check-model-pr.mjs --list` 解析出 50 条、自测 8 个用例通过；只剩 `cross-runtime-sync` 的一行正文用短名（`waits.rs`） |

### 7.6 追踪表的统计（#634）

覆盖范围是"保证登记表 + 各协议规格的全部规则"。追踪表 1825 行：219 行登记表、54 行手写（`TRC` 44、`ECA` 10）、1552 行 #634 的规则清单（1368 条现行 + 184 条已取代）。状态：359 已验证、423 部分验证、482 未验证、61 明确不保证、296 已撤回、204 不单独计数。

协议复核（1368 条现行）：179 已验证、364 部分验证、479 未验证、42 明确不保证、201 不单独计数，103 条按 0.19 / 0.22 删除的机制记已撤回；161 条有 `SESS-*` 锚点，261 条没有现行来源。其中 67 条按证据与变异从「未验证／部分验证」升到「已验证」（35 条是 2026-11-17 那批，32 条在那之后）。

完成的标准是"应有形式化检查的规则全部已覆盖（并有变异或修复前的配置证明它不是空的），或写明理由改为其他手段；只靠测试的规则都有测试"（#612、#635）。按现在的统计差得远：479 条未验证。语义重抽已完成 `TRC` 与六个协议分区（规则文字仍是 issue 快照原文），剩下 219 行登记表与 10 行 `ECA` 没重抽。数字由 `node tools/formal-trace.mjs` 计算，不手写；逐条记录在[追踪表](formal-trace/README.md) §4 与[复核表](formal-trace/recheck-0.22.md)。

按 B 口径（#622 原文的「手段」列）先收窄范围：21 条只有具名用例的规则（按编号拆开 22 条）逐条的手段决定在[手段复核](formal-trace/means-0.23.md)。第一批 7 条 loader／host 集成层规则（`loader/rows-gate-on-services a`、`b`、`loader/row-unload-leaves-nothing a`、`host/dev-reload b`、`MNT-5-3`、`MNT-5-7`、`MNT-11-4`）按 #622 完成条件第三项把手段改为**测试**并写明理由：内核式 shuttle 只能测到替身，这些规则的判据在真实子进程、进程身份与跨进程的租借上（§7.1「SH 证明不了」那一栏）。`MNT-11-4` 的「跟随替换」与「实例里的等待／停止」补了两处断言（`crates/rutis-loader/tests/row_inject_gate.rs`、`crates/rutis-loader/tests/instance_runtimes.rs`）。其余 15 条内核／事件规则留在下一个 PR；在那之前它们的手段列仍是 `SH`／`PT`，不写成「已覆盖」。

第二批 15 条内核／事件规则落地在同一个手段复核文档：`core/D7a` 新增模型 28（内核自己的 `unload`：Unloading 先于清理、清理看到取消、LIFO、Disposed 最后），`core/D14a` 新增 `eviction_drains_every_consumer_before_the_provider`（两个消费者都回 Pending、清理先于提供方），`core/D7b` 的 update 那一半与其余确定性行为写「测试」并说明理由；`events/framework c`、`EVP-7.1-5` 以 K 为主证据，`instance/subtrees.5` 的手段列本来就写了测试，`EVP-12.1-6`、`EVP-12.1-10` 的 TV 那一半仍归 #624。内核 emit 链的 panic 模型没有写成：在 spawn 替身下驱动真实 emit 链时，同名两次 emit 的开始顺序在个别调度下反了；#1332 复现后定论为替身限制（scope 的任务在同一条 OS 线程上，spawn 窗口里跑完的任务会误置内核的 `DROPPED_INLINE`），真实执行器上不可见，因此 `core/D31b` 记为测试加宿主队列模型；根因、失败序列与数字写在[手段复核](formal-trace/means-0.23.md) §5。

### 7.7 形式化手段覆盖不到的

- 操作系统与平台：macOS 的 fork / CLOEXEC、Windows 的 TLS 与路径、CPython 与 V8 的行为差异 → 支持矩阵、端到端。
- 计时与截止时间：问候截止时间、退避的 30 / 60 秒、被拒后的等待 → 虚拟时钟测试（[测试时钟设计](design-test-clocks-2026-10-11.md)、`virtual_clock_contract.rs`；退避翻倍、被拒后 30 秒重试与 60 秒重置在 `crates/rutis-host/src/connect/dial.rs` 的 `start_paused` 用例里，问候的 10 秒截止在 `crates/rutis-bridge/tests/session_plugin.rs`；#633 并入 #639，随 #639 关闭）。
- 资源上限与缓慢增长 → 残余检查（§5）与浸泡（S8）。
- 测试设计造成的偶发失败、打包与 CI、CLI 与代码生成的逻辑错误。
- 读代码就能看到的 `unsafe` 问题：`crates/rutis-dylib/src/unix.rs:47` 的 `CStr::from_ptr(libc::dlerror())` 没有空指针检查（同文件的 `find_symbol` 检查了）。

2026-10-24 曾按 2026-10-02 至 10-24 的 150 个修复分类：产品缺陷 109 个，其中 45 个任何手段都抓不到（[#612](https://github.com/arcships/rutis/issues/612)）。这份分类没有随之后的版本重做。

### 7.8 做法

- 模型放在 `specs/<名字>/`：`.tla`、`.cfg`、`expected.txt`、`run.sh`、中英 README（对应哪份设计文档哪一节、做了哪些简化、检查了哪些不变式、变异与反例）。7 个模型目录都按此组织。
- 设计改动涉及已建模的协议时，同一个 PR 里改对应的 `specs/` 目录；只改实现时，PR 描述写一行 `模型：<目录> 不涉及（原因）`（规范 Q9.1.1 第 5 项）。这条检查在 `model-pr.yml` 里跑（`tools/check-model-pr.mjs`），不在 `ci-ok` 里。
- trace validation 从会话通道开始（§7.5）；检查的规则来自[会话追踪契约](formal-trace.md)，不另写一份。
- 时间：Spec 1（跨运行时同步）在 Bun 之前完成，Spec 2（远端租约）在 0.12 完成；事件、行生命周期、调用生命周期、节点托管行的模型在 0.22 之前完成（#613）。

### 7.9 最早两个模型的范围记录（2026-10）

保留作记录；模型本身、简化与不变式以各自目录的 README 为准。

#### Spec 1：跨运行时同步调用与死锁策略（对应 P10、C6）

**结果**（#208 → #222，2026-10-11）：模型在 [`specs/cross-runtime-sync/`](../specs/cross-runtime-sync/README.md)。选定策略 B：宿主跟踪跨会话的等待关系，转发会让等待成环时不转发，直接返回 `SyncWaitCycle`。结论写回多语言设计 §九。实现是 #228 → #261：帧上的 `sync` 字段，`sync-wait`、`sync-stack` 能力；测试在 `rpc_callbacks.rs`、`rutis-loader/tests/sync_cycles.rs`。实现时定的两点：未声明可重入的运行时按不可重入处理；未声明 `sync-wait` 的运行时不在检测范围内（不误报）。

**为什么先做**：多语言设计 §九 写明这个问题没有定论。Node 运行时不可重入，Python 可重入，两个 Node 运行时交叉同步调用会卡死。接入第三种语言（Swift、Go，或 Bun #194）前必须选定策略。卡死只在特定交错下出现，测试很难稳定碰到。

**模型**

- 参与者：Rust 宿主（中继），2–3 个运行时，各标为可重入或不可重入。
- 动作：运行时发起同步调用（目标可以是另一个运行时的服务）；宿主转发并改写调用链 `path`；被调用方在同步等待期间收到进来的调用，按自己的规则处理（不可重入：只执行本调用链上的，其他延后；可重入：全部执行）；返回；返回 `SyncWaitCycle`。
- 不变式：不死锁（有未完成的调用时，总有参与者可以继续）；反向调用一定回到发起它的运行时；`SyncWaitCycle` 只在等待确实成环时返回。

**比较的两个候选策略**（多语言设计 §九 原文）：

| 策略 | 内容 | 要回答的问题 |
| --- | --- | --- |
| A | 叶子运行时都可重入，只有 Cordis 运行时不可重入；两个不可重入运行时之间的同步调用由 Rust 侧拒绝 | 能否在调用发出时判断；会不会误拒合法调用 |
| B | Rust 侧跟踪跨会话的等待关系，检测到成环时返回 `SyncWaitCycle` | 宿主看到的信息是否足以判断成环；会不会漏判或误报 |

**规模**：3 个运行时、调用链深度 ≤ 3、并发调用 ≤ 2。

**产出**：选定的策略写回多语言设计 §九；反例转成 `rpc_callbacks.rs` 的测试。

#### Spec 2：远端租约接管、link 重连与注册撤销（对应 E4、E5）

**结果**（#210 → #432，2026-10-17）：模型在 [`specs/remote-lease/`](../specs/remote-lease/README.md)，两个：`RemoteLease.tla`（监听的远端运行时、两个拨号的控制方 link）与 `AccessPoint.tla`（issue 之后落地的接入点：注册代次与移交、自报 id、同 id 接管、接入点停止再启动）。检查的不变式：一个运行时最多一个活动租约、接入点每个 id 最多一个实例；旧会话的指令（调用）不作用于新租约（实例）；注册撤销后旧握手不完成移交；link（拨出方）停止后不再重连；迟到的握手结果不让放弃了的会话复活。活性：没有故障时最终有一个控制方持有租约，结束时仍有人持有。规模：2 个控制方（拨出方）、各最多 3 次重连（接入点打开断线的配置 2 次），消息延迟不限；最大的配置约 120 万个状态，全部 16 个配置不到 1 分钟。每个性质配一个故意引入的错误（变异），TLC 都能找到反例，模型不是空的。

TLC 找到过两处握手期间吊销身份仍建立会话，已修复（#432）；身份吊销（#273）在 0.19 删除，对应的性质、配置与测试随之删除。§4.4 的接管顺序、注册代次复核、按连接读帧、停止时取消重试与模型一致。两个控制方配置同一个运行时（或两个拨出方同一个 id）时会互相顶掉，靠退避放慢（拨出稿 §六 已写明）；模型里活性成立是因为重连次数有限。CI：`specs` 任务（[CI](ci.md) §4）。

**为什么做**：远程设计 §4.4 规定了接管顺序（鉴权 → 关闭旧会话 → 清理旧租约 → 回应新 hello）；网络栈设计规定了注册代次、握手期间撤销、迟到结果不得交付。现有的 `leases.rs`、`link.rs` 只覆盖构造得出来的时序（`leases.rs` 已在 #695 删除，由 #1113 的 `crates/rutis-host/tests/lease_e2e.rs` 接替）。

**模型**

- 参与者：2 个控制方 link、1 个远端运行时（监听方）、Identity（可撤销）。
- 动作：拨号、鉴权、hello、断开、按退避重连、新连接接管、清理租约、吊销 Identity、迟到的握手结果、旧会话的迟到指令。
- 不变式：任何时刻一个运行时实例最多一个活动租约；旧会话的指令不作用于新租约；注册撤销后旧握手不完成移交；link 停止后不再重连。
- 活性：两个控制方都正常时，最终有一个持有租约。

**规模**：2 个控制方、各最多 3 次重连、消息最多延迟 2 步。

运行中吊销曾是身份层的钩子（#273），0.19 删除；模型里的"吊销 Identity"随之删除。

---

## 8. 持续集成

对应规范 §12。CI 现在怎么运行、修改时要做什么，见 [ci.md](ci.md)。本节记录度量和与规范的差距。

### 8.1 改造前的测量（历史记录：2026-10-10，`ci.yml` 最近 30 次运行）

以下是改造前的数据，保留用来对比，不代表现状。

从推送到得到结果：没有排队时 22–25 分钟，拥挤时 35–46 分钟，最长一次超过 2 小时。

一次没有排队的运行（#202，run `38024950688`）各任务的执行时间：

| 任务 | 执行时间 | 组成 |
| --- | ---: | --- |
| dylib-macos | 25.1 min | 构建可重现 9.8、外部插件构建 7.6、launcher 校验 5.3 |
| dylib-linux | 20.9 min | 构建可重现 9.1、外部插件构建 6.1、launcher 校验 4.4 |
| test（Linux 全量） | 7.7 min | `cargo test --workspace` 3.3 |
| sdk-repro ×2、sdk-repro-macos ×2 | 3.6–4.9 min 各 | — |
| runtimes-windows | 4.2 min | — |
| network-macos | 2.2 min | — |
| 其余 8 个 | < 3 min 各 | — |

同一分支连续推送时（`feat/bun-runtime` 在 02:10–02:52 推了 6 次，6 次运行同时进行），macOS 任务的排队时间（run `38016607150`）：sdk-repro-macos 32 分钟，dylib-macos 23.5 分钟，static-platforms 14.7 分钟，network-macos 10.7 分钟。

当时的问题：没有 `concurrency`；没有按改动选择，只改文档也跑全部 17 个任务；dylib 任务是关键路径；每次占 6–8 个 macOS 任务；构建可重现检查做了三处；打包任务每个 PR 都跑；没有汇总检查；没有度量。

### 8.2 做了什么

| 改动 | 对应条款 | PR |
| --- | --- | --- |
| PR 上新推送取消旧运行；main 上按 ref 分组，运行中的不取消 | Q12.4 | #224 |
| `changes` 任务按改动打开开关，PR 只跑相关任务，main 全部运行；汇总检查 `ci-ok` 作为唯一必需检查；Linux 上 dylib 拆成 launcher、sdk-bundle、repro 三个任务；`sdk-repro*` 和打包移到 main；每周 CI 度量（`ci-stats.yml`、`tools/ci-stats.mjs`） | Q12.1、Q12.3、Q12.5、Q12.6.1、Q12.7、Q12.9 | #213 |
| CI 设计与流程文档 `docs/ci.md` | Q12.8、Q12.10 | #237 |
| `lint`（fmt、clippy `-D warnings`）；MSRV 1.88；Linux 用最低版本（Node 22、Python 3.12、websockets 15），macOS、Windows 用最新；每周依赖漏洞检查 `deps.yml` | Q5.3.7、Q10.2、ST | #256 |
| 安装冒烟 `install.yml`；发布前五平台安装、发布后用注册表上的包验证 | Q6.15、Q13 | #254 |
| `e2e` 任务（与 `rust` 并行）；去掉重复的发布构建 | Q12.7 | #265 → #275 |
| macOS、Windows 上跑和平台有关的测试：先加入 `value_roundtrip`、`fault_links`，再改为默认全部测试文件、只排除与平台无关的（`PURE_TESTS_*`） | Q10.2、Q7.8 | #313、#328 |
| 每晚 `stress.yml` 增加 `multiprocess` 任务：bridge、loader 的多进程测试重复多轮 | Q7.3 | #245 |
| 自托管 runner：`dylib-macos`、`sdk-repro-macos` 等 macOS 任务，以及与操作系统无关的任务（`links`、`js-py`、`msrv`、`checks`、`release-dry-run`、`semver-rutis`、`changes`、`ci-ok`）默认跑在自托管 macOS 上（仓库变量 `MACOS_RUNNER`、`GATE_RUNNER` 可切回）；Linux 任务、`runtimes-windows` 可由 `LINUX_RUNNER`、`WINDOWS_RUNNER` 切到自托管（默认仍是 GitHub 的 runner） | Q12.6.2 | #333、#348、#350 |

`network-macos` 也在自托管的 macOS 机器上跑（仓库变量 `NETWORK_MACOS_RUNNER`；#287 由 #344 修复后切过来，ci.md §10）。

### 8.3 测量（2026-10-12，`ci.yml` 2026-10-12 的最近 40 次完成的运行）

用 `node tools/ci-stats.mjs 40 ci.yml --since 2026-10-11` 统计。这一天正在把任务移到自托管 runner（#333、#348、#350），数据里新旧 runner 混在一起，只作为切换时的记录。

| 运行 | 次数 | 推送到结果中位 | p90 | 最长 |
| --- | ---: | ---: | ---: | ---: |
| main（push） | 13 | 26.3 min | 48.6 min | 48.6 min |
| PR | 27 | 14.3 min | 38.2 min | 42.9 min |
| PR，普通代码改动 | 9 | 15.6 min | 42.9 min | 42.9 min |

- 普通代码 PR 的目标是 10 分钟（ci.md §1），中位 15.6 分钟，未达到。排队是主要原因：多个任务的最长排队在 20–40 分钟。
- 切换完成后的最后两次运行：#350 的 PR 运行 10.1 分钟（全部任务，因为改了 CI），合并后 main 运行 7.2 分钟。
- 执行时间中位最长的任务：`runtimes-windows` 8.2 分钟、`dylib-linux-repro` 7.5 分钟、`rust` 6.2 分钟、`network-macos` 4.5 分钟。
- main 失败 3 次（#339、#349、#348 合并后），重跑过的运行 6 次（其中 4 次重跑后通过，对应的偶发失败已登记修复，§9.2）。

### 8.4 与规范的差距

| 条款 | 差距 |
| --- | --- |
| Q12.4 | ~~main 上按 ref 分组，不是每次合并都有自己的完整结果~~：规范改为由每晚的运行检查上一次以来的所有提交（#369），已满足 |
| Q12.6 | 普通代码 PR 的中位时间 15.6 分钟（§8.3），目标 10 分钟；需要自托管 runner 稳定后的一周数据再判断 |
| Q12.7 | 完整运行中单机与两机两种可复现检查并存 |
| Q12.1 | 旧记录曾写“每晚缺 TLC”，但当前 CI 在修改 `specs/**` 的 PR 上运行 `specs`；nightly 仅在相应路径有变更时运行（或 workflow 配置等全量触发路径有变更），周一的 weekly run 才无条件运行全部现有模型。仍需把完整模型矩阵、trace validation 和跨语言一致性统一纳入每日/发布前报告。每周还缺更长时间的宿主浸泡与基准测试（一小时的宿主浸泡 S8 已进每晚，#192）。真实网络（S5 容器 + netem）已进每晚（只报告，#341 → #433），最新 Cordis 插件基线也已进每晚（只报告，#190 → #429）；模糊测试与既有模型已进 stress/nightly（#179 → #425） |

## 9. 计划

按风险级别排序，P0 先做；同级的先做被其他工作依赖的基础设施。与当前的版本规划对齐。进度以 #183 和[形式化验证重新设计](design-formal-verification-2026-11-08.md)为准；旧的 0.11–0.14 排期不再作为当前计划。

截至 2026-11-08，形式化验证重新分成四层，依赖关系如下：

| 层 | 主要 issue | 当前门槛和依赖 |
| --- | --- | --- |
| A 协议基础 | #635、#634 | 先统一会话帧/握手/错误/引用/取消/上限规格，再清理旧来源并建立规则追踪 |
| B 并发与生命周期 | #622、#613、#630 | 依赖 A；#1025 先做 marker commit/admission 的模型和确定性竞态验证 |
| C 跨进程与部署 | #613、#624 | 依赖 A、B；TLA+ 证明设计，trace validation 对照真实执行，一致性用例覆盖各语言 |
| D 输入与长期可靠性 | #631、#639 | 依赖 A、B、C 的规则；属性/模糊、Kani/Miri、虚拟时钟、普通测试和 E2E 补模型边界 |

#634 是四层共用的追踪和收口，不是额外功能。0.23 已并入 0.22：0.22 同时完成 #1025 的高风险关闭语义验证和完整四层覆盖，执行计划见[形式化覆盖执行计划](plan/formal-verification-0.22.md)。不把 #1040 重新纳入 0.22。

以下 §9.1–§9.3 只保留为旧质量计划的历史记录，不是当前的版本排期；其中“已完成”仅描述当时的验收结论，不能替代新四层计划的规则、模型、trace 和跨实现证据。

### 9.1 历史记录：第一步（0.8.1 → 0.9）——基础设施和已经发生过的 P0

| 项 | 覆盖的风险 | issue → PR |
| --- | --- | --- |
| 四个已知 bug 修复并带回归测试 | P2、B2、C11、P11 | #173 → #214、#174 → #216、#184 → #215、#197 → #218 |
| 确定性规则；`interleave.rs` 加种子 | K10、P11 | #182 → #220 |
| CI：取消旧运行、按改动选择、汇总检查、合并 macOS 任务、去重 | 所有风险的反馈速度 | #203 → #213、#224、#237；收尾 #265 → #275 |
| ST 补齐（fmt、clippy、依赖检查、MSRV）；最低版本矩阵 | 全部；MX | #204 → #256 |
| E2E 框架 + 残余检查 + S2 | A3–A5、A8、B2、B3、B10 | #175 → #221、#186 → #253 |
| S9 安装冒烟 | B1；#195、#196 的验收 | #193 → #254 |
| rutis-host 配置与装配的单元测试；常见错误的输出快照 | B5、B6、A6、A7 | #205 → #217；#258 |

E2E 发现并修复：#225、#226（#261、#217）。Node 最低版本降到 22（#195 → #219），Python 降到 3.10（#196 → #255）。

### 9.2 第二步（0.10）：跨进程与跨平台的 P0——已完成（2026-10-12）

方案：[design-quality-step2-2026-10-11](design-quality-step2-2026-10-11.md)（#272）。

| 项 | 覆盖的风险 | issue → PR | 状态 |
| --- | --- | --- | --- |
| S3 跨语言组合与崩溃 | C3、C4、B10 | #187 → #317 | 完成；恢复方式为重启宿主（方案 Q1）；Windows 不含 Bun 行 |
| S4 实现迁移 | G2、G3、G5 | #188 → #326、#330 | 完成；用 fixture Rust 宿主（方案 Q1） |
| S5 两个宿主进程，loopback | E11、E1、E6 | #189 → #322、#336 | 完成；运行中吊销见 #273，容器 + netem 见 #341 → #433 |
| 协议层：故障注入与链路、边界规则 3/4/5/7、SDK 对齐、负向 TLS、值往返属性测试 | C4、E1、E6、E8、E9、P3、P4、P6、P7、P8、F3 | #178 → #285、#283、#288、#297、#296 | 完成；资源上限等设计 |
| 一致性套件的 Windows 列；补缺失的格子 | C10 | #176 → #295、#309 | 完成 |
| 测试工具与真实运行时比对 | A1 | #206 → #298 | 完成（Node、Python）；Go、Bun 测试工具为后续 |
| 持久化格式样本与 `format` 字段 | J3 | #207 → #284 | 完成；0.10 发布说明写明 `format` |
| 内核保证补齐 | K3–K8 | #177 → #280、#286；后续 #345 | 完成；D22 的环、K7 的两种 flavor 未做 |
| 其他 crate 与其他语言的真实时钟 | K10 | #231 → #257、#278、#299、#301 | 完成 |
| E2E 在 Windows 上的残余检查（Job Object）；S2 的 Windows 场景、S9 的 Windows 停止 | B3、C10 | #232 → #277、#323、#324 | 完成 |
| S9 安装冒烟的后续格子 | B1 | #266 → #324 | Bun 行、websockets 15.0、Go 行、Windows 停止完成；Python 指南示例的标记等 #259（进行中） |
| 保证登记表 | 全部 | #181 → #276、#310 | 进行中：`loader/` 族与 `host/` 其余条目未登记；0.10 以现状作为基线（方案 Q3） |
| TLA+ Spec 1 与策略 B 的实现 | P10、C6 | #208 → #222；#228 → #261 | 完成 |

第二步测试发现并已修复的缺陷：#279（#338）、#281 #282（#337）、#287（#344）、#290（#312）、#291 #292（#345）、#293（#335）、#294（#305）、#300（#334）、#302（#311）、#304（#346）、#321（#336）、#325（#330）、#327（#331）、#332（#339）。偶发失败：#289（#301）、#303（#319）、#306 #307 #316（#320）、#314（#318）、#342（#347）、深度上限测试（#349）；另有 #233（#243）、#234（#244）、#246 #249（#261）、#247（#248）、#250（#252）。

第二步中移出或新开的：运行中吊销（#273）、`run` 运行中改配置（#274）、Date / Map / Set / 二进制 / 大整数（#308）、emit 携带分发方式（#340）进入"等待设计"；S5 容器 + netem（#341）进入第三步。

### 9.3 第三步（0.12；原定 0.11，2026-10-17 顺延）：长时间运行、模糊、并发模型、设计未定的部分

| 项 | 覆盖的风险 | issue → PR |
| --- | --- | --- |
| FZ 三个目标；SH 三个模型 | P2、E7、K2 | #179 → #425。已完成（0.12） |
| 宿主级浸泡（S8） | L1、L3、L4 | #192 → #431。已完成（0.12）：每晚三个平台；发现并修复 Python 运行时在宿主被 `kill -9` 后不退出 |
| S6 接入 Cordis 插件；最新 Cordis 插件 nightly 基线 | F8、F4 | #190 → #429。已完成（0.12）；发现的缺口 #428 已修复（0.12） |
| S7 Rust 嵌入与多租户 | D2、D3、D5 | #191 → #430。已完成（0.12）：发现并修复语言行的投影任务与导出槽泄漏 |
| 上一版运行时 vs 当前宿主 | J2、E13、P8 | #209 → #426。已完成（0.12）：`tests/e2e/tests/cross_version.rs`，上一个发布版本由 npm 上的版本推出，在 `ci.yml` 完整运行（每晚、发布门）的 `e2e` 任务里跑；没有发现加载后才出错或挂起的组合 |
| TLA+ Spec 2 | E4、E5 | #210 → #432。已完成（0.12）：两个模型，CI `specs` 任务；找到并修复两处握手期间吊销仍建立会话 |
| Bun 运行时按 Q6.8 准入（B2、B3） | C10 的 Bun 变体 | #194 → #200；远程监听与拨出 #402；#236 → #261。已完成 |
| S5 nightly：两个容器、非 loopback + TLS、netem | E12 | #341 → #433。已完成（0.12）：`tests/e2e/tests/netem.rs`，`netem.yml`，每晚只报告；发现并修复 Python `listen:` 连着控制方时 Ctrl-C 不退出、SIGTERM 不清理；断网时 B 打印 websockets 的 traceback（#434） |
| 值的跨语言一致：Python `datetime` 按引用传、Go 结构体绕过检查、Python dict 删 undefined 键 | P7、A1 | #355 → #410、#412、#413。已完成（0.11） |
| 跨语言传递日期时间、二进制、Map、Set、大整数、Decimal、UUID | P7、A1 | #308、#360 → 方案 #404、#414，实现 #406、#415、#410、#412、#413。已完成（0.11） |
| 运行中吊销对端凭据，不重启进程；完成后 S5 加一步 | E5 | #273 → #424。已完成机制（0.12）：身份层的吊销钩子与库级测试；rutis-host 没有触发方式，S5（loopback 与容器版 #433）都没有这一步，留给管理服务 |
| `rutis-host run` 运行中改配置；运行时崩溃后重新拉起 | B4 | #274 → #421、#422、#423。已完成（0.12）：SIGUSR1 / `--watch-config` 重读配置，全有或全无（`host/reload-config`）；崩溃自动重启可选，默认关（`host/runtime-restart`） |
| 语言运行时按注册表组织，测试辅助移到 `common` | — | #230 → #420。已完成（0.12） |

第三步发现并修复：#407、#408、#409（#419），#411（#427），#428（#443），#434（#436），#435（#437），#438（#439），发布评审的修正（#450）。

原第三步的"CI 改造 §8.6 第 3–4 步"（Linux 上拆分 dylib 任务、CI 度量）已在 #213 完成。

### 9.4 当前形式化验证工作

旧的“等待设计”清单不再作为当前排期。现行设计和实施边界见[形式化验证重新设计](design-formal-verification-2026-11-08.md)，按以下八个 issue 收口：

| 层/工作 | issue | 状态和下一步 |
| --- | --- | --- |
| 协议基线与规则追踪 | #635、#634 | 已关闭（2026-11-16，COMPLETED）：会话协议规格第一版（`docs/protocol-session.md`）与行协议规格第一版（`docs/protocol-rows.md`）已写入；规则追踪表 1825 行（含 1368 条协议规则，§7.6）。未知字段已定：拒绝但只失败该调用（`bad-frame`），七种运行时都已实现并进 CI |
| 事件、调用、生命周期模型 | #613 | 已关闭（2026-11-16，COMPLETED）：7 个模型目录、17 个模块、220 个配置：`cross-runtime-sync`、`remote-lease`、`event-close`、`events` 四模块、`row-lifecycle`、`call-lifecycle`、`peer-rows`；逐目录的现状与缺口见 §7.2。模型发现的 bail/async/取消表/peer row 键等已转 issue 或实现修复 |
| Rust 实现交错 | #622 | 已关闭（2026-11-16，COMPLETED）：内核 28 个 shuttle 模型通过（§7.3）：#1025 的 marker 发布、`Entry::live`、effect 登记与清理、在途卸载、并发子树关闭、root dispose/shutdown、依赖撤下、emit 链、监听器表、重启发布新代次、真实 fiber 树、子节点挂载、dispose 结果跨驱动退出、依赖窗口里的重启、spawn 的任务、截止时间；7 个确定性竞态用例。会话层与事件宿主的 30 个模型（`rutis-bridge`）、运行时监督与宿主重载的 3 个（`rutis-host`）、二进制运行时重启的 3 个（`rutis-loader`）也已合入（#622、#627、#628）；`interleave.rs` 的顺序参考模型已合入（#1190）；#1194 由 #1196 修好。待补：dylib；`rutis-host`、`rutis-loader` 的模型不在任何 workflow 里跑 |
| 非 Rust 运行时调度 | #630 | 已关闭（2026-11-16，COMPLETED），首块完成：Node 帧泵 4 个用例、Python 的 `SteppedLoop`；Bun 只有现状盘点；Kotlin 的 Fray 用例与 Swift 的 TSan 后来补上（§7.3）；Go 仍没有可控调度 |
| trace 与一致性 | #624 | 已关闭（2026-11-16，COMPLETED），首块完成：Rust 通道/帧验证器 23 个用例（引用计数、错误图、调用范围、上限、替换/重连分类）；租约套件补了 Go、Bun 列（Python、Go、Bun 三列）；接入点 TV 随 0.19 删除（#695）撤回，不适用；事件宿主的动作日志、`sync_cycles` 的 Kotlin 列与 `host_gone` 的 Swift / Kotlin 列待补（理由见 §7.5）；等待图已做（#613，§7.5） |
| 属性、模糊、Kani/Miri | #631 | 已关闭（2026-11-16，COMPLETED），首块完成：引用授予回滚 6 个用例、值/帧/坏帧属性 5 个（proptest + corpus 重放）、监听器表模型属性；cargo-fuzz 实际跑 4 个目标无 crash；Miri 跑了会话帧与值路径（无 UB）；Kani 6 个 harness 没有结论（0.68.0 不收敛），不写成通过（`fuzz/KANI-MIRI.md`）。服务槽与五种分发的参考模型已合入（#1200：`crates/rutis/src/registry/prop_tests.rs`、`crates/rutis/src/bus/prop_tests.rs`） |
| 一致性与普通测试 | #639 | 已关闭（2026-11-15，COMPLETED）：按追踪表补的一致性用例分批合入，虚拟时钟计时用例在 `rutis-host/src/connect/dial.rs`、`rutis-bridge/tests/session_plugin.rs` 里（§7.7）；追踪表的状态仍按复核结论计（§7.6），`M1-13-3` 等行仍记未验证 |
| 统一收口 | #634 | 已关闭（2026-11-16，COMPLETED）：追踪表 1825 行（含 1368 条协议规则），`node tools/formal-trace.mjs --check` 零问题，rule tracker 检查已进 CI；协议规则已按 0.22 来源复核（§7.6、[复核表](formal-trace/recheck-0.22.md)），但规则文字仍是 issue 快照原文，没有按现行规格重写 |

八个 issue（#613、#622、#624、#630、#631、#634、#635、#639）都已关闭；表中列出的缺口由 #776 与残余的 #1270–#1274 跟踪。

#1040 继续延期；不把服务公告策略重新纳入 0.22。

---

## 10. 待确认的假设与开放问题

1. **规模假设**（§2.2）：影响 D11、L1、L2 的评级，以及浸泡的时长和规模。
2. ~~`rutis-host run` 是否监视配置文件~~：已核对（2026-10-11）：不监视。0.10 不改 `run`，S3 用重启宿主恢复，S4 用 fixture 宿主（第二步方案 Q1）；`run` 是否支持运行中改配置见 #274。
3. ~~服务契约测试工具（G1）~~：已决定（2026-10-11）。同一服务不同实现的数据是否一致由服务作者负责，rutis 不提供契约工具（规范 Q6.9.4）；rutis 只保证跨语言的值转换规则在各运行时和测试工具里一致（P7、A1）。
4. **agent 写插件的隔离与权限（H1、H2）**：设计哲学 §八"权限"已列为开放问题。
5. ~~TLA+ 的引入~~：Spec 1 已完成（#222）。Spec 2 已完成（#210）。TLC 由 CI 的 `specs` 任务在 PR（改了 `specs/**`）和每晚运行。
6. ~~S5 的"吊销 token"~~：已决定（第二步方案 Q2）：运行中吊销作为对 remote 的需求另开 #273；0.10 的 S5 测"换 token 重启后重连被拒"。
7. ~~登记表的发布门从哪个版本执行~~：已决定（方案 Q3）：0.10 建立基线，Q9.4.4 从 0.11 起执行。
8. ~~持久化格式是否加版本字段~~：已决定（方案 Q4）：`rutis.json` 加顶层 `format`；dsh 的 `cordis.patch.yml` 是 Cordis 的配置格式，不加。
9. ~~值往返超出转换表的情况怎么处理~~：已决定（方案 Q5）：先在表里写明当前行为，登记为明确不保证；改实现另开 issue（#308）。
10. ~~第二步的实现方式~~：已决定（方案 Q6）：逐个开 PR，方案 PR 只保留方案。
11. ~~main 上的并发组与 Q12.4~~：已解决（2026-10-13，[CI 重新设计](design-ci-redesign-2026-10-13.md)，#369）：合并到 main 不再运行检查；每晚的运行检查上一次成功以来的所有提交，失败时开问题单写明提交范围；规范 Q12.3、Q12.4 随之修改。
12. ~~Unix 上宿主被强制终止后不退出的运行时（Q5.4.4、B3）~~：已修复（#353、#354）：会话断开后，运行时在 `RUTIS_SHUTDOWN_TIMEOUT`（由 rutis-host 的 `--shutdown-timeout` 传入，默认 10 秒）内卸载各行后退出。
13. ~~`network-macos` 是否切到自托管 runner~~：已切换（仓库变量 `NETWORK_MACOS_RUNNER`，ci.md §10）。

---

## 附录：已评估的手段与现行结论

按规范 Q11.3 评估。**采用** 的已经在跑；**部分采用** 的写清用在哪一块、哪一块不采用；**不采用** 的是在 §4 中没有对应的 P0 / P1 风险，或与已选手段重复。

| 手段 | 现行结论 | 用在哪、为什么不用、什么情况下重新考虑 |
| --- | --- | --- |
| 覆盖率门槛 | 不采用 | 覆盖率只说明代码被执行过，不说明结果被检查；设门槛会诱导出只为提高数字的测试。§2.3 的覆盖率只作量级参考 |
| 变异测试设门槛 | 不采用 | 运行耗时数小时；门槛需要有人持续处理存活的变异体。TLA 的"每个性质一个变异"是模型内部的做法，不是这条 |
| 内核 proptest 状态机 | 采用 | 监听器表已有 model-based 属性测试（`crates/rutis/tests/listener_table_model.rs`）；fiber 生命周期的顺序操作批次在 `interleave.rs`（带种子），参考模型已在 #1190 合入。Bug A 用顺序操作批次就能触发，这一条不再以"与契约测试重叠"为由排除 |
| 有界模型检查（Kani） | 部分采用 | 11 个 harness 在 `fuzz/kani/`，覆盖日期与时间的谓词和往返、boot blob 的 const ABI（#1271）；2026-11-18 全部 `VERIFICATION:- SUCCESSFUL`（`fuzz/KANI-MIRI.md`）。构造器本身连同错误消息、`parse_boot`、`boot_meta`/`is_sha256` 的 `&str` 包装仍无结论；`unsafe` 与 dylib 的其余部分仍在 #631 |
| Miri | 部分采用 | 用在会话帧与值路径：`fuzz/miri/` 按固定输入跑 30 个（13 帧、1 个超深嵌套、9 个分帧种子、7 个错误图），无 UB；`session/protocol.rs` 的属性测试在小例数下也在 Miri 里跑（记录里的一次 122 s）。不覆盖 tokio、网络与文件，因此不用于 dylib |
| ASan | 不采用 | `unsafe` 集中在 dylib 加载和进程启动（FFI 与系统调用），没有能跑 ASan 的 Rust 侧代码；dylib 的内存问题由 Miri 的范围与端到端覆盖 |
| TSan | 采用（只用于 Swift） | Swift 的数据竞争加压测（#630）：CI 的 `runtimes-swift` 任务对 Peer、Writer、EventsConformance、App、Concurrency 五个套件跑 `swift run --sanitize=thread`（`TSAN_OPTIONS=halt_on_error=1`）；`Package.swift` 是 Swift 6 语言模式，严格并发是错误（§7.3）。Rust 侧没有用 |
| loom 穷举 | 不采用 | shuttle 已覆盖内核的 27 个模型，loom 需要更大的代码改造；shuttle 发现问题但无法稳定复现时再考虑 |
| SDK 各自运行通道契约、跨语言帧标准样本 | 不采用 | 与 Rust 驱动的一致性套件重叠；非法帧的部分已并入 P3 与 `malformed` 矩阵 |
| `cargo public-api` | 不采用 | 与 `cargo semver-checks` 重复 |
| TLA+ trace validation | 采用 | 重新考虑的条件（spec 与实现多次不一致）已满足：会话通道的验证器已落地（`crates/rutis-bridge/tests/trace_validation.rs`，#624），等待图也已落地（#613）；接入点 TV 随 0.19 删除（#695）撤回（§7.5），事件宿主的动作日志按 §7.5 待补。规则来自[会话追踪契约](formal-trace.md) |
| TLA+ 用于内核状态机 | 不采用 | 单进程，历史问题都在实现层；进程内并发用 shuttle（§7.3） |
| CI 上的性能回归阈值 | 不采用 | CI 机器噪声超过 15%，会反复误报；性能目前不是承诺。有固定基准机器时再考虑 |
| 磁盘满、fd 上限、OOM 注入 | 不采用 | §4 中对应的风险都是低可能性；用户报告相关问题时再考虑 |
| 登记表从文档自动抽取、CI 强制核对 | 部分采用 | 追踪表的证据检查、编号与词汇检查已在 CI（`node tools/formal-trace.mjs --check`）；登记表本体仍手写，正则抽取误报多 |
| 端到端场景的 TOML DSL | 不采用 | 场景数量少，用 Rust 直接写更简单；场景超过 15 个，或非 Rust 贡献者需要写场景时再考虑 |
