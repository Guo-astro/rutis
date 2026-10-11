"""Eager start: the emulation (`rutis._eager.start`, used on 3.10 and 3.11)
and, on 3.12+, the native `eager_start=True` meet the same fixed
expectations. Each scenario states what must happen; both implementations
are checked against it, not against each other."""

import asyncio
import contextvars
import gc
import logging
import sys
import unittest

import background

from rutis import _eager

var = contextvars.ContextVar("var", default="unset")


def native(loop, coro, context):
    return asyncio.Task(coro, loop=loop, context=context, eager_start=True)


STARTS = [("emulated", _eager.start)]
if sys.version_info >= (3, 12):
    STARTS.append(("native", native))


async def turns(n=3):
    for _ in range(n):
        await asyncio.sleep(0)


class EagerStart(background.IsolatedAsyncioTestCase):
    def start(self, how, coro):
        return how(asyncio.get_running_loop(), coro, contextvars.copy_context())

    async def each(self, scenario):
        for name, how in STARTS:
            with self.subTest(start=name):
                await scenario(lambda coro: self.start(how, coro))

    async def test_a_coroutine_that_never_suspends_is_done_at_once(self):
        async def scenario(start):
            log = []

            async def method():
                log.append("first step")
                return 1

            done = start(method())
            self.assertEqual(log, ["first step"])
            self.assertTrue(done.done())
            self.assertEqual(done.result(), 1)

        await self.each(scenario)

    async def test_an_error_in_the_first_step_finishes_it_with_that_error(self):
        async def scenario(start):
            async def method():
                raise ValueError("bad")

            done = start(method())
            self.assertTrue(done.done())
            self.assertIsInstance(done.exception(), ValueError)

            async def cancels():
                raise asyncio.CancelledError()

            cancelled = start(cancels())
            self.assertTrue(cancelled.cancelled())

        await self.each(scenario)

    async def test_the_rest_runs_later_in_the_same_context(self):
        async def scenario(start):
            log = []
            gate = asyncio.get_running_loop().create_future()

            async def method():
                var.set("set in the first step")
                log.append("first step")
                await gate
                log.append(("rest", var.get()))
                return 2

            task = start(method())
            self.assertEqual(log, ["first step"])
            self.assertFalse(task.done())
            self.assertEqual(var.get(), "unset")  # the caller's context is untouched
            gate.set_result(None)
            self.assertEqual(await task, 2)
            self.assertEqual(log, ["first step", ("rest", "set in the first step")])

        await self.each(scenario)

    async def test_the_first_step_runs_as_the_returned_task(self):
        async def scenario(start):
            seen = []
            gate = asyncio.get_running_loop().create_future()
            outer = asyncio.current_task()

            async def method():
                seen.append(asyncio.current_task())
                await gate

            task = start(method())
            self.assertIs(seen[0], task)
            self.assertIs(asyncio.current_task(), outer)
            gate.set_result(None)
            await task

        await self.each(scenario)

    @unittest.skipIf(sys.version_info < (3, 11), "asyncio.timeout is new in 3.11")
    async def test_a_timeout_scope_in_the_first_step_works(self):
        async def scenario(start):
            gate = asyncio.get_running_loop().create_future()

            async def method():
                async with asyncio.timeout(10):
                    await gate
                return "in time"

            task = start(method())
            gate.set_result(None)
            self.assertEqual(await task, "in time")

        await self.each(scenario)

    async def test_cancelling_right_away_reaches_the_await(self):
        async def scenario(start):
            log = []
            gate = asyncio.get_running_loop().create_future()

            async def method():
                try:
                    await gate
                except asyncio.CancelledError:
                    log.append("cancelled at the await")
                    raise

            task = start(method())
            task.cancel()
            with self.assertRaises(asyncio.CancelledError):
                await task
            self.assertEqual(log, ["cancelled at the await"])
            self.assertTrue(gate.cancelled())

        await self.each(scenario)

    async def test_cancelling_later_reaches_the_await(self):
        async def scenario(start):
            log = []
            gate = asyncio.get_running_loop().create_future()

            async def method():
                try:
                    await gate
                except asyncio.CancelledError:
                    log.append("cancelled")
                    raise

            task = start(method())
            await turns()
            task.cancel()
            with self.assertRaises(asyncio.CancelledError):
                await task
            self.assertEqual(log, ["cancelled"])

        await self.each(scenario)

    async def test_a_method_that_swallows_the_cancellation_finishes(self):
        async def scenario(start):
            gate = asyncio.get_running_loop().create_future()

            async def method():
                try:
                    await gate
                except asyncio.CancelledError:
                    return 3

            task = start(method())
            task.cancel()
            self.assertEqual(await task, 3)

        await self.each(scenario)

    async def test_yielding_to_the_loop_resumes_before_later_callbacks(self):
        async def scenario(start):
            log = []

            async def method():
                log.append("a")
                await asyncio.sleep(0)
                log.append("b")

            task = start(method())
            asyncio.get_running_loop().call_soon(log.append, "callback")
            await task
            await turns()
            self.assertEqual(log, ["a", "b", "callback"])

        await self.each(scenario)

    async def test_a_future_finished_later_resumes_it(self):
        async def scenario(start):
            log = []
            gate = asyncio.get_running_loop().create_future()

            async def method():
                log.append("a")
                value = await gate
                log.append(("b", value))

            task = start(method())
            await turns()
            gate.set_result("v")
            await task
            self.assertEqual(log, ["a", ("b", "v")])

        await self.each(scenario)

    async def test_an_error_after_the_first_step_ends_the_task(self):
        async def scenario(start):
            async def method():
                await asyncio.sleep(0)
                raise KeyError("later")

            task = start(method())
            with self.assertRaises(KeyError):
                await task

        await self.each(scenario)

    async def test_the_loop_task_factory_is_not_used(self):
        async def scenario(start):
            loop = asyncio.get_running_loop()
            made = []

            def factory(loop, coro, **kwargs):
                made.append(coro)
                return asyncio.Task(coro, loop=loop, **kwargs)

            loop.set_task_factory(factory)
            try:
                async def method():
                    await asyncio.sleep(0)
                    return 4

                self.assertEqual(await start(method()), 4)
                self.assertEqual(made, [])
            finally:
                loop.set_task_factory(None)

        await self.each(scenario)

    async def test_the_stack_shows_the_methods_coroutine(self):
        async def scenario(start):
            gate = asyncio.get_running_loop().create_future()

            async def the_method():
                await gate

            task = start(the_method())
            frames = task.get_stack()
            self.assertEqual(frames[-1].f_code.co_name, "the_method")
            gate.set_result(None)
            await task

        await self.each(scenario)

    async def test_a_retrieved_error_is_not_logged_again(self):
        async def scenario(start):
            async def method():
                raise ValueError("retrieved")

            with self.assertNoLogs("asyncio", level=logging.ERROR):
                done = start(method())
                self.assertIsInstance(done.exception(), ValueError)
                del done
                await turns()
                gc.collect()
                await turns()

        await self.each(scenario)


if __name__ == "__main__":
    unittest.main()
