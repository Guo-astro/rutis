package arcships.rutis

import arcships.rutis.internal.ConfigSchema
import arcships.rutis.testing.RutisTest
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.jsonArray
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import java.io.File
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith

/** Config schemas (docs/design-config-schema-2026-10-27.en.md): the shared case list, and rows filled and validated before decoding. */
class ConfigSchemaTest {
    @Serializable
    data class Weather(val city: String, val units: String = "metric", val days: Int? = null)

    @Serializable
    data class Strict(val days: Int)

    private fun repo(): File {
        var dir = File(".").absoluteFile
        while (!File(dir, "specs/config-schema/cases.json").exists()) dir = dir.parentFile ?: error("no repository above ${File(".").absolutePath}")
        return dir
    }

    // guarantee: sdk/config-schema (the shared case list, Kotlin's column)
    @Test
    fun everyCaseOfTheSharedListPasses() {
        val doc = Json.parseToJsonElement(File(repo(), "specs/config-schema/cases.json").readText()).jsonObject
        val failures = ArrayList<String>()
        for (case in doc["cases"]!!.jsonArray) {
            val c = case.jsonObject
            val name = c["name"]!!.jsonPrimitive.content
            val schema = ConfigSchema.fromValue(c["schema"])
            val config = if (c.containsKey("config")) ConfigSchema.fromValue(c["config"]) else null
            val checked = ConfigSchema.check(schema, config)
            val warnings = checked.warnings.map { "config: ${it.message}" }
            val expectedWarnings = c["warnings"]!!.jsonArray.map { it.jsonPrimitive.content }
            if (warnings != expectedWarnings) failures.add("$name: warnings $warnings")
            val then = c["then"]!!.jsonObject
            val errors = then["errors"]
            if (errors != null) {
                val lines = ConfigSchema.lines(checked.errors)
                if (lines != errors.jsonArray.map { it.jsonPrimitive.content }) failures.add("$name: errors $lines")
            } else if (checked.errors.isNotEmpty()) {
                failures.add("$name: unexpected errors ${checked.errors}")
            } else if (ConfigSchema.toJson(checked.config) != ConfigSchema.toJson(ConfigSchema.fromValue(then["config"]))) {
                failures.add("$name: config ${ConfigSchema.toJson(checked.config)}")
            }
        }
        assertEquals(emptyList(), failures)
    }

    @Test
    fun numbersAreWrittenAsJavaScriptWritesThem() {
        for ((value, text) in listOf(1.0 to "1", 1.5 to "1.5", -0.0 to "0", 1e21 to "1e+21", 1.5e21 to "1.5e+21", 1e-7 to "1e-7",
            0.000001 to "0.000001", 0.00001 to "0.00001", 123.456 to "123.456", -2.5 to "-2.5", 0.1 to "0.1", 1.5e20 to "150000000000000000000")) {
            assertEquals(text, ConfigSchema.doubleText(value), "$value")
        }
    }

    // guarantee: sdk/config-schema (filled and validated before decoding)
    @Test
    fun aRowsConfigIsFilledAndValidatedBeforeItIsDecoded() {
        val seen = ArrayList<Weather>()
        val plugin = Rutis.define<Weather>(name = "weather") { _, config -> synchronized(seen) { seen.add(config) } }
        RutisTest.load(plugin, mapOf("city" to "Oslo")).close()
        assertEquals(listOf(Weather("Oslo")), seen)
        val error = assertFailsWith<RutisException> { RutisTest.load(plugin, mapOf("days" to "x")).close() }
        assertEquals("BindingError", error.name)
        assertEquals("bad-config", error.code)
        assertEquals("config: city is required\nconfig: days should be an integer or null, got \"x\"", error.message)
        assertEquals(1, seen.size)
    }

    /** A decoder error after the schema passed is one sentence, with its path (design §3.3). */
    @Test
    fun aDecoderErrorAfterTheSchemaIsOneSentence() {
        val loose = Json.parseToJsonElement("""{"type":"object","properties":{"days":{}}}""") as JsonObject
        val plugin = Rutis.define<Strict>(name = "strict", schema = loose) { _, _ -> }
        var error = assertFailsWith<RutisException> { RutisTest.load(plugin, mapOf("days" to "x")).close() }
        assertEquals("config: days is not a value the plugin accepts", error.message)
        error = assertFailsWith<RutisException> { RutisTest.load(plugin, emptyMap<String, Any>()).close() }
        assertEquals("config: days is not a value the plugin accepts", error.message)
    }
}
