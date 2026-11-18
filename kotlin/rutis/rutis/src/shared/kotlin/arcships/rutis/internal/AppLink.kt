package arcships.rutis.internal

import arcships.rutis.internal.remote.Ended
import arcships.rutis.internal.remote.Status

/**
 * What one start of an application connects with (#1274): a session that
 * dials an access point and dials again, or waits for one to dial in, until
 * [stop]. The real one is a WebSocket client or listener, made by
 * `RutisApp`; the Fray cases make one they drive themselves, because
 * Fray 0.9.2 cannot schedule the WebSocket threads (see `RutisAppFrayTest`).
 */
internal interface AppLink {
    /** Where a listener bound, for its `listening on …` line: null when dialing. */
    val bound: String?

    /** Runs until stopped or refused: [Ended.Stopped] or [Ended.Refused]. */
    fun run(): Ended

    /** Stops it: [run] returns once the rows went and the connection closed. */
    fun stop()
}

/**
 * What a start connects with (#1274): [open] is called under the
 * application's lock, once a stop that came first was seen, and returns the
 * link to run outside it. [real] makes the real WebSocket link; a test that
 * drives its own does not call it.
 */
internal fun interface AppLinks {
    fun open(status: (Status) -> Unit, real: () -> AppLink): AppLink

    companion object {
        /** The real links: every start connects over WebSocket. */
        val REAL: AppLinks = AppLinks { _, real -> real() }
    }
}
