package arcships.rutis

import arcships.rutis.internal.RuntimeSession
import arcships.rutis.internal.peer.ENDPOINT_PROTOCOL
import arcships.rutis.internal.peer.Endpoint
import arcships.rutis.internal.peer.validId
import arcships.rutis.internal.remote.Dialing
import arcships.rutis.internal.remote.Ended
import arcships.rutis.internal.remote.Heartbeat
import arcships.rutis.internal.remote.ListenAddress
import arcships.rutis.internal.remote.Listening
import arcships.rutis.internal.remote.Retry
import arcships.rutis.internal.remote.Status
import arcships.rutis.internal.remote.Tls
import arcships.rutis.internal.remote.WebSocketDialer
import arcships.rutis.internal.remote.WebSocketServer
import arcships.rutis.internal.remote.dialHost
import arcships.rutis.internal.refusedLine
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlin.time.Duration
import kotlin.time.Duration.Companion.seconds

/** Where an application stands with rutis, for its business layer to show. */
sealed interface RutisState {
    /** Not started, stopped, or given up: `reason` says why when it was not [RutisApp.stop]. */
    data class Stopped(val reason: String? = null) : RutisState

    /** Connecting, or waiting to connect again; `retrying` is the last failure, if any. */
    data class Connecting(val retrying: String? = null) : RutisState

    /** rutis is connected and drives the plugins. */
    data class Connected(val address: String) : RutisState
}

/**
 * An application hosting rutis plugins (design-android-runtime §五): it
 * reports [plugins] to rutis, which decides which of them load, with what
 * configuration, and when they unload.
 *
 * ```kotlin
 * val rutis = RutisApp(
 *     plugins = listOf(contacts, notify, device),
 *     connect = "wss://rutis.example.com:7443/rutis",
 *     id = "kim-pixel",
 *     token = secrets.rutisToken,
 * )
 * rutis.start()   // connects, and stays connected until stop()
 * rutis.stop()    // every plugin's cleanups in reverse, then disconnects
 * ```
 *
 * [connect] dials a rutis access point (recommended: the application is
 * behind NAT, its address changes); [listen] waits for rutis to dial in
 * instead (a device with a fixed address), one controller at a time, a newer
 * connection taking over. A non-loopback address needs `wss://`: [trust]
 * holds the PEM certificates of CAs trusted besides the system's, for a
 * self-signed access point; [certificate] the PEM certificate chain and
 * private key a `wss://` listener serves. [id] is this connection's
 * self-reported id (`[a-z0-9-]+`), its instance's name in rutis; [token] the
 * shared token rutis expects; [peer] rutis's own id, when it must be checked.
 *
 * The SDK only connects: when to [start] and [stop] (foreground,
 * background, a foreground service) is the application's business. Lost
 * connections are dialed again by the rules every runtime follows
 * (0.5 s doubling up to 30 s; 30 s after refused credentials); every
 * connection is a new session, loaded afresh by rutis's configuration.
 * [state] says where it stands; the SDK decides nothing by it.
 */
class RutisApp(
    val plugins: List<Plugin>,
    val connect: String? = null,
    val listen: String? = null,
    val id: String,
    private val token: String,
    val peer: String? = null,
    private val trust: String? = null,
    private val certificate: String? = null,
    /** How long cleanups may take when plugins unload. */
    val shutdownTimeout: Duration = 10.seconds,
    /** Where the state lines go, without the `rutis: ` prefix. */
    private val log: (String) -> Unit = { System.err.println("rutis: $it") },
) {
    private val endpoint: Endpoint
    private val states = MutableStateFlow<RutisState>(RutisState.Stopped())

    /** Where the application stands with rutis. */
    val state: StateFlow<RutisState> = states.asStateFlow()

    private val lock = Object()

    /** The running connection: its thread, and how to stop it. */
    private class Run(val thread: Thread, @Volatile var stop: (() -> Unit)?) {
        @Volatile
        var stopping = false
    }

    private var current: Run? = null

    init {
        RuntimeSession.checkPlugins(plugins)
        require((connect == null) != (listen == null)) { "rutis: give connect (an access point to dial) or listen (an address to wait on), not both" }
        require(validId(id)) { "rutis: invalid id $id: use lower-case letters, digits and -" }
        require(peer == null || validId(peer)) { "rutis: invalid peer $peer: use lower-case letters, digits and -" }
        require(shutdownTimeout.isPositive()) { "rutis: shutdownTimeout must be positive" }
        if (connect != null) dialHost(connect)
        if (listen != null) {
            val at = ListenAddress.parse(listen)
            require(!at.secure || certificate != null) { "rutis: $listen: wss needs a certificate (its PEM chain and private key)" }
        }
        endpoint = Endpoint(id, peer, listOf("runtime"))
    }

    /**
     * Connects (or listens), reports the plugins, and from then on rutis
     * drives them; lost connections are dialed again, until [stop]. Returns
     * at once; [state] follows. Starting while running does nothing;
     * starting while a stop is still unloading starts once it is done.
     */
    fun start() {
        synchronized(lock) {
            val previous = current
            if (previous != null && !previous.stopping && previous.thread.isAlive) return
            states.value = RutisState.Connecting()
            val run = Run(Thread({ run(previous) }, "rutis-app-$id").apply { isDaemon = true }, null)
            current = run
            run.thread.start()
        }
    }

    /**
     * Unloads every plugin (their cleanups in reverse) and disconnects; it
     * may [start] again. Returns at once: [state] is [RutisState.Stopped]
     * once the plugins are unloaded and the connection is closed.
     */
    fun stop() {
        val run = synchronized(lock) { current } ?: return
        run.stopping = true
        run.stop?.invoke()
    }

    private fun run(previous: Run?) {
        previous?.thread?.join()
        val run = synchronized(lock) { current } ?: return
        val ended = try {
            if (connect != null) dial(run, connect) else listen(run, listen!!)
        } catch (e: Exception) {
            Ended.Refused(e.message ?: e.toString())
        }
        val reason = when (ended) {
            is Ended.Refused -> refusedLine(ended.reason).also(log)
            is Ended.Stopped -> if (ended.clean) null else "cleanups did not finish within ${shutdownTimeout.inWholeMilliseconds} ms".also(log)
        }
        synchronized(lock) {
            // A start after this stop has its own state.
            if (current === run) states.value = RutisState.Stopped(reason)
        }
    }

    /** Sets what stops `run`; when it was asked to stop already, stops at once. */
    private fun stopping(run: Run, stop: () -> Unit) {
        run.stop = stop
        if (run.stopping) stop()
    }

    /**
     * What `run` reports, unless a later start took over or it is stopping:
     * a session greeted as it stops says nothing (it ends at Stopped).
     */
    private fun status(run: Run): (Status) -> Unit = { status ->
        synchronized(lock) {
            if (current === run && !run.stopping) {
                states.value = when (status) {
                    is Status.Connected -> RutisState.Connected(status.address)
                    is Status.Connecting -> RutisState.Connecting(status.retry)
                }
            }
        }
    }

    private fun dial(run: Run, address: String): Ended {
        val retry = Retry()
        val extra = trust?.let(Tls::certificates).orEmpty()
        val tls = if (address.startsWith("wss:")) Tls.trust(extra) else null
        val dialer = WebSocketDialer(address, PROTOCOL, token, tls, retry, Heartbeat())
        try {
            val dialing = Dialing(
                address, endpoint, plugins, retry, dialer::dial,
                say = log, status = status(run), shutdownMillis = shutdownTimeout.inWholeMilliseconds,
            )
            stopping(run, dialing::stop)
            return dialing.run()
        } finally {
            dialer.close()
        }
    }

    private fun listen(run: Run, address: String): Ended {
        val at = ListenAddress.parse(address)
        val tls = if (at.secure) Tls.server(certificate!!, certificate) else null
        val server = WebSocketServer(at, PROTOCOL, token, tls, Heartbeat())
        log("listening on ${server.address}")
        val listening = Listening(server, endpoint, plugins, status = status(run), shutdownMillis = shutdownTimeout.inWholeMilliseconds)
        stopping(run, listening::stop)
        return listening.run()
    }

    private companion object {
        const val PROTOCOL = "rutis.$ENDPOINT_PROTOCOL"
    }
}
