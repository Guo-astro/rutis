# rutis

Write [rutis](https://github.com/arcships/rutis) plugins in Python, and the runtime that runs them.

```python
from rutis import define_plugin


class Weather:
    def __init__(self, llm, city):
        self.llm, self.city = llm, city

    async def today(self):
        return f"{await self.llm.ask(self.city)} in {self.city}"


def apply(ctx, config):
    ctx.provide("weather", Weather(ctx.use("llm"), config["city"]))


plugin = define_plugin(apply, inject=["llm"])
```

What `ctx.provide` provides reaches rutis by itself, its methods the object's public callables, called as `async`. `provides` is optional: `provides={"weather": Weather}` (or `{"weather": {"unit": "sync"}}`) marks methods `sync` and tells `rutis-host check` what the plugin provides before it runs.

Events are one bus across every plugin, runtime and linked node (rutis 0.13): wherever a listener is, the result is the same.

```python
def apply(ctx, config):
    async def check(text, next):           # in a waterfall, next comes last
        return "blocked" if "spam" in text else await next()

    ctx.on("chat/check", check)
    ctx.on("chat/said", lambda text: print(text), prepend=True)

# elsewhere
ctx.emit("chat/said", "hi")                # starts every listener, returns at once
await ctx.parallel("chat/said", "hi")      # waits for all; rutis.advanced.AggregateError on failures
await ctx.serial("chat/lookup", 7)         # the first result (not None / False)
await ctx.waterfall("chat/check", "hi", terminal=lambda text: "ok")
ctx.bail("chat/lookup", 7)                 # serial, synchronously
await ctx.filter(lambda d: d["row"] == "audit").parallel("chat/said", "hi")
```

A listener hears every event of the name on the one flat bus; a scope is a descriptor for filters. A failing `emit` listener is reported on stderr, never raised. `rutis/*` events can be listened to, not emitted. On unload a running listener's task is cancelled and waited for (at most `RUTIS_SHUTDOWN_TIMEOUT`).

A filter's descriptor is a dict: `name`, `prefix`, `plugin`, `row`, `runtime`, `node`, `scope`, `global`. `plugin` is the name the plugin declares, `define_plugin(apply, name="chat-ui")`; without one, the id of its row in the configuration (`row`). Errors from listeners arrive through the host as `rutis.RemoteError` (rutis's own, such as `BindingError`, as their classes; `rutis.advanced.AggregateError` children too); match errors by `name` (and `code`), not by class. A row that unloaded cannot emit any more (`BindingError`). `ctx.on`, the emitters and disposers may be called from the plugin's own threads: they run on the runtime's loop, in call order (from a thread, `parallel` / `serial` / `waterfall` return an awaitable for that thread's loop, or a `concurrent.futures.Future` without one).

Test it on a host of its own (`rutis-host test`: `rutis-host` on PATH, from the `rutis-host` package, or `RUTIS_HOST`):

```python
from rutis.testing import load

async with load(plugin, config={"city": "Oslo"}, services={"llm": FakeLlm()}) as t:
    assert await t.service("weather").today() == "sunny in Oslo"
```

The plugin runs on this process's Python runtime as a row of that host: the host's loader checks its config, every event goes through the host. The loaded plugin `t` has the same `emit` / `parallel` / `serial` / `waterfall` / `bail` / `filter`, `t.on` to listen, and `await t.reported()` for what the host reported of it; plugins loaded with one `rutis.testing.Bus` share one host.

A host runs plugins with `python -m rutis <channel> --id <id> <project>` (it starts that itself); `python -m rutis listen:wss://… --id <id> --peer <controller> <project>` runs a runtime a host on another machine controls (install `rutis[network]`; not on CPython 3.13.6, whose TLS sockets deadlock, [python/cpython#137583](https://github.com/python/cpython/issues/137583): use 3.13.7 or later); `python -m rutis dial:wss://… --id <id> --peer <controller> <project>` connects to that host instead and reconnects by itself (token in `RUTIS_TOKEN`, a further CA in `RUTIS_CA`). Packaged plugins register under the entry point group `rutis.plugins`. Python 3.12 or later; no dependencies.

Start a project with `uvx rutis-host new <name> --lang python`. Guide (Chinese): [docs/guide/python-plugin.md](https://github.com/arcships/rutis/blob/main/docs/guide/python-plugin.md).
