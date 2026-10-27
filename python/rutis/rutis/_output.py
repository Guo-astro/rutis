"""What a runtime's plugins write, and a plugin holding the runtime (#678,
docs/design-diagnostics-2026-10-26.en.md §6.4, §7).

- Prefixes: with RUTIS_PREFIX_OUTPUT=1 (`rutis-host dev`, or
  `--prefix-output`), each line the plugins write to `sys.stdout` or
  `sys.stderr` starts with `[<row>] `, the row whose code is running (its
  context variable, as for errors nobody handled); what no row's code
  writes (a thread a plugin started, which has a context of its own) with
  `[<runtime>] `. What is written to the file descriptor itself (a child
  process, `os.write`) is not seen here: the host passes it on as it is.
- Blocking: a row's service method that keeps the event loop for 5 s in
  one go (a `def` method, or a blocking call between two `await`s of an
  `async def` one) is said once, on stderr, by a thread of its own: which
  row's which method, that the other rows wait meanwhile, and what to do.
  Only said, never stopped; not under a debugger (RUTIS_DEBUG=1), which
  stops the runtime for as long as the person likes.
"""

from __future__ import annotations

import inspect
import os
import sys
import threading
from typing import Any, Callable


def say(text: str) -> None:
    """Write `text` and a newline to stderr as the runtime's own line,
    without a prefix."""
    stream = sys.stderr
    stream = getattr(stream, "_rutis_raw", stream)
    # One write: the line is never split by another thread's.
    stream.write(text + "\n")
    stream.flush()


def prefix_lines(text: str, prefix: str, at_start: bool) -> tuple[str, bool]:
    """`text` with `prefix` at the start of each line (the first when
    `at_start`), and whether the next write starts a line."""
    if not text:
        return text, at_start
    lines = text.split("\n")
    out = (prefix if at_start else "") + lines[0]
    for index, line in enumerate(lines[1:], 1):
        # Not after the last newline: the next write starts that line.
        out += "\n" + (line if index == len(lines) - 1 and not line else prefix + line)
    return out, text.endswith("\n")


class Prefixed:
    """A text stream (`stream`) each of whose lines starts with
    `[<name()>] `; anything else is the stream's."""

    def __init__(self, stream: Any, name: Callable[[], str]) -> None:
        self._rutis_raw = stream
        self.name = name
        self._at_start = True
        self._lock = threading.Lock()

    def write(self, text: str) -> int:
        try:
            prefix = f"[{self.name()}] "
        except Exception:  # noqa: BLE001 - the text as written then
            prefix = ""
        with self._lock:
            out, self._at_start = prefix_lines(text, prefix, self._at_start)
            self._rutis_raw.write(out)
        return len(text)

    def writelines(self, lines: Any) -> None:
        for line in lines:
            self.write(line)

    def __getattr__(self, name: str) -> Any:
        return getattr(self._rutis_raw, name)


def install_prefix(name: Callable[[], str]) -> None:
    """Start each line written to `sys.stdout` and `sys.stderr` with
    `[<name()>] `; installed once, a later call names anew."""
    for attribute in ("stdout", "stderr"):
        stream = getattr(sys, attribute)
        if isinstance(stream, Prefixed):
            stream.name = name
        elif stream is not None:
            setattr(sys, attribute, Prefixed(stream, name))


def blocking_limit() -> float:
    """How long a method may hold the runtime before it is said, in
    seconds: 5, shorter only in this package's tests."""
    try:
        test = float(os.environ.get("RUTIS_TEST_BLOCKING_MS", ""))
    except ValueError:
        test = 0
    return test / 1000 if test > 0 else 5.0


def blocking_text(runtime: str, row: str, method: str, seconds: float) -> str:
    """What is said of row `row`'s `method` holding runtime `runtime` for
    `seconds`."""
    shown = f"{seconds:g}"
    return (
        f"runtime {runtime}: {method} of row {row} has held the runtime for {shown} s, "
        "and the other rows wait meanwhile: it blocks the event loop (a def method, or a blocking call "
        "in an async def one); make it async def and await what it waits for "
        "(asyncio.sleep, an async client, asyncio.to_thread for a blocking call), "
        "and it will not hold the runtime"
    )


class BlockingWatch:
    """`held(row, method, call)` runs `call()` as a call of row `row`'s
    `method` (`forecast.tomorrow`); one that runs for `limit` seconds is
    said once, on stderr, by a thread (started with the first call).
    `runtime()`: the runtime's name. Off (`call()` alone) when `quiet`."""

    def __init__(self, runtime: Callable[[], str], limit: float | None = None, quiet: bool | None = None) -> None:
        self.runtime = runtime
        self.limit = blocking_limit() if limit is None else limit
        self.quiet = os.environ.get("RUTIS_DEBUG") == "1" if quiet is None else quiet
        self._changed = threading.Condition()
        # The outermost call running: (row, method, its number), or None.
        self._held: tuple[str, str, int] | None = None
        self._calls = 0
        self._depth = 0
        self._thread: threading.Thread | None = None

    def held(self, row: str, method: str, call: Callable[[], Any]) -> Any:
        if self.quiet:
            return call()
        with self._changed:
            # The outermost call: one it waits for is part of it.
            self._depth += 1
            if self._depth == 1:
                self._calls += 1
                self._held = (row, method, self._calls)
                self._changed.notify_all()
            if self._thread is None:
                self._thread = threading.Thread(target=self._watch, name="rutis-blocking", daemon=True)
                self._thread.start()
        try:
            return call()
        finally:
            with self._changed:
                self._depth -= 1
                if self._depth == 0:
                    self._held = None
                    self._changed.notify_all()

    def watched(self, row: str, method: str, value: Any) -> Any:
        """`value`, a coroutine a call of row `row`'s `method` returned, with
        each of its steps (from its start to an `await` that suspends, and
        between two such) run as [`held`]; anything else as it is."""
        if self.quiet or not inspect.iscoroutine(value):
            return value
        return _watched(_Steps(value, lambda step: self.held(row, method, step)))

    def _watch(self) -> None:
        with self._changed:
            while True:
                self._changed.wait_for(lambda: self._held is not None)
                row, method, number = self._held  # type: ignore[misc]

                def ended() -> bool:
                    return self._held is None or self._held[2] != number

                if not self._changed.wait_for(ended, timeout=self.limit):
                    try:
                        say(blocking_text(self.runtime(), row, method, self.limit))
                    except Exception:  # noqa: BLE001 - nowhere to say it
                        pass
                    self._changed.wait_for(ended)


async def _watched(steps: "_Steps") -> Any:
    return await steps


class _Steps:
    """Awaiting it drives `coroutine` a step at a time, each step through
    `held(step)`, passing what it waits for, and what it is sent or thrown,
    through."""

    def __init__(self, coroutine: Any, held: Callable[[Callable[[], Any]], Any]) -> None:
        self.coroutine = coroutine
        self.held = held

    def __await__(self):  # noqa: ANN204 - a generator
        coroutine = self.coroutine
        sent: Any = None
        thrown: BaseException | None = None
        try:
            while True:
                try:
                    if thrown is None:
                        waits = self.held(lambda: coroutine.send(sent))
                    else:
                        error, thrown = thrown, None
                        waits = self.held(lambda: coroutine.throw(error))
                except StopIteration as stop:
                    return stop.value
                try:
                    sent = yield waits
                except BaseException as error:  # noqa: BLE001 - passed on to the coroutine
                    if isinstance(error, GeneratorExit):
                        raise
                    sent, thrown = None, error
        finally:
            coroutine.close()
