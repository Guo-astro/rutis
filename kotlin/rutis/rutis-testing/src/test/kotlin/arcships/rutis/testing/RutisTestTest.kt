package arcships.rutis.testing

import arcships.rutis.Rutis
import arcships.rutis.RutisClient
import arcships.rutis.RutisException
import arcships.rutis.RutisService
import arcships.rutis.service
import kotlinx.coroutines.runBlocking
import kotlinx.serialization.Serializable
import java.util.Collections
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertTrue

@Serializable
data class Weather(val city: String, val units: String = "metric")

@RutisClient
interface Clock {
    fun now(): Long
}

@RutisService
class FakeClock {
    fun now(): Long = 42
}

@RutisService
class Forecast(private val city: String, private val clock: Clock) {
    fun today(): String = "sunny in $city at ${clock.now()}"

    suspend fun later(): String = "rain in $city"
}

private val weather = Rutis.define<Weather>(
    name = "weather",
    inject = listOf("clock"),
    provides = mapOf("forecast" to service<Forecast>(sync = setOf("today"))),
) { ctx, config ->
    ctx.provide("forecast", Forecast("${config.city} (${config.units})", ctx.use<Clock>("clock")))
    ctx.on("weather/ask") { args -> ctx.emit("weather/said", "sunny in ${config.city} for ${args[0]}"); null }
}

/** The testing tool on the real kernel: `rutis-host test`, one host for this test process. */
class RutisTestTest {
    @Test
    fun aPluginLoadsIsCalledHeardUpdatedAndUnloaded() = runBlocking<Unit> {
        RutisTest.load(weather, Weather("Oslo"), services = mapOf("clock" to FakeClock())).use { loaded ->
            assertEquals("weather", loaded.row)
            val forecast = loaded.service("forecast")
            assertEquals(mapOf("today" to "sync", "later" to "async"), forecast.methods)
            // rutis filled the default before the runtime decoded it.
            assertEquals("sunny in Oslo (metric) at 42", forecast.call("today"))
            val said = Collections.synchronizedList(ArrayList<String>())
            loaded.on("weather/said") { args -> said.add(args[0] as String); null }
            loaded.emit("weather/ask", "Kim")
            val deadline = System.currentTimeMillis() + 5_000
            while (said.isEmpty() && System.currentTimeMillis() < deadline) Thread.sleep(5)
            assertEquals(listOf("sunny in Oslo for Kim"), said)
            loaded.update(mapOf("city" to "Bergen"))
            assertEquals("sunny in Bergen (metric) at 42", loaded.service("forecast").call("today"))
        }
    }

    @Test
    fun aConfigRutisRefusesFailsTheLoadWithTheRowsLine() {
        val error = assertFailsWith<RutisException> {
            RutisTest.load(weather, mapOf("units" to 1), services = mapOf("clock" to FakeClock())).close()
        }
        assertTrue(error.message!!.startsWith("weather: config: "), error.message)
        assertTrue("city is required" in error.message!!, error.message)
    }

    @Test
    fun rowsHaveTheIdsTheTestGives() = runBlocking<Unit> {
        RutisTest.load(weather, Weather("Oslo"), services = mapOf("clock" to FakeClock()), row = "oslo").use { oslo ->
            assertEquals("oslo", oslo.row)
            assertEquals("sunny in Oslo (metric) at 42", oslo.service("forecast").call("today"))
            // The id is taken: rutis says so, and adds no row.
            val error = assertFailsWith<RutisException> { oslo.load(Weather("Bergen"), row = "oslo") }
            assertTrue("oslo" in error.message!!, error.message)
        }
    }
}
