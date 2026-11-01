package arcships.rutis

import arcships.rutis.internal.Report
import arcships.rutis.internal.RuntimeState
import kotlinx.coroutines.runBlocking
import java.util.Collections
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse

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
}
