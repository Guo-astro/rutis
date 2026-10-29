// guarantee: bridge/dial-out-rules
package arcships.rutis

import arcships.rutis.internal.peer.EndpointMismatchException
import arcships.rutis.internal.peer.IncompatibleException
import arcships.rutis.internal.peer.RefusedException
import arcships.rutis.internal.remote.Backoff
import arcships.rutis.internal.remote.CloseCode
import arcships.rutis.internal.remote.Failure
import arcships.rutis.internal.remote.Heartbeat
import arcships.rutis.internal.remote.ListenAddress
import arcships.rutis.internal.remote.Retry
import arcships.rutis.internal.remote.Tls
import arcships.rutis.internal.remote.dialHost
import arcships.rutis.internal.remote.protocolRefusal
import arcships.rutis.internal.remote.seconds
import arcships.rutis.internal.remote.sessionFailure
import java.io.IOException
import java.security.Signature
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertTrue

/**
 * The remote channels' rules (design-remote-runtime-dial §六) that need no
 * connection; what needs one is checked from outside, on the conformance
 * jar (crates/rutis-host/tests/dial_conformance.rs, kotlin_remote.rs).
 */
class RemoteTest {
    @Test
    fun backoffDoublesUpToItsMaximumAndResets() {
        val backoff = Backoff(Retry(), random = { 0.5 })
        assertEquals(listOf(500L, 1000, 2000, 4000, 8000, 16000, 30000, 30000), List(8) { backoff.waitMillis() })
        backoff.reset()
        assertEquals(500L, backoff.waitMillis())
        // ± 20 % jitter at the extremes.
        assertEquals(400L, Backoff(Retry(), random = { 0.0 }).waitMillis())
        assertEquals(600L, Backoff(Retry(), random = { 1.0 }).waitMillis())
    }

    @Test
    fun testsShortenTheWaits() {
        val retry = Retry.from("100,400,60000,300")
        assertEquals(listOf(100L, 400, 60000, 300), listOf(retry.initialMillis, retry.maxMillis, retry.stableMillis, retry.rejectedMillis))
        for (wrong in listOf(null, "", "1,2,3", "1,2,3,x", "1,2,3,-4")) {
            assertEquals(500L, Retry.from(wrong).initialMillis, "$wrong")
        }
        assertEquals(100L, Heartbeat.from("100,1000").pingMillis)
        assertEquals(10_000L, Heartbeat.from("100").pingMillis)
    }

    @Test
    fun waitsAreSaidToATenth() {
        assertEquals(listOf("0.5s", "1s", "30s", "0.1s", "0.3s"), listOf(500L, 1000, 30000, 96, 320).map(::seconds))
    }

    @Test
    fun anEndedSessionIsClassifiedByTypeAndCloseCode() {
        assertEquals(Failure.AuthRejected, sessionFailure(IOException("rejected: pixel is not known"), CloseCode.REJECTED))
        assertEquals(Failure.AuthRejected, sessionFailure(EndpointMismatchException("endpoint mismatch"), 0))
        assertEquals(Failure.Incompatible, sessionFailure(IncompatibleException("incompatible session"), 0))
        assertEquals(Failure.Incompatible, sessionFailure(RefusedException("this access point is incompatible"), 0))
        assertEquals(Failure.Retryable, sessionFailure(IOException("replaced by a new connection"), CloseCode.REPLACED))
        assertEquals(Failure.Retryable, sessionFailure(null, 0))
    }

    @Test
    fun onlyLoopbackGoesWithoutTls() {
        assertEquals("127.0.0.1", dialHost("ws://127.0.0.1:7443/rutis"))
        assertEquals("::1", dialHost("ws://[::1]:7443/rutis"))
        assertEquals("localhost", dialHost("ws://localhost/rutis"))
        assertEquals("rutis.example.com", dialHost("wss://rutis.example.com:7443/rutis"))
        assertTrue(assertFailsWith<IllegalArgumentException> { dialHost("ws://10.0.2.2:7443/rutis") }.message!!.endsWith("a non-loopback address needs wss://"))
        assertTrue(assertFailsWith<IllegalArgumentException> { dialHost("http://127.0.0.1/rutis") }.message!!.endsWith("is not a ws:// or wss:// address"))

        val at = ListenAddress.parse("wss://0.0.0.0:7443/rutis")
        assertEquals(listOf("0.0.0.0", "7443", "/rutis", "true"), listOf(at.host, at.port.toString(), at.path, at.secure.toString()))
        assertEquals("/", ListenAddress.parse("ws://127.0.0.1:0").path)
        assertTrue(assertFailsWith<IllegalArgumentException> { ListenAddress.parse("ws://0.0.0.0:0/rutis") }.message!!.endsWith("only a loopback listener may go without TLS"))
    }

    @Test
    fun aProtocolRefusalSaysWhichEndToUpgrade() {
        assertEquals(
            "this endpoint speaks rutis.4, the dialer offered rutis.5: upgrade this endpoint (the Kotlin SDK io.github.arcships:rutis)",
            protocolRefusal("rutis.4", listOf("rutis.5")),
        )
        assertEquals("this endpoint speaks rutis.4, the dialer offered rutis.3: upgrade the dialer", protocolRefusal("rutis.4", listOf("rutis.3")))
        assertEquals("this endpoint speaks rutis.4, the dialer offered rutis.3, rutis.5", protocolRefusal("rutis.4", listOf("rutis.3", "rutis.5")))
        assertEquals("this endpoint speaks rutis.4", protocolRefusal("rutis.4", listOf("chat")))
    }

    private fun resource(name: String): String = javaClass.getResource("/tls/$name")!!.readText()

    /** Every key form a listener takes: its key signs what its certificate verifies. */
    @Test
    fun aListenerTakesPkcs8Pkcs1AndSec1Keys() {
        for ((certificate, key) in listOf("rsa.crt" to "rsa-pkcs1.key", "ec.crt" to "ec-sec1.key", "ec-pkcs8.crt" to "ec-pkcs8.key")) {
            val chain = Tls.certificates(resource(certificate))
            val private = Tls.privateKey(resource(key))
            val algorithm = if (private.algorithm == "RSA") "SHA256withRSA" else "SHA256withECDSA"
            val signature = Signature.getInstance(algorithm).apply {
                initSign(private)
                update(byteArrayOf(1, 2, 3))
            }.sign()
            val verified = Signature.getInstance(algorithm).apply {
                initVerify(chain[0].publicKey)
                update(byteArrayOf(1, 2, 3))
            }.verify(signature)
            assertTrue(verified, key)
            Tls.server(resource(certificate), resource(key))
        }
        assertFailsWith<IllegalArgumentException> { Tls.certificates("no PEM here") }
        assertFailsWith<IllegalArgumentException> { Tls.privateKey(resource("rsa.crt")) }
    }
}
