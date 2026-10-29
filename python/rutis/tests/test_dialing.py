# guarantee: bridge/dial-out-rules
"""Dialing out (dial-out design §六): how failures are classified, the
backoff, and the loop that dials again, against stand-ins for the dial and
the session. The real WebSocket dial is in test_websocket.py; the rules
against a rutis-host/listen row are crates/rutis-host/tests/dial_conformance.rs."""

import asyncio
import random
import unittest

import background
from support import within

from rutis.dialing import (
    AUTH_REJECTED,
    INCOMPATIBLE,
    RETRYABLE,
    Backoff,
    DialError,
    Retry,
    keep_dialing,
    retry_from,
    seconds,
    session_failure,
)

# Short enough that the loop runs at once; the shape is what is checked.
FAST = Retry(initial=0.001, max=0.004, stable=60, rejected=0.002, handshake=1)


class BackoffTest(unittest.TestCase):
    def test_it_doubles_to_its_cap_within_jitter_and_resets(self):
        backoff = Backoff(Retry(), random.Random(7))
        steps = [backoff.next() for _ in range(10)]
        for step, base in zip(steps, [0.5, 1, 2, 4, 8, 16, 30, 30, 30, 30]):
            self.assertTrue(base * 0.8 <= step <= base * 1.2, (step, base))
        backoff.reset()
        self.assertLessEqual(backoff.next(), 0.6)

    def test_the_defaults_are_rutis_bridges(self):
        retry = retry_from({})
        self.assertEqual((retry.initial, retry.max, retry.stable, retry.rejected), (0.5, 30, 60, 30))

    def test_tests_shorten_them(self):
        retry = retry_from({"RUTIS_RETRY": "100,400,1000,300"})
        self.assertEqual((retry.initial, retry.max, retry.stable, retry.rejected), (0.1, 0.4, 1, 0.3))

    def test_waits_read_short(self):
        self.assertEqual([seconds(w) for w in (0.5, 1.04, 30.0, 0.123)], ["0.5s", "1s", "30s", "0.1s"])


class SessionFailureTest(unittest.TestCase):
    def test_a_rejected_id_is_an_authentication_failure(self):
        self.assertEqual(session_failure(ConnectionError("rejected: not known"), 4003), AUTH_REJECTED)

    def test_an_unexpected_controller_is_an_authentication_failure(self):
        error = ValueError("endpoint mismatch: the far end greeted as other, but main is the expected endpoint")
        self.assertEqual(session_failure(error, None), AUTH_REJECTED)

    def test_a_greeting_this_side_cannot_speak_is_incompatible(self):
        self.assertEqual(session_failure(ValueError("incompatible session: version 9"), None), INCOMPATIBLE)

    def test_anything_else_is_retryable_being_replaced_too(self):
        self.assertEqual(session_failure(ConnectionError("replaced by a new connection"), 4002), RETRYABLE)
        self.assertEqual(session_failure(ConnectionError("the host disconnected"), 1001), RETRYABLE)
        self.assertEqual(session_failure(ConnectionError("connection lost"), None), RETRYABLE)
        # Only the session's own handshake errors are its words to trust.
        self.assertEqual(session_failure(ConnectionError("incompatible session"), None), RETRYABLE)


class FakePeer:
    def __init__(self):
        loop = asyncio.get_running_loop()
        self.ready, self.closed = loop.create_future(), loop.create_future()
        self.closed_error = None

    def greet(self):
        self.ready.set_result(None)

    def close(self, error):
        if self.closed_error is None:
            self.closed_error = error
            if not self.ready.done():
                self.ready.set_exception(error)
                self.ready.exception()
            self.closed.set_result(None)


class FakeChannel:
    def __init__(self, far_close=None):
        self.far_close = far_close
        self.closed = None

    def close(self, reason=""):
        self.closed = reason


class FakeSession:
    def __init__(self, channel, log):
        self.channel, self.log = channel, log
        self.peer = FakePeer()

    async def end(self):
        self.log.append("end")

    async def stop(self):
        self.log.append("stop")


class KeepDialingTest(background.IsolatedAsyncioTestCase):
    """`script` is what each dial does: a DialError to raise, or a function
    of the session that plays it (greets, closes)."""

    async def dial(self, script, stop_on=None):
        """Run the loop through `script`; it stops once out of script, or
        once it says a line starting with `stop_on`."""
        lines, log = [], []
        stopping = asyncio.Event()
        loop = asyncio.get_running_loop()
        steps = iter(script)

        def say(line):
            lines.append(line)
            if stop_on is not None and line.startswith(stop_on):
                stopping.set()

        def dial():
            step = next(steps, None)
            if step is None:
                # Out of script: the test is over.
                loop.call_soon_threadsafe(stopping.set)
                raise DialError(RETRYABLE, "the end of the script")
            if isinstance(step, DialError):
                raise step
            return step

        def start(channel):
            session = FakeSession(channel, log)
            loop.call_soon(channel.play, session)
            return session

        code = await within("the loop", keep_dialing("ws://main", dial, start, stopping, FAST, say))
        return code, lines, log

    def playing(self, play, far_close=None):
        channel = FakeChannel(far_close)
        channel.play = play
        return channel

    async def test_failures_are_waited_out_by_kind_until_incompatible(self):
        code, lines, _ = await self.dial(
            [
                DialError(RETRYABLE, "refused"),
                DialError(RETRYABLE, "refused"),
                DialError(AUTH_REJECTED, "403"),
                DialError(RETRYABLE, "refused"),
                DialError(INCOMPATIBLE, "400"),
            ]
        )
        self.assertEqual(code, 1)
        self.assertEqual(
            lines,
            [
                "cannot connect (refused); retrying in 0s",
                "cannot connect (refused); retrying in 0s",
                "refused: auth-rejected (403); retrying in 0s",
                "cannot connect (refused); retrying in 0s",
                "refused: incompatible (400); stopping",
            ],
        )

    async def test_each_session_ends_before_the_next_dial_and_says_why(self):
        def lost(session):
            session.peer.greet()
            session.peer.close(ConnectionError("replaced by a new connection"))

        def rejected(session):
            session.peer.greet()
            session.peer.close(ConnectionError("rejected: not known"))

        code, lines, log = await self.dial([self.playing(lost, 4002), self.playing(rejected, 4003)])
        self.assertEqual(code, 0)
        self.assertEqual(log, ["end", "end"])
        self.assertEqual(lines[0], "connected to ws://main")
        self.assertTrue(lines[1].startswith("connection lost (replaced by a new connection); retrying in"), lines)
        self.assertEqual(lines[2], "connected to ws://main")
        self.assertTrue(lines[3].startswith("refused: auth-rejected (rejected: not known); retrying in"), lines)

    async def test_a_greeting_this_side_cannot_speak_stops_it(self):
        def incompatible(session):
            session.peer.close(ValueError("incompatible session: version 9"))

        code, lines, log = await self.dial([self.playing(incompatible)])
        self.assertEqual(code, 1)
        self.assertEqual(log, ["end"])
        self.assertEqual(lines, ["refused: incompatible (incompatible session: version 9); stopping"])

    async def test_an_access_point_that_does_not_greet_is_given_up_on(self):
        retry = Retry(initial=0.001, max=0.004, handshake=0.01)
        lines, stopping = [], asyncio.Event()
        loop = asyncio.get_running_loop()
        channels = [self.playing(lambda session: None)]

        def dial():
            if not channels:
                loop.call_soon_threadsafe(stopping.set)
                raise DialError(RETRYABLE, "done")
            return channels.pop()

        def start(channel):
            return FakeSession(channel, [])

        await within("the loop", keep_dialing("ws://main", dial, start, stopping, retry, lines.append))
        self.assertTrue(lines[0].startswith("connection lost (the access point did not greet in time)"), lines)

    async def test_stopping_during_a_session_stops_it_and_does_not_dial_again(self):
        def connected(session):
            session.peer.greet()

        code, lines, log = await self.dial(
            [self.playing(connected), DialError(RETRYABLE, "dialed again")], stop_on="connected"
        )
        self.assertEqual(code, 0)
        self.assertEqual(log, ["stop"])
        self.assertEqual(lines, ["connected to ws://main"])

    async def test_stopping_while_waiting_ends_the_wait(self):
        stopping = asyncio.Event()
        retry = Retry(initial=60, rejected=60)
        lines = []

        def dial():
            raise DialError(RETRYABLE, "refused")

        def say(line):
            lines.append(line)
            stopping.set()

        code = await within(
            "the loop", keep_dialing("ws://main", dial, lambda channel: None, stopping, retry, say)
        )
        self.assertEqual(code, 0)
        self.assertEqual(len(lines), 1)
