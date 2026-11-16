# 规则：协议清单 runtime（#634 评论）

[中文索引](../README.md) · [English](../README.en.md) · 来源：[评论 6384933883](https://github.com/arcships/rutis/issues/634#issuecomment-6384933883) · [保证登记表](../../guarantees.md)

本文件把 #634 的 `runtime` 分区评论（[评论 6384933883](https://github.com/arcships/rutis/issues/634#issuecomment-6384933883)）里的 Markdown 表格导出到仓库。表格内容原样保留；单元格里的 `|` 按 Markdown 表格规则转义为 `\|`，渲染后文字不变。列的含义见 #634 正文：编号、来源、规则、参与方、手段、对应、现有形式化、状态、归属 issue、备注。

抽取基线：`main` `3496669b`（issue 正文的统计口径）。表格保持 issue 原文；这 309 行已按 0.22 现行来源复核，并在追踪表的语义重抽里逐行重抽来源、锚点、证据与状态（见 `README.md` §1 与[复核表](../recheck-0.22.md) §5）。状态列仍是 issue 的词汇，`node tools/formal-trace.mjs` 在统计时按 `README.md` §3 的映射换成追踪表状态；映射与复核都只降不升，不改写本文件。行协议的现行规格是[行协议规格](../../protocol-rows.md)；本分区的行生命周期与服务规则在追踪表里仍以登记表行作锚点（见 `README.md` 2026-11-16），尚未改成 `ROWS-*`。

分区合计：现行 264 条，已取代 45 条，共 309 行（与 issue 正文的统计表一致）。

## 分区 `runtime`

> 分区 `runtime`：跨运行时调用、挂载、行与服务（SYN、M2、MNT、REQ、RTP、INS、MSV）
>
> 现行 264 条，已取代 45 条（列出、不计入）。"归属 issue"一列带 ※ 的是这次新分配或调整的。列的含义见 #634 正文。

| 编号 | 来源 | 规则 | 参与方 | 手段 | 对应 | 现有形式化 | 状态 | 归属 issue | 备注 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| SYN-2-1 | SYN §二.1、§二.6、§八 | 每种语言一个运行时插件；每个运行时实例一个进程，同一运行时的行装进同一进程，进程数不随插件数增长；默认每种语言一个实例，需要隔离时才多开 | 外部 | 测试 | — | — | 不适用 | — | 进程数是操作系统层面的观察；结构由 `RuntimePlugin` / `LocalRuntime` 保证（`runtime/plugin.rs:1-11`） |
| SYN-2-2 | SYN §二.2 | 依赖门控、启停、重启、配置分层、热更新只由 rutis 与 loader 实现；每个其他语言的插件在 rutis 里是一行（一个 fiber） | 进程内并发 | 测试 | loader/rows-gate-on-services | — | 不适用 | — | 结构性分工，不是可穷举的行为；可检查的部分见 SYN-4-*、SYN-6-* |
| SYN-2-3 | SYN §二.3 | 叶子插件 `apply(ctx, config)` 返回的清理函数在这一行卸载（或会话结束）时执行 | 跨进程 | K + TLA | loader/row-unload-leaves-nothing | `host_gone.rs`（宿主离开时运行清理：Node / Python / Go / Bun）、`leases.rs`（会话结束时清理：Python / Node / Go / Bun） | 部分 | #624 | Swift、Kotlin 列缺（#624）；正常 `rows.unload` 路径的清理不在套件里 |
| SYN-2-4 | SYN §二.3 | 事件只有一条进程内共享总线：`ctx.on` / `ctx.emit` 收发同进程事件和宿主按 `emits` 发入的事件 | 跨进程 | — | sdk/leaf-events | — | 已取代 | — | 统一事件（EVD §三、EVP）取代；`emits` 字段 0.13 删除（`cordis/build.rs:317-321`、`removed_field` :360-366） |
| SYN-2-5 | SYN §二.3 | 监听随插件卸载注销 | 跨进程 | — | sdk/leaf-events | — | 已取代 | — | 由 EVP 的 `events.off` / `rows.ended` 规则重新规定，归事件分区 |
| SYN-2-6 | SYN §二.3、REQ §5 规则 5 | `internal/*` 不跨边界 | 跨进程 | — | boundary/5.5 | — | 已取代 | — | REQ 规则 5 已删（#464），改为"宿主拒绝登记 `internal/*` 的监听器"，由 EVD 第三节规定 |
| SYN-2-7 | SYN §二.3 | 没有 `events` 运行时能力的运行时设置 `emits` 在挂载时失败（只检查 `RuntimePlugin` 路径） | 跨进程 | — | — | — | 已取代 | — | 0.13 删除 `emits`；`events-v2` 是必需能力，不声明的对端在握手后即被拒（`rpc.rs:740-744`）；`ROW_FEATURES` 只查 `rows.v2`、`hosts`（`runtime/plugin.rs:27`） |
| SYN-2-8 | SYN §二.3 | 监听器出错时 Node、Bun、Python 的异步失败结束进程，Go 同步返回 error | 跨进程 | — | — | — | 已取代 | — | 插件 API 现在写 `emit` 的监听器失败"从不结束进程"（`guide/plugin-api.md:52`） |
| SYN-2-9 | SYN §二.4 | Node 运行时是完整的 Cordis：同一运行时里的 Cordis 插件按 Cordis 原生规则互相依赖 | 外部 | 测试 | — | — | 不适用 | — | 第三方框架的语义；端到端对照原生 Cordis（`tests/e2e/tests/cordis.rs`） |
| SYN-2-10 | SYN §二.5、§五、§六、§八、§十一 M2；M2 §三.1 | 同一运行时里两个插件互相使用服务时在本地直接完成：使用方拿到提供者的对象本身，不经 rutis、不走 IPC；只有跨运行时的调用经 rutis | 跨进程 | K | loader/services-by-name | — | 未覆盖 | #639 ※ | 只有普通测试 `multilang.rs`；一致性套件没有"同进程拿到对象本身"的用例 |
| SYN-2-11 | SYN §二.7、§七、§十一 M4 | Go：一组插件编译进一个可执行文件，作为一个运行时进程拉起；换代码即换二进制，并重启这一组插件 | 外部 | SH + 测试 | loader/go-runtimes | — | 未覆盖 | #627 | #627 的二进制监督模型（同名一个进程、只在文件变化或请求时重试）；换二进制后重启这一组只在真实进程测试里 |
| SYN-2-12 | SYN §二.7、§七 | Swift 运行时用 `dlopen` 加载插件 bundle | 外部 | — | loader/swift-runtimes | — | 已取代 | — | Swift SDK 附录 A：一个插件组一个签名二进制，与 Go 相同 |
| SYN-2-13 | SYN §二.8、§十一 M2；RTP §3 | 多语言运行时沿用同一份会话协议（帧、引用、取消、同步调用链）与 `rows.*` 控制操作，不为此升协议版本 | 跨进程 | K + TLA | bridge/version-refusal | `session::testing`（`runtime_conformance.rs`，7 种实现） | 部分 | #625、#639 ※ | 会话部分有套件；`rows.*` 的行契约没有跨语言检查（#625 只按 Node 的规则建，其他语言写进对应表） |
| SYN-2-14 | SYN §二.9、§四、§十一 M1/M2；M2 §三.1 | 运行时只依赖它启动本身需要的东西（它的会话），不依赖任何行的服务：几个运行时同时冷启动、各自的行互用对方的服务时全部启动，运行时之间不互等 | 跨进程 | TLA | loader/runtimes-do-not-wait-for-each-other | — | 未覆盖 | #625 ※ | 代码：`RuntimePlugin::injects` 只有 `RuntimeSession#<名>`（`runtime/plugin.rs:277`）；#625 只建一个运行时，多运行时的活性没有计划 |
| SYN-2-15 | SYN §二.9、§五 | 插件要用的服务的门控落在具体的消费行上；调用路由在运行时启动后动态绑定和撤销 | 跨进程 | TLA | loader/rows-gate-on-services | — | 未覆盖 | #625 | #625 的依赖出现 / 消失与 `hosts.provide` / `hosts.withdraw` 租约 |
| SYN-2-16 | SYN §二.10、§四、§六、§十一 M1 | 运行时握手完成后先提供 `<名>Runtime`；刷新完绑在它上面的行的解析后才提供 `<名>RuntimeRows`；行依赖后者 | 跨进程 | TLA | loader/rows-gate-on-services | — | 未覆盖 | #625 | #625 `NoProvisional`（`before-stale`，6f8fb5c7）。实现：第二段是 rutis-loader 的 `RuntimeRowsPlugin`（`rutis-loader/src/runtime/rows.rs:1-10`），不是运行时插件本身 |
| SYN-3-1 | SYN §三、§九「调用号前缀」、§十 | 运行时一侧的调用号都用 `node:` 前缀，Rust 侧只接受 `node:` 开头的调用号 | 纯逻辑 | FZ + K | — | — | 未覆盖 | #631 | 只对 compat 格式（协议 2）的本机运行时会话成立；endpoint 格式（3）用端点 id 作调用号（`session/mod.rs:14-18`）。`MALFORMED` 没有前缀错误的用例；#631 的有状态帧 fuzz 可覆盖 |
| SYN-4-1 | SYN §四；M2 §一 | 运行时可以有多个实例：服务键带名字（`Runtime::key(name)`、`RuntimeRows::key(name)`），Node 默认 `node`，Python 默认 `py` | 纯逻辑 | 测试 | — | — | 不适用 | — | 键由名字直接构造（`runtime/plugin.rs:42`、`rutis-loader/src/runtime/rows.rs:51`），默认值是常量，没有可探索的状态 |
| SYN-4-2 | SYN §四 | 宿主服务不放在运行时的依赖里：每一行按需租用（`lease_host`），等待落在这一行上 | 跨进程 | TLA | loader/rows-gate-on-services | — | 未覆盖 | #625 | #625 `LeaseWithinRow`；#625 的疑点：`lease()` 在等 `hosts.provide` 时被丢弃会把计数留在 1（`process.rs:1268-1300`） |
| SYN-4-3 | SYN §四；M2 §三.4；RTP §2「崩溃」 | 运行时进程意外结束时 `Runtime` 与 `RuntimeRows` 都撤销：它的行和用到它的服务的其他行停下等待 | 跨进程 | TLA + SH | loader/runtime-down-stops-its-rows | — | 未覆盖 | #625 | #625 的"运行时崩溃与重启"动作；观察进程退出的竞态在 #623 |
| SYN-4-4 | SYN §四；M2 §三.4；REQ §7 | 运行时崩溃时其他运行时、其他挂载以及不用它的服务的行照常运行 | 跨进程 | TLA | loader/runtime-down-stops-its-rows | — | 未覆盖 | #625 ※ | #625 只建一个运行时，隔离性不在任何计划里 |
| SYN-4-5 | SYN §四；RTP §2「崩溃」；REQ §8 | rutis 库不自动重启崩溃的运行时，是否重启由应用决定 | 进程内并发 | 测试 | loader/runtime-down-stops-its-rows | — | 不适用 | — | "不做"的规则：`RuntimePlugin` 没有重启路径，`watch_exit` 只撤服务（`runtime/plugin.rs:410-418`）。rutis-host 的 `restart` 见 REQ-8-1 |
| SYN-4-6 | SYN §四、§六 | 行依赖所在运行时的 `RuntimeRows` 加上插件声明的依赖；它用到的其他服务由这一行自己等，不由运行时等 | 跨进程 | TLA | loader/rows-gate-on-services | — | 未覆盖 | #625 |  |
| SYN-4-7 | SYN §四；RTP §2「行」 | 行 apply 时请运行时装载插件（`rows.load`），清理时卸载（`rows.unload`） | 跨进程 | TLA + K | loader/row-unload-leaves-nothing | — | 未覆盖 | #625 | #625 `OneGeneration`、`OwnGenerationOnly`；`rows.ended`、代次、拒绝没有写进本分区的任何文档（问题 2） |
| SYN-4-8 | SYN §四；RTP §2「行」 | 只改易变（`x-volatile`）字段的更新就地交给运行时（`rows.update`），不重启这一行 | 跨进程 | TLA + K | loader/volatile-in-place | — | 未覆盖 | #625 ※ | #625 的动作里没有 `rows.update`（#613 也写明不建）；叶子运行时没有 volatile，见 M2-2-5 |
| SYN-4-9 | SYN §四 | 哪些字段算易变、一次修改是否只改了易变字段的判定 | 纯逻辑 | PT | loader/volatile-in-place | — | 未覆盖 | #631 | #631"volatile 判定" |
| SYN-4-10 | SYN §四、§五、§十一 M1 | 行提供的服务投到 rutis，按名字登记进服务名目录，任何语言的行和 Rust 插件都按名字使用 | 跨进程 | TLA + K | loader/services-by-name、loader/announced-services | — | 未覆盖 | #625 | #625 的服务公告（`ServicesBeforeRunning`、`NewerWins`） |
| SYN-4-11 | SYN §四「放置」；M2 §一 | 行名带运行时前缀（`py:`、`go:`、`swift:` 等）选运行时，不带前缀的 npm 包名走 Cordis 运行时，`dylib:` 照旧；前缀后面的部分由运行时自己解析 | 纯逻辑 | PT | — | — | 未覆盖 | #631 | #631 只列"二进制解析器的命名与缓存"；Resolver 链按前缀分派没有计划 |
| SYN-5-1 | SYN §五「启动」；M2 §一 | 运行时插件的配置给出启动命令（`Launcher { program, args, env, cwd }`），最后两个参数是通道和项目位置 | 外部 | 测试 | — | — | 不适用 | — | 进程启动参数。通道现在是 `fd:3`、socket 路径或 loopback 地址（`transport/local/spawn.rs:76`、`runtime/spawn.rs:233`），不再总是 socket 路径 |
| SYN-5-2 | SYN §五「装载」 | `rows.load(key, entry, config, isolate, inject)`：`entry` 由运行时自己解释（模块名、二进制里的插件名） | 跨进程 | K + TLA | — | — | 未覆盖 | #625、#639 ※ | #625 建 `rows.load` 的代次；消息字段在各语言间的一致性没有套件 |
| SYN-5-3 | SYN §五「装载」 | `isolate` 只对 Cordis 有意义，叶子 SDK 可以忽略 | 跨进程 | — | — | — | 已取代 | — | INS §4.2：Python 补上 `isolate`，实例内服务靠它（`scopes`）；现在各运行时都实现 |
| SYN-5-4 | SYN §五「描述」、§六 | `rows.schema(entry)` 返回配置的 JSON Schema，并报出插件声明的依赖与提供的服务及其方法形状 | 跨进程 | K | loader/schema、loader/rows-gate-on-services | — | 未覆盖 | #639 ※ | 现在没有声明也会投出服务（loader/announced-services，#428），schema 里的服务只是声明 |
| SYN-5-5 | SYN §五「服务」 | 运行时用服务槽位通知（`service` 控制调用）报出行提供的服务，rutis 按名字注册并登记进服务名目录 | 跨进程 | TLA | loader/announced-services | — | 未覆盖 | #625 |  |
| SYN-5-6 | SYN §五「服务」 | 行服务的键为 `TypeKey::keyed_dynamic::<RemoteService>(name)` | 纯逻辑 | — | — | — | 已取代 | — | 键是 `host_key(name)`，与 Rust 提供的宿主服务同键（`runtime/rows.rs:4-6`） |
| SYN-5-7 | SYN §五「方法形状」 | 运行时报服务时一并报出每个方法同步还是异步（Python 用 `inspect`，Swift 由 SDK 声明） | 跨进程 | — | loader/announced-services | — | 已取代 | — | 现在没有声明的方法一律按 `async` 调用，声明了 `sync` 的才同步（loader/announced-services，M1 §十四） |
| SYN-5-8 | SYN §五「调用方向」；M2 §一「跨语言转发」 | 其他语言的插件用 rutis 服务或另一语言的服务走 `host:<名字>`：rutis 在调用时按名字找到当前提供者，来自另一个运行时就经调用方的会话转发过去 | 跨进程 | TLA | loader/services-by-name | `CrossRuntimeSync`（转发与回复的路由，`RoutingSound`） | 部分 | #625 | 同步模型建了转发，但按名字找"当前提供者"、提供者换代时的路由没有建 |
| SYN-5-9 | SYN §五 | 叶子 SDK 的 `ctx.use(name)` 返回按名字调用的代理；消费行把 `name` 列在依赖里：服务存在时才启动，撤销时停下 | 跨进程 | TLA | loader/rows-gate-on-services | — | 未覆盖 | #625 |  |
| SYN-5-10 | SYN §五 | Cordis 插件靠 Cordis 的 `inject` 等服务：rutis 在服务出现、撤销时用 `hosts.provide` / `hosts.withdraw` 在 Node 的 Context 里注册、撤销代理 | 跨进程 | TLA | loader/rows-gate-on-services | — | 未覆盖 | #625 | 实现按行租约（`Process::lease_host_for`，`process.rs:1233`）：第一次租用发 provide，最后一次释放发 withdraw |
| SYN-5-11 | SYN §五、§九「待定」 | 转发经跨会话的引用转交：函数和异步结果可以转给第三个会话 | 跨进程 | TLA + PT | bridge/multihop-cancel-and-release、bridge/release-count | `value_roundtrip.rs` `*_releases_balance`（两方，无中继） | 部分 | #626 | #626 建两端加一跳中继的 Release / Cancel；#631 计划引用计数 PT 扩到中继三方 |
| SYN-5-12 | SYN §五「转发时改写调用链」；M2 §一 | 转发到另一会话时，`path` 里来自其他会话的调用号加上会话标签，标签是目标会话的条目还原，同一会话内的条目保持原样 | 纯逻辑 | TLA + PT | bridge/call-chains-across-processes | `CrossRuntimeSync` `RoutingSound`（`Rebase = TRUE` 的全部配置；`current-norebase` 预期被违反） | 部分 | #631 | 模型的 `RebasePath` 对应 `rpc.rs:684` `rebase`；带随机数的跨进程标签（#325）不在模型里；#631 计划 PT"经 k 次改写后能还原、单射、外来的不会变成本地的" |
| SYN-5-13 | SYN §五、§九「待定」 | 改写后的条目永远对不上对端的调用号：反向调用回到发起它的一方，两边都有 `node:1` 时不走错、不死锁 | 跨进程 | TLA | bridge/call-chains-across-processes | `CrossRuntimeSync` `RoutingSound`、`ChainRecognised` | 已覆盖 | #614 | 模型范围：3 个运行时、链 ≤ 3 跳；#614 按现在的实现更新模型 |
| SYN-6-1 | SYN §六、§十一 M1 | 插件用 `inject` 声明依赖，经 `rows.schema` 报给 rutis，写进这一行的依赖声明，原生门控生效；插件之间的依赖、启动与停止顺序全由 rutis 决定 | 跨进程 | TLA | loader/rows-gate-on-services | — | 未覆盖 | #625 |  |
| SYN-6-2 | SYN §六 | 运行时实现同步等待期间的重入：Python 由 I/O 线程加主线程按调用链执行反向调用 | 跨进程 | — | — | — | 已取代 | — | M2 §二、SYN §九：Python 同步等待期间执行所有进来的调用（可重入） |
| SYN-6-3 | SYN §六 | 提供 `Runtime` 之后、`RuntimeRows` 之前，用 loader 的公开 API（`entries()` + `reload(id)`）重新解析绑在这个运行时上、离线解析的或版本已变的行 | 跨进程 | TLA | loader/rows-gate-on-services | — | 未覆盖 | #625 | #625 `NoProvisional` 与活性"被重启打断的刷新最终重做" |
| SYN-6-4 | SYN §六 | 刷新期间这些行在等 `RuntimeRows`：`reload` 与依赖声明变化引起的 fiber 重建都不让它们启动 | 进程内并发 | TLA + SH | loader/rows-gate-on-services | — | 未覆盖 | #625 |  |
| SYN-7-1 | SYN §七 | Python 运行时需要 Python 3.10 或更高 | 外部 | 测试 | — | — | 不适用 | — | 平台支持矩阵 |
| SYN-7-2 | SYN §七、§十一 M3 | Swift：系统授权（TCC）记在运行时二进制上，它必须有自己的签名和用途说明；只支持 Apple 平台 | 外部 | 测试 | swift/tcc-belongs-to-the-binary | — | 不适用 | — | 操作系统授权，只在本地跑（`docs/ci.md` §12） |
| SYN-7-3 | SYN §七 | Go 不用 `plugin` 包；构建由部署方负责，宿主不临时编译 | 外部 | 测试 | loader/go-runtimes | — | 不适用 | — | `rutis-host dev` 在 Go / Swift 项目里会重新构建（`guide/rutis-host.md:21`），本条只对正常运行成立 |
| SYN-9-1 | SYN §九；M2 §二 | Node 运行时不可重入：同步等待期间只执行 `path` 含自己等待号的进来调用，其余放到一边，栈空后再执行 | 跨进程 | TLA + SH | bridge/sync-wait-cycle-refused | `CrossRuntimeSync` `ChainRecognised`（全部配置）；`session::testing` `reenter` | 部分 | #614 | 模型里的 Node 只有"链上的执行、其余放到一边"；按键扣住、behind、对未完成 Promise 的 await 未建（#614）；JS 帧泵的可控调度在 #630 |
| SYN-9-2 | SYN §九；M2 §二 | Python、Bun 运行时可重入：同步等待期间执行所有进来的调用 | 跨进程 | TLA + SH | bridge/sync-wait-cycle-refused | `CrossRuntimeSync`（`Reentrant` 运行时：`current-reentrant`、`policy-b-*`） | 部分 | #614 | Python 从别的线程（`run_coroutine_threadsafe`）发起的同步调用不带 `sync`，未建（#614）；Python 的可控事件循环在 #630 |
| SYN-9-3 | SYN §九；M2 §二 | 明确不保证：可重入运行时的服务可能在它自己的同步调用中途被调用（"调用 rutis 服务时不要持有锁"无法强制） | 进程内并发 | TLA | — | — | 未覆盖 | #614 ※ | 可像 `NeverCycle` 那样加一条预期被违反的性质，锁定"确实会嵌套" |
| SYN-9-4 | SYN §九；M2 §二 | Node↔Node 交叉同步调用、一个 Node 加两个 Python 的组合可能卡死，只建议用异步方法 | 跨进程 | — | — | — | 已取代 | — | §九「已定」（#228）：这些环由宿主拒绝为 `SyncWaitCycle`（模型反例 1、2；`policy-b-2node`、`policy-b-1node` 通过） |
| SYN-9-5 | SYN §九「已定」 | 宿主检测跨会话的等待环，把卡死变成 `SyncWaitCycle`：在覆盖的范围内没有卡死 | 跨进程 | TLA + K | bridge/sync-wait-cycle-refused | `CrossRuntimeSync` 死锁检查（`policy-b-2node`、`-1node`、`-nonreentrant`、`-depth4`、`-2node-3roots`、`-1node-3roots`、`-2node-seq`、`-posts`、`-posts-node`）；`sync_cycles.rs`（Node、Python、Bun、Go、Swift） | 部分 | #614 | 缺：每个调用一个线程的运行时、宿主自己的同步调用与有限的工作线程、同步分发（`bail`、`waterfall-sync`）、取消；Kotlin 列（#624） |
| SYN-9-6 | SYN §九 | 宿主维护一张宿主范围的表，记录所有已转发、未返回的同步调用（来源与目标的会话和调用号、去标签后的调用链） | 进程内并发 | TLA + TV | bridge/sync-wait-cycle-refused | `CrossRuntimeSync`（变量 `hops`） | 部分 | #614 | 实现（`waits.rs` `Waits`）是否照模型做没有 trace validation（#614 计划） |
| SYN-9-7 | SYN §九 | 返回按读帧顺序登记：同一会话里先读到的返回先于后读到的调用从表里移除 | 跨进程 | TLA + TV | bridge/sync-wait-cycle-refused | `CrossRuntimeSync` `NoFalseCycle`（栈顺序边的前提） | 部分 | #614 | 实现：目标会话的读线程读到返回时调用 `returned`（`waits.rs:135`） |
| SYN-9-8 | SYN §九 | 等待图的调用链边：d 的 `path` 含 c 的调用号时 c 等 d | 跨进程 | TLA + PT | bridge/sync-wait-cycle-refused | `CrossRuntimeSync` 死锁检查与 `NoFalseCycle`（`policy-b-*`） | 已覆盖 | #614 | 实现 `cycle_among` 与模型 `RustSucc` 的差分 PT 在 #614 |
| SYN-9-9 | SYN §九 | 延后边：c 进入一个不可重入、正在同步等待、c 不在其链上的运行时，c 等这个运行时发出的所有同步调用 | 跨进程 | TLA + PT | bridge/sync-wait-cycle-refused | `CrossRuntimeSync`（`policy-b-2node`、`policy-b-nonreentrant`） | 已覆盖 | #614 | 同上 |
| SYN-9-10 | SYN §九 | 栈顺序边：c、d 先后进入同一个运行时且 d 已在那里开始执行，c 等 d | 跨进程 | TLA + PT | bridge/sync-wait-cycle-refused | `CrossRuntimeSync`（`policy-b-1node` 通过；去掉此边的 `policy-bchain-2node` 预期卡死） | 已覆盖 | #614 | 同上 |
| SYN-9-11 | SYN §九「入口」 | 有调用被标记时，没有调用方同步等待的调用（emit 的 `events.call`、异步调用）也记进表，作为入口，不论哪种运行时 | 跨进程 | TLA | bridge/sync-wait-cycle-refused | `CrossRuntimeSync`（`policy-b-posts`、`policy-b-posts-node`：emit 的投递） | 部分 | #614 | 模型只有 emit 的投递，运行时发出的异步调用未建；实现把调用链末尾是已标记调用的异步调用记为一跳而不是入口（`waits.rs:195-203`，854bef5d），与本条不同（问题 1） |
| SYN-9-12 | SYN §九「入口」 | 入口是调用链的边界：它等它之下发出的同步调用，它所在链上更早的调用不等它（带着发出方调用链的 `events.call` 也如此） | 跨进程 | TLA | bridge/sync-wait-cycle-refused | `CrossRuntimeSync` `policy-b-posts` 的 `NoFalseCycle`（入口不作边界时被违反） | 部分 | #614 | 宿主 Rust 插件经 `Bus::emit_erased` 发出的、发回发出方会话的 `events.call` 不带调用链（#614 列为待核对的不一致） |
| SYN-9-13 | SYN §九「入口」 | 单栈的可重入运行时把入口压在正在等待的调用上面执行，栈顺序对入口同样成立 | 跨进程 | TLA | bridge/sync-wait-cycle-refused | `CrossRuntimeSync` `policy-b-posts`（`current-posts` 预期卡死） | 已覆盖 | #614 |  |
| SYN-9-14 | SYN §九「入口」 | 检查的开销与进行中的调用数及调用链长度成线性（"放到一边"那类边除外） | 外部 | 测试 | — | — | 不适用 | — | 性能（`measure_the_check`、`the_check_is_near_linear_in_entries`） |
| SYN-9-15 | SYN §九 | 转发会让等待图成环时不转发，直接向调用方返回 `SyncWaitCycle` | 跨进程 | TLA + K | bridge/sync-wait-cycle-refused | `CrossRuntimeSync`（`policy-b-*`；`policy-b-acts` 违反 `NeverCycle` 确认会触发）；`sync_cycles.rs` | 部分 | #614 | Kotlin 没有 `sync_cycles` 列（#624） |
| SYN-9-16 | SYN §九 | `SyncWaitCycle` 的消息列出环上的调用 | 外部 | 测试 | bridge/sync-wait-cycle-refused | — | 不适用 | — | 文字输出 |
| SYN-9-17 | SYN §九；模型 README | 只在这次转发真会让等待成环时拒绝：调用链上的回调、进入空闲运行时的调用不被拒绝（不误报） | 跨进程 | TLA | bridge/sync-wait-cycle-refused | `CrossRuntimeSync` `NoFalseCycle`（`policy-b-*`） | 部分 | #614 | 模型外的情形（异步调用记为一跳、Go / Swift / Kotlin 本地放弃同步调用不发 `cancel`）可能误报，#614 计划检查 |
| SYN-9-18 | SYN §九 | 只有被拒的这一次调用失败，调用方处理错误后返回，其他调用照常完成 | 跨进程 | TLA + K | bridge/sync-wait-cycle-refused | `CrossRuntimeSync` 死锁检查；`sync_cycles.rs`（"everything else completes"） | 部分 | #614 | 缺口同 SYN-9-5 |
| SYN-9-19 | SYN §九 | 调用帧（`invoke`、`call`、`get`、`await`）的可选字段 `sync: true` 表示调用方在同步等待 | 纯逻辑 | FZ + K | bridge/sync-wait-cycle-refused | `session_frame`（帧的解析与写回含此字段） | 部分 | #631 | 各语言是否在同步调用时都带上它，没有套件检查 |
| SYN-9-20 | SYN §九 | 没有 `sync` 字段的调用按异步处理，不计入等待图 | 跨进程 | — | — | — | 已取代 | — | 同节「入口」把异步调用记为入口；实现把调用链末尾是已标记调用的异步调用记为一跳（`waits.rs:166-205`，854bef5d）。§九 自相矛盾（问题 1） |
| SYN-9-21 | SYN §九「兼容」 | 能力 `sync-wait` 指"同步调用时带 `sync: true`，也接受这个字段"；一方只向声明了它的对端发送这个字段 | 纯逻辑 | PT + K | bridge/sync-wait-cycle-refused | — | 未覆盖 | #631 ※ | `rpc.rs:1558`、`session.mjs:419`、`peer.py:1118`、`peer.go:964`、`Peer.swift:777`；#631 的握手判定表不含"按对端能力决定发不发" |
| SYN-9-22 | SYN §九「兼容」、§九 进展（10-10、10-14） | 各运行时在握手（含 compat）里如实声明：宿主与全部运行时声明 `sync-wait`；Node、Python、Bun 另声明 `sync-stack`；Python、Bun、Go、Swift、Kotlin 声明 `reentrant-sync`；不需要运行时插件的配置 | 跨进程 | K | bridge/sync-wait-cycle-refused | `sync_cycles.rs`（经行为间接检查 Node、Python、Bun、Go、Swift） | 部分 | #624 | Kotlin 列缺（#624）；模型 README"B 实现时需要满足的条件"第 4 条仍写"compat 格式没有能力列表，由运行时插件的配置给出"，与本条矛盾（问题 1） |
| SYN-9-23 | SYN §九「兼容」 | 宿主在 compat 握手里只采纳 `sync-wait`、`sync-stack`、`reentrant-sync` 三项 | 纯逻辑 | — | — | — | 已取代 | — | 还采纳 `events-v2` 与十种 `value-*`（`rpc.rs:765` `COMPAT_DECLARED`，#308、#464） |
| SYN-9-24 | SYN §九「可重入」 | 可重入取自握手能力 `reentrant-sync`，没有声明的按不可重入处理（Node 不声明） | 纯逻辑 | PT | bridge/sync-wait-cycle-refused | — | 未覆盖 | #614 | 实现 `rpc.rs:1318-1329`；#614 的 `cycle_among` 差分用随机 profile，不含能力到 profile 的映射 |
| SYN-9-25 | SYN §九「可重入」、进展 2026-10-14 | "延后""栈顺序"两类边只用于声明了 `sync-stack` 的对端（由对端声明而不是推断）；Rust、Go、Swift、Kotlin 只参与调用链边 | 跨进程 | TLA | bridge/sync-wait-cycle-refused | — | 未覆盖 | #614 | #614 的常量 `Threaded` 与 `policy-b-threaded`；实现 `Profile.stacked`（`rpc.rs:1324`） |
| SYN-9-26 | SYN §九「新旧版本混用」 | 明确不保证：没有声明 `sync-wait` 的运行时（旧版 SDK）发出的调用不计入图，这个会话不在检测范围内、可能卡死；但不因此误报 | 跨进程 | TLA | bridge/sync-wait-cycle-refused | — | 未覆盖 | #614 ※ | 登记表写明"没有测试锁定"；模型里所有运行时都标记同步等待；#614 只对 Python 别的线程那一种"看不见的边"建模 |
| SYN-9-27 | SYN §九「实现」 | 收到带 `sync` 的调用时登记（标记），回复后移除 | 跨进程 | TLA + TV | bridge/sync-wait-cycle-refused | `CrossRuntimeSync`（Rust 一侧的状态） | 部分 | #614 | trace validation 记录 `waits.rs` 的 `marked` / `answered`（#614） |
| SYN-9-28 | SYN §九「实现」 | 转发时调用链最后一项是另一个会话里登记过的调用，就是一跳：先检查再登记 | 跨进程 | TLA + TV | bridge/sync-wait-cycle-refused | `CrossRuntimeSync`（`hops`） | 部分 | #614 | 实现另有三条规格没写的规则：没有已标记调用方的同步调用（宿主自己的）也是一跳（#597，`waits.rs:208-216`）；发回来源会话的不记（:194）；经 `unchained` 发出的不记（:181）（问题 1） |
| SYN-9-29 | SYN §九「实现」 | 没有登记任何调用时，读写这张表只看两个原子计数，不取锁 | 进程内并发 | SH | bridge/sync-wait-cycle-refused | — | 未覆盖 | #628 ※ | 快速路径（`waits.rs:121-123、136-138、181`）与并发的 `marked` 之间的交错没有可控调度检查；#628 的会话清单不含 `waits.rs` |
| SYN-9-30 | SYN §九「实现」 | 会话结束时它的调用全部从表里移除 | 跨进程 | TLA | bridge/sync-wait-cycle-refused | — | 未覆盖 | #614 ※ | 模型不建会话关闭（README"简化"）；#614 只加取消 |
| SYN-9-31 | SYN §九「实现」 | 取消只移除还没开始运行或被异步等待的调用；已经在运行的同步调用保留标记到它的回复 | 跨进程 | TLA | bridge/sync-wait-cycle-refused | — | 未覆盖 | #614 | #614 `MarkedWhileRunning`、配置 `before-cancel` |
| SYN-9-32 | SYN §九「模型覆盖的范围」 | 明确不保证（检测范围外）：Rust 自己发起的同步调用、线程数有限的 Rust 执行器造成的卡死 | 进程内并发 | TLA | bridge/sync-wait-cycle-refused | — | 未覆盖 | #614 | #614 的宿主参与者（`Workers`，`policy-b-workers1` 预期卡死）并要求写成明确不保证；登记表这一行反而写"宿主自己发起、没有调用链的同步调用同样记入等待图"（问题 1） |
| SYN-11-1 | SYN §十一 M1/M2；M2 §三.2 | 任一提供者撤销服务时只有用它的行（各语言的）停下，其他行照常运行 | 跨进程 | TLA + SH | loader/rows-gate-on-services、core/D21 | — | 未覆盖 | #625 | 内核驱逐的精确性在 #622（依赖驱逐，Bug B）；跨运行时在 #625（依赖消失） |
| M2-1-1 | M2 §一「JS/TS 叶子插件」、§一「一致性测试」 | `definePlugin` 定义的 JS/TS 叶子插件由 Node runner 包成 Cordis 插件装进同一个 Node 进程，行为与其他语言的叶子插件相同 | 跨进程 | K | sdk/testing-matches-runtime | `events.expected.json`（只事件部分：`events_with_x_a_node_leaf_plugin`） | 部分 | #639 ※ | 装载、服务、门控的跨语言对照只有普通测试 `multilang.rs` |
| M2-2-1 | M2 §二 | 叶子运行时在挂载时报告 `leaf` 特性 | 跨进程 | K | loader/rows-gate-on-services | — | 未覆盖 | #639 ※ | Python `runner.py:57`；宿主据此决定门控（`rutis-loader/src/runtime.rs:363`） |
| M2-2-2 | M2 §二 | 对报告 `leaf` 的运行时，插件 `inject` 的每个名字都进这一行的 rutis 依赖，不要求 `register_shared` | 纯逻辑 | PT | loader/rows-gate-on-services | — | 未覆盖 | #631 ※ | `rutis-loader/src/runtime.rs:360-368` |
| M2-2-3 | M2 §二 | Node（Cordis）运行时只把登记为共享的名字放进 rutis 依赖，其余交给 Cordis 解析 | 纯逻辑 | PT | loader/rows-gate-on-services | — | 未覆盖 | #631 ※ | 仍是代码的规则（`runtime.rs:367` `is_shared`）；默认值已变：`ServiceCatalog::new()` 共享所有名字（MNT-11-8） |
| M2-2-4 | M2 §二、§三.2 | 提供者比使用者后停：行卸载时先撤销它投到 rutis 的服务（内核先停下使用者），再让运行时卸载插件本身 | 跨进程 | TLA + SH | loader/row-unload-leaves-nothing、core/D14 | — | 未覆盖 | #625 | #625 `WithdrawBeforeUnload`；`Projection::withdraw` 的并发在 #628 |
| M2-2-5 | M2 §二、§四 | 叶子插件没有 volatile 字段：配置变化即重启，`rows.update` 等同于卸载后重新装载 | 跨进程 | K | loader/volatile-in-place | — | 未覆盖 | #639 ※ | Python `runner.py:471`；#625 不建 `rows.update` |
| M2-2-6 | M2 §二 | 按模块名加载的运行时没有包版本可比：resolver 不缓存它的解析结果，每次都问运行时 | 纯逻辑 | PT | loader/reload-all-or-nothing | — | 未覆盖 | #631 | #631"二进制解析器的命名与缓存"只含二进制解析器 |
| M2-2-7 | M2 §二、§四 | 运行时发现模块文件的修改时间或大小变了才重新导入，只重新导入插件模块本身 | 外部 | 测试 | loader/reload-all-or-nothing | — | 不适用 | — | 文件系统时间戳（`runner.py:902`） |
| M2-2-8 | M2 §二 | 每种语言一个 Cargo feature，应用只编译启用的语言；不挂运行时插件就没有进程；CI 分别检查只开一种语言时能编译 | 外部 | 测试 | — | — | 不适用 | — | 构建配置；crate 已从 rutis-interop 并入 rutis-bridge（ee5b605c） |
| M2-2-9 | M2 §二 | 旧的 `Cordis*` 类型名保留为弃用别名 | 纯逻辑 | — | — | — | 已取代 | — | 并入 rutis-bridge 时去掉，crates 里已没有 `CordisRuntimePlugin` 等名字 |
| MNT-2-1 | MNT §2「包级接入」 | npm 项目的锁文件是插件版本的唯一来源；构建不安装 npm 依赖；缺包、版本不符、运行时协议版本不符时构建失败并给出处理命令 | 外部 | 测试 | — | — | 不适用 | — | 构建期、文字输出（`cordis/build.rs:336-358`） |
| MNT-2-2 | MNT §2、§9 | 运行时包在 `package.json` 声明 `rutisProtocol`，必须等于 crate 的 `PROTOCOL`（构建时比对），运行时握手再核对一次 | 纯逻辑 | PT + 测试 | bridge/version-refusal | — | 未覆盖 | #631 | 构建期比对是测试；握手的版本判定属 #631 的握手判定表 |
| MNT-2-3 | MNT §2「底层 API」 | 插件可以是 TypeScript 源文件或已安装的 npm 包目录；包按 `types` 分析、按运行时入口加载 | 外部 | 测试 | — | — | 不适用 | — | 构建期，依赖 TypeScript 检查器 |
| MNT-2-4 | MNT §2「组合挂载」；REQ §1 | `cordis_group` 的一组插件按给定顺序装进同一个 Node 进程的同一个 Context，彼此依赖按 Cordis 原生规则解析 | 外部 | 测试 | — | — | 不适用 | — | Cordis 的语义 |
| MNT-2-5 | MNT §2「组合挂载」 | 组内全部插件的服务都导出到 rutis；同名服务构建时报错；每个插件的配置是组合 `Config` 中以给定名字为名的字段 | 外部 | 测试 | — | — | 不适用 | — | 构建期 |
| MNT-2-6 | MNT §2「服务发现」 | `Service` 子类插件取 Context 声明中类型为它或其基类的成员；函数插件取字面量名的 `ctx.provide` 或包自己声明的 Context 成员 | 外部 | 测试 | — | — | 不适用 | — | 构建期，依赖 TypeScript 检查器 |
| MNT-2-7 | MNT §2 | 配置类型取 `Service` 子类构造函数或 `apply` 的第二个参数；全部字段可选时 `Config` 实现 `Default` | 外部 | 测试 | — | — | 不适用 | — | 构建期 |
| MNT-2-8a | MNT §2「类型映射」 | `number` / `string` / `boolean` / `void` 映射为 `f64` / `String` / `bool` / `()` | 外部 | 测试 | — | — | 不适用 | — | 构建期生成 |
| MNT-2-8b | MNT §2「类型映射」 | 带别名的品牌类型生成同名 newtype（`serde(transparent)`，可 `From<&str>`） | 外部 | 测试 | — | — | 不适用 | — | 构建期生成 |
| MNT-2-8c | MNT §2「类型映射」 | 字符串字面量联合生成同名枚举，变体按原字符串重命名 | 外部 | 测试 | — | — | 不适用 | — | 构建期生成 |
| MNT-2-8d | MNT §2「类型映射」 | 数据接口、对象字面量生成同名结构体，字段 snake_case、按原名序列化 | 外部 | 测试 | — | — | 不适用 | — | 构建期生成 |
| MNT-2-8e | MNT §2「类型映射」「方法」 | 必填可空（`T` 或 `null`）映射为 `Option<T>`，`None` 发送 `null`（字段与参数相同） | 纯逻辑 | PT | — | — | 未覆盖 | #631 ※ | 生成类型的编解码没有 PT；值往返 PT 只在会话层 |
| MNT-2-8f | MNT §2「类型映射」「方法」 | `T` 或 `undefined`、可选字段与参数映射为 `Option<T>`，`None` 发送 `undefined`（字段省略） | 纯逻辑 | PT | — | — | 未覆盖 | #631 ※ | 同上 |
| MNT-2-8g | MNT §2「类型映射」 | 可选且可空映射为 `Option<Option<T>>`：`None` 省略、`Some(None)` 发 `null`；解码时区分缺失与 `null` | 纯逻辑 | PT | — | — | 未覆盖 | #631 ※ | 同上 |
| MNT-2-8h | MNT §2「类型映射」 | 数组、只读数组映射为 `Vec<T>`，参数位置借用为 `&[T]`；`Record<string, T>` 映射为 `BTreeMap<String, T>` | 外部 | 测试 | — | — | 不适用 | — | 构建期生成 |
| MNT-2-8i | MNT §2「类型映射」 | `any`、`unknown` 与其他纯数据联合映射为 `serde_json::Value`，数据原样过线；值里有活对象或函数时解码报错，不把内部引用标记当数据返回 | 纯逻辑 | PT | bridge/value-tags | — | 未覆盖 | #631 | #631"serde 标记：带 `\0rutis:` 前缀键与嵌套引用的随机 JSON 往返" |
| MNT-2-8j | MNT §2「类型映射」 | 活对象的联合映射为 `ObjectRef`：引用保留，按需包进某个成员的代理 | 跨进程 | K | — | — | 未覆盖 | #639 ※ | 只有 Node 导出对象引用 |
| MNT-2-8k | MNT §2「类型映射」 | 活对象与数据的联合生成 `untagged` 枚举，引用变体在前，多个活对象成员共用一个 `Object(ObjectRef)` 变体 | 纯逻辑 | PT | — | — | 未覆盖 | #631 ※ |  |
| MNT-2-8l | MNT §2「类型映射」 | 活对象生成代理结构体：数据属性的 getter 每次读实时值；两个代理指向同一对象时相等；传回 Cordis 时还原为原对象 | 跨进程 | K + PT | — | — | 未覆盖 | #639 ※ | 身份规则见 REQ-4-3 |
| MNT-2-8m | MNT §2「类型映射」 | 函数参数（回调）映射为 `impl Fn`，回调返回 Promise 时为 `BoxFuture`；Cordis 可以立即调用，也可以保存后再调用 | 跨进程 | K | — | `session::testing`（`apply` 立即回调；`hold` / `fire` 保存后调用） | 部分 | #639 ※ | 套件按会话层检查；生成代码的 `BoxFuture` 回调没有 |
| MNT-2-8n | MNT §2「类型映射」 | 返回的函数映射为 `RemoteFunction`：`call` 同步调用，`call_async` 等返回的 Promise，函数留在 Cordis 侧 | 跨进程 | K | — | — | 未覆盖 | #639 ※ |  |
| MNT-2-8o | MNT §2「类型映射」 | JS `Error` 值映射为 `JsError { name, message, stack }` | 纯逻辑 | PT | bridge/error-code-and-data | — | 未覆盖 | #631 | #631 的错误图 FZ |
| MNT-2-9 | MNT §2「方法」 | 同步方法生成同步方法，返回 Promise 的生成 `async fn`，都返回 `Result<T, Error>` | 外部 | 测试 | — | — | 不适用 | — | 构建期生成；运行时的方法形状见 REQ-3-3 |
| MNT-2-10 | MNT §2「方法」 | 生成代码的内部变量带 `__rutis_` 前缀，不与插件的参数名冲突 | 外部 | 测试 | — | — | 不适用 | — | 构建期生成 |
| MNT-2-11 | MNT §2「方法」；REQ §7 | `AbortSignal` 参数不出现在 Rust 签名里：Cordis 方法收到真实的 `AbortSignal`，丢弃返回的 future 即取消调用并中止这个信号 | 跨进程 | K + TLA | bridge/cancel-reaches-callee | `session::testing`（"Dropping an async call aborts the far end's signal"，7 种实现） | 部分 | #626 | 一跳直连由套件覆盖；与回复、释放、关闭的交错及经中继两跳在 #626 `CancelReaches` |
| MNT-2-12 | MNT §2「方法」 | 明确不保证：同步方法无法中途取消，它收到的信号永远不会中止 | 跨进程 | K | — | — | 未覆盖 | #639 ※ |  |
| MNT-2-13 | MNT §2「方法」 | 选项对象里的 `AbortSignal` 字段暂不支持，不传 | 外部 | 测试 | — | — | 不适用 | — | 构建期生成 |
| MNT-2-14 | MNT §2「不支持的成员」、§10 | 不支持的成员（重载、泛型方法、可选回调、`Uint8Array`、流、JS 内置类型等）不生成；构建时 `cargo:warning` 报出位置和原因并列进文档注释，其他成员照常生成 | 外部 | 测试 | — | — | 不适用 | — | 构建期输出 |
| MNT-3-1 | MNT §3 | 一个双向 Unix socket，逐行 JSON，协议版本 1 | 跨进程 | — | bridge/version-refusal | — | 已取代 | — | 本机运行时协议版本为 2（`session/mod.rs:12`、`rutisProtocol` 2），endpoint 格式为 3；通道可以是 Unix socket、`fd:3`、loopback TCP、WebSocket |
| MNT-3-2 | MNT §3 | 每帧一行 JSON | 纯逻辑 | FZ | — | `local_lines` | 部分 | #631 | `local_lines` 检查分帧不 panic；帧内容的有状态 fuzz 在 #631 |
| MNT-3-3 | MNT §3 | `hello { version }` 握手：完成前不接受其他帧 | 跨进程 | PT + K | bridge/version-refusal | — | 未覆盖 | #631 | `session::testing::malformed` 在握手之后才发畸形帧；#631 握手判定表 |
| MNT-3-4 | MNT §3 | `invoke { id, path, target, method, args }`：`target` 为句柄时调用服务方法，为空时是控制操作 | 跨进程 | K + FZ | — | `session::testing`（`invoke`）；`session_frame` | 部分 | #631 | 控制操作（`target` 为空）不在套件里 |
| MNT-3-5 | MNT §3 | `call { id, path, reference, method?, args }`：调用对端传来的函数引用，带 `method` 时调用对象引用的方法 | 跨进程 | K + FZ | — | `session::testing`（`apply`、`hold` / `fire`）；`session_frame` | 部分 | #631 | 对象方法只有 Node 导出对象，套件不测 |
| MNT-3-6 | MNT §2、§3 | `get { id, path, reference, property }` 实时读取对象引用的属性；服务自身的属性经 `get(句柄, 属性)` 实时读取 | 跨进程 | K + FZ | — | `session_frame` | 部分 | #639 ※ | 套件没有 `get` 的用例 |
| MNT-3-7 | MNT §3、§3.1 | `await { id, path, reference }`：等待对端传来的异步结果 | 跨进程 | K | — | `session::testing`（`later`） | 已覆盖 | — |  |
| MNT-3-8 | MNT §3 | `return { id, value }` / `throw { id, error }` 回复调用 | 跨进程 | K | — | `session::testing`（`echo`、`fail`） | 已覆盖 | #626 | 恰好结束一次见 #626 `SettlesOnce` |
| MNT-3-9 | MNT §3、§3.1 | `cancel { id }`：中止被调方收到的 `AbortSignal`，或停止等待异步结果（Rust 侧收到时停止对应的异步等待） | 跨进程 | TLA + K | bridge/cancel-reaches-callee | `session::testing`（取消中止信号；含 Rust 作被调方的 `rust_meets_the_session_contract`） | 部分 | #626 | 取消之后被调方要不要回复没有成文规则（#626） |
| MNT-3-10 | MNT §3「值」 | 值：`undefined`、JSON 数据、列表、记录、引用；数据按值往返 | 跨进程 | PT | bridge/value-roundtrip | `value_roundtrip.rs` `*_values_round_trip`（Rust、Node、Python、Go、Swift、Bun、Kotlin） | 已覆盖 | — | 例外见 bridge/value-limits |
| MNT-3-11 | MNT §3「Rust 侧解码」 | 解码时对象引用先替换为标记，再由 `ObjectRef` 反序列化取回；看起来像标记的数据仍是数据 | 纯逻辑 | PT | bridge/value-tags | — | 未覆盖 | #631 | #631"serde 标记"；现在只有单元测试 `objects.rs::data_shaped_like_a_marker_stays_data` |
| MNT-3-12 | MNT §3「引用计数」 | 发送方每授予一次加一；接收方先登记持有再排队执行；本地代理全部释放后按收到的次数 `release`；旧的 release 与新的授予交叉时不误删 | 跨进程 | PT + TLA | bridge/release-count、bridge/reference-release-count | `value_roundtrip.rs` `*_releases_balance`（Rust、Node、Python、Swift；顺序、两方、静止时检查） | 部分 | #626 | #626 `PinnedTarget`；Go、Bun、Kotlin 列在 #624；并发与中继在 #631 |
| MNT-3-13 | MNT §3「引用计数」 | 会话关闭时整张引用表清空，不等待 GC | 跨进程 | TLA + SH | — | — | 未覆盖 | #626 | #626 `BoundedTables`（静止时为空）；#628 的 `close()` 模型 |
| MNT-3-14 | MNT §3「错误」；REQ §4 | 错误以对象图传输，保留 `name`、`message`、`cause`、AggregateError 的 `errors` 及其顺序、共享和循环引用 | 跨进程 | PT + FZ | bridge/error-code-and-data | `session::testing`（只查 `name`、`message`） | 部分 | #631 | #631 只计划错误图的 FZ（大小与时间上限）；`cause`、`errors`、共享与循环的往返没有 PT |
| MNT-3-15 | MNT §3「调用链」 | 调用号为 `rust:n` / `node:n`，`path` 记录嵌套调用链，用于把反向调用交给正在同步等待的一方 | 跨进程 | TLA + K | bridge/call-chains-across-processes | `CrossRuntimeSync` `RoutingSound`；`session::testing`（`apply`、`reenter`） | 已覆盖 | #614 | endpoint 格式用端点 id 作调用号（M1） |
| MNT-3.1-1 | MNT §3.1 | 生成的同步方法在调用线程上同步等待；等待期间同一调用链的反向调用在等待线程上执行，嵌套同步回调可以工作 | 跨进程 | TLA + K | — | `CrossRuntimeSync` `RoutingSound`；`session::testing`（`apply`、`reenter`，7 种实现） | 已覆盖 | — |  |
| MNT-3.1-2 | MNT §3.1 | 异步方法返回 Future（Node 返回 Promise）；调用与等待是两个操作，`invoke` 的异步结果以引用传回、需要时再 `await` | 跨进程 | K | — | `session::testing`（`later`） | 已覆盖 | — |  |
| MNT-3.1-3 | MNT §3.1、§10 | 同步调用链需要的结果只能由被它阻塞的 Rust `current_thread` 运行时推进时，返回 `SyncWaitCycle` 而不卡死 | 进程内并发 | SH | boundary/5.6 | — | 未覆盖 | #628 | #628 只查 `ready_on_blocked_executor` 的 `FutureState`；`rpc_callbacks.rs` 是普通测试 |
| MNT-3.1-4 | MNT §3.1 | `Connection::independent_future` 把与调用方执行器无关的异步工作放到每连接一个的后台执行器；只有显式声明的工作才会放过去 | 进程内并发 | SH | — | — | 未覆盖 | #628 ※ |  |
| MNT-3.1-5 | MNT §3.1 | 异步调用的 future 在完成前被丢弃时发送 `cancel` | 跨进程 | K + TLA | bridge/cancel-reaches-callee | `session::testing`（取消中止信号） | 已覆盖 | #626 |  |
| MNT-3.1-6 | MNT §3.1 | 被调方为带 `signal` 参数的调用创建 AbortController，调用返回 Promise 时保留到它结束，所以等待阶段的取消也能中止原调用的信号 | 跨进程 | K | bridge/cancel-reaches-callee | `session::testing`（`abortable` 返回 future、开始后丢弃、查 `aborted`，7 种实现） | 已覆盖 | — |  |
| MNT-3.1-7 | MNT §3.1 | 已取消调用的迟到应答被丢弃并计入 `Connection::orphans()`，不当作协议错误而断开连接 | 跨进程 | TLA | bridge/cancel-reaches-callee | — | 未覆盖 | #626 | #626 `NoUnknownCall` |
| MNT-4.1-1 | MNT §4.1 | 槽位第一次出现的对象句柄就是服务名，之后每换一个对象分配新句柄（`name#2`…）；句柄永远指向创建时的对象，方法调用按句柄定位、不重读槽位 | 跨进程 | K | boundary/5.1 | — | 未覆盖 | #639 ※ | 叶子运行时的槽位规则没有对照 |
| MNT-4.1-2 | MNT §4.1 | 槽位变化时被调方经控制调用 `service(name, handle 或 null, version)` 通知 Rust；`version` 保证旧通知不覆盖新通知 | 跨进程 | TLA | boundary/5.1、loader/announced-services | — | 未覆盖 | #625 | #625 `NewerWins` |
| MNT-4.1-3 | MNT §4.1 | 服务是否导出由 Cordis 原生门控决定（提供者状态、`Service.check()`），`check` 不通过时不导出 | 外部 | 测试 | — | — | 不适用 | — | Cordis 的语义 |
| MNT-4.1-4 | MNT §4.1 | 服务方法经调用方 Context 创建的 effect 属于导出 fiber，随它卸载清理 | 外部 | 测试 | — | — | 不适用 | — | Cordis 的语义 |
| MNT-4.1-5 | MNT §4.1 | 判断换值时先用 `Symbol.for('cordis.original')` 取原对象再比较：只读调用不产生新句柄 | 外部 | 测试 | boundary/5.1 | — | 不适用 | — | Cordis 的追踪代理 |
| MNT-4.2-1 | MNT §4.2 | 槽位第一次可用时注册代理并保留 `ServiceWriter`；换成新对象时用它替换，已取得的 `Arc` 快照仍指向旧对象 | 进程内并发 | SH | boundary/5.1 | — | 未覆盖 | #628 | #628 的 `Projection` 模型 |
| MNT-4.2-2 | MNT §4.2、§6 | 槽位变为不可用时撤销注册，依赖它的 rutis 插件按原生规则停止；再次可用时重新注册，依赖方重新启动 | 跨进程 | TLA + SH | boundary/5.1、core/D14 | — | 未覆盖 | #625 |  |
| MNT-4.2-3 | MNT §4.2；REQ §4 | 通知在同步调用链内到达时在调用返回前处理（`swap` 返回后已是新代理）；明确不保证：异步路径允许短暂读到旧代理 | 跨进程 | TLA | boundary/5.1 | — | 未覆盖 | #625 ※ | #625 的服务公告不建"在调用链内、返回前"的先后 |
| MNT-4.2-4 | MNT §4.2 | 发布在锁外执行，重入或并发的变化交给正在进行的发布循环 | 进程内并发 | SH | — | — | 未覆盖 | #628 | #628"静止时每个槽应用的句柄是最新的" |
| MNT-4.2-5 | MNT §4.2 | 撤销完成之后才允许重新注册 | 进程内并发 | SH | boundary/5.1 | — | 未覆盖 | #628 | #628"`withdraw` 在发布方空闲后才返回"；#373 修复前能被抓到 |
| MNT-4.2-6 | MNT §4.2 | 发布失败不记为已应用，槽位保持待发布，下一次变化时重试 | 进程内并发 | SH | — | — | 未覆盖 | #628 |  |
| MNT-4.2-7 | MNT §4.2 | 正在发布中的句柄不会因为重入的换值被提前释放 | 进程内并发 | SH | — | — | 未覆盖 | #628 | #628"每个句柄恰好释放一次" |
| MNT-4.3-1 | MNT §4.3 | 代理的最后一个 `Arc` 释放时发 `release(handle)`；被调方只在句柄既被释放、又不再是槽位当前对象时删除它 | 跨进程 | TLA + SH | — | — | 未覆盖 | #628、#639 ※ | 宿主一侧"恰好释放一次"在 #628；被调方的删除条件没有计划 |
| MNT-4.3-2 | MNT §4.3 | 还没来得及生成代理就被替换掉的句柄由 `Projection` 直接释放 | 进程内并发 | SH | — | — | 未覆盖 | #628 |  |
| MNT-4.3-3 | MNT §4.3 | 挂载插件卸载时 `Projection::close()` 丢弃发布闭包，断开 Process → Projection → ServiceWriter → 代理 → Process 的环，`Process` 能被回收 | 进程内并发 | SH + 测试 | — | — | 未覆盖 | #628 | #628"关闭后不再应用"；回收本身是残余检查 |
| MNT-5-1 | MNT §5「声明」 | `.provide(名字)` 声明宿主服务，接口取自插件能看到的 Context 声明；找不到声明或与组内服务同名时构建报错 | 外部 | 测试 | — | — | 不适用 | — | 构建期 |
| MNT-5-2 | MNT §5「生成物」 | 生成 trait `{接口}Host`：同步方法为 `fn`，Promise 方法返回 `BoxFuture`；每个方法默认返回"宿主未实现"错误 | 外部 | 测试 | — | — | 不适用 | — | 构建期生成 |
| MNT-5-3 | MNT §5「生成物」 | 挂载插件的 `injects` 含这些宿主服务：rutis 侧未就绪时挂载插件按原生规则等待 | 进程内并发 | SH | core/D22 | — | 未覆盖 | #622 | 内核依赖门控；#622 的依赖驱逐与 dispose 模型 |
| MNT-5-4 | MNT §5「类型」 | 函数类型的参数以引用传入，宿主可以调用；宿主返回 `rpc::Value::callback` 时 Cordis 侧拿到可调用的函数 | 跨进程 | K | — | `session::testing`（回调参数 `apply`） | 部分 | #639 ※ | "宿主返回函数"只在 PT 的 `make()` 里用到，套件不调用它 |
| MNT-5-5 | MNT §5「类型」 | 联合类型中的函数成员不绑定，只绑定数据成员并给构建警告；Cordis 侧调用未绑定的成员时明确报错"未由 rutis 宿主提供" | 外部 | 测试 | — | — | 不适用 | — | 构建期与错误文字 |
| MNT-5-6 | MNT §5「Node 侧」 | runner 在装载插件之前用 `ctx.provide(名字, 代理)` 注册代理，代理的方法经协议调用 Rust（控制目标 `host:{服务名}`） | 跨进程 | TLA | loader/rows-gate-on-services | — | 未覆盖 | #625 | 先注册代理、后装载插件；行模式下对应 `hosts.provide` 先于 `rows.load`（#625 租约） |
| MNT-5-7 | MNT §5「变化与清理」 | rutis 侧宿主服务撤销时挂载插件按原生规则停止、Node 进程随之关闭，重新提供后重启；不做单个宿主服务的就地替换 | 进程内并发 | SH | core/D14 | — | 未覆盖 | #622 | 静态挂载；行模式见 SYN-5-10 |
| MNT-5-8 | MNT §5「变化与清理」 | 宿主服务的提供者在依赖方清理完之后才撤掉：挂载插件清理时先卸载 Cordis 插件，插件 effect 调用宿主返回的注销函数时宿主仍在 | 跨进程 | TLA + SH | core/D14、loader/row-unload-leaves-nothing | — | 未覆盖 | #625 | #625 `LeaseWithinRow` 对行；静态挂载靠内核 D14（#622） |
| MNT-5-9 | MNT §5「变化与清理」 | JS 同步调用宿主服务时 Node 主线程同步等待 Rust 返回，沿用同步规则与 `SyncWaitCycle` 检测 | 跨进程 | TLA | boundary/5.6、bridge/sync-wait-cycle-refused | — | 未覆盖 | #614 | 进入 Rust 服务的调用在 `waits.rs` 里不是一跳；#614 把宿主作为参与者建模 |
| MNT-5-10 | MNT §5「已知差异」 | 明确不保证：插件在 effect 之外注册并丢弃注销函数时，宿主不知道调用方插件的生命周期，需要自己撤销（最迟在挂载卸载时） | 外部 | 测试 | — | — | 不适用 | — | 宿主应用的义务；行模式下可用调用方行（MNT-11-9a） |
| MNT-6-1 | MNT §6「启动」 | 挂载插件启动 Node 进程，握手后发 `mount`；Node 侧加载原插件并等 `fiber.await()`；原插件启动失败时挂载失败，不注册任何服务 | 跨进程 | TLA | — | — | 未覆盖 | #625 | #625 的"失败的装载归还租约"对行；静态挂载的 `mount` 不在模型里 |
| MNT-6-2 | MNT §6「启动」 | runner 使用插件自己解析到的 Cordis（`Service` 子类与 Context 来自同一模块实例）；入口可导出 `apply` 或默认导出 `Service` 子类 | 外部 | 测试 | — | — | 不适用 | — | npm 模块解析 |
| MNT-6-3 | MNT §6「启动」 | 一个 Node 进程只运行这次挂载的插件；组内满足不了的必需依赖让挂载直接失败，错误列出哪个插件缺哪些服务 | 外部 | 测试 | — | — | 不适用 | — | 错误文字。"rutis 侧也暂不能为它们提供服务"与 §5 的宿主服务矛盾（问题 6） |
| MNT-6-4 | MNT §6「清理顺序」；REQ §4 | 挂载插件先登记清理 effect、再注册服务；rutis 逆序清理：先撤服务、执行消费者的 disposer（此时仍可调用远端服务），最后关闭 Node 进程 | 进程内并发 | SH | core/D14、core/D6 | — | 未覆盖 | #622 ※ | #622 的 dispose 模型不查 effect 与 provide 之间的后进先出顺序 |
| MNT-6-5 | MNT §6「先清理后排空」 | `dispose` 同时启动插件卸载和在途调用排空，不先等调用结束 | 跨进程 | TLA | — | — | 未覆盖 | #626 ※ | #626 建 Close，不建 dispose 与排空的并行 |
| MNT-6-6 | MNT §6「故障」、§9；REQ §6、§8 | 进程退出或连接断开时，所有在途调用和后续调用都以 `Transport` 错误失败：不重试、不重放、不返回默认值，已发送未返回的调用结果未知 | 跨进程 | TLA + K | bridge/in-flight-call-fails-once、bridge/no-call-on-a-dead-session | — | 未覆盖 | #626 | #626 `SettlesOnce`（关闭错误也算结束） |
| MNT-6-7 | MNT §6「故障」；REQ §7 | `Transport` 错误说明进程如何结束（退出状态或信号），`Process::exit_status()` 也可读取 | 外部 | SH + 测试 | — | — | 未覆盖 | #623 | #623"通道的结束说明进程怎样结束，不报 Broken pipe"；文字本身是测试 |
| MNT-6-8 | MNT §6「崩溃后的服务」；REQ §7 | 连接断开后挂载投影的服务全部撤销，依赖它们的 rutis 插件停止并等待；挂载插件本身保持 Active、不再提供服务；是否重新挂载由应用决定 | 跨进程 | TLA | boundary/5.8 | — | 未覆盖 | #625 | #625 的运行时崩溃动作（行模式）；静态挂载不在模型里 |
| MNT-6-9 | MNT §6「崩溃后的服务」 | 进程退出状态由独立线程查看（不回收进程），`current_thread` 运行时被同步调用阻塞时错误也带上退出状态 | 进程内并发 | SH | — | — | 未覆盖 | #623 |  |
| MNT-7-1 | MNT §7 | `Bindings::event` 选择要从 Cordis 转发到 rutis 的事件，每个生成实现 `rutis::Event` 的结构体 | 跨进程 | — | — | — | 已取代 | — | 0.13：监听器自己登记；`Bindings` 已无 `event` / `emit`（`cordis/build.rs:56-105`），清单的 `events` 字段被拒 |
| MNT-7-2 | MNT §7 | 只转发返回 `void` 的通知事件，waterfall、bail 等在构建时报错 | 跨进程 | — | — | — | 已取代 | — | 统一事件第三节：五种方式跨边界与进程内同义 |
| MNT-7-3 | MNT §7 | `internal/*` 事件不转发 | 跨进程 | — | — | — | 已取代 | — | 见 SYN-2-6 |
| MNT-7-4 | MNT §7 | 转发监听在原插件启动后登记；Cordis `emit` 忽略它的 Promise（发出即忘），`parallel` / `serial` 等它 | 跨进程 | — | boundary/5.2 | — | 已取代 | — | EVD 第三节 |
| MNT-7-5 | MNT §7 | Rust 侧把参数解码成事件类型后用 `parallel` 从挂载插件的 Context 发出 | 跨进程 | — | — | — | 已取代 | — | EVD 第三节 |
| MNT-7-6 | MNT §7 | 顺序：启动期间登记的监听 → rutis 一组 → 之后登记的；原插件启动期间发出的事件不转发 | 跨进程 | — | boundary/5.3 | — | 已取代 | — | 统一事件：所有监听器一张表、按登记顺序 |
| MNT-7-7 | MNT §7 | `Bindings::emit` 选择由 rutis 发往 Cordis 的通知事件；`EmitToCordis` 监听属于挂载插件、随它卸载 | 跨进程 | — | — | — | 已取代 | — | EVD 第三节 |
| MNT-7-8 | MNT §7 | runner 用 `parallel` 发出，只接受挂载时声明过的事件名；rutis `parallel` 等 Cordis 监听处理完，`emit` 发出即忘 | 跨进程 | — | — | — | 已取代 | — | EVD 第三节 |
| MNT-7-9 | MNT §7 | 同一事件名只能选一个方向，两个方向都选时构建报错（防回环） | 外部 | — | — | — | 已取代 | — | 0.13 删除事件选择 |
| MNT-8-1 | MNT §8；REQ §1、§8 | 反方向（Cordis 应用挂载 rutis 插件）冻结：保留代码和测试，不再增加能力 | 外部 | 测试 | — | — | 不适用 | — | 工程约束；`rutis_plugin` 仍在（`cordis/build/rust.rs:662`） |
| MNT-8-2 | MNT §8 | `rutis_plugin` 只支持单文件中的公开具体插件、`&self` 方法和基本类型；遇到模块声明、宏、泛型、借用、公开字段时报错 | 外部 | 测试 | — | — | 不适用 | — | 构建期 |
| MNT-8-3 | MNT §8 | 明确不保证：反方向导出的对象在挂载时捕获，不跟随换值 | 外部 | 测试 | — | — | 不适用 | — | 冻结的代码 |
| MNT-9-1 | MNT §9 | 帧走独立的通道：插件向 stdout 打印不会破坏帧流 | 外部 | 测试 | — | — | 不适用 | — | 操作系统的描述符 |
| MNT-9-2 | MNT §9 | 目前只有一个协议版本，不做多版本协商 | 跨进程 | — | bridge/cross-version-meetings | — | 已取代 | — | compat（2）与 endpoint（3）两种格式并存（`session/mod.rs:9-18`） |
| MNT-10-1 | MNT §10 | Cordis `emit` 返回时 rutis 监听已执行完：做不到，采用发出即忘 | 跨进程 | — | boundary/5.2 | — | 已取代 | — | EVD 第三节 |
| MNT-10-2 | MNT §10 | 跨侧逐个交错的监听顺序：做不到，按组转发 | 跨进程 | — | boundary/5.3、boundary/5.4 | — | 已取代 | — | EVD 第三节 |
| MNT-10-3 | MNT §10 | 返回值含义不同的同名事件契约按签名显式转换，无法转换时报不兼容 | 跨进程 | — | — | — | 已取代 | — | REQ §4：0.13 起 `false` 与 `Some(false)` 都是没有结果 |
| MNT-10-4 | MNT §3「值」、§10；M2 §四 | 明确不保证：Rust 侧的对象不能以引用交给 Node / Python（只有 Node 导出对象引用）；宿主服务可以返回从 Cordis 取得的对象 | 跨进程 | K | bridge/grant-rollback | — | 未覆盖 | #639 ※ | Node、Python 不声明 `objects`（`session.mjs:60`、`peer.py:43`）；不能跨的值应显式失败（bridge/grant-rollback） |
| MNT-11-1 | MNT §11「声明」 | `build.rs` 用 `Exports::new().service(name, path)` / `.service_in(name, path, group)` 列出要导出的 Rust 服务，路径可以是依赖 crate 的类型 | 外部 | 测试 | bridge/exported-services | — | 不适用 | — | 构建期 API；之后只用于改不了源码的类型（MSV R10） |
| MNT-11-2 | MNT §11「读源码」 | 构建脚本用 `cargo metadata` 与 syn 按 rustc 规则解析名字（mod、use、glob、pub use、别名、prelude、cfg）；看不到宏写出的条目 | 外部 | 测试 | bridge/exported-services | — | 不适用 | — | 构建期，依赖 cargo |
| MNT-11-3 | MNT §11 | 生成的 dispatcher 不写参数、结果或错误的类型，由类型推断按真实签名定型；读错源码是编译错误而不是调错 | 外部 | 测试 | bridge/exported-services | — | 不适用 | — | 编译期保证 |
| MNT-11-4 | MNT §11「提供」 | 导出的生命周期就是原生服务的：加载器在 `host_key(name)`（实例里 `host_key_in`）下提供 dispatcher；行按内核规则等它、跟随替换、随它撤下而停止 | 进程内并发 | SH | bridge/exported-services、core/D14 | — | 未覆盖 | #622 | 内核依赖驱逐（#622） |
| MNT-11-5 | MNT §11「提供」 | 导出是普通的 `host_key` 服务：节点、Rust 插件和行的 `check` 都把它当作应用自己的服务 | 纯逻辑 | PT | loader/check | — | 未覆盖 | #629 | #629 的 `check` 与 reconcile 的差分 |
| MNT-11-6 | MNT §11「方法都是 async」 | 对调用方每个导出方法都是 `async`：慢方法不阻塞运行时的线程，也不形成同步等待环；可被 `AbortSignal` 或丢弃 future 取消 | 跨进程 | TLA + K | bridge/exported-services、bridge/cancel-reaches-callee | — | 未覆盖 | #614 | "不形成同步等待环"要靠宿主参与者的建模（#614） |
| MNT-11-7a | MNT §11「错误」；MSV R4 | `Err(e)` 变成会话的失败，错误图根节点带 `code` 和 `data`；不认识它们的解码方仍拿到名字和消息；各运行时读出并发送它们，再次抛出时保留 | 跨进程 | PT + K | bridge/error-code-and-data | — | 未覆盖 | #631 | 跨语言保留只有普通测试 |
| MNT-11-7b | MNT §11「错误」 | `code` 的取值：E 有 serde 时取 serde 写出的变体名，否则取 Debug 里的变体名，再否则取类型名 | 纯逻辑 | PT | bridge/error-code-and-data | — | 未覆盖 | #631 | #631"错误码与 build.rs 生成器做差分" |
| MNT-11-8 | MNT §11「默认按名字共享」 | `ServiceCatalog::new()` 按名字共享所有名字，只有 rutis 一侧有提供者的名字才由 rutis 门控行；Cordis 行提供的服务自动投到 rutis；`share_registered_only()` 是原来的行为 | 纯逻辑 | PT | loader/rows-gate-on-services | — | 未覆盖 | #631 ※ |  |
| MNT-11-9a | MNT §11「调用方行的 Ctx」 | `&Ctx` 参数对调用方不是参数：dispatcher 在 `invoke` 运行时（返回 future 之前）读 `caller_row()`，传入发起调用那一行的上下文 | 跨进程 | TLA + PT | bridge/caller-row | — | 未覆盖 | #631 | 行的身份靠按行的租约作用域（`process.rs:1233`、`row_label`）；#631 的作用域 id PT 含 U+0001 碰撞疑点（问题 3） |
| MNT-11-9b | MNT §11「调用方行的 Ctx」 | 方法在调用方 `Ctx` 上登记的东西（effect、监听）随该行卸载或重启而移除，运行时照常运行 | 进程内并发 | SH | bridge/caller-row | — | 未覆盖 | #622 ※ |  |
| MNT-11-9c | MNT §11「调用方行的 Ctx」 | 不在任何行里的调用（Rust、对端节点）被拒绝，错误写明该方法需要调用方行的上下文 | 纯逻辑 | 测试 | bridge/caller-row | — | 不适用 | — | 错误文字；判定是单分支 |
| REQ-1-1 | REQ §1 | Cordis 插件仍在真实的 Cordis 中运行、源码不改；rutis 侧通过构建时生成的 Rust 类型按原生方式使用 | 外部 | 测试 | — | — | 不适用 | — | 端到端（`tests/e2e/tests/cordis.rs`、`examples/dsh-baseline`） |
| REQ-1-2 | REQ §1、§6 | 依赖是双向的：rutis 插件可依赖被挂载插件的服务，被挂载的 Cordis 插件可按原生方式（`inject`、`ctx.xxx`）依赖 rutis 的服务；未就绪时等待，撤销时按原生规则停止，两个方向各守自己框架的门控与清理顺序 | 跨进程 | TLA | loader/rows-gate-on-services、core/D14 | — | 未覆盖 | #625 |  |
| REQ-3-1 | REQ §3；MNT §1 | rutis 内核不为兼容修改、只用公开 API；Cordis 只用公开 API（含 `internal/*`），不改源码、不 fork、不 monkey patch | 外部 | 测试 | — | — | 不适用 | — | 工程约束，由评审保证 |
| REQ-3-2 | REQ §3 | 原 TS 插件与 rutis 消费者的业务源码不因跨进程改写；类型绑定在普通 Cargo 构建时自动生成，不手写契约、不维护 IDL | 外部 | 测试 | — | — | 不适用 | — | 工程约束 |
| REQ-3-3 | REQ §3、§4 | 方法形状是接口契约：同步方法仍是同步方法，异步方法仍返回 Future | 跨进程 | K | loader/announced-services | — | 未覆盖 | #639 ※ | 生成器（MNT-2-9）与运行时（未声明即 `async`）两处；没有跨语言套件 |
| REQ-4-1 | REQ §4 | 框架契约：服务方法的参数与返回值、业务错误（含 AggregateError 的 `errors` / `cause`）跨边界保留 | 跨进程 | PT | bridge/value-roundtrip、bridge/error-code-and-data | `value_roundtrip.rs` `*_values_round_trip`（数据部分） | 部分 | #631 | 错误图没有 PT，见 MNT-3-14 |
| REQ-4-2 | REQ §4 | 框架契约：插件的装配、依赖就绪、卸载清理；卸载期间仍可调用服务 | 跨进程 | TLA + SH | core/D14、loader/row-unload-leaves-nothing | — | 未覆盖 | #625 |  |
| REQ-4-3 | REQ §4；MNT §3「值」 | 对象身份：已取得的服务对象、回调、异步结果不会被替换成别的对象或快照；同一个对象无论出现几次都是同一个引用 | 跨进程 | PT | — | — | 未覆盖 | #631 ※ | 引用计数 PT 不检查身份 |
| REQ-4-4 | REQ §4 | 事件返回值的含义：`false` 与 `Some(false)` 都是没有结果 | 跨进程 | — | — | — | 已取代 | — | EVD 第三节，归事件分区 |
| REQ-4-5 | REQ §4 | `emit` 按登记顺序开始每个监听器后立即返回，Node 运行时的例外 | 跨进程 | — | — | — | 已取代 | — | EVD 第三节、EVP §5.5 |
| REQ-4-6 | REQ §4 | 明确不保证：JS async 函数调用时立即执行同步前半段，跨边界不保证 | 跨进程 | 测试 | — | — | 不适用 | — | 明确不保证，没有要证明的性质 |
| REQ-4-7 | REQ §4 | 所有监听器在一张表里按登记顺序逐个执行，不分侧 | 跨进程 | — | — | — | 已取代 | — | EVD 第三节 |
| REQ-4-8 | REQ §4 | 同步 `bail` 遇到异步监听器以 `AsyncListener` 失败 | 跨进程 | — | — | — | 已取代 | — | EVD 第三节 |
| REQ-4-9 | REQ §4 | waterfall 的 `next` 只能调用一次，第二次以 `WaterfallNext` 失败 | 跨进程 | — | — | — | 已取代 | — | EVD 第三节 |
| REQ-5-0 | REQ §5 | 能检测的违规在挂载时或运行时明确报错，不静默降级 | 跨进程 | K | — | — | 未覆盖 | #639 ※ | 总括句，没有列出哪些违规"能检测"，写法无法验证（问题 7） |
| REQ-5-1 | REQ §5 规则 1；MNT §4.1、§10 | 服务经 `ctx.provide` 或属性赋值发布和换值（rutis 侧跟随）；直接 `ctx.set` 不产生通知，rutis 侧在下一次调用该服务（含抛错）后才切换到新对象 | 跨进程 | TLA | boundary/5.1 | — | 未覆盖 | #625 | #625 的服务公告；"下一次调用后刷新"这条路径要在公告动作里体现 |
| REQ-5-2 | REQ §5 规则 2 | 跨边界事件 | 跨进程 | — | boundary/5.2 | — | 已取代 | — | 0.13 删除（#464） |
| REQ-5-3 | REQ §5 规则 3 | 事件顺序 | 跨进程 | — | boundary/5.3 | — | 已取代 | — | 0.13 删除（#464） |
| REQ-5-4 | REQ §5 规则 4 | 中间件链 | 跨进程 | — | boundary/5.4 | — | 已取代 | — | 0.13 删除（#464） |
| REQ-5-5 | REQ §5 规则 5 | 框架内部事件：`internal/*` 仍不跨边界，登记它的监听器被宿主拒绝；挂载不再列出事件 | 跨进程 | — | boundary/5.5 | — | 已取代 | — | 规则删除，内容由 EVD 第三节规定，归事件分区 |
| REQ-5-6 | REQ §5 规则 6；MNT §3.1、§10 | 同步方法在执行中等待需要 Node 事件循环推进的结果（定时器、Promise）时返回 `SyncWaitCycle`，不卡死 | 进程内并发 | TLA + SH | boundary/5.6 | — | 未覆盖 | #614 | #614 的 Node 细节（等待链上对未完成 promise 的 await）；JS 帧泵的可控调度在 #630；登记表的验证是普通测试 |
| REQ-5-7 | REQ §5 规则 7；MNT §5、§10 | 宿主提供的服务在 Cordis 侧是按接口生成的代理：按接口调用与原生一致；它不是插件声明的类的实例，`instanceof` 不成立 | 外部 | 测试 | boundary/5.7 | — | 不适用 | — | JS 对象身份，只有 Node 一个实现 |
| REQ-5-8a | REQ §5 规则 8 | 插件不得长期阻塞事件循环或卡死 | 外部 | 测试 | boundary/5.8 | — | 不适用 | — | 对插件作者的要求，rutis 无法检查 |
| REQ-5-8b | REQ §5 规则 8、§7；MNT §3.1 | 兼容层不为调用加超时：需要超时时用异步方法自行设置（超时即取消）；卸载的等待时间由应用决定 | 外部 | 测试 | boundary/5.8 | — | 不适用 | #641 ※ | 计时；登记表写明"不加超时"没有断言 |
| REQ-5-8c | REQ §5 规则 8 | 未捕获的异常或拒绝结束整个运行时进程，同一挂载的服务随之撤销 | 跨进程 | TLA + SH | boundary/5.8 | — | 未覆盖 | #625 | 撤销服务见 MNT-6-8（#625），退出的观察见 #623 |
| REQ-6-1 | REQ §6 | 远端退出、通信中断和失效引用明确失败，不返回伪成功，不自动重试副作用 | 跨进程 | TLA + K | bridge/in-flight-call-fails-once | `session::testing`（"a dropped reference is gone"） | 部分 | #626 | 退出与中断见 MNT-6-6 |
| REQ-7-1 | REQ §7 | 明确不保证：不提供单独的强制终止操作，正常卸载等插件完成清理 | 外部 | 测试 | boundary/5.8 | — | 不适用 | — | 计时 |
| REQ-7-2 | REQ §7 | 挂载被完全释放时 Node 进程以 SIGKILL 结束；只结束 Node 进程本身，插件自己启动的子进程不由兼容层回收 | 外部 | 测试 | host/killed-host-leaves-no-runtimes | — | 不适用 | — | 现在：通道关闭后进程有 2 秒自行结束（`transport/local/spawn.rs:982` `GRACE`），之后才强杀；运行时进程在自己的进程组里（:158）（问题 6） |
| REQ-7-3 | REQ §7 | 同一个挂载（含组合挂载）的插件共享一个 Node 进程，不同挂载各自一个进程；需要隔离的放进不同挂载 | 外部 | 测试 | — | — | 不适用 | — | 进程数；行模式下是"一个运行时实例一个进程"（SYN-2-1） |
| REQ-7-4 | REQ §7 | 卸载、终止与超时分别报告：正常卸载成功即成功；进程非正常结束时卸载也返回 `Transport` 错误并说明进程如何结束 | 跨进程 | TLA + SH | bridge/in-flight-call-fails-once | — | 未覆盖 | #626 | 说明部分见 MNT-6-7（#623） |
| REQ-7-5 | REQ §7 | 调用超时由应用丢弃 future 取消，Cordis 侧收到 `AbortSignal`；结果未知时不报告为未执行 | 跨进程 | TLA + K | bridge/cancel-reaches-callee | `session::testing`（取消中止信号） | 部分 | #626 | "结果未知不报为未执行"没有检查 |
| REQ-7-6 | REQ §7 | 明确不保证：进程外运行不是沙箱，运行时以应用的同一用户和权限运行、继承环境变量 | 外部 | 测试 | — | — | 不适用 | — | 操作系统 |
| REQ-8-1 | REQ §8 | rutis-host 的本机运行时可按配置在崩溃后重启（`restart`，默认不重启） | 进程内并发 | SH | host/runtime-restart | — | 未覆盖 | #627 | #627 的宿主重载 × 本地运行时监督（`restart: no` 时丢掉结束） |
| REQ-8-2 | REQ §8 | 节点行和远程运行时的 link 断开后自己重连 | 跨进程 | TLA | loader/peer-rows、loader/remote-runtime-lease | `RemoteLease` `EventuallyHeld`、`EndsHeld`（无故障） | 部分 | #615 | 故障之后的活性在 #615；节点托管行没有模型（#612 T6） |
| REQ-8-3 | REQ §8 | 其他语言的接入不在 rutis 内做 | 外部 | — | — | — | 已取代 | — | SYN：每种语言一个运行时插件（#121） |
| RTP-2-1 | RTP §2「配置」 | rutis 服务以 `host_key(名字)` 提供为 `dyn HostDispatch`；运行时插件可为它声明方法清单（`RuntimePlugin::host`） | 纯逻辑 | 测试 | — | — | 不适用 | — | 配置 API（`runtime/plugin.rs:229`） |
| RTP-2-2 | RTP §2「injects」、§4.3 | 运行时插件的 `injects` 是宿主服务：宿主服务没就绪时运行时停在 Pending，撤销后整个运行时卸载并重新等待 | 跨进程 | — | — | — | 已取代 | — | SYN §四（M1）：宿主服务移出运行时依赖、按行租用；`injects` 只有会话（`runtime/plugin.rs:9-11、277`） |
| RTP-2-3 | RTP §2「apply」 | apply 时用 `ctx.require_as` 取宿主服务构造 `Host`，再 `Process::mount` | 跨进程 | — | — | — | 已取代 | — | 按行租约（`Process::lease_host_for`）；运行时 apply 用 `Process::over(session, mount)` |
| RTP-2-4 | RTP §2「apply」「行」、§4.1 | 运行时插件先登记清理 effect（dispose 进程）、再提供 `Runtime`：清理时先撤服务、各行先卸载，最后才关进程 | 进程内并发 | SH | loader/row-unload-leaves-nothing、core/D14 | — | 未覆盖 | #627 ※ | `runtime/plugin.rs:353-392`；#612 认为 RuntimePlugin 生命周期 shuttle 进不去，也没有别的计划 |
| RTP-2-5 | RTP §2「崩溃」 | 运行时插件起一个任务等 `process.closed()`：进程不是由 dispose 结束的，就用 `provide` 返回的 Disposer 撤销服务；运行时插件本身保持 Active | 进程内并发 | 测试 | loader/runtime-down-stops-its-rows | — | 不适用 | — | #612 §2.2：RuntimePlugin 生命周期依赖 tokio spawn 与真实进程，用确定性顺序测试；两种先后都处理（`runtime/plugin.rs:357-380`） |
| RTP-2-6 | RTP §2「崩溃」、§4.2 | 进程崩溃后各行失去依赖、回到 Pending；应用调用运行时 fiber 的 `restart` 后各行恢复 | 跨进程 | TLA | loader/runtime-down-stops-its-rows | — | 未覆盖 | #625 | #625 运行时崩溃与重启、活性"行最终 Active" |
| RTP-2-7 | RTP §2「崩溃」、§4.2 | 诊断里能看到运行时缺失服务，各行的 Pending 原因指向它 | 外部 | 测试 | — | — | 不适用 | — | 诊断输出 |
| RTP-2-8 | RTP §2「行」 | 行的 `injects` 含 `TypeKey::of::<CordisRuntime>()` | 纯逻辑 | — | — | — | 已取代 | — | 行依赖 `RuntimeRows::key(name)`（`rutis-loader/src/runtime/rows.rs:47-53`） |
| RTP-2-9 | RTP §2「行」 | 行 apply 时从 ctx 取当前运行时的进程，不在工厂里捕获：运行时换代后行装到新进程上 | 跨进程 | TLA | loader/row-unload-leaves-nothing | — | 未覆盖 | #625 | #625 `OneGeneration`（运行时以新会话重启） |
| RTP-2-10 | RTP §2「Resolver」 | 名字解析在 Rust 侧完成，不需要进程 | 纯逻辑 | — | — | — | 已取代 | — | 按模块名的运行时每次问运行时（M2 §二）；远程运行时在远端解析行名（`runtime/plugin.rs:212-213`） |
| RTP-2-11 | RTP §2「Resolver」 | 运行时正在启动时，Resolver 等它就绪再取 schema | 进程内并发 | SH | loader/runtime-start-failure-isolated | — | 未覆盖 | #627 | 等待走 `RuntimeHandle`：#627 第 5 项修 `changed()` 丢唤醒 |
| RTP-2-12 | RTP §2「Resolver」、§4.4 | 没有运行时（等待中、启动失败、已退出）时不等：返回无 schema 的结果，在 `meta.schema` 写明原因、不缓存，之后 `reload` 可补上；行照常解析、按依赖等运行时 | 跨进程 | TLA | loader/runtime-start-failure-isolated、loader/unresolved-rows | — | 未覆盖 | #625 | #625 的"解析从临时变为确定"、`NoProvisional`；原因文字对任何运行时都写"the Cordis runtime is not running"（`rutis-loader/src/runtime.rs:333`） |
| RTP-2-13 | RTP §2「Resolver」、§4.4 | reconcile 永远不会卡在运行时上 | 进程内并发 | PT | loader/reconcile | — | 未覆盖 | #629 | #629"settle 会结束"；它的夹具是进程内的假插件，不含运行时 Resolver |
| RTP-2-14 | RTP §2 | 运行时插件仅 Unix | 外部 | — | — | — | 已取代 | — | Windows 上经 loopback TCP 加 token（`runtime_conformance.rs` 头注）；`runtime/mod.rs` 只有 `unix` 模块是条件编译 |
| RTP-3-1 | RTP §3 | 多个运行时实例用不同服务键（`TypeKey::keyed`）挂；行选择挂到哪个运行时留待以后 | 纯逻辑 | — | — | — | 已取代 | — | M2 §一（运行时有名字）与 SYN §四 的行名前缀 |
| INS-2-1 | INS §2、§8 | 声明为实例内的名字在一行里解析为它实例链上最近的 `group` 实例的键（Rust 服务：键带 `InstanceId`；跨语言：`host_key_in(name, instance)`）；嵌套实例里的行可用外层实例的服务 | 纯逻辑 | PT | loader/instance-services | — | 未覆盖 | #631 | #631 的"作用域与表达式"是否含实例链解析没有写明 |
| INS-2-2 | INS §2 | 每个名字只有一种归属（全局或某一种实例），没有回退：实例内的名字不在实例外解析成全局服务，反之亦然 | 纯逻辑 | PT | loader/instance-services | — | 未覆盖 | #631 |  |
| INS-3-1 | INS §3 | `isolate`、`inject`、表达式 `has` / `read`、语言行的门控与宿主都按副本用它的实例链解析 | 纯逻辑 | PT | loader/instance-services | — | 未覆盖 | #631 |  |
| INS-3-2 | INS §3、§8 | 组装期检查：一行的 `isolate` / `inject` 用了实例内名字而它不在该 `group` 之下时，这一行 `Unresolved`，错误说明要放进哪个分组；语言行、peer 行同样适用 | 纯逻辑 | PT | loader/instance-services | — | 未覆盖 | #631 | 错误文字是测试 |
| INS-3-3 | INS §3 | 表达式在实例外引用实例内名字时报错，而不是返回 `false` | 纯逻辑 | PT | loader/instance-services、loader/expressions | — | 未覆盖 | #631 |  |
| INS-4.1-1 | INS §4.1 | 语言行由 `scoped` 工厂按副本生成：依赖的跨语言服务按实例链门控，实例内名字门控在 `host_key_in(name, instance)` 上 | 纯逻辑 | PT | loader/instance-services | — | 未覆盖 | #631 | 门控本身（等、停）见 SYN-2-15 |
| INS-4.1-2 | INS §4.1 | 租用宿主服务时同一个名字在不同实例里是不同的宿主条目 | 跨进程 | PT + TLA | loader/instance-services | — | 未覆盖 | #631 | 条目 id 的单射见 INS-4.2-2；#625 的租约不含实例 |
| INS-4.1-3 | INS §4.1、§8 | 行提供的实例内服务以 `host_key_in(name, instance)` 发布到 rutis，只有同一实例里的插件看得到 | 纯逻辑 | PT | loader/instance-services | — | 未覆盖 | #631 |  |
| INS-4.2-1 | INS §4.2 | 每个副本对实例内名字自动带一条 isolate，标签为 `rutis-loader/instance/<InstanceId>`；配置里已 isolate 这个名字时用配置的标签 | 纯逻辑 | PT | loader/instance-services、loader/isolate-and-inject | — | 未覆盖 | #631 |  |
| INS-4.2-2 | INS §4.2 | 运行时里服务的身份是 (名字, 标签)，协议上的 id 为 `name` 或 `name` + NUL + `label`；名字与标签不含 NUL、标签非空，所以不同的 (名字, 标签) 不会得到同一个 id | 纯逻辑 | PT | loader/instance-services、bridge/caller-row | — | 未覆盖 | #631 | #631 疑点：`check_scoped` 不拒绝 U+0001，配置的标签 `x\u{1}row:<key>` 等于 `row_label(Some("x"), key)`（`process.rs:56-70`）；按行的标签（#441）不在本节（问题 3） |
| INS-4.2-3 | INS §4.2 | 导出槽、句柄、宿主代理、`host:<id>` 调用目标都按 id 登记 | 跨进程 | K | loader/instance-services | — | 未覆盖 | #639 ※ | 各运行时（Node、Python、Go、Bun、Swift、Kotlin）的按 id 登记没有一致性用例 |
| INS-4.2-4 | INS §4.2 | 插件看到的仍是名字：Node 用 Cordis 的 isolate（同标签的行共享一个作用域，符号同时含名字和标签），叶子运行时按行的 isolate 表查找 | 跨进程 | K | loader/instance-services、loader/isolate-and-inject | — | 未覆盖 | #639 ※ |  |
| INS-4.2-5 | INS §4.2 | `hosts.provide [name, methods, label?]`：带标签时代理只在该标签的作用域里可见，调用目标为 `host:<id>`；`hosts.withdraw [id]` 按 id | 跨进程 | TLA + K | loader/instance-services | — | 未覆盖 | #625 | #625 建 `hosts.provide` / `withdraw`，不建标签 |
| INS-4.2-6 | INS §4.2 | `service [id, handle, version]` 按 id 通知导出槽；`rows.load` 不变，导出名的标签取自本行的 isolate | 跨进程 | TLA + K | loader/instance-services | — | 未覆盖 | #625 |  |
| INS-4.2-7 | INS §4.2 | 无标签的行和以前逐字节相同 | 跨进程 | — | — | — | 已取代 | — | #441：支持 `scopes` 的运行时每行另登记一份带 `row_label` 的租约（`process.rs:1233-1265`），无标签的行也带标签 |
| INS-4.2-8 | INS §4.2 | Rust 只在确实用到标签时要求运行时支持 `scopes`；缺少它的运行时用实例内服务时报错 | 纯逻辑 | PT | loader/instance-services | — | 未覆盖 | #631 ※ | 报错文字对任何语言都写"the Node runtime lacks `scopes`"（`process.rs:1184-1190`），与规格的"需要支持 `scopes` 的运行时"不同 |
| INS-4.2-9 | INS §4.2 | 全部运行时实现 `scopes` 与 `isolate`（规格写 Node 与 Python） | 跨进程 | K | loader/instance-services | — | 未覆盖 | #639 ※ | 现在 Go、Bun、Swift、Kotlin 也实现（bridge/caller-row 依赖它）；一致性套件没有 `scopes` 用例 |
| INS-4.2-10 | INS §4.2「顺带修复」 | isolate 了某个名字的语言行能看到为它租用的宿主代理；两行各自 isolate 同一名字并导出时第二行不被拒绝 | 跨进程 | K | loader/instance-services | — | 未覆盖 | #639 ※ |  |
| INS-5-1 | INS §5、§8 | 实例里的节点连接每个实例一条：导出、导入和宿主按该行所在实例映射名字（`ServiceKeys`，默认 `host_key(name)`）；宿主为对方装载的插件按该行的实例链生成工厂 | 纯逻辑 | PT | loader/instance-links | — | 未覆盖 | #631 ※ | #631 的"export / import 收窄"只查匹配，不含按实例映射 |
| INS-5-2 | INS §5 | 每个实例的连接提供的 `Peer#<id>` 以该实例的标签 isolate，同一配置在多个实例里各有一条连接互不冲突 | 纯逻辑 | PT | loader/instance-links | — | 未覆盖 | #631 ※ |  |
| INS-5-3 | INS §5 | 对端按身份区分连接：同一对端无法同时接受同一节点的两条连接 | 跨进程 | TLA | loader/instance-links | — | 未覆盖 | #615 ※ | `RemoteLease` `OneLease` / `AccessPoint` `OneInstancePerId` 是运行时租约与接入点，不含节点之间的 link；peer 协议暂停（#612 T6） |
| INS-5-4 | INS §5、§8 | 放在实例外的连接导出或导入实例内名字时连接行报错（说明要放进哪个分组）；实例里的连接不能开启 `rows` 或 `runtime` | 纯逻辑 | PT + 测试 | loader/instance-links | — | 未覆盖 | #631 ※ | 错误文字是测试 |
| INS-5-5 | INS §5 | 实例里的 `peer:` 行和远程运行时的行照常使用实例外的连接；远程运行时与本地运行时走同一套 `scopes` | 跨进程 | K | loader/instance-services | — | 未覆盖 | #639 ※ |  |
| INS-6-1 | INS §6 | 实例内服务的键带实例 id，内核在实例子树关闭时撤销它们 | 进程内并发 | SH | loader/instance-close-leaves-nothing、instance/subtrees.5 | — | 未覆盖 | #622 | #622 的子树关闭模型 |
| INS-6-2 | INS §6、§8 | 宿主租约、投影、导出槽随行的副本卸载，运行时进程里按 id 登记的条目随之删除，同名的其他实例不受影响 | 跨进程 | TLA + PT | loader/instance-close-leaves-nothing | — | 未覆盖 | #625 | #625 `LeaseWithinRow`、`WithdrawBeforeUnload`（不含实例与标签）；#629 的实例 PT（"任务与计数回到基线"） |
| INS-6-3 | INS §6 | 节点连接放在实例里时随实例关闭断开，导入的服务随之撤销 | 跨进程 | TLA | loader/instance-links | — | 未覆盖 | #615 ※ |  |
| INS-7-1 | INS §7 | 只注册全局名字的应用，行为和协议都不变 | 跨进程 | 测试 | bridge/cross-version-meetings | — | 不适用 | — | 跨版本端到端矩阵；"协议不变"已被 #441 的按行租约部分改变（INS-4.2-7） |
| INS-7-2 | INS §7 | `ServiceCatalog::key(name)` 只返回全局名字的键；实例内名字用 `key_in(name, build)` | 纯逻辑 | PT | loader/instance-services | — | 未覆盖 | #631 |  |
| MSV-4-1a | MSV §四 R1 | 对外的只有带 `#[service]` 的 impl 块里的公开 `&self` 方法；一个类型的几个标注块合起来是它的方法 | 外部 | 测试 | bridge/marked-services | — | 不适用 | — | 编译期（过程宏） |
| MSV-4-1b | MSV §四 R1、§五.3 | 没有标注的类型不对外：`provide` 时按 `TypeId` 查登记表，没有登记项就只提供原生服务 | 纯逻辑 | PT | bridge/marked-services | — | 未覆盖 | #631 | #631"标记服务的代次规则" |
| MSV-4-2a | MSV §四 R2 | `ctx.provide(value)`（及键是类型本身的 `provide_as`）在同一作用域里同时提供对外入口，两者由同一个撤销函数撤销、一起随插件卸载 | 进程内并发 | SH | bridge/marked-services | — | 未覆盖 | #622 ※ | 内核 shuttle（#622）的模型不含标记服务；#612 风险 SV1 的控制只列单元与端到端测试 |
| MSV-4-2b | MSV §四 R2 | `provide_mut_as` 的可变绑定和带限定名（`TypeKey::keyed`）的不对外 | 纯逻辑 | 测试 | bridge/marked-services | — | 不适用 | — | 单分支的确定判定，没有输入空间可探索 |
| MSV-4-2c | MSV §四 R2 | 带 `check()` 谓词时入口与原生绑定一样受它门控 | 进程内并发 | SH + PT | bridge/marked-services | — | 未覆盖 | #622 ※ |  |
| MSV-4-2d | MSV §四 R2 | 每次按名字的调用经提供方上下文的严格读取路径取服务：读取拦截器以调用方的行为读取方生效（没有行调用时为提供方），只读到入口提供时的那个绑定 | 进程内并发 | PT | bridge/marked-services | — | 未覆盖 | #631 | #631"查找、作用域与严格读取的判定表"；SV1 的 #534 |
| MSV-4-2e | MSV §四 R2、§五.3 | 因原生槽位或名字被占而被拒的 `provide` 什么都不提供（先查原生槽位、再由 `Exposer::admit` 查名字，都通过才登记） | 进程内并发 | SH | bridge/marked-services | — | 未覆盖 | #622 ※ | 并发 provide 之间的检查与登记 |
| MSV-4-2f | MSV §四 R2、§五.3 | 先提供原生服务、再按名字提供；撤销时先撤名字、再撤原生服务（撤销函数与插件卸载相同），不会有"名字还在、原生服务已撤"的调用 | 进程内并发 | SH | bridge/marked-services | — | 未覆盖 | #622 ※ |  |
| MSV-4-2g | MSV §八 | 撤销 `provide` 返回的撤销函数后其他语言立刻调不到它，用它的行先停 | 跨进程 | SH + TLA | bridge/marked-services、core/D14 | — | 未覆盖 | #622 | #622 的依赖驱逐（Bug B：撤下返回时消费者已停） |
| MSV-4-3a | MSV §四 R3、§五.3 | 作用域跟着提供的位置：在实例里提供只在那个实例里对外（`host_key_in`），实例外为全局；在隔离作用域里提供在同一作用域对外，两个隔离作用域各自对外 | 纯逻辑 | PT | bridge/marked-services、loader/instance-services | — | 未覆盖 | #631 | #631"查找、作用域与严格读取的判定表" |
| MSV-4-3b | MSV §四 R3、§五.3 | 在实例里提供时实例认领这个名字：loader 解析实例里的行先看认领，不退回同名全局服务；新的认领让已按全局键建好的副本重建 | 进程内并发 | SH + PT | bridge/marked-services | — | 未覆盖 | #622 ※ | 认领与副本重建的交错 |
| MSV-4-4 | MSV §四 R4 | 能传的与 `Exports` 相同：serde 参数与返回值、`async` 与同步方法（对调用方都是 Promise）、按引用的回调、`&Ctx` 调用方行、`Result` 错误（`name` / `code` / `data`）、带标签的值 | 跨进程 | PT + K | bridge/marked-services、bridge/exported-services | — | 未覆盖 | #631 | 值往返 PT 只在会话层；按名字调用 Rust 服务没有 PT，#631 只有错误码差分 |
| MSV-4-5 | MSV §四 R5 | 标注的 impl 里有绑定不了的公开方法时是编译错误、指到那个方法；`#[service(skip)]` 跳过；带 `#[cfg]` 的方法只在含它的构建里检查 | 外部 | 测试 | bridge/marked-services | — | 不适用 | — | 编译期 |
| MSV-4-6 | MSV §四 R6 | 同一作用域里两个类型对外名字相同时后提供的 `provide` 失败，错误写明双方类型；同一类型的不同块不能有两个方法映射到同一个名字 | 进程内并发 | SH | bridge/marked-services | — | 未覆盖 | #622 ※ | 并发 provide 的重名判定 |
| MSV-4-7 | MSV §四 R7 | 行按名字依赖 `x`、没有叫 `x` 的服务、但有默认名为 `x` 的未标注 Rust 服务时，`Loader::waiting_for` 写明要在它的 impl 上加 `#[rutis::service]` | 外部 | 测试 | bridge/marked-services | — | 不适用 | — | 诊断文字 |
| MSV-4-8a | MSV §四 R8 | dylib 里带标注的类型加载后同样对外，宿主不需要知道它 | 外部 | 测试 | bridge/marked-services | — | 不适用 | — | 动态库加载（`tools/test-dylib.sh`） |
| MSV-4-8b | MSV §四 R8 | 换代时旧代的对外入口随旧代的服务撤销，新代提供时重新对外，消费者按现有规则重载 | 进程内并发 | SH | bridge/marked-services | — | 未覆盖 | #632 | #632 的 dylib 代次与版本槽 shuttle |
| MSV-4-9 | MSV §四 R9 | `rutis-host types` 从源码找出全部标注类型，写出与 `Exports` 相同的声明 | 外部 | 测试 | bridge/marked-services | — | 不适用 | — | 代码生成与 tsc |
| MSV-4-10 | MSV §四 R10 | `build.rs` 的 `.service` / `.service_in` 与手写 `HostDispatch` 继续可用；一个类型既有标注又在清单里时构建报错 | 外部 | 测试 | bridge/marked-services、bridge/exported-services | — | 不适用 | — | 构建期 |
| MSV-5-1 | MSV §五.1 | 宏把按类型的判断（是不是 `Result`、错误怎么取 `code`、参数是不是函数）交给 trait 在展开后选，类型别名照样识别 | 外部 | 测试 | bridge/marked-services | — | 不适用 | — | 编译期 |
| MSV-5-2 | MSV §五.2 | 每个标注块生成一个静态登记项放进初始化段，加载时加进登记表 | 外部 | 测试 | bridge/marked-services | — | 不适用 | — | 链接器与操作系统的加载 |
| MSV-5-3 | MSV §五.2 | 登记表按代区分各映像的登记项：库的初始化段登记的项属于它自己的一代，库构建出的插件 fiber 标上这一代 | 进程内并发 | PT + SH | bridge/marked-services | — | 未覆盖 | #631 | #631"标记服务的代次规则"；并发打开见 #632 |
| MSV-5-4 | MSV §五.2 | 带标注的值按提供它的代码所属那一代的登记项对外、在提供时取定；那一代没为它登记或代码不属于任何库时只用第 0 代，否则就没有；绝不用另一个库的 | 纯逻辑 | PT | bridge/marked-services | — | 未覆盖 | #631 | SV1（#522、#531） |
| MSV-5-5 | MSV §五.2 | 名字只在每个插件最新的构建里查：新构建改名或去掉的名字不再对外 | 纯逻辑 | PT | bridge/marked-services | — | 未覆盖 | #631 |  |
| MSV-5-6 | MSV §五.2 | 映像永不卸载、取定的登记项一直有效；登记项与代的数量受加载器每个插件的版本数上限约束 | 外部 | SH + 测试 | bridge/marked-services | — | 未覆盖 | #632 | #632"每个插件最多 `max_versions` 个"；资源上限本身是测试 |
| MSV-5-7 | MSV §五.2 | 同一版本的两次构建可以同时加载，各自的值运行各自构建的分发代码 | 进程内并发 | SH | bridge/marked-services | — | 未覆盖 | #632 | #632 的并发打开 |
