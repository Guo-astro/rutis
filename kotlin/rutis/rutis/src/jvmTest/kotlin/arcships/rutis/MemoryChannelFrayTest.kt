package arcships.rutis

import arcships.rutis.internal.peer.MemoryChannel
import org.junit.jupiter.api.extension.ExtendWith
import org.pastalab.fray.junit.junit5.FrayTestExtension
import org.pastalab.fray.junit.junit5.annotations.FrayTest
import java.io.IOException
import java.util.Collections
import java.util.concurrent.atomic.AtomicReference
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertNull

/**
 * [`MemoryChannel`] under Fray's scheduler (#630): the two ends of a pair
 * keep message boundaries and order when both send and receive at once, and
 * an end's close reaches the other side's blocked receive.
 */
@ExtendWith(FrayTestExtension::class)
class MemoryChannelFrayTest {
    /**
     * One sender and one receiver on each end, four threads at once: each end
     * receives exactly what the other sent, whole and in the sender's order.
     */
    @FrayTest(iterations = 20, resetClassLoaderPerIteration = false)
    fun concurrentSendsAndReceivesKeepEveryMessageWholeAndInOrder() {
        val (a, b) = MemoryChannel.pair()
        val messages = 3
        val receivedByA = Collections.synchronizedList(ArrayList<String>())
        val receivedByB = Collections.synchronizedList(ArrayList<String>())
        val receivers = listOf(
            Thread { repeat(messages) { receivedByA.add(String(a.recv()!!, Charsets.UTF_8)) } },
            Thread { repeat(messages) { receivedByB.add(String(b.recv()!!, Charsets.UTF_8)) } },
        )
        val senders = listOf(
            Thread { repeat(messages) { a.send("a-$it ✓".toByteArray(Charsets.UTF_8)) } },
            Thread { repeat(messages) { b.send("b-$it ✓".toByteArray(Charsets.UTF_8)) } },
        )
        receivers.forEach(Thread::start)
        senders.forEach(Thread::start)
        (receivers + senders).forEach(Thread::join)
        assertEquals((0 until messages).map { "b-$it ✓" }, receivedByA)
        assertEquals((0 until messages).map { "a-$it ✓" }, receivedByB)
    }

    /**
     * Closing one end wakes the other end's receive with null, and every
     * receive after it returns null too; a send on the closed end fails.
     */
    @FrayTest(iterations = 20, resetClassLoaderPerIteration = false)
    fun closingWakesAReceiveAndRefusesFurtherSends() {
        val (a, b) = MemoryChannel.pair()
        val received = AtomicReference<ByteArray?>()
        val receiver = Thread { received.set(b.recv()) }
        receiver.start()
        a.close("done")
        receiver.join()
        assertNull(received.get(), "the blocked receive did not see the end")
        assertNull(b.recv(), "a receive after the end returned a message")
        assertNull(a.recv(), "the closing end still had a message")
        assertFailsWith<IOException>("a send on the closed end went out") { a.send(byteArrayOf(1)) }
    }
}
