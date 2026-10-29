package arcships.rutis

import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import org.junit.After
import org.junit.Assume.assumeTrue
import org.junit.Test
import org.junit.runner.RunWith
import java.util.concurrent.CompletableFuture
import java.util.concurrent.TimeUnit
import kotlin.test.assertEquals
import kotlin.test.assertTrue

@RutisClient
interface Clock {
    fun now(): Long
}

@RutisService
class TestClock {
    fun now(): Long = 42
}

/** The second connection's plugin: provides `clock`, and answers `ask`. */
private val clockPlugin = Rutis.define<Rutis.NoConfig>(name = "main-thread-clock") { ctx, _ ->
    ctx.provide("main-thread-clock", TestClock())
    ctx.on("main-thread/ask") { "yes" }
}

/** The first connection's plugin: uses the other connection's `clock` through rutis, and emits. */
@Volatile
private var usesClock = CompletableFuture<Pair<Clock, Ctx>>()
private val user = Rutis.define<Rutis.NoConfig>(name = "main-thread-user", inject = listOf("main-thread-clock")) { ctx, _ ->
    usesClock.complete(ctx.use<Clock>("main-thread-clock") to ctx)
}

/**
 * A call that waits for rutis is refused on the main thread (design §11.3
 * #3), with what to do instead; from another thread it is made. A
 * synchronous `bail` always waits for rutis, which dispatches every event:
 * refused on the main thread whatever listens.
 *
 * As on a device an application hosts plugins: two connections of this
 * process ([RutisApp]) each install their own plugins, dialing a rutis on
 * the development machine. The instrumentation arguments `address` and
 * `token` say where (`adb reverse` to a `rutis-host/listen` row); without
 * them the test is skipped. Run locally only (docs/ci.md §12).
 */
@RunWith(AndroidJUnit4::class)
class MainThreadTest {
    private val apps = ArrayList<RutisApp>()

    @After
    fun stop() {
        for (app in apps) app.stop()
    }

    private fun connected(): Pair<Clock, Ctx> {
        val arguments = InstrumentationRegistry.getArguments()
        val address = arguments.getString("address")
        assumeTrue("needs a rutis to dial: the instrumentation arguments address and token", address != null)
        val token = arguments.getString("token") ?: ""
        usesClock = CompletableFuture()
        val clocks = RutisApp(plugins = listOf(clockPlugin), connect = address, id = "main-thread-a", token = token).also(apps::add)
        val users = RutisApp(plugins = listOf(user), connect = address, id = "main-thread-b", token = token).also(apps::add)
        clocks.start()
        users.start()
        runBlocking { withTimeout(60_000) { users.state.first { it is RutisState.Connected } } }
        return usesClock.get(60, TimeUnit.SECONDS)
    }

    @Test
    fun aBlockingCallOnTheMainThreadIsRefused() {
        val (clock, _) = connected()
        assertEquals(42L, clock.now())
        var refused: Throwable? = null
        InstrumentationRegistry.getInstrumentation().runOnMainSync {
            refused = runCatching { clock.now() }.exceptionOrNull()
        }
        assertTrue(refused is IllegalStateException, "$refused")
        assertTrue(refused!!.message!!.contains("not on the main thread"), refused!!.message)
    }

    /** A bail waits for rutis: refused on the main thread, made from another one. */
    @Test
    fun aBailOnTheMainThreadIsRefused() {
        val (_, ctx) = connected()
        assertEquals("yes", ctx.bail("main-thread/ask"))
        var refused: Throwable? = null
        InstrumentationRegistry.getInstrumentation().runOnMainSync {
            refused = runCatching { ctx.bail("main-thread/ask") }.exceptionOrNull()
        }
        assertTrue(refused is IllegalStateException, "$refused")
        assertTrue(refused!!.message!!.contains("not on the main thread"), refused!!.message)
        // An emit does not wait: it goes on the main thread too.
        InstrumentationRegistry.getInstrumentation().runOnMainSync { ctx.emit("main-thread/ask") }
    }
}
