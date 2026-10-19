package arcships.rutis.internal.peer

import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.ThreadContextElement
import kotlinx.coroutines.asContextElement
import kotlin.coroutines.CoroutineContext

/** The wire's `undefined`: an omitted argument or field. */
object Undefined {
    override fun toString(): String = "undefined"
}

/**
 * A function this side exports: the far end calls it. [original] is what it
 * was made from (a Kotlin function), by which repeated exports are the same
 * reference and which a home reference decodes back to.
 */
class Exported(val original: Any, val body: suspend (List<Any?>) -> Any?)

/** An object this side exports: the far end calls its methods and reads its properties. */
interface ExportedObject {
    val original: Any
    suspend fun call(method: String, args: List<Any?>): Any?
    suspend fun get(property: String): Any?
}

/** An argument that crosses as the wire's `signal`: the callee's Signal aborts when this side cancels the call. */
object SignalArgument

/** The cancellation a call received as an argument: done once the caller cancels. */
class Signal {
    private val done = CompletableDeferred<Unit>()

    /** Whether the caller gave up. */
    val aborted: Boolean get() = done.isCompleted

    /** Suspends until the caller gives up. */
    suspend fun await() = done.await()

    internal fun cancel() {
        done.complete(Unit)
    }
}

/**
 * The call chain (`path`) of the code running now: set on the thread of each
 * incoming call to the call's path and id, carried into coroutines by
 * [element], and taken by every outgoing call. Plugin code never passes it.
 */
object CallChain {
    @PublishedApi internal val local: ThreadLocal<List<String>?> = ThreadLocal()

    /** The chain of the code running now. */
    fun current(): List<String> = local.get() ?: emptyList()

    /** The coroutine context element carrying `chain`. */
    fun element(chain: List<String> = current()): ThreadContextElement<List<String>?> = local.asContextElement(chain)

    /** Runs `block` with `chain` as this thread's chain. */
    inline fun <T> with(chain: List<String>, block: () -> T): T {
        val previous = local.get()
        local.set(chain)
        try {
            return block()
        } finally {
            local.set(previous)
        }
    }
}

/** The context element of the chain running now, for a coroutine started from it. */
fun chainContext(): CoroutineContext = CallChain.element()
