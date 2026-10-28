package arcships.rutis

import arcships.rutis.internal.Native
import arcships.rutis.internal.peer.Decimal
import arcships.rutis.internal.peer.ValueException
import arcships.rutis.internal.peer.WireMap
import arcships.rutis.internal.peer.WireSet
import arcships.rutis.internal.peer.checkKeys
import arcships.rutis.internal.peer.formatInstant
import arcships.rutis.internal.peer.parseBigInt
import arcships.rutis.internal.peer.parseBytes
import arcships.rutis.internal.peer.parseDate
import arcships.rutis.internal.peer.parseDateTime
import arcships.rutis.internal.peer.parseDecimal
import arcships.rutis.internal.peer.parseInstant
import arcships.rutis.internal.peer.parseTagged
import arcships.rutis.internal.peer.parseTime
import arcships.rutis.internal.peer.parseUuid
import arcships.rutis.internal.peer.taggedText
import kotlinx.serialization.Contextual
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.JsonPrimitive
import java.math.BigDecimal
import java.math.BigInteger
import java.time.Instant
import java.time.LocalDate
import java.time.LocalDateTime
import java.time.LocalTime
import java.util.UUID
import kotlin.test.Test
import kotlin.test.assertContentEquals
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertIs
import kotlin.test.assertTrue
import arcships.rutis.advanced.RutisFormat

/** The ten tagged values' text, map keys, and RutisFormat (design §3.3). */
class ValuesTest {
    @Test
    fun taggedTextsRoundTrip() {
        val texts = mapOf(
            "instant" to listOf("2026-10-15T03:22:15Z", "2026-10-15T03:22:15.123456789Z", "0001-01-01T00:00:00Z", "9999-12-31T23:59:59.999999999Z"),
            "datetime" to listOf("2026-10-15T14:30:05", "2026-10-15T14:30:05.25"),
            "date" to listOf("2026-10-15", "2024-02-29"),
            "time" to listOf("00:00:00", "23:59:59.000000001", "14:30:05.25"),
            "bytes" to listOf("", "eA==", "aGVsbG8="),
            "bigint" to listOf("0", "-1", "18446744073709551617"),
            "decimal" to listOf("0", "-0", "1.10", "-12.3400"),
            "uuid" to listOf("0f8fad5b-d9cb-469f-a165-70867728950e"),
        )
        for ((tag, list) in texts) {
            for (text in list) assertEquals(text, taggedText(parseTagged(tag, text)), "$tag $text")
        }
    }

    @Test
    fun malformedTaggedTextsAreRefused() {
        val bad = mapOf(
            "instant" to listOf("2026-10-15T03:22:15", "2026-10-15T03:22:15+02:00", "0000-01-01T00:00:00Z", "2026-10-15T03:22:15.1234567891Z"),
            "datetime" to listOf("2026-10-15", "2026-10-15T24:00:00", "2026-10-15T14:30:05Z"),
            "date" to listOf("2026-02-30", "2023-02-29", "26-10-15"),
            "time" to listOf("24:00:00", "14:30", "14:30:60"),
            "bytes" to listOf("eA", "eA==\n", "eB==", "e A="),
            "bigint" to listOf("-0", "01", "1.0", "", "9".repeat(4301)),
            "decimal" to listOf("1e3", ".5", "1.", "01"),
            "uuid" to listOf("0F8FAD5B-D9CB-469F-A165-70867728950E", "0f8fad5b"),
        )
        for ((tag, list) in bad) {
            for (text in list) assertFailsWith<ValueException>("$tag $text") { parseTagged(tag, text) }
        }
    }

    @Test
    fun anInstantIsTruncatedToNoFractionWhenWhole() {
        assertEquals("2026-10-15T03:22:15Z", formatInstant(Instant.parse("2026-10-15T03:22:15.000Z")))
        assertEquals(Instant.parse("2026-10-15T03:22:15.5Z"), parseInstant("2026-10-15T03:22:15.5Z"))
        assertEquals(LocalDate.of(2026, 10, 15), parseDate("2026-10-15"))
        assertEquals(LocalTime.of(14, 30, 5, 250_000_000), parseTime("14:30:05.25"))
        assertEquals(LocalDateTime.of(2026, 10, 15, 14, 30, 5), parseDateTime("2026-10-15T14:30:05"))
        assertContentEquals("hello".toByteArray(), parseBytes("aGVsbG8="))
        assertEquals(BigInteger("-5"), parseBigInt("-5"))
        assertEquals(Decimal("1.10"), parseDecimal("1.10"))
        assertEquals(UUID.fromString("0f8fad5b-d9cb-469f-a165-70867728950e"), parseUuid("0f8fad5b-d9cb-469f-a165-70867728950e"))
    }

    @Test
    fun keysAreCheckedAsEveryLanguageComparesThem() {
        checkKeys(listOf("a", 1L, true, BigInteger.TEN, Decimal("1.5"), LocalDate.of(2026, 1, 1)), "map key")
        // 1 and 1.0 are one key, as are 0 and -0.
        assertFailsWith<ValueException> { checkKeys(listOf(1L, 1.0), "map key") }
        assertFailsWith<ValueException> { checkKeys(listOf(JsonPrimitive(0), -0.0), "set element") }
        // Kinds no language keys by.
        assertFailsWith<ValueException> { checkKeys(listOf(Instant.EPOCH), "map key") }
        assertFailsWith<ValueException> { checkKeys(listOf(byteArrayOf(1)), "map key") }
        assertFailsWith<ValueException> { checkKeys(listOf(null), "set element") }
        assertFailsWith<ValueException> { checkKeys(listOf(listOf(1)), "map key") }
        // Beyond ±(2^53-1): send it as a bigint.
        assertFailsWith<ValueException> { checkKeys(listOf(1L shl 60), "map key") }
    }

    @Serializable
    enum class Sky { Clear, Cloudy }

    @Serializable
    data class Reading(
        val name: String,
        val count: Int = 0,
        val big: Long = 0,
        val ratio: Double = 0.0,
        val sky: Sky = Sky.Clear,
        val raw: ByteArray? = null,
        @Contextual val at: Instant? = null,
        @Contextual val day: LocalDate? = null,
        @Contextual val money: BigDecimal? = null,
        @Contextual val huge: BigInteger? = null,
        @Contextual val id: UUID? = null,
        val tags: Set<String> = emptySet(),
        val byNumber: Map<Int, String> = emptyMap(),
        val byName: Map<String, Int> = emptyMap(),
        val unsigned: ULong = 0u,
        val when_: kotlin.time.Instant? = null,
        val note: String? = null,
    )

    @Test
    fun aSerializableClassCrossesWithItsValuesTagged() {
        val reading = Reading(
            name = "oslo", count = 3, big = 1L shl 60, ratio = 1.5, sky = Sky.Cloudy, raw = byteArrayOf(1, 2),
            at = Instant.parse("2026-10-15T03:22:15Z"), day = LocalDate.of(2026, 10, 15), money = BigDecimal("1.10"),
            huge = BigInteger("123456789012345678901"), id = UUID.fromString("0f8fad5b-d9cb-469f-a165-70867728950e"),
            tags = linkedSetOf("a", "b"), byNumber = linkedMapOf(2 to "b", 1 to "a"), byName = linkedMapOf("x" to 1),
            unsigned = ULong.MAX_VALUE, when_ = kotlin.time.Instant.parse("2026-10-15T03:22:15Z"),
        )
        @Suppress("UNCHECKED_CAST")
        val encoded = RutisFormat.encode(Reading.serializer(), reading) as Map<String, Any?>
        assertEquals("oslo", encoded["name"])
        assertEquals(3L, encoded["count"])
        assertEquals(BigInteger.ONE.shiftLeft(60), encoded["big"], "beyond ±(2^53−1): a bigint")
        assertEquals("Cloudy", encoded["sky"])
        assertIs<ByteArray>(encoded["raw"])
        assertEquals(Instant.parse("2026-10-15T03:22:15Z"), encoded["at"])
        assertEquals(Decimal("1.10"), encoded["money"], "the scale is kept")
        assertEquals(WireSet(listOf("a", "b")), encoded["tags"])
        assertEquals(WireMap(listOf(2L to "b", 1L to "a")), encoded["byNumber"])
        assertEquals(mapOf("x" to 1L), encoded["byName"], "string keys: a record")
        assertEquals(BigInteger("18446744073709551615"), encoded["unsigned"])
        assertEquals(Instant.parse("2026-10-15T03:22:15Z"), encoded["when_"])
        assertEquals(null, encoded["note"])
        val decoded = RutisFormat.decode(Reading.serializer(), encoded)
        assertEquals(reading.copy(raw = null), decoded.copy(raw = null))
        assertContentEquals(reading.raw, decoded.raw)
    }

    @Test
    fun absentFieldsTakeTheirDefaultsOrNull() {
        val decoded = RutisFormat.decode(Reading.serializer(), mapOf("name" to "x"))
        assertEquals(Reading("x"), decoded)
        assertFailsWith<Exception> { RutisFormat.decode(Reading.serializer(), mapOf("count" to 1L)) }
    }

    @Test
    fun integersDecodeIntoTypesThatHoldThem() {
        assertEquals(5, RutisFormat.decode(RutisFormat.serializer<Int>(), JsonPrimitive(5)))
        assertEquals(1L shl 60, RutisFormat.decode(RutisFormat.serializer<Long>(), BigInteger.ONE.shiftLeft(60)))
        assertFailsWith<ValueException> { RutisFormat.decode(RutisFormat.serializer<Int>(), JsonPrimitive(1L shl 40)) }
        assertFailsWith<ValueException> { RutisFormat.decode(RutisFormat.serializer<Long>(), BigInteger.ONE.shiftLeft(70)) }
        assertFailsWith<ValueException> { RutisFormat.encode(RutisFormat.serializer<Double>(), Double.NaN) }
    }

    @Test
    fun anyIsDecodedNatively() {
        val value = Native.toNative(listOf(JsonPrimitive(1), JsonPrimitive(1.5), JsonPrimitive("a"), WireSet(listOf(JsonPrimitive("s"))), Decimal("2.0")))
        assertEquals(listOf(1L, 1.5, "a", setOf("s"), BigDecimal("2.0")), value)
        val map = Native.toNative(WireMap(listOf(JsonPrimitive(1) to JsonPrimitive("a"))))
        assertEquals(mapOf(1L to "a"), map)
        // And encoded by what it is.
        assertEquals(WireMap(listOf(1L to "a")), Native.fromNative(mapOf(1 to "a")))
        assertEquals(WireSet(listOf("a")), Native.fromNative(setOf("a")))
        assertEquals(BigInteger.ONE.shiftLeft(60), Native.fromNative(1L shl 60))
        assertTrue(Native.fromNative(Reading("x")) is Map<*, *>, "a @Serializable value by its serializer")
        assertFailsWith<ValueException> { Native.fromNative(Object()) }
    }
}
