package arcships.rutis.internal.peer

import arcships.rutis.HostGoneException
import arcships.rutis.Plugin
import arcships.rutis.advanced.RemoteFunction
import arcships.rutis.advanced.RemoteObject
import arcships.rutis.RutisException
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Job
import kotlinx.coroutines.future.await
import kotlinx.coroutines.runBlocking
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import java.io.IOException
import java.lang.ref.ReferenceQueue
import java.lang.ref.WeakReference
import java.math.BigInteger
import java.util.IdentityHashMap
import java.util.concurrent.CompletableFuture
import java.util.concurrent.ExecutionException
import java.util.concurrent.LinkedBlockingQueue
import java.util.concurrent.TimeUnit
import java.util.concurrent.TimeoutException
import kotlin.coroutines.intrinsics.suspendCoroutineUninterceptedOrReturn
import kotlin.coroutines.jvm.internal.CoroutineStackFrame

// One session of the rutis protocol: the compat format (version 2) local
// channels speak, and the endpoint format (version 3) network channels speak.
//
// One thread reads frames and one writes them; every incoming call runs on a
// thread of its own (design K5), so a synchronous call blocks only the thread
// that made it and any incoming call can run at any time: the session is
// reentrant. The call chain (`path`) of a call is in CallChain: set on the
// call's thread, carried into coroutines, taken by outgoing calls.

const val PROTOCOL: Int = 2
const val ENDPOINT_PROTOCOL: Int = 3

/** How deep a value crossing a session may nest (#279): each list, record, map or set is a level. */
const val MAX_DEPTH: Int = 64

/** The event protocol of rutis 0.13 (#464), which both ends of every 0.13 session declare. */
const val EVENTS_CAPABILITY: String = "events-v2"

/**
 * What this side declares in either format: `signals`; `sync-wait` (its
 * synchronous calls say so); `reentrant-sync` (a thread per incoming call);
 * not `sync-stack`; `events-v2`; every `value-<tag>`.
 */
val SESSION_CAPABILITIES: List<String> = listOf("objects", "signals", "reentrant-sync", "sync-wait", EVENTS_CAPABILITY) + VALUE_CAPABILITIES

private val endpointId = Regex("^[a-z0-9-]+$")
private val compatOrigin = Regex("^(node|rust|s[0-9]+/[a-z0-9-]+):[1-9][0-9]*$")
private val endpointOrigin = Regex("^(s[0-9]+/)?[a-z0-9-]+:[1-9][0-9]*$")
private val sequence = Regex("^[1-9][0-9]*$")

/** Whether `id` is a valid endpoint id (`[a-z0-9-]+`). */
fun validId(id: String): Boolean = endpointId.matches(id)

/** The error of a call whose value nests deeper than [MAX_DEPTH]. */
fun tooDeep(): ValueException =
    ValueException("value nested deeper than $MAX_DEPTH levels, the most a value crossing processes may nest")

/** The far end greeted in a way this side cannot speak: retrying cannot fix it. */
open class IncompatibleException(message: String) : IOException(message)

/**
 * The far end greeted as another endpoint than the expected one, or as this
 * one: who it is was not accepted (a dialer waits and retries, as for
 * refused credentials).
 */
class EndpointMismatchException(message: String) : IncompatibleException(message)

/** The far end lacks a capability [Options.require] lists; the message is the reason alone. */
class RefusedException(val reason: String) : IncompatibleException(reason)

/** A frame not in the protocol's shape: the session ends. */
class ProtocolException(message: String) : IOException(message)

/** The endpoint format: this side's id, the far end's expected id (if known), capabilities to declare besides. */
class Endpoint(val local: String, val expected: String? = null, val declare: List<String> = emptyList())

class Options(
    /** Serves `invoke`: target "" is a control operation. Runs on the call's own thread. */
    val dispatch: (suspend (target: String, method: String, args: List<Any?>) -> Any?)? = null,
    /** Selects the endpoint format; null speaks compat. */
    val endpoint: Endpoint? = null,
    /** Sent in the endpoint handshake. */
    val implementation: Map<String, String> = emptyMap(),
    /** Declared in the endpoint handshake. */
    val capabilities: List<String> = SESSION_CAPABILITIES,
    /** This side is the host of a compat session (call ids `rust:`), for tests that drive a runtime. */
    val host: Boolean = false,
    /** Capabilities the far end must declare: the session ends right after its handshake when it does not. */
    val require: List<String> = emptyList(),
    val refuse: ((String) -> String)? = null,
    /**
     * Sees each incoming control call (`invoke` with an empty target) on the
     * reading thread, before the next frame is read. It returns the rest of
     * the call, run on a thread of its own, or null to have [dispatch] serve
     * the call.
     */
    val arrive: ((method: String, args: List<Any?>) -> (suspend () -> Any?)?)? = null,
    /**
     * A test host (RutisTest) in the runtime's process: the plugin definition
     * the runtime exported as `id`, so that a test double gets the definition
     * itself, as the runtime would take it back.
     */
    val localPlugin: ((id: Long) -> Plugin?)? = null,
)

/** What the far end said of itself (endpoint format). */
class Greeting(val endpoint: String, val implementation: JsonElement?, val capabilities: List<String>)

internal class Outcome(val value: Any?, val error: Throwable?, val ended: Boolean = false)

private class Export(val value: Any, val kind: String, val origin: List<String>) {
    var grants = 0L
}

/** A reference granted by the far end. */
class Imported internal constructor(val id: Long, val kind: String, val origin: List<String>) {
    internal var grants = 1L
    internal var released = false
    internal var function: WeakReference<RemoteFunction>? = null
    internal var future: WeakReference<RemoteFuture>? = null
    internal var plugin: WeakReference<RemotePlugin>? = null
    internal var obj: WeakReference<RemoteObject>? = null
}

/**
 * A plugin definition of the far end, held by a test host (RutisTest): it
 * can only be handed back. [local] is the definition itself when the runtime
 * is in this process.
 */
class RemotePlugin internal constructor(internal val peer: Peer, internal val record: Imported, val local: Plugin?)

/** An asynchronous result of the far end. */
class RemoteFuture internal constructor(internal val peer: Peer, internal val record: Imported) {
    @Volatile
    internal var settled: Outcome? = null
}

/** The state of decoding one frame's value. */
private class Decoding(val call: String?) {
    val fresh = ArrayList<Imported>()
    var refused: Throwable? = null

    /** The value is a call's argument list, whose items keep undefined (#687 §七); only the outermost list. */
    var arguments = false

    fun refuse(error: Throwable) {
        if (refused == null) refused = error
    }
}

/** An incoming call running on its own thread. */
private class Running(val id: String, val job: Job) {
    @Volatile
    var thread: Thread? = null
}

/** Releases imported references whose proxies were collected. */
private object Collector {
    class Ref(referent: Any, val peer: Peer, val record: Imported, queue: ReferenceQueue<Any>) : WeakReference<Any>(referent, queue)

    val queue = ReferenceQueue<Any>()
    private val live = java.util.Collections.newSetFromMap(java.util.concurrent.ConcurrentHashMap<Ref, Boolean>())

    init {
        Thread({
            while (true) {
                val ref = try {
                    queue.remove() as Ref
                } catch (_: InterruptedException) {
                    continue
                }
                live.remove(ref)
                ref.peer.release(ref.record)
            }
        }, "rutis-release").apply { isDaemon = true }.start()
    }

    fun watch(proxy: Any, peer: Peer, record: Imported) {
        live.add(Ref(proxy, peer, record, queue))
    }
}

private val frameJson = Json { encodeDefaults = true }

private fun compact(frame: JsonElement): ByteArray =
    frameJson.encodeToString(JsonElement.serializer(), frame).toByteArray(Charsets.UTF_8)

class Peer(private val channel: Channel, private val opts: Options) {
    private val lock = Object()
    private val local: String
    private var remote: String

    private var next = 0L
    private var received = 0L
    private var ref = 0L
    private var handshake = false
    private var greeting: Greeting? = null
    private var declared: List<String> = emptyList()

    private val pending = HashMap<String, CompletableFuture<Outcome>>()
    private val exports = HashMap<Long, Export>()
    private val identities = IdentityHashMap<Any, Long>()
    private val imports = HashMap<Long, Imported>()

    /**
     * A test host's plugin definitions of the far end, by the definition
     * itself, held until the session ends (or the reference is released):
     * one handed back goes home as the original reference, as rutis sends
     * it. So the test host never lets one go before the session ends, and a
     * test does not see rutis dropping a definition and the runtime
     * releasing it with it.
     */
    private val heldPlugins = java.util.IdentityHashMap<Plugin, RemotePlugin>()
    private val running = HashMap<String, Running>()
    private val signals = HashMap<String, Signal>()

    private var active = 0
    private val draining = ArrayList<CompletableFuture<Unit>>()

    private val outbox = LinkedBlockingQueue<ByteArray>()
    private val ready = CompletableFuture<Unit>()
    private val closed = CompletableFuture<Unit>()

    @Volatile
    private var closeError: Throwable? = null

    init {
        val endpoint = opts.endpoint
        when {
            endpoint != null -> {
                require(endpointId.matches(endpoint.local)) { "invalid endpoint id ${endpoint.local}" }
                local = endpoint.local + ":"
                remote = ""
            }
            opts.host -> {
                local = "rust:"
                remote = "node:"
            }
            else -> {
                local = "node:"
                remote = "rust:"
            }
        }
    }

    /** Sends the handshake and starts reading and writing. */
    fun start() {
        Thread(::read, "rutis-reader").apply { isDaemon = true }.start()
        Thread(::write, "rutis-writer").apply { isDaemon = true }.start()
        val hello = buildJsonObject {
            put("op", "hello")
            val endpoint = opts.endpoint
            if (endpoint == null) {
                put("version", PROTOCOL)
                // A test host takes plugin definitions, as rutis does.
                put("capabilities", JsonArray((SESSION_CAPABILITIES + if (opts.host) listOf("plugins") else emptyList()).map(::JsonPrimitive)))
            } else {
                put("version", ENDPOINT_PROTOCOL)
                put("endpoint", endpoint.local)
                put("implementation", JsonObject(opts.implementation.mapValues { JsonPrimitive(it.value) }))
                put("capabilities", JsonArray((opts.capabilities + endpoint.declare).map(::JsonPrimitive)))
            }
        }
        outbox.offer(compact(hello))
    }

    /** Completes once the far end greeted (or the session closed). */
    val readyFuture: CompletableFuture<Unit> get() = ready

    /** Completes once the session ended; [error] says why. */
    val closedFuture: CompletableFuture<Unit> get() = closed

    val error: Throwable? get() = closeError

    /** Whether the far end declared `capability`. */
    fun supports(capability: String): Boolean = synchronized(lock) { capability in declared }

    /** The plugin definition exported as `id`, while the far end holds it. */
    fun exportedPlugin(id: Long): Plugin? = synchronized(lock) { exports[id]?.value as? Plugin }

    /** What the far end said of itself, once it greeted (endpoint format). */
    fun greeting(): Greeting? = synchronized(lock) { greeting }

    /** The number of incoming calls running now. */
    fun incoming(): Int = synchronized(lock) { running.size }

    /** The sizes of the reference tables: what this side exports, what it imports. */
    fun references(): Pair<Int, Int> = synchronized(lock) { exports.size to imports.size }

    // ── I/O ──────────────────────────────────────────────────────

    private fun write() {
        while (true) {
            val data = try {
                outbox.poll(1, TimeUnit.SECONDS)
            } catch (_: InterruptedException) {
                continue
            }
            if (data == null) {
                if (closeError != null && outbox.isEmpty()) return
                continue
            }
            if (closeError != null) return
            try {
                channel.send(data)
            } catch (e: Exception) {
                fault(HostGoneException(e.message ?: e.javaClass.simpleName, e))
                return
            }
        }
    }

    private fun read() {
        while (true) {
            val message = try {
                channel.recv()
            } catch (e: Exception) {
                close(HostGoneException(e.message ?: e.javaClass.simpleName, e))
                channel.close("the connection failed")
                return
            }
            if (message == null) {
                close(HostGoneException(null))
                return
            }
            try {
                val frame = parseFrame(message)
                receive(frame)
            } catch (e: Exception) {
                fault(
                    when (e) {
                        is ProtocolException -> HostGoneException(e.message, e)
                        is IOException -> e
                        else -> HostGoneException(e.message ?: e.javaClass.simpleName, e)
                    },
                )
                return
            }
        }
    }

    private fun parseFrame(message: ByteArray): JsonObject {
        val text = String(message, Charsets.UTF_8)
        if (text.isNotEmpty() && text[0] == '\uFEFF') throw ProtocolException("the host sent a message with a byte order mark")
        val element = try {
            frameJson.parseToJsonElement(text)
        } catch (e: Exception) {
            throw ProtocolException("the host sent a message that is not JSON")
        }
        return element as? JsonObject ?: throw ProtocolException("the host sent a message that is not an object")
    }

    /** Ends the session with `error`. */
    fun close(error: Throwable?) {
        val waiting: Collection<CompletableFuture<Outcome>>
        val drains: List<CompletableFuture<Unit>>
        val cancelled: Collection<Running>
        val reason = error ?: HostGoneException.closed()
        synchronized(lock) {
            if (closeError != null) return
            closeError = reason
            waiting = ArrayList(pending.values)
            pending.clear()
            exports.clear()
            identities.clear()
            imports.clear()
            heldPlugins.clear()
            drains = ArrayList(draining)
            draining.clear()
            cancelled = ArrayList(running.values)
        }
        ready.complete(Unit)
        for (waiter in waiting) waiter.complete(Outcome(null, reason, ended = true))
        for (drain in drains) drain.complete(Unit)
        for (call in cancelled) call.job.cancel()
        closed.complete(Unit)
    }

    private fun fault(error: Throwable) {
        close(error)
        channel.close(error.message ?: "fault")
    }

    /** Ends the session and its channel. */
    fun shutdown(reason: String) {
        close(IOException(reason))
        channel.close(reason)
    }

    // ── Values ───────────────────────────────────────────────────

    private fun check(record: Imported) {
        if (record.released) throw IllegalStateException("reference released")
    }

    /** Gives an imported reference back now instead of at collection. */
    fun release(record: Imported) {
        val count: Long
        synchronized(lock) {
            if (record.released || closeError != null) {
                record.released = true
                return
            }
            record.released = true
            if (imports[record.id] === record) imports.remove(record.id)
            // A released plugin definition is no longer held.
            if (record.kind == "plugin") heldPlugins.values.removeIf { it.record === record }
            count = record.grants
        }
        outbox.offer(compact(buildJsonObject {
            put("op", "release")
            put("reference", record.id)
            put("count", count)
        }))
    }

    private fun releaseAll(records: List<Imported>) {
        for (record in records) release(record)
    }

    private fun supportsLocked(capability: String) = capability in declared

    /** Whether `value` holds a reference, a tagged value or undefined, so it crosses as a list or record. */
    private fun marked(value: Any?): Boolean = when (value) {
        is JsonElement -> false
        is Exported, is ExportedObject, is RemoteFunction, is RemoteFuture, is RemoteObject, is Plugin, is RemotePlugin, SignalArgument, Undefined -> true
        is Long -> value > MAX_SAFE || value < -MAX_SAFE
        is List<*> -> value.any(::marked)
        is Map<*, *> -> value.values.any(::marked)
        else -> tagOf(value) != null
    }

    /**
     * The wire form of `value`; `lock` is held. `grants` collects the exports
     * it granted, for a rollback; `depth` counts the lists and records around
     * the value (an argument list starts at -1: it is not a level).
     */
    private fun encode(value: Any?, grants: MutableList<Long>, origin: List<String>, depth: Int): JsonObject {
        when (value) {
            is List<*>, is Map<*, *>, is Throwable, is WireMap, is WireSet -> if (depth >= MAX_DEPTH) throw tooDeep()
        }
        val tag = tagOf(value)
        if (tag != null && !supportsLocked("value-$tag")) {
            throw ValueException("cannot send ${describe(value)}: the far end does not take $tag values (it did not declare the capability value-$tag)")
        }
        return when (value) {
            is WireMap -> {
                checkKeys(value.entries.map { it.first }, "map key")
                wire("map", JsonArray(value.entries.map { (key, item) ->
                    JsonArray(listOf(encode(key, grants, origin, depth + 1), encode(if (item === Undefined) null else item, grants, origin, depth + 1)))
                }))
            }
            is WireSet -> {
                checkKeys(value.elements, "set element")
                wire("set", JsonArray(value.elements.map { encode(it, grants, origin, depth + 1) }))
            }
            Undefined -> buildJsonObject { put("type", "undefined") }
            SignalArgument -> buildJsonObject { put("type", "signal") }
            is RemoteFunction -> {
                check(value.record)
                homeReference(value.record)
            }
            is RemoteFuture -> {
                check(value.record)
                homeReference(value.record)
            }
            is RemoteObject -> {
                check(value.record)
                homeReference(value.record)
            }
            is RemotePlugin -> {
                check(value.record)
                homeReference(value.record)
            }
            // A plugin definition, only to a far end that takes them; only
            // this call fails (plugin values §5).
            is Plugin -> {
                // A test host hands a definition of the far end back home.
                val held = heldPlugins[value]
                if (held != null && !held.record.released) return homeReference(held.record)
                if (!supportsLocked("plugins")) {
                    throw ValueException("cannot send a plugin definition: the far end does not accept plugin definitions (it did not declare the capability plugins)")
                }
                exportValue(value, "plugin", value, grants, origin)
            }
            is Exported -> exportValue(value, "function", value.original, grants, origin)
            is ExportedObject -> {
                if (opts.endpoint != null && !supportsLocked("objects")) throw objectRefused(value.original, greeting?.endpoint)
                exportValue(value, "object", value.original, grants, origin)
            }
            is Long -> if (value > MAX_SAFE || value < -MAX_SAFE) {
                encode(BigInteger.valueOf(value), grants, origin, depth)
            } else {
                data(JsonPrimitive(value), depth)
            }
            is List<*> -> if (marked(value)) {
                // Undefined inside a value is null (#687 §七); an argument
                // list (depth -1) keeps it, as an omitted argument.
                wire("list", JsonArray(value.map { encode(if (it === Undefined && depth >= 0) null else it, grants, origin, depth + 1) }))
            } else {
                data(plain(value), depth)
            }
            is Map<*, *> -> if (marked(value)) {
                wire("record", JsonObject(value.entries.associate { (key, item) ->
                    (key as? String ?: throw ValueException("a record key is not a string")) to encode(if (item === Undefined) null else item, grants, origin, depth + 1)
                }))
            } else {
                data(plain(value), depth)
            }
            is Throwable -> data(buildJsonObject {
                put("name", errorName(value))
                put("message", errorMessage(value))
            }, depth)
            else -> if (tag != null) {
                try {
                    wire(tag, JsonPrimitive(taggedText(value!!)))
                } catch (e: ValueException) {
                    throw e
                }
            } else {
                data(plain(value), depth)
            }
        }
    }

    private fun wire(type: String, value: JsonElement) = buildJsonObject {
        put("type", type)
        put("value", value)
    }

    private fun data(value: JsonElement, depth: Int): JsonObject {
        if (depth + jsonDepth(value) > MAX_DEPTH) throw tooDeep()
        return wire("data", value)
    }

    private fun homeReference(record: Imported) = wire("reference", buildJsonObject {
        put("id", record.id)
        put("kind", record.kind)
        put("home", true)
        put("origin", JsonArray(record.origin.map(::JsonPrimitive)))
    })

    private fun exportValue(value: Any, kind: String, key: Any, grants: MutableList<Long>, origin: List<String>): JsonObject {
        var id = identities[key]
        var entry = id?.let { exports[it] }
        if (entry == null) {
            ref++
            id = ref
            entry = Export(value, kind, origin.toList())
            exports[id] = entry
            identities[key] = id
        }
        entry.grants++
        grants.add(id!!)
        return wire("reference", buildJsonObject {
            put("id", id)
            put("kind", entry.kind)
            put("home", false)
            put("origin", JsonArray(entry.origin.map(::JsonPrimitive)))
        })
    }

    private fun rollback(grants: List<Long>) {
        for (id in grants) {
            val entry = exports[id] ?: continue
            entry.grants--
            if (entry.grants == 0L) forget(id)
        }
    }

    private fun forget(id: Long) {
        exports.remove(id)
        identities.entries.removeIf { it.value == id }
    }

    /** Decodes a wire value; `lock` is held. A malformed value throws ProtocolException; one this side cannot take is in `d.refused`. */
    private fun decode(raw: JsonElement?, d: Decoding): Any? {
        if (raw == null) return Undefined
        val wire = raw as? JsonObject ?: throw ProtocolException("invalid wire value")
        val type = (wire["type"] as? JsonPrimitive)?.takeIf { it.isString }?.content ?: throw ProtocolException("invalid wire value")
        val value = wire["value"]
        return when (type) {
            "undefined" -> Undefined
            "data" -> value ?: JsonNull
            "list" -> {
                val items = value as? JsonArray ?: throw ProtocolException("invalid list")
                // Undefined inside a value is null, also from an older far
                // end (#687 §七); the items of an argument list keep it.
                val keep = d.arguments
                d.arguments = false
                items.map { decode(it, d).let { item -> if (item === Undefined && !keep) JsonNull else item } }
            }
            "record" -> {
                val items = value as? JsonObject ?: throw ProtocolException("invalid record")
                d.arguments = false
                LinkedHashMap<String, Any?>().apply { for ((k, v) in items) put(k, decode(v, d).let { item -> if (item === Undefined) JsonNull else item }) }
            }
            "signal" -> {
                val call = d.call ?: throw ProtocolException("a signal is only valid as a call argument")
                signals.getOrPut(call) { Signal() }
            }
            "reference" -> decodeReference(value, d)
            else -> if (type in TAGS) decodeTagged(type, value, d) else throw ProtocolException("invalid wire value")
        }
    }

    private fun decodeTagged(tag: String, raw: JsonElement?, d: Decoding): Any? {
        fun refuse(error: Throwable): Any? {
            d.refuse(error)
            return null
        }
        val undeclared = { ValueException("received a $tag value, which this side does not take (it did not declare the capability value-$tag)") }
        if (tag == "map" || tag == "set") {
            val items = raw as? JsonArray ?: throw ProtocolException("invalid $tag")
            val before = d.refused
            val out = ArrayList<Any?>()
            if (tag == "map") {
                for (item in items) {
                    val pair = item as? JsonArray
                    if (pair == null || pair.size != 2) throw ProtocolException("invalid map")
                    for (part in pair) out.add(decode(part, d))
                }
            } else {
                for (item in items) out.add(decode(item, d))
            }
            // Their entries are read even then, for the references in them.
            if ("value-$tag" !in declaredHere()) return refuse(undeclared())
            if (d.refused !== before) return null
            return try {
                if (tag == "set") {
                    checkKeys(out, "set element")
                    WireSet(out)
                } else {
                    val entries = (0 until out.size / 2).map { i ->
                        val item = out[2 * i + 1]
                        // Undefined is null in data (design V10).
                        out[2 * i] to (if (item === Undefined) JsonNull else item)
                    }
                    checkKeys(entries.map { it.first }, "map key")
                    WireMap(entries)
                }
            } catch (e: ValueException) {
                refuse(e)
            }
        }
        val text = (raw as? JsonPrimitive)?.takeIf { it.isString }?.content ?: throw ProtocolException("invalid $tag")
        if ("value-$tag" !in declaredHere()) return refuse(undeclared())
        return try {
            parseTagged(tag, text)
        } catch (e: ValueException) {
            refuse(e)
        }
    }

    private fun declaredHere(): List<String> = opts.endpoint?.let { opts.capabilities + it.declare } ?: SESSION_CAPABILITIES

    private fun decodeReference(raw: JsonElement?, d: Decoding): Any? {
        val value = raw as? JsonObject ?: throw ProtocolException("invalid reference")
        val invalid = { ProtocolException("invalid reference") }
        val idElement = value["id"] as? JsonPrimitive ?: throw invalid()
        val home = (value["home"] as? JsonPrimitive)?.takeIf { !it.isString }?.content
        val originElement = value["origin"] as? JsonArray ?: throw invalid()
        if (idElement.isString || home == null || (home != "true" && home != "false")) throw invalid()
        val id = idElement.content.toLongOrNull() ?: throw invalid()
        if (id <= 0 || id > MAX_SAFE) throw invalid()
        val kind = (value["kind"] as? JsonPrimitive)?.takeIf { it.isString }?.content
        if (kind != "function" && kind != "future" && kind != "object" && kind != "plugin") throw invalid()
        val pattern = if (opts.endpoint == null) compatOrigin else endpointOrigin
        val origin = originElement.map {
            val text = (it as? JsonPrimitive)?.takeIf { p -> p.isString }?.content ?: throw invalid()
            if (!pattern.matches(text)) throw invalid()
            text
        }
        if (home == "true") {
            val entry = exports[id]
            if (entry == null || entry.kind != kind) throw ProtocolException("unknown, released or mismatched reference")
            return entry.value
        }
        // Only a test host takes the far end's plugin definitions; rutis
        // never sends one that is not this side's.
        if (kind == "plugin" && !opts.host) throw ProtocolException("plugin definitions from the far end are not supported")
        imports[id]?.let { record ->
            val proxy: Any? = when (record.kind) {
                "function" -> record.function?.get()
                "plugin" -> record.plugin?.get()
                "object" -> record.obj?.get()
                else -> record.future?.get()
            }
            if (proxy != null) {
                if (record.kind != kind || record.origin != origin) throw ProtocolException("invalid repeated grant")
                record.grants++
                return proxy
            }
        }
        val record = Imported(id, kind, origin)
        imports[id] = record
        d.fresh.add(record)
        if (kind == "plugin") {
            val proxy = RemotePlugin(this, record, opts.localPlugin?.invoke(id))
            record.plugin = WeakReference(proxy)
            proxy.local?.let { heldPlugins[it] = proxy }
            Collector.watch(proxy, this, record)
            return proxy
        }
        if (kind == "object") {
            val proxy = RemoteObject(this, record)
            record.obj = WeakReference(proxy)
            Collector.watch(proxy, this, record)
            return proxy
        }
        return if (kind == "function") {
            val proxy = RemoteFunction(this, record)
            record.function = WeakReference(proxy)
            Collector.watch(proxy, this, record)
            proxy
        } else {
            val proxy = RemoteFuture(this, record)
            record.future = WeakReference(proxy)
            Collector.watch(proxy, this, record)
            proxy
        }
    }

    // ── Outgoing calls ───────────────────────────────────────────

    /** The frame fields of a synchronous call: `sync: true` for a far end that declared `sync-wait`. */
    private fun waiting(fields: Map<String, JsonElement>, sync: Boolean): Map<String, JsonElement> =
        if (sync && supports("sync-wait")) fields + ("sync" to JsonPrimitive(true)) else fields

    private fun invokeFields(target: String, method: String) =
        mapOf("target" to JsonPrimitive(target), "method" to JsonPrimitive(method))

    /**
     * Sends a call now and returns its id and the wait for its result: frames
     * of successive requests go out in that order.
     */
    private fun request(
        op: String,
        fields: Map<String, JsonElement>,
        args: List<Any?>?,
        origin: List<String>,
        chain: List<String>,
        answered: ((Outcome) -> Unit)? = null,
    ): Pair<String, CompletableFuture<Outcome>> {
        synchronized(lock) {
            closeError?.let { throw it }
            if (!handshake) throw IOException("protocol handshake incomplete")
            next++
            if (next > MAX_SAFE) throw IOException("call identifiers exhausted")
            val id = local + next
            val path = LinkedHashSet<String>().apply {
                addAll(chain)
                addAll(origin)
            }
            val frame = LinkedHashMap<String, JsonElement>()
            frame["op"] = JsonPrimitive(op)
            frame["id"] = JsonPrimitive(id)
            frame["path"] = JsonArray(path.map(::JsonPrimitive))
            frame.putAll(fields)
            val grants = ArrayList<Long>()
            // What cannot be sent fails only this call, here (#675).
            if (args != null) {
                try {
                    frame["args"] = encode(args, grants, chain, -1)
                } catch (e: Exception) {
                    rollback(grants)
                    throw if (e is ValueException) blame(e, args, chain, describe(op, fields)) else e
                }
            }
            val data = try {
                compact(JsonObject(frame))
            } catch (e: Exception) {
                rollback(grants)
                throw e
            }
            if (data.size > MAX_MESSAGE) {
                rollback(grants)
                throw tooLarge(data.size, frame["args"], describe(op, fields))
            }
            val waiter = CompletableFuture<Outcome>()
            // Attached before the frame goes out: it runs on the thread that
            // completes the waiter, the reader as it takes the reply.
            if (answered != null) waiter.thenAccept(answered)
            pending[id] = waiter
            outbox.offer(data)
            return id to waiter
        }
    }

    /** What an outgoing call is, for messages. */
    private fun describe(op: String, fields: Map<String, JsonElement>): String {
        fun field(name: String) = (fields[name] as? JsonPrimitive)?.takeIf { it.isString }?.content
        return when (op) {
            "invoke" -> invocation(field("target") ?: "", field("method") ?: "")
            "call" -> field("method")?.let { "the method $it" } ?: "a function"
            else -> "an awaited result"
        }
    }

    /**
     * A value error of a call's arguments as one of the argument it is in,
     * found by encoding each alone (granting nothing); `lock` is held.
     */
    private fun blame(error: ValueException, args: List<Any?>, chain: List<String>, call: String): ValueException {
        for ((index, arg) in args.withIndex()) {
            val scratch = ArrayList<Long>()
            val failed = try {
                encode(arg, scratch, chain, 0)
                false
            } catch (_: Exception) {
                true
            }
            rollback(scratch)
            if (failed) return ofArgument(error.message, index, call)
        }
        return ValueException("the arguments of $call: ${error.message}")
    }

    /** A call's message over the limit, blamed on its largest argument. */
    private fun tooLarge(size: Int, args: JsonElement?, call: String): ValueException {
        val items = (args as? JsonObject)?.get("value") as? JsonArray
        val index = items?.indices?.maxByOrNull { compact(items[it]).size }
        return if (index != null) ofArgument(overLimit(size), index, call, MAX_MESSAGE) else ofArguments(overLimit(size), call, MAX_MESSAGE)
    }

    /** Gives up waiting for call `id`; an asynchronous one hears so. */
    private fun abandon(id: String, cancellable: Boolean) {
        val waiting = synchronized(lock) { pending.remove(id) != null }
        if (waiting && cancellable && closeError == null) {
            outbox.offer(compact(buildJsonObject {
                put("op", "cancel")
                put("id", id)
            }))
        }
    }

    private fun result(outcome: Outcome): Any? {
        outcome.error?.let {
            // A received error's frames are where it reaches the caller here,
            // what it crosses with when thrown on (its `rethrown`).
            if (it is RutisException && it.graph != null) it.fillInStackTrace()
            throw it
        }
        return outcome.value
    }

    private fun waitBlocking(id: String, waiter: CompletableFuture<Outcome>, cancellable: Boolean): Any? {
        val outcome = try {
            waiter.get()
        } catch (e: InterruptedException) {
            abandon(id, cancellable)
            Thread.currentThread().interrupt()
            throw CancellationException("interrupted").apply { initCause(e) }
        } catch (e: ExecutionException) {
            throw e.cause ?: e
        }
        return result(outcome)
    }

    private suspend fun waitSuspend(id: String, waiter: CompletableFuture<Outcome>, cancellable: Boolean): Any? {
        val outcome = try {
            waiter.await()
        } catch (e: CancellationException) {
            abandon(id, cancellable)
            throw e
        }
        val error = outcome.error
        if (error is RutisException && error.graph != null) {
            // Resumed, the thread's stack no longer holds the suspended
            // callers: the coroutine's own frames are where it reaches them.
            val frames = suspendedFrames()
            if (frames.isNotEmpty()) {
                error.stackTrace = frames.toTypedArray()
                throw error
            }
        }
        return result(outcome)
    }

    /** The frames of the suspend functions waiting on this one, innermost first. */
    private suspend fun suspendedFrames(): List<StackTraceElement> = suspendCoroutineUninterceptedOrReturn { continuation ->
        val frames = ArrayList<StackTraceElement>()
        var frame = continuation as? CoroutineStackFrame
        while (frame != null && frames.size < 256) {
            frame.getStackTraceElement()?.let(frames::add)
            frame = frame.callerFrame
        }
        frames
    }

    /**
     * Invokes `method` of `target` on the far end, blocking this thread:
     * synchronously (the far end may call back on this chain while it runs) or
     * as an asynchronous call, which an interrupt cancels.
     */
    fun call(target: String, method: String, args: List<Any?>, sync: Boolean): Any? {
        refuseOnMainThread("$method")
        val (id, waiter) = request("invoke", waiting(invokeFields(target, method), sync), args, emptyList(), CallChain.current())
        return settle(waitBlocking(id, waiter, !sync))
    }

    /** [call], suspending: cancelling the coroutine cancels an asynchronous call. */
    suspend fun callSuspend(target: String, method: String, args: List<Any?>, sync: Boolean): Any? {
        val (id, waiter) = request("invoke", waiting(invokeFields(target, method), sync), args, emptyList(), CallChain.current())
        return settleSuspend(waitSuspend(id, waiter, !sync))
    }

    /**
     * Sends a call now, and returns the wait for its result: frames sent by
     * successive sends go out in that order.
     */
    fun send(target: String, method: String, args: List<Any?>, sync: Boolean): suspend () -> Any? {
        val (id, waiter) = request("invoke", waiting(invokeFields(target, method), sync), args, emptyList(), CallChain.current())
        return { settleSuspend(waitSuspend(id, waiter, !sync)) }
    }

    /**
     * Sends a call nobody waits for, ahead of anything sent later, and tells
     * `answered` its reply as the reply's frame is taken, before the next
     * frame (or at the session's end): the reply's value, or why it failed.
     */
    fun sendAnswered(target: String, method: String, args: List<Any?>, answered: (Any?, Throwable?) -> Unit) {
        request("invoke", invokeFields(target, method), args, emptyList(), CallChain.current()) { outcome ->
            answered(outcome.value, outcome.error)
        }
    }

    /** Sends a call whose result nobody awaits, ahead of anything sent later. */
    fun notify(target: String, method: String, args: List<Any?>) {
        try {
            val (id, _) = request("invoke", invokeFields(target, method), args, emptyList(), emptyList())
            // Its reply, when it comes, is a late one: dropped.
            synchronized(lock) { pending.remove(id) }
        } catch (_: Exception) {
        }
    }

    /**
     * Sends a call as [notify] does, and returns a future completed once the
     * far end answered it (whatever the answer) or the session ended; `failed`
     * is told why when the far end refuses the call or it cannot be sent.
     */
    fun announce(target: String, method: String, args: List<Any?>, failed: ((Throwable) -> Unit)? = null): CompletableFuture<Unit> {
        val done = CompletableFuture<Unit>()
        val waiter = try {
            request("invoke", invokeFields(target, method), args, emptyList(), emptyList()).second
        } catch (e: Exception) {
            failed?.invoke(e)
            done.complete(Unit)
            return done
        }
        waiter.whenComplete { outcome, _ ->
            if (outcome != null && outcome.error != null && !outcome.ended) failed?.invoke(outcome.error)
            done.complete(Unit)
        }
        return done
    }

    /**
     * A call that waits for rutis is refused on an application's main
     * thread (design §11.3 #3): it would freeze the interface, and rutis may
     * need that thread to answer. A `suspend` function waits without it.
     */
    private fun refuseOnMainThread(what: String) {
        if (arcships.rutis.internal.onMainThread()) {
            throw IllegalStateException("$what waits for rutis: not on the main thread; call it as a suspend function or from another thread")
        }
    }

    /** Calls a function of the far end synchronously, blocking this thread. */
    fun callFunction(function: RemoteFunction, args: List<Any?>, sync: Boolean = true): Any? {
        refuseOnMainThread("a function of the far end")
        synchronized(lock) { check(function.record) }
        val fields = waiting(mapOf("reference" to JsonPrimitive(function.record.id)), sync)
        val (id, waiter) = request("call", fields, args, emptyList(), CallChain.current())
        return settle(waitBlocking(id, waiter, !sync))
    }

    /** Calls a function of the far end synchronously, suspending. */
    suspend fun callFunctionSuspend(function: RemoteFunction, args: List<Any?>, sync: Boolean = true): Any? {
        synchronized(lock) { check(function.record) }
        val fields = waiting(mapOf("reference" to JsonPrimitive(function.record.id)), sync)
        val (id, waiter) = request("call", fields, args, emptyList(), CallChain.current())
        return settleSuspend(waitSuspend(id, waiter, !sync))
    }

    /** Calls the method `method` of an object of the far end, blocking this thread. */
    fun callMethod(target: RemoteObject, method: String, args: List<Any?>, sync: Boolean = true): Any? {
        refuseOnMainThread("an object of the far end")
        synchronized(lock) { check(target.record) }
        val fields = waiting(mapOf("reference" to JsonPrimitive(target.record.id), "method" to JsonPrimitive(method)), sync)
        val (id, waiter) = request("call", fields, args, emptyList(), CallChain.current())
        return settle(waitBlocking(id, waiter, !sync))
    }

    /** Calls the method `method` of an object of the far end, suspending. */
    suspend fun callMethodSuspend(target: RemoteObject, method: String, args: List<Any?>, sync: Boolean = false): Any? {
        synchronized(lock) { check(target.record) }
        val fields = waiting(mapOf("reference" to JsonPrimitive(target.record.id), "method" to JsonPrimitive(method)), sync)
        val (id, waiter) = request("call", fields, args, emptyList(), CallChain.current())
        return settleSuspend(waitSuspend(id, waiter, !sync))
    }

    private fun awaitRequest(future: RemoteFuture): Pair<String, CompletableFuture<Outcome>> {
        synchronized(lock) { check(future.record) }
        return request("await", mapOf("reference" to JsonPrimitive(future.record.id)), null, future.record.origin, CallChain.current())
    }

    /** Waits for `value` while it is a future, blocking. */
    fun settle(value: Any?): Any? {
        if (value is RemoteFuture) refuseOnMainThread("a result of the far end")
        var current = value
        while (current is RemoteFuture) {
            val future = current
            future.settled?.let { current = result(it); continue }
            val (id, waiter) = awaitRequest(future)
            val outcome = try {
                Outcome(waitBlocking(id, waiter, true), null)
            } catch (e: CancellationException) {
                throw e
            } catch (e: Throwable) {
                Outcome(null, e)
            }
            future.settled = outcome
            current = result(outcome)
        }
        return current
    }

    /** Waits for `value` while it is a future, suspending. */
    suspend fun settleSuspend(value: Any?): Any? {
        var current = value
        while (current is RemoteFuture) {
            val future = current
            future.settled?.let { current = result(it); continue }
            val (id, waiter) = awaitRequest(future)
            val outcome = try {
                Outcome(waitSuspend(id, waiter, true), null)
            } catch (e: CancellationException) {
                throw e
            } catch (e: Throwable) {
                Outcome(null, e)
            }
            future.settled = outcome
            current = result(outcome)
        }
        return current
    }

    /** Waits until no call that is the far end's business runs, at most `timeoutMillis`. */
    fun drain(timeoutMillis: Long) {
        val waiter = synchronized(lock) {
            if (active == 0 || closeError != null) return
            CompletableFuture<Unit>().also { draining.add(it) }
        }
        try {
            waiter.get(timeoutMillis, TimeUnit.MILLISECONDS)
        } catch (_: TimeoutException) {
        } catch (_: InterruptedException) {
            Thread.currentThread().interrupt()
        }
    }

    private fun finish() {
        val drains = synchronized(lock) {
            active--
            if (active == 0) ArrayList(draining).also { draining.clear() } else emptyList()
        }
        for (drain in drains) drain.complete(Unit)
    }

    // ── Incoming frames ──────────────────────────────────────────

    private fun string(frame: JsonObject, field: String, error: String): String =
        (frame[field] as? JsonPrimitive)?.takeIf { it.isString }?.content ?: throw ProtocolException(error)

    private fun receive(frame: JsonObject) {
        val op = (frame["op"] as? JsonPrimitive)?.takeIf { it.isString }?.content ?: throw ProtocolException("invalid frame")
        synchronized(lock) {
            if (closeError != null) return
            if (op == "hello") {
                if (handshake) throw ProtocolException("duplicate protocol handshake")
                greet(frame)
                handshake = true
                ready.complete(Unit)
                return
            }
            if (!handshake) throw ProtocolException("request before protocol handshake")
        }
        when (op) {
            "return", "throw" -> {
                val id = string(frame, "id", "invalid response")
                var fresh: List<Imported> = emptyList()
                val waiter: CompletableFuture<Outcome>?
                val outcome: Outcome
                synchronized(lock) {
                    outcome = if (op == "return") {
                        val d = Decoding(null)
                        val value = decode(frame["value"], d)
                        val refused = d.refused
                        if (refused != null) {
                            // A value this side cannot take fails the call
                            // waiting for it; the references it carried go back.
                            fresh = d.fresh
                            Outcome(null, refused)
                        } else {
                            Outcome(value, null)
                        }
                    } else {
                        Outcome(null, decodeError(frame["error"]))
                    }
                    waiter = pending.remove(id)
                }
                releaseAll(fresh)
                if (waiter == null) {
                    // A late reply to a call of ours given up (cancelled, or a
                    // notification) is dropped; any other is a fault.
                    if (issued(id)) return
                    throw ProtocolException("response for unknown call")
                }
                waiter.complete(outcome)
            }
            "cancel" -> {
                val id = string(frame, "id", "invalid cancel")
                val (call, signal) = synchronized(lock) { running[id] to signals[id] }
                signal?.cancel()
                if (call != null) {
                    call.job.cancel()
                    call.thread?.interrupt()
                }
            }
            "release" -> synchronized(lock) {
                val reference = (frame["reference"] as? JsonPrimitive)?.takeIf { !it.isString }?.content?.toLongOrNull()
                    ?: throw ProtocolException("unknown, released or mismatched reference")
                val entry = exports[reference] ?: throw ProtocolException("unknown, released or mismatched reference")
                val count = (frame["count"] as? JsonPrimitive)?.takeIf { !it.isString }?.content?.toLongOrNull()
                if (count == null || count <= 0 || count > entry.grants) throw ProtocolException("invalid reference release count")
                entry.grants -= count
                if (entry.grants == 0L) forget(reference)
            }
            else -> invocation(op, frame)
        }
    }

    /** Whether `id` names a call this side made. */
    private fun issued(id: String): Boolean {
        if (!id.startsWith(local)) return false
        val rest = id.substring(local.length)
        if (!sequence.matches(rest)) return false
        val n = rest.toLongOrNull() ?: return false
        return synchronized(lock) { n <= next }
    }

    private fun versionMismatch(frame: JsonObject, localVersion: Int): String {
        val far = frame["version"]?.toString() ?: "none"
        val mismatch = "the far end speaks protocol $far, this side $localVersion"
        val version = (frame["version"] as? JsonPrimitive)?.takeIf { !it.isString }?.content?.toIntOrNull()
        if (version == null || version == localVersion) return mismatch
        if (version > localVersion) return "$mismatch: upgrade this side (io.github.arcships:rutis)"
        val implementation = frame["implementation"] as? JsonObject
        val name = (implementation?.get("name") as? JsonPrimitive)?.content
        val ver = (implementation?.get("version") as? JsonPrimitive)?.content
        if (name != null && ver != null) return "$mismatch: upgrade the far end ($name $ver)"
        return "$mismatch: upgrade the far end"
    }

    private fun capabilitiesOf(frame: JsonObject): List<String> =
        (frame["capabilities"] as? JsonArray)?.mapNotNull { (it as? JsonPrimitive)?.takeIf { p -> p.isString }?.content } ?: emptyList()

    /** Reads the far end's handshake; `lock` is held. */
    private fun greet(frame: JsonObject) {
        val version = (frame["version"] as? JsonPrimitive)?.takeIf { !it.isString }?.content?.toIntOrNull()
        val endpoint = opts.endpoint
        if (endpoint == null) {
            if (version != PROTOCOL || frame.containsKey("endpoint")) {
                throw IncompatibleException("incompatible session: " + versionMismatch(frame, PROTOCOL))
            }
            declared = capabilitiesOf(frame)
            required()
            return
        }
        if (version != ENDPOINT_PROTOCOL) throw IncompatibleException("incompatible session: " + versionMismatch(frame, ENDPOINT_PROTOCOL))
        val far = (frame["endpoint"] as? JsonPrimitive)?.takeIf { it.isString }?.content
        if (far == null || !endpointId.matches(far)) throw IncompatibleException("incompatible session: the far end named no valid endpoint")
        if (endpoint.expected != null && endpoint.expected != far) {
            throw EndpointMismatchException("endpoint mismatch: the far end greeted as $far, but ${endpoint.expected} is the expected endpoint")
        }
        if (far == endpoint.local) throw EndpointMismatchException("endpoint mismatch: the far end greeted as this endpoint ($far)")
        remote = "$far:"
        val capabilities = capabilitiesOf(frame)
        greeting = Greeting(far, frame["implementation"], capabilities)
        declared = capabilities
        required()
    }

    /** Refuses a far end lacking a capability [Options.require] lists; `lock` is held. */
    private fun required() {
        for (capability in opts.require) {
            if (capability !in declared) {
                throw RefusedException(opts.refuse?.invoke(capability) ?: "the far end does not declare $capability")
            }
        }
    }

    /** Validates an incoming call and starts it on a thread of its own. */
    private fun invocation(op: String, frame: JsonObject) {
        val invalid = { ProtocolException("invalid invocation") }
        if (op != "invoke" && op != "call" && op != "get" && op != "await") throw invalid()
        val id = (frame["id"] as? JsonPrimitive)?.takeIf { it.isString }?.content ?: throw invalid()
        val pathElement = frame["path"] as? JsonArray ?: throw invalid()
        val path = pathElement.map { (it as? JsonPrimitive)?.takeIf { p -> p.isString }?.content ?: throw invalid() }
        var args: List<Any?> = emptyList()
        var method: String? = null
        var target = ""
        var property = ""
        var entry: Export? = null
        var refused: Throwable? = null
        var fresh: List<Imported> = emptyList()
        val call: Running
        val business: Boolean
        synchronized(lock) {
            if (!id.startsWith(remote) || !sequence.matches(id.substring(remote.length))) throw invalid()
            if (id in path) throw invalid()
            val seq = id.substring(remote.length).toLongOrNull()
            if (seq == null || seq <= received || seq > MAX_SAFE) throw ProtocolException("invalid or repeated invocation identity")
            received = seq
            val methodElement = frame["method"]
            if (methodElement != null && methodElement != JsonNull) {
                method = (methodElement as? JsonPrimitive)?.takeIf { it.isString }?.content ?: throw ProtocolException("invalid method")
            }
            if (op == "get") property = string(frame, "property", "invalid property")
            if (op == "invoke" || op == "call") {
                val d = Decoding(id)
                d.arguments = true
                args = arguments(decode(frame["args"], d))
                // Arguments this side cannot take fail the call: it is
                // answered by a throw, and the references they carried go back.
                refused = d.refused
                fresh = d.fresh
            }
            if (op == "invoke") {
                val targetElement = frame["target"] ?: throw invalid()
                target = (targetElement as? JsonPrimitive)?.takeIf { it.isString }?.content ?: throw invalid()
                if (method == null) throw invalid()
                business = target != ""
            } else {
                val kind = when {
                    op == "await" -> "future"
                    op == "get" || method != null -> "object"
                    else -> "function"
                }
                val reference = (frame["reference"] as? JsonPrimitive)?.takeIf { !it.isString }?.content?.toLongOrNull()
                    ?: throw ProtocolException("unknown, released or mismatched reference")
                val found = exports[reference]
                if (found == null || found.kind != kind) throw ProtocolException("unknown, released or mismatched reference")
                entry = found
                business = true
            }
            if (business) active++
            call = Running(id, Job())
            running[id] = call
        }
        val chain = path + id
        // What it is, for a result that cannot be sent.
        val described = when (op) {
            "invoke" -> invocation(target, method ?: "")
            "call" -> method?.let { "the method $it" } ?: "a function"
            "get" -> "the property $property"
            else -> null
        }
        var then: (suspend () -> Any?)? = null
        val arrive = opts.arrive
        if (arrive != null && op == "invoke" && target == "" && refused == null) {
            try {
                then = CallChain.with(chain) { arrive(method!!, args) }
            } catch (e: Throwable) {
                then = { throw e }
            }
        }
        val runMethod = method
        val runEntry = entry
        val thread = Thread({
            try {
                execute(call, chain, refused, fresh, described) {
                    when {
                        then != null -> then!!.invoke()
                        op == "invoke" -> (opts.dispatch ?: throw IllegalStateException("no target $target"))(target, runMethod!!, args)
                        op == "await" -> throw IllegalStateException("this side exports no futures")
                        op == "get" -> (runEntry!!.value as ExportedObject).get(property)
                        runMethod != null -> (runEntry!!.value as ExportedObject).call(runMethod, args)
                        else -> (runEntry!!.value as Exported).body(args)
                    }
                }
            } finally {
                if (business) finish()
            }
        }, "rutis-call-$id")
        thread.isDaemon = true
        call.thread = thread
        thread.start()
    }

    /** A call's argument list. */
    private fun arguments(decoded: Any?): List<Any?> = when (decoded) {
        is List<*> -> decoded
        Undefined, null, JsonNull -> emptyList()
        is JsonArray -> decoded.toList()
        else -> listOf(decoded)
    }

    private fun execute(
        call: Running,
        chain: List<String>,
        refused: Throwable?,
        fresh: List<Imported>,
        described: String?,
        body: suspend () -> Any?,
    ) {
        CallChain.local.set(chain)
        if (refused != null) {
            releaseAll(fresh)
            respond(call.id, null, refused, chain, described)
            return
        }
        var value: Any? = null
        var failure: Throwable? = null
        try {
            value = runBlocking(call.job + CallChain.element(chain)) { body() }
        } catch (e: InterruptedException) {
            failure = CancellationException("cancelled").apply { initCause(e) }
        } catch (e: Throwable) {
            failure = e
        }
        Thread.interrupted()
        respond(call.id, value, failure, chain, described)
    }

    /**
     * Answers call `id`, a call of `described` (for messages): a result that
     * cannot be sent (a value the far end does not take, a message over the
     * limit) is answered with an error saying why instead (#675).
     */
    private fun respond(id: String, value: Any?, failure: Throwable?, chain: List<String>, described: String?) {
        val data: ByteArray
        synchronized(lock) {
            running.remove(id)
            signals.remove(id)
            if (closeError != null) return
            var error = failure
            var encoded: ByteArray? = null
            if (error == null) {
                val grants = ArrayList<Long>()
                try {
                    val wire = try {
                        encode(value, grants, chain, 0)
                    } catch (e: ValueException) {
                        throw ofResult(e.message, described)
                    }
                    val bytes = compact(buildJsonObject {
                        put("op", "return")
                        put("id", id)
                        put("value", wire)
                    })
                    if (bytes.size > MAX_MESSAGE) throw ofResult(overLimit(bytes.size), described, MAX_MESSAGE)
                    encoded = bytes
                } catch (e: Throwable) {
                    rollback(grants)
                    error = e
                    encoded = null
                }
            }
            fun short(message: String) = compact(buildJsonObject {
                put("op", "throw")
                put("id", id)
                put("error", buildJsonObject {
                    put("name", "Error")
                    put("message", message)
                })
            })
            data = encoded ?: try {
                val thrown = compact(buildJsonObject {
                    put("op", "throw")
                    put("id", id)
                    put("error", encodeError(error!!))
                })
                // An error too large to send: a short one saying so.
                if (thrown.size > MAX_MESSAGE) {
                    val what = "the error ${errorName(error!!)}" + (described?.let { " of $it" } ?: "")
                    short("$what: ${overLimit(thrown.size)}")
                } else {
                    thrown
                }
            } catch (_: Exception) {
                short(error?.message ?: "")
            }
        }
        outbox.offer(data)
    }
}

/** How deep lists and objects nest in a JSON value. */
fun jsonDepth(value: JsonElement): Int = when (value) {
    is JsonArray -> 1 + (value.maxOfOrNull(::jsonDepth) ?: 0)
    is JsonObject -> 1 + (value.values.maxOfOrNull(::jsonDepth) ?: 0)
    else -> 0
}

/** A plain value as JSON data: undefined inside data becomes null (design V10). */
fun plain(value: Any?): JsonElement = when (value) {
    null, Undefined -> JsonNull
    is JsonElement -> value
    is String -> JsonPrimitive(value)
    is Char -> JsonPrimitive(value.toString())
    is Boolean -> JsonPrimitive(value)
    is Double -> if (value.isNaN() || value.isInfinite()) throw ValueException("a number that is not finite ($value) cannot cross") else JsonPrimitive(value)
    is Float -> if (value.isNaN() || value.isInfinite()) throw ValueException("a number that is not finite ($value) cannot cross") else JsonPrimitive(value.toDouble())
    is Long -> if (value > MAX_SAFE || value < -MAX_SAFE) throw ValueException("the integer $value is beyond ±(2^53-1): it crosses as a bigint") else JsonPrimitive(value)
    is Int, is Short, is Byte -> JsonPrimitive(value as Number)
    is List<*> -> JsonArray(value.map(::plain))
    is Map<*, *> -> JsonObject(value.entries.associate { (k, v) -> (k as? String ?: throw ValueException("a record key is not a string")) to plain(v) })
    is Throwable -> buildJsonObject {
        put("name", errorName(value))
        put("message", errorMessage(value))
    }
    else -> throw ValueException("${value.javaClass.name} cannot cross the boundary")
}
