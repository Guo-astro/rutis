// guarantee: bridge/unknown-fields
package arcships.rutis

import arcships.rutis.internal.peer.Channel
import arcships.rutis.internal.peer.Endpoint
import arcships.rutis.internal.peer.Exported
import arcships.rutis.internal.peer.IncompatibleException
import arcships.rutis.internal.peer.MemoryChannel
import arcships.rutis.internal.peer.Options
import arcships.rutis.internal.peer.Peer
import arcships.rutis.internal.peer.decodeError
import arcships.rutis.internal.peer.unknownFields
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import kotlinx.serialization.json.long
import java.util.concurrent.CompletableFuture
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicInteger
import kotlin.test.AfterTest
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNull
import kotlin.test.assertTrue

/**
 * The far end of a session scripted frame by frame (protocol §9): it writes
 * what the test wrote and reads what this side answers.
 */
private class Scripted(private val channel: Channel) {
    fun send(frame: String) = channel.send(frame.toByteArray(Charsets.UTF_8))

    /** The next frame this side wrote, or the test fails instead of waiting on. */
    fun next(): JsonObject {
        val message = CompletableFuture.supplyAsync {
            channel.recv() ?: throw IllegalStateException("the session ended")
        }.get(5, TimeUnit.SECONDS)
        return Json.parseToJsonElement(String(message, Charsets.UTF_8)).jsonObject
    }

    /** The next frame, which is the reply to call `id`. */
    fun replyTo(id: String): JsonObject = next().also {
        assertEquals(id, it["id"]?.jsonPrimitive?.content, "the reply's call")
    }
}

/**
 * A frame field this side does not know (#635, protocol §9): a greeting with
 * one fails the handshake, a call or a reply with one fails only its call,
 * and `release`/`cancel`, which have no call to fail, are read as if the
 * extra fields were not there. A malformed frame still ends the session.
 */
class UnknownFieldsTest {
    private lateinit var peer: Peer

    @AfterTest
    fun stop() {
        if (::peer.isInitialized) peer.shutdown("done")
    }

    /** A session this side serves `dispatch` on, greeting far end `main`. */
    private fun start(dispatch: suspend (String, String, List<Any?>) -> Any?): Scripted {
        val (here, there) = MemoryChannel.pair()
        peer = Peer(here, Options(dispatch = dispatch, endpoint = Endpoint("conformance", "main")))
        val far = Scripted(there)
        peer.start()
        far.next() // this side's greeting
        far.send("""{"op":"hello","version":5,"endpoint":"main"}""")
        peer.readyFuture.get(5, TimeUnit.SECONDS)
        return far
    }

    /** What a `throw` frame says: a `BindingError` with its code and data. */
    private fun thrown(frame: JsonObject): RutisException {
        assertEquals("throw", frame["op"]?.jsonPrimitive?.content, "the frame is a throw: $frame")
        assertEquals("BindingError", frame["error"]?.jsonObject?.get("name")?.jsonPrimitive?.content, "$frame")
        return decodeError(frame["error"])
    }

    /** Runs `block` on a thread of its own; the future holds what it threw, or null. */
    private fun failureOf(block: () -> Any?): CompletableFuture<Throwable?> {
        val failure = CompletableFuture<Throwable?>()
        Thread({
            failure.complete(
                try {
                    block()
                    null
                } catch (e: Throwable) {
                    e
                },
            )
        }, "test-call").apply { isDaemon = true }.start()
        return failure
    }

    @Test
    fun anUnknownFieldInACallFailsOnlyThatCall() {
        val ran = AtomicInteger()
        val far = start { _, method, _ ->
            ran.incrementAndGet()
            method
        }

        // A field at the top of the frame.
        far.send("""{"op":"invoke","id":"main:1","path":[],"target":"t","method":"m","args":{"type":"data","value":[]},"trace":"x"}""")
        val top = thrown(far.replyTo("main:1"))
        assertEquals("unknown field \"trace\" in invoke", top.message)
        assertEquals("bad-frame", top.code)
        assertEquals(mapOf("op" to "invoke", "fields" to listOf("trace")), top.data)

        // A field inside an argument: named by its path in the value.
        far.send("""{"op":"invoke","id":"main:2","path":[],"target":"t","method":"m","args":{"type":"record","value":{"x":{"type":"undefined","extra":1}}}}""")
        val nested = thrown(far.replyTo("main:2"))
        assertEquals("unknown field \"args.value.x.extra\" in invoke", nested.message)
        assertEquals("bad-frame", nested.code)
        assertEquals(mapOf("op" to "invoke", "fields" to listOf("args.value.x.extra")), nested.data)
        assertEquals(0, ran.get(), "neither call ran")

        // The session goes on: the next call runs and is answered.
        far.send("""{"op":"invoke","id":"main:3","path":[],"target":"t","method":"m","args":{"type":"data","value":[]}}""")
        val replied = far.replyTo("main:3")
        assertEquals("return", replied["op"]?.jsonPrimitive?.content)
        assertEquals("m", (replied["value"] as JsonObject)["value"]?.jsonPrimitive?.content)
        assertEquals(1, ran.get())
        assertNull(peer.error)
    }

    @Test
    fun anUnknownFieldInAReplyFailsOnlyTheCallWaitingForIt() {
        val far = start { _, _, args -> args.firstOrNull() }

        // A reply with an extra field: the call waiting for it fails, the session goes on.
        val returned = failureOf { peer.call("t", "echo", listOf(JsonPrimitive("a")), true) }
        val first = far.next()
        assertEquals("invoke", first["op"]?.jsonPrimitive?.content)
        far.send("""{"op":"return","id":"${first["id"]!!.jsonPrimitive.content}","value":{"type":"data","value":"a"},"extra":1}""")
        val failed = returned.get(5, TimeUnit.SECONDS) as? RutisException
        assertEquals("unknown field \"extra\" in return", failed?.message, "the call failed with $failed")
        assertEquals("bad-frame", failed?.code)
        assertEquals(mapOf("op" to "return", "fields" to listOf("extra")), failed?.data)

        // A thrown error with an extra field, the same.
        val thrownOn = failureOf { peer.call("t", "echo", listOf(JsonPrimitive("b")), true) }
        val second = far.next()
        far.send("""{"op":"throw","id":"${second["id"]!!.jsonPrimitive.content}","error":{"name":"Error","message":"boom"},"extra":1}""")
        val refused = thrownOn.get(5, TimeUnit.SECONDS) as? RutisException
        assertEquals("unknown field \"extra\" in throw", refused?.message, "the call failed with $refused")
        assertEquals("bad-frame", refused?.code)
        assertEquals(mapOf("op" to "throw", "fields" to listOf("extra")), refused?.data)

        // The next call is answered as usual.
        val answered = failureOf { peer.call("t", "echo", listOf(JsonPrimitive("c")), true) }
        val third = far.next()
        far.send("""{"op":"return","id":"${third["id"]!!.jsonPrimitive.content}","value":{"type":"data","value":"c"}}""")
        assertNull(answered.get(5, TimeUnit.SECONDS), "the call was answered")
        assertNull(peer.error)
    }

    @Test
    fun releaseAndCancelIgnoreFieldsTheyDoNotKnow() {
        val ran = AtomicInteger()
        val far = start { _, _, _ ->
            Exported("f") {
                ran.incrementAndGet()
                null
            }
        }

        // `cancel` has no call to fail: its extra field is ignored.
        far.send("""{"op":"cancel","id":"main:99","extra":1}""")

        // A function this side grants the far end...
        far.send("""{"op":"invoke","id":"main:1","path":[],"target":"t","method":"function","args":{"type":"undefined"}}""")
        val granted = far.replyTo("main:1")
        assertEquals("return", granted["op"]?.jsonPrimitive?.content)
        val reference = (granted["value"] as JsonObject)["value"]!!.jsonObject["id"]!!.jsonPrimitive.long
        assertEquals(1, peer.references().first)

        // ... released with an extra field: the field is ignored, the release is not.
        far.send("""{"op":"release","reference":$reference,"count":1,"extra":1}""")
        val deadline = System.currentTimeMillis() + 5000
        while (peer.references().first != 0 && System.currentTimeMillis() < deadline) Thread.sleep(5)
        assertEquals(0, peer.references().first, "the release went through")

        // A `call` of another function with an extra field fails only that call.
        far.send("""{"op":"invoke","id":"main:2","path":[],"target":"t","method":"function","args":{"type":"undefined"}}""")
        val again = (far.replyTo("main:2")["value"] as JsonObject)["value"]!!.jsonObject["id"]!!.jsonPrimitive.long
        far.send("""{"op":"call","id":"main:3","path":[],"reference":$again,"args":{"type":"undefined"},"extra":1}""")
        val refused = thrown(far.replyTo("main:3"))
        assertEquals("unknown field \"extra\" in call", refused.message)
        assertEquals("bad-frame", refused.code)
        assertEquals(mapOf("op" to "call", "fields" to listOf("extra")), refused.data)
        assertEquals(0, ran.get(), "the function did not run")

        // The session is open, and a call still runs and gets its reply.
        far.send("""{"op":"call","id":"main:4","path":[],"reference":$again,"args":{"type":"undefined"}}""")
        assertEquals("return", far.replyTo("main:4")["op"]?.jsonPrimitive?.content)
        assertEquals(1, ran.get())
        assertNull(peer.error)
    }

    @Test
    fun anUnknownFieldInHelloFailsTheHandshake() {
        val (here, there) = MemoryChannel.pair()
        peer = Peer(here, Options(dispatch = { _, _, _ -> null }, endpoint = Endpoint("conformance", "main")))
        val far = Scripted(there)
        peer.start()
        far.next() // this side's greeting
        far.send("""{"op":"hello","version":5,"endpoint":"main","extra":1}""")
        peer.closedFuture.get(5, TimeUnit.SECONDS)
        val error = peer.error
        assertTrue(error is IncompatibleException, "ended with $error")
        assertEquals("incompatible session: unknown field \"extra\" in hello", error.message)
    }

    @Test
    fun anUnknownFieldIsCheckedBeforeTheReference() {
        val far = start { _, _, _ -> Exported("f") { null } }

        // A `call` naming a reference this side never granted, with a field
        // this side does not know: the field is the failure of the call, not of
        // the session (§9). The call gets `bad-frame` and the session goes on.
        far.send("""{"op":"call","id":"main:1","path":[],"reference":99,"args":{"type":"undefined"},"extra":1}""")
        val refused = thrown(far.replyTo("main:1"))
        assertEquals("unknown field \"extra\" in call", refused.message)
        assertEquals("bad-frame", refused.code)
        assertEquals(mapOf("op" to "call", "fields" to listOf("extra")), refused.data)
        far.send("""{"op":"invoke","id":"main:2","path":[],"target":"t","method":"m","args":{"type":"undefined"}}""")
        assertEquals("return", far.replyTo("main:2")["op"]?.jsonPrimitive?.content)
        assertNull(peer.error)

        // Without the field the same frame is a session fault, as it was before.
        far.send("""{"op":"call","id":"main:3","path":[],"reference":99,"args":{"type":"undefined"}}""")
        peer.closedFuture.get(5, TimeUnit.SECONDS)
        val ended = peer.error
        assertTrue(ended is HostGoneException, "ended with $ended")
        assertTrue(ended.message!!.contains("unknown, released or mismatched reference"), ended.message!!)
    }

    @Test
    fun anUnknownFieldIsNamedByItsPath() {
        fun unknown(json: String): List<String> = unknownFields(Json.parseToJsonElement(json).jsonObject)

        // A frame's own fields, and those of its values: a list item, a map
        // pair, a record entry, a reference's own fields, an error's.
        assertEquals(listOf("trace"), unknown("""{"op":"invoke","id":"main:1","path":[],"target":"t","method":"m","args":{"type":"data","value":[]},"trace":"x"}"""))
        assertEquals(listOf("args.value[0].x"), unknown("""{"op":"call","id":"main:1","path":[],"reference":1,"args":{"type":"list","value":[{"type":"data","value":1,"x":2}]}}"""))
        assertEquals(listOf("args.value[0][1].x"), unknown("""{"op":"call","id":"main:1","path":[],"reference":1,"args":{"type":"map","value":[[{"type":"data","value":"k"},{"type":"undefined","x":1}]]}}"""))
        assertEquals(listOf("args.value.x"), unknown("""{"op":"invoke","id":"main:1","path":[],"target":"t","method":"m","args":{"type":"reference","value":{"id":1,"home":false,"kind":"function","origin":[],"x":1}}}"""))
        assertEquals(listOf("value.value[0].y"), unknown("""{"op":"return","id":"main:1","value":{"type":"list","value":[{"type":"data","value":1,"y":2}]}}"""))
        assertEquals(listOf("error.trace"), unknown("""{"op":"throw","id":"main:1","error":{"name":"E","message":"m","trace":1}}"""))
        assertEquals(listOf("implementation.extra"), unknown("""{"op":"hello","version":5,"endpoint":"main","implementation":{"name":"n","version":"v","extra":1}}"""))

        // A `data` value is arbitrary JSON and not inspected; an unknown `op`
        // has no table and is left to the usual refusal.
        assertEquals(emptyList(), unknown("""{"op":"return","id":"main:1","value":{"type":"data","value":{"anything":[1,2]}}}"""))
        assertEquals(emptyList(), unknown("""{"op":"frobnicate","x":1}"""))
    }
}
