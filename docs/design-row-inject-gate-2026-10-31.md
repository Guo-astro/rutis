# 运行时行的行 `inject`：rutis 门控并租借

[English](design-row-inject-gate-2026-10-31.en.md)

状态：已实现（分支 `claude/row-inject-gated`，未合并）。日期：2026-10-31。基准：[声明集合](design-declared-services-2026-10-29.md) §七 记的后续，叠在那份设计所在的提交之上。§一 描述的代码是本稿所在提交的父提交里的。
Issue：尚无，随本稿开。相关：[声明集合](design-declared-services-2026-10-29.md)、[可选依赖](design-optional-deps-2026-10-28.md)。

## 一、现状

Rust 行的行 `inject` 经 `combined_injects`（`crates/rutis-loader/src/loader/reconcile.rs`）并进 fiber 的 injects，fiber 等齐才启动。运行时的行（Node、Bun、Python、Go、Swift、Kotlin 的行）走另一条路：

```
Inner 启动一行（reconcile.rs）
├─ ① Row::foreign_scope（desired.rs）      → BoundScope.inject 为空
├─ ② combined_injects(factory, scope)      → 只有插件声明的依赖（JsFactory 的 gated）
└─ ③ JsRow::start（runtime.rs）            → 只租借 gated；行 inject 只经 rows.load 的第五个参数交给运行时
```

结果因运行时而异：

| 运行时 | 行 `inject` 里的服务由宿主或别的进程提供，进程里还没有 |
| --- | --- |
| Node | 行在 rutis 里显示运行，实际停在进程内的 Cordis 门控上，`apply` 不运行，一直等 |
| Bun、Python、Go、Swift、Kotlin | 不等，`apply` 照常运行，`use` 报 `service <name> is not available` |

而 `Loader::check`（`rutis-host check`）早就按另一条规则报：运行时的行在 rutis 里等它行 `inject` 中服务目录共享的名字（`crates/rutis-loader/src/loader/check.rs` 的 `Loader::check` 里行 `inject` 那一段，`0606ace0` 引入）。`check` 说的与实际运行不一致。

## 二、决策

| # | 决策 | 为什么；放弃的做法 |
| --- | --- | --- |
| D1 | 运行时的行在 rutis 里按它行 `inject` 中服务目录共享的名字门控：`Row::foreign_scope` 给出这些名字的 key，`combined_injects` 把它们并进 fiber 的 injects | 与 Rust 行一致（`combined_injects`），也与 `check` 已经报的规则一致。key 与这一行租借它时用的相同（`ServiceCatalog::shared_key_in`），实例里是实例自己的那份。放弃"只在 SDK 里补"：rutis 没租借进来的服务，SDK 进程里拿不到 |
| D2 | 启动时把这些名字像插件的 `inject` 一样租借进运行时：同一进程里的行提供的，用原生的；这一行自己 isolate 了的名字，租借在那个隔离里（分组上的 isolate 见 §六） | 门控只保证服务在 rutis 里；行要用它，得租借进进程。与插件的 `inject` 用同一段租借代码 |
| D3 | 服务目录不共享的名字（应用登记成 Rust 类型的）不门控、不租借，照旧只经 `rows.load` 交给运行时，由运行时原生解析（Node 行的 Cordis 门控在进程里等这类名字，同一进程里没有行提供时一直等，本稿之前就如此） | 与 `check` 一致；这类名字不是 `dyn HostDispatch`，跨不了语言。注：leaf 运行时的插件 `inject` 按 `split_dependencies` 的 `leaf \|\| is_shared` 对这类名字也门控（等 `host_key(name)`），行 `inject` 不这样，两者不对称；本稿跟 `check` 走 |
| D4 | Node 进程内的 Cordis 门控保留 | 它等的名字现在由 rutis 租借进来，门控随之通过；同一进程的行提供的服务照旧原生解析 |

## 三、开发者看到什么

### 插件作者、用 `rutis-host` 组装插件的人

行上写 `inject`，插件自己不声明：

```json
{ "id": "chat", "name": "./chat.ts", "inject": ["llm"] }
```

```ts
export default definePlugin({
  apply(ctx) {
    ctx.use<Llm>('llm').ask('hi')     // 行 inject 里的 llm：在了才运行到这里
  },
})
```

正常时：宿主或别的行还没有 `llm`，`rutis-host status` 打印 `chat: waiting for llm (no row provides it)`，六种语言一样，与 `rutis-host check` 说的一致。`llm` 来了，行启动，插件拿到它（[声明集合](design-declared-services-2026-10-29.md) D1）。`llm` 走了，行停下；回来后重新启动，与插件 `inject` 里的名字一样。给一行加上 `inject`，这一行重启一次。其他语言写法不同之处只在插件代码（`ctx.use`/`Use`），配置与输出相同。

出错时：

| # | 怎样引起 | 输出（逐字） | 码 | 怎样改 |
| --- | --- | --- | --- | --- |
| 1 | 行 `inject` 有 `llm`，没有行提供它，宿主也没有 | `chat: waiting for llm (no row provides it)`；有相近的名字时括号里加 `; did you mean <名字>?`，有被禁用的行时见 [rutis-host](guide/rutis-host.md) 的等待原因表 | 行等待，不失败 | 让一行或宿主提供 `llm`，或从行的 `inject` 里去掉 |
| 2 | 行 `inject` 有 `clock`，应用把 `clock` 登记成 Rust 类型（服务目录不共享，D3） | Bun、Python、Go、Swift、Kotlin：行启动，`use('clock')` 报 `service clock is not available`；Node：rutis 里显示运行，进程内的 Cordis 门控等 `clock`，`apply` 不运行，没有输出 | `unavailable`（Node 无） | 这类名字跨不了语言：从运行时行的 `inject` 里去掉，改在 Rust 行里用 |

### 宿主持有者

用 `Resolved::with_foreign_scope` 写自己的 resolver 时，它的行同样按行 `inject` 里共享的名字等（§四 第 3 条）。

| # | 怎样引起 | 输出（逐字） | 码 | 怎样改 |
| --- | --- | --- | --- | --- |
| 3 | 自定义 resolver 的插件在自己的运行时里找到行 `inject` 里的共享名字，从不公告给 rutis | `<行 id>: waiting for <名字> (no row provides it)`，一直等 | 行等待，不失败 | 把这个服务公告给 rutis（像 `rows.announce`），或不把它写进行的 `inject` |

### 新增的概念

没有。没有新的配置；输出只多了运行时的行也会出现的 `waiting for <名字>`，与 Rust 行同一句。

## 四、兼容

三处可见的行为变化，前两处与 Rust 行一致：

- Bun、Python、Go、Swift、Kotlin 的行：行 `inject` 里的服务不在时，原来照常启动、`use` 报 `unavailable`；现在停在 `waiting for <name>`。
- 提供方撤回后，这些行原来继续运行；现在停下，等它回来。
- 用 `Resolved::with_foreign_scope` 的自定义 resolver：它的行同样按行 `inject` 里共享的名字门控（`check` 早就这样报）。把这些名字租借进运行时是插件自己的事（本仓库的运行时由 `JsRow::start` 租借）；插件若在自己的运行时里找到这个名字、从不公告给 rutis，这一行会一直等。`Resolved::foreign_scope` 的文档注释随之改写。本仓库里设它的都是运行时行的解析：`leaf_resolved_on`、`SessionResolver` 会话未连上时的 `offline`（行本来就等会话）、运行时里没有这个插件时的 `unavailable_resolved`（它在 `validate_config` 就失败，走不到门控）；前两种的行由 `JsRow::start` 租借。

Node 的行原来在 rutis 里显示运行、实际停在进程内的门控上；现在在 rutis 里显示等待。协议不变，`PROTOCOL` 不升。

## 五、实施

| 步 | 内容 | 完成条件 |
| --- | --- | --- |
| 1 | `desired.rs` 的 `Row::foreign_scope`：`inject` 取服务目录共享的名字，key 用 `shared_key_in`，按名字排序去重 | `combined_injects` 对同一份配置给出同样的顺序，不因顺序无谓重启 |
| 2 | `runtime.rs`：`JsFactory`、`JsRow` 加 `row_inject`（名字 → 租借的 key，不共享时为空），`JsModule::factory` 用服务目录构造；`JsRow::start` 把行 `inject` 里不在 `gated` 中的共享名字接在 `gated` 后面租借 | 门控的 key 与租借的 key 出自同一个函数 |
| 3 | 用例 `crates/rutis-loader/tests/row_inject_gate.rs`：Node 与 Python 的行等宿主的 `llm`、用它、撤回后停下、回来后重启；Node 行的行 `inject` 指向 Python 行提供的服务；带 isolate 的行 `inject`；加上 `inject` 只重启一次。`instance_runtimes.rs` 加一条实例里的 | 去掉第 1、2 步，`row_inject_gate.rs` 的四条都失败。实例那条断言 `ri` 经自己的租借、在本实例的 scope 里调用 `tools`（`tool_callers`）：`row_inject` 不租借时，它只能用同一进程里 `js` 行租借的共享那份，用例失败 |
| 4 | 文档：`plugin-api` 的 `ctx.use` 一行、guarantees 的 `loader/rows-gate-on-services` 与 `loader/isolate-and-inject`、[声明集合](design-declared-services-2026-10-29.md) §三、§四、§六、§七、[可选依赖](design-optional-deps-2026-10-28.md) §4.5、§六，`Resolved::foreign_scope` 与 `RowInfo::isolate` 的文档注释 | 链接检查、文档示例检查通过 |

## 六、后续

分组上的 `isolate` 到不了运行时。`Loader::row` 给出的 `isolate` 只有这一行自己的（`RowInfo::isolate`），不沿父分组往上找（`Loader::check` 的 `isolated_in` 会找）。分组写 `isolate: {llm: team}`、组内运行时的行 `inject` 里有 `llm` 时：rutis 里的门控在 `team` 里等，租借取到的也是 `team` 里那份，这一行经自己的租借读到的对象是对的；但共享的那份登记没有标签，落在进程的根上，同一进程里别的行也看得到。插件 `inject` 里的名字走同一段租借代码，这一点本稿之前就如此，不是本稿引入的；要修得让 `RowInfo` 带上父分组的 isolate，另起一稿。
