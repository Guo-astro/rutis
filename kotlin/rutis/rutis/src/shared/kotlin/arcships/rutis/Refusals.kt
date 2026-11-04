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
import java.io.IOException
import arcships.rutis.advanced.RemoteFunction
import arcships.rutis.advanced.RemoteObject
import arcships.rutis.generated.ServiceTable

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
    /** What the parameter takes, for `data.expected`. */
    val expected: String,
    /**
     * Why it is refused, in the words of whoever refused it: the decoder's
     * own sentence for a serialized type (`expected a string, got 3`), or
     * this runtime's for a value no decoder reads (design §4.5: rutis adds
     * only the position). Null when the argument is missing.
     */
    val why: String?,
) : ValueException(if (why == null) "$method: argument ${index + 1} is missing" else "$method: argument ${index + 1}: $why")

/** The refusal of `refusal` by the service (or object) `owner`. */
internal fun refused(refusal: ArgumentRefusal, owner: String, isObject: Boolean): RutisException {
    val label = "$owner.${refusal.method}"
    val data = linkedMapOf<String, Any?>(if (isObject) "object" to owner else "service" to owner, "method" to refusal.method,
        "argument" to refusal.index + 1, "expected" to refusal.expected)
    // The refusal's own words, with only the position and the call before
    // them (design §4.5).
    val message = if (refusal.why == null) {
        "$label: argument ${refusal.index + 1} is missing"
    } else {
        "$label: argument ${refusal.index + 1}: ${refusal.why}"
    }
    return RutisException(RutisException.BINDING_ERROR, message, "bad-argument", data)
}

/**
 * `<service> has no method <method> (methods: a, b)`, code no-method; for an
 * object, `<Type> has no method …` with `{object, method, methods}`.
 * `provider`: the row providing the service, when this side knows it, which
 * the message names and `data.provider` carries (#937).
 */
internal fun noMethod(owner: String?, method: String, methods: Collection<String>, isObject: Boolean = false,
    provider: String? = null): RutisException {
    val sorted = methods.sorted()
    val listed = if (sorted.isEmpty()) "no methods" else "methods: " + sorted.joinToString(", ")
    val data = linkedMapOf<String, Any?>(if (isObject) "object" to owner else "service" to owner, "method" to method, "methods" to sorted)
    var message = "${owner ?: "the object"} has no method $method ($listed)"
    if (provider != null) {
        message += "; it is provided by row $provider"
        data["provider"] = provider
    }
    return RutisException(RutisException.BINDING_ERROR, message, "no-method", data)
}

/**
 * A name another provider has: `<name> is already provided by <row>: rename
 * one of them`, and `: rename it` for a name the host holds itself (#937) --
 * the same sentence rutis and every other runtime give.
 */
internal fun takenMessage(name: String, holder: String): String =
    if (holder == "the host") "$name is already provided by the host: rename it"
    else "$name is already provided by $holder: rename one of them"

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

    /** `a Cache`, `an Item`. */
    fun article(name: String): String = (if (name.firstOrNull()?.lowercaseChar()?.let { it in "aeiou" } == true) "an " else "a ") + name

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
        is RemoteObject, is arcships.rutis.internal.peer.ExportedObject -> "an object"
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
 * A service a plugin uses without declaring it in `inject` (#704), or reads
 * with `get` without declaring it in `optional` or `inject`: code
 * not-declared with `{service}`. Still an IllegalArgumentException.
 */
class NotDeclaredException private constructor(service: String, message: String) : IllegalArgumentException(message) {
    internal constructor(service: String) :
        this(service, "service $service is not available to this plugin, which does not declare it in inject: add '$service' to its inject")

    /** `not-declared`. */
    val code: String = "not-declared"

    /** `{service}`. */
    val data: Map<String, Any?> = mapOf("service" to service)

    internal companion object {
        /** `get` of a service the plugin declares nowhere: declare it in `optional`. */
        fun optional(service: String) = NotDeclaredException(
            service,
            "service $service is not available to this plugin, which does not declare it: add '$service' to its optional (or to its inject, if it cannot run without it)",
        )
    }
}

/** A declared service nothing provides now: code unavailable with `{service}`. Still an IllegalStateException. */
class UnavailableException internal constructor(service: String) : IllegalStateException("service $service is not available") {
    /** `unavailable`. */
    val code: String = "unavailable"

    /** `{service}`. */
    val data: Map<String, Any?> = mapOf("service" to service)
}

/**
 * The host went (#677): what calls fail with then, `the host disconnected`
 * (with why, when known), code peer-gone with `{reason}`. It crosses as a
 * BindingError.
 */
class HostGoneException internal constructor(why: String?, cause: Throwable? = null, message: String? = null) :
    IOException(message ?: if (why == null) "the host disconnected" else "the host disconnected ($why)", cause) {
    /** `peer-gone`. */
    val code: String = "peer-gone"

    /** `{reason}`: `disconnected`, `closed`, or what failed. */
    val data: Map<String, Any?> = mapOf("reason" to (why ?: "disconnected"))

    internal companion object {
        /** This side closed the connection. */
        fun closed() = HostGoneException("closed", message = "the connection to the host is closed")
    }
}
