package arcships.rutis.internal.remote

import java.io.ByteArrayInputStream
import java.io.ByteArrayOutputStream
import java.security.KeyFactory
import java.security.KeyStore
import java.security.PrivateKey
import java.security.cert.CertificateException
import java.security.cert.CertificateFactory
import java.security.cert.X509Certificate
import java.security.spec.PKCS8EncodedKeySpec
import java.util.Base64
import javax.net.ssl.KeyManagerFactory
import javax.net.ssl.SSLContext
import javax.net.ssl.TrustManagerFactory
import javax.net.ssl.X509TrustManager

/**
 * PEM certificates and keys, and what TLS is built from them, with the
 * platform's own APIs (the JDK's and Android's): a listener's certificate
 * and key (`RUTIS_CERT`, `RUTIS_KEY`), and the CA certificates a dialer
 * trusts besides the system's (`RUTIS_CA`). Keys stay in this process: no
 * key store on disk, no Android Keystore.
 */
object Tls {
    private class Block(val label: String, val der: ByteArray)

    private fun blocks(pem: String): List<Block> {
        val found = ArrayList<Block>()
        val begin = Regex("-----BEGIN ([A-Z0-9 ]+)-----")
        var at = 0
        while (true) {
            val match = begin.find(pem, at) ?: break
            val label = match.groupValues[1]
            val end = "-----END $label-----"
            val stop = pem.indexOf(end, match.range.last + 1)
            if (stop < 0) throw IllegalArgumentException("PEM block $label has no end")
            val body = pem.substring(match.range.last + 1, stop).filterNot { it.isWhitespace() }
            found.add(Block(label, Base64.getDecoder().decode(body)))
            at = stop + end.length
        }
        return found
    }

    /** The certificates in `pem`, in order. */
    fun certificates(pem: String): List<X509Certificate> {
        val factory = CertificateFactory.getInstance("X.509")
        val certificates = blocks(pem).filter { it.label == "CERTIFICATE" }.map {
            factory.generateCertificate(ByteArrayInputStream(it.der)) as X509Certificate
        }
        if (certificates.isEmpty()) throw IllegalArgumentException("no PEM certificate in it")
        return certificates
    }

    /** The private key in `pem`: PKCS#8, PKCS#1 (RSA) or SEC1 (EC). */
    fun privateKey(pem: String): PrivateKey {
        val block = blocks(pem).firstOrNull { it.label.endsWith("PRIVATE KEY") }
            ?: throw IllegalArgumentException("no PEM private key in it")
        val (algorithms, pkcs8) = when (block.label) {
            "PRIVATE KEY" -> listOf("EC", "RSA") to block.der
            "RSA PRIVATE KEY" -> listOf("RSA") to wrap(RSA, null, block.der)
            "EC PRIVATE KEY" -> listOf("EC") to wrap(EC, curveOf(block.der), block.der)
            else -> throw IllegalArgumentException("${block.label} is not a supported key (PKCS#8, PKCS#1 or SEC1)")
        }
        for (algorithm in algorithms) {
            try {
                return KeyFactory.getInstance(algorithm).generatePrivate(PKCS8EncodedKeySpec(pkcs8))
            } catch (_: java.security.spec.InvalidKeySpecException) {
            }
        }
        throw IllegalArgumentException("the private key is neither an EC nor an RSA key")
    }

    /** TLS for a listener: the certificate chain in `certificatePem`, its key in `keyPem`. */
    fun server(certificatePem: String, keyPem: String): SSLContext {
        val chain = certificates(certificatePem)
        val key = privateKey(keyPem)
        val store = KeyStore.getInstance("PKCS12")
        store.load(null, null)
        val password = CharArray(0)
        store.setKeyEntry("rutis", key, password, chain.toTypedArray())
        val keys = KeyManagerFactory.getInstance(KeyManagerFactory.getDefaultAlgorithm())
        keys.init(store, password)
        val context = SSLContext.getInstance("TLS")
        context.init(keys.keyManagers, null, null)
        return context
    }

    /** The system's trust, and the CA certificates `extra` besides. */
    fun trust(extra: List<X509Certificate>): X509TrustManager {
        val system = trustManager(null)
        if (extra.isEmpty()) return system
        val store = KeyStore.getInstance(KeyStore.getDefaultType())
        store.load(null, null)
        extra.forEachIndexed { i, certificate -> store.setCertificateEntry("rutis-ca-$i", certificate) }
        val own = trustManager(store)
        return object : X509TrustManager {
            override fun checkClientTrusted(chain: Array<out X509Certificate>, authType: String) = system.checkClientTrusted(chain, authType)

            override fun checkServerTrusted(chain: Array<out X509Certificate>, authType: String) {
                try {
                    system.checkServerTrusted(chain, authType)
                } catch (e: CertificateException) {
                    try {
                        own.checkServerTrusted(chain, authType)
                    } catch (_: CertificateException) {
                        throw e
                    }
                }
            }

            override fun getAcceptedIssuers(): Array<X509Certificate> = system.acceptedIssuers + own.acceptedIssuers
        }
    }

    private fun trustManager(store: KeyStore?): X509TrustManager {
        val factory = TrustManagerFactory.getInstance(TrustManagerFactory.getDefaultAlgorithm())
        factory.init(store)
        return factory.trustManagers.filterIsInstance<X509TrustManager>().first()
    }

    // ── DER: wrapping PKCS#1 and SEC1 keys as PKCS#8 ─────────────

    private val RSA = byteArrayOf(0x2A, 0x86.toByte(), 0x48, 0x86.toByte(), 0xF7.toByte(), 0x0D, 0x01, 0x01, 0x01)
    private val EC = byteArrayOf(0x2A, 0x86.toByte(), 0x48, 0xCE.toByte(), 0x3D, 0x02, 0x01)

    private fun der(tag: Int, content: ByteArray): ByteArray {
        val out = ByteArrayOutputStream()
        out.write(tag)
        val n = content.size
        when {
            n < 0x80 -> out.write(n)
            n < 0x100 -> {
                out.write(0x81)
                out.write(n)
            }
            n < 0x10000 -> {
                out.write(0x82)
                out.write(n shr 8)
                out.write(n and 0xFF)
            }
            else -> {
                out.write(0x83)
                out.write(n shr 16)
                out.write((n shr 8) and 0xFF)
                out.write(n and 0xFF)
            }
        }
        out.write(content)
        return out.toByteArray()
    }

    /** PrivateKeyInfo { 0, { algorithm, parameter }, OCTET STRING key }. */
    private fun wrap(algorithm: ByteArray, curve: ByteArray?, key: ByteArray): ByteArray {
        val parameter = curve?.let { der(0x06, it) } ?: byteArrayOf(0x05, 0x00)
        val identifier = der(0x30, der(0x06, algorithm) + parameter)
        return der(0x30, byteArrayOf(0x02, 0x01, 0x00) + identifier + der(0x04, key))
    }

    /** The named curve of a SEC1 ECPrivateKey (its parameters, `[0]`). */
    private fun curveOf(sec1: ByteArray): ByteArray {
        var at = 0
        fun header(): Pair<Int, Int> {
            val tag = sec1[at++].toInt() and 0xFF
            var length = sec1[at++].toInt() and 0xFF
            if (length >= 0x80) {
                val bytes = length and 0x7F
                length = 0
                repeat(bytes) { length = (length shl 8) or (sec1[at++].toInt() and 0xFF) }
            }
            return tag to length
        }
        val (outer, _) = header()
        if (outer != 0x30) throw IllegalArgumentException("not an EC private key")
        while (at < sec1.size) {
            val (tag, length) = header()
            if (tag == 0xA0) {
                val (oid, size) = header()
                if (oid != 0x06) break
                return sec1.copyOfRange(at, at + size)
            }
            at += length
        }
        throw IllegalArgumentException("the EC private key names no curve")
    }
}
