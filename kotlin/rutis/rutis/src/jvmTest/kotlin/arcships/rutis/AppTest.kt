package arcships.rutis

import arcships.rutis.advanced.RemoteFunction
import arcships.rutis.internal.Report
import arcships.rutis.internal.peer.ENDPOINT_PROTOCOL
import arcships.rutis.internal.peer.Exported
import arcships.rutis.internal.peer.Endpoint
import arcships.rutis.internal.peer.Options
import arcships.rutis.internal.peer.Peer
import arcships.rutis.internal.remote.Heartbeat
import arcships.rutis.internal.remote.ListenAddress
import arcships.rutis.internal.remote.WebSocketServer
import kotlinx.coroutines.runBlocking
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.JsonPrimitive
import java.util.Collections
import java.util.concurrent.TimeUnit
import kotlin.test.AfterTest
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertTrue

@RutisService
class AppGreeting(private val word: String) {
    fun say(): String = word
}

@Serializable
data class AppUnits(val units: String = "metric")

@Serializable
data class AppWords(val word: String)

@RutisClient
interface AppSayer {
    suspend fun say(): String
}

@RutisClient
interface AppClock {
    suspend fun now(): String
}

/**
 * An application's own plugins (docs/design-remote-session-2026-10-28.md
 * §2.6, decision 2): once rutis greeted, [RutisApp] offers `services` and
 * installs them in order, each with its configuration or its schema's
 * defaults, once the services it injects are there (its own plugins' or
 * rutis's). It announces each service they provide with the plugin's name
 * once rutis offers `services`, again to a new offer, never one rutis
 * announced; rutis's rows cannot run them. A service rutis withdraws unloads
 * the plugin injecting it, which loads again once it is back. Stopping
 * withdraws the services, then the plugins clean up, latest first; the
 * versions come from one sequence.
 */
class AppTest {
    private val closing = ArrayList<AutoCloseable>()

    @AfterTest
    fun closeAll() {
        closing.forEach { runCatching { it.close() } }
    }

    private fun until(what: String, condition: () -> Boolean) {
        val deadline = System.nanoTime() + 10_000_000_000
        while (!condition()) {
            assertTrue(System.nanoTime() < deadline, what)
            Thread.sleep(10)
        }
    }

    /** A field of a frame as text (data arrives as JSON). */
    private fun Map<*, *>.text(key: String): String? = (this[key] as? JsonPrimitive)?.content

    private fun server(): WebSocketServer {
        val server = WebSocketServer(ListenAddress.parse("ws://127.0.0.1:0/rutis"), "rutis.$ENDPOINT_PROTOCOL", "secret", null, Heartbeat())
        closing.add(AutoCloseable { server.close() })
        return server
    }

    /** rutis's end of the next connection: what it is told goes to `told`; `refuse` may refuse a call. */
    private fun host(
        server: WebSocketServer,
        id: String,
        told: MutableList<Pair<String, Map<*, *>>>,
        refuse: (String, Map<*, *>) -> Unit = { _, _ -> },
    ): Peer {
        val connection = server.accepted.poll(10, TimeUnit.SECONDS)!!
        val host = Peer(
            connection,
            Options(
                dispatch = { _, method, args ->
                    val fields = args[0] as Map<*, *>
                    told.add(method to fields)
                    refuse(method, fields)
                    null
                },
                endpoint = Endpoint("test", id),
            ),
        )
        host.start()
        host.readyFuture.get(10, TimeUnit.SECONDS)
        return host
    }

    private fun Peer.offer(version: Long) {
        call("", "link.offers", listOf(mapOf("families" to listOf("services"), "version" to version, "since" to mapOf("services" to version))), false)
    }

    /** What the host was told by `method`, in the order of the frames' versions (each call is served on a thread of its own). */
    private fun List<Pair<String, Map<*, *>>>.of(method: String) =
        toList().filter { it.first == method }.map { it.second }.sortedBy { it.text("version")?.toLong() ?: 0 }

    @Test
    fun aConfigurationOfAnotherTypeOrOfNoPluginIsRefused() {
        val words = Rutis.define<AppWords>(name = "words") { _, _ -> }
        val wrong = assertFailsWith<IllegalArgumentException> {
            RutisApp(plugins = listOf(words), configs = mapOf("words" to AppUnits()), connect = "ws://127.0.0.1:1/rutis", id = "pad", token = "")
        }
        assertTrue(wrong.message!!.startsWith("rutis: plugin words: AppUnits(units=metric) is not its configuration"), wrong.message)
        val stray = assertFailsWith<IllegalArgumentException> {
            RutisApp(plugins = listOf(words), configs = mapOf("nope" to AppWords("x")), connect = "ws://127.0.0.1:1/rutis", id = "pad", token = "")
        }
        assertEquals("rutis: configs names nope, which is not one of the plugins: words", stray.message)
    }

    @Test
    fun aConnectedAppInstallsItsPluginsAndAnnouncesTheirServices() {
        val server = server()
        val lines = Collections.synchronizedList(ArrayList<String>())
        val first = Rutis.define<AppUnits>(name = "first") { ctx, config ->
            lines.add("first ${config.units}")
            ctx.provide("units", AppGreeting(config.units))
            ctx.effect { lines.add("first cleans up") }
        }
        // A plugin that fails: reported, and the next one installed.
        val failing = Rutis.define<Rutis.NoConfig>(name = "failing") { _, _ -> throw IllegalStateException("no contacts") }
        // It uses the service of a plugin installed before it in the app.
        val second = Rutis.define<AppWords>(name = "second", inject = listOf("units")) { ctx, config ->
            lines.add("second ${config.word} ${ctx.use<AppSayer>("units").say()}")
            ctx.provide("greeting", AppGreeting(config.word))
            ctx.effect { lines.add("second cleans up") }
        }
        // It uses a service rutis announces.
        val third = Rutis.define<Rutis.NoConfig>(name = "third", inject = listOf("clock")) { ctx, _ ->
            lines.add("third ${ctx.use<AppClock>("clock").now()}")
            ctx.effect { lines.add("third cleans up") }
        }
        val reported = Collections.synchronizedList(ArrayList<String>())
        val previous = Report.replace { reported.add(it) }
        closing.add(AutoCloseable { Report.replace(previous) })
        val app = RutisApp(
            plugins = listOf(first, failing, second, third), configs = mapOf("second" to AppWords("hello")),
            connect = server.address, id = "pad", token = "secret", peer = "test", log = {},
        )
        closing.add(AutoCloseable { app.stop() })
        app.start()

        val told = Collections.synchronizedList(ArrayList<Pair<String, Map<*, *>>>())
        val host = host(server, "pad", told)
        until("the app's offer: $told") { told.of("link.offers").isNotEmpty() }
        assertEquals(listOf("services"), (told.of("link.offers")[0]["families"] as List<*>).map { (it as JsonPrimitive).content })
        until("first and second: $lines") { lines.any { it.startsWith("second") } }
        assertEquals(listOf("first metric", "second hello metric"), lines.toList(), "third waits for clock")
        assertTrue("rutis: plugin failing: IllegalStateException: no contacts" in reported, "$reported")
        Thread.sleep(100)
        assertEquals(0, told.of("services.announce").size, "nothing announced before rutis offers services")

        host.offer(1)
        until("two announcements: $told") { told.of("services.announce").size == 2 }
        val announced = told.of("services.announce")
        assertEquals(listOf("units", "greeting"), announced.map { it.text("name") })
        assertEquals(listOf("first", "second"), announced.map { it.text("plugin") })
        assertEquals(listOf("1", "2"), announced.map { it.text("version") })
        assertEquals("async", (announced[1]["shape"] as Map<*, *>).text("say"))
        val say = (announced[1]["service"] as Map<*, *>)["say"] as RemoteFunction
        assertEquals(JsonPrimitive("hello"), runBlocking { host.callFunctionSuspend(say, emptyList(), sync = false) })
        // A new offer of services: announced again, at the same versions.
        host.offer(2)
        until("announced again: $told") { told.of("services.announce").size == 4 }
        assertEquals(listOf("1", "1", "2", "2"), told.of("services.announce").map { it.text("version") })

        // rutis's clock: third installs; withdrawn, it unloads; back, it loads again.
        fun clock(method: String, version: Long, time: String = "") {
            val fields = linkedMapOf<String, Any?>("name" to "clock", "version" to version)
            if (method == "services.announce") {
                fields["service"] = mapOf("now" to Exported(Any()) { _ -> time })
                fields["shape"] = mapOf("now" to "async")
            }
            host.call("", method, listOf(fields), false)
        }
        fun third() = lines.filter { it.startsWith("third") }
        clock("services.announce", 1, "noon")
        until("third installed: $lines") { third().size == 1 }
        assertEquals(listOf("third noon"), third())
        clock("services.withdraw", 2)
        until("third unloaded: $lines") { third().size == 2 }
        // Older than the withdrawal: ignored.
        clock("services.announce", 1, "late")
        clock("services.announce", 3, "dusk")
        until("third back: $lines") { third().size == 3 }
        assertEquals(listOf("third noon", "third cleans up", "third dusk"), third())
        assertTrue(told.of("services.announce").none { it.text("name") == "clock" }, "rutis's service is not sent back")

        // rutis's rows cannot run an application's plugins.
        val mounted = host.call("", "mount", listOf(emptyMap<String, Any?>()), false) as Map<*, *>
        assertEquals(emptyMap<String, Any?>(), mounted["plugins"])
        assertFailsWith<RutisException> { host.call("", "rows.load", listOf("again", "first"), false) }

        app.stop()
        until("two withdrawals: $told") { told.of("services.withdraw").size == 2 }
        val withdrawn = told.of("services.withdraw")
        assertEquals(listOf("greeting", "units"), withdrawn.map { it.text("name") })
        assertEquals(listOf("3", "4"), withdrawn.map { it.text("version") })
        until("the cleanups: $lines") { lines.count { it.endsWith("cleans up") } == 4 }
        assertEquals(
            listOf("third cleans up", "third cleans up", "second cleans up", "first cleans up"),
            lines.filter { it.endsWith("cleans up") },
        )
    }

    /**
     * Two applications provide `health`: rutis refuses the later one's
     * announcement (the host's kernel rule; here the host refuses as rutis
     * does), and that application says why. The first one's stays.
     */
    @Test
    fun theLaterOfTwoAppsProvidingOneNameIsRefusedWithTheReason() {
        val server = server()
        val health = Rutis.define<Rutis.NoConfig>(name = "health") { ctx, _ -> ctx.provide("health", AppGreeting("ok")) }
        val reported = Collections.synchronizedList(ArrayList<String>())
        val previous = Report.replace { reported.add(it) }
        closing.add(AutoCloseable { Report.replace(previous) })
        val provided = Collections.synchronizedMap(HashMap<String, String>())
        val told = Collections.synchronizedList(ArrayList<Pair<String, Map<*, *>>>())
        for (id in listOf("pad-1", "pad-2")) {
            val app = RutisApp(plugins = listOf(health), connect = server.address, id = id, token = "secret", peer = "test", log = {})
            closing.add(AutoCloseable { app.stop() })
            app.start()
            val host = host(server, id, told) { method, fields ->
                if (method != "services.announce") return@host
                val name = fields.text("name")!!
                val owner = provided.putIfAbsent(name, "$id:${fields.text("plugin")}")
                if (owner != null) throw RutisException("Error", "$id:${fields.text("plugin")}: $name is provided here by row $owner: not registered from $id")
            }
            host.offer(1)
            until("$id announced: $told") { told.of("services.announce").size == if (id == "pad-1") 1 else 2 }
        }
        val refused = "rutis: cannot announce health: Error: pad-2:health: health is provided here by row pad-1:health: not registered from pad-2"
        until("the refusal: $reported") { refused in reported }
        assertEquals("pad-1:health", provided["health"])
    }
}
