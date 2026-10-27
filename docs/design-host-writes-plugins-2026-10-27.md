# 宿主自己写插件：把 loader 作为各语言插件都能用的服务

[English](design-host-writes-plugins-2026-10-27.en.md) · [中文](design-host-writes-plugins-2026-10-27.md)

状态：草案，未实现。日期：2026-10-27。基准：`main` `103c0d22`。排期：0.19（只做设计）；实现在 0.19 之后。
Issue：[#185](https://github.com/arcships/rutis/issues/185)（含并入的 [#693](https://github.com/arcships/rutis/issues/693)）。取代 [#505](https://github.com/arcships/rutis/pull/505) 的草案。
与 0.18 的关系：状态用 [0.18 的诊断](design-diagnostics-2026-10-26.md) §五 的 `Loader::status()`，出错文字按它 §四；不加结构。

范围外：从包仓库安装（#691 的 `rutis-host add` 与嵌入 `Installer`，归宿主）；`update`、`set_disabled`、`move_to`、`check` 等其余 `Loader` 方法（§三，S1 用不到）；任何权限层（宿主要限制时用 #683 的 `limits.services`）；agent 的工具、提示、批准（agent 产品的事）；一行对自己 `reload` / `remove`（会等它自己正在进行的调用，S1 用不到）；Windows 上的 S1（#670 的 `call` 不在 Windows 上）。

## 一、要做成什么，现在卡在哪

[设计哲学](design-philosophy.md) §1：宿主需要接触什么，就自己写一个插件接上去，常常由 agent 来写；试用、修改、替换。

场景：宿主里有一个 Python 插件（例如一个 agent）。它要写一个新插件 `weather`、让宿主加载它、改坏了知道错在哪而旧版本继续服务、修好后换上、不要了删掉。

现状（`crates/rutis-loader/src/loader/`）：

- `LoaderPlugin::apply`（`plugins.rs`）已经 `ctx.provide(self.loader.clone())`。**Rust 插件**从 ctx 拿到 `Loader`，就能用 `create`、`update`、`set_disabled`、`remove`、`reload`、`entries`、`get`、`check`（`api.rs`、`check.rs`）管理插件。
- 编辑全有或全无（`commit.rs` 的 `commit`）：改写可编辑层 → 试运行（解析并导入新模块）→ 对账；有行新失败就退回原来的层、再对账一次，返回错误。改动经 `Persist` 存下。
- `reload(id)`：重新解析这一行的模块，试运行，换上；失败时保留旧模块，旧版本继续服务。
- `check(&layers)`：不运行任何东西，检查一份候选配置（每行能解析、过试运行、依赖都有人提供、没有依赖环、没有同名提供）。

缺的两处：

1. `Loader` 只按 Rust 类型提供，**没有按名字提供**，其他语言的插件拿不到。
2. **rutis-host 没有可编辑层**：它用 `reconcile(layers, None)`（`crates/rutis-host/src/host.rs`）把 `rutis.json` 交给 loader，所以在 rutis-host 里调 `create` / `remove` 会得到 `no editable layer`。

## 二、Cordis 的做法

Cordis 4 的 loader（dsh 带的 `@deepseek-ai/cordis-plugin-loader`）把自己作为服务 `loader` 提供（`ctx.reflect.provide('loader', …)`）。插件写 `inject: ['loader']`，然后用 `ctx.loader.create(options, parent, position)`、`ctx.loader.update(id, options, parent?, position?)`、`ctx.loader.remove(id)`（`config/tree.ts`）。每次改动写回配置文件（`tree.write()`）。没有权限层：任何插件都能用。重新加载代码由另一个插件（HMR）负责，不在 loader 上。

rutis 照这个做：同一个名字、同样的方法形状、同样默认谁都能用；只多一个 `reload`，因为 rutis 的代码重载本来就在 loader 上（`dev` 用的就是它）。

## 三、决策

| # | 决策 |
| --- | --- |
| L1 | `LoaderPlugin` 再按名字提供一个服务 `loader`，走 0.17 起已有的路径：一个标了 `#[rutis::service(name = "loader")]` 的类型，`ctx.provide` 时自动对外。不另开通道 |
| L2 | 方法只放 S1 用得到的 4 个，名字与参数贴近 Cordis 的 `ctx.loader`（下表） |
| L3 | 默认任何插件都能 `inject: ["loader"]`，与 Cordis 相同；宿主要限制，用 #683 的 `limits.services`。不另做权限 |
| L4 | 检查与替换只用 loader 已有的行为（§五）；补的只有 rutis-host 的可编辑层 |
| L5 | 本地目录的插件直接写进行名就能加载，不需要安装；从包仓库安装归 #691，不进 `loader` 服务 |

### 方法

| `loader` 的方法 | 参数 | 结果 | 对应的 Rust `Loader` | 对应的 Cordis `ctx.loader` |
| --- | --- | --- | --- | --- |
| `create(options, parent?, position?)` | `options`：`{id?, name, config?, disabled?, inject?}`，与 `rutis.json` 一行的写法相同 | 行 id | `create(NewEntry, parent, position)` | `create(options, parent, position)` |
| `reload(id)` | 行 id | 无 | `reload(id)` | 没有：Cordis 的代码重载在 HMR 插件里；rutis 的在 loader 上 |
| `remove(id)` | 行 id | 无 | `remove(id)` | `remove(id)` |
| `status()` | 无 | 0.18 的快照：行与状态、原因，服务与提供者，运行时，进行中的调用 | `status()`（0.18） | 没有对应方法（Cordis 插件直接读 `loader` 的条目对象）；跨进程拿不到对象，给快照 |

- 失败时抛出错误：消息是 rutis 打印给人看的那段文字（0.18 §四：哪一行、为什么、文件与行号），`data` 是 `{row}`。不加新的错误码。
- `update`、`set_disabled`、`move_to`、`set_inject`、`entries`、`get`、`check` 照旧只给 Rust；有使用方时按同样的方式加（Cordis 的 `update` 同时管配置和移动，加的时候照它）。

## 四、怎样跨语言提供

- rutis-loader 加一个类型 `LoaderService`（包着 `Loader`），标 `#[rutis::service(name = "loader")]`，方法是上表的 4 个 `async fn`，参数、结果都是 serde 类型。`LoaderPlugin::apply` 在已有的 `ctx.provide(self.loader.clone())` 旁边再 `ctx.provide(LoaderService(..))`。按 0.17 的规则（[Rust 服务自动对外](design-rust-service-exposure-2026-10-21.md)），标了的类型被提供时自动按名字对外，Node、Bun、Python、Go、Swift、Kotlin 的插件都能 `inject` 它；Rust 插件照旧直接拿 `Loader`。
- 为什么另起一个类型：`Loader` 已有的同名方法（`create(NewEntry, …)`）参数是 Rust 类型，不能直接给其他语言；宏不支持给方法改名，所以用一个薄的包装类型。
- 只在宿主按名字共享服务时可见（rutis-host 是 `share_by_name`）。用 `share_registered_only` 的宿主（例如 rutis-dsh：它的 Cordis 有自己的 `loader`）不受影响。

## 五、检查与替换：已有的和缺的

| 要的 | 现状 | 本稿 |
| --- | --- | --- |
| `create` 前检查 | 已有：`commit` 先试运行（解析、导入新模块；语法、导入错误在这里报出） | 不变 |
| `create` 失败 | 已有：有行新失败就**退回**原来的层，不留下失败的行，返回错误 | 不变；错误的文字按 0.18 |
| `reload` 失败时旧版本继续服务 | 已有：`reload` 全有或全无，失败时保留旧模块 | 不变。已知边界：新代码在 `apply` 里失败时，旧版本先卸下再装回，中间短暂不可用（语法、导入错误在试运行就拦下，不会走到这里） |
| `remove` | 已有：卸载这一行，每项清理执行一次；运行时进程不因此退出（它可能还有别的行） | 不变 |
| 应用前检查一份候选配置 | 已有：`Loader::check` | 不放进服务（S1 用不到） |
| 改动存下 | 已有：`Persist`；**rutis-host 缺可编辑层** | 补：rutis-host 把 `rutis.json` 的 `rows` 作为可编辑层，带一个 `Persist` 把行写回 `rutis.json`（只改 `rows`，其他字段原样）。写回的内容就是重读配置会得到的，`--watch-config` 下次读到时不算变化 |

## 六、本地目录的插件怎样加载

不需要安装，行名直接指向本地的代码（[rutis-host 指南](guide/rutis-host.md)的行名表）：

| 语言 | 行名 |
| --- | --- |
| Node | `./plugins/weather.ts`（相对 `rutis.json` 的插件文件） |
| Bun | `bun:./weather.ts`（相对 `runtimes.bun.project`） |
| Python | `py:weather`：`runtimes.py.project` 里的模块 `weather.py` 或 `weather/__init__.py`（项目目录在运行时的 `PYTHONPATH` 前面） |

这些行与手写的行完全相同，插件代码不写任何 rutis 特有的东西。

## 七、开发者看到什么

插件作者（Python 示意）：

```python
plugin = define_plugin(name="maker", inject=["loader"], apply=apply)

async def apply(ctx, config):
    loader = ctx.use("loader")
    row = await loader.create({"name": "py:weather"})
    await loader.reload(row)
    await loader.remove(row)
```

| 调用 | 结果 |
| --- | --- |
| `create({"name": "py:weather"})` | `"weather"`（没给 `id` 时生成）；`rutis.json` 多一行；终端 `weather: running` |
| 同上，`weather.py` 有语法错误 | 抛出：`weather: cannot load weather.py: SyntaxError: invalid syntax (weather.py, line 3)`；`rutis.json` 不变 |
| 同上，`apply` 出错 | 抛出：`weather: failed in apply: KeyError: 'units'` + `weather.py:12 in apply`；这一行被退回 |
| 同上，`id` 已存在 | 抛出：`a row weather already exists` |
| `reload("weather")`，语法错误 | 抛出：`weather: cannot reload: SyntaxError: invalid syntax (weather.py, line 3); the previous version keeps running` |
| 同上，修好 | 返回；终端 `weather: reloaded` |
| `remove("weather")` | 返回；`rutis.json` 少这一行；终端 `weather: removed` |
| `reload("wether")` | 抛出：`no row wether` |
| 嵌入宿主没有可编辑层时 `create` | 抛出：`this host keeps no editable configuration, so rows cannot be added or removed` |
| `status()` | `{rows: [{id: "weather", state: "running"}, …], services: [{name: "weather", provider: "weather"}, …], runtimes: […], calls: […]}` |

文字按 0.18 §4.5 的禁用词改写（现在的 `no editable layer` 等 loader 内部的说法换成上面的句子）。宿主持有者在终端看到的与插件收到的同一段文字。

## 八、场景 S1（按本设计更新）

取代 #185 正文里的场景与断言（issue 本身不改）。

用一个脚本化的 Python 插件 `maker` 代替 agent：`inject: ["loader"]`，提供服务 `maker`，`step(n)` 照固定脚本做第 n 步。测试启动 `rutis-host run --allow-call`（#670），用 `rutis-host call maker.step <n>` 驱动，用 `rutis-host call weather.today '"Oslo"'` 使用新服务。

| 步 | `maker.step(n)` 做的 | 断言 |
| --- | --- | --- |
| 1 | 在 Python 项目里写出 `weather.py`（提供 `weather`，`today` 返回 `"sunny (v1)"`） | 文件存在；`status()` 里没有 `weather` |
| 2 | `loader.create({"id": "weather", "name": "py:weather"})` | 返回 `"weather"`；`rutis.json` 多了这一行 |
| 3 | （测试）`rutis-host call weather.today '"Oslo"'` | `"sunny (v1)"` |
| 4 | 写进语法错误，`loader.reload("weather")` | 抛出的文字以 `weather: cannot reload: SyntaxError` 开头、含 `weather.py`；`weather.today` 仍是 `"sunny (v1)"` |
| 5 | 改成返回 `"rainy (v2)"`，`loader.reload("weather")` | 返回；`weather.today` 是 `"rainy (v2)"` |
| 6 | `loader.remove("weather")`，`status()` | 没有 `weather` 行与服务；`rutis.json` 回到第 1 步时的样子；`weather.today` 报 `no row provides weather` |

另外：第 4 步期间测试每 100 ms 调一次 `weather.today`，没有一次失败；第 5 步之后全部是 v2；宿主停下后残余检查通过。落点 `tests/e2e/tests/host_writes_plugin.rs`（E2E 框架 #175 已完成）；Linux、macOS，进 nightly。

## 九、兼容

- 新增，默认可用，与 Cordis 一致：所有共享名字的宿主多了一个服务 `loader`。
- 已经有插件或宿主按名字提供 `loader` 的，会与它同名冲突（后提供的那个失败，已有的同名规则）。这一条写进《迁移到 0.19》：改名，或用 `share_registered_only`。
- rutis-host：运行中经 `loader` 加、删的行写回 `rutis.json`；文件里其他字段不变。

## 十、实施

实现在 0.19 之后，一个 PR：

| 内容 | 完成 |
| --- | --- |
| `LoaderService` 与 4 个方法、`LoaderPlugin` 提供它；出错文字按 0.18 改写 | Node、Python、Go 各一个测试：`inject` 后 `create` / `reload` / `remove` / `status`，以及 §七 每一行的文字 |
| rutis-host 的可编辑层与写回 `rutis.json` 的 `Persist` | 加、删行后文件正确、重读不算变化、重启后照常 |
| S1（§八）；rutis-host.md 与插件 API 指南加"用 `loader` 管理插件"一节（中英） | Linux、macOS 通过 |

依赖：0.18 的 `Loader::status()`（#678）与出错文字（#677）；S1 用 #670 的 `call`。不依赖 #683（宿主要限制时才用它的 `limits.services`）与 #691。

## 十一、自审：还有没有没有使用方的东西

- 4 个方法：`create`、`reload`、`remove` 是 S1 的第 2、4–5、6 步；`status` 是第 1、6 步。其余 `Loader` 方法不放。
- 权限、命名运行时、默认限制、管理服务、工具注册、批准、排队、版本检查：都没有使用方，不做。
- rutis-host 的可编辑层：没有它 S1 在 rutis-host 里做不成，保留。
