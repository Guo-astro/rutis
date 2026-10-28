# 写一个 Python 插件

从创建项目到发布，再到被宿主使用。需要 Python 3.12 或更高；下面用 [uv](https://docs.astral.sh/uv/)，用 pip 和 venv 也可以。

## 1. 创建项目

<!-- example: python run -->
```bash
uvx rutis-host new weather --lang python
cd weather
uv sync
```

得到的项目：

| 文件 | 作用 |
| --- | --- |
| `src/weather/__init__.py` | 插件本身 |
| `tests/test_plugin.py` | 单元测试，不需要宿主 |
| `rutis.dev.json` | 本地运行时的配置和测试用的其他插件 |
| `pyproject.toml` | 依赖 `rutis`（只写下限，见 §5）；开发依赖 `rutis-host`；入口点 `rutis.plugins` |
| `.github/workflows/publish.yml` | 打 `v*` tag 时测试并发布到 PyPI |

## 2. 写插件

生成的 `src/weather/__init__.py` 是一个不用任何服务的 `greeter`，`tests/test_plugin.py` 是它的测试。下面把它换成一个用到 `llm` 服务的 `weather`：`__init__.py` 换成这段，`tests/test_plugin.py` 同时换成 §3 的那段（两个文件一起换）。

<!-- example: python file=weather/src/weather/__init__.py -->
```python
from rutis import define_plugin


class Weather:
    def __init__(self, llm, city):
        self.llm, self.city = llm, city

    async def today(self):                # async def：异步方法
        return f"{await self.llm.ask(f'weather in {self.city}')} in {self.city}"


def apply(ctx, config):
    ctx.provide("weather", Weather(ctx.use("llm"), config["city"]))
    return lambda: None                   # 清理（可以是 async），也可以不返回


plugin = define_plugin(
    apply,
    inject=["llm"],                       # 用到的服务：都就绪才启动，任何一个撤销就停下
    provides={"weather": Weather},        # 提供的服务：check 运行前就知道
    config={"type": "object", "properties": {"city": {"type": "string", "default": "Oslo"}}},
)
```

`apply` 之前，rutis 给配置补上 Schema 里的默认值（行里不写 `city` 时就是 `Oslo`）并校验：配置不合格的行以 `config: …` 失败，不调用 `apply`（[插件 API：配置](plugin-api.md#配置)）。`rutis.testing` 的 `load`、`update` 也一样，不合格时抛出 `BindingError`，`code` 是 `bad-config`；多余的键写到标准错误。

也可以不用 `define_plugin`，直接在模块里写 `apply`、`inject`、`provides`、`Config`。`apply` 可以是 `async def`。

`provides` 写出插件提供什么，`rutis-host check` 不运行插件就知道 `weather` 由这一行提供：宿主在 CI 里用 `check --strict` 检查配置时，用到 `weather` 的行才能通过（模板默认就写了）。`provides={"weather": Weather}` 从类里读方法：不以 `_` 开头的方法，`async def` 为 async，其余为 sync；也可以写 `{"weather": {"today": "async"}}`。不写也能运行：`ctx.provide` 提供的服务照样自动对外，方法取对象上不以 `_` 开头的可调用属性，都按 `async` 调用，只是 `check` 要等插件运行才知道，`--strict` 会报错。

`ctx.use("llm")` 拿到的服务：同一个进程里的插件提供的，就是对象本身；其他语言或其他机器提供的，是代理，方法按声明同步返回或返回可 await 的结果。能做什么、值怎样传递，见 [插件 API](plugin-api.md)。

插件本身也是一个值，可以交给服务、在别处挂很多次：见 [插件 API：插件定义](plugin-api.md#插件定义)。

### 带方法的对象

数据（`dict`、`list`、`tuple`、dataclass）复制；其他类的实例按引用交给别的语言，对方拿到代理：见 [插件 API：带方法的对象](plugin-api.md#带方法的对象)。

## 3. 测试

`rutis.testing` 不需要宿主：

<!-- example: python file=weather/tests/test_plugin.py -->
```python
import asyncio
import unittest

from rutis.testing import load
from weather import plugin


class FakeLlm:
    async def ask(self, question):
        return "sunny"


class Weather(unittest.TestCase):
    def test_reports_the_weather_of_the_configured_city(self):
        async def go():
            async with load(plugin, config={"city": "Oslo"}, services={"llm": FakeLlm()}) as t:
                self.assertEqual(await t.service("weather").today(), "sunny in Oslo")

        asyncio.run(go())
```

<!-- example: python run -->
```bash
uv run python -m unittest discover -s tests
```

`load` 会检查宿主会检查的事：插件只用 `inject` 里声明的服务；不提供 `services` 里已有的名字（那是宿主提供的：`PluginTestError: wordcount: notes is already provided by the host`）；提供的服务带着 `provides` 里声明的每个方法（没有声明的服务，像 rutis 一样按对象的公开方法、都按 async 调用；`t.methods(name)` 返回 rutis 看到的方法）；退出 `async with` 时清理函数都运行了。默认是严格模式，值按跨进程的规则传递，见 [插件 API：严格模式](plugin-api.md#严格模式)。

测试工具的其余接口（`Bus`、作用域、`update` 等）见 [插件 API：Python 的测试工具](plugin-api.md#python-的测试工具)。

测试要确认插件发出了某个事件，用 `t.on` 加一个列表：

<!-- example: python file=weather/tests/test_asked.py order=2 -->
```python
import asyncio
import unittest

from rutis import define_plugin
from rutis.testing import load


def apply(ctx, config):
    class Weather:
        async def today(self):
            ctx.emit("weather/asked", config["city"])
            return "sunny"

    ctx.provide("weather", Weather())


asking = define_plugin(apply)


class Asked(unittest.TestCase):
    def test_each_call_emits_weather_asked(self):
        async def go():
            async with load(asking, config={"city": "Oslo"}) as t:
                asked = []
                off = t.on("weather/asked", asked.append)
                await t.service("weather").today()
                self.assertEqual(asked, ["Oslo"])
                off()

        asyncio.run(go())
```

### 事件

`ctx.on(name, listener)` 监听事件，`ctx.emit(name, *args)` 发出事件（上面的 `test_asked.py`）。监听器自动登记给宿主，什么都不用声明：发出方在同一个进程、另一种语言的运行时、宿主里还是相连的节点上，结果都一样，TypeScript 插件发出、Python 插件监听，什么都不用改。其余发出方式（`parallel`、`serial`、`waterfall`、`bail`）、过滤、作用域和跨语言的例子见 [插件 API：事件](plugin-api.md#事件)。

## 4. 在本地宿主里运行

<!-- example: python run until="weather: running" order=99 -->
```bash
uv run rutis-host dev
```

`dev` 用项目的 `.venv` 运行这个插件，改了它的文件就重新导入它；只有文件变了的行重新加载，`rutis.dev.json` 改了也在运行中生效（见 [用 dev 开发插件](rutis-host.md#用-dev-开发插件)）。插件需要的其他服务写在 `rutis.dev.json` 里，例如一个假的 `llm`（行名 `py:fake_llm` 指项目 `src/` 里的模块 `fake_llm`，内容在后面）：

<!-- example: python file=weather/rutis.dev.json -->
```json
{
  "rows": [
    { "id": "weather", "config": { "city": "Oslo" } },
    { "id": "llm", "name": "py:fake_llm" }
  ]
}
```

<!-- example: python file=weather/src/fake_llm.py -->
```python
# src/fake_llm.py
from rutis import define_plugin


class FakeLlm:
    async def ask(self, question):
        return "sunny"


def apply(ctx, config):
    ctx.provide("llm", FakeLlm())


plugin = define_plugin(apply, provides={"llm": FakeLlm})
```

`dev` 从项目的 `src/` 找插件模块，所以 `fake_llm` 不需要入口点，也不在构建出的 wheel 里（`pyproject.toml` 只打包 `src/weather`）。

和插件 id 相同的那一行给插件本身加配置；其他行是一起运行的插件，也可以是 TypeScript 插件（这时 `rutis.dev.json` 里要加 `"runtimes": { "node": { "project": "." } }`）。`uv run rutis-host check` 列出每一行的依赖、提供的服务和配置 Schema，有问题时以非零状态退出。

只重新导入插件模块本身；它导入的其他模块改了，要重启 `rutis-host dev`。

### 调用一个方法

`dev` 启动后和每次重新加载后，打印现在有哪些服务、各由哪一行提供：`rutis-host dev: services: llm (llm), loader (host), weather (weather)`。

`dev` 运行时，另开一个终端，在项目目录里调用插件提供的服务，看它返回什么、出错时报什么，不用另写一个调用它的插件：

```console
$ uv run rutis-host call weather.today
"sunny in Oslo"
```

每个参数是一个 JSON 值（字符串带引号：`'"Oslo"'`），结果打印成 JSON；错误写到标准错误并以 1 退出。见 [rutis-host：调用一个服务](rutis-host.md#调用一个服务)。

### 调试

`--debug` 让 Python 运行时读这个终端：插件里的 `breakpoint()` 停在 pdb，在终端里用 `w` 看调用栈、`p <变量>` 看变量、`c` 继续。

<!-- example: python run until="weather: running" order=100 -->
```bash
uv run rutis-host dev --debug
```

- 停在断点时整个 Python 运行时停着：这个运行时里其他行的调用也等它继续。
- 不加 `--debug` 时运行时没有标准输入，`breakpoint()` 让这一行失败。
- 运行时和 `dev` 共用终端，Ctrl-C 也送到运行时：它直接退出，插件的清理可能来不及运行。
- 插件 `print` 的内容和写到标准错误的内容出现在 `dev` 的输出里。
- 用图形界面的调试器（VS Code 等）：在项目环境里装 `debugpy`，在 `apply` 里调用 `debugpy.listen(5678)`，再用调试器连 `127.0.0.1:5678`。
- 测试（§3）在测试进程里运行插件，`breakpoint()` 不用 `--debug` 也能用。

## 5. 发布

```bash
uv build
uv publish
```

或者推一个 `v0.1.0` 这样的 tag，模板里的工作流会测试、`check` 并用 PyPI 的 trusted publishing 发布（先在 PyPI 上为仓库配置 trusted publisher）。

`pyproject.toml` 里的入口点让宿主按名字找到插件：

```toml
[project.entry-points."rutis.plugins"]
weather = "weather"
```

对 `rutis` 的依赖只写下限：`rutis>=0.17`（`rutis-host new` 写的是它自己的版本）。插件和 Python 运行时是同一个 `rutis` 包，装在宿主的同一个环境里；写了上限（`<0.18`），宿主装插件时 rutis 会被降级，或者以后升级 rutis 时解析失败。能不能加载由插件 API 版本决定：`define_plugin` 在插件上标记它所用的插件 API 版本，宿主的运行时比它旧时明确报错，比它新时照常加载；`rutis` 删掉或移动插件导入的名字时，那一版的迁移指南写明，插件随宿主一起升级（[插件 API：版本](plugin-api.md#版本)）。

发布前给分发包起一个你自己的名字：`weather` 这样的短名字在 PyPI 上多半已被占用。改 `pyproject.toml` 的 `name`，例如 `your-name-weather`；入口点名（上面的 `weather`）不用改，宿主的行名仍是 `py:weather`。

## 6. 被宿主使用

宿主怎样装这个插件见 [rutis-host：宿主怎样装插件](rutis-host.md#宿主怎样装插件)。

### 宿主给的类型

Rust 宿主可以给出它的服务的 Python 类型（`rutis-host types --python` 写出的模块，见 [在 Rust 应用里嵌入：声明](rust-host.md#声明)），通常放在插件模板里。用它标注 `ctx.use` 的结果，编辑器和 pyright 就在运行之前检查调用；不标注也照样运行：

```python
from deskmate_types import Notes  # 宿主给的模块（rutis-host types --python deskmate_types.py 写出）


def apply(ctx, config):
    notes: Notes = ctx.use("notes")
    ...
```

调用不符时 pyright 报：

```text
error: Argument of type "Literal[42]" cannot be assigned to parameter "value" of type "str" in function "put"
```
