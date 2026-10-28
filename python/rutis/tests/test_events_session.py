"""Unified events between the runtime and its host (#464), against a
scripted host on a socketpair: the `events-v2` handshake, registration,
name presence, dispatch through the host, and the host calling this
process's listeners. docs/design-events-protocol-2026-10-18.md §2, §4, §5,
§7."""

import asyncio
import contextlib
import io
import json
import queue
import socket
import sys
import threading
import types
import unittest

import background
from support import HANG_GUARD, LOCAL, eventually, hello, within

from rutis import BindingError
from rutis.advanced import AggregateError, AsyncListener
from rutis.dialing import INCOMPATIBLE, session_failure
from rutis.peer import CAPABILITIES, EVENTS_REFUSAL, Incompatible, Peer
from rutis.runner import Runtime


def module(name: str, apply) -> str:
    plugin = types.ModuleType(name)
    plugin.apply = apply
    sys.modules[name] = plugin
    return name


def value(wire):
    """A wire value as the test reads it: references as {"$ref": id}."""
    kind = wire.get("type")
    if kind == "data":
        return wire["value"]
    if kind == "undefined":
        return None
    if kind == "list":
        return [value(item) for item in wire["value"]]
    if kind == "record":
        return {key: value(item) for key, item in wire["value"].items()}
    if kind == "reference":
        return {"$ref": wire["value"]["id"], "kind": wire["value"]["kind"]}
    raise ValueError(f"unexpected wire value {wire!r}")


def data(item):
    return {"type": "data", "value": item}


class Host:
    """The host's end of a session with a real runtime."""

    def __init__(self, capabilities=("objects", "events-v2", "sync-wait")):
        ours, self.sock = socket.socketpair()
        self.sock.settimeout(HANG_GUARD)
        self.capabilities = list(capabilities)
        self.runtime = Runtime()
        self.peer = Peer(ours, self.runtime.dispatch, endpoint=LOCAL, require={"events-v2": EVENTS_REFUSAL})
        self.runtime.peer = self.peer
        self.frames: queue.Queue = queue.Queue()
        self.held: list = []
        self.calls = 0
        self.lines = self.sock.makefile("rb")
        threading.Thread(target=self._read, name="fake host", daemon=True).start()

    def _read(self):
        try:
            for line in self.lines:
                self.frames.put(json.loads(line))
        except (OSError, ValueError):
            pass
        self.frames.put(None)

    def start(self) -> dict:
        self.peer.start()
        greeting = self.take(lambda frame: frame["op"] == "hello")
        self.send(hello(self.capabilities))
        return greeting

    def close(self):
        self.peer.close()
        self.peer._channel.close()
        try:
            # Ends the reader's read before its file closes.
            self.sock.shutdown(socket.SHUT_RDWR)
        except OSError:
            pass
        for closing in (self.lines, self.sock):
            try:
                closing.close()
            except OSError:
                pass

    def send(self, frame):
        self.sock.sendall(json.dumps(frame).encode() + b"\n")

    def take(self, matches):
        """The first frame (held or next) that `matches`, blocking."""
        for frame in self.held:
            if matches(frame):
                self.held.remove(frame)
                return frame
        while True:
            frame = self.frames.get(timeout=HANG_GUARD)
            if frame is None:
                raise ConnectionError("the runtime closed the session")
            if matches(frame):
                return frame
            self.held.append(frame)

    async def next(self, matches):
        return await asyncio.to_thread(self.take, matches)

    async def request(self, method):
        """The next call the runtime made to the host's `method`."""
        frame = await self.next(lambda f: f["op"] == "invoke" and f.get("method") == method)
        frame["value"] = value(frame["args"])
        return frame

    def invoke(self, method, args) -> str:
        self.calls += 1
        call = f"main:{self.calls}"
        self.send({"op": "invoke", "id": call, "path": [], "target": "", "method": method, "args": args})
        return call

    def call(self, reference, args) -> str:
        self.calls += 1
        call = f"main:{self.calls}"
        self.send({"op": "call", "id": call, "path": [], "reference": reference, "args": data(args)})
        return call

    async def outcome(self, call):
        """("return", value) or ("throw", error) of the host's call; a
        returned future is awaited."""
        frame = await self.next(lambda f: f["op"] in ("return", "throw") and f["id"] == call)
        if frame["op"] == "throw":
            return "throw", frame["error"]
        wire = frame["value"]
        if wire.get("type") == "reference" and wire["value"]["kind"] == "future":
            self.calls += 1
            waited = f"main:{self.calls}"
            self.send({"op": "await", "id": waited, "path": [], "reference": wire["value"]["id"]})
            return await self.outcome(waited)
        return "return", value(wire)

    def answer(self, frame, result=None):
        self.send({"op": "return", "id": frame["id"], "value": data(result)})

    def fail(self, frame, error):
        self.send({"op": "throw", "id": frame["id"], "error": error})


class Session(background.IsolatedAsyncioTestCase):
    async def asyncSetUp(self):
        self.host = Host()
        self.addCleanup(self.host.close)
        self.contexts = {}
        await asyncio.to_thread(self.host.start)
        await within("the handshake", self.host.peer.ready)

    async def mount(self, elsewhere=None):
        self.host.invoke("events.elsewhere", data([elsewhere or {"add": []}]))
        call = self.host.invoke("mount", data({"plugins": [], "announce": True, "runtime": "py"}))
        return await self.host.outcome(call)

    async def load(self, key, apply=None, scope=None, registrations=0):
        """Load the row `key`, answering its `events.on` calls."""

        def keep(ctx, config):
            self.contexts[key] = ctx
            if apply is not None:
                apply(ctx, config)

        entry = module(f"events_session_{key}", keep)
        call = self.host.invoke(
            "rows.load", data([key, entry, {}, [], [], {}, {"scope": scope or []}])
        )
        registered = []
        for _ in range(registrations):
            frame = await self.host.request("events.on")
            registered.append(frame["value"][0])
            self.host.answer(frame)
        self.assertEqual(await self.host.outcome(call), ("return", None))
        return registered

    async def elsewhere(self, **change):
        self.host.invoke("events.elsewhere", data([change]))
        # Handled in frame order: a call after it sees it.
        call = self.host.invoke("rows.schema", data(["events_session_probe_missing"]))
        await self.host.outcome(call)


class Handshake(Session):
    async def test_the_runtime_declares_events_v2(self):
        self.assertIn("events-v2", CAPABILITIES)

    async def test_mount_has_no_events_feature_and_refuses_the_0_12_declarations(self):
        outcome, reply = await self.mount()
        self.assertEqual(outcome, "return")
        self.assertNotIn("events", reply["features"])
        for field in ("emits", "events"):
            call = self.host.invoke("mount", data({field: ["tick"]}))
            outcome, error = await self.host.outcome(call)
            self.assertEqual(outcome, "throw")
            self.assertEqual(
                error["message"],
                f"the mount argument {field} was removed in rutis 0.13: listeners register themselves; "
                "emit with ctx.emit / parallel / serial / waterfall / bail (https://github.com/arcships/rutis/blob/main/docs/migration-0.13.md)",
            )


class OldHost(background.IsolatedAsyncioTestCase):
    async def test_a_host_without_events_v2_ends_the_session_after_the_handshake(self):
        # §2.2: the runtime sends nothing more and says which end to upgrade.
        host = Host(capabilities=["sync-wait"])
        self.addCleanup(host.close)
        await asyncio.to_thread(host.start)
        await within("the session end", host.peer.closed)
        self.assertIsInstance(host.peer.closed_error, Incompatible)
        self.assertEqual(
            str(host.peer.closed_error),
            "this host does not declare events-v2 (the event protocol of rutis 0.13): upgrade rutis-host to 0.13 or later",
        )
        self.assertEqual(session_failure(host.peer.closed_error, None), INCOMPATIBLE, "a dialing runtime stops")
        with self.assertRaises(ConnectionError):
            await asyncio.to_thread(host.take, lambda frame: True)


class Registration(Session):
    async def test_listeners_register_before_the_row_loads_and_withdraw_on_off(self):
        # §4.1, §4.2, §4.4 rule 2.
        await self.mount()
        offs = []

        async def created(session):
            return session

        def apply(ctx, config):
            ctx.on("session/created", created)
            offs.append(ctx.on("session/closed", lambda: None, prepend=True, global_=True))

        scope = [{"id": "17", "group": "tenant"}]
        registered = await self.load("chat", apply, scope, registrations=2)
        self.assertEqual(
            registered,
            [
                {"id": "1", "name": "session/created", "row": "chat", "plugin": "chat",
                 "kind": "async", "prepend": False, "global": False},
                {"id": "2", "name": "session/closed", "row": "chat", "plugin": "chat",
                 "kind": "any", "prepend": True, "global": True},
            ],
        )
        offs[0]()
        frame = await self.host.request("events.off")
        self.assertEqual(frame["value"], [["2"]])
        self.assertEqual(self.host.runtime.rows["chat"].scope, scope)

    async def test_the_row_is_answered_only_once_its_registrations_are(self):
        await self.mount()
        entry = module("events_session_waiting", lambda ctx, config: ctx.on("t/x", lambda: None))
        call = self.host.invoke("rows.load", data(["w", entry, {}, [], [], {}, {"scope": []}]))
        frame = await self.host.request("events.on")
        await asyncio.sleep(0.05)
        self.assertIsNotNone(self.host.runtime.rows["w"].pending, "rows.load still waits for the host")
        self.host.answer(frame)
        self.assertEqual(await self.host.outcome(call), ("return", None))

    async def test_unloading_withdraws_and_refuses_later_calls(self):
        await self.mount()
        await self.load("a", lambda ctx, config: ctx.on("t/x", lambda: "A"), registrations=1)
        call = self.host.invoke("rows.unload", data(["a"]))
        frame = await self.host.request("events.off")
        self.assertEqual(frame["value"], [["1"]])
        self.assertEqual(await self.host.outcome(call), ("return", None))
        call = self.host.invoke("events.call", data([{"listener": "1", "dispatch": "d1", "own": False, "name": "t/x", "mode": "serial", "args": []}]))
        outcome, error = await self.host.outcome(call)
        self.assertEqual((outcome, error["name"], error["message"]), ("throw", "BindingError", "the listener of t/x is withdrawn"))


class ThroughTheHost(Session):
    async def test_a_name_listened_to_elsewhere_is_dispatched_by_the_host(self):
        # §5.2, §5.3: the request names the row, never a scope.
        await self.mount({"add": ["t/x"]})
        log = []
        await self.load("a", lambda ctx, config: ctx.on("t/x", lambda value: log.append(value)), registrations=1)
        ctx = self.contexts["a"]
        dispatch = asyncio.ensure_future(ctx.serial("t/x", 1))
        frame = await self.host.request("events.dispatch")
        self.assertEqual(frame["value"], [{"row": "a", "name": "t/x", "mode": "serial", "args": [1]}])
        self.assertEqual(log, [], "its own listener is called back by the host, not here")
        self.host.answer(frame, "R")
        self.assertEqual(await within("the dispatch", dispatch), "R")
        # Prefixes count too; removed names are dispatched here again.
        await self.elsewhere(remove=["t/x"], addPrefixes=["t/"])
        dispatch = asyncio.ensure_future(ctx.parallel("t/x", 2))
        frame = await self.host.request("events.dispatch")
        self.host.answer(frame)
        self.assertIsNone(await within("the dispatch", dispatch))
        await self.elsewhere(removePrefixes=["t/"])
        self.assertIsNone(await ctx.serial("t/x", 3))
        self.assertEqual(log, [3])

    async def test_the_filter_runs_here_as_a_selector(self):
        # §6: one call with the descriptors, a boolean each.
        await self.mount({"add": ["t/f"]})
        await self.load("a")
        seen = []
        dispatch = asyncio.ensure_future(
            self.contexts["a"].filter(lambda d: seen.append(d["row"]) or d["row"] == "chat").serial("t/f")
        )
        frame = await self.host.request("events.dispatch")
        selector = frame["value"][0]["filter"]
        self.assertEqual(selector["kind"], "function")
        call = self.host.call(selector["$ref"], [[{"row": "chat"}, {"row": "other"}]])
        self.assertEqual(await self.host.outcome(call), ("return", [True, False]))
        self.assertEqual(seen, ["chat", "other"])
        self.host.answer(frame)
        await within("the dispatch", dispatch)

    async def test_the_terminal_is_a_function_the_host_calls(self):
        await self.mount({"add": ["t/wf"]})
        await self.load("a")
        dispatch = asyncio.ensure_future(self.contexts["a"].waterfall("t/wf", "Oslo", terminal=lambda city: f"end {city}"))
        frame = await self.host.request("events.dispatch")
        request = frame["value"][0]
        self.assertEqual((request["mode"], request["args"]), ("waterfall", ["Oslo"]))
        call = self.host.call(request["terminal"]["$ref"], ["Oslo"])
        self.assertEqual(await self.host.outcome(call), ("return", "end Oslo"))
        self.host.answer(frame, "end Oslo")
        self.assertEqual(await within("the dispatch", dispatch), "end Oslo")

    async def test_a_host_aggregate_error_is_a_rutis_aggregate_error(self):
        await self.mount({"add": ["t/par"]})
        await self.load("a")
        dispatch = asyncio.ensure_future(self.contexts["a"].parallel("t/par"))
        frame = await self.host.request("events.dispatch")
        self.host.fail(frame, {
            "name": "AggregateError",
            "message": "2 listeners of t/par failed",
            "graph": {"root": {"type": "reference", "value": 0}, "nodes": [
                {"type": "error", "name": "AggregateError", "message": "2 listeners of t/par failed",
                 "errors": [{"type": "reference", "value": 1}, {"type": "reference", "value": 2}]},
                {"type": "error", "name": "AErr", "message": "a"},
                {"type": "error", "name": "BindingError", "message": "listener 3 of t/par is withdrawn"},
            ]},
        })
        with self.assertRaises(AggregateError) as raised:
            await within("the dispatch", dispatch)
        self.assertEqual(raised.exception.message, "2 listeners of t/par failed")
        self.assertEqual([error.name for error in raised.exception.errors], ["AErr", "BindingError"])
        self.assertIsInstance(raised.exception.errors[1], BindingError)

    async def test_emits_through_the_host_keep_their_order(self):
        # §5.5: while an emit of a name through the host is unanswered, the
        # next ones follow it there, even once nothing is elsewhere.
        await self.mount({"add": ["t/e"]})
        log = []
        await self.load("a", lambda ctx, config: ctx.on("t/e", lambda n: log.append(n)), registrations=1)
        ctx = self.contexts["a"]
        self.assertIsNone(ctx.emit("t/e", 1))
        first = await self.host.request("events.dispatch")
        await self.elsewhere(remove=["t/e"])
        ctx.emit("t/e", 2)
        second = await self.host.request("events.dispatch")
        self.assertEqual([first["value"][0]["args"], second["value"][0]["args"]], [[1], [2]])
        self.assertEqual(log, [])
        self.host.answer(first)
        self.host.answer(second)
        await eventually("both answered", lambda: not self.host.runtime.unanswered)
        ctx.emit("t/e", 3)
        self.assertEqual(log, [3], "the fast path again")

    async def test_an_emit_reply_is_taken_before_the_next_frame(self):
        # §5.5 (2026-10-21): the reply counts the emit as answered as its
        # frame is read, so a call the host sends right after sees it.
        await self.mount({"add": ["t/e"]})
        seen = []
        runtime = self.host.runtime
        registered = await self.load(
            "a", lambda ctx, config: ctx.on("t/e", lambda n: seen.append((n, dict(runtime.unanswered)))), registrations=1
        )
        self.contexts["a"].emit("t/e", 1)
        frame = await self.host.request("events.dispatch")
        self.assertEqual(runtime.unanswered, {"t/e": 1})
        # The host's listener call for it, its reply, a call after both.
        self.host.invoke("events.call", data([{"listener": registered[0]["id"], "dispatch": "d1", "own": True, "name": "t/e", "mode": "emit", "args": [1]}]))
        self.host.answer(frame)
        self.host.invoke("events.call", data([{"listener": registered[0]["id"], "dispatch": "d2", "own": False, "name": "t/e", "mode": "emit", "args": [2]}]))
        await eventually("both listener calls", lambda: len(seen) == 2)
        # In frame order; the call after the reply sees the emit answered.
        self.assertEqual([n for n, _ in seen], [1, 2])
        self.assertEqual(seen[1], (2, {}))

    async def test_a_0_13_hosts_future_answer_counts_once_it_settles(self):
        # A 0.13 host answers a queued emit with a future: the emit stays
        # unanswered until it settles.
        await self.mount({"add": ["t/e"]})
        await self.load("a")
        runtime = self.host.runtime
        self.contexts["a"].emit("t/e", 1)
        frame = await self.host.request("events.dispatch")
        self.host.send({"op": "return", "id": frame["id"], "value": {"type": "reference", "value": {"id": 1, "kind": "future", "home": False, "origin": []}}})
        waited = await self.host.next(lambda f: f["op"] == "await")
        self.assertEqual(waited["reference"], 1)
        self.assertEqual(runtime.unanswered, {"t/e": 1})
        self.host.send({"op": "return", "id": waited["id"], "value": {"type": "undefined"}})
        await eventually("answered once settled", lambda: not runtime.unanswered)

    async def test_a_failed_emit_through_the_host_is_reported(self):
        await self.mount({"add": ["t/e"]})
        await self.load("a")
        stderr = io.StringIO()
        with contextlib.redirect_stderr(stderr):
            self.contexts["a"].emit("t/e")
            frame = await self.host.request("events.dispatch")
            self.host.fail(frame, {"name": "BindingError", "message": "row a is not loaded in this runtime"})
            await eventually("the report", stderr.getvalue)
        self.assertEqual(stderr.getvalue(), "rutis: emit of t/e from row a failed: BindingError: row a is not loaded in this runtime\n")

    async def test_bail_through_the_host_is_a_synchronous_call(self):
        await self.mount({"add": ["t/b"]})
        await self.load("a")
        seen = []

        def host():
            frame = self.host.take(lambda f: f["op"] == "invoke" and f.get("method") == "events.dispatch")
            seen.append(frame)
            self.host.answer(frame, "B")

        answering = threading.Thread(target=host, daemon=True)
        answering.start()
        self.assertEqual(self.contexts["a"].bail("t/b"), "B")
        answering.join(HANG_GUARD)
        self.assertTrue(seen[0].get("sync"), "a synchronous call says so (sync-wait)")
        self.assertEqual(value(seen[0]["args"])[0]["mode"], "bail")


class CalledByTheHost(Session):
    def call(self, listener, mode, args=(), name="t/x", **extra):
        request = {"listener": listener, "dispatch": "d1", "own": False, "name": name, "mode": mode, "args": list(args)}
        request.update(extra)
        return self.host.invoke("events.call", data([request]))

    async def test_a_listener_answers_its_value_as_is(self):
        await self.mount()
        await self.load("a", lambda ctx, config: ctx.on("t/x", lambda n: False if n else n), registrations=1)
        self.assertEqual(await self.host.outcome(self.call("1", "serial", [1])), ("return", False))
        self.assertEqual(await self.host.outcome(self.call("1", "parallel", [0])), ("return", 0))
        outcome, error = await self.host.outcome(self.call("9", "serial"))
        self.assertEqual((error["name"], error["message"]), ("BindingError", "the listener of t/x is withdrawn"))

    async def test_an_emit_listener_answers_undefined_and_reports_its_failure(self):
        await self.mount()

        async def fail():
            raise LookupError("a failed")

        await self.load("a", lambda ctx, config: ctx.on("t/x", fail), registrations=1)
        stderr = io.StringIO()
        with contextlib.redirect_stderr(stderr):
            self.assertEqual(await self.host.outcome(self.call("1", "emit")), ("return", None))
        self.assertEqual(stderr.getvalue(), "rutis: listener of t/x in row a failed: LookupError: a failed\n")

    async def test_a_waterfall_listener_gets_next_last(self):
        await self.mount()

        async def outer(city, next):
            return f"{city}:" + await next()

        await self.load("a", lambda ctx, config: ctx.on("t/x", outer), registrations=1)
        call = self.host.invoke("events.call", {"type": "list", "value": [{"type": "record", "value": {
            "listener": data("1"), "dispatch": data("d1"), "own": data(False), "name": data("t/x"),
            "mode": data("waterfall"), "args": data(["Oslo"]),
            "next": {"type": "reference", "value": {"id": 7, "kind": "function", "home": False, "origin": []}},
        }}]})
        frame = await self.host.next(lambda f: f["op"] == "call")
        self.assertEqual((frame["reference"], value(frame["args"])), (7, []))
        self.host.answer(frame, "inner")
        self.assertEqual(await self.host.outcome(call), ("return", "Oslo:inner"))

    async def test_a_synchronous_bail_refuses_an_async_listener(self):
        await self.mount()

        async def listener():
            return "A"

        await self.load("a", lambda ctx, config: (ctx.on("t/x", listener), ctx.on("t/y", lambda: "Y")), registrations=2)
        outcome, error = await self.host.outcome(self.call("1", "bail"))
        self.assertEqual(
            (outcome, error["name"], error["message"]),
            ("throw", "AsyncListener", "listener of t/x in row a is asynchronous and cannot run in a synchronous bail"),
        )
        self.assertEqual(await self.host.outcome(self.call("2", "bail", name="t/y")), ("return", "Y"))

    async def test_a_cancel_cancels_the_listener_task(self):
        # §5.7: the host gave up the dispatch.
        await self.mount()
        log = []
        started = asyncio.Event()

        async def waits():
            started.set()
            try:
                await asyncio.Event().wait()
            except asyncio.CancelledError:
                log.append("cancelled")
                raise

        await self.load("a", lambda ctx, config: ctx.on("t/x", waits), registrations=1)
        call = self.call("1", "parallel")
        await within("the listener", started.wait())
        self.host.send({"op": "cancel", "id": call})
        await eventually("the cancellation", lambda: log)
        self.assertEqual(log, ["cancelled"])

    async def test_unload_cancels_a_listener_the_host_is_running(self):
        await self.mount()
        log = []
        started = asyncio.Event()

        async def waits():
            started.set()
            try:
                await asyncio.Event().wait()
            except asyncio.CancelledError:
                log.append("b:cancelled")
                return None

        await self.load("b", lambda ctx, config: ctx.on("t/unload", waits), registrations=1)
        running = self.call("1", "parallel", name="t/unload")
        await within("the listener", started.wait())
        unload = self.host.invoke("rows.unload", data(["b"]))
        self.assertEqual(await self.host.outcome(unload), ("return", None))
        self.assertEqual(log, ["b:cancelled"])
        self.assertEqual(await self.host.outcome(running), ("return", None))


class Review(Session):
    """Rules settled across languages after #464's review."""

    async def test_the_selector_hides_the_hosts_listener_field(self):
        # The host marks the emitter's own session's entries with their
        # `listener` id; the plugin's filter sees the fast path's shape.
        await self.mount({"add": ["t/f"]})
        await self.load("a")
        seen = []
        dispatch = asyncio.ensure_future(self.contexts["a"].filter(lambda d: seen.append(d) or True).serial("t/f"))
        frame = await self.host.request("events.dispatch")
        selector = frame["value"][0]["filter"]
        call = self.host.call(selector["$ref"], [[{"row": "a", "listener": "3"}, {"row": "b"}]])
        self.assertEqual(await self.host.outcome(call), ("return", [True, True]))
        self.assertEqual(seen, [{"row": "a"}, {"row": "b"}])
        self.host.answer(frame)
        await within("the dispatch", dispatch)

    async def test_a_refused_registration_fails_the_load_and_is_not_reported(self):
        # §4.4 rule 2, §11: the load fails with the host's error and the
        # row is unloaded; no registration line for it.
        await self.mount()
        entry = module("events_session_refused", lambda ctx, config: ctx.on("t/r", lambda: None))
        stderr = io.StringIO()
        with contextlib.redirect_stderr(stderr):
            call = self.host.invoke("rows.load", data(["r", entry, {}, [], [], {}, {"scope": []}]))
            frame = await self.host.request("events.on")
            self.host.fail(frame, {"name": "BindingError", "message": "row r is not loaded in this runtime"})
            outcome, error = await self.host.outcome(call)
            await asyncio.sleep(0.05)
        self.assertEqual((outcome, error["name"], error["message"]), ("throw", "BindingError", "row r is not loaded in this runtime"))
        self.assertNotIn("r", self.host.runtime.rows)
        self.assertEqual(self.host.runtime.bus.entries, [])
        self.assertEqual(stderr.getvalue(), "")

    async def test_a_refused_registration_of_a_running_row_is_reported(self):
        await self.mount()
        await self.load("a")
        stderr = io.StringIO()
        with contextlib.redirect_stderr(stderr):
            self.contexts["a"].on("t/late", lambda: None)
            frame = await self.host.request("events.on")
            self.host.fail(frame, {"name": "BindingError", "message": "refused"})
            await eventually("the report", stderr.getvalue)
        self.assertEqual(
            stderr.getvalue(), "rutis: registration of listener of t/late in row a failed: BindingError: refused\n"
        )

    async def test_the_registration_names_the_declared_plugin_else_the_rows_id(self):
        # §4.2.
        from rutis import define_plugin

        await self.mount()
        named = types.ModuleType("events_session_named")
        named.plugin = define_plugin(lambda ctx, config: ctx.on("t/n", lambda: None), name="chat-ui")
        sys.modules["events_session_named"] = named
        anonymous = module("events_session_anonymous", lambda ctx, config: ctx.on("t/n", lambda: None))
        registered = []
        for key, entry in (("k1", "events_session_named"), ("k2", anonymous)):
            call = self.host.invoke("rows.load", data([key, entry, {}, [], [], {}, {"scope": [], "id": f"id-{key}"}]))
            frame = await self.host.request("events.on")
            registered.append((frame["value"][0]["row"], frame["value"][0]["plugin"]))
            self.host.answer(frame)
            self.assertEqual(await self.host.outcome(call), ("return", None))
        self.assertEqual(registered, [("k1", "chat-ui"), ("k2", "id-k2")])

    async def test_next_through_the_host_starts_at_once(self):
        # §5.4: next() sends its call before it is awaited.
        await self.mount()
        gate = asyncio.Event()

        async def outer(next):
            rest = next()
            await gate.wait()
            return "outer:" + await rest

        await self.load("a", lambda ctx, config: ctx.on("t/x", outer), registrations=1)
        call = self.host.invoke("events.call", {"type": "list", "value": [{"type": "record", "value": {
            "listener": data("1"), "dispatch": data("d1"), "own": data(False), "name": data("t/x"),
            "mode": data("waterfall"), "args": data([]),
            "next": {"type": "reference", "value": {"id": 7, "kind": "function", "home": False, "origin": []}},
        }}]})
        frame = await self.host.next(lambda f: f["op"] == "call")
        self.assertFalse(gate.is_set(), "sent while the listener had not awaited it")
        self.host.answer(frame, "inner")
        gate.set()
        self.assertEqual(await self.host.outcome(call), ("return", "outer:inner"))

    async def test_a_threads_registration_goes_out_before_its_emission(self):
        # §4.4 rule 1 from a plugin's own thread: both marshalled onto the
        # loop, in call order.
        await self.mount({"add": ["t/th"]})
        await self.load("a")
        ctx = self.contexts["a"]

        def plugin_thread():
            ctx.on("t/th", lambda: None)
            ctx.emit("t/th")

        await within("the thread", asyncio.to_thread(plugin_thread))
        methods = ("events.on", "events.dispatch")
        first = await self.host.next(lambda f: f["op"] == "invoke" and f.get("method") in methods)
        second = await self.host.next(lambda f: f["op"] == "invoke" and f.get("method") in methods)
        self.assertEqual([first["method"], second["method"]], list(methods))
        self.host.answer(first)
        self.host.answer(second)

    async def test_a_dispatch_through_the_host_is_sent_when_called(self):
        # §5.1 as on the fast path: the frame goes out at the call.
        await self.mount({"add": ["t/now"]})
        await self.load("a")
        dispatch = self.contexts["a"].serial("t/now")
        frame = await self.host.request("events.dispatch")
        self.host.answer(frame, "R")
        self.assertEqual(await within("the dispatch", dispatch), "R")


class NoAsyncListenerKind(unittest.TestCase):
    def test_async_listener_names_itself(self):
        self.assertEqual(AsyncListener("m").name, "AsyncListener")


if __name__ == "__main__":
    unittest.main()
