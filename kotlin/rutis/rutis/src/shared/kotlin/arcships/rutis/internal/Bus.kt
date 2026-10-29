package arcships.rutis.internal

import arcships.rutis.RutisException
import arcships.rutis.internal.peer.errorMessage
import arcships.rutis.internal.peer.errorName

// What the runtime's events share (#464,
// docs/design-events-protocol-2026-10-18.md §5): the five modes, the names no
// plugin may emit or listen to, the lines of failures nobody waits for. The
// host dispatches every emission.

/** How a dispatch runs its listeners. */
enum class Mode(val wire: String) {
    Emit("emit"),
    Parallel("parallel"),
    Serial("serial"),
    Waterfall("waterfall"),
    Bail("bail"),
    ;

    /** Whether listeners get `next` in this mode. */
    val waterfall: Boolean get() = this == Waterfall

    companion object {
        fun of(wire: String): Mode? = entries.firstOrNull { it.wire == wire }
    }
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
