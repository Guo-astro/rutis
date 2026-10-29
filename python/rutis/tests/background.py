"""Exceptions in background threads fail the tests (quality standard Q7.4).

An uncaught exception in a thread is only printed by Python, so a test
whose work went wrong off the main thread would still pass (#197). Importing
this module records every such exception; the test cases below fail the
test that was running when one arrived, and an exception after the last
test makes the run exit with an error.
"""

import atexit
import os
import sys
import threading
import traceback
import unittest

_lock = threading.Lock()
_pending: list[str] = []
_print = threading.excepthook


def _record(args) -> None:
    _print(args)
    if args.exc_type is SystemExit:
        return
    name = args.thread.name if args.thread is not None else "unknown thread"
    trace = "".join(traceback.format_exception(args.exc_type, args.exc_value, args.exc_traceback))
    with _lock:
        _pending.append(f"exception in thread {name}:\n{trace}")


def _take() -> list[str]:
    with _lock:
        taken = _pending[:]
        _pending.clear()
    return taken


threading.excepthook = _record


@atexit.register
def _unreported() -> None:
    late = _take()
    if late:
        print("background thread exceptions after the last test:", *late, sep="\n", file=sys.stderr, flush=True)
        os._exit(1)


class _FailOnThreadExceptions:
    def run(self, result=None):
        # Added before setUp, so it runs after every other cleanup.
        self.addCleanup(self._no_thread_exceptions)
        return super().run(result)

    def _no_thread_exceptions(self) -> None:
        raised = _take()
        if raised:
            self.fail("background thread raised:\n" + "\n".join(raised))


class TestCase(_FailOnThreadExceptions, unittest.TestCase):
    pass


class IsolatedAsyncioTestCase(_FailOnThreadExceptions, unittest.IsolatedAsyncioTestCase):
    pass
