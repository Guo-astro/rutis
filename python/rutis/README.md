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

Test it without a host:

```python
from rutis.testing import load

async with load(plugin, config={"city": "Oslo"}, services={"llm": FakeLlm()}) as t:
    assert await t.service("weather").today() == "sunny in Oslo"
```

A host runs plugins with `python -m rutis <channel> <project>` (it starts that itself); `python -m rutis listen:wss://… --id <id> --peer <controller> <project>` runs a runtime a host on another machine controls (install `rutis[network]`; not on CPython 3.13.6, whose TLS sockets deadlock, [python/cpython#137583](https://github.com/python/cpython/issues/137583): use 3.13.7 or later); `python -m rutis dial:wss://… --id <id> --peer <controller> <project>` connects to that host instead and reconnects by itself (token in `RUTIS_TOKEN`, a further CA in `RUTIS_CA`). Packaged plugins register under the entry point group `rutis.plugins`. Python 3.10 or later; no dependencies.

Start a project with `uvx rutis-host new <name> --lang python`. Guide (Chinese): [docs/guide/python-plugin.md](https://github.com/arcships/rutis/blob/main/docs/guide/python-plugin.md).
