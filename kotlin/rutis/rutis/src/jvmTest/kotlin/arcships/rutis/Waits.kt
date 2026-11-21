package arcships.rutis

import kotlin.test.assertTrue

/** How long a wait in a test lasts before it fails as hung: it only guards against a hang. */
internal const val HANG_GUARD_MILLIS = 10_000L

/**
 * Waits until `condition` holds, at most [HANG_GUARD_MILLIS]; else the test
 * fails, naming `what`. A wait a test needs between two observations: it only
 * guards against a hang and is never what a test measures (docs/ci.md,
 * "Waits and `clock:` markers").
 */
internal fun until(what: String, condition: () -> Boolean) {
    val deadline = System.nanoTime() + HANG_GUARD_MILLIS * 1_000_000
    while (!condition()) {
        assertTrue(System.nanoTime() < deadline, "timed out ($HANG_GUARD_MILLIS ms): $what")
        Thread.sleep(10)
    }
}
