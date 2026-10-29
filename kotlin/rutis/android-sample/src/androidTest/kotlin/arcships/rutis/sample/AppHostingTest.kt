package arcships.rutis.sample

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.util.Log
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import arcships.rutis.RutisApp
import arcships.rutis.RutisState
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import org.junit.After
import org.junit.Assume.assumeTrue
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith
import java.util.concurrent.LinkedBlockingQueue
import java.util.concurrent.TimeUnit
import kotlin.test.assertEquals

/**
 * The application on ART dialing a rutis-host
 * that crates/rutis-host/tests/android.rs runs on the machine (reached
 * through `adb reverse`; the instrumentation arguments `address` and
 * `token`). Two connections of the application, `pixel` and `tablet`, each
 * installing its own plugins; the tablet provides its services under other
 * names (`device-tablet`, …), as two devices on one rutis must; `stop`
 * unloads one connection's plugins alone, `start` brings them back.
 *
 * The host test checks rutis's side between the steps and says so with a
 * broadcast (`STEP`, extra `step`), which this test waits for; without the
 * host test (no `address`), it is skipped.
 */
@RunWith(AndroidJUnit4::class)
class AppHostingTest {
    private val context: Context get() = InstrumentationRegistry.getInstrumentation().targetContext
    private val arguments get() = InstrumentationRegistry.getArguments()
    private val steps = LinkedBlockingQueue<String>()
    private val receiver = object : BroadcastReceiver() {
        override fun onReceive(context: Context, intent: Intent) {
            steps.put(intent.getStringExtra("step") ?: "")
        }
    }
    private val apps = ArrayList<RutisApp>()

    @Before
    fun listen() {
        // Exported: the host test sends it with `adb shell am broadcast`. (The
        // flag is a constant of API 33, ignored before it.)
        context.registerReceiver(receiver, IntentFilter(STEP), Context.RECEIVER_EXPORTED)
    }

    @After
    fun close() {
        for (app in apps) app.stop()
        context.unregisterReceiver(receiver)
    }

    /** Waits until the host test checked `name`. */
    private fun checked(name: String) {
        Log.i(TAG, "at $name")
        assertEquals(name, steps.poll(3, TimeUnit.MINUTES), "the host test's check after $name")
    }

    private fun RutisApp.reaches(what: String, predicate: (RutisState) -> Boolean): RutisState = runBlocking {
        withTimeout(60_000) { state.first(predicate) }.also { Log.i(TAG, "$id: $what: $it") }
    }

    @Test
    fun connectionsAreInstancesThatStopAndStartAgain() {
        val address = arguments.getString("address")
        assumeTrue("run by crates/rutis-host/tests/android.rs", address != null)
        val token = arguments.getString("token") ?: ""
        val pixel = Samples.app(context, address!!, "pixel", token).also(apps::add)
        val tablet = Samples.app(context, address, "tablet", token, suffix = "tablet").also(apps::add)

        pixel.start()
        tablet.start()
        pixel.reaches("connected") { it is RutisState.Connected }
        tablet.reaches("connected") { it is RutisState.Connected }
        checked("connected")

        pixel.stop()
        pixel.reaches("stopped") { it == RutisState.Stopped() }
        assertEquals(RutisState.Connected::class, tablet.state.value::class)
        checked("stopped")

        pixel.start()
        pixel.reaches("connected again") { it is RutisState.Connected }
        checked("started")

        pixel.stop()
        tablet.stop()
        pixel.reaches("stopped") { it == RutisState.Stopped() }
        tablet.reaches("stopped") { it == RutisState.Stopped() }
    }

    companion object {
        const val STEP = "arcships.rutis.sample.test.STEP"
        const val TAG = "rutis-test"
    }
}
