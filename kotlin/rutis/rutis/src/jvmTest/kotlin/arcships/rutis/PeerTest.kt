package arcships.rutis

import arcships.rutis.internal.peer.CallChain
import arcships.rutis.internal.peer.Endpoint
import arcships.rutis.internal.peer.Exported
import arcships.rutis.internal.peer.ExportedObject
import arcships.rutis.internal.peer.MAX_MESSAGE
import arcships.rutis.internal.peer.MemoryChannel
import arcships.rutis.internal.peer.Options
import arcships.rutis.internal.peer.Peer
import arcships.rutis.internal.peer.Signal
import arcships.rutis.internal.peer.Undefined
import arcships.rutis.internal.peer.ValueException
import arcships.rutis.internal.peer.WireSet
import arcships.rutis.internal.peer.decodeError
import arcships.rutis.internal.peer.encodeError
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.async
import kotlinx.coroutines.awaitAll
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.delay
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeout
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.int
import kotlinx.serialization.json.jsonArray
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import kotlinx.serialization.json.put
import java.math.BigInteger
import java.time.Instant
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicReference
import kotlin.test.AfterTest
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertTrue
import arcships.rutis.advanced.RemoteFunction

/** An exception with a code and data, as a plugin writes one. */
class Stormy(val code: String, val data: Any?) : RuntimeException("too windy")

/** An object with methods: it crosses only to a far end that takes objects. */
class Counter : ExportedObject {
    override val original: Any get() = this
    override suspend fun call(method: String, args: List<Any?>): Any? = 1
    override suspend fun get(property: String): Any? = Undefined
}

/**
 * The session (design §4): a host and a runtime side over memory channels,
 * one thread per incoming call.
 */
class PeerTest {
    private val aborted = AtomicBoolean(false)
    private val seenChain = AtomicReference<List<String>>()
    private lateinit var host: Peer
    private lateinit var runtime: Peer

    private suspend fun serve(target: String, method: String, args: List<Any?>): Any? = when (method) {
        "echo" -> args.firstOrNull() ?: Undefined
        "apply" -> runtime.callFunctionSuspend(args[0] as RemoteFunction, listOf(args[1]))
        "applyBlocking" -> runtime.callFunction(args[0] as RemoteFunction, listOf(args[1]))
        "abortable" -> {
            try {
                (args[0] as Signal).await()
            } finally {
                aborted.set(true)
            }
        }
        "chain" -> withContext(Dispatchers.IO) {
            // The chain follows the coroutine onto another thread.
            seenChain.set(CallChain.current())
            runtime.callSuspend("", "inner", emptyList(), true)
        }
        "fail" -> throw Stormy("Stormy", mapOf("wind" to (1L shl 60), "gusts" to listOf(1, 2)))
        "rethrow" -> runtime.callFunctionSuspend(args[0] as RemoteFunction, emptyList())
        "rethrowBlocking" -> runtime.callFunction(args[0] as RemoteFunction, emptyList())
        "function" -> Exported("double") { a -> ((a[0] as JsonPrimitive).content.toLong() * 2) }
        "slow" -> {
            delay(20)
            args.firstOrNull()
        }
        "big" -> "x".repeat((args[0] as JsonPrimitive).content.toInt())
        "applyBig" -> runtime.callFunctionSuspend(args[0] as RemoteFunction, listOf("x".repeat((args[1] as JsonPrimitive).content.toInt())))
        "object" -> Counter()
        else -> throw IllegalArgumentException("no method $method")
    }

    private fun start(): Pair<Peer, Peer> {
        val (a, b) = MemoryChannel.pair()
        runtime = Peer(a, Options(dispatch = ::serve, endpoint = Endpoint("kotlin", "main")))
        host = Peer(b, Options(endpoint = Endpoint("main", "kotlin"), plugins = true, dispatch = { _, method, _ ->
            if (method == "inner") CallChain.current() else throw IllegalArgumentException("host has no $method")
        }))
        runtime.start()
        host.start()
        host.readyFuture.get(5, TimeUnit.SECONDS)
        runtime.readyFuture.get(5, TimeUnit.SECONDS)
        return host to runtime
    }

    @AfterTest
    fun stop() {
        if (::host.isInitialized) host.shutdown("done")
    }

    @Test
    fun valuesCrossUnchanged() {
        start()
        val values = listOf(
            JsonPrimitive("a\nb ✓"), JsonPrimitive(9007199254740991), Instant.parse("2026-10-15T03:22:15.123456789Z"),
            BigInteger("-123456789012345678901"), WireSet(listOf(JsonPrimitive(1), JsonPrimitive("1"))), listOf(Instant.EPOCH, JsonPrimitive(1)),
        )
        for (value in values) assertEquals(value, host.call("t", "echo", listOf(value), true))
    }

    @Test
    fun aCallbackRunsDuringTheSyncCallThatPassedIt() {
        start()
        val double = Exported("double") { a -> (a[0] as JsonPrimitive).content.toLong() * 2 }
        assertEquals(JsonPrimitive(42), host.call("t", "apply", listOf(double, 21L), true))
        assertEquals(JsonPrimitive(42), host.call("t", "applyBlocking", listOf(double, 21L), true))
        // Many at once: each incoming call has a thread of its own.
        runBlocking {
            val replies = (1..50).map { n -> async(Dispatchers.IO) { host.callSuspend("t", "apply", listOf(double, n.toLong()), false) } }.awaitAll()
            assertEquals((1..50).map { JsonPrimitive(it * 2L) }, replies)
        }
    }

    @Test
    fun aReceivedFunctionIsCalledAndReleased() {
        start()
        val function = host.call("t", "function", emptyList(), true) as RemoteFunction
        assertEquals(JsonPrimitive(10), host.callFunction(function, listOf(5L)))
        assertEquals(1, runtime.references().first)
        function.release()
        val deadline = System.currentTimeMillis() + 5000
        while (runtime.references().first != 0 && System.currentTimeMillis() < deadline) Thread.sleep(5)
        assertEquals(0, runtime.references().first, "the export is gone once released")
    }

    @Test
    fun cancellingAnAsyncCallAbortsItsSignal() = runBlocking<Unit> {
        start()
        // A signal argument is the wire's `signal`.
        val call = async(Dispatchers.IO) {
            host.callSuspend("t", "abortable", listOf(arcships.rutis.internal.peer.SignalArgument), false)
        }
        delay(100)
        call.cancel()
        withTimeout(5000) { while (!aborted.get()) delay(5) }
        assertTrue(aborted.get())
    }

    @Test
    fun theChainFollowsCoroutinesAcrossThreads() {
        start()
        val inner = host.call("t", "chain", emptyList(), true)
        val chain = seenChain.get()
        assertEquals(1, chain.size, "the incoming call's id: $chain")
        // The nested call carried the chain: the host saw its own call in it.
        assertEquals(JsonArray(listOf(JsonPrimitive(chain[0]), JsonPrimitive("kotlin:1"))), inner)
    }

    @Test
    fun errorsCrossWithCodeAndDataAndAgainWhenThrownOn() {
        start()
        val error = assertFailsWith<RutisException> { host.call("t", "fail", emptyList(), true) }
        assertEquals("Stormy", error.name)
        assertEquals("too windy", error.message)
        assertEquals("Stormy", error.code)
        assertEquals(mapOf("wind" to BigInteger.ONE.shiftLeft(60), "gusts" to listOf(1L, 2L)), error.data)
        // A received error thrown on crosses whole.
        val thrower = Exported("thrower") { throw error }
        val again = assertFailsWith<RutisException> { host.call("t", "rethrow", listOf(thrower), true) }
        assertEquals("Stormy", again.name)
        assertEquals("Stormy", again.code)
        assertEquals(error.data, again.data)
    }

    /**
     * #677: an error crosses with its real JVM stack, "Name: message" and its
     * frames, the first the function that threw it, without the SDK's
     * dispatch below the plugin's method.
     */
    @Test
    fun errorsCrossWithTheirJvmStack() {
        val thrown = assertFailsWith<IllegalStateException> { throwHere() }
        val encoded = encodeError(thrown)
        val graph = encoded["graph"]!!.jsonObject
        val rootId = graph["root"]!!.jsonObject["value"]!!.jsonPrimitive.int
        val stack = graph["nodes"]!!.jsonArray[rootId].jsonObject["stack"]!!.jsonPrimitive.content
        val lines = stack.lines()
        assertEquals("IllegalStateException: broken here", lines[0])
        assertTrue(lines[1].startsWith("    at arcships.rutis.PeerTest.throwHere(PeerTest.kt:"), stack)

        // Across a session: the plugin's method first, no SDK frames under it.
        start()
        val error = assertFailsWith<RutisException> { host.call("t", "fail", emptyList(), true) }
        val far = error.graph!!
        val farRoot = far["root"]!!.jsonObject["value"]!!.jsonPrimitive.int
        val farStack = far["nodes"]!!.jsonArray[farRoot].jsonObject["stack"]!!.jsonPrimitive.content
        val farLines = farStack.lines()
        assertEquals("Stormy: too windy", farLines[0])
        assertTrue(farLines[1].startsWith("    at arcships.rutis.PeerTest.serve(PeerTest.kt:"), farStack)
        assertTrue(farLines.none { it.contains("arcships.rutis.internal.") }, farStack)
    }

    private fun throwHere(): Nothing = throw IllegalStateException("broken here")

    @Test
    fun aValueTooDeepFailsOnlyItsCall() {
        start()
        var deep: Any? = JsonPrimitive("bottom")
        repeat(65) { deep = listOf(deep, Instant.EPOCH) }
        val error = assertFailsWith<ValueException> { host.call("t", "echo", listOf(deep), true) }
        assertTrue(error.message!!.contains("64 levels"), error.message)
        assertTrue(error.message!!.startsWith("t.echo: argument 1 cannot be sent: "), error.message)
        assertEquals(JsonPrimitive("after"), host.call("t", "echo", listOf(JsonPrimitive("after")), true))
    }

    /**
     * #675: a message over 16 MiB fails only its call, on the side that would
     * send it, naming the call, the argument (or the result), the size and the
     * limit; the session goes on.
     */
    @Test
    fun aMessageOverTheLimitFailsOnlyItsCall() {
        start()
        val over = MAX_MESSAGE + 10
        val limit = "over the limit of 16777216 bytes (16 MiB)"
        // An argument this side would send.
        val sent = assertFailsWith<ValueException> {
            host.call("host:t\u0000scope", "echo", listOf(JsonPrimitive(1), "x".repeat(over)), true)
        }
        assertTrue(sent.message!!.startsWith("t.echo: argument 2 cannot be sent: the message would be "), sent.message)
        assertTrue(sent.message!!.endsWith(limit), sent.message)
        // guarantee: sdk/error-texts (code bad-value, data {argument, part, limit}: #677)
        assertEquals("bad-value", sent.code)
        assertEquals(mapOf("part" to "argument", "argument" to 2, "limit" to MAX_MESSAGE), sent.data)
        // A result the far end would send.
        val result = assertFailsWith<RutisException> { host.call("t", "big", listOf(over.toLong()), true) }
        assertTrue(result.message.startsWith("t.big: the result cannot be sent: the message would be "), result.message)
        assertTrue(result.message.endsWith(limit), result.message)
        assertEquals("bad-value", result.code)
        // An argument the far end would send to a function of this side, which never runs.
        val called = AtomicBoolean(false)
        val callback = Exported("callback") { called.set(true) }
        val argument = assertFailsWith<RutisException> { host.call("t", "applyBig", listOf(callback, over.toLong()), true) }
        assertTrue(argument.message.startsWith("a function: argument 1 cannot be sent: the message would be "), argument.message)
        assertTrue(!called.get(), "the function ran")
        assertEquals(JsonPrimitive("after"), host.call("t", "echo", listOf(JsonPrimitive("after")), true))
        assertEquals(null, host.error)
        assertEquals(null, runtime.error)
    }

    /**
     * #675: an object with methods crosses only to a far end that declared
     * `objects`: to any other, only its call fails, naming the far end.
     */
    // objects-case(kotlin): no-objects-declared
    @Test
    fun anObjectGoesOnlyToAFarEndThatTakesObjects() {
        val (a, b) = MemoryChannel.pair()
        runtime = Peer(a, Options(dispatch = ::serve, endpoint = Endpoint("kotlin", "main")))
        // A far end older than 0.18: it does not declare `objects`.
        host = Peer(b, Options(endpoint = Endpoint("main", "kotlin"), capabilities = arcships.rutis.internal.peer.SESSION_CAPABILITIES - "objects"))
        runtime.start()
        host.start()
        host.readyFuture.get(5, TimeUnit.SECONDS)
        runtime.readyFuture.get(5, TimeUnit.SECONDS)
        val error = assertFailsWith<RutisException> { host.call("t", "object", emptyList(), true) }
        assertEquals(
            "t.object: the result cannot be sent: an object with methods (Counter) cannot cross to main; pass plain data or a function instead, or upgrade main to rutis 0.18",
            error.message,
        )
        assertEquals(JsonPrimitive("after"), host.call("t", "echo", listOf(JsonPrimitive("after")), true))
        assertEquals(0, runtime.references().first, "the object is left exported")
    }

    /**
     * #675: a thrown value that is not an error (a JavaScript `throw 'a string'`)
     * crosses again whole when thrown on: its name, message and graph, whose
     * nodes stay a list.
     */
    @Test
    fun aRelayedNonErrorThrowKeepsItsGraph() {
        start()
        val graph = buildJsonObject {
            put("root", buildJsonObject {
                put("type", "data")
                put("value", "a string")
            })
            put("nodes", JsonArray(emptyList()))
        }
        val thrown = RutisException("ThrownValue", "a string").also { it.graph = graph }
        val thrower = Exported("thrower") { throw thrown }
        val relayed = assertFailsWith<RutisException> { host.call("t", "rethrow", listOf(thrower), true) }
        assertEquals("ThrownValue", relayed.name)
        assertEquals("a string", relayed.message)
        assertEquals(graph, relayed.graph)
        assertEquals(JsonPrimitive("after"), host.call("t", "echo", listOf(JsonPrimitive("after")), true))
    }

    private fun rootNode(graph: JsonObject): JsonObject {
        val id = graph["root"]!!.jsonObject["value"]!!.jsonPrimitive.int
        return graph["nodes"]!!.jsonArray[id].jsonObject
    }

    private fun rethrownOf(encoded: JsonObject): List<String> =
        rootNode(encoded["graph"]!!.jsonObject)["rethrown"]?.jsonArray?.map { it.jsonPrimitive.content } ?: emptyList()

    /**
     * #677: a received error thrown on crosses with the received graph as it
     * came, plus one `rethrown` item on its root: the frames where it was
     * thrown on, without "Name: message". The kept graph is not changed.
     */
    @Test
    fun aReceivedErrorThrownOnCarriesWhereItWasThrownOn() {
        start()
        val error = assertFailsWith<RutisException> { host.call("t", "fail", emptyList(), true) }
        val kept = error.graph!!
        assertEquals(null, rootNode(kept)["rethrown"])

        // Once: thrown on here, from this test.
        val once = rethrownOf(encodeError(error))
        assertEquals(1, once.size)
        assertTrue(once[0].startsWith("    at "), once[0])
        assertTrue(!once[0].contains("Stormy"), once[0])
        assertTrue(once[0].contains("PeerTest.aReceivedErrorThrownOnCarriesWhereItWasThrownOn(PeerTest.kt:"), once[0])
        assertTrue(once[0].lines().none { it.contains("arcships.rutis.internal.") }, once[0])
        assertEquals(kept, error.graph)
        assertEquals(null, rootNode(error.graph!!)["rethrown"])

        // Twice: thrown on by the host's function, received by the runtime
        // (from a blocking call in `serve`) and thrown on again.
        val thrower = Exported("thrower") { throw error }
        val again = assertFailsWith<RutisException> { host.call("t", "rethrowBlocking", listOf(thrower), true) }
        val twice = rootNode(again.graph!!)["rethrown"]!!.jsonArray.map { it.jsonPrimitive.content }
        assertEquals(2, twice.size, twice.toString())
        assertTrue(twice[0].contains("PeerTest.aReceivedErrorThrownOnCarriesWhereItWasThrownOn(PeerTest.kt:"), twice[0])
        assertTrue(twice[1].startsWith("    at arcships.rutis.PeerTest.serve(PeerTest.kt:"), twice[1])
        assertEquals("Stormy", again.name)
        assertEquals(error.data, again.data)
        assertEquals(kept, error.graph)

        // From a suspend function: its frames are the suspended callers'.
        val suspended = assertFailsWith<RutisException> { host.call("t", "rethrow", listOf(thrower), true) }
        val viaSuspend = rootNode(suspended.graph!!)["rethrown"]!!.jsonArray.map { it.jsonPrimitive.content }
        assertEquals(2, viaSuspend.size, viaSuspend.toString())
        assertTrue(viaSuspend[1].startsWith("    at arcships.rutis.PeerTest.serve(PeerTest.kt:"), viaSuspend[1])
        assertTrue(viaSuspend[1].lines().none { it.contains("arcships.rutis.internal.") }, viaSuspend[1])
    }

    /** At most 8 `rethrown` items: a ninth is not appended. */
    @Test
    fun rethrownStopsAtEightItems() {
        fun received(items: Int) = decodeError(buildJsonObject {
            put("name", "Stormy")
            put("message", "too windy")
            put("graph", buildJsonObject {
                put("root", buildJsonObject {
                    put("type", "reference")
                    put("value", 0)
                })
                put("nodes", JsonArray(listOf(buildJsonObject {
                    put("type", "error")
                    put("name", "Stormy")
                    put("message", "too windy")
                    put("stack", "Stormy: too windy")
                    put("rethrown", JsonArray((1..items).map { JsonPrimitive("    at caller$it") }))
                })))
            })
        })
        val seven = received(7)
        val before = seven.graph
        val eight = rethrownOf(encodeError(seven))
        assertEquals(8, eight.size)
        assertEquals("    at caller7", eight[6])
        assertTrue(eight[7].contains("PeerTest.rethrownStopsAtEightItems"), eight[7])
        assertEquals(before, seven.graph)
        assertEquals(7, rootNode(seven.graph!!)["rethrown"]!!.jsonArray.size)
        val full = rethrownOf(encodeError(received(8)))
        assertEquals((1..8).map { "    at caller$it" }, full)
    }

    /** A locally thrown error has no `rethrown`. */
    @Test
    fun aLocalErrorHasNoRethrown() {
        val thrown = assertFailsWith<IllegalStateException> { throwHere() }
        assertEquals(emptyList(), rethrownOf(encodeError(thrown)))
    }
}
