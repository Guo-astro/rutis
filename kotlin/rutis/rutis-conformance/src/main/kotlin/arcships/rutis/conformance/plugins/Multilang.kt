// The Kotlin plugins of the cross-language suites
// (crates/rutis-loader/tests/multilang_kotlin.rs): leaf plugins that provide
// and use services across Node and Python, one gated on a host service, one
// providing a service it does not declare, and a relay for the wait-cycle
// check. Each records what it does through the host service `probe`.
package arcships.rutis.conformance.plugins

import arcships.rutis.Rutis
import arcships.rutis.RutisClient
import arcships.rutis.RutisException
import arcships.rutis.RutisService
import arcships.rutis.service

@RutisClient
interface Probe {
    fun record(line: String)
}

/** `js_weather` and `py_weather`, as the Kotlin consumer uses them. */
@RutisClient
interface Weather2 {
    fun today(): String

    suspend fun later(): String
}

@RutisClient
interface Llm {
    fun ask(): String
}

/** `b_work` of the wait-cycle suite. */
@RutisClient
interface Work {
    fun work(): String
}

/** The interface of `kt_weather`: the provider's own object implements it. */
@RutisClient
interface KtWeatherApi {
    fun today(): String

    suspend fun later(): String
}

/** An error with a code and data, as a plain Kotlin exception declares them. */
class WeatherError : RuntimeException("too windy") {
    val code: String = "Stormy"
    val data: Map<String, Any?> = mapOf("wind" to (1L shl 60))
}

@RutisService
class KtWeather : KtWeatherApi {
    override fun today(): String = "kt sunny"

    override suspend fun later(): String = "kt later"

    /** Calls back during the (synchronous) call. */
    fun each(days: List<String>, transform: (String) -> String): List<String> = days.map(transform)

    suspend fun fail(): String = throw WeatherError()
}

@RutisService
class KtForecast {
    fun tomorrow(): String = "kt tomorrow"
}

@RutisService
class Relay(private val work: Work) {
    /** Calls `b_work` synchronously, on the caller's chain. */
    fun work(): String = try {
        "kt_relay relays ${work.work()}"
    } catch (e: RutisException) {
        if (e.name != RutisException.SYNC_WAIT_CYCLE) throw e
        "kt_relay: SyncWaitCycle"
    }
}

val ktProvider = Rutis.define<Rutis.NoConfig>(
    name = "kt_provider",
    inject = listOf("probe"),
    provides = mapOf("kt_weather" to service<KtWeather>(sync = setOf("today", "each"))),
) { ctx, _ ->
    val probe = ctx.use<Probe>("probe")
    ctx.provide("kt_weather", KtWeather())
    probe.record("kt provider: start")
    ctx.effect { probe.record("kt provider: bye") }
}

val ktConsumer = Rutis.define<Rutis.NoConfig>(name = "kt_consumer", inject = listOf("js_weather", "py_weather", "probe")) { ctx, _ ->
    val probe = ctx.use<Probe>("probe")
    val js = ctx.use<Weather2>("js_weather")
    val py = ctx.use<Weather2>("py_weather")
    probe.record("kt consumer: ${js.today()} / ${js.later()} / ${py.today()} / ${py.later()}")
    ctx.effect { probe.record("kt consumer: bye") }
}

val ktLocal = Rutis.define<Rutis.NoConfig>(name = "kt_local", inject = listOf("kt_weather", "probe")) { ctx, _ ->
    val probe = ctx.use<Probe>("probe")
    // Within one process, a service is the provider's own object.
    val weather = ctx.use<KtWeatherApi>("kt_weather")
    probe.record("kt local: " + (if (weather is KtWeather) "native" else "proxy") + " / ${weather.later()}")
    ctx.effect { probe.record("kt local: bye") }
}

val ktGated = Rutis.define<Rutis.NoConfig>(name = "kt_gated", inject = listOf("llm", "probe")) { ctx, _ ->
    val probe = ctx.use<Probe>("probe")
    probe.record("kt gated: ${ctx.use<Llm>("llm").ask()}")
    ctx.effect { probe.record("kt gated: bye") }
}

/** Provides `kt_forecast` without declaring it: rutis hears of it with its methods, all async. */
val ktForecast = Rutis.define<Rutis.NoConfig>(name = "kt_forecast", inject = listOf("probe")) { ctx, _ ->
    ctx.provide("kt_forecast", KtForecast())
    ctx.use<Probe>("probe").record("kt forecast: start")
}

val ktRelay = Rutis.define<Rutis.NoConfig>(
    name = "kt_relay",
    inject = listOf("b_work"),
    provides = mapOf("g_relay" to service<Relay>(sync = setOf("work"))),
) { ctx, _ ->
    ctx.provide("g_relay", Relay(ctx.use<Work>("b_work")))
}
