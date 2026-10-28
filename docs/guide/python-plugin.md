# 写一个 Python 插件

从起项目到发布，八步。例子是模板生成的 `greeter`：它提供 `greeter` 服务，用别的插件提供的 `translate` 服务把问候语翻译出来。需要 Python 3.12 或更高；下面用 [uv](https://docs.astral.sh/uv/)，用 pip 和 venv 也可以。

## 1. 起项目

<!-- example: python run -->
```bash
uvx rutis-host new greeter --lang python
cd greeter
uv sync
uv run python -m unittest discover -s tests
```

得到的项目：

| 文件 | 作用 |
| --- | --- |
| `src/greeter/__init__.py` | 插件本身 |
| `tests/test_plugin.py` | 测试，不需要宿主 |
| `rutis.dev.json` | 本地运行时的行（第 7 步） |
| `pyproject.toml` | 依赖 `rutis`；开发依赖 `rutis-host`；入口点 `rutis.plugins` |
| `.github/workflows/publish.yml` | 打 `v*` tag 时测试并发布到 PyPI（第 8 步） |

## 2. 提供服务

生成的 `src/greeter/__init__.py`：

<!-- example: python file=greeter/src/greeter/__init__.py -->
```python
"""greeter: a rutis plugin."""

# Guide: https://github.com/arcships/rutis/blob/main/docs/guide/python-plugin.en.md

from rutis import define_plugin


class Greeter:
    def __init__(self, greeting: str) -> None:
        self.greeting = greeting

    async def hello(self, name: str) -> str:
        return f"{self.greeting}, {name}!"


def apply(ctx, config):
    # config has the schema's default when the row leaves greeting out.
    ctx.provide("greeter", Greeter(config["greeting"]))


plugin = define_plugin(
    apply,
    # The services it uses (step 3 of the guide): inject=["translate"], then
    # ctx.use("translate") in apply.
    inject=[],
    # The services it provides, so a host knows them before the plugin runs:
    # the class's public methods.
    provides={"greeter": Greeter},
    # The JSON Schema of its config (step 4).
    config={"type": "object", "properties": {"greeting": {"type": "string", "default": "Hello"}}},
)
```

- `define_plugin(apply, …)` 定义插件。插件启动时 rutis 调用 `apply(ctx, config)`；`apply` 也可以是 `async def`。
- `ctx.provide("greeter", …)` 提供名为 `greeter` 的服务：别的插件、别的语言的插件和宿主都能按名字用它。服务的方法是对象上不以 `_` 开头的方法。
- `provides` 写出插件提供哪些服务、各有哪些方法，宿主不运行插件就知道。

## 3. 使用服务

`greeter` 用 `translate` 服务翻译问候语。把 `src/greeter/__init__.py` 换成：

<!-- example: python file=greeter/src/greeter/__init__.py -->
```python
from rutis import define_plugin


class Greeter:
    def __init__(self, translate, greeting: str) -> None:
        self.translate = translate
        self.greeting = greeting

    async def hello(self, name: str) -> str:
        return f"{await self.translate.text(self.greeting)}, {name}!"


def apply(ctx, config):
    ctx.provide("greeter", Greeter(ctx.use("translate"), config["greeting"]))


plugin = define_plugin(
    apply,
    inject=["translate"],
    provides={"greeter": Greeter},
    config={"type": "object", "properties": {"greeting": {"type": "string", "default": "Hello"}}},
)
```

`inject` 写出插件用到的服务，`ctx.use("translate")` 拿到它。`inject` 里的服务都就绪了插件才启动，其中一个撤下时插件停下，回来时再启动。`translate` 由哪个插件提供、用什么语言写，`greeter` 不用知道；调用它的方法用 `await`。

现在模板自带的测试会失败：它没有给 `translate`。第 6 步换掉它。

## 4. 配置

`config` 是插件配置的 JSON Schema。`apply` 之前，rutis 补上 Schema 里的默认值并校验：行里不写 `greeting` 时，`config["greeting"]` 是 `"Hello"`；配置不合格时这一行以 `config: …` 失败，不调用 `apply`。

配置从哪里来：测试里由 `load(plugin, config=…)` 传入（第 6 步）；运行时来自宿主里这个插件的那一行（第 7 步的 `rutis.dev.json`，宿主的 `rutis.json`）。

## 5. 发出和监听事件

`hello` 每次问候后发出 `greeter/greeted` 事件；插件自己监听这个事件，数一共问候了几次，`count` 返回这个数。把 `src/greeter/__init__.py` 换成：

<!-- example: python file=greeter/src/greeter/__init__.py -->
```python
from rutis import define_plugin


class Greeter:
    def __init__(self, ctx, greeting: str) -> None:
        self.ctx = ctx
        self.translate = ctx.use("translate")
        self.greeting = greeting
        self.greeted = 0

    async def hello(self, name: str) -> str:
        word = await self.translate.text(self.greeting)
        self.ctx.emit("greeter/greeted", name)
        return f"{word}, {name}!"

    async def count(self) -> int:
        return self.greeted


def apply(ctx, config):
    greeter = Greeter(ctx, config["greeting"])

    def greeted(name):
        greeter.greeted += 1

    ctx.on("greeter/greeted", greeted)
    ctx.provide("greeter", greeter)


plugin = define_plugin(
    apply,
    inject=["translate"],
    provides={"greeter": Greeter},
    config={"type": "object", "properties": {"greeting": {"type": "string", "default": "Hello"}}},
)
```

`ctx.emit(name, *args)` 发出事件，立即返回，不等监听器。`ctx.on(name, listener)` 监听事件，什么都不用声明：发出方是同一个插件、别的插件、别的语言的插件还是宿主，都一样收到。插件停下时它的监听器自动撤掉。其余发出方式见 [插件 API：事件](plugin-api.md#事件)。

## 6. 测试

`rutis.testing` 的 `load` 不需要宿主：装上插件、给它配置和假的 `translate`，取它的服务来调用，监听它发出的事件；退出 `async with` 时卸载插件。把 `tests/test_plugin.py` 换成：

<!-- example: python file=greeter/tests/test_plugin.py -->
```python
import asyncio
import unittest

from rutis.testing import load

from greeter import plugin


class FakeTranslate:
    async def text(self, text: str) -> str:
        return {"Hello": "Bonjour", "Hi": "Salut"}.get(text, text)


class Plugin(unittest.TestCase):
    def test_greets_with_the_translated_greeting(self):
        async def go():
            async with load(plugin, config={"greeting": "Hi"}, services={"translate": FakeTranslate()}) as t:
                greeted = []
                t.on("greeter/greeted", greeted.append)
                self.assertEqual(await t.service("greeter").hello("Ada"), "Salut, Ada!")
                self.assertEqual(greeted, ["Ada"])
                self.assertEqual(await t.service("greeter").count(), 1)

        asyncio.run(go())


if __name__ == "__main__":
    unittest.main()
```

<!-- example: python run -->
```bash
uv run python -m unittest discover -s tests
```

`load` 检查宿主会检查的事：配置按 Schema 补默认值并校验，不合格时抛出 `BindingError`（`code` 是 `bad-config`）；插件只用 `inject` 里写的服务；提供的服务带着 `provides` 里写的方法；值按跨进程的规则传递。测试工具的其余接口见 [插件 API：Python 的测试工具](plugin-api.md#python-的测试工具)。宿主给了它的服务的 Python 类型时，用法见 [宿主给的类型](#宿主给的类型)。

## 7. 本地运行并试一下

`rutis-host dev` 在本地宿主里运行插件。运行哪些插件由 `rutis.dev.json` 的行写出：

<!-- example: python file=greeter/rutis.dev.json -->
```json
{
  "format": 1,
  "rows": [
    { "id": "greeter", "config": { "greeting": "Hello" } },
    { "id": "translate", "name": "py:fake_translate" }
  ]
}
```

- `id` 和插件同名的那一行（`greeter`）就是插件自己，`config` 是给它的配置。
- 另一行是假的 `translate`：`py:fake_translate` 是项目 `src/` 里的模块 `fake_translate`，内容如下。它只在本地运行时用，不在发布的包里（`pyproject.toml` 只打包 `src/greeter`）。

<!-- example: python file=greeter/src/fake_translate.py -->
```python
from rutis import define_plugin


class FakeTranslate:
    async def text(self, text: str) -> str:
        return {"Hello": "Bonjour", "Hi": "Salut"}.get(text, text)


def apply(ctx, config):
    ctx.provide("translate", FakeTranslate())


plugin = define_plugin(apply, provides={"translate": FakeTranslate})
```

<!-- example: python run until="greeter: running" -->
```bash
uv run rutis-host dev
```

```text
rutis-host dev: running greeter; changes reload it (Ctrl-C ends)
greeter: starting
translate: running
rutis-host dev: services: greeter (greeter), loader (host), translate (translate)
greeter: running
```

每一行的状态变化时打印一行，各行之间的先后每次可能不同；`rutis-host dev: services:` 是现在的服务和提供它的行。

另开一个终端，在项目目录里看现在有哪些服务、由哪一行提供，再调一次（Windows 上不提供 `status` 与 `call`）：

```console
$ uv run rutis-host status
rows:
  greeter: running
  translate: running
services:
  greeter: greeter
  loader: host
  translate: translate
runtimes:
  py: running (pid 55011)
calls:
  none
$ uv run rutis-host call greeter.hello '"Ada"'
"Bonjour, Ada!"
$ uv run rutis-host call greeter.count
1
```

`call` 的每个参数是一个 JSON 值，字符串带引号；结果打印成 JSON，出错时写到标准错误并以 1 退出（见 [调用一个服务](rutis-host.md#调用一个服务)）。

改代码就重新加载：把 `hello` 返回的 `!` 改成 `!!` 并保存，`dev` 打印

```text
greeter: reloaded (src/greeter/__init__.py changed)
rutis-host dev: services: greeter (greeter), loader (host), translate (translate)
```

再 `call greeter.hello '"Ada"'` 得到 `"Bonjour, Ada!!"`。只有文件变了的那一行重新加载；`rutis.dev.json` 改了也在运行中生效。只重新导入插件模块本身，它导入的其他模块改了要重启 `dev`。见 [用 dev 开发插件](rutis-host.md#用-dev-开发插件)。

## 8. 发布

```bash
uv build
uv publish
```

或者推一个 `v0.1.0` 这样的 tag，模板里的工作流会测试并用 PyPI 的 trusted publishing 发布（先在 PyPI 上为仓库配置 trusted publisher）。

- 包名：`greeter` 这样的短名字在 PyPI 上多半已被占用，发布前改 `pyproject.toml` 的 `name`，例如 `your-name-greeter`。入口点名不用改，宿主的行名仍是 `py:greeter`：

  ```toml
  [project.entry-points."rutis.plugins"]
  greeter = "greeter"
  ```

- 依赖范围：对 `rutis` 只写下限（`rutis>=0.19`，`rutis-host new` 写的是它自己的版本）。插件和宿主的 Python 运行时用同一个 `rutis` 包，写了上限会让宿主降级或升级不了（见 [插件 API：版本](plugin-api.md#版本)）。

宿主怎样装你的插件见 [rutis-host：宿主怎样装插件](rutis-host.md#宿主怎样装插件)。

## 调试

`--debug` 让 Python 运行时读这个终端：插件里的 `breakpoint()` 停在 pdb，在终端里用 `w` 看调用栈、`p <变量>` 看变量、`c` 继续。

<!-- example: python run until="greeter: running" -->
```bash
uv run rutis-host dev --debug
```

- 停在断点时整个 Python 运行时停着：这个运行时里其他行的调用也等它继续。
- 不加 `--debug` 时运行时没有标准输入，`breakpoint()` 让这一行失败。
- 运行时和 `dev` 共用终端，Ctrl-C 也送到运行时：它直接退出，插件的清理可能来不及运行。
- 插件 `print` 的内容和写到标准错误的内容出现在 `dev` 的输出里。
- 用图形界面的调试器（VS Code 等）：在项目环境里装 `debugpy`，在 `apply` 里调用 `debugpy.listen(5678)`，再用调试器连 `127.0.0.1:5678`。
- 测试（第 6 步）在测试进程里运行插件，`breakpoint()` 不用 `--debug` 也能用。

## 宿主给的类型

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

## 进阶：插件 API

插件作为值、其余事件方式、作用域、带方法的对象、值怎样传递、测试工具的其余接口：见 [插件 API](plugin-api.md)。在另一台机器上运行见 [rutis-host.md 的远程一节](rutis-host.md#远程运行时)。
