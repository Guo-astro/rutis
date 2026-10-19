package arcships.rutis.internal.peer

import java.io.IOException
import java.io.InputStream
import java.io.OutputStream
import java.net.InetSocketAddress
import java.net.Socket
import java.net.StandardProtocolFamily
import java.net.UnixDomainSocketAddress
import java.nio.ByteBuffer
import java.nio.channels.SocketChannel

/** The environment variable holding the token a process presents first on a `tcp:` channel. */
const val CHANNEL_TOKEN: String = "RUTIS_CHANNEL_TOKEN"

/**
 * The local channels (runtime contract §1): `unix:<path>` or a bare path, a
 * Unix socket rutis created that this process dials back (JDK 16+); and
 * `tcp:127.0.0.1:<port>`, a loopback port it dials, presenting the token in
 * `RUTIS_CHANNEL_TOKEN` and a newline first. `fd:3` is not taken: the JDK
 * has no public way to wrap an inherited descriptor.
 */
object LocalChannels {
    fun open(spec: String, token: () -> String? = { System.getenv(CHANNEL_TOKEN) }): Channel = when {
        spec.startsWith("fd:") -> throw IOException("fd channels are not supported on the JVM: start the runtime on a socket path or loopback address")
        spec.startsWith("tcp:") -> tcp(spec.removePrefix("tcp:"), token() ?: throw IOException("$CHANNEL_TOKEN is not set for $spec"))
        else -> unix(spec.removePrefix("unix:"))
    }

    private fun tcp(address: String, token: String): Channel {
        val colon = address.lastIndexOf(':')
        if (colon < 0) throw IOException("invalid channel tcp:$address")
        val host = address.substring(0, colon).removePrefix("[").removeSuffix("]")
        val port = address.substring(colon + 1).toIntOrNull() ?: throw IOException("invalid channel tcp:$address")
        val socket = Socket()
        socket.tcpNoDelay = true
        socket.connect(InetSocketAddress(host, port))
        val output = socket.getOutputStream()
        output.write((token + "\n").toByteArray(Charsets.UTF_8))
        output.flush()
        return LineChannel(socket.getInputStream(), output) { socket.close() }
    }

    private fun unix(path: String): Channel {
        val channel = SocketChannel.open(StandardProtocolFamily.UNIX)
        channel.connect(UnixDomainSocketAddress.of(path))
        return of(channel)
    }

    /**
     * A line channel over a connected socket channel, read and written from
     * two threads at once (the streams of `java.nio.channels.Channels` would
     * hold one lock for both).
     */
    fun of(channel: SocketChannel): Channel {
        val input = object : InputStream() {
            override fun read(): Int {
                val one = ByteArray(1)
                return if (read(one, 0, 1) < 0) -1 else one[0].toInt() and 0xff
            }

            override fun read(b: ByteArray, off: Int, len: Int): Int {
                if (len == 0) return 0
                return channel.read(ByteBuffer.wrap(b, off, len))
            }
        }
        val output = object : OutputStream() {
            override fun write(b: Int) = write(byteArrayOf(b.toByte()), 0, 1)

            override fun write(b: ByteArray, off: Int, len: Int) {
                val buffer = ByteBuffer.wrap(b, off, len)
                while (buffer.hasRemaining()) channel.write(buffer)
            }
        }
        return LineChannel(input, output) {
            try {
                channel.shutdownInput()
            } catch (_: IOException) {
            }
            try {
                channel.shutdownOutput()
            } catch (_: IOException) {
            }
            channel.close()
        }
    }
}
