package arcships.rutis

import arcships.rutis.internal.EventsRuntime
import arcships.rutis.internal.Mode
import arcships.rutis.internal.Report
import arcships.rutis.internal.Row
import arcships.rutis.internal.RuntimeState
import arcships.rutis.internal.peer.CallChain
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.runBlocking
import kotlin.coroutines.CoroutineContext

/**
 * What `apply` receives: the row's services, events and lifetime. It is the
 * row's coroutine scope, cancelled when the row unloads (`isActive`,
 * `ensureActive()`); coroutines started in it ([launch]) keep the call
 * chain and are waited for when the row unloads.
 */
class Ctx internal constructor(private val rt: RuntimeState, internal val row: Row) : CoroutineScope, Emitter {
    override val coroutineContext: CoroutineContext = row.job + Dispatchers.Default + CallChain.element(row.chain)

    /** This row's scope chain, outermost first; empty at the root. */
    val scope: List<Scope> get() = row.scope.toList()

    /** The service `name` (which `inject` lists) through the `@RutisClient` interface `I`. */
    inline fun <reified I : Any> use(name: String): I = use(name, I::class.java)

    /**
     * The service `name` through the `@RutisClient` interface `type`. A
     * provider in this process whose object implements `type` gives the
     * object itself; any other gives a proxy (calls cross as values do, by the
     * service's own id of this row). Every method of `type` must be one the
     * service has.
     */
    fun <I : Any> use(name: String, type: Class<I>): I {
        if (name !in row.plugin.inject) throw IllegalArgumentException("service $name is not available to this plugin, which does not declare it in inject: add '$name' to its inject")
        val factory = Tables.clientFactory(type)
            ?: throw IllegalArgumentException("${type.name} has no proxy: annotate it @RutisClient and apply the rutis-ksp processor")
        val id = row.id(name)
        val (local, host) = synchronized(rt.lock) { rt.services[id] to rt.hosts[id] }
        val handle: ServiceHandle = when {
            local != null -> {
                if (type.isInstance(local.value)) return type.cast(local.value)
                LocalHandle(name, local.value, local.table, local.shape?.kinds ?: local.table.methods.associateWith { "async" })
            }
            host != null -> HostHandle(rt, name, "host:$id", host)
            else -> throw IllegalStateException("service $name is not available")
        }
        val missing = factory.methods.filter { it !in handle.kinds }
        if (missing.isNotEmpty()) throw IllegalArgumentException("use $name: ${type.simpleName} calls ${missing.joinToString()}, which the service does not have")
        return factory.create(handle)
    }

    /**
     * Provides `service`, an instance of a `@RutisService` class, as `name`
     * until the row unloads or the returned function is called. A service
     * `provides` does not declare reaches rutis by itself, its methods all
     * `async`.
     */
    fun provide(name: String, service: Any): () -> Unit = rt.provide(row, name, service)

    /** Registers a cleanup, run when the row unloads (latest first). */
    fun effect(cleanup: suspend () -> Unit) {
        if (!rt.addCleanup(row, cleanup)) {
            Report.line("rutis: row ${row.key} is not active: cleanup is not registered")
        }
    }

    /**
     * Listens to the event `name` on the bus until the row unloads or the
     * returned function is called. The listener gets the event's arguments,
     * and in a waterfall `next` last ([EventArgs.next]); its result counts in
     * serial, bail and waterfall (null and false are no result). [prepend]
     * puts it before the listeners registered so far; [global] exempts it from
     * emitters' filters. `rutis/` events can be listened to.
     */
    fun on(name: String, prepend: Boolean = false, global: Boolean = false, listener: suspend (EventArgs) -> Any?): () -> Unit =
        EventsRuntime.on(rt, row, name, prepend, global, listener)

    /** An emitter whose dispatches reach only the listeners `keep` accepts (global listeners always). */
    fun filter(keep: suspend (Listener) -> Boolean): Emitter = Filtered(keep)

    override fun emit(name: String, vararg args: Any?) {
        dispatchNow(name, Mode.Emit, args.toList(), null)
    }

    override suspend fun parallel(name: String, vararg args: Any?) {
        EventsRuntime.dispatch(rt, row, name, Mode.Parallel, args.toList(), null, null)
    }

    override suspend fun serial(name: String, vararg args: Any?): Any? =
        EventsRuntime.dispatch(rt, row, name, Mode.Serial, args.toList(), null, null)

    override suspend fun waterfall(name: String, vararg args: Any?, terminal: (suspend (EventArgs) -> Any?)?): Any? =
        EventsRuntime.dispatch(rt, row, name, Mode.Waterfall, args.toList(), terminal, null)

    override fun bail(name: String, vararg args: Any?): Any? = dispatchNow(name, Mode.Bail, args.toList(), null)

    /** A dispatch that does not suspend (emit, bail): on this thread, with its chain. */
    private fun dispatchNow(name: String, mode: Mode, args: List<Any?>, keep: (suspend (Listener) -> Boolean)?): Any? =
        runBlocking(CallChain.element()) { EventsRuntime.dispatch(rt, row, name, mode, args, null, keep) }

    private inner class Filtered(private val keep: suspend (Listener) -> Boolean) : Emitter {
        override fun emit(name: String, vararg args: Any?) {
            dispatchNow(name, Mode.Emit, args.toList(), keep)
        }

        override suspend fun parallel(name: String, vararg args: Any?) {
            EventsRuntime.dispatch(rt, row, name, Mode.Parallel, args.toList(), null, keep)
        }

        override suspend fun serial(name: String, vararg args: Any?): Any? =
            EventsRuntime.dispatch(rt, row, name, Mode.Serial, args.toList(), null, keep)

        override suspend fun waterfall(name: String, vararg args: Any?, terminal: (suspend (EventArgs) -> Any?)?): Any? =
            EventsRuntime.dispatch(rt, row, name, Mode.Waterfall, args.toList(), terminal, keep)

        override fun bail(name: String, vararg args: Any?): Any? = dispatchNow(name, Mode.Bail, args.toList(), keep)
    }
}

/** A service provided in this process, used through its method table (values cross as copies). */
internal class LocalHandle(
    override val name: String,
    private val target: Any,
    private val table: ServiceTable<Any>,
    override val kinds: Map<String, String>,
) : ServiceHandle {
    override suspend fun callSuspend(method: String, args: List<Any?>): Any? = table.invoke(target, method, args)

    override fun callBlocking(method: String, args: List<Any?>): Any? =
        runBlocking(CallChain.element()) { table.invoke(target, method, args) }
}

/** A rutis service, called through the session (`host:<id>`). */
internal class HostHandle(
    private val rt: RuntimeState,
    override val name: String,
    private val target: String,
    override val kinds: Map<String, String>,
) : ServiceHandle {
    private fun session() = rt.session ?: throw IllegalStateException("the runtime has no session")

    override suspend fun callSuspend(method: String, args: List<Any?>): Any? =
        session().callSuspend(target, method, args, kinds[method] == "sync")

    override fun callBlocking(method: String, args: List<Any?>): Any? =
        session().call(target, method, args, kinds[method] == "sync")
}
