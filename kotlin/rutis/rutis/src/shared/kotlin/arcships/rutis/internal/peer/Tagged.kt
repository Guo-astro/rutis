package arcships.rutis.internal.peer

import java.math.BigInteger
import java.time.Instant
import java.time.LocalDate
import java.time.LocalDateTime
import java.time.LocalTime
import java.time.ZoneOffset
import java.util.Base64
import java.util.UUID

// The tagged values beyond JSON data (#308, docs/design-value-types-2026-10-15.md)
// and their text on the wire (docs/design-protocol-plugins-2026-09-25.md §6.1):
//
//   tag       session value             text on the wire
//   instant   java.time.Instant         2026-10-15T03:22:15.123456789Z
//   datetime  java.time.LocalDateTime   2026-10-15T14:30:05.25
//   date      java.time.LocalDate       2026-10-15
//   time      java.time.LocalTime       14:30:05.25
//   bytes     ByteArray                 base64, padded
//   map       WireMap                   [[key, value], ...]
//   set       WireSet                   [element, ...]
//   bigint    java.math.BigInteger      decimal integer, at most 4300 digits
//   decimal   Decimal (its text)        decimal, no exponent, zeros kept
//   uuid      java.util.UUID            lowercase 8-4-4-4-12 hex
//
// Years are 0001–9999; fractions of a second have 0–9 digits.

/** The tags beyond JSON data; each is declared as the capability `value-<tag>`. */
val TAGS: List<String> = listOf("instant", "datetime", "date", "time", "bytes", "map", "set", "bigint", "decimal", "uuid")

/** The capabilities of the tagged values: this side takes them all. */
val VALUE_CAPABILITIES: List<String> = TAGS.map { "value-$it" }

/** The most digits a bigint or decimal may have: Python's default limit on integer text. */
const val MAX_DIGITS: Int = 4300

/** Largest integer every language holds exactly: ±(2^53−1). */
const val MAX_SAFE: Long = (1L shl 53) - 1

/** A map's entries in the sender's order (tag `map`). */
class WireMap(val entries: List<Pair<Any?, Any?>>) {
    override fun equals(other: Any?): Boolean = other is WireMap && other.entries == entries
    override fun hashCode(): Int = entries.hashCode()
    override fun toString(): String = "WireMap($entries)"
}

/** A set's elements in the sender's order (tag `set`). */
class WireSet(val elements: List<Any?>) {
    override fun equals(other: Any?): Boolean = other is WireSet && other.elements == elements
    override fun hashCode(): Int = elements.hashCode()
    override fun toString(): String = "WireSet($elements)"
}

/** A decimal kept as its text (tag `decimal`), so that it crosses unchanged. */
data class Decimal(val text: String) {
    override fun toString(): String = text
}

/** A value this side cannot send or take: only the call carrying it fails. */
open class ValueException(
    message: String,
    /** bad-value when it is of a part of a call (#677); null otherwise. */
    val code: String? = null,
    /** `{argument?, part, limit?}` with [code]. */
    val data: Map<String, Any?>? = null,
) : RuntimeException(message)

private fun quoted(text: String): String {
    val shown = if (text.codePointCount(0, text.length) <= 40) text else text.substring(0, text.offsetByCodePoints(0, 40)) + "…"
    return "\"" + shown.replace("\\", "\\\\").replace("\"", "\\\"") + "\""
}

private fun invalid(what: String, text: String, expected: String) =
    ValueException("invalid $what ${quoted(text)}: expected $expected")

/** The tag of a tagged value, null for any other. */
fun tagOf(value: Any?): String? = when (value) {
    is Instant -> "instant"
    is LocalDateTime -> "datetime"
    is LocalDate -> "date"
    is LocalTime -> "time"
    is ByteArray -> "bytes"
    is WireMap -> "map"
    is WireSet -> "set"
    is BigInteger -> "bigint"
    is Decimal -> "decimal"
    is UUID -> "uuid"
    else -> null
}

private class Cursor(val text: String) {
    var at = 0

    fun digits(count: Int): Int? {
        if (at + count > text.length) return null
        var value = 0
        for (i in at until at + count) {
            val c = text[i]
            if (c !in '0'..'9') return null
            value = value * 10 + (c - '0')
        }
        at += count
        return value
    }

    fun char(c: Char): Boolean {
        if (at < text.length && text[at] == c) {
            at++
            return true
        }
        return false
    }

    /** An optional `.` and 1–9 digits, in nanoseconds; null when malformed. */
    fun fraction(): Int? {
        if (!char('.')) return 0
        val start = at
        while (at < text.length && text[at] in '0'..'9') at++
        val digits = text.substring(start, at)
        if (digits.isEmpty() || digits.length > 9) return null
        return digits.padEnd(9, '0').toInt()
    }

    fun end() = at == text.length

    fun date(): LocalDate? {
        val year = digits(4) ?: return null
        if (!char('-')) return null
        val month = digits(2) ?: return null
        if (!char('-')) return null
        val day = digits(2) ?: return null
        if (year < 1 || month !in 1..12 || day < 1) return null
        if (day > LocalDate.of(year, month, 1).lengthOfMonth()) return null
        return LocalDate.of(year, month, day)
    }

    fun time(): LocalTime? {
        val hour = digits(2) ?: return null
        if (!char(':')) return null
        val minute = digits(2) ?: return null
        if (!char(':')) return null
        val second = digits(2) ?: return null
        val nanos = fraction() ?: return null
        if (hour > 23 || minute > 59 || second > 59) return null
        return LocalTime.of(hour, minute, second, nanos)
    }

    fun datetime(): LocalDateTime? {
        val date = date() ?: return null
        if (!char('T')) return null
        val time = time() ?: return null
        return LocalDateTime.of(date, time)
    }
}

private fun fraction(nanos: Int): String =
    if (nanos == 0) "" else "." + nanos.toString().padStart(9, '0').trimEnd('0')

fun formatDate(date: LocalDate): String {
    if (date.year !in 1..9999) throw ValueException("the date $date is outside years 0001–9999")
    return pad(date.year, 4) + "-" + pad(date.monthValue, 2) + "-" + pad(date.dayOfMonth, 2)
}

fun formatTime(time: LocalTime): String =
    pad(time.hour, 2) + ":" + pad(time.minute, 2) + ":" + pad(time.second, 2) + fraction(time.nano)

private fun pad(n: Int, width: Int): String = n.toString().padStart(width, '0')

fun formatDateTime(dt: LocalDateTime): String = formatDate(dt.toLocalDate()) + "T" + formatTime(dt.toLocalTime())

fun formatInstant(instant: Instant): String {
    val utc = LocalDateTime.ofEpochSecond(instant.epochSecond, instant.nano, ZoneOffset.UTC)
    if (utc.year !in 1..9999) throw ValueException("the time $instant is outside years 0001–9999")
    return formatDateTime(utc) + "Z"
}

fun parseDate(text: String): LocalDate {
    val c = Cursor(text)
    val date = c.date()
    if (date == null || !c.end()) throw invalid("date", text, "YYYY-MM-DD, years 0001–9999")
    return date
}

fun parseTime(text: String): LocalTime {
    val c = Cursor(text)
    val time = c.time()
    if (time == null || !c.end()) throw invalid("time", text, "HH:MM:SS[.fraction], 24-hour, 0–9 fraction digits")
    return time
}

fun parseDateTime(text: String): LocalDateTime {
    val c = Cursor(text)
    val dt = c.datetime()
    if (dt == null || !c.end()) throw invalid("datetime", text, "YYYY-MM-DDTHH:MM:SS[.fraction] without a time zone, years 0001–9999")
    return dt
}

fun parseInstant(text: String): Instant {
    val c = Cursor(text)
    val dt = c.datetime()
    if (dt == null || !c.char('Z') || !c.end()) {
        throw invalid("instant", text, "RFC 3339 in UTC, YYYY-MM-DDTHH:MM:SS[.fraction]Z, years 0001–9999")
    }
    return dt.toInstant(ZoneOffset.UTC)
}

fun formatBytes(bytes: ByteArray): String = Base64.getEncoder().encodeToString(bytes)

/** Padded base64 only: no whitespace, padding only at the end, unused bits zero. */
fun parseBytes(text: String): ByteArray {
    fun fail(): Nothing = throw ValueException("invalid bytes (${text.length} characters): expected base64 (RFC 4648 §4) with padding")
    if (text.length % 4 != 0) fail()
    for (c in text) {
        if (!(c in 'A'..'Z' || c in 'a'..'z' || c in '0'..'9' || c == '+' || c == '/' || c == '=')) fail()
    }
    val decoded = try {
        Base64.getDecoder().decode(text)
    } catch (_: IllegalArgumentException) {
        fail()
    }
    // Canonical: re-encoding gives the same text (unused bits zero).
    if (formatBytes(decoded) != text) fail()
    return decoded
}

/** Whether `text` is `-`?, then digits without a leading zero; the digit count, or -1. */
private fun integerDigits(text: String): Int {
    val rest = text.removePrefix("-")
    if (rest.isEmpty() || (rest.length > 1 && rest[0] == '0')) return -1
    if (rest.any { it !in '0'..'9' }) return -1
    return rest.length
}

fun formatBigInt(n: BigInteger): String {
    val text = n.toString()
    val digits = text.removePrefix("-").length
    if (digits > MAX_DIGITS) throw ValueException("a bigint of $digits digits is beyond $MAX_DIGITS, the most that cross")
    return text
}

fun parseBigInt(text: String): BigInteger {
    val digits = integerDigits(text)
    if (digits < 0 || digits > MAX_DIGITS || text == "-0") {
        throw invalid("bigint", text, "a decimal integer without leading zeros, at most 4300 digits")
    }
    return BigInteger(text)
}

/** Checks decimal text: `-?(0|[1-9][0-9]*)(\.[0-9]+)?`, at most 4300 digits. */
fun parseDecimal(text: String): Decimal {
    val dot = text.indexOf('.')
    val integer = if (dot < 0) text else text.substring(0, dot)
    var digits = integerDigits(integer)
    var ok = digits >= 0
    if (ok && dot >= 0) {
        val fraction = text.substring(dot + 1)
        ok = fraction.isNotEmpty() && fraction.all { it in '0'..'9' }
        digits += fraction.length
    }
    if (!ok || digits > MAX_DIGITS) {
        throw invalid("decimal", text, "decimal digits with an optional fraction, no exponent, at most 4300 digits")
    }
    return Decimal(text)
}

private fun uuidText(text: String): Boolean {
    if (text.length != 36) return false
    for ((i, c) in text.withIndex()) {
        when (i) {
            8, 13, 18, 23 -> if (c != '-') return false
            else -> if (!(c in '0'..'9' || c in 'a'..'f')) return false
        }
    }
    return true
}

/** A UUID as it crosses: lowercase `8-4-4-4-12` hex. */
fun parseUuid(text: String): UUID {
    if (!uuidText(text)) throw invalid("uuid", text, "lowercase hex 8-4-4-4-12")
    return UUID.fromString(text)
}

fun formatUuid(uuid: UUID): String = uuid.toString()

/** The text of a tagged value other than a map or a set. */
fun taggedText(value: Any): String = when (value) {
    is Instant -> formatInstant(value)
    is LocalDateTime -> formatDateTime(value)
    is LocalDate -> formatDate(value)
    is LocalTime -> formatTime(value)
    is ByteArray -> formatBytes(value)
    is BigInteger -> formatBigInt(value)
    is Decimal -> parseDecimal(value.text).text
    is UUID -> formatUuid(value)
    else -> throw ValueException("not a tagged value: ${value.javaClass.name}")
}

/** Reads the text of a tagged value other than a map or a set. */
fun parseTagged(tag: String, text: String): Any = when (tag) {
    "instant" -> parseInstant(text)
    "datetime" -> parseDateTime(text)
    "date" -> parseDate(text)
    "time" -> parseTime(text)
    "bytes" -> parseBytes(text)
    "bigint" -> parseBigInt(text)
    "decimal" -> parseDecimal(text)
    "uuid" -> parseUuid(text)
    else -> throw ValueException("invalid $tag")
}
