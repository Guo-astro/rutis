"""Unified events in the runtime's own process (#464): the five modes, the
flat bus, filters, order, reserved names, error types and unload
waiting, on the fast path (no host: every listener is here).
docs/design-events-protocol-2026-10-18.md §3–§8, §11, §12.2."""

import asyncio
import contextlib
import io
import os
import sys
import threading
import types
import unittest

import background
from support import HANG_GUARD, within

from rutis import AggregateError, AsyncListener, BindingError, ReservedEvent, WaterfallNext, define_plugin
from rutis.events import Bus, no_result, within as hears
from rutis.runner import Runtime


def module(name: str, apply) -> str:
    """Register a plugin module `name` whose `apply` is `apply`."""
    plugin = types.ModuleType(name)
    plugin.apply = apply
    sys.modules[name] = plugin
    return name


def chain(*ids):
    return [{"id": id, "group": "g"} for id in ids]


class Rows(background.IsolatedAsyncioTestCase):
    """A runtime with rows whose contexts the tests drive."""

    async def asyncSetUp(self):
        self.runtime = Runtime()
        self.contexts = {}

    async def asyncTearDown(self):
        await self.runtime.dispose()

    async def row(self, key: str, scope=None):
        def apply(ctx, config):
            self.contexts[key] = ctx

        await self.runtime.load(key, module(f"events_row_{key}", apply), {}, None, None, scope or [])
        return self.contexts[key]


class NoResult(unittest.TestCase):
    def test_none_and_false_are_no_result_zero_and_empty_are(self):
        # §1: "no result" is None or False, in every language.
        for value in (None, False):
            self.assertTrue(no_result(value))
        for value in (0, "", [], {}, 0.0, True):
            self.assertFalse(no_result(value), value)

    def test_the_scope_rule_compares_ids_as_a_prefix(self):
        self.assertTrue(hears([], chain("t1", "s1")))
        self.assertTrue(hears(chain("t1"), chain("t1", "s1")))
        self.assertTrue(hears(chain("t1", "s1"), chain("t1", "s1")))
        self.assertFalse(hears(chain("t1", "s1"), chain("t1")))
        self.assertFalse(hears(chain("t2"), chain("t1", "s1")))


class Emit(Rows):
    async def test_emit_returns_at_once_and_starts_every_listener_in_order(self):
        # §5.4 emit, §5.5: listeners start in registration order, each runs
        # to its first await before emit returns; the emitter waits for none.
        ctx = await self.row("a")
        log, gate = [], asyncio.Event()

        async def slow(value):
            log.append(f"slow:{value}")
            await gate.wait()
            log.append("slow done")

        ctx.on("t/emit", slow)
        ctx.on("t/emit", lambda value: log.append(f"sync:{value}"))
        self.assertIsNone(ctx.emit("t/emit", 1))
        self.assertEqual(log, ["slow:1", "sync:1"])
        gate.set()
        await asyncio.sleep(0)
        self.assertEqual(log[-1], "slow done")

    async def test_emits_of_one_name_reach_each_listener_in_emission_order(self):
        ctx = await self.row("a")
        log = []

        async def listener(value):
            log.append(value)
            await asyncio.sleep(0)

        ctx.on("t/order", listener)
        for value in (1, 2, 3):
            ctx.emit("t/order", value)
        self.assertEqual(log, [1, 2, 3])

    async def test_a_failing_listener_is_reported_and_affects_nobody(self):
        # §5.6: not the emitter, not the other listeners, never the process.
        ctx = await self.row("a")
        log = []

        def boom():
            raise KeyError("a failed")

        async def later():
            await asyncio.sleep(0)
            raise ValueError("later failed")

        ctx.on("t/emit-error", boom)
        ctx.on("t/emit-error", later)
        ctx.on("t/emit-error", lambda: log.append("b"))
        stderr = io.StringIO()
        with contextlib.redirect_stderr(stderr):
            self.assertIsNone(ctx.emit("t/emit-error"))
            for _ in range(3):
                await asyncio.sleep(0)
        self.assertEqual(log, ["b"])
        self.assertEqual(
            stderr.getvalue().splitlines(),
            [
                "rutis: listener of t/emit-error in row a failed: KeyError: 'a failed'",
                "rutis: listener of t/emit-error in row a failed: ValueError: later failed",
            ],
        )


class Parallel(Rows):
    async def test_all_run_and_the_emitter_waits_for_all(self):
        ctx = await self.row("a")
        log = []

        async def slow():
            await asyncio.sleep(0.01)
            log.append("slow")
            return "ignored"

        ctx.on("t/par", slow)
        ctx.on("t/par", lambda: log.append("fast"))
        self.assertIsNone(await ctx.parallel("t/par"))
        self.assertEqual(sorted(log), ["fast", "slow"])

    async def test_failures_aggregate_in_registration_order_even_one(self):
        # §5.4 parallel, §11 AggregateError: registration order, not the
        # order they failed in; also a single failure is aggregated.
        ctx = await self.row("a")

        async def late():
            await asyncio.sleep(0.02)
            raise LookupError("a")

        def early():
            raise TypeError("b")

        ctx.on("t/par-errors", late)
        ctx.on("t/par-errors", early)
        ctx.on("t/par-errors", lambda: None)
        with self.assertRaises(AggregateError) as raised:
            await ctx.parallel("t/par-errors")
        self.assertEqual([type(error) for error in raised.exception.errors], [LookupError, TypeError])
        self.assertEqual(raised.exception.message, "2 listeners of t/par-errors failed")
        ctx.on("t/par-one", early)
        with self.assertRaises(AggregateError) as raised:
            await ctx.parallel("t/par-one")
        self.assertEqual(len(raised.exception.errors), 1)
        self.assertEqual(raised.exception.message, "1 listeners of t/par-one failed")


class Serial(Rows):
    async def test_the_first_result_stops_none_and_false_are_no_result(self):
        ctx = await self.row("a")
        log = []

        def listener(label, value):
            def call(arg):
                log.append(label)
                return value

            return call

        async def c(arg):
            log.append("c")
            return f"C{arg}"

        ctx.on("t/serial", listener("a", None))
        ctx.on("t/serial", listener("b", False))
        ctx.on("t/serial", c)
        ctx.on("t/serial", listener("d", "D"))
        self.assertEqual(await ctx.serial("t/serial", "x"), "Cx")
        self.assertEqual(log, ["a", "b", "c"])

    async def test_zero_is_a_result_and_nothing_is_none(self):
        ctx = await self.row("a")
        ctx.on("t/serial-zero", lambda: 0)
        ctx.on("t/serial-zero", lambda: 1)
        self.assertEqual(await ctx.serial("t/serial-zero"), 0)
        ctx.on("t/serial-none", lambda: False)
        self.assertIsNone(await ctx.serial("t/serial-none"))
        self.assertIsNone(await ctx.serial("t/nobody"))

    async def test_an_error_stops_and_is_raised_as_is(self):
        ctx = await self.row("a")
        log = []

        def fail():
            raise LookupError("a")

        ctx.on("t/serial-error", fail)
        ctx.on("t/serial-error", lambda: log.append("b") or "B")
        with self.assertRaisesRegex(LookupError, "a"):
            await ctx.serial("t/serial-error")
        self.assertEqual(log, [])


class Waterfall(Rows):
    async def test_a_chain_from_outermost_inwards_with_a_veto(self):
        ctx = await self.row("a")
        log = []

        def link(label):
            async def call(next):
                log.append(label)
                return label + (await next() or "")

            return call

        ctx.on("t/wf", link("a"))
        ctx.on("t/wf", link("b"))
        ctx.on("t/wf", lambda next: log.append("c") or "c")
        ctx.on("t/wf", link("d"))
        self.assertEqual(await ctx.waterfall("t/wf"), "abc")
        self.assertEqual(log, ["a", "b", "c"])

    async def test_the_terminal_gets_the_event_arguments(self):
        ctx = await self.row("a")
        ctx.on("t/wf-terminal", lambda city, next: next())

        async def terminal(city):
            return f"weather in {city}"

        self.assertEqual(await ctx.waterfall("t/wf-terminal", "Oslo", terminal=terminal), "weather in Oslo")
        self.assertEqual(await ctx.waterfall("t/wf-none", "Oslo", terminal=lambda city: city), "Oslo")
        self.assertIsNone(await ctx.waterfall("t/wf-none", "Oslo"))

    async def test_the_outermost_value_is_returned_as_is(self):
        # §5.4: no "no result" normalisation; False stays False.
        ctx = await self.row("a")
        ctx.on("t/wf-false", lambda next: False)
        self.assertIs(await ctx.waterfall("t/wf-false"), False)

    async def test_next_twice_or_after_the_end_fails(self):
        ctx = await self.row("a")
        log, kept = [], []

        async def twice(next):
            log.append("a")
            await next()
            return await next()

        ctx.on("t/wf-twice", twice)
        ctx.on("t/wf-twice", lambda next: log.append("b") or "b")
        with self.assertRaisesRegex(WaterfallNext, "^next of t/wf-twice was already called$"):
            await ctx.waterfall("t/wf-twice")
        self.assertEqual(log, ["a", "b"])
        ctx.on("t/wf-late", lambda next: kept.append(next) or "early")
        self.assertEqual(await ctx.waterfall("t/wf-late"), "early")
        with self.assertRaisesRegex(WaterfallNext, "^next of t/wf-late was called after its waterfall ended$"):
            kept[0]()

    async def test_next_after_the_emitter_gave_up_fails(self):
        # §5.7: a cancelled dispatch fails any later next with WaterfallNext.
        ctx = await self.row("a")
        kept, started = [], asyncio.Event()

        async def holds(next):
            kept.append(next)
            started.set()
            await asyncio.Event().wait()

        ctx.on("t/wf-cancel", holds)
        ctx.on("t/wf-cancel", lambda next: "never")
        dispatch = asyncio.ensure_future(ctx.waterfall("t/wf-cancel"))
        await within("the listener starts", started.wait())
        dispatch.cancel()
        with self.assertRaises(asyncio.CancelledError):
            await dispatch
        with self.assertRaisesRegex(WaterfallNext, "^next of t/wf-cancel was called after its waterfall ended$"):
            kept[0]()

    async def test_an_inner_error_reaches_the_emitter(self):
        ctx = await self.row("a")

        def fail(next):
            raise LookupError("b")

        ctx.on("t/wf-error", lambda next: next())
        ctx.on("t/wf-error", fail)
        with self.assertRaisesRegex(LookupError, "b"):
            await ctx.waterfall("t/wf-error")


class Bail(Rows):
    async def test_the_first_result_synchronously(self):
        ctx = await self.row("a")
        ctx.on("t/bail", lambda: None)
        ctx.on("t/bail", lambda: "B")
        ctx.on("t/bail", lambda: "C")
        self.assertEqual(ctx.bail("t/bail"), "B")

    async def test_an_async_def_listener_is_not_called(self):
        ctx = await self.row("a")
        log = []

        async def listener():
            log.append("called")
            return "A"

        ctx.on("t/bail-async", listener)
        with self.assertRaisesRegex(
            AsyncListener, "^listener of t/bail-async in row a is asynchronous and cannot run in a synchronous bail$"
        ):
            ctx.bail("t/bail-async")
        self.assertEqual(log, [])

    async def test_a_plain_function_returning_an_awaitable_fails_and_the_coroutine_is_closed(self):
        ctx = await self.row("a")
        log = []

        async def later():
            log.append("ran")

        coroutines = []

        def listener():
            log.append("called")
            coroutines.append(later())
            return coroutines[0]

        ctx.on("t/bail-any", listener)
        with self.assertRaises(AsyncListener):
            ctx.bail("t/bail-any")
        self.assertEqual(log, ["called"])
        self.assertIsNone(coroutines[0].cr_frame, "closed, never run")


class Scope(Rows):
    async def test_events_are_one_flat_bus(self):
        # §3, case scope/flat-bus.
        log = []
        root = await self.row("r")
        tenant1 = await self.row("u", chain("t1"))
        session = await self.row("s", chain("t1", "s1"))
        tenant2 = await self.row("v", chain("t2"))
        for label, ctx in (("r", root), ("u", tenant1), ("s", session), ("v", tenant2)):
            ctx.on("t/scope", lambda label=label: log.append(label))
        await session.parallel("t/scope")
        self.assertEqual(sorted(log), ["r", "s", "u", "v"])
        log.clear()
        await tenant1.parallel("t/scope")
        self.assertEqual(sorted(log), ["r", "s", "u", "v"])
        log.clear()
        await tenant2.parallel("t/scope")
        self.assertEqual(sorted(log), ["r", "s", "u", "v"])
        log.clear()
        await root.parallel("t/scope")
        self.assertEqual(sorted(log), ["r", "s", "u", "v"])

    async def test_a_malformed_scope_is_refused(self):
        with self.assertRaisesRegex(ValueError, "scope chain"):
            await self.runtime.load("x", module("events_bad_scope", lambda ctx, config: None), {}, None, None, [{"id": 1}])


class Filter(Rows):
    async def test_asked_once_per_listener_in_order_before_any_runs(self):
        # §6, case filter/keeps-and-sees: the name snapshot first (d is
        # asked too), then the filter, then the listeners.
        log = []
        a = await self.row("a")
        await self.row("b")
        await self.row("c")
        d = await self.row("d", chain("t2"))
        a.on("t/filter", lambda: log.append("a"))
        self.contexts["b"].on("t/filter", lambda: log.append("b"))
        self.contexts["c"].on("t/filter", lambda: log.append("c"))
        d.on("t/filter", lambda: log.append("d"))
        seen = []

        def keep(descriptor):
            seen.append(descriptor)
            log.append(f"filter:{descriptor['row']}")
            return descriptor["row"] in ("a", "c")

        self.assertIsNone(await a.filter(keep).serial("t/filter"))
        self.assertEqual(log, ["filter:a", "filter:b", "filter:c", "filter:d", "a", "c"])
        self.assertEqual(
            seen[0],
            {
                "name": "t/filter",
                "prefix": False,
                "plugin": "a",  # no declared name: the row's id
                "row": "a",
                "runtime": None,
                "node": None,
                "scope": [],
                "global": False,
            },
        )

    async def test_every_mode_filters_and_filters_combine(self):
        ctx = await self.row("a")
        other = await self.row("b")
        log = []
        ctx.on("t/f", lambda *args: log.append("a") or "A")
        other.on("t/f", lambda *args: log.append("b") or "B")
        only_b = ctx.filter(lambda d: d["row"] == "b")
        only_b.emit("t/f")
        await only_b.parallel("t/f")
        self.assertEqual(await only_b.serial("t/f"), "B")
        self.assertEqual(await only_b.waterfall("t/f"), "B")
        self.assertEqual(only_b.bail("t/f"), "B")
        self.assertEqual(log, ["b"] * 5)
        self.assertIsNone(await only_b.filter(lambda d: d["row"] == "a").serial("t/f"))

    async def test_global_listeners_skip_the_filter(self):
        log = []
        ctx = await self.row("a", chain("t1"))
        other = await self.row("b", chain("t2"))
        ctx.on("t/filter-global", lambda: log.append("g"), global_=True)
        other.on("t/filter-global", lambda: log.append("h"), global_=True)
        asked = []
        await ctx.filter(lambda d: asked.append(d) or False).parallel("t/filter-global")
        self.assertEqual((log, asked), (["g", "h"], []))

    async def test_a_failing_filter_fails_the_dispatch_and_is_reported_in_emit(self):
        ctx = await self.row("a")
        log = []
        ctx.on("t/f", lambda: log.append("a"))

        def broken(descriptor):
            raise LookupError("no")

        with self.assertRaisesRegex(LookupError, "no"):
            await ctx.filter(broken).parallel("t/f")
        stderr = io.StringIO()
        with contextlib.redirect_stderr(stderr):
            ctx.filter(broken).emit("t/f")
        self.assertEqual(log, [])
        self.assertEqual(stderr.getvalue(), "rutis: filter of t/f from row a failed: LookupError: no\n")


class Order(Rows):
    async def test_prepend_goes_before_every_listener_so_far(self):
        # §4.3, case order/prepend.
        ctx = await self.row("a")
        log = []
        ctx.on("t/prepend", lambda: log.append("b"))
        ctx.on("t/prepend", lambda: log.append("a"), prepend=True)
        ctx.on("t/prepend", lambda: log.append("c"))
        await ctx.serial("t/prepend")
        self.assertEqual(log, ["a", "b", "c"])

    async def test_a_listener_withdrawn_during_a_dispatch_is_skipped(self):
        # §5.1 step 3: checked just before it starts.
        ctx = await self.row("a")
        log = []
        offs = []
        ctx.on("t/live", lambda: offs[0]())
        offs.append(ctx.on("t/live", lambda: log.append("b")))
        await ctx.parallel("t/live")
        await ctx.serial("t/live")
        self.assertEqual(log, [])


class Reserved(Rows):
    async def test_rutis_events_are_refused_in_every_mode_synchronously_and_can_be_listened_to(self):
        # §8, §11 ReservedEvent; case reserved/rutis-emit.
        ctx = await self.row("a")
        ctx.on("rutis/row", lambda record: None)
        for mode in ("emit", "parallel", "serial", "waterfall", "bail"):
            with self.subTest(mode):
                with self.assertRaisesRegex(ReservedEvent, "^rutis/\\* events are emitted by rutis only: rutis/fake$"):
                    getattr(ctx, mode)("rutis/fake")
                with self.assertRaises(ReservedEvent):
                    getattr(ctx.filter(lambda d: True), mode)("rutis/fake")

    async def test_internal_events_are_refused(self):
        ctx = await self.row("a")
        for call in (lambda: ctx.on("internal/x", lambda: None), lambda: ctx.emit("internal/x")):
            with self.assertRaisesRegex(ValueError, "^event internal/x is internal to the runtime$"):
                call()
        with self.assertRaises(TypeError):
            ctx.on(3, lambda: None)


class Unload(Rows):
    async def test_unload_cancels_running_listeners_and_waits_for_them(self):
        # §7.1, case unload/waits-and-cancels.
        emitter = await self.row("e")
        listener = await self.row("b")
        log = []
        started = asyncio.Event()

        async def waits():
            log.append("b")
            started.set()
            try:
                await asyncio.Event().wait()
            except asyncio.CancelledError:
                await asyncio.sleep(0)
                log.append("b:cancelled")
                return None

        listener.on("t/unload", waits)
        dispatch = asyncio.ensure_future(emitter.parallel("t/unload"))
        await within("the listener", started.wait())
        await within("the unload", self.runtime.unload("b"))
        self.assertEqual(log, ["b", "b:cancelled"])
        self.assertIsNone(await within("the dispatch", dispatch))

    async def test_unload_gives_up_on_listeners_after_the_shutdown_deadline(self):
        emitter = await self.row("e")
        listener = await self.row("b")
        cleaned = []
        listener.effect(lambda: cleaned.append(True))
        started = asyncio.Event()
        release = asyncio.Event()

        async def stubborn():
            started.set()
            while not release.is_set():
                try:
                    await release.wait()
                except asyncio.CancelledError:
                    pass

        listener.on("t/stubborn", stubborn)
        emitter.emit("t/stubborn")
        await within("the listener", started.wait())
        stderr = io.StringIO()
        previous = os.environ.get("RUTIS_SHUTDOWN_TIMEOUT")
        os.environ["RUTIS_SHUTDOWN_TIMEOUT"] = "0.05"
        try:
            with contextlib.redirect_stderr(stderr):
                await within("the unload", self.runtime.unload("b"))
        finally:
            if previous is None:
                del os.environ["RUTIS_SHUTDOWN_TIMEOUT"]
            else:
                os.environ["RUTIS_SHUTDOWN_TIMEOUT"] = previous
            release.set()
        self.assertEqual(stderr.getvalue(), "rutis: listeners of row b still running after 0.05s; unloading anyway\n")
        self.assertEqual(cleaned, [True], "the cleanups ran after")

    async def test_an_unloaded_row_hears_nothing_and_a_disposer_withdraws(self):
        # Case unload/not-called-after.
        emitter = await self.row("e")
        a = await self.row("a")
        b = await self.row("b")
        a.on("t/unloaded", lambda: None)
        b.on("t/unloaded", lambda: "B")
        await self.runtime.unload("b")
        self.assertIsNone(await emitter.serial("t/unloaded"))
        with self.assertRaisesRegex(RuntimeError, "not active"):
            b.on("t/unloaded", lambda: None)
        off = a.on("t/off", lambda: "A")
        self.assertEqual(await emitter.serial("t/off"), "A")
        off()
        off()
        self.assertIsNone(await emitter.serial("t/off"))

    async def test_listeners_leave_on_update_and_failed_apply(self):
        seen, contexts = [], []

        def apply(ctx, config):
            contexts.append(ctx)
            ctx.on("tick", lambda value: seen.append([config.get("label"), value]))
            if config.get("fail"):
                raise ValueError("apply failed")

        entry = module("events_update_rows", apply)
        await self.runtime.load("a", entry, {"label": "first"}, None)
        await self.runtime.load("b", entry, {"label": "other"}, None)
        await self.runtime.update("a", {"label": "updated"})
        with self.assertRaisesRegex(ValueError, "apply failed"):
            await self.runtime.load("failed", entry, {"fail": True}, None)
        contexts[1].emit("tick", 1)
        # The updated row registered again: after b.
        self.assertEqual(seen, [["other", 1], ["updated", 1]])


class Review(Rows):
    """Rules settled across languages after #464's review."""

    async def test_an_unloaded_row_emits_nothing_refused_at_once(self):
        # §3: `row <key> is not loaded in this runtime`, synchronously in
        # every mode, as a reserved name is refused; filtered emitters too.
        ctx = await self.row("gone")
        filtered = ctx.filter(lambda d: True)
        await self.runtime.unload("gone")
        for emitter in (ctx, filtered):
            for mode in ("emit", "parallel", "serial", "waterfall", "bail"):
                with self.assertRaisesRegex(BindingError, "^row gone is not loaded in this runtime$"):
                    getattr(emitter, mode)("t/gone")

    async def test_next_starts_the_rest_of_the_chain_at_once(self):
        # §5.4: as in JavaScript and Go, next() starts the rest eagerly.
        ctx = await self.row("a")
        log = []

        async def outer(next):
            rest = next()
            log.append("outer after next")
            return "outer:" + await rest

        async def inner(next):
            log.append("inner")
            return "inner"

        ctx.on("t/wf-eager", outer)
        ctx.on("t/wf-eager", inner)
        self.assertEqual(await ctx.waterfall("t/wf-eager"), "outer:inner")
        self.assertEqual(log, ["inner", "outer after next"])

    async def test_the_snapshot_and_the_filter_are_taken_when_called(self):
        # §5.1: not when the awaitable is first awaited.
        ctx = await self.row("a")
        log, asked = [], []
        ctx.on("t/snap", lambda *next: log.append("early"))
        dispatches = [
            ctx.filter(lambda d: asked.append(mode) or True).__getattribute__(mode)("t/snap")
            for mode in ("parallel", "serial", "waterfall")
        ]
        self.assertEqual(asked, ["parallel", "serial", "waterfall"], "the filter ran at the call")
        ctx.on("t/snap", lambda *next: log.append("late"))
        for dispatch in dispatches:
            await dispatch
        self.assertEqual(log, ["early", "early", "early"])

    async def test_a_listener_cancelled_by_its_rows_unload_is_not_reported(self):
        # §5.6, §7.1: cancellation by the unload is not a failure.
        emitter = await self.row("e")
        listener = await self.row("b")
        started = asyncio.Event()

        async def waits():
            started.set()
            await asyncio.Event().wait()

        listener.on("t/cancelled", waits)
        stderr = io.StringIO()
        with contextlib.redirect_stderr(stderr):
            emitter.emit("t/cancelled")
            await within("the listener", started.wait())
            await within("the unload", self.runtime.unload("b"))
            await asyncio.sleep(0)
        self.assertEqual(stderr.getvalue(), "")

    async def test_listeners_given_up_on_are_dropped_from_the_row(self):
        # §7.1 step 4: a reload of the same key does not wait for them.
        emitter = await self.row("e")
        listener = await self.row("b")
        started, release = asyncio.Event(), asyncio.Event()

        async def stubborn():
            started.set()
            while not release.is_set():
                try:
                    await release.wait()
                except asyncio.CancelledError:
                    pass

        listener.on("t/stubborn", stubborn)
        emitter.emit("t/stubborn")
        await within("the listener", started.wait())
        previous = os.environ.get("RUTIS_SHUTDOWN_TIMEOUT")
        os.environ["RUTIS_SHUTDOWN_TIMEOUT"] = "0.05"
        try:
            with contextlib.redirect_stderr(io.StringIO()):
                await within("the unload", self.runtime.unload("b"))
            self.assertNotIn("b", self.runtime.bus.running)
            await self.row("b")
            stderr = io.StringIO()
            with contextlib.redirect_stderr(stderr):
                await within("the second unload", self.runtime.unload("b"))
            self.assertEqual(stderr.getvalue(), "", "nothing left to wait for")
        finally:
            if previous is None:
                del os.environ["RUTIS_SHUTDOWN_TIMEOUT"]
            else:
                os.environ["RUTIS_SHUTDOWN_TIMEOUT"] = previous
            release.set()

    async def test_the_declared_name_is_the_descriptors_plugin(self):
        # §4.2: `plugin` is define_plugin's name, else the row's id.
        def apply(ctx, config):
            ctx.on("t/named", lambda: None)

        plugin = types.ModuleType("events_named_plugin")
        plugin.plugin = define_plugin(apply, name="chat-ui")
        sys.modules["events_named_plugin"] = plugin
        await self.runtime.load("named", "events_named_plugin", {}, None, None, [], "chat")
        await self.runtime.load("anon", module("events_anon_plugin", apply), {}, None, None, [], "audit")
        emitter = await self.row("e")
        seen = []
        await emitter.filter(lambda d: seen.append((d["plugin"], d["row"]))).parallel("t/named")
        self.assertEqual(seen, [("chat-ui", "chat"), ("audit", "audit")])
        with self.assertRaises(TypeError):
            define_plugin(apply, name="")


class Threads(Rows):
    async def test_a_plugins_threads_listen_emit_and_withdraw_on_the_loop(self):
        # A plugin's own threads: marshalled onto the session's loop, in
        # call order; the table is safe to change from any of them.
        ctx = await self.row("a")
        loop_thread = threading.get_ident()
        log = []

        def listener(value):
            log.append((value, threading.get_ident() == loop_thread))
            return value

        def plugin_thread():
            offs = [ctx.on("t/thread", listener) for _ in range(50)]
            ctx.emit("t/thread", "emitted")
            serial = ctx.serial("t/thread", "serial")
            bailed = ctx.bail("t/thread", "bail")
            for off in offs[1:]:
                off()
            return serial.result(HANG_GUARD), bailed

        serial, bailed = await within("the thread", asyncio.to_thread(plugin_thread))
        self.assertEqual((serial, bailed), ("serial", "bail"))
        self.assertTrue(all(on_loop for _value, on_loop in log), "listeners ran on the loop")
        self.assertEqual([value for value, _ in log], ["emitted"] * 50 + ["serial", "bail"])
        self.assertEqual(len(self.runtime.bus.entries), 1)

        async def from_a_loop_of_its_own():
            return await ctx.parallel("t/thread", "own loop")

        self.assertIsNone(await within("the thread", asyncio.to_thread(asyncio.run, from_a_loop_of_its_own())))
        self.assertEqual(log[-1], ("own loop", True))

    async def test_concurrent_registrations_keep_the_table_whole(self):
        ctx = await self.row("a")

        def register():
            for _ in range(200):
                ctx.on("t/many", lambda: None)()

        threads = [threading.Thread(target=register) for _ in range(4)]
        for thread in threads:
            thread.start()
        await within("the threads", asyncio.to_thread(lambda: [thread.join() for thread in threads]))
        self.assertEqual((self.runtime.bus.entries, self.runtime.bus.by_id), ([], {}))


class BusAlone(unittest.TestCase):
    def test_a_listener_must_be_callable(self):
        with self.assertRaises(TypeError):
            Bus().add("t", "row", "plugin", None, [])


if __name__ == "__main__":
    unittest.main()
