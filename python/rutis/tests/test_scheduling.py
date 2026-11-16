"""Scheduling the Python session's frames, controlled by the test (#630,
formal verification design §3.3: "Python: the event loop, cancel
propagation, deferred callbacks and frames during close").

Nothing here waits on a wall clock: `SteppedLoop` runs the loop's ready
callbacks only when the test steps it and moves the clock only when the test
advances it; `ScriptedChannel` hands over one incoming frame at a time and
waits for the answer to the previous one. `tests/scheduling.py` says how.

`ControlFrameTests` is the one place a loop thread still waits in the
synchronous pump (peer.py `_call_sync`): the test drives that pump through
the channel, handing over the frames in the order it chooses.
"""

import asyncio
import sys
import threading
import types

import background
from scheduling import ScriptedChannel, SteppedLoop
from support import HANG_GUARD, LOCAL, hello

from rutis.peer import Peer
from rutis.runner import Runtime


def data(value):
    return {"type": "data", "value": value}


def invoke(call, method, args=None, sync=False):
    frame = {
        "op": "invoke",
        "id": call,
        "path": [],
        "target": "test",
        "method": method,
        "args": args or {"type": "list", "value": []},
    }
    if sync:
        frame["sync"] = True
    return frame


class DeferredFrameTests(background.TestCase):
    """A received frame is a job the loop runs, not work the reader does."""

    def setUp(self):
        self.loop = SteppedLoop()
        self.channel = ScriptedChannel()
        self.log = []

    def tearDown(self):
        self.loop.finish()

    def dispatch(self, target, method, args):
        self.log.append(method)
        return f"answer:{method}"

    def test_frames_run_in_arrival_order_and_only_when_the_loop_runs_them(self):
        with self.loop.entered():
            peer = Peer(self.channel, self.dispatch, endpoint=LOCAL)
            peer.receive(hello())
            peer.receive(invoke("main:1", "first"))
            peer.receive(invoke("main:2", "second"))
            self.assertEqual(self.log, [], "a received frame runs only when the loop runs it")
            self.assertEqual(self.loop.step(), 1)
            self.assertEqual(self.log, ["first"], "one step ran one job")
            self.assertEqual(
                self.channel.pending(),
                [{"op": "return", "id": "main:1", "value": data("answer:first")}],
            )
            self.assertEqual(self.loop.step(), 1)
            self.assertEqual(self.log, ["first", "second"])
            self.assertEqual(
                [frame["id"] for frame in self.channel.pending()],
                ["main:1", "main:2"],
                "the answers keep the frame order",
            )
            peer.close()

    def test_a_timer_runs_on_the_test_s_clock(self):
        # The loop's clock is virtual: the callback a plugin scheduled 30
        # seconds out runs when the test advances 30 seconds, not before.
        with self.loop.entered():
            fired = []

            def fire():
                peer.notify("host", "timer", [])
                fired.append(True)

            def dispatch(target, method, args):
                if method == "arm":
                    self.loop.call_later(30, fire)
                    return "armed"
                return method

            peer = Peer(self.channel, dispatch, endpoint=LOCAL)
            peer.receive(hello())
            peer.receive(invoke("main:1", "arm"))
            self.loop.run_ready()
            self.assertEqual(self.channel.take("the answer", lambda frame: frame["op"] == "return"), {
                "op": "return", "id": "main:1", "value": data("armed"),
            })
            self.assertEqual(fired, [], "30 s have not passed")
            self.assertEqual(self.loop.advance(29), 0, "no timer is due at 29 s")
            self.loop.run_ready()
            self.assertEqual(fired, [])
            self.assertEqual(self.loop.advance(1), 1, "the 30 s timer is due")
            self.loop.run_ready()
            self.assertEqual(fired, [True], "the timer asked the far end, once")
            # The timer ran in the call's context: the notification carries
            # the chain of the call that scheduled it.
            self.assertEqual(
                [frame for frame in self.channel.pending() if frame["op"] == "invoke"],
                [{
                    "op": "invoke", "id": "py:1", "path": ["main:1"], "target": "host", "method": "timer",
                    "args": {"type": "data", "value": []},
                }],
            )
            peer.close()


class CancelTests(background.TestCase):
    """Cancellation travels through the loop: a call's task is found and
    cancelled when the cancel is read, and its signal is set."""

    def setUp(self):
        self.loop = SteppedLoop()
        self.channel = ScriptedChannel()
        self.events = []

    def tearDown(self):
        self.loop.finish()

    # guarantee: bridge/cancel-reaches-callee
    def test_a_cancel_read_while_the_method_runs_reaches_its_signal_and_its_task(self):
        with self.loop.entered():
            def dispatch(target, method, args):
                if method == "listen":
                    return listen(args[0])
                return method

            async def listen(signal):
                self.events.append(("started", signal.cancelled))
                try:
                    await signal.wait()
                except asyncio.CancelledError:
                    self.events.append(("task cancelled", signal.cancelled))
                    return "cancelled"
                return "done"

            peer = Peer(self.channel, dispatch, endpoint=LOCAL)
            peer.receive(hello())
            peer.receive(invoke("main:1", "listen", {"type": "list", "value": [{"type": "signal"}]}))
            self.loop.run_until_idle()
            # The method is running and a future reference answers the call.
            self.assertEqual(self.events, [("started", False)])
            reference = self.channel.take(
                "the reply", lambda frame: frame["op"] == "return" and frame["id"] == "main:1"
            )["value"]
            self.assertEqual(reference["value"]["kind"], "future")
            # The cancel names the call, not the future: the session maps it
            # back to the signal and the task.
            peer.receive({"op": "cancel", "id": "main:1"})
            self.loop.run_ready()
            self.assertEqual(self.events, [("started", False), ("task cancelled", True)])
            # The task's outcome is what an await of the future would read.
            peer.receive({
                "op": "await", "id": "main:2", "path": [], "reference": reference["value"]["id"],
            })
            self.loop.run_ready()
            self.assertEqual(
                self.channel.pending(),
                [{"op": "return", "id": "main:2", "value": data("cancelled")}],
            )
            peer.close()

    # guarantee: bridge/cancel-reaches-callee
    def test_a_cancel_read_before_the_deferred_call_runs_drops_it_without_an_answer(self):
        # The frame was read, but the loop has not run its job: a cancel read
        # first must not be lost. Under SESS-57 a call not started is dropped
        # and never answered, as Rust's callee does.
        with self.loop.entered():
            def dispatch(target, method, args):
                self.events.append(("started", method))
                return method

            peer = Peer(self.channel, dispatch, endpoint=LOCAL)
            peer.receive(hello())
            peer.receive(invoke("main:1", "work"))
            peer.receive({"op": "cancel", "id": "main:1"})
            self.assertEqual(self.events, [], "the method has not started when the cancel is read")
            self.loop.run_until_idle()
            self.assertEqual(self.events, [], "a call cancelled while queued never runs")
            self.assertEqual(self.channel.pending(), [], "and is not answered (SESS-57)")
            # Dropping it settles the call here: nothing stays in flight.
            draining = self.loop.create_task(peer.drain())
            self.loop.run_until_idle()
            self.assertTrue(draining.done(), "a call dropped before it started is not left in flight")
            # A cancel for a call this side does not have is ignored (SESS-58).
            peer.receive({"op": "cancel", "id": "main:99"})
            self.assertIsNone(peer.closed_error, "an unknown cancel does not end the session")
            # The session goes on: the call behind it runs and is answered.
            peer.receive(invoke("main:2", "later"))
            self.loop.run_until_idle()
            self.assertEqual(self.events, [("started", "later")])
            self.assertEqual(
                self.channel.pending(),
                [{"op": "return", "id": "main:2", "value": data("later")}],
            )
            peer.close()

    # guarantee: bridge/cancel-reaches-callee
    def test_a_cancel_read_before_a_deferred_await_runs_drops_it_without_an_answer(self):
        # SESS-57's second bullet: an `await` not started is dropped without a
        # reply too, and it settles the call the session counts as active.
        with self.loop.entered():
            async def later():
                return "value"

            def dispatch(target, method, args):
                self.events.append(("started", method))
                return later()

            peer = Peer(self.channel, dispatch, endpoint=LOCAL)
            peer.receive(hello())
            peer.receive(invoke("main:1", "later"))
            self.loop.run_until_idle()
            reference = self.channel.take("the reply", lambda frame: frame["op"] == "return")["value"]
            self.assertEqual(reference["value"]["kind"], "future")
            peer.receive({"op": "await", "id": "main:2", "path": [], "reference": reference["value"]["id"]})
            peer.receive({"op": "cancel", "id": "main:2"})
            self.loop.run_until_idle()
            self.assertEqual(self.channel.pending(), [], "an await cancelled while queued is not answered")
            draining = self.loop.create_task(peer.drain())
            self.loop.run_until_idle()
            self.assertTrue(draining.done(), "the dropped await is settled")
            peer.close()


class SynchronousWaitTests(background.TestCase):
    """A synchronous call blocks the loop thread and pumps the frames its
    answer comes through; the test hands each frame over only after the
    previous one was answered (peer.py `_call_sync`)."""

    # guarantee: boundary/5.6
    def test_a_synchronous_call_pumps_the_frames_between_the_call_and_its_reply(self):
        channel = ScriptedChannel()
        log = []
        started = threading.Event()
        failure = []
        # What the serving thread made: the test ends them from here.
        made = {}

        def dispatch(target, method, args):
            log.append(("start", method))
            if method == "outer":
                value = made["peer"].call("host", "inner", [])
                log.append(("inner", value))
                return f"outer:{value}"
            return method

        def serve():
            loop = asyncio.new_event_loop()
            asyncio.set_event_loop(loop)
            made["loop"] = loop
            try:
                async def setup():
                    peer = Peer(channel, dispatch, endpoint=LOCAL)
                    peer.start()
                    return peer

                made["peer"] = loop.run_until_complete(setup())
                started.set()
                loop.run_forever()
            except BaseException as error:  # noqa: BLE001 - reported by the test
                failure.append(error)
            finally:
                loop.close()

        thread = threading.Thread(target=serve, name="scripted runtime", daemon=True)
        thread.start()
        try:
            self.assertTrue(started.wait(HANG_GUARD), "the runtime did not start")
            # The greeting goes out first, then the far end's.
            self.assertEqual(channel.take("the greeting", lambda frame: frame["op"] == "hello")["endpoint"], "py")
            channel.feed(hello())
            # A synchronous call: its method calls back into the far end
            # before it returns, and that call's frame is the proof the loop
            # thread now waits in the pump.
            channel.feed(invoke("main:1", "outer", sync=True))
            inner = channel.take(
                "the inner call", lambda frame: frame["op"] == "invoke" and frame.get("method") == "inner"
            )
            self.assertEqual(log, [("start", "outer")], "the outer method waits for the inner answer")
            self.assertTrue(inner["sync"], "the nested call says it waits")
            channel.feed({"op": "return", "id": inner["id"], "value": data("inner-result")})
            outer = channel.take(
                "the outer answer", lambda frame: frame["op"] == "return" and frame["id"] == "main:1"
            )
            self.assertEqual(outer["value"], data("outer:inner-result"))
            self.assertEqual(log, [("start", "outer"), ("inner", "inner-result")])
            # A frame arriving after the wait is deferred like any other.
            channel.feed(invoke("main:2", "after"))
            after = channel.take(
                "the next answer", lambda frame: frame["op"] == "return" and frame["id"] == "main:2"
            )
            self.assertEqual(after["value"], data("after"))
            self.assertEqual(log[-1], ("start", "after"))
            self.assertEqual(failure, [])
        finally:
            # End the session on its loop, then the loop; a failure in the
            # test does not leave the thread running.
            loop = made.get("loop")
            if loop is not None:
                loop.call_soon_threadsafe(made["peer"].close)
                loop.call_soon_threadsafe(loop.stop)
            channel.close("the test is done")
            thread.join(HANG_GUARD)
            self.assertFalse(thread.is_alive(), "the runtime thread did not stop")


class ControlFrameTests(background.TestCase):
    """A control operation — an `invoke` whose `target` is empty (session
    protocol SESS-6: `events.*`, `rows.*` 等) — that arrives while a
    synchronous call waits is taken in place, in frame order: the frame
    behind it, the call's reply, sees its effect (#630 "控制帧原地处理").

    The loop thread is inside the pump here (peer.py `_call_sync`): the test
    hands over both frames, the control one first, and takes the answers in
    the order they were written.
    """

    def test_a_control_call_arriving_during_a_synchronous_wait_is_taken_in_place(self):
        channel = ScriptedChannel()
        runtime = Runtime()
        made = {}
        failure = []
        log = []

        def dispatch(target, method, args):
            if target == "":
                return runtime.dispatch(target, method, args)
            log.append(("start", method))
            if method == "outer":
                inner = made["peer"].call("host", "inner", [])
                log.append(("resumed", inner))
                # The control frame came first: the service it provided is
                # here before the reply that resumed this call.
                return f"outer:{inner}:{sorted(runtime.hosts)}"
            return method

        def serve():
            loop = asyncio.new_event_loop()
            asyncio.set_event_loop(loop)
            made["loop"] = loop
            try:
                async def setup():
                    peer = Peer(channel, dispatch, endpoint=LOCAL)
                    runtime.peer = peer
                    peer.start()
                    return peer

                made["peer"] = loop.run_until_complete(setup())
                made["started"].set()
                loop.run_forever()
            except BaseException as error:  # noqa: BLE001 - reported by the test
                failure.append(error)
            finally:
                loop.close()

        made["started"] = threading.Event()
        thread = threading.Thread(target=serve, name="scripted runtime", daemon=True)
        thread.start()
        try:
            self.assertTrue(made["started"].wait(HANG_GUARD), "the runtime did not start")
            self.assertEqual(channel.take("the greeting", lambda frame: frame["op"] == "hello")["endpoint"], "py")
            channel.feed(hello())
            channel.feed(invoke("main:1", "outer", sync=True))
            inner = channel.take(
                "the inner call", lambda frame: frame["op"] == "invoke" and frame.get("method") == "inner"
            )
            # The host's control call arrives while the loop thread waits in
            # the pump; the reply behind it is what lets the call go on.
            channel.feed({
                "op": "invoke", "id": "main:2", "path": [], "target": "",
                "method": "hosts.provide", "args": data(["status", [], None]),
            })
            channel.feed({"op": "return", "id": inner["id"], "value": data("inner-result")})
            # Taken in place, before the next frame: its answer goes out
            # first, then the answer of the call the reply resumed.
            control = channel.take("the control call's answer", lambda frame: frame["op"] in ("return", "throw"))
            self.assertEqual(control, {"op": "return", "id": "main:2", "value": data(None)})
            outer = channel.take("the call's answer", lambda frame: frame["op"] in ("return", "throw"))
            self.assertEqual(outer["id"], "main:1")
            self.assertEqual(outer["value"], data("outer:inner-result:['status']"))
            self.assertEqual(log, [("start", "outer"), ("resumed", "inner-result")])
            self.assertEqual(failure, [])
        finally:
            loop = made.get("loop")
            peer = made.get("peer")
            if loop is not None and peer is not None:
                loop.call_soon_threadsafe(peer.close)
                loop.call_soon_threadsafe(loop.stop)
            channel.close("the test is done")
            thread.join(HANG_GUARD)
            self.assertFalse(thread.is_alive(), "the runtime thread did not stop")


class Count:
    """A service a plugin provides: a method the host may call."""

    def count(self):
        return 1


class RuntimeOnASteppedLoop(background.TestCase):
    """The runtime's rows on a loop the test steps: what a plugin thread
    hands to `ctx.on` / `ctx.provide` is marshalled onto the loop, and the
    unload that follows is ordered by the test, not the scheduler (#630
    "线程里的 ctx.on / provide 与卸载")."""

    def setUp(self):
        self.loop = SteppedLoop()
        self.channel = ScriptedChannel()
        self.runtime = Runtime()
        self.contexts = {}
        self.entered = self.loop.entered()
        self.entered.__enter__()
        self.addCleanup(self.closed)
        self.peer = Peer(self.channel, self.runtime.dispatch, endpoint=LOCAL)
        self.runtime.peer = self.peer
        self.peer.receive(hello())

    def closed(self):
        peer = getattr(self, "peer", None)
        if peer is not None:
            peer.close()
        self.loop.finish()
        self.entered.__exit__(None, None, None)

    def load(self, key, apply):
        entry = types.ModuleType(f"scheduled_{key}")
        entry.apply = apply
        sys.modules[entry.__name__] = entry
        loading = self.loop.create_task(self.runtime.load(key, entry.__name__, {}, None))
        self.step_until("the row to load", loading.done)
        loading.result()

    def step_until(self, what, check, limit=1000):
        for _ in range(limit):
            if check():
                return
            if not self.loop.step():
                self.fail(f"nothing was ready before {what}")
        self.fail(f"{what} did not happen in {limit} steps")

    def thread(self, call):
        """`call()` on a thread of its own; the exceptions it raised."""
        raised = []

        def run():
            try:
                call()
            except BaseException as error:  # noqa: BLE001 - raised by the test
                raised.append(error)

        thread = threading.Thread(target=run, name="a plugin thread")
        thread.start()
        thread.join()
        return raised

    def test_a_thread_s_registration_and_service_go_out_in_call_order_before_the_unload(self):
        def apply(ctx, config):
            self.contexts["a"] = ctx

        self.load("a", apply)
        ctx = self.contexts["a"]
        self.assertEqual(self.thread(lambda: (ctx.on("t/x", lambda: None), ctx.provide("tally", Count()))), [])
        self.assertEqual(self.channel.pending(), [], "a thread's work waits for the loop to run it")

        self.step_until("the registration and the announcement", lambda: len(self.channel.pending()) >= 2)
        frames = self.channel.pending()
        self.assertEqual(
            [frame["method"] for frame in frames],
            ["events.on", "service"],
            "the thread's calls keep their order on the loop",
        )
        registration, announcement = frames
        self.assertEqual(announcement["args"]["value"][0], "tally")
        self.assertEqual(announcement["args"]["value"][3]["row"], "a")

        # The host answers both before the row unloads.
        for frame in (registration, announcement):
            self.channel.take("an answer", lambda sent, id=frame["id"]: sent["id"] == id)
            self.peer.receive({"op": "return", "id": frame["id"], "value": {"type": "undefined"}})
        self.loop.run_ready()

        unloading = self.loop.create_task(self.runtime.unload("a"))
        self.step_until("the row to unload", unloading.done)
        self.loop.run_ready()
        frames = self.channel.pending()
        self.assertEqual(
            [frame["method"] for frame in frames],
            ["events.off", "service"],
            "the unload withdraws what the thread registered, after it went out",
        )
        withdrawal, gone = frames
        listener = registration["args"]["value"][0]["id"]
        self.assertEqual(withdrawal["args"]["value"], [[listener]], "the listener the registration made")
        self.assertEqual(gone["args"]["value"][0], "tally")
        self.assertIsNone(gone["args"]["value"][1], "the service is withdrawn")
        self.assertGreater(gone["args"]["value"][2], announcement["args"]["value"][2])
        self.assertEqual(self.runtime.rows, {})
        self.assertEqual(self.runtime.services, {})
        self.assertEqual(self.runtime.bus.entries, [])

    def test_a_registration_from_a_thread_after_the_unload_started_is_refused(self):
        release = asyncio.Event()

        def apply(ctx, config):
            self.contexts["a"] = ctx

            async def cleanup():
                await release.wait()

            ctx.effect(cleanup)

        self.load("a", apply)
        ctx = self.contexts["a"]
        unloading = self.loop.create_task(self.runtime.unload("a"))
        self.loop.step()
        self.assertTrue(self.runtime.rows["a"].unloading, "the row is on its unload stack")
        self.assertFalse(unloading.done(), "a cleanup is waiting")

        raised = self.thread(lambda: ctx.on("t/late", lambda: None))
        self.assertEqual([str(error) for error in raised], ["row a is not active"])
        self.assertEqual(
            [frame for frame in self.channel.pending() if frame.get("method") == "events.on"],
            [],
            "a refused registration sends nothing",
        )
        release.set()
        self.step_until("the unload", unloading.done)

    # guarantee: sdk/unload-order
    # `ctx.provide` has the guard `ctx.on` / `ctx.effect` have, as Rust's
    # InactiveEffect (#1143): the service of a row on its way out is not
    # registered and the host is not told about it.
    def test_a_service_provided_after_the_unload_started_is_refused(self):
        release = asyncio.Event()

        def apply(ctx, config):
            self.contexts["a"] = ctx

            async def cleanup():
                await release.wait()

            ctx.effect(cleanup)

        self.load("a", apply)
        ctx = self.contexts["a"]
        unloading = self.loop.create_task(self.runtime.unload("a"))
        self.loop.step()
        self.assertTrue(self.runtime.rows["a"].unloading, "the row is on its unload stack")

        refused = self.thread(lambda: ctx.provide("late", Count()))
        self.assertEqual(
            [(type(error).__name__, str(error)) for error in refused],
            [("RuntimeError", "row a is not active")],
            "providing on a row on its way out is refused",
        )
        self.assertNotIn("late", self.runtime.services, "nothing is registered")
        release.set()
        self.step_until("the unload", unloading.done)
        self.loop.run_ready()
        self.assertEqual(self.runtime.services, {}, "nothing a row on its way out provided is left")
        self.assertEqual(
            [frame for frame in self.channel.pending() if frame.get("method") == "service"],
            [],
            "the host is not told about it",
        )

