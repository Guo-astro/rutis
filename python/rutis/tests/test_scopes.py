"""Scoped services in the runtime: a name isolated under a label is its
own service, which no other (name, label) pair, nor a global name that
looks like one, reaches."""

import asyncio
import sys
import types
import unittest

import background

from rutis.runner import Runtime, handle_of, scoped_id


def module(name: str, apply) -> str:
    """Register a plugin module `name` whose `apply` is `apply`."""
    plugin = types.ModuleType(name)
    plugin.apply = apply
    sys.modules[name] = plugin
    return name


class ScopedIds(background.TestCase):
    def test_ids_of_different_pairs_differ(self):
        self.assertNotEqual(scoped_id("x@L", None), scoped_id("x", "L"))
        self.assertNotEqual(scoped_id("x", "a#2"), handle_of(scoped_id("x", "a"), 2))
        self.assertEqual(scoped_id("x", None), "x")
        with self.assertRaises(ValueError):
            scoped_id("x\0y", None)
        with self.assertRaises(ValueError):
            scoped_id("x", "L\0M")
        # An empty label is refused: it would pass for no label.
        with self.assertRaises(ValueError):
            scoped_id("x", "")

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

    def test_an_empty_label_is_refused(self):
        def provide(ctx, config):
            ctx.provide("x", "private")

        runtime = Runtime()
        provider = module("empty_label_provider", provide)
        with self.assertRaises(ValueError):
            asyncio.run(runtime.load("p", provider, {}, None, {"x": ""}))
        self.assertNotIn("x", runtime.services)

class Inspect(background.TestCase):
    """The test control method `inspect` (RUTIS_TEST_INSPECT): what the
    runtime holds, and nothing of a row once it is unloaded."""

    def test_inspect_reports_what_a_scoped_row_holds_until_it_unloads(self):
        def apply(ctx, config):
            ctx.provide("x", "private")
            ctx.on("tick", lambda: None)

        runtime = Runtime()
        provider = module("inspect_provider", apply)
        runner = sys.modules["rutis.runner"]
        inspect = runner.INSPECT
        runner.INSPECT = True
        self.addCleanup(setattr, runner, "INSPECT", inspect)

        async def run():
            await runtime.load("p", provider, {}, {"x": {}}, {"x": "L"})
            loaded = runtime._control("inspect", [])
            await runtime.unload("p")
            # rutis lets go of the object it held (its proxy dropped).
            runtime._control("release", [scoped_id("x", "L")])
            return loaded, runtime._control("inspect", [])

        loaded, unloaded = asyncio.run(run())
        self.assertEqual(loaded["rows"], ["p"])
        self.assertEqual(loaded["services"], [scoped_id("x", "L")])
        self.assertEqual(loaded["slots"], [scoped_id("x", "L")])
        self.assertEqual(loaded["handles"], [scoped_id("x", "L")])
        self.assertEqual(loaded["listeners"], 1)
        self.assertEqual(
            unloaded,
            {"rows": [], "services": [], "hosts": [], "slots": [], "handles": [], "listeners": 0},
        )

    def test_without_the_setting_inspect_is_unknown(self):
        runner = sys.modules["rutis.runner"]
        inspect = runner.INSPECT
        runner.INSPECT = False
        self.addCleanup(setattr, runner, "INSPECT", inspect)
        with self.assertRaises(ValueError):
            Runtime()._control("inspect", [])


if __name__ == "__main__":
    unittest.main()
