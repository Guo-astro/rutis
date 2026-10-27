"""Errors nobody handled (#688, docs/design-diagnostics-2026-10-26.en.md §4.7).

The plugins of a runtime share its process, so what an exception nobody
handled does is the runtime's to say. Each row's code runs with the row in
a context variable: its `apply`, its service methods, its listeners. A task
it starts copies the context, so an exception the task leaves unhandled
names the row, and the runtime fails that row; one it cannot trace is
reported as from an unknown row. The process goes on either way. A plugin
calling `os._exit` still ends it, and the line before says which row
called it.

The row of a task is read from its context: `Task.get_context()` from 3.12.
On 3.10 and 3.11 the C tasks do not show their context, so the loop's task
factory records the row of each task when it is created (from the context
it copies).
"""

from __future__ import annotations

import asyncio
import contextvars
import inspect
import os
import sys
import traceback
import weakref
from typing import Any, Callable

# The key of the row whose code is running (None: no row's).
current_row: contextvars.ContextVar[str | None] = contextvars.ContextVar("rutis_row", default=None)


def as_row(key: str | None, call: Callable[[], Any], awaited: bool = True) -> Any:
    """`call()` run as row `key`'s code; a coroutine it returns runs so too,
    unless it is not `awaited` (a synchronous dispatch, which refuses it).
    """
    context = contextvars.copy_context()

    def run() -> Any:
        current_row.set(key)
        return call()

    value = context.run(run)
    if awaited and inspect.iscoroutine(value):
        return _in_row(key, value)
    return value


async def _in_row(key: str | None, coroutine: Any) -> Any:
    current_row.set(key)
    return await coroutine


# 3.10, 3.11: the row of each task, recorded by the loop's task factory.
_task_rows: weakref.WeakKeyDictionary = weakref.WeakKeyDictionary()


def record_task_rows(loop: asyncio.AbstractEventLoop) -> None:
    """On 3.10 and 3.11, record the row of every task `loop` creates (its
    tasks do not show their context); the loop's own factory, if it has
    one, still makes them."""
    if sys.version_info >= (3, 12):
        return
    previous = loop.get_task_factory()

    def factory(loop: asyncio.AbstractEventLoop, coro: Any, **options: Any) -> asyncio.Future:
        made = previous(loop, coro, **options) if previous is not None else asyncio.Task(coro, loop=loop, **options)
        copied = options.get("context")
        row = copied.get(current_row) if isinstance(copied, contextvars.Context) else current_row.get()
        if row is not None:
            try:
                _task_rows[made] = row
            except TypeError:
                pass  # a future that takes no weak reference
        return made

    loop.set_task_factory(factory)


def row_of(context: dict) -> str | None:
    """The row the failure an event loop reports (`context`, as its
    exception handler gets it) comes from: the row in the context of the
    task, future or callback that failed, if any."""
    for name in ("task", "future", "handle"):
        owner = context.get(name)
        if owner is None:
            continue
        get_context = getattr(owner, "get_context", None)
        ran = get_context() if callable(get_context) else getattr(owner, "_context", None)
        if isinstance(ran, contextvars.Context):
            return ran.get(current_row)
        try:
            row = _task_rows.get(owner)
        except TypeError:
            row = None
        if row is not None:
            return row
    return None


def error_site(error: BaseException) -> str | None:
    """Where `error` was raised: `file:line in function`."""
    frames = traceback.extract_tb(error.__traceback__)
    if not frames:
        return None
    last = frames[-1]
    return f"{last.filename}:{last.lineno} in {last.name}"


def failure_text(kind: str, error: BaseException) -> str:
    """The error as a row's failure: what kind of failure, the error, where."""
    site = error_site(error)
    return f"{kind}: {type(error).__name__}: {error}" + (f" (at {site})" if site else "")


def kind_text(kind: str, error: BaseException) -> str:
    """What failed, the error's own text: `unhandled exception: ValueError:
    boom` (where, the host prints from the error's traceback)."""
    return f"{kind}: {type(error).__name__}: {error}"


class UnhandledError(Exception):
    """A load failed because the row's code left an exception unhandled
    while it loaded: says what, with that exception's traceback, code and
    data, and the exception itself as its `__cause__`, so that its own
    cause and an AggregateError's errors cross with it as they do for an
    exception once the row has loaded."""

    # Its stack is the exception's frames; the exception, with its chain,
    # crosses as the cause (peer.encode_error).
    reports_its_cause = True


def unhandled_error(kind: str, error: BaseException) -> UnhandledError:
    """The `UnhandledError` a load fails with for `error`."""
    failure = UnhandledError(kind_text(kind, error)).with_traceback(error.__traceback__)
    for name in ("code", "data"):
        if hasattr(error, name):
            setattr(failure, name, getattr(error, name))
    failure.__cause__ = error
    return failure


def report_text(error: BaseException) -> str:
    """The error as reported for no row: its traceback with only the
    plugin's frames, or the whole traceback when none of its frames is
    the plugin's (it is then all there is to find its source by)."""
    from .runner import _plugin_frame  # runner imports this module

    every = traceback.extract_tb(error.__traceback__)
    own = [frame for frame in every if _plugin_frame(frame)]
    if not own:
        return "".join(traceback.format_exception(type(error), error, error.__traceback__)).rstrip()
    frames = "".join(traceback.StackSummary.from_list(own).format())
    text = "".join(traceback.format_exception_only(type(error), error))
    return f"Traceback (most recent call last):\n{frames}{text}".rstrip()


def install_exit(name_row: Callable[[str | None, list[str]], str | None], runtime: Callable[[], str]) -> None:
    """Say which row calls `os._exit` before the process exits.
    `name_row(key, files)`: the id of the row, from the row in the context
    (`key`), else from the source files on the stack (a thread a plugin
    started has a context of its own); None when it cannot tell."""
    exit = os._exit

    def _exit(code: int) -> None:
        try:
            files = [frame.filename for frame in traceback.extract_stack()]
            row = name_row(current_row.get(), files)
            if row is not None:
                print(f"runtime {runtime()}: row {row} called os._exit({code})", file=sys.stderr, flush=True)
        except BaseException:  # noqa: BLE001 - the process exits whatever happens here
            pass
        exit(code)

    os._exit = _exit
