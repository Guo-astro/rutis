package arcships.rutis.internal.peer

import java.io.BufferedInputStream
import java.io.ByteArrayOutputStream
import java.io.IOException
import java.io.InputStream
import java.io.OutputStream
import java.nio.ByteBuffer
import java.nio.charset.CharacterCodingException
import java.nio.charset.CodingErrorAction
import java.util.concurrent.LinkedBlockingQueue
import java.util.concurrent.atomic.AtomicBoolean

/** The largest message a channel carries, as on the WebSocket channel. */
const val MAX_MESSAGE: Int = 16 * 1024 * 1024

/**
 * Ordered, reliable messages with their boundaries kept. [recv] returns
 * null once the far end finished; [send] and [recv] are each called from one
 * thread at a time (the session's writer and reader).
 */
interface Channel {
    fun send(message: ByteArray)
    fun recv(): ByteArray?
    fun close(reason: String)
}

/** Whether `bytes` are valid UTF-8. */
internal fun validUtf8(bytes: ByteArray): Boolean = try {
    Charsets.UTF_8.newDecoder()
        .onMalformedInput(CodingErrorAction.REPORT)
        .onUnmappableCharacter(CodingErrorAction.REPORT)
        .decode(ByteBuffer.wrap(bytes))
    true
} catch (_: CharacterCodingException) {
    false
}

/** A stream channel, one message per line (`\n`). */
class LineChannel(
    input: InputStream,
    private val output: OutputStream,
    private val closer: () -> Unit,
) : Channel {
    private val reader = BufferedInputStream(input, 64 * 1024)
    private val closed = AtomicBoolean(false)

    override fun send(message: ByteArray) {
        if (message.contains('\n'.code.toByte())) throw IOException("a message contains a raw newline")
        if (message.size > MAX_MESSAGE) throw IOException("message of ${message.size} bytes exceeds the limit of $MAX_MESSAGE")
        val framed = message.copyOf(message.size + 1)
        framed[message.size] = '\n'.code.toByte()
        output.write(framed)
        output.flush()
    }

    override fun recv(): ByteArray? {
        val line = ByteArrayOutputStream()
        while (true) {
            val b = try {
                reader.read()
            } catch (e: IOException) {
                if (closed.get()) return null
                throw e
            }
            if (b < 0) {
                if (line.size() > 0) throw IOException("stream ended inside a message")
                return null
            }
            if (b == '\n'.code) {
                val bytes = line.toByteArray()
                if (!validUtf8(bytes)) throw IOException("a message is not UTF-8")
                return bytes
            }
            line.write(b)
            if (line.size() > MAX_MESSAGE) throw IOException("a message exceeds the limit of $MAX_MESSAGE bytes")
        }
    }

    override fun close(reason: String) {
        if (closed.compareAndSet(false, true)) {
            try {
                closer()
            } catch (_: IOException) {
            }
        }
    }
}

/** Two channels, each delivering what the other sends (tests). */
object MemoryChannel {
    private val END = ByteArray(0)

    private class End(private val inbox: LinkedBlockingQueue<ByteArray>, private val outbox: LinkedBlockingQueue<ByteArray>) : Channel {
        private val closed = AtomicBoolean(false)

        override fun send(message: ByteArray) {
            if (closed.get()) throw IOException("channel closed")
            if (message.size > MAX_MESSAGE) throw IOException("message of ${message.size} bytes exceeds the limit of $MAX_MESSAGE")
            outbox.offer(message.copyOf())
        }

        override fun recv(): ByteArray? {
            val message = inbox.take()
            if (message === END) {
                inbox.offer(END)
                return null
            }
            return message
        }

        override fun close(reason: String) {
            if (closed.compareAndSet(false, true)) {
                outbox.offer(END)
                inbox.offer(END)
            }
        }
    }

    fun pair(): Pair<Channel, Channel> {
        val a = LinkedBlockingQueue<ByteArray>()
        val b = LinkedBlockingQueue<ByteArray>()
        return End(a, b) to End(b, a)
    }
}
