# 跨语言插件的可选依赖

[English](design-optional-deps-2026-10-28.en.md)

状态：已实现（分支 `claude/design-optional-deps`）。日期：2026-10-28（同日修订：可选依赖改为与 Rust 的 `Option<Arc<T>>` 相同的装载时快照，去掉每个可选名字一个子 fiber 的做法；补上另外三处构造行的路径、"两处都写"的报错阶段、行自己的 `inject`、Go 的文字、测试工具、Swift/Kotlin 的方法检查。同日复核：12 维度 + 完整性检查，15 条报出中 12 条确认、3 条丢弃，另 8 条完整性缺口 + 1 条未核实；按确认项修订；同日再补一处："被替换"按服务形态拆写——Marked 的调用过 provision 同一性检查，槽位换了提供即报 `unavailable`，只有手写的 `HostDispatch` 留旧对象，实施步 1 用例随拆）。基准：`main` `4c58a3c5`（0.19.1）。2026-10-30 从 `7e57170d` 移植过来：上游把 `RuntimeResolver` 换成了 `SessionResolver`，删去了 `RuntimeInstances`，各语言的测试工具改在真实宿主上运行，本稿的构造路径（§二、§4.3）与用例随之调整，行为不变；可选依赖随下一个次版本（0.20）发布。2026-10-31 修订 §4.4"被替换"：别的运行时的行在运行中替换自己的对象时，手里的句柄留着旧对象，直到那一行卸载（原稿报 `unavailable`，与已登记的 `boundary/5.1`"快照留着旧对象"冲突；Rust 里已拿到的 `Arc<T>` 也不因替换失效）；行卸载（停了或重启）后才报 `unavailable`。同日修订 O4：`use` 不再把 `optional` 里的名字算作已声明，与 Rust 相同（`Option<Arc<T>>` 不进 fiber 的 `declared_injects`，`ctx.require` 对它报 `Undeclared`；临时用例实测：`Deps` 与 `ctx.get` 都拿到，`require` 报 `dependency not declared`）；可选的服务只用 `get` 读，`use` 一个只写在 `optional` 里的名字报 `use` 原来那句（§五 第 2 条）。2026-10-31 rebase 到 main `d5aa3200`（0.19.3）：上游把 Node、Bun 测试工具里测试给的服务改为各自一行（#988），§4.6 随之改写；行为不变。
Issue：尚无，随本稿开。相关：[#683](https://github.com/arcships/rutis/issues/683) 的 `allowed_services`（[设计](design-host-plugin-limits-2026-10-27.md) §4.3）里"可选的依赖不在名单里"一条要靠本稿才有对象；[#676](https://github.com/arcships/rutis/issues/676) / #704 的 `not-declared`。

范围外：Rust 插件（已有 `Option<Arc<T>>` 与 `ctx.get`，不变）；行自己的 `inject`（rutis.json 一行上的）的可选版本；Cordis 3 / Koishi 的 `inject: { required, optional }` 写法；可选依赖出现或消失时重跑 `apply`、或跟随它租借与归还（Rust 的 `Option` 也不做，§三 O3）。

## 一、现状

插件作者要的：有 `llm` 就用它总结，没有就截断原文。

| 在哪写 | 怎样写 | 本稿之前的行为（基准 0.19.1） |
| --- | --- | --- |
| Cordis | 不写进 `inject`，用 `ctx.get('llm')` | 不门控；`get` 查服务表，提供方 ACTIVE 时返回，否则 `undefined`；不因出现、消失重跑 |
| Rust | `type Deps = Option<Arc<Llm>>`，或 `ctx.get::<Llm>()` | 不门控；`Option` 是装载时的快照：不进 `declared_injects`，所以不建反向边，提供方走了不驱逐、手里的 `Arc` 还在，后来才出现也不唤醒（[deps.rs](../crates/rutis/src/deps.rs)）；`ctx.get` 随时查 |
| TypeScript（leaf）、Bun、Python、Go、Swift、Kotlin | 只能写进 `inject` | 宿主没有 `llm` 时，这一行一直等：`Loader::status()` 里是 `waiting for llm` |
| Node（TS leaf）/ Bun / Python | 不写进 `inject`，调 `ctx.use('llm')` | 宿主有 `llm` 也拿不到：`service llm is not available to this plugin, which does not declare it in inject: add 'llm' to its inject`。同一进程里恰好有别的行声明了 `llm` 时又拿得到 |
| Go、Swift、Kotlin | 不写进 `Inject` | `Use` 先查插件声明再查服务表，无条件拒绝；别的行声明了也拿不到 |
| Cordis 插件在 Node 运行时里 | 不写进 `inject`，用 `ctx.get('llm')` | 宿主的 `llm` 没有被租借进进程：返回 `undefined`；别的行声明了它时才有 |

原因有两层。

- **声明层只有必需一种。** M1 原本设计了 `required: false` 的可选依赖（[M1 §5.2](design-multilang-m1-2026-10-04.md)），实现时去掉了：Cordis 4 的 `inject` 只有数组或"名字 → 拦截配置"两种写法，表达不了可选（[M1 §13](design-multilang-m1-2026-10-04.md)）。之后的 leaf 运行时都沿用了"只有 `inject`"。
- **租借跟着门控走。** M1 §5.4 把宿主服务从"运行时启动时注册给整个进程"改成"每一行按自己的依赖租借"。代码里同一个列表 `gated` 既是这一行的 `injects`（门控），又是 `JsRow::start` 租借的名字（[runtime.rs](../crates/rutis-loader/src/runtime.rs)）。不门控的名字没有地方租借。`lease_host_for` 另做一份进程内共享的登记，所以别的行不声明也可能读到，但那取决于别的行。

## 二、现有的接缝

| 接缝 | 在哪里 | 本稿怎样用 |
| --- | --- | --- |
| `rows.schema`、`plugins.read`、`--rutis-manifest` 报出 `inject`、`provides` | `RowSchema`（rutis-bridge `process.rs`；`PluginSchema` 与 `BinaryManifest` 内嵌它），各运行时的 `describe` | 加一个 `optional` |
| 四处从声明构造行 | loader 里：`SessionResolver`（`runtime/sessions.rs`）与 `BinaryResolver`（都经 `leaf_resolved_on`）、`DefinitionMounter::mount`（经 `JsFactory::with_keys`），实例里的副本经 `JsModule::scoped`；bridge 里：`SessionHandle::run`（`session/plugin/run.rs` 的 `RunRow`，`sessions.open` 用它） | loader 的三处共用一个函数处理 `optional`；`RunRow` 在 bridge 里用不到它，自己做同样的重名检查与租借。`RunRow` 只为报出 `leaf` 特性的运行时（Bun、Python、Go、Swift、Kotlin）租借；Node 运行时报的是 `leaf.js`，`RunRow` 本来就不为它租借 `inject`，`optional` 也一样（补上要 bridge 像 loader 那样按服务目录判断，不在本稿） |
| 门控与租借共用 `gated` | `JsFactory.gated` → `injects()` 与 `JsRow::start` 的租借循环 | `optional` 另存一份，只进租借，不进 `injects` |
| 为一行租借宿主服务 | `Process::lease_host_for`：共享登记加这一行自己的标签；最后一个租约释放时 `hosts.withdraw`；运行时不支持 `scopes` 时只有共享租约，调用不点名行 | 可选的名字用同一个函数；无 `scopes` 时与必需租约一样退化为共享租约 |
| 租约在行卸载之后归还 | `JsRow::apply` 返回的清理里，先 `unload_row` 再 `release(leases)`（"插件不会在自己卸载之前看到它注入的服务消失"） | 可选的租约放进同一个 `leases`，顺序不变 |
| 各语言的 `use` | 先查进程里的服务表，查不到时没声明的报 `not-declared`，声明了的报 `unavailable`；"声明" = 插件的 `inject` ∪ `optional`；Node/Bun/Python 再加行的 `inject`，Go/Swift/Kotlin 目前丢弃行的 `inject` | `use` 的声明集合不变（不含 `optional`，与 Rust 的 `require` 相同）；`get` 另加，它的声明集合是 `inject` ∪ `optional`；Go/Swift/Kotlin 的行 `inject` 缺口不在本稿修（已由[声明集合](design-declared-services-2026-10-29.md)补上：六种语言的声明集合都含行的 `inject`） |

## 三、决策

| # | 决策 | 为什么；放弃的做法 |
| --- | --- | --- |
| O1 | 每种语言在 `inject` 旁边加一个列表 `optional`；`rows.schema`（以及 `plugins.read`、`--rutis-manifest`）报出 `optional: string[]` | `inject` 在每种语言里都只表示必需，不改它的意思。放弃 `inject: { llm: { required: false } }`：Cordis 4 把对象的值当拦截配置。放弃在名字上加标记（`'llm?'`）：七种语言的校验、报错、测试工具都要解析它。放弃各语言自己的可空类型：Go 没有，`rows.schema` 终究要一个名字列表。放弃不声明、把宿主允许的服务全部租借进每个进程（让 `get` 像 Rust、Cordis 那样读整张表）：每个进程要为每个宿主服务做一次 `hosts.provide`；没有这一行自己的租约，调用认不出是哪一行，需要调用行的宿主方法会失败 |
| O2 | 可选的名字不门控：不进 `injects`、`requires`，不出现在 `waiting for …`、`StillWaiting` 里 | 与 Rust、Cordis 相同 |
| O3 | 与 Rust 的 `Option<Arc<T>>` 相同：装载时快照。行启动时看得到就租借，租约一直留到行停止；之后才出现的不租借，提供方走了也不归还；都不重启行。跨进程的差别：提供方走后调用不会打到旧对象（手写的 `HostDispatch` 除外，由宿主自己负责）；Marked 在提供方撤销后、别的运行时的行在提供方行停了且进程还在时统一报 `service <name> is not available`（`unavailable`），进程没了报 `peer-gone`；`get` 仍返回租到的代理，失败在调用时出现 | 与 Rust 一致，插件作者不学新规则；实现只在现有的租借循环里加几行。代价与 Rust 相同：可选服务由另一个运行时的行提供时，有没有取决于启动顺序，要用它就重启这一行。跨进程不能像 Rust 那样握着 `Arc` 保证旧对象可用，所以这里把失败语义定死（§4.4、§五 第 3 条），而不是照抄"旧对象"。放弃"跟随服务"（每个可选名字挂一个子 fiber，到了就租、走了就还）：多一套机制，语义与 Rust 不同，子 fiber 的行归属、诊断、卸载顺序都要另外规定 |
| O4 | 读：每种语言加 `get(name)`，缺席时返回空值；`get` 的声明集合是 `inject` ∪ `optional`，对未声明的名字报 `not-declared`。`use(name)` 的声明集合不含 `optional`：一个只写在 `optional` 里的名字，`use` 与没声明的一样报原来那句 | `get`：不声明的名字永远不会被租借，`get` 会一直返回空值而不说原因（#676 的原则：说该声明什么）。Rust、Cordis 允许不声明就读，是因为它们读的是本进程的整张服务表。`use`：与 Rust 相同，`Option<Arc<T>>` 不进 fiber 的 `declared_injects`，`ctx.require` 对它报 `Undeclared`；可选的服务用 `get` 读。放弃"`use` 也认 `optional`、缺席报 `unavailable`"（原稿）：与 Rust 内核不一致 |
| O5 | 同一名字同时写在插件的 `inject` 与 `optional` 里：这一行失败；行自己的 `inject` 与插件的 `optional` 同名也同样失败（行 `inject` 是宿主配置的必需依赖，不能同时声明为可选）；插件两列表在加载器过滤前查，行 `inject` 在 loader 把它并入 `requires` 之前查 | 插件的两个列表在加载器的一个函数里查，各语言、各路径一致；行 `inject` 必须在 `combined_injects` 之前查，不能等到 `JsRow::start`——那一步可能因这个名字未就绪而永远不到。在过滤之前查：Cordis 运行时里，一个不共享的名字两处都写也是写错了。放弃"按必需处理"：会藏住作者没想清楚的地方 |
| O6 | Node 运行时里的 Cordis 插件：在插件上（与 `inject` 同一处）写 `optional`，读用 Cordis 自己的 `ctx.get` | Cordis 忽略不认识的 `optional`；本进程里 Cordis 行提供的服务 Cordis 本来就能 `get` 到，`optional` 只决定哪些共享名字要租借进来 |
| O7 | #683 的 `allowed_services` 不允许的可选名字：不租借 | 与 #683 §4.3、§五 第 4 条一致；本稿只给出它在哪里查 |
| O8 | `get` 一个未声明的名字时用新的一句（§五 第 1 条）；`use` 对未声明的名字、对只写在 `optional` 里的名字都用现有一句 | 对 `get` 的调用方，"加进 `inject`"是错的建议：那会让这一行等它。`use` 的调用方要的是必需依赖，原句对；不改它，现有的逐字测试也不动 |
| O9 | Go 的文字照 Go 现有的写法：字段名 `Optional`、`Inject`，名字用 `%q` | Go 的 `Use` 现在就写 `does not declare it in Inject: add "llm" to its Inject`，Go 作者写的是字段名；跨语言用例对 Go 比对 Go 的那一句 |

## 四、规则

### 4.1 声明

每种语言都是一个服务名列表，校验与 `inject` 相同（每项是字符串，否则定义时报错，文字照各语言现有的 `inject` 校验，把 `inject` 换成 `optional`）。

| 语言 | 写法 |
| --- | --- |
| TypeScript / JavaScript（`definePlugin`） | `optional: ['llm']` |
| Node 运行时里的 Cordis 插件 | 插件对象、函数或类上的 `optional = ['llm']`，与 `inject` 同一处 |
| Bun（模块导出） | `export const optional = ['llm']` |
| Python | 模块属性 `optional = ["llm"]`；或 `define_plugin(apply, inject=…, optional=["llm"])` |
| Go | `rutis.Plugin{..., Optional: []string{"llm"}}`；`--rutis-manifest` 的每个插件带 `optional` |
| Swift | `Plugin(name:, inject:, optional: ["llm"], …)`；`--rutis-manifest` 同上 |
| Kotlin | `Rutis.define(name, inject = …, optional = listOf("llm"), …)` |

### 4.2 协议

- `rows.schema`、`plugins.read`、`--rutis-manifest` 的每个插件都加 `optional: string[]`；不写时为 `[]`。Rust 侧 `RowSchema` 加 `#[serde(default)] pub optional: Vec<String>`（`PluginSchema`、`BinaryManifest` 内嵌它，随之都有）。
- `rutis-host check` 也读插件声明并打印 `inject:`（`main.rs` 的 `check`）：`meta` 加上 `optional`，非空时按 `inject` 同样的 JSON 形式打印一行 `optional: [...]`（为空时不打印，不写 `optional` 的插件输出不变）；可选缺席不算失败。
- 不加控制操作，不升 `PROTOCOL`：租借、归还用已有的 `hosts.provide` / `hosts.withdraw`。

### 4.3 加载器

1. 一个函数把声明分成两份，loader 里三处构造行的路径（§二）都用它（bridge 的 `RunRow` 照同样的规则自己做）：
   1. 先查重名：插件的 `inject` 与 `optional` 有同一个名字，失败（§五 第 4 条）。
   2. 再过滤：`gated` 照旧；`optional` 用同一规则（leaf 运行时取全部；Cordis 运行时只取 catalog 里共享的名字）。
2. `JsFactory`（以及 `JsModule`）加字段 `optional: Vec<(String, TypeKey)>`，键的算法与 `gated` 相同（共享名字按实例取键）。它不进 `injects()`，不进 `Declared.requires`；所以行不因它们 Pending，`Loader::status()` 不列它们，也没有 `StillWaiting`。
3. 重名在哪一步报：`SessionResolver`、`BinaryResolver` 在解析时报，是现有的解析错误（这一行 `unresolved`）；`DefinitionMounter` 在构造时报，是现有的构造错误；`RunRow` 在装载前报，是 `RowHandle::loaded` 的错误。消息相同，前缀照各自现有的格式（§五 第 4 条）。
4. 行自己的 `inject` 与插件 `optional` 的重名在 loader 把行 `inject` 并入 `requires` 之前查（`combined_injects` 一带），不能等到 `JsRow::start`——那一步可能因这个名字未就绪而永远不到；消息见 §五 第 4b 条。

### 4.4 行启动与停止

`JsRow::start`（[runtime.rs](../crates/rutis-loader/src/runtime.rs)）的现有顺序里加一步。行自己的 `inject` 与插件 `optional` 的重名在 loader 合并 `requires` 之前查（§4.3.4），`JsRow::start` 不做这项检查：

```
① for (name, key) in &self.gated                      （不变）
     require_key → 同一进程提供的跳过 → lease_host_for → leases.push
② for (name, key) in &self.optional                   ← 新加
     #683 不允许 → 跳过
     get_key::<dyn HostDispatch>(key)
       None → 跳过（这一行运行期间不再看它）
       Some(dispatch)：同一进程提供的跳过；否则 lease_host_for → leases.push
       租到时，标签处理与①相同（自己的标签推进 isolate；运行时没有 scopes 时只有共享租约，调用不点名行）
③ rows.load                                           （不变）
卸载：unload_row，然后 release(leases)                （不变：可选的租约一起归还）
```

- 之后才出现：不为这一行租借。这一行重启（`FiberView::restart()`、配置变更、`rutis-host` 重载这一行）后才一定看得到。在那之前，进程里已经有的——同一进程的行提供的，或本进程别的行为自己租借进来的（租借也在进程根上登记一份共享的，见 `Process::lease_host_for`）——`get` 照样读得到，与 Rust 的 `ctx.get` 读此刻的服务表一致。
- 提供方走了：不归还，这一行不停。`get` 仍返回租到的代理；调用按服务形态：手写的 `HostDispatch` 到旧对象（宿主自己负责）；Marked 在提供方撤销后、别的运行时的行在提供方行停了且进程还在时统一报 `service <name> is not available`（`unavailable`）；进程没了报 `peer-gone`。Node 目前在行停后抛 `TypeError`，本稿要求改成前一句的统一错误。
- 被替换：也按服务形态。手写的 `HostDispatch` 不换，留着旧的（重启这一行才换成新的）；Marked 的调用经 provision 同一性检查（[ctx.rs](../crates/rutis/src/ctx.rs)：槽位换了提供，`provision.is(binding)` 不再匹配）报 `service <name> is not available`（`unavailable`）；别的运行时的行：提供方行重启（卸载后再装载）同上一条，报 `unavailable`；行在运行中替换自己的对象，手里的句柄留着旧对象，直到那一行卸载，与手写的 `HostDispatch`、Rust 已拿到的 `Arc<T>` 以及 `boundary/5.1` 一致（六个运行时只在行卸载或重启时让它各槽位的句柄失效；替换只换掉槽位当前的句柄，旧句柄带着自己对象的方法表）。这是可见行为，实施步 1 的用例按形态拆成两条。
- 行在租借中途或 `rows.load` 失败：照现有的失败路径归还 `leases`，可选的一起。

### 4.5 读

| 语言 | 可选读（新） | 必需读（不变） |
| --- | --- | --- |
| TypeScript（leaf） | `ctx.get<T>(name): T \| undefined` | `ctx.use<T>(name): T` |
| Cordis 插件 | Cordis 的 `ctx.get(name)` | Cordis 的 `ctx[name]` |
| Bun | `ctx.get(name): unknown \| undefined` | `ctx.use(name)` |
| Python | `ctx.get(name) -> Any \| None` | `ctx.use(name)` |
| Go | `ctx.Get(name, target) (bool, error)`：缺席时 `false, nil` | `ctx.Use(name, target) error` |
| Swift | `ctx.get(name, P.self) throws -> P.Service?`，`ctx.get(name) throws -> Client?` | `ctx.use(…)` |
| Kotlin | `ctx.get(name, type): I?` | `ctx.use(name, type): I` |

- 读的是这个进程的服务表此刻有什么：本进程的提供方返回对象本身（与 `use` 相同）；租借进来的返回代理；没有返回空值。所以宿主服务以 §4.4 的快照为准，本进程别的行提供的服务按运行时自己的表随时变化（与 Cordis 的 `ctx.get` 相同）。
- `get` 的已声明 = 插件的 `inject` ∪ 插件的 `optional`；`use` 的已声明不含 `optional`（下一条）；Node/Bun/Python 再加行的 `inject`。Go/Swift/Kotlin 目前只认插件的 `inject`，本稿只把 `optional` 加进 `get` 的集合，不把行的 `inject` 一并修（那是一个独立的缺口，§六 记一条）（已由[声明集合](design-declared-services-2026-10-29.md)补上：六种语言的声明集合都含行的 `inject` 与插件自己提供的名字，`use` 也先查声明；rutis 为运行时的行门控、租借行 `inject`，是那份设计 §七 记的后续。）`get` 一个未声明的名字：报 `not-declared`（§五 第 1 条）。Cordis 插件例外：它用 Cordis 的 `ctx.get`，rutis 不改，未声明的照 Cordis 返回 `undefined`。
- `use` 一个只写在 `optional` 里的名字：与没声明的一样报 `service llm is not available to this plugin, which does not declare it in inject: add 'llm' to its inject`，码 `not-declared`（`use` 原来那句，§五 第 2 条），与 Rust 的 `ctx.require` 对 `Option<Arc<T>>` 报 `Undeclared` 相同。可选的服务用 `get` 读。
- Swift、Kotlin 的有类型的 `get`：服务在、但缺少代理要的方法时，照 `use` 的检查抛错，开头写 `get` 而不是 `use`（§五 第 6 条）；服务不在时返回 `nil` / `null`，不查方法。

### 4.6 测试工具

各语言的测试工具（Node / Bun `@arcships/rutis/testing`，Bun 下用 `bun test`；Python `testing`；Go `rutistest`；Swift / Kotlin `RutisTest`）在桌面上都在真实宿主上运行（`rutis-host test`，见状态段 2026-10-30 的移植说明；Swift 的 `RutisTest` 只在 macOS，其余语言支持哪些桌面平台以各自的指南为准）：测试给出的服务由测试工具在宿主上提供（Node、Bun 每个服务单独一行 `rutis-test/<名字>`，Go 另起的一行，Python、Swift、Kotlin 由测试自己的插件提供），被测插件的行照常装载、租借。Swift 在设备与 iOS 模拟器上的 `RutisTest` 不同：测试是自己装插件的应用（`RutisTestDevice.swift`），连上开发机的宿主但不经它装载这一行，下面第 4 条的重名检查与为这一行的租借都在宿主装载行时做，这条路径上没有。

- `optional` 里的名字不要求测试给出服务；没给时 `load` 照常成功，`get` 返回空值。
- 给了时 `get` 拿到它（`use` 不读 `optional` 里的名字，见 §4.5）。
- `get` 一个未声明的名字报 `not-declared`，文字与运行时相同。
- 两处都写（桌面）：宿主装载这一行时拒绝，与 `rutis-host run` 相同；测试工具把宿主的话连同它的前缀抛出，结尾是 §五 第 4 条的消息（`…: llm is in both inject and optional; keep it in one`）。

### 4.7 `allowed_services`（#683）

不在名单里的可选名字：§4.4 ②里跳过；`get` 返回空值；`use` 与调用报 #683 §五 第 4 条的文字。#683 先实现时，这一条随本稿一起做。

## 五、开发者看到什么

### 插件作者

正常时（TypeScript；别的语言只差 §4.1、§4.5 的写法）：

```ts
import { definePlugin } from '@arcships/rutis'

interface Llm {
  ask(question: string): Promise<string>
}

export default definePlugin({
  inject: ['notes'],                    // 必需：到齐才启动，撤销就停
  optional: ['llm'],                    // 可选：不等；启动时有就租借进来，留到这一行停止
  provides: { summary: { of: 'async' } },
  apply(ctx) {
    ctx.provide('summary', {
      async of(text: string) {
        const llm = ctx.get<Llm>('llm')
        return llm ? await llm.ask(`summarize: ${text}`) : text.slice(0, 200)
      },
    })
  },
})
```

- 启动时宿主有 `llm`：`of` 用它总结。
- 启动时宿主没有 `llm`：这一行照常 `active`，`of` 返回截断的原文。宿主之后才提供 `llm`：重启这一行前仍是截断的原文（除非同一进程里别的行为自己租借了 `llm`，见 §4.4）；重启后用上它。
- 运行中 `llm` 撤销：这一行不停。

Go：

```go
var llm struct{ Ask func(string) (string, error) }
ok, err := ctx.Get("llm", &llm)
if err != nil { return "", err }
if !ok { return text[:min(len(text), 200)], nil }
return llm.Ask("summarize: " + text)
```

出错时：

| # | 怎样引起 | 输出（逐字） | 码 | 记在哪 | 怎样改 |
| --- | --- | --- | --- | --- | --- |
| 1 | `get('llm')`，`llm` 未声明（Cordis 插件除外，§4.5） | `service llm is not available to this plugin, which does not declare it: add 'llm' to its optional (or to its inject, if it cannot run without it)`；Go：`service llm is not available to this plugin, which does not declare it: add "llm" to its Optional (or to its Inject, if it cannot run without it)` | `not-declared`，`data` `{service}` | 插件代码里抛出 | 照文字加进 `optional` |
| 2 | `llm` 只写在 `optional` 里，调 `use('llm')` | `service llm is not available to this plugin, which does not declare it in inject: add 'llm' to its inject`（Go：`… in Inject: add "llm" to its Inject`）（`use` 原来那句） | `not-declared`，`data` `{service}` | 插件代码里抛出 | 改用 `get` 并处理空值；离不开它就改写进 `inject` |
| 3 | 启动时租到的 `llm` 由别的运行时的行或 `#[rutis::service]`/Marked 提供，提供方后来撤销/停了：进程还在 | `service llm is not available`；提供方进程没了则是现有的 `runtime <名> exited/disconnected during the call`；手写的 `HostDispatch` 保持旧对象（宿主自己负责） | `unavailable` / `peer-gone` | 插件代码里抛出 | 重启这一行，或改用 `inject` |
| 4 | `llm` 同时写在插件的 `inject` 与 `optional` 里 | 消息 `llm is in both inject and optional; keep it in one`。经 `SessionResolver` / `BinaryResolver`：`<行 id>: unresolved: cannot load "<模块名>": llm is in both inject and optional; keep it in one`（`Loader::status()` 还有两格缩进与 ` in <instance>`）；经 `DefinitionMounter`：`failed: llm is in both inject and optional; keep it in one`（没有 headline）；经 `SessionHandle::run`：`RowHandle::loaded` 以这句失败 | 行失败 | 这一行的状态 | 删去一处 |
| 4b | `llm` 同时写在行的 `inject` 与插件的 `optional` 里 | `<行 id>: unresolved: entry "<行 id>" rejected: llm is in the row's inject and the plugin's optional; keep it in one` | 行失败 | 这一行的状态 | 删去一处 |
| 5 | 宿主用 `allowed_services` 不让用 `llm`（#683；示例行 id 与运行时名都是 `summarizer`） | `get` 返回空值；`use`、调用：`llm is not available to summarizer: the host does not let runtime summarizer use it` | `unavailable` | 插件代码里抛出 | 宿主的决定；插件按缺席处理 |
| 6 | Swift / Kotlin 有类型的 `get`，服务缺代理要的方法 | Swift：`get llm: LlmProxy calls ask, which the service does not have`；Kotlin：`get llm: Llm calls ask, which the service does not have` | 与 `use` 的同一检查相同 | 插件代码里抛出 | 改代理接口，或换提供方 |

`use` 对未声明名字的现有文字（`… does not declare it in inject: add 'llm' to its inject`，Go 为 `… in Inject: add "llm" to its Inject`）不变。

### 用 `rutis-host` 组装插件的人、宿主持有者

正常时：写了可选依赖的行不再出现在 `waiting for …` 里。没有新的配置。可选服务晚于这一行出现时，要用它就重启这一行。宿主持有者只在用 #683 的 `allowed_services` 时碰到 §4.7。

### 新增的概念

插件作者一个：`optional` 列表加 `get`。语义与 Rust 的 `Option<Arc<T>>` 相同（装载时看到的就是这一行运行期间的）。其他角色没有。

## 六、兼容

- 全是新增：不写 `optional` 的插件行为、输出不变，不进迁移指南。
- 旧运行时、新宿主：`rows.schema` 没有 `optional`，按 `[]`。
- 新 SDK、旧宿主：旧宿主的 `RowSchema` 忽略不认识的字段，不租借可选的名字；插件的 `get` 一直返回空值，不报错。这一组合在各语言指南里写明"要 rutis-host 0.20 或以后"。原计划实施步 2 有一条旧宿主上 `get` 为空的用例，落地时没有写（要在测试里起一个 0.19 的宿主）。
- Go/Swift/Kotlin 的 `use` 目前不认行 `inject`；本稿只把 `optional` 加进声明集合，不改变这一点（已由[声明集合](design-declared-services-2026-10-29.md)补上：六种语言的声明集合都含行的 `inject` 与插件自己提供的名字，`use` 也先查声明；rutis 为运行时的行门控、租借行 `inject`，是那份设计 §七 记的后续。）
- Cordis 3 / Koishi 的对象写法与 `optional` 并存的行为未核实，不在本稿范围。
- 协议版本、控制操作不变。

## 七、实施

| 步 | 内容 | 完成条件 |
| --- | --- | --- |
| 1 rutis-bridge、rutis-loader | `RowSchema.optional`；§4.3 的拆分函数，loader 的三处路径都用，bridge 的 `RunRow` 同样检查与租借；`JsFactory`、`JsModule` 的 `optional`；§4.4 ②；行 `inject` 与 `optional` 的重名检查 | 用 Node leaf 运行时的 Rust 测试：启动时在（`apply` 里 `get` 读到）；启动时不在（行 `active`，不在 `waiting for` 里，`report_waiting_after` 之后也没有 `StillWaiting`；之后出现仍为空，重启后读到）；提供方是 Rust 服务，运行中撤销（行仍 `active`；手写 `HostDispatch` 到旧对象，`#[rutis::service]`/Marked 报 `service <name> is not available`）；提供方是别的运行时的行，运行中停止（行仍 `active`，调用报 `service <name> is not available`，进程没了报 `peer-gone`）；被替换（按服务形态：手写 `HostDispatch` 旧对象留到重启，Marked 与别的运行时的行报 `unavailable`，各一条用例）；行卸载时可选租约在 `unload_row` 之后归还；租借中途或 `rows.load` 失败归还；三处路径的插件 `inject`/`optional` 重名（§五 第 4 条逐字）；在 `combined_injects` 之前查行 `inject` 与 `optional` 重名（§五 第 4b 条）；运行时没有 `scopes` 时只有共享租约；`rutis-host check` 打印 `optional`；Cordis 运行时只租借共享名字；`RowSchema.optional` 不写时为 `[]` |
| 2 各语言 SDK 与测试工具 | Node（leaf 与 Cordis 插件）、Bun、Python、Go、Swift、Kotlin：§4.1 的声明、schema、§4.5 的 `get` 与 `use` 的声明集合（`get` 认 `optional`，`use` 不认）；§4.6 的测试工具；各运行时 `RowService`/handle 在提供方行停后报 `unavailable`（Node 的 `TypeError` 修掉） | 每种语言各自的用例（落地时没有放进 rutis-bridge 由六种语言共跑：测试工具改在真实宿主上运行后，各语言的用例已经经过宿主与真实运行时；另有 loader 的 `optional_deps.rs`（Node、Python 运行时）与 `optional_deps_go.rs`（Go，`--features go`））：§五 第 1、2、4、6 条逐字，`get` 在 / 不在，`use` 对可选名报 `not-declared`，行的 `inject` 只在 Node/Bun/Python 算已声明，Go/Swift/Kotlin 维持现状（这一句已由[声明集合](design-declared-services-2026-10-29.md)改为六种语言都算）；每种语言的测试工具各一组，都断言 §4.6 第 4 条（两处都写由宿主拒绝，断言宿主的那句：Bun、Python、Go、Kotlin 断言结尾，Node 按正则、Swift 按包含；Bun 用 `@arcships/rutis/testing`）；§4.6 第 1–3 条即上面的 `get` 在 / 不在与 §五 第 1 条，不都经过测试工具（Kotlin 的第 3 条直接对 `RuntimeState` 断言）；§4.1 的非字符串校验、`optional` 不写为 `[]` 各有断言，Cordis 形态的插件由 `rutis-host check` 的用例断言（`optional` 照常列出，含非字符串的不能装载）；Cordis 未声明 `get` 返回 `undefined`（照 Cordis，rutis 不改）与旧宿主上 `get` 为空没有单独的用例（§六） |
| 3 文档 | plugin-api 指南的 API 表加 `ctx.get` 一行；各语言插件指南在"使用服务"处加可选依赖一段（含"晚到要重启"）；cordis 指南写 `optional`；`docs/guarantees.md` 登记，每条写明验证它的测试 | 文档链接检查、doc 例子检查通过 |
| 4 #683 | §4.7 | 随 #683 的步 2 |
