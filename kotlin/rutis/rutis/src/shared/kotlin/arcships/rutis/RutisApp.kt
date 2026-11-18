package arcships.rutis

import arcships.rutis.internal.AppLink
import arcships.rutis.internal.AppLinks
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

    /** rutis is connected: the plugins are installed, and rutis uses them. */
    data class Connected(val address: String) : RutisState
}

/**
 * An application hosting rutis plugins (design-android-runtime §五): once
 * connected it offers rutis the `services` family and installs [plugins],
 * in order, each with its configuration ([configs], by plugin name; without
 * one, its schema's defaults) once every service it injects is there (its
 * own plugins' or rutis's); the services they provide reach rutis, as
 * `<id>:<plugin>`, once rutis offers `services`
 * (docs/design-remote-session-2026-10-28.md §2.6, decision 2).
 *
 * ```kotlin
 * val rutis = RutisApp(
 *     plugins = listOf(contacts, notify, device),
 *     configs = mapOf("notify" to NotifyConfig(channel = "rutis sample")),
 *     connect = "wss://rutis.example.com:7443/rutis",
 *     id = "kim-pixel",
 *     token = secrets.rutisToken,
 * )
 * rutis.start()   // connects, installs the plugins, and stays connected until stop()
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
 * self-reported id (`[a-z0-9-]+`): rutis registers its plugins as `<id>:<plugin>`; [token] the
 * shared token rutis expects; [peer] rutis's own id, when it must be checked.
 *
 * The SDK only connects: when to [start] and [stop] (foreground,
 * background, a foreground service) is the application's business. Lost
 * connections are dialed again by the rules every runtime follows
 * (0.5 s doubling up to 30 s; 30 s after refused credentials); every
 * connection is a new session: the plugins clean up when one is lost,
 * latest first, and are installed again on the next.
 * [state] says where it stands; the SDK decides nothing by it.
 */
class RutisApp internal constructor(
    val plugins: List<Plugin>,
    /** The configurations of plugins, by plugin name: values of their configuration types; a plugin without one gets its schema's defaults. */
    configs: Map<String, Any?>,
    val connect: String?,
    val listen: String?,
    val id: String,
    private val token: String,
    val peer: String?,
    private val trust: String?,
    private val certificate: String?,
    /** How long cleanups may take when plugins unload. */
    val shutdownTimeout: Duration,
    /** Where the state lines go, without the `rutis: ` prefix. */
    private val log: (String) -> Unit,
    /** What a start connects with: the real WebSocket links, or a test's (#1274). */
    private val links: AppLinks,
) {
    constructor(
        plugins: List<Plugin>,
        configs: Map<String, Any?> = emptyMap(),
        connect: String? = null,
        listen: String? = null,
        id: String,
        token: String,
        peer: String? = null,
        trust: String? = null,
        certificate: String? = null,
        shutdownTimeout: Duration = 10.seconds,
        log: (String) -> Unit = { System.err.println("rutis: $it") },
    ) : this(plugins, configs, connect, listen, id, token, peer, trust, certificate, shutdownTimeout, log, AppLinks.REAL)

    private val endpoint: Endpoint

    /** The plugins with their configurations, encoded. */
    private val own: List<Pair<Plugin, Any?>>
    private val states = MutableStateFlow<RutisState>(RutisState.Stopped())

    /** Where the application stands with rutis. */
    val state: StateFlow<RutisState> = states.asStateFlow()

    private val lock = Object()

    /**
     * One start: its thread, the one it waits for, and how to stop what it
     * connects. A thread serves only the run it was started for.
     */
    private class Run(val previous: Run?) {
        lateinit var thread: Thread

        /** How to stop what this run connected, once it has; read and set under [lock]. */
        var stop: (() -> Unit)? = null

        /** Whether [stop] was asked: this run must not connect, and stops what it connected; read and set under [lock]. */
        var stopping = false
    }

    private var current: Run? = null

    /**
     * The thread of the current run, for the controlled-scheduling tests
     * (#1274): a run sets its last state before its thread ends, so joining
     * it says the run is over without waiting on the clock.
     */
    internal val runThread: Thread?
        get() = synchronized(lock) { current?.thread }

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
        val names = plugins.map { it.name }.toSet()
        configs.keys.sorted().firstOrNull { it !in names }?.let {
            throw IllegalArgumentException("rutis: configs names $it, which is not one of the plugins: ${names.sorted().joinToString(", ")}")
        }
        own = plugins.map { plugin ->
            if (!configs.containsKey(plugin.name)) return@map plugin to arcships.rutis.internal.peer.Undefined
            val config = configs[plugin.name]
            plugin to try {
                arcships.rutis.advanced.RutisFormat.encode(plugin.config, config)
            } catch (e: Exception) {
                throw IllegalArgumentException("rutis: plugin ${plugin.name}: $config is not its configuration: ${e.message}", e)
            }
        }
    }

    /**
     * Connects (or listens) and, once rutis greeted, installs the plugins;
     * lost connections are dialed again, until [stop]. Returns
     * at once; [state] follows. Starting while running does nothing;
     * starting while a stop is still unloading starts once it is done.
     */
    fun start() {
        synchronized(lock) {
            val previous = current
            if (previous != null && !previous.stopping && previous.thread.isAlive) return
            states.value = RutisState.Connecting()
            val started = Run(previous)
            started.thread = Thread({ run(started) }, "rutis-app-$id").apply { isDaemon = true }
            current = started
            started.thread.start()
        }
    }

    /**
     * Unloads every plugin (their cleanups in reverse) and disconnects; it
     * may [start] again. Returns at once: [state] is [RutisState.Stopped]
     * once the plugins are unloaded and the connection is closed.
     */
    fun stop() {
        val stop = synchronized(lock) {
            val run = current ?: return
            run.stopping = true
            run.stop
        }
        stop?.invoke()
    }

    private fun run(started: Run) {
        started.previous?.thread?.join()
        synchronized(lock) {
            // A later start took over: it has its own state, and its own connection.
            if (current !== started) return
            // Stopped while it waited for the one before it: it must not dial or bind.
            if (started.stopping) {
                states.value = RutisState.Stopped()
                return
            }
        }
        val ended = try {
            if (connect != null) dial(started, connect) else listen(started, listen!!)
        } catch (e: Exception) {
            Ended.Refused(e.message ?: e.toString())
        }
        val reason = when (ended) {
            is Ended.Refused -> refusedLine(ended.reason).also(log)
            is Ended.Stopped -> if (ended.clean) null else "cleanups did not finish within ${shutdownTimeout.inWholeMilliseconds} ms".also(log)
        }
        synchronized(lock) {
            // A start after this stop has its own state.
            if (current === started) states.value = RutisState.Stopped(reason)
        }
    }

    /**
     * What `run` connects with, made and given its stop hook under [lock]: a
     * [stop] that came first is seen here (null: it must not connect), and
     * one that comes after stops it at once, so nothing is dialed or bound
     * once a stop returned.
     */
    private fun <T : Any> connecting(run: Run, make: () -> T, stop: (T) -> Unit): T? =
        synchronized(lock) {
            if (current !== run || run.stopping) null
            else make().also { run.stop = { stop(it) } }
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
        val link = connecting(
            run,
            make = {
                links.open(status(run)) {
                    val extra = trust?.let(Tls::certificates).orEmpty()
                    val tls = if (address.startsWith("wss:")) Tls.trust(extra) else null
                    val dialer = WebSocketDialer(address, PROTOCOL, token, tls, retry, Heartbeat())
                    try {
                        val dialing = Dialing(
                            address, endpoint, plugins, retry, dialer::dial,
                            say = log, status = status(run), shutdownMillis = shutdownTimeout.inWholeMilliseconds, own = own,
                        )
                        object : AppLink {
                            override val bound: String? = null

                            override fun run(): Ended = try {
                                dialing.run()
                            } finally {
                                dialer.close()
                            }

                            override fun stop() = dialing.stop()
                        }
                    } catch (e: Throwable) {
                        dialer.close()
                        throw e
                    }
                }
            },
            stop = AppLink::stop,
        ) ?: return Ended.Stopped(true)
        return link.run()
    }

    private fun listen(run: Run, address: String): Ended {
        val at = ListenAddress.parse(address)
        val link = connecting(
            run,
            // Bound under the lock: a stop that came first is seen before a listener exists.
            make = {
                links.open(status(run)) {
                    val tls = if (at.secure) Tls.server(certificate!!, certificate) else null
                    val server = WebSocketServer(at, PROTOCOL, token, tls, Heartbeat())
                    val listening = Listening(
                        server, endpoint, plugins, status = status(run),
                        shutdownMillis = shutdownTimeout.inWholeMilliseconds, own = own,
                    )
                    object : AppLink {
                        override val bound: String = server.address

                        override fun run(): Ended = listening.run()

                        override fun stop() = listening.stop()
                    }
                }
            },
            stop = AppLink::stop,
        ) ?: return Ended.Stopped(true)
        link.bound?.let { log("listening on $it") }
        return link.run()
    }

    private companion object {
        const val PROTOCOL = "rutis.$ENDPOINT_PROTOCOL"
    }
}
