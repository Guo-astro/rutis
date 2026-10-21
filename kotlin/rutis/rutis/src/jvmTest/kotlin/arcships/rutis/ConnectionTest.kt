package arcships.rutis

import arcships.rutis.internal.peer.Endpoint
import arcships.rutis.internal.remote.CloseCode
import arcships.rutis.internal.remote.Connection
import arcships.rutis.internal.remote.Ended
import arcships.rutis.internal.remote.FrameConnection
import arcships.rutis.internal.remote.Heartbeat
import arcships.rutis.internal.remote.ListenAddress
import arcships.rutis.internal.remote.Listening
import arcships.rutis.internal.remote.Retry
import arcships.rutis.internal.remote.Tls
import arcships.rutis.internal.remote.WebSocketDialer
import arcships.rutis.internal.remote.WebSocketServer
import arcships.rutis.internal.remote.echoedCloseCode
import java.io.BufferedInputStream
import java.io.ByteArrayOutputStream
import java.io.DataInputStream
import java.io.IOException
import java.io.InputStream
import java.net.InetAddress
import java.net.InetSocketAddress
import java.net.ServerSocket
import java.net.Socket
import java.net.SocketTimeoutException
import java.security.MessageDigest
import java.security.cert.CertificateException
import java.security.cert.X509Certificate
import java.util.Base64
import java.util.concurrent.CompletableFuture
import java.util.concurrent.ExecutionException
import java.util.concurrent.TimeUnit
import javax.net.ssl.SSLContext
import javax.net.ssl.SSLEngine
import javax.net.ssl.X509ExtendedTrustManager
import kotlin.test.AfterTest
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertTrue

private const val PROTOCOL = "rutis.test"
private const val TOKEN = "secret"

/** A raw WebSocket client of a listener: it reads and writes frames itself, or stops reading. */
private class RawClient(address: String, receiveBuffer: Int? = null) : AutoCloseable {
    val socket = Socket()
    private val input: DataInputStream

    init {
        val port = address.substringAfterLast(':').substringBefore('/').toInt()
        if (receiveBuffer != null) socket.receiveBufferSize = receiveBuffer
        socket.connect(InetSocketAddress(InetAddress.getLoopbackAddress(), port))
        socket.soTimeout = 10_000
        val key = Base64.getEncoder().encodeToString(ByteArray(16) { it.toByte() })
        socket.getOutputStream().write(
            ("GET /rutis HTTP/1.1\r\nHost: localhost\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n" +
                "Sec-WebSocket-Key: $key\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Protocol: $PROTOCOL\r\n" +
                "Authorization: Bearer $TOKEN\r\n\r\n").toByteArray(Charsets.ISO_8859_1),
        )
        input = DataInputStream(BufferedInputStream(socket.getInputStream()))
        val head = readHead(input)
        assertTrue(head.startsWith("HTTP/1.1 101"), head)
    }

    fun send(opcode: Int, payload: ByteArray) {
        val out = ByteArrayOutputStream()
        out.write(0x80 or opcode)
        out.write(0x80 or payload.size)
        val key = byteArrayOf(1, 2, 3, 4)
        out.write(key)
        out.write(ByteArray(payload.size) { (payload[it].toInt() xor key[it % 4].toInt()).toByte() })
        socket.getOutputStream().write(out.toByteArray())
    }

    /** The next frame: its opcode and payload. */
    fun next(): Pair<Int, ByteArray> {
        val first = input.readUnsignedByte()
        var length = (input.readUnsignedByte() and 0x7F).toLong()
        if (length == 126L) length = input.readUnsignedShort().toLong() else if (length == 127L) length = input.readLong()
        val payload = ByteArray(length.toInt())
        input.readFully(payload)
        return (first and 0x0F) to payload
    }

    /** The code of the close frame that comes next, the pings before it skipped. */
    fun closed(): Int {
        while (true) {
            val (opcode, payload) = next()
            if (opcode == 0x8) return ((payload[0].toInt() and 0xFF) shl 8) or (payload[1].toInt() and 0xFF)
        }
    }

    override fun close() {
        FrameConnection.quietly { socket.close() }
    }
}

private fun readHead(input: InputStream): String {
    val head = ByteArrayOutputStream()
    while (!head.toString("ISO-8859-1").endsWith("\r\n\r\n")) {
        val b = input.read()
        if (b < 0) throw IOException("closed in the head: $head")
        head.write(b)
    }
    return head.toString("ISO-8859-1")
}

/** Whether `socket` is closed by the far end within `millis`. */
private fun closedWithin(socket: Socket, millis: Long): Boolean {
    socket.soTimeout = millis.toInt()
    return try {
        while (socket.getInputStream().read() >= 0) {
        }
        true
    } catch (_: SocketTimeoutException) {
        false
    } catch (_: IOException) {
        true
    }
}

/**
 * The WebSocket connections themselves (listen and dial), on loopback:
 * what a far end that stops reading, stays silent, or drips its request
 * does to them.
 */
class ConnectionTest {
    private val closing = ArrayList<AutoCloseable>()

    @AfterTest
    fun closeAll() {
        closing.forEach { runCatching { it.close() } }
    }

    private fun server(heartbeat: Heartbeat = Heartbeat(100, 1000), headMillis: Long = 10_000, maxPending: Int = 64): WebSocketServer {
        val server = WebSocketServer(ListenAddress.parse("ws://127.0.0.1:0/rutis"), PROTOCOL, TOKEN, null, heartbeat, headMillis, maxPending)
        closing.add(AutoCloseable { server.close() })
        return server
    }

    private fun client(server: WebSocketServer, receiveBuffer: Int? = null): Pair<RawClient, FrameConnection> {
        val client = RawClient(server.address, receiveBuffer)
        closing.add(client)
        val connection = server.accepted.poll(5, TimeUnit.SECONDS)!!
        closing.add(AutoCloseable { connection.close("the test ended") })
        return client to connection
    }

    /** Sends until a send blocks: the client stopped reading. Returns the blocked send. */
    private fun stuck(connection: Connection): CompletableFuture<Unit> {
        val sending = CompletableFuture<Unit>()
        Thread {
            try {
                while (true) connection.send(ByteArray(256 * 1024) { 'x'.code.toByte() })
            } catch (e: Throwable) {
                sending.completeExceptionally(e)
            }
        }.apply { isDaemon = true }.start()
        // A send taking longer than this is blocked.
        Thread.sleep(500)
        assertTrue(!sending.isDone, "the sends did not block")
        return sending
    }

    @Test
    fun aFarEndThatStopsReadingBlocksNeitherItsReaderNorClosing() {
        val server = server(Heartbeat(100, 3000))
        val (stalled, connection) = client(server, receiveBuffer = 4096)
        val sending = stuck(connection)
        // Its reader takes a ping without waiting for the blocked writer,
        // and goes on reading.
        val received = CompletableFuture.supplyAsync { runCatching { connection.recv() } }
        stalled.send(0x9, "still here".toByteArray())
        stalled.send(0x1, "hello".toByteArray())
        assertEquals("hello", received.get(2, TimeUnit.SECONDS).getOrThrow()!!.toString(Charsets.UTF_8))
        val started = System.nanoTime()
        connection.closeWith(CloseCode.GOING_AWAY, "stopping")
        assertTrue((System.nanoTime() - started) / 1_000_000 < 100, "closeWith blocked")
        // The socket closes within the close grace, failing the blocked send.
        val failure = assertFailsWith<ExecutionException> { sending.get(FrameConnection.CLOSE_GRACE_MILLIS + 2000, TimeUnit.MILLISECONDS) }
        assertTrue(failure.cause is IOException, "${failure.cause}")
    }

    @Test
    fun theHeartbeatOfOneConnectionGoesOnWhileAnotherIsBlocked() {
        val server = server(Heartbeat(100, 3000))
        val (_, connection) = client(server, receiveBuffer = 4096)
        stuck(connection)
        val (other, _) = client(server)
        val started = System.nanoTime()
        assertEquals(0x9, other.next().first)
        assertTrue((System.nanoTime() - started) / 1_000_000 < 1000, "the other connection was not pinged")
    }

    @Test
    fun aWriteBlockedForTheTimeoutClosesTheSocket() {
        val server = server(Heartbeat(100, 1000))
        val (stalled, connection) = client(server, receiveBuffer = 4096)
        // The client keeps answering, so only the write deadline can end it.
        val talking = Thread {
            try {
                while (true) {
                    stalled.send(0xA, ByteArray(0))
                    Thread.sleep(100)
                }
            } catch (_: Exception) {
            }
        }.apply {
            isDaemon = true
            start()
        }
        val sending = stuck(connection)
        val failure = assertFailsWith<ExecutionException> { sending.get(5, TimeUnit.SECONDS) }
        assertTrue(failure.cause is IOException, "${failure.cause}")
        talking.interrupt()
    }

    @Test
    fun aCloseWithoutStatusIsAnsweredWithNormal() {
        val server = server()
        val (client, connection) = client(server)
        val received = CompletableFuture.supplyAsync { connection.recv() }
        client.send(0x8, ByteArray(0))
        assertEquals(CloseCode.NORMAL, client.closed())
        assertEquals(null, received.get(5, TimeUnit.SECONDS))
        assertEquals(CloseCode.NO_STATUS, connection.farClose)
    }

    @Test
    fun closeCodesThatCannotBeSentAreEchoedAsNormal() {
        for (code in listOf(1000, 1001, 1002, 1003, 1007, 1009, 1011, 3000, 4002, 4003, 4999)) assertEquals(code, echoedCloseCode(code))
        for (code in listOf(0, 999, 1004, 1005, 1006, 1015, 2000, 2999, 5000, 65535)) assertEquals(CloseCode.NORMAL, echoedCloseCode(code))
    }

    @Test
    fun aRequestHeadHasADeadlineInAll() {
        val server = server(headMillis = 500)
        val socket = Socket(InetAddress.getLoopbackAddress(), server.address.substringAfterLast(':').substringBefore('/').toInt())
        closing.add(socket)
        // One byte every 100 ms: each read is quick, the head never ends.
        val dripping = Thread {
            try {
                for (b in "GET /rutis HTTP/1.1\r\nX-Slow: ".toByteArray() + ByteArray(1000) { 'a'.code.toByte() }) {
                    socket.getOutputStream().write(b.toInt())
                    Thread.sleep(100)
                }
            } catch (_: Exception) {
            }
        }.apply {
            isDaemon = true
            start()
        }
        assertTrue(closedWithin(socket, 3000), "a dripping head was not cut off")
        dripping.interrupt()
    }

    @Test
    fun upgradesUnderWayAreCapped() {
        val server = server(maxPending = 2)
        val port = server.address.substringAfterLast(':').substringBefore('/').toInt()
        val idle = (1..2).map { Socket(InetAddress.getLoopbackAddress(), port).also(closing::add) }
        Thread.sleep(200)
        val extra = Socket(InetAddress.getLoopbackAddress(), port).also(closing::add)
        assertTrue(closedWithin(extra, 2000), "a connection over the cap was kept")
        assertTrue(!closedWithin(idle[0], 200), "a pending upgrade was dropped")
        // Once one goes, another may come.
        idle.forEach { it.close() }
        Thread.sleep(200)
        val client = RawClient(server.address)
        closing.add(client)
    }

    @Test
    fun aClosedListenerClosesWhatItAcceptedAndNobodyTook() {
        val server = server()
        val client = RawClient(server.address)
        closing.add(client)
        val deadline = System.nanoTime() + 5_000_000_000
        while (server.accepted.isEmpty() && System.nanoTime() < deadline) Thread.sleep(10)
        server.close()
        assertEquals(CloseCode.GOING_AWAY, client.closed())
    }

    @Test
    fun aStoppedListeningLeavesNoThreadBehind() {
        fun forwarding() = Thread.getAllStackTraces().keys.count { it.name == "rutis-accepted" && it.isAlive }
        val before = forwarding()
        repeat(3) {
            val listening = Listening(server(), Endpoint("app", null, listOf("runtime")), emptyList())
            val ended = CompletableFuture.supplyAsync { listening.run() }
            Thread.sleep(100)
            listening.stop()
            assertTrue(ended.get(5, TimeUnit.SECONDS) is Ended.Stopped)
        }
        val deadline = System.nanoTime() + 2_000_000_000
        while (forwarding() > before && System.nanoTime() < deadline) Thread.sleep(10)
        assertEquals(before, forwarding())
    }

    /** An access point that greets a dialer's upgrade, then sends pings at `pingMillis` (until `pinging` is false), and never answers one. */
    private class AccessPoint : AutoCloseable {
        val server = ServerSocket(0, 50, InetAddress.getLoopbackAddress())
        val address = "ws://127.0.0.1:${server.localPort}/rutis"

        @Volatile
        var pinging = true
        val closes = CompletableFuture<Int>()
        lateinit var socket: Socket

        fun serve(pingMillis: Long, closeAtOnce: Boolean = false) {
            Thread {
                try {
                    socket = server.accept()
                    val input = DataInputStream(BufferedInputStream(socket.getInputStream()))
                    val head = readHead(input)
                    val key = head.lines().first { it.startsWith("Sec-WebSocket-Key:", ignoreCase = true) }.substringAfter(':').trim()
                    val accept = Base64.getEncoder().encodeToString(
                        MessageDigest.getInstance("SHA-1").digest((key + "258EAFA5-E914-47DA-95CA-C5AB0DC85B11").toByteArray()),
                    )
                    val out = socket.getOutputStream()
                    out.write(
                        ("HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n" +
                            "Sec-WebSocket-Accept: $accept\r\nSec-WebSocket-Protocol: $PROTOCOL\r\n\r\n").toByteArray(),
                    )
                    out.flush()
                    if (closeAtOnce) out.write(byteArrayOf(0x88.toByte(), 0))
                    Thread {
                        try {
                            while (!closeAtOnce) {
                                if (pinging) synchronized(out) { out.write(byteArrayOf(0x89.toByte(), 0)) }
                                Thread.sleep(pingMillis)
                            }
                        } catch (_: Exception) {
                        }
                    }.apply { isDaemon = true }.start()
                    // The dialer's frames: their close code, the rest ignored.
                    while (true) {
                        val first = input.readUnsignedByte()
                        var length = (input.readUnsignedByte() and 0x7F).toLong()
                        if (length == 126L) length = input.readUnsignedShort().toLong() else if (length == 127L) length = input.readLong()
                        val mask = ByteArray(4).also(input::readFully)
                        val payload = ByteArray(length.toInt()).also(input::readFully)
                        for (i in payload.indices) payload[i] = (payload[i].toInt() xor mask[i % 4].toInt()).toByte()
                        if (first and 0x0F == 0x8) {
                            closes.complete(((payload[0].toInt() and 0xFF) shl 8) or (payload[1].toInt() and 0xFF))
                            return@Thread
                        }
                    }
                } catch (e: Exception) {
                    closes.completeExceptionally(e)
                }
            }.apply { isDaemon = true }.start()
        }

        override fun close() {
            server.close()
            if (::socket.isInitialized) socket.close()
        }
    }

    private fun dialer(at: AccessPoint, heartbeat: Heartbeat): WebSocketDialer {
        val dialer = WebSocketDialer(at.address, PROTOCOL, TOKEN, null, Retry(handshakeMillis = 5000), heartbeat)
        closing.add(AutoCloseable { dialer.close() })
        return dialer
    }

    @Test
    fun aDialedConnectionLastsWhilePingsComeAndEndsAtTheTimeout() {
        val at = AccessPoint().also(closing::add)
        at.serve(pingMillis = 200)
        // The access point never answers this side's pings: only its own count.
        val connection = dialer(at, Heartbeat(100, 1000)).dial()
        val received = CompletableFuture.supplyAsync { runCatching { connection.recv() } }
        Thread.sleep(1500)
        assertTrue(!received.isDone, "the connection ended while the access point pinged: ${received.getNow(null)}")
        at.pinging = false
        val started = System.nanoTime()
        val failure = received.get(5, TimeUnit.SECONDS).exceptionOrNull()
        assertTrue(failure is IOException && failure.message!!.contains("heartbeat timeout"), "$failure")
        assertTrue((System.nanoTime() - started) / 1_000_000 >= 800, "ended before the timeout")
    }

    @Test
    fun aDialedConnectionAnswersACloseWithoutStatusWithNormal() {
        val at = AccessPoint().also(closing::add)
        at.serve(pingMillis = 200, closeAtOnce = true)
        val connection = dialer(at, Heartbeat(100, 1000)).dial()
        runCatching { connection.recv() }
        assertEquals(CloseCode.NORMAL, at.closes.get(5, TimeUnit.SECONDS))
        assertEquals(CloseCode.NO_STATUS, connection.farClose)
    }

    /** A system trust as Android's under a `<domain-config>`: only hostname-aware checks pass. */
    private class DomainConfigTrust : X509ExtendedTrustManager() {
        override fun checkServerTrusted(chain: Array<out X509Certificate>, authType: String) =
            throw CertificateException("Domain specific configurations require that hostname aware checkServerTrusted is used")

        override fun checkServerTrusted(chain: Array<out X509Certificate>, authType: String, socket: Socket?) {}

        override fun checkServerTrusted(chain: Array<out X509Certificate>, authType: String, engine: SSLEngine?) {}

        override fun checkClientTrusted(chain: Array<out X509Certificate>, authType: String) = throw CertificateException("no")

        override fun checkClientTrusted(chain: Array<out X509Certificate>, authType: String, socket: Socket?) = throw CertificateException("no")

        override fun checkClientTrusted(chain: Array<out X509Certificate>, authType: String, engine: SSLEngine?) = throw CertificateException("no")

        override fun getAcceptedIssuers(): Array<X509Certificate> = emptyArray()
    }

    @Test
    fun extraCasKeepTheSystemTrustHostnameAware() {
        val ca = Tls.certificates(javaClass.getResource("/tls/ec.crt")!!.readText())
        assertTrue(Tls.trust(ca) is X509ExtendedTrustManager)
        val other = Tls.certificates(javaClass.getResource("/tls/rsa.crt")!!.readText())
        val own = Tls.trust(ca) as X509ExtendedTrustManager
        val either = Tls.Either(DomainConfigTrust(), own)
        // The system trusts it, asked with the socket: own CAs are not needed.
        either.checkServerTrusted(other.toTypedArray(), "RSA", Socket())
        either.checkServerTrusted(other.toTypedArray(), "RSA", SSLContext.getDefault().createSSLEngine())
        // Without one, the system refuses, and own CAs trust only their own.
        assertFailsWith<CertificateException> { either.checkServerTrusted(other.toTypedArray(), "RSA") }
    }
}
