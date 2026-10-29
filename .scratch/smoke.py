import asyncio, sys, time
sys.path.insert(0, "/Users/eric8810/Code/rutis/.claude/worktrees/agent-aad5ddc8a34de2a37/python/rutis")
from rutis import define_plugin
from rutis.testing import load, Bus


class Llm:
    async def ask(self, q):
        return f"sunny in {q}"


def apply(ctx, config):
    llm = ctx.use("llm")

    class Weather:
        async def today(self):
            return await llm.ask(config["city"])

        def unit(self):
            return "C"

    ctx.provide("weather", Weather())
    ctx.on("weather/ask", lambda city: "yes" if city == config["city"] else None)


plugin = define_plugin(apply, inject=["llm"], provides={"weather": {"today": "async", "unit": "sync"}},
                       config={"type": "object", "properties": {"city": {"type": "string", "default": "Oslo"}}}, name="weather")


async def main():
    t0 = time.time()
    async with load(plugin, services={"llm": Llm()}) as t:
        print(t.row, await t.service("weather").today(), t.service("weather").unit(), t.provided())
        print("serial", await t.serial("weather/ask", "Oslo"), t.bail("weather/ask", "Oslo"))
        heard = []
        t.on("x/said", heard.append)
        t.emit("x/said", 1)
        await asyncio.sleep(0.05)
        print("heard", heard)
        await t.update({"city": "Bergen"})
        print(await t.service("weather").today())
        try:
            await t.update({"city": 3})
        except Exception as e:
            print("refused:", type(e).__name__, e)
    print("took", time.time() - t0)
    try:
        async with load(plugin):
            pass
    except Exception as e:
        print("missing:", e)
    try:
        async with load(plugin, config={"city": 5}, services={"llm": Llm()}):
            pass
    except Exception as e:
        print("bad config:", e)
    print("took", time.time() - t0)


asyncio.run(main())
