"""Unloading runs one stack in reverse: cleanups and the withdrawals of the
services a row provides, in the order they were registered (Rust's
drain_effects; docs/design-unload-order-2026-11-01.md).

guarantee: sdk/unload-order
"""

import asyncio
import contextlib
import io
import sys
import types
import unittest

import background

from rutis.runner import Runtime


class Tally:
    def __init__(self, n):
        self.n = n

    def count(self):
        return self.n


def module(name, apply, inject=()) -> str:
    """Register a plugin module `name` whose `apply` is `apply`."""
    plugin = types.ModuleType(name)
    plugin.apply = apply
    plugin.inject = list(inject)
    sys.modules[name] = plugin
    return name


def read(ctx, name="tally") -> str:
    """What reading `name` through `ctx` gives: the count, or the error."""
    try:
        return f"count {ctx.use(name).count()}"
    except Exception as error:  # noqa: BLE001 - what the cleanup saw
        return f"error {getattr(error, 'code', type(error).__name__)}"


class Order(background.TestCase):
    def runtime(self):
        runtime = Runtime()
        runtime._control("mount", {"announce": True})
        return runtime

    def test_a_cleanup_after_provide_reads_it_and_one_before_does_not(self):
        log = []

        def apply(ctx, config):
            ctx.effect(lambda: log.append(f"before: {read(ctx)}"))
            ctx.provide("tally", Tally(7))
            ctx.effect(lambda: log.append(f"after: {read(ctx)}"))

        async def run():
            runtime = self.runtime()
            await runtime.load("o", module("order_around", apply), None, None)
            await runtime.unload("o")

        asyncio.run(run())
        self.assertEqual(log, ["after: count 7", "before: error not-declared"])

    def test_another_row_loses_it_when_the_unload_starts(self):
        log = []
        holder = {}

        def provider(ctx, config):
            ctx.provide("tally", Tally(2))

            # Registered after the provide: it runs before the withdrawal, and
            # at that same moment the other row no longer sees the service.
            # Reading it only after the unload would not prove this: by then
            # the registration is gone for everyone.
            def during():
                log.append(f"own: {read(ctx)}")
                log.append(f"other: {read(holder['ctx'])}")

            ctx.effect(during)

        def user(ctx, config):
            holder["ctx"] = ctx

        async def run():
            runtime = self.runtime()
            await runtime.load("p", module("loser_provider", provider), None, None)
            await runtime.load("u", module("loser_user", user, inject=["tally"]), None, None)
            log.append(f"before: {read(holder['ctx'])}")
            await runtime.unload("p")
            log.append(f"after: {read(holder['ctx'])}")

        asyncio.run(run())
        self.assertEqual(
            log,
            [
                "before: count 2",
                "own: count 2",
                "other: error unavailable",
                "after: error unavailable",
            ],
        )

    def test_a_cleanup_registering_one_is_refused(self):
        written = io.StringIO()
        ran = []

        def apply(ctx, config):
            def outer():
                ctx.effect(lambda: ran.append("inner"))

            ctx.effect(outer)

        async def run():
            runtime = self.runtime()
            await runtime.load("nested", module("nested_cleanup", apply), None, None)
            with contextlib.redirect_stderr(written):
                await runtime.unload("nested")

        asyncio.run(run())
        self.assertEqual(ran, [])
        self.assertEqual(written.getvalue(), "rutis: row nested is not active: cleanup is not registered\n")

    def test_an_old_withdrawal_leaves_a_new_registration_alone(self):
        log = []

        def apply(ctx, config):
            withdraw = ctx.provide("tally", Tally(5))
            withdraw()
            ctx.provide("tally", Tally(5))
            # The first registration's withdrawal is on the stack too; it
            # removes nothing now.
            log.append(f"during: {read(ctx)}")
            ctx.effect(lambda: log.append(f"cleanup: {read(ctx)}"))

        async def run():
            runtime = self.runtime()
            await runtime.load("a", module("again", apply), None, None)
            await runtime.unload("a")

        asyncio.run(run())
        self.assertEqual(log, ["during: count 5", "cleanup: count 5"])

    def test_the_service_is_gone_afterwards(self):
        def apply(ctx, config):
            ctx.provide("tally", Tally(1))

        async def run():
            runtime = self.runtime()
            await runtime.load("g", module("gone_after", apply), None, None)
            self.assertIn("tally", runtime.services)
            await runtime.unload("g")
            self.assertNotIn("tally", runtime.services)

        asyncio.run(run())


if __name__ == "__main__":
    unittest.main()
