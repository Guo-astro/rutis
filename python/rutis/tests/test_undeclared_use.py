"""A service a plugin does not declare in `inject` is not given to it,
running or not: `use` says so and how to fix it, not that the service does
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
            "the plugin uses tsvc without declaring it in inject: add 'tsvc' to its inject "
            "(rutis gives a plugin only the services it declares, running or not)",
        )
        self.assertEqual(seen["sneaky"], undeclared_use("tsvc"))
        self.assertEqual(seen["declared"], "service gone is not available")
        self.assertEqual(seen["row"], "service extra is not available")


if __name__ == "__main__":
    unittest.main()
