package arcships.rutis.conformance

import arcships.rutis.RutisException
import arcships.rutis.RutisService
import arcships.rutis.conformance.plugins.weather
import arcships.rutis.testing.RutisTest
import arcships.rutis.testing.TestService
import kotlinx.coroutines.runBlocking
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.jsonArray
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import java.util.concurrent.atomic.AtomicLong
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertTrue
import kotlin.test.fail

@RutisService
class TestClock {
    private val n = AtomicLong()

    fun now(): Long = n.getAndIncrement()
}

/**
 * The testing tool against the expectations the real runtimes are checked
 * against (rutis_bridge::runtime::testing::runtime, weather.expected.json):
 * the same list (#206, design K16).
 */
class WeatherConformanceTest {
    private val problems = ArrayList<String>()

    private fun check(what: String, entry: JsonObject, outcome: Result<Any?>) {
        val why = (entry["why"] as? JsonPrimitive)?.content ?: ""
        val returns = entry["returns"]
        val throws = entry["throws"] as? JsonObject
        when {
            returns != null -> {
                val value = outcome.getOrElse {
                    problems.add("$what ($why): expected $returns, got $it")
                    return
                }
                var got = written(value)
                val ignore = (entry["ignore"] as? JsonArray)?.map { it.jsonPrimitive.content } ?: emptyList()
                if (got is JsonObject) got = JsonObject(got.filterKeys { it !in ignore })
                if (got != normalized(returns)) problems.add("$what ($why): got $got, want $returns")
            }
            throws != null -> {
                val error = outcome.exceptionOrNull() ?: run {
                    problems.add("$what ($why): expected an error, got ${outcome.getOrNull()}")
                    return
                }
                val name = (throws["name"] as? JsonPrimitive)?.content
                val message = (throws["message"] as? JsonPrimitive)?.content
                if (name == null && message == null) return
                if (error !is RutisException) {
                    problems.add("$what ($why): expected an error from the plugin, got $error")
                    return
                }
                if (name != null && error.name != name) problems.add("$what ($why): the error's name ${error.name}, want $name")
                if (message != null && error.message != message) problems.add("$what ($why): the error's message ${error.message}, want $message")
            }
            else -> fail("$what: the expectations say neither returns nor throws")
        }
    }

    private suspend fun call(service: TestService, method: String, args: List<JsonElement>): Result<Any?> =
        runCatching { service.raw(method, args.map(::argument)) }

    @Test
    fun theTestingToolMeetsTheRuntimeExpectations() = runBlocking<Unit> {
        val expected = expectations("weather.expected.json")
        RutisTest.load(weather, services = mapOf("clock" to TestClock())).use { loaded ->
            val service = loaded.service(expected["service"]!!.jsonPrimitive.content)
            for ((method, kind) in expected["methods"]!!.jsonObject) {
                assertEquals(kind.jsonPrimitive.content, service.methods[method], "${service.name}.$method")
            }
            for ((i, entry) in expected["calls"]!!.jsonArray.withIndex()) {
                entry as JsonObject
                if (entry["allowed"] != null) continue
                val method = entry["call"]!!.jsonPrimitive.content
                check("calls[$i] $method", entry, call(service, method, entry["args"]!!.jsonArray))
            }
            // Values only Kotlin makes.
            @Suppress("UNCHECKED_CAST")
            val names = service.call("samples") as List<String>
            assertTrue(names.isNotEmpty(), "the Kotlin plugin names its samples")
            for (name in names) {
                val entry = expected["samples"]!!.jsonArray.map { it.jsonObject }.firstOrNull { it["sample"]?.jsonPrimitive?.content == name }
                    ?: fail("the expectations list no sample $name")
                check("sample $name", entry, call(service, "sample", listOf(JsonPrimitive(name))))
            }
            for (event in expected["events"]!!.jsonArray.map { it.jsonObject }) {
                val then = event["then"] as? JsonObject ?: continue
                // As a host plugin emits: parallel waits for the listeners.
                loaded.parallel(event["emit"]!!.jsonPrimitive.content, *event["args"]!!.jsonArray.map { arcships.rutis.internal.Native.toNative(argument(it)) }.toTypedArray())
                val entry = JsonObject(then + ("why" to (event["why"] ?: JsonPrimitive(""))))
                check("after ${event["emit"]}", entry, call(service, then["call"]!!.jsonPrimitive.content, then["args"]!!.jsonArray))
            }
            // A service the plugin does not declare: its methods inferred, all async.
            val undeclared = expected["undeclared"]!!.jsonObject
            val forecast = loaded.service(undeclared["service"]!!.jsonPrimitive.content)
            assertEquals(undeclared["methods"]!!.jsonObject.mapValues { it.value.jsonPrimitive.content }, forecast.methods, "the undeclared service's methods")
            for ((i, entry) in undeclared["calls"]!!.jsonArray.withIndex()) {
                entry as JsonObject
                val method = entry["call"]!!.jsonPrimitive.content
                check("undeclared.calls[$i] $method", entry, call(forecast, method, entry["args"]!!.jsonArray))
            }
        }
        if (problems.isNotEmpty()) fail(problems.joinToString("\n"))
    }
}
