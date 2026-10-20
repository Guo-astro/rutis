package arcships.rutis.internal

import arcships.rutis.Plugin
import arcships.rutis.Rutis
import arcships.rutis.internal.peer.Channel
import arcships.rutis.internal.peer.EVENTS_CAPABILITY
import arcships.rutis.internal.peer.Endpoint
import arcships.rutis.internal.peer.Options
import arcships.rutis.internal.peer.Peer
import arcships.rutis.internal.peer.RefusedException
import arcships.rutis.internal.peer.SESSION_CAPABILITIES
import kotlinx.coroutines.runBlocking

/** Why a host that does not declare events-v2 is refused. */
const val OLD_EVENTS_HOST: String =
    "this host does not declare events-v2 (the event protocol of rutis 0.13): upgrade rutis-host to 0.13 or later"

/** The state line of a refusal: `refused: incompatible (<reason>); stopping`. */
fun refusedLine(reason: String): String = "refused: incompatible ($reason); stopping"

/** One controller's session and the rows it loaded. */
class RuntimeSession private constructor(val peer: Peer, val runtime: RuntimeState) {
    /** Why this side refused the far end at its handshake, if it did. */
    val refusal: String? get() = (peer.error as? RefusedException)?.reason

    /**
     * Ends the lease: nothing more is read, then every row goes, within the
     * shutdown deadline. Returns false when cleanups were given up.
     */
    fun end(): Boolean {
        peer.close(null)
        runtime.closing = true
        return runBlocking { runtime.dispose(Shutdown.millis()) }
    }

    /**
     * Stops for good, this side's own decision: every row goes while the
     * controller still hears from this end (its cleanups in reverse), then
     * the session and its channel close. Returns false when cleanups were
     * given up.
     */
    fun stop(reason: String): Boolean {
        runtime.closing = true
        val disposed = runBlocking { runtime.dispose(Shutdown.millis()) }
        peer.shutdown(reason)
        return disposed
    }

    companion object {
        /** Starts serving `plugins` on `channel`: compat (no endpoint) or endpoint format. */
        fun start(channel: Channel, endpoint: Endpoint?, plugins: List<Plugin>): RuntimeSession {
            checkPlugins(plugins)
            val runtime = RuntimeState(plugins)
            val peer = Peer(
                channel,
                Options(
                    dispatch = runtime::dispatch,
                    arrive = { method, args -> EventsRuntime.arrive(runtime, method, args) },
                    endpoint = endpoint,
                    implementation = mapOf("name" to Rutis.IMPLEMENTATION, "version" to Rutis.VERSION),
                    capabilities = SESSION_CAPABILITIES,
                    // A host older than 0.13 is refused right after its handshake (#464 §2.2).
                    require = listOf(EVENTS_CAPABILITY),
                    refuse = { OLD_EVENTS_HOST },
                ),
            )
            runtime.session = peer
            peer.start()
            return RuntimeSession(peer, runtime)
        }

        /** Two plugins of one name, or none, cannot be served. */
        fun checkPlugins(plugins: List<Plugin>) {
            if (plugins.isEmpty()) throw IllegalArgumentException("no plugins to serve")
            val seen = HashSet<String>()
            for (plugin in plugins) {
                if (!seen.add(plugin.name)) throw IllegalArgumentException("two plugins are named ${plugin.name}")
            }
        }
    }
}
