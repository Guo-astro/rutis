package arcships.rutis

import arcships.rutis.internal.RuntimeState
import arcships.rutis.internal.peer.Channel
import arcships.rutis.internal.peer.Options
import arcships.rutis.internal.peer.Peer
import arcships.rutis.internal.peer.encodeError
import kotlinx.coroutines.runBlocking
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.jsonArray
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import java.util.concurrent.CompletableFuture
import java.util.concurrent.LinkedBlockingQueue
import java.util.concurrent.TimeUnit
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertIs
import arcships.rutis.generated.ServiceObject
import arcships.rutis.generated.Tables

/** #677: what this runtime refuses says so with what the developer wrote, a code and data. */
class ErrorTextTest {
    private val table = Tables.serviceTable(Shapes::class.java)!!

    // guarantee: sdk/error-texts (a call the Kotlin runtime refuses names the
    // service, method and 1-based argument, code no-method or bad-argument)
    @Test
    fun aRefusedCallNamesTheServiceMethodAndArgument() = runBlocking<Unit> {
        val shapes = Shapes()
        var error = assertFailsWith<RutisException> { table.serve(shapes, "aera", emptyList(), "shapes") }
        assertEquals("shapes has no method aera (methods: adder, area, each, echo, inherited, later, touch)", error.message)
        assertEquals(RutisException.BINDING_ERROR, error.name)
        assertEquals("no-method", error.code)
        assertEquals(mapOf("service" to "shapes", "method" to "aera", "methods" to listOf("adder", "area", "each", "echo", "inherited", "later", "touch")), error.data)

        error = assertFailsWith<RutisException> { table.serve(shapes, "adder", listOf(JsonPrimitive("x")), "shapes") }
        assertEquals("shapes.adder: argument 1 should be an integer, got \"x\"", error.message)
        assertEquals("bad-argument", error.code)
        assertEquals(mapOf("service" to "shapes", "method" to "adder", "argument" to 1, "expected" to "an integer"), error.data)

        error = assertFailsWith<RutisException> { table.serve(shapes, "area", emptyList(), "shapes") }
        assertEquals("shapes.area: argument 1 is missing", error.message)
        assertEquals("bad-argument", error.code)
        error = assertFailsWith<RutisException> { table.serve(shapes, "area", listOf(JsonPrimitive(3)), "shapes") }
        assertEquals("shapes.area: argument 1 should be a Point, got 3", error.message)
        error = assertFailsWith<RutisException> { table.serve(shapes, "each", listOf(listOf("a"), JsonPrimitive(1)), "shapes") }
        assertEquals("shapes.each: argument 2 should be a function, got 1", error.message)
    }

    // guarantee: sdk/error-texts (an object passed by reference refuses a
    // method it lacks naming its type)
    @Test
    fun anObjectRefusesAMethodByItsTypeName() = runBlocking<Unit> {
        val error = assertFailsWith<RutisException> { ServiceObject(Shapes(), table).call("gett", emptyList()) }
        assertEquals("Shapes has no method gett (methods: adder, area, each, echo, inherited, later, touch)", error.message)
        assertEquals("no-method", error.code)
        assertEquals("Shapes", (error.data as Map<*, *>)["object"])
    }

    // guarantee: sdk/undeclared-use (codes: not-declared, unavailable, each
    // with {service}; still an IllegalArgumentException / IllegalStateException)
    @Test
    fun useSaysWhyAServiceIsNotThere() = runBlocking<Unit> {
        val seen = mutableListOf<Throwable>()
        val plugin = Rutis.define<Rutis.NoConfig>(name = "sneaky", inject = listOf("weather")) { ctx, _ ->
            seen += assertFailsWith<IllegalArgumentException> { ctx.use<JournalClient>("llm") }
            seen += assertFailsWith<IllegalStateException> { ctx.use<JournalClient>("weather") }
        }
        val rt = RuntimeState(listOf(plugin))
        rt.load("row:1.0", "sneaky", plugin.name, null, emptyMap(), emptyMap(), emptyList())
        val undeclared = assertIs<NotDeclaredException>(seen[0])
        assertEquals("service llm is not available to this plugin, which does not declare it in inject: add 'llm' to its inject", undeclared.message)
        assertEquals("not-declared", undeclared.code)
        assertEquals(mapOf("service" to "llm"), undeclared.data)
        val unavailable = assertIs<UnavailableException>(seen[1])
        assertEquals("service weather is not available", unavailable.message)
        assertEquals("unavailable", unavailable.code)
        assertEquals(mapOf("service" to "weather"), unavailable.data)
        // They cross with their code and data.
        val graph = encodeError(undeclared)
        val node = graph["graph"]!!.jsonObject["nodes"]!!.jsonArray[0].jsonObject
        assertEquals("not-declared", node["code"]!!.jsonPrimitive.content)
    }

    // guarantee: sdk/error-texts (a call once the host went fails with `the
    // host disconnected`, code peer-gone, crossing as a BindingError)
    @Test
    fun aCallFailsSayingTheHostDisconnected() {
        val ended = EndingChannel()
        val runtime = Peer(ended, Options(dispatch = { _, _, _ -> null }))
        runtime.start()
        ended.end()
        runtime.closedFuture.get(5, TimeUnit.SECONDS)
        val error = assertFailsWith<HostGoneException> { runtime.call("", "wait", emptyList(), false) }
        assertEquals("the host disconnected", error.message)
        assertEquals("peer-gone", error.code)
        assertEquals(mapOf("reason" to "disconnected"), error.data)
        val encoded: JsonObject = encodeError(error)
        assertEquals("BindingError", encoded["name"]!!.jsonPrimitive.content)
    }
}

/** A channel whose far end goes at once. */
private class EndingChannel : Channel {
    private val inbox = LinkedBlockingQueue<CompletableFuture<ByteArray?>>()
    fun end() = inbox.put(CompletableFuture.completedFuture(null))
    override fun send(message: ByteArray) {}
    override fun recv(): ByteArray? = inbox.take().get()
    override fun close(reason: String) {}
}
