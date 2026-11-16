package arcships.rutis

import arcships.rutis.internal.Report
import arcships.rutis.internal.RuntimeState
import kotlinx.coroutines.runBlocking
import org.junit.jupiter.api.extension.ExtendWith
import org.pastalab.fray.junit.junit5.FrayTestExtension
import org.pastalab.fray.junit.junit5.annotations.FrayTest
import java.util.Collections
import java.util.concurrent.CountDownLatch
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicInteger
import java.util.concurrent.atomic.AtomicReference
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertTrue

/**
 * [`RuntimeState`] under Fray's scheduler (#630): which rows and services
 * exist, and what a load, an unload and a provide see, whichever way they
 * interleave.
 */
@ExtendWith(FrayTestExtension::class)
class RuntimeStateFrayTest {
    /**
     * Two loads of one key at once: one loads the row, the other is told it is
     * loaded already; the key is free again once it unloads.
     */
    @FrayTest(iterations = 20, resetClassLoaderPerIteration = false)
    fun twoLoadsOfOneKeyLoadItOnce() {
        val applications = AtomicInteger()
        val plugin = Rutis.define<Rutis.NoConfig>(name = "one-row") { _, _ -> applications.incrementAndGet() }
        val rt = RuntimeState(listOf(plugin))
        val loaded = AtomicInteger()
        val refused = Collections.synchronizedList(ArrayList<Throwable>())
        val loaders = (1..2).map {
            Thread {
                try {
                    runBlocking { rt.load("row", "row", plugin.name, null, emptyMap(), emptyMap(), emptyList()) }
                    loaded.incrementAndGet()
                } catch (e: Throwable) {
                    refused.add(e)
                }
            }
        }
        loaders.forEach(Thread::start)
        loaders.forEach(Thread::join)
        assertEquals(1, loaded.get(), "the row loaded ${loaded.get()} times")
        assertEquals(1, refused.size, "loads refused: $refused")
        assertTrue(refused[0].message!!.contains("row row is already loaded"), refused[0].message!!)
        assertEquals(1, applications.get())
        assertEquals(setOf("row"), rt.rows.keys)
        runBlocking { rt.unload("row") }
        assertTrue(rt.rows.isEmpty(), "the row stayed loaded: ${rt.rows.keys}")
        runBlocking { rt.load("row", "row", plugin.name, null, emptyMap(), emptyMap(), emptyList()) }
        assertEquals(2, applications.get(), "the key did not load again")
        runBlocking { rt.dispose() }
    }

    /**
     * A service provided while its row unloads does not outlive the row: after
     * the unload no service, slot or handle of it is left, its name can be
     * provided again, and the first provide's withdrawal does not take the
     * new one away.
     */
    @FrayTest(iterations = 20, resetClassLoaderPerIteration = false)
    fun aServiceProvidedWhileItsRowUnloadsNeverOutlivesIt() {
        val contexts = Collections.synchronizedList(ArrayList<Ctx>())
        val late = AtomicReference<(() -> Unit)?>(null)
        val go = CountDownLatch(1)
        val provided = CountDownLatch(1)
        val plugin = Rutis.define<Rutis.NoConfig>(name = "provides-late") { ctx, _ ->
            contexts.add(ctx)
            Thread {
                go.await()
                late.set(ctx.provide("late", Hello("late")))
                provided.countDown()
            }.start()
        }
        val again = Rutis.define<Rutis.NoConfig>(name = "provides-late-again") { ctx, _ -> contexts.add(ctx) }
        val rt = RuntimeState(listOf(plugin, again))
        // The report line of a provide a row in its unload refuses is not the point here.
        val reported = Collections.synchronizedList(ArrayList<String>())
        val previous = Report.replace { reported.add(it) }
        try {
            runBlocking { rt.load("row", "row", plugin.name, null, emptyMap(), emptyMap(), emptyList()) }
            val unload = Thread { runBlocking { rt.unload("row") } }
            unload.start()
            go.countDown()
            provided.await()
            unload.join()
            assertTrue(rt.rows.isEmpty(), "a row outlived its unload: ${rt.rows.keys}")
            assertTrue(rt.services.isEmpty(), "a service outlived its row: ${rt.services.keys}")
            assertTrue(rt.slots.isEmpty(), "a slot outlived its row: ${rt.slots.keys}")
            assertTrue(
                rt.handles.values.none { it.current },
                "a current handle outlived its row: ${rt.handles.filterValues { it.current }.keys}",
            )
            // The name is free again, and the first provide's withdrawal (if it
            // registered at all) does not take the new registration away.
            runBlocking { rt.load("row", "row", again.name, null, emptyMap(), emptyMap(), emptyList()) }
            val withdrawAgain = contexts[1].provide("late", Hello("again"))
            assertTrue(rt.services.containsKey("late"), "the reloaded row's service is not registered")
            late.get()!!()
            assertTrue(rt.services.containsKey("late"), "the first provide's withdrawal took the new service")
            withdrawAgain()
            assertTrue(rt.services.isEmpty(), "the withdrawal left the service: ${rt.services.keys}")
            runBlocking { rt.unload("row") }
            assertTrue(rt.rows.isEmpty())
        } finally {
            Report.replace(previous)
        }
    }

    /**
     * A load whose plugin fails leaves no row, service or slot behind, and no
     * current handle: the same key and service name load again.
     */
    @FrayTest(iterations = 10, resetClassLoaderPerIteration = false)
    fun aLoadThatFailsLeavesNothingBehind() {
        val fail = AtomicBoolean(true)
        val plugin = Rutis.define<Rutis.NoConfig>(name = "failing-load") { ctx, _ ->
            ctx.provide("hello", Hello("row"))
            if (fail.compareAndSet(true, false)) throw IllegalStateException("apply failed")
        }
        val rt = RuntimeState(listOf(plugin))
        val error = assertFailsWith<IllegalStateException> {
            runBlocking { rt.load("row", "row", plugin.name, null, emptyMap(), emptyMap(), emptyList()) }
        }
        assertEquals("apply failed", error.message)
        assertTrue(rt.rows.isEmpty(), "a row outlived a failed load: ${rt.rows.keys}")
        assertTrue(rt.services.isEmpty(), "a service outlived a failed load: ${rt.services.keys}")
        assertTrue(rt.slots.isEmpty(), "a slot outlived a failed load: ${rt.slots.keys}")
        assertTrue(rt.handles.values.none { it.current }, "a current handle outlived a failed load: ${rt.handles.keys}")
        runBlocking { rt.load("row", "row", plugin.name, null, emptyMap(), emptyMap(), emptyList()) }
        assertTrue(rt.services.containsKey("hello"), "the key did not load again")
        runBlocking { rt.unload("row") }
        assertTrue(rt.services.isEmpty(), "the unload left the service: ${rt.services.keys}")
    }
}
