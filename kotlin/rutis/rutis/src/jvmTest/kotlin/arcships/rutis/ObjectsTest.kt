package arcships.rutis

import arcships.rutis.internal.Native
import arcships.rutis.internal.peer.Channel
import arcships.rutis.internal.peer.Endpoint
import arcships.rutis.internal.peer.ExportedObject
import arcships.rutis.internal.peer.MemoryChannel
import arcships.rutis.internal.peer.Options
import arcships.rutis.internal.peer.Peer
import kotlinx.coroutines.DelicateCoroutinesApi
import kotlinx.coroutines.launch
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.jsonArray
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import java.util.concurrent.CompletableFuture
import java.util.concurrent.TimeUnit
import kotlin.test.AfterTest
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertSame
import kotlin.test.assertTrue
import arcships.rutis.generated.rutisObject
import arcships.rutis.generated.ObjectProxy
import arcships.rutis.advanced.RemoteObject
import arcships.rutis.generated.Tables

/** An object with methods of the test's side. */
@RutisService
class Cache {
    private val values = HashMap<String, Long>()

    fun get(key: String): Long? = values[key]

    fun set(key: String, value: Long) {
        values[key] = value
    }

    fun size(): Long = values.size.toLong()
}

/** How the service uses an object of another process. */
@RutisClient
interface CacheClient {
    suspend fun get(key: String): Long?

    suspend fun set(key: String, value: Long)
}

/** Hands out its cache, an object with methods. */
@RutisService
class Source {
    val cache = Cache()

    fun make(): Cache = cache
}

/** How a plugin uses [Source]: its result is an object of another process. */
@RutisClient
interface SourceClient {
    suspend fun make(): CacheClient
}

/** Uses the cache [SourceClient.make] returns, received twice. */
@RutisService
class Reader(private val source: SourceClient) {
    suspend fun read(): String {
        val first = source.make()
        val second = source.make()
        first.set("k", 5)
        return "${first === second} ${second.get("k")}"
    }
}

/** An object whose method runs until its call is cancelled. */
@RutisService
class Slow {
    val cancelled = CompletableFuture<Boolean>()

    suspend fun run(): String = try {
        kotlinx.coroutines.awaitCancellation()
    } catch (e: kotlinx.coroutines.CancellationException) {
        cancelled.complete(true)
        throw e
    }
}

/** A service taking objects as a @RutisClient interface, as Any?, and as data (which takes none). */
@RutisService
class Warmer {
    var kept: CacheClient? = null

    suspend fun warm(cache: CacheClient): Long? {
        cache.set("a", 1)
        kept = cache
        return cache.get("a")
    }

    fun back(): CacheClient = kept!!

    /** Whether `cache` is the proxy it keeps. */
    fun same(cache: CacheClient): Boolean = cache === kept

    fun anything(value: Any?): String = value.toString()

    fun point(p: Point): Long = p.x.toLong()

    var seen: Point? = null

    fun keep(p: Point): Long {
        seen = p
        return p.y.toLong()
    }

    suspend fun missing(): String = try {
        kept!!.let { (it as ObjectProxy).rutisObject!! }.call("gett")
        "answered"
    } catch (e: RutisException) {
        "${e.code}: ${e.message}"
    }
}

/**
 * Objects with methods across processes, Kotlin's side (#687): a received
 * object is a proxy whose methods call the original; one proxy per object,
 * the original again back home; a method the object lacks is no-method;
 * undefined is null inside a value.
 */
@OptIn(DelicateCoroutinesApi::class)
class ObjectsTest {
    private lateinit var runtime: Peer
    private lateinit var host: Peer

    @AfterTest
    fun stop() {
        if (::runtime.isInitialized) runtime.shutdown("done")
        if (::host.isInitialized) host.shutdown("done")
    }

    private val warmer = Warmer()
    private val table = Tables.serviceTable(Warmer::class.java)!!

    private fun start() {
        val (a, b) = MemoryChannel.pair()
        runtime = Peer(a, Options(dispatch = { _, method, args -> table.invoke(warmer, method, args) }, endpoint = Endpoint("kotlin", "main")))
        host = Peer(b, Options(endpoint = Endpoint("main", "kotlin")))
        runtime.start()
        host.start()
        host.readyFuture.get(5, TimeUnit.SECONDS)
        runtime.readyFuture.get(5, TimeUnit.SECONDS)
    }

    // objects-case(kotlin): send-call-state
    // objects-case(kotlin): same-proxy-home
    // objects-case(kotlin): no-method
    // objects-case(kotlin): declared-type-refuses
    @Test
    fun anObjectIsTakenAsAClientInterfaceAndGoesHome() {
        start()
        val cache = Cache()
        assertEquals(JsonPrimitive(1), host.call("t", "warm", listOf(Native.fromNative(cache)), true))
        assertEquals(1L, cache.size(), "the original object changed")
        // Back home: the original.
        val back = host.call("t", "back", emptyList(), true)
        assertSame(cache, (back as ExportedObject).original)
        // Received again: the same proxy.
        assertEquals(JsonPrimitive(true), host.call("t", "same", listOf(Native.fromNative(cache)), true))
        // Any? takes a RemoteObject.
        assertTrue((host.call("t", "anything", listOf(Native.fromNative(cache)), true) as JsonPrimitive).content.startsWith("RemoteObject("))
        // A method the object lacks: its process answers no-method.
        assertEquals(JsonPrimitive("no-method: Cache has no method gett (methods: get, set, size)"), host.call("t", "missing", emptyList(), true))
        // A data parameter takes no object.
        val refused = assertFailsWith<RutisException> { host.call("t", "point", listOf(Native.fromNative(cache)), true) }
        assertTrue(
            refused.message.endsWith("argument 1 should be a Point, got an object (an object is received only as a @RutisClient interface or Any?)"),
            refused.message,
        )
        assertEquals(null, runtime.error)
    }

    private lateinit var far: Channel

    private fun read(): JsonObject = Json.parseToJsonElement(String(far.recv()!!)).jsonObject

    private fun raw(dispatch: suspend (String, String, List<Any?>) -> Any?) {
        val (a, b) = MemoryChannel.pair()
        far = b
        runtime = Peer(a, Options(dispatch = dispatch))
        runtime.start()
        assertEquals("hello", read()["op"]?.jsonPrimitive?.content)
        far.send("""{"op":"hello","version":2,"capabilities":["events-v2","sync-wait"]}""".toByteArray())
        runtime.readyFuture.get(5, TimeUnit.SECONDS)
    }

    // objects-case(kotlin): receiver-releases
    // objects-case(kotlin): undefined-nested-null
    // A received object is a RemoteObject, one per object; its methods are
    // calls naming the method; let go of, it is released. Undefined inside
    // a value is null; a whole argument keeps it.
    @Test
    fun aReceivedObjectIsARemoteObject() {
        val got = CompletableFuture<List<Any?>>()
        raw { _, _, args -> got.complete(args); "ok" }
        val reference = """{"type":"reference","value":{"id":7,"kind":"object","home":false,"origin":[]}}"""
        far.send(
            """{"op":"invoke","id":"rust:1","path":[],"target":"t","method":"take","args":{"type":"list","value":[$reference,$reference,{"type":"undefined"},{"type":"list","value":[{"type":"undefined"}]}]}}""".toByteArray(),
        )
        val args = got.get(5, TimeUnit.SECONDS)
        read()
        val remote = args[0] as RemoteObject
        assertSame(remote, args[1], "one object, one proxy")
        assertSame(arcships.rutis.internal.peer.Undefined, args[2], "a whole argument keeps undefined")
        assertEquals(listOf(JsonNull), args[3], "inside a value, null")
        val answer = CompletableFuture<Any?>()
        Thread { answer.complete(remote.callBlocking("get", "a")) }.start()
        val call = read()
        assertEquals("call", call["op"]?.jsonPrimitive?.content)
        assertEquals("get", call["method"]?.jsonPrimitive?.content)
        assertEquals(JsonPrimitive(7), call["reference"])
        far.send("""{"op":"return","id":${call["id"]},"value":{"type":"data","value":1}}""".toByteArray())
        assertEquals(1L, answer.get(5, TimeUnit.SECONDS))
        remote.release()
        val release = read()
        assertEquals("release", release["op"]?.jsonPrimitive?.content)
        assertEquals(JsonPrimitive(7), release["reference"])
        assertEquals(JsonPrimitive(2), release["count"])
        assertEquals(null, runtime.error)
    }

    // objects-case(kotlin): data-stays-data
    // Data is copied: the service gets an equal value, not the caller's.
    @Test
    fun dataIsCopied() {
        start()
        val point = Point(1, 2)
        assertEquals(JsonPrimitive(2), host.call("t", "keep", listOf(Native.fromNative(point)), true))
        assertEquals(point, warmer.seen)
        assertTrue(point !== warmer.seen, "a copy")
    }

    // objects-case(kotlin): cancel
    // An object of this side: a call of its method the caller gives up is
    // cancelled here; once the host releases it, it is let go of. And a
    // call of a far end's object this side gives up is cancelled there.
    @Test
    fun callsOnObjectsAreCancelledBothWays() {
        val slow = Slow()
        raw { _, method, _ -> if (method == "slow") Native.fromNative(slow) else "ok" }
        far.send("""{"op":"invoke","id":"rust:1","path":[],"target":"t","method":"slow","args":{"type":"list","value":[]}}""".toByteArray())
        val id = read()["value"]!!.jsonObject["value"]!!.jsonObject["id"]!!.jsonPrimitive.content
        far.send("""{"op":"call","id":"rust:2","path":[],"reference":$id,"method":"run","args":{"type":"list","value":[]}}""".toByteArray())
        Thread.sleep(100)
        far.send("""{"op":"cancel","id":"rust:2"}""".toByteArray())
        assertEquals(true, slow.cancelled.get(5, TimeUnit.SECONDS))
        read()
        val before = runtime.references().first
        far.send("""{"op":"release","reference":$id,"count":1}""".toByteArray())
        far.send("""{"op":"invoke","id":"rust:3","path":[],"target":"t","method":"ping","args":{"type":"list","value":[]}}""".toByteArray())
        assertEquals("rust:3", read()["id"]?.jsonPrimitive?.content)
        assertEquals(before - 1, runtime.references().first, "the object is let go of")
        // This side gives up a call of the far end's object.
        val got = CompletableFuture<List<Any?>>()
        runtime.shutdown("next")
        raw { _, _, args -> got.complete(args); "ok" }
        val reference = """{"type":"reference","value":{"id":9,"kind":"object","home":false,"origin":[]}}"""
        far.send("""{"op":"invoke","id":"rust:1","path":[],"target":"t","method":"take","args":{"type":"list","value":[$reference]}}""".toByteArray())
        val remote = got.get(5, TimeUnit.SECONDS)[0] as RemoteObject
        read()
        val job = kotlinx.coroutines.GlobalScope.launch { remote.call("slow") }
        val call = read()
        assertEquals("slow", call["method"]?.jsonPrimitive?.content)
        job.cancel()
        var frame = read()
        while (frame["op"]?.jsonPrimitive?.content != "cancel") frame = read()
        assertEquals(call["id"], frame["id"])
    }

    // objects-case(kotlin): holder-killed-releases
    // The holder goes without releasing the object (its process killed:
    // the channel ends): the object is let go of all the same.
    @Test
    fun anObjectIsLetGoOfWhenItsHolderGoes() {
        raw { _, method, _ -> if (method == "slow") Native.fromNative(Slow()) else "ok" }
        far.send("""{"op":"invoke","id":"rust:1","path":[],"target":"t","method":"slow","args":{"type":"list","value":[]}}""".toByteArray())
        assertEquals("return", read()["op"]?.jsonPrimitive?.content)
        assertEquals(1, runtime.references().first, "the object is exported")
        far.close("killed")
        val deadline = System.nanoTime() + 5_000_000_000
        while (runtime.references().first != 0 && System.nanoTime() < deadline) Thread.sleep(20)
        assertEquals(0, runtime.references().first, "the object is let go of")
    }

    // objects-case(kotlin): strict-testing-tool
    // The testing tool hands the plugin an object across its session, as
    // between processes: the plugin's interface proxy calls the original.
    @Test
    fun theTestingToolPassesObjectsAsBetweenProcesses() = kotlinx.coroutines.runBlocking<Unit> {
        val plugin = Rutis.define<Rutis.NoConfig>(name = "warmer", provides = mapOf("warmer" to service<Warmer>())) { ctx, _ ->
            ctx.provide("warmer", Warmer())
        }
        arcships.rutis.testing.RutisTest.load(plugin).use { loaded ->
            val cache = Cache()
            assertEquals(1L, loaded.service("warmer").call("warm", cache))
            assertEquals(1L, cache.size(), "the original object changed")
            assertSame(cache, loaded.service("warmer").call("back"), "back home, the original")
        }
    }

    // objects-case(kotlin): return-array-event
    // As the result of a @RutisClient interface method, an object of another
    // process is that interface's proxy, the same one each time.
    @Test
    fun anObjectAsAResultIsAClientInterface() = kotlinx.coroutines.runBlocking<Unit> {
        val plugin = Rutis.define<Rutis.NoConfig>(name = "reader", inject = listOf("source"), provides = mapOf("reader" to service<Reader>())) { ctx, _ ->
            ctx.provide("reader", Reader(ctx.use<SourceClient>("source")))
        }
        val source = Source()
        arcships.rutis.testing.RutisTest.load(plugin, services = mapOf("source" to source)).use { loaded ->
            assertEquals("true 5", loaded.service("reader").call("read"))
            assertEquals(5L, source.cache.get("k"), "the original object changed")
        }
    }
}
