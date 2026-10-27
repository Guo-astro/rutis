# 试用与测试插件：`rutis-host call`、`dev` 列出服务、本语言的假依赖、测试工具听事件

[English](design-try-and-test-plugins-2026-10-27.en.md)

状态：草案，未实现。日期：2026-10-27。基准：`main` `103c0d22`（0.17.2 之后）。排期：0.19。
Issue：[#670](https://github.com/arcships/rutis/issues/670)（属于 [#663](https://github.com/arcships/rutis/issues/663) 的 A5、A6、H5）。
与 0.18 的关系：建在 [0.18 的诊断](design-diagnostics-2026-10-26.md)上，不重复、不改它：`call` 用它 §5.3 的本地 socket 与找法，`dev` 列出的服务就是它 §5.1 `Loader::status()` 的 `services`，出错文字按它 §4（`site`、错误码、§4.5 的禁用词）。依赖 [#678](https://github.com/arcships/rutis/issues/678) 先合并。
与其他 0.19 设计的关系：从宿主的 `#[rutis::service]` 生成严格假服务、装进测试工具，归 [#684](https://github.com/arcships/rutis/issues/684)；本稿只管插件作者自己写的假依赖和测试工具的 `on`。SDK 默认入口归 [#692](https://github.com/arcships/rutis/issues/692)：本稿不往 SDK 根上加名字，`on` 加在测试工具装载出来的对象上。限制插件能用哪些服务归 [#683](https://github.com/arcships/rutis/issues/683)，它限制的是插件，`call` 是宿主自己的调用。

范围外：Windows 上的 `call`（0.18 的 `status` 在 Windows 上也不提供）；从命令行写 JSON 以外的值（大整数超出 64 位、时间点、日期、二进制、Map、Set、Decimal、UUID、函数）；调用实例里的服务；从命令行发事件（`rutis-host emit`）；`call` 打开一个新的宿主（它只连正在运行的）；`run` 打印服务表（用 `rutis-host status`）；`new` 生成假依赖（模板与指南的例子归 [#671](https://github.com/arcships/rutis/issues/671)）；TypeScript、Python 项目里用别的语言写假依赖（它们已经能用本语言写）；测试工具里"收集收到的事件"的便捷写法（用 `on` 加一个列表就够）；Node 运行时缺 `@arcships/rutis` 时的安装提示（出错文字，归 0.18 #677 的改写清单）。

## 一、开发者要做成什么，现在卡在哪

一个 Go 作者写了 `weather` 插件，它用 `llm` 服务。他想：

1. 跑起来后调一下 `weather.today("Oslo")`，看返回什么、出错时报什么；
2. 改了代码重新加载后，确认 `weather` 还在提供 `forecast`；
3. 用 Go 写一个假的 `llm`，和 `weather` 一起跑；
4. 写测试："每次调用都发出 `weather/asked`"。

0.17.0 的评估（#663）里四件都卡住了：

| # | 现在 | 代价 |
| --- | --- | --- |
| 1 | 没有调用的办法 | TypeScript、Python、Go、Rust 四种语言的作者都另写了一个"调用者"插件；宿主作者写了探测插件 |
| 2 | `dev` 只打印 `weather: running` | 看不出提供了哪些服务、由谁提供 |
| 3 | `dev` 只构建项目自己的那个 Go 二进制；指南让假依赖用 Python 或 TypeScript 写 | Go 作者在 Go 项目里建 Node 项目、装 npm 包，中途两次报错 |
| 4 | TypeScript、Python 的测试工具没有 `on`（Go、Swift、Kotlin 有）；Python 测试工具的接口没有文档 | 要在共享总线上另装一个监听插件；用 `dir()` 找接口 |

## 二、已有的接缝

| 要的 | 已有 |
| --- | --- |
| 宿主按名字调用一个服务 | `rutis_bridge::session::call_service(&root, 服务, 方法, 参数列表)`（[Rust 嵌入指南](guide/rust-host.md#在-rust-里按名字调用服务)）：任何语言、远程运行时、节点提供的服务都能调；同步方法放到阻塞线程；调用不来自任何行 |
| 从另一个进程找到正在运行的宿主 | 0.18 §5.3：`run` / `dev` 开一个本地 socket（0600，系统运行时目录，文件名带配置路径的散列），`rutis-host status` 按同一配置路径找到它 |
| 服务和提供者 | 0.18 §5.1：`Loader::status().services: [{name, provider}]` |
| `dev` 里"变化已经结束" | `dev` 每次启动、重新加载、重新构建、重读 `rutis.dev.json` 之后都 `loader.settled().await`（`crates/rutis-host/src/dev.rs`） |
| 构建 Go、Swift 二进制并作为运行时运行 | `GoDev` / `SwiftDev`（`languages/go.rs`、`swift.rs`）、`dev_rows` 从二进制的清单读出行、`GoRuntime.binaries` 已能列多个二进制，一个二进制一个运行时 |
| 测试工具听事件 | Go `loaded.On(name, fn)`、Swift `loaded.on(name) { … }`、Kotlin `loaded.on(name) { … }` 已有，规则一样：像宿主一样在根作用域监听，返回撤销函数 |

所以四件事都是在已有的东西上补缺口：不新开通道、不新建状态数据、不改内核和 loader。

## 三、决策

| # | 决策 | 章节 |
| --- | --- | --- |
| T1 | `rutis-host call <服务>.<方法> [参数…]`：连 0.18 `status` 用的同一个 socket，多一种 `call` 请求；找法与 `status` 相同；只在 Unix 上 | 四 |
| T2 | `dev` 默认接受 `call`；`run` 默认不接受，`run --allow-call` 才接受；`call` 请求另外核对连进来的进程是同一用户 | 四 |
| T3 | 每个参数是一个 JSON 值；结果打印成 JSON；JSON 表达不了的参数类型不支持 | 四 |
| T4 | `call` 发起的调用，调用方记为"宿主"（与 0.18 里所有不在行里的调用相同）；错误的 `site` 照 0.18 是被调用的那一行 | 四 |
| T5 | `dev` 在启动和每次变化结束后打印一行服务表，数据就是 `Loader::status().services`；和上次打印的一样时不再打印 | 五 |
| T6 | 假依赖用插件自己的语言写：`rutis.dev.json` 里一行的 `name` 是项目里的一个目录（Go 的 main 包、Swift 的可执行 target），`dev` 把它构建成第二个二进制、作为它自己的运行时运行；不用 `new` 生成 | 六 |
| T7 | Go、Swift 项目的源码变了，只重新构建包含这个文件的那些二进制（Go 用 `go list -deps`，Swift 用 `swift package describe`）；`check` 同样构建它们 | 六 |
| T8 | TypeScript（Node、Bun）、Python 的测试工具加 `t.on(name, listener, 选项)`，返回撤销函数，形式与 Go、Swift、Kotlin 的测试工具和各语言的 `ctx.on` 相同；加在装载出来的对象上，不加到 SDK 根上 | 七 |
| T9 | Python 测试工具的接口写进 Python 指南（一张参考表）；各语言指南的"测试"一节写明 `on` | 七 |

## 四、`rutis-host call`

### 4.1 用法

```text
rutis-host call [--config <路径>] <服务>.<方法> [参数…]
```

- `<服务>.<方法>` 在最后一个 `.` 处拆开（服务名可以带点，方法名不带）。
- 每个参数是一个 JSON 值：`3`、`true`、`null`、`'"Oslo"'`、`'{"days": 3}'`、`'[1, 2]'`。不是 JSON 的参数报错，不猜成字符串（`007`、`yes` 猜错了会悄悄传错值）。
- 找宿主与 `rutis-host status` 完全相同（0.18 §5.3）：`--config` 给的配置路径，否则当前目录的 `rutis.json`；在插件项目里是 `dev` 用的那个项目。不往项目里写文件。

### 4.2 宿主一侧

- 0.18 的 socket 多答一种请求：`call`，带服务名、方法名、参数列表。socket 上的格式是 rutis-host 自己的，`call` 与 `status` 是同一版本的 rutis-host，不对外承诺。0.18 定的"只答 `status`"在不接受 `call` 的 `run` 里照旧成立。
- 宿主收到后执行 `call_service::<Value>(&root, 服务, 方法, 参数)`，与 Rust 嵌入的按名字调用相同：调用根作用域按名字看到的那个服务，任何语言的行、远程运行时、节点提供的都可以；同步方法在阻塞线程上执行。
- 不设时限（0.18 §6.1：等多久归调用方）。`rutis-host call` 一直等到结果；Ctrl-C 结束 `call` 进程、断开 socket，宿主一侧丢弃这次调用的 future，取消照常传到对面（TypeScript 的方法不会被打断，与 0.18 §6.1 相同）。
- 一次连接一个调用；几个 `call` 同时进行互不影响。

### 4.3 权限

| | `dev` | `run` | `run --allow-call` |
| --- | --- | --- | --- |
| `status` | 答 | 答 | 答 |
| `call` | 接受 | 拒绝，告诉怎样打开 | 接受 |

- **为什么 `run` 默认不接受**：`call` 能执行任何服务的任何方法，生产环境里这等于给同一用户的任何进程一个后门；默认与 0.18 的只读 socket 一致，需要时由运维在启动命令上明确打开。开关放在命令行（`--allow-call`），不放进 `rutis.json`：它是这次运行的决定，不随配置文件被复制到别处。
- **谁能连**：socket 文件 0600，只有同一用户（和 root）能连。`call` 请求另外用对端凭据（Linux `SO_PEERCRED`，macOS `getpeereid`）核对连进来的进程是同一用户，不靠文件权限一层（有的系统不按 socket 文件的权限拒绝连接）；不一致时拒绝，不执行。
- **Windows**：0.18 不提供 socket，`call` 也不提供，打印 `rutis-host call is not available on Windows`。
- rutis-dev 的通道不复用，理由同 0.18：它不发布，且带改动命令；`call` 只需要已经要做的这一个 socket。

### 4.4 参数与结果的值

参数只支持 JSON 能写出来的值：`null`、布尔、数字、字符串、数组、对象。整数在 64 位以内的，按已有的规则传（超出 ±(2^53−1) 自动按大整数传，[插件 API：十种值](guide/plugin-api.md#十种值)）。日期、二进制、Map、Set、Decimal、UUID、更大的整数、函数从命令行写不出来，不支持：要试这类方法，在测试里调用。字符串到对面就是字符串（Python 的方法要 `date` 时收到的是 `str`，与 JS 发来的字符串一样）。

结果打印成 JSON（两格缩进）到标准输出。JSON 没有的值写成能读的样子，这只是为了给人看，不保证能原样再传回去：

| 结果里的值 | 打印成 |
| --- | --- |
| 没有结果（`undefined`、`None`、`nil`） | `null` |
| 大整数 | 数字（全部位数） |
| 时间点、本地日期时间、日期、时刻、Decimal、UUID | 字符串（规范写法，时间点为 UTC） |
| 二进制 | 字符串 `"<12 bytes>"` |
| Map | 数组 `[[键, 值], …]` |
| Set | 数组 |
| 函数、按引用传的对象、插件定义 | 字符串 `"<function>"`、`"<object>"`、`"<plugin>"` |

### 4.5 调用方是谁

`call` 发起的调用不来自任何行，记为"宿主"：

- 状态快照（0.18 §5.2）里是 `宿主 → weather.today  3s`（英文输出写 `host`）；
- 错误的 `site` 照 0.18 §4.1 是被调用的那一行，`call` 打印它的位置；没有"调用方"那一段位置；
- 方法要调用方那一行的 `&Ctx`（例如替行登记工具）时，照 `call_service` 现在的规则失败，文字见 §八。

不另设一种"命令行"身份：rutis-host 自己不调用任何服务，在 rutis-host 里"宿主"发起的调用就只有 `call`，不会混淆；另设身份要改 `caller_row()` 和 0.18 的快照，没有使用方。

## 五、`dev` 列出服务

- 启动后（所有行 `settled`）打印一行：

  ```text
  rutis-host dev: services: forecast (weather), llm (llm)
  ```

  按服务名排序，括号里是提供它的行（0.18 的 `provider`；宿主提供的写 `host`）。没有服务时写 `rutis-host dev: services: none`。
- 每次重新加载、重新构建、重读 `rutis.dev.json` 结束（已有的 `settled` 之后）再读一次；和上次打印的不同时打印新的一行，相同时不打印（大多数重新加载不改服务，每次都打会淹没有用的行）。
- 数据只来自 `Loader::status().services`，不另建。`run` 不打印（生产输出不变），要看用 `rutis-host status`。
- 这是 `dev` 输出里新加的一种行，以 `rutis-host dev: ` 开头，和已有的 `rutis-host dev: running …` 一样，不像 `<行>: <状态>`，读状态行的脚本不受影响；已有的行不变。写进 rutis-host.md 的"用 dev 开发插件"一节的输出表。

## 六、用插件自己的语言写假依赖

### 6.1 选哪种：`dev` 构建另一个入口

issue 给了两种：`dev` 构建项目里另一个入口，或 `new` 生成一个能跑的假依赖。选前者：

- `new` 不知道插件将来会用什么服务，生成的假依赖只对模板里的例子有用；作者自己写的假依赖照样要有办法跑起来，这个办法就是前一种。
- 前一种和 TypeScript、Python 已有的写法一致：`rutis.dev.json` 里一行，`name` 指向项目里的假依赖（`./dev/fake-llm.ts`）。
- 模板里要不要带一个假依赖，是模板与指南的事（#671），用的也是这个机制。

### 6.2 写法（Go）

```text
weather/
  go.mod  plugin.go  plugin_test.go
  cmd/weather/main.go        # 插件本身
  dev/fakellm/main.go        # 假的 llm：普通的 Go main 包
  rutis.dev.json
```

```go
// dev/fakellm/main.go
package main

import (
	"context"

	"github.com/arcships/rutis/go/rutis"
)

type llm struct{}

func (llm) Ask(ctx context.Context, question string) (string, error) { return "sunny", nil }

var plugin = rutis.Define(rutis.Plugin[rutis.NoConfig]{
	Name:  "fakellm",
	Apply: func(ctx *rutis.Ctx, _ rutis.NoConfig) error { ctx.Provide("llm", llm{}); return nil },
})

func main() { rutis.Serve(plugin) }
```

```json
{
  "format": 1,
  "rows": [
    { "id": "weather", "config": { "city": "Oslo" } },
    { "id": "llm", "name": "./dev/fakellm" }
  ]
}
```

规则：

- `rutis.dev.json` 里 `name` 是以 `./` 开头、指向项目里一个目录的行：在 Go 项目里，这个目录是同一个 go.mod 下的 main 包；在 Swift 项目里，它是 Package.swift 里一个可执行 target 的 `path`。`dev` 把它构建成 `.rutis/go/fakellm-<n>`（Swift：`.rutis/swift/<target>-<n>`），作为它自己的运行时（`go-fakellm`）运行，这一行就是这个二进制里的插件。
- 这个二进制只能有一个插件；有几个时报错（§八）。运行时名与项目自己的相同时报错，让作者改目录名。
- 只在 `dev` 和插件项目里的 `check` 里有这种行（都从 `project::dev_config` 读 `rutis.dev.json`）：读的时候换成已有的 `go-<名>:<插件>` 行，并把构建加进 Go 的设置（`GoRuntime.dev` 从一个二进制改成一个列表）。`rutis.json` 和 `run` 不认这种写法，loader 不变。
- `dev/fakellm` 是模块里的普通包，`go build ./...`、`go test ./...` 会编译它；发布的工作流只构建 `cmd/<名>`，不受影响。
- 测试里照旧把一个 Go 值放进 `rutistest.Load` 的 `services`。想和 `dev` 共用同一个类型时，把类型放进一个普通包（如 `internal/fake`），`dev/fakellm/main.go` 和测试都导入它；指南写这一句。

Swift 写法相同：在 Package.swift 里加一个可执行 target（`.executableTarget(name: "FakeLLM", dependencies: [.product(name: "Rutis", package: "rutis")], path: "dev/FakeLLM")`），行名写 `./dev/FakeLLM`；`dev` 用 `swift build --product FakeLLM` 构建。要多写 Package.swift 的这一项，是 SwiftPM 自己的规则，不是 rutis 的。

### 6.3 改了文件时重新构建哪个

现在 Go 项目里任何 `.go` 文件变了都重新构建项目自己的二进制、重启它的运行时。有了第二个二进制，改假依赖不应重启插件，反过来也一样（"一个插件的变化不重启别人"）：

- 每个二进制在构建后记下它包含的、项目里的目录：Go 用 `go list -deps -f '{{.Dir}}' <包>`，Swift 用 `swift package describe --type json` 里 target 的 `path` 与它依赖的本地 target。
- 一个源码文件变了，只重新构建包含它所在目录的那些二进制；`go.mod`、`go.sum`、`go.work`（Swift：`Package.swift`、`Package.resolved`）变了，全部重新构建。
- 每个二进制各自"构建失败时上一次的构建继续运行"，打印的行与现在相同（`<项目>: building` 用这个二进制的目录名）。
- 假依赖导入了插件的包时，改插件的包会两个都重新构建，这是对的。

### 6.4 其他语言

TypeScript、Python 项目已经能用本语言写假依赖（`./dev/fake-llm.ts`、`py:fake_llm`），不变。Kotlin 不在 `rutis-host dev` 里运行（它是应用里的插件）。Rust 插件由宿主自己编译，不经 `dev`。

## 七、测试工具直接听事件

### 7.1 形式

Go、Swift、Kotlin 的测试工具已经有 `on`；TypeScript（Node、Bun 共用 `@arcships/rutis/testing`）和 Python 补上同样的：

```ts
const t = await load(weather, { config: { city: 'Oslo' }, services: { llm } })
const asked: string[] = []
const off = t.on('weather/asked', (city: string) => { asked.push(city) })
await t.service('weather').today()
assert.deepEqual(asked, ['Oslo'])
off()
```

```python
async with load(plugin, config={"city": "Oslo"}, services={"llm": FakeLlm()}) as t:
    asked = []
    off = t.on("weather/asked", lambda city: asked.append(city))
    await t.service("weather").today()
    assert asked == ["Oslo"]
```

- 名字、参数、选项与这门语言的 `ctx.on` 相同：TypeScript `t.on(name, listener, { prepend, global })`，Python `t.on(name, listener, *, prepend=False, global_=False)`；监听器可以是同步或异步函数，返回值在 `serial`、`waterfall`、`bail` 里照常算数。
- 规则与 Go 的 `loaded.On` 相同：像宿主一样在根作用域监听；插件发出的这个名字、测试自己发出的都送到；严格模式（默认）下参数像从另一个进程收到的那样；返回撤销函数；不撤销时随测试工具卸载而撤掉。
- 加在装载出来的对象上：TypeScript 的 `Loaded`（`host()` 装载的每一行都有，它们共用一条总线），Python 的 `Loaded`。不加到 SDK 根上，不加到 `ctx` 上（`ctx.on` 已有），符合 #692"插件作者默认只看到 7 个概念"——`on` 是"测试工具"这个概念里的一个方法。
- 没有"返回收到的事件列表"的另一种写法：一个列表加一行就够，四种语言保持一种形式。

### 7.2 文档

- Python 指南"测试"一节加一张参考表：`load(...)` 的参数（`config`、`services`、`strict`、`bus`、`scope`、`row`、`runtime`、`deadline`），`Loaded` 的成员（`service`、`methods`、`provided`、`on`、`emit`、`parallel`、`serial`、`waterfall`、`bail`、`filter`、`reported`、`update`、`unload`），`Bus`、`Reported`、`PluginTestError`。内容取自 `rutis/testing.py` 的文档字符串，指南与它一致。
- TypeScript、Bun、Python、Swift、Kotlin 指南的"测试"一节都写一句 `on`（Go 指南已有）。
- 各语言指南"在本地宿主里运行"一节加 `rutis-host call` 的一步，Go、Swift 指南改用本语言的假依赖（§6.2）。

## 八、开发者看到什么

插件作者在 Go 项目 `weather` 里：

```console
$ rutis-host dev
weather: building
weather: built in 1.4s
fakellm: building
fakellm: built in 0.6s
weather: running
llm: running
rutis-host dev: running weather; changes reload it (Ctrl-C ends)
rutis-host dev: services: forecast (weather), llm (llm)
```

另一个终端：

| 输入 | 输出（标准输出 / 标准错误）与退出码 |
| --- | --- |
| `rutis-host call forecast.today '"Oslo"'` | `"sunny in Oslo"`，0 |
| `rutis-host call forecast.week '{"days": 3}'` | 两格缩进的 JSON，0 |
| `rutis-host call forecast.today Oslo` | `rutis-host: argument 1 is not JSON: write a string in quotes, as '"Oslo"'`，1 |
| `rutis-host call forecast` | `rutis-host: name a method, as forecast.<method>`，1 |
| `rutis-host call forcast.today '"Oslo"'` | `rutis-host: no row provides forcast; did you mean forecast? (services: forecast, llm)`，1 |
| `rutis-host call forecast.tomorow '"Oslo"'` | `rutis-host: forecast has no method tomorow (methods: today, week)`，1（0.18 `no-method`） |
| `rutis-host call forecast.today 3` | `rutis-host: forecast.today: argument 1 should be a string, got 3`，1（0.18 `bad-argument`） |
| 方法抛错 | `rutis-host: call to forecast.today failed: KeyError: 'region'`，下一行 `weather at plugin.go:31 in Today`，1 |
| 提供者在等运行时 | `rutis-host: forecast is not available (row weather waits for its runtime go-weather)`，1（0.18 `unavailable` 的原因） |
| 方法要调用方那一行 | `rutis-host: tools.register needs a calling row; call it from a plugin, not from rutis-host call`，1 |
| 没有在运行的宿主 | `rutis-host: no rutis-host run or dev is running for /work/weather (start one, or name its configuration with --config)`，1 |
| 宿主是 `run`、没开 `--allow-call` | `rutis-host: this rutis-host run does not accept calls; start it with rutis-host run --allow-call`，1 |
| 不是同一用户 | `rutis-host: the host runs as another user and does not accept calls from this one`，1 |
| Windows | `rutis-host call is not available on Windows`，1 |
| 等待中按 Ctrl-C | 不打印；宿主一侧取消这次调用，130 |
| 同时 `rutis-host status` | `host → forecast.today  12s` |

改了 `dev/fakellm/main.go`：

```text
fakellm: building
fakellm: built in 0.5s
go-fakellm: rebuilt and restarted
weather: restarted (go-fakellm restarted)
```

`weather` 的二进制不重新构建；它用到 `llm`，所以按已有规则随之重启。服务没有变，不打印服务行。

假依赖写错时：

| 情况 | 输出 |
| --- | --- |
| 目录不是 main 包 | `rutis-host: rutis.dev.json: row llm: ./dev/fakellm is not a Go main package (its package is fakellm); a fake needs package main and a main that calls rutis.Serve` |
| 二进制里有两个插件 | `rutis-host: rutis.dev.json: row llm: ./dev/fakes serves 2 plugins (fakellm, fakedb); give each its own directory` |
| 运行时名和项目的相同 | `rutis-host: rutis.dev.json: row llm: ./dev/weather would run as go-weather, as the project does; rename the directory` |
| 编译失败 | `fakellm: the build failed; the last build keeps running`，下面是编译器的错误（启动时没有上一次的构建：`dev` 以 1 退出） |
| Swift：目录不是可执行 target | `rutis-host: rutis.dev.json: row llm: ./dev/FakeLLM is not the path of an executable target in Package.swift; add .executableTarget(name: "FakeLLM", path: "dev/FakeLLM", …)` |

测试里：TypeScript、Python 的 `t.on` 收到事件；名字不合规则（如 `rutis/` 开头的保留名）时与 `ctx.on` 报同样的错误。

以上文字不出现 0.18 §4.5 的禁用词。

## 九、兼容与迁移

全部是新增，不进《迁移到 0.19》：

- 新命令 `rutis-host call`、`run` 的新选项 `--allow-call`；
- `dev` 输出多一种以 `rutis-host dev: services:` 开头的行，已有的行不变；
- `rutis.dev.json` 的行多一种 `name` 写法，只在 `dev` / 项目里的 `check` 有效；
- Go、Swift 项目改一个源码文件时，不包含它的二进制不再重新构建（以前任何 `.go` 文件都重新构建项目）；
- 测试工具多一个方法 `on`。

0.18 的 socket 多答一种请求；`run` 不加 `--allow-call` 时与 0.18 完全相同。

## 十、自审：是不是太重了

| 项 | 使用方 | 已有的东西 | 结论 |
| --- | --- | --- | --- |
| `call` | 4 种语言的作者、宿主作者都另写了调用插件 | socket、`call_service` | 保留；不新开通道 |
| `run --allow-call` | issue 要求对 `run` 也能调用；运维核对部署 | 0.18 的只读默认 | 保留，默认关 |
| 对端凭据核对 | 生产里打开 `call` 时的安全 | 0600 | 保留；只核对 `call` |
| 结果的非 JSON 值表 | 结果里出现日期、二进制时总要打印点什么 | `Value::json()` 只管纯 JSON | 保留，一张表 |
| 另设"命令行"调用方 | 没有 | "宿主" | 删去 |
| 从命令行写日期、二进制等 | 没有具体的使用方，测试里能做 | — | 删去，写进范围外 |
| `dev` 服务行 | issue 第 2 条 | `Loader::status()` | 保留；不变时不打印 |
| 第二个二进制 | Go 作者为假依赖建了 Node 项目 | `GoDev`、`dev_rows`、`binaries` | 保留，只在 dev |
| 按依赖重新构建 | 改假依赖不重启插件 | `go list`、`swift package describe` | 保留 |
| 一个目录多个插件、`./dir:<插件>` | 没有 | — | 删去，报错说明 |
| `new` 生成假依赖 | 只对模板例子有用 | — | 删去，模板归 #671 |
| 测试工具 `on` | TS、Python 作者另装监听插件 | Go、Swift、Kotlin 已有 | 保留，同一形式 |
| "收集事件"的便捷写法 | 一行列表就够 | — | 删去 |

## 十一、实施

| 步骤 | 内容 | 完成条件（对应 issue 的「完成」） |
| --- | --- | --- |
| C1 | `rutis-host call`、socket 的 `call` 请求、对端凭据核对、`run --allow-call`、结果打印 | §八 `call` 的每一行有测试逐字锁住（含 `run` 拒绝、不是 JSON、方法抛错的两段位置）；跨语言各调一次 Node、Bun、Python、Go、Swift 提供的服务；rutis-host.md 命令表与新的一节、各语言指南"在本地宿主里运行"加 `call`（完成 1） |
| C2 | `dev` 的服务行 | 启动、重新加载后服务变化、不变三种情况的输出测试；rutis-host.md 输出表（完成 2） |
| C3 | `rutis.dev.json` 里的目录行、第二个二进制、按依赖重新构建、`check` 构建 | Go：§八 的启动、改假依赖只重建它、四种写错的输出有测试；Swift：同样的用例，和现有的 Swift `dev` 测试放在一起（要 macOS 与 Swift 工具链）；Go、Swift 指南改用本语言的假依赖（完成 3） |
| C4 | TypeScript、Python 测试工具的 `on`；Python 测试工具参考表；各语言指南写 `on` | 两种语言各有测试：插件发出、测试自己发出、严格模式的参数、撤销、卸载时撤掉、`serial` 里的返回值；指南的例子由 docs 例子测试跑过（完成 4） |

C1、C2 依赖 0.18 #678（socket、`Loader::status()`）；C3、C4 不依赖别的，可以先做。
