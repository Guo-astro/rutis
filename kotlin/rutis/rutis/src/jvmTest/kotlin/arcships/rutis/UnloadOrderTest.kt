package arcships.rutis

import arcships.rutis.internal.Report
import arcships.rutis.internal.RuntimeState
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.cancelAndJoin
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.runBlocking
import java.util.Collections
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertFalse
import kotlin.test.assertTrue

/**
 * Unloading runs one stack in reverse: cleanups and the withdrawals of the
 * services a row provides, in the order they were registered (Rust's
 * drain_effects; docs/design-unload-order-2026-11-01.md).
 */
class UnloadOrderTest {
    /** What reading `hello` through `ctx` gives: the greeting, or the error's code. */
    private fun read(ctx: Ctx): String = try {
        "greet " + ctx.use<Greeter>("hello").greet("Kim")
    } catch (e: Exception) {
        val code = when (e) {
            is NotDeclaredException -> e.code
            is UnavailableException -> e.code
            else -> throw e
        }
        "error $code"
    }

    private suspend fun load(rt: RuntimeState, key: String, plugin: Plugin) =
        rt.load(key, key, plugin.name, null, emptyMap(), emptyMap(), emptyList())

    @Test
    fun aCleanupRegisteredAfterProvideReadsItAndOneBeforeDoesNot() = runBlocking<Unit> {
        val log = Collections.synchronizedList(ArrayList<String>())
        val plugin = Rutis.define<Rutis.NoConfig>(name = "order") { ctx, _ ->
            ctx.effect { log.add("before: ${read(ctx)}") }
            ctx.provide("hello", Hello("hi"))
            ctx.effect { log.add("after: ${read(ctx)}") }
        }
        val rt = RuntimeState(listOf(plugin))
        load(rt, "o", plugin)
        rt.unload("o")
        assertEquals(listOf("after: greet hi Kim", "before: error not-declared"), log)
    }

    @Test
    fun anotherRowLosesItWhenTheUnloadStarts() = runBlocking<Unit> {
        val log = Collections.synchronizedList(ArrayList<String>())
        var user: Ctx? = null
        val provider = Rutis.define<Rutis.NoConfig>(name = "provider") { ctx, _ ->
            ctx.provide("hello", Hello("hi"))
            // Registered after the provide: it runs before the withdrawal.
            ctx.effect { log.add("own: ${read(ctx)}"); log.add("other: ${read(user!!)}") }
        }
        val using = Rutis.define<Rutis.NoConfig>(name = "using", inject = listOf("hello")) { ctx, _ -> user = ctx }
        val rt = RuntimeState(listOf(provider, using))
        load(rt, "p", provider)
        load(rt, "u", using)
        assertEquals("greet hi Kim", read(user!!))
        rt.unload("p")
        assertEquals(listOf("own: greet hi Kim", "other: error unavailable"), log)
        assertEquals("error unavailable", read(user!!))
    }

    @Test
    fun updatingARowThatIsUnloadingSaysItIsNotLoaded() = runBlocking<Unit> {
        val plugin = Rutis.define<Rutis.NoConfig>(name = "updating") { _, _ -> }
        val rt = RuntimeState(listOf(plugin))
        load(rt, "u", plugin)
        rt.rows["u"]!!.unloading = true
        val error = assertFailsWith<IllegalStateException> { rt.update("u", null) }
        assertEquals("row u is not loaded", error.message)
    }

    @Test
    fun aCleanupRegisteringOneIsRefused() = runBlocking<Unit> {
        val lines = Collections.synchronizedList(ArrayList<String>())
        var ran = false
        val plugin = Rutis.define<Rutis.NoConfig>(name = "nested") { ctx, _ ->
            ctx.effect { ctx.effect { ran = true } }
        }
        val rt = RuntimeState(listOf(plugin))
        load(rt, "nested", plugin)
        val previous = Report.replace { lines.add(it) }
        try {
            rt.unload("nested")
        } finally {
            Report.replace(previous)
        }
        assertFalse(ran, "the cleanup registered while unloading ran")
        assertEquals(listOf("rutis: row nested is not active: cleanup is not registered"), lines)
    }

    @Test
    fun anOldWithdrawalLeavesANewRegistrationAlone() = runBlocking<Unit> {
        val log = Collections.synchronizedList(ArrayList<String>())
        val plugin = Rutis.define<Rutis.NoConfig>(name = "again") { ctx, _ ->
            val hello = Hello("hi")
            val withdraw = ctx.provide("hello", hello)
            withdraw()
            ctx.provide("hello", hello)
            withdraw()
            log.add("during: ${read(ctx)}")
            ctx.effect { log.add("cleanup: ${read(ctx)}") }
        }
        val rt = RuntimeState(listOf(plugin))
        load(rt, "a", plugin)
        rt.unload("a")
        assertEquals(listOf("during: greet hi Kim", "cleanup: greet hi Kim"), log)
    }

    /**
     * The deadline bounds the cleanups, not the withdrawals: a cleanup that
     * runs out of time does not stop the withdrawal behind it, which leaves
     * no registration and frees the name for the next load.
     */
    @Test
    fun theDeadlineGivesUpCleanupsButStillWithdrawsTheServices() = runBlocking<Unit> {
        val lines = Collections.synchronizedList(ArrayList<String>())
        val hang = CompletableDeferred<Unit>()
        val slow = Rutis.define<Rutis.NoConfig>(name = "slow") { ctx, _ ->
            ctx.provide("hello", Hello("hi"))
            // Registered after the provide: it runs first, and is the one the
            // deadline reaches.
            ctx.effect { hang.await() }
        }
        val again = Rutis.define<Rutis.NoConfig>(name = "again") { ctx, _ -> ctx.provide("hello", Hello("again")) }
        val rt = RuntimeState(listOf(slow, again))
        load(rt, "slow", slow)
        val previous = Report.replace { lines.add(it) }
        try {
            // The deadline given to this row alone: nothing else cancels it,
            // so which step runs out of time is the test's to decide.
            val deadline = System.nanoTime() + 200 * 1_000_000L
            assertFalse(rt.unload("slow", deadline), "the cleanups finished in time")
            assertTrue("rutis: gave up the cleanups of row slow" in lines, "the first cleanup did not run out of time: $lines")
            assertFalse("hello" in rt.services, "the withdrawal behind the cleanup that timed out did not run")
            // The name is free: the row loaded again provides it.
            load(rt, "slow", slow)
            assertTrue("hello" in rt.services)
        } finally {
            Report.replace(previous)
            hang.complete(Unit)
        }
    }

    /**
     * A caller that cancels the unload while a cleanup runs (the session
     * ending, say) still gets every withdrawal: they run after the loop, in
     * `NonCancellable`. One skipped would keep the service registered and the
     * name taken for the row's next load.
     */
    @Test
    fun aCancelledUnloadStillWithdrawsTheServices() = runBlocking<Unit> {
        val hung = CompletableDeferred<Unit>()
        val slow = Rutis.define<Rutis.NoConfig>(name = "slow") { ctx, _ ->
            ctx.provide("hello", Hello("hi"))
            // Registered after the provide: it hangs, so the cancel lands
            // before the withdrawal's place comes.
            ctx.effect { hung.await() }
        }
        val rt = RuntimeState(listOf(slow))
        load(rt, "slow", slow)
        try {
            val unloading = launch { rt.unload("slow") }
            // The cleanup is running and hanging: cancel the unload now.
            delay(100)
            unloading.cancelAndJoin()
            assertFalse("hello" in rt.services, "the cancellation skipped the row's withdrawal")
            // The name is free: the row loaded again provides it.
            load(rt, "slow", slow)
            assertTrue("hello" in rt.services)
        } finally {
            hung.complete(Unit)
        }
    }

    /**
     * Each wait before the stack has its own cap, the shutdown budget, so a
     * caller with a larger deadline cannot make one wait run for it: a
     * listener that never returns is given up at the budget, not at
     * `dispose`'s deadline.
     */
    @Test
    fun aWaitBeforeTheStackIsCappedByTheShutdownBudget() = runBlocking<Unit> {
        val lines = Collections.synchronizedList(ArrayList<String>())
        val slow = Rutis.define<Rutis.NoConfig>(name = "slow") { ctx, _ -> ctx.provide("hello", Hello("hi")) }
        val rt = RuntimeState(listOf(slow))
        rt.shutdownMillis = 200
        load(rt, "slow", slow)
        // A listener that never returns: waited for, at most the budget.
        rt.rows["slow"]!!.listening.add()
        val previous = Report.replace { lines.add(it) }
        try {
            val started = System.nanoTime()
            // A deadline far past the shutdown budget: without its own cap the
            // wait would run for five seconds.
            assertFalse(rt.dispose(5_000), "dispose finished in time")
            val took = (System.nanoTime() - started) / 1_000_000
            assertTrue(took < 3_000, "the wait used dispose's deadline: ${took}ms")
            assertTrue(lines.any { it.startsWith("rutis: listeners of row slow still running") }, "the listener wait was not given up: $lines")
            assertFalse("hello" in rt.services, "the withdrawal behind the wait did not run")
        } finally {
            Report.replace(previous)
            rt.rows["slow"]?.listening?.done()
        }
    }

    /**
     * The cancellation can land before the stack is reached: `dispose` bounds
     * the whole row with its own timeout, and a listener that does not return
     * within the shutdown budget keeps the row in the wait before the stack.
     * The withdrawals still run — the protection covers the waits too.
     */
    @Test
    fun aCancellationDuringTheListenerWaitStillWithdrawsTheServices() = runBlocking<Unit> {
        val slow = Rutis.define<Rutis.NoConfig>(name = "slow") { ctx, _ -> ctx.provide("hello", Hello("hi")) }
        val rt = RuntimeState(listOf(slow))
        // The listener budget far above the dispose deadline: the wait runs
        // past it, so the unload is cancelled there, before any cleanup or
        // withdrawal.
        rt.shutdownMillis = 5_000
        load(rt, "slow", slow)
        val row = rt.rows["slow"]!!
        // A listener of the row that never returns.
        row.listening.add()
        try {
            assertFalse(rt.dispose(200), "dispose finished in time")
            assertFalse("hello" in rt.services, "the cancellation during the listener wait skipped the withdrawal")
            // The name is free: the row loaded again provides it.
            load(rt, "slow", slow)
            assertTrue("hello" in rt.services)
        } finally {
            row.listening.done()
        }
    }
}

