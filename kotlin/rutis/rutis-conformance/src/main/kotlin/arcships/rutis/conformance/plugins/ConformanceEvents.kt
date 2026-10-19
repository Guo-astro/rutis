// The plugin conformance-events (#464, docs/design-events-protocol-2026-10-18.md
// §13.1): each row is one participant of an event expectation, a listener or
// an emitter, and logs what it does through the host's synchronous service
// events-log. One plugin provides every emitter undeclared (announced: its
// methods `async` on the wire), its synchronous method named `dispatchSync`.
package arcships.rutis.conformance.plugins

import arcships.rutis.Ctx
import arcships.rutis.EventArgs
import arcships.rutis.Emitter
import arcships.rutis.Listener
import arcships.rutis.RemoteFunction
import arcships.rutis.Rutis
import arcships.rutis.RutisClient
import arcships.rutis.RutisException
import arcships.rutis.RutisFormat
import arcships.rutis.RutisService
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.async
import kotlinx.coroutines.awaitCancellation
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlinx.serialization.Contextual
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.booleanOrNull
import kotlinx.serialization.json.jsonArray
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import kotlinx.serialization.json.longOrNull

@Serializable
data class Thrown(val name: String = "Error", val message: String = "")

@Serializable
data class ListenOptions(val prepend: Boolean = false, val global: Boolean = false)

/** A row's configuration: a listener, or an emitter. */
@Serializable
data class Participant(
    val label: String = "",
    val emitter: Boolean = false,
    val name: String = "",
    val log: JsonElement? = null,
    val does: JsonElement? = null,
    val options: ListenOptions = ListenOptions(),
    /** A service of this name the row provides besides (rutis/service). */
    val provides: String = "",
    /** The row's apply fails with this error, having registered nothing. */
    val fails: Thrown? = null,
)

@Serializable
data class FilterSpec(val keep: List<String> = emptyList(), val throws: Thrown? = null)

@Serializable
data class CancelSpec(val `when`: String)

/** An emission (§13.2). */
@Serializable
data class Spec(
    val mode: String,
    val name: String = "",
    val args: List<@Contextual Any?> = emptyList(),
    val filter: FilterSpec? = null,
    /** A text it returns, or {"echo": true}: its first argument. */
    val terminal: JsonElement? = null,
    val cancel: CancelSpec? = null,
)

@RutisClient
interface EventsLog {
    fun add(entry: String)

    /** Logs `filter:<row>` and keeps the description (filterSees). */
    fun filter(description: Map<String, @Contextual Any?>)
}

/** The host's events-log.wait, where it has one: an emitter waits for a listener of another process to log. */
@RutisClient
interface EventsLogWait {
    suspend fun wait(entry: String)
}

/** What this process logged, so that an emitter of it can wait for a listener of it. */
private object Logged {
    private val entries = ArrayList<String>()
    private val lock = Object()

    fun note(entry: String) = synchronized(lock) {
        entries.add(entry)
        lock.notifyAll()
    }

    suspend fun waitFor(entry: String) {
        while (synchronized(lock) { entry !in entries }) delay(5)
    }
}

/** The service emitter-<label>. */
@RutisService
class EventEmitter internal constructor(private val row: ParticipantRow) {
    /** Emits `spec` from the row, natively, and returns the result. */
    suspend fun dispatch(spec: Spec): Any? = row.dispatch(spec)

    /** [dispatch] for `bail` and `waterfall-sync`. */
    suspend fun dispatchSync(spec: Spec): Any? = row.dispatch(spec)
}

/** The service a row provides with `provides`. */
@RutisService
class Present {
    fun ping(): String = "pong"
}

class ParticipantRow internal constructor(
    private val ctx: Ctx,
    private val p: Participant,
    private val log: EventsLog,
    private val wait: EventsLogWait?,
) {
    /** Logs `entry`, whatever cancelled the code that logs it. */
    suspend fun add(entry: String) {
        Logged.note(entry)
        withContext(NonCancellable) {
            // An interrupted thread (its call was cancelled) still logs.
            val interrupted = Thread.interrupted()
            try {
                log.add(entry)
            } finally {
                if (interrupted) Thread.currentThread().interrupt()
            }
        }
    }

    private suspend fun waitFor(entry: String) {
        if (wait != null) wait.wait(entry) else Logged.waitFor(entry)
    }

    /** Reports a description the filter is called with to events-log's filter, whole (§4.2). */
    private fun sees(listener: Listener) {
        Logged.note("filter:${listener.row}")
        log.filter(
            linkedMapOf(
                "name" to listener.name, "prefix" to listener.prefix, "plugin" to listener.plugin, "row" to listener.row,
                "runtime" to listener.runtime, "node" to listener.node,
                "scope" to listener.scope.map { linkedMapOf("id" to it.id, "group" to it.group) }, "global" to listener.global,
            ),
        )
    }

    suspend fun dispatch(spec: Spec): Any? {
        val cancel = spec.cancel ?: return emit(spec)
        return coroutineScope {
            val dispatching = async { emit(spec) }
            val watcher = launch {
                waitFor(cancel.`when`)
                dispatching.cancel()
            }
            try {
                dispatching.await()
            } finally {
                watcher.cancel()
            }
        }
    }

    private suspend fun emit(spec: Spec): Any? {
        val name = spec.name.ifEmpty { p.name }
        val filter = spec.filter
        val emitter: Emitter = if (filter == null) {
            ctx
        } else {
            ctx.filter { listener ->
                filter.throws?.let { throw RutisException(it.name, it.message) }
                sees(listener)
                listener.row in filter.keep
            }
        }
        val args = spec.args.toTypedArray()
        return when (spec.mode) {
            "emit" -> {
                emitter.emit(name, *args)
                null
            }
            "parallel" -> {
                emitter.parallel(name, *args)
                null
            }
            "serial" -> emitter.serial(name, *args)
            "bail" -> emitter.bail(name, *args)
            "waterfall" -> {
                val terminal = spec.terminal
                val last: (suspend (EventArgs) -> Any?)? = when {
                    terminal is JsonPrimitive && terminal.isString -> { _ ->
                        add("terminal")
                        terminal.content
                    }
                    terminal is JsonObject && terminal["echo"]?.jsonPrimitive?.booleanOrNull == true -> { a ->
                        add("terminal")
                        a.firstOrNull()
                    }
                    else -> null
                }
                emitter.waterfall(name, *args, terminal = last)
            }
            else -> throw IllegalArgumentException("conformance-events: mode ${spec.mode} is not the Kotlin runtime's to emit")
        }
    }

    /** What a listener logs when it starts (§13.2 `log`), or null. */
    private fun entry(args: EventArgs): String? {
        val label = p.label
        return when (val log = p.log) {
            null, JsonNull -> label
            is JsonPrimitive -> when {
                log.isString && log.content == "label" -> label
                log.isString && log.content == "arg" -> "$label:${text(args.firstOrNull())}"
                !log.isString && log.content == "false" -> null
                else -> label
            }
            is JsonArray -> {
                @Suppress("UNCHECKED_CAST")
                val record = args.firstOrNull() as? Map<String, Any?> ?: emptyMap()
                label + ":" + log.joinToString(",") { text(record[it.jsonPrimitive.content]) }
            }
            else -> label
        }
    }

    suspend fun listen(args: EventArgs): Any? {
        entry(args)?.let { add(it) }
        return does(p.does ?: JsonObject(emptyMap()), args)
    }

    private suspend fun does(spec: JsonElement, args: EventArgs): Any? {
        val does = spec as? JsonObject ?: throw IllegalArgumentException("conformance-events: does $spec")
        fun next() = args.next ?: throw IllegalStateException("conformance-events: ${p.label} has no next in this dispatch")
        return when {
            "returns" in does -> arcships.rutis.Generated.toNative(does["returns"])
            "returnsNothing" in does -> null
            "echo" in does -> args.firstOrNull()
            "throws" in does -> {
                val thrown = does["throws"]!!.jsonObject
                throw RutisException(thrown["name"]?.jsonPrimitive?.content ?: "Error", thrown["message"]?.jsonPrimitive?.content ?: "")
            }
            "after" in does -> {
                delay(does["after"]!!.jsonPrimitive.longOrNull ?: 0)
                does(does["then"] ?: JsonObject(emptyMap()), args)
            }
            "next" in does -> {
                val prefix = does["next"]!!.jsonPrimitive.content
                val rest = next()()
                prefix + (if (rest == null) "" else text(rest))
            }
            "veto" in does -> arcships.rutis.Generated.toNative(does["veto"])
            "nextTwice" in does -> {
                val n = next()
                n()
                n()
            }
            "waits" in does -> try {
                awaitCancellation()
            } finally {
                add("${p.label}:cancelled")
            }
            "calls" in does -> {
                val fn = args.firstOrNull() as? RemoteFunction
                    ?: throw IllegalArgumentException("conformance-events: ${p.label} needs a function, got ${args.firstOrNull()}")
                fn("x")
            }
            "emits" in does -> {
                val emitted = RutisFormat.decode(Spec.serializer(), does["emits"])
                dispatch(if (emitted.name.isEmpty()) emitted.copy(name = p.name) else emitted)
            }
            else -> throw IllegalArgumentException("conformance-events: unknown does $does")
        }
    }

    /** A logged value: numbers without a fraction as integers. */
    private fun text(value: Any?): String = when (value) {
        null -> "null"
        is Double -> if (value == Math.floor(value) && !value.isInfinite()) value.toLong().toString() else value.toString()
        else -> value.toString()
    }
}

val conformanceEvents = Rutis.define<Participant>(
    name = "conformance-events",
    inject = listOf("events-log"),
) { ctx, p ->
    p.fails?.let { throw RutisException(it.name, it.message) }
    val wait = try {
        ctx.use<EventsLogWait>("events-log")
    } catch (_: IllegalArgumentException) {
        null
    }
    val row = ParticipantRow(ctx, p, ctx.use<EventsLog>("events-log"), wait)
    if (p.emitter) {
        ctx.provide("emitter-${p.label}", EventEmitter(row))
    } else {
        ctx.on(p.name, prepend = p.options.prepend, global = p.options.global) { args -> row.listen(args) }
    }
    if (p.provides.isNotEmpty()) ctx.provide(p.provides, Present())
}
