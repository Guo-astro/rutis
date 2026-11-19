// guarantee: bridge/large-and-deep-values
package arcships.rutis

import arcships.rutis.internal.peer.CHUNK_SIZE
import arcships.rutis.internal.peer.Channel
import arcships.rutis.internal.peer.DEFAULT_MAX_TRANSFER
import arcships.rutis.internal.peer.ENDPOINT_PROTOCOL
import arcships.rutis.internal.peer.Endpoint
import arcships.rutis.internal.peer.LocalChannels
import arcships.rutis.internal.peer.MAX_MESSAGE
import arcships.rutis.internal.peer.MemoryChannel
import arcships.rutis.internal.peer.Options
import arcships.rutis.internal.peer.Peer
import arcships.rutis.internal.peer.SESSION_CAPABILITIES
import arcships.rutis.internal.peer.Undefined
import arcships.rutis.internal.peer.ValueException
import arcships.rutis.internal.peer.formatBytes
import arcships.rutis.internal.peer.maxTransferFrom
import arcships.rutis.internal.peer.parseBytes
import kotlinx.coroutines.runBlocking
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.boolean
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.int
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.put
import org.junit.jupiter.api.Assumptions.assumeTrue
import java.io.ByteArrayOutputStream
import java.io.File
import java.net.StandardProtocolFamily
import java.net.UnixDomainSocketAddress
import java.nio.channels.ServerSocketChannel
import java.nio.file.Files
import java.nio.file.Path
import java.util.concurrent.CompletableFuture
import java.util.concurrent.CopyOnWriteArrayList
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicInteger
import kotlin.test.AfterTest
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertFalse
import kotlin.test.assertNull
import kotlin.test.assertTrue

/**
 * A frame over the transport's message limit, carried as `chunk` frames and
 * reassembled by the far end (#1361, #1363). A chunk is not a call: the method
 * runs once, after the last chunk, and never for a transfer that does not
 * finish.
 */
class ChunkedTest {
    private val peers = ArrayList<Peer>()
    private val farEnds = ArrayList<Channel>()

    @AfterTest
    fun stop() {
        for (peer in peers) runCatching { peer.close(null) }
        for (far in farEnds) runCatching { far.close("done") }
    }

    private class Harness(val peer: Peer, val far: Channel)

    private fun start(
        dispatch: suspend (String, String, List<Any?>) -> Any? = { _, _, _ -> Undefined },
        farCapabilities: List<String> = SESSION_CAPABILITIES,
        channelLimit: Int = MAX_MESSAGE,
        maxTransfer: Long = DEFAULT_MAX_TRANSFER,
        greet: Boolean = true,
    ): Harness {
        val (near, far) = MemoryChannel.pair()
        val peer = Peer(near, Options(dispatch = dispatch, endpoint = Endpoint("kotlin", "main")))
        peer.channelLimit = channelLimit
        peer.maxTransfer = maxTransfer
        peer.start()
        peers.add(peer)
        farEnds.add(far)
        val hello = decode(far.recv()!!)
        assertEquals(ENDPOINT_PROTOCOL, (hello["version"] as JsonPrimitive).int)
        if (greet) {
            far.send(encode(buildJsonObject {
                put("op", "hello")
                put("version", ENDPOINT_PROTOCOL)
                put("endpoint", "main")
                put("implementation", buildJsonObject { put("name", "rutis-test"); put("version", "0.0.0") })
                put("capabilities", JsonArray(farCapabilities.map(::JsonPrimitive)))
            }))
            peer.readyFuture.get(5, TimeUnit.SECONDS)
        }
        return Harness(peer, far)
    }

    private fun why(peer: Peer): String? = peer.error?.cause?.message ?: peer.error?.message

    private fun receive(far: Channel): ByteArray =
        CompletableFuture.supplyAsync { far.recv() }.get(5, TimeUnit.SECONDS)!!

    private fun chunk(transfer: String, seq: Long, last: Boolean, bytes: ByteArray): JsonObject = buildJsonObject {
        put("op", "chunk")
        put("transfer", transfer)
        put("seq", seq)
        put("last", last)
        put("data", formatBytes(bytes))
    }

    private fun parts(text: String, size: Int): List<ByteArray> {
        val bytes = text.toByteArray(Charsets.UTF_8)
        return (bytes.indices step size).map { bytes.copyOfRange(it, minOf(it + size, bytes.size)) }
    }

    private fun data(value: JsonElement): JsonObject = buildJsonObject {
        put("type", "data")
        put("value", value)
    }

    private fun invoke(id: String, method: String, args: JsonElement): JsonObject = buildJsonObject {
        put("op", "invoke")
        put("id", id)
        put("path", JsonArray(emptyList()))
        put("target", "test")
        put("method", method)
        put("args", args)
    }

    private fun answer(id: String, value: JsonElement): ByteArray = encode(buildJsonObject {
        put("op", "return")
        put("id", id)
        put("value", value)
    })

    @Test
    fun aChunkedCallIsDispatchedOnceAfterItsLastChunk() {
        val calls = CopyOnWriteArrayList<List<Any?>>()
        val ran = CountDownLatch(1)
        val harness = start(dispatch = { _, _, args -> calls.add(args); ran.countDown(); args[0] })
        val value = buildJsonObject { put("text", "x".repeat(4096)) }
        val request = invoke("main:1", "echo", buildJsonObject {
            put("type", "list")
            put("value", JsonArray(listOf(data(value))))
        })
        val pieces = parts(encode(request).toString(Charsets.UTF_8), 1024)
        assertTrue(pieces.size > 1, "one frame, many chunks")
        pieces.dropLast(1).forEachIndexed { seq, part -> harness.far.send(encode(chunk("main:1", seq.toLong(), false, part))) }
        assertEquals(0, calls.size, "dispatched before the last chunk")
        harness.far.send(encode(chunk("main:1", (pieces.size - 1).toLong(), true, pieces.last())))
        assertTrue(ran.await(5, TimeUnit.SECONDS), "the call did not run")
        assertEquals(listOf(value), calls[0])
        val reply = decode(receive(harness.far))
        assertEquals("return", (reply["op"] as JsonPrimitive).content)
        assertEquals(data(value), reply["value"])
        assertNull(harness.peer.error)
    }

    @Test
    fun aTransferTheSessionNeverFinishesIsNeverDispatched() {
        val calls = AtomicInteger()
        val ran = CountDownLatch(1)
        val harness = start(dispatch = { _, _, _ -> calls.incrementAndGet(); ran.countDown(); Undefined })
        val request = invoke("main:1", "m", buildJsonObject { put("type", "list"); put("value", JsonArray(emptyList())) })
        val pieces = parts(encode(request).toString(Charsets.UTF_8), 16)
        pieces.dropLast(1).forEachIndexed { seq, part -> harness.far.send(encode(chunk("main:1", seq.toLong(), false, part))) }
        harness.peer.close(HostGoneException("the far end disconnected midway"))
        assertFalse(ran.await(200, TimeUnit.MILLISECONDS), "a call was dispatched from an incomplete transfer")
        assertEquals(0, calls.get())
    }

    @Test
    fun aChunkOutOfOrderEndsTheSession() {
        val harness = start()
        harness.far.send(encode(chunk("main:1", 1, false, "{".toByteArray())))
        harness.peer.closedFuture.get(5, TimeUnit.SECONDS)
        assertTrue(why(harness.peer)!!.startsWith("chunked transfer main:1: expected chunk 0, got 1"), why(harness.peer))
    }

    @Test
    fun aSecondTransferBeforeTheFirstEndedEndsTheSession() {
        val harness = start()
        harness.far.send(encode(chunk("main:1", 0, false, "{\"op\"".toByteArray())))
        harness.far.send(encode(chunk("main:2", 0, false, "{".toByteArray())))
        harness.peer.closedFuture.get(5, TimeUnit.SECONDS)
        assertTrue(
            why(harness.peer)!!.startsWith("chunked transfer main:1: transfer main:2 started before it ended"),
            why(harness.peer),
        )
    }

    @Test
    fun aChunkBeforeTheGreetingEndsTheSession() {
        val harness = start(greet = false)
        harness.far.send(encode(chunk("main:1", 0, true, "{}".toByteArray())))
        harness.peer.closedFuture.get(5, TimeUnit.SECONDS)
        assertTrue(why(harness.peer)!!.startsWith("request before protocol handshake"), why(harness.peer))
    }

    @Test
    fun aChunkFromAFarEndThatDidNotDeclareChunkedEndsTheSession() {
        val harness = start(farCapabilities = listOf("reentrant-sync"))
        harness.far.send(encode(chunk("main:1", 0, true, "{}".toByteArray())))
        harness.peer.closedFuture.get(5, TimeUnit.SECONDS)
        assertTrue(
            why(harness.peer)!!.startsWith("chunk frame from a far end that did not declare chunked"),
            why(harness.peer),
        )
    }

    @Test
    fun aChunkWithInvalidBase64EndsTheSession() {
        val harness = start()
        harness.far.send(encode(buildJsonObject {
            put("op", "chunk")
            put("transfer", "main:1")
            put("seq", 0)
            put("last", true)
            put("data", "not base64!")
        }))
        harness.peer.closedFuture.get(5, TimeUnit.SECONDS)
        assertTrue(
            why(harness.peer)!!.startsWith("chunked transfer main:1: invalid bytes (11 characters)"),
            why(harness.peer),
        )
    }

    @Test
    fun aTransferOverTheReassemblyLimitEndsTheSession() {
        assertEquals(256L * 1024 * 1024, DEFAULT_MAX_TRANSFER)
        assertEquals(8 * 1024 * 1024, CHUNK_SIZE)
        val harness = start(maxTransfer = 1024)
        harness.far.send(encode(chunk("main:1", 0, false, ByteArray(1024))))
        harness.far.send(encode(chunk("main:1", 1, true, "{".toByteArray())))
        harness.peer.closedFuture.get(5, TimeUnit.SECONDS)
        assertTrue(
            why(harness.peer)!!.startsWith("chunked transfer main:1: over the limit of 1024 bytes"),
            why(harness.peer),
        )
    }

    @Test
    fun aFrameOverTheMessageLimitIsSentAsChunksThatFitIt() {
        val harness = start(channelLimit = 4096)
        val big = "x".repeat(8192)
        val waiting = harness.peer.send("host:svc", "take", listOf(JsonPrimitive(big)), false)
        val frames = ArrayList<JsonObject>()
        while (true) {
            val bytes = receive(harness.far)
            val frame = decode(bytes)
            assertEquals("chunk", (frame["op"] as JsonPrimitive).content)
            assertTrue(bytes.size <= 4096, "chunk of ${bytes.size} bytes, over the limit")
            frames.add(frame)
            if ((frame["last"] as JsonPrimitive).boolean) break
        }
        val transfer = (frames[0]["transfer"] as JsonPrimitive).content
        frames.forEachIndexed { seq, frame ->
            assertEquals(transfer, (frame["transfer"] as JsonPrimitive).content)
            assertEquals(seq.toLong(), (frame["seq"] as JsonPrimitive).content.toLong())
            assertEquals(seq == frames.size - 1, (frame["last"] as JsonPrimitive).boolean)
        }
        val request = decode(frames.map { parseBytes((it["data"] as JsonPrimitive).content) }.reduce { a, b -> a + b })
        assertEquals("invoke", (request["op"] as JsonPrimitive).content)
        assertEquals(JsonPrimitive(big), ((request["args"] as JsonObject)["value"] as JsonArray)[0])
        harness.far.send(answer((request["id"] as JsonPrimitive).content, data(JsonPrimitive("ok"))))
        assertEquals(JsonPrimitive("ok"), runBlocking { waiting() })
        assertNull(harness.peer.error)
    }

    @Test
    fun aResultOverTheMessageLimitIsSentAsChunksToo() {
        val big = "y".repeat(8192)
        val harness = start(dispatch = { _, _, _ -> big }, channelLimit = 4096)
        harness.far.send(encode(invoke("main:1", "big", buildJsonObject {
            put("type", "list")
            put("value", JsonArray(emptyList()))
        })))
        val frames = ArrayList<JsonObject>()
        while (true) {
            val frame = decode(receive(harness.far))
            assertEquals("chunk", (frame["op"] as JsonPrimitive).content)
            frames.add(frame)
            if ((frame["last"] as JsonPrimitive).boolean) break
        }
        assertTrue(frames.size > 1)
        val reply = decode(frames.map { parseBytes((it["data"] as JsonPrimitive).content) }.reduce { a, b -> a + b })
        assertEquals("return", (reply["op"] as JsonPrimitive).content)
        assertEquals(data(JsonPrimitive(big)), reply["value"])
        assertNull(harness.peer.error)
    }

    @Test
    fun aFarEndWithoutChunkedStillGetsTheOverLimitError() {
        val harness = start(farCapabilities = listOf("reentrant-sync"), channelLimit = 4096)
        val error = assertFailsWith<ValueException> {
            harness.peer.send("host:svc", "take", listOf(JsonPrimitive("x".repeat(8192))), false)
        }
        assertTrue(error.message!!.startsWith("svc.take: argument 1 cannot be sent: the message would be "), error.message)
        assertTrue(error.message!!.endsWith("over the limit of 4096 bytes"), error.message)
        assertEquals(4096, error.data?.get("limit"))
        assertNull(harness.peer.error)
    }

    @Test
    fun aFrameOverTheReassemblyLimitFailsOnlyItsCall() {
        val harness = start(channelLimit = 4096, maxTransfer = 2048)
        val error = assertFailsWith<ValueException> {
            harness.peer.send("host:svc", "take", listOf(JsonPrimitive("x".repeat(8192))), false)
        }
        assertTrue(error.message!!.endsWith("over the limit of 2048 bytes"), error.message)
        assertEquals(2048, error.data?.get("limit"))
        assertNull(harness.peer.error)
    }

    @Test
    fun theReassemblyLimitComesFromTheEnvironmentOrTheDefault() {
        assertEquals(DEFAULT_MAX_TRANSFER, maxTransferFrom(null))
        assertEquals(1024L, maxTransferFrom("1024"))
        for (bad in listOf("abc", "0", "-1", "1.5", "")) {
            val error = assertFailsWith<IllegalArgumentException> { maxTransferFrom(bad) }
            assertEquals("RUTIS_MAX_TRANSFER \"$bad\": expected a positive number of bytes", error.message)
        }
    }

    /**
     * A real Node process on the other end of a Unix socket, running the Node
     * runtime's own Session: the argument and the result cross whole and each
     * call runs once (#1363).
     */
    @Test
    fun aRealNodeRuntimeCarriesValuesOver16MiBWhole() {
        val root = repoRoot()
        val fixture = root?.resolve("bun/rutis-bun/test/fixtures/node-chunked-session.mjs")
        val node = onPath("node")
        assumeTrue(fixture != null && Files.isRegularFile(fixture), "the Node runtime fixture is in this checkout")
        assumeTrue(node != null, "node is on PATH")
        val fixturePath = fixture!!
        val nodePath = node!!

        val dir = Files.createTempDirectory(Path.of("/tmp"), "rutis-chunked-")
        val socket = dir.resolve("s")
        val server = ServerSocketChannel.open(StandardProtocolFamily.UNIX)
        server.bind(UnixDomainSocketAddress.of(socket))
        var process: Process? = null
        var peer: Peer? = null
        try {
            val output = ByteArrayOutputStream()
            process = ProcessBuilder(nodePath.toString(), fixturePath.toString(), "unix:$socket")
                .directory(root!!.toFile())
                .redirectErrorStream(true)
                .start()
            val processOutput = process
            Thread { runCatching { processOutput.inputStream.copyTo(output) } }.apply { isDaemon = true }.start()
            val accepted = CompletableFuture.supplyAsync { server.accept() }.get(10, TimeUnit.SECONDS)
            peer = Peer(LocalChannels.of(accepted), Options(
                dispatch = { _, _, _ -> Undefined },
                endpoint = Endpoint("main", "node"),
            ))
            peer.start()
            peer.readyFuture.get(10, TimeUnit.SECONDS)
            assertEquals("node", peer.greeting()?.endpoint)
            assertTrue(peer.supports("chunked"), "the Node runtime declares chunked")

            val big = "x".repeat(17 * 1024 * 1024)
            val first = peer.call("conformance", "tally", listOf(big), true) as JsonObject
            assertEquals(1, (first["calls"] as JsonPrimitive).int)
            assertEquals(big.length, (first["size"] as JsonPrimitive).int)
            val second = peer.call("conformance", "tally", listOf(big), true) as JsonObject
            assertEquals(2, (second["calls"] as JsonPrimitive).int)
            assertEquals(big.length, (second["size"] as JsonPrimitive).int)
            val back = peer.call("conformance", "big", listOf(big.length.toLong()), true) as JsonPrimitive
            assertEquals(big.length, back.content.length)
            assertEquals(JsonPrimitive("after"), peer.call("conformance", "echo", listOf(JsonPrimitive("after")), true))
            assertNull(peer.error)
            peer.shutdown("done")
            assertTrue(process.waitFor(10, TimeUnit.SECONDS), "the Node runtime did not stop")
            assertEquals(0, process.exitValue(), "the Node runtime said: ${output.toString(Charsets.UTF_8)}")
        } finally {
            runCatching { peer?.close(null) }
            runCatching { server.close() }
            if (process != null && process.isAlive) process.destroyForcibly()
            dir.toFile().deleteRecursively()
        }
    }

    private fun encode(value: JsonElement): ByteArray = json.encodeToString(JsonElement.serializer(), value).toByteArray(Charsets.UTF_8)

    private fun decode(bytes: ByteArray): JsonObject = json.parseToJsonElement(String(bytes, Charsets.UTF_8)).jsonObject

    private fun repoRoot(): Path? {
        var at: Path? = Path.of(System.getProperty("user.dir")).toAbsolutePath()
        while (at != null) {
            if (Files.isRegularFile(at.resolve("bun/rutis-bun/test/fixtures/node-chunked-session.mjs"))) return at
            at = at.parent
        }
        return null
    }

    private fun onPath(name: String): Path? =
        System.getenv("PATH")?.split(File.pathSeparator)?.asSequence()
            ?.map { Path.of(it, name) }
            ?.firstOrNull { Files.isExecutable(it) }

    private companion object {
        val json = Json { encodeDefaults = true }
    }
}
