package arcships.rutis.internal.peer

import arcships.rutis.HostGoneException
import arcships.rutis.RutisException
import arcships.rutis.internal.Native
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.intOrNull
import kotlinx.serialization.json.jsonArray
import kotlinx.serialization.json.put
import java.math.BigInteger
import java.util.IdentityHashMap
import java.util.concurrent.CancellationException

// Errors cross as their error graph (as Node encodes it): the root's name and
// message, and nodes `{type: "error", name, message, stack, cause?, errors?,
// code?, data?}`, `{type: "array", values}`, `{type: "object", values: [[key,
// value], ...]}`; graph values `{type: "data" | "undefined" | "number" |
// "bigint" | <tag> | "reference", value}`.

/** The name a throwable crosses with. */
fun errorName(error: Throwable): String = when (error) {
    is RutisException -> error.name
    is HostGoneException -> RutisException.BINDING_ERROR
    is CancellationException -> "CancellationException"
    // The SDK's value errors cross as plain errors, as in the other languages.
    is ValueException -> "Error"
    else -> error.javaClass.simpleName.ifEmpty { "Error" }
}

/** The message a throwable crosses with. */
fun errorMessage(error: Throwable): String = when (error) {
    is RutisException -> error.message
    else -> error.message ?: ""
}

/**
 * A throwable's `code`: its public `code: String` property, read through its
 * getter (`getCode()`), as a plain Kotlin exception declares it.
 */
private fun codeOf(error: Throwable): String? {
    if (error is RutisException) return error.code
    if (error is ValueException) return error.code
    return try {
        error.javaClass.getMethod("getCode").invoke(error) as? String
    } catch (_: ReflectiveOperationException) {
        null
    } catch (_: SecurityException) {
        null
    }
}

/** A throwable's `data` property (`getData()`), whether it has one. */
private fun dataOf(error: Throwable): Pair<Boolean, Any?> {
    if (error is RutisException) return (error.data != null) to error.data
    if (error is ValueException) return (error.data != null) to error.data
    return try {
        true to error.javaClass.getMethod("getData").invoke(error)
    } catch (_: ReflectiveOperationException) {
        false to null
    } catch (_: SecurityException) {
        false to null
    }
}

/** Frames of the SDK's own dispatch: where a plugin's method was called from. */
private fun isDispatch(frame: StackTraceElement): Boolean {
    val c = frame.className
    return c.startsWith("arcships.rutis.internal.") ||
        c.startsWith("arcships.rutis.ServiceObject") ||
        c.startsWith("arcships.rutis.LocalHandle") ||
        c.startsWith("arcships.rutis.HostHandle") ||
        c.startsWith("arcships.rutis.ServiceTable") ||
        c.contains("_RutisService")
}

/** Frames of the runtime under any call: coroutine resumption, reflection, threads. */
private fun isMachinery(frame: StackTraceElement): Boolean {
    val c = frame.className
    return isDispatch(frame) ||
        c.startsWith("kotlin.coroutines.") || c.startsWith("kotlinx.coroutines.") ||
        c.startsWith("java.lang.reflect.") || c.startsWith("jdk.internal.reflect.") ||
        c.startsWith("sun.reflect.") || c.startsWith("java.util.concurrent.") ||
        c == "java.lang.Thread" || c.startsWith("kotlin.reflect.")
}

/**
 * A throwable's frames as they cross: without the SDK's dispatch below the
 * plugin's method and the runtime under it. Null when none of them is a frame
 * of the plugin's own.
 */
private fun ownFrames(error: Throwable): List<StackTraceElement>? {
    val frames = error.stackTrace.toList()
    // The first dispatch frame below a frame of the plugin's own.
    val firstOwn = frames.indexOfFirst { !isMachinery(it) }
    if (firstOwn < 0) return null
    var end = frames.size
    val cut = (firstOwn + 1 until frames.size).firstOrNull { isDispatch(frames[it]) }
    if (cut != null) end = cut
    while (end > firstOwn + 1 && isMachinery(frames[end - 1])) end--
    return frames.subList(0, end)
}

/**
 * The `stack` an error crosses with: "Name: message" and the throwable's
 * frames as `    at <frame>` lines, as the JVM prints them, without the
 * SDK's dispatch below the plugin's method and the runtime under it.
 */
internal fun stackOf(name: String, message: String, error: Throwable): String {
    val frames = ownFrames(error) ?: error.stackTrace.toList()
    return buildString {
        append(name).append(": ").append(message)
        for (frame in frames) append("\n    at ").append(frame)
    }
}

/** At most this many `rethrown` items on an error node; no more are appended. */
private const val MAX_RETHROWN = 8

/**
 * The root node of a received graph thrown on again here, with this rethrow's
 * frames (`    at <frame>` lines as in [stackOf], without "Name: message")
 * appended to its
 * `rethrown`; unchanged when there are no frames of our own or already 8 items.
 */
private fun withRethrown(node: JsonObject, error: Throwable): JsonObject {
    if (node["type"] != JsonPrimitive("error")) return node
    // Its frames from the first of our own: where the SDK handed it over
    // (the wait for the reply) is not where it was thrown on.
    val frames = ownFrames(error)?.dropWhile { isMachinery(it) }
    if (frames.isNullOrEmpty()) return node
    val previous = when (val r = node["rethrown"]) {
        null, JsonNull -> emptyList()
        is JsonArray -> r
        else -> return node
    }
    if (previous.size >= MAX_RETHROWN) return node
    val item = frames.joinToString("\n") { "    at $it" }
    return JsonObject(node + ("rethrown" to JsonArray(previous + JsonPrimitive(item))))
}

private fun reference(id: Int): JsonObject = buildJsonObject {
    put("type", "reference")
    put("value", id)
}

private fun shift(value: JsonElement, offset: Int): JsonElement = when (value) {
    is JsonObject -> if (value["type"] == JsonPrimitive("reference") && value["value"] is JsonPrimitive) {
        val id = (value["value"] as JsonPrimitive).intOrNull
        if (id != null) reference(id + offset) else value
    } else {
        JsonObject(value.mapValues { shift(it.value, offset) })
    }
    is JsonArray -> JsonArray(value.map { shift(it, offset) })
    else -> value
}

/**
 * The wire form of `error`: its error graph (causes, an aggregate's children
 * in order). A received [RutisException] crosses with the far end's graph
 * spliced in, so that it crosses again whole.
 */
fun encodeError(error: Throwable): JsonObject {
    if (error is RutisException && error.name == RutisException.SYNC_WAIT_CYCLE && error.graph == null) {
        return buildJsonObject {
            put("name", error.name)
            put("message", error.message)
        }
    }
    val nodes = ArrayList<JsonElement?>()
    val seen = IdentityHashMap<Throwable, Int>()
    fun visit(error: Throwable, depth: Int): JsonObject {
        seen[error]?.let { return reference(it) }
        val graph = (error as? RutisException)?.graph
        if (graph != null) {
            val root = graph["root"]
            val graphNodes = graph["nodes"]
            if (root is JsonObject && graphNodes is JsonArray) {
                val offset = nodes.size
                for (node in graphNodes) nodes.add(shift(node, offset))
                val shifted = shift(root, offset) as JsonObject
                // Thrown on whole: the caller's frames go on the root's
                // `rethrown`, in a copy (the received graph stays as it came).
                if (depth == 0) {
                    val id = (shifted["value"] as? JsonPrimitive)?.intOrNull
                    if (shifted["type"] == JsonPrimitive("reference") && id != null && id in offset until nodes.size) {
                        (nodes[id] as? JsonObject)?.let { nodes[id] = withRethrown(it, error) }
                    }
                }
                return shifted
            }
        }
        val name = errorName(error)
        val message = errorMessage(error)
        val id = nodes.size
        seen[error] = id
        nodes.add(null)
        val node = LinkedHashMap<String, JsonElement>()
        node["type"] = JsonPrimitive("error")
        node["name"] = JsonPrimitive(name)
        node["message"] = JsonPrimitive(message)
        node["stack"] = JsonPrimitive(stackOf(name, message, error))
        // A code and its data cross, as from JavaScript and Rust (#440): data
        // that cannot cross is left out, nothing of it kept.
        codeOf(error)?.let { node["code"] = JsonPrimitive(it) }
        val (hasData, data) = dataOf(error)
        if (hasData) {
            val mark = nodes.size
            try {
                node["data"] = graphValue(Native.fromNative(data), nodes, 0)
            } catch (_: Exception) {
                while (nodes.size > mark) nodes.removeAt(nodes.size - 1)
            }
        }
        val cause = error.cause
        if (cause != null && cause !== error && depth < 64) node["cause"] = visit(cause, depth + 1)
        if (error is RutisException && (error.name == RutisException.AGGREGATE_ERROR || error.errors.isNotEmpty())) {
            node["errors"] = JsonArray(error.errors.map { visit(it, depth + 1) })
        }
        nodes[id] = JsonObject(node)
        return reference(id)
    }
    val root = visit(error, 0)
    return buildJsonObject {
        put("name", errorName(error))
        put("message", errorMessage(error))
        put("graph", buildJsonObject {
            put("root", root)
            put("nodes", JsonArray(nodes.map { it ?: JsonNull }))
        })
    }
}

/** A session value (an error's `data`) as an error graph value. */
private fun graphValue(value: Any?, nodes: MutableList<JsonElement?>, depth: Int): JsonObject {
    if (depth > 64) throw ValueException("too deep")
    fun tagged(tag: String, text: String) = buildJsonObject {
        put("type", tag)
        put("value", text)
    }
    fun array(items: List<Any?>): JsonObject {
        val id = nodes.size
        nodes.add(null)
        val values = JsonArray(items.map { graphValue(it, nodes, depth + 1) })
        nodes[id] = buildJsonObject {
            put("type", "array")
            put("values", values)
        }
        return reference(id)
    }
    fun objectOf(entries: List<Pair<String, Any?>>): JsonObject {
        val id = nodes.size
        nodes.add(null)
        val values = JsonArray(entries.sortedBy { it.first }.map { (key, item) ->
            JsonArray(listOf(JsonPrimitive(key), graphValue(item, nodes, depth + 1)))
        })
        nodes[id] = buildJsonObject {
            put("type", "object")
            put("values", values)
        }
        return reference(id)
    }
    return when (value) {
        Undefined -> buildJsonObject { put("type", "undefined") }
        null, JsonNull -> buildJsonObject {
            put("type", "data")
            put("value", JsonNull)
        }
        is JsonArray -> array(value)
        is JsonObject -> objectOf(value.entries.map { it.key to it.value })
        is JsonPrimitive -> {
            if (!value.isString) {
                val integer = value.content.toBigIntegerOrNull()
                if (integer != null && integer.abs() > BigInteger.valueOf(MAX_SAFE)) return tagged("bigint", integer.toString())
            }
            buildJsonObject {
                put("type", "data")
                put("value", value)
            }
        }
        is Double -> if (value.isNaN() || value.isInfinite()) {
            buildJsonObject {
                put("type", "number")
                put("value", if (value.isNaN()) "NaN" else if (value > 0) "Infinity" else "-Infinity")
            }
        } else {
            buildJsonObject {
                put("type", "data")
                put("value", value)
            }
        }
        is String, is Boolean, is Long, is Int -> buildJsonObject {
            put("type", "data")
            put("value", plain(value))
        }
        is List<*> -> array(value)
        is Map<*, *> -> objectOf(value.entries.map { (it.key as String) to it.value })
        is WireMap -> {
            checkKeys(value.entries.map { it.first }, "map key")
            buildJsonObject {
                put("type", "map")
                put("value", JsonArray(value.entries.map { (k, v) ->
                    JsonArray(listOf(graphValue(k, nodes, depth + 1), graphValue(v, nodes, depth + 1)))
                }))
            }
        }
        is WireSet -> {
            checkKeys(value.elements, "set element")
            buildJsonObject {
                put("type", "set")
                put("value", JsonArray(value.elements.map { graphValue(it, nodes, depth + 1) }))
            }
        }
        else -> {
            val tag = tagOf(value) ?: throw ValueException("${value.javaClass.name} cannot be error data")
            tagged(tag, taggedText(value))
        }
    }
}

/** An error received in a `throw` frame. */
fun decodeError(raw: JsonElement?): RutisException {
    val failure = raw as? JsonObject ?: return RutisException("Error", "malformed error")
    val name = text(failure["name"]) ?: "Error"
    val message = text(failure["message"]) ?: ""
    val graph = failure["graph"] as? JsonObject ?: return RutisException(name, message)
    val nodes = graph["nodes"] as? JsonArray ?: return RutisException(name, message).also { it.graph = graph }
    val reader = GraphReader(nodes)
    val rootId = ((graph["root"] as? JsonObject)?.get("value") as? JsonPrimitive)?.intOrNull
    val root = if (rootId != null && reader.node(rootId)?.get("type") == JsonPrimitive("error")) {
        val decoded = reader.error(rootId, 0)
        RutisException(name, message, decoded.code, decoded.data, decoded.cause, decoded.errors)
    } else {
        RutisException(name, message)
    }
    root.graph = graph
    return root
}

/** Reads the values and errors of an error graph's nodes. */
private class GraphReader(val nodes: JsonArray) {
    private val built = HashMap<Int, RutisException>()

    fun node(id: Int?): JsonObject? = if (id == null || id < 0 || id >= nodes.size) null else nodes[id] as? JsonObject

    fun read(value: JsonElement?, depth: Int): Any? {
        val v = value as? JsonObject ?: throw ValueException("invalid graph value")
        if (depth > 64) throw ValueException("too deep")
        return when (val type = (v["type"] as? JsonPrimitive)?.content) {
            "undefined" -> null
            "data" -> Native.toNative(v["value"] ?: JsonNull)
            "number" -> when (val t = (v["value"] as? JsonPrimitive)?.content) {
                "NaN" -> Double.NaN
                "Infinity" -> Double.POSITIVE_INFINITY
                "-Infinity" -> Double.NEGATIVE_INFINITY
                else -> t?.toDouble() ?: throw ValueException("invalid number")
            }
            "map", "set" -> {
                val items = v["value"]?.jsonArray ?: throw ValueException("invalid $type")
                if (type == "set") {
                    val elements = items.map { Native.fromNative(read(it, depth + 1)) }
                    checkKeys(elements, "set element")
                    Native.toNative(WireSet(elements))
                } else {
                    val entries = items.map {
                        val pair = it.jsonArray
                        if (pair.size != 2) throw ValueException("invalid map")
                        Native.fromNative(read(pair[0], depth + 1)) to Native.fromNative(read(pair[1], depth + 1))
                    }
                    checkKeys(entries.map { it.first }, "map key")
                    Native.toNative(WireMap(entries))
                }
            }
            "reference" -> {
                val id = (v["value"] as? JsonPrimitive)?.intOrNull
                val target = node(id) ?: throw ValueException("invalid reference")
                when ((target["type"] as? JsonPrimitive)?.content) {
                    "array" -> (target["values"]?.jsonArray ?: JsonArray(emptyList())).map { read(it, depth + 1) }
                    "object" -> LinkedHashMap<String, Any?>().apply {
                        for (entry in target["values"]?.jsonArray ?: JsonArray(emptyList())) {
                            val pair = entry.jsonArray
                            put((pair[0] as JsonPrimitive).content, read(pair[1], depth + 1))
                        }
                    }
                    "error" -> error(id!!, depth + 1)
                    else -> throw ValueException("invalid node")
                }
            }
            null -> throw ValueException("invalid graph value")
            else -> {
                if (type !in TAGS) throw ValueException("invalid graph value")
                val text = (v["value"] as? JsonPrimitive)?.takeIf { it.isString }?.content ?: throw ValueException("invalid $type")
                Native.toNative(parseTagged(type, text))
            }
        }
    }

    fun error(id: Int, depth: Int): RutisException {
        built[id]?.let { return it }
        val n = node(id) ?: return RutisException("Error", "malformed error")
        val nodeName = text(n["name"]) ?: "Error"
        val nodeMessage = text(n["message"]) ?: ""
        val code = (n["code"] as? JsonPrimitive)?.takeIf { it.isString }?.content
        val data = n["data"]?.let { runCatching { read(it, depth + 1) }.getOrNull() }
        val cause = if (depth < 64) n["cause"]?.let { runCatching { read(it, depth + 1) as? Throwable }.getOrNull() } else null
        val children = (n["errors"] as? JsonArray)?.map {
            runCatching { read(it, depth + 1) as? Throwable }.getOrNull() ?: RutisException("Error", "malformed error")
        } ?: emptyList()
        val error = RutisException(nodeName, nodeMessage, code, data, cause, children)
        // The graph rooted at this node: thrown on, it crosses whole.
        error.graph = buildJsonObject {
            put("root", reference(id))
            put("nodes", nodes)
        }
        built[id] = error
        return error
    }
}

private fun text(element: JsonElement?): String? = when (element) {
    null, JsonNull -> null
    is JsonPrimitive -> element.content
    else -> element.toString()
}
