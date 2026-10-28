package arcships.rutis

import arcships.rutis.internal.Native
import arcships.rutis.advanced.Next
import arcships.rutis.advanced.RutisFormat

/**
 * A listener's arguments: the event's arguments as native values (`Any?`
 * rules: integers as Long, objects as `Map<String, Any?>`, ...), and in a
 * waterfall [next] as the last element.
 */
class EventArgs internal constructor(
    private val values: List<Any?>,
    /** The arguments as they crossed, for [decode]. */
    private val raw: List<Any?>,
    /** In a waterfall: the rest of it (also the last element). */
    val next: Next?,
) : List<Any?> by values {
    /** Argument `i` decoded into `T` (a `@Serializable` type). */
    inline fun <reified T> decode(i: Int): T = decode(i, RutisFormat.serializer<T>())

    /** [decode] with the serializer given. */
    fun <T> decode(i: Int, deserializer: kotlinx.serialization.DeserializationStrategy<T>): T =
        RutisFormat.decode(deserializer, if (i < raw.size) raw[i] else null)

    override fun toString(): String = values.toString()

    internal companion object {
        /** The arguments of session values `raw`, with `next` last in a waterfall. */
        fun of(raw: List<Any?>, next: Next?): EventArgs {
            val values = raw.map(Native::toNative).toMutableList<Any?>()
            if (next != null) values.add(next)
            return EventArgs(values, raw, next)
        }
    }
}
