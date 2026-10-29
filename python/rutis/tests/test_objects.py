"""Objects with methods across processes, Python's side (#687): a received
object is a proxy whose public names are asynchronous methods of the
original; one proxy per object, passed back it is the original there; a
method the object here lacks is `no-method`; undefined is None inside a
value; a call given up is cancelled through."""

import asyncio
import json
import socket
import unittest

import background
from support import LOCAL, hello
from rutis.peer import Peer, decode_error

OBJECT = {"type": "reference", "value": {"id": 5, "kind": "object", "home": False, "origin": []}}
data = lambda value: {"type": "data", "value": value}  # noqa: E731


class ObjectTests(background.IsolatedAsyncioTestCase):
    def session(self, dispatch):
        ours, theirs = socket.socketpair()
        theirs.settimeout(10)
        self.addCleanup(ours.close)
        self.addCleanup(theirs.close)
        peer = Peer(ours, dispatch, endpoint=LOCAL)
        self.addCleanup(peer._channel.close)
        self.addCleanup(peer.close)
        peer._receive(hello())
        lines = theirs.makefile("rb")
        self.addCleanup(lines.close)

        async def read():
            return json.loads(await asyncio.to_thread(lines.readline))

        async def read_op(op):
            while True:
                frame = await read()
                if frame["op"] == op:
                    return frame

        return peer, read, read_op

    @staticmethod
    def invoke(sequence, method, args):
        return {"op": "invoke", "id": f"main:{sequence}", "path": [], "target": "t", "method": method, "args": args}

    async def test_a_received_object_is_a_proxy_whose_methods_call_it(self):
        # objects-case(python): send-call-state
        # objects-case(python): same-proxy-home
        # objects-case(python): receiver-releases
        got = []
        peer, read, read_op = self.session(lambda target, method, args: got.extend(args) or "ok")
        peer._receive(self.invoke(1, "take", {"type": "list", "value": [OBJECT, OBJECT]}))
        await read_op("return")
        cache, again = got
        self.assertIs(cache, again, "passed twice, one proxy")
        self.assertEqual(repr(cache), "<remote object>")
        with self.assertRaises(AttributeError):
            cache._private  # noqa: B018
        result = asyncio.ensure_future(cache.get("a"))
        call = await read_op("call")
        self.assertEqual((call["reference"], call["method"], call["args"]), (5, "get", data(["a"])))
        peer._receive({"op": "return", "id": call["id"], "value": data(1)})
        self.assertEqual(await result, 1)
        # A call given up here is cancelled on the far end.
        given_up = asyncio.ensure_future(cache.slow())
        slow = await read_op("call")
        given_up.cancel()
        self.assertEqual(await read_op("cancel"), {"op": "cancel", "id": slow["id"]})
        peer._receive({"op": "throw", "id": slow["id"], "error": {"name": "CancelledError", "message": "cancelled"}})
        # Passed back: the far end's own again.
        sent = asyncio.ensure_future(peer.request("t", "give", [cache]))
        give = await read_op("invoke")
        self.assertEqual(give["args"]["value"][0]["value"], {"id": 5, "kind": "object", "home": True, "origin": []})
        peer._receive({"op": "return", "id": give["id"], "value": {"type": "undefined"}})
        await sent
        # Let go of: released.
        peer.release(cache)
        self.assertEqual(await read_op("release"), {"op": "release", "reference": 5, "count": 2})
        self.assertIsNone(peer.closed_error)

    async def test_a_method_the_object_lacks_is_no_method(self):
        # objects-case(python): no-method
        class Cache:
            def __init__(self):
                self.size = 0

            def get(self, key):
                return key

            def set(self, key, value):
                self.size += 1

        objects = {"cache": Cache()}
        peer, read, read_op = self.session(lambda target, method, args: objects[method])
        peer._receive(self.invoke(1, "cache", data([])))
        reference = (await read_op("return"))["value"]["value"]["id"]
        cases = [
            ("gett", "Cache has no method gett (methods: get, set)"),
            ("size", "Cache has no method size (size is a field; objects passed by reference expose only their methods)"),
        ]
        for n, (method, message) in enumerate(cases, start=2):
            peer._receive({"op": "call", "id": f"main:{n}", "path": [], "reference": reference, "method": method, "args": data([])})
            thrown = await read_op("throw")
            self.assertEqual(thrown["error"]["message"], message)
            error = decode_error(thrown["error"])
            self.assertEqual(error.code, "no-method")
            self.assertEqual(error.data, {"object": "Cache", "method": method, "methods": ["get", "set"]})
        self.assertIsNone(peer.closed_error)

    async def test_undefined_is_none_inside_a_value(self):
        # objects-case(python): undefined-nested-null
        got = []
        peer, read, read_op = self.session(lambda target, method, args: got.append(args) or None)
        undefined = {"type": "undefined"}
        nested = {"type": "list", "value": [data(1), undefined, data(3)]}
        peer._receive(self.invoke(1, "take", {"type": "list", "value": [nested, {"type": "record", "value": {"a": undefined}}]}))
        await read_op("return")
        self.assertEqual(got[0], [[1, None, 3], {"a": None}], "the same at the top as inside")
        # Sent: None inside, an undefined argument kept as one.
        from rutis.peer import UNDEFINED

        asyncio.ensure_future(peer.request("t", "m", [[1, UNDEFINED, 3], UNDEFINED]))
        sent = await read_op("invoke")
        self.assertEqual(sent["args"]["value"][0], {"type": "data", "value": [1, None, 3]})
        self.assertEqual(sent["args"]["value"][1], undefined)
        self.assertIsNone(peer.closed_error)

    async def test_an_object_here_is_cancelled_through_and_released(self):
        # objects-case(python): cancel
        # objects-case(python): holder-killed-releases
        stopped = asyncio.Event()

        class Job:
            async def run(self, signal=None):
                try:
                    await asyncio.sleep(60)
                except asyncio.CancelledError:
                    stopped.set()
                    raise

        peer, read, read_op = self.session(lambda target, method, args: Job())
        peer._receive(self.invoke(1, "job", data([])))
        reference = (await read_op("return"))["value"]["value"]["id"]
        peer._receive({"op": "call", "id": "main:2", "path": [], "reference": reference, "method": "run", "args": {"type": "list", "value": [{"type": "signal"}]}})
        await asyncio.sleep(0.05)
        peer._receive({"op": "cancel", "id": "main:2"})
        await asyncio.wait_for(stopped.wait(), 10)
        before = peer.references()["exports"]
        peer._receive({"op": "release", "reference": reference, "count": 1})
        self.assertEqual(peer.references()["exports"], before - 1)
        self.assertIsNone(peer.closed_error)



if __name__ == "__main__":
    unittest.main()
