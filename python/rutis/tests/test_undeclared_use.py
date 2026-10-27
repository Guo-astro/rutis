"""A service neither the plugin nor its row declares in `inject`, and that
is not there for it: `use` says what to declare, not that the service does
not exist (#676). guarantee: sdk/undeclared-use"""

import asyncio
import sys
import types
import unittest

import background

from rutis.runner import Runtime, undeclared_use


def module(name: str, apply, inject=()) -> str:
    """Register a plugin module `name` whose `apply` is `apply`."""
    plugin = types.ModuleType(name)
    plugin.apply = apply
    plugin.inject = list(inject)
    sys.modules[name] = plugin
    return name


class UndeclaredUse(background.TestCase):
    def test_a_service_not_in_inject_is_named_with_the_fix(self):
        seen = {}

        def read(ctx, config):
            try:
                ctx.use(config["name"])
            except LookupError as error:
                seen[config["as"]] = str(error)
                seen[config["as"] + " code"] = (error.code, error.data)

        runtime = Runtime()
        sneaky = module("undeclared_sneaky", read)
        declared = module("undeclared_declared", read, inject=["gone"])

        async def run():
            await runtime.load("s", sneaky, {"name": "tsvc", "as": "sneaky"}, None)
            await runtime.load("d", declared, {"name": "gone", "as": "declared"}, None)
            # The row's own `inject` declares it too.
            await runtime.load(
                "r", sneaky, {"name": "extra", "as": "row"}, None, inject=["extra"]
            )

        asyncio.run(run())
        self.assertEqual(
            seen["sneaky"],
            "service tsvc is not available to this plugin, which does not declare it "
            "in inject: add 'tsvc' to its inject",
        )
        self.assertEqual(seen["sneaky"], undeclared_use("tsvc"))
        self.assertEqual(seen["declared"], "service gone is not available")
        self.assertEqual(seen["row"], "service extra is not available")
        # The codes a program decides by (#677).
        self.assertEqual(seen["sneaky code"], ("not-declared", {"service": "tsvc"}))
        self.assertEqual(seen["declared code"], ("unavailable", {"service": "gone"}))
        self.assertEqual(seen["row code"], ("unavailable", {"service": "extra"}))


if __name__ == "__main__":
    unittest.main()
