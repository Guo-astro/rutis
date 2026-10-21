package arcships.rutis.internal.remote

import arcships.rutis.internal.peer.MAX_MESSAGE
import arcships.rutis.internal.peer.validUtf8
import java.io.ByteArrayOutputStream
import java.io.DataInputStream
import java.io.IOException
import java.io.InputStream
import java.io.OutputStream
import java.net.Socket
import java.util.concurrent.CompletableFuture
import java.util.concurrent.ExecutionException
import java.util.concurrent.Executors
import java.util.concurrent.LinkedBlockingDeque
import java.util.concurrent.ScheduledFuture
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean

/**
 * A WebSocket connection carrying text messages (RFC 6455), on a socket
 * already upgraded and accepted by a listener (frames received must be
 * masked, frames sent are not). It pings the far end every
 * [Heartbeat.pingMillis] and closes with 1001 when nothing came for
 * [Heartbeat.timeoutMillis]. Broken frames close it with their code:
 * unmasked 1002, binary 1003, over 16 MiB 1009, not UTF-8 1007.
 *
 * Frames are written by its own writer thread, in order (pings and pongs
 * ahead of the messages waiting): a far end that stops reading blocks only
 * that thread, never the reader nor the shared heartbeat timer. A write
 * blocked for [Heartbeat.timeoutMillis] closes the socket ([transport], the
 * TCP socket under TLS, closes without waiting for any lock), and a close
 * frame not written within [CLOSE_GRACE_MILLIS] does not hold the socket
 * open.
 */
class FrameConnection(
    private val socket: Socket,
    input: InputStream,
    private val output: OutputStream,
    private val heartbeat: Heartbeat,
    /** The TCP socket under [socket] (itself without TLS): closing it never blocks. */
    private val transport: Socket = socket,
) : Connection {
    private val reader = DataInputStream(input)
    private val closed = AtomicBoolean(false)

    @Volatile
    private var lastSeen = System.nanoTime()

    @Volatile
    override var farClose: Int = 0
        private set

    /** A frame for the writer; `written` is completed once it is written (or failed). */
    private class Frame(val opcode: Int, val payload: ByteArray, val written: CompletableFuture<Unit>? = null)

    /** What the writer writes, in order. */
    private val outbox = LinkedBlockingDeque<Frame>()

    /** When the frame being written started (System.nanoTime), 0 between frames. */
    @Volatile
    private var writingSince = 0L

    /** Set once the writer stopped: what is queued after it fails. */
    @Volatile
    private var writerDone = false

    private val pinging: ScheduledFuture<*> = timers.scheduleWithFixedDelay({
        if (closed.get()) return@scheduleWithFixedDelay
        val since = writingSince
        when {
            // The far end stopped reading: the blocked write fails.
            since != 0L && (System.nanoTime() - since) / 1_000_000 > heartbeat.timeoutMillis -> shut()
            (System.nanoTime() - lastSeen) / 1_000_000 > heartbeat.timeoutMillis ->
                closeWith(CloseCode.GOING_AWAY, "the far end stopped answering")
            // A frame being written does what a ping would.
            since == 0L -> outbox.offerFirst(Frame(0x9, ByteArray(0)))
        }
    }, heartbeat.pingMillis, heartbeat.pingMillis, TimeUnit.MILLISECONDS)

    init {
        Thread(::writeAll, "rutis-websocket-write").apply {
            isDaemon = true
            start()
        }
    }

    private fun writeAll() {
        var failure: IOException? = null
        var frame: Frame? = null
        try {
            while (true) {
                frame = outbox.take()
                if (frame === STOP) break
                writingSince = System.nanoTime()
                try {
                    writeFrame(frame.opcode, frame.payload)
                } finally {
                    writingSince = 0L
                }
                frame.written?.complete(Unit)
                if (frame.opcode == 0x8) {
                    // The far end's close frame may follow: it gets a moment.
                    Thread.sleep(200)
                    quietly { socket.close() }
                    break
                }
            }
        } catch (e: IOException) {
            failure = e
            frame?.written?.completeExceptionally(e)
            shut()
        } catch (_: InterruptedException) {
        } finally {
            writerDone = true
            failQueued(failure)
        }
    }

    /** Fails what waits in the outbox: the writer stopped. */
    private fun failQueued(failure: IOException?) {
        while (true) {
            val frame = outbox.poll() ?: return
            frame.written?.completeExceptionally(failure ?: IOException("connection closed"))
        }
    }

    private fun writeFrame(opcode: Int, payload: ByteArray) {
        val header = ByteArrayOutputStream(14)
        header.write(0x80 or opcode)
        val n = payload.size
        when {
            n < 126 -> header.write(n)
            n <= 0xFFFF -> {
                header.write(126)
                header.write(n shr 8)
                header.write(n and 0xFF)
            }
            else -> {
                header.write(127)
                for (shift in 56 downTo 0 step 8) header.write(((n.toLong() shr shift) and 0xFF).toInt())
            }
        }
        output.write(header.toByteArray())
        output.write(payload)
        output.flush()
    }

    /** Queues `frame` for the writer: last, or ahead of the frames waiting. */
    private fun queue(frame: Frame, first: Boolean = false) {
        if (first) outbox.offerFirst(frame) else outbox.offerLast(frame)
        // The writer stopped before taking it: nobody will.
        if (writerDone) failQueued(null)
    }

    override fun send(message: ByteArray) {
        if (message.size > MAX_MESSAGE) {
            val reason = "message of ${message.size} bytes exceeds the limit of $MAX_MESSAGE"
            closeWith(CloseCode.TOO_BIG, reason)
            throw IOException(reason)
        }
        if (closed.get()) throw IOException("connection closed")
        val frame = Frame(0x1, message, CompletableFuture())
        queue(frame)
        // Waits until it is written (a far end that stops reading pushes
        // back), at most until the write deadline closes the socket.
        var interrupted = false
        try {
            while (true) {
                try {
                    frame.written!!.get()
                    return
                } catch (_: InterruptedException) {
                    interrupted = true
                } catch (e: ExecutionException) {
                    val cause = e.cause
                    throw cause as? IOException ?: IOException(cause?.message ?: "connection closed", cause)
                }
            }
        } finally {
            if (interrupted) Thread.currentThread().interrupt()
        }
    }

    /** Fails reading: closes with `code` and says why. */
    private fun broken(code: Int, reason: String, error: String): Nothing {
        closeWith(code, reason)
        throw IOException(error)
    }

    override fun recv(): ByteArray? {
        val message = ByteArrayOutputStream()
        while (true) {
            val first: Int
            try {
                first = reader.read()
                if (first < 0) {
                    shut()
                    return null
                }
            } catch (e: IOException) {
                shut()
                if (closed.get() && farClose != 0) return null
                throw e
            }
            val second = reader.readUnsignedByte()
            val fin = first and 0x80 != 0
            val opcode = first and 0x0F
            val masked = second and 0x80 != 0
            var length = (second and 0x7F).toLong()
            if (length == 126L) length = reader.readUnsignedShort().toLong()
            else if (length == 127L) length = reader.readLong()
            if (!masked) {
                broken(CloseCode.PROTOCOL_ERROR, "client frames must be masked", "an unmasked client frame")
            }
            if (length < 0 || length > MAX_MESSAGE) {
                broken(CloseCode.TOO_BIG, "message too big", "a message exceeds the limit of $MAX_MESSAGE bytes")
            }
            val key = ByteArray(4)
            if (masked) reader.readFully(key)
            val payload = ByteArray(length.toInt())
            reader.readFully(payload)
            if (masked) for (i in payload.indices) payload[i] = (payload[i].toInt() xor key[i % 4].toInt()).toByte()
            lastSeen = System.nanoTime()
            when (opcode) {
                0x8 -> {
                    val code = if (payload.size >= 2) ((payload[0].toInt() and 0xFF) shl 8) or (payload[1].toInt() and 0xFF) else CloseCode.NO_STATUS
                    val said = if (payload.size > 2) String(payload, 2, payload.size - 2, Charsets.UTF_8) else ""
                    farClose = code
                    closeWith(echoedCloseCode(code), "")
                    return when (code) {
                        CloseCode.NORMAL, CloseCode.GOING_AWAY, CloseCode.NO_STATUS -> null
                        CloseCode.REPLACED -> throw IOException("replaced by a new connection")
                        CloseCode.REJECTED -> throw IOException("rejected: $said")
                        else -> throw IOException("closed ($code): $said")
                    }
                }
                0x9 -> if (!closed.get()) queue(Frame(0xA, payload), first = true)
                0xA -> {}
                0x2 -> broken(CloseCode.UNSUPPORTED, "binary messages are reserved for a binary encoding", "received a binary message")
                0x0, 0x1 -> {
                    message.write(payload)
                    if (message.size() > MAX_MESSAGE) {
                        broken(CloseCode.TOO_BIG, "message too big", "a message exceeds the limit of $MAX_MESSAGE bytes")
                    }
                    if (fin) {
                        val bytes = message.toByteArray()
                        if (!validUtf8(bytes)) broken(CloseCode.INVALID_DATA, "invalid UTF-8", "a message is not UTF-8")
                        return bytes
                    }
                }
                else -> broken(CloseCode.PROTOCOL_ERROR, "unknown opcode", "protocol error")
            }
        }
    }

    /**
     * Sends a close frame with `code`, then closes the socket shortly after;
     * at most [CLOSE_GRACE_MILLIS] later, whether the frame went out or not.
     * Never blocks.
     */
    fun closeWith(code: Int, reason: String) {
        if (!closed.compareAndSet(false, true)) return
        pinging.cancel(false)
        val said = closeReason(reason).toByteArray(Charsets.UTF_8)
        val payload = ByteArray(2 + said.size)
        payload[0] = (code shr 8).toByte()
        payload[1] = code.toByte()
        said.copyInto(payload, 2)
        queue(Frame(0x8, payload))
        timers.schedule({ quietly { transport.close() } }, CLOSE_GRACE_MILLIS, TimeUnit.MILLISECONDS)
    }

    /** Closes at once, without a close frame: never blocks. */
    private fun shut() {
        if (closed.compareAndSet(false, true)) pinging.cancel(false)
        quietly { transport.close() }
        outbox.offerFirst(STOP)
    }

    override fun close(reason: String) = closeWith(CloseCode.GOING_AWAY, reason)

    companion object {
        /** How long a close frame may take to go out before the socket closes anyway. */
        const val CLOSE_GRACE_MILLIS: Long = 1_000

        private val STOP = Frame(-1, ByteArray(0))

        internal val timers = Executors.newScheduledThreadPool(1) { r ->
            Thread(r, "rutis-websocket").apply { isDaemon = true }
        }

        internal inline fun quietly(block: () -> Unit) {
            try {
                block()
            } catch (_: IOException) {
            }
        }
    }
}
