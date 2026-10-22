package arcships.rutis

import arcships.rutis.internal.Schema
import arcships.rutis.testing.Harness
import arcships.rutis.testing.RutisTest
import kotlinx.coroutines.delay
import kotlinx.coroutines.runBlocking
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.Json
import java.util.Collections
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertFalse
import kotlin.test.assertTrue

@Serializable
data class Settings(
    val city: String,
    val days: Int = 3,
    val units: Units = Units.Metric,
    val alerts: List<String> = emptyList(),
    val home: Place? = null,
)

@Serializable
enum class Units { Metric, Imperial }

@Serializable
data class Place(val lat: Double, val lon: Double)

@RutisClient
interface Greeter {
    fun greet(name: String): String

    suspend fun greetLater(name: String): String
}

@RutisService
class Hello(private val prefix: String) : Greeter {
    override fun greet(name: String): String = "$prefix $name"

    override suspend fun greetLater(name: String): String = "$prefix $name, later"
}

/** A test service the plugin injects. */
@RutisService
class Journal {
    val lines: MutableList<String> = Collections.synchronizedList(ArrayList())

    fun record(line: String) {
        lines.add(line)
    }
}

@RutisClient
interface JournalClient {
    fun record(line: String)
}

/** The plugin API (design §3) through the testing tool: a real session, the test as host. */
class RuntimeTest {
    @Test
    fun theSchemaIsDerivedFromTheConfigType() {
        val schema = Json.parseToJsonElement(Schema.of(Settings.serializer().descriptor).toString())
        val expected = Json.parseToJsonElement(
            """{"type":"object","properties":{"city":{"type":"string"},"days":{"type":"integer"},
            "units":{"type":"string","enum":["Metric","Imperial"]},"alerts":{"type":"array","items":{"type":"string"}},
            "home":{"type":["object","null"],"properties":{"lat":{"type":"number"},"lon":{"type":"number"}},"required":["lat","lon"]}},
            "required":["city"]}""",
        )
        assertEquals(expected, schema)
    }

    @Test
    fun aPluginProvidesUsesAndCleansUp() = runBlocking<Unit> {
        val journal = Journal()
        val plugin = Rutis.define<Settings>(
            name = "hello",
            inject = listOf("journal"),
            provides = mapOf("hello" to service<Hello>(sync = setOf("greet"))),
        ) { ctx, config ->
            val log = ctx.use<JournalClient>("journal")
            val hello = Hello("hi from ${config.city}")
            ctx.provide("hello", hello)
            ctx.provide("extra", Hello("extra"))
            assertFailsWith<IllegalArgumentException> { ctx.use<Greeter>("undeclared") }
            ctx.effect { log.record("first cleanup") }
            ctx.effect { log.record("second cleanup") }
            log.record("applied ${config.days}")
        }
        RutisTest.load(plugin, Settings("Oslo"), services = mapOf("journal" to journal)).use { loaded ->
            val hello = loaded.service("hello")
            assertEquals(mapOf("greet" to "sync", "greetLater" to "async"), hello.methods)
            assertEquals("hi from Oslo Kim", hello.call("greet", "Kim"))
            assertEquals("hi from Oslo Kim, later", hello.bind<Greeter>().greetLater("Kim"))
            // Not declared: announced, every method async.
            assertEquals(mapOf("greet" to "async", "greetLater" to "async"), loaded.service("extra").methods)
        }
        assertEquals(listOf("applied 3", "second cleanup", "first cleanup"), journal.lines)
    }

    @Test
    fun aTaskOfAnUnloadedRowCannotProvideOrRegisterACleanup() = runBlocking<Unit> {
        var late: Ctx? = null
        val plugin = Rutis.define<Rutis.NoConfig>(
            name = "late",
            provides = mapOf("hello" to service<Hello>(sync = setOf("greet"))),
        ) { ctx, _ ->
            late = ctx
            ctx.provide("hello", Hello("row"))
        }
        RutisTest.load(plugin).use { first ->
            first.unload()
            late!!.provide("hello", Hello("late"))
            late!!.effect { }
            val again = first.load(isolate = mapOf("hello" to "again"))
            assertEquals("row Kim", again.service("hello").call("greet", "Kim"))
        }
    }

    @Test
    fun disposeGivesEveryRowTheSameBudget() = runBlocking<Unit> {
        val plugin = Rutis.define<Rutis.NoConfig>(name = "slow-cleanups") { ctx, _ ->
            ctx.effect { delay(5_000) }
        }
        val harness = Harness(plugin, emptyMap(), "kotlin")
        harness.start()
        try {
            harness.load(null, null, emptyList())
            harness.load(null, null, emptyList())
            val started = System.nanoTime()
            val finished = harness.dispose(200)
            val elapsed = (System.nanoTime() - started) / 1_000_000
            assertFalse(finished)
            assertTrue(elapsed < 2_000, "dispose took ${elapsed}ms for two rows")
        } finally {
            harness.close()
        }
    }

    @Test
    fun everyRowGetsItsDeclaredServiceSlots() = runBlocking<Unit> {
        val plugin = Rutis.define<Rutis.NoConfig>(
            name = "rows",
            provides = mapOf("hello" to service<Hello>(sync = setOf("greet"))),
        ) { ctx, _ ->
            ctx.provide("hello", Hello("row"))
        }
        RutisTest.load(plugin).use { first ->
            val second = first.load(isolate = mapOf("hello" to "second"))
            val hello = second.service("hello")
            assertEquals(mapOf("greet" to "sync", "greetLater" to "async"), hello.methods)
            assertEquals("row Kim", hello.call("greet", "Kim"))
        }
    }

    @Test
    fun aSecondRowWithoutItsOwnLabelCannotProvideTheSameService() = runBlocking<Unit> {
        val plugin = Rutis.define<Rutis.NoConfig>(
            name = "rows-unisolated",
            provides = mapOf("hello" to service<Hello>(sync = setOf("greet"))),
        ) { ctx, _ ->
            ctx.provide("hello", Hello("row"))
        }
        RutisTest.load(plugin).use { first ->
            assertFailsWith<RutisException> { first.load() }
        }
    }

    @Test
    fun listenersHearEmissionsInEveryMode() = runBlocking<Unit> {
        val plugin = Rutis.define<Rutis.NoConfig>(name = "events") { ctx, _ ->
            ctx.on("t/greet") { args -> "${args[0]}!" }
            ctx.on("t/format") { args -> "<" + (args.next!!() ?: args[0]) + ">" }
            ctx.on("t/check", prepend = true) { args -> if (args[0] == 42L) "yes" else null }
            ctx.on("t/check") { _ -> "fallback" }
            ctx.on("t/ask") { _ -> ctx.serial("t/greet", "inner") }
            assertFailsWith<RutisException> { ctx.emit("rutis/row", 1) }.also { assertEquals(RutisException.RESERVED_EVENT, it.name) }
        }
        RutisTest.load(plugin).use { loaded ->
            assertEquals("hi!", loaded.serial("t/greet", "hi"))
            assertEquals("<HI>", loaded.waterfall("t/format", "hi", terminal = { args -> (args[0] as String).uppercase() }))
            // No terminal: `next` has no result.
            assertEquals("<hi>", loaded.waterfall("t/format", "hi"))
            assertEquals("yes", loaded.bail("t/check", 42))
            assertEquals("fallback", loaded.bail("t/check", 1))
            assertEquals("inner!", loaded.serial("t/ask"))
            val seen = Collections.synchronizedList(ArrayList<String>())
            val off = loaded.on("t/greet") { args -> seen.add(args[0] as String); null }
            loaded.parallel("t/greet", "all")
            assertEquals(listOf("all"), seen)
            off()
            // A filter that keeps none: no listener runs.
            assertEquals(null, loaded.filter { false }.serial("t/greet", "x"))
            assertTrue(loaded.reported().isEmpty(), "${loaded.reported()}")
        }
    }

    @Test
    fun failuresOfParallelAggregateInRegistrationOrder() = runBlocking<Unit> {
        val plugin = Rutis.define<Rutis.NoConfig>(name = "failing") { ctx, _ ->
            ctx.on("t/fail") { _ -> throw IllegalStateException("one") }
            ctx.on("t/fail") { _ -> throw RutisException("Custom", "two", code = "E2") }
        }
        RutisTest.load(plugin).use { loaded ->
            val error = assertFailsWith<RutisException> { loaded.parallel("t/fail") }
            assertEquals(RutisException.AGGREGATE_ERROR, error.name)
            assertEquals("2 listeners of t/fail failed", error.message)
            assertEquals(listOf("IllegalStateException", "Custom"), error.errors.map { (it as RutisException).name })
            assertEquals("E2", (error.errors[1] as RutisException).code)
        }
    }
}
