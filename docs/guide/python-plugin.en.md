# Write a Python Plugin

This guide covers creating a project, publishing it, and using it from a host. Python 3.10 or later is required. The examples use [uv](https://docs.astral.sh/uv/), but pip and venv work too.

## 1. Create a project

<!-- example: python-en run -->
```bash
uvx rutis-host new weather --lang python
cd weather
uv sync
```

The generated project contains:

| File | Purpose |
| --- | --- |
| `src/weather/__init__.py` | The plugin itself. |
| `tests/test_plugin.py` | Unit tests that do not need a host. |
| `rutis.dev.json` | Local runtime configuration and other plugins used for testing. |
| `pyproject.toml` | Depends on `rutis` (a lower bound only, see §5); development dependency `rutis-host`; entry point under `rutis.plugins`. |
| `.github/workflows/publish.yml` | Tests and publishes to PyPI when a `v*` tag is pushed. |

## 2. Write the plugin

The generated `src/weather/__init__.py` is a `greeter` that uses no service, and `tests/test_plugin.py` is its test. Here it becomes a `weather` that uses an `llm` service: replace `__init__.py` with the code below, and `tests/test_plugin.py` with the one in §3 at the same time (the two files go together).

<!-- example: python-en file=weather/src/weather/__init__.py -->
```python
from rutis import define_plugin


class Weather:
    def __init__(self, llm, city):
        self.llm, self.city = llm, city

    async def today(self):                # async def declares an asynchronous method
        return f"{await self.llm.ask(f'weather in {self.city}')} in {self.city}"


def apply(ctx, config):
    ctx.provide("weather", Weather(ctx.use("llm"), config["city"]))
    return lambda: None                   # Cleanup may be async; returning nothing is also fine


plugin = define_plugin(
    apply,
    inject=["llm"],                       # Services used: start when all are ready; stop if any is revoked
    provides={"weather": Weather},        # What it provides: check knows before running
    config={"type": "object", "properties": {"city": {"type": "string", "default": "Oslo"}}},
)
```

Before `apply`, rutis fills the config with the schema's defaults (`Oslo` when the row has no `city`) and validates it: a row whose config does not match fails with `config: …`, and `apply` is not called ([plugin API: configuration](plugin-api.en.md#configuration)). `load` and `update` in `rutis.testing` do the same, raising a `BindingError` with code `bad-config`; extra keys are written on stderr.

You can also omit `define_plugin` and declare `apply`, `inject`, `provides`, and `Config` directly in the module. `apply` may be an `async def` function.

`provides` says what the plugin provides, so `rutis-host check` knows without running the plugin that this row provides `weather`: when a host checks its configuration with `check --strict` in CI, the rows that use `weather` pass (the template declares it by default). `provides={"weather": Weather}` reads the methods from the class: those not starting with `_`, `async def` ones async, the others sync; `{"weather": {"today": "async"}}` works too. It also runs without one: what `ctx.provide` provides still reaches rutis by itself, its methods the object's callable attributes not starting with `_`, all called as `async`; `check` then only knows once the plugin runs, and `--strict` reports an error.

`ctx.use("llm")` gets the service. A service provided by a plugin in the same process is the object itself; services from another language or machine are proxies, whose methods return synchronously or produce awaitable results according to their declarations. See [Plugin API](plugin-api.en.md) for capabilities and value passing rules.

A plugin is itself a value, to hand to a service and mount elsewhere many times: see [plugin API: plugin definitions](plugin-api.en.md#plugin-definitions).

### Objects with methods

Data (`dict`, `list`, `tuple`, dataclasses) is copied; an instance of any other class goes to another language by reference, the far end getting a proxy: see [plugin API: objects with methods](plugin-api.en.md#objects-with-methods).

## 3. Test

`rutis.testing` does not require a host:

<!-- example: python-en file=weather/tests/test_plugin.py -->
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

<!-- example: python-en run -->
```bash
uv run python -m unittest discover -s tests
```

`load` checks the same things as the host: the plugin uses only services declared in `inject`; it does not provide a name already in `services` (the host provides it: `PluginTestError: wordcount: notes is already provided by the host`); its provided service has every method declared in `provides` (an undeclared service is called as rutis calls it, by the object's public methods, all async; `t.methods(name)` returns the methods rutis sees); and all cleanup functions run when the `async with` block exits. Strict mode is the default: values follow cross-process rules, see [plugin API: strict mode](plugin-api.en.md#strict-mode).

The rest of the test tool's interface (`Bus`, scopes, `update` and more) is in [plugin API: the Python test tool](plugin-api.en.md#the-python-test-tool).

To check that the plugin emits an event, listen with `t.on` into a list:

<!-- example: python-en file=weather/tests/test_asked.py order=2 -->
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

### Events

`ctx.on(name, listener)` listens to an event, `ctx.emit(name, *args)` emits one (`test_asked.py` above). Listeners register themselves with the host and nothing is declared: whether the emitter is in the same process, in a runtime of another language, in the host or on a linked node, the result is the same, so a TypeScript plugin emits and a Python plugin listens with nothing to change. The other ways to emit (`parallel`, `serial`, `waterfall`, `bail`), filters, scopes and an example across languages are in [plugin API: events](plugin-api.en.md#events).

## 4. Run in a local host

<!-- example: python-en run until="weather: running" order=99 -->
```bash
uv run rutis-host dev
```

`dev` runs the plugin with the project's `.venv` and reimports it when its files change; only the rows whose files changed reload, and a change to `rutis.dev.json` applies while it runs (see [Developing a plugin with `dev`](rutis-host.en.md#developing-a-plugin-with-dev)). Add other required services to `rutis.dev.json`, such as a fake `llm` (the row name `py:fake_llm` names the module `fake_llm` in the project's `src/`, shown after it):

<!-- example: python-en file=weather/rutis.dev.json -->
```json
{
  "rows": [
    { "id": "weather", "config": { "city": "Oslo" } },
    { "id": "llm", "name": "py:fake_llm" }
  ]
}
```

<!-- example: python-en file=weather/src/fake_llm.py -->
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

`dev` finds plugin modules in the project's `src/`, so `fake_llm` needs no entry point, and it is not in the built wheel (`pyproject.toml` packages only `src/weather`).

The row with the same ID as the plugin supplies its configuration; the other rows are additional plugins. They may also be TypeScript plugins (in that case, add `"runtimes": { "node": { "project": "." } }` to `rutis.dev.json`). `uv run rutis-host check` lists each row's dependencies, provided services, and configuration schema, and exits with a nonzero status if there is a problem.

Only the plugin module itself is reimported. If another imported module changes, restart `rutis-host dev`.

### Calling a method

After it starts and after each reload, `dev` prints which services there are and the row that provides each: `rutis-host dev: services: llm (llm), loader (host), weather (weather)`.

While `dev` runs, in another terminal in the project's directory, call a service the plugin provides, to see what it returns and what it says when it fails, without writing a plugin to call it:

```console
$ uv run rutis-host call weather.today
"sunny in Oslo"
```

Each argument is a JSON value (a string with its quotes: `'"Oslo"'`); the result is printed as JSON; an error goes to stderr with exit status 1. See [rutis-host: Calling a service](rutis-host.en.md#calling-a-service).

### Debugging

`--debug` gives the Python runtime this terminal: a `breakpoint()` in the plugin stops in pdb, where `w` shows the call stack, `p <variable>` a variable, and `c` continues.

<!-- example: python-en run until="weather: running" order=100 -->
```bash
uv run rutis-host dev --debug
```

- While it is stopped at a breakpoint, the whole Python runtime waits: calls to its other rows wait too.
- Without `--debug` the runtime has no standard input, and `breakpoint()` fails the row.
- The runtime shares the terminal with `dev`, so Ctrl-C reaches it too: it exits at once, and the plugin's cleanups may not get to run.
- What the plugin prints, and what it writes to standard error, shows in `dev`'s output.
- With a graphical debugger (VS Code and the like): install `debugpy` in the project's environment, call `debugpy.listen(5678)` in `apply`, and attach the debugger to `127.0.0.1:5678`.
- Tests (§3) run the plugin in the test process, where `breakpoint()` works without `--debug`.

## 5. Publish

```bash
uv build
uv publish
```

Alternatively, push a tag such as `v0.1.0`. The generated workflow tests, runs `check`, and publishes through PyPI trusted publishing (configure a trusted publisher for the repository on PyPI first).

The entry point in `pyproject.toml` lets the host find the plugin by name:

```toml
[project.entry-points."rutis.plugins"]
weather = "weather"
```

Depend on `rutis` with a lower bound only: `rutis>=0.17` (`rutis-host new` writes its own version). The plugin and the Python runtime are the same `rutis` package, installed in the host's one environment; with an upper bound (`<0.18`), installing the plugin downgrades rutis, or a later upgrade of rutis fails to resolve. Whether the runtime can load the plugin is decided by the plugin API version: `define_plugin` tags the plugin with the plugin API version it uses, a host runtime older than that refuses it with a clear message, and a newer one loads it; a release of `rutis` that removes or moves a name plugins import says so in its migration guide, and plugins upgrade together with the host ([plugin API: versions](plugin-api.en.md#versions)).

Before publishing, give the distribution a name of your own: a short name such as `weather` is most likely taken on PyPI. Change `name` in `pyproject.toml`, for example to `your-name-weather`; the entry point name (`weather` above) stays, and a host's row is still `py:weather`.

## 6. Use from a host

Install the plugin in the Python environment used to run Python plugins (the interpreter named by `runtimes.py.python` in `rutis.json`; by default the active virtual environment's, then `<runtimes.py.project>/.venv/bin/python`), then add a row named `py:<entry-point-name>`. Create the environment first if there is none. The Python runtime is the `rutis` in that environment: install it at the version of the host's rutis-host (`rutis-host --version`):

```bash
uv venv                                                         # creates .venv
uv pip install --python .venv/bin/python "rutis==0.18.0" your-name-weather   # the runtime (the host's version) and the plugin
```

```json
{ "id": "weather", "name": "py:weather", "config": { "city": "Oslo" } }
```

See [rutis-host and rutis.json](rutis-host.en.md#how-a-host-installs-plugins), or [Embed in a Rust application](rust-host.en.md) for a Rust host.

### Types from the host

A Rust host can give the Python types of its services (the module `rutis-host types --python` writes; see [Embed in a Rust application: Declarations](rust-host.en.md#declarations)), usually in the plugin template. Annotate what `ctx.use` returns with them, and the editor and pyright check the calls before anything runs; without the annotation it runs the same:

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
