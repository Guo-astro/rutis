import asyncio
import unittest

from rutis import define_plugin
from rutis.testing import Bus, load


def greeter_apply(ctx, config):
    # 返回 None 或 False 是"没有结果"，serial 接着问下一个监听器
    ctx.on("greeting/pick", lambda lang: "Hej" if lang == "sv" else None)

    class Greeter:
        async def hello(self, name, lang):
            word = await ctx.serial("greeting/pick", lang) or "Hello"
            # terminal：链的最里面，收到事件参数
            text = await ctx.waterfall("greeting/format", f"{word}, {name}", terminal=lambda text: text)
            ctx.emit("greeting/said", name)      # 立即返回，不等监听器
            return text

    ctx.provide("greeter", Greeter())


greeter = define_plugin(greeter_apply)
said = []


def shout_apply(ctx, config):
    async def exclaim(text, next):           # waterfall 的监听器最后一个参数是 next
        return f"{await next()}!"

    ctx.on("greeting/format", exclaim)
    ctx.on("greeting/said", said.append)


shout = define_plugin(shout_apply)


class Events(unittest.TestCase):
    def test_plugins_hear_each_other(self):
        async def go():
            bus = Bus()                          # 同一个 bus 上的插件互相听得到
            async with load(greeter, bus=bus, id="greeter") as t, load(shout, bus=bus, id="shout"):
                self.assertEqual(await t.service("greeter").hello("Ada", "sv"), "Hej, Ada!")
                self.assertEqual(await t.service("greeter").hello("Bo", "en"), "Hello, Bo!")
                self.assertEqual(await t.serial("greeting/pick", "sv"), "Hej")  # 测试自己也能发出
                self.assertEqual(said, ["Ada", "Bo"])    # emit 经宿主，在它之后的 serial 回来时已送到

        asyncio.run(go())
