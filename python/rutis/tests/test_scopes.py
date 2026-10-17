"""Scoped services in the runtime: a name isolated under a label is its
own service, which no other (name, label) pair, nor a global name that
looks like one, reaches."""

import asyncio
import sys
import subprocess
import types
import unittest

import background

from rutis.runner import Runtime, handle_of, scoped_id
from rutis import AggregateError
from support import HANG_GUARD


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

    # guarantee: sdk/leaf-events (shared rows, update, failed apply, unload)
    def test_events_share_rows_and_leave_on_update_failure_and_unload(self):
        seen, contexts = [], []

        def apply(ctx, config):
            contexts.append(ctx)
            ctx.on("tick", lambda value: seen.append([config.get("label"), value]))
            if config.get("fail"):
                raise ValueError("apply failed")
            if config.get("cancel"):
                async def cleanup():
                    raise asyncio.CancelledError()
                ctx.effect(cleanup)

        runtime = Runtime()
        entry = module("event_rows", apply)

        async def go():
            self.assertIn("events", runtime.dispatch("", "mount", {"emits": ["tick", "failure"]})["features"])
            # The sentence of rutis's own check (`Process::mount`), naming every internal event.
            with self.assertRaisesRegex(ValueError, r"^framework-internal events do not cross the boundary \(mount emits\): internal/test, internal/other$"):
                runtime.dispatch("", "mount", {"emits": ["tick", "internal/test", "internal/other"]})
            await runtime.load("a", entry, {"label": "first"}, None)
            await runtime.load("b", entry, {"label": "other"}, None)
            await runtime.update("a", {"label": "updated"})
            with self.assertRaisesRegex(RuntimeError, "not active"):
                contexts[0].on("tick", lambda: None)
            with self.assertRaisesRegex(ValueError, "apply failed"):
                await runtime.load("failed", entry, {"fail": True}, None)
            await runtime.dispatch("", "emit", ["tick", [1]])
            self.assertEqual(seen, [["other", 1], ["updated", 1]])
            def fail():
                raise ValueError("host listener failed")
            contexts[1].on("failure", fail)
            with self.assertRaises(AggregateError):
                await runtime.dispatch("", "emit", ["failure", []])
            await runtime.load("cancelled", entry, {"cancel": True}, None)
            with self.assertRaises(asyncio.CancelledError):
                await runtime.unload("cancelled")
            await runtime.dispatch("", "emit", ["tick", [2]])
            self.assertEqual(seen, [["other", 1], ["updated", 1], ["other", 2], ["updated", 2]])
            await runtime.dispose()
            contexts[1].emit("tick", 3)
            self.assertEqual(len(seen), 4)

        asyncio.run(go())

    # guarantee: sdk/leaf-events (an unhandled async listener ends the process)
    def test_unhandled_async_plugin_event_exits_the_runtime_process(self):
        script = """
import asyncio, sys, types
from rutis.runner import Runtime
async def fail():
    await asyncio.sleep(0)
    raise ValueError('unhandled leaf event')
def apply(ctx, config):
    ctx.on('tick', fail)
    ctx.emit('tick')
plugin = types.ModuleType('fatal_event')
plugin.apply = apply
sys.modules[plugin.__name__] = plugin
async def run():
    await Runtime().load('fatal', plugin.__name__, {}, None)
    await asyncio.Event().wait()
asyncio.run(run())
"""
        result = subprocess.run([sys.executable, "-c", script], capture_output=True, text=True, timeout=HANG_GUARD)
        self.assertEqual(result.returncode, 1)
        self.assertIn("ValueError: unhandled leaf event", result.stderr)


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
