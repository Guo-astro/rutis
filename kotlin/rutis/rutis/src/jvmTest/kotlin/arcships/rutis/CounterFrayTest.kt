package arcships.rutis

import arcships.rutis.internal.Counter
import org.junit.jupiter.api.extension.ExtendWith
import org.pastalab.fray.junit.junit5.FrayTestExtension
import org.pastalab.fray.junit.junit5.annotations.FrayTest
import kotlin.test.assertTrue

/**
 * `Counter` under Fray's scheduler (#630): every add/done pair from four
 * threads leaves no invocation running, whatever the interleaving of the
 * counter's lock and the threads' exits.
 */
@ExtendWith(FrayTestExtension::class)
class CounterFrayTest {
    @FrayTest(iterations = 50)
    fun pairedAddsAndDonesLeaveTheCounterIdle() {
        val counter = Counter()
        val threads = (1..4).map {
            Thread {
                repeat(4) {
                    counter.add()
                    counter.done()
                }
            }
        }
        threads.forEach(Thread::start)
        threads.forEach(Thread::join)
        assertTrue(counter.await(0), "an add/done pair was lost")
    }
}
