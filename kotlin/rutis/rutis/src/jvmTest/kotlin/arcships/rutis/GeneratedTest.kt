package arcships.rutis

import arcships.rutis.internal.Native
import arcships.rutis.internal.peer.Exported
import arcships.rutis.internal.peer.WireSet
import kotlinx.coroutines.runBlocking
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.JsonPrimitive
import java.time.LocalDate
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertNotNull
import kotlin.test.assertSame

@Serializable
data class Point(val x: Int, val y: Int)

open class Base {
    fun inherited(): String = "base"
}

/** A service: its public functions are its methods, its superclass's included. */
@RutisService
class Shapes : Base() {
    var calls = 0

    fun area(p: Point): Long = (p.x * p.y).toLong()

    suspend fun later(days: List<LocalDate>): Set<LocalDate> = days.toSet()

    fun each(names: List<String>, transform: (String) -> String): List<String> = names.map(transform)

    fun echo(value: Any?): Any? = value

    fun adder(n: Int): (Int) -> Int = { it + n }

    fun touch() {
        calls++
    }

    @Suppress("unused")
    private fun hidden() = Unit

    val notAMethod: String get() = "property"
}

/** A client: suspend functions suspend, plain ones block. */
@RutisClient
interface ShapesClient {
    fun area(p: Point): Long

    suspend fun later(days: List<LocalDate>): Set<LocalDate>

    fun each(names: List<String>, transform: (String) -> String): List<String>

    fun touch()
}

/** The processor's method tables and proxies (design §3.1): found by name, no reflection on methods. */
class GeneratedTest {
    private val table = assertNotNull(Tables.serviceTable(Shapes::class.java))

    @Test
    fun theTableListsPublicMemberFunctions() {
        assertEquals(setOf("inherited", "area", "later", "each", "echo", "adder", "touch"), table.methods.toSet())
    }

    @Test
    fun theTableDecodesArgumentsAndEncodesResults() = runBlocking<Unit> {
        val shapes = Shapes()
        assertEquals(6L, table.invoke(shapes, "area", listOf(mapOf("x" to JsonPrimitive(2), "y" to JsonPrimitive(3)))))
        val day = LocalDate.of(2026, 10, 15)
        assertEquals(WireSet(listOf(day)), table.invoke(shapes, "later", listOf(listOf(day, day))))
        assertEquals("base", table.invoke(shapes, "inherited", emptyList()))
        // A function argument from this process is the function itself.
        val upper = Exported({ s: String -> s.uppercase() }) { error("not called through the wire") }
        assertEquals(listOf("A"), table.invoke(shapes, "each", listOf(listOf("a"), upper)))
        // A function result is exported, called with session values.
        val adder = table.invoke(shapes, "adder", listOf(JsonPrimitive(2))) as Exported
        assertEquals(5L, adder.body(listOf(JsonPrimitive(3))))
        assertEquals(null, table.invoke(shapes, "touch", emptyList()))
        assertEquals(1, shapes.calls)
        assertFailsWith<RutisException> { table.invoke(shapes, "notAMethod", emptyList()) }
    }

    @Test
    fun theProxyCallsThroughItsHandle() = runBlocking<Unit> {
        val shapes = Shapes()
        val factory = assertNotNull(Tables.clientFactory(ShapesClient::class.java))
        assertEquals(listOf("area", "later", "each", "touch"), factory.methods)
        val client = factory.create(object : ServiceHandle {
            override val name = "shapes"
            override val kinds = table.methods.associateWith { "async" }
            override suspend fun callSuspend(method: String, args: List<Any?>) = table.invoke(shapes, method, args)
            override fun callBlocking(method: String, args: List<Any?>) = runBlocking { table.invoke(shapes, method, args) }
        })
        assertEquals(12L, client.area(Point(3, 4)))
        val day = LocalDate.of(2026, 10, 15)
        assertEquals(setOf(day), client.later(listOf(day)))
        assertEquals(listOf("AB"), client.each(listOf("ab")) { it.uppercase() })
        client.touch()
        assertEquals(1, shapes.calls)
        // A service object crosses by reference.
        val exported = Native.fromNative(shapes)
        assertSame(shapes, (exported as arcships.rutis.internal.peer.ExportedObject).original)
    }
}
