# 实例的常驻占用：每实例、每行由什么组成（0.20）

日期：2026-11-02。代码：main `be3e76a1` + #1016。测量对象：一个 `instanced` 分组的一个实例 = 1 个 group fiber + 8 行普通 Rust 插件，和 `crates/rutis-loader/tests/close_memory.rs` 里的 `plain` 完全相同。

[English](performance-instance-footprint-2026-11-02.en.md)

## 怎么测

- 全局计数分配器统计**活跃分配字节**（请求大小，不是 RSS、不是分配器 arena）；
- 常驻组成用按调用栈采样：小于 512 B 的分配采样 1/128，512 B 及以上全部采样，按样本的权重还原；创建前先记一次空 loader 的基线并减掉；
- 同一个测试（`close_memory`）给出总量与关闭残留，采样拆分用来回答“这些字节是什么”；
- 机器：Linux x86-64，32 核，62 GiB，rustc 1.98.1。debug 与 release 的每实例总量相差不到 3%，debug 的调用栈更完整，拆分用 debug、N=400；总量在 debug 和 release、N=40…1000 上交叉核对。

采样拆分的合计与 `close_memory` 的总量差约 2 KiB（1/128 采样误差和未列出的尾部）；下面标了“估算”的行来自采样，余量按采样总数归一。

## 一个实例的常驻组成

plain，debug，N=400，每个实例，活跃分配：

| 组成 | 每实例 | 占比 | 是什么 |
| --- | ---: | ---: | --- |
| tokio 任务与通道 | 27.7 KiB | 51% | 9 个 fiber 各一个 drive 任务（5.6）、一个 intent mpsc 的 block 与 chan（9.3）、watch 通道（`Shared<usize>` 4.5、`Shared<Snapshot>` 3.0、`Shared<()>` 0.1）、取消树节点 0.9、每行一个监视任务 4.2（future 344 B 加任务头） |
| 内核 fiber / ctx / effect | 15.8 KiB | 30% | `FiberInner` 6.7、`EffectRecord` 4.0、`CtxInner` 1.4、`FactoryAdapter` 1.0、cleanup 列表 0.7、mount effect 与 effect 索引 1.0、加载闭包与 `spawn_fiber_inner` 0.5、`mark_row`/scope 0.5 |
| loader 的记录 | 3.1 KiB | 6% | 每行的 `EntryConfig` 0.9、行事件闭包 0.9、`Follower` 0.8、group 的 `GroupPlugin` 0.2、`spawn_instance` 的 future 0.1、行 id 的字符串 0.1 |
| 运行中的哈希表容量 | 7.1 KiB | 13% | loader 的 `running`（`Slot`→`Running`）6.7、`followers` 0.35、其余表 0.03；这是分配出去的容量，不是已用条目 |
| 其余 | 0.6 KiB | 1% | 服务登记、scope 链、实例 id 的格式化、行/插件的短字符串等 |

合计约 53.9 KiB（采样，监视任务按最终 344 B 的 future 计）；同一测试不采样时每实例 52.2 KiB（debug N=400）/ 50.9 KiB（release N=1000）。`watch` 组（每行再挂一个 `FiberStatusChanged` 监听者）每实例多约 8.1 KiB：监听者、监听者列表和关闭时的队列。

**每行**：一个实例 9 个 fiber（组 + 8 行），平均约 6.0 KiB/fiber；8 行的记录只差 `EntryConfig` 里的行配置，组 fiber 用 `GroupPlugin` 代替它。行 id 只有 2–3 B 的堆字符串，不占主要部分。

## 能省的

1. **每行的监视任务（本次已改）**。每行一个 `Inner::monitor` 任务，原来的 async 块把 `set_disabled`（一次完整的 loader 操作）内联在里面，future 1,872 B。把一个实例 9 行的监视任务加起来约 16.5 KiB，占修复前的 25%。把少见的“行自停”分支放进 `Box::pin` 后，监视 future 降到 344 B，每实例省约 13.4 KiB；`Box` 只在行真的自停时才分配。改法见 `crates/rutis-loader/src/loader/reconcile.rs` 的 `Inner::monitor`；`the_monitor_future_stays_small` 直接测这个 future 的大小（正常 344 B、回退 1,872 B，上限 512 B），把回退钉住。
2. **`running` 表的容量余量（不改）**。1000 个实例运行时有 9,000 个条目、每条约 328 B，`HashMap` 的表按 2 的幂增长；N=400 时表 2.70 MB、已用条目 1.18 MB，余量约 3.8 KiB/实例。没有“一批实例创建完”的边界：每插入一个实例就 `shrink_to_fit` 会变成 O(N²)，`create_instance` 是按需一个个调的。关闭时本 PR 的收缩会把它还回去，运行中的余量属于增长代价，不算泄漏。
3. **每行记录（不改）**。`EntryConfig` 的配置可能按副本所在实例的表达式求值，不能跨实例共享；`Follower` 与行事件闭包是行事件的基础；`FiberInner`、每个 fiber 的 watch/mpsc/取消节点是内核按 fiber 的状态和取消语义需要的。没有发现重复记录、可以共享的大常量或按实例重复的固定容量。

## 前后对比（同一个测试、同一构建）

`cargo test -p rutis-loader --release --test close_memory -- --nocapture`（release，plain/watch）：

| 项目 | 本 PR 之前 | 本 PR 之后 | 变化 |
| --- | ---: | ---: | ---: |
| plain，每实例，N=1000 | 63.3 KiB | 50.9 KiB | −20% |
| watch，每实例，N=1000 | 71.5 KiB | 59.1 KiB | −17% |
| plain，每实例，debug N=400 | 64.6 KiB | 52.2 KiB | −19% |
| watch，每实例，debug N=400 | 72.7 KiB | 60.3 KiB | −17% |
| plain，关完稳定后，N=1000 | 6,001,908 B | 22,708 B | −99.6% |

关闭后的数字来自本 PR 的表收缩；常驻数字来自监视任务的 `Box`。只做表收缩时常驻不变（评审已验证：N=100/1000 创建后仍是 6,610,672 / 64,809,972 B），这次补上的 `Box` 才动常驻。

阈值随之收紧：每实例创建后改成 61.5 KiB，落在正常最差 60.3 KiB 和回退后最好 62.0 KiB 之间（回退后最差 72.7 KiB），所以把 `Box` 整段去掉会在这里失败；`Inner::monitor` 另有单元测试 `the_monitor_future_stays_small` 直接断言这个 future ≤ 512 B（正常 344 B，回退 1,872 B），定向钉住回退。关完后仍为固定部分 + 128 B/实例，固定部分从 64 KiB 改为 128 KiB（watch、N=40 的关完后残留有一次测到 58 KiB，取约 2 倍余量；修复前的 188 KiB 仍会失败）。

## 这个测试覆盖什么、不覆盖什么

覆盖：

- fiber、服务绑定（`Ctx::diagnostics`）、`FiberStatusChanged` 与全部事件监听者、tokio 任务数；
- Linux 上测试进程的 OS 线程数（`/proc/self/status` 的 `Threads:`）；
- 活跃分配字节：运行中的每实例占用、关闭峰值、关完后的残留；
- 关闭 40–1000 个实例后以上计数全部回到创建前，残留字节不随 N 增长。

不覆盖：

- **语言运行时进程**：本测试只有 Rust 行，不启动 Node、Python 等；运行时进程里的行、服务、代理、导出槽和对象句柄的残留由 `crates/rutis-loader/tests/instance_runtimes.rs`（D2/D3）和 S7（`tests/e2e/tests/embedding.rs`，PR 20 次、每晚 3000 次）覆盖；
- **非零服务绑定的实例内残留**：`plain`/`watch` 不 `provide` 服务，本测试只验证绑定的零增量；实例内同名服务隔离和关闭后的绑定清除由 `instance_services.rs`、`instance_runtimes.rs` 和 S7 覆盖；
- **macOS / Windows 的 OS 线程数**：`Counts::threads` 在非 Linux 上为 0，不做断言；完整运行在这两个平台仍检查其余计数；
- fd、堆的 RSS、分配器内部碎片：不在本测试范围。

对 #979 第 3 项“每实例、每行活着时存了什么，能省的省”的结论：组成与可省项见上，监视任务的 13.4 KiB/实例已在本 PR 省掉；运行中的 `HashMap` 容量余量和每行记录判断为增长代价/按副本必要，不再改。运行时进程与跨语言绑定的残留仍由上面列出的测试守着。
