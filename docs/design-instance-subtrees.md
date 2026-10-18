# 实例键、实例事件与子树永久关闭

状态：实现中；对应 GitHub [#22](https://github.com/arcships/rutis/issues/22)、[#23](https://github.com/arcships/rutis/issues/23)、[#24](https://github.com/arcships/rutis/issues/24)。

核对基准：上游 `a6ce10300f3cd75caa7732e7d4e9e18877a073a0`，以及一个下游应用固定的 rutis vendor。本文描述拟增加的契约，不表示当前代码已满足这些保证。

## 1. 范围与基线

rutis 负责 fiber 身份、服务可见性、事件通道、子树关闭和内部回收。runtime 装配迁移、Session/Branch 业务类型、VENDOR-PATCH 来源更新、真实 Session 压测归消费仓库。rutis 保留自己的 1000 轮资源回收回归测试。

上游与 vendor 的差异必须按实际代码处理：

| 项目 | 当前上游 | 本次处理 |
| --- | --- | --- |
| TypeKey | 已有静态/动态限定名，动态路径持有 Arc，只有 Clone | 保留，增加实例字段 |
| 事件通道 | hooks、wf_hooks、dispatch_tail 已按 TypeKey 索引 | 扩展现有键，不退回 TypeId 二元组 |
| 子插件回收 | mount effect 自摘、依赖索引注销、空事件通道及完成尾链清理已有实现 | 复用；强化永久关闭完成屏障 |
| 子树遍历 | 只有 parent_fiber，没有 children 表 | 增加 Weak 子节点关系，并随终态脱离 |
| 诊断 | 没有 vendor 的 PluginDiagnostics / ServiceAccess | issue 1 包含最小诊断接口，与既有 #13 协调 |
| root shutdown | 已有 Shared.closing 与缓存结果；与 vendor 的子错误汇总不同 | 保留旧接口及既有语义，不直接照搬 vendor |

## 2. 身份和服务键

### 2.1 API

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct InstanceId(NonZeroU64);

impl Ctx {
    pub fn instance(&self) -> InstanceId;
}

impl TypeKey {
    pub fn instance<T: ?Sized + 'static>(id: InstanceId) -> Self;
    pub fn with_instance(self, id: InstanceId) -> Self;
    pub fn instance_id(&self) -> Option<InstanceId>;
}
```

以上为签名草案，省略 import 和实现。`TypeKey` 继续 Clone，不增加 Copy。构造器与取值器名称分开，避免 Rust 同名方法冲突。`with_instance` 消耗并返回原键，保留类型和限定名；若已有实例字段则替换为指定 ID，不产生额外通道层。

```rust
let key = TypeKey::keyed_dynamic::<Database>(name).with_instance(ctx.instance());
```

`of`、`keyed`、`keyed_dynamic`、`Key<T>` 转换仍生成 instance=None。相等和哈希使用 `(type_id, qualifier, instance)`。限定名仍按内容比较。describe 输出依次为 `Type`、`Type#qualifier`、`Type@id`、`Type#qualifier@id`；这是诊断文本，不作为可解析的标识格式。

### 2.2 ID 生命周期

采用创建 fiber 时分配 ID，取代最初的首次调用懒分配。每个成功创建的 fiber（含 root）持有一个进程内唯一 ID，同一 fiber 的 restart/update/依赖重载不改变它。重新建立的子 fiber 得到新 ID。

分配器为进程内单调计数器；字段私有，不提供从整数恢复 ID 的公共构造器。使用 checked 分配；计数耗尽时在登记 fiber/启动 driver 之前以明确的 ID exhausted 原因 panic，不回绕复用，也不为这个不可恢复边界改变现有创建 API。ID 不用于持久化或跨进程协议，不包含 generation。

Ctx 的共享身份元数据保存这个数值，isolate 派生的 Ctx 沿用本 fiber 身份。因此外部保留的 Ctx 在 fiber 结束后调用 instance() 仍返回原 ID，不会重新分配，也不保活 FiberInner。取得旧 ID 不会恢复其注册或访问能力。ID 为 Copy 数值，无需回收数值本身；需回收的是关联的任务和注册记录。

### 2.3 可见性

携带实例的键必须先校验访问方的实际 fiber 祖先链（含自身），再查注册表。祖先链中存在对应 ID 的 fiber 才算在作用域内。可以沿现有 parent_fiber 查找，不另外维护永久的 ID 到服务映射表。

| 操作 | 不在实例子树内 | 位于子树内 |
| --- | --- | --- |
| provide_as / provide_as_with_check | 返回 InstanceOutOfScope，无注册副作用 | 按原类型校验、重复注册、生命周期规则处理 |
| get_as | None，记录越界原因 | 按原 provider 活跃状态和清理期自访问规则处理 |
| resolve_dep | 缺失，不运行该绑定的 check 回调 | 按原门控规则处理 |
| 依赖诊断 | OutOfScope，不暴露外部 provider 信息 | 返回实际已记录的依赖状态 |

新增结构化错误 `InstanceOutOfScope { instance: InstanceId }`。已结束/关闭的调用上下文按生命周期错误拒绝新注册。实例检查不放宽原有生命周期检查。已有 isolate 继续对完整 TypeKey 解析 scope；最终绑定身份仍为 `(provider, generation, key, scope)`。

实例键是运行期可见性规则。它不强制某个 T 必须使用实例键，不区分业务 SessionId/BranchId，也不能撤回此前合法取得的 Arc 服务。业务侧不得据此宣称漏写 ID 会编译失败。

### 2.4 最小诊断

在上游增加 `Ctx::diagnostics()`，复用 vendor DTO 的概念与原始 Arc 错误，不机械复制其旧 Copy 假设。包含 plugin 身份、父节点、状态/代次、声明与解析依赖、绑定，以及 apply 阶段的读取记录。PluginDiagnostics 暴露本 fiber 的 InstanceId。

ServiceAccess 增加明确的 `out_of_scope` 字段。越界检查必须在 lookup 早退前记录；越界记录的 provider/generation 均为空。DependencyStatus 增加 OutOfScope。门控判定和诊断使用同一个可见性判定。

读取记录只在本代 apply 期间采集并去重，下一代清空；Pending 的越界声明通过依赖诊断呈现，不伪造一次 get 调用。diagnostics 不调用 name/injects/check 或驱动任务。check 的状态由正常门控过程记录。读取结果是尽力一致的快照，不宣称全树事务一致性。

复用注册时捕获的 declared_injects，使索引、门控与诊断来自同一份声明。全树遍历使用可清理的 Weak children；在终态脱离前仍可看到节点。watch_diagnostics 和完整观测能力仍可由 #13 后续扩展。

## 3. 实例事件

### 3.1 API 与派发规则

```rust
on_instance<E: Event>(&self, ctx: &Ctx, id: InstanceId,
                     listener: impl Listener<E>) -> Result<Disposer, CordisError>;
emit_instance<E: Event>(&self, ctx: &Ctx, id: InstanceId,
                       event: Arc<E>) -> Result<(), CordisError>;
serial_instance<E: Event>(&self, ctx: &Ctx, id: InstanceId,
                         event: &E) -> impl Future<Output = Result<Option<E::Value>, CordisError>>;
parallel_instance<E: Event>(&self, ctx: &Ctx, id: InstanceId,
                           event: Arc<E>) -> impl Future<Output = Result<(), CordisError>>;
```

on/emit 为同步准入；serial/parallel 在首次 poll 时准入，未 poll 的 future 不算已接纳。emit 的 Ok 只表示入队，回调错误继续送 ErrorSink。serial 保持短路语义，parallel 保持聚合错误语义。既有无实例 API 的签名和派发模式不变。

本设计补充规定：实例事件的注册者和发射者都必须在目标实例子树内，且使用该 root 的总线。跨子树业务通知应通过共同祖先的业务服务完成。校验失败返回 InstanceOutOfScope，关闭后返回 Closed；无监听器也先校验，避免关闭后的无效发送假报成功。

不带实例、不同实例、现有 named 通道互不匹配。实例变体内部构造 qualifier=None 的 TypeKey；不增加实例 waterfall 或 named+instance 的公共事件组合 API。内部索引继续使用完整 TypeKey，为旧 named 通道保留语义。

emit 对同一事件类型、同一实例按接纳顺序保序，不同通道独立。接纳的线性化点必须同时完成校验、监听快照和尾链链接，避免并发线程交换快照与排队顺序。serial 仅保证单次调用内的监听顺序；两个并发 serial 调用不共享尾链。

回调传入的 Ctx 仍来自发射者。监听资源的生命周期归注册者，需捕获注册方 Ctx 的代码不得误用回调参数代替它。

### 3.2 已接纳派发的归属

实例派发从准入成功到实际处理结束持有内部派发凭据，关闭流程据此等待。凭据标识目标实例、发射 fiber，以及快照中的监听器注册 fiber。当前按 fiber 保守计数，跨代派发也等到整次快照处理完毕；代次不参与在途计数。实现可以去重计数，但不能只数任务数或只保存最后一个 JoinHandle。

整个派发快照在释放前计为在途；这允许保守地等完整次派发结束。关闭一个子节点时，只关闭该节点及其后代的注册与发送准入，不关闭祖先实例供其他子节点使用的整个通道。关闭节点的监听器不进入新快照；已进入快照的派发先完成。

- emit 由总线持有执行与完成所有权，调用方返回或丢弃结果不取消任务。
- serial 载荷可借用：future 被丢弃时，借用回调 future 一起析构，再释放凭据；不把借用数据转为后台任务。
- parallel 若创建子任务，必须有负责取消/结束并 join 子任务的所有者。调用方 future 被丢弃，凭据也不能早于所有已启动回调结束释放。
- 完成、错误、panic、调用方取消均必须释放凭据，不能留下永久的在途计数。

凭据不把用户任意 spawn 的任务自动纳入总线；业务任务仍由插件登记清理。

### 3.3 关闭时的事件行为

关闭先停止实例事件准入并取消子树 token，再等待已接纳的相关派发完成；之后才能卸载相关监听器和服务。不会为了清空表而直接丢弃或 abort 任意回调。没有让出执行权或不结束的回调可能阻塞关闭；等待超时不表示已关闭。

停止准入后，已接纳回调再发送实例事件也返回 Closed。需要业务终态事件的宿主，应在框架永久关闭之前完成业务结算。

公开登记和实例派发入口在稳定状态下按下表返回错误；同时满足多个条件时从左到右优先判定。普通 `dispose()` 后的 fiber 属于失活，永久 `shutdown()` 后属于关闭。`effect()` 没有实例键；已开始的 Loading 代仍按原有装配与回滚协议完成。

| 入口 | root/子树永久关闭 | fiber 已卸载/Disposed | 实例键越界 |
|---|---|---|---|
| `effect()` | `Closed` | `InactiveEffect` | 不适用 |
| `provide_as()` | `Closed` | `InactiveEffect` | `InstanceOutOfScope` |
| `on_instance()` | `Closed` | `InactiveEffect` | `InstanceOutOfScope` |
| `emit_instance()` / `serial_instance()` / `parallel_instance()` | `Closed` | `InactiveEffect` | `InstanceOutOfScope` |

回调可以发起本子树 shutdown 并返回，但不得 await 包含自身的关闭结果；apply、finalizer 也不得等待包含自身的关闭屏障。示例与 API 文档明确这一约束。不承诺自动检测跨用户任务形成的任意自等待环。

监听器单独 disposer 或普通重载也必须排干其旧代实例派发引用，之后再完成注销，避免实例回调继续使用已释放的插件资源。同一回调不得等待注销自身的 disposer。这一约束只增加在新实例事件路径，旧无实例事件维持原契约。

## 4. 子树永久关闭与回收

### 4.1 API 和准入

```rust
impl FiberView {
    pub fn shutdown(&self) -> BoxFuture<'static, Result<(), Arc<CordisError>>>;
}
```

对 root view 调用时委托现有 Ctx::shutdown；Ctx::shutdown 从子 Ctx 调用仍关闭整个 root。非 root shutdown 不设置 Shared.closing，不影响兄弟子树。普通 dispose/restart/update 继续保留原契约；已进入永久关闭的子树拒绝 restart/update 和任何后续 reload。

新子树关闭使用每 fiber 的单调 closing 状态与缓存完成任务；不新增一套业务 Session 注册表。根共享的准入锁串行化关闭提交与框架登记。关锁内只操作内部状态，不调用插件元数据、factory 或其他用户回调，不跨 await。

调用 shutdown 时，在返回 future 前同步完成：认领/复用关闭任务，固定本次子树成员与拥有关系，关闭成员准入，取消当前代 token，提交独立协调任务。并发创建的子插件要么登记在关闭成员中，要么返回 Closed 终态 view，不能成为遗漏的 driver。

服务和监听器登记的校验与实际插入必须有同一提交边界。load 发布新代 token/进入 Loading 也需与 closing 检查互斥，防止取消后又发布一个未取消的新代。所有准入路径统一遵循锁序：根准入锁在最外层；查祖先、状态、children 和表操作采用短临界区；任何用户回调或等待均在解锁后执行。

### 4.2 清理登记仍可进行

子树关闭拒绝新插件、服务、监听器和实例派发，但在途 apply 在 Unloading 前仍可登记已经取得资源的清理。登记要么被当前卸载接管，要么立即执行清理且纳入关闭屏障；不能返回错误后留下无人负责的资源或回滚任务。

这不是允许清理工厂绕过服务/监听器准入。框架各注册 API 都要自己校验 closing，不能仅借用 effect 的检查来保护它们。根关闭原有的公共行为以已有测试为基线；本文不把所有旧 effect 调用一律改为关闭期间成功。

### 4.3 关闭顺序

1. 同步关闭准入、认领子树与取消 token。
2. 等待已接纳的 apply 和相关实例派发退出。服务在必要的清理窗口仍按既有规则可读，不因 closing 一律变 None。
3. 启动依赖驱逐与拥有关系下的清理。属于正在关闭子树的消费者直接终止，不进入 Pending；外部消费者执行原依赖重查/重载。binding 在消费者排干前保留，随后按原四元组和绑定身份移除。
4. 所有清理均尝试执行；收集结果，结束各 driver，排空意图等待者。
5. 移除父级 mount/children、声明索引、绑定记账、实例监听与已结束尾链，释放协调任务中的临时引用。
6. 等 driver 的真实退出和上述回收全部完成后，发布一次公开缓存结果。

“下游先于上游”指先完成依赖消费者的卸载，再移除其依赖绑定；不承诺对插件中任意用户 effect 自动推导全局拓扑顺序。

已经处于关闭中的成员收到 RefreshDeps/Restart/Update 时不能重新 load；Pending 成员可以直接结束，测试只要求关闭提交后不新增一次 Pending 过渡或 apply。

### 4.4 避免等待自己

当前 mount effect 的清理会等待 child.dispose；当前子 driver 的 release_transient 又会 drain mount。严格的 shutdown 不能沿这条路径等待自己的公开完成任务。

将内部“本节点清理完成”与公开“driver 及子树全部退出”分开：driver 完成本节点清理后交付内部结果并退出；独立协调者负责 join、脱离父级记录和公开结果发布。子节点自摘只完成/移除 mount 的拥有记录，不重新执行一个等待自身的关闭回调。

父级已取走 mount 开始清理时，父级等待内部子节点结果；子节点自摘不再等待该 mount 的 drain。相同记录的认领与脱离由单一状态决定，避免清理两次。公开完成结果必须在真正回收后发布，不能靠提前通知等待者规避死锁。

父与子同时关闭时复用已存在的子树清理任务，父级 join 子级内部结果和 driver；不能为同一节点再建一个互相等待的关闭任务。

root shutdown 或普通父级 dispose/restart 遇到已在永久关闭的子树，也接入同一内部完成结果，不能绕开它提前释放相关资源。旧操作自身的返回值、错误路由和 root 可重启规则保持原契约；新子树 shutdown 的强完成屏障不反向强加给所有旧 dispose 调用。

### 4.5 错误归属

依赖边负责等待顺序，不收集第二份错误。拥有关系负责收集直接子节点的清理结果；多个失败保持 Aggregate 结构，单一失败保留原 Arc。正常关闭成功不因为进入 Closed 状态而返回错误。

子树结果缓存在该子树句柄所持的任务中。独立关闭并已从父级脱离的失败子树，不把历史错误无限寄存在常驻父级。父级关闭提交时认领尚未脱离的子节点；这些节点的结果纳入父级本次关闭。认领与脱离在共享准入边界上互斥，已认领结果即使随后从 children 移除也由该次协调者持有。

旧 effect 提前清理错误的现有聚合规则不在本次顺手改变；新永久关闭的 mount 结果应从普通 drained_errors 寄存中分开处理，避免一份失败通过两个路径重复保存。一般历史错误消费仍属于已有 #11。

### 4.6 回收完成的含义

移除父级 mount 与 Weak children；按 declared_injects 注销，空键删除；绑定和 provided 记账清空；实例 hooks 与尾链在相关派发结束后删除；driver 和框架拥有的派发/清理任务全部完成。新建实例不可复用旧 ID。

在外部仍保留 FiberView、Ctx、服务 Arc、诊断快照或错误结果时，相应对象可以继续存活。shutdown 保证内部拥有关系不再保留整个子树，不保证强制销毁外部持有的对象。诊断快照不被框架存档。

父级集合容量沿用稀疏收缩策略；回归检查条目和内部引用回到基线、容量不随历史轮数持续增长，不要求分配器/RSS 精确回到初始值。

## 5. 实施与验收

三项 issue：身份与服务可见性；实例事件及在途归属；子树永久关闭与回收。R3 与 R4 合并交付，不先发布一个提前返回、日后再补回收的 shutdown。

issue 1 建立身份、最小诊断、可清理的 children 和共同准入设施；issue 2 增加实例派发凭据及关闭准入接口；issue 3 接通完整关闭协调。已有 root、动态键和回收逻辑需复用，不能用 vendor 旧源码覆盖上游新能力。

必须保护的流程：

- 两个 sibling 实例、嵌套实例、跨 root 和旧 ID；祖先实例可用，兄弟/外部不可用；类型、限定名、实例均参与匹配。
- 越界提供无残留，越界门控不调用 check；诊断读操作无用户回调；合法进程服务重载影响两个实例，实例服务变更只影响合法消费者。
- 实例事件隔离，emit 顺序，A 阻塞 B 继续；serial/parallel 原分发语义；关闭与发送/注册并发，不接纳迟到事件。
- 在途 emit、借用 serial 被 drop、parallel 被 drop、回调失败/panic，相关派发退出后关闭才完成；回调发起关闭后返回可正常收敛。
- Loading 时关闭，迟到资源清理被接管；close 与新子节点/提供/监听/重载竞争均无遗漏。
- 内部消费者先结束且不重载；外部消费者 Pending 后可恢复；父子并发关闭、dispose/shutdown 竞争、丢弃等待者后再次 join。
- 错误按拥有边汇总一次，共享错误 Arc；独立关闭的历史失败不随子树创建次数被父级持续保留。
- 长寿 root 上 1000 轮公开 API 创建和关闭，包括有常驻兄弟的场景；释放外部句柄后检查 Weak/Drop、内部表项、任务数和容量趋势。

复用 tests/contract.rs、parity.rs、event_keys.rs、transient_release.rs，必要时增加实例与子树流程测试。内部计数仅作为测试观测辅助。vendor 的 lifecycle_diagnostics.rs 尚不在上游；迁入适用的框架流程断言，消费仓库回灌时再执行其完整原测试，不能把未运行的 vendor 测试写成上游验证结果。

实施后的必要命令：

```sh
cargo +1.98.1 test --offline -p rutis
cargo +1.98.1 clippy --offline -p rutis --all-targets -- -D warnings
cargo +1.98.1 fmt --all -- --check
```

真实 Session 的性能阈值、runtime 迁移和 vendor 来源 commit 更新由下游应用的迁移任务验收。
