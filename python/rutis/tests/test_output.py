"""What the plugins of the Python runtime write, and a plugin holding it
(#678).

guarantee: host/output-prefix — each line a row's code writes starts
with the row's name (`[weather] `), also after an await; a thread a plugin
started writes with the runtime's name.
guarantee: host/blocking-hint — a row's synchronous method holding the
runtime is said once on stderr (row, method, `async def`), and not under a
debugger (RUTIS_DEBUG=1).
"""

import asyncio
import contextlib
import io
import os
import sys
import threading
import time
import types
import unittest
from unittest import mock

import background

from rutis._output import BlockingWatch, Prefixed, blocking_text, prefix_lines
from rutis.runner import Runtime


class Talk:
    def say(self, text):
        print(text)
        return None

    async def later(self, text):
        await asyncio.sleep(0.01)
        print(f"later {text}")
        return None

    def threaded(self, text):
        thread = threading.Thread(target=lambda: print(f"thread {text}", flush=True))
        thread.start()
        thread.join()
        return None

    def block(self, seconds):
        time.sleep(seconds)
        return None

    async def block_later(self, seconds):
        await asyncio.sleep(0)
        time.sleep(seconds)
        return None

    async def wait(self, seconds):
        await asyncio.sleep(seconds)
        return None


def apply(ctx, config):
    print(f"applied {config['label']}")
    ctx.provide(f"talk-{config['label']}", Talk())


METHODS = {
    "say": "sync",
    "later": "async",
    "threaded": "sync",
    "block": "sync",
    "block_later": "async",
    "wait": "async",
}


def module() -> str:
    plugin = types.ModuleType("output_rows")
    plugin.apply = apply
    sys.modules["output_rows"] = plugin
    return "output_rows"


async def call(runtime: Runtime, target: str, method: str, *args):
    value = runtime.dispatch(target, method, list(args))
    if asyncio.iscoroutine(value):
        # In a task of its own, as the session runs each call.
        value = await asyncio.ensure_future(value)
    return value


class Prefixes(background.TestCase):
    def test_prefix_lines_starts_each_line_across_writes(self):
        self.assertEqual(prefix_lines("a\nb", "[w] ", True), ("[w] a\n[w] b", False))
        self.assertEqual(prefix_lines("c\n", "[w] ", False), ("c\n", True))
        self.assertEqual(prefix_lines("", "[w] ", True), ("", True))
        self.assertEqual(prefix_lines("d\n\ne\n", "[w] ", True), ("[w] d\n[w] \n[w] e\n", True))

    def test_each_line_a_row_writes_starts_with_its_name(self):
        runtime = Runtime()
        runtime._control("mount", {"announce": True})
        written = io.StringIO()
        prefixed = Prefixed(written, runtime.output_name)

        async def run():
            with contextlib.redirect_stdout(prefixed):
                await runtime.load("a", module(), {"label": "a"}, {"talk-a": METHODS}, id="weather")
                await runtime.load("b", module(), {"label": "b"}, {"talk-b": METHODS}, id="chat")
                await call(runtime, "talk-a", "say", "hello")
                await call(runtime, "talk-b", "later", "tick")
                await call(runtime, "talk-a", "threaded", "t")
                print("not a row's")
                await runtime.unload("a")
                await runtime.unload("b")

        asyncio.run(run())
        lines = written.getvalue().splitlines()
        for line in [
            "[weather] applied a",
            "[chat] applied b",
            "[weather] hello",
            "[chat] later tick",
            "[py] thread t",
            "[py] not a row's",
        ]:
            self.assertIn(line, lines)


class Blocking(background.TestCase):
    def run_blocking(self, watch: BlockingWatch, seconds: list, method: str = "block") -> str:
        runtime = Runtime()
        runtime._control("mount", {"announce": True})
        runtime.blocking = watch
        written = io.StringIO()

        async def run():
            with contextlib.redirect_stdout(io.StringIO()):
                await runtime.load("a", module(), {"label": "a"}, {"talk-a": METHODS}, id="weather")
            with contextlib.redirect_stderr(written):
                for wait in seconds:
                    await call(runtime, "talk-a", method, wait)
                # The watch's line, written by its thread, is in.
                await asyncio.sleep(0.05)
            await runtime.unload("a")

        asyncio.run(run())
        return written.getvalue()

    def test_a_method_holding_the_runtime_is_said_once(self):
        watch = BlockingWatch(lambda: "py", limit=0.2, quiet=False)
        said = self.run_blocking(watch, [0.02, 0.6])
        hints = [line for line in said.splitlines() if "held the runtime" in line]
        self.assertEqual(hints, [blocking_text("py", "weather", "talk-a.block", 0.2)])
        self.assertIn("make it async def", hints[0])

    def test_a_blocking_call_in_an_async_method_is_said(self):
        watch = BlockingWatch(lambda: "py", limit=0.2, quiet=False)
        said = self.run_blocking(watch, [0.6], "block_later")
        hints = [line for line in said.splitlines() if "held the runtime" in line]
        self.assertEqual(hints, [blocking_text("py", "weather", "talk-a.block_later", 0.2)])

    def test_an_async_method_awaiting_long_is_not_said(self):
        watch = BlockingWatch(lambda: "py", limit=0.1, quiet=False)
        said = self.run_blocking(watch, [0.4], "wait")
        self.assertNotIn("held the runtime", said)

    def test_under_a_debugger_nothing_is_said(self):
        with mock.patch.dict(os.environ, {"RUTIS_DEBUG": "1"}):
            watch = BlockingWatch(lambda: "py", limit=0.1)
        said = self.run_blocking(watch, [0.4])
        self.assertNotIn("held the runtime", said)

    def test_the_limit_is_five_seconds(self):
        with mock.patch.dict(os.environ, {"RUTIS_TEST_BLOCKING_MS": ""}):
            self.assertEqual(BlockingWatch(lambda: "py").limit, 5.0)


if __name__ == "__main__":
    unittest.main()
