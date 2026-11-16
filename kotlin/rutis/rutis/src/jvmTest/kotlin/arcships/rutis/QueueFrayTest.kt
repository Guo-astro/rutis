package arcships.rutis

import arcships.rutis.internal.Queue
import org.junit.jupiter.api.extension.ExtendWith
import org.pastalab.fray.junit.junit5.FrayTestExtension
import org.pastalab.fray.junit.junit5.annotations.FrayTest
import java.util.Collections
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicInteger
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertTrue

/**
 * `Queue` under Fray's scheduler (#630): pushes from several threads run
 * every job exactly once, one at a time, in each pusher's order.
 */
@ExtendWith(FrayTestExtension::class)
class QueueFrayTest {
    @FrayTest(iterations = 20)
    fun everyJobPushedConcurrentlyRunsOnceAndOneAtATime() {
        val queue = Queue("fray-queue")
        val pushers = 4
        val each = 3
        val jobs = pushers * each
        val ran = Collections.synchronizedList(ArrayList<Pair<Int, Int>>())
        val active = AtomicInteger()
        val overlapped = AtomicBoolean()
        val done = CountDownLatch(jobs)
        val threads = (1..pushers).map { pusher ->
            Thread {
                repeat(each) { n ->
                    queue.push {
                        if (active.incrementAndGet() != 1) overlapped.set(true)
                        ran.add(pusher to n)
                        active.decrementAndGet()
                        done.countDown()
                    }
                }
            }
        }
        threads.forEach(Thread::start)
        threads.forEach(Thread::join)
        assertTrue(done.await(10, TimeUnit.SECONDS), "the jobs did not all run")
        assertEquals(jobs, ran.size, "a job ran twice or not at all: $ran")
        assertFalse(overlapped.get(), "two jobs ran at once")
        for (pusher in 1..pushers) {
            assertEquals(
                (0 until each).toList(),
                ran.filter { it.first == pusher }.map { it.second },
                "pusher $pusher's jobs ran out of order: $ran",
            )
        }
    }
}
