package arcships.rutis

import arcships.rutis.internal.peer.Channel
import arcships.rutis.internal.peer.Endpoint
import arcships.rutis.internal.peer.MemoryChannel
import arcships.rutis.internal.peer.Options
import arcships.rutis.internal.peer.Peer
import arcships.rutis.internal.peer.ValueException
import kotlinx.coroutines.runBlocking
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import java.util.concurrent.CompletableFuture
import java.util.concurrent.TimeUnit
import kotlin.test.AfterTest
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertNull
import kotlin.test.assertTrue

/** The far end of a session written frame by frame, as the test wants it. */
private class Far(private val channel: Channel) {
    fun send(frame: String) = channel.send(frame.toByteArray(Charsets.UTF_8))

    fun next(): JsonObject {
        val message = CompletableFuture.supplyAsync {
            channel.recv() ?: throw IllegalStateException("the session ended")
        }.get(5, TimeUnit.SECONDS)
        return Json.parseToJsonElement(String(message, Charsets.UTF_8)).jsonObject
    }
}

/** `depth` arrays around a number: a frame's value, as deep as the test says. */
private fun nested(depth: Int): String = "[".repeat(depth) + "0" + "]".repeat(depth)

/**
 * A frame nested deeper than the 208 levels this side reads (#279, protocol
 * §8): a call it names that is this far end's fails only that call and the
 * session goes on; one that names no call ends the session. The parser is
 * never given more than it can take, so the reader cannot die with the session
 * looking alive.
 */
class FrameDepthTest {
    private lateinit var peer: Peer

    @AfterTest
    fun stop() {
        if (::peer.isInitialized) peer.shutdown("done")
    }

    /** Starts this side with `ping` answering "ok"; returns the scripted far end. */
    private fun start(): Far {
        val (here, there) = MemoryChannel.pair()
        peer = Peer(
            here,
            Options(
                dispatch = { _, method, _ -> if (method == "ping") "ok" else throw IllegalArgumentException("no method $method") },
                endpoint = Endpoint("conformance", "main"),
            ),
        )
        val far = Far(there)
        peer.start()
        far.next() // this side's greeting
        far.send("""{"op":"hello","version":5,"endpoint":"main"}""")
        peer.readyFuture.get(5, TimeUnit.SECONDS)
        return far
    }

    private fun invoke(id: String, depth: Int): String =
        """{"op":"invoke","id":"$id","path":[],"target":"t","method":"ping","args":{"type":"data","value":${nested(depth)}}}"""

    private fun reply(id: String, depth: Int): String =
        """{"op":"return","id":"$id","value":{"type":"data","value":${nested(depth)}}}"""

    @Test
    fun aFrameAtTheLimitIsReadAndOneOverItFailsItsCall() {
        val far = start()
        // 206 arrays plus the frame and its args are 208 levels: the limit.
        far.send(invoke("main:1", 206))
        val accepted = far.next()
        assertEquals("return", accepted["op"]?.jsonPrimitive?.content, "$accepted")
        assertEquals("main:1", accepted["id"]?.jsonPrimitive?.content, "$accepted")

        // One more array is 209: only this call fails.
        far.send(invoke("main:2", 207))
        val refused = far.next()
        assertEquals("throw", refused["op"]?.jsonPrimitive?.content, "$refused")
        assertEquals("main:2", refused["id"]?.jsonPrimitive?.content, "$refused")
        val error = refused["error"]?.jsonObject ?: throw AssertionError("no error in $refused")
        assertEquals(
            "frame nested deeper than 208 levels: a value crossing a session may nest at most 64 levels",
            error["message"]?.jsonPrimitive?.content,
        )

        // The session goes on, and the reader is still there.
        far.send("""{"op":"invoke","id":"main:3","path":[],"target":"t","method":"ping","args":{"type":"data","value":[]}}""")
        val after = far.next()
        assertEquals("return", after["op"]?.jsonPrimitive?.content, "$after")
        assertEquals("main:3", after["id"]?.jsonPrimitive?.content, "$after")
        assertNull(peer.error)
    }

    @Test
    fun theCallOfADeepFrameIsReadWhereverItsFieldsAre() {
        val far = start()
        // The value comes before the fields that name the call.
        far.send("""{"args":{"type":"data","value":${nested(1000)}},"method":"ping","target":"t","path":[],"op":"invoke","id":"main:1"}""")
        val refused = far.next()
        assertEquals("throw", refused["op"]?.jsonPrimitive?.content, "$refused")
        assertEquals("main:1", refused["id"]?.jsonPrimitive?.content, "$refused")
        assertNull(peer.error)
    }

    @Test
    fun aFrameTooDeepForAnyStackFailsItsCallInsteadOfTheReader() {
        val far = start()
        // 100,000 arrays: deeper than any reader's stack, and the frame that
        // used to kill the reader with StackOverflowError.
        far.send(invoke("main:1", 100_000))
        val refused = far.next()
        assertEquals("throw", refused["op"]?.jsonPrimitive?.content, "$refused")
        assertEquals("main:1", refused["id"]?.jsonPrimitive?.content, "$refused")
        // The reader is still there: the next call is read and answered.
        far.send("""{"op":"invoke","id":"main:2","path":[],"target":"t","method":"ping","args":{"type":"data","value":[]}}""")
        val after = far.next()
        assertEquals("return", after["op"]?.jsonPrimitive?.content, "$after")
        assertEquals("main:2", after["id"]?.jsonPrimitive?.content, "$after")
        assertNull(peer.error)
    }

    @Test
    fun aReplyOverTheLimitFailsOnlyTheCallWaitingForIt() {
        val far = start()
        val waiter = peer.send("t", "ping", emptyList(), true)
        val request = far.next()
        val id = request["id"]?.jsonPrimitive?.content ?: throw AssertionError("no id in $request")
        // 207 arrays plus the frame and the wire value are 209 levels.
        far.send(reply(id, 207))
        val failure = runBlocking {
            assertFailsWith<ValueException> { waiter() }
        }
        assertEquals(
            "frame nested deeper than 208 levels: a value crossing a session may nest at most 64 levels",
            failure.message,
        )
        assertNull(peer.error)

        // The session goes on.
        far.send("""{"op":"invoke","id":"main:1","path":[],"target":"t","method":"ping","args":{"type":"data","value":[]}}""")
        val after = far.next()
        assertEquals("return", after["op"]?.jsonPrimitive?.content, "$after")
        assertEquals("main:1", after["id"]?.jsonPrimitive?.content, "$after")
    }

    @Test
    fun aFrameOverTheLimitThatNamesNoCallEndsTheSession() {
        val far = start()
        // No id: this frame belongs to no call.
        far.send("""{"op":"call","path":[],"reference":1,"method":"m","args":{"type":"data","value":${nested(207)}}}""")
        peer.closedFuture.get(5, TimeUnit.SECONDS)
        val gone = peer.error as? HostGoneException ?: throw AssertionError("ended with ${peer.error}")
        assertTrue(
            (gone.message ?: "").contains("frame nested deeper than 208 levels"),
            gone.message ?: "",
        )
    }

    @Test
    fun aFrameOverTheLimitFromAnotherEndpointEndsTheSession() {
        val far = start()
        // An id another endpoint would use: this frame names no call of the far end's.
        far.send(invoke("other:1", 207))
        peer.closedFuture.get(5, TimeUnit.SECONDS)
        val gone = peer.error as? HostGoneException ?: throw AssertionError("ended with ${peer.error}")
        assertTrue(
            (gone.message ?: "").contains("frame nested deeper than 208 levels"),
            gone.message ?: "",
        )
    }
}
