package arcships.rutis.conformance

import arcships.rutis.RutisException
import arcships.rutis.RutisService
import arcships.rutis.conformance.plugins.conformanceEvents
import arcships.rutis.testing.Loaded
import arcships.rutis.testing.RutisTest
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.booleanOrNull
import kotlinx.serialization.json.jsonArray
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import java.util.concurrent.CompletableFuture
import java.util.concurrent.TimeUnit
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.fail

/** The language of the Kotlin testing tool in the file. */
private const val LANGUAGE = "kotlin"

/** How long a log expectation may take to hold (events.rs SETTLE). */
private const val SETTLE = 5_000L

/** How long `logLacks` / `reported` wait after the rest held (events.rs LACKS). */
private const val LACKS = 300L

/** events-log, as the test gives it: add(entry) and filter(description), both sync. */
@RutisService
class EventLog {
    private val entries = ArrayList<String>()

    fun add(entry: String) {
        synchronized(entries) { entries.add(entry) }
    }

    /** Logs `filter:<row>` of the description. */
    fun filter(description: Map<String, Any?>) = add("filter:" + description["row"])

    internal fun list(): List<String> = synchronized(entries) { entries.toList() }
}

private class Case(val raw: JsonObject) {
    val case: String = raw["case"]!!.jsonPrimitive.content
    val name: String = raw["name"]!!.jsonPrimitive.content
    val listeners: List<JsonObject> = raw["listeners"]?.jsonArray?.map { it.jsonObject } ?: emptyList()
    val steps: List<JsonObject> = raw["steps"]?.jsonArray?.map { it.jsonObject } ?: emptyList()
    private val only: List<JsonObject> = raw["only"]?.jsonArray?.map { it.jsonObject } ?: emptyList()
    private val except: List<JsonObject> = raw["except"]?.jsonArray?.map { it.jsonObject } ?: emptyList()
    private val tool: Boolean = (raw["tool"] as? JsonPrimitive)?.booleanOrNull ?: true

    /** The emitter rows of a step: its emit's, or its concurrent ones'. */
    private fun emitRows(step: JsonObject): List<String> {
        (step["emit"] as? JsonObject)?.let { return listOf((it["row"] as? JsonPrimitive)?.content ?: "e") }
        return step["concurrent"]?.jsonArray?.flatMap { emitRows(it.jsonObject) } ?: emptyList()
    }

    /** The labels of the case (events.rs `places`): its listeners, loaded participants and emitter rows. */
    private fun labels(): Set<String> {
        val labels = HashSet<String>()
        listeners.forEach { labels.add(it["label"]!!.jsonPrimitive.content) }
        for (step in steps) {
            (step["load"] as? JsonObject)?.let { labels.add(it["label"]!!.jsonPrimitive.content) }
            labels.addAll(emitRows(step))
        }
        return labels
    }

    /** Whether a limit (an only / except item) holds with every label at X in Kotlin (events.rs `limit_holds`). */
    private fun holds(limit: JsonObject, labels: Set<String>): Boolean = limit.entries.filter { it.key != "why" }.all { (key, list) ->
        (key == "X" || key == "Y" || key in labels) && list.jsonArray.any { it.jsonPrimitive.content == LANGUAGE }
    }

    /**
     * Whether the case places a row in a scope: the testing tool has no
     * scopes (docs/design-platform-cleanup-2026-10-28.md §7.5); rutis-loader's
     * events_conformance runs those cases.
     */
    fun scoped(): Boolean {
        fun at(o: JsonObject?) = (o?.get("scope") as? JsonPrimitive)?.content.let { it != null && it != "" && it != "root" }
        fun step(o: JsonObject): Boolean = at(o["emit"] as? JsonObject) || at(o["load"] as? JsonObject) ||
            (o["concurrent"]?.jsonArray?.any { step(it.jsonObject) } ?: false)
        return listeners.any { at(it) } || steps.any(::step)
    }

    /** Whether the case runs on the Kotlin testing tool, and why not (events.rs `tool_combo`). */
    fun forKotlin(): String? {
        if (!tool) return "tool: false"
        val labels = labels()
        only.firstOrNull { !holds(it, labels) }?.let { return "only: ${it["why"]}" }
        except.firstOrNull { holds(it, labels) }?.let { return "except: ${it["why"]}" }
        return null
    }
}

/** One case on one harness. */
private class Run(val c: Case, val log: EventLog, val problems: MutableList<String>) {
    lateinit var root: Loaded
    val rows = HashMap<String, Loaded>()
    val participants = HashMap<String, Map<String, Any?>>()
    var generation = 0
    var pending: CompletableFuture<Result<Any?>>? = null
    var reported = 0

    /** Loads a listener row. */
    fun load(raw: JsonObject) {
        @Suppress("UNCHECKED_CAST")
        val listener = (arcships.rutis.internal.Native.toNative(raw) as Map<String, Any?>).toMutableMap()
        if ("name" !in listener) listener["name"] = c.name
        val label = listener["label"] as String
        rows[label] = root.load(listener, row = label)
        participants[label] = listener
    }

    /** Changes the row `label`'s config (a new generation), so it registers its listeners again. */
    fun update(label: String) {
        val participant = participants[label] ?: fail("no listener row $label to update")
        generation++
        val config = participant + ("generation" to generation.toLong())
        rows[label]!!.update(config)
        participants[label] = config
    }

    /** The emitter row of `label`, created on first use. */
    fun emitter(label: String): Pair<Loaded, String> {
        val row = rows.getOrPut(label) { root.load(mapOf("label" to label, "emitter" to true), row = label) }
        return row to label
    }

    fun emit(spec: JsonObject): Result<Any?> {
        val row = (spec["row"] as? JsonPrimitive)?.content ?: "e"
        val (loaded, key) = emitter(row)
        val mode = spec["mode"]!!.jsonPrimitive.content
        val request = linkedMapOf<String, Any?>(
            "mode" to JsonPrimitive(mode),
            "name" to JsonPrimitive((spec["name"] as? JsonPrimitive)?.content ?: c.name),
            "args" to (spec["args"]?.jsonArray?.map(::fromWritten) ?: emptyList()),
        )
        spec["filter"]?.let { request["filter"] = it }
        spec["terminal"]?.let { request["terminal"] = it }
        spec["cancel"]?.let { request["cancel"] = it }
        val method = if (mode == "bail") "dispatchSync" else "dispatch"
        return runBlocking {
            runCatching { withTimeout(10_000) { loaded.service("emitter-$key").raw(method, listOf(request)) } }
        }
    }

    private fun matches(error: Throwable, want: JsonObject): String? {
        if (error !is RutisException) return "not an error of the plugin: $error"
        val name = (want["name"] as? JsonPrimitive)?.content
        val message = (want["message"] as? JsonPrimitive)?.content
        if (!name.isNullOrEmpty() && error.name != name) return "name ${error.name}, want $name ($error)"
        if (!message.isNullOrEmpty() && !error.message.contains(message)) return "message ${error.message}, want $message"
        val errors = want["errors"] as? JsonArray ?: return null
        if (error.errors.size != errors.size) return "errors of $error, want ${errors.size}"
        for ((i, child) in errors.withIndex()) matches(error.errors[i], child.jsonObject)?.let { return "errors[$i]: $it" }
        return null
    }

    /** `returns` and `throws` against the dispatches that end in a step (events.rs `check_outcomes`). */
    fun checkOutcomes(where: String, e: JsonObject, outcomes: List<Result<Any?>>) {
        if ((e["cancelled"] as? JsonPrimitive)?.booleanOrNull == true) {
            problems.add("$where: cancelled: the testing tool does not give up a dispatch")
            return
        }
        val returns = e["returns"]
        val throws = e["throws"] as? JsonObject
        if ((returns != null || throws != null) && outcomes.isEmpty()) {
            problems.add("$where: expects a result, but no dispatch ends in this step")
            return
        }
        for (got in outcomes) {
            if (throws == null && got.isFailure) {
                problems.add("$where: expected the dispatch to succeed, it threw ${got.exceptionOrNull()}")
                continue
            }
            if (returns != null && got.isSuccess) {
                val have = written(got.getOrNull())
                if (have != normalized(returns)) problems.add("$where: got $have, want $returns")
            }
        }
        if (throws != null) {
            var problem: String? = "expected an error, got $outcomes"
            for (got in outcomes) {
                val error = got.exceptionOrNull() ?: continue
                problem = matches(error, throws)
                if (problem == null) break
            }
            if (problem != null) problems.add("$where: $problem")
        }
    }

    /**
     * The error name of a §5.6 line rutis reported about `event` naming
     * `row`, or null: `<row>: listener of <event> failed: …`, `filter of
     * <event> from row <row> failed: …`.
     */
    private fun reports(line: String, event: String, filter: Boolean, row: String): String? {
        val prefix = if (filter) "filter of $event from row $row failed: " else "$row: listener of $event failed: "
        if (!line.startsWith(prefix)) return null
        val rest = line.removePrefix(prefix)
        val colon = rest.indexOf(": ")
        return if (colon < 0) null else rest.substring(0, colon)
    }

    private fun strings(e: JsonObject, key: String): List<String>? = (e[key] as? JsonArray)?.map { it.jsonPrimitive.content }

    /** What of a step's log expectations does not hold now, or null (events.rs `log_holds`). */
    fun logProblems(e: JsonObject, event: String): String? {
        val log = this.log.list()
        val out = ArrayList<String>()
        strings(e, "log")?.let { if (log != it) out.add("log $log, want $it") }
        strings(e, "logUnordered")?.let { if (log.sorted() != it.sorted()) out.add("log $log, want in any order $it") }
        strings(e, "logContains")?.let { want ->
            var i = 0
            for (entry in log) if (i < want.size && entry == want[i]) i++
            if (i != want.size) out.add("log $log, want it to contain $want")
        }
        strings(e, "logLacks")?.forEach { if (it in log) out.add("log $log has $it") }
        strings(e, "filterSees")?.let { want ->
            val seen = log.filter { it.startsWith("filter:") }.map { it.removePrefix("filter:") }
            if (seen != want) out.add("the filter saw $seen, want $want")
        }
        val lines = root.reported().drop(reported)
        for (want in (e["reported"] as? JsonArray)?.map { it.jsonObject } ?: emptyList()) {
            val label = want["label"]!!.jsonPrimitive.content
            val name = want["name"]!!.jsonPrimitive.content
            val filter = (want["filter"] as? JsonPrimitive)?.booleanOrNull ?: false
            val about = lines.filter { reports(it, event, filter, label) != null }
            when {
                about.size != 1 -> out.add("reported $lines: want one line for $label about $event, got ${about.size}")
                reports(about[0], event, filter, label) != name -> out.add("reported ${about[0]}: $label should fail with $name")
            }
        }
        return if (out.isEmpty()) null else out.joinToString("; ")
    }

    /** Checks a step's log expectations until they hold; with `logLacks` or `reported`, once more a moment later. */
    fun checkLog(where: String, e: JsonObject, event: String) {
        val deadline = System.currentTimeMillis() + SETTLE
        while (true) {
            val problem = logProblems(e, event) ?: break
            if (System.currentTimeMillis() > deadline) {
                problems.add("$where: $problem")
                return
            }
            Thread.sleep(10)
        }
        if (e["logLacks"] != null || e["reported"] != null) {
            Thread.sleep(LACKS)
            logProblems(e, event)?.let { problems.add("$where: $it (after the step had settled)") }
        }
    }

    fun waitFor(label: String) {
        val deadline = System.currentTimeMillis() + SETTLE
        while (label !in log.list()) {
            if (System.currentTimeMillis() > deadline) fail("$label did not start: ${log.list()}")
            Thread.sleep(5)
        }
    }

    fun step(i: Int, step: JsonObject) {
        val where = "${c.case} step $i"
        val e = step["expect"] as? JsonObject ?: JsonObject(emptyMap())
        val emit = step["emit"] as? JsonObject
        val event = (emit?.get("name") as? JsonPrimitive)?.content ?: c.name
        val outcomes = ArrayList<Result<Any?>>()
        val started = System.currentTimeMillis()
        when {
            emit != null && (step["background"] as? JsonPrimitive)?.booleanOrNull == true -> {
                pending = CompletableFuture.supplyAsync { emit(emit) }
                checkLog(where, e, event)
                return
            }
            emit != null -> outcomes.add(emit(emit))
            step["load"] != null -> load(step["load"]!!.jsonObject)
            step["update"] != null -> update(step["update"]!!.jsonPrimitive.content)
            step["unload"] != null -> {
                (step["when"] as? JsonPrimitive)?.content?.let(::waitFor)
                rows[step["unload"]!!.jsonPrimitive.content]!!.unload()
                pending?.let {
                    pending = null
                    try {
                        outcomes.add(it.get(SETTLE, TimeUnit.MILLISECONDS))
                    } catch (_: Exception) {
                        problems.add("$where: the background dispatch did not settle within ${SETTLE}ms")
                    }
                }
                if ((e["settles"] as? JsonPrimitive)?.booleanOrNull == true && System.currentTimeMillis() - started >= SETTLE) {
                    problems.add("$where: did not settle within ${SETTLE}ms")
                }
            }
            else -> fail("$where: a step the testing tool cannot take: $step")
        }
        checkOutcomes(where, e, outcomes)
        checkLog(where, e, event)
    }
}

/**
 * The testing tool against the event expectations (#464 §13): the real
 * runtimes and the testing tools are checked against the same list.
 */
class EventsConformanceTest {
    @Test
    fun theTestingToolMeetsTheEventExpectations() {
        val expected = expectations("events.expected.json")
        val toolCases = expected["toolCases"]!!.jsonObject[LANGUAGE]?.jsonPrimitive?.content?.toInt()
        var ran = 0
        var scoped = 0
        val failures = ArrayList<String>()
        for (raw in expected["cases"]!!.jsonArray) {
            val c = Case(raw.jsonObject)
            if (c.forKotlin() != null) continue
            if (c.scoped()) {
                scoped++
                continue
            }
            ran++
            val log = EventLog()
            val problems = ArrayList<String>()
            val run = Run(c, log, problems)
            try {
                RutisTest.load(conformanceEvents, mapOf("label" to "e", "emitter" to true), mapOf("events-log" to log), row = "e").use { root ->
                    run.root = root
                    run.rows["e"] = root
                    run.reported = root.reported().size
                    c.listeners.forEach(run::load)
                    c.steps.forEachIndexed(run::step)
                }
            } catch (e: Throwable) {
                problems.add("${c.case}: $e")
            }
            if (problems.isNotEmpty()) failures.add(problems.joinToString("\n"))
        }
        if (failures.isNotEmpty()) fail("${failures.size} cases failed:\n" + failures.joinToString("\n"))
        // The skip logic agrees with events.rs `tool_combo`; the cases with
        // scopes are rutis-loader's.
        assertEquals(toolCases, ran + scoped, "the cases the file gives $LANGUAGE")
    }
}
