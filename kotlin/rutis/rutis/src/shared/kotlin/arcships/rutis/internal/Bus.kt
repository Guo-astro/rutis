package arcships.rutis.internal

import arcships.rutis.Listener
import arcships.rutis.Next
import arcships.rutis.RutisException
import arcships.rutis.Scope
import arcships.rutis.internal.peer.Undefined
import arcships.rutis.internal.peer.errorMessage
import arcships.rutis.internal.peer.errorName
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.async
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.ensureActive
import kotlinx.coroutines.isActive
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonPrimitive
import java.util.concurrent.atomic.AtomicBoolean

// The dispatch of the unified event model (#464,
// docs/design-events-protocol-2026-10-18.md §5): the five modes over a
// snapshot of entries, wherever each entry runs. The runtime dispatches its
// fast path with it (every entry a listener of its own process); the testing
// tool dispatches as a host with it.

/** How a dispatch runs its listeners. */
enum class Mode(val wire: String) {
    Emit("emit"),
    Parallel("parallel"),
    Serial("serial"),
    Waterfall("waterfall"),
    Bail("bail"),
    WaterfallSync("waterfall-sync"),
    ;

    /** Whether listeners get `next` in this mode. */
    val waterfall: Boolean get() = this == Waterfall || this == WaterfallSync

    companion object {
        fun of(wire: String): Mode? = entries.firstOrNull { it.wire == wire }
    }
}

/** Runs one reserved invocation of an entry: with the event's arguments (session values) and, in a waterfall, `next`. */
typealias Call = suspend (mode: Mode, args: List<Any?>, next: Next?) -> Any?

/** One listener of a dispatch's snapshot. */
interface Entry {
    fun info(): Listener

    /** Reserves an invocation, unless the entry was withdrawn: then it is skipped. */
    fun begin(): Call?

    /**
     * Starts an `emit` invocation after the earlier ones of this entry (§5.5),
     * unless the entry was withdrawn. Its failure is reported by the entry.
     */
    fun emit(args: List<Any?>): Boolean
}

/** Refuses what no plugin may emit: `internal/` names, and `rutis/` names (ReservedEvent). */
fun checkEmit(name: String) {
    if (name.startsWith("internal/")) throw IllegalArgumentException("event $name is internal to the runtime")
    if (name.startsWith("rutis/")) throw RutisException(RutisException.RESERVED_EVENT, "rutis/* events are emitted by rutis only: $name")
}

/** Refuses what no leaf plugin may listen to: `internal/` names. */
fun checkListen(name: String) {
    if (name.startsWith("internal/")) throw IllegalArgumentException("event $name is internal to the runtime")
}

/** Whether `v` is a result: not null, undefined or false (§1), native or a session value. */
fun hasResult(v: Any?): Boolean = when (v) {
    null, Undefined, JsonNull -> false
    is Boolean -> v
    is JsonPrimitive -> !(v.content == "false" && !v.isString)
    else -> true
}

/** How long an unload waits for its row's running listeners, and cleanups may take once the session ended. */
object Shutdown {
    const val VARIABLE: String = "RUTIS_SHUTDOWN_TIMEOUT"
    const val DEFAULT_MILLIS: Long = 10_000

    /** The deadline `RUTIS_SHUTDOWN_TIMEOUT` sets (seconds), in milliseconds. */
    fun millis(): Long {
        val seconds = System.getenv(VARIABLE)?.trim()?.toDoubleOrNull() ?: return DEFAULT_MILLIS
        if (!(seconds > 0) || seconds.isInfinite()) return DEFAULT_MILLIS
        return (seconds * 1000).toLong()
    }

    /** The deadline in seconds as the lines show it: `10`, `0.5`. */
    fun shown(millis: Long): String {
        val seconds = millis / 1000.0
        return if (seconds == Math.floor(seconds)) seconds.toLong().toString() else seconds.toString()
    }
}

/** The lines of §5.6: failures nobody waits for, one stderr line each. */
object Report {
    @Volatile
    private var write: (String) -> Unit = { System.err.println(it) }

    fun line(text: String) = write(text)

    /** Replaces where the lines go, returning the previous. */
    @Synchronized
    fun replace(writer: (String) -> Unit): (String) -> Unit {
        val previous = write
        write = writer
        return previous
    }

    /** An error as the lines show it: `<ErrorName>: <message>`. */
    fun failure(error: Throwable): String = errorName(error) + ": " + errorMessage(error)
}

/** A dispatch: the snapshot, in registration order, by name (§5.1 step 1). */
class Dispatch(
    val name: String,
    val entries: List<Entry>,
    /** The filter (§6), null for none: asked once per non-global entry, in order, before any invocation. */
    val keep: (suspend (Listener) -> Boolean)? = null,
) {
    private suspend fun selected(): List<Entry> {
        val keep = keep ?: return entries
        val kept = ArrayList<Entry>(entries.size)
        for (entry in entries) {
            val info = entry.info()
            if (!info.global && !keep(info)) continue
            kept.add(entry)
        }
        return kept
    }

    /** Starts every invocation in order and waits for none; a filter's failure is thrown, for the dispatcher to report. */
    suspend fun emit(args: List<Any?>) {
        for (entry in selected()) entry.emit(args)
    }

    /**
     * Starts every invocation in order and waits for all; failures are an
     * `AggregateError` in registration order, even a single one.
     */
    suspend fun parallel(args: List<Any?>) {
        val entries = selected()
        val failures = coroutineScope {
            val running = entries.mapNotNull { entry ->
                currentCoroutineContext().ensureActive()
                val call = entry.begin() ?: return@mapNotNull null
                // Each invocation on a thread of its own, as each incoming call.
                async(Dispatchers.IO) {
                    try {
                        call(Mode.Parallel, args, null)
                        null
                    } catch (e: CancellationException) {
                        // Cancelled by itself (its row unloads): a failure;
                        // by the emitter giving up: the dispatch's end.
                        if (currentCoroutineContext().isActive) e else throw e
                    } catch (e: Throwable) {
                        e
                    }
                }
            }
            running.mapNotNull { it.await() }
        }
        if (failures.isNotEmpty()) {
            throw RutisException(RutisException.AGGREGATE_ERROR, "${failures.size} listeners of $name failed", errors = failures)
        }
    }

    /** Runs one invocation at a time and returns the first result; a failure stops it. `mode` is serial or bail. */
    suspend fun serial(mode: Mode, args: List<Any?>): Any? {
        for (entry in selected()) {
            currentCoroutineContext().ensureActive()
            val call = entry.begin() ?: continue
            val value = call(mode, args, null)
            if (hasResult(value)) return value
        }
        return null
    }

    /**
     * Calls the outermost entry with `next`, each `next` calling the entry
     * after it and the last one `terminal` with the event's arguments (none:
     * no result); returns the outermost value as it is.
     */
    suspend fun waterfall(mode: Mode, args: List<Any?>, terminal: (suspend (List<Any?>) -> Any?)?): Any? {
        val entries = selected()
        val ended = AtomicBoolean(false)
        try {
            return run(entries, 0, mode, args, terminal, ended)
        } finally {
            ended.set(true)
        }
    }

    private suspend fun run(entries: List<Entry>, from: Int, mode: Mode, args: List<Any?>, terminal: (suspend (List<Any?>) -> Any?)?, ended: AtomicBoolean): Any? {
        for (i in from until entries.size) {
            val call = entries[i].begin() ?: continue
            val called = AtomicBoolean(false)
            val next: Next = {
                if (called.getAndSet(true)) throw RutisException(RutisException.WATERFALL_NEXT, "next of $name was already called")
                if (ended.get()) throw RutisException(RutisException.WATERFALL_NEXT, "next of $name was called after its waterfall ended")
                run(entries, i + 1, mode, args, terminal, ended)
            }
            return call(mode, args, next)
        }
        return terminal?.invoke(args.toList())
    }
}

/**
 * Runs functions one after another, in the order given, on a thread while
 * there are any (§5.5: the `emit` invocations of one listener).
 */
class Queue(private val label: String) {
    private val pending = ArrayDeque<() -> Unit>()
    private var running = false

    fun push(job: () -> Unit) {
        synchronized(pending) {
            pending.addLast(job)
            if (running) return
            running = true
        }
        Thread({
            while (true) {
                val next = synchronized(pending) {
                    if (pending.isEmpty()) {
                        running = false
                        null
                    } else {
                        pending.removeFirst()
                    }
                } ?: return@Thread
                try {
                    next()
                } catch (e: Throwable) {
                    Report.line("rutis: $label failed: ${Report.failure(e)}")
                }
            }
        }, "rutis-$label").apply { isDaemon = true }.start()
    }
}
