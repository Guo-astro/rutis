package arcships.rutis

import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import arcships.rutis.testing.RutisTest
import org.junit.Test
import org.junit.runner.RunWith
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

/** The `clock` a loaded row was given, for the test to call where it likes. */
@Volatile
private var given: Clock? = null

private val usesClock = Rutis.define<Rutis.NoConfig>(name = "uses-clock", inject = listOf("clock")) { ctx, _ ->
    given = ctx.use<Clock>("clock")
}

/** The context of a loaded row, for the test to emit with where it likes. */
@Volatile
private var bailing: Ctx? = null

private val emits = Rutis.define<Rutis.NoConfig>(name = "emits") { ctx, _ ->
    bailing = ctx
}

/**
 * A call that waits for rutis is refused on the main thread (design §11.3
 * #3), with what to do instead; from another thread it is made. The plugin
 * runs in the testing tool, on ART, the tool being the host.
 */
@RunWith(AndroidJUnit4::class)
class MainThreadTest {
    @Test
    fun aBlockingCallOnTheMainThreadIsRefused() {
        RutisTest.load(usesClock, services = mapOf("clock" to TestClock())).use {
            val clock = given!!
            assertEquals(42L, clock.now())
            var refused: Throwable? = null
            InstrumentationRegistry.getInstrumentation().runOnMainSync {
                refused = runCatching { clock.now() }.exceptionOrNull()
            }
            assertTrue(refused is IllegalStateException, "$refused")
            assertTrue(refused!!.message!!.contains("not on the main thread"), refused!!.message)
        }
    }

    /** A bail whose listeners are elsewhere waits for the host: refused on the main thread too. */
    @Test
    fun aBailThroughTheHostOnTheMainThreadIsRefused() {
        RutisTest.load(emits).use { loaded ->
            // A listener of the test side is elsewhere: the bail goes through the host.
            loaded.on("ask") { "yes" }
            val ctx = bailing!!
            assertEquals("yes", ctx.bail("ask"))
            var refused: Throwable? = null
            InstrumentationRegistry.getInstrumentation().runOnMainSync {
                refused = runCatching { ctx.bail("ask") }.exceptionOrNull()
            }
            assertTrue(refused is IllegalStateException, "$refused")
            assertTrue(refused!!.message!!.contains("not on the main thread"), refused!!.message)
            // An emit does not wait: it goes on the main thread too.
            InstrumentationRegistry.getInstrumentation().runOnMainSync { ctx.emit("ask") }
        }
    }
}
