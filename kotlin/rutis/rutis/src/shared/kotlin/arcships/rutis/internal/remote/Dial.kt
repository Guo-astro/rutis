package arcships.rutis.internal.remote

import arcships.rutis.internal.peer.MAX_MESSAGE
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.Response
import okhttp3.WebSocket
import okhttp3.WebSocketListener
import okhttp3.internal.ws.RealWebSocket
import okio.ByteString
import java.io.IOException
import java.net.UnknownServiceException
import java.security.cert.CertPathValidatorException
import java.security.cert.CertificateException
import java.util.concurrent.CompletableFuture
import java.util.concurrent.ExecutionException
import java.util.concurrent.Executors
import java.util.concurrent.LinkedBlockingQueue
import java.util.concurrent.ScheduledFuture
import java.util.concurrent.TimeUnit
import java.util.concurrent.TimeoutException
import javax.net.ssl.SSLContext
import javax.net.ssl.SSLHandshakeException
import javax.net.ssl.SSLPeerUnverifiedException
import javax.net.ssl.X509TrustManager

/** The first line of `text`, printable and bounded. */
internal fun firstLine(text: String): String {
    val line = text.trim().lineSequence().firstOrNull()?.trim() ?: return ""
    return line.take(200).filter { !it.isISOControl() }
}

/** `reason` cut to the 123 bytes a close frame carries, on a character boundary. */
internal fun closeReason(reason: String): String {
    if (reason.toByteArray(Charsets.UTF_8).size <= 123) return reason
    var cut = reason
    while (cut.toByteArray(Charsets.UTF_8).size > 123) cut = cut.dropLast(1)
    return cut
}

/**
 * Dialing an access point over WebSocket, with OkHttp (design K8): one
 * client per dialing runtime. A failure is a [DialException], classified
 * from its type or status, never from its text, as rutis-bridge's dialer
 * does: 401 / 403 and a server certificate this side refuses are an
 * authentication failure; 400, 404, 405, 426, a session protocol the
 * access point did not choose, and a platform refusing cleartext are
 * incompatible; the rest is retryable.
 *
 * The heartbeat is the SDK's own, as in the other runtimes: every
 * [Heartbeat.pingMillis] it looks whether anything came from the access
 * point (a message, a ping or a pong; the access point pings every ping
 * interval) and drops the connection once nothing came for
 * [Heartbeat.timeoutMillis]. OkHttp's ping is off: it fails a connection
 * whose pong is late by one ping interval.
 */
class WebSocketDialer(
    private val spec: String,
    private val protocol: String,
    private val token: String?,
    trust: X509TrustManager?,
    private val retry: Retry,
    private val heartbeat: Heartbeat,
) {
    private val host = dialHost(spec)
    private val client: OkHttpClient = OkHttpClient.Builder()
        .connectTimeout(retry.handshakeMillis, TimeUnit.MILLISECONDS)
        .readTimeout(retry.handshakeMillis, TimeUnit.MILLISECONDS)
        .writeTimeout(30, TimeUnit.SECONDS)
        .retryOnConnectionFailure(false)
        .followRedirects(false)
        .apply {
            if (trust != null) {
                val context = SSLContext.getInstance("TLS")
                context.init(null, arrayOf(trust), null)
                sslSocketFactory(context.socketFactory, trust)
            }
        }
        .build()

    /** Dials once: the connection, upgraded and speaking [protocol]. */
    fun dial(): Connection {
        val request = Request.Builder()
            .url(spec)
            .header("Sec-WebSocket-Protocol", protocol)
            .apply { if (!token.isNullOrEmpty()) header("Authorization", "Bearer $token") }
            .build()
        val connection = OkHttpConnection(host, protocol, heartbeat)
        val socket = client.newWebSocket(request, connection.listener)
        try {
            return connection.opened.get(retry.handshakeMillis * 2, TimeUnit.MILLISECONDS)
        } catch (e: ExecutionException) {
            socket.cancel()
            throw e.cause ?: e
        } catch (_: TimeoutException) {
            socket.cancel()
            throw DialException(Failure.Retryable, "$host: the upgrade did not finish in time")
        }
    }

    /** Lets the client's threads go (an application that stopped). */
    fun close() {
        client.dispatcher.executorService.shutdown()
        client.connectionPool.evictAll()
    }
}

/** One dialed WebSocket connection: OkHttp's callbacks turned into a [Connection]. */
private class OkHttpConnection(
    private val host: String,
    private val protocol: String,
    private val heartbeat: Heartbeat,
) : Connection {
    val opened = CompletableFuture<Connection>()
    private val inbox = LinkedBlockingQueue<Any>()
    private var socket: WebSocket? = null

    @Volatile
    private var ended = false

    @Volatile
    override var farClose: Int = 0
        private set

    /** The end of what is received: null when it ended cleanly. */
    private class End(val error: IOException?)

    private fun end(error: IOException?) {
        if (ended) return
        ended = true
        beating?.cancel(false)
        inbox.put(End(error))
    }

    /** When something last came from the far end (System.nanoTime). */
    @Volatile
    private var lastSeen = System.nanoTime()

    /** The pings and pongs received when the heartbeat last looked. */
    private var frames = 0

    private var beating: ScheduledFuture<*>? = null

    /**
     * The pings and pongs received on `ws` so far; null when this OkHttp
     * does not say (another version than the SDK's): the heartbeat cannot
     * tell a quiet access point from a lost one, and leaves it to TCP.
     */
    private fun controlFrames(ws: WebSocket): Int? = try {
        (ws as? RealWebSocket)?.let { it.receivedPingCount() + it.receivedPongCount() }
    } catch (_: LinkageError) {
        null
    }

    private fun beat(ws: WebSocket) {
        if (ended) return
        val count = controlFrames(ws)
        if (count == null || count != frames) {
            frames = count ?: 0
            lastSeen = System.nanoTime()
        }
        val silent = (System.nanoTime() - lastSeen) / 1_000_000
        if (silent > heartbeat.timeoutMillis) {
            end(IOException("no message from the access point for ${seconds(silent)}: heartbeat timeout"))
            ws.cancel()
            return
        }
        // An unsolicited pong (RFC 6455 §5.5.3): the access point hears this side.
        try {
            (ws as? RealWebSocket)?.pong(ByteString.EMPTY)
        } catch (_: LinkageError) {
        }
    }

    val listener = object : WebSocketListener() {
        override fun onOpen(webSocket: WebSocket, response: Response) {
            socket = webSocket
            val chosen = response.header("Sec-WebSocket-Protocol")
            if (chosen != protocol) {
                webSocket.close(CloseCode.PROTOCOL_ERROR, "no session protocol")
                val said = if (chosen == null) "the access point chose none" else "the access point chose \"$chosen\""
                opened.completeExceptionally(DialException(Failure.Incompatible, "$host: session protocol: $said"))
                return
            }
            lastSeen = System.nanoTime()
            beating = timers.scheduleWithFixedDelay(
                { beat(webSocket) },
                heartbeat.pingMillis,
                heartbeat.pingMillis,
                TimeUnit.MILLISECONDS,
            )
            opened.complete(this@OkHttpConnection)
        }

        override fun onMessage(webSocket: WebSocket, text: String) {
            lastSeen = System.nanoTime()
            val bytes = text.toByteArray(Charsets.UTF_8)
            if (bytes.size > MAX_MESSAGE) {
                webSocket.close(CloseCode.TOO_BIG, "message too big")
                end(IOException("a message exceeds the limit of $MAX_MESSAGE bytes"))
                return
            }
            inbox.put(bytes)
        }

        override fun onMessage(webSocket: WebSocket, bytes: ByteString) {
            webSocket.close(CloseCode.UNSUPPORTED, "binary messages are reserved for a binary encoding")
            end(IOException("received a binary message"))
        }

        override fun onClosing(webSocket: WebSocket, code: Int, reason: String) {
            farClose = code
            webSocket.close(echoedCloseCode(code), null)
            end(
                when (code) {
                    CloseCode.NORMAL, CloseCode.NO_STATUS -> IOException("the access point closed the connection")
                    CloseCode.REPLACED -> IOException("replaced by a new connection")
                    CloseCode.REJECTED -> IOException("rejected: $reason")
                    else -> IOException("closed ($code): $reason")
                },
            )
        }

        override fun onClosed(webSocket: WebSocket, code: Int, reason: String) {
            end(null)
        }

        override fun onFailure(webSocket: WebSocket, t: Throwable, response: Response?) {
            if (!opened.isDone) {
                opened.completeExceptionally(refused(t, response))
                return
            }
            end(t as? IOException ?: IOException(t.message, t))
        }
    }

    /** What a dial that failed means. */
    private fun refused(t: Throwable, response: Response?): DialException {
        if (response != null && response.code != 101) {
            var reason = "$host: upgrade refused with ${response.code}"
            val said = try {
                firstLine(response.peekBody(4096).string())
            } catch (_: Exception) {
                ""
            }
            if (said.isNotEmpty()) reason += ": $said"
            val failure = when (response.code) {
                401, 403 -> Failure.AuthRejected
                400, 404, 405, 426 -> Failure.Incompatible
                else -> Failure.Retryable
            }
            return DialException(failure, reason)
        }
        // OkHttp tries each address of the host (IPv6 and IPv4 at once) and
        // reports the first failure, the others suppressed in it: a
        // certificate refused on one address is the access point's.
        val failures = failures(t)
        failures.firstOrNull { it is SSLPeerUnverifiedException || (it is SSLHandshakeException && untrusted(it)) }?.let {
            return DialException(Failure.AuthRejected, "$host: the access point's certificate is not trusted: ${it.message}")
        }
        // A platform policy (Android's refusing cleartext): retrying cannot fix it.
        failures.firstOrNull { it is UnknownServiceException }?.let {
            return DialException(Failure.Incompatible, "$host: ${it.message}")
        }
        return DialException(Failure.Retryable, "$host: ${t.message ?: t}")
    }

    /** `t`, and what it suppressed, at any depth. */
    private fun failures(t: Throwable): List<Throwable> {
        val all = ArrayList<Throwable>()
        fun add(at: Throwable) {
            if (all.any { it === at }) return
            all.add(at)
            at.suppressed.forEach(::add)
        }
        add(t)
        return all
    }

    /** Whether a certificate refused `t`: one this side does not trust. */
    private fun untrusted(t: Throwable): Boolean {
        var at: Throwable? = t
        while (at != null) {
            if (at is CertificateException || at is CertPathValidatorException) return true
            at = at.cause
        }
        return false
    }

    override fun send(message: ByteArray) {
        val ws = socket ?: throw IOException("not connected")
        // OkHttp closes a connection whose outgoing queue would exceed 16 MiB:
        // wait for room instead.
        while (ws.queueSize() + message.size > MAX_QUEUE) {
            if (ended) throw IOException("connection closed")
            Thread.sleep(1)
        }
        if (!ws.send(String(message, Charsets.UTF_8))) throw IOException("connection closed")
    }

    override fun recv(): ByteArray? {
        val item = inbox.take()
        if (item is End) {
            inbox.put(item)
            item.error?.let { throw it }
            return null
        }
        return item as ByteArray
    }

    override fun close(reason: String) {
        val ws = socket ?: return
        ws.close(CloseCode.GOING_AWAY, closeReason(reason))
        end(null)
        // The far end's close frame is not waited for long.
        timers.schedule({ ws.cancel() }, 1, TimeUnit.SECONDS)
    }

    companion object {
        private const val MAX_QUEUE: Long = 16L * 1024 * 1024
        private val timers = Executors.newSingleThreadScheduledExecutor { r ->
            Thread(r, "rutis-dial-timer").apply { isDaemon = true }
        }
    }
}
