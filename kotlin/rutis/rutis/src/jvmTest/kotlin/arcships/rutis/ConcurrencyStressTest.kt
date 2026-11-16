package arcships.rutis

import arcships.rutis.internal.Counter
import arcships.rutis.internal.Queue
import java.util.Collections
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicInteger
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertTrue

/**
 * The concurrency of the runtime's small structures under the real
 * scheduler (#630): `Queue` runs what is pushed to it one job at a time,
 * every job once; `Counter` counts invocations until none run.
 */
class ConcurrencyStressTest {
    @Test
    fun everyJobPushedConcurrentlyRunsOnceAndOneAtATime() {
        repeat(20) { round ->
            val queue = Queue("stress-$round")
            val pushers = 4
            val each = 50
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
            assertTrue(done.await(10, TimeUnit.SECONDS), "round $round: the jobs did not all run")
            assertEquals(jobs, ran.size, "round $round: a job ran twice or not at all")
            assertFalse(overlapped.get(), "round $round: two jobs ran at once")
            for (pusher in 1..pushers) {
                assertEquals(
                    (0 until each).toList(),
                    ran.filter { it.first == pusher }.map { it.second },
                    "round $round: pusher $pusher's jobs ran out of order",
                )
            }
        }
    }

    @Test
    fun counterWaitsForEveryRunningInvocation() {
        val counter = Counter()
        val added = CountDownLatch(8)
        val release = CountDownLatch(1)
        val threads = (1..8).map {
            Thread {
                counter.add()
                added.countDown()
                release.await(10, TimeUnit.SECONDS)
                counter.done()
            }
        }
        threads.forEach(Thread::start)
        assertTrue(added.await(10, TimeUnit.SECONDS), "the invocations did not all add")
        assertFalse(counter.await(50), "the counter was idle while eight invocations ran")
        release.countDown()
        threads.forEach(Thread::join)
        assertTrue(counter.await(10_000), "the counter did not become idle after every done")
    }
}
