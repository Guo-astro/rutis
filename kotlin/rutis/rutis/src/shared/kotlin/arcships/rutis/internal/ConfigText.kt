package arcships.rutis.internal

import kotlinx.serialization.descriptors.PolymorphicKind
import kotlinx.serialization.descriptors.PrimitiveKind
import kotlinx.serialization.descriptors.SerialDescriptor
import kotlinx.serialization.descriptors.SerialKind
import kotlinx.serialization.descriptors.StructureKind
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.booleanOrNull
import kotlinx.serialization.json.doubleOrNull
import kotlinx.serialization.json.longOrNull

/**
 * What is wrong with an application's own plugin's config that its type
 * could not decode, worded as rutis's check (`rutis::config::check`) words
 * it: `is required`, `should be a number, got "x"`.
 */
internal object ConfigText {
    /** The problem at `path` in `value` that `error` was, or null when it says nothing a key could. */
    fun problem(root: SerialDescriptor, value: Any?, path: List<Any>, error: Throwable): String? {
        if (error is kotlinx.serialization.MissingFieldException) return "is required"
        val descriptor = at(root, path) ?: return null
        val expected = expected(descriptor) ?: return null
        val got = find(value, path) ?: return null
        return "should be $expected, got ${valueText(got.value)}"
    }

    /** The descriptor of the element at `path`. */
    private fun at(root: SerialDescriptor, path: List<Any>): SerialDescriptor? {
        var current = root
        for (segment in path) {
            current = when (current.kind) {
                StructureKind.CLASS, StructureKind.OBJECT -> {
                    val index = current.getElementIndex(segment as? String ?: return null)
                    if (index < 0) return null
                    current.getElementDescriptor(index)
                }
                StructureKind.LIST -> current.getElementDescriptor(0)
                StructureKind.MAP -> current.getElementDescriptor(1)
                else -> return null
            }
        }
        return current
    }

    private fun expected(d: SerialDescriptor): String? {
        val type: String = when (val kind: SerialKind = d.kind) {
            PrimitiveKind.STRING, PrimitiveKind.CHAR -> "a string"
            PrimitiveKind.BOOLEAN -> "a boolean"
            PrimitiveKind.INT, PrimitiveKind.LONG, PrimitiveKind.SHORT, PrimitiveKind.BYTE -> "an integer"
            PrimitiveKind.FLOAT, PrimitiveKind.DOUBLE -> "a number"
            StructureKind.CLASS, StructureKind.OBJECT, StructureKind.MAP -> "an object"
            StructureKind.LIST -> "an array"
            is PolymorphicKind -> return null
            else -> if (kind.toString() == "ENUM") {
                val names = (0 until d.elementsCount).map { valueText(d.getElementName(it)) }
                return when (names.size) {
                    1 -> names[0]
                    2 -> "${names[0]} or ${names[1]}"
                    else -> "one of ${names.joinToString(", ")}"
                }
            } else {
                return null
            }
        }
        return if (d.isNullable) "$type or null" else type
    }

    private class Found(val value: Any?)

    private fun find(value: Any?, path: List<Any>): Found? {
        var current = value
        for (segment in path) {
            current = when {
                segment is String && current is JsonObject -> current[segment] ?: return null
                segment is String && current is Map<*, *> -> if (current.containsKey(segment)) current[segment] else return null
                segment is Int && current is JsonArray -> current.getOrNull(segment) ?: return null
                segment is Int && current is List<*> -> if (segment < current.size) current[segment] else return null
                else -> return null
            }
        }
        return Found(current)
    }

    /** As `rutis::config::value_text`: JSON for strings (cut at 40 with …), numbers, booleans and null; `an object`, `an array of N`. */
    fun valueText(value: Any?): String = when (value) {
        null, JsonNull, Undefined_ -> "null"
        is JsonPrimitive -> when {
            value.isString -> quoted(value.content)
            value.booleanOrNull != null -> value.content
            value.longOrNull != null -> value.content
            else -> value.doubleOrNull?.let(::numberText) ?: value.content
        }
        is String -> quoted(value)
        is Boolean -> value.toString()
        is Double -> numberText(value)
        is Float -> numberText(value.toDouble())
        is Number -> value.toString()
        is JsonArray -> "an array of ${value.size}"
        is List<*> -> "an array of ${value.size}"
        is JsonElement, is Map<*, *> -> "an object"
        else -> "an object"
    }

    private val Undefined_ = arcships.rutis.internal.peer.Undefined

    private fun numberText(f: Double): String =
        if (f == Math.rint(f) && Math.abs(f) <= 9007199254740992.0) f.toLong().toString() else f.toString()

    private fun quoted(s: String): String {
        val count = s.codePointCount(0, s.length)
        val shown = if (count <= 40) s else s.substring(0, s.offsetByCodePoints(0, 40)) + "…"
        return JsonPrimitive(shown).toString()
    }
}
