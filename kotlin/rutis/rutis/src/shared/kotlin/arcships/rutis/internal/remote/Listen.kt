package arcships.rutis.internal.remote

import arcships.rutis.Plugin
import arcships.rutis.internal.RuntimeSession
import arcships.rutis.internal.peer.Endpoint
import arcships.rutis.internal.peer.RefusedException
import java.io.BufferedInputStream
import java.io.ByteArrayOutputStream
import java.io.IOException
import java.io.InputStream
import java.net.InetAddress
import java.net.InetSocketAddress
import java.net.ServerSocket
import java.net.Socket
import java.net.URI
import java.security.MessageDigest
import java.util.Base64
import java.util.concurrent.LinkedBlockingQueue
import javax.net.ssl.SSLContext

/**
 * Why a dialer that offered none of this endpoint's session protocol is
 * refused, and which end to upgrade when the versions it offered
 * (`rutis.<n>`) are all newer (this endpoint) or all older (the dialer)
 * than this endpoint's. The wording is rutis-bridge's
 * (`transport::websocket::listen::protocol_refusal`).
 */
fun protocolRefusal(ours: String, offered: List<String>): String {
    fun version(protocol: String): Int? {
        val number = protocol.removePrefix("rutis.")
        if (number == protocol || number.isEmpty() || !number.all { it in '0'..'9' }) return null
        return number.toIntOrNull()
    }
    val reason = "this endpoint speaks $ours"
    val n = version(ours) ?: return reason
    val theirs = offered.mapNotNull { p -> version(p)?.let { p to it } }
    if (theirs.isEmpty()) return reason
    val said = "$reason, the dialer offered ${theirs.joinToString(", ") { it.first }}"
    return when {
        theirs.all { it.second > n } -> "$said: upgrade this endpoint (the Kotlin SDK io.github.arcships:rutis)"
        theirs.all { it.second < n } -> "$said: upgrade the dialer"
        else -> said
    }
}

/** Where a listener binds: its address, path and whether it serves TLS, from a ws:// or wss:// address. */
class ListenAddress(val host: String, val port: Int, val path: String, val secure: Boolean) {
    companion object {
        /** `spec` as a listener takes it: a non-loopback one needs wss://. */
        fun parse(spec: String): ListenAddress {
            val uri = try {
                URI(spec)
            } catch (_: java.net.URISyntaxException) {
                null
            }
            if (uri == null || (uri.scheme != "ws" && uri.scheme != "wss")) throw IllegalArgumentException("$spec is not a ws:// or wss:// address")
            val secure = uri.scheme == "wss"
            val host = uri.host?.removePrefix("[")?.removeSuffix("]")?.takeIf { it.isNotEmpty() } ?: "127.0.0.1"
            if (!secure && host != "127.0.0.1" && host != "::1" && host != "localhost") {
                throw IllegalArgumentException("$spec: only a loopback listener may go without TLS")
            }
            val port = if (uri.port >= 0) uri.port else if (secure) 443 else 80
            return ListenAddress(host, port, uri.rawPath?.takeIf { it.isNotEmpty() } ?: "/", secure)
        }
    }
}

/**
 * A WebSocket listener for a controller (rutis dialing in): it accepts
 * connections presenting `token` (constant-time) and speaking `protocol`,
 * and refuses the others with the status rutis's dialer tells apart: 404
 * for another path, 401 without credentials, 403 for a wrong token, 400
 * without the session protocol (the refusal's words in the body). What it
 * accepts comes out of [accepted].
 */
class WebSocketServer(
    private val at: ListenAddress,
    private val protocol: String,
    private val token: String?,
    tls: SSLContext?,
    private val heartbeat: Heartbeat,
) {
    private val server: ServerSocket = (tls?.serverSocketFactory?.createServerSocket() ?: ServerSocket()).apply {
        reuseAddress = true
        bind(InetSocketAddress(InetAddress.getByName(at.host), at.port))
    }

    /** The address it is bound to, as a dialer would write it. */
    val address: String = run {
        val bound = server.inetAddress.hostAddress ?: at.host
        val shown = if (':' in bound) "[${bound.substringBefore('%')}]" else bound
        "${if (at.secure) "wss" else "ws"}://$shown:${server.localPort}${at.path}"
    }

    /** The upgraded connections, in the order they came. */
    val accepted = LinkedBlockingQueue<FrameConnection>()

    init {
        Thread({
            while (true) {
                val socket = try {
                    server.accept()
                } catch (_: IOException) {
                    return@Thread
                }
                Thread({ upgrade(socket) }, "rutis-upgrade").apply { isDaemon = true }.start()
            }
        }, "rutis-listen").apply { isDaemon = true }.start()
    }

    /** Stops accepting. */
    fun close() {
        try {
            server.close()
        } catch (_: IOException) {
        }
    }

    private class Request(val target: String, val headers: Map<String, List<String>>) {
        fun header(name: String): String? = headers[name.lowercase()]?.firstOrNull()
    }

    /** The request head, up to its blank line (16 KiB at most). */
    private fun readHead(input: InputStream): Request? {
        val head = ByteArrayOutputStream()
        var matched = 0
        val end = byteArrayOf(13, 10, 13, 10)
        while (matched < 4) {
            val b = input.read()
            if (b < 0 || head.size() > 16 * 1024) return null
            head.write(b)
            matched = if (b.toByte() == end[matched]) matched + 1 else if (b == 13) 1 else 0
        }
        val lines = head.toString("ISO-8859-1").split("\r\n")
        val parts = lines[0].split(" ")
        if (parts.size != 3 || parts[0] != "GET") return null
        val headers = HashMap<String, MutableList<String>>()
        for (line in lines.drop(1)) {
            val colon = line.indexOf(':')
            if (colon <= 0) continue
            headers.getOrPut(line.substring(0, colon).trim().lowercase()) { ArrayList() }.add(line.substring(colon + 1).trim())
        }
        return Request(parts[1], headers)
    }

    private fun refuse(socket: Socket, status: Int, text: String, body: String) {
        val content = (body + "\n").toByteArray(Charsets.UTF_8)
        val head = "HTTP/1.1 $status $text\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: ${content.size}\r\nConnection: close\r\n\r\n"
        try {
            socket.getOutputStream().apply {
                write(head.toByteArray(Charsets.ISO_8859_1))
                write(content)
                flush()
            }
        } catch (_: IOException) {
        }
        FrameConnection.quietly { socket.close() }
    }

    private fun upgrade(socket: Socket) {
        try {
            socket.soTimeout = 10_000
            socket.tcpNoDelay = true
            val input = BufferedInputStream(socket.getInputStream())
            val request = readHead(input)
            if (request == null) {
                refuse(socket, 400, "Bad Request", "a WebSocket upgrade is required")
                return
            }
            val path = request.target.substringBefore('?')
            if (path != at.path) return refuse(socket, 404, "Not Found", "no rutis endpoint here")
            val authorization = request.header("Authorization")
            if (authorization == null || !authorization.startsWith("Bearer ")) {
                return refuse(socket, 401, "Unauthorized", "credentials required")
            }
            val presented = authorization.removePrefix("Bearer ").toByteArray(Charsets.UTF_8)
            if (token.isNullOrEmpty() || !MessageDigest.isEqual(presented, token.toByteArray(Charsets.UTF_8))) {
                return refuse(socket, 403, "Forbidden", "not accepted here")
            }
            val offered = request.headers["sec-websocket-protocol"].orEmpty().flatMap { it.split(",") }.map { it.trim() }
            if (protocol !in offered) return refuse(socket, 400, "Bad Request", protocolRefusal(protocol, offered))
            val challenge = request.header("Sec-WebSocket-Key")
            if (!request.header("Upgrade").equals("websocket", ignoreCase = true) || challenge.isNullOrEmpty()) {
                return refuse(socket, 400, "Bad Request", "a WebSocket upgrade is required")
            }
            val digest = MessageDigest.getInstance("SHA-1").digest((challenge + GUID).toByteArray(Charsets.ISO_8859_1))
            val response = "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n" +
                "Sec-WebSocket-Accept: ${Base64.getEncoder().encodeToString(digest)}\r\n" +
                "Sec-WebSocket-Protocol: $protocol\r\n\r\n"
            val output = socket.getOutputStream()
            output.write(response.toByteArray(Charsets.ISO_8859_1))
            output.flush()
            socket.soTimeout = 0
            accepted.put(FrameConnection(socket, input, output, heartbeat))
        } catch (_: IOException) {
            FrameConnection.quietly { socket.close() }
        }
    }

    companion object {
        private const val GUID = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11"
    }
}

/**
 * Serves one controller at a time on [server]: a newer connection takes
 * over (the older closed with 4002), and is greeted only once the old
 * lease is gone. Runs until [stop] (the rows go, then the connection and
 * the listener close) or a controller is incompatible (it refused the
 * host, older than 0.13).
 */
class Listening(
    private val server: WebSocketServer,
    private val endpoint: Endpoint,
    private val plugins: List<Plugin>,
    private val status: (Status) -> Unit = {},
    private val shutdownMillis: Long? = null,
) {
    private sealed class Event {
        class Accepted(val connection: FrameConnection) : Event()

        class Closed(val session: RuntimeSession) : Event()

        object Stop : Event()
    }

    private val events = LinkedBlockingQueue<Event>()

    fun stop() {
        events.put(Event.Stop)
    }

    fun run(): Ended {
        Thread({
            while (true) {
                val connection = try {
                    server.accepted.take()
                } catch (_: InterruptedException) {
                    return@Thread
                }
                events.put(Event.Accepted(connection))
            }
        }, "rutis-accepted").apply { isDaemon = true }.start()
        var current: RuntimeSession? = null
        var connection: FrameConnection? = null
        try {
            while (true) {
                when (val event = events.take()) {
                    is Event.Accepted -> {
                        current?.let {
                            connection?.closeWith(CloseCode.REPLACED, "replaced by a new connection")
                            it.end()
                        }
                        val started = try {
                            RuntimeSession.start(event.connection, endpoint, plugins, shutdownMillis)
                        } catch (e: Exception) {
                            event.connection.closeWith(CloseCode.GOING_AWAY, e.message ?: "the session did not start")
                            null
                        }
                        current = started
                        connection = if (started != null) event.connection else null
                        started?.peer?.readyFuture?.thenRun {
                            if (started.peer.error == null) status(Status.Connected(server.address))
                        }
                        started?.peer?.closedFuture?.thenRun { events.put(Event.Closed(started)) }
                    }
                    is Event.Closed -> {
                        if (event.session !== current) continue
                        event.session.end()
                        (event.session.peer.error as? RefusedException)?.let { return Ended.Refused(it.reason) }
                        current = null
                        connection = null
                        status(Status.Connecting(null))
                    }
                    Event.Stop -> return Ended.Stopped(current?.stop("the runtime stopped") ?: true)
                }
            }
        } finally {
            server.close()
        }
    }
}
