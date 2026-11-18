# 手段复核：#622 跟踪表里只有具名用例的规则（#1270）

[English](means-0.23.en.md) · [中文索引](README.md) · [复核表](recheck-0.22.md) · [保证登记表](../guarantees.md) · [质量规范](../quality-standard.md)

日期：2026-11-18。基准：`origin/main` `31047fc4`。议题：[#1270](https://github.com/arcships/rutis/issues/1270)（父 [#622](https://github.com/arcships/rutis/issues/622)）。本文件是这些规则「用什么手段、为什么」的现行记录，逐条结论与验收命令都在这里。

## 1. 两个口径

#622 的完成条件写的是：应有形式化检查的规则在追踪表里都是「已覆盖」（有变异或修复前的配置证明它不是空的），**只靠测试的写上测试名**，**改为其他手段的写明理由**。

- **A 口径**（#1265 引入、#1269 用的 `upgrade`）：语义条目可以带 `upgrade`，证据是模型、属性测试或具名用例，只要 `path::name` 在树里解析、写了变异，就能从「未验证／部分验证」升到「已验证」。这 21 条已按 A 口径全部登记。
- **B 口径**（#622 原文的「手段」列）：手段写 `SH`、`PT`、`TLA`、`TV`、`K` 的规则，要有对应的形式化检查；只有具名用例算「部分」，或者把手段改成测试并写明理由。

A 口径的「已验证」不等于 B 口径的「已覆盖」：一条规则可以因为用例断言得完整而「已验证」，同时它的手段列仍写着 `SH`。本文件按 B 口径逐条决定手段，这就是 #1270 要补的那一栏。

「手段」列是 #634 issue 快照的十列之一，在 `rules/protocol-*.md` 里原样导出，导出文件不手改（[README](README.md) §5）。所以手段的现行结论写在两个地方：

- 本文件的 §2（逐条决定与理由）；
- `tools/formal-trace-semantic.mjs` 里同编号条目的 `why`/`whyEn`，它生成到[复核表](recheck-0.22.md) §5 与[英文复核页](recheck-0.22.en.md)，`node tools/formal-trace.mjs --check` 核对同编号条目与证据。

## 2. 逐条

issue 正文把这批写成 21 条，其中 `loader/rows-gate-on-services a/b` 是一个名目下的两条规则。按编号拆开是 **22 条：7 条 loader／host 集成层 + 15 条内核／事件**。

- 「现在有什么」= 追踪表里已登记的证据（`tools/formal-trace-semantic.mjs` 的 `upgrade.evidence`）；
- 「手段列」= #634 issue 快照写的；
- 「决定」= 本文件定的 B 口径手段；
- 验收命令都从仓库根目录跑；`RUTIS_TEST_RUNTIMES=node,python` 是 loader／host 的运行时用例的门（未设置时用例直接返回，等于没跑）。

### 2.1 七条 loader／host 集成层：改成「测试」

| 规则 | 现在有什么 | 手段列 | 决定 | 理由（详见 §3） |
| --- | --- | --- | --- | --- |
| `loader/rows-gate-on-services a` | `rutis-loader/tests/scope.rs::inject_gates_a_row`、`rutis-loader/tests/runtime_rows.rs::rows_resolved_offline_wait_for_what_they_inject`、`rutis-loader/tests/row_inject_gate.rs::a_runtime_rows_inject_gates_it_and_is_leased_to_it`、`rutis-loader/tests/loader.rs::provider_update_reloads_consumers` | `SH + PT` | **测试** | 门控本身是内核的状态机（`core/D14`、`core/D22`，已有内核模型）；这一行多出来的是 loader 把名字放进闸门、租进运行时进程，只有真实运行时才看得见 |
| `loader/rows-gate-on-services b` | `rutis-loader/tests/multilang.rs::a_provider_that_goes_stops_only_its_users`、`row_inject_gate.rs::a_runtime_rows_inject_gates_it_and_is_leased_to_it`、`runtime_rows.rs::a_plugin_that_disposes_itself_ends_its_row`、`loader.rs::provider_update_reloads_consumers` | `SH + TLA` | **测试**（TLA 那一半转指 #625 的 `WithdrawBeforeUnload` 设计模型） | 撤销停住内核门控下的消费者是内核模型 9 的事；这一行断言的是「只停这个运行时里用它的行、其他行继续」，跨进程 |
| `loader/row-unload-leaves-nothing a` | `runtime_rows.rs::rows_unload_before_the_runtime` | `SH` | **测试** | 顺序靠运行时进程把 cleanup 送进 Cordis 的先后断言；替身进程测的是替身 |
| `host/dev-reload b` | `rutis-host/tests/dev.rs::dev_follows_a_node_project`、`::dev_follows_a_python_project`、`::dev_follows_a_bun_project`（三者走 `the_dev_loop`，fixture 记 cleanup，断言被替换的实例恰好清理一次）、`rutis-host/tests/reload.rs::rows_added_removed_and_changed_apply` | `PT + SH` | **测试** | 一次 dev 重载是真实的构建 + 进程换代；#627 的宿主模型覆盖重载监督，覆盖不到「换代码后旧实例清理一次」 |
| `MNT-5-3` | `examples/dsh-baseline/tests/host.rs::persona_uses_the_system_prompt_the_rutis_host_provides` | `SH` | **测试** | 生成物 `injects` 的门控发生在真实挂载与真实子进程上；内核门控已经有模型 |
| `MNT-5-7` | `rutis-bridge/tests/mount_host_service.rs::withdrawing_a_host_service_closes_the_mounts_process`、`examples/dsh-baseline/tests/host.rs::persona_uses_the_system_prompt_the_rutis_host_provides` | `SH` | **测试** | 规则的一半是「Node 进程随之关闭、不做就地替换」，判据是进程身份与结束状态，`SH` 替换不到的操作系统对象 |
| `MNT-11-4` | `runtime_rows.rs::rows_wait_for_their_hosts_not_the_runtime`、`instance_runtimes.rs::a_language_rows_inject_inside_an_instance_is_the_instances`、`row_inject_gate.rs::a_runtime_rows_inject_gates_it_and_is_leased_to_it`（本 PR 补了替换那一半）、`instance_runtimes.rs::a_rows_instance_host_service_going_stops_it`（本 PR 新加）、内核模型 9 | `SH` | **测试** | 同一份 dispatcher 的键在全局是 `host_key`、实例里是 `host_key_in`；等待与停止是内核规则，loader 这边要断言的是它把 dispatcher 放在正确的键、作用域上 |

### 2.2 十五条内核／事件层：模型、PT 与写明的测试

| 规则 | 现在有什么 | 手段列 | 决定 |
| --- | --- | --- | --- |
| `core/D7a` | 模型 28（本轮新增，`rutis/src/fiber/shuttle_tests.rs::shuttle_an_unload_cancels_and_cleans_up_before_it_disposes`：内核自己的 `unload`，Unloading 先于清理、清理看到取消、LIFO、Disposed 最后）、模型 26、`rutis/tests/lifecycle_order.rs` 两个用例、`rutis/tests/interleave.rs` | `PT + SH` | **模型 + 测试**：`PT` 是 `interleave.rs` 的顺序参考模型；`SH` 是模型 28 与 26；「等 apply 退出」由 `lifecycle_order::unload_waits_for_apply_to_exit_before_cleanup` 断言（公共 `FiberView`，shuttle 驱动不了） |
| `core/D7b` | 模型 12、23、9；`rutis/tests/evicted_apply.rs` 四个用例；`rutis/tests/load_window.rs` 三个用例 | `SH` | **模型 + 测试**：dispose／restart／驱逐由模型 12、23、9 承担；`update` 与 `restart` 共用临界区，update 自己的 dry-run 与两次终态检查没有模型，那半边记测试（`load_window::an_update_right_after_mounting_is_taken_up_on_current_thread`、`evicted_apply::updating_a_cancelled_loading_consumer_runs_one_apply_without_failing`） |
| `core/D14a` | 模型 9；`rutis/tests/contract.rs::eviction_order`；本轮新增 `rutis/tests/contract.rs::eviction_drains_every_consumer_before_the_provider`（两个消费者都回 Pending、两次清理都先于提供方；变异实测）；`instance_subtrees::closing_subtree_disposes_consumer_before_provider` | `SH` | **模型 + 测试**：模型 9 覆盖一个消费者的停住；「≥2 个消费者并发排干」由新用例断言 |
| `core/D14b` | `rutis/tests/contract.rs::self_access_during_cleanup` | `PT` | **测试**：确定性行为（清理里读自身服务），模型能加的只有替身；变异是让 `Ctx::get` 在 Unloading 时拒绝 |
| `core/D21a` | 模型 19；`rutis/tests/contract.rs::single_service_evict`、`::isolate_no_cross_evict`；`rutis/tests/parity.rs::distinguishes_provider_incarnations_without_a_global_counter`；注册槽 PT | `SH + PT` | **模型 + PT**：模型 19 覆盖窗口里的 restart；「同键不同代并存」在当前设计里不可达（一个作用域一个键一个绑定），不硬造模型，边界写在登记表 |
| `core/D30c` | `rutis/tests/contract.rs::cleanup_panics_contained` | `PT` | **测试**：`catch_unwind` → `PluginFailed` 是确定性行为，模型 13 的清理只返回 `Err` |
| `core/D31b` | `rutis/tests/event_order.rs::emit_after_listener_panic_is_still_delivered`；`rutis-bridge/src/events/hub/shuttle_tests.rs::a_cancelled_or_panicking_emit_does_not_stop_the_queue` | `SH` | **测试 + 宿主队列模型**：内核 emit 链的模型这一轮没写成——在 spawn 替身下驱动真实 emit 链时，同名两次 emit 的监听器开始顺序在个别调度下反了（去掉 panic 监听器也能复现），是替身组合还是真的乱序没有定论，见 §5；因此不作为 SH 证据 |
| `events/remote e` | 模型 10；`rutis/tests/events.rs::raw_dispatch_and_entry_calls` | `SH + PT` | **模型 + 测试**：`emit` 完成时机是模型 10；`invoke_entry` 是按 id 查找的确定性行为，用例断言 |
| `events/framework c` | 模型 24；K 的 `rutis/service`、`rutis/service-of-rust`、`rutis/service-scoped` | `K + SH` | **K + 模型**：K 横跨七种实现，是主证据；模型 24 断言同一绑定的 `Provided` 先于 `Removed`；「每个宿主服务」是代表性覆盖 |
| `instance/subtrees.5` | 模型 16、17；`rutis/tests/instance_subtrees.rs` 两个资源用例；`rutis/src/fiber/transient_tests.rs` 两个；`rutis/tests/transient_release.rs::churn_on_long_lived_root_releases_instances` | `SH + PT + 测试` | **模型 + 测试**：手段列本来就写了测试，任务、fiber、容量、根侧记账都是资源检查；模型 16、17 覆盖并发关闭；`PT` 没有单独属性测试 |
| `EVP-7.1-5` | 模型 6；K `unload/waits-and-cancels`；`node/rutis/test/events.test.mjs::unload aborts ctx.signal and waits for running listeners`；`rutis/tests/events.rs::unload_cancels_and_waits_for_running_listeners` | `K + SH` | **K + 模型**：K 一行横跨七种实现；模型 6 是在途计数的内核部分 |
| `EVP-12.1-6` | `rutis-bridge/tests/event_hub.rs::dropping_a_dispatch_cancels_the_remote_listener_it_started`、`::dispatches_cross_between_host_and_runtime`；`rutis/tests/events.rs::remote_entries_take_part_like_any_listener` | `SH + TV` | **测试 + TV（未建）**：两个方向的确定性用例是现行证据；`TV` 归 #624 的 trace 验证器（事件宿主的动作日志仍未建），这一条没有单独的 SH 模型 |
| `EVP-12.1-10` | 模型 10；`rutis/tests/events.rs::remote_entries_in_synchronous_modes`；`rutis/tests/waterfall_sync.rs::sync_waterfall_chains_listeners_into_the_terminal`；`rutis/src/bus/prop_tests.rs::the_five_dispatch_modes_match_a_reference_interpreter` | `SH + TV` | **模型 + PT + 测试；TV 归 #624**：`dispatch_sync` 与 waterfall-sync 由用例断言，五种分发方式有参考解释器差分 |
| `INS-6-1` | `rutis/tests/instance_subtrees.rs::closing_a_subtree_withdraws_the_instance_keys_it_provided`、`::instance_keys_enforce_subtree_and_explain_pending_dependency`、`::a_recreated_child_gets_a_new_instance_id` | `SH` | **测试**：实例键带 id、子树关闭撤销它是确定性行为；并发子树关闭由模型 16、17 覆盖（那属于 `instance/subtrees.5`，不是这一行） |
| `MSV-4-2g` | `rutis-bridge/src/marked/shuttle_tests.rs::shuttle_a_marked_service_is_served_after_its_native_and_withdrawn_before_it`；`rutis-loader/tests/marked_services.rs::rows_of_every_language_call_a_marked_rust_service` | `SH + TLA` | **模型 + 测试**：模型 1 覆盖撤销顺序；消费者停止那一半跨三种语言，用例断言；`TLA` 那一半没有落地模型 |

## 3. 七条 loader／host：为什么是测试

### 3.1 内核式 shuttle 在这七条上只能测到替身

`SH` 的做法是把执行器、锁和时钟换成可控实现，让一段**进程内**代码在不同调度下重跑（质量规范 §7.1、§7.3）。这七条的判据都不在进程内：

- 规则说的是**跨进程**的事：目录里的名字被租进 Node／Python／Bun 的进程、挂载的 Node 进程随宿主服务撤销而结束、dev 重载换掉的是另一个进程里的代码。这些要用真实子进程才看得见；给 `Process`／`Launcher` 写个替身，模型检查的是替身的行为，不是「真实运行时确实这样」。
- 质量规范 §7.1 已经写明 `SH` 证明不了「没被纳入探索的外部事件（真实进程、操作系统、运行时自身）」。这七条恰好落在那一栏里。
- #627 已经把能接上 `SH` 的那部分接上了：`rutis-loader` 的 3 个模型（二进制运行时的重启、替换、被要的行）与 `rutis-host` 的 3 个模型（重载移除运行时后等它自己的进程）测的是**监督**，不是门控与退出在这几个 crate 里的接线。

所以 #622 完成条件的第三项（改为其他手段并写明理由）适用于这七条：手段写「测试」，测试名写进追踪表，变异写进 `upgrade.mutation`。

### 3.2 每条的变异（测试抓什么）

| 规则 | 变异（这些测试会失败） |
| --- | --- |
| `loader/rows-gate-on-services a` | 在 inject 解析前启动行（`transitions.loads` 大于 1）；或不把声明的依赖放进闸门（行在服务到达前就 Active） |
| `loader/rows-gate-on-services b` | 撤销时把整个运行时的行都停掉（其他行不再 Active）；或服务回来后不重新启动（行停在 Pending） |
| `loader/row-unload-leaves-nothing a` | 先卸运行时行再卸行（`c: bye` 落在进程关闭之后） |
| `host/dev-reload b` | 跳过旧实例的卸载（cleanup 0 次）或卸载两次（2 次） |
| `MNT-5-3` | 把宿主服务从生成的 `injects` 里去掉（挂载在提供前就 apply，先看不到 Pending） |
| `MNT-5-7` | 挂载的清理不关闭进程（`first.ended()` 不成立）；或撤销只停用服务、不卸挂载（视图仍 Active） |
| `MNT-11-4` | 把 dispatcher 提供到别的键（行永远不启动）；撤销时不撤下它（行保持 Active）；替换后仍用旧绑定（新值读不到）；实例里的 dispatcher 撤下而行不停 |

### 3.3 本 PR 补的两处断言

`MNT-11-4` 的「跟随替换」与「实例里的等待／停止」此前没有单独断言，这次补上，都是用例而不是模型：

- `rutis-loader/tests/row_inject_gate.rs::a_runtime_rows_inject_gates_it_and_is_leased_to_it`：宿主服务撤下后再提供一个**不同的** dispatcher，两行重新启动并读到新值（`43`，不再是 `42`）。变异：替换后仍用旧绑定——断言读到 `42`，用例失败（实测）。
- `rutis-loader/tests/instance_runtimes.rs::a_rows_instance_host_service_going_stops_it`：禁用一个实例组里的服务提供行，两个实例的 `reader` 都停下并等该名字（`loader.waits` 含 `js_timeline`），重新启用后两行再启动、重新读到该实例自己的时间线。变异：撤下后行不停——`!active` 断言失败（实测，见 §4）。

## 4. 验收

从仓库根目录：

```sh
node tools/check-guarantees.mjs                       # 219 行、1232 个标记、0 problem
node tools/formal-trace.mjs --check                   # 0 problem

# 七条 loader／host 的证据
RUTIS_TEST_RUNTIMES=node,python cargo test -p rutis-loader --features runtimes \
  --test scope --test runtime_rows --test row_inject_gate --test loader \
  --test multilang --test instance_runtimes
RUTIS_TEST_RUNTIMES=node,python cargo test -p rutis-bridge --test mount_host_service
RUTIS_TEST_RUNTIMES=node,python cargo test -p rutis-host --test dev -- --test-threads=1
RUTIS_TEST_RUNTIMES=node,python cargo test -p rutis-host --test reload -- --test-threads=1

# 十五条内核／事件规则（第二个 PR）
RUSTFLAGS="--cfg rutis_shuttle" cargo test -p rutis --lib shuttle_ \
  # 含模型 28 `shuttle_an_unload_cancels_and_cleans_up_before_it_disposes`
cargo test -p rutis --test contract eviction_drains_every_consumer_before_the_provider
cargo test -p rutis --test lifecycle_order --test evicted_apply --test load_window --test event_order
cargo test -p rutis --test instance_subtrees --test waterfall_sync --test parity

# `MNT-5-3` 的 dsh 用例在 nightly 跑；要 node/baseline 的钉住版本，
# 且 examples/dsh-baseline 的 Cargo.toml 先去掉 mount 的 `version` 钉（nightly.yml 的做法）：
npm --prefix node/baseline ci
awk '/^\[/ { mounts = /^\[package\.metadata\.rutis-cordis\.mounts\./ } !(mounts && /^version = /)' \
  examples/dsh-baseline/Cargo.toml > /tmp/dsh-Cargo.toml && cp /tmp/dsh-Cargo.toml examples/dsh-baseline/Cargo.toml
RUTIS_TEST_RUNTIMES=node cargo test -p dsh-baseline
```

`rutis-host` 的两个文件要 `--test-threads=1`：本机并行跑时三个 `dev` 用例会同时构建 TS 而互相打断（与本次改动无关，串行跑 10 个全过）。

## 5. 还没有做的

- **内核 emit 链的 panic 模型没写成**（`core/D31b`）。第一次尝试在 `SpawnScope` 下驱动真实的 `Ctx::emit`（`emit_erased` 的 tail 等待与 start loop 都是内核的），结果同名两次 emit 的监听器开始顺序在个别调度下反了（例如 4 次 emit 收到 `[0, 2, 1, 3]`），而且**去掉 panic 监听器也能复现**（300 次随机 + 300 次 PCT 里出现一次）。成因没有定论：要么 `emit_erased` 在「持 bus 的 std 锁调用 `crate::sync::spawn`、而 scope 可能就地运行另一个任务」下会乱序，要么是替身与 bus 锁的组合不成立（换成 shuttle 的锁以后死锁消失，但那次实验里 panic 一度被报成任务失败）。在定论之前不把这条链的模型当证据，单独复现后再补；复现方式是上面的 `shuttle_` 测试加一个小模型（同名 emit 两次、两个监听器）。
- `EVP-12.1-6` 与 `EVP-12.1-10` 的 `TV` 那一半仍是 #624 的遗留：事件宿主的动作日志没有建，现有 trace 验证器只覆盖会话通道与等待图。
- `events/remote e` 的 `PT` 那一半与 `instance/subtrees.5` 的 `PT` 那一半没有独立属性测试，本文件不把它们写成「已覆盖」。
- `core/D7b` 的 `update` 自己（dry-run、两次终态检查）没有模型，记在测试上。
- `MNT-5-3` 只有 nightly 上的一个宿主服务，跨实现（Node、Bun、Python、Go、Swift、Kotlin）的同一用例没有建，理由写在追踪表里。
- `loader/rows-gate-on-services b` 的 TLA 那一半指向 #625 的 `WithdrawBeforeUnload`（设计模型），不是本仓库 `specs/` 里的模型；要写成「TLA 已覆盖」得先有落地模型。
