package arcships.rutis

import arcships.rutis.advanced.manifest
import arcships.rutis.internal.RuntimeState
import arcships.rutis.testing.RutisTest
import kotlinx.coroutines.runBlocking
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonPrimitive
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertIs
import kotlin.test.assertNull
import kotlin.test.assertTrue

/** A test service a plugin uses optionally. */
@RutisService
class FakeLlm {
    fun ask(question: String): String = "re: $question"
}

@RutisClient
interface Llm {
    fun ask(question: String): String
}

/** A proxy calling a method the service lacks. */
@RutisClient
interface Chatty {
    fun ask(question: String): String

    fun chat(line: String): String
}

/** Optional services (docs/design-optional-deps-2026-10-28.md): `optional`, `ctx.get`, and `use` counting them as declared. */
class OptionalDepsTest {
    // guarantee: sdk/optional-deps (declared beside inject, `[]` when left
    // out, reported by rows.schema and --rutis-manifest)
    @Test
    fun optionalIsDeclaredAndReported() {
        val plugin = Rutis.define<Rutis.NoConfig>(name = "summary", inject = listOf("notes"), optional = listOf("llm")) { _, _ -> }
        val bare = Rutis.define<Rutis.NoConfig>(name = "bare") { _, _ -> }
        assertEquals(listOf("llm"), plugin.optional)
        assertEquals(emptyList<String>(), bare.optional)
        val rt = RuntimeState(listOf(plugin, bare))
        assertEquals(listOf("notes"), rt.describe(plugin)["inject"])
        assertEquals(listOf("llm"), rt.describe(plugin)["optional"])
        assertEquals(emptyList<String>(), rt.describe(bare)["optional"])
        // As --rutis-manifest prints it.
        val manifest = Json.parseToJsonElement(Rutis.manifest(plugin, bare)).toString()
        assertTrue(manifest.contains(""""optional":["llm"]"""), manifest)
        assertTrue(manifest.contains(""""optional":[]"""), manifest)
    }

    // guarantee: sdk/optional-deps (`get`: null when absent, the service when
    // there; `use` reads only what `inject` lists, as Rust's `require`, so an
    // optional name is told to go there; `get` of an undeclared name says
    // not-declared with {service})
    @Test
    fun getReadsAnOptionalServiceAndUseDoesNot() = runBlocking<Unit> {
        val seen = LinkedHashMap<String, Any?>()
        val plugin = Rutis.define<Rutis.NoConfig>(name = "summary", optional = listOf("llm")) { ctx, _ ->
            seen["absent"] = ctx.get<Llm>("llm")
            seen["use optional"] = assertFailsWith<IllegalArgumentException> { ctx.use<Llm>("llm") }
            seen["get undeclared"] = assertFailsWith<IllegalArgumentException> { ctx.get<Llm>("other") }
            seen["use undeclared"] = assertFailsWith<IllegalArgumentException> { ctx.use<Llm>("other") }
            // One this row provides, declared by providing it: the object
            // itself, as `use` gives it; withdrawn, it is like a name never
            // declared.
            val local = Hello("hi")
            val withdraw = ctx.provide("local", local)
            seen["local"] = ctx.get<Greeter>("local") === local
            withdraw()
            seen["use withdrawn"] = assertFailsWith<NotDeclaredException> { ctx.use<Greeter>("local") }.message
            seen["get withdrawn"] = assertFailsWith<NotDeclaredException> { ctx.get<Greeter>("local") }.message
        }
        val rt = RuntimeState(listOf(plugin))
        rt.load("row:1.0", "summary", plugin.name, null, emptyMap(), emptyMap(), emptyList())
        assertNull(seen["absent"])
        val optional = assertIs<NotDeclaredException>(seen["use optional"])
        assertEquals(
            "service llm is not available to this plugin, which does not declare it in inject: add 'llm' to its inject",
            optional.message,
        )
        assertEquals("not-declared", optional.code)
        assertEquals(mapOf("service" to "llm"), optional.data)
        val undeclared = assertIs<NotDeclaredException>(seen["get undeclared"])
        assertEquals(
            "service other is not available to this plugin, which does not declare it: add 'other' to its optional (or to its inject, if it cannot run without it)",
            undeclared.message,
        )
        assertEquals("not-declared", undeclared.code)
        assertEquals(mapOf("service" to "other"), undeclared.data)
        // `use` keeps its line for a truly undeclared name.
        assertEquals(
            "service other is not available to this plugin, which does not declare it in inject: add 'other' to its inject",
            assertIs<NotDeclaredException>(seen["use undeclared"]).message,
        )
        assertEquals(true, seen["local"])
        assertEquals(
            "service local is not available to this plugin, which does not declare it in inject: add 'local' to its inject",
            seen["use withdrawn"],
        )
        assertEquals(
            "service local is not available to this plugin, which does not declare it: add 'local' to its optional (or to its inject, if it cannot run without it)",
            seen["get withdrawn"],
        )
    }

    // guarantee: sdk/optional-deps (a given optional service is found by `get`,
    // not by `use`; typed `get` checks methods as `use` does, saying `get`)
    @Test
    fun aGivenOptionalServiceIsFoundAndItsMethodsChecked() {
        val seen = LinkedHashMap<String, Any?>()
        val plugin = Rutis.define<Rutis.NoConfig>(name = "summary", optional = listOf("llm")) { ctx, _ ->
            seen["get"] = ctx.get<Llm>("llm")?.ask("get")
            seen["use"] = assertFailsWith<NotDeclaredException> { ctx.use<Llm>("llm") }.code
            seen["missing"] = assertFailsWith<IllegalArgumentException> { ctx.get<Chatty>("llm") }.message
        }
        RutisTest.load(plugin, services = mapOf("llm" to FakeLlm())).close()
        assertEquals("re: get", seen["get"])
        assertEquals("not-declared", seen["use"])
        assertEquals("get llm: Chatty calls chat, which the service does not have", seen["missing"])
    }

    // guarantee: sdk/optional-deps (RutisTest: an optional service need not be
    // given; a name in both lists fails before the plugin loads)
    @Test
    fun theTestingToolTakesOptionalServicesGivenOrNot() {
        var got: Any? = "unset"
        val plugin = Rutis.define<Rutis.NoConfig>(name = "summary", optional = listOf("llm")) { ctx, _ ->
            got = ctx.get<Llm>("llm")
        }
        RutisTest.load(plugin).close()
        assertNull(got)
        var applied = false
        val both = Rutis.define<Rutis.NoConfig>(name = "both", inject = listOf("llm"), optional = listOf("llm")) { _, _ -> applied = true }
        // The host refuses the row, saying why.
        val error = assertFailsWith<Throwable> { RutisTest.load(both, services = mapOf("llm" to FakeLlm())) }
        assertTrue(error.message.orEmpty().endsWith(": llm is in both inject and optional; keep it in one"), error.message)
        assertEquals(false, applied)
    }

    // guarantee: sdk/optional-deps (a handle its row stopped, kept until
    // rutis releases it, says unavailable to calls and property reads)
    @Test
    fun aRetiredHandleIsNotAvailable() = runBlocking<Unit> {
        val plugin = Rutis.define<Rutis.NoConfig>(name = "hello") { ctx, _ -> ctx.provide("hello", Hello("hi")) }
        val rt = RuntimeState(listOf(plugin))
        rt.load("row:1.0", "hello", plugin.name, null, mapOf("hello" to mapOf("greet" to "sync")), emptyMap(), emptyList())
        val handle = rt.slots["hello"]!!.handle!!
        assertEquals("hi Kim", rt.dispatch(handle, "greet", listOf(JsonPrimitive("Kim"))))
        rt.unload("row:1.0")
        for (error in listOf(
            assertFailsWith<RutisException> { rt.dispatch(handle, "greet", listOf(JsonPrimitive("Kim"))) },
            assertFailsWith<RutisException> { rt.property(handle, "prefix") },
        )) {
            assertEquals(RutisException.BINDING_ERROR, error.name)
            assertEquals("service hello is not available", error.message)
            assertEquals("unavailable", error.code)
            assertEquals(mapOf("service" to "hello"), error.data)
        }
        // Released: forgotten.
        rt.released(handle)
        assertNull(rt.handles[handle])
    }

    // guarantee: sdk/optional-deps (one its row replaced while running keeps its object until the
    // row unloads, as a snapshot of a service does in Rust)
    @Test
    fun aHandleItsRowReplacedKeepsItsObjectUntilTheRowUnloads() = runBlocking<Unit> {
        var swap: () -> Unit = {}
        val plugin = Rutis.define<Rutis.NoConfig>(name = "hello") { ctx, _ ->
            var withdraw = ctx.provide("hello", Hello("hi"))
            swap = { withdraw(); withdraw = ctx.provide("hello", Hello("yo")) }
        }
        val rt = RuntimeState(listOf(plugin))
        rt.load("row:1.0", "hello", plugin.name, null, mapOf("hello" to mapOf("greet" to "sync")), emptyMap(), emptyList())
        val old = rt.slots["hello"]!!.handle!!
        swap()
        val now = rt.slots["hello"]!!.handle!!
        assertTrue(now != old, "the new object has a handle of its own")
        assertEquals("yo Kim", rt.dispatch(now, "greet", listOf(JsonPrimitive("Kim"))))
        assertEquals("hi Kim", rt.dispatch(old, "greet", listOf(JsonPrimitive("Kim"))))
        rt.unload("row:1.0")
        val gone = assertFailsWith<RutisException> { rt.dispatch(old, "greet", listOf(JsonPrimitive("Kim"))) }
        assertEquals("service hello is not available", gone.message)
    }

    // guarantee: sdk/optional-deps (a row loaded again gives its service a handle of its own: one a
    // caller still holds for the object before stays retired, rather than reaching the new one)
    @Test
    fun aServiceOfARowLoadedAgainHasANewHandle() = runBlocking<Unit> {
        val plugin = Rutis.define<Rutis.NoConfig>(name = "hello") { ctx, _ -> ctx.provide("hello", Hello("hi")) }
        val rt = RuntimeState(listOf(plugin))
        val exports = mapOf("hello" to mapOf("greet" to "sync"))
        rt.load("row:1.0", "hello", plugin.name, null, exports, emptyMap(), emptyList())
        val old = rt.slots["hello"]!!.handle!!
        rt.unload("row:1.0")
        rt.load("row:1.0", "hello", plugin.name, null, exports, emptyMap(), emptyList())
        val now = rt.slots["hello"]!!.handle!!
        assertTrue(now != old, "the new object has a handle of its own")
        assertEquals("hi Kim", rt.dispatch(now, "greet", listOf(JsonPrimitive("Kim"))))
        val gone = assertFailsWith<RutisException> { rt.dispatch(old, "greet", listOf(JsonPrimitive("Kim"))) }
        assertEquals("service hello is not available", gone.message)
    }

    // guarantee: sdk/optional-deps (a row's own inject counts as declared:
    // the plugin uses it without listing it, and it is unavailable rather
    // than not-declared when nothing provides it)
    @Test
    fun aRowsInjectCountsAsDeclared() = runBlocking<Unit> {
        val seen = LinkedHashMap<String, Any?>()
        val plugin = Rutis.define<Rutis.NoConfig>(name = "reader") { ctx, _ ->
            seen["use"] = ctx.use<Greeter>("row").greet("Kim")
            seen["use absent"] = assertFailsWith<UnavailableException> { ctx.use<Greeter>("search") }
            seen["use undeclared"] = assertFailsWith<NotDeclaredException> { ctx.use<Greeter>("other") }
        }
        val provider = Rutis.define<Rutis.NoConfig>(name = "provider") { ctx, _ -> ctx.provide("row", Hello("hi")) }
        val rt = RuntimeState(listOf(plugin, provider))
        rt.load("row:1.0", "provider", provider.name, null, mapOf("row" to mapOf("greet" to "sync")), emptyMap(), emptyList())
        rt.load("row:2.0", "reader", plugin.name, null, emptyMap(), emptyMap(), emptyList(), listOf("row", "search"))
        assertEquals("hi Kim", seen["use"])
        assertEquals("service search is not available", assertIs<UnavailableException>(seen["use absent"]).message)
        // Nothing declares `other`: said whether or not this process has one.
        assertEquals(
            "service other is not available to this plugin, which does not declare it in inject: add 'other' to its inject",
            assertIs<NotDeclaredException>(seen["use undeclared"]).message,
        )
    }

    // guarantee: sdk/optional-deps (the row's `inject` is rows.load's fifth
    // argument, as rutis sends it, and a restarted row keeps it)
    @Test
    fun aRowsInjectArrivesWithRowsLoad() = runBlocking<Unit> {
        val seen = ArrayList<List<String?>>()
        val plugin = Rutis.define<Rutis.NoConfig>(name = "reader") { ctx, _ ->
            seen += listOf(
                ctx.use<Greeter>("row").greet("Kim"),
                assertFailsWith<UnavailableException> { ctx.use<Greeter>("search") }.message,
                assertFailsWith<NotDeclaredException> { ctx.use<Greeter>("other") }.message,
                ctx.get<Greeter>("row")?.greet("Kim"),
            )
        }
        val provider = Rutis.define<Rutis.NoConfig>(name = "provider") { ctx, _ -> ctx.provide("row", Hello("hi")) }
        val rt = RuntimeState(listOf(plugin, provider))
        rt.load("row:1.0", "provider", provider.name, null, mapOf("row" to mapOf("greet" to "sync")), emptyMap(), emptyList())
        val placed = mapOf("scope" to emptyList<Any?>(), "id" to "reader")
        rt.dispatch("", "rows.load", listOf("row:2.0", plugin.name, null, emptyList<Any?>(), listOf("row", "search"), emptyMap<String, Any?>(), placed))
        rt.dispatch("", "rows.update", listOf("row:2.0", null))
        val expected = listOf(
            "hi Kim",
            "service search is not available",
            "service other is not available to this plugin, which does not declare it in inject: add 'other' to its inject",
            "hi Kim",
        )
        assertEquals<List<List<String?>>>(listOf(expected, expected), seen)
    }
}
