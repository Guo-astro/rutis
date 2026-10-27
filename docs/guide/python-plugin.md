# 写一个 Python 插件

从创建项目到发布，再到被宿主使用。需要 Python 3.10 或更高；下面用 [uv](https://docs.astral.sh/uv/)，用 pip 和 venv 也可以。

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
    ctx.provide("weather", Weather(ctx.use("llm"), config.get("city", "Oslo")))
    return lambda: None                   # 清理（可以是 async），也可以不返回


plugin = define_plugin(
    apply,
    inject=["llm"],                       # 用到的服务：都就绪才启动，任何一个撤销就停下
    provides={"weather": Weather},        # 提供的服务：check 运行前就知道
    config={"type": "object", "properties": {"city": {"type": "string"}}},
)
```

也可以不用 `define_plugin`，直接在模块里写 `apply`、`inject`、`provides`、`Config`。`apply` 可以是 `async def`。

`provides` 写出插件提供什么，`rutis-host check` 不运行插件就知道 `weather` 由这一行提供：宿主在 CI 里用 `check --strict` 检查配置时，用到 `weather` 的行才能通过（模板默认就写了）。`provides={"weather": Weather}` 从类里读方法：不以 `_` 开头的方法，`async def` 为 async，其余为 sync；也可以写 `{"weather": {"today": "async"}}`。不写也能运行：`ctx.provide` 提供的服务照样自动对外，方法取对象上不以 `_` 开头的可调用属性，都按 `async` 调用，只是 `check` 要等插件运行才知道，`--strict` 会报错。

`ctx.use("llm")` 拿到的服务：同一个进程里的插件提供的，就是对象本身；其他语言或其他机器提供的，是代理，方法按声明同步返回或返回可 await 的结果。能做什么、值怎样传递，见 [插件 API](plugin-api.md)。

插件本身也是一个值：把 `define_plugin(...)` 的结果照常作为参数交给一个服务（比如应用的部件登记处），收的一方就能在别处挂它，每挂一次是一份，每一份在这个进程里运行，用的是你在 `apply` 里准备好的东西：

```python
async def apply(ctx, config):
    pool = await open_pool()

    def session(ctx, config):
        ctx.provide("gitStatus", GitStatus(pool, config["depth"]))

    ctx.use("parts").register("session", define_plugin(session, inject=["history"]))
```

这一行卸载、重载时，从它交出的定义挂出的每一份先结束。测试工具里，替身收到的定义可以直接交给 `load` 装上。见 [插件 API](plugin-api.md#插件定义)。

### 带方法的对象

数据（`dict`、`list`、`tuple`、dataclass）复制；其他类的实例按引用交给别的语言：对方拿到代理，调用的是这里的原对象。别的进程交来的对象，这里拿到的也是代理：

```python
cache = await ctx.use("store").cache()
await cache.set("a", 1)          # 方法都是 async，调用的是对方的原对象
value = await cache.get("a")
```

- 代理上任何公开名字都是方法（和 `ctx.use` 一样），没有数据字段：要数据，让方法返回它。`repr` 是 `<remote object>`。
- 同一个对象交出几次，对方拿到同一个代理（`is`）；代理交回原来的进程，那边拿到的是原对象。
- 对象不跟行走：交出它的行卸载后，对象照常可以调用。对方的运行时退出后，对它的调用以 `peer-gone` 失败。
- 调用对象没有的方法，这次调用以 `no-method` 失败：`Cache has no method gett (methods: get, set)`。
- 列表、字典里的 `undefined`（JavaScript 发来的）到这里是 `None`，不论在哪一层。
- 同一个运行时里的行拿到的是对象本身。测试工具的严格模式按跨进程的样子给代理。

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

`load` 会检查宿主会检查的事：插件只用 `inject` 里声明的服务；提供的服务带着 `provides` 里声明的每个方法（没有声明的服务，像 rutis 一样按对象的公开方法、都按 async 调用；`t.methods(name)` 返回 rutis 看到的方法）；退出 `async with` 时清理函数都运行了。默认的严格模式下，值按跨进程的规则传递：数据被复制（dataclass 变成 dict），函数和带方法的对象按引用传递，`sync` 方法不能返回协程。只在同一进程里才成立的写法，在这里就会失败。

测试工具的全部接口（取自 `rutis/testing.py` 的文档字符串）：

| `load(plugin, *, …)` 的参数 | 作用 |
| --- | --- |
| `config` | 插件的配置；有 `config` 声明时补默认值并校验 |
| `services` | 插件 `inject` 的服务，按名字给 |
| `strict` | 值按跨进程的规则传递（默认 `True`） |
| `bus` | 与同一个 `Bus` 上的插件共用事件（默认自己一个） |
| `scope` | 加载到哪个作用域路径（`t1/s1`，默认根） |
| `row` | 在总线上的行名（默认插件声明的名字，否则模块名） |
| `runtime` | 监听器描述里的 `runtime`（默认 `"python"`） |
| `deadline` | 卸载时等正在运行的监听器多少秒（默认 `RUTIS_SHUTDOWN_TIMEOUT`，10） |

| `Loaded`（`async with load(...) as t`）的成员 | 作用 |
| --- | --- |
| `t.service(name)` | 插件提供的服务，按 rutis 看到的样子：声明的方法，没有声明时是对象的公开方法，都按 async |
| `t.methods(name)` | rutis 看到的方法：`{"today": "async"}` |
| `t.provided()` | 插件现在提供的服务名 |
| `t.on(name, listener, *, prepend=False, global_=False)` | 像宿主一样在根作用域监听：插件发出的、测试自己发出的都送到；严格模式下参数像从另一个进程收到的；返回撤销函数，不撤销时随插件卸载撤掉 |
| `t.emit`、`t.parallel`、`t.serial`、`t.waterfall`、`t.bail` | 从插件所在的行和作用域发出事件 |
| `t.filter(predicate)` | 带这五个方法的对象，只送达 `predicate(descriptor)` 留下的监听器 |
| `t.reported` | 没人接收的失败（`emit` 里出错的监听器、过滤函数） |
| `await t.update(config)` | 像宿主的 `rows.update` 那样换配置：卸载后用新配置再运行 |
| `await t.unload()` | 像运行时那样卸载：撤下监听器和服务，取消并等待正在运行的监听器，再按倒序运行清理（退出 `async with` 时自动做） |

`Bus(scopes=...)`：一条测试用的总线，同一个 `bus` 上的插件互相听得到；`scopes` 给作用域路径起组名。它的 `reported`、`listeners(name)`（过滤函数看到的描述）、`emitter(row=..., strict=...)`（以某一行的身份发出）。`Reported`：没人接收的一次失败，字段 `kind`（`"listener"` 或 `"filter"`）、`name`、`row`、`error`。`PluginTestError`：插件做了宿主会拒绝或跨进程会出错的事，是 `AssertionError` 的子类。

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

`ctx.on(name, listener)` 监听事件；`ctx.emit`、`await ctx.parallel`、`await ctx.serial`、`await ctx.waterfall`、`ctx.bail` 是发出的五种方式。监听器自动登记给宿主，什么都不用声明：发出方在同一个进程、另一种语言的运行时、宿主里还是相连的节点上，结果都一样。完整的规则见 [插件 API：事件](plugin-api.md#事件)。

下面两个插件在同一个 `Bus` 上：`greeter` 用 `serial` 取第一个结果、用 `waterfall` 让别的插件改写问候语、用 `emit` 通知；`shout` 是 waterfall 的一环，并监听通知。

<!-- example: python file=weather/tests/test_events.py order=2 -->
```python
import asyncio
import unittest

from rutis import define_plugin
from rutis.testing import Bus, load


def greeter_apply(ctx, config):
    # 返回 None 或 False 是"没有结果"，serial 接着问下一个监听器
    ctx.on("greeting/pick", lambda lang: "Hej" if lang == "sv" else None)

    class Greeter:
        async def hello(self, name, lang):
            word = await ctx.serial("greeting/pick", lang) or "Hello"
            # terminal：链的最里面，收到事件参数
            text = await ctx.waterfall("greeting/format", f"{word}, {name}", terminal=lambda text: text)
            ctx.emit("greeting/said", name)      # 立即返回，不等监听器
            return text

    ctx.provide("greeter", Greeter())


greeter = define_plugin(greeter_apply)
said = []


def shout_apply(ctx, config):
    async def exclaim(text, next):           # waterfall 的监听器最后一个参数是 next
        return f"{await next()}!"

    ctx.on("greeting/format", exclaim)
    ctx.on("greeting/said", said.append)


shout = define_plugin(shout_apply)


class Events(unittest.TestCase):
    def test_plugins_hear_each_other(self):
        async def go():
            bus = Bus()                          # 同一个 bus 上的插件互相听得到
            async with load(greeter, bus=bus, row="greeter") as t, load(shout, bus=bus, row="shout"):
                self.assertEqual(await t.service("greeter").hello("Ada", "sv"), "Hej, Ada!")
                self.assertEqual(await t.service("greeter").hello("Bo", "en"), "Hello, Bo!")
                self.assertEqual(said, ["Ada", "Bo"])
                self.assertEqual(await t.serial("greeting/pick", "sv"), "Hej")  # 测试自己也能发出

        asyncio.run(go())
```

- `emit` 立即返回；监听器出错不影响发出方和别的监听器，在监听器所在处报告一行（测试里在 `t.reported`）。要等全部监听器、拿到它们的错误，用 `await ctx.parallel(...)`：失败是 `rutis.AggregateError`，`errors` 按登记顺序。
- `bail` 是同步的 `serial`：遇到 `async def` 监听器（或返回协程的函数）以 `rutis.AsyncListener` 失败。
- `ctx.filter(lambda d: d["row"] == "audit")` 返回带这五个方法的对象，只送达过滤函数留下的监听器；`ctx.on(name, fn, prepend=True)` 排到最前，`global_=True` 不受过滤。
- 作用域：`load(plugin, bus=bus, scope="t1/s1")` 把插件加载到一个实例里（`Bus(scopes={"t1": "tenant"})` 给出组名）；监听器收到这个 root 总线上的所有同名事件；作用域只是描述，供过滤使用。
- 卸载时正在运行的监听器的任务被取消并被等待（最多 `RUTIS_SHUTDOWN_TIMEOUT`），然后才运行清理。发出方放弃时（它的任务被取消），进行中的监听器同样被取消。

**跨语言**：TypeScript 插件发出、Python 插件监听，什么都不用改。宿主的 `rutis.json`：

```json
{
  "runtimes": { "node": { "project": "web" }, "py": { "project": "plugins" } },
  "rows": [
    { "id": "sessions", "name": "sessions" },
    { "id": "audit", "name": "py:audit" }
  ]
}
```

`web/` 里的 TypeScript 插件 `sessions` 发出事件，并用 `serial` 问别人要配额：

```ts
ctx.provide('sessions', {
  async create(user: string) {
    const quota = (await ctx.serial<number>('session/quota', user)) ?? 1
    const id = crypto.randomUUID()
    await ctx.parallel('session/created', id, user, quota)   // 等每个监听器，不论在哪种语言
    return id
  },
})
```

`plugins/audit.py` 回答配额、记录创建：

```python
import sys

from rutis import define_plugin


def apply(ctx, config):
    ctx.on("session/quota", lambda user: 10 if user == "ada" else None)

    async def created(session_id, user, quota):
        print(f"session {session_id} for {user}, quota {quota}", file=sys.stderr)

    ctx.on("session/created", created)


plugin = define_plugin(apply)
```

参数按[值怎样传递](plugin-api.md#值怎样传递)跨进程；Python 监听器抛出的错误在 TypeScript 一侧是同名的错误（`parallel` 里是 `AggregateError` 的子错误）。

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

对 `rutis` 的依赖只写下限：`rutis>=0.17`（`rutis-host new` 写的是它自己的版本）。插件和 Python 运行时是同一个 `rutis` 包，装在宿主的同一个环境里；写了上限（`<0.18`），宿主装插件时 rutis 会被降级，或者以后升级 rutis 时解析失败。能不能加载由插件 API 版本决定：`define_plugin` 在插件上标记它所用的插件 API 版本，宿主的运行时比它旧时明确报错，比它新时照常加载；插件 API 版本不变时，`rutis` 不删、不改插件导入的名字（[插件 API：版本](plugin-api.md#版本)）。

发布前给分发包起一个你自己的名字：`weather` 这样的短名字在 PyPI 上多半已被占用。改 `pyproject.toml` 的 `name`，例如 `your-name-weather`；入口点名（上面的 `weather`）不用改，宿主的行名仍是 `py:weather`。

## 6. 被宿主使用

宿主把插件装进它运行 Python 插件的环境（rutis.json 里 `runtimes.py.python` 指向的解释器；默认是激活的虚拟环境的，再是 `<runtimes.py.project>/.venv/bin/python`），然后加一行 `py:<入口点名>`。环境还没有时先建一个。Python 运行时就是这个环境里的 `rutis`，装成和宿主的 rutis-host 相同的版本（`rutis-host --version`）：

```bash
uv venv                                                         # 建 .venv
uv pip install --python .venv/bin/python "rutis==0.17.2" your-name-weather   # 运行时（宿主的版本）和插件
```

```json
{ "id": "weather", "name": "py:weather", "config": { "city": "Oslo" } }
```

见 [rutis-host 与 rutis.json](rutis-host.md#宿主怎样装插件)；Rust 宿主见 [在 Rust 应用里嵌入](rust-host.md)。
