# guarantee: bridge/large-and-deep-values
"""A frame over the channel's message limit, carried by `chunk` frames and
reassembled by the far end (#1363). A chunk is not a call: the method runs
once, after the last chunk, and never for a transfer that does not finish."""

import asyncio
import base64
import collections
import json
import os
import threading
import unittest
from unittest import mock

import background
from support import hello

from rutis import _chunks
from rutis.peer import CAPABILITIES, MAX_MESSAGE, Peer


class Capture:
    """A session channel that keeps what was sent and never feeds anything
    unless a test does."""

    def __init__(self, max_message=MAX_MESSAGE):
        self.max_message = max_message
        self.sent = []
        self.closed = None
        self._inbox = collections.deque()
        self._cv = threading.Condition()

    def send(self, message):
        self.sent.append(message)

    def recv(self):
        with self._cv:
            while not self._inbox:
                self._cv.wait()
            return self._inbox.popleft()

    def close(self, reason=""):
        if self.closed is None:
            self.closed = reason
        with self._cv:
            self._cv.notify_all()

    def feed(self, message):
        with self._cv:
            self._inbox.append(message)
            self._cv.notify()


class Link:
    """One end of an in-memory channel pair: `send` queues on the other
    end, `recv` blocks on this one."""

    def __init__(self, max_message=MAX_MESSAGE):
        self.max_message = max_message
        self.target = None
        self.messages = collections.deque()
        self.closed = False
        self._cv = threading.Condition()

    def send(self, message):
        with self.target._cv:
            if self.target.closed:
                raise ConnectionError("the channel is closed")
            self.target.messages.append(message)
            self.target._cv.notify()

    def recv(self):
        with self._cv:
            while not self.messages and not self.closed:
                self._cv.wait()
            return self.messages.popleft() if self.messages else None

    def close(self, reason=""):
        with self._cv:
            self.closed = True
            self._cv.notify_all()


def chunked(transfer, seq, last, part):
    return {"op": "chunk", "transfer": transfer, "seq": seq, "last": last, "data": base64.b64encode(part).decode("ascii")}


def parts_of(data, size):
    return [data[at:at + size] for at in range(0, len(data), size)]


def invoke(value, call="main:1"):
    return {
        "op": "invoke",
        "id": call,
        "path": [],
        "target": "test",
        "method": "echo",
        "args": {"type": "list", "value": [{"type": "data", "value": value}]},
    }


class ChunkTests(background.IsolatedAsyncioTestCase):
    def session(self, dispatch=None, max_message=MAX_MESSAGE, capabilities=("chunked",), greet=True):
        channel = Capture(max_message)
        peer = Peer(channel, dispatch or (lambda target, method, args: None), endpoint={"local": "py"})
        self.addCleanup(peer.close)
        self.addCleanup(channel.close)
        if greet:
            peer._receive(hello(capabilities))
        return peer, channel

    async def settle(self):
        await asyncio.sleep(0.01)

    def test_the_session_declares_chunked(self):
        self.assertIn("chunked", CAPABILITIES)

    async def test_a_chunked_call_is_dispatched_once_after_its_last_chunk(self):
        calls = []

        def dispatch(target, method, args):
            calls.append(args)
            return args[0]

        peer, channel = self.session(dispatch)
        value = "x" * 4096
        parts = parts_of(json.dumps(invoke(value), separators=(",", ":")).encode(), 16)
        self.assertGreater(len(parts), 1)
        for seq, part in enumerate(parts[:-1]):
            peer.receive(chunked("main:1", seq, False, part))
        await self.settle()
        self.assertEqual(calls, [], "dispatched before the last chunk")
        peer.receive(chunked("main:1", len(parts) - 1, True, parts[-1]))
        await self.settle()
        self.assertEqual(len(calls), 1, "the call ran once")
        self.assertEqual(calls[0], [value], "the call ran with the whole value")
        reply = json.loads(channel.sent[-1])
        self.assertEqual(reply["op"], "return", reply)
        self.assertEqual(reply["value"], {"type": "data", "value": value})
        self.assertIsNone(peer.closed_error)

    async def test_a_transfer_the_session_never_finishes_is_never_dispatched(self):
        calls = []
        peer, _channel = self.session(lambda target, method, args: calls.append(args))
        parts = parts_of(json.dumps(invoke("value"), separators=(",", ":")).encode(), 8)
        for seq, part in enumerate(parts[:-1]):
            peer.receive(chunked("main:1", seq, False, part))
        peer.close(ConnectionError("the far end disconnected midway"))
        await self.settle()
        self.assertEqual(calls, [], "a call was dispatched from an incomplete transfer")

    async def test_a_chunk_with_an_unknown_field_ends_the_session(self):
        peer, _channel = self.session()
        peer.receive({"op": "chunk", "transfer": "main:1", "seq": 0, "last": True, "data": "", "extra": 1})
        self.assertRegex(str(peer.closed_error), r'^invalid chunk frame: unknown field "extra"')

    async def test_a_chunk_out_of_order_ends_the_session(self):
        peer, _channel = self.session()
        peer.receive(chunked("main:1", 1, False, b"{"))
        self.assertRegex(str(peer.closed_error), r"^chunked transfer main:1: expected chunk 0, got 1")

    async def test_a_second_transfer_before_the_first_ended_ends_the_session(self):
        peer, _channel = self.session()
        peer.receive(chunked("main:1", 0, False, b'{"op"'))
        peer.receive(chunked("main:2", 0, False, b"{"))
        self.assertRegex(str(peer.closed_error), r"^chunked transfer main:1: transfer main:2 started before it ended")

    async def test_a_chunk_before_the_greeting_ends_the_session(self):
        peer, _channel = self.session(greet=False)
        peer.receive(chunked("main:1", 0, True, b"{}"))
        self.assertRegex(str(peer.closed_error), r"^request before protocol handshake")

    async def test_a_chunk_from_a_far_end_that_did_not_declare_chunked_ends_the_session(self):
        peer, _channel = self.session(capabilities=())
        peer.receive(chunked("main:1", 0, True, b"{}"))
        self.assertRegex(str(peer.closed_error), r"^chunk frame from a far end that did not declare chunked")

    async def test_a_chunk_with_invalid_base64_ends_the_session(self):
        peer, _channel = self.session()
        peer.receive({"op": "chunk", "transfer": "main:1", "seq": 0, "last": True, "data": "not base64!"})
        self.assertRegex(str(peer.closed_error), r"^chunked transfer main:1: invalid bytes \(11 characters\)")

    async def test_a_transfer_over_the_reassembly_limit_ends_the_session(self):
        peer, _channel = self.session()
        peer._max_transfer = 1024
        peer.receive(chunked("main:1", 0, False, bytes(1024)))
        peer.receive(chunked("main:1", 1, True, b"{"))
        self.assertRegex(str(peer.closed_error), r"^chunked transfer main:1: over the limit of 1024 bytes$")

    async def test_a_chunk_inside_a_chunked_transfer_ends_the_session(self):
        peer, _channel = self.session()
        nested = json.dumps(chunked("main:2", 0, True, b"{}"), separators=(",", ":")).encode()
        peer.receive(chunked("main:1", 0, True, nested))
        self.assertRegex(str(peer.closed_error), r"chunk frame inside a chunked transfer")

    async def test_a_frame_over_the_message_limit_is_sent_as_chunks_that_fit_it(self):
        peer, channel = self.session(max_message=1024)
        value = "x" * 8192
        task = asyncio.ensure_future(peer.call_async("host:svc", "take", [value]))
        await self.settle()
        frames = [json.loads(message) for message in channel.sent]
        self.assertGreater(len(frames), 1, "one message for a frame over the limit")
        for at, frame in enumerate(frames):
            self.assertEqual(frame["op"], "chunk", f"message {at} is not a chunk: {frame['op']}")
            self.assertLessEqual(len(channel.sent[at]), 1024, f"chunk {at} is over the limit")
        transfer = frames[0]["transfer"]
        for seq, frame in enumerate(frames):
            self.assertEqual(frame["transfer"], transfer)
            self.assertEqual(frame["seq"], seq)
            self.assertEqual(frame["last"], seq == len(frames) - 1)
        call = json.loads(b"".join(base64.b64decode(frame["data"]) for frame in frames).decode("utf-8"))
        self.assertEqual(call["op"], "invoke", call)
        self.assertEqual(call["args"]["value"][0], value)
        peer._receive({"op": "return", "id": call["id"], "value": {"type": "data", "value": "ok"}})
        self.assertEqual(await asyncio.wait_for(task, 10), "ok")

    async def test_a_result_over_the_message_limit_is_sent_as_chunks_too(self):
        big = "y" * 8192
        peer, channel = self.session(lambda target, method, args: big, max_message=1024)
        peer._receive(invoke(""))
        await self.settle()
        frames = [json.loads(message) for message in channel.sent]
        self.assertGreater(len(frames), 1)
        self.assertTrue(all(frame["op"] == "chunk" for frame in frames), frames)
        reply = json.loads(b"".join(base64.b64decode(frame["data"]) for frame in frames).decode("utf-8"))
        self.assertEqual(reply["op"], "return", reply)
        self.assertEqual(reply["value"], {"type": "data", "value": big})

    async def test_a_far_end_without_chunked_still_gets_the_over_limit_error(self):
        peer, channel = self.session(max_message=1024, capabilities=())
        with self.assertRaises(ValueError) as refused:
            await peer.call_async("host:svc", "take", ["x" * 8192])
        self.assertRegex(str(refused.exception), r"^svc\.take: argument 1 cannot be sent: the message would be \d+ bytes, over the limit of 1024 bytes$")
        self.assertEqual(channel.sent, [], "nothing was sent")

    async def test_a_frame_over_the_reassembly_limit_fails_only_its_call(self):
        peer, channel = self.session(max_message=1024)
        peer._max_transfer = 2048
        with self.assertRaises(ValueError) as refused:
            await peer.call_async("host:svc", "take", ["x" * 8192])
        self.assertRegex(str(refused.exception), r"over the limit of 2048 bytes$")
        self.assertEqual(channel.sent, [], "nothing was sent")
        self.assertIsNone(peer.closed_error)

    def test_max_transfer_is_read_from_the_environment(self):
        with mock.patch.dict(os.environ, {"RUTIS_MAX_TRANSFER": "1024"}):
            self.assertEqual(_chunks.max_transfer_from_environment(), 1024)
        with mock.patch.dict(os.environ, {"RUTIS_MAX_TRANSFER": "abc"}):
            with self.assertRaisesRegex(ValueError, r'^RUTIS_MAX_TRANSFER "abc": expected a positive number of bytes$'):
                _chunks.max_transfer_from_environment()
        with mock.patch.dict(os.environ, {"RUTIS_MAX_TRANSFER": "0"}):
            with self.assertRaisesRegex(ValueError, "expected a positive number of bytes"):
                _chunks.max_transfer_from_environment()
        self.assertEqual(_chunks.DEFAULT_MAX_TRANSFER, 256 * 1024 * 1024)


class RoundTripTests(background.IsolatedAsyncioTestCase):
    async def test_a_value_over_16_mib_crosses_whole_once(self):
        calls = []

        def dispatch(target, method, args):
            calls.append(args[0])
            return args[0]

        ours_theirs = Link()
        theirs_ours = Link()
        ours_theirs.target = theirs_ours
        theirs_ours.target = ours_theirs
        caller = Peer(ours_theirs, lambda target, method, args: None, endpoint={"local": "a"})
        callee = Peer(theirs_ours, dispatch, endpoint={"local": "b"})
        self.addCleanup(caller.close)
        self.addCleanup(callee.close)
        self.addCleanup(ours_theirs.close)
        self.addCleanup(theirs_ours.close)
        caller.start()
        callee.start()
        await asyncio.wait_for(asyncio.gather(caller.ready, callee.ready), 10)
        big = "x" * (17 * 1024 * 1024)
        value = await asyncio.wait_for(caller.call_async("svc", "echo", [big]), 30)
        self.assertEqual(value, big)
        self.assertEqual(len(calls), 1, "the call ran once")
        self.assertEqual(len(calls[0]), len(big))
