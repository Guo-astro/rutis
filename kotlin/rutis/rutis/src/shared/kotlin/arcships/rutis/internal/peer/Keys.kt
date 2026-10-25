package arcships.rutis.internal.peer

import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import java.math.BigDecimal
import java.math.BigInteger
import java.time.Instant
import java.time.LocalDate
import java.time.LocalDateTime
import java.time.LocalTime
import java.util.UUID

/** What a session value is, for errors: "null", "a string", "an instant", ... */
fun describe(value: Any?): String = when (value) {
    null, JsonNull -> "null"
    Undefined -> "undefined"
    is JsonPrimitive -> when {
        value.isString -> "a string"
        value.content == "true" || value.content == "false" -> "a boolean"
        else -> "a number"
    }
    is JsonArray -> "an array"
    is JsonObject -> "an object"
    is String, is Char -> "a string"
    is Boolean -> "a boolean"
    is Number -> if (value is BigInteger) "a bigint" else "a number"
    is List<*> -> "an array"
    is Map<*, *>, is Throwable -> "an object"
    is Signal -> "a signal"
    is ByteArray -> "bytes"
    is Instant -> "an instant"
    is Exported, is ExportedObject, is arcships.rutis.RemoteFunction, is RemoteFuture -> "a reference"
    is arcships.rutis.Plugin, is RemotePlugin -> "a plugin definition"
    else -> when (val tag = tagOf(value)) {
        null -> "a ${value.javaClass.name}"
        else -> "a $tag"
    }
}

private val maxSafe = BigInteger.valueOf(MAX_SAFE)

private fun integerKey(n: BigInteger, what: String): String {
    if (n.abs() > maxSafe) throw ValueException("a $what cannot be the integer $n, beyond ±(2^53-1): send it as a bigint")
    return "n$n"
}

private fun floatKey(f: Double): String {
    if (f == Math.floor(f) && !f.isInfinite() && Math.abs(f) < 1e38) {
        return "n" + BigDecimal(f).toBigInteger()
    }
    return "f" + java.lang.Long.toHexString(java.lang.Double.doubleToLongBits(f))
}

/** The key of a JSON number: an integer literal by its value, any other by its float value. */
private fun numberKey(text: String, what: String): String {
    if (!text.contains('.') && !text.contains('e') && !text.contains('E')) {
        text.toBigIntegerOrNull()?.let { return integerKey(it, what) }
    }
    val f = text.toDoubleOrNull() ?: throw ValueException("a $what cannot be $text")
    return floatKey(f)
}

/**
 * A map key or set element as the languages compare it: numbers by value
 * (`1` and `1.0` are one key, as are `0` and `-0`), the rest by kind and
 * text. `what` is "map key" or "set element".
 */
fun keyOf(value: Any?, what: String): String = when (value) {
    is String -> "s$value"
    is Char -> "s$value"
    is Boolean -> "b$value"
    is Byte, is Short, is Int, is Long -> integerKey(BigInteger.valueOf((value as Number).toLong()), what)
    is Float, is Double -> {
        val f = (value as Number).toDouble()
        if (f.isNaN() || f.isInfinite()) throw ValueException("a $what cannot be ${describe(value)} that is not finite")
        floatKey(f)
    }
    is JsonPrimitive -> when {
        value == JsonNull -> notKey(value, what)
        value.isString -> "s" + value.content
        value.content == "true" || value.content == "false" -> "b" + value.content
        else -> numberKey(value.content, what)
    }
    is BigInteger -> "B$value"
    is LocalDateTime -> "datetime" + formatDateTime(value)
    is LocalDate -> "date" + formatDate(value)
    is LocalTime -> "time" + formatTime(value)
    is Decimal -> "D" + value.text
    is UUID -> "U$value"
    else -> notKey(value, what)
}

private fun notKey(value: Any?, what: String): Nothing = throw ValueException(
    "a $what cannot be ${describe(value)}: only strings, finite numbers, booleans, bigint, datetime, date, time, " +
        "decimal and uuid values can (an instant, bytes, null, an array, an object, a map, a set or a reference cannot)",
)

/** A key as an error shows it. */
private fun shownKey(value: Any?): String {
    val text = when (value) {
        is String -> "\"" + value + "\""
        is JsonElement -> value.toString()
        is Instant, is ByteArray, is WireMap, is WireSet -> describe(value)
        else -> value.toString()
    }
    return if (text.codePointCount(0, text.length) > 40) text.substring(0, text.offsetByCodePoints(0, 40)) + "…" else text
}

/**
 * Checks the keys of a map (`what` "map key") or the elements of a set
 * ("set element"): each of a kind every language takes as a key, and no
 * two equal, since a language that merges them would lose an entry.
 */
fun checkKeys(keys: List<Any?>, what: String) {
    val seen = HashSet<String>(keys.size * 2)
    for (key in keys) {
        val identity = keyOf(key, what)
        if (!seen.add(identity)) {
            throw ValueException("duplicate $what ${shownKey(key)}: a map or set whose keys collide would lose entries")
        }
    }
}
