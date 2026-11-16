// The plugin of crates/rutis-bridge/tests/host_gone.rs's Kotlin column: it
// keeps work going that would keep the process alive (a thread nobody
// stops), and its cleanup writes `marker`, then waits for ever when `hang`
// (the runtime gives it up at its deadline).
package arcships.rutis.conformance.plugins

import arcships.rutis.Rutis
import kotlinx.coroutines.awaitCancellation
import kotlinx.serialization.Serializable
import java.io.File

@Serializable
data class HostGoneConfig(val marker: String, val hang: Boolean = false)

val hostGone = Rutis.define<HostGoneConfig>(name = "host_gone") { ctx, config ->
    Thread {
        while (true) {
            Thread.sleep(60_000)
        }
    }.start()
    ctx.effect {
        File(config.marker).writeText("cleaned up")
        if (config.hang) awaitCancellation()
    }
}
