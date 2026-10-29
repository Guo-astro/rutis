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
                # 测试也能发出；parallel 等全部监听器做完，之前的 emit 也已送到
                await t.parallel("greeter/greeted", "Bo")
                self.assertEqual(greeted, ["Ada", "Bo"])
                self.assertEqual(await t.service("greeter").count(), 2)

        asyncio.run(go())


if __name__ == "__main__":
    unittest.main()
