package arcships.rutis

import arcships.rutis.internal.peer.Channel
import arcships.rutis.internal.peer.MemoryChannel
import arcships.rutis.internal.peer.Options
import arcships.rutis.internal.peer.Peer
import arcships.rutis.internal.peer.ValueException
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import java.util.concurrent.CompletableFuture
import java.util.concurrent.TimeUnit
import kotlin.test.AfterTest
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertTrue

/**
 * #675: an object reference the far end sends (an older host's, say) fails
 * only the call it came in, as a call's argument or as a reply, and is given
 * back; the session goes on. It used to end the session.
 */
class ReceivedObjectTest {
    private lateinit var runtime: Peer
    private lateinit var far: Channel

    private val refused = "received an object with methods, which Kotlin cannot take; pass plain data or a function instead"

    private fun send(frame: String) = far.send(frame.toByteArray())

    private fun read(): JsonObject = Json.parseToJsonElement(String(far.recv()!!)).jsonObject

    private fun reference(id: Int) =
        """{"type":"reference","value":{"id":$id,"kind":"object","home":false,"origin":[]}}"""

    @AfterTest
    fun stop() {
        if (::runtime.isInitialized) runtime.shutdown("done")
    }

    @Test
    fun aReceivedObjectFailsOnlyItsCall() {
        val (a, b) = MemoryChannel.pair()
        far = b
        runtime = Peer(a, Options(dispatch = { _, _, args -> args.firstOrNull() }))
        runtime.start()
        assertEquals("hello", read()["op"]?.jsonPrimitive?.content)
        send("""{"op":"hello","version":2,"capabilities":["events-v2"]}""")
        runtime.readyFuture.get(5, TimeUnit.SECONDS)

        // As a call's argument: the call is answered with a throw, and the
        // reference is given back.
        send("""{"op":"invoke","id":"rust:1","path":[],"target":"t","method":"echo","args":{"type":"list","value":[${reference(7)}]}}""")
        val frames = listOf(read(), read()).associateBy { it["op"]!!.jsonPrimitive.content }
        assertEquals(JsonPrimitive(7), frames["release"]?.get("reference"))
        assertEquals(JsonPrimitive(1), frames["release"]?.get("count"))
        assertEquals(refused, frames["throw"]?.get("error")?.jsonObject?.get("message")?.jsonPrimitive?.content)

        // As a reply: the call waiting for it fails.
        val outcome = CompletableFuture<Throwable?>()
        Thread {
            outcome.complete(
                try {
                    runtime.call("t", "make", emptyList(), true)
                    null
                } catch (e: Throwable) {
                    e
                },
            )
        }.start()
        val call = read()
        assertEquals("invoke", call["op"]?.jsonPrimitive?.content)
        send("""{"op":"return","id":${call["id"]},"value":${reference(8)}}""")
        val error = outcome.get(5, TimeUnit.SECONDS)
        assertTrue(error is ValueException, "$error")
        assertEquals(refused, error.message)
        val release = read()
        assertEquals(JsonPrimitive(8), release["reference"])

        // The session goes on.
        send("""{"op":"invoke","id":"rust:2","path":[],"target":"t","method":"echo","args":{"type":"data","value":["after"]}}""")
        val reply = read()
        assertEquals("return", reply["op"]?.jsonPrimitive?.content)
        assertEquals(null, runtime.error)
    }
}
