"""The session layer against a scripted peer on a socketpair."""

import asyncio
import json
import socket
import threading
import unittest

import background
from support import HANG_GUARD, within

from rutis.peer import ENDPOINT_PROTOCOL, PROTOCOL, AggregateError, Peer, decode_error, encode_error


class PeerTests(background.IsolatedAsyncioTestCase):
    async def asyncSetUp(self):
        self.ours, self.theirs = socket.socketpair()
        self.theirs.settimeout(HANG_GUARD)
        self.peer = Peer(self.ours, lambda target, method, args: None)
        self.peer._handshake = True
        self.lines = self.theirs.makefile("rb")

    async def asyncTearDown(self):
        self.peer.close()
        self.lines.close()
        self.ours.close()
        self.theirs.close()

    def read(self):
        return json.loads(self.lines.readline())

    async def test_a_finished_future_is_ready_during_a_synchronous_call(self):
        loop = asyncio.get_running_loop()
        future = loop.create_future()
        future.set_result(42)
        # Exported, but its done-callback is still only scheduled.
        reference = self.peer._encode(future, [], False)["value"]["id"]
        # Inside a synchronous call `node:1`, Rust awaits it on that chain.
        self.peer._waiting.append("node:1")
        self.peer._receive({"op": "await", "id": "rust:1", "path": ["node:1"], "reference": reference})
        self.peer._waiting.pop()
        reply = self.read()
        self.assertEqual(reply["op"], "return", reply)
        self.assertEqual(reply["value"], {"type": "data", "value": 42})

    async def test_a_pending_future_on_the_chain_is_a_cycle(self):
        future = asyncio.get_running_loop().create_future()
        reference = self.peer._encode(future, [], False)["value"]["id"]
        self.peer._waiting.append("node:1")
        self.peer._receive({"op": "await", "id": "rust:1", "path": ["node:1"], "reference": reference})
        self.peer._waiting.pop()
        reply = self.read()
        self.assertEqual(reply["op"], "throw")
        self.assertEqual(reply["error"]["name"], "SyncWaitCycle")
        future.cancel()



class AsyncMethodTests(background.IsolatedAsyncioTestCase):
    """`async def` methods start at once, like JS async functions (#196):
    the first step runs before the reply, so one that never suspends is
    already done, on every supported Python (eager start is native on
    3.12+, emulated by rutis._eager on 3.10 and 3.11)."""

    def session(self, dispatch):
        ours, theirs = socket.socketpair()
        theirs.settimeout(10)
        self.addCleanup(ours.close)
        self.addCleanup(theirs.close)
        peer = Peer(ours, dispatch)
        self.addCleanup(peer._channel.close)
        self.addCleanup(peer.close)
        peer._handshake = True
        lines = theirs.makefile("rb")
        self.addCleanup(lines.close)
        return peer, theirs, lambda: json.loads(lines.readline())

    @staticmethod
    def invoke(call, path):
        return {"op": "invoke", "id": call, "path": path, "target": "svc", "method": "m", "args": {"type": "data", "value": []}}

    async def test_a_never_suspending_async_method_is_ready_inside_a_synchronous_chain(self):
        # Risk P10: without eager start the task would not have run yet, and
        # awaiting it on the chain would be a (false) SyncWaitCycle.
        async def method():
            return 5

        peer, _theirs, read = self.session(lambda target, name, args: method())
        peer._waiting.append("node:1")
        peer._receive(self.invoke("rust:1", ["node:1"]))
        reply = read()
        self.assertEqual(reply["op"], "return", reply)
        reference = reply["value"]["value"]["id"]
        peer._receive({"op": "await", "id": "rust:2", "path": ["node:1"], "reference": reference})
        peer._waiting.pop()
        reply = read()
        self.assertEqual(reply["op"], "return", reply)
        self.assertEqual(reply["value"], {"type": "data", "value": 5})

    async def test_the_first_step_runs_before_the_reply(self):
        log = []
        gate = asyncio.get_running_loop().create_future()

        async def method():
            log.append("first step")
            await gate

        peer, _theirs, read = self.session(lambda target, name, args: method())
        peer._waiting.append("node:1")
        peer._receive(self.invoke("rust:1", ["node:1"]))
        peer._waiting.pop()
        self.assertEqual(log, ["first step"])
        self.assertEqual(read()["op"], "return")
        gate.set_result(None)

    async def test_an_error_in_the_first_step_or_after_it_is_thrown_to_the_awaiter(self):
        gate = asyncio.get_running_loop().create_future()

        async def at_once():
            raise ValueError("at once")

        async def later():
            await gate
            raise KeyError("later")

        methods = iter([at_once, later])
        peer, _theirs, read = self.session(lambda target, name, args: next(methods)())
        for call, name, finish in (("rust:1", "ValueError", None), ("rust:3", "KeyError", gate)):
            with self.subTest(name):
                peer._receive(self.invoke(call, []))
                reply = await asyncio.to_thread(read)
                self.assertEqual(reply["op"], "return", reply)
                reference = reply["value"]["value"]["id"]
                peer._receive({"op": "await", "id": f"rust:{int(call[5:]) + 1}", "path": [], "reference": reference})
                if finish is not None:
                    finish.set_result(None)
                reply = await asyncio.to_thread(read)
                self.assertEqual(reply["op"], "throw", reply)
                self.assertEqual(reply["error"]["name"], name)

    async def test_calls_after_the_await_carry_the_call_in_their_path(self):
        gate = asyncio.get_running_loop().create_future()
        done = asyncio.get_running_loop().create_future()
        peer = None

        async def method():
            await gate
            peer.notify("other", "m", [])
            done.set_result(None)

        peer, _theirs, read = self.session(lambda target, name, args: method())
        peer._receive(self.invoke("rust:1", []))
        self.assertEqual((await asyncio.to_thread(read))["op"], "return")
        gate.set_result(None)
        await asyncio.wait_for(done, 10)
        notification = await asyncio.to_thread(read)
        self.assertEqual(notification["op"], "invoke")
        self.assertIn("rust:1", notification["path"])

    async def test_a_cancel_that_arrives_during_the_first_step_cancels_the_method(self):
        # The first step makes a synchronous call; while it waits, the far
        # end cancels this call. That cancel was ignored before #196.
        log = []
        gate = asyncio.get_running_loop().create_future()
        peer = None

        async def method():
            peer.call("far", "m", [])
            try:
                await gate
            except asyncio.CancelledError:
                log.append("cancelled")
                raise

        peer, theirs, read = self.session(lambda target, name, args: method())
        peer.start()
        self.assertEqual(read()["op"], "hello")

        def far_end():
            # Two frames: the first step's synchronous call, answered after
            # cancelling rust:1, and the reply to rust:1. Whatever their
            # order, the call is answered, so a regression fails, not hangs.
            reply = None
            for _ in range(2):
                frame = read()
                if frame["op"] == "invoke":
                    theirs.sendall((json.dumps({"op": "cancel", "id": "rust:1"}) + "\n").encode())
                    theirs.sendall((json.dumps({"op": "return", "id": frame["id"], "value": {"type": "undefined"}}) + "\n").encode())
                else:
                    reply = frame
            return reply

        replying = asyncio.create_task(asyncio.to_thread(far_end))
        peer._receive(self.invoke("rust:1", []))
        reply = await asyncio.wait_for(replying, 10)
        self.assertEqual(reply["op"], "return", reply)
        for _ in range(10):
            if log:
                break
            await asyncio.sleep(0)
        self.assertEqual(log, ["cancelled"])
        self.assertTrue(gate.cancelled())


class MalformedFrameTests(unittest.IsolatedAsyncioTestCase):
    """Q6.2.3, risk P3: a frame of the wrong shape, or naming a call or
    reference this side does not have, ends the session; the cases of
    rutis-bridge's `malformed_and_dangling_frames_close_the_session`."""

    def session(self):
        ours, theirs = socket.socketpair()
        theirs.settimeout(10)
        self.addCleanup(ours.close)
        self.addCleanup(theirs.close)
        peer = Peer(ours, lambda target, method, args: "served")
        self.addCleanup(peer._channel.close)
        self.addCleanup(peer.close)
        return peer, theirs

    async def test_text_that_is_not_json_closes_the_session_and_the_channel(self):
        cases = {
            "malformed JSON": b'{"op":"invoke","id":"rust:1"',
            # json.loads takes bytes that start with a BOM: Rust and Node do not.
            "a leading BOM": b'\xef\xbb\xbf{"op":"cancel","id":"rust:1"}',
            "invalid UTF-8": b'{"op":"cancel","id":"rust:\xff"}',
        }
        for name, text in cases.items():
            with self.subTest(name):
                peer, theirs = self.session()
                peer.start()
                lines = theirs.makefile("rb")
                self.addCleanup(lines.close)
                self.assertEqual(json.loads(lines.readline())["op"], "hello")
                theirs.sendall(text + b"\n")
                await asyncio.wait_for(peer.closed, 10)
                self.assertIsInstance(peer.closed_error, ConnectionError)
                self.assertEqual(lines.readline(), b"", "the channel ends too")

    async def test_malformed_and_dangling_frames_close_the_session(self):
        cases = {
            "not an object": 42,
            "unknown op": {"op": "frobnicate", "id": "rust:1"},
            "wrong field type": {"op": "invoke", "id": "rust:1", "path": [], "target": 7, "method": "m", "args": {"type": "undefined"}},
            "missing field": {"op": "invoke", "id": "rust:1", "path": [], "target": "t", "args": {"type": "undefined"}},
            "unknown wire value": {"op": "invoke", "id": "rust:1", "path": [], "target": "t", "method": "m", "args": {"type": "bogus"}},
            "call of an unknown reference": {"op": "call", "id": "rust:1", "path": [], "reference": 99, "args": {"type": "undefined"}},
            "await of an unknown reference": {"op": "await", "id": "rust:1", "path": [], "reference": 99},
            "release of an unknown reference": {"op": "release", "reference": 99, "count": 1},
            "reply to an unknown call": {"op": "return", "id": "node:99", "value": {"type": "undefined"}},
            "cancel without an id": {"op": "cancel", "id": 7},
        }
        for name, frame in cases.items():
            with self.subTest(name):
                peer, _theirs = self.session()
                peer._receive({"op": "hello", "version": 2})
                peer.receive(frame)
                self.assertIsNotNone(peer.closed_error)
                self.assertTrue(peer.closed.done())

    async def test_a_cancel_for_an_unknown_call_is_ignored(self):
        peer, theirs = self.session()
        peer._receive({"op": "hello", "version": 2})
        lines = theirs.makefile("rb")
        peer.receive({"op": "cancel", "id": "rust:7"})
        peer.receive({"op": "invoke", "id": "rust:1", "path": [], "target": "t", "method": "m", "args": {"type": "data", "value": []}})
        # The call runs on the loop: the reply is read from the far end.
        reply = await asyncio.to_thread(lines.readline)
        self.assertEqual(json.loads(reply)["value"], {"type": "data", "value": "served"})
        self.assertIsNone(peer.closed_error)


class Weather:
    def today(self):
        return "monday"


class CapabilityTests(background.IsolatedAsyncioTestCase):
    """An object reference goes only to a far end that declared `objects`."""

    async def session(self, capabilities):
        ours, theirs = socket.socketpair()
        theirs.settimeout(HANG_GUARD)
        self.addCleanup(ours.close)
        self.addCleanup(theirs.close)
        peer = Peer(ours, lambda target, method, args: Weather(), endpoint={"local": "py", "expected": "main"})
        self.addCleanup(peer.close)
        lines = theirs.makefile("rb")
        peer.start()
        self.assertEqual(json.loads(lines.readline())["op"], "hello")
        hello = {"op": "hello", "version": 3, "endpoint": "main", "capabilities": capabilities}
        peer._receive(hello)
        peer._receive({"op": "invoke", "id": "main:1", "path": [], "target": "svc", "method": "get", "args": {"type": "data", "value": []}})
        # The call runs on the loop: read its reply off the loop.
        return json.loads(await within("the reply", asyncio.to_thread(lines.readline)))

    async def test_an_object_is_refused_to_a_far_end_without_objects(self):
        reply = await self.session(["signals"])
        self.assertEqual(reply["op"], "throw", reply)
        self.assertIn("cannot receive object references", reply["error"]["message"])

    async def test_an_object_goes_to_a_far_end_with_objects(self):
        reply = await self.session(["objects"])
        self.assertEqual(reply["op"], "return", reply)
        self.assertEqual(reply["value"]["type"], "reference")
        self.assertEqual(reply["value"]["value"]["kind"], "object")


class ReferenceCountTests(background.IsolatedAsyncioTestCase):
    """Risk P4: grants of an exported reference are counted, a release
    takes back exactly its count, an identifier is never reused, and an
    encoding that fails publishes no reference; the cases of Node's
    `session.test.mjs`. The far end is read on a worker thread, never on
    the loop."""

    def session(self, dispatch):
        ours, theirs = socket.socketpair()
        theirs.settimeout(10)
        self.addCleanup(ours.close)
        self.addCleanup(theirs.close)
        peer = Peer(ours, dispatch)
        self.addCleanup(peer._channel.close)
        self.addCleanup(peer.close)
        peer._receive({"op": "hello", "version": 2})
        lines = theirs.makefile("rb")
        self.addCleanup(lines.close)

        async def read():
            return json.loads(await asyncio.to_thread(lines.readline))

        return peer, read

    @staticmethod
    def invoke(sequence, method):
        return {"op": "invoke", "id": f"rust:{sequence}", "path": [], "target": "test", "method": method, "args": {"type": "data", "value": []}}

    async def test_an_admitted_callback_survives_a_following_counted_release(self):
        # guarantee: bridge/reference-release-count; risk P4.
        calls = []

        def callback():
            calls.append(None)
            return len(calls)

        peer, read = self.session(lambda target, method, args: [callback, callback])
        peer._receive(self.invoke(1, "get"))
        a, b = (await read())["value"]["value"]
        self.assertEqual(a["value"]["id"], b["value"]["id"], "one export, granted twice")
        reference = a["value"]["id"]
        # The call is admitted, then both grants are released before it runs.
        peer._receive({"op": "call", "id": "rust:2", "path": [], "reference": reference, "args": {"type": "data", "value": []}})
        peer._receive({"op": "release", "reference": reference, "count": 2})
        self.assertEqual(calls, [])
        reply = await read()
        self.assertEqual(reply["op"], "return", reply)
        self.assertEqual(reply["value"], {"type": "data", "value": 1})
        self.assertEqual(len(calls), 1)
        self.assertIsNone(peer.closed_error)

    async def test_an_old_release_preserves_a_new_grant_and_identifiers_are_not_reused(self):
        # guarantee: bridge/reference-release-count; risk P4.
        def callback():
            return 42

        peer, read = self.session(lambda target, method, args: callback)
        peer._receive(self.invoke(1, "get"))
        reference = (await read())["value"]["value"]["id"]
        peer._receive(self.invoke(2, "get"))
        self.assertEqual((await read())["value"]["value"]["id"], reference)
        # The release of the first grant leaves the second one usable.
        peer._receive({"op": "release", "reference": reference, "count": 1})
        peer._receive({"op": "call", "id": "rust:3", "path": [], "reference": reference, "args": {"type": "data", "value": []}})
        self.assertEqual((await read())["value"], {"type": "data", "value": 42})
        # Released for good: the same function is exported again under a new id.
        peer._receive({"op": "release", "reference": reference, "count": 1})
        peer._receive(self.invoke(4, "get"))
        self.assertNotEqual((await read())["value"]["value"]["id"], reference)
        self.assertIsNone(peer.closed_error)

    async def test_an_explicit_release_accounts_repeated_live_imports(self):
        # guarantee: bridge/reference-release-count; risk P4.
        peer, read = self.session(lambda target, method, args: None)
        reference = {"type": "reference", "value": {"id": 1, "kind": "function", "home": False, "origin": []}}
        # The reply is already in the inbox: the synchronous call finds it
        # without waiting on the loop thread.
        peer._deliver({"op": "return", "id": "node:1", "value": {"type": "list", "value": [reference, reference]}})
        a, b = peer.call("test", "get", [])
        self.assertIs(a, b)
        peer.release(a)
        self.assertEqual((await read())["op"], "invoke")
        self.assertEqual(await read(), {"op": "release", "reference": 1, "count": 2})
        with self.assertRaisesRegex(RuntimeError, "released"):
            b()

    async def test_an_unsupported_nested_value_fails_explicitly_and_rolls_back_partial_grants(self):
        # guarantee: bridge/grant-rollback; risk P4.
        def callback():
            return 42

        peer, read = self.session(lambda target, method, args: None)
        # The callback is granted before the set (which this far end did
        # not declare it takes) fails.
        with self.assertRaisesRegex(ValueError, "did not declare the capability value-set"):
            peer.call("test", "bad", [callback, {"nested": {1, 2}}])
        self.assertEqual(peer._exports, {}, "the partial grant is rolled back")
        self.assertEqual(peer._identities, {})
        # Failed encoding consumes the invocation id, but sends nothing.
        peer._deliver({"op": "return", "id": "node:2", "value": {"type": "data", "value": 7}})
        self.assertEqual(peer.call("test", "good", []), 7)
        sent = await read()
        self.assertEqual((sent["id"], sent["method"]), ("node:2", "good"))
        self.assertIsNone(peer.closed_error)


class ScriptedChannel:
    """A channel whose far end already sent `incoming` and then ended. It
    notes whether this side had sent anything when it first read; with
    `ended`, sending fails as on a channel the far end closed."""

    def __init__(self, incoming, ended=False):
        self.incoming = list(incoming)
        self.ended = ended
        self.sent = []
        self.sent_before_first_read = None
        self.lock = threading.Lock()

    def send(self, message):
        if self.ended:
            raise BrokenPipeError("the far end closed the channel")
        with self.lock:
            self.sent.append(json.loads(message))

    def recv(self):
        with self.lock:
            if self.sent_before_first_read is None:
                self.sent_before_first_read = bool(self.sent)
        return self.incoming.pop(0) if self.incoming else None

    def close(self, reason=""):
        pass


class HandshakeOrderTests(unittest.IsolatedAsyncioTestCase):
    """#239: a session greets before it reads, so a far end it refuses
    still reads its greeting; and a far end's greeting decides the
    handshake even when the channel ended before this side greeted."""

    async def ready_error(self, peer):
        with self.assertRaises(Exception) as raised:
            # The timeout only stops a hang.
            await asyncio.wait_for(peer.ready, 10)
        return raised.exception

    async def test_issue_239_a_session_greets_before_it_reads_the_far_greeting(self):
        far = json.dumps({"op": "hello", "version": 3, "endpoint": "main"}).encode()
        channel = ScriptedChannel([far])
        peer = Peer(channel, lambda target, method, args: None)
        self.addCleanup(peer.close)
        peer.start()
        error = await self.ready_error(peer)
        self.assertIs(channel.sent_before_first_read, True)
        self.assertEqual(channel.sent[0]["op"], "hello")
        # Incompatible (ValueError), not the channel end (ConnectionError).
        self.assertIsInstance(error, ValueError)
        self.assertNotIsInstance(error, ConnectionError)

    async def test_issue_239_a_greeting_followed_by_the_channel_end_is_incompatible(self):
        far = json.dumps({"op": "hello", "version": 2}).encode()
        channel = ScriptedChannel([far], ended=True)
        peer = Peer(channel, lambda target, method, args: None, endpoint={"local": "py"})
        self.addCleanup(peer.close)
        peer.start()
        error = await self.ready_error(peer)
        self.assertIsInstance(error, ValueError)
        self.assertNotIsInstance(error, ConnectionError)

    async def test_a_channel_end_before_any_greeting_is_a_connection_error(self):
        channel = ScriptedChannel([], ended=True)
        peer = Peer(channel, lambda target, method, args: None, endpoint={"local": "py"})
        self.addCleanup(peer.close)
        peer.start()
        self.assertIsInstance(await self.ready_error(peer), ConnectionError)


class VersionMismatchTests(unittest.IsolatedAsyncioTestCase):
    """Risk P8: a far end that greets with a protocol version this side
    does not speak is refused at the handshake, not after loading, and the
    refusal names both versions and which end to upgrade (standard
    Q6.2.4): this side, naming its package, when the far end is newer; the
    far end, naming its implementation when it said, when it is older. The
    wording is rutis-bridge's (`session::rpc::version_mismatch`)."""

    async def refusal(self, endpoint, version, implementation=None):
        far = {"op": "hello", "version": version}
        if endpoint is not None:
            far.update(endpoint="main", capabilities=[])
            if implementation is not None:
                far.update(implementation=implementation)
        channel = ScriptedChannel([json.dumps(far).encode()])
        peer = Peer(channel, lambda target, method, args: None, endpoint=endpoint)
        self.addCleanup(peer.close)
        peer.start()
        with self.assertRaises(ValueError) as raised:
            # The timeout only stops a hang.
            await asyncio.wait_for(peer.ready, 10)
        self.assertTrue(peer.closed.done(), "the session ends")
        return str(raised.exception)

    async def test_an_endpoint_session_refuses_a_far_end_of_version_99(self):
        # guarantee: bridge/version-refusal; risk P8.
        self.assertEqual(
            await self.refusal({"local": "py"}, 99),
            f"incompatible session: the far end speaks protocol 99, this side {ENDPOINT_PROTOCOL}: "
            "upgrade this side (PyPI rutis)",
        )

    async def test_a_compat_session_refuses_a_far_end_of_version_99(self):
        # guarantee: bridge/version-refusal; risk P8.
        self.assertEqual(
            await self.refusal(None, 99),
            f"incompatible session: the far end speaks protocol 99, this side {PROTOCOL}: "
            "upgrade this side (PyPI rutis)",
        )

    async def test_an_endpoint_session_refuses_an_older_far_end_naming_it(self):
        # guarantee: bridge/version-refusal; risk P8.
        self.assertEqual(
            await self.refusal({"local": "py"}, 1, {"name": "rutis-bridge", "version": "0.1.0"}),
            f"incompatible session: the far end speaks protocol 1, this side {ENDPOINT_PROTOCOL}: "
            "upgrade the far end (rutis-bridge 0.1.0)",
        )
        self.assertEqual(
            await self.refusal({"local": "py"}, 1),
            f"incompatible session: the far end speaks protocol 1, this side {ENDPOINT_PROTOCOL}: "
            "upgrade the far end",
        )

    async def test_a_compat_session_refuses_an_older_far_end(self):
        # guarantee: bridge/version-refusal; risk P8.
        self.assertEqual(
            await self.refusal(None, 1),
            f"incompatible session: the far end speaks protocol 1, this side {PROTOCOL}: upgrade the far end",
        )


if __name__ == "__main__":
    unittest.main()


class DepthTests(unittest.TestCase):
    """#279: values nest at most MAX_DEPTH levels; far deeper ones (past
    Python's recursion limit) still fail with the depth error."""

    def test_far_deeper_values_fail_with_the_depth_error(self):
        import dataclasses
        import functools

        from rutis.peer import MAX_DEPTH, _data, _holds_reference

        @dataclasses.dataclass
        class Node:
            inner: object

        def nest(depth, wrap):
            return functools.reduce(lambda inner, _: wrap(inner), range(depth), 1)

        def listed(inner):
            return [inner]

        self.assertEqual(_data(nest(MAX_DEPTH, listed)), nest(MAX_DEPTH, listed))
        for value in (nest(MAX_DEPTH + 1, listed), nest(5000, listed), nest(5000, Node)):
            self.assertFalse(_holds_reference(value))
            with self.assertRaisesRegex(ValueError, "nested deeper than 64 levels"):
                _data(value)


class IntegerLimitTests(background.IsolatedAsyncioTestCase):
    """#356, design V6: an integer crosses as a number only within
    ±(2^53−1), as in JavaScript and Go; a larger one crosses as a bigint,
    at any depth, so to a far end that did not declare `value-bigint` it
    fails its own call, and the session goes on. `bool` is an int and
    crosses as before."""

    SAFE = 2**53 - 1

    def test_data_within_the_limit_crosses_and_beyond_it_fails(self):
        from rutis.peer import MAX_SAFE_INTEGER, _data

        self.assertEqual(MAX_SAFE_INTEGER, self.SAFE)
        for value in (self.SAFE, -self.SAFE, 0, True, False, [self.SAFE, {"n": -self.SAFE}]):
            self.assertEqual(_data(value), value)
        for value in (2**53, -(2**53), 2**64 + 1, [1, 2**53], {"a": {"b": [2**64]}}, (2**60,)):
            with self.subTest(value=value):
                with self.assertRaisesRegex(ValueError, r"beyond ±\(2\^53-1\)"):
                    _data(value)

    def session(self, dispatch):
        ours, theirs = socket.socketpair()
        theirs.settimeout(10)
        self.addCleanup(ours.close)
        self.addCleanup(theirs.close)
        peer = Peer(ours, dispatch)
        self.addCleanup(peer._channel.close)
        self.addCleanup(peer.close)
        peer._receive({"op": "hello", "version": 2})
        lines = theirs.makefile("rb")
        self.addCleanup(lines.close)

        async def read():
            return json.loads(await asyncio.to_thread(lines.readline))

        return peer, read

    async def test_an_argument_beyond_the_limit_to_a_far_end_without_bigints_fails_that_call(self):
        def callback():
            return 1

        peer, read = self.session(lambda target, method, args: None)
        # Beside a reference too: a list holding one crosses as a list.
        for args in ([2**53], [[1, {"n": 2**64 + 1}]], [callback, {"n": -(2**53)}]):
            with self.subTest(args=args):
                with self.assertRaisesRegex(ValueError, "did not declare the capability value-bigint"):
                    peer.call("test", "big", args)
        self.assertEqual(peer._exports, {}, "the partial grant is rolled back")
        # Failed encodings consume their invocation ids, but send nothing.
        peer._deliver({"op": "return", "id": "node:4", "value": {"type": "data", "value": 7}})
        self.assertEqual(peer.call("test", "good", [self.SAFE, -self.SAFE, True]), 7)
        sent = await read()
        self.assertEqual((sent["id"], sent["method"]), ("node:4", "good"))
        self.assertEqual(sent["args"], {"type": "data", "value": [self.SAFE, -self.SAFE, True]})
        self.assertIsNone(peer.closed_error)

    async def test_a_result_beyond_the_limit_to_a_far_end_without_bigints_fails_that_call(self):
        def callback():
            return 1

        results = {"big": 2**64 + 1, "nested": {"list": [callback, 2**53]}, "safe": [self.SAFE, -self.SAFE, True]}
        peer, read = self.session(lambda target, method, args: results[method])
        for sequence, method in enumerate(("big", "nested"), 1):
            peer._receive(ReferenceCountTests.invoke(sequence, method))
            reply = await read()
            self.assertEqual(reply["op"], "throw", reply)
            self.assertRegex(reply["error"]["message"], "did not declare the capability value-bigint")
        peer._receive(ReferenceCountTests.invoke(3, "safe"))
        reply = await read()
        self.assertEqual(reply["op"], "return", reply)
        self.assertEqual(reply["value"], {"type": "data", "value": [self.SAFE, -self.SAFE, True]})
        self.assertIsNone(peer.closed_error)


class ErrorGraphTests(unittest.TestCase):
    FAR = {
        "name": "TypeError",
        "message": "far",
        "graph": {
            "root": {"type": "reference", "value": 0},
            "nodes": [
                {"type": "error", "name": "TypeError", "message": "far", "stack": "far", "cause": {"type": "reference", "value": 1}},
                {"type": "error", "name": "Error", "message": "root cause", "stack": "root cause"},
            ],
        },
    }

    def test_a_rust_service_error_has_its_code_and_data(self):
        # As rutis-bridge's `exports::failure` sends it (#440): a bigint in data.
        error = decode_error({
            "name": "CalendarError",
            "message": "no entry 7",
            "graph": {
                "root": {"type": "reference", "value": 0},
                "nodes": [
                    {"type": "error", "name": "CalendarError", "message": "no entry 7", "code": "NotFound", "data": {"type": "reference", "value": 1}},
                    {"type": "object", "values": [["NotFound", {"type": "bigint", "value": "18446744073709551615"}]]},
                ],
            },
        })
        self.assertEqual((error.name, error.message, error.code), ("CalendarError", "no entry 7", "NotFound"))
        self.assertEqual(error.data, {"NotFound": 18446744073709551615})
        self.assertEqual((decode_error(self.FAR).code, decode_error(self.FAR).data), (None, None))
        self.assertIsNone(decode_error({"name": "Error", "message": "plain"}).code)

    def test_a_remote_error_thrown_on_crosses_with_its_whole_graph(self):
        # Alone, not only inside an AggregateError: as Node's decoded Error re-encodes.
        wire = encode_error(decode_error(self.FAR))
        self.assertEqual((wire["name"], wire["message"], wire["graph"]), ("TypeError", "far", self.FAR["graph"]))
        # Inside one, its nodes follow the aggregate's and its references shift.
        wire = encode_error(AggregateError([ValueError("near"), decode_error(self.FAR)]))
        nodes = wire["graph"]["nodes"]
        self.assertEqual([node["name"] for node in nodes], ["AggregateError", "ValueError", "TypeError", "Error"])
        self.assertEqual(nodes[0]["errors"], [{"type": "reference", "value": 1}, {"type": "reference", "value": 2}])
        self.assertEqual(nodes[2]["cause"], {"type": "reference", "value": 3})
