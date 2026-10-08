# 按需创建插件实例

[English](design-loader-instance-mounts-and-services-2026-10-07.en.md)

设计提案 · 关联 [#158](https://github.com/arcships/rutis/issues/158)、[#159](https://github.com/arcships/rutis/issues/159) · 代码基线 `330ff51`

## 1. 背景

应用需要自己管理自己的插件：装哪些插件、配置是什么，都由一份配置决定，并能在运行中动态修改。

有些插件需要按业务对象各开一份，实例何时出现由业务决定。例如每个会话一组插件，会话里每个分支再一组。这是插件体系的通用问题（参见 IntelliJ 的 project 级服务、依赖注入框架的 scoped 生命周期）。框架不支持时，只能由插件自己维护按对象划分的状态，或由宿主另写一套装配层；两者都绕开了框架的依赖、生命周期、重载与诊断。

rutis 内核已经提供实例子树、实例键和子树永久卸载。本设计补上 loader 侧：用配置声明每个实例里装哪些插件，并在配置变化时同步到所有实例。

分工：

- **业务**决定实例什么时候创建、什么时候关闭；
- **loader**决定每个实例里装哪些插件、用什么配置。

本设计不修改 rutis 内核，也不在框架中引入业务分类。

## 2. 配置

只增加一个属性：**分组可以声明 `instanced: true`**。

- 普通分组（来自 Cordis）由 reconcile 在父级下装载一次，子行装在分组的 ctx 里。
- `instanced` 分组不由 reconcile 装载，只通过 `create_instance` 创建，可以有多个实例。每个实例是一个独立的分组 fiber，子行在每个实例里各装一份。

```yaml
- id: session
  group: true
  instanced: true
  config:
    - id: session-scope
      name: dim/session-scope
    - id: tool-registry
      name: dim/tool-registry
    - id: timeline-tree
      name: dim/timeline-tree
    - id: branch
      group: true
      instanced: true
      config:
        - id: branch-scope
          name: dim/branch-scope
        - id: loop
          name: dim/loop
        - id: tool-executor
          name: dim/tool-executor
- id: aimux
  name: dim/aimux
```

规则：

- 实例的位置由配置树决定：实例建在其配置父级的 ctx 下。顶层的 `instanced` 分组建在 loader 的 ctx 下；嵌套的建在它所属的那个外层实例里。
- 实例里的普通行和普通分组自动装载；嵌套的 `instanced` 分组需要再调用 `create_instance`。
- 同一实例里的插件是平级关系，相互关系由 `injects` 表达。例如 `tool-registry` 依赖 `session-scope` 提供的服务，会等它就绪；`loop` 依赖全局的 `aimux`。
- 不在任何 `instanced` 分组之下的行，行为与现在相同。
- `instanced` 只能用在分组上；写在插件行上，该行无效。

## 3. 接口

```rust
let s1 = loader.create_instance(&ctx, "session").with(assembly).await?;

// 在 session 实例里的任何插件中，例如 timeline-tree 创建分支：
let b1 = loader.create_instance(ctx, "branch").await?;

loader.remove_instance(b1.plugin).await?;
```

### 3.1 `create_instance(&ctx, group_id)`

- **父实例由 ctx 确定**：从 `ctx` 所属的受管插件开始，沿 loader 记录的父子关系向上，找到该分组配置父级的实例，作为父实例。顶层分组可以向上走到 loader 为止；不是运行中受管插件的 ctx 只能用于顶层分组，且必须是 loader 的 ctx 或其祖先 ctx（无关分支、已结束插件的 ctx 都会被拒绝）。找不到时报错。
  - 因此可以在父实例内任何受管插件里调用，不必拿到父实例本身的 ctx。
  - loader 只沿自己记录的关系查找，不依赖内核的祖先查询。
- 目标必须是 `instanced` 分组，且未禁用、有效；否则报错。
- `.with(value)` 附带一个业务值（任意 `Send + Sync` 类型），交给实例内插件的工厂（§4）。
- 返回前等待：实例的子行稳定。返回 `Instance { plugin, view, report }`，`report` 按树序列出实例里每一行（不含嵌套的 `instanced` 分组）的 `(行 id, InstanceResult)`：

| 结果 | 含义 |
| --- | --- |
| `Active` | 已运行 |
| `Waiting` | 在等依赖的服务，或它在实例内的上级分组在等 |
| `Failed(error)` | 解析、校验、表达式求值或 `apply` 失败（本行，或它在实例内的上级分组） |
| `Skipped` | 子行或它在实例内的上级分组被禁用 |

- 子行失败不影响实例创建，实例是否可用由调用方根据 `report` 判断。实例的分组 fiber 本身失败时返回错误，并移除该实例。
- 可以在插件自己的 `apply` 中调用并等待。

### 3.2 关闭实例

- `remove_instance(plugin)` 卸载该实例及其子树。
- 业务直接关闭实例的 fiber（例如 `FiberView::shutdown()`）也可以，loader 观察到卸载后只更新记账，不重复卸载。
- 实例随创建它的分组 ctx 存亡：父实例关闭，或该 ctx 被重建、卸载（例如所在分组的 `isolate` 改变）时，其中的实例随子树卸载并移除（`InstanceRemoved`），由创建它们的插件或业务在新 ctx 中重新创建。
- 实例不写入配置。进程重启后由业务重新创建，例如按数据库恢复会话时逐个 `create_instance`。

## 4. 工厂

实例里的插件常需要知道自己在哪个实例里，用来生成实例键。

```rust
builtins.register_with::<LoopConfig, _, _>("dim/loop", |build: &Build| {
    let scope = BranchScope {
        session: SessionScope { instance: build.instance("session")? },
        instance: build.instance("branch")?,
    };
    Ok(LoopFactory::new(scope))
});
```

- `Build` 提供：
  - `instance(group_id)`：所在实例链上该分组实例的 `InstanceId`，即那个分组 fiber 的 `ctx.instance()`；
  - `value::<T>()`：实例链上最近一次 `with` 提供的该类型的值；
  - `instances()`：整条实例链，由内向外。
- 同一实例里的插件都在该分组 fiber 的子树内，因此用这个 `InstanceId` 生成的实例键对它们都可见。提供实例服务的插件（如 `session-scope`）也用这个 id，而不是自己 fiber 的 `ctx.instance()`。
- 普通 `register` 的插件不变，在每个实例里使用同一个工厂。
- `register_with` 登记的插件不在任何实例里时无法装载（行为 `Unresolved`）。
- `Resolved` 新增 `scoped: Option<ScopedFactory>`，按 `Build` 生成每个副本的工厂；解析缓存仍按模块名，不随实例增长。
- loader 不改写工厂返回的 `injects()`。

## 5. 动态更新

对配置行的修改作用于这一行的全部实例：

| 修改 | 结果 |
| --- | --- |
| 在 `instanced` 分组中新增行 | 在该分组的每个实例里装载 |
| 删除或禁用实例中的行 | 在每个实例里卸载 |
| 修改实例中行的配置 | `injects()` 不变时在原 fiber 上更新；改变时建立新 fiber，rutis 沿依赖重载下游 |
| 禁用或删除 `instanced` 分组 | 关闭它的全部实例；之后 `create_instance` 被拒绝 |
| 修改 `instanced` 分组的 `isolate`、`inject` | 按普通分组规则重建每个实例，沿用创建时的 `with` 值；其中创建的实例被移除（§3.2） |
| `rename_module`、`reload` | 全部实例 dry-run 后一起应用；任一新失败则全部回到旧模块 |
| volatile / overlay 层 | 同样作用于全部实例 |

- 编辑先在全部实例上 dry-run（编辑分组时，对其下每个插件按分组将给出的上下文 dry-run），任一失败则整次编辑回滚，不写可编辑层，旧代继续运行。
- 新创建的实例按当前配置装载。
- 是否在运行中修改、何时修改由应用决定。下游如何处理重载由各插件按自身合同处理。

## 6. 管理与诊断

| 操作 | 对行 id | 对实例（`PluginId`） |
| --- | --- | --- |
| `update`、`set_disabled`、`set_inject`、`set_isolate`、`rename_module`、`move_to`、`remove`、`reload` | 按 §5 作用于全部实例 | — |
| `restart` | 重启该行的全部副本 | `restart_instance`：只重启这一个 |
| `get` | 行条目 | — |
| `locate(plugin)` / `row(instance)` | — | 返回所属行 id |

- `entries()` 按树序列出行；在实例里的行，后面跟着它在每个实例中的条目。`instanced` 分组即使没有实例也可见。
- `EntryInfo` 新增 `instance: Option<InstanceInfo>`，包含所在实例的 `PluginId` 与实例链。
- `LoaderChanged` 新增 `InstanceCreated { group, plugin }`、`InstanceRemoved { group, plugin }` 与 `Stopped { id, instance, plugin }`。
- 实例中的插件 `dispose_self`：只停这一份，状态为 `EntryStatus::Stopped`，`EntryInfo.plugin` 保留它最后的 fiber，不写配置；`restart_instance(plugin)`、对该行的修改或实例重建会让它重新启动。不在实例中的行，`dispose_self` 不变。

## 7. 以 dim-agent 为例

| dim-agent 现状 | 使用 loader 后 |
| --- | --- |
| `SessionScopePlugin` 用自己的 `ctx.instance()` 提供 Session 服务 | `session` 分组里的 `session-scope` 行；用 `build.instance("session")` 提供 |
| `session_plugin(\|scope: &SessionScope\| P)`，在 SessionScope Active 后 `install` | `session` 分组里的行；工厂用 `build.instance("session")`；靠 `injects` 等 `session-scope` 就绪 |
| `timeline_plugin(\|scope: &BranchScope\| P)` | `branch` 分组里的行；工厂用 `build.instance("session")`、`build.instance("branch")` |
| `ctx.plugin(SessionScopePlugin(assembly))` 后 `wait_active` | `create_instance(&ctx, "session").with(assembly)`，按 `report` 判断，再照旧 `validate` |
| `TimelineFactory::build` 中建 BranchScope 并 `wait_active` | `create_instance(ctx, "branch").with(branch_assembly)` |
| 关闭：`FiberView::shutdown()` | 不变 |
| 能力集合在发布前固定 | 由应用决定是否在运行中修改配置 |

## 8. 不在范围

- **实例内的跨语言插件与按名字的实例服务**（#159）：第二期，在本设计的实例链上解析名字。
- **只对某个实例生效的覆盖**（#158 需求 8）：overlay 仍是全局的。
- **Windows**：见 #160。

## 9. 验收

- 一份配置同时管理全局行与两层 `instanced` 分组；创建、关闭、再创建后运行态与配置一致。
- 两个实例里的同名插件按各自实例键提供和读取服务，互不可见；内层实例的插件能读取外层实例的服务；同一实例内的插件靠 `injects` 等待彼此。
- 在实例内的任意受管插件中以其 ctx 调用 `create_instance` 能找到正确的父实例；配置父级不匹配时报错。
- `report` 正确报告 `Active`、`Waiting`、`Failed`。
- 新增、删除、修改、`reload` 实例中的行立即作用于全部实例；`injects()` 改变时下游随之重载；任一实例 dry-run 失败时整次编辑回滚。
- 修改 `instanced` 分组的 `isolate` / `inject` 时实例重建，`with` 值保留。
- 实例中的插件 `dispose_self` 只停这一份，可恢复；配置不变。
- 关闭实例后，其子树、记账与诊断条目全部清除；多轮创建与关闭不增长。
- 用存下来的配置在新 loader 里 reconcile 并重新创建实例，得到相同运行态。

## 10. 实现

| 位置 | 内容 |
| --- | --- |
| `crates/rutis-loader/src/loader/mod.rs` | 运行记录按 `Slot { row, scope }` 记录：`scope` 为所在实例编号，实例外为 `None` |
| `crates/rutis-loader/src/loader/instances.rs` | `create_instance`、`remove_instance`、`restart_instance` 与 `report` |
| `crates/rutis-loader/src/loader/reconcile.rs` | 按副本装载、更新、重建实例、清理失效实例，`dispose_self` 的三种处理 |
| `crates/rutis-loader/src/loader/commit.rs` | 编辑在全部副本上 dry-run |
| `crates/rutis-loader/src/resolver.rs` | `Build`、`ScopedFactory`、`Builtins::register_with` |
| `crates/rutis-loader/tests/instances.rs` | §9 的验收 |

实现依据：[运行表](../crates/rutis-loader/src/loader/mod.rs)、[实例](../crates/rutis-loader/src/loader/instances.rs)、[装载与清理](../crates/rutis-loader/src/loader/reconcile.rs)、[更新流程](../crates/rutis-loader/src/loader/commit.rs)、[Resolver](../crates/rutis-loader/src/resolver.rs)、[实例服务键](../crates/rutis/src/key.rs#L166)。

状态：已实现（`rutis-loader`），跨语言部分见 §8。
