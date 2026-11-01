# 迁移到 0.20

[English](migration-0.20.en.md)

0.20 还没发布，这份指南随不兼容的改动一起写：每合并一件加一节，发布前按 [#694](https://github.com/arcships/rutis/issues/694) 补齐。下面只写与 0.19 相比的变化，删掉的写法直接删，不标弃用。

不兼容的改动按类列在这里，每类的改法见对应一节：

- **卸载顺序**：清理与服务撤回在同一个栈里逆序一条一条跑（[卸载顺序](#卸载顺序1010)）。
- **`use` 先查声明**：插件和行都没声明、靠同一进程里别的行声明才拿到的服务，现在报 `not-declared`（[`use` 先查声明](#use-先查声明1000)）。
- **运行时行的行 `inject`**：行 `inject` 里的服务不在时，五种语言的行不再启动；提供方走了行会停下（[运行时行的行 `inject`](#运行时行的行-inject1001)）。
- **运行时进程的问候时限**：`Process::mount` 到时限还没等到问候就失败，`Mount` 多一个公有字段（[运行时进程的问候时限](#运行时进程的问候时限1014)）。

Go 与 Kotlin 的两处只放宽：原来报错的写法现在成功，原来成功的写法结果不变，只是错误文字换了（[Go 的接口与 `any` 目标](#go-的接口与-any-目标1015)、[Kotlin：`use` 先给本进程的对象](#kotlinuse-先给本进程的对象1006)）。

其余是新增的功能，不改已有的写法。

## 卸载顺序（#1010）

设计：[卸载顺序：撤回与清理同一个逆序栈](design-unload-order-2026-11-01.md)。六种语言（Go、Python、Bun、Kotlin、Swift、Node）都是这个行为。

### 改了什么

一行卸载时跑的是**一个**栈：`provide` 把“撤回这个服务”记进去，`ctx.effect` 与 `apply` 返回的清理也记进去。卸载时逆序、一条一条跑，撤回在自己的位置上发生。

| 情况 | 0.19 | 0.20 |
| --- | --- | --- |
| `provide` **之后**登记的清理读自己提供的服务 | Go、Python、Bun、Kotlin、Swift：`not-declared`；Node：读得到 | 六家都读得到，是对象本身 |
| `provide` **之前**登记的清理读自己提供的服务 | 同上：五家 `not-declared`，Node 读得到 | 六家都读不到：`use` 报 `not-declared`，`get` 返回空值 |
| 别的行在这一行卸载期间读它提供的服务 | 五家：撤回在清理之前删掉了，`unavailable`；Node：这一行的处置跑完之前仍读得到 | 六家都是 `unavailable`，从这一行开始卸载那一刻起 |
| 清理里再登记清理 | Kotlin 写一行、不运行；Go、Python、Bun、Swift 静默丢弃；Node 交给 Cordis 的处置表 | 六家都写一行 `rutis: row <行> is not active: cleanup is not registered`（Node：`plugin <插件>`），那条清理不运行 |
| 几个清理一起开始（Node） | Node 的 `_unload` 用 `Promise.all` 同时开始全部处置 | 六家都是一条一条，后登记的先跑 |

### 要改什么

- **清理里不要等别的清理。** 一条一条跑之后，一个清理等另一个清理（例如等一个 Promise，而它由后跑的那条清理兑现）就永远等不到：一次 `rows.unload` 六家都没有逐条的清理预算，这一行不回复，宿主也就一直等。
- **清理里不要挂清理。** 卸载跑的是开始卸载那一刻的栈，卸载期间登记的清理不运行（上表第四行）。把要做的放进一个清理里。
- **清理读自己提供的服务不再报错**（清理登记在 `provide` 之后时）：不用改代码，但也不要因此以为撤回早就跑过了——想读到服务，就把清理登记在 `provide` 之后；之前登记的仍然读不到。
- **不要依赖“别的行在清理期间还读得到”**：Node 上以前读得到，现在读不到，与另外五家一致。
- **卸载可能变慢**：清理从“同时开始”（Node）或“逐条但撤回提前”（五家）变成“逆序逐条”，一行有很多慢清理时卸载更慢。六家的清理截止时间并不相同，见[卸载顺序](design-unload-order-2026-11-01.md) §三「宿主持有者」的表格：Kotlin 在 `dispose` 时逐条计时，Go、Swift 在会话结束时给整次卸载一个预算，Python、Node、Bun 只在进程退出时放弃。

### 影响

- 插件作者：在 0.19 上正常工作的插件，只有依赖上面那张表里的这几处差别时才需要改：清理并行跑、清理里挂清理（六家），以及 Node 的两处——`provide` 之前登记的清理读自己提供的服务、别的行在这一行的清理期间读它（Node 以前读得到，现在读不到，与另外五家一致）。它们在 0.19 上也不是承诺，只是那时恰好成立。
- 宿主持有者：帧的顺序不变——宣布过的服务仍在 `rows.unload` 的回复之前撤回，回复仍在全部清理跑完之后；`--shutdown-timeout` 的含义不变，慢清理的行为见上面那张表。

## `use` 先查声明（#1000）

设计：[行的 `inject`，以及 `use` 先查声明](design-declared-services-2026-10-29.md)。六种语言一致。

### 改了什么

`use` 与 `get` 都是先查声明，再查表。已声明的是插件的 `inject`、行的 `inject`，加上插件**正提供着的**名字；`get` 再认插件的 `optional`（[可选依赖](design-optional-deps-2026-10-28.md) O4：`use` 不认 `optional`）。

| 情况 | 0.19 | 0.20 |
| --- | --- | --- |
| Go、Swift、Kotlin 的 `use`、`get`，名字在行 `inject` 里 | `not-declared`（这三家把 `rows.load` 的行 `inject` 丢掉了） | 进程里有就给：同进程提供的给对象本身，租借进来的给代理；没有时 `use` 报 `unavailable`、`get` 返回空值 |
| 插件读自己正提供着的服务 | Bun、Python 的 `use` 拿得到，Node 的要等 `apply` 之后；`get` 与 Go、Swift、Kotlin 的 `use` 报 `not-declared` | 提供着的时候 `use`、`get` 都给对象本身；撤回之后报 `not-declared` |
| Node、Bun、Python 的 `use`：插件与行都没声明，同一进程里**别的行**声明了 | 拿得到 | `not-declared` |

### 要改什么

- **把用到的服务写进插件的 `inject`。** 第三行是唯一会弄坏现有插件的一处：靠"进程里恰好有别的行声明了它"拿服务的插件，现在报

  `service llm is not available to this plugin, which does not declare it in inject: add 'llm' to its inject`

  （Go：`… which does not declare it in Inject: add "llm" to its Inject`。）离不开它，写进 `inject`；有就用、没有也行，写进 `optional` 并用 `get` 读（[可选服务](guide/plugin-api.md#可选服务)）。
- 按 `not-declared` 判断"服务不在"的代码会看到 `unavailable`（第一行）：服务在、只是没租借进来时的文字不一样。
- 测试工具没有"行"：`load` 的已声明是插件的 `inject` 与 `optional`，没有行 `inject` 这一项，与以前一样。

### 影响

- 插件作者：这是 [#676](https://github.com/arcships/rutis/issues/676) 要的结果，不是回归——以前拿不拿得到取决于进程里还装着什么，不可解释也不可测试。第一、二行只放宽，原来成功的调用不会因此失败。
- 用 `rutis-host` 组装插件的人：同一份 `rutis.json` 在六种语言下的声明集合一致了。没有新的配置，没有新的输出。

## 运行时行的行 `inject`（#1001）

设计：[运行时行的行 `inject`](design-row-inject-gate-2026-10-31.md)。

### 改了什么

运行时的行（Node、Bun、Python、Go、Swift、Kotlin 的行）在 rutis 里按行 `inject` 里**服务目录共享的名字**门控，启动时把这些名字租借进运行时，与插件的 `inject` 用同一段租借。以前：

| 运行时 | 行 `inject` 里的服务由宿主或别的进程提供、进程里还没有 |
| --- | --- |
| Node | 在 rutis 里显示运行，实际停在进程里的 Cordis 门控上，`apply` 不运行，一直等 |
| Bun、Python、Go、Swift、Kotlin | 不等，`apply` 照常运行，`use` 报 `service llm is not available` |

现在六种语言一样：`chat: waiting for llm (no row provides it)`（有相近的名字时括号里加 `; did you mean <名字>?`；有被禁用的行提供它时见 [rutis-host](guide/rutis-host.md) 的等待原因表），与 `rutis-host check` 早就报的一致。服务来了行启动、插件拿到它（[`use` 先查声明](#use-先查声明1000)）；服务走了行停下，回来后重新启动；给一行加上 `inject` 时这一行重启一次。

**服务目录不共享的名字**（应用登记成 Rust 类型的）不门控、不租借，由运行时自己解析，与以前一样：Bun、Python、Go、Swift、Kotlin 的行照常启动，`use('clock')` 报 `service clock is not available`；Node 的行在 rutis 里显示运行，进程里的 Cordis 门控等 `clock`，`apply` 不运行，没有输出。这类名字不是 `dyn HostDispatch`，跨不了语言。

### 要改什么

- **行 `inject` 里别写服务目录不共享的名字。** 应用登记成 Rust 类型的服务跨不了语言：从运行时行的 `inject` 里去掉，改在 Rust 行里用。
- **别依赖"行照常启动、`use` 失败了自己兜住"。** 行现在不会跑到 `apply`。要么让一行或宿主提供它，要么从行的 `inject` 里去掉。
- **写自己的 resolver（`Resolved::with_foreign_scope`）的。** 它的行同样按行 `inject` 里共享的名字门控（`rutis-host check` 早就这样报）。把插件在自己运行时里找到的这些名字公告给 rutis（像 `rows.announce`），否则这一行一直等：`<行 id>: waiting for <名字> (no row provides it)`。

### 影响

- 插件作者：靠行 `inject` 的服务才能运行的插件，现在一定在服务齐了之后才运行；只写在行 `inject` 里、由宿主或另一个进程提供的名字也拿得到（以前那五种语言拿不到）。
- 用 `rutis-host` 组装插件的人：六种语言的等待与重启行为一致了；Node 的行以前显示运行、实际在等，现在显示等待。`rutis-host check` 的输出没变。

## Kotlin：`use` 先给本进程的对象（#1006）

### 改了什么

`ctx.use(name, type)` 的取值顺序：

| 步骤 | 0.19 | 0.20 |
| --- | --- | --- |
| 查声明 | 不在插件的 `inject` 里就报 `not-declared` | 同左，声明集合还含行 `inject` 与插件正提供的名字（[`use` 先查声明](#use-先查声明1000)） |
| 取代理表 | 先取 `type` 的代理表，没有就以 `IllegalArgumentException` 失败：`<类型> has no proxy: annotate it @RutisClient and apply the rutis-ksp processor` | 最后才取，只在确实要用代理时 |
| 本进程的对象 | 代理表取到之后才看 | 先看：`type.isInstance(对象)` 就返回它本身 |

### 要改什么

没有要改的：只放宽。

- 只在本进程用的接口不必再标 `@RutisClient`：`@RutisService` 的提供方加 `ctx.use(name, 接口)` 就够。
- `provide` 一个不是 `@RutisService` 的对象仍然失败：`rutis: provide <名字>: <类名> is not a @RutisService class (no generated method table: apply the rutis-ksp processor)`。只在本进程用的服务也要标 `@RutisService`。

### 影响

插件作者：本进程的对象仍然优先；接口在别的进程仍然要有 `@RutisClient` 的代理，那几句报错不变。

## Go 的接口与 `any` 目标（#1015）

设计：[Go：同一进程里拿到提供方的对象本身](design-go-local-objects-2026-11-01.md)。

### 改了什么

`ctx.Use(name, &target)` 的目标按这个顺序定：

1. `*advanced.Service`（不变）；
2. 目标能装下本进程提供方的对象（Go 自己的赋值规则：它实现的接口、它自己的类型、`any`）——给这个对象本身，调用是普通的 Go 调用，参数与返回值不经编码；
3. 非空接口、提供方在别的进程：报错（见下）；
4. 函数字段：类型与提供方在本进程声明的方法完全相同就直接给方法值，不以 `error` 结尾也行；其余照跨进程的规则（以 `error` 结尾、参数能编码）。

以前第 2 步不存在，第 4 步的"类型相同"排在跨进程的签名检查之后，于是这些写法在 `bind` 里报错：

```text
use hello: the target must point to a struct of function fields or a *advanced.Service, not rutis.Greeter
use canvas: field Draw: func(string) string must return an error last
```

现在：

| 目标 | 提供方在本进程 | 提供方在别的进程 |
| --- | --- | --- |
| 接口 | 对象本身；对象没实现时报错（见下） | 报错（见下） |
| `any` | 对象本身 | `*advanced.Service` |
| 函数字段 | 类型相同给方法值，其余照跨进程的规则 | 照旧 |
| `*advanced.Service` | 照旧（`Local()` 为 `true`） | 照旧 |

都是 `Use`、`Get` 返回的 `error`，不带错误码，不写到 stderr：

| 怎样引起 | 输出（逐字） |
| --- | --- |
| 接口目标，提供方在别的进程 | `use workspace: editor.Workspace is an interface, which only a service provided in this process fills: for one of another process, use a struct of function fields or *advanced.Service` |
| 接口目标，本进程的对象少一个方法 | `use workspace: *editor.files does not implement editor.Workspace (missing method OnDraw)` |
| 接口目标，方法在但签名不同 | `use workspace: *editor.files does not implement editor.Workspace (wrong type for method Open)` |
| 接口目标，`Provide` 的是值、方法在指针上 | `use workspace: editor.files does not implement editor.Workspace (method OnDraw has pointer receiver)` |

### 要改什么

- **接口目标只在同一进程成立。** 以接口 `Use` 一个服务的插件，部署时这个服务被挪到别的进程就报第一句。跨进程用函数字段的结构体或 `*advanced.Service`。
- 按旧的两句文字判断的代码改一下：接口与 `any` 不再报 `must point to a struct of function fields …`，同类型的函数字段不再报 `must return an error last`。
- 提供一侧不变：`MethodsOf` 仍要求方法能跨进程（以 `error` 结尾、参数能编码）；只在本进程用的服务不写进 `Provides`。

### 影响

插件作者：以前这三种写法只能绕开 rutis（包级全局变量传对象）；现在直接拿到对象，`ctx.Get` 同样。以前成功的写法结果不变。

## 运行时进程的问候时限（#1014）

### 改了什么

`Process::mount` 等运行时进程问候以前没有时限（`peer.ready()`）：一个起来了却一直不问候的运行时会让它永远等下去。0.19 给 `Greet::open`、装载器、rutis-host 都加了问候时限，这一处是遗漏。现在：

- 默认 30 秒，常量是 `rutis_bridge::runtime::GREETING`；rutis-host 的 `SpawnPlugin` 原来自己有一份 30 秒，现在用同一条常量。
- `Mount` 多一个公有字段 `greeting: Option<Duration>`：不设用 `GREETING`，`Duration::MAX` 不限（调试器停在下一条语句的进程）。
- 到时限还没问候：`Error::Transport`，文字是 `the runtime process <名字> started but did not greet within <时限>s`，并关闭会话，这个进程随之结束。
- 时限有三处：`Session::handshake` 10 秒（通道已由别人打开，没有进程要启动），`Process::mount` 与 rutis-host 的 `SpawnPlugin` 30 秒（要启动一个冷进程，用同一条常量）。

### 要改什么

- 用 `Mount { … }` 写全所有字段的构造：补 `greeting: None`，或改用 `..Mount::default()`（`Mount` 实现了 `Default`）。
- 用调试器停在运行时第一条语句的：给这个 mount 设 `greeting: Some(Duration::MAX)`，否则 30 秒后这次 mount 失败，进程也被结束。

### 影响

宿主持有者：一个不问候的运行时现在按时限失败，报错写明哪个运行时的进程、时限多少，进程也结束；以前是一直等。用 rutis-host 的看不到变化：它本来就是 30 秒。
