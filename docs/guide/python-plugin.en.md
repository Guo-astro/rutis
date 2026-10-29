# Write a Python Plugin

From starting a project to publishing it, in eight steps. The example is the `greeter` the template makes: it provides the `greeter` service and uses the `translate` service, which another plugin provides, to translate its greeting. It needs Python 3.12 or later; the commands below use [uv](https://docs.astral.sh/uv/), and pip with venv works too.

## 1. Start a project

<!-- example: python-en run -->
```bash
uvx rutis-host new greeter --lang python
cd greeter
uv sync
uv run python -m unittest discover -s tests
```

The project:

| File | What it is |
| --- | --- |
| `src/greeter/__init__.py` | The plugin |
| `tests/test_plugin.py` | Its tests, which need no host |
| `rutis.dev.json` | The rows of a local run (step 7) |
| `pyproject.toml` | Depends on `rutis`; dev dependency `rutis-host`; entry point `rutis.plugins` |
| `.github/workflows/publish.yml` | Tests and publishes to PyPI on a `v*` tag (step 8) |

## 2. Provide a service

The generated `src/greeter/__init__.py`:

<!-- example: python-en file=greeter/src/greeter/__init__.py -->
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

- `define_plugin(apply, …)` defines the plugin. When the plugin starts, rutis calls `apply(ctx, config)`; `apply` may be an `async def`.
- `ctx.provide("greeter", …)` provides the service named `greeter`: other plugins, plugins in other languages and the host use it by that name. The service's methods are the object's methods whose names do not start with `_`.
- `provides` says which services the plugin provides and their methods, so a host knows them without running the plugin.

## 3. Use a service

`greeter` translates its greeting with the `translate` service. Replace `src/greeter/__init__.py` with:

<!-- example: python-en file=greeter/src/greeter/__init__.py -->
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

`inject` lists the services the plugin uses, and `ctx.use("translate")` gets one. The plugin starts once all of them are there; when one goes, the plugin stops, and it starts again when the service is back. `greeter` need not know which plugin provides `translate` or in which language; call its methods with `await`.

The template's test now fails: it gives no `translate`. Step 6 replaces it.

## 4. Configuration

`config` is the JSON Schema of the plugin's config. Before `apply`, rutis fills in the schema's defaults and validates the config: when the row leaves `greeting` out, `config["greeting"]` is `"Hello"`; when the config does not match, the row fails with `config: …` and `apply` is not called.

Where the config comes from: in a test, `load(plugin, config=…)` passes it (step 6); when running, it is the plugin's row in the host (`rutis.dev.json` in step 7, the host's `rutis.json`).

## 5. Emit and listen to events

After each greeting, `hello` emits the `greeter/greeted` event; the plugin listens to it to count the greetings, and `count` returns the number. Replace `src/greeter/__init__.py` with:

<!-- example: python-en file=greeter/src/greeter/__init__.py -->
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

`ctx.emit(name, *args)` emits an event and returns at once, without waiting for the listeners. `ctx.on(name, listener)` listens to one, with nothing to declare: it hears the event alike whether this plugin, another plugin, a plugin in another language or the host emits it. The plugin's listeners go when it stops. The other ways to emit: [Plugin API: events](plugin-api.en.md#events).

## 6. Test

`load` from `rutis.testing` needs no host: it loads the plugin with a config and a fake `translate`, gives you its service to call and its events to listen to, and unloads the plugin when the `async with` ends. Replace `tests/test_plugin.py` with:

<!-- example: python-en file=greeter/tests/test_plugin.py -->
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

<!-- example: python-en run -->
```bash
uv run python -m unittest discover -s tests
```

`load` checks what a host checks: the config gets the schema's defaults and is validated, and a config that does not match raises `BindingError` (its `code` is `bad-config`); the plugin uses only the services in `inject`; the services it provides have the methods in `provides`; values pass by the rules across processes. The rest of the test tool: [Plugin API: the Python test tool](plugin-api.en.md#the-python-test-tool). When a host gives Python types for its services, see [Types from the host](#types-from-the-host).

## 7. Run it locally and try it

`rutis-host dev` runs the plugin in a local host. The rows of `rutis.dev.json` say which plugins run:

<!-- example: python-en file=greeter/rutis.dev.json -->
```json
{
  "format": 1,
  "rows": [
    { "id": "greeter", "config": { "greeting": "Hello" } },
    { "id": "translate", "name": "py:fake_translate" }
  ]
}
```

- The row whose `id` is the plugin's (`greeter`) is the plugin itself, and `config` is its config.
- The other row is a fake `translate`: `py:fake_translate` is the module `fake_translate` in the project's `src/`, below. It runs only locally and is not in the published package (`pyproject.toml` packages only `src/greeter`).

<!-- example: python-en file=greeter/src/fake_translate.py -->
```python
from rutis import define_plugin


class FakeTranslate:
    async def text(self, text: str) -> str:
        return {"Hello": "Bonjour", "Hi": "Salut"}.get(text, text)


def apply(ctx, config):
    ctx.provide("translate", FakeTranslate())


plugin = define_plugin(apply, provides={"translate": FakeTranslate})
```

<!-- example: python-en run until="greeter: running" -->
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

Each row prints a line when its state changes, and the rows' lines may come in another order each time; `rutis-host dev: services:` gives the services there are now and the row that provides each.

In another terminal, in the project's directory, see which services there are and which row provides each, then call one (`status` and `call` are not on Windows):

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

Each argument of `call` is a JSON value, strings in quotes; the result is printed as JSON, and an error goes to standard error with exit status 1 (see [Calling a service](rutis-host.en.md#calling-a-service)).

A change to the code reloads it: change the `!` that `hello` returns to `!!` and save, and `dev` prints

```text
greeter: reloaded (src/greeter/__init__.py changed)
rutis-host dev: services: greeter (greeter), loader (host), translate (translate)
```

`call greeter.hello '"Ada"'` now gives `"Bonjour, Ada!!"`. Only the row whose file changed reloads; a change to `rutis.dev.json` takes effect while it runs too. Only the plugin's module is imported again; when a module it imports changes, restart `dev`. See [Developing a plugin with `dev`](rutis-host.en.md#developing-a-plugin-with-dev).

## 8. Publish

```bash
uv build
uv publish
```

Or push a tag such as `v0.1.0`: the template's workflow tests and publishes with PyPI trusted publishing (configure a trusted publisher for the repository on PyPI first).

- Package name: a short name like `greeter` is most likely taken on PyPI, so change `name` in `pyproject.toml` before publishing, to `your-name-greeter` for example. The entry point's name stays, and so does the host's row name `py:greeter`:

  ```toml
  [project.entry-points."rutis.plugins"]
  greeter = "greeter"
  ```

- Dependency range: give `rutis` a lower bound only (`rutis>=0.19`; `rutis-host new` writes its own version). The plugin and the host's Python runtime use the same `rutis` package, so an upper bound would downgrade the host or keep it from upgrading (see [Plugin API: versions](plugin-api.en.md#versions)).

How a host installs your plugin: [rutis-host: how a host installs plugins](rutis-host.en.md#how-a-host-installs-plugins).

## Debugging

`--debug` has the Python runtime read this terminal: a `breakpoint()` in the plugin stops in pdb; in the terminal, `w` shows the stack, `p <variable>` a variable, `c` continues.

<!-- example: python-en run until="greeter: running" -->
```bash
uv run rutis-host dev --debug
```

- While it is stopped at a breakpoint, the whole Python runtime is stopped: calls to its other rows wait too.
- Without `--debug` the runtime has no standard input, and `breakpoint()` fails the row.
- The runtime shares the terminal with `dev`, so Ctrl-C reaches it too: it exits at once, and the plugin's cleanups may not run.
- What the plugin prints, and what it writes to standard error, appears in `dev`'s output.
- With a graphical debugger (VS Code and others): install `debugpy` in the project's environment, call `debugpy.listen(5678)` in `apply`, and attach the debugger to `127.0.0.1:5678`.
- Tests (step 6) run the plugin in the test process, where `breakpoint()` works without `--debug`.

## Types from the host

A Rust host can give the Python types of its services (the module `rutis-host types --python` writes, see [Embed in a Rust application: declarations](rust-host.en.md#declarations)), usually in its plugin template. Annotate what `ctx.use` returns with them, and your editor and pyright check the calls before anything runs; without the annotation it runs the same:

```python
from deskmate_types import Notes  # the host's module (written by rutis-host types --python deskmate_types.py)


def apply(ctx, config):
    notes: Notes = ctx.use("notes")
    ...
```

A call that does not match, as pyright reports it:

```text
error: Argument of type "Literal[42]" cannot be assigned to parameter "value" of type "str" in function "put"
```

## Advanced: plugin API

Plugins as values, the other ways to emit events, scopes, objects with methods, how values pass, the rest of the test tool: see [Plugin API](plugin-api.en.md). To run on another machine, see [the remote section of rutis-host.en.md](rutis-host.en.md#remote-runtimes).
