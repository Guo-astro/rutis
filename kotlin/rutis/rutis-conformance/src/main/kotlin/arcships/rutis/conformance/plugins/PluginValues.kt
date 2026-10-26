// The Kotlin plugins of the plugin-values suite
// (crates/rutis-loader/tests/plugin_values.rs,
// docs/design-plugin-values-2026-10-25.md §9): the same origin as the
// JavaScript one, handing plugin definitions to a Rust service.
package arcships.rutis.conformance.plugins

import arcships.rutis.Plugin
import arcships.rutis.Rutis
import arcships.rutis.RutisClient
import arcships.rutis.RutisService
import kotlinx.serialization.Serializable
import kotlin.concurrent.thread
import kotlin.system.exitProcess

/** The part registry, a Rust service: `register` takes a plugin definition, `takesData` only data. */
@RutisClient
interface ValuesParts {
    suspend fun register(scope: String, definition: Plugin)

    suspend fun takesData(value: Plugin): String

    suspend fun latest(scope: String): Plugin?
}

/** The registry through a blocking method: the calling thread waits for the answer. */
@RutisClient
interface ValuesPartsBlocking {
    fun register(scope: String, definition: Plugin)
}

/** The instance's `tools`. */
@RutisClient
interface ValuesTools {
    fun title(): String
}

/** `maker`: returns a new definition each time. */
@RutisService
class ValuesMaker(private val made: () -> Plugin) {
    fun make(): Plugin = made()
}

@Serializable
data class PartConfig(val n: String)

/** A session part using what its origin's `apply` prepared. */
fun valuesPart(tag: String, prepared: String): Plugin =
    Rutis.define<PartConfig>(name = "values_part", inject = listOf("tools", "probe")) { ctx, config ->
        val probe = ctx.use<Probe>("probe")
        val title = ctx.use<ValuesTools>("tools").title()
        probe.record("$tag ${config.n} sees $title with $prepared")
        ctx.effect { probe.record("$tag ${config.n} down") }
    }

/** The origin: prepares a value, hands a session part using it to `parts`, and provides `maker`. */
val valuesOrigin = Rutis.define<Rutis.NoConfig>(name = "values_origin", inject = listOf("parts", "probe")) { ctx, _ ->
    val probe = ctx.use<Probe>("probe")
    val parts = ctx.use<ValuesParts>("parts")
    val prepared = "prepared in apply"
    val part = valuesPart("part", prepared)
    parts.register("session", part)
    // Handed back by Rust, it comes home as itself.
    val back = parts.latest("session")
    probe.record(if (back === part) "part came home as itself" else "part came home as another")
    try {
        parts.takesData(valuesPart("data", prepared))
        probe.record("data took a definition")
    } catch (e: Exception) {
        probe.record("data refused: ${e.message}")
    }
    ctx.provide("maker", ValuesMaker { valuesPart("made", prepared) })
    ctx.effect { probe.record("origin down") }
}

/**
 * Hands a definition over through a blocking call: rutis reads it
 * (`plugins.read`) while this thread waits for the call's answer.
 */
val valuesBlocking = Rutis.define<Rutis.NoConfig>(name = "values_blocking", inject = listOf("parts", "probe")) { ctx, _ ->
    val probe = ctx.use<Probe>("probe")
    ctx.use<ValuesPartsBlocking>("parts").register("blocking", valuesPart("blocking", "prepared in apply"))
    probe.record("blocking register answered")
}

/** Ends its runtime's process soon after it loads. */
val valuesCrash = Rutis.define<Rutis.NoConfig>(name = "values_crash") { _, _ ->
    thread(isDaemon = true) {
        Thread.sleep(200)
        exitProcess(17)
    }
}
