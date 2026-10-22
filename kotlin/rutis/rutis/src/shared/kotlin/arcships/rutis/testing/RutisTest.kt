package arcships.rutis.testing

import arcships.rutis.ClientFactory
import arcships.rutis.EventArgs
import arcships.rutis.Listener
import arcships.rutis.Next
import arcships.rutis.Plugin
import arcships.rutis.RemoteFunction
import arcships.rutis.RutisException
import arcships.rutis.Scope
import arcships.rutis.ServiceHandle
import arcships.rutis.ServiceTable
import arcships.rutis.Tables
import arcships.rutis.internal.Call
import arcships.rutis.internal.Dispatch
import arcships.rutis.internal.Entry
import arcships.rutis.internal.EventsRuntime
import arcships.rutis.internal.Mode
import arcships.rutis.internal.Native
import arcships.rutis.internal.Queue
import arcships.rutis.internal.Report
import arcships.rutis.internal.RuntimeSession
import arcships.rutis.internal.Shutdown
import arcships.rutis.internal.checkEmit
import arcships.rutis.internal.checkListen
import arcships.rutis.internal.within
import arcships.rutis.internal.peer.CallChain
import arcships.rutis.internal.peer.Exported
import arcships.rutis.internal.peer.MemoryChannel
import arcships.rutis.internal.peer.Options
import arcships.rutis.internal.peer.Peer
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import kotlinx.serialization.json.JsonNull
import java.util.Collections
import java.util.concurrent.TimeUnit

/**
 * Tests a plugin without a host (design K16): it gives the plugin the
 * services it injects, calls the services it provides, emits and listens to
 * events as a host does, and unloads it.
 *
 * ```kotlin
 * RutisTest.load(weather, Config("Oslo"), services = mapOf("llm" to FakeLlm())).use { loaded ->
 *     assertEquals("sunny", loaded.service("weather").call("today"))
 * }
 * ```
 *
 * The plugin runs in its real runtime, across a real session (in memory):
 * values cross as they would between processes, so what works only in one
 * process fails here too. It checks what a host would: the plugin uses only
 * the services it injects. A service `provides` does not declare is used as
 * rutis uses it: its methods are its class's, all `async`.
 *
 * Events: the test side is the host of the bus. The plugin's listeners
 * register with it; [Loaded.emit], [Loaded.parallel], ... dispatch to them
 * from that row's scope, as a host plugin would. [Loaded.on] listens as the
 * host: the plugin's emissions of that name then go through the test side.
 * More rows of the plugin load into the same runtime with [Loaded.load], each
 * in a scope of its own.
 */
object RutisTest {
    /**
     * Loads `plugin` with `config` (a value of its config type, or null) and
     * the services in `services` (instances of `@RutisService` classes). The
     * first row is named `test`; [runtime] is the runtime's name in
     * descriptors. Close the result to unload it.
     */
    fun load(
        plugin: Plugin,
        config: Any? = null,
        services: Map<String, Any> = emptyMap(),
        row: String? = null,
        scope: List<Scope> = emptyList(),
        runtime: String = "kotlin",
        isolate: Map<String, String> = emptyMap(),
    ): Loaded {
        for (name in plugin.inject) {
            if (name !in services) throw IllegalArgumentException("RutisTest: the plugin injects $name: give the test a service $name")
        }
        val harness = Harness(plugin, services, runtime)
        harness.start()
        return harness.load(config, row, scope, isolate)
    }

    /** A scope chain of `path` (`t1/s1`): one instance per segment, its group the segment unless `groups` says. */
    fun scope(path: String, vararg groups: String): List<Scope> {
        if (path.isEmpty() || path == "root") return emptyList()
        val segments = path.split("/")
        return segments.indices.map { i ->
            Scope(segments.subList(0, i + 1).joinToString("/"), groups.getOrNull(i) ?: segments[i])
        }
    }
}

private const val WAIT_MILLIS = 30_000L

/** The runtime under test and the host side of its session. */
internal class Harness(val plugin: Plugin, private val services: Map<String, Any>, val runtime: String) {
    lateinit var session: Peer
    private lateinit var served: RuntimeSession
    val lock = Object()
    private val handles = HashMap<String, String>()
    private val announced = HashMap<String, Map<String, String>>()
    val rows = ArrayList<Loaded>()
    val scopes = HashMap<String, List<Scope>>()
    val entries = ArrayList<BusEntry>()
    private var first = 0L
    private var last = 0L
    private var dispatches = 0L
    val reported: MutableList<String> = Collections.synchronizedList(ArrayList())
    @Volatile
    var closed = false
    private val pushing = Object()
    private var declared: Map<String, Map<String, String>> = emptyMap()

    fun start() {
        Reporting.add(this)
        val (runtimeSide, hostSide) = MemoryChannel.pair()
        served = RuntimeSession.start(runtimeSide, null, listOf(plugin))
        session = Peer(
            hostSide,
            Options(
                host = true,
                dispatch = ::dispatch,
                arrive = ::arrive,
            ),
        )
        session.start()
        session.readyFuture.get(WAIT_MILLIS, TimeUnit.MILLISECONDS)
        call("", "events.elsewhere", listOf(linkedMapOf("add" to emptyList<String>())))
        call("", "mount", listOf(linkedMapOf("announce" to true, "runtime" to runtime)))
        @Suppress("UNCHECKED_CAST")
        val schema = Native.toNative(call("", "rows.schema", listOf(plugin.name))) as Map<String, Any?>
        @Suppress("UNCHECKED_CAST")
        declared = (schema["provides"] as? Map<String, Map<String, String>>) ?: emptyMap()
        for (name in plugin.inject) {
            val table = tableOf(name)
            call("", "hosts.provide", listOf(name, table.methods.associateWith { "sync" }))
        }
    }

    private fun tableOf(name: String): ServiceTable<Any> {
        val service = services[name] ?: throw IllegalStateException("RutisTest: no service $name")
        return Tables.serviceTable(service.javaClass)
            ?: throw IllegalArgumentException("RutisTest: the service $name (${service.javaClass.name}) is not a @RutisService class")
    }

    fun call(target: String, method: String, args: List<Any?>, sync: Boolean = false): Any? =
        runBlocking { withTimeout(WAIT_MILLIS + Shutdown.millis()) { session.callSuspend(target, method, args, sync) } }

    fun load(
        config: Any?,
        row: String?,
        scope: List<Scope>,
        isolate: Map<String, String> = emptyMap(),
    ): Loaded {
        val key: String
        val exports = LinkedHashMap<String, Any?>()
        synchronized(lock) {
            key = row ?: if (rows.isEmpty()) "test" else "test-${rows.size + 1}"
            if (key in scopes) throw IllegalStateException("RutisTest: a row $key is already loaded")
            // Every row gets the plugin's declared service slots, as a host
            // passes them for each `rows.load`.
            for ((name, kinds) in declared) exports[name] = LinkedHashMap<String, Any?>(kinds)
            scopes[key] = scope
        }
        val loaded = Loaded(this, key, scope)
        synchronized(lock) { rows.add(loaded) }
        val configValue = if (config == null) JsonNull else Native.fromNative(config)
        val placed = linkedMapOf("scope" to scope.map { linkedMapOf("id" to it.id, "group" to it.group) })
        val labels = isolate.map { (name, label) -> listOf(name, label) }
        try {
            call("", "rows.load", listOf(key, plugin.name, configValue, labels, emptyList<Any?>(), exports, placed))
        } catch (e: Throwable) {
            forgetRow(key)
            throw e
        }
        return loaded
    }

    /** Withdraws a row's listeners from the bus, as a host does when it unloads the row. */
    fun forgetRow(row: String) = synchronized(lock) {
        withdrawRow(row)
        scopes.remove(row)
        rows.removeAll { it.row == row }
    }

    /** Withdraws the runtime's listeners of `row`; lock is held. */
    fun withdrawRow(row: String) {
        entries.removeAll { entry ->
            (entry.fn == null && entry.row == row).also { if (it) entry.withdrawn = true }
        }
    }

    /** Puts an entry on the bus; lock is held. */
    fun add(entry: BusEntry, prepend: Boolean) {
        if (prepend) {
            first--
            entry.position = first
            entries.add(0, entry)
        } else {
            last++
            entry.position = last
            entries.add(entry)
        }
    }

    /** Takes the runtime's registrations in frame order (§4.1). */
    private fun arrive(method: String, args: List<Any?>): (suspend () -> Any?)? {
        val error: Throwable? = try {
            when (method) {
                "events.on" -> register(args)
                "events.off" -> {
                    val ids = (Native.toNative(args.firstOrNull()) as? List<*>)?.map { it as String } ?: emptyList()
                    synchronized(lock) {
                        entries.removeAll { entry -> (entry.fn == null && entry.id in ids).also { if (it) entry.withdrawn = true } }
                    }
                }
                else -> return null
            }
            null
        } catch (e: Throwable) {
            e
        }
        return { if (error != null) throw error else null }
    }

    private fun register(args: List<Any?>) {
        @Suppress("UNCHECKED_CAST")
        val registration = Native.toNative(args.firstOrNull()) as? Map<String, Any?> ?: throw IllegalArgumentException("events.on: not a record")
        val name = registration["name"] as String
        if (name.startsWith("internal/")) throw RutisException(RutisException.BINDING_ERROR, "event $name is internal to its runtime")
        val row = registration["row"] as? String ?: ""
        synchronized(lock) {
            if (row !in scopes) throw RutisException(RutisException.BINDING_ERROR, "row $row is not loaded in this runtime")
            add(
                BusEntry(this, registration["id"] as String, name, row, registration["plugin"] as? String ?: "", registration["global"] == true, null),
                registration["prepend"] == true,
            )
        }
    }

    private suspend fun dispatch(target: String, method: String, args: List<Any?>): Any? {
        if (target.isEmpty() && method == "service") {
            val id = Native.toNative(args.getOrNull(0)) as? String ?: ""
            val handle = Native.toNative(args.getOrNull(1)) as? String
            @Suppress("UNCHECKED_CAST")
            val announcement = Native.toNative(args.getOrNull(3)) as? Map<String, Any?>
            synchronized(lock) {
                if (handle == null) {
                    handles.remove(id)
                } else {
                    handles[id] = handle
                    @Suppress("UNCHECKED_CAST")
                    (announcement?.get("methods") as? Map<String, String>)?.let { announced[id] = it }
                }
                lock.notifyAll()
            }
            return null
        }
        if (target.isEmpty() && method == "events.dispatch") return dispatchFromRuntime(args)
        val name = target.removePrefix("host:")
        if (name == target) throw IllegalArgumentException("the test serves no $target $method")
        val service = services[name] ?: throw IllegalArgumentException("the test gives no service $name")
        val table = tableOf(name)
        if (method !in table.methods) throw IllegalArgumentException("the test's $name has no method $method")
        return table.invoke(service, method, args)
    }

    /** The service `name` the plugin provides: its handle and methods, once provided. */
    fun provided(name: String): Pair<String, Map<String, String>> {
        val deadline = System.currentTimeMillis() + WAIT_MILLIS
        synchronized(lock) {
            while (true) {
                val handle = handles[name]
                if (handle != null) return handle to (declared[name] ?: announced[name] ?: emptyMap())
                val left = deadline - System.currentTimeMillis()
                if (left <= 0) throw IllegalStateException("RutisTest: the plugin does not provide $name (now)")
                lock.wait(left)
            }
        }
    }

    fun providedNames(): List<String> = synchronized(lock) { handles.keys.toList() }

    /** A dispatch of `name` emitted in `scope` (§5.1). */
    fun snapshot(name: String, scope: List<Scope>, own: Boolean, keep: (suspend (Listener) -> Boolean)?): Dispatch = synchronized(lock) {
        dispatches++
        val id = "d$dispatches"
        val selected = entries.filter { it.name == name && within(if (it.fn != null) emptyList() else scopes[it.row] ?: emptyList(), scope) }
            .map { Invocation(it, id, own && it.fn == null) }
        Dispatch(name, selected, keep)
    }

    /** Serves the runtime's `events.dispatch` (§5.3). */
    private suspend fun dispatchFromRuntime(args: List<Any?>): Any? {
        @Suppress("UNCHECKED_CAST")
        val record = (args.firstOrNull() as? Map<String, Any?>) ?: (args.firstOrNull() as? kotlinx.serialization.json.JsonObject)
            ?: throw IllegalArgumentException("events.dispatch needs a record")
        val row = Native.toNative(record["row"]) as? String ?: ""
        val name = Native.toNative(record["name"]) as? String ?: ""
        val mode = (Native.toNative(record["mode"]) as? String)?.let(Mode::of)
        if (mode == null || mode == Mode.WaterfallSync) throw IllegalArgumentException("events.dispatch: unknown mode ${record["mode"]}")
        val values: List<Any?> = when (val list = record["args"]) {
            is List<*> -> list
            is kotlinx.serialization.json.JsonArray -> list
            else -> emptyList()
        }
        checkEmit(name)
        val scope = synchronized(lock) { scopes[row] } ?: throw RutisException(RutisException.BINDING_ERROR, "row $row is not loaded in this runtime")
        var d = snapshot(name, scope, true, null)
        val filter = record["filter"] as? RemoteFunction
        if (filter != null) {
            // One call of the emitter's selector, with every candidate the
            // filter applies to (§6).
            val infos = d.entries.map { it.info() }.filter { !it.global }
            val answers = try {
                val answer = session.callFunctionSuspend(filter, listOf(infos.map { EventsRuntime.data(it) }), mode == Mode.Bail)
                val list = Native.toNative(answer) as? List<*>
                if (list == null || list.size != infos.size || list.any { it !is Boolean }) {
                    throw RutisException(RutisException.BINDING_ERROR, "filter of $name returned ${list?.size ?: 0} answers for ${infos.size} listeners")
                }
                list.map { it as Boolean }
            } catch (e: Throwable) {
                if (mode == Mode.Emit) {
                    Report.line("rutis: filter of $name from row $row failed: ${Report.failure(e)}")
                    return null
                }
                throw e
            }
            var asked = 0
            d = Dispatch(name, d.entries) { answers[asked++] }
        }
        val terminal = record["terminal"] as? RemoteFunction
        val last: (suspend (List<Any?>) -> Any?)? = terminal?.let { t -> { raw -> session.settleSuspend(session.callFunctionSuspend(t, raw, false)) } }
        return run(d, mode, values, last)
    }

    suspend fun run(d: Dispatch, mode: Mode, args: List<Any?>, terminal: (suspend (List<Any?>) -> Any?)?): Any? = when (mode) {
        Mode.Emit -> {
            d.emit(args)
            null
        }
        Mode.Parallel -> {
            d.parallel(args)
            null
        }
        Mode.Waterfall, Mode.WaterfallSync -> d.waterfall(mode, args, terminal)
        else -> d.serial(mode, args)
    }

    /** Tells the runtime of a change of the names with listeners outside it, and waits until it applied it. */
    fun elsewhere(change: Map<String, Any?>) {
        if (closed) return
        call("", "events.elsewhere", listOf(change))
    }

    fun hasTestListener(name: String): Boolean = entries.any { it.fn != null && it.name == name }

    fun listen(name: String, prepend: Boolean, global: Boolean, fn: suspend (EventArgs) -> Any?): () -> Unit {
        checkListen(name)
        val entry = BusEntry(this, "", name, "", "rutistest", global, fn)
        synchronized(pushing) {
            val first = synchronized(lock) {
                val first = !hasTestListener(name)
                add(entry, prepend)
                first
            }
            if (first) elsewhere(linkedMapOf("add" to listOf(name)))
        }
        var done = false
        return {
            synchronized(pushing) {
                if (!done) {
                    done = true
                    val last = synchronized(lock) {
                        entry.withdrawn = true
                        entries.remove(entry)
                        !hasTestListener(name)
                    }
                    if (last) elsewhere(linkedMapOf("remove" to listOf(name)))
                }
            }
        }
    }

    fun close() {
        val all = synchronized(lock) { rows.toList() }
        for (row in all.asReversed()) row.unload()
        closed = true
        Reporting.remove(this)
        session.shutdown("the test ended")
        served.peer.closedFuture.get(WAIT_MILLIS, TimeUnit.MILLISECONDS)
        if (!served.end()) throw IllegalStateException("RutisTest: the runtime's cleanups did not finish")
    }
}

/** Collects the lines the runtime reports (§5.6) for every harness. */
private object Reporting {
    private val harnesses = Collections.synchronizedSet(HashSet<Harness>())

    @Volatile
    private var hooked = false

    fun add(harness: Harness) {
        synchronized(this) {
            if (!hooked) {
                hooked = true
                var previous: (String) -> Unit = {}
                previous = Report.replace { line ->
                    synchronized(harnesses) { for (h in harnesses) h.reported.add(line) }
                    previous(line)
                }
            }
        }
        harnesses.add(harness)
    }

    fun remove(harness: Harness) {
        harnesses.remove(harness)
    }
}

/** A listener on the test side's bus: one of the runtime's (by its id) or one of the test's (fn). */
internal class BusEntry(
    val h: Harness,
    val id: String,
    val name: String,
    val row: String,
    val plugin: String,
    val global: Boolean,
    val fn: (suspend (EventArgs) -> Any?)?,
) {
    var position = 0L

    @Volatile
    var withdrawn = false
    val queue = Queue("test-listener")

    fun info(): Listener = if (fn != null) {
        Listener(name = name, plugin = "rutistest", global = global)
    } else {
        // A copy: a filter changing it changes nothing else.
        val scope = synchronized(h.lock) { h.scopes[row]?.toList() ?: emptyList() }
        Listener(name = name, plugin = plugin, row = row, runtime = h.runtime, scope = scope, global = global)
    }
}

/** An entry within one dispatch. */
private class Invocation(val entry: BusEntry, val dispatch: String, val own: Boolean) : Entry {
    private val h = entry.h

    override fun info(): Listener = entry.info()

    override fun begin(): Call? {
        if (entry.withdrawn) return null
        return { mode, args, next -> call(mode, args, next) }
    }

    private suspend fun call(mode: Mode, args: List<Any?>, next: Next?): Any? {
        val fn = entry.fn ?: return h.session.settleSuspend(
            h.session.callSuspend("", "events.call", listOf(record(mode, args, next)), mode == Mode.Bail || mode == Mode.WaterfallSync),
        )
        return local(fn, mode, args, next)
    }

    /** Runs one of the test's listeners: session values in and out. */
    private suspend fun local(fn: suspend (EventArgs) -> Any?, mode: Mode, args: List<Any?>, next: Next?): Any? {
        val native: Next? = if (mode.waterfall && next != null) ({ Native.toNative(next()) }) else null
        return Native.fromNative(fn(EventArgs.of(args, native)))
    }

    /** An `events.call` (§5.3). */
    private fun record(mode: Mode, args: List<Any?>, next: Next?): Map<String, Any?> {
        val record = linkedMapOf<String, Any?>(
            "listener" to entry.id, "dispatch" to dispatch, "own" to own, "name" to entry.name, "mode" to mode.wire, "args" to args,
        )
        if (next != null) record["next"] = Exported(next) { next() }
        return record
    }

    override fun emit(args: List<Any?>): Boolean {
        if (entry.withdrawn) return false
        val fn = entry.fn
        if (fn != null) {
            val chain = CallChain.current()
            entry.queue.push {
                if (!entry.withdrawn) {
                    try {
                        runBlocking(CallChain.element(chain)) { local(fn, Mode.Emit, args, null) }
                    } catch (e: Throwable) {
                        Report.line("rutis: listener of ${entry.name} in row rutistest failed: ${Report.failure(e)}")
                    }
                }
            }
            return true
        }
        // Sent now, in order: the runtime starts them in arrival order.
        val report = { e: Throwable -> Report.line("rutis: listener of ${entry.name} in runtime ${h.runtime} failed: ${Report.failure(e)}") }
        val wait = try {
            h.session.send("", "events.call", listOf(record(Mode.Emit, args, null)), false)
        } catch (e: Throwable) {
            report(e)
            return true
        }
        Thread({
            try {
                runBlocking { wait() }
            } catch (e: Throwable) {
                report(e)
            }
        }, "rutistest-emit").apply { isDaemon = true }.start()
        return true
    }
}

/** A row of a plugin loaded for a test. Close it (or the first row) to unload. */
class Loaded internal constructor(private val h: Harness, val row: String, val scope: List<Scope>) : AutoCloseable, arcships.rutis.Emitter {
    @Volatile
    private var unloaded = false

    /** Loads another row of the same plugin into the same runtime, in `scope` and with `isolate` labels. */
    fun load(
        config: Any? = null,
        row: String? = null,
        scope: List<Scope> = emptyList(),
        isolate: Map<String, String> = emptyMap(),
    ): Loaded = h.load(config, row, scope, isolate)

    /** The services the plugin provides now. */
    val provided: List<String> get() = h.providedNames()

    /**
     * The service `name` the plugin provides, as rutis sees it: its declared
     * methods, or, undeclared, its class's, all `async`.
     */
    fun service(name: String): TestService {
        val (handle, kinds) = h.provided(name)
        return TestService(h.session, name, handle, kinds)
    }

    /**
     * Listens to `name` as the host does, until the returned function is
     * called or the test ends: the plugin's emissions of `name` reach
     * `listener` (through the test side, as through rutis), and so do the
     * test's own. The listener is at the root scope.
     */
    fun on(name: String, prepend: Boolean = false, global: Boolean = false, listener: suspend (EventArgs) -> Any?): () -> Unit =
        h.listen(name, prepend, global, listener)

    /** Dispatches only to the listeners `keep` accepts. */
    fun filter(keep: suspend (Listener) -> Boolean): arcships.rutis.Emitter = Filtered(keep)

    override fun emit(name: String, vararg args: Any?) {
        runBlocking { dispatch(name, Mode.Emit, args.toList(), null, null) }
    }

    override suspend fun parallel(name: String, vararg args: Any?) {
        dispatch(name, Mode.Parallel, args.toList(), null, null)
    }

    override suspend fun serial(name: String, vararg args: Any?): Any? = dispatch(name, Mode.Serial, args.toList(), null, null)

    override suspend fun waterfall(name: String, vararg args: Any?, terminal: (suspend (EventArgs) -> Any?)?): Any? =
        dispatch(name, Mode.Waterfall, args.toList(), terminal, null)

    override fun bail(name: String, vararg args: Any?): Any? = runBlocking { dispatch(name, Mode.Bail, args.toList(), null, null) }

    private inner class Filtered(private val keep: suspend (Listener) -> Boolean) : arcships.rutis.Emitter {
        override fun emit(name: String, vararg args: Any?) {
            runBlocking { dispatch(name, Mode.Emit, args.toList(), null, keep) }
        }

        override suspend fun parallel(name: String, vararg args: Any?) {
            dispatch(name, Mode.Parallel, args.toList(), null, keep)
        }

        override suspend fun serial(name: String, vararg args: Any?): Any? = dispatch(name, Mode.Serial, args.toList(), null, keep)

        override suspend fun waterfall(name: String, vararg args: Any?, terminal: (suspend (EventArgs) -> Any?)?): Any? =
            dispatch(name, Mode.Waterfall, args.toList(), terminal, keep)

        override fun bail(name: String, vararg args: Any?): Any? = runBlocking { dispatch(name, Mode.Bail, args.toList(), null, keep) }
    }

    private suspend fun dispatch(name: String, mode: Mode, args: List<Any?>, terminal: (suspend (EventArgs) -> Any?)?, keep: (suspend (Listener) -> Boolean)?): Any? {
        checkEmit(name)
        val values = args.map(Native::fromNative)
        val d = h.snapshot(name, scope, false, keep)
        val last: (suspend (List<Any?>) -> Any?)? = terminal?.let { t -> { raw -> Native.fromNative(t(EventArgs.of(raw, null))) } }
        return Native.toNative(h.run(d, mode, values, last))
    }

    /** What the runtime reported so far (§5.6), one line per failure nobody waited for. */
    fun reported(): List<String> = synchronized(h.reported) { h.reported.toList() }

    /**
     * Unloads the row: the runtime cancels its running listeners and waits
     * for them (up to RUTIS_SHUTDOWN_TIMEOUT), then runs its cleanups.
     */
    fun unload() {
        if (unloaded) return
        unloaded = true
        // The host withdraws the row's listeners as it unloads it (§7.1).
        h.forgetRow(row)
        if (h.closed) return
        h.call("", "rows.unload", listOf(row))
    }

    /**
     * Changes the row's config, as a host's rows.update: the runtime unloads
     * the row and loads it again, so its listeners register again.
     */
    fun update(config: Any?) {
        synchronized(h.lock) {
            if (row !in h.scopes || h.closed) throw IllegalStateException("RutisTest: row $row is not loaded")
            h.withdrawRow(row)
        }
        h.call("", "rows.update", listOf(row, if (config == null) JsonNull else Native.fromNative(config)))
    }

    /** Unloads every row, latest first, and ends the runtime (the first row's close), or this row. */
    override fun close() {
        if (h.rows.firstOrNull() === this || h.rows.isEmpty()) h.close() else unload()
    }
}

/** A provided service, called across the session. */
class TestService internal constructor(
    private val session: Peer,
    val name: String,
    private val handle: String,
    /** The service's methods, each `sync` or `async`. */
    val methods: Map<String, String>,
) {
    /** Calls `method` with native values; the result as a native value. */
    suspend fun call(method: String, vararg args: Any?): Any? = Native.toNative(raw(method, args.map(Native::fromNative)))

    /** Calls `method` with session values; the result as a session value (for the conformance checks). */
    suspend fun raw(method: String, args: List<Any?>): Any? {
        val kind = methods[method] ?: throw IllegalArgumentException("$name has no method $method")
        return session.settleSuspend(session.callSuspend(handle, method, args, kind == "sync"))
    }

    /** The service through the `@RutisClient` interface `I`. */
    inline fun <reified I : Any> bind(): I = bind(I::class.java)

    fun <I : Any> bind(type: Class<I>): I {
        val factory: ClientFactory<I> = Tables.clientFactory(type) ?: throw IllegalArgumentException("${type.name} has no proxy: annotate it @RutisClient")
        val service = this
        return factory.create(object : ServiceHandle {
            override val name: String = service.name
            override val kinds: Map<String, String> = methods
            override suspend fun callSuspend(method: String, args: List<Any?>): Any? = raw(method, args)
            override fun callBlocking(method: String, args: List<Any?>): Any? = runBlocking { raw(method, args) }
        })
    }
}
