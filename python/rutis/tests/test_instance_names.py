"""Per-row service names retain native objects and survive updates."""
import types
import unittest

from rutis.runner import Runtime, Context


class InstanceNamesTests(unittest.IsolatedAsyncioTestCase):
    async def test_isolation_update_and_unload(self):
        runtime = Runtime()
        module = types.ModuleType("instance_test")
        module.apply = lambda ctx, config: ctx.provide("document", config)
        runtime.module = lambda _: module
        first, second = object(), object()
        exports = {"document": {}}
        await runtime.load("a", "test", first, exports, {"document": "instance-a"})
        await runtime.load("b", "test", second, exports, {"document": "instance-b"})
        self.assertIs(Context(runtime, runtime.rows["a"]).use("document"), first)
        self.assertIs(Context(runtime, runtime.rows["b"]).use("document"), second)
        replacement = object()
        await runtime.update("a", replacement)
        self.assertIs(Context(runtime, runtime.rows["a"]).use("document"), replacement)
        await runtime.unload("a")
        self.assertNotIn("instance-a", runtime.services)
        self.assertIs(runtime.lookup("instance-b"), second)
        await runtime.unload("b")

    async def test_invalid_mapping_does_not_load(self):
        runtime = Runtime()
        with self.assertRaises(ValueError):
            await runtime.load("a", "test", None, {}, {"document": "bad#name"})
        self.assertFalse(runtime.rows)
