# 一个 loader 管理多个实例

[English](design-loader-instance-mounts-and-services-2026-10-07.en.md)

设计提案 · [#158](https://github.com/arcships/rutis/issues/158)、[#159](https://github.com/arcships/rutis/issues/159) · 基线 `330ff51`

## 1. 配置与装载

```yaml
- id: assistant
  mount: document
  name: assistant-plugin
  inject: [document]
```

宿主注册两个 document 实例，loader 分别创建 `assistant@d1`、`assistant@d2`。编辑 assistant，两个插件一起更新；关闭 d1，卸载 `assistant@d1`。

| 接口 | 结果 |
| --- | --- |
| `register_mount("document", ctx, wait)` | 在 ctx 下装载匹配模板；可选择等待并返回逐行结果 |
| `handle.unregister()` | 卸载该挂载的受管插件 |

`mount` 放在顶层行或分组，后代继承。宿主实例根的 ID 记为 owner；运行记录使用 `(行 ID, owner)`。配置文件保存模板，运行状态按实例查询。

## 2. 名称与构造

配置保存模块名，loader 为每个挂载生成运行名称：

```text
assistant-plugin + d1 → resolve(实例运行名称1) → 工厂1 → 子插件1
assistant-plugin + d2 → resolve(实例运行名称2) → 工厂2 → 子插件2
```

Resolver 从运行名称取得原模块名和实例信息，查找模块，再构造持有 owner 的工厂。每个工厂返回自己的依赖列表。运行名称同时作为解析缓存键。

## 3. 服务

沿用现有服务登记方式，增加全局或实例作用域选项，默认全局。实例作用域由 loader 按 owner 生成服务键；Rust 类型信息和跨语言方法描述仍按现有方式登记。作用域与能否跨语言调用互相独立。

```text
document@d1 → host_key("document").with_instance(d1)
document@d2 → host_key("document").with_instance(d2)
```

插件仍按 `document` 读取服务。loader 在创建插件前确定所属实例，并把名字转换成对应服务键。

- 实例行优先使用实例声明；其余名字使用全局声明。选中的服务尚未出现时，必需依赖进入等待。
- inject、isolate、表达式和跨语言代理共用解析结果。
- 隔离标签按实例区分；同实例的相同共享标签指向同一服务位置。
- Node 用 Cordis 的 `isolate(name, symbol)` 设置名字位置；Python 通过当前行查询名字映射。

## 4. 更新与关闭

| 操作或结果 | 行为 |
| --- | --- |
| 编辑配置行 | 更新该行的全部实例 |
| 禁用、删除或卸载配置行 | 清理该行的全部实例；分组包含各实例中的后代 |
| 换模块 | 清理该行全部旧实例，再按新模块装载 |
| reload | 检查使用该模块的全部行和实例 |
| 预检失败 | 拒绝编辑，当前版本继续运行 |
| apply 失败 | 按旧配置重新装载，报告执行及恢复结果 |
| 新实例装载失败 | 向宿主报告该实例的逐行错误 |
| 插件自行停止 | 临时停用当前实例的该行；分组同时停用后代 |
| 恢复临时停用 | 重启该实例行，或重新注册挂载 |
| 宿主关闭一个实例、撤销一个挂载 | 清理该实例下的受管插件 |
| 卸载 loader | 清理所有配置行的全部实例 |

编辑流程：**确定受影响实例 → 全部预检 → 更新插件 → 持久化。**

注册、撤销、编辑串行执行。宿主关闭信号立即使挂载失效。重启后的宿主重新注册，旧注册标识随之失效。

## 5. 改动范围

| 改动 | 落点 |
| --- | --- |
| 模板展开 | loader 登记挂载，为每个实例生成运行名称和运行记录 |
| 名称映射 | Resolver 按运行名称构造插件；catalog 生成实例键；bridge 的字符串索引使用内部服务名 |
| 批量管理 | 按配置行遍历全部实例完成预检、更新和卸载；按宿主实例清理对应子树；查询汇总实例结果 |

跨语言控制只给 `rows.load` 增加 `names` 映射：

```text
names = { 插件中的服务名: 内部服务名 }
内部服务名 = encode(服务名, owner, 有效隔离位置)
```

host 注册、引用计数、导出查重、服务通知和撤销统一使用内部服务名。现有 `name` 字段及字符串表直接承载该值。对象调用仍使用具体对象的 handle。

## 附录 A：详细规则

### 挂载与诊断

- `mount` 为非空字符串。注册 ctx 与 loader 属于同一 root，且处于有效运行代；一个 owner 对应一个类别，重复注册报错。
- 注册的等待选项返回逐行结果；等待启动使用现有 FiberView 状态观察。超时列出仍在等待的行，结果绑定本次注册和配置。现有 loader settle 只等待状态转换结束，不能直接当成启动成功。
- 宿主提供服务并完成 apply 后，再等待消费者启动。显式 unregister 承担异步卸载；句柄 drop 仅释放句柄。
- entries/get 按模板汇总实例状态；locate/row 从 fiber 反查行和 owner；尚无实例的模板显示“未展开”。
- disabled 在挂载根求值，config 在父分组及本行隔离后的上下文求值。实例相关预检在注册时执行。
- 分组编辑检查后代；volatile 在全部预检通过后逐实例发送。恢复旧配置属于重新装载；外部操作由插件处理。

### 服务与工厂

- 实际服务身份为 `(TypeKey, 有效 ScopeId)`；有效 ScopeId 包含祖先隔离。私有标签包含行和 owner，共享标签包含 owner。
- 全局行使用全局声明；仅有实例声明而缺少 owner 时报告上下文错误。未登记的 Cordis 原生名字交给 Cordis，share_by_name 按既有规则处理。
- 实例服务名集合在装配前确定。Node 每行获得全部实例名映射，覆盖动态 provide/get/inject 及内部子插件访问；未知动态名字遵循 Cordis 原生规则。
- 同进程原生消费核对连接与服务位置后直接取得对象。内部子插件的独立隔离由 Cordis 管理；导出读取声明所在行的作用域。
- 同类型子插件分别构造，各自持有依赖列表。依赖可按挂载实例动态生成，在各自 fiber 创建时固定；改变已创建 fiber 的依赖列表时重建 fiber。
- 运行名称使用带版本的专用编码，字段为模块名、类别、owner、挂载注册标识；示例中的 `@d1` 仅供显示。统一编码和解码处理模块名中的特殊字符；错误编码返回解析错误。
- Builtins 解码后按原模块名查表。实例注册入口接收 `MountInfo → PluginFactory` 构造函数；普通注册项复用原工厂。RuntimeResolver 解码后用原模块名定位包，生成持有 owner 的 JsFactory。第三方 Resolver 要支持实例构造时使用相同解码函数。
- 工厂捕获实例信息，injects 返回本实例依赖，build(config) 将实例信息交给插件构造函数。Chain 继续按 NotFound 选择 Resolver。
- loader 和 RuntimeResolver 的 Resolved、offline 记录按完整运行名称索引；模块定位及版本检查使用原模块名。保留运行时现有缓存策略。reconcile 复用对应 Arc，挂载撤销移除该注册生成的运行名称。公共 schema_of 使用配置模块名。
- spawn、update、volatile、dry-run 及状态/schema/meta 查询使用各目标缓存的 Resolved；预检的新结果在提交时复用。依赖列表变化走现有重建判断，配置变化走 update。直接挂载检查 ctx 的实例祖先归属。
- reload 先使该模块全部运行名称的缓存失效，取得新结果并预检，再整批替换；失败恢复整批旧缓存。禁用目标同步失效。RuntimeRowsPlugin 按实际运行名称处理离线刷新；rename 清理对应运行名称。远端模块声明遵循现有重新查询规则。

### 通信与清理

- host、导出槽、观察者、Projection 初始化统一使用内部服务名；同名重复导出沿用现有查重。多个实例使用同一全局服务时共用 lease。
- Node 的槽保存插件本地名，exporter 从对应 scope 读取本地名，通知发送内部服务名。host 注册也在该 scope 提供本地名。Python 通过行映射查询服务，update 携带原映射。
- handle 使用会话内递增标识区分具体对象；服务通知沿用现有 version 去旧规则。能力检查在现有 features 中增加映射支持标识。
- 卸载配置行时先锁定该行全部运行记录，再逐实例撤销投影、卸载插件、释放 lease，最后汇总清理结果。共享服务随引用计数释放。
- 主动卸载顺序：撤销 Rust 投影、等待消费者停止 → 卸载语言行 → 释放 host lease。
- Cordis 自行 dispose 与 Rust 消费者停止并行发生；Rust 收到终止通知后撤销投影。
- #158 的局部停用先修改现有 Rust FiberView 观察路径。Cordis 原生自行停止到 loader 的同步属于独立生命周期问题，另行处理。

### 范围

本期配置覆盖以模板为单位。远程 Runtime 使用扩展控制协议；跨节点 Export/Import 和广播事件使用全局语义。旧端及 peer 行收到实例绑定请求时返回能力错误。

## 附录 B：消融结果

| 删除的改动 | 直接后果 | 结论 |
| --- | --- | --- |
| 挂载注册及运行记录增加 owner | 同一行的两个实例占同一条运行记录 | 必须改 |
| 逐实例传入 owner 并生成依赖 | 实例插件仍使用全局服务键 | 必须改构造数据；同类型插件可具有不同依赖 |
| 新解析入口 | 实例信息可由运行名称传入 resolve(name) | 删除 resolve_in |
| 独立 bind 阶段及通用绑定器 | Resolver 从名称构造实例工厂 | 删除 |
| 复合解析缓存键 | 运行名称已包含挂载注册身份 | 继续使用字符串键 |
| 全实例预检 | 第一实例检查通过后，其他实例的配置错误才暴露 | 必须改 |
| 外部受管行异步清理 | loader 卸载后，宿主子树中的插件继续运行 | 必须改 |
| host、导出槽、语言查找增加实例身份 | 同名服务覆盖、导出冲突或读取错误实例 | 必须改索引，复用现有算法 |
| 独立 MountHandle.settled | 注册提供等待选项即可满足装配需求 | 删除独立接口 |
| 一组内核公开检查接口 | 同 root 可比较 root_view().instance()；取消使用 cancellation_token() | 删除整组预设；仅补实际缺口 |
| 新对象管理和白名单生命周期改造 | 属于已有对象生命周期问题 | 移出；handle 只补实例及装载区分 |
| Cordis 原生终止通知协议 | 宿主关闭仍走现有按行清理 | 移出，另列原生生命周期同步 |
| 新协商、断线及通知管理机制 | 现有 features、观察者与清理可复用 | 仅增加 feature 和新索引 |

需要有效隔离位置是功能要求；公开 ScopeId 是接口选择。`host_key_in` 是构造便捷函数。两者均不单列为新系统。

## 附录 C：源码与验收

| 依据 | 源码 |
| --- | --- |
| 工厂依赖与构造 | [plugin.rs:30](../crates/rutis/src/plugin.rs#L30)、[plugins.rs:12](../crates/rutis-loader/src/loader/plugins.rs#L12)、[resolver.rs:15](../crates/rutis-loader/src/resolver.rs#L15) |
| 行管理、预检、回滚 | [reconcile.rs:64](../crates/rutis-loader/src/loader/reconcile.rs#L64)、[commit.rs:24](../crates/rutis-loader/src/loader/commit.rs#L24) |
| 实例归属、隔离、取消 | [ctx.rs:163](../crates/rutis/src/ctx.rs#L163)、[491](../crates/rutis/src/ctx.rs#L491)、[1252](../crates/rutis/src/ctx.rs#L1252) |
| 服务绑定与运行时 | [catalog.rs:52](../crates/rutis-loader/src/catalog.rs#L52)、[runtime.rs:274](../crates/rutis-loader/src/runtime.rs#L274)、[process.rs:570](../crates/rutis-bridge/src/runtime/process.rs#L570) |
| 语言行与原生隔离 | [runner.mjs:205](../node/rutis-runtime/src/runner.mjs#L205)、[runner.py:90](../python/rutis/rutis/runner.py#L90)、`@deepseek-ai/cordis@4.0.4/src/context.ts:121`（本地依赖源码） |
| 原生终止 | `@deepseek-ai/cordis@4.0.4/src/events.ts:329`、`src/fiber.ts:265`（本地依赖源码） |

验收覆盖：多类挂载及恢复、Rust/TS/Python 同名实例服务、全局共享与祖先隔离、Cordis 原生插件行为、全实例预检、恢复失败、自行停止、级联关闭、断线与迟到通知。

状态：源码研究及独立复核完成，功能待实现。Cordis 依据为本地 4.0.4；部署兼容性按插件实际依赖核对。
