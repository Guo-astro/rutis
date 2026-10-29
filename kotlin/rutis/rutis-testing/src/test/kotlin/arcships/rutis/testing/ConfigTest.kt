package arcships.rutis.testing

import arcships.rutis.Rutis
import arcships.rutis.RutisException
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonObject
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith

/**
 * A row's config: rutis checks it against the plugin's schema and fills its
 * defaults (`rutis::config::check`); the runtime only decodes it
 * (docs/design-platform-cleanup-2026-10-28.md §7.5).
 */
class ConfigTest {
    @Serializable
    data class Weather(val city: String, val units: String = "metric", val days: Int? = null)

    @Serializable
    data class Strict(val days: Int)

    /** A nullable config that is not a class gets null when there is none. */
    @Test
    fun aNullableConfigGetsNull() {
        val seen = ArrayList<Int?>()
        val plugin = Rutis.define<Int?>(name = "count") { _, config -> synchronized(seen) { seen.add(config) } }
        RutisTest.load(plugin).close()
        assertEquals(listOf<Int?>(null), seen)
    }

    // guarantee: sdk/config-schema (filled and validated before decoding)
    @Test
    fun aRowsConfigIsFilledAndValidatedBeforeItIsDecoded() {
        val seen = ArrayList<Weather>()
        val plugin = Rutis.define<Weather>(name = "weather") { _, config -> synchronized(seen) { seen.add(config) } }
        RutisTest.load(plugin, mapOf("city" to "Oslo")).close()
        assertEquals(listOf(Weather("Oslo")), seen)
        val error = assertFailsWith<RutisException> { RutisTest.load(plugin, mapOf("days" to "x")).close() }
        assertEquals("weather: config: city is required\nconfig: days should be an integer or null, got \"x\"", error.message)
        assertEquals(1, seen.size)
    }

    /** A decoder error after the schema passed is one sentence, with its path (config schema design §3.3). */
    @Test
    fun aDecoderErrorAfterTheSchemaIsOneSentence() {
        val loose = Json.parseToJsonElement("""{"type":"object","properties":{"days":{}}}""") as JsonObject
        val plugin = Rutis.define<Strict>(name = "strict", schema = loose) { _, _ -> }
        var error = assertFailsWith<RutisException> { RutisTest.load(plugin, mapOf("days" to "x")).close() }
        assertEquals("strict: config: days is not a value the plugin accepts", error.message)
        error = assertFailsWith<RutisException> { RutisTest.load(plugin, emptyMap<String, Any>()).close() }
        assertEquals("strict: config: days is not a value the plugin accepts", error.message)
    }
}
