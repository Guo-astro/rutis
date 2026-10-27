// The Kotlin owner and holder of the object cases
// (crates/rutis-loader/tests/objects_cross.rs, #687): the same as the
// JavaScript, Python, Go and Swift ones there. The owner hands its counter
// (a @RutisService instance, by reference) to the holder's service in
// another runtime; the holder keeps what it is given as a @RutisClient proxy.
package arcships.rutis.conformance.plugins.objects

import arcships.rutis.RemoteObject
import arcships.rutis.Rutis
import arcships.rutis.RutisClient
import arcships.rutis.RutisException
import arcships.rutis.RutisService
import arcships.rutis.service

/** The owner's object. */
@RutisService
class Counter {
    @Volatile
    var n = 0L

    @Synchronized
    fun inc(): Long = ++n

    fun value(): Long = n
}

@RutisService
class Pinger {
    fun ping(): String = "pong"
}

/** The holder's service as the owner uses it: objects go as `Any?`, by what they are. */
@RutisClient
interface HolderService {
    suspend fun take(counter: Any?): String

    suspend fun inc(): Long

    suspend fun listed(items: Any?): String

    suspend fun back(): Any?

    suspend fun rust(): Long

    suspend fun nope(): String

    suspend fun plain(thing: Any?): String
}

/** The `owner` service. */
@RutisService
class Owner(private val ctx: arcships.rutis.Ctx) {
    private val made = Counter()

    suspend fun run(): String {
        val holder = ctx.use<HolderService>("holder")
        holder.take(made)
        holder.inc()
        holder.inc()
        val listed = holder.listed(listOf(mapOf("data" to 1), made))
        val home = holder.back() === made
        val viaRust = holder.rust()
        val missing = holder.nope()
        val plain = holder.plain(Pinger())
        return "${made.n} $listed $home $viaRust $plain | $missing"
    }

    fun announce(): String {
        ctx.emit("counter", made)
        return "sent"
    }
}

val ktOwner = Rutis.define<Rutis.NoConfig>(name = "kt_owner", inject = listOf("holder"), provides = mapOf("owner" to service<Owner>())) { ctx, _ ->
    ctx.provide("owner", Owner(ctx))
}

/** The holder's parameters: an object of another process, as a proxy. */
@RutisClient
interface Counted {
    suspend fun inc(): Long

    suspend fun value(): Long

    suspend fun nope()
}

@RutisClient
interface Pinged {
    suspend fun ping(): String
}

@RutisClient
interface Poker {
    suspend fun poke(obj: Counted): Long
}

@RutisClient
interface Recorder {
    fun record(line: String)
}

private fun said(error: RutisException): String = "${error.code}: ${error.message}"

/** The `holder` service. */
@RutisService
class Holder(private val poker: Poker) {
    @Volatile
    var kept: Counted? = null

    /** The object as it arrived inside data: a [RemoteObject]. */
    @Volatile
    var listedObject: RemoteObject? = null

    fun take(counter: Counted): String {
        kept = counter
        return "taken"
    }

    suspend fun inc(): Long = kept!!.inc()

    fun listed(items: Any?): String {
        val list = items as List<*>
        val data = (list[0] as Map<*, *>)["data"]
        // Inside data the object is a RemoteObject; the event compares with it.
        listedObject = list[1] as? RemoteObject
        return "{\"data\":$data} ${listedObject != null}"
    }

    fun back(): Counted = kept!!

    suspend fun rust(): Long = poker.poke(kept!!)

    suspend fun nope(): String = try {
        kept!!.nope()
        "answered"
    } catch (e: RutisException) {
        said(e)
    }

    suspend fun plain(thing: Pinged): String = thing.ping()

    suspend fun after(): String = try {
        kept!!.inc().toString()
    } catch (e: RutisException) {
        said(e)
    }
}

val ktHolder = Rutis.define<Rutis.NoConfig>(name = "kt_holder", inject = listOf("poker", "probe"), provides = mapOf("holder" to service<Holder>())) { ctx, _ ->
    val holder = Holder(ctx.use<Poker>("poker"))
    val probe = ctx.use<Recorder>("probe")
    ctx.on("counter") { args ->
        val counter = args[0] as RemoteObject
        val value = counter.call("value")
        probe.record("event: ${counter === holder.listedObject} $value")
        null
    }
    ctx.provide("holder", holder)
}
