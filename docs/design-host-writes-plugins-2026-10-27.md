# 宿主自己写插件：把 loader 作为各语言插件都能用的服务

[English](design-host-writes-plugins-2026-10-27.en.md) · [中文](design-host-writes-plugins-2026-10-27.md)

状态：定稿，未实现。日期：2026-10-27；2026-10-28 修订：按 [远程连接：只留一个会话插件](design-remote-session-2026-10-28.md)（#811）与 [0.19 平台清理](design-platform-cleanup-2026-10-28.md)（#813）对照改写——排期改为 0.19 内实现、拆成三个 PR；去掉已删除的 `share_by_name` / `share_registered_only`、#683 的 `allowed_services`、`run --allow-call`、嵌入 `Installer`；写回 `rutis.json` 复用 #805 的写回；`status()` 按 0.19 的快照；补"开发者看到什么"的各角色与错误。2026-10-29 修订（H3）：§七 的文字按实现逐字改正——错误码是 Rust 服务通用的 `Error`；没给 `id` 时生成 8 位十六进制；Python 的语法错误、`apply` 失败、找不到模块按运行时实际的写法；Python 重载时 `apply` 失败的行从此跑新代码并失败（与 `dev` 相同），不装回旧版本；`reload` 成功不另打终端行（调用方要能在行之外调用，例如 `rutis-host call`，所以方法不取调用方的行）。2026-10-29 修订（H2）：写回时把 loader 的这次编辑套在文件里原样的 `rows` 上再写出（插件文件的相对路径、节点行的身份按文件里的写法，不写宿主运行时补全的样子）；SIGUSR1 与 `--watch-config` 的重新读取从 loader 正在运行的行算起（含插件加、删的行），被拒绝时换回的也是它们。基准：`main` `212ac7ce`。排期：0.19。
Issue：[#185](https://github.com/arcships/rutis/issues/185)（含并入的 [#693](https://github.com/arcships/rutis/issues/693)）；实施跟踪 [#771](https://github.com/arcships/rutis/issues/771) 的 H1–H4。取代 [#505](https://github.com/arcships/rutis/pull/505) 的草案。
与 0.18 的关系：状态用 [0.18 的诊断](design-diagnostics-2026-10-26.md) §五 的 `Loader::status()`，出错文字按它 §四；不加结构。

范围外：从包仓库安装（#691 的 `rutis-host add`，在 rutis-host 里）；`update`、`set_disabled`、`move_to`、`check` 等其余 `Loader` 方法（§三，S1 用不到）；任何权限或限制（谁能用哪个服务是业务的分配，平台不做，见 #813 §5.5）；agent 的工具、提示、批准（agent 产品的事）；一行对自己 `reload` / `remove`（会等它自己正在进行的调用，S1 用不到）；`rutis-host dev` 的可编辑层（§五）；Windows 上的 S1（#670 的 `call` 不在 Windows 上）。

## 一、要做成什么，现在卡在哪

[设计哲学](design-philosophy.md) §1：宿主需要接触什么，就自己写一个插件接上去，常常由 agent 来写；试用、修改、替换。

场景：宿主里有一个 Python 插件（例如一个 agent）。它要写一个新插件 `weather`、让宿主加载它、改坏了知道错在哪而旧版本继续服务、修好后换上、不要了删掉。

现状（`crates/rutis-loader/src/loader/`）：

- `LoaderPlugin::apply`（`plugins.rs`）已经 `ctx.provide(self.loader.clone())`。**Rust 插件**从 ctx 拿到 `Loader`，就能用 `create`、`update`、`set_disabled`、`remove`、`reload`、`entries`、`get`、`check`（`api.rs`、`check.rs`）管理插件。
- 编辑全有或全无（`commit.rs` 的 `commit`）：改写可编辑层 → 试运行（解析并导入新模块）→ 对账；有行新失败就退回原来的层、再对账一次，返回错误。改动经 `Persist` 存下；存的时候发现文件已被别人改过（版本不同），先读回文件里的行、对账，再重做这次改动（`persist_queue`）。
- `reload(id)`：重新解析这一行的模块，试运行，换上；失败时保留旧模块，旧版本继续服务。

缺的两处：

1. `Loader` 只按 Rust 类型提供，**没有按名字提供**，其他语言的插件拿不到。
2. **rutis-host 没有可编辑层**：它用 `reconcile(layers, None)`（`crates/rutis-host/src/host.rs`）把 `rutis.json` 交给 loader，所以在 rutis-host 里调 `create` / `remove` 会得到 `no editable layer`。

## 二、Cordis 的做法

Cordis 4 的 loader（dsh 带的 `@deepseek-ai/cordis-plugin-loader`）把自己作为服务 `loader` 提供（`ctx.reflect.provide('loader', …)`）。插件写 `inject: ['loader']`，然后用 `ctx.loader.create(options, parent, position)`、`ctx.loader.update(id, options, parent?, position?)`、`ctx.loader.remove(id)`（`config/tree.ts`）。每次改动写回配置文件（`tree.write()`）。没有权限层：任何插件都能用。重新加载代码由另一个插件（HMR）负责，不在 loader 上。

rutis 照这个做：同一个名字、同样的方法形状、同样谁都能用；只多一个 `reload`，因为 rutis 的代码重载本来就在 loader 上（`dev` 用的就是它）。这是通过插件扩展：`loader` 是 `LoaderPlugin` 提供的一个普通服务，不是宿主或内核里的专门入口。

## 三、决策

| # | 决策 |
| --- | --- |
| L1 | `LoaderPlugin` 再按名字提供一个服务 `loader`，走 0.17 起已有的路径：一个标了 `#[rutis::service(name = "loader")]` 的类型，`ctx.provide` 时自动对外。不另开通道 |
| L2 | 方法只放 S1 用得到的 4 个，名字与参数贴近 Cordis 的 `ctx.loader`（下表） |
| L3 | 任何插件都能 `inject: ["loader"]`，与 Cordis 相同。平台不加权限或限制：不想让某个插件管理插件的宿主，不给它这一行，或不在配置里放它 |
| L4 | 检查与替换只用 loader 已有的行为（§五）；补的只有 rutis-host 的可编辑层 |
| L5 | 本地目录的插件直接写进行名就能加载，不需要安装；从包仓库安装是 rutis-host 的 `add`（#691），不进 `loader` 服务 |

### 方法

| `loader` 的方法 | 参数 | 结果 | 对应的 Rust `Loader` | 对应的 Cordis `ctx.loader` |
| --- | --- | --- | --- | --- |
| `create(options, parent?, position?)` | `options`：`{id?, name, config?, disabled?, inject?}`，与 `rutis.json` 一行的写法相同 | 行 id | `create(NewEntry, parent, position)` | `create(options, parent, position)` |
| `reload(id)` | 行 id | 无 | `reload(id)` | 没有：Cordis 的代码重载在 HMR 插件里；rutis 的在 loader 上 |
| `remove(id)` | 行 id | 无 | `remove(id)` | `remove(id)` |
| `status()` | 无 | 状态快照，与 `rutis-host status` 读的是同一份：行与状态、原因，服务与提供者，进行中的调用（字段按 0.19 的快照，#813 CL21、#811 S4 之后） | `status()` | 没有对应方法（Cordis 插件直接读 `loader` 的条目对象）；跨进程拿不到对象，给快照 |

- 失败时抛出错误：消息是 rutis 打印给人看的那段文字（0.18 §四：哪一行、为什么、文件与行号），`data` 是 `{row}`。错误码是 Rust 服务通用的 `Error`（与返回字符串错误的方法相同），不加新的码。错误的名字按 #813 CL5 之后的线上名 `Error`（Python 里是 `rutis.RemoteError`，`name` 为 `"Error"`）。
- `update`、`set_disabled`、`move_to`、`set_inject`、`entries`、`get`、`check` 照旧只给 Rust；有使用方时按同样的方式加（Cordis 的 `update` 同时管配置和移动，加的时候照它）。

## 四、怎样跨语言提供

- rutis-loader 加一个类型 `LoaderService`（包着 `Loader`），标 `#[rutis::service(name = "loader")]`，方法是上表的 4 个 `async fn`，参数、结果都是 serde 类型。`LoaderPlugin::apply` 在已有的 `ctx.provide(self.loader.clone())` 旁边再 `ctx.provide(LoaderService(..))`。按 0.17 的规则（[Rust 服务自动对外](design-rust-service-exposure-2026-10-21.md)），标了的类型被提供时自动按名字对外，Node、Bun、Python、Go、Swift、Kotlin 的插件都能 `inject` 它；Rust 插件照旧直接拿 `Loader`。
- 为什么另起一个类型：`Loader` 已有的同名方法（`create(NewEntry, …)`）参数是 Rust 类型，不能直接给其他语言；宏不支持给方法改名，所以用一个薄的包装类型。
- 0.19 起服务总是按名字共享（#813 CL17 删掉 `share_by_name` / `share_registered_only`），所以用 `LoaderPlugin` 的宿主都对外提供 `loader`，没有关掉它的开关。rutis-dsh 的生产代码不用 `LoaderPlugin`（只有测试 `profile_loader.rs` 用），它的 Cordis 自己的 `loader` 不受影响。
- 另有一行也提供名叫 `loader` 的服务时，按内核已有的同名规则处理（后提供的一方失败），不另加检查或专门的报错（§七"用 rutis-host 组装插件的人"）。
- 连接插件（#811 §5.3）也经这个服务加载：agent 写的 TS / Python 连接插件用 `create` 装上，`remove` 它时它的 session 与其下的远程插件一起卸载。不为它另加方法。

## 五、检查与替换：已有的和缺的

| 要的 | 现状 | 本稿 |
| --- | --- | --- |
| `create` 前检查 | 已有：`commit` 先试运行（解析、导入新模块；语法、导入错误在这里报出） | 不变 |
| `create` 失败 | 已有：有行新失败就**退回**原来的层，不留下失败的行，返回错误 | 不变；错误的文字按 0.18 |
| `reload` 失败 | 已有：新模块解析不了或试运行不过（语法、导入错误）时保留旧模块；新代码在 `apply` 里让行失败时，换回旧的解析结果再对账 | 不变，与 `Loader::reload` 的文档同一种说法：语法、导入错误时旧版本继续运行；`apply` 失败时，换回的旧解析结果对按文件加载模块的运行时（Python、Node）来说就是文件里的新代码，所以这一行跑新代码并以那个错误失败，直到改好再 `reload`——与 [`dev`](guide/rutis-host.md#用-dev-开发插件) 的重新加载相同。不另做回滚 |
| `remove` | 已有：卸载这一行，每项清理执行一次；运行时进程不因此退出（它可能还有别的行） | 不变 |
| 改动存下 | 已有：`Persist`；**rutis-host 缺可编辑层** | 补：`rutis-host run` 把 `rutis.json` 的 `rows` 作为可编辑层，带一个 `Persist` 把行写回 `rutis.json`。写文件用 #805 的 `rutis-host add` 已有的写回（只改 `rows`，其他字段与键的顺序原样），不另写一份。写回后宿主把写出的内容记为"上次读到的"，`--watch-config` 不把它当作变化 |
| 文件在宿主读过之后被改过（手改、`rutis-host add`） | 已有：`persist_queue` 的冲突处理 | 不变：`Persist::load` 读回文件里的 `rows`，loader 对账后重做这次改动。`rows` 以外的字段变了的，照旧要 SIGUSR1 或重启宿主才生效（[运行中改配置](guide/rutis-host.md#运行中改配置)） |
| 在一行自己的 `apply` 里调用 `create` / `reload` / `remove` | loader 的操作等所有行的 `apply` 结束 | 一行在自己的 `apply` 里调 `loader` 的 `create` / `remove` / `reload` 会等到它自己的 `apply` 结束，所以会卡住；`loader` 的方法在 `apply` 返回后调用（服务方法、事件里）；在 `apply` 里挂子插件用 `ctx.plugin`（Rust；Node 里的 Cordis 插件），它属于这一行，随这一行卸载。不加报错或检查 |
| `rutis-host dev` | 读 `rutis.dev.json` | 不给可编辑层：`dev` 是开发一个插件项目，加、删行改 `rutis.dev.json`。在 `dev` 下 `create` / `remove` 得到 §七 的"没有可编辑的配置"错误；`reload`、`status` 照常 |

## 六、本地目录的插件怎样加载

不需要安装，行名直接指向本地的代码（[rutis-host 指南](guide/rutis-host.md)的行名表）：

| 语言 | 行名 |
| --- | --- |
| Node | `./plugins/weather.ts`（相对 `rutis.json` 的插件文件；0.19 起由 Node 运行时自己解析，#813 CL19） |
| Bun | `bun:./weather.ts`（相对 `runtimes.bun.project`） |
| Python | `py:weather`：`runtimes.py.project` 里的模块 `weather.py` 或 `weather/__init__.py`（项目目录在运行时的 `PYTHONPATH` 前面） |

这些行与手写的行完全相同，插件代码不写任何 rutis 特有的东西。

## 七、开发者看到什么

受影响的角色：**插件作者**（写管理插件的插件，例如 agent）与**用 `rutis-host` 组装插件的人**（看终端、看 `rutis.json`）。嵌入 rutis 的宿主持有者：用 `LoaderPlugin` 的宿主多一个服务 `loader`，其余看不到变化。部署的人看不到变化。

### 插件作者

正常时（Python；TypeScript、Go 只是写法不同）。`loader` 的方法在 `apply` 返回后调用（服务方法、事件里），这里放在服务方法 `step` 里；在 `apply` 里挂子插件用 `ctx.plugin`（§五）：

```python
class Maker:
    def __init__(self, loader):
        self.loader = loader

    async def step(self):
        row = await self.loader.create({"id": "weather", "name": "py:weather"})
        await self.loader.reload(row)
        snapshot = await self.loader.status()
        await self.loader.remove(row)


def apply(ctx, config):
    ctx.provide("maker", Maker(ctx.use("loader")))


plugin = define_plugin(apply, name="maker", inject=["loader"])
```

```ts
const loader = ctx.use<Loader>('loader')               // inject: ['loader']
const row = await loader.create({ name: 'py:weather' })   // 在服务方法里
```

```go
var loader struct {                                    // Inject: []string{"loader"}
	Create func(ctx context.Context, options map[string]any) (string, error)
	Reload func(ctx context.Context, id string) error
	Status func(ctx context.Context) (map[string]any, error)
	Remove func(ctx context.Context, id string) error
}
if err := ctx.Use("loader", &loader); err != nil { return err }
row, err := loader.Create(ctx, map[string]any{"name": "py:weather"})   // 在服务方法里
```

| 调用 | 返回 |
| --- | --- |
| `create({"id": "weather", "name": "py:weather"})` | `"weather"` |
| `create({"name": "py:weather"})` | 生成的 id，8 位十六进制，例如 `"3f2a91c0"` |
| `create({"id": "w2", "name": "py:weather", "config": {"units": "metric"}})` | `"w2"` |
| `reload("weather")`（代码修好了） | `None` |
| `remove("weather")` | `None` |
| `status()` | `{"rows": [{"id": "weather", "state": "running"}, …], "services": [{"name": "weather", "provider": "weather"}, …], "calls": […]}`，与 `rutis-host status` 同一份快照 |

出错时：每种情况抛出一个错误，`name` 是 `"Error"`，`code` 是 `"Error"`，`data` 是 `{"row": <行 id>}`（没给 `id` 时是生成的 id），`message` 逐字如下。改法写在最后一列。

| 怎样触发 | 抛出的 `message` | 记在哪一行 | 怎样改 |
| --- | --- | --- | --- |
| `create`，`weather.py` 有语法错误 | `weather: cannot load weather.py: SyntaxError: expected ':' (weather.py:3)` | 不留下这一行 | 改代码再 `create` |
| `create`，`apply` 抛错 | `weather: KeyError: 'units'` | 这一行被退回，不留下 | 改代码或配置再 `create` |
| `create`，配置不合 Schema | `weather: config: units should be "metric" or "imperial", got "kelvin"` | 不留下 | 改 `config` |
| `create`，没有这个模块 | `weather: cannot load "py:wether": ModuleNotFoundError: No module named 'wether'` | 不留下 | 改 `name` |
| `create`，`id` 已存在 | `a row weather already exists` | 原来的行不变 | 换 `id`，或先 `remove` |
| `reload("weather")`，语法错误 | `weather: cannot reload: cannot load weather.py: SyntaxError: expected ':' (weather.py:3); the previous version keeps running` | 原来的版本继续运行 | 改代码再 `reload` |
| `reload("weather")`，`apply` 抛错 | `weather: cannot reload: KeyError: 'units'` | 这一行跑新代码并失败（§五） | 同上 |
| `reload` / `remove` 一个没有的行 | `no row wether` | — | 用 `status()` 里的 id |
| `create` / `remove`，宿主没有可编辑的配置（`rutis-host dev`、不带 `Persist` 的嵌入宿主） | `this host keeps no editable configuration, so rows cannot be added or removed` | — | 在 `rutis.json` / `rutis.dev.json` 里加行 |
| `create` 已生效，但写回 `rutis.json` 失败 | `weather: added, but rutis.json could not be written: <操作系统的原因>` | 行在运行；文件没有它，宿主重启后就没有 | 修好文件权限后再改一次，或手动加行 |

重复的 id 与没有的行由 loader 本身报出，Rust 直接调 `Loader::create` / `reload` / `remove` 拿到同一句（`LoaderError::RowExists`、`UnknownEntry`）。文字按 0.18 §4.5 的禁用词改写：loader 原来的 `no editable layer`、`no entry "x"`、`apply failed, rolled back: …`、`invalid entry: duplicate id "x"` 换成上面的句子；`<行>: cannot reload: …` 与 `dev` 打印的是同一个函数写出的。插件收到的与宿主在终端打印的是同一段文字。

要懂的概念：一个服务 `loader` 与它的 4 个方法；行与行名本来就要懂（`rutis.json`）。不新增概念。

### 用 rutis-host 组装插件的人

终端（标准错误，[run 打印什么](guide/rutis-host.md#run-打印什么)的格式）：

| 插件做的 | 终端 |
| --- | --- |
| `create` 成功 | `weather: running`（或 `weather: waiting for …`，与手写的行相同） |
| `create` 失败 | 两行：先 `fails: failed: KeyError: 'units'`（行起不来），再 `fails: removed`（被退回）；插件收到上表的错误 |
| `reload` 成功，或语法错误（旧版本继续运行） | 不打印：这一行一直是 `running` |
| `reload`，新代码的 `apply` 失败 | `weather: failed: KeyError: 'units'`；改好再 `reload` 后 `weather: running` |
| `remove` | `weather: removed` |

`rutis.json`：`create` 后 `rows` 末尾多一行 `{"id": "weather", "name": "py:weather"}`（`id` 总是写上，其余只写给出的字段），`remove` 后这一行消失；其他字段与键的顺序不变。`rutis-host check rutis.json` 对写回的文件与手写的一样。

名字 `loader` 被占：`rutis.json` 里另有一行提供名叫 `loader` 的服务时，那一行以已有的同名错误失败（[插件 API](guide/plugin-api.md)"一个服务名在 rutis 里只有一个提供者"：`service loader is already provided by …: row <行> cannot provide it too`），终端照旧打印 `<行>: failed …`。改法：给那一行的服务换名。不另加检查或专门的报错。

要懂的概念：`rutis.json` 会被插件改写（只改 `rows`）。

## 八、场景 S1（按本设计更新）

取代 #185 正文里的场景与断言（issue 本身不改）。

用一个脚本化的 Python 插件 `maker` 代替 agent：`inject: ["loader"]`，提供服务 `maker`，`step(n)` 照固定脚本做第 n 步。测试启动 `rutis-host run`，用 `rutis-host call maker.step <n>` 驱动（#670），用 `rutis-host call weather.today '"Oslo"'` 使用新服务。

| 步 | `maker.step(n)` 做的 | 断言 |
| --- | --- | --- |
| 1 | 在 Python 项目里写出 `weather.py`（提供 `weather`，`today` 返回 `"sunny (v1)"`） | 文件存在；`status()` 里没有 `weather` |
| 2 | `loader.create({"id": "weather", "name": "py:weather"})` | 返回 `"weather"`；`rutis.json` 多了这一行；终端 `weather: running` |
| 3 | （测试）`rutis-host call weather.today '"Oslo"'` | `"sunny (v1)"` |
| 4 | 写进语法错误，`loader.reload("weather")` | 抛出的文字以 `weather: cannot reload: cannot load weather.py: SyntaxError` 开头、含 `weather.py`、以 `; the previous version keeps running` 结尾；`weather.today` 仍是 `"sunny (v1)"` |
| 5 | 改成返回 `"rainy (v2)"`，`loader.reload("weather")` | 返回；`weather.today` 是 `"rainy (v2)"` |
| 6 | `loader.remove("weather")`，`status()` | 没有 `weather` 行与服务；`rutis.json` 回到第 1 步时的样子（逐字节）；终端 `weather: removed`；`rutis-host call weather.today '"Oslo"'` 报 `rutis-host: no row provides weather (services: loader, maker)` |

另外：第 4 步期间测试每 100 ms 调一次 `weather.today`，没有一次失败；第 5 步之后全部是 v2；宿主停下后残余检查通过。落点 `tests/e2e/tests/host_writes_plugin.rs`（E2E 框架 #175 已完成）；Linux、macOS，进 nightly。

## 九、兼容与《迁移到 0.19》

- 用 `LoaderPlugin` 的宿主（包括 rutis-host）多了一个服务 `loader`，所有插件都能用。
- 已经有行按名字提供 `loader` 的，与它同名，按已有的同名规则后起来的失败。《迁移到 0.19》两份写一句："`loader` 现在是 rutis 自己提供的服务名；提供同名服务的插件改名"。0.19 不做兼容，不留开关。
- rutis-host：运行中经 `loader` 加、删的行写回 `rutis.json`；文件里其他字段与键的顺序不变。

## 十、实施

三个 PR，各自闭环（替换的东西同一个 PR 删完、文档同步改），跟踪见 #771：

| PR | 内容 | 完成 | 依赖（#771 的行） |
| --- | --- | --- | --- |
| H2 | rutis-host 的可编辑层：`run` 把 `rutis.json` 的 `rows` 作为可编辑层，`Persist` 用 #805 的写回；`dev` 不给；出错文字 `this host keeps no editable configuration, …` 与写回失败一行 | 加、删行后文件逐字节正确、键顺序不变；`--watch-config` 不把写回当作变化；外部改了文件后再改，按冲突处理合并；重启后照常；rutis-host.md "运行中改配置"一节写明 | P2（#805） |
| H3 | rutis-loader 的 `LoaderService` 与 4 个方法，`LoaderPlugin` 提供它；§七 出错文字（替换 `no editable layer`、`no entry`、`apply failed, rolled back` 的旧说法，所有使用方同一个 PR 改完）；`cannot reload` 的文字由 loader 写出，`dev` 改用它 | Node、Python、Go 各一个测试：`inject` 后按同一套调用 `create` / `reload` / `remove` / `status`，逐字比较 §七 插件作者表的每一行；"没有可编辑的配置""写回失败"两行各一个端到端测试；保证表加 `loader/service` | CL5、CL17（含 C2）、CL21、CL23 |
| H4 | S1（§八）；rutis-host.md 与 plugin-api.md 加"用 `loader` 管理插件"一节（中英）；《迁移到 0.19》的一句；关闭 #185 | Linux、macOS 通过，进 nightly | H2、H3；与 G3、G5 改同一批文档，不并行 |

H1 是本次修订（只改两份设计稿，在 #771 加 H2–H4 行）。
