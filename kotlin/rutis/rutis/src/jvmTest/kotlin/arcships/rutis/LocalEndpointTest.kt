package arcships.rutis

import arcships.rutis.internal.peer.Endpoint
import arcships.rutis.internal.peer.IncompatibleException
import arcships.rutis.internal.peer.LocalChannels
import arcships.rutis.internal.peer.MemoryChannel
import arcships.rutis.internal.peer.Options
import arcships.rutis.internal.peer.Peer
import arcships.rutis.internal.peer.SESSION_CAPABILITIES
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import java.net.StandardProtocolFamily
import java.net.UnixDomainSocketAddress
import java.nio.channels.ServerSocketChannel
import java.nio.file.Files
import java.nio.file.Path
import java.util.concurrent.CompletableFuture
import java.util.concurrent.TimeUnit
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertTrue

/** A local channel speaks the endpoint format and greets as its `--id` (#811 R6, S2b). */
class LocalEndpointTest {
    private val plugin = Rutis.define<Rutis.NoConfig>(name = "any") { _, _ -> }

    @Test
    fun aLocalChannelWithAnIdGreetsAsThatEndpoint() {
        // A short path: a socket path has a length limit.
        val dir = Files.createTempDirectory(Path.of("/tmp"), "r6")
        val path = dir.resolve("s")
        val server = ServerSocketChannel.open(StandardProtocolFamily.UNIX)
        try {
            server.bind(UnixDomainSocketAddress.of(path))
            val served = CompletableFuture.supplyAsync {
                serveArgs(listOf("unix:$path", "--id", "kotlinrun", "--peer", "host", dir.toString()), listOf(plugin))
            }
            val accepted = server.accept()
            val host = Peer(
                LocalChannels.of(accepted),
                Options(dispatch = { _, method, _ -> error("not served: $method") }, endpoint = Endpoint("host", "kotlinrun"), capabilities = SESSION_CAPABILITIES),
            )
            host.start()
            host.readyFuture.get(10, TimeUnit.SECONDS)
            assertEquals("kotlinrun", host.greeting()?.endpoint)
            assertTrue(host.supports("runtime"))
            val mounted = host.call("", "mount", listOf(JsonObject(mapOf("runtime" to JsonPrimitive("kotlinrun")))), false)
            val features = ((mounted as JsonElement) as JsonObject)["features"] as JsonArray
            assertTrue(JsonPrimitive("rows.v2") in features, "mount answered $mounted")
            // The host goes: the runtime's session ends and serving returns 0.
            host.close(null)
            accepted.close()
            assertEquals(0, served.get(10, TimeUnit.SECONDS))
        } finally {
            server.close()
            dir.toFile().deleteRecursively()
        }
    }

    @Test
    fun anInvalidIdIsRefusedBeforeTheChannelOpens() {
        val error = assertFailsWith<IllegalArgumentException> { serveArgs(listOf("unix:/nowhere", "--id", "Kotlin Run", "."), listOf(plugin)) }
        assertEquals("invalid endpoint id Kotlin Run: use lower-case letters, digits and -", error.message)
    }

    @Test
    fun aLocalChannelWithoutAnIdIsRefused() {
        val error = assertFailsWith<IllegalArgumentException> { serveArgs(listOf("unix:/nowhere", "."), listOf(plugin)) }
        assertEquals("a local channel needs --id <endpoint>: unix:/nowhere; a host older than rutis 0.19 started this runtime: upgrade the host", error.message)
    }

    /** A far end of protocol 3 (rutis 0.18) is refused at its handshake. */
    @Test
    fun aFarEndOfProtocol3IsRefused() {
        val (a, far) = MemoryChannel.pair()
        val runtime = Peer(a, Options(dispatch = { _, _, _ -> null }, endpoint = Endpoint("kotlin", "main")))
        runtime.start()
        far.recv() // its hello
        far.send("""{"op":"hello","version":3,"endpoint":"main","implementation":{"name":"rutis","version":"0.18.0"},"capabilities":[]}""".toByteArray())
        runtime.closedFuture.get(5, TimeUnit.SECONDS)
        val error = runtime.error
        assertTrue(error is IncompatibleException, "ended with $error")
        assertEquals("incompatible session: the far end speaks protocol 3, this side 4: upgrade the far end (rutis 0.18.0)", error.message)
    }
}
