package arcships.rutis.internal

import arcships.rutis.EventArgs
import arcships.rutis.advanced.Listener
import arcships.rutis.advanced.Next
import arcships.rutis.advanced.RemoteFunction
import arcships.rutis.RutisException
import arcships.rutis.advanced.Scope
import arcships.rutis.internal.peer.CallChain
import arcships.rutis.internal.peer.Exported
import arcships.rutis.internal.peer.RemoteFuture
import arcships.rutis.internal.peer.Undefined
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Job
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.future.await
import kotlinx.coroutines.job
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withContext
import java.util.concurrent.CompletableFuture

// Events (#464, docs/design-events-2026-10-18.md): every listener on a host
// is on one logical bus, which the host dispatches. A listener this process
// registers is registered with the host, which invokes it (`events.call`);
// every emission goes to the host (`events.dispatch`), its listeners here
// included (docs/design-platform-cleanup-2026-10-28.md §7.5).

/** A listener of this process: the host invokes it (`events.call`). */
internal class LocalListener(
    val rt: RuntimeState,
    val id: String,
    val name: String,
    val row: Row,
    val fn: suspend (EventArgs) -> Any?,
) {
    /** Withdrawn (rt.lock): no invocation starts any more. */
    var withdrawn = false
    val queue = Queue("listener-$id")

    /** Whether invocations may start; rt.lock is held. */
    fun live(): Boolean = !withdrawn && rt.rows[row.key] === row

    /** Counts an invocation in the row's listeners, which unload waits for, unless withdrawn. */
    fun reserve(): Boolean = synchronized(rt.lock) {
        if (!live()) return false
        row.listening.add()
        true
    }

    /** Reserves an invocation, unless the listener was withdrawn. */
    fun begin(): (suspend (Mode, List<Any?>, Next?) -> Any?)? {
        if (!reserve()) return null
        return { mode, args, next ->
            try {
                run(mode, args, next)
            } finally {
                row.listening.done()
            }
        }
    }

    /**
     * Calls the listener with session values, returning a session value:
     * cancelled when the caller gives up or the row unloads. `next` returns a
     * session value; the listener's `next` a native one.
     */
    suspend fun run(mode: Mode, args: List<Any?>, next: Next?): Any? {
        val caller = currentCoroutineContext().job
        val job = Job(caller)
        val stop = row.job.invokeOnCompletion { job.cancel() }
        if (row.job.isCancelled) job.cancel()
        val native: Next? = if (mode.waterfall && next != null) ({ Native.toNative(next()) }) else null
        try {
            return withContext(job) { Native.fromNative(fn(EventArgs.of(args, native))) }
        } catch (e: CancellationException) {
            // Cancelled because its row unloads while the dispatch goes on:
            // the listener has gone, with no result (a cancelled coroutine
            // cannot return one; Go and Python listeners return after their
            // cancellation, §7.1).
            if (row.job.isCancelled && caller.isActive) return null
            throw e
        } finally {
            stop.dispose()
            job.complete()
        }
    }
}

internal object EventsRuntime {
    /** Registers a listener of `row` (Ctx.on). */
    fun on(rt: RuntimeState, row: Row, name: String, prepend: Boolean, global: Boolean, fn: suspend (EventArgs) -> Any?): () -> Unit {
        checkListen(name)
        val listener: LocalListener
        synchronized(rt.lock) {
            if (rt.rows[row.key] !== row) throw IllegalStateException("rutis: row ${row.name} is not active")
            rt.listenerSeq++
            listener = LocalListener(rt, rt.listenerSeq.toString(), name, row, fn)
            rt.byId[listener.id] = listener
            // The host hears of it before any emission this process makes after
            // it (sent in order); a row loading answers once the host has it,
            // and fails when the host refuses it (§4.4). Refused outside a
            // load, it is reported (§11).
            val session = rt.session
            if (session != null && !rt.closing) {
                val loading = row.loading
                val registration = listOf(
                    linkedMapOf<String, Any?>(
                        "id" to listener.id, "name" to name, "row" to row.key, "plugin" to row.plugin.name,
                        "kind" to "sync", "prepend" to prepend, "global" to global,
                    ),
                )
                val refused = CompletableFuture<Throwable?>()
                val answered = session.announce("", "events.on", registration) { error ->
                    if (loading) {
                        refused.complete(error)
                    } else {
                        Report.line("rutis: registration of listener of $name in row ${row.name} failed: ${Report.failure(error)}")
                    }
                }
                if (loading) row.pending?.add(answered.thenApply { refused.getNow(null) })
            }
        }
        return {
            synchronized(rt.lock) {
                if (!listener.withdrawn) {
                    withdraw(rt, listener)
                    val session = rt.session
                    if (session != null && !rt.closing) session.notify("", "events.off", listOf(listOf(listener.id)))
                }
            }
        }
    }

    /** Takes a listener off the table; rt.lock is held. */
    fun withdraw(rt: RuntimeState, listener: LocalListener) {
        listener.withdrawn = true
        rt.byId.remove(listener.id)
    }

    /**
     * Withdraws a row's listeners before it unloads; rt.lock is held. A
     * dispatch in progress keeps its snapshot but starts none of them. The
     * host, which withdraws them itself on rows.unload, hears of it for the
     * other unloads (a failed load, an update).
     */
    fun forgetListeners(rt: RuntimeState, row: Row) {
        val ids = rt.byId.values.filter { it.row === row }
        for (listener in ids) withdraw(rt, listener)
        val session = rt.session
        if (ids.isNotEmpty() && session != null && !rt.closing) session.notify("", "events.off", listOf(ids.map { it.id }))
    }

    /** Dispatches an emission of `row` through the host; `terminal` only in a waterfall. */
    suspend fun dispatch(
        rt: RuntimeState,
        row: Row,
        name: String,
        mode: Mode,
        args: List<Any?>,
        terminal: (suspend (EventArgs) -> Any?)?,
        keep: (suspend (Listener) -> Boolean)?,
    ): Any? {
        checkEmit(name)
        val values = args.map(Native::fromNative)
        val wait = synchronized(rt.lock) {
            if (rt.rows[row.key] !== row) throw RutisException(RutisException.BINDING_ERROR, "row ${row.name} is not loaded in this runtime")
            sendViaHost(rt, row, name, mode, values, terminal, keep)
        }
        return wait()
    }

    /**
     * Sends `events.dispatch` now (rt.lock is held, so it goes out behind
     * this process's earlier frames, such as the events.on of a listener
     * registered before, §4.4), and returns the wait for its result.
     */
    private fun sendViaHost(
        rt: RuntimeState,
        row: Row,
        name: String,
        mode: Mode,
        values: List<Any?>,
        terminal: (suspend (EventArgs) -> Any?)?,
        keep: (suspend (Listener) -> Boolean)?,
    ): suspend () -> Any? {
        val session = rt.session?.takeIf { !rt.closing && it.error == null }
            ?: throw IllegalStateException("rutis: emit of $name from row ${row.name}: the session with rutis has ended")
        val record = linkedMapOf<String, Any?>("row" to row.key, "name" to name, "mode" to mode.wire, "args" to values)
        if (keep != null) record["filter"] = selector(keep)
        if (terminal != null && mode == Mode.Waterfall) {
            // The host calls it with the event's arguments.
            record["terminal"] = Exported(terminal) { raw -> Native.fromNative(terminal(EventArgs.of(raw, null))) }
        }
        if (mode == Mode.Emit) {
            // The emitter does not wait, nor cancel it.
            val answered = { error: Throwable? ->
                if (error != null) Report.line("rutis: emit of $name from row ${row.name} failed: ${Report.failure(error)}")
            }
            // The host queues an emit while one of the same row runs, and
            // answers with a future: its settlement.
            session.sendAnswered("", "events.dispatch", listOf(record)) { value, error ->
                if (error == null && value is RemoteFuture) {
                    Thread({
                        answered(runCatching { session.settle(value) }.exceptionOrNull())
                    }, "rutis-emit").apply { isDaemon = true }.start()
                } else {
                    answered(error)
                }
            }
            return { null }
        }
        if (mode == Mode.Bail && onMainThread()) {
            // A bail blocks its thread until the host answers (design
            // §11.3 #3): not the main thread, whose freezing it would be, and
            // which the answer may need.
            throw IllegalStateException(
                "bail of $name waits for rutis: not on the main thread; call it from another thread, or use serial, a suspend function",
            )
        }
        val wait = session.send("", "events.dispatch", listOf(record), mode == Mode.Bail)
        return {
            val value = wait()
            if (mode == Mode.Parallel) null else Native.toNative(value)
        }
    }

    /** The function the host calls with the descriptors of a dispatch's candidates, answering one boolean each (§6). */
    private fun selector(keep: suspend (Listener) -> Boolean): Exported = Exported(keep) { args ->
        val infos = Native.toNative(if (args.isNotEmpty()) args[0] else Undefined) as? List<*> ?: emptyList<Any?>()
        infos.map { keep(descriptor(it)) }
    }

    /** A descriptor received as data. */
    fun descriptor(value: Any?): Listener {
        @Suppress("UNCHECKED_CAST")
        val d = value as? Map<String, Any?> ?: throw IllegalArgumentException("filter: a descriptor is not an object")
        return Listener(
            name = d["name"] as? String ?: "",
            prefix = d["prefix"] == true,
            plugin = d["plugin"] as? String ?: "",
            row = d["row"] as? String,
            runtime = d["runtime"] as? String,
            node = d["node"] as? String,
            scope = (d["scope"] as? List<*> ?: emptyList<Any?>()).map {
                @Suppress("UNCHECKED_CAST")
                val s = it as? Map<String, Any?> ?: emptyMap()
                Scope(s["id"] as? String ?: "", s["group"] as? String ?: "")
            },
            global = d["global"] == true,
        )
    }

    /** A descriptor as data, as the protocol names its fields (§4.2). */
    fun data(listener: Listener): Map<String, Any?> = linkedMapOf(
        "name" to listener.name,
        "prefix" to listener.prefix,
        "plugin" to listener.plugin,
        "row" to listener.row,
        "runtime" to listener.runtime,
        "node" to listener.node,
        "scope" to listener.scope.map { linkedMapOf("id" to it.id, "group" to it.group) },
        "global" to listener.global,
    )

    // ── From the host ────────────────────────────────────────────

    /** Takes the host's event calls on the reading thread, in frame order (§4.1, §5.5). */
    fun arrive(rt: RuntimeState, method: String, args: List<Any?>): (suspend () -> Any?)? = when (method) {
        "events.call" -> call(rt, args)
        else -> null
    }

    /** A record received as a call argument: its fields as session values. */
    @Suppress("UNCHECKED_CAST")
    private fun fields(value: Any?): Map<String, Any?>? = when (value) {
        is kotlinx.serialization.json.JsonObject -> value
        is Map<*, *> -> value as Map<String, Any?>
        else -> null
    }

    /** Runs one invocation of a listener here (§5.3): an `emit` one is queued now, after the earlier ones of that listener (§5.5). */
    private fun call(rt: RuntimeState, args: List<Any?>): suspend () -> Any? {
        val record = fields(if (args.isNotEmpty()) args[0] else null) ?: return { throw IllegalArgumentException("events.call: not a record") }
        val id = Native.toNative(record["listener"]) as? String
        val name = Native.toNative(record["name"]) as? String ?: ""
        val mode = (Native.toNative(record["mode"]) as? String)?.let(Mode::of)
            ?: return { throw IllegalArgumentException("events.call: unknown mode ${record["mode"]}") }
        val values: List<Any?> = when (val list = record["args"]) {
            is List<*> -> list
            is kotlinx.serialization.json.JsonArray -> list
            null, Undefined -> emptyList()
            else -> return { throw IllegalArgumentException("events.call args: not a list") }
        }
        val withdrawn = RutisException(RutisException.BINDING_ERROR, "listener $id of $name is withdrawn")
        val listener = synchronized(rt.lock) { rt.byId[id] } ?: return { throw withdrawn }
        if (mode == Mode.Emit) {
            if (!listener.reserve()) return { throw withdrawn }
            val done = CompletableFuture<Throwable?>()
            val chain = CallChain.current()
            listener.queue.push {
                try {
                    if (!synchronized(rt.lock) { listener.live() }) {
                        done.complete(withdrawn)
                    } else {
                        val failure = try {
                            runBlocking(CallChain.element(chain)) { listener.run(mode, values, null) }
                            null
                        } catch (e: Throwable) {
                            // Cancelled because its row unloads (§7.1): not a failure.
                            if (e is CancellationException && listener.row.job.isCancelled) null else e
                        }
                        done.complete(failure)
                    }
                } finally {
                    listener.row.listening.done()
                }
            }
            // Answered once it ran, with its failure, which rutis reports (§5.6).
            return {
                done.await()?.let { throw it }
                null
            }
        }
        var next: Next? = null
        if (mode.waterfall) {
            val remote = record["next"] as? RemoteFunction ?: return { throw IllegalArgumentException("events.call: a ${mode.wire} needs next") }
            next = { remote.peer.callFunctionSuspend(remote, emptyList(), false) }
        }
        val call = listener.begin() ?: return { throw withdrawn }
        return { call(mode, values, next) }
    }
}
