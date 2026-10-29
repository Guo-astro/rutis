"""Events in one process: the listener table of the rows here, and what
the runtime shares with the host's dispatch.

The rules are rutis's unified events (#464,
docs/design-events-protocol-2026-10-18.md §5): an event is a name, its
arguments and the emitter's scope (a descriptor for filters). Every
emission goes through the host, which dispatches over every listener and
calls this process's back (`events.call`).

| mode        | the emitter gets                     | listeners                         |
| ----------- | ------------------------------------ | --------------------------------- |
| `emit`      | None at once                         | each started in order; a failure is reported, never raised |
| `parallel`  | None once all finished               | all started in order; failures raise AggregateError in registration order |
| `serial`    | the first result                     | one at a time; an error stops and is raised |
| `waterfall` | the outermost return value, as is    | each gets `next` as its last argument |
| `bail`      | the first result, synchronously      | an asynchronous listener fails it (AsyncListener) |

A result is anything but None and False.
"""

from __future__ import annotations

import asyncio
import contextvars
import inspect
import threading
from typing import Any, Callable

from ._output import say
from .peer import (
    UNDEFINED,
    AggregateError,
    RemoteError,
    ReservedEvent,
)

MODES = ("emit", "parallel", "serial", "waterfall", "bail")


def reserved(name: str, *, listening: bool = False) -> str:
    """`name`, unless a plugin may not use it: `internal/*` is each
    runtime's own, and `rutis/*` is emitted by rutis only (it may be
    listened to)."""
    if not isinstance(name, str):
        raise TypeError(f"an event name is a string, not {type(name).__name__}")
    if name.startswith("internal/"):
        raise ValueError(f"event {name} is internal to the runtime")
    if not listening and name.startswith("rutis/"):
        raise ReservedEvent(f"rutis/* events are emitted by rutis only: {name}")
    return name


def no_result(value: Any) -> bool:
    """None (and the wire's undefined) and False are no result; anything
    else, 0 and "" too, is one."""
    return value is None or value is False or value is UNDEFINED


def plain(value: Any) -> Any:
    """A value as a plugin sees it: the wire's undefined is None."""
    return None if value is UNDEFINED else value


def kind_of(listener: Callable) -> str:
    """`async` for an `async def`, which a synchronous `bail` never calls;
    `any` for every other callable, known only from what it returns."""
    return "async" if inspect.iscoroutinefunction(listener) else "any"


def error_name(error: BaseException) -> str:
    return error.name if isinstance(error, (RemoteError, AggregateError)) else type(error).__name__


def error_message(error: BaseException) -> str:
    return error.message if isinstance(error, (RemoteError, AggregateError)) else str(error)


# What failed, for a report: a listener in an `emit`, the filter of an
# `emit`, an `emit` sent through the host, a registration with the host.
_SUBJECTS = {
    "listener": "listener of {name} in row {row}",
    "filter": "filter of {name} from row {row}",
    "emit": "emit of {name} from row {row}",
    "registration": "registration of listener of {name} in row {row}",
}


def failed_line(kind: str, name: str, row: str | None, error: BaseException) -> str:
    """The state line of a failure nobody receives (§5.6), such as
    `rutis: listener of <name> in row <row> failed: <ErrorName>: <message>`."""
    subject = _SUBJECTS[kind].format(name=name, row=row)
    return f"rutis: {subject} failed: {error_name(error)}: {error_message(error)}"


def print_report(kind: str, name: str, row: str | None, error: BaseException) -> None:
    """Report a failure nobody receives on stderr, in one line."""
    say(failed_line(kind, name, row, error))


def check_scope(chain: Any) -> list:
    """A scope chain from the wire: `[{id, group}, ...]`, the outermost
    first; `[]` is the root."""
    if chain is None:
        return []
    if not isinstance(chain, list) or not all(
        isinstance(item, dict) and isinstance(item.get("id"), str) and isinstance(item.get("group"), str)
        for item in chain
    ):
        raise ValueError("a scope chain is a list of {id, group}")
    return [{"id": item["id"], "group": item["group"]} for item in chain]


def start(awaitable: Any) -> asyncio.Future:
    """`awaitable` run as a task started at once (to its first suspension),
    like a JavaScript async function; a finished future when it never
    suspends."""
    loop = asyncio.get_running_loop()
    context = contextvars.copy_context()
    return asyncio.Task(_settle(awaitable), loop=loop, context=context, eager_start=True)


async def _settle(value: Any) -> Any:
    while inspect.isawaitable(value):
        value = await value
    return value


def discard(value: Any) -> None:
    """Drop an awaitable nobody will await: a coroutine is closed, a
    future's outcome is taken and ignored."""
    if inspect.iscoroutine(value):
        value.close()
    elif asyncio.isfuture(value):
        value.add_done_callback(_ignore)


def _ignore(future: asyncio.Future) -> None:
    if not future.cancelled():
        future.exception()


class Entry:
    """A listener of a row: what the bus knows of it."""

    def __init__(self, id: str, name: str, row: str, plugin: str, listener: Callable, scope: list, prepend: bool, global_: bool):
        self.id = id
        self.name = name
        self.row = row
        self.plugin = plugin
        self.listener = listener
        self.kind = kind_of(listener)
        self.scope = scope
        self.prepend = prepend
        self.global_ = global_
        self.alive = True


class Bus:
    """The listeners of this process's rows, in registration order, and the
    invocations of them still running, by row (which unload cancels and
    waits for). `report(kind, name, row, error)` gets the failures nobody
    receives (`failed_line`). The table may change from any thread (a
    plugin's own threads register and withdraw); invocations run on the
    loop only."""

    def __init__(self, report: Callable = print_report, runtime: str | None = None):
        self.entries: list[Entry] = []
        self.by_id: dict[str, Entry] = {}
        # Guards `entries`, `by_id` and the ids.
        self._lock = threading.Lock()
        self.running: dict[str, set] = {}
        self._report = report
        self.runtime = runtime
        # `within(row, call)`: calls a listener of `row` as that row's code
        # (the runtime traces which row an unhandled error comes from, #688).
        # `awaited=False`: a synchronous dispatch, which refuses an awaitable.
        self.within: Callable[..., Any] = lambda _row, call, awaited=True: call()
        self._ids = 0
        # The id each row's configuration gives it, by row key (`rows.load`'s
        # `id`): descriptors and reported lines call the row so (§4.2).
        self.row_ids: dict[str, str] = {}

    def row_id(self, row: str | None) -> str | None:
        """What descriptors and lines call row `row` (a key): its id, or the
        key when it was not named."""
        return None if row is None else self.row_ids.get(row, row)

    def report(self, kind: str, name: str, row: str | None, error: BaseException) -> None:
        self._report(kind, name, self.row_id(row), error)

    # ── The table ────────────────────────────────────────────────

    def add(
        self, name: str, row: str, plugin: str, listener: Callable, scope: list, prepend: bool = False, global_: bool = False
    ) -> Entry:
        if not callable(listener):
            raise TypeError("on needs a listener function")
        with self._lock:
            self._ids += 1
            entry = Entry(str(self._ids), name, row, plugin, listener, list(scope), bool(prepend), bool(global_))
            if entry.prepend:
                self.entries.insert(0, entry)
            else:
                self.entries.append(entry)
            self.by_id[entry.id] = entry
        return entry

    def withdraw(self, entry: Entry) -> bool:
        """Take `entry` off the table: it starts no invocation any more.
        False when it was off already."""
        with self._lock:
            if not entry.alive:
                return False
            entry.alive = False
            self.entries.remove(entry)
            del self.by_id[entry.id]
            return True

    def withdraw_row(self, row: str) -> list[str]:
        """Take the row's listeners off the table; their ids."""
        with self._lock:
            mine = [entry for entry in self.entries if entry.row == row]
            for entry in mine:
                entry.alive = False
                self.entries.remove(entry)
                del self.by_id[entry.id]
        return [entry.id for entry in mine]

    def clear(self) -> None:
        with self._lock:
            for entry in self.entries:
                entry.alive = False
            self.entries.clear()
            self.by_id.clear()

    def listener(self, id: str) -> Entry | None:
        """The listener `id`, while it is on the table."""
        with self._lock:
            return self.by_id.get(id)

    # ── Invocations ──────────────────────────────────────────────

    def track(self, row: str, task: asyncio.Future) -> None:
        """Count `task` among the row's running invocations until it ends."""
        if task.done():
            return
        running = self.running.setdefault(row, set())
        running.add(task)

        def ended(_task, running=running) -> None:
            running.discard(task)
            if not running and self.running.get(row) is running:
                del self.running[row]

        task.add_done_callback(ended)

    async def stop(self, row: str, seconds: float, say=None) -> None:
        """Cancel the row's running invocations and wait for them, at most
        `seconds`; then say so and go on (§7.1 step 4)."""
        deadline = asyncio.get_running_loop().time() + seconds
        current = asyncio.current_task()
        tasks = {task for task in self.running.get(row, ()) if task is not current}
        if not tasks:
            return
        for task in tasks:
            task.cancel()
        remaining = max(0.0, deadline - asyncio.get_running_loop().time())
        _done, pending = await asyncio.wait(tasks, timeout=remaining)
        if pending:
            line = f"rutis: listeners of row {self.row_id(row)} still running after {seconds:g}s; unloading anyway"
            (say or _say)(line)
            # Abandoned: a reload of the same key does not wait for them.
            running = self.running.get(row)
            if running is not None:
                running.difference_update(pending)
                if not running:
                    del self.running[row]

def _say(line: str) -> None:
    say(line)


class Emitter:
    """The five ways to emit, from one row, through `send(mode, name, args,
    terminal, keep)`; `filter` narrows who receives (§6)."""

    def __init__(self, send: Callable, keep: Callable | None = None):
        self._send = send
        self._keep = keep

    def emit(self, name: str, *args: Any) -> None:
        """Start every listener of `name` in registration order and return
        at once. A listener's failure never reaches here: it is reported
        where the listener runs."""
        reserved(name)
        self._send("emit", name, list(args), None, self._keep)

    def parallel(self, name: str, *args: Any):
        """Await it: start every listener, wait for all. Failures raise
        `rutis.advanced.AggregateError`, in registration order."""
        reserved(name)
        return self._send("parallel", name, list(args), None, self._keep)

    def serial(self, name: str, *args: Any):
        """Await it: run the listeners one at a time; the first result
        (anything but None and False), or None. An error stops and is
        raised."""
        reserved(name)
        return self._send("serial", name, list(args), None, self._keep)

    def waterfall(self, name: str, *args: Any, terminal: Callable | None = None):
        """Await it: the outermost listener's return value, as is. Each
        listener gets `next` as its last argument: `next()` starts the rest
        of the chain at once and returns an awaitable of its result, ending
        with `terminal(*args)` (None without one); a listener that does not
        call it vetoes the rest."""
        reserved(name)
        if terminal is not None and not callable(terminal):
            raise TypeError("terminal must be a function")
        return self._send("waterfall", name, list(args), terminal, self._keep)

    def bail(self, name: str, *args: Any) -> Any:
        """`serial`, synchronously: an asynchronous listener (an `async
        def`, or one returning an awaitable) fails it with AsyncListener."""
        reserved(name)
        return self._send("bail", name, list(args), None, self._keep)

    def filter(self, predicate: Callable[[dict], Any]) -> "Emitter":
        """The same five methods, delivering only to listeners for which
        `predicate(descriptor)` is true; `global` listeners skip it. A
        descriptor is a dict: name, prefix, plugin, row, runtime, node,
        scope, global."""
        if not callable(predicate):
            raise TypeError("filter needs a predicate function")
        outer = self._keep
        keep = predicate if outer is None else (lambda descriptor: bool(outer(descriptor)) and predicate(descriptor))
        return Emitter(self._send, keep)
