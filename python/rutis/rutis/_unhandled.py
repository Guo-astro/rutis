"""Errors nobody handled (#688, docs/design-diagnostics-2026-10-26.en.md §4.7).

The plugins of a runtime share its process, so what an exception nobody
handled does is the runtime's to say. Each row's code runs with the row in
a context variable: its `apply`, its service methods, its listeners. A task
it starts copies the context, so an exception the task leaves unhandled
names the row, and the runtime fails that row; one it cannot trace is
reported as from an unknown row. The process goes on either way. A plugin
calling `os._exit` still ends it, and the line before says which row
called it.

The row of a task is read from its context (`Task.get_context()`).
"""

from __future__ import annotations

import asyncio
import contextvars
import inspect
import os
import traceback
from typing import Any, Callable

from ._output import say

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

    report = traceback.TracebackException.from_exception(error)
    if not any(_plugin_frame(frame) for frame in report.stack):
        return "".join(report.format()).rstrip()
    # Each exception of the chain (its cause, its context) keeps only the
    # plugin's frames, or all of its own when none is the plugin's.
    seen: set[int] = set()
    pending = [report]
    while pending:
        part = pending.pop()
        if id(part) in seen:
            continue
        seen.add(id(part))
        own = [frame for frame in part.stack if _plugin_frame(frame)]
        if own:
            part.stack = traceback.StackSummary.from_list(own)
        pending.extend(link for link in (part.__cause__, part.__context__) if link is not None)
    return "".join(report.format()).rstrip()


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
                say(f"runtime {runtime()}: row {row} called os._exit({code})")
        except BaseException:  # noqa: BLE001 - the process exits whatever happens here
            pass
        exit(code)

    os._exit = _exit
