"""The process event bus shared by the runtime and its test tool."""

import asyncio
import contextvars
import inspect

from . import _eager
from .peer import AggregateError


def reserved(name: str) -> str:
    if name.startswith("internal/"):
        raise ValueError(f"event {name} is internal to the runtime")
    return name


async def _settle(value):
    while inspect.isawaitable(value):
        value = await value
    return value


def _start(value):
    return _eager.start(asyncio.get_running_loop(), _settle(value), contextvars.copy_context())


class Events:
    def __init__(self, unhandled=None):
        self.listeners = {}
        self.pending = set()
        self.failures = []
        self.unhandled = unhandled

    def on(self, name, listener, disposers):
        reserved(name)
        if not callable(listener):
            raise TypeError("on needs a listener function")
        listeners = self.listeners.setdefault(name, {})
        token = object()
        listeners[token] = listener

        def off():
            if token not in listeners:
                return
            del listeners[token]
            if not listeners and self.listeners.get(name) is listeners:
                del self.listeners[name]
            if off in disposers:
                disposers.remove(off)

        disposers.append(off)
        return off

    def emit(self, name, *args):
        reserved(name)
        for listener in list(self.listeners.get(name, {}).values()):
            result = listener(*args)
            if inspect.isawaitable(result):
                task = _start(result)
                self.pending.add(task)
                task.add_done_callback(self._finished)

    def _finished(self, task):
        self.pending.discard(task)
        if not task.cancelled() and task.exception() is not None:
            if self.unhandled is None:
                self.failures.append(task.exception())
            else:
                self.unhandled(task.exception())

    async def parallel(self, name, *args):
        reserved(name)

        async def call(listener):
            await _settle(listener(*args))

        tasks = [_start(call(listener)) for listener in list(self.listeners.get(name, {}).values())]
        results = await asyncio.gather(*tasks, return_exceptions=True)
        errors = [result for result in results if isinstance(result, BaseException)]
        if errors:
            raise AggregateError(errors)

    async def drain(self):
        # The test tool reports fire-and-forget failures instead of exiting
        # its own process, and waits for these listeners before reporting.
        while self.pending:
            await asyncio.gather(*self.pending, return_exceptions=True)
        if self.failures:
            errors, self.failures = self.failures, []
            raise AggregateError(errors)

    def clear(self):
        self.listeners.clear()
