"""Eager start for Python 3.10 and 3.11, which lack
`asyncio.Task(..., eager_start=True)` (3.12+).

An `async def` method runs its first step (up to its first suspension) at
once, inside the call that started it, like a JS async function; a
coroutine that never suspends is done before `start` returns. The peer
relies on this: a finished result can be awaited inside a synchronous call
chain, an unfinished one cannot (`SyncWaitCycle`).

`start` builds the task first, makes it the current task, runs the
coroutine's first step in `context`, and leaves the rest to the task. When
the first step already finished, it returns a finished future instead; the
task then ends the same way on its first step and is dropped. Like the
native eager start, it does not go through the loop's task factory.

It sets the current task with `asyncio.tasks._swap_current_task` where
there is one (3.12+, what the native eager start uses; from 3.14
`current_task()` no longer reads the dict below), else through
`asyncio.tasks._current_tasks`, private but stable on 3.10 and 3.11, which
receive only security fixes.
"""

from __future__ import annotations

import asyncio
import collections.abc
import contextvars
from asyncio import tasks as _tasks
from typing import Any, Coroutine

_UNSTARTED = object()


def _swap_dict(loop: asyncio.AbstractEventLoop, task: asyncio.Task | None) -> asyncio.Task | None:
    previous = _tasks._current_tasks.get(loop)
    if task is None:
        _tasks._current_tasks.pop(loop, None)
    else:
        _tasks._current_tasks[loop] = task
    return previous


# Makes `task` the loop's current task (None: none), returns the previous.
_swap = getattr(_tasks, "_swap_current_task", _swap_dict)


def start(loop: asyncio.AbstractEventLoop, coro: Coroutine, context: contextvars.Context) -> asyncio.Future:
    """`asyncio.Task(coro, loop=loop, context=context, eager_start=True)`."""
    rest = _Rest(coro, context)
    task = asyncio.Task(rest, loop=loop)  # schedules its first step
    if not loop.is_running():
        # Native eager start does not run the first step either.
        return task
    previous = _swap(loop, task)
    try:
        try:
            rest.waited = context.run(coro.send, None)
            return task
        except StopIteration as stop:
            rest.outcome = (True, stop.value)
        except (KeyboardInterrupt, SystemExit):
            # It propagates from here, as from a native eager start; the
            # task just ends, cancelled.
            rest.outcome = (False, asyncio.CancelledError())
            task.cancel()
            task.add_done_callback(_retrieve)
            raise
        except BaseException as error:  # noqa: BLE001 - the task's outcome
            rest.outcome = (False, error)
    finally:
        _swap(loop, previous)
    task.add_done_callback(_retrieve)
    return _finished(loop, rest.outcome)


def _finished(loop: asyncio.AbstractEventLoop, outcome: tuple) -> asyncio.Future:
    future = loop.create_future()
    ok, value = outcome
    if ok:
        future.set_result(value)
    elif isinstance(value, asyncio.CancelledError):
        future.cancel()
    else:
        future.set_exception(value)
    return future


def _retrieve(task: asyncio.Task) -> None:
    # The finished future carries the outcome; the task's copy is not logged.
    if not task.cancelled():
        task.exception()


class _Rest(collections.abc.Coroutine):
    """What is left of `coro` after its first step, driven by the task;
    every step runs in `context`."""

    def __init__(self, coro: Coroutine, context: contextvars.Context) -> None:
        self._coro = coro
        self._context = context
        self._first = True
        self.waited: Any = _UNSTARTED
        self.outcome: tuple | None = None
        # Task repr shows the plugin's coroutine.
        self.__qualname__ = getattr(coro, "__qualname__", type(coro).__name__)
        self.__name__ = getattr(coro, "__name__", self.__qualname__)

    def send(self, value: Any) -> Any:
        if not self._first:
            return self._context.run(self._coro.send, value)
        self._first = False
        if self.outcome is not None:
            ok, result = self.outcome
            if ok:
                raise StopIteration(result)
            raise result
        waited, self.waited = self.waited, None
        if waited is _UNSTARTED or waited is None:
            # Not started (the loop was not running), or it yielded to the
            # loop (`await asyncio.sleep(0)`), which a loop iteration has done.
            return self._context.run(self._coro.send, None)
        if asyncio.isfuture(waited) and waited.done():
            # It suspended on a future that finished meanwhile: go on.
            waited._asyncio_future_blocking = False
            return self._context.run(self._coro.send, None)
        # Hand the future to the task, which wakes this up when it is done.
        return waited

    def throw(self, *error: Any) -> Any:
        if self._first and self.outcome is not None:
            return self.send(None)
        if self._first:
            self._first = False
            waited, self.waited = self.waited, None
            if asyncio.isfuture(waited) and not waited.done():
                # A native task waiting on it would cancel it.
                waited.cancel()
        return self._context.run(self._coro.throw, *error)

    def close(self) -> None:
        self._coro.close()

    def __await__(self):  # pragma: no cover - only a task drives this
        raise TypeError("_Rest is driven by its task")

    # get_stack() and print_stack() show the plugin's coroutine.
    @property
    def cr_frame(self) -> Any:
        return self._coro.cr_frame

    @property
    def cr_running(self) -> bool:
        return self._coro.cr_running

    @property
    def cr_code(self) -> Any:
        return self._coro.cr_code

    @property
    def cr_await(self) -> Any:
        return self._coro.cr_await
