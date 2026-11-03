# 0.21 的测试工具：出错时说清原因

[English](design-test-tool-errors-2026-11-03.en.md)

状态：定稿，未实现。日期：2026-11-03。基准：`main` `d2a8d069`（0.20.1 之后）。排期：0.21。
Issue：[#933](https://github.com/arcships/rutis/issues/933)（属于 [#663](https://github.com/arcships/rutis/issues/663) 的 A5、D1、D2）。按本稿实施：#933 的 T-1 到 T-6（§7）。
依据：[诊断](design-diagnostics-2026-10-26.md)（错误码 §4.3、只用开发者写过的东西说话 §4.5、`status` §五、故障表 §8.1）；[出错信息](https://github.com/arcships/rutis/issues/937)（#937 的设计稿，评审中，合并后在 `docs/design-error-messages-2026-11-03.md`：`dev`、`status`、`diagnostics()` 的等待说法与状态词归它，本稿只转述）；[配置 Schema](design-config-schema-2026-10-27.md) §5.2（测试工具抛的错误带 `bad-config`）；[插件 API](guide/plugin-api.md)「测试工具」「测试工具与真实运行时的差异」；[rutis-host](guide/rutis-host.md)「run 打印什么」（`dev` 的行状态表，本稿说的"`dev` 的话"就是这张表）。

范围外：

- Swift 与 Kotlin 的测试工具。它们经同一个 `rutis-host test`，§3 给宿主加的方法与文字对它们同样成立；接上是 Swift、Kotlin 各自的 issue，完成时文字与 §4 逐字相同。
- 测试工具读整份 `status` 快照（诊断设计「范围外」，#689 原文列过它）。
- 远程运行时、节点、设备上的插件。
- 插件自己抛出的错误怎样跨语言、怎样打印（诊断设计 §4.4、#675、#687 已定）。
- Rust 里由 `rutis-loader` 按 `rutis.json` 装的行：那里行的名字、位置由 loader 给，与 `dev` 相同。

## 一、现在怎样

### 1.1 issue 的原话

| 情形 | 测试里看到的 | `dev` / `check` 里看到的 |
| --- | --- | --- |
| inject 一个没人提供的服务 | TS：`the plugin does not provide weather (now)`；Go：`timed out (10s): the service weather`；Rust：`(&plugin).await` 返回 `Ok`，接着 `root.get::<Weather>()` panic | `waiting for lmm (no row provides it; did you mean llm?)` |
| 两个插件互相 inject | Go、TS：只有超时 | `they inject each other`，check 让你 `break the cycle` |
| 服务调用不返回 | TS：`node:test` 超时后测试进程不退出（CI 会挂住）；Python：测试一直挂着，没有任何输出 | `status` 列出在途调用 |
| `apply` 出错 | Python：`PluginTestError: plugin: KeyError: 'town'`，没有堆栈；TS：没有源码位置 | 带堆栈、指到 `apply` 那一行 |
| 行名 | Python 是 `plugin`，TS 是文件名 `index`，Rust 是类型名 `WeatherPlugin`；指南例子写的是插件名（`greeter: config: …`） | 插件名 |

另外：TS 里一个测试在 `unload` 之前失败，它给的假服务留在共用的测试宿主上，下一个测试报无关的 `llm is already provided by rutis-test`（`node/rutis/src/testing.mjs`）；指南的测试例子没有 `try/finally`。

### 1.2 已经修好的两条

- **0.19.1（#943）**：TypeScript 的调用不返回时，`node:test` 的时限到了之后测试进程能退出（#997 接着修了"时限落在 `load`、落在宿主启动里"的两种）。
- **0.19.2（#971）**：TS、Bun 的测试结束时自动卸载，失败的那个测试不再把服务留给下一个；Python 的异步调用有 30 秒时限。

本稿不重做这两条，只把它们的文字和其余语言拉齐（§3.4、§4）。

### 1.3 现在每种工具的做法

`dev` 的话在 `crates/rutis-loader/src/loader/status.rs`（`describe_waits`、`why`）与 `crates/rutis-host/src/status.rs`（`describe`、`prefixed`）里；`apply` 的位置在 `crates/rutis-bridge/src/report.rs`（`headline`、`whereabouts`）。三种工具都不经过这两处：宿主 `rutis-host test` 用 `LoaderError` 里已经拍平成字符串的 `Failure`（`crates/rutis-host/src/test.rs:414`），插件没起来时又什么都不问。

| 工具 | 插件一直在等 | 两个插件互相 inject | 调用不返回 | `apply` 出错 | 行名 |
| --- | --- | --- | --- | --- | --- |
| TypeScript、Bun | `load` 照常返回；`t.service('weather')` 抛 `the plugin does not provide weather (now)`（`node/rutis/src/testing.mjs:359`） | 同上：只有这一句，看不出是两个插件 | `node:test` 的 `timeout`（§1.2）；工具不记在途的调用 | `index: Error: boom`，没有位置 | 文件名去掉扩展名：`src/index.ts` → `index`（`crates/rutis-host/src/test.rs:393` 的 `base_id`） |
| Python | `load` 抛 `weather did not start: it injects llm, which nothing provides: give the test services=`（`python/rutis/rutis/testing.py:518`）：工具自己猜的，不是 `dev` 的话 | `a did not start: it injects bsvc, which nothing provides: give the test services=`：`b` 其实会提供它，只是 `b` 也还没起来 | 30 秒后 `PluginTestError: weather.today() did not return within 30 s`（`testing.py:398`） | `plugin: KeyError: 'town'`，没有堆栈 | 定义里声明的名字，没有时 `plugin`（`testing.py:492`） |
| Go | `Load` 照常返回；`loaded.Service("weather")` 10 秒后 `timed out (10s): the service weather`（`rutistest.go:320`、`internal/testwait/testwait.go:17`） | 同上：只有这一句 | 调用方给的 `ctx`；不给就等到 `go test` 自己的时限 | `rutistest: weather: Error: boom`，panic 没有位置 | `Name`（`rutistest.go:608`） |
| Rust | `(&plugin).await` 返回 `Ok`（`crates/rutis/src/fiber.rs:1714` 写明了），`root.get::<Greeter>()` 是 `None`，`.unwrap()` panic | 同上 | 测试自己的 `tokio::time::timeout` | `?` 交回 `Error`，`cargo test` 打印它的 `Debug`：`Error: PluginFailed(…)` | `Plugin::name()`：类型名的最后一段，`GreeterPlugin`（`crates/rutis/src/plugin.rs:57`） |

## 二、决策

| # | 决策 | 章节 |
| --- | --- | --- |
| T1 | **一行的话只有一处写**：宿主 `rutis-host test` 用 `dev` 的同一段代码算出这一行的状态行（`describe` + `describe_waits` + `prefixed`），SDK 原样打印，不改一个字。那句话的用词与错误码归 #937 与诊断设计，本稿只转述（`state(id)` 是宿主本地的，协议、`rutis.json`、插件 API 都不变） | 3.1、3.3 |
| T2 | **行名是插件的名字**（包名 / 模块名 / `Name`），与各语言指南里的例子一致；测试给 `id` 时用给的 | 3.2 |
| T3 | **插件没起来时说这一行在等什么**：`t.service()`、`loaded.Service()` 不再回"没有这个服务"，而是这一行的状态行；另有 `waiting()` 让人主动看。`load` 照旧返回（测试可以随后加载提供者，让这一行起来） | 3.3 |
| T4 | **`apply` 出错带位置**：loader 的 `Failure` 不再用 `Display`，改用 `report()`（同一句 + 位置行）；`dev` 的 `cannot reload`、`rutis/loader`、测试工具因此说同一句话 | 3.1、6 |
| T5 | **调用不返回按工具自己的时限失败**：TypeScript、Bun、Python、Go 都是 30 秒；Go 调用方自己给了 `ctx` 的时限时用调用方的；到时那次调用失败、测试进程能退出 | 3.4 |
| T6 | **工具自己的失败有错误码**（`not-running`、`no-service`、`timeout`），宿主拒绝装载时带插件错误的码（`bad-config` 等）。三种语言今天都只有文字、没有码，`code`/`data` 是本稿加的 | 3.5 |
| T7 | **一个测试结束时什么都不留给下一个**：TS、Bun 已有（§1.2）；Python 的 `async with`、Go 的 `t.Cleanup` 照旧。本稿加的是"宿主拒绝装载的行走了一半"的路径 | 3.6 |
| T8 | **Rust 没有工具**：给 `FiberView` 加一个"等到它跑起来"的方法（`running(limit)`），失败时说的就是 `diagnostics()` 对这一个插件说的那一句（用词归 #937 的 E-2），所以四种语言说的是同一句，只有名字与"它要的东西"是各自的语言 | 3.7 |

放弃的做法：

- **让 `load` 在插件一直在等时失败**：测试可以先装载一行、再装载它用的提供者，让它起来（`node/rutis/test/testing-plugins.test.mjs` 里 "waits, and starts when it arrives" 就是这么测的）。失败改在"用这一行"的地方，也在 `waiting()` 里。
- **测试结束时因为"这一行没起来"让测试失败**：故意装载一行、断言它在等的测试会因此失败；而且 TS 的 `unload` 也在测试已经结束的钩子里跑，在那里抛错只会把报告弄乱。
- **每种语言各自写一句**（例如 Python 现在那句 `did not start: it injects llm…`）：四份文字迟早不一致，而且猜不准：它看的是这个运行时里有没有人提供，不是这一行为什么没起来（互相 inject 时说 `bsvc` 没人提供，其实 `b` 会提供）。
- **给测试工具一份完整的 `status` 快照**：诊断设计把它列在范围外；工具只需要这一行。

## 三、规则

### 3.1 宿主写这一行，SDK 原样打印

这一行（`<行 id>: <状态>`，等什么、失败在哪一步，后面跟着位置行）不是测试工具写的：宿主 `rutis-host test` 用 `dev` 的同一段代码算出来，SDK 一个字不改地放进错误里。

| 情形 | 宿主用的函数 |
| --- | --- |
| 行在跑 | `describe`（`crates/rutis-host/src/status.rs:18`） |
| 行在等 | `describe_waits`（`crates/rutis-loader/src/loader/status.rs:411`，`Loader::waits` / `waits_of`） |
| 行失败 | `describe` → `report()`（`crates/rutis-bridge/src/report.rs:172`）：同一句加位置行 |
| 行名与位置行的缩进 | `prefixed`（`crates/rutis-host/src/status.rs:297`） |

**这些句子、它们的用词与错误码归[出错信息](https://github.com/arcships/rutis/issues/937)（#937 的设计稿，评审中）与[诊断](design-diagnostics-2026-10-26.md) §4.3、§4.5。** 本稿不写、也不改其中任何一句：`status` 与 `dev` 的等待说法、`failed in apply:`、位置行、状态词都由那边定（#937 的 E-2 把"没人提供"改说 `nothing provides it yet`，测试工具里就是那句）。本稿只加三件事：

1. 宿主给测试用的一个方法（下面）；
2. 工具自己产生的两种失败的说法与码（`no-service`、`timeout`，§3.3、§3.4），以及宿主那一行不跑时用的码 `not-running`；
3. 每种语言怎样把码交出来（§3.5）。

### 3.1.1 正文是底层错误自己的话

一条消息分两段：**前缀**回答"是我哪一次调用"（这一行、这一步 `apply`、这个服务与方法），**正文逐字是底层错误自己写的字**。不转述、不概括，也不把错误里已经说过的话再说一遍；拿不准时保留原文。

| 插件抛出的 | 测试里看到的（前缀 + 原文） |
| --- | --- |
| `KeyError: 'town'`（Python） | `greeter: failed in apply: KeyError: 'town'`，下面 `in greeter:` 与 `File "/…/greeter/plugin.py", line 42, in apply` |
| `new Error('boom')`（Node） | `greeter: failed in apply: Error: boom`，下面 `in greeter:` 与 `at apply (/…/src/index.ts:42:11)` |
| serde 的 ``invalid type: integer `42`, expected a string`` | ``greeter: failed in apply: invalid type: integer `42`, expected a string`` |
| Go 的 `json: unsupported type: chan int` | `greeter: failed in apply: json: unsupported type: chan int` |

`failed in apply: `、`in greeter:`、位置行都是 rutis 加的，其余一个字不动。这条规则对本稿只有一个落点：`state(id)` 回来的 `line` 与 `load` 的失败里，前缀之后的部分必须与插件抛出来的字完全相同（§4 的表逐字写着这一点，T-1 的用例逐字比较）——本稿不改写它，实现时也不许"顺手说清楚一点"。

只有 **rutis 自己编出来的错**才换成平实的话：等待原因（`nothing provides it yet`）、失败在哪一步、配置不合格（`config: greeting should be a string, got 3`），以及本稿的三句（`no such service …`、`did not return within 30s`、`not running (…)`）。它们没有"底层的原话"可保留：它们是 rutis 的判断，不是插件说的话。（诊断设计 §4.5 管的是这一套词；上面这条管的是别改写插件自己的话。）

`rutis-host/test` 是 `rutis-host test` 的服务（`crates/rutis-host/src/test.rs` 的 `Harness`），只在测试进程与它起的那个宿主之间用。它多一个方法：

```text
state(id) -> null | { line, code?, data? }
```

- `null`：这一行在跑。否则 `line` 是上面那一行（宿主写给 `dev` 的那个字），`code`、`data` 是这一行错误的码与细节（插件自己的错误没有码时省略）。
- **只读**：它在宿主里读一份快照（`describe` / `describe_waits`），不 await 任何东西，也不改变任何东西。
- **谁在什么时候问它**：对 SDK 来说它是一次往返（TS、Python 的 `waiting()` 用 `await`），所以**两个同步的 `t.service()` 不在里面做往返**：`load`（`Load`）在返回前问一次，`waiting()` 每次问，`t.service()` 用这两处最近拿到的那一行（§3.3）。Go 的 `Service()` 本来就把对宿主的调用当作阻塞调用（它装载时已是这样），所以它直接问。
- **只在这个宿主里**，不碰三样东西：会话协议（版本 4，`ENDPOINT_PROTOCOL`，`crates/rutis-bridge/src/session/mod.rs:14`）、`rutis.json` 的格式（1，`FORMAT`，`crates/rutis-host/src/config.rs:78`）、插件 API（3，`node/rutis/src/advanced.mjs:15`、`python/rutis/rutis/plugin.py:125`、`go/rutis/internal/runtime/version.go:9`）。
- `load`、`update`、`unload` 的失败也换成同样的 `{ line, code?, data? }`（Display 就是 `line`）。

新旧相遇：

| 情形 | 看到什么 |
| --- | --- |
| 0.20 的测试工具 + 0.21 的宿主 | 不变：旧工具不问 `state`，多出来的方法它看不到 |
| 0.21 的测试工具 + 0.20 的宿主（TS、Bun 还有各自 0.20 的运行时包 `@arcships/rutis-runtime`、`@arcships/rutis-bun`：它们的测试驱动没有 `state`） | 这一行不跑时 `greeter: not running (this rutis-host is older than 0.21 and cannot say why; upgrade it)`，码 `not-running`；`waiting()` 回 `null`。指南写明把 `rutis-host`（TS、Bun 还有各自的运行时包）升到同一版本 |

### 3.2 行名

测试里这一行的 id 是**插件的名字**，与指南的例子一致（`greeter: config: …`）：

| 语言 | 顺序 |
| --- | --- |
| TypeScript、Bun | 测试给的 `id` → 插件声明的 `name` → 插件文件最近的 `package.json` 的 `name`（去掉作用域：`@ana/wordcount` → `wordcount`）→ 文件名去掉扩展名 → `plugin` |
| Python | 测试给的 `id` → 插件声明的 `name` → 它的 `apply` 所在模块的包名（`greeter.plugin` → `greeter`）→ `plugin` |
| Go | `Row(key)` → `Name` → 包名 → `plugin` |
| Rust | `Plugin::name()`：类型名的最后一段；插件自己写 `fn name` 时用它的 |

重名时照旧加 `-2`、`-3`……（`crates/rutis-host/src/test.rs:382` 的 `free_id`）。同一个包里两个插件都叫包名时第二个是 `<包名>-2`：一个测试要装两个时给 `id`（§4.1 写了这一句）。

宿主那边的 `base_id`（文件的最后一段、去掉扩展名）留在最后兜底，其他语言传不上名字时用它。

Rust 不一样：它没有 `rutis.json` 的行，内核只知道插件的名字，所以行名就是 `Plugin::name()`。

### 3.3 插件没起来时

- `t.service(name)`、`t.methods(name)`（TS、Bun、Python）、`loaded.Service(name)`（Go）在这一行**不在跑**时，失败的文字是这一行的状态行：`greeter: waiting for translate (nothing provides it yet)`、`a: waiting for bsvc (row b waits for asvc too: they inject each other)`、`greeter: failed in apply: …`。码 `not-running`，`data: {row}`。
  - 括号里的原因是 #937 §3.3 的词（`nothing provides it yet`、`row b waits for asvc too: they inject each other`），本稿只是转述：宿主改说法，这里跟着改。
- 这一行**压根没起来过**（0.20 的宿主）：`greeter: not running (this rutis-host is older than 0.21 and cannot say why; upgrade it)`，码同样是 `not-running`（§3.1）。
- 这一行在跑、但没有这个服务名时：`greeter: no such service forcast (it provides: forecast, today; did you mean forecast?)`；它什么都不提供时 `(it provides nothing)`。码 `no-service`，`data: {row, service, services}`。
- `await t.waiting()`（TS、Bun、Python）、`loaded.Waiting()`（Go）：在跑时是 `null` / `""`，否则是同一行。每次调用都问宿主，所以后加载的提供者让它起来之后立刻是 `null`。
- `t.service()`（TS、Bun、Python）是同步的，问不了宿主：它先用运行时里的实情（TS 的 `provided`、Python 的 `runtime.slots`）判断这一行在不在跑，不在跑时用 `load` 与 `waiting()` 最近一次拿到的那一行说原因。行跑起来之后它照常交出服务，那一行用不上。
- `load`、`Load` 照旧返回：这一行在等不算装载失败。`apply` 真出错、配置不合格这两件事仍然是装载失败，文字换成这一行（§3.1）。
- Python 现在那句 `weather did not start: it injects llm, which nothing provides: give the test services=` 删掉：它说的是这个运行时里有没有人提供，不是这一行为的什么没起来（互相 inject 时它说 `bsvc` 没人提供，而 `b` 会提供它），而且与 `dev` 不同。

### 3.4 调用不返回

| 语言 | 时限 | 到时的失败 |
| --- | --- | --- |
| TypeScript、Bun | 30 秒，工具的（异步方法的调用；`node:test` 自己的 `timeout` 更短时用它的） | `PluginTestError: greeter.hello() did not return within 30s` |
| Python | 30 秒（已有，文字从 `30 s` 改成 `30s`） | `PluginTestError: greeter.hello() did not return within 30s` |
| Go | 调用方 `ctx` 没有时限时 30 秒；有时限时用调用方的 | `rutistest: greeter.hello() did not return within 30s`（调用方自己的时限写它自己的秒数） |
| Rust | 测试自己的 `tokio::time::timeout` | 测试自己写 |

- 码 `timeout`，`data: {service, method, limit}`；`limit` 是秒数。
- 到时只让这次调用失败，不结束别的调用，也不重启运行时。
- 进程能退出：TypeScript、Bun 到时把这次调用从"占住进程"的账上划掉（`node/rutis/src/testing.mjs` 的 `hold`），定时器本身不占住进程；Python 取消这次调用（已有）；Go 用带时限的 `ctx`（与插件调用方的写法一样）。
- TypeScript 的同步方法（`provides` 里写 `sync` 的）打断不了：JS 的主线程阻塞之后没有地方跑定时器。指南写明这一点，与 `node:test` 的 `timeout` 也打断不了同步代码是同一件事。

### 3.5 错误码

本稿给测试工具加三个码，加进[诊断](design-diagnostics-2026-10-26.md) §4.3 的表（那张表说"实施时可以加行，不改已有的行"；#937 往同一张表加 `already-provided`，两稿各加各的行）。表里的其他行、以及"码不出现在文字里"这条规则归那张表，本稿不改：

| code | 什么时候 | `data` |
| --- | --- | --- |
| `not-running` | 这一行不在跑（在等、失败、被禁用），而测试要用它 | `{row}` |
| `no-service` | 这一行在跑，但它不提供这个服务名 | `{row, service, services}` |
| `timeout` | 一次调用在时限内没有返回（§3.4） | `{service, method, limit}` |

- **今天这三种错误只带名字与文字，`code` 与 `data` 是本稿加的**：

| 语言 | 今天 | 本稿加 |
| --- | --- | --- |
| TypeScript、Bun | `class PluginTestError extends Error { name = 'PluginTestError' }`（`node/rutis/src/testing.mjs:32`），测试只能比文字 | `code`、`data` 两个字段 |
| Python | `class PluginTestError(AssertionError)`，只有一段 docstring（`python/rutis/rutis/testing.py:64`） | `code`、`data` 两个属性 |
| Go | 没有错误类型：`Load` 用 `t.Fatalf`，`TryLoad` 交回 `error`；宿主拒绝装载时那个是 `*rutis.RemoteError`，`Name` 是 `BindingError`，`Code` 已经有值（[配置 Schema](design-config-schema-2026-10-27.md) §5.2 就是这么写的） | `*rutistest.Error{Code, Line, Data}` |
| Rust | 没有工具 | `running` 的错误是 `rutis::Error`，按 `ErrorKind` 判断 |

- 宿主拒绝装载时带的是这一行错误自己的码：配置不合格是 `bad-config`（`data: {issues}`，配置 Schema 设计 §5.2）；插件自己的错误没有码。
- 码不出现在给人看的文字里（诊断设计 §4.3 的规则）。
- 测试按码断言时就不比文字：TypeScript、Bun 的 `assert.equal(error.code, 'not-running')`，Python 的 `self.assertEqual(refused.exception.code, 'not-running')`，Go 的 `errors.As(err, &target)` 之后看 `target.Code`。

### 3.6 一个测试结束时

- 三种工具都不把行、也不把测试给的服务留给下一个测试：TS、Bun 的自动卸载（§1.2），Python 的 `async with` 与 `finally` 里的撤回（`python/rutis/rutis/testing.py:525`），Go 的 `t.Cleanup`（`go/rutis/rutistest/rutistest.go:141`）与 `release`。本稿不改这些做法，只加一个测试把它们锁住。
- 半路失败也撤干净：装载被拒绝时，这一次已经给出去的服务与已经装载的行要撤下再抛错（TS 在 `ask('load', …)` 失败时 `withdrawOwn`，Python 在 `finally` 里，Go 在 `start` 失败时 `release`）。`state`、`waiting()` 不改变任何东西。
- 每个工具有两个测试锁住它：一个测试装载失败的插件（`apply` 抛错），紧接着的测试用同样的服务装载同一个插件，通过；一个测试在 `unload` 之前失败（TS、Bun），下一个测试照常装载。

### 3.7 Rust

Rust 没有测试工具：指南第 6 步在 `#[tokio::test]` 里建根、装插件、断言，没有 `rutis-host test`，也没有 loader 与行。给它加一件东西，`crates/rutis/src/fiber.rs`：

```rust
/// 等到这个插件跑起来：`apply` 做完。失败（`apply` 出错、配置不合格）立刻
/// 回来的 `Err` 说的就是这一行；一直在等时，等到 `limit` 仍没跑起来就回来，
/// 说的是它在等什么。
pub async fn running(&self, limit: Duration) -> Result<(), Error>
```

- 跑起来：`Ok(())`。
- 失败：`Err`，文字是 `<名字>: <diagnostics() 对这一个插件说的那一句>`：`GreeterPlugin: failed in apply: boom`、`GreeterPlugin: config: greeting should be a string, got 3`。
- 一直在等：`Err`，同一句：`GreeterPlugin: waiting for Arc<dyn Translate> (nothing provides it yet)`；多个依赖用 `; ` 连起来。
- 所有没满足的依赖都是 `Missing`（没有插件提供它）时**立刻**回来，不等 `limit`：测试里所有插件都是测试自己装的，这种情况下再等也没有用。
- **这句话的用词不归本稿**：它就是 `RuntimeDiagnostics` 的 `Display` 对这一个插件说的那一句（`crates/rutis/src/diagnostics.rs:65`，在等的那一段在 `:75`），`running` 与它共用一个函数，所以两边不会说法不同。#937 的 E-2 改那里的用词（与 `status` 同一套），`running` 跟着改，本稿不另写。
- 指南第 6 步的例子用它（§4.4），`emit`、`dispose`、`shutdown` 那几行不变。

**为什么 Rust 不算另一套行为。** 同一个输入——插件要一个没人提供的依赖——四种语言说的是同一句 `<名字>: waiting for <它要的东西> (<原因>)`，只有两处用开发者自己写的东西代替：

| | 名字 | 它要的东西 | 原因 |
| --- | --- | --- | --- |
| TypeScript、Bun、Python、Go | 行名：插件的名字（§3.2） | 服务名，例如 `translate` | `nothing provides it yet`（#937 §3.3） |
| Rust | `Plugin::name()`：插件的名字 | 它声明的类型，例如 `Arc<dyn Translate>` | 同一句 |

Rust 里没有"行"这一层：进程是测试自己的，插件跑在一个 fiber 上，没有 `rutis-host test`，没有 `rutis.json` 的行，也没有模块名可以拿来当行名，所以名字只能是插件自己的。它要的东西也只能写类型，因为 `type Deps` 里写的就是类型。换掉的是这两处主语，不是说法。

### 3.8 说不到的地方

- 同一个运行时里的调用、按引用传过来的函数：`dev` 也看不到（诊断设计 §5.2），工具同样只写这一行。
- TypeScript 的同步方法调用打断不了（§3.4）。
- 宿主进程被杀、运行时退出：文字来自宿主的同一处（`runtime py: down (…)`），测试工具不再包一层；测试里只有本机运行时，所以这一条在测试里少见。
- 插件自己抛的错误、跨语言的位置：诊断设计已定，本稿不碰。

## 四、开发者看到什么

只写受影响的角色：写测试的插件作者（四种语言各一）、用 `rutis-host` 组装插件的人（`rutis-host test` 的输出变了，`dev` 的文字也有变化）、部署的人（没有变化）。错误码按 §3.5；码不出现在文字里。

以下每一行都是工具打印的那一行，前面加上测试框架自己的前缀（`go test` 的 `file:line:`、`cargo test` 的 `panicked at …`）；测试逐字比较，位置行只比文件名与方法名（诊断设计 §4.6）。

表里每一条的前缀之后都是插件自己的字，一个字不改（§3.1.1）。"在等"与"失败"那几行是宿主写给 `dev` 的那一行（§3.1），括号里的原因按[出错信息](https://github.com/arcships/rutis/issues/937)（#937）§3.3 现在的稿子写；那份稿子改词，这里跟着改。工具自己写的两句（`no such service …`、`did not return within 30s`）归本稿。

### 4.1 TypeScript 与 Bun

正常：

```ts
// test/index.test.ts
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { load } from '@arcships/rutis/testing'
import plugin from '../src/index.ts'

const translate = { text: async (text: string) => ({ Hello: 'Bonjour', Hi: 'Salut' })[text] ?? text }

test('greets with the translated greeting', async () => {
  const t = await load(plugin, { config: { greeting: 'Hi' }, services: { translate } })
  assert.equal(t.row, 'greeter')
  assert.equal(await t.waiting(), null)
  assert.equal(await t.service('greeter').hello('Ada'), 'Salut, Ada!')
  await t.unload()
})
```

`npm test` 打印 `✔ greets with the translated greeting`（`node --test` 的报告；输出被管道接走时是 TAP 的 `ok 1 - …`），工具不另外打印什么。

| 触发 | 看到的 | 码 |
| --- | --- | --- |
| `services` 少给了 `inject` 的服务 | `PluginTestError: greeter: waiting for translate (nothing provides it yet)` | `not-running`，`data: {row: 'greeter'}` |
| 两个插件互相 inject（`host()` 上） | `PluginTestError: a: waiting for bsvc (row b waits for asvc too: they inject each other)` | `not-running` |
| 提供者失败 | `PluginTestError: chat: waiting for llm (row model failed: Error: no key)` | `not-running` |
| 插件在跑，服务名写错 | `PluginTestError: greeter: no such service greter (it provides: greeter; did you mean greeter?)` | `no-service`，`data: {row: 'greeter', service: 'greter', services: ['greeter']}` |
| `apply` 出错 | `PluginTestError: greeter: failed in apply: Error: boom`，下面 `in greeter:` 与 `at apply (/Users/me/greeter/src/index.ts:42:11)` 等帧 | 无（插件自己的错误） |
| 配置不合格 | `PluginTestError: greeter: config: greeting should be a string, got 3` | `bad-config`，`data: {issues: [{path, message}]}`（[诊断](design-diagnostics-2026-10-26.md) §4.3 的形状） |
| 调用不返回 | `PluginTestError: greeter.hello() did not return within 30s` | `timeout`，`data: {service: 'greeter', method: 'hello', limit: 30}` |
| `await t.waiting()` | 在跑：`null`；在等：`greeter: waiting for translate (nothing provides it yet)` | 无（返回字符串，不抛错） |
| 宿主是 0.20（没有 `state`） | `PluginTestError: greeter: not running (this rutis-host is older than 0.21 and cannot say why; upgrade it)` | `not-running` |

`t.service()` 与 `t.methods()` 是同步方法，照样同步抛 `PluginTestError`：用 `load` 与 `waiting()` 最近看到的那一行说原因，不在这里等一次往返（§3.3）。

`host()` 上装同一个包的两个插件时两个都想叫包名，第二个是 `<包名>-2`：给 `id`（`h.load(plugin, { id: 'shout' })`）。

### 4.2 Python

正常：与现在相同（`async with load(plugin, config={...}, services={...}) as t`），`t.row` 现在是 `greeter`（指南的例子里它的 `apply` 在 `greeter/plugin.py`，包名是 `greeter`）。

| 触发 | 看到的 | 码 |
| --- | --- | --- |
| `services` 少给了服务 | `PluginTestError: greeter: waiting for translate (nothing provides it yet)` | `not-running` |
| 两个插件互相 inject | `PluginTestError: a: waiting for bsvc (row b waits for asvc from this row: they inject each other)` | `not-running` |
| 服务名写错 | `PluginTestError: greeter: no such service greter (it provides: greeter; did you mean greeter?)` | `no-service` |
| `apply` 出错 | `PluginTestError: greeter: failed in apply: KeyError: 'town'`，下面 `in greeter:` 与 `File "/Users/me/greeter/plugin.py", line 42, in apply` | 无 |
| 配置不合格 | `PluginTestError: greeter: config: greeting should be a string, got 3` | `bad-config` |
| 调用不返回 | `PluginTestError: greeter.hello() did not return within 30s` | `timeout` |
| `await t.waiting()` | `None` / 同一行 | 无 |
| 宿主是 0.20（没有 `state`） | `PluginTestError: greeter: not running (this rutis-host is older than 0.21 and cannot say why; upgrade it)` | `not-running` |

`PluginTestError` 仍是 `AssertionError` 的子类，多两个属性 `code`、`data`。

### 4.3 Go

正常：

```go
loaded := rutistest.Load(t, Plugin, Config{Greeting: "Hi"}, map[string]any{"translate": fakeTranslate{}})
if line := loaded.Waiting(); line != "" {
    t.Fatal(line)   // 装上就该跑起来
}
```

`go test ./...` 打印 `ok`。`loaded.Service("greeter")` 的调用不变。

| 触发 | 看到的 | 码 |
| --- | --- | --- |
| `services` 少给了服务 | `--- FAIL: TestHello (0.01s)` / `plugin_test.go:23: rutistest: greeter: waiting for translate (nothing provides it yet)` | `not-running` |
| 两个插件互相 inject | 同上，`a: waiting for bsvc (row b waits for asvc too: they inject each other)` | `not-running` |
| 服务名写错 | `rutistest: greeter: no such service greter (it provides: greeter; did you mean greeter?)` | `no-service` |
| `apply` 出错 | `rutistest: greeter: failed in apply: Error: boom`，下面 `in greeter:` 与 `at Apply (/Users/me/greeter/plugin.go:31)` | 无 |
| panic 的 `apply` | `rutistest: greeter: failed in apply: Panic: panic: boom`，下面是 `goroutine …` 的完整堆栈（与现在服务方法的 panic 一样） | 无 |
| 配置不合格 | `rutistest: greeter: config: greeting should be a string, got 3`（`TryLoad` 交回同样的错误） | `bad-config` |
| 调用不返回 | `rutistest: greeter.hello() did not return within 30s`；调用方给了 `ctx` 的时限时写它的秒数 | `timeout` |

`rutistest.Error`：字段 `Code`、`Line`、`Data`；`Loaded.Service`、`Load`、`TryLoad`、`Update` 都交出它或宿主的 `*rutis.RemoteError`。

### 4.4 Rust

正常（指南第 6 步）：

```rust
let plugin = root.plugin(GreeterPlugin::new(&Config { greeting: "Hi".into() })?);
plugin.running(Duration::from_secs(30)).await.unwrap_or_else(|error| panic!("{error}"));

let greeter = root.get::<Greeter>().unwrap();
assert_eq!(greeter.hello("Ada")?, "Salut, Ada!");
```

`cargo test` 打印 `test tests::greets_with_the_configured_greeting ... ok`。

| 触发 | 看到的 |
| --- | --- |
| 依赖没有插件提供 | `thread 'tests::greets…' panicked at src/lib.rs:…: GreeterPlugin: waiting for Arc<dyn Translate> (nothing provides it yet)`（立刻，不等 30 秒） |
| `apply` 出错 | `GreeterPlugin: failed in apply: boom` |
| 配置不合格 | `GreeterPlugin: config: greeting should be a string, got 3` |
| 调用不返回 | 测试自己的 `tokio::time::timeout`：指南写明怎么写，工具不代替 |

别的等待原因（提供者失败、在等它自己的依赖）的括号里是 `diagnostics()` 对那一个插件的说法，词由 #937 的 E-2 定，本稿不重复。

Rust 的行名就是 `Plugin::name()`：上例是 `GreeterPlugin`。指南在这一步写一句：要它叫 `greeter`（与包名一致），在 `impl Plugin` 里写 `fn name(&self) -> &str { "greeter" }`。

### 4.5 用 rutis-host 组装插件的人、宿主持有者、部署的人

- **用 `rutis-host` 组装插件的人**：`rutis-host test` 的输出变了（这是它给测试工具的服务）；`dev` 里改代码后 `apply` 出错的那一行变成多行（`weather: cannot reload: failed in apply: …` 加位置行，`; the previous version keeps running` 照旧）；`check` 只做 dry run、不跑 `apply`，它的文字不变。
- **宿主持有者**（嵌入 rutis 的应用）：文字不变；`Failure` 的 `error` 字段多了位置行（§6），按文字匹配的代码要改。
- **部署的人**：看不到变化。

### 4.6 要懂的概念

对照 #663 写下的概念预算（写插件 7 个、调试时合理的 9 个）：这份设计不让插件作者多懂概念。`waiting()` 与 `running(limit)` 是"行的状态"这一个概念的新入口，作者本来就要懂它（`dev` 的 `waiting for …`）；错误码是给程序的，不是新的概念。测试里"行"这个概念只多一句：行名是插件的名字。

## 五、用例放在哪里

**没有共用的期望文件，也不新开 `specs/` 目录**：与配置 Schema 的 [`specs/config-schema/cases.json`](../specs/config-schema/cases.json) 不同，这里的每一条不是一份数据，而是"一个插件加一次测试里的动作"，写成跨语言的文件反而要为三种语言各写一段插件代码。用例放在各工具已有的测试里：

- **这一行的字**只有一处写（§3.1），锁在 `crates/rutis-host` 的测试里：一条用例跑 `dev` 与 `state`，比同一行（T-1 的完成条件）。它的词随 #937 的 E-2 改。
- **工具原样打印**每种工具一条：一个插件要一个没人提供的服务，断言 `t.service()` / `loaded.Service()` 抛出的文字与宿主给的一字不差。
- **工具自己写的两句**（`no such service …`、`did not return within 30s`）每种语言一条，期望的文字就是 §3.3、§3.4 里写下的那两句；三处逐字相同，评审时一起看。
- Rust 由 `crates/rutis` 里 `running` 的测试锁住（§3.7）。

## 六、兼容与迁移

写进《迁移到 0.21》（两份）：

- **行名变了**：TypeScript、Bun 的测试里行 id 从文件名的头一段（`index`）变成包名（`greeter`）；Python 从 `plugin` 变成包名。断言 `t.row`、按行名匹配 `reported()` 的测试要改。给 `id` 的测试不受影响。
- **Python 的 `load` 不再为"这一行在等"抛错**：`weather did not start: it injects llm, which nothing provides: give the test services=` 没有了；同一件事现在在 `t.service()` 上以这一行的状态行（`greeter: waiting for translate (nothing provides it yet)`）出现，还有 `await t.waiting()`。
- **几处文字变了**（按文字断言的测试要改）：`the plugin does not provide X (now)` → 这一行的状态行或 `no such service …`；`timed out (10s): the service X` → 同上；Python 的 `did not return within 30 s` → `30s`；`plugin: KeyError: 'town'` → `greeter: failed in apply: KeyError: 'town'` 加位置行——变的只是前缀（行名、哪一步），`KeyError: 'town'` 一个字没动（§3.1.1）。
- **`Failure` 的 `error` 带位置**：Rust 宿主读 `Failure.error` 并当作一行用的地方要改成多行（`error.lines()`）；`dev` 的 `cannot reload` 那一行随之变成多行。`check` 不变（它不跑 `apply`）。
- **`DependencyStatus` / `RuntimeDiagnostics` 的用词**：不归本稿，归 [#937](https://github.com/arcships/rutis/issues/937) 的 E-2（`service missing` 这类内部说法换掉）；`running` 只是跟着它走。
- **新增**：`rutis-host/test` 的 `state`（宿主本地，§3.1）；`FiberView::running`；`PluginTestError.code` / `.data`（今天没有，§3.5）；`rutistest.Error`。插件 API（3）、`rutis.json` 格式（1）、会话协议（4）都不变。
- 新旧相遇：0.20 的测试工具在 0.21 的宿主上不变；0.21 的工具在 0.20 的宿主上见到 `greeter: not running (this rutis-host is older than 0.21 and cannot say why; upgrade it)`（§3.1）。

## 七、实施

| Issue | 内容 | 完成条件 |
| --- | --- | --- |
| T-1 | `rutis-host` 与 `rutis-loader`：`rutis-host/test` 的 `state(id)` 与带 `code`/`data` 的错误；`Failure` 的位置（`report()` 代替 `Display`）；`describe`/`describe_waits`/`prefixed` 复用；Go 的 `Apply` panic 带位置（Go 运行时） | 宿主返回的行与 `dev` 逐字相同（一个测试同时跑 `dev` 与 `state`，比同一行）；`crates/rutis-host` 与 `crates/rutis-loader` 里按文字断言的测试都按新文字更新并写明这是用户可见的变化 |
| T-2 | TypeScript、Bun：行名（包名）；`waiting()`；`service()`/`methods()` 的行与码；30 秒调用时限与 `hold` 的放行；拒绝装载时撤下这一批 | 指南第 6 步的例子照它跑出的输出通过；`node/rutis/test` 与 `bun/rutis-bun/test` 里各一条：抛出的文字与宿主给的一字不差、`code` 是 `not-running`；一个测试失败后下一个测试用同样的服务装载同一个插件通过；0.20 宿主那条（§3.1）一条 |
| T-3 | Python：行名（包名）；`waiting()`；`service()`/`methods()` 的行与码；`30 s` → `30s`；删掉 `did not start` | 同上（`python/rutis/tests/test_testing.py` 加这三条） |
| T-4 | Go：`Waiting()`；`Service()` 的行与码；没有时限的 `ctx` 的 30 秒；`rutistest.Error` | 同上（`go/rutis/rutistest` 的测试加这三条） |
| T-5 | Rust：`FiberView::running(limit)`，与 `RuntimeDiagnostics` 共用同一个句子；指南第 6 步的例子。**排在 #937 的 E-2 之后**：那一步定 `diagnostics()` 的用词 | `crates/rutis` 里一个测试锁住四种文字（在等、`apply` 失败、配置不合格、立刻失败）；指南的例子照它跑出的输出通过 |
| T-6 | 文档（两份）：[插件 API](guide/plugin-api.md) 的「测试工具」与「测试工具与真实运行时的差异」、四份语言指南的第 6 步、[rutis-host](guide/rutis-host.md) 的 `test` 与「run 打印什么」（`dev` 的说法不变，补一句测试工具用同一句）、《迁移到 0.21》 | 四份指南第 6 步的例子与输出一致；两份语言同步 |

T-1 先合并，T-2 到 T-4 各一个 PR 按顺序；T-5 排在 #937 的 E-2 之后，可与 T-2 到 T-4 并行；T-6 随最后一份实现。

#933 在全部满足后关闭：§4 的每一行在四种语言里都有逐字比较的测试；四份指南第 6 步的例子与输出一致。
