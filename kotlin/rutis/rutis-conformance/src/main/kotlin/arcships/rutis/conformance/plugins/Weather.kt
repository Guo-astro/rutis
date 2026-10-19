// The runtime conformance plugin (rutis_bridge::runtime::testing::runtime)
// for the Kotlin SDK: it injects `clock` and provides `weather`. What calls on
// it return is in crates/rutis-bridge/src/runtime/testing/weather.expected.json.
package arcships.rutis.conformance.plugins

import arcships.rutis.Rutis
import arcships.rutis.RutisClient
import arcships.rutis.RutisService
import arcships.rutis.service
import kotlinx.coroutines.delay
import kotlinx.serialization.Contextual
import kotlinx.serialization.Serializable
import java.math.BigDecimal
import java.math.BigInteger
import java.time.Instant
import java.time.LocalDate
import java.time.LocalDateTime
import java.time.LocalTime
import java.time.OffsetDateTime
import java.time.ZoneOffset
import java.util.UUID
import kotlin.system.exitProcess

@Serializable
data class WeatherConfig(val city: String = "")

@RutisClient
interface Clock {
    fun now(): Long
}

/** A data class with fields JSON would have turned into text: they cross as their values (V9). */
@Serializable
data class Reading(
    val raw: ByteArray? = null,
    @Contextual val at: Instant? = null,
    val n: Long? = null,
)

private val oslo = ZoneOffset.ofHours(2)

/** Values only Kotlin makes, by name (`samples` in the expectations). */
private val samples: Map<String, () -> Any?> = linkedMapOf(
    "kotlin/value-instant" to { OffsetDateTime.of(2026, 10, 15, 5, 22, 15, 123456789, oslo).toInstant() },
    "kotlin/value-kotlin-instant-uuid" to {
        listOf(kotlin.time.Instant.parse("2026-10-15T03:22:15.5Z"), kotlin.uuid.Uuid.parse("0f8fad5b-d9cb-469f-a165-70867728950e"))
    },
    "kotlin/value-data-class" to { Reading(raw = "hello".toByteArray(), at = OffsetDateTime.of(2026, 10, 15, 5, 22, 15, 0, oslo).toInstant()) },
    "kotlin/value-int-keys" to { linkedMapOf(1 to "a", 2 to "b") },
    "kotlin/value-set" to { linkedSetOf("a", "b") },
    "kotlin/value-bigint" to { BigInteger("-123456789012345678901") },
    "kotlin/value-constructed" to {
        listOf(
            LocalDateTime.of(2026, 10, 15, 14, 30, 5, 250_000_000),
            LocalDate.of(2026, 10, 15),
            LocalTime.of(14, 30, 5, 250_000_000),
            BigDecimal("1.10"),
            UUID.fromString("0f8fad5b-d9cb-469f-a165-70867728950e"),
        )
    },
    "kotlin/big-long" to { Reading(n = 1L shl 60) },
    "kotlin/big-long-keys" to { linkedMapOf(1L to "one", Long.MAX_VALUE to "max") },
    "kotlin/nan" to { Double.NaN },
    "kotlin/exception-value" to { IllegalStateException("bad") },
)

@RutisService
class Weather(private val clock: Clock, @Volatile var city: String) {
    fun today(): String = "$city at ${clock.now()}"

    suspend fun later(): String {
        delay(10)
        return "$city later"
    }

    /** Calls back during the call. */
    fun each(callback: (String) -> String): List<String> = listOf("mon", "tue").map(callback)

    /** Returns the value it was given. */
    fun echo(value: Any?): Any? = value

    fun crash() {
        exitProcess(17)
    }

    /** The values only Kotlin makes. */
    fun samples(): List<String> = samples.keys.toList()

    /** The value `name`. */
    fun sample(name: String): Any? = (samples[name] ?: throw IllegalArgumentException("no sample $name"))()
}

/** `undeclared` in the expectations: provided without a declaration, with a method inherited. */
open class Days {
    fun week(n: Int): List<String> = listOf("mon", "tue", "wed").take(n)
}

@RutisService
class Forecast(private val city: String) : Days() {
    fun tomorrow(): String = "$city tomorrow"

    @Suppress("unused")
    private fun hidden(): String = "not a method rutis sees"
}

val weather = Rutis.define<WeatherConfig>(
    name = "weather",
    inject = listOf("clock"),
    provides = mapOf("weather" to service<Weather>(sync = setOf("today", "each", "echo", "crash", "samples", "sample"))),
) { ctx, config ->
    val w = Weather(ctx.use<Clock>("clock"), config.city.ifEmpty { "Oslo" })
    ctx.provide("weather", w)
    ctx.provide("forecast", Forecast("Oslo"))
    ctx.on("weather/city") { args ->
        w.city = args.singleOrNull() as? String ?: throw IllegalArgumentException("weather/city needs one city")
        null
    }
    ctx.effect { println("weather: bye") }
}
