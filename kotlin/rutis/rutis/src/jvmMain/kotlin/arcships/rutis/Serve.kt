package arcships.rutis

import arcships.rutis.internal.RuntimeSession
import arcships.rutis.internal.peer.ENDPOINT_PROTOCOL
import arcships.rutis.internal.peer.Endpoint
import arcships.rutis.internal.peer.LocalChannels
import arcships.rutis.internal.peer.validId
import arcships.rutis.internal.refusedLine
import arcships.rutis.internal.remote.Dialing
import arcships.rutis.internal.remote.Ended
import arcships.rutis.internal.remote.Heartbeat
import arcships.rutis.internal.remote.ListenAddress
import arcships.rutis.internal.remote.Listening
import arcships.rutis.internal.remote.Retry
import arcships.rutis.internal.remote.Tls
import arcships.rutis.internal.remote.WebSocketDialer
import arcships.rutis.internal.remote.WebSocketServer
import arcships.rutis.internal.remote.dialHost
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import java.io.File
import java.io.IOException
import kotlin.system.exitProcess

private const val USAGE = "usage: <program> --rutis-manifest | <channel> --id <endpoint> [--peer <endpoint>] <project>"

@Volatile
internal var projectDir: String = ""

/**
 * The desktop entry (jvm target): runs `plugins` as a rutis runtime as the
 * command line says, and exits the process. `<channel> --id <endpoint>
 * <project>`, greeting as `<endpoint>` (and expecting `--peer <endpoint>`,
 * the controller's id, when given): the channel is `unix:<path>` (or a bare
 * path) or `tcp:127.0.0.1:<port>`, a local channel rutis made;
 * `dial:<ws or wss address>` to dial a rutis access point and stay connected, or `listen:<ws or wss
 * address>` to wait for rutis to dial in. `--rutis-manifest` prints what the
 * plugins declare. Exit status 0 when the session ended (or, remotely, the
 * runtime was stopped: SIGTERM, Ctrl-C) and the rows unloaded; 1 when the
 * host was refused (it does not declare events-v2, or the access point is
 * otherwise incompatible) or cleanups did not finish within
 * `RUTIS_SHUTDOWN_TIMEOUT`.
 */
fun Rutis.serve(args: Array<String>, vararg plugins: Plugin): Nothing {
    val status = try {
        serveArgs(args.toList(), plugins.toList())
    } catch (e: Exception) {
        System.err.println("rutis: ${e.message}")
        1
    }
    System.out.flush()
    System.err.flush()
    exitProcess(status)
}

/** `arcships.rutis.advanced.manifest`. */
internal fun manifestOf(plugins: List<Plugin>): String {
    RuntimeSession.checkPlugins(plugins)
    val state = arcships.rutis.internal.RuntimeState(plugins)
    val manifest = buildJsonObject {
        put("manifest", 1)
        put("sdk", arcships.rutis.advanced.VERSION)
        put("pluginApi", arcships.rutis.advanced.PLUGIN_API)
        put("runtime", "rutis-kotlin-runtime:1")
        put("platform", "jvm")
        put("plugins", JsonObject(plugins.associate { it.name to jsonOf(state.describe(it)) }))
    }
    return Json.encodeToString(JsonElement.serializer(), manifest)
}

private fun jsonOf(value: Any?): JsonElement = arcships.rutis.internal.peer.plain(value)

/** Serves the session `args` name; returns the exit status. */
internal fun serveArgs(args: List<String>, plugins: List<Plugin>): Int {
    RuntimeSession.checkPlugins(plugins)
    if (args.size == 1 && args[0] == "--rutis-manifest") {
        println(manifestOf(plugins))
        return 0
    }
    if (args.isEmpty()) throw IllegalArgumentException(USAGE)
    val channel = args[0]
    var rest = args.drop(1)
    val flags = HashMap<String, String>()
    while (rest.isNotEmpty() && rest[0].startsWith("--")) {
        val flag = rest[0].removePrefix("--")
        if (rest.size < 2) throw IllegalArgumentException("--$flag needs a value")
        if (flag != "id" && flag != "peer") throw IllegalArgumentException("unknown flag --$flag; $USAGE")
        flags[flag] = rest[1]
        rest = rest.drop(2)
    }
    if (rest.size != 1) throw IllegalArgumentException(USAGE)
    projectDir = rest[0]
    val network = channel.startsWith("listen:") || channel.startsWith("dial:")
    if (!network && (channel.startsWith("ws:") || channel.startsWith("wss:"))) {
        throw IllegalArgumentException("a WebSocket channel is listen:<address> or dial:<address>: $channel")
    }
    // Every channel greets as --id: a runtime, its controller managing its rows.
    val id = flags["id"] ?: throw IllegalArgumentException("a ${if (network) "network" else "local"} channel needs --id <endpoint>: $channel")
    if (!validId(id)) throw IllegalArgumentException("invalid endpoint id $id: use lower-case letters, digits and -")
    val endpoint = Endpoint(id, flags["peer"], listOf("runtime"))
    if (!network) return serveLocal(channel, endpoint, plugins)
    val ended = if (channel.startsWith("dial:")) {
        dial(channel.removePrefix("dial:"), endpoint, plugins)
    } else {
        listen(channel.removePrefix("listen:"), endpoint, plugins)
    }
    return when (ended) {
        is Ended.Refused -> {
            System.err.println("rutis: " + refusedLine(ended.reason))
            1
        }
        is Ended.Stopped -> if (ended.clean) {
            0
        } else {
            System.err.println("rutis: cleanups did not finish within the shutdown deadline after the runtime stopped")
            1
        }
    }
}

/** A local channel rutis made: one session, then the process ends. */
private fun serveLocal(channel: String, endpoint: Endpoint, plugins: List<Plugin>): Int {
    val session = RuntimeSession.start(LocalChannels.open(channel), endpoint, plugins)
    session.peer.closedFuture.join()
    val ended = session.end()
    session.refusal?.let {
        System.err.println("rutis: " + refusedLine(it))
        return 1
    }
    if (!ended) {
        System.err.println("rutis: cleanups did not finish within the shutdown deadline after the host disconnected")
        return 1
    }
    return 0
}

/** The session protocol of the WebSocket channels. */
private const val PROTOCOL = "rutis.$ENDPOINT_PROTOCOL"

/** Runs `stop` on SIGTERM or Ctrl-C instead of ending the process at once: a remote runtime stops by unloading its rows. */
private fun onStop(stop: () -> Unit) {
    for (name in listOf("TERM", "INT")) {
        try {
            sun.misc.Signal.handle(sun.misc.Signal(name)) { stop() }
        } catch (_: IllegalArgumentException) {
            // A signal this platform does not have.
        }
    }
}

/** The file `path` that the environment variable `variable` names, read. */
private fun readNamed(variable: String, path: String): String = try {
    File(path).readText()
} catch (e: IOException) {
    throw IllegalArgumentException("$variable $path: ${e.message}")
}

/**
 * `dial:<address>`: dials a rutis access point and stays connected (the
 * dial-out rules, design-remote-runtime-dial §六), presenting
 * `RUTIS_TOKEN`; a wss:// one is trusted by the system's CAs and those in
 * the PEM file `RUTIS_CA`.
 */
private fun dial(address: String, endpoint: Endpoint, plugins: List<Plugin>): Ended {
    dialHost(address)
    val trust = if (address.startsWith("wss:")) {
        val extra = System.getenv("RUTIS_CA")?.takeIf { it.isNotEmpty() }?.let { ca ->
            try {
                Tls.certificates(readNamed("RUTIS_CA", ca))
            } catch (e: IllegalArgumentException) {
                throw IllegalArgumentException("RUTIS_CA $ca: ${e.message}")
            }
        }
        Tls.trust(extra ?: emptyList())
    } else {
        null
    }
    val retry = Retry.from(System.getenv("RUTIS_RETRY"))
    val heartbeat = Heartbeat.from(System.getenv("RUTIS_HEARTBEAT"))
    val dialer = WebSocketDialer(address, PROTOCOL, System.getenv("RUTIS_TOKEN"), trust, retry, heartbeat)
    val dialing = Dialing(address, endpoint, plugins, retry, dialer::dial, say = { System.err.println("rutis: $it") })
    onStop(dialing::stop)
    return dialing.run()
}

/**
 * `listen:<address>`: serves one controller at a time on a WebSocket
 * address, taking `RUTIS_TOKEN`; a wss:// one serves the PEM certificate
 * and key in `RUTIS_CERT` and `RUTIS_KEY`.
 */
private fun listen(address: String, endpoint: Endpoint, plugins: List<Plugin>): Ended {
    val at = ListenAddress.parse(address)
    val tls = if (at.secure) {
        val certificate = System.getenv("RUTIS_CERT")?.takeIf { it.isNotEmpty() }
        val key = System.getenv("RUTIS_KEY")?.takeIf { it.isNotEmpty() }
        if (certificate == null || key == null) throw IllegalArgumentException("$address: wss needs a certificate and key (RUTIS_CERT, RUTIS_KEY)")
        Tls.server(readNamed("RUTIS_CERT", certificate), readNamed("RUTIS_KEY", key))
    } else {
        null
    }
    val server = WebSocketServer(at, PROTOCOL, System.getenv("RUTIS_TOKEN"), tls, Heartbeat.from(System.getenv("RUTIS_HEARTBEAT")))
    System.err.println("rutis: listening on ${server.address}")
    val listening = Listening(server, endpoint, plugins)
    onStop(listening::stop)
    return listening.run()
}
