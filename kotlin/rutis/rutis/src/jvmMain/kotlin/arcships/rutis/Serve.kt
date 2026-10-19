package arcships.rutis

import arcships.rutis.internal.RuntimeSession
import arcships.rutis.internal.peer.LocalChannels
import arcships.rutis.internal.refusedLine
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import kotlin.system.exitProcess

private const val USAGE = "usage: <program> --rutis-manifest | <channel> [--id <endpoint>] [--peer <endpoint>] <project>"

@Volatile
private var projectDir: String = ""

/** The project directory the host gave this runtime (its last argument). */
val Rutis.project: String get() = projectDir

/**
 * The desktop test entry (jvm target): runs `plugins` as a rutis runtime as
 * the command line says, and exits the process. `<channel> <project>`: the
 * channel is `unix:<path>` (or a bare path) or `tcp:127.0.0.1:<port>`;
 * `--rutis-manifest` prints what the plugins declare. Exit status 0 when the
 * session ended and the rows unloaded; 1 when the host was refused (it does
 * not declare events-v2) or cleanups did not finish within
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

/** What the plugins declare, as `--rutis-manifest` prints it. */
fun Rutis.manifest(vararg plugins: Plugin): String {
    RuntimeSession.checkPlugins(plugins.toList())
    val state = arcships.rutis.internal.RuntimeState(plugins.toList())
    val manifest = buildJsonObject {
        put("manifest", 1)
        put("sdk", VERSION)
        put("pluginApi", PLUGIN_API)
        put("runtime", "rutis-kotlin-runtime:1")
        put("platform", "jvm")
        put("plugins", JsonObject(plugins.associate { it.name to jsonOf(state.describe(it)) }))
    }
    return Json.encodeToString(JsonElement.serializer(), manifest)
}

private fun jsonOf(value: Any?): JsonElement = arcships.rutis.internal.peer.plain(value)

/** Serves the session `args` name; returns the exit status. */
private fun serveArgs(args: List<String>, plugins: List<Plugin>): Int {
    RuntimeSession.checkPlugins(plugins)
    if (args.size == 1 && args[0] == "--rutis-manifest") {
        println(Rutis.manifest(*plugins.toTypedArray()))
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
    if (channel.startsWith("listen:") || channel.startsWith("dial:") || channel.startsWith("ws:") || channel.startsWith("wss:")) {
        throw IllegalArgumentException("network channels come with the Kotlin SDK's next phase (K-P2): $channel")
    }
    val session = RuntimeSession.start(LocalChannels.open(channel), null, plugins)
    session.peer.closedFuture.join()
    val ended = session.end()
    session.refusal?.let {
        System.err.println("rutis: " + refusedLine(it))
        return 1
    }
    if (!ended) {
        System.err.println("rutis: cleanups did not finish within the shutdown deadline after the session ended")
        return 1
    }
    return 0
}

