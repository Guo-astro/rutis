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
            async with load(greeter, bus=bus, id="greeter") as t, load(shout, bus=bus, id="shout"):
                self.assertEqual(await t.service("greeter").hello("Ada", "sv"), "Hej, Ada!")
                self.assertEqual(await t.service("greeter").hello("Bo", "en"), "Hello, Bo!")
                self.assertEqual(await t.serial("greeting/pick", "sv"), "Hej")  # the test can emit too
                self.assertEqual(said, ["Ada", "Bo"])    # emits go through the host: delivered once the serial after them is back

        asyncio.run(go())
