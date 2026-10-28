// The session conformance target (rutis_bridge::session::testing::session),
// served by the Kotlin session: connects to the channel given as the first
// argument (a Unix socket path, or `tcp:<host>:<port>`, presenting the token
// in RUTIS_CHANNEL_TOKEN first) as endpoint `kotlin`, expecting `main`, and
// serves `conformance` until the session ends.
package arcships.rutis.conformance

import arcships.rutis.advanced.RemoteFunction
import arcships.rutis.RutisException
import arcships.rutis.internal.peer.Endpoint
import arcships.rutis.internal.peer.ExportedObject
import arcships.rutis.internal.peer.LocalChannels
import arcships.rutis.internal.peer.Options
import arcships.rutis.internal.peer.Peer
import arcships.rutis.internal.peer.SESSION_CAPABILITIES
import arcships.rutis.internal.peer.Signal
import arcships.rutis.internal.peer.Undefined
import kotlinx.coroutines.delay
import kotlinx.serialization.json.JsonPrimitive
import kotlin.system.exitProcess

/** An object with methods: it crosses only to a far end that takes objects (#675). */
private class Counter : ExportedObject {
    private var n = 0
    override val original: Any get() = this
    override suspend fun call(method: String, args: List<Any?>): Any? =
        if (method == "inc") ++n else throw IllegalArgumentException("no method $method")
    override suspend fun get(property: String): Any? = if (property == "n") n else Undefined
}

private class Conformance {
    @Volatile
    var held: RemoteFunction? = null

    @Volatile
    var started = false

    @Volatile
    var aborted = false

    lateinit var session: Peer

    private fun arg(args: List<Any?>, i: Int): Any? = if (i < args.size) args[i] else Undefined

    private fun text(value: Any?): String = (value as? JsonPrimitive)?.content ?: ""

    private fun size(value: Any?): Int = (value as? JsonPrimitive)?.content?.toIntOrNull() ?: throw IllegalArgumentException("a size expected")

    private fun function(value: Any?, method: String): RemoteFunction =
        value as? RemoteFunction ?: throw IllegalArgumentException("$method needs a function")

    suspend fun dispatch(target: String, method: String, args: List<Any?>): Any? {
        if (target != "conformance") throw IllegalArgumentException("no target $target")
        return when (method) {
            "echo" -> arg(args, 0)
            "apply" -> session.callFunctionSuspend(function(arg(args, 0), method), listOf(arg(args, 1)))
            "later" -> {
                delay(5)
                arg(args, 0)
            }
            "fail" -> throw RutisException(text(arg(args, 0)), text(arg(args, 1)))
            "hold" -> {
                held = arg(args, 0) as? RemoteFunction
                Undefined
            }
            "fire" -> {
                val fn = held ?: throw RutisException("LookupError", "nothing held")
                session.callFunctionSuspend(fn, listOf(arg(args, 0)))
            }
            "drop" -> {
                held?.release()
                held = null
                Undefined
            }
            "abortable" -> {
                val signal = arg(args, 0) as? Signal ?: throw IllegalArgumentException("abortable needs a signal")
                started = true
                try {
                    signal.await()
                } finally {
                    aborted = true
                }
                Undefined
            }
            "started" -> started
            "aborted" -> aborted
            // The value one level deeper, returned or passed to a callback:
            // past the depth limit, this side's call or reply fails (#279).
            "wrap" -> listOf(arg(args, 0))
            "applyWrapped" -> session.callFunctionSuspend(function(arg(args, 0), method), listOf(listOf(arg(args, 1))))
            "reenter" -> session.callFunctionSuspend(function(arg(args, 0), method), emptyList())
            // Past the message limit, this side's reply or call fails (#675).
            "big" -> "x".repeat(size(arg(args, 0)))
            "applyBig" -> session.callFunctionSuspend(function(arg(args, 0), method), listOf("x".repeat(size(arg(args, 1)))))
            "object" -> Counter()
            else -> throw IllegalArgumentException("no method $method")
        }
    }
}

fun main(args: Array<String>) {
    if (args.isEmpty()) {
        System.err.println("usage: SessionKt <socket path | tcp:host:port>")
        exitProcess(2)
    }
    val channel = try {
        LocalChannels.open(args[0])
    } catch (e: Exception) {
        System.err.println(e.message)
        exitProcess(1)
    }
    val service = Conformance()
    val session = Peer(
        channel,
        Options(
            dispatch = service::dispatch,
            endpoint = Endpoint(local = "kotlin", expected = "main"),
            implementation = mapOf("name" to "conformance", "version" to "0"),
            capabilities = SESSION_CAPABILITIES,
        ),
    )
    service.session = session
    session.start()
    session.closedFuture.join()
    exitProcess(0)
}
