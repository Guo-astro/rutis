package arcships.rutis.advanced

/** A method's kind: a `sync` method is waited for, an `async` one awaited (default). */
enum class Kind(val wire: String) {
    Sync("sync"),
    Async("async"),
}

/**
 * The methods a provided service has on the wire, and the kind of each:
 * [service] from a `@RutisService` class, or from the kinds themselves.
 */
class ServiceShape internal constructor(
    /** The class whose instances provide it, when known. */
    val type: Class<*>?,
    /** Each method's kind on the wire. */
    val kinds: Map<String, String>,
    internal val problem: String? = null,
)
