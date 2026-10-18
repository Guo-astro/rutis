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
await ctx.parallel("chat/said", "hi")      # waits for all; rutis.AggregateError on failures
await ctx.serial("chat/lookup", 7)         # the first result (not None / False)
await ctx.waterfall("chat/check", "hi", terminal=lambda text: "ok")
ctx.bail("chat/lookup", 7)                 # serial, synchronously
await ctx.filter(lambda d: d["row"] == "audit").parallel("chat/said", "hi")
```

A listener hears events emitted in its scope and inside it. A failing `emit` listener is reported on stderr, never raised. `rutis/*` events can be listened to, not emitted. On unload a running listener's task is cancelled and waited for (at most `RUTIS_SHUTDOWN_TIMEOUT`).

Test it without a host:

```python
from rutis.testing import load

async with load(plugin, config={"city": "Oslo"}, services={"llm": FakeLlm()}) as t:
    assert await t.service("weather").today() == "sunny in Oslo"
```

The loaded plugin `t` has the same `emit` / `parallel` / `serial` / `waterfall` / `bail` / `filter`; plugins loaded with one `rutis.testing.Bus` hear each other, each in its `scope` (`load(plugin, bus=bus, scope="t1/s1")`).

A host runs plugins with `python -m rutis <channel> <project>` (it starts that itself); `python -m rutis listen:wss://… --id <id> --peer <controller> <project>` runs a runtime a host on another machine controls (install `rutis[network]`; not on CPython 3.13.6, whose TLS sockets deadlock, [python/cpython#137583](https://github.com/python/cpython/issues/137583): use 3.13.7 or later); `python -m rutis dial:wss://… --id <id> --peer <controller> <project>` connects to that host instead and reconnects by itself (token in `RUTIS_TOKEN`, a further CA in `RUTIS_CA`). Packaged plugins register under the entry point group `rutis.plugins`. Python 3.10 or later; no dependencies.

Start a project with `uvx rutis-host new <name> --lang python`. Guide (Chinese): [docs/guide/python-plugin.md](https://github.com/arcships/rutis/blob/main/docs/guide/python-plugin.md).
