"""Scoped services in the runtime: a name isolated under a label is its
own service, which no other (name, label) pair, nor a global name that
looks like one, reaches."""

import asyncio
import sys
import types
import unittest

from rutis.runner import Runtime, handle_of, scoped_id


def module(name: str, apply) -> str:
    """Register a plugin module `name` whose `apply` is `apply`."""
    plugin = types.ModuleType(name)
    plugin.apply = apply
    sys.modules[name] = plugin
    return name


class ScopedIds(unittest.TestCase):
    def test_ids_of_different_pairs_differ(self):
        self.assertNotEqual(scoped_id("x@L", None), scoped_id("x", "L"))
        self.assertNotEqual(scoped_id("x", "a#2"), handle_of(scoped_id("x", "a"), 2))
        self.assertEqual(scoped_id("x", None), "x")
        with self.assertRaises(ValueError):
            scoped_id("x\0y", None)
        with self.assertRaises(ValueError):
            scoped_id("x", "L\0M")

    def test_a_global_name_cannot_reach_a_scoped_service(self):
        seen = {}

        def provide(ctx, config):
            ctx.provide("x", "private")

        def read(ctx, config):
            try:
                seen[config["as"]] = ctx.use(config["name"])
            except LookupError:
                seen[config["as"]] = None

        runtime = Runtime()
        provider = module("scope_provider", provide)
        reader = module("scope_reader", read)

        async def run():
            await runtime.load("p", provider, {}, None, {"x": "L"})
            await runtime.load("outside", reader, {"name": "x@L", "as": "outside"}, None, {})
            await runtime.load("inside", reader, {"name": "x", "as": "inside"}, None, {"x": "L"})

        asyncio.run(run())
        self.assertEqual(seen, {"outside": None, "inside": "private"})


if __name__ == "__main__":
    unittest.main()
