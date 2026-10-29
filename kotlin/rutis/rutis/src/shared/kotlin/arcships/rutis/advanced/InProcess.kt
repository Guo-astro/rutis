package arcships.rutis.advanced

import arcships.rutis.Plugin
import arcships.rutis.Rutis

/**
 * Serves one runtime session in this process and returns at once: for a
 * test tool that runs the plugins it tests in its own process, the far end
 * being `rutis-host test` (the `rutis-testing` artifact does). Not for
 * applications: a runtime program calls `Rutis.serve(args, …)`, an
 * application `RutisApp`.
 *
 * The session is on a connection rutis made, read from `input` and written
 * to `output` (one message per line, as every local channel), greeting as
 * `id`; `close` closes the connection. It starts with no plugins:
 * [InProcessSession.register] adds them while it runs, and rutis asks for
 * each one's declarations when it loads a row of it.
 */
fun Rutis.serveInProcess(input: java.io.InputStream, output: java.io.OutputStream, id: String, close: () -> Unit): InProcessSession {
    if (!arcships.rutis.internal.peer.validId(id)) throw IllegalArgumentException("invalid endpoint id $id: use lower-case letters, digits and -")
    val channel = arcships.rutis.internal.peer.LineChannel(input, output, close)
    val endpoint = arcships.rutis.internal.peer.Endpoint(id, null, listOf("runtime"))
    return InProcessSession(arcships.rutis.internal.RuntimeSession.start(channel, endpoint, emptyList(), open = true))
}

/** A runtime session [serveInProcess] serves. */
class InProcessSession internal constructor(private val session: arcships.rutis.internal.RuntimeSession) {
    /** Adds `plugin` (or replaces the one of its name) for the rows rutis loads from now on. */
    fun register(plugin: Plugin) = session.runtime.register(plugin)

    /** Completes when the session ended, by either side. */
    val ended: java.util.concurrent.CompletableFuture<Unit> get() = session.peer.closedFuture

    /**
     * Ends the session: every row unloads, latest first, within
     * `RUTIS_SHUTDOWN_TIMEOUT`; whether their cleanups finished.
     */
    fun end(): Boolean = session.stop("the test ended")
}
