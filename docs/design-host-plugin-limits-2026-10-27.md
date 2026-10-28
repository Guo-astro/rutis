# 宿主管住插件：每个插件一个进程、调用时限、问候时限、能用的服务

[English](design-host-plugin-limits-2026-10-27.en.md)

状态：定稿，未实现。日期：2026-10-27（同日复查：删去 rutis.json 的写法、`runtimes.local`、`prefixed()`、顶层与逐字段覆盖、远程运行时上的选项、`not-allowed` 码）。基准：`main` `103c0d22`。排期：0.19。
0.19 平台清理（[§5.5](design-platform-cleanup-2026-10-28.md)）删去了 H2 调用时限（`call_limit`、`kill_on_timeout`、错误码 `timeout`）与 H3 能用的服务（`allowed_services`）：调用方自己限时，超时后宿主结束进程并用 `FiberView::restart()` 重启；能用哪些服务由宿主自己决定。留下的是每个插件一个进程（`RuntimeResolver::modules`）与问候时限（`greeting`）。下文按原样保留。
Issue：[#683](https://github.com/arcships/rutis/issues/683)。依据：[#663](https://github.com/arcships/rutis/issues/663) 的宿主持有者端到端评估（[报告](https://github.com/arcships/rutis/issues/663#issuecomment-6411076725)）。
与 0.18 的关系：建在 [0.18 的诊断](design-diagnostics-2026-10-26.md) 上——错误码沿用它 §4.3 的表并加一行 `timeout`，超时的调用就是它 §5.2 登记的进行中的调用，出错文字遵守它 §4.5。它 §6.1 "平台不设调用时限"不变，见 §三。
与其他 0.19 设计：[#185](https://github.com/arcships/rutis/issues/185) 把 loader 作为服务 `loader` 交给插件（与 Cordis 的 `ctx.loader` 相同），宿主想限制哪些插件能用它时用本稿的 `allowed_services`（§4.3）；[#671](https://github.com/arcships/rutis/issues/671) 的宿主持有者指南 `docs/guide/open-host.md` 第 3 节讲本稿的选项（§七）。

范围外（都没有具体的使用方）：rutis.json 里的写法（评估里的宿主是 Rust 嵌入）、远程运行时上的这些选项、Go/Swift 的 `BinaryRuntimes` 上的这些选项、资源上限（内存、CPU）、按服务（方法）设时限（调用方自己限时，0.18 §6.1）、监听器与 `apply` 的时限、沙箱、进程内的 Rust 插件。

## 一、场景与卡在哪

宿主持有者用 Rust 嵌入做 deskmate（命令行助手），把扩展点开放给第三方：插件注册"工具"，宿主调用工具的处理函数（按引用传过来的函数）；宿主提供 `tools`、`notes` 两个服务给插件用。他要：

1. 一个插件崩溃或卡死，不连累别的插件；
2. 处理函数 30 秒不返回就停下它，卡在忙循环里的插件进程要结束掉；
3. 调试某个插件时，让它的进程停在第一行（`--inspect-brk`）等调试器连上；
4. 第三方插件只能用 `tools`、`notes`，用别的服务时被拒绝并说清楚。

| 要的 | 0.17 里他怎样做 | 卡在哪 |
| --- | --- | --- |
| 每个插件一个进程 | 每个插件一个 `LocalRuntime::node(..).named(..)`，再自己写一个 `Resolver`（约 60 行） | 他用的 `RuntimeResolver::node` 接受任何 npm 名和文件，第一个就把所有行都接走了；没有文档说该用哪个 |
| 调用时限 | 每次调用外面套 tokio 的超时，超时后 `processes().kill()`（约 30 行） | 只管得住宿主自己发出的调用；插件调插件管不住 |
| 调试时的问候时限 | 做不到 | 本机运行时 30 秒内没问候就被结束（`LOCAL_RUNTIME_GREETING` 写死） |
| 能用哪些服务 | 做不到 | 没有有文档的办法 |

## 二、现有的接缝

| 接缝 | 在哪里 | 本稿怎样用 |
| --- | --- | --- |
| 一个运行时一个进程，可以起多个、各自命名 | `LocalRuntime::named` | 不变；"每个插件一个进程"就是每个插件一个命名的运行时 |
| 按 `<运行时名>:` 前缀把行送到运行时 | `RuntimeResolver::modules`：Python、Bun、远程、Go/Swift 已经这样用。Node 运行时自己会解析 npm 名与 `file:`（`runner.mjs` 的 `located`，远程 Node 靠的就是它），所以 `modules` 对本机 Node 运行时现在就能用 | **不加新写法**：Node 行写 `<运行时名>:<npm 名或文件>`，用 `RuntimeResolver::modules`；只补文档与测试（§4.1） |
| 进程的结束与重启 | `LocalRuntime::processes().kill()`；`FiberView::restart()` | 超时后结束进程用它；重启由宿主决定，用已有的接口 |
| 进行中的调用 | 0.18 在 `RowService::invoke` 登记，调用方来自 `caller_row()` | 限时的就是这些调用；超时信息里的"谁调用谁"从这里来 |
| 取消传到对面 | 各语言已有（0.18 §6.1 的表） | 超时就是由宿主替调用方取消 |
| 问候时限 | 远程：`Retry::handshake`、`AccessPointConfig::handshake` 已能设；本机：写死 30 秒 | 本机补一个设置，写法与远程相同 |
| 叶子运行时只租借写进 `inject` 的服务 | `RuntimeResolver` 解析时得到插件的 `inject` | 能用的服务在这里多一道检查 |
| `share_registered_only`、读取拦截 | `ServiceCatalog`、`intercept_require_as` | 不用，它们不是权限；指南写明（§七） |

Cordis 没有进程，也没有"只许用这些服务"的名单。它的行上有 `inject`（加依赖）和 `isolate`（让一行看到另一份同名服务）。用 `isolate` 把不许用的名字藏起来，必需的依赖会一直等，不报原因，做不到 issue 要的"被拒绝并说明"，所以另加一个名单；名单设在进程上，因为同一进程里的插件能用语言本身的办法互相拿到，在那里管不住。

## 三、与 0.18"平台不设调用时限"为什么不矛盾

0.18 §二说"平台只看，不替业务做决定"，所以 rutis 不设、也不默认任何调用时限。本稿不改这一条：数值由宿主持有者写，不写时与现在、与 Cordis 相同。0.18 §6.1 末尾已经写明这件事归 #683（宿主的选项，默认关）。调用方自己限时做不到的两件事是本稿要补的：调用方不是宿主时（插件 A 调插件 B），以及卡在同步代码里的插件（取消传过去也停不下，只能结束进程）。两者同时存在时，先到的那个生效。

## 四、决策

| # | 决策 | 章节 |
| --- | --- | --- |
| H1 | 每个插件（或每组插件）一个进程 = 一个命名的 `LocalRuntime`；它的行用已有的 `RuntimeResolver::modules`，写 `<运行时名>:<npm 名或文件>`。不加新接口 | 4.1 |
| H2 | 调用时限：`LocalRuntime::call_limit(Duration)`，进入这个运行时里插件代码的调用超过它就取消并报 `timeout`；`kill_on_timeout()` 再结束进程 | 4.2 |
| H3 | 能用的服务：`LocalRuntime::allowed_services([..])`，只限来自运行时之外的服务；必需的依赖不在名单里时这一行启动失败，其他调用报已有的 `unavailable` | 4.3 |
| H4 | 问候时限：`LocalRuntime::greeting(Duration)`，与远程的 `Retry::handshake` 同样用 `Duration::MAX` 表示不限 | 4.4 |
| H5 | 都只在 `LocalRuntime` 上，默认关，不改已有行为；整个宿主的设置就是给每个运行时设同一个值，不另加接口 | 六 |

### 4.1 每个插件一个进程

```rust
for plugin in &plugins {                      // 每个第三方插件一个进程
    let runtime = LocalRuntime::node(&runtime_pkg, &plugin.dir)
        .named(&plugin.id)                    // 运行时名：两个以上的 a-z、0-9、-
        .call_limit(Duration::from_secs(30))
        .kill_on_timeout()
        .allowed_services(["tools", "notes"]);
    let rows = Arc::new(RuntimeResolver::modules(runtime.handle()).with_catalog(&catalog));
    root.plugin(runtime);
    chain = chain.with_shared(rows.clone());
    resolvers.push(rows);                     // 加载器之后各挂一个 RuntimeRowsPlugin，与现在相同
}
// 行：{ "id": "wordcount", "name": "wordcount:wordcount-plugin" }
```

- 一组插件共用一个进程：几行写同一个前缀。
- `RuntimeResolver::node` 不变，仍是默认 Node 运行时的写法（不带前缀）；它本来就不接受带运行时前缀的名字，不会抢走别的运行时的行。
- 代价：`modules` 每次解析都问运行时（与 Python、Bun 相同），不按包的 `version` 缓存。
- 要做的只是：`RuntimeResolver::modules` 与 rutis-loader README 的文档写明它也用于 Node（现在只写了 Python），加测试。

### 4.2 调用时限

- **管哪些调用**：这一侧进入这个运行时里插件代码的调用——按名字调用服务（`RowService::invoke`）和按引用传过来的函数（deskmate 的工具处理函数）。调用方是宿主、别的运行时的插件还是 Rust 插件都一样。从发出到结束计时。
- **不管的**：同一运行时里插件之间的调用（不经过宿主）；清理（关闭的时限归宿主，0.18 §6.2）。
- **超时以后**：调用方收到错误码 `timeout` 的错误；同时向对面取消这个调用（各语言已有的取消）。
- **`kill_on_timeout()`**：再结束这个运行时的进程（`processes().kill()`）。同一进程里其他进行中的调用报 0.18 已有的 `peer-gone`。要不要重启由宿主决定：对 `root.plugin(runtime)` 返回的 fiber 调用已有的 `FiberView::restart()`。为什么要它：TypeScript 的 promise 取消不了，Python 的 `def` 方法和 JS 的忙循环占住整个进程，取消传过去也停不下；而插件调插件时宿主不在调用链上，没法自己去结束。

### 4.3 能用的服务

- `allowed_services` 列出这个运行时的插件能用的、**来自运行时之外**的服务名（宿主的服务、别的运行时的行提供的服务）。同一运行时里的行互相提供的服务不受限制。
- 检查在加载器解析这一行时做，拿的是插件声明的 `inject`：
  - 必需的依赖不在名单里：这一行启动失败并写明原因，不让它一直等。
  - 可选的依赖不在名单里：不租借进进程，插件看到它不存在；仍去调用时报已有的 `unavailable`，`reason` 写明是宿主不让用。
- 只管插件，不管宿主自己的调用（`call_service`）。
- #185 的 `loader` 服务：宿主不想让某些插件用，就不把它列进那些运行时的名单。默认不限，与 Cordis 一致。

### 4.4 问候时限

- `LocalRuntime::greeting(Duration)`；不设时 30 秒，与现在相同；`Duration::MAX` 不限，调试器停在第一行时用。远程已有的 `Retry::handshake`、`AccessPointConfig::handshake` 不变。
- 不限时，一个启动了却一直不说话的进程会一直停在"启动中"；0.18 的 `Loader::status()` 里看得到它的行在等哪个运行时。

## 五、开发者看到什么

| # | 输入 | 输出 |
| --- | --- | --- |
| 1 | `wordcount` 的处理函数 30 秒没返回 | 宿主调用方：`a function of wordcount did not finish within 30s (the host's limit for runtime wordcount); the call was cancelled`；按名字调用时写 `chat: call to weather.tomorrow did not finish within 30s (the host's limit for runtime agent); the call was cancelled`。错误码 `timeout`，`data` 为 `{runtime, limit, service?, method?}` |
| 2 | 同上，设了 `kill_on_timeout()` | 上一条结尾加 `; runtime wordcount was ended`；运行时的状态是 `down (ended by the host: a function of wordcount did not finish within 30s)` |
| 3 | `wordcount` 必需的依赖里有 `llm`，名单是 `tools`、`notes` | `wordcount: uses llm, which the host does not let runtime wordcount use (allowed: tools, notes)`；这一行失败 |
| 4 | 可选的依赖 `llm` 不在名单里，插件仍调用 | 插件代码里的错误：`llm is not available to wordcount: the host does not let runtime wordcount use it`，错误码 `unavailable` |
| 5 | `greeting(Duration::MAX)`，运行时停在 `--inspect-brk` | `Loader::status()`：`runtime wordcount: starting`，它的行 `waiting for runtime wordcount` |

插件作者不写任何新东西：他的代码看到的只是普通的错误（名字、消息、错误码），与别的出错一样处理。

## 六、兼容

- 全部是新增的、默认关的 `LocalRuntime` 方法：不设时行为、输出与 0.18 相同，不进《迁移到 0.19》。
- 错误码表（0.18 §4.3）加一行，不改已有的行：

| code | 什么时候 | `data` |
| --- | --- | --- |
| `timeout` | 进入运行时的调用超过宿主设的时限 | `{runtime, limit, service?, method?}` |

- 协议不变：取消、进程结束都是已有的消息；运行时一侧不用升级。

## 七、指南：「管住插件」

按 #671 的安排写进 `docs/guide/open-host.md` 第 3 节（两份）；rust-host.md 的参考表加 `call_limit`、`kill_on_timeout`、`allowed_services`、`greeting` 四行，并在 `RuntimeResolver::modules` 一行写明 Node 也用它。第 3 节的内容：

1. 每个插件一个进程：§4.1 的循环，行写 `<运行时名>:<包名>`。
2. 调用时限与 `kill_on_timeout`，以及结束后怎样用 `FiberView::restart()` 重启。
3. 能用的服务。
4. 问候时限：调试器停在第一行时设 `Duration::MAX`。
5. 不是限制的东西，各一句：`share_registered_only` 与读取拦截；同一进程里插件能互相拿到；插件进程有宿主用户的全部权限。

插件 API 指南的"同一运行时里的插件共用一个进程"一节加一句：宿主可以把插件放进单独的进程并设时限，插件收到的是 `timeout`、`unavailable` 错误。

## 八、实施

一个 issue（#683），分三步：

| 步 | 内容 | 完成条件 |
| --- | --- | --- |
| 1 rutis-bridge | `LocalRuntime::call_limit`、`kill_on_timeout`、`greeting`；在按名字与按引用进入运行时的两处计时、取消、按设置结束进程；`timeout` 码 | Node、Python 各一个测试：异步方法超时被取消、同步占住时 `kill_on_timeout` 结束进程且别的运行时不受影响、按引用的函数超时；`greeting` 设为 1 秒与 `Duration::MAX` 各一个测试 |
| 2 rutis-loader | 解析时检查 `allowed_services`（必需的失败、可选的不租借）；`RuntimeResolver::modules` 用于 Node 的文档 | 两个 Node 运行时用 `modules` 各自的行进各自的进程，其中一个崩溃另一个照常；名单的三种情况（允许、必需不允许、可选不允许后调用）；同一运行时里的行互相提供不受限；§五 每一条有逐字断言 |
| 3 文档 | open-host.md 第 3 节（#671）、rust-host.md 参考表（§七）、`docs/guarantees.md` 登记 `host/plugin-limits`（每条写明验证它的测试） | 文档链接检查、doc 例子检查通过 |

对应 issue 的「完成」：步 1–2 是"选项与测试"，步 3 是"指南一节"（issue 原文写在 rust-host.md，按 #671 改为 open-host.md 第 3 节）。
