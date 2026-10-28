package arcships.rutis

import arcships.rutis.internal.RuntimeState
import arcships.rutis.internal.peer.Endpoint
import arcships.rutis.internal.peer.MemoryChannel
import arcships.rutis.internal.peer.Options
import arcships.rutis.internal.peer.Peer
import arcships.rutis.internal.peer.RemotePlugin
import arcships.rutis.internal.peer.Undefined
import arcships.rutis.internal.peer.ValueException
import arcships.rutis.testing.RutisTest
import kotlinx.coroutines.runBlocking
import kotlinx.serialization.Serializable
import java.util.Collections
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicReference
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertFalse
import kotlin.test.assertIs
import kotlin.test.assertSame
import kotlin.test.assertTrue
import arcships.rutis.generated.Tables

@Serializable
data class Depth(val depth: Int)

@RutisClient
interface PartsClient {
    suspend fun register(scope: String, definition: Plugin)

    suspend fun echo(definition: Plugin): Plugin
}

/** A test double of a part registry: keeps what it is handed. */
@RutisService
class HandedParts {
    val handed: MutableList<Pair<String, Plugin>> = Collections.synchronizedList(ArrayList())

    fun register(scope: String, definition: Plugin) {
        handed.add(scope to definition)
    }

    /** Hands the definition back. */
    fun echo(definition: Plugin): Plugin = definition

    /** A nullable plugin parameter and result. */
    fun maybe(definition: Plugin?): Plugin? = definition
}

@RutisService
class TagService(private val line: String) {
    fun text(): String = line
}

/**
 * Plugin definitions as values (docs/design-plugin-values-2026-10-25.md): a
 * `Rutis.define` result crosses as a `plugin` reference, only to a far end
 * that takes plugin definitions; back home it is the definition itself; the
 * runtime describes it (`plugins.read`) and loads it as a row's entry, each
 * copy with its config; RutisTest hands a test double the definition itself,
 * so the test loads it as the host would mount it (R10). The cross-language
 * cases: crates/rutis-loader/tests/plugin_values.rs.
 */
class PluginValuesTest {
    @Test
    fun theRuntimeTakesDefinitionsBack() = runBlocking<Unit> {
        val part = Rutis.define<Depth>(name = "part", inject = listOf("history")) { _, _ -> }
        val rt = RuntimeState(emptyList())
        @Suppress("UNCHECKED_CAST")
        val described = rt.dispatch("", "plugins.read", listOf(part)) as Map<String, Any?>
        assertEquals("part", described["name"])
        assertEquals(listOf("history"), described["inject"])
        assertEquals(true, described["announces"])
        val refused = assertFailsWith<IllegalArgumentException> { rt.dispatch("", "plugins.read", listOf(42L)) }
        assertTrue(refused.message!!.contains("not a plugin definition"), refused.message)
        @Suppress("UNCHECKED_CAST")
        val mounted = rt.dispatch("", "mount", listOf(emptyMap<String, Any?>())) as Map<String, Any?>
        assertTrue("plugin-values" in (mounted["features"] as List<*>))
    }

    /** Each copy loads with its own config and leaves with its row; the runtime need not list it. */
    @Test
    fun eachCopyLoadsWithItsConfig() = runBlocking<Unit> {
        val lines = Collections.synchronizedList(ArrayList<String>())
        val part = Rutis.define<Depth>(name = "part") { ctx, config ->
            lines.add("up ${config.depth}")
            ctx.effect { lines.add("down ${config.depth}") }
        }
        val rt = RuntimeState(emptyList())
        for ((key, depth) in listOf("c1" to 1L, "c2" to 2L)) {
            rt.dispatch("", "rows.load", listOf(key, part, mapOf("depth" to depth), emptyList<Any?>(), emptyList<Any?>(), null))
        }
        assertEquals(listOf("up 1", "up 2"), lines.toList())
        rt.dispatch("", "rows.unload", listOf("c1"))
        assertEquals(listOf("up 1", "up 2", "down 1"), lines.toList())
        rt.dispatch("", "rows.unload", listOf("c2"))
        assertEquals(listOf("up 1", "up 2", "down 1", "down 2"), lines.toList())
    }

    /** Sent home it is the same definition; a far end that does not take them fails only that call. */
    @Test
    fun aDefinitionGoesHomeAsItself() = runBlocking<Unit> {
        val part = Rutis.define<Rutis.NoConfig>(name = "part") { _, _ -> }
        val back = AtomicReference<Any?>()
        val (a, b) = MemoryChannel.pair()
        val runtime = Peer(a, Options(dispatch = { _, method, args ->
            when (method) {
                "give" -> part
                "take" -> {
                    back.set(args.firstOrNull())
                    Undefined
                }
                else -> throw IllegalArgumentException("no $method")
            }
        }, endpoint = Endpoint("kotlin", "main")))
        val host = Peer(b, Options(endpoint = Endpoint("main", "kotlin"), plugins = true))
        runtime.start()
        host.start()
        host.readyFuture.get(5, TimeUnit.SECONDS)
        runtime.readyFuture.get(5, TimeUnit.SECONDS)
        try {
            val given = host.callSuspend("t", "give", emptyList(), false)
            assertIs<RemotePlugin>(given)
            // The same value handed over twice is one reference.
            val twice = host.callSuspend("t", "give", emptyList(), false)
            assertIs<RemotePlugin>(twice)
            assertEquals(given.record.id, twice.record.id)
            assertSame(given, twice)
            host.callSuspend("t", "take", listOf(given), false)
            assertSame(part, back.get())
            // The runtime's far end (the test host) takes them; the host's does not.
            assertTrue(runtime.supports("plugins"))
            assertFalse(host.supports("plugins"))
            val refused = assertFailsWith<ValueException> { host.callSuspend("t", "take", listOf(part), false) }
            assertTrue(refused.message!!.contains("does not accept plugin definitions"), refused.message)
            assertIs<RemotePlugin>(host.callSuspend("t", "give", emptyList(), false))
        } finally {
            host.shutdown("done")
        }
    }

    /** A definition handed to a test double is the definition itself: the test loads it, each copy with its config (R10). */
    @Test
    fun aDefinitionHandedToATestDoubleLoads() = runBlocking<Unit> {
        val prepared = "prepared in apply"
        val parts = HandedParts()
        val made = AtomicReference<Plugin>()
        val origin = Rutis.define<Rutis.NoConfig>(name = "origin", inject = listOf("parts")) { ctx, _ ->
            val part = Rutis.define<Depth>(name = "part", provides = mapOf("tag" to service<TagService>(sync = setOf("text")))) { ctx, config ->
                ctx.provide("tag", TagService("${config.depth} $prepared"))
            }
            made.set(part)
            ctx.use<PartsClient>("parts").register("session", part)
        }
        RutisTest.load(origin, services = mapOf("parts" to parts)).use {
            assertEquals(1, parts.handed.size)
            val (scope, definition) = parts.handed.single()
            assertEquals("session", scope)
            assertSame(made.get(), definition)
            RutisTest.load(definition, Depth(1)).use { one ->
                RutisTest.load(definition, Depth(2)).use { two ->
                    assertEquals("1 $prepared", one.service("tag").call("text"))
                    assertEquals("2 $prepared", two.service("tag").call("text"))
                }
            }
        }
    }

    /** A test double hands a definition back: the plugin gets the same definition, as rutis sends it home. */
    @Test
    fun aTestDoubleHandsADefinitionBack() = runBlocking<Unit> {
        val same = AtomicReference<Boolean>()
        val origin = Rutis.define<Rutis.NoConfig>(name = "origin", inject = listOf("parts")) { ctx, _ ->
            val part = Rutis.define<Rutis.NoConfig>(name = "part") { _, _ -> }
            same.set(ctx.use<PartsClient>("parts").echo(part) === part)
        }
        RutisTest.load(origin, services = mapOf("parts" to HandedParts())).use {
            assertEquals(true, same.get())
        }
    }

    /** The generated method table takes and returns plugin definitions, nullable ones too. */
    @Test
    fun theTableTakesPluginDefinitions() = runBlocking<Unit> {
        val table = Tables.serviceTable(HandedParts::class.java)!!
        val parts = HandedParts()
        val part = Rutis.define<Rutis.NoConfig>(name = "part") { _, _ -> }
        table.invoke(parts, "register", listOf("s", part))
        assertSame(part, parts.handed.single().second)
        assertSame(part, table.invoke(parts, "maybe", listOf(part)))
        assertEquals(null, table.invoke(parts, "maybe", listOf(null)))
        val refused = assertFailsWith<RutisException> { table.serve(parts, "register", listOf("s", "not a plugin"), "parts") }
        assertEquals("parts.register: argument 2 should be a plugin definition, got \"not a plugin\"", refused.message)
    }
}
