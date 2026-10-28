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

A plugin is itself a value: hand the result of `define_plugin(...)` to a service as an ordinary argument (an application's part registry, say), and the receiver can mount it elsewhere, one copy per mount, each running in this process with what your `apply` prepared:

```python
async def apply(ctx, config):
    pool = await open_pool()

    def session(ctx, config):
        ctx.provide("gitStatus", GitStatus(pool, config["depth"]))

    ctx.use("parts").register("session", define_plugin(session, inject=["history"]))
```

When this row unloads or reloads, every copy mounted from a definition it handed over ends first. In the testing tool, a definition a test double receives can be passed to `load` as it is. See [Plugin API](plugin-api.en.md#plugin-definitions).

### Objects with methods

Data (`dict`, `list`, `tuple`, dataclasses) is copied; an instance of any other class goes to another language by reference: the far end gets a proxy whose calls reach the original object here. An object another process passes here is a proxy too:

```python
cache = await ctx.use("store").cache()
await cache.set("a", 1)          # every method is async, a call of the far end's object
value = await cache.get("a")
```

- Every public name on a proxy is a method (as with `ctx.use`); it has no data fields: to get data, have a method return it. Its `repr` is `<remote object>`.
- One object passed several times is one proxy on the far end (`is`); a proxy passed back to its own process is the original object there.
- An object does not follow its row: after the row that passed it unloads, it can still be called. Once its runtime has exited, calls on it fail with `peer-gone`.
- Calling a method the object lacks fails that call with `no-method`: `Cache has no method gett (methods: get, set)`.
- `undefined` (sent by JavaScript) inside a list or dict is `None` here, at any depth.
- Rows in the same runtime get the object itself. The testing tool's strict mode gives the cross-process proxy.

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

`load` checks the same things as the host: the plugin uses only services declared in `inject`; its provided service has every method declared in `provides` (an undeclared service is called as rutis calls it, by the object's public methods, all async; `t.methods(name)` returns the methods rutis sees); and all cleanup functions run when the `async with` block exits. In strict mode (the default), values follow cross-process rules: data is copied (dataclasses become dictionaries), functions and objects with methods are passed by reference, and `sync` methods cannot return coroutines. Code that works only within one process fails here.

The testing tool's whole interface (from the docstrings of `rutis/testing.py`):

| Parameter of `load(plugin, *, …)` | What it does |
| --- | --- |
| `config` | The plugin's config; with a `config` declaration, defaults are filled in and it is validated |
| `services` | The services the plugin `inject`s, by name |
| `strict` | Values follow cross-process rules (default `True`) |
| `bus` | Share events with the plugins on the same `Bus` (default: one of its own) |
| `scope` | The scope path it is loaded in (`t1/s1`; default the root) |
| `row` | Its row name on the bus (default the plugin's declared name, else its module's) |
| `runtime` | What listener descriptors give as `runtime` (default `"python"`) |
| `deadline` | How many seconds unloading waits for running listeners (default `RUTIS_SHUTDOWN_TIMEOUT`, 10) |

| Member of `Loaded` (`async with load(...) as t`) | What it does |
| --- | --- |
| `t.service(name)` | The service the plugin provides, as rutis sees it: its declared methods, or, undeclared, the object's public methods, all async |
| `t.methods(name)` | The methods rutis sees: `{"today": "async"}` |
| `t.provided()` | The names of the services the plugin provides now |
| `t.on(name, listener, *, prepend=False, global_=False)` | Listen as the host does, at the root scope: the plugin's emissions and the test's own reach it; in strict mode it gets the arguments as from another process; returns the function that stops it, and without it, it stops when the plugin unloads |
| `t.emit`, `t.parallel`, `t.serial`, `t.waterfall`, `t.bail` | Emit from the plugin's row and scope |
| `t.filter(predicate)` | The five methods, delivering only to the listeners `predicate(descriptor)` keeps |
| `t.reported` | The failures nobody received (listeners failing in an `emit`, filters) |
| `await t.update(config)` | Change the config as a host's `rows.update` does: unload, then run again with the new config |
| `await t.unload()` | Unload as the runtime does: withdraw listeners and services, cancel and wait for running listeners, then run the cleanups, latest first (done when the `async with` block exits) |

`Bus(scopes=...)`: a bus for tests; the plugins on the same `bus` hear each other, and `scopes` names the group of a scope path. It has `reported`, `listeners(name)` (the descriptors a filter sees) and `emitter(row=..., strict=...)` (emit as a given row). `Reported`: one failure nobody received, with the fields `kind` (`"listener"` or `"filter"`), `name`, `row` and `error`. `PluginTestError`: the plugin did something a host would refuse or that would fail across processes; a subclass of `AssertionError`.

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

`ctx.on(name, listener)` listens to an event; `ctx.emit`, `await ctx.parallel`, `await ctx.serial`, `await ctx.waterfall` and `ctx.bail` are the five ways to emit. Listeners register themselves with the host and nothing is declared: whether the emitter is in the same process, in a runtime of another language, in the host or on a linked node, the result is the same. The full rules are in [plugin API: events](plugin-api.en.md#events).

The two plugins below are on one `Bus`: `greeter` takes the first result with `serial`, lets other plugins rewrite the greeting with `waterfall` and notifies with `emit`; `shout` is a link of the waterfall and listens to the notification.

<!-- example: python-en file=weather/tests/test_events.py order=2 -->
```python
import asyncio
import unittest

from rutis import define_plugin
from rutis.testing import Bus, load


def greeter_apply(ctx, config):
    # None or False is "no result": serial asks the next listener
    ctx.on("greeting/pick", lambda lang: "Hej" if lang == "sv" else None)

    class Greeter:
        async def hello(self, name, lang):
            word = await ctx.serial("greeting/pick", lang) or "Hello"
            # terminal: the innermost end of the chain, given the event's arguments
            text = await ctx.waterfall("greeting/format", f"{word}, {name}", terminal=lambda text: text)
            ctx.emit("greeting/said", name)      # returns at once, waits for no listener
            return text

    ctx.provide("greeter", Greeter())


greeter = define_plugin(greeter_apply)
said = []


def shout_apply(ctx, config):
    async def exclaim(text, next):           # a waterfall listener gets next last
        return f"{await next()}!"

    ctx.on("greeting/format", exclaim)
    ctx.on("greeting/said", said.append)


shout = define_plugin(shout_apply)


class Events(unittest.TestCase):
    def test_plugins_hear_each_other(self):
        async def go():
            bus = Bus()                          # plugins on one bus hear each other
            async with load(greeter, bus=bus, row="greeter") as t, load(shout, bus=bus, row="shout"):
                self.assertEqual(await t.service("greeter").hello("Ada", "sv"), "Hej, Ada!")
                self.assertEqual(await t.service("greeter").hello("Bo", "en"), "Hello, Bo!")
                self.assertEqual(said, ["Ada", "Bo"])
                self.assertEqual(await t.serial("greeting/pick", "sv"), "Hej")  # the test can emit too

        asyncio.run(go())
```

- `emit` returns at once; a failing listener reaches neither the emitter nor the other listeners and is reported in one line where it runs (in a test, in `t.reported`). To wait for every listener and get their errors, use `await ctx.parallel(...)`: failures are a `rutis.AggregateError` whose `errors` are in registration order.
- `bail` is a synchronous `serial`: an `async def` listener (or a function returning a coroutine) fails it with `rutis.AsyncListener`.
- `ctx.filter(lambda d: d["row"] == "audit")` returns an object with the five methods that delivers only to the listeners the filter keeps; `ctx.on(name, fn, prepend=True)` goes first, `global_=True` is exempt from filters.
- Scopes: `load(plugin, bus=bus, scope="t1/s1")` loads a plugin into an instance (`Bus(scopes={"t1": "tenant"})` names the groups); a listener receives every event of the name on the root bus; a scope is only a descriptor for filters.
- On unload, the tasks of running listeners are cancelled and waited for (at most `RUTIS_SHUTDOWN_TIMEOUT`) before the cleanups run. When the emitter gives up (its task is cancelled), the listeners in progress are cancelled too.

**Across languages**: a TypeScript plugin emits, a Python plugin listens, nothing to change. The host's `rutis.json`:

```json
{
  "runtimes": { "node": { "project": "web" }, "py": { "project": "plugins" } },
  "rows": [
    { "id": "sessions", "name": "sessions" },
    { "id": "audit", "name": "py:audit" }
  ]
}
```

The TypeScript plugin `sessions` in `web/` emits the event, and asks others for a quota with `serial`:

```ts
ctx.provide('sessions', {
  async create(user: string) {
    const quota = (await ctx.serial<number>('session/quota', user)) ?? 1
    const id = crypto.randomUUID()
    await ctx.parallel('session/created', id, user, quota)   // waits for every listener, in any language
    return id
  },
})
```

`plugins/audit.py` answers the quota and records the creation:

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

Arguments cross processes as in [passing values](plugin-api.en.md#passing-values); an error a Python listener raises is an error of the same name on the TypeScript side (in `parallel`, a child of the `AggregateError`).

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

Depend on `rutis` with a lower bound only: `rutis>=0.17` (`rutis-host new` writes its own version). The plugin and the Python runtime are the same `rutis` package, installed in the host's one environment; with an upper bound (`<0.18`), installing the plugin downgrades rutis, or a later upgrade of rutis fails to resolve. Whether the runtime can load the plugin is decided by the plugin API version: `define_plugin` tags the plugin with the plugin API version it uses, a host runtime older than that refuses it with a clear message, and a newer one loads it; while the plugin API version stays, `rutis` removes or changes no name plugins import ([plugin API: versions](plugin-api.en.md#versions)).

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
