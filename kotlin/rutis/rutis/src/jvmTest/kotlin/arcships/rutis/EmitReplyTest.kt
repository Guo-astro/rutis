package arcships.rutis

import arcships.rutis.internal.peer.Channel
import arcships.rutis.internal.peer.Endpoint
import arcships.rutis.internal.peer.MemoryChannel
import arcships.rutis.internal.peer.Options
import arcships.rutis.internal.peer.Peer
import arcships.rutis.internal.peer.RemoteFuture
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import java.util.concurrent.CompletableFuture
import java.util.concurrent.LinkedBlockingQueue
import java.util.concurrent.TimeUnit
import kotlin.test.AfterTest
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertIs
import kotlin.test.assertNull
import kotlin.test.assertTrue

/**
 * The reply to an `emit` through the host (events protocol §5.5): taken as
 * its frame is read, before the next frame; a 0.13 host's future answer
 * still settles. A runtime session against a scripted host.
 */
class EmitReplyTest {
    private lateinit var runtime: Peer
    private lateinit var far: Channel
    private val answeredSeen = LinkedBlockingQueue<Boolean>()
    @Volatile private var answered = false

    private fun start() {
        val (a, b) = MemoryChannel.pair()
        far = b
        runtime = Peer(a, Options(dispatch = { _, _, _ -> answeredSeen.put(answered); null }, endpoint = Endpoint("kotlin", "main")))
        runtime.start()
        far.recv() // its hello
        send("""{"op":"hello","version":4,"endpoint":"main","capabilities":[]}""")
        runtime.readyFuture.get(5, TimeUnit.SECONDS)
    }

    private fun send(frame: String) = far.send(frame.toByteArray())

    private fun next(): JsonObject = Json.parseToJsonElement(String(far.recv()!!)).jsonObject

    private fun id(frame: JsonObject) = frame["id"].toString()

    @AfterTest
    fun stop() {
        if (::runtime.isInitialized) runtime.shutdown("done")
    }

    @Test
    fun theReplyIsToldBeforeTheNextFrameIsHandled() {
        start()
        val told = CompletableFuture<Pair<Any?, Throwable?>>()
        runtime.sendAnswered("", "events.dispatch", listOf(mapOf("mode" to "emit"))) { value, error ->
            answered = true
            told.complete(value to error)
        }
        val sent = next()
        assertEquals("events.dispatch", sent["method"]!!.jsonPrimitive.content)
        // The reply, then at once a frame the runtime handles after it.
        send("""{"op":"return","id":${id(sent)},"value":{"type":"undefined"}}""")
        send("""{"op":"invoke","id":"main:1","path":[],"target":"","method":"after","args":{"type":"list","value":[]}}""")
        assertTrue(answeredSeen.poll(5, TimeUnit.SECONDS)!!, "the frame after the reply was handled before the reply was told")
        val (_, error) = told.get(5, TimeUnit.SECONDS)
        assertNull(error)
    }

    @Test
    fun aFutureAnswerOfA013HostSettles() {
        start()
        val told = CompletableFuture<Pair<Any?, Throwable?>>()
        runtime.sendAnswered("", "events.dispatch", listOf(mapOf("mode" to "emit"))) { value, error -> told.complete(value to error) }
        val sent = next()
        send("""{"op":"return","id":${id(sent)},"value":{"type":"reference","value":{"id":1,"kind":"future","home":false,"origin":[]}}}""")
        val (value, error) = told.get(5, TimeUnit.SECONDS)
        assertNull(error)
        assertIs<RemoteFuture>(value)
        // What EventsRuntime does with it, off the reading thread.
        val settled = CompletableFuture.supplyAsync { runCatching { runtime.settle(value) } }
        val await = next()
        assertEquals("await", await["op"]!!.jsonPrimitive.content)
        assertEquals("1", await["reference"]!!.jsonPrimitive.content)
        assertTrue(!settled.isDone, "settled before the host answered the await")
        send("""{"op":"return","id":${id(await)},"value":{"type":"undefined"}}""")
        assertTrue(settled.get(5, TimeUnit.SECONDS).isSuccess)
    }
}
