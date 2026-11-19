package arcships.rutis.internal.peer

import arcships.rutis.RutisException
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive

// The fields each frame op defines (#635). A field outside them is not a
// session error: a call frame fails only its call, a greeting fails the
// handshake, and `release`/`cancel`, which have no call to fail, ignore the
// extras (session protocol §9).

private val FRAME_FIELDS: Map<String, List<String>> = mapOf(
    "hello" to listOf("op", "version", "endpoint", "implementation", "capabilities"),
    "invoke" to listOf("op", "id", "path", "target", "method", "args", "sync"),
    "call" to listOf("op", "id", "path", "reference", "method", "args", "sync"),
    "get" to listOf("op", "id", "path", "reference", "property", "sync"),
    "await" to listOf("op", "id", "path", "reference", "sync"),
    "return" to listOf("op", "id", "value"),
    "throw" to listOf("op", "id", "error"),
    "release" to listOf("op", "reference", "count"),
    "cancel" to listOf("op", "id"),
    "ack" to listOf("op", "id"),
)

/**
 * The field paths `frame` carries beyond the ones its `op` defines, sorted:
 * a field nested in a value is named by its path (`args.value.x`). `data`
 * values and error graphs are arbitrary JSON and are not inspected. An
 * unknown `op` has no table and is left to the usual refusal.
 */
internal fun unknownFields(frame: JsonObject): List<String> {
    val op = (frame["op"] as? JsonPrimitive)?.takeIf { it.isString }?.content ?: return emptyList()
    val allowed = FRAME_FIELDS[op] ?: return emptyList()
    val out = ArrayList<String>()
    for (key in frame.keys.sorted()) {
        if (key !in allowed) out.add(key)
    }
    when (op) {
        "hello" -> frame["implementation"]?.let { unknownObjectFields(it, listOf("name", "version"), "implementation", out) }
        "invoke", "call", "get", "await" -> frame["args"]?.let { unknownValueFields(it, "args", out) }
        "return" -> frame["value"]?.let { unknownValueFields(it, "value", out) }
        "throw" -> frame["error"]?.let { unknownObjectFields(it, listOf("name", "message", "graph"), "error", out) }
    }
    return out
}

private fun unknownObjectFields(value: JsonElement, allowed: List<String>, path: String, out: MutableList<String>) {
    val entries = value as? JsonObject ?: return
    for (key in entries.keys.sorted()) {
        if (key !in allowed) out.add("$path.$key")
    }
}

/** A `WireValue`: `{type, value}`, with further fields only in a reference. */
private fun unknownValueFields(value: JsonElement, path: String, out: MutableList<String>) {
    val wire = value as? JsonObject ?: return
    for (key in wire.keys.sorted()) {
        if (key != "type" && key != "value") out.add("$path.$key")
    }
    val type = (wire["type"] as? JsonPrimitive)?.takeIf { it.isString }?.content ?: return
    val content = wire["value"] ?: return
    when (type) {
        "list", "set" -> (content as? JsonArray)?.forEachIndexed { index, item ->
            unknownValueFields(item, "$path.value[$index]", out)
        }
        "map" -> (content as? JsonArray)?.forEachIndexed { index, pair ->
            (pair as? JsonArray)?.forEachIndexed { part, item ->
                unknownValueFields(item, "$path.value[$index][$part]", out)
            }
        }
        "record" -> (content as? JsonObject)?.let { entries ->
            for (name in entries.keys.sorted()) unknownValueFields(entries.getValue(name), "$path.value.$name", out)
        }
        "reference" -> unknownObjectFields(content, listOf("id", "home", "kind", "origin"), "$path.value", out)
    }
}

/** `fields` as an error says them: `"x", "y"`. */
internal fun quotedFields(fields: List<String>): String = fields.joinToString(", ") { "\"$it\"" }

/**
 * The error a call gets when its frame carried a field this side does not
 * know (#635): code `bad-frame`, data `{op, fields}`.
 */
internal fun badFrame(op: String, fields: List<String>): RutisException =
    RutisException(
        RutisException.BINDING_ERROR,
        "unknown field ${quotedFields(fields)} in $op",
        "bad-frame",
        mapOf("op" to op, "fields" to fields),
    )
