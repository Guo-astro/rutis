"""rutis.testing refuses a plugin's provide of a name the host provides, as
the host and Cordis do (#684 H4)."""

import asyncio
import unittest

from rutis import define_plugin
from rutis.testing import PluginTestError, load


class Notes:
    async def put(self, key, value):
        pass

    async def get(self, key):
        return None


def wordcount():
    def apply(ctx, config):
        ctx.provide("notes", Notes())

    return define_plugin(apply, name="wordcount")


def run(coroutine):
    return asyncio.run(coroutine)


class Provide(unittest.TestCase):
    def test_providing_a_name_in_services_fails(self):
        async def go():
            with self.assertRaises(PluginTestError) as raised:
                async with load(wordcount(), services={"notes": Notes()}):
                    pass
            self.assertEqual(str(raised.exception), "wordcount: notes is already provided by the host")

        run(go())

    def test_a_name_not_in_services_is_provided_as_before(self):
        async def go():
            async with load(wordcount()) as t:
                self.assertIsNone(await t.service("notes").get("k"))

        run(go())


if __name__ == "__main__":
    unittest.main()
