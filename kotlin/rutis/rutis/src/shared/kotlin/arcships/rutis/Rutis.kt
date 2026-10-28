package arcships.rutis

import arcships.rutis.internal.Schema
import kotlinx.serialization.KSerializer
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.JsonElement
import arcships.rutis.advanced.Kind
import arcships.rutis.advanced.RutisFormat
import arcships.rutis.advanced.ServiceShape
import arcships.rutis.generated.Tables

/**
 * Writes rutis plugins in Kotlin (design: docs/design-android-runtime-2026-10-12.md).
 *
 * ```kotlin
 * @Serializable data class Config(val city: String)
 *
 * @RutisClient interface Llm { suspend fun ask(question: String): String }
 *
 * @RutisService class Weather(private val llm: Llm, private val city: String) {
 *     suspend fun today(question: String): String = llm.ask("weather in $city: $question")
 * }
 *
 * val weather = Rutis.define<Config>(name = "weather", inject = listOf("llm")) { ctx, config ->
 *     ctx.provide("weather", Weather(ctx.use<Llm>("llm"), config.city))
 * }
 * ```
 *
 * rutis decides when a plugin runs: `apply` runs once every injected service
 * is there, and the plugin is unloaded when one goes away.
 */
object Rutis {
    /** The configuration of a plugin that takes none. */
    @Serializable
    object NoConfig

    /**
     * Defines a plugin whose configuration is `C`, a `@Serializable` type
     * ([NoConfig] for none): its JSON Schema is derived from `C`'s
     * descriptor unless [schema] gives one.
     *
     * [inject] lists the services it uses; [provides] optionally declares the
     * services it provides and which of their methods are `sync` (undeclared
     * services reach rutis by themselves, every method `async`). `apply`
     * starts it; a failure fails the load, and the cleanups registered so far
     * run.
     */
    inline fun <reified C> define(
        name: String,
        inject: List<String> = emptyList(),
        provides: Map<String, ServiceShape> = emptyMap(),
        schema: JsonElement? = null,
        noinline apply: suspend (Ctx, C) -> Unit,
    ): Plugin = define(name, RutisFormat.serializer<C>(), inject, provides, schema, apply)

    /** [define] with the configuration's serializer given. */
    fun <C> define(
        name: String,
        config: KSerializer<C>,
        inject: List<String> = emptyList(),
        provides: Map<String, ServiceShape> = emptyMap(),
        schema: JsonElement? = null,
        apply: suspend (Ctx, C) -> Unit,
    ): Plugin {
        require(name.isNotEmpty() && name.none { it == ':' || it == '/' || it == '\u0000' }) { "rutis: invalid plugin name \"$name\"" }
        for ((service, shape) in provides) {
            require(service.none { it == '#' || it == '\u0000' }) { "rutis: plugin $name: service name \"$service\" cannot be projected" }
            shape.problem?.let { throw IllegalArgumentException("rutis: plugin $name: provides $service: $it") }
        }
        val derived = when {
            schema != null -> schema
            config.descriptor.serialName == NoConfig.serializer().descriptor.serialName -> null
            else -> Schema.of(config.descriptor)
        }
        @Suppress("UNCHECKED_CAST")
        return Plugin(name, inject.toList(), provides, derived, arcships.rutis.advanced.PLUGIN_API, config as KSerializer<Any?>, apply as suspend (Ctx, Any?) -> Unit)
    }
}

/**
 * The shape of a service of the `@RutisService` class `T`: its methods,
 * `async` but those in [sync].
 */
inline fun <reified T : Any> service(sync: Set<String> = emptySet()): ServiceShape = service(T::class.java, sync)

/** [service] for the class `type`. */
fun service(type: Class<*>, sync: Set<String> = emptySet()): ServiceShape {
    val table = Tables.serviceTable(type)
        ?: return ServiceShape(type, emptyMap(), "${type.name} has no method table: annotate it @RutisService and apply the rutis-ksp processor")
    val unknown = sync.filter { it !in table.methods }
    if (unknown.isNotEmpty()) return ServiceShape(type, emptyMap(), "sync $unknown: ${type.name} has no such methods")
    if (table.methods.isEmpty()) return ServiceShape(type, emptyMap(), "${type.name} has no public methods")
    return ServiceShape(type, table.methods.associateWith { if (it in sync) "sync" else "async" })
}

/** The shape of a service with these methods and kinds. */
fun service(kinds: Map<String, Kind>): ServiceShape = ServiceShape(null, kinds.mapValues { it.value.wire })

/** A defined plugin, for `Rutis.serve` and the testing tool. */
class Plugin internal constructor(
    val name: String,
    val inject: List<String>,
    val provides: Map<String, ServiceShape>,
    internal val schema: JsonElement?,
    internal val api: Int,
    internal val config: KSerializer<Any?>,
    internal val apply: suspend (Ctx, Any?) -> Unit,
)
