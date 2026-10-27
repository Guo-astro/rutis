"""rutis.testing: plugins tested without a host."""

import asyncio
import contextlib
import functools
import io
import unittest
from dataclasses import dataclass

import background

from rutis import AggregateError, BindingError, PLUGIN_API, RemoteError, ReservedEvent, define_plugin
from rutis.testing import Bus, PluginTestError, load


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

        with self.assertRaises(PluginTestError) as refused:
            run(undeclared())
        # As the runtime says it, with its code (#677).
        self.assertEqual(
            str(refused.exception),
            "service llm is not available to this plugin, which does not declare it in inject: add 'llm' to its inject",
        )
        self.assertEqual((refused.exception.code, refused.exception.data), ("not-declared", {"service": "llm"}))

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
                    t.service("store").put(object)
                # Nesting: 64 levels cross, 65 do not (#279).
                nest = lambda depth: functools.reduce(lambda inner, _: [inner], range(depth), "bottom")
                t.service("store").put(nest(64))
                with self.assertRaisesRegex(PluginTestError, "^store.put: argument 1 cannot be sent: value nested deeper than 64 levels, the most a value crossing processes may nest$"):
                    t.service("store").put(nest(65))
                # Integers of any size cross: beyond ±(2^53−1) as bigints
                # (design V6), which stay ints to the plugin.
                for big in (2**53, -(2**53), [1, {"n": 2**64 + 1}]):
                    t.service("store").put(big)
                    self.assertEqual(t.service("store").last(), big)
                safe = [2**53 - 1, -(2**53 - 1), True]
                t.service("store").put(safe)
                self.assertEqual(t.service("store").last(), safe)
                self.assertEqual(t.service("store").big(), {"n": [2**53]})
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

    def test_the_five_modes_from_the_test_cross_values_and_errors(self):
        # #464 §12.2: the tool has the five ways to emit; with strict, what
        # the test sends and gets back crosses as between processes.
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

                ctx.on("tick", wait)
                ctx.on("tick", fail)
                ctx.on("tick", lambda value: called.append(value["n"]))
                ctx.on("reading", lambda: Reading("Oslo", 3))
                ctx.on("wrap", lambda city, next: next())

            async with load(define_plugin(apply)) as t:
                value = {"n": 1}
                done = asyncio.ensure_future(t.parallel("tick", value))
                await started.wait()
                self.assertEqual((called, value, done.done()), ([2], {"n": 1}, False))
                gate.set()
                with self.assertRaises(AggregateError) as failure:
                    await done
                error = failure.exception
                self.assertEqual(error.message, "2 listeners of tick failed")
                # Registration order, not the order they failed in.
                self.assertEqual([(e.name, e.message) for e in error.errors], [("TypeError", "async"), ("ValueError", "sync")])
                self.assertTrue(all(isinstance(e, RemoteError) for e in error.errors))
                self.assertEqual(await t.serial("reading"), {"city": "Oslo", "degrees": 3}, "a dataclass crosses as a dict")
                self.assertEqual(t.bail("reading"), {"city": "Oslo", "degrees": 3})
                self.assertEqual(await t.waterfall("wrap", "Bergen", terminal=lambda city: city.upper()), "BERGEN")
                self.assertEqual(await t.filter(lambda d: False).serial("reading"), None)

        run(go())

    def test_names_are_strings_internal_is_refused_rutis_is_reserved(self):
        async def go():
            saved = []
            async with load(define_plugin(lambda ctx, config: saved.append(ctx))) as t:
                ctx = saved[0]
                for operation in (ctx.on, ctx.emit, t.emit):
                    with self.assertRaisesRegex(TypeError, "^an event name is a string, not NoneType$"):
                        operation(None, "internal/test")
                    with self.assertRaisesRegex(ValueError, "^event internal/test is internal to the runtime$"):
                        operation("internal/test", lambda: None)
                ctx.on("rutis/row", lambda record: None)
                for emitter in (ctx, t):
                    for mode in ("emit", "parallel", "serial", "waterfall", "bail"):
                        with self.assertRaisesRegex(ReservedEvent, "^rutis/\\* events are emitted by rutis only: rutis/row$"):
                            getattr(emitter, mode)("rutis/row")
            with self.assertRaisesRegex(PluginTestError, "^the load option emits was removed in rutis 0.13: listeners register themselves"):
                async with load(define_plugin(lambda ctx, config: None), emits=["tick"]):
                    pass

        run(go())

    def test_event_disposers_and_failed_cleanup_remove_every_listener(self):
        async def go():
            seen, saved, refused = [], [], []

            def apply(ctx, config):
                callback = lambda value: seen.append(value)
                ctx.on("early", callback)
                def cleanup():
                    try:
                        ctx.emit("early", 99)
                    except BindingError as error:
                        refused.append(str(error))
                    raise asyncio.CancelledError()
                ctx.effect(cleanup)
                off = ctx.on("tick", callback)
                ctx.on("tick", callback)
                saved.extend([ctx, off])

            with self.assertRaises(asyncio.CancelledError):
                async with load(define_plugin(apply)) as t:
                    await t.parallel("tick", 1)
                    saved[1]()
                    saved[1]()
                    self.assertIsNone(saved[0].emit("tick", 2))
                    self.assertEqual(seen, [1, 1, 2])
            # An unloaded row emits nothing: refused at once (#464 §3).
            self.assertEqual(refused, ["row test_testing is not loaded in this runtime"])
            for mode in ("emit", "parallel", "serial", "waterfall", "bail"):
                with self.assertRaisesRegex(BindingError, "^row test_testing is not loaded in this runtime$"):
                    getattr(saved[0], mode)("early", 3)
            self.assertEqual(seen, [1, 1, 2], "cleanup emissions cannot reach the unloading plugin")
            with self.assertRaisesRegex(PluginTestError, "unloaded"):
                saved[0].on("tick", lambda: None)

        run(go())

    def test_a_failing_emit_listener_is_reported_never_raised(self):
        # #464 §5.6, as on the runtime: in `reported` and on stderr.
        order = []

        async def fail():
            order.append("listener")
            raise ValueError("async event")

        async def go():
            def apply(ctx, config):
                ctx.on("tick", fail)
                ctx.on("tick", lambda: order.append("other"))
                ctx.emit("tick")
                order.append("emitter")

            stderr = io.StringIO()
            with contextlib.redirect_stderr(stderr):
                async with load(define_plugin(apply), row="ticker") as t:
                    await asyncio.sleep(0)
                    reported = list(t.reported)
            self.assertEqual(order, ["listener", "other", "emitter"], "ctx.emit starts async listeners eagerly")
            self.assertEqual([(r.kind, r.name, r.row, str(r.error)) for r in reported], [("listener", "tick", "ticker", "async event")])
            self.assertEqual(stderr.getvalue(), "rutis: listener of tick in row ticker failed: ValueError: async event\n")

        run(go())

    def test_plugins_on_one_bus_hear_each_other_and_filter(self):
        # #464 §3, §4.3, §6 in the tool: a shared bus, scopes, prepend,
        # global, the filter seeing descriptors.
        async def go():
            log = []

            def listener(label, **options):
                def apply(ctx, config):
                    ctx.on("t/x", lambda: log.append(label), **options)
                return define_plugin(apply)

            bus = Bus(scopes={"t1": "tenant", "t1/s1": "session"})
            async with load(listener("r"), bus=bus, row="r") as root, \
                    load(listener("u"), bus=bus, row="u", scope="t1") as tenant, \
                    load(listener("s", prepend=True), bus=bus, row="s", scope="t1/s1") as session, \
                    load(listener("v", global_=True), bus=bus, row="v", scope="t2"):
                await session.parallel("t/x")
                self.assertEqual(log, ["s", "r", "u", "v"])
                log.clear()
                await tenant.parallel("t/x")
                self.assertEqual(log, ["s", "r", "u", "v"])
                log.clear()
                seen = []
                await bus.emitter().filter(lambda d: seen.append(d) or d["row"] == "u").parallel("t/x")
                self.assertEqual(log, ["u", "v"], "v is global and skips the filter")
                self.assertEqual([d["row"] for d in seen], ["s", "r", "u"], "global v skips the filter")
                self.assertEqual(seen[0]["scope"], [{"id": "t1", "group": "tenant"}, {"id": "t1/s1", "group": "session"}])
                log.clear()
                await bus.emitter().filter(lambda d: False).parallel("t/x")
                self.assertEqual(log, ["v"], "global skips the filter")
                with self.assertRaisesRegex(PluginTestError, "already loaded"):
                    async with load(listener("x"), bus=bus, row="r"):
                        pass
                self.assertEqual(root.row, "r")

        run(go())

    def test_unload_cancels_unfinished_local_listeners(self):
        async def go():
            cancelled = asyncio.Event()

            async def stay():
                try:
                    await asyncio.Event().wait()
                finally:
                    cancelled.set()

            def apply(ctx, config):
                ctx.on("tick", stay)
                ctx.emit("tick")

            async def scope():
                async with load(define_plugin(apply)):
                    pass

            await asyncio.wait_for(scope(), 1)
            self.assertTrue(cancelled.is_set())

        run(go())

    def test_local_failures_do_not_replace_body_or_cleanup_errors(self):
        async def fail():
            raise ValueError("async event")

        async def go():
            for cleanup in (None, ValueError("cleanup"), asyncio.CancelledError(), AggregateError([ValueError("cleanup")])):
                primary = cleanup if cleanup is not None else AssertionError("test body")

                def apply(ctx, config):
                    ctx.on("tick", fail)
                    ctx.emit("tick")
                    if cleanup is not None:
                        def stop():
                            raise cleanup
                        ctx.effect(stop)

                with self.assertRaises(type(primary)) as failure, contextlib.redirect_stderr(io.StringIO()):
                    async with load(define_plugin(apply)):
                        if cleanup is None:
                            raise primary
                self.assertIs(failure.exception, primary)

        run(go())

    def test_errors_reach_a_plugins_own_emitter_as_from_another_process(self):
        # #464 §11: matched by name, never by class; AggregateError children
        # in registration order; rutis's own names keep their classes.
        class Refused(Exception):
            code = "Refused"

        async def go():
            outcomes = {}

            def listener(ctx, config):
                def refuse():
                    raise Refused("no")

                ctx.on("t/err", refuse)
                ctx.on("t/err", lambda: (_ for _ in ()).throw(BindingError("gone")))

            def emitter(ctx, config):
                async def emit():
                    for mode in ("parallel", "serial"):
                        try:
                            await getattr(ctx, mode)("t/err")
                        except Exception as error:  # noqa: BLE001
                            outcomes[mode] = error
                    try:
                        ctx.bail("t/err")
                    except Exception as error:  # noqa: BLE001
                        outcomes["bail"] = error

                saved.append(emit)

            saved = []
            bus = Bus()
            async with load(define_plugin(listener), bus=bus, row="l"), load(define_plugin(emitter), bus=bus, row="e"):
                await saved[0]()
            aggregate = outcomes["parallel"]
            self.assertIsInstance(aggregate, AggregateError)
            self.assertEqual([(type(e), e.name, e.message) for e in aggregate.errors], [
                (RemoteError, "Refused", "no"), (BindingError, "BindingError", "gone"),
            ])
            self.assertEqual(aggregate.errors[0].code, "Refused")
            for mode in ("serial", "bail"):
                self.assertIs(type(outcomes[mode]), RemoteError, mode)
                self.assertEqual((outcomes[mode].name, outcomes[mode].code), ("Refused", "Refused"))

        run(go())

    def test_descriptors_name_the_declared_plugin_and_python(self):
        # #464 §4.2: `plugin` is the declared name, else the row; `runtime`
        # "python" unless load(runtime=) says otherwise.
        async def go():
            on = lambda ctx, config: ctx.on("t/d", lambda: None)
            bus = Bus()
            async with load(define_plugin(on, name="chat-ui"), bus=bus), \
                    load(define_plugin(on), bus=bus, row="audit"), \
                    load(define_plugin(on), bus=bus, row="other", runtime="go"):
                seen = [(d["plugin"], d["row"], d["runtime"]) for d in bus.listeners("t/d")]
            self.assertEqual(seen, [("chat-ui", "chat-ui", "python"), ("audit", "audit", "python"), ("other", "other", "go")])

        run(go())

    def test_unload_waits_at_most_its_deadline(self):
        async def go():
            release = asyncio.Event()

            async def stubborn():
                while not release.is_set():
                    try:
                        await release.wait()
                    except asyncio.CancelledError:
                        pass

            def apply(ctx, config):
                ctx.on("tick", stubborn)
                ctx.emit("tick")

            stderr = io.StringIO()
            with contextlib.redirect_stderr(stderr):
                async with load(define_plugin(apply), row="slow", deadline=0.05):
                    pass
            release.set()
            self.assertEqual(stderr.getvalue(), "rutis: listeners of row slow still running after 0.05s; unloading anyway\n")
            with self.assertRaisesRegex(PluginTestError, "deadline"):
                async with load(define_plugin(apply), deadline=-1):
                    pass

        run(go())


if __name__ == "__main__":
    unittest.main()
