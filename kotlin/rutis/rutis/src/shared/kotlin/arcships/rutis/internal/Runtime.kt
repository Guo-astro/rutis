package arcships.rutis.internal

import arcships.rutis.Ctx
import arcships.rutis.generated.DecodePaths
import arcships.rutis.Plugin
import arcships.rutis.Rutis
import arcships.rutis.RutisException
import arcships.rutis.advanced.RutisFormat
import arcships.rutis.advanced.Scope
import arcships.rutis.noMethod
import arcships.rutis.serve
import arcships.rutis.generated.ServiceObject
import arcships.rutis.advanced.ServiceShape
import arcships.rutis.generated.ServiceTable
import arcships.rutis.generated.Tables
import arcships.rutis.internal.peer.CallChain
import arcships.rutis.internal.peer.Peer
import arcships.rutis.internal.peer.Undefined
import kotlinx.coroutines.CompletableJob
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancelAndJoin
import kotlinx.coroutines.future.await
import kotlinx.coroutines.runInterruptible
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeoutOrNull
import kotlinx.serialization.json.JsonObject
import java.util.concurrent.CompletableFuture
import arcships.rutis.advanced.PLUGIN_API

/**
 * What this runtime supports of the runtime contract. `rows.announce`:
 * services rows provide without declaring them are announced to the host
 * with their methods. `plugin-values`: a plugin definition
 * this runtime handed over comes back to be described (`plugins.read`) and
 * loaded as a row's entry (docs/design-plugin-values-2026-10-25.md §5).
 */
val FEATURES: List<String> = listOf("rows.v2", "hosts", "leaf", "scopes", "rows.announce", "plugin-values")

/** How the service `name` is identified in the scope `label`: the name, or the name, NUL and the label. */
fun scopedId(name: String, label: String?, scoped: Boolean): String {
    if ('\u0000' in name || (label != null && '\u0000' in label)) throw IllegalArgumentException("service \"$name\" or its scope label contains NUL")
    if (!scoped) return name
    if (label.isNullOrEmpty()) throw IllegalArgumentException("service \"$name\" has an empty scope label")
    return name + "\u0000" + label
}

fun handleOf(id: String, generation: Int): String {
    if (generation == 1) return id
    val separator = if ('\u0000' in id) "\u0000" else "#"
    return id + separator + generation
}

fun printable(id: String): String = id.replace('\u0000', '@')

/** Counts running invocations; waits until none run. */
class Counter {
    private var count = 0
    private val lock = Object()

    fun add() = synchronized(lock) { count++ }

    fun done() = synchronized(lock) {
        count--
        if (count <= 0) lock.notifyAll()
    }

    /** Waits until none run, at most `millis`; whether none run. */
    fun await(millis: Long): Boolean = synchronized(lock) {
        val deadline = System.currentTimeMillis() + millis
        while (count > 0) {
            val left = deadline - System.currentTimeMillis()
            if (left <= 0) return false
            lock.wait(left)
        }
        true
    }
}

internal class Provided(val row: String, val value: Any, val shape: ServiceShape?, val table: ServiceTable<Any>)

internal class Slot(val row: String, val name: String, var methods: Set<String>?, val announced: Boolean = false) {
    var obj: Any? = null
    var present = false
    var handle: String? = null
    var generation = 0
}

internal class HandleEntry(val id: String, val name: String, val obj: Any, val table: ServiceTable<Any>, val methods: Set<String>?) {
    var current = true
    var released = false
}

internal class Row(
    val key: String,
    /** The row's id in the configuration (rows.load's `id`; the key when not given). */
    val name: String,
    val entry: String,
    val plugin: Plugin,
    val config: Any?,
    val exports: Map<String, Map<String, String>>,
    val isolate: Map<String, String>,
    /** Its scope chain, outermost first. */
    val scope: List<Scope>,
    /** The chain of the call that loaded it, which its own code keeps. */
    val chain: List<String>,
) {
    val job: CompletableJob = SupervisorJob()
    val cleanups = ArrayList<suspend () -> Unit>()
    val provided = ArrayList<String>()
    val announced = ArrayList<String>()
    val listening = Counter()
    var order = 0L

    /** Announcements and registrations the host has yet to answer while the row loads; each completes with a refusal or null. */
    var pending: MutableList<CompletableFuture<Throwable?>>? = ArrayList()
    var loading = true

    fun id(name: String): String {
        val label = isolate[name]
        return try {
            scopedId(name, label, label != null)
        } catch (_: IllegalArgumentException) {
            name
        }
    }
}

/** The rows one controller loads into this process. */
class RuntimeState(plugins: List<Plugin>) {
    internal val plugins: Map<String, Plugin> = plugins.associateBy { it.name }

    /**
     * How long a row's listeners and cleanups may take when it goes, and
     * the whole unloading when the session ends (`RUTIS_SHUTDOWN_TIMEOUT`,
     * or what an application gives `RutisApp`).
     */
    @Volatile
    var shutdownMillis: Long = Shutdown.millis()

    @Volatile
    var session: Peer? = null

    internal val lock = Object()
    internal val rows = HashMap<String, Row>()
    internal val services = HashMap<String, Provided>()
    internal val hosts = HashMap<String, Map<String, String>>()
    internal val slots = HashMap<String, Slot>()
    internal val handles = HashMap<String, HandleEntry>()
    private var version = 0L
    private var loaded = 0L

    @Volatile
    internal var closing = false

    // The listeners of each name, in position order, and by id.
    internal val listeners = HashMap<String, MutableList<LocalListener>>()
    internal val byId = HashMap<String, LocalListener>()
    internal var listenerSeq = 0L
    internal var first = 0L
    internal var last = 0L

    // The names and prefixes with listeners outside this process
    // (events.elsewhere), and the emits through the host not answered yet.
    internal val elsewhere = HashSet<String>()
    internal val elsewherePrefixes = HashSet<String>()
    internal val hostEmits = HashMap<String, Int>()

    /** What the host calls this runtime (mount `runtime`), for descriptors. */
    @Volatile
    internal var runtimeName: String? = null

    private fun names(): String = plugins.keys.sorted().joinToString(", ")

    internal fun definition(entry: String): Plugin =
        supported(plugins[entry] ?: throw RutisException("NotFound", "plugin $entry is not in this runtime; it has: ${names()}"))

    /** `plugin`, unless it needs a newer plugin API than this runtime's. */
    internal fun supported(plugin: Plugin): Plugin {
        if (plugin.api > arcships.rutis.advanced.PLUGIN_API) throw IllegalStateException("plugin ${plugin.name} needs plugin API ${plugin.api}; this runtime supports ${arcships.rutis.advanced.PLUGIN_API}")
        return plugin
    }

    // ── Services ─────────────────────────────────────────────────

    /** Reports the object now in an exported slot under a new handle; `lock` is held. */
    internal fun refresh(id: String) {
        val slot = slots[id] ?: return
        val provided = services[id]?.takeIf { it.row == slot.row }
        val present = provided != null
        val current = provided?.value
        if (present == slot.present && current === slot.obj) return
        slot.handle?.let(::retire)
        slot.obj = current
        slot.present = present
        slot.handle = null
        if (provided != null) {
            slot.generation++
            val handle = handleOf(id, slot.generation)
            slot.handle = handle
            if (slot.announced) slot.methods = provided.table.methods.toSet()
            handles[handle] = HandleEntry(id, slot.name, provided.value, provided.table, slot.methods)
        }
        version++
        val session = session ?: return
        if (closing) return
        val handle = slot.handle
        if (!slot.announced) {
            session.notify("", "service", listOf(id, handle, version))
            return
        }
        // An announced service says whose it is and what it offers, and a row
        // loading answers once rutis has heard it (a refusal fails the row there).
        val methods = LinkedHashMap<String, Any?>()
        for (method in slot.methods ?: emptySet()) methods[method] = "async"
        val answered = session.announce("", "service", listOf(id, handle, version, linkedMapOf("row" to slot.row, "methods" to methods)))
        val row = rows[slot.row]
        if (row != null && row.loading) row.pending?.add(answered.thenApply { null })
    }

    private fun retire(handle: String) {
        val entry = handles[handle] ?: return
        entry.current = false
        if (entry.released) handles.remove(handle)
    }

    internal fun provide(row: Row, name: String, value: Any): () -> Unit {
        val table = Tables.serviceTable(value.javaClass)
            ?: throw IllegalArgumentException("rutis: provide $name: ${value.javaClass.name} is not a @RutisService class (no generated method table: apply the rutis-ksp processor)")
        val shape = row.plugin.provides[name]
        val type = shape?.type
        if (type != null && !type.isInstance(value)) {
            throw IllegalArgumentException("rutis: provide $name: ${value.javaClass.name} is not the ${type.name} provides declares")
        }
        val id = row.id(name)
        var inactive = false
        synchronized(lock) {
            if (rows[row.key] !== row) {
                // A task of an unloaded row providing: refused, or the
                // service would outlive its row and the next load find it
                // taken.
                inactive = true
            } else {
                services[id]?.let { throw IllegalStateException("$name is already provided by ${rows[it.row]?.name ?: it.row}") }
                services[id] = Provided(row.key, value, shape, table)
                row.provided.add(id)
                // Not declared: announced, with its methods.
                if (slots[id] == null && '#' !in name) {
                    slots[id] = Slot(row.key, name, null, announced = true)
                    row.announced.add(id)
                }
                refresh(id)
            }
        }
        if (inactive) {
            Report.line("rutis: row ${row.name} is not active: service $name is not provided")
            return {}
        }
        return {
            synchronized(lock) {
                val existing = services[id]
                if (existing != null && existing.row == row.key && existing.value === value) {
                    services.remove(id)
                    row.provided.remove(id)
                    refresh(id)
                }
            }
        }
    }

    // ── Rows ─────────────────────────────────────────────────────

    /**
     * Loads row `key`, whose id in the configuration is `name`, running
     * `plugin`: the one `entry` names in this runtime, or a plugin definition
     * this runtime handed over (a copy, `entry` its name).
     */
    internal suspend fun load(
        key: String,
        name: String,
        entry: String,
        config: Any?,
        exports: Map<String, Map<String, String>>,
        isolate: Map<String, String>,
        scope: List<Scope>,
        plugin: Plugin = definition(entry),
    ) {
        val row = Row(key, name.ifEmpty { key }, entry, plugin, config, exports, isolate, scope, CallChain.current())
        synchronized(lock) {
            if (rows.containsKey(key)) throw IllegalStateException("row $key is already loaded")
            for ((service, label) in isolate) scopedId(service, label, true)
            for (service in exports.keys) {
                if ('#' in service) throw IllegalArgumentException("service name $service cannot be projected")
                slots[row.id(service)]?.let { throw IllegalStateException("$service is already provided by ${rows[it.row]?.name ?: it.row}") }
            }
            loaded++
            row.order = loaded
            rows[key] = row
            for ((service, methods) in exports) slots[row.id(service)] = Slot(key, service, methods.keys.toSet())
        }
        val failure = try {
            val decoded = decodeConfig(plugin, config)
            withContext(row.job + CallChain.element(row.chain)) {
                plugin.apply(Ctx(this@RuntimeState, row), decoded)
            }
            null
        } catch (e: Throwable) {
            e
        }
        if (failure != null) {
            try {
                unload(key)
            } catch (e: Throwable) {
                failure.addSuppressed(e)
            }
            throw failure
        }
        val pending = synchronized(lock) {
            for (service in exports.keys) refresh(row.id(service))
            val pending = row.pending ?: emptyList()
            row.pending = null
            row.loading = false
            pending
        }
        // rutis has published the announced services (or refused one, which
        // fails the row there) and registered the row's listeners before it
        // hears that the row loaded. A refused listener fails the load (§4.4).
        var refused: Throwable? = null
        for (wait in pending) {
            val error = wait.await()
            if (error != null && refused == null) refused = error
        }
        if (refused != null) {
            try {
                unload(key)
            } catch (e: Throwable) {
                refused.addSuppressed(e)
            }
            throw refused
        }
    }

    private fun decodeConfig(plugin: Plugin, raw: Any?): Any? {
        // Defaults filled and the config validated against the plugin's
        // schema before decoding (config schema design §2.2); a decoder
        // error after that is the schema and the type disagreeing (§3.3).
        var config = raw
        val schema = plugin.schema
        if (schema is kotlinx.serialization.json.JsonObject) {
            // A config of plain data (one holding dates or bytes, which only
            // Kotlin's own callers send, is decoded as it is).
            val data = try {
                Result.success(ConfigSchema.fromValue(raw))
            } catch (e: ConfigSchema.NotData) {
                Result.failure(e)
            }
            if (data.isSuccess) {
                val checked = ConfigSchema.check(ConfigSchema.fromValue(schema), data.getOrNull())
                if (checked.errors.isNotEmpty()) throw configError(checked.errors)
                config = ConfigSchema.toJson(checked.config)
            }
            // No config: `{}` for a class, as before (a nullable value gets null).
            val kind = plugin.config.descriptor.kind
            val structured = kind == kotlinx.serialization.descriptors.StructureKind.CLASS || kind == kotlinx.serialization.descriptors.StructureKind.OBJECT
            val absent = config == null || config === Undefined || config == kotlinx.serialization.json.JsonNull
            return try {
                RutisFormat.decode(plugin.config, if (absent && structured) JsonObject(emptyMap()) else config)
            } catch (e: Exception) {
                // Any failure of decoding (the decoder's, or an `init` check
                // of the type's): one sentence, as in Swift.
                if (e is kotlinx.coroutines.CancellationException) throw e
                throw configError(listOf(ConfigSchema.Issue(DecodePaths.of(e), "${ConfigSchema.pathText(DecodePaths.of(e))} is not a value the plugin accepts")))
            }
        }
        val value = if ((config == null || config === Undefined || config == kotlinx.serialization.json.JsonNull) &&
            plugin.config.descriptor.kind.let { it == kotlinx.serialization.descriptors.StructureKind.CLASS || it == kotlinx.serialization.descriptors.StructureKind.OBJECT }
        ) {
            JsonObject(emptyMap())
        } else {
            config
        }
        return try {
            RutisFormat.decode(plugin.config, value)
        } catch (e: kotlinx.serialization.SerializationException) {
            throw IllegalArgumentException("config: ${e.message}", e)
        } catch (e: arcships.rutis.internal.peer.ValueException) {
            throw IllegalArgumentException("config: ${e.message}", e)
        }
    }

    /**
     * The error a row fails with when its config does not match: a
     * BindingError with code `bad-config`, its message a `config: …` line
     * per problem, its data `{ issues: [{ path, message }] }` (design §5.2).
     */
    private fun configError(issues: List<ConfigSchema.Issue>): arcships.rutis.RutisException =
        arcships.rutis.RutisException(
            "BindingError",
            ConfigSchema.lines(issues).joinToString("\n"),
            code = "bad-config",
            data = mapOf("issues" to issues.map { mapOf("path" to it.path, "message" to it.message) }),
        )

    /**
     * Unloads row `key`. Returns false when it gave up (listeners or
     * cleanups still running at the deadline); a failing cleanup does not
     * stop the others, and the first failure is thrown once they ran.
     */
    internal suspend fun unload(key: String, deadlineNanos: Long? = null): Boolean {
        val (done, failure) = unloadRow(key, deadlineNanos)
        failure?.let { throw it }
        return done
    }

    /** How a row unloaded: whether it finished in time, and its cleanups' failure. */
    private data class Unloaded(val done: Boolean, val failure: Throwable?)

    private suspend fun unloadRow(key: String, deadlineNanos: Long?): Unloaded {
        val row = synchronized(lock) {
            val row = rows.remove(key) ?: return Unloaded(true, null)
            EventsRuntime.forgetListeners(this, row)
            // Withdrawals first: rutis hears them before the plugin goes away.
            for (id in row.provided.toList()) {
                val existing = services[id]
                if (existing != null && existing.row == key) {
                    services.remove(id)
                    refresh(id)
                }
            }
            row.provided.clear()
            val ids = row.announced.toMutableList()
            for (service in row.exports.keys) ids.add(row.id(service))
            for (id in ids) {
                val slot = slots[id]
                if (slot != null && slot.row == key) {
                    slots.remove(id)
                    slot.handle?.let(::retire)
                }
            }
            row.announced.clear()
            row
        }
        // The row's coroutines are cancelled; its running listeners waited
        // for, at most the shutdown deadline (§7.1); then its cleanups run,
        // latest first.
        row.job.cancel()
        fun leftMillis(): Long? = deadlineNanos?.let { ((it - System.nanoTime()) / 1_000_000).coerceAtLeast(0) }
        val budget = leftMillis() ?: shutdownMillis
        val listened = if (deadlineNanos == null) {
            row.listening.await(budget)
        } else {
            withTimeoutOrNull(budget) {
                runInterruptible(Dispatchers.IO) { row.listening.await(budget) }
            } ?: false
        }
        var gaveUp = !listened
        if (!listened) {
            Report.line("rutis: listeners of row ${row.name} still running after ${Shutdown.shown(budget)}s; unloading anyway")
        }
        val joinBudget = leftMillis() ?: budget
        if (joinBudget > 0) withTimeoutOrNull(joinBudget) { row.job.cancelAndJoin() }
        var failure: Throwable? = null
        for (cleanup in row.cleanups.asReversed()) {
            val left = leftMillis()
            if (deadlineNanos != null && left == 0L) {
                Report.line("rutis: gave up the cleanups of row ${row.name}")
                gaveUp = true
                break
            }
            try {
                val done = if (deadlineNanos == null) {
                    withContext(CallChain.element(row.chain)) { cleanup() }
                    true
                } else {
                    withTimeoutOrNull(left!!) {
                        withContext(CallChain.element(row.chain)) { cleanup() }
                    } != null
                }
                if (!done) {
                    Report.line("rutis: gave up the cleanups of row ${row.name}")
                    gaveUp = true
                    break
                }
            } catch (e: Throwable) {
                if (failure == null) failure = e else failure.addSuppressed(e)
            }
        }
        return Unloaded(!gaveUp, failure)
    }

    internal suspend fun update(key: String, config: Any?) {
        val row = synchronized(lock) { rows[key] } ?: throw IllegalStateException("row $key is not loaded")
        // Leaf plugins have no volatile fields: a new config restarts the row.
        unload(key)
        load(key, row.name, row.entry, config, row.exports, row.isolate, row.scope, row.plugin)
    }

    /** Registers a cleanup if `row` is still the loaded row with `key`. */
    internal fun addCleanup(row: Row, cleanup: suspend () -> Unit): Boolean = synchronized(lock) {
        if (rows[row.key] !== row) return false
        row.cleanups.add(cleanup)
        true
    }

    /**
     * Unloads every row, latest first. Returns false when it gave up
     * cleanups at the deadline. The deadline bounds waiting for listeners
     * and suspending cleanups; a cleanup that blocks its thread without
     * suspending cannot be interrupted, so plugins should not do that.
     * Only the deadline gives up: a failing cleanup is reported and the
     * other rows still unload.
     */
    suspend fun dispose(budgetMillis: Long? = null): Boolean {
        val ordered = synchronized(lock) { rows.values.sortedBy { it.order } }
        val deadline = budgetMillis?.let { System.nanoTime() + it * 1_000_000 }
        var all = true
        for (row in ordered.asReversed()) {
            val left = deadline?.let { ((it - System.nanoTime()) / 1_000_000).coerceAtLeast(0) }
            if (deadline != null && left == 0L) {
                System.err.println("rutis: gave up the cleanups of row ${row.name} and those after it")
                return false
            }
            val outcome = if (deadline == null) {
                unloadRow(row.key, null)
            } else {
                withTimeoutOrNull(left!!) { unloadRow(row.key, deadline) } ?: Unloaded(false, null)
            }
            outcome.failure?.let {
                System.err.println("rutis: a cleanup of row ${row.name} failed: ${Report.failure(it)}")
            }
            if (!outcome.done) {
                all = false
                // Without a deadline every row has its own budget: go on.
                if (deadline != null) {
                    System.err.println("rutis: gave up the cleanups of row ${row.name} and those after it")
                    return false
                }
            }
        }
        return all
    }

    // ── Dispatch ─────────────────────────────────────────────────

    /** Serves `invoke`: a control operation (target "") or a method of an exported service object. */
    suspend fun dispatch(target: String, method: String, args: List<Any?>): Any? {
        if (closing) throw IllegalStateException("runtime is closing")
        if (target.isEmpty()) return Control.control(this, method, args)
        val entry = synchronized(lock) { handles[target] } ?: throw IllegalArgumentException("unknown or released service object ${printable(target)}")
        val methods = entry.methods
        if ((methods != null && method !in methods) || method !in entry.table.methods) {
            throw noMethod(entry.name, method, methods ?: entry.table.methods)
        }
        return entry.table.serve(entry.obj, method, args, entry.name)
    }

    internal fun mounted(runtime: String?) = synchronized(lock) {
        this.runtimeName = runtime
    }

    internal fun released(handle: String) = synchronized(lock) {
        val entry = handles[handle] ?: return@synchronized
        entry.released = true
        if (!entry.current) handles.remove(handle)
    }

    internal fun property(handle: String, property: String): Any? {
        val entry = synchronized(lock) { handles[handle] } ?: throw IllegalArgumentException("unknown or released service object ${printable(handle)}")
        throw IllegalArgumentException("no property $property of ${printable(entry.id)}: a service's methods cross, not its fields")
    }

    /** What a plugin definition this runtime handed over declares (`plugins.read`), with its name. */
    internal fun describeDefinition(value: Any?): Map<String, Any?> {
        val plugin = value as? Plugin ?: throw IllegalArgumentException("not a plugin definition: expected a Rutis.define result, got ${arcships.rutis.internal.peer.describe(value)}")
        return linkedMapOf<String, Any?>("name" to plugin.name) + describe(supported(plugin))
    }

    /** The plugin's declarations, as `rows.schema` answers them. */
    internal fun describe(plugin: Plugin): Map<String, Any?> = linkedMapOf(
        "config" to (plugin.schema ?: kotlinx.serialization.json.JsonNull),
        "inject" to plugin.inject,
        "provides" to LinkedHashMap<String, Any?>().apply { for ((name, shape) in plugin.provides) put(name, LinkedHashMap<String, Any?>(shape.kinds)) },
        "version" to null,
        // Every plugin of this SDK may provide services `provides` does not
        // list: they reach rutis by themselves.
        "announces" to true,
    )

}
