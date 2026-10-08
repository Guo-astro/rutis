# 实例内的服务名

[English](design-instance-services-2026-10-08.en.md)

设计提案 · 关联 [#159](https://github.com/arcships/rutis/issues/159) · 依赖 [#161](https://github.com/arcships/rutis/pull/161)（`instanced` 分组）

## 1. 背景

#161 让 loader 按需创建实例：`instanced` 分组的每个实例是一个分组 fiber，组内的行在每个实例里各装一份。Rust 插件可以用 `build.instance("session")` 拿到实例的 `InstanceId`，自己拼实例键（`TypeKey::instance::<T>(id)`）。

配置和 TS / Python 插件只认服务**名字**，而名字到键的映射目前是进程级的：

- `ServiceCatalog` 的一个名字只对应一个固定的键；
- 跨语言服务都在 `host_key(name)` 下，没有实例；
- 语言运行时进程里，服务、宿主代理、导出槽都按名字登记，两个实例的同名服务会冲突（第二个被拒绝，或者后来者静默覆盖）；
- 节点间的导出、导入、宿主也只按名字。

于是实例里的跨语言服务只能把实例 id 编进名字，丢掉了实例可见性和随实例关闭的语义。

本设计让一个名字可以声明为**实例内服务**：同一个名字，在每个实例里解析为该实例自己的键；配置和插件代码只写名字，不需要知道实例 id。

## 2. 声明

服务目录按名字声明它属于哪种实例：

```rust
let mut catalog = ServiceCatalog::new();
catalog.register::<Aimux>("aimux");                       // 全局，与现在相同
catalog.register_instance::<Tools>("tools", "session");  // session 实例内的 Rust 服务
catalog.register_shared_instance("timeline", "session"); // session 实例内的跨语言服务
```

| 方法 | 键 |
| --- | --- |
| `register_instance::<T>(name, group)` | `TypeKey::of::<T>()` 带上所在 `group` 实例的 `InstanceId` |
| `register_instance_keyed::<T>(name, group, key)` | `key` 带上该实例的 `InstanceId`（命名、动态键） |
| `register_shared_instance(name, group)` | `host_key_in(name, instance)` = `host_key(name)` 带上该实例的 `InstanceId` |
| `readable_instance::<T>(name, group)`、`readable_instance_keyed` | 同 `register_instance{,_keyed}`，表达式还可以用 `read` 读取本实例服务的值 |

- `group` 是一个 `instanced` 分组的 id。一行使用该名字时，取它所在实例链上**最近的 `group` 实例**：嵌套实例里的行可以用外层实例的服务。
- 每个名字只有一种归属：要么全局，要么属于某一种实例。**没有回退**——实例内的名字不会在实例外解析成全局服务，反之亦然。需要"默认实现、个别实例替换"时，在实例里放一行提供该名字的插件，默认实现转发给全局服务。
- `rutis-bridge` 新增 `host_key_in(name, instance)`。

## 3. 解析

名字在三处变成键，三处都改为**按副本**解析：取副本的实例链（#161 的 `Build`），按名字的归属得到键。

| 位置 | 现在 | 之后 |
| --- | --- | --- |
| `isolate`、`inject` | 组装期按行解析一次，全部副本共用 | 组装期检查归属，装载每个副本时用它的实例链解析 |
| 表达式 `has` / `read` | 固定的键 | 求值时用副本的实例链解析 |
| 语言行的门控与宿主（`RuntimeResolver`） | 解析模块时固定为 `host_key(name)` | 每个副本由 `scoped` 工厂按实例链生成 |

- **组装期检查**：一行的 `isolate` / `inject` 用了实例内名字，而它不在该 `group` 之下时，这一行无效（`Unresolved`），错误说明该名字需要放在哪个分组里。配置写错位置会在装载时直接报出。自行处理作用域的行（语言行、peer 行）同样适用。
- 表达式在实例外引用实例内名字时报错，而不是返回 `false`。
- `ExprScope::new(ctx, catalog)` 不变（全局）；新增 `ExprScope::in_instances(ctx, catalog, build)`。

## 4. 语言运行时

### 4.1 Rust 侧

`JsRow`（TS / Python 行）由 `scoped` 工厂按副本生成，带上本副本的实例链：

- 门控：依赖的跨语言服务按实例链解析，实例内名字门控在 `host_key_in(name, instance)` 上；
- 租用宿主服务：同一个名字在不同实例里是不同的宿主条目；
- 投影：行提供的实例内服务以 `host_key_in(name, instance)` 发布到 rutis，只有同一实例里的插件看得到。

### 4.2 进程里的作用域

运行时进程是共享的（每种语言一个进程，服务所有实例的行），所以进程里的登记也要区分实例。做法是复用 `isolate`：

- 每个副本对实例内名字自动带上一条 isolate，标签为该实例的 `rutis-loader/instance/<InstanceId>`；配置里已经 isolate 了这个名字时，用配置的标签（#161 起已经按副本 / 按实例区分）。
- 进程里一个服务的身份是 **(名字, 标签)**，在协议上写作 id：无标签时就是 `name`，有标签时是 `name` + NUL + `label`。名字和标签都不允许含 NUL、标签不允许为空，因此不同的 (名字, 标签) 不会得到同一个 id（全局名字 `x@L` 不会被当成标签 `L` 下的 `x`）。导出槽、句柄、宿主代理、`host:<id>` 调用目标都按 id 登记。
- 插件看到的仍是名字 `x`：Node 用 Cordis 的 isolate，同标签的行共享一个作用域；Python 运行时按行的 isolate 表查找。

协议变化（新增特性 `scopes`）：

| 消息 | 变化 |
| --- | --- |
| `hosts.provide [name, methods, label?]` | 带标签时，代理只在该标签的作用域里可见；调用目标为 `host:<id>` |
| `hosts.withdraw [id]` | 按 id |
| `service [id, handle, version]` | 导出槽按 id 通知 |
| `rows.load` | 不变；导出名的标签取自本行的 isolate |

- Rust 只在确实用到标签时要求运行时支持 `scopes`；无标签的行和现在逐字节相同。缺少该特性的运行时，实例内服务报错："需要支持 `scopes` 的运行时"。
- Node 和 Python 运行时都实现 `scopes`；Python 运行时同时补上 `isolate`（目前忽略）。
- 顺带修复三处已有问题：isolate 了某个名字的语言行看不到为它租用的宿主代理（代理登记在根作用域）；两行各自 isolate 同一名字并导出时，第二行被拒绝；Node 里用同一个标签 isolate 两个名字时，两者落在 Cordis 的同一个存储位置（Cordis 只按 isolate 符号存储），现在符号同时包含名字和标签。

## 5. 节点

一条节点连接（`rutis-bridge/peer` 行）放在 `instanced` 分组里时，每个实例有自己的连接；它的导出、导入和宿主按**该行所在的实例**映射名字：

- `ExportPlugin` / `ImportPlugin` 接受名字到键的映射（`ServiceKeys`），默认仍为 `host_key(name)`；`Features` 新增 `services`，由 loader 按该行的实例链提供；
- `HostPlugin` 同样使用这份映射；宿主为对方装载的插件，工厂也按该行的实例链生成（`register_with` 的插件不再拿到无实例的工厂）；
- 协议不变：连接本身就是按实例的，名字在一条连接里不会冲突；
- 每个实例的连接提供的 `Peer#<id>` 以该实例的标签 isolate，同一配置在多个实例里各有一条连接，互不冲突。各实例的连接通常指向不同的对端（例如每个会话一台沙箱机器，用表达式读实例服务得到地址）：对端按身份区分连接，同一对端无法同时接受同一节点的两条连接。

限制与报错：

- 放在实例外的连接导出或导入实例内名字时，连接行报错："`tools` 是 `session` 实例内的服务，请把行放在 `session` 分组里"；
- 实例里的连接不能开启 `rows` 或 `runtime`：它们发布的 `PeerRows#<id>`、`RuntimeSession#<name>` 是全局键，供实例外的行使用，每个实例都发布会冲突。实例里的 `peer:` 行和远程运行时的行照常使用实例外的连接；远程运行时与本地运行时走同一套 `scopes`，实例内服务同样可用。

## 6. 生命周期

- 实例内服务的键带实例 id，内核在实例子树关闭时撤销它们；
- 宿主租约、投影、导出槽随行的副本卸载；进程里按 id 登记的条目随之删除，同名的其他实例不受影响；
- 节点连接放在实例里时，随实例关闭断开，导入的服务随之撤销。

## 7. 兼容性

- 只注册全局名字的应用，行为和协议都不变；
- 新增的公开接口：`ServiceCatalog::register_instance{,_keyed}`、`readable_instance{,_keyed}`、`register_shared_instance`、`key_in`，`ExprScope::in_instances`，`host_key_in`，`Process::lease_host_in`，`row_projection_with`，`ExportPlugin::with_keys` / `ImportPlugin::with_keys`，`Features::services`；
- `ServiceCatalog::key(name)` 只返回全局名字的键；实例内名字用 `key_in(name, build)`。

## 8. 验收

- 两个实例各提供一个同名的 Rust 服务（实例内跨语言名字），各自的 TS 行与 Python 行只看到本实例的那个；
- 实例里的 TS 行和 Python 行提供的服务，同一实例的 Rust 插件按名字读到，其他实例读不到；
- 实例内名字用在实例外的行：组装期报错，说明需要的分组；
- 放在实例里的节点连接，按本实例导出、导入；放在实例外的连接导出实例内名字时报错；
- 关闭实例后，相关注册、代理、导出槽全部撤销，诊断与运行时进程中没有残留；另一个实例照常运行。

## 9. 实现

| 位置 | 内容 |
| --- | --- |
| `crates/rutis-loader/src/catalog.rs` | 名字的归属、`key_in`、`ExprScope::in_instances` |
| `crates/rutis-loader/src/loader/desired.rs`、`reconcile.rs`、`commit.rs` | 组装期检查；`isolate` / `inject` / 表达式按副本解析 |
| `crates/rutis-loader/src/runtime.rs` | `RuntimeResolver` 的 `scoped` 工厂；`JsRow` 按实例门控、租用、投影并带上实例标签 |
| `crates/rutis-bridge/src/session/services.rs` | `host_key_in` |
| `crates/rutis-bridge/src/runtime/process.rs`、`rows.rs` | 按 id 登记导出槽与宿主；`lease_host_in`、`row_projection_with`；特性 `scopes` |
| `node/rutis-runtime/src/runner.mjs`、`python/rutis/rutis/runner.py` | `scopes`：按 (名字, 标签) 登记；Python 实现 `isolate` |
| `crates/rutis-bridge/src/services.rs`、`compose.rs`，`crates/rutis-loader/src/peer.rs` | 导出、导入、宿主使用名字映射；节点行按实例链提供映射与工厂 |
| `crates/rutis-loader/tests/instance_services.rs`、`instance_runtimes.rs`、`instance_peers.rs` | §8 的验收：Rust 侧解析、Node 与 Python 运行时、节点连接 |
