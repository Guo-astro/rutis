package arcships.rutis

import arcships.rutis.internal.Control
import arcships.rutis.internal.RuntimeState
import kotlinx.coroutines.runBlocking
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertNull

/**
 * A call for a listener this runtime withdrew (#1138): the host may have
 * picked it for a dispatch before `events.off` reached it, and the call
 * answers nothing; an unknown id, and one a row's unload took, still fail.
 */
class EventsTest {
    private suspend fun load(rt: RuntimeState, key: String, plugin: Plugin) =
        rt.load(key, key, plugin.name, null, emptyMap(), emptyMap(), emptyList())

    private suspend fun call(rt: RuntimeState, id: String, name: String = "t/x"): Any? =
        Control.control(
            rt,
            "events.call",
            listOf(
                linkedMapOf<String, Any?>(
                    "listener" to id, "dispatch" to "d", "own" to false, "name" to name,
                    "mode" to "serial", "args" to emptyList<Any?>(),
                ),
            ),
        )

    @Test
    fun aListenerWithdrawnHereAnswersACallAlreadyOnItsWayWithNothing() = runBlocking<Unit> {
        lateinit var off: () -> Unit
        val plugin = Rutis.define<Rutis.NoConfig>(name = "gone") { ctx, _ ->
            off = ctx.on("t/x") { "A" }
            ctx.on("t/y") { "Y" }
        }
        val rt = RuntimeState(listOf(plugin))
        load(rt, "a", plugin)
        assertEquals("A", call(rt, "1"))
        off()
        assertNull(call(rt, "1"))
        val unknown = assertFailsWith<RutisException> { call(rt, "9") }
        assertEquals("listener 9 of t/x is withdrawn", unknown.message)
        // The host withdrew these itself on the row's unload: the failure it
        // was.
        rt.unload("a")
        val unloaded = assertFailsWith<RutisException> { call(rt, "2", name = "t/y") }
        assertEquals("listener 2 of t/y is withdrawn", unloaded.message)
    }

    @Test
    fun aRestartWithdrawnHereAnswersACallAlreadyOnItsWayWithNothing() = runBlocking<Unit> {
        val plugin = Rutis.define<Rutis.NoConfig>(name = "restarted") { ctx, _ -> ctx.on("t/x") { "A" } }
        val rt = RuntimeState(listOf(plugin))
        load(rt, "b", plugin)
        // `rows.update` keeps the row loaded on the host, whose table still
        // has the old listener until `events.off`.
        rt.update("b", null)
        assertNull(call(rt, "1"))
    }
}
