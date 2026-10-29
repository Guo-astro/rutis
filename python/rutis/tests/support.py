"""Wait helpers for the tests (quality standard Q7.1, #231).

A timeout here only guards against a hang (HANG_GUARD); it never stands in
for an event. Wait for what the test is about: a reply, a line, a state.
Poll (`eventually`) only where nothing can be awaited. See
docs/design-test-clocks-2026-10-11.md §3.
"""

import asyncio
import queue
import threading
import time

# Seconds a wait lasts before the test fails as hung. Never what a test
# measures.
HANG_GUARD = 10

# How often `eventually` looks again.
_POLL = 0.01


class Hung(AssertionError):
    """A wait outlasted HANG_GUARD."""

    def __init__(self, what: str):
        super().__init__(f"timed out ({HANG_GUARD} s): {what}")


async def within(what: str, awaitable):
    """`awaitable`'s result, unless it takes longer than HANG_GUARD: then
    the test fails, naming `what`."""
    try:
        return await asyncio.wait_for(awaitable, HANG_GUARD)
    except asyncio.TimeoutError:
        raise Hung(what) from None


async def eventually(what: str, check):
    """The first truthy result of `check()`, looked at every 10 ms, within
    HANG_GUARD; else the test fails, naming `what`."""
    deadline = time.monotonic() + HANG_GUARD
    while True:
        value = check()
        if value:
            return value
        if time.monotonic() >= deadline:
            raise Hung(what)
        await asyncio.sleep(_POLL)


def eventually_blocking(what: str, check):
    """`eventually` on a thread without an event loop."""
    deadline = time.monotonic() + HANG_GUARD
    while True:
        value = check()
        if value:
            return value
        if time.monotonic() >= deadline:
            raise Hung(what)
        time.sleep(_POLL)


def blocking_within(what: str, call):
    """`call()` run on its own (daemon) thread: its result, or what it
    raised, unless it takes longer than HANG_GUARD; then the test fails,
    naming `what`. For a blocking call that takes no timeout of its own."""
    done: queue.Queue = queue.Queue()

    def run():
        try:
            done.put((True, call()))
        except BaseException as error:  # noqa: BLE001 - raised in the caller
            done.put((False, error))

    threading.Thread(target=run, name=f"within: {what}", daemon=True).start()
    try:
        ok, value = done.get(timeout=HANG_GUARD)
    except queue.Empty:
        raise Hung(what) from None
    if ok:
        return value
    raise value


# A test session's two ends: this runtime (`py`), and the host, which greets
# as `main`.
LOCAL = {"local": "py"}


def hello(capabilities=()):
    """The host's greeting, declaring `capabilities`."""
    from rutis.peer import ENDPOINT_PROTOCOL

    return {"op": "hello", "version": ENDPOINT_PROTOCOL, "endpoint": "main", "capabilities": list(capabilities)}
