package arcships.rutis

import kotlinx.serialization.json.JsonObject

/**
 * An error that crossed from another process, or one of rutis's own
 * (`AggregateError`, `AsyncListener`, `WaterfallNext`, `ReservedEvent`,
 * `SyncWaitCycle`, `BindingError`). Match it by [name], not by class: the
 * error crossed as data, not as the class it was thrown as.
 *
 * [code] and [data] are what the error carried besides its message (a Rust
 * service's `Err(e)`: the variant's name and its value; a Kotlin exception's
 * `code` and `data` properties); [data] is decoded as any value is. [errors]
 * are an `AggregateError`'s failures, in registration order. Thrown on, it
 * crosses again with everything it carried.
 */
open class RutisException(
    val name: String,
    message: String,
    val code: String? = null,
    val data: Any? = null,
    cause: Throwable? = null,
    val errors: List<Throwable> = emptyList(),
) : RuntimeException(message, cause) {
    /** The error graph it crossed with, kept so that it crosses again whole. */
    internal var graph: JsonObject? = null

    override val message: String get() = super.message ?: ""

    override fun toString(): String = "$name: $message"

    companion object {
        /** Failures of a `parallel` dispatch, in registration order. */
        const val AGGREGATE_ERROR: String = "AggregateError"

        /** A `bail` reached a listener known to be asynchronous. */
        const val ASYNC_LISTENER: String = "AsyncListener"

        /** A waterfall's `next` called twice, or after the waterfall ended. */
        const val WATERFALL_NEXT: String = "WaterfallNext"

        /** A plugin emitted an event named `rutis/...`. */
        const val RESERVED_EVENT: String = "ReservedEvent"

        /** A synchronous call would wait for itself (the host detects it). */
        const val SYNC_WAIT_CYCLE: String = "SyncWaitCycle"

        /** A transport failure, a withdrawn listener, a refused frame. */
        const val BINDING_ERROR: String = "BindingError"
    }
}
