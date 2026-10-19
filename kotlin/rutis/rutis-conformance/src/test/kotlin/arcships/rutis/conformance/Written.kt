package arcships.rutis.conformance

import arcships.rutis.RutisException
import arcships.rutis.internal.Native
import arcships.rutis.internal.peer.Decimal
import arcships.rutis.internal.peer.Undefined
import arcships.rutis.internal.peer.WireMap
import arcships.rutis.internal.peer.WireSet
import arcships.rutis.internal.peer.formatBigInt
import arcships.rutis.internal.peer.formatBytes
import arcships.rutis.internal.peer.formatDate
import arcships.rutis.internal.peer.formatDateTime
import arcships.rutis.internal.peer.formatInstant
import arcships.rutis.internal.peer.formatTime
import arcships.rutis.internal.peer.parseTagged
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.jsonArray
import kotlinx.serialization.json.jsonPrimitive
import java.io.File
import java.math.BigInteger
import java.time.Instant
import java.time.LocalDate
import java.time.LocalDateTime
import java.time.LocalTime
import java.util.UUID

/** An expectation file of crates/rutis-bridge/src/runtime/testing. */
fun expectations(name: String): JsonObject {
    val repo = System.getProperty("rutis.repo") ?: "../.."
    return Json.parseToJsonElement(File(repo, "crates/rutis-bridge/src/runtime/testing/$name").readText()) as JsonObject
}

/** A session value as the expectations write it: data as JSON, a tagged value as {"$<tag>": <its wire text>}. */
fun written(value: Any?): JsonElement {
    fun tagged(tag: String, inner: JsonElement) = JsonObject(mapOf("$$tag" to inner))
    return when (value) {
        null, Undefined -> JsonNull
        is JsonElement -> normalized(value)
        is List<*> -> JsonArray(value.map(::written))
        is Map<*, *> -> JsonObject(value.entries.associate { (k, v) -> k as String to written(v) })
        is WireMap -> tagged("map", JsonArray(value.entries.map { (k, v) -> JsonArray(listOf(written(k), written(v))) }))
        is WireSet -> tagged("set", JsonArray(value.elements.map(::written)))
        is Instant -> tagged("instant", JsonPrimitive(formatInstant(value)))
        is LocalDateTime -> tagged("datetime", JsonPrimitive(formatDateTime(value)))
        is LocalDate -> tagged("date", JsonPrimitive(formatDate(value)))
        is LocalTime -> tagged("time", JsonPrimitive(formatTime(value)))
        is ByteArray -> tagged("bytes", JsonPrimitive(formatBytes(value)))
        is BigInteger -> tagged("bigint", JsonPrimitive(formatBigInt(value)))
        is Decimal -> tagged("decimal", JsonPrimitive(value.text))
        is UUID -> tagged("uuid", JsonPrimitive(value.toString()))
        is String -> JsonPrimitive(value)
        is Boolean -> JsonPrimitive(value)
        is Long -> JsonPrimitive(value)
        is Double -> normalized(JsonPrimitive(value))
        else -> throw AssertionError("not data: $value (${value.javaClass})")
    }
}

/** JSON numbers compared by value: `1.0` is `1`. */
fun normalized(value: JsonElement): JsonElement = when (value) {
    is JsonArray -> JsonArray(value.map(::normalized))
    is JsonObject -> JsonObject(value.mapValues { normalized(it.value) })
    is JsonPrimitive -> if (!value.isString && value != JsonNull && value.content != "true" && value.content != "false") {
        val number = value.content.toBigDecimal()
        val whole = try {
            number.toBigIntegerExact()
        } catch (_: ArithmeticException) {
            null
        }
        if (whole != null) JsonPrimitive(whole) else JsonPrimitive(number.toDouble())
    } else {
        value
    }
}

/** An argument as the expectations write it: data, or a callback ({"$callback": ...}). */
fun argument(raw: JsonElement): Any? {
    val callback = (raw as? JsonObject)?.get("\$callback") ?: return raw
    if (callback is JsonPrimitive && callback.content == "upper") {
        return Native.fromNative({ day: Any? -> (day as String).uppercase() })
    }
    val thrown = (callback as JsonObject)["throws"] as JsonObject
    return Native.fromNative({ _: Any? ->
        throw RutisException(thrown["name"]!!.jsonPrimitive.content, thrown["message"]!!.jsonPrimitive.content)
    })
}

/**
 * A value as the expectations write it, as the session carries it: tagged
 * values from {"$<tag>": …}, a callback from {"$callback": "upper"}, lists
 * of them as lists, other data as it is.
 */
fun fromWritten(raw: JsonElement): Any? {
    if (raw is JsonObject && raw.size == 1) {
        val (key, inner) = raw.entries.first()
        if (key.startsWith("$")) {
            return when (val tag = key.substring(1)) {
                "callback" -> Native.fromNative({ text: Any? -> (text as String).uppercase() })
                "map" -> WireMap(inner.jsonArray.map { pair -> fromWritten(pair.jsonArray[0]) to fromWritten(pair.jsonArray[1]) })
                "set" -> WireSet(inner.jsonArray.map(::fromWritten))
                else -> parseTagged(tag, inner.jsonPrimitive.content)
            }
        }
    }
    if (raw is JsonArray) return raw.map(::fromWritten)
    return raw
}
