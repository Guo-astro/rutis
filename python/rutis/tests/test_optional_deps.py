"""Optional services (docs/design-optional-deps-2026-10-28.md §4.1, §4.5,
§4.6): a plugin declares beside `inject` the services it can do without
(`optional`), rutis never waits for them, and `ctx.get` reads them, None
when not there; `use` counts them as declared. guarantee: sdk/optional-deps"""

import asyncio
import sys
import types
import unittest

import background

from rutis import BindingError, define_plugin
from rutis.plugin import load as load_module
from rutis.runner import Runtime, undeclared_get, undeclared_use
from rutis.testing import PluginTestError, load


def module(name: str, apply, inject=(), optional=None) -> str:
    """Register a plugin module `name` whose `apply` is `apply`."""
    plugin = types.ModuleType(name)
    plugin.apply = apply
    plugin.inject = list(inject)
    if optional is not None:
        plugin.optional = optional
    sys.modules[name] = plugin
    return name


def caught(run) -> tuple:
    """What `run()` raises: its class, text, code and data."""
    try:
        run()
    except Exception as error:  # noqa: BLE001 - what it raises is the point
        return type(error), str(error), getattr(error, "code", None), getattr(error, "data", None)
    raise AssertionError("it did not raise")


class Llm:
    def ask(self, question):
        return f"re: {question}"


class Declaring(background.TestCase):
    def test_optional_is_a_module_attribute_or_a_define_plugin_argument(self):
        self.assertEqual(load_module(sys.modules[module("opt_none", lambda ctx, config: None)]).optional, [])
        self.assertEqual(
            load_module(sys.modules[module("opt_some", lambda ctx, config: None, optional=("llm",))]).optional,
            ["llm"],
        )
        self.assertEqual(define_plugin(lambda ctx, config: None).optional, [])
        self.assertEqual(define_plugin(lambda ctx, config: None, optional=("llm",)).optional, ["llm"])

    def test_a_name_that_is_not_a_string_is_refused(self):
        with self.assertRaisesRegex(TypeError, "^opt_bad.optional must be a list of service names$"):
            load_module(sys.modules[module("opt_bad", lambda ctx, config: None, optional=[1])])
        with self.assertRaisesRegex(TypeError, "^opt_str.optional must be a list of service names$"):
            load_module(sys.modules[module("opt_str", lambda ctx, config: None, optional="llm")])
        with self.assertRaisesRegex(TypeError, "^optional must be a list of service names$"):
            define_plugin(lambda ctx, config: None, optional=[1])

    def test_the_schema_reports_optional(self):
        runtime = Runtime()
        self.assertEqual(runtime.describe(module("opt_plain", lambda ctx, config: None))["optional"], [])
        described = runtime.describe(module("opt_described", lambda ctx, config: None, optional=["llm"]))
        self.assertEqual(described["optional"], ["llm"])
        defined = define_plugin(lambda ctx, config: None, inject=["notes"], optional=["llm"])
        self.assertEqual(runtime.describe_definition(defined)["optional"], ["llm"])
        self.assertEqual(runtime.describe_definition(define_plugin(lambda ctx, config: None))["optional"], [])


class Reading(background.TestCase):
    def test_get_use_and_what_they_say(self):
        seen = {}

        def provide(ctx, config):
            ctx.provide("llm", Llm())

        def read(ctx, config):
            seen["present"] = ctx.get("llm")
            seen["use present"] = caught(lambda: ctx.use("llm"))
            seen["absent"] = ctx.get("vision")
            seen["row"] = ctx.get("extra")
            seen["use absent"] = caught(lambda: ctx.use("vision"))
            seen["use present"] = caught(lambda: ctx.use("llm"))
            seen["get undeclared"] = caught(lambda: ctx.get("nope"))
            seen["use undeclared"] = caught(lambda: ctx.use("nope"))

        runtime = Runtime()

        async def run():
            await runtime.load("p", module("opt_provider", provide), None, None)
            # The row's own `inject` declares it too.
            await runtime.load(
                "r", module("opt_reader", read, optional=["llm", "vision"]), None, None, inject=["extra"]
            )

        asyncio.run(run())
        self.assertIsInstance(seen["present"], Llm)
        self.assertIsNone(seen["absent"])
        self.assertIsNone(seen["row"])
        # `use` reads only what `inject` lists, as Rust's `require`: an
        # optional one, there or not, is told to go in `inject`.
        self.assertEqual(
            seen["use present"], (LookupError, undeclared_use("llm"), "not-declared", {"service": "llm"})
        )
        self.assertEqual(
            seen["use absent"], (LookupError, undeclared_use("vision"), "not-declared", {"service": "vision"})
        )
        # Even when it is there.
        self.assertEqual(seen["use present"], (LookupError, undeclared_use("llm"), "not-declared", {"service": "llm"}))
        self.assertEqual(
            seen["get undeclared"],
            (
                LookupError,
                "service nope is not available to this plugin, which does not declare it: "
                "add 'nope' to its optional (or to its inject, if it cannot run without it)",
                "not-declared",
                {"service": "nope"},
            ),
        )
        self.assertEqual(seen["get undeclared"][1], undeclared_get("nope"))
        # `use`'s own text is unchanged.
        self.assertEqual(
            seen["use undeclared"], (LookupError, undeclared_use("nope"), "not-declared", {"service": "nope"})
        )

    def test_a_neighbours_service_is_not_declared(self):
        # A name declared nowhere is refused even when another row here
        # provides it (#676): the answer does not depend on who else is in
        # the process.
        seen = {}

        def provide(ctx, config):
            ctx.provide("neighbour", Llm())

        def read(ctx, config):
            seen["use"] = caught(lambda: ctx.use("neighbour"))
            seen["get"] = caught(lambda: ctx.get("neighbour"))

        runtime = Runtime()

        async def run():
            await runtime.load("p", module("neighbour_provider", provide), None, None)
            await runtime.load("r", module("neighbour_reader", read), None, None)

        asyncio.run(run())
        self.assertEqual(
            seen["use"], (LookupError, undeclared_use("neighbour"), "not-declared", {"service": "neighbour"})
        )
        self.assertEqual(
            seen["get"], (LookupError, undeclared_get("neighbour"), "not-declared", {"service": "neighbour"})
        )

    def test_a_plugin_may_use_and_get_what_it_provides_itself(self):
        # Providing a name declares it, for use and get alike, while it is
        # provided; withdrawn, it is like a name never declared.
        seen = {}

        def apply(ctx, config):
            withdraw = ctx.provide("own", Llm())
            seen["use"] = type(ctx.use("own"))
            seen["get"] = type(ctx.get("own"))
            withdraw()
            seen["use withdrawn"] = caught(lambda: ctx.use("own"))
            seen["get withdrawn"] = caught(lambda: ctx.get("own"))

        runtime = Runtime()
        asyncio.run(runtime.load("o", module("own_provider", apply), None, None))
        self.assertEqual(
            seen,
            {
                "use": Llm,
                "get": Llm,
                "use withdrawn": (LookupError, undeclared_use("own"), "not-declared", {"service": "own"}),
                "get withdrawn": (LookupError, undeclared_get("own"), "not-declared", {"service": "own"}),
            },
        )

    def test_a_service_whose_row_stopped_is_unavailable_to_who_holds_it(self):
        def provide(ctx, config):
            ctx.provide("llm", Llm())

        runtime = Runtime()
        runtime._control("mount", {"announce": True})

        async def run():
            await runtime.load("p", module("opt_held", provide), None, {"llm": {"ask": "sync"}})
            handle = runtime.slots["llm"].handle
            self.assertEqual(runtime.dispatch(handle, "ask", ["x"]), "re: x")
            await runtime.unload("p")
            return handle

        handle = asyncio.run(run())
        # Retired, not released: the handle is known, its object gone.
        self.assertIn(handle, runtime.handles)
        for run in (lambda: runtime.dispatch(handle, "ask", ["x"]), lambda: runtime._control("get", [handle, "ask"])):
            self.assertEqual(
                caught(run), (BindingError, "service llm is not available", "unavailable", {"service": "llm"})
            )

    def test_a_service_its_row_replaced_keeps_its_object_until_the_row_unloads(self):
        # As a snapshot of a service does in Rust (boundary/5.1).
        held = {}

        class Coin:
            def __init__(self, side):
                self.side = side

            def toss(self):
                return self.side

        def provide(ctx, config):
            def swap():
                held["withdraw"]()
                held["withdraw"] = ctx.provide("flip", Coin("tails"))

            held["swap"] = swap
            held["withdraw"] = ctx.provide("flip", Coin("heads"))

        runtime = Runtime()
        runtime._control("mount", {"announce": True})

        async def run():
            await runtime.load("p", module("opt_flip", provide), None, {"flip": {"toss": "sync"}})
            old = runtime.slots["flip"].handle
            held["swap"]()
            new = runtime.slots["flip"].handle
            self.assertNotEqual(new, old)
            self.assertEqual(runtime.dispatch(new, "toss", []), "tails")
            self.assertEqual(runtime.dispatch(old, "toss", []), "heads")
            await runtime.unload("p")
            return old

        old = asyncio.run(run())
        self.assertEqual(
            caught(lambda: runtime.dispatch(old, "toss", [])),
            (BindingError, "service flip is not available", "unavailable", {"service": "flip"}),
        )


    def test_a_handle_its_row_replaced_answers_with_its_own_objects_methods(self):
        # An announced service offers its object's methods: a handle its
        # row replaced keeps the old object's.
        held = {}

        class Coin:
            def toss(self):
                return "heads"

            def swap(self):
                held["withdraw"]()
                held["withdraw"] = held["ctx"].provide("coin", Spinner())

        class Spinner:
            def spin(self):
                return "spun"

        def provide(ctx, config):
            held["ctx"] = ctx
            held["withdraw"] = ctx.provide("coin", Coin())

        runtime = Runtime()
        runtime._control("mount", {"announce": True})

        async def run():
            await runtime.load("c", module("opt_reshape", provide), None, None)
            old = runtime.slots["coin"].handle
            runtime.dispatch(old, "swap", [])
            new = runtime.slots["coin"].handle
            self.assertEqual(runtime.dispatch(new, "spin", []), "spun")
            self.assertEqual(runtime.dispatch(old, "toss", []), "heads")
            self.assertEqual(
                caught(lambda: runtime.dispatch(old, "spin", []))[:2],
                (BindingError, "coin has no method spin (methods: swap, toss)"),
            )

        asyncio.run(run())

class Testing(background.TestCase):
    def test_the_testing_tool_reads_optional_services(self):
        seen = {}

        def read(ctx, config):
            seen["llm"] = ctx.get("llm")
            if seen["llm"] is None:
                seen["use"] = caught(lambda: ctx.use("llm"))
            seen["get undeclared"] = caught(lambda: ctx.get("other"))

        summary = define_plugin(read, inject=["notes"], optional=["llm"])

        async def without():
            # Not given: load still succeeds.
            async with load(summary, services={"notes": object()}):
                pass

        asyncio.run(without())
        self.assertIsNone(seen["llm"])
        self.assertEqual(seen["use"], (LookupError, undeclared_use("llm"), "not-declared", {"service": "llm"}))
        self.assertEqual(
            seen["get undeclared"], (LookupError, undeclared_get("other"), "not-declared", {"service": "other"})
        )

        async def given():
            # Given: `get` finds it.
            async with load(summary, services={"notes": object(), "llm": Llm()}):
                return seen["llm"].ask("x")

        self.assertEqual(asyncio.run(given()), "re: x")

        both = define_plugin(lambda ctx, config: None, inject=["llm"], optional=["llm"])

        async def in_both():
            async with load(both, services={"llm": Llm()}):
                pass

        with self.assertRaises(PluginTestError) as refused:
            asyncio.run(in_both())
        # The host refuses the row, saying why.
        self.assertTrue(
            str(refused.exception).endswith(": llm is in both inject and optional; keep it in one"),
            str(refused.exception),
        )


if __name__ == "__main__":
    unittest.main()
