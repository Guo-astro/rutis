> 摘自 rutis 仓库 `docs/design-philosophy.md`（main，2026-10-14），随 skill 一起分发以便独立使用；以原文为准：https://github.com/arcships/rutis/blob/main/docs/design-philosophy.md

## 七、原则与判断标准

这一节由前面的主张推出，主要写给参与开发的人。

### 原则

1. **范式只实现一份，在 Rust 内核里**；其他语言的插件是叶子，只有很小的 SDK（[多语言设计](https://github.com/arcships/rutis/blob/main/docs/design-multilang-runtimes-2026-10-03.md)）。
2. **兼容工作在外层完成，不改内核**（[挂载需求](https://github.com/arcships/rutis/blob/main/docs/requirements-protocol-plugins.md) §1）。
3. **保证要写出来，并用测试钉住**；不提供的保证也写明（[内核设计](https://github.com/arcships/rutis/blob/main/docs/design-rust-port.md) D31）。
4. **扩展点按具体需求开，开得窄**，不提供万能钩子（[观察与拦截设计](https://github.com/arcships/rutis/blob/main/docs/design-cordis-observation.md)）。
5. **接上的东西都是插件**：运行时、节点、挂载受同一套生命周期管理，不享有特权。
6. **契约只约定跨边界真正需要的东西**：服务名和方法形状（同步还是异步），不建通用的类型层。值跨语言时怎样转换由 rutis 统一；同一服务的不同实现返回的数据是否一致、能否互相替换，由服务的作者负责。
7. **跨边界做不到的事写成边界规则**，不静默降级。
8. **依赖声明里只有服务名**，不出现语言和位置。
9. **配置描述期望状态**，由 loader 持续对账，而不是由宿主手写启停顺序。
10. **显式优于隐式**：隐式机制跨不过进程边界。

### 判断一项能力该不该做

1. 它扩大了宿主能连接或适应的范围，或者让宿主更能认识和迭代自己的连接吗？接一种运行时，看它带来多少生态能力，而不是为了多支持一种语言。
2. 它能放在内核之外吗？
3. 它跨过进程边界后还成立吗？
4. 它有具体的使用方吗？
5. 它的保证能写成测试吗？

