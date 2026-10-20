package arcships.rutis.internal

import kotlinx.serialization.descriptors.PolymorphicKind
import kotlinx.serialization.descriptors.PrimitiveKind
import kotlinx.serialization.descriptors.SerialDescriptor
import kotlinx.serialization.descriptors.SerialKind
import kotlinx.serialization.descriptors.StructureKind
import kotlinx.serialization.descriptors.getContextualDescriptor
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put

/**
 * The JSON Schema of a configuration type, derived from its serial
 * descriptor (generated at compile time, no reflection): primitive types,
 * lists, maps, nested classes, nullable fields, fields with defaults (not
 * required) and enums.
 */
object Schema {
    fun of(descriptor: SerialDescriptor): JsonElement = of(descriptor, HashSet())

    private fun of(descriptor: SerialDescriptor, visiting: MutableSet<String>): JsonElement {
        val base = when (descriptor.serialName.removeSuffix("?")) {
            "java.time.Instant", "kotlin.time.Instant", "java.time.LocalDateTime" -> stringFormat("date-time")
            "java.time.LocalDate" -> stringFormat("date")
            "java.time.LocalTime" -> stringFormat("time")
            "java.util.UUID", "kotlin.uuid.Uuid" -> stringFormat("uuid")
            "java.math.BigInteger" -> type("integer")
            "java.math.BigDecimal" -> type("number")
            "arcships.rutis.Any" -> JsonObject(emptyMap())
            else -> structural(descriptor, visiting)
        }
        if (!descriptor.isNullable || base !is JsonObject || base.isEmpty()) return base
        val kind = base["type"]
        return if (kind is JsonPrimitive) {
            JsonObject(base + ("type" to JsonArray(listOf(kind, JsonPrimitive("null")))))
        } else {
            buildJsonObject { put("anyOf", JsonArray(listOf(base, type("null")))) }
        }
    }

    private fun type(name: String): JsonObject = buildJsonObject { put("type", name) }

    private fun stringFormat(format: String): JsonObject = buildJsonObject {
        put("type", "string")
        put("format", format)
    }

    private fun structural(descriptor: SerialDescriptor, visiting: MutableSet<String>): JsonElement = when (descriptor.kind) {
        PrimitiveKind.BOOLEAN -> type("boolean")
        PrimitiveKind.BYTE, PrimitiveKind.SHORT, PrimitiveKind.INT, PrimitiveKind.LONG -> type("integer")
        PrimitiveKind.FLOAT, PrimitiveKind.DOUBLE -> type("number")
        PrimitiveKind.CHAR, PrimitiveKind.STRING -> type("string")
        SerialKind.ENUM -> buildJsonObject {
            put("type", "string")
            put("enum", JsonArray((0 until descriptor.elementsCount).map { JsonPrimitive(descriptor.getElementName(it)) }))
        }
        StructureKind.LIST -> buildJsonObject {
            put("type", "array")
            put("items", of(descriptor.getElementDescriptor(0), visiting))
            if (descriptor.serialName.removeSuffix("?").let { it.endsWith("HashSet") }) put("uniqueItems", true)
        }
        StructureKind.MAP -> buildJsonObject {
            put("type", "object")
            put("additionalProperties", of(descriptor.getElementDescriptor(1), visiting))
        }
        StructureKind.CLASS, StructureKind.OBJECT -> {
            val name = descriptor.serialName.removeSuffix("?")
            if (!visiting.add(name)) {
                // A recursive type: its nested occurrences take any object.
                type("object")
            } else {
                val required = ArrayList<JsonPrimitive>()
                val properties = LinkedHashMap<String, JsonElement>()
                for (i in 0 until descriptor.elementsCount) {
                    val field = descriptor.getElementName(i)
                    properties[field] = of(descriptor.getElementDescriptor(i), visiting)
                    if (!descriptor.isElementOptional(i) && !descriptor.getElementDescriptor(i).isNullable) required.add(JsonPrimitive(field))
                }
                visiting.remove(name)
                buildJsonObject {
                    put("type", "object")
                    put("properties", JsonObject(properties))
                    if (required.isNotEmpty()) put("required", JsonArray(required))
                }
            }
        }
        SerialKind.CONTEXTUAL -> arcships.rutis.RutisFormat.serializersModule.getContextualDescriptor(descriptor)
            ?.let { of(it, visiting) } ?: JsonObject(emptyMap())
        is PolymorphicKind -> JsonObject(emptyMap())
        else -> JsonObject(emptyMap())
    }
}
