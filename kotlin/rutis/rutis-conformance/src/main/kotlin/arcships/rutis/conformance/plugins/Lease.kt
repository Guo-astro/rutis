// The plugin of crates/rutis-host/tests/lease_e2e.rs's Kotlin column: it
// writes every start, call and cleanup to `log`, provides `log`, and its
// cleanup waits for `stop`; a call waits in `hold` for `hold`. The test
// decides when those finish, so the log says whether the new controller
// was greeted before the old lease was gone.
package arcships.rutis.conformance.plugins

import arcships.rutis.Rutis
import arcships.rutis.RutisService
import arcships.rutis.service
import kotlinx.coroutines.delay
import kotlinx.serialization.Serializable
import java.io.File

@Serializable
data class LeaseConfig(val who: String, val log: String, val stop: String, val hold: String)

private fun write(path: String, line: String) {
    File(path).appendText(line + "\n")
}

private suspend fun waitFor(path: String) {
    while (!File(path).exists()) delay(10)
}

@RutisService
class LeaseLogger(private val config: LeaseConfig) {
    suspend fun who(): String = config.who

    suspend fun hold(): String {
        write(config.log, "hold ${config.who}")
        waitFor(config.hold)
        return "held ${config.who}"
    }
}

val lease = Rutis.define<LeaseConfig>(
    name = "kt_lease",
    provides = mapOf("log" to service<LeaseLogger>()),
) { ctx, config ->
    write(config.log, "start ${config.who}")
    ctx.provide("log", LeaseLogger(config))
    ctx.effect {
        write(config.log, "cleanup ${config.who}")
        waitFor(config.stop)
        write(config.log, "stop ${config.who}")
    }
}
