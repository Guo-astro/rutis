package arcships.rutis.internal

import arcships.rutis.RemoteFunction
import arcships.rutis.RutisFormat
import arcships.rutis.ServiceObject
import arcships.rutis.Tables
import arcships.rutis.internal.peer.Decimal
import arcships.rutis.internal.peer.Exported
import arcships.rutis.internal.peer.ExportedObject
import arcships.rutis.internal.peer.JsonValues
import arcships.rutis.internal.peer.RemoteFuture
import arcships.rutis.internal.peer.RemotePlugin
import arcships.rutis.internal.peer.Signal
import arcships.rutis.internal.peer.Undefined
import arcships.rutis.internal.peer.ValueException
import arcships.rutis.internal.peer.WireMap
import arcships.rutis.internal.peer.WireSet
import kotlinx.coroutines.suspendCancellableCoroutine
import kotlinx.serialization.InternalSerializationApi
import kotlinx.serialization.KSerializer
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.serializerOrNull
import java.math.BigDecimal
import java.math.BigInteger
import java.time.Instant
import java.time.LocalDate
import java.time.LocalDateTime
import java.time.LocalTime
import java.time.OffsetDateTime
import java.time.ZonedDateTime
import java.util.UUID
import kotlin.coroutines.Continuation
import kotlin.coroutines.intrinsics.COROUTINE_SUSPENDED
import kotlin.coroutines.intrinsics.suspendCoroutineUninterceptedOrReturn
import kotlin.time.toJavaInstant
import kotlin.uuid.toJavaUuid

/**
 * Values whose type is not known at compile time (`Any?`): what a session
 * value is as a native Kotlin value, and what a Kotlin value is as a session
 * value, by what it is at run time (design §3.3).
 */
object Native {
    /**
     * A session value as a native value: integers as Long, other numbers as
     * Double, objects as `Map<String, Any?>`, lists as `List<Any?>`, a `map`
     * as `Map<Any?, Any?>`, a `set` as `Set<Any?>`, a `bigint` as BigInteger,
     * a `decimal` as BigDecimal, points in time as `java.time` types, a
     * function as [RemoteFunction].
     */
    fun toNative(value: Any?): Any? = when (value) {
        null, Undefined, JsonNull -> null
        is JsonPrimitive -> when {
            value.isString -> value.content
            value.content == "true" -> true
            value.content == "false" -> false
            else -> JsonValues.number(value.content)
        }
        is JsonArray -> value.map(::toNative)
        is JsonObject -> LinkedHashMap<String, Any?>().apply { for ((k, v) in value) put(k, toNative(v)) }
        is List<*> -> value.map(::toNative)
        is Map<*, *> -> LinkedHashMap<Any?, Any?>().apply { for ((k, v) in value) put(k, toNative(v)) }
        is WireMap -> LinkedHashMap<Any?, Any?>().apply { for ((k, v) in value.entries) put(toNative(k), toNative(v)) }
        is WireSet -> LinkedHashSet<Any?>().apply { for (e in value.elements) add(toNative(e)) }
        is Decimal -> BigDecimal(value.text)
        is Exported -> value.original
        is ExportedObject -> value.original
        // A plugin definition of this process, handed to a test double.
        is RemotePlugin -> value.local ?: value
        else -> value
    }

    /** A Kotlin value as a session value; one that cannot cross fails, naming its type. */
    fun fromNative(value: Any?): Any? = fromNative(value, 0)

    private fun fromNative(value: Any?, depth: Int): Any? {
        if (depth > 200) throw ValueException("value nested deeper than 64 levels, or cyclic")
        return when (value) {
            null, Undefined -> value
            is String, is Boolean, is JsonElement, is WireMap, is WireSet, is Decimal,
            is RemoteFunction, is RemoteFuture, is Exported, is ExportedObject, is Signal, is arcships.rutis.Plugin, is RemotePlugin,
            is Instant, is LocalDateTime, is LocalDate, is LocalTime, is ByteArray, is BigInteger, is UUID,
            -> value
            is Char -> value.toString()
            is Int -> value.toLong()
            is Long -> if (value > arcships.rutis.internal.peer.MAX_SAFE || value < -arcships.rutis.internal.peer.MAX_SAFE) BigInteger.valueOf(value) else value
            is Short -> value.toLong()
            is Byte -> value.toLong()
            is Double -> value
            is Float -> value.toDouble()
            is UInt -> value.toLong()
            is UShort -> value.toLong()
            is UByte -> value.toLong()
            is ULong -> if (value > Long.MAX_VALUE.toULong()) BigInteger(value.toString()) else value.toLong()
            is BigDecimal -> Decimal(value.toPlainString())
            is OffsetDateTime -> value.toInstant()
            is ZonedDateTime -> value.toInstant()
            is kotlin.time.Instant -> value.toJavaInstant()
            is kotlin.uuid.Uuid -> value.toJavaUuid()
            is Enum<*> -> value.name
            is Throwable -> value
            is Set<*> -> WireSet(value.map { fromNative(it, depth + 1) })
            is Map<*, *> -> if (value.keys.all { it is String }) {
                LinkedHashMap<String, Any?>().apply { for ((k, v) in value) put(k as String, fromNative(v, depth + 1)) }
            } else {
                WireMap(value.entries.map { (k, v) -> fromNative(k, depth + 1) to fromNative(v, depth + 1) })
            }
            is Array<*> -> value.map { fromNative(it, depth + 1) }
            is IntArray -> value.map { it.toLong() }
            is LongArray -> value.toList()
            is ShortArray -> value.map { it.toLong() }
            is DoubleArray -> value.toList()
            is FloatArray -> value.map { it.toDouble() }
            is BooleanArray -> value.toList()
            is CharArray -> value.map { it.toString() }
            is Iterable<*> -> value.map { fromNative(it, depth + 1) }
            is Pair<*, *> -> listOf(fromNative(value.first, depth + 1), fromNative(value.second, depth + 1))
            else -> objectOf(value)
        }
    }

    @OptIn(InternalSerializationApi::class)
    private fun objectOf(value: Any): Any? {
        Tables.serviceTable(value.javaClass)?.let { return ServiceObject(value, it) }
        if (value is Function<*>) return exportFunction(value)
        @Suppress("UNCHECKED_CAST")
        val serializer = value::class.serializerOrNull() as KSerializer<Any>?
            ?: throw ValueException("a ${value.javaClass.name} cannot cross: it is neither @Serializable nor a @RutisService")
        return RutisFormat.encode(serializer, value)
    }

    private val suspendMarker: Class<*>? = try {
        Class.forName("kotlin.coroutines.jvm.internal.SuspendFunction")
    } catch (_: ClassNotFoundException) {
        null
    }

    /** The arity of a function: its class says (a suspend lambda's counts its continuation), or the FunctionN it is. */
    private fun arityOf(function: Function<*>): Int = (function as? kotlin.jvm.internal.FunctionBase<*>)?.arity
        ?: when (function) {
            is Function0<*> -> 0
            is Function1<*, *> -> 1
            is Function2<*, *, *> -> 2
            is Function3<*, *, *, *> -> 3
            is Function4<*, *, *, *, *> -> 4
            is Function5<*, *, *, *, *, *> -> 5
            is Function6<*, *, *, *, *, *, *> -> 6
            else -> throw ValueException("a ${function.javaClass.name} cannot cross: its arity is unknown")
        }

    /** A Kotlin function as an exported function, called with native values. */
    fun exportFunction(function: Function<*>): Exported {
        val suspending = suspendMarker?.isInstance(function) == true
        val arity = arityOf(function) - if (suspending) 1 else 0
        return Exported(function) { args ->
            val native = (0 until arity).map { if (it < args.size) toNative(args[it]) else null }
            fromNative(if (suspending) invokeSuspend(function, native) else invokePlain(function, native))
        }
    }

    @Suppress("UNCHECKED_CAST")
    private fun invokePlain(function: Function<*>, args: List<Any?>): Any? = when (args.size) {
        0 -> (function as Function0<Any?>)()
        1 -> (function as Function1<Any?, Any?>)(args[0])
        2 -> (function as Function2<Any?, Any?, Any?>)(args[0], args[1])
        3 -> (function as Function3<Any?, Any?, Any?, Any?>)(args[0], args[1], args[2])
        4 -> (function as Function4<Any?, Any?, Any?, Any?, Any?>)(args[0], args[1], args[2], args[3])
        5 -> (function as Function5<Any?, Any?, Any?, Any?, Any?, Any?>)(args[0], args[1], args[2], args[3], args[4])
        6 -> (function as Function6<Any?, Any?, Any?, Any?, Any?, Any?, Any?>)(args[0], args[1], args[2], args[3], args[4], args[5])
        else -> throw ValueException("a function of ${args.size} parameters cannot cross")
    }

    @Suppress("UNCHECKED_CAST")
    private suspend fun invokeSuspend(function: Function<*>, args: List<Any?>): Any? = suspendCoroutineUninterceptedOrReturn { continuation: Continuation<Any?> ->
        when (args.size) {
            0 -> (function as Function1<Continuation<Any?>, Any?>)(continuation)
            1 -> (function as Function2<Any?, Continuation<Any?>, Any?>)(args[0], continuation)
            2 -> (function as Function3<Any?, Any?, Continuation<Any?>, Any?>)(args[0], args[1], continuation)
            3 -> (function as Function4<Any?, Any?, Any?, Continuation<Any?>, Any?>)(args[0], args[1], args[2], continuation)
            4 -> (function as Function5<Any?, Any?, Any?, Any?, Continuation<Any?>, Any?>)(args[0], args[1], args[2], args[3], continuation)
            else -> throw ValueException("a suspend function of ${args.size} parameters cannot cross")
        }
    }

}
