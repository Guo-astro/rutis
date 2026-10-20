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
}
