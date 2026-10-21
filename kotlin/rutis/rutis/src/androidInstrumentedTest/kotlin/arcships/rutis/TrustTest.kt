package arcships.rutis

import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import arcships.rutis.internal.remote.Tls
import org.junit.Test
import org.junit.runner.RunWith
import java.net.InetAddress
import java.util.concurrent.CompletableFuture
import java.util.concurrent.TimeUnit
import javax.net.ssl.SSLContext
import javax.net.ssl.SSLServerSocket
import javax.net.ssl.SSLSocket

/**
 * `trust` adds CAs to the system's trust without losing it under a network
 * security config with a `<domain-config>` (this test application's): a
 * server the system trusts (raw/system_ca, a base-config anchor) is trusted
 * with an unrelated extra CA (raw/other_ca), over a real TLS handshake.
 */
@RunWith(AndroidJUnit4::class)
class TrustTest {
    private fun raw(name: String): String {
        val context = InstrumentationRegistry.getInstrumentation().context
        val id = context.resources.getIdentifier(name, "raw", context.packageName)
        return context.resources.openRawResource(id).use { it.readBytes().toString(Charsets.UTF_8) }
    }

    @Test
    fun extraCasKeepTheSystemTrust() {
        val server = Tls.server(raw("system_ca"), raw("server_key")).serverSocketFactory
            .createServerSocket(0, 1, InetAddress.getByName("127.0.0.1")) as SSLServerSocket
        server.use {
            val served = CompletableFuture<Unit>()
            Thread {
                try {
                    (server.accept() as SSLSocket).use { it.startHandshake() }
                    served.complete(Unit)
                } catch (e: Exception) {
                    served.completeExceptionally(e)
                }
            }.apply { isDaemon = true }.start()
            val client = SSLContext.getInstance("TLS")
            client.init(null, arrayOf(Tls.trust(Tls.certificates(raw("other_ca")))), null)
            (client.socketFactory.createSocket("localhost", server.localPort) as SSLSocket).use { it.startHandshake() }
            served.get(10, TimeUnit.SECONDS)
        }
    }
}
