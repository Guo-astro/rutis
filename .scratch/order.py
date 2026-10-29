import asyncio, sys, time
sys.path.insert(0, "/Users/eric8810/Code/rutis/.claude/worktrees/agent-aad5ddc8a34de2a37/python/rutis")
from rutis import define_plugin
from rutis.testing import load


def apply(ctx, config):
    class Greeter:
        async def hello(self, name):
            ctx.emit("greeter/greeted", name)
            return f"Hi, {name}!"

    ctx.provide("greeter", Greeter())


plugin = define_plugin(apply, name="greeter")


async def main():
    inside = after = 0
    for _ in range(int(sys.argv[1])):
        greeted = []
        async with load(plugin) as t:
            t.on("greeter/greeted", greeted.append)
            await t.service("greeter").hello("Ada")
            inside += greeted == ["Ada"]
        after += greeted == ["Ada"]
    print("inside", inside, "after", after)


asyncio.run(main())
