package arcships.rutis.testing

import arcships.rutis.Ctx
import arcships.rutis.EventArgs
import arcships.rutis.Plugin
import arcships.rutis.Rutis
import arcships.rutis.RutisClient
import arcships.rutis.advanced.Emitter
import arcships.rutis.advanced.InProcessSession
import arcships.rutis.advanced.Listener
import arcships.rutis.advanced.serveInProcess
import arcships.rutis.generated.ClientFactory
import arcships.rutis.generated.Generated
import arcships.rutis.generated.ServiceHandle
import arcships.rutis.generated.Tables
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import java.util.concurrent.CompletableFuture
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicLong

/**
 * Tests a plugin on the real rutis kernel (docs/design-platform-cleanup-2026-10-28.md
 * §7.5): it gives the plugin the services it injects, calls the services
 * it provides, emits and listens to events, and unloads it.
 *
 * ```kotlin
 * RutisTest.load(weather, Config("Oslo"), services = mapOf("llm" to FakeLlm())).use { loaded ->
 *     assertEquals("sunny", loaded.service("weather").call("today"))
 * }
 * ```
 *
 * The plugin runs in its real runtime, in this process, as a row of a real
 * host: `rutis-host test`, found on `PATH` or at `RUTIS_HOST`, started once
 * per test process. rutis checks its config, fills its defaults, resolves
 * what it injects and dispatches its events, as a host running it would.
 * The test's services, listeners and emissions are a plugin of the same
 * runtime (`rutistest`), at the root scope.
 */
object RutisTest {
    /**
     * Loads `plugin` with `config` (a value of its config type, or null) and
     * the services in `services` (instances of `@RutisService` classes), as
     * a row of rutis: its id is `row`, or one rutis gives from the plugin's
     * name; `isolate` gives services a scope label of the row's
     * (`{ "hello": "second" }`, as the row's `isolate` in rutis.json). A
     * config rutis refuses fails the load with the row's line. Close the
     * result to unload it.
     */
    fun load(
        plugin: Plugin,
        config: Any? = null,
        services: Map<String, Any> = emptyMap(),
        row: String? = null,
        isolate: Map<String, String> = emptyMap(),
    ): Loaded {
        val test = Test(Host.get(), plugin)
        try {
            for ((name, service) in services) test.provide(name, service)
            return test.load(config, row, isolate)
        } catch (e: Throwable) {
            test.close()
            throw e
        }
    }
}

/** How long a call to rutis may take. */
private const val WAIT_MILLIS = 30_000L

/** The name of the plugin of the test's own services, listeners and emissions. */
private const val DRIVER = "rutistest"

/** The service of `rutis-host test`. */
@RutisClient
internal interface HostTest {
    suspend fun load(name: String, config: Any?, options: Map<String, Any?>): String

    suspend fun update(id: String, config: Any?)

    suspend fun unload(id: String)

    suspend fun reported(id: String): List<String>
}

private fun <T> wait(block: suspend () -> T): T = runBlocking { withTimeout(WAIT_MILLIS) { block() } }

/** The host of this test process, `rutis-host test`, and the runtime it drives here. */
internal class Host private constructor(
    private val process: Process,
    private val session: InProcessSession,
    val ctx: Ctx,
    val rutis: HostTest,
) {
    private val plugins = AtomicLong()

    /** Registers `plugin` for the rows loaded from now on. */
    fun register(plugin: Plugin) = session.register(plugin)

    /** A plugin name of this test process's own. */
    fun fresh(prefix: String): String = "$prefix-${plugins.incrementAndGet()}"

    fun end() {
        session.end()
        if (!process.waitFor(10, TimeUnit.SECONDS)) process.destroyForcibly()
    }

    companion object {
        @Volatile
        private var started: Host? = null

        @Synchronized
        fun get(): Host = started ?: start().also { started = it }

        private fun start(): Host {
            val ready = CompletableFuture<Pair<Ctx, HostTest>>()
            val driver = Rutis.define<Rutis.NoConfig>(name = DRIVER, inject = listOf("rutis-host/test")) { ctx, _ ->
                ready.complete(ctx to ctx.use<HostTest>("rutis-host/test"))
            }
            val (process, socket) = channel()
            val session = try {
                Rutis.serveInProcess(socket.getInputStream(), socket.getOutputStream(), "test", socket::close).also { it.register(driver) }
            } catch (e: Throwable) {
                process.destroyForcibly()
                throw e
            }
            val (ctx, rutis) = try {
                ready.get(WAIT_MILLIS, TimeUnit.MILLISECONDS)
            } catch (e: Exception) {
                session.end()
                process.destroyForcibly()
                throw IllegalStateException("RutisTest: rutis-host test did not load the test's plugin: ${e.message}", e)
            }
            val host = Host(process, session, ctx, rutis)
            Runtime.getRuntime().addShutdownHook(Thread(host::end, "rutistest-end"))
            return host
        }
    }
}

/**
 * Starts `rutis-host test` (`RUTIS_HOST`, else `rutis-host` on `PATH`) on a
 * channel this process made, and returns the connection of its session: a
 * loopback port the host dials, presenting a one-time token
 * (`--channel tcp:`; the JVM cannot hand a descriptor over).
 */
private fun channel(): Pair<Process, java.net.Socket> {
    val host = System.getenv("RUTIS_HOST")?.takeIf { it.isNotEmpty() } ?: "rutis-host"
    val server = java.net.ServerSocket(0, 1, java.net.InetAddress.getLoopbackAddress())
    val token = java.util.HexFormat.of().formatHex(ByteArray(32).also(java.security.SecureRandom()::nextBytes))
    val builder = ProcessBuilder(host, "test", DRIVER, "--channel", "tcp:127.0.0.1:${server.localPort}").redirectErrorStream(true)
    builder.environment()["RUTIS_CHANNEL_TOKEN"] = token
    val process = try {
        builder.start()
    } catch (e: java.io.IOException) {
        server.close()
        throw IllegalStateException("RutisTest: cannot start $host (put rutis-host on PATH or set RUTIS_HOST): ${e.message}", e)
    }
    // The host's lines (config warnings, failures it reports) reach the test's output.
    Thread({ process.inputStream.bufferedReader().forEachLine(System.err::println) }, "rutistest-host").apply { isDaemon = true }.start()
    return try {
        server.soTimeout = WAIT_MILLIS.toInt()
        val socket = server.accept()
        socket.tcpNoDelay = true
        val presented = StringBuilder()
        val input = socket.getInputStream()
        while (true) {
            val b = input.read()
            if (b < 0 || b == '\n'.code) break
            presented.append(b.toChar())
            if (presented.length > token.length) break
        }
        if (presented.toString() != token) {
            socket.close()
            throw IllegalStateException("rutis-host test presented another token")
        }
        process to socket
    } catch (e: Exception) {
        process.destroyForcibly()
        throw IllegalStateException("RutisTest: rutis-host test did not connect: ${e.message}", e)
    } finally {
        server.close()
    }
}

/** One `RutisTest.load`: the plugin's rows, and the test's services and listeners. */
internal class Test(val host: Host, val plugin: Plugin) {
    val rows = ArrayList<Loaded>()
    private val withdrawals = ArrayList<() -> Unit>()
    private val uses = ArrayList<String>()

    @Volatile
    var closed = false

    fun provide(name: String, service: Any) {
        val withdraw = host.ctx.provide(name, service)
        synchronized(this) { withdrawals.add(withdraw) }
    }

    fun listen(name: String, prepend: Boolean, global: Boolean, listener: suspend (EventArgs) -> Any?): () -> Unit {
        val off = host.ctx.on(name, prepend, global, listener)
        synchronized(this) { withdrawals.add(off) }
        return off
    }

    fun load(config: Any?, row: String?, isolate: Map<String, String>): Loaded {
        check(!closed) { "RutisTest: the test ended" }
        host.register(plugin)
        val options = LinkedHashMap<String, Any?>()
        if (row != null) options["id"] = row
        if (isolate.isNotEmpty()) options["isolate"] = isolate
        val id = wait { host.rutis.load(plugin.name, config, options) }
        return Loaded(this, id, isolate).also { synchronized(this) { rows.add(it) } }
    }

    /** The service `name` as a row in `isolate` uses it: through a row of its own that injects it, once rutis has it. */
    fun use(name: String, isolate: Map<String, String>): ServiceHandle {
        val used = CompletableFuture<ServiceHandle>()
        val user = Rutis.define<Rutis.NoConfig>(name = host.fresh("$DRIVER-use"), inject = listOf(name)) { ctx, _ ->
            used.complete(ctx.use(name, Dynamic::class.java).handle)
        }
        host.register(user)
        val options = LinkedHashMap<String, Any?>()
        isolate[name]?.let { options["isolate"] = mapOf(name to it) }
        val id = wait { host.rutis.load(user.name, null, options) }
        synchronized(this) { uses.add(id) }
        return try {
            used.get(WAIT_MILLIS, TimeUnit.MILLISECONDS)
        } catch (e: java.util.concurrent.TimeoutException) {
            throw IllegalStateException("RutisTest: no service $name (now)")
        }
    }

    fun unload(id: String) = wait { host.rutis.unload(id) }

    fun reported(): List<String> {
        val ids = synchronized(this) { rows.map { it.row } }
        return ids.flatMap { wait { host.rutis.reported(it) } }
    }

    /** Unloads every row, latest first, and withdraws the test's services and listeners. */
    fun close() {
        if (closed) return
        closed = true
        val (all, used, withdrawn) = synchronized(this) { Triple(rows.toList(), uses.toList(), withdrawals.toList()) }
        for (row in all.asReversed()) row.unload()
        for (id in used.asReversed()) unload(id)
        for (withdraw in withdrawn.asReversed()) withdraw()
    }
}

/** A row of a plugin loaded for a test. Close it (or the first row) to unload. */
class Loaded internal constructor(
    private val test: Test,
    /** Its id. */
    val row: String,
    private val isolate: Map<String, String>,
) : AutoCloseable, Emitter {
    @Volatile
    private var unloaded = false

    /** Loads another row of the same plugin, as [RutisTest.load] does. */
    fun load(config: Any? = null, row: String? = null, isolate: Map<String, String> = emptyMap()): Loaded =
        test.load(config, row, isolate)

    /**
     * The service `name` the plugin provides, as a plugin of the same
     * runtime gets it: its declared methods, or, undeclared, its class's, all
     * `async`. Waits until rutis has it.
     */
    fun service(name: String): TestService = TestService(name, test.use(name, isolate))

    /**
     * Listens to `name` at the root scope until the returned function is
     * called or the test ends: the plugin's emissions of `name` reach
     * `listener`, and so do the test's own.
     */
    fun on(name: String, prepend: Boolean = false, global: Boolean = false, listener: suspend (EventArgs) -> Any?): () -> Unit =
        test.listen(name, prepend, global, listener)

    /** Emits only to the listeners `keep` accepts (global listeners always). */
    fun filter(keep: suspend (Listener) -> Boolean): Emitter = test.host.ctx.filter(keep)

    override fun emit(name: String, vararg args: Any?) = test.host.ctx.emit(name, *args)

    override suspend fun parallel(name: String, vararg args: Any?) = test.host.ctx.parallel(name, *args)

    override suspend fun serial(name: String, vararg args: Any?): Any? = test.host.ctx.serial(name, *args)

    override suspend fun waterfall(name: String, vararg args: Any?, terminal: (suspend (EventArgs) -> Any?)?): Any? =
        test.host.ctx.waterfall(name, *args, terminal = terminal)

    override fun bail(name: String, vararg args: Any?): Any? = test.host.ctx.bail(name, *args)

    /** What rutis reported of the test's rows so far, one line per failure nobody waited for (listeners, filters of `emit`). */
    fun reported(): List<String> = test.reported()

    /**
     * Unloads the row: the runtime cancels its running listeners and waits
     * for them (up to RUTIS_SHUTDOWN_TIMEOUT), then runs its cleanups.
     */
    fun unload() {
        if (unloaded) return
        unloaded = true
        test.unload(row)
    }

    /** Changes the row's config, as rutis does when a row's config changes: the row unloads and loads again. */
    fun update(config: Any?) {
        check(!unloaded && !test.closed) { "RutisTest: row $row is not loaded" }
        wait { test.host.rutis.update(row, config) }
    }

    /** Unloads every row of the test, latest first (the first row's close), or this row. */
    override fun close() {
        if (synchronized(test) { test.rows.firstOrNull() } === this) test.close() else unload()
    }
}

/** A provided service, called as a plugin of the same runtime calls it. */
class TestService internal constructor(
    val name: String,
    private val handle: ServiceHandle,
) {
    /** The service's methods, each `sync` or `async`. */
    val methods: Map<String, String> get() = handle.kinds

    /** Calls `method` with native values; the result as a native value. */
    suspend fun call(method: String, vararg args: Any?): Any? = Generated.toNative(raw(method, args.map(Generated::nativeResult)))

    /** Calls `method` with session values; the result as a session value (for the conformance checks). */
    suspend fun raw(method: String, args: List<Any?>): Any? = handle.callSuspend(method, args)

    /** The service through the `@RutisClient` interface `I`. */
    inline fun <reified I : Any> bind(): I = bind(I::class.java)

    fun <I : Any> bind(type: Class<I>): I {
        val factory: ClientFactory<I> = Tables.clientFactory(type) ?: throw IllegalArgumentException("${type.name} has no proxy: annotate it @RutisClient")
        return factory.create(handle)
    }
}

/** A service called by method name: what [Loaded.service] uses it through. */
internal interface Dynamic {
    val handle: ServiceHandle
}

/** The proxy factory of [Dynamic], found by its name as generated ones are: it calls no method of its own. */
@Suppress("ClassName", "unused")
internal class Dynamic_RutisClient : ClientFactory<Dynamic>() {
    override val methods: List<String> = emptyList()

    override fun create(handle: ServiceHandle): Dynamic = object : Dynamic {
        override val handle: ServiceHandle = handle
    }
}
