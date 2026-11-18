package arcships.rutis

import arcships.rutis.internal.AppLink
import arcships.rutis.internal.AppLinks
import arcships.rutis.internal.remote.Ended
import arcships.rutis.internal.remote.Status
import org.junit.jupiter.api.extension.ExtendWith
import org.pastalab.fray.junit.junit5.FrayTestExtension
import org.pastalab.fray.junit.junit5.annotations.FrayTest
import java.io.IOException
import java.util.Collections
import java.util.concurrent.CountDownLatch
import java.util.concurrent.LinkedBlockingQueue
import java.util.concurrent.TimeUnit
import kotlin.test.assertEquals
import kotlin.test.assertNotSame
import kotlin.test.assertNotNull
import kotlin.test.assertTrue
import kotlin.time.Duration.Companion.seconds

/** The access point the fake links claim to be connected to; nothing dials it. */
private const val DIALED = "ws://127.0.0.1:1/rutis"

/** Where the fake listener claims to be bound; nothing binds it. */
private const val BOUND = "ws://127.0.0.1:9/rutis"

/**
 * `RutisApp`'s start and stop under Fray's scheduler (#1274, the Kotlin
 * part of #630 and the AH1 row of the quality status): which run connects,
 * what a stop does to a connection under way, and what a failed dial or
 * listener leaves behind, whichever way the starting thread, the run's
 * thread and [RutisApp.stop] interleave.
 *
 * Fray 0.9.2 cannot schedule the real transport: the WebSocket listener
 * sits in a native `accept()` and OkHttp's dispatcher and timers run
 * outside its scheduler, so a case that dials a real server never finishes
 * (the run's thread stays blocked in Fray's `scheduleNextOperation`, see
 * the issue). The cases here inject [FakeLinks] instead: [AppLinks.open]
 * returns a link the case drives, without calling `real`, so nothing binds,
 * dials or starts a thread of its own. The real WebSocket paths stay
 * covered by `AppTest` under the real scheduler.
 */
@ExtendWith(FrayTestExtension::class)
class RutisAppFrayTest {
    /**
     * A normal start and stop: the run dials, the state says connected, and
     * the stop ends the link the start opened, uncontested.
     */
    @FrayTest(iterations = 10, resetClassLoaderPerIteration = false)
    fun aStartDialsAndAStopEndsIt() {
        val links = FakeLinks()
        val app = app(links)
        app.start()
        val link = links.next().awaitStarted()
        assertEquals(RutisState.Connected(DIALED), app.state.value, "the link connected")
        app.stop()
        link.awaitFinished()
        assertTrue(link.stoppedByApp, "the stop reached the link")
        assertEquals(RutisState.Stopped(null), awaitRunEnds(app))
    }

    /**
     * A normal start and stop of a listener: it says where it bound once,
     * and the stop ends it.
     */
    @FrayTest(iterations = 5, resetClassLoaderPerIteration = false)
    fun aStartListensAndAStopEndsIt() {
        val lines = Collections.synchronizedList(ArrayList<String>())
        val links = FakeLinks(first = Status.Connected(BOUND), bound = BOUND)
        val app = app(links, connect = null, listen = BOUND, log = lines::add)
        app.start()
        val link = links.next().awaitStarted()
        assertEquals(RutisState.Connected(BOUND), app.state.value, "the listener is connected")
        assertEquals(listOf("listening on $BOUND"), lines.toList(), "one listening line, for the bound address")
        app.stop()
        link.awaitFinished()
        assertEquals(RutisState.Stopped(null), awaitRunEnds(app))
    }

    /**
     * A listener that cannot bind: the run ends as refused, with the
     * reason, and no link exists to stop.
     */
    @FrayTest(iterations = 5, resetClassLoaderPerIteration = false)
    fun aListenerThatCannotBindStopsWithTheReason() {
        val links = FakeLinks(failOpen = IOException("Address already in use"))
        val app = app(links, connect = null, listen = BOUND)
        app.start()
        assertEquals(
            RutisState.Stopped("refused: incompatible (Address already in use); stopping"),
            awaitRunEnds(app),
        )
        assertEquals(0, links.opened.size, "nothing listened")
    }

    /**
     * A dial that cannot connect: the link reports it and keeps retrying
     * (the retry rules are `Dialing`'s, covered by `AppTest`); a stop ends
     * it as stopped, not refused.
     */
    @FrayTest(iterations = 10, resetClassLoaderPerIteration = false)
    fun aDialThatCannotConnectKeepsRetryingUntilStopped() {
        val retrying = "cannot connect (Connection refused); retrying in 0.5s"
        val links = FakeLinks(first = Status.Connecting(retrying))
        val app = app(links)
        app.start()
        val link = links.next().awaitStarted()
        assertEquals(RutisState.Connecting(retrying), app.state.value, "the failure is shown while it retries")
        app.stop()
        link.awaitFinished()
        assertTrue(link.stoppedByApp, "the stop cancelled the retrying dial")
        assertEquals(RutisState.Stopped(null), awaitRunEnds(app))
    }

    /**
     * A dial the access point refuses as incompatible: the run stops with
     * the reason, and the app is stopped without a stop() call.
     */
    @FrayTest(iterations = 5, resetClassLoaderPerIteration = false)
    fun aDialRefusedAsIncompatibleStopsWithTheReason() {
        val links = FakeLinks()
        val app = app(links)
        app.start()
        val link = links.next().awaitStarted()
        val reason = "this endpoint speaks rutis.2, the dialer offered rutis.1: upgrade the dialer"
        link.refuse(reason)
        assertEquals(RutisState.Stopped("refused: incompatible ($reason); stopping"), awaitRunEnds(app))
    }

    /**
     * A start stopped at once: whether the stop came before the link was
     * opened (then none is) or after it, no link is left running and the app
     * ends stopped.
     */
    @FrayTest(iterations = 20, resetClassLoaderPerIteration = false)
    fun aStartStoppedAtOnceLeavesNoLinkRunning() {
        val links = FakeLinks()
        val app = app(links)
        app.start()
        app.stop()
        assertEquals(RutisState.Stopped(null), awaitRunEnds(app))
        val opened = links.opened.toList()
        assertTrue(opened.all { it.stoppedByApp }, "a link the start opened was not stopped: $opened")
    }

    /**
     * Start, stop, start: each run connects with a link of its own, and the
     * second starts only after the first ended.
     */
    @FrayTest(iterations = 10, resetClassLoaderPerIteration = false)
    fun aRestartConnectsWithAFreshLink() {
        val links = FakeLinks()
        val app = app(links)
        app.start()
        val first = links.next().awaitStarted()
        assertEquals(RutisState.Connected(DIALED), app.state.value)
        app.stop()
        first.awaitFinished()
        assertEquals(RutisState.Stopped(null), awaitRunEnds(app))

        app.start()
        val second = links.next().awaitStarted()
        assertEquals(RutisState.Connected(DIALED), app.state.value)
        app.stop()
        second.awaitFinished()
        assertEquals(RutisState.Stopped(null), awaitRunEnds(app))
        assertNotSame(first, second)
    }

    private fun app(
        links: AppLinks,
        connect: String? = DIALED,
        listen: String? = null,
        log: (String) -> Unit = {},
    ): RutisApp = RutisApp(
        plugins = listOf(Rutis.define<Rutis.NoConfig>(name = "nothing") { _, _ -> }),
        configs = emptyMap(),
        connect = connect,
        listen = listen,
        id = "fray",
        token = "secret",
        peer = "test",
        trust = null,
        certificate = null,
        shutdownTimeout = 10.seconds,
        log = log,
        links = links,
    )

    /**
     * Joins the run's thread: it sets its last state before it ends, so the
     * state needs no clock, and a blocked test lets Fray schedule the run.
     */
    private fun awaitRunEnds(app: RutisApp): RutisState.Stopped {
        val thread = assertNotNull(app.runThread, "the app has a run")
        thread.join(10_000)
        assertTrue(!thread.isAlive, "the run did not end")
        val state = app.state.value
        assertTrue(state is RutisState.Stopped, "the run ended stopped: $state")
        return state
    }

    /**
     * The links of a case: [open] never calls `real`, so nothing binds or
     * dials, and every link it returns is one the case starts, fails and
     * stops by hand.
     */
    private class FakeLinks(
        private val first: Status = Status.Connected(DIALED),
        private val bound: String? = null,
        private val failOpen: Throwable? = null,
    ) : AppLinks {
        /** In the order the starts opened them; a start stopped before its link exists opens none. */
        val opened = LinkedBlockingQueue<FakeLink>()

        override fun open(status: (Status) -> Unit, real: () -> AppLink): AppLink {
            failOpen?.let { throw it }
            return FakeLink(status, first, bound).also { opened.put(it) }
        }

        /** The link a start just opened. */
        fun next(): FakeLink = assertNotNull(opened.poll(10, TimeUnit.SECONDS), "the start opened no link")
    }

    /** One connection a case drives: it says what it is, waits, and ends as the case says. */
    private class FakeLink(
        private val status: (Status) -> Unit,
        /** What the link reports before it waits: a connection, or a dial being retried. */
        private val first: Status,
        override val bound: String?,
    ) : AppLink {
        private val events = LinkedBlockingQueue<String>()
        private val started = CountDownLatch(1)
        private val finished = CountDownLatch(1)

        @Volatile
        var stoppedByApp = false
            private set

        /** Waits until the app runs this link and it said what it is. */
        fun awaitStarted(): FakeLink {
            assertTrue(started.await(10, TimeUnit.SECONDS), "the link did not run")
            return this
        }

        /** Waits until [run] returned. */
        fun awaitFinished(): FakeLink {
            assertTrue(finished.await(10, TimeUnit.SECONDS), "the link did not end")
            return this
        }

        /** Ends [run] as refused, as a dial the access point refuses does; the words are the reason. */
        fun refuse(reason: String) {
            events.put(reason)
        }

        override fun run(): Ended {
            status(first)
            started.countDown()
            val event = events.take()
            finished.countDown()
            return if (event == STOP) Ended.Stopped(true) else Ended.Refused(event)
        }

        override fun stop() {
            stoppedByApp = true
            events.put(STOP)
        }

        private companion object {
            const val STOP = "stop"
        }
    }
}
