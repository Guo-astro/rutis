package arcships.rutis

import arcships.rutis.internal.Native

/**
 * One loader instance (a group copy, such as a tenant or a session) of a
 * scope chain, outermost first.
 */
data class Scope(val id: String, val group: String)

/**
 * What the bus says of a listener, as a filter predicate sees it; null for
 * absent values: [row] for a listener of no row, [runtime] for one of the
 * host, [node] for one of this node.
 */
data class Listener(
    val name: String,
    val prefix: Boolean = false,
    val plugin: String,
    val row: String? = null,
    val runtime: String? = null,
    val node: String? = null,
    val scope: List<Scope> = emptyList(),
    val global: Boolean = false,
)

/** The rest of a waterfall: a waterfall listener gets it last, and calls it at most once. */
typealias Next = suspend () -> Any?

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

/**
 * Emits events, filtered: [Ctx.filter] makes one. Its five methods are those
 * of [Ctx].
 */
interface Emitter {
    /** Starts every listener of `name` and returns at once; a listener's failure is reported where it is. */
    fun emit(name: String, vararg args: Any?)

    /** Starts every listener of `name` and waits for all; failures are an `AggregateError` in registration order. */
    suspend fun parallel(name: String, vararg args: Any?)

    /** Runs the listeners of `name` one at a time and returns the first result (anything but null and false). */
    suspend fun serial(name: String, vararg args: Any?): Any?

    /**
     * Calls the outermost listener of `name`, which gets `next` last: `next`
     * runs the rest, the innermost [terminal] (called with the arguments;
     * none: no result). Returns the outermost listener's value as it is.
     */
    suspend fun waterfall(name: String, vararg args: Any?, terminal: (suspend (EventArgs) -> Any?)? = null): Any?

    /** [serial] as a synchronous call, blocking this thread (a wait cycle fails with `SyncWaitCycle`). */
    fun bail(name: String, vararg args: Any?): Any?
}
