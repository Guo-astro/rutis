# rutis 自身机制的问题，以及哪些是 Cordis 兼容的代价

[English](review-cordis-architecture-2026-11-18.en.md)

状态：评审记录，2026-11-18。不是设计文档，不新增设计决策。
范围：只讨论 **rutis 自己的机制**。Cordis 自身的缺陷不算 rutis 的问题；只有“为了兼容 Cordis 而保留/引入”的代价才算进来。

分类：

- **R：rutis 自身机制问题**。不做 Cordis 兼容也存在。
- **C：兼容 Cordis 导致的代价**。去掉兼容就会换一种做法。
- **R/C：两者都有**。机制本身有缺口，Cordis 兼容让它更难改。
- **D：考验开发者能力的部分**。不是框架缺陷，而是框架把决定权交给开发者；会用的人不踩，不会用的人会静默踩。D 不是第四类缺陷，而是一个视角：同一项可能同时是 R/C 和 D。
- **X：Cordis 自己的行为**。我们选择运行真实 Cordis，所以不修；只写清楚边界。

## 0. 先说结论

rutis 的内核生命周期做得比 Cordis 稳，但“稳”要限定对象：Rust 内核里同一个 `EffectRecord` 只执行一次，正常卸载按逆序逐条等待；卸载期注册直接拒绝；装载失败回滚已注册项；配置更新先 `validate_config → build → validate`（不试跑 `apply`）；依赖身份带 provider generation 和 scope；服务、事件、插件都能跨进程。它不保证所有 SDK 的所有注册共用一个栈，不保证清理成功，也不撤回已经交出去的普通引用。

**真正属于 rutis 自己的问题，主要是这七条：**

1. 裸定时器、裸 socket、裸连接池不会自动清理。
2. 依赖是“状态标记”，不是图；静态环能查，运行中双行互等能报 `Cycle`，但更复杂的动态环没有完整诊断。
3. 内核更新不是原子事务；loader 有回滚，但回滚也可能失败，且模块 reload 不保证旧文件代码回来。
4. 默认一个运行时进程，同一运行时的插件共享故障域。
5. 清理可能卡死；Node / Python / Bun 的单行卸载没有逐条预算。
6. 事件没有“这个事件必须用什么模式派发”的声明契约，拦截仍靠派发处自觉。
7. 不同 SDK 的 effect API 不一致，资源本体要自己保存；`isolate` 只在运行时可见，类型层看不见。

**兼容 Cordis 带来的代价，主要是这五条：**

1. 五种事件模式、`emit` 丢返回值、`parallel` 丢返回值、`serial`/`bail` 分开，都是为了保持 Cordis 的 API 形状。
2. `bail` 遇到异步监听器改成报错，而不是像旧 Cordis 那样把 Promise 当结果；这是故意不兼容，为了不重蹈 `isBailed(Promise)` 的坑。
3. Cordis 的 `intercept` 配置（按服务合并配置）不支持。
4. Cordis 侧直接 `ctx.set` 不发通知，runner 在一次对象方法调用之后刷新 slot，不保证下一次调用已经命中新对象。
5. 宿主提供的服务在 Cordis 侧是代理，`instanceof` 不成立；同步等待可能报 `SyncWaitCycle`。

**Cordis 自己的毛病，不记在 rutis 账上：**

- Cordis 原生 fiber 的 `_unload` 用 `Promise.all` 并发清理。我们选择运行真实 Cordis，所以它的清理顺序就是 Cordis 的；rutis 不修。唯一要做的是别让文档/保证声称覆盖它。
- Cordis 的 `bail` 把 Promise 当返回值、Cordis 的论文/造词问题，都不是 rutis 的机制问题。

## 0.1 三个问题的短答

### 自己得修的

- **P0：先把对外保证说准**。不是所有 SDK 的所有注册共用一个栈；`dispose` 可以提前执行；插件定义的副本在普通 effect 栈之前结束；Node leaf 只有 `provide` 撤回、`effect`、`apply` 返回的清理进自己的 `steps`，`on` 走 Cordis fiber；runner 先撤监听器和 exporter，再 dispose fiber。截止时间之后监听器可能还在跑；任意后台任务没有自动 join；`apply` 不响应取消会挡住卸载。loader 不保证旧可执行代码回来：模块解析失败可以保留旧实例，但新 `apply` 失败后恢复的是 previous resolution，Node/Python 从文件启动的行会再次读到坏代码，直到修复前 Failed。这些会改变使用者对热更新失败、强停和资源撤回的处理。
- **P1：单行卸载的可诊断性和超时策略**（R5）。Node/Bun/Python 直接等待用户清理；逐条 timeout 本身不会终止旧代码，不能同时声称“解决卡死”和“保持严格串行”。需要可控热卸载的部署可以升为 P0。
- **P1/P2：按证据做工具**。R1 裸资源是所有权边界，不是已注册清理失效；托管 timer 和示例有价值，但没有证据把所有 socket/pool lint 列为 P0。R2 只补长环或未知提供者的动态诊断——`Loader::waits` 已经能从运行中 Pending 行报 `WaitReason::Cycle`。R4 先给部署建议；“高风险”不是框架能自动判定的属性。R6 先做可选事件契约，有真实误用证据再升级。R3 写清配置编辑和模块 reload 的差别。R7/R8/`collect` 不宜阻塞发布。

**不修、只写边界的**：Cordis 原生 `_unload` 并发、`intercept` 配置、`ctx.set` 不通知、`instanceof`、`SyncWaitCycle`。

### 为兼容 Cordis 做的选择

- **运行真实 Cordis**，不 fork 上游；但 Node 运行时在安装适配器时保存原 `events.dispatch` 和五个 `events[mode]`，再覆写它们（`internal/*` 才委托原实现）。这是实质性的兼容维护成本，不只是“依赖一个包”。Node 运行时依赖 `@deepseek-ai/cordis` 4.0.4，单副本，协议版本握手。
- **保留 Cordis 的五种事件方法**：`emit` 返回 `void`、`parallel` 不收集值、`serial`/`bail` 分开。
- **`bail` 故意不兼容旧 Cordis**：异步监听器报 `AsyncListener`，不把 Promise 当结果；这是为了修 Cordis 的静默坑。
- **保留 `ctx.provide` / `ctx.get` / `ctx.set` 语义**：宿主服务是代理，`instanceof` 不成立；同步等待可能 `SyncWaitCycle`。
- **把 Cordis 监听器接到逻辑总线**：`internal/listener` 注册，`internal/dispatch` 本地触发；`internal/*` 不跨进程。
- **不统一 Cordis 原生 fiber 的 `_unload`**：它的清理顺序就是 Cordis 的；`sdk/unload-order` 只覆盖 leaf/SDK 自己管理的行。
- **`intercept` 配置不支持**：loader 把这样的行标为 `Unresolved`。

**以后不兼容了会怎样**：可以只留一个 `dispatch`，返回所有监听器的结果；`serial`/`bail` 退化成 helper；事件定义里要求 mode；用原生类型化服务 API 取代 `ctx.get/set` 代理；去掉 `internal/*` 桥和 `intercept`。但跨进程的代理、同步等待、取消边界仍然存在，不是 Cordis 独有的。

### 隔离与回收到底做到了什么

- **注册项级**：Rust 内核里，同一个 `EffectRecord` 只有一个 drainer，重复 drain 等同一结果；正常 fiber 卸载按逆序逐条等待。不是所有 SDK 的所有注册共用一个总栈：`dispose` 可以提前执行，插件定义副本先结束，Node leaf 只有 `provide` 撤回、`effect`、`apply` 返回的清理进 `steps`，`on` 走 Cordis fiber，runner 先撤监听器和 exporter 再 dispose fiber。撤回注册、调用清理、清理成功、资源不可用是四个不同结果；已交出的 `Arc`/普通引用不会因撤回而失效。
- **服务作用域**：做到了。`isolate` 按服务键分作用域，loader 的 `isolate` 也覆盖其他语言的行。
- **事件作用域**：没有。事件是按名字在一条总线上送达，不按 `isolate` 过滤（`events_are_not_filtered_by_isolate`）；要隔离就过滤或换名字。
- **故障/CPU/内存/安全**：默认没有。同一个运行时的行共享进程、堆和事件循环；拆分运行时/节点才有故障隔离；不是沙箱，插件是可信代码。
- **未注册资源**：没有。裸 timer/socket/pool 不在栈上，不会自动回收。
- **运行中的监听器/任务**：截止时间覆盖运行时跟踪的在途调用；超过截止时间，监听器可能还在跑，卸载继续。任意后台任务没有自动 join；`apply` 不响应取消会挡住卸载。
- **外部副作用**：没有。消息已发、文件已写、数据库已提交，清理只能补偿，不能回滚；跨进程也没有分布式事务。

**一句话**：对“经过 rutis 注册的影响”，我们做到的是**按所有者、按各层契约**的生命周期管理和回收，Rust 内核里同一 `EffectRecord` 只执行一次；不是所有 SDK 共用一个总栈，也不是所有资源都一定收得回来。对“插件能影响到的整个世界”，没有，也不应该假装有。架构上要明确写：我们保证的是受管注册项的清理去重和 dependency-scoped service isolation，不是 fault/CPU/memory/security isolation，也不是 distributed transaction。

## 1. R：rutis 自身机制问题

| 编号 | 问题 | 现状 | 为什么是 rutis 的问题 | 影响 |
| --- | --- | --- | --- | --- |
| R1 | 裸资源不自动清理 | `ctx.effect` / `ctx.provide` / `ctx.on` 进同一个 effects 列表；裸 timer / socket / pool 不进；`ctx.signal` 只提供取消信号 | 框架只保证“经过 effect 的东西”；没有统一资源包装，也没有泄漏检测 | 长期运行的宿主里，卸载一行后裸 timer 继续跑；不报错 |
| R2 | 依赖是标记，不是图 | `last_deps` 是 `(provider id, generation, TypeKey, scope)` 集合；能做门控、等值合并、依赖变化重载；`Loader::waits` 已能从运行中 Pending 行报 `WaitReason::Cycle` | 不做一般环检测/传递闭包；长环、未知提供者的动态图没有完整诊断 | 双行互等已有报告；更复杂的动态环仍可能停在 Pending，只能靠诊断或人工排查 |
| R3 | 内核更新不是原子事务 | `FiberView::update` 先 `validate_config → build → validate`（不试跑 `apply`）；Active/Loading/Failed 才卸旧装新，Pending 只重查依赖、保留新配置。loader 分配置编辑和模块 reload 两种回滚 | 内核没有跨 fiber 事务；loader 回滚是重新协调，不是补偿协议；模块 reload 恢复 previous resolution，不保证旧文件代码回来 | 回滚失败会留下部分状态；Node/Python 从文件启动的行在 apply 失败后会读到坏代码直到修复 |
| R4 | 默认故障域是单个运行时进程 | 每个语言运行时至少一个进程；行可以拆到其他运行时/节点，但默认同运行时共享进程 | 默认值是共享；隔离要显式配置 | 一个插件死循环/崩溃，同运行时的行一起受影响 |
| R5 | 清理可能卡死，单行卸载没有逐条预算 | `dispose_with_timeout` / `shutdown_with_timeout` 有界；`rutis-host --shutdown-timeout` 限制总清理；Node/Python/Bun 的 `rows.unload` 没有逐条预算 | 内核等待清理完成；运行时只在进程退出时兜底 | 一个清理永不返回，这一行一直 Unloading；只能靠宿主停止/杀进程 |
| R6 | 事件没有“模式声明”契约 | `Event` 只有 `NAME` 和 `Value`；监听器运行时看 `call.mode()`；派发处选择模式 | rutis 没有在事件定义/目录里记录“这个事件必须用什么模式派发” | 监听器想拦截、派发处用 `emit`，返回值被丢掉；编译和运行都不报错 |
| R7 | 单回调 effect；资源本体自己保存 | Rust `ctx.effect` 立即执行同步 factory，把返回的 future 当清理；Node leaf `effect(cleanup)` 只把函数压栈；原生 Cordis 另有异步 effect 重载。三者不是同一个 API，也不是同一个问题 | 这是按 SDK 不同的 API 选择：资源获取、资源持有、清理注册是三件事；没有 `effect(acquire, release)` 的统一形状 | 跨生命周期的资源要靠闭包/IIFE/基类；写起来啰嗦，容易忘；R7 更接近 SDK 易用性，不是共同机制缺陷 |
| R8 | `isolate` 只在运行时可见 | `isolate` 是运行时 scope；诊断显示 scope；类型系统不编码 | 类型层无法表达“这个服务在哪个隔离域” | 读错隔离域只能在运行时发现；代码审查看不出 |

### R1：裸资源

这是最实在的一条。框架能管的是“你告诉我怎么清理”的东西；你不说，它就不管。rutis 比 Cordis 多的是 `ctx.signal` 和进程退出兜底，但没有 timer 包装，也没有泄漏检测。

**要修的话**：提供 `ctx.timeout` / `ctx.interval` 或统一资源包装；或者在 `rutis-host check` / lint 里提示裸 timer。不修的话，就写进边界：裸资源是作者责任。

### R2：依赖图 / 环

rutis 的依赖身份比 Cordis 强：provider 换一代、换一个 scope 都能区分；常见的 A→B→A 靠 provider generation 区分。**但同代、同键、同 scope 重新提供时，四元组可以完全相同，不能识别任意 ABA**；`evicted_apply` 测的是“撤回已标记、旧 consumer 正退出时，同代重供只触发一次重新装载”，旧撤回不误删新 binding 另靠 `Arc::ptr_eq`。它仍然只回答“提供者组合变了吗”，不回答“这张图有没有环、有没有传递闭包”。`Loader::waits` 已经能从运行中 Pending 行报 `WaitReason::Cycle`（A/B 互等有测试）；缺的是一般长环或未知提供者的完整动态诊断。

**要修的话**：内核 D22 可以不承诺一般环检测，但 `Loader::waits` 的双行 `Cycle` 已经存在；补长环或未知提供者的疑似环报告。静态环已经有 `rutis-host check`。

### R3：事务性

内核的 `update` 先 `validate_config → build → validate`，**不试跑 `apply`**；Active/Loading/Failed 才“先卸后装”，Pending 只重查依赖、保留新配置。loader 的 all-or-nothing 分两种：配置编辑恢复旧可编辑层；模块 `reload` 在新 `apply` 失败时恢复 previous resolution。Node/Python 从文件启动的行会再次读到磁盘上的坏代码，直到修复前 Failed；测试明确断言这时不能打印“previous version keeps running”。回滚失败返回 `RollbackFailed`，不是跨插件事务。

**要修的话**：要么接受“内核不做事务，事务在 loader”，并写清楚；要么设计一个跨插件的两阶段替换。后者代价很大，不建议为了好看去做。

### R4：默认故障域

隔离是 rutis 的一等能力，但默认不是隔离。同一个运行时的行共享内存、全局状态、事件循环；一个插件阻塞事件循环，其他行也停。

**要修的话**：不是改内核，而是改默认部署/文档：高风险插件默认拆运行时；`rutis-host` 提供更明显的隔离配置。

### R5：清理预算

`dispose_with_timeout` / `shutdown_with_timeout` 能让你“不等了”，但清理本身还在跑；Node/Python/Bun 的单行 `rows.unload` 没有逐条预算，只有整个进程退出时兜底。一个清理永不返回，这一行会一直 Unloading。

**要修的话**：给运行时行的卸载加逐条预算，或至少把“单行卸载没有逐条预算”写成明确边界。Go/Swift 已经有整次卸载的预算，可以参考。

### R6：事件模式契约

内核底层是 `EventBus::dispatch(Dispatch { mode, .. })`，所以底层本身是中性的；问题是**事件定义**里没有 mode。监听器只能在运行时看 `call.mode()`，不能在注册时声明“我必须被 `bail` 调用”。

R6 和 C3 是同一个问题的两面：R6 是 rutis 自己没有“模式声明”这个机制；C3 是 Cordis 的事件声明没有 mode，让这个机制更难强制。

**要修的话**：最小做法是在事件目录/生成物里记录 mode，并在 `check` 时比对派发处；更强做法是让 `Event` 带上 mode，或在 `ctx.on` 上要求声明。至少让“监听器想拦截、派发处用 emit”在开发期可见。注意 `Event::name()` 允许运行时名字，跨语言 check 不是无条件可行的；先做可选契约，有真实误用证据再升级。

### R7：单回调 effect

Rust `ctx.effect` 立即执行同步 factory，把返回的 future 当清理；Node leaf `effect(cleanup)` 只把清理函数压栈；原生 Cordis 才有异步 effect 重载。**同步获取、异步获取、是否把资源本体返回给调用方，是三个不同问题**，不能拼成一条“单回调缺陷”。异步 acquire 在 Rust 里没问题（Promise 重载在）；真正的摩擦是“没有本体”和“获取/持有分两步”的资源。

**要修的话**：这不是签名缺陷，封装就能解决：IIFE、基类、或 `ctx.effect` 之外的服务包装。文档里已经有这类写法；可以把它们提成推荐模式。

### R8：isolate 类型不可见

`isolate` 是运行时 scope，类型系统不编码。诊断能看到 scope，但写代码时看不出“这个服务在哪个隔离域”。

**要修的话**：类型层很难做；更现实的是在代码生成/诊断里把 scope 标出来，或在 API 上要求显式传 scope。不要为了类型漂亮去改运行时语义。

## 2. C：兼容 Cordis 导致的代价

| 编号 | 代价 | 兼容要求 | 去掉兼容会怎样 | 现在能不能改 |
| --- | --- | --- | --- | --- |
| C1 | 五种事件模式；`emit` 丢返回值；`parallel` 丢返回值；`serial`/`bail` 分开 | Cordis 的 API 就是五个方法，`emit` 返回 `void`，`parallel` 不要值 | 可以只留一个 `dispatch`，返回所有监听器的结果；`serial`/`bail` 可以退化成 helper | 不能删；`collect` 不是现有 `parallel` 的薄包装。跨运行时收集值需要新的结果通路或应用层协议 |
| C2 | `bail` 遇到异步监听器报 `AsyncListener` | Cordis 旧行为把 Promise 当短路值；rutis 选择显式报错 | 可以保留旧行为，但会重新引入 `isBailed(Promise)` 的静默错误 | 不建议改回；把差异写进兼容文档 |
| C3 | 事件 mode 不在类型契约；拦截由派发处决定 | Cordis 的事件声明没有 mode，派发处选方法 | 可以在事件定义里要求 mode，编译期检查 | 可以加“事件目录 + check 比对”，不必改 Cordis API |
| C4 | Cordis `intercept` 配置不支持 | 兼容层不做按服务合并配置；`null`、`true`、`{}` 这种裸声明仍接受，只有非空合并配置被拒绝 | 不做 Cordis 兼容就不需要 | 可以做，但优先级低；dsh 插件里也没搜到使用 |
| C5 | 直接 `ctx.set` 不发通知 | Cordis 的 `ctx.set` 语义就是不通知；runner 在一次对象方法调用之后刷新所有 slot | 不做 Cordis 兼容就不需要 | 可以 hook，但侵入 Cordis；先写边界 |
| C6 | 宿主服务是代理，`instanceof` 失败；同步等待可能 `SyncWaitCycle` | 跨进程/跨语言无法保留 JS 对象身份和事件循环 | 全部同进程 Rust 插件就不会有 | 不能完全消除；写清楚边界，调用方按接口用 |

### C1：五种模式 / `emit` 丢返回值

这是最大的一笔兼容税。文章说 Cordis 的 `emit` 既是地基又是五种用法之一；rutis 的内核把地基改成了 `dispatch(mode)`，但对外 API 仍然保留五种方法，因为 Cordis 插件就是这么写的。

- **如果不兼容 Cordis**：可以只留一个 `dispatch`，返回所有监听器的结果；`serial`/`bail` 变成 helper；`parallel` 可以收集值。
- **现在**：不能删。删了 Cordis 插件全挂。而且 `collect` 不是现有 `parallel` 的薄包装：`serial`/`bail` 命中后后续监听器根本不执行，`parallel` 等全部完成后丢掉成功值，普通 helper 无法从现有结果还原各监听器返回值。跨运行时收集值需要新的结果通路或显式应用层协议。
- **结论**：保留方法名和返回形状是 C；把五种执行语义写进通用协议、原生 SDK 是否另设接口，是 R 的设计选择，不能全算成兼容税。

### C2：`bail` 的异步语义

Cordis 的 `bail` 会把返回的 Promise 本身当成结果；rutis 改成“遇到异步监听器报 `AsyncListener`”。这是故意不兼容，因为旧行为会静默把未落定的 Promise 当真值。

- **如果不兼容 Cordis**：可以重新设计 `bail`，不需要保留这个历史包袱。
- **现在**：保留显式报错；依赖旧行为的插件需要改。
- **结论**：这是“为了修 Cordis 的坑而故意不兼容”的代价，不是 rutis 机制退步。

### C3：事件 mode / 拦截

Cordis 的事件声明里没有 mode，派发处选方法；rutis 继承了这一点。监听器只能运行时看 `call.mode()`，不能声明“我必须被 `bail` 调用”。

- **如果不兼容 Cordis**：可以在事件定义里要求 mode，编译期检查。
- **现在**：可以加“事件目录 + `check` 比对”，不必改 Cordis API；`observe_dispatch` 已在 0.19 删除（无生产调用方），要观察就注册普通监听器。
- **结论**：一半是兼容税，一半是 rutis 自己没有做声明契约。

### C4：`intercept` 配置

Cordis 的 `inject` 对象形式可以按服务合并配置；rutis loader 接受 `null`、`true`、`{}` 这种裸声明，但拒绝非空合并配置，并把这样的行标为 `Unresolved`。这是**未实现的兼容能力**，不是拒绝整个对象形式。

- **如果不兼容 Cordis**：不需要这个功能。
- **现在**：可以做，但优先级低；dsh 插件里也没搜到使用。
- **结论**：兼容缺口，不是内核问题。

### C5：`ctx.set`

Cordis 侧直接替换服务对象不发通知；runner 在一次对象方法调用之后刷新所有 slot，**不保证下一次调用一定已经命中新对象**。

- **如果不兼容 Cordis**：不需要处理这个语义。
- **现在**：可以 hook，但侵入 Cordis；先写边界。
- **结论**：兼容边界，不是 rutis 机制错误。

### C6：代理 / 同步等待

宿主提供的服务在 Cordis 侧是代理，`instanceof` 失败；同步方法不能等需要 Node 事件循环推进的结果，成环会报 `SyncWaitCycle`。

- **如果不兼容 Cordis**：全部同进程 Rust 插件就不会有。
- **现在**：跨进程/跨语言无法完全保留 JS 对象身份和事件循环；写清楚边界，调用方按接口用。
- **结论**：跨进程固有代价，Cordis 兼容让它更明显。

## 3. D：其实是考验开发者能力的部分

有些“问题”不是框架坏了，而是框架把决定权交给了开发者。会用的人不会踩，不会用的人会静默踩。把它们单独列出来，是为了判断“该修框架”还是“该写文档/工具”。

| 编号 | 考验什么 | 开发者要做什么 | 框架现在帮了什么 | 还能帮什么 |
| --- | --- | --- | --- | --- |
| D1 | 清理顺序 | 按依赖顺序注册；知道 `provide` 之后注册的清理才读得到自己的服务 | 文档写明 LIFO；`sdk/unload-order` 有测试 | 依赖感知的 cleanup 诊断；`check` 提示可疑顺序 |
| D2 | 资源生命周期 | 裸 timer/socket 包进 `ctx.effect`，或用 `ctx.signal` 协作退出 | `ctx.effect`、`ctx.signal`、进程退出兜底 | timer/socket 包装；lint/check 提示裸资源 |
| D3 | 资源本体持有 | 用闭包/IIFE/基类保存 pool 等跨生命周期资源 | 单回调 + 文档示例 | 资源包装基类/宏；推荐模式 |
| D4 | 事件模式/拦截 | 知道派发处用哪个方法；监听器按 mode 写 | `call.mode()` 运行时可见；`bail`/`waterfall` 的错误明确 | 事件目录 + `check` 比对 mode；类型层 mode |
| D5 | `waterfall` 的 await | Cordis JS 里自己 `await next()`；Rust 用 `Option` 避免 Promise 真值 | Rust 类型、`WaterfallNext` 错误 | JS 侧 lint/模板；文档示例 |
| D6 | 服务替换 | 用 `provide`/属性赋值，不直接 `ctx.set` | 兼容层代理；下次调用切换 | hook `ctx.set`，或 `check` 提示 |
| D7 | 代理/同步等待 | 按接口调用，不做 `instanceof`；同步方法不等待事件循环 | 明确的边界错误 `SyncWaitCycle` / `AsyncListener` | 静态检查/文档；调用方超时模板 |
| D8 | 依赖环 | 不写互等 `inject`；动态依赖自己保证 | `rutis-host check` 查静态环 | 运行时疑似环报告 |
| D9 | `parallel` 的返回值 | 知道 `parallel` 不收集值；要值就用共享容器或带新结果通路的 helper | 文档、`AggregateError` 明确 | 需要新结果通路的 `collect`/`all`，不是薄包装 |
| D10 | 插件描述 | 写 `name` / `inject` / `apply`、工具说明和参数 | 生成器、模板、文档 | 更好的脚手架；从类型生成描述 |

**怎么用这张表**：

- D1 / D3 / D9 是“显式契约”：文档写清楚了，按规则写就不会错。这类可以接受，但示例和检查要做足。
- D2 / D4 / D5 / D6 是“静默失败”：不按规则写不报错。这类最危险，优先让它们变响（check、类型、错误），而不是只写文档。
- D7 是“边界是显式的，但只有撞上才知道”：`SyncWaitCycle` / `AsyncListener` 会报错，不是静默；问题在 API 形状和开发期不可见。
- D8 是“图的问题”：静态能查，动态只能靠报告。
- D10 是纯成本，不是坑：插件要让别人调用，就得描述自己。

## 4. X：Cordis 自己的行为，不算 rutis 的问题

| 编号 | Cordis 的行为 | 为什么不记在 rutis 账上 | 我们唯一要做的 |
| --- | --- | --- | --- |
| X1 | Cordis 原生 fiber 的 `_unload` 用 `Promise.all` 并发清理 | 我们选择运行真实 Cordis，不 fork 上游；Node 运行时保存并覆写 `events` 方法，但不接管原生 fiber 的 `_unload`；它的清理顺序就是它的 | 别让 `sdk/unload-order` 声称覆盖原生 Cordis 插件；在 `guide/cordis` 写清楚 |
| X2 | Cordis 的 `bail` 把 Promise 当结果 | 这是 Cordis 的旧语义；rutis 已经改成显式报错 | 把差异写进兼容文档 |
| X3 | Cordis 的论文、造词、版本新旧 | 不是 rutis 的机制问题 | 不讨论 |

### X1 展开

- rutis 的 leaf 插件（`definePlugin`）在 `node/rutis-runtime/src/leaf.mjs` 里有自己的 `steps` 栈，逆序、逐条清理；`sdk/unload-order` 覆盖的是这一类。
- 原生 Cordis 插件没有经过这个栈；`runner.mjs` 卸载一行时 `await row.fiber.dispose()`，走的是 Cordis 自己的 `_unload`。
- 这是 Cordis 的机制，不是 rutis 的机制。rutis 不修它；要修就得接管 Cordis 的 `ctx.effect` 处置，侵入 Cordis 内部，代价大。
- **唯一要做的**：不要让文档/保证暗示 `sdk/unload-order` 覆盖原生 Cordis 插件；把它写成明确的兼容边界。

## 5. 如果去掉 Cordis 兼容，哪些问题会消失

- **会消失或换做法**：C1（五模式、emit/parallel 丢值）、C2（bail 异步语义）、C3（事件 mode 声明）、C4（intercept）、C5（ctx.set）。
- **仍然存在**：R1（裸资源）、R2（依赖图/环）、R3（事务性）、R4（默认故障域）、R5（清理预算）、R7（单回调 effect）、R8（isolate 类型不可见）。
- **C6（代理/同步等待）**：如果所有插件都在同一个进程里，会轻很多；跨进程仍然存在，不是 Cordis 独有的。

## 6. 先修什么

按 Astra 评审后的口径，先按“会不会让使用者依赖错保证”排序，而不是按“听起来最像架构问题”排序：

1. **P0：纠正对外保证的范围**。写明注册项不是所有 SDK 共用一个栈；`dispose` 可提前、插件定义副本先结束、Node leaf 的 `on` 走 Cordis fiber、runner 先撤监听器/exporter 再 dispose；截止时间后监听器可能还在跑；loader 模块 reload 不保证旧文件代码回来。这些会改变使用者对热更新失败、强停和资源撤回的处理。
2. **P1：R5 单行卸载的可诊断性和超时策略**。Node/Bun/Python 直接等用户清理；逐条 timeout 不会终止旧代码，不能同时声称“解决卡死”和“保持严格串行”。需要可控热卸载的部署升 P0。
3. **P1/P2：R1 托管 timer / 示例**。裸资源是所有权边界，不是已注册清理失效；托管 timer 和示例有价值，但没有证据把所有 socket/pool lint 列为 P0。
4. **P1/P2：R2 长环/未知提供者**。`Loader::waits` 的双行 `Cycle` 已有；只补更复杂的动态图。
5. **P1/P2：R4 部署建议**。高风险插件拆运行时；不要把“高风险”当成框架能自动判定的属性。
6. **P1/P2：R6 可选事件契约**。先做可选的事件目录 + `check` 比对，有真实误用证据再升级；`Event::name()` 允许运行时名字，跨语言 check 不是无条件“低成本”。
7. **P1/P2：R3 事务边界文档**。把配置编辑、模块 reload、旧代码恢复、无缝切换拆开写。
8. **不阻塞发布**：R7（SDK 易用性）、R8（类型层 isolate）、`collect`（需要新结果通路，不是薄包装）。
9. **兼容边界集中一页**：C4/C5/C6、`bail` 差异、Cordis 原生 `_unload`，选型时一眼看完。

如果按 D 的视角，重点不是“把责任从开发者手里拿走”，而是“让不按规则写的时候有声音”：D2（裸资源）、D4（事件 mode）、D5（waterfall await）、D6（`ctx.set`）至少要在 `check`、lint 或错误里出现，不能只写在文档里；D7 已经有显式错误，问题是要在开发期更早看见。

这些 P0/P1 是工程判断，没有生产故障频率或插件使用率数据支撑；不能据此断言裸资源或事件 mode 是最常见故障。

## 7. 证据索引

核心：

- `crates/rutis/src/effect.rs:66` `EffectRecord`；`:111` `drain`；`:202` `run_cleanups`；`:238` `Disposer`。
- `crates/rutis/src/fiber.rs:856` `resolve_deps`；`:909` `refresh_deps`；`:1141` `fail_load`；`:1211` `drain_effects`；`:1239` `unload`；`:2046` `dispose_with_timeout`；`:2120` `update`。
- `crates/rutis/src/ctx.rs:634` `ctx.effect`；`:993` `register_effect_named`；`:1015-1017` 卸载期拒绝；`:1028-1041` 晚到记录排干。
- `crates/rutis/src/events.rs:101` `Event`；`:116` `Mode`。
- `crates/rutis/src/events/api.rs:83,132` `Call::mode` / `SyncCall::mode`；`:454-486` 五种模式；`:654-690` `Ctx` 上的五种模式。
- `crates/rutis/src/bus.rs:916` `EventBus::dispatch`；`:958` `dispatch_sync`。
- `crates/rutis/src/bus/run.rs` 的 `run_parallel`（只保留错误）。
- `crates/rutis/src/events/remote.rs:224` `Dispatch { mode, .. }`。

兼容层：

- `node/rutis-runtime/src/leaf.mjs:112-130`：leaf 插件自己的逆序栈。
- `node/rutis-runtime/src/cordis-events.mjs:84-96`：生命周期上只接管 `internal/listener`。
- `node/rutis-runtime/src/runner.mjs:775-797`：先撤 exporter，再 `row.fiber.dispose()`。
- `node/rutis-runtime/package.json`：`@deepseek-ai/cordis` 4.0.4。
- `crates/rutis-loader/src/loader/check.rs:467`：静态环检测。
- `crates/rutis-loader/src/loader/waits.rs:354-372`：运行中 Pending 行的 `WaitReason::Cycle`；测试 `crates/rutis-loader/tests/waits.rs:224`。
- `crates/rutis-loader/src/loader/desired.rs:309-321`：`intercept` 只拒绝非空合并配置；裸声明仍接受。
- `crates/rutis-host/tests/dev.rs:395-417`：apply 失败时不能打印“previous version keeps running”。
- `crates/rutis-host/src/main.rs:1001`：环的错误文字。
- `crates/rutis-loader/tests/loader.rs:212,245`：apply 失败回滚 / 回滚失败。
- `crates/rutis-loader/tests/reload.rs:43`：reload all-or-nothing。
- `node/rutis-runtime/src/cordis-events.mjs:100-143`：保存原方法后覆写 `events.dispatch` 和五个 `events[mode]`。
- `node/rutis-runtime/src/runner.mjs:270-285`：`ctx.set` 后在一次对象方法调用后刷新 slot。

保证与指南：

- `docs/guarantees.md` 的 `core/D6`、`core/D7`、`core/D14`、`core/D22`、`core/D32`、`events/emit`、`events/waterfall`、`sdk/unload-order`、`loader/apply-failure-rolls-back`、`loader/reload-all-or-nothing`、`host/stop-deadline`。
- `docs/guide/cordis.en.md`：Cordis 插件的运行、事件、卸载、边界。
- `docs/requirements-protocol-plugins.en.md` §4–§5：同步等待、`instanceof`、`ctx.set`、`bail` 差异。
- `docs/design-unload-order-2026-11-01.md`：卸载栈的设计背景；注意它记录的是设计时的现状，当前实现以 `guarantees.md` 的 `sdk/unload-order` 为准。

测试：

- `crates/rutis/tests/parity.rs:1141` `rejects_effect_registration_during_unload`。
- `crates/rutis/tests/contract.rs:700` `effect_during_cleanup_fails`；`:690-735` 事件与卸载。
- `crates/rutis/tests/evicted_apply.rs:356` 装载中被撤销、服务先回来仍只重载一次。
- `crates/rutis/tests/config_update.rs:133,158,548,626` 配置更新、dry-run、装载/卸载中的更新。
- `crates/rutis/tests/events.rs:274` `emit_does_not_wait_and_contains_failures`；`:511` `bail_meets_an_async_listener`；`:550` waterfall；`:956` 卸载等待监听器。
- `crates/rutis/tests/unload_deadline.rs:36` 监听器超时。
- `crates/rutis-host/tests/check.rs:576` 环检测输出。
- `crates/rutis-loader/tests/events_conformance.rs:1491` 起的 `events_with_x_*`：Cordis / leaf / Python / Go / Bun / Swift / Kotlin 的事件一致性。
- `tests/e2e/tests/cordis.rs`：真实 DSH 插件在 Node 运行时；Node 运行时崩溃不影响 Python 行。
