# rutis 实例与子树能力：三个 GitHub issues

状态：已发布，实现在 `feat/instance-subtrees` 分支进行。

统一设计见 [实例键、实例事件与子树永久关闭](../../design-instance-subtrees.md)。需求来自一个下游应用，但这里仅跟踪 rutis 框架能力。

| 顺序 | Issue | 对应需求 | 依赖 |
| --- | --- | --- | --- |
| 1 | [#22 实例服务键与子树可见性](https://github.com/arcships/rutis/issues/22) | R1、最小诊断与共同准入设施 | 无其他业务项目依赖 |
| 2 | [#23 实例事件与在途派发归属](https://github.com/arcships/rutis/issues/23) | R2 | #22 |
| 3 | [#24 子树永久关闭与完整回收](https://github.com/arcships/rutis/issues/24) | R4 + R3 | #22、#23 |

不建 runtime 迁移或业务压测 issue。1000 轮创建/关闭属于第三项的框架回归测试。

## 已在草案中定下来的选择

- TypeKey 保留 Clone 和动态限定名，getter 使用 instance_id；with_instance 可为已有键附加实例身份。
- 创建 fiber 时分配进程内唯一 ID，重载不变，重建不复用；旧 Ctx 可以读取旧 ID，但不能重新注册。
- 实例事件的发送与注册都受子树可见性和关闭准入约束；关闭等待已经接纳的相关派发结束。
- 子树 shutdown 只有在 driver、派发和内部回收完成后才返回；内部完成信号和公开完成结果分开，避免父子互等。
- 关闭期间仍接管在途 apply 已取得资源的清理。
- 错误经拥有关系收集，独立关闭并脱离的失败子树不无限保留在父级。
- 不宣称服务漏写实例 ID 会编译失败；业务作用域类型策略归调用方。

现有 [#13 诊断](https://github.com/arcships/rutis/issues/13)、[#9 root shutdown](https://github.com/arcships/rutis/issues/9)、[#11 历史错误消费](https://github.com/arcships/rutis/issues/11)、[#12 卸载等待截止](https://github.com/arcships/rutis/issues/12) 只作关联；不在本草案中更改它们的远端状态。
