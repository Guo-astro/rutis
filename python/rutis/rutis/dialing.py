"""Dialing out to a rutis access point, and dialing again: the rules every
runtime follows (docs/design-remote-runtime-dial-2026-10-14.md §六), as
rutis-bridge's dialing link does (`link.rs`).

A failure is retryable (backoff from 0.5 s, doubling up to 30 s, reset by
a session that lasted 60 s), an authentication failure (again every 30 s:
401 / 403 at the upgrade, or the access point rejecting this id with close
code 4003 after the hello), or incompatible (stop: retrying cannot fix
it). Every session is new: the rows of one go when it ends, and the next
connection is mounted afresh. Nothing here needs the websockets package.
"""

from __future__ import annotations

import asyncio
import os
import random
import sys
from dataclasses import dataclass

RETRYABLE, AUTH_REJECTED, INCOMPATIBLE = "retryable", "auth-rejected", "incompatible"
# The close code of an access point rejecting the id this side greeted with.
REJECTED = 4003


class DialError(Exception):
    """A dial that failed: `failure` says what it means for retrying."""

    def __init__(self, failure: str, reason: str):
        super().__init__(reason)
        self.failure = failure
        self.reason = reason


@dataclass
class Retry:
    """Reconnection, in seconds: rutis-bridge's `Retry` defaults."""

    # The first wait after a retryable failure; doubled each time.
    initial: float = 0.5
    max: float = 30.0
    # ± this fraction of each wait, at random.
    jitter: float = 0.2
    # A session lasting this long resets the backoff.
    stable: float = 60.0
    # The wait after an authentication failure.
    rejected: float = 30.0
    # How long the access point may take to greet.
    handshake: float = 10.0


def retry_from(environ=os.environ) -> Retry:
    """The defaults, unless tests shorten them with
    RUTIS_RETRY=<initial ms>,<max ms>,<stable ms>,<rejected ms>."""
    configured = environ.get("RUTIS_RETRY")
    if not configured:
        return Retry()
    initial, longest, stable, rejected = (int(part) / 1000 for part in configured.split(","))
    return Retry(initial=initial, max=longest, stable=stable, rejected=rejected)


class Backoff:
    """Exponential backoff with jitter."""

    def __init__(self, retry: Retry, rng: random.Random | None = None):
        self._retry = retry
        self._next = retry.initial
        self._rng = rng or random.Random()

    def reset(self) -> None:
        self._next = self._retry.initial

    def next(self) -> float:
        """The next wait: the current step ± jitter; the step doubles up to
        the maximum."""
        base = self._next
        self._next = min(self._next * 2, self._retry.max)
        return base * (1 + self._retry.jitter * (2 * self._rng.random() - 1))


def session_failure(error: BaseException | None, far_close: int | None) -> str:
    """What a session's end means for retrying: the access point rejecting
    this id (4003), or the far end greeting as another endpoint than
    expected, is an authentication failure; a greeting this side cannot
    speak is incompatible (the session's own handshake errors); anything
    else, being replaced (4002) too, is retryable."""
    if far_close == REJECTED:
        return AUTH_REJECTED
    reason = str(error or "")
    if isinstance(error, ValueError) and reason.startswith("incompatible session"):
        return INCOMPATIBLE
    if isinstance(error, ValueError) and reason.startswith("endpoint mismatch"):
        return AUTH_REJECTED
    return RETRYABLE


def seconds(wait: float) -> str:
    """`wait` for a state line: 0.5s, 1s, 30s."""
    return f"{round(wait, 1):g}s"


def report(line: str) -> None:
    print(f"rutis: {line}", file=sys.stderr, flush=True)


async def keep_dialing(address: str, dial, start, stopping: asyncio.Event, retry: Retry | None = None, say=report) -> int:
    """Dial `address`, serve the session, and dial again by the rules,
    until `stopping` is set (0) or the access point is incompatible (1).

    `dial()` blocks (it runs in a thread) and returns a channel or raises
    DialError. `start(channel)` returns a started session: its `peer`
    (`ready`, `closed`, `closed_error`), `end()` once the session ended
    (its rows go), and `stop()` (the rows go, then the connection closes).
    `say` gets one line per state change."""
    retry = retry or retry_from()
    backoff = Backoff(retry)
    loop = asyncio.get_running_loop()
    stopped = asyncio.ensure_future(stopping.wait())
    try:
        while True:
            attempt = asyncio.ensure_future(asyncio.to_thread(dial))
            await asyncio.wait({attempt, stopped}, return_when=asyncio.FIRST_COMPLETED)
            if stopping.is_set():
                attempt.add_done_callback(_close_late)
                return 0
            try:
                channel = attempt.result()
            except DialError as error:
                failure, reason, lost = error.failure, error.reason, "cannot connect"
            else:
                session = start(channel)
                peer = session.peer
                done, _ = await asyncio.wait(
                    {peer.ready, peer.closed, stopped}, timeout=retry.handshake, return_when=asyncio.FIRST_COMPLETED
                )
                if stopping.is_set():
                    await session.stop()
                    return 0
                if not done:
                    peer.close(ConnectionError("the access point did not greet in time"))
                    channel.close("no greeting")
                elif peer.ready.done() and peer.ready.exception() is None:
                    say(f"connected to {address}")
                    since = loop.time()
                    await asyncio.wait({peer.closed, stopped}, return_when=asyncio.FIRST_COMPLETED)
                    if stopping.is_set():
                        await session.stop()
                        return 0
                    if loop.time() - since >= retry.stable:
                        backoff.reset()
                failure = session_failure(peer.closed_error, getattr(channel, "far_close", None))
                reason, lost = str(peer.closed_error), "connection lost"
                await session.end()
            if failure == INCOMPATIBLE:
                say(f"refused: incompatible ({reason}); stopping")
                return 1
            if failure == AUTH_REJECTED:
                wait = retry.rejected
                say(f"refused: auth-rejected ({reason}); retrying in {seconds(wait)}")
            else:
                wait = backoff.next()
                say(f"{lost} ({reason}); retrying in {seconds(wait)}")
            await asyncio.wait({stopped}, timeout=wait)
            if stopping.is_set():
                return 0
    finally:
        stopped.cancel()


def _close_late(attempt: asyncio.Future) -> None:
    """A dial that connected after the runtime stopped: close it."""
    if not attempt.cancelled() and attempt.exception() is None:
        attempt.result().close("the runtime stopped")
