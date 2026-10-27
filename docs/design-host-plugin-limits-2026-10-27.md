# 宿主管住插件：每个插件一个进程、调用时限、问候时限、能用的服务

[English](design-host-plugin-limits-2026-10-27.en.md)

状态：草案，未实现。日期：2026-10-27。基准：`main` `103c0d22`。排期：0.19。
Issue：[#683](https://github.com/arcships/rutis/issues/683)。依据：[#663](https://github.com/arcships/rutis/issues/663) 的宿主持有者端到端评估（[报告](https://github.com/arcships/rutis/issues/663#issuecomment-6411076725)）。
与 0.18 的关系：建在 [0.18 的诊断](design-diagnostics-2026-10-26.md) 上——错误码沿用它 §4.3 的表并加两行，超时的调用就是它 §5.2 登记的进行中的调用，出错文字遵守它 §4.5。它 §6.1 "平台不设调用时限"不变，为什么不矛盾见 §三。
与其他 0.19 设计：[远程运行时一致性](design-remote-runtime-consistency-2026-10-26.md)（#695）的 `runtimes.remote` 条目接受本稿的 `limits` 与 `greeting`；[#185](https://github.com/arcships/rutis/issues/185)（宿主自己写插件）在运行中给每个 agent 写的插件加一个带 `Limits` 的命名运行时（§4.1 末尾），本稿不设计 agent 的工具；[#671](https://github.com/arcships/rutis/issues/671) 的宿主持有者指南 `docs/guide/open-host.md` 第 3 节讲本稿的选项（§七）；[#670](https://github.com/arcships/rutis/issues/670) 的 `rutis-host call` 是宿主的调用（§4.3、§4.4）。

范围外：
- **资源上限**（内存、CPU、文件数）：不做成选项。没有具体的使用方提出要限哪一项；操作系统的做法（Linux 的 cgroup 与 `systemd-run`、`prlimit`，macOS 与 Windows 各不相同）本来就按进程管，本稿让每个插件可以有自己的进程以后，这些做法就能按插件用。指南写明做法（§七）。
- **按服务（某个方法）设时限**：想给某个服务定时限的是调用它的那一方，0.18 §6.1 的各语言写法已经能做；宿主要防的是失控的第三方代码，按插件（进程）设就够了。
- **给共用进程里的某一行单设时限或能用的服务**：同一进程里的插件能用语言本身的办法互相拿到（Cordis 上下文、Python 的 `import`），能用的服务在那里管不住；超时后结束进程也会结束邻居。所以这些选项设在进程（运行时）上，一行要单独管就给它单独的运行时。
- **限制插件能提供哪些服务、能发哪些事件**：同名提供已经让后来的那一行失败（0.17.2、0.18 §九），宿主自己的服务在加载器之前提供；事件没有具体的使用方。
- **沙箱**：插件进程照旧以宿主用户的全部权限运行（文件、网络）。本稿管的是插件经 rutis 能做的事。
- 进程内的 Rust 插件（含 dylib）：与宿主同一进程，结束不了、也拦不住，不在本稿；宿主自己的 Rust 代码本来就可信。
- 按需启动 Node、Python 运行时、`rutis-host` 里"每行自动一个进程"的开关、取消某个进行中调用的命令：都没有具体的使用方。

## 一、场景与卡在哪

宿主持有者做 deskmate（命令行助手），把扩展点开放给第三方：插件注册"工具"，宿主调用工具的处理函数；宿主提供 `tools`、`notes` 两个服务给插件用。他要：

1. 一个插件崩溃或卡死，不连累别的插件；
2. 处理函数 30 秒不返回就停下它，卡在忙循环里的插件进程要结束掉，再按需重启；
3. 调试某个插件时，让它的进程停在第一行（`--inspect-brk`）等调试器连上；
4. 第三方插件只能用 `tools`、`notes`，用别的服务时被拒绝并说清楚。

0.17 里他读 crate 源码、写了约 160 行做到前两条的一部分，后两条做不到：

| 要的 | 现在怎样做 | 卡在哪 |
| --- | --- | --- |
| 每个插件一个进程 | 每个插件一个 `LocalRuntime::node(..).named(..)`，再自己写一个 `Resolver` 把每一行送到它自己的进程（约 60 行） | `RuntimeResolver::node` 接受任何 npm 名和文件，没有"这一行用这个运行时"的写法；rutis.json 每种语言只能有一个运行时 |
| 调用时限 | 每次调用外面套 tokio 的超时，超时后 `processes().kill()`（约 30 行） | 只管得住宿主自己发出的调用；插件调插件、`apply` 卡住都管不住 |
| 调试时的问候时限 | 做不到 | 本机运行时 30 秒内没问候就被结束（`LinkConfig::local_runtime()` 写死） |
| 能用哪些服务 | 做不到 | 没有有文档的办法 |

## 二、现有的接缝

| 接缝 | 在哪里 | 本稿怎样用 |
| --- | --- | --- |
| 一个运行时一个进程，可以起多个、各自命名 | `LocalRuntime::named`（`crates/rutis-bridge/src/runtime/local.rs`） | 不变；"每个插件一个进程"就是每个插件一个命名的运行时 |
| 按 `<运行时名>:` 前缀把行送到运行时 | `RuntimeResolver::modules`（Python、Bun、远程）、Go/Swift 的每个二进制一个运行时 | Node 也接受这种写法（§4.1），这是唯一缺的那一块 |
| 进程的结束 | `LocalRuntime::processes().kill()`；运行时的 `restart` 策略（rutis-host）、`BinaryRuntimes::supervise`（Go、Swift） | 超时后结束进程用它；结束后照现有的 `restart` 处理 |
| 进行中的调用 | 0.18 在 `RowService::invoke` 登记，调用方来自 `caller_row()` | 限时的就是这些调用；超时信息里的"谁调用谁"从这里来 |
| 取消传到对面 | 各语言已有（0.18 §6.1 的表） | 超时就是由宿主替调用方取消 |
| 问候时限 | 远程：`Retry::handshake`、`AccessPointConfig::handshake` 已能设；本机：`LOCAL_RUNTIME_GREETING` 30 秒写死 | 本机补一个设置；rutis.json 两种都能写 |
| 只租借写进 `inject` 的服务 | 叶子运行时只把 `inject` 里的服务租借进进程，没写的调用报 `not-declared` | 能用的服务在这里多一道检查 |
| 运行中增删插件与运行时 | loader 的编辑接口增删行；`LocalRuntime`、`RuntimeRowsPlugin` 都是普通插件，`root.plugin(..)` 加入、卸载它的 fiber 就结束进程（关闭通道），不碰别的运行时 | 缺一处：`Chain` 在建 loader 时就定下，运行中加的运行时的解析器加不进去（§4.1 末尾） |
| `share_registered_only`、读取拦截 | `ServiceCatalog`、`intercept_require_as` | **不用**：前者决定哪些名字跨语言，剩下的名字交给 Cordis 原生解析，Node 插件照样拿得到，不是限制；后者按 Rust 类型拦 Rust 的读者，管不到别的语言的行。指南写明这两者不是权限 |

## 三、与 0.18"平台不设调用时限"为什么不矛盾

0.18 §二说"平台只看，不替业务做决定"：一个调用该等多久是业务的决定，所以 rutis 不设、也不默认任何调用时限。本稿不改这一条：

- **做决定的是宿主持有者**，他就是这个业务的主人。宿主把插件开放给第三方时，"第三方的代码最多占用 30 秒"正是他的业务决定；rutis 只提供写下这个决定的地方，不替他定数值。
- **默认关**：不写 `limits` 时与现在、与 Cordis 完全相同。
- **调用方自己限时做不到这两件事**：调用方不是宿主时（插件 A 调插件 B，宿主不在调用链上）；以及卡在同步代码里的插件，取消传过去也停不下，只能结束进程。

0.18 §6.1 末尾已经写明"宿主想限制第三方插件的资源，属于 0.19 的 #683（宿主的选项，默认关）"，本稿就是这件事。两者同时存在时，先到的那个时限生效。0.18 §6.3 的握手时限（本机 30 秒、网络 10 秒）仍是平台的默认值，本稿只让宿主能改。

## 四、决策

| # | 决策 | 章节 |
| --- | --- | --- |
| H1 | 每个插件（或每组插件）一个进程 = 一个命名的运行时；Node 行可以写 `<运行时名>:<npm 名>` 指定运行时；rutis.json 加 `runtimes.local` 列出更多本机运行时；Rust 嵌入可用 `RuntimeSet` 在运行中增删运行时 | 4.1 |
| H2 | 时限与能用的服务是运行时上的一组选项 `limits`，管这个运行时里的所有插件；rutis.json 顶层的 `limits` 是所有运行时的默认 | 4.2 |
| H3 | 调用时限 `call`：进入这个运行时里插件代码的每次调用（按名字、按引用、监听器、行的启动）超过它就失败，错误码 `timeout`；`stop` 决定超时后停掉调用（默认）还是结束进程 | 4.3 |
| H4 | 能用的服务 `services`：这个运行时的插件只能用列出的、来自运行时之外的服务；必需的依赖不在列表里时这一行启动失败，其他调用报 `not-allowed` | 4.4 |
| H5 | 问候时限 `greeting`：每个运行时条目可设秒数或 `"off"`，本机与远程同一个字段，默认不变（30 秒、10 秒） | 4.5 |
| H6 | 都是默认关的选项，不改任何已有行为；错误码表加 `timeout`、`not-allowed` 两行 | 六 |

### 4.1 每个插件一个进程

**Rust 嵌入**：`RuntimeResolver::node(handle)` 照旧接受 npm 名和文件（不改，已有的嵌入不受影响），另外接受 `<它的运行时名>:<npm 名>`；加 `.prefixed()`，只接受带前缀的写法。Python、Bun 本来就是前缀写法，不用改。

```rust
for plugin in &plugins {                      // 每个第三方插件一个进程
    let runtime = LocalRuntime::node(&runtime_pkg, &plugin.dir).named(&plugin.id).limits(limits.clone());
    let rows = Arc::new(RuntimeResolver::node(runtime.handle()).prefixed().with_catalog(&catalog));
    root.plugin(runtime);
    chain = chain.with_shared(rows.clone());
    resolvers.push(rows);                     // 加载器之后各挂一个 RuntimeRowsPlugin，与现在相同
}
// 行：{ "id": "wordcount", "name": "wordcount:wordcount-plugin" }
```

**rutis.json**：`runtimes.node`、`py`、`bun` 照旧是每种语言的默认运行时。新加 `runtimes.local`，每一条是一个命名的本机运行时：

```json
"runtimes": {
  "node": { "project": "." },
  "local": [
    { "name": "wordcount", "language": "node", "restart": "on-failure" },
    { "name": "agent",     "language": "python", "limits": { "services": ["notes"] } }
  ]
},
"rows": [
  { "id": "wordcount", "name": "wordcount:wordcount-plugin" },
  { "id": "weather",   "name": "agent:weather" }
]
```

- `language` 是 `node`、`python`、`bun`；其余字段与这种语言的条目相同（`project`、`runtime`、`python`、`program`、`restart`），没写的取 `runtimes.<语言>` 的值，那一条也没有时取默认。Go、Swift 本来就是一个二进制一个运行时，不在 `local` 里。
- `name` 按运行时名的规则（两个以上的 `a-z`、`0-9`、`-`），不能与 `node`、`py`、`bun`、Go/Swift 运行时、`runtimes.remote` 的名字重复，重复时启动报错。
- 一组插件共用一个进程：几行写同一个前缀。一个插件一个进程：一条 `local` 一行。
- 和语言默认运行时一样随宿主启动；`dev` 改一个插件时只重启它所在的运行时（与现在的规则相同：不为一个插件重启共用的运行时）。
- 远程的 Node 运行时同样接受 `<名字>:<npm 名>`（#695：一台与多台写法一致）。

**运行中增删一个带限制的运行时（Rust 嵌入，#185 用）**：运行时和它的 `RuntimeRowsPlugin` 本来就是可以随时加入、卸载的插件，卸载运行时就结束它的进程，别的运行时不受影响；缺的只是解析器——`Chain` 在建 loader 时定下，之后加不进新的解析器。最小的补法：rutis-loader 加一个解析器 `RuntimeSet`，建 loader 时放进 `Chain` 一次，之后 `add(rows)` / `remove(name)`，把 `<名字>:` 开头的行交给同名运行时的 `RuntimeResolver`，没有这个名字时与现在一样报没有这个插件。

```rust
// 启动时：chain = chain.with_shared(set.clone());
// 加一个 agent 写的插件：先运行时，再行
let runtime = LocalRuntime::python(&dir).named("agent-weather").limits(limits.clone());
let rows = Arc::new(RuntimeResolver::modules(runtime.handle()).with_catalog(&catalog));
let fibers = (root.plugin(runtime), root.plugin(RuntimeRowsPlugin::new(rows.clone())));
set.add(rows);
// loader 编辑：插入 { "id": "weather", "name": "agent-weather:weather" }
// 移除：先用 loader 编辑删掉这一行，再卸载两个 fiber（进程随之结束），最后 set.remove("agent-weather")
```

**rutis-host 重读配置时的 `runtimes.local`**：rutis-host 建 loader 时用同一个 `RuntimeSet` 承载 `runtimes.local` 的所有条目，所以重读配置（`dev`、`run --watch-config` 与 SIGUSR1、[#185](https://github.com/arcships/rutis/issues/185) 的管理服务写配置）时按条目处理，不要求重启宿主：

| 条目 | 处理 |
| --- | --- |
| 在运行、内容没变 | 不动 |
| 新增 | 运行中加上（运行时、`RuntimeRowsPlugin`、`set.add`），再加它的行；其他运行时不重启 |
| 删掉 | 先删它的行，再卸载运行时（进程结束），最后 `set.remove` |
| 内容变了（任一字段，含 `limits`、`greeting`） | 只重启这一个运行时（`FiberView::restart`），它的行等它回来后重新解析 |

现在 `restart_needed`（`crates/rutis-host/src/host.rs`）对 `runtimes.remote` 和各语言条目的增加、修改都要求重启宿主；语言条目被删时已经在重读中停掉它的运行时（`Reloaded::stopped`）。本稿只把 `runtimes.local` 从这个检查里拿出来，按上表处理；`runtimes.node`、`py`、`bun`、`go`、`swift` 与 `remote` 的规则不变（运行中新增仍要求重启宿主，#691 依赖这一条）。

### 4.2 选项放在运行时上

```jsonc
"limits": {
  "call": 30,                  // 秒；不写为不限
  "stop": "call",              // 超时后停掉什么："call"（默认）或 "process"
  "services": ["tools", "notes"]  // 不写为不限
}
```

- 可以写在顶层（所有运行时的默认）和每个运行时条目上：`runtimes.node`、`py`、`bun`、`go`、`swift`（管它的每个二进制）、`local` 的每一条、`remote` 的每一条（含 `instances` 的每条连接）。条目上写了的字段覆盖顶层的同名字段。
- Rust：`rutis_bridge::runtime::Limits`，设在 `RuntimePlugin::limits`、`LocalRuntime::limits`、`BinaryRuntimes::limits` 上。没有"整个宿主"的 Rust 接口：宿主把同一个 `Limits` 交给每个运行时。

```rust
let limits = Limits::new().call(Duration::from_secs(30)).stop(Stop::Process).services(["tools", "notes"]);
```

### 4.3 调用时限

- **管哪些调用**：这一侧进入这个运行时里插件代码的每次调用——按名字调用服务（`RowService::invoke`，0.18 在这里登记进行中的调用）、按引用传过来的函数（deskmate 的工具处理函数就是这种）、事件监听器、行的启动（导入入口模块与 `apply`）。调用方是宿主、别的运行时的插件还是同一宿主里的 Rust 插件都一样；`rutis-host call`（#670，经 `call_service`，记为"宿主"）进入受限的运行时，同样受这个时限。从发出到结束计时，远程运行时包括网络时间。
- **不管的**：同一运行时里插件之间的调用（不经过宿主，0.18 的状态里也看不到）；清理（关闭的时限归宿主，0.18 §6.2）。
- **超时以后**：调用方收到错误码 `timeout` 的错误；同时向对面取消这个调用（各语言已有的取消）。行的启动超时，这一行失败。
- **`stop: "process"`**：再结束这个运行时的进程（`processes().kill()`），之后按它的 `restart` 处理（rutis-host 的 `no` / `on-failure` / `exit`，Go/Swift 的 `RestartPolicy`；Rust 嵌入由宿主决定是否重启：对 `root.plugin(runtime)` 返回的 fiber 调用已有的 `FiberView::restart()`，起一个新进程，它的行随之回来；Go/Swift 用已有的 `BinaryRuntimesHandle::restart(名字)`。不需要新接口）。同一进程里其他进行中的调用报 0.18 已有的 `peer-gone`。只用于本机运行时：写在远程运行时上时启动报错（宿主结束不了别处的进程）。
- **为什么要 `process`**：取消是协作的。TypeScript 的 promise 取消不了（0.18 §6.1），Python 的 `def` 方法和 JS 的忙循环占住整个进程，取消传过去也停不下；这时只有结束进程能让别的插件继续。
- 调用方自己也限了时：先到的那个生效。

### 4.4 能用的服务

- `services` 列出这个运行时的插件能用的、**来自运行时之外**的服务名（宿主的服务、别的运行时的行提供的服务）。同一运行时里的行互相提供的服务不受限制（在同一进程里本来拦不住）。
- 检查在加载器解析这一行时做，拿的是插件声明的 `inject`：
  - 必需的依赖不在列表里：这一行启动失败，原因的错误码是 `not-allowed`。不让它等：等一个永远不会给它的服务只会让人以为卡住了。
  - 可选的依赖不在列表里：不租借进进程，插件看到它不存在；插件仍去调用时报 `not-allowed`。
- `rutis-host check` 用同样的规则静态检查，以 1 退出。
- 只管插件用服务，不管宿主：宿主自己的调用（Rust 里的 `call_service`、`rutis-host call`）不受 `services` 限制。
- 不写 `services` 时与现在相同。

### 4.5 问候时限

- rutis.json：每个运行时条目（`node`、`py`、`bun`、`go`、`swift`、`local`、`remote`）可写 `"greeting": 120`（秒）或 `"greeting": "off"`（不限，调试器停在第一行时用）。不写时本机 30 秒、远程 10 秒，与现在相同。不放进顶层 `limits`：它是运行时启动的事，按运行时调。
- Rust：`LocalRuntime::greeting(Option<Duration>)`（`None` 不限），`BinaryRuntimes::greeting` 同样；远程已有的 `Retry::handshake`、`AccessPointConfig::handshake` 不变（#695 M7：Rust API 不变），要不限时写 `Duration::MAX`。
- 关掉以后，一个启动了却一直不说话的进程会一直停在"启动中"；0.18 的状态里看得到它在等哪个运行时。

## 五、开发者看到什么

宿主持有者写的配置与看到的输出（rutis-host；Rust 嵌入的错误文字相同，只是没有第 6 条的提示行）。

| # | 输入 | 输出 |
| --- | --- | --- |
| 1 | `wordcount` 的处理函数 30 秒没返回（`call: 30`） | 宿主调用方：``a function of wordcount did not finish within 30s (the host's limit for runtime wordcount); the call was cancelled``；按名字调用时写 `chat: call to weather.tomorrow did not finish within 30s (the host's limit for runtime agent); the call was cancelled`。错误码 `timeout`，`data` 为 `{runtime, limit, service?, method?}`，`site` 是被调用的那一行 |
| 2 | 同上，`stop: "process"`，`restart: "on-failure"` | 上一条的结尾改为 `; runtime wordcount was ended`，接着 `runtime wordcount: down (ended by the host: a function of wordcount did not finish within 30s)`，再是现有的重启行 |
| 3 | `apply` 卡住 | `weather: did not finish starting within 30s (the host's limit for runtime agent)`；这一行失败，其他行照常 |
| 4 | `wordcount` 必需的依赖里有 `llm`，`services: ["tools", "notes"]` | `wordcount: uses llm, which the host does not let runtime wordcount use (allowed: tools, notes)`；这一行失败；`check` 打印同一句并以 1 退出 |
| 5 | 可选的依赖 `llm` 不在列表里，插件仍调用 | 插件代码里的错误：`llm is not available to wordcount: the host does not let runtime wordcount use it`，错误码 `not-allowed`，`data` 为 `{service, runtime}` |
| 6 | 本机运行时 30 秒没问候 | 0.18 的那一行（运行时 `down`，写明 30 秒内没有问候）；rutis-host 后面再接一行 `to wait longer, or not at all while debugging, set "greeting" on runtime wordcount` |
| 7 | `greeting: "off"`，运行时停在 `--inspect-brk` | 运行时显示为启动中；`rutis-host status`：`runtime wordcount: starting`，它的行 `waiting for runtime wordcount` |
| 8 | `stop: "process"` 写在远程运行时上 | 启动报错：`runtime gpu: limits.stop "process" needs a runtime this host starts; gpu is remote` |
| 9 | `local` 的名字重复 / 语言写错 | `runtimes.local: wordcount is already the name of another runtime`；`runtime wordcount: the language is node, python or bun, not go` |
| 10 | Rust 嵌入运行中 `set.add` 一个运行时再插入它的行，之后删行、卸载运行时 | 加入时只有这个运行时的启动行与这一行的状态行；移除后 `runtime agent-weather: stopped`，进程结束，其他运行时的 pid 不变、行照常 |
| 11 | rutis-host 运行中，rutis.json 的 `runtimes.local` 新增 `agent-notes` 一条和它的行（手改、`--watch-config` 或管理服务写入） | 重读的那一行在现有的 `1 row changed` 之后接 `, runtime agent-notes started`；其他运行时不重启 |
| 12 | 删掉 `agent-notes` 一条和它的行 | `1 row changed, runtime agent-notes stopped`（与现在语言运行时被删时的写法相同），进程结束 |
| 13 | 改了 `agent-notes` 的 `limits.call` | `runtime agent-notes restarted`；只有它的行短暂等它 |
| 14 | 运行中新增 `runtimes.bun` | 与现在相同：`runtimes.bun added: restart the host`，配置不生效 |
| 15 | 行写了 `wordcunt:wordcount-plugin`（前缀写错） | 与现在没有解析器接受这个名字时相同（0.18 的文字），不另加输出 |

插件作者不写任何新东西：他的代码看到的只是普通的错误（名字、消息、错误码），与别的出错一样处理。

## 六、兼容

- 全部是新增的、默认关的选项：不写时行为、输出与 0.18 相同，不进《迁移到 0.19》。
- `RuntimeResolver::node` 多接受 `<自己的名字>:<npm 名>`：这种名字以前没有解析器接受（报没有这个插件），现在由同名的运行时接受，不影响已有的行。
- 错误码表（0.18 §4.3）加两行，不改已有的行：

| code | 什么时候 | `data` |
| --- | --- | --- |
| `timeout` | 进入运行时的调用超过宿主设的时限 | `{runtime, limit, service?, method?}` |
| `not-allowed` | 用了宿主不让这个运行时用的服务 | `{service, runtime, allowed?}` |

- 协议不变：取消、进程结束都是已有的消息；运行时一侧不用升级。

## 七、指南：「管住插件」

按 #671 的安排：讲解写进宿主持有者指南 `docs/guide/open-host.md` 第 3 节「管住插件」（两份，例子是 #671 的 `jot`）；rust-host.md 与 rutis-host.md 只在参考表里加本稿的选项（`Limits`、`greeting`、`prefixed()`、`RuntimeSet`；rutis.json 的 `limits`、`greeting`、`runtimes.local`）。第 3 节的内容：

1. 每个插件一个进程：rutis.json 的 `runtimes.local` 与前缀写法；Rust 的 `named` + `prefixed()` 的循环，与运行中用 `RuntimeSet` 增删（§4.1 的例子）。
2. `limits`：三个字段、写在哪里、谁覆盖谁；`stop: "process"` 与 `restart` 一起用。
3. `greeting`：调试器停在第一行时关掉。
4. 不是限制的东西：`share_registered_only` 与读取拦截各一句；同一进程里插件能互相拿到；插件进程有宿主用户的全部权限。
5. 资源上限的做法：插件有自己的进程后，用操作系统的工具按进程限——
   - 整个宿主：Linux 上 `systemd-run --scope -p MemoryMax=2G -p CPUQuota=200% rutis-host run`（cgroup 管住它启动的所有运行时进程）；
   - 某个运行时：把解释器换成一个先限再 `exec` 的脚本（`runtimes.py.python`、`runtimes.bun.program`、Rust 的 `LocalRuntime::interpreter`），例如 `prlimit --as=1073741824 -- python3 "$@"`；
   - macOS、Windows 没有同样的命令行办法，由宿主自己用系统接口（Windows 的作业对象）。
6. 插件 API 指南的"同一运行时里的插件共用一个进程"一节加一句：宿主可以把插件放进单独的进程并设时限，插件收到的是 `timeout`、`not-allowed` 错误。

## 八、实施

一个 issue（#683），分四步：

| 步 | 内容 | 完成条件 |
| --- | --- | --- |
| 1 rutis-bridge | `Limits`、`Stop`；`RuntimePlugin::limits`、`LocalRuntime::limits` / `greeting`；在进入运行时的几处（`RowService::invoke`、按引用的函数调用、监听器、行的加载与 `apply`）计时、取消、按 `stop` 结束进程；`timeout`、`not-allowed` 两个错误码 | 每种语言（Node、Bun、Python、Go、Swift）各一个测试：异步方法超时被取消、同步占住时 `stop: process` 结束进程且其他运行时不受影响、按引用的函数超时、`apply` 超时；本机 `greeting` 设为 1 秒与 `None` 各一个测试 |
| 2 rutis-loader | Node 的前缀写法与 `prefixed()`；`RuntimeSet`；解析时检查 `services`（必需的失败、可选的不租借）；`BinaryRuntimes::limits` / `greeting` | 两个 Node 运行时各自的行进各自的进程，其中一个崩溃另一个照常；运行中 `RuntimeSet` 加入、移除一个带限制的运行时与它的行，进程结束，其他运行时的进程不变；`services` 的三种情况（允许、必需不允许、可选不允许后调用）；同一运行时里的行互相提供不受限 |
| 3 rutis-host | 顶层与各条目的 `limits`、`greeting`；`runtimes.local` 经 `RuntimeSet` 承载，重读配置时按 §4.1 的表增、删、重启单个运行时（`restart_needed` 不再管它，`Reloaded` 加 started / restarted）；`check` 的静态检查；§五 的输出；`dev` 只重启改动的插件所在的运行时 | §五 每一条有逐字断言的测试，出错文字不含 0.18 §4.5 的禁用词；重读配置时新增、删掉、修改一条 `local`，其他运行时的 pid 不变；deskmate 的场景端到端：两个 TS 插件各一个进程、忙循环的那个被结束并重启、用了不让用的服务被拒 |
| 4 文档 | open-host.md 第 3 节（#671 的指南，在本步之后写）、rust-host.md 与 rutis-host.md 参考表里的选项（§七），`docs/guarantees.md` 登记 `host/plugin-limits`（每条写明验证它的测试） | 文档链接检查、doc 例子检查通过 |

对应 issue 的「完成」：步 1–3 是"选项与测试"，步 4 是"指南一节"（issue 原文写在 rust-host.md，按 #671 改为 open-host.md 第 3 节）。
