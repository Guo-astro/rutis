package arcships.rutis

import arcships.rutis.internal.peer.Endpoint
import arcships.rutis.internal.peer.MemoryChannel
import arcships.rutis.internal.peer.Options
import arcships.rutis.internal.peer.Peer
import arcships.rutis.internal.peer.Undefined
import kotlinx.serialization.json.JsonPrimitive
import org.junit.jupiter.api.extension.ExtendWith
import org.pastalab.fray.junit.junit5.FrayTestExtension
import org.pastalab.fray.junit.junit5.annotations.FrayTest
import java.io.IOException
import java.util.Collections
import java.util.concurrent.TimeUnit
import kotlin.test.assertEquals
import kotlin.test.assertTrue

/**
 * `Peer` over [`MemoryChannel`] under Fray's scheduler (#630): what the
 * session promises whichever way its reader, writer, the callers and the
 * session's end interleave.
 *
 * Cancellation is not a Fray case here: the host's paths wait on a
 * `CompletableFuture` (an interrupted blocking call's `get`, a cancelled
 * suspend call's `await`) in ways Fray 0.9.2 cannot schedule, and the runtime
 * side's `Job.cancel()` for an incoming call deadlocks in Fray's scheduler on
 * kotlinx.coroutines' lock-free lists. `PeerTest.cancellingAnAsyncCallAbortsItsSignal`
 * covers it under the real scheduler.
 */
@ExtendWith(FrayTestExtension::class)
class PeerFrayTest {
    /** A host and a runtime side over memory channels; `dispatch` serves the runtime's calls. */
    private fun pair(dispatch: suspend (String, String, List<Any?>) -> Any?): Pair<Peer, Peer> {
        val (runtimeChannel, hostChannel) = MemoryChannel.pair()
        val runtime = Peer(runtimeChannel, Options(dispatch = dispatch, endpoint = Endpoint("kotlin", "main")))
        val host = Peer(hostChannel, Options(endpoint = Endpoint("main", "kotlin")))
        runtime.start()
        host.start()
        host.readyFuture.get(10, TimeUnit.SECONDS)
        runtime.readyFuture.get(10, TimeUnit.SECONDS)
        // Runs the coroutine classes the incoming calls use on this thread, before
        // the test's threads run calls of their own: Fray does not model class
        // initialization, and two calls racing to initialize one deadlock.
        assertEquals(JsonPrimitive("warm-up"), host.call("t", "echo", listOf("warm-up"), true))
        return host to runtime
    }

    private fun echo(): suspend (String, String, List<Any?>) -> Any? = { _, method, args ->
        when (method) {
            "echo" -> args.firstOrNull() ?: Undefined
            else -> throw IllegalArgumentException("no method $method")
        }
    }

    /**
     * Two callers make synchronous calls at the same time: each hears its own
     * reply, whatever the interleaving of the two writer queues and the
     * runtime's per-call threads.
     */
    @FrayTest(iterations = 20, resetClassLoaderPerIteration = false)
    fun syncCallsFromTwoThreadsEachHearTheirOwnReply() {
        val (host, runtime) = pair(echo())
        val failures = Collections.synchronizedList(ArrayList<Throwable>())
        val threads = (1..2).map { caller ->
            Thread {
                try {
                    repeat(3) { n ->
                        val reply = host.call("t", "echo", listOf("$caller-$n"), true)
                        assertEquals(JsonPrimitive("$caller-$n"), reply)
                    }
                } catch (e: Throwable) {
                    failures.add(e)
                }
            }
        }
        threads.forEach(Thread::start)
        threads.forEach(Thread::join)
        assertTrue(failures.isEmpty(), "calls failed: $failures")
        assertEquals(null, host.error)
        assertEquals(null, runtime.error)
        host.shutdown("done")
        runtime.shutdown("done")
    }

    /**
     * Two threads end one session while calls run: every call either hears its
     * own reply or ends with the close, never a wrong reply or a hang; both
     * ends see the session closed.
     */
    @FrayTest(iterations = 20, resetClassLoaderPerIteration = false)
    fun callsRacingACloseEachHearTheirReplyOrTheClose() {
        val (host, runtime) = pair(echo())
        val failures = Collections.synchronizedList(ArrayList<Throwable>())
        val callers = (1..2).map { caller ->
            Thread {
                repeat(3) { n ->
                    try {
                        val reply = host.call("t", "echo", listOf("$caller-$n"), true)
                        assertEquals(JsonPrimitive("$caller-$n"), reply)
                    } catch (e: Throwable) {
                        failures.add(e)
                    }
                }
            }
        }
        callers.forEach(Thread::start)
        val closers = (1..2).map { Thread { host.shutdown("closed by test") } }
        closers.forEach(Thread::start)
        (callers + closers).forEach(Thread::join)
        for (failure in failures) {
            assertTrue(
                failure is IOException && failure.message == "closed by test",
                "a call failed with $failure",
            )
        }
        assertTrue(host.closedFuture.isDone, "the host's session is not closed")
        runtime.closedFuture.get(10, TimeUnit.SECONDS)
        assertTrue(runtime.error != null, "the runtime's session saw no end")
    }
}
