package arcships.rutis

import arcships.rutis.internal.peer.Exported
import arcships.rutis.internal.peer.ExportedObject
import arcships.rutis.internal.peer.RemoteFuture
import arcships.rutis.internal.peer.Undefined
import arcships.rutis.internal.peer.ValueException
import arcships.rutis.internal.peer.describe
import kotlinx.serialization.descriptors.PolymorphicKind
import kotlinx.serialization.descriptors.PrimitiveKind
import kotlinx.serialization.descriptors.SerialDescriptor
import kotlinx.serialization.descriptors.SerialKind
import kotlinx.serialization.descriptors.StructureKind
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive

// The errors this runtime makes when it refuses a call (#677,
// docs/design-diagnostics-2026-10-26.md §4.3): a BindingError with a code
// for programs and data with the details; the text only uses names the
// developer wrote. The same in every runtime.

/**
 * An argument generated code could not decode: what the caller of the
 * method table turns into the refusal naming the service ([refused]).
 */
internal class ArgumentRefusal(
    val method: String,
    val index: Int,
    val expected: String,
    /** The argument as shown; null when it is missing. */
    val actual: String?,
) : ValueException(if (actual == null) "$method: argument ${index + 1} is missing" else "$method: argument ${index + 1} should be $expected, got $actual")

/** The refusal of `refusal` by the service (or object) `owner`. */
internal fun refused(refusal: ArgumentRefusal, owner: String, isObject: Boolean): RutisException {
    val label = "$owner.${refusal.method}"
    val data = linkedMapOf<String, Any?>(if (isObject) "object" to owner else "service" to owner, "method" to refusal.method,
        "argument" to refusal.index + 1, "expected" to refusal.expected)
    val message = if (refusal.actual == null) {
        "$label: argument ${refusal.index + 1} is missing"
    } else {
        "$label: argument ${refusal.index + 1} should be ${refusal.expected}, got ${refusal.actual}"
    }
    return RutisException(RutisException.BINDING_ERROR, message, "bad-argument", data)
}

/**
 * `<service> has no method <method> (methods: a, b)`, code no-method; for an
 * object, `<Type> has no method …` with `{object, method, methods}`.
 */
internal fun noMethod(owner: String?, method: String, methods: Collection<String>, isObject: Boolean = false): RutisException {
    val sorted = methods.sorted()
    val listed = if (sorted.isEmpty()) "no methods" else "methods: " + sorted.joinToString(", ")
    val data = linkedMapOf<String, Any?>(if (isObject) "object" to owner else "service" to owner, "method" to method, "methods" to sorted)
    return RutisException(RutisException.BINDING_ERROR, "${owner ?: "the object"} has no method $method ($listed)", "no-method", data)
}

/**
 * Calls `method` of `target` through its method table, as the service (or
 * object) `owner`: a method it lacks and an argument it cannot take are
 * refused naming `owner`.
 */
internal suspend fun ServiceTable<Any>.serve(target: Any, method: String, args: List<Any?>, owner: String, isObject: Boolean = false): Any? {
    if (method !in methods) throw noMethod(owner, method, methods, isObject)
    try {
        return invoke(target, method, args)
    } catch (e: ArgumentRefusal) {
        if (e.method != method) throw e
        throw refused(e, owner, isObject)
    }
}

internal object Refusals {
    /** What a parameter of `descriptor` takes, in words every language shares: `a string`, `an integer`, `a Forecast`. */
    fun expected(descriptor: SerialDescriptor): String = when (val kind: SerialKind = descriptor.kind) {
        PrimitiveKind.BOOLEAN -> "a boolean"
        PrimitiveKind.BYTE, PrimitiveKind.SHORT, PrimitiveKind.INT, PrimitiveKind.LONG -> "an integer"
        PrimitiveKind.FLOAT, PrimitiveKind.DOUBLE -> "a number"
        PrimitiveKind.CHAR, PrimitiveKind.STRING -> when (simpleName(descriptor)) {
            "String", "Char" -> "a string"
            else -> named(descriptor) ?: "a string"
        }
        StructureKind.LIST -> when (simpleName(descriptor)) {
            "ByteArray" -> "bytes"
            "LinkedHashSet", "HashSet", "Set" -> "a set"
            else -> "a list"
        }
        StructureKind.MAP -> if (descriptor.elementsCount > 0 && descriptor.getElementDescriptor(0).kind == PrimitiveKind.STRING) "an object" else "a map"
        is PolymorphicKind, StructureKind.CLASS, StructureKind.OBJECT -> named(descriptor) ?: "an object"
        else -> named(descriptor) ?: "a value"
    }

    private fun simpleName(descriptor: SerialDescriptor): String =
        descriptor.serialName.removeSuffix("?").substringAfterLast('.')

    private fun named(descriptor: SerialDescriptor): String? {
        val name = simpleName(descriptor)
        if (name.isEmpty()) return null
        return (if (name.first().lowercaseChar() in "aeiou") "an " else "a ") + name
    }

    /** The argument as a refusal shows it: a short value (`"x"`, `1.5`), or what it is (`a list`, `an object`). */
    fun shown(value: Any?): String = when (value) {
        null, JsonNull -> "null"
        Undefined -> "nothing"
        is JsonPrimitive -> abbreviated(value.toString())
        is String -> abbreviated(JsonPrimitive(value).toString())
        is Boolean, is Int, is Long, is Double, is Float, is Short, is Byte -> value.toString()
        is JsonArray, is List<*> -> "a list"
        is JsonObject, is Map<*, *> -> "an object"
        is Exported, is RemoteFunction -> "a function"
        is ExportedObject -> "an object"
        is RemoteFuture -> "an asynchronous result"
        else -> describe(value)
    }

    /** Why it failed, when the value is of the right kind but not this value (an integer too large, a field of the wrong type). */
    fun reason(value: Any?, error: Throwable): String? {
        val nested = value is JsonObject || value is Map<*, *> || value is JsonArray || value is List<*>
        val message = (error as? ValueException)?.message ?: return null
        return if (nested || "does not fit" in message || "is not one of" in message) message else null
    }

    private fun abbreviated(text: String): String = if (text.length > 60) text.take(57) + "..." else text
}

/**
 * A service a plugin uses without declaring it in `inject` (#704): code
 * not-declared with `{service}`. Still an IllegalArgumentException.
 */
class NotDeclaredException internal constructor(service: String) :
    IllegalArgumentException("service $service is not available to this plugin, which does not declare it in inject: add '$service' to its inject") {
    /** `not-declared`. */
    val code: String = "not-declared"

    /** `{service}`. */
    val data: Map<String, Any?> = mapOf("service" to service)
}

/** A declared service nothing provides now: code unavailable with `{service}`. Still an IllegalStateException. */
class UnavailableException internal constructor(service: String) : IllegalStateException("service $service is not available") {
    /** `unavailable`. */
    val code: String = "unavailable"

    /** `{service}`. */
    val data: Map<String, Any?> = mapOf("service" to service)
}
