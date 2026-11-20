# Cordis 兼容边界

[English](cordis-boundaries.en.md)

一个 Cordis 插件不改源码跑在 rutis 里时，"兼容"指[需求](../requirements-protocol-plugins.md) §4 的框架契约：服务方法的同步 / 异步形状、参数与返回值、装配与依赖门控、卸载清理、对象身份、五种事件方式的含义。由 JavaScript 语言栈、进程边界和少数未实现的能力带来的差异不装作没有，集中写在下面八条；每条按"现在是什么 / 不是什么 / 遇到怎么办"写。

怎么挂载、生成哪些 Rust 类型见 [Cordis](cordis.md)；五种方式和值怎样跨边界见[插件 API：事件](plugin-api.md#事件)与[值怎样传递](plugin-api.md#值怎样传递)；每条的保证编号、状态和用例见[保证登记表](../guarantees.md) §8。

## 1. 五种事件方式，没有 `collect`

**现在是什么**

- 方式只有五种：`emit`、`parallel`、`serial`、`waterfall`、`bail`。Cordis 插件用 Cordis 自己的同名方法；跨进程时仍是同一张监听器表上的同一种方式，别处的监听器和本进程的按登记顺序在一条链上。
- `emit` 是通知：返回 `undefined`。Cordis 插件的 `emit` 返回前，本进程的监听器都已被调用（它同步等宿主分发完；排在本进程前一个还没开始的 `emit` 之后时除外，调用随后按顺序到达）；别处的监听器不等。监听器出错只报告，不影响发出方，也不影响其他监听器。
- `parallel` 等全部完成，成功时也返回 `undefined`；有失败时抛 `AggregateError`，子错误按登记顺序，只有一个失败也如此。
- `serial` 是异步的、`bail` 是同步的，两者是分开的方法；都返回第一个"有结果"的值：`null` / `undefined` / `false` 不算结果，其余（包括 `0`、`""`）算。`bail` 遇到异步监听器的问题见第 2 条。

**不是什么**

- `emit` / `parallel` 不收集监听器的返回值。每个监听器都返回值也一样：期望文件里 `parallel/all-ok` 的三个监听器返回 `"A"`、`null`、`"C"`，期望的 `parallel` 结果是"没有结果"。
- 没有 `collect`，也没有任何"把所有监听器的结果收成数组"的方法。Cordis 4.0.4 的分发入口只有 `dispatch` 和这五种方式；rutis 没有加第六种。
- `serial` / `bail` 也不会跑完全部监听器再挑结果：第一个有结果的就停，其余不再调用。

**遇到怎么办**

- 要每个监听器的结果：让监听器把结果写进一个共享结构（`Map`、数组），用 `parallel` 当屏障，等它返回后再读。
- 只要一个结果：用 `serial`（可以 `await`）；同步上下文里用 `bail`，并保证链上没有异步监听器（第 2 条）。
- 只要通知、不需要等：用 `emit`；要拿到失败：用 `parallel` 并处理 `AggregateError`。

依据：保证 `events/emit`、`events/parallel`、`events/serial-bail`。`node/rutis-runtime/test/cordis-events.test.mjs` 的 `the five Cordis methods dispatch through the host, each in its mode` 与 `leaf plugins: …`（断言 `await leaf.parallel('o')` 是 `undefined`）；`crates/rutis-bridge/src/runtime/testing/events.expected.json` 的 `emit/reaches-all`、`parallel/all-ok`。

## 2. `bail` 遇到异步监听器：`AsyncListener`

**现在是什么**

- 同步的 `bail` 逐个同步调用监听器。本进程的 `async` 函数，或普通函数但返回 Promise 的监听器，让这次分发以 `AsyncListener` 失败；返回的 Promise 被丢弃。
- 已知是异步的别处监听器（另一个运行时里标成 `async` 的条目）同样让它失败。错误文字是 `listener of <事件> in <监听器> is asynchronous and cannot run in a synchronous bail`，`<监听器>` 写明是哪一行、哪个运行时、哪个节点或哪个插件。

**不是什么**

- 不是旧 Cordis 的行为（把返回的 Promise 本身当成短路值）。
- 不会退化成等待：不会 `await` 那个监听器，也不会跳过它继续找后面的结果。

**遇到怎么办**

- 需要等监听器：用 `serial`（异步）。
- 需要同步短路：把监听器写成同步函数（不返回 Promise），并且链上别处也不能有异步监听器。
- 从 Cordis 迁过来时，凡是 `ctx.bail` 的监听器可能为 `async`，改成 `serial`，或把监听器改成同步。

依据：保证 `events/serial-bail`、`events/hub-bail`；`crates/rutis/tests/events/events.rs::bail_meets_an_async_listener`；`crates/rutis-bridge/tests/events/event_hub.rs::bail_is_synchronous_everywhere`；`node/rutis-runtime/test/cordis-events.test.mjs` 的 `the five Cordis methods dispatch through the host, each in its mode`（`root.bail('b')` 抛 `AsyncListener`）。

## 3. `intercept`：非空的按服务合并配置不支持

**现在是什么**

- 行的 `inject` 可以写成数组，或写成对象。对象里每个名字只接受裸声明：`null`、`true` 或空对象 `{}`，等同于把这个名字放进 `inject`。
- 值非空（例如 `{ "dep": { "level": 1 } }`）时，loader 返回 `Unsupported`，这一行是 `unresolved`、不会运行。`rutis-host status` 里这一行的状态是 `unresolved: unsupported: intercept config for "dep" in inject of "i"`。

**不是什么**

- 不是"忽略 intercept 继续跑"：这一行不启动。
- 不是把 Cordis 的按服务合并配置搬到了 rutis。rutis 的 `ServiceIntercept` 用于拦截服务读写，和 Cordis 配置层的 `intercept` 不是一回事。
- 不是行配置里的一个 `intercept` 字段：rutis 的行没有这个字段，写了按未知字段忽略；它看的是 `inject` 对象里每个名字的值。
- 不是只对某些运行时有效：不认这个配置的是 loader，所有语言的运行时行都一样。

**遇到怎么办**

- 插件需要这部分配置：把它搬进插件自己的 `config`，在 `apply` 里合并；或者去掉这一行。
- 只是裸声明：把 `{ "dep": {} }` 简写成 `["dep"]`，或保留 `{ "dep": true }`。
- 先跑 `rutis-host check`，或看 `rutis-host status`，确认是哪一行、哪个服务。

依据：`crates/rutis-loader/src/loader/desired.rs` 的 `parse_scope`（`null` / `true` / `{}` 被接受，其余返回 `Unsupported`）；测试 `crates/rutis-loader/tests/rows/scope.rs::unknown_services_and_intercept_config_are_unresolved`；保证 `loader/isolate-and-inject`。

## 4. 直接 `ctx.set` 换值：不通知，下一次调用后刷新

**现在是什么**

- `ctx.provide` 发布、属性赋值（`ctx.counter = x`）换值会通知：前者触发 `internal/service`，后者走 `internal/set`。兼容层据此重新读取槽位，把新对象以新句柄、新版本发布给 rutis 侧。
- 直接 `ctx.set('counter', x)` 不发任何通知。Node 运行时在每次从 rutis 调到这里导出的服务方法之后重新读取所有槽位，方法抛错时也刷新；异步方法在它的 Promise 落定后刷新。对象身份变了就换新句柄、新版本，并通知 rutis 侧。

**不是什么**

- 不是立即对 rutis 可见：换值发生在一次调用里时，要等这次调用返回（同步）或落定（异步）之后才刷新；异步路径下允许短暂读到旧代理。
- 已经拿到的代理不会跟着换：它一直指向当初那个对象，直到调用方放掉它。"下一次调用命中新对象"只对重新读取服务（`ctx.get`、依赖门控重启后的 `ctx.use`）成立；手里旧句柄的下一次调用仍打到旧对象。
- 不是对同进程 Cordis 插件也这样：它们读 Cordis 的 store，直接拿到新对象。这条边界只对 rutis 一侧（宿主、其他进程、其他语言）成立。

**遇到怎么办**

- 要让别的进程、宿主或其他语言看见换值：用属性赋值换值，或撤回后重新 `provide`，不要用 `ctx.set`。
- 用了 `ctx.set`：把它放在一次方法调用里，调用返回后让使用方重新读服务（`ctx.get`、重新 `ctx.use`），不要复用换值前拿到的代理。
- 不要在定时器、事件回调里用 `ctx.set` 静默换值，然后指望下一次调用立刻命中新对象。

依据：保证 `boundary/5.1`；`crates/rutis-bridge/tests/session/service_projection.rs::projected_service_follows_replacement_and_withdrawal`、`crates/rutis-bridge/tests/session/projection_lifecycle.rs::a_throwing_method_still_refreshes_the_slot`；实现见 `node/rutis-runtime/src/runner.mjs` 的 `refresh` 与 `dispatchCall`。

## 5. 宿主提供的服务：代理，`instanceof` 不成立

**现在是什么**

- rutis 提供给 Cordis 插件的服务，在 Cordis 侧是按声明的方法生成的代理对象：按接口调用和原生一致（参数、返回值、异步 / 同步），未知方法报 `no-method` 并列出有的方法。
- 代理只有声明过的方法；属性、getter、字段不在其中。Node 侧访问一个未知属性会得到一个函数，调用它才报 `no-method`，所以 `typeof ctx.log.anything === 'function'` 不能当作"有这个成员"。

**不是什么**

- 不是插件声明的那个类的实例：`ctx.log instanceof Log` 是 `false`（同一份插件在原生 Cordis 里是 `true`）。
- 不是宿主那边的对象本身，也不带它的原型链、构造器和私有字段。
- 不是宿主对象的快照：每次调用都跨到宿主，读到的是宿主那一刻的状态。

**遇到怎么办**

- 按接口写：调用声明过的方法，不判断 `instanceof`、`constructor`、原型。
- 需要类的实例：在插件里用代理包一层自己的类。
- 需要判断服务在不在：用 `ctx.get('log')` 或 `inject`（依赖门控），不要用 `instanceof` 或 `typeof` 猜。
- 给插件写 `.d.ts` 时声明成 `interface`，不要声明成 `class` 再期待 `instanceof`。

依据：保证 `boundary/5.7`；`crates/rutis-bridge/tests/session/boundary_rules.rs::a_host_service_is_a_proxy_not_an_instance_of_the_declared_class`（同一份插件在原生 Cordis 里 `instanceof` 为 `true`，经兼容层为 `false`，调用结果相同）；[需求 §5](../requirements-protocol-plugins.md) 规则 7。

## 6. 同步等待：`SyncWaitCycle`

**现在是什么**

- 同步方法（Cordis 里不是 `async` 的方法）执行中如果等待需要事件循环推进的结果（定时器、Promise），这次调用以 `SyncWaitCycle` 失败，不会卡死。Node、Bun、Python 由运行时自己发现；宿主里等待的 future 也一样。
- 宿主转发同步调用前检查跨运行时的等待关系：这次转发会让运行时互等（例如两个运行时互相同步调用）时不转发，向调用方返回 `SyncWaitCycle`，只让这一次调用失败。
- 同步 `bail` 等到别处的监听器也是一次同步调用，按同一套规则参加等待环检测。

**不是什么**

- 不是超时：没有"等多久就放弃"的时间，真正的等待环直接报错。
- 不是全部同步等待的保险：运行时没有在调用帧上标明的同步调用不在检测范围内，可能死锁（明确不保证）；说了发出即不管（`awaited: false`）却实际等结果的调用也漏边。本仓库的运行时都标明同步调用。
- 不是加超时可以绕过的：同步等待本身就不该等需要事件循环的结果。

**遇到怎么办**

- 要等 Promise、定时器、别处的服务：把方法改成 `async`。接口形状变了，但这是唯一正确的做法。
- 确实是环：把一侧改成异步，或让发出即不管的调用标 `awaited: false`（会话协议 SESS-83），不要在同步链上互相等。
- 看到 `SyncWaitCycle`：看错误里的调用链；宿主返回的那条还会列出经过的行。

依据：保证 `boundary/5.6`、`bridge/sync-wait-cycle-refused`；`crates/rutis-bridge/tests/session/rpc_callbacks.rs::node_sync_wait_pumps_callbacks_and_reports_its_executor_cycle`（夹具 `node/rutis-runtime/test/fixtures/rpc-client.mjs` 断言 `SyncWaitCycle`）、`::sync_wait_detects_an_async_callback_already_running_on_its_runtime`；`crates/rutis-loader/tests/lang/sync_cycles.rs`；设计[多语言运行时](../design-multilang-runtimes-2026-10-03.md) §九、[函数引用](../design-function-refs-2026-11-03.md)。

## 7. 卸载顺序与运行时覆写

**现在是什么**

- `sdk/unload-order` 的保证是：行卸载时跑一个栈，逆序、一条一条；`provide` 的撤回、`effect` 和 `apply` 返回的清理都在里面。它覆盖按插件 API 登记的这些清理：Node 的叶子插件（`definePlugin`）和另外五种运行时。
- 原生 Cordis fiber 的 `_unload` 是 Cordis 自己的：对全部处置用 `Promise.all` 一起开始。Cordis 4.0.4 就是这样，兼容层不改它。Node 运行时只为叶子插件维护自己的栈，用一个 Cordis effect 逆序排干，从而给出与别处相同的顺序。
- Node 运行时确实覆写了 Cordis 的 `events.dispatch` 和五个方法（`emit`、`parallel`、`serial`、`waterfall`、`bail`），把它们接到逻辑总线上；`internal/*` 仍走 Cordis 自己的表，监听器登记走 `internal/listener`。

**不是什么**

- 不是"Cordis 插件的 `ctx.effect` 清理也逆序、一条一条"：它们一起开始，完成顺序不作承诺，`sdk/unload-order` 不覆盖它们。
- 不是"运行时装了 `_unload` 的补丁"：`_unload` 没被改；覆写的是事件对象上的 `dispatch` 和五个方法，另外给每个 Context 加了只读的 `ctx.rutis` 和 `ctx.signal`，监听器登记接在 `internal/listener` 上。
- 不是把所有 Cordis 行为都接管：对总线上的名字，`events.dispatch` 仍返回回调（一个），`internal/*` 照 Cordis 原样；五个方法保留 Cordis 的形状（Cordis 插件的 `emit` 与 `waterfall` 同步等宿主分发）。

**遇到怎么办**

- Cordis 插件里不要依赖两个 `ctx.effect` 清理的先后：把有顺序的步骤写进同一个清理，按需要的顺序执行；不要拆成两个 `ctx.effect` 再指望先后。
- 需要"停了就不再工作"：用这一行的 `ctx.signal`，不要靠清理顺序。
- 用 `ctx.on` / `ctx.emit` 等 Context 方法，不要改 `ctx.events` 或它的内部字段；插件自己调用 `events.dispatch` 时，拿到的回调会经总线分发。

依据：保证 `sdk/unload-order`；设计[卸载顺序](../design-unload-order-2026-11-01.md)（§二 D7、§三"卸载栈"）；`node/rutis-runtime/src/leaf.mjs` 的 `toCordis`（自己的栈）、`node/rutis-runtime/src/cordis-events.mjs` 的 `install`（覆写 `events.dispatch` 和五个方法）；Cordis 4.0.4 的 `lib/index.js`（`_unload` 用 `Promise.all`）；测试 `node/rutis-runtime/test/cordis-events.test.mjs` 的 `events.dispatch gives one callback that dispatches through the host`。

## 8. 跨进程边界：对象身份、取消、超时、事件作用域

| | 现在是什么 | 不是什么 | 遇到怎么办 |
| --- | --- | --- | --- |
| 对象身份 | 带方法的对象按引用传：同一个对象交出几次是同一个代理，交回它自己的进程还是原对象。代理只有方法，没有字段；放掉的引用不能再调用；提供方的进程退出后，调用以 `peer-gone` 失败 | 不是拷贝或快照，也不是两边的同一个 JS 对象：`instanceof`、构造器、私有字段都不成立；纯数据的对象按值传，过一趟就不再有身份 | 需要身份就把对象本身交过去，不要交它的字段副本；对象不能作为 `#[rutis::service]` 方法的参数（以 `bad-argument` 失败）；持有代理并复用；对 `peer-gone` 有处理 |
| 取消 | Rust 丢弃调用的 future 即取消；Python 协程、Go 的 context、Swift 的 Task、Kotlin 的协程收到取消；JavaScript 方法只在调用方传了 `AbortSignal` 时收到 abort（Rust 绑定调用 Cordis 方法时会传）。事件分发上，除 `emit` 外，发出方放弃会取消正在运行的监听器；JavaScript 不能取消一次分发，也没有逐次调用的信号 | 不保证对面已经停下：方法可能已经跑完，结果被丢弃；协议不把取消报告成"没有执行"。`emit` 不会被取消 | 长方法写成 `async` 并接收 `AbortSignal`，在检查点看 `signal.aborted`；副作用做成幂等；取消或超时之后把结果当作未知，不当作失败或成功 |
| 超时 | rutis 不给调用加时限，由调用方用本语言的方式设：Rust `tokio::time::timeout`、Python `asyncio.wait_for`、Go 的 `context.WithTimeout`、Swift 的 `Task` 取消、Kotlin `withTimeout`。超时即取消。宿主停止另有截止时间：`rutis-host` 的 `--shutdown-timeout`，默认 10 秒；等待正在运行的监听器也用它 | 不是兼容层会替你限时；也不是到点对面就停：结果未知，副作用可能已经发生。卸载截止时间不是调用超时，它限制宿主停止时等监听器和清理多久 | 每个跨进程调用都自己设超时；用异步方法，不让同步方法等事件循环（第 6 条）；看进行中的调用用 `rutis-host status`；宿主停止的等待时间按应用需要设 `--shutdown-timeout` |
| 事件作用域 | 事件在一条平总线上按名字送达所有监听器；作用域（`ctx.rutis.scope`、监听器描述里的 `scope`）是给过滤用的描述，由加载器标出所在的组实例；`isolate` 只隔离服务，不隔离事件；`internal/*` 留在本进程，`rutis/*` 只有宿主能发 | 不是路由，也不是隔离或权限边界：别的实例、别的租户的监听器照样收到同名事件，除非它自己用过滤或不同的事件名排除 | 要限制范围：用不同的事件名，或用过滤（`ctx.filter`、`thisArg[Context.filter]`）按 `c.rutis.scope` / 行筛选；不要把作用域当安全边界；`rutis/*` 只能监听 |

依据：保证 `bridge/object-refs`、`events/cancel`、`events/flat`、`events/hub-flat`、`boundary/5.8`；`crates/rutis-bridge/src/session/rpc/tests/objects.rs`（`an_object_given_twice_is_one_relay_and_comes_back_as_itself`、`a_call_on_an_object_given_up_is_cancelled_at_its_owner`）；`node/rutis-runtime/test/session.test.mjs` 的 `an object from the far end is a proxy whose methods call it (#687)`；`crates/rutis-bridge/src/runtime/testing/events.expected.json` 的 `scope/flat-bus`、`cancel/emitter-gives-up`（`node`、`node-leaf`、`bun` 不在其中：JavaScript 取消不了分发）；[插件 API：取消](plugin-api.md#取消)。
