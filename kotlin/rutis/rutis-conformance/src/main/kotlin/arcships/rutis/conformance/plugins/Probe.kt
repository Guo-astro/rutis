// The plugins of crates/rutis-bridge/tests/kotlin_runtime.rs: what crosses
// the session (cancellation reaching the coroutine, errors with a code and
// data in both directions and thrown on), and the row a host service sees
// calling it.
package arcships.rutis.conformance.plugins

import arcships.rutis.Rutis
import arcships.rutis.RutisClient
import arcships.rutis.RutisException
import arcships.rutis.RutisService
import arcships.rutis.service
import kotlinx.coroutines.awaitCancellation
import kotlinx.serialization.Serializable

/** The `work` service. */
@RutisService
class ProbeWork {
    @Volatile
    private var started = false

    @Volatile
    private var aborted = false

    /** Waits until its caller cancels: the call's coroutine is cancelled. */
    suspend fun wait() {
        started = true
        try {
            awaitCancellation()
        } finally {
            aborted = true
        }
    }

    fun started(): Boolean = started

    fun aborted(): Boolean = aborted

    fun ping(): String = "pong"

    /** Throws an error with a code and data. */
    fun fail(): String = throw WeatherError()

    /** Calls `callback`; what it throws is thrown on as it is. */
    fun relay(callback: () -> String): String = callback()

    /** Calls `callback` and says what it threw: its name, code and data. */
    fun inspect(callback: () -> String): String = try {
        callback()
    } catch (e: RutisException) {
        "${e.name} ${e.code} ${e.data}"
    }
}

val probe = Rutis.define<Rutis.NoConfig>(
    name = "probe",
    provides = mapOf("work" to service<ProbeWork>(sync = setOf("started", "aborted", "ping", "fail", "relay", "inspect"))),
) { ctx, _ ->
    ctx.provide("work", ProbeWork())
}

@Serializable
data class Registration(val name: String)

@RutisClient
interface Ledger {
    fun register(name: String)
}

/** Registers its name with `ledger`, a host service that sees the calling row. */
val registrant = Rutis.define<Registration>(name = "registrant", inject = listOf("ledger")) { ctx, config ->
    ctx.use<Ledger>("ledger").register(config.name)
}
