# guarantee: sdk/undeclared-use
# guarantee: sdk/testing-matches-runtime
"""rutis.testing: plugins tested on a real host (`rutis-host test`), run by
this process's runtime."""

import asyncio
import unittest

import background

from rutis import BindingError, RemoteError, define_plugin
from rutis.advanced import PLUGIN_API, AggregateError, ReservedEvent
from rutis.testing import Bus, PluginTestError, load


class Weather:
    def __init__(self, llm, city):
        self.llm, self.city = llm, city

    async def today(self):
        return f"{await self.llm.ask(self.city)} in {self.city}"

    def unit(self):
        return "C"


class Llm:
    async def ask(self, question):
        return "sunny"


def weather_plugin():
    order = []

    def apply(ctx, config):
        ctx.provide("weather", Weather(ctx.use("llm"), config["city"]))
        ctx.effect(lambda: order.append("effect"))
        return lambda: order.append("returned")

    schema = {"type": "object", "properties": {"city": {"type": "string", "default": "Oslo"}}}
    return define_plugin(apply, inject=["llm"], provides={"weather": Weather}, config=schema, name="weather"), order


def run(coroutine):
    return asyncio.run(coroutine)


class Testing(background.TestCase):
    def test_a_plugin_runs_with_its_services_and_unloads_latest_first(self):
        plugin, order = weather_plugin()

        async def go():
            async with load(plugin, config={"city": "Bergen"}, services={"llm": Llm()}) as t:
                self.assertEqual(t.row, "weather")
                self.assertEqual(await t.service("weather").today(), "sunny in Bergen")
                # Declared sync: called synchronously, as the host calls it.
                self.assertEqual(t.service("weather").unit(), "C")
                self.assertEqual(t.methods("weather"), {"today": "async", "unit": "sync"})
                self.assertEqual(t.provided(), ["weather"])
            self.assertEqual(order, ["returned", "effect"])

        run(go())

    # guarantee: sdk/config-schema
    def test_the_hosts_loader_checks_the_config_and_fills_its_defaults(self):
        plugin, _ = weather_plugin()

        async def go():
            async with load(plugin, services={"llm": Llm()}) as t:
                self.assertEqual(await t.service("weather").today(), "sunny in Oslo")
                await t.update({"city": "Tromsø"})
                self.assertEqual(await t.service("weather").today(), "sunny in Tromsø")
                with self.assertRaises(PluginTestError) as refused:
                    await t.update({"city": 3})
                self.assertEqual(str(refused.exception), "weather: config: city should be a string, got 3")
                self.assertEqual(await t.service("weather").today(), "sunny in Tromsø", "the row keeps its config")
            with self.assertRaises(PluginTestError) as refused:
                async with load(plugin, config={"city": 5}, services={"llm": Llm()}):
                    pass
            self.assertEqual(str(refused.exception), "weather: config: city should be a string, got 5")

        run(go())

    def test_a_missing_service_is_an_error(self):
        plugin, _ = weather_plugin()

        async def missing():
            async with load(plugin):
                pass

        with self.assertRaises(PluginTestError) as refused:
            run(missing())
        self.assertEqual(str(refused.exception), "weather did not start: it injects llm, which nothing provides: give the test services=")

    def test_a_plugin_that_needs_a_newer_plugin_api_is_refused(self):
        plugin, _ = weather_plugin()
        plugin.api = PLUGIN_API + 1

        async def go():
            async with load(plugin, services={"llm": Llm()}):
                pass

        with self.assertRaisesRegex(PluginTestError, "needs plugin API"):
            run(go())

    def test_a_plugin_provides_a_name_the_test_gives(self):
        # #684 H4: the runtime's words.
        class Notes:
            async def get(self, key):
                return None

        wordcount = define_plugin(lambda ctx, config: ctx.provide("notes", Notes()), name="wordcount")

        async def go():
            with self.assertRaises(PluginTestError) as raised:
                async with load(wordcount, services={"notes": Notes()}):
                    pass
            self.assertIn("notes is already provided by tester", str(raised.exception))
            async with load(wordcount) as t:
                self.assertIsNone(await t.service("notes").get("k"))

        run(go())

    def test_the_five_modes(self):
        # #464 §12.2, through the host.
        async def go():
            gate, started = asyncio.Event(), asyncio.Event()

            def apply(ctx, config):
                def fail():
                    raise ValueError("sync")

                async def wait():
                    started.set()
                    await gate.wait()
                    raise TypeError("async")

                ctx.on("tick", wait)
                ctx.on("tick", fail)
                ctx.on("reading", lambda: {"city": "Oslo", "degrees": 3})
                ctx.on("wrap", lambda city, next: next())

            async with load(define_plugin(apply)) as t:
                done = asyncio.ensure_future(t.parallel("tick"))
                await started.wait()
                self.assertFalse(done.done())
                gate.set()
                with self.assertRaises(AggregateError) as failure:
                    await done
                error = failure.exception
                self.assertEqual(error.message, "2 listeners of tick failed")
                # Registration order, not the order they failed in.
                self.assertEqual([(e.name, e.message) for e in error.errors], [("TypeError", "async"), ("ValueError", "sync")])
                self.assertTrue(all(isinstance(e, RemoteError) for e in error.errors))
                self.assertEqual(await t.serial("reading"), {"city": "Oslo", "degrees": 3})
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

        run(go())

    def test_an_unloaded_row_emits_nothing(self):
        async def go():
            seen, saved, refused = [], [], []

            def apply(ctx, config):
                ctx.on("early", seen.append)

                def cleanup():
                    try:
                        ctx.emit("early", 99)
                    except BindingError as error:
                        refused.append(str(error))

                ctx.effect(cleanup)
                off = ctx.on("tick", seen.append)
                ctx.on("tick", seen.append)
                saved.extend([ctx, off])

            async with load(define_plugin(apply), id="ticks") as t:
                await t.parallel("tick", 1)
                saved[1]()
                saved[1]()
                await saved[0].parallel("tick", 2)
                self.assertEqual(seen, [1, 1, 2])
            # An unloaded row emits nothing: refused at once (#464 §3).
            self.assertEqual(refused, ["row ticks is not loaded in this runtime"])
            self.assertEqual(seen, [1, 1, 2], "cleanup emissions cannot reach the unloading plugin")

        run(go())

    def test_a_failing_emit_listener_is_reported_never_raised(self):
        # #464 §5.6: the host reports it, the emitter never sees it.
        async def fail():
            raise ValueError("async event")

        async def go():
            def apply(ctx, config):
                ctx.on("tick", fail)

            bus = Bus()
            async with load(define_plugin(apply), id="ticker", bus=bus) as t:
                self.assertIsNone(t.emit("tick"))
                for _ in range(100):
                    if await t.reported():
                        break
                    await asyncio.sleep(0.01)
                reported = await t.reported()
                self.assertEqual([r.line.split("\n")[0] for r in reported], ["ticker: listener of tick failed: ValueError: async event"])
                self.assertEqual(await bus.reported(), reported)

        run(go())

    # guarantee: sdk/leaf-events
    def test_plugins_on_one_bus_hear_each_other_and_filter(self):
        # #464 §4.3, §6: prepend, global, the filter seeing descriptors.
        async def go():
            log = []

            def listener(label, **options):
                def apply(ctx, config):
                    ctx.on("t/x", lambda: log.append(label), **options)
                return define_plugin(apply)

            bus = Bus()
            async with load(listener("r"), bus=bus, id="r") as root, \
                    load(listener("u"), bus=bus, id="u"), \
                    load(listener("s", prepend=True), bus=bus, id="s"), \
                    load(listener("v", global_=True), bus=bus, id="v"):
                await root.parallel("t/x")
                self.assertEqual(log, ["s", "r", "u", "v"])
                log.clear()
                seen = []
                await bus.emitter().filter(lambda d: seen.append(d) or d["row"] == "u").parallel("t/x")
                self.assertEqual(log, ["u", "v"], "v is global and skips the filter")
                self.assertEqual([(d["row"], d["runtime"]) for d in seen], [("s", "test"), ("r", "test"), ("u", "test")])
                log.clear()
                await bus.emitter().filter(lambda d: False).parallel("t/x")
                self.assertEqual(log, ["v"], "global skips the filter")
                with self.assertRaises(PluginTestError) as taken:
                    async with load(listener("x"), bus=bus, id="r"):
                        pass
                self.assertIn("a row r is loaded already", str(taken.exception))

        run(go())

    def test_without_an_id_a_taken_name_gets_a_suffix(self):
        async def go():
            bus = Bus()
            plugin = define_plugin(lambda ctx, config: None, name="chat")
            async with load(plugin, bus=bus) as first, load(plugin, bus=bus) as second:
                self.assertEqual((first.row, second.row), ("chat", "chat-2"))

        run(go())

    def test_isolate_gives_each_row_its_own_service(self):
        # `isolate` as in rutis.json: rows isolating a name each provide
        # their own.
        class Notes:
            def __init__(self, owner):
                self.owner = owner

            async def who(self):
                return self.owner

        def notes(owner):
            return define_plugin(lambda ctx, config: ctx.provide("notes", Notes(owner)))

        async def go():
            bus = Bus()
            async with load(notes("a"), bus=bus, id="a", isolate={"notes": True}) as a, \
                    load(notes("b"), bus=bus, id="b", isolate={"notes": True}) as b:
                self.assertEqual((await a.service("notes").who(), await b.service("notes").who()), ("a", "b"))
            with self.assertRaises(PluginTestError):
                async with load(notes("a"), bus=bus, id="a"), load(notes("b"), bus=bus, id="b"):
                    pass

        run(go())

    def test_unload_cancels_unfinished_listeners(self):
        async def go():
            cancelled = asyncio.Event()

            async def stay():
                try:
                    await asyncio.Event().wait()
                finally:
                    cancelled.set()

            def apply(ctx, config):
                ctx.on("tick", stay)

            async def scope():
                async with load(define_plugin(apply)) as t:
                    t.emit("tick")
                    await asyncio.sleep(0.05)

            await asyncio.wait_for(scope(), 5)
            self.assertTrue(cancelled.is_set())

        run(go())

    def test_a_failure_in_the_test_body_is_the_one_raised(self):
        async def go():
            with self.assertRaises(AssertionError) as failure:
                async with load(define_plugin(lambda ctx, config: None)):
                    raise AssertionError("test body")
            self.assertEqual(str(failure.exception), "test body")

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
            async with load(define_plugin(listener), bus=bus, id="l"), load(define_plugin(emitter), bus=bus, id="e"):
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

    def test_descriptors_name_the_declared_plugin(self):
        # #464 §4.2: `plugin` is the declared name, else the row.
        async def go():
            on = lambda ctx, config: ctx.on("t/d", lambda: None)
            bus = Bus()
            seen = []
            async with load(define_plugin(on, name="chat-ui"), bus=bus) as chat, load(define_plugin(on), bus=bus, id="audit"):
                await chat.filter(lambda d: seen.append(d) and False).parallel("t/d")
            self.assertEqual([(d["plugin"], d["row"]) for d in seen], [("chat-ui", "chat-ui"), ("audit", "audit")])

        run(go())

    def test_without_rutis_host_it_says_what_to_install(self):
        import os

        saved = os.environ.get("RUTIS_HOST"), os.environ.get("PATH")
        os.environ["RUTIS_HOST"], os.environ["PATH"] = "", ""

        async def go():
            async with load(define_plugin(lambda ctx, config: None)):
                pass

        try:
            with self.assertRaises(PluginTestError) as refused:
                run(go())
        finally:
            for name, value in zip(("RUTIS_HOST", "PATH"), saved):
                if value is None:
                    os.environ.pop(name, None)
                else:
                    os.environ[name] = value
        self.assertEqual(
            str(refused.exception),
            "rutis.testing runs plugins on rutis-host: install the rutis-host package "
            "(a dev dependency of the plugin) or set RUTIS_HOST to its file",
        )


if __name__ == "__main__":
    unittest.main()
