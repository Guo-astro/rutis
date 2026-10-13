"""rutis.testing: plugins tested without a host."""

import asyncio
import functools
import unittest
from dataclasses import dataclass

import background

from rutis import AggregateError, PLUGIN_API, RemoteError, define_plugin
from rutis.testing import PluginTestError, load


@dataclass
class Reading:
    city: str
    degrees: int


class Weather:
    def __init__(self, llm, city):
        self.llm, self.city = llm, city

    async def today(self):
        return f"{await self.llm.ask(self.city)} in {self.city}"

    def reading(self):
        return Reading(self.city, 21)


class Llm:
    async def ask(self, question):
        return "sunny"


def weather_plugin():
    order = []

    def apply(ctx, config):
        ctx.provide("weather", Weather(ctx.use("llm"), config["city"]))
        ctx.effect(lambda: order.append("effect"))
        return lambda: order.append("returned")

    return define_plugin(apply, inject=["llm"], provides={"weather": Weather}), order


def run(coroutine):
    return asyncio.run(coroutine)


class Testing(background.TestCase):
    def test_a_plugin_runs_with_its_services_and_unloads_latest_first(self):
        plugin, order = weather_plugin()

        async def go():
            async with load(plugin, config={"city": "Oslo"}, services={"llm": Llm()}) as t:
                self.assertEqual(await t.service("weather").today(), "sunny in Oslo")
                # A dataclass crosses as data.
                self.assertEqual(t.service("weather").reading(), {"city": "Oslo", "degrees": 21})
                self.assertEqual(t.provided(), ["weather"])
            self.assertEqual(order, ["returned", "effect"])

        run(go())

    def test_undeclared_and_missing_services_are_errors(self):
        plugin, _ = weather_plugin()

        async def missing():
            async with load(plugin, config={"city": "x"}):
                pass

        with self.assertRaisesRegex(PluginTestError, "injects llm"):
            run(missing())

        sneaky = define_plugin(lambda ctx, config: ctx.use("llm"))

        async def undeclared():
            async with load(sneaky, services={"llm": Llm()}):
                pass

        with self.assertRaisesRegex(PluginTestError, "without declaring it in inject"):
            run(undeclared())

    def test_strict_mode_crosses_values_as_between_processes(self):
        class Store:
            def __init__(self):
                self.kept = []

            def put(self, item):
                self.kept.append(item)

            def first(self):
                return self.kept[0]

            def last(self):
                return self.kept[-1]

            def big(self):
                return {"n": [2**53]}

            def wrong(self):
                return asyncio.sleep(0)

        plugin = define_plugin(
            lambda ctx, config: ctx.provide("store", Store()),
            provides={"store": {"put": "sync", "first": "sync", "last": "sync", "big": "sync", "wrong": "sync"}},
        )

        async def go():
            async with load(plugin) as t:
                item = {"n": 1}
                t.service("store").put(item)
                item["n"] = 2
                self.assertEqual(t.service("store").first(), {"n": 1})
                with self.assertRaisesRegex(PluginTestError, "cannot cross"):
                    t.service("store").put({1, 2})
                # Nesting: 64 levels cross, 65 do not (#279).
                nest = lambda depth: functools.reduce(lambda inner, _: [inner], range(depth), "bottom")
                t.service("store").put(nest(64))
                with self.assertRaisesRegex(PluginTestError, "nested deeper than 64 levels cannot cross"):
                    t.service("store").put(nest(65))
                # Integers: within ±(2^53−1) cross, beyond it do not (#356).
                for big in (2**53, -(2**53), [1, {"n": 2**64 + 1}]):
                    with self.assertRaisesRegex(PluginTestError, r"beyond ±\(2\^53-1\) cannot cross"):
                        t.service("store").put(big)
                safe = [2**53 - 1, -(2**53 - 1), True]
                t.service("store").put(safe)
                self.assertEqual(t.service("store").last(), safe)
                with self.assertRaisesRegex(PluginTestError, r"store.big.*beyond ±\(2\^53-1\) cannot cross"):
                    t.service("store").big()
                with self.assertRaisesRegex(PluginTestError, "declared sync but returned an awaitable"):
                    t.service("store").wrong()

        run(go())

    def test_a_plugin_that_needs_a_newer_plugin_api_is_refused(self):
        plugin, _ = weather_plugin()
        plugin.api = PLUGIN_API + 1

        async def go():
            async with load(plugin, services={"llm": Llm()}):
                pass

        with self.assertRaisesRegex(PluginTestError, "needs plugin API"):
            run(go())

    def test_host_events_wait_for_all_listeners_and_cross_aggregate_errors(self):
        async def go():
            gate, started = asyncio.Event(), asyncio.Event()
            called = []

            def apply(ctx, config):
                def fail(value):
                    value["n"] += 1
                    raise ValueError("sync")

                async def wait(value):
                    started.set()
                    await gate.wait()
                    raise TypeError("async")

                ctx.on("tick", fail)
                ctx.on("tick", wait)
                ctx.on("tick", lambda value: called.append(value["n"]))

            async with load(define_plugin(apply), emits=["tick"]) as t:
                value = {"n": 1}
                done = asyncio.create_task(t.emit("tick", value))
                await started.wait()
                self.assertEqual((called, value, done.done()), ([2], {"n": 1}, False))
                gate.set()
                with self.assertRaises(RemoteError) as failure:
                    await done
                error = failure.exception
                self.assertEqual(error.name, "AggregateError")
                nodes = error.graph["nodes"]
                errors = nodes[error.graph["root"]["value"]]["errors"]
                self.assertEqual([(nodes[ref["value"]]["name"], nodes[ref["value"]]["message"]) for ref in errors],
                                 [("ValueError", "sync"), ("TypeError", "async")])

        run(go())

    def test_event_disposers_and_failed_cleanup_remove_every_listener(self):
        async def go():
            seen, saved = [], []

            def apply(ctx, config):
                callback = lambda value: seen.append(value)
                ctx.on("early", callback)
                def cleanup():
                    ctx.emit("early", 99)
                    raise asyncio.CancelledError()
                ctx.effect(cleanup)
                off = ctx.on("tick", callback)
                ctx.on("tick", callback)
                saved.extend([ctx, off])

            with self.assertRaises(asyncio.CancelledError):
                async with load(define_plugin(apply), emits=["tick"]) as t:
                    await t.emit("tick", 1)
                    saved[1]()
                    saved[1]()
                    self.assertIsNone(saved[0].emit("tick", 2))
                    self.assertEqual(seen, [1, 1, 2])
                    for operation in (saved[0].on, saved[0].emit):
                        with self.assertRaisesRegex(ValueError, "internal"):
                            operation("internal/test", lambda: None)
            saved[0].emit("early", 3)
            self.assertEqual(seen, [1, 1, 2], "cleanup emissions cannot reach the unloading plugin")
            with self.assertRaisesRegex(PluginTestError, "unloaded"):
                saved[0].on("tick", lambda: None)

        run(go())

    def test_unhandled_async_emission_fails_the_test_at_unload(self):
        order = []

        async def fail():
            order.append("listener")
            await asyncio.sleep(0)
            raise ValueError("async event")

        async def go():
            def apply(ctx, config):
                ctx.on("tick", fail)
                ctx.emit("tick")
                order.append("emitter")

            with self.assertRaises(AggregateError) as failure:
                async with load(define_plugin(apply)):
                    pass
            self.assertEqual(str(failure.exception.errors[0]), "async event")
            self.assertEqual(order, ["listener", "emitter"], "ctx.emit starts async listeners eagerly")

        run(go())


if __name__ == "__main__":
    unittest.main()
