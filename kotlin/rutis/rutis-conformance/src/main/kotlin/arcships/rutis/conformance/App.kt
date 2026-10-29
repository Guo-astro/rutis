// An application hosting the test plugins through RutisApp, on the desktop
// (crates/rutis-host/tests/kotlin.rs): `java -cp rutis-conformance-all.jar
// arcships.rutis.conformance.AppKt connect|listen <address> <id>`, the token
// in RUTIS_TOKEN. Once connected it installs `probe` and `marker` (its
// config: the application's id), which rutis sees as `<id>:probe` and
// `<id>:marker`. It reads commands from stdin (`start`, `stop`) as an
// application's business layer would call them, and prints each state it
// goes through on stdout: `state: <state>`.
package arcships.rutis.conformance

import arcships.rutis.RutisApp
import arcships.rutis.conformance.plugins.marker
import arcships.rutis.conformance.plugins.probe
import kotlinx.coroutines.runBlocking
import kotlin.system.exitProcess

fun main(args: Array<String>) {
    if (args.size != 3 || args[0] !in setOf("connect", "listen")) {
        System.err.println("usage: AppKt connect|listen <address> <id>")
        exitProcess(2)
    }
    val (mode, address, id) = args
    val app = RutisApp(
        plugins = listOf(probe, marker),
        configs = mapOf("marker" to id),
        connect = address.takeIf { mode == "connect" },
        listen = address.takeIf { mode == "listen" },
        id = id,
        token = System.getenv("RUTIS_TOKEN") ?: "",
    )
    Thread({
        runBlocking {
            app.state.collect {
                println("state: $it")
                System.out.flush()
            }
        }
    }, "states").apply { isDaemon = true }.start()
    while (true) {
        when (val command = readlnOrNull()?.trim() ?: break) {
            "start" -> app.start()
            "stop" -> app.stop()
            else -> System.err.println("unknown command $command")
        }
    }
    exitProcess(0)
}
