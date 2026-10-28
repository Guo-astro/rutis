package arcships.rutis.advanced

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
import arcships.rutis.generated.ValueDecoder
import arcships.rutis.generated.ValueEncoder

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
