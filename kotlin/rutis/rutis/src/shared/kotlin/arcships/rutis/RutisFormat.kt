package arcships.rutis

import arcships.rutis.internal.Native
import arcships.rutis.internal.peer.Decimal
import arcships.rutis.internal.peer.JsonValues
import arcships.rutis.internal.peer.MAX_SAFE
import arcships.rutis.internal.peer.Undefined
import arcships.rutis.internal.peer.ValueException
import arcships.rutis.internal.peer.WireMap
import arcships.rutis.internal.peer.WireSet
import arcships.rutis.internal.peer.describe
import arcships.rutis.internal.peer.formatDate
import arcships.rutis.internal.peer.formatDateTime
import arcships.rutis.internal.peer.formatInstant
import arcships.rutis.internal.peer.formatTime
import arcships.rutis.internal.peer.formatUuid
import arcships.rutis.internal.peer.parseDate
import arcships.rutis.internal.peer.parseDateTime
import arcships.rutis.internal.peer.parseDecimal
import arcships.rutis.internal.peer.parseInstant
import arcships.rutis.internal.peer.parseTime
import arcships.rutis.internal.peer.parseUuid
import kotlinx.serialization.DeserializationStrategy
import kotlinx.serialization.KSerializer
import kotlinx.serialization.SerializationStrategy
import kotlinx.serialization.descriptors.PolymorphicKind
import kotlinx.serialization.descriptors.PrimitiveKind
import kotlinx.serialization.descriptors.PrimitiveSerialDescriptor
import kotlinx.serialization.descriptors.SerialDescriptor
import kotlinx.serialization.descriptors.SerialKind
import kotlinx.serialization.descriptors.StructureKind
import kotlinx.serialization.encoding.AbstractDecoder
import kotlinx.serialization.encoding.AbstractEncoder
import kotlinx.serialization.encoding.CompositeDecoder
import kotlinx.serialization.encoding.CompositeEncoder
import kotlinx.serialization.encoding.Decoder
import kotlinx.serialization.encoding.Encoder
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.modules.SerializersModule
import kotlinx.serialization.modules.contextual
import kotlinx.serialization.serializer
import java.math.BigDecimal
import java.math.BigInteger
import java.time.Instant
import java.time.LocalDate
import java.time.LocalDateTime
import java.time.LocalTime
import java.util.UUID
import kotlin.time.toJavaInstant
import kotlin.time.toKotlinInstant
import kotlin.uuid.toJavaUuid
import kotlin.uuid.toKotlinUuid

/**
 * The value layer: a kotlinx-serialization format between Kotlin values and
 * the values a session carries (docs/design-value-types-2026-10-15.md),
 * driven by the serializers the compiler generates. Every value is checked
 * on its way: points in time, binary data, maps with keys that are not
 * strings, sets, big integers, decimals and UUIDs cross as their tagged
 * values, never as text or numbers; an integer beyond ±(2^53−1) crosses as a
 * `bigint`.
 *
 * | Kotlin | crosses as |
 * | --- | --- |
 * | `java.time.Instant`, `kotlin.time.Instant` | `instant` |
 * | `LocalDateTime` / `LocalDate` / `LocalTime` | `datetime` / `date` / `time` |
 * | `ByteArray` | `bytes` |
 * | `Map` with keys that are not strings | `map` |
 * | `Set` | `set` |
 * | `BigInteger` | `bigint` |
 * | `BigDecimal` | `decimal`, its scale kept |
 * | `java.util.UUID`, `kotlin.uuid.Uuid` | `uuid` |
 *
 * Fields of these JDK types in a `@Serializable` class are marked
 * `@Contextual` (kotlinx's rule); [serializersModule] has their serializers.
 */
object RutisFormat {
    /** The serializers of the JDK types the format takes, and of `Any` (decoded natively). */
    val serializersModule: SerializersModule = SerializersModule {
        contextual(Instant::class, JdkSerializers.InstantSerializer)
        contextual(LocalDateTime::class, JdkSerializers.LocalDateTimeSerializer)
        contextual(LocalDate::class, JdkSerializers.LocalDateSerializer)
        contextual(LocalTime::class, JdkSerializers.LocalTimeSerializer)
        contextual(BigInteger::class, JdkSerializers.BigIntegerSerializer)
        contextual(BigDecimal::class, JdkSerializers.BigDecimalSerializer)
        contextual(UUID::class, JdkSerializers.UuidSerializer)
        contextual(Any::class, AnySerializer)
    }

    /** `value` as a session value. */
    fun <T> encode(serializer: SerializationStrategy<T>, value: T): Any? {
        var result: Any? = null
        ValueEncoder { result = it }.encodeSerializableValue(serializer, value)
        return result
    }

    /** A session value as a `T`. */
    fun <T> decode(deserializer: DeserializationStrategy<T>, value: Any?): T =
        ValueDecoder(value).decodeSerializableValue(deserializer)

    /** The serializer of `T`, the JDK types and `Any` included. */
    inline fun <reified T> serializer(): KSerializer<T> = serializersModule.serializer()
}

/** `Any`: decoded as the value's native Kotlin form, encoded by what the value is. */
object AnySerializer : KSerializer<Any> {
    override val descriptor: SerialDescriptor = PrimitiveSerialDescriptor("arcships.rutis.Any", PrimitiveKind.STRING)

    override fun serialize(encoder: Encoder, value: Any) {
        if (encoder !is ValueEncoder) throw ValueException("Any is encoded by RutisFormat only")
        encoder.put(Native.fromNative(value))
    }

    override fun deserialize(decoder: Decoder): Any {
        if (decoder !is ValueDecoder) throw ValueException("Any is decoded by RutisFormat only")
        return Native.toNative(decoder.value) ?: throw ValueException("null where a value was expected")
    }
}

/** The serializers of JDK types, as text outside RutisFormat (their wire text). */
object JdkSerializers {
    private abstract class Text<T : Any>(name: String, val format: (T) -> String, val parse: (String) -> T) : KSerializer<T> {
        override val descriptor: SerialDescriptor = PrimitiveSerialDescriptor(name, PrimitiveKind.STRING)
        override fun serialize(encoder: Encoder, value: T) = encoder.encodeString(format(value))
        override fun deserialize(decoder: Decoder): T = parse(decoder.decodeString())
    }

    val InstantSerializer: KSerializer<Instant> = object : Text<Instant>("java.time.Instant", ::formatInstant, ::parseInstant) {}
    val LocalDateTimeSerializer: KSerializer<LocalDateTime> = object : Text<LocalDateTime>("java.time.LocalDateTime", ::formatDateTime, ::parseDateTime) {}
    val LocalDateSerializer: KSerializer<LocalDate> = object : Text<LocalDate>("java.time.LocalDate", ::formatDate, ::parseDate) {}
    val LocalTimeSerializer: KSerializer<LocalTime> = object : Text<LocalTime>("java.time.LocalTime", ::formatTime, ::parseTime) {}
    val BigIntegerSerializer: KSerializer<BigInteger> = object : Text<BigInteger>("java.math.BigInteger", { it.toString() }, ::BigInteger) {}
    val BigDecimalSerializer: KSerializer<BigDecimal> = object : Text<BigDecimal>("java.math.BigDecimal", { it.toPlainString() }, { BigDecimal(parseDecimal(it).text) }) {}
    val UuidSerializer: KSerializer<UUID> = object : Text<UUID>("java.util.UUID", ::formatUuid, { parseUuid(it.lowercase()) }) {}
}

private const val SET = "kotlin.collections.LinkedHashSet"
private const val HASH_SET = "kotlin.collections.HashSet"

/** Whether the descriptor's values cross as a string key (a record's), not as a tagged value. */
private fun stringKey(descriptor: SerialDescriptor): Boolean =
    (descriptor.kind == PrimitiveKind.STRING || descriptor.kind == SerialKind.ENUM) && descriptor.serialName.removeSuffix("?") !in tagged

/** The serial names of the types that cross as tagged values. */
private val tagged = setOf(
    "java.time.Instant", "java.time.LocalDateTime", "java.time.LocalDate", "java.time.LocalTime",
    "java.math.BigInteger", "java.math.BigDecimal", "java.util.UUID", "kotlin.time.Instant", "kotlin.uuid.Uuid",
    "kotlin.ByteArray", SET, HASH_SET, "arcships.rutis.Any",
)

/** The session value of a value whose type the format takes directly, or null when it is not one. */
private fun direct(value: Any?): Any? = when (value) {
    is Instant, is LocalDateTime, is LocalDate, is LocalTime, is ByteArray, is BigInteger, is UUID -> value
    is BigDecimal -> Decimal(value.toPlainString())
    is kotlin.time.Instant -> value.toJavaInstant()
    is kotlin.uuid.Uuid -> value.toJavaUuid()
    is JsonElement -> value
    else -> null
}

/** Encodes into session values; [put] receives the value of what it encodes. */
class ValueEncoder internal constructor(private val sink: (Any?) -> Unit) : AbstractEncoder() {
    override val serializersModule: SerializersModule = RutisFormat.serializersModule

    internal fun put(value: Any?) = sink(value)

    override fun <T> encodeSerializableValue(serializer: SerializationStrategy<T>, value: T) {
        val known = direct(value)
        when {
            known != null -> sink(known)
            serializer.descriptor.serialName.removeSuffix("?").let { it == SET || it == HASH_SET } -> {
                // The set's own serializer writes its elements as a list.
                serializer.serialize(ValueEncoder { sink(WireSet(it as List<*>)) }, value)
            }
            else -> serializer.serialize(this, value)
        }
    }

    override fun encodeNull() = sink(null)
    override fun encodeBoolean(value: Boolean) = sink(value)
    override fun encodeByte(value: Byte) = sink(value.toLong())
    override fun encodeShort(value: Short) = sink(value.toLong())
    override fun encodeInt(value: Int) = sink(value.toLong())
    override fun encodeLong(value: Long) = sink(if (value > MAX_SAFE || value < -MAX_SAFE) BigInteger.valueOf(value) else value)
    override fun encodeFloat(value: Float) = encodeDouble(value.toDouble())
    override fun encodeDouble(value: Double) {
        if (value.isNaN() || value.isInfinite()) throw ValueException("a number that is not finite ($value) cannot cross")
        sink(value)
    }
    override fun encodeChar(value: Char) = sink(value.toString())
    override fun encodeString(value: String) = sink(value)
    override fun encodeEnum(enumDescriptor: SerialDescriptor, index: Int) = sink(enumDescriptor.getElementName(index))

    override fun encodeInline(descriptor: SerialDescriptor): Encoder = when (descriptor.serialName) {
        // Unsigned integers are encoded as their signed bits: read them back.
        "kotlin.ULong" -> object : ForwardingEncoder(this) {
            override fun encodeLong(value: Long) {
                val unsigned = value.toULong()
                sink(if (unsigned > MAX_SAFE.toULong()) BigInteger(unsigned.toString()) else unsigned.toLong())
            }
        }
        "kotlin.UInt" -> object : ForwardingEncoder(this) {
            override fun encodeInt(value: Int) = sink(value.toUInt().toLong())
        }
        "kotlin.UShort" -> object : ForwardingEncoder(this) {
            override fun encodeShort(value: Short) = sink(value.toUShort().toLong())
        }
        "kotlin.UByte" -> object : ForwardingEncoder(this) {
            override fun encodeByte(value: Byte) = sink(value.toUByte().toLong())
        }
        else -> this
    }

    override fun beginStructure(descriptor: SerialDescriptor): CompositeEncoder = when (descriptor.kind) {
        StructureKind.LIST -> ListEncoder(sink)
        StructureKind.MAP -> MapEncoder(descriptor, sink)
        is PolymorphicKind -> RecordEncoder(descriptor, sink)
        else -> RecordEncoder(descriptor, sink)
    }
}

private abstract class ForwardingEncoder(private val base: ValueEncoder) : AbstractEncoder() {
    override val serializersModule: SerializersModule = base.serializersModule
    override fun encodeNull() = base.encodeNull()
}

/** The base of the encoders of a structure's elements. */
private abstract class ElementsEncoder : AbstractEncoder() {
    override val serializersModule: SerializersModule = RutisFormat.serializersModule
    protected var index = -1

    /** Stores the value of the element at [index]. */
    abstract fun store(value: Any?)

    private val element = ValueEncoder { store(it) }

    override fun encodeElement(descriptor: SerialDescriptor, index: Int): Boolean {
        this.index = index
        return true
    }

    override fun <T> encodeSerializableValue(serializer: SerializationStrategy<T>, value: T) = element.encodeSerializableValue(serializer, value)
    override fun encodeNull() = store(null)
    override fun encodeBoolean(value: Boolean) = element.encodeBoolean(value)
    override fun encodeByte(value: Byte) = element.encodeByte(value)
    override fun encodeShort(value: Short) = element.encodeShort(value)
    override fun encodeInt(value: Int) = element.encodeInt(value)
    override fun encodeLong(value: Long) = element.encodeLong(value)
    override fun encodeFloat(value: Float) = element.encodeFloat(value)
    override fun encodeDouble(value: Double) = element.encodeDouble(value)
    override fun encodeChar(value: Char) = element.encodeChar(value)
    override fun encodeString(value: String) = element.encodeString(value)
    override fun encodeEnum(enumDescriptor: SerialDescriptor, index: Int) = element.encodeEnum(enumDescriptor, index)
    override fun encodeInline(descriptor: SerialDescriptor): Encoder = element.encodeInline(descriptor)
    override fun beginStructure(descriptor: SerialDescriptor): CompositeEncoder = element.beginStructure(descriptor)
}

private class ListEncoder(private val sink: (Any?) -> Unit) : ElementsEncoder() {
    private val items = ArrayList<Any?>()
    override fun store(value: Any?) {
        items.add(value)
    }
    override fun endStructure(descriptor: SerialDescriptor) = sink(items)
}

private class RecordEncoder(private val descriptor: SerialDescriptor, private val sink: (Any?) -> Unit) : ElementsEncoder() {
    private val fields = LinkedHashMap<String, Any?>()
    override fun store(value: Any?) {
        fields[descriptor.getElementName(index)] = value
    }
    override fun endStructure(descriptor: SerialDescriptor) = sink(fields)
}

private class MapEncoder(descriptor: SerialDescriptor, private val sink: (Any?) -> Unit) : ElementsEncoder() {
    private val byString = stringKey(descriptor.getElementDescriptor(0))
    private val pairs = ArrayList<Pair<Any?, Any?>>()
    private var key: Any? = null
    override fun store(value: Any?) {
        if (index % 2 == 0) key = value else pairs.add(key to value)
    }
    override fun endStructure(descriptor: SerialDescriptor) {
        if (byString && pairs.all { it.first is String }) {
            sink(LinkedHashMap<String, Any?>().apply { for ((k, v) in pairs) put(k as String, v) })
        } else {
            sink(WireMap(pairs))
        }
    }
}

/** Decodes from a session value. */
class ValueDecoder internal constructor(internal val value: Any?) : AbstractDecoder() {
    override val serializersModule: SerializersModule = RutisFormat.serializersModule

    private fun fail(expected: String): Nothing = throw ValueException("expected $expected, got ${describe(value)}")

    private fun primitive(): JsonPrimitive? = value as? JsonPrimitive

    override fun <T> decodeSerializableValue(deserializer: DeserializationStrategy<T>): T {
        val name = deserializer.descriptor.serialName.removeSuffix("?")
        @Suppress("UNCHECKED_CAST")
        return when (name) {
            "java.time.Instant" -> instant() as T
            "kotlin.time.Instant" -> instant().toKotlinInstant() as T
            "java.time.LocalDateTime" -> (value as? LocalDateTime ?: text("a datetime")?.let(::parseDateTime) ?: fail("a datetime")) as T
            "java.time.LocalDate" -> (value as? LocalDate ?: text("a date")?.let(::parseDate) ?: fail("a date")) as T
            "java.time.LocalTime" -> (value as? LocalTime ?: text("a time")?.let(::parseTime) ?: fail("a time")) as T
            "java.math.BigInteger" -> bigInteger() as T
            "java.math.BigDecimal" -> when (value) {
                is Decimal -> BigDecimal(value.text)
                is BigInteger -> BigDecimal(value)
                is Long -> BigDecimal.valueOf(value)
                else -> primitive()?.takeIf { !it.isString }?.content?.let(::BigDecimal) ?: text("a decimal")?.let { BigDecimal(parseDecimal(it).text) } ?: fail("a decimal")
            } as T
            "java.util.UUID" -> uuid() as T
            "kotlin.uuid.Uuid" -> uuid().toKotlinUuid() as T
            "kotlin.ByteArray" -> (value as? ByteArray ?: fail("bytes")) as T
            // JSON trees take data as it is (lists and records of data too).
            "kotlinx.serialization.json.JsonElement", "kotlinx.serialization.json.JsonObject",
            "kotlinx.serialization.json.JsonArray", "kotlinx.serialization.json.JsonPrimitive",
            "kotlinx.serialization.json.JsonNull",
            -> (if (value === Undefined) JsonNull else arcships.rutis.internal.peer.plain(value)) as T
            SET, HASH_SET -> {
                val elements = when (value) {
                    is WireSet -> value.elements
                    else -> items() ?: fail("a set")
                }
                // The set's own serializer, fed a list of the elements.
                deserializer.deserialize(ValueDecoder(elements))
            }
            else -> deserializer.deserialize(this)
        }
    }

    private fun text(what: String): String? = when (value) {
        is String -> value
        is JsonPrimitive -> if (value.isString) value.content else null
        else -> null
    }

    private fun instant(): Instant = value as? Instant ?: text("an instant")?.let(::parseInstant) ?: fail("an instant")

    private fun uuid(): UUID = value as? UUID ?: text("a uuid")?.let { parseUuid(it.lowercase()) } ?: fail("a uuid")

    private fun bigInteger(): BigInteger = when (value) {
        is BigInteger -> value
        is Long -> BigInteger.valueOf(value)
        is Int -> BigInteger.valueOf(value.toLong())
        else -> primitive()?.takeIf { !it.isString }?.content?.let { JsonValues.integer(it) } ?: fail("an integer")
    }

    private fun items(): List<Any?>? = when (value) {
        is List<*> -> value
        is JsonArray -> value
        is WireSet -> value.elements
        else -> null
    }

    override fun decodeNotNullMark(): Boolean = value != null && value != JsonNull && value !== Undefined
    override fun decodeNull(): Nothing? = null
    override fun decodeBoolean(): Boolean = when (value) {
        is Boolean -> value
        is JsonPrimitive -> if (!value.isString && (value.content == "true" || value.content == "false")) value.content == "true" else fail("a boolean")
        else -> fail("a boolean")
    }

    private fun integer(min: Long, max: Long, what: String): Long {
        val n = bigInteger()
        if (n < BigInteger.valueOf(min) || n > BigInteger.valueOf(max)) throw ValueException("the integer $n does not fit in $what")
        return n.toLong()
    }

    override fun decodeByte(): Byte = integer(Byte.MIN_VALUE.toLong(), Byte.MAX_VALUE.toLong(), "a Byte").toByte()
    override fun decodeShort(): Short = integer(Short.MIN_VALUE.toLong(), Short.MAX_VALUE.toLong(), "a Short").toShort()
    override fun decodeInt(): Int = integer(Int.MIN_VALUE.toLong(), Int.MAX_VALUE.toLong(), "an Int").toInt()
    override fun decodeLong(): Long = integer(Long.MIN_VALUE, Long.MAX_VALUE, "a Long")
    override fun decodeFloat(): Float = decodeDouble().toFloat()
    override fun decodeDouble(): Double = when (value) {
        is Double -> value
        is Long -> value.toDouble()
        is Int -> value.toDouble()
        is BigInteger -> value.toDouble()
        is JsonPrimitive -> if (!value.isString) value.content.toDoubleOrNull() ?: fail("a number") else fail("a number")
        else -> fail("a number")
    }
    override fun decodeChar(): Char = decodeString().singleOrNull() ?: fail("one character")
    override fun decodeString(): String = text("a string") ?: fail("a string")
    override fun decodeEnum(enumDescriptor: SerialDescriptor): Int {
        val name = decodeString()
        val index = enumDescriptor.getElementIndex(name)
        if (index == CompositeDecoder.UNKNOWN_NAME) throw ValueException("$name is not one of ${(0 until enumDescriptor.elementsCount).map(enumDescriptor::getElementName)}")
        return index
    }

    override fun decodeInline(descriptor: SerialDescriptor): Decoder = when (descriptor.serialName) {
        "kotlin.ULong" -> object : InlineDecoder(this) {
            override fun decodeLong(): Long {
                val n = bigIntegerOf(value)
                if (n.signum() < 0 || n.bitLength() > 64) throw ValueException("the integer $n does not fit in a ULong")
                return n.toLong()
            }
        }
        "kotlin.UInt" -> object : InlineDecoder(this) {
            override fun decodeInt(): Int {
                val n = bigIntegerOf(value)
                if (n.signum() < 0 || n.bitLength() > 32) throw ValueException("the integer $n does not fit in a UInt")
                return n.toLong().toInt()
            }
        }
        else -> this
    }

    override fun decodeElementIndex(descriptor: SerialDescriptor): Int = throw IllegalStateException("decodeElementIndex on a value")

    override fun beginStructure(descriptor: SerialDescriptor): CompositeDecoder = when (descriptor.kind) {
        StructureKind.LIST -> ListDecoder(items() ?: fail("an array"))
        StructureKind.MAP -> when (value) {
            is WireMap -> MapDecoder(value.entries)
            else -> MapDecoder(fields()?.entries?.map { it.key to it.value } ?: fail("an object"))
        }
        else -> RecordDecoder(fields() ?: fail("an object"))
    }

    private fun fields(): Map<String, Any?>? = when (value) {
        is JsonObject -> value
        is Map<*, *> -> @Suppress("UNCHECKED_CAST") (value as Map<String, Any?>)
        else -> null
    }
}

private fun bigIntegerOf(value: Any?): BigInteger = when (value) {
    is BigInteger -> value
    is Long -> BigInteger.valueOf(value)
    is JsonPrimitive -> if (!value.isString) JsonValues.integer(value.content) ?: throw ValueException("expected an integer, got ${describe(value)}") else throw ValueException("expected an integer, got a string")
    else -> throw ValueException("expected an integer, got ${describe(value)}")
}

private abstract class InlineDecoder(private val base: ValueDecoder) : AbstractDecoder() {
    override val serializersModule: SerializersModule = base.serializersModule
    val value: Any? get() = base.value
    override fun decodeElementIndex(descriptor: SerialDescriptor): Int = throw IllegalStateException()
}

/** The base of the decoders of a structure's elements. */
private abstract class ElementsDecoder : AbstractDecoder() {
    override val serializersModule: SerializersModule = RutisFormat.serializersModule
    protected var index = -1

    /** The value of the element at [index]. */
    abstract fun current(): Any?

    private fun element() = ValueDecoder(current())

    override fun <T> decodeSerializableValue(deserializer: DeserializationStrategy<T>): T = element().decodeSerializableValue(deserializer)
    override fun decodeNotNullMark(): Boolean = element().decodeNotNullMark()
    override fun decodeNull(): Nothing? = null
    override fun decodeBoolean(): Boolean = element().decodeBoolean()
    override fun decodeByte(): Byte = element().decodeByte()
    override fun decodeShort(): Short = element().decodeShort()
    override fun decodeInt(): Int = element().decodeInt()
    override fun decodeLong(): Long = element().decodeLong()
    override fun decodeFloat(): Float = element().decodeFloat()
    override fun decodeDouble(): Double = element().decodeDouble()
    override fun decodeChar(): Char = element().decodeChar()
    override fun decodeString(): String = element().decodeString()
    override fun decodeEnum(enumDescriptor: SerialDescriptor): Int = element().decodeEnum(enumDescriptor)
    override fun decodeInline(descriptor: SerialDescriptor): Decoder = element().decodeInline(descriptor)
    override fun beginStructure(descriptor: SerialDescriptor): CompositeDecoder = element().beginStructure(descriptor)
}

private class ListDecoder(private val items: List<Any?>) : ElementsDecoder() {
    override fun current(): Any? = items[index]
    override fun decodeCollectionSize(descriptor: SerialDescriptor): Int = items.size
    override fun decodeElementIndex(descriptor: SerialDescriptor): Int {
        index++
        return if (index < items.size) index else CompositeDecoder.DECODE_DONE
    }
}

private class MapDecoder(private val entries: List<Pair<Any?, Any?>>) : ElementsDecoder() {
    override fun current(): Any? = entries[index / 2].let { if (index % 2 == 0) it.first else it.second }
    override fun decodeCollectionSize(descriptor: SerialDescriptor): Int = entries.size
    override fun decodeElementIndex(descriptor: SerialDescriptor): Int {
        index++
        return if (index < entries.size * 2) index else CompositeDecoder.DECODE_DONE
    }
}

private class RecordDecoder(private val fields: Map<String, Any?>) : ElementsDecoder() {
    private var position = -1

    override fun current(): Any? = fields[name]

    private var name = ""

    override fun decodeElementIndex(descriptor: SerialDescriptor): Int {
        while (++position < descriptor.elementsCount) {
            val element = descriptor.getElementName(position)
            val present = fields.containsKey(element) && fields[element] !== Undefined
            // An absent field: its default when it has one; null when it may
            // be null (undefined, as other languages leave it out).
            if (present || (!descriptor.isElementOptional(position) && descriptor.getElementDescriptor(position).isNullable)) {
                index = position
                name = element
                return position
            }
        }
        return CompositeDecoder.DECODE_DONE
    }
}

@Suppress("unused")
private val unusedKinds: List<SerialKind> = listOf(SerialKind.CONTEXTUAL)
